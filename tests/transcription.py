"""Actual offline speech, independent word clocks and rendered synchronized cuts.

Requires an explicitly configured external runtime. Missing setup is a failure,
never a skipped test that can award transcription coverage.
"""
from engine import ENGINE
import argparse
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import time as clock
import unicodedata
import wave
from PIL import Image
from agents import Client
from tracks import time, seconds, edit, placement, track
from transcription_speech import generate
from transcription_runtime import maximum, failures
from transcription_guard import run as guard_checks

ROOT=Path(__file__).resolve().parents[1]
EXE=ENGINE
W,H=16,12


def normal(value):
    return ''.join(c for c in unicodedata.normalize('NFC',value.casefold()) if c.isalpha())


def correspondence(reference, words):
    """Independent word edit-distance path, retaining insertions/deletions explicitly."""
    a=[normal(w['text']) for w in reference];b=[normal(w['text']) for w in words]
    table=[[0]*(len(b)+1) for _ in range(len(a)+1)]
    for i in range(len(a)+1):table[i][0]=i
    for j in range(len(b)+1):table[0][j]=j
    for i in range(1,len(a)+1):
        for j in range(1,len(b)+1):
            table[i][j]=min(table[i-1][j]+1,table[i][j-1]+1,table[i-1][j-1]+(a[i-1]!=b[j-1]))
    i,j=len(a),len(b);pairs=[];missing=[];extra=[]
    while i or j:
        if i and j and table[i][j]==table[i-1][j-1]+(a[i-1]!=b[j-1]):
            pairs.append((i-1,j-1));i-=1;j-=1
        elif i and table[i][j]==table[i-1][j]+1:missing.append(i-1);i-=1
        else:extra.append(j-1);j-=1
    return {'distance':table[-1][-1],'rate':table[-1][-1]/len(a),'pairs':list(reversed(pairs)),
            'missing_reference_indices':list(reversed(missing)),'extra_word_indices':list(reversed(extra))}


def stats(values):
    values=sorted(values)
    return {'maximum_ms':max(values)*1000,'p95_ms':values[math.ceil(.95*len(values))-1]*1000,'mean_ms':sum(values)/len(values)*1000}


