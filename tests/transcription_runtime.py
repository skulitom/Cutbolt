"""Maximum native inputs, original negative fixtures and owner cancellation."""
from engine import BUILD, ENGINE
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess
import time as clock
import wave
from tracks import time, seconds
from transcription_guard import linux, processes, no_namespaces, wsl
from transcription_speech import lines

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


def music_tail(root,runtime,call,ff,correspondence):
    """A named line that ends in 26 s of the steady bed, recognized with the names as a vocabulary.
    Prompted with the terms, the recognizer writes them out over windows that hold only music
    ("PixelForge, Cutbolt." in the part-two demo's end card). The acoustic model hears nothing
    there, so those windows are not decoded: every word lies in the line, which keeps its names."""
    root.mkdir();spoken=lines(root/'speech',{'names':NAMES})
    up=ff(['-i',str(spoken['names']['path']),'-af','aresample=48000:resampler=swr:dither_method=none:filter_size=32:phase_shift=10','-f','s16le','-'])
    voice=array('h',up);count=len(voice)+26*48000
    step=[2*math.pi*f/48000 for f in (110,165)];pcm=array('h')
    for v in voice:pcm.extend((v,v))
    for n in range(count-len(voice)):
        x=round(980*math.sin(step[0]*n)+980*math.sin(step[1]*n));pcm.extend((x,x))
    source=root/'line then music.wav'
    with wave.open(str(source),'wb') as stream:
        stream.setparams((2,2,48000,count,'NONE','not compressed'));stream.writeframes(pcm.tobytes())
    identity={'bytes':source.stat().st_size,'sha256':hashlib.sha256(source.read_bytes()).hexdigest()}
    scratch=root/'scratch';scratch.mkdir();terms=['PixelForge','Cutbolt']
    local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots']['en']
    request={'command':'transcript.transcribe','id':'tail','source':{'path':source.name,'identity':identity,'duration':time(count,48000)},
        'format':{'type':'stereo_wav'},'start':time(0),'duration':time(count,48000),'channel':'mean','language':'en',
        'input_root':str(root),'scratch_root':str(scratch),'runtime':local,'timeout_seconds':300,'vocabulary':terms}
    started=clock.monotonic();result=call(request,timeout=340)
    (root/'result.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    words=result['document']['words'];windows=result['worker']['recognition_blocks'];end=len(voice)/48000
    heard=' '.join(w['text'] for w in words)
    assert all(seconds(w['start'])<end for w in words),('Words were recognized over music alone',heard,windows)
    match=correspondence([{'text':e['text']} for e in spoken['names']['events']],words)
    assert not match['extra_word_indices'] and match['rate']<=.10,(heard,match)
    letters=lambda text:''.join(c for c in text.casefold() if c.isalnum())
    assert {'pixelforge','cutbolt'}<={letters(w['text']) for w in words},heard
    # The windows after the line were not decoded; the first one was.
    music=[w for w in windows if w['start_sample']>=end*16000]
    assert len(windows)>=2 and windows[0]['decoded'] and music and not any(w['decoded'] for w in music),windows
    assert not [n for n in result['non_speech'] if n['kind']=='unheard'],result['non_speech']
    assert not list(scratch.iterdir()) and hashlib.sha256(source.read_bytes()).hexdigest()==identity['sha256']
    record={'seconds':clock.monotonic()-started,'line_seconds':end,'heard':heard,'recognition_blocks':windows,
        'non_speech':result['non_speech'],'source_preserved':True,'scratch_removed':True}
    print(json.dumps({k:v for k,v in record.items() if k!='recognition_blocks'},ensure_ascii=False),flush=True)
    return record


NAMES='This film was drawn with PixelForge, then cut and mixed by an agent using Cutbolt.'
FILLER='Keep the blue circle, um, and remove the green triangle.'
WORKER_RULES="""import json,runpy,sys
sys.path[:0]=json.loads(sys.argv[2])
import numpy as np
worker=runpy.run_path(sys.argv[1])
respell,prompt_of,reading,uncovered=(worker[k] for k in ('respell','prompt_of','reading','uncovered'))
w=lambda t,a=0.,b=.1,p=.9:{'word':t,'start':a,'end':b,'probability':p}
said=lambda words:[x['word'] for x in words]
assert prompt_of(None) is None and prompt_of(['um','uh','PixelForge'])=='um, uh, PixelForge.'
# Split and differently spelled names become the term, keeping the outer punctuation.
out,n=respell([w('with'),w('pixel',1.,1.2,.8),w('forge,',1.2,1.5,.6),w('using'),w('cut-bolt.')],['PixelForge','Cutbolt'])
assert said(out)==['with','PixelForge,','using','Cutbolt.'] and n==2,out
assert (out[1]['start'],out[1]['end'],out[1]['probability'])==(1.,1.5,.6),out
assert said(respell([w('"Pixel'),w('Forge"')],['PixelForge'])[0])==['"PixelForge"']
# Punctuation between tokens, other letters, many-word terms and a capitalized um stay as heard.
for words,terms in (([w('pixel,'),w('forge')],['PixelForge']),([w('cut'),w('bolts')],['Cutbolt']),([w('new'),w('york')],['New York']),([w('Um,')],['um'])):
 assert respell(words,terms)==(words,0),(words,terms)
assert said(respell([w('UM')],['um'])[0])==['um'] and said(respell([w('cut'),w('bolt'),w('cutters')],['Cutbolt'])[0])==['Cutbolt','cutters']
# A filler between two words: letters the acoustic model reads where no word is, grown over
# its voiced audio. A quiet murmur is background, covered letters belong to a word, and a sound
# that runs on into a word without a dip is left to it as its onset.
names={0:'<pad>',1:'|',2:'U',3:'M',4:'A',5:'B'};letter=np.array([False,False,True,True,True,True])
pcm=np.zeros(16000,dtype=np.float32);t=np.arange(16000)/16000
for a,b,level in ((.1,.3,.3),(.5,.7,.3),(.8,.85,.01),(.9,.98,.3)):
 pcm[int(a*16000):int(b*16000)]=level*np.sin(2*np.pi*220*t[int(a*16000):int(b*16000)])
best=np.zeros(49,dtype=np.int64);best[5:15]=4;best[45:49]=5;best[26:29]=2;best[31:34]=3;best[41]=4;best[24]=1
raw=[(1600,4880),(14400,15760)]
found=uncovered(pcm,best,letter,1,names,[(5,15),(45,49)],raw,np)
assert found==[{'start_sample':8000,'end_sample':11200,'letters':'UM'}],found
assert uncovered(pcm,best,letter,1,names,[(5,15),(24,35),(45,49)],raw,np)==[]
onset=pcm.copy();onset[12800:14400]=.3*np.sin(2*np.pi*220*t[12800:14400])
assert uncovered(onset,best,letter,1,names,[(5,15),(45,49)],raw,np)==found
assert reading([2,2,0,2,1,1,3,0],letter,1,names)=='UU M' and reading([4]*70,letter,1,names)=='A'
long=reading([4,0]*70,letter,1,names);assert len(long)==64 and long[:63]=='A'*63
# Numerals align as the English words they are read as (the shared table), other words exactly as
# before; the Greek profile and number signs other than digits reject, naming the word.
spoken,labels_of,Failure=(worker[k] for k in ('spoken','labels_of','Failure'))
table=json.loads(open(sys.argv[3],encoding='utf-8').read())
for text,words in table.items():assert spoken(text)==words,(text,spoken(text),words)
english={c:i for i,c in enumerate("|'ABCDEFGHIJKLMNOPQRSTUVWXYZ")};named={i:c for c,i in english.items()}
read=lambda labels:''.join(named[n] for n in labels)
assert read(labels_of('80-second,',english,'en',False))=='EIGHTY|SECOND' and read(labels_of('170',english,'en',True))=='ONE|HUNDRED|SEVENTY'
assert read(labels_of('9:00',english,'en',False))=="NINE|O'CLOCK" and read(labels_of('Caf\\u00e9,',english,'en',False))=='CAFE'
greek={c:i for i,c in enumerate('|\\u0391\\u0393\\u03a4')}
assert labels_of('\\u03b3\\u03ac\\u03c4\\u03b1',greek,'el',True)==[greek[c] for c in '\\u0393\\u0391\\u03a4\\u0391']
for text,vocab,language,given,needle in (('80',greek,'el',False,'known text'),('3',greek,'el',True,'Greek words'),
  ('1\\u00bd',english,'en',False,'\\u00bd'),('m\\u00b2',english,'en',True,'\\u00b2')):
 try:labels_of(text,vocab,language,given)
 except Failure as error:assert error.code=='UNSUPPORTED_ALIGNMENT_TEXT' and repr(text)[1:-1] in error.message and needle in error.message,error.message
 else:raise AssertionError(text)
# Speech evidence: a frame hears speech when its most likely acoustic label is not the blank. Frames
# are centred every 320 samples from sample 200. A segment none of whose words' CTC spans hears
# speech (the prompt written over music) is dropped with an unheard note, and the windows' word
# indices follow; a segment with one heard word is kept whole.
frame_at,unheard=worker['frame_at'],worker['unheard']
assert [frame_at(n,10) for n in (0,200,201,520,521,99999)]==[0,0,1,1,2,10]
heard=np.zeros(40,dtype=bool);heard[2:6]=True;heard[30]=True
said_at=lambda t,segment,a,b:{**w(t),'segment':segment,'at':(a,b)}
words=[said_at('Hello',0,0,1600),said_at('there.',0,1600,3200),said_at('Pip,',1,16000,19200),said_at('PixelForge.',1,19200,24000),
 said_at('Bye.',2,32000,35200)]
windows=[{'start_sample':0,'end_sample':24000,'first_word':0,'end_word':4},{'start_sample':24000,'end_sample':40000,'first_word':4,'end_word':5}]
kept,renumbered,notes=unheard(words,windows,[(2,4),(6,9),(10,14),(14,20),(28,32)],heard)
assert said(kept)==['Hello','there.','Bye.'] and [(x['first_word'],x['end_word']) for x in renumbered]==[(0,2),(2,3)],(kept,renumbered)
assert notes==[{'text':'Pip, PixelForge.','kind':'unheard','start_sample':16000,'end_sample':24000}],notes
one=[{**windows[0],'end_word':2}];assert unheard(words[:2],one,[(2,4),(6,9)],heard)==(words[:2],one,[])
print(json.dumps({'uncovered':found,'spoken_numbers':len(table)}))
"""


def vocabulary_and_uncovered(root,runtime,call):
    """A vocabulary makes the recognizer spell names whole, and speech no word covers (an "um" the
    script leaves out) is reported as an uncovered sound, never as a word, which transcript.fillers
    lists and, when asked, cuts between its neighbours."""
    root.mkdir()
    rules=wsl(runtime,WORKER_RULES,linux(ROOT/'tools/transcribe_worker.py'),json.dumps(runtime['python_paths']),
        linux(ROOT/'tests/support/spoken_numbers.json'))
    spoken=lines(root/'speech',{'names':NAMES,'filler':FILLER})
    local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots']['en']
    def transcribe(name,label,**fields):
        saved=root/(label+'.json')
        result=call({'command':'media.transcribe','path':str(spoken[name]['path']),'input_root':str(root/'speech'),'output_root':str(root),
            'output':str(saved),'runtime':local,'language':'en','id':name,**fields},timeout=600)
        return result,json.loads(saved.read_text(encoding='utf-8'))['transcripts']
    letters=lambda text:''.join(c for c in text.casefold() if c.isalnum())
    terms=['PixelForge','Cutbolt']
    plain,plain_docs=transcribe('names','names-plain')
    named,named_docs=transcribe('names','names-vocabulary',vocabulary=terms)
    heard=[w['text'] for d in named_docs for w in d['words']]
    assert {'pixelforge','cutbolt'}<={letters(t) for t in heard},heard
    assert all(d['recognition']['vocabulary']==terms for d in named_docs) and named['vocabulary']==terms,named
    assert all('vocabulary' not in d['recognition'] for d in plain_docs) and plain['vocabulary']==[]
    # The script without its "um": the aligned words skip it, and the sound is left uncovered.
    script=FILLER.replace(' um,','')
    aligned,docs=transcribe('filler','filler-script',text=script)
    doc=docs[0];words={w['text']:w for w in doc['words']}
    assert [w['text'] for w in doc['words']]==script.split() and len(doc.get('uncovered',[]))==1,doc
    sound=doc['uncovered'][0];read=sound['letters'].replace(' ','')
    events={letters(e['text']):e['start'] for e in spoken['filler']['events']}
    assert re.fullmatch('[AEU]+H?[MR]*|H?M+',read),sound
    assert events['um']-.15<=seconds(sound['start'])<seconds(sound['end'])<=events['and']+.05,(sound,events)
    assert seconds(words['circle,']['end'])<=seconds(sound['start']) and seconds(sound['end'])<=seconds(words['and']['start']),(sound,words)
    assert aligned['uncovered']=={'count':1,'listed':[{'document':doc['id'],'start':sound['start'],'end':sound['end'],
        'letters':sound['letters'],'after':'circle,','before':'and'}]},aligned
    frames=int(seconds(doc['source']['duration'])*25)
    project=call({'command':'project.create','id':'filler','width':16,'height':12,'frame_rate':time(25)})
    project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':[
        {'op':'media.add','asset':{'id':'take','path':'filler.wav','duration':time(frames,25)}},
        {'op':'clip.append','clip':{'id':'c','asset_id':'take','source_in':time(0),'duration':time(frames,25)}}]})
    listed=call({'command':'transcript.fillers','project':project,'transcripts':docs})
    assert listed['operations']==[] and listed['fillers']['count']==0 and listed['uncovered']['filler_like']==1,listed
    cut=call({'command':'transcript.fillers','project':project,'transcripts':docs,'uncovered':True})
    span=cut['uncovered']['listed'][0]['cut']
    assert cut['uncovered']['cuts']==1 and len(cut['operations'])==1,cut
    assert seconds(words['circle,']['end'])<=seconds(span['start'])<seconds(span['end'])<=seconds(words['and']['start']),(span,words)
    record={'rules':rules,'names':{'terms':terms,'without_vocabulary':' '.join(w['text'] for d in plain_docs for w in d['words']),
        'with_vocabulary':' '.join(heard)},'filler':{'script':script,'uncovered':sound,'events':events,'cut':span}}
    print(json.dumps(record,ensure_ascii=False),flush=True)
    return record


