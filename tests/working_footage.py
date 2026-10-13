"""Precise footage wiring against prepared scenes, independent byte-alpha/box arithmetic and exports."""
import argparse
import copy
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import struct
import subprocess
import wave

from PIL import Image
from engine import BUILD, ENGINE
from scenes import identity
from tracks import edit, placement, time, track
from sequences import nested, seqedit
from keying import key
from grading import changed
from clip_properties import curve
from working_color import bayer, decode, encode
from tracking import coverage

ROOT=Path(__file__).resolve().parents[1]
W,H,N=32,24,12
STRIDE=W*H*3


def nearest(v):
    return math.floor(v+F(1,2))


def stored(v,bits):
    value=nearest(max(0,min(1,v))*65535)
    return nearest(F(value,257))*257 if bits==8 else value


@lru_cache(maxsize=4096)
def channel(v,bits,light,transfer):
    return stored(decode(v/255,transfer) if light=='linear' else F(v,255),bits)


def imported(data,bits,light,transfer='srgb',alpha=False):
    stride=4 if alpha else 3
    return [[channel(v,bits,light,transfer) for v in data[i:i+3]]+
            [data[i+3]*257 if alpha else 65535] for i in range(0,len(data),stride)]


def over(source,backdrop,bits,opacity=65535):
    # All tested backdrops are opaque. Keep the complete alpha product until storage.
    a=F(source[3]*opacity,65535**2)
    return [stored(F(s,65535)*a+F(d,65535)*(1-a),bits) for s,d in zip(source[:3],backdrop[:3])]+[65535]


def box_reference(source,backdrop,bits,mask=None,byte_alpha=True):
    result=copy.deepcopy(backdrop); quantum=257 if bits==8 else 1
    for n in range(N):
        for y in range(5,13):
            for x in range(0,10):
                taps=[]
                for yy in range(2):
                    for xx in range(2):
                        sx,sy=2+(x+2)*2+xx,2+(y-5)*2+yy
                        color=source[(n*H+sy)*W+sx]
                        weight=coverage(mask,mask['rect'],sx,sy) if mask else F(1)
                        taps.append((color,F(color[3],65535)*weight))
                a=sum(a for _,a in taps)
                if not a:continue
                rgb=[math.floor(sum(p[c]*a for p,a in taps)/a/quantum)*quantum for c in range(3)]
                alpha=math.floor(a*65535/4/quantum)*quantum
                opacity=137*257
                if byte_alpha:alpha=nearest(F(alpha*opacity*255,65535**2))*257;opacity=65535
                at=(n*H+y)*W+x;result[at]=over(rgb+[alpha],backdrop[at],bits,opacity)
    return result


