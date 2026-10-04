"""Authored speed integrals, complete RGB/PCM references and pitch measurements."""
from engine import ENGINE, MCP_TOOLS
import argparse
from bisect import bisect_right
import copy
from fractions import Fraction as F
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import struct
import subprocess
import wave

from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from color import convert_pixel
from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 16, 100


def rational(v):
    return F(v['num'], v['den'])


def clock(v):
    v = F(v)
    return time(v.numerator, v.denominator)


def segment(duration, first, last, reverse=False):
    return {'duration': clock(duration), 'start_rate': clock(first), 'end_rate': clock(last), 'reverse': reverse}


def position(recipe, t):
    # Independent polynomial integration: signed speed slope, with full preceding
    # trapezoid areas accumulated in arbitrary-precision Fraction arithmetic.
    source = rational(recipe['source_in'])
    for s in recipe['remap']['segments']:
        d = rational(s['duration']); a = rational(s['start_rate']); b = rational(s['end_rate'])
        u = min(t, d)
        source += (-1 if s['reverse'] else 1) * (a*u + (b-a)*u*u/(2*d))
        if t <= d:
            return source
        t -= d
    raise AssertionError('Outside fixture clock')


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, out, store = (root / n for n in ('sources', 'output', 'store'))
    for p in (sources, out, store): p.mkdir()
    exe = ENGINE
    passed, cases, pitches = [], [], []
    frame_count = sample_count = rejected = previews = 0

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=120).stdout

    def decode(path, audio=False):
        return ff(['-i', str(path), '-map', '0:a:0', '-f', 's16le', '-'] if audio else
                  ['-i', str(path), '-map', '0:v:0', '-fps_mode', 'passthrough', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])

    def call(req, error=None, env=None):
        nonlocal rejected
        p = subprocess.run([str(exe)], input=json.dumps(req).encode(), capture_output=True, timeout=180, env=env)
        data = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and data['error']['code'] == error, data
            rejected += 1
            return data
        assert p.returncode == 0 and data['ok'], data
        return data['result']

    raw = bytes(2*n if c == 0 else (x*17+y*11+n*19)%256 if c == 1 else (x*3+y*29+n*37)%256
                for n in range(N) for y in range(H) for x in range(W) for c in range(3))
    (sources/'frames.rgb').write_bytes(raw)
    for rate, channels in ((48000, 2), (44100, 2), (24000, 1)):
        with wave.open(str(sources/f'audio-{rate}.wav'), 'wb') as f:
            f.setnchannels(channels); f.setsampwidth(2); f.setframerate(rate)
            f.writeframes(b''.join(struct.pack('<'+'h'*channels, *[round(18000*math.sin(2*math.pi*(220 if c == 0 else 337)*i/rate)) for c in range(channels)]) for i in range(rate*4)))
    for name, rate, wav, filter_ in [('cfr', '25', 48000, None), ('fractional', '30000/1001', 44100, None),
                                     ('vfr', '25', 24000, 'settb=1/1000,setpts=N*40+floor(N/5)*20')]:
        args = ['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{W}x{H}', '-framerate', rate, '-i', str(sources/'frames.rgb'), '-i', str(sources/f'audio-{wav}.wav'), '-map', '0:v:0', '-map', '1:a:0']
        if filter_: args += ['-vf', filter_, '-enc_time_base', '1/1000']
        args += ['-fps_mode', 'passthrough', '-c:v', 'ffv1', '-pix_fmt', 'bgr0', '-threads', '1', '-c:a', 'pcm_s16le', str(sources/(name+'.mkv'))]
        ff(args)
    ff(['-i', str(sources/'fractional.mkv'), '-map', '0', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-g', '12', '-bf', '2', '-colorspace', 'bt709', '-color_range', 'tv', '-color_primaries', 'bt709', '-color_trc', 'bt709', '-c:a', 'aac', str(sources/'fractional.mp4')])
    ff(['-i', str(sources/'cfr.mkv'), '-map', '0:v:0', '-c', 'copy', str(sources/'silent.mkv')])
    # Tagged normalization and a nonlinear 1D table expose temporal/color ordering.
    ff(['-i', str(sources/'cfr.mkv'), '-map', '0', '-c', 'copy', '-colorspace', 'rgb', '-color_range', 'pc', '-color_primaries', 'bt709', '-color_trc', 'iec61966-2-1', str(sources/'tagged.mkv')])
    table = sources/'curve.cube'
    table.write_text('LUT_1D_SIZE 3\n0 0 0\n0.25 0.25 0.25\n1 1 1\n', encoding='ascii')
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    refs = {}

    def reference(name):
        if name in refs: return refs[name]
        path = sources/name
        meta = json.loads(subprocess.run(['ffprobe', '-v', 'error', '-show_streams', '-show_frames', '-of', 'json', str(path)], capture_output=True, check=True).stdout)
        vs = next((s for s in meta['streams'] if s['codec_type'] == 'video'), None)
        a = next((s for s in meta['streams'] if s['codec_type'] == 'audio'), None)
        pts = [int(f['best_effort_timestamp'])*F(vs['time_base']) for f in meta['frames'] if f['media_type'] == 'video'] if vs else []
        pixels = decode(path) if vs else b''
        if name.endswith('.mp4'):
            pixels = ff(['-i', str(path), '-map', '0:v:0', '-fps_mode', 'passthrough', '-vf', 'scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
        pcm = list(struct.iter_unpack('<'+'h'*a['channels'], decode(path, True))) if a else []
        half = F(vs['time_base'])/2 if vs else F(0)
        refs[name] = pixels, pts, pcm, int(a['sample_rate']) if a else 0, a['channels'] if a else 0, half
        return refs[name]

    def recipe(name, spans, mode='linear', mute=False, start=F(1, 10), **fields):
        value = {'schema_version': 1, 'id': name, 'source': {'file': identity(sources/name, sources), 'color': None if name.endswith('.wav') else 'bt709_limited' if name.endswith('.mp4') else 'encoded_rgb'},
                 'source_in': clock(start), 'duration': clock(sum(rational(s['duration']) for s in spans)), 'rate': time(1), 'reverse': False, 'freeze': False,
                 'width': W, 'height': H, 'audio': 'mute' if mute else 'resample',
                 'remap': {'segments': spans, 'video_sampling': mode, 'audio_pitch': 'mute' if mute else 'follow_speed'}}
        value.update(fields)
        return value

    def request(value, label):
        return {'command': 'media.conform', 'recipe': value, 'input_root': str(sources), 'output_root': str(out), 'output': str(out/(label+'.mkv'))}

    def expected(value):
        rgb, pts, pcm, rate, channels, half = reference(value['source']['file']['path'])
        if 'working_transfer' in value:
            mapping = [convert_pixel(v, v, v, 'rgb', 'full', 'srgb', value['working_transfer'])[0] for v in range(256)]
            if 'lut' in value:
                mapping = [int((F(v, 2) if v <= F(255, 2) else F(3*v, 2)-F(255, 2)) + F(1, 2)) for v in mapping]
            rgb = bytes(mapping[v] for v in rgb)
        pixels, audio, samples, times = bytearray(), bytearray(), [], []
        mode = value['remap']['video_sampling']; width, height = value['width'], value['height']
        for n in range(int(rational(value['duration'])*25)):
            t = position(value, F(n, 25)); times.append(clock(t))
            if not pts:
                pixels.extend(bytes(width*height*3)); continue
            # A frame within half a source tick after t is the frame at t (rounded container times).
            low = max(bisect_right(pts, t+half)-1, 0); high = min(low+1, len(pts)-1)
            weight = (t-pts[low])/(pts[high]-pts[low]) if high != low and t >= pts[low] else F(0)
            if mode == 'previous': high = low; weight = F(0)
            elif mode == 'nearest':
                low = high if weight >= F(1, 2) else low
                high = low; weight = F(0)
            elif weight == 0: high = low
            samples.append({'source_time': clock(t), 'first': low, 'second': high, 'second_weight': clock(weight)})
            a = rgb[low*W*H*3:(low+1)*W*H*3]; b = rgb[high*W*H*3:(high+1)*W*H*3]
            for y in range(height):
                for x in range(width):
                    p = (y*H//height*W+x*W//width)*3
                    pixels.extend(int(a[p+c]*(1-weight)+b[p+c]*weight+F(1, 2)) for c in range(3))
        for n in range(int(rational(value['duration'])*48000)):
            if value['audio'] == 'mute' or not pcm: audio.extend(bytes(4)); continue
            pos = position(value, F(n, 48000))*rate; low = int(pos); high = min(low+1, len(pcm)-1); r = pos-low
            for c in range(2):
                ch = c if channels == 2 else 0
                v = pcm[low][ch]*(1-r)+pcm[high][ch]*r
                audio.extend(struct.pack('<h', int(abs(v)+F(1, 2))*(-1 if v < 0 else 1)))
        return bytes(pixels), bytes(audio), samples, times

    def check(value, label):
        nonlocal frame_count, sample_count
        req = {'command': 'media.conform.inspect', 'recipe': value, 'input_root': str(sources)}
        plan = call(req); assert call(req) == plan
        receipt = call(request(value, label)); assert receipt['remap'] == plan['remap']
        rgb, pcm, samples, times = expected(value)
        assert receipt['remap']['source_times'] == times
        assert receipt['remap']['video_samples'] == samples
        assert receipt['source_frame_indices'] == [s['first'] for s in samples]
        actual = decode(out/(label+'.mkv'))
        assert len(actual) == len(rgb)
        pixel_error = max(abs(a-b) for a, b in zip(actual, rgb))
        assert pixel_error <= (1 if 'lut' in value else 0), (label, pixel_error)
        assert decode(out/(label+'.mkv'), True) == pcm, label
        assert receipt['asset']['identity']['sha256'] == hashlib.sha256((out/(label+'.mkv')).read_bytes()).hexdigest()
        clock_ = F(0)
        for span, authored in zip(receipt['remap']['segments'], value['remap']['segments']):
            end = clock_+rational(authored['duration'])
            assert span['output_start'] == clock(clock_) and span['output_end'] == clock(end)
            assert span['source_start'] == clock(position(value, clock_)) and span['source_end'] == clock(position(value, end))
            clock_ = end
        frame_count += receipt['frames']; sample_count += receipt['samples']
        cases.append({'name': label, 'frames': receipt['frames'], 'stereo_sample_frames': receipt['samples'], 'maximum_rgb_error': pixel_error})
        (root/(label+'-recipe.json')).write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8')
        (root/(label+'-receipt.json')).write_text(json.dumps(receipt, indent=2)+'\n', encoding='utf-8')
        return receipt, rgb, pcm

    ramp = recipe('cfr.mkv', [segment(2, F(1, 4), 2)])
    up = check(ramp, 'accelerate')
    down = check(recipe('cfr.mkv', [segment(2, 2, F(1, 4))]), 'decelerate')
    split = check(recipe('cfr.mkv', [segment(1, F(1, 4), F(9, 8)), segment(1, F(9, 8), 2)]), 'equivalent-split')
    assert up[1:] == split[1:]
    check(recipe('cfr.mkv', [segment(1, 0, 2)], start=0), 'start-at-rest')
    check(recipe('cfr.mkv', [segment(1, 2, 0)], start=0), 'stop-at-end')
    piecewise = [segment(F(1, 10), F(1, 2), F(3, 2)), segment(F(3, 10), F(3, 2), 3), segment(F(3, 5), F(1, 4), F(3, 4))]
    check(recipe('cfr.mkv', piecewise, start=F(1, 96000)), 'between-frame-keys')
    check(recipe('cfr.mkv', [segment(F(1, 25), 0, 16)], start=0), 'maximum-ramp-rate')
    check(recipe('cfr.mkv', [segment(1, F(1, 32), F(1, 16))], start=0), 'very-slow')
    fine = [segment(F(1, 48000), F(1, 2) if i%2 else F(3, 2), F(3, 2) if i%2 else F(1, 2)) for i in range(63)]
    fine.append(segment(F(1, 25)-F(63, 48000), F(1, 2), F(3, 2)))
    check(recipe('cfr.mkv', fine, start=0), 'maximum-segments-sample-boundaries')
    passed.append('remapping.exact_variable_speed_and_boundaries')

    previous = check(recipe('cfr.mkv', ramp['remap']['segments'], mode='previous'), 'previous')
    nearest = check(recipe('cfr.mkv', ramp['remap']['segments'], mode='nearest'), 'nearest')
    assert len({up[1], previous[1], nearest[1]}) == 3
    # The fixture red channel is exactly 50*source_seconds. Linear sampling
    # reconstructs that physical intensity; held/nearest frames show stepping.
    errors = {}
    for name, result in [('linear', up), ('previous', previous), ('nearest', nearest)]:
        errors[name] = sum(abs(result[1][n*W*H*3]-50*position(ramp, F(n, 25))) for n in range(50))/50
    assert errors['linear'] <= F(1, 2) and errors['linear'] < errors['nearest'] < errors['previous'], errors
    ties = check(recipe('cfr.mkv', [segment(F(2, 5), F(1, 2), F(1, 2))], mode='nearest', start=F(1, 50)), 'nearest-ties')
    assert ties[0]['source_frame_indices'][0] == 1
    check(recipe('cfr.mkv', [segment(F(1, 5), 0, 0)], start=F(399, 100), mute=True), 'last-frame-extension')
    check(recipe('vfr.mkv', piecewise), 'variable-timestamps-mono')
    check(recipe('fractional.mkv', piecewise), 'fractional-timestamps-stereo')
    check(recipe('fractional.mp4', piecewise), 'fractional-codec-reordering')
    check(recipe('silent.mkv', piecewise), 'missing-audio-silence')
    check(recipe('cfr.mkv', piecewise, width=17, height=9), 'resize-after-blend')
    passed.append('remapping.timestamp_interpolation_and_quality')

    yoyo = [segment(1, F(1, 5), F(9, 5)), segment(F(1, 5), 0, 0), segment(F(4, 5), 2, F(1, 2), True)]
    reversal = check(recipe('cfr.mkv', yoyo, mute=True, start=F(2, 5)), 'forward-freeze-reverse')
    assert reversal[1][25*W*H*3:26*W*H*3]*5 == reversal[1][25*W*H*3:30*W*H*3]
    check(recipe('cfr.mkv', [segment(1, 2, 0, True)], mute=True, start=1), 'reverse-to-zero')
    check(recipe('cfr.mkv', [segment(F(1, 5), 0, 0)], mute=True, start=F(3, 50)), 'freeze-between-frames')
    # A discontinuity in authored speed does not cause a source-position jump.
    check(recipe('cfr.mkv', [segment(F(1, 2), F(1, 4), F(1, 4)), segment(F(1, 2), 2, 2)]), 'speed-step')
    passed.append('remapping.reverse_freeze_and_continuity')

    for name, value, result in [('accelerate', ramp, up), ('decelerate', recipe('cfr.mkv', [segment(2, 2, F(1, 4))]), down)]:
        left = [v[0] for v in struct.iter_unpack('<hh', result[2])]
        for center in (F(1, 4), F(1), F(7, 4)):
            lo, hi = int((center-F(1, 10))*48000), int((center+F(1, 10))*48000)
            crossings = [i+F(-left[i], left[i+1]-left[i]) for i in range(lo, hi-1) if left[i] <= 0 < left[i+1]]
            measured = (len(crossings)-1)*48000/(crossings[-1]-crossings[0])
            s = value['remap']['segments'][0]
            midpoint = (crossings[-1]+crossings[0])/96000
            wanted = 220*(rational(s['start_rate'])+(rational(s['end_rate'])-rational(s['start_rate']))*midpoint/2)
            assert abs(float(measured-wanted)) < .15, (name, center, measured, wanted)
            pitches.append({'case': name, 'time': float(midpoint), 'expected_hz': float(wanted), 'measured_hz': float(measured)})
    check(recipe('audio-24000.wav', piecewise, start=F(1, 96000)), 'audio-only-mono')
    check(recipe('audio-44100.wav', piecewise, start=F(1, 96000)), 'audio-only-stereo')
    passed.append('remapping.pitch_policy_and_pcm')

    normalized = recipe('tagged.mkv', piecewise)
    normalized['source'].pop('color'); normalized['source']['sdr'] = {'matrix': 'rgb', 'range': 'full', 'transfer': 'srgb', 'missing_tags': 'reject'}
    normalized['working_transfer'] = 'bt709'
    check(normalized, 'normalize-before-blend')
    graded = copy.deepcopy(normalized); graded['lut'] = {'file': identity(table, sources), 'interpolation': 'linear'}
    ordered = check(graded, 'lut-before-blend')
    before_table = expected(normalized)[0]
    wrong_order = bytes(int((F(v, 2) if v <= F(255, 2) else F(3*v, 2)-F(255, 2))+F(1, 2)) for v in before_table)
    assert max(abs(a-b) for a, b in zip(ordered[1], wrong_order)) > 20
    passed.append('remapping.color_and_lut_order')

    client = Client(exe)
    try:
        client.initialize(); catalog = client.rpc('tools/list')['result']['tools']; assert len(catalog) == MCP_TOOLS
        tool = next(t for t in catalog if t['name'] == 'cutbolt_media_conform_inspect')
        fields = {'recipe': ramp, 'input_root': str(sources)}
        Draft202012Validator(tool['inputSchema']).validate(fields); assert tool['annotations']['readOnlyHint']
        assert client.call('media.conform.inspect', **fields)['remap'] == up[0]['remap']
        project = call({'command': 'project.create', 'id': 'ramped-edit', 'width': W, 'height': H, 'frame_rate': time(25)})
        client.call('session.create', store_root=str(store), project=project, request_id='create')
        common = {'store_root': str(store), 'project_id': project['id']}
        fields = {**common, 'expected_revision': 0, 'request_id': 'add', 'operations': [{'op': 'media.add', 'asset': up[0]['asset']}, {'op': 'clip.append', 'clip': {'id': 'ramped', 'asset_id': up[0]['asset']['id'], 'source_in': time(1, 5), 'duration': time(6, 5)}}]}
        receipt = client.call('session.apply', **fields); assert client.call('session.apply', **fields) == receipt
        saved = client.call('session.get', **common)
        dest = out/'saved.mkv'; call({'command': 'render.run', 'project': saved, 'input_root': str(out), 'output_root': str(out), 'output': str(dest)})
        assert decode(dest) == up[1][5*W*H*3:35*W*H*3] and decode(dest, True) == up[2][9600*4:67200*4]
        frame_count += 30; sample_count += 57600
        for n in (0, 1, 11, 29):
            dest = out/f'preview-{n}.png'; client.call('preview.frame', project=saved, input_root=str(out), output_root=str(out), output=str(dest), time=time(n, 25))
            with Image.open(dest) as im: assert im.tobytes() == up[1][(n+5)*W*H*3:(n+6)*W*H*3]
            previews += 1
        dest = out/'range.mkv'; call({'command': 'preview.range', 'project': saved, 'input_root': str(out), 'output_root': str(out), 'output': str(dest), 'start': time(4, 25), 'duration': time(8, 25)})
        assert decode(dest) == up[1][9*W*H*3:17*W*H*3] and decode(dest, True) == up[2][9*1920*4:17*1920*4]
        frame_count += 8; sample_count += 15360
        client.call('session.apply', **common, expected_revision=1, request_id='trim', operations=[{'op': 'clip.trim', 'clip_id': 'ramped', 'source_in': time(2, 5), 'duration': time(1)}])
        client.call('session.undo', **common, expected_revision=2, request_id='undo'); assert client.call('session.get', **common)['clips'] == saved['clips']
    finally: client.close()
    passed.append('remapping.mcp_saved_history_and_previews')

    bad = []
    for key, val in [('segments', []), ('segments', [segment(F(1, 100), 1, 1)]*65), ('segments', [segment(1, 1, 1)]), ('segments', [segment(3, 1, 1)]),
                     ('segments', [segment(2, 0, 0)]), ('segments', [segment(2, 1, 1, True)]), ('segments', [segment(2, 17, 1)]), ('segments', [segment(0, 1, 1)]), ('audio_pitch', 'mute')]:
        r = copy.deepcopy(ramp); r['remap'][key] = val; bad.append((r, 'INVALID_REMAP'))
    for key, val, error in [('rate', time(2), 'INVALID_REMAP'), ('reverse', True, 'INVALID_CONFORM'), ('freeze', True, 'INVALID_CONFORM'), ('audio', 'mute', 'INVALID_REMAP'), ('source_in', time(3), 'INVALID_RANGE'), ('source_in', time(4), 'INVALID_RANGE')]:
        r = copy.deepcopy(ramp); r[key] = val; bad.append((r, error))
    for key, val in [('video_sampling', 'optical_flow'), ('audio_pitch', 'preserve_pitch'), ('extra', True)]:
        r = copy.deepcopy(ramp); r['remap'][key] = val; bad.append((r, 'INVALID_JSON'))
    r = copy.deepcopy(ramp); r['remap']['segments'][0]['duration'] = time(1, 7); bad.append((r, 'UNALIGNED_TIME'))
    r = copy.deepcopy(ramp); r['remap']['segments'][0]['start_rate'] = {'num': 1, 'den': 0}; bad.append((r, 'INVALID_TIME'))
    r = copy.deepcopy(ramp); r['remap']['segments'][0]['end_rate'] = time(1, 9007199254740991); bad.append((r, 'TIME_OVERFLOW'))
    bad.append((recipe('cfr.mkv', [segment(1, 1, 1, True)], start=F(1, 2), mute=True), 'INVALID_RANGE'))
    # A brief excursion beyond the source must fail even when no output frame
    # falls inside it and the following reverse segment returns to range.
    bad.append((recipe('cfr.mkv', [segment(F(1, 100), 16, 16), segment(F(1, 100), 16, 16, True), segment(F(1, 50), 0, 0)], start=F(39, 10), mute=True), 'INVALID_RANGE'))
    bad.append((recipe('cfr.mkv', [segment(F(1, 25), 0, 0)], start=4, mute=True), 'INVALID_RANGE'))
    bad.append((recipe('vfr.mkv', [segment(1, F(1, 2), F(1, 2))], start=F(15, 4)), 'INVALID_RANGE'))
    r = copy.deepcopy(ramp); r['source']['file']['sha256'] = '0'*64; bad.append((r, 'MEDIA_CHANGED'))
    r = copy.deepcopy(ramp); r['source']['file']['path'] = '../outside.mkv'; bad.append((r, 'INVALID_PATH'))
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    for i, (value, error) in enumerate(bad):
        call(request(value, 'invalid-'+str(i)), error); assert not (out/('invalid-'+str(i)+'.mkv')).exists()
    call(request(ramp, 'accelerate'), 'OUTPUT_EXISTS')
    wrapper = root/'change-source.rs'; tool = root/'change-source.exe'
    wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}', encoding='utf-8')
    subprocess.run(['rustc', str(wrapper), '-o', str(tool)], capture_output=True, check=True)
    for source, value in [(sources/'cfr.mkv', ramp), (table, graded)]:
        old = source.read_bytes()
        try: call(request(value, 'changed-source'), 'MEDIA_CHANGED', {**os.environ, 'CUTBOLT_FFMPEG': str(tool), 'REFERENCE_FFMPEG': shutil.which('ffmpeg'), 'FIXTURE_SOURCE': str(source)})
        finally: source.write_bytes(old)
    assert before == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    assert originals == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(out.glob('.cutbolt-scene-*'))
    passed.append('remapping.validation_and_source_preservation')
    report = {'passed': passed, 'cases': cases, 'frames_compared': frame_count, 'stereo_sample_frames_compared': sample_count,
              'rejected_cases': rejected, 'frame_previews': previews, 'pitch_measurements': pitches,
              'physical_intensity_mean_errors': {k: float(v) for k, v in errors.items()},
              'reference': 'Independent signed polynomial speed integration in Fraction, timestamp search, exact frame blending and PCM interpolation; analytic time-varying tone phase/crossings and linear encoded-intensity chart. Complete decoded RGB/PCM comparisons; one-unit existing LUT rounding tolerance, otherwise exact RGB and PCM. Independent Decimal normalization and nonlinear table ordering. Codec decoding uses the external media tools.'}
    (root/'verification.json').write_text(json.dumps(report, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    p = argparse.ArgumentParser(); p.add_argument('--output', type=Path, required=True)
    run(p.parse_args().output)
