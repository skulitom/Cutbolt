"""Original complete numbered RGBA footage, lossless outputs and long native-clock acceptance."""
from engine import ENGINE
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess
import time

import numpy as np
from PIL import Image, PngImagePlugin
from agents import Client

ROOT=Path(__file__).resolve().parents[1]


def run(root,long_form):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/x for x in ['sources','output','store']]
    for path in [sources,output,store]:path.mkdir()
    exe=ENGINE;w,h=32,18;passed=[];originals={};frames=samples=rejected=0
    def t(f):return {'num':f.numerator,'den':f.denominator}
    def sha(p):
        with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
    def call(command,error=None,**fields):
        nonlocal rejected
        result=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=600);value=json.loads(result.stdout)
        if error:assert result.returncode==1 and (error is True or value['error']['code']==error),(error,value);rejected+=1;return value
        assert result.returncode==0 and value['ok'],value;return value['result']
    def create(label,associated=False):
        entries=[];pixels=[]
        for n in range(5):
            y,x=np.indices((h,w),dtype='int64');a=np.take(np.array([0,1,127,128,254,255]),(x+y+n)%6)
            rgb=np.stack(((x*7+n*29)%256,(y*11+n*43)%256,((x+y)*13+n*17)%256),axis=-1)
            if associated:rgb=(rgb*a[...,None]+127)//255
            rgba=np.concatenate((rgb,a[...,None]),axis=-1).astype('uint8');pixels.append(rgba)
            path=sources/f'{label}-{n+47:06}.png';Image.fromarray(rgba).save(path)
            entries.append({'number':n+47,'image':{'path':path.name,'bytes':path.stat().st_size,'sha256':sha(path)}});originals[str(path)]=sha(path)
        recipe={'schema_version':1,'id':label,'first_number':47,'frames':entries,'frame_rate':t(F(25)),'source_start':1,'source_count':3,'repeat':5,'input_transfer':'srgb','alpha_mode':'premultiplied' if associated else 'straight','profile':'rgba_ffv1'}
        return recipe,pixels
    def expected(recipe,pixels,n):
        p=pixels[recipe['source_start']+n%recipe['source_count']].astype('int64');a=p[...,3:4];rgb=p[...,:3]
        if recipe['profile']=='reference_rgb':
            contribution=rgb*255 if recipe['alpha_mode']=='premultiplied' else rgb*a
            return ((contribution+np.array(recipe['background'])*(255-a)+127)//255).astype('uint8').tobytes()
        if recipe['alpha_mode']=='premultiplied':rgb=np.where(a==0,0,(rgb*255+a//2)//np.maximum(a,1))
        return np.concatenate((rgb,a),axis=-1).astype('uint8').tobytes()
    def verify(path,r,pixels):
        nonlocal frames,samples
        count=r['source_count']*r['repeat'];rate=F(r['frame_rate']['num'],r['frame_rate']['den']);alpha=r['profile']!='reference_rgb';size=w*h*(4 if alpha else 3)
        proc=subprocess.Popen(['ffmpeg','-v','error','-i',str(path),'-map','0:v:0','-fps_mode','passthrough','-pix_fmt','rgba' if alpha else 'rgb24','-f','rawvideo','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            for n in range(count):assert proc.stdout.read(size)==expected(r,pixels,n),(path,n)
            assert not proc.stdout.read() and proc.wait(timeout=30)==0,proc.stderr.read()
        finally:
            if proc.poll() is None:proc.kill();proc.wait()
        number=F(count)/rate*48000;assert number.denominator==1;number=int(number)
        proc=subprocess.Popen(['ffmpeg','-v','error','-i',str(path),'-map','0:a:0','-f','s16le','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        remaining=number*4
        try:
            while remaining:
                block=min(remaining,65536);assert proc.stdout.read(block)==bytes(block);remaining-=block
            assert not proc.stdout.read() and proc.wait(timeout=30)==0,proc.stderr.read()
        finally:
            if proc.poll() is None:proc.kill();proc.wait()
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_frames','-show_streams','-of','json',str(path)],timeout=240));tb=F(probe['streams'][0]['time_base']);assert len(probe['frames'])==count
        for n,frame in enumerate(probe['frames']):
            position=F(n)/rate/tb;assert frame['best_effort_timestamp']==(2*position.numerator+position.denominator)//(2*position.denominator)
        if r['profile']=='rgba_png_mov':assert F(probe['streams'][0]['duration_ts'])*tb==F(count)/rate
        frames+=count;samples+=number
    def compile(r,pixels,name):
        target=output/(name+('.mov' if r['profile']=='rgba_png_mov' else '.mkv'))
        fields={'recipe':r,'input_root':str(sources),'output_root':str(output),'output':str(target)}
        inspected=call('image.sequence.inspect',recipe=r,input_root=str(sources));assert not target.exists()
        receipt=call('image.sequence.compile',**fields);assert receipt['frames']==inspected['frames'];verify(target,r,pixels)
        return target,receipt
    base,pixels=create('straight');pm,associated=create('associated',True)
    saved=None
    for label,r,images in [('straight',base,pixels),('associated',pm,associated)]:
        for rate in [F(25),F(24000,1001),F(30000,1001),F(60000,1001)]:
            for profile in ['rgba_ffv1','rgba_png_mov','reference_rgb']:
                recipe=copy.deepcopy(r);recipe.update(frame_rate=t(rate),profile=profile)
                if profile=='reference_rgb':recipe['background']=[23,65,125]
                path,result=compile(recipe,images,label+'-'+str(rate).replace('/','-')+'-'+profile)
                if label=='straight' and rate==25 and profile=='reference_rgb':saved=(recipe,result)
    passed.append('image_sequence.lossless_straight_associated_rgba_and_color_clocks')
    for bg in [[0,0,0],[255,255,255]]:
        r=copy.deepcopy(base);r.update(profile='reference_rgb',background=bg);compile(r,pixels,'background-'+str(bg[0]))
    recipe,result=saved
    p=call('project.create',id='compiled',width=w,height=h,frame_rate=t(F(25)))
    p=call('timeline.apply',project=p,expected_revision=0,operations=[{'op':'media.add','asset':result['asset']},{'op':'clip.append','clip':{'id':'clip','asset_id':recipe['id'],'source_in':t(F(0)),'duration':result['duration']}}])
    call('session.create',project=p,store_root=str(store),request_id='create')
    edit={'store_root':str(store),'project_id':'compiled','expected_revision':0,'request_id':'split','operations':[{'op':'clip.split','clip_id':'clip','new_clip_id':'tail','offset':t(F(5,25))}]}
    receipt=call('session.apply',**edit);assert call('session.apply',**edit)==receipt
    p=call('session.get',store_root=str(store),project_id='compiled');target=output/'saved.mkv';call('render.run',project=p,input_root=str(output),output_root=str(output),output=str(target));verify(target,recipe,pixels)
    call('session.undo',store_root=str(store),project_id='compiled',expected_revision=1,request_id='undo')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65 and not any(x['name']=='cutbolt_image_sequence_compile' for x in catalog)
        assert client.call('image.sequence.inspect',recipe=base,input_root=str(sources))['complete_sequence_validated']
    finally:client.close()
    passed.append('image_sequence.opaque_composition_saved_edits_and_typed_inspection')
    for change in [{'schema_version':2},{'source_count':0},{'repeat':0},{'source_start':4,'source_count':2},{'repeat':100000},{'background':[0,0,0]},{'first_number':46},{'alpha_mode':'premultiplied'}]:
        r=copy.deepcopy(base);r.update(change);call('image.sequence.inspect',error=True,recipe=r,input_root=str(sources))
    for index in [0,2,4]:
        r=copy.deepcopy(base);r['frames'][index]['image']['sha256']='0'*64;call('image.sequence.inspect',error='MEDIA_CHANGED',recipe=r,input_root=str(sources))
    r=copy.deepcopy(base);r['frames'][2]['number']=99;call('image.sequence.inspect',error='INVALID_IMAGE_SEQUENCE',recipe=r,input_root=str(sources))
    for frame in [2,4]:
        path=sources/base['frames'][frame]['image']['path'];original=path.read_bytes()
        try:
            path.write_bytes(b'original corrupted PNG');r=copy.deepcopy(base);r['frames'][frame]['image'].update(bytes=path.stat().st_size,sha256=sha(path))
            call('image.sequence.compile',error=True,recipe=r,input_root=str(sources),output_root=str(output),output=str(output/'invalid.mkv'));assert not (output/'invalid.mkv').exists()
        finally:path.write_bytes(original)
    tagged=sources/'tagged.png';info=PngImagePlugin.PngInfo();info.add(b'sRGB',b'\x00');Image.fromarray(pixels[0]).save(tagged,pnginfo=info);originals[str(tagged)]=sha(tagged)
    r=copy.deepcopy(base);r['input_transfer']='bt709';r['frames'][0]['image']={'path':tagged.name,'bytes':tagged.stat().st_size,'sha256':sha(tagged)}
    call('image.sequence.inspect',error='INVALID_IMAGE_SEQUENCE',recipe=r,input_root=str(sources))
    large=sources/'staging-limit.png';Image.new('RGBA',(512,512),(2,3,5,127)).save(large);originals[str(large)]=sha(large)
    r=copy.deepcopy(base);r.update(first_number=0,frames=[{'number':0,'image':{'path':large.name,'bytes':large.stat().st_size,'sha256':sha(large)}}],source_start=0,source_count=1,repeat=2500)
    call('image.sequence.inspect',error='LIMIT_EXCEEDED',recipe=r,input_root=str(sources))
    occupied=output/'occupied.mkv';occupied.write_bytes(b'original occupied output');before=sha(occupied)
    fields={'recipe':base,'input_root':str(sources),'output_root':str(output),'output':str(occupied)}
    call('image.sequence.compile',error='OUTPUT_EXISTS',**fields);assert sha(occupied)==before
    call('image.sequence.compile',error='PATH_OUTSIDE_ROOT',**{**fields,'output':str(root/'outside.mkv')})
    source=sources/base['frames'][0]['image']['path'];before=sha(source)
    call('image.sequence.compile',error='OUTPUT_EXISTS',**{**fields,'output_root':str(sources),'output':str(source)});assert sha(source)==before
    r=copy.deepcopy(base);r['frames'][0]['image']['path']='../outside.png'
    call('image.sequence.inspect',error='INVALID_PATH',recipe=r,input_root=str(sources))
    passed.append('image_sequence.complete_numbered_identity_alpha_color_and_boundary_rejections')
    long=None
    if long_form:
        r=copy.deepcopy(base);r.update(frame_rate=t(F(30000,1001)),repeat=18000,profile='rgba_ffv1')
        start=time.monotonic();path,result=compile(r,pixels,'long-transparent');long={'frames':result['frames'],'samples':result['samples'],'duration':result['duration'],'seconds_including_decode':time.monotonic()-start,'sha256':sha(path)}
        assert result['frames']==54000 and result['samples']==86486400
        passed.append('image_sequence.thirty_minute_lossless_alpha_and_exact_silent_audio')
    assert originals=={name:sha(name) for name in originals};assert not list(output.glob('.cutbolt-images-*'))
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'rejected':rejected,'long_form':long,'sources_preserved':len(originals)}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--long-form',action='store_true');args=parser.parse_args();run(args.output,args.long_form)
