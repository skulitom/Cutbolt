"""Decimal geometry, Fraction taps and independent f64 references for wide layer frames."""
import argparse
import copy
from decimal import Decimal as D, ROUND_HALF_UP
from fractions import Fraction as F
import hashlib
import itertools
import json
import os
from pathlib import Path
import random
import subprocess

from engine import BUILD
from scenes import time
from spatial import state
from tracking import coverage, spatial
from working_color import Q, quantize
from working_effects import exact_sample, nearest, curve, effect_pixel

ROOT=Path(__file__).resolve().parents[1]
GRID=65536


def reference(request):
    output=copy.deepcopy(request['backdrop']); width,height=request['size']
    for draw in request['draws']:
        layer=copy.deepcopy(draw['layer']); layer['start']=time(0)
        when=F(draw['time']['num'],draw['time']['den'])
        tr=layer['transform']; tr.setdefault('spatial',spatial())
        spec=tr['spatial']; _,_,inverse,_=state(layer,when*25,draw['anchor'])
        uncompensated=None
        if spec.get('compensation'):
            original=copy.deepcopy(layer); del original['transform']['spatial']['compensation']
            uncompensated=state(original,when*25,draw['anchor'])[2]
        opacity=nearest(exact_sample(layer['animation']['opacity'],when)*257) if layer.get('animation',{}).get('opacity') else tr['opacity']*257
        mask=layer.get('mask'); rect=list(mask['rect']) if mask else None
        if mask:
            for key,animation in mask.get('animation',{}).items(): rect[['x','y','width','height'].index(key)]=nearest(exact_sample(animation,when))
        cx,cy,cw,ch=tr['crop']; ox,oy=draw['offset']; iw,ih=draw['size']
        processed={}
        def tap(x,y):
            if spec['edge']=='clamp': x=min(cx+cw-1,max(cx,x)); y=min(cy+ch-1,max(cy,y))
            if not cx<=x<cx+cw or not cy<=y<cy+ch or not ox<=x<ox+iw or not oy<=y<oy+ih: return [F(0)]*4
            mask_value=coverage(mask,rect,x,y) if mask else F(1)
            if (x,y) not in processed:
                processed[x,y]=effect_pixel(draw['pixels'][(y-oy)*iw+x-ox],[x,y],layer.get('effects',[]),when,{})
            pixel=processed[x,y]; alpha=F(pixel[3],Q)*mask_value
            return [F(v,Q)*alpha for v in pixel[:3]]+[alpha]
        def mapped(matrix,x,y):
            a,b,c,d,e,f=matrix
            return [int(v.to_integral_value(rounding=ROUND_HALF_UP)) for v in
                    [(a*(D(x)+D('.5'))+b*(D(y)+D('.5'))+c)*GRID,
                     (d*(D(x)+D('.5'))+e*(D(y)+D('.5'))+f)*GRID]]
        for y in range(height):
            for x in range(width):
                if spec.get('viewport'):
                    vx,vy,vw,vh=spec['viewport']
                    if not vx<=x<vx+vw or not vy<=y<vy+vh: continue
                if uncompensated:
                    u,v=mapped(uncompensated,x,y); vx,vy,vw,vh=spec['compensation']['viewport']
                    if not vx*GRID<=u<(vx+vw)*GRID or not vy*GRID<=v<(vy+vh)*GRID: continue
                u,v=mapped(inverse,x,y)
                if spec['edge']=='clamp' and not (cx*GRID<=u<(cx+cw)*GRID and cy*GRID<=v<(cy+ch)*GRID): continue
                if spec['sampling']=='nearest': weights=[(u//GRID,v//GRID,F(1))]
                else:
                    px,py=F(u,GRID)-F(1,2),F(v,GRID)-F(1,2); xx,yy=px.numerator//px.denominator,py.numerator//py.denominator
                    fx,fy=px-xx,py-yy
                    weights=[(xx,yy,(1-fx)*(1-fy)),(xx+1,yy,fx*(1-fy)),(xx,yy+1,(1-fx)*fy),(xx+1,yy+1,fx*fy)]
                filtered=[F(0)]*4
                for xx,yy,weight in weights:
                    if weight: filtered=[a+weight*b for a,b in zip(filtered,tap(xx,yy))]
                alpha=float(filtered[3])*opacity/Q
                if not alpha: continue
                source=[float(v/filtered[3]) for v in filtered[:3]]
                old=[v/Q for v in output[y*width+x]]; total=alpha+old[3]*(1-alpha)
                mode=layer.get('blend_mode','normal'); color=[]
                for a,b in zip(source,old[:3]):
                    blend=a if mode=='normal' else a*b if mode=='multiply' else a+b-a*b
                    color.append(((1-alpha)*old[3]*b+alpha*(1-old[3])*a+alpha*old[3]*blend)/total)
                output[y*width+x]=[quantize(v) for v in color+[total]]
    return output


def run(root):
    root=root.resolve(); assert root!=ROOT and ROOT not in root.parents; root.mkdir(parents=True,exist_ok=True)
    sources=root/'sources'; sources.mkdir(exist_ok=True)
    subprocess.run(['cargo','build','--locked','--example','working_layers_fixture'],cwd=ROOT,check=True)
    helper=BUILD/'examples'/('working_layers_fixture.exe' if os.name=='nt' else 'working_layers_fixture')
    rng=random.Random(784157); cases=[]; digest=hashlib.sha256(); rejected=0
    def call(request,error=None):
        nonlocal rejected
        result=subprocess.run([str(helper),str(sources)],input=json.dumps(request),text=True,encoding='utf-8',capture_output=True,timeout=120)
        response=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and response['error']['code']==error,response
            rejected+=1; return
        assert result.returncode==0 and response['ok'],response
        digest.update(json.dumps(response['result'],sort_keys=True,separators=(',',':')).encode())
        return response['result']['pixels']
    def chart():
        return [[rng.randrange(Q+1) for _ in range(3)]+[[0,1,17,257,32768,Q][i%6]] for i in range(8*6)]
    def draw():
        return dict(layer=dict(canvas=[12,10],transform=dict(position=[11,8],crop=[1,1,10,8],scale=1,quarter_turns=0,opacity=173)),
                    duration=time(4),time=time(733,997),anchor=[6,5],size=[8,6],offset=[2,2],pixels=chart())
    def request(d): return dict(size=[23,17],backdrop=chart_backdrop(),draws=[d])
    def chart_backdrop(): return [[rng.randrange(Q+1) for _ in range(3)]+[[0,127,32767,Q][i%4]] for i in range(23*17)]
    def check(name,req):
        expected=reference(req); actual=call(req)
        assert len(actual)==len(expected)
        maximum=max(abs(a-b) for p,q in zip(actual,expected) for a,b in zip(p,q))
        assert maximum<=len(req['draws']),(name,maximum,next((i,p,q) for i,(p,q) in enumerate(zip(actual,expected)) if max(abs(a-b) for a,b in zip(p,q))>len(req['draws'])))
        cases.append(dict(name=name,pixels=len(actual),maximum_unorm16_error=maximum,tolerance=len(req['draws'])))
        return actual
    for turns,scale in itertools.product(range(4),(1,3)):
        d=draw();d['layer']['transform'].update(quarter_turns=turns,scale=scale)
        check(f'integer_{turns}_{scale}',request(d))
    specifications=[spatial(),spatial(sampling='bilinear',translate_milli=[379,-217]),
                    spatial(sampling='bilinear',edge='clamp',translate_milli=[-501,501]),
                    spatial(rotation_mdeg=31777,scale_milli=[1397,713],sampling='bilinear'),
                    spatial(flip=[True,False],pixel_aspect=time(4,3),sampling='bilinear'),
                    spatial(flip=[True,True],rotation_mdeg=-63333,viewport=[3,2,14,11],sampling='bilinear'),
                    spatial(fit=dict(size=[17,9],mode='contain'),sampling='bilinear'),
                    spatial(fit=dict(size=[17,9],mode='cover',window_size=[8,6]),sampling='bilinear'),
                    spatial(fit=dict(size=[17,9],mode='stretch'),sampling='nearest')]
    for index,spec in enumerate(specifications):
        for mode in ('normal','multiply','screen'):
            d=draw();d['layer'].update(blend_mode=mode);d['layer']['transform']['spatial']=spec
            check(f'spatial_{index}_{mode}',request(d))
    for edge,inverted in itertools.product(('inner','centered','outer'),(False,True)):
        d=draw();d['layer']['transform']['spatial']=spatial(sampling='bilinear',rotation_mdeg=17331)
        d['layer']['mask']=dict(rect=[3,2,5,5],inverted=inverted,feather=dict(radius=3,edge=edge),animation=dict(x=curve([(0,1,1),(3,1,4)])))
        check(f'mask_{edge}_{inverted}',request(d))
    d=draw();d['layer']['transform']['spatial']=spatial(sampling='bilinear',rotation_mdeg=11831,
        compensation=dict(center_milli=[6000,5000],translation_x_milli=curve([(0,1,317),(3,1,-117)]),
                          translation_y_milli=curve([(0,1,-239),(3,1,411)]),rotation_mdeg=curve([(0,1,-7331),(3,1,9777)]),
                          zoom_milli=1123,viewport=[1,1,9,7]))
    check('stabilized_viewport',request(d))
    animated=draw();animated['layer']['transform']['spatial']=spatial(sampling='bilinear',animation=dict(
        translate_x_milli=curve([(0,1,-231),(3,1,713)],'ease_in_out'),rotation_mdeg=curve([(0,1,-17557),(3,1,30113)],'ease_out')))
    animated['layer']['animation']=dict(position_x=curve([(0,1,9),(3,1,15)]),opacity=curve([(0,1,0),(3,1,255)],'ease_in'))
    grade=dict(kind='grade',exposure_milli=233,contrast_milli=1127,white_balance_milli=[1090,977,1041])
    key=dict(kind='chroma_key',key_rgb=[0,255,0],inner_milli=13,outer_milli=757,strength_milli=913)
    animated['layer']['effects']=[grade,key]
    for clock in (F(1,30000),F(1,1000),F(733,997),F(299,100)):
        d=copy.deepcopy(animated);d['time']=time(clock.numerator,clock.denominator)
        check(f'animated_effects_{clock}',request(d))
    stack=request(copy.deepcopy(animated));stack['draws']+=[draw(),copy.deepcopy(animated)]
    stack['draws'][1]['layer']['blend_mode']='screen';stack['draws'][2]['layer']['blend_mode']='multiply'
    check('three_transparent_layers',stack)
    d=draw();d['layer']['transform']['position']=[-32768,32768]
    off=request(d);assert check('off_canvas_no_change',off)==off['backdrop']
    d=draw();d['pixels']=[[Q,0,Q,0]]*48
    hidden=request(d);assert check('hidden_rgb_no_change',hidden)==hidden['backdrop']
    d=draw();d['layer']['transform'].update(opacity=0,spatial=spatial(sampling='bilinear'))
    zero=request(d);assert check('zero_opacity_no_change',zero)==zero['backdrop']
    d=dict(layer=dict(canvas=[4096,1],transform=dict(position=[0,0],crop=[0,0,4096,1],scale=1,quarter_turns=0,opacity=255),
                     animation=dict(opacity=curve([(0,1,0),(12,1,255)])),effects=[grade]),
           duration=time(12),time=time(1,1000),anchor=[0,0],size=[4096,1],offset=[0,0],pixels=[[i,i,i,Q] for i in range(4096)])
    ramp=check('graded_gradient_fractional_fade',dict(size=[4096,1],backdrop=[[0,0,0,0]]*4096,draws=[d]))
    assert all(p[3]==5 for p in ramp),{p[3] for p in ramp}
    # Contrast clips the bottom of this grade; test unique levels only in its unclipped interval.
    d['layer']['effects']=[{**grade,'contrast_milli':1000,'white_balance_milli':[1000]*3}]
    ramp=check('monotonic_graded_gradient_fractional_fade',dict(size=[4096,1],backdrop=[[0,0,0,0]]*4096,draws=[d]))
    assert all(a[0]<b[0] for a,b in zip(ramp,ramp[1:])); assert len({p[0] for p in ramp})==4096
    for mutate in (lambda r:r['draws'][0].update(offset=[10,10]),lambda r:r['draws'][0].update(anchor=[32769,0]),
                   lambda r:r.update(backdrop=[]),lambda r:r['draws'][0]['layer']['transform'].update(crop=[0,0,99,99]),
                   lambda r:r['draws'][0].update(duration=time(0))):
        bad=request(draw());mutate(bad);call(bad,'INVALID_EFFECT')
    report=dict(passed=['working_layers.integer_and_spatial_geometry','working_layers.associated_masks_and_transparency',
                        'working_layers.animated_effects_and_fractional_opacity','working_layers.ordered_layer_blends',
                        'working_layers.monotonic_graded_gradient','working_layers.bounds_validation'],
                cases=cases,pixel_samples=sum(c['pixels'] for c in cases),maximum_unorm16_error=max(c['maximum_unorm16_error'] for c in cases),
                numerical_response_sha256=digest.hexdigest(),rejected_cases=rejected,gradient_levels=4096,fractional_fade_alpha=5,
                reference='Independent Decimal geometry and Fraction masks/taps; f64 source-over and separate effect equations',
                limits='Library layer frames only; saved working_space, scene/generated/nested/track integration, full rendered gradient/fade gate and stage-six exit remain open')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
