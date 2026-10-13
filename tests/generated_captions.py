"""Generated caption edits: exact old-overlay equality and saved/job boundaries."""
import copy
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time as clock

from tracks import edit, track, time


def run(root, engine, sources, document, layouts, timeline, reference_engine=None):
    root=Path(root);root.mkdir()
    reference_engine=reference_engine or engine
    digest=lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest()
    original={p.name:digest(p) for p in sources.iterdir()}
    passed=[];compared=0;samples=0
    layouts=copy.deepcopy(layouts)
    for layout in layouts.values():
        for font in layout['fonts']:font['path']=(sources/font['path']).relative_to(root.parent).as_posix()

    def call(command, error=None, exe=engine, workspace=None, env=None, **fields):
        argv=[str(exe)]+(['--workspace',str(workspace)] if workspace else [])
        result=subprocess.run(argv,input=json.dumps(dict(command=command,**fields)).encode(),capture_output=True,timeout=180,env=env)
        value=json.loads(result.stdout)
        assert result.returncode==(1 if error else 0),(value,result.stderr)
        if error:
            assert value['error']['code']==error,value
            return value['error']
        assert value['ok'],value
        return value['result']

    def decode(project,name):
        output=root/(name+'.mkv')
        call('render.run',project=project,input_root=str(root.parent),output_root=str(root),output=str(output))
        video=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(output),'-an','-f','rawvideo','-pix_fmt','rgb24','-'])
        audio=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(output),'-vn','-f','s16le','-c:a','pcm_s16le','-'])
        return video,audio

    def generate(project,name,**fields):
        output=root/(name+'.json')
        result=call('captions.render',mode='generated',document=document,layouts=layouts,project=project,
            input_root=str(root.parent),output_root=str(root),output=str(output),**fields)
        assert result['compiled'] is False and result['sha256']==digest(output)
        assert result['bytes']==output.stat().st_size and len(json.dumps(result))<2500,result
        assert result['operations']['select']=='operations'
        assert Path(result['operations']['file']).samefile(output)
        artifact=json.loads(output.read_text());ops=artifact['operations']
        assert result['expected_revision']==artifact['expected_revision']==project['revision']
        assert not any(op['op']=='media.add' for op in ops)
        return result,artifact,call('timeline.apply',project=project,expected_revision=project['revision'],operations=ops)

    cases=[('whole',timeline(time(30)),time(0),time(30),4),
        ('offset',timeline(time(30)),time(24),time(3),1),
        ('ntsc',timeline(time(305*1001,30000),time(30000,1001)),time(0),time(305*1001,30000),2)]
    for name,project,start,duration,windows in cases:
        common=dict(document=document,layouts=layouts,project=project,input_root=str(root.parent),
            output_root=str(root),start=start,duration=duration)
        old=call('captions.render',exe=reference_engine,output=str(root/(name+'-old.mkv')),**common)
        result,artifact,native=generate(project,name,start=start,duration=duration)
        assert result['windows']==old['windows']==windows,(name,result,old)
        assert result['duration']==duration and result['frames']==int(F(duration['num'],duration['den'])*F(project['frame_rate']['num'],project['frame_rate']['den']))
        placed=[op['edit']['clip'] for op in artifact['operations'] if op.get('edit',{}).get('op')=='place']
        assert sum((F(c['duration']['num'],c['duration']['den']) for c in placed),F(0))==F(duration['num'],duration['den'])
        if name=='ntsc':assert placed[-1]['duration']!=placed[-1]['scene']['duration']
        reference_track=track('old-overlay','video');reference_track['composite']='alpha_over'
        legacy=call('timeline.apply',project=project,expected_revision=project['revision'],operations=[
            dict(op='media.add',asset=old['asset']),edit('promote',video_track_id='v',audio_track_id='a'),
            edit('add',track=reference_track),edit('place',track_id='old-overlay',collision='reject',
                clip=dict(id='old-overlay',asset_id=old['asset']['id'],start=start,source_in=time(0),duration=duration))])
        actual=decode(native,name+'-native');expected=decode(legacy,name+'-legacy')
        assert actual==expected,name
        compared+=len(actual[0])//(96*64*3);samples+=len(actual[1])//4
    passed.append('captions.generated_windows_equal_compiled_pixels_pcm_and_exact_ntsc_trim')

    project=timeline(time(30));result,artifact,native=generate(project,'session',track_id='subtitles')
    store=root/'store';store.mkdir()
    call('session.create',store_root=str(store),project=project,request_id='create')
    mutation=dict(store_root=str(store),project_id=project['id'],request_id='captions',expected_revision=0,
        operations=result['operations'])
    # Font paths resolve against the declared media root; the edit document is under this workspace.
    saved=call('session.apply',workspace=root.parent,**mutation)
    assert saved['revision']==1
    assert call('session.apply',workspace=root.parent,**mutation)==saved
    head=call('session.get',store_root=str(store),project_id=project['id'])
    assert head['schema_version']==2 and head['tracks']==native['tracks']
    stale={**mutation,'request_id':'stale'}
    call('session.apply',workspace=root.parent,error='REVISION_CONFLICT',**stale)
    call('session.undo',store_root=str(store),project_id=project['id'],request_id='undo',expected_revision=1)
    assert call('session.apply',workspace=root.parent,**mutation)==saved
    assert call('session.check',store_root=str(store))
    # The generated file cannot be replaced, and invalid proposals never publish.
    common=dict(mode='generated',document=document,layouts=layouts,project=project,input_root=str(root.parent),output_root=str(root))
    call('captions.render',error='OUTPUT_EXISTS',output=result['output'],**common)
    call('captions.render',error='PATH_OUTSIDE_ROOT',output=str(root.parent/'escaped.json'),**common)
    call('captions.render',error='UNSUPPORTED_OUTPUT',output=str(root/'wrong.mkv'),**common)
    locked=copy.deepcopy(native);next(t for t in locked['tracks']['tracks'] if t['id']=='subtitles')['locked']=True
    call('captions.render',error='TRACK_LOCKED',output=str(root/'locked.json'),track_id='subtitles',**{**common,'project':locked})
    call('captions.render',error='CLIP_COLLISION',output=str(root/'collision.json'),track_id='subtitles',asset_id='again',**{**common,'project':native})
    call('captions.render',error='INVALID_RANGE',output=str(root/'overlong.json'),duration=time(31),**common)
    for name in ['locked.json','collision.json','overlong.json','wrong.mkv']:assert not (root/name).exists()
    passed.append('captions.generated_proposals_validate_before_publication_and_save_atomically')

    queue=root/'jobs';queue.mkdir()
    request=dict(job_root=str(queue),request_id='generated',run='captions.render',
        arguments={**common,'output':str(root/'queued.json')})
    ticket=call('job.start',**request)
    waited=call('job.wait',job_root=str(queue),job_id=ticket['job_id'],timeout_seconds=120)
    assert waited['status']=='completed',waited
    assert call('job.start',**request)==ticket
    assert waited['result']['compiled'] is False and (root/'queued.json').is_file()
    # A receipt genuinely created by the previous engine must replay with omitted new fields.
    old_arguments={k:v for k,v in common.items() if k!='mode'}
    old_queue=root/'old-jobs';old_queue.mkdir()
    old_request=dict(job_root=str(old_queue),request_id='compiled',run='captions.render',
        arguments={**old_arguments,'duration':time(1),'output':str(root/'queued-old.mkv')})
    old_ticket=call('job.start',exe=reference_engine,**old_request)
    old_wait=call('job.wait',exe=reference_engine,job_root=str(old_queue),job_id=old_ticket['job_id'],timeout_seconds=120)
    assert old_wait['status']=='completed',old_wait
    assert call('job.start',**old_request)==old_ticket
    new_wait=call('job.wait',job_root=str(old_queue),job_id=old_ticket['job_id'],timeout_seconds=120)
    assert new_wait['result']==old_wait['result'] and 'asset' in new_wait['result']
    passed.append('captions.generated_jobs_and_actual_compiled_job_receipt_replay')

    no_codecs={**os.environ,'CUTBOLT_FFMPEG':str(root/'no-ffmpeg'),'CUTBOLT_FFPROBE':str(root/'no-ffprobe')}
    call('captions.render',env=no_codecs,output=str(root/'without-codecs.json'),**common)
    assert (root/'without-codecs.json').is_file()
    long_project=timeline(time(3600));long_project['width']=1920;long_project['height']=1080
    long_document=copy.deepcopy(document)
    long_document['cues']=[{**document['cues'][0],'id':f'cue-{n}','start':time(n*2),'end':time(n*2+1)} for n in range(1800)]
    long_arguments={**common,'document':long_document,'project':long_project}
    # Wait for progress emitted from inside scene preparation, so these exercise a running
    # command rather than cancelling an unstarted queue entry.
    def preparing(name):
        args={**long_arguments,'output':str(root/(name+'.json'))}
        ticket=call('job.start',job_root=str(queue),request_id=name,run='captions.render',arguments=args)
        deadline=clock.monotonic()+60
        while True:
            state=call('job.status',job_root=str(queue),job_id=ticket['job_id'])
            if state['status']=='running' and state['progress']['phase']=='caption-scenes':break
            assert state['status'] in ['queued','running'] and clock.monotonic()<deadline,state
            clock.sleep(0.02)
        return ticket
    ticket=preparing('cancel')
    call('job.start',error='OUTPUT_RESERVED',job_root=str(queue),request_id='same-output',run='captions.render',
        arguments={**long_arguments,'output':str(root/'cancel.json')})
    call('job.cancel',job_root=str(queue),job_id=ticket['job_id'])
    stopped=call('job.wait',job_root=str(queue),job_id=ticket['job_id'],timeout_seconds=120)
    assert stopped['status']=='cancelled' and not (root/'cancel.json').exists(),stopped
    assert not list(root.glob('.cutbolt-captions-*'))
    ticket=preparing('changed-source')
    font=sources/'primary.ttf';font_bytes=font.read_bytes()
    try:
        font.write_bytes(font_bytes[:-1]+bytes([font_bytes[-1]^1]))
        changed=call('job.wait',job_root=str(queue),job_id=ticket['job_id'],timeout_seconds=120)
        assert changed['status']=='failed' and changed['error']['code']=='MEDIA_CHANGED',changed
        assert not (root/'changed-source.json').exists()
    finally:font.write_bytes(font_bytes)
    assert not list(root.glob('.cutbolt-captions-*'))
    passed.append('captions.generated_no_codec_dependency_running_cancellation_and_changed_source_rejection')

    assert original=={p.name:digest(p) for p in sources.iterdir()}
    assert not list(root.glob('.cutbolt-captions-*'))
    report=dict(passed=passed,frames_compared=compared,stereo_sample_frames_compared=samples,
        source_preservation=True,reference_engine_sha256=digest(reference_engine),
        actual_older_engine=Path(reference_engine).resolve()!=Path(engine).resolve())
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    return report
