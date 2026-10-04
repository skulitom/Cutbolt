"""Editable transitions: independent integer image/audio oracles and saved/range behavior."""
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
from PIL import Image, ImageDraw
from jsonschema import Draft202012Validator
from agents import Client
from tracks import time, seconds, edit, placement, track, move
ROOT=Path(__file__).resolve().parents[1]
W,H,N=32,24,30

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store,queue=[root/n for n in ('sources','output','store','queue')]
    for path in (sources,out,store,queue):path.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';passed=[];rejected=0;frames=0;samples=0;cases=[];previews=0
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode or not r['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert p.returncode==0 and r['ok'],r;return r['result']
    graphic=Image.new('RGB',(W,H),(19,43,73));draw=ImageDraw.Draw(graphic);draw.rectangle((2,2,27,7),fill=(255,255,255));draw.ellipse((8,9,23,22),fill=(227,33,101));graphic.save(sources/'original-card.png')
    pictures=[];sounds=[];assets=[]
    for s in range(3):
        images=[bytes(v for y in range(H) for x in range(W) for v in ((n*17+x*3)%256,(y*11+n*5)%256,(x+y+n*19)%256)) if s==0 else graphic.tobytes() if s==1 else bytes(W*H*3) for n in range(N)]
        pcm=array('h',(v for n in range(N*1920) for v in (((n*73+s*5003)%60000)-30000,((n*29+s*9001)%64000)-32000)))
        pictures.append(images);sounds.append(pcm);raw=sources/f'{s}.rgb';audio=sources/f'{s}.pcm';movie=sources/f'{s}.mkv';raw.write_bytes(b''.join(images));audio.write_bytes(pcm.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        assets.append({'id':str(s),'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def preserved():assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def apply(project,ops,error=None):return call({'command':'timeline.apply','project':project,'expected_revision':project['revision'],'operations':ops},error)
    base=call({'command':'project.create','id':'transitions','width':W,'height':H,'frame_rate':time(25)})
    ops=[{'op':'media.add','asset':a} for a in assets]+[edit('create',duration=time(20,25))]
    for name,kind in [('v','video'),('a','audio')]:ops.append(edit('add',track=track(name,kind)))
    for name,s,start,count,source_in,c in [('vl',0,2,8,3,25),('vr',1,10,8,7,25),('al',0,2*1920+7,8*1920,3*1920+7,48000),('ar',1,10*1920+7,8*1920,7*1920+7,48000)]:
        ops.append(edit('place',track_id=name[0],clip=placement(name,s,start,count,source_in,c),collision='reject'))
    ops += [edit('link',id='left',clip_ids=['vl','al']),edit('link',id='right',clip_ids=['vr','ar'])];base=apply(base,ops)
    def effect(name,before,after,kind):return {'id':name,'left_id':name[0]+'l','right_id':name[0]+'r','before':before,'after':after,'kind':kind}
    def effects(kind,b=2,a=3):
        return [edit('transition_set',track_id='v',transition=effect('vt',time(b,25),time(a,25),kind)),edit('transition_set',track_id='a',transition=effect('at',time(b*1920+7,48000),time(a*1920+11,48000),'dip_black' if kind=='dip_black' else 'dissolve'))]
    def oracle(project,scale=1):
        width,height=W//scale,H//scale;count=int(seconds(project['tracks']['duration'])*25);video=[];audio=[0]*(count*1920*2)
        def image(c,t):
            frame=pictures[int(c['asset_id'])][int((seconds(c['source_in'])+t-seconds(c['start']))*25)]
            return b''.join(frame[(y*scale*W+x*scale)*3:(y*scale*W+x*scale)*3+3] for y in range(height) for x in range(width))
        def transition(layer,t):
            clips={c['id']:c for c in layer['clips']}
            for e in layer.get('transitions',[]):
                left,right=clips[e['left_id']],clips[e['right_id']];start=seconds(right['start'])-seconds(e['before']);end=seconds(right['start'])+seconds(e['after'])
                if start<=t<end:return e,left,right,start,end
            return None
        def weights(kind,k,d):return (max(d-2*k,0),max(2*k-d,0)) if kind=='dip_black' else (d-k,k)
        for n in range(count):
            pixels=bytes(width*height*3);t=F(n,25)
            for layer in project['tracks']['tracks']:
                if layer['kind']!='video' or not layer['enabled']:continue
                e=transition(layer,t)
                if e:
                    e,l,r,start,end=e;k=2*int((t-start)*25)+1;length=int((end-start)*25);d=2*length;aa,bb=weights(e['kind'],k,d);left,right=image(l,t),image(r,t);values=[]
                    for i,(a,b) in enumerate(zip(left,right)):
                        x=i//3%width
                        if e['kind']=='wipe_left':v=b if (2*x+1)*length<=width*k else a
                        elif e['kind']=='wipe_right':v=b if (2*(width-x)-1)*length<=width*k else a
                        else:v=(a*aa+b*bb+d//2)//d
                        values.append(v)
                    pixels=bytes(values)
                else:
                    for c in layer['clips']:
                        if seconds(c['start'])<=t<seconds(c['start'])+seconds(c['duration']):pixels=image(c,t)
            video.append(pixels)
        for layer in project['tracks']['tracks']:
            if layer['kind']!='audio' or not layer['enabled']:continue
            clips={c['id']:(int(c['asset_id']),int(seconds(c['start'])*48000),int((seconds(c['start'])+seconds(c['duration']))*48000),int(seconds(c['source_in'])*48000)) for c in layer['clips']}
            effects=[]
            for e in layer.get('transitions',[]):
                l,r=clips[e['left_id']],clips[e['right_id']];effects.append((r[1]-int(seconds(e['before'])*48000),r[1]+int(seconds(e['after'])*48000),l,r,e['kind']))
            for n in range(count*1920):
                e=next((e for e in effects if e[0]<=n<e[1]),None)
                c=next((c for c in clips.values() if c[1]<=n<c[2]),None)
                for ch in range(2):
                    def value(c):return sounds[c[0]][(c[3]+n-c[1])*2+ch]
                    if e:
                        start,end,l,r,kind=e;k=2*(n-start)+1;d=2*(end-start);a,b=weights(kind,k,d);v=value(l)*a+value(r)*b;v=(1 if v>=0 else -1)*((abs(v)+d//2)//d)
                    else:v=value(c) if c else 0
                    audio[n*2+ch]+=v
        return b''.join(video),array('h',(max(-32768,min(32767,v)) for v in audio)).tobytes()
    def compare(path,rgb,pcm,label,scale=1):
        nonlocal frames,samples
        actual=ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-']);assert actual==rgb,(label,'video',next(((i,a,b) for i,(a,b) in enumerate(zip(actual,rgb)) if a!=b),None),len(actual),len(rgb))
        actual=ff(['-i',str(path),'-vn','-f','s16le','-']);assert actual==pcm,(label,'audio',next(((i,a,b) for i,(a,b) in enumerate(zip(actual,pcm)) if a!=b),None),len(actual),len(pcm))
        frames+=len(rgb)//((W//scale)*(H//scale)*3);samples+=len(pcm)//4;cases.append(label);preserved()
    def render(project,label):
        output=out/(label+'.mkv');call({'command':'render.run','project':project,'input_root':str(root),'output_root':str(out),'output':str(output)});expected=oracle(project);compare(output,*expected,label);return expected
    projects={}
    for kind in ('dissolve','dip_black','wipe_left','wipe_right'):
        for b,a in [(2,3),(0,5),(5,0)]:
            p=apply(base,effects(kind,b,a));assert p['tracks']['tracks'][0]['transitions'][0]['kind']==kind;render(p,f'{kind}-{b}-{a}');projects[kind]=p
    for b,a in [(0,1),(1,0)]:render(apply(base,effects('dissolve',b,a)),f'one-frame-{b}-{a}')
    p=apply(base,effects('dissolve'));expected=render(p,'centered-base');passed.append('transitions.styles_asymmetric_handles_exact')
    # Occlusion changes graph segmentation but must never restart the transition clock.
    overlay=apply(p,[edit('add',track=track('overlay','video')),edit('place',track_id='overlay',clip=placement('occluder',2,10,1),collision='reject'),edit('add',track=track('bed','audio')),edit('place',track_id='bed',clip=placement('bed',2,0,20),collision='reject')]);render(overlay,'occluded-with-audio-bed')
    chain=copy.deepcopy(base);chain['tracks']['links']=[];chain['tracks']['duration']=time(24,25)
    for layer in chain['tracks']['tracks']:
        prefix=layer['id'];layer['clips']=[placement(prefix+str(i),i,2+i*6,6,7+i) for i in range(3)];layer['transitions']=[{'id':prefix+'x'+str(i),'left_id':prefix+str(i),'right_id':prefix+str(i+1),'before':time(3,25),'after':time(3,25),'kind':'dip_black' if i else 'dissolve'} for i in range(2)]
    render(chain,'touching-transitions-mixed-content');passed.append('transitions.mixed_content_occlusion_chains')
    moved=apply(p,[edit('add',track=track('new-v','video')),edit('add',track=track('new-a','audio')),move(['vl','vr'],1,targets=[('vl','new-v'),('vr','new-v'),('al','new-a'),('ar','new-a')])]);assert [len(t.get('transitions',[])) for t in moved['tracks']['tracks']]==[0,0,1,1];render(moved,'linked-move-and-retarget')
    removed=apply(p,[edit('remove',clip_ids=['vl'],links='include')]);assert all(not t.get('transitions') for t in removed['tracks']['tracks']);render(removed,'remove-linked-endpoints')
    untransitioned=apply(p,[edit('transition_remove',track_id='v',id='vt')]);render(untransitioned,'remove-video-transition')
    replaced=apply(p,[edit('place',track_id='v',clip=placement('replace',2,4,2),collision='replace_clips')]);assert all(not t.get('transitions') for t in replaced['tracks']['tracks']);render(replaced,'replace-linked-endpoints')
    apply(p,[move(['vr'],1)],'INVALID_TRANSITION');apply(p,[move(['vl','vr'],targets=[('vl','a')])],'INVALID_TRACKS')
    locked=apply(p,[edit('state',track_id='v',locked=True,enabled=True)])
    for op in [effects('dip_black')[0],edit('transition_remove',track_id='v',id='vt')]:apply(locked,[op],'TRACK_LOCKED')
    passed.append('transitions.edit_dependencies_locks')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        client.call('session.create',store_root=str(store),project=base,request_id='create');common={'store_root':str(store),'project_id':base['id']}
        fields={**common,'expected_revision':0,'request_id':'effects','operations':effects('dissolve')};Draft202012Validator(schema).validate(fields)
        diff=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});assert diff['clips']==[] and len(diff['track_layout']['after']['tracks'][0]['transitions'])==1
        before=client.call('session.get',**common);receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt
        saved=client.call('session.get',**common);assert saved['tracks']==p['tracks'];history=client.call('session.history',**common)
        client.call('session.apply','INVALID_TRANSITION',**common,expected_revision=1,request_id='bad',operations=[edit('state',track_id='a',locked=False,enabled=False),move(['vr'],1)])
        assert client.call('session.get',**common)==saved and client.call('session.history',**common)==history
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['tracks']==before['tracks']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1);assert client.call('session.get',**common)['tracks']==p['tracks'];render(client.call('session.get',**common),'saved-restored')
        assert client.call('capabilities')['tracks']['transitions']['video']==['dissolve','dip_black','wipe_left','wipe_right'];passed.append('transitions.saved_diffs_history_schema')
        for kind in ('dissolve','dip_black','wipe_left','wipe_right'):
            q=apply(base,effects(kind));rgb,_=oracle(q)
            for n in [7,8,9,10,11,12,13]:
                output=out/f'preview-{kind}-{n}.png';r=client.call('preview.frame',project=q,input_root=str(root),output_root=str(out),output=str(output),time=time(n,25))
                with Image.open(output) as image:assert image.tobytes()==rgb[n*W*H*3:(n+1)*W*H*3]
                if 8<=n<13:assert r['transition_id']=='vt' and len(r['sources'])==2
                previews+=1
        rgb,pcm=expected
        for first,count in [(8,1),(9,2),(10,1),(11,2),(5,10),(17,3)]:
            output=out/f'range-{first}-{count}.mkv';call({'command':'preview.range','project':p,'input_root':str(root),'output_root':str(out),'output':str(output),'start':time(first,25),'duration':time(count,25)})
            compare(output,rgb[first*W*H*3:(first+count)*W*H*3],pcm[first*1920*4:(first+count)*1920*4],f'range-{first}-{count}')
        scope=client.call('scopes.inspect',project=p,input_root=str(root),time=time(9,25),input_transfer='srgb',missing_tags='reject',columns=7)
        pixels=rgb[9*W*H*3:10*W*H*3];assert scope['rgb_sha256']==hashlib.sha256(pixels).hexdigest() and len(scope['source_interpretations'])==2
        for ch,name in enumerate(('red','green','blue')):assert scope['values'][name]['histogram']==[pixels[ch::3].count(v) for v in range(256)]
        for profile in ('reference','h264_aac'):
            output=out/('export.mkv' if profile=='reference' else 'export.mp4');request={'command':'export.run','project':p,'input_root':str(root),'output_root':str(out),'output':str(output),'profile':profile,'streams':'audio_video','range':{'start':time(9,25),'duration':time(3,25)}}
            if profile=='h264_aac':request['input_transfer']='srgb'
            call(request)
            if profile=='reference':compare(output,rgb[9*W*H*3:12*W*H*3],pcm[9*1920*4:12*1920*4],'reference-range-export')
        passed.append('transitions.previews_ranges_scopes_delivery')
        proxied=copy.deepcopy(p)
        for asset in assets:
            proxied=call({'command':'proxy.generate','project':proxied,'expected_revision':proxied['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(root),'output_root':str(out),'output':str(out/f'proxy-{asset["id"]}.mkv')})['project']
        proxied=apply(proxied,[{'op':'preview.proxy','scale':2}]);small,_=oracle(proxied,2)
        output=out/'proxy-preview.png';r=client.call('preview.frame',project=proxied,input_root=str(root),output_root=str(out),output=str(output),time=time(9,25));assert r['source_quality']=='proxy'
        with Image.open(output) as image:assert image.size==(W//2,H//2) and image.tobytes()==small[9*(W//2)*(H//2)*3:10*(W//2)*(H//2)*3]
        previews+=1;output=out/'proxy-range.mkv';call({'command':'preview.range','project':proxied,'input_root':str(root),'output_root':str(out),'output':str(output),'start':time(8,25),'duration':time(4,25)})
        compare(output,small[8*(W//2)*(H//2)*3:12*(W//2)*(H//2)*3],pcm[8*1920*4:12*1920*4],'proxy-transition-range',2)
        render(proxied,'original-quality-selected-proxy');ticket=client.call('render.start',job_root=str(queue),request_id='transition-job',render={'project':proxied,'input_root':str(root),'output_root':str(out),'output':str(out/'queued.mkv')});deadline=clock.monotonic()+90
        while True:
            state=client.call('job.status',job_root=str(queue),job_id=ticket['job_id'])
            if state['status'] in ('completed','failed','cancelled','interrupted'):break
            assert clock.monotonic()<deadline;clock.sleep(.05)
        assert state['status']=='completed',state;compare(out/'queued.mkv',*expected,'queued-transitions');passed.append('transitions.proxies_queued_original_quality')
    finally:client.close()
    for fx,error in [(effect('vt',time(0),time(0),'dissolve'),'INVALID_TRANSITION'),(effect('vt',time(9,25),time(0),'dissolve'),'INVALID_TRANSITION'),(effect('vt',time(8,25),time(1,25),'dissolve'),'INSUFFICIENT_HANDLES'),(effect('vt',time(1,48000),time(1,25),'dissolve'),'UNALIGNED_TIME'),({**effect('vt',time(1,25),time(1,25),'dissolve'),'left_id':'missing'},'INVALID_TRANSITION'),({**effect('vt',time(1,25),time(1,25),'dissolve'),'right_id':'vl'},'INVALID_TRANSITION')]:apply(base,[edit('transition_set',track_id='v',transition=fx)],error)
    apply(base,[edit('transition_set',track_id='a',transition=effect('at',time(1,25),time(1,25),'wipe_left'))],'INVALID_TRANSITION')
    apply(p,[edit('transition_set',track_id='v',transition=effect('duplicate-interval',time(2,25),time(3,25),'dissolve')|{'left_id':'vl','right_id':'vr'})],'TRANSITION_COLLISION')
    apply(p,[edit('transition_remove',track_id='v',id='missing')],'MISSING_TRANSITION')
    apply(p,[edit('transition_set',track_id='a',transition=effect('at',time(1,25),time(1,25),'dissolve')|{'id':'vt'})],'DUPLICATE_ID')
    short=copy.deepcopy(base);short['tracks']['links']=[];short['tracks']['tracks'][0]['clips'][0]['source_in']=time(20,25);apply(short,effects('dissolve'),'INSUFFICIENT_HANDLES')
    overdeclared=copy.deepcopy(short);overdeclared['assets'][0]['duration']=time(40,25);overdeclared=apply(overdeclared,effects('dissolve'))
    request={'command':'render.run','project':overdeclared,'input_root':str(root),'output_root':str(out),'output':str(out/'bad-handles.mkv')};call(request,'INVALID_RANGE')
    call({**request,'project':p,'output':str(out/'centered-base.mkv')},'OUTPUT_EXISTS')
    huge=copy.deepcopy(base);huge['tracks']['links']=[];huge['tracks']['duration']=time(300000000000001)
    for asset in huge['assets']:asset['duration']=time(300000000000001)
    huge['tracks']['tracks']=[{**track('a','audio'),'clips':[placement('al',0,0,1,0,1),placement('ar',1,1,300000000000000,1,1)],'transitions':[effect('at',time(1),time(300000000000000),'dissolve')]}]
    call({'command':'preview.range','project':huge,'input_root':str(root),'output_root':str(out),'output':str(out/'overflow.mkv'),'start':time(0),'duration':time(1,25)},'TIME_OVERFLOW')
    broken=copy.deepcopy(p);broken['assets'][1]['identity']['sha256']='0'*64;call({**request,'project':broken},'IDENTITY_MISMATCH')
    # Scope declarations must be checked against both transition inputs.
    different=out/'different-transfer.mkv';ff(['-i',assets[1]['path'],'-c','copy','-color_trc','bt709',str(different)])
    wrong=copy.deepcopy(p);wrong['assets'][1].update(path=str(different),identity={'sha256':hashlib.sha256(different.read_bytes()).hexdigest(),'bytes':different.stat().st_size})
    call({'command':'scopes.inspect','project':wrong,'input_root':str(root),'time':time(9,25),'input_transfer':'srgb','missing_tags':'reject','columns':7},'INVALID_COLOR')
    tool=root/'changed-source.exe';wrapper=root/'changed-source.rs'
    wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with(".partial.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=sources/'1.mkv';saved=source.read_bytes()
    try:call({**request,'project':p,'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (out/'changed.mkv').exists() and not list(out.glob('*.partial.mkv'));preserved();passed.append('transitions.rejections_identity_preservation')
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'frame_previews_compared':previews,'render_cases':cases,'rejected_cases':rejected,'reference':'Independent Fraction interval/source selection, integer RGB blend/wipe equations and signed PCM rounding before track summation. All pixels and samples match exactly.','limits':'Four encoded-RGB8 video styles; linear/dip stereo PCM transitions; explicit adjacent-clip handles at 25 fps or 48 kHz. Track boundary edits and broader timing remain separate work.'}
    (root/'project.json').write_text(json.dumps(p,indent=2)+'\n',encoding='utf-8');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))
if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
