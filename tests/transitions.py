"""Editable transitions: independent integer image/audio oracles and saved/range behavior."""
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
import wave
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
    exe=ENGINE;passed=[];rejected=0;frames=0;samples=0;cases=[];previews=0
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
    def curve_value(curve,t):
        keys=sorted((seconds(k['time']),k['value'],k['interpolation']) for k in curve['keys'])
        if t<=keys[0][0]:return keys[0][1]
        if t>=keys[-1][0]:return keys[-1][1]
        j=max(i for i,k in enumerate(keys) if k[0]<=t);(ta,va,mode),(tb,vb,_)=keys[j],keys[j+1]
        if mode=='hold':return va
        u=(t-ta)/(tb-ta);w=va*(u.denominator-u.numerator)+vb*u.numerator
        return (1 if w>=0 else -1)*((abs(w)+u.denominator//2)//u.denominator)
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
            clips={c['id']:(int(c['asset_id']),int(seconds(c['start'])*48000),int((seconds(c['start'])+seconds(c['duration']))*48000),int(seconds(c['source_in'])*48000),c.get('gain_milli',1000),int(seconds(c.get('fade_in',time(0)))*48000),int(seconds(c.get('fade_out',time(0)))*48000),c.get('gain_curve')) for c in layer['clips']}
            effects=[]
            for e in layer.get('transitions',[]):
                l,r=clips[e['left_id']],clips[e['right_id']];effects.append((r[1]-int(seconds(e['before'])*48000),r[1]+int(seconds(e['after'])*48000),l,r,e['kind']))
            for n in range(count*1920):
                e=next((e for e in effects if e[0]<=n<e[1]),None)
                c=next((c for c in clips.values() if c[1]<=n<c[2]),None)
                for ch in range(2):
                    def value(c):
                        # Clip gain and linear fades on the clip's own sample clock, one signed rounding.
                        s=sounds[c[0]][(c[3]+n-c[1])*2+ch];g,fi,fo,curve=c[4:];i=n-c[1];total=c[2]-c[1]
                        # A gain curve replaces gain_milli, sampled on the clip's source clock.
                        if curve:g=curve_value(curve,F(c[3]+n-c[1],48000))
                        if fi and i<fi:w,d=g*i,1000*fi
                        elif fo and i>=total-fo:w,d=g*(total-i),1000*fo
                        else:w,d=g,1000
                        v=s*w;return (1 if v>=0 else -1)*((abs(v)+d//2)//d)
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
    # Clip gain and fades apply per clip before transitions and track summation; cuts keep them.
    # al's gain curve (ramps and holds on its source clock, which starts at sample 3*1920+7) replaces its gain.
    source=3*1920+7
    ramp={'keys':[{'time':time(source+2400,48000),'value':1000,'interpolation':'linear'},{'time':time(source+7200,48000),'value':300,'interpolation':'hold'},
                  {'time':time(source+9600,48000),'value':2600,'interpolation':'linear'},{'time':time(source+14000,48000),'value':800,'interpolation':'linear'}]}
    levels=apply(p,[edit('clip_audio',clip_ids=['al'],gain_milli=1500,fade_in=time(5003,48000),gain_curve=ramp),edit('clip_audio',clip_ids=['ar'],gain_milli=600,fade_out=time(3*1920+11,48000))])
    assert [c.get('gain_milli') for c in levels['tracks']['tracks'][1]['clips']]==[1500,600] and levels['tracks']['tracks'][1]['clips'][0]['gain_curve']==ramp
    leveled=render(levels,'clip-gain-fades-with-transition');assert leveled[1]!=expected[1]
    cut=apply(levels,[edit('split',clip_ids=['al'],at=time(6,25),links='include',right_clip_ids=[{'id':'al','new_id':'al2'},{'id':'vl','new_id':'vl2'}],right_link_ids=[{'id':'left','new_id':'left2'}])])
    assert render(cut,'split-keeps-clip-fades')==leveled
    apply(levels,[edit('split',clip_ids=['al'],at=time(3,25),links='include',right_clip_ids=[{'id':'al','new_id':'al2'},{'id':'vl','new_id':'vl2'}],right_link_ids=[{'id':'left','new_id':'left2'}])],'INVALID_RANGE')
    for first,count in [(3,4),(15,4)]:
        output=out/f'levels-range-{first}.mkv';call({'command':'preview.range','project':levels,'input_root':str(root),'output_root':str(out),'output':str(output),'start':time(first,25),'duration':time(count,25)})
        compare(output,leveled[0][first*W*H*3:(first+count)*W*H*3],leveled[1][first*1920*4:(first+count)*1920*4],f'levels-range-{first}-{count}')
    apply(p,[edit('clip_audio',clip_ids=['vl'],gain_milli=500)],'INVALID_TRACKS')
    apply(p,[edit('clip_audio',clip_ids=['ar'],fade_in=time(1920,48000))],'INVALID_TRANSITION')
    apply(p,[edit('clip_audio',clip_ids=['al'],gain_milli=4001)],'INVALID_TRACKS')
    apply(base,[edit('clip_audio',clip_ids=['al'],fade_in=time(10000,48000),fade_out=time(6000,48000))],'INVALID_RANGE')
    apply(base,[edit('clip_audio',clip_ids=['al'],fade_in=time(1,96000))],'UNALIGNED_TIME')
    passed.append('transitions.clip_gain_fades_exact')
    # Timeline meters measure exactly the rendered timeline PCM: compare with a mix recipe of the oracle PCM.
    def metered(pcm,name):
        path=out/(name+'.wav')
        with wave.open(str(path),'wb') as w:w.setnchannels(2);w.setsampwidth(2);w.setframerate(48000);w.writeframes(pcm)
        n=len(pcm)//4;length={'num':n,'den':48000}
        clip={'id':'c','file':{'path':path.name,'sha256':hashlib.sha256(path.read_bytes()).hexdigest(),'bytes':path.stat().st_size},'channels':'preserve_stereo','start':time(0),'source_in':time(0),'duration':length}
        return call({'command':'audio.inspect','mix':{'schema_version':1,'id':name,'duration':length,'tracks':[{'id':'t','clips':[clip]}]},'input_root':str(out)})['meters']
    whole=call({'command':'timeline.meters','project':levels,'input_root':str(root)})
    assert whole['mix']==metered(leveled[1],'levels-mix') and [t['track_id'] for t in whole['tracks']]==['a'] and whole['tracks'][0]['meters']==whole['mix']
    part=call({'command':'timeline.meters','project':levels,'input_root':str(root),'start':time(5,25),'duration':time(10,25),'tracks':False})
    assert part['mix']==metered(leveled[1][5*1920*4:15*1920*4],'levels-part') and part['tracks']==[]
    passed.append('transitions.timeline_meters_match_rendered_pcm')
    # Ducking: speech on the dialogue track lowers a music bed. Regions and keys are recomputed here
    # from the oracle's voice-only PCM, and the applied curves render exactly.
    ducking=apply(base,[edit('unlink',id='right'),edit('remove',clip_ids=['ar'],links='reject_partial'),edit('add',track=track('m','audio')),
                        edit('place',track_id='m',clip=placement('bed',2,0,20,0),collision='reject')])
    settings={'threshold_db':-45,'duck_milli':300,'attack':time(960,48000),'release':time(1920,48000),'bridge':time(4800,48000)}
    proposal=call({'command':'audio.duck','project':ducking,'input_root':str(root),'voice_track_id':'a','music_track_id':'m',**settings})
    voice=copy.deepcopy(ducking);next(t for t in voice['tracks']['tracks'] if t['id']=='m')['enabled']=False
    pcm=array('h',oracle(voice)[1]);limit=2*480*32768**2*10**(-45/10);windows=[]
    for w in range(len(pcm)//960):
        energy=sum(v*v for v in pcm[w*960:(w+1)*960])
        if energy>=limit:
            if windows and windows[-1][1]==w*480:windows[-1][1]=(w+1)*480
            else:windows.append([w*480,(w+1)*480])
    regions=[]
    for region in windows:
        if regions and region[0]-regions[-1][1]<4800:regions[-1][1]=region[1]
        else:regions.append(region)
    assert [(r['start'],r['end']) for r in proposal['speech']['runs']]==[(time(a,48000),time(b,48000)) for a,b in regions] and regions,proposal['speech']
    spans=[]
    for a,b in regions:
        span=[a-960,a,b,b+1920]
        if span[3]<=0 or span[0]>=20*1920:continue
        if spans and span[0]<=spans[-1][3]:spans[-1][2:]=span[2:]
        else:spans.append(span)
    keys=[]
    for down,low,high,up in spans:
        for t,value,mode in ([(down,1000,'linear')] if down<low else [])+[(low,300,'hold')]+([(high,300,'linear'),(up,1000,'hold')] if high<up else [(high,1000,'hold')]):
            if 0<=t<=30*1920:keys.append({'time':time(t,48000),'value':value,'interpolation':mode})
    assert proposal['operations']==[{'op':'tracks.edit','edit':{'op':'clip_audio','clip_ids':['bed'],'gain_curve':{'keys':keys}}}],proposal['operations']
    ducked=apply(ducking,proposal['operations']);render(ducked,'ducked-music')
    call({'command':'audio.duck','project':ducking,'input_root':str(root),'voice_track_id':'a','music_track_id':'a'},'INVALID_ARGUMENT')
    call({'command':'audio.duck','project':ducking,'input_root':str(root),'voice_track_id':'v','music_track_id':'m'},'MISSING_TRACK')
    passed.append('transitions.ducking_proposal_renders_exactly')
    # Normalizing: one factor scales every audio clip. The factor, its limit and every level are
    # recomputed here from meters of the oracle's mix; the applied levels render exactly and land
    # on the target or the limit.
    import math
    measured_mixes=[0]
    def normalized(p,target,ceiling):
        clips=[c for t in p['tracks']['tracks'] if t['kind']=='audio' for c in t['clips']]
        scale=lambda v,f:min(4000,math.floor(v*f+0.5))
        def levels(f):return [[scale(k['value'],f) for k in c['gain_curve']['keys']] if c.get('gain_curve') else [scale(c.get('gain_milli',1000),f)] for c in clips]
        def scaled(f):
            q=copy.deepcopy(p)
            for t in q['tracks']['tracks']:
                if t['kind']!='audio':continue
                for c in t['clips']:
                    if c.get('gain_curve'):c['gain_curve']['keys']=[{**k,'value':scale(k['value'],f)} for k in c['gain_curve']['keys']]
                    else:c['gain_milli']=scale(c.get('gain_milli',1000),f)
            return q
        def measure(f):
            measured_mixes[0]+=1;m=metered(oracle(scaled(f))[1],f'normalize-{measured_mixes[0]}')
            return m['integrated_lkfs'],max(v for v in m['sample_peak_dbfs'] if v is not None)
        largest=max(v for level in levels(1.0) for v in level)
        by_range=4000/largest if largest else math.inf
        first=measure(1.0);f,(loud,peak),limited,passes=1.0,first,None,1
        for _ in range(3):
            wanted=f*10**((target-loud)/20);by_peak=f*10**((ceiling-peak)/20)
            nxt=min(wanted,by_peak,by_range)
            limited=None if wanted<=min(by_peak,by_range) else ('peak_ceiling' if by_peak<=by_range else 'clip_gain_range')
            if levels(nxt)==levels(f):break
            f=nxt;loud,peak=measure(f);passes+=1
        groups,curves={},[]
        for c,new in zip(clips,levels(f)):
            if c.get('gain_curve'):
                if new!=[k['value'] for k in c['gain_curve']['keys']]:
                    curves.append({'op':'tracks.edit','edit':{'op':'clip_audio','clip_ids':[c['id']],'gain_curve':{**c['gain_curve'],'keys':[{**k,'value':v} for k,v in zip(c['gain_curve']['keys'],new)]}}})
            elif new[0]!=c.get('gain_milli',1000):groups.setdefault(new[0],[]).append(c['id'])
        operations=[{'op':'tracks.edit','edit':{'op':'clip_audio','clip_ids':ids,'gain_milli':level}} for level,ids in sorted(groups.items())]+curves
        return first,f,limited,operations,(loud,peak),passes
    def normalize(p,target,ceiling,label):
        proposal=call({'command':'audio.normalize','project':p,'input_root':str(root),'target_lkfs':target,'peak_ceiling_dbfs':ceiling})
        first,factor,limited,operations,final,passes=normalized(p,target,ceiling)
        assert proposal['operations']==operations and proposal['limited_by']==limited,(proposal,operations,limited)
        assert abs(proposal['measured']['integrated_lkfs']-first[0])<=.006 and abs(proposal['measured']['sample_peak_dbfs']-first[1])<=.006
        assert abs(proposal['result']['integrated_lkfs']-final[0])<=.006 and abs(proposal['result']['sample_peak_dbfs']-final[1])<=.006
        assert proposal['result']['measured_passes']==passes and abs(proposal['gain_db']-20*math.log10(factor))<=.006
        assert proposal['clips']==sum(len(o['edit']['clip_ids']) for o in operations)
        done=apply(p,operations);render(done,label)
        after=call({'command':'timeline.meters','project':done,'input_root':str(root),'tracks':False})['mix']
        assert abs(after['integrated_lkfs']-final[0])<1e-9 and max(after['sample_peak_dbfs'])==final[1]
        return proposal,after
    loudness,peak=normalized(ducked,-14,-1)[0]
    target=round(loudness-6,1)
    proposal,after=normalize(ducked,target,-1,'normalized-quieter')
    assert proposal['limited_by'] is None and abs(after['integrated_lkfs']-target)<=.05,(after,target)
    ceiling=math.floor(peak)-2
    proposal,after=normalize(ducked,min(-5,round(loudness+3,1)),ceiling,'normalized-peak-limited')
    assert proposal['limited_by']=='peak_ceiling' and max(after['sample_peak_dbfs'])<=ceiling+.01,(proposal,after)
    quiet=apply(ducked,[edit('clip_audio',clip_ids=['al'],gain_milli=100),
                        edit('clip_audio',clip_ids=['bed'],gain_curve={'keys':[{**k,'value':k['value']//10} for k in next(c for t in ducked['tracks']['tracks'] for c in t['clips'] if c['id']=='bed')['gain_curve']['keys']]}),
                        edit('add',track={**track('spare','audio'),'enabled':False}),edit('place',track_id='spare',clip=placement('extra',2,0,10,0),collision='reject'),
                        edit('clip_audio',clip_ids=['extra'],gain_milli=3000)])
    proposal,after=normalize(quiet,-5,0,'normalized-gain-range')
    assert proposal['limited_by']=='clip_gain_range' and {'op':'tracks.edit','edit':{'op':'clip_audio','clip_ids':['extra'],'gain_milli':4000}} in proposal['operations']
    sequential=call({'command':'project.create','id':'plain','width':W,'height':H,'frame_rate':time(25)})
    for subject,fields,code in ((sequential,{},'UNSUPPORTED_TIMELINE'),(apply(ducked,[edit('state',track_id='m',locked=True,enabled=True)]),{},'TRACK_LOCKED'),
                          (apply(ducked,[edit('clip_audio',clip_ids=['al'],gain_milli=0),edit('clip_audio',clip_ids=['bed'],clear_gain_curve=True,gain_milli=0)]),{},'LOUDNESS_UNMEASURED'),
                          (ducked,{'target_lkfs':-50},'INVALID_ARGUMENT'),(ducked,{'peak_ceiling_dbfs':1},'INVALID_ARGUMENT')):
        call({'command':'audio.normalize','project':subject,'input_root':str(root),**fields},code)
    passed.append('transitions.normalize_proposal_reaches_target_or_limit')
    # Tightening: a 4 s talking-head source with silences at frames 20-45, 70-80 and 92-100. Cuts are
    # recomputed here from the oracle's voice-only PCM, and the result is the original render with
    # exactly those intervals deleted.
    talk_frames=100
    talk_pictures=[bytes(v for y in range(H) for x in range(W) for v in ((n*7+x*5)%256,(y*13+n*3)%256,(x*y+n*11)%256)) for n in range(talk_frames)]
    talk_pcm=array('h',(0 if 20*1920<=n<45*1920 or 70*1920<=n<80*1920 or n>=92*1920 else v for n in range(talk_frames*1920) for v in (((n*67)%60000)-30000,((n*41)%60000)-30000)))
    (sources/'3.rgb').write_bytes(b''.join(talk_pictures));(sources/'3.pcm').write_bytes(talk_pcm.tobytes())
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(sources/'3.rgb'),'-f','s16le','-ar','48000','-ac','2','-i',str(sources/'3.pcm'),
        '-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(sources/'3.mkv')])
    pictures.append(talk_pictures);sounds.append(talk_pcm)
    talk_asset={'id':'3','path':str(sources/'3.mkv'),'duration':time(talk_frames,25),'identity':{'sha256':hashlib.sha256((sources/'3.mkv').read_bytes()).hexdigest(),'bytes':(sources/'3.mkv').stat().st_size}}
    original.update({p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()})
    talking=call({'command':'project.create','id':'talking','width':W,'height':H,'frame_rate':time(25)})
    talking=apply(talking,[{'op':'media.add','asset':a} for a in assets+[talk_asset]]+[edit('create',duration=time(150,25)),
        edit('add',track=track('v','video')),edit('add',track=track('a','audio')),edit('add',track=track('m','audio')),
        edit('place',track_id='v',clip=placement('tv',3,10,100,0),collision='reject'),edit('place',track_id='a',clip=placement('ta',3,10,100,0),collision='reject'),
        edit('place',track_id='m',clip=placement('mb',0,25,30,0),collision='reject'),edit('link',id='talk',clip_ids=['tv','ta']),
        edit('clip_audio',clip_ids=['mb'],gain_milli=300,fade_in=time(1,5))])
    def tighten_oracle(p,voice,threshold=-45,min_pause=F(3,4),keep=F(1,5),edges=True):
        solo=copy.deepcopy(p)
        if p.get('tracks'):
            for t in solo['tracks']['tracks']:
                if t['kind']=='audio':t['enabled']=t['id']==voice
            pcm=array('h',oracle(solo)[1])
        else:pcm=array('h',sequential_oracle(solo)[1])
        limit=2*480*32768**2*10**(threshold/10);regions=[];count=len(pcm)//2
        for w in range((count+479)//480):
            chunk=pcm[w*960:(w+1)*960]
            if sum(v*v for v in chunk)>=limit:
                end=w*480+len(chunk)//2
                if regions and regions[-1][1]==w*480:regions[-1][1]=end
                else:regions.append([w*480,end])
        merged=[]
        for r in regions:
            if merged and r[0]-merged[-1][1]<min_pause*48000:merged[-1][1]=r[1]
            else:merged.append(r)
        duration=seconds(p['tracks']['duration']) if p.get('tracks') else sum(seconds(c['duration']) for c in p['clips'])
        total=math.floor(duration*48000);pauses=[]
        if merged:
            if edges and merged[0][0]>=min_pause*48000:pauses.append((0,merged[0][0],'leading'))
            pauses+=[(a[1],b[0],'between') for a,b in zip(merged,merged[1:])]
            if edges and total-merged[-1][1]>=min_pause*48000:pauses.append((merged[-1][1],total,'trailing'))
        cuts=[]
        for a,b,kind in pauses:
            first=0 if kind=='leading' else math.ceil(F(a+keep*48000,48000)*25)
            last=int(duration*25) if kind=='trailing' else math.floor(F(b-keep*48000,48000)*25)
            if last>first:cuts.append((F(first,25),F(last,25)))
        used={c['id'] for c in p['clips']}
        if p.get('tracks'):used|={c['id'] for t in p['tracks']['tracks'] for c in t['clips']}|{l['id'] for l in p['tracks']['links']}
        def fresh(base):
            n=1
            while f'{base}-j{n}' in used:n+=1
            used.add(f'{base}-j{n}');return f'{base}-j{n}'
        operations=[]
        for s,e in reversed(cuts):
            if p.get('tracks'):
                split=[c['id'] for t in p['tracks']['tracks'] for c in t['clips'] if seconds(c['start'])<s and seconds(c['start'])+seconds(c['duration'])>e]
                right=[{'id':c,'new_id':fresh(c)} for c in split]
                links=[{'id':l['id'],'new_id':fresh(l['id'])} for l in p['tracks']['links'] if all(m['clip_id'] in split for m in l['members'])]
                operations.append({'op':'tracks.edit','edit':{'op':'ripple_delete','track_ids':[t['id'] for t in p['tracks']['tracks']],'start':time(s),'duration':time(e-s),
                    'links':'include','right_clip_ids':right,'right_link_ids':links,'end_policy':'resize','transitions':'reject_affected'}})
            else:
                op={'op':'timeline.ripple_delete','start':time(s),'duration':time(e-s)};at=F(0)
                for c in p['clips']:
                    if at<s and at+seconds(c['duration'])>e:op['right_id']=fresh(c['id'])
                    at+=seconds(c['duration'])
                operations.append(op)
        return cuts,operations
    def sequential_oracle(p):
        rgb,pcm=b'',b''
        for c in p['clips']:
            first=int(seconds(c['source_in'])*25);count=int(seconds(c['duration'])*25)
            if c.get('gap'):rgb+=bytes(count*W*H*3);pcm+=bytes(count*1920*4);continue
            rgb+=b''.join(pictures[int(c['asset_id'])][first:first+count]);pcm+=sounds[int(c['asset_id'])][first*1920*2:(first+count)*1920*2].tobytes()
        return rgb,pcm
    whole_talk=render(talking,'talking')
    proposal=call({'command':'audio.tighten','project':talking,'input_root':str(root),'voice_track_id':'a'})
    cuts,operations=tighten_oracle(talking,'a')
    assert cuts==[(F(35,25),F(50,25)),(F(107,25),F(150,25))] and proposal['operations']==operations,(cuts,proposal['operations'],operations)
    assert proposal['removed']==time(58,25) and proposal['duration_after']==time(92,25) and proposal['pauses']['cuts']==2
    tightened=apply(talking,operations);result=render(tightened,'tightened')
    assert result==(whole_talk[0][:35*W*H*3]+whole_talk[0][50*W*H*3:107*W*H*3],whole_talk[1][:35*1920*4]+whole_talk[1][50*1920*4:107*1920*4])
    assert {c['id'] for t in tightened['tracks']['tracks'] for c in t['clips']}=={'tv','tv-j1','ta','ta-j1','mb','mb-j1'} and {l['id'] for l in tightened['tracks']['links']}=={'talk','talk-j1'}
    # A sequential timeline is analysed whole; its cut keeps the right part as a new clip.
    plain=call({'command':'project.create','id':'plain-talk','width':W,'height':H,'frame_rate':time(25)})
    plain=apply(plain,[{'op':'media.add','asset':talk_asset},{'op':'clip.append','clip':{'id':'c1','asset_id':'3','source_in':time(0),'duration':time(100,25)}}])
    proposal=call({'command':'audio.tighten','project':plain,'input_root':str(root)})
    cuts,operations=tighten_oracle(plain,None)
    assert cuts==[(F(25,25),F(40,25))] and proposal['operations']==operations==[{'op':'timeline.ripple_delete','start':time(1),'duration':time(15,25),'right_id':'c1-j1'}]
    rgb,pcm=sequential_oracle(plain);done=apply(plain,operations)
    out_path=out/'plain-tightened.mkv';call({'command':'render.run','project':done,'input_root':str(root),'output_root':str(out),'output':str(out_path)})
    compare(out_path,rgb[:25*W*H*3]+rgb[40*W*H*3:],pcm[:25*1920*4]+pcm[40*1920*4:],'plain-tightened')
    # Cuts the editor refuses are listed, not proposed: here a long fade-out covers both pauses.
    faded=apply(talking,[edit('clip_audio',clip_ids=['ta'],fade_out=time(3))])
    refused=call({'command':'audio.tighten','project':faded,'input_root':str(root),'voice_track_id':'a'})
    assert refused['operations']==[] and all('skipped' in p for p in refused['pauses']['listed']) and refused['pauses']['count']==2,refused
    loose=call({'command':'audio.tighten','project':talking,'input_root':str(root),'voice_track_id':'a','min_pause':time(3,10),'keep':time(1,10),'edges':False})
    assert loose['operations']==tighten_oracle(talking,'a',min_pause=F(3,10),keep=F(1,10),edges=False)[1] and loose['pauses']['count']==2
    for subject,fields,code in ((talking,{},'INVALID_ARGUMENT'),(talking,{'voice_track_id':'v'},'MISSING_TRACK'),(plain,{'voice_track_id':'a'},'INVALID_ARGUMENT'),
                                (apply(talking,[edit('state',track_id='m',locked=True,enabled=True)]),{'voice_track_id':'a'},'TRACK_LOCKED'),
                                (talking,{'voice_track_id':'a','keep':time(2,5)},'INVALID_ARGUMENT')):
        call({'command':'audio.tighten','project':subject,'input_root':str(root),**fields},code)
    passed.append('transitions.tighten_cuts_pauses_exactly')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
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
        assert client.call('capabilities',section='all')['tracks']['transitions']['video']==['dissolve','dip_black','wipe_left','wipe_right'];passed.append('transitions.saved_diffs_history_schema')
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
