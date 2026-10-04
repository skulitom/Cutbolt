"""Reusable sequences checked against complete independently composed integer buffers."""
from engine import ENGINE
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
from tracks import time, seconds, edit, placement, track
ROOT=Path(__file__).resolve().parents[1]
W,H,N=12,8,48

def seqedit(id,op):return {'op':'sequence.edit','id':id,'edit':op['edit']}
def nested(id,source,start,duration,source_in=0,den=25):return {'id':id,'sequence_id':source,'start':time(start,den),'source_in':time(source_in,den),'duration':time(duration,den)}
def arrangement(duration,tracks):return {'duration':time(duration,25),'tracks':tracks,'links':[]}
def populated(id,kind,clips):return {**track(id,kind),'clips':clips}
def effect(id,left,right,before=2,after=2,kind='dissolve',den=25):return {'id':id,'left_id':left,'right_id':right,'before':time(before,den),'after':time(after,den),'kind':kind}

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store,queue=[root/n for n in ('sources','output','store','queue')]
    for p in (sources,out,store,queue):p.mkdir()
    exe=ENGINE;passed=[];rejected=0;frames=0;samples=0;previews=0;cases=[]
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode or not r['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert p.returncode==0 and r['ok'],r;return r['result']
    pictures=[];sounds=[];assets=[]
    for s in range(4):
        rgb=[bytes(v for y in range(H) for x in range(W) for v in ((n*17+x*7+s*59)%256,(y*19+n*3+s*101)%256,(x*13+y+n*11+s*31)%256)) for n in range(N)]
        pcm=array('h',(v for n in range(N*1920) for v in ((((n*73+s*3001)%60000)-30000,((n*41+s*7013)%62000)-31000) if s<2 else ((28000,-28000) if s==2 else (-24000,24000)))))
        pictures.append(rgb);sounds.append(pcm);raw=sources/f'{s}.rgb';audio=sources/f'{s}.pcm';movie=sources/f'{s}.mkv';raw.write_bytes(b''.join(rgb));audio.write_bytes(pcm.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        assets.append({'id':str(s),'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def preserved():assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def apply(p,ops,error=None):return call({'command':'timeline.apply','project':p,'expected_revision':p['revision'],'operations':ops},error)
    def validate(p,error=None):return call({'command':'project.validate','project':p},error)
    p=call({'command':'project.create','id':'nested-sequences','width':W,'height':H,'frame_rate':time(25)})
    ops=[{'op':'media.add','asset':a} for a in assets]+[{'op':'sequence.create','id':'shot','duration':time(20,25)}]
    ops += [seqedit('shot',edit('add',track=track(id,kind))) for id,kind in [('v','video'),('a','audio'),('mix','audio')]]
    for i in range(2):
        ops += [seqedit('shot',edit('place',track_id='v',clip=placement('v'+str(i),i,2+8*i,8,4+4*i),collision='reject')),seqedit('shot',edit('place',track_id='a',clip=placement('a'+str(i),i,(2+8*i)*1920+7,8*1920,(4+4*i)*1920+11,48000),collision='reject')),seqedit('shot',edit('link',id='link'+str(i),clip_ids=['v'+str(i),'a'+str(i)]))]
    ops += [seqedit('shot',edit('place',track_id='mix',clip=placement('mix',2,0,20,0),collision='reject'))]
    for id in ('v','a'):ops.append(seqedit('shot',edit('transition_set',track_id=id,transition=effect(id+'fx',id+'0',id+'1'))))
    ops += [edit('create',duration=time(44,25)),edit('add',track=track('v','video')),edit('add',track=track('a','audio'))]
    for kind in ('v','a'):
        for i,start in enumerate((1,23)):ops.append(edit('place',track_id=kind,clip=nested(kind+str(i),'shot',start,20),collision='reject'))
    base=apply(p,ops)
    # The oracle builds entire child RGB/PCM buffers before selecting parent source ranges.
    # No renderer graph, filter expression, output decoder or engine helper supplies expected values.
    def reference(project,scale=1):
        definitions={s['id']:s['arrangement'] for s in project.get('sequences',[])};cache={}
        pictures_scaled=[[bytes(frame[(y*scale*W+x*scale)*3+c] for y in range(H//scale) for x in range(W//scale) for c in range(3)) for frame in movie] for movie in pictures]
        def build(a,kind):
            rate=25 if kind=='video' else 48000;length=int(seconds(a['duration'])*rate);size=(W//scale)*(H//scale)*3 if kind=='video' else 2
            layers=[]
            for t in a['tracks']:
                if not t['enabled'] or t['kind']!=kind:continue
                clips={}
                for c in t['clips']:
                    if 'sequence_id' in c:
                        key=c['sequence_id'],kind
                        if key not in cache:cache[key]=build(definitions[c['sequence_id']],kind)
                        content=cache[key]
                    else:content=pictures_scaled[int(c['asset_id'])] if kind=='video' else sounds[int(c['asset_id'])]
                    start=int(seconds(c['start'])*rate);duration=int(seconds(c['duration'])*rate);first=int(seconds(c['source_in'])*rate);clips[c['id']]=(start,start+duration,first,content)
                effects=[]
                for e in t.get('transitions',[]):
                    l,r=clips[e['left_id']],clips[e['right_id']];effects.append((r[0]-int(seconds(e['before'])*rate),r[0]+int(seconds(e['after'])*rate),l,r,e['kind']))
                layers.append((list(clips.values()),effects))
            result=[] if kind=='video' else array('h')
            for n in range(length):
                total=bytes(size) if kind=='video' else [0,0]
                for clips,effects in layers:
                    clip=next((c for c in clips if c[0]<=n<c[1]),None)
                    if not clip:continue
                    def values(c):
                        first=c[2]+n-c[0]
                        return c[3][first] if kind=='video' else c[3][first*2:first*2+2]
                    fx=next((e for e in effects if e[0]<=n<e[1]),None)
                    if fx:
                        begin,end,l,r,style=fx;k=2*(n-begin)+1;d=2*(end-begin);aa,bb=(max(d-2*k,0),max(2*k-d,0)) if style=='dip_black' else (d-k,k)
                        combined=[a*aa+b*bb for a,b in zip(values(l),values(r))]
                        part=[(1 if v>=0 else -1)*((abs(v)+d//2)//d) for v in combined]
                    else:part=values(clip)
                    if kind=='video':total=bytes(part)
                    else:total=[a+b for a,b in zip(total,part)]
                if kind=='video':result.append(total)
                else:result.extend(max(-32768,min(32767,v)) for v in total)
            return result
        return b''.join(build(project['tracks'],'video')),build(project['tracks'],'audio').tobytes()
    def compare(path,expected,label,scale=1):
        nonlocal frames,samples
        rgb,pcm=expected;actual=ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-']);assert actual==rgb,(label,'video',len(actual),len(rgb),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,rgb)) if a!=b),None))
        actual=ff(['-i',str(path),'-vn','-f','s16le','-']);assert actual==pcm,(label,'audio',len(actual),len(pcm),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,pcm)) if a!=b),None))
        frames+=len(rgb)//((W//scale)*(H//scale)*3);samples+=len(pcm)//4;cases.append(label);preserved()
    def render(project,label):
        path=out/(label+'.mkv');call({'command':'render.run','project':project,'input_root':str(root),'output_root':str(out),'output':str(path)});expected=reference(project);compare(path,expected,label);return expected
    child=copy.deepcopy(base);child['tracks']=child['sequences'][0]['arrangement'];child_output=render(child,'child-alone');reused_output=render(base,'repeated-child')
    b=W*H*3;assert reused_output[0][b:21*b]==child_output[0] and reused_output[0][23*b:43*b]==child_output[0]
    assert reused_output[1][1920*4:21*1920*4]==child_output[1] and reused_output[1][23*1920*4:43*1920*4]==child_output[1];passed.append('sequences.reuse_exact_child_output')
    background=copy.deepcopy(base);background['tracks']['tracks'].insert(0,populated('background','video',[placement('background',1,0,44)]));background_output=render(background,'opaque-child-gaps-over-background')
    assert background_output[0][b:2*b]==bytes(b) and background_output[0][21*b:22*b]==pictures[1][21]
    wrapper=arrangement(24,[populated('wv','video',[nested('w0','shot',0,12,4),placement('w1',1,12,8,5)]),populated('wa','audio',[nested('w0a','shot',0,12*1920,4*1920+13,48000),placement('w1a',1,12,8,5)])])
    wrapper['tracks'][0]['transitions']=[effect('wvfx','w0','w1')];wrapper['tracks'][1]['transitions']=[effect('wafx','w0a','w1a',kind='dip_black')]
    deep=copy.deepcopy(base);deep['sequences'].append({'id':'montage','arrangement':wrapper});deep['tracks']=arrangement(30,[populated('v','video',[nested('r0','montage',0,20,1),nested('r1','shot',20,10,5)]),populated('a','audio',[nested('ra0','montage',0,20*1920,1920+5,48000),nested('ra1','shot',20*1920,10*1920,5*1920+5,48000)]),populated('bed','audio',[placement('bed',3,0,30,2)])])
    for t in deep['tracks']['tracks'][:2]:t['transitions']=[effect(t['id']+'rootfx',t['clips'][0]['id'],t['clips'][1]['id'],kind='dip_black' if t['kind']=='video' else 'dissolve')]
    validate(deep);deep_output=render(deep,'multilevel-offsets-parent-child-effects')
    # A clipped child is mixed into the parent as PCM16, not as unbounded original voices.
    assert max(array('h',child_output[1]))==32767 and min(array('h',child_output[1]))==-32768
    change=seqedit('shot',edit('slip',clip_ids=['v0'],shift={'backward':False,'amount':time(1,25)},links='include'))
    changed=apply(deep,[change]);changed_output=render(changed,'shared-child-edit-propagates');assert changed_output!=deep_output
    fxchange=seqedit('shot',edit('transition_set',track_id='v',transition=effect('vfx','v0','v1',kind='dip_black')));render(apply(deep,[fxchange]),'nested-effect-edit')
    muted=apply(deep,[seqedit('shot',edit('state',track_id='mix',enabled=False,locked=False))]);render(muted,'child-mix-change')
    sample=copy.deepcopy(deep);sample['tracks']['tracks'][1]['clips'][0]['source_in']=time(1920+17,48000);render(sample,'sample-offset-child-audio')
    split_instances=apply(deep,[edit('split',clip_ids=['r0'],at=time(5,25),links='include',right_clip_ids=[{'id':'r0','new_id':'r0-mid'}],right_link_ids=[]),edit('split',clip_ids=['r0-mid'],at=time(19,25),links='include',right_clip_ids=[{'id':'r0-mid','new_id':'r0-end'}],right_link_ids=[])])
    assert render(split_instances,'split-nested-instance-inside-effects')==deep_output
    # Source and definition namespaces are explicit; equal IDs never select the wrong source.
    same=copy.deepcopy(base);same['sequences'][0]['id']='0'
    for t in same['tracks']['tracks']:
        for c in t['clips']:c['sequence_id']='0'
    render(same,'explicit-reference-namespace');passed.append('sequences.time_mapping_nested_effects_changes')
    for project in [deep]:
        locked=copy.deepcopy(project);locked['tracks']['tracks'][0].update(locked=True,enabled=False);apply(locked,[change],'TRACK_LOCKED')
        locked=copy.deepcopy(project);locked['sequences'][1]['arrangement']['tracks'][0]['locked']=True;apply(locked,[change],'TRACK_LOCKED')
        locked=copy.deepcopy(project);locked['sequences'][0]['arrangement']['tracks'][0]['locked']=True;apply(locked,[change],'TRACK_LOCKED')
    cycle=[seqedit('shot',edit('add',track=track('cycle','video'))),seqedit('shot',edit('place',track_id='cycle',clip=nested('cycle','montage',0,1),collision='reject'))]
    apply(deep,cycle,'SEQUENCE_CYCLE');apply(deep,[cycle[0],seqedit('shot',edit('place',track_id='cycle',clip=nested('self','shot',0,1),collision='reject'))],'SEQUENCE_CYCLE')
    apply(deep,[{'op':'sequence.remove','id':'shot'}],'SEQUENCE_IN_USE')
    apply(deep,[{'op':'sequence.create','id':'shot','duration':time(1,25)}],'DUPLICATE_ID')
    apply(deep,[{'op':'sequence.remove','id':'missing'}],'MISSING_SEQUENCE')
    apply(deep,[seqedit('shot',edit('create',duration=time(1,25)))],'INVALID_SEQUENCE_EDIT')
    apply(deep,[{'op':'sequence.create','id':'badclock','duration':time(1,48000)}],'UNALIGNED_TIME')
    orphan=apply(deep,[{'op':'sequence.create','id':'unused','duration':time(0)},{'op':'sequence.remove','id':'unused'}]);assert orphan['sequences']==deep['sequences']
    for source,code in [({'sequence_id':'missing'},'MISSING_SEQUENCE'),({'sequence_id':'shot','asset_id':'0'},'INVALID_CLIP_SOURCE'),({},'INVALID_CLIP_SOURCE')]:
        bad=copy.deepcopy(deep);bad['tracks']['tracks'][0]['clips'][0].pop('sequence_id');bad['tracks']['tracks'][0]['clips'][0].update(source);validate(bad,code)
    bad=copy.deepcopy(deep);bad['tracks']['tracks'][0]['clips'][0]['source_in']=time(5,25);validate(bad,'INVALID_RANGE')
    bad=copy.deepcopy(deep);bad['tracks']['tracks'][0]['transitions'][0]['after']=time(4,25);validate(bad,'INSUFFICIENT_HANDLES')
    # A linear chain exercises the maximum depth; a distinct fan-out exercises expanded work.
    def chain(count,fanout=1):
        q=copy.deepcopy(base);q['sequences']=[];duration=1
        for i in range(count):
            children=[placement('leaf',0,0,1)] if i==0 else [nested(str(j),'s'+str(i-1),j*duration,duration) for j in range(fanout)]
            if i:duration*=fanout
            q['sequences'].append({'id':'s'+str(i),'arrangement':arrangement(duration,[populated('v','video',children)])})
        q['tracks']=arrangement(duration,[populated('v','video',[nested('root','s'+str(count-1),0,duration)])]);return q
    render(chain(8),'eight-level-chain');validate(chain(9),'LIMIT_EXCEEDED')
    fan=chain(8,3);validate(fan);call({'command':'render.plan','project':fan,'input_root':str(root),'output_root':str(out),'output':str(out/'fanout.mkv')},'LIMIT_EXCEEDED')
    many=copy.deepcopy(base);many['sequences']=[{'id':str(i),'arrangement':arrangement(1,[])} for i in range(33)];validate(many,'LIMIT_EXCEEDED')
    passed.append('sequences.cycles_locks_missing_bounds_limits')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65;schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        client.call('session.create',store_root=str(store),project=deep,request_id='create');common={'store_root':str(store),'project_id':deep['id']};fields={**common,'expected_revision':0,'request_id':'child-edit','operations':[change]};Draft202012Validator(schema).validate(fields)
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});assert not preview['clips'] and preview['sequences'][0]['id']=='shot' and set(preview['sequences'][0]['root_instances'])=={'r0','r1','ra0','ra1'}
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt
        saved=client.call('session.get',**common);assert saved['sequences']==changed['sequences'];history=client.call('session.history',**common)
        client.call('session.apply','SEQUENCE_CYCLE',**common,expected_revision=1,request_id='cycle',operations=cycle);assert client.call('session.history',**common)==history and client.call('session.get',**common)==saved
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['sequences']==deep['sequences']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1);render(client.call('session.get',**common),'saved-child-restored')
        definitions=[{'op':'sequence.create','id':'unused','duration':time(1,25)},seqedit('unused',edit('add',track=track('v','video'))),{'op':'sequence.remove','id':'unused'}];fields={**common,'expected_revision':3,'request_id':'definitions','operations':definitions};Draft202012Validator(schema).validate(fields);client.call('session.apply',**fields)
        # Reference-only changes remain visible in root placement diffs.
        swapped=copy.deepcopy(deep);swapped['tracks']['tracks'][0]['clips'][0]['sequence_id']='shot';swapped['tracks']['tracks'][0]['clips'][0]['source_in']=time(0);swapped['tracks']['tracks'][0]['transitions']=[]
        call({'command':'project.validate','project':swapped})
        swap_ops=[edit('remove',clip_ids=['r0'],links='include'),edit('place',track_id='v',clip=nested('r0','shot',0,20),collision='reject')]
        swap_diff=client.call('session.preview',**common,expected_revision=4,operations=swap_ops)
        delta=next(c for c in swap_diff['clips'] if c['clip_id']=='r0');assert delta['before']['sequence_id']=='montage' and delta['after']['sequence_id']=='shot'
        passed.append('sequences.saved_diffs_retries_undo_schema')
        for n in [0,1,6,7,8,9,10,11,12,17,18,19,20,21,22,29]:
            path=out/f'preview-{n}.png';receipt=client.call('preview.frame',project=deep,input_root=str(root),output_root=str(out),output=str(path),time=time(n,25));assert receipt['source'] is None and receipt['sources']
            with Image.open(path) as image:assert image.tobytes()==deep_output[0][n*b:(n+1)*b]
            previews+=1
        path=out/'range.mkv';call({'command':'preview.range','project':deep,'input_root':str(root),'output_root':str(out),'output':str(path),'start':time(9,25),'duration':time(12,25)});compare(path,(deep_output[0][9*b:21*b],deep_output[1][9*1920*4:21*1920*4]),'inside-child-and-parent-effects-range')
        path=out/'export.mkv';call({'command':'export.run','project':deep,'input_root':str(root),'output_root':str(out),'output':str(path),'profile':'reference','streams':'audio_video','range':{'start':time(7,25),'duration':time(15,25)}});compare(path,(deep_output[0][7*b:22*b],deep_output[1][7*1920*4:22*1920*4]),'nested-reference-export')
        proxied=copy.deepcopy(deep)
        for asset in assets:proxied=call({'command':'proxy.generate','project':proxied,'expected_revision':proxied['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(root),'output_root':str(out),'output':str(out/f'proxy-{asset["id"]}.mkv')})['project']
        proxied=apply(proxied,[{'op':'preview.proxy','scale':2}]);small=reference(deep,2);path=out/'proxy.png';client.call('preview.frame',project=proxied,input_root=str(root),output_root=str(out),output=str(path),time=time(10,25))
        with Image.open(path) as image:assert image.size==(W//2,H//2) and image.tobytes()==small[0][10*b//4:11*b//4]
        previews+=1;path=out/'proxy-range.mkv';call({'command':'preview.range','project':proxied,'input_root':str(root),'output_root':str(out),'output':str(path),'start':time(9,25),'duration':time(3,25)});compare(path,(small[0][9*b//4:12*b//4],small[1][9*1920*4:12*1920*4]),'nested-proxy-range',2)
        missing_proxy=copy.deepcopy(proxied);missing_proxy['assets'][0].pop('proxy');call({'command':'preview.frame','project':missing_proxy,'input_root':str(root),'output_root':str(out),'output':str(out/'missing-proxy.png'),'time':time(10,25)},'PROXY_MISSING');assert not (out/'missing-proxy.png').exists()
        ticket=client.call('render.start',job_root=str(queue),request_id='nested-job',render={'project':proxied,'input_root':str(root),'output_root':str(out),'output':str(out/'queued.mkv')});deadline=clock.monotonic()+90
        while True:
            state=client.call('job.status',job_root=str(queue),job_id=ticket['job_id'])
            if state['status'] in ('completed','failed','cancelled','interrupted'):break
            assert clock.monotonic()<deadline;clock.sleep(.05)
        assert state['status']=='completed',state;compare(out/'queued.mkv',deep_output,'queued-nested-original-quality');passed.append('sequences.previews_ranges_proxies_queue')
    finally:client.close()
    # A covered nested input is still checked against decoded media, including its effect handles.
    bad=copy.deepcopy(deep);bad['assets'][0]['duration']=time(100,25);bad['sequences'][0]['arrangement']['links']=[];bad['sequences'][0]['arrangement']['tracks'][0]['clips'][0]['source_in']=time(47,25)
    bad['tracks']['tracks'].append(populated('cover','video',[placement('cover',1,0,30)]));validate(bad);call({'command':'render.plan','project':bad,'input_root':str(root),'output_root':str(out),'output':str(out/'decoded-bounds.mkv')},'INVALID_RANGE')
    request={'command':'render.run','project':deep,'input_root':str(root),'output_root':str(out),'output':str(out/'repeated-child.mkv')};call(request,'OUTPUT_EXISTS')
    changed_source=copy.deepcopy(deep);changed_source['assets'][0]['identity']['sha256']='0'*64;call({**request,'project':changed_source,'output':str(out/'identity.mkv')},'IDENTITY_MISMATCH')
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with(".partial.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],capture_output=True,check=True)
    source=sources/'0.mkv';saved=source.read_bytes()
    try:call({**request,'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (out/'changed.mkv').exists() and not list(out.glob('*.partial.mkv'));preserved();passed.append('sequences.decoded_handles_identity_preservation')
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'frame_previews_compared':previews,'render_cases':cases,'rejected_cases':rejected,'reference':'Complete independently composed RGB and integer PCM buffers per child; parent source windows index those buffers. Exact reusable output, rational clocks, transition equations, signed rounding and per-child saturation; zero tolerance.','limits':'Native 25 fps reference sequences with sample-based audio, unit-rate child windows, eight levels, 32 definitions and bounded expanded render work. Multicam and broader rate/sync checkpoints remain open.'}
    (root/'project.json').write_text(json.dumps(deep,indent=2)+'\n');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))
if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
