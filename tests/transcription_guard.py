"""Original failure fixtures for the exact production supervisor and worker.

Helper workers exist only in external test directories. The native command has
no worker override; its separate tests invoke the embedded production worker.
"""
import hashlib
import json
from pathlib import Path
import subprocess
import threading
import time

ROOT=Path(__file__).resolve().parents[1]


def linux(path):
    path=Path(path).resolve()
    assert len(path.drive)==2 and path.drive[1]==':'
    return '/mnt/'+path.drive[0].lower()+'/'+path.as_posix()[3:]


def wsl(runtime,code,*args):
    result=subprocess.run(['wsl','--distribution',runtime['distribution'],'--exec',runtime['python'],'-c',code,*args],
        capture_output=True,check=True,timeout=20)
    return json.loads(result.stdout)


def processes(runtime,needle):
    return wsl(runtime,"""import json,os,sys
from pathlib import Path
out=[]
for path in Path('/proc').glob('[0-9]*'):
 try:
  argv=(path/'cmdline').read_bytes().split(b'\\0')
  if int(path.name)!=os.getpid() and any(sys.argv[1].encode() in a for a in argv):
   out.append({'pid':int(path.name),'namespace':os.readlink(path/'ns/pid'),'argv':[a.decode(errors='replace') for a in argv]})
 except (OSError,ValueError):pass
print(json.dumps(out))
""",needle)


def no_namespaces(runtime,namespaces):
    alive=wsl(runtime,"""import json,os,sys
from pathlib import Path
wanted=set(json.loads(sys.argv[1]));out=[]
for path in Path('/proc').glob('[0-9]*'):
 try:
  if os.readlink(path/'ns/pid') in wanted:out.append(int(path.name))
 except OSError:pass
print(json.dumps(out))
""",json.dumps(list(namespaces)))
    assert not alive,('owned namespace survived',alive)


def drain(stream,output):
    output.append(stream.read())


HELPER=r'''import json,os,subprocess,sys,time
from pathlib import Path
request=json.load(sys.stdin);root=Path(request['record_root']);mode=request['mode']
(root/'namespace.txt').write_text(os.readlink('/proc/self/ns/pid'))
if mode in ('timeout','owner-eof','bridge-kill','success-detached'):
 code="import os,time; from pathlib import Path; p=Path("+repr(str(root/'heartbeat'))+");\nwhile True:\n with p.open('ab') as f:f.write(b'x');f.flush()\n time.sleep(.03)"
 subprocess.Popen([sys.executable,'-c',code],stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,start_new_session=True)
 deadline=time.monotonic()+3
 while not (root/'heartbeat').exists():
  if time.monotonic()>deadline:raise RuntimeError('No detached child heartbeat')
  time.sleep(.01)
if mode in ('timeout','owner-eof','bridge-kill'):time.sleep(60)
if mode=='malformed':print('broken-json')
elif mode=='oversized':print('x'*1100000)
elif mode=='stderr-limit':sys.stderr.write('x'*70000)
else:print(json.dumps({'ok':True,'result':{'mode':mode}}))
if mode=='nonzero':sys.exit(7)
'''


