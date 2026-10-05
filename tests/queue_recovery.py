"""Original real-media attempts, interrupted workers and bounded queue recovery."""
from engine import ENGINE
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess

from agents import Client, ProcessHandle, until
from jsonschema import Draft202012Validator
from scenes import time

ROOT = Path(__file__).resolve().parents[1]


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/'sources', root/'output'
    sources.mkdir(); output.mkdir()
    exe = ENGINE
    passed, jobs, gates, handles = [], [], [], []
    compared = samples = rejected = 0

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=60).stdout

    def call(command, error=None, env=None, **fields):
        nonlocal rejected
        p = subprocess.run([str(exe)], input=json.dumps({'command': command, **fields}).encode(), capture_output=True, env=env, timeout=45)
        v = json.loads(p.stdout)
        if error:
            assert p.returncode == 1 and v['error']['code'] == error, v
            rejected += 1
            return v
        assert p.returncode == 0 and v['ok'], v
        return v['result']

    w, h, count = 32, 24, 32
    rgb = bytes(v for n in range(count) for y in range(h) for x in range(w) for v in ((x*7+n*13)%256, (y*11+n*17)%256, (x+y+n*3)%256))
    pcm = b''.join(struct.pack('<hh', (n*29)%40001-20000, (n*41)%36001-18000) for n in range(count*1920))
    (sources/'source.rgb').write_bytes(rgb); (sources/'source.pcm').write_bytes(pcm)
    movie = sources/'source.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate','25','-i',str(sources/'source.rgb'),'-f','s16le','-ar','48000','-ac','2','-i',str(sources/'source.pcm'),'-map','0:v:0','-map','1:a:0','-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)])
    source_bytes = movie.read_bytes()
    original = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    project = call('project.create', id='queue-recovery', width=w, height=h, frame_rate=time(25))
    project = call('timeline.apply', project=project, expected_revision=0, operations=[
        {'op':'media.add','asset':{'id':'source','path':str(movie),'duration':time(count,25)}},
        {'op':'clip.append','clip':{'id':'shot','asset_id':'source','source_in':time(3,25),'duration':time(16,25)}}])
    wrapper = root/'tool.rs'
    wrapper.write_text(r'''
use std::{env,fs,io::Write,path::Path,process::{Command,exit},thread,time::Duration};
fn main(){
 let args:Vec<String>=env::args().skip(1).collect();
 if args.iter().any(|s|s=="-progress") {
  let root=fs::canonicalize(env::var("CUTBOLT_QUEUE_FIXTURE").unwrap()).unwrap();
  let mode=env::var("CUTBOLT_QUEUE_MODE").unwrap();
  if mode=="stall" {
   // A tool that hangs without using CPU or writing progress, as the demo's deadlocked FFmpeg did.
   fs::write(root.join("held.txt"),std::process::id().to_string()).unwrap();
   for _ in 0..1200 {if root.join("release").exists(){break;}thread::sleep(Duration::from_millis(100));}
   exit(75);
  }
  if mode=="pass" {
   let status=Command::new(env::var("CUTBOLT_QUEUE_REAL_TOOL").unwrap()).args(&args).status().unwrap();
   exit(status.code().unwrap_or(1));
  }
  let count=root.join("attempt-count.txt");
  let n=fs::read_to_string(&count).ok().map(|s|s.parse::<u32>().unwrap()).unwrap_or(0)+1;
  fs::write(count,n.to_string()).unwrap();
  let output=Path::new(args.last().unwrap());
  assert!(output.file_name().unwrap().to_str().unwrap().starts_with(".cutbolt-job-"));
  if mode=="always"||(n==1&&(mode=="once"||mode=="source")) {
   fs::write(output,b"original partial fixture").unwrap();
   if mode=="source" {let path=fs::canonicalize(env::var("CUTBOLT_QUEUE_SOURCE").unwrap()).unwrap();
    assert!(path.starts_with(fs::canonicalize(env::var("CUTBOLT_QUEUE_SOURCES").unwrap()).unwrap()));
    fs::OpenOptions::new().append(true).open(path).unwrap().write_all(b"original source change").unwrap();}
   exit(73);
  }
  if mode=="hold"&&n==1 {
   fs::write(output,b"original interrupted partial").unwrap();
   fs::write(root.join("held.txt"),std::process::id().to_string()).unwrap();
   for _ in 0..1200 {if root.join("release").exists(){break;}thread::sleep(Duration::from_millis(100));}
   fs::remove_file(output).unwrap();
  }
 }
 let status=Command::new(env::var("CUTBOLT_QUEUE_REAL_TOOL").unwrap()).args(args).status().unwrap();
 exit(status.code().unwrap_or(1));
}
''', encoding='utf-8')
    helper = root/'tool.exe'
    subprocess.run(['rustc', '--edition=2024', str(wrapper), '-o', str(helper)], capture_output=True, check=True)

    def setup(label, mode):
        queue = root/label; queue.mkdir()
        gates.append(queue/'release')
        environment = {**os.environ, 'CUTBOLT_FFMPEG': str(helper), 'CUTBOLT_QUEUE_FIXTURE': str(queue), 'CUTBOLT_QUEUE_MODE': mode, 'CUTBOLT_QUEUE_REAL_TOOL': shutil.which('ffmpeg'), 'CUTBOLT_QUEUE_SOURCE': str(movie), 'CUTBOLT_QUEUE_SOURCES': str(sources)}
        return queue, environment

    def request(queue, key, attempts=2):
        return {'job_root':str(queue), 'request_id':key, 'render':{'project':project, 'input_root':str(sources), 'output_root':str(output), 'output':str(output/(queue.name+'-'+key+'.mkv')), 'retry':{'max_attempts':attempts}}}

    def start(queue, environment, key='render', attempts=2):
        r = request(queue, key, attempts)
        ticket = call('render.start', env=environment, **r)
        jobs.append((queue,ticket['job_id']))
        return ticket, r

    def status(queue, ticket):
        return call('job.status', job_root=str(queue), job_id=ticket['job_id'])

    def terminal(queue, ticket):
        return until(lambda:status(queue,ticket), lambda v:v['status'] in {'completed','failed','cancelled','interrupted'}, seconds=90)

    def compare(result):
        nonlocal compared, samples
        assert result['status']=='completed', result
        path = Path(result['result']['output'])
        assert ff(['-i',str(path),'-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo','-']) == rgb[3*w*h*3:19*w*h*3]
        assert ff(['-i',str(path),'-map','0:a:0','-f','s16le','-']) == pcm[3*1920*4:19*1920*4]
        assert hashlib.sha256(path.read_bytes()).hexdigest()==result['result']['sha256']
        compared += 16; samples += 16*1920

    try:
        queue, env = setup('retry-success','once')
        ticket, r = start(queue,env)
        result = terminal(queue,ticket); compare(result)
        assert result['attempts']['current']==2 and [v['status'] for v in result['attempts']['history']]==['failed','completed']
        assert call('render.start', env=env, **r)==ticket
        assert status(queue,ticket)==result and not list(output.glob('.cutbolt-*'))
        changed=copy.deepcopy(r);changed['render']['retry']['max_attempts']=3
        call('render.start',env=env,error='REQUEST_ID_CONFLICT',**changed)
        for maximum in [0,4]:
            bad=request(queue,f'invalid-{maximum}',maximum)
            call('render.start',env=env,error='INVALID_RETRY',**bad)
        client=Client(exe)
        try:
            client.initialize();tools=client.rpc('tools/list')['result']['tools']
            schema=next(t['inputSchema'] for t in tools if t['name']=='cutbolt_render_start')
            Draft202012Validator(schema).validate(r)
            assert client.call('job.status',job_root=str(queue),job_id=ticket['job_id'])==result
        finally:client.close()
        passed.append('recovery.actual_retry_pixels_audio_idempotency_and_schema')

        queue, env = setup('exhaustion','always');ticket,_=start(queue,env,attempts=3)
        result=terminal(queue,ticket)
        assert result['status']=='failed' and result['attempts']['current']==3 and len(result['attempts']['history'])==3
        assert (queue/'attempt-count.txt').read_text()=='3' and not list(output.glob('.cutbolt-*'))
        call('job.resume',job_root=str(queue),env=env)
        assert status(queue,ticket)==result
        queue, env = setup('source-change','source');ticket,r=start(queue,env)
        try:
            result=terminal(queue,ticket)
            assert result['status']=='failed' and result['error']['code']=='MEDIA_CHANGED',result
            assert (queue/'attempt-count.txt').read_text()=='1' and not Path(r['render']['output']).exists()
        finally:movie.write_bytes(source_bytes)
        passed.append('recovery.attempt_exhaustion_and_source_pinning')

        queue, env = setup('interruption','hold');ticket,r=start(queue,env)
        until(lambda:(queue/'held.txt').exists(),bool)
        live=status(queue,ticket)
        worker=ProcessHandle(live['worker_pid']);tool=ProcessHandle(int((queue/'held.txt').read_text()));handles.extend([worker,tool])
        partial=output/f".cutbolt-job-{ticket['job_id']}.partial.mkv"
        before=partial.read_bytes();worker.stop();until(lambda:worker.exited() and tool.exited(),bool,seconds=5)
        result=status(queue,ticket)
        assert result['status']=='queued' and result['attempts']['history'][0]['status']=='interrupted'
        (queue/'release').touch();call('job.resume',job_root=str(queue),env=env)
        result=terminal(queue,ticket);compare(result)
        assert result['attempts']['current']==2 and partial.read_bytes()==before
        assert not list(output.glob(f".cutbolt-job-{ticket['job_id']}.attempt-*.partial.mkv"))
        passed.append('recovery.actual_worker_interruption_and_checked_rerender')

        queue, env = setup('bounded','hold');first,_=start(queue,env,'first')
        until(lambda:(queue/'held.txt').exists(),bool)
        queued=[]
        for n in range(31):
            t,_=start(queue,env,f'queued-{n}');queued.append(t)
        call('render.start',env=env,error='QUEUE_FULL',**request(queue,'overflow'))
        assert all(status(queue,t)['status']=='queued' for t in queued)
        assert (queue/'attempt-count.txt').read_text()=='1'
        for t in queued:
            assert call('job.cancel',job_root=str(queue),job_id=t['job_id'])['status']=='cancelled'
        call('job.cancel',job_root=str(queue),job_id=first['job_id'])
        result=terminal(queue,first)
        assert result['status']=='cancelled' and result['attempts']['current']==1
        assert not list(output.glob('bounded-*.mkv'))
        passed.append('recovery.all_32_slots_single_worker_and_cancel_no_retry')

        # Queued commands (job.start) run in their own contained process: frame progress is relayed,
        # cancelling a running one stops its tools, and a watchdog stops one that hangs.
        def export(queue, environment, key):
            arguments = {'project': project, 'input_root': str(sources), 'output_root': str(output),
                         'output': str(output/f'{queue.name}-{key}.mkv'), 'profile': 'reference', 'streams': 'audio_video'}
            ticket = call('job.start', env=environment, job_root=str(queue), request_id=key, run='export.run', arguments=arguments)
            jobs.append((queue, ticket['job_id']))
            return ticket, Path(arguments['output'])
        retained = set(output.glob('.cutbolt-*'))  # the interrupted render's crash partial
        queue, env = setup('command-progress', 'pass'); ticket, target = export(queue, env, 'export')
        result = terminal(queue, ticket)
        assert result['status'] == 'completed' and result['progress'] == {'phase': 'completed', 'frames': 16, 'total_frames': 16}, result
        assert ff(['-i', str(target), '-map', '0:v:0', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-']) == rgb[3*w*h*3:19*w*h*3]
        compared += 16
        queue, env = setup('command-cancel', 'stall'); ticket, target = export(queue, env, 'export')
        until(lambda: (queue/'held.txt').exists(), bool)
        tool = ProcessHandle(int((queue/'held.txt').read_text())); handles.append(tool)
        live = until(lambda: status(queue, ticket), lambda v: v['progress']['phase'] == 'rendering', seconds=10)
        assert live['status'] == 'running' and live['progress']['total_frames'] == 16, live
        cancelled = call('job.cancel', job_root=str(queue), job_id=ticket['job_id'])
        assert cancelled['cancel_requested'] and cancelled['status'] == 'running'
        result = until(lambda: status(queue, ticket), lambda v: v['status'] != 'running', seconds=30)
        assert result['status'] == 'cancelled' and result['error']['code'] == 'JOB_CANCELLED', result
        until(tool.exited, bool, seconds=5)
        assert not target.exists() and set(output.glob('.cutbolt-*')) == retained, list(output.glob('.cutbolt-*'))
        queue, env = setup('command-stall', 'stall'); env['CUTBOLT_JOB_STALL_SECONDS'] = '4'
        ticket, target = export(queue, env, 'export')
        until(lambda: (queue/'held.txt').exists(), bool)
        tool = ProcessHandle(int((queue/'held.txt').read_text())); handles.append(tool)
        result = until(lambda: status(queue, ticket), lambda v: v['status'] != 'running', seconds=60)
        assert result['status'] == 'failed' and result['error']['code'] == 'JOB_STALLED' and not result['cancel_requested'], result
        until(tool.exited, bool, seconds=5)
        assert not target.exists() and set(output.glob('.cutbolt-*')) == retained, list(output.glob('.cutbolt-*'))
        passed.append('recovery.command_progress_cancellation_and_stall_watchdog')
        assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
        report={'passed':passed,'frames_compared':compared,'stereo_sample_frames_compared':samples,'rejections':rejected,'reference':'Original generated RGB/PCM slices; real encoded output, transient failures, killed worker/process tree, pinned source mutation and all 32 queue slots. Publication crash points have separate Rust child-process evidence.'}
        (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
        print(json.dumps(report,indent=2))
    finally:
        for gate in gates:gate.touch(exist_ok=True)
        for queue,job in jobs:
            try:call('job.cancel',job_root=str(queue),job_id=job)
            except Exception:pass
        for handle in handles:handle.close()
        if movie.read_bytes()!=source_bytes:movie.write_bytes(source_bytes)


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
