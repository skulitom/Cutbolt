"""Maximum native inputs, original negative fixtures and owner cancellation."""
from engine import BUILD, ENGINE
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
import math
from pathlib import Path
import subprocess
import time as clock
import wave
from tracks import time, seconds
from transcription_guard import linux, processes, no_namespaces, wsl

ROOT=Path(__file__).resolve().parents[1]
EXE=ENGINE


def maximum(root,speech,runtime,call,ff,correspondence,stats):
    root.mkdir();records=[]
    segmentation=wsl(runtime,"""import json,runpy,sys
sys.path[:0]=json.loads(sys.argv[2])
import numpy as np
blocks=runpy.run_path(sys.argv[1])['recognition_blocks'];out=[]
for count in [400,480000,480001,1920000]:
 result=blocks(np.full(count,.1,dtype=np.float32),np)
 assert result[0][0]==0 and result[-1][1]==count
 assert all(a[1]==b[0] for a,b in zip(result,result[1:]))
 if count<=480000:assert result==[(0,count,'source_end')]
 else:
  assert all(0<b-a<=240000 for a,b,_ in result)
  assert all(p=='hard_12s' for _,_,p in result[:-1])
 out.append({'sample_count':count,'blocks':result})
worker=runpy.run_path(sys.argv[1]);speech,gap=worker['speech'],worker['word_gap']
w=lambda t,a,b,p=.9:{'word':t,'start':a,'end':b,'probability':p}
# Annotations and music symbols are notes, not words; a glued "-" joins its word.
kept,notes=speech([w(' A',0,.2),w(' f',.3,.4,.4),w('-',.4,.4,.3),w(' [Music]',1,2),w(' (upbeat',2,2.5),w(' music)',2.5,3),
 w(' \\u266a',3,3.5),w(' ...',3.6,3.7),w(' (and',4,4.2),w(' then',4.2,4.4)],0,80000)
assert [(k['word'],k['probability']) for k in kept]==[('A',.9),('f-',.3),('(and',.9),('then',.9)],kept
assert [(n['kind'],n['text'],n['start_sample'],n['end_sample']) for n in notes]==[('annotation','[Music]',16000,32000),
 ('annotation','(upbeat music)',32000,48000),('music','\\u266a',48000,56000),('symbols','...',57600,59200)],notes
# Without a quiet gap the cut moves to the widest gap between recognized words in 8-14 s.
assert gap([w(' a',7.,9.),w(' b',9.6,13.),w(' c',13.,14.)],0)==(144000+153600)//2
assert gap([],0)==(128000+224000)//2 and gap([w(' a',7.,14.)],0) is None
print(json.dumps(out))
""",linux(ROOT/'tools/transcribe_worker.py'),json.dumps(runtime['python_paths']))
    (root/'segmentation.json').write_text(json.dumps(segmentation,indent=2)+'\n',encoding='utf-8')
    for language,channel in [('en','left'),('el','right')]:
        fixture=speech['holdout-'+language]
        up=ff(['-i',str(fixture['path']),'-af','aresample=48000:resampler=swr:dither_method=none:filter_size=32:phase_shift=10','-f','s16le','-'])
        mono=array('h',up);block=mono+array('h',[0]*48000);count=120*48000;repeats=count//len(block)
        full=block*repeats;full.extend([0]*(count-len(full)))
        # Only the explicitly selected channel carries speech.
        pcm=array('h',(v for sample in full for v in ((sample,0) if channel=='left' else (0,sample))))
        source=root/(language+' channel source.wav')
        with wave.open(str(source),'wb') as stream:
            stream.setparams((2,2,48000,count,'NONE','not compressed'));stream.writeframes(pcm.tobytes())
        identity={'bytes':source.stat().st_size,'sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
        scratch=root/('scratch '+language);scratch.mkdir()
        local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots'][language]
        request={'command':'transcript.transcribe','id':'maximum-'+language,'source':{'path':source.name,'identity':identity,'duration':time(120)},
            'format':{'type':'stereo_wav'},'start':time(0),'duration':time(120),'channel':channel,'language':language,
            'input_root':str(root),'scratch_root':str(scratch),'runtime':local,'timeout_seconds':300}
        started=clock.monotonic();result=call(request,timeout=340)
        (root/(language+'-result.json')).write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
        doc=result['document'];worker=result['worker'];words=doc['words']
        reference=[{'text':word['text'],'start':word['start']+i*len(block)/48000} for i in range(repeats) for word in fixture['reference']]
        match=correspondence(reference,words)
        assert match['rate']<=.10,(language,match)
        # These statistics retain mismatched pairs and do not claim perfect long
        # word alignment when recognition inserted or omitted repeated words.
        onsets=stats([abs(float(seconds(words[j]['start']))-reference[i]['start']) for i,j in match['pairs']])
        assert onsets['maximum_ms']<=250 and onsets['p95_ms']<=150,(language,onsets)
        assert result['analysis']['sample_count']==1920000 and result['analysis']['last_sample_clamp_48000']==0
        assert result['analysis']['channel']==channel and doc['range_start']==time(0) and doc['range_duration']==time(120)
        assert len(worker['alignment_blocks'])==5
        windows=worker['recognition_blocks']
        assert len(windows)>=4 and windows[0]['start_sample']==0 and windows[-1]['end_sample']==1920000
        assert all(0<w['end_sample']-w['start_sample']<=240000 for w in windows)
        assert all(a['end_sample']==b['start_sample'] and a['end_word']==b['first_word'] for a,b in zip(windows,windows[1:]))
        assert windows[0]['first_word']==0 and windows[-1]['end_word']==len(words)
        assert all(w['end_policy']=='quiet_gap' for w in windows[:-1]) and windows[-1]['end_policy']=='source_end'
        for window in windows:
            for word in worker['words'][window['first_word']:window['end_word']]:
                assert window['start_sample']<=word['ctc_start_sample']<word['ctc_end_sample']<=window['end_sample']
        assert worker['alignment_blocks'][0]['first_frame']==0 and worker['alignment_blocks'][-1]['end_frame']==5999
        assert all(a['end_frame']==b['first_frame'] for a,b in zip(worker['alignment_blocks'],worker['alignment_blocks'][1:]))
        assert worker['peak_cpu_bytes']<=6*1024**3 and worker['peak_cuda_bytes']<=8*1024**3
        assert worker['network_interfaces']==['lo'] and worker['python_network_attempts']==0
        assert not list(scratch.iterdir()) and hashlib.sha256(source.read_bytes()).hexdigest()==identity['sha256']
        record={'language':language,'channel':channel,'seconds':clock.monotonic()-started,'words':len(words),'reference_words':len(reference),
            'word_errors':match,'paired_contextual_start':onsets,'cpu_bytes':worker['peak_cpu_bytes'],'cuda_bytes':worker['peak_cuda_bytes'],
            'recognition_blocks':windows,
            'full_source_clock_samples':count,'source_preserved':True,'scratch_removed':True}
        records.append(record);print(json.dumps({k:v for k,v in record.items() if k!='word_errors'}),flush=True)
    return records


def music_bed(root,speech,runtime,call,ff,correspondence):
    """Isolated words over a steady two-tone bed: no RMS-quiet gap anywhere, so recognition
    windows must end between recognized words rather than at a fixed 12 s cut."""
    root.mkdir();fixture=speech['isolated-en']
    up=ff(['-i',str(fixture['path']),'-af','aresample=48000:resampler=swr:dither_method=none:filter_size=32:phase_shift=10','-f','s16le','-'])
    block=array('h',up);count=60*48000;repeats=count//len(block)
    mono=block*repeats;mono.extend([0]*(count-len(mono)))
    step=[2*math.pi*f/48000 for f in (110,165)];pcm=array('h')
    for n,v in enumerate(mono):
        x=max(-32768,min(32767,v+round(980*math.sin(step[0]*n)+980*math.sin(step[1]*n))));pcm.extend((x,x))
    source=root/'speech over bed.wav'
    with wave.open(str(source),'wb') as stream:
        stream.setparams((2,2,48000,count,'NONE','not compressed'));stream.writeframes(pcm.tobytes())
    identity={'bytes':source.stat().st_size,'sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
    scratch=root/'scratch';scratch.mkdir()
    local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots']['en']
    request={'command':'transcript.transcribe','id':'bed','source':{'path':source.name,'identity':identity,'duration':time(60)},
        'format':{'type':'stereo_wav'},'start':time(0),'duration':time(60),'channel':'mean','language':'en',
        'input_root':str(root),'scratch_root':str(scratch),'runtime':local,'timeout_seconds':300}
    started=clock.monotonic();result=call(request,timeout=340)
    (root/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    words=result['document']['words'];windows=result['worker']['recognition_blocks'];period=len(block)/48000
    spoken=[(w['start']+i*period,w['end']+i*period) for i in range(repeats) for w in fixture['reference']]
    reference=[{'text':w['text'],'start':w['start']+i*period} for i in range(repeats) for w in fixture['reference']]
    assert len(windows)>=4 and all(w['end_policy']=='word_gap' for w in windows[:-1]) and windows[-1]['end_policy']=='source_end',windows
    cuts=[w['end_sample']/16000 for w in windows[:-1]]
    assert not [(c,a,b) for c in cuts for a,b in spoken if a<c<b],(cuts,spoken)
    match=correspondence(reference,words)
    assert match['rate']<=.10,match
    assert not list(scratch.iterdir()) and hashlib.sha256(source.read_bytes()).hexdigest()==identity['sha256']
    record={'seconds':clock.monotonic()-started,'words':len(words),'reference_words':len(reference),'word_error_rate':match['rate'],
        'recognition_blocks':windows,'source_preserved':True,'scratch_removed':True}
    print(json.dumps({k:v for k,v in record.items() if k!='recognition_blocks'}),flush=True)
    return record


CANCEL_HELPER=r'''use cutbolt::{media::Control, transcribe::{Transcribe,run_controlled}, error};
use std::{path::PathBuf,io::{self,Read}};
struct Cancel(PathBuf);
impl Control for Cancel {
 fn check(&self)->cutbolt::Result<()> {
  if self.0.exists(){Err(error("CANCELLED","Fixture owner cancelled recognition"))}else{Ok(())}
 }
}
fn main(){
 let mut bytes=Vec::new();io::stdin().read_to_end(&mut bytes).unwrap();
 let request:Transcribe=serde_json::from_slice(&bytes).unwrap();
 let result=run_controlled(&request,&Cancel(std::env::args_os().nth(1).unwrap().into()));
 match result {
  Ok(value)=>{println!("{}",serde_json::json!({"ok":true,"result":value}));},
  Err(err)=>{println!("{}",serde_json::json!({"ok":false,"error":err}));std::process::exit(1);}
 }
}
'''


def failures(root,request,runtime,call,preserved):
    root.mkdir();records=[]
    def reject(label,change,code):
        candidate=copy.deepcopy(request);change(candidate);started=clock.monotonic()
        result=call(candidate,error=code,timeout=50);preserved()
        records.append({'case':label,'seconds':clock.monotonic()-started,'error':result['error']})
        print(json.dumps({'rejected':label,'code':code}),flush=True)
    def field(path,value):
        def change(candidate):
            for key in path[:-1]:candidate=candidate[key]
            candidate[path[-1]]=value
        return change
    cases=[('too-long',['duration'],time(120*48000+1,48000),'INVALID_TRANSCRIPTION'),
        ('too-short',['duration'],time(1199,48000),'INVALID_TRANSCRIPTION'),
        ('outside-source',['start'],request['source']['duration'],'INVALID_TRANSCRIPTION'),
        ('nonreduced-clock',['start'],{'num':0,'den':48000},'INVALID_TRANSCRIPTION'),
        ('timeout-zero',['timeout_seconds'],0,'INVALID_TRANSCRIPTION'),('timeout-bound',['timeout_seconds'],601,'INVALID_TRANSCRIPTION'),
        ('thread-zero',['runtime','threads'],0,'INVALID_TRANSCRIPTION'),('thread-bound',['runtime','threads'],9,'INVALID_TRANSCRIPTION'),
        ('distribution-syntax',['runtime','distribution'],'bad distribution','INVALID_TRANSCRIPTION'),
        ('python-relative',['runtime','python'],'python3','INVALID_TRANSCRIPTION'),
        ('packages-relative',['runtime','python_paths'],['../packages'],'INVALID_TRANSCRIPTION'),
        ('unknown-channel',['channel'],'automatic','INVALID_JSON'),('unknown-language',['language'],'auto','INVALID_JSON'),
        ('unknown-format',['format'],{'type':'arbitrary'},'INVALID_JSON'),('unknown-worker',['worker'],'override.py','INVALID_JSON'),
        ('source-escape',['source','path'],'../escape.mkv','INVALID_TRANSCRIPTION'),
        ('changed-parent',['source','identity','sha256'],'0'*64,'MEDIA_CHANGED'),
        ('wrong-duration',['source','duration'],time(seconds(request['source']['duration'])+1),'MEDIA_DURATION_MISMATCH'),
        ('missing-model',['runtime','model'],str(root/'missing.model'),'MODEL_UNAVAILABLE'),
        ('missing-alignment',['runtime','alignment_root'],str(root/'missing-alignment'),'MODEL_UNAVAILABLE')]
    for label,path,value,code in cases:reject(label,field(path,value),code)
    wrong=root/'original-invalid-model';wrong.write_bytes(b'original invalid model fixture')
    reject('changed-model',field(['runtime','model'],str(wrong)),'MODEL_CHANGED')
    empty=root/'empty alignment';empty.mkdir()
    reject('missing-acoustic-file',field(['runtime','alignment_root'],str(empty)),'MODEL_UNAVAILABLE')
    (empty/'config.json').write_text('{}',encoding='utf-8')
    reject('changed-acoustic-file',field(['runtime','alignment_root'],str(empty)),'MODEL_CHANGED')
    # An original metadata stub shadows exactly one installed version. No model
    # package implementation is copied or modified.
    shadow=root/'version shadow';shadow.mkdir();meta=shadow/'openai_whisper-0.dist-info';meta.mkdir()
    (meta/'METADATA').write_text('Metadata-Version: 2.1\nName: openai-whisper\nVersion: 0\n',encoding='utf-8')
    reject('wrong-runtime-version',field(['runtime','python_paths'],[linux(shadow),*request['runtime']['python_paths']]),'RUNTIME_VERSION')
    reject('deadline',field(['timeout_seconds'],1),'WORKER_TIMEOUT')
    # Unknown executable is local configuration failure, and must not create any
    # published transcript or touch source media.
    reject('missing-python',field(['runtime','python'],'/nonexistent/cutbolt-python'),'WORKER_FAILED')
    # Trusted-runtime configuration may point at an explicit local launcher. This
    # original fixture hides the GPU but still executes the production Python
    # supervisor/worker and the actual installed libraries and models.
    cpu_launcher=root/'cpu-python'
    cpu_launcher.write_text('#!'+runtime['python']+'\nimport os,sys\nos.environ["CUDA_VISIBLE_DEVICES"]=""\nos.execv('
        +repr(runtime['python'])+',['+repr(runtime['python'])+',*sys.argv[1:]])\n',encoding='utf-8',newline='\n')
    reject('unavailable-cuda',field(['runtime','python'],linux(cpu_launcher)),'DEVICE_UNAVAILABLE')
    silent=root/'silent.wav'
    with wave.open(str(silent),'wb') as stream:
        stream.setparams((2,2,48000,1201,'NONE','not compressed'));stream.writeframes(bytes(1201*4))
    silent_hash=hashlib.sha256(silent.read_bytes()).hexdigest()
    def silence(candidate):
        candidate.update(source={'path':silent.name,'identity':{'bytes':silent.stat().st_size,'sha256':silent_hash},'duration':time(1201,48000)},
            format={'type':'stereo_wav'},input_root=str(root),start=time(0),duration=time(1201,48000))
    reject('digital-silence-minimum-clock',silence,'NO_WORDS')
    assert hashlib.sha256(silent.read_bytes()).hexdigest()==silent_hash
    helper=root/'cancel.rs';helper.write_text(CANCEL_HELPER,encoding='utf-8');binary=root/'cancel.exe'
    serde=max((BUILD/'deps').glob('libserde_json-*.rlib'),key=lambda p:p.stat().st_mtime)
    subprocess.run(['rustc','--edition=2024',str(helper),'-o',str(binary),'-L',str(BUILD/'deps'),
        '--extern','cutbolt='+str(BUILD/'libcutbolt.rlib'),'--extern','serde_json='+str(serde)],check=True,capture_output=True,timeout=90)
    for mode in ['library-cancel','native-owner-kill']:
        candidate=copy.deepcopy(request);owned=root/(mode+' scratch');owned.mkdir();candidate['scratch_root']=str(owned)
        cancel=root/(mode+'.signal');command=[str(EXE)]
        if mode=='library-cancel':candidate.pop('command');command=[str(binary),str(cancel)]
        child=subprocess.Popen(command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        started=clock.monotonic();namespaces=set();observed=[]
        try:
            child.stdin.write(json.dumps(candidate).encode());child.stdin.close();child.stdin=None
            deadline=clock.monotonic()+25
            while clock.monotonic()<deadline:
                observed=processes(runtime,linux(owned))
                if any(any(a.endswith('/transcribe_worker.py') for a in p['argv']) for p in observed):break
                assert child.poll() is None,'Native worker exited before cancellation observation'
                clock.sleep(.05)
            else:raise AssertionError('No live embedded worker observed')
            # unshare itself is outside its new PID namespace. Its argv also
            # matches the scratch path, so only the actual worker identifies
            # the namespace owned by this invocation.
            namespaces={p['namespace'] for p in observed if any(a.endswith('/transcribe_worker.py') for a in p['argv'])}
            (root/(mode+'-observed.json')).write_text(json.dumps(observed,indent=2),encoding='utf-8')
            if mode=='library-cancel':cancel.write_text('explicit fixture cancellation',encoding='utf-8')
            else:child.kill()
            out,err=child.communicate(timeout=20)
            if mode=='library-cancel':assert child.returncode==1 and json.loads(out)['error']['code']=='CANCELLED',(out,err)
            else:assert not out,(out,err)
            deadline=clock.monotonic()+8
            while list(owned.iterdir()) and clock.monotonic()<deadline:clock.sleep(.05)
            assert not list(owned.iterdir()),(mode,'scratch remains')
            no_namespaces(runtime,namespaces);preserved()
            records.append({'case':mode,'seconds':clock.monotonic()-started,'live_worker_observed':True,'surviving_owned_processes':0,
                'scratch_removed':True,'source_preserved':True})
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=5)
    (root/'verification.json').write_text(json.dumps(records,indent=2)+'\n',encoding='utf-8')
    return records
