"""Original charts, high-precision color equations and independent forward compositing."""
from engine import ENGINE
import argparse
import copy
from decimal import Decimal as D, getcontext, ROUND_HALF_UP
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image
from jsonschema import Draft202012Validator

from agents import Client
from animation import sample
from compositing import expected_frame
from graphics import PRIMARY, make_font, shape_pixels, text_pixels
from scenes import audio_bytes, identity, selected, time

ROOT=Path(__file__).resolve().parents[1]
getcontext().prec=48

def grade():
    return dict(kind='grade',exposure_milli=0,contrast_milli=1000,white_balance_milli=[1000]*3)

def changed(**fields):
    result=grade();result.update(fields);return result

def resolve(effect,clock):
    result=copy.deepcopy(effect)
    for key,curve in result.pop('animation',{}).items():
        value=sample(curve,clock)
        if key in ('exposure_milli','contrast_milli'):result[key]=value
        else:result['white_balance_milli'][('red_balance_milli','green_balance_milli','blue_balance_milli').index(key)]=value
    return result

def neutral(effects):
    return all(e['exposure_milli']==0 and e['contrast_milli']==1000 and e['white_balance_milli']==[1000]*3
        and all(all(x==y for x,y in e[k]['points']) for k in ('master_curve','red_curve','green_curve','blue_curve') if e.get(k)) for e in effects)

@lru_cache(maxsize=8192)
def linear(value,denominator):
    v=D(value)/denominator
    return v/D('12.92') if v<=D('0.04045') else ((v+D('0.055'))/D('1.055'))**D('2.4')

def encoded(value):
    v=D('12.92')*value if value<=D('0.0031308') else D('1.055')*value**(D(5)/12)-D('0.055')
    return int((v*255).to_integral_value(rounding=ROUND_HALF_UP))

def tone(value,curve):
    if not curve:return value
    # A weighted sum of the two surrounding output knots, evaluated in Decimal.
    x=value*65535
    a,b=next((a,b) for a,b in zip(curve['points'],curve['points'][1:]) if a[0]<=x<=b[0])
    return (D(a[1])*(D(b[0])-x)+D(b[1])*(x-D(a[0])))/(D(b[0]-a[0])*65535)

@lru_cache(maxsize=262144)
def channel(value,denominator,c,chain):
    v=linear(value,denominator)
    for e in json.loads(chain):
        # Original grading contract, evaluated at 48 digits, not the runtime's binary floats/LUTs.
        gain=D(2)**(D(e['exposure_milli'])/1000)*D(e['white_balance_milli'][c])/1000
        slope=D(e['contrast_milli'])/1000
        v=min(D(1),max(D(0),v*gain*slope+D('0.18')*(1-slope)))
        v=tone(tone(v,e.get('master_curve')),e.get(('red_curve','green_curve','blue_curve')[c]))
    return encoded(v)

