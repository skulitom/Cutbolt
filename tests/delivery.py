"""Original range/stream fixtures, exact lossless samples and bounded delivery quality."""
from engine import ENGINE
import argparse
from array import array
import copy
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
import math
import os
from pathlib import Path
import shutil
import subprocess

from agents import Client
from grading import D, linear
from scenes import time
from jsonschema import Draft202012Validator

ROOT=Path(__file__).resolve().parents[1]
W,H,N=192,128,75
PALETTE=[(0,0,0),(255,255,255),(64,64,64),(128,128,128),(192,192,192),(192,32,32),(32,192,32),(32,32,192)]

@lru_cache(None)
def frame(source,n):
    image=bytearray(W*H*3)
    def rect(x,y,w,h,c):
        row=bytes(c)*w
        for iy in range(y,y+h):i=(iy*W+x)*3;image[i:i+w*3]=row
    for block in range(8):rect(block*24,0,24,40,PALETTE[(block+source)%8])
    for y in range(40,H):
        for x in range(W):
            v=16+((x+y+n*2+source*30)%192);i=(y*W+x)*3;image[i:i+3]=bytes((v,v,v))
    rect((n*2)%(W-32),64,32,24,(200,100,40) if source==0 else (40,100,200))
    for bit in range(8):rect(4+bit*8,H-12,8,8,(255,)*3 if (source*80+n)&(1<<bit) else (0,)*3)
    return bytes(image)

def pcm(source):
    result=array('h')
    frequencies=[(437,691),(563,997)][source]
    for n in range(N*1920):
        for f in frequencies:result.append(round(11000*math.sin(2*math.pi*f*n/48000)+1800*math.sin(2*math.pi*f*3*n/48000)))
    return result.tobytes()

@lru_cache(None)
def transfer_byte(value,transfer):
    if transfer=='bt709':return value
    l=linear(value,255);v=D('4.5')*l if l<D('0.018') else D('1.099')*l**D('0.45')-D('0.099')
    return max(0,min(255,int(v*255+D('0.5'))))

