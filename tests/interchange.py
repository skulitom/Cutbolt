"""Original OTIO fixtures, independent library checks and decoded editorial oracles."""
from engine import ENGINE
import argparse
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess

from agents import Client
from jsonschema import Draft202012Validator
from PIL import Image
from tracks import time, seconds, edit, move

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 24, 40


def run(root, reference_python):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = [root/n for n in ('sources Ω', 'output Ω', 'store')]
    for directory in (sources, output, store):
        directory.mkdir()
    exe = ENGINE
    passed, cases, references = [], [], []
    frames = samples = rejected = blocked = 0

    def digest(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    def identity(path):
        return {'path': path.name, 'sha256': digest(path), 'bytes': path.stat().st_size}

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args],
                              capture_output=True, check=True, timeout=90).stdout

    def reference(mode, path, target=None):
        args = [str(reference_python), '-X', 'utf8', str(ROOT/'tests/interchange_reference.py'), mode, str(path)]
        if target:
            args += ['--output', str(target)]
        p = subprocess.run(args, capture_output=True, timeout=30)
        assert p.returncode == 0, (p.stdout, p.stderr)
        result = json.loads(p.stdout)
        references.append({'mode': mode, 'file': path.name, **result})
        return result['timeline']

    def call(command, error=None, **fields):
        nonlocal rejected
        p = subprocess.run([str(exe)], input=json.dumps({'command': command, **fields}).encode(),
                           capture_output=True, timeout=120)
        result = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and result['error']['code'] == error, (error, result)
            rejected += 1
            return result
        assert p.returncode == 0 and result['ok'], result
        return result['result']

    pictures, sounds, assets = [], [], []
    for source in range(3):
        rgb = [bytes(v for y in range(H) for x in range(W) for v in
                     ((source*51+n*7+x*3)%256, (source*17+y*11+n*13)%256, (x+y+n*19)%256)) for n in range(N)]
        pcm = array('h', (v for n in range(N*1920) for v in
                         ((n*23+source*503)%15001-7500, (n*41+source*901)%17001-8500)))
        raw, audio, movie = [sources/f'original-{source}.{ext}' for ext in ('rgb', 'pcm', 'mkv')]
        raw.write_bytes(b''.join(rgb)); audio.write_bytes(pcm.tobytes())
        ff(['-f', 'rawvideo', '-pixel_format', 'rgb24', '-video_size', f'{W}x{H}', '-framerate', '25', '-i', str(raw),
            '-f', 's16le', '-ar', '48000', '-ac', '2', '-i', str(audio), '-map', '0:v:0', '-map', '1:a:0',
            '-c:v', 'ffv1', '-level', '3', '-pix_fmt', 'bgr0', '-threads', '1', '-c:a', 'pcm_s16le', str(movie)])
        assets.append({'id': f'media-{source}', 'path': str(movie), 'duration': time(N, 25),
                       'identity': {k: v for k, v in identity(movie).items() if k != 'path'}})
        pictures.append(rgb); sounds.append(pcm)
    original = {p.name: digest(p) for p in sources.iterdir()}

    def preserved():
        assert original == {p.name: digest(p) for p in sources.iterdir()}

    authored = output/'authored.otio'
    authored_info = reference('create', authored)
    assert math.isclose(authored_info['duration'], 30/25, abs_tol=1e-12)
    assert [t['kind'] for t in authored_info['tracks']] == ['Video']*3+['Audio']*2
    assert authored_info['tracks'][2]['enabled'] is False
    assert math.isclose(authored_info['tracks'][3]['items'][1]['start'], 3841/48000, abs_tol=1e-12)
    bindings = [{'target_url': f'original-{i}.mkv', 'asset': asset} for i, asset in enumerate(assets)]

    def import_fields(path, **changes):
        return {'source': identity(path), 'input_root': str(output), 'media_root': str(sources), 'id': 'editorial',
                'width': W, 'height': H, 'frame_rate': time(25), 'bindings': copy.deepcopy(bindings), **changes}

    imported = call('interchange.import', **import_fields(authored))
    assert imported['ready'] and not imported['losses'] and not imported['writes_files'], imported
    project = imported['project']
    assert seconds(project['tracks']['duration']) == F(30, 25)
    assert len(project['tracks']['tracks']) == 5 and not project['tracks']['tracks'][2]['enabled']
    v, a = project['tracks']['tracks'][0], project['tracks']['tracks'][3]
    assert [seconds(c['start']) for c in v['clips']] == [F(2,25), F(12,25)]
    assert [seconds(c['source_in']) for c in v['clips']] == [F(4,25), F(6,25)]
    assert [seconds(c['start']) for c in a['clips']] == [F(3841,48000), F(12,25)]
    assert seconds(a['clips'][0]['source_in']) == F(5763,48000)
    assert seconds(a['transitions'][0]['before']) == F(2,25)
    assert seconds(a['transitions'][0]['after']) == F(3,25)
    assert any(n['source_name'] == 'Picture Ω' for n in imported['names'])
    passed.append('interchange.public_reference_tracks_gaps_and_sample_clocks')

    # Direct reference from the authored decisions; never uses imported placements.
    def expected(overlay_start=11):
        images, audio = [], array('h')
        for n in range(30):
            pixels = bytes(W*H*3)
            if 2 <= n < 24:
                pixels = pictures[0][4+n-2] if n < 12 else pictures[1][6+n-12]
            if 10 <= n < 15:
                k, d = 2*(n-10)+1, 10
                pixels = bytes((l*(d-k)+r*k+d//2)//d for l, r in
                               zip(pictures[0][4+n-2], pictures[1][6+n-12]))
            if overlay_start <= n < overlay_start+3:
                pixels = pictures[2][2+n-overlay_start]
            images.append(pixels)
        for n in range(30*1920):
            for channel in range(2):
                value = 0
                if 3841 <= n < 23040:
                    value = sounds[0][(5763+n-3841)*2+channel]
                elif 23040 <= n < 46080:
                    value = sounds[1][(11520+n-23040)*2+channel]
                if 19200 <= n < 28800:
                    k, d = 2*(n-19200)+1, 19200
                    value = sounds[0][(5763+n-3841)*2+channel]*(d-k)+sounds[1][(11520+n-23040)*2+channel]*k
                    value = (1 if value >= 0 else -1)*((abs(value)+d//2)//d)
                if 1 <= n < 57599:
                    value += sounds[2][(13+n-1)*2+channel]
                audio.append(max(-32768, min(32767, value)))
        return b''.join(images), audio.tobytes()

    expected_rgb, expected_pcm = expected()

    def render(value, label, rgb=expected_rgb, pcm=expected_pcm, media_root=sources):
        nonlocal frames, samples
        target = output/(label+'.mkv')
        call('render.run', project=value, input_root=str(media_root), output_root=str(output), output=str(target))
        actual_rgb = ff(['-i', str(target), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])
        actual_pcm = ff(['-i', str(target), '-vn', '-f', 's16le', '-'])
        assert actual_rgb == rgb, (label, 'video', next(((i,a,b) for i,(a,b) in enumerate(zip(actual_rgb,rgb)) if a!=b), None))
        assert actual_pcm == pcm, (label, 'audio', next(((i,a,b) for i,(a,b) in enumerate(zip(actual_pcm,pcm)) if a!=b), None))
        frames += len(rgb)//(W*H*3); samples += len(pcm)//4
        cases.append(label); preserved()

    render(project, 'imported')
    inspect = call('interchange.export.inspect', project=project, input_root=str(sources))
    assert inspect['ready'] and inspect['losses'] == [] and inspect['document']
    exported = output/'exported.otio'
    export_fields = {'project': project, 'input_root': str(sources), 'output_root': str(output), 'output': str(exported)}
    receipt = call('interchange.export', **export_fields)
    assert receipt['ready'] and receipt['sha256'] == digest(exported) and receipt['bytes'] == exported.stat().st_size
    normalized = output/'normalized.otio'
    native_info = reference('normalize', exported, normalized)
    assert math.isclose(native_info['duration'], 30/25, abs_tol=1e-12)
    assert native_info['tracks'][2]['enabled'] is False
    for old, new in zip(authored_info['tracks'], native_info['tracks']):
        old_items = [i for i in old['items'] if i['schema'] != 'Gap']
        new_items = [i for i in new['items'] if i['schema'] != 'Gap']
        assert len(old_items) == len(new_items)
        for before, after in zip(old_items, new_items):
            for key in ('schema', 'url'):
                assert before.get(key) == after.get(key), (key, before, after)
            for key in ('start', 'duration', 'source_in', 'before', 'after'):
                if key in before:
                    assert math.isclose(before[key], after[key], abs_tol=1e-12), (key,before,after)
    for name, path in [('roundtrip', exported), ('reference-normalized', normalized)]:
        result = call('interchange.import', **import_fields(path))
        assert result['ready'], result
        render(result['project'], name)
    passed.append('interchange.independent_decoded_roundtrip_transitions_and_audio')

    document = json.loads(authored.read_text(encoding='utf-8'))

    def variant(label, change):
        value = copy.deepcopy(document); change(value)
        path = output/(label+'.otio'); path.write_text(json.dumps(value), encoding='utf-8')
        return path

    def clips(value):
        return [c for t in value['tracks']['children'] for c in t['children'] if c['OTIO_SCHEMA'].startswith('Clip.')]

    def reference_value(c):
        return c['media_references'][c['active_media_reference_key']]

    def origins(value):
        for c in clips(value):
            ref = reference_value(c)
            for rt in (ref['available_range']['start_time'], c['source_range']['start_time']):
                rt['value'] -= 80*rt['rate']/25
    signed = variant('negative-origins', origins)
    result = call('interchange.import', **import_fields(signed))
    assert result['ready'] and result['project'] == project
    reference('inspect', signed)

    def legacy(value):
        for c in clips(value):
            c['OTIO_SCHEMA'] = 'Clip.1'; c['media_reference'] = reference_value(c)
            del c['media_references']; del c['active_media_reference_key']
    old_clip = variant('clip-version-one', legacy)
    assert call('interchange.import', **import_fields(old_clip))['project'] == project
    reference('inspect', old_clip)
    disabled = variant('disabled-clip', lambda d: clips(d)[2].update(enabled=False))
    d = call('interchange.import', **import_fields(disabled))
    assert not d['ready'] and any(l['code'] == 'disabled_clip' for l in d['losses'])
    d = call('interchange.import', **import_fields(disabled, acknowledged_losses=d['required_acknowledgements']))
    assert d['ready'] and not d['project']['tracks']['tracks'][1]['clips']
    # Disabled overlay becomes an explicitly acknowledged empty interval.
    render(d['project'], 'disabled-overlay', *expected(50))
    passed.append('interchange.signed_origins_versioned_clips_and_disabled_items')

    def losses(value):
        value['metadata'] = {'original-note': 'retain separately'}
        value['global_start_time'] = {'OTIO_SCHEMA':'RationalTime.1', 'value':90000, 'rate':25}
        c = clips(value)[0]
        c['effects'] = [{'OTIO_SCHEMA':'LinearTimeWarp.1','name':'Original speed','metadata':{},'effect_name':'LinearTimeWarp','time_scalar':2}]
        c['media_references']['INACTIVE'] = copy.deepcopy(reference_value(c))
        value['tracks']['children'][0]['children'][2]['transition_type'] = 'Custom_Original'
    lossy = variant('reported-losses', losses)
    result = call('interchange.import', **import_fields(lossy))
    assert not result['ready'] and result['project'] is None
    assert {'omitted_property','display_origin','inactive_references','transition_omitted'} <= {l['code'] for l in result['losses']}
    assert all(l['path'].startswith('$') and not l['blocking'] for l in result['losses'])
    acknowledged = result['required_acknowledgements']
    partial = call('interchange.import', **import_fields(lossy, acknowledged_losses=acknowledged[:-1]))
    assert not partial['ready'] and partial['project'] is None
    result = call('interchange.import', **import_fields(lossy, acknowledged_losses=acknowledged))
    assert result['ready'] and not result['project']['tracks']['tracks'][0].get('transitions', [])
    call('interchange.import', 'INVALID_LOSS_ACKNOWLEDGEMENT', **import_fields(lossy, acknowledged_losses=acknowledged+['unknown']))
    call('interchange.import', 'INVALID_LOSS_ACKNOWLEDGEMENT', **import_fields(lossy, acknowledged_losses=acknowledged*2))

    def blocked_import(label, change):
        nonlocal blocked
        path = variant(label, change)
        result = call('interchange.import', **import_fields(path))
        assert not result['ready'] and result['project'] is None and any(l['blocking'] for l in result['losses']), (label,result)
        blocked += 1
        call('interchange.import', 'INVALID_LOSS_ACKNOWLEDGEMENT', **import_fields(path, acknowledged_losses=[next(l['id'] for l in result['losses'] if l['blocking'])]))
    for label, change in [
        ('unknown-field', lambda d: clips(d)[0].update(mystery_timing=1)),
        ('nested-stack', lambda d: d['tracks']['children'][0]['children'][1].update(OTIO_SCHEMA='Stack.1')),
        ('missing-range', lambda d: reference_value(clips(d)[0]).update(available_range=None)),
        ('bad-rate', lambda d: clips(d)[0]['source_range']['duration'].update(rate=24.5)),
        ('bad-fractional-value', lambda d: clips(d)[0]['source_range']['duration'].update(value=10.25)),
        ('too-large-integer', lambda d: clips(d)[0]['source_range']['duration'].update(value=9007199254740992)),
        ('zero-rate', lambda d: clips(d)[0]['source_range']['duration'].update(rate=0)),
        ('negative-duration', lambda d: clips(d)[0]['source_range']['duration'].update(value=-1)),
        ('before-media', lambda d: clips(d)[0]['source_range']['start_time'].update(value=-1)),
        ('insufficient-handles', lambda d: clips(d)[1]['source_range']['start_time'].update(value=0)),
        ('unknown-media-reference', lambda d: reference_value(clips(d)[0]).update(OTIO_SCHEMA='GeneratorReference.1')),
        ('missing-binding', lambda d: reference_value(clips(d)[0]).update(target_url='https://example.invalid/never-fetched')),
        ('trimmed-track', lambda d: d['tracks']['children'][0].update(source_range=copy.deepcopy(clips(d)[0]['source_range']))),
    ]:
        blocked_import(label, change)

    lossy_native = copy.deepcopy(project)
    lossy_native['tracks']['tracks'][0]['locked'] = True
    lossy_native['tracks']['tracks'][0]['transitions'][0]['kind'] = 'dip_black'
    report = call('interchange.export.inspect', project=lossy_native, input_root=str(sources))
    assert not report['ready'] and report['document'] and {'track_lock','transition_omitted'} == {l['code'] for l in report['losses']}
    loss_output = output/'acknowledged-export.otio'
    loss_fields = {**export_fields, 'project':lossy_native, 'output':str(loss_output)}
    assert not call('interchange.export', **loss_fields)['ready'] and not loss_output.exists()
    assert call('interchange.export', **loss_fields, acknowledged_losses=report['required_acknowledgements'])['ready']
    reference('inspect', loss_output)
    reloaded = call('interchange.import', **import_fields(loss_output))['project']
    assert not reloaded['tracks']['tracks'][0]['locked'] and not reloaded['tracks']['tracks'][0].get('transitions', [])
    # Export of reusable definitions is blocked, even when they are currently unused.
    nested = call('timeline.apply', project=project, expected_revision=0,
                  operations=[{'op':'sequence.create','id':'reusable','duration':time(2,25)}])
    report = call('interchange.export.inspect', project=nested, input_root=str(sources))
    assert not report['ready'] and report['document'] is None and any(l['code']=='sequence_definitions' and l['blocking'] for l in report['losses'])
    blocked += 1
    multiple = copy.deepcopy(project)
    effects = multiple['tracks']['tracks'][0]['transitions']
    effects[0]['after'] = time(0)
    effects.append({**effects[0], 'id':'second-half','before':time(0),'after':time(3,25)})
    call('project.validate', project=multiple)
    report = call('interchange.export.inspect', project=multiple, input_root=str(sources))
    assert report['document'] is None and any(l['code']=='multiple_boundary_effects' and l['blocking'] for l in report['losses'])
    blocked += 1
    passed.append('interchange.reviewed_losses_and_unacknowledgeable_unknowns')

    sequential = call('project.create', id='sequential', width=W, height=H, frame_rate=time(25))
    sequential.update(assets=[assets[0]], clips=[
        {'id':'first','asset_id':assets[0]['id'],'source_in':time(2,25),'duration':time(4,25)},
        {'id':'gap','gap':True,'source_in':time(0),'duration':time(2,25)},
        {'id':'last','asset_id':assets[0]['id'],'source_in':time(12,25),'duration':time(3,25)}])
    report = call('interchange.export.inspect', project=sequential, input_root=str(sources))
    assert {l['code'] for l in report['losses']} == {'editing_links'}
    seq_path = output/'sequential.otio'
    assert call('interchange.export', **{**export_fields,'project':sequential,'output':str(seq_path)},
                acknowledged_losses=report['required_acknowledgements'])['ready']
    reference('inspect', seq_path)
    seq_round = call('interchange.import', **import_fields(seq_path))
    assert seq_round['ready']
    render(seq_round['project'], 'sequential-roundtrip', b''.join(pictures[0][2:6])+bytes(W*H*3*2)+b''.join(pictures[0][12:15]),
           sounds[0][2*1920*2:6*1920*2].tobytes()+bytes(2*1920*4)+sounds[0][12*1920*2:15*1920*2].tobytes())
    empty = call('project.create', id='empty', width=W, height=H, frame_rate=time(25))
    empty_path = output/'empty.otio'
    assert call('interchange.export', **{**export_fields,'project':empty,'output':str(empty_path)})['ready']
    assert reference('inspect', empty_path)['duration'] == 0
    assert call('interchange.import', **import_fields(empty_path))['ready']
    unused = copy.deepcopy(project); unused['assets'].append({**assets[0],'id':'unused'})
    report = call('interchange.export.inspect', project=unused, input_root=str(sources))
    assert not report['ready'] and {l['code'] for l in report['losses']} == {'unused_asset'}

    moved = root/'relocated Ω'; moved.mkdir()
    relative_bindings = copy.deepcopy(bindings)
    for binding in relative_bindings:
        source = Path(binding['asset']['path'])
        shutil.copy2(source, moved/source.name)
        binding['asset']['path'] = source.name
    relocated = call('interchange.import', **import_fields(authored, media_root=str(moved), bindings=relative_bindings))
    assert relocated['ready'] and all(Path(a['path']).is_absolute() for a in relocated['project']['assets'])
    render(relocated['project'], 'relative-relocated-media', media_root=moved)
    for index, binding in enumerate(relative_bindings):
        name = f'preview-{index}.mkv'
        proxy = call('proxy.generate', project=relocated['project'], expected_revision=0, asset_id=binding['asset']['id'],
                     scale=2, input_root=str(moved), output_root=str(moved), output=str(moved/name))['proxy']
        binding['asset']['proxy'] = {**proxy,'path':name}
    with_proxy = call('interchange.import', **import_fields(authored, media_root=str(moved), bindings=relative_bindings))
    assert with_proxy['ready'] and Path(with_proxy['project']['assets'][0]['proxy']['path']).is_absolute()
    preview_project = call('timeline.apply', project=with_proxy['project'], expected_revision=0,
                           operations=[{'op':'preview.proxy','scale':2}])
    preview_path = output/'relocated-proxy.png'
    call('preview.frame', project=preview_project, input_root=str(moved), output_root=str(output),
         output=str(preview_path), time=time(4,25))
    expected_small = bytes(v for y in range(H//2) for x in range(W//2)
                           for v in pictures[0][6][(y*2*W+x*2)*3:(y*2*W+x*2)*3+3])
    with Image.open(preview_path) as image:
        assert image.size == (W//2,H//2) and image.convert('RGB').tobytes() == expected_small
    render(with_proxy['project'], 'relative-relocated-proxy-master', media_root=moved)
    relative_bindings[0]['asset']['proxy']['identity']['sha256'] = '0'*64
    call('interchange.import', 'MEDIA_CHANGED', **import_fields(authored, media_root=str(moved), bindings=relative_bindings))

    exported_hash = digest(exported)
    call('interchange.export', 'OUTPUT_EXISTS', **export_fields)
    assert digest(exported) == exported_hash
    call('interchange.export', 'PATH_OUTSIDE_ROOT', **{**export_fields,'output':str(root/'escape.otio')})
    call('interchange.export', 'UNSUPPORTED_OUTPUT', **{**export_fields,'output':str(output/'wrong.json')})
    call('interchange.import', 'PATH_OUTSIDE_ROOT', **import_fields(authored, media_root=str(output)))
    traversal = identity(authored); traversal['path'] = '../output Ω/authored.otio'
    call('interchange.import', 'INVALID_PATH', **import_fields(authored, source=traversal))
    bad_bindings = copy.deepcopy(bindings); bad_bindings[0]['asset']['identity']['sha256'] = '0'*64
    call('interchange.import', 'MEDIA_CHANGED', **import_fields(authored, bindings=bad_bindings))
    bad_bindings = copy.deepcopy(bindings); del bad_bindings[0]['asset']['identity']
    call('interchange.import', 'IDENTITY_REQUIRED', **import_fields(authored, bindings=bad_bindings))
    call('interchange.import', 'INVALID_BINDING', **import_fields(authored, bindings=bindings+[bindings[0]]))
    changed_identity = identity(authored); changed_identity['sha256'] = '0'*64
    call('interchange.import', 'MEDIA_CHANGED', **import_fields(authored, source=changed_identity))
    huge = identity(authored); huge['bytes'] = 4*1024*1024+1
    call('interchange.import', 'LIMIT_EXCEEDED', **import_fields(authored, source=huge))
    assert not list(output.glob('.cutbolt-interchange-*'))
    preserved()
    passed.append('interchange.identity_roots_limits_and_no_overwrite')

    client = Client(exe)
    try:
        client.initialize()
        catalog = client.rpc('tools/list')['result']['tools']
        assert len(catalog) == 65
        for command, fields, read_only in [('interchange.import',import_fields(authored),True),
            ('interchange.export.inspect',{'project':project,'input_root':str(sources)},True),
            ('interchange.export',{**export_fields,'output':str(output/'mcp.otio')},False)]:
            tool = next(t for t in catalog if t['name']=='cutbolt_'+command.replace('.','_'))
            Draft202012Validator(tool['inputSchema']).validate(fields)
            assert tool['annotations']['readOnlyHint'] is read_only
            assert tool['annotations']['idempotentHint'] is read_only
            assert client.call(command, **fields)['ready']
        common = {'store_root':str(store),'project_id':project['id']}
        client.call('session.create', store_root=str(store), project=project, request_id='create')
        operations = [move([project['tracks']['tracks'][1]['clips'][0]['id']], 1)]
        client.call('session.preview', **common, expected_revision=0, operations=operations)
        request = {**common,'expected_revision':0,'request_id':'move','operations':operations}
        edited = client.call('session.apply', **request)
        assert client.call('session.apply', **request) == edited
        moved_project = client.call('session.get', **common)
        render(moved_project, 'saved-edit', *expected(12))
        client.call('session.undo', **common, expected_revision=1, request_id='undo')
        render(client.call('session.get', **common), 'saved-undo')
        history = client.call('session.history', **common)
        assert history
    finally:
        client.close()
    preserved()
    passed.append('interchange.typed_tools_saved_edits_retry_and_undo')
    report = {'passed':passed,'profile':'otio-editorial-v1','reference_library':{'name':'OpenTimelineIO','version':'0.18.1','license':'Apache-2.0'},
              'frames_compared':frames,'stereo_samples_compared':samples,'rejected':rejected,'blocked_structures':blocked,
              'rendered_cases':cases,'reference_checks':references,'sources_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k!='reference_checks'},indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--reference-python',type=Path,required=True)
    args = parser.parse_args()
    run(args.output,args.reference_python)
