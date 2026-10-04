"""Original SDR charts: independent Decimal transfer and YCbCr matrix/range references."""
import argparse
from array import array
import copy
from decimal import Decimal as D, getcontext
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path
import subprocess
import wave
from agents import Client
from scenes import identity, time
from jsonschema import Draft202012Validator

getcontext().prec=48
ROOT=Path(__file__).resolve().parents[1]
W,H,N=64,32,8

def rounded(v):return max(0,min(255,int(v+D('0.5'))))
def encode(v,transfer):
    if transfer=='srgb':return D('12.92')*v if v<=D('0.0031308') else D('1.055')*v**(D(1)/D('2.4'))-D('0.055')
    return D('4.5')*v if v<D('0.018') else D('1.099')*v**D('0.45')-D('0.099')
def decode(v,transfer):
    if transfer=='srgb':return v/D('12.92') if v<=D('0.04045') else ((v+D('0.055'))/D('1.055'))**D('2.4')
    return v/D('4.5') if v<D('0.081') else ((v+D('0.099'))/D('1.099'))**(D(1)/D('0.45'))
@lru_cache(None)
def convert_pixel(a,b,c,matrix,range_,source,target):
    if matrix=='rgb':
        offset,span=(0,255) if range_=='full' else (16,219)
        encoded=[(D(v)-offset)/span for v in (a,b,c)]
    else:
        offset,ys,cs=(0,255,255) if range_=='full' else (16,219,224)
        y=(D(a)-offset)/ys;cb=(D(b)-128)/cs;cr=(D(c)-128)/cs
        # Solve public luma and color-difference equations independently in Decimal.
        blue=y+D('1.8556')*cb;red=y+D('1.5748')*cr
        green=(y-D('0.2126')*red-D('0.0722')*blue)/D('0.7152')
        encoded=[red,green,blue]
    result=[]
    for v in encoded:
        v=max(D(0),min(D(1),v))
        if source!=target:v=encode(decode(v,source),target)
        result.append(rounded(v*255))
    return bytes(result)

