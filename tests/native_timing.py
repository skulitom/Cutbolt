"""Original exact native-rate editing and long-form decoded video/audio clock checks."""
from engine import ENGINE
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
    exe=ENGINE;w,h=32,18;passed=[];cases=[];frames=samples=rejected=0
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
    # Long sequential timelines render as chunks of whole frames, samples and milliseconds, joined by
    # stream copy; frames, samples and container timestamps must match a single exact clock.
    for rate in [F(30000,1001),F(24000,1001)]:
        label='long-'+str(rate).replace('/','-');p=create(rate,60,label)
        segments=[(None,5) if i%17==16 else (((i*7)%11)*5,5) for i in range(150)];p=append(p,rate,segments);path=output/(label+'.mkv')
        fields={'project':p,'input_root':str(sources),'output_root':str(output),'output':str(path)}
        plan=call('render.plan',**fields);assert plan['frames']==750 and len(plan['chunks'])>1,plan.get('chunks')
        call('render.run',**fields);case=compare(path,rate,segments);case['frame_rate']=t(rate);case['chunks']=len(plan['chunks']);cases.append(case)
        wav=output/(label+'.wav')
        call('export.run',project=p,input_root=str(sources),output_root=str(output),output=str(wav),profile='reference',streams='audio')
        expected=b''.join(bytes(int(F(n,1)/rate*48000)*4) if first is None else pcm(int(F(first,1)/rate*48000),int(F(n,1)/rate*48000)) for first,n in segments)
        assert ff(['-i',str(wav),'-f','s16le','-'])==expected
    passed.append('native_timing.long_timelines_render_in_exact_chunks')
    # Placed tracks at native rates: promotion keeps playback, an upper track covers a gap, a
    # dissolve follows its integer equation on the native clock, and H.264 delivers at that rate.
    def video_frames(path,expected):
        nonlocal frames
        raw=ff(['-i',str(path),'-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt','rgb24','-f','rawvideo','-'])
        assert len(raw)==len(expected)*w*h*3,(path,len(raw)//(w*h*3),len(expected))
        for n,want in enumerate(expected):assert raw[n*w*h*3:(n+1)*w*h*3]==want,(path,n)
        frames+=len(expected)
    def dissolve(a,b,k,d,count):
        return bytes((x*(d-k)+y*k+count)//d for x,y in zip(a,b))
    tb=lambda rate:t(F(1)/rate)
    promote={'op':'tracks.edit','edit':{'op':'promote','video_track_id':'v','audio_track_id':'a'}}
    for rate in [F(30000,1001),F(60)]:
        label='tracks-'+str(rate).replace('/','-');p=create(rate,60,label)
        segments=[(5,10),(None,5),(20,15)];p=append(p,rate,segments)
        tracked=call('timeline.apply',project=p,expected_revision=p['revision'],operations=[promote])
        assert tracked['tracks'] and not tracked['clips']
        path=output/(label+'.mkv');plan=call('render.plan',project=tracked,input_root=str(sources),output_root=str(output),output=str(path))
        assert plan['profile']=='reference-tracks-ffv1-pcm-rational-v2' and plan['frames']==30
        call('render.run',project=tracked,input_root=str(sources),output_root=str(output),output=str(path))
        case=compare(path,rate,segments);case['frame_rate']=t(rate);case['placed_tracks']=True;cases.append(case)
        cover={'id':'cover','asset_id':'source','start':t(F(10)/rate),'source_in':t(F(40)/rate),'duration':t(F(5)/rate)}
        covered=call('timeline.apply',project=tracked,expected_revision=tracked['revision'],operations=[
            {'op':'tracks.edit','edit':{'op':'add','track':{'id':'top','kind':'video','locked':False,'enabled':True,'clips':[]}}},
            {'op':'tracks.edit','edit':{'op':'place','track_id':'top','clip':cover,'collision':'reject'}}])
        path=output/(label+'-covered.mkv');call('render.run',project=covered,input_root=str(sources),output_root=str(output),output=str(path))
        video_frames(path,[rgb(n) for n in range(5,15)]+[rgb(n) for n in range(40,45)]+[rgb(n) for n in range(20,35)])
        adjacent=append(create(rate,60,label+'-x'),rate,[(0,10),(30,10)])
        adjacent=call('timeline.apply',project=adjacent,expected_revision=adjacent['revision'],operations=[promote,
            {'op':'tracks.edit','edit':{'op':'transition_set','track_id':'v','transition':{'id':'x','left_id':'clip-0','right_id':'clip-1','before':t(F(2)/rate),'after':t(F(3)/rate),'kind':'dissolve'}}}])
        expected=[]
        for n in range(20):
            if 8<=n<13:expected.append(dissolve(rgb(n),rgb(30+n-10),2*(n-8)+1,10,5))
            else:expected.append(rgb(n) if n<10 else rgb(30+n-10))
        path=output/(label+'-dissolve.mkv');call('render.run',project=adjacent,input_root=str(sources),output_root=str(output),output=str(path))
        video_frames(path,expected)
        target=output/(label+'-dissolve-9.png')
        call('preview.frame',project=adjacent,input_root=str(sources),output_root=str(output),output=str(target),time=t(F(9)/rate))
        with Image.open(target) as image:assert image.convert('RGB').tobytes()==expected[9]
        delivery=output/(label+'.mp4')
        receipt=call('export.run',project=tracked,input_root=str(sources),output_root=str(output),output=str(delivery),profile='h264_aac',streams='audio_video',input_transfer='bt709')
        assert receipt['video']['gop_frames']==2*round(rate) and receipt['video']['level']=='4.0',receipt['video']
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-count_frames','-show_entries','stream=r_frame_rate,nb_read_frames,codec_name','-of','json',str(delivery)]))['streams'][0]
        assert F(probe['r_frame_rate'])==rate and int(probe['nb_read_frames'])==30 and probe['codec_name']=='h264',probe
    passed.append('native_timing.placed_tracks_and_h264_at_native_rates')
    # Scenes at native rates: strict one-frame holds alternate two images on the scene clock, and the
    # compiled asset, opaque or transparent, is placed on a track timeline at the same rate.
    for n in (0,1):Image.frombytes('RGB',(w,h),rgb(n)).save(sources/f'alternate-{n}.png')
    def identity(path):return {'path':path.name,'sha256':sha(path),'bytes':path.stat().st_size}
    for rate,transparent in [(F(30000,1001),False),(F(60),True)]:
        count=30;label='scene-'+str(rate).replace('/','-')
        frames_=[{'image':identity(sources/f'alternate-{n}.png'),'hold':t(F(1)/rate),'offset':[0,0],'anchor':[0,0]} for n in (0,1)]
        scene={'schema_version':1,'id':label,'width':w,'height':h,'output_scale':1,'duration':t(F(count)/rate),'frame_rate':t(rate),'background':[0,0,0],
               'color':'srgb_straight_encoded','audio':None,'transparent':transparent,
               'layers':[{'id':'flip','canvas':[w,h],'start':t(F(0)),'duration':t(F(count)/rate),'frames':frames_,'timing':'strict','end':'loop',
                          'transform':{'position':[0,0],'crop':[0,0,w,h],'scale':1,'quarter_turns':0,'opacity':255}}]}
        compiled=call('scene.render',scene=scene,input_root=str(sources),output_root=str(sources),output=str(sources/(label+'.mkv')))
        assert compiled['frames']==count and compiled['frame_rate']==t(rate) and compiled['samples']==F(count)/rate*48000
        video_frames(sources/(label+'.mkv'),[rgb(n%2) for n in range(count)])
        placed=create(rate,40,label+'-base');placed=append(placed,rate,[(0,count)])
        placed=call('timeline.apply',project=placed,expected_revision=placed['revision'],operations=[promote,{'op':'media.add','asset':compiled['asset']},
            {'op':'tracks.edit','edit':{'op':'add','track':{'id':'title','kind':'video','locked':False,'enabled':True,'clips':[],**({'composite':'alpha_over'} if transparent else {})}}},
            {'op':'tracks.edit','edit':{'op':'place','track_id':'title','clip':{'id':'t','asset_id':label,'start':t(F(0)),'source_in':t(F(0)),'duration':t(F(count)/rate)},'collision':'reject'}}])
        path=output/(label+'-placed.mkv');call('render.run',project=placed,input_root=str(sources),output_root=str(output),output=str(path))
        video_frames(path,[rgb(n%2) for n in range(count)])
    passed.append('native_timing.scenes_at_native_rates')
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
