"""Original local speech fixtures and independent synthesis/activity clocks."""
from array import array
import hashlib
import json
from pathlib import Path
import subprocess
import wave

PASSAGES = {
    'en': ['The red square moves slowly across the screen. Keep the blue circle and remove the green triangle. The final picture stays still.',
           'A yellow bird rests beside the window. Move the small box to the left. Leave the quiet ending in place.'],
    'el': ['Η γάτα κάθεται κοντά στο παράθυρο. Ο ήλιος λάμπει και τα παιδιά παίζουν στον κήπο. Η πόρτα είναι ανοιχτή.',
           'Το μικρό πουλί πετά πάνω από το σπίτι. Η Μαρία κλείνει αργά το βιβλίο. Το νερό είναι κρύο.'],
}
WORDS = {'en':['red','square','blue','circle','green','triangle'], 'el':['γάτα','ήλιος','παιδιά','πόρτα','παράθυρο','κήπος']}
VOICES = {'en':'Microsoft Hazel Desktop','el':'Microsoft Stefanos'}


def read_wave(path):
    with wave.open(str(path),'rb') as stream:
        assert (stream.getnchannels(),stream.getsampwidth(),stream.getframerate()) == (1,2,16000)
        return array('h',stream.readframes(stream.getnframes()))


def generate(root):
    root=Path(root).resolve();root.mkdir()
    cases=[]
    for language,texts in PASSAGES.items():
        for i,text in enumerate(texts):
            cases.append({'id':('pilot' if i==0 else 'holdout')+'-'+language,'language':language,'voice':VOICES[language],'rate':i-1,'text':text})
        for i,text in enumerate(WORDS[language]):
            cases.append({'id':f'word-{language}-{i}','language':language,'voice':VOICES[language],'rate':-1,'text':text})
    receipt=synthesize(root,cases)
    fixtures={}
    for item in receipt['cases']:
        path=root/(item['id']+'.wav')
        item['sha256']=hashlib.sha256(path.read_bytes()).hexdigest()
        if not item['id'].startswith('word-'):
            fixtures[item['id']]={'path':path,'language':item['language'],'pcm':read_wave(path),'text':item['text'],
                'reference':[{'text':w['text'],'start':w['ticks']/1e7} for w in item['events']]}
    return isolated(root,receipt,fixtures)


def lines(root,texts):
    """English lines spoken by the fixture voice: {id: {path, text, events}}, each event a spoken
    word and its start in seconds."""
    root=Path(root).resolve();root.mkdir()
    receipt=synthesize(root,[{'id':key,'language':'en','voice':VOICES['en'],'rate':-1,'text':text} for key,text in texts.items()])
    (root/'receipt.json').write_text(json.dumps(receipt,ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    return {item['id']:{'path':root/(item['id']+'.wav'),'text':item['text'],
        'events':[{'text':w['text'],'start':w['ticks']/1e7} for w in item['events']]} for item in receipt['cases']}


def synthesize(root,cases):
    """Speak each case to <root>/<id>.wav (16 kHz mono) with the local voices; returns the receipt
    with each case's word events."""
    (root/'cases.json').write_text(json.dumps(cases,ensure_ascii=False),encoding='utf-8')
    script=r'''param([string]$FixtureRoot)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName System.Speech
Add-Type -ReferencedAssemblies @('System.Speech','System.Runtime','System.Collections','System.ComponentModel.Primitives','System.Threading') -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Speech.Synthesis;
using System.Speech.AudioFormat;
public sealed class WordEvent { public string text; public long ticks; }
public static class SpeechMaker {
 public static WordEvent[] Make(string path,string voice,int rate,string text) {
  var words=new List<WordEvent>();
  using(var synth=new SpeechSynthesizer()) {
   synth.SelectVoice(voice); synth.Rate=rate;
   synth.SetOutputToWaveFile(path,new SpeechAudioFormatInfo(16000,AudioBitsPerSample.Sixteen,AudioChannel.Mono));
   synth.SpeakProgress+=(sender,e)=>{lock(words) words.Add(new WordEvent{text=e.Text,ticks=e.AudioPosition.Ticks});};
   synth.Speak(text); synth.SetOutputToNull();
  }
  lock(words) return words.ToArray();
 }
}
'@
$items=Get-Content -LiteralPath (Join-Path $FixtureRoot 'cases.json') -Raw -Encoding utf8 | ConvertFrom-Json
$records=@(foreach($item in $items) {
 if ($item.id -notmatch '^[a-z0-9-]+$') {throw 'Invalid fixture id'}
 $path=Join-Path $FixtureRoot ($item.id+'.wav')
 if(Test-Path -LiteralPath $path) {throw 'Source already exists'}
 $events=[SpeechMaker]::Make($path,$item.voice,$item.rate,$item.text)
 [pscustomobject]@{id=$item.id;language=$item.language;voice=$item.voice;rate=$item.rate;text=$item.text;events=$events}
})
@{powershell=$PSVersionTable.PSVersion.ToString();assembly=[System.Speech.Synthesis.SpeechSynthesizer].Assembly.FullName;cases=$records} | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $FixtureRoot 'synthesis.json') -Encoding utf8
'''
    path=root/'generate.ps1';path.write_text(script,encoding='utf-8')
    subprocess.run(['pwsh','-NoProfile','-NonInteractive','-File',str(path),'-FixtureRoot',str(root)],check=True,capture_output=True,timeout=90)
    return json.loads((root/'synthesis.json').read_text(encoding='utf-8-sig'))


def isolated(root,receipt,fixtures):
    """Each language's isolated words, joined with 400 ms of digital silence between them."""
    for language in WORDS:
        pcm=array('h',[0]*6400);reference=[]
        for i,text in enumerate(WORDS[language]):
            original=read_wave(root/f'word-{language}-{i}.wav')
            active=[n for n,v in enumerate(original) if abs(v)>=200]
            assert active
            first,last=max(0,active[0]-320),min(len(original),active[-1]+321)
            offset=len(pcm)
            reference.append({'text':text,'start':(offset+active[0]-first)/16000,'end':(offset+active[-1]+1-first)/16000})
            pcm.extend(original[first:last]);pcm.extend([0]*6400)
        path=root/('isolated-'+language+'.wav')
        with wave.open(str(path),'wb') as stream:
            stream.setparams((1,2,16000,len(pcm),'NONE','not compressed'));stream.writeframes(pcm.tobytes())
        fixtures['isolated-'+language]={'path':path,'language':language,'pcm':pcm,'reference':reference}
    (root/'receipt.json').write_text(json.dumps({'synthesis':receipt,'activity_threshold':200,'silence_samples':6400,
        'reference':{name:fixture['reference'] for name,fixture in fixtures.items()}},ensure_ascii=False,indent=2)+'\n',encoding='utf-8')
    return fixtures
