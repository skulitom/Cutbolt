"""Original planes; independent high-precision forward projection and polygon clipping.

The production renderer intersects camera rays in inverse plane coordinates.
This reference transforms plane corners forward, clips polygons in camera space,
triangulates the projected polygons and interpolates depth/texture coordinates.
"""
from engine import ENGINE, MCP_TOOLS
import argparse
import budgets
import copy
from decimal import Decimal as D, getcontext, ROUND_HALF_UP
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import random
import subprocess
import time as clock
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from animation import sample
from scenes import identity,time,selected
from spatial import trig
from keying import pixel_reference,resolve_chain
from compositing import rect_at
from temporal import exposure,sample_times

ROOT=Path(__file__).resolve().parents[1]
getcontext().prec=60


def dec(v):return D(v.numerator)/D(v.denominator) if isinstance(v,F) else D(v)
def add(a,b):return tuple(x+y for x,y in zip(a,b))
def sub(a,b):return tuple(x-y for x,y in zip(a,b))
def scale(a,b):return tuple(x*b for x in a)
def dot(a,b):return sum(x*y for x,y in zip(a,b))
def cross(a,b):return (a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0])
def unit(a):return scale(a,1/dot(a,a).sqrt())
def frac(t):return F(t['num'],t['den'])
def scalar(v,curve=None):return {'value':v,**({'animation':curve} if curve else {})}
def vector(v,**curves):return {'value':v,**({'animation':curves} if curves else {})}
def curve(a,b,duration):return {'keys':[{'time':time(0),'value':a,'interpolation':'linear'},{'time':duration,'value':b,'interpolation':'hold'}]}
def scalar_at(s,t):return sample(s['animation'],t) if s.get('animation') else s['value']
def vector_at(v,t):return [sample(v['animation'][name],t) if name in v.get('animation',{}) else v['value'][i] for i,name in enumerate(('x','y','z'))]
def transform(position=(0,0,0),rotation=(0,0,0),size=(1000,1000,1000)):
    return {'position_milli':vector(list(position)),'rotation_mdeg':vector(list(rotation)),'scale_milli':vector(list(size))}
def node(label,layer=None,position=(0,0,0),rotation=(0,0,0),size=(1000,1000,1000),parent=None,material='unlit',extent=(32000,24000),double=False):
    return {'id':label,'transform':transform(position,rotation,size),**({'parent':parent} if parent else {}),
            **({'plane':{'layer':layer,'size_milli':list(extent),'material':material,'double_sided':double}} if layer else {})}


def forward(nodes,name,point,t):
    if name is None:return tuple(map(dec,point))
    n=nodes[name];tr=n['transform'];p=tuple(map(dec,point))
    p=tuple(v*D(s)/1000 for v,s in zip(p,vector_at(tr['scale_milli'],t)))
    angles=vector_at(tr['rotation_mdeg'],t)
    sn,cs=trig(angles[0]);x,y,z=p;p=(x,cs*y-sn*z,sn*y+cs*z)
    sn,cs=trig(angles[1]);x,y,z=p;p=(cs*x+sn*z,y,-sn*x+cs*z)
    sn,cs=trig(angles[2]);x,y,z=p;p=(cs*x-sn*y,sn*x+cs*y,z)
    p=add(p,tuple(D(v)/1000 for v in vector_at(tr['position_milli'],t)))
    return forward(nodes,n.get('parent'),p,t)


def clip_polygon(vertices,bound,greater):
    result=[]
    for a,b in zip(vertices,vertices[1:]+vertices[:1]):
        inside_a=a[2]>=bound if greater else a[2]<=bound
        inside_b=b[2]>=bound if greater else b[2]<=bound
        if inside_a:result.append(a)
        if inside_a!=inside_b:
            weight=(bound-a[2])/(b[2]-a[2])
            result.append(tuple(x+(y-x)*weight for x,y in zip(a,b)))
    return result


def barycentric(point,triangle):
    a,b,c=triangle
    det=(b[1]-c[1])*(a[0]-c[0])+(c[0]-b[0])*(a[1]-c[1])
    if not det:return None
    wa=((b[1]-c[1])*(point[0]-c[0])+(c[0]-b[0])*(point[1]-c[1]))/det
    wb=((c[1]-a[1])*(point[0]-c[0])+(a[0]-c[0])*(point[1]-c[1]))/det
    weights=(wa,wb,1-wa-wb)
    return weights if all(0<=v<=1 for v in weights) else None


