"""Release-build A/B previews and late-range exactness for the original 20-minute fixture.

Generate with tests/footage_sources.py's generate(root, long=True), save before/after releases,
then supply --before, --after, --sources and an external --output directory. No builds occur
during timed requests. CPU load is sampled for three seconds before every interleaved run.
"""
import argparse
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import threading
import time as clock

import psutil
from PIL import Image
from agents import Client
from footage_sources import ROOT, identity
from source_clocks import time


class PatientClient(Client):
    def receive(self):
        line = self.lines.get(timeout=1800)
        assert line is not None, self.process.stderr.read()
        return json.loads(line)


def cpu_times():
    if os.name == 'nt':
        idle, kernel, user = [ctypes.c_ulonglong() for _ in range(3)]
        if not ctypes.windll.kernel32.GetSystemTimes(ctypes.byref(idle),ctypes.byref(kernel),ctypes.byref(user)):
            raise ctypes.WinError()
        return idle.value, kernel.value + user.value
    values = [int(n) for n in Path('/proc/stat').read_text().splitlines()[0].split()[1:9]]
    return values[3] + values[4], sum(values)


def busy_fraction():
    idle, total = cpu_times()
    clock.sleep(3)
    end_idle, end_total = cpu_times()
    return 1 - (end_idle-idle)/(end_total-total)


def compilers():
    names={'rustc','cargo','cl','link','gcc','g++','clang','clang++','cc1','cc1plus','ld','lld','rust-lld'}
    found=set()
    for process in psutil.process_iter(['name','exe']):
        name=(process.info['name'] or '').lower()
        if name.removesuffix('.exe') not in names:continue
        path=(process.info['exe'] or '').replace('\\','/').lower()
        # Gigabyte Control Center shares GNU C's executable name; it does no compilation.
        if name=='gcc.exe' and path.endswith('/gigabyte/control center/gcc.exe'):continue
        found.add(name)
    return sorted(found)


def tools_at(output):
    config = output/'tools.json'
    if config.exists():
        return json.loads(config.read_text())
    folder=output/'tools';folder.mkdir()
    code=folder/'counted.rs'
    code.write_text(r'''
use std::{env,fs::OpenOptions,io::Write,process::{Command,exit}};
fn main(){
 let probe=env::current_exe().unwrap().file_stem().unwrap().to_string_lossy().contains("probe");
 let args:Vec<String>=env::args().skip(1).collect();
 let mut log=OpenOptions::new().create(true).append(true).open(env::var("COUNTED_LOG").unwrap()).unwrap();
 writeln!(log,"{{\"tool\":\"{}\",\"args\":{:?}}}",if probe {"ffprobe"} else {"ffmpeg"},args).unwrap();
 let real=env::var(if probe {"COUNTED_FFPROBE"} else {"COUNTED_FFMPEG"}).unwrap();
 exit(Command::new(real).args(args).status().unwrap().code().unwrap_or(1));
}
''')
    decoder,probe=folder/'count-decoder.exe',folder/'count-probe.exe'
    subprocess.run(['rustc','--edition=2024',str(code),'-o',str(decoder)],check=True,capture_output=True)
    shutil.copy2(decoder,probe)
    value=dict(CUTBOLT_FFMPEG=str(decoder),CUTBOLT_FFPROBE=str(probe),
               COUNTED_FFMPEG=shutil.which('ffmpeg'),COUNTED_FFPROBE=shutil.which('ffprobe'))
    config.write_text(json.dumps(value,indent=2))
    return value


def cli(executable, env, command, **arguments):
    result=subprocess.run([str(executable)],input=json.dumps(dict(command=command,**arguments)),
                          text=True,encoding='utf-8',capture_output=True,env=env,timeout=1800)
    value=json.loads(result.stdout)
    assert result.returncode==0 and value['ok'], value
    return value['result']


def decoded_digest(ffmpeg, path, video):
    args=['-map','0:v:0','-an','-fps_mode','passthrough','-pix_fmt','rgb24','-f','rawvideo'] if video else [
        '-map','0:a:0','-vn','-c:a','pcm_s16le','-f','s16le']
    process=subprocess.Popen([ffmpeg,'-v','error','-nostdin','-threads','16','-i',str(path),*args,'pipe:1'],stdout=subprocess.PIPE)
    digest=hashlib.sha256();size=0
    while chunk:=process.stdout.read(1<<20):
        digest.update(chunk);size+=len(chunk)
    assert process.wait(timeout=120)==0
    return dict(sha256=digest.hexdigest(),bytes=size)


