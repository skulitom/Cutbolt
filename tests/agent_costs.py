"""Five scripted MCP-only workflows, measured on the wire through the official Python SDK.

Scripts are best-known recipes, not model trials. The transparent local relay records complete
JSON-RPC lines outside Git, including duplicate text/structured content, images and notifications.
"""
import argparse
import asyncio
from datetime import timedelta
from fractions import Fraction as F
import importlib.metadata
import json
import os
from pathlib import Path
import subprocess
import sys
import threading

from engine import ENGINE
from footage_sources import FFMPEG, ROOT, encode, identity
from graphics import PRIMARY, make_font

# The explicit full catalog retains stage-0 ceilings for compatibility comparisons.
# Default/core budgets were tightened after stage 2 on Windows and Linux.
FULL_BUDGETS = {  # calls, request bytes, received bytes (including initialize/list and image blocks)
    'talking-head': (17, 10_000, 455_000),
    'interview': (7, 5_000, 280_000),
    'tutorial': (14, 9_000, 300_000),
    'revision': (9, 3_000, 275_000),
    'handoff': (9, 3_000, 430_000),
}


COMPACT_BUDGETS = {
    'talking-head': (15, 9_500, 235_000),
    'interview': (6, 4_500, 55_000),
    'tutorial': (10, 4_000, 60_000),
    'revision': (8, 2_500, 48_000),
    'handoff': (8, 2_600, 210_000),
}
BUDGETS = FULL_BUDGETS if os.environ.get('CUTBOLT_COST_CATALOG') == 'full' else COMPACT_BUDGETS
LIST_BUDGET = 250_000 if os.environ.get('CUTBOLT_COST_CATALOG') == 'full' else 40_000


def relay(executable, workspace, wire):
    process = subprocess.Popen([str(executable), '--workspace', str(workspace), 'mcp'],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE)
    lock = threading.Lock()
    with wire.open('x', encoding='utf-8') as log:
        def record(direction, line):
            with lock:
                log.write(json.dumps({'direction': direction, 'bytes': len(line), 'message': json.loads(line)}) + '\n')
                log.flush()

        def send():
            try:
                for line in sys.stdin.buffer:
                    record('sent', line)
                    process.stdin.write(line)
                    process.stdin.flush()
            finally:
                process.stdin.close()

        sender = threading.Thread(target=send, daemon=True)
        sender.start()
        try:
            for line in process.stdout:
                record('received', line)
                sys.stdout.buffer.write(line)
                sys.stdout.buffer.flush()
            assert process.wait(timeout=10) == 0
            sender.join(timeout=10)
        finally:
            if process.poll() is None:
                process.kill()
                process.wait()


def track_edit(op, **fields):
    return {'op': 'tracks.edit', 'edit': {'op': op, **fields}}


def track(ident, kind, **fields):
    return {'id': ident, 'kind': kind, 'clips': [], 'enabled': True, 'locked': False, **fields}


def transcript(asset):
    return {'schema_version': 1, 'id': 'words', 'revision': 0, 'parent_fingerprint': None,
            'source': {'path': asset['path'], 'identity': asset['identity'], 'duration': 6},
            'range_start': 0, 'range_duration': 6, 'language': 'en',
            'recognition': {'profile': 'original-canned-contract-fixture', 'model': {'bytes': 1, 'sha256': 'a' * 64},
                            'worker_sha256': 'b' * 64, 'analysis_sha256': 'c' * 64, 'versions': {'fixture': '1'}},
            'words': [{'id': f'w{i}', 'text': text, 'start': start, 'end': end,
                       'origin': 'estimated', 'probability_milli': 900}
                      for i, (text, start, end) in enumerate([('A', 0.2, 0.6), ('um', 0.8, 1),
                                                             ('B', 1.2, 1.6), ('A', 4.2, 4.6), ('B', 5, 5.4)])]}


