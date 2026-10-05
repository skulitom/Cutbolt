"""Independent rational HSL/feather selection, Decimal correction and forward composition."""
from engine import ENGINE, per_frame
import argparse
import copy
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image
from jsonschema import Draft202012Validator

from agents import Client
from animation import animated_scene_at, sample
from compositing import rect_at
from grading import D, changed, curve, encoded, hue_saturation, linear, neutral, resolve, sampled, tone
from graphics import shape_pixels
from scenes import audio_bytes, identity, selected, time

ROOT=Path(__file__).resolve().parents[1]

def qround(x):return int(x+F(1,2))

@lru_cache(None)
def hsl(rgb):
    # Exact normalized color geometry, independent of the engine's integer implementation.
    r,g,b=[F(c,255) for c in rgb];high=max(r,g,b);low=min(r,g,b);delta=high-low
    light=(high+low)/2
    if not delta:return None,0,qround(light*1000)
    sat=delta/(1-abs(2*light-1))
    unit=[(c-low)/delta for c in (r,g,b)]
    if r==high:hue=(unit[1]-unit[2])%6
    elif g==high:hue=2+unit[2]-unit[0]
    else:hue=4+unit[0]-unit[1]
    return qround(hue*60000)%360000,qround(sat*1000),qround(light*1000)

def band(value,b):
    if b['low']<=value<=b['high']:return F(1)
    if not b['feather']:return F(0)
    edge=b['low'] if value<b['low'] else b['high']
    return max(F(0),1-F(abs(value-edge),b['feather']))

def qualification(rgb,q):
    if q is None:return F(1)
    hue,saturation,lightness=hsl(rgb);weight=F(1)
    if q.get('hue'):
        h=q['hue']
        if hue is None:weight=F(0)
        else:
            distance=min((hue-h['center'])%360000,(h['center']-hue)%360000)
            if distance<=h['inner']:weight=F(1)
            elif distance>=h['outer']:weight=F(0)
            else:weight=F(h['outer']-distance,h['outer']-h['inner'])
    if q.get('saturation'):weight*=band(saturation,q['saturation'])
    if q.get('lightness'):weight*=band(lightness,q['lightness'])
    return 1-weight if q.get('inverted',False) else weight

def mask_weight(position,mask):
    if mask is None:return F(1)
    x,y=[F(p)+F(1,2) for p in position];left,top,w,h=mask['rect']
    if not w or not h:coverage=F(0)
    elif mask['feather']:
        ramps=[(x-left)/mask['feather'],(left+w-x)/mask['feather'],(y-top)/mask['feather'],(top+h-y)/mask['feather']]
        coverage=min(max(F(0),r) for r in [*ramps,F(1)])
    else:coverage=F(int(left<=x<left+w and top<=y<top+h))
    return 1-coverage if mask.get('inverted',False) else coverage

def resolve_chain(effects,clock):
    result=[]
    for e in copy.deepcopy(effects):
        if e['kind']=='grade':e=resolve(e,clock)
        else:
            e['grade']=resolve(e['grade'],clock)
            if e.get('mix_curve'):e['mix_milli']=sample(e.pop('mix_curve'),clock)
            if e.get('mask'):
                m=e['mask']
                for name,c in m.pop('animation',{}).items():m['rect'][('x','y','width','height').index(name)]=sample(c,clock)
        result.append(e)
    return result

@lru_cache(maxsize=65536)
def corrected(color,g_json):
    g=json.loads(g_json);exposure=D(2)**(D(g['exposure_milli'])/1000);contrast=D(g['contrast_milli'])/1000
    result=[]
    for c,v in enumerate(color):
        v=min(D(1),max(D(0),v*exposure*D(g['white_balance_milli'][c])/1000*contrast+D('0.18')*(1-contrast)))
        result.append(tone(tone(v,g.get('master_curve')),g.get(('red_curve','green_curve','blue_curve')[c])))
    return hue_saturation(tuple(result),g)

