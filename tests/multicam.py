"""Declared multicam sync: independent camera/time signals and retained decision history."""
import argparse
from array import array
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time as clock
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from tracks import time,seconds,edit,placement,track
from sequences import nested,populated,arrangement,seqedit
ROOT=Path(__file__).resolve().parents[1]
W,H,N=16,12,48
NAMES=('wide','close','side')
PREFIX=(0,3,5)
APREFIX=(0,3*1920+13,5*1920+17)
def change(op,**fields):return {'op':'multicam.edit','id':'program','edit':{'op':op,**fields}}
def group_of(p):return next(s for s in p['sequences'] if s['id']=='program')['multicam']
def cut(id,at,angle):return {'id':id,'at':time(at,25),'angle_id':angle}

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store,queue=[root/n for n in ('sources','output','store','queue')]
    for p in (sources,out,store,queue):p.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';passed=[];rejected=0;frames=0;samples=0;previews=0;cases=[]
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode or not r['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert p.returncode==0 and r['ok'],r;return r['result']
    def apply(p,ops,error=None):return call({'command':'timeline.apply','project':p,'expected_revision':p['revision'],'operations':ops},error)
    def validate(p,error=None):return call({'command':'project.validate','project':p},error)
    pictures=[];sounds=[];assets=[]
    for s in range(3):
        rgb=[bytes(v for y in range(H) for x in range(W) for v in (((n-PREFIX[s])*7+x*3+y)%256,(s*83+y*17)%256,((n-PREFIX[s])*11+x*5+y*3+s*37)%256)) for n in range(N)]
        pcm=array('h',(v for n in range(N*1920) for v in ((((n-APREFIX[s])*37)%28000)-14000+s*701,(((n-APREFIX[s])*71)%30000)-15000+s*431)))
        pictures.append(rgb);sounds.append(pcm);raw=sources/f'{s}.rgb';audio=sources/f'{s}.pcm';movie=sources/f'{s}.mkv';raw.write_bytes(b''.join(rgb));audio.write_bytes(pcm.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        assets.append({'id':str(s),'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def preserved():assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    p=call({'command':'project.create','id':'multicam-test','width':W,'height':H,'frame_rate':time(25)})
    ops=[{'op':'media.add','asset':a} for a in assets]
    for i in range(3):
        id='cam'+str(i);ops.append({'op':'sequence.create','id':id,'duration':time(N,25)})
        for kind in ('video','audio'):ops += [seqedit(id,edit('add',track=track(kind,kind))),seqedit(id,edit('place',track_id=kind,clip=placement(kind,i,0,N),collision='reject'))]
    sources_project=apply(p,ops)
    group={'duration':time(30,25),'locked':False,'angles':[{'id':name,'sequence_id':'cam'+str(i),'start':time(4 if i==1 else 0,25),'duration':time(24 if i==1 else 30,25),'source_in':time(PREFIX[i]+(4 if i==1 else 0),25),'audio_in':time(APREFIX[i]+(4*1920 if i==1 else 0),48000)} for i,name in enumerate(NAMES)],'cuts':[cut('last',25,'wide'),cut('first',0,'wide'),cut('middle',18,'side'),cut('early',8,'close')],'audio':{'mode':'fixed','angle_id':'wide'}}
    base=apply(sources_project,[{'op':'multicam.create','id':'program','group':group},edit('create',duration=time(30,25)),edit('add',track=track('v','video')),edit('add',track=track('a','audio')),edit('place',track_id='v',clip=nested('program-video','program',0,30),collision='reject'),edit('place',track_id='a',clip=nested('program-audio','program',0,30),collision='reject')])
    # Expected values come from retained camera decisions and independent physical source buffers,
    # never from the engine's generated native arrangement.
    def reference(p,scale=1):
        g=group_of(p);angles={a['id']:a for a in g['angles']};cuts=sorted(g['cuts'],key=lambda c:seconds(c['at']));count=int(seconds(g['duration'])*25);rgb=[];pcm=array('h')
        def selected(n):return next(c['angle_id'] for c in reversed(cuts) if int(seconds(c['at'])*25)<=n)
        for n in range(count):
            a=angles[selected(n)];camera=int(a['sequence_id'][3:]);source_frame=n-int(seconds(a['start'])*25)+int(seconds(a['source_in'])*25);raw=pictures[camera][source_frame]
            rgb.append(bytes(raw[(y*scale*W+x*scale)*3+c] for y in range(H//scale) for x in range(W//scale) for c in range(3)))
            policy=g['audio'];name=selected(n) if policy['mode']=='follow_video' else policy.get('angle_id')
            if policy['mode']=='mute':pcm.extend([0]*(1920*2));continue
            a=angles[name];camera=int(a['sequence_id'][3:]);first=n*1920-int(seconds(a['start'])*48000)+int(seconds(a['audio_in'])*48000);pcm.extend(sounds[camera][first*2:(first+1920)*2])
        return b''.join(rgb),pcm.tobytes()
    def compare(path,expected,label,scale=1):
        nonlocal frames,samples
        rgb,pcm=expected;actual=ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-']);assert actual==rgb,(label,'video',len(actual),len(rgb),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,rgb)) if a!=b),None))
        actual=ff(['-i',str(path),'-vn','-f','s16le','-']);assert actual==pcm,(label,'audio',len(actual),len(pcm),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,pcm)) if a!=b),None))
        frames+=len(rgb)//((W//scale)*(H//scale)*3);samples+=len(pcm)//4;cases.append(label);preserved()
    def render(p,label,expected=None):
        path=out/(label+'.mkv');call({'command':'render.run','project':p,'input_root':str(root),'output_root':str(out),'output':str(path)});expected=reference(p) if expected is None else expected;compare(path,expected,label);return expected
    base_output=render(base,'declared-sync-fixed-audio');b=W*H*3
    assert all(base_output[0][n*b]==(n*7)%256 for n in range(30))
    for n in range(30):assert base_output[0][n*b+1]==(0 if n<8 or n>=25 else (83 if n<18 else 166))
    assert base_output[1]==sounds[0][:30*1920*2].tobytes()
    ordered=copy.deepcopy(group);ordered['cuts'].sort(key=lambda c:seconds(c['at']));other=apply(sources_project,[{'op':'multicam.create','id':'program','group':ordered}]);assert other['sequences'][-1]['arrangement']==base['sequences'][-1]['arrangement']
    reused=copy.deepcopy(base);reused['tracks']['duration']=time(65,25)
    for t in reused['tracks']['tracks']:t['clips'].append(nested('again-'+t['id'],'program',35,30))
    render(reused,'repeated-camera-edit',(base_output[0]+bytes(5*b)+base_output[0],base_output[1]+bytes(5*1920*4)+base_output[1]));passed.append('multicam.declared_sync_camera_switches')
    followed=apply(base,[change('audio',policy={'mode':'follow_video'})]);followed_output=render(followed,'sample-aligned-follow-audio')
    audio_values=array('h',followed_output[1]);assert all(audio_values[n*2]-((n*37)%28000-14000) in (0,701,1402) for n in range(30*1920))
    muted=apply(base,[change('audio',policy={'mode':'mute'})]);assert render(muted,'muted-camera-edit')[1]==bytes(30*1920*4)
    side_audio=apply(base,[change('audio',policy={'mode':'fixed','angle_id':'side'})]);render(side_audio,'alternate-master-audio')
    shifted=copy.deepcopy(group['angles'][2]);shifted['audio_in']=time(APREFIX[2]+1,48000);render(apply(side_audio,[change('angle_set',angle=shifted)]),'one-sample-audio-adjustment');passed.append('multicam.audio_policies_sample_alignment')
    switched=apply(base,[change('cut_set',cut=cut('early',8,'side'))]);switched_output=render(switched,'recoverable-angle-switch');assert switched_output[1]==base_output[1] and switched_output[0]!=base_output[0];assert group_of(switched)['angles']==group['angles']
    moved=apply(base,[change('cut_set',cut=cut('middle',12,'side'))]);render(moved,'moved-camera-cut')
    removed=apply(base,[change('cut_remove',id='early')]);render(removed,'removed-camera-cut');assert group_of(removed)['angles']==group['angles']
    recovered=apply(removed,[change('cut_set',cut=cut('early',8,'close'))]);assert render(recovered,'recovered-alternate-angle')==base_output
    trimmed=apply(removed,[change('angle_remove',id='close')]);render(trimmed,'explicit-unused-angle-removal');assert len(group_of(trimmed)['angles'])==2
    extra=copy.deepcopy(group['angles'][0]);extra['id']='extra';added=apply(base,[change('angle_set',angle=extra)]);assert len(group_of(added)['angles'])==4;render(added,'added-unused-angle')
    apply(base,[change('duration',duration=time(29,25)),edit('duration',duration=time(29,25))],'INVALID_RANGE')
    # Parent references must be trimmed before shrinking their shared group.
    short=copy.deepcopy(base)
    for t in short['tracks']['tracks']:t['clips'][0]['duration']=time(29,25)
    short['tracks']['duration']=time(29,25);short=apply(short,[change('duration',duration=time(29,25))]);render(short,'shortened-camera-group')
    locked=apply(base,[change('state',locked=True)]);apply(locked,[change('cut_set',cut=cut('early',8,'side'))],'TRACK_LOCKED');unlocked=apply(locked,[change('state',locked=False)]);assert render(unlocked,'explicit-group-unlock')==base_output
    locked_parent=apply(base,[edit('state',track_id='v',locked=True,enabled=False)]);apply(locked_parent,[change('cut_remove',id='early')],'TRACK_LOCKED')
    one_angle=copy.deepcopy(group);one_angle['cuts']=[cut('first',0,'wide')];one_angle['locked']=True
    unused_locked=apply(sources_project,[{'op':'multicam.create','id':'program','group':one_angle}]);apply(unused_locked,[seqedit('cam2',edit('state',track_id='video',locked=False,enabled=False))],'TRACK_LOCKED')
    apply(unused_locked,[{'op':'sequence.remove','id':'cam2'}],'SEQUENCE_IN_USE')
    unused_cycle=copy.deepcopy(group['angles'][2]);unused_cycle.update(sequence_id='program',source_in=time(0),audio_in=time(0));no_other_cuts=apply(base,[change('cut_remove',id='early'),change('cut_remove',id='middle'),change('cut_remove',id='last')]);apply(no_other_cuts,[change('angle_set',angle=unused_cycle)],'SEQUENCE_CYCLE')
    apply(base,[seqedit('program',edit('state',track_id='multicam-video',locked=False,enabled=False))],'MANAGED_SEQUENCE')
    forged=copy.deepcopy(base);forged['sequences'][-1]['arrangement']['tracks'][0]['clips'][0]['source_in']=time(1,25);validate(forged,'MULTICAM_PROJECTION_MISMATCH')
    failures=[(change('cut_set',cut=cut('early',2,'close')),'ANGLE_COVERAGE'),(change('cut_set',cut=cut('early',29,'close')),'ANGLE_COVERAGE'),(change('cut_set',cut=cut('early',18,'close')),'INVALID_MULTICAM'),(change('cut_set',cut=cut('new',30,'wide')),'INVALID_MULTICAM'),(change('cut_set',cut=cut('early',8,'missing')),'MISSING_ANGLE'),(change('cut_remove',id='first'),'INVALID_MULTICAM'),(change('cut_remove',id='missing'),'MISSING_CUT'),(change('angle_remove',id='wide'),'ANGLE_IN_USE'),(change('angle_remove',id='missing'),'MISSING_ANGLE'),(change('audio',policy={'mode':'fixed','angle_id':'close'}),'ANGLE_COVERAGE'),(change('duration',duration=time(0)),'INVALID_MULTICAM')]
    for op,code in failures:apply(base,[op],code)
    bad=copy.deepcopy(group['angles'][0]);bad['audio_in']=time(1,96000);apply(base,[change('angle_set',angle=bad)],'UNALIGNED_TIME')
    bad=copy.deepcopy(group['angles'][0]);bad['source_in']=time(30,25);apply(base,[change('angle_set',angle=bad)],'INVALID_RANGE')
    bad=copy.deepcopy(group['angles'][0]);bad['sequence_id']='missing';apply(base,[change('angle_set',angle=bad)],'MISSING_SEQUENCE')
    many=copy.deepcopy(group);many['angles']=[{**group['angles'][0],'id':str(i)} for i in range(17)];apply(sources_project,[{'op':'multicam.create','id':'many','group':many}],'INVALID_MULTICAM')
    many=copy.deepcopy(group);many['cuts']=[cut(str(i),i,'wide') for i in range(129)];apply(sources_project,[{'op':'multicam.create','id':'many','group':many}],'INVALID_MULTICAM')
    duplicate=copy.deepcopy(group);duplicate['angles'][1]['id']='wide';apply(sources_project,[{'op':'multicam.create','id':'duplicate','group':duplicate}],'DUPLICATE_ID')
    duplicate=copy.deepcopy(group);duplicate['cuts'][1]['id']=duplicate['cuts'][0]['id'];apply(sources_project,[{'op':'multicam.create','id':'duplicate','group':duplicate}],'DUPLICATE_ID')
    passed.append('multicam.dependencies_locks_projection_limits')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65;schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        client.call('session.create',store_root=str(store),project=base,request_id='create');common={'store_root':str(store),'project_id':base['id']};fields={**common,'expected_revision':0,'request_id':'switch','operations':[change('cut_set',cut=cut('early',8,'side'))]};Draft202012Validator(schema).validate(fields)
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});delta=preview['sequences'][0];assert delta['id']=='program' and delta['before']['multicam']['angles']==delta['after']['multicam']['angles'] and set(delta['root_instances'])=={'program-video','program-audio'}
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt;saved=client.call('session.get',**common);history=client.call('session.history',**common)
        client.call('session.apply','MISSING_ANGLE',**common,expected_revision=1,request_id='bad',operations=[change('audio',policy={'mode':'mute'}),change('cut_set',cut=cut('early',8,'missing'))]);assert client.call('session.history',**common)==history and client.call('session.get',**common)==saved
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['sequences']==base['sequences']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1);render(client.call('session.get',**common),'saved-camera-restore')
        create_fields={**common,'expected_revision':3,'request_id':'create-copy','operations':[{'op':'multicam.create','id':'unused-copy','group':group}]};Draft202012Validator(schema).validate(create_fields);client.call('session.apply',**create_fields)
        client.call('session.apply',**common,expected_revision=4,request_id='remove-copy',operations=[{'op':'sequence.remove','id':'unused-copy'}]);passed.append('multicam.recoverable_cuts_angles_history')
        for n in [0,7,8,9,17,18,19,24,25,29]:
            path=out/f'preview-{n}.png';client.call('preview.frame',project=base,input_root=str(root),output_root=str(out),output=str(path),time=time(n,25))
            with Image.open(path) as image:assert image.tobytes()==base_output[0][n*b:(n+1)*b]
            previews+=1
        path=out/'range.mkv';call({'command':'preview.range','project':followed,'input_root':str(root),'output_root':str(out),'output':str(path),'start':time(7,25),'duration':time(19,25)});compare(path,(followed_output[0][7*b:26*b],followed_output[1][7*1920*4:26*1920*4]),'range-across-camera-cuts')
        path=out/'export.mkv';call({'command':'export.run','project':switched,'input_root':str(root),'output_root':str(out),'output':str(path),'profile':'reference','streams':'audio_video','range':{'start':time(8,25),'duration':time(10,25)}});compare(path,(switched_output[0][8*b:18*b],switched_output[1][8*1920*4:18*1920*4]),'changed-angle-reference-export')
        proxied=copy.deepcopy(base)
        for asset in assets:proxied=call({'command':'proxy.generate','project':proxied,'expected_revision':proxied['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(root),'output_root':str(out),'output':str(out/f'proxy-{asset["id"]}.mkv')})['project']
        proxied=apply(proxied,[{'op':'preview.proxy','scale':2}]);small=reference(base,2);path=out/'proxy.png';client.call('preview.frame',project=proxied,input_root=str(root),output_root=str(out),output=str(path),time=time(8,25))
        with Image.open(path) as image:assert image.size==(W//2,H//2) and image.tobytes()==small[0][8*b//4:9*b//4]
        previews+=1;ticket=client.call('render.start',job_root=str(queue),request_id='multicam-job',render={'project':proxied,'input_root':str(root),'output_root':str(out),'output':str(out/'queued.mkv')});deadline=clock.monotonic()+90
        while True:
            state=client.call('job.status',job_root=str(queue),job_id=ticket['job_id'])
            if state['status'] in ('completed','failed','cancelled','interrupted'):break
            assert clock.monotonic()<deadline;clock.sleep(.05)
        assert state['status']=='completed',state;compare(out/'queued.mkv',base_output,'queued-original-camera-output');passed.append('multicam.previews_proxies_ranges_queue')
    finally:client.close()
    request={'command':'render.run','project':base,'input_root':str(root),'output_root':str(out),'output':str(out/'declared-sync-fixed-audio.mkv')};call(request,'OUTPUT_EXISTS')
    bad=copy.deepcopy(base);bad['assets'][1]['identity']['sha256']='0'*64;call({**request,'project':bad,'output':str(out/'identity.mkv')},'IDENTITY_MISMATCH')
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with(".partial.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],capture_output=True,check=True)
    source=sources/'1.mkv';saved=source.read_bytes()
    try:call({**request,'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (out/'changed.mkv').exists() and not list(out.glob('*.partial.mkv'));preserved();passed.append('multicam.source_identity_preservation')
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'frame_previews_compared':previews,'render_cases':cases,'rejected_cases':rejected,'reference':'Original camera identity and shared-event-time pixels plus signed stereo signals with declared frame/sample prefixes. Reference uses retained camera choices and original buffers, never the generated native projection. Zero tolerance.','limits':'Declared 25 fps camera synchronization, explicit sample-based audio mapping, fixed/follow/mute audio and recoverable cuts. The separate synchronization fixture verifies audio/timecode alignment and constant clock-drift correction for T10 extended.'}
    (root/'project.json').write_text(json.dumps(base,indent=2)+'\n');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
