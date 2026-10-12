"""Execute every original skill JSON example over synthetic footage through compact stdio MCP."""
import argparse
import copy
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import numpy as np

from agents import Client
from agent_costs import transcript
from engine import ENGINE
from footage_sources import encode, identity, FFMPEG
from graphics import make_font, PRIMARY
from source_clocks import decoded as source_decoded, rgb_reference

ROOT = Path(__file__).resolve().parents[1]
NAMES = {'cutbolt', 'recut-talking-head', 'screen-tutorial', 'interview-multicam', 'captions', 'deliver-and-review'}
EXAMPLE = re.compile(r'<!-- example: ([a-z-]+) -->\s*(?:~~~|```)json\n(.*?)\n(?:~~~|```)', re.S)


def catalog(root=ROOT):
    manifest = json.loads((root / '.claude-plugin/plugin.json').read_text(encoding='utf-8'))
    marketplace = json.loads((root / '.claude-plugin/marketplace.json').read_text(encoding='utf-8'))
    assert manifest['name'] == 'cutbolt' and manifest['license'] == 'MIT'
    assert manifest['mcpServers'] == {'cutbolt': {'command':'cutbolt', 'args':['--workspace','${CLAUDE_PROJECT_DIR}','mcp']}}
    entry, = marketplace['plugins']
    assert marketplace['name'] == 'cutbolt-local' and entry['name'] == 'cutbolt'
    assert entry['source'] == './' and entry['version'] == manifest['version']
    recipes = {}
    contracts = {}
    for path in sorted((root / 'skills').glob('*/SKILL.md')):
        name = path.parent.name
        text = path.read_text(encoding='utf-8')
        assert name in NAMES and len(text.splitlines()) < 300, path
        assert text.startswith('---\n') and re.search(r'^name: ' + re.escape(name) + '$', text, re.M), path
        examples = EXAMPLE.findall(text)
        assert examples and len(examples) == len(re.findall(r'^(?:~~~|```)json\s*$', text, re.M)), path
        recipe = []
        for label, raw in examples:
            request = json.loads(raw)
            assert isinstance(request, dict) and isinstance(request.get('command'), str), (path, label)
            command = request['command']
            assert '_' not in command, (path, command)
            signature = command + (':' + request['run'] if command == 'job.start' else '')
            shape = (tuple(sorted(request)), tuple(sorted(request.get('arguments', {}))))
            if signature in contracts:
                assert contracts[signature]['keys'] == shape, (signature, contracts[signature], path, label, shape)
            else:
                contracts[signature] = {'keys': shape, 'first': f'{name}/{label}'}
            recipe.append((label, request))
        assert len({name for name, _ in recipe}) == len(recipe), path
        recipes[name] = recipe
    assert set(recipes) == NAMES
    return recipes, contracts


def substitute(value, context):
    if isinstance(value, str) and value.startswith('$'):
        assert value[1:] in context, f'Unbound documentation slot: {value}'
        return copy.deepcopy(context[value[1:]])
    if isinstance(value, list):
        return [substitute(v, context) for v in value]
    if isinstance(value, dict):
        return {k: substitute(v, context) for k, v in value.items()}
    return value


def cli(workspace, request):
    proc = subprocess.run([str(ENGINE), '--workspace', str(workspace)], input=json.dumps(request).encode(),
                          capture_output=True, timeout=120)
    result = json.loads(proc.stdout)
    assert proc.returncode == 0 and result['ok'], (result, proc.stderr)
    return result['result']