class Replay:
    def __init__(self, session, names, name):
        self.session, self.names, self.name = session, names, name
        self.revision = 0
        self.project = {'project_id': name}
        self.commands = []

    async def request(self, command, fields):
        self.commands.append(command)
        name = 'cutbolt_' + command.replace('.', '_')
        if name not in self.names:
            assert 'cutbolt_run' in self.names, command
            name, fields = 'cutbolt_run', {'command': command, 'arguments': fields}
        response = await self.session.call_tool(name, fields)
        return response

    async def call(self, command, **fields):
        response = await self.request(command, fields)
        data = response.structuredContent
        assert not response.isError and data['ok'], (command, data)
        return data['result']

    async def mistake(self, command, **fields):
        response = await self.request(command, fields)
        data = response.structuredContent
        assert response.isError and not data['ok'], (command, data)
        # Recovery deliberately cannot read either legacy field. Keep the complete envelope
        # on the wire for byte accounting, but expose only structured guidance to this recipe.
        guidance = {k: v for k, v in data['error'].items() if k not in {'code', 'message'}}
        assert guidance['retry'] == 'changed' and guidance['path'] and guidance['hint'], guidance
        return guidance

    async def apply(self, operations):
        result = await self.call('session.apply', project_id=self.name, expected_revision=self.revision,
                                 request_id=f'edit-{self.revision}', operations=operations)
        self.revision = result['revision']
        return result

    async def job(self, command, **fields):
        ticket = await self.call('job.start', request_id=f'job-{len(self.commands)}', run=command, arguments=fields)
        # One long wait keeps the sequence stable across platforms; a slow host must fail the budget
        # explicitly, rather than silently adding polling calls to its baseline.
        done = await self.call('job.wait', job_id=ticket['job_id'], timeout_seconds=120)
        assert done['status'] == 'completed', (command, done)
        return done['result']

    async def setup(self, assets, placed=True):
        await self.call('session.create', id=self.name, width=160, height=90, frame_rate=25, request_id='create')
        operations = [{'op': 'media.add', 'asset': a} for a in assets]
        operations.append({'op': 'clip.append', 'clip': {'id': 'shot', 'asset_id': assets[0]['id'],
                                                        'source_in': 0, 'duration': 6}})
        if placed:
            operations.append(track_edit('promote', video_track_id='v', audio_track_id='a'))
        await self.apply(operations)

    async def export(self, output='delivery.mp4', range_=None):
        result = await self.job('export.run', project=self.project, output=output, profile='h264_aac',
                                streams='audio_video', input_transfer='bt709', **({'range': range_} if range_ else {}))
        assert result['timeline_frames'] > 0, result
        return result

    async def review(self, output='delivery.mp4'):
        result = await self.job('export.review', path=output, output='review', project=self.project,
                                frames=4, rendition_height=0)
        assert result['timing']['video']['ok'] and result['timing']['audio']['ok'], result
        return result


