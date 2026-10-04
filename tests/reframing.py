"""Original subject trajectories, enumerated crop feasibility and decoded scene references."""
from engine import ENGINE, MCP_TOOLS
import argparse
import budgets
import array
import copy
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import threading
import time as clock
import wave
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from animation import sample
from grading import grade
from registry import peak_memory
from scenes import identity, selected, time
from spatial import reference
from tracking import coverage

ROOT=Path(__file__).resolve().parents[1]
EXE=ENGINE


def nearest(value):
    return (1 if value>=0 else -1)*int(abs(value)+F(1,2))


def expected_paths(request, boxes, cuts):
    """Enumerate geometric candidate positions and permissible edges, not interval algebra."""
    canvas=request['scene']['layers'][0]['canvas'];window=request['window'];padding=request['padding']
    count=len(boxes);positions=[[0,0] for _ in boxes];targets=[[0,0] for _ in boxes]
    for start,end in zip(cuts,cuts[1:]+[count]):
        for axis in range(2):
            maximum=request['maximum_step'][axis];radius=request['smoothing_radius']
            candidates=[];centers=[]
            for box in boxes[start:end]:
                a,b=box[axis]-padding,box[axis]+box[axis+2]+padding
                candidates.append({p for p in range(canvas[axis]-window[axis]+1) if p<=a and b<=p+window[axis]})
                centers.append(F(a+b-window[axis],2))
            reachable=[set() for _ in candidates];reachable[-1]=candidates[-1]
            for n in range(len(candidates)-2,-1,-1):
                reachable[n]={p for p in candidates[n] if any(q in reachable[n+1] for q in range(max(0,p-maximum),min(canvas[axis]-window[axis],p+maximum)+1))}
            assert reachable[0],('infeasible independent crop graph',start,axis)
            previous=None
            for n in range(len(candidates)):
                weights=[(j,radius+1-abs(j-n)) for j in range(max(0,n-radius),min(len(candidates),n+radius+1))]
                target=nearest(sum((centers[j]*w for j,w in weights),F(0))/sum(w for _,w in weights))
                choices=[p for p in reachable[n] if previous is None or abs(p-previous)<=maximum]
                chosen=min(choices,key=lambda p:(abs(p-target),p))
                positions[start+n][axis]=chosen;targets[start+n][axis]=target;previous=chosen
    return positions,targets


def curve(values):
    return {'keys':[{'time':time(n,25),'value':v,'interpolation':'hold'} for n,v in enumerate(values)]}


