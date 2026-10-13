"""Independent display geometry, original source pixels, and explicit failure cases."""
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess

import numpy as np

from engine import ENGINE
from footage_sources import ROOT, encode, identity
from source_clocks import time, decoded, rgb_reference

W, H, N = 36, 18, 6


def run(root, convert_pixel):
    root = Path(root).resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=False)
    sources, outputs = root/'sources', root/'outputs'
    sources.mkdir(); outputs.mkdir()

    def ff(args, data=None):
        process = subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*map(str,args)],
                                 input=data,capture_output=True,timeout=180)
        assert process.returncode == 0, process.stderr.decode(errors='replace')
        return process.stdout

    def meta(path):
        data = json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-of','json',str(path)]))
        return next(s for s in data['streams'] if s['codec_type']=='video')

    def call(request, code=None):
        process = subprocess.run([str(ENGINE)],input=json.dumps(request).encode(),capture_output=True,timeout=180)
        value = json.loads(process.stdout)
        if code:
            assert process.returncode and value['error']['code']==code, value
            return value['error']
        assert not process.returncode and value['ok'], value
        return value['result']

    # Input geometry is authored explicitly; each orientation is stream-copy metadata only.
    fixtures = {}
    for name,sar in [('square',F(1)),('wide',F(4,3)),('tall',F(3,4))]:
        data = encode(sources,name,rate=F(25),count=N,size=(W,H),container='mov',audio_codec='pcm_s16le',extra=['-vf',f'setsar={sar}'])
        source = sources/data['path']
        rgb = np.frombuffer(rgb_reference(decoded(source,True,'yuv420p'),W,H),dtype=np.uint8).reshape(N,H,W,3)
        for rotation in (0,90,180,270):
            path = sources/f'{name}-{rotation}.mov'
            ff(['-display_rotation',rotation,'-i',source,'-map','0','-c','copy',path])
            stream=meta(path)
            assert F(stream['sample_aspect_ratio'].replace(':','/'))==sar
            observed=[s['rotation'] for s in stream.get('side_data_list',[]) if s['side_data_type']=='Display Matrix']
            assert observed==([] if rotation==0 else [rotation if rotation<180 else rotation-360]), observed
            fixtures[f'{name}-{rotation}']=(path,rgb,sar,rotation,False)

    # Original RGB beneath zero alpha must survive geometry as well as visible pixels.
    n,y,x=np.indices((N,H,W),dtype=np.int64)
    rgba=np.stack(((x*7+n*13)%256,(y*11+n*19)%256,(x*3+y*5+n*23)%256,
                   np.array([0,51,127,255],dtype=np.uint8)[(x+y+n)%4]),axis=-1).astype(np.uint8)
    alpha=sources/'alpha.mov'
    ff(['-f','rawvideo','-pixel_format','rgba','-video_size',f'{W}x{H}','-framerate','25','-i','pipe:0',
        '-i',sources/'square.wav','-map','0:v:0','-map','1:a:0',
        '-vf','scale=in_range=pc:out_range=tv:out_color_matrix=bt709,format=yuva444p10le,setsar=4/3',
        '-c:v','prores_ks','-profile:v','4','-alpha_bits','16','-threads','1','-pix_fmt','yuva444p10le',
        '-colorspace','bt709','-color_range','tv','-color_primaries','bt709','-color_trc','bt709',
        '-c:a','pcm_s16le',alpha],rgba.tobytes())
    fmt=meta(alpha)['pix_fmt'];bits=12 if '12' in fmt else 10
    native=np.frombuffer(decoded(alpha,True,fmt),dtype='<u2').reshape(N,4,H,W)
    reference=np.zeros((N,H,W,4),dtype=np.uint8)
    for t in range(N):
        for row in range(H):
            for col in range(W):
                reference[t,row,col,:3]=list(convert_pixel(*map(int,native[t,:3,row,col]),'bt709','limited','bt709','bt709',bits))
    maximum=(1<<bits)-1
    reference[:,:,:,3]=((native[:,3].astype(np.uint32)*255+maximum//2)//maximum).astype(np.uint8)
    assert np.any((reference[:,:,:,3]==0) & (reference[:,:,:,:3]!=0).any(axis=-1))
    for rotation in (0,90):
        path=sources/f'alpha-{rotation}.mov'
        ff(['-display_rotation',rotation,'-i',alpha,'-map','0','-c','copy',path])
        fixtures[f'alpha-{rotation}']=(path,reference,F(4,3),rotation,True)

    # Independent NumPy rotation, exact physical aspect and per-axis nearest coordinates.
    def expected(rgb,sar,rotation,canvas,fit):
        oriented=np.rot90(rgb,rotation//90,axes=(1,2))
        aspect=F(W,H)*sar
        if rotation%180:aspect=1/aspect
        width,height=canvas
        if fit=='stretch':tw,th=width,height
        else:
            box=F(width,height)
            if (aspect>=box)==(fit=='contain'):
                tw=width;vertical=F(tw)/aspect
                th=vertical.numerator//vertical.denominator if fit=='contain' else -(-vertical.numerator//vertical.denominator)
            else:
                th=height;horizontal=th*aspect
                tw=horizontal.numerator//horizontal.denominator if fit=='contain' else -(-horizontal.numerator//horizontal.denominator)
        assert tw>0 and th>0
        result=np.zeros((len(rgb),height,width,rgb.shape[-1]),dtype=np.uint8)
        if fit=='contain':
            left,top=(width-tw)//2,(height-th)//2
            ys=np.arange(th)*oriented.shape[1]//th;xs=np.arange(tw)*oriented.shape[2]//tw
            result[:,top:top+th,left:left+tw]=oriented[:,ys[:,None],xs[None,:]]
        else:
            left,top=(tw-width)//2,(th-height)//2
            ys=(np.arange(height)+top)*oriented.shape[1]//th;xs=(np.arange(width)+left)*oriented.shape[2]//tw
            result=oriented[:,ys[:,None],xs[None,:]]
        return result

    def recipe(name,canvas,fit):
        path,_,_,_,alpha=fixtures[name]
        source={'file':{'path':path.name,**identity(path)},'sdr':{'matrix':'bt709','range':'limited','transfer':'bt709',
            'missing_tags':'use_declared' if alpha else 'reject'},'geometry':{'fit':fit}}
        if alpha:source['alpha']='straight'
        return {'schema_version':1,'id':name,'source':source,'working_transfer':'bt709','source_in':0,'duration':time(F(N,25)),
                'rate':1,'reverse':False,'freeze':False,'width':canvas[0],'height':canvas[1],'audio':'resample','frame_rate':25}

    # True non-quarter/reflection metadata, never a rounded rotation scalar fabricated by the test.
    for name,args in [('oblique',['-display_rotation','45']),('mirrored',['-display_hflip'])]:
        ff([*args,'-i',sources/'square.mov','-map','0','-c','copy',sources/f'{name}.mov'])
    originals={p.name:identity(p) for p in sources.iterdir()}
    cases=[]
    for index,(name,(path,rgb,sar,rotation,alpha)) in enumerate(fixtures.items()):
        for fit in ('contain','cover','stretch'):
            canvas=(27,19) if index%2==0 else (17,29)
            request=recipe(name,canvas,fit); label=f'{name}-{fit}'
            inspected=call({'command':'media.conform.inspect','input_root':str(sources),'recipe':request})
            output=outputs/(label+'.mkv')
            result=call({'command':'media.conform','input_root':str(sources),'output_root':str(outputs),'output':str(output),'recipe':request,
                         **({'max_output_bytes':4*1024*1024} if alpha and fit=='contain' else {})})
            assert inspected['geometry']==result['geometry']
            assert result['geometry']['rotation_counterclockwise']==rotation
            assert result['geometry']['sample_aspect']==time(sar)
            wanted=expected(rgb,sar,rotation,canvas,fit)
            raw=decoded(output,True,'rgba' if alpha else 'rgb24')
            actual=np.frombuffer(raw,dtype=np.uint8).reshape(wanted.shape)
            if alpha:
                assert np.array_equal(actual[:,:,:,3],wanted[:,:,:,3]),label
                assert np.abs(actual[:,:,:,:3].astype(int)-wanted[:,:,:,:3]).max()<=1,label
            else:assert np.array_equal(actual,wanted),label
            assert decoded(output,False)==decoded(path,False),label
            assert meta(output).get('sample_aspect_ratio','1:1') in ('1:1','N/A')
            cases.append({'name':label,'canvas':canvas,'rgb_max_error':int(np.abs(actual.astype(int)-wanted.astype(int)).max()),
                          'alpha_exact':alpha,'pcm_exact':True,'decoded_sha256':hashlib.sha256(raw).hexdigest()})
    # Orientation and fitting commute with the declared straight-channel temporal sampling.
    name='alpha-90';path,rgb,sar,rotation,alpha=fixtures[name]
    for label,changes,frames in [
        ('reverse',{'source_in':time(F(5,25)),'reverse':True},rgb[[5,4,3,2]]),
        ('linear',{'remap':{'segments':[{'duration':time(F(4,25)),'start_rate':time(F(1,2)),
            'end_rate':time(F(1,2)),'reverse':False}],'video_sampling':'linear','audio_pitch':'mute'}},
            np.stack((rgb[0],((rgb[0].astype(np.uint16)+rgb[1]+1)//2).astype(np.uint8),rgb[1],
                      ((rgb[1].astype(np.uint16)+rgb[2]+1)//2).astype(np.uint8))))]:
        request=recipe(name,(17,29),'contain')
        request.update(duration=time(F(4,25)),audio='mute',**changes)
        output=outputs/f'alpha-geometry-{label}.mkv'
        result=call({'command':'media.conform','recipe':request,'input_root':str(sources),'output_root':str(outputs),'output':str(output)})
        wanted=expected(frames,sar,rotation,(17,29),'contain')
        raw=decoded(output,True,'rgba');actual=np.frombuffer(raw,dtype=np.uint8).reshape(wanted.shape)
        assert np.array_equal(actual[:,:,:,3],wanted[:,:,:,3]),label
        assert np.abs(actual[:,:,:,:3].astype(int)-wanted[:,:,:,:3]).max()<=1,label
        assert decoded(output,False)==bytes(4*1920*4)
        cases.append({'name':'alpha-geometry-'+label,'alpha_exact':True,'muted_pcm_exact':True,
                      'decoded_sha256':hashlib.sha256(raw).hexdigest()})
    # An omitted policy retains legacy nearest resizing, with no geometry receipt added.
    request=recipe('square-0',(27,19),'stretch');request['source'].pop('geometry')
    output=outputs/'legacy-resize.mkv'
    legacy=call({'command':'media.conform','recipe':request,'input_root':str(sources),'output_root':str(outputs),'output':str(output)})
    assert 'geometry' not in legacy and legacy['resize']=='nearest_top_left'
    assert decoded(output,True)==decoded(outputs/'square-0-stretch.mkv',True)
    # The existing convenience preparation command proposes and executes the visible fit policy.
    output=outputs/'prepared-phone.mkv';name='wide-90';path,rgb,sar,rotation,_=fixtures[name]
    prepared=call({'command':'media.prepare','path':str(path),'input_root':str(sources),
                   'output_root':str(outputs),'output':str(output)})
    assert prepared['converted'],prepared
    stream=meta(output);canvas=(stream['width'],stream['height'])
    assert canvas==(H,W),canvas
    assert decoded(output,True)==expected(rgb,sar,rotation,canvas,'contain').tobytes()
    assert decoded(output,False)==decoded(path,False)
    before={p.name:identity(p) for p in outputs.iterdir()}
    negatives=[]
    for name in ('oblique','mirrored'):
        request=recipe('square-0',(27,19),'contain');p=sources/(name+'.mov');request['source']['file']={'path':p.name,**identity(p)}
        negatives.append((name,request,'UNSUPPORTED_MEDIA'))
    request=recipe('wide-90',(27,19),'contain');request['source'].pop('geometry');negatives.append(('missing-policy',request,'UNSUPPORTED_MEDIA'))
    request=recipe('square-0',(27,19),'contain');request['source']['geometry']['fit']='guess';negatives.append(('invalid-fit',request,'INVALID_JSON'))
    request=recipe('square-0',(27,19),'contain');request['source'].pop('sdr');request['source']['color']='bt709_limited';negatives.append(('no-sdr',request,'INVALID_CONFORM'))
    for name,request,code in negatives:
        error=call({'command':'media.conform','recipe':request,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/(name+'.mkv'))},code)
        if name in ('oblique','mirrored','no-sdr'):assert error['path']=='recipe.source.geometry' and error['hint']
    assert before=={p.name:identity(p) for p in outputs.iterdir()}
    assert originals=={p.name:identity(p) for p in sources.iterdir()}
    assert not list(outputs.glob('.cutbolt-*'))
    report={'cases':cases,'rejected_cases':len(negatives),'sources_preserved':True,'legacy_resize_identical':True,
            'preparation_oriented_canvas_exact':True,'originals':originals,
            'tools':{tool:subprocess.check_output([tool,'-version'],text=True).splitlines()[0] for tool in ('ffmpeg','ffprobe')},
            'reference':'Independent NumPy quarter turns and rational fit coordinates; native YUV RGB reference exact, ProRes Decimal RGB tolerance 1 and alpha exact.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    return report
