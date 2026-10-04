"""Actual previous-engine migration, moved media and complete local history recovery."""
import argparse
from array import array
import copy
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import shutil
import sqlite3
import subprocess
import threading
import time as clock

from agents import Client
from jsonschema import Draft202012Validator
from PIL import Image
from tracks import time

ROOT=Path(__file__).resolve().parents[1]


def run(root,legacy):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    media,output,old,portable,moved,recovered=[root/n for n in ('original Ω','outputs','old-store','portable-store','moved Ω','recovered')]
    for p in (media,output,old,portable,moved,recovered):p.mkdir()
    for n in ('clips','previews'):(media/n).mkdir();(moved/n).mkdir()
    exe=ROOT/'target/debug/cutbolt.exe'
    passed=[];rejected=frames=samples=0

    def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
    def identity(path):return {'path':str(path),'bytes':path.stat().st_size,'sha256':sha(path)}
    def ff(args):return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,check=True,timeout=90).stdout
    def call(command,error=None,executable=exe,**fields):
        nonlocal rejected
        p=subprocess.run([str(executable)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=90)
        value=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and value['error']['code']==error,(error,value)
            rejected+=1;return value
        assert p.returncode==0 and value['ok'],value
        return value['result']
    def sqlrows(path):
        with sqlite3.connect(f'{path.as_uri()}?mode=ro',uri=True) as db:
            return {table:db.execute(f'SELECT * FROM {table} ORDER BY 1,2').fetchall() for table in ('projects','revisions','requests')}
    def version(path):
        with sqlite3.connect(f'{path.as_uri()}?mode=ro',uri=True) as db:return db.execute('PRAGMA user_version').fetchone()[0]
    def get(store,revision=None):return call('session.get',store_root=str(store),project_id='portable',**({} if revision is None else {'revision':revision}))
    def mutate(store,key,rev,operations):return call('session.apply',store_root=str(store),project_id='portable',request_id=key,expected_revision=rev,operations=operations)

    w,h,count=64,48,36
    rgb=[bytes(v for y in range(h) for x in range(w) for v in ((x*3+n*7)%256,(y*5+n*11)%256,(x+y+n*17)%256)) for n in range(count)]
    pcm=array('h',(v for n in range(count*1920) for v in ((n*19)%22001-11000,(n*31)%24001-12000)))
    (media/'original.rgb').write_bytes(b''.join(rgb));(media/'original.pcm').write_bytes(pcm.tobytes())
    movie=media/'clips/source.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate','25','-i',str(media/'original.rgb'),
        '-f','s16le','-ar','48000','-ac','2','-i',str(media/'original.pcm'),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3',
        '-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le',str(movie)])
    asset={'id':'source','path':str(movie),'duration':time(count,25),'identity':{k:v for k,v in identity(movie).items() if k!='path'}}
    project=call('project.create',executable=legacy,id='portable',width=w,height=h,frame_rate=time(25))
    project=call('timeline.apply',executable=legacy,project=project,expected_revision=0,operations=[{'op':'media.add','asset':asset},
        {'op':'clip.append','clip':{'id':'left','asset_id':'source','source_in':time(4,25),'duration':time(10,25)}},
        {'op':'clip.append','clip':{'id':'right','asset_id':'source','source_in':time(16,25),'duration':time(8,25)}}])
    create={'store_root':str(old),'project':project,'request_id':'create'}
    original_create=call('session.create',executable=legacy,**create)
    def trim(length):return [{'op':'clip.trim','clip_id':'left','source_in':time(4,25),'duration':time(length,25)}]
    common={'store_root':str(old),'project_id':'portable'}
    call('session.apply',executable=legacy,**common,expected_revision=0,request_id='trim-six',operations=trim(6))
    call('session.undo',executable=legacy,**common,expected_revision=1,request_id='undo')
    call('session.restore',executable=legacy,**common,expected_revision=2,request_id='restore',target_revision=0)
    last={'expected_revision':3,'request_id':'trim-eight','operations':trim(8)}
    last_receipt=call('session.apply',executable=legacy,**common,**last)
    db=old/'projects.sqlite3';assert version(db)==1
    legacy_copy=output/'original-version-one.sqlite3';shutil.copy2(db,legacy_copy)
    before=sqlrows(db);before_bytes=sha(db)
    assert get(old)['revision']==4
    check=call('session.check',store_root=str(old));assert check['migration_required'] and check['revisions']==5
    call('session.apply','MIGRATION_REQUIRED',**common,expected_revision=4,request_id='blocked-before-migration',operations=trim(7))
    assert sha(db)==before_bytes and sqlrows(db)==before
    migration=call('session.migrate',store_root=str(old));assert migration['changed'] and migration['from_version']==1 and migration['to_version']==2
    assert sqlrows(db)==before and version(db)==2
    assert not call('session.migrate',store_root=str(old))['changed']
    assert call('session.create',**create)==original_create
    assert call('session.apply',**common,**last)==last_receipt
    call('session.get','UNSUPPORTED_STORE',executable=legacy,**common)
    assert call('session.check',store_root=str(old))['valid']
    passed.append('portable.actual_version_one_migration_and_exact_history_preservation')

    current=get(old)
    generated=call('proxy.generate',project=current,expected_revision=4,asset_id='source',scale=2,input_root=str(media),output_root=str(media),output=str(media/'previews/source.mkv'))
    mutate(old,'attach-proxy',4,generated['operations']);current=get(old)
    proposal=call('project.portable',project=current,expected_revision=5,input_root=str(media))
    assert not proposal['writes_files'] and not proposal['updates_session'] and not proposal['media_copied']
    assert proposal['project']['assets'][0]['path']=='clips/source.mkv'
    assert proposal['project']['assets'][0]['proxy']['path']=='previews/source.mkv'
    assert get(old)==current
    call('session.preview',**common,expected_revision=5,operations=proposal['operations'])
    moved_receipt=mutate(old,'portable-paths',5,proposal['operations'])
    assert mutate(old,'portable-paths',5,proposal['operations'])==moved_receipt
    head=get(old);assert head==proposal['project']
    assert call('project.portable',project=head,expected_revision=6,input_root=str(media))['operations']==[]
    portable_create={'store_root':str(portable),'project':head,'request_id':'portable-create'}
    portable_receipt=call('session.create',**portable_create)
    for name in ('clips/source.mkv','previews/source.mkv'):shutil.copy2(media/name,moved/name)
    originals={str(p):sha(p) for p in media.rglob('*') if p.is_file()}
    moved_hashes={str(p):sha(p) for p in moved.rglob('*') if p.is_file()}

    def render(project,label,media_root=moved,length=8):
        nonlocal frames,samples
        target=output/(label+'.mkv')
        call('render.run',project=project,input_root=str(media_root),output_root=str(output),output=str(target))
        expected=b''.join(rgb[4:4+length])+b''.join(rgb[16:24])
        sound=pcm[4*1920*2:(4+length)*1920*2].tobytes()+pcm[16*1920*2:24*1920*2].tobytes()
        assert ff(['-i',str(target),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==expected,label
        assert ff(['-i',str(target),'-vn','-f','s16le','-'])==sound,label
        frames+=length+8;samples+=(length+8)*1920
    render(get(portable),'moved-native')
    status=call('registry.status',project=get(portable),input_root=str(moved));assert status['assets'][0]['state']=='online'
    proxy_project=call('timeline.apply',project=get(portable),expected_revision=0,operations=[{'op':'preview.proxy','scale':2}])
    target=output/'moved-preview.png'
    call('preview.frame',project=proxy_project,input_root=str(moved),output_root=str(output),output=str(target),time=time(1,25))
    expected=bytes(v for y in range(h//2) for x in range(w//2) for v in rgb[5][(y*2*w+x*2)*3:(y*2*w+x*2)*3+3])
    with Image.open(target) as image:assert image.size==(w//2,h//2) and image.convert('RGB').tobytes()==expected
    native=call('timeline.apply',project=get(portable),expected_revision=0,
                operations=[{'op':'tracks.edit','edit':{'op':'promote','video_track_id':'v','audio_track_id':'a'}}])
    render(native,'relative-native-tracks')
    nested=copy.deepcopy(native);nested['sequences']=[{'id':'child','arrangement':native['tracks']}]
    nested['tracks']={'duration':time(16,25),'links':[],'tracks':[
        {'id':'root-'+kind,'kind':kind,'enabled':True,'locked':False,'clips':[
            {'id':'instance-'+kind,'sequence_id':'child','start':time(0),'source_in':time(0),'duration':time(16,25)}]}
        for kind in ('video','audio')]}
    render(nested,'relative-nested-tracks')
    cache=root/'cache';cache.mkdir()
    for index in range(2):
        path=output/f'cached-relative-{index}.png'
        result=call('cache.run',cache_root=str(cache),policy={'max_bytes':10000000,'max_entries':32},
                    task={'type':'frame','project':nested,'time':time(1,25),'input_root':str(moved),'output_root':str(output),'output':str(path)})
        assert result['cache']['hit']==bool(index)
        with Image.open(path) as image:assert image.size==(w,h) and image.convert('RGB').tobytes()==rgb[5]
    mutate(portable,'trim-six',0,trim(6));render(get(portable),'moved-edit',length=6)
    call('session.undo',store_root=str(portable),project_id='portable',request_id='undo-edit',expected_revision=1)
    render(get(portable),'moved-undo')
    assert call('session.check',store_root=str(portable))['revisions']==3
    passed.append('portable.relative_sources_proxies_and_saved_relocation')

    ready=threading.Event();release=threading.Event()
    def writer():
        ready.set();assert release.wait(20)
        for index in range(24):
            mutate(portable,f'background-{index}',index+2,[{'op':'media.metadata','asset_id':'source','metadata':{'title':f'Original revision {index}'}}])
            clock.sleep(.01)
    backups=[]
    with ThreadPoolExecutor(max_workers=1) as executor:
        future=executor.submit(writer);assert ready.wait(20);release.set()
        for index in range(3):
            path=output/f'backup-{index}.sqlite3'
            receipt=call('session.backup',store_root=str(portable),output_root=str(output),output=str(path))
            assert receipt['history_preserved'] and not receipt['media_copied']
            assert receipt['identity']['sha256']==sha(path) and receipt['identity']['bytes']==path.stat().st_size
            assert Path(receipt['identity']['path']).samefile(path)
            backups.append(receipt)
        future.result(timeout=60)
    heads=[v['store']['projects'][0]['revision'] for v in backups]
    assert heads==sorted(heads) and heads[0]<get(portable)['revision']==26,heads
    final_backup=output/'complete.sqlite3'
    receipt=call('session.backup',store_root=str(portable),output_root=str(output),output=str(final_backup))
    assert sqlrows(final_backup)==sqlrows(portable/'projects.sqlite3')
    recovery={'source':identity(final_backup),'input_root':str(output),'store_root':str(recovered)}
    result=call('session.recover',**recovery);assert result['history_preserved'] and not result['migration_performed']
    assert sqlrows(recovered/'projects.sqlite3')==sqlrows(final_backup)
    assert call('session.create',**{**portable_create,'store_root':str(recovered)})==portable_receipt
    call('session.restore',store_root=str(recovered),project_id='portable',expected_revision=26,request_id='restore-in-recovery',target_revision=0)
    render(get(recovered),'recovered-head')
    call('session.undo',store_root=str(recovered),project_id='portable',expected_revision=27,request_id='undo-restoration')
    assert get(recovered)['assets'][0]['metadata']['title']=='Original revision 23'
    assert call('session.check',store_root=str(recovered))['revisions']==29
    assert sha(final_backup)==recovery['source']['sha256']
    old_recovery=root/'recovered-version-one';old_recovery.mkdir()
    call('session.recover',source=identity(legacy_copy),input_root=str(output),store_root=str(old_recovery))
    assert version(old_recovery/'projects.sqlite3')==1 and sqlrows(old_recovery/'projects.sqlite3')==before
    call('session.migrate',store_root=str(old_recovery));assert sqlrows(old_recovery/'projects.sqlite3')==before
    passed.append('portable.consistent_live_backup_complete_history_and_recovery')

    call('session.recover','OUTPUT_EXISTS',**recovery)
    call('session.backup','OUTPUT_EXISTS',store_root=str(portable),output_root=str(output),output=str(final_backup))
    call('session.backup','PATH_OUTSIDE_ROOT',store_root=str(portable),output_root=str(output),output=str(root/'outside.sqlite3'))
    fresh=root/'reject-recovery';fresh.mkdir();(fresh/'keep.txt').write_text('Original unrelated file')
    bad=identity(final_backup);bad['sha256']='0'*64
    call('session.recover','MEDIA_CHANGED',source=bad,input_root=str(output),store_root=str(fresh))
    call('session.recover','PATH_OUTSIDE_ROOT',source=identity(final_backup),input_root=str(moved),store_root=str(fresh))
    (fresh/'projects.sqlite3-journal').write_bytes(b'original journal placeholder')
    call('session.recover','OUTPUT_EXISTS',source=identity(final_backup),input_root=str(output),store_root=str(fresh))
    assert (fresh/'projects.sqlite3-journal').read_bytes()==b'original journal placeholder'
    (fresh/'projects.sqlite3-journal').unlink()
    bad_backup=output/'corrupt.sqlite3';shutil.copy2(final_backup,bad_backup)
    with sqlite3.connect(bad_backup) as conn:conn.execute("UPDATE revisions SET undo_target=0 WHERE revision=26")
    call('session.recover','STORE_CORRUPT',source=identity(bad_backup),input_root=str(output),store_root=str(fresh))
    assert not (fresh/'projects.sqlite3').exists() and not list(fresh.glob('.cutbolt-history-*'))
    assert (fresh/'keep.txt').read_text()=='Original unrelated file'
    for label,sql in [('head','UPDATE projects SET head=25'),('request',"UPDATE requests SET payload_hash='"+'0'*64+"' WHERE request_id='background-23'"),('snapshot',"UPDATE revisions SET snapshot='{}' WHERE revision=26")]:
        bad_root=root/('bad-'+label);bad_root.mkdir();shutil.copy2(final_backup,bad_root/'projects.sqlite3')
        with sqlite3.connect(bad_root/'projects.sqlite3') as conn:conn.execute(sql)
        call('session.check','STORE_CORRUPT',store_root=str(bad_root))
    for path in ['../clips/source.mkv','C:source.mkv','']:
        operations=[{'op':'media.paths','asset_id':'source','path':path}]
        call('session.apply','INVALID_PATH',store_root=str(portable),project_id='portable',expected_revision=26,request_id='bad-path',operations=operations)
    assert get(portable)['revision']==26
    malformed=copy.deepcopy(get(portable));malformed['assets'][0]['identity']['sha256']='0'*64
    malformed['assets'][0]['proxy']['source_identity']['sha256']='0'*64
    call('project.portable','MEDIA_CHANGED',project=malformed,expected_revision=26,input_root=str(moved))
    assert not list(output.glob('.cutbolt-history-*'))
    assert originals=={str(p):sha(p) for p in media.rglob('*') if p.is_file()}
    assert moved_hashes=={str(p):sha(p) for p in moved.rglob('*') if p.is_file()}
    passed.append('portable.corrupt_history_boundaries_and_publication_preservation')

    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==63
        calls=[('project.portable',{'project':get(portable),'expected_revision':26,'input_root':str(moved)},True,True),
               ('session.check',{'store_root':str(portable)},True,True),
               ('session.migrate',{'store_root':str(portable)},False,True),
               ('session.backup',{'store_root':str(portable),'output_root':str(output),'output':str(output/'mcp.sqlite3')},False,False)]
        for name,fields,readonly,idempotent in calls:
            tool=next(t for t in catalog if t['name']=='cutbolt_'+name.replace('.','_'))
            Draft202012Validator(tool['inputSchema']).validate(fields)
            assert tool['annotations']['readOnlyHint']==readonly and tool['annotations']['idempotentHint']==idempotent
            client.call(name,**fields)
        fresh=root/'mcp-recovered';fresh.mkdir();fields={'source':identity(output/'mcp.sqlite3'),'input_root':str(output),'store_root':str(fresh)}
        tool=next(t for t in catalog if t['name']=='cutbolt_session_recover');Draft202012Validator(tool['inputSchema']).validate(fields)
        assert client.call('session.recover',**fields)['history_preserved']
    finally:client.close()
    passed.append('portable.typed_local_tools_and_explicit_migration')
    report={'passed':passed,'legacy_engine':identity(legacy),'legacy_store_version':1,'current_store_version':2,
            'frames_compared':frames,'stereo_samples_compared':samples,'rejected':rejected,'concurrent_backup_heads':heads,
            'source_preservation':True,'history_rows_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);parser.add_argument('--legacy-engine',type=Path,required=True)
    args=parser.parse_args();run(args.output,args.legacy_engine)
