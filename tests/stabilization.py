"""Original handheld/roll/cut fixtures with physical trajectory and rendered pixel references."""
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import subprocess
from PIL import Image, ImageFilter
from jsonschema import Draft202012Validator
from agents import Client
from animation import sample
from scenes import identity, time, selected
from spatial import reference
from tracking import coverage, spatial

ROOT=Path(__file__).resolve().parents[1]
W,H=64,48
REGIONS=[[8,8,8,8],[48,8,8,8],[8,32,8,8],[48,32,8,8]]


def world(seed):
    rng=random.Random(seed);im=Image.new('RGB',(W+32,H+32))
    im.putdata([(v,v,v) for v in (rng.randrange(8,248) for _ in range(im.width*im.height))])
    return im.filter(ImageFilter.GaussianBlur(.6))


def camera(image,pose,zoom=1):
    """Independent inverse pinhole-plane pose with pixel-center bilinear source sampling."""
    tx,ty,angle=[v/1000 for v in pose];sn=math.sin(math.radians(angle));cs=math.cos(math.radians(angle));result=Image.new('RGB',(W,H))
    for y in range(H):
        for x in range(W):
            u=(x+.5-W/2)/zoom-tx;v=(y+.5-H/2)/zoom-ty
            sx=cs*u+sn*v+W/2+16-.5;sy=-sn*u+cs*v+H/2+16-.5
            left,top=math.floor(sx),math.floor(sy);fx,fy=sx-left,sy-top
            values=[0.0]*3
            for xx,yy,weight in [(left,top,(1-fx)*(1-fy)),(left+1,top,fx*(1-fy)),(left,top+1,(1-fx)*fy),(left+1,top+1,fx*fy)]:
                assert 0<=xx<image.width and 0<=yy<image.height
                for c,v in enumerate(image.getpixel((xx,yy))):values[c]+=weight*v
            result.putpixel((x,y),tuple(int(v+.5) for v in values))
    return result


def transform(point,pose,zoom=1):
    tx,ty,angle=[v/1000 for v in pose];sn=math.sin(math.radians(angle));cs=math.cos(math.radians(angle))
    x,y=point[0]-W/2,point[1]-H/2
    return [W/2+zoom*(cs*x-sn*y+tx),H/2+zoom*(sn*x+cs*y+ty)]


def desired(poses,n,mode,strength=1000):
    values=[0,0,0]
    if mode['mode']=='smooth':
        radius=mode['radius'];weights=[(j,radius+1-abs(n-j)) for j in range(max(0,n-radius),min(len(poses),n+radius+1))];total=sum(w for _,w in weights)
        values=[F(sum(poses[j][a]*w for j,w in weights),total) for a in range(3)]
    return [F(v*(1000-strength)+d*strength,1000) for v,d in zip(poses[n],values)]