async def workflow(name, workspace, assets, font, executable):
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    environment = {k: v for k, v in os.environ.items() if k != 'CUTBOLT_MCP_TOOLS'}
    if os.environ.get('CUTBOLT_COST_CATALOG'):
        environment['CUTBOLT_MCP_TOOLS'] = os.environ['CUTBOLT_COST_CATALOG']
    parameters = StdioServerParameters(command=sys.executable, args=[str(Path(__file__).resolve()), '--relay',
        '--engine', str(executable), '--output', str(workspace), '--wire', str(workspace / 'wire.jsonl')],
        env=environment)
    async with stdio_client(parameters) as (read, write):
        async with ClientSession(read, write, read_timeout_seconds=timedelta(seconds=150)) as session:
            await session.initialize()
            listing = await session.list_tools()
            replay = Replay(session, {t.name for t in listing.tools}, name)
            title_commands = []
            if name == 'recovery':
                intended = {'id': name, 'width': 160, 'height': 90, 'frame_rate': 25, 'request_id': 'create'}
                bad = await replay.mistake('session.create', **{**intended, 'width': 'wide'})
                assert bad['path'] == 'width'
                await replay.call('session.create', **intended)
                await replay.apply([{'op': 'clip.append', 'clip': {'id': 'first', 'gap': True, 'source_in': 0, 'duration': 1}}])
                edit = {'project_id': name, 'expected_revision': 0, 'request_id': 'stale',
                        'operations': [{'op': 'clip.append', 'clip': {'id': 'second', 'gap': True, 'source_in': 0, 'duration': 1}}]}
                bad = await replay.mistake('session.apply', **edit)
                assert bad['path'] == 'expected_revision'
                refreshed = await replay.call('session.get', project_id=name)
                assert refreshed['revision'] == bad['detail']['current_revision']
                # Reconsider the append against the actual head before applying it once.
                assert len(refreshed['clips']) == 1 and refreshed['clips'][0]['id'] == 'first'
                applied = await replay.call('session.apply', **{**edit, bad['path']: refreshed['revision']})
                assert applied['revision'] == 2
                frame = {'project': replay.project, 'time': 0, 'output': 'frame.png'}
                await replay.call('preview.frame', **frame)
                original = identity(workspace / frame['output'])
                bad = await replay.mistake('preview.frame', **frame)
                assert bad['path'] == 'output'
                candidate = Path(bad['detail']['suggested_output'])
                if not candidate.is_absolute():
                    candidate = workspace / candidate
                assert candidate.parent.samefile(workspace) and not candidate.exists()
                await replay.call('preview.frame', **{**frame, bad['path']: str(candidate)})
                assert identity(candidate) == original == identity(workspace / frame['output'])
                return replay.commands, []
            await replay.setup(assets if name in {'revision', 'interview'} else assets[:1], placed=name != 'revision')
            if name == 'talking-head':
                doc = transcript(assets[0])
                await replay.call('transcript.inspect', document=doc)
                pauses = await replay.call('audio.tighten', project=replay.project, voice_track_id='a')
                assert pauses['operations'], pauses
                await replay.apply(pauses['operations'])
                fillers = await replay.call('transcript.fillers', project=replay.project, transcripts=[doc])
                assert fillers['operations'], fillers
                await replay.apply(fillers['operations'])
                captions = await replay.call('captions.draft', project=replay.project, transcripts=[doc])
                document = captions['document']
                assert document['cues'] and all('um' not in cue['text'] for cue in document['cues'])
                overlay = await replay.job('captions.render', document=document, project=replay.project,
                    layouts={'default': {'fonts': [font], 'size': 14, 'rect': [8, 58, 144, 28], 'line_height': 16,
                                         'letter_spacing': 0, 'wrap': 'none', 'overflow': 'clip'}}, output='captions.mkv')
                await replay.apply([{'op': 'media.add', 'asset': overlay['asset']},
                    track_edit('add', track=track('captions', 'video', composite='alpha_over')),
                    track_edit('place', track_id='captions', clip={'id': 'caption', 'asset_id': overlay['asset']['id'],
                        'start': 0, 'source_in': 0, 'duration': overlay['asset']['duration']}, collision='reject')])
                exported = await replay.export()
                assert exported['timeline_frames'] == 124
                await replay.review()
            elif name == 'interview':
                # Keep synchronized alternatives as child sequences; fixed audio survives camera cuts.
                ops = []
                for n, asset in enumerate(assets):
                    seq = f'cam{n}'
                    ops.append({'op': 'sequence.create', 'id': seq, 'duration': 6})
                    for kind in ('video', 'audio'):
                        ops.extend([{'op': 'sequence.edit', 'id': seq, 'edit': {'op': 'add', 'track': track(kind, kind)}},
                            {'op': 'sequence.edit', 'id': seq, 'edit': {'op': 'place', 'track_id': kind, 'collision': 'reject',
                             'clip': {'id': kind, 'asset_id': asset['id'], 'start': 0, 'source_in': 0, 'duration': 6}}}])
                ops += [{'op': 'multicam.create', 'id': 'program', 'group': {'duration': 6, 'locked': False,
                    'angles': [{'id': f'angle{n}', 'sequence_id': f'cam{n}', 'start': 0, 'duration': 6, 'source_in': 0, 'audio_in': 0}
                               for n in range(2)], 'cuts': [{'id': 'first', 'at': 0, 'angle_id': 'angle0'},
                                {'id': 'second', 'at': 3, 'angle_id': 'angle1'}], 'audio': {'mode': 'fixed', 'angle_id': 'angle0'}}}]
                # Replace both linked placements using their actual saved identifiers.
                saved = await replay.call('session.get', project_id=name)
                ids = [c['id'] for t in saved['tracks']['tracks'] for c in t['clips']]
                ops.append(track_edit('remove', clip_ids=ids, links='include'))
                for kind, tid in [('video', 'v'), ('audio', 'a')]:
                    ops.append(track_edit('place', track_id=tid, clip={'id': kind, 'sequence_id': 'program',
                        'start': 0, 'source_in': 0, 'duration': 6}, collision='reject'))
                ops.append(track_edit('link', id='interview-av', clip_ids=['video', 'audio']))
                await replay.apply(ops)
                exported = await replay.export()
                # The streamed encoder's inputs must be the independently assembled camera cut
                # and the complete, unchanged master audio. No model tokens are spent on this oracle.
                import hashlib
                def decoded(path, audio=False):
                    arguments = ['-map', '0:a:0', '-f', 's16le'] if audio else ['-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo']
                    return subprocess.check_output([FFMPEG, '-v', 'error', '-i', str(path), *arguments, '-'], timeout=120)
                a, b = [decoded(workspace / asset['path']) for asset in assets]
                boundary = 75 * 160 * 90 * 3
                assert exported['verification']['timeline_video_sha256'] == hashlib.sha256(a[:boundary] + b[boundary:]).hexdigest()
                assert exported['verification']['timeline_audio_sha256'] == hashlib.sha256(decoded(workspace / assets[0]['path'], True)).hexdigest()
            elif name == 'tutorial':
                saved = await replay.call('session.get', project_id=name)
                # A linked boundary trim and a ripple delete, followed by a title and loudness pass.
                ids = [c['id'] for t in saved['tracks']['tracks'] for c in t['clips']]
                await replay.apply([track_edit('ripple_delete', track_ids=['v', 'a'], start=0, duration=0.4,
                    links='include', right_clip_ids=[], right_link_ids=[], end_policy='resize', transitions='reject_affected'),
                    track_edit('ripple_delete', track_ids=['v', 'a'], start=2, duration=0.8, links='include',
                        right_clip_ids=[{'id': cid, 'new_id': cid + '-right'} for cid in ids],
                        right_link_ids=[{'id': l['id'], 'new_id': l['id'] + '-right'} for l in saved['tracks']['links']],
                        end_policy='resize', transitions='reject_affected')])
                start = len(replay.commands)
                title = await replay.job('graphics.place', project_id=name, expected_revision=replay.revision,
                    request_id=f'title-{replay.revision}', instance_id='title', track_id='titles',
                    template_id='cutbolt-title-card-v1', values={'fonts': {'type': 'fonts', 'value': [font]},
                        'label': {'type': 'text', 'value': 'A'}, 'duration': {'type': 'time', 'value': 1},
                        'transparent': {'type': 'boolean', 'value': True}})
                replay.revision = title['revision']
                title_commands = list(range(start, len(replay.commands)))
                normalized = await replay.call('audio.normalize', project=replay.project, target_lkfs=-18)
                assert normalized['operations'], normalized
                await replay.apply(normalized['operations'])
                exported = await replay.export()
                assert exported['timeline_frames'] == 120
            elif name == 'revision':
                before = await replay.call('session.get', project_id=name)
                await replay.apply([{'op': 'clip.overwrite', 'at': 2, 'right_id': 'tail',
                    'clip': {'id': 'replacement', 'asset_id': assets[1]['id'], 'source_in': 1, 'duration': 1}}])
                undone = await replay.call('session.undo', project_id=name, expected_revision=replay.revision, request_id='undo')
                replay.revision = undone['revision']
                after = await replay.call('session.get', project_id=name)
                assert after['clips'] == before['clips'] and after['assets'] == before['assets']
                exported = await replay.export(range_={'start': 2, 'duration': 1})
                assert exported['timeline_frames'] == 25
            elif name == 'handoff':
                await replay.export()
                await replay.review()
                losses = await replay.call('interchange.export.inspect', project=replay.project)
                assert all(not loss['blocking'] and loss['code'] == 'editing_links' for loss in losses['losses']), losses
                interchange = await replay.call('interchange.export', project=replay.project, output='handoff.otio',
                                                acknowledged_losses=losses['required_acknowledgements'])
                assert interchange['ready'] and (workspace / 'handoff.otio').is_file()
            else:
                raise AssertionError(name)
            assert (workspace / 'delivery.mp4').is_file()
            return replay.commands, title_commands