def run(before, after, sources, output):
    before,after,sources,output=[p.resolve() for p in (before,after,sources,output)]
    assert output!=ROOT and ROOT not in output.parents and not output.is_relative_to(sources)
    output.mkdir(parents=True,exist_ok=True)
    case=json.loads((sources/'manifest.json').read_text())['cases']['h264-20min']
    source=sources/case['path'];original={k:case[k] for k in ('sha256','bytes')}
    assert case['size']==[1920,1080] and case['rate']=='30' and case['encoded_frames']==36000
    policy=dict(frame=35999,preview_max_seconds=10,rounds_per_build=5,
                order='alternating before/after; swap first build each round',
                cpu_load='GetSystemTimes system busy fraction during 3 seconds immediately before each request' if os.name=='nt' else '/proc/stat busy fraction over 3 seconds',
                warm=True,compiling_during_measurements=False)
    if (output/'policy.json').exists():
        previous=json.loads((output/'policy.json').read_text())
        assert all(previous[k]==policy[k] for k in ('frame','preview_max_seconds','rounds_per_build','warm'))
    else:
        (output/'policy.json').write_text(json.dumps(policy,indent=2))
    run_root=output/'runs'/datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S')
    run_root.mkdir(parents=True)
    config=tools_at(output)
    contexts={};records=[];expected=None
    def rows(path):
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
    try:
        for label, executable in [('before',before),('after',after)]:
            folder=output/label;folder.mkdir(exist_ok=True)
            target=run_root/label;target.mkdir()
            launches=folder/'tool-launches.jsonl'
            env={**os.environ,**config,'CUTBOLT_INSPECTION_CACHE':str(folder/'cache'),'COUNTED_LOG':str(launches)}
            inspected=cli(executable,env,'media.inspect',path=str(source),input_root=str(sources))
            assert inspected['timeline']['ready'] and inspected['source_index']['frames']==36000,inspected
            asset=inspected['timeline']['asset'];assert asset['identity']==original
            (target/'inspection.json').write_text(json.dumps(inspected,indent=2))
            project=dict(schema_version=1,id='long-original',revision=0,width=1920,height=1080,
                         frame_rate=time(30),assets=[asset],clips=[dict(id='shot',asset_id=asset['id'],
                         source_in=time(0),duration=time(1200))])
            client=PatientClient(executable,env=env);client.initialize()
            contexts[label]=dict(client=client,env=env,exe=executable,target=target,project=project,launches=launches,
                                 engine=identity(executable),build=cli(executable,env,'capabilities',section='all')['build'])
            # Cache/index work and lookup-table construction are outside the timed requests.
            client.call('preview.frame',project=project,input_root=str(sources),output_root=str(target),
                        output=str(target/'warm.png'),time={'num':35999,'den':30})
        for round_ in range(5):
            for label in (['before','after'] if round_%2==0 else ['after','before']):
                c=contexts[label];path=c['target']/f'frame-{round_}.png'
                while True:
                    while compilers():clock.sleep(1)
                    load=busy_fraction()
                    if not compilers():break
                observed=set();stop=threading.Event()
                def watch():
                    while not stop.is_set():
                        observed.update(compilers());stop.wait(.2)
                watcher=threading.Thread(target=watch);watcher.start()
                start_calls=len(rows(c['launches']));start=clock.perf_counter()
                try:
                    receipt=c['client'].call('preview.frame',project=c['project'],input_root=str(sources),
                        output_root=str(c['target']),output=str(path),time={'num':35999,'den':30})
                    elapsed=clock.perf_counter()-start
                finally:
                    stop.set();watcher.join()
                assert receipt['source_frame']==35999 and receipt['decode']['seek_frame']>35000
                with Image.open(path) as image: pixels=hashlib.sha256(image.convert('RGB').tobytes()).hexdigest()
                expected=expected or pixels;assert pixels==expected
                records.append(dict(build=label,round=round_,seconds=elapsed,busy_fraction=load,pixels=pixels,
                                    compilers=sorted(observed),decode=receipt['decode'],commands=rows(c['launches'])[start_calls:]))
                (run_root/'preview-runs.json').write_text(json.dumps(records,indent=2))
                assert not observed,('Compiler activity invalidated this run; its measurements are retained',sorted(observed))
                print(json.dumps({k:records[-1][k] for k in ('build','round','seconds','busy_fraction')}),flush=True)
        c=contexts['after'];target=c['target'];queue=target/'jobs';queue.mkdir()
        direct=target/'late-range.mp4';start_calls=len(rows(c['launches']))
        ticket=c['client'].call('job.start',job_root=str(queue),request_id='late-range',run='export.run',
            arguments=dict(project=c['project'],input_root=str(sources),output_root=str(target),output=str(direct),
                profile='h264_aac',streams='audio_video',input_transfer='bt709',range=dict(start=time(1190),duration=time(10))))
        while True:
            job=c['client'].call('job.wait',job_root=str(queue),job_ids=[ticket['job_id']],timeout_seconds=30)
            if job['finished']:break
        assert job['counts']=={'completed':1},job
        commands=rows(c['launches'])[start_calls:]
        reads=[r for r in commands if r['tool']=='ffmpeg' and any(a.endswith(case['path']) for a in r['args'])]
        assert len(reads)==1 and 'rawvideo' in reads[0]['args'] and 'yuv420p' in reads[0]['args'],reads
        args=reads[0]['args'];assert '-ss' in args and float(args[args.index('-ss')+1])>1100,args
        assert not any('-show_frames' in r['args'] for r in commands),commands
        reference=run_root/'reference';reference.mkdir()
        recipe=dict(schema_version=1,id='long-window',source=dict(file=dict(path=case['path'],**original),
            sdr=dict(matrix='bt709',range='limited',transfer='bt709',missing_tags='reject'),clock='first_video_frame'),
            source_in=time(1190),duration=time(10),rate=time(1),reverse=False,freeze=False,width=1920,height=1080,
            audio='resample',working_transfer='bt709',frame_rate=time(30))
        conformed=cli(after,c['env'],'media.conform',recipe=recipe,input_root=str(sources),
                      output_root=str(reference),output=str(reference/'window.mkv'))
        p=dict(c['project'],assets=[conformed['asset']],clips=[dict(id='shot',asset_id=conformed['asset']['id'],source_in=time(0),duration=time(10))])
        cli(after,c['env'],'export.run',project=p,input_root=str(reference),output_root=str(reference),
            output=str(reference/'window.mp4'),profile='h264_aac',streams='audio_video',input_transfer='bt709')
        digests={}
        for video in [True,False]:
            actual=decoded_digest(config['COUNTED_FFMPEG'],direct,video)
            assert actual==decoded_digest(config['COUNTED_FFMPEG'],reference/'window.mp4',video)
            digests['video' if video else 'audio']=actual
        final=subprocess.check_output([config['COUNTED_FFMPEG'],'-v','error','-threads','16','-i',str(reference/'window.mkv'),
            '-an','-vf','select=eq(n\\,299)','-frames:v','1','-pix_fmt','rgb24','-f','rawvideo','pipe:1'],timeout=180)
        assert len(final)==1920*1080*3 and hashlib.sha256(final).hexdigest()==expected
        assert identity(source)==original
        timing={label:dict(median=statistics.median(r['seconds'] for r in records if r['build']==label),
                          range=[min(r['seconds'] for r in records if r['build']==label),max(r['seconds'] for r in records if r['build']==label)])
                for label in contexts}
        report=dict(policy=policy,python=sys.version,psutil=psutil.__version__,engines={label:dict(identity=c['engine'],build=c['build']) for label,c in contexts.items()},
                    previews=records,timing=timing,export=dict(commands=commands,digests=digests,job=job,reference=conformed),
                    originals_preserved=True,last_frame_matches_conform=True,
                    after_ceiling_met=timing['after']['range'][1]<=10)
        (run_root/'verification.json').write_text(json.dumps(report,indent=2))
        print(json.dumps(dict(report=str(run_root/'verification.json'),timing=timing,after_ceiling_met=report['after_ceiling_met'])))
        assert report['after_ceiling_met'],timing
    finally:
        for c in contexts.values():c['client'].close()


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ('before','after','sources','output'):parser.add_argument('--'+name,type=Path,required=True)
    args=parser.parse_args()
    run(args.before,args.after,args.sources,args.output)
