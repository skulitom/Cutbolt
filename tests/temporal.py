"""Original shutter fixtures, analytic moving-box exposure and independent sampling."""
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess
import time as clock
import wave
from PIL import Image
from agents import Client
from scenes import identity, time, expected_frame
from spatial import reference as spatial_reference
from expressions import graph, resolved, constant
from keying import pixel_reference, resolve_chain, key
from selection import selective

ROOT = Path(__file__).resolve().parents[1]


def fraction(t):
    return F(t['num'],t['den'])


def rat(t):
    t = F(t)
    return {'num':t.numerator,'den':t.denominator}


def exposure(samples=8, angle=360, phase=-180):
    return {'samples':samples,'shutter_angle':rat(angle),'phase':rat(phase),'integration':'encoded_rgb'}


def sample_times(scene,n):
    if 'temporal' not in scene:
        return [F(n,25)]
    s = scene['temporal']; count = s['samples']
    return [F(n,25)+(fraction(s['phase'])+fraction(s['shutter_angle'])*F(2*k+1,2*count))/9000 for k in range(count)]


def integrate(scene,sources,n,expression=False):
    w,h = scene['width'],scene['height']; samples = sample_times(scene,n)
    sums = [0]*(w*h*3)
    for t in samples:
        if not 0 <= t < fraction(scene['duration']):
            pixels = bytes(scene['background'])*(w*h)
        elif expression:
            pixels = expected_frame(resolved(scene,t*25),sources,t*25)
        else:
            pixels = spatial_reference(scene,sources,t*25)
        sums = [a+b for a,b in zip(sums,pixels)]
    return bytes((v+len(samples)//2)//len(samples) for v in sums)


def analytic_box(scene,n):
    # Continuous box [12+100*t,16+100*t) x [4,8), sampled at pixel centres.
    # Independent interval intersections; no temporal sample loop or renderer.
    a,b = F(n,25)-F(1,50),F(n,25)+F(1,50)
    result = bytearray()
    for y in range(scene['height']):
        for x in range(scene['width']):
            if 4 <= y < 8:
                center = F(2*x+1,2)
                left = max(a,F(0),(center-16)/100)
                right = min(b,fraction(scene['duration']),(center-12)/100)
                coverage = max(F(0),right-left)/(b-a)
            else:
                coverage = F(0)
            result.extend(int(old*(1-coverage)+255*coverage+F(1,2)) for old in scene['background'])
    return bytes(result)


def effect_integral(scene,n):
    # Single static green patch: independent point-effect reference, direct
    # rational alpha composition, then an independently enumerated exposure.
    sums=[0]*(scene['width']*scene['height']*3);times=sample_times(scene,n)
    for t in times:
        pixels=bytearray(bytes(scene['background'])*(scene['width']*scene['height']))
        if 0<=t<fraction(scene['duration']):
            chain=json.dumps(resolve_chain(scene['layers'][0]['effects'],t),sort_keys=True)
            for y in range(4):
                for x in range(4):
                    colors,alpha=pixel_reference((0,255,0,255),'straight',(x,y),chain)
                    coverage=F(alpha,255)
                    pos=((4+y)*scene['width']+12+x)*3
                    for c,old in enumerate(scene['background']):
                        pixels[pos+c]=int(old*(1-coverage)+255*colors[c]*coverage+F(1,2))
        sums=[a+b for a,b in zip(sums,pixels)]
    return bytes((v+len(times)//2)//len(times) for v in sums)


def curve(first,last,duration,mode='linear'):
    return {'keys':[{'time':time(0),'value':first,'interpolation':mode},{'time':duration,'value':last,'interpolation':'hold'}]}


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,store = [root/n for n in ('sources','output','store')]
    for p in (sources,output,store): p.mkdir()
    for name,color,size in [('white',[255,255,255,255],(4,4)),('green',[0,255,0,255],(4,4)),('red',[231,19,61,170],(4,4)),
                            ('blue',[23,107,219,255],(4,4)),('large',[255,255,255,255],(512,512))]:
        Image.new('RGBA',size,tuple(color)).save(sources/(name+'.png'))
    pcm = b''.join(struct.pack('<hh',(n*53)%20001-10000,(n*61)%18001-9000) for n in range(8*1920))
    with wave.open(str(sources/'sound.wav'),'wb') as wav:
        wav.setparams((2,2,48000,0,'NONE','not compressed')); wav.writeframes(pcm)
    originals = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    duration = time(8,25)
    spatial = {'translate_milli':[0,0],'scale_milli':[1000,1000],'rotation_mdeg':0,'flip':[False,False],
               'pixel_aspect':time(1),'sampling':'nearest','edge':'transparent',
               'animation':{'translate_x_milli':curve(0,32000,duration)}}
    layer = {'id':'box','canvas':[4,4],'start':time(0),'duration':duration,
             'frames':[{'image':identity(sources/'white.png',sources),'hold':duration,'offset':[0,0],'anchor':[0,0]}],
             'timing':'strict','end':'hold_last','transform':{'position':[12,4],'crop':[0,0,4,4],'scale':1,'quarter_turns':0,'opacity':255,'spatial':spatial}}
    scene = {'schema_version':1,'id':'shutter-box','width':64,'height':12,'output_scale':1,'duration':duration,
             'background':[7,11,19],'color':'srgb_straight_encoded','layers':[layer],
             'audio':{'file':identity(sources/'sound.wav',sources),'start':time(0),'channels':'preserve_stereo','resampling':'linear','padding':'silence'},
             'temporal':exposure()}
    exe = ROOT/'target/debug/cutbolt.exe'; passed=[]; cases=[]; frames=0; samples=0; rejected=0; previews=0

    def call(command,error=None,**fields):
        nonlocal rejected
        request = {'command':command,**fields}
        p = subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180)
        r = json.loads(p.stdout)
        if error:
            assert p.returncode==1 and r['error']['code']==error,(error,r)
            rejected+=1; return r['error']
        if p.returncode: (root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert p.returncode==0 and r['ok'],r
        return r['result']

    def decode(path,kind):
        args=['-an','-pix_fmt','rgb24','-f','rawvideo'] if kind=='video' else ['-vn','-f','s16le']
        return subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),*args,'-'],timeout=120)

    def check_media(path,expected,expected_pcm):
        nonlocal frames,samples
        rgb=decode(path,'video'); audio=decode(path,'audio')
        assert rgb==b''.join(expected),(path,'pixel mismatch',next(((i,a,b) for i,(a,b) in enumerate(zip(rgb,b''.join(expected))) if a!=b),None))
        assert audio==expected_pcm,(path,'audio mismatch')
        frames+=len(expected); samples+=len(audio)//4

    def check(s,name,oracle=None,expected_pcm=None):
        s=copy.deepcopy(s);s['id']=name
        receipt=call('scene.render',scene=s,input_root=str(sources),output_root=str(output),output=str(output/(name+'.mkv')))
        count=int(fraction(s['duration'])*25)
        expected=[(oracle(n) if oracle else integrate(s,sources,n)) for n in range(count)]
        expected_pcm=expected_pcm if expected_pcm is not None else pcm if s.get('audio') else bytes(count*1920*4)
        check_media(output/(name+'.mkv'),expected,expected_pcm)
        if 'temporal' in s:
            signed=[t for n in range(count) for t in sample_times(s,n)]
            actual=receipt['temporal']
            assert actual['sample_times']==[rat(t) for t in signed]
            assert actual['active_sample_times']==[rat(t) if 0<=t<fraction(s['duration']) else None for t in signed]
            assert actual['sample_count']==count*s['temporal']['samples']
            assert all(len(t['sampled_parameters'])==len(signed) for t in receipt['timing'])
            assert actual['effects']['history_filters']=='unsupported' and actual['effects']['optical_flow']=='unsupported'
        cases.append({'name':name,'frames':count,'samples_per_frame':s.get('temporal',{}).get('samples',1),'maximum_rgb_error':0})
        return receipt,expected

    convergences=[]
    for count in (1,2,4,8,16,32):
        s=copy.deepcopy(scene);s['temporal']['samples']=count
        _,expected=check(s,f'converge-{count}')
        errors=[abs(a-b) for n,frame in enumerate(expected) for a,b in zip(frame,analytic_box(s,n))]
        maximum=max(errors);mean=sum(errors)/len(errors)
        assert maximum<=math.ceil(255/count),(count,maximum)
        if count>=8: assert maximum==0,(count,maximum)
        convergences.append({'samples':count,'maximum_error_to_continuous_exposure':maximum,'mean_absolute_error':mean})
    assert convergences[0]['mean_absolute_error']>convergences[-1]['mean_absolute_error']
    passed.append('temporal.analytic_moving_box_shutter_and_convergence')

    for count,angle,phase in [(1,0,0),(1,0,90),(4,180,-90),(4,360,0),(4,360,360),(4,360,-360),(3,F(181,2),F(-83,3))]:
        s=copy.deepcopy(scene);s['temporal']=exposure(count,angle,phase)
        check(s,'phase-'+str(len(cases)))
    baseline=copy.deepcopy(scene);baseline.pop('temporal')
    _,unblurred=check(baseline,'legacy-sampling')
    instantaneous=copy.deepcopy(scene);instantaneous['temporal']=exposure(1,0,0)
    _,equivalent=check(instantaneous,'instantaneous')
    assert equivalent==unblurred
    passed.append('temporal.exact_fractional_phases_boundaries_and_unchanged_audio')

    # Fractional held image boundaries and layered activity, opacity/mask/effect
    # curves independently evaluated at actual exposure sample times.
    changes=copy.deepcopy(scene);changes['id']='boundaries';changes['temporal']=exposure(8,360,-90)
    a=changes['layers'][0];a.update(start=time(2,25),duration=time(4,25),timing='sample_start',end='loop')
    a['frames']=[{'image':identity(sources/(name+'.png'),sources),'hold':time(hold,50),'offset':[0,0],'anchor':[0,0]} for name,hold in [('red',3),('blue',1)]]
    a['transform']['spatial']['animation']={'translate_x_milli':curve(0,16000,a['duration'])}
    a['animation']={'opacity':curve(21,255,a['duration'],'ease_in_out')}
    a['mask']={'rect':[0,0,4,4],'inverted':False,'animation':{'width':{'keys':[
        {'time':time(0),'value':1,'interpolation':'hold'},{'time':time(1,50),'value':4,'interpolation':'hold'}]}}}
    a['effects']=[{'kind':'grade','exposure_milli':0,'contrast_milli':1000,'white_balance_milli':[1000,1000,1000],
                  'animation':{'exposure_milli':curve(-1000,1000,a['duration'])}}]
    b=copy.deepcopy(a);b.update(id='second',start=time(1,25),duration=time(6,25),end='transparent')
    b['transform']['position']=[8,6];b['transform']['spatial']['sampling']='bilinear'
    changes['layers'].append(b)
    receipt,wanted=check(changes,'animated-boundaries')
    reordered=copy.deepcopy(changes)
    for l in reordered['layers']:
        for c in l.get('animation',{}).values():c['keys'].reverse()
        for c in l['transform']['spatial']['animation'].values():c['keys'].reverse()
    _,again=check(reordered,'reordered-keys')
    assert again==wanted
    for name,effect in [('key',key(strength_curve=curve(0,1000,duration))),
        ('selective',selective(mask={'rect':[0,0,4,4]},mix=1000,mix_curve=curve(0,1000,duration),
                             grade={'exposure_milli':0,'contrast_milli':0,'white_balance_milli':[1000,1000,1000]}))]:
        s=copy.deepcopy(scene);s['layers'][0]['transform'].pop('spatial')
        s['layers'][0]['frames'][0]['image']=identity(sources/'green.png',sources)
        s['layers'][0]['effects']=[effect]
        check(s,'per-sample-'+name,lambda n,s=s:effect_integral(s,n))
    passed.append('temporal.subframe_held_sources_masks_effects_and_layer_boundaries')

    # An expression graph evaluated at 400 distinct temporal slots, beyond the
    # 256-time read-only inspection limit, inside the shared total-work budget.
    expr=copy.deepcopy(scene);expr.update(id='temporal-expressions',height=48,duration=time(2),audio=None,expressions=graph(),temporal=exposure(8))
    expr['layers']=[]
    for name in ('A','B'):
        l=copy.deepcopy(layer);l.update(id=name,start=time(0) if name=='A' else time(2,5),duration=time(2) if name=='A' else time(6,5))
        l['transform'].pop('spatial');l['transform']['position']=[3,3]
        l['frames'][0]['hold']=l['duration']
        if name=='A':l['animation']={'position_x':curve(3,13,time(2))}
        expr['layers'].append(l)
    er,ew=check(expr,'expressions',lambda n:integrate(expr,sources,n,True))
    assert len(er['expressions']['frame_bindings'])==400 and er['temporal']['sample_count']==400
    assert er['expressions']['frame_bindings'][:4]==[[],[],[],[]]
    passed.append('temporal.linked_properties_and_bounded_aggregate_evaluation')

    client=Client(exe)
    try:
        client.initialize(); catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==63
        inspected=client.call('scene.inspect',scene=changes,input_root=str(sources))
        assert inspected['temporal']==receipt['temporal']
        project=client.call('project.create',id='temporal-edit',width=64,height=12,frame_rate=time(25))
        common={'store_root':str(store),'project_id':project['id']}
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        operations=[{'op':'media.add','asset':receipt['asset']},
            {'op':'clip.append','clip':{'id':'tail','asset_id':receipt['asset']['id'],'source_in':time(4,25),'duration':time(4,25)}},
            {'op':'clip.append','clip':{'id':'head','asset_id':receipt['asset']['id'],'source_in':time(0),'duration':time(4,25)}}]
        request={**common,'expected_revision':0,'operations':operations,'request_id':'append'}
        edit=client.call('session.apply',**request); assert client.call('session.apply',**request)==edit
        saved=client.call('session.get',**common); fields={'project':saved,'input_root':str(output),'output_root':str(output)}
        call('render.run',**fields,output=str(output/'saved.mkv'))
        reordered_audio=pcm[4*1920*4:]+pcm[:4*1920*4]
        check_media(output/'saved.mkv',wanted[4:]+wanted[:4],reordered_audio)
        for n,source in [(0,4),(3,7),(4,0),(7,3)]:
            p=output/f'preview-{n}.png';client.call('preview.frame',**fields,output=str(p),time=time(n,25))
            with Image.open(p) as im:assert im.tobytes()==wanted[source]
            previews+=1
        call('preview.range',**fields,output=str(output/'range.mkv'),start=time(3,25),duration=time(2,25))
        check_media(output/'range.mkv',[wanted[7],wanted[0]],pcm[7*1920*4:]+pcm[:1920*4])
        client.call('session.undo',**common,expected_revision=saved['revision'],request_id='undo')
        restored=client.call('session.get',**common)
        assert restored['clips']==project['clips'] and restored['assets']==project['assets']
    finally:client.close()
    passed.append('temporal.typed_scene_inspection_saved_edits_and_previews')

    # Fixed maximum-work render, independently known to be an opaque constant.
    maximum=copy.deepcopy(scene);maximum.update(id='maximum',width=512,height=512,audio=None,temporal=exposure(32,360,0))
    l=maximum['layers'][0];l.update(canvas=[512,512]);l['frames'][0]['image']=identity(sources/'large.png',sources)
    l['transform']={'position':[0,0],'crop':[0,0,512,512],'scale':1,'quarter_turns':0,'opacity':255}
    start=clock.perf_counter();mr,_=check(maximum,'maximum',lambda n:bytes([255])*(512*512*3));seconds=clock.perf_counter()-start
    assert mr['temporal']['layer_pixel_sample_visits']==67108864 and seconds<90,seconds
    records=copy.deepcopy(maximum);records.update(id='records',width=1,height=1,duration=time(64,25))
    records['layers']=[copy.deepcopy(records['layers'][0]) for _ in range(16)]
    for i,l in enumerate(records['layers']):l.update(id=f'record-{i}',duration=records['duration'])
    rr,_=check(records,'maximum-records',lambda n:bytes([255])*3)
    assert sum(len(t['sampled_parameters']) for t in rr['timing'])==32768
    passed.append('temporal.maximum_render_work_and_sample_record_limits')

    invalid=[]
    def bad(name,mutate,code='INVALID_TEMPORAL',base=None):
        s=copy.deepcopy(base or scene);mutate(s);invalid.append((name,s,code))
    for count in (0,33):bad('samples-'+str(count),lambda s,count=count:s['temporal'].update(samples=count))
    for angle in (-1,361):bad('angle-'+str(angle),lambda s,angle=angle:s['temporal'].update(shutter_angle=rat(angle)))
    for phase in (-361,361):bad('phase-'+str(phase),lambda s,phase=phase:s['temporal'].update(phase=rat(phase)))
    bad('zero-multiple',lambda s:s['temporal'].update(shutter_angle=rat(0)))
    bad('zero-denominator',lambda s:s['temporal'].update(phase={'num':0,'den':0}),'EXPRESSION_DOMAIN')
    bad('precision',lambda s:s['temporal'].update(phase={'num':1,'den':1000000000001}),'EXPRESSION_PRECISION')
    bad('integration',lambda s:s['temporal'].update(integration='linear_rgb'),'INVALID_JSON')
    bad('history',lambda s:s['temporal'].update(history_frames=2),'INVALID_JSON')
    bad('optical-flow',lambda s:s['layers'][0].update(effects=[{'kind':'optical_flow'}]),'INVALID_JSON')
    bad('raster-work',lambda s:(s.update(duration=time(9,25)),s['layers'][0].update(duration=time(9,25))),'LIMIT_EXCEEDED',maximum)
    bad('record-work',lambda s:(s.update(duration=time(65,25)),[l.update(duration=time(65,25)) for l in s['layers']]),'LIMIT_EXCEEDED',records)
    bad('graph-work',lambda s:s['expressions']['nodes'].extend([constant(f'filler{i}',0) for i in range(230)]),'LIMIT_EXCEEDED',expr)
    for name,s,code in invalid:
        p=output/('invalid-'+name+'.mkv')
        result=call('scene.render',code,scene=s,input_root=str(sources),output_root=str(output),output=str(p))
        if name=='optical-flow':assert 'optical_flow' in result['message'] and 'unknown variant' in result['message']
        assert not p.exists()
    existing=hashlib.sha256((output/'animated-boundaries.mkv').read_bytes()).hexdigest()
    call('scene.render','OUTPUT_EXISTS',scene=changes,input_root=str(sources),output_root=str(output),output=str(output/'animated-boundaries.mkv'))
    assert hashlib.sha256((output/'animated-boundaries.mkv').read_bytes()).hexdigest()==existing
    changed=copy.deepcopy(scene);changed['layers'][0]['frames'][0]['image']['sha256']='0'*64
    call('scene.render','MEDIA_CHANGED',scene=changed,input_root=str(sources),output_root=str(output),output=str(output/'changed.mkv'))
    assert not (output/'changed.mkv').exists() and not list(output.glob('.cutbolt-*'))
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    passed.append('temporal.precision_effect_diagnostics_and_publication_preservation')
    result={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'previews':previews,
            'rejections':rejected,'cases':cases,'convergence':convergences,'maximum_render_seconds':seconds,
            'maximum_layer_pixel_sample_visits':67108864,'maximum_layer_sample_records':32768,
            'source_preserved':True,'reference':'Analytic moving-box exposure and independent Fraction time enumeration with high-precision forward pixels',
            'scope':'Encoded-RGB equal-weight midpoint exposures; held source images; no optical flow or history effects'}
    (root/'scene.json').write_text(json.dumps(scene,indent=2)+'\n');(root/'verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