def reference_sample(scene,sources,t):
    w,h=scene['width'],scene['height'];pixels=[list(scene['background']) for _ in range(w*h)]
    if not 0<=t<frac(scene['duration']):return bytes(v for p in pixels for v in p)
    g=scene['geometry'];nodes={n['id']:n for n in g['nodes']};c=g['camera'];parent=c.get('parent')
    def point(v,p=parent):return forward(nodes,p,tuple(D(x)/1000 for x in vector_at(v,t)),t)
    camera=point(c['position_milli']);target=point(c['target_milli']);forward_axis=unit(sub(target,camera))
    up=unit(sub(point(c['up_milli']),forward(nodes,parent,(0,0,0),t)));right=unit(cross(forward_axis,up));up=cross(right,forward_axis)
    perspective=c['projection']['kind']=='perspective'
    if perspective:
        fov=scalar_at(c['projection']['vertical_fov_mdeg'],t)
        sn,cs=trig(D(fov)/2);vertical=2*sn/cs
    else:vertical=D(scalar_at(c['projection']['vertical_size_milli'],t))/1000
    near,far=D(c['near_milli'])/1000,D(c['far_milli'])/1000
    lights=[]
    for light in g['lights']:
        gain=tuple(D(v)*scalar_at(light['intensity_milli'],t)/255000 for v in light['color'])
        p=light.get('parent')
        if light['kind']=='ambient':lights.append(('ambient',gain,None,None,None))
        elif light['kind']=='directional':
            toward=unit(sub(point(light['toward_light_milli'],p),forward(nodes,p,(0,0,0),t)))
            lights.append(('directional',gain,toward,None,None))
        else:lights.append(('point',gain,point(light['position_milli'],p),light['falloff'],D(light['reference_distance_milli'])/1000))
    hits=[[] for _ in range(w*h)]
    layers={l['id']:l for l in scene['layers']}
    for n in g['nodes']:
        if 'plane' not in n:continue
        p=n['plane'];layer=layers[p['layer']];local=t-frac(layer['start'])
        if not 0<=local<frac(layer['duration']):continue
        index=selected(layer,t*25)
        if index is None:continue
        frame=layer['frames'][index]
        with Image.open(sources/frame['image']['path']) as image:source=image.convert('RGBA')
        ex,ey=[D(v)/1000 for v in p['size_milli']]
        corners=[forward(nodes,n['id'],v,t) for v in [(-ex/2,ey/2,0),(ex/2,ey/2,0),(ex/2,-ey/2,0),(-ex/2,-ey/2,0)]]
        normal=unit(cross(sub(corners[1],corners[0]),sub(corners[0],corners[3])))
        front=dot(normal,sub(camera,corners[0]) if perspective else scale(forward_axis,D(-1)))>0
        if not front and not p['double_sided']:continue
        if not front:normal=scale(normal,D(-1))
        vertices=[]
        for world,uv in zip(corners,[(0,0),(1,0),(1,1),(0,1)]):
            delta=sub(world,camera)
            vertices.append((dot(delta,right),dot(delta,up),dot(delta,forward_axis),*world,*map(D,uv)))
        vertices=clip_polygon(clip_polygon(vertices,near,True),far,False)
        if len(vertices)<3:continue
        projected=[]
        for v in vertices:
            divisor=v[2] if perspective else D(1)
            projected.append((D(w)/2+v[0]*h/(vertical*divisor),D(h)/2-v[1]*h/(vertical*divisor)))
        opacity=sample(layer['animation']['opacity'],local) if layer.get('animation',{}).get('opacity') else layer['transform']['opacity']
        if scene.get('expressions'):
            q=255-100*t
            if layer['id']=='B':q/=2
            opacity=int(q+F(1,2))
        chain=json.dumps(resolve_chain(layer.get('effects',[]),local),sort_keys=True)
        mask=rect_at(layer,t*25);cx,cy,cw,ch=layer['transform']['crop'];cache={}
        for y in range(h):
            for x in range(w):
                point2=(D(x)+D('.5'),D(y)+D('.5'));interpolated=None
                for k in range(1,len(vertices)-1):
                    ids=[0,k,k+1];weights=barycentric(point2,[projected[i] for i in ids])
                    if weights is None:continue
                    weights=[weight/vertices[i][2] if perspective else weight for weight,i in zip(weights,ids)]
                    total=sum(weights);weights=[v/total for v in weights]
                    interpolated=[sum(weight*vertices[i][column] for weight,i in zip(weights,ids)) for column in range(2,8)]
                    break
                if interpolated is None:continue
                depth,wx,wy,wz,u,v=interpolated
                if not near<=depth<far or not 0<=u<1 or not 0<=v<1:continue
                sx,sy=int(u*layer['canvas'][0]),int(v*layer['canvas'][1])
                if not cx<=sx<cx+cw or not cy<=sy<cy+ch:continue
                if mask:
                    mx,my,mw,mh=mask;inside=mx<=sx<mx+mw and my<=sy<my+mh
                    if inside==layer['mask'].get('inverted',False):continue
                ix,iy=sx-frame['offset'][0],sy-frame['offset'][1]
                if not 0<=ix<source.width or not 0<=iy<source.height:continue
                if (sx,sy) not in cache:cache[sx,sy]=pixel_reference(source.getpixel((ix,iy)),layer.get('alpha_mode','straight'),(sx,sy),chain)
                colors,alpha=cache[sx,sy]
                if p['material']=='lambert' and alpha:
                    gain=[D(0)]*3
                    for kind,color,position,falloff,distance in lights:
                        if kind=='ambient':factor=D(1)
                        elif kind=='directional':factor=max(D(0),dot(normal,position))
                        else:
                            delta=sub(position,(wx,wy,wz));squared=dot(delta,delta)
                            if not squared:continue
                            factor=max(D(0),dot(normal,delta)/squared.sqrt())*(distance*distance/squared if falloff=='inverse_square' else 1)
                        gain=[a+b*factor for a,b in zip(gain,color)]
                    colors=tuple(F(int(max(D(0),min(D(255),dec(v)*255*q)).to_integral_value(rounding=ROUND_HALF_UP)),255) for v,q in zip(colors,gain))
                hits[y*w+x].append((depth,n['id'],colors,F(alpha*opacity,255*255),layer.get('blend_mode','normal')))
    for i,items in enumerate(hits):
        for depth,name,colors,coverage,mode in sorted(items,key=lambda hit:(-hit[0],hit[1])):
            for c,color in enumerate(colors):
                back=F(pixels[i][c],255)
                blended=color if mode=='normal' else color*back if mode=='multiply' else 1-(1-color)*(1-back)
                pixels[i][c]=int(255*(coverage*blended+(1-coverage)*back)+F(1,2))
    return bytes(v for p in pixels for v in p)


