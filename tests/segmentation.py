"""Original binary foreground fixtures; analytic labels and independent raster checks."""
from engine import ENGINE
import argparse
import budgets
import copy
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import time as clock
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time, expected_frame
from geometry import reference as geometry_reference, node, vector, scalar
from temporal import exposure
from fractions import Fraction as F

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def border(mask, width, height):
    return {(x, y) for y in range(height) for x in range(width) if mask[y*width+x]
            and any(not (0 <= a < width and 0 <= b < height) or not mask[b*width+a]
                    for a, b in [(x-1,y),(x+1,y),(x,y-1),(x,y+1)])}


def quality(actual, truth, width, height):
    intersection = sum(a and b for a, b in zip(actual, truth))
    union = sum(a or b for a, b in zip(actual, truth))
    left, right = border(actual,width,height), border(truth,width,height)
    return {'iou': intersection/union, 'boundary_f1': 2*len(left & right)/(len(left)+len(right)),
            'wrong_pixels': sum(a != b for a,b in zip(actual,truth))}


def integrate(scene, root, n):
    spec=scene['temporal']; count=spec['samples']; values=[]
    for k in range(count):
        t=F(n,25)+(F(spec['phase']['num'],spec['phase']['den'])+F(spec['shutter_angle']['num'],spec['shutter_angle']['den'])*F(2*k+1,2*count))/9000
        values.append(expected_frame(scene,root,t*25) if 0<=t<F(scene['duration']['num'],scene['duration']['den']) else bytes(scene['background'])*(scene['width']*scene['height']))
    return bytes((sum(items)+count//2)//count for items in zip(*values))


def run(root, python):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources = root/'sources'; sources.mkdir(); output = root/'output'; output.mkdir()
    for name in ('profile-a','profile-b','store','oracle'):
        (root/name).mkdir()
    original = {}; truths = []; images = []; annotations = []; frames = []
    width,height,count = 96,64,12
    for n in range(count):
        cx,cy = 28+2*n,32
        truth=[]; pixels=[]
        for y in range(height):
            for x in range(width):
                u,v=x-cx,y-cy
                fg = (u*u*81+v*v*196 <= 196*81 and not (-2<=u<=2 and -2<=v<=2)) or (10<=u<=22 and -1<=v<=1)
                # Texture and illumination vary independently of the authored silhouette.
                noise=(x*17+y*11+n*7)%19
                color=(174+noise,64+noise,35+noise) if fg else (18+noise,80+noise,142+noise)
                if 74<=x<=83 and 4<=y<=13: color=(181,71,42) # excluded foreground-colored distractor
                truth.append(fg);pixels.append(color)
        image=Image.new('RGB',(width,height));image.putdata(pixels);path=sources/f'frame-{n:02}.png';image.save(path)
        ann={'region':[cx-18,cy-13,44,27],'foreground':[[cx-8,cy-2,4,5]],'background':[[cx-2,cy-2,5,5]]}
        annotations.append(ann);images.append(image);truths.append(truth)
        frames.append({'source':identity(path,root),'hold':time(1,25),'annotations':ann})
    original={p.name:digest(p) for p in sources.iterdir()}
    recipe={'schema_version':1,'id':'moving-original-shape','automation':'annotated_frames','iterations':4,'seed':819,
            'frames':frames}
    passed=[]; rejected=0; rendered=0; samples=0; previews=0
    def helper(command,error=None,profile='profile-a',**fields):
        nonlocal rejected
        request={'command':command,'input_root':str(root),**fields}
        env={**os.environ,'USERPROFILE':str(root/profile),'HOME':str(root/profile),'XDG_CACHE_HOME':str(root/profile),
             'HF_HUB_OFFLINE':'1','TRANSFORMERS_OFFLINE':'1','HTTP_PROXY':'http://127.0.0.1:1','HTTPS_PROXY':'http://127.0.0.1:1',
             'NO_PROXY':'','PYTHONDONTWRITEBYTECODE':'1'}
        p=subprocess.run([str(python),'-I',str(ROOT/'tools/segmentation.py')],input=json.dumps(request).encode(),capture_output=True,env=env,timeout=180)
        data=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and data['error']['code']==error,(error,data,p.stderr.decode());rejected+=1;return data
        if p.returncode:(root/'failed-helper-request.json').write_text(json.dumps(request,indent=2))
        assert p.returncode==0 and data['ok'],(data,p.stderr.decode());return data['result']
    def segment(r,name,**kw):return helper('segment',recipe=r,output_root=str(output),output=str(output/name),**kw)
    def read_doc(receipt):return json.loads((root/receipt['document']['path']).read_text())
    def masks(doc):
        return [[v[0]==255 for v in Image.open(root/f['mask']['path']).getdata()] for f in doc['frames']]
    start=clock.monotonic();first=segment(recipe,'first');elapsed=clock.monotonic()-start
    budgets.check(elapsed<90,elapsed)
    doc=read_doc(first);actual=masks(doc);metrics=[quality(a,b,width,height) for a,b in zip(actual,truths)]
    assert min(m['iou'] for m in metrics)>=.98,metrics
    assert min(m['boundary_f1'] for m in metrics)>=.95,metrics
    temporal=[]
    for n in range(count-1):
        # Known authored translation, with all silhouette support away from frame edges.
        disagreement=sum(actual[n][y*width+x] != actual[n+1][y*width+x+2]
                         for y in range(height) for x in range(width-2))
        temporal.append(disagreement/((width-2)*height))
    assert max(temporal)<=.005,temporal
    passed.append('segmentation.authored_edge_and_motion_compensated_temporal_quality')

    second=segment(recipe,'second',profile='profile-b');again=read_doc(second)
    assert masks(again)==actual and again['provenance']==doc['provenance']
    for f,g in zip(doc['frames'],again['frames']):
        assert f['model']==g['model'] and f['mask']['sha256']==g['mask']['sha256']
        values=f['model']['background']+f['model']['foreground']
        assert f['model']['sha256']==hashlib.sha256(struct.pack('<130d',*values)).hexdigest()
        assert any(v != 0 for v in values)
    assert len({f['model']['sha256'] for f in doc['frames']})>=10
    assert not list((root/'profile-a').iterdir()) and not list((root/'profile-b').iterdir())
    inspected=helper('inspect',document=first['document']);assert inspected['runtime_matches_producer']
    gate="import runpy,socket; d=runpy.run_path("+repr(str(ROOT/'tools/segmentation.py'))+");\ntry: socket.socket()\nexcept d['Failure'] as e: assert e.code=='NETWORK_DISABLED'; print('network rejected')\nelse: raise AssertionError('socket permitted')"
    p=subprocess.run([str(python),'-I','-c',gate],capture_output=True,timeout=30);assert p.returncode==0 and b'network rejected' in p.stdout
    passed.append('segmentation.fresh_offline_workers_repeatability_and_fitted_model_identity')

    # Deliberately wrong hard foreground constraint creates a visible, editable error.
    wrong=copy.deepcopy(recipe);wrong['frames'][0]['annotations']['foreground'].append([12,21,3,3])
    bad=segment(wrong,'needs-correction');bad_doc=read_doc(bad)
    assert quality(masks(bad_doc)[0],truths[0],width,height)['wrong_pixels']>=9
    parent_hash=digest(root/bad['document']['path'])
    corrected=helper('correct',document=bad['document'],expected_revision=1,
                     corrections=[{'frame':0,'annotations':annotations[0]}],output_root=str(output),output=str(output/'corrected'))
    corrected_doc=read_doc(corrected)
    assert corrected_doc['revision']==2 and corrected_doc['parent']==bad['document'] and masks(corrected_doc)==actual
    assert digest(root/bad['document']['path'])==parent_hash
    helper('correct','REVISION_CONFLICT',document=corrected['document'],expected_revision=1,
           corrections=[{'frame':0,'annotations':annotations[0]}],output_root=str(output),output=str(output/'stale'))
    assert not (output/'stale').exists()
    passed.append('segmentation.versioned_corrections_parent_identity_and_source_preservation')

    scene=helper('scene',document=corrected['document'],scene_id='masked-sequence',background=[7,19,41])['scene']
    exe=ENGINE
    def call(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=240);r=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and r['error']['code']==error,(error,r);rejected+=1;return r
        assert p.returncode==0 and r['ok'],r;return r['result']
    def compare(path,wanted):
        nonlocal rendered,samples
        rgb=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=120)
        assert rgb==b''.join(wanted),(path, next(((i,a,b) for i,(a,b) in enumerate(zip(rgb,b''.join(wanted))) if a!=b),None))
        pcm=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),'-vn','-f','s16le','-'],timeout=120)
        assert pcm==bytes(len(wanted)*1920*4);rendered+=len(wanted);samples+=len(pcm)//4
    # Oracle images use independently read binary masks, leaving all transform math
    # to the pre-existing independent forward references rather than production.
    reference=copy.deepcopy(scene)
    for n,f in enumerate(reference['layers'][0]['frames']):
        image=images[n].convert('RGBA');image.putalpha(Image.new('L',(width,height)))
        image.putdata([(*color,255 if keep else 0) for color,keep in zip(images[n].getdata(),actual[n])])
        p=root/'oracle'/f'{n}.png';image.save(p);f['image']=identity(p,root);f.pop('matte')
    wanted=[expected_frame(reference,root,n) for n in range(count)]
    receipt=call('scene.render',scene=scene,input_root=str(root),output_root=str(output),output=str(output/'masked.mkv'))
    compare(output/'masked.mkv',wanted);assert receipt['frame_matte']['matted_pairs']==count
    # The same source can use different masks without shared-cache contamination.
    dual=copy.deepcopy(scene);dual['duration']=time(2,25);dual['layers'][0]['duration']=time(2,25);dual['layers'][0]['frames']=dual['layers'][0]['frames'][:2]
    dual['layers'][0]['frames'][1]['image']=dual['layers'][0]['frames'][0]['image']
    refdual=copy.deepcopy(reference);refdual['duration']=time(2,25);refdual['layers'][0]['duration']=time(2,25);refdual['layers'][0]['frames']=refdual['layers'][0]['frames'][:2]
    shared=images[0].convert('RGBA');shared.putdata([(*c,255 if keep else 0) for c,keep in zip(images[0].getdata(),actual[1])]);shared.save(root/'oracle/shared.png');refdual['layers'][0]['frames'][1]['image']=identity(root/'oracle/shared.png',root)
    call('scene.render',scene=dual,input_root=str(root),output_root=str(output),output=str(output/'shared.mkv'));compare(output/'shared.mkv',[expected_frame(refdual,root,n) for n in range(2)])
    for name,s,r,oracle in [('crop',copy.deepcopy(scene),copy.deepcopy(reference),expected_frame),
                          ('temporal',copy.deepcopy(scene),copy.deepcopy(reference),integrate)]:
        if name=='crop':
            for item in (s,r):item['layers'][0]['transform'].update(position=[3,2],crop=[8,9,70,45],opacity=173)
        else:
            for item in (s,r):item['temporal']=exposure(4,360,-180)
        call('scene.render',scene=s,input_root=str(root),output_root=str(output),output=str(output/(name+'.mkv')))
        compare(output/(name+'.mkv'),[oracle(r,root,n) for n in range(count)])
    geo={'nodes':[node('foreground-plane','foreground',extent=(96000,64000))],
         'camera':{'position_milli':vector([0,0,100000]),'target_milli':vector([0,0,0]),'up_milli':vector([0,1000,0]),
                   'projection':{'kind':'orthographic','vertical_size_milli':scalar(64000)},'near_milli':1000,'far_milli':200000},
         'lights':[],'shadows':'none'}
    s=copy.deepcopy(scene);s['geometry']=geo;r=copy.deepcopy(reference);r['geometry']=geo
    call('scene.render',scene=s,input_root=str(root),output_root=str(output),output=str(output/'geometry.mkv'));compare(output/'geometry.mkv',[geometry_reference(r,root,n) for n in range(count)])
    passed.append('segmentation.native_held_mattes_crop_temporal_and_geometry_pixels')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65
        schema=next(t for t in catalog if t['name']=='cutbolt_scene_inspect')['inputSchema']
        Draft202012Validator(schema).validate({'scene':scene,'input_root':str(root)})
        assert client.call('scene.inspect',scene=scene,input_root=str(root))['frame_matte']['matted_pairs']==count
        p=client.call('project.create',id='segmentation-edit',width=width,height=height,frame_rate=time(25));common={'store_root':str(root/'store'),'project_id':p['id']}
        client.call('session.create',store_root=str(root/'store'),project=p,request_id='create')
        ops=[{'op':'media.add','asset':receipt['asset']},{'op':'clip.append','clip':{'id':'last','asset_id':receipt['asset']['id'],'source_in':time(6,25),'duration':time(6,25)}},
             {'op':'clip.append','clip':{'id':'first','asset_id':receipt['asset']['id'],'source_in':time(0),'duration':time(6,25)}}]
        req={**common,'expected_revision':0,'operations':ops,'request_id':'append'};result=client.call('session.apply',**req);assert client.call('session.apply',**req)==result
        saved=client.call('session.get',**common);fields={'project':saved,'input_root':str(output),'output_root':str(output)}
        call('render.run',**fields,output=str(output/'saved.mkv'));compare(output/'saved.mkv',wanted[6:]+wanted[:6])
        for n,source in [(0,6),(5,11),(6,0),(11,5)]:
            p=output/f'preview-{n}.png';client.call('preview.frame',**fields,output=str(p),time=time(n,25));assert Image.open(p).tobytes()==wanted[source];previews+=1
        call('preview.range',**fields,output=str(output/'range.mkv'),start=time(5,25),duration=time(2,25));compare(output/'range.mkv',[wanted[11],wanted[0]])
        client.call('session.undo',**common,expected_revision=saved['revision'],request_id='undo');assert client.call('session.get',**common)['clips']==[]
    finally:client.close()
    passed.append('segmentation.typed_inspection_saved_edits_retry_undo_and_previews')

    invalid=[]
    def bad(name,change,code):
        r=copy.deepcopy(recipe);change(r);invalid.append((name,r,code))
    bad('automation',lambda r:r.update(automation='track_automatically'),'UNSUPPORTED_AUTOMATION')
    bad('unknown',lambda r:r.update(model_url='unused'),'INVALID_REQUEST')
    bad('empty',lambda r:r.update(frames=[]),'LIMIT_EXCEEDED')
    bad('many',lambda r:r.update(frames=r['frames']*6),'LIMIT_EXCEEDED')
    bad('seed',lambda r:r.update(seed=-1),'INVALID_REQUEST')
    bad('iterations',lambda r:r.update(iterations=11),'INVALID_REQUEST')
    bad('time',lambda r:r['frames'][0].update(hold=time(1,100)),'UNALIGNED_TIME')
    bad('duration',lambda r:r['frames'][0].update(hold=time(10)),'LIMIT_EXCEEDED')
    bad('conflict',lambda r:r['frames'][0]['annotations']['background'].append([20,30,4,5]),'INVALID_ANNOTATION')
    bad('no-fg',lambda r:r['frames'][0]['annotations'].update(foreground=[]),'INVALID_ANNOTATION')
    bad('outside',lambda r:r['frames'][0]['annotations'].update(region=[0,0,1,1]),'INVALID_ANNOTATION')
    bad('rect',lambda r:r['frames'][0]['annotations'].update(region=[0,0,97,64]),'INVALID_REQUEST')
    bad('many-rects',lambda r:r['frames'][0]['annotations'].update(foreground=[[20,30,4,5]]*257),'LIMIT_EXCEEDED')
    bad('changed',lambda r:r['frames'][0]['source'].update(sha256='0'*64),'MEDIA_CHANGED')
    bad('traversal',lambda r:r['frames'][0]['source'].update(path='../outside.png'),'INVALID_PATH')
    for name,r,code in invalid:
        segment(r,'reject-'+name,error=code);assert not (output/('reject-'+name)).exists()
    segment(recipe,'first',error='OUTPUT_EXISTS')
    # Actual maximum frame/pixel workload, not a hypothetical size calculation.
    maximum_image=Image.new('RGB',(256,256));maximum_image.putdata([(190+(x*7+y)%20,60,40) if 64<=x<192 and 64<=y<192 else (20,90+(x+y)%20,150) for y in range(256) for x in range(256)])
    maximum_path=sources/'maximum.png';maximum_image.save(maximum_path)
    maximum={'schema_version':1,'id':'maximum-masks','automation':'annotated_frames','iterations':1,'seed':21,
             'frames':[{'source':identity(maximum_path,root),'hold':time(1,25),
                        'annotations':{'region':[48,48,160,160],'foreground':[[90,90,20,20]],'background':[]}} for _ in range(64)]}
    start=clock.monotonic();maximum_result=segment(maximum,'maximum');maximum_seconds=clock.monotonic()-start
    budgets.check(maximum_seconds<120,maximum_seconds)
    maximum_doc=read_doc(maximum_result);maximum_truth=[64<=x<192 and 64<=y<192 for y in range(256) for x in range(256)]
    assert all(mask==maximum_truth for mask in masks(maximum_doc))
    # Cross the aggregate pixel limit while each individual image is valid.
    wider=sources/'wider.png';Image.new('RGB',(257,256),(20,90,150)).save(wider)
    over=copy.deepcopy(maximum)
    for f in over['frames']:f['source']=identity(wider,root)
    segment(over,'reject-pixels',error='LIMIT_EXCEEDED');assert not (output/'reject-pixels').exists()
    maximum_path.unlink();wider.unlink()
    passed.append('segmentation.maximum_frame_and_pixel_workload')
    # Validate edited model arrays independently of document identity.
    altered=copy.deepcopy(doc);altered['frames'][0]['model']['background'][0]+=1
    p=output/'altered.json';p.write_text(json.dumps(altered));helper('inspect','INVALID_MODEL',document=identity(p,root))
    p=output/'unknown-doc.json';altered=copy.deepcopy(doc);altered['extra']=1;p.write_text(json.dumps(altered));helper('inspect','INVALID_REQUEST',document=identity(p,root))
    for name,mode,size,color in [('gray','RGB',(width,height),(127,127,127)),('colored','RGB',(width,height),(255,0,0)),
                                  ('alpha','RGBA',(width,height),(255,255,255,127)),('size','RGB',(8,8),(255,255,255))]:
        p=sources/(name+'.png');Image.new(mode,size,color).save(p)
        bad_scene=copy.deepcopy(scene);bad_scene['layers'][0]['frames'][0]['matte']=identity(p,root)
        call('scene.render','INVALID_MATTE',scene=bad_scene,input_root=str(root),output_root=str(output),output=str(output/('bad-'+name+'.mkv')))
        assert not (output/('bad-'+name+'.mkv')).exists()
        p.unlink()
    bad_scene=copy.deepcopy(scene);bad_scene['layers'][0]['frames'][0]['matte']['sha256']='0'*64
    call('scene.render','MEDIA_CHANGED',scene=bad_scene,input_root=str(root),output_root=str(output),output=str(output/'bad-identity.mkv'))
    output_hash=digest(output/'masked.mkv');call('scene.render','OUTPUT_EXISTS',scene=scene,input_root=str(root),output_root=str(output),output=str(output/'masked.mkv'));assert digest(output/'masked.mkv')==output_hash
    assert original=={p.name:digest(p) for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-*'))
    passed.append('segmentation.unsupported_automation_limits_identity_and_publication_guards')
    result={'passed':passed,'quality':metrics,'motion_compensated_disagreement':temporal,'source_preserved':True,
            'fresh_worker_seconds':elapsed,'frames_compared':rendered,'stereo_sample_frames_compared':samples,'previews':previews,
            'rejections':rejected,'maximum_worker_seconds':maximum_seconds,'maximum_frames':64,'maximum_pixels':4194304,'provenance':doc['provenance'],'model_identities':[f['model']['sha256'] for f in doc['frames']],
            'scope':'Annotated opaque RGB8 frames, binary masks, original synthetic silhouettes; no natural-footage or automatic tracking claim'}
    (root/'verification.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--runtime-python',type=Path,required=True)
    args=parser.parse_args();run(args.output,args.runtime_python)
