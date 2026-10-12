"""Sample-exact indexed audio ranges versus complete explicit conversion."""
import hashlib
import json
import os
import shutil
import subprocess
import sqlite3

from direct_decode import helper
from footage_sources import FFMPEG, identity
from source_clocks import decoded


def verify(root, manifest, indexed):
    requests, expected, names = [], [], []
    for name, case in manifest['cases'].items():
        if not indexed[name]['index']['eligible'] or name=='h264-20min':
            continue
        reference = decoded(root/'conform'/(name+'.mkv'),False)
        count = len(reference)//4
        windows = sorted({(0,1),(1,1),(count-1,1),(count-2,2),(0,count),
                          (count-4800,4800),(1023,4),(44101,1025),(count//2,1001)})
        requests.append({'root':str(root/'sources'),'file':{'path':case['path'],**identity(root/'sources'/case['path'])},
                         'audio':True,'ranges':[*windows,(count,1),(0,0),(2**64-1,2)]})
        expected.append([hashlib.sha256(reference[a*4:(a+b)*4]).hexdigest() for a,b in windows])
        names.append(name)
    binary = helper(root)
    cache = root/'audio-reader-cache'
    env = {**os.environ, 'CUTBOLT_INSPECTION_CACHE':str(cache)}
    process = subprocess.run([str(binary)],input=json.dumps(requests),capture_output=True,
                             text=True,encoding='utf-8',timeout=1800,env=env)
    assert process.returncode==0,process.stderr
    output=json.loads(process.stdout)
    (root/'direct-audio-observed.json').write_text(json.dumps(output,indent=2),encoding='utf-8')
    reports={}
    for name,request,hashes,result in zip(names,requests,expected,output,strict=True):
        assert 'error' not in result, (name,result)
        for (first,count),digest,actual in zip(request['ranges'],hashes,result['ranges']):
            assert actual.get('sha256')==digest,(name,first,count,actual,digest)
            assert actual['receipt']['first']==first and actual['receipt']['samples']==count
        assert all(r['error']['code']=='INVALID_RANGE' for r in result['ranges'][-3:]),(name,result)
        receipts = [r['receipt'] for r in result['ranges'][:-3]]
        active = [r for r in receipts if r['mode']!='silence']
        canonical = any(r['mode']=='canonical_cache' for r in active)
        if canonical:
            assert all(r['decoded_native_samples']==0 and r['cached_chunks']>0 and r['rebuilt_chunks']==0 for r in active), (name,receipts)
            assert result['commands'][result['commands_before_ranges']:] == [], name
        else:
            assert any(r['seek_seconds'] is not None for r in active),name
        assert identity(root/'sources'/request['file']['path'])=={k:request['file'][k] for k in ('bytes','sha256')}
        reports[name]={'windows':result['ranges'][:-3],'sources_preserved':True}
    reports['rejections_and_silence']=verify_auxiliary(root,manifest,binary)
    reports['cache']=verify_cache(root,requests,expected,output,binary,env,cache)
    return reports


def verify_auxiliary(root,manifest,binary):
    auxiliary = root/'audio-cases'
    auxiliary.mkdir(exist_ok=True)
    original = root/'sources'/manifest['cases']['h264-25']['path']
    checks = []
    for mode in ('before','during'):
        copy = auxiliary/(mode+'.mp4')
        shutil.copy2(original,copy)
        checks.append({'root':str(auxiliary),'file':{'path':copy.name,**identity(copy)},
                       'audio':True,'ranges':[[0,48000]],'mutation':mode})
    for flag in ('corrupt_pcm','sink_error'):
        checks.append({'root':str(original.parent),'file':{'path':original.name,**identity(original)},
                       'audio':True,'ranges':[[0,48000]],flag:True})
    video_only = auxiliary/'video-only.mp4'
    subprocess.run([FFMPEG,'-v','error','-nostdin','-n','-i',str(original),'-map','0:v:0',
                    '-c:v','copy',str(video_only)],check=True,capture_output=True)
    checks.append({'root':str(auxiliary),'file':{'path':video_only.name,**identity(video_only)},
                   'audio':True,'ranges':[[0,48000],[1234,321]]})
    process = subprocess.run([str(binary)],input=json.dumps(checks),capture_output=True,
                             text=True,encoding='utf-8',timeout=180,env={**os.environ,'CUTBOLT_INSPECTION_CACHE':''})
    assert process.returncode==0,process.stderr
    faults=json.loads(process.stdout)
    for result,code in zip(faults[:4],('MEDIA_CHANGED','MEDIA_CHANGED','RENDER_VALIDATION_FAILED','TOOL_ABORTED'),strict=True):
        assert result['ranges'][0]['error']['code']==code,result
    assert faults[2]['ranges'][0]['error']['message']=='Canonical audio chunk disagrees with its source index'
    assert faults[3]['ranges'][0]['error']['message']=='fixture sink stopped'
    for n,actual in zip((48000,321),faults[4]['ranges'],strict=True):
        assert actual['sha256']==hashlib.sha256(bytes(n*4)).hexdigest(),actual
        assert actual['receipt']['decoded_native_samples']==0
    return {'changed_before':True,'changed_during':True,
        'altered_pcm_rejected':True,'consumer_error_preserved':True,'video_only_silence':True}


def verify_cache(root,requests,hashes,observed,binary,env,cache):
    selected = next(n for n,result in enumerate(observed) if result['ranges'][0]['receipt']['mode']=='canonical_cache')
    request = requests[selected]
    wanted = hashes[selected]
    def read():
        process=subprocess.run([str(binary)],input=json.dumps([request]),capture_output=True,text=True,encoding='utf-8',timeout=180,env=env)
        assert process.returncode==0,process.stderr
        result=json.loads(process.stdout)[0]
        assert 'error' not in result,result
        assert [r.get('sha256') for r in result['ranges'][:-3]]==wanted,result
        return result
    warm=read()
    assert warm['commands'][warm['commands_before_ranges']:] == []
    database=cache/'audio/cutbolt-cache.sqlite'
    with sqlite3.connect(database) as connection:
        count,total=connection.execute('SELECT count(*),sum(bytes) FROM entries').fetchone()
        assert 0<count<=4096 and 0<total<=1024**3
        assert connection.execute('SELECT max(bytes) FROM entries').fetchone()[0]<=1024**2
        # Corruption in an owned cache must rebuild from canonical content, never alter output.
        connection.execute('UPDATE entries SET data=zeroblob(bytes)')
    rebuilt=read()
    assert any(r['receipt']['rebuilt_chunks']>0 for r in rebuilt['ranges'][:-3]),rebuilt
    assert sum('pcm_s16le' in args for args in rebuilt['commands'][rebuilt['commands_before_ranges']:]) == 1
    repaired=read()
    assert repaired['commands'][repaired['commands_before_ranges']:] == []
    # Prune the audio sub-cache through the public existing command; cached index stays valid.
    from engine import ENGINE
    process=subprocess.run([str(ENGINE)],input=json.dumps({'command':'cache.prune','cache_root':str(cache/'audio'),
        'policy':{'max_bytes':0,'max_entries':0}}),capture_output=True,text=True,encoding='utf-8',timeout=90)
    assert process.returncode==0,process.stdout
    assert json.loads(process.stdout)['result']['cache']['payload_bytes']==0
    evicted=read()
    assert any(r['receipt']['rebuilt_chunks']>0 for r in evicted['ranges'][:-3])
    assert not list((cache/'audio').glob('.cutbolt-cache-*.pcm'))
    return {'cross_process_warm_without_decoder':True,'corruption_rebuilt_exactly':True,'prune_rebuilt_exactly':True,
            'chunks_before_corruption':count,'payload_bytes_before_corruption':total,'maximum_chunk_bytes':1024**2}