def reference(scene,sources,n):
    times=sample_times(scene,n);sums=[0]*(scene['width']*scene['height']*3)
    for t in times:sums=[a+b for a,b in zip(sums,reference_sample(scene,sources,t))]
    return bytes((v+len(times)//2)//len(times) for v in sums)


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/n for n in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    for k in range(2):
        im=Image.new('RGBA',(8,6));im.putdata([((x*33+y*12+k*51)%255,(y*39+x*15+k*66)%255,(201-x*9+y*6+k*24)%255,[0,85,170,255][(x+y+k)%4]) for y in range(6) for x in range(8)])
        im.save(sources/f'original-{k}.png')
        im.putdata([tuple(v*p[3]//255 for v in p[:3])+(p[3],) for p in im.getdata()]);im.save(sources/f'premult-{k}.png')
    Image.new('RGBA',(1,1),(255,255,255,255)).save(sources/'white.png')
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    duration=time(6,25)
    def layer(label,start=0,count=6):return {'id':label,'canvas':[8,6],'start':time(start,25),'duration':time(count,25),
        'frames':[{'image':identity(sources/f'original-{k}.png',sources),'hold':time(k+1,25),'offset':[0,0],'anchor':[0,0]} for k in range(2)],
        'timing':'strict','end':'loop','transform':{'position':[0,0],'crop':[0,0,8,6],'scale':1,'quarter_turns':0,'opacity':215}}
    camera={'position_milli':vector([0,0,100000]),'target_milli':vector([0,0,0]),'up_milli':vector([0,1000,0]),
            'projection':{'kind':'perspective','vertical_fov_mdeg':scalar(90000)},'near_milli':1000,'far_milli':1000000}
    scene={'schema_version':1,'id':'original-planes','width':64,'height':48,'output_scale':1,'duration':duration,'background':[17,31,53],
           'color':'srgb_straight_encoded','layers':[layer('A'),layer('B',1,5)],'audio':None,
           'geometry':{'camera':camera,'nodes':[node('plane-a','A',(-13000,1000,0),extent=(50000,35000)),
                                              node('plane-b','B',(12000,-4000,30000),extent=(40000,30000))],
                       'lights':[],'shadows':'none'}}
    exe=ENGINE;passed=[];cases=[];frames=0;samples=0;rejected=0;previews=0
    def call(command,error=None,**fields):
        nonlocal rejected
        request={'command':command,**fields};p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=240);r=json.loads(p.stdout)
        if error:assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r['error']
        if p.returncode:(root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert p.returncode==0 and r['ok'],r;return r['result']
    def decode(path,kind):
        args=['-an','-pix_fmt','rgb24','-f','rawvideo'] if kind=='video' else ['-vn','-f','s16le']
        return subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),*args,'-'],timeout=180)
    def compare(path,wanted,tolerance=0):
        nonlocal frames,samples
        rgb=decode(path,'video');expected=b''.join(wanted);assert len(rgb)==len(expected),(path,len(rgb),len(expected))
        maximum=max(abs(a-b) for a,b in zip(rgb,expected));assert maximum<=tolerance,(path,maximum,next(((i,a,b) for i,(a,b) in enumerate(zip(rgb,expected)) if abs(a-b)>tolerance),None))
        audio=decode(path,'audio');assert audio==bytes(len(wanted)*1920*4)
        frames+=len(wanted);samples+=len(audio)//4;return maximum
    def check(s,name,tolerance=0,oracle=None):
        s=copy.deepcopy(s);s['id']=name;count=int(frac(s['duration'])*25)
        receipt=call('scene.render',scene=s,input_root=str(sources),output_root=str(output),output=str(output/(name+'.mkv')))
        wanted=[oracle(n) if oracle else reference(s,sources,n) for n in range(count)]
        error=compare(output/(name+'.mkv'),wanted,tolerance)
        # Forward basis-point checks also expose hierarchy/animation errors that
        # happen to be invisible in a clipped or transparent plane.
        slots=[t for n in range(count) for t in sample_times(s,n)];nodes={n['id']:n for n in s['geometry']['nodes']}
        animated=any(v.get('animation') for node_ in nodes.values() for v in node_['transform'].values());basis_cache={}
        for t,reported in zip(slots,receipt['geometry']['node_world_matrices']):
            if not 0<=t<frac(s['duration']):assert reported is None;continue
            for label,matrix in reported.items():
                cache_key=(t if animated else F(0),label)
                if cache_key not in basis_cache:
                    origin=forward(nodes,label,(0,0,0),t)
                    basis_cache[cache_key]=(origin,[sub(forward(nodes,label,basis,t),origin) for basis in [(1,0,0),(0,1,0),(0,0,1)]])
                origin,bases=basis_cache[cache_key]
                for i in range(3):
                    assert abs(float(origin[i])-matrix[i][3])<1e-7
                    for j,basis in enumerate(bases):
                        assert abs(float(basis[i])-matrix[i][j])<1e-7
        cases.append({'name':name,'frames':count,'maximum_rgb_error':error,'allowed_rgb_error':tolerance})
        return receipt,wanted

    receipt,wanted=check(scene,'perspective')
    orthographic=copy.deepcopy(scene);orthographic['geometry']['camera']['projection']={'kind':'orthographic','vertical_size_milli':scalar(60000)}
    check(orthographic,'orthographic')
    tilted=copy.deepcopy(scene);tilted['geometry']['nodes'][0]['transform']['rotation_mdeg']['value']=[7000,37000,5000]
    tilted['geometry']['nodes'][1]['transform']['rotation_mdeg']['value']=[-13000,-29000,9000]
    check(tilted,'intersecting-depth')
    reordered=copy.deepcopy(tilted);reordered['geometry']['nodes'].reverse();reordered['layers'].reverse()
    _,reverse_pixels=check(reordered,'reordered-depth');assert reverse_pixels==[reference(tilted,sources,n) for n in range(6)]
    tied=copy.deepcopy(scene)
    for n in tied['geometry']['nodes']:n['transform']['position_milli']['value'][2]=0
    _,tied_pixels=check(tied,'equal-depth');tied['geometry']['nodes'].reverse();tied['layers'].reverse()
    _,again=check(tied,'equal-depth-reversed');assert again==tied_pixels
    passed.append('geometry.independent_projection_per_pixel_depth_and_alpha')

    clipped=copy.deepcopy(tilted);clipped['geometry']['camera'].update(near_milli=70000,far_milli=95000)
    check(clipped,'near-and-far-clipping')
    offscreen=copy.deepcopy(tilted);offscreen['geometry']['nodes'][0]['transform']['position_milli']['value']=[-130000,60000,20000]
    offscreen['geometry']['nodes'][0]['plane']['size_milli']=[180000,180000]
    check(offscreen,'frustum-edges')
    backward=copy.deepcopy(scene);backward['geometry']['nodes'][0]['transform']['rotation_mdeg']['value']=[0,180000,0]
    check(backward,'backface-hidden');backward['geometry']['nodes'][0]['plane']['double_sided']=True
    check(backward,'double-sided')
    passed.append('geometry.clipping_frustum_edges_and_face_policy')

    hierarchy=copy.deepcopy(tilted);g=hierarchy['geometry']
    g['nodes'][0]['parent']='group';g['nodes'][1]['parent']='child'
    g['nodes'] += [node('group',position=(6000,-2000,0),rotation=(0,0,17000),size=(1250,800,1100)),
                   node('child',parent='group',position=(-5000,1000,0),rotation=(0,11000,0)),node('camera-rig',position=(2000,0,0),rotation=(0,-5000,0))]
    g['nodes'][2]['transform']['rotation_mdeg']['animation']={'z':curve(17000,-19000,duration)}
    g['camera']['parent']='camera-rig';g['camera']['position_milli']['animation']={'x':curve(10000,-15000,duration)}
    g['camera']['target_milli']['animation']={'x':curve(0,8000,duration)}
    g['camera']['projection']['vertical_fov_mdeg']['animation']=curve(90000,60000,duration)
    check(hierarchy,'animated-hierarchy-camera')
    shuffled=copy.deepcopy(hierarchy);random.Random(394).shuffle(shuffled['geometry']['nodes']);check(shuffled,'shuffled-hierarchy')
    orthographic['geometry']['camera']['projection']['vertical_size_milli']['animation']=curve(60000,45000,duration)
    orthographic['geometry']['camera']['up_milli']['animation']={'x':curve(0,500,duration)}
    check(orthographic,'animated-orthographic-roll')
    passed.append('geometry.hierarchy_and_actual_camera_animation')

    lit=copy.deepcopy(tilted)
    for n in lit['geometry']['nodes']:n['plane']['material']='lambert'
    lit['geometry']['lights']=[{'kind':'ambient','color':[255,211,173],'intensity_milli':scalar(500)}]
    check(lit,'ambient',1)
    lit['geometry']['lights'].append({'kind':'directional','color':[191,229,255],'intensity_milli':scalar(500,curve(200,1400,duration)),
                                    'toward_light_milli':vector([300,400,1000],x=curve(-700,700,duration))})
    check(lit,'animated-directional',1)
    lit['geometry']['nodes'].append(node('light-rig',position=(1000,2000,0),rotation=(0,0,20000)))
    lit['geometry']['lights'][1]['parent']='light-rig'
    lit['geometry']['lights'].append({'kind':'point','parent':'light-rig','color':[255,109,67],'intensity_milli':scalar(1200),
        'position_milli':vector([-10000,5000,70000],x=curve(-10000,15000,duration)), 'falloff':'constant','reference_distance_milli':40000})
    check(lit,'animated-point-constant',1);lit['geometry']['lights'][2]['falloff']='inverse_square';check(lit,'animated-point-inverse-square',1)
    lit['geometry']['nodes'][0]['transform']['rotation_mdeg']['value']=[0,180000,0];lit['geometry']['nodes'][0]['plane']['double_sided']=True
    check(lit,'lit-backface-normal',1)
    passed.append('geometry.declared_lighting_and_actual_light_animation')

    premult=copy.deepcopy(scene)
    for l in premult['layers']:
        l['alpha_mode']='premultiplied'
        for k,f in enumerate(l['frames']):f['image']=identity(sources/f'premult-{k}.png',sources)
    _,premult_pixels=check(premult,'premultiplied');assert premult_pixels==wanted
    masked=copy.deepcopy(scene);a=masked['layers'][0];a['transform']['crop']=[1,1,6,4]
    a['mask']={'rect':[0,0,8,6],'inverted':False,'animation':{'width':curve(2,8,duration)}}
    a['effects']=[{'kind':'grade','exposure_milli':1000,'contrast_milli':1000,'white_balance_milli':[1000,1000,1000]}]
    a['animation']={'opacity':curve(0,255,duration)};check(masked,'texture-mask-grade-opacity',1)
    expressions=copy.deepcopy(scene)
    expressions['expressions']={'schema_version':1,'seed':0,'nodes':[
        {'id':'time','kind':'scalar','expression':{'op':'time'}},
        {'id':'rate','kind':'scalar','expression':{'op':'literal','value':{'type':'scalar','value':time(100)}}},
        {'id':'full','kind':'scalar','expression':{'op':'literal','value':{'type':'scalar','value':time(255)}}},
        {'id':'two','kind':'scalar','expression':{'op':'literal','value':{'type':'scalar','value':time(2)}}},
        {'id':'fade','kind':'scalar','expression':{'op':'multiply','a':'time','b':'rate'}},
        {'id':'a','kind':'scalar','expression':{'op':'subtract','a':'full','b':'fade'}},
        {'id':'linked','kind':'scalar','expression':{'op':'property','layer':'A','property':'opacity'}},
        {'id':'b','kind':'scalar','expression':{'op':'divide','a':'linked','b':'two'}}],
        'bindings':[{'layer':'A','property':'opacity','node':'a'},{'layer':'B','property':'opacity','node':'b'}]}
    check(expressions,'linked-opacity')
    temporal=copy.deepcopy(hierarchy);temporal['temporal']=exposure(4)
    check(temporal,'subframe-camera-hierarchy')
    passed.append('geometry.texture_parameters_links_and_temporal_composition')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        tool=next(t for t in catalog if t['name']=='cutbolt_scene_inspect')
        Draft202012Validator(tool['inputSchema']).validate({'scene':scene,'input_root':str(sources)})
        inspected=client.call('scene.inspect',scene=scene,input_root=str(sources));assert inspected['geometry']==receipt['geometry']
        p=client.call('project.create',id='planes-edit',width=64,height=48,frame_rate=time(25));common={'store_root':str(store),'project_id':p['id']}
        client.call('session.create',store_root=str(store),project=p,request_id='create')
        ops=[{'op':'media.add','asset':receipt['asset']},{'op':'clip.append','clip':{'id':'second','asset_id':receipt['asset']['id'],'source_in':time(3,25),'duration':time(3,25)}},
             {'op':'clip.append','clip':{'id':'first','asset_id':receipt['asset']['id'],'source_in':time(0),'duration':time(3,25)}}]
        req={**common,'expected_revision':0,'operations':ops,'request_id':'append'};r=client.call('session.apply',**req);assert client.call('session.apply',**req)==r
        saved=client.call('session.get',**common);fields={'project':saved,'input_root':str(output),'output_root':str(output)}
        call('render.run',**fields,output=str(output/'saved.mkv'));compare(output/'saved.mkv',wanted[3:]+wanted[:3])
        for n,source in [(0,3),(2,5),(3,0),(5,2)]:
            path=output/f'preview-{n}.png';client.call('preview.frame',**fields,output=str(path),time=time(n,25))
            with Image.open(path) as image:assert image.tobytes()==wanted[source]
            previews+=1
        call('preview.range',**fields,output=str(output/'range.mkv'),start=time(2,25),duration=time(2,25));compare(output/'range.mkv',[wanted[5],wanted[0]])
        client.call('session.undo',**common,expected_revision=saved['revision'],request_id='undo');restored=client.call('session.get',**common)
        assert restored['clips']==p['clips'] and restored['assets']==p['assets']
    finally:client.close()
    passed.append('geometry.typed_inspection_saved_edits_retry_undo_and_previews')

    maximum=copy.deepcopy(scene);maximum.update(id='maximum',width=128,height=128,duration=time(64,25));maximum['layers']=[];maximum['geometry']['nodes']=[]
    maximum['geometry']['camera']['projection']={'kind':'orthographic','vertical_size_milli':scalar(64000)}
    for i in range(16):
        l=layer(f'L{i}',0,64);l['canvas']=[1,1];l['frames']=l['frames'][:1];l['frames'][0]['image']=identity(sources/'white.png',sources)
        l['transform']['crop']=[0,0,1,1];l['transform']['opacity']=255;maximum['layers'].append(l)
        maximum['geometry']['nodes'].append(node(f'n{i}',l['id'],position=(0,0,i),extent=(1000000,1000000)))
    # A fixed 16,777,216-visit render (the former maximum). Visits now share the scene compositing budget.
    began=clock.perf_counter();mr,_=check(maximum,'maximum-work',oracle=lambda n:bytes([255])*(128*128*3));elapsed=clock.perf_counter()-began
    assert mr['geometry']['pixel_plane_sample_visits']==mr['work']['composited_pixels']==16777216,elapsed
    budgets.check(elapsed<180,elapsed)
    records=copy.deepcopy(maximum);records.update(width=1,height=1,duration=time(128,25),temporal=exposure(8,360,0))
    for l in records['layers']:l['duration']=records['duration']
    records['geometry']['nodes'] += [node(f'g{i}',parent=f'g{i-1}' if i else None) for i in range(15)]+[node('extra')]
    records['geometry']['nodes'][0]['parent']='g14'
    records['geometry']['lights']=[{'kind':'ambient','color':[255,255,255],'intensity_milli':scalar(125)} for _ in range(8)]
    for n in records['geometry']['nodes']:
        if n.get('plane'):n['plane']['material']='lambert'
    rr,_=check(records,'maximum-records-depth-lights',oracle=lambda n:bytes([255])*3)
    assert rr['geometry']['node_sample_records']==32768 and len(rr['geometry']['sample_states'][0]['lights'])==8
    passed.append('geometry.maximum_work_nodes_depth_lights_and_record_limits')

    invalid=[]
    def bad(name,change,code='INVALID_GEOMETRY',base=None):
        s=copy.deepcopy(base or scene);change(s);invalid.append((name,s,code))
    bad('shadows',lambda s:s['geometry'].update(shadows='ray_traced'),'UNSUPPORTED_GEOMETRY')
    bad('material',lambda s:s['geometry']['nodes'][0]['plane'].update(material='metallic'),'UNSUPPORTED_GEOMETRY')
    bad('mesh',lambda s:s['geometry']['nodes'][0].update(mesh={'vertices':[]}),'INVALID_JSON')
    bad('position',lambda s:s['layers'][0]['transform'].update(position=[1,0]),'UNSUPPORTED_GEOMETRY')
    bad('scale',lambda s:s['layers'][0]['transform'].update(scale=2),'UNSUPPORTED_GEOMETRY')
    bad('anchor',lambda s:s['layers'][0]['frames'][0].update(anchor=[1,0]),'UNSUPPORTED_GEOMETRY')
    bad('position-animation',lambda s:s['layers'][0].update(animation={'position_x':curve(0,1,duration)}),'UNSUPPORTED_GEOMETRY')
    bad('position-expression',lambda s:s.update(expressions={'schema_version':1,'seed':0,
        'nodes':[{'id':'p','kind':'vector2','expression':{'op':'literal','value':{'type':'vector2','value':[time(0),time(0)]}}}],
        'bindings':[{'layer':'A','property':'position','node':'p'}]}),'UNSUPPORTED_GEOMETRY')
    bad('no-nodes',lambda s:s['geometry'].update(nodes=[]),'LIMIT_EXCEEDED')
    bad('many-nodes',lambda s:s['geometry']['nodes'].extend([node(f'extra{i}') for i in range(31)]),'LIMIT_EXCEEDED')
    bad('many-lights',lambda s:s['geometry'].update(lights=[{'kind':'ambient','color':[255,255,255],'intensity_milli':scalar(1)}]*9),'LIMIT_EXCEEDED')
    bad('duplicate-node',lambda s:s['geometry']['nodes'][1].update(id='plane-a'))
    bad('unknown-parent',lambda s:s['geometry']['nodes'][0].update(parent='missing'))
    bad('parent-cycle',lambda s:(s['geometry']['nodes'][0].update(parent='plane-b'),s['geometry']['nodes'][1].update(parent='plane-a')),'GEOMETRY_CYCLE')
    bad('unbound-layer',lambda s:s['geometry']['nodes'][1].pop('plane'))
    bad('duplicate-layer',lambda s:s['geometry']['nodes'][1]['plane'].update(layer='A'))
    bad('unknown-layer',lambda s:s['geometry']['nodes'][1]['plane'].update(layer='missing'))
    bad('zero-size',lambda s:s['geometry']['nodes'][0]['plane'].update(size_milli=[0,1]))
    bad('near-zero',lambda s:s['geometry']['camera'].update(near_milli=0))
    bad('far-before-near',lambda s:s['geometry']['camera'].update(far_milli=1000))
    bad('coincident-camera',lambda s:s['geometry']['camera'].update(target_milli=vector([0,0,100000])))
    bad('parallel-up',lambda s:s['geometry']['camera'].update(up_milli=vector([0,0,1000])))
    bad('bad-fov',lambda s:s['geometry']['camera']['projection']['vertical_fov_mdeg'].update(value=180000))
    bad('bad-scale',lambda s:s['geometry']['nodes'][0]['transform']['scale_milli'].update(value=[0,1000,1000]))
    bad('empty-animation',lambda s:s['geometry']['nodes'][0]['transform']['position_milli'].update(animation={}))
    bad('zero-light-direction',lambda s:s['geometry'].update(lights=[{'kind':'directional','color':[255]*3,'intensity_milli':scalar(1000),'toward_light_milli':vector([0,0,0])}]))
    bad('point-distance',lambda s:s['geometry'].update(lights=[{'kind':'point','color':[255]*3,'intensity_milli':scalar(1000),'position_milli':vector([0,0,1]),'falloff':'inverse_square','reference_distance_milli':0}]))
    # Every plane is tested at every pixel and sample, active or not: 501 frames of 4000 x 2000 with 16 planes
    # is 64,128,000,000 visits, though the layers composite only 8,192,000,000 pixels in their 64 frames.
    bad('pixel-work',lambda s:s.update(width=4000,height=2000,duration=time(501,25)),'LIMIT_EXCEEDED',maximum)
    bad('record-work',lambda s:(s.update(duration=time(129,25)),[l.update(duration=time(129,25)) for l in s['layers']]),'LIMIT_EXCEEDED',records)
    for reverse in (False,True):
        def depth(s,reverse=reverse):
            s['geometry']['nodes'] += [node(f'd{i}',parent=f'd{i-1}' if i else None) for i in range(16)]
            s['geometry']['nodes'][0]['parent']='d15'
            if reverse:s['geometry']['nodes'].reverse()
        bad('depth-'+str(reverse),depth,'LIMIT_EXCEEDED')
    def precision(s):
        s['geometry']['nodes'] += [node(f'p{i}',parent=f'p{i-1}' if i else None,size=(100000,100000,100000)) for i in range(6)]
        s['geometry']['nodes'][0]['parent']='p5'
    bad('world-precision',precision,'GEOMETRY_PRECISION')
    for name,s,code in invalid:
        p=output/('invalid-'+name+'.mkv');error=call('scene.render',code,scene=s,input_root=str(sources),output_root=str(output),output=str(p));assert not p.exists()
        if name=='pixel-work':assert '64128000000 pixel-plane sample visits' in error['message'],error
    digest=hashlib.sha256((output/'perspective.mkv').read_bytes()).hexdigest()
    call('scene.render','OUTPUT_EXISTS',scene=scene,input_root=str(sources),output_root=str(output),output=str(output/'perspective.mkv'))
    assert hashlib.sha256((output/'perspective.mkv').read_bytes()).hexdigest()==digest
    changed=copy.deepcopy(scene)
    for l in changed['layers']:l['frames'][0]['image']['sha256']='0'*64
    call('scene.render','MEDIA_CHANGED',scene=changed,input_root=str(sources),output_root=str(output),output=str(output/'changed.mkv'))
    assert not (output/'changed.mkv').exists() and not list(output.glob('.cutbolt-*'))
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    passed.append('geometry.invalid_hierarchies_materials_shadows_and_publication_guards')
    result={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'previews':previews,'rejections':rejected,'cases':cases,
            'maximum_render_seconds':elapsed,'heavy_render_pixel_plane_sample_visits':16777216,'maximum_pixel_plane_sample_visits':64000000000,
            'maximum_node_sample_records':32768,'source_preserved':True,
            'reference':'60-digit forward vertex transforms, camera-space polygon clipping and projected barycentric texture/depth interpolation; independent point-light/alpha equations',
            'scope':'Textured planes, nearest held textures, bounded camera hierarchy and encoded-color lighting; no meshes or shadows'}
    (root/'scene.json').write_text(json.dumps(scene,indent=2)+'\n');(root/'verification.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
