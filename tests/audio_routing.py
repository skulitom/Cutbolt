"""Original named-channel signals, independent graph algebra and rendered PCM checks."""
import argparse
import copy
from decimal import Decimal as D
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import time as wall_clock
import uuid
import wave

from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from animation import sample
from scenes import identity, time
from spatial import trig
from registry import peak_memory

ROOT = Path(__file__).resolve().parents[1]
LAYOUTS = {'mono': (4, ['FC']), 'stereo': (3, ['FL', 'FR']), 'quad': (0x33, ['FL', 'FR', 'BL', 'BR']),
           '5.1': (0x3f, ['FL', 'FR', 'FC', 'LFE', 'BL', 'BR']), '5.1(side)': (0x60f, ['FL', 'FR', 'FC', 'LFE', 'SL', 'SR']),
           '7.1': (0x63f, ['FL', 'FR', 'FC', 'LFE', 'BL', 'BR', 'SL', 'SR'])}


def fraction(v): return F(v['num'], v['den'])
def rounded(v): return int(abs(v)+(D('.5') if isinstance(v, D) else F(1, 2)))*(-1 if v < 0 else 1)
def pack(values): return b''.join(struct.pack('<'+'h'*len(v), *v) for v in values)
def node(kind, id_=None): return {'kind': kind, **({'id': id_} if id_ is not None else {})}
def key(ref): return ref['kind']+':'+ref.get('id', '')


def wave_bytes(layout, rate, values, classic=False):
    mask, speakers = LAYOUTS[layout]; count = len(speakers); data = pack(values)
    fmt = struct.pack('<HHIIHH', 1 if classic else 65534, count, rate, rate*count*2, count*2, 16)
    if not classic: fmt += struct.pack('<HHI', 22, 16, mask)+uuid.UUID('00000001-0000-0010-8000-00aa00389b71').bytes_le
    body = b'WAVEfmt '+struct.pack('<I', len(fmt))+fmt+b'data'+struct.pack('<I', len(data))+data
    return b'RIFF'+struct.pack('<I', len(body))+body


@lru_cache(None)
def pan_weights(position):
    sine, cosine = trig((position+1000)*45)
    return cosine, sine


def effects_reference(values, effects):
    if not effects: return values
    channels = len(values[0]); buffer = [[v/32768 for v in frame] for frame in values]
    for effect in effects:
        if effect['type'] != 'compressor':
            name = {'low_pass': 'lowpass', 'high_pass': 'highpass', 'peaking': 'equalizer'}[effect['type']]
            filter_ = f"{name}=f={effect['frequency_hz']}:t=q:w={effect['q']}:precision=f64"
            if name == 'equalizer': filter_ += f":g={effect['gain_db']}"
            raw = b''.join(struct.pack('<'+'d'*channels, *v) for v in buffer)
            result = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'f64le', '-ar', '48000', '-ac', str(channels), '-i', 'pipe:0', '-af', filter_, '-f', 'f64le', 'pipe:1'], input=raw, capture_output=True, check=True)
            buffer = list(struct.iter_unpack('<'+'d'*channels, result.stdout))
        else:
            # Closed-form evolution over each constant-peak run. The fixture's
            # longest runs exercise attack/release state without a matching recurrence.
            output = []; start = 0; reduction = 0.0
            while start < len(buffer):
                peak = max(abs(v) for v in buffer[start]); end = start+1
                while end < len(buffer) and max(abs(v) for v in buffer[end]) == peak: end += 1
                over = max(0, 20*math.log10(peak)-effect['threshold_db']) if peak else 0
                target = -over*(1-1/effect['ratio']); ms = effect['attack_ms'] if target < reduction else effect['release_ms']
                for i in range(start, end):
                    decay = math.exp(-(i-start+1)/(48*ms)) if ms else 0
                    value = target+(reduction-target)*decay
                    gain = 10**((value+effect['makeup_db'])/20)
                    output.append([v*gain for v in buffer[i]])
                reduction = target+(reduction-target)*(math.exp(-(end-start)/(48*ms)) if ms else 0)
                start = end
            buffer = output
    return [[rounded(v*32768) for v in frame] for frame in buffer]


