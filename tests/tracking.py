"""Original known-motion images and an independent geometric/alpha output oracle."""
from engine import ENGINE, MCP_TOOLS
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time
from spatial import reference
from grading import grade

ROOT = Path(__file__).resolve().parents[1]


def coverage(mask, rect, x, y):
    """Fractional pixel-center distances; quantize once to the declared mask grid."""
    left, top, width, height = rect
    value = F(0)
    if width and height:
        distances = [F(x)+F(1,2)-left, left+width-F(x)-F(1,2),
                     F(y)+F(1,2)-top, top+height-F(y)-F(1,2)]
        if 'feather' not in mask:
            value = F(int(all(d > 0 for d in distances)))
        else:
            f = mask['feather']; distance = min(distances)/f['radius']
            value = {'inner': distance, 'centered': (distance+1)/2, 'outer': distance+1}[f['edge']]
            value = min(F(1), max(F(0), value))
    value = F(int(value*65536+F(1,2)),65536)
    return 1-value if mask.get('inverted',False) else value


def spatial(**fields):
    value = {'translate_milli':[0,0], 'scale_milli':[1000,1000], 'rotation_mdeg':0,
             'flip':[False,False], 'pixel_aspect':time(1), 'sampling':'nearest', 'edge':'transparent'}
    value.update(fields)
    return value


def curve(values):
    return {'keys':[{'time':time(n,25),'value':value,'interpolation':'hold'} for n,value in enumerate(values)]}


