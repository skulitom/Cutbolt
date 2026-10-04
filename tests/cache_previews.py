"""Original content-cache fixtures and independent contact-sheet/interval pixels."""
import argparse
from array import array
from concurrent.futures import ThreadPoolExecutor
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import statistics
import subprocess
import time as clock

from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from tracks import time, placement, track
from sequences import arrangement, populated, nested, effect

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 16, 32
POLICY = {'max_bytes': 64*1024*1024, 'max_entries': 128}


def digest(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, out, cache, store = [root / name for name in ('sources', 'output', 'cache', 'sessions')]
    for p in (sources, out, cache, store):
        p.mkdir()
    exe = ROOT / 'target/debug/cutbolt.exe'
    passed, cases, timings = [], [], []
    rejected = frames = samples = 0

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=120).stdout

    def call(command, error=None, env=None, **fields):
        nonlocal rejected
        request = {'command': command, **fields}
        p = subprocess.run([str(exe)], input=json.dumps(request).encode(), capture_output=True, timeout=180, env=env)
        v = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and v['error']['code'] == error, (error, v)
            rejected += 1
            return v
        if p.returncode:
            (root / 'failed-request.json').write_text(json.dumps(request, indent=2), encoding='utf-8')
        assert p.returncode == 0 and v['ok'], v
        return v['result']

    def apply(p, operations):
        return call('timeline.apply', project=p, expected_revision=p['revision'], operations=operations)

    pictures, sounds, assets = [], [], []
    for s in range(2):
        rgb = [bytes(v for y in range(H) for x in range(W) for v in ((x*5+n*11+s*73)%256, (y*13+n*7+s*31)%256, (x*3+y*17+n*19+s*101)%256)) for n in range(N)]
        pcm = array('h', (v for n in range(N*1920) for v in ((n*71+s*5303)%50001-25000, (n*37+s*7307)%46001-23000)))
        pictures.append(rgb)
        sounds.append(pcm.tobytes())
        raw, audio, movie = [sources / f'{s}.{extension}' for extension in ('rgb', 'pcm', 'mkv')]
        raw.write_bytes(b''.join(rgb))
        audio.write_bytes(pcm.tobytes())
        ff(['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{W}x{H}', '-framerate', '25', '-i', str(raw), '-f', 's16le', '-ar', '48000', '-ac', '2', '-i', str(audio), '-map', '0:v', '-map', '1:a', '-c:v', 'ffv1', '-pix_fmt', 'bgr0', '-threads', '1', '-c:a', 'pcm_s16le', '-colorspace', 'rgb', '-color_range', 'pc', '-color_primaries', 'bt709', '-color_trc', 'iec61966-2-1', str(movie)])
        assets.append({'id': str(s), 'path': str(movie), 'duration': time(N, 25), 'identity': {'sha256': digest(movie), 'bytes': movie.stat().st_size}})
    original = {p.name: digest(p) for p in sources.iterdir()}
    base = call('project.create', id='cache-fixture', width=W, height=H, frame_rate=time(25))
    base = apply(base, [{'op': 'media.add', 'asset': a} for a in assets] + [
        {'op': 'clip.append', 'clip': {'id': 'first', 'asset_id': '0', 'source_in': time(3, 25), 'duration': time(10, 25)}},
        {'op': 'clip.append', 'clip': {'id': 'gap', 'gap': True, 'source_in': time(0), 'duration': time(2, 25)}},
        {'op': 'clip.append', 'clip': {'id': 'last', 'asset_id': '1', 'source_in': time(5, 25), 'duration': time(12, 25)}}])
    expected_rgb = pictures[0][3:13] + [bytes(W*H*3)]*2 + pictures[1][5:17]
    expected_pcm = sounds[0][3*7680:13*7680] + bytes(2*7680) + sounds[1][5*7680:17*7680]

    def task(kind, project=base, label='preview', **kw):
        extension = 'png' if kind in ('frame', 'sheet') else 'mkv'
        return {'type': kind, 'project': project, 'input_root': str(root), 'output_root': str(out), 'output': str(out / f'{label}.{extension}'), **kw}

    def cached(t, *, hit=None, selected_cache=cache, policy=POLICY, error=None, env=None):
        start = clock.perf_counter()
        result = call('cache.run', cache_root=str(selected_cache), policy=policy, task=t, error=error, env=env)
        if error:
            return result
        elapsed = clock.perf_counter()-start
        if hit is not None:
            assert result['cache']['hit'] is hit, (t['type'], hit, result)
        timings.append({'kind': t['type'], 'hit': result['cache']['hit'], 'wall_seconds': elapsed, 'engine_micros': result['cache']['elapsed_micros']})
        return result

    def pair(t, label):
        a = cached(t, hit=False)
        warm = copy.deepcopy(t)
        if 'output' in t:
            warm['output'] = str(Path(t['output']).with_stem(label+'-warm'))
        b = cached(warm, hit=True)
        assert a['cache']['key'] == b['cache']['key']
        if 'output' in t:
            assert Path(t['output']).read_bytes() == Path(warm['output']).read_bytes()
            assert a['result']['identity'] == b['result']['identity']
        return a, b

    def compare_movie(path, wanted_frames, wanted_pcm, label, scale=1):
        nonlocal frames, samples
        actual = ff(['-i', str(path), '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
        assert actual == b''.join(wanted_frames), (label, 'pixels')
        actual = ff(['-i', str(path), '-map', '0:a:0', '-f', 's16le', '-'])
        assert actual == wanted_pcm, (label, 'samples')
        frames += len(wanted_frames)
        samples += len(wanted_pcm)//4
        cases.append(label)

    def compare_frame(path, pixels, width=W, height=H):
        nonlocal frames
        with Image.open(path) as im:
            assert im.size == (width, height) and im.convert('RGB').tobytes() == pixels
        frames += 1

    probe = {'type': 'probe', 'path': assets[0]['path'], 'input_root': str(root)}
    a, b = pair(probe, 'probe')
    assert a['result'] == b['result'] and a['result']['identity'] == assets[0]['identity']
    direct = json.loads(subprocess.run(['ffprobe', '-v', 'error', '-show_format', '-show_streams', '-of', 'json', assets[0]['path']], capture_output=True, check=True).stdout)
    assert os.path.samefile(a['result']['metadata']['format']['filename'], direct['format']['filename'])
    direct['format']['filename'] = a['result']['metadata']['format']['filename']
    assert a['result']['metadata'] == direct
    for n in (0, 10, 11, 12, 23):
        a, b = pair(task('frame', label=f'frame-{n}', time=time(n, 25)), f'frame-{n}')
        for result in (a, b):
            compare_frame(result['result']['output'], expected_rgb[n])
    interval, interval_hit = pair(task('interval', label='interval', start=time(1, 25), duration=time(22, 25)), 'interval')
    for result in (interval, interval_hit):
        compare_movie(result['result']['output'], expected_rgb[1:23], expected_pcm[7680:23*7680], 'cached-interval')
    final = out / 'final.mkv'
    call('render.run', project=base, input_root=str(root), output_root=str(out), output=str(final))
    compare_movie(final, expected_rgb, expected_pcm, 'final-quality')

    def downsample(frame, scale):
        return bytes(frame[(y*scale*W+x*scale)*3+c] for y in range(H//scale) for x in range(W//scale) for c in range(3))

    proxy_ops = []
    for i in range(2):
        a, b = pair(task('proxy', label=f'proxy-{i}', expected_revision=base['revision'], asset_id=str(i), scale=2), f'proxy-{i}')
        assert b['result']['proxy']['path'] == b['result']['output']
        proxy_ops.extend(b['result']['operations'])
        for result in (a, b):
            compare_movie(result['result']['output'], [downsample(f, 2) for f in pictures[i]], sounds[i], 'proxy-cold-warm', 2)
    selected = apply(base, proxy_ops + [{'op': 'preview.proxy', 'scale': 2}])
    a, b = pair(task('interval', selected, 'proxy-interval', start=time(1, 25), duration=time(22, 25)), 'proxy-interval')
    assert a['cache']['key'] != interval['cache']['key']
    for result in (a, b):
        compare_movie(result['result']['output'], [downsample(f, 2) for f in expected_rgb[1:23]], expected_pcm[7680:23*7680], 'proxy-preview', 2)
    proxy_final = out / 'selected-final.mkv'
    call('render.run', project=selected, input_root=str(root), output_root=str(out), output=str(proxy_final))
    compare_movie(proxy_final, expected_rgb, expected_pcm, 'proxy-selected-final-full-quality')
    passed.append('cache.probes_proxies_frames_intervals_content_identity')

    def sheet_pixels(spec, expected, scale=1):
        # Build a separately allocated image with explicit letterboxes and unused cells.
        rows = (len(spec['times'])+spec['columns']-1)//spec['columns']
        width = spec['columns']*spec['tile_width']+(spec['columns']-1)*spec['gap']
        height = rows*spec['tile_height']+(rows-1)*spec['gap']
        canvas = Image.new('RGB', (width, height), tuple(spec['background']))
        sw, sh = W//scale, H//scale
        for index, t in enumerate(spec['times']):
            n = t['num']*25//t['den']
            ratio = min(spec['tile_width']/sw, spec['tile_height']/sh)
            dw, dh = max(1, int(sw*ratio)), max(1, int(sh*ratio))
            pixels = downsample(expected[n], scale) if scale != 1 else expected[n]
            # Independent coordinate table evaluated before copying pixels.
            xs = [min(sw-1, int((x+.5)*sw/dw)) for x in range(dw)]
            ys = [min(sh-1, int((y+.5)*sh/dh)) for y in range(dh)]
            scaled = bytes(pixels[(sy*sw+sx)*3+c] for sy in ys for sx in xs for c in range(3))
            cell = Image.frombytes('RGB', (dw, dh), scaled)
            x = (index % spec['columns'])*(spec['tile_width']+spec['gap'])+(spec['tile_width']-dw)//2
            y = (index // spec['columns'])*(spec['tile_height']+spec['gap'])+(spec['tile_height']-dh)//2
            canvas.paste(cell, (x, y))
        return canvas

    spec = {'times': [time(n, 25) for n in (23, 0, 10, 12, 11)], 'columns': 3, 'tile_width': 17, 'tile_height': 13, 'gap': 3, 'background': [29, 43, 61]}
    for project, scale, label in ((base, 1, 'sheet'), (selected, 2, 'sheet-proxy')):
        a, b = pair(task('sheet', project, label, spec=spec), label)
        expected = sheet_pixels(spec, expected_rgb, scale)
        for result in (a, b):
            compare_frame(result['result']['output'], expected.tobytes(), *expected.size)
            assert result['result']['details']['cells'][-1]['timeline_frame'] == 11
        direct_path = out / f'{label}-direct.png'
        call('preview.sheet', project=project, spec=spec, input_root=str(root), output_root=str(out), output=str(direct_path))
        compare_frame(direct_path, expected.tobytes(), *expected.size)
    # Reusable child with a dissolve; parent samples from inside it and includes gaps.
    child = arrangement(16, [populated('v', 'video', [placement('v0', 0, 0, 8, 3), placement('v1', 1, 8, 8, 5)]), populated('a', 'audio', [placement('a0', 0, 0, 8, 3), placement('a1', 1, 8, 8, 5)])])
    for t in child['tracks']:
        t['transitions'] = [effect(t['id']+'fx', t['id']+'0', t['id']+'1')]
    nested_project = copy.deepcopy(base)
    nested_project['clips'] = []
    nested_project['sequences'] = [{'id': 'shot', 'arrangement': child}]
    nested_project['tracks'] = arrangement(18, [populated('root-v', 'video', [nested('nv', 'shot', 2, 12, 2)]), populated('root-a', 'audio', [nested('na', 'shot', 2, 12, 2)])])
    call('project.validate', project=nested_project)
    child_rgb = pictures[0][3:11] + pictures[1][5:13]
    child_pcm = bytearray(sounds[0][3*7680:11*7680] + sounds[1][5*7680:13*7680])
    for n in range(6, 10):
        weight = 2*(n-6)+1
        child_rgb[n] = bytes((a*(8-weight)+b*weight+4)//8 for a, b in zip(pictures[0][3+n], pictures[1][n-3]))
    for n in range(6*1920, 10*1920):
        weight, denominator = 2*(n-6*1920)+1, 8*1920
        aa = array('h', sounds[0][(3*1920+n)*4:(3*1920+n+1)*4])
        bb = array('h', sounds[1][(n-3*1920)*4:(n-3*1920+1)*4])
        raw = [a*(denominator-weight)+b*weight for a, b in zip(aa, bb)]
        child_pcm[n*4:(n+1)*4] = array('h', ((1 if v >= 0 else -1)*((abs(v)+denominator//2)//denominator) for v in raw)).tobytes()
    nested_rgb = [bytes(W*H*3)]*2 + child_rgb[2:14] + [bytes(W*H*3)]*4
    nested_pcm = bytes(2*7680) + bytes(child_pcm[2*7680:14*7680]) + bytes(4*7680)
    a, b = pair(task('interval', nested_project, 'nested', start=time(0), duration=time(18, 25)), 'nested')
    for result in (a, b):
        compare_movie(result['result']['output'], nested_rgb, nested_pcm, 'nested-transition-interval')
    nested_spec = {**spec, 'times': [time(n, 25) for n in (0, 6, 7, 8, 9, 13, 17)]}
    a, b = pair(task('sheet', nested_project, 'nested-sheet', spec=nested_spec), 'nested-sheet')
    expected = sheet_pixels(nested_spec, nested_rgb)
    for result in (a, b):
        compare_frame(result['result']['output'], expected.tobytes(), *expected.size)
    passed.append('preview.contact_sheets_cached_intervals_final_agreement')

    moved = root / 'relocated'
    moved.mkdir()
    moved_project = copy.deepcopy(base)
    for asset in moved_project['assets']:
        path = moved / (asset['id']+'.mkv')
        shutil.copyfile(asset['path'], path)
        asset['path'] = str(path)
    relocated = cached({**probe, 'path': str(moved / '0.mkv')}, hit=True)
    assert os.path.samefile(relocated['result']['metadata']['format']['filename'], moved / '0.mkv')
    relocated = cached(task('frame', moved_project, 'relocated-frame', time=time(0)), hit=True)
    compare_frame(relocated['result']['output'], expected_rgb[0])
    changed = apply(base, [{'op': 'clip.slip', 'clip_id': 'first', 'source_in': time(4, 25)}])
    changed_result = cached(task('frame', changed, 'changed-source-window', time=time(0)), hit=False)
    compare_frame(changed_result['result']['output'], pictures[0][4])
    revised = copy.deepcopy(base)
    revised['revision'] += 100
    revised_result = cached(task('frame', revised, 'changed-revision', time=time(0)), hit=False)
    compare_frame(revised_result['result']['output'], expected_rgb[0])
    child_changed = copy.deepcopy(nested_project)
    child_changed['sequences'][0]['arrangement']['tracks'][0]['transitions'][0]['kind'] = 'dip_black'
    a = cached(task('frame', nested_project, 'original-nested-effect', time=time(7, 25)), hit=False)
    b = cached(task('frame', child_changed, 'changed-nested-effect', time=time(7, 25)), hit=False)
    assert a['cache']['key'] != b['cache']['key'] and a['result']['identity'] != b['result']['identity']
    changed_bound = copy.deepcopy(moved_project)
    changed_bound['assets'][0]['identity']['sha256'] = '0'*64
    cached(task('frame', changed_bound, 'changed-bound', time=time(0)), error='MEDIA_CHANGED')
    assert not (out / 'changed-bound.png').exists()
    # Equal size and timestamp are deliberately insufficient cache identities.
    import wave
    wav = moved / 'mutable.wav'
    with wave.open(str(wav), 'wb') as f:
        f.setnchannels(2)
        f.setsampwidth(2)
        f.setframerate(48000)
        f.writeframes(bytes(19200))
    before = wav.stat()
    before_probe = cached({'type': 'probe', 'path': str(wav), 'input_root': str(root)}, hit=False)
    data = bytearray(wav.read_bytes())
    data[-20] = 37
    wav.write_bytes(data)
    os.utime(wav, ns=(before.st_atime_ns, before.st_mtime_ns))
    after_probe = cached({'type': 'probe', 'path': str(wav), 'input_root': str(root)}, hit=False)
    assert wav.stat().st_size == before.st_size and wav.stat().st_mtime_ns == before.st_mtime_ns
    assert before_probe['cache']['key'] != after_probe['cache']['key']
    # Identical executable bytes at a new location reuse; changed bytes invalidate,
    # even when the external executable reports the same version and media facts.
    tool_copy = moved / 'ffprobe.exe'
    shutil.copyfile(shutil.which('ffprobe'), tool_copy)
    env = {**os.environ, 'CUTBOLT_FFPROBE': str(tool_copy)}
    tool_same = cached(probe, hit=True, env=env)
    with tool_copy.open('ab') as f:
        f.write(b'original cache identity fixture trailer\n')
    tool_changed = cached(probe, hit=False, env=env)
    assert tool_same['cache']['key'] != tool_changed['cache']['key']
    assert tool_same['result'] == tool_changed['result']
    # A corrupt owned entry is discarded, rebuilt and checked before publication.
    frame_task = task('frame', label='before-corruption', time=time(0))
    known = cached(frame_task, hit=True)
    for column, value in (('data', b'wrong'), ('metadata', '{}')):
        with sqlite3.connect(cache / 'cutbolt-cache.sqlite') as db:
            db.execute(f'UPDATE entries SET {column}=? WHERE key=?', (value, known['cache']['key']))
        rebuilt = cached(task('frame', label='rebuilt-'+column, time=time(0)), hit=False)
        assert rebuilt['cache']['key'] == known['cache']['key']
        compare_frame(rebuilt['result']['output'], expected_rgb[0])
    passed.append('cache.invalidation_relocation_tools_and_corruption')

    call('session.create', store_root=str(store), project=base, request_id='create')
    saved_base = call('session.get', store_root=str(store), project_id=base['id'])
    changes = [{'op': 'clip.slip', 'clip_id': 'first', 'source_in': time(4, 25)}]
    fields = {'store_root': str(store), 'project_id': base['id'], 'expected_revision': saved_base['revision'], 'request_id': 'slip', 'operations': changes}
    call('session.preview', **{k: v for k, v in fields.items() if k != 'request_id'})
    receipt = call('session.apply', **fields)
    assert call('session.apply', **fields) == receipt
    saved = call('session.get', store_root=str(store), project_id=base['id'])
    a = cached(task('frame', saved, 'saved-changed', time=time(0)), hit=False)
    b = cached(task('frame', saved, 'saved-changed-retry', time=time(0)), hit=True)
    assert a['cache']['key'] == b['cache']['key']
    compare_frame(a['result']['output'], pictures[0][4])
    call('session.undo', store_root=str(store), project_id=base['id'], expected_revision=saved['revision'], request_id='undo')
    undone = call('session.get', store_root=str(store), project_id=base['id'])
    a = cached(task('frame', undone, 'saved-undone', time=time(0)), hit=False)
    compare_frame(a['result']['output'], expected_rgb[0])
    call('session.restore', store_root=str(store), project_id=base['id'], expected_revision=undone['revision'], target_revision=saved['revision'], request_id='restore')
    restored = call('session.get', store_root=str(store), project_id=base['id'])
    a = cached(task('frame', restored, 'saved-restored', time=time(0)), hit=False)
    compare_frame(a['result']['output'], pictures[0][4])
    assert restored['revision'] > undone['revision'] > saved['revision']
    passed.append('cache.saved_edits_retry_undo_and_restore')

    budget = root / 'budget-cache'
    budget.mkdir()
    small = {**POLICY, 'max_entries': 2}
    def budget_frame(n, label, hit):
        return cached(task('frame', label=label, time=time(n, 25)), selected_cache=budget, policy=small, hit=hit)
    first = budget_frame(0, 'budget-first', False)
    second = budget_frame(1, 'budget-second', False)
    budget_frame(0, 'budget-touch-first', True)
    third = budget_frame(2, 'budget-third', False)
    assert third['cache']['store']['evicted'] == [second['cache']['key']]
    status = call('cache.inspect', cache_root=str(budget))
    assert {e['key'] for e in status['entries']} == {first['cache']['key'], third['cache']['key']}
    one_bytes = max(e['bytes'] for e in status['entries'])
    pruned = call('cache.prune', cache_root=str(budget), policy={'max_entries': 2, 'max_bytes': one_bytes})
    assert pruned['evicted'] == [first['cache']['key']]
    assert pruned['cache']['payload_bytes'] <= one_bytes and len(pruned['cache']['entries']) == 1
    assert pruned['cache']['database_bytes'] >= pruned['cache']['payload_bytes']
    pruned = call('cache.prune', cache_root=str(budget), policy={'max_entries': 0, 'max_bytes': 0})
    assert pruned['cache']['payload_bytes'] == 0 and not pruned['cache']['entries']
    for n, policy in enumerate(({'max_entries': 2, 'max_bytes': 1}, {'max_entries': 0, 'max_bytes': 1048576}, {'max_entries': 2, 'max_bytes': 0})):
        result = cached(task('frame', label=f'budget-bypass-{n}', time=time(0)), selected_cache=budget, policy=policy, hit=False)
        assert result['cache']['store']['stored'] is False and result['cache']['store']['reason'] == 'budget'
        compare_frame(result['result']['output'], expected_rgb[0])
    assert not call('cache.inspect', cache_root=str(budget))['entries']
    call('cache.prune', cache_root=str(budget), policy={'max_entries': 4096, 'max_bytes': 1073741824})
    for policy in ({'max_entries': 4097, 'max_bytes': 1000}, {'max_entries': 1, 'max_bytes': 1073741825}):
        cached(probe, selected_cache=budget, policy=policy, error='INVALID_CACHE')
    passed.append('cache.lru_entry_byte_eviction_and_budget_bypass')

    concurrent = root / 'concurrent-cache'
    concurrent.mkdir()
    def concurrent_frame(n):
        return cached(task('frame', label=f'concurrent-{n}', time=time(3, 25)), selected_cache=concurrent)
    with ThreadPoolExecutor(max_workers=4) as pool:
        together = list(pool.map(concurrent_frame, range(4)))
    assert len({v['cache']['key'] for v in together}) == 1
    assert len(call('cache.inspect', cache_root=str(concurrent))['entries']) == 1
    for result in together:
        compare_frame(result['result']['output'], expected_rgb[3])
    assert not list(concurrent.glob('.cutbolt-cache-*'))
    # Concurrent callers contest one destination; exactly one publishes it.
    collision = task('frame', label='contested-output', time=time(4, 25))
    def contest(_):
        p = subprocess.run([str(exe)], input=json.dumps({'command': 'cache.run', 'cache_root': str(concurrent), 'policy': POLICY, 'task': collision}).encode(), capture_output=True, timeout=180)
        return p.returncode, json.loads(p.stdout)
    with ThreadPoolExecutor(max_workers=2) as pool:
        contenders = list(pool.map(contest, range(2)))
    assert sum(v[1]['ok'] for v in contenders) == 1, contenders
    failures = [v[1]['error']['code'] for v in contenders if not v[1]['ok']]
    # Preflight collision or the shared publisher's atomic late-collision error.
    assert len(failures) == 1 and failures[0] in {'OUTPUT_EXISTS', 'PUBLISH_FAILED'}, contenders
    rejected += 1
    compare_frame(collision['output'], expected_rgb[4])
    assert not list(concurrent.glob('.cutbolt-cache-*')) and not list(out.glob('.cutbolt-cache-*'))
    passed.append('cache.concurrent_builds_and_no_overwrite_publication')

    missing = root / 'missing-cache'
    missing.mkdir()
    call('cache.inspect', cache_root=str(missing), error='CACHE_MISSING')
    foreign = root / 'foreign-cache'
    foreign.mkdir()
    with sqlite3.connect(foreign / 'cutbolt-cache.sqlite') as db:
        db.execute('CREATE TABLE original_sentinel(value TEXT)')
        db.execute("INSERT INTO original_sentinel VALUES('preserve')")
    sentinel = digest(foreign / 'cutbolt-cache.sqlite')
    cached(probe, selected_cache=foreign, error='INVALID_CACHE')
    assert digest(foreign / 'cutbolt-cache.sqlite') == sentinel
    before_db = digest(cache / 'cutbolt-cache.sqlite')
    cached({'type': 'probe', 'path': str(cache / 'cutbolt-cache.sqlite'), 'input_root': str(root)}, error='INVALID_CACHE')
    assert digest(cache / 'cutbolt-cache.sqlite') == before_db
    known_output = digest(Path(frame_task['output']))
    cached(frame_task, error='OUTPUT_EXISTS')
    assert digest(Path(frame_task['output'])) == known_output
    cached({**task('frame', label='bad-output', time=time(0)), 'output': str(root / 'outside.png')}, error='PATH_OUTSIDE_ROOT')
    cached({**probe, 'input_root': str(out)}, error='PATH_OUTSIDE_ROOT')
    cached(task('frame', label='end-frame', time=time(24, 25)), error='INVALID_RANGE')
    cached(task('frame', label='half-frame', time=time(1, 50)), error='UNALIGNED_TIME')
    cached(task('interval', label='zero-range', start=time(0), duration=time(0)), error='INVALID_RANGE')
    cached(task('interval', label='past-end', start=time(23, 25), duration=time(2, 25)), error='INVALID_RANGE')
    for index, mutation in enumerate(({'times': []}, {'times': [time(0)]*65}, {'columns': 0}, {'columns': 9}, {'tile_width': 0}, {'tile_width': 1921}, {'tile_height': 1081}, {'gap': 33}, {'times': [time(24, 25)]}, {'times': [time(1, 50)]}, {'times': [time(0)]*8, 'tile_width': 1920, 'tile_height': 1080, 'columns': 8})):
        bad_spec = {**spec, **mutation}
        code = 'INVALID_RANGE' if mutation == {'times': [time(24, 25)]} else 'UNALIGNED_TIME' if mutation == {'times': [time(1, 50)]} else 'LIMIT_EXCEEDED' if mutation.get('columns') == 8 else 'INVALID_SHEET'
        cached(task('sheet', label=f'bad-sheet-{index}', spec=bad_spec), error=code)
    # Exercise actual maximum item count and exact 8M canvas, not just validation.
    for label, maximum in (
        ('max-items', {'times': [time(n%24, 25) for n in range(64)], 'columns': 8, 'tile_width': 1, 'tile_height': 1, 'gap': 32, 'background': [7, 11, 19]}),
        ('max-canvas', {'times': [time(n, 25) for n in range(8)], 'columns': 4, 'tile_width': 1000, 'tile_height': 1000, 'gap': 0, 'background': [7, 11, 19]})):
        a, b = pair(task('sheet', label=label, spec=maximum), label)
        expected = sheet_pixels(maximum, expected_rgb)
        for result in (a, b):
            compare_frame(result['result']['output'], expected.tobytes(), *expected.size)
    passed.append('cache.preview_bounds_roots_and_source_preservation')

    client = Client(exe)
    try:
        client.initialize()
        catalog = client.rpc('tools/list')['result']['tools']
        names = {t['name'] for t in catalog}
        assert len(catalog) == len(names) == 63
        assert {'cutbolt_cache_inspect', 'cutbolt_cache_prune'} <= names
        assert 'cutbolt_cache_run' not in names and 'cutbolt_preview_sheet' not in names
        for name, fields in (('cache.inspect', {'cache_root': str(budget)}), ('cache.prune', {'cache_root': str(budget), 'policy': {'max_bytes': 0, 'max_entries': 0}})):
            schema = next(t['inputSchema'] for t in catalog if t['name'] == 'cutbolt_'+name.replace('.', '_'))
            Draft202012Validator(schema).validate(fields)
            result = client.call(name, **fields)
            assert not (result['entries'] if name.endswith('inspect') else result['cache']['entries'])
    finally:
        client.close()
    assert original == {p.name: digest(p) for p in sources.iterdir()}
    passed.append('cache.typed_stdio_inspection_and_pruning')

    # Three separate empty cache roots per kind provide repeated real misses;
    # warm requests include source/tool hashing, payload validation and publishing.
    latency = {}
    benchmark_tasks = {'probe': probe, 'frame': task('frame', time=time(5, 25)), 'proxy': task('proxy', expected_revision=base['revision'], asset_id='0', scale=2), 'interval': task('interval', start=time(0), duration=time(24, 25)), 'sheet': task('sheet', spec=spec)}
    for kind, original_task in benchmark_tasks.items():
        cold, warm = [], []
        for repetition in range(3):
            benchmark_cache = root / f'latency-{kind}-{repetition}'
            benchmark_cache.mkdir()
            for hit, measured in ((False, cold), (True, warm)):
                t = copy.deepcopy(original_task)
                if 'output' in t:
                    t['output'] = str(Path(t['output']).with_stem(f'latency-{kind}-{repetition}-{hit}'))
                result = cached(t, selected_cache=benchmark_cache, hit=hit)
                measured.append(timings[-1]['wall_seconds'])
        latency[kind] = {'cold_seconds': cold, 'warm_seconds': warm, 'cold_median': statistics.median(cold), 'warm_median': statistics.median(warm)}
        assert latency[kind]['warm_median'] < latency[kind]['cold_median'], (kind, latency[kind])
    passed.append('cache.measured_cold_and_warm_latency')
    result = {'passed': passed, 'frames_compared': frames, 'stereo_sample_frames_compared': samples, 'rejections': rejected, 'cases': cases, 'latency': latency, 'timings': timings,
              'limits': {'actual_contact_sheet_items': 64, 'actual_contact_sheet_pixels': 8000000, 'payload_budget_max': 1073741824, 'entry_budget_max': 4096},
              'reference': 'Original coded RGB/PCM sources, independent literal timeline/nested-transition composition and contact-sheet sampling. Complete decoded pixels/audio checked, cold/warm identical bytes, actual SHA invalidation and real concurrent native callers. Budget maxima are validated policy bounds; fixture does not claim a 1 GiB cache load test.'}
    (root / 'verification.json').write_text(json.dumps(result, indent=2)+'\n', encoding='utf-8')
    print(json.dumps(result, indent=2))
    return result


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', required=True, type=Path)
    run(parser.parse_args().output)
