"""Independent rational key mattes, Decimal color recovery and original edge quality fixtures."""
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
from grading import D, changed, curve, encoded, linear, neutral
from graphics import shape_pixels
from scenes import audio_bytes, identity, selected, time
from selection import corrected, mask_weight, qualification, qround, resolve_chain as resolve_grades

ROOT=Path(__file__).resolve().parents[1]

def decimal(value):
    value=F(value);return D(value.numerator)/value.denominator

def encode_unit(v):
    return D('12.92')*v if v<=D('0.0031308') else D('1.055')*v**(D(5)/12)-D('0.055')

def matte(rgb,e):
    # Three explicit opponent contrasts rather than the engine's extrema shortcut.
    contrasts=[abs((rgb[a]-rgb[b])-(e['key_rgb'][a]-e['key_rgb'][b])) for a,b in ((0,1),(1,2),(2,0))]
    d=F(max(contrasts)*1000,510)
    if d<=e['inner_milli']:return F(0)
    if d>=e['outer_milli']:return F(1)
    return (d-e['inner_milli'])/(e['outer_milli']-e['inner_milli'])

def resolve_chain(effects,clock):
    result=[]
    for e in copy.deepcopy(effects):
        if e['kind']!='chroma_key':result+=resolve_grades([e],clock);continue
        if e.get('strength_curve'):e['strength_milli']=sample(e.pop('strength_curve'),clock)
        if e.get('mask'):
            for name,c in e['mask'].pop('animation',{}).items():e['mask']['rect'][('x','y','width','height').index(name)]=sample(c,clock)
        result.append(e)
    return result

@lru_cache(maxsize=131072)
def pixel_reference(pixel,alpha,position,chain_json):
    denominator=pixel[3] if alpha=='premultiplied' else 255
    if not pixel[3]:return (F(0),)*3,0
    color=tuple(linear(v,denominator) for v in pixel[:3]);coverage=F(pixel[3]);touched=False
    for e in json.loads(chain_json):
        if e['kind']=='chroma_key':
            w=F(e['strength_milli'],1000)*mask_weight(position,e.get('mask'))
            if not w:continue
            m=matte(tuple(encoded(v) for v in color),e);candidate=list(map(encode_unit,color));spill_changed=False
            if m<1 and e.get('unmix_milli',0):
                u=D(e['unmix_milli'])/1000
                for c,v in enumerate(candidate):
                    recovered=D(0) if not m else min(D(1),max(D(0),(v-(1-decimal(m))*D(e['key_rgb'][c])/255)/decimal(m)))
                    candidate[c]=v*(1-u)+recovered*u
            if e.get('spill'):
                s=e['spill'];c=('red','green','blue').index(s['channel'])
                target=max(v for i,v in enumerate(candidate) if i!=c)
                excess=max(D(0),candidate[c]-target);spill_changed=bool(excess and s['strength_milli'])
                candidate[c]-=excess*D(s['strength_milli'])/1000
            if m==1 and not spill_changed:continue
            color=tuple(a*(1-decimal(w))+linear(b,1)*decimal(w) for a,b in zip(color,candidate))
            coverage*=1-w*(1-m);touched=True
            if not coverage:return (F(0),)*3,0
        else:
            g=e if e['kind']=='grade' else e['grade']
            if neutral([g]):continue
            w=F(1) if e['kind']=='grade' else F(e['mix_milli'],1000)*qualification(tuple(encoded(v) for v in color),e.get('qualifier'))*mask_weight(position,e.get('mask'))
            if not w:continue
            graded=corrected(color,json.dumps(g,sort_keys=True))
            color=tuple(a*(1-decimal(w))+b*decimal(w) for a,b in zip(color,graded));touched=True
    return (tuple(F(encoded(v),255) for v in color),qround(coverage)) if touched else (tuple(F(v,denominator) for v in pixel[:3]),pixel[3])

