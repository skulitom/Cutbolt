"""Complete original-footage timelines against full conform, including audio and delivery."""
from copy import deepcopy
from fractions import Fraction as F
import json
import os
from pathlib import Path
import subprocess
import tempfile

from engine import ENGINE
from footage_sources import identity
from source_clocks import decoded, time


def verify(root, case, asset, client):
    rate = F(case['rate'])
    out = root/'binding-previews'
    env = {**os.environ, 'CUTBOLT_INSPECTION_CACHE':str(root/'binding-cache')}
    source = dict(asset, path='sources/' + asset['path'])
    native = {**source, 'path':'conform/' + case['name'] + '.mkv'}
    native.pop('decode')
    native['identity'] = identity(root/native['path'])

    def call(command, **args):
        request = dict(command=command, **args)
        process = subprocess.Popen([str(ENGINE)], env=env, stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding='utf-8')
        stdout, stderr = process.communicate(json.dumps(request), timeout=180)
        assert not list(Path(tempfile.gettempdir()).glob(f'.cutbolt-direct-audio-{process.pid}-*.wav'))
        response = json.loads(stdout)
        assert process.returncode == 0 and response['ok'], (case['name'], command, response)
        return response['result']

    # These windows cross a keyframe and start mid-GOP. Multiples of five frames are
    # also exact 48 kHz sample positions at all eight accepted fractional/native rates.
    project = dict(schema_version=1, id='direct-timeline', revision=0,
                   width=case['size'][0], height=case['size'][1], frame_rate=time(rate),
                   assets=[source], clips=[
                       dict(id='left', asset_id=asset['id'], source_in=time(F(55)/rate), duration=time(F(10)/rate)),
                       dict(id='right', asset_id=asset['id'], source_in=time(F(75)/rate), duration=time(F(10)/rate))])
    comparisons = []

    def compare(snapshot, label, command='render.run', **args):
        reference = deepcopy(snapshot)
        reference['assets'] = [native]
        extension = '.mp4' if args.get('profile') == 'h264_aac' else '.wav' if args.get('streams') == 'audio' else '.mkv'
        outputs = []
        for kind, p in [('reference',reference), ('direct',snapshot)]:
            path = out/(case['name']+'-av-'+label+'-'+kind+extension)
            before = deepcopy(p)
            receipt = call(command, project=p, input_root=str(root), output_root=str(out), output=str(path), **args)
            assert p == before
            outputs.append(path)
            if 'sources' in receipt:
                assert any(s.get('sha256') == p['assets'][0]['identity']['sha256'] for s in receipt['sources']), receipt
        if args.get('streams') != 'audio':
            assert decoded(outputs[0],True) == decoded(outputs[1],True), (case['name'],label,'video')
        expected, actual = [decoded(p,False) for p in outputs]
        assert expected == actual, (case['name'],label,'audio')
        assert len(actual) > 0
        comparisons.append(dict(case=label, pcm_bytes=len(actual), identical=True))

    compare(project,'sequential')
    tracked = client.call('timeline.apply', project=project, expected_revision=0, operations=[
        {'op':'tracks.edit','edit':{'op':'promote','video_track_id':'v','audio_track_id':'a'}}])
    compare(tracked,'tracks')
    # Linked slip moves the left source window from 55 to 60; roll moves the shared cut
    # to 15 and extends that same window to 75. Both video and audio must follow it.
    edited = client.call('timeline.apply',project=tracked,expected_revision=tracked['revision'],operations=[
        {'op':'tracks.edit','edit':{'op':'slip','clip_ids':['left'], 'links':'include',
                                  'shift':{'backward':False,'amount':time(F(5)/rate)}}},
        {'op':'tracks.edit','edit':{'op':'roll','left_ids':['left'], 'links':'include',
                                  'shift':{'backward':False,'amount':time(F(5)/rate)}}}])
    compare(edited,'slip-roll')
    processed = deepcopy(tracked)
    for track in processed['tracks']['tracks']:
        if track['kind'] == 'audio':
            for clip in track['clips']:
                clip.update(gain_milli=800, fade_in=time(F(5)/rate), fade_out=time(F(5)/rate))
    compare(processed,'gain-fades')
    selected = dict(start=time(F(5)/rate),duration=time(F(10)/rate))
    compare(tracked,'preview-range','preview.range',**selected)
    compare(tracked,'reference-range','export.run',profile='reference',streams='audio_video',range=selected)
    compare(project,'audio-range','export.run',profile='reference',streams='audio',range=selected)
    compare(tracked,'h264-range','export.run',profile='h264_aac',streams='audio_video',
            input_transfer='bt709',range=selected)
    if case['name'] == 'h264-25':
        gaps = deepcopy(project)
        gaps['clips'].insert(1,dict(id='gap',gap=True,source_in=time(0),duration=time(F(5)/rate)))
        compare(gaps,'sequential-gap')
        # Audio transitions need both adjacent source handles, independently of picture cuts.
        transitioned = deepcopy(tracked)
        for track in transitioned['tracks']['tracks']:
            left, right = track['clips']
            track['transitions'] = [dict(id=track['id']+'-mix',left_id=left['id'],right_id=right['id'],
                                         before=time(F(5)/rate),after=time(F(5)/rate),kind='dissolve')]
        compare(transitioned,'picture-and-audio-transition')
        queue = root/'direct-av-jobs'
        queue.mkdir()
        args = dict(project=tracked,input_root=str(root),output_root=str(out),
                    output=str(out/'direct-av-queued.mp4'),profile='h264_aac',streams='audio_video',
                    input_transfer='bt709',range=selected)
        request = dict(job_root=str(queue),request_id='direct-av',run='export.run',arguments=args)
        ticket = client.call('job.start',**request)
        assert client.call('job.start',**request)['job_id'] == ticket['job_id']
        result = client.call('job.wait',job_root=str(queue),job_ids=[ticket['job_id']],timeout_seconds=120)
        assert result['finished'] and result['counts'] == {'completed':1}, result
        for video in [True,False]:
            assert decoded(Path(args['output']),video) == decoded(out/'h264-25-av-h264-range-reference.mp4',video)
        comparisons.append(dict(case='queued-h264-range-with-retry',identical=True))
    assert identity(root/source['path']) == source['identity']
    return dict(comparisons=comparisons, source_preserved=True,
                reference='Full explicit conform, decoded RGB and PCM equality for each output')
