"""Original geometric charts; high-precision forward geometry and rational filtering oracle."""
from engine import ENGINE
import argparse
import copy
from decimal import Decimal as D, getcontext, ROUND_HALF_UP, ROUND_FLOOR
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import subprocess
import shutil
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from animation import sample
from compositing import expected_frame as legacy_frame, rect_at
from graphics import shape_pixels
from grading import channel, resolve, grade
from scenes import identity, selected, time

ROOT=Path(__file__).resolve().parents[1]
getcontext().prec=60
Q=65536


def fraction(value):return F(value['num'],value['den'])
def dec(value):return D(value.numerator)/D(value.denominator) if isinstance(value,F) else D(value)
def atan(z):
    result=D(0);power=z
    for n in range(100):
        result+=power/(2*n+1);power*=-z*z
    return result
PI=16*atan(D(1)/5)-4*atan(D(1)/239)


def trig(mdeg):
    mdeg%=360000
    if mdeg%90000==0:return [(D(0),D(1)),(D(1),D(0)),(D(0),D(-1)),(D(-1),D(0))][mdeg//90000]
    z=D(mdeg)*PI/180000;sin=term=z;cos=cterm=D(1)
    for n in range(1,100):
        term*=-z*z/((2*n)*(2*n+1));sin+=term
        cterm*=-z*z/((2*n-1)*(2*n));cos+=cterm
    return sin,cos


def state(layer,n,anchor):
    tr=layer['transform'];sp=tr['spatial'];clock=F(n,25)-fraction(layer['start'])
    pos=list(tr['position']);opacity=tr['opacity']
    for name,c in layer.get('animation',{}).items():
        if name=='opacity':opacity=sample(c,clock)
        else:pos[0 if name=='position_x' else 1]=sample(c,clock)
    value=[*sp['translate_milli'],*sp['scale_milli'],sp['rotation_mdeg']]
    fields=['translate_x_milli','translate_y_milli','scale_x_milli','scale_y_milli','rotation_mdeg']
    for name,c in sp.get('animation',{}).items():value[fields.index(name)]=sample(c,clock)
    par=fraction(sp['pixel_aspect']);base=[tr['scale']*par,F(tr['scale'])]
    if sp.get('fit'):
        fit=sp['fit'];window=fit.get('window_size') or tr['crop'][2:];ratios=[F(fit['size'][i],window[i])/base[i] for i in range(2)]
        factors=ratios if fit['mode']=='stretch' else [min(ratios) if fit['mode']=='contain' else max(ratios)]*2
        base=[a*b for a,b in zip(base,factors)]
    scale=[dec(base[i]*F(value[i+2],1000)*(-1 if sp['flip'][i] else 1)) for i in range(2)]
    sn,cs=trig(value[4]+tr['quarter_turns']*90000)
    def forward(x,y):
        x,y=D(x),D(y)
        if sp.get('compensation'):
            cp=sp['compensation'];cc=[D(v)/1000 for v in cp['center_milli']];zoom=D(cp['zoom_milli'])/1000
            tx,ty=[D(sample(cp['translation_'+axis+'_milli'],clock))/1000 for axis in ('x','y')]
            sine,cosine=trig(sample(cp['rotation_mdeg'],clock));u=x-cc[0];v=y-cc[1]
            x=cc[0]+zoom*(cosine*u-sine*v+tx);y=cc[1]+zoom*(sine*u+cosine*v+ty)
        u=(x-anchor[0])*scale[0];v=(y-anchor[1])*scale[1]
        return cs*u-sn*v+D(pos[0])+D(value[0])/1000,sn*u+cs*v+D(pos[1])+D(value[1])/1000
    origin=forward(0,0);right=forward(1,0);down=forward(0,1)
    a,c=right[0]-origin[0],right[1]-origin[1];b,d=down[0]-origin[0],down[1]-origin[1]
    det=a*d-b*c;u,v=origin
    inverse=[d/det,-b/det,(b*v-d*u)/det,-c/det,a/det,(c*u-a*v)/det]
    return value,[a,b,u,c,d,v],inverse,opacity


def reference(scene,sources,n,mask_coverage=None):
    w,h=scene['width'],scene['height'];output=[list(scene['background']) for _ in range(w*h)]
    for layer in scene['layers']:
        active=0<=F(n,25)-fraction(layer['start'])<fraction(layer['duration'])
        index=0 if layer.get('graphics') and active else selected(layer,n) if not layer.get('graphics') else None
        if index is None:continue
        if layer.get('graphics'):image=shape_pixels(layer['graphics'],layer['canvas']);anchor=[0,0];offset=[0,0]
        else:
            frame=layer['frames'][index];image=Image.open(sources/frame['image']['path']).convert('RGBA');anchor=frame['anchor'];offset=frame['offset']
        sp=layer['transform']['spatial'];values,forward,inverse,opacity=state(layer,n,anchor)
        cx,cy,cw,ch=layer['transform']['crop'];mask=rect_at(layer,n);inv_mask=layer.get('mask',{}).get('inverted',False)
        effects=[resolve(e,F(n,25)-fraction(layer['start'])) for e in layer.get('effects',[])];chain=json.dumps(effects,sort_keys=True)
        premult=layer.get('alpha_mode','straight')=='premultiplied';mode=layer.get('blend_mode','normal')
        cache={}
        def tap(x,y):
            if sp['edge']=='clamp':x=min(cx+cw-1,max(cx,x));y=min(cy+ch-1,max(cy,y))
            if not cx<=x<cx+cw or not cy<=y<cy+ch:return (0,0,0,0)
            coverage=F(1)
            if mask:
                if mask_coverage is not None:coverage=mask_coverage(layer['mask'],mask,x,y)
                else:
                    mx,my,mw,mh=mask
                    coverage=F(int((mx<=x<mx+mw and my<=y<my+mh)!=inv_mask))
                if not coverage:return (0,0,0,0)
            xx,yy=x-offset[0],y-offset[1]
            if not 0<=xx<image.width or not 0<=yy<image.height:return (0,0,0,0)
            if (xx,yy) in cache:return cache[xx,yy]
            p=image.getpixel((xx,yy));alpha=p[3];colors=list(p[:3])
            if effects and alpha:colors=[channel(colors[c],alpha if premult else 255,c,chain) for c in range(3)]
            result=tuple(coverage*(c*alpha if effects or not premult else c*255) for c in colors)+(coverage*alpha,)
            cache[xx,yy]=result;return result
        a,b,c,d,e,f=inverse
        uncompensated=None
        if sp.get('compensation'):
            original=copy.deepcopy(layer);del original['transform']['spatial']['compensation'];uncompensated=state(original,n,anchor)[2]
        for y in range(h):
            for x in range(w):
                if uncompensated:
                    aa,bb,cc,dd,ee,ff=uncompensated
                    px,py=D(x)+D('.5'),D(y)+D('.5')
                    u=int(((aa*px+bb*py+cc)*Q).to_integral_value(rounding=ROUND_HALF_UP));v=int(((dd*px+ee*py+ff)*Q).to_integral_value(rounding=ROUND_HALF_UP))
                    vx,vy,vw,vh=sp['compensation']['viewport']
                    if not vx*Q<=u<(vx+vw)*Q or not vy*Q<=v<(vy+vh)*Q:continue
                if sp.get('viewport'):
                    vx,vy,vw,vh=sp['viewport']
                    if not vx<=x<vx+vw or not vy<=y<vy+vh:continue
                u=(a*(D(x)+D('.5'))+b*(D(y)+D('.5'))+c)*Q
                v=(d*(D(x)+D('.5'))+e*(D(y)+D('.5'))+f)*Q
                u=int(u.to_integral_value(rounding=ROUND_HALF_UP));v=int(v.to_integral_value(rounding=ROUND_HALF_UP))
                if sp['edge']=='clamp' and not (cx*Q<=u<(cx+cw)*Q and cy*Q<=v<(cy+ch)*Q):continue
                if sp['sampling']=='nearest':weights=[(u//Q,v//Q,F(1))]
                else:
                    px,py=F(u,Q)-F(1,2),F(v,Q)-F(1,2);xx=px.numerator//px.denominator;yy=py.numerator//py.denominator
                    fx,fy=px-xx,py-yy;weights=[(xx,yy,(1-fx)*(1-fy)),(xx+1,yy,fx*(1-fy)),(xx,yy+1,(1-fx)*fy),(xx+1,yy+1,fx*fy)]
                filtered=[F(0)]*4
                for xx,yy,weight in weights:
                    if weight:
                        p=tap(xx,yy)
                        filtered=[old+weight*v for old,v in zip(filtered,p)]
                if filtered[3]==0:continue
                coverage=filtered[3]*opacity/(255*255);old=output[y*w+x]
                for i in range(3):
                    color=filtered[i]/(filtered[3]*255);backdrop=F(old[i],255)
                    blend=color if mode=='normal' else color*backdrop if mode=='multiply' else 1-(1-color)*(1-backdrop)
                    old[i]=int(255*(coverage*blend+(1-coverage)*backdrop)+F(1,2))
    data=bytes(c for p in output for c in p);image=Image.frombytes('RGB',(w,h),data)
    return image.resize((w*scene['output_scale'],h*scene['output_scale']),Image.Resampling.NEAREST).tobytes()


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store=[root/n for n in ('sources','output','store')]
    for p in (sources,out,store):p.mkdir()
    for k in range(2):
        image=Image.new('RGBA',(12,8));image.putdata([(((x*51+y*30+k*9)%256)//3*3,((y*57+x*24+k*30)%256)//3*3,((x*9+y*45+k*72)%256)//3*3,[0,85,170,255][(x+y+k)%4]) for y in range(8) for x in range(12)])
        image.save(sources/f'chart-{k}.png');image.putdata([tuple(c*p[3]//255 for c in p[:3])+(p[3],) for p in image.getdata()]);image.save(sources/f'premult-{k}.png')
    original={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;passed=[];cases=[];rejected=0;frames=0;samples=0;previews=0
    def call(req,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(req).encode(),capture_output=True,timeout=180,env=env);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        if p.returncode:(root/'failed-request.json').write_text(json.dumps(req,indent=2),encoding='utf-8')
        assert p.returncode==0 and r['ok'],r;return r['result']
    def inspect(scene,error=None):return call({'command':'scene.inspect','scene':scene,'input_root':str(sources)},error)
    def render(scene,name,error=None):return call({'command':'scene.render','scene':scene,'input_root':str(sources),'output_root':str(out),'output':str(out/(name+'.mkv'))},error)
    def decode(path,kind):return subprocess.check_output(['ffmpeg','-v','error','-i',str(path),*(['-an','-pix_fmt','rgb24','-f','rawvideo'] if kind=='video' else ['-vn','-f','s16le']),'-'],timeout=120)
    def spatial(**changes):
        value={'translate_milli':[0,0],'scale_milli':[1000,1000],'rotation_mdeg':0,'flip':[False,False],'pixel_aspect':time(1),'sampling':'nearest','edge':'transparent'};value.update(changes);return value
    def base(spec=None):
        layer={'id':'chart','canvas':[18,12],'start':time(2,25),'duration':time(12,25),'frames':[{'image':identity(sources/f'chart-{i}.png',sources),'hold':time(3,25),'offset':[3,2],'anchor':[9,6]} for i in range(2)],'timing':'strict','end':'loop','transform':{'position':[20,15],'crop':[1,1,16,10],'scale':1,'quarter_turns':0,'opacity':173,'spatial':spatial() if spec is None else spec}}
        return {'schema_version':1,'id':'spatial','width':40,'height':30,'output_scale':1,'duration':time(16,25),'background':[23,57,101],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    def check(scene,name,tolerance=0):
        nonlocal frames,samples
        scene=copy.deepcopy(scene);scene['id']=name;info=inspect(scene);receipt=render(scene,name);count=int(fraction(scene['duration'])*25);cache={};expected=[]
        animated=any(l.get('animation') or l['transform']['spatial'].get('animation') or l.get('mask',{}).get('animation') or any(e.get('animation') for e in l.get('effects',[])) for l in scene['layers'])
        for n in range(count):
            key=n if animated else tuple((0 if 0<=F(n,25)-fraction(l['start'])<fraction(l['duration']) else None) if l.get('graphics') else selected(l,n) for l in scene['layers'])
            if key not in cache:cache[key]=reference(scene,sources,n)
            expected.append(cache[key])
            for layer,item in zip(scene['layers'],info['timing']):
                actual=item['sampled_parameters'][n]
                if actual is None:continue
                anchor=[0,0] if layer.get('graphics') else layer['frames'][selected(layer,n)]['anchor'];v,forward,inverse,_=state(layer,n,anchor)
                assert actual['spatial']['translate_milli']==v[:2] and actual['spatial']['scale_milli']==v[2:4] and actual['spatial']['rotation_mdeg']==v[4]
                for name_,matrix in [('source_to_scene',forward),('scene_to_source',inverse)]:assert max(abs(float(a)-b) for a,b in zip(matrix,actual['spatial'][name_]))<1e-8,(name,name_,matrix,actual)
        rgb=decode(out/(name+'.mkv'),'video');pcm=decode(out/(name+'.mkv'),'audio');wanted=b''.join(expected)
        assert len(rgb)==len(wanted);maximum=max(abs(a-b) for a,b in zip(rgb,wanted));assert maximum<=tolerance,(name,maximum,next(((i,a,b) for i,(a,b) in enumerate(zip(rgb,wanted)) if abs(a-b)>tolerance),None))
        assert pcm==bytes(count*1920*4);frames+=count;samples+=count*1920
        assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
        cases.append({'name':name,'frames':count,'maximum_channel_error':maximum,'allowed_error':tolerance});print(json.dumps(cases[-1]),flush=True)
        return scene,info,receipt,rgb
    # Exact geometric landmarks: the new identity path equals the existing integer compositor.
    for q in range(4):
        scene=base();scene['layers'][0]['transform'].update(quarter_turns=q,scale=q%3+1)
        result=check(scene,'legacy-quadrant-'+str(q));old=copy.deepcopy(scene);old['layers'][0]['transform'].pop('spatial');render(old,'legacy-reference-'+str(q))
        raw=decode(out/f'legacy-reference-{q}.mkv','video');assert raw==result[3]==b''.join(legacy_frame(old,sources,n) for n in range(16));assert decode(out/f'legacy-reference-{q}.mkv','audio')==bytes(16*1920*4);frames+=16;samples+=16*1920
    passed.append('spatial.legacy_exact_quadrant_regression')
    for par,label in [(time(2),'wide'),(time(1,2),'tall'),(time(16,15),'fractional')]:
        for mode in ('contain','cover','stretch'):
            check(base(spatial(pixel_aspect=par,fit={'size':[30,20],'mode':mode},viewport=[5,5,30,20])),'aspect-'+label+'-'+mode)
    for mode in ('contain','cover','stretch'):
        check(base(spatial(pixel_aspect=time(2),fit={'size':[30,20],'mode':mode,'window_size':[11,7]},viewport=[5,5,30,20],sampling='bilinear')),'explicit-fit-window-'+mode,1)
    passed.append('spatial.aspect_fit_crop_anchor')
    for method in ('nearest','bilinear'):
        for edge in ('transparent','clamp'):
            check(base(spatial(translate_milli=[250,-500],scale_milli=[1750,625],rotation_mdeg=33000,flip=[True,False],sampling=method,edge=edge)),'sample-'+method+'-'+edge,1 if method=='bilinear' else 0)
    mirror=base(spatial(translate_milli=[-1125,875],scale_milli=[900,2200],rotation_mdeg=127000,flip=[False,True],sampling='bilinear',viewport=[-4,5,30,20]));mirror['layers'][0]['transform']['quarter_turns']=3;check(mirror,'vertical-mirror-composed-rotation',1)
    for blend in ('normal','multiply','screen'):
        scene=base(spatial(translate_milli=[500,500],scale_milli=[2250,1750],sampling='bilinear'));scene['layers'][0]['blend_mode']=blend
        straight=check(scene,'alpha-straight-'+blend)
        pre=copy.deepcopy(scene);pre['layers'][0]['alpha_mode']='premultiplied'
        for i,f in enumerate(pre['layers'][0]['frames']):f['image']=identity(sources/f'premult-{i}.png',sources)
        premult=check(pre,'alpha-premult-'+blend);assert straight[3]==premult[3]
    passed.append('spatial.interpolation_alpha_edges_blends')
    def curve(entries,**extra):return {'keys':[{'time':time(n,25),'value':v,'interpolation':mode} for n,v,mode in entries],**extra}
    animated=base(spatial(sampling='bilinear',animation={'translate_x_milli':curve([(0,-12000,'ease_in'),(10,9000,'hold')]),'translate_y_milli':curve([(2,250,'hold'),(8,3250,'linear'),(12,-1250,'hold')]),'scale_x_milli':curve([(0,500,'ease_out'),(12,2250,'hold')]),'scale_y_milli':curve([(0,750,'ease_in_out'),(12,1750,'hold')],retime={'start':time(2,25),'rate':time(2),'reverse':True}),'rotation_mdeg':curve([(0,-35000,'linear'),(12,95000,'hold')])}))
    animated['layers'][0]['animation']={'position_y':curve([(0,12,'linear'),(12,16,'hold')]),'opacity':curve([(0,255,'linear'),(12,80,'hold')])}
    animated['layers'][0]['duration']=time(14,25)
    animate=check(animated,'animated-spatial',1);reordered=copy.deepcopy(animated)
    for c in reordered['layers'][0]['transform']['spatial']['animation'].values():c['keys'].reverse()
    repeated=check(reordered,'reordered-spatial',1);assert animate[1]['timing'][0]['sampled_parameters']==repeated[1]['timing'][0]['sampled_parameters'] and animate[3]==repeated[3]
    masked=copy.deepcopy(animated);masked['layers'][0]['mask']={'rect':[2,1,12,10],'inverted':True,'animation':{'width':curve([(0,0,'linear'),(12,12,'hold')])}};check(masked,'animated-mask-before-filter',1)
    anchored=copy.deepcopy(animated);anchored['layers'][0]['frames'][1].update(anchor=[7,4],offset=[1,3]);check(anchored,'changing-trim-anchor',1)
    passed.append('spatial.animated_property_clocks')
    graded=base(spatial(translate_milli=[-375,750],scale_milli=[2400,1700],rotation_mdeg=-17000,sampling='bilinear'));graded['layers'][0]['effects']=[{**grade(),'exposure_milli':700,'white_balance_milli':[1200,950,800]}];check(graded,'grade-before-filter',1)
    shape=base(spatial(translate_milli=[500,-500],scale_milli=[1800,1500],rotation_mdeg=29000,sampling='bilinear'));l=shape['layers'][0];l.update(frames=[],graphics={'kind':'shape','shape':'ellipse','rect':[2,1,12,8],'fill':[201,72,33,190]},timing='strict',end='hold_last');l['transform']['position']=[12,3];check(shape,'shape-spatial',1)
    layered=copy.deepcopy(graded);upper=copy.deepcopy(shape['layers'][0]);upper['id']='upper';upper['blend_mode']='screen';layered['layers'].append(upper);layered['output_scale']=2;check(layered,'layered-output-scale',1)
    passed.append('spatial.graphics_mask_effect_order')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_scene_inspect');fields={'scene':animate[0],'input_root':str(sources)};Draft202012Validator(schema).validate(fields);assert client.call('scene.inspect',**fields)==animate[1]
        copied=client.call('graphics.instantiate',template={'schema_version':1,'id':'spatial-template','scene':animate[0],'parameters':[]},values={},instance_id='spatial-copy',input_root=str(sources))
        assert copied['scene']['layers'][0]['transform']['spatial']==animate[0]['layers'][0]['transform']['spatial']
        project=call({'command':'project.create','id':'spatial-edit','width':40,'height':30,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create');common={'store_root':str(store),'project_id':project['id']}
        asset=copy.deepcopy(animate[2]['asset']);asset['identity']={'sha256':animate[2]['sha256'],'bytes':(out/'animated-spatial.mkv').stat().st_size}
        fields={**common,'expected_revision':0,'request_id':'add','operations':[{'op':'media.add','asset':asset},{'op':'clip.append','clip':{'id':'moving','asset_id':asset['id'],'source_in':time(0),'duration':time(16,25)}}]}
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt;saved=client.call('session.get',**common)
        path=out/'saved.mkv';call({'command':'render.run','project':saved,'input_root':str(out),'output_root':str(out),'output':str(path)});assert decode(path,'video')==animate[3] and decode(path,'audio')==bytes(16*1920*4);frames+=16;samples+=16*1920
        for n in (0,2,4,10,14,15):
            path=out/f'preview-{n}.png';client.call('preview.frame',project=saved,input_root=str(out),output_root=str(out),output=str(path),time=time(n,25))
            with Image.open(path) as image:assert image.tobytes()==animate[3][n*40*30*3:(n+1)*40*30*3]
            previews+=1
        path=out/'range.mkv';call({'command':'preview.range','project':saved,'input_root':str(out),'output_root':str(out),'output':str(path),'start':time(3,25),'duration':time(10,25)});assert decode(path,'video')==animate[3][3*40*30*3:13*40*30*3] and decode(path,'audio')==bytes(10*1920*4);frames+=10;samples+=10*1920
        proxied=call({'command':'proxy.generate','project':saved,'expected_revision':saved['revision'],'asset_id':asset['id'],'scale':2,'input_root':str(out),'output_root':str(out),'output':str(out/'proxy.mkv')})['project']
        proxied=call({'command':'timeline.apply','project':proxied,'expected_revision':proxied['revision'],'operations':[{'op':'preview.proxy','scale':2}]});path=out/'proxy-preview.png';client.call('preview.frame',project=proxied,input_root=str(out),output_root=str(out),output=str(path),time=time(5,25))
        expected=animate[3][5*40*30*3:6*40*30*3];small=bytes(expected[(y*2*40+x*2)*3+c] for y in range(15) for x in range(20) for c in range(3))
        with Image.open(path) as image:assert image.size==(20,15) and image.tobytes()==small
        previews+=1
        client.call('session.apply',**common,expected_revision=1,request_id='trim',operations=[{'op':'clip.trim','clip_id':'moving','source_in':time(2,25),'duration':time(12,25)}]);client.call('session.undo',**common,expected_revision=2,request_id='undo');assert client.call('session.get',**common)['clips']==saved['clips']
    finally:client.close()
    passed.append('spatial.mcp_templates_saved_previews')
    # Large declared coordinates cannot allocate a destination-sized intermediate or overflow sampling.
    extreme=base(spatial(translate_milli=[32768000,-32768000],scale_milli=[1,16000],rotation_mdeg=-3600000,pixel_aspect=time(16),fit={'size':[1,32768],'mode':'contain'},viewport=[-32768,32768,32768,1],sampling='bilinear'))
    extreme_info=inspect(extreme);assert extreme_info['frames']==16
    render(extreme,'extreme-bounded');assert decode(out/'extreme-bounded.mkv','video')==bytes(extreme['background'])*(40*30*16);assert decode(out/'extreme-bounded.mkv','audio')==bytes(16*1920*4);frames+=16;samples+=16*1920
    invalid=[]
    for size in ([0,10],[10,0],[32769,1],[1,4294967295]):
        invalid.append((base(spatial(fit={'size':[30,20],'mode':'cover','window_size':size})),'INVALID_SPATIAL'))
    for change,code in [({'scale_milli':[0,1000]},'INVALID_SPATIAL'),({'scale_milli':[16001,1000]},'INVALID_SPATIAL'),({'translate_milli':[32768001,0]},'INVALID_SPATIAL'),({'rotation_mdeg':3600001},'INVALID_SPATIAL'),({'pixel_aspect':time(0)},'INVALID_SPATIAL'),({'pixel_aspect':time(17)},'INVALID_SPATIAL'),({'pixel_aspect':time(1,17)},'INVALID_SPATIAL'),({'pixel_aspect':{'num':1,'den':0}},'INVALID_TIME'),({'pixel_aspect':time(1000000,1000001)},'INVALID_SPATIAL'),({'fit':{'size':[0,10],'mode':'contain'}},'INVALID_SPATIAL'),({'viewport':[0,0,0,10]},'INVALID_SPATIAL'),({'viewport':[-32769,0,10,10]},'INVALID_SPATIAL'),({'sampling':'area'},'INVALID_JSON'),({'edge':'wrap'},'INVALID_JSON'),({'animation':{}},'INVALID_SPATIAL'),({'unknown':True},'INVALID_JSON')]:invalid.append((base(spatial(**change)),code))
    for field,value in [('translate_x_milli',32768001),('translate_y_milli',-32768001),('scale_x_milli',0),('scale_y_milli',16001),('rotation_mdeg',-3600001)]:invalid.append((base(spatial(animation={field:curve([(0,value,'hold')])})),'INVALID_ANIMATION'))
    for curve_,code in [({'keys':[]},'INVALID_ANIMATION'),(curve([(0,1000,'linear'),(0,2000,'hold')]),'INVALID_ANIMATION'),(curve([(13,1000,'hold')]),'INVALID_ANIMATION'),(curve([(0,1000,'hold')],retime={'start':time(0),'rate':time(0),'reverse':False}),'INVALID_ANIMATION'),({'keys':[{'time':{'num':1,'den':0},'value':1000,'interpolation':'hold'}]},'INVALID_TIME')]:invalid.append((base(spatial(animation={'scale_x_milli':curve_})),code))
    invisible=base(spatial(scale_milli=[0,1000]));invisible['layers'][0]['transform']['opacity']=0;invalid.append((invisible,'INVALID_SPATIAL'))
    bad=base();bad['layers'][0]['frames'][0]['image']['sha256']='0'*64;invalid.append((bad,'MEDIA_CHANGED'))
    bad=base(spatial(sampling='bilinear'));bad['layers'][0]['alpha_mode']='premultiplied';invalid.append((bad,'INVALID_ALPHA'))
    snapshots={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    for i,(scene,code) in enumerate(invalid):render(scene,'rejected-'+str(i),code)
    render(animate[0],'animated-spatial','OUTPUT_EXISTS')
    assert snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=sources/'chart-0.png';saved_bytes=source.read_bytes()
    try:call({'command':'scene.render','scene':animate[0],'input_root':str(sources),'output_root':str(out),'output':str(out/'changed.mkv')},'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved_bytes)
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert snapshots=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()};assert not list(out.glob('.cutbolt-scene-*'))
    passed.append('spatial.validation_source_preservation')
    report={'passed':passed,'cases':cases,'frames_compared':frames,'stereo_sample_frames_compared':samples,'rejected_cases':rejected,'frame_previews':previews,'reference':'60-digit Decimal forward geometry from independently transformed basis points, analytic inverse and rational premultiplied interpolation/compositing. Exact quadrants, aspect fitting and alpha equivalence; at most one RGB unit for arbitrary rotations/graded colors. Existing pixel path remains exactly identical. All silence samples verified.'}
    (root/'scene.json').write_text(json.dumps(animated,indent=2)+'\n',encoding='utf-8');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output)