def key(**extra):
    result={'kind':'chroma_key','key_rgb':[0,255,0],'inner_milli':60,'outer_milli':180,'strength_milli':1000,'unmix_milli':0,**extra}
    if result.get('mask'):result['mask'].setdefault('inverted',False)
    return result

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/n for n in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    palette=[(0,255,0),(0,0,255),(255,0,0),(128,128,128),(0,0,0),(255,255,255),(0,204,0),(0,203,0),(0,164,0),(0,163,0),
             (20,140,60),(40,160,80),(100,160,120),(100,120,180),(180,100,120),(255,0,255),(0,255,255),(255,255,0),(50,203,50),
             (1,255,1),(254,255,254),(20,90,20),(50,50,50),(200,200,200)]
    chart=Image.new('RGBA',(48,16))
    for y in range(16):
        for x in range(48):
            color=palette[x%24]
            if 1<=y<8:color=tuple((v*(8-y)+128*y+4)//8 for v in color)
            chart.putpixel((x,y),(*color,255 if y<8 else [0,1,2,17,64,128,254,255][y-8]))
    chart.save(sources/'chart.png')
    for name,func in [('premultiplied',lambda p:(*((v*p[3]+127)//255 for v in p[:3]),p[3])),('alternate',lambda p:(p[1],p[2],p[0],p[3]))]:
        image=Image.new('RGBA',chart.size);image.putdata([func(p) for p in chart.getdata()]);image.save(sources/(name+'.png'))
    # Known original gray/white foreground, with single-pixel strands and every third alpha level.
    truth=Image.new('RGBA',(48,16));plate=Image.new('RGBA',truth.size)
    for y in range(16):
        for x in range(48):
            a=[0,3,6,15,30,51,75,99,126,153,180,204,225,240,252,255][x%16]
            if y%4==0:a=255 if x%2 else 0
            f=[85,170,255][y%3];v=f*a//255
            truth.putpixel((x,y),(f,f,f,a));plate.putpixel((x,y),(v,v+255-a,v,255))
    truth.save(sources/'edge-truth.png');plate.save(sources/'edge-plate.png')
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ROOT/'target/debug/cutbolt.exe';decoded=0;maximum_error=0;passed=[];cases=[];quality={}
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
        layer={'id':'keyed','canvas':[64,32],'start':time(0),'duration':time(frames,25),'frames':[{'image':identity(sources/path,sources),'hold':time(frames,25),'offset':[8,8],'anchor':[0,0]}],
               'timing':'strict','end':'hold_last','transform':{'position':[0,0],'crop':[0,0,64,32],'scale':1,'quarter_turns':0,'opacity':255},'effects':effects}
        return {'schema_version':1,'id':'keying','width':64,'height':32,'output_scale':2,'duration':time(frames,25),'background':[23,57,101],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
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
                    color,alpha=pixel_reference(p,layer.get('alpha_mode','straight'),(x,y),chain)
                    px,py=x-cx,y-cy;ax,ay=frame['anchor'][0]-cx,frame['anchor'][1]-cy;w,h=cw,ch
                    for _ in range(tr['quarter_turns']):px,py,ax,ay,w,h=h-1-py,px,h-ay,ax,h,w
                    left=tr['position'][0]+(px-ax)*scale;top=tr['position'][1]+(py-ay)*scale
                    coverage=F(alpha*tr['opacity'],255*255);mode=layer.get('blend_mode','normal')
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
            for n,parameters in enumerate(report['sampled_parameters']):
                if parameters is None:continue
                clock=F(n,25)-F(layer['start']['num'],layer['start']['den']);expected=[]
                for e in resolve_chain(layer['effects'],clock):
                    if e['kind']=='chroma_key':r={'kind':e['kind'],'strength_milli':e['strength_milli'],'mask_rect':e.get('mask',{}).get('rect')}
                    else:
                        g=e if e['kind']=='grade' else e['grade'];r={k:g[k] for k in ('exposure_milli','contrast_milli','white_balance_milli')};r['kind']=e['kind']
                        if e['kind']=='selective_grade':r.update(mix_milli=e['mix_milli'],mask_rect=e.get('mask',{}).get('rect'))
                    expected.append(r)
                assert parameters.get('effects',[])==expected
        frames=int(F(scene['duration']['num'],scene['duration']['den'])*25)
        wanted=lru_cache(None)(lambda n:reference(scene,n));raw=verify_bytes(output/(name+'.mkv'),scene,wanted,frames);cases.append(name)
        return scene,info,receipt,wanted,raw
    def point(case,x,y):
        s=case[0];scale=s['output_scale'];start=(y*scale*s['width']*scale+x*scale)*3
        return tuple(case[-1][start:start+3])

    assert 'chroma_key' in request({'command':'capabilities'})['effects']['types']
    baseline=check(base([]),'bypass')
    soft=check(base([key()]),'green-soft')
    assert point(soft,8,8)==(23,57,101) and point(soft,9,8)==palette[1] and point(soft,11,8)==palette[3]
    hard=check(base([key(inner_milli=100,outer_milli=100)]),'green-hard-boundary')
    assert point(hard,14,8)==(23,57,101) and point(hard,15,8)==palette[7]
    blue=check(base([key(key_rgb=[0,0,255])]),'blue-soft')
    assert point(blue,9,8)==(23,57,101) and point(blue,8,8)==palette[0]
    check(base([key(key_rgb=[20,140,60],inner_milli=0,outer_milli=0)]),'custom-gray-offset')
    check(base([key(outer_milli=1000)]),'maximum-distance-range')
    check(base([key(strength_milli=350)]),'partial-key')
    passed.append('keying.chroma_geometry_boundaries')

    no_unmix=check(base([key(inner_milli=0,outer_milli=500)],'edge-plate.png'),'contaminated-edges')
    recovered=check(base([key(inner_milli=0,outer_milli=500,unmix_milli=1000)],'edge-plate.png'),'recovered-edges')
    original=check(base([],'edge-truth.png'),'original-edges')
    edge_error=max(abs(a-b) for a,b in zip(recovered[-1],original[-1]));assert edge_error<=1
    before_error=max(abs(a-b) for a,b in zip(no_unmix[-1],original[-1]));assert before_error>=30
    quality.update(known_neutral_edge_max_error=edge_error,without_unmix_max_error=before_error)
    for channel,index in [('red',14),('green',12),('blue',13)]:
        s=check(base([key(inner_milli=0,outer_milli=0,spill={'channel':channel,'strength_milli':1000})]),'spill-'+channel)
        expected=list(palette[index]);c=('red','green','blue').index(channel);expected[c]=max(v for i,v in enumerate(expected) if i!=c)
        assert point(s,8+index,8)==tuple(expected)
    check(base([key(unmix_milli=400,spill={'channel':'green','strength_milli':600},strength_milli=650)]),'partial-unmix-and-spill')
    check(base([key(inner_milli=0,outer_milli=1000,unmix_milli=1000)]),'unmix-clipping')
    passed.append('keying.spill_and_known_edge_quality')

    for mode in ('normal','multiply','screen'):
        s=base([key(strength_milli=650,spill={'channel':'green','strength_milli':700})],'premultiplied.png')
        s['layers'][0].update(alpha_mode='premultiplied',blend_mode=mode);s['layers'][0]['transform']['opacity']=173
        check(s,'premult-'+mode)
    p=base([],'premultiplied.png');p['layers'][0]['alpha_mode']='premultiplied';b=check(p,'premult-bypass')
    for name,e in [('disabled',key(strength_milli=0,spill={'channel':'green','strength_milli':1000})),('outside',key(mask={'rect':[1000,1000,1,1],'feather':0}))]:
        p['layers'][0]['effects']=[e];assert check(p,'premult-'+name)[-1]==b[-1]
    s=base([key(strength_milli=100)]*8);chain=check(s,'eight-alpha-stages')
    assert point(chain,8,19)==tuple(qround(F(7*color+(255-7)*bg,255)) for color,bg in zip(palette[0],[23,57,101]))
    passed.append('keying.alpha_precision_and_bypass')

    for name,rect,feather,inverted in [('feather',[10,7,30,17],4,False),('inverted',[10,7,30,17],4,True),('thin',[8,8,1,16],1,False),('empty',[8,8,0,16],4,False),('empty-inverse',[8,8,0,16],4,True)]:
        s=check(base([key(mask={'rect':rect,'feather':feather,'inverted':inverted})]),'mask-'+name)
        if name=='empty':assert s[-1]==baseline[-1]
    spatial=base([key(mask={'rect':[9,7,31,18],'feather':3},spill={'channel':'green','strength_milli':500})]);l=spatial['layers'][0]
    l['frames'][0]['anchor']=[16,12];l['transform'].update(crop=[2,4,58,24],position=[32,16],opacity=173)
    l['mask']={'rect':[9,7,40,16],'inverted':False}
    for turn in range(4):l['transform']['quarter_turns']=turn;check(spatial,'rotated-source-mask-'+str(turn))
    passed.append('keying.masks_and_transforms')

    invert=changed(master_curve={'points':[[0,65535],[65535,0]]})
    before=check(base([invert,key()]),'grade-then-key');after=check(base([key(),invert]),'key-then-grade')
    assert point(before,8,8)==(255,0,255) and point(after,8,8)==(23,57,101)
    from selection import selective, hue
    check(base([key(strength_milli=550),selective(hue(120000,30000,60000),grade=changed(exposure_milli=400)),key(key_rgb=[0,0,255],strength_milli=750)]),'mixed-three-effect-types')
    two=base([key()]);layer=copy.deepcopy(two['layers'][0]);layer['id']='blue-copy';layer['effects']=[key(key_rgb=[0,0,255])]
    layer['transform'].update(position=[4,2],opacity=128);two['layers'].append(layer);check(two,'independent-layer-keys')
    passed.append('keying.effect_order_and_independence')

    moving=base([key(mask={'rect':[8,8,0,16],'feather':3},spill={'channel':'green','strength_milli':600})],frames=25)
    l=moving['layers'][0];l.update(start=time(2,25),duration=time(20,25),timing='sample_start',end='loop')
    l['frames']=[{'image':identity(sources/name,sources),'hold':time(3 if i==0 else 7,100),'offset':[8,8],'anchor':[0,0]} for i,name in enumerate(('chart.png','alternate.png'))]
    e=l['effects'][0];e['strength_curve']=curve([(0,1,1000,'ease_in_out'),(18,25,0,'hold')],{'start':time(1,25),'rate':time(3,2),'reverse':True})
    e['mask']['animation']={'width':curve([(0,1,0,'linear'),(18,25,40,'hold')]),'x':curve([(0,1,8,'hold'),(1,5,12,'linear'),(18,25,6,'hold')])}
    m=check(moving,'animated-key');assert m[3](0)==m[3](24) and m[3](4)!=m[3](16)
    reordered=copy.deepcopy(m[0]);e=reordered['layers'][0]['effects'][0]
    for c in [e['strength_curve'],*e['mask']['animation'].values()]:c['keys'].reverse()
    assert inspect(reordered)['timing'][0]['sampled_parameters']==m[1]['timing'][0]['sampled_parameters']
    assert check(json.loads(json.dumps(reordered)),'reloaded-key')[-1]==m[-1]
    shape=base([key(key_rgb=[0,180,0],inner_milli=0,outer_milli=0)]);l=shape['layers'][0];l['frames']=[]
    l['graphics']={'kind':'shape','shape':'ellipse','rect':[3,2,54,28],'fill':[0,180,0,150],'stroke':{'color':[200,30,40,255],'width':2}}
    check(shape,'shape-key')
    passed.append('keying.animation_and_graphics')

    client=Client(exe)
    try:
        client.initialize();listing=client.rpc('tools/list')['result']['tools'];tool=next(t for t in listing if t['name']=='cutbolt_effects_preset')
        assert tool['annotations']['readOnlyHint'] and tool['annotations']['idempotentHint']
        for name in ('green_soft','blue_soft','green_hard','blue_hard'):
            args={'name':name,'strength_milli':1000,'spill_milli':650};Draft202012Validator(tool['inputSchema']).validate(args)
            recipe=client.call('effects.preset',**args);assert recipe==request({'command':'effects.preset',**args})
            assert recipe['schema_version']==1 and recipe['preset']==name
            assert recipe['effects'][0]['key_rgb']==([0,0,255] if name.startswith('blue') else [0,255,0])
            assert recipe['effects'][0]['inner_milli']==(100 if name.endswith('hard') else 60)
            assert recipe['effects'][0]['outer_milli']==(100 if name.endswith('hard') else 180)
            check(base(recipe['effects']),'preset-'+name)
            recipe['effects'][0]['key_rgb'][0]=99
            assert client.call('effects.preset',**args)['effects'][0]['key_rgb'][0]==0
        schema=next(t for t in listing if t['name']=='cutbolt_scene_inspect')['inputSchema']
        Draft202012Validator(schema).validate({'scene':m[0],'input_root':str(sources)})
        assert client.call('scene.inspect',scene=m[0],input_root=str(sources))==m[1]
        template={'schema_version':1,'id':'key-template','scene':m[0],'parameters':[]}
        assert client.call('graphics.instantiate',template=template,instance_id='key-copy',values={},input_root=str(sources))['scene']['layers'][0]['effects']==m[0]['layers'][0]['effects']
        project=request({'command':'project.create','id':'key-edit','width':128,'height':64,'frame_rate':time(25)})
        client.call('session.create',project=project,request_id='create',store_root=str(store))
        ops=[{'op':'media.add','asset':m[2]['asset']},{'op':'clip.append','clip':{'id':'key','asset_id':m[0]['id'],'source_in':time(4,25),'duration':time(12,25)}}]
        fields={'store_root':str(store),'project_id':'key-edit','expected_revision':0,'request_id':'compiled','operations':ops}
        receipt=client.call('session.apply',**fields);assert receipt==client.call('session.apply',**fields)
        saved=client.call('session.get',store_root=str(store),project_id='key-edit')
        request({'command':'render.run','project':saved,'input_root':str(output),'output_root':str(output),'output':str(output/'saved.mkv')})
        verify_bytes(output/'saved.mkv',m[0],lambda n:m[3](n+4),12)
    finally:client.close()
    passed.append('keying.presets_mcp_templates_sessions')

    invalid=[]
    def bad(e,code='INVALID_EFFECT'):invalid.append((base([e]),code))
    bad(key(key_rgb=[128]*3));bad(key(inner_milli=181));bad(key(outer_milli=1001));bad(key(strength_milli=1001));bad(key(unmix_milli=1001))
    bad(key(spill={'channel':'green','strength_milli':1001}));bad(key(strength_curve=curve([(0,1,-1,'hold')])),'INVALID_ANIMATION')
    bad(key(strength_curve=curve([(0,1,1001,'hold')])),'INVALID_ANIMATION');bad(key(strength_curve={'keys':[]}),'INVALID_ANIMATION')
    bad(key(strength_curve=curve([(0,1,0,'linear'),(0,2,1000,'hold')])),'INVALID_ANIMATION')
    bad(key(mask={'rect':[0,0,-1,1],'feather':0}),'INVALID_MASK');bad(key(mask={'rect':[32769,0,1,1],'feather':0}),'INVALID_MASK')
    bad(key(mask={'rect':[0,0,1,1],'feather':4097}));bad(key(mask={'rect':[0,0,1,1],'feather':0,'animation':{}}),'INVALID_MASK')
    bad(key(mask={'rect':[0,0,1,1],'feather':0,'animation':{'height':curve([(0,1,-1,'hold')])}}),'INVALID_ANIMATION')
    bad(key(key_rgb=[0,256,0]),'INVALID_JSON');bad(key(key_rgb=[0,255]),'INVALID_JSON');bad(key(strength_milli=0.5),'INVALID_JSON')
    bad(key(spill={'channel':'yellow','strength_milli':1000}),'INVALID_JSON');bad(key(spill={'channel':'green','strength_milli':-1}),'INVALID_JSON')
    bad(key(spill={'channel':'green','strength_milli':500,'radius':3}),'INVALID_JSON');bad(key(key_curve={}),'INVALID_JSON')
    invalid.append((base([key()]*9),'INVALID_EFFECT'))
    hidden=base([key(key_rgb=[0,0,0],strength_milli=0)]);hidden['layers'][0]['transform']['opacity']=0;invalid.append((hidden,'INVALID_EFFECT'))
    outputs={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(s,code) in enumerate(invalid):render(s,'rejected-'+str(i),code)
    preset_bad=[({'name':'unknown','strength_milli':1000,'spill_milli':0},'INVALID_JSON'),({'name':'green_soft','strength_milli':1001,'spill_milli':0},'INVALID_EFFECT'),
                ({'name':'blue_soft','strength_milli':1000,'spill_milli':1001},'INVALID_EFFECT'),({'name':'green_soft','strength_milli':1000},'INVALID_JSON')]
    for args,code in preset_bad:request({'command':'effects.preset',**args},code)
    render(m[0],'animated-key','OUTPUT_EXISTS')
    assert outputs=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append('keying.validation_and_preservation')
    report={'passed':passed,'decoded_frames':decoded,'silent_stereo_sample_frames':decoded*1920,'render_cases':cases,'rejected_cases':len(invalid)+len(preset_bad)+1,
            'maximum_rgb_error':maximum_error,'rgb_tolerance':1,'quality':quality,
            'reference':'Exact Fraction opponent geometry, matte multiplication and masks; 48-digit Decimal screen unmix/spill/grading; forward composition; original known-alpha neutral edge and single-pixel strand truth',
            'scope':'Bounded original chroma-distance keys, spill, optional screen subtraction, hard/soft presets and independent effect chains; no automatic screen estimation, spatial matte cleanup or arbitrary native video track path'}
    (root/'animated-scene.json').write_text(json.dumps(m[0],indent=2)+'\n',encoding='utf-8')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
