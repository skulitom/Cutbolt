"""One-job graphic placement: decoded reference equality, saved retries, races and cleanup."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from agents import Client
from engine import ENGINE
from graphics import make_font
from scenes import identity

ROOT = Path(__file__).resolve().parents[1]
HOLD = r'''
use std::{env,fs,process::{Command,exit},thread,time::{Duration,Instant}};
fn main() {
 let args:Vec<_>=env::args_os().skip(1).collect();
 if args.iter().any(|a| a=="pipe:0") && args.iter().any(|a| a=="rawvideo") {
  let root=std::path::PathBuf::from(env::var_os("GRAPHIC_HOLD_ROOT").unwrap());
  fs::write(root.join(format!("held-{}",std::process::id())),b"ready").unwrap();
  if env::var("GRAPHIC_HOLD_MODE").unwrap_or_default()=="fail" {exit(9)}
  let began=Instant::now();
  while !root.join("release").exists() {
   if began.elapsed()>Duration::from_secs(90) {exit(8)}
   thread::sleep(Duration::from_millis(10));
  }
 }
 exit(Command::new(env::var_os("GRAPHIC_REAL_FFMPEG").unwrap()).args(args).status().unwrap().code().unwrap_or(1));
}
'''


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def run(root):
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

    def call(request, error=None, env=None):
        process = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, env=env, timeout=180)
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
    assert saved['revision'] == 1 and len(saved['assets']) == 2 and saved['clips'] == []
    generated = next(a for a in saved['assets'] if a['id'] == 'title')
    assert generated['identity']['sha256'] == digest(Path(generated['path']))
    assert len(list(output.glob('.cutbolt-graphic-*'))) == 1
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
    native_rgb = [subprocess.check_output(['ffmpeg', '-v', 'error', '-i', str(path), '-map', '0:v',
                  '-f', 'rawvideo', '-pix_fmt', 'rgb24', '-']) for path in (rendered, placed['asset']['path'])]
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
    assert call(request) == first and len(list(output.iterdir())) == 1
    call({'command': 'session.restore', 'store_root': str(store), 'project_id': base['id'], 'request_id': 'restore',
          'expected_revision': 2, 'target_revision': 1})
    assert call({'command': 'session.check', 'store_root': str(store)})
    passed.append('graphics_place.atomic_saved_batch_retry_undo_restore_and_history_integrity')

    workspace, store, output, request = fixture('preflight')
    for replacement, error in [({'expected_revision': 7}, 'REVISION_CONFLICT'),
                               ({'start': 2}, 'INVALID_RANGE'), ({'start': 0.01}, 'UNALIGNED_TIME'),
                               ({'output_root': str(root.parent)}, 'PATH_OUTSIDE_ROOT')]:
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
    source = helpers / 'hold.rs'; source.write_text(HOLD)
    shim = helpers / ('hold.exe' if os.name == 'nt' else 'hold')
    subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(shim)], check=True, capture_output=True)
    for name, mode in [('revision-conflict', 'wait'), ('same-request-race', 'wait'), ('render-failure', 'fail')]:
        workspace, store, output, request = fixture(name)
        environment = {**os.environ, 'CUTBOLT_FFMPEG': str(shim), 'GRAPHIC_REAL_FFMPEG': shutil.which('ffmpeg'),
                       'GRAPHIC_HOLD_ROOT': str(workspace), 'GRAPHIC_HOLD_MODE': mode}
        count = 2 if name == 'same-request-race' else 1
        children = [subprocess.Popen([str(ENGINE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=environment)
                    for _ in range(count)]
        for child in children:
            child.stdin.write(json.dumps(request).encode()); child.stdin.close(); child.stdin = None
        deadline = time.monotonic() + 60
        while len(list(workspace.glob('held-*'))) < count:
            assert time.monotonic() < deadline and all(c.poll() is None for c in children), name
            time.sleep(0.01)
        if name == 'revision-conflict':
            call({'command': 'session.apply', 'store_root': str(store), 'project_id': base['id'], 'request_id': 'other',
                  'expected_revision': 0, 'operations': [{'op': 'project.transfer', 'transfer': 'srgb'}]})
        (workspace / 'release').write_text('continue')
        results = [json.loads(child.communicate(timeout=120)[0]) for child in children]
        head = call({'command': 'session.get', 'store_root': str(store), 'project_id': base['id']})
        if name == 'same-request-race':
            assert results[0] == results[1] and results[0]['ok'], results
            assert head['revision'] == 1 and len(list(output.iterdir())) == 1
        else:
            assert not results[0]['ok'], results
            if name == 'revision-conflict':
                assert results[0]['error']['code'] == 'REVISION_CONFLICT' and head['revision'] == 1, results
            else:
                assert head['revision'] == 0
            assert len(head['assets']) == 1 and list(output.iterdir()) == []
        assert call({'command': 'session.check', 'store_root': str(store)})
    passed.append('graphics_place.concurrent_replay_revision_conflict_and_render_failure')

    workspace, store, output, request = fixture('cancel')
    job_root = workspace / 'jobs'; job_root.mkdir()
    environment = {**os.environ, 'CUTBOLT_FFMPEG': str(shim), 'GRAPHIC_REAL_FFMPEG': shutil.which('ffmpeg'),
                   'GRAPHIC_HOLD_ROOT': str(workspace), 'GRAPHIC_HOLD_MODE': 'wait'}
    ticket = call({'command': 'job.start', 'job_root': str(job_root), 'request_id': 'cancel-graphic',
                   'run': 'graphics.place', 'arguments': {k: v for k, v in request.items() if k != 'command'}}, env=environment)
    deadline = time.monotonic() + 60
    while not list(workspace.glob('held-*')):
        assert time.monotonic() < deadline
        time.sleep(0.01)
    call({'command': 'job.cancel', 'job_root': str(job_root), 'job_id': ticket['job_id']})
    stopped = call({'command': 'job.wait', 'job_root': str(job_root), 'job_id': ticket['job_id'], 'timeout_seconds': 120})
    assert stopped['status'] == 'cancelled', stopped
    assert stopped['error']['detail']['session_receipt']['request_id'] == request['request_id'], stopped
    assert list(output.iterdir()) == []
    assert call({'command': 'session.get', 'store_root': str(store), 'project_id': base['id']})['revision'] == 0
    passed.append('graphics_place.cancelled_job_keeps_session_unchanged_and_cleans_owned_asset')
    assert source_hashes == {p.name: digest(p) for p in sources.iterdir()}
    report = {'passed': passed, 'title_cost': title_cost, 'compared_frames': frames + 10,
              'compared_stereo_sample_frames': 96000 + 16016, 'sources_preserved': True}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
