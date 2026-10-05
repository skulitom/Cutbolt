"""Original LUT charts and exact Fraction scope references; generated media stays external."""
from engine import ENGINE
import argparse
from array import array
import copy
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import wave
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time
from color import convert_pixel

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 256, 4, 4
STRIDE = W * H * 3


def rounded(v):
    return max(0, min(255, (v + F(1, 2)).numerator // (v + F(1, 2)).denominator))


def reference(rgb, kind, size, mode, low=(0, 0, 0), high=(1, 1, 1)):
    q = [max(F(0), min(F(1), (v - a) / (b - a))) for v, a, b in zip(rgb, low, high)]
    if kind in ('identity1', 'curve1'):
        table = [(F(i, size - 1), F(i, size - 1), F(i, size - 1)) for i in range(size)]
        if kind == 'curve1':
            table = [(F(-1, 4), F(1), F(0)), (F(1, 2), F(1, 4), F(1)), (F(5, 4), F(0), F(1, 4))]
        out = []
        for c, v in enumerate(q):
            x = v * (size - 1)
            if mode == 'nearest':
                out.append(table[int(x + F(1, 2))][c])
            else:
                a = int(x); b = min(size - 1, a + 1); f = x - a
                out.append(table[a][c] * (1 - f) + table[b][c] * f)
        return out
    if mode == 'nearest':
        q = [F(int(v * (size - 1) + F(1, 2)), size - 1) for v in q]
    if kind == 'identity3':
        return q
    # Tables contain products of coordinate pairs. On a simplex the cross term
    # is min(fa,fb), giving an independent closed-form reference for all six tetrahedra.
    result = []
    for a, b in ((0, 1), (1, 2), (0, 2)):
        value = q[a] * q[b]
        if mode == 'tetrahedral':
            xa, xb = q[a] * (size - 1), q[b] * (size - 1)
            ia, ib = min(int(xa), size - 2), min(int(xb), size - 2)
            fa, fb = xa - ia, xb - ib
            value = (ia * ib + ia * fb + ib * fa + min(fa, fb)) / (size - 1) ** 2
        result.append(F(3, 2) * value - F(1, 4) if kind == 'clip3' else value)
    return result


@lru_cache(None)
def transformed(rgb, kind, size, mode, low, high, transfer):
    if transfer != 'srgb':
        rgb = tuple(convert_pixel(*rgb, 'rgb', 'full', 'srgb', transfer))
    return bytes(rounded(v * 255) for v in reference([F(v, 255) for v in rgb], kind, size, mode, low, high))


def scope_reference(rgb, width, columns):
    planes = {name: {'histogram': [0] * 256, 'waveform': [[0] * 256 for _ in range(columns)],
                     'minimum': 255, 'maximum': 0, 'sum': 0} for name in ('red', 'green', 'blue', 'luma')}
    vectors = [[0] * 256 for _ in range(256)]
    for i in range(len(rgb) // 3):
        r, g, b = rgb[3 * i:3 * i + 3]
        y = F(2126, 10000) * r + F(7152, 10000) * g + F(722, 10000) * b
        for name, value in zip(planes, (r, g, b, rounded(y))):
            plane = planes[name]; plane['histogram'][value] += 1
            plane['waveform'][(i % width) * columns // width][value] += 1
            plane['minimum'] = min(plane['minimum'], value); plane['maximum'] = max(plane['maximum'], value)
            plane['sum'] += value
        cb = rounded(128 + (b - y) / F('1.8556')); cr = rounded(128 + (r - y) / F('1.5748'))
        vectors[cr][cb] += 1
    return {**planes, 'vectorscope': vectors}


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = [root / p for p in ('sources', 'output', 'store')]
    for p in (sources, output, store): p.mkdir()
    exe = ENGINE
    passed, renders, scope_checks, rejected = [], [], [], 0

    def ff(args, cwd=None):
        p = subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], cwd=cwd, capture_output=True, timeout=180)
        assert p.returncode == 0, p.stderr.decode(errors='replace')
        return p.stdout

    def call(request, error=None, env=None):
        p = subprocess.run([str(exe)], input=json.dumps(request).encode(), capture_output=True, timeout=180, env=env)
        v = json.loads(p.stdout)
        if error:
            nonlocal rejected
            assert p.returncode == 1 and v['error']['code'] == error, (error, v)
            rejected += 1; return v
        assert p.returncode == 0 and v['ok'], v
        return v['result']

    palette = [(0, 0, 0), (255, 255, 255), (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 0), (0, 255, 255), (255, 0, 255)]
    native = bytes(v for n in range(N) for y in range(H) for x in range(W)
                   for v in ((x,) * 3 if y == 0 else palette[x // 32] if y == 1 else ((x + n * 19) % 256, (x * 7 + n * 11) % 256, (x * 13 + y * 23) % 256)))
    (sources / 'chart.rgb').write_bytes(native)
    pcm = array('h', [(i * 37 + c * 91) % 12001 - 6000 for i in range(N * 1920) for c in range(2)]).tobytes()
    with wave.open(str(sources / 'sound.wav'), 'wb') as f:
        f.setnchannels(2); f.setsampwidth(2); f.setframerate(48000); f.writeframes(pcm)
    tags = ['-colorspace', 'rgb', '-color_range', 'pc', '-color_primaries', 'bt709', '-color_trc', 'iec61966-2-1']
    for name, metadata in [('chart', tags), ('untagged', [])]:
        ff(['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{W}x{H}', '-framerate', '25', '-i', str(sources / 'chart.rgb'),
            '-i', str(sources / 'sound.wav'), '-map', '0:v:0', '-map', '1:a:0', '-vf', 'setsar=1', '-c:v', 'ffv1', '-pix_fmt', 'bgr0',
            '-threads', '1', '-c:a', 'pcm_s16le', *metadata, str(sources / (name + '.mkv'))])
    assert ff(['-i', str(sources / 'chart.mkv'), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']) == native

    def lut_file(label, kind, size, low=(0, 0, 0), high=(1, 1, 1)):
        dim = 1 if kind.endswith('1') else 3
        header = f'# Original generated fixture\nTITLE "{label} # chart"\nLUT_{dim}D_SIZE {size}\n'
        header += 'DOMAIN_MIN ' + ' '.join(str(float(v)) for v in low) + '\nDOMAIN_MAX ' + ' '.join(str(float(v)) for v in high) + '\n'
        values = []
        if dim == 1:
            for i in range(size):
                values.append([F(i, size - 1)] * 3 if kind == 'identity1' else reference([F(i, size - 1)] * 3, kind, size, 'linear'))
        else:
            for b, g, r in itertools.product(range(size), repeat=3):
                q = [F(r, size - 1), F(g, size - 1), F(b, size - 1)]
                values.append(reference(q, kind, size, 'trilinear'))
        path = sources / (label + '.cube')
        path.write_text(header + '\n'.join(' '.join(format(float(v), '.12g') for v in row) for row in values) + '\n', encoding='ascii')
        return identity(path, sources)

    cases = []
    for kind, size in [('identity1', 2), ('curve1', 3), ('identity3', 5), ('cross3', 2), ('cross3', 5), ('clip3', 5)]:
        label = f'{kind}-{size}'; file = lut_file(label, kind, size)
        for mode in (('nearest', 'linear') if kind.endswith('1') else ('nearest', 'trilinear', 'tetrahedral')):
            cases.append((label + '-' + mode, kind, size, mode, (0, 0, 0), (1, 1, 1), file, 'srgb'))
    low, high = (F(-1, 4), F(1, 4), F(0)), (F(3, 4), F(1), F(1, 2))
    domain = lut_file('domain', 'cross3', 5, low, high)
    cases += [('domain-tetrahedral', 'cross3', 5, 'tetrahedral', low, high, domain, 'srgb'),
              ('normalized-before-lut', 'cross3', 5, 'trilinear', (0, 0, 0), (1, 1, 1), cases[10][6], 'bt709')]
    outputs, receipts, recipes = {}, {}, {}
    samples = [tuple(map(F, p)) for p in itertools.permutations((F(1, 4), F(1, 2), F(3, 4)))]
    samples += [(F(v),) * 3 for v in (-1, 0, F(1, 8), F(1, 2), 1, 2)]
    samples += [(F(1, 2), F(1, 2), F(1, 4)), (F(1), F(0), F(1))]
    max_error = 0
    for label, kind, size, mode, low, high, file, transfer in cases:
        transform = {'file': file, 'interpolation': mode}
        inspected = call({'command': 'lut.inspect', 'transform': transform, 'input_root': str(sources), 'samples': [[float(v) for v in s] for s in samples]})
        assert inspected['entries'] == size ** (1 if kind.endswith('1') else 3)
        for s, actual in zip(samples, inspected['samples']):
            want = reference(s, kind, size, mode, low, high)
            assert max(abs(float(a) - b) for a, b in zip(want, actual['output_unclipped'])) < 1e-10, (label, s, want, actual)
            assert max(abs(rounded(a * 255) - b) for a, b in zip(want, actual['output_rgb8'])) <= 1
        recipe = {'schema_version': 1, 'id': label, 'source': {'file': identity(sources / 'chart.mkv', sources),
                  'sdr': {'matrix': 'rgb', 'range': 'full', 'transfer': 'srgb', 'missing_tags': 'reject'}},
                  'working_transfer': transfer, 'lut': transform, 'source_in': time(0), 'duration': time(N, 25), 'rate': time(1),
                  'reverse': False, 'freeze': False, 'width': W, 'height': H, 'audio': 'resample'}
        request = {'command': 'media.conform', 'recipe': recipe, 'input_root': str(sources), 'output_root': str(output), 'output': str(output / (label + '.mkv'))}
        plan = call({'command': 'media.conform.inspect', 'recipe': recipe, 'input_root': str(sources)})
        receipt = call(request)
        assert plan['lut'] == receipt['lut'] and plan['source_frame_indices'] == list(range(N))
        rgb = ff(['-i', request['output'], '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
        want = b''.join(transformed(tuple(native[i:i + 3]), kind, size, mode, low, high, transfer) for i in range(0, len(native), 3))
        assert len(rgb) == len(want)
        difference = max(abs(a - b) for a, b in zip(rgb, want)); assert difference <= 1, (label, difference)
        if kind.startswith('identity') and mode != 'nearest': assert rgb == native
        assert ff(['-i', request['output'], '-vn', '-f', 's16le', '-']) == pcm
        max_error = max(max_error, difference); outputs[label] = rgb; receipts[label] = receipt; recipes[label] = recipe
        renders.append({'case': label, 'frames': N, 'maximum_rgb_error': difference})
    passed += ['lut.one_d_and_three_d_rendering', 'lut.interpolation_domains_and_clipping']

    # External published filter interfaces supply a second, independently implemented reference.
    external = []
    for label, kind, size, mode, low, high, file, transfer in cases:
        if low != (0, 0, 0) or high != (1, 1, 1) or kind == 'clip3' or transfer != 'srgb': continue
        filter_name = 'lut1d' if kind.endswith('1') else 'lut3d'
        rgb = ff(['-i', 'chart.mkv', '-an', '-vf', f'{filter_name}=file={file["path"]}:interp={mode}', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'], sources)
        difference = max(abs(a - b) for a, b in zip(rgb, outputs[label])); assert difference <= 1, (label, difference)
        external.append({'case': label, 'maximum_rgb_error': difference})
    passed.append('lut.external_filter_agreement')

    # Colour matching: the target is the reference with per-channel gain, offset and gamma errors.
    # The LUT is recomputed here from the same sampled frames, and conforming with the returned
    # recipe applies exactly that table to every target frame.
    MW, MH, MN = 64, 36, 12
    match_reference = bytes(v for n in range(MN) for y in range(MH) for x in range(MW) for v in ((x*4+n*7) % 256, (y*7+x+n*3) % 256, (x*y+n*11) % 256))
    def distort(r, g, b):return (min(255, max(0, math.floor(0.8*r+10+.5))), min(255, max(0, math.floor(1.1*g-5+.5))), min(255, math.floor(255*(b/255)**1.3+.5)))
    match_target = bytes(v for i in range(0, len(match_reference), 3) for v in distort(*match_reference[i:i+3]))
    for name, data in (('match-reference', match_reference), ('match-target', match_target)):
        (sources/(name+'.rgb')).write_bytes(data)
        ff(['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{MW}x{MH}', '-framerate', '25', '-i', str(sources/(name+'.rgb')),
            '-vf', 'setsar=1', '-c:v', 'ffv1', '-pix_fmt', 'bgr0', '-threads', '1', *tags, str(sources/(name+'.mkv'))])
    def sampled(path):
        counts = [[0]*256 for _ in range(3)]
        for i in range(9):
            t = F((2*i+1)*MN//18, 25);micros = t.numerator*1000000//t.denominator
            frame = subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-ss', f'{micros//1000000}.{micros % 1000000:06}', '-i', str(path), '-map', '0:v:0', '-frames:v', '1',
                                    '-vf', f'scale={MW}:{MH}:flags=area,format=rgb24', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'], capture_output=True, check=True).stdout
            for j, v in enumerate(frame):counts[j % 3][v] += 1
        return counts
    def statistics(counts):
        total = 0;weighted = 0.0
        for v, n in enumerate(counts):total += n;weighted += v*n
        mean = weighted/total;spread = 0.0
        for v, n in enumerate(counts):spread += (v-mean)**2*n
        return mean, math.sqrt(spread/total)
    def table(target, reference, method):
        if method == 'levels':
            (mt, st), (mr, sr) = statistics(target), statistics(reference);gain = sr/st if st > 0 else 1.0
            return [min(255, max(0, math.floor((v-mt)*gain+mr+.5))) for v in range(256)]
        total_t, total_r = sum(target), sum(reference);cumulative = list(itertools.accumulate(reference));out = [];running = 0
        for v in range(256):
            running += target[v];out.append(next((r for r in range(256) if cumulative[r]*total_t >= running*total_r), 255))
        return out
    reference_counts, target_counts = sampled(sources/'match-reference.mkv'), sampled(sources/'match-target.mkv')
    for method in ('levels', 'histogram'):
        cube = sources/f'match-{method}.cube'
        matched = call({'command':'color.match', 'reference':'match-reference.mkv', 'target':str(sources/'match-target.mkv'), 'input_root':str(sources),
                        'output_root':str(sources), 'output':str(cube), 'method':method, 'transfer':'srgb'})
        tables = [table(target_counts[c], reference_counts[c], method) for c in range(3)]
        assert cube.read_text(encoding='ascii') == 'TITLE "cutbolt colour match"\nLUT_1D_SIZE 256\n'+''.join(f'{tables[0][v]/255:.6f} {tables[1][v]/255:.6f} {tables[2][v]/255:.6f}\n' for v in range(256)), method
        assert matched['frames'] == {'reference':9, 'target':9} and matched['recipe']['lut'] == matched['lut'] == {'file':identity(cube, sources), 'interpolation':'linear'}, (matched['frames'], matched['lut'], identity(cube, sources))
        assert matched['recipe']['source']['sdr'] == {'matrix':'rgb', 'range':'full', 'transfer':'srgb', 'missing_tags':'use_declared'} and matched['recipe']['working_transfer'] == 'srgb'
        out = output/f'matched-{method}.mkv'
        call({'command':'media.conform', 'recipe':matched['recipe'], 'input_root':str(sources), 'output_root':str(output), 'output':str(out)})
        rgb = ff(['-i', str(out), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
        assert rgb == bytes(tables[i % 3][v] for i, v in enumerate(match_target)), method
        if method == 'levels':
            for c in range(3):
                assert abs(matched['matched'][c]['mean']-matched['reference'][c]['mean']) <= 1, (c, matched)
    call({'command':'color.match', 'reference':'match-reference.mkv', 'target':'match-target.mkv', 'input_root':str(sources),
          'output_root':str(sources), 'output':str(sources/'match-levels.cube')}, 'OUTPUT_EXISTS')
    call({'command':'color.match', 'reference':'match-reference.mkv', 'target':'match-target.mkv', 'input_root':str(sources),
          'output_root':str(output), 'output':str(output/'outside.cube')}, 'PATH_OUTSIDE_ROOT')
    assert not (output/'outside.cube').exists()
    call({'command':'color.match', 'reference':'match-reference.mkv', 'target':'match-target.mkv', 'input_root':str(sources), 'target_times':[],
          'output_root':str(sources), 'output':str(sources/'match-empty.cube')}, 'INVALID_ARGUMENT')
    passed.append('lut.colour_match_recomputed_and_applied')

    # Limit boundary tables are parsed and sampled without adding them to the repository.
    for kind, size, mode in [('identity1', 65536, 'linear'), ('identity3', 65, 'tetrahedral')]:
        file = lut_file('maximum-' + kind, kind, size)
        info = call({'command': 'lut.inspect', 'transform': {'file': file, 'interpolation': mode}, 'input_root': str(sources), 'samples': [[0.1, 0.5, 0.9]]})
        assert info['entries'] == size ** (1 if kind.endswith('1') else 3)
        assert max(abs(a - b) for a, b in zip(info['samples'][0]['output_unclipped'], [0.1, 0.5, 0.9])) < 1e-10

    client = Client(exe)
    try:
        client.initialize(); tools = {t['name']: t for t in client.rpc('tools/list')['result']['tools']}
        for command in ('lut.inspect', 'scopes.inspect'):
            assert tools['cutbolt_' + command.replace('.', '_')]['annotations']['readOnlyHint']
        args = {'transform': recipes['cross3-5-tetrahedral']['lut'], 'input_root': str(sources), 'samples': [[0.75, 0.5, 0.25]]}
        Draft202012Validator(tools['cutbolt_lut_inspect']['inputSchema']).validate(args)
        assert client.call('lut.inspect', **args) == call({'command': 'lut.inspect', **args})
        project = call({'command': 'project.create', 'id': 'lut-scopes', 'width': W, 'height': H, 'frame_rate': time(25)})
        client.call('session.create', store_root=str(store), project=project, request_id='create')
        labels = ['curve1-3-linear', 'cross3-5-tetrahedral']; ops = []
        for label in labels:
            ops += [{'op': 'media.add', 'asset': receipts[label]['asset']}, {'op': 'clip.append', 'clip': {'id': label, 'asset_id': label, 'source_in': time(1, 25), 'duration': time(3, 25)}}]
            if label == labels[0]: ops += [{'op': 'clip.append', 'clip': {'id': 'pause', 'gap': True, 'source_in': time(0), 'duration': time(1, 25)}}]
        fields = {'store_root': str(store), 'project_id': project['id'], 'expected_revision': 0, 'request_id': 'assemble', 'operations': ops}
        edit = client.call('session.apply', **fields); assert edit == client.call('session.apply', **fields)
        project = client.call('session.get', store_root=str(store), project_id=project['id'])
        sequence = outputs[labels[0]][STRIDE:] + bytes(STRIDE) + outputs[labels[1]][STRIDE:]
        movie = output / 'timeline.mkv'
        call({'command': 'render.run', 'project': project, 'input_root': str(output), 'output_root': str(output), 'output': str(movie)})
        assert ff(['-i', str(movie), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']) == sequence
        assert ff(['-i', str(movie), '-vn', '-f', 's16le', '-']) == pcm[1920 * 4:] + bytes(1920 * 4) + pcm[1920 * 4:]
        def scope(p, n, cols=7, transfer='srgb', policy='reject'):
            return {'project': p, 'input_root': str(root), 'time': time(n, 25), 'input_transfer': transfer, 'missing_tags': policy, 'columns': cols}
        for n, cols in [(0, 1), (2, 7), (3, 256), (4, 7), (6, 256)]:
            args = scope(project, n, cols)
            Draft202012Validator(tools['cutbolt_scopes_inspect']['inputSchema']).validate(args)
            report = client.call('scopes.inspect', **args)
            assert report == call({'command': 'scopes.inspect', **args})
            rgb = sequence[n * STRIDE:(n + 1) * STRIDE]
            assert report['values'] == scope_reference(rgb, W, cols), (n, cols)
            assert report['pixels'] == W * H and report['rgb_sha256'] == hashlib.sha256(rgb).hexdigest()
            assert report['frame']['source_quality'] == 'original'
            preview = call({'command': 'preview.frame', 'project': project, 'input_root': str(root), 'output_root': str(output), 'output': str(output / f'frame-{n}.png'), 'time': time(n, 25)})
            with Image.open(preview['output']) as image: assert image.tobytes() == rgb
            scope_checks.append({'timeline_frame': n, 'columns': cols, 'pixels': W * H})
        passed.append('scopes.exact_histogram_waveform_vectorscope')
        # A selected, now-offline proxy must never change full-quality scope values.
        proxy = call({'command': 'proxy.generate', 'project': project, 'expected_revision': project['revision'], 'asset_id': labels[0], 'scale': 2, 'input_root': str(root), 'output_root': str(output), 'output': str(output / 'proxy.mkv')})
        # The two-row proxy also catches encoder slice defaults corrupting tiny frames.
        proxy_want = b''.join(outputs[labels[0]][(n * W * H + y * W + x) * 3:(n * W * H + y * W + x) * 3 + 3]
                              for n in range(N) for y in range(0, H, 2) for x in range(0, W, 2))
        assert ff(['-i', str(output / 'proxy.mkv'), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']) == proxy_want
        assert ff(['-i', str(output / 'proxy.mkv'), '-vn', '-f', 's16le', '-']) == pcm
        selected = call({'command': 'timeline.apply', 'project': proxy['project'], 'expected_revision': proxy['project']['revision'], 'operations': [{'op': 'preview.proxy', 'scale': 2}]})
        (output / 'proxy.mkv').rename(output / 'offline-proxy.mkv')
        before = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
        assert call({'command': 'scopes.inspect', **scope(selected, 0)})['values'] == scope_reference(sequence[:STRIDE], W, 7)
        assert before == {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()}
        assert client.call('session.get', store_root=str(store), project_id=project['id']) == project
        passed.append('scopes.timeline_proxy_preview_and_sessions')
    finally:
        client.close()

    bad = [b'', b'LUT_1D_SIZE 1\n', b'LUT_3D_SIZE 66\n', b'LUT_1D_SIZE 65537\n', b'LUT_1D_SIZE -2\n',
           b'LUT_1D_SIZE 2\n0 0 0\n', b'LUT_1D_SIZE 2\n0 0 0\n1 1 1\n2 2 2\n',
           b'LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n', b'LUT_1D_SIZE 2\nLUT_1D_SIZE 2\n', b'0 0 0\n',
           b'LUT_1D_SIZE 2\n0 0\n1 1 1\n', b'LUT_1D_SIZE 2\n0 0 0\nDOMAIN_MIN 0 0 0\n1 1 1\n',
           b'TITLE unquoted\n', b'TITLE "one" "two"\n', b'TITLE "' + b'x' * 257 + b'"\n',
           b'#' + b'x' * 4096 + b'\n', b'\xef\xbb\xbfLUT_1D_SIZE 2\n', b'\0', b'\x7f', b'\xff',
           b'LUT_1D_INPUT_RANGE 0 1\n', b'LUT_1D_SIZE 2\n0 0 0\nNaN 1 1\n', b'LUT_1D_SIZE 2\n0 0 0\ninf 1 1\n',
           b'LUT_1D_SIZE 2\n0 0 0\n16.1 1 1\n', b'DOMAIN_MIN 0 0 0\nDOMAIN_MIN 0 0 0\n']
    for header in ('DOMAIN_MIN 1 0 0', 'DOMAIN_MAX -1 1 1', 'DOMAIN_MAX 0.0000001 1 1', 'DOMAIN_MIN 0 0', 'DOMAIN_MAX 17 1 1'):
        bad.append((header + '\nLUT_1D_SIZE 2\n0 0 0\n1 1 1\n').encode())
    for i, data in enumerate(bad):
        path = sources / f'invalid-{i}.cube'; path.write_bytes(data)
        call({'command': 'lut.inspect', 'transform': {'file': identity(path, sources), 'interpolation': 'linear'}, 'input_root': str(sources)}, 'INVALID_LUT' if data else 'INVALID_IDENTITY')
    base = recipes['curve1-3-linear']; transform = base['lut']
    for change, code in [({'sha256': '0' * 64}, 'MEDIA_CHANGED'), ({'bytes': 8388609}, 'INVALID_LUT'), ({'path': '../escape.cube'}, 'INVALID_PATH'), ({'path': 'test.txt'}, 'INVALID_LUT')]:
        t = copy.deepcopy(transform); t['file'].update(change)
        call({'command': 'lut.inspect', 'transform': t, 'input_root': str(sources)}, code)
    for t in [dict(transform, interpolation='tetrahedral'), dict(recipes['cross3-5-nearest']['lut'], interpolation='linear')]:
        call({'command': 'lut.inspect', 'transform': t, 'input_root': str(sources)}, 'INVALID_LUT')
    for colors in ([[0, 0, 0]] * 257, [[17, 0, 0]]):
        call({'command': 'lut.inspect', 'transform': transform, 'input_root': str(sources), 'samples': colors}, 'INVALID_LUT')
    original_hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    output_hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    def render_request(recipe, name='invalid-output'):
        return {'command': 'media.conform', 'recipe': recipe, 'input_root': str(sources), 'output_root': str(output), 'output': str(output / (name + '.mkv'))}
    r = copy.deepcopy(base); r.pop('working_transfer'); call(render_request(r), 'INVALID_COLOR')
    r = copy.deepcopy(base); r['source'].pop('sdr'); r['source']['color'] = 'encoded_rgb'; r.pop('working_transfer'); call(render_request(r), 'INVALID_LUT')
    call(render_request(base, 'curve1-3-linear'), 'OUTPUT_EXISTS')
    req = render_request(base); req.update(output_root=str(sources), output=str(sources / 'chart.mkv')); call(req, 'OUTPUT_EXISTS')
    # Tool substitution changes only this owned synthetic LUT after output encoding.
    wrapper = root / 'mutate_lut.rs'; tool = root / 'mutate_lut.exe'
    wrapper.write_text('''use std::{env,process::Command,fs::OpenOptions,io::Write};
fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();
if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){let mut f=OpenOptions::new().append(true).open(env::var("FIXTURE_LUT").unwrap()).unwrap();writeln!(f,"# changed during conversion").unwrap();}
std::process::exit(status.code().unwrap_or(1));}''', encoding='utf-8')
    subprocess.run(['rustc', str(wrapper), '-o', str(tool)], check=True, capture_output=True)
    lut_path = (sources / transform['file']['path']).resolve(); assert sources in lut_path.parents
    saved = lut_path.read_bytes()
    try:
        call(render_request(base), 'MEDIA_CHANGED', {**os.environ, 'CUTBOLT_FFMPEG': str(tool), 'REFERENCE_FFMPEG': shutil.which('ffmpeg'), 'FIXTURE_LUT': str(lut_path)})
        assert lut_path.read_bytes() != saved
    finally:
        lut_path.write_bytes(saved)
    assert original_hashes == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert output_hashes == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('lut.validation_identity_and_preservation')

    for fields, code in [({'columns': 0}, 'INVALID_SCOPE'), ({'columns': 257}, 'INVALID_SCOPE'), ({'time': time(7, 25)}, 'INVALID_RANGE'),
                         ({'time': time(1, 30)}, 'UNALIGNED_TIME'), ({'input_transfer': 'bt709'}, 'INVALID_COLOR'),
                         ({'input_transfer': 'pq'}, 'INVALID_JSON'), ({'missing_tags': 'auto'}, 'INVALID_JSON'), ({'input_root': str(store)}, 'PATH_OUTSIDE_ROOT')]:
        call({'command': 'scopes.inspect', **scope(project, 0), **fields}, code)
    p = copy.deepcopy(project); p['assets'][0]['identity']['sha256'] = '0' * 64
    call({'command': 'scopes.inspect', **scope(p, 0)}, 'IDENTITY_MISMATCH')
    p = copy.deepcopy(project); p['assets'][0]['path'] = str(sources / 'untagged.mkv'); p['assets'][0].pop('identity')
    call({'command': 'scopes.inspect', **scope(p, 0)}, 'INVALID_COLOR')
    assumed = call({'command': 'scopes.inspect', **scope(p, 0, policy='use_declared')})
    assert assumed['values'] == scope_reference(native[STRIDE:2 * STRIDE], W, 7) and assumed['interpretation']['assumed_tags']
    p = copy.deepcopy(project); p['width'] = 4096; p['height'] = 2048
    p['clips'] = [{'id': 'black', 'gap': True, 'source_in': time(0), 'duration': time(1)}]
    call({'command': 'scopes.inspect', **scope(p, 0)}, 'LIMIT_EXCEEDED')
    p['width'] = 257; p['height'] = 1
    uneven = call({'command': 'scopes.inspect', **scope(p, 0, 256)})
    assert uneven['values'] == scope_reference(bytes(257 * 3), 257, 256)
    p['width'] = 6
    call({'command': 'scopes.inspect', **scope(p, 0, 7)}, 'INVALID_SCOPE')
    assert output_hashes == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert original_hashes == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    passed.append('scopes.invalid_requests_and_preservation')
    report = {'passed': passed, 'renders': renders, 'external_filter_comparisons': external, 'scopes': scope_checks,
              'frames_compared': len(renders) * N + 7 + N, 'stereo_sample_frames_compared': (len(renders) * N + 7 + N) * 1920,
              'proxy_frames_compared': N,
              'maximum_rgb_error': max_error, 'rejected_cases': rejected,
              'threshold': {'maximum_lut_rgb_error': 1, 'maximum_external_rgb_error': 1, 'maximum_scope_count_error': 0},
              'reference': 'Original synthetic LUT tables and charts. Exact Fraction analytic cross terms for every tetrahedron, multilinear products and one-dimensional knots; exact integer-population scope oracle; independent external lut1d/lut3d filters. Every decoded RGB pixel and PCM sample checked.',
              'limits': '8-bit encoded SDR, single 1D or 3D ASCII cube, explicit interpolation/domain clamping. Numerical full-quality frame scopes only; no HDR, display management, combined shapers, temporal scopes or live LUT scene effects.'}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    (root / 'recipe.json').write_text(json.dumps(base, indent=2) + '\n', encoding='utf-8')
    (root / 'project.json').write_text(json.dumps(project, indent=2) + '\n', encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