def run(root):
    root=root.resolve();root.mkdir(parents=True,exist_ok=True);sources=root/'sources';out=root/'renders';store=root/'sessions'
    for path in (sources,out,store):path.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';passed=[];cases=[];physical=[];frames=0;samples=0;rejected=0;previews=0
    def call(req,error=None,env=None):
        nonlocal rejected
        process=subprocess.run([str(exe)],input=json.dumps(req).encode(),capture_output=True,env=env,timeout=120);result=json.loads(process.stdout)
        if error:
            assert not result['ok'] and result['error']['code']==error,(error,result)
            rejected+=1;return result
        assert result['ok'],result
        return result['result']
    def decode(path,audio=False):return subprocess.check_output(['ffmpeg','-v','error','-i',str(path),*(['-vn','-f','s16le'] if audio else ['-an','-pix_fmt','rgb24','-f','rawvideo']),'-'],timeout=120)
    def render(scene,name,error=None):return call({'command':'scene.render','scene':scene,'input_root':str(sources),'output_root':str(out),'output':str(out/(name+'.mkv'))},error)
    def check(scene,name):
        nonlocal frames,samples
        info=call({'command':'scene.inspect','scene':scene,'input_root':str(sources)});receipt=render(scene,name);count=int(F(scene['duration']['num'],scene['duration']['den'])*25)
        expected=b''.join(reference(scene,sources,n,coverage) for n in range(count));actual=decode(out/(name+'.mkv'));assert len(expected)==len(actual)
        error=max(abs(a-b) for a,b in zip(expected,actual));assert error<=1,(name,error)
        assert decode(out/(name+'.mkv'),True)==bytes(count*1920*4)
        frames+=count;samples+=count*1920;cases.append({'name':name,'frames':count,'maximum_rgb_error':error})
        return receipt,actual,info
    def footage(name,poses,worlds=None):
        frames=[];worlds=worlds or [world(42)]*len(poses)
        for n,(pose,image) in enumerate(zip(poses,worlds)):
            path=sources/f'{name}-{n}.png';camera(image,pose).save(path)
            frames.append({'image':identity(path,sources),'hold':time(1,25),'offset':[0,0],'anchor':[0,0]})
        layer={'id':'camera','canvas':[W,H],'start':time(2,25),'duration':time(len(poses),25),'frames':frames,'timing':'strict','end':'hold_last',
               'transform':{'position':[0,0],'crop':[0,0,W,H],'scale':1,'quarter_turns':0,'opacity':255}}
        return {'schema_version':1,'id':name,'width':W,'height':H,'output_scale':1,'duration':time(len(poses)+4,25),'background':[255,0,255],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    def request(scene,**fields):
        req={'command':'stabilization.inspect','scene':scene,'input_root':str(sources),'layer_id':'camera','model':'rigid',
             'segments':[{'start':time(0),'regions':REGIONS}],
             'tracking':{'search_radius':7,'maximum_step':7,'maximum_acceleration':14,'minimum_correlation_milli':850,'minimum_margin_milli':20,'maximum_frame_change_milli':400},
             'maximum_fit_error_milli':1000,'maximum_roll_mdeg':5000,'smoothing':{'mode':'lock'},'strength_milli':1000,'crop':{'mode':'zoom','maximum_zoom_milli':1400},'sampling':'bilinear'}
        req.update(fields);return req
    def stabilize(scene,name,**fields):
        req=request(scene,**fields);result=call(req);assert call(req)==result
        assert not result['applied'] and result['sources']==[f['image'] for f in scene['layers'][0]['frames']]
        cp=result['compensation'];assert cp==result['scene']['layers'][0]['transform']['spatial']['compensation']
        for n,m in enumerate(result['measurements']):
            assert m['time']==time(n+2,25) and m['layer_time']==time(n,25) and m['source_frame']==selected(scene['layers'][0],n+2)
            assert [sample(cp['translation_'+axis+'_milli'],F(n,25)) for axis in ('x','y')]==m['compensation']['translation_milli']
            assert sample(cp['rotation_mdeg'],F(n,25))==m['compensation']['rotation_mdeg']
        receipt,pixels,info=check(result['scene'],name)
        (root/(name+'-request.json')).write_text(json.dumps(req,indent=2)+'\n',encoding='utf-8');(root/(name+'-result.json')).write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
        return result,receipt,pixels,req
    def quality(result,pixels,poses,name,mode,worlds=None,segments=None,exact=False,strength=1000):
        worlds=worlds or [world(42)]*len(poses);segments=segments or [0,len(poses)];errors=[];before=[];ideal=[];zoom=result['crop']['zoom_milli']/1000
        for n,(pose,m) in enumerate(zip(poses,result['measurements'])):
            a,b=next((a,b) for a,b in zip(segments,segments[1:]) if a<=n<b);target=desired(poses[a:b],n-a,mode,strength)
            measured=m['compensation'];correction=[*measured['translation_milli'],measured['rotation_mdeg']]
            for r in REGIONS:
                point=[r[0]+r[2]/2,r[1]+r[3]/2];wanted=transform(point,target);raw=transform(point,pose);stabilized=transform(raw,correction)
                errors.append(sum((x-y)**2 for x,y in zip(stabilized,wanted)));before.append(sum((x-y)**2 for x,y in zip(raw,wanted)))
            ideal.append(camera(worlds[n],target,zoom).tobytes())
        rms=math.sqrt(sum(errors)/len(errors));raw_rms=math.sqrt(sum(before)/len(before))
        assert rms<=1e-8 if exact else rms<=.25,(name,rms)
        if raw_rms>.1:assert rms<raw_rms*.25,(name,raw_rms,rms)
        active=pixels[2*W*H*3:(2+len(poses))*W*H*3];ideal=b''.join(ideal);mae=sum(abs(a-b) for a,b in zip(active,ideal))/len(ideal)
        assert mae<=8,(name,mae)
        if result['crop']['border_free_source_footprint']:assert all(tuple(active[i:i+3])!=(255,0,255) for i in range(0,len(active),3))
        physical.append({'name':name,'input_motion_rms_pixels':raw_rms,'residual_rms_pixels':rms,'ideal_image_mean_absolute_rgb_error':mae,'zoom_milli':result['crop']['zoom_milli']})

    offsets=[(0,0),(2,-1),(-1,1),(1,2),(-2,1),(0,-2),(2,0),(-1,-1),(1,1),(-2,-1),(0,2),(1,0)]
    translation=[[x*1000,y*1000,0] for x,y in offsets];scene=footage('translation',translation)
    locked=stabilize(scene,'locked-translation',model='translation');quality(locked[0],locked[2],translation,'locked-translation',{'mode':'lock'},exact=True)
    passed.append('stabilization.measured_handheld_and_rendered_lock')
    angles=[0,-1300,1800,2200,-1700,900,-2400,1300,-700,2000,-1600,400]
    rolled=[[x*1000,y*1000,angle] for (x,y),angle in zip(offsets,angles)];roll_scene=footage('roll',rolled)
    rigid=stabilize(roll_scene,'locked-roll');quality(rigid[0],rigid[2],rolled,'locked-roll',{'mode':'lock'})
    stronger=[[x*1000,y*1000,angle*2] for (x,y),angle in zip(offsets,angles)]
    strong_scene=footage('strong-roll',stronger);strong=stabilize(strong_scene,'stronger-roll');quality(strong[0],strong[2],stronger,'stronger-roll',{'mode':'lock'})
    passed.append('stabilization.roll_geometry_and_quality')
    panned=[[x*1000+n*500,y*1000,angle] for n,((x,y),angle) in enumerate(zip(offsets,angles))];pan_scene=footage('pan',panned)
    smoothed=stabilize(pan_scene,'smoothed-pan',smoothing={'mode':'smooth','radius':2});quality(smoothed[0],smoothed[2],panned,'smoothed-pan',{'mode':'smooth','radius':2})
    partial=stabilize(pan_scene,'partial-strength',smoothing={'mode':'smooth','radius':2},strength_milli=500);quality(partial[0],partial[2],panned,'partial-strength',{'mode':'smooth','radius':2},strength=500)
    bypass=stabilize(pan_scene,'zero-strength',strength_milli=0,crop={'mode':'preserve'},sampling='nearest');assert all(m['compensation']=={'translation_milli':[0,0],'rotation_mdeg':0} for m in bypass[0]['measurements'])
    for n,f in enumerate(pan_scene['layers'][0]['frames']):
        with Image.open(sources/f['image']['path']) as im:assert bypass[2][(n+2)*W*H*3:(n+3)*W*H*3]==im.tobytes()
    fractional=copy.deepcopy(roll_scene);fractional['layers'][0]['timing']='sample_start'
    for f in fractional['layers'][0]['frames']:f['hold']=time(3,50)
    fractional_result=stabilize(fractional,'fractional-holds');quality(fractional_result[0],fractional_result[2],[rolled[(n*2)//3] for n in range(12)],'fractional-holds',{'mode':'lock'})
    passed.append('stabilization.smoothing_strength_and_exact_clocks')
    cut_poses=rolled[:6]+translation[:6];cut_worlds=[world(42)]*6+[world(1907)]*6;cut_scene=footage('cut',cut_poses,cut_worlds)
    segments=[{'start':time(0),'regions':REGIONS},{'start':time(6,25),'regions':REGIONS}]
    cut=stabilize(cut_scene,'declared-cut',segments=segments,smoothing={'mode':'smooth','radius':2});quality(cut[0],cut[2],cut_poses,'declared-cut',{'mode':'smooth','radius':2},cut_worlds,[0,6,12])
    assert [m['segment'] for m in cut[0]['measurements']]==[0]*6+[1]*6
    call(request(cut_scene),'TRACKING_UNRELIABLE')
    passed.append('stabilization.cut_resets_and_unreliable_rejection')
    preserve=stabilize(roll_scene,'preserve-edges',crop={'mode':'preserve'});assert preserve[0]['crop']['zoom_milli']==1000 and not preserve[0]['crop']['border_free_source_footprint']
    active=preserve[2][2*W*H*3:14*W*H*3];assert any(tuple(active[i:i+3])==(255,0,255) for i in range(0,len(active),3))
    required=rigid[0]['crop']['minimum_border_free_zoom_milli'];assert required==rigid[0]['crop']['zoom_milli'] and required>1000
    assert abs(rigid[0]['crop']['retained_area_fraction']-(1000/required)**2)<1e-12
    call(request(roll_scene,crop={'mode':'zoom','maximum_zoom_milli':required-1}),'STABILIZATION_CROP_LIMIT')
    authored=copy.deepcopy(roll_scene);l=authored['layers'][0];l['transform'].update(position=[28,8],crop=[4,4,56,40],opacity=187,spatial=spatial(scale_milli=[650,850],rotation_mdeg=12000,sampling='bilinear',flip=[True,False]))
    l['mask']={'rect':[9,6,43,32],'inverted':False,'feather':{'radius':3,'edge':'centered'}}
    for i,frame in enumerate(l['frames']):frame['anchor']=[i%2,i%3]
    transformed=stabilize(authored,'authored-transform',crop={'mode':'preserve'});assert [m['measured'] for m in transformed[0]['measurements']]==[m['measured'] for m in rigid[0]['measurements']]
    animated=copy.deepcopy(authored);sp=animated['layers'][0]['transform']['spatial']
    sp['animation']={'rotation_mdeg':{'keys':[{'time':time(0),'value':-12000,'interpolation':'ease_in_out'},{'time':time(12,25),'value':21000,'interpolation':'hold'}]},
                     'scale_x_milli':{'keys':[{'time':time(0),'value':650,'interpolation':'linear'},{'time':time(12,25),'value':1100,'interpolation':'hold'}], 'retime':{'start':time(0),'rate':time(1),'reverse':True}}}
    stabilize(animated,'animated-authored-transform',crop={'mode':'preserve'})
    trimmed=copy.deepcopy(roll_scene)
    for i,f in enumerate(trimmed['layers'][0]['frames']):
        with Image.open(sources/f['image']['path']) as im:
            path=sources/f'trimmed-{i}.png';im.crop((2,2,W-2,H-2)).save(path)
        f['offset']=[2,2];f['image']=identity(path,sources)
    trimmed_result=stabilize(trimmed,'trimmed-footprint');assert trimmed_result[0]['crop']['zoom_milli']>required
    encodings=[]
    for mode in ('straight','premultiplied'):
        alpha=copy.deepcopy(roll_scene);alpha['layers'][0]['alpha_mode']=mode
        for i,f in enumerate(alpha['layers'][0]['frames']):
            with Image.open(sources/f['image']['path']) as im:
                image=Image.new('RGBA',im.size);image.putdata([tuple(c//3*3 if mode=='straight' else c//3*2 for c in p)+(170,) for p in im.getdata()]);path=sources/f'{mode}-{i}.png';image.save(path)
            f['image']=identity(path,sources)
        encodings.append(stabilize(alpha,'alpha-'+mode))
    assert encodings[0][0]['measurements']==encodings[1][0]['measurements'] and encodings[0][2]==encodings[1][2]
    passed.append('stabilization.crop_tradeoffs_and_authored_mapping')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==63
        tool=next(t for t in catalog if t['name']=='cutbolt_stabilization_inspect');fields={k:v for k,v in rigid[3].items() if k!='command'}
        Draft202012Validator(tool['inputSchema']).validate(fields);assert tool['annotations']['readOnlyHint'];assert client.call('stabilization.inspect',**fields)==rigid[0]
        template=client.call('graphics.instantiate',template={'schema_version':1,'id':'stabilized-template','scene':rigid[0]['scene'],'parameters':[]},values={},instance_id='stabilized-copy',input_root=str(sources))
        assert template['scene']['layers'][0]['transform']['spatial']['compensation']==rigid[0]['compensation']
        project=call({'command':'project.create','id':'stable-edit','width':W,'height':H,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create');common={'store_root':str(store),'project_id':project['id']}
        asset=copy.deepcopy(rigid[1]['asset']);asset['identity']={'sha256':rigid[1]['sha256'],'bytes':(out/'locked-roll.mkv').stat().st_size}
        fields={**common,'expected_revision':0,'request_id':'add','operations':[{'op':'media.add','asset':asset},{'op':'clip.append','clip':{'id':'stable','asset_id':asset['id'],'source_in':time(0),'duration':time(16,25)}}]}
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt;saved=client.call('session.get',**common)
        dest=out/'saved.mkv';call({'command':'render.run','project':saved,'input_root':str(out),'output_root':str(out),'output':str(dest)});assert decode(dest)==rigid[2] and decode(dest,True)==bytes(16*1920*4);frames+=16;samples+=16*1920
        for n in (0,2,7,13,15):
            dest=out/f'preview-{n}.png';client.call('preview.frame',project=saved,input_root=str(out),output_root=str(out),output=str(dest),time=time(n,25))
            with Image.open(dest) as im:assert im.tobytes()==rigid[2][n*W*H*3:(n+1)*W*H*3]
            previews+=1
        dest=out/'range.mkv';call({'command':'preview.range','project':saved,'input_root':str(out),'output_root':str(out),'output':str(dest),'start':time(4,25),'duration':time(8,25)});assert decode(dest)==rigid[2][4*W*H*3:12*W*H*3] and decode(dest,True)==bytes(8*1920*4);frames+=8;samples+=8*1920
        client.call('session.apply',**common,expected_revision=1,request_id='trim',operations=[{'op':'clip.trim','clip_id':'stable','source_in':time(2,25),'duration':time(12,25)}]);client.call('session.undo',**common,expected_revision=2,request_id='undo');assert client.call('session.get',**common)['clips']==saved['clips']
    finally:client.close()
    passed.append('stabilization.mcp_templates_saved_history_previews')
    nonrigid=footage('nonrigid',[[0,0,0]]*3)
    for i,f in enumerate(nonrigid['layers'][0]['frames']):
        if i:
            path=sources/f['image']['path']
            with Image.open(path) as im:
                im.load();patch=im.crop((6,6,18,18));im.paste((15,15,15),(6,6,18,18));im.paste(patch,(10,6));im.save(path)
            f['image']=identity(path,sources)
    call(request(nonrigid),'STABILIZATION_UNRELIABLE')
    for name,pattern in [('flat',lambda x:80),('repeated',lambda x:50 if x%4<2 else 180)]:
        uncertain=footage(name,[[0,0,0]]*3)
        for f in uncertain['layers'][0]['frames']:
            im=Image.new('RGB',(W,H));im.putdata([(pattern(x),)*3 for y in range(H) for x in range(W)]);path=sources/f['image']['path'];im.save(path);f['image']=identity(path,sources)
        call(request(uncertain),'TRACKING_UNRELIABLE')
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()};snapshots={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    for field,value in [('model','projective'),('sampling','area')]:call(request(scene,**{field:value}),'INVALID_JSON')
    for field,value in [('maximum_fit_error_milli',0),('maximum_fit_error_milli',2001),('maximum_roll_mdeg',15001),('strength_milli',1001),('smoothing',{'mode':'smooth','radius':0}),('crop',{'mode':'zoom','maximum_zoom_milli':4001}),('segments',[]),('segments',[{'start':time(1,25),'regions':REGIONS}]),('segments',[{'start':time(0),'regions':REGIONS[:2]}]),('segments',[{'start':time(0),'regions':[REGIONS[0]]*3}]),('segments',[{'start':time(0),'regions':[[4294967295,0,8,8],*REGIONS[1:]]}]),('segments',[{'start':time(0),'regions':REGIONS},{'start':time(0),'regions':REGIONS}]),('segments',[{'start':time(0),'regions':REGIONS},{'start':time(11,25),'regions':REGIONS}])]:call(request(scene,**{field:value}),'INVALID_STABILIZATION')
    call(request(scene,segments=[{'start':time(0),'regions':REGIONS},{'start':time(1,50),'regions':REGIONS}]),'UNALIGNED_TIME')
    call(request(roll_scene,maximum_roll_mdeg=0),'STABILIZATION_UNRELIABLE')
    call(request(roll_scene,maximum_fit_error_milli=1),'STABILIZATION_UNRELIABLE')
    bad=copy.deepcopy(scene);bad['layers'][0]['frames'][0]['image']['sha256']='0'*64;call(request(bad),'MEDIA_CHANGED')
    call(request(rigid[0]['scene']),'INVALID_STABILIZATION')
    large=copy.deepcopy(scene);large['layers'][0]['duration']=time(128,25);large['duration']=time(132,25)
    excessive=request(large,segments=[{'start':time(0),'regions':[[0,0,32,24],[32,0,32,24],[0,24,32,24],[32,24,32,24]]}]);excessive['tracking']['search_radius']=32;call(excessive,'LIMIT_EXCEEDED')
    for field,value,code in [('zoom_milli',999,'INVALID_STABILIZATION'),('center_milli',[32768001,0],'INVALID_STABILIZATION'),('viewport',[0,0,0,10],'INVALID_STABILIZATION')]:
        bad=copy.deepcopy(rigid[0]['scene']);bad['layers'][0]['transform']['spatial']['compensation'][field]=value;render(bad,'invalid-'+field,code)
    render(rigid[0]['scene'],'locked-roll','OUTPUT_EXISTS')
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()} and snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=sources/'roll-0.png';old=source.read_bytes()
    try:call({'command':'scene.render','scene':rigid[0]['scene'],'input_root':str(sources),'output_root':str(out),'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(old)
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()} and snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    assert not list(out.glob('.cutbolt-scene-*'));passed.append('stabilization.validation_and_source_preservation')
    report={'passed':passed,'cases':cases,'physical_quality':physical,'frames_compared':frames,'stereo_sample_frames_compared':samples,'rejected_cases':rejected,'frame_previews':previews,
            'reference':'Known rigid camera poses on original textured planes, exact Fraction target smoothing, independently transformed landmarks, ideal continuous-world frames, Decimal source/authored matrix composition and Fraction alpha filtering. One-unit rendering tolerance, <=0.25px residual motion and >4x motion reduction for rolling fixtures; exact integer translation lock.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output)
