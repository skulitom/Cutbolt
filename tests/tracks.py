"""Original placed-track fixtures with independent frame and integer PCM references."""
from engine import ENGINE, MCP_TOOLS
import argparse
from array import array
import copy
from fractions import Fraction as F
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

ROOT=Path(__file__).resolve().parents[1]
W,H,N=32,24,24
def time(n,d=1):
    value=F(n,d);return {'num':value.numerator,'den':value.denominator}
def seconds(value):return F(value['num'],value['den'])
def edit(op,**fields):return {'op':'tracks.edit','edit':{'op':op,**fields}}
def placement(name,source,start,count,source_in=0,clock_=25):
    return {'id':name,'asset_id':str(source),'start':time(start,clock_),'duration':time(count,clock_),'source_in':time(source_in,clock_)}
def track(name,kind):return {'id':name,'kind':kind,'enabled':True,'locked':False,'clips':[]}
def move(names,amount=0,backward=False,targets=(),links='include',collision='reject'):
    return edit('move',clip_ids=names,shift={'backward':backward,'amount':time(amount,25)},targets=[{'clip_id':c,'track_id':t} for c,t in targets],links=links,collision=collision)

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store,queue=[root/name for name in ('sources','output','store','queue')]
    for path in (sources,out,store,queue):path.mkdir()
    exe=ENGINE;passed=[];cases=[];rejected=0;frames_checked=0;samples_checked=0
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],check=True,capture_output=True,timeout=120).stdout
    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);value=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and value['error']['code']==error,(error,value);rejected+=1;return value
        if p.returncode or not value['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert p.returncode==0 and value['ok'],value;return value['result']
    def frame(s,n):return bytes(v for y in range(H) for x in range(W) for v in ((n*17+x*3+s*97)%256,(y*11+n*5+s*61)%256,(x+y+n*19+s*29)%256))
    audio=[];assets=[]
    for s in range(3):
        raw=sources/f'{s}.rgb';pcm=sources/f'{s}.pcm';movie=sources/f'{s}.mkv'
        raw.write_bytes(b''.join(frame(s,n) for n in range(N)))
        sound=array('h',(v for n in range(N*1920) for v in (((n*73+s*5003)%60000)-30000,((n*29+s*9001)%64000)-32000)));audio.append(sound);pcm.write_bytes(sound.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar','48000','-ac','2','-i',str(pcm),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        assert ff(['-i',str(movie),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==raw.read_bytes()
        assets.append({'id':str(s),'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def preserved():assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    base=call({'command':'project.create','id':'tracks-fixture','width':W,'height':H,'frame_rate':time(25)})
    operations=[{'op':'media.add','asset':a} for a in assets]+[edit('create',duration=time(20,25))]
    operations += [edit('add',track=track(name,kind)) for name,kind in [('bottom','video'),('top','video'),('dialogue','audio'),('upper-audio','audio'),('extra-audio','audio')]]
    placements=[('bottom',placement('low',0,0,8,2)),('bottom',placement('late',0,10,6,10)),('top',placement('upper',1,3,6,4)),('top',placement('upper-late',1,14,4,0)),
                ('dialogue',placement('low-a',0,0,8,2)),('dialogue',placement('late-a',0,10*1920+7,6*1920-7,10*1920+7,48000)),
                ('upper-audio',placement('upper-a',1,3,6,4)),('upper-audio',placement('upper-late-a',1,14,4,0)),
                ('extra-audio',placement('sample-offset',2,2*1920+13,5001,7,48000)),('extra-audio',placement('last-samples',2,20*1920-11,11,111,48000))]
    operations += [edit('place',track_id=t,clip=c,collision='reject') for t,c in placements]
    operations += [edit('link',id=name,clip_ids=ids) for name,ids in [('low-link',['low','low-a']),('late-link',['late','late-a']),('upper-link',['upper','upper-a'])]]
    base=call({'command':'timeline.apply','project':base,'expected_revision':0,'operations':operations})
    def apply(project,ops,error=None):return call({'command':'timeline.apply','project':project,'expected_revision':project['revision'],'operations':ops},error)
    def positions(project):return {c['id']:(t['id'],seconds(c['start']),seconds(c['source_in']),seconds(c['duration'])) for t in project['tracks']['tracks'] for c in t['clips']}
    def oracle(project):
        arrangement=project['tracks'];count=int(seconds(arrangement['duration'])*25);rgb=[];pcm=[0]*(count*1920*2)
        for n in range(count):
            image=bytes(W*H*3);t=F(n,25)
            for layer in arrangement['tracks']:
                if layer['kind']!='video' or not layer['enabled']:continue
                for c in layer['clips']:
                    if seconds(c['start'])<=t<seconds(c['start'])+seconds(c['duration']):image=frame(int(c['asset_id']),int((t-seconds(c['start'])+seconds(c['source_in']))*25))
            rgb.append(image)
        for layer in arrangement['tracks']:
            if layer['kind']!='audio' or not layer['enabled']:continue
            for c in layer['clips']:
                start=int(seconds(c['start'])*48000);source=int(seconds(c['source_in'])*48000);length=int(seconds(c['duration'])*48000)
                for n in range(length*2):pcm[start*2+n]+=audio[int(c['asset_id'])][source*2+n]
        return b''.join(rgb),array('h',(max(-32768,min(32767,x)) for x in pcm)).tobytes()
    def compare(path,rgb,pcm,label):
        nonlocal frames_checked,samples_checked
        assert ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==rgb,label
        assert ff(['-i',str(path),'-vn','-f','s16le','-'])==pcm,label
        frames_checked+=len(rgb)//(W*H*3);samples_checked+=len(pcm)//4;cases.append(label);preserved()
    def render(project,label):
        path=out/(label+'.mkv');r=call({'command':'render.run','project':project,'input_root':str(root),'output_root':str(out),'output':str(path)})
        rgb,pcm=oracle(project);compare(path,rgb,pcm,label);assert r['frames']==len(rgb)//(W*H*3);return rgb,pcm
    original_rgb,original_pcm=render(base,'base')
    assert len(set(array('h',original_pcm)))>1000 and any(v==32767 for v in array('h',original_pcm)) and any(v==-32768 for v in array('h',original_pcm))
    empty=apply(base,[edit('remove',clip_ids=list(positions(base)),links='include')]);assert not positions(empty);render(empty,'empty-gaps')
    passed.append('tracks.native_composition_audio_gaps')

    moved=apply(base,[edit('add',track=track('new-video','video')),edit('add',track=track('new-audio','audio')),move(['low'],1,targets=[('low','new-video'),('low-a','new-audio')])])
    want=positions(base);want['low']=('new-video',F(1,25),F(2,25),F(8,25));want['low-a']=('new-audio',F(1,25),F(2,25),F(8,25));assert positions(moved)==want;render(moved,'linked-move-targets')
    swapped=apply(base,[move(['low','upper'],targets=[('low','top'),('upper','bottom')])]);want=positions(base);want['low']=('top',*want['low'][1:]);want['upper']=('bottom',*want['upper'][1:]);assert positions(swapped)==want;render(swapped,'simultaneous-retarget')
    removed=apply(base,[edit('remove',clip_ids=['upper'],links='include')]);assert set(positions(removed))==set(positions(base))-{'upper','upper-a'};render(removed,'linked-remove')
    offset_move=apply(base,[move(['late'],1)]);old=positions(base);new=positions(offset_move)
    assert new['late'][1]-old['late'][1]==new['late-a'][1]-old['late-a'][1]==F(1,25)
    assert new['late-a'][1]-new['late'][1]==F(7,48000);render(offset_move,'linked-sample-offset')
    reordered=apply(base,[edit('order',track_ids=['top','bottom','dialogue','upper-audio','extra-audio'])]);rgb,_=render(reordered,'track-order');assert rgb!=original_rgb
    disabled=apply(base,[edit('state',track_id='top',locked=False,enabled=False),edit('state',track_id='upper-audio',locked=False,enabled=False)]);render(disabled,'disabled-tracks')
    passed.append('tracks.linked_move_remove_targets')

    replacement=placement('replacement',2,2,13,0)
    apply(base,[edit('place',track_id='bottom',clip=replacement,collision='reject')],'CLIP_COLLISION')
    replaced=apply(base,[edit('place',track_id='bottom',clip=replacement,collision='replace_clips')]);assert set(positions(replaced))==set(positions(base))-{'low','low-a','late','late-a'}|{'replacement'};render(replaced,'replace-whole-linked-clips')
    touched=apply(base,[edit('place',track_id='bottom',clip=placement('touching',2,8,2),collision='reject')]);render(touched,'exact-touching-boundaries')
    locked=apply(base,[edit('state',track_id='dialogue',locked=True,enabled=True)])
    for op in [move(['low'],1),edit('remove',clip_ids=['low'],links='include'),edit('unlink',id='low-link'),edit('place',track_id='bottom',clip=replacement,collision='replace_clips'),edit('state',track_id='dialogue',locked=False,enabled=False)]:apply(locked,[op],'TRACK_LOCKED')
    apply(base,[move(['low'],1,links='reject_partial')],'LINKED_SELECTION')
    target_locked=apply(base,[edit('state',track_id='top',locked=True,enabled=True)])
    apply(target_locked,[move(['low'],targets=[('low','top')])],'TRACK_LOCKED')
    apply(target_locked,[edit('order',track_ids=['top','bottom','dialogue','upper-audio','extra-audio'])],'TRACK_LOCKED')
    broken=copy.deepcopy(base);broken['tracks']['tracks'][0]['clips'][0]['start']=time(1,25);call({'command':'project.validate','project':broken},'SYNC_CONFLICT')
    unlinked=apply(base,[edit('unlink',id='low-link'),move(['low'],1,links='reject_partial')]);assert positions(unlinked)['low-a']==positions(base)['low-a'];render(unlinked,'explicit-unlink')
    passed.append('tracks.locks_collisions_sync')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        created=client.call('session.create',store_root=str(store),project=base,request_id='create');assert created['changes']['track_layout']['after']['tracks']
        common={'store_root':str(store),'project_id':base['id']};saved=client.call('session.get',**common);assert saved['tracks']==base['tracks']
        metadata=client.call('session.preview',**common,expected_revision=0,operations=[edit('state',track_id='top',locked=True,enabled=False)])
        assert metadata['clips']==[] and metadata['track_layout']['after']['tracks'][1]['locked'] and not metadata['track_layout']['after']['tracks'][1]['enabled']
        replacement_diff=client.call('session.preview',**common,expected_revision=0,operations=[edit('place',track_id='bottom',clip=replacement,collision='replace_clips')])
        assert {c['clip_id'] for c in replacement_diff['clips']}=={'low','low-a','late','late-a','replacement'}
        ops=[move(['low'],1)];fields={**common,'expected_revision':0,'request_id':'move','operations':ops};Draft202012Validator(schema).validate(fields)
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});assert {c['clip_id'] for c in preview['clips']}=={'low','low-a'}
        assert all(c['after']['timeline_start']==time(1,25) and c['after']['track_id'] in ('bottom','dialogue') for c in preview['clips']);assert client.call('session.get',**common)==saved
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt
        render(client.call('session.get',**common),'saved-move')
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['tracks']==base['tracks']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1);assert positions(client.call('session.get',**common))==positions(apply(base,ops))
        before=client.call('session.get',**common);history=client.call('session.history',**common)
        bad=[edit('add',track=track('must-not-commit','video')),edit('state',track_id='dialogue',locked=True,enabled=True),edit('remove',clip_ids=['low'],links='include')]
        client.call('session.apply','TRACK_LOCKED',**common,expected_revision=3,request_id='failed-batch',operations=bad)
        assert client.call('session.get',**common)==before and client.call('session.history',**common)==history
        assert client.call('capabilities',section='all')['tracks']['profile']=='native-tracks-v1'
        passed.append('tracks.atomic_sessions_history_mcp')

        for n in [0,2,3,7,8,9,10,13,14,17,18,19]:
            path=out/f'preview-{n}.png';r=client.call('preview.frame',project=base,input_root=str(root),output_root=str(out),output=str(path),time=time(n,25))
            with Image.open(path) as image:assert image.tobytes()==original_rgb[n*W*H*3:(n+1)*W*H*3]
            assert r['timeline_frame']==n and r['time']==time(n,25)
        for first,count in [(0,3),(5,10),(18,2)]:
            path=out/f'range-{first}.mkv';call({'command':'preview.range','project':base,'input_root':str(root),'output_root':str(out),'output':str(path),'start':time(first,25),'duration':time(count,25)})
            compare(path,original_rgb[first*W*H*3:(first+count)*W*H*3],original_pcm[first*1920*4:(first+count)*1920*4],f'range-{first}')
        path=out/'delivery.mp4';call({'command':'export.run','project':base,'input_root':str(root),'output_root':str(out),'output':str(path),'range':{'start':time(2,25),'duration':time(15,25)},'profile':'h264_aac','streams':'audio_video','input_transfer':'srgb'})
        path=out/'reference-export.mkv';call({'command':'export.run','project':base,'input_root':str(root),'output_root':str(out),'output':str(path),'profile':'reference','streams':'audio_video'})
        compare(path,original_rgb,original_pcm,'reference-export');passed.append('tracks.previews_ranges_delivery')

        proxied=copy.deepcopy(base)
        for asset in assets:
            r=call({'command':'proxy.generate','project':proxied,'expected_revision':proxied['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(root),'output_root':str(out),'output':str(out/f'proxy-{asset["id"]}.mkv')});proxied=r['project']
        proxied=apply(proxied,[{'op':'preview.proxy','scale':2}]);client.call('proxy.status',project=proxied,input_root=str(root))
        path=out/'proxy-preview.png';client.call('preview.frame',project=proxied,input_root=str(root),output_root=str(out),output=str(path),time=time(4,25))
        expected=original_rgb[4*W*H*3:5*W*H*3];small=b''.join(expected[(y*2*W+x*2)*3:(y*2*W+x*2)*3+3] for y in range(H//2) for x in range(W//2))
        with Image.open(path) as image:assert image.size==(W//2,H//2) and image.tobytes()==small
        render(proxied,'full-quality-with-proxy')
        job=client.call('render.start',job_root=str(queue),request_id='native-tracks',render={'project':proxied,'input_root':str(root),'output_root':str(out),'output':str(out/'queued.mkv')})
        deadline=clock.monotonic()+60
        while True:
            status=client.call('job.status',job_root=str(queue),job_id=job['job_id'])
            if status['status'] in ('completed','failed','interrupted','cancelled'):break
            assert clock.monotonic()<deadline;clock.sleep(.05)
        assert status['status']=='completed',status;compare(out/'queued.mkv',original_rgb,original_pcm,'queued-full-quality');passed.append('tracks.proxies_and_queued_full_quality')
    finally:client.close()

    legacy=call({'command':'project.create','id':'legacy-promote','width':W,'height':H,'frame_rate':time(25)})
    legacy=apply(legacy,[*[{'op':'media.add','asset':a} for a in assets],{'op':'clip.append','clip':{'id':'original','asset_id':'0','source_in':time(2,25),'duration':time(5,25)}},{'op':'clip.append','clip':{'id':'gap','gap':True,'source_in':time(0),'duration':time(3,25)}},{'op':'clip.append','clip':{'id':'promoted-audio-0','asset_id':'1','source_in':time(7,25),'duration':time(4,25)}}])
    promoted=apply(legacy,[edit('promote',video_track_id='v',audio_track_id='a')]);assert promoted['clips']==[] and seconds(promoted['tracks']['duration'])==F(12,25)
    assert len(promoted['tracks']['links'])==2 and len(positions(promoted))==4;rgb,pcm=render(promoted,'promoted')
    assert rgb==b''.join(frame(0,n) for n in range(2,7))+bytes(3*W*H*3)+b''.join(frame(1,n) for n in range(7,11))
    assert pcm==audio[0][2*1920*2:7*1920*2].tobytes()+bytes(3*1920*4)+audio[1][7*1920*2:11*1920*2].tobytes()
    passed.append('tracks.legacy_promotion_preserves_media')

    bad=[(edit('create',duration=time(1)),'INVALID_TRACKS'),(edit('promote',video_track_id='v',audio_track_id='a'),'INVALID_TRACKS'),(edit('add',track=track('top','audio')),'DUPLICATE_ID'),
         (edit('place',track_id='missing',clip=placement('x',0,0,1),collision='reject'),'MISSING_TRACK'),(edit('place',track_id='bottom',clip=placement('low',0,8,1),collision='reject'),'DUPLICATE_ID'),
         (edit('place',track_id='bottom',clip=placement('x',0,19,2),collision='reject'),'INVALID_RANGE'),(edit('place',track_id='bottom',clip=placement('x',0,8,1,24),collision='reject'),'INVALID_RANGE'),
         (edit('place',track_id='bottom',clip=placement('x',0,9*1920+1,1,0,48000),collision='reject'),'UNALIGNED_TIME'),(move(['missing'],1),'MISSING_CLIP'),(move(['low','low'],1),'INVALID_TRACKS'),
         (move(['low'],1,backward=True),'INVALID_RANGE'),(move(['low'],targets=[('low','extra-audio')]),'INVALID_TRACKS'),(move(['low'],targets=[('upper','top')]),'INVALID_TRACKS'),
         (edit('order',track_ids=['bottom']),'INVALID_TRACKS'),(edit('duration',duration=time(1,25)),'INVALID_RANGE'),(edit('link',id='single',clip_ids=['sample-offset']),'INVALID_TRACKS'),
         (edit('link',id='duplicate-member',clip_ids=['low','low-a']),'INVALID_TRACKS'),(edit('unlink',id='missing'),'MISSING_LINK'),({'op':'clip.remove','clip_id':'low'},'UNSUPPORTED_TIMELINE')]
    before=json.dumps(base,sort_keys=True)
    for op,error in bad:apply(base,[op],error)
    assert json.dumps(base,sort_keys=True)==before
    # Maximum simultaneous voices use one exact final PCM saturation, without normalization.
    voices=copy.deepcopy(empty);voices['tracks']['duration']=time(1,25);voices['tracks']['tracks']=[]
    for n in range(32):
        layer=track(f'voice-{n}','audio');layer['clips']=[placement(f'voice-{n}',n%3,0,1,n%20)];voices['tracks']['tracks'].append(layer)
    render(voices,'32-simultaneous-voices')
    too_many=copy.deepcopy(voices);too_many['tracks']['tracks'].append(track('overflow','audio'));call({'command':'project.validate','project':too_many},'LIMIT_EXCEEDED')
    model_limit=copy.deepcopy(empty);model_limit['tracks']['duration']=time(1000,25)
    model_limit['tracks']['tracks'][0]['clips']=[placement(f'clip-{n}',0,n,1) for n in range(1000)]
    call({'command':'project.validate','project':model_limit})
    # More clips than one graph holds: the render is planned as chunks of at most 64 clips and joined exactly.
    model_limit['tracks']['tracks'][0]['clips']=[placement(f'clip-{n}',n%3,n,1,n%20) for n in range(1000)]
    plan=call({'command':'render.plan','project':model_limit,'input_root':str(root),'output_root':str(out),'output':str(out/'thousand-clips-plan.mkv')})
    assert plan['frames']==1000 and len(plan['chunks'])==16 and [c['frames'] for c in plan['chunks']]==[64]*15+[40],plan['chunks']
    render(model_limit,'thousand-clips-chunked')
    model_limit['tracks']['duration']=time(1001,25);model_limit['tracks']['tracks'][0]['clips'].append(placement('overflow',0,1000,1))
    call({'command':'project.validate','project':model_limit},'LIMIT_EXCEEDED')
    malformed=copy.deepcopy(base);malformed['tracks']['links'][0]['members'][0]['start']={'num':0,'den':0}
    call({'command':'project.validate','project':malformed},'INVALID_TIME')
    malformed=copy.deepcopy(base);malformed['tracks']['tracks'][0]['clips'][0]['opacity']=1
    call({'command':'project.validate','project':malformed},'INVALID_JSON')
    req={'command':'render.run','project':base,'input_root':str(root),'output_root':str(out),'output':str(out/'base.mkv')};call(req,'OUTPUT_EXISTS')
    missing=copy.deepcopy(base);missing['assets'][2]['path']=str(sources/'missing.mkv')
    call({**req,'project':missing,'output':str(out/'missing.mkv')},'IO_ERROR')
    disabled_missing=apply(missing,[edit('state',track_id='extra-audio',locked=False,enabled=False)]);render(disabled_missing,'disabled-missing-source')
    broken=copy.deepcopy(base);broken['assets'][0]['identity']['sha256']='0'*64;call({**req,'project':broken,'output':str(out/'bad-identity.mkv')},'IDENTITY_MISMATCH')
    # Only an owned synthetic source is changed; restore it even when the assertion fails.
    tool=root/'changed-source.exe';wrapper=root/'changed-source.rs'
    wrapper.write_text('''use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with(".partial.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}''',encoding='utf-8')
    subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=sources/'0.mkv';saved=source.read_bytes()
    try:call({**req,'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (out/'changed.mkv').exists() and not list(out.glob('*.partial.mkv'));preserved();passed.append('tracks.invalid_preservation_faults')
    report={'passed':passed,'frames_compared':frames_checked,'stereo_sample_frames_compared':samples_checked,'render_cases':cases,'rejected_cases':rejected,
            'reference':'Original per-pixel frame identifiers, independent Fraction timeline selection and integer PCM sums with one final saturation; zero pixel/sample tolerance.',
            'limits':'Native opaque video and stereo PCM tracks at 25 fps. Explicit source/target locks, linked content offsets, whole-clip replacement and saved-session history. Track transitions, interval overwrite/ripple, nesting and multicam remain separate work.'}
    (root/'project.json').write_text(json.dumps(base,indent=2)+'\n',encoding='utf-8');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
