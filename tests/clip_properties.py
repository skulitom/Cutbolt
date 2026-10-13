"""Saved spatial properties; called with native_layers' original synthetic footage."""
import copy
import json
import subprocess
from fractions import Fraction as F

from PIL import Image
from engine import ENGINE
from keying import key
from scenes import identity
from tracks import edit, placement, time, track
from sequences import nested, seqedit


def curve(entries):
    return {'keys':[{'time':time(F(at).numerator,F(at).denominator),'value':value,'interpolation':mode} for at,value,mode in entries]}


def check(root, sources, outputs, pictures, raw, call, apply, project, render):
    w, h, count = 32, 24, 12
    stride = w*h*3
    identity_transform = {'position':[0,0], 'crop':[0,0,w,h], 'scale':1, 'quarter_turns':0, 'opacity':255}
    end = F(count,25)
    opacity = {'kind':'opacity','value':199,'curve':curve([(0,31,'linear'),(end,241,'hold')])}
    reveal = {'kind':'mask','mask':{'rect':[3,2,20,18], 'inverted':False, 'feather':{'radius':3,'edge':'centered'},
              'animation':{'x':curve([(0,-3,'linear'),(end,13,'hold')])}}}
    pip = {'kind':'transform','value':{**identity_transform,'position':[24,3],'crop':[2,2,16,12],'quarter_turns':1},'anchor':[2,2]}
    spatial = {'kind':'transform','value':{**identity_transform, 'position':[4,3], 'spatial':{
        'translate_milli':[350,-250],'scale_milli':[650,700],'rotation_mdeg':7000,'flip':[False,True],
        'pixel_aspect':time(1),'sampling':'bilinear','edge':'transparent',
        'animation':{'translate_x_milli':curve([(0,-500,'linear'),(end,2500,'hold')])}}},
        'animation':{'position_y':curve([(0,7,'linear'),(end,19,'hold')])}, 'anchor':[4,2]}
    stacks = {'pip':[pip,{'kind':'opacity','value':173}],
              'spatial':[spatial,opacity,reveal,{'kind':'blend_mode','mode':'screen'}],
              'reveal':[reveal,opacity,{'kind':'blend_mode','mode':'multiply'}],
              'keyed-pip':[key(strength_milli=500),pip,opacity]}

    def decode(path):
        return subprocess.run(['ffmpeg','-v','error','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],
                              check=True,capture_output=True,timeout=180).stdout

    def recipe(codec, stack, name, frames=None):
        scene = {'schema_version':1,'id':name,'width':w,'height':h,'output_scale':1,'duration':time(count,25),
                 'background':[0,0,0],'color':'srgb_straight_encoded','audio':None,'layers':[]}
        for source in ('background','presenter'):
            layer = {'id':source,'canvas':[w,h],'start':time(0),'duration':time(count,25),
                     'frames':copy.deepcopy((frames or pictures)[codec,source]),'timing':'strict','end':'hold_last',
                     'effects':[], 'transform':copy.deepcopy(identity_transform)}
            if source == 'presenter':
                for effect in stack:
                    kind=effect['kind']
                    if kind=='transform':
                        layer['transform']=copy.deepcopy(effect['value'])
                        if 'animation' in effect: layer['animation']=copy.deepcopy(effect['animation'])
                        for frame in layer['frames']: frame['anchor']=effect.get('anchor',[0,0])
                    elif kind=='opacity':
                        layer['transform']['opacity']=effect['value']
                        if 'curve' in effect: layer.setdefault('animation',{})['opacity']=effect['curve']
                    elif kind=='mask': layer['mask']=effect['mask']
                    elif kind=='blend_mode': layer['blend_mode']=effect['mode']
                    else: layer['effects'].append(effect)
            scene['layers'].append(layer)
        return scene

    def oracle(codec, stack, name, frames=None):
        value=recipe(codec,stack,name,frames)
        file=outputs/(name+'-scene.mkv')
        call('scene.render',scene=value,input_root=str(sources),output_root=str(outputs),output=str(file))
        return decode(file)

    references={}
    for codec in ('ffv1','phone'):
        base=project(codec)
        for name,stack in stacks.items():
            p=apply(base,[edit('clip_effects',clip_id='presenter',effects=stack)])
            expected=oracle(codec,stack,codec+'-props-'+name)
            references[codec,name]=expected
            render(p,codec+'-props-'+name,expected)
            plan=call('render.plan',project=p,input_root=str(sources),output_root=str(outputs),output=str(outputs/(codec+'-'+name+'-plan.mkv')))
            assert [c['clip_id'] for c in plan['compositor']['stack']['clips']]==['background','presenter']
        # List position does not change a structural property's phase; pixel order stays separate.
        reordered=apply(base,[edit('clip_effects',clip_id='presenter',effects=list(reversed(stacks['spatial'])))])
        render(reordered,codec+'-props-reordered',references[codec,'spatial'])
        transition=apply(base,[edit('remove',clip_ids=['presenter'],links='include'),
            edit('place',track_id='presenter',clip=placement('left','presenter',0,6),collision='reject'),
            edit('place',track_id='presenter',clip=placement('right','presenter',6,6,6),collision='reject'),
            edit('clip_effects',clip_id='left',effects=stacks['pip']),
            edit('clip_effects',clip_id='right',effects=stacks['spatial']),
            edit('transition_set',track_id='presenter',transition={'id':'blend','left_id':'left','right_id':'right','kind':'dissolve','before':time(2,25),'after':time(2,25)})])
        left,right=references[codec,'pip'],references[codec,'spatial']
        mixed=bytearray(left[:4*stride])
        for n in range(4,8):
            k=2*(n-4)+1
            mixed.extend((a*(8-k)+b*k+4)//8 for a,b in zip(left[n*stride:(n+1)*stride],right[n*stride:(n+1)*stride]))
        mixed.extend(right[8*stride:])
        render(transition,codec+'-props-transition',bytes(mixed))
        # Exact source clock survives a trim, slip and split; all pixels below have matching source time.
        p=apply(base,[edit('remove',clip_ids=['background','presenter'],links='include'),
                     edit('duration',duration=time(6,25)),
                     *[edit('place',track_id=s,clip=placement(s,s,0,6,2),collision='reject') for s in ('background','presenter')],
                     edit('clip_effects',clip_id='presenter',effects=stacks['spatial'])])
        render(p,codec+'-props-trim',references[codec,'spatial'][2*stride:8*stride])
        p=apply(p,[edit('slip',clip_ids=['background','presenter'],shift={'backward':False,'amount':time(1,25)},links='include'),
                   edit('split',clip_ids=['presenter'],at=time(3,25),links='include',right_clip_ids=[{'id':'presenter','new_id':'right'}],right_link_ids=[])])
        expected=references[codec,'spatial'][3*stride:9*stride]
        render(p,codec+'-props-slip-split',expected)
        preview=outputs/(codec+'-props-preview.png')
        call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(2,25))
        assert Image.open(preview).convert('RGB').tobytes()==expected[2*stride:3*stride]
        ranged=outputs/(codec+'-props-range.mkv')
        call('export.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(ranged),profile='reference',streams='video',range={'start':time(2,25),'duration':time(3,25)})
        assert decode(ranged)==expected[2*stride:5*stride]
        # The same properties apply at the child-definition and sequence-instance boundaries.
        for boundary in ('child','instance'):
            ops=[edit('remove',clip_ids=['presenter'],links='include'),{'op':'sequence.create','id':'shot','duration':time(count,25)},
                 seqedit('shot',edit('add',track=track('v','video'))),
                 seqedit('shot',edit('place',track_id='v',clip=placement('c','presenter',0,count),collision='reject')),
                 edit('place',track_id='presenter',clip=nested('instance','shot',0,count),collision='reject')]
            if boundary=='instance':
                ops.append(edit('clip_effects',clip_id='instance',effects=stacks['spatial']))
                wanted=references[codec,'spatial']
            else:
                # Child results are opaque RGB8 pictures: compose against black inside the child.
                ops.append(seqedit('shot',edit('clip_effects',clip_id='c',effects=stacks['spatial'])))
                child_recipe=recipe(codec,stacks['spatial'],codec+'-child-props')
                child_recipe['layers']=child_recipe['layers'][1:]
                file=outputs/(codec+'-child-props-scene.mkv')
                call('scene.render',scene=child_recipe,input_root=str(sources),output_root=str(outputs),output=str(file))
                wanted=decode(file)
            render(apply(base,ops),codec+'-props-nested-'+boundary,wanted)

        # Legacy overlay is actual saved-property sugar, including crop-origin anchor and rounding.
        old={'crop':[2,2,24,16],'divisor':2,'opacity':137,'position':[-2,5]}
        sugar=[{'kind':'transform','value':{**identity_transform,'position':old['position'],'crop':old['crop']},
                'anchor':[2,2],'sampling':{'filter':'box_floor','divisor':2}},
               {'kind':'opacity','value':137,'rounding':'byte_alpha'}]
        legacy=copy.deepcopy(base)
        legacy['tracks']['tracks'][1]['composite']='alpha_over'
        legacy=apply(legacy,[edit('clip_transform',clip_ids=['presenter'],transform=old)])
        file=outputs/(codec+'-props-legacy.mkv')
        call('render.run',project=legacy,input_root=str(sources),output_root=str(outputs),output=str(file))
        render(apply(base,[edit('clip_effects',clip_id='presenter',effects=sugar)]),codec+'-props-sugar',decode(file))

    # Independent integer opacity and all three blend equations on a known RGB canvas.
    for mode in ('normal','multiply','screen'):
        value=137
        p=apply(project('ffv1'),[edit('clip_effects',clip_id='presenter',effects=[{'kind':'opacity','value':value},{'kind':'blend_mode','mode':mode}])])
        expected=bytearray()
        for s,d in zip(raw['presenter'],raw['background']):
            blend=F(s,255) if mode=='normal' else F(s*d,255**2) if mode=='multiply' else 1-F((255-s)*(255-d),255**2)
            out=blend*value+F(d*(255-value),255)
            expected.append(int(out+F(1,2)))
        render(p,'props-integer-'+mode,bytes(expected))

    # Proxy geometry uses original coordinates; compare with block-expanded proxy source images.
    p=apply(project('ffv1'),[edit('clip_effects',clip_id='presenter',effects=stacks['spatial'])])
    proxy_pictures={}
    for source in ('background','presenter'):
        file=sources/(source+'-props-proxy.mkv')
        generated=call('proxy.generate',project=p,expected_revision=p['revision'],asset_id=source,scale=2,input_root=str(sources),output_root=str(sources),output=str(file))
        p=apply(p,generated['operations'])
        pixels=decode(file); frames=[]
        for n in range(count):
            image=sources/f'{source}-props-proxy-{n}.png'
            Image.frombytes('RGB',(w//2,h//2),pixels[n*stride//4:(n+1)*stride//4]).resize((w,h),Image.Resampling.NEAREST).save(image)
            frames.append({'image':identity(image,sources),'hold':time(1,25),'offset':[0,0],'anchor':[0,0]})
        proxy_pictures['ffv1',source]=frames
    expected=oracle('ffv1',stacks['spatial'],'props-proxy',proxy_pictures)
    p=apply(p,[{'op':'preview.proxy','scale':2}])
    preview=outputs/'props-proxy.png'
    call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(5,25))
    wanted=bytes(expected[5*stride+((2*y+1)*w+2*x+1)*3+c] for y in range(h//2) for x in range(w//2) for c in range(3))
    assert Image.open(preview).convert('RGB').tobytes()==wanted
    render(p,'props-proxy-final-originals',references['ffv1','spatial'])

    job_root=root/'property-jobs'; job_root.mkdir()
    file=outputs/'props-queued.mkv'
    p=apply(project('phone'),[edit('clip_effects',clip_id='presenter',effects=stacks['spatial'])])
    ticket=call('render.start',job_root=str(job_root),request_id='properties',render={'project':p,'input_root':str(sources),'output_root':str(outputs),'output':str(file)})
    state=call('job.wait',job_root=str(job_root),job_id=ticket['job_id'],timeout_seconds=120)
    assert state['status']=='completed',state
    assert decode(file)==references['phone','spatial']

    # Invalid owners/bounds and unknown fields must reject the edit, not silently accept a no-op.
    base=project('ffv1')
    for stack in ([opacity,opacity], [pip,pip], [reveal,reveal], [{'kind':'blend_mode','mode':'screen'}]*2,
                  [{'kind':'transform','value':{**identity_transform,'opacity':100}},opacity],
                  [{'kind':'transform','value':{**identity_transform,'crop':[0,0,w+1,h]}}],
                  [{'kind':'transform','value':identity_transform,'anchor':[32769,0]}],
                  [{'kind':'transform','value':identity_transform,'sampling':{'filter':'box_floor','divisor':0}}],
                  [{'kind':'opacity','value':255,'typo':True}]):
        request={'command':'timeline.apply','project':base,'expected_revision':base['revision'],
                 'operations':[edit('clip_effects',clip_id='presenter',effects=stack)]}
        result=subprocess.run([str(ENGINE)],input=json.dumps(request).encode(),capture_output=True,timeout=60)
        assert not json.loads(result.stdout)['ok'],stack
    return ['native_layers.saved_spatial_properties_match_scene_on_direct_and_reference',
            'native_layers.spatial_source_clock_trim_slip_split_and_nested',
            'native_layers.spatial_preview_range_proxy_and_queue',
            'native_layers.overlay_is_exact_property_sugar',
            'native_layers.independent_integer_opacity_blends',
            'native_layers.property_owners_and_bounds_rejected']