def run(root):
    root = root.resolve(); root.mkdir(parents=True,exist_ok=True)
    sources=root/'sources'; out=root/'renders'; store=root/'sessions'
    for p in (sources,out,store):p.mkdir()
    exe=ENGINE; passed=[]; cases=[]; rejected=0; frame_count=0; sample_count=0; previews=0
    def call(request,error=None,env=None):
        nonlocal rejected
        process=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,env=env,timeout=120)
        result=json.loads(process.stdout)
        if error:
            assert not result['ok'] and result['error']['code']==error,(request['command'],result,error)
            rejected+=1;return result
        assert result['ok'],result
        return result['result']
    def inspect(scene,error=None):return call({'command':'scene.inspect','scene':scene,'input_root':str(sources)},error)
    def render(scene,name,error=None):return call({'command':'scene.render','scene':scene,'input_root':str(sources),'output_root':str(out),'output':str(out/(name+'.mkv'))},error)
    def decode(path,audio=False):
        return subprocess.check_output(['ffmpeg','-v','error','-i',str(path),*(['-vn','-f','s16le'] if audio else ['-an','-pix_fmt','rgb24','-f','rawvideo']),'-'],timeout=120)
    def check(scene,name):
        nonlocal frame_count,sample_count
        report=inspect(scene); receipt=render(scene,name); oracle=copy.deepcopy(scene)
        for layer in oracle['layers']:layer['transform'].setdefault('spatial',spatial())
        n=int(F(scene['duration']['num'],scene['duration']['den'])*25)
        expected=b''.join(reference(oracle,sources,k,coverage) for k in range(n))
        actual=decode(out/(name+'.mkv')); assert len(actual)==len(expected)
        error=max((abs(a-b) for a,b in zip(actual,expected)),default=0)
        assert error<=1,(name,error)
        assert decode(out/(name+'.mkv'),True)==bytes(n*1920*4)
        cases.append({'name':name,'frames':n,'maximum_rgb_error':error})
        frame_count+=n;sample_count+=n*1920
        return receipt,actual,report

    # Gray integer textures have exactly known translations; trims/anchors are separate metadata.
    rng=random.Random(452719); texture=[rng.randrange(30,191) for _ in range(12*10)]
    positions=[(6+2*i,7+[0,1,2,3,2,1,0,1][i]) for i in range(8)]
    def footage(name,positions,change=None,size=(48,36),patch=(12,10),trim=True,alpha=255):
        frames=[]
        for i,(x,y) in enumerate(positions):
            image=Image.new('RGBA',size,(15,15,15,alpha))
            for yy in range(patch[1]):
                for xx in range(patch[0]):
                    v=texture[yy*patch[0]+xx]
                    if alpha==85:v=v//3*3
                    if change=='brightness':v=v//2+20+i
                    if change=='occluded' and i==4 and xx==0 and yy<2:v=15
                    image.putpixel((x+xx,y+yy),(v,v,v,alpha))
            if callable(change):change(image,i,x,y)
            offset=[1+i%2,2-i%2] if trim else [0,0]
            image=image.crop((offset[0],offset[1],size[0]-1,size[1]-1)) if trim else image
            path=sources/f'{name}-{i}.png';image.save(path)
            frames.append({'image':identity(path,sources),'hold':time(2,25),'offset':offset,'anchor':[i%2,2*(i%2)]})
        layer={'id':'subject','canvas':list(size),'start':time(2,25),'duration':time(2*len(frames),25),'frames':frames,'timing':'strict','end':'hold_last',
               'transform':{'position':[2,1],'crop':[0,0,*size],'scale':1,'quarter_turns':0,'opacity':201}}
        return {'schema_version':1,'id':name,'width':size[0],'height':size[1],'output_scale':1,'duration':time(2*len(frames)+4,25),
                'background':[19,43,97],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    def request(scene,region=None,**fields):
        value={'command':'tracking.inspect','scene':scene,'input_root':str(sources),'layer_id':'subject','model':'translation',
               'region':region or [*positions[0],12,10],'search_radius':4,'maximum_step':3,'maximum_acceleration':4,
               'minimum_correlation_milli':850,'minimum_margin_milli':60,'maximum_frame_change_milli':400,
               'mask':{'rect':[4,5,16,14],'inverted':False,'feather':{'radius':3,'edge':'centered'}}}
        value.update(fields);return value
    def track(scene,positions,name,**fields):
        req=request(scene,region=[*positions[0],12,10],**fields);result=call(req)
        observed=result['observations'];assert len(observed)==2*len(positions)
        assert result['applied'] is False and result['sources']==[f['image'] for f in scene['layers'][0]['frames']]
        expected=copy.deepcopy(scene);mask=copy.deepcopy(req['mask']);values=[[],[]]
        for n,measurement in enumerate(observed):
            pos=positions[n//2];displacement=[pos[i]-positions[0][i] for i in range(2)]
            assert measurement['region']==[*pos,12,10] and measurement['displacement']==displacement,measurement
            assert measurement['layer_time']==time(n,25) and measurement['time']==time(n+2,25) and measurement['source_frame']==n//2
            assert measurement['correlation']>=.85 and measurement['margin']>=.06
            for i in range(2):values[i].append(mask['rect'][i]+displacement[i])
        mask['animation']={'x':curve(values[0]),'y':curve(values[1])};expected['layers'][0]['mask']=mask
        assert result['mask']==mask and result['scene']==expected
        receipt,pixels,report=check(expected,name)
        (root/(name+'-request.json')).write_text(json.dumps(req,indent=2)+'\n',encoding='utf-8')
        (root/(name+'-result.json')).write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
        return result,receipt,pixels,req
    moving=footage('moving',positions)
    result,receipt,pixels,req=track(moving,positions,'tracked-moving')
    assert call(req)==result
    bright=footage('bright',positions,'brightness');track(bright,positions,'tracked-light-change')
    obscured=footage('occluded',positions,'occluded');occ=track(obscured,positions,'tracked-small-occlusion')
    assert .85<occ[0]['observations'][8]['correlation']<.999
    edge_positions=[(0,0),(1,0),(2,1),(3,1),(2,2),(1,1),(0,1),(0,0)]
    edge=footage('boundary',edge_positions,trim=False);track(edge,edge_positions,'tracked-source-boundary')
    semi=footage('semi',positions,alpha=85);semi_result=track(semi,positions,'tracked-semitransparent')
    premult=copy.deepcopy(semi);premult['layers'][0]['alpha_mode']='premultiplied'
    for i,frame in enumerate(premult['layers'][0]['frames']):
        with Image.open(sources/frame['image']['path']) as im:
            associated=Image.new('RGBA',im.size)
            associated.putdata([tuple(c*a//255 for c in (r,g,b))+(a,) for r,g,b,a in im.getdata()])
            path=sources/f'associated-{i}.png';associated.save(path)
        frame['image']=identity(path,sources)
    premult_result=track(premult,positions,'tracked-premultiplied')
    assert premult_result[0]['observations']==semi_result[0]['observations'] and premult_result[2]==semi_result[2]
    passed.append('tracking.known_motion_confidence_and_boundaries')
    # Transform and effect state does not alter the measured source-space trajectory.
    transformed=copy.deepcopy(moving);l=transformed['layers'][0]
    l['transform'].update(position=[18,8],crop=[2,3,36,28],spatial=spatial(translate_milli=[250,-750],scale_milli=[800,1100],rotation_mdeg=14000,sampling='bilinear'))
    l['effects']=[{**grade(),'exposure_milli':400}]
    transformed_result=track(transformed,positions,'tracked-transformed')
    assert transformed_result[0]['observations']==result['observations']
    # Fractional source holds select frames at exact output sample starts.
    fractional=copy.deepcopy(moving);l=fractional['layers'][0]
    l['timing']='sample_start'
    for frame in l['frames']:frame['hold']=time(3,50)
    l['duration']=time(12,25);fractional['duration']=time(16,25)
    fractional_req=request(fractional);fractional_req['maximum_acceleration']=5
    fractional_result=call(fractional_req)
    for n,o in enumerate(fractional_result['observations']):
        idx=(F(n,25)/F(3,50)).numerator//(F(n,25)/F(3,50)).denominator
        assert o['source_frame']==idx and o['region']==[*positions[idx],12,10]
    check(fractional_result['scene'],'tracked-fractional-source-holds')
    passed.append('tracking.exact_clocks_transforms_and_rendered_masks')

    # Independently exercise every feather profile, inverse, empty/outside mask, encoding and blend.
    for mode in ('straight','premultiplied'):
        im=Image.new('RGBA',(16,12))
        for y in range(12):
            for x in range(16):
                a=[0,1,17,85,255][(x+2*y)%5]
                rgb=[255 if (x+y+c)%3 else 0 for c in range(3)]
                if mode=='premultiplied':rgb=[v*a//255 for v in rgb]
                im.putpixel((x,y),(*rgb,a))
        im.save(sources/(mode+'.png'))
    def mask_scene(edge='inner',inverse=False,mode='straight',blend='normal',interpolated=False):
        layer={'id':'chart','canvas':[20,16],'start':time(1,25),'duration':time(4,25),
               'frames':[{'image':identity(sources/(mode+'.png'),sources),'hold':time(4,25),'offset':[2,2],'anchor':[3,2]}],
               'timing':'strict','end':'hold_last','alpha_mode':mode,'blend_mode':blend,
               'transform':{'position':[8,7],'crop':[1,1,18,14],'scale':1,'quarter_turns':0,'opacity':177},
               'mask':{'rect':[5,4,9,7],'inverted':inverse,'feather':{'radius':3,'edge':edge}}}
        if interpolated:layer['transform']['spatial']=spatial(translate_milli=[375,-250],scale_milli=[1400,1200],rotation_mdeg=23000,sampling='bilinear')
        return {'schema_version':1,'id':'mask','width':32,'height':26,'output_scale':1,'duration':time(6,25),'background':[11,59,131],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    for edge_name in ('inner','centered','outer'):
        for inverse in (False,True):
            for blend in ('normal','multiply','screen'):
                results=[]
                for mode in ('straight','premultiplied'):
                    scene=mask_scene(edge_name,inverse,mode,blend,True)
                    results.append(check(scene,f'feather-{edge_name}-{inverse}-{blend}-{mode}')[1])
                assert results[0]==results[1],(edge_name,inverse,blend)
    for q in range(4):
        scene=mask_scene('outer',q%2==1);scene['layers'][0]['transform'].update(quarter_turns=q,position=[16,12],scale=2)
        check(scene,f'feather-integer-quarter-{q}')
    for rect in ([5,4,0,7],[-100,-100,1,1],[-32768,-32768,32768,32768],[0,0,1,1]):
        for inv in (False,True):
            scene=mask_scene('centered',inv);scene['layers'][0]['mask'].update(rect=rect,feather={'radius':4096,'edge':'centered'})
            check(scene,f'feather-bounds-{rect[0]}-{rect[2]}-{inv}')
    animated=mask_scene('inner',True);l=animated['layers'][0]
    l['mask']['animation']={'width':{'keys':[{'time':time(0),'value':0,'interpolation':'ease_in_out'},{'time':time(4,25),'value':12,'interpolation':'hold'}],
                                            'retime':{'start':time(0),'rate':time(1),'reverse':True}},'x':curve([3,4,6,9])}
    l['transform']['spatial']=spatial(sampling='bilinear');check(animated,'feather-animated-empty-inverse')
    shape=mask_scene('centered');l=shape['layers'][0]
    l['frames']=[];l['graphics']={'kind':'shape','shape':'ellipse','rect':[2,2,14,10],'fill':[191,43,73,173]}
    l['transform']['spatial']=spatial(sampling='bilinear',rotation_mdeg=-17000);check(shape,'feather-generated-shape')
    passed.append('tracking.feather_alpha_blends_and_geometric_edges')

    # No uncertain partial output: ambiguous, untextured, occluded, displaced or cut samples fail.
    def copy_patch(im,i,x,y):
        im.paste(im.crop((x,y,x+12,y+10)),(x+16,y))
    ambiguous=footage('ambiguous',[positions[0]]*2,copy_patch)
    # Search includes two identical templates from the seed.
    call(request(ambiguous,search_radius=18,maximum_step=18),'TRACKING_UNRELIABLE')
    def periodic(im,i,x,y):
        for yy in range(im.height):
            for xx in range(im.width):im.putpixel((xx,yy),((40 if xx%2 else 180),)*3+(255,))
    flat=footage('periodic',[positions[0]]*2,periodic);call(request(flat),'TRACKING_UNRELIABLE')
    flat=footage('flat',[positions[0]]*2,lambda im,i,x,y:im.paste((60,60,60,255),(0,0,im.width,im.height)))
    call(request(flat),'TRACKING_UNRELIABLE')
    transparent=footage('transparent',positions,alpha=0);call(request(transparent),'TRACKING_UNRELIABLE')
    hidden=footage('hidden',positions,lambda im,i,x,y:im.paste((15,15,15,255),(x,y,x+12,y+10)) if i==4 else None)
    call(request(hidden),'TRACKING_UNRELIABLE')
    jumped=footage('jumped',[positions[0],(20,8)]);call(request(jumped),'TRACKING_UNRELIABLE')
    step=footage('step',[positions[0],(10,7)]);call(request(step),'TRACKING_UNRELIABLE')
    accel=footage('accel',[positions[0],(9,7),(6,7)]);call(request(accel,maximum_acceleration=2),'TRACKING_UNRELIABLE')
    cut=footage('cut',positions,lambda im,i,x,y:im.paste((255,255,255,255),(0,0,im.width,im.height)) if i==3 else None)
    call(request(cut),'TRACKING_UNRELIABLE')
    # A large rotation is outside the translation model, never silently accepted.
    def turn(im,i,x,y):
        if i==3:im.paste(im.crop((x,y,x+12,y+10)).transpose(Image.Transpose.ROTATE_180),(x,y))
    rotated=footage('rotated',positions,turn);call(request(rotated),'TRACKING_UNRELIABLE')
    call(request(moving,minimum_correlation_milli=1000,minimum_margin_milli=1000),'TRACKING_UNRELIABLE')
    beyond=copy.deepcopy(moving);beyond['layers'][0]['end']='transparent';beyond['layers'][0]['frames']=beyond['layers'][0]['frames'][:2]
    call(request(beyond),'TRACKING_UNRELIABLE')
    passed.append('tracking.ambiguity_occlusion_and_discontinuity_rejection')

    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        tool=next(t for t in catalog if t['name']=='cutbolt_tracking_inspect');fields={k:v for k,v in req.items() if k!='command'}
        assert tool['annotations']['readOnlyHint'] is True
        Draft202012Validator(tool['inputSchema']).validate(fields)
        assert client.call('tracking.inspect',**fields)==result
        copied=client.call('graphics.instantiate',template={'schema_version':1,'id':'track-template','scene':result['scene'],'parameters':[]},values={},instance_id='copy',input_root=str(sources))
        assert copied['scene']['layers'][0]['mask']==result['mask']
        project=call({'command':'project.create','id':'tracked-session','width':48,'height':36,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create');common={'store_root':str(store),'project_id':project['id']}
        asset=copy.deepcopy(receipt['asset']);asset['identity']={'sha256':receipt['sha256'],'bytes':(out/'tracked-moving.mkv').stat().st_size}
        fields={**common,'expected_revision':0,'request_id':'add','operations':[{'op':'media.add','asset':asset},{'op':'clip.append','clip':{'id':'tracked','asset_id':asset['id'],'source_in':time(0),'duration':moving['duration']}}]}
        applied=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==applied
        saved=client.call('session.get',**common);dest=out/'saved.mkv'
        call({'command':'render.run','project':saved,'input_root':str(out),'output_root':str(out),'output':str(dest)})
        assert decode(dest)==pixels and decode(dest,True)==bytes(20*1920*4);frame_count+=20;sample_count+=20*1920
        for n in (0,2,9,17,19):
            dest=out/f'preview-{n}.png';client.call('preview.frame',project=saved,input_root=str(out),output_root=str(out),output=str(dest),time=time(n,25))
            with Image.open(dest) as im:assert im.tobytes()==pixels[n*48*36*3:(n+1)*48*36*3]
            previews+=1
        dest=out/'range.mkv';call({'command':'preview.range','project':saved,'input_root':str(out),'output_root':str(out),'output':str(dest),'start':time(4,25),'duration':time(10,25)})
        assert decode(dest)==pixels[4*48*36*3:14*48*36*3] and decode(dest,True)==bytes(10*1920*4);frame_count+=10;sample_count+=10*1920
        client.call('session.apply',**common,expected_revision=1,request_id='trim',operations=[{'op':'clip.trim','clip_id':'tracked','source_in':time(2,25),'duration':time(16,25)}])
        client.call('session.undo',**common,expected_revision=2,request_id='undo');assert client.call('session.get',**common)['clips']==saved['clips']
    finally:client.close()
    passed.append('tracking.mcp_templates_saved_history_and_previews')

    snapshots={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    for field,value,code in [('search_radius',0,'INVALID_TRACKING'),('search_radius',33,'INVALID_TRACKING'),('maximum_step',5,'INVALID_TRACKING'),('maximum_acceleration',0,'INVALID_TRACKING'),
                             ('minimum_correlation_milli',849,'INVALID_TRACKING'),('minimum_margin_milli',19,'INVALID_TRACKING'),('maximum_frame_change_milli',1001,'INVALID_TRACKING'),
                             ('region',[0,0,3,10],'INVALID_TRACKING'),('region',[0,0,65,10],'INVALID_TRACKING'),('region',[4294967295,0,12,10],'INVALID_TRACKING'),
                             ('layer_id','absent','INVALID_TRACKING'),('model','affine','INVALID_JSON'),('unknown',True,'INVALID_JSON')]:
        bad=copy.deepcopy(req);bad[field]=value;call(bad,code)
    bad=copy.deepcopy(req);bad['mask']['animation']={'x':curve([0,1])};call(bad,'INVALID_TRACKING')
    bad=copy.deepcopy(req);bad['scene']['layers'][0]['duration']=time(129,25);bad['scene']['duration']=time(132,25);call(bad,'INVALID_TRACKING')
    bad=copy.deepcopy(req);bad['scene']['layers'][0]['duration']=time(1,25);call(bad,'INVALID_TRACKING')
    bad=copy.deepcopy(req);bad['search_radius']=32;bad['scene']['layers'][0]['duration']=time(128,25);bad['scene']['duration']=time(132,25);call(bad,'LIMIT_EXCEEDED')
    bad=copy.deepcopy(req);bad['scene']['layers'][0]['frames'][0]['image']['sha256']='0'*64;call(bad,'MEDIA_CHANGED')
    bad=copy.deepcopy(req);bad['scene']['layers'][0]['frames'][0]['image']['path']='../outside.png';call(bad,'INVALID_PATH')
    bad=copy.deepcopy(req);bad['mask']['rect'][0]=32768;call(bad,'INVALID_ANIMATION')
    for radius in (0,4097):
        bad=mask_scene();bad['layers'][0]['mask']['feather']['radius']=radius;render(bad,'invalid-feather-'+str(radius),'INVALID_MASK')
    bad=mask_scene();bad['layers'][0]['mask']['feather']['edge']='wrap';render(bad,'invalid-edge','INVALID_JSON')
    bad=mask_scene();bad['layers'][0]['mask']['feather']['unexpected']=True;render(bad,'unknown-mask-field','INVALID_JSON')
    render(result['scene'],'tracked-moving','OUTPUT_EXISTS')
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    # Changed input after encoding cannot publish a stale tracked render.
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe'
    wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8')
    subprocess.run(['rustc',str(wrapper),'-o',str(tool)],capture_output=True,check=True)
    source=sources/'moving-0.png';saved_bytes=source.read_bytes()
    try:call({'command':'scene.render','scene':result['scene'],'input_root':str(sources),'output_root':str(out),'output':str(out/'changed.mkv')},'MEDIA_CHANGED',
             {**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved_bytes)
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()} and not list(out.glob('.cutbolt-scene-*'))
    passed.append('tracking.validation_atomicity_and_source_preservation')
    report={'passed':passed,'cases':cases,'frames_compared':frame_count,'stereo_sample_frames_compared':sample_count,'rejected_cases':rejected,'frame_previews':previews,
            'reference':'Authored integer subject coordinates, independently selected rational frame clocks, Fraction pixel-center coverage and normalized alpha/blend equations, high-precision forward geometry and rational interpolation; RGB tolerance one unit and exact encoding equivalence. All PCM silence checked. No uncertain partial trajectories.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output)