def yuv_patch(rgb,transfer):
    r,g,b=[D(transfer_byte(c,transfer))/255 for c in rgb]
    y=D('0.2126')*r+D('0.7152')*g+D('0.0722')*b
    return [int(v+D('0.5')) for v in (16+219*y,128+224*(b-y)/D('1.8556'),128+224*(r-y)/D('1.5748'))]

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/n for n in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    audio=[pcm(i) for i in range(2)];assets=[]
    for i in range(2):
        raw=sources/f'original-{i}.rgb';sound=sources/f'original-{i}.pcm';path=sources/f'source-{i}.mkv'
        raw.write_bytes(b''.join(frame(i,n) for n in range(N)));sound.write_bytes(audio[i])
        subprocess.run(['ffmpeg','-v','error','-nostdin','-n','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),
                        '-f','s16le','-ar','48000','-ac','2','-i',str(sound),'-vf','setsar=1','-c:v','ffv1','-level','3','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-metadata','title=original-fixture-title',str(path)],check=True)
        assets.append({'id':'source-'+str(i),'path':str(path),'duration':time(N,25),'identity':{'bytes':path.stat().st_size,'sha256':hashlib.sha256(path.read_bytes()).hexdigest()}})
    original_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;cases=[];passed=[];frames_checked=0;samples_checked=0;quality=[]
    def call(r,error=None,env=None):
        p=subprocess.run([str(exe)],input=json.dumps(r).encode(),capture_output=True,timeout=240,env=env);v=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and v['error']['code']==error,(error,v)
            return v
        assert p.returncode==0 and v['ok'],v
        return v['result']
    project=call({'command':'project.create','id':'export-edit','width':W,'height':H,'frame_rate':time(25)})
    ops=[{'op':'media.add','asset':a} for a in assets]
    ops += [{'op':'clip.append','clip':{'id':'a','asset_id':'source-0','source_in':time(7,25),'duration':time(40,25)}},
            {'op':'clip.append','clip':{'id':'pause','gap':True,'source_in':time(0),'duration':time(9,25)}},
            {'op':'clip.append','clip':{'id':'b','asset_id':'source-1','source_in':time(11,25),'duration':time(35,25)}},
            {'op':'clip.append','clip':{'id':'a2','asset_id':'source-0','source_in':time(2,25),'duration':time(17,25)}}]
    project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':ops})
    timeline=[(0,n) for n in range(7,47)]+[None]*9+[(1,n) for n in range(11,46)]+[(0,n) for n in range(2,19)]
    expected_audio=b''.join(bytes(1920*4) if item is None else audio[item[0]][item[1]*1920*4:(item[1]+1)*1920*4] for item in timeline)
    def request(name,profile='reference',streams='audio_video',start=0,count=None,transfer=None,p=None):
        ext=('wav' if streams=='audio' else 'mkv') if profile=='reference' else ('m4a' if streams=='audio' else 'mp4')
        result={'command':'export.run','project':p or project,'profile':profile,'streams':streams,'input_root':str(sources),'output_root':str(output),'output':str(output/(name+'.'+ext))}
        if count is not None:result['range']={'start':time(start,25),'duration':time(count,25)}
        if profile=='h264_aac' and streams!='audio':result['input_transfer']=transfer or 'srgb'
        return result
    def decode(path,video):
        options=['-map','0:v:0','-an','-fps_mode','passthrough','-vf','scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd','-pix_fmt','rgb24','-f','rawvideo'] if video else ['-map','0:a:0','-vn','-c:a','pcm_s16le','-f','s16le']
        return subprocess.check_output(['ffmpeg','-v','error','-xerror','-i',str(path),*options,'-'],timeout=90)
    def check(r,start=0,count=None):
        nonlocal frames_checked,samples_checked
        count=len(timeline) if count is None else count
        selected=timeline[start:start+count];expected=expected_audio[start*1920*4:(start+count)*1920*4]
        planned=call({**r,'command':'export.inspect'});assert not Path(r['output']).exists()
        assert planned['source_quality']=='original' and planned['timeline_frames']==count
        result=call(r);path=Path(r['output']);assert result['range']==planned['range'] and result['sources']==planned['sources']
        assert result['sha256']==hashlib.sha256(path.read_bytes()).hexdigest()
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-show_format','-of','json',str(path)]));metadata=probe['streams']
        assert 'title' not in probe['format'].get('tags',{})
        assert len(metadata)==(2 if r['streams']=='audio_video' else 1)
        video=audio_raw=None;metrics={}
        if r['streams']!='audio':
            if r['profile']=='reference':
                video=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-map','0:v:0','-an','-pix_fmt','rgb24','-f','rawvideo','-'])
                assert video==b''.join(bytes(W*H*3) if item is None else frame(*item) for item in selected)
            else:
                video=decode(path,True);assert len(video)==count*W*H*3
                yuv=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-map','0:v:0','-an','-pix_fmt','yuv420p','-f','rawvideo','-'])
                assert len(yuv)==count*W*H*3//2
                transfer=r['input_transfer'];errors=0;patch_error=0;identity_frames=0
                for n,item in enumerate(selected):
                    source=bytes(W*H*3) if item is None else frame(*item)
                    table=bytes(transfer_byte(v,transfer) for v in range(256));ideal=source.translate(table)
                    actual=video[n*W*H*3:(n+1)*W*H*3];errors+=sum((a-b)**2 for a,b in zip(ideal,actual))
                    if item is not None:
                        for bit in range(8):
                            value=actual[((H-8)*W+8+bit*8)*3]
                            assert (value>128)==bool((item[0]*80+item[1])&(1<<bit)),(path,n,bit,value)
                        identity_frames+=1
                    for block in range(8):
                        expected_patch=yuv_patch((0,0,0) if item is None else PALETTE[(block+item[0])%8],transfer)
                        x,y=block*24+12,16;offset=n*W*H*3//2
                        observed=[yuv[offset+y*W+x],yuv[offset+W*H+(y//2)*(W//2)+x//2],yuv[offset+W*H*5//4+(y//2)*(W//2)+x//2]]
                        patch_error=max(patch_error,max(abs(a-b) for a,b in zip(expected_patch,observed)))
                psnr=10*math.log10(255**2/(errors/len(video))) if errors else 99
                assert psnr>=30,(path,'RGB PSNR',psnr)
                assert patch_error<=4,(path,'patch YUV error',patch_error)
                metrics.update(rgb_psnr_db=round(psnr,3),maximum_flat_patch_yuv_error=patch_error,identified_frames=identity_frames)
                v=next(s for s in metadata if s['codec_type']=='video')
                assert all(v[k]==value for k,value in {'codec_name':'h264','profile':'High','level':40,'color_space':'bt709','color_primaries':'bt709','color_transfer':'bt709','color_range':'tv','chroma_location':'left'}.items())
                assert F(v['duration_ts'])*F(v['time_base'])==F(count,25)
                data=path.read_bytes();assert data.index(b'moov')<data.index(b'mdat')
            frames_checked+=count
        if r['streams']!='video':
            audio_raw=decode(path,False);wanted=count*1920
            if r['profile']=='reference':assert audio_raw==expected
            else:
                actual=array('h');actual.frombytes(audio_raw[:len(expected)]);original=array('h');original.frombytes(expected)
                assert len(audio_raw)//4==((wanted+1023)//1024)*1024
                assert result['verification']['audio_tail_padding_samples']==len(audio_raw)//4-wanted
                assert result['verification']['aac_priming_samples']==1024
                power=sum(v*v for v in original);error=sum((a-b)**2 for a,b in zip(actual,original))
                snr=10*math.log10(power/error) if power and error else 99
                if power:
                    assert snr>=28,(path,'audio SNR',snr)
                    # Independent channel and timing check: the unshifted samples must fit best.
                    def mse(lag):
                        lo,hi=16,len(original)-16;return sum((actual[i+lag*2]-original[i])**2 for i in range(lo,hi,2))
                    assert min(range(-4,5),key=mse)==0,(path,'AAC alignment')
                else:assert max((abs(v) for v in actual),default=0)<=1
                metrics.update(audio_snr_db=round(snr,3),audio_tail_padding_samples=len(audio_raw)//4-wanted)
                a=next(s for s in metadata if s['codec_type']=='audio')
                assert a['codec_name']=='aac' and a['profile']=='LC' and a['start_pts']==0
                assert F(a['duration_ts'])*F(a['time_base'])==F(count,25)
            samples_checked+=wanted
        cases.append(path.name)
        if metrics:quality.append({'case':path.name,**metrics})
        return result,video,audio_raw

    reference=check(request('full-reference'))
    check(request('cross-reference',start=33,count=49),33,49)
    for streams in ('video','audio'):
        check(request('full-'+streams,streams=streams))
        check(request('cross-'+streams,streams=streams,start=33,count=49),33,49)
    for name,start,count in [('first',0,1),('last',100,1),('gap',40,9),('cut-gap',39,2),('gap-cut',48,2)]:check(request(name,start=start,count=count),start,count)
    passed.append('delivery.lossless_ranges_and_streams')
    full=check(request('full-delivery','h264_aac'))
    cross=check(request('cross-delivery','h264_aac',start=33,count=49),33,49)
    check(request('cross-bt709','h264_aac',start=33,count=49,transfer='bt709'),33,49)
    for streams in ('video','audio'):check(request('delivery-'+streams,'h264_aac',streams,start=33,count=49),33,49)
    for name,start,count in [('one-frame',0,1),('exact-aac-block',1,8),('seven-frames',2,7),('silent-gap',40,9)]:check(request(name,'h264_aac',start=start,count=count),start,count)
    replay=check(request('repeat-delivery','h264_aac',start=33,count=49),33,49)
    assert replay[1:]==cross[1:]
    passed.append('delivery.h264_aac_color_quality_and_timing')
    passed.append('delivery.aac_delay_padding_and_stream_options')

    # Exercise the declared dimension endpoints independently of the chart-sized oracle.
    for name,w,h,profile,streams in [('full-hd-gap',1920,1080,'h264_aac','audio_video'),('minimum-gap',2,2,'h264_aac','audio_video'),('odd-reference',17,11,'reference','video')]:
        p=call({'command':'project.create','id':name,'width':w,'height':h,'frame_rate':time(25)})
        p=call({'command':'timeline.apply','project':p,'expected_revision':0,'operations':[{'op':'clip.append','clip':{'id':'gap','gap':True,'source_in':time(0),'duration':time(1,25)}}]})
        r=request(name,profile,streams,p=p);receipt=call(r);assert receipt['width']==w and receipt['height']==h and receipt['video_frames']==1
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',r['output'],'-map','0:v:0','-an','-pix_fmt','rgb24','-f','rawvideo','-'])
        assert raw==bytes(w*h*3);frames_checked+=1
        if streams=='audio_video':
            raw=decode(r['output'],False);assert raw==bytes(2048*4) and receipt['audio_samples']==1920;samples_checked+=1920
        cases.append(Path(r['output']).name)

    # Preview attachments are deliberately offline during export; full-quality data must still win.
    proxy=call({'command':'proxy.generate','project':project,'expected_revision':project['revision'],'asset_id':'source-0','scale':2,'input_root':str(sources),'output_root':str(output),'output':str(output/'proxy.mkv')})
    preview=call({'command':'timeline.apply','project':project,'expected_revision':project['revision'],'operations':proxy['operations']+[{'op':'preview.proxy','scale':2}]})
    Path(proxy['output']).rename(output/'offline-proxy.mkv')
    assert check(request('full-quality-despite-preview',p=preview))[1:]==reference[1:]
    passed.append('delivery.original_quality_and_repeatability')

    client=Client(exe)
    try:
        client.initialize();tool=next(t for t in client.rpc('tools/list')['result']['tools'] if t['name']=='cutbolt_export_inspect')
        r=request('mcp-planned','h264_aac',start=33,count=49);args={k:v for k,v in r.items() if k!='command'}
        Draft202012Validator(tool['inputSchema']).validate(args);assert tool['annotations']['readOnlyHint']
        assert client.call('export.inspect',**args)==call({**r,'command':'export.inspect'})
        fresh=call({'command':'project.create','id':'saved-export','width':W,'height':H,'frame_rate':time(25)})
        client.call('session.create',project=fresh,store_root=str(store),request_id='create')
        fields={'store_root':str(store),'project_id':fresh['id'],'request_id':'edit','expected_revision':0,'operations':ops}
        receipt=client.call('session.apply',**fields);assert receipt==client.call('session.apply',**fields)
        saved=client.call('session.get',store_root=str(store),project_id=fresh['id']);snapshot=copy.deepcopy(saved)
        check(request('saved-range','h264_aac',start=33,count=49,p=saved),33,49)
        assert client.call('session.get',store_root=str(store),project_id=fresh['id'])==snapshot
        assert not Path(r['output']).exists()
    finally:client.close()
    passed.append('delivery.mcp_saved_sessions')

    bad=[]
    def reject(r,code):bad.append((r,code))
    base=request('invalid')
    for start,count in [(0,0),(101,1),(100,2),(0,180001)]:reject(request('invalid',start=start,count=count),'INVALID_EXPORT')
    reject({**base,'range':{'start':time(1,100),'duration':time(1,25)}},'UNALIGNED_TIME')
    reject({**base,'range':{'start':{'num':0,'den':0},'duration':time(1,25)}},'INVALID_TIME')
    reject({**base,'input_transfer':'srgb'},'INVALID_EXPORT')
    r=request('missing-transfer','h264_aac');r.pop('input_transfer');reject(r,'INVALID_EXPORT')
    reject({**request('unused-transfer','h264_aac','audio'),'input_transfer':'bt709'},'INVALID_EXPORT')
    declared=copy.deepcopy(project);declared['transfer']='srgb'
    r=request('declared-transfer','h264_aac',p=declared);r.pop('input_transfer')
    assert call({**r,'command':'export.inspect'})['input_transfer']=='srgb'
    reject({**r,'input_transfer':'bt709'},'INVALID_EXPORT')
    for w,h in [(191,128),(192,127),(1922,128),(192,1082)]:
        p=copy.deepcopy(project);p.update(width=w,height=h);reject(request('bad-geometry','h264_aac',p=p),'INVALID_EXPORT')
    p=copy.deepcopy(project);p['clips']=[];reject(request('empty',p=p),'INVALID_EXPORT')
    p=copy.deepcopy(project);p['clips']=[{'id':'gap','gap':True,'source_in':time(0),'duration':time(1)}];p['frame_rate']=time(24);reject(request('rate','h264_aac',p=p),'INVALID_EXPORT')
    p=copy.deepcopy(p);p['frame_rate']=time(23);reject(request('unsupported-native-rate',p=p),'UNSUPPORTED_TIMELINE')
    reject({**base,'profile':'h265'},'INVALID_JSON');reject({**base,'streams':'none'},'INVALID_JSON');reject({**base,'bitrate':1},'INVALID_JSON')
    reject({**request('bad-transfer','h264_aac'),'input_transfer':'hdr'},'INVALID_JSON')
    reject({**base,'output':str(output/'wrong.mp4')},'UNSUPPORTED_OUTPUT')
    reject({**base,'output':str(root/'escape.mkv')},'PATH_OUTSIDE_ROOT')
    reject({**base,'input_root':str(output)},'PATH_OUTSIDE_ROOT')
    p=copy.deepcopy(project);p['assets'][0]['identity']['sha256']='0'*64;reject(request('identity',p=p),'IDENTITY_MISMATCH')
    original_outputs={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(r,code) in enumerate(bad):call(r,code)
    call(request('full-reference'),'OUTPUT_EXISTS')
    call({**base,'output_root':str(sources),'output':assets[0]['path']},'OUTPUT_EXISTS')
    call(request('missing-tool'), 'TOOL_UNAVAILABLE',{**os.environ,'CUTBOLT_FFMPEG':str(root/'missing.exe')})

    # Original controlled tool fixture: corrupt only our own staged output after a successful codec call.
    wrapper=root/'controlled.rs';wrapper.write_text(r'''
use std::{env,process::{Command,exit},path::Path,fs};
fn main(){let a:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("EXPORT_REAL_FFMPEG").unwrap()).args(&a).status().unwrap();
if !status.success(){exit(status.code().unwrap_or(1));}
if let Some(last)=a.last(){let p=Path::new(last);if p.file_name().is_some_and(|x|x=="encoded.mp4"){
let parent=p.parent().unwrap();let root=fs::canonicalize(env::var("EXPORT_FIXTURE_ROOT").unwrap()).unwrap();assert!(fs::canonicalize(parent).unwrap().starts_with(root));
if env::var("EXPORT_FAULT").unwrap()=="container"{fs::copy(parent.join("reference.mkv"),p).unwrap();}
else {let source=Path::new(&env::var("EXPORT_FIXTURE_SOURCE").unwrap()).to_path_buf();assert!(fs::canonicalize(&source).unwrap().starts_with(fs::canonicalize(env::var("EXPORT_FIXTURE_ROOT").unwrap()).unwrap()));let mut bytes=fs::read(&source).unwrap();bytes.push(1);fs::write(source,bytes).unwrap();}
}}}
''',encoding='utf-8')
    wrapper_exe=root/'controlled.exe';subprocess.run(['rustc',str(wrapper),'-o',str(wrapper_exe)],check=True,capture_output=True)
    env={**os.environ,'CUTBOLT_FFMPEG':str(wrapper_exe),'EXPORT_REAL_FFMPEG':shutil.which('ffmpeg'),'EXPORT_FIXTURE_ROOT':str(root),'EXPORT_FIXTURE_SOURCE':assets[0]['path']}
    call(request('corrupt-staging','h264_aac',start=0,count=7),'RENDER_VALIDATION_FAILED',{**env,'EXPORT_FAULT':'container'})
    source=Path(assets[0]['path']);original=source.read_bytes()
    try:call(request('changed-source','h264_aac',start=0,count=7),'MEDIA_CHANGED',{**env,'EXPORT_FAULT':'source'})
    finally:source.write_bytes(original)
    assert original_outputs=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert original_hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-export-*'))
    passed.append('delivery.validation_faults_and_preservation')
    report={'passed':passed,'decoded_video_frames':frames_checked,'presentation_stereo_sample_frames':samples_checked,'exports':cases,'rejected_cases':len(bad)+5,
            'quality':quality,'thresholds':{'minimum_rgb_psnr_db':30,'maximum_flat_patch_yuv_error':4,'minimum_audio_snr_db':28,'aac_best_sample_lag':0},
            'reference':'Original RGB charts/motion/binary frame IDs and stereo tones; exact Fraction timeline slices; public sRGB/BT.709 transfer and matrix equations in Decimal; independent full decoded samples, container timing, AAC priming and faststart checks',
            'limits':'One fixed H.264/AAC preset; no broader bitrate/device matrix, HDR, automatic color inference, general formats or queued export claim'}
    (root/'project.json').write_text(json.dumps(project,indent=2)+'\n',encoding='utf-8')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output)
