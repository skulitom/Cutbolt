"""Original signals, external EQ/loudness references and closed-form compressor checks."""
from engine import ENGINE
from array import array
import argparse
from fractions import Fraction
import hashlib
import json
import math
from pathlib import Path
import re
import struct
import subprocess
import wave

from agents import Client
from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]
RATE = 48000


def round_sample(value):
    return max(-32768, min(32767, math.floor(abs(value) + 0.5) * (-1 if value < 0 else 1)))


def ffmpeg_equalize(values, effects):
    filters = []
    for e in effects:
        kind = {"low_pass": "lowpass", "high_pass": "highpass", "peaking": "equalizer"}[e["type"]]
        filters.append(f"{kind}=f={e['frequency_hz']}:t=q:w={e['q']}:precision=f64" + (f":g={e['gain_db']}" if kind == "equalizer" else ""))
    raw = b"".join(struct.pack("<dd", *v) for v in values)
    result = subprocess.run(["ffmpeg", "-v", "error", "-f", "f64le", "-ar", "48000", "-ac", "2", "-i", "pipe:0", "-af", ",".join(filters), "-f", "f64le", "pipe:1"], input=raw, capture_output=True, check=True, timeout=60)
    return list(struct.iter_unpack("<dd", result.stdout))


def compressor_steps(segments, settings):
    """Closed-form step response; no recursive sample-by-sample gain update."""
    result = []
    initial = 0.0
    for samples, left, right in segments:
        peak = max(abs(left), abs(right)) / 32768
        over = max(0, 20 * math.log10(peak) - settings["threshold_db"]) if peak else 0
        target = -over * (1 - 1/settings["ratio"])
        ms = settings["attack_ms"] if target < initial else settings["release_ms"]
        for n in range(1, samples + 1):
            decay = math.exp(-n / (48 * ms)) if ms else 0
            db = target + (initial - target) * decay + settings["makeup_db"]
            gain = 10 ** (db/20)
            result.append((left/32768*gain, right/32768*gain))
        initial = target + (initial-target) * (math.exp(-samples/(48*ms)) if ms else 0)
    return result


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/"sources", root/"output"
    sources.mkdir(); output.mkdir()
    signals = {}
    def write(name, values):
        signals[name] = values
        with wave.open(str(sources/(name+".wav")), "wb") as w:
            w.setnchannels(2); w.setsampwidth(2); w.setframerate(RATE)
            w.writeframes(b"".join(struct.pack("<hh", *x) for x in values))
    def tone(frequency, amplitude, seconds=2, right=True):
        return [(x, x if right else 0) for n in range(seconds*RATE) for x in [round(amplitude * math.sin(math.tau * frequency*n/RATE))]]
    write("tone", tone(997, 3277))
    write("mono", tone(997, 3277, right=False))
    write("half", tone(997, 1638))
    write("low", tone(100, 3277))
    write("high", tone(7000, 3277))
    write("quiet", tone(997, 3))
    write("silence", [(0, 0)]*RATE)
    write("short", signals["tone"][:19199])
    write("tail", signals["tone"] + [(-32768, 32767)]*1432)
    write("gated", signals["tone"] + [(0,0)]*(RATE*2) + tone(997, 60))
    write("multisine", [tuple(round(sum(1700*math.sin(math.tau*f*n/RATE + c*.7) for f in (97,997,7003))) for c in (0,1)) for n in range(RATE)])
    write("impulse", [(16000 if n==0 else 0, -12000 if n==17 else 0) for n in range(4800)])
    segments = [(2400, 1000, -500), (4800, 24000, -12000), (4800, 2000, -1000), (2400, 0, 0)]
    write("steps", [pair for length, left, right in segments for pair in [(left, right)]*length])
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    executable = ENGINE
    passed, comparisons, meter_comparisons = [], [], []
    total = 0
    def request(value, error=None):
        result = subprocess.run([str(executable)], input=json.dumps(value).encode(), capture_output=True, timeout=90)
        data = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and data["error"]["code"] == error, data
        else:
            assert result.returncode == 0 and data["ok"], data
            return data["result"]
    def recipe(name, effects=None):
        length = time(len(signals[name]), RATE)
        return {"schema_version": 1, "id": "processing", "duration": length, "tracks": [{"id": "track", "clips": [{"id": "clip", "file": identity(sources/(name+".wav"), sources), "channels": "preserve_stereo", "start": time(0), "source_in": time(0), "duration": length}]}], "effects": effects or []}
    def inspect(mix):
        return request({"command": "audio.inspect", "mix": mix, "input_root": str(sources)})
    def render(mix, name, error=None):
        return request({"command": "audio.render", "mix": mix, "input_root": str(sources), "output_root": str(output), "output": str(output/(name+".wav"))}, error)
    def compare(name, mix, wanted):
        nonlocal total
        receipt = render(mix, name)
        assert inspect(mix)["pcm_sha256"] == receipt["pcm_sha256"]
        with wave.open(str(output/(name+".wav")), "rb") as w:
            raw = w.readframes(w.getnframes())
            actual = list(struct.iter_unpack("<hh", raw))
        assert len(actual) == len(wanted)
        maximum = max(abs(v-round_sample(w*32768)) for pair, reference in zip(actual, wanted) for v,w in zip(pair, reference))
        assert maximum <= 1, (name, maximum)
        assert hashlib.sha256(raw).hexdigest() == receipt["pcm_sha256"]
        for ch in (0,1):
            peak = max(abs(pair[ch]) for pair in actual)/32768
            power = math.fsum((pair[ch]/32768)**2 for pair in actual)/len(actual)
            assert abs(receipt["meters"]["sample_peak_dbfs"][ch]-20*math.log10(peak)) < 1e-9
            assert abs(receipt["meters"]["rms_dbfs"][ch]-10*math.log10(power)) < 1e-9
        total += len(actual)
        comparisons.append({"case": name, "sample_frames": len(actual), "maximum_pcm_error": maximum})
        return receipt, actual
    eqs = [{"type":"low_pass","frequency_hz":1500,"q":.707}, {"type":"high_pass","frequency_hz":500,"q":.707}, {"type":"peaking","frequency_hz":997,"q":1.5,"gain_db":12}]
    for source in ("impulse", "multisine"):
        for e in eqs:
            values = [(l/32768,r/32768) for l,r in signals[source]]
            compare(source+"-"+e["type"], recipe(source,[e]), ffmpeg_equalize(values,[e]))
    flat = [eqs[2], {**eqs[2], "gain_db":-12}]
    compare("inverse-eq", recipe("multisine",flat), [(l/32768,r/32768) for l,r in signals["multisine"]])
    compare("eight-effect-chain", recipe("multisine",flat*4), [(l/32768,r/32768) for l,r in signals["multisine"]])
    chain = eqs + [{"type":"peaking","frequency_hz":7003,"q":.5,"gain_db":-9}]
    compare("eq-chain", recipe("multisine",chain), ffmpeg_equalize([(l/32768,r/32768) for l,r in signals["multisine"]],chain))
    # Steady-state center gain is checked independently of the external filter.
    receipt, boosted = compare("center-gain", recipe("tone",[eqs[2]]), ffmpeg_equalize([(l/32768,r/32768) for l,r in signals["tone"]],[eqs[2]]))
    powers = [sum(x[0]*x[0] for x in vals[RATE:]) for vals in (boosted, signals["tone"])]
    assert abs(10*math.log10(powers[0]/powers[1])-12) < .01
    assert abs(receipt["meters"]["integrated_lkfs"]+8) < .02
    passed.append("audio_processing.eq_reference_and_response")
    comp = {"type":"compressor","threshold_db":-18,"ratio":4,"attack_ms":20,"release_ms":40,"makeup_db":3}
    for name, settings in (("compressor", comp), ("instant", {**comp,"attack_ms":0,"release_ms":0}), ("unity",{**comp,"ratio":1,"makeup_db":0})):
        compare(name, recipe("steps",[settings]), compressor_steps(segments, settings))
    passed.append("audio_processing.dynamics_step_response")
    first = recipe("steps",[comp,eqs[1]])
    _, forward = compare("compressor-then-eq", first, ffmpeg_equalize(compressor_steps(segments,comp),[eqs[1]]))
    reverse = recipe("steps",[eqs[1],comp])
    render(reverse,"eq-then-compressor")
    with wave.open(str(output/"eq-then-compressor.wav"),"rb") as w:
        backward = list(struct.iter_unpack("<hh",w.readframes(w.getnframes())))
    assert max(abs(a-b) for x,y in zip(forward,backward) for a,b in zip(x,y)) > 100
    assert inspect(json.loads(json.dumps(first)))["pcm_sha256"] == inspect(first)["pcm_sha256"]
    passed.append("audio_processing.effect_order_roundtrip")

    # Compare meter readings on exported final PCM with independent decoded math and FFmpeg.
    readings = {}
    for name in ("tone","mono","half","low","high","gated","quiet","silence","short","tail"):
        receipt = render(recipe(name),"meter-"+name)
        meter = receipt["meters"]; readings[name] = meter
        values = signals[name]
        for ch in (0,1):
            peak = max(abs(pair[ch]) for pair in values)/32768
            power = math.fsum((pair[ch]/32768)**2 for pair in values)/len(values)
            if peak:
                assert abs(meter["sample_peak_dbfs"][ch]-20*math.log10(peak)) < 1e-9
                assert abs(meter["rms_dbfs"][ch]-10*math.log10(power)) < 1e-9
            else:
                assert meter["sample_peak_dbfs"][ch] is None and meter["rms_dbfs"][ch] is None
        if name not in ("quiet","silence","short"):
            result = subprocess.run(["ffmpeg","-hide_banner","-nostats","-i",str(output/("meter-"+name+".wav")),"-af","ebur128=peak=true:framelog=verbose","-f","null","-"],capture_output=True,text=True,check=True,timeout=60)
            matches = re.findall(r"I:\s+(-?[0-9.]+) LUFS",result.stderr)
            assert matches, result.stderr
            other = float(matches[-1]); difference = abs(meter["integrated_lkfs"]-other)
            assert difference <= .11, (name,meter,other)
            # The 4x true-peak estimate against FFmpeg's own true-peak meter, which prints 0.1 dB
            # steps. The two interpolation filters ring differently at an abrupt start; that
            # dominates only the 7 kHz tone, which starts at full level (faded, both read -20.0).
            reference_peak = float(re.search(r"True peak:\s+Peak:\s+(-?[0-9.]+) dBFS",result.stderr).group(1))
            true_peak = max(v for v in meter["true_peak_dbtp"] if v is not None)
            assert abs(true_peak-reference_peak) <= (.35 if name == "high" else .06), (name,meter["true_peak_dbtp"],reference_peak)
            meter_comparisons.append({"case":name,"engine_lkfs":meter["integrated_lkfs"],"reference_lufs":other,"absolute_difference":difference,"engine_true_peak_dbtp":true_peak,"reference_true_peak_dbtp":reference_peak})
        assert meter["true_peak_available"] is True
        for ch in range(2):
            assert (meter["true_peak_dbtp"][ch] is None) == (meter["sample_peak_dbfs"][ch] is None)
            assert meter["true_peak_dbtp"][ch] is None or meter["true_peak_dbtp"][ch] >= meter["sample_peak_dbfs"][ch]-1e-9
    assert abs(readings["mono"]["integrated_lkfs"]+23.01)<.02
    assert abs(readings["tone"]["integrated_lkfs"]+20)<.02
    assert abs(readings["tone"]["integrated_lkfs"]-readings["half"]["integrated_lkfs"]-20*math.log10(3277/1638))<.002
    for name in ("quiet","silence"):
        assert readings[name]["loudness_status"]=="below_gate" and readings[name]["integrated_lkfs"] is None
    assert readings["short"]["loudness_status"]=="insufficient_duration" and readings["short"]["gating_blocks"]==0
    assert readings["tone"]["gating_blocks"]==17 and readings["tail"]["gating_blocks"]==17
    assert readings["tail"]["unmeasured_tail_samples"]==1432
    assert readings["tail"]["integrated_lkfs"]==readings["tone"]["integrated_lkfs"]
    assert readings["tail"]["sample_peak_dbfs"][0]==0
    assert 0 < readings["gated"]["relative_gated_blocks"] < readings["gated"]["absolute_gated_blocks"] < readings["gated"]["gating_blocks"]
    passed.append("audio_processing.meter_accuracy_gates")
    # Compression after wide summation must see the unclipped signal.
    hot=recipe("steps",[{**comp,"attack_ms":0,"release_ms":0}]);hot["tracks"][0]["gain_milli"]=4000
    wanted=compressor_steps([(n,l*4,r*4) for n,l,r in segments],hot["effects"][0])
    result,_=compare("headroom",hot,wanted);assert result["clipped_samples"]==[0,0]
    client=Client(executable)
    try:
        client.initialize()
        assert client.call("audio.inspect",mix=first,input_root=str(sources))["meters"]==inspect(first)["meters"]
    finally:client.close()
    bad=[([{**eqs[0],"frequency_hz":19}],"INVALID_AUDIO_EFFECT"),([{**eqs[0],"frequency_hz":20001}],"INVALID_AUDIO_EFFECT"),([{**eqs[0],"q":0}],"INVALID_AUDIO_EFFECT"),([{**eqs[2],"gain_db":25}],"INVALID_AUDIO_EFFECT"),([{**comp,"ratio":.9}],"INVALID_AUDIO_EFFECT"),([{**comp,"threshold_db":-61}],"INVALID_AUDIO_EFFECT"),([{**comp,"attack_ms":-1}],"INVALID_AUDIO_EFFECT"),([{**comp,"release_ms":5001}],"INVALID_AUDIO_EFFECT"),([{**comp,"makeup_db":25}],"INVALID_AUDIO_EFFECT"),(eqs*3,"INVALID_AUDIO_EFFECT"),([{**eqs[0],"gain_db":0}],"INVALID_JSON"),([{"type":"limiter"}],"INVALID_JSON")]
    for i,(effects,code) in enumerate(bad):
        render(recipe("tone",effects),f"bad-{i}",code)
        assert not (output/f"bad-{i}.wav").exists()
    render(recipe("tone"),"meter-tone","OUTPUT_EXISTS")
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("audio_processing.validation_preservation_mcp")
    # Beats against ground truth: synthetic drums at known tempos. Every true beat must have a
    # detected beat within one 10 ms hop, with no extra beats, and the tempo within 0.1 BPM.
    music = root/"music"; music.mkdir()
    def drums(name, bpm, offset, seconds, hats=False, accent=False):
        count = seconds*RATE; pcm = [[0, 0] for _ in range(count)]; period = 60/bpm; truth = []; k = 0
        while offset+k*period < seconds-0.1:
            t = offset+k*period; truth.append(t); start = round(t*RATE); amplitude = 30000 if not accent or k % 4 == 0 else 16000
            for i in range(int(0.03*RATE)):
                v = int(amplitude*math.exp(-i/600)*math.sin(math.tau*60*i/RATE)); pcm[start+i] = [v, v]
            if hats:
                h = round((t+period/2)*RATE)
                for i in range(int(0.01*RATE)):
                    if h+i < count:v = int(6000*(((i*7919) % 200)-100)/100); pcm[h+i] = [pcm[h+i][0]+v, pcm[h+i][1]+v]
            k += 1
        with wave.open(str(music/name), "wb") as w:
            w.setnchannels(2); w.setsampwidth(2); w.setframerate(RATE); w.writeframes(b"".join(struct.pack("<hh", *x) for x in pcm))
        return truth
    seconds_of = lambda t: t["num"]/t["den"]
    for name, bpm, offset, hats, accent in (("kick120.wav", 120, 0.25, True, False), ("click128.wav", 128, 0.1, False, False), ("accent93.wav", 93, 0.4, True, True)):
        truth = drums(name, bpm, offset, 20, hats, accent)
        found = request({"command":"audio.beats","path":str(music/name),"input_root":str(music),"frame_rate":time(25)})
        beats = [seconds_of(t) for t in found["beats"]["times"]]
        assert abs(found["tempo_bpm"]-bpm) <= .1 and found["beats"]["count"] == len(truth) == len(beats), (name, found["tempo_bpm"], len(beats), len(truth))
        assert all(min(abs(b-t) for b in beats) < .01 for t in truth) and all(min(abs(b-t) for t in truth) < .01 for b in beats), name
        assert found["beat_frames"]["frames"] == [math.floor(Fraction(t["num"], t["den"])*25+Fraction(1, 2)) for t in found["beats"]["times"]]
    # Arranged music at exact tempos, generated here: a triangle bass on eighths and a quiet pulse
    # arpeggio on sixteenths throughout, with drums only in the middle bars. In the backbeat (kick
    # on 1 and 3, snare on 2 and 4, hats on eighths) the kicks make the half-note level correlate
    # best; the first version read it at half tempo with a drifting grid. The kick's low sweep also
    # ripples the 10 ms level, which must not add onsets.
    def arranged(name, bpm, bars, drums, lead_in=0.0, four_floor=False):
        beat = Fraction(60, bpm); total = round((lead_in+bars*4*beat)*RATE); out = array("d", bytes(8*total)); seed = [12345]
        def noise():
            seed[0] = (seed[0]*1103515245+12345) % 2**31; return seed[0]/2**30-1
        def add(at, length, gain, voice):
            first = round(at*RATE)
            for i in range(min(round(length*RATE), total-first)):out[first+i] += gain*voice(i/RATE)
        def tone(note, decay, shape):
            return lambda t: shape(t*440*2**((note-69)/12) % 1)*math.exp(-t/decay)
        triangle, pulse = (lambda p: 4*abs(p-.5)-1), (lambda p: 1.0 if p < .125 else -1.0)
        kick = lambda t: math.sin(math.tau*(140*t-95*t*t/.36))*math.exp(-t/.06)
        snare = lambda t: (noise()+.4*math.sin(math.tau*190*t))*math.exp(-t/.045)
        hat = lambda decay: lambda t: noise()*math.exp(-t/decay)
        for bar in range(bars):
            t0 = lead_in+bar*4*beat; root = [48, 43, 45, 41][bar % 4]
            for e in range(8):add(t0+e*beat/2, beat/2, .3, tone(root-12+12*(e % 2), .12, triangle))
            for q in range(16):add(t0+q*beat/4, beat/4, .045, tone(root+12+[0, 4, 7][q % 3], .05, pulse))
            for b in range(4) if bar in drums else ():
                if four_floor or b % 2 == 0:add(t0+b*beat, .18, .55, kick)
                if b % 2:add(t0+b*beat, .16, .22, snare)
                if four_floor:add(t0+b*beat+beat/2, .12, .09, hat(.05))
                else:add(t0+b*beat, .04, .1, hat(.012)); add(t0+b*beat+beat/2, .04, .07, hat(.012))
        peak = max(map(abs, out))
        with wave.open(str(music/name), "wb") as w:
            w.setnchannels(2); w.setsampwidth(2); w.setframerate(RATE); w.writeframes(array("h", (round(x/peak*23000) for x in out for _ in (0, 1))).tobytes())
    exact = lambda times: [Fraction(t["num"], t["den"]) for t in times]
    arranged("backbeat125.wav", 125, 10, range(2, 8))
    found = request({"command":"audio.beats","path":str(music/"backbeat125.wav"),"input_root":str(music),"frame_rate":time(25)})
    # The beat level is preferred over the half-note level; both are reported. Every beat lies on
    # the true grid, all but the first (the file's first hop has no rise) on an onset, and the
    # onsets are exactly the eighths.
    assert found["tempo_bpm"] == 125 and found["period"] == {"num":12, "den":25}, found["tempo_bpm"]
    assert found["tempo_alternatives"] == [{"bpm":62.5, "ratio":{"num":1, "den":2}, "first_beat":0}], found["tempo_alternatives"]
    assert exact(found["beats"]["times"]) == [Fraction(12*n, 25) for n in range(40)] and found["beats"]["on_onsets"] == 39
    assert found["beat_frames"]["frames"] == [12*n for n in range(40)]
    assert exact(found["onsets"]["times"]) == [Fraction(6*k, 25) for k in range(1, 80)]
    # A range keeps the grid; the half-note level then starts on the second beat, a kick.
    part = request({"command":"audio.beats","path":str(music/"backbeat125.wav"),"input_root":str(music),"start":time(7),"duration":time(10)})
    assert exact(part["beats"]["times"]) == [Fraction(12*n, 25) for n in range(15, 36)] and part["tempo_alternatives"][0]["first_beat"] == 1
    # A narrowed range chooses the half-note level, on the kicks.
    slow = request({"command":"audio.beats","path":str(music/"backbeat125.wav"),"input_root":str(music),"min_bpm":40,"max_bpm":70})
    assert slow["tempo_bpm"] == 62.5 and slow["tempo_alternatives"] == [] and exact(slow["beats"]["times"]) == [Fraction(24*n, 25) for n in range(20)]
    # Four on the floor after a 0.35 s lead-in: a kick on every beat, claps on 2 and 4, open hats
    # off the beat. The offbeat eighths make the double level, which is listed.
    arranged("floor100.wav", 100, 8, range(1, 7), lead_in=Fraction(7, 20), four_floor=True)
    found = request({"command":"audio.beats","path":str(music/"floor100.wav"),"input_root":str(music)})
    assert found["tempo_bpm"] == 100 and found["period"] == {"num":3, "den":5} and found["tempo_alternatives"] == [{"bpm":200.0, "ratio":{"num":2, "den":1}}], found
    assert exact(found["beats"]["times"]) == [Fraction(7, 20)+Fraction(3, 5)*n for n in range(32)] and found["beats"]["on_onsets"] == 32
    onsets = exact(found["onsets"]["times"])
    assert {Fraction(7, 20)+Fraction(3, 10)*k for k in range(64)} <= set(onsets) and all((t-Fraction(7, 20))/Fraction(3, 20) % 1 == 0 for t in onsets)
    # A range reports file times inside it; silence has no tempo.
    part = request({"command":"audio.beats","path":str(music/"kick120.wav"),"input_root":str(music),"start":time(5),"duration":time(10)})
    times = [seconds_of(t) for t in part["beats"]["times"]]
    assert part["tempo_bpm"] == 120 and all(5 <= t < 15 for t in times) and min(abs(t-5.25) for t in times) < .01 and len(times) == 20
    write("silent-music", [(0, 0)]*(RATE*3))
    quiet = request({"command":"audio.beats","path":str(sources/"silent-music.wav"),"input_root":str(sources)})
    assert quiet["tempo_bpm"] is None and quiet["beats"]["count"] == 0 and quiet["onsets"]["count"] == 0 and quiet["tempo_alternatives"] == []
    for fields, code in (({"min_bpm":20}, "INVALID_ARGUMENT"), ({"min_bpm":100, "max_bpm":140}, "INVALID_ARGUMENT"), ({"frame_rate":time(23)}, "UNSUPPORTED_TIMELINE")):
        request({"command":"audio.beats","path":str(music/"kick120.wav"),"input_root":str(music),**fields}, code)
    client = Client(executable)
    try:
        client.initialize()
        tool = next(t for t in client.rpc("tools/list")["result"]["tools"] if t["name"] == "cutbolt_audio_beats")
        assert tool["annotations"]["readOnlyHint"] and client.call("audio.beats", path=str(music/"click128.wav"), input_root=str(music))["tempo_bpm"] == request({"command":"audio.beats","path":str(music/"click128.wav"),"input_root":str(music)})["tempo_bpm"]
    finally:client.close()
    passed.append("audio_processing.beats_match_ground_truth")
    report={"passed":passed,"pcm_sample_frames_compared":total,"pcm_tolerance":1,"comparisons":comparisons,"meter_reference":"FFmpeg ebur128 (loudness and true peak), original 997 Hz calibration, independent PCM peak/RMS math; no true-peak certification", "meter_tolerance_lu":.11,"meter_comparisons":meter_comparisons,"rejected_cases":len(bad),"ffmpeg":subprocess.run(["ffmpeg","-version"],capture_output=True,text=True,check=True).stdout.splitlines()[0]}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    (root/"mix.json").write_text(json.dumps(first,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
