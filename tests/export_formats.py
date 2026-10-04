"""Independent decoded export clocks, numbered images, large sizes and publication faults."""
import argparse
import copy
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess

import numpy as np
from PIL import Image
from agents import Client

ROOT=Path(__file__).resolve().parents[1]


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    source=root/'sources';output=root/'output';source.mkdir();output.mkdir()
    exe=ROOT/'target/debug/cutbolt.exe';passed=[];cases=[];frames=samples=rejected=0;originals={}
    def t(n):return {'num':n.numerator,'den':n.denominator}
    def sha(p):
        with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
    def call(command,error=None,env=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=600,env={**os.environ,**(env or {})})
        v=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and (error is True or v['error']['code']==error),(error,v);rejected+=1;return v
        assert p.returncode==0 and v['ok'],v;return v['result']
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=600).stdout
    def rgb(n,w,h):
        y,x=np.indices((h,w),dtype='int32');n=n//3
        return np.stack(((x*31+n*17)%256,(y*7+n*23)%256,((x+y)*11+n*29)%256),axis=-1).astype('uint8').tobytes()
    def pcm(start,count):
        n=np.arange(start,start+count,dtype='int64');return np.stack(((n*37)%30001-15000,(n*71)%40001-20000),axis=-1).astype('<i2').tobytes()
    def create(rate,w,h,label,count=40):
        video=source/(label+'.rgb');audio=source/(label+'.pcm');movie=source/(label+'.mkv')
        with video.open('wb') as f:
            for n in range(count):f.write(rgb(n,w,h))
        duration=F(count)/rate;total=duration*48000;assert total.denominator==1
        audio.write_bytes(pcm(0,int(total)))
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate',str(rate),'-i',str(video),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),
            '-c:v','ffv1','-level','1' if min(w,h)<4 else '3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)])
        for path in [video,audio,movie]:originals[str(path)]=sha(path)
        p=call('project.create',id=label,width=w,height=h,frame_rate=t(rate))
        p=call('timeline.apply',project=p,expected_revision=0,operations=[{'op':'media.add','asset':{'id':'source','path':str(movie),'duration':t(duration),'identity':{'bytes':movie.stat().st_size,'sha256':sha(movie)}}}])
        return p
    def append(p,rate,segments):
        ops=[]
        for i,(first,count) in enumerate(segments):
            c={'id':'clip-'+str(i),'source_in':t(F(first or 0)/rate),'duration':t(F(count)/rate)};c.update({'gap':True} if first is None else {'asset_id':'source'});ops.append({'op':'clip.append','clip':c})
        return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def verify(path,receipt,p,rate,segments,streams,profile):
        nonlocal frames,samples
        w,h=p['width'],p['height'];timeline=[None if first is None else first+n for first,length in segments for n in range(length)]
        expected_hash=hashlib.sha256();count=len(timeline)
        for n in timeline:expected_hash.update(bytes(w*h*3) if n is None else rgb(n,w,h))
        expected_audio=b''
        for first,length in segments:
            start=F(first or 0)/rate*48000;number=F(length)/rate*48000;assert start.denominator==number.denominator==1
            expected_audio+=bytes(int(number)*4) if first is None else pcm(int(start),int(number))
        if profile=='png_sequence':
            manifest=json.loads((path/'manifest.json').read_text());assert manifest['frame_count']==count and manifest['frame_rate']==t(rate) and manifest['duration']==t(F(count)/rate)
            assert sha(path/'manifest.json')==receipt['manifest']['sha256'];assert manifest['decoded_rgb_sha256']==expected_hash.hexdigest()
            first=receipt['sequence_first'];assert manifest['first_number']==first and len(manifest['frames'])==count
            for index,(frame,n) in enumerate(zip(manifest['frames'],timeline)):
                assert frame['number']==first+index and frame['path']==f'frame-{first+index:06}.png'
                item=path/frame['path'];assert sha(item)==frame['sha256'] and item.stat().st_size==frame['bytes']
                with Image.open(item) as image:assert image.mode=='RGB' and image.size==(w,h) and image.tobytes()==(bytes(w*h*3) if n is None else rgb(n,w,h))
            assert len(list(path.iterdir()))==count+1+(streams=='audio_video')
            if streams=='audio_video':assert ff(['-i',str(path/'audio.wav'),'-f','s16le','-'])==expected_audio
            else:assert manifest['audio'] is None
        else:
            if streams!='audio':
                proc=subprocess.Popen(['ffmpeg','-v','error','-i',str(path),'-map','0:v:0','-fps_mode','passthrough','-pix_fmt','rgb24','-f','rawvideo','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                try:
                    for n in timeline:assert proc.stdout.read(w*h*3)==(bytes(w*h*3) if n is None else rgb(n,w,h))
                    assert proc.stdout.read()==b'' and proc.wait(timeout=30)==0,proc.stderr.read()
                finally:
                    if proc.poll() is None:proc.kill();proc.wait()
                meta=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_frames','-show_streams','-of','json',str(path)],timeout=120))
                tb=F(meta['streams'][0]['time_base']);assert len(meta['frames'])==count
                for n,f in enumerate(meta['frames']):
                    ideal=F(n)/rate/tb;nearest=(2*ideal.numerator+ideal.denominator)//(2*ideal.denominator)
                    assert f['best_effort_timestamp']==nearest
                if profile=='png_mov':
                    stream=meta['streams'][0]
                    assert stream['codec_name']=='png' and F(stream['duration_ts'])*tb==F(count)/rate
                    assert stream['color_range']=='pc' and stream['color_space']=='gbr' and stream['color_primaries']=='bt709'
                    assert stream['color_transfer']=={'srgb':'iec61966-2-1','bt709':'bt709'}[receipt['input_transfer']]
            if streams!='video':assert ff(['-i',str(path),'-map','0:a:0','-f','s16le','-'])==expected_audio
        assert receipt['video_frames']==(0 if streams=='audio' else count) and receipt['audio_samples']==(0 if streams=='video' else len(expected_audio)//4)
        assert receipt['frame_rate']==t(rate)
        if streams!='audio':frames+=count
        if streams!='video':samples+=len(expected_audio)//4
    def export(p,rate,segments,label,profile,streams='audio_video',selected=None,first=None,transfer='srgb'):
        extension={'reference':'wav' if streams=='audio' else 'mkv','png_mov':'mov','png_sequence':'frames'}[profile]
        path=output/(label+'.'+extension);req={'project':p,'input_root':str(source),'output_root':str(output),'output':str(path),'profile':profile,'streams':streams}
        if profile!='reference':req['input_transfer']=transfer
        if selected:req['range']=selected
        if first is not None:req['sequence_first']=first
        before=call('export.inspect',**req);assert not path.exists()
        result=call('export.run',**req);assert result['timeline_frames']==before['timeline_frames'];verify(path,result,p,rate,segments,streams,profile)
        cases.append({'profile':profile,'streams':streams,'rate':t(rate),'dimensions':[p['width'],p['height']],'frames':len([n for _,length in segments for n in range(length)])})
        return req,result
    for rate in map(F,[24,25,30,50,60,'24000/1001','30000/1001','60000/1001']):
        label='rate-'+str(rate).replace('/','-');p=create(rate,48,26,label);p=append(p,rate,[(0,10),(None,5),(20,15)])
        selected={'start':t(F(5)/rate),'duration':t(F(20)/rate)};segments=[(5,5),(None,5),(20,10)]
        for profile in ['reference','png_mov','png_sequence']:last,_=export(p,rate,segments,label+'-'+profile,profile,selected=selected,first=37 if profile=='png_sequence' else None)
        if rate==F(30000,1001):
            for profile,streams in [('reference','audio'),('png_mov','video'),('png_sequence','video')]:export(p,rate,segments,label+'-'+profile+'-'+streams,profile,streams,selected=selected,transfer='bt709')
    passed.append('formats.eight_native_rates_mov_mkv_wav_and_ordered_png_ranges')
    for w,h in [(1,1),(127,73),(1920,1080),(1080,1920),(3840,2160),(4096,2160)]:
        rate=F(25);label=f'size-{w}-{h}';p=create(rate,w,h,label,10);p=append(p,rate,[(0,5)])
        for profile in ['png_mov','png_sequence']:export(p,rate,[(0,5)],label+'-'+profile,profile)
    passed.append('formats.large_portrait_odd_and_single_pixel_lossless_outputs')
    # Two actual simultaneous writers must leave exactly one complete directory.
    rate=F(25);p=create(rate,48,26,'faults');p=append(p,rate,[(0,30)])
    unreduced=copy.deepcopy(p);unreduced['frame_rate']={'num':2500000000000,'den':100000000000}
    export(unreduced,rate,[(0,30)],'unreduced-rate','png_mov')
    previous=output;output=output/'Unicode Ω %25';output.mkdir()
    export(p,rate,[(0,30)],'numbered % images','png_sequence')
    export(p,rate,[(0,30)],'last-number','png_sequence',first=999970)
    output=previous
    req={'project':p,'input_root':str(source),'output_root':str(output),'output':str(output/'race.frames'),'profile':'png_sequence','streams':'audio_video','input_transfer':'srgb'}
    def raw():return subprocess.run([str(exe)],input=json.dumps({'command':'export.run',**req}).encode(),capture_output=True,timeout=600)
    with ThreadPoolExecutor(max_workers=2) as pool:results=list(pool.map(lambda _:raw(),range(2)))
    assert sorted(r.returncode for r in results)==[0,1]
    winner=json.loads(next(r.stdout for r in results if r.returncode==0))['result'];verify(Path(req['output']),winner,p,rate,[(0,30)],'audio_video','png_sequence')
    loser=json.loads(next(r.stdout for r in results if r.returncode==1));assert loser['error']['code'] in {'PUBLISH_FAILED','OUTPUT_EXISTS'}
    adapter=root/'format_failure_tool.exe';subprocess.run(['rustc','--edition=2024',str(ROOT/'tests/format_failure_tool.rs'),'-o',str(adapter)],check=True,capture_output=True)
    movie=Path(p['assets'][0]['path']);original=movie.read_bytes()
    for mode in ['missing','corrupt','swapped','failure','occupied','changed']:
        target=output/(mode+'.frames');fields={**req,'output':str(target)}
        env={'CUTBOLT_FFMPEG':str(adapter),'CUTBOLT_FORMAT_REAL':shutil.which('ffmpeg'),'CUTBOLT_FORMAT_ROOT':str(output),'CUTBOLT_FORMAT_MODE':mode,'CUTBOLT_FORMAT_DESTINATION':str(target),'CUTBOLT_FORMAT_SOURCE':str(movie),'CUTBOLT_FORMAT_SOURCES':str(source)}
        try:call('export.run',error=True,env=env,**fields)
        finally:
            if mode=='changed':movie.write_bytes(original)
        if mode=='occupied':assert [x.name for x in target.iterdir()]==['preserve.txt'] and (target/'preserve.txt').read_bytes()==b'original occupied destination'
        else:assert not target.exists()
        assert not list(output.glob('.cutbolt-export-*'))
    passed.append('formats.complete_sequence_validation_concurrent_publication_and_failures')
    for change,code in [({'sequence_first':999990},'INVALID_EXPORT'),({'input_transfer':None},'INVALID_EXPORT'),({'streams':'audio'},'INVALID_EXPORT'),({'profile':'png_mov','output':str(output/'numbered.mov'),'sequence_first':1},'INVALID_EXPORT'),({'output':str(root/'outside.frames')},'PATH_OUTSIDE_ROOT')]:
        call('export.inspect',error=code,**{**req,'output':str(output/'invalid.frames'),**change})
    call('export.run',error='OUTPUT_EXISTS',**req)
    occupied=output/'existing-file.frames';occupied.write_bytes(b'original occupied file');before=sha(occupied)
    call('export.run',error='OUTPUT_EXISTS',**{**req,'output':str(occupied)});assert sha(occupied)==before
    client=Client(exe)
    try:
        client.initialize();tools=client.rpc('tools/list')['result']['tools'];assert len(tools)==65
        v=client.call('export.inspect',**{**req,'output':str(output/'mcp.frames')});assert v['profile']=='png_sequence' and v['timeline_frames']==30
    finally:client.close()
    assert originals=={name:sha(name) for name in originals}
    passed.append('formats.typed_inspection_roots_settings_and_source_preservation')
    report={'passed':passed,'cases':cases,'frames_compared':frames,'stereo_sample_frames_compared':samples,'rejected':rejected,'source_files_preserved':len(originals)}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