def physical_pixel(x,y,n):
    if y<8:return (x*4,)*3
    palette=[(0,0,0),(255,255,255),(128,128,128),(64,64,64),(196,32,32),(32,196,32),(32,32,196),(180,120,60)]
    if y<16:return palette[x//8]
    return ((x*4+n*7)%256,(y*6+n*9)%256,((x//2+y//2+n)*11)%256)

@lru_cache(None)
def encoded_byte(v,transfer):return encode(decode(D(v)/255,'srgb'),transfer)

def samples(fmt,transfer,range_,n):
    values=[]
    for y in range(H):
        for x in range(W):
            values.append([encoded_byte(v,transfer) for v in physical_pixel(x,y,n)])
    offset,ys,cs=(0,255,255) if range_=='full' else (16,219,224)
    if fmt=='bgr0':
        return b''.join(bytes([rounded(offset+ys*v) for v in (rgb[2],rgb[1],rgb[0])]+[0]) for rgb in values)
    planes=[[],[],[]]
    for i,(r,g,b) in enumerate(values):
        lum=D('0.2126')*r+D('0.7152')*g+D('0.0722')*b
        planes[0].append(rounded(offset+ys*lum))
        if fmt=='yuv444p' or (i//W)%2==0 and i%W%2==0:
            planes[1].append(rounded(128+cs*(b-lum)/D('1.8556')))
            planes[2].append(rounded(128+cs*(r-lum)/D('1.5748')))
    # Include legal stored bytes outside nominal range to verify declared clipping, not wraparound.
    if fmt=='yuv444p':
        for i,triplet in enumerate([(0,128,128),(255,128,128),(16,0,255),(235,255,0),(81,90,240)]):
            for c in range(3):planes[c][-1-i]=triplet[c]
    return b''.join(bytes(v) for v in planes)

def expected(native,fmt,range_,source,target,indices):
    stride=W*H*(4 if fmt=='bgr0' else 3) if fmt!='yuv420p' else W*H*3//2
    result=bytearray()
    for n in indices:
        raw=native[n*stride:(n+1)*stride]
        for i in range(W*H):
            if fmt=='bgr0':triple=(raw[i*4+2],raw[i*4+1],raw[i*4]);matrix='rgb'
            else:
                sub=i if fmt=='yuv444p' else (i//W//2)*(W//2)+(i%W//2)
                plane=W*H if fmt=='yuv444p' else W*H//4
                triple=(raw[i],raw[W*H+sub],raw[W*H+plane+sub]);matrix='bt709'
            result.extend(convert_pixel(*triple,matrix,range_,source,target))
    return bytes(result)

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/p for p in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    def ff(args):
        p=subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,timeout=180)
        assert p.returncode==0,p.stderr.decode(errors='replace')
        return p.stdout
    def probe(path):return json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-of','json',str(path)]))['streams']
    pcm=array('h',[(n*37+c*271)%18001-9000 for n in range(N*1920) for c in range(2)]).tobytes()
    with wave.open(str(sources/'sound.wav'),'wb') as f:f.setnchannels(2);f.setsampwidth(2);f.setframerate(48000);f.writeframes(pcm)
    fixtures={}
    def generate(name,fmt,transfer,range_,tagged=True,overrides=None):
        raw=b''.join(samples(fmt,transfer,range_,n) for n in range(N));raw_path=sources/(name+'.raw');raw_path.write_bytes(raw)
        compressed=fmt=='yuv420p';path=sources/(name+('.mp4' if compressed else '.mkv'))
        tags={'colorspace':'rgb' if fmt=='bgr0' else 'bt709','color_range':'pc' if range_=='full' else 'tv','color_primaries':'bt709','color_trc':'iec61966-2-1' if transfer=='srgb' else 'bt709'}
        if compressed:tags['chroma_sample_location']='left'
        if overrides:tags.update(overrides)
        args=['-f','rawvideo','-pixel_format',fmt,'-video_size',f'{W}x{H}','-framerate','25','-i',str(raw_path),'-i',str(sources/'sound.wav'),'-map','0:v:0','-map','1:a:0','-vf','setsar=1',
              '-c:v','libx264' if compressed else 'ffv1','-pix_fmt',fmt,'-threads','1','-c:a','aac' if compressed else 'pcm_s16le']
        if compressed:args+=['-crf','10','-bf','2','-b:a','320k']
        if tagged:
            for k,v in tags.items():args+=['-'+k,v]
        args += [str(path)];ff(args)
        meta=next(s for s in probe(path) if s['codec_type']=='video')
        native=ff(['-i',str(path),'-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt',meta['pix_fmt'],'-f','rawvideo','-'])
        if not compressed:assert native==raw,(name,'native fixture bytes changed')
        sound=ff(['-i',str(path),'-map','0:a:0','-vn','-f','s16le','-'])
        fixtures[name]=(path,fmt,transfer,range_,native,sound,meta)
    for fmt in ('bgr0','yuv444p'):
        for transfer in ('srgb','bt709'):
            for range_ in ('full','limited'):generate(f'{fmt}-{transfer}-{range_}',fmt,transfer,range_)
    generate('rgb-untagged','bgr0','srgb','full',False)
    generate('yuv-untagged','yuv444p','bt709','limited',False)
    generate('h264-untagged','yuv420p','bt709','limited',False)
    generate('h264-limited','yuv420p','bt709','limited')
    generate('h264-full','yuv420p','bt709','full')
    generate('wrong-primaries','bgr0','srgb','full',overrides={'color_primaries':'bt2020'})
    generate('hdr','bgr0','srgb','full',overrides={'color_trc':'smpte2084'})
    generate('wrong-chroma','yuv420p','bt709','limited',overrides={'chroma_sample_location':'center'})
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ROOT/'target/debug/cutbolt.exe'
    def call(request,error=None,env=None):
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180,env=env);v=json.loads(p.stdout)
        if error:assert p.returncode==1 and v['error']['code']==error,(error,v);return v
        assert p.returncode==0 and v['ok'],v;return v['result']
    def recipe(name,target='srgb',policy='reject',**changes):
        path,fmt,transfer,range_,*_=fixtures[name]
        r={'schema_version':1,'id':name+'-'+target,'source':{'file':identity(path,sources),'sdr':{'matrix':'rgb' if fmt=='bgr0' else 'bt709','range':range_,'transfer':transfer,'missing_tags':policy}},'working_transfer':target,
           'source_in':time(0),'duration':time(N,25),'rate':time(1),'reverse':False,'freeze':False,'width':W,'height':H,'audio':'resample'}
        r.update(changes);return r
    def request(r,label,command='media.conform'):return {'command':command,'recipe':r,'input_root':str(sources),**({} if command.endswith('inspect') else {'output_root':str(output),'output':str(output/(label+'.mkv'))})}
    comparisons=[];receipts={};expected_outputs={};passed=[];maximum=0
    def check(name,r,label,indices=None):
        nonlocal maximum
        indices=list(range(N)) if indices is None else indices
        plan=call(request(r,label,'media.conform.inspect'));receipt=call(request(r,label))
        path=output/(label+'.mkv');fixture=fixtures[name]
        rgb=ff(['-i',str(path),'-map','0:v:0','-an','-pix_fmt','rgb24','-f','rawvideo','-'])
        want=expected(fixture[4],fixture[1],fixture[3],fixture[2],r['working_transfer'],indices)
        assert len(rgb)==len(want)
        difference=max(abs(a-b) for a,b in zip(rgb,want));assert difference<=1,(label,difference);maximum=max(maximum,difference)
        if fixture[1]=='bgr0' and fixture[3]=='full' and fixture[2]==r['working_transfer']:assert rgb==want
        audio=ff(['-i',str(path),'-map','0:a:0','-vn','-f','s16le','-'])
        assert audio==(bytes(len(indices)*1920*4) if r['audio']=='mute' else fixture[5][:len(indices)*1920*4])
        meta=next(s for s in probe(path) if s['codec_type']=='video')
        for key,value in {'color_space':'gbr','color_range':'pc','color_primaries':'bt709','color_transfer':'iec61966-2-1' if r['working_transfer']=='srgb' else 'bt709'}.items():assert meta[key]==value,(label,key,meta)
        assert receipt['source_frame_indices']==plan['source_frame_indices']==indices
        assert receipt['normalization']==plan['normalization']
        assert receipt['sha256']==hashlib.sha256(path.read_bytes()).hexdigest()
        receipts[label]=receipt;expected_outputs[label]=(rgb,audio)
        comparisons.append({'case':label,'frames':len(indices),'maximum_rgb_error':difference,'assumed_tags':plan['normalization']['assumed_tags']})
        return rgb
    for name in list(fixtures)[:8]:
        for target in ('srgb','bt709'):
            r=recipe(name,target);check(name,r,r['id']);assert not comparisons[-1]['assumed_tags']
    passed+=['color.transfer_matrix_range_charts','color.clipping_and_identity_precision']
    for name in ('rgb-untagged','yuv-untagged','h264-untagged','h264-limited','h264-full'):
        for target in ('srgb','bt709'):
            r=recipe(name,target,'use_declared' if 'untagged' in name else 'reject');check(name,r,r['id'])
            assert bool(comparisons[-1]['assumed_tags'])==('untagged' in name)
    passed.append('color.tagged_untagged_and_native_yuv420')
    name='yuv444p-bt709-limited'
    for label,fields,indices in [('retime',{'source_in':time(1,25),'duration':time(4,25),'rate':time(3,2),'audio':'mute'},[1,2,4,5]),('reverse',{'source_in':time(7,25),'duration':time(4,25),'reverse':True,'audio':'mute'},[7,6,5,4]),('freeze',{'source_in':time(3,25),'duration':time(4,25),'freeze':True,'audio':'mute'},[3]*4)]:check(name,recipe(name,**fields),label,indices)
    repeat=check('h264-full',recipe('h264-full'),'repeat');assert repeat==expected_outputs['h264-full-srgb'][0]
    passed.append('color.retime_and_repeatability')

    # Normalize different source encodings before using the normal saved editing workflow.
    client=Client(exe)
    try:
        client.initialize();r=recipe('rgb-untagged',policy='use_declared')
        tool=next(t for t in client.rpc('tools/list')['result']['tools'] if t['name']=='cutbolt_media_conform_inspect')
        arguments={'recipe':r,'input_root':str(sources)};Draft202012Validator(tool['inputSchema']).validate(arguments)
        assert client.call('media.conform.inspect',**arguments)==call(request(r,'mcp','media.conform.inspect'))
        project=call({'command':'project.create','id':'mixed-sdr','width':W,'height':H,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        labels=['rgb-untagged-srgb','yuv444p-bt709-limited-srgb','h264-full-srgb'];operations=[]
        for label in labels:operations += [{'op':'media.add','asset':receipts[label]['asset']},{'op':'clip.append','clip':{'id':label,'asset_id':receipts[label]['asset']['id'],'source_in':time(0),'duration':time(N,25)}}]
        args={'store_root':str(store),'project_id':'mixed-sdr','expected_revision':0,'request_id':'assemble','operations':operations}
        edit=client.call('session.apply',**args);assert edit==client.call('session.apply',**args)
        project=client.call('session.get',store_root=str(store),project_id='mixed-sdr')
        call({'command':'render.run','project':project,'input_root':str(output),'output_root':str(output),'output':str(output/'mixed.mkv')})
        assert ff(['-i',str(output/'mixed.mkv'),'-map','0:v:0','-an','-pix_fmt','rgb24','-f','rawvideo','-'])==b''.join(expected_outputs[label][0] for label in labels)
        assert ff(['-i',str(output/'mixed.mkv'),'-map','0:a:0','-vn','-f','s16le','-'])==b''.join(expected_outputs[label][1] for label in labels)
        call({'command':'export.run','project':project,'input_root':str(output),'output_root':str(output),'output':str(output/'mixed.mp4'),'profile':'h264_aac','streams':'audio_video','input_transfer':'srgb'})
        assert client.call('session.get',store_root=str(store),project_id='mixed-sdr')==project
    finally:client.close()
    passed.append('color.mixed_saved_session_mcp_and_delivery')

    bad=[]
    def reject(r,code='INVALID_COLOR'):bad.append((r,code))
    base=recipe('bgr0-srgb-full')
    for name in ('rgb-untagged','yuv-untagged','h264-untagged','wrong-primaries','hdr','wrong-chroma'):reject(recipe(name))
    for name,key,value in [('bgr0-srgb-full','transfer','bt709'),('yuv444p-bt709-limited','range','full'),('h264-full','range','limited'),('bgr0-srgb-full','matrix','bt709')]:
        r=recipe(name,policy='use_declared');r['source']['sdr'][key]=value;reject(r)
    r=copy.deepcopy(base);r['source']['color']='encoded_rgb';reject(r)
    r=copy.deepcopy(base);r.pop('working_transfer');reject(r)
    r=copy.deepcopy(base);r['source'].pop('sdr');r['source']['color']='encoded_rgb';reject(r)
    for key,value in [('matrix','bt2020'),('range','auto'),('transfer','pq'),('missing_tags','guess'),('icc','profile.icc')]:
        r=copy.deepcopy(base);r['source']['sdr'][key]=value;reject(r,'INVALID_JSON')
    r=copy.deepcopy(base);r['working_transfer']='linear';reject(r,'INVALID_JSON')
    r=copy.deepcopy(base);r['source']['file']['sha256']='0'*64;reject(r,'MEDIA_CHANGED')
    r=copy.deepcopy(base);r['source']['file']['path']='../escape.mkv';reject(r,'INVALID_PATH')
    r=copy.deepcopy(base);r['source']['file']=identity(sources/'sound.wav',sources);reject(r,'UNSUPPORTED_MEDIA')
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(r,code) in enumerate(bad):call(request(r,'bad-'+str(i)),code)
    call(request(base,base['id']),'OUTPUT_EXISTS')
    req=request(base,'missing-tool');call(req,'TOOL_UNAVAILABLE',{**os.environ,'CUTBOLT_FFMPEG':str(root/'absent.exe')})
    req=request(base,'source');req.update(output_root=str(sources),output=str(fixtures['bgr0-srgb-full'][0]));call(req,'OUTPUT_EXISTS')
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*')) and not list(output.glob('.cutbolt-export-*'))
    passed.append('color.invalid_metadata_and_preservation')
    report={'passed':passed,'cases':comparisons,'frames_compared':sum(c['frames'] for c in comparisons)+24,'stereo_sample_frames_compared':(sum(c['frames'] for c in comparisons)+24)*1920,'rejected_cases':len(bad)+3,'maximum_rgb_error':maximum,'threshold':{'maximum_rgb_error':1},'reference':'Original native RGB/YUV chart samples; independent 48-digit Decimal transfer/matrix/range equations and Fraction frame mapping. External tools supply codec encode/decode only. All output RGB/PCM checked; original sources and existing outputs preserved.','limits':'8-bit BT.709 primaries/D65, sRGB or BT.709 OETF interpretation, fixed full/limited ranges; YUV420 nearest 2x2 blocks, no HDR/ICC/wide gamut or display transform'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    (root/'recipe.json').write_text(json.dumps(recipe('yuv-untagged',policy='use_declared'),indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',required=True,type=Path);run(p.parse_args().output)
