"""Original exact native-rate editing and long-form decoded video/audio clock checks."""
import argparse
from bisect import bisect_right
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess
import time as clock

import numpy as np
from PIL import Image
from agents import until
from scenes import time

ROOT=Path(__file__).resolve().parents[1]


def run(root,long_form):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,output,store,queue=[root/n for n in ['sources','output','store','queue']]
    for p in [sources,output,store,queue]:p.mkdir()
    cache=root/'cache';cache.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';w,h=32,18;passed=[];cases=[];frames=samples=rejected=0
    originals={}

    def sha(path):
        with Path(path).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
    def t(value):return time(value.numerator,value.denominator)
    def call(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=600)
        v=json.loads(p.stdout)
        if error:assert p.returncode==1 and v['error']['code']==error,(error,v);rejected+=1;return v
        assert p.returncode==0 and v['ok'],v;return v['result']
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=240).stdout
    def rgb(n):
        y,x=np.indices((h,w));return np.stack(((x*7+n*13)%256,(y*11+n*17)%256,((x+y)*3+n*23)%256),axis=-1).astype('uint8').tobytes()
    def pcm(start,count):
        n=np.arange(start,start+count,dtype='int64');return np.stack(((n*43)%40001-20000,(n*61)%36001-18000),axis=-1).astype('<i2').tobytes()
    def create(rate,count,label):
        movie=sources/(label+'.mkv');video=sources/(label+'.rgb');audio=sources/(label+'.pcm')
        total=F(count,1)/rate*48000;assert total.denominator==1;total=int(total)
        with video.open('wb') as f:
            for n in range(count):f.write(rgb(n))
        with audio.open('wb') as f:
            for start in range(0,total,48000):f.write(pcm(start,min(48000,total-start)))
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate',str(rate),'-i',str(video),
            '-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)])
        for p in [video,audio,movie]:originals[str(p)]=sha(p)
        p=call('project.create',id=label,width=w,height=h,frame_rate=t(rate))
        p=call('timeline.apply',project=p,expected_revision=0,operations=[{'op':'media.add','asset':{'id':'source','path':str(movie),'duration':t(F(count,1)/rate),'identity':{'bytes':movie.stat().st_size,'sha256':sha(movie)}}}])
        return p
    def append(p,rate,segments):
        ops=[]
        for i,(first,count) in enumerate(segments):
            clip={'id':'clip-'+str(i),'source_in':t(F(first or 0,1)/rate),'duration':t(F(count,1)/rate)}
            clip.update({'gap':True} if first is None else {'asset_id':'source'})
            ops.append({'op':'clip.append','clip':clip})
        return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def compare(path,rate,segments):
        nonlocal frames,samples
        timeline=[None if first is None else first+n for first,length in segments for n in range(length)]
        command=['ffmpeg','-v','error','-i',str(path),'-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt','rgb24','-f','rawvideo','-']
        proc=subprocess.Popen(command,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        try:
            for n in timeline:assert proc.stdout.read(w*h*3)==(bytes(w*h*3) if n is None else rgb(n)),(path,n)
            assert proc.stdout.read()==b'';assert proc.wait(timeout=30)==0,proc.stderr.read()
        finally:
            if proc.poll() is None:proc.kill();proc.wait()
        proc=subprocess.Popen(['ffmpeg','-v','error','-i',str(path),'-map','0:a:0','-vn','-f','s16le','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        total=0
        try:
            for first,length in segments:
                start=F(first or 0,1)/rate*48000;count=F(length,1)/rate*48000
                assert start.denominator==count.denominator==1
                start,count=int(start),int(count);total+=count
                for offset in range(0,count,48000):
                    number=min(48000,count-offset);expected=bytes(number*4) if first is None else pcm(start+offset,number)
                    assert proc.stdout.read(number*4)==expected,(path,start,offset)
            assert proc.stdout.read()==b'';assert proc.wait(timeout=30)==0,proc.stderr.read()
        finally:
            if proc.poll() is None:proc.kill();proc.wait()
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_streams','-show_frames','-show_entries','stream=time_base,r_frame_rate:frame=best_effort_timestamp','-of','json',str(path)],timeout=120))
        hint=F(probe['streams'][0]['r_frame_rate'])
        assert hint==rate or (rate!=25 and abs(1/hint-1/rate)<=F(1,1000000000))
        tb=F(probe['streams'][0]['time_base']);assert tb<=F(1,1000)
        for n,frame in enumerate(probe['frames']):
            exact=F(n,1)/rate/tb;quantized=(2*exact.numerator+exact.denominator)//(2*exact.denominator)
            assert frame['best_effort_timestamp']==quantized,(n,frame,quantized)
        assert len(probe['frames'])==len(timeline)
        frames+=len(timeline);samples+=total
        return {'frames':len(timeline),'samples':total,'exact_duration':t(F(len(timeline),1)/rate),'container_time_base':str(tb)}

    for rate in [F(24),F(25),F(30),F(50),F(60),F(24000,1001),F(30000,1001),F(60000,1001)]:
        label='rate-'+str(rate).replace('/','-');p=create(rate,40,label)
        segments=[(5,10),(None,5),(20,15)];p=append(p,rate,segments);path=output/(label+'.mkv')
        fields={'project':p,'input_root':str(sources),'output_root':str(output),'output':str(path)}
        plan=call('render.plan',**fields);assert plan['frames']==30 and plan['samples']==F(30,1)/rate*48000
        receipt=call('render.run',**fields);case=compare(path,rate,segments);case['frame_rate']=t(rate);cases.append(case)
        for index in [0,9,10,14,15,29]:
            target=output/(label+'-frame-'+str(index)+'.png')
            call('preview.frame',project=p,input_root=str(sources),output_root=str(output),output=str(target),time=t(F(index,1)/rate))
            n=([*range(5,15)]+[None]*5+[*range(20,35)])[index]
            with Image.open(target) as image:assert image.convert('RGB').tobytes()==(bytes(w*h*3) if n is None else rgb(n))
        selected=output/(label+'-range.mkv');call('preview.range',project=p,input_root=str(sources),output_root=str(output),output=str(selected),start=t(F(5,1)/rate),duration=t(F(20,1)/rate))
        compare(selected,rate,[(10,5),(None,5),(20,10)])
        if rate.denominator==1001:
            for hit in [False,True]:
                target=output/(label+'-cached-frame-'+str(hit)+'.png')
                cached=call('cache.run',cache_root=str(cache),policy={'max_bytes':20000000,'max_entries':32},task={
                    'type':'frame','project':p,'time':t(F(7,1)/rate),'input_root':str(sources),'output_root':str(output),'output':str(target)})
                assert cached['cache']['hit']==hit
                with Image.open(target) as image:assert image.convert('RGB').tobytes()==rgb(12)
                target=output/(label+'-cached-range-'+str(hit)+'.mkv')
                cached=call('cache.run',cache_root=str(cache),policy={'max_bytes':20000000,'max_entries':32},task={
                    'type':'interval','project':p,'start':t(F(5,1)/rate),'duration':t(F(20,1)/rate),'input_root':str(sources),'output_root':str(output),'output':str(target)})
                assert cached['cache']['hit']==hit;compare(target,rate,[(10,5),(None,5),(20,10)])
            scope=call('scopes.inspect',project=p,input_root=str(sources),time=t(F(7,1)/rate),input_transfer='srgb',missing_tags='use_declared',columns=4)
            pixels=np.frombuffer(rgb(12),dtype='uint8').reshape((-1,3))
            for channel,name in enumerate(['red','green','blue']):assert scope['values'][name]['histogram']==np.bincount(pixels[:,channel],minlength=256).tolist()
            target=output/(label+'-sheet.png')
            call('preview.sheet',project=p,input_root=str(sources),output_root=str(output),output=str(target),spec={
                'times':[t(F(n,1)/rate) for n in [0,10,29]],'columns':3,'tile_width':w,'tile_height':h,'gap':0,'background':[0,0,0]})
            with Image.open(target) as image:
                assert image.size==(w*3,h)
                for i,n in enumerate([5,None,34]):assert image.crop((i*w,0,(i+1)*w,h)).convert('RGB').tobytes()==(bytes(w*h*3) if n is None else rgb(n))
            call('session.create',store_root=str(store),project=p,request_id='create-'+label)
            op=[{'op':'clip.split','clip_id':'clip-0','new_clip_id':'split','offset':t(F(5,1)/rate)}]
            edit={'store_root':str(store),'project_id':label,'expected_revision':0,'request_id':'split','operations':op}
            before=call('session.preview',**{k:v for k,v in edit.items() if k!='request_id'})
            receipt=call('session.apply',**edit);assert call('session.apply',**edit)==receipt and before==receipt['changes']
            saved=call('session.get',store_root=str(store),project_id=label)
            target=output/(label+'-saved.mkv');call('render.run',project=saved,input_root=str(sources),output_root=str(output),output=str(target));compare(target,rate,segments)
            call('session.undo',store_root=str(store),project_id=label,expected_revision=1,request_id='undo')
            restored=call('session.get',store_root=str(store),project_id=label)
            target=output/(label+'-queued.mkv');ticket=call('render.start',job_root=str(queue),request_id=label,render={'project':restored,'input_root':str(sources),'output_root':str(output),'output':str(target)})
            result=until(lambda:call('job.status',job_root=str(queue),job_id=ticket['job_id']),lambda v:v['status'] in {'completed','failed','cancelled','interrupted'},seconds=90)
            assert result['status']=='completed',result;compare(target,rate,segments)
            if rate.numerator in [30000,60000]:
                bad=copy.deepcopy(p);bad['clips'][0]['duration']=t(F(1,1)/rate)
                call('render.plan',error='UNALIGNED_TIME',**{**fields,'project':bad,'output':str(output/(label+'-unaligned.mkv'))})
    for rate,code in [(F(23),'UNSUPPORTED_TIMELINE'),(F(30),'UNSUPPORTED_MEDIA')]:
        bad=copy.deepcopy(p);bad['clips']=[];bad['frame_rate']=t(rate);bad=append(bad,rate,[(0,10)])
        target=output/('mismatched-'+str(rate)+'.mkv')
        call('render.plan',error=code,project=bad,input_root=str(sources),output_root=str(output),output=str(target))
        assert not target.exists()
    passed.extend(['native_timing.fractional_and_integer_rates_decoded_pixels_samples_and_pts','native_timing.fractional_saved_edits_undo_previews_and_queued_output'])
    # Declared VFR input conversion uses its observed source clock, never nominal frame labels.
    create(F(25),75,'vfr-reference')
    variable=sources/'variable.mkv'
    ff(['-i',str(sources/'vfr-reference.mkv'),'-map','0:v:0','-map','0:a:0','-vf','settb=1/1000,setpts=N*40+floor(N/5)*20',
        '-fps_mode','passthrough','-enc_time_base:v','1/1000','-c:v','ffv1','-pix_fmt','bgr0','-c:a','copy',str(variable)])
    originals[str(variable)]=sha(variable)
    v=call('project.create',id='variable',width=w,height=h,frame_rate=t(F(25)))
    v=call('timeline.apply',project=v,expected_revision=0,operations=[{'op':'media.add','asset':{'id':'source','path':str(variable),'duration':t(F(75,25))}}])
    v=append(v,F(25),[(0,20)])
    call('render.plan',error='UNSUPPORTED_MEDIA',project=v,input_root=str(sources),output_root=str(output),output=str(output/'unconverted.mkv'))
    recipe={'schema_version':1,'id':'variable','source':{'file':{'path':'variable.mkv','bytes':variable.stat().st_size,'sha256':sha(variable)},'color':'encoded_rgb'},
            'source_in':t(F(0)),'duration':t(F(1)),'rate':t(F(1)),'reverse':False,'freeze':False,'width':w,'height':h,'audio':'resample'}
    converted=output/'variable-conformed.mkv'
    result=call('media.conform',recipe=recipe,input_root=str(sources),output_root=str(output),output=str(converted))
    pts=[F(n*40+(n//5)*20,1000) for n in range(75)]
    indices=[bisect_right(pts,F(n,25))-1 for n in range(25)];assert result['source_frame_indices']==indices
    assert ff(['-i',str(converted),'-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo','-'])==b''.join(rgb(n) for n in indices)
    v['assets'][0].update(path=str(converted),duration=t(F(1)),identity={'bytes':converted.stat().st_size,'sha256':sha(converted)})
    v['clips']=[];v=append(v,F(25),[(5,15),(None,5)])
    final=output/'variable-edited.mkv';receipt=call('render.run',project=v,input_root=str(root),output_root=str(output),output=str(final))
    assert ff(['-i',str(final),'-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo','-'])==b''.join(rgb(indices[n]) for n in range(5,20))+bytes(5*w*h*3)
    assert ff(['-i',str(final),'-map','0:a:0','-f','s16le','-'])==pcm(5*1920,15*1920)+bytes(5*1920*4)
    assert receipt['frames']==20 and receipt['samples']==38400;frames+=45;samples+=86400
    passed.append('native_timing.vfr_source_clock_conversion_and_rendered_edits')
    long_result=None
    if long_form:
        rate=F(30000,1001);count=54000;p=create(rate,count,'long-native');p=append(p,rate,[(0,count)])
        path=output/'long-native.mkv';start=clock.monotonic()
        receipt=call('render.run',project=p,input_root=str(sources),output_root=str(output),output=str(path))
        long_result=compare(path,rate,[(0,count)]);long_result.update(render_wall_seconds=clock.monotonic()-start,output_sha256=sha(path))
        assert long_result['samples']==86486400 and receipt['samples']==86486400
        passed.append('native_timing.thirty_minute_fractional_render_complete_sync')
    assert originals=={name:sha(name) for name in originals}
    report={'passed':passed,'cases':cases,'frames_compared':frames,'stereo_sample_frames_compared':samples,'rejected':rejected,'long_form':long_result,'sources_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--long-form',action='store_true')
    args=parser.parse_args();run(args.output,args.long_form)
