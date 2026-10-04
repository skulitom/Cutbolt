"""Original known-clock signals: measured alignment, explicit correction, and camera use."""
from engine import ENGINE, MCP_TOOLS
import argparse
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
import math
import os
from pathlib import Path
import random
import shutil
import subprocess
from jsonschema import Draft202012Validator
from agents import Client
from tracks import time, seconds, edit, track, placement
from sequences import nested, seqedit

ROOT=Path(__file__).resolve().parents[1]
W,H,N,R=16,12,150,48000


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store=[root/n for n in ('sources','output','store')]
    for p in (sources,out,store):p.mkdir()
    exe=ENGINE;passed=[];rejected=0;frames=0;samples=0;measurements=[]
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(req,error=None,env=None):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps(req).encode(),capture_output=True,timeout=180,env=env);value=json.loads(p.stdout)
        if error:assert p.returncode==1 and value['error']['code']==error,(error,value);rejected+=1;return value
        if p.returncode:(root/'failed-request.json').write_text(json.dumps(req,indent=2))
        assert p.returncode==0 and value['ok'],value;return value['result']
    def apply(p,ops):return call({'command':'timeline.apply','project':p,'expected_revision':p['revision'],'operations':ops})
    # A continuous, reproducible, aperiodic piecewise-linear signal, independent of the estimator.
    # Each camera samples this common physical clock with its own declared offset and rate.
    rng=random.Random(61327);knots=[[rng.randrange(-11000,11001) for _ in range(8000)] for _ in range(2)]
    def wave(at,ch):
        pos=at/48+100;k=math.floor(pos);f=pos-k
        return round(knots[ch][k]*(1-f)+knots[ch][k+1]*f)
    specs={'reference':(0,F(1),'normal'),'fast':(5000,F(1251,1250),'normal'),
           'slow':(7200,F(4997,5000),'normal'),'inverted':(6000,F(1),'inverted'),
           'bent':(5000,F(1),'bent'),'flipped':(5000,F(1),'flipped'),
           'silent':(0,F(1),'silent'),'periodic':(0,F(1),'periodic'),
           'timecode':(5760,F(1),'normal')}
    assets=[];pictures={};sounds={}
    for name,(offset,rate,mode) in specs.items():
        def physical(n):return (n-offset)/float(rate)-(30 if mode=='bent' and n>2*R and n<3*R else 0)
        pcm=array('h')
        for n in range(N*1920):
            at=physical(n)
            for ch in range(2):
                v=0 if mode=='silent' else round(9000*math.sin(2*math.pi*n/240)) if mode=='periodic' else wave(at,ch)
                if mode=='flipped' and n>2*R and n<3*R:v=-v
                # Inversion case deliberately measures the other channel.
                if mode=='inverted':v=-wave(at,0) if ch==1 else wave(at,1)
                pcm.append(v)
        rgb=[bytes((max(0,min(255,round(physical(n*1920)/R*32))),i*29%256,n%256))*W*H for i,n in enumerate(range(N))]
        raw=sources/(name+'.rgb');audio=sources/(name+'.pcm');movie=sources/(name+'.mkv')
        raw.write_bytes(b''.join(rgb));audio.write_bytes(pcm.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),'-f','s16le','-ar',str(R),'-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        pictures[name]=rgb;sounds[name]=pcm
        assets.append({'id':name,'path':str(movie),'duration':time(N,25),'identity':{'sha256':hashlib.sha256(movie.read_bytes()).hexdigest(),'bytes':movie.stat().st_size}})
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    p=call({'command':'project.create','id':'clock-test','width':W,'height':H,'frame_rate':time(25)})
    p=apply(p,[{'op':'media.add','asset':a} for a in assets])
    method={'mode':'audio','windows':[{'reference_center':time(n,R),'candidate_center':time(n,R),'length':time(4096,R),'search_radius':time(9600,R)} for n in (R,5*R//2,4*R)],'reference_channel':0,'candidate_channel':0,'polarity':'same','minimum_correlation_milli':900,'minimum_margin_milli':100,'maximum_drift_ppm':2000,'maximum_residual_samples':2}
    base={'command':'sync.inspect','id':'aligned','project':p,'input_root':str(sources),'reference_asset_id':'reference','candidate_asset_id':'fast','start':time(13,25),'duration':time(4),'rounding':'nearest_sample','method':method}
    results={};outputs={};receipts={}
    def decode(path):return ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-']),ff(['-i',str(path),'-vn','-f','s16le','-'])
    def oracle(name,recipe):
        start=seconds(recipe['source_in']);rate=seconds(recipe['rate']);duration=seconds(recipe['duration']);count=int(duration*R)
        indices=[int((start+F(n,25)*rate)*25) for n in range(int(duration*25))]
        rgb=b''.join(pictures[name][i] for i in indices);pcm=array('h');source=sounds[name]
        # Integer fraction oracle, without the implementation's float or correlation arithmetic.
        first=start*R;den=math.lcm(first.denominator,rate.denominator);a=int(first*den);step=int(rate*den)
        for n in range(count):
            lo,rem=divmod(a+n*step,den);hi=min(lo+1,len(source)//2-1)
            for ch in range(2):
                num=source[lo*2+ch]*(den-rem)+source[hi*2+ch]*rem
                v=(abs(num)*2+den)//(2*den);pcm.append(-v if num<0 else v)
        return rgb,pcm.tobytes(),indices
    def conform(name,recipe,label):
        nonlocal frames,samples
        path=out/(label+'.mkv');receipt=call({'command':'media.conform','recipe':recipe,'input_root':str(sources),'output_root':str(out),'output':str(path)})
        expected=oracle(name,recipe);actual=decode(path)
        assert actual==expected[:2],label
        assert receipt['source_frame_indices']==expected[2];frames+=len(expected[2]);samples+=len(expected[1])//4
        receipts[label]=receipt;outputs[label]=actual;return actual
    for name in ('fast','slow','inverted'):
        req=copy.deepcopy(base);req['id']=name;req['candidate_asset_id']=name
        if name=='inverted':req['method'].update(candidate_channel=1,polarity='either')
        r=call(req);results[name]=r;offset,rate,_=specs[name];m=r['mapping'];measured=seconds(m['rate'])
        errors=[]
        for w in r['measurement']['windows']:
            err=abs(seconds(w['candidate_center'])*R-(offset+rate*seconds(w['reference_center'])*R));errors.append(float(err));assert err<=1,(name,err,r)
        assert abs(float(measured-rate))*1e6<15,(name,m,rate)
        assert all(w['inverted']==(name=='inverted') for w in r['measurement']['windows'])
        assert abs(r['candidate_start_adjustment_samples'])<=0.5
        print(json.dumps({'case':name,'measurement':r['measurement'],'ground_truth_max_sample_error':max(errors)}),flush=True)
        if name=='inverted':continue
        ref=conform('reference',r['reference_recipe'],name+'-reference');cand=conform(name,r['candidate_recipe'],name+'-candidate')
        ref_pcm=array('h',ref[1]);cand_pcm=array('h',cand[1]);start=int(seconds(req['start'])*R)
        corrected=math.sqrt(sum((a-b)**2 for a,b in zip(ref_pcm,cand_pcm))/len(ref_pcm))
        uncorrected=math.sqrt(sum((ref_pcm[n*2]-sounds[name][(start+n)*2])**2 for n in range(4*R))/(4*R))
        # Integer-sample lag estimates and explicit start rounding retain sub-sample error.
        # Bound RMS below 0.8% PCM full scale and require a twentyfold improvement.
        assert corrected<256 and corrected<uncorrected/20,(name,corrected,uncorrected)
        # Frame-held video is within one source frame of physical-clock alignment.
        assert max(abs(ref[0][i]-cand[0][i]) for i in range(0,len(ref[0]),W*H*3))<=2
        measurements.append({'case':name,'true_drift_ppm':float(rate-1)*1e6,'measured_drift_ppm':r['measurement']['drift_ppm'],'max_match_error_samples':max(errors),'corrected_stereo_rms_error_pcm16':corrected,'uncorrected_left_rms_error_pcm16':uncorrected})
    passed.extend(['sync.known_audio_offsets_drift_polarity','sync.explicit_correction_pixels_pcm_quality'])
    # Declared non-drop labels include explicit day rollover and nonzero media anchors.
    tc=copy.deepcopy(base);tc.update(id='clock-labels',candidate_asset_id='timecode',rounding='reject')
    tc['method']={'mode':'timecode','reference':{'day':1,'label':'00:00:00:02','media_at':time(28,25)},'candidate':{'day':0,'label':'23:59:59:21','media_at':time(1)}}
    tcr=call(tc);assert seconds(tcr['candidate_recipe']['source_in'])==seconds(tc['start'])+F(3,25)
    assert seconds(tcr['mapping']['rate'])==1 and tcr['measurement']['drift_estimated'] is False
    aligned=conform('timecode',tcr['candidate_recipe'],'timecode-aligned');assert aligned[1]==outputs['fast-reference'][1]
    assert all(aligned[0][i]==outputs['fast-reference'][0][i] for i in range(0,len(aligned[0]),W*H*3))
    equivalent=copy.deepcopy(tc);equivalent['method']['reference']={'day':0,'label':'23:59:59:24','media_at':time(1)}
    assert call(equivalent)['candidate_recipe']==tcr['candidate_recipe']
    reverse=copy.deepcopy(tc);reverse['reference_asset_id'],reverse['candidate_asset_id']='timecode','reference';reverse['method']['reference'],reverse['method']['candidate']=reverse['method']['candidate'],reverse['method']['reference']
    assert seconds(call(reverse)['candidate_recipe']['source_in'])==seconds(tc['start'])-F(3,25)
    passed.append('sync.declared_timecode_day_anchors')
    # Returned, content-bound corrected assets use the ordinary saved multicam contract.
    program=call({'command':'project.create','id':'aligned-cameras','width':W,'height':H,'frame_rate':time(25)})
    labels=['fast-reference','fast-candidate','slow-candidate'];ops=[]
    for i,label in enumerate(labels):
        asset=receipts[label]['asset'];seq='camera'+str(i);ops.extend([{'op':'media.add','asset':asset},{'op':'sequence.create','id':seq,'duration':time(4)}])
        for kind in ('video','audio'):ops.extend([seqedit(seq,edit('add',track=track(kind,kind))),seqedit(seq,edit('place',track_id=kind,clip=placement(kind,asset['id'],0,100),collision='reject'))])
    group={'duration':time(4),'locked':False,'angles':[{'id':str(i),'sequence_id':'camera'+str(i),'start':time(0),'duration':time(4),'source_in':time(0),'audio_in':time(0)} for i in range(3)],'cuts':[{'id':str(i),'at':time(n,25),'angle_id':str(i)} for i,n in enumerate((0,33,66))],'audio':{'mode':'follow_video'}}
    ops.extend([{'op':'multicam.create','id':'aligned','group':group},edit('create',duration=time(4)),edit('add',track=track('v','video')),edit('add',track=track('a','audio'))])
    for kind in ('v','a'):ops.append(edit('place',track_id=kind,clip=nested(kind,'aligned',0,100),collision='reject'))
    program=apply(program,ops);path=out/'aligned-program.mkv'
    call({'command':'render.run','project':program,'input_root':str(out),'output_root':str(out),'output':str(path)})
    expected=tuple(b''.join(outputs[label][stream][lo*unit:hi*unit] for label,lo,hi in zip(labels,(0,33,66),(33,66,100))) for stream,unit in ((0,W*H*3),(1,1920*4)))
    assert decode(path)==expected;frames+=100;samples+=4*R
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
        tool=next(t for t in catalog if t['name']=='cutbolt_sync_inspect');assert tool['annotations']['readOnlyHint'] and tool['annotations']['idempotentHint']
        fields={k:v for k,v in base.items() if k!='command'};Draft202012Validator(tool['inputSchema']).validate(fields)
        client.call('session.create',store_root=str(store),project=program,request_id='create');common={'store_root':str(store),'project_id':program['id']};saved=client.call('session.get',**common);history=client.call('session.history',**common)
        assert client.call('sync.inspect',**fields)==call(base)
        Draft202012Validator(tool['inputSchema']).validate({k:v for k,v in tc.items() if k!='command'})
        assert client.call('sync.inspect',**{k:v for k,v in tc.items() if k!='command'})==tcr
        assert client.call('session.get',**common)==saved and client.call('session.history',**common)==history
        change={'op':'multicam.edit','id':'aligned','edit':{'op':'cut_set','cut':{'id':'1','at':time(33,25),'angle_id':'2'}}}
        fields={**common,'expected_revision':0,'request_id':'alternate','operations':[change]}
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'});assert preview['sequences'][0]['before']['multicam']['angles']==preview['sequences'][0]['after']['multicam']['angles']
        receipt=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==receipt
        client.call('session.undo',**common,expected_revision=1,request_id='undo');assert client.call('session.get',**common)['sequences']==saved['sequences']
    finally:client.close()
    passed.append('sync.corrected_multicam_saved_mcp')
    # Fail closed on weak, repetitive, nonlinear or inconsistent evidence and invalid bounds.
    for name,code in [('silent','SYNC_INSUFFICIENT_SIGNAL'),('bent','SYNC_INCONSISTENT'),('flipped','SYNC_INCONSISTENT')]:
        req=copy.deepcopy(base);req['candidate_asset_id']=name
        if name=='flipped':req['method']['polarity']='either'
        call(req,code)
    req=copy.deepcopy(base);req.update(reference_asset_id='periodic',candidate_asset_id='periodic');call(req,'SYNC_AMBIGUOUS')
    for changes,code in [({'maximum_drift_ppm':100},'SYNC_DRIFT_EXCEEDED'),({'reference_channel':2},'INVALID_SYNC'),({'minimum_correlation_milli':499},'INVALID_SYNC'),({'minimum_margin_milli':0},'INVALID_SYNC'),({'maximum_residual_samples':17},'INVALID_SYNC'),({'maximum_drift_ppm':5001},'INVALID_SYNC')]:
        req=copy.deepcopy(base);req['method'].update(changes);call(req,code)
    req=copy.deepcopy(base);req['method']['windows']=req['method']['windows'][:2];call(req,'INVALID_SYNC')
    req=copy.deepcopy(base);req['method']['windows'].append(req['method']['windows'][0]);call(req,'INVALID_SYNC')
    for field,value,code in [('length',time(2049,R),'INVALID_SYNC'),('length',time(8194,R),'INVALID_SYNC'),('search_radius',time(31,R),'INVALID_SYNC'),('search_radius',time(48001,R),'INVALID_SYNC'),('reference_center',time(1,R),'INVALID_SYNC'),('candidate_center',time(6),'INVALID_SYNC'),('reference_center',time(9007199254740991),'TIME_OVERFLOW'),('reference_center',{'num':1,'den':0},'INVALID_TIME')]:
        req=copy.deepcopy(base);req['method']['windows'][0][field]=value;call(req,code)
    req=copy.deepcopy(base)
    for w in req['method']['windows']:w['search_radius']=time(32,R)
    call(req,'SYNC_AMBIGUOUS')
    req=copy.deepcopy(base);req['rounding']='reject';call(req,'UNALIGNED_TIME')
    for changes,code in [({'id':''},'INVALID_SYNC'),({'duration':time(0)},'INVALID_SYNC'),({'duration':time(1501,25)},'INVALID_SYNC'),({'start':time(1,R)},'UNALIGNED_TIME'),({'start':time(5)},'INVALID_RANGE'),({'candidate_asset_id':'missing'},'MISSING_MEDIA')]:call({**tc,**changes},code)
    for changes,code in [({'label':'00:00:00;00'},'UNSUPPORTED_TIMECODE'),({'label':'00:00:00:25'},'INVALID_TIMECODE'),({'label':'24:00:00:00'},'INVALID_TIMECODE'),({'label':'00:60:00:00'},'INVALID_TIMECODE'),({'day':1000001},'UNSUPPORTED_TIMECODE'),({'media_at':time(6)},'INVALID_SYNC'),({'media_at':time(1,R)},'UNALIGNED_TIME')]:
        req=copy.deepcopy(tc);req['method']['candidate'].update(changes);call(req,code)
    req=copy.deepcopy(tc);req['method']['candidate']['day']=1;call(req,'INVALID_RANGE')
    req=copy.deepcopy(tc);req['project']['assets'][0]['identity']=None;call(req,'IDENTITY_REQUIRED')
    req=copy.deepcopy(tc);req['project']['assets'][0]['identity']['sha256']='0'*64;call(req,'IDENTITY_MISMATCH')
    req=copy.deepcopy(tc);req['input_root']=str(out);call(req,'PATH_OUTSIDE_ROOT')
    passed.append('sync.ambiguity_clock_bounds_rejection')
    # Inspect is read-only; conversion preserves sources and refuses reusing any output.
    before={x.relative_to(root).as_posix():hashlib.sha256(x.read_bytes()).hexdigest() for x in root.rglob('*') if x.is_file()}
    call(tc)
    assert before=={x.relative_to(root).as_posix():hashlib.sha256(x.read_bytes()).hexdigest() for x in root.rglob('*') if x.is_file()}
    call({'command':'media.conform','recipe':tcr['candidate_recipe'],'input_root':str(sources),'output_root':str(out),'output':str(out/'timecode-aligned.mkv')},'OUTPUT_EXISTS')
    wrapper=root/'changed-source.rs';tool=root/'changed-source.exe';wrapper.write_text('use std::{env,process::Command,fs::OpenOptions,io::Write};fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();if status.success() && args.last().is_some_and(|p|p=="pipe:1") && args.iter().any(|a|a=="s16le"){OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap().write_all(b"changed").unwrap();}std::process::exit(status.code().unwrap_or(1));}');subprocess.run(['rustc',str(wrapper),'-o',str(tool)],capture_output=True,check=True)
    source=sources/'fast.mkv';saved_bytes=source.read_bytes()
    try:call(base,'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved_bytes)
    assert originals=={x.name:hashlib.sha256(x.read_bytes()).hexdigest() for x in sources.iterdir()}
    assert not list(out.glob('*.partial.mkv'));passed.append('sync.identities_read_only_source_preservation')
    report={'passed':passed,'measurements':measurements,'rejected_cases':rejected,'frames_compared':frames,'sample_frames_compared':samples,'reference':'Original seeded aperiodic continuous signal sampled at known affine camera clocks; all corrected video and PCM compared exactly with independent integer-fraction resampling. Physical alignment separately checked against known clocks: <=1 sample window error, <15 ppm drift error, RMS <256 PCM16 and >20-fold improvement. Explicit 25 fps day-labelled timecode, saved corrected multicam and fail-closed evidence.'}
    (root/'request.json').write_text(json.dumps(base,indent=2)+'\n');(root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