NUMBERS='This morning, an eighty second video took over two hours and a hundred and seventy tool calls.'
NUMERALS='This morning, an 80-second video took over 2 hours and 170 tool calls.'
NUMBER_WORDS=set('zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen '
    'eighteen nineteen twenty thirty forty fifty sixty seventy eighty ninety hundred thousand million'.split())


def spoken_numbers(root,runtime,call):
    """Narration with numbers keeps its speech check (the part-two demo's line). Recognition writes
    numerals such as "80" and aligns them as the words they are read as, known text with digits
    aligns word for word, and export.review matches the numerals it hears with the numbers the
    script spells out."""
    root.mkdir()
    spoken=lines(root/'speech',{'numbers':NUMBERS})
    path=spoken['numbers']['path'];starts=[e['start'] for e in spoken['numbers']['events']]
    local={k:v for k,v in runtime.items() if k!='alignment_roots'};local['alignment_root']=runtime['alignment_roots']['en']
    def transcribe(label,**fields):
        saved=root/(label+'.json')
        call({'command':'media.transcribe','path':str(path),'input_root':str(root/'speech'),'output_root':str(root),
            'output':str(saved),'runtime':local,'language':'en','id':label,**fields},timeout=600)
        return json.loads(saved.read_text(encoding='utf-8'))['transcripts']
    near=lambda word:min(abs(float(seconds(word['start']))-s) for s in starts)<=.25
    digits=lambda text:any(c.isdigit() for c in text)
    # Known text with digits keeps its spelling, each word near a word the synthesizer spoke.
    aligned=[w for d in transcribe('known-numerals',text=NUMERALS) for w in d['words']]
    assert [w['text'] for w in aligned]==NUMERALS.split() and all(near(w) for w in aligned),(aligned,starts)
    script=transcribe('script',text=NUMBERS)
    # The production speech check: review the line against its spelled-out script, recognizing it.
    frames=int(seconds(script[0]['source']['duration'])*25)
    project=call({'command':'project.create','id':'numbers','width':16,'height':12,'frame_rate':time(25)})
    project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':[
        {'op':'media.add','asset':{'id':'line','path':path.name,'duration':time(frames,25)}},
        {'op':'clip.append','clip':{'id':'c','asset_id':'line','source_in':time(0),'duration':time(frames,25)}}]})
    review=call({'command':'export.review','path':str(path),'input_root':str(root/'speech'),'output_root':str(root),
        'output':str(root/'review'),'project':project,'transcripts':script,'runtime':local,'language':'en','rendition_height':0},timeout=900)
    speech=review['speech'];comparison=speech['comparison']
    assert speech['recognition']['ok'],speech
    heard=[w for d in json.loads((root/'review'/'transcripts.json').read_text(encoding='utf-8'))['transcripts'] for w in d['words']]
    numerals=[w for w in heard if digits(w['text'])]
    assert numerals,('Recognition spelled every number out, so this case tests no numeral',heard)
    assert all(near(w) for w in numerals),(numerals,starts)
    number=lambda text:digits(text) or bool(NUMBER_WORDS & set(re.findall('[a-z]+',text.lower())))
    assert comparison['number_matches']>=len(numerals),comparison
    assert not [d for d in comparison['differences']['listed'] if number(d['expected']) or number(d['heard'])],comparison
    record={'script':NUMBERS,'known_text':NUMERALS,'heard':' '.join(w['text'] for w in heard),
        'numerals':[{'text':w['text'],'start':w['start'],'end':w['end']} for w in numerals],
        'comparison':{k:comparison[k] for k in ('expected_words','heard_words','matched','number_matches','joined_matches','differences')}}
    print(json.dumps(record,ensure_ascii=False),flush=True)
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