def curve(entries,retime=None):
    result={'keys':[{'time':time(n,d),'value':v,'interpolation':mode} for n,d,v,mode in entries]}
    if retime:result['retime']=retime
    return result

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,oracle,store=[root/n for n in ('sources','output','oracle','store')]
    for p in (sources,output,oracle,store):p.mkdir()
    image=Image.new('RGBA',(64,16))
    alphas=[0,1,2,17,64,128,254,255]
    for y in range(16):
        for x in range(64):
            value=x+(y%4)*64
            p=(value,value,value,255) if y<4 else (value,(x*13+y*19)%256,(255-x*3+y)%256,255 if y<8 else alphas[y-8])
            image.putpixel((x,y),p)
    image.save(sources/'chart.png')
    for name,operation in [('premultiplied',lambda p:tuple((c*p[3]+127)//255 for c in p[:3])+(p[3],)),
                           ('alternate',lambda p:(255-p[0],p[2],p[1],p[3]))]:
        result=Image.new('RGBA',image.size);result.putdata([operation(p) for p in image.getdata()]);result.save(sources/(name+'.png'))
    Image.new('RGBA',(64,16),(89,124,170,255)).save(sources/'neutral.png')
    Image.new('RGBA',(8,8),(128,64,200,128)).save(sources/'small.png')
    make_font(sources/'original.ttf',PRIMARY)
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;passed=[];decoded=0;maximum_error=0;cases=[]
    def request(value,error=None):
        p=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=120)
        r=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and r['error']['code']==error,(error,r)
            return r
        assert p.returncode==0 and r['ok'],r
        return r['result']
    def inspect(scene):return request({'command':'scene.inspect','scene':scene,'input_root':str(sources)})
    def render(scene,name,error=None):return request({'command':'scene.render','scene':scene,'input_root':str(sources),'output_root':str(output),'output':str(output/(name+'.mkv'))},error)
    def base(effects=None,path='chart.png',frames=3):
        layer={'id':'chart','canvas':[64,16],'start':time(0),'duration':time(frames,25),
               'frames':[{'image':identity(sources/path,sources),'hold':time(frames,25),'offset':[0,0],'anchor':[0,0]}],
               'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,64,16],'scale':1,'quarter_turns':0,'opacity':255}}
        if effects is not None:layer['effects']=effects
        return {'schema_version':1,'id':'grading','width':64,'height':16,'output_scale':2,'duration':time(frames,25),'background':[23,57,101],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    def reference(scene,n,name):
        ref=copy.deepcopy(scene)
        for i,layer in enumerate(ref['layers']):
            clock=F(n,25)-F(layer['start']['num'],layer['start']['den'])
            if not 0<=clock<F(layer['duration']['num'],layer['duration']['den']):continue
            if layer.get('graphics'):
                graphic=layer.pop('graphics')
                if graphic['kind']=='text':source,_,_=text_pixels(graphic,layer['canvas'],{'original.ttf':PRIMARY})
                else:source=shape_pixels(graphic,layer['canvas'])
                layer['frames']=[{'image':{},'hold':layer['duration'],'offset':[0,0],'anchor':[0,0]}];index=0
            else:
                index=selected(layer,n)
                if index is None:continue
                source=Image.open(sources/layer['frames'][index]['image']['path']).convert('RGBA')
            effects=[resolve(e,clock) for e in layer.pop('effects',[])]
            if not neutral(effects):
                chain=json.dumps(effects,sort_keys=True);pixels=[]
                premult=layer.get('alpha_mode')=='premultiplied'
                for p in source.getdata():
                    pixels.append(tuple(channel(p[c],p[3] if premult else 255,c,chain) for c in range(3))+(p[3],) if p[3] else (0,0,0,0))
                source.putdata(pixels);layer['alpha_mode']='straight'
            path=oracle/f'{name}-{n}-{i}.png';source.save(path)
            layer['frames'][index]['image']=identity(path,oracle)
        return expected_frame(ref,oracle,n)
    def verify_bytes(path,scene,expected,count):
        nonlocal decoded,maximum_error
        w,h=scene['width']*scene['output_scale'],scene['height']*scene['output_scale']
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=90)
        assert len(raw)==w*h*3*count
        for n in range(count):
            wanted=expected(n);actual=raw[n*w*h*3:(n+1)*w*h*3]
            err=max(abs(a-b) for a,b in zip(actual,wanted));maximum_error=max(maximum_error,err)
            assert err<=1,(path,n,err)
        assert audio_bytes(path)==b'\0'*(count*1920*4)
        decoded+=count
        return raw
    def check(scene,name):
        scene=copy.deepcopy(scene);scene['id']=name;info=inspect(scene);receipt=render(scene,name)
        for layer,report in zip(scene['layers'],info['timing']):
            for n,parameters in enumerate(report['sampled_parameters']):
                if parameters is None:continue
                clock=F(n,25)-F(layer['start']['num'],layer['start']['den'])
                expected=[{k:e[k] for k in ('kind','exposure_milli','contrast_milli','white_balance_milli')} for e in (resolve(e,clock) for e in layer.get('effects',[]))]
                assert parameters.get('effects',[])==expected
        count=int(F(scene['duration']['num'],scene['duration']['den'])*25)
        expected=lru_cache(None)(lambda n:reference(scene,n,name))
        raw=verify_bytes(output/(name+'.mkv'),scene,expected,count);cases.append(name)
        return scene,info,receipt,expected,raw

    assert request({'command':'capabilities'})['effects']['working_space']=='linear_srgb_f64'
    bypass=check(base(),'bypass');identity_case=check(base([grade()]),'neutral')
    assert bypass[-1]==identity_case[-1]
    identity_curve={'points':[[0,0],[100,100],[32768,32768],[65535,65535]]}
    assert check(base([changed(master_curve=identity_curve,red_curve=identity_curve)]),'identity-curves')[-1]==bypass[-1]
    plus=check(base([changed(exposure_milli=1000)]),'one-stop')
    minus=check(base([changed(exposure_milli=-1000)]),'minus-stop')
    flat=check(base([changed(contrast_milli=0)]),'flat-contrast')
    # Analytical anchors independent of the generic oracle: 18% gray and exact one-stop mappings.
    def point(case,x,y):
        scene=case[0];scale=scene['output_scale'];w=scene['width']*scale
        start=(y*scale*w+x*scale)*3;return tuple(case[-1][start:start+3])
    assert point(plus,0,2)==(176,176,176) and point(minus,0,2)==(92,92,92)
    assert all(point(flat,x,y)==(118,118,118) for y in range(4) for x in range(64))
    balanced=check(base([changed(white_balance_milli=[2000,1000,500])],'neutral.png'),'white-balance')
    assert max(point(balanced,0,0))-min(point(balanced,0,0))<=1
    check(base([changed(exposure_milli=-8000,contrast_milli=4000,white_balance_milli=[100,4000,1000])]),'control-min-max')
    check(base([changed(exposure_milli=8000,contrast_milli=1250,white_balance_milli=[4000,100,2000])]),'highlights-clipped')
    passed.append('grading.exposure_contrast_white_balance_charts')

    tone_curve={'points':[[0,2000],[4000,1000],[17000,30000],[40000,52000],[65535,63000]]}
    inverted={'points':[[0,65535],[65535,0]]}
    check(base([changed(master_curve=tone_curve,red_curve=inverted,green_curve={'points':[[0,0],[10000,40000],[65535,65535]]},blue_curve={'points':[[0,0],[65535,45000]]})]),'master-channel-curves')
    extremes=check(base([changed(master_curve=inverted,red_curve=inverted)]),'double-inversion-red')
    assert point(extremes,0,0)==(0,255,255) and point(extremes,63,3)==(255,0,0)
    ab=check(base([changed(exposure_milli=1000),changed(exposure_milli=-1000)]),'ordered-bright-dark')
    ba=check(base([changed(exposure_milli=-1000),changed(exposure_milli=1000)]),'ordered-dark-bright')
    assert point(ab,63,3)==(188,188,188) and point(ba,63,3)==(255,255,255) and ab[-1]!=ba[-1]
    check(base([changed(exposure_milli=125),changed(contrast_milli=800)]*4),'eight-stages')
    passed.append('grading.curves_chain_order_and_clipping')

    for mode in ('normal','multiply','screen'):
        s=base([changed(exposure_milli=750,contrast_milli=800,white_balance_milli=[1500,800,1200],master_curve=tone_curve)],'premultiplied.png')
        s['layers'][0].update(alpha_mode='premultiplied',blend_mode=mode)
        s['layers'][0]['transform']['opacity']=173
        check(s,'premult-'+mode)
    p=base(None,'premultiplied.png');p['layers'][0]['alpha_mode']='premultiplied'
    a=check(p,'premult-bypass');p['layers'][0]['effects']=[grade()]
    assert check(p,'premult-neutral')[-1]==a[-1]
    holes=base([changed(master_curve={'points':[[0,65535],[65535,65535]]})],'small.png')
    holes['layers'][0]['frames'][0]['offset']=[4,4]
    h=check(holes,'trimmed-source-holes');assert point(h,0,0)==tuple(holes['background']) and point(h,5,5)!=tuple(holes['background'])
    passed.append('grading.alpha_modes_edges_and_neutral_bypass')

    moving=base([changed(master_curve=tone_curve)],frames=25);l=moving['layers'][0]
    l.update(start=time(2,25),duration=time(20,25),timing='sample_start',end='loop')
    l['frames']=[{'image':identity(sources/name,sources),'hold':time(7 if i else 3,100),'offset':[0,0],'anchor':[0,0]} for i,name in enumerate(('chart.png','alternate.png'))]
    l['effects'][0]['animation']={
        'exposure_milli':curve([(0,1,-1000,'ease_in'),(3,10,1000,'linear'),(18,25,1500,'hold')]),
        'contrast_milli':curve([(0,1,500,'hold'),(1,5,1400,'ease_out'),(18,25,1000,'hold')]),
        'red_balance_milli':curve([(0,1,600,'ease_in_out'),(18,25,1800,'hold')],{'start':time(1,25),'rate':time(3,2),'reverse':True}),
        'green_balance_milli':curve([(0,1,800,'linear'),(123,1000,1600,'ease_in'),(18,25,1000,'hold')]),
        'blue_balance_milli':curve([(0,1,2000,'ease_out'),(18,25,500,'hold')])}
    l['mask']={'rect':[2,1,60,14],'animation':{'width':curve([(0,1,30,'linear'),(18,25,64,'hold')])}}
    l['animation']={'position_x':curve([(0,1,3,'linear'),(18,25,-4,'hold')]),'opacity':curve([(0,1,80,'linear'),(18,25,255,'hold')])}
    m=check(moving,'animated-grade')
    assert m[3](2)!=m[3](8)!=m[3](19) and m[3](0)==m[3](24)
    shuffled=copy.deepcopy(m[0])
    for c in shuffled['layers'][0]['effects'][0]['animation'].values():c['keys'].reverse()
    # Static effect descriptions retain input key order; sampled values and pixels must not change.
    assert inspect(shuffled)['timing'][0]['sampled_parameters']==m[1]['timing'][0]['sampled_parameters']
    assert check(json.loads(json.dumps(shuffled)),'reloaded-grade')[-1]==m[-1]
    passed.append('grading.animated_local_clock_and_retime')

    graphics=base([changed(exposure_milli=1000,white_balance_milli=[1000,700,1400])],frames=3)
    graphics['height']=32;g=graphics['layers'][0];g['canvas']=[64,32];g['frames']=[];g['transform']['crop']=[0,0,64,32]
    g['graphics']={'kind':'shape','shape':'rectangle','rect':[1,2,58,26],'fill':[128,64,200,128],'stroke':{'color':[200,100,30,220],'width':2}}
    check(graphics,'graded-shape')
    g['graphics']={'kind':'text','text':'ABC','fonts':[identity(sources/'original.ttf',sources)],'size':20,'color':[128,64,200,180],'rect':[0,0,64,32],'line_height':24,'letter_spacing':0,'align':'left','wrap':'none','overflow':'reject'}
    check(graphics,'graded-text')
    passed.append('grading.graphics_sources')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools']
        tool=next(t for t in catalog if t['name']=='cutbolt_scene_inspect')
        args={'scene':m[0],'input_root':str(sources)};Draft202012Validator(tool['inputSchema']).validate(args)
        assert client.call('scene.inspect',**args)==m[1]
        template={'schema_version':1,'id':'grade-preset','scene':m[0],'parameters':[]}
        assert client.call('graphics.instantiate',template=template,values={},instance_id='grade-copy',input_root=str(sources))['scene']['layers'][0]['effects']==m[0]['layers'][0]['effects']
        project=request({'command':'project.create','id':'graded-edit','width':128,'height':32,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),request_id='create',project=project)
        ops=[{'op':'media.add','asset':m[2]['asset']},{'op':'clip.append','clip':{'id':'graded','asset_id':m[0]['id'],'source_in':time(4,25),'duration':time(12,25)}}]
        receipt=client.call('session.apply',store_root=str(store),project_id='graded-edit',expected_revision=0,request_id='grade-asset',operations=ops)
        assert receipt==client.call('session.apply',store_root=str(store),project_id='graded-edit',expected_revision=0,request_id='grade-asset',operations=ops)
        saved=client.call('session.get',store_root=str(store),project_id='graded-edit')
        request({'command':'render.run','project':saved,'input_root':str(output),'output_root':str(output),'output':str(output/'saved.mkv')})
        verify_bytes(output/'saved.mkv',m[0],lambda n:m[3](n+4),12)
    finally:client.close()
    passed.append('grading.mcp_templates_and_saved_session')

    invalid=[]
    def bad(effect,code='INVALID_EFFECT'):
        invalid.append((base([effect]),code))
    for effect in [changed(exposure_milli=8001),changed(exposure_milli=-8001),changed(contrast_milli=4001),changed(white_balance_milli=[99,1000,1000]),changed(white_balance_milli=[1000,4001,1000]),changed(animation={})]:bad(effect)
    for points in ([],[[0,0]],[[1,0],[65535,65535]],[[0,0],[65534,65535]],[[0,0],[0,10],[65535,65535]],[[0,0],[200,200],[100,100],[65535,65535]],[[i*100,i] for i in range(32)]+[[65535,65535]]):bad(changed(master_curve={'points':points}))
    for field,value in [('exposure_milli',8001),('contrast_milli',-1),('red_balance_milli',99),('green_balance_milli',4001),('blue_balance_milli',0)]:
        bad(changed(animation={field:curve([(0,1,value,'hold')])}),'INVALID_ANIMATION')
    bad(changed(animation={'exposure_milli':curve([(1,1,100,'hold')])}),'INVALID_ANIMATION')
    bad(changed(animation={'exposure_milli':curve([(0,1,0,'linear'),(0,2,100,'hold')])}),'INVALID_ANIMATION')
    bad(changed(animation={'exposure_milli':{'keys':[{'time':{'num':1,'den':0},'value':0,'interpolation':'hold'}]}}),'INVALID_TIME')
    bad(changed(contrast_milli=1.5),'INVALID_JSON');bad(changed(white_balance_milli=[1000,1000]),'INVALID_JSON')
    bad(changed(master_curve={'points':[[0,0],[65535,65536]]}),'INVALID_JSON')
    bad(changed(unknown=True),'INVALID_JSON');bad({'kind':'unknown'},'INVALID_JSON')
    invalid.append((base([grade()]*9),'INVALID_EFFECT'))
    invisible=base([changed(exposure_milli=9000)]);invisible['layers'][0]['transform']['opacity']=0;invalid.append((invisible,'INVALID_EFFECT'))
    bad_alpha=base([grade()]);bad_alpha['layers'][0]['alpha_mode']='premultiplied';invalid.append((bad_alpha,'INVALID_ALPHA'))
    original_outputs={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(s,code) in enumerate(invalid):render(s,'rejected-'+str(i),code)
    render(m[0],'animated-grade','OUTPUT_EXISTS')
    assert original_outputs=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('grading.validation_and_preservation')
    report={'passed':passed,'decoded_frames':decoded,'silent_stereo_sample_frames':decoded*1920,'render_cases':cases,'rejected_cases':len(invalid)+1,
            'maximum_rgb_error':maximum_error,'rgb_tolerance':1,'reference':'48-digit Decimal sRGB and original grading equations, explicit analytical anchors, Fraction property clocks and independent forward compositor',
            'scope':'Eight ordered grades per scene layer, exposure/contrast/diagonal white balance/master and RGB curves, five animated controls, fixed linear-sRGB working space with explicit clipping; no C01/C03/C04/C05 or keying claim'}
    (root/'animated-scene.json').write_text(json.dumps(m[0],indent=2)+'\n',encoding='utf-8')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
