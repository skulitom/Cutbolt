"""Real encoding with original write-fault injection, worker exits and source changes."""
from engine import ENGINE
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess

from agents import ProcessHandle, until
from scenes import time

ROOT = Path(__file__).resolve().parents[1]


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/'sources', root/'output'
    sources.mkdir(); output.mkdir()
    exe = ENGINE
    helper = root/'write-fault.exe'
    subprocess.run(['rustc', '--edition=2024', str(ROOT/'tests/render_failure_tool.rs'), '-o', str(helper)], check=True, capture_output=True)
    real = shutil.which('ffmpeg')
    passed, jobs, handles, gates = [], [], [], []
    frames = samples = 0
    faults = []

    def ff(args):
        return subprocess.run([real, '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=60).stdout

    def call(command, error=None, env=None, **fields):
        p = subprocess.run([str(exe)], input=json.dumps({'command':command, **fields}).encode(), env=env, capture_output=True, timeout=60)
        v = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and v['error']['code'] == error, v
            return v['error']
        assert p.returncode == 0 and v['ok'], v
        return v['result']

    def sha(path):
        return hashlib.sha256(path.read_bytes()).hexdigest()

    w, h, count = 64, 48, 48
    rgb = bytes(v for n in range(count) for y in range(h) for x in range(w) for v in ((x*13+n*19)%256, (y*23+n*29)%256, (x+y*3+n*7)%256))
    pcm = b''.join(struct.pack('<hh', (n*43)%50001-25000, (n*61)%48001-24000) for n in range(count*1920))
    (sources/'source.rgb').write_bytes(rgb); (sources/'source.pcm').write_bytes(pcm)
    movie = sources/'source.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate','25','-i',str(sources/'source.rgb'),
        '-f','s16le','-ar','48000','-ac','2','-i',str(sources/'source.pcm'),'-map','0:v:0','-map','1:a:0',
        '-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)])
    source_bytes = movie.read_bytes()
    originals = {p.name:sha(p) for p in sources.iterdir()}
    project = call('project.create', id='failure-injection', width=w, height=h, frame_rate=time(25))
    project = call('timeline.apply', project=project, expected_revision=0, operations=[
        {'op':'media.add','asset':{'id':'source','path':str(movie),'duration':time(count,25)}},
        {'op':'clip.append','clip':{'id':'shot','asset_id':'source','source_in':time(5,25),'duration':time(32,25)}}])
    sentinel = output/'unrelated.mkv'; sentinel.write_bytes(b'original existing output remains unchanged')
    sentinel_hash = sha(sentinel)

    def setup(label, mode):
        queue = root/label; queue.mkdir(); gates.append(queue/'release')
        env = {**os.environ, 'CUTBOLT_FFMPEG':str(helper), 'CUTBOLT_FAILURE_REAL_TOOL':real,
               'CUTBOLT_FAILURE_ROOT':str(queue), 'CUTBOLT_FAILURE_MODE':mode, 'CUTBOLT_FAILURE_OUTPUT':str(output),
               'CUTBOLT_FAILURE_SOURCE':str(movie), 'CUTBOLT_FAILURE_SOURCES':str(sources)}
        request = {'project':project, 'input_root':str(sources), 'output_root':str(output), 'output':str(output/(label+'.mkv'))}
        return queue, env, request

    def start(queue, env, request, attempts):
        ticket = call('render.start', env=env, job_root=str(queue), request_id='render', render={**request,'retry':{'max_attempts':attempts}})
        jobs.append((queue,ticket['job_id']))
        return ticket

    def status(queue, ticket):
        return call('job.status', job_root=str(queue), job_id=ticket['job_id'])

    def terminal(queue, ticket):
        return until(lambda:status(queue,ticket), lambda v:v['status'] in {'completed','failed','interrupted','cancelled'}, seconds=90)

    def fault(queue, attempt):
        record = json.loads((queue/f'failure-{attempt}.json').read_text())
        assert record == {'written':8192,'raw_os_error':112,'injected':True}, record
        saved = queue/f'encoded-{attempt}.mkv'
        assert saved.stat().st_size == 8192 and saved.read_bytes()[:4] == bytes.fromhex('1a45dfa3')
        faults.append({**record, 'partial_sha256':sha(saved)})

    def unchanged():
        assert originals == {p.name:sha(p) for p in sources.iterdir()}
        assert sha(sentinel) == sentinel_hash

    def compare(path):
        nonlocal frames, samples
        assert ff(['-i',str(path),'-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo','-']) == rgb[5*w*h*3:37*w*h*3]
        assert ff(['-i',str(path),'-map','0:a:0','-f','s16le','-']) == pcm[5*1920*4:37*1920*4]
        frames += 32; samples += 32*1920

    try:
        queue, env, request = setup('synchronous-full','full')
        failure = call('render.run', error='TOOL_FAILED', env=env, **request)
        assert 'injected output write failure' in failure['message']; fault(queue,1)
        assert not Path(request['output']).exists() and not list(output.glob('.cutbolt-*'))
        unchanged()
        queue, env, request = setup('queued-full','full'); ticket = start(queue,env,request,3)
        result = terminal(queue,ticket)
        assert result['status']=='failed' and result['attempts']['current']==3, result
        assert len(result['attempts']['history'])==3
        for n, attempt in enumerate(result['attempts']['history'],1):
            assert attempt['status']=='failed' and attempt['error']['code']=='TOOL_FAILED', attempt
            fault(queue,n)
        call('job.resume', env=env, job_root=str(queue))
        assert status(queue,ticket)==result and not Path(request['output']).exists() and not list(output.glob('.cutbolt-*'))
        unchanged()
        queue, env, request = setup('available-after-failure','once'); ticket = start(queue,env,request,2)
        result = terminal(queue,ticket)
        assert result['status']=='completed' and result['attempts']['current']==2, result
        fault(queue,1); compare(request['output'])
        assert result['result']['sha256']==sha(Path(request['output'])) and not list(output.glob('.cutbolt-*'))
        unchanged()
        passed.append('failure_injection.real_encoded_disk_full_cleanup_and_bounded_retry')

        for queued in [False,True]:
            queue, env, request = setup('changed-queued' if queued else 'changed-sync','changed')
            try:
                if queued:
                    ticket = start(queue,env,request,3); result = terminal(queue,ticket)
                    assert result['status']=='failed' and result['error']['code']=='MEDIA_CHANGED', result
                    assert result['attempts']['current']==1 and (queue/'attempt-count.txt').read_text()=='1'
                else:
                    call('render.run', error='MEDIA_CHANGED', env=env, **request)
                assert movie.read_bytes()==source_bytes+b'original changed-source fixture'
                compare(queue/'encoded-1.mkv')
                assert not Path(request['output']).exists() and not list(output.glob('.cutbolt-*'))
            finally:
                movie.write_bytes(source_bytes)
            unchanged()
        passed.append('failure_injection.changed_source_after_complete_encode_blocks_publication')

        queue, env, request = setup('crash-encoded-partial','hold'); ticket = start(queue,env,request,2)
        until(lambda:(queue/'held.txt').exists(),bool)
        worker = ProcessHandle(status(queue,ticket)['worker_pid']); handles.append(worker)
        helper_pid, encoder_pid = map(int,(queue/'held.txt').read_text().split())
        adapter = ProcessHandle(helper_pid); handles.append(adapter)
        encoder = ProcessHandle(encoder_pid); handles.append(encoder)
        partial = output/f".cutbolt-job-{ticket['job_id']}.partial.mkv"
        before = partial.read_bytes()
        assert len(before)==8192 and before[:4]==bytes.fromhex('1a45dfa3')
        worker.stop(); until(lambda:all(v.exited() for v in [worker,adapter,encoder]),bool,seconds=5)
        result = status(queue,ticket)
        assert result['status']=='queued' and result['attempts']['history'][0]['status']=='interrupted',result
        assert not Path(request['output']).exists() and partial.read_bytes()==before
        (queue/'release').touch(); call('job.resume', env=env, job_root=str(queue))
        result = terminal(queue,ticket)
        assert result['status']=='completed' and result['attempts']['current']==2,result
        compare(request['output'])
        assert partial.read_bytes()==before and len(list(output.glob('.cutbolt-*')))==1
        unchanged()
        passed.append('failure_injection.actual_encode_worker_exit_and_preserved_crash_partial')
        report={'passed':passed, 'frames_compared':frames, 'stereo_sample_frames_compared':samples, 'faults':faults,
                'method':'Original bounded output writer injects Windows ERROR_DISK_FULL after 8192 bytes from real encoding; no physical volume was filled. Worker termination uses live process handles. Changed-source injection occurs after a complete encoded output.',
                'source_and_existing_output_preservation':True, 'crash_partial_preserved':True}
        (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))
    finally:
        for gate in gates:gate.touch(exist_ok=True)
        for queue,job in jobs:
            try:call('job.cancel',job_root=str(queue),job_id=job)
            except (AssertionError,subprocess.TimeoutExpired):pass
        for handle in handles:
            try:
                if not handle.exited():handle.stop()
            finally:handle.close()
        movie.write_bytes(source_bytes)


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
