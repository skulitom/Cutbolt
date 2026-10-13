"""Original stage-2 sources: saved recipes, queued preparation and exact explicit-path exports."""
import argparse
import copy
from fractions import Fraction
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

from engine import ENGINE
from footage_sources import encode, identity, remux, pixels, FFMPEG
from conform_cache import decoded

ROOT = Path(__file__).resolve().parents[1]

def run(root):
    root=root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,outputs,cache,jobs,store=[root/name for name in ('sources','outputs','cache','jobs','store')]
    for path in (sources,outputs,cache,jobs,store):path.mkdir()
    shim=root/('counter.exe' if os.name=='nt' else 'counter')
    subprocess.run(['rustc','--edition=2024',str(ROOT/'tests/conform_count_tool.rs'),'-o',str(shim)],check=True,capture_output=True)
    log=root/'encoder.log'
    env={**os.environ,'CUTBOLT_FFMPEG':str(shim),'CUTBOLT_CONFORM_COUNT_LOG':str(log),
         'CUTBOLT_CONFORM_COUNT_REAL':str(Path(shutil.which(FFMPEG) or FFMPEG).resolve()),'CUTBOLT_CONFORM_CACHE':str(cache)}
    def call(request,code=None,custom=None,workspace=None):
        command=[str(ENGINE)]+(['--workspace',str(workspace)] if workspace else [])
        p=subprocess.run(command,input=json.dumps(request).encode(),capture_output=True,env=custom or env,timeout=240)
        value=json.loads(p.stdout)
        if code:
            assert p.returncode and value['error']['code']==code,value
            return value['error']
        assert p.returncode==0 and value['ok'],(value,p.stderr)
        return value['result']
    def count():return log.read_text().splitlines().count('encode') if log.exists() else 0
    phases=set()
    def queued(name,run,arguments):
        ticket=call({'command':'job.start','job_root':str(jobs),'request_id':name,'run':run,'arguments':arguments})
        for _ in range(240):
            status=call({'command':'job.wait','job_root':str(jobs),'job_id':ticket['job_id'],'timeout_seconds':1})
            phases.add(status['progress']['phase'])
            if status.get('finished'):break
        assert status['status']=='completed',status
        return status['result']
    seed=encode(sources,'phone',rate=Fraction(25),count=25,size=(32,24))
    cases={
        'vfr':encode(sources,'vfr',rate=Fraction(25),count=25,size=(32,24),extra=['-vf',"select='not(eq(mod(n,7),3))'",'-fps_mode','vfr']),
        'rotated':remux(sources,seed,'rotated',['-display_rotation:v:0','90']),
        'hevc10':encode(sources,'hevc10',codec='hevc',pix_fmt='yuv420p10le',rate=Fraction(25),count=25,size=(32,24)),
        'prores':encode(sources,'prores',codec='prores',pix_fmt='yuv422p10le',rate=Fraction(25),count=25,size=(32,24)),
    }
    originals={p.name:identity(p) for p in sources.iterdir()}
    projects={};passed=[];measurements={}
    for name,data in cases.items():
        path=sources/data['path']
        inspection=call({'command':'media.inspect','path':str(path),'input_root':str(sources)})
        assert inspection['timeline']['direct'] is False
        proposed=inspection['timeline']['conform']['recipe']
        assert inspection['timeline']['conform']['cache']['expected_bytes']>0
        project=call({'command':'project.create','id':name,'width':32,'height':24,'frame_rate':25})
        project=call({'command':'timeline.apply','project':project,'expected_revision':0,'operations':[
            {'op':'media.add','input_root':str(sources),'asset':{'id':name,'path':path.name,'duration':proposed['duration']}},
            {'op':'clip.append','clip':{'id':'clip','asset_id':name,'source_in':0,'duration':proposed['duration']}}]})
        asset=project['assets'][0];assert asset['decode']['mode']=='converted'
        assert asset['path']==path.name and asset['identity']==identity(path)
        assert asset['provenance']['original']==identity(path) and 'conversion' not in asset['provenance']
        call({'command':'session.create','store_root':str(store),'project':project,'request_id':'create-'+name})
        saved=call({'command':'session.get','store_root':str(store),'project_id':name})
        assert saved['assets'][0]==asset,saved
        preview={'command':'preview.frame','project':project,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/(name+'.png')),'time':0}
        error=call(preview,'PREPARATION_REQUIRED')
        assert error['detail']['asset_id']==name and error['detail']['request']['run']=='media.prepare'
        assert not Path(preview['output']).exists()
        export={'project':project,'profile':'h264_aac','streams':'audio_video','input_transfer':'bt709',
                'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/(name+'-first.mp4'))}
        before=count();first=queued(name+'-first','export.run',export);cold=count()-before
        export['output']=str(outputs/(name+'-again.mp4'))
        before=count();second=queued(name+'-again','export.run',export);warm=count()-before
        assert (cold,warm)==(1,0),(name,cold,warm)
        call(preview)
        explicit=outputs/(name+'-explicit.mkv')
        receipt=call({'command':'media.conform','recipe':asset['decode']['recipe'],'input_root':str(sources),
                      'output_root':str(outputs),'output':str(explicit)})
        reference=copy.deepcopy(project)
        reference['assets']=[{**receipt['asset'],'path':str(explicit)}]
        reference['clips'][0]['asset_id']=receipt['asset']['id']
        # Output roots are beneath one explicit root for this comparison; originals stay preserved.
        request={**export,'command':'export.run','project':reference,'input_root':str(root),'output':str(outputs/(name+'-reference.mp4'))}
        call(request)
        expected=(decoded(Path(request['output'])),decoded(Path(request['output']),True))
        for suffix in ('first','again'):
            path=outputs/(name+'-'+suffix+'.mp4')
            assert (decoded(path),decoded(path,True))==expected,(name,suffix)
        projects[name]=project
        passed.append('automatic_prepare.'+name+'_exact_h264_and_cached_reuse')
        measurements[name]={'cold_encoders':cold,'warm_encoders':warm,'video_bytes':len(expected[0]),'pcm_bytes':len(expected[1])}
    # Saved interpretations reject identity drift and editorial effects before rendering.
    for path,value in ((('identity','sha256'),'0'*64),(('decode','recipe','reverse'),True)):
        invalid=copy.deepcopy(projects['hevc10'])
        target=invalid['assets'][0]
        for part in path[:-1]:target=target[part]
        target[path[-1]]=value
        call({'command':'media.prepare','cached':True,'project':invalid,'input_root':str(sources),'output_root':str(outputs)},'INVALID_CONFORM')
    # An explicit subdirectory input root must still use the workspace cache by default.
    workspace_env={k:v for k,v in env.items() if k!='CUTBOLT_CONFORM_CACHE'}
    prepared=call({'command':'media.prepare','cached':True,'project':projects['hevc10'],'input_root':str(sources),'output_root':str(outputs)},custom=workspace_env,workspace=root)
    assert (root/Path(prepared['sources']['hevc10']['path'])).parent.samefile(root/'.cutbolt/cache/conform')
    overridden=call({'command':'media.prepare','cached':True,'project':projects['hevc10'],'input_root':str(sources),'output_root':str(outputs)},workspace=root)
    assert (root/Path(overridden['sources']['hevc10']['path'])).parent.samefile(cache)
    passed.append('automatic_prepare.saved_recipe_rejection_and_workspace_cache_defaults')
    # A transparent ProRes original becomes a cached BGRA source on an alpha-over track.
    rgba=bytearray()
    for n in range(25):
        rgb=pixels(n,32,24)
        for i in range(32*24):rgba.extend((*rgb[i*3:i*3+3],(i+n)%5*63))
    alpha=sources/'alpha.mov'
    subprocess.run([FFMPEG,'-v','error','-nostdin','-n','-f','rawvideo','-pix_fmt','rgba','-s','32x24','-r','25','-i','pipe:0',
                    '-i',str(sources/'phone.wav'),'-threads','2','-c:v','prores_ks','-profile:v','4','-pix_fmt','yuva444p10le',
                    '-color_primaries','bt709','-color_trc','bt709','-colorspace','bt709','-color_range','tv','-c:a','pcm_s16le',str(alpha)],
                   input=rgba,check=True,capture_output=True)
    originals[alpha.name]=identity(alpha)
    alpha_project=call({'command':'project.create','id':'alpha','width':32,'height':24,'frame_rate':25})
    alpha_project=call({'command':'timeline.apply','project':alpha_project,'expected_revision':0,'operations':[
        {'op':'media.add','input_root':str(sources),'asset':{'id':'alpha','path':alpha.name,'duration':1}},
        {'op':'tracks.edit','edit':{'op':'create','duration':1}},
        {'op':'tracks.edit','edit':{'op':'add','track':{'id':'overlay','kind':'video','composite':'alpha_over','locked':False,'enabled':True,'clips':[]}}},
        {'op':'tracks.edit','edit':{'op':'place','track_id':'overlay','collision':'reject','clip':{'id':'alpha','asset_id':'alpha','start':0,'source_in':0,'duration':1}}}]})
    assert alpha_project['assets'][0]['decode']['recipe']['source']['alpha']=='straight'
    alpha_export={'project':alpha_project,'profile':'h264_aac','streams':'audio_video','input_transfer':'bt709',
                  'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/'alpha.mp4')}
    before=count();queued('alpha','export.run',alpha_export);assert count()-before==1
    reference=copy.deepcopy(alpha_project)
    explicit=outputs/'alpha-explicit.mkv'
    receipt=call({'command':'media.conform','recipe':alpha_project['assets'][0]['decode']['recipe'],'input_root':str(sources),
                  'output_root':str(outputs),'output':str(explicit)})
    reference['assets']=[{**receipt['asset'],'path':str(explicit)}]
    call({**alpha_export,'command':'export.run','project':reference,'input_root':str(root),'output':str(outputs/'alpha-reference.mp4')})
    assert all(decoded(outputs/'alpha.mp4',audio)==decoded(outputs/'alpha-reference.mp4',audio) for audio in (False,True))
    passed.append('automatic_prepare.transparent_prores_overlay_matches_explicit_conversion')
    # Two absent sources share one preparation working set; neither may evict the other.
    call({'command':'cache.prune','cache_root':str(cache),'kind':'conform','policy':{'max_bytes':0,'max_entries':0}})
    combined=copy.deepcopy(projects['hevc10']);combined['id']='combined'
    combined['assets']+=copy.deepcopy(projects['prores']['assets'])
    combined['clips']+=copy.deepcopy(projects['prores']['clips']);combined['clips'][1]['id']='second'
    missing={'command':'preview.frame','project':combined,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/'combined.png'),'time':0}
    failure=call(missing,'PREPARATION_REQUIRED')
    repair=failure['detail']['request'];repair_jobs=repair['job_root']
    assert Path(repair_jobs).is_dir()
    ticket=call(repair)
    for _ in range(10):
        status=call({'command':'job.wait','job_root':repair_jobs,'job_id':ticket['job_id'],'timeout_seconds':30})
        if status.get('finished'):break
    assert status['status']=='completed',status
    assert len(status['result']['prepared'])==2 and all(a['provenance']['conversion']['bytes']>0 for a in status['result']['prepared'])
    call(missing)
    passed.append('automatic_prepare.runnable_missing_cache_repair_and_multi_source_leases')
    # Name the actually missing second source and offer a fresh idempotency key after eviction.
    cached_prores=Path(status['result']['sources']['prores']['path'])
    assert cached_prores.parent.samefile(cache) and cached_prores.name.startswith('.cutbolt-conform-')
    cached_prores.unlink()
    missing['output']=str(outputs/'repaired-again.png')
    later=call(missing,'PREPARATION_REQUIRED')
    assert later['detail']['asset_id']=='prores'
    assert later['detail']['request']['request_id']!=repair['request_id']
    queued('repair-again','media.prepare',later['detail']['request']['arguments'])
    call(missing)
    passed.append('automatic_prepare.precise_missing_asset_and_fresh_repair_ticket')
    # The older reference-render ticket prepares in a supervised command phase, then resumes rendering.
    call({'command':'cache.prune','cache_root':str(cache),'kind':'conform','policy':{'max_bytes':0,'max_entries':0}})
    rendered=outputs/'background-reference.mkv'
    before=count()
    ticket=call({'command':'render.start','job_root':str(jobs),'request_id':'background-reference','render':{
        'project':combined,'input_root':str(sources),'output_root':str(outputs),'output':str(rendered)}})
    for _ in range(240):
        status=call({'command':'job.wait','job_root':str(jobs),'job_id':ticket['job_id'],'timeout_seconds':1})
        phases.add(status['progress']['phase'])
        if status.get('finished'):break
    assert status['status']=='completed',status
    assert status['progress']['frames']==status['progress']['total_frames']==50,status
    assert count()-before==3  # two missing conversions plus the requested FFV1 render
    for audio in (False,True):
        expected=b''.join(decoded(outputs/(name+'-explicit.mkv'),audio) for name in ('hevc10','prores'))
        assert decoded(rendered,audio)==expected
    passed.append('automatic_prepare.background_reference_render_and_frame_progress')
    # Cancellation stops the source encoder inside the same containment boundary as other jobs.
    call({'command':'cache.prune','cache_root':str(cache),'kind':'conform','policy':{'max_bytes':0,'max_entries':0}})
    cancel_jobs=root/'cancel-jobs';cancel_jobs.mkdir()
    gate=root/'encoder-gate';gate.write_text('hold')
    held={**env,'CUTBOLT_CONFORM_COUNT_GATE':str(gate)}
    cancelled_output=outputs/'cancelled.mkv'
    ticket=call({'command':'render.start','job_root':str(cancel_jobs),'request_id':'cancel-preparation','render':{
        'project':projects['hevc10'],'input_root':str(sources),'output_root':str(outputs),'output':str(cancelled_output)}},custom=held)
    deadline=time.monotonic()+30
    while not gate.with_suffix('.entered').exists():
        assert time.monotonic()<deadline,'preparation encoder did not reach the gate'
        time.sleep(.05)
    status=call({'command':'job.wait','job_root':str(cancel_jobs),'job_id':ticket['job_id'],'timeout_seconds':1},custom=held)
    assert status['progress']['phase']=='preparing sources',status
    phases.add(status['progress']['phase'])
    call({'command':'job.cancel','job_root':str(cancel_jobs),'job_id':ticket['job_id']},custom=held)
    for _ in range(30):
        status=call({'command':'job.wait','job_root':str(cancel_jobs),'job_id':ticket['job_id'],'timeout_seconds':1},custom=held)
        if status.get('finished'):break
    assert status['status']=='cancelled' and not cancelled_output.exists(),status
    gate.unlink()
    # A writer reclaims indexed interrupted work too, if termination crossed the grace period.
    call({'command':'cache.prune','cache_root':str(cache),'kind':'conform','policy':{'max_bytes':0,'max_entries':0}})
    assert not list(cache.glob('.cutbolt-conform-stage-*'))
    passed.append('automatic_prepare.cancelled_source_preparation_and_staging_recovery')
    # A small configured cap fails before a delivery is published; source identities never change.
    small={**env,'CUTBOLT_CONFORM_CACHE_BYTES':'128'}
    call({'command':'media.prepare','cached':True,'project':combined,'input_root':str(sources),'output_root':str(outputs)},'LIMIT_EXCEEDED',small)
    assert originals=={p.name:identity(p) for p in sources.iterdir() if p.is_file()}
    passed.append('automatic_prepare.originals_and_configured_budget')
    report={'passed':passed,'measurements':measurements,'source_preservation':True,'explicit_path_equality':True,'observed_phases':sorted(phases)}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    return report

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    print(json.dumps(run(parser.parse_args().output)))
