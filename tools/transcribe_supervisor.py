"""Original owner-lifetime guard for the fixed local speech worker.

Run as PID 1 in a new Linux PID/network namespace. The native owner sends one
bounded JSON line and keeps stdin open. Closing that pipe cancels the analysis.
Namespace-init exit also terminates descendants that detached their process group.
"""
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import time


def error(code, message):
    return json.dumps({'ok':False,'error':{'code':code,'message':message}}).encode()+b'\n',1


# Files this engine invocation can make: its worker, this supervisor and up to 16 analyses.
OWNED = ['analysis.wav','transcribe_worker.py','transcribe_supervisor.py']+[f'analysis-{n}.wav' for n in range(16)]
RESULT_BYTES = 16*1048576


def cleanup(root):
    # Only the files made by this engine invocation; never recursive.
    if root.name.startswith('cutbolt-transcribe-') and root == Path(__file__).resolve().parent:
        for name in OWNED:
            try: (root/name).unlink()
            except OSError: pass
        try: root.rmdir()
        except OSError: pass


def run(root):
    if os.getpid() != 1:
        return error('ISOLATION_REQUIRED','Speech supervisor requires its own PID namespace')
    def stop(signum, frame):
        raise InterruptedError('Speech supervisor interrupted')
    signal.signal(signal.SIGTERM,stop); signal.signal(signal.SIGINT,stop)
    selector = selectors.DefaultSelector()
    selector.register(0,selectors.EVENT_READ,'owner')
    deadline = time.monotonic()+10
    data = bytearray()
    child = None
    try:
        while b'\n' not in data:
            if time.monotonic() >= deadline: return error('REQUEST_TIMEOUT','Initial request timed out')
            if not selector.select(.05): continue
            chunk = os.read(0,65537-len(data))
            if not chunk: return error('OWNER_GONE','Owner closed input before request')
            data.extend(chunk)
            if len(data) > 65536: return error('REQUEST_LIMIT','Speech request exceeds 64 KiB')
        if data.count(b'\n') != 1 or not data.endswith(b'\n'):
            return error('INVALID_REQUEST','Expected exactly one JSON line')
        settings = json.loads(data)
        if type(settings) is not dict or set(settings) != {'timeout_seconds','worker_sha256','request'}:
            return error('INVALID_REQUEST','Missing or unknown supervisor fields')
        timeout = settings['timeout_seconds']
        if type(timeout) is not int or not 1 <= timeout <= 600 or type(settings['request']) is not dict:
            return error('INVALID_REQUEST','Invalid supervisor deadline or worker request')
        worker = root/'transcribe_worker.py'
        if hashlib.sha256(worker.read_bytes()).hexdigest() != settings['worker_sha256']:
            return error('WORKER_CHANGED','Owned speech worker identity changed')
        child = subprocess.Popen([sys.executable,'-B',str(worker)],stdin=subprocess.PIPE,stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,close_fds=True,start_new_session=True)
        child.stdin.write(json.dumps(settings['request'],ensure_ascii=False).encode()); child.stdin.close()
        for name,stream in [('result',child.stdout),('errors',child.stderr)]:
            os.set_blocking(stream.fileno(),False); selector.register(stream,selectors.EVENT_READ,name)
        chunks = {'result':bytearray(),'errors':bytearray()}
        ended = set(); deadline = time.monotonic()+timeout
        while len(ended) < 2 or child.poll() is None:
            if time.monotonic() >= deadline: return error('WORKER_TIMEOUT','Speech analysis deadline expired')
            for key,_ in selector.select(.05):
                chunk = os.read(key.fd,16384)
                if key.data == 'owner': return error('OWNER_GONE','Owner disconnected or sent unexpected input')
                if not chunk:
                    selector.unregister(key.fileobj); ended.add(key.data); continue
                chunks[key.data].extend(chunk)
                if len(chunks[key.data]) > (RESULT_BYTES if key.data == 'result' else 65536):
                    return error('RESULT_LIMIT','Speech worker exceeded its output bound')
        value = json.loads(chunks['result'])
        if type(value) is not dict or type(value.get('ok')) is not bool:
            return error('INVALID_RESULT','Worker must return one structured result')
        if child.returncode != (0 if value['ok'] else 1):
            return error('WORKER_FAILED','Worker process/result status mismatch')
        return bytes(chunks['result']), child.returncode
    finally:
        if child is not None and child.poll() is None:
            try: os.killpg(child.pid,signal.SIGKILL)
            except ProcessLookupError: pass
            try: child.wait(timeout=1)
            except subprocess.TimeoutExpired: pass
        selector.close()


if __name__ == '__main__':
    root = Path(__file__).resolve().parent
    try:
        output,status = run(root)
    except Exception as exc:
        output,status = error('SUPERVISOR_FAILED',str(exc)[:512])
    cleanup(root)
    try: os.write(1,output)
    finally: os._exit(status)