def run(root,runtime):
    root=Path(root);root.mkdir();records=[];namespaces=set()
    supervisor=(ROOT/'tools/transcribe_supervisor.py').read_bytes()
    for mode in ['success','nonzero','malformed','oversized','stderr-limit','timeout','owner-eof','bridge-kill','success-detached','changed-worker','unknown-field']:
        folder=root/mode;folder.mkdir();owned=folder/'cutbolt-transcribe-fixture';owned.mkdir()
        (owned/'transcribe_supervisor.py').write_bytes(supervisor)
        (owned/'transcribe_worker.py').write_text(HELPER,encoding='utf-8')
        (owned/'analysis.wav').write_bytes(b'original owned cleanup sentinel')
        sentinel=folder/'preserve.bin';sentinel.write_bytes(b'not owned by the supervisor')
        worker_hash=hashlib.sha256((owned/'transcribe_worker.py').read_bytes()).hexdigest()
        settings={'timeout_seconds':2 if mode=='timeout' else 10,'worker_sha256':worker_hash,
            'request':{'mode':mode,'record_root':linux(folder)}}
        if mode=='changed-worker':settings['worker_sha256']='0'*64
        if mode=='unknown-field':settings['extra']=True
        child=subprocess.Popen(['wsl','--distribution',runtime['distribution'],'--exec','unshare','-Urnpf','--kill-child=KILL','--mount-proc',
            runtime['python'],'-B',linux(owned/'transcribe_supervisor.py')],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        out=[];err=[];readers=[threading.Thread(target=drain,args=(child.stdout,out)),threading.Thread(target=drain,args=(child.stderr,err))]
        for reader in readers:reader.start()
        started=time.monotonic()
        try:
            child.stdin.write(json.dumps(settings).encode()+b'\n');child.stdin.flush()
            if mode in ('owner-eof','bridge-kill'):
                deadline=time.monotonic()+10
                while not (folder/'heartbeat').exists():
                    assert time.monotonic()<deadline,'Fixture child did not start'
                    time.sleep(.02)
                if mode=='owner-eof':child.stdin.close()
                else:child.kill()
            child.wait(timeout=18)
            child.stdin.close()
            for reader in readers:reader.join(5);assert not reader.is_alive(),'Owned pipe remained open'
            expected={'nonzero':'WORKER_FAILED','malformed':'SUPERVISOR_FAILED','oversized':'RESULT_LIMIT','stderr-limit':'RESULT_LIMIT',
                'timeout':'WORKER_TIMEOUT','owner-eof':'OWNER_GONE','changed-worker':'WORKER_CHANGED','unknown-field':'INVALID_REQUEST'}
            response=json.loads(out[0]) if out[0] else None
            if mode in expected:assert response['error']['code']==expected[mode],(mode,response,err)
            elif mode!='bridge-kill':assert response['ok'],(mode,response,err)
            deadline=time.monotonic()+5
            while owned.exists() and time.monotonic()<deadline:time.sleep(.02)
            cleaned=not owned.exists()
            if mode=='bridge-kill' and not cleaned:
                # Killing the WSL bridge can kill namespace init before Python's
                # cleanup runs. The OS still ends its descendants; no result was
                # published. Remove only this fixture's known, verified files.
                assert {p.name for p in owned.iterdir()}=={'analysis.wav','transcribe_worker.py','transcribe_supervisor.py'}
                assert (owned/'transcribe_supervisor.py').read_bytes()==supervisor
                for name in ['analysis.wav','transcribe_worker.py','transcribe_supervisor.py']:(owned/name).unlink()
                owned.rmdir()
            else:assert cleaned,('Known scratch was not removed',mode)
            assert sentinel.read_bytes()==b'not owned by the supervisor'
            if (folder/'heartbeat').exists():
                size=(folder/'heartbeat').stat().st_size;time.sleep(.15)
                assert (folder/'heartbeat').stat().st_size==size,('Detached child survived',mode)
            if (folder/'namespace.txt').exists():namespaces.add((folder/'namespace.txt').read_text())
            records.append({'mode':mode,'seconds':time.monotonic()-started,'response':response,'scratch_removed_by_supervisor':cleaned,
                'stderr':err[0].decode(errors='replace')})
        finally:
            if child.poll() is None:child.kill();child.wait(timeout=5)
            if not child.stdin.closed:child.stdin.close()
    no_namespaces(runtime,namespaces)
    report={'production_supervisor_sha256':hashlib.sha256(supervisor).hexdigest(),'records':records,
        'namespaces_checked':len(namespaces),'surviving_owned_processes':0,'unrelated_files_preserved':True}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    return report