def run(root,setup):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,scratch,store=[root/name for name in ['sources','output','scratch','store']]
    for folder in [sources,output,scratch,store]:folder.mkdir()
    runtime=json.loads(setup.read_text(encoding='utf-8'))
    assert set(runtime)=={'distribution','python','python_paths','model','alignment_roots','threads'}
    speech=generate(root/'speech')
    passed=[];quality=[];results={};parents={};requests={};frames=samples=previews=rejected=0
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=120).stdout
    def call(request,error=None,timeout=220):
        nonlocal rejected
        value=subprocess.run([str(EXE)],input=json.dumps(request).encode(),capture_output=True,timeout=timeout)
        result=json.loads(value.stdout)
        if error:
            assert value.returncode==1 and result['error']['code']==error,(error,result)
            rejected+=1;return result
        if value.returncode:(root/'failed-request.json').write_text(json.dumps(request,indent=2),encoding='utf-8')
        assert value.returncode==0 and result['ok'],result
        return result['result']
    def preserved():
        assert not list(scratch.iterdir()),list(scratch.iterdir())
        for data in parents.values():assert hashlib.sha256(data['path'].read_bytes()).hexdigest()==data['identity']['sha256']
    def native_source(name,fixture):
        # A source-clock offset not divisible by three tests exact parent anchoring.
        lead=48007;duration=len(fixture['pcm'])*3
        up=ff(['-i',str(fixture['path']),'-af','aresample=48000:resampler=swr:dither_method=none:filter_size=32:phase_shift=10',
            '-ac','1','-f','s16le','-'])
        assert len(up)==duration*2
        mono=array('h',up);count=math.ceil((lead+duration+19200)/1920)*1920
        padded=array('h',[0]*lead);padded.extend(mono);padded.extend([0]*(count-len(padded)))
        stereo=array('h',(v for x in padded for v in (x,x)))
        n=count//1920
        rgb=[bytes(v for y in range(H) for x in range(W) for v in ((k*11+x*17)%256,(k*7+y*29)%256,(k*19+x+y)%256)) for k in range(n)]
        raw=sources/(name+'.rgb');audio=sources/(name+'.pcm');movie=sources/(name+'.mkv')
        raw.write_bytes(b''.join(rgb));audio.write_bytes(stereo.tobytes())
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),
            '-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3','-slices','4',
            '-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
        identity={'bytes':movie.stat().st_size,'sha256':hashlib.sha256(movie.read_bytes()).hexdigest()}
        return {'path':movie,'identity':identity,'lead':lead,'length':duration+(1 if name=='holdout-en' else 0),
            'count':count,'frames':rgb,'pcm':stereo,'n':n}
    for name,fixture in speech.items():
        parent=native_source(name,fixture);parents[name]=parent
        local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots'][fixture['language']]
        request={'command':'transcript.transcribe','id':name,'source':{'path':parent['path'].name,'identity':parent['identity'],'duration':time(parent['count'],48000)},
            'format':{'type':'reference_movie','width':W,'height':H},'start':time(parent['lead'],48000),'duration':time(parent['length'],48000),
            'channel':'mean','language':fixture['language'],'input_root':str(sources),'scratch_root':str(scratch),'runtime':local,'timeout_seconds':180}
        requests[name]=request
        (root/(name+'-request.json')).write_text(json.dumps(request,indent=2)+'\n',encoding='utf-8')
        started=clock.monotonic();result=call(request);elapsed=clock.monotonic()-started
        results[name]=result
        (root/(name+'-transcription.json')).write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        doc=result['document'];words=doc['words'];reference=fixture['reference'];origin=F(parent['lead'],48000)
        assert doc['range_start']==request['start'] and doc['source']==request['source'] and result['review_required']
        assert result['analysis']['sample_count']==(parent['length']+2)//3
        assert result['analysis']['last_sample_clamp_48000']==result['analysis']['sample_count']*3-parent['length']
        assert result['analysis']['partial_tail_padding_16000']==(1 if name=='holdout-en' else 0)
        assert doc['recognition']['worker_sha256']==hashlib.sha256((ROOT/'tools/transcribe_worker.py').read_bytes()).hexdigest()
        assert doc['recognition']['supervisor_sha256']==hashlib.sha256((ROOT/'tools/transcribe_supervisor.py').read_bytes()).hexdigest()
        assert all(w['origin']=='estimated' and 'alignment' in w for w in words)
        match=correspondence(reference,words)
        assert not match['missing_reference_indices'] and not match['extra_word_indices'],match
        contextual_start=stats([abs(float(seconds(words[j]['start'])-origin)-reference[i]['start']) for i,j in match['pairs']])
        acoustic_start=stats([abs(float(seconds(words[j]['alignment']['acoustic_start'])-origin)-reference[i]['start']) for i,j in match['pairs']])
        assert contextual_start['maximum_ms']<=250 and contextual_start['p95_ms']<=150,(name,contextual_start)
        item={'fixture':name,'words':len(words),'word_errors':match,'contextual_start':contextual_start,'raw_acoustic_start':acoustic_start,
            'seconds':elapsed,'cpu_bytes':result['worker']['peak_cpu_bytes'],'cuda_bytes':result['worker']['peak_cuda_bytes']}
        if name.startswith('isolated'):
            end=stats([abs(float(seconds(words[j]['end'])-origin)-reference[i]['end']) for i,j in match['pairs']])
            assert end['maximum_ms']<=150 and end['p95_ms']<=100,(name,end)
            item['contextual_end']=end
        else:assert match['rate']<=.10,(name,match)
        assert result['worker']['network_interfaces']==['lo'] and result['worker']['python_network_attempts']==0
        quality.append(item);preserved();print(json.dumps({k:v for k,v in item.items() if k!='word_errors'}),flush=True)
    passed.append('transcription.offline_languages_independent_word_clocks')

    def decode(path,expected,label):
        nonlocal frames,samples
        rgb,pcm=expected
        assert ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==rgb,(label,'pixels')
        assert ff(['-i',str(path),'-vn','-f','s16le','-'])==pcm,(label,'audio')
        frames+=len(rgb)//(W*H*3);samples+=len(pcm)//4;preserved()
    for language in ['en','el']:
        name='isolated-'+language;parent=parents[name];fixture=speech[name];result=results[name];doc=result['document'];n=parent['n']
        project=call({'command':'project.create','id':'spoken-'+language,'width':W,'height':H,'frame_rate':time(25)})
        project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':[
            {'op':'media.add','asset':{'id':'voice','path':str(parent['path']),'duration':time(n,25),'identity':parent['identity']}},
            edit('create',duration=time(n-5,25)),edit('add',track=track('v','video')),edit('add',track=track('a','audio')),
            edit('place',track_id='v',clip=placement('video','voice',10,n-25,20),collision='reject'),
            edit('place',track_id='a',clip=placement('audio','voice',10*1920+7,(n-25)*1920,20*1920+7,48000),collision='reject'),
            edit('link',id='av',clip_ids=['video','audio'])]})
        corrections=[];boundaries=[]
        for word,reference in zip(doc['words'],fixture['reference']):
            start=F(parent['lead'],48000)+F(round(reference['start']*16000),16000)
            end=F(parent['lead'],48000)+F(round(reference['end']*16000),16000)
            a=math.floor(start*25);b=math.ceil(end*25);boundaries.append((a,b))
            corrections.append({'op':'replace','word':{'id':word['id'],'text':reference['text'],'start':time(a,25),'end':time(b,25)}})
        corrected=call({'command':'transcript.correct','document':doc,'expected_fingerprint':result['fingerprint'],'edits':corrections,'input_root':str(sources)})
        assert corrected['document']['recognition']==doc['recognition']
        assert all(w['origin']=='corrected' and 'alignment' not in w for w in corrected['document']['words'])
        selection=[1,4]
        spec={'clip_id':'video','track_ids':['v'],'ranges':[{'first_id':doc['words'][i]['id'],'last_id':doc['words'][i]['id']} for i in selection],
            'clock':'video','rounding':'strict','estimates':'reject','collateral':'reject','links':'include','end_policy':'resize','transitions':'reject_affected'}
        rgb=bytes(10*W*H*3)+b''.join(parent['frames'][20:n-5])+bytes(10*W*H*3)
        pcm=bytes((10*1920+7)*4)+parent['pcm'][(20*1920+7)*2:((n-5)*1920+7)*2].tobytes()+bytes((10*1920-7)*4)
        base_rgb,base_pcm=rgb,pcm
        cuts=[(boundaries[i][0]-10,boundaries[i][1]-10) for i in selection]
        for a,b in sorted(cuts,reverse=True):
            rgb=rgb[:a*W*H*3]+rgb[b*W*H*3:];pcm=pcm[:a*1920*4]+pcm[b*1920*4:]
        client=Client(EXE)
        try:
            client.initialize();client.call('session.create',store_root=str(store),project=project,request_id='create')
            common={'store_root':str(store),'project_id':project['id']};saved=client.call('session.get',**common)
            estimated_spec={**spec,'rounding':'outward','estimates':'allow_reported'}
            estimated=client.call('transcript.plan',project=saved,document=doc,expected_revision=0,
                expected_document_fingerprint=result['fingerprint'],spec=estimated_spec,input_root=str(sources))
            assert set(estimated['estimated_word_ids'])=={doc['words'][i]['id'] for i in selection}
            estimated_cuts=[(math.floor(seconds(doc['words'][i]['start'])*25)-10,
                math.ceil(seconds(doc['words'][i]['end'])*25)-10) for i in selection]
            assert [(seconds(c['start'])*25,seconds(c['end'])*25) for c in estimated['merged_cuts']]==estimated_cuts
            assert estimated_cuts!=cuts,'Independent caller correction must change this fixture cut'
            call({'command':'transcript.plan','project':saved,'document':doc,'expected_revision':0,
                'expected_document_fingerprint':result['fingerprint'],'spec':{**estimated_spec,'estimates':'reject'},
                'input_root':str(sources)},error='ESTIMATED_BOUNDARY')
            estimated_rgb,estimated_pcm=base_rgb,base_pcm
            for a,b in sorted(estimated_cuts,reverse=True):
                estimated_rgb=estimated_rgb[:a*W*H*3]+estimated_rgb[b*W*H*3:]
                estimated_pcm=estimated_pcm[:a*1920*4]+estimated_pcm[b*1920*4:]
            estimated_project=call({'command':'timeline.apply','project':saved,'expected_revision':0,'operations':[estimated['operation']]})
            path=output/(name+'-estimated.mkv');call({'command':'render.run','project':estimated_project,
                'input_root':str(sources),'output_root':str(output),'output':str(path)})
            decode(path,(estimated_rgb,estimated_pcm),name+'-estimated')
            planned=client.call('transcript.plan',project=saved,document=corrected['document'],expected_revision=0,
                expected_document_fingerprint=corrected['fingerprint'],spec=spec,input_root=str(sources))
            assert planned['estimated_word_ids']==[] and planned['collateral_word_ids']==[]
            assert [(seconds(c['start'])*25,seconds(c['end'])*25) for c in planned['merged_cuts']]==cuts
            fields={**common,'expected_revision':0,'request_id':'remove-words','operations':[planned['operation']]}
            preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'})
            assert preview['duration_after']==time(len(rgb)//(W*H*3),25)
            applied=client.call('session.apply',**fields);assert client.call('session.apply',**fields)==applied
            edited=client.call('session.get',**common)
            path=output/(name+'.mkv');call({'command':'render.run','project':edited,'input_root':str(sources),'output_root':str(output),'output':str(path)})
            decode(path,(rgb,pcm),name)
            client.call('session.undo',**common,expected_revision=1,request_id='undo')
            assert client.call('session.get',**common)['tracks']==saved['tracks']
            client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1)
            restored=client.call('session.get',**common);assert restored['tracks']==edited['tracks']
            for frame in [0,cuts[0][0],len(rgb)//(W*H*3)-1]:
                path=output/f'{name}-{frame}.png';client.call('preview.frame',project=restored,input_root=str(sources),output_root=str(output),output=str(path),time=time(frame,25))
                with Image.open(path) as image:assert image.tobytes()==rgb[frame*W*H*3:(frame+1)*W*H*3]
                previews+=1
        finally:client.close()
    passed.append('transcription.recognized_corrections_saved_rendered_cuts')
    (root/'editing-verification.json').write_text(json.dumps({'passed':passed,'quality':quality,'frames_compared':frames,
        'stereo_sample_frames_compared':samples,'previews':previews,'rejected_cases':rejected},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    long_cases=maximum(root/'maximum input',speech,runtime,call,ff,correspondence,stats)
    passed.append('transcription.maximum_channels_resources_and_exact_clock')
    failure_cases=failures(root/'failures',requests['holdout-en'],runtime,call,preserved)
    passed.append('transcription.native_rejection_cancellation_and_owner_lifetime')
    guard=guard_checks(root/'guard',runtime)
    passed.append('transcription.supervisor_protocol_and_detached_descendants')
    preserved()
    report={'passed':passed,'quality':quality,'frames_compared':frames,'stereo_sample_frames_compared':samples,'previews':previews,'rejected_cases':rejected,
        'maximum_inputs':long_cases,'native_failures':failure_cases,'supervisor':guard,
        'gates':{'contextual_onset_max_ms':250,'contextual_onset_p95_ms':150,'isolated_end_max_ms':150,'isolated_end_p95_ms':100},
        'scope':'Pinned optional WSL/CUDA English/Greek profile, six short fixtures, full 120-second WAV parents, native movies, explicit contextual intervals and reviewed corrections; raw phonetic timing and arbitrary natural speech accuracy are not claimed'}
    (root/'verification.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    print(json.dumps({k:v for k,v in report.items() if k not in ['quality','maximum_inputs','native_failures','supervisor']},indent=2))
    return report


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    parser.add_argument('--runtime',type=Path,default=os.environ.get('CUTBOLT_TRANSCRIPTION_RUNTIME'))
    args=parser.parse_args()
    if args.runtime is None:raise SystemExit('Set CUTBOLT_TRANSCRIPTION_RUNTIME to an external runtime configuration JSON; no optional-backend skipping is allowed.')
    run(args.output,Path(args.runtime))
