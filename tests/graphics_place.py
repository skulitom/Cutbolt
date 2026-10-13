"""One-job graphic placement: decoded reference equality, saved retries, races and cleanup."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import time

from agents import Client
from engine import ENGINE
from graphics import make_font
from scenes import identity

ROOT = Path(__file__).resolve().parents[1]
DENY_CODECS = r'''
use std::{env,fs,process::exit};
fn main() {
 fs::write(env::var_os("GRAPHIC_CODEC_MARKER").unwrap(),b"unexpected codec launch").unwrap();
 exit(9);
}
'''


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(root, legacy_engine=None):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources = root / 'sources'
    sources.mkdir()
    font = sources / 'original.ttf'
    make_font(font, {chr(c): (600, [(0, 0, 250 + c % 3 * 100, 600)]) for c in range(33, 127)} | {' ': (250, [])})
    fonts = [{'path': str(font.relative_to(root)), 'sha256': digest(font), 'bytes': font.stat().st_size}]
    width, height, frames, rate = 128, 72, 60, 30
    raw = sources / 'original.rgb'
    raw.write_bytes(bytes(channel for n in range(frames) for y in range(height) for x in range(width)
                         for channel in ((n * 7 + x) % 256, (y * 3 + n) % 256, (x + y + 91) % 256)))
    original = sources / 'original.mkv'
    subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', '-f', 'rawvideo', '-pixel_format', 'rgb24',
                    '-video_size', f'{width}x{height}', '-framerate', str(rate), '-i', str(raw),
                    '-f', 'lavfi', '-i', 'sine=frequency=431:sample_rate=48000:duration=2',
                    '-map', '0:v', '-map', '1:a', '-ac', '2', '-c:v', 'ffv1', '-pix_fmt', 'bgr0',
                    '-c:a', 'pcm_s16le', '-colorspace', 'rgb', '-color_range', 'pc', '-color_primaries', 'bt709',
                    '-color_trc', 'iec61966-2-1', str(original)], check=True, capture_output=True)
    source_hashes = {p.name: digest(p) for p in sources.iterdir()}
    passed = []

    def call(request, error=None, env=None, engine=ENGINE):
        process = subprocess.run([str(engine)], input=json.dumps(request).encode(), capture_output=True, env=env, timeout=180)
        result = json.loads(process.stdout)
        assert process.returncode == (1 if error else 0), (result, process.stderr)
        if error:
            assert result['error']['code'] == error, result
            return result['error']
        assert result['ok'], result
        return result['result']

    base = call({'command': 'project.create', 'id': 'graphic-fixture', 'width': width, 'height': height, 'frame_rate': rate})
    base = call({'command': 'timeline.apply', 'project': base, 'expected_revision': 0, 'operations': [
        {'op': 'media.add', 'asset': {'id': 'original', 'path': str(original), 'duration': 2,
         'identity': {'sha256': digest(original), 'bytes': original.stat().st_size}}},
        {'op': 'clip.append', 'clip': {'id': 'original', 'asset_id': 'original', 'source_in': 0, 'duration': 2}}]})

    def fixture(name):
        workspace = root / name
        workspace.mkdir()
        store, output = workspace / 'store', workspace / 'assets'
        store.mkdir(); output.mkdir()
        call({'command': 'session.create', 'store_root': str(store), 'project': base, 'request_id': 'create'})
        request = {'command': 'graphics.place', 'store_root': str(store), 'project_id': base['id'],
                   'request_id': 'place', 'expected_revision': 0, 'instance_id': 'title',
                   'template_id': 'cutbolt-lower-third-v1', 'start': 0.2,
                   'values': {'fonts': {'type': 'fonts', 'value': fonts}, 'label': {'type': 'text', 'value': 'ABC'},
                              'duration': {'type': 'time', 'value': 0.2}, 'transparent': {'type': 'boolean', 'value': True}},
                   'input_root': str(root), 'output_root': str(output)}
        return workspace, store, output, request

    workspace, store, output, request = fixture('queued')
    client = Client(ENGINE, args=('--workspace', str(root), 'mcp'))
    try:
        client.initialize()
        arguments = {k: v for k, v in request.items() if k != 'command'}
        arguments['store_root'] = str(store.relative_to(root))
        arguments['output_root'] = str(output.relative_to(root))
        arguments.pop('input_root')
        arguments['values']['fonts']['value'] = [{'path': str(font.relative_to(root))}]
        # Record exactly the two transmitted requests, including JSON-RPC envelopes.
        messages = []
        write = client.write
        def measured(data):
            messages.append(data)
            write(data)
        client.write = measured
        ticket = client.call('job.start', request_id='graphic-job', run='graphics.place', arguments=arguments)
        waited = client.call('job.wait', job_id=ticket['job_id'], timeout_seconds=120)
        assert waited['status'] == 'completed', waited
        result = waited['result']
        assert len(messages) == 2 and sum(map(len, messages)) <= 1500, [len(m) for m in messages]
        title_cost = {'calls': len(messages), 'sent_bytes': sum(map(len, messages)), 'schema_lookups': 0}
    finally:
        client.close()
    # The queued request was rooted/identity-completed; retry its resolved CLI equivalent.
    request['values']['fonts']['value'] = fonts
    saved = call({'command': 'session.get', 'store_root': str(store), 'project_id': base['id']})
    assert saved['revision'] == 1 and saved['schema_version'] == 2 and len(saved['assets']) == 1 and saved['clips'] == []
    generated = next(t for t in saved['tracks']['tracks'] if t['id']=='graphics')['clips'][0]
    assert generated['scene']['id']=='title' and not generated.get('asset_id')
    assert result['compiled'] is False and result['scene_id']=='title' and 'asset' not in result
    assert list(output.iterdir()) == []
    assert call({'command': 'session.check', 'store_root': str(store)})

    project = copy.deepcopy(base)
    project['revision'] = 0
    expanded = call({'command': 'graphics.instantiate', 'template_id': request['template_id'], 'instance_id': 'title',
                     'values': request['values'], 'input_root': str(root), 'project': project})
    reference_file = workspace / 'reference.mkv'
    compiled = call({'command': 'scene.render', 'scene': expanded['scene'], 'input_root': str(root),
                     'output_root': str(workspace), 'output': str(reference_file)})
    reference_asset = compiled['asset']
    operations = [{'op': 'media.add', 'asset': reference_asset},
                  {'op': 'tracks.edit', 'edit': {'op': 'promote', 'video_track_id': 'video', 'audio_track_id': 'audio'}},
                  {'op': 'tracks.edit', 'edit': {'op': 'add', 'track': {'id': 'titles', 'kind': 'video', 'locked': False,
                   'enabled': True, 'clips': [], 'composite': 'alpha_over'}}},
                  {'op': 'tracks.edit', 'edit': {'op': 'place', 'track_id': 'titles', 'collision': 'reject',
                   'clip': {'id': 'title', 'asset_id': 'title', 'start': 0.2, 'source_in': 0, 'duration': 0.2}}}]
    reference = call({'command': 'timeline.apply', 'project': project, 'expected_revision': 0, 'operations': operations})
    decoded = []
    for label, timeline in [('shortcut', saved), ('explicit', reference)]:
        movie = workspace / (label + '.mkv')
        call({'command': 'render.run', 'project': timeline, 'input_root': str(root), 'output_root': str(workspace), 'output': str(movie)})
        rgb = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(movie), '-map', '0:v', '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-'])
        pcm = subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(movie), '-map', '0:a', '-f', 's16le', '-'])
        assert len(rgb) == width * height * 3 * frames and len(pcm) == 48000 * 2 * 2 * 2
        decoded.append((rgb, pcm))
    assert decoded[0] == decoded[1]
    passed.append('graphics_place.two_call_queued_placement_matches_explicit_picture_and_sound')

    empty_root = root / 'empty-inline'; empty_root.mkdir()
    empty_store = empty_root / 'store'; empty_store.mkdir()
    empty_output = empty_root / 'assets'; empty_output.mkdir()
    native_rate = {'num': 30000, 'den': 1001}
    native_duration = {'num': 1001, 'den': 3000}  # Ten frames and exactly 16016 stereo samples.
    empty = call({'command': 'project.create', 'id': 'empty', 'width': width, 'height': height, 'frame_rate': native_rate})
    call({'command': 'session.create', 'store_root': str(empty_store), 'project': empty, 'request_id': 'create'})
    inline_scene = copy.deepcopy(expanded['scene'])
    inline_scene.update(frame_rate=native_rate, duration=native_duration, transparent=False)
    for layer in inline_scene['layers']:
        layer.update(start=0, duration=native_duration)
        layer.pop('animation', None)
    inline = {**request, 'store_root': str(empty_store), 'project_id': 'empty', 'start': 0,
              'template_id': None, 'template': {'schema_version': 1, 'id': 'inline', 'scene': inline_scene, 'parameters': []},
              'values': {}, 'output_root': str(empty_output)}
    placed = call(inline)
    native = call({'command': 'session.get', 'store_root': str(empty_store), 'project_id': 'empty'})
    assert native['tracks']['duration'] == native_duration and native['revision'] == 1
    rendered = empty_root / 'timeline.mkv'
    call({'command': 'render.run', 'project': native, 'input_root': str(root),
          'output_root': str(empty_root), 'output': str(rendered)})
    native_reference = empty_root/'compiled-reference.mkv'
    call({'command':'scene.render','scene':inline_scene,'input_root':str(root),'output_root':str(empty_root),'output':str(native_reference)})
    native_rgb = [subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:v',
                  '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-']) for path in (rendered, native_reference)]
    assert native_rgb[0] == native_rgb[1] and len(native_rgb[0]) == width * height * 3 * 10
    assert subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(rendered), '-map', '0:a', '-f', 's16le', '-']) == bytes(16016 * 4)
    passed.append('graphics_place.inline_template_empty_timeline_opaque_output_and_fractional_clock')

    # Direct retries after unrelated revisions and undo return the original committed receipt.
    workspace, store, output, request = fixture('retry')
    first = call(request)
    assert call(request) == first
    call({**request, 'start': 0.3}, 'REQUEST_ID_CONFLICT')
    call({'command': 'session.undo', 'store_root': str(store), 'project_id': base['id'], 'request_id': 'undo', 'expected_revision': 1})
    undone = call({'command': 'session.get', 'store_root': str(store), 'project_id': base['id']})
    expected = copy.deepcopy(base); expected['revision'] = 2
    assert undone == expected
    assert call(request) == first and list(output.iterdir()) == []
    call({'command': 'session.restore', 'store_root': str(store), 'project_id': base['id'], 'request_id': 'restore',
          'expected_revision': 2, 'target_revision': 1})
    assert call({'command': 'session.check', 'store_root': str(store)})
    passed.append('graphics_place.atomic_saved_batch_retry_undo_restore_and_history_integrity')

    workspace, store, output, request = fixture('preflight')
    for replacement, error in [({'expected_revision': 7}, 'REVISION_CONFLICT'),
                               ({'start': 2}, 'INVALID_RANGE'), ({'start': 0.01}, 'UNALIGNED_TIME'),
                               ({'output_root': str(root.parent)}, 'PATH_OUTSIDE_ROOT'),
                               ({'instance_id': 'original'}, 'DUPLICATE_ID')]:
        call({**request, **replacement}, error)
        assert list(output.iterdir()) == []
    first = call(request)
    preserved = {str(p): digest(p) for p in output.rglob('*.mkv')}
    call({**request, 'request_id': 'duplicate', 'expected_revision': 1}, 'DUPLICATE_ID')
    second = {**request, 'instance_id': 'second', 'request_id': 'second', 'expected_revision': 1}
    call(second, 'CLIP_COLLISION')
    call({'command': 'session.apply', 'store_root': str(store), 'project_id': base['id'], 'request_id': 'lock',
          'expected_revision': 1, 'operations': [{'op': 'tracks.edit', 'edit': {
              'op': 'state', 'track_id': 'graphics', 'locked': True, 'enabled': True}}]})
    call({**second, 'expected_revision': 2, 'start': 0.6}, 'TRACK_LOCKED')
    queue = workspace / 'jobs'; queue.mkdir()
    stale = {**request, 'request_id': 'stale'}
    ticket = call({'command': 'job.start', 'job_root': str(queue), 'request_id': 'stale-job',
                   'run': 'graphics.place', 'arguments': {k: v for k, v in stale.items() if k != 'command'}})
    failed = call({'command': 'job.wait', 'job_root': str(queue), 'job_id': ticket['job_id'], 'timeout_seconds': 120})
    assert failed['status'] == 'failed' and failed['error']['code'] == 'REVISION_CONFLICT', failed
    assert failed['error']['detail']['current_revision'] == 2, failed
    assert failed['error']['detail']['session_receipt']['request_id'] == 'stale', failed
    assert preserved == {str(p): digest(p) for p in output.rglob('*.mkv')}
    passed.append('graphics_place.preflight_bounds_roots_ids_collision_and_lock_preserve_state')

    helpers = root / 'helpers'; helpers.mkdir()
    source = helpers/'deny.rs'; source.write_text(DENY_CODECS)
    shim = helpers/('deny.exe' if os.name == 'nt' else 'deny')
    subprocess.run(['rustc','--edition=2024',str(source),'-o',str(shim)],check=True,capture_output=True)
    workspace, store, output, request = fixture('no-compile')
    marker = workspace/'codec-launched'
    environment = {**os.environ,'CUTBOLT_FFMPEG':str(shim),'CUTBOLT_FFPROBE':str(shim),'GRAPHIC_CODEC_MARKER':str(marker)}
    no_compile = call(request,env=environment)
    assert not marker.exists() and list(output.iterdir()) == [] and not no_compile['compiled']
    passed.append('graphics_place.no_codec_process_or_intermediate_movie')

    # A reserved write lock prevents either child from committing before both have been started.
    # Both real processes must succeed for identical intents, or exactly one wins differing intents.
    for same in [True,False]:
        workspace, store, output, request = fixture('same-request-race' if same else 'revision-race')
        requests = [request,request if same else {**request,'request_id':'other','instance_id':'other','start':0.6}]
        with sqlite3.connect(store/'projects.sqlite3') as lock:
            lock.execute('BEGIN IMMEDIATE')
            children = [subprocess.Popen([str(ENGINE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE) for _ in requests]
            for child,value in zip(children,requests):
                child.stdin.write(json.dumps(value).encode());child.stdin.close();child.stdin=None
            assert all(c.poll() is None for c in children)
            lock.rollback()
        results = [json.loads(child.communicate(timeout=120)[0]) for child in children]
        if same:
            assert results[0] == results[1] and results[0]['ok'],results
        else:
            assert sum(r['ok'] for r in results)==1 and next(r for r in results if not r['ok'])['error']['code']=='REVISION_CONFLICT',results
        head = call({'command':'session.get','store_root':str(store),'project_id':base['id']})
        assert head['revision']==1 and len(head['assets'])==1 and list(output.iterdir())==[]
        assert call({'command':'session.check','store_root':str(store)})
    passed.append('graphics_place.concurrent_replay_and_revision_race_commit_once')

    workspace, store, output, request = fixture('source-failure')
    invalid = copy.deepcopy(request); invalid['values']['fonts']['value'][0]['sha256']='0'*64
    call(invalid,'MEDIA_CHANGED')
    assert call({'command':'session.get','store_root':str(store),'project_id':base['id']})['revision']==0
    assert list(output.iterdir())==[]
    passed.append('graphics_place.failed_preparation_preserves_session_and_files')

    # Keep the project write-locked through cancellation: the worker cannot save an edit.
    workspace, store, output, request = fixture('cancel')
    job_root = workspace/'jobs';job_root.mkdir()
    with sqlite3.connect(store/'projects.sqlite3') as lock:
        lock.execute('BEGIN IMMEDIATE')
        ticket = call({'command':'job.start','job_root':str(job_root),'request_id':'cancel-graphic',
            'run':'graphics.place','arguments':{k:v for k,v in request.items() if k!='command'}})
        deadline=time.monotonic()+30
        while True:
            status=call({'command':'job.status','job_root':str(job_root),'job_id':ticket['job_id']})
            if status['status']=='running':break
            assert status['status']=='queued' and time.monotonic()<deadline,status
            time.sleep(0.01)
        call({'command':'job.cancel','job_root':str(job_root),'job_id':ticket['job_id']})
        stopped=call({'command':'job.wait','job_root':str(job_root),'job_id':ticket['job_id'],'timeout_seconds':120})
        assert stopped['status']=='cancelled',stopped
        assert stopped['error']['detail']['session_receipt']['request_id']==request['request_id'],stopped
        lock.rollback()
    assert list(output.iterdir())==[]
    assert call({'command':'session.get','store_root':str(store),'project_id':base['id']})['revision']==0
    passed.append('graphics_place.cancelled_job_cannot_commit_through_locked_store')

    if legacy_engine is not None:
        workspace, store, output, request = fixture('legacy-replay')
        old = call(request,engine=legacy_engine)
        assert Path(old['asset']['path']).is_file()
        assert call(request)==old
        call({'command':'session.undo','store_root':str(store),'project_id':base['id'],'request_id':'undo','expected_revision':1})
        assert call(request)==old
        assert call({'command':'session.check','store_root':str(store)})
        passed.append('graphics_place.actual_legacy_compiled_receipt_replay_before_and_after_undo')
    assert source_hashes == {p.name: digest(p) for p in sources.iterdir()}
    report = {'passed': passed, 'title_cost': title_cost, 'compared_frames': frames + 10,
              'compared_stereo_sample_frames': 96000 + 16016, 'sources_preserved': True, 'legacy_receipt_replay': legacy_engine is not None}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--legacy-engine', type=Path)
    args=parser.parse_args()
    run(args.output, args.legacy_engine)
