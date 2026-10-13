"""Repeated H.264 delivery from identical direct and lossless timeline frames.

The moving, colored 160x90 source exposed varying exports despite identical
encoder-input hashes. Compare every decoded picture and PCM sample, including
AAC padding, for default quality, Main/Baseline and two-pass rate control.
"""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess

from engine import ENGINE
from footage_sources import ROOT, encode, identity
from placed_long import cli, project
from source_clocks import time


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    sources,output=root/'sources',root/'outputs'
    sources.mkdir(parents=True);output.mkdir()
    case=encode(sources,'moving-color')
    original={k:case[k] for k in ['bytes','sha256']}
    env={**os.environ,'CUTBOLT_INSPECTION_CACHE':str(root/'cache')}
    def call(command,**args):return cli(ENGINE,env,command,**args)
    inspected=call('media.inspect',path=str(sources/case['path']),input_root=str(sources))
    direct=project(inspected['timeline']['asset'],[160,90],time(30),time(5),'plain')
    recipe=dict(schema_version=1,id='last-second',source=dict(file=dict(path=case['path'],**original),
        sdr=dict(matrix='bt709',range='limited',transfer='bt709',missing_tags='reject'),clock='first_video_frame'),
        source_in=time(4),duration=time(1),rate=time(1),reverse=False,freeze=False,width=160,height=90,
        audio='resample',working_transfer='bt709',frame_rate=time(30))
    conformed=call('media.conform',recipe=recipe,input_root=str(sources),output_root=str(sources),output=str(sources/'reference.mkv'))
    reference=project(conformed['asset'],[160,90],time(30),time(1),'plain')
    originals={p.name:identity(p) for p in sources.iterdir() if p.is_file()}
    def decoded(path,video):
        args=['-an','-map','0:v:0','-pix_fmt','rgb24','-f','rawvideo'] if video else ['-vn','-map','0:a:0','-f','s16le']
        raw=subprocess.check_output(['ffmpeg','-v','error','-i',str(path),*args,'-'],timeout=120)
        if video:assert len(raw)==30*160*90*3
        else:assert 48000*4<=len(raw)<(48000+1024)*4
        return dict(sha256=hashlib.sha256(raw).hexdigest(),bytes=len(raw))
    settings=[('default',{}),('main',dict(h264=dict(compatibility='main_hd',rate_control=dict(mode='quality',crf=18,maximum_bitrate=12000000,buffer_size=24000000)))),
        ('baseline',dict(h264=dict(compatibility='baseline720p',rate_control=dict(mode='quality',crf=18,maximum_bitrate=6000000,buffer_size=12000000)))),
        ('two-pass',dict(h264=dict(compatibility='high_hd',rate_control=dict(mode='two_pass',bitrate=500000,maximum_bitrate=1000000,buffer_size=2000000))))]
    cases=[];timeline=None
    for name,options in settings:
        expected=None
        for round_ in range(6):
            for kind,p,start in [('direct',direct,4),('conformed',reference,0)]:
                p=copy.deepcopy(p)
                # Exercise both plain and effect compositor paths without changing the frames.
                if round_%2:p['tracks']['tracks'][0]['clips'][0]['effects']=[dict(kind='grade',exposure_milli=0,contrast_milli=1000,white_balance_milli=[1000]*3)]
                path=output/f'{name}-{kind}-{round_}.mp4'
                result=call('export.run',project=p,input_root=str(sources),output_root=str(output),output=str(path),
                    profile='h264_aac',streams='audio_video',input_transfer='bt709',range=dict(start=time(start),duration=time(1)),**options)
                assert result['profile_version']==4 and result['video']['cpu_independent'] is True
                inputs={stream:result['verification']['timeline_'+stream+'_sha256'] for stream in ['video','audio']}
                if timeline is None:timeline=inputs
                assert inputs==timeline,(name,kind,round_,inputs,timeline)
                digests={stream:decoded(path,stream=='video') for stream in ['video','audio']}
                if expected is None:expected=digests
                assert digests==expected,(name,kind,round_,digests,expected)
                assert result['verification']['decoded_video_sha256']==digests['video']['sha256']
                cases.append(dict(profile=name,source=kind,round=round_,grade=bool(round_%2),decoded=digests,encoder_input=inputs))
        print(name+': twelve exports match exactly',flush=True)
    assert originals=={name:identity(sources/name) for name in originals}
    report=dict(passed=['export_repeatability.direct_and_conform_repeated_default_and_selected_profiles'],
        exports=len(cases),frames=30*len(cases),cases=cases,sources_preserved=True,source=case,
        engine=subprocess.check_output([str(ENGINE),'--version']).decode().strip(),
        limits='One synthetic 160x90 30fps colored motion source; decoded equality on the recorded local tool build, not binary identity across builds.')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(dict(exports=report['exports'],frames=report['frames'])))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