def segment(boxes, start=0, subject='subject', cut=True):
    return {'start':time(start,25),'subject_id':subject,'cut':cut,'selection':{'mode':'manual','keys':[{'time':time(n,25),'rect':b,'interpolation':'hold'} for n,b in enumerate(boxes)]}}


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store=[root/name for name in ('sources','outputs','store')]
    for p in (sources,out,store):p.mkdir()
    hashes={};passed=[];cases=[];rejected=0;frames=0;samples=0;previews=0
    def remember(path):hashes[path]=hashlib.sha256(path.read_bytes()).hexdigest();return identity(path,sources)
    def png(name,image):path=sources/(name+'.png');image.save(path);return remember(path)
    def call(req,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(EXE)],input=json.dumps(req).encode(),capture_output=True,timeout=180,env=env)
        response=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and not response['ok'] and response['error']['code']==error,(error,response)
            rejected+=1;return response
        if not response.get('ok'):(root/'failed-request.json').write_text(json.dumps(req,indent=2),encoding='utf-8')
        assert p.returncode==0 and response['ok'],response;return response['result']
    def decode(path,audio=False):
        return subprocess.check_output(['ffmpeg','-v','error','-i',str(path),*(['-vn','-f','s16le'] if audio else ['-an','-pix_fmt','rgb24','-f','rawvideo']),'-'],timeout=120)
    def compare(path,expected,pcm,tolerance=1):
        nonlocal frames,samples
        wanted=b''.join(expected);actual=decode(path)
        assert len(actual)==len(wanted)
        delta=max((abs(a-b) for a,b in zip(actual,wanted)),default=0)
        assert delta<=tolerance,(path.name,delta)
        assert decode(path,True)==pcm,path.name
        frames+=len(expected);samples+=len(pcm)//4
        return delta
    def base(images, n=16, canvas=(72,54)):
        layer={'id':'picture','canvas':list(canvas),'start':time(0),'duration':time(n,25),
               'frames':[{'image':i,'hold':time(3,25),'offset':[0,0],'anchor':[0,0]} for i in images],
               'timing':'strict','end':'loop','transform':{'position':[0,0],'crop':[0,0,*canvas],'scale':1,'quarter_turns':0,'opacity':255}}
        return {'schema_version':1,'id':'reframed','width':canvas[0],'height':canvas[1],'output_scale':1,'duration':time(n,25),'background':[13,17,23],'color':'srgb_straight_encoded','layers':[layer],'audio':None}
    charts=[]
    for k in range(2):
        image=Image.new('RGBA',(72,54))
        image.putdata([((31*x+17*y+k*13)%256,(19*x+29*y+k*27)%256,(5*x+47*y+k*61)%256,[0,85,170,255][(x+y+k)%4]) for y in range(54) for x in range(72)])
        charts.append(png(f'original-chart-{k}',image))
    def request(boxes,window=(18,32),target=(27,48),**fields):
        r={'command':'reframe.inspect','scene':base(charts,len(boxes)),'input_root':str(sources),'window':list(window),'output_size':list(target),
           'padding':1,'maximum_step':[3,3],'smoothing_radius':3,'sampling':'nearest','segments':[segment(boxes)]}
        r.update(fields);return r
    def check(req,boxes,name,cuts=None,pcm=None):
        req=copy.deepcopy(req);req['scene']['id']=name
        cuts=[0] if cuts is None else cuts
        expected_origins,targets=expected_paths(req,boxes,cuts)
        (root/(name+'-request.json')).write_text(json.dumps(req,indent=2),encoding='utf-8')
        result=call(req);decisions=result['decisions'];n=len(boxes)
        assert len(decisions)==n and not result['applied']
        assert result['sampling']==req['sampling'] and result['crop_edge_filtering']=='neighbouring_source_taps' and result['source_canvas_edge']=='transparent'
        assert F(result['scale']['num'],result['scale']['den'])==F(req['output_size'][0],req['window'][0])
        original=req['scene']['layers'][0];oracle=copy.deepcopy(req['scene'])
        oracle['width'],oracle['height']=req['output_size'];l=oracle['layers'][0];l['frames']=[];l['timing']='strict';l['end']='hold_last'
        l['transform']['spatial']={'translate_milli':[0,0],'scale_milli':[1000,1000],'rotation_mdeg':0,'flip':[False,False],
                                  'pixel_aspect':time(1),'sampling':req['sampling'],'edge':'transparent',
                                  'fit':{'size':req['output_size'],'mode':'cover','window_size':req['window']}}
        segments=sorted(req['segments'],key=lambda s:F(s['start']['num'],s['start']['den']))
        starts=[int(F(s['start']['num'],s['start']['den'])*25) for s in segments]
        for j,(box,p,d) in enumerate(zip(boxes,expected_origins,decisions)):
            source_index=selected(original,j);assert source_index is not None
            frame=copy.deepcopy(original['frames'][source_index]);frame['hold']=time(1,25);frame['anchor']=p;l['frames'].append(frame)
            segment_index=max(i for i,v in enumerate(starts) if v<=j)
            assert d['time']==time(j,25) and d['segment_time']==time(j-starts[segment_index],25)
            assert d['segment']==segment_index and d['subject_id']==segments[segment_index]['subject_id']
            assert d['source_frame']==source_index and d['focus']==box and d['crop']==p+req['window']
            assert d['cut']==(j in cuts) and d['smoothed_origin']==targets[j]
            assert d['constraint_adjustment']==[p[a]-targets[j][a] for a in range(2)]
            assert d['step']==([0,0] if j in cuts else [p[a]-expected_origins[j-1][a] for a in range(2)])
            assert result['scene']['layers'][0]['frames'][j]==frame
            for a in range(2):
                assert 0<=p[a]<=original['canvas'][a]-req['window'][a]
                assert p[a]<=box[a]-req['padding'] and box[a]+box[a+2]+req['padding']<=p[a]+req['window'][a]
                assert abs(d['step'][a])<=req['maximum_step'][a]
        for key in ('audio','audio_mix'):
            assert result['scene'].get(key)==req['scene'].get(key)
        for key in ('animation','mask','effects','alpha_mode','blend_mode'):
            default={'effects':[],'alpha_mode':'straight','blend_mode':'normal'}.get(key)
            assert result['scene']['layers'][0].get(key,default)==original.get(key,default)
        expected=[reference(oracle,sources,j,coverage) for j in range(n)]
        pcm=bytes(n*1920*4) if pcm is None else pcm
        output=out/(name+'.mkv')
        receipt=call({'command':'scene.render','scene':result['scene'],'input_root':str(sources),'output_root':str(out),'output':str(output)})
        delta=compare(output,expected,pcm)
        cases.append({'name':name,'frames':n,'maximum_rgb_error':delta,'cuts':len(cuts)})
        assert all(hashlib.sha256(p.read_bytes()).hexdigest()==h for p,h in hashes.items())
        print(json.dumps(cases[-1]),flush=True)
        return req,result,receipt,expected,pcm

    cap=call({'command':'capabilities'})
    assert 'reframe.inspect' in cap['commands'] and cap['reframing']['maximum_frames']==128
    boxes=[[16+2*n,23+[0,2,0,-2][n%4],6,5] for n in range(16)]
    first=None
    for i,(window,target) in enumerate([((18,32),(27,48)),((40,20),(60,30)),((32,32),(48,48)),((22,34),(22,34)),((36,24),(24,16)),((16,18),(32,36))]):
        req=request(boxes,window,target,sampling='nearest' if i%2==0 else 'bilinear');req['scene']['output_scale']=1+i%2
        result=check(req,boxes,f'geometry-{i}')
        if first is None:first=result
    for i,(window,target,origin) in enumerate([([72,54],[144,108],[0,0]),([72,54],[48,36],[0,0]),([18,32],[27,48],[0,0]),([18,32],[27,48],[54,22]),([11,17],[33,51],[61,37]),([17,31],[34,62],[0,23])]):
        b=[origin+window];check(request(b,window,target,padding=0,sampling='bilinear'),b,f'edge-{i}')
    passed.append('reframe.explicit_geometry_aspect_edges_and_alpha')

    keys=[{'time':time(0),'rect':[16,20,6,5],'interpolation':'ease_in_out'},
          {'time':time(15,25),'rect':[46,26,11,9],'interpolation':'hold'}]
    changing=[[sample({'keys':[{'time':k['time'],'value':k['rect'][axis],'interpolation':k['interpolation']} for k in keys]},F(n,25)) for axis in range(4)] for n in range(16)]
    req=request(changing,maximum_step=[5,5]);req['segments'][0]['selection']['keys']=keys
    authored=check(req,changing,'authored-size-and-easing')
    shuffled=copy.deepcopy(authored[0]);shuffled['segments'][0]['selection']['keys'].reverse()
    assert call(shuffled)==authored[1]
    trimmed=[]
    for i in range(2):
        with Image.open(sources/charts[i]['path']) as image:trimmed.append(png(f'trimmed-{i}',image.crop((0,0,60,42))))
    for end in ('loop','hold_last'):
        req=request(boxes,sampling='bilinear');l=req['scene']['layers'][0];l['timing']='sample_start';l['end']=end
        l['frames']=[{'image':trimmed[i%2],'hold':hold,'offset':[6,5],'anchor':[0,0]} for i,hold in enumerate([time(1,50),time(1,25),time(3,50)])]
        check(req,boxes,'fractional-source-holds-'+end)
    passed.append('reframe.authored_paths_source_clocks_and_corrections')

    # Different selected subjects can jump at an explicit cut or share one feasible pan.
    swap_boxes=[[4,20,8,8]]*8+[[60,20,8,8]]*8
    req=request(swap_boxes);req['segments']=[segment(swap_boxes[:8],subject='left'),segment(swap_boxes[8:],8,'right')]
    swapped=check(req,swap_boxes,'subject-cut',[0,8]);assert swapped[1]['decisions'][8]['step']==[0,0]
    impossible=copy.deepcopy(req);impossible['segments'][1]['cut']=False;call(impossible,'REFRAME_INFEASIBLE')
    continuous_boxes=[[20+n,20,8,8] for n in range(16)]
    req=request(continuous_boxes,(32,32),(48,48),sampling='bilinear')
    req['segments']=[segment(continuous_boxes[:8],subject='speaker-left'),segment(continuous_boxes[8:],8,'speaker-right',False)]
    continuous=check(req,continuous_boxes,'continuous-subject-change');assert not continuous[1]['decisions'][8]['cut']
    reversed_segments=copy.deepcopy(continuous[0]);reversed_segments['segments'].reverse();assert call(reversed_segments)==continuous[1]
    future_boxes=[[8,10,4,4],[8,10,4,4],[68,10,4,4]]
    future=check(request(future_boxes,(64,32),(64,32),padding=0,maximum_step=[2,2],smoothing_radius=0),future_boxes,'future-feasible-pan')
    assert [d['crop'][0] for d in future[1]['decisions']]==[4,6,8]
    jitter=[[30+[-3,3][n%2],23,6,6] for n in range(32)]
    direct=check(request(jitter,maximum_step=[10,10],smoothing_radius=0),jitter,'jitter-follow')
    smoothed=check(request(jitter,maximum_step=[10,10],smoothing_radius=3),jitter,'jitter-smoothed')
    motion=lambda r:sum(abs(d['step'][0]) for d in r[1]['decisions'])
    assert motion(smoothed)<motion(direct)/4
    motion_evidence={'direct_total_pan_pixels':motion(direct),'smoothed_total_pan_pixels':motion(smoothed),'complete_padded_subject_retention':True}
    passed.append('reframe.subject_changes_motion_constraints_and_smoothing')

    controls={'search_radius':4,'maximum_step':4,'maximum_acceleration':4,'minimum_correlation_milli':900,'minimum_margin_milli':40,'maximum_frame_change_milli':200}
    rng=random.Random(894230);textures=[[rng.randrange(10,80)*3 for _ in range(120)] for _ in range(2)]
    def footage(name,positions,alpha=255,premult=False,trim=False,cut=None,fault=None):
        images=[]
        for n,(x,y) in enumerate(positions):
            background=6*alpha//255 if premult else 6
            image=Image.new('RGBA',(72,54),(background,background,background,alpha))
            texture=textures[int(cut is not None and n>=cut)]
            for yy in range(10):
                for xx in range(12):
                    value=texture[yy*12+xx]
                    if premult:value=value*alpha//255
                    image.putpixel((x+xx,y+yy),(value,value,value,alpha))
            if fault=='constant':image=Image.new('RGBA',(72,54),(90,90,90,255))
            if fault=='duplicate':
                for yy in range(10):
                    for xx in range(12):
                        value=texture[yy*12+xx];image.putpixel((x+18+xx,y+yy),(value,value,value,255))
            if n==1 and fault in ('occlusion','cut','transparent'):
                if fault=='occlusion':
                    for yy in range(7):
                        for xx in range(12):image.putpixel((x+xx,y+yy),(6,6,6,255))
                elif fault=='cut':image=Image.new('RGBA',(72,54),(255,255,255,255))
                else:image=Image.new('RGBA',(72,54),(0,0,0,0))
            if trim:image=image.crop((4,7,68,49))
            images.append(png(f'{name}-{n}',image))
        s=base(images,len(positions));l=s['layers'][0];l['end']='hold_last'
        if premult:l['alpha_mode']='premultiplied'
        for f in l['frames']:f['hold']=time(1,25);f['offset']=[4,7] if trim else [0,0]
        return s
    def tracked(scene,positions):
        b=[[x-2,y-2,16,14] for x,y in positions];r=request(b);r['scene']=scene
        r['segments']=[{'start':time(0),'subject_id':'selected','cut':True,'selection':{'mode':'track','focus':b[0],'region':[*positions[0],12,10],'controls':copy.deepcopy(controls)}}]
        return r,b
    positions=[[8+2*n,21+n%2] for n in range(12)]
    track_request,track_boxes=tracked(footage('tracked',positions),positions)
    tracked_result=check(track_request,track_boxes,'local-tracked-subject')
    confidence=[d['confidence'] for d in tracked_result[1]['decisions']]
    assert all(c['correlation']>.999999 and c['margin']>=.04 for c in confidence)
    for trim in (False,True):
        r,b=tracked(footage('alpha-straight-'+str(trim),positions,alpha=170,trim=trim),positions)
        straight=check(r,b,'tracked-alpha-straight-'+str(trim))
        r,b=tracked(footage('alpha-premult-'+str(trim),positions,alpha=170,premult=True,trim=trim),positions)
        associated=check(r,b,'tracked-alpha-premult-'+str(trim))
        assert straight[3]==associated[3]
    positions2=positions[:6]+[[44-2*n,19+n%2] for n in range(6)]
    r,b=tracked(footage('tracked-subject-change',positions2,cut=6),positions2)
    r['segments'].append({'start':time(6,25),'subject_id':'second-subject','cut':True,'selection':{'mode':'track','focus':b[6],'region':[*positions2[6],12,10],'controls':copy.deepcopy(controls)}})
    check(r,b,'local-tracking-subject-cut',[0,6])
    corrected_boxes=copy.deepcopy(track_boxes);corrected_boxes[5][0]-=1
    corrected=copy.deepcopy(track_request);corrected['segments']=[segment(corrected_boxes,subject='corrected')]
    correction=check(corrected,corrected_boxes,'authored-tracking-correction')
    assert correction[1]['decisions'][5]['crop']!=tracked_result[1]['decisions'][5]['crop']
    assert all(d['confidence'] is None for d in correction[1]['decisions'])
    for fault in ('constant','duplicate','occlusion','cut','transparent'):
        s=footage('unreliable-'+fault,positions[:3],fault=fault);r,_=tracked(s,positions[:3])
        if fault=='duplicate':r['segments'][0]['selection']['controls'].update(search_radius=20,maximum_step=20)
        call(r,'TRACKING_UNRELIABLE')
    jump=[[8,21],[35,21],[37,21]];r,_=tracked(footage('jump',jump),jump);call(r,'TRACKING_UNRELIABLE')
    tracking_evidence={'minimum_correlation':min(c['correlation'] for c in confidence),'minimum_margin':min(c['margin'] for c in confidence),'all_authored_positions_exact':True,'rejected_failure_types':['low_texture','ambiguous_duplicate','occlusion','source_cut','transparent_patch','excessive_motion']}
    passed.append('reframe.local_tracking_confidence_and_loss')

    # Masks/effects retain source coordinates; opacity and sound retain the original clock.
    styled=request(boxes,sampling='bilinear');l=styled['scene']['layers'][0]
    l['mask']={'rect':[12,12,40,32],'inverted':False,'feather':{'radius':4,'edge':'centered'}}
    l['effects']=[{**grade(),'exposure_milli':350,'white_balance_milli':[1050,950,1100]}]
    l['blend_mode']='screen';l['animation']={'opacity':{'keys':[{'time':time(0),'value':51,'interpolation':'linear'},{'time':time(15,25),'value':231,'interpolation':'hold'}]}}
    pcm_values=array.array('h')
    for n in range(16*1920):pcm_values.extend([(n*61)%40001-20000,15000-(n*43)%30001])
    assert pcm_values.itemsize==2
    if __import__('sys').byteorder!='little':pcm_values.byteswap()
    pcm=pcm_values.tobytes();audio_file=sources/'original-audio.wav'
    with wave.open(str(audio_file),'wb') as wav:
        wav.setnchannels(2);wav.setsampwidth(2);wav.setframerate(48000);wav.writeframes(pcm)
    styled['scene']['audio']={'file':remember(audio_file),'start':time(0),'channels':'preserve_stereo','resampling':'linear','padding':'silence'}
    styled_result=check(styled,boxes,'styled-with-audio',pcm=pcm)

    client=Client(EXE)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        tool=next(t for t in catalog if t['name']=='cutbolt_reframe_inspect');assert tool['annotations']['readOnlyHint']
        args={k:v for k,v in styled_result[0].items() if k!='command'}
        Draft202012Validator(tool['inputSchema']).validate(args)
        assert client.call('reframe.inspect',**args)==styled_result[1]
        bad_args=copy.deepcopy(args);bad_args['window']=[0,32]
        client.call('reframe.inspect',expected_error='INVALID_REFRAME',**bad_args)
        template={'schema_version':1,'id':'selected-subject','scene':styled_result[1]['scene'],'parameters':[]}
        instance=client.call('graphics.instantiate',template=template,values={},instance_id='reframe-template',input_root=str(sources))
        assert instance['scene']['layers'][0]==styled_result[1]['scene']['layers'][0]
        call({'command':'scene.render','scene':instance['scene'],'input_root':str(sources),'output_root':str(out),'output':str(out/'template.mkv')})
        compare(out/'template.mkv',styled_result[3],pcm)
        project=client.call('project.create',id='reframe-edits',width=27,height=48,frame_rate=time(25))
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        fields={'store_root':str(store),'project_id':'reframe-edits','request_id':'choose-ranges','expected_revision':0,'operations':[
            {'op':'media.add','asset':styled_result[2]['asset']},
            {'op':'clip.append','clip':{'id':'first','asset_id':'styled-with-audio','source_in':time(2,25),'duration':time(5,25)}},
            {'op':'clip.append','clip':{'id':'second','asset_id':'styled-with-audio','source_in':time(10,25),'duration':time(3,25)}}]}
        changed=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==changed
        saved=client.call('session.get',store_root=str(store),project_id='reframe-edits')
        chosen=[2,3,4,5,6,10,11,12];expected=[styled_result[3][n] for n in chosen]
        audio=b''.join(pcm[n*1920*4:(n+1)*1920*4] for n in chosen)
        call({'command':'render.run','project':saved,'input_root':str(out),'output_root':str(out),'output':str(out/'saved.mkv')});compare(out/'saved.mkv',expected,audio)
        for n in (0,4,7):
            target=out/f'preview-{n}.png'
            client.call('preview.frame',project=saved,input_root=str(out),output_root=str(out),output=str(target),time=time(n,25))
            with Image.open(target) as image:
                assert image.size==(27,48);actual=image.convert('RGB').tobytes()
            assert len(actual)==len(expected[n]) and max(abs(a-b) for a,b in zip(actual,expected[n]))<=1;previews+=1
        call({'command':'preview.range','project':saved,'input_root':str(out),'output_root':str(out),'output':str(out/'range.mkv'),'start':time(2,25),'duration':time(4,25)})
        compare(out/'range.mkv',expected[2:6],audio[2*1920*4:6*1920*4]);previews+=1
        common={'store_root':str(store),'project_id':'reframe-edits'}
        client.call('session.undo',**common,request_id='undo',expected_revision=1)
        client.call('session.restore',**common,request_id='restore',expected_revision=2,target_revision=1)
        restored=client.call('session.get',**common)
        assert restored['clips']==saved['clips'] and len(client.call('session.history',**common)['entries'])==4
        call({'command':'render.run','project':restored,'input_root':str(out),'output_root':str(out),'output':str(out/'restored.mkv')});compare(out/'restored.mkv',expected,audio)
    finally:client.close()
    passed.append('reframe.effects_templates_mcp_saved_audio_and_previews')

    image=Image.new('RGBA',(512,512));image.putdata([((x*13+y*3)%256,(x*17+y*7)%256,(x*23+y*11)%256,255) for y in range(512) for x in range(512)])
    maximum_source=png('maximum-original',image);maximum_scene=base([maximum_source],128,(512,512))
    maximum_scene['layers'][0]['frames'][0]['hold']=time(1,25)
    maximum_boxes=[];maximum_segments=[]
    for s in range(16):
        b=[[20+(s*29)%420+3*n,20+(s*23)%420+2*n,8,8] for n in range(8)]
        if s==15:b=[[504,504,8,8]]*8
        maximum_boxes+=b;maximum_segments.append(segment(b,s*8,f'subject-{s}'))
    maximum=request(maximum_boxes,(64,64),(32,32),scene=maximum_scene,padding=0,maximum_step=[4,4],smoothing_radius=2,sampling='bilinear',segments=maximum_segments)
    process=subprocess.Popen([str(EXE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    response={}
    def collect():response['stdout'],response['stderr']=process.communicate(json.dumps(maximum).encode())
    started=clock.monotonic();reader=threading.Thread(target=collect);reader.start();peak=0
    while process.poll() is None:
        try:peak=max(peak,peak_memory(process.pid))
        except AssertionError:
            if process.poll() is None:raise
        clock.sleep(.01)
    reader.join(timeout=5);wall=clock.monotonic()-started;value=json.loads(response['stdout'])
    assert process.returncode==0 and value['ok'],value
    assert 0<peak<128*1024*1024,(peak,wall)
    budgets.check(wall<30,(peak,wall))
    max_result=check(maximum,maximum_boxes,'maximum-frames-and-segments',list(range(0,128,8)))
    assert len(max_result[1]['decisions'])==128 and all(d['source_frame']==0 for d in max_result[1]['decisions'])
    performance={'frames':128,'segments':16,'source_canvas':[512,512],'inspection_peak_working_set_bytes':peak,'inspection_seconds':wall,'gates':{'maximum_bytes':128*1024*1024,'maximum_seconds':30}}
    too_many=copy.deepcopy(maximum);too_many['scene']['duration']=time(129,25);too_many['scene']['layers'][0]['duration']=time(129,25);call(too_many,'INVALID_REFRAME')
    aggregate=copy.deepcopy(maximum);aggregate['scene']=base(charts,128);aggregate['window']=[72,54];aggregate['output_size']=[72,54]
    aggregate['segments']=[{'start':time(n*8,25),'subject_id':f'part-{n}','cut':True,'selection':{'mode':'track','focus':[0,0,72,54],'region':[0,0,64,54],'controls':{**controls,'search_radius':9}}} for n in range(16)]
    assert 8*19**2*64*54<64_000_000<128*19**2*64*54
    call(aggregate,'LIMIT_EXCEEDED')
    passed.append('reframe.maximum_duration_work_and_memory')

    invalid=[]
    def bad(changes,code='INVALID_REFRAME'):
        r=copy.deepcopy(first[0]);r.update(changes);invalid.append((r,code))
    for changes in ({'window':[0,32]},{'window':[18,0]},{'window':[73,32]},{'window':[17,32]},
                    {'output_size':[0,48]},{'output_size':[513,912]},{'padding':4097},{'padding':30},
                    {'maximum_step':[0,3]},{'maximum_step':[3,4097]},{'smoothing_radius':17},{'segments':[]}):bad(changes)
    bad({'sampling':'area'},'INVALID_JSON');bad({'unexpected':True},'INVALID_JSON');bad({'input_root':'.'},'INVALID_PATH')
    for mutation in ('first-cut','first-start','unaligned-start','duplicate','many','subject-id','unknown-selection','empty-keys','many-keys','missing-zero','outside','empty-box','overflow-box','duplicate-keys','invalid-time','late-key','precision','unknown-interpolation'):
        r=copy.deepcopy(first[0]);s=r['segments'][0];keys=s['selection']['keys'];code='INVALID_REFRAME'
        if mutation=='first-cut':s['cut']=False
        if mutation=='first-start':s['start']=time(1,25)
        if mutation=='unaligned-start':s['start']=time(1,50);code='UNALIGNED_TIME'
        if mutation=='duplicate':r['segments'].append(copy.deepcopy(s));r['segments'][1]['start']={'num':0,'den':2}
        if mutation=='many':r['segments']=[copy.deepcopy(s) for _ in range(17)]
        if mutation=='subject-id':s['subject_id']='not valid'
        if mutation=='unknown-selection':s['selection']['mode']='automatic';code='INVALID_JSON'
        if mutation=='empty-keys':s['selection']['keys']=[]
        if mutation=='many-keys':s['selection']['keys']=[copy.deepcopy(keys[0]) for _ in range(129)]
        if mutation=='missing-zero':s['selection']['keys']=keys[1:]
        if mutation=='outside':keys[0]['rect']=[71,20,6,5]
        if mutation=='empty-box':keys[0]['rect'][2]=0
        if mutation=='overflow-box':keys[0]['rect']=[2147483647,0,2147483647,1]
        if mutation=='duplicate-keys':keys.append(copy.deepcopy(keys[0]));code='INVALID_ANIMATION'
        if mutation=='invalid-time':keys[0]['time']={'num':0,'den':0};code='INVALID_TIME'
        if mutation=='late-key':keys[-1]['time']=time(17,25);code='INVALID_ANIMATION'
        if mutation=='precision':keys[1]['time']=time(1,1000001);code='INVALID_ANIMATION'
        if mutation=='unknown-interpolation':keys[0]['interpolation']='cosine';code='INVALID_JSON'
        invalid.append((r,code))
    for mutation in ('anchor','spatial','position','scale','rotation','crop','short-layer','late-layer','multiple','graphics','position-animation','bad-alpha','changed','escape','transparent-end'):
        r=copy.deepcopy(first[0]);l=r['scene']['layers'][0];code='INVALID_REFRAME'
        if mutation=='anchor':l['frames'][0]['anchor']=[1,0]
        if mutation=='spatial':l['transform']['spatial']=copy.deepcopy(first[1]['scene']['layers'][0]['transform']['spatial'])
        if mutation=='position':l['transform']['position']=[1,0]
        if mutation=='scale':l['transform']['scale']=2
        if mutation=='rotation':l['transform']['quarter_turns']=1
        if mutation=='crop':l['transform']['crop']=[1,0,71,54]
        if mutation=='short-layer':l['duration']=time(15,25)
        if mutation=='late-layer':l['duration']=time(15,25);l['start']=time(1,25)
        if mutation=='multiple':r['scene']['layers'].append(copy.deepcopy(l));r['scene']['layers'][1]['id']='another'
        if mutation=='graphics':l.update(frames=[],graphics={'kind':'shape','shape':'rectangle','rect':[0,0,30,30],'fill':[0,0,0,255]},end='hold_last')
        if mutation=='position-animation':l['animation']={'position_x':curve([0])}
        if mutation=='bad-alpha':l['alpha_mode']='premultiplied';code='INVALID_ALPHA'
        if mutation=='changed':l['frames'][0]['image']['sha256']='0'*64;code='MEDIA_CHANGED'
        if mutation=='escape':l['frames'][0]['image']['path']='../outside.png';code='INVALID_PATH'
        if mutation=='transparent-end':l['end']='transparent';code='REFRAME_UNAVAILABLE'
        invalid.append((r,code))
    impossible_boxes=[[8,20,16,14],[12,20,16,14]]
    invalid.append((request(impossible_boxes),'REFRAME_INFEASIBLE'))
    r=request([[16,20,30,6]]);invalid.append((r,'REFRAME_INFEASIBLE'))
    for mutation in ('one-frame','patch-outside','small-patch','radius','step','acceleration','correlation','margin','frame-change'):
        r=copy.deepcopy(track_request);selection=r['segments'][0]['selection'];code='INVALID_REFRAME'
        if mutation=='one-frame':r['scene']['duration']=time(1,25);r['scene']['layers'][0]['duration']=time(1,25)
        if mutation=='patch-outside':selection['region']=[0,0,12,10]
        if mutation=='small-patch':selection['region'][2]=3
        if mutation=='radius':selection['controls']['search_radius']=0
        if mutation=='step':selection['controls']['maximum_step']=0;code='INVALID_TRACKING'
        if mutation=='acceleration':selection['controls']['maximum_acceleration']=0;code='INVALID_TRACKING'
        if mutation=='correlation':selection['controls']['minimum_correlation_milli']=849;code='INVALID_TRACKING'
        if mutation=='margin':selection['controls']['minimum_margin_milli']=19;code='INVALID_TRACKING'
        if mutation=='frame-change':selection['controls']['maximum_frame_change_milli']=1001;code='INVALID_TRACKING'
        invalid.append((r,code))
    existing={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    for r,code in invalid:call(r,code)
    call({'command':'scene.render','scene':styled_result[1]['scene'],'input_root':str(sources),'output_root':str(out),'output':str(out/'styled-with-audio.mkv')},'OUTPUT_EXISTS')
    assert existing=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    # Only the generator's own input is mutated after successful encoding, then restored.
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe'
    wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}',encoding='utf-8')
    subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=sources/charts[0]['path'];original_bytes=source.read_bytes()
    try:
        call({'command':'scene.render','scene':styled_result[1]['scene'],'input_root':str(sources),'output_root':str(out),'output':str(out/'changed.mkv')},'MEDIA_CHANGED',
             {**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(original_bytes)
    assert existing=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in out.iterdir()}
    assert all(hashlib.sha256(p.read_bytes()).hexdigest()==h for p,h in hashes.items())
    assert not list(out.glob('.cutbolt-scene-*'))
    passed.append('reframe.validation_identity_and_output_preservation')
    report={'passed':passed,'cases':cases,'decoded_frames':frames,'stereo_sample_frames':samples,'previews':previews,'rejected_cases':rejected,
            'motion':motion_evidence,'tracking':tracking_evidence,'performance':performance,'source_preservation':True,
            'oracle':'Enumerated integer crop positions and motion edges, authored subject/source trajectories, exact Fraction clocks, 60-digit forward source geometry and rational premultiplied filtering; all decoded RGB/PCM compared, at most one RGB unit and exact PCM.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k!='cases'},indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
