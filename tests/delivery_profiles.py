"""Original rate-control quality and actual software/CUDA decode compatibility matrix."""
from engine import ENGINE
import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
from fractions import Fraction as F
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import subprocess

import numpy as np
from agents import Client
from jsonschema import Draft202012Validator
from scenes import time

ROOT=Path(__file__).resolve().parents[1]


def run(root,device):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output=root/'sources',root/'output';sources.mkdir();output.mkdir()
    exe=ENGINE;cases=[];bounds=[];passed=[];rejected=0
    w,h,count=320,180,200
    y,x=np.indices((h,w));rng=np.random.default_rng(74011)
    pictures=[]
    for n in range(count):
        gray=np.clip((x//3+y//4+n*3)%180+38+rng.integers(-20,21,(h,w)),0,255).astype('uint8')
        frame=np.repeat(gray[:,:,None],3,axis=2)
        for bit in range(8):frame[-12:,(bit*32):(bit+1)*32]=230 if n&(1<<bit) else 20
        pictures.append(frame)
    expected=np.stack(pictures);(sources/'original.rgb').write_bytes(expected.tobytes())
    ticks=np.arange(count*1920)/48000
    pcm=np.stack((11000*np.sin(2*np.pi*310*ticks)+1500*np.sin(2*np.pi*1900*ticks),9000*np.sin(2*np.pi*710*ticks)+1200*np.sin(2*np.pi*2700*ticks)),axis=1).round().astype('<i2')
    (sources/'original.pcm').write_bytes(pcm.tobytes())

    def ff(args):
        return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=240).stdout

    def sha(path):
        with Path(path).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()

    movie=sources/'source.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate','25','-i',str(sources/'original.rgb'),
        '-f','s16le','-ar','48000','-ac','2','-i',str(sources/'original.pcm'),'-map','0:v:0','-map','1:a:0',
        '-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)])
    originals={p.name:sha(p) for p in sources.iterdir()}

    def call(request,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,env=env,timeout=300)
        v=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and v['error']['code']==error,(error,v);rejected+=1;return v
        assert p.returncode==0 and v['ok'],v
        return v['result']

    project=call({'command':'project.create','id':'delivery-matrix','width':w,'height':h,'frame_rate':time(25)})
    project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':[
        {'op':'media.add','asset':{'id':'source','path':str(movie),'duration':time(count,25),'identity':{'bytes':movie.stat().st_size,'sha256':sha(movie)}}},
        {'op':'clip.append','clip':{'id':'shot','asset_id':'source','source_in':time(0),'duration':time(count,25)}}]})
    (root/'project.json').write_text(json.dumps(project,indent=2),encoding='utf-8')
    controls=[{'mode':'quality','crf':18,'maximum_bitrate':2000000,'buffer_size':4000000},
              {'mode':'quality','crf':26,'maximum_bitrate':2000000,'buffer_size':4000000},
              {'mode':'two_pass','bitrate':500000,'maximum_bitrate':2000000,'buffer_size':4000000},
              {'mode':'two_pass','bitrate':1500000,'maximum_bitrate':2000000,'buffer_size':4000000}]

    def request(name,compatibility,rate,aac=320000):
        return {'command':'export.run','project':project,'input_root':str(sources),'output_root':str(output),'output':str(output/(name+'.mp4')),
                'profile':'h264_aac','streams':'audio_video','input_transfer':'bt709','h264':{'compatibility':compatibility,'rate_control':rate},'aac_bitrate':aac}

    def verify(r,result):
        path=Path(r['output']);settings=r['h264'];profile=settings['compatibility'];rate=settings['rate_control']
        assert result['profile_version']==2 and result['video']['rate_control']==rate and result['audio']['bitrate']==r['aac_bitrate']
        assert result['encoder_passes']==(2 if rate['mode']=='two_pass' else 1)
        assert sha(path)==result['sha256']
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_packets','-of','json',str(path)]))
        v=next(s for s in probe['streams'] if s['codec_type']=='video');a=next(s for s in probe['streams'] if s['codec_type']=='audio')
        assert v['profile']=={'baseline720p':'Constrained Baseline','main_hd':'Main','high_hd':'High'}[profile]
        assert v['level']==(31 if profile=='baseline720p' else 40) and v['pix_fmt']=='yuv420p'
        assert v['has_b_frames']==(0 if profile=='baseline720p' else 2)
        assert F(v['duration_ts'])*F(v['time_base'])==F(count,25)==F(a['duration_ts'])*F(a['time_base'])
        packets=[p for p in probe['packets'] if p['stream_index']==v['index']]
        assert len(packets)==count
        dts=[F(p['dts'])*F(v['time_base']) for p in packets];assert all(b>a for a,b in zip(dts,dts[1:]))
        assert sorted(F(p['pts'])*F(v['time_base']) for p in packets)==[F(n,25) for n in range(count)]
        bit_rate=sum(int(p['size'])*8 for p in packets)/(count/25)
        if rate['mode']=='two_pass':assert abs(bit_rate/rate['bitrate']-1)<=.20,(path,bit_rate,rate)
        # One-second windows are bounded by selected maximum rate plus declared buffer.
        for first in range(count):
            bits=sum(int(p['size'])*8 for i,p in enumerate(packets) if 0<=dts[i]-dts[first]<1)
            assert bits<=rate['maximum_bitrate']+rate['buffer_size']
        cpu=ff(['-i',str(path),'-map','0:v:0','-an','-pix_fmt','yuv420p','-f','rawvideo','-'])
        gpu=ff(['-hwaccel','cuda','-hwaccel_device',str(device),'-hwaccel_output_format','cuda','-i',str(path),'-map','0:v:0','-an',
                '-vf','hwdownload,format=nv12,format=yuv420p','-f','rawvideo','-'])
        assert len(cpu)==len(gpu)==count*w*h*3//2 and cpu==gpu
        decoded=np.frombuffer(ff(['-i',str(path),'-map','0:v:0','-an','-vf','scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd','-pix_fmt','rgb24','-f','rawvideo','-']),dtype='uint8').reshape(expected.shape)
        mse=float(np.mean((decoded.astype(float)-expected)**2));psnr=10*math.log10(255**2/mse)
        assert psnr>=27,(path,psnr)
        for n in range(count):
            for bit in range(8):assert (decoded[n,-6,bit*32+16,0]>128)==bool(n&(1<<bit))
        audio=np.frombuffer(ff(['-i',str(path),'-map','0:a:0','-vn','-f','s16le','-']),dtype='<i2').reshape((-1,2))[:len(pcm)]
        snr=10*math.log10(float(np.sum(pcm.astype(float)**2))/float(np.sum((audio.astype(float)-pcm)**2)))
        assert snr>=28,(path,snr)
        candidate_shifts={shift:float(np.sum((audio[256+shift:len(audio)-256+shift].astype(float)-pcm[256:-256])**2)) for shift in range(-3,4)}
        assert min(candidate_shifts,key=candidate_shifts.get)==0
        record={'name':path.stem,'compatibility':profile,'rate_control':rate,'actual_video_bitrate':bit_rate,'rgb_psnr_db':psnr,'pcm_snr_db':snr,
                'video_frames':count,'presentation_sample_frames':len(pcm),'aac_bitrate':r['aac_bitrate'],'cpu_cuda_yuv_identical':True,'decoded_yuv_sha256':hashlib.sha256(cpu).hexdigest()}
        cases.append(record);(root/'partial-results.json').write_text(json.dumps(cases,indent=2),encoding='utf-8')
        return record

    for profile in ['baseline720p','main_hd','high_hd']:
        profile_cases=[]
        for i,rate in enumerate(controls):
            r=request(f'{profile}-{i}',profile,rate,[192000,256000,320000,320000][i])
            planned=call({**r,'command':'export.inspect'});assert not Path(r['output']).exists() and planned['video']['compatibility']==profile
            result=call(r);profile_cases.append(verify(r,result));assert not list(output.glob('.cutbolt-*'))
        assert profile_cases[0]['rgb_psnr_db']>profile_cases[1]['rgb_psnr_db'] and profile_cases[0]['actual_video_bitrate']>profile_cases[1]['actual_video_bitrate']
        assert profile_cases[3]['rgb_psnr_db']>profile_cases[2]['rgb_psnr_db'] and profile_cases[3]['actual_video_bitrate']>profile_cases[2]['actual_video_bitrate']*2
    passed.extend(['delivery_profiles.quality_two_pass_bitrate_and_audio_matrix','delivery_profiles.actual_software_cuda_compatibility_and_timing'])

    concurrent=[request('concurrent-'+str(i),'high_hd',controls[2+i]) for i in range(2)]
    with ThreadPoolExecutor(max_workers=2) as pool:results=list(pool.map(call,concurrent))
    for i,(r,result) in enumerate(zip(concurrent,results)):
        actual=verify(r,result);earlier=next(c for c in cases if c['name']=='high_hd-'+str(2+i))
        assert actual['decoded_yuv_sha256']==earlier['decoded_yuv_sha256']
    assert not list(output.glob('.cutbolt-*'))
    passed.append('delivery_profiles.concurrent_two_pass_logs_and_decoded_repeatability')

    for profile,width,height in [('baseline720p',128,128),('baseline720p',1280,720),('main_hd',1920,1080),('high_hd',1920,1080)]:
        label=f'{profile}-{width}x{height}';yy,xx=np.indices((height,width))
        original=np.stack([np.repeat((((xx//32+yy//32+n*20)%180)+35).astype('uint8')[:,:,None],3,axis=2) for n in range(4)])
        raw=sources/(label+'.rgb');raw.write_bytes(original.tobytes());clip=sources/(label+'.mkv')
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{width}x{height}','-framerate','25','-i',str(raw),
            '-f','lavfi','-i','anullsrc=r=48000:cl=stereo','-t','0.16','-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(clip)])
        originals.update({p.name:sha(p) for p in [raw,clip]})
        p=copy.deepcopy(project);p.update(width=width,height=height)
        p['assets'][0].update(path=str(clip),duration=time(4,25),identity={'bytes':clip.stat().st_size,'sha256':sha(clip)})
        p['clips'][0]['duration']=time(4,25)
        r={**request(label,profile,controls[0]),'project':p};result=call(r);path=r['output']
        decoded=np.frombuffer(ff(['-i',path,'-map','0:v:0','-an','-vf','scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd','-pix_fmt','rgb24','-f','rawvideo','-']),dtype='uint8').reshape(original.shape)
        error=float(np.mean((decoded.astype(float)-original)**2));psnr=99 if error==0 else 10*math.log10(255**2/error);assert psnr>=30
        cpu=ff(['-i',path,'-map','0:v:0','-an','-pix_fmt','yuv420p','-f','rawvideo','-'])
        gpu=ff(['-hwaccel','cuda','-hwaccel_device',str(device),'-hwaccel_output_format','cuda','-i',path,'-map','0:v:0','-an','-vf','hwdownload,format=nv12,format=yuv420p','-f','rawvideo','-'])
        assert cpu==gpu and len(cpu)==4*width*height*3//2 and result['audio_samples']==4*1920
        bounds.append({'compatibility':profile,'width':width,'height':height,'frames':4,'rgb_psnr_db':psnr,'cpu_cuda_yuv_identical':True})
    for bitrate in [192000,256000,320000]:
        r={**request('audio-'+str(bitrate),'high_hd',controls[0],bitrate),'streams':'audio','input_transfer':None,'h264':None,'output':str(output/f'audio-{bitrate}.m4a')}
        result=call(r);assert result['encoder_passes']==1 and result['audio']['bitrate']==bitrate and result['video'] is None
        audio=np.frombuffer(ff(['-i',r['output'],'-map','0:a:0','-f','s16le','-']),dtype='<i2').reshape((-1,2))[:len(pcm)]
        snr=10*math.log10(float(np.sum(pcm.astype(float)**2))/float(np.sum((audio.astype(float)-pcm)**2)));assert snr>=28
    passed.append('delivery_profiles.geometry_boundaries_and_audio_only_controls')

    # First-pass statistics are real; injected failure is limited to either selected phase.
    helper_source=root/'phase-failure.rs';helper=root/'phase-failure.exe'
    helper_source.write_text(r'''
use std::{env,fs,process::{Command,exit}};
fn main(){let args:Vec<String>=env::args().skip(1).collect();
 if let Some(i)=args.iter().position(|s|s=="-pass") {
  let phase=&args[i+1];let root=env::var("CUTBOLT_PHASE_ROOT").unwrap();
  fs::write(format!("{root}/seen-{phase}.txt"),"observed actual encoder phase").unwrap();
  if phase==&env::var("CUTBOLT_FAIL_PHASE").unwrap(){
   if phase=="2" {let i=args.iter().position(|s|s=="-passlogfile").unwrap();assert!(fs::metadata(format!("{}-0.log",args[i+1])).unwrap().len()>100);}
   eprintln!("original selected-pass failure fixture");exit(75);
  }
 }
 exit(Command::new(env::var("CUTBOLT_PHASE_REAL").unwrap()).args(args).status().unwrap().code().unwrap_or(1));}
''',encoding='utf-8')
    subprocess.run(['rustc','--edition=2024',str(helper_source),'-o',str(helper)],check=True,capture_output=True)
    for phase in ['1','2']:
        marker=root/('phase-'+phase);marker.mkdir()
        env={**os.environ,'CUTBOLT_FFMPEG':str(helper),'CUTBOLT_PHASE_ROOT':str(marker),'CUTBOLT_FAIL_PHASE':phase,'CUTBOLT_PHASE_REAL':shutil.which('ffmpeg')}
        r=request('failed-pass-'+phase,'high_hd',controls[2]);call(r,'TOOL_FAILED',env)
        assert (marker/'seen-1.txt').is_file() and (marker/'seen-2.txt').exists()==(phase=='2')
        assert not Path(r['output']).exists() and not list(output.glob('.cutbolt-*'))
    passed.append('delivery_profiles.first_and_second_pass_failures_clean_owned_statistics')

    r=request('invalid','high_hd',controls[0])
    invalid=[]
    for crf in [0,9,36,255]:
        b=copy.deepcopy(r);b['h264']['rate_control']['crf']=crf;invalid.append((b,'INVALID_EXPORT'))
    for maxrate,buffer in [(99999,200000),(20000001,25000000),(1000000,999999),(1000000,25000001)]:
        b=copy.deepcopy(r);b['h264']['rate_control'].update(maximum_bitrate=maxrate,buffer_size=buffer);invalid.append((b,'INVALID_EXPORT'))
    for rate in [99999,2000001]:
        b=copy.deepcopy(r);b['h264']['rate_control']={**controls[2],'bitrate':rate};invalid.append((b,'INVALID_EXPORT'))
    for bitrate in [0,128000,319999]:invalid.append(({**r,'aac_bitrate':bitrate},'INVALID_EXPORT'))
    invalid.extend([({**r,'streams':'audio','input_transfer':None},'INVALID_EXPORT'),({**r,'streams':'video'},'INVALID_EXPORT'),
                    ({**r,'profile':'reference','input_transfer':None},'INVALID_EXPORT')])
    b=copy.deepcopy(r);b['h264']['rate_control']['bitrate']=500000;invalid.append((b,'INVALID_JSON'))
    b=copy.deepcopy(r);b['h264']['extra']=True;invalid.append((b,'INVALID_JSON'))
    b=copy.deepcopy(r);b['h264']['compatibility']='unspecified';invalid.append((b,'INVALID_JSON'))
    b=copy.deepcopy(r);b['h264']['compatibility']='baseline720p';b['project']['width']=1920;b['project']['height']=1080;invalid.append((b,'INVALID_EXPORT'))
    for b,error in invalid:call({**b,'command':'export.inspect'},error)
    existing=output/'preserve.mp4';existing.write_bytes(b'original occupied output');before=sha(existing)
    call({**r,'output':str(existing)},'OUTPUT_EXISTS');assert sha(existing)==before
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools']
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_export_inspect')
        Draft202012Validator(schema).validate({k:v for k,v in r.items() if k!='command'})
        assert client.call('export.inspect',**{k:v for k,v in r.items() if k!='command'})['profile_version']==2
    finally:client.close()
    assert not Path(r['output']).exists() and not list(output.glob('.cutbolt-*'))
    assert originals=={p.name:sha(p) for p in sources.iterdir()}
    passed.append('delivery_profiles.invalid_combinations_mcp_and_source_preservation')
    device_info=subprocess.run(['nvidia-smi','--query-gpu=index,name,driver_version','--format=csv,noheader'],capture_output=True,check=True,text=True).stdout.strip()
    report={'passed':passed,'cases':cases,'geometry_boundaries':bounds,'rejected':rejected,'hardware_device':device,'hardware_inventory':device_info,
            'ffmpeg':subprocess.check_output(['ffmpeg','-version'],text=True).splitlines()[0],
            'compatibility_scope':'Pinned software decoder and explicitly selected CUDA device; no claim for untested browsers, televisions or phones.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--device',type=int,required=True)
    args=parser.parse_args();run(args.output,args.device)