def delivery(pixels,bits,light,transfer='srgb',width=W):
    ranks=bayer(8);out=bytearray()
    for i,p in enumerate(pixels):
        values=[v/65535 for v in p[:3]]
        if light=='linear':values=[encode(v,transfer) for v in values]
        threshold=(ranks[(i//width)%8][i%8]+.5)/64
        out.extend(max(0,min(255,math.floor(v*255+threshold))) for v in values)
    return bytes(out)


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    sources,outputs,jobs,cache=[root/p for p in ['sources','outputs','jobs','cache']]
    for p in [sources,outputs,jobs,cache]:p.mkdir(parents=True)
    env={**os.environ,'CUTBOLT_CONFORM_CACHE':str(cache)}
    subprocess.run(['cargo','build','--locked','--example','working_scenes_fixture'],cwd=ROOT,check=True)
    helper=BUILD/'examples'/('working_scenes_fixture.exe' if os.name=='nt' else 'working_scenes_fixture')
    cases=[];extras=[];conform_recipes={}
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],check=True,capture_output=True,timeout=180).stdout
    def call(command,error=None,custom_env=None,**fields):
        request=dict(command=command,**fields)
        result=subprocess.run([str(ENGINE)],input=json.dumps(request).encode(),capture_output=True,env=custom_env or env,timeout=300)
        response=json.loads(result.stdout)
        if error:
            assert result.returncode and response['error']['code']==error,(error,response)
            return response['error']
        if result.returncode or not response['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert result.returncode==0 and response['ok'],response
        return response['result']
    def apply(p,ops):return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def decoded(path,audio=False):return ff(['-i',str(path),*(['-vn','-f','s16le','-c:a','pcm_s16le'] if audio else ['-an','-pix_fmt','rgb24','-f','rawvideo']),'-'])
    def queued(name,command,arguments):
        ticket=call('job.start',job_root=str(jobs),request_id=name,run=command,arguments=arguments)
        assert call('job.start',job_root=str(jobs),request_id=name,run=command,arguments=arguments)['job_id']==ticket['job_id']
        for _ in range(12):
            state=call('job.wait',job_root=str(jobs),job_id=ticket['job_id'],timeout_seconds=30,detail='full')
            if state['status'] in ['completed','failed','cancelled']:break
        assert state['status']=='completed',state
        return state
    pcm=b''.join(struct.pack('<hh',(i*31)%16001-8000,(i*47)%18001-9000) for i in range(N*1920))
    (sources/'sound.pcm').write_bytes(pcm)
    with wave.open(str(sources/'sound.wav'),'wb') as sound:
        sound.setnchannels(2);sound.setsampwidth(2);sound.setframerate(48000);sound.writeframes(pcm)
    raw={
        'background':bytes(c for n in range(N) for y in range(H) for x in range(W) for c in ((x*7+n*11)%256,(y*9+n*3)%256,(x+y+n*17)%256)),
        'reference':bytes(c for n in range(N) for y in range(H) for x in range(W) for c in ((0,255,0) if x<8 else ((x*11+n*17)%256,(y*13+n*5)%256,(x*7+y*3+n*19)%256))) }
    files={}
    for name,pixels in raw.items():
        path=sources/(name+'.mkv');data=sources/(name+'.rgb');data.write_bytes(pixels)
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(data),'-f','s16le','-ar','48000','-ac','2','-i',str(sources/'sound.pcm'),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le',str(path)])
        assert decoded(path)==pixels;files[name]=path
    for codec,encoder,pixel_format in [('h264','libx264','yuv420p'),('hevc','libx265','yuv420p'),('hevc10','libx265','yuv420p10le')]:
        path=sources/(codec+'.mp4')
        settings=['-x265-params','log-level=error:pools=1:frame-threads=1:bframes=2:keyint=8'] if encoder=='libx265' else ['-g','8','-bf','2']
        ff(['-i',str(files['reference']),'-vf','scale=in_range=pc:out_range=tv:out_color_matrix=bt709','-c:v',encoder,'-crf','18',*settings,'-pix_fmt',pixel_format,'-c:a','aac','-ar','48000','-color_range','tv','-colorspace','bt709','-color_primaries','bt709','-color_trc','bt709',str(path)])
        files[codec]=path
        conformed=sources/(codec+'-oracle.mkv')
        conform_recipes[codec]=dict(schema_version=1,id='presenter',source=dict(file=identity(path,sources),sdr=dict(matrix='bt709',range='limited',transfer='bt709',missing_tags='reject'),clock='first_video_frame'),source_in=time(0),duration=time(N,25),rate=time(1),reverse=False,freeze=False,width=W,height=H,audio='resample',working_transfer='srgb',frame_rate=time(25))
        call('media.conform',input_root=str(sources),output_root=str(sources),output=str(conformed),recipe=conform_recipes[codec])
        raw[codec]=decoded(conformed)
    frames={}
    for name,pixels in raw.items():
        frames[name]=[]
        for n in range(N):
            path=sources/f'{name}-{n}.png';Image.frombytes('RGB',(W,H),pixels[n*STRIDE:(n+1)*STRIDE]).save(path)
            frames[name].append(dict(image=identity(path,sources),hold=time(1,25),offset=[0,0],anchor=[0,0]))
    table=sources/'look.cube';table.write_text('LUT_1D_SIZE 2\n0 0.1 0.2\n1 0.8 0.6\n',encoding='ascii')
    rgba=bytes(v for i in range(N*W*H) for v in (*raw['reference'][i*3:i*3+3],[0,1,17,128,254,255][i%6]))
    alpha_raw=sources/'alpha.rgba';alpha_raw.write_bytes(rgba);alpha_file=sources/'alpha.mkv'
    ff(['-f','rawvideo','-pixel_format','rgba','-video_size',f'{W}x{H}','-framerate','25','-i',str(alpha_raw),'-f','s16le','-ar','48000','-ac','2','-i',str(sources/'sound.pcm'),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgra','-threads','1','-c:a','pcm_s16le',str(alpha_file)])
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    lut=dict(kind='lut',file=identity(table,sources),interpolation='linear')
    end=F(N,25);identity_transform=dict(position=[0,0],crop=[0,0,W,H],scale=1,quarter_turns=0,opacity=255)
    opacity=dict(kind='opacity',value=199,curve=curve([(0,31,'linear'),(end,241,'hold')]))
    mask=dict(kind='mask',mask=dict(rect=[3,2,20,18],inverted=False,feather=dict(radius=3,edge='centered'),animation=dict(x=curve([(0,-3,'linear'),(end,13,'hold')]))))
    spatial=dict(kind='transform',value={**identity_transform,'position':[4,3],'spatial':dict(translate_milli=[350,-250],scale_milli=[650,700],rotation_mdeg=7000,flip=[False,True],pixel_aspect=time(1),sampling='bilinear',edge='transparent',animation=dict(translate_x_milli=curve([(0,-500,'linear'),(end,2500,'hold')])))},animation=dict(position_y=curve([(0,7,'linear'),(end,19,'hold')])),anchor=[4,2])
    grade=changed(animation=dict(exposure_milli=curve([(0,-500,'linear'),(end,1000,'hold')])))
    pip=dict(kind='transform',value={**identity_transform,'position':[24,3],'crop':[2,2,16,12],'quarter_turns':1},anchor=[2,2])
    stacks={'grade-lut':[grade,lut,opacity], 'spatial':[spatial,opacity,mask,dict(kind='blend_mode',mode='screen')],
            'keyed-pip':[key(strength_milli=500),pip,opacity], 'multiply':[opacity,mask,dict(kind='blend_mode',mode='multiply')]}
    snapshots={}; references={}
    def project(codec,bits,light,transfer='srgb',automatic=False):
        p=call('project.create',id='footage-'+codec,width=W,height=H,frame_rate=time(25))
        ops=[dict(op='project.working_space',working_space=dict(bits=bits,light=light))]
        if transfer:ops.append(dict(op='project.transfer',transfer=transfer))
        ops += [dict(op='media.add',input_root=str(sources),asset=dict(id=name,path=str(files[src]),duration=time(N,25))) for name,src in [('background','background'),('presenter',codec)]]
        # Saved conversion recipes pin their normalization transfer. The sRGB chart uses
        # an explicitly sRGB recipe; unmodified automatic BT.709 recipes are tested below.
        if codec=='hevc10' and not automatic:ops[-1]['asset']['decode']=dict(mode='converted',recipe=conform_recipes[codec])
        ops += [edit('create',duration=time(N,25))]
        for name in ['background','presenter']:
            ops += [edit('add',track=track(name,'video')),edit('place',track_id=name,clip=placement(name,name,0,N),collision='reject')]
        p=apply(p,ops)
        if codec in ['h264','hevc']:assert p['assets'][1]['decode']['mode']=='direct'
        if codec=='hevc10':assert p['assets'][1]['decode']['mode']=='converted'
        return p
    def scene(codec,stack,name,pictures=None):
        recipe=dict(schema_version=1,id=name,width=W,height=H,output_scale=1,duration=time(N,25),background=[0,0,0],color='srgb_straight_encoded',audio=None,layers=[])
        for layer_name,source in [('background','background'),('presenter',codec)]:
            layer=dict(id=layer_name,canvas=[W,H],start=time(0),duration=time(N,25),frames=copy.deepcopy((pictures or frames)[source]),timing='strict',end='hold_last',transform=copy.deepcopy(identity_transform),effects=[])
            if layer_name=='presenter':
                for effect in stack:
                    kind=effect['kind']
                    if kind=='transform':
                        layer['transform']=copy.deepcopy(effect['value'])
                        if effect.get('animation'):layer['animation']=copy.deepcopy(effect['animation'])
                        for frame in layer['frames']:frame['anchor']=effect.get('anchor',[0,0])
                    elif kind=='opacity':
                        layer['transform']['opacity']=effect['value']
                        if effect.get('curve'):layer.setdefault('animation',{})['opacity']=effect['curve']
                    elif kind=='mask':layer['mask']=effect['mask']
                    elif kind=='blend_mode':layer['blend_mode']=effect['mode']
                    else:layer['effects'].append(effect)
            recipe['layers'].append(layer)
        return recipe
    def wide(recipe,bits,light,requests=None):
        result=subprocess.run([str(helper),str(sources)],input=json.dumps(dict(scene=recipe,frames=requests or [(n,1) for n in range(N)],context=dict(space=dict(bits=bits,light=light),transfer='srgb'))).encode(),capture_output=True,timeout=120)
        response=json.loads(result.stdout);assert result.returncode==0 and response['ok'],response
        return response['result']['frames']
    def render(p,name,expected,queue=False):
        path=outputs/(name+'.mkv');args=dict(project=p,input_root=str(sources),output_root=str(outputs),output=str(path))
        if queue:queued(name,'export.run',dict(**args,profile='reference',streams='video'))
        else:call('render.run',**args)
        pixels=decoded(path);assert pixels==expected,(name,len(pixels),len(expected),next(((i,a,b) for i,(a,b) in enumerate(zip(pixels,expected)) if a!=b),None))
        cases.append(dict(name=name,frames=len(pixels)//STRIDE,sha256=hashlib.sha256(pixels).hexdigest()))
        return path
    for bits,light in [(8,'linear'),(16,'encoded'),(16,'linear')]:
        for codec in ['reference','h264','hevc','hevc10']:
            label=f'{bits}-{light}-{codec}';base=project(codec,bits,light)
            for name,stack in stacks.items():
                p=apply(base,[edit('clip_effects',clip_id='presenter',effects=stack)])
                expected=b''.join(delivery(frame,bits,light) for frame in wide(scene(codec,stack,label+'-'+name),bits,light))
                references[bits,light,codec,name]=expected;snapshots[bits,light,codec,name]=p
                render(p,label+'-'+name,expected,queue=(codec=='hevc10' and name=='grade-lut'))
            # Slicing before rendering must not restart any source-clock property.
            p=apply(snapshots[bits,light,codec,'spatial'],[edit('remove',clip_ids=['background','presenter'],links='include'),edit('duration',duration=time(6,25)),
                *[edit('place',track_id=s,clip=placement(s,s,0,6,2),collision='reject') for s in ['background','presenter']],
                edit('clip_effects',clip_id='presenter',effects=stacks['spatial']),edit('slip',clip_ids=['background','presenter'],shift=dict(backward=False,amount=time(1,25)),links='include'),
                edit('split',clip_ids=['presenter'],at=time(3,25),links='include',right_clip_ids=[dict(id='presenter',new_id='right')],right_link_ids=[])])
            expected=references[bits,light,codec,'spatial'][3*STRIDE:9*STRIDE]
            render(p,label+'-source-clock',expected)
            png=outputs/(label+'-preview.png');call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(png),time=time(2,25))
            assert Image.open(png).convert('RGB').tobytes()==expected[2*STRIDE:3*STRIDE]
            ranged=outputs/(label+'-range.mkv');call('export.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(ranged),profile='reference',streams='video',range=dict(start=time(1,25),duration=time(3,25)))
            assert decoded(ranged)==expected[STRIDE:4*STRIDE]
        # A direct child definition keeps its precision before a transformed parent instance.
        base=project('h264',bits,light)
        p=apply(base,[edit('remove',clip_ids=['presenter'],links='include'),dict(op='sequence.create',id='child',duration=time(N,25)),
            seqedit('child',edit('add',track=track('v','video'))),seqedit('child',edit('place',track_id='v',clip=placement('source','presenter',0,N),collision='reject')),
            edit('place',track_id='presenter',clip=nested('instance','child',0,N),collision='reject'),edit('clip_effects',clip_id='instance',effects=stacks['spatial'])])
        render(p,f'{bits}-{light}-nested-direct',references[bits,light,'h264','spatial'])
        # Independent, exact block averages and legacy alpha quantization.
        label=f'{bits}-{light}';base=project('reference',bits,light)
        source=imported(raw['reference'],bits,light);backdrop=imported(raw['background'],bits,light)
        old=dict(crop=[2,2,24,16],divisor=2,opacity=137,position=[-2,5])
        sugar=[dict(kind='transform',value={**identity_transform,'position':old['position'],'crop':old['crop']},
                    anchor=[2,2],sampling=dict(filter='box_floor',divisor=2)),dict(kind='opacity',value=137,rounding='byte_alpha')]
        wanted=delivery(box_reference(source,backdrop,bits),bits,light)
        legacy=copy.deepcopy(base);legacy['tracks']['tracks'][1]['composite']='alpha_over'
        render(apply(legacy,[edit('clip_transform',clip_ids=['presenter'],transform=old)]),label+'-legacy-box',wanted)
        render(apply(base,[edit('clip_effects',clip_id='presenter',effects=sugar)]),label+'-sugar-box',wanted)
        feather=dict(rect=[7,4,12,9],inverted=False,feather=dict(radius=3,edge='centered'))
        for byte_alpha in [False,True]:
            stack=copy.deepcopy(sugar);stack[1]['rounding']='byte_alpha' if byte_alpha else 'coverage'
            stack.append(dict(kind='mask',mask=feather))
            wanted=delivery(box_reference(source,backdrop,bits,feather,byte_alpha),bits,light)
            render(apply(base,[edit('clip_effects',clip_id='presenter',effects=stack)]),label+f'-masked-box-{byte_alpha}',wanted)

        # Preview proxies keep source geometry, but delivery dither belongs to the reduced canvas.
        p=copy.deepcopy(snapshots[bits,light,'reference','spatial']);proxy_pictures={}
        for name,codec in [('background','background'),('presenter','reference')]:
            file=sources/(label+'-'+name+'-proxy.mkv')
            generated=call('proxy.generate',project=p,expected_revision=p['revision'],asset_id=name,scale=2,input_root=str(sources),output_root=str(sources),output=str(file))
            p=apply(p,generated['operations']);pixels=decoded(file);pictures=[]
            for n in range(N):
                image=sources/f'{label}-{name}-proxy-{n}.png'
                Image.frombytes('RGB',(W//2,H//2),pixels[n*STRIDE//4:(n+1)*STRIDE//4]).resize((W,H),Image.Resampling.NEAREST).save(image)
                pictures.append(dict(image=identity(image,sources),hold=time(1,25),offset=[0,0],anchor=[0,0]))
            proxy_pictures[codec]=pictures
        wide_proxy=wide(scene('reference',stacks['spatial'],label+'-proxy',proxy_pictures),bits,light,[(5,1)])[0]
        selected=[wide_proxy[(2*y+1)*W+2*x+1] for y in range(H//2) for x in range(W//2)]
        p=apply(p,[dict(op='preview.proxy',scale=2)]);preview=outputs/(label+'-proxy.png')
        call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(5,25))
        assert Image.open(preview).size==(W//2,H//2)
        assert Image.open(preview).convert('RGB').tobytes()==delivery(selected,bits,light,width=W//2)
        render(p,label+'-proxy-final-originals',references[bits,light,'reference','spatial'])
        extras.append(dict(name=label+'-proxy',pixels=len(selected)))

        # Automatic recipes default to BT.709 independently of the project declaration.
        p=project('hevc10',bits,light,transfer=None,automatic=True)
        recipe=p['assets'][1]['decode']['recipe'];assert recipe['working_transfer']=='bt709'
        explicit=sources/(label+'-automatic-oracle.mkv')
        call('media.conform',input_root=str(sources),output_root=str(sources),output=str(explicit),recipe=recipe)
        p=apply(p,[edit('clip_effects',clip_id='presenter',effects=[dict(kind='opacity',value=137)])])
        a=imported(decoded(explicit),bits,light,'bt709');b=imported(raw['background'],bits,light,'bt709')
        wanted=delivery([over(s,d,bits,137*257) for s,d in zip(a,b)],bits,light,'bt709')
        render(p,label+'-automatic-bt709-fallback',wanted,queue=True)
        explicit_transfer=apply(p,[dict(op='project.transfer',transfer='bt709')])
        render(explicit_transfer,label+'-automatic-bt709-declared',wanted)
        invalid=outputs/(label+'-invalid-transfer.mp4')
        call('export.run',error='INVALID_EXPORT',project=p,input_root=str(sources),output_root=str(outputs),output=str(invalid),profile='h264_aac',streams='video',input_transfer='srgb')
        assert not invalid.exists()

        # Generated PCM remains sample-exact across a window spanning the 4096-sample chunk.
        sound_scene=scene('reference',[],label+'-sound')
        sound_scene['audio']=dict(file=identity(sources/'sound.wav',sources),start=time(0),channels='preserve_stereo',resampling='linear',padding='silence')
        p=apply(base,[edit('add',track=track('audio','audio')),edit('place',track_id='audio',
            clip=dict(id='sound',scene=sound_scene,start=time(1,25),source_in=time(2,25),duration=time(9,25)),collision='reject')])
        wanted=bytes(1920*4)+pcm[2*1920*4:11*1920*4]+bytes(2*1920*4)
        movie=render(p,label+'-generated-pcm',delivery(source,bits,light))
        assert decoded(movie,True)==wanted
        wav=outputs/(label+'-generated-range.wav')
        call('export.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(wav),profile='reference',streams='audio',range=dict(start=time(2,25),duration=time(7,25)))
        assert decoded(wav,True)==wanted[2*1920*4:9*1920*4]
        extras.append(dict(name=label+'-generated-pcm',stereo_sample_frames=(N+7)*1920))

        # Native straight alpha must retain coverage independently of the RGB transfer.
        p=copy.deepcopy(base);p['tracks']['tracks'][1]['composite']='alpha_over'
        p=apply(p,[dict(op='media.add',asset=dict(id='alpha',path=str(alpha_file),duration=time(N,25),identity={k:v for k,v in identity(alpha_file,sources).items() if k!='path'})),
            edit('remove',clip_ids=['presenter'],links='include'),edit('place',track_id='presenter',clip=placement('presenter','alpha',0,N),collision='reject')])
        alpha_pixels=imported(rgba,bits,light,alpha=True)
        for rounding in ['coverage','byte_alpha']:
            stack=[dict(kind='opacity',value=137,rounding=rounding),dict(kind='mask',mask=feather)]
            expected=[]
            for i,(s,d) in enumerate(zip(alpha_pixels,backdrop)):
                s=list(s);opacity=137*257
                if rounding=='byte_alpha':s[3]=nearest(F(s[3]*opacity*255,65535**2))*257;opacity=65535
                mask_weight=coverage(feather,feather['rect'],i%W,(i//W)%H)
                # Mask coverage is multiplied before the one final store, not rounded to depth.
                expected.append(over(s,d,bits,opacity*mask_weight))
            render(apply(p,[edit('clip_effects',clip_id='presenter',effects=stack)]),label+'-native-alpha-'+rounding,delivery(expected,bits,light))

    # Cache keys separate working modes, and even a cache hit rechecks effect dependencies.
    preview_cache=root/'previews';preview_cache.mkdir()
    policy=dict(max_bytes=64*1024*1024,max_entries=128)
    for bits,light in [(8,'linear'),(16,'encoded'),(16,'linear')]:
        p=snapshots[bits,light,'h264','grade-lut']
        task=dict(type='frame',project=p,input_root=str(sources),output_root=str(outputs),time=time(5,25))
        for repeat in range(2):
            file=outputs/f'cache-{bits}-{light}-{repeat}.png'
            result=call('cache.run',cache_root=str(preview_cache),policy=policy,task={**task,'output':str(file)})
            assert result['cache']['hit']==(repeat==1)
            assert Image.open(file).convert('RGB').tobytes()==references[bits,light,'h264','grade-lut'][5*STRIDE:6*STRIDE]
    saved=table.read_bytes();denied=outputs/'changed-lut.png'
    try:
        table.write_bytes(saved+b'# changed owned fixture\n')
        call('cache.run',error='MEDIA_CHANGED',cache_root=str(preview_cache),policy=policy,task={**task,'output':str(denied)})
        assert not denied.exists()
    finally:table.write_bytes(saved)
    # The real PNG encoder finishes before this owned adapter changes the original LUT.
    # Final full hashing must still reject the completed temporary directory.
    adapter=root/('publication-check.exe' if os.name=='nt' else 'publication-check')
    subprocess.run(['rustc','--edition=2024',str(ROOT/'tests/format_failure_tool.rs'),'-o',str(adapter)],check=True,capture_output=True)
    denied=outputs/'changed-after-encoding.frames'
    fault={**env,'CUTBOLT_FFMPEG':str(adapter),'CUTBOLT_FORMAT_REAL':shutil.which('ffmpeg'),
        'CUTBOLT_FORMAT_ROOT':str(outputs),'CUTBOLT_FORMAT_MODE':'changed','CUTBOLT_FORMAT_SOURCE':str(table),'CUTBOLT_FORMAT_SOURCES':str(sources)}
    # Direct index bindings include the decoder executable's identity, including adapters.
    fault_project=call('timeline.apply',custom_env=fault,project=p,expected_revision=p['revision'],
        operations=[dict(op='media.index',asset_id='presenter',input_root=str(sources))])
    try:
        call('export.run',error='MEDIA_CHANGED',custom_env=fault,project=fault_project,input_root=str(sources),output_root=str(outputs),output=str(denied),profile='png_sequence',streams='video')
        assert table.read_bytes()==saved+b'original changed source' and not denied.exists()
    finally:table.write_bytes(saved)
    assert not list(outputs.glob('.cutbolt-*'))
    for name,digest in originals.items():assert hashlib.sha256((sources/name).read_bytes()).hexdigest()==digest,name
    report=dict(passed=['working_footage.original_and_cached_sources_match_precise_scenes','working_footage.source_clock_mask_transform_key_lut_and_blends',
        'working_footage.preview_range_queue_retry_and_nested_direct','working_footage.independent_box_floor_byte_alpha_and_mask',
        'working_footage.proxy_coordinates_and_delivery_dither','working_footage.bt709_automatic_preparation_and_fallback',
        'working_footage.generated_pcm_windows_and_source_preservation','working_footage.native_straight_alpha_and_opacity_policy',
        'working_footage.cache_context_lut_identity_and_post_encoding_publication'],cases=cases,extras=extras,frames=sum(c['frames'] for c in cases),rgb_tolerance=0,
        oracle='Prepared precise scenes with independent delivery conversion; exact Fraction references for box-floor, masks, byte-alpha, BT.709 source-over. Source inputs from explicit conform; component scene/layer/color arithmetic checked separately.',
        limits='Synthetic SDR sources at25fps in three nondefault working modes; not a speed or complete stage-six claim.')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
