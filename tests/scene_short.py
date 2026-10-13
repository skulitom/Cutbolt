"""Shorthand versus separately authored full scenes: exact pixels, clocks, digests and queued pins."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

from PIL import Image
from engine import ENGINE
from graphics import make_font

ROOT = Path(__file__).resolve().parents[1]


def run(root):
    root = root.resolve()
    assert ROOT != root and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    inputs, output, home = [root / name for name in ('inputs', 'output', 'home')]
    for folder in (inputs, output, home):
        folder.mkdir()
    font = inputs / 'font.ttf'
    glyphs = {chr(c): (600, [(0, 0, 200 + c % 4 * 90, 600)]) for c in range(33, 127)} | {' ': (250, [])}
    make_font(font, glyphs)
    identity = {'path': 'font.ttf', 'bytes': font.stat().st_size, 'sha256': hashlib.sha256(font.read_bytes()).hexdigest()}
    env = os.environ.copy()
    env['HOME'] = str(home)
    if os.name == 'nt':
        env['WINDIR'], env['LOCALAPPDATA'] = str(home / 'Windows'), str(home / 'Local')
        (home / 'Windows/Fonts').mkdir(parents=True)
        folder = home / 'Local/Microsoft/Windows/Fonts'
    else:
        folder = home / ('Library/Fonts' if sys.platform == 'darwin' else '.local/share/fonts')
    folder.mkdir(parents=True)
    installed = folder / 'CutboltShorthandOriginal.ttf'
    make_font(installed, glyphs)

    def call(request, error=None):
        proc = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, env=env, timeout=120)
        reply = json.loads(proc.stdout)
        assert proc.returncode == (1 if error else 0), (reply, proc.stderr)
        if error:
            assert reply['error']['code'] == error, reply
            return reply['error']
        assert reply['ok'], reply
        return reply['result']

    def inspect(scene, **context):
        return call({'command': 'scene.inspect', 'scene': scene, 'input_root': str(inputs), **context})

    def decoded(path, kind):
        flags = ['-map', '0:v:0', '-pix_fmt', 'rgba', '-f', 'rawvideo'] if kind == 'video' else ['-map', '0:a:0', '-acodec', 'pcm_s16le', '-f', 's16le']
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-i', str(path), *flags, 'pipe:1'], check=True, capture_output=True).stdout

    short = {'form': 'short', 'id': 'short-title', 'size': [128, 72], 'duration': 0.4, 'background': '#246',
             'layers': [
                 {'id': 'panel', 'anchor': 'lower_third', 'content': {'type': 'rectangle', 'fill': '#1359'}},
                 {'id': 'label', 'anchor': 'center', 'content': {'type': 'text', 'text': 'ABC', 'font': {'path': 'font.ttf'}, 'size': 12},
                  'animation': {'opacity': {'from': 0, 'to': 255, 'duration': 0.2, 'ease': 'ease_in_out'}}},
                 {'id': 'dot', 'box_size': [4, 6], 'content': {'type': 'ellipse', 'fill': '#AABBCCDD'}}]}
    # Independent explicit oracle: positions and all defaults are stated, never obtained from expansion.
    def layer(name, graphic, **extra):
        return {'id': name, 'canvas': [128, 72], 'start': 0, 'duration': 0.4, 'frames': [],
                'graphics': graphic, 'timing': 'strict', 'end': 'hold_last',
                'transform': {'position': [0, 0], 'crop': [0, 0, 128, 72], 'scale': 1, 'quarter_turns': 0, 'opacity': 255}, **extra}
    explicit = {'schema_version': 1, 'id': 'short-title', 'width': 128, 'height': 72, 'output_scale': 1,
                'duration': 0.4, 'frame_rate': 25, 'background': [34, 68, 102], 'color': 'srgb_straight_encoded', 'audio': None,
                'layers': [
                    layer('panel', {'kind': 'shape', 'shape': 'rectangle', 'rect': [3, 47, 122, 22], 'fill': [17, 51, 85, 153], 'stroke': None}),
                    layer('label', {'kind': 'text', 'text': 'ABC', 'fonts': [identity], 'size': 12, 'color': [255]*4,
                                    'rect': [3, 3, 122, 66], 'line_height': 15, 'letter_spacing': 0, 'align': 'center', 'valign': 'middle',
                                    'wrap': 'character', 'overflow': 'reject'},
                          animation={'opacity': {'keys': [{'time': 0, 'value': 0, 'interpolation': 'ease_in_out'},
                                                         {'time': 0.2, 'value': 255, 'interpolation': 'hold'}]}}),
                    layer('dot', {'kind': 'shape', 'shape': 'ellipse', 'rect': [3, 3, 4, 6], 'fill': [170, 187, 204, 221], 'stroke': None})]}
    passed = []
    for transparent in (False, True):
        short['transparent'] = explicit['transparent'] = transparent
        a, b = inspect(short), inspect(explicit)
        assert {k:v for k,v in a.items() if k not in ('expanded_scene', 'resolved_identities')} == b, (a,b)
        assert inspect(a['expanded_scene']) == b
        outputs = []
        digests = []
        for name, recipe in [('short', short), ('full', explicit)]:
            path = output / f'{name}-{transparent}.mkv'
            receipt = call({'command': 'scene.render', 'scene': recipe, 'input_root': str(inputs), 'output_root': str(output), 'output': str(path)})
            digests.append(receipt['scene_sha256'])
            outputs.append(path)
            still = output / f'{name}-{transparent}.png'
            call({'command': 'scene.still', 'scene': recipe, 'time': 0.12, 'input_root': str(inputs), 'output_root': str(output), 'output': str(still)})
        assert digests[0] == digests[1]
        for kind in ('video', 'audio'):
            assert decoded(outputs[0], kind) == decoded(outputs[1], kind)
        assert Image.open(output / f'short-{transparent}.png').tobytes() == Image.open(output / f'full-{transparent}.png').tobytes()
    passed.append('scene_short.independent_full_scene_rgb_alpha_pcm_still_and_digest_equality')

    project = call({'command': 'project.create', 'id': 'short-project', 'width': 128, 'height': 72, 'frame_rate': {'num': 30000, 'den': 1001}})
    store = root / 'store'; store.mkdir()
    call({'command': 'session.create', 'store_root': str(store), 'project': project, 'request_id': 'create'})
    saved = {'store_root': str(store), 'project_id': project['id'], 'revision': 0}
    sized = copy.deepcopy(short)
    sized['size'], sized['duration'] = 'project', {'num': 1001, 'den': 3000}
    a = inspect(sized, project=saved)
    assert a['expanded_scene']['frame_rate'] == {'num': 30000, 'den': 1001}
    assert a['expanded_scene']['width'] == 128 and a['expanded_scene']['height'] == 72
    jobs = root / 'jobs'; jobs.mkdir()
    ticket = call({'command': 'job.start', 'job_root': str(jobs), 'request_id': 'short-render', 'run': 'scene.render',
                   'arguments': {'scene': sized, 'project': saved, 'input_root': str(inputs), 'output_root': str(output), 'output': str(output / 'queued.mkv')}})
    assert ticket['expanded_scene'] == a['expanded_scene']
    result = call({'command': 'job.wait', 'job_root': str(jobs), 'job_id': ticket['job_id'], 'timeout_seconds': 120})
    assert result['status'] == 'completed', (result['status'], result['error'])
    full_path = output / 'queued-reference.mkv'
    reference = call({'command': 'scene.render', 'scene': a['expanded_scene'], 'input_root': str(inputs), 'output_root': str(output), 'output': str(full_path)})
    assert result['result']['scene_sha256'] == reference['scene_sha256']
    for kind in ('video', 'audio'):
        assert decoded(output / 'queued.mkv', kind) == decoded(full_path, kind)
    passed.append('scene_short.saved_project_fractional_clock_and_pinned_queued_scene')

    template = {'schema_version': 1, 'id': 'short-template', 'scene': short, 'parameters': [
        {'name': 'label', 'definition': {'type': 'text', 'default': 'ABC', 'max_chars': 10}, 'bindings': [{'layer': 'label', 'property': 'text'}]}]}
    command = {'command': 'graphics.instantiate', 'template': template, 'instance_id': 'bound', 'input_root': str(inputs),
               'values': {'label': {'type': 'text', 'value': 'CAB'}}}
    a = call(command)
    command['template']['scene'] = explicit
    b = call(command)
    assert a['scene_sha256'] == b['scene_sha256'] and a['template_sha256'] == b['template_sha256']
    named = copy.deepcopy(short)
    named['layers'][1]['content']['font'] = installed.stem
    named_result = inspect(named)
    assert named_result['resolved_identities'][0]['font_name'] == installed.stem
    assert named_result['expanded_scene']['layers'][1]['graphics']['fonts'][0]['sha256'] == hashlib.sha256(installed.read_bytes()).hexdigest()
    passed.append('scene_short.template_bindings_and_unambiguous_installed_font')

    place_project = call({'command': 'project.create', 'id': 'short-place', 'width': 128, 'height': 72, 'frame_rate': 25})
    call({'command': 'session.create', 'store_root': str(store), 'project': place_project, 'request_id': 'create'})
    place_template = copy.deepcopy(template)
    place_template['scene'] = copy.deepcopy(short)
    place_template['scene']['size'] = 'project'
    generated = inputs / 'generated'; generated.mkdir()
    place = {'command': 'graphics.place', 'template': place_template, 'instance_id': 'placed',
             'store_root': str(store), 'project_id': 'short-place', 'expected_revision': 0, 'request_id': 'place',
             'input_root': str(inputs), 'output_root': str(generated), 'values': {'label': {'type': 'text', 'value': 'CAB'}}}
    placed = call(place)
    assert placed['revision'] == 1 and call(place) == placed
    expanded = call({'command': 'graphics.instantiate', 'template': place_template, 'instance_id': 'placed',
                     'project': place_project, 'input_root': str(inputs), 'values': place['values']})
    assert placed['scene_sha256'] == expanded['scene_sha256']
    reference_path = output / 'placed-reference.mkv'
    call({'command': 'scene.render', 'scene': expanded['scene'], 'input_root': str(inputs), 'output_root': str(output), 'output': str(reference_path)})
    assert placed['compiled'] is False and 'asset' not in placed and list(generated.iterdir()) == []
    native = call({'command': 'session.get', 'store_root': str(store), 'project_id': 'short-place'})
    assert native['schema_version'] == 2 and native['assets'] == []
    clip = next(t for t in native['tracks']['tracks'] if t['id'] == 'graphics')['clips'][0]
    assert clip['scene'] == expanded['scene'] and not clip.get('asset_id')
    placed_path = output / 'placed-timeline.mkv'
    call({'command': 'render.run', 'project': native, 'input_root': str(inputs), 'output_root': str(output), 'output': str(placed_path)})
    # Independently flatten the compiled transparent oracle over the timeline's black canvas.
    # Keep the earlier raw-alpha comparisons; timeline delivery itself is opaque.
    rgba = decoded(reference_path, 'video')
    assert len(rgba) == 10 * 128 * 72 * 4
    expected = bytearray(len(rgba))
    for offset in range(0, len(rgba), 4):
        alpha = rgba[offset + 3]
        expected[offset:offset + 4] = bytes([(rgba[offset + c] * alpha + 127) // 255 for c in range(3)] + [255])
    assert decoded(placed_path, 'video') == expected
    assert decoded(placed_path, 'audio') == decoded(reference_path, 'audio')
    passed.append('scene_short.project_sized_inline_placement_bound_digest_pixels_and_retry')

    for field, value, code in [('safe_area', 36, 'INVALID_SCENE'), ('background', '#badhex', 'INVALID_SCENE'),
                               ('background', '#0000', 'INVALID_SCENE'), ('size', 'project', 'INVALID_SCENE'),
                               ('duration', 0.01, 'UNALIGNED_TIME'), ('typo', 1, 'INVALID_JSON')]:
        invalid = copy.deepcopy(short); invalid[field] = value
        failure = call({'command': 'scene.inspect', 'scene': invalid, 'input_root': str(inputs)}, code)
        assert failure['path'] == 'duration' if code == 'UNALIGNED_TIME' else failure['path'].startswith('scene'), failure
    invalid = copy.deepcopy(short)
    invalid['layers'][0]['box_size'] = [200, 20]
    assert call({'command': 'scene.inspect', 'scene': invalid, 'input_root': str(inputs)}, 'INVALID_SCENE')['path'] == 'scene.layers[0].box_size'
    invalid = copy.deepcopy(short)
    invalid['layers'][1]['animation']['opacity']['duration'] = 0
    call({'command': 'scene.inspect', 'scene': invalid, 'input_root': str(inputs)}, 'INVALID_ANIMATION')
    invalid = copy.deepcopy(short)
    invalid['layers'][1]['content']['font'] = {**identity, 'sha256': '0'*64}
    call({'command': 'scene.inspect', 'scene': invalid, 'input_root': str(inputs)}, 'MEDIA_CHANGED')
    passed.append('scene_short.invalid_colors_bounds_fields_clocks_and_curves_rejected')

    example = ROOT / 'examples/title-card-short.json'
    assert example.stat().st_size < 1000
    example_scene = json.loads(example.read_text())
    checked = inspect(example_scene)
    rendered = call({'command': 'scene.render', 'scene': example_scene, 'input_root': str(inputs), 'output_root': str(output), 'output': str(output / 'example.mkv')})
    assert checked['frames'] == rendered['frames'] == 75
    assert identity['sha256'] == hashlib.sha256(font.read_bytes()).hexdigest()
    passed.append('scene_short.original_title_example_under_one_kilobyte_renders')
    report = {'passed': passed, 'example_bytes': example.stat().st_size, 'compared_frames': 40, 'sources_preserved': True}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    run(parser.parse_args().output)
