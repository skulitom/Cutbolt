"""Load-aware release A/B on placed video/audio from the original twenty-minute source.

Runs five alternating samples per build for a plain edit and a neutral grade that
forces the compositor. Each sample includes a last-frame preview and a queued
ten-second late lossless export. Setup, cache warming and decoded-output checks
are untimed. H.264 delivery from the final build is separately compared with the
explicit conversion reference, so encoder-policy changes cannot skew this A/B.
No source, build or media is downloaded. All generated artifacts stay external.
"""
import argparse
import copy
from datetime import datetime, timezone
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import threading
import time as clock

import psutil
from PIL import Image
from direct_long import PatientClient, busy_fraction, cli, compilers, cpu_times, decoded_digest, tools_at
from footage_sources import ROOT, identity
from source_clocks import time


def rows(path):
    return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []


def project(asset, size, rate, duration, profile):
    tracks=[]
    for kind in ['video','audio']:
        clip=dict(id=kind,asset_id=asset['id'],start=time(0),source_in=time(0),duration=duration)
        if kind=='video' and profile=='neutral_grade':
            clip['effects']=[dict(kind='grade',exposure_milli=0,contrast_milli=1000,white_balance_milli=[1000]*3)]
        tracks.append(dict(id=kind,kind=kind,enabled=True,locked=False,clips=[clip]))
    return dict(schema_version=2,id='placed-long-'+profile,revision=0,width=size[0],height=size[1],
                frame_rate=rate,transfer='bt709',assets=[copy.deepcopy(asset)],clips=[],
                tracks=dict(duration=duration,tracks=tracks,links=[]))


def export(c, p, name, start, duration, profile='reference'):
    path=c['target']/(name+('.mkv' if profile=='reference' else '.mp4'))
    transfer={'input_transfer':'bt709'} if profile=='h264_aac' else {}
    ticket=c['client'].call('job.start',job_root=str(c['jobs']),request_id=name,run='export.run',
        arguments=dict(project=p,input_root=str(c['sources']),output_root=str(c['target']),output=str(path),
            profile=profile,streams='audio_video',range=dict(start=start,duration=duration),**transfer))
    while True:
        state=c['client'].call('job.wait',job_root=str(c['jobs']),job_ids=[ticket['job_id']],timeout_seconds=30)
        if state['finished']:break
    assert state['counts']=={'completed':1},state
    return path,dict(job_id=ticket['job_id'],status='completed')


def preview(c, p, name, at):
    path=c['target']/(name+'.png')
    receipt=c['client'].call('preview.frame',project=p,input_root=str(c['sources']),
                           output_root=str(c['target']),output=str(path),time=at)
    return path,receipt


def source_reads(commands, name, minimum_seek):
    original=[row for row in commands if any(a.endswith(name) for a in row['args'])]
    pictures=[row for row in original if row['tool']=='ffmpeg' and 'rawvideo' in row['args']]
    assert len(pictures)==1 and 'yuv420p' in pictures[0]['args'],pictures
    args=pictures[0]['args']
    seek=float(args[args.index('-ss')+1]) if '-ss' in args else 0
    assert seek>=minimum_seek,(seek,minimum_seek,args)
    assert not any('-show_frames' in row['args'] for row in original if row['tool']=='ffprobe'),original
    return dict(video_decoder_count=1,seek_seconds=seek,no_original_frame_scan=True,commands=original)


def measured(action, max_busy):
    waiting=clock.perf_counter();wait_limit=1800
    while True:
        compiler=compilers()
        if not compiler:
            load=busy_fraction()
            if load<=max_busy and not compilers():break
        assert clock.perf_counter()-waiting<wait_limit,'No admissible quiet interval within 30 minutes'
        clock.sleep(1)
    observed=set();stop=threading.Event()
    def watch():
        while not stop.is_set():observed.update(compilers());stop.wait(.2)
    watcher=threading.Thread(target=watch);watcher.start()
    idle,total=cpu_times();start=clock.perf_counter()
    try:
        result=action();elapsed=clock.perf_counter()-start
    finally:
        end_idle,end_total=cpu_times();stop.set();watcher.join()
    return result,dict(seconds=elapsed,busy_fraction=load,
        during_busy_fraction=1-(end_idle-idle)/(end_total-total),compilers=sorted(observed))