def measure(workspace, commands, title_commands):
    messages = [json.loads(line) for line in (workspace / 'wire.jsonl').read_text().splitlines()]
    requests = [m for m in messages if m['direction'] == 'sent' and 'id' in m['message']]
    responses = {m['message']['id']: m for m in messages if m['direction'] == 'received' and 'id' in m['message']}
    calls = []
    initialize = listing = 0
    for request in requests:
        msg = request['message']
        response = responses[msg['id']]
        if msg['method'] == 'initialize':
            initialize = response['bytes']
        elif msg['method'] == 'tools/list':
            listing = response['bytes']
        elif msg['method'] == 'tools/call':
            command = commands[len(calls)]
            tool = msg['params']['name']
            actual = msg['params']['arguments']['command'] if tool == 'cutbolt_run' else tool.removeprefix('cutbolt_').replace('_', '.')
            assert actual == command, (actual, command)
            payload = response['message'].get('result', {})
            job = payload.get('structuredContent', {}).get('result', {})
            calls.append({'command': command, 'sent_bytes': request['bytes'], 'received_bytes': response['bytes'],
                          'failed': bool(response['message'].get('error') or payload.get('isError') or
                                         (command in {'job.wait', 'job.status'} and job.get('status') in {'failed', 'interrupted', 'cancelled'}))})
    assert len(calls) == len(commands)
    result = {'initialize_bytes': initialize, 'tools_list_bytes': listing, 'calls': len(calls),
              'failed_calls': sum(c['failed'] for c in calls), 'schema_lookups': commands.count('schema'),
              'capabilities_lookups': commands.count('capabilities'), 'per_call': calls,
              'sent_bytes': sum(m['bytes'] for m in messages if m['direction'] == 'sent'),
              'received_bytes': sum(m['bytes'] for m in messages if m['direction'] == 'received')}
    if title_commands:
        result['title'] = {'calls': len(title_commands), 'sent_bytes': sum(calls[i]['sent_bytes'] for i in title_commands)}
        assert result['title']['calls'] <= 2 and result['title']['sent_bytes'] <= 1500, result['title']
    return result


