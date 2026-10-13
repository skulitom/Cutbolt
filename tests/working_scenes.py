"""Independent complete scene frames: linear composition, exact clocks and forward 3D projection."""
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import wave

from PIL import Image
from engine import BUILD
from scenes import identity, time, selected
from working_color import Q, decode, encode, quantize, over
from working_effects import exact_sample, nearest, curve, effect_pixel
from working_layers import reference as layer_reference
from graphics import shape_pixels
from geometry import reference_sample as geometry_reference, node, vector, scalar, curve as geometry_curve
from expressions import constant
from temporal import exposure
from tracking import coverage

ROOT=Path(__file__).resolve().parents[1]


def fraction(value): return F(value['num'],value['den'])


def linear(pixel,alpha='straight'):
    denominator=pixel[3] if alpha=='premultiplied' else 255
    return [quantize(decode(v/denominator,'srgb')) for v in pixel[:3]]+[pixel[3]*257] if denominator else [0]*4


def average(pixels):
    alpha=sum(p[3] for p in pixels)
    if not alpha:return [0]*4
    return [nearest(F(sum(p[c]*p[3] for p in pixels),alpha)) for c in range(3)]+[nearest(F(alpha,len(pixels)))]


class Oracle:
    def __init__(self,root,expression=None): self.root=root;self.expression=expression
    @staticmethod
    def background(scene):return [0]*4 if scene.get('transparent') else linear(scene['background']+[255])
    def image(self,layer,frame):
        if layer.get('graphics'):return shape_pixels(layer['graphics'],layer['canvas'])
        with Image.open(self.root/frame['image']['path']) as im: result=im.convert('RGBA')
        if frame.get('matte'):
            with Image.open(self.root/frame['matte']['path']) as im: matte=list(im.convert('RGB').getdata())
            result.putdata([p if m[0] else (0,0,0,0) for p,m in zip(result.getdata(),matte)])
        return result
    def opacity(self,layer,t):
        if self.expression is not None:return nearest(self.expression(layer,t)*257)
        when=t-fraction(layer['start'])
        return nearest(exact_sample(layer['animation']['opacity'],when)*257) if layer.get('animation',{}).get('opacity') else layer['transform']['opacity']*257
    @staticmethod
    def texture(pixel,layer,position,local):
        result=effect_pixel(linear(pixel,layer.get('alpha_mode','straight')),position,layer.get('effects',[]),local,{})
        return result[:3],result[3]
    @staticmethod
    def light(colors,gain):return [quantize(decode(min(1,max(0,encode(v/Q,'srgb')*float(g))),'srgb')) for v,g in zip(colors,gain)]
    @staticmethod
    def coverage(layer,rect,x,y,alpha,opacity):
        weight=coverage(layer['mask'],rect,x,y) if layer.get('mask') else F(1)
        return alpha,opacity,int(weight*65536)
    @staticmethod
    def over(colors,amount,mode,backdrop):
        alpha,opacity,mask=amount
        return over(dict(source=list(colors)+[alpha],backdrop=backdrop,opacity=opacity,mask=mask,mode=mode))
    def sample(self,scene,t):
        if scene.get('geometry'):return geometry_reference(scene,self.root,t,self)
        base=[self.background(scene) for _ in range(scene['width']*scene['height'])]
        if not 0<=t<fraction(scene['duration']):return base
        draws=[]
        for layer in scene['layers']:
            local=t-fraction(layer['start'])
            if not 0<=local<fraction(layer['duration']):continue
            if layer.get('graphics'):
                image=self.image(layer,None);offset=[0,0];anchor=[0,0]
            elif layer.get('tilemap'):
                m=layer['tilemap'];image=Image.new('RGBA',tuple(layer['canvas']));offset=[0,0];anchor=[0,0]
                for y,row in enumerate(m['cells']):
                    for x,index in enumerate(row):
                        if index is None:continue
                        tile=m['tiles'][index]
                        selector=dict(start=layer['start'],duration=layer['duration'],frames=tile['frames'],end=tile['end'])
                        index=selected(selector,t*25)
                        if index is not None:
                            frame=tile['frames'][index]
                            image.paste(self.image(layer,frame),(x*m['tile_size'][0]+frame['offset'][0],y*m['tile_size'][1]+frame['offset'][1]))
            else:
                index=selected(layer,t*25)
                if index is None:continue
                frame=layer['frames'][index];image=self.image(layer,frame);offset=frame['offset'];anchor=frame['anchor']
            description={key:copy.deepcopy(layer[key]) for key in ('canvas','transform','animation','mask','effects','blend_mode') if key in layer}
            # The frame oracle accepts exact Fraction keys, independently of the engine's u8 curve storage.
            description.setdefault('animation',{})['opacity']=curve([(0,1,F(self.opacity(layer,t),257))])
            draws.append(dict(layer=description,duration=layer['duration'],time=time(local.numerator,local.denominator),anchor=anchor,
                              size=list(image.size),offset=offset,pixels=[linear(p,layer.get('alpha_mode','straight')) for p in image.getdata()]))
        return layer_reference(dict(size=[scene['width'],scene['height']],backdrop=base,draws=draws))
    def frame(self,scene,n,divisor):
        rate=fraction(scene.get('frame_rate',time(25)));base=F(n)/rate;spec=scene.get('temporal')
        times=[base] if spec is None else [base+(fraction(spec['phase'])+fraction(spec['shutter_angle'])*F(2*k+1,2*spec['samples']))/(360*rate) for k in range(spec['samples'])]
        samples=[self.sample(scene,t) for t in times]
        pixels=samples[0] if len(samples)==1 else [average(p) for p in zip(*samples)]
        scale=scene['output_scale'];w=scene['width']*scale//divisor;h=scene['height']*scale//divisor
        if scale==1 and divisor==1:return pixels
        if divisor==1:return [pixels[(y//scale)*scene['width']+x//scale] for y in range(h) for x in range(w)]
        return [average([pixels[((y*divisor+dy)//scale)*scene['width']+(x*divisor+dx)//scale] for dy in range(divisor) for dx in range(divisor)]) for y in range(h) for x in range(w)]


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources=root/'sources';sources.mkdir(exist_ok=True)
    subprocess.run(['cargo','build','--locked','--example','working_scenes_fixture'],cwd=ROOT,check=True)
    helper=BUILD/'examples'/('working_scenes_fixture.exe' if os.name=='nt' else 'working_scenes_fixture')
    for k in range(2):
        image=Image.new('RGBA',(8,6));image.putdata([((x*33+y*12+k*51)%255,(y*39+x*15+k*66)%255,(201-x*9+y*6+k*24)%255,[0,1,85,170,254,255][(x+y+k)%6]) for y in range(6) for x in range(8)])
        image.save(sources/f'original-{k}.png')
        image.putdata([tuple(v*p[3]//255 for v in p[:3])+(p[3],) for p in image.getdata()]);image.save(sources/f'premult-{k}.png')
    matte=Image.new('RGB',(8,6));matte.putdata([(255,255,255) if (x+y)%3 else (0,0,0) for y in range(6) for x in range(8)]);matte.save(sources/'matte.png')
    Image.new('RGBA',(8,6),(255,255,255,255)).save(sources/'white.png')
    pcm=[(n*53)%20001-10000 for n in range(5*1920*2)]
    with wave.open(str(sources/'sound.wav'),'wb') as wav:
        wav.setparams((2,2,48000,0,'NONE','not compressed'));wav.writeframes(struct.pack('<'+'h'*len(pcm),*pcm))
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    duration=time(6,25)
    def frame(name,k=0):return dict(image=identity(sources/name,sources),hold=time(k+1,25),offset=[0,0],anchor=[0,0])
    def layer(label,start=0,count=6):return dict(id=label,canvas=[8,6],start=time(start,25),duration=time(count,25),frames=[frame(f'original-{k}.png',k) for k in range(2)],
                                               timing='strict',end='loop',transform=dict(position=[5,4],crop=[0,0,8,6],scale=1,quarter_turns=0,opacity=215))
    scene=dict(schema_version=1,id='wide-scene',width=24,height=16,output_scale=1,duration=duration,background=[17,31,53],color='srgb_straight_encoded',layers=[layer('A'),layer('B',1,5)],audio=None)
    cases=[];digest=hashlib.sha256();rejected=0
    def call(s,requests,error=None,audio=None):
        nonlocal rejected
        request=dict(scene=s,frames=requests,audio=audio or [])
        result=subprocess.run([str(helper),str(sources)],input=json.dumps(request),text=True,encoding='utf-8',capture_output=True,timeout=120)
        response=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and response['error']['code']==error,response
            rejected+=1;return
        if result.returncode:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert result.returncode==0 and response['ok'],response
        return response['result']
    def check(name,s,requests=None,expression=None,audio=None):
        requests=requests if requests is not None else [(n,1) for n in range(int(fraction(s['duration'])*fraction(s.get('frame_rate',time(25)))))]
        response=call(s,requests,audio=audio);oracle=Oracle(sources,expression);maximum=0
        for request,actual in zip(requests,response['frames']):
            wanted=oracle.frame(s,*request);assert len(actual)==len(wanted)
            delta=max(abs(a-b) for p,q in zip(actual,wanted) for a,b in zip(p,q));maximum=max(maximum,delta)
            assert delta<=2,(name,request,delta,next((i,p,q) for i,(p,q) in enumerate(zip(actual,wanted)) if max(abs(a-b) for a,b in zip(p,q))>2))
        digest.update(json.dumps(response['frames'],separators=(',',':')).encode())
        assert 0<response['retained_bytes']<1<<30 and 0<response['frame_buffer_bytes']<1<<30
        assert response['report']['profile']=='linear-unorm16-scene-source'
        cases.append(dict(name=name,frames=len(requests),pixels=sum(len(f) for f in response['frames']),maximum_unorm16_error=maximum,tolerance=2))
        return response
    check('held_sources',scene)
    for ending in ('hold_last','transparent'):
        s=copy.deepcopy(scene);s['layers'][0]['end']=ending;check(ending,s)
    s=copy.deepcopy(scene);s['layers'][0]['frames'][0]['offset']=[2,1];s['layers'][0]['frames'][0]['anchor']=[3,2];s['layers'][0]['canvas']=[10,8]
    s['layers'][0]['transform'].update(crop=[1,1,8,6],quarter_turns=3,scale=2);check('offset_anchor_crop_turn_scale',s)
    premult=copy.deepcopy(scene)
    for l in premult['layers']:
        l['alpha_mode']='premultiplied'
        for i,f in enumerate(l['frames']):f['image']=identity(sources/f'premult-{i}.png',sources)
    check('encoded_premultiplied_sources',premult)
    masked=copy.deepcopy(premult)
    for l in masked['layers']:
        for f in l['frames']:f['matte']=identity(sources/'matte.png',sources)
    check('binary_matte_before_decode',masked)
    graded=copy.deepcopy(scene)
    graded['layers'][0]['effects']=[dict(kind='grade',exposure_milli=233,contrast_milli=1127,white_balance_milli=[1090,977,1041]),
                                  dict(kind='chroma_key',key_rgb=[0,255,0],inner_milli=13,outer_milli=757,strength_milli=913)]
    graded['layers'][0]['animation']=dict(opacity=curve([(0,1,0),(6,25,255)],'ease_in'))
    for mode in ('normal','multiply','screen'):
        graded['layers'][1]['blend_mode']=mode;check('grade_key_fade_'+mode,graded)
    transparent=copy.deepcopy(graded);transparent['transparent']=True
    # Transparent source scenes keep their existing validation contract (normal blend only).
    transparent['layers'][1]['blend_mode']='normal';check('transparent_scene',transparent)
    shutter=copy.deepcopy(transparent);shutter['temporal']=exposure(8);check('transparent_shutter_edges',shutter)
    from tracking import spatial
    filtered=copy.deepcopy(graded);filtered['layers'][0]['transform']['spatial']=spatial(sampling='bilinear',rotation_mdeg=17331,translate_milli=[379,-217])
    filtered['layers'][0]['mask']=dict(rect=[1,1,5,4],inverted=False,feather=dict(radius=2,edge='centered'));check('filtered_mask_grade',filtered)
    scaled=copy.deepcopy(shutter);scaled['output_scale']=3;check('scaled_and_proxy_frames',scaled,[(n,d) for n in (0,3,5) for d in (1,2,4,8)])
    # 1/3 coverage rounds to zero in the old byte control, yet must remain 86/65535 here.
    tiny=copy.deepcopy(scene);tiny['layers']=[layer('A')];tiny['layers'][0]['frames']=[frame('white.png')];tiny['layers'][0]['end']='hold_last';tiny['transparent']=True
    tiny['expressions']=dict(schema_version=1,seed=0,nodes=[constant('tiny',F(1,3))],bindings=[dict(layer='A',property='opacity',node='tiny')])
    result=check('exact_expression_subbyte_opacity',tiny,expression=lambda l,t:F(1,3));assert any(p[3]==86 for p in result['frames'][0])
    # Same sampled u8 opacity at every frame: a byte-derived static-base cache would freeze it.
    animated=copy.deepcopy(tiny);animated.pop('expressions');animated['layers'][0]['animation']=dict(opacity=curve([(0,1,0),(6,25,1)],'ease_in'))
    result=check('fractional_fade_changes_within_one_byte',animated);assert result['frames'][1]!=result['frames'][2]
    large=copy.deepcopy(tiny);value=F(224999999999999,1000000000000)
    large['expressions']['nodes']=[constant('tiny',value)];check('large_expression_rational',large,expression=lambda l,t:value)
    static=copy.deepcopy(scene);static['layers']=[layer('A')];static['layers'][0]['frames']=[frame('original-0.png')];static['layers'][0]['end']='hold_last'
    result=check('static_prefix_cache',static);assert result['report']['working_processing']['cached_layers']==1
    graphic=copy.deepcopy(static);graphic['layers'][0]['frames']=[];graphic['layers'][0]['graphics']=dict(kind='shape',shape='rectangle',rect=[1,1,5,3],fill=[101,37,213,171],stroke=None)
    check('trimmed_generated_graphics',graphic)
    graphic['layers'][0]['graphics']['rect']=[30,30,2,2];check('empty_graphics',graphic)
    tile=copy.deepcopy(static);l=tile['layers'][0];l['frames']=[];l['canvas']=[16,12];l['transform']['crop']=[0,0,16,12]
    l['tilemap']=dict(tile_size=[8,6],cells=[[0,1],[None,0]],tiles=[dict(frames=[frame('original-0.png'),frame('original-1.png',1)],timing='strict',end='loop'),dict(frames=[frame('premult-0.png')],timing='strict',end='transparent')])
    check('held_tile_canvas',tile)
    sound=copy.deepcopy(scene);sound['audio']=dict(file=identity(sources/'sound.wav',sources),start=time(1,25),channels='preserve_stereo',resampling='linear',padding='silence')
    blocks=[(0,1921),(2877,1931),(6*1920-7,7),(6*1920,0)]
    result=check('original_pcm_windows',sound,[(2,1)],audio=blocks);expected=[0]*1920*2+pcm
    assert result['audio']==[expected[n*2:(n+c)*2] for n,c in blocks] and result['audio_samples']==6*1920
    result=call(scene,[],audio=[(100,77)]);assert result['audio']==[[0]*154]
    camera=dict(position_milli=vector([0,0,100000]),target_milli=vector([0,0,0]),up_milli=vector([0,1000,0]),projection=dict(kind='perspective',vertical_fov_mdeg=scalar(90000)),near_milli=1000,far_milli=1000000)
    planes=copy.deepcopy(graded)
    for l in planes['layers']:l['transform']['position']=[0,0]
    planes['geometry']=dict(camera=camera,nodes=[node('plane-a','A',(-13000,1000,0),rotation=(7000,37000,5000),extent=(70000,55000)),node('plane-b','B',(12000,-4000,30000),rotation=(-13000,-29000,9000),extent=(60000,50000))],lights=[],shadows='none')
    check('perspective_intersecting_depth',planes)
    tied=copy.deepcopy(planes)
    for n in tied['geometry']['nodes']:n['transform']['position_milli']['value']=[0,0,0];n['transform']['rotation_mdeg']['value']=[0,0,0]
    tied_result=check('3d_equal_depth',tied)
    tied['layers'].reverse();tied['geometry']['nodes'].reverse();assert check('3d_equal_depth_reordered',tied)['frames']==tied_result['frames']
    plane_mask=copy.deepcopy(planes);plane_mask['layers'][0]['mask']=dict(rect=[1,1,5,4],inverted=True,feather=dict(radius=2,edge='centered'))
    check('3d_feathered_mask',plane_mask)
    shape=copy.deepcopy(planes);l=shape['layers'][0];l['frames']=[];l['end']='hold_last';l['graphics']=dict(kind='shape',shape='rectangle',rect=[1,1,5,3],fill=[101,37,213,171],stroke=None)
    check('3d_graphics_texture',shape)
    graph=copy.deepcopy(planes);graph['expressions']=dict(schema_version=1,seed=0,nodes=[constant('tiny',F(1,3))],bindings=[dict(layer=l['id'],property='opacity',node='tiny') for l in graph['layers']])
    check('3d_subbyte_expression_opacity',graph,expression=lambda l,t:F(1,3))
    hierarchy=copy.deepcopy(planes);g=hierarchy['geometry'];g['nodes'][0]['parent']='group';g['nodes'].append(node('group',position=(6000,-2000,0),rotation=(0,0,17000),size=(1250,800,1100)))
    g['nodes'][2]['transform']['rotation_mdeg']['animation']={'z':geometry_curve(17000,-19000,duration)}
    g['camera']['position_milli']['animation']={'x':geometry_curve(10000,-15000,duration)};g['camera']['projection']['vertical_fov_mdeg']['animation']=geometry_curve(90000,60000,duration)
    check('3d_animated_hierarchy_camera',hierarchy)
    ortho=copy.deepcopy(planes);ortho['geometry']['camera']['projection']=dict(kind='orthographic',vertical_size_milli=scalar(120000));check('orthographic_depth',ortho)
    lit=copy.deepcopy(planes)
    for n in lit['geometry']['nodes']:n['plane']['material']='lambert'
    lit['geometry']['lights']=[dict(kind='ambient',color=[255,211,173],intensity_milli=scalar(500)),dict(kind='directional',color=[191,229,255],intensity_milli=scalar(500,geometry_curve(200,1400,duration)),toward_light_milli=vector([300,400,1000]))]
    check('continuous_encoded_material_lighting',lit)
    lit['geometry']['lights'].append(dict(kind='point',color=[233,117,197],intensity_milli=scalar(431),position_milli=vector([30000,10000,60000]),falloff='inverse_square',reference_distance_milli=50000));check('point_light',lit)
    lit['temporal']=exposure(4);check('lit_shutter',lit,[(0,1),(3,1),(5,2)])
    clipped=copy.deepcopy(planes);clipped['geometry']['camera'].update(near_milli=70000,far_milli=95000);check('3d_near_far_clipping',clipped)
    backwards=copy.deepcopy(planes);backwards['geometry']['nodes'][0]['transform']['rotation_mdeg']['value']=[0,180000,0]
    check('3d_backface',backwards);backwards['geometry']['nodes'][0]['plane']['double_sided']=True;check('3d_double_sided',backwards)
    native=copy.deepcopy(static);native['frame_rate']=time(60);native['duration']=time(1,10);native['layers'][0]['duration']=time(1,10);native['layers'][0]['frames'][0]['hold']=time(1,60);native['temporal']=exposure(4)
    check('native_60fps_shutter',native)
    for requests,audio in [([(6,1)],None),([(0,3)],None),([],[ (6*1920,1) ]),([],[ (2**64-1,2) ])]:call(scene,requests,'INVALID_RANGE',audio)
    bad=copy.deepcopy(scene);bad['layers'][0]['frames'][0]['image']['sha256']='0'*64;call(bad,[],'MEDIA_CHANGED')
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    report=dict(passed=['working_scenes.complete_2d_frames_and_source_interpretation','working_scenes.exact_opacity_curves_and_expressions',
                        'working_scenes.temporal_proxy_and_native_clocks','working_scenes.generated_graphics_tiles_and_static_prefix',
                        'working_scenes.independent_forward_3d_and_lighting','working_scenes.audio_identity_and_bounds'],
                cases=cases,frames=sum(c['frames'] for c in cases),pixel_samples=sum(c['pixels'] for c in cases),maximum_unorm16_error=max(c['maximum_unorm16_error'] for c in cases),
                numerical_response_sha256=digest.hexdigest(),rejected_cases=rejected,source_hashes_preserved=True,
                reference='Independent exact Fraction clocks/coverage, Decimal forward 3D polygons, and f64 color/effect equations; at most two UNORM16 codes per final channel',
                limits='Prepared precise scene library source; project setting, generated/nested track integration and final timeline delivery remain open')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
