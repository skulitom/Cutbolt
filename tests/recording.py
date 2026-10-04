"""Original isolated native capture, exact placement and bounded streaming acceptance."""
from engine import ENGINE
import budgets
import argparse,copy,hashlib,json,os,struct,subprocess,time as clock,wave
from pathlib import Path
import numpy as np
from agents import Client
from registry import peak_memory
from scenes import time
from PIL import Image
from jsonschema import Draft202012Validator

ROOT=Path(__file__).resolve().parents[1]
EXE=ENGINE
HELPER=ROOT/'target/debug/examples/recording_fixture.exe'
def sha(path):
    h=hashlib.sha256()
    with open(path,'rb') as f:
        while b:=f.read(65536):h.update(b)
    return h.hexdigest()
def identity(path):return {'path':str(path),'bytes':path.stat().st_size,'sha256':sha(path)}
def write_wave(path,pcm,rate=48000):
    with wave.open(str(path),'wb') as f:f.setnchannels(2);f.setsampwidth(2);f.setframerate(rate);f.writeframes(np.asarray(pcm,dtype='<i2').tobytes())
def read_wave(path):
    with wave.open(str(path),'rb') as f:
        assert f.getnchannels()==2 and f.getsampwidth()==2 and f.getframerate()==48000
        count=f.getnframes();b=f.readframes(count);assert len(b)==count*4
    return np.frombuffer(b,dtype='<i2').reshape(-1,2).copy()