def run(root, executable, fixtures_only=False):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    assert importlib.metadata.version('mcp') == '1.12.3'
    results = {}
    for name in ['talking-head', 'interview', 'tutorial', 'revision', 'handoff']:
        workspace = root / name
        workspace.mkdir()
        assets = []
        for n in range(2):
            case = encode(workspace, f'camera{n}', codec='ffv1', rate=F(25), count=150, pix_fmt='bgr0', phase=n * 61)
            assets.append({'id': f'camera{n}', 'path': case['path'], 'duration': 6,
                           'identity': {k: case[k] for k in ['bytes', 'sha256']}})
        make_font(workspace / 'font.ttf', PRIMARY)
        font = {'path': 'font.ttf', **identity(workspace / 'font.ttf')}
        for filename, value in [('assets.json', assets), ('font.json', font), ('transcript.json', transcript(assets[0]))]:
            (workspace / filename).write_text(json.dumps(value, indent=2) + '\n', encoding='utf-8')
        (workspace / 'title-template.json').write_bytes((ROOT / 'examples/title-card-template.json').read_bytes())
        if fixtures_only:
            print(f'{name}: {workspace}', flush=True)
            continue
        originals = {p.name: identity(p) for p in workspace.iterdir() if p.suffix in {'.mkv', '.wav', '.ttf'}}
        commands, title_commands = asyncio.run(workflow(name, workspace, assets, font, executable))
        assert originals == {p: identity(workspace / p) for p in originals}
        results[name] = measure(workspace, commands, title_commands)
        print(f"{name:14} {results[name]['calls']:3} calls  {results[name]['sent_bytes']:8} sent  {results[name]['received_bytes']:8} received", flush=True)
    if fixtures_only:
        return
    recovery_workspace = root / 'recovery'
    recovery_workspace.mkdir()
    commands, title_commands = asyncio.run(workflow('recovery', recovery_workspace, [], None, executable))
    recovery = measure(recovery_workspace, commands, title_commands)
    assert recovery['calls'] == 9 and recovery['failed_calls'] == 3
    assert recovery['schema_lookups'] == recovery['capabilities_lookups'] == 0
    assert recovery['sent_bytes'] <= 4000 and recovery['received_bytes'] <= LIST_BUDGET + 35_000
    print(f"Recovery: {recovery['calls']} calls, 3 deliberate mistakes repaired using structured guidance", flush=True)
    report = {'passed': ['agent_costs.five_sdk_workflows_with_wire_budgets', 'agent_costs.structured_error_recovery'], 'kind': 'scripted_replay',
              'sdk_version': importlib.metadata.version('mcp'), 'engine': subprocess.check_output([str(executable), '--version']).decode().strip(),
              'catalog': os.environ.get('CUTBOLT_COST_CATALOG', 'default'),
              'sources_preserved': True, 'budgets': BUDGETS, 'workflows': results, 'recovery': recovery}
    (root / 'verification.json').write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    for name, result in results.items():
        calls, sent, received = BUDGETS[name]
        assert result['calls'] <= calls and result['sent_bytes'] <= sent and result['received_bytes'] <= received, (name, result)
        assert result['initialize_bytes'] <= 1600 and result['tools_list_bytes'] <= LIST_BUDGET, (name, result)
        assert result['failed_calls'] == result['schema_lookups'] == result['capabilities_lookups'] == 0


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--engine', type=Path, default=ENGINE)
    parser.add_argument('--relay', action='store_true')
    parser.add_argument('--wire', type=Path)
    parser.add_argument('--fixtures-only', action='store_true', help='Prepare fresh inputs for independent model trials, without making MCP calls')
    args = parser.parse_args()
    if args.relay:
        relay(args.engine.resolve(), args.output.resolve(), args.wire)
    else:
        run(args.output, args.engine.resolve(), args.fixtures_only)
