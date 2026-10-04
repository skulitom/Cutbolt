"""Original PCM fixtures with independent rational trim, resample, envelope and mix references."""
from engine import ENGINE
import argparse
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import wave
from PIL import Image

from agents import Client
from animation import sample
from scenes import identity, time

ROOT=Path(__file__).resolve().parents[1]
def fraction(t):return Fraction(t["num"],t["den"])
def rounding(x):return int(abs(x)+Fraction(1,2))*(-1 if x<0 else 1)


def expected(mix, sources):
    total=int(fraction(mix["duration"])*48000)
    acc=[[0,0] for _ in range(total)]
    for track in mix["tracks"]:
        for clip in track["clips"]:
            with wave.open(str(sources/clip["file"]["path"]),"rb") as w:
                channels,rate=w.getnchannels(),w.getframerate();raw=w.readframes(w.getnframes())
            values=list(struct.iter_unpack("<"+"h"*channels,raw))
            length=int(fraction(clip["duration"])*48000);start=int(fraction(clip["start"])*48000)
            cut=int(fraction(clip["source_in"])*rate)
            fade_in=int(fraction(clip.get("fade_in",time(0)))*48000);fade_out=int(fraction(clip.get("fade_out",time(0)))*48000)
            for n in range(length):
                pos=Fraction(n*rate,48000)+cut;a=int(pos);b=min(a+1,len(values)-1)
                gain=sample(clip["gain_curve"],Fraction(n,48000)) if "gain_curve" in clip else clip.get("gain_milli",1000)
                weight=Fraction(gain*track.get("gain_milli",1000),1000000)
                if fade_in:weight*=min(Fraction(1),Fraction(n,fade_in))
                if fade_out:weight*=min(Fraction(1),Fraction(length-n,fade_out))
                if track.get("mute",False) or clip.get("mute",False):weight=Fraction(0)
                for ch in range(2):
                    c=0 if channels==1 else ch
                    source=rounding(values[a][c]*(1-(pos-a))+values[b][c]*(pos-a))
                    acc[start+n][ch]+=rounding(source*weight)
    clipped=[sum(not -32768<=v[c]<=32767 for v in acc) for c in range(2)]
    vals=[[max(-32768,min(32767,x)) for x in v] for v in acc]
    return b"".join(struct.pack("<hh",*v) for v in vals),clipped


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True);sources=root/"sources";out=root/"output";store=root/"store"
    for p in (sources,out,store):p.mkdir()
    def write(name,rate,channels,values):
        with wave.open(str(sources/name),"wb") as w:
            w.setnchannels(channels);w.setsampwidth(2);w.setframerate(rate)
            w.writeframes(b"".join(struct.pack("<"+"h"*channels,*v) for v in values))
    for rate in (24000,44100,48000):
        for channels in (1,2):
            count=rate//100
            vals=[[(-30000 if n==0 else 30000 if n==count-1 else (n*193+c*811)%30001-15000) for c in range(channels)] for n in range(count)]
            write(f"{rate}-{channels}.wav",rate,channels,vals)
    for name,value in (("flat",9600),("positive",30000),("negative",-30000)):
        write(name+".wav",48000,2,[[value,value]]*960)
    write("unsupported.wav",32000,1,[[0]]*32)
    Image.new("RGB",(4,4),(25,90,170)).save(sources/"image.png")
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;passed=[];cases=[];samples=0
    def request(value,error=None):
        result=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=90)
        data=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and data["error"]["code"]==error,data
            return data
        assert result.returncode==0 and data["ok"],data
        return data["result"]
    def clip(name,path,start=0,duration=480,channels=2):
        return {"id":name,"file":identity(sources/path,sources),"channels":"duplicate_mono" if channels==1 else "preserve_stereo","start":time(start,48000),"source_in":time(0),"duration":time(duration,48000)}
    def mix_for(clips,duration=720):return {"schema_version":1,"id":"audio-fixture","duration":time(duration,48000),"tracks":[{"id":"track","clips":clips}]}
    def render(mix,name,error=None):return request({"command":"audio.render","mix":mix,"input_root":str(sources),"output_root":str(out),"output":str(out/name)},error)
    def verify(mix,name):
        nonlocal samples
        info=request({"command":"audio.inspect","mix":mix,"input_root":str(sources)})
        receipt=render(mix,name+".wav")
        wanted,clipped=expected(mix,sources)
        with wave.open(str(out/(name+".wav")),"rb") as w:
            assert (w.getframerate(),w.getnchannels(),w.getsampwidth())==(48000,2,2)
            actual=w.readframes(w.getnframes())
        assert actual==wanted,name
        assert info["pcm_sha256"]==receipt["pcm_sha256"]==hashlib.sha256(wanted).hexdigest()
        assert info["clipped_samples"]==clipped and info["samples"]==len(wanted)//4
        samples+=len(wanted)//4;cases.append(name)
        return receipt,wanted
    for rate in (24000,44100,48000):
        for channels in (1,2):
            c=clip("cut",f"{rate}-{channels}.wav",start=3,channels=channels)
            verify(mix_for([c]),f"full-{rate}-{channels}")
            c["source_in"]=time(3,rate);c["duration"]=time(173,48000);c["start"]=time(7,48000)
            verify(mix_for([c],duration=183),f"trim-{rate}-{channels}")
    passed.append("audio_mix.sample_cuts_rates_layouts")
    base=mix_for([clip("voice","48000-2.wav",start=7)],duration=497)
    for name,fields in (("gain",{"gain_milli":500}),("mute",{"mute":True}),("fades",{"fade_in":time(120,48000),"fade_out":time(96,48000)})):
        value=copy.deepcopy(base);value["tracks"][0]["clips"][0].update(fields);verify(value,name)
    passed.append("audio_mix.gain_mute_fades")
    automated=copy.deepcopy(base)
    automated["tracks"][0]["clips"][0]["gain_curve"]={"keys":[{"time":time(0),"value":0,"interpolation":"linear"},{"time":time(48,48000),"value":2000,"interpolation":"hold"},{"time":time(173,48000),"value":250,"interpolation":"ease_in_out"},{"time":time(480,48000),"value":1000,"interpolation":"hold"}],"retime":{"start":time(1,48000),"rate":time(3,2),"reverse":True}}
    verify(automated,"automation")
    cross=mix_for([],duration=720)
    a=clip("outgoing","flat.wav");a["fade_out"]=time(240,48000)
    b=clip("incoming","flat.wav",start=240);b["fade_in"]=time(240,48000)
    cross["tracks"]=[{"id":"a","clips":[a]},{"id":"b","clips":[b]}]
    _,cross_bytes=verify(cross,"crossfade")
    assert cross_bytes==struct.pack("<hh",9600,9600)*720
    hot=mix_for([],duration=480)
    hot["tracks"]=[{"id":str(i),"clips":[clip(str(i),name+".wav")]} for i,name in enumerate(("positive","positive"))]
    clipped,hot_bytes=verify(hot,"clipping")
    assert hot_bytes==struct.pack("<hh",32767,32767)*480 and clipped["clipped_samples"]==[480,480]
    hot["tracks"].append({"id":"negative","clips":[clip("negative","negative.wav")]})
    unclipped,unc_bytes=verify(hot,"cancellation")
    assert unc_bytes==struct.pack("<hh",30000,30000)*480 and unclipped["clipped_samples"]==[0,0]
    hot["tracks"].reverse();assert verify(hot,"track-order")[1]==unc_bytes
    passed.append("audio_mix.automation_crossfade_clipping")
    hot["tracks"][0]["mute"]=True;hot["tracks"][1]["gain_milli"]=250;verify(hot,"track-controls")
    silence=copy.deepcopy(base);silence["tracks"]=[];assert not any(verify(silence,"silence")[1])
    passed.append("audio_mix.multitrack_sum")
    client=Client(exe)
    try:
        client.initialize()
        info=client.call("audio.inspect",mix=automated,input_root=str(sources))
        reordered=json.loads(json.dumps(automated));reordered["tracks"][0]["clips"][0]["gain_curve"]["keys"].reverse()
        assert client.call("audio.inspect",mix=reordered,input_root=str(sources))["pcm_sha256"]==info["pcm_sha256"]
    finally:client.close()
    passed.append("audio_mix.mcp_recipe_roundtrip")

    # A mix feeds a scene and the compiled result enters the existing session/render contract.
    soundtrack=copy.deepcopy(cross);soundtrack["duration"]=time(1,25)
    scene={"schema_version":1,"id":"mixed-scene","width":4,"height":4,"output_scale":1,"duration":time(1,25),"background":[0,0,0],"color":"srgb_straight_encoded","layers":[{"id":"image","canvas":[4,4],"start":time(0),"duration":time(1,25),"frames":[{"image":identity(sources/"image.png",sources),"hold":time(1,25),"offset":[0,0],"anchor":[0,0]}],"timing":"strict","end":"hold_last","transform":{"position":[0,0],"crop":[0,0,4,4],"scale":1,"quarter_turns":0,"opacity":255}}],"audio":None,"audio_mix":soundtrack}
    receipt=request({"command":"scene.render","scene":scene,"input_root":str(sources),"output_root":str(out),"output":str(out/"scene.mkv")})
    project=request({"command":"project.create","id":"soundtrack-session","width":4,"height":4,"frame_rate":time(25)})
    request({"command":"session.create","store_root":str(store),"project":project,"request_id":"create"})
    common={"store_root":str(store),"project_id":project["id"]}
    request({"command":"session.apply",**common,"expected_revision":0,"request_id":"soundtrack","operations":[{"op":"media.add","asset":receipt["asset"]},{"op":"clip.append","clip":{"id":"shot","asset_id":scene["id"],"source_in":time(0),"duration":time(1,25)}}]})
    saved=request({"command":"session.get",**common})
    request({"command":"render.run","project":saved,"input_root":str(out),"output_root":str(out),"output":str(out/"session.mkv")})
    pcm=subprocess.run(["ffmpeg","-v","error","-i",str(out/"session.mkv"),"-map","0:a:0","-f","s16le","-"],capture_output=True,check=True).stdout
    assert pcm==expected(soundtrack,sources)[0]
    rgb=subprocess.run(["ffmpeg","-v","error","-i",str(out/"session.mkv"),"-map","0:v:0","-f","rawvideo","-pix_fmt","rgb24","-"],capture_output=True,check=True).stdout
    assert rgb==bytes([25,90,170])*16
    passed.append("audio_mix.scene_session_integration")

    bad=[]
    def invalid(change,code="INVALID_AUDIO"):
        value=copy.deepcopy(base);change(value);bad.append((value,code))
    invalid(lambda m:m.update(duration=time(0)))
    invalid(lambda m:m.update(duration=time(61)))
    invalid(lambda m:m.update(duration=time(1,96000)),"UNALIGNED_TIME")
    invalid(lambda m:m["tracks"][0]["clips"][0].update(start=time(490,48000)))
    invalid(lambda m:m["tracks"][0]["clips"][0].update(source_in=time(479,48000)),"INVALID_RANGE")
    invalid(lambda m:m["tracks"][0]["clips"][0].update(source_in=time(1,96000)),"UNALIGNED_TIME")
    invalid(lambda m:m["tracks"][0]["clips"][0].update(gain_milli=4001))
    invalid(lambda m:m["tracks"][0]["clips"][0].update(fade_in=time(481,48000)))
    invalid(lambda m:m["tracks"][0]["clips"].append(copy.deepcopy(m["tracks"][0]["clips"][0])))
    invalid(lambda m:m["tracks"][0]["clips"][0].update(channels="duplicate_mono"),"UNSUPPORTED_AUDIO")
    invalid(lambda m:m["tracks"][0]["clips"][0].update(file=identity(sources/"unsupported.wav",sources)),"UNSUPPORTED_AUDIO")
    invalid(lambda m:m["tracks"][0]["clips"][0].update(gain_curve={"keys":[]}),"INVALID_ANIMATION")
    invalid(lambda m:m["tracks"][0].update(pan=0),"INVALID_JSON")
    invalid(lambda m:m["tracks"][0]["clips"][0]["file"].update(sha256="0"*64),"MEDIA_CHANGED")
    invalid(lambda m:m["tracks"][0]["clips"][0]["file"].update(path="../outside.wav"),"INVALID_PATH")
    for i,(value,code) in enumerate(bad):
        render(value,f"invalid-{i}.wav",code);assert not (out/f"invalid-{i}.wav").exists()
    wrong_scene=copy.deepcopy(scene);wrong_scene["audio_mix"]["duration"]=time(1)
    request({"command":"scene.inspect","scene":wrong_scene,"input_root":str(sources)},"INVALID_AUDIO")
    saved_hash=hashlib.sha256((out/"crossfade.wav").read_bytes()).hexdigest()
    render(cross,"crossfade.wav","OUTPUT_EXISTS")
    assert saved_hash==hashlib.sha256((out/"crossfade.wav").read_bytes()).hexdigest()
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(out.glob(".cutbolt-scene-*"))
    passed.append("audio_mix.validation_source_protection")
    report={"passed":passed,"sample_frames_compared":samples,"render_cases":cases,"rejected_cases":len(bad)+1,"reference":"Independent Fraction resampling/envelopes and wide per-voice PCM summation; exact full output comparison","limits":"PCM16 mono/stereo 24/44.1/48 kHz sources, 48 kHz stereo output, linear resampling, <=60 seconds/16 tracks/128 clips/8M clip samples; this legacy-profile fixture excludes optional routing; inline media speed remains unsupported"}
    (root/"mix.json").write_text(json.dumps(automated,indent=2)+"\n",encoding="utf-8")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
