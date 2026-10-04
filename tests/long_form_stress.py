"""4K queue cancellation, actual encoder write faults and repeated latency gates."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time as clock

from agents import ProcessHandle, until
from long_form_4k import monitored, process_tree, run as moving_fixture
from scenes import time

ROOT=Path(__file__).resolve().parents[1]


def run(root):
    root=root.resolve(); assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    fixture=root/'moving';moving_fixture(fixture,False)
    request=json.loads((fixture/'request.json').read_text())
    baseline=json.loads((fixture/'verification.json').read_text());assert not baseline['long_gate_passed']
    exe=Path(os.environ.get('CUTBOLT_TEST_ENGINE',ROOT/'target/debug/cutbolt.exe'));sources=fixture/'sources';output=fixture/'output';passed=[];jobs=[]
    def sha(path):
        with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
    originals={p.name:sha(p) for p in sources.iterdir()}
    sentinel=output/'occupied.mkv';sentinel.write_bytes(b'preexisting content remains');sentinel_hash=sha(sentinel)
    def call(command,error=None,env=None,**fields):
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,env=env,timeout=180)
        reply=json.loads(p.stdout)
        if error:assert p.returncode==1 and reply['error']['code']==error,reply;return reply['error']
        assert p.returncode==0 and reply['ok'],reply;return reply['result']
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=90).stdout
    expected_frames=(output/'decoded.framemd5').read_bytes()
    expected_audio=hashlib.sha256(ff(['-i',request['output'],'-map','0:a:0','-f','s16le','-'])).hexdigest()
    def compare(path,label):
        # The preceding moving fixture established this reference independently
        # from original pixels/sample indices. Each repeated render is fully decoded.
        result=output/(label+'.framemd5')
        ff(['-threads','16','-i',str(path),'-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt','rgb24','-threads','1','-f','framemd5',str(result)])
        assert result.read_bytes()==expected_frames
        assert hashlib.sha256(ff(['-i',str(path),'-map','0:a:0','-f','s16le','-'])).hexdigest()==expected_audio
    repetitions=[]
    for n in range(3):
        current={**request,'output':str(output/f'repeated-{n}.mkv')}
        receipt,metrics=monitored([str(exe)],current,120)
        assert metrics['seconds']<60 and metrics['sampled_tree_peak_bytes']<4*1024**3,metrics
        assert receipt['frames']==300 and receipt['samples']==576000
        compare(current['output'],f'repeated-{n}');repetitions.append(metrics)
    passed.append('long_form_4k.repeated_complete_pipeline_latency')
    helper=root/'write-fault.exe'
    subprocess.run(['rustc','--edition=2024',str(ROOT/'tests/render_failure_tool.rs'),'-o',str(helper)],capture_output=True,check=True)
    failed=root/'failed';failed.mkdir()
    env={**os.environ,'CUTBOLT_FFMPEG':str(helper),'CUTBOLT_FAILURE_REAL_TOOL':shutil.which('ffmpeg'),
         'CUTBOLT_FAILURE_ROOT':str(failed),'CUTBOLT_FAILURE_MODE':'full','CUTBOLT_FAILURE_OUTPUT':str(output)}
    failed_request={k:v for k,v in request.items() if k!='command'};failed_request['output']=str(output/'failed.mkv')
    queue=root/'queue';queue.mkdir()
    cancellation_seconds=None
    child_handles=[]
    try:
        ticket=call('render.start',env=env,job_root=str(failed),request_id='failed',render={**failed_request,'retry':{'max_attempts':2}})
        jobs.append((failed,ticket['job_id']))
        failure=until(lambda:call('job.status',job_root=str(failed),job_id=ticket['job_id']),lambda r:r['status'] in {'failed','completed','interrupted'},seconds=120)
        assert failure['status']=='failed' and failure['attempts']['current']==2,failure
        for n in [1,2]:
            record=json.loads((failed/f'failure-{n}.json').read_text());assert record=={'written':8192,'raw_os_error':112,'injected':True},record
            assert (failed/f'encoded-{n}.mkv').stat().st_size==8192
        assert not Path(failed_request['output']).exists() and not list(output.glob('.cutbolt-*'))
        large=call('project.create',id='cancel-long-4k',width=3840,height=2160,frame_rate=time(25))
        large=call('timeline.apply',project=large,expected_revision=0,operations=[{'op':'clip.append','clip':{
            'id':'long','gap':True,'source_in':time(0),'duration':time(1800)}}])
        cancelled_output=output/'cancelled.mkv'
        ticket=call('render.start',job_root=str(queue),request_id='cancel',render={'project':large,'input_root':str(sources),'output_root':str(output),'output':str(cancelled_output)})
        jobs.append((queue,ticket['job_id']))
        state=until(lambda:call('job.status',job_root=str(queue),job_id=ticket['job_id']),lambda r:r['status']=='running' and r['progress']['frames']>0,seconds=30)
        # Hold OS identities before cancellation; recycled PIDs cannot turn this
        # into a false positive. The active scope and encoder must both exit.
        child_pids=process_tree(state['worker_pid'],child_program=exe.name)
        assert len(child_pids)>=2,child_pids
        child_handles=[ProcessHandle(pid) for pid in child_pids]
        following={k:v for k,v in request.items() if k!='command'};following['output']=str(output/'after-cancel.mkv')
        next_ticket=call('render.start',job_root=str(queue),request_id='after-cancel',render=following);jobs.append((queue,next_ticket['job_id']))
        start=clock.monotonic();call('job.cancel',job_root=str(queue),job_id=ticket['job_id'])
        final=until(lambda:call('job.status',job_root=str(queue),job_id=ticket['job_id']),lambda r:r['status'] in {'cancelled','failed','completed'},seconds=10)
        cancellation_seconds=clock.monotonic()-start;assert final['status']=='cancelled' and cancellation_seconds<10,final
        until(lambda:all(handle.exited() for handle in child_handles),bool,seconds=10)
        assert not cancelled_output.exists()
        next_final=until(lambda:call('job.status',job_root=str(queue),job_id=next_ticket['job_id']),lambda r:r['status'] in {'completed','failed','interrupted'},seconds=120)
        assert next_final['status']=='completed',next_final
        compare(following['output'],'after-cancel')
        assert not list(output.glob('.cutbolt-*'))
        call('render.run',error='OUTPUT_EXISTS',**{**following,'output':str(sentinel)})
        assert sha(sentinel)==sentinel_hash and originals=={p.name:sha(p) for p in sources.iterdir()}
        passed.append('long_form_4k.real_write_failures_cancel_and_queue_recovery')
    finally:
        for job_root,job_id in jobs:
            try:call('job.cancel',job_root=str(job_root),job_id=job_id)
            except Exception:pass
        for handle in child_handles:
            handle.close()
    report={'passed':passed,'moving_fixture':baseline,'repetitions':repetitions,'cancel_seconds':cancellation_seconds,
            'injected_write_failures':2,'cancelled_tool_processes_exited':len(child_handles),'frames_compared':1500,'samples_compared':2880000,
            'gates':{'repetition_seconds':60,'sampled_tree_bytes':4*1024**3,'cancel_seconds':10},'sources_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