@lru_cache(maxsize=131072)
def pixel_reference(pixel,alpha,position,chain_json):
    denominator=pixel[3] if alpha=='premultiplied' else 255
    if not pixel[3]:return (F(0),)*3
    color=tuple(linear(v,denominator) for v in pixel[:3]);changed_pixel=False
    for e in json.loads(chain_json):
        g=e if e['kind']=='grade' else e['grade']
        if neutral([g]):continue
        weight=F(1) if e['kind']=='grade' else F(e['mix_milli'],1000)*qualification(tuple(encoded(v) for v in color),e.get('qualifier'))*mask_weight(position,e.get('mask'))
        if not weight:continue
        w=D(weight.numerator)/weight.denominator;graded=corrected(color,json.dumps(g,sort_keys=True))
        color=tuple(a*(1-w)+b*w for a,b in zip(color,graded));changed_pixel=True
    return tuple(F(encoded(v),255) for v in color) if changed_pixel else tuple(F(v,denominator) for v in pixel[:3])

def selective(qualifier=None,mask=None,mix=1000,grade=None,**extra):
    g=copy.deepcopy(grade or changed(contrast_milli=0));g.pop('kind',None)
    result={'kind':'selective_grade','grade':g,'mix_milli':mix,**extra}
    if qualifier is not None:result['qualifier']={'inverted':False,**qualifier}
    if mask is not None:result['mask']={'feather':0,'inverted':False,**mask}
    return result