def reference(c, case, original, last_picture, config, start=1190, duration=10):
    folder=c['target']/'reference';folder.mkdir()
    recipe=dict(schema_version=1,id='late-reference',source=dict(file=dict(path=case['path'],**original),
        sdr=dict(matrix='bt709',range='limited',transfer='bt709',missing_tags='reject'),clock='first_video_frame'),
        source_in=time(start),duration=time(duration),rate=time(1),reverse=False,freeze=False,
        width=case['size'][0],height=case['size'][1],
        audio='resample',working_transfer='bt709',frame_rate=time(case['rate']))
    conformed=cli(c['exe'],c['env'],'media.conform',recipe=recipe,input_root=str(c['sources']),
                  output_root=str(folder),output=str(folder/'late.mkv'))
    hashes={};reference_projects={};h264={}
    for profile in ['plain','neutral_grade']:
        p=project(conformed['asset'],case['size'],time(case['rate']),time(duration),profile)
        reference_projects[profile]=p
        target=folder/(profile+'.mkv')
        cli(c['exe'],c['env'],'export.run',project=p,input_root=str(folder),output_root=str(folder),
            output=str(target),profile='reference',streams='audio_video')
        hashes[profile]={stream:decoded_digest(config['COUNTED_FFMPEG'],target,stream=='video') for stream in ['video','audio']}
        frame=folder/(profile+'.png')
        at=time(Fraction(duration)-1/Fraction(case['rate']))
        cli(c['exe'],c['env'],'preview.frame',project=p,input_root=str(folder),output_root=str(folder),output=str(frame),time=at)
        with Image.open(frame) as image:actual=hashlib.sha256(image.convert('RGB').tobytes()).hexdigest()
        assert actual==last_picture[profile],(profile,actual,last_picture[profile])
        if 'projects' in c:
            delivery,_=export(c,c['projects'][profile],profile+'-delivery-proof',time(start),time(duration),'h264_aac')
            reference_delivery=folder/(profile+'.mp4')
            cli(c['exe'],c['env'],'export.run',project=p,input_root=str(folder),output_root=str(folder),
                output=str(reference_delivery),profile='h264_aac',streams='audio_video',input_transfer='bt709')
            h264[profile]={stream:decoded_digest(config['COUNTED_FFMPEG'],delivery,stream=='video') for stream in ['video','audio']}
            for stream,expected in h264[profile].items():
                assert decoded_digest(config['COUNTED_FFMPEG'],reference_delivery,stream=='video')==expected
    (folder/'projects.json').write_text(json.dumps(reference_projects,indent=2))
    return dict(digests=hashes,h264=h264,conform=conformed,preview_matches=True)


