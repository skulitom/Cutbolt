"""Original 4K motion, complete decoded clocks/content, process memory and stress gates."""
import argparse
import ctypes
from ctypes import wintypes
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import subprocess
import threading
import time as clock

import numpy as np
from agents import until
from registry import peak_memory
from scenes import time

ROOT = Path(__file__).resolve().parents[1]
WIDTH, HEIGHT, RATE = 3840, 2160, 25


def process_tree(root_pid, child_program=None):
    """A read-only Windows process snapshot; never terminate by a sampled PID."""
    class Entry(ctypes.Structure):
        _fields_ = [('size', wintypes.DWORD), ('usage', wintypes.DWORD), ('pid', wintypes.DWORD),
                    ('heap', ctypes.c_size_t), ('module', wintypes.DWORD), ('threads', wintypes.DWORD),
                    ('parent', wintypes.DWORD), ('priority', wintypes.LONG), ('flags', wintypes.DWORD),
                    ('name', wintypes.WCHAR * 260)]
    api = ctypes.WinDLL('kernel32', use_last_error=True)
    api.CreateToolhelp32Snapshot.argtypes = [wintypes.DWORD, wintypes.DWORD]
    api.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    api.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    api.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(Entry)]
    api.CloseHandle.argtypes = [wintypes.HANDLE]
    handle = api.CreateToolhelp32Snapshot(2, 0)
    assert handle != ctypes.c_void_p(-1).value, ctypes.get_last_error()
    entries = {}; names = {}
    try:
        e = Entry(); e.size = ctypes.sizeof(e)
        more = api.Process32FirstW(handle, ctypes.byref(e))
        assert more, ctypes.get_last_error()
        while more:
            entries[e.pid] = e.parent
            names[e.pid] = e.name
            more = api.Process32NextW(handle, ctypes.byref(e))
    finally:
        api.CloseHandle(handle)
    # Queue workers retain their own console host while running the next job.
    # For cancellation evidence, select only the active tool scope descendants.
    result = ({pid for pid,parent in entries.items() if parent==root_pid and names[pid].lower()==child_program.lower()}
              if child_program else {root_pid})
    while True:
        expanded = result | {pid for pid, parent in entries.items() if parent in result}
        if expanded == result:
            return result & entries.keys()
        result = expanded


def monitored(command, request, seconds, env=None):
    p = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                         env=env, creationflags=subprocess.CREATE_NO_WINDOW)
    stop = threading.Event(); peaks = {}; totals = []; errors = []
    def sample():
        while not stop.is_set():
            try:
                total = 0
                for pid in process_tree(p.pid):
                    try:
                        value = peak_memory(pid)
                    except AssertionError:  # A process can exit between snapshot and opening it.
                        continue
                    peaks[pid] = max(peaks.get(pid, 0), value); total += value
                totals.append(total)
            except Exception as exc:
                errors.append(str(exc))
            stop.wait(.2)
    sampler = threading.Thread(target=sample); sampler.start(); start = clock.monotonic()
    try:
        out, err = p.communicate(json.dumps(request).encode(), timeout=seconds)
    finally:
        if p.poll() is None:
            p.kill(); p.wait(timeout=10)  # The native tool scopes contain their descendants.
        stop.set(); sampler.join(timeout=10)
    assert not errors, errors
    measurement = {'seconds': clock.monotonic()-start, 'sampled_tree_peak_bytes': max(totals),
                   'process_peak_bytes': peaks, 'samples': len(totals), 'interval_seconds': .2}
    assert len(totals) > 1 and len(peaks) >= 2, measurement
    assert p.returncode == 0, (out, err, measurement)
    reply = json.loads(out); assert reply['ok'], reply
    return reply['result'], measurement