def reference(mix, fixtures):
    total = int(fraction(mix['duration'])*48000); routing = mix['routing']
    layouts = {'track:'+t['id']: t['layout'] for t in routing['tracks']}
    layouts.update({'bus:'+b['id']: b['layout'] for b in routing['buses']}); layouts['output:'] = routing['output']
    ready = {}
    for track in mix['tracks']:
        channels = len(LAYOUTS[layouts['track:'+track['id']]][1]); values = [[0]*channels for _ in range(total)]
        for clip in track['clips']:
            source, rate, _ = fixtures[clip['file']['path']]
            cut = fraction(clip['source_in'])*rate; assert cut.denominator == 1
            start = int(fraction(clip['start'])*48000); length = int(fraction(clip['duration'])*48000)
            fi = int(fraction(clip.get('fade_in', time(0)))*48000); fo = int(fraction(clip.get('fade_out', time(0)))*48000)
            for n in range(length):
                p = cut+F(n*rate, 48000); a = int(p); b = min(a+1, len(source)-1); w = p-a
                gain = sample(clip['gain_curve'], F(n, 48000)) if clip.get('gain_curve') else clip.get('gain_milli', 1000)
                weight = F(gain*track.get('gain_milli', 1000), 1000000)
                if fi: weight *= min(F(1), F(n, fi))
                if fo: weight *= min(F(1), F(length-n, fo))
                if clip.get('mute') or track.get('mute'): weight = F(0)
                for c in range(channels):
                    sc = 0 if len(source[0]) == 1 else c
                    values[start+n][c] += rounded(rounded(source[a][sc]*(1-w)+source[b][sc]*w)*weight)
        ready['track:'+track['id']] = values
    # Recursively evaluate incoming connections in whole buffers, independently of
    # the engine's streaming topological schedule and per-node sample storage.
    def evaluate(target):
        if target in ready: return ready[target]
        channels = len(LAYOUTS[layouts[target]][1]); acc = [[0]*channels for _ in range(total)]
        for route in routing['routes']:
            if key(route['destination']) != target: continue
            upstream = evaluate(key(route['source'])); mapping = route['mapping']
            for n, frame in enumerate(upstream):
                kind = mapping['type']
                if kind == 'identity': output = frame
                elif kind == 'matrix': output = [rounded(sum(F(v*c, 1000) for v, c in zip(frame, row))) for row in mapping['coefficients_milli']]
                else:
                    p = sample(mapping['curve'], F(n, 48000)) if mapping.get('curve') else mapping['position_milli']
                    if kind == 'balance': output = [rounded(F(frame[0]*(1000-max(0, p)), 1000)), rounded(F(frame[1]*(1000+min(0, p)), 1000))]
                    elif mapping['law'] == 'linear': output = [rounded(F(frame[0]*(1000-p), 2000)), rounded(F(frame[0]*(1000+p), 2000))]
                    else: output = [rounded(D(frame[0])*v) for v in pan_weights(p)]
                for c, v in enumerate(output): acc[n][c] += v
        if target.startswith('bus:'):
            bus = next(b for b in routing['buses'] if 'bus:'+b['id'] == target)
            acc = [[rounded(F(v*(sample(bus['gain_curve'], F(n, 48000)) if bus.get('gain_curve') else bus.get('gain_milli', 1000)), 1000)) for v in frame] for n, frame in enumerate(acc)]
            acc = effects_reference(acc, bus.get('effects', []))
            if bus.get('mute'): acc = [[0]*channels for _ in range(total)]
        else: acc = effects_reference(acc, mix.get('effects', []))
        ready[target] = acc; return acc
    wide = evaluate('output:'); channels = len(wide[0])
    clipped = [sum(v[c] < -32768 or v[c] > 32767 for v in wide) for c in range(channels)]
    return [[max(-32768, min(32767, v)) for v in frame] for frame in wide], clipped


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = (root/n for n in ('sources', 'output', 'store'))
    for p in (sources, output, store): p.mkdir()
    exe = ROOT/'target/debug/cutbolt.exe'; fixtures = {}; passed = []; comparisons = []; rejected = 0; previews = 0; video_frames = 0

    def write(name, layout, rate, values, classic=False):
        fixtures[name] = values, rate, layout
        (sources/name).write_bytes(wave_bytes(layout, rate, values, classic))
    for layout, (_, speakers) in LAYOUTS.items():
        for rate in (24000, 44100, 48000):
            name = layout.replace('(', '-').replace(')', '')+'-'+str(rate)+'.wav'
            write(name, layout, rate, [[(1000*(c+1) if n == c*4 else -1000*(c+1) if n == c*4+1 else (n*17+c*211)%4001-2000) for c in range(len(speakers))] for n in range(rate//10)])
    write('classic-mono.wav', 'mono', 48000, [[10000]]*9600, True)
    write('classic-stereo.wav', 'stereo', 48000, [[12000, -6000]]*9600, True)
    write('small.wav', 'stereo', 48000, [[1, 1], [-1, -1], [1, -1], [-1, 1]]*120, True)
    write('loud.wav', 'stereo', 48000, [[30000, -30000]]*9600, True)
    steps = [[1000 if c == 0 else 500 for c in range(8)]]*2400 + [[2000 if c < 7 else 24000 for c in range(8)]]*4800 + [[1000 if c == 0 else -500 for c in range(8)]]*4800
    write('steps.wav', '7.1', 48000, steps)
    Image.new('RGB', (4, 4), (25, 90, 170)).save(sources/'image.png')

    def call(req, error=None):
        nonlocal rejected
        p = subprocess.run([str(exe)], input=json.dumps(req).encode(), capture_output=True, timeout=180)
        data = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and data['error']['code'] == error, data
            rejected += 1; return data
        assert p.returncode == 0 and data['ok'], data
        return data['result']

    def clip(name, length=480, start=0, policy='preserve_layout'):
        return {'id': name, 'file': identity(sources/name, sources), 'channels': policy, 'start': time(start, 48000), 'source_in': time(0), 'duration': time(length, 48000)}
    def route(id_, source, destination, mapping=None):
        return {'id': id_, 'source': source, 'destination': destination, 'mapping': mapping or {'type': 'identity'}}
    def mix_for(name, layout=None, length=480, duration=None, policy='preserve_layout'):
        layout = layout or fixtures[name][2]
        return {'schema_version': 1, 'id': 'routing-fixture', 'duration': time(duration or length, 48000), 'tracks': [{'id': 'voice', 'clips': [clip(name, length, policy=policy)]}],
                'routing': {'output': layout, 'tracks': [{'id': 'voice', 'layout': layout}], 'buses': [], 'routes': [route('direct', node('track', 'voice'), node('output'))]}}
    def render_request(mix, name):
        return {'command': 'audio.render', 'mix': mix, 'input_root': str(sources), 'output_root': str(output), 'output': str(output/(name+'.wav'))}

    def check(mix, name, tolerance=0):
        receipt = call(render_request(mix, name)); info = call({'command': 'audio.inspect', 'mix': mix, 'input_root': str(sources)})
        assert receipt['pcm_sha256'] == info['pcm_sha256']
        path = output/(name+'.wav'); raw = path.read_bytes(); channels = len(LAYOUTS[mix['routing']['output']][1])
        assert raw[:4] == b'RIFF' and struct.unpack_from('<I', raw, 4)[0]+8 == len(raw)
        assert raw[12:16] == b'fmt ' and struct.unpack_from('<I', raw, 16)[0] == 40
        assert struct.unpack_from('<HHIIHHHHI', raw, 20) == (65534, channels, 48000, 48000*channels*2, channels*2, 16, 22, 16, LAYOUTS[mix['routing']['output']][0])
        assert uuid.UUID(bytes_le=raw[44:60]) == uuid.UUID('00000001-0000-0010-8000-00aa00389b71')
        assert raw[60:64] == b'data' and struct.unpack_from('<I', raw, 64)[0] == len(raw)-68
        actual = list(struct.iter_unpack('<'+'h'*channels, raw[68:])); wanted, clipped = reference(mix, fixtures)
        assert len(actual) == len(wanted)
        error = max(abs(a-b) for v, w in zip(actual, wanted) for a, b in zip(v, w)); assert error <= tolerance, (name, error)
        assert info['clipped_samples'] == clipped and info['samples'] == len(wanted)
        assert info['pcm_sha256'] == hashlib.sha256(raw[68:]).hexdigest()
        assert info['speakers'] == LAYOUTS[mix['routing']['output']][1]
        assert info['peak_absolute'] == [max(abs(v[c]) for v in actual) for c in range(channels)]
        for c in range(channels):
            power = sum(v[c]**2 for v in actual)/(len(actual)*32768**2)
            rms = 10*math.log10(power) if power else None
            assert info['meters']['rms_dbfs'][c] is None if rms is None else abs(info['meters']['rms_dbfs'][c]-rms) < 1e-10
        if channels != 2: assert info['meters']['loudness_status'] == 'unsupported_layout' and info['meters']['integrated_lkfs'] is None
        meta = json.loads(subprocess.run(['ffprobe', '-v', 'error', '-show_streams', '-of', 'json', str(path)], capture_output=True, check=True).stdout)['streams'][0]
        assert meta['channel_layout'] == mix['routing']['output'] and meta['channels'] == channels
        decoded = subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-f', 's16le', '-'], capture_output=True, check=True).stdout
        assert decoded == raw[68:]
        comparisons.append({'name': name, 'channels': channels, 'sample_frames': len(actual), 'maximum_pcm_error': error})
        (root/(name+'-mix.json')).write_text(json.dumps(mix, indent=2)+'\n', encoding='utf-8')
        return receipt, actual

    for name, (values, rate, layout) in list(fixtures.items()):
        if name.startswith(('classic', 'small', 'loud', 'steps')): continue
        mix = mix_for(name, length=473, duration=483); c = mix['tracks'][0]['clips'][0]
        c.update(source_in=time(3, rate), start=time(7, 48000), gain_milli=1750, fade_in=time(13, 48000), fade_out=time(17, 48000))
        check(mix, 'layout-'+name[:-4])
    original = (sources/'7.1-48000.wav').read_bytes(); body = original[8:12]+b'JUNK'+struct.pack('<I', 3)+b'abc\0'+original[12:]
    (sources/'padded-chunk.wav').write_bytes(b'RIFF'+struct.pack('<I', len(body))+body)
    fixtures['padded-chunk.wav'] = fixtures['7.1-48000.wav']; check(mix_for('padded-chunk.wav'), 'unknown-padded-chunk')
    passed.append('audio_routing.named_layouts_and_sample_clocks')

    legacy = mix_for('classic-stereo.wav', policy='preserve_stereo', length=960, duration=1200)
    c = legacy['tracks'][0]['clips'][0]; c.update(gain_milli=1237, start=time(7, 48000), fade_in=time(31, 48000), fade_out=time(71, 48000))
    legacy['tracks'][0]['gain_milli'] = 831
    routed = check(legacy, 'legacy-equivalence')
    old = copy.deepcopy(legacy); old.pop('routing'); old_info = call(render_request(old, 'legacy-stereo'))
    with wave.open(str(output/'legacy-stereo.wav'), 'rb') as w: assert w.readframes(w.getnframes()) == pack(routed[1])
    assert old_info['pcm_sha256'] == routed[0]['pcm_sha256']
    duplicate = mix_for('classic-mono.wav', 'stereo', policy='duplicate_mono'); check(duplicate, 'duplicate-mono')
    small = mix_for('small.wav', policy='preserve_stereo'); small['routing']['output'] = 'mono'; small['routing']['routes'][0]['mapping'] = {'type': 'matrix', 'coefficients_milli': [[500, 500]]}
    values = check(small, 'matrix-round-once')[1]; assert values[:4] == [(1,), (-1,), (0,), (0,)]
    fan = mix_for('loud.wav', policy='preserve_stereo'); fan['tracks'][0]['gain_milli'] = 4000
    fan['routing']['buses'] = [{'id': 'sum', 'layout': 'stereo', 'gain_milli': 500}]
    fan['routing']['routes'] = [route('dry', node('track', 'voice'), node('output')), route('send', node('track', 'voice'), node('bus', 'sum')), route('return', node('bus', 'sum'), node('output'), {'type': 'matrix', 'coefficients_milli': [[-2000, 0], [0, -2000]]})]
    assert all(v == (0, 0) for v in check(fan, 'bus-headroom-cancellation')[1])
    gain = copy.deepcopy(fan); gain['routing']['routes'][-1]['mapping']['coefficients_milli'] = [[1000, 0], [0, 1000]]
    assert all(v == (32767, -32768) for v in check(gain, 'final-only-clipping')[1])
    automation = copy.deepcopy(gain); automation['tracks'][0]['gain_milli'] = 1000
    automation['routing']['buses'][0]['gain_curve'] = {'keys': [{'time': time(0), 'value': 0, 'interpolation': 'ease_in_out'}, {'time': time(480, 48000), 'value': 1000, 'interpolation': 'hold'}]}
    check(automation, 'bus-gain-automation')
    muted = copy.deepcopy(automation); muted['routing']['buses'][0]['mute'] = True; check(muted, 'bus-mute')
    surround = mix_for('7.1-48000.wav', length=1920)
    surround['routing']['output'] = 'stereo'; surround['routing']['buses'] = [{'id': 'fold', 'layout': '5.1(side)'}, {'id': 'stereo', 'layout': 'stereo', 'gain_milli': 750}]
    matrix = [[0]*8 for _ in range(6)]
    for i in range(4): matrix[i][i] = 1000
    matrix[4][4] = matrix[4][6] = matrix[5][5] = matrix[5][7] = 500
    surround['routing']['routes'] = [route('fold', node('track', 'voice'), node('bus', 'fold'), {'type': 'matrix', 'coefficients_milli': matrix}),
                                   route('downmix', node('bus', 'fold'), node('bus', 'stereo'), {'type': 'matrix', 'coefficients_milli': [[1000, 0, 707, 0, 707, 0], [0, 1000, 707, 0, 0, 707]]}),
                                   route('output', node('bus', 'stereo'), node('output'))]
    surround_result = check(surround, 'surround-buses-downmix')
    reordered = copy.deepcopy(surround)
    for field in ('tracks', 'buses', 'routes'): reordered['routing'][field].reverse()
    assert check(reordered, 'graph-order-invariance')[1] == surround_result[1]
    ensemble = copy.deepcopy(surround)
    ensemble['tracks'].append({'id': 'dialogue', 'gain_milli': 517, 'clips': [clip('classic-mono.wav', length=1800, start=9, policy='preserve_mono')]})
    voice = ensemble['tracks'][1]['clips'][0]
    voice.update(fade_in=time(31, 48000), fade_out=time(41, 48000), gain_curve={'keys': [{'time': time(0), 'value': 300, 'interpolation': 'ease_in'}, {'time': time(1800, 48000), 'value': 1700, 'interpolation': 'hold'}]})
    ensemble['routing']['tracks'].append({'id': 'dialogue', 'layout': 'mono'})
    ensemble['routing']['routes'].append(route('dialogue-pan', node('track', 'dialogue'), node('bus', 'stereo'), {'type': 'pan', 'law': 'linear', 'position_milli': -250}))
    result = check(ensemble, 'mixed-track-layouts-and-envelopes')
    shuffled = copy.deepcopy(ensemble); shuffled['tracks'].reverse()
    for field in ('tracks', 'buses', 'routes'): shuffled['routing'][field].reverse()
    assert check(shuffled, 'mixed-track-order-invariance')[1] == result[1]
    empty = {'schema_version': 1, 'id': 'silence', 'duration': time(7, 48000), 'tracks': [], 'routing': {'output': '7.1', 'tracks': [], 'buses': [], 'routes': []}}
    assert all(v == (0,)*8 for v in check(empty, 'silent-surround')[1])
    passed.append('audio_routing.buses_matrices_and_headroom')

    for law in ('linear', 'equal_power'):
        for position in (-1000, -500, 0, 500, 1000):
            mix = mix_for('classic-mono.wav', policy='preserve_mono', length=480)
            mix['routing']['output'] = 'stereo'; mix['routing']['routes'][0]['mapping'] = {'type': 'pan', 'law': law, 'position_milli': position}
            result = check(mix, f'pan-{law}-{position}')[1]
            if law == 'equal_power': assert abs(sum(v*v for v in result[0])/10000**2-1) < .0002
    pan = mix_for('classic-mono.wav', policy='preserve_mono', length=4800, duration=9600); pan['tracks'][0]['clips'][0]['start'] = time(7, 48000)
    curve = {'keys': [{'time': time(0), 'value': -1000, 'interpolation': 'ease_in_out'}, {'time': time(4800, 48000), 'value': 1000, 'interpolation': 'hold'}], 'retime': {'start': time(3, 48000), 'rate': time(3, 2), 'reverse': True}}
    pan['routing']['output'] = 'stereo'; pan['routing']['routes'][0]['mapping'] = {'type': 'pan', 'law': 'equal_power', 'position_milli': 0, 'curve': curve}
    panned = check(pan, 'automated-pan')
    for position in (-1000, 0, 1000):
        mix = mix_for('classic-stereo.wav', policy='preserve_stereo'); mix['routing']['routes'][0]['mapping'] = {'type': 'balance', 'position_milli': position}; check(mix, f'balance-{position}')
    mix['routing']['routes'][0]['mapping']['curve'] = copy.deepcopy({k: v for k, v in curve.items() if k != 'retime'})
    mix['routing']['routes'][0]['mapping']['curve']['keys'][1]['time'] = time(480, 48000)
    check(mix, 'automated-balance')
    passed.append('audio_routing.pan_laws_and_automation')

    eq = {'type': 'low_pass', 'frequency_hz': 2400, 'q': .707}
    processed = mix_for('7.1-48000.wav', length=1920); processed['effects'] = [eq]
    check(processed, 'eight-channel-master-eq', tolerance=1)
    processed = copy.deepcopy(surround); processed['routing']['buses'][0]['effects'] = [eq]; processed['effects'] = [{'type': 'high_pass', 'frequency_hz': 80, 'q': .707}]
    check(processed, 'bus-then-master-eq', tolerance=2)
    compressor = {'type': 'compressor', 'threshold_db': -18, 'ratio': 4, 'attack_ms': 4, 'release_ms': 25, 'makeup_db': 0}
    dynamics = mix_for('steps.wav', length=len(steps)); dynamics['effects'] = [compressor]
    compressed = check(dynamics, 'linked-eight-channel-compressor', tolerance=1)[1]
    assert abs(compressed[7000][0]) < 800  # The rear-right loud channel controls all linked channels.
    bus_dynamics = copy.deepcopy(dynamics); bus_dynamics.pop('effects'); bus_dynamics['routing']['buses'] = [{'id': 'dynamics', 'layout': '7.1', 'effects': [compressor]}]
    bus_dynamics['routing']['routes'] = [route('to-bus', node('track', 'voice'), node('bus', 'dynamics')), route('return', node('bus', 'dynamics'), node('output'))]
    assert check(bus_dynamics, 'bus-compressor', tolerance=1)[1] == compressed
    passed.append('audio_routing.multichannel_processing_and_meters')

    # Scene templates and saved edits use an explicitly routed stereo master.
    scene = {'schema_version': 1, 'id': 'routed-scene', 'width': 4, 'height': 4, 'output_scale': 1, 'duration': time(1, 5), 'background': [0, 0, 0], 'color': 'srgb_straight_encoded', 'audio': None, 'audio_mix': pan,
             'layers': [{'id': 'image', 'canvas': [4, 4], 'start': time(0), 'duration': time(1, 5), 'frames': [{'image': identity(sources/'image.png', sources), 'hold': time(1, 5), 'offset': [0, 0], 'anchor': [0, 0]}], 'timing': 'strict', 'end': 'hold_last', 'transform': {'position': [0, 0], 'crop': [0, 0, 4, 4], 'scale': 1, 'quarter_turns': 0, 'opacity': 255}}]}
    def decode_media(path, audio=False):
        return subprocess.run(['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:a:0' if audio else '0:v:0', *(['-f', 's16le'] if audio else ['-pix_fmt', 'rgb24', '-f', 'rawvideo']), '-'], capture_output=True, check=True).stdout
    dest = output/'scene.mkv'; scene_receipt = call({'command': 'scene.render', 'scene': scene, 'input_root': str(sources), 'output_root': str(output), 'output': str(dest)})
    assert decode_media(dest, True) == pack(panned[1]); assert decode_media(dest) == bytes((25, 90, 170))*4*4*5; video_frames += 5
    client = Client(exe)
    try:
        client.initialize(); catalog = client.rpc('tools/list')['result']['tools']; assert len(catalog) == 63
        tool = next(t for t in catalog if t['name'] == 'cutbolt_audio_inspect'); args = {'mix': pan, 'input_root': str(sources)}
        Draft202012Validator(tool['inputSchema']).validate(args); assert tool['annotations']['readOnlyHint']
        assert client.call('audio.inspect', **args)['pcm_sha256'] == panned[0]['pcm_sha256']
        instantiated = client.call('graphics.instantiate', template={'schema_version': 1, 'id': 'routed-template', 'scene': scene, 'parameters': []}, values={}, instance_id='routed-copy', input_root=str(sources))
        assert instantiated['scene']['audio_mix']['routing'] == pan['routing']
        project = call({'command': 'project.create', 'id': 'routed-edit', 'width': 4, 'height': 4, 'frame_rate': time(25)})
        client.call('session.create', store_root=str(store), project=project, request_id='create'); common = {'store_root': str(store), 'project_id': project['id']}
        asset = scene_receipt['asset']; asset['identity'] = {'sha256': scene_receipt['sha256'], 'bytes': dest.stat().st_size}
        args = {**common, 'expected_revision': 0, 'request_id': 'add', 'operations': [{'op': 'media.add', 'asset': asset}, {'op': 'clip.append', 'clip': {'id': 'sound', 'asset_id': asset['id'], 'source_in': time(1, 25), 'duration': time(3, 25)}}]}
        receipt = client.call('session.apply', **args); assert client.call('session.apply', **args) == receipt
        saved = client.call('session.get', **common); dest = output/'saved.mkv'
        call({'command': 'render.run', 'project': saved, 'input_root': str(output), 'output_root': str(output), 'output': str(dest)})
        assert decode_media(dest, True) == pack(panned[1][1920:7680]); assert decode_media(dest) == bytes((25, 90, 170))*4*4*3; video_frames += 3
        for n in (0, 2):
            dest = output/f'preview-{n}.png'; client.call('preview.frame', project=saved, input_root=str(output), output_root=str(output), output=str(dest), time=time(n, 25))
            with Image.open(dest) as im: assert im.tobytes() == bytes((25, 90, 170))*4*4
            previews += 1
        dest = output/'range.mkv'; call({'command': 'preview.range', 'project': saved, 'input_root': str(output), 'output_root': str(output), 'output': str(dest), 'start': time(1, 25), 'duration': time(1, 25)})
        assert decode_media(dest, True) == pack(panned[1][3840:5760]); assert decode_media(dest) == bytes((25, 90, 170))*4*4; video_frames += 1
        client.call('session.apply', **common, expected_revision=1, request_id='trim', operations=[{'op': 'clip.trim', 'clip_id': 'sound', 'source_in': time(0), 'duration': time(2, 25)}])
        client.call('session.undo', **common, expected_revision=2, request_id='undo'); assert client.call('session.get', **common)['clips'] == saved['clips']
    finally: client.close()
    passed.append('audio_routing.mcp_templates_saved_edits_and_previews')

    # Deep bus chains and route fan-out remain bounded and deterministic.
    deep = mix_for('classic-stereo.wav', policy='preserve_stereo', length=1)
    deep['routing']['buses'] = [{'id': str(i), 'layout': 'stereo'} for i in range(16)]
    deep['routing']['routes'] = [route('start', node('track', 'voice'), node('bus', '0'))]+[route(str(i), node('bus', str(i)), node('bus', str(i+1))) for i in range(15)]+[route('end', node('bus', '15'), node('output'))]
    check(deep, 'maximum-bus-depth')
    many = mix_for('classic-stereo.wav', policy='preserve_stereo', length=7)
    many['routing']['routes'] = [route(str(i), node('track', 'voice'), node('output'), {'type': 'matrix', 'coefficients_milli': [[10, 0], [0, 10]]}) for i in range(64)]
    check(many, 'maximum-routes')
    longest = mix_for('7.1-48000.wav', length=1, duration=2880000)
    client = Client(exe)
    try:
        client.initialize(); started = wall_clock.monotonic()
        info = client.call('audio.inspect', mix=longest, input_root=str(sources))
        inspection_seconds = wall_clock.monotonic()-started; memory = peak_memory(client.process.pid)
        assert memory < 256*1024*1024 and inspection_seconds < 45, (memory, inspection_seconds)
    finally: client.close()
    receipt = call(render_request(longest, 'maximum-duration'))
    raw = (output/'maximum-duration.wav').read_bytes(); first = pack(fixtures['7.1-48000.wav'][0][:1])
    assert len(raw) == 68+2880000*16 and raw[68:84] == first and not any(raw[84:])
    assert info['pcm_sha256'] == receipt['pcm_sha256'] == hashlib.sha256(raw[68:]).hexdigest()
    assert info['graph_live_values'] == 16 and info['samples'] == 2880000
    comparisons.append({'name': 'maximum-duration', 'channels': 8, 'sample_frames': 2880000, 'maximum_pcm_error': 0})
    performance = {'inspection_peak_working_set_bytes': memory, 'inspection_seconds': inspection_seconds, 'graph_live_values': info['graph_live_values'], 'weighted_sample_operations': info['weighted_sample_operations'], 'maximum_output_sample_frames': 2880000}
    (root/'maximum-duration-mix.json').write_text(json.dumps(longest, indent=2)+'\n', encoding='utf-8')
    passed.append('audio_routing.bounded_graphs')

    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    base = mix_for('classic-stereo.wav', policy='preserve_stereo'); bad = []
    def invalid(change, code='INVALID_AUDIO_ROUTING', original=base):
        value = copy.deepcopy(original); change(value); bad.append((value, code))
    invalid(lambda m: m['routing'].update(tracks=[]))
    invalid(lambda m: m['routing']['tracks'][0].update(id='missing'))
    invalid(lambda m: m['routing'].update(routes=[]))
    invalid(lambda m: m['routing']['routes'][0].update(source=node('bus', 'missing')))
    invalid(lambda m: m['routing']['routes'][0].update(destination=node('track', 'voice')))
    invalid(lambda m: m['routing']['routes'][0].update(source=node('output')))
    invalid(lambda m: m['routing']['routes'].append(copy.deepcopy(m['routing']['routes'][0])))
    invalid(lambda m: m['routing'].update(buses=[{'id': 'unused', 'layout': 'stereo'}]))
    invalid(lambda m: m['routing'].update(output='5.1'))
    invalid(lambda m: m['routing']['tracks'][0].update(layout='mono'))
    for matrix in ([[1000]], [[1000, 0], [0, 4001]], [[-4001, 0], [0, 1000]]): invalid(lambda m, matrix=matrix: m['routing']['routes'][0].update(mapping={'type': 'matrix', 'coefficients_milli': matrix}))
    invalid(lambda m: m['routing']['routes'][0].update(mapping={'type': 'pan', 'law': 'linear', 'position_milli': 0}))
    invalid(lambda m: m['routing']['routes'][0].update(mapping={'type': 'balance', 'position_milli': 1001}))
    invalid(lambda m: m['routing']['routes'][0].update(mapping={'type': 'balance', 'position_milli': 0, 'curve': {'keys': [{'time': time(0), 'value': 1001, 'interpolation': 'hold'}]}}), 'INVALID_ANIMATION')
    invalid(lambda m: m['routing'].update(output='unknown'), 'INVALID_JSON')
    invalid(lambda m: m['routing'].update(extra=True), 'INVALID_JSON')
    invalid(lambda m: m['routing']['buses'][0].update(gain_milli=4001), original=fan)
    invalid(lambda m: m['routing']['buses'][0].update(effects=[dict(eq, q=0)]), 'INVALID_AUDIO_EFFECT', original=fan)
    invalid(lambda m: m['routing']['routes'].append(route('cycle', node('bus', 'sum'), node('bus', 'sum'))), original=fan)
    invalid(lambda m: m['routing']['buses'].append({'id': 'sum', 'layout': 'stereo'}), original=fan)
    invalid(lambda m: m['routing']['buses'].append({'id': 'overflow', 'layout': 'stereo'}), original=deep)
    invalid(lambda m: m['routing']['routes'].append(route('65', node('track', 'voice'), node('output'))), original=many)
    invalid(lambda m: m.update(duration=time(60)), 'LIMIT_EXCEEDED', original={**deep, 'effects': [eq]*8})
    invalid(lambda m: [b.update(gain_milli=4000) for b in m['routing']['buses']], 'AUDIO_ROUTING_OVERFLOW', original=deep)
    invalid(lambda m: m['tracks'][0]['clips'][0]['file'].update(sha256='0'*64), 'MEDIA_CHANGED')
    invalid(lambda m: m['tracks'][0]['clips'][0]['file'].update(path='../outside.wav'), 'INVALID_PATH')
    invalid(lambda m: m['tracks'][0]['clips'][0].update(source_in=time(1, 96000)), 'UNALIGNED_TIME')
    invalid(lambda m: m['tracks'][0]['clips'][0].update(source_in=time(1)), 'INVALID_RANGE')
    for i, (value, code) in enumerate(bad): call(render_request(value, 'invalid-'+str(i)), code)
    wrong_scene = copy.deepcopy(scene); wrong_scene['audio_mix']['routing']['output'] = 'mono'; wrong_scene['audio_mix']['routing']['routes'][0]['mapping'] = {'type': 'identity'}
    call({'command': 'scene.render', 'scene': wrong_scene, 'input_root': str(sources), 'output_root': str(output), 'output': str(output/'wrong-scene.mkv')}, 'UNSUPPORTED_AUDIO')
    for name, transform in [('mask', lambda b: struct.pack_into('<I', b, 40, 0)), ('count', lambda b: struct.pack_into('<H', b, 22, 3)), ('precision', lambda b: struct.pack_into('<H', b, 38, 15)), ('subtype', lambda b: b.__setitem__(44, 3)), ('rate', lambda b: struct.pack_into('<I', b, 24, 32000)), ('alignment', lambda b: struct.pack_into('<H', b, 32, 1)), ('byte-rate', lambda b: struct.pack_into('<I', b, 28, 1)), ('riff', lambda b: b.__setitem__(4, 1))]:
        raw = bytearray((sources/'7.1-48000.wav').read_bytes()); transform(raw); path = sources/('invalid-'+name+'.wav'); path.write_bytes(raw)
        value = mix_for('7.1-48000.wav'); value['tracks'][0]['clips'][0]['file'] = identity(path, sources); call(render_request(value, 'invalid-wave-'+name), 'UNSUPPORTED_AUDIO'); path.unlink()
    valid = (sources/'7.1-48000.wav').read_bytes()
    malformed = {'duplicate-format': valid[8:]+valid[12:60], 'duplicate-data': valid[8:]+valid[60:], 'trailing-chunk': valid[8:]+b'bad', 'partial-frame': valid[8:64]+struct.pack('<I', len(valid)-70)+valid[68:-2]}
    for name, body in malformed.items():
        path = sources/('invalid-'+name+'.wav'); path.write_bytes(b'RIFF'+struct.pack('<I', len(body))+body)
        value = mix_for('7.1-48000.wav'); value['tracks'][0]['clips'][0]['file'] = identity(path, sources)
        call(render_request(value, 'invalid-wave-'+name), 'UNSUPPORTED_AUDIO'); path.unlink()
    path = sources/'ambiguous-classic.wav'; path.write_bytes(wave_bytes('7.1', 48000, fixtures['7.1-48000.wav'][0], True))
    value = mix_for('7.1-48000.wav'); value['tracks'][0]['clips'][0]['file'] = identity(path, sources)
    call(render_request(value, 'ambiguous-classic'), 'UNSUPPORTED_AUDIO'); path.unlink()
    conflicting = copy.deepcopy(base); duplicate = copy.deepcopy(conflicting['tracks'][0]['clips'][0]); duplicate['id'] = 'conflict'; duplicate['file']['sha256'] = '0'*64; conflicting['tracks'][0]['clips'].append(duplicate)
    call(render_request(conflicting, 'conflicting-identity'), 'INVALID_IDENTITY')
    hidden = copy.deepcopy(fan); hidden['routing']['buses'][0].update(mute=True, gain_curve={'keys': [{'time': time(0), 'value': 4001, 'interpolation': 'hold'}]})
    call(render_request(hidden, 'invalid-hidden-curve'), 'INVALID_ANIMATION')
    call(render_request(base, 'legacy-equivalence'), 'OUTPUT_EXISTS')
    assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert originals == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('audio_routing.validation_and_preservation')
    report = {'passed': passed, 'cases': comparisons, 'sample_frames_compared': sum(v['sample_frames'] for v in comparisons), 'individual_samples_compared': sum(v['sample_frames']*v['channels'] for v in comparisons), 'scene_session_frames': video_frames, 'frame_previews': previews, 'rejected_cases': rejected, 'performance': performance,
              'reference': 'Original named-channel PCM; independent Fraction voice and whole-buffer incoming-graph algebra; 60-digit Decimal equal-power panning; external per-channel EQ and closed-form linked compressor steps. Exact PCM without effects, declared 1/2-unit processing tolerances. Independent WAV headers/layouts and external full PCM decoding.'}
    (root/'verification.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8'); print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
