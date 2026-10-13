"""Original conversion-cache fixture; whole-output RGB/PCM, identity, budgets and recovery."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import copy
from fractions import Fraction
import hashlib
import json
import os
from pathlib import Path
import shutil
import sqlite3
import subprocess
import wave

from agents import Client
from engine import ENGINE
from footage_sources import encode, identity, pixels, FFMPEG

ROOT=Path(__file__).resolve().parents[1]
def decoded(path,audio=False):
    args=['-map','0:a:0','-vn','-f','s16le'] if audio else ['-map','0:v:0','-an','-pix_fmt','rgb24','-f','rawvideo']
    return subprocess.check_output([FFMPEG,'-v','error','-i',str(path),*args,'-'],timeout=120)

def run(root):
    root=root.resolve();assert ROOT not in root.parents and root!=ROOT
    root.mkdir(parents=True,exist_ok=True)
    sources=root/'sources';outputs=root/'outputs';cache=root/'cache'
    for p in (sources,outputs,cache):p.mkdir()
    first=encode(sources,'original',codec='ffv1',rate=Fraction(25),count=25,size=(32,24),pix_fmt='bgr0')
    second=encode(sources,'different',codec='ffv1',rate=Fraction(25),count=25,size=(32,24),pix_fmt='bgr0',phase=47)
    originals={p.name:identity(p) for p in sources.iterdir() if p.is_file()}
    recipe={'schema_version':1,'id':'converted','source':{'file':{'path':first['path'],**identity(sources/first['path'])},'color':'encoded_rgb'},
        'source_in':0,'duration':1,'rate':1,'reverse':False,'freeze':False,'width':32,'height':24,'audio':'resample'}
    policy={'max_bytes':8*1024*1024,'max_entries':3}
    def request(value,error=None,env=None):
        p=subprocess.run([str(ENGINE)],input=json.dumps(value).encode(),capture_output=True,timeout=180,env=env)
        result=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and result['error']['code']==error,result
            return result['error']
        assert p.returncode==0 and result['ok'],(result,p.stderr)
        return result['result']
    def cached(name,value=None,budget=None,location=None,error=None,env=None):
        value=copy.deepcopy(value or recipe)
        ext='.mkv' if 'width' in value else '.wav'
        return request({'command':'cache.run','cache_root':str(location or cache),'policy':budget or policy,
            'task':{'type':'conform','recipe':value,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/(name+ext))}},error,env)
    explicit=outputs/'explicit.mkv'
    normal=request({'command':'media.conform','recipe':recipe,'input_root':str(sources),'output_root':str(outputs),'output':str(explicit)})
    a=cached('first');assert not a['cache']['hit'] and a['cache']['decoded_verified']
    expected=(decoded(explicit),decoded(explicit,True))
    assert (decoded(outputs/'first.mkv'),decoded(outputs/'first.mkv',True))==expected
    assert a['result']['details']['decoded']==normal['decoded']
    changed_id={**recipe,'id':'another-id'};hit=cached('second',changed_id)
    assert hit['cache']['hit'] and hit['cache']['key']==a['cache']['key']
    assert hit['result']['asset']['id']=='another-id'
    assert identity(outputs/'first.mkv')==identity(outputs/'second.mkv')
    copied=sources/'relocated.mkv';shutil.copy2(sources/first['path'],copied)
    relocated=copy.deepcopy(recipe);relocated['source']['file']['path']=copied.name
    assert cached('relocated',relocated)['cache']['hit']
    cached('invalid-id',{**recipe,'id':' '},error='INVALID_CONFORM')
    passed=['conform_cache.exact_content_and_relocated_reuse']
    # Recipe and source bytes invalidate independently.
    resized={**recipe,'width':16,'height':12};smaller=cached('resize',resized)
    assert not smaller['cache']['hit'] and smaller['cache']['key']!=a['cache']['key']
    other=copy.deepcopy(recipe);other['source']['file']={'path':second['path'],**identity(sources/second['path'])}
    different=cached('different',other);assert not different['cache']['hit'] and different['cache']['key']!=a['cache']['key']
    passed.append('conform_cache.recipe_and_source_invalidation')
    # A changed payload or metadata is discarded, regenerated and checked, never trusted.
    payload=cache/('.cutbolt-conform-'+a['cache']['key']+'.mkv');data=bytearray(payload.read_bytes());data[len(data)//2]^=1;payload.write_bytes(data)
    repair=cached('repaired');assert not repair['cache']['hit']
    assert (decoded(outputs/'repaired.mkv'),decoded(outputs/'repaired.mkv',True))==expected
    with sqlite3.connect(cache/'cutbolt-conform.sqlite') as db:db.execute('UPDATE entries SET metadata=? WHERE key=?',('{}',a['cache']['key']))
    assert not cached('repaired-metadata')['cache']['hit']
    # Even self-consistent metadata must pass the recorded decoded-digest check on reuse.
    with sqlite3.connect(cache/'cutbolt-conform.sqlite') as db:
        text=db.execute('SELECT metadata FROM entries WHERE key=?',(a['cache']['key'],)).fetchone()[0]
        meta=json.loads(text);meta['value']['decoded']['video_sha256']='0'*64
        text=json.dumps(meta,separators=(',',':'));db.execute('UPDATE entries SET metadata=?,metadata_digest=? WHERE key=?',(text,hashlib.sha256(text.encode()).hexdigest(),a['cache']['key']))
    assert not cached('repaired-decoded')['cache']['hit']
    passed.append('conform_cache.encoded_metadata_and_decoded_integrity')
    # Two cached outputs coexist; newest access controls entry-cap eviction.
    smallcache=root/'lru';smallcache.mkdir();two={'max_bytes':8*1024*1024,'max_entries':2}
    a2=cached('lru-a',location=smallcache,budget=two);b2=cached('lru-b',resized,location=smallcache,budget=two)
    assert cached('lru-touch',location=smallcache,budget=two)['cache']['hit']
    c2=cached('lru-c',other,location=smallcache,budget=two)
    state=request({'command':'cache.inspect','kind':'conform','cache_root':str(smallcache)})
    assert {e['key'] for e in state['entries']}=={a2['cache']['key'],c2['cache']['key']}
    assert state['payload_bytes']==sum(e['bytes'] for e in state['entries'])<=two['max_bytes']
    assert b2['cache']['key'] not in {e['key'] for e in state['entries']}
    prune=request({'command':'cache.prune','kind':'conform','cache_root':str(smallcache),'policy':{'max_bytes':0,'max_entries':0}})
    assert prune['remaining']['payload_bytes']==0 and len(prune['evicted'])==2
    passed.append('conform_cache.lru_inspect_prune')
    # Over-budget output and cancellation-style interrupted production leave no published copy.
    budgetcache=root/'budget';budgetcache.mkdir()
    failure=cached('too-large',location=budgetcache,budget={'max_bytes':4096,'max_entries':2},error='LIMIT_EXCEEDED')
    assert failure['path']=='policy.max_bytes' and 'policy.max_bytes' in failure['hint']
    assert request({'command':'cache.inspect','kind':'conform','cache_root':str(budgetcache)})['payload_bytes']==0
    assert not list(budgetcache.glob('.cutbolt-conform-stage-*')) and not (outputs/'too-large.mkv').exists()
    passed.append('conform_cache.hard_encoded_budget_and_cleanup')
    # Two independent processes request one missing key. Only one publishes it.
    duplicate=root/'duplicate';duplicate.mkdir()
    with ThreadPoolExecutor(max_workers=2) as pool:
        values=list(pool.map(lambda n:cached('concurrent-'+str(n),location=duplicate),range(2)))
    assert sorted(v['cache']['hit'] for v in values)==[False,True]
    assert values[0]['cache']['key']==values[1]['cache']['key']
    assert identity(outputs/'concurrent-0.mkv')==identity(outputs/'concurrent-1.mkv')
    passed.append('conform_cache.concurrent_single_publication')
    # The warm request still decodes for verification, but never launches a conversion encoder.
    shim=root/'counter.exe';log=root/'encoder-launches.log';counted=root/'counted';counted.mkdir()
    subprocess.run(['rustc','--edition=2024',str(ROOT/'tests/conform_count_tool.rs'),'-o',str(shim)],capture_output=True,check=True)
    env={**os.environ,'CUTBOLT_FFMPEG':str(shim),'CUTBOLT_CONFORM_COUNT_LOG':str(log),
         'CUTBOLT_CONFORM_COUNT_REAL':str(Path(shutil.which(FFMPEG) or FFMPEG).resolve())}
    counted_first=cached('counted-first',location=counted,env=env)
    cold_encoders=log.read_text().splitlines().count('encode');assert cold_encoders==1
    log.write_text('');counted_hit=cached('counted-hit',location=counted,env=env)
    warm_encoders=log.read_text().splitlines().count('encode');assert warm_encoders==0 and counted_hit['cache']['hit']
    # A changed selected executable must invalidate the key even with identical behavior.
    with shim.open('ab') as f:f.write(b'original fixture tool identity change')
    log.write_text('');tool_changed=cached('tool-changed',location=counted,env=env)
    assert not tool_changed['cache']['hit'] and tool_changed['cache']['key']!=counted_first['cache']['key']
    assert log.read_text().splitlines().count('encode')==1
    passed.append('conform_cache.counted_encoder_reuse_and_tool_invalidation')
    # Audio-only cache preserves exact PCM without producing a picture.
    wav=copy.deepcopy(recipe);wav.pop('width');wav.pop('height');wav['source']={'file':{'path':'original.wav',**identity(sources/'original.wav')},'color':None}
    wa=cached('audio',wav);wb=cached('audio-hit',wav)
    assert not wa['cache']['hit'] and wb['cache']['hit']
    with wave.open(str(sources/'original.wav'),'rb') as f:pcm=f.readframes(f.getnframes())
    assert decoded(outputs/'audio.wav',True)==pcm
    passed.append('conform_cache.audio_only_reuse')
    # Neither cache namespace collisions nor output collisions can overwrite a user's data.
    before=identity(outputs/'first.mkv');cached('first',error='OUTPUT_EXISTS');assert identity(outputs/'first.mkv')==before
    cached_source=copy.deepcopy(recipe);cached_source['source']['file']={'path':'.cutbolt-conform-'+a['cache']['key']+'.mkv',**identity(payload)}
    request({'command':'cache.run','cache_root':str(cache),'policy':policy,'task':{'type':'conform','recipe':cached_source,
        'input_root':str(cache),'output_root':str(outputs),'output':str(outputs/'recursive.mkv')}},'INVALID_CACHE')
    # Same path, size and modification time with changed valid PCM must not reuse an old identity.
    changing=sources/'changing.wav';shutil.copy2(sources/'original.wav',changing)
    change=copy.deepcopy(wav);change['source']['file']={'path':changing.name,**identity(changing)}
    was=cached('before-change',change);stat=changing.stat();data=bytearray(changing.read_bytes());data[44]^=1;changing.write_bytes(data)
    os.utime(changing,ns=(stat.st_atime_ns,stat.st_mtime_ns))
    cached('stale-identity',change,error='MEDIA_CHANGED')
    change['source']['file'].update(identity(changing));now=cached('after-change',change)
    assert not now['cache']['hit'] and now['cache']['key']!=was['cache']['key']
    assert decoded(outputs/'after-change.wav',True)!=decoded(outputs/'before-change.wav',True)
    passed.append('conform_cache.same_path_same_metadata_content_invalidation')
    # LUT content is a dependency, while relocating an unchanged .cube is a cache hit.
    lut=sources/'grade.cube';lut.write_text('LUT_1D_SIZE 2\n0 0 0\n1 1 1\n')
    graded=copy.deepcopy(recipe);graded['source'].pop('color');graded['source']['sdr']={
        'matrix':'rgb','range':'full','transfer':'bt709','missing_tags':'use_declared'}
    graded['working_transfer']='bt709';graded['lut']={'file':{'path':lut.name,**identity(lut)},'interpolation':'linear'}
    g1=cached('lut',graded);assert not g1['cache']['hit']
    moved=sources/'moved.cube';shutil.copy2(lut,moved);relocated_lut=copy.deepcopy(graded);relocated_lut['lut']['file']['path']=moved.name
    assert cached('lut-relocated',relocated_lut)['cache']['hit']
    lut.write_text('LUT_1D_SIZE 2\n1 1 1\n0 0 0\n');graded['lut']['file'].update(identity(lut))
    g2=cached('lut-changed',graded);assert not g2['cache']['hit'] and g2['cache']['key']!=g1['cache']['key']
    assert decoded(outputs/'lut.mkv')!=decoded(outputs/'lut-changed.mkv')
    passed.append('conform_cache.lut_identity_invalidation')
    # Straight alpha includes hidden RGB at zero coverage; cached verification uses all RGBA bytes.
    rgba=bytearray()
    for n in range(25):
        rgb=pixels(n,32,24)
        for i in range(32*24):rgba.extend((*rgb[i*3:i*3+3],(i+n)%5*63))
    raw=root/'original.rgba';raw.write_bytes(rgba);alpha=sources/'alpha.mkv'
    subprocess.run([FFMPEG,'-v','error','-nostdin','-n','-f','rawvideo','-pix_fmt','rgba','-s','32x24','-r','25','-i',str(raw),
        '-i',str(sources/'original.wav'),'-c:v','ffv1','-level','3','-pix_fmt','bgra','-c:a','pcm_s16le',str(alpha)],check=True)
    alpha_before=identity(alpha)
    transparent=copy.deepcopy(graded);transparent.pop('lut');transparent['source']['file']={'path':alpha.name,**alpha_before}
    transparent['source']['alpha']='straight'
    alpha_first=cached('alpha',transparent);alpha_hit=cached('alpha-hit',transparent)
    assert not alpha_first['cache']['hit'] and alpha_hit['cache']['hit']
    actual=subprocess.check_output([FFMPEG,'-v','error','-i',str(outputs/'alpha-hit.mkv'),'-map','0:v:0','-pix_fmt','rgba','-f','rawvideo','-'])
    assert actual==rgba and alpha_hit['result']['details']['decoded']['video_sha256']==hashlib.sha256(rgba).hexdigest()
    assert identity(alpha)==alpha_before
    passed.append('conform_cache.exact_straight_alpha_and_hidden_rgb')
    # Compact MCP can queue a cache miss with workspace defaults, then inspect/prune its own family.
    task={'type':'conform','recipe':copy.deepcopy(recipe),'output':'outputs/queued.mkv'}
    task['recipe']['source']['file']['path']='sources/original.mkv'
    client=Client(ENGINE,args=('--workspace',str(root),'mcp'))
    try:
        client.initialize()
        ticket=client.call('job.start',request_id='cached-conversion',run='cache.run',arguments={'policy':policy,'task':task})
        for _ in range(6):
            status=client.call('job.wait',job_id=ticket['job_id'],timeout_seconds=30)
            if status.get('finished'):break
        assert status['status']=='completed' and status['result']['cache']['kind']=='conform',status
        envelope=client.rpc('tools/call',{'name':'cutbolt_run','arguments':{'command':'cache.inspect','arguments':{'kind':'conform'}}})['result']['structuredContent']
        assert envelope['ok'],envelope
        state=envelope['result']
        assert len(state['entries'])==1 and state['payload_bytes']>0
    finally:client.close()
    assert (root/'.cutbolt/cache/conform/cutbolt-conform.sqlite').is_file()
    assert (decoded(outputs/'queued.mkv'),decoded(outputs/'queued.mkv',True))==expected
    passed.append('conform_cache.compact_mcp_queue_and_workspace_defaults')
    assert originals=={name:identity(sources/name) for name in originals}
    passed.append('conform_cache.originals_and_destinations_preserved')
    report={'passed':passed,'kind':'explicit_cached_conform','video_frames_compared':25*4,'sample_frames_compared':48000*6,
        'encoder_launches':{'cold':cold_encoders,'warm':warm_encoders,'changed_tool':1},
        'decoded_cache_hits_verified':True,'automatic_timeline_preparation':False,'cache_keys':{'original':a['cache']['key'],'resized':smaller['cache']['key'],'changed_source':different['cache']['key']}}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n');return report

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    print(json.dumps(run(parser.parse_args().output)))