def run(root, long_form):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/'sources', root/'output'; sources.mkdir(); output.mkdir()
    exe = Path(os.environ.get('CUTBOLT_TEST_ENGINE', ROOT/'target/debug/cutbolt.exe')); passed = []; measurements = {}; originals = {}
    seconds = 1800 if long_form else 12; count = seconds*RATE
    def save(name, value):
        (root/name).write_text(json.dumps(value, indent=2)+'\n', encoding='utf-8')
    def sha(path):
        with path.open('rb') as f: return hashlib.file_digest(f, 'sha256').hexdigest()
    def call(command, error=None, **fields):
        p = subprocess.run([str(exe)], input=json.dumps({'command':command, **fields}).encode(), capture_output=True, timeout=1200)
        value = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and value['error']['code'] == error, value
            return value['error']
        assert p.returncode == 0 and value['ok'], value
        return value['result']
    def ff(args, timeout=300):
        return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args], capture_output=True, check=True, timeout=timeout).stdout
    # A one-second original cycle has moving edges, gradients and visible binary IDs.
    # Audio has an independent non-cycling sample-index pattern over the long source.
    y = np.arange(HEIGHT, dtype=np.uint32)[:, None]; x = np.arange(WIDTH, dtype=np.uint32)[None, :]
    hashes = []
    raw = sources/'cycle.rgb'
    with raw.open('wb') as f:
        for n in range(RATE):
            frame = np.empty((HEIGHT, WIDTH, 3), dtype=np.uint8)
            frame[:,:,0] = ((x//32+n*3)+(y//64)) % 256
            frame[:,:,1] = ((y//24+n*7)+(x//96)) % 256
            frame[:,:,2] = ((x//80+y//72+n*11)) % 256
            frame[300:620, (n*131) % (WIDTH-420):(n*131) % (WIDTH-420)+420] = [17,231,69]
            for bit in range(5): frame[50:150, 60+bit*120:150+bit*120] = 255 if n & (1<<bit) else 0
            data = frame.tobytes(); f.write(data); hashes.append(hashlib.md5(data).hexdigest())
    cycle = sources/'cycle.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{WIDTH}x{HEIGHT}','-framerate','25','-i',str(raw),
        '-c:v','ffv1','-level','3','-slices','16','-threads','16','-pix_fmt','bgr0',str(cycle)])
    def pcm(start, length):
        n = np.arange(start, start+length, dtype=np.int64)
        return np.stack(((n*43)%50021-25010, (n*61)%48017-24008), axis=-1).astype('<i2').tobytes()
    audio = sources/'source.pcm'; source_frames = count+50; source_samples = source_frames*1920
    with audio.open('wb') as f:
        for start in range(0, source_samples, 48000): f.write(pcm(start, min(48000, source_samples-start)))
    movie = sources/'source.mkv'
    ff(['-stream_loop',str(seconds+2),'-i',str(cycle),'-f','s16le','-ar','48000','-ac','2','-i',str(audio),
        '-map','0:v:0','-map','1:a:0','-c','copy','-t',str(seconds+2),str(movie)])
    originals = {p.name:sha(p) for p in sources.iterdir()}
    p = call('project.create', id='long-4k', width=WIDTH, height=HEIGHT, frame_rate=time(25))
    p = call('timeline.apply', project=p, expected_revision=0, operations=[{'op':'media.add','asset':{
        'id':'source','path':str(movie),'duration':time(source_frames,25),
        'identity':{'bytes':movie.stat().st_size,'sha256':originals['source.mkv']}}}])
    half=count//2
    segments=[(7,half), (None,25), (half+39,count-half-25)]
    ops=[]
    for i,(first,length) in enumerate(segments):
        clip={'id':f'cut-{i}', 'source_in':time(first or 0,25), 'duration':time(length,25)}
        clip.update({'gap':True} if first is None else {'asset_id':'source'})
        ops.append({'op':'clip.append','clip':clip})
    p=call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    save('project.json',p)
    request={'command':'render.run','project':p,'input_root':str(sources),'output_root':str(output),'output':str(output/'edited.mkv')}
    save('request.json',request)
    receipt, measurement=monitored([str(exe)],request,3300)
    save('render-measurement.json',measurement);save('receipt.json',receipt)
    # Predetermined gates apply to the entire source inspection, encode, output
    # inspection and integrity checking path, not just encoder progress.
    max_seconds=2700 if long_form else 120
    assert measurement['seconds'] < max_seconds, measurement
    assert measurement['sampled_tree_peak_bytes'] < 4*1024**3, measurement
    assert receipt['frames']==count and receipt['samples']==count*1920,receipt
    measurements['render']={**measurement,'gates':{'seconds':max_seconds,'tree_bytes':4*1024**3}}
    timeline=[None if first is None else first+n for first,length in segments for n in range(length)]
    destination=Path(request['output'])
    started=clock.monotonic()
    # Hash every decoded frame in parallel segments. Accurate input seeking with copied timestamps
    # keeps each row's absolute frame index, which is checked for every frame below.
    parts=8;per=-(-count//parts);jobs=[]
    for k in range(parts):
        first=k*per;n=min(per,count-first)
        if n<=0:break
        digest=output/f'decoded-{k}.framemd5'
        jobs.append((n,digest,subprocess.Popen(['ffmpeg','-v','error','-nostdin','-n','-threads','4','-copyts','-ss',f'{first*40//1000}.{first*40%1000:03d}',
            '-i',str(destination),'-map','0:v:0','-an','-fps_mode','passthrough','-frames:v',str(n),'-pix_fmt','rgb24','-threads','1','-f','framemd5',str(digest)],
            stdout=subprocess.DEVNULL,stderr=subprocess.PIPE)))
    rows=[];header=None
    for n,digest,job in jobs:
        _,errors=job.communicate(timeout=2100);assert job.returncode==0,errors
        lines=digest.read_bytes().splitlines(keepends=True)
        head=[line for line in lines if line.startswith(b'#')];part=[line for line in lines if line.strip() and not line.startswith(b'#')]
        assert header in (None,head) and len(part)==n,(digest,len(part),n);header=head;rows.extend(part)
    # Reassemble the single-pass framemd5, which later repeated renders are compared against.
    (output/'decoded.framemd5').write_bytes(b''.join(header+rows));rows=[line.decode() for line in rows]
    assert len(rows)==count
    black=hashlib.md5(bytes(WIDTH*HEIGHT*3)).hexdigest()
    for i,(line,index) in enumerate(zip(rows,timeline)):
        columns=[v.strip() for v in line.split(',')]
        assert int(columns[2])==i and int(columns[3])==1 and int(columns[4])==WIDTH*HEIGHT*3,(i,line)
        assert columns[5]==(black if index is None else hashes[index%RATE]),(i,index,line)
    measurements['full_rgb_decode_seconds']=clock.monotonic()-started
    proc=subprocess.Popen(['ffmpeg','-v','error','-i',str(destination),'-map','0:a:0','-vn','-f','s16le','-'],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    try:
        for first,length in segments:
            total=length*1920; start=(first or 0)*1920
            for offset in range(0,total,48000):
                n=min(48000,total-offset)
                assert proc.stdout.read(n*4)==(bytes(n*4) if first is None else pcm(start+offset,n)),(first,offset)
        assert proc.stdout.read()==b'' and proc.wait(timeout=30)==0,proc.stderr.read()
    finally:
        if proc.poll() is None:proc.kill();proc.wait()
    probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-threads','16','-select_streams','v:0',
        '-show_streams','-show_frames','-show_entries','stream=time_base:frame=best_effort_timestamp','-of','json',str(destination)],timeout=900))
    tb=Fraction(probe['streams'][0]['time_base']);assert tb==Fraction(1,1000)
    assert len(probe['frames'])==count
    for i,frame in enumerate(probe['frames']):assert frame['best_effort_timestamp']==i*40,(i,frame)
    assert originals=={path.name:sha(path) for path in sources.iterdir()}
    assert not list(output.glob('.cutbolt-*'))
    passed.extend(['long_form_4k.complete_moving_pixels_pcm_and_clocks','long_form_4k.bounded_tree_memory_and_render_latency'])
    result={'passed':passed,'long_gate_passed':long_form,'width':WIDTH,'height':HEIGHT,'frames':count,
            'samples':count*1920,'seconds':seconds,'measurements':measurements,'sources_preserved':True,
            'output_sha256':sha(destination)}
    save('verification.json',result);print(json.dumps(result,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--long-form',action='store_true')
    args=parser.parse_args();run(args.output,args.long_form)
