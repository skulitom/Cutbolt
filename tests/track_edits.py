"""Native boundary editing with independent source/transition and byte-splice references."""
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
from tracks import time,seconds,edit,placement,track
ROOT=Path(__file__).resolve().parents[1]
W,H,N=24,16,50

def shift(n,d=25):return {'backward':n<0,'amount':time(abs(n),d)}
def ids(values):return [{'id':a,'new_id':b} for a,b in values]
def split(names,at,right,links=(),policy='include'):
    return edit('split',clip_ids=names,at=at,links=policy,right_clip_ids=ids(right),right_link_ids=ids(links))
def span(op,start,count,right=(),links=(),clips=(),targets=('v',),end='resize',transitions='reject_affected',policy='include',clock_=25):
    return edit(op,**{'start' if op=='ripple_delete' else 'at':time(start,clock_)},duration=time(count,clock_),track_ids=list(targets),links=policy,right_clip_ids=ids(right),right_link_ids=ids(links),end_policy=end,transitions=transitions,**({} if op=='ripple_delete' else {'clips':list(clips)}))

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store,queue=[root/n for n in ('sources','output','store','queue')]
    for p in (sources,out,store,queue):p.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';passed=[];rejected=0;frames=0;samples=0;cases=[];previews=0
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode or not r['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert p.returncode==0 and r['ok'],r;return r['result']
    pictures=[];sounds=[];assets=[]
    for s in range(3):
        rgb=[bytes(v for y in range(H) for x in range(W) for v in ((n*11+x*7+s*79)%256,(y*13+n*3+s*101)%256,(x+y+n*17+s*37)%256)) for n in range(N)]
        pcm=array('h',(v for n in range(N*1920) for v in (((n*73+s*3001)%58000)-29000,((n*41+s*7013)%64000)-32000)))
        pictures.append(rgb);sounds.append(pcm);raw=sources/f'{s}.rgb';audio=sources/f'{s}.pcm';movie=sources/f'{s}.mkv';raw.write_bytes(b''.join(rgb));audio.write_bytes(pcm.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        assets.append({'id':str(s),'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def preserved():assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    def apply(p,ops,error=None):return call({'command':'timeline.apply','project':p,'expected_revision':p['revision'],'operations':ops},error)
    base=call({'command':'project.create','id':'boundary-edits','width':W,'height':H,'frame_rate':time(25)})
    ops=[{'op':'media.add','asset':a} for a in assets]+[edit('create',duration=time(30,25)),edit('add',track=track('v','video')),edit('add',track=track('a','audio'))]
    for i in range(3):
        ops += [edit('place',track_id='v',clip=placement('v'+str(i),i,2+8*i,8,8+2*i),collision='reject'),edit('place',track_id='a',clip=placement('a'+str(i),i,(2+8*i)*1920+7,8*1920,(8+2*i)*1920+7,48000),collision='reject'),edit('link',id='link'+str(i),clip_ids=['v'+str(i),'a'+str(i)])]
    hard=apply(base,ops)
    effects=[]
    for prefix in ('v','a'):
        for i in range(2):
            b,a=(2,3) if i==0 else (3,2)
            effects.append(edit('transition_set',track_id=prefix,transition={'id':prefix+'t'+str(i),'left_id':prefix+str(i),'right_id':prefix+str(i+1),'before':time(b,25) if prefix=='v' else time(b*1920+7,48000),'after':time(a,25) if prefix=='v' else time(a*1920+11,48000),'kind':'dissolve' if i==0 else 'dip_black'}))
    base=apply(hard,effects)
    def positions(p):return {c['id']:(t['id'],c['asset_id'],seconds(c['start']),seconds(c['source_in']),seconds(c['duration'])) for t in p['tracks']['tracks'] for c in t['clips']}
    def change(p,name,**values):
        c=next(c for t in p['tracks']['tracks'] for c in t['clips'] if c['id']==name)
        for k,v in values.items():c[k]=time(v.numerator,v.denominator) if isinstance(v,F) else v
    def reference(p,scale=1):
        count=int(seconds(p['tracks']['duration'])*25);width,height=W//scale,H//scale;video=[];audio=[0]*(count*1920*2)
        for n in range(count):
            t=F(n,25);pixels=bytes(width*height*3)
            for layer in p['tracks']['tracks']:
                if not layer['enabled'] or layer['kind']!='video':continue
                clips={c['id']:c for c in layer['clips']};fx=None
                for e in layer.get('transitions',[]):
                    l,r=clips[e['left_id']],clips[e['right_id']];begin=seconds(r['start'])-seconds(e['before']);end=seconds(r['start'])+seconds(e['after'])
                    if begin<=t<end:fx=(e,l,r,begin,end);break
                def image(c):
                    raw=pictures[int(c['asset_id'])][int((seconds(c['source_in'])+t-seconds(c['start']))*25)]
                    return b''.join(raw[(y*scale*W+x*scale)*3:(y*scale*W+x*scale)*3+3] for y in range(height) for x in range(width))
                if fx:
                    e,l,r,begin,end=fx;k=2*int((t-begin)*25)+1;d=2*int((end-begin)*25);a,b=(max(d-2*k,0),max(2*k-d,0)) if e['kind']=='dip_black' else (d-k,k);pixels=bytes((x*a+y*b+d//2)//d for x,y in zip(image(l),image(r)))
                else:
                    c=next((c for c in layer['clips'] if seconds(c['start'])<=t<seconds(c['start'])+seconds(c['duration'])),None)
                    if c:pixels=image(c)
            video.append(pixels)
        for layer in p['tracks']['tracks']:
            if not layer['enabled'] or layer['kind']!='audio':continue
            clips={c['id']:(int(c['asset_id']),int(seconds(c['start'])*48000),int((seconds(c['start'])+seconds(c['duration']))*48000),int(seconds(c['source_in'])*48000)) for c in layer['clips']}
            effects=[]
            for e in layer.get('transitions',[]):
                l,r=clips[e['left_id']],clips[e['right_id']];effects.append((r[1]-int(seconds(e['before'])*48000),r[1]+int(seconds(e['after'])*48000),l,r,e['kind']))
            for n in range(count*1920):
                e=next((e for e in effects if e[0]<=n<e[1]),None);c=next((c for c in clips.values() if c[1]<=n<c[2]),None)
                for ch in range(2):
                    def value(c):return sounds[c[0]][(c[3]+n-c[1])*2+ch]
                    if e:
                        begin,end,l,r,kind=e;k=2*(n-begin)+1;d=2*(end-begin);a,b=(max(d-2*k,0),max(2*k-d,0)) if kind=='dip_black' else (d-k,k);v=value(l)*a+value(r)*b;v=(1 if v>=0 else -1)*((abs(v)+d//2)//d)
                    else:v=value(c) if c else 0
                    audio[n*2+ch]+=v
        return b''.join(video),array('h',(max(-32768,min(32767,v)) for v in audio)).tobytes()
    def compare(path,rgb,pcm,label,scale=1):
        nonlocal frames,samples
        actual=ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-']);assert actual==rgb,(label,'video',len(actual),len(rgb),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,rgb)) if a!=b),None))
        actual=ff(['-i',str(path),'-vn','-f','s16le','-']);assert actual==pcm,(label,'audio',len(actual),len(pcm),next(((i,a,b) for i,(a,b) in enumerate(zip(actual,pcm)) if a!=b),None))
        frames+=len(rgb)//((W//scale)*(H//scale)*3);samples+=len(pcm)//4;cases.append(label);preserved()
    def render(p,label,expected=None):
        output=out/(label+'.mkv');call({'command':'render.run','project':p,'input_root':str(root),'output_root':str(out),'output':str(output)});expected=reference(p) if expected is None else expected;compare(output,*expected,label);return expected
    original_output=render(base,'base');hard_output=render(hard,'hard-cuts')
    split_ops=[split(['v0'],time(9,25),[('v0','v0-r'),('a0','a0-r')],[('link0','link0-r')]),split(['v1'],time(11,25),[('v1','v1-r'),('a1','a1-r')],[('link1','link1-r')])]
    divided=apply(base,split_ops);assert len(positions(divided))==10 and len(divided['tracks']['links'])==5
    assert divided['tracks']['tracks'][0]['transitions'][0]['left_id']=='v0-r';assert divided['tracks']['tracks'][0]['transitions'][1]['left_id']=='v1-r'
    for name in ('v0-r','a0-r'):assert positions(divided)[name][2]==F(9,25)
    render(divided,'splits-inside-both-transition-sides',original_output)
    # Repeat a split across already continuous fragments; the effect clock stays anchored to its cut.
    repeated=apply(divided,[split(['v1-r'],time(12,25),[('v1-r','v1-rr'),('a1-r','a1-rr')],[('link1-r','link1-rr')])]);render(repeated,'repeated-split-continuous-handles',original_output)
    apply(divided,[split(['v0-r'],time(19,50),[('v0-r','v0-rr'),('a0-r','a0-rr')],[('link0-r','link0-rr')])],'UNALIGNED_TIME')
    audio_only=apply(base,[edit('unlink',id='link0'),split(['a0'],time(9*1920+13,48000),[('a0','a0-sample')])]);assert positions(audio_only)['a0-sample'][2]==F(9*1920+13,48000);render(audio_only,'subframe-audio-split-inside-transition',original_output)
    grouped_audio=apply(base,[edit('unlink',id='link0'),edit('add',track=track('a-copy','audio')),edit('place',track_id='a-copy',clip=placement('a-copy',0,2*1920+7,8*1920,8*1920+7,48000),collision='reject'),edit('link',id='audio-pair',clip_ids=['a0','a-copy'])])
    audio_pair=apply(grouped_audio,[split(['a0'],time(9*1920+13,48000),[('a0','a0-pair'),('a-copy','a-copy-pair')],[('audio-pair','audio-pair-r')])]);render(audio_pair,'linked-subframe-audio-split',reference(grouped_audio));passed.append('track_edits.split_links_transitions_subframes')
    for n in (-1,1):
        slip=apply(base,[edit('slip',clip_ids=['v1'],shift=shift(n),links='include')]);expected=copy.deepcopy(base)
        for name in ('v1','a1'):change(expected,name,source_in=positions(base)[name][3]+F(n,25))
        assert positions(slip)==positions(expected);render(slip,f'linked-slip-{n}',reference(expected))
        rolled=apply(base,[edit('roll',left_ids=['v0'],shift=shift(n),links='include')]);expected=copy.deepcopy(base)
        for name in ('v0','a0'):change(expected,name,duration=F(8+n,25))
        for name in ('v1','a1'):
            old=positions(base)[name];change(expected,name,start=old[2]+F(n,25),source_in=old[3]+F(n,25),duration=F(8-n,25))
        assert positions(rolled)==positions(expected);render(rolled,f'linked-roll-{n}',reference(expected))
        slid=apply(base,[edit('slide',clip_ids=['v1'],shift=shift(n),links='include')]);expected=copy.deepcopy(base)
        for name in ('v0','a0'):change(expected,name,duration=F(8+n,25))
        for name in ('v1','a1'):change(expected,name,start=positions(base)[name][2]+F(n,25))
        for name in ('v2','a2'):
            old=positions(base)[name];change(expected,name,start=old[2]+F(n,25),source_in=old[3]+F(n,25),duration=F(8-n,25))
        assert positions(slid)==positions(expected);render(slid,f'linked-slide-{n}',reference(expected))
    # Only the right pair is linked: rolling must discover its adjacent left partner too.
    discovery=apply(base,[edit('unlink',id='link0')]);discovered=apply(discovery,[edit('roll',left_ids=['v0'],shift=shift(1),links='include')]);assert positions(discovered)==positions(apply(base,[edit('roll',left_ids=['v0'],shift=shift(1),links='include')]))
    render(discovered,'neighbor-link-discovery');passed.append('track_edits.slip_slide_roll_handles_links')
    fragment_ids=[('v0','v0-after'),('a0','a0-after')];link_ids=[('link0','link0-after')]
    inserted_media=[{'track_id':name,'clip':placement('insert-'+name,2,4,2)} for name in ('v','a')]
    inserted=apply(base,[span('insert',4,2,fragment_ids,link_ids,inserted_media),edit('link',id='insert-link',clip_ids=['insert-v','insert-a'])])
    rgb,pcm=original_output;new_rgb=b''.join(pictures[2][:2]);new_pcm=sounds[2][:2*1920*2].tobytes()
    inserted_output=(rgb[:4*W*H*3]+new_rgb+rgb[4*W*H*3:],pcm[:4*1920*4]+new_pcm+pcm[4*1920*4:]);render(inserted,'insert-across-linked-tracks',inserted_output)
    assert seconds(inserted['tracks']['duration'])==F(32,25) and positions(inserted)['v1'][2]==F(12,25) and positions(inserted)['a1'][2]==F(12,25)+F(7,48000)
    overwritten=apply(base,[span('overwrite',4,2,fragment_ids,link_ids,inserted_media,end='keep')]);render(overwritten,'interval-overwrite-survivors',(rgb[:4*W*H*3]+new_rgb+rgb[6*W*H*3:],pcm[:4*1920*4]+new_pcm+pcm[6*1920*4:]))
    deleted=apply(base,[span('ripple_delete',4,2,fragment_ids,link_ids)]);deleted_output=(rgb[:4*W*H*3]+rgb[6*W*H*3:],pcm[:4*1920*4]+pcm[6*1920*4:]);render(deleted,'ripple-across-linked-tracks',deleted_output)
    gap=apply(base,[span('insert',4,1,fragment_ids,link_ids,end='keep')]);render(gap,'insert-gap-fixed-end',(rgb[:4*W*H*3]+bytes(W*H*3)+rgb[4*W*H*3:29*W*H*3],pcm[:4*1920*4]+bytes(1920*4)+pcm[4*1920*4:29*1920*4]))
    # Explicit removal permits an interval edit through a transition, with unchanged source slices outside the interval.
    removed_effect=apply(base,[span('ripple_delete',9,2,transitions='remove_affected')]);expected=copy.deepcopy(base)
    for layer in expected['tracks']['tracks']:layer['transitions']=[e for e in layer['transitions'] if not e['id'].endswith('0')]
    ergb,epcm=reference(expected);render(removed_effect,'ripple-removes-affected-transition',(ergb[:9*W*H*3]+ergb[11*W*H*3:],epcm[:9*1920*4]+epcm[11*1920*4:]))
    assert all([e['id'] for e in t.get('transitions',[])]==[t['id']+'t1'] for t in removed_effect['tracks']['tracks'])
    cleared=apply(base,[span('ripple_delete',0,30,end='keep',transitions='remove_affected')]);assert not positions(cleared) and not cleared['tracks']['links'];render(cleared,'clear-retains-explicit-end',(bytes(30*W*H*3),bytes(30*1920*4)))
    zero=apply(base,[span('ripple_delete',0,30,transitions='remove_affected')]);assert seconds(zero['tracks']['duration'])==0 and not positions(zero)
    extended=apply(base,[span('overwrite',30,2,clips=[{'track_id':'v','clip':placement('tail',2,30,2)}],end='resize')]);render(extended,'overwrite-extends-end',(rgb+new_rgb,pcm+bytes(2*1920*4)))
    passed.append('track_edits.insert_overwrite_ripple_sync')
    partial=apply(base,[edit('add',track=track('untargeted','video')),edit('place',track_id='untargeted',clip=placement('late',2,29,1),collision='reject')])
    apply(partial,[span('ripple_delete',4,2,fragment_ids,link_ids)],'INVALID_RANGE')
    kept=apply(partial,[span('ripple_delete',4,2,fragment_ids,link_ids,end='keep')]);expected_rgb=deleted_output[0]+bytes(2*W*H*3);expected_rgb=expected_rgb[:29*W*H*3]+pictures[2][0]
    render(kept,'partial-track-ripple-fixed-end',(expected_rgb,deleted_output[1]+bytes(2*1920*4)))
    locked=apply(base,[edit('state',track_id='a',locked=True,enabled=True)])
    for op in [split_ops[0],edit('slip',clip_ids=['v0'],shift=shift(1),links='include'),edit('roll',left_ids=['v0'],shift=shift(1),links='include'),edit('slide',clip_ids=['v1'],shift=shift(1),links='include'),span('insert',4,2,fragment_ids,link_ids),span('overwrite',4,2,fragment_ids,link_ids,end='keep'),span('ripple_delete',4,2,fragment_ids,link_ids)]:apply(locked,[op],'TRACK_LOCKED')
    apply(base,[span('insert',4,2,fragment_ids,link_ids,policy='reject_partial')],'LINKED_SELECTION')
    apply(base,[span('ripple_delete',2,8,transitions='remove_affected')],'SYNC_CONFLICT')
    for op in ['insert','overwrite','ripple_delete']:apply(base,[span(op,9,1)],'TRANSITION_CONFLICT')
    collision=span('insert',4,2,fragment_ids,link_ids,inserted_media+[{'track_id':'v','clip':placement('overlap',1,5,1)}]);apply(base,[collision],'CLIP_COLLISION')
    passed.append('track_edits.cross_track_locks_collisions_atomicity')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65;schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        client.call('session.create',store_root=str(store),project=base,request_id='create');common={'store_root':str(store),'project_id':base['id']};fields={**common,'expected_revision':0,'request_id':'split','operations':split_ops};Draft202012Validator(schema).validate(fields)
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});assert {x['clip_id'] for x in preview['clips']}=={'v0','a0','v0-r','a0-r','v1','a1','v1-r','a1-r'};assert preview['track_layout']['after']['tracks'][0]['transitions'][0]['left_id']=='v0-r'
        saved=client.call('session.get',**common);receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt
        assert client.call('session.get',**common)['tracks']==divided['tracks'];history=client.call('session.history',**common)
        client.call('session.apply','INVALID_RANGE',**common,expected_revision=1,request_id='bad',operations=[edit('state',track_id='a',locked=False,enabled=False),split(['v0'],time(0),[('v0','bad-v'),('a0','bad-a')],[('link0','bad-link')])]);assert client.call('session.history',**common)==history and client.call('session.get',**common)['tracks']==divided['tracks']
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['tracks']==saved['tracks']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1);render(client.call('session.get',**common),'saved-split-restored',original_output)
        # All new operation schemas are executable through the same saved contract.
        for i,op in enumerate([edit('slip',clip_ids=['v2'],shift=shift(1),links='include'),edit('roll',left_ids=['v1'],shift=shift(0),links='include'),edit('slide',clip_ids=['v0-r'],shift=shift(0),links='include')]):
            fields={**common,'expected_revision':3+i,'request_id':'later-'+str(i),'operations':[op]};Draft202012Validator(schema).validate(fields);client.call('session.apply',**fields)
        # Span schemas execute against the original snapshot and restore between independent edits.
        for i,op in enumerate([span('insert',4,2,fragment_ids,link_ids,inserted_media),span('overwrite',4,2,fragment_ids,link_ids,inserted_media,end='keep'),span('ripple_delete',4,2,fragment_ids,link_ids)]):
            revision=6+i*2;client.call('session.restore',**common,expected_revision=revision,request_id='span-base-'+str(i),target_revision=0)
            fields={**common,'expected_revision':revision+1,'request_id':'span-'+str(i),'operations':[op]};Draft202012Validator(schema).validate(fields);client.call('session.apply',**fields)
            assert client.call('session.get',**common)['tracks']==apply(base,[op])['tracks']
        passed.append('track_edits.saved_diffs_history_schema')
        for n in [0,7,8,9,10,11,12,13,15,17,18,19,20,29]:
            output=out/f'preview-{n}.png';client.call('preview.frame',project=divided,input_root=str(root),output_root=str(out),output=str(output),time=time(n,25))
            with Image.open(output) as image:assert image.tobytes()==rgb[n*W*H*3:(n+1)*W*H*3]
            previews+=1
        output=out/'range.mkv';call({'command':'preview.range','project':divided,'input_root':str(root),'output_root':str(out),'output':str(output),'start':time(9,25),'duration':time(3,25)});compare(output,rgb[9*W*H*3:12*W*H*3],pcm[9*1920*4:12*1920*4],'split-transition-range')
        output=out/'export.mkv';call({'command':'export.run','project':inserted,'input_root':str(root),'output_root':str(out),'output':str(output),'profile':'reference','streams':'audio_video'});compare(output,*inserted_output,'insert-reference-export')
        proxied=copy.deepcopy(divided)
        for asset in assets:proxied=call({'command':'proxy.generate','project':proxied,'expected_revision':proxied['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(root),'output_root':str(out),'output':str(out/f'proxy-{asset["id"]}.mkv')})['project']
        proxied=apply(proxied,[{'op':'preview.proxy','scale':2}]);small,_=reference(base,2);output=out/'proxy-preview.png';client.call('preview.frame',project=proxied,input_root=str(root),output_root=str(out),output=str(output),time=time(9,25))
        with Image.open(output) as image:assert image.size==(W//2,H//2) and image.tobytes()==small[9*(W//2)*(H//2)*3:10*(W//2)*(H//2)*3]
        previews+=1;ticket=client.call('render.start',job_root=str(queue),request_id='edited-job',render={'project':proxied,'input_root':str(root),'output_root':str(out),'output':str(out/'queued.mkv')});deadline=clock.monotonic()+90
        while True:
            state=client.call('job.status',job_root=str(queue),job_id=ticket['job_id'])
            if state['status'] in ('completed','failed','cancelled','interrupted'):break
            assert clock.monotonic()<deadline;clock.sleep(.05)
        assert state['status']=='completed',state;compare(out/'queued.mkv',*original_output,'queued-edited-full-quality');passed.append('track_edits.previews_ranges_proxies_queue')
    finally:client.close()
    failures=[(split(['v0'],time(4,25),[('v0','x')],[('link0','lr')]),'SPLIT_ID_REQUIRED'),(split(['v0'],time(4,25),[('v0','x'),('a0','y')]),'SPLIT_ID_REQUIRED'),(split(['v0'],time(4,25),[('v0','v1'),('a0','y')],[('link0','lr')]),'DUPLICATE_ID'),(split(['v0'],time(4,25),[('v0','x'),('a0','y'),('v2','z')],[('link0','lr')]),'UNUSED_SPLIT_ID'),(split(['v0'],time(2,25),[('v0','x'),('a0','y')],[('link0','lr')]),'INVALID_RANGE'),(edit('slip',clip_ids=['v0'],shift=shift(-9),links='include'),'INVALID_RANGE'),(edit('roll',left_ids=['v2'],shift=shift(1),links='include'),'MISSING_NEIGHBOR'),(edit('slide',clip_ids=['v0'],shift=shift(1),links='include'),'MISSING_NEIGHBOR'),(edit('roll',left_ids=['v0','v1'],shift=shift(1),links='include'),'INVALID_TRACK_EDIT'),(edit('roll',left_ids=['v0'],shift=shift(8),links='include'),'INVALID_RANGE'),(span('ripple_delete',29,2),'INVALID_RANGE'),(span('insert',4,0),'INVALID_RANGE'),(span('insert',4,1,targets=('missing',)),'MISSING_TRACK'),(span('insert',4,1,targets=('v','v')),'INVALID_TRACK_EDIT'),(span('insert',4,1,fragment_ids,link_ids,clips=[{'track_id':'v','clip':placement('outside',1,8,1)}]),'INVALID_TRACK_EDIT')]
    for op,code in failures:apply(base,[op],code)
    thin=copy.deepcopy(base);thin['tracks']['links']=[];change(thin,'v1',source_in=F(2,25));apply(thin,[edit('slip',clip_ids=['v1'],shift=shift(-1),links='include')],'INSUFFICIENT_HANDLES')
    original_snapshot=json.dumps(base,sort_keys=True);apply(base,[edit('roll',left_ids=['v0'],shift=shift(4),links='include')],'TRANSITION_COLLISION');assert json.dumps(base,sort_keys=True)==original_snapshot
    request={'command':'render.run','project':divided,'input_root':str(root),'output_root':str(out),'output':str(out/'base.mkv')};call(request,'OUTPUT_EXISTS')
    # Mutate only this fixture's owned source after encoding, then restore its exact bytes.
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with(".partial.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],capture_output=True,check=True)
    source=sources/'1.mkv';saved=source.read_bytes()
    try:call({**request,'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (out/'changed.mkv').exists() and not list(out.glob('*.partial.mkv'));preserved();passed.append('track_edits.invalid_handles_preservation')
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'frame_previews_compared':previews,'render_cases':cases,'rejected_cases':rejected,'reference':'Independent rational source mapping and integer transition equations; split invariance and insert/overwrite/ripple byte splices; hand-computed placements for slip/slide/roll. Zero pixel and PCM tolerance.','limits':'Native 25 fps reference tracks with 48 kHz audio; explicit fragment IDs, linked-track expansion, whole-batch locks and transition removal/end policies. Broader rate/sync and nested/multicam criteria remain open.'}
    (root/'project.json').write_text(json.dumps(divided,indent=2)+'\n',encoding='utf-8');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))
if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