def decoded(path, audio=False):
    arguments = ['-map', '0:a:0', '-f', 's16le'] if audio else ['-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo']
    return subprocess.check_output([FFMPEG, '-v', 'error', '-i', str(path), *arguments, '-'], timeout=120)


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    recipes, contracts = catalog()
    executed = set()
    reports = {}
    passed = ['skills.six_small_original_recipes_with_consistent_command_argument_keys']
    cases = {
        'talking-head': [('recut-talking-head', None)],
        'talking-head-direct': [('recut-talking-head', None)],
        'caption-reuse': [('cutbolt', None), ('recut-talking-head', ['pauses','apply-pauses','fillers','apply-fillers']), ('captions', None), ('deliver-and-review', 4)],
        'tutorial': [('cutbolt', None), ('screen-tutorial', None), ('deliver-and-review', 2)],
        'interview': [('cutbolt', None), ('interview-multicam', None), ('deliver-and-review', 2)],
        'handoff': [('cutbolt', None), ('deliver-and-review', None)],
    }
    for case, sequence in cases.items():
        workspace = root / case
        workspace.mkdir()
        assets = []
        for n in range(2):
            encoding = {'codec':'h264','pix_fmt':'yuv420p','container':'mkv','audio_codec':'pcm_s16le'} if case == 'talking-head-direct' else {'codec':'ffv1','pix_fmt':'bgr0'}
            source = encode(workspace, f'camera{n}', rate=Fraction(25), count=150, phase=n*61, **encoding)
            assets.append({'id': f'camera{n}', 'path': source['path'], 'duration': 6,
                           'identity': {k: source[k] for k in ('bytes', 'sha256')}})
        make_font(workspace / 'font.ttf', PRIMARY)
        (workspace / 'transcript.json').write_text(json.dumps(transcript(assets[0])), encoding='utf-8')
        originals = {str(p.relative_to(workspace)): identity(p) for p in workspace.iterdir() if p.is_file()}
        context = {'PROJECT_ID': case, 'WIDTH': 160, 'HEIGHT': 90, 'FRAME_RATE': 25, 'REVISION': 0,
                   'SOURCE': assets[0]['path'], 'TRANSFER': 'bt709', 'FONT': {'path': 'font.ttf'},
                   'FONT_SIZE': 14, 'LINE_HEIGHT': 16, 'CAPTION_RECT': [8, 58, 144, 28],
                   'LEAD': 0.4, 'MISTAKE_START': 2, 'MISTAKE_DURATION': 0.8, 'TITLE': 'A',
                   'TITLE_DURATION': 1, 'TARGET_LKFS': -18, 'SWITCH': 3,
                   'OUTPUT': 'delivery.mp4', 'REVIEW_OUTPUT': 'review', 'INPUT_TRANSFER': 'bt709',
                   'HANDOFF_OUTPUT': 'handoff.otio'}
        environment = {k:v for k,v in os.environ.items() if k != 'CUTBOLT_MCP_TOOLS'}
        client = Client(ENGINE, env=environment, args=('--workspace', str(workspace), 'mcp'))
        calls = []
        results = {}
        try:
            client.initialize()
            names = {t['name'] for t in client.rpc('tools/list')['result']['tools']}
            assert len(names) == 18
            for skill, count in sequence:
                if skill == 'interview-multicam':
                    context['SOURCE'] = assets[1]['path']
                examples = [entry for entry in recipes[skill] if entry[0] in count] if isinstance(count,list) else recipes[skill][:count]
                if isinstance(count,list):
                    assert {label for label,_ in examples} == set(count)
                for label, example in examples:
                    request = substitute(example, context)
                    command = request.pop('command')
                    name = 'cutbolt_' + command.replace('.', '_')
                    fields = request
                    if name not in names:
                        name, fields = 'cutbolt_run', {'command': command, 'arguments': request}
                    raw = client.rpc('tools/call', {'name': name, 'arguments': fields})
                    assert 'error' not in raw, (skill, label, raw)
                    tool = raw['result']
                    envelope = tool['structuredContent']
                    assert not tool['isError'] and envelope['ok'], (skill, label, envelope)
                    result = envelope['result']
                    calls.append(command)
                    executed.add((skill, label))
                    results[f'{skill}/{label}'] = result
                    if command in ('session.create', 'session.apply', 'session.get'):
                        context['REVISION'] = result['revision']
                    if command == 'media.inspect':
                        timeline = result['timeline']
                        assert timeline['ready'] and timeline['width'] == 160 and timeline['height'] == 90
                        assert timeline['frame_rate'] == {'num':25, 'den':1}
                        asset = copy.deepcopy(timeline['asset'])
                        if case == 'talking-head-direct':
                            assert timeline['direct'] and asset['decode']['mode'] == 'direct'
                        asset.setdefault('identity', {k:result['identity'][k] for k in ('bytes','sha256')})
                        if skill in ('cutbolt','recut-talking-head'):
                            context.update(ASSET=asset, ASSET_ID=asset['id'], ASSET_A_ID=asset['id'], DURATION=asset['duration'])
                        else:
                            context.update(ASSET_B=asset, ASSET_B_ID=asset['id'])
                    if command == 'session.get':
                        clips = [c['id'] for t in result['tracks']['tracks'] for c in t['clips']]
                        context['OLD_CLIP_IDS'] = clips
                        context['RIGHT_CLIP_IDS'] = [{'id':c, 'new_id':c+'-right'} for c in clips]
                        context['RIGHT_LINK_IDS'] = [{'id':l['id'], 'new_id':l['id']+'-right'} for l in result['tracks']['links']]
                    if command in ('audio.tighten', 'transcript.fillers', 'audio.normalize'):
                        slot = {'audio.tighten':'PAUSE_OPERATIONS','transcript.fillers':'FILLER_OPERATIONS','audio.normalize':'LEVEL_OPERATIONS'}[command]
                        context[slot] = result['operations']
                        assert result['operations'], (case, command, result)
                    if command == 'captions.draft':
                        context['CAPTION_DOCUMENT'] = result['document']
                    if command == 'job.start':
                        context['JOB_ID'] = result['job_id']
                        context['JOB_RECEIPT'] = {'captions.render':'caption-job.json', 'graphics.place':'title-job.json',
                                                   'export.run':'export-job.json', 'export.review':'review-job.json'}[request['run']]
                    if command == 'job.wait':
                        assert result['status'] == 'completed', (skill,label,result.get('error'),result['status'])
                        outcome = result['result']
                        if context['JOB_RECEIPT'] == 'caption-job.json':
                            context.update(CAPTION_ASSET=outcome['asset'], CAPTION_ASSET_ID=outcome['asset']['id'], CAPTION_DURATION=outcome['asset']['duration'])
                        if context['JOB_RECEIPT'] == 'title-job.json':
                            context['REVISION'] = outcome['revision']
                    if command == 'interchange.export.inspect':
                        assert all(not loss['blocking'] and loss['code'] in ('editing_links', 'declared_transfer') for loss in result['losses']), result
                        # This original handoff task explicitly allows omission of edit links and the transfer declaration.
                        context['ACKNOWLEDGEMENTS'] = result['required_acknowledgements']
        finally:
            client.close()
        assert 'schema' not in calls
        deliver_skill = 'recut-talking-head' if case in ('talking-head','talking-head-direct') else 'deliver-and-review'
        export = results[deliver_skill + '/wait-export']['result']
        wanted = {'talking-head':124,'talking-head-direct':124,'caption-reuse':124,'tutorial':120,'interview':150,'handoff':150}[case]
        assert export['timeline_frames'] == wanted, (case, export['timeline_frames'])
        assert (workspace / 'delivery.mp4').is_file()
        if case in ('talking-head','talking-head-direct','caption-reuse','handoff'):
            reviewed = results[deliver_skill + '/wait-review']['result']
            assert reviewed['timing']['video']['ok'] and reviewed['timing']['audio']['ok']
        if case in ('talking-head','talking-head-direct','caption-reuse'):
            caption_skill = 'recut-talking-head' if case in ('talking-head','talking-head-direct') else 'captions'
            document = results[caption_skill + '/draft']['document']
            assert [cue['text'] for cue in document['cues']] == ['A B','A B']
            if caption_skill == 'recut-talking-head':
                cells = reviewed['picture']['sheet']['cells']
                times = [Fraction(cell['time']['num'],cell['time']['den']) for cell in cells if cell['present']]
                for cue in document['cues']:
                    start,end = [Fraction(cue[k]['num'],cue[k]['den']) for k in ('start','end')]
                    assert any(start + Fraction(1,25) <= t < end - Fraction(1,25) for t in times), (case,cue,times)
            # Independent assembly of requested source frames/samples and straight-alpha
            # captions: an initially populated session must preserve the former linked edit.
            spans = [(0,20),(25,74),(95,150)]
            source_path = workspace/assets[0]['path']
            source_rgb = rgb_reference(source_decoded(source_path,True,'yuv420p'),160,90) if case == 'talking-head-direct' else decoded(source_path)
            rgb = np.frombuffer(source_rgb,dtype=np.uint8).reshape(150,90,160,3)
            expected_rgb = np.concatenate([rgb[start:end] for start,end in spans]).astype(np.uint32)
            rgba = np.frombuffer(subprocess.check_output([FFMPEG,'-v','error','-i',str(workspace/'captions.mkv'),
                '-map','0:v:0','-pix_fmt','rgba','-f','rawvideo','-'],timeout=120),dtype=np.uint8).reshape(wanted,90,160,4).astype(np.uint32)
            alpha = rgba[:,:,:,3:4]
            composed = ((rgba[:,:,:,:3]*alpha + expected_rgb*(255-alpha) + 127)//255).astype(np.uint8)
            assert export['verification']['timeline_video_sha256'] == hashlib.sha256(composed.tobytes()).hexdigest()
            samples = np.frombuffer(decoded(workspace/assets[0]['path'],True),dtype='<i2').reshape(-1,2)
            expected_pcm = np.concatenate([samples[start*1920:end*1920] for start,end in spans])
            assert export['verification']['timeline_audio_sha256'] == hashlib.sha256(expected_pcm.tobytes()).hexdigest()
            assert len(calls) == 14
        if case == 'interview':
            a,b = [decoded(workspace / asset['path']) for asset in assets]
            cut = 75*160*90*3
            assert export['verification']['timeline_video_sha256'] == hashlib.sha256(a[:cut]+b[cut:]).hexdigest()
            assert export['verification']['timeline_audio_sha256'] == hashlib.sha256(decoded(workspace / assets[0]['path'],True)).hexdigest()
        if case == 'handoff':
            assert results['deliver-and-review/handoff']['ready'] and (workspace/'handoff.otio').is_file()
        assert originals == {name:identity(workspace/name) for name in originals}
        assert cli(workspace, {'command':'session.check'})
        assert len(calls) == {'talking-head':14,'talking-head-direct':14,'caption-reuse':14,'tutorial':10,'interview':7,'handoff':8}[case]
        reports[case] = {'calls':len(calls), 'schema_lookups':0, 'failed_calls':0, 'commands':calls, 'frames':wanted, 'revision':context['REVISION']}
        (workspace/'recipe-results.json').write_text(json.dumps(results,indent=2)+'\n',encoding='utf-8')
        passed.append('skills.'+case.replace('-','_')+'_recipe_outputs_and_source_preservation')
        print(f'{case}: {len(calls)} recipe calls, {wanted} frames, zero schema lookups',flush=True)
    assert executed == {(name,label) for name, recipe in recipes.items() for label,_ in recipe}
    report = {'passed':passed, 'kind':'scripted_skill_examples', 'example_count':len(executed),
              'contracts':contracts, 'workflows':reports, 'sources_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