def source(seconds,mode='mute',rate=48000,offset=0):
    p=subprocess.Popen([str(HELPER),str(seconds),mode,str(rate),str(offset)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,creationflags=subprocess.CREATE_NO_WINDOW)
    ready=json.loads(p.stdout.readline());assert ready['ready'] and ready['pid']==p.pid
    return p,ready
def stop(p):
    if p.poll() is None:p.stdin.write('\n');p.stdin.flush()
    p.wait(timeout=5);assert p.returncode==0,p.stderr.read()

def run(root,native_seconds):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,out,store=[root/n for n in ('sources','output','store')]
    for p in (sources,out,store):p.mkdir()
    subprocess.run(['cargo','build','--locked','--example','recording_fixture'],cwd=ROOT,check=True,capture_output=True)
    passed=[];cases=[];rejected=0;rendered_frames=0;sample_frames=0
    original_hashes={}
    def call(r,error=None,timeout=120):
        nonlocal rejected
        p=subprocess.run([str(EXE)],input=json.dumps(r).encode(),capture_output=True,timeout=timeout);v=json.loads(p.stdout)
        if error:assert p.returncode==1 and v['error']['code']==error,(error,v);rejected+=1;return v
        if p.returncode or not v['ok']:(root/'failed-request.json').write_text(json.dumps(r,indent=2),encoding='utf-8')
        assert p.returncode==0 and v['ok'],v;return v['result']
    def apply(p,ops,error=None):return call({'command':'timeline.apply','project':p,'expected_revision':p['revision'],'operations':ops},error)
    def edit(op,**kw):return {'op':'tracks.edit','edit':{'op':op,**kw}}
    def capture(pid,name,duration=2):
        r=call({'command':'audio.record','input':{'type':'process','pid':pid},'duration':time(duration),'output_root':str(out),'output':str(out/f'{name}.wav')},timeout=duration+20)
        pcm=read_wave(Path(r['output']));assert len(pcm)==duration*48000
        assert hashlib.sha256(pcm.tobytes()).hexdigest()==r['pcm_sha256'];assert sha(r['output'])==r['identity']['sha256']
        original_hashes[Path(r['output'])]=r['identity']['sha256']
        (root/f'{name}-receipt.json').write_text(json.dumps(r,indent=2),encoding='utf-8')
        return r,pcm
    devices=call({'command':'audio.inputs'});assert devices['capture_started'] is False and devices['inputs']
    for d in devices['inputs']:
        if 'error' not in d:
            plan=call({'command':'audio.record.inspect','input':{'type':'endpoint','id':d['id']},'duration':time(1)})
            assert plan['device']==d and plan['capture_started'] is False and plan['samples']==48000
    cases.append({'discovery':devices,'no_capture_during_discovery':True})
    # Native format conversion, channel separation and exclusion of another playback process.
    native=[]
    for rate in (48000,44100):
        a,ready=source(15,'quiet',rate);b,other=source(15,'quiet',48000,1000)
        try:
            plan=call({'command':'audio.record.inspect','input':{'type':'process','pid':a.pid},'duration':time(2)})
            assert plan['capture_started'] is False
            receipt,pcm=capture(a.pid,f'native-{rate}')
            region=pcm[24000:72000].astype(float);n=np.arange(len(region));measure=[]
            for ch,freq in enumerate((310,710)):
                def amplitude(f):return abs(np.sum(region[:,ch]*np.exp(-2j*np.pi*f*n/48000)))*2/len(region)
                wanted=amplitude(freq);foreign=amplitude(freq+1000);cross=amplitude((710,310)[ch])
                assert abs(wanted-(100,50)[ch])<2,(rate,ch,wanted)
                assert foreign<1 and cross<1,(foreign,cross)
                measure.append({'channel':ch,'wanted_amplitude':wanted,'unselected_process_amplitude':foreign,'other_channel_amplitude':cross})
            native.append({'source':ready,'excluded_source':other,'measurements':measure,'receipt':receipt})
            if rate==48000:captured,captured_pcm=receipt,pcm
        finally:stop(a);stop(b)
    passed.append('recording.native_capture_formats_channels_and_isolation')
    # Independently authored one-sample pulses expose both signs of placement correction.
    pulse=np.zeros((12001,2),dtype='<i2');pulse[2400]=[17000,-23000];pulse[7183]=[-29000,29000]
    pulse_file=sources/'delayed-pulses.wav';write_wave(pulse_file,pulse);original_hashes[pulse_file]=sha(pulse_file)
    p=call({'command':'project.create','id':'recording-fixture','width':4,'height':4,'frame_rate':time(25)})
    p=apply(p,[edit('create',duration=time(3)),edit('add',track={'id':'voice','kind':'audio','enabled':True,'locked':False,'clips':[]})])
    def proposal(project,source,asset_id,clip_id,start,amount=0,backward=False,sequence=None,error=None):
        return call({'command':'audio.record.place','project':project,'source':source,'input_root':str(root),'asset_id':asset_id,'clip_id':clip_id,'track_id':'voice','sequence_id':sequence,'start':time(start,48000),'compensation':{'backward':backward,'amount':time(amount,48000)},'collision':'reject'},error)
    def render(project,name,expected):
        nonlocal rendered_frames,sample_frames
        receipt=call({'command':'render.run','project':project,'input_root':str(root),'output_root':str(out),'output':str(out/f'{name}.mkv')})
        audio=subprocess.run(['ffmpeg','-v','error','-i',receipt['output'],'-vn','-c:a','pcm_s16le','-f','s16le','-'],capture_output=True,check=True).stdout
        assert audio==expected.astype('<i2').tobytes(),name
        rgb=subprocess.run(['ffmpeg','-v','error','-i',receipt['output'],'-an','-pix_fmt','rgb24','-f','rawvideo','-'],capture_output=True,check=True).stdout
        assert rgb==bytes(receipt['frames']*4*4*3)
        rendered_frames+=receipt['frames'];sample_frames+=len(expected);cases.append({'render':name,'frames':receipt['frames'],'sample_frames':len(expected),'maximum_pcm_error':0})
        original_hashes[Path(receipt['output'])]=sha(receipt['output'])
        return receipt
    actual_proposal=proposal(p,captured['identity'],'captured','actual-take',1001,11,True)
    expected=np.zeros((144000,2),dtype='<i2');expected[990:990+len(captured_pcm)]=captured_pcm
    render(actual_proposal['project'],'captured-on-sequence',expected)
    for back,amount,start in [(True,1200,6000),(False,1201,6000),(True,0,1)]:
        placed=proposal(p,identity(pulse_file),'pulse','take',start,amount,back)
        at=start-amount if back else start+amount
        expected_pulse=np.zeros((144000,2),dtype='<i2');expected_pulse[at:at+len(pulse)]=pulse
        render(placed['project'],f'compensation-{back}-{amount}',expected_pulse)
        assert placed['compensated_start']['num']/placed['compensated_start']['den']==at/48000
    passed.append('recording.exact_native_placement_and_latency_shifts')
    # Same operations go through actual saved revisions, retries, undo and MCP schemas.
    call({'command':'session.create','store_root':str(store),'project':p,'request_id':'create'})
    req={'command':'session.apply','store_root':str(store),'project_id':p['id'],'expected_revision':0,'request_id':'captured-take','operations':actual_proposal['operations']}
    saved=call(req);assert call(req)==saved
    client=Client(EXE)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==65
        for name in ('audio_inputs','audio_record_inspect','audio_record_place'):
            tool=next(t for t in catalog if t['name']=='cutbolt_'+name);assert tool['annotations']['readOnlyHint']
        assert not any(t['name']=='cutbolt_audio_record' for t in catalog)
        same=client.call('audio.record.place',**{k:v for k,v in {'project':p,'source':captured['identity'],'input_root':str(root),'asset_id':'captured','clip_id':'actual-take','track_id':'voice','start':time(1001,48000),'compensation':{'backward':True,'amount':time(11,48000)},'collision':'reject'}.items()})
        assert same==actual_proposal
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_audio_record_place')
        Draft202012Validator(schema).validate({'project':p,'source':captured['identity'],'input_root':str(root),'asset_id':'new','clip_id':'new','track_id':'voice','start':time(0),'compensation':{'backward':False,'amount':time(0)},'collision':'reject'})
        common={'store_root':str(store),'project_id':p['id']}
        saved_project=client.call('session.get',**common);assert saved_project['revision']==1
        render(saved_project,'saved-recording',expected)
        client.call('session.undo',**common,expected_revision=1,request_id='undo-capture')
        undone=client.call('session.get',**common);assert undone['tracks']==p['tracks'] and undone['assets']==p['assets']
        assert call(req)==saved
        client.call('session.restore',**common,expected_revision=2,target_revision=1,request_id='restore-capture')
        restored=client.call('session.get',**common);assert restored['tracks']==saved_project['tracks']
        assert len(client.call('session.history',**common)['entries'])==4
        for index in (0,50,74):
            dest=out/f'preview-{index}.png';client.call('preview.frame',project=restored,input_root=str(root),output_root=str(out),output=str(dest),time=time(index,25))
            with Image.open(dest) as image:assert image.size==(4,4) and image.convert('RGB').tobytes()==bytes(48)
        range_path=out/'selected-range.mkv';call({'command':'preview.range','project':restored,'input_root':str(root),'output_root':str(out),'output':str(range_path),'start':time(1),'duration':time(1)})
        sound=subprocess.run(['ffmpeg','-v','error','-i',str(range_path),'-vn','-f','s16le','-'],capture_output=True,check=True).stdout
        assert sound==expected[48000:96000].tobytes();sample_frames+=48000;rendered_frames+=25
        original_hashes[range_path]=sha(range_path)
        proxy_project=apply(restored,[{'op':'preview.proxy','scale':2}]);proxy_path=out/'audio-with-scaled-video.png'
        client.call('preview.frame',project=proxy_project,input_root=str(root),output_root=str(out),output=str(proxy_path),time=time(1))
        with Image.open(proxy_path) as image:assert image.size==(2,2) and image.convert('RGB').tobytes()==bytes(12)
    finally:client.close()
    passed.append('recording.saved_edits_and_read_only_mcp')
    # Child audio definitions retain the same recording and source clock.
    child=apply(p,[{'op':'sequence.create','id':'recorded-child','duration':time(3)},{'op':'sequence.edit','id':'recorded-child','edit':{'op':'add','track':{'id':'voice','kind':'audio','enabled':True,'locked':False,'clips':[]}}}])
    child=proposal(child,captured['identity'],'nested-capture','take',990,sequence='recorded-child')['project']
    nested=apply(child,[edit('place',track_id='voice',clip={'id':'instance','sequence_id':'recorded-child','start':time(0),'source_in':time(0),'duration':time(3)},collision='reject')])
    render(nested,'nested-recording',expected)
    passed.append('recording.native_wave_previews_ranges_and_nesting')
    # Invalid inputs never start recording; malformed/changed files do not create a placement.
    base_inspect={'command':'audio.record.inspect','input':{'type':'process','pid':os.getpid()},'duration':time(1)}
    for duration,code in [(time(0),'LIMIT_EXCEEDED'),(time(7200*48000+1,48000),'LIMIT_EXCEEDED'),(time(1,48001),'UNALIGNED_TIME'),({'num':1,'den':0},'INVALID_TIME')]:
        request=copy.deepcopy(base_inspect);request['duration']=duration;call(request,code)
    for input_ in ({'type':'process','pid':0},{'type':'endpoint','id':''},{'type':'endpoint','id':'\u0000'}):
        call({**base_inspect,'input':input_},'INVALID_CAPTURE_INPUT')
    call({**base_inspect,'input':{'type':'endpoint','id':'cutbolt-nonexistent-fixture-input'}},'CAPTURE_DEVICE_ERROR')
    call({**base_inspect,'input':{'type':'process','pid':4294967295}},'CAPTURE_DEVICE_ERROR')
    base_record={**base_inspect,'command':'audio.record','output_root':str(out),'output':str(out/'never.wav')}
    for fields,code in [({'output':captured['output']},'OUTPUT_EXISTS'),({'output_root':'.'},'INVALID_PATH'),({'output':str(sources/'outside.wav')},'PATH_OUTSIDE_ROOT'),({'output':str(out/'bad.mp3')},'UNSUPPORTED_OUTPUT')]:call({**base_record,**fields},code)
    for changes in ({'sha256':'0'*64},{'bytes':1}):proposal(p,{**identity(pulse_file),**changes},'bad','bad',0,error='MEDIA_CHANGED')
    proposal(p,identity(pulse_file),'bad','bad',0,1,True,error='INVALID_RANGE')
    proposal(p,identity(pulse_file),'','bad',0,error='INVALID_ID')
    locked=copy.deepcopy(p);locked['tracks']['tracks'][0]['locked']=True;proposal(locked,identity(pulse_file),'bad','bad',0,error='TRACK_LOCKED')
    video=copy.deepcopy(p);video['tracks']['tracks'][0]['kind']='video';proposal(video,identity(pulse_file),'bad','bad',0,error='INVALID_CAPTURE_PLACEMENT')
    proposal(p,identity(pulse_file),'bad','bad',144000,error='INVALID_RANGE')
    data=pulse_file.read_bytes()
    broken=[('truncated',data[:-1]),('wrong-riff',b'RIFX'+data[4:]),('rate',data[:24]+struct.pack('<I',44100)+data[28:]),('align',data[:32]+b'\x02\x00'+data[34:]),('channels',data[:22]+b'\x01\x00'+data[24:]),('bits',data[:34]+b'\x18\x00'+data[36:])]
    for name,content in broken:
        path=sources/f'bad-{name}.wav';path.write_bytes(content);original_hashes[path]=sha(path);proposal(p,identity(path),'bad','bad',0,error='UNSUPPORTED_AUDIO')
    passed.append('recording.validation_and_source_preservation')
    # Actual source process exit is rejected; no implicit input fallback or final publication.
    a,_=source(15)
    bad_output=out/'interrupted.wav'
    request={'command':'audio.record','input':{'type':'process','pid':a.pid},'duration':time(10),'output_root':str(out),'output':str(bad_output)}
    recorder=subprocess.Popen([str(EXE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    recorder.stdin.write(json.dumps(request).encode());recorder.stdin.close();clock.sleep(.75);stop(a)
    recorder.wait(timeout=8);value=json.loads(recorder.stdout.read());assert recorder.returncode==1 and value['error']['code']=='CAPTURE_PROCESS_EXITED',value
    assert not bad_output.exists() and not list(out.glob('.cutbolt-scene-*'))
    a,_=source(15)
    try:
        capture(a.pid,'explicit-new-process',1)
        one=call({'command':'audio.record','input':{'type':'process','pid':a.pid},'duration':time(1,48000),'output_root':str(out),'output':str(out/'one-sample.wav')})
        one_pcm=read_wave(Path(one['output']));assert one_pcm.shape==(1,2) and one['samples']==1
        assert hashlib.sha256(one_pcm.tobytes()).hexdigest()==one['pcm_sha256']
        assert one['timing']['received_sample_frames']-1==one['timing']['discarded_final_packet_frames']
        original_hashes[Path(one['output'])]=sha(one['output'])
    finally:stop(a)
    passed.append('recording.input_loss_and_explicit_reselection')
    # Predeclared sustained real-time gate. A shorter development run earns no long-capture ID.
    assert 1<=native_seconds<=900
    a,ready=source(native_seconds+30)
    long_request={'command':'audio.record','input':{'type':'process','pid':a.pid},'duration':time(native_seconds),'output_root':str(out),'output':str(out/'sustained.wav')}
    peak=0;start=clock.monotonic()
    try:
        recorder=subprocess.Popen([str(EXE)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        recorder.stdin.write(json.dumps(long_request).encode());recorder.stdin.close()
        while recorder.poll() is None:
            try:peak=max(peak,peak_memory(recorder.pid))
            except AssertionError:
                if recorder.poll() is None:raise
            clock.sleep(.1)
        wall=clock.monotonic()-start;value=json.loads(recorder.stdout.read());assert recorder.returncode==0 and value['ok'],value
        r=value['result'];assert r['samples']==native_seconds*48000 and peak<128*1024*1024;budgets.check(wall<native_seconds+30,wall)
        budgets.check(r['timing']['maximum_qpc_sample_clock_deviation_100ns']<=200000,r['timing'])
        digest=hashlib.sha256();peak_pcm=0;count=0
        with wave.open(str(out/'sustained.wav')) as wav:
            assert (wav.getnchannels(),wav.getsampwidth(),wav.getframerate(),wav.getnframes())==(2,2,48000,native_seconds*48000)
            while block:=wav.readframes(48000):
                digest.update(block);count+=len(block)//4;peak_pcm=max(peak_pcm,int(np.abs(np.frombuffer(block,dtype='<i2').astype(np.int32)).max()))
        assert count==native_seconds*48000 and peak_pcm<=2 and digest.hexdigest()==r['pcm_sha256']
        sustained={'seconds':native_seconds,'sample_frames':count,'peak_working_set_bytes':peak,'wall_seconds':wall,'maximum_silent_pcm':peak_pcm,'receipt':r,'source':ready,'long_gate_passed':native_seconds==900}
        original_hashes[out/'sustained.wav']=sha(out/'sustained.wav')
    finally:stop(a)
    if native_seconds==900:passed.append('recording.sustained_native_capture_clock_and_memory')
    for path,before in original_hashes.items():assert sha(path)==before,path
    report={'passed':passed,'native':native,'cases':cases,'sustained':sustained,'rendered_frames':rendered_frames,'sample_frames_compared':sample_frames,'rejected_cases':rejected,'source_preservation':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2),encoding='utf-8');print(json.dumps({'passed':passed,'frames':rendered_frames,'sample_frames':sample_frames,'long_gate_passed':sustained['long_gate_passed']},indent=2))

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--native-seconds',type=int,default=900,help='Use a shorter development run without earning the sustained-capture acceptance ID')
    args=parser.parse_args();run(args.output,args.native_seconds)