def run(before, after, sources, output, max_busy=.2):
    before,after,sources,output=[p.resolve() for p in (before,after,sources,output)]
    assert output!=ROOT and ROOT not in output.parents and not output.is_relative_to(sources)
    assert 0<=max_busy<=1
    output.mkdir(parents=True,exist_ok=True)
    case=json.loads((sources/'manifest.json').read_text())['cases']['h264-20min']
    source=sources/case['path'];original={k:case[k] for k in ['bytes','sha256']}
    assert case['size']==[1920,1080] and case['rate']=='30' and case['encoded_frames']==36000
    assert identity(source)==original
    policy=dict(placement='separate native video/audio tracks, no sequential clips',profiles=['plain','neutral_grade'],
        working_space='unchanged default 8-bit encoded',preview_frame=35999,export_range_seconds=[1190,1200],
        timed_export='lossless FFV1/PCM; unchanged encoding policy in both builds',
        h264='untimed final-build direct-versus-conform equality, including AAC padding',
        rounds_per_build=5,order='alternate before/after, reverse first build each round',warm=True,
        plain_preview_ceiling_seconds=10,maximum_preflight_busy_fraction=max_busy,
        cpu_sample='three seconds immediately before each request; whole-system busy also recorded during each request',
        compilers_allowed=False,export_seconds_ceiling=None)
    policy_path=output/'policy.json'
    if policy_path.exists():assert json.loads(policy_path.read_text())==policy
    else:policy_path.write_text(json.dumps(policy,indent=2)+'\n')
    run_root=output/'runs'/datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S');run_root.mkdir(parents=True)
    config=tools_at(output);contexts={};samples=[];last_picture={};export_paths=[]
    try:
        for label,exe in [('before',before),('after',after)]:
            target=run_root/label;target.mkdir();jobs=target/'jobs';jobs.mkdir()
            persistent=output/label;persistent.mkdir(exist_ok=True);launches=target/'launches.jsonl'
            env={**os.environ,**config,'CUTBOLT_INSPECTION_CACHE':str(persistent/'cache'),'COUNTED_LOG':str(launches)}
            print(label+': inspecting the original and warming caches (untimed)',flush=True)
            inspected=cli(exe,env,'media.inspect',path=str(source),input_root=str(sources))
            assert inspected['timeline']['ready'] and inspected['source_index']['frames']==36000
            asset=inspected['timeline']['asset'];assert asset['identity']==original
            client=PatientClient(exe,env=env);client.initialize()
            c=dict(client=client,exe=exe,env=env,target=target,jobs=jobs,sources=sources,launches=launches,
                engine=identity(exe),build=cli(exe,env,'capabilities',section='all')['build'])
            contexts[label]=c;c['projects']={profile:project(asset,[1920,1080],time(30),time(1200),profile) for profile in policy['profiles']}
            (target/'inspection.json').write_text(json.dumps(inspected,indent=2))
            (target/'projects.json').write_text(json.dumps(c['projects'],indent=2))
            for profile,p in c['projects'].items():
                preview(c,p,profile+'-warm',time('35999/30'))
                export(c,p,profile+'-warm',time(1190),time(10))
        print('Both releases are warm; starting interleaved measurements',flush=True)
        for profile in policy['profiles']:
            for round_ in range(5):
                for label in (['before','after'] if round_%2==0 else ['after','before']):
                    c=contexts[label];p=c['projects'][profile]
                    for method in ['preview','export']:
                        name=f'{profile}-{method}-{round_}'
                        start_calls=len(rows(c['launches']))
                        action=(lambda:preview(c,p,name,time('35999/30'))) if method=='preview' else (lambda:export(c,p,name,time(1190),time(10)))
                        (path,receipt),timing=measured(action,max_busy)
                        commands=rows(c['launches'])[start_calls:]
                        row=dict(build=label,profile=profile,method=method,round=round_,**timing,
                                 source_reads=source_reads(commands,case['path'],1100),receipt=receipt)
                        if method=='preview':
                            assert receipt['timeline_frame']==35999 and receipt['track_id']=='video',receipt
                            with Image.open(path) as image:picture=hashlib.sha256(image.convert('RGB').tobytes()).hexdigest()
                            last_picture.setdefault(profile,picture);assert picture==last_picture[profile]
                            row['pixels']=picture
                        else:export_paths.append((profile,path))
                        samples.append(row);(run_root/'measurements.json').write_text(json.dumps(samples,indent=2))
                        assert not timing['compilers'],('Compiler activity invalidated the retained sample',row)
                        print(json.dumps({k:row[k] for k in ['build','profile','method','round','seconds','busy_fraction','during_busy_fraction']}),flush=True)
        print('Comparing every timed output with the explicit normalization reference (untimed)',flush=True)
        proof=reference(contexts['after'],case,original,last_picture,config)
        for profile,path in export_paths:
            for stream,expected in proof['digests'][profile].items():
                assert decoded_digest(config['COUNTED_FFMPEG'],path,stream=='video')==expected,(profile,path,stream)
        assert identity(source)==original
        timing={}
        for profile in policy['profiles']:
            timing[profile]={}
            for method in ['preview','export']:
                timing[profile][method]={}
                for label in contexts:
                    measured_rows=[r for r in samples if (r['profile'],r['method'],r['build'])==(profile,method,label)]
                    assert len(measured_rows)==5
                    values=[r['seconds'] for r in measured_rows]
                    timing[profile][method][label]=dict(median=statistics.median(values),range=[min(values),max(values)],
                        busy_fraction_range=[min(r['busy_fraction'] for r in measured_rows),max(r['busy_fraction'] for r in measured_rows)])
        report=dict(policy=policy,python=sys.version,psutil=psutil.__version__,source=original,
            engines={label:dict(identity=c['engine'],build=c['build']) for label,c in contexts.items()},
            samples=samples,timing=timing,reference=proof,all_timed_exports_match=True,originals_preserved=True,
            plain_preview_ceiling_met=timing['plain']['preview']['after']['range'][1]<=10)
        (run_root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps(dict(report=str(run_root/'verification.json'),timing=timing,plain_preview_ceiling_met=report['plain_preview_ceiling_met'])),flush=True)
        assert report['plain_preview_ceiling_met'],timing
    finally:
        for c in contexts.values():c['client'].close()


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ['before','after','sources','output']:parser.add_argument('--'+name,type=Path,required=True)
    parser.add_argument('--max-busy-fraction',type=float,default=.2)
    args=parser.parse_args();run(args.before,args.after,args.sources,args.output,args.max_busy_fraction)