def hue(center,inner,outer):return {'hue':{'center':center,'inner':inner,'outer':outer}}

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/n for n in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    palette=[(0,0,0),(255,255,255),(128,128,128),(255,0,0),(255,255,0),(0,255,0),(0,255,255),(0,0,255),(255,0,255),
             (180,90,0),(180,1,0),(180,0,1),(128,129,128),(68,60,60),(64,1,0),(64,0,1),(200,30,40),(30,40,200),(20,180,140),(10,11,10),(254,255,255),(255,254,254),(12,0,1),(1,0,12)]
    source=Image.new('RGBA',(48,16))
    for y in range(16):
        for x in range(48):
            c=palette[x%len(palette)]
            if 1<=y<8:c=tuple((v*(8-y)+128*y+4)//8 for v in c)
            a=255 if y<8 else [0,1,2,17,64,128,254,255][y-8]
            source.putpixel((x,y),(*c,a))
    source.save(sources/'chart.png')
    for name,func in [('premultiplied',lambda p:(*((v*p[3]+127)//255 for v in p[:3]),p[3])),('alternate',lambda p:(p[2],p[0],p[1],p[3]))]:
        image=Image.new('RGBA',source.size);image.putdata([func(p) for p in source.getdata()]);image.save(sources/(name+'.png'))
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;decoded=0;maximum_error=0;passed=[];cases=[]
    def request(cmd,error=None):
        p=subprocess.run([str(exe)],input=json.dumps(cmd).encode(),capture_output=True,timeout=120);r=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and r['error']['code']==error,(error,r)
            return r
        assert p.returncode==0 and r['ok'],r
        return r['result']
    def inspect(s):return request({'command':'scene.inspect','scene':s,'input_root':str(sources)})
    def render(s,name,error=None):return request({'command':'scene.render','scene':s,'input_root':str(sources),'output_root':str(output),'output':str(output/(name+'.mkv'))},error)
    def base(effects,path='chart.png',frames=3):
        layer={'id':'selected','canvas':[64,32],'start':time(0),'duration':time(frames,25),'frames':[{'image':identity(sources/path,sources),'hold':time(frames,25),'offset':[8,8],'anchor':[0,0]}],
               'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,64,32],'scale':1,'quarter_turns':0,'opacity':255},'effects':effects}
        return {'schema_version':1,'id':'selective','width':64,'height':32,'output_scale':2,'duration':time(frames,25),'background':[23,57,101],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    def reference(scene,n):
        canvas=Image.new('RGB',(scene['width'],scene['height']),tuple(scene['background']))
        for layer in animated_scene_at(scene,n)['layers']:
            clock=F(n,25)-F(layer['start']['num'],layer['start']['den'])
            if not 0<=clock<F(layer['duration']['num'],layer['duration']['den']):continue
            if layer.get('graphics'):
                image=shape_pixels(layer['graphics'],layer['canvas']);frame={'offset':[0,0],'anchor':[0,0]}
            else:
                index=selected(layer,n)
                if index is None:continue
                frame=layer['frames'][index];image=Image.open(sources/frame['image']['path']).convert('RGBA')
            chain=json.dumps(resolve_chain(layer.get('effects',[]),clock),sort_keys=True)
            tr=layer['transform'];cx,cy,cw,ch=tr['crop'];scale=tr['scale'];mask=rect_at(layer,n)
            for iy in range(image.height):
                for ix in range(image.width):
                    p=image.getpixel((ix,iy))
                    if not p[3]:continue
                    x,y=ix+frame['offset'][0],iy+frame['offset'][1]
                    if not cx<=x<cx+cw or not cy<=y<cy+ch:continue
                    if mask:
                        a,b,w,h=mask;inside=a<=x<a+w and b<=y<b+h
                        if inside==layer['mask'].get('inverted',False):continue
                    color=pixel_reference(p,layer.get('alpha_mode','straight'),(x,y),chain)
                    px,py=x-cx,y-cy;ax,ay=frame['anchor'][0]-cx,frame['anchor'][1]-cy;w,h=cw,ch
                    for _ in range(tr['quarter_turns']):px,py,ax,ay,w,h=h-1-py,px,h-ay,ax,h,w
                    left=tr['position'][0]+(px-ax)*scale;top=tr['position'][1]+(py-ay)*scale
                    coverage=F(p[3]*tr['opacity'],255*255);mode=layer.get('blend_mode','normal')
                    for dy in range(max(0,top),min(canvas.height,top+scale)):
                        for dx in range(max(0,left),min(canvas.width,left+scale)):
                            old=canvas.getpixel((dx,dy));new=[]
                            for c in range(3):
                                dest=F(old[c],255);v=color[c]
                                blend={'normal':v,'multiply':v*dest,'screen':1-(1-v)*(1-dest)}[mode]
                                new.append(qround(255*(coverage*blend+(1-coverage)*dest)))
                            canvas.putpixel((dx,dy),tuple(new))
        return canvas.resize((canvas.width*scene['output_scale'],canvas.height*scene['output_scale']),Image.Resampling.NEAREST).tobytes()
    def verify_bytes(path,scene,wanted,count):
        nonlocal maximum_error,decoded
        w,h=scene['width']*scene['output_scale'],scene['height']*scene['output_scale'];stride=w*h*3
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=90)
        assert len(raw)==stride*count
        for n in range(count):
            error=max(abs(a-b) for a,b in zip(raw[n*stride:(n+1)*stride],wanted(n)))
            maximum_error=max(maximum_error,error);assert error<=1,(path,n,error)
        assert audio_bytes(path)==b'\0'*(count*1920*4);decoded+=count
        return raw
    def check(scene,name):
        scene=copy.deepcopy(scene);scene['id']=name;info=inspect(scene);receipt=render(scene,name)
        for layer,report in zip(scene['layers'],info['timing']):
            for n,parameters in enumerate(per_frame(report['sampled_parameters'])):
                if parameters is None:continue
                clock=F(n,25)-F(layer['start']['num'],layer['start']['den']);expected=[]
                for e in resolve_chain(layer['effects'],clock):
                    g=e if e['kind']=='grade' else e['grade'];r=sampled(g);r['kind']=e['kind']
                    if e['kind']=='selective_grade':r.update(mix_milli=e['mix_milli'],mask_rect=e.get('mask',{}).get('rect'))
                    expected.append(r)
                assert parameters.get('effects',[])==expected
        frames=int(F(scene['duration']['num'],scene['duration']['den'])*25)
        wanted=lru_cache(None)(lambda n:reference(scene,n));raw=verify_bytes(output/(name+'.mkv'),scene,wanted,frames);cases.append(name)
        return scene,info,receipt,wanted,raw
    def point(case,x,y):
        s=case[0];scale=s['output_scale'];start=(y*scale*s['width']*scale+x*scale)*3
        return tuple(case[-1][start:start+3])

    assert 'selective_grade' in request({'command':'capabilities','section':'all'})['effects']['types']
    baseline=check(base([]),'bypass')
    red=check(base([selective(hue(0,10000,25000))]),'red-soft')
    assert point(red,11,8)==(118,118,118) and point(red,13,8)==palette[5] and point(red,10,8)==palette[2]
    inverted=check(base([selective({**hue(0,10000,25000),'inverted':True})]),'red-inverted')
    assert point(inverted,11,8)==palette[3] and point(inverted,10,8)==(118,118,118)
    check(base([selective(hue(330000,0,60000),mix=350)]),'wrap-soft-mix')
    all_hues=check(base([selective(hue(0,180000,180000))]),'all-defined-hues')
    assert point(all_hues,10,8)==palette[2] and point(all_hues,15,8)==(118,118,118)
    tie=check(base([selective(hue(0,937,937))]),'hue-rounding-boundary')
    assert point(tie,22,8)==palette[14] and point(tie,23,8)==(118,118,118)
    sat=check(base([selective({'saturation':{'low':63,'high':63,'feather':0}})]),'saturation-half-tie')
    assert point(sat,21,8)==(118,118,118) and point(sat,11,8)==palette[3]
    light=check(base([selective({'lightness':{'low':500,'high':500,'feather':0}})]),'lightness-hard-boundary')
    assert point(light,11,8)==(118,118,118) and point(light,10,8)==palette[2]
    combined={**hue(0,15000,45000),'saturation':{'low':300,'high':800,'feather':150},'lightness':{'low':150,'high':700,'feather':200}}
    check(base([selective(combined,grade=changed(exposure_milli=700,white_balance_milli=[1200,900,700]))]),'hsl-product')
    passed.append('selection.hsl_qualifiers_boundaries_inversion')

    # A selected hue turn recolors only the qualified reds: pure red turns 55 degrees to (255,234,0).
    scarf=check(base([selective(hue(0,10000,25000),grade=changed(hue_shift_mdeg=55000))]),'red-to-gold')
    assert point(scarf,11,8)==(255,234,0) and point(scarf,13,8)==palette[5] and point(scarf,10,8)==palette[2]
    check(base([selective({**hue(350000,8000,16000),'saturation':{'low':400,'high':1000,'feather':50}},mix=600,
        grade=changed(exposure_milli=200,hue_shift_mdeg=-300000,saturation_milli=1300))]),'soft-hue-turn')
    spin=base([selective(hue(0,20000,60000),grade=changed(hue_shift_mdeg=0))],frames=12)
    spin['layers'][0]['effects'][0]['grade']['animation']={'hue_shift_mdeg':curve([(0,1,0,'linear'),(11,25,360000,'hold')]),
        'saturation_milli':curve([(0,1,4000,'ease_out'),(11,25,0,'hold')])}
    check(spin,'animated-hue-turn')
    passed.append('selection.hue_saturation_turns')

    for name,rect,feather,invert in [('hard',[10,7,20,15],0,False),('feather',[10,7,20,15],4,False),('inverse',[10,7,20,15],4,True),
            ('thin',[10,7,1,15],1,False),('empty',[10,7,0,15],2,False),('empty-inverse',[10,7,0,15],2,True),('outside',[-90,-90,10,10],4,False)]:
        case=check(base([selective(mask={'rect':rect,'feather':feather,'inverted':invert})]),'mask-'+name)
        if name in ('empty','outside'):assert case[-1]==baseline[-1]
    spatial=base([selective(combined,mask={'rect':[14,9,22,12],'feather':3},mix=800)],frames=3)
    l=spatial['layers'][0];l['frames'][0]['anchor']=[16,12]
    l['transform'].update(crop=[2,4,58,24],position=[32,16],quarter_turns=1,opacity=173)
    l['mask']={'rect':[9,7,40,16],'inverted':False}
    for turn in range(4):
        l['transform']['quarter_turns']=turn;check(spatial,'source-mask-transform-'+str(turn))
    passed.append('selection.feathered_masks_source_coordinates')

    for mode in ('normal','multiply','screen'):
        s=base([selective(combined,mask={'rect':[9,7,35,16],'feather':4},mix=700)],'premultiplied.png')
        s['layers'][0].update(alpha_mode='premultiplied',blend_mode=mode);s['layers'][0]['transform']['opacity']=179
        check(s,'premult-'+mode)
    p=base([],'premultiplied.png');p['layers'][0]['alpha_mode']='premultiplied';b=check(p,'premult-bypass')
    for name,fx in [('unselected',selective(mask={'rect':[1000,1000,10,10]})),('zero-mix',selective(hue(0,180000,180000),mix=0)),('neutral',selective(hue(0,180000,180000),grade=changed()))]:
        p['layers'][0]['effects']=[fx];assert check(p,'premult-'+name)[-1]==b[-1]
    passed.append('selection.alpha_and_unselected_exactness')

    invert=changed(master_curve={'points':[[0,65535],[65535,0]]});cyan=selective(hue(180000,0,0))
    before=check(base([invert,cyan]),'incoming-color-selection');after=check(base([cyan,invert]),'opposite-order')
    assert point(before,11,8)==(118,118,118) and point(after,11,8)==(0,255,255)
    check(base([selective(hue(i*45000,10000,25000),mix=200,grade=changed(exposure_milli=200)) for i in range(8)]),'eight-selections')
    two=base([selective(hue(0,20000,40000))]);overlay=copy.deepcopy(two['layers'][0]);overlay['id']='second';overlay['effects']=[selective(hue(240000,20000,40000))]
    overlay['transform'].update(position=[4,2],opacity=128);two['layers'].append(overlay);check(two,'independent-layer-selections')
    passed.append('selection.ordered_effects_and_layer_isolation')

    moving=base([selective(combined,mask={'rect':[8,8,0,16],'feather':3},mix=1000,grade=changed(exposure_milli=700))],frames=25)
    l=moving['layers'][0];l.update(start=time(2,25),duration=time(20,25),timing='sample_start',end='loop')
    l['frames']=[{'image':identity(sources/name,sources),'hold':time(3 if i==0 else 7,100),'offset':[8,8],'anchor':[0,0]} for i,name in enumerate(('chart.png','alternate.png'))]
    effect=l['effects'][0]
    effect['mix_curve']=curve([(0,1,0,'ease_in_out'),(18,25,1000,'hold')],{'start':time(1,25),'rate':time(3,2),'reverse':False})
    effect['mask']['animation']={'width':curve([(0,1,0,'linear'),(18,25,40,'hold')]),'x':curve([(0,1,8,'hold'),(1,5,12,'linear'),(18,25,6,'hold')])}
    effect['grade']['animation']={'exposure_milli':curve([(0,1,-1000,'ease_out'),(18,25,2000,'hold')])}
    m=check(moving,'animated-selection');assert m[3](0)==m[3](24) and m[3](4)!=m[3](16)
    reordered=copy.deepcopy(m[0]);e=reordered['layers'][0]['effects'][0]
    for c in [e['mix_curve'],*e['mask']['animation'].values(),*e['grade']['animation'].values()]:c['keys'].reverse()
    assert inspect(reordered)['timing'][0]['sampled_parameters']==m[1]['timing'][0]['sampled_parameters']
    assert check(json.loads(json.dumps(reordered)),'reloaded-selection')[-1]==m[-1]
    shape=base([selective(mask={'rect':[8,4,30,24],'feather':4})]);l=shape['layers'][0];l['frames']=[]
    l['graphics']={'kind':'shape','shape':'ellipse','rect':[3,2,54,28],'fill':[180,30,80,150],'stroke':None}
    check(shape,'shape-selection')
    passed.append('selection.animated_mix_masks_and_graphics')

    client=Client(exe)
    try:
        client.initialize();tool=next(t for t in client.rpc('tools/list')['result']['tools'] if t['name']=='cutbolt_scene_inspect')
        args={'scene':m[0],'input_root':str(sources)};Draft202012Validator(tool['inputSchema']).validate(args)
        assert client.call('scene.inspect',**args)==m[1]
        template={'schema_version':1,'id':'selection-template','scene':m[0],'parameters':[]}
        assert client.call('graphics.instantiate',template=template,instance_id='selection-copy',values={},input_root=str(sources))['scene']['layers'][0]['effects']==m[0]['layers'][0]['effects']
        project=request({'command':'project.create','id':'selection-edit','width':128,'height':64,'frame_rate':time(25)})
        client.call('session.create',project=project,request_id='create',store_root=str(store))
        ops=[{'op':'media.add','asset':m[2]['asset']},{'op':'clip.append','clip':{'id':'selection','asset_id':m[0]['id'],'source_in':time(4,25),'duration':time(12,25)}}]
        fields={'store_root':str(store),'project_id':'selection-edit','expected_revision':0,'request_id':'compiled','operations':ops}
        receipt=client.call('session.apply',**fields);assert receipt==client.call('session.apply',**fields)
        saved=client.call('session.get',store_root=str(store),project_id='selection-edit')
        request({'command':'render.run','project':saved,'input_root':str(output),'output_root':str(output),'output':str(output/'saved.mkv')})
        verify_bytes(output/'saved.mkv',m[0],lambda n:m[3](n+4),12)
    finally:client.close()
    passed.append('selection.mcp_templates_saved_session')

    invalid=[]
    def bad(e,code='INVALID_EFFECT'):invalid.append((base([e]),code))
    bad(selective());bad(selective({}));bad(selective(hue(360000,0,1)));bad(selective(hue(0,2,1)));bad(selective(hue(0,0,180001)))
    for key in ('saturation','lightness'):
        for b in ({'low':1001,'high':1001,'feather':0},{'low':500,'high':499,'feather':1},{'low':0,'high':1000,'feather':1001}):bad(selective({key:b}))
    bad(selective(hue(0,0,1),mix=1001));bad(selective(hue(0,0,1),mix_curve=curve([(0,1,-1,'hold')])),'INVALID_ANIMATION')
    bad(selective(hue(0,0,1),mix_curve=curve([(0,1,0,'linear'),(0,2,1000,'hold')])),'INVALID_ANIMATION')
    bad(selective(mask={'rect':[0,0,-1,5]}),'INVALID_MASK');bad(selective(mask={'rect':[32769,0,1,1]}),'INVALID_MASK')
    bad(selective(mask={'rect':[0,0,1,1],'feather':4097}));bad(selective(mask={'rect':[0,0,1,1],'animation':{}}),'INVALID_MASK')
    bad(selective(mask={'rect':[0,0,1,1],'animation':{'width':curve([(0,1,-1,'hold')])}}),'INVALID_ANIMATION')
    bad(selective(hue(0,0,1),grade=changed(exposure_milli=8001)));bad(selective(hue(0,0,1),grade=changed(hue_shift_mdeg=360001)))
    bad(selective({'hue':{'center':-1,'inner':0,'outer':1}}),'INVALID_JSON');bad(selective({'hue':{'center':0.5,'inner':0,'outer':1}}),'INVALID_JSON')
    bad(selective({'lightness':{'low':0,'high':1000,'feather':0},'luma':100}),'INVALID_JSON')
    extra=selective(hue(0,0,1));extra['grade']['kind']='grade';bad(extra,'INVALID_JSON')
    invalid.append((base([selective(hue(0,0,1))]*9),'INVALID_EFFECT'))
    hidden=base([selective({})]);hidden['layers'][0]['transform']['opacity']=0;invalid.append((hidden,'INVALID_EFFECT'))
    original_outputs={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(s,code) in enumerate(invalid):render(s,'rejected-'+str(i),code)
    render(m[0],'animated-selection','OUTPUT_EXISTS')
    assert original_outputs=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('selection.validation_and_preservation')
    report={'passed':passed,'decoded_frames':decoded,'silent_stereo_sample_frames':decoded*1920,'render_cases':cases,'rejected_cases':len(invalid)+1,
            'maximum_rgb_error':maximum_error,'rgb_tolerance':1,'reference':'Exact Fraction HSL and feather/mix weights, 48-digit Decimal grading, original charts and forward per-pixel composition; unselected stored premultiplied colors remain fractional until final blending',
            'scope':'HSL qualifiers, hard/soft/inverted source-canvas rectangle masks, ordered selected grades, animated grade/mix/mask, preserved alpha; no chroma-key, tracking or general color-management claim'}
    (root/'animated-scene.json').write_text(json.dumps(m[0],indent=2)+'\n',encoding='utf-8')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
