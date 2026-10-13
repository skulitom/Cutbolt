"""Run a preregistered local footage pilot through a normally authenticated Claude Code client.

All profiles, traces and media are external. No provider API or credential value is logged.
The client owns model access; this script supplies bounded tasks, records observations and audits outputs.
"""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import time

import psutil

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT/'tests'))
from footage_benchmark import TASKS, audit, identity, prompt  # noqa: E402

SKILLS = ('cutbolt','recut-talking-head','captions','screen-tutorial','deliver-and-review','interview-multicam')


def save(path, value):
    path.write_text(json.dumps(value,indent=2)+'\n',encoding='utf-8')


def relay(engine, workspace, directory):
    directory.mkdir(parents=True,exist_ok=True)
    process=subprocess.Popen([str(engine),'--workspace',str(workspace),'mcp'],stdin=subprocess.PIPE,stdout=subprocess.PIPE)
    lock=threading.Lock()
    with (directory/f'wire-{os.getpid()}.jsonl').open('x',encoding='utf-8') as log:
        def record(direction,line):
            with lock:
                log.write(json.dumps({'direction':direction,'bytes':len(line),'time':time.monotonic(),'message':json.loads(line)})+'\n');log.flush()
        def send():
            try:
                for line in sys.stdin.buffer:
                    record('sent',line);process.stdin.write(line);process.stdin.flush()
            finally:process.stdin.close()
        sender=threading.Thread(target=send,daemon=True);sender.start()
        for line in process.stdout:
            record('received',line);sys.stdout.buffer.write(line);sys.stdout.buffer.flush()
        process.wait()


def prepare(args):
    base=args.root.resolve()
    assert base!=ROOT and ROOT not in base.parents, 'Trials must stay outside the repository'
    base.mkdir(parents=True,exist_ok=True)
    if (base/'plan.json').exists():raise FileExistsError(base/'plan.json')
    engine=base/'cutbolt.exe' if os.name=='nt' else base/'cutbolt'
    if engine.exists():
        assert identity(engine)==identity(args.engine), 'Existing pinned engine differs'
    else:shutil.copy2(args.engine,engine)
    # Keep the exact harness, oracle and skills immutable through this matrix.
    harness=base/'harness';harness.mkdir()
    (harness/'tools').mkdir();(harness/'tests').mkdir()
    shutil.copy2(__file__,harness/'tools/benchmark_agents.py')
    shutil.copy2(ROOT/'tests/footage_benchmark.py',harness/'tests/footage_benchmark.py')
    trials=[]
    for repeat in range(args.repeats):
        for model_index,model in enumerate(args.models):
            for task_index,task in enumerate(TASKS):
                arms=('cutbolt','ffmpeg') if (repeat+model_index+task_index)%2==0 else ('ffmpeg','cutbolt')
                for arm in arms:
                    name=f'{model}-{repeat+1}-{task}-{arm}'
                    case=base/'trials'/name;work=case/'workspace';work.mkdir(parents=True)
                    for file in ('a.mov','b.mov'):shutil.copy2(args.inputs/file,work/file)
                    text=prompt(task,arm)
                    (case/'prompt.txt').write_text(text+'\n',encoding='utf-8')
                    trial={'name':name,'model':model,'repeat':repeat+1,'task':task,'arm':arm,'prompt':identity(case/'prompt.txt')}
                    if arm=='cutbolt':
                        plugin=case/'plugin'
                        shutil.copytree(ROOT/'skills',plugin/'skills')
                        shutil.copytree(ROOT/'.claude-plugin',plugin/'.claude-plugin')
                        manifest=plugin/'.claude-plugin/plugin.json';value=json.loads(manifest.read_text())
                        value['mcpServers']['cutbolt']={'command':sys.executable,'args':[str(harness/'tools/benchmark_agents.py'),'relay',str(engine),'${CLAUDE_PROJECT_DIR}',str(case/'wire')]}
                        save(manifest,value)
                        trial['skills']={p.parent.name:identity(p) for p in (plugin/'skills').glob('*/SKILL.md')}
                    trials.append(trial)
    plan={'version':1,'created_utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'engine':identity(engine),
          'engine_version':subprocess.check_output([str(engine),'--version'],text=True).strip(),
          'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip(),
          'source_modified':bool(subprocess.check_output(['git','status','--porcelain'],cwd=ROOT,text=True).strip()),
          'protocol':identity(ROOT/'docs/FOOTAGE_BENCHMARK.md'),'oracle':identity(ROOT/'tests/footage_benchmark.py'),
          'runner':identity(__file__),'inputs':str(args.inputs.resolve()),'input_manifest':identity(args.inputs/'manifest.json'),
          'client':str(args.client),'client_version':subprocess.check_output([str(args.client),'--version'],text=True).strip(),
          'models':args.models,'effort':args.effort,'repeats':args.repeats,'timeout_seconds':600,'max_reported_usd':5,
          'trials':trials}
    shutil.copy2(ROOT/'docs/FOOTAGE_BENCHMARK.md',base/'protocol.md')
    save(base/'plan.json',plan)
    print(f'Prepared {len(trials)} fresh trials at {base}',flush=True)


def measure(case, plan, trial, exit_state):
    events=[]
    for line in (case/'stream.jsonl').read_text(encoding='utf-8').splitlines():
        try:events.append(json.loads(line))
        except ValueError:pass
    init=next((e for e in events if e.get('type')=='system' and e.get('subtype')=='init'),{})
    final=next((e for e in reversed(events) if e.get('type')=='result'),{})
    calls={};failures=set();peak=[]
    for event in events:
        message=event.get('message',{})
        if event.get('type')=='assistant':
            usage=message.get('usage',{})
            if all(k in usage for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens')):
                peak.append(sum(usage[k] for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens')))
            for item in message.get('content',[]):
                if item.get('type')=='tool_use':calls[item['id']]={'name':item['name'],'input':item.get('input',{})}
        if event.get('type')=='user':
            for item in message.get('content',[]):
                if isinstance(item,dict) and item.get('type')=='tool_result' and item.get('is_error'):failures.add(item['tool_use_id'])
    tools=init.get('tools',[]);skills=init.get('skills',[])
    if trial['arm']=='cutbolt':
        isolated=len(tools)==19 and all(t=='Skill' or t.startswith('mcp__plugin_cutbolt_cutbolt__') for t in tools) and len(skills)==6 and all(s.startswith('cutbolt:') for s in skills)
    else:isolated=set(tools)=={'Bash','Read','Write'} and not skills and not init.get('mcp_servers')
    def failed_job(value):
        if isinstance(value,dict):
            return value.get('status') in ('failed','cancelled','interrupted') or any(failed_job(v) for v in value.values())
        return isinstance(value,list) and any(failed_job(v) for v in value)
    wire_calls=[]
    for wire in sorted((case/'wire').glob('*.jsonl')):
        rows=[json.loads(line) for line in wire.read_text().splitlines()]
        replies={r['message']['id']:r for r in rows if r['direction']=='received' and 'id' in r['message']}
        for row in rows:
            request=row['message']
            if row['direction']!='sent' or request.get('method')!='tools/call':continue
            reply=replies.get(request['id']);result=(reply or {}).get('message',{}).get('result',{})
            envelope=result.get('structuredContent',{});outcome=envelope.get('result',{})
            tool=request['params']['name'];arguments=request['params']['arguments']
            command=arguments.get('command') if tool=='cutbolt_run' else tool.removeprefix('cutbolt_').replace('_','.')
            failed=bool(not reply or result.get('isError') or envelope.get('ok') is False or failed_job(outcome))
            wire_calls.append({'command':command,'failed':failed,'seconds':reply['time']-row['time'] if reply else None,
                               'sent_bytes':row['bytes'],'received_bytes':reply['bytes'] if reply else None})
    usage=final.get('usage',{})
    observed=json.loads((case/'observations.json').read_text())
    report={'trial':trial,'requested_model':trial['model'],'resolved_model':init.get('model'),'client_version':init.get('claude_code_version'),
            'isolation':{'passed':isolated,'tools':tools,'skills':skills,'mcp_servers':init.get('mcp_servers')},
            'all_tool_calls':len(calls),'skill_calls':sum(c['name']=='Skill' for c in calls.values()),
            'failed_tool_results':len(failures),'mcp_calls':wire_calls,'mcp_failed_calls':sum(c['failed'] for c in wire_calls),
            'schema_lookups':sum(c['command']=='schema' for c in wire_calls),'peak_input_context':max(peak) if peak else None,
            'cumulative_input':sum(usage[k] for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens')) if all(k in usage for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens')) else None,
            'output_tokens':usage.get('output_tokens'),'usage_as_reported':usage,'reported_cost_usd':final.get('total_cost_usd'),
            'client_result':final.get('subtype'),'client_is_error':final.get('is_error'),'permission_denials':final.get('permission_denials'),
            'exit':exit_state,'observations':observed,'first_useful_preview_seconds':None,'render_seconds':None,
            'audit':audit(Path(plan['inputs']),case/'workspace',trial['task'])}
    report['success']=bool(isolated and exit_state['exit_code']==0 and not exit_state['timed_out'] and not final.get('is_error') and report['audit']['passed'])
    save(case/'measurement.json',report)
    return report


def run(args):
    base=args.root.resolve();plan=json.loads((base/'plan.json').read_text())
    # Resume only unstarted trials. Existing model attempts are immutable, including failures.
    for trial in plan['trials']:
        case=base/'trials'/trial['name']
        if (case/'client-start.json').exists():continue
        profile=Path(tempfile.mkdtemp(prefix='cutbolt-benchmark-profile-'))
        credential=profile/'.credentials.json'
        env={**os.environ,'CLAUDE_CONFIG_DIR':str(profile),'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC':'1',
             'ENABLE_CLAUDEAI_MCP_SERVERS':'false','DISABLE_DOCTOR_COMMAND':'1'}
        for variable in ('CUTBOLT_WORKSPACE','CUTBOLT_MCP_TOOLS','CUTBOLT_CONFORM_CACHE','CUTBOLT_CONFORM_CACHE_BYTES','CUTBOLT_INSPECTION_CACHE'):
            env.pop(variable,None)
        client=plan['client'];work=case/'workspace'
        try:
            # Auth is the ordinary client's existing user-authorized source; never print contents.
            source=json.loads(args.credential_source.read_text())
            if source['claudeAiOauth']['expiresAt']/1000<time.time()+720:
                raise RuntimeError('Normal client authentication needs renewal before another bounded trial')
            shutil.copyfile(args.credential_source,credential)
            if trial['arm']=='cutbolt':
                for name,command in [('marketplace',['plugin','marketplace','add',str(case/'plugin')]),('install',['plugin','install','cutbolt@cutbolt-local','--scope','user','--json'])]:
                    with (case/(name+'.log')).open('w',encoding='utf-8') as out:
                        subprocess.run([client,*command],cwd=work,env=env,stdin=subprocess.DEVNULL,stdout=out,stderr=subprocess.STDOUT,check=True,timeout=120)
            command=[client,'--print','--verbose','--output-format','stream-json','--model',trial['model'],'--effort',plan['effort'],
                     '--settings',json.dumps({'disableClaudeAiConnectors':True,'disableBundledSkills':True}),
                     '--permission-mode','dontAsk','--setting-sources','user','--max-budget-usd',str(plan['max_reported_usd'])]
            if trial['arm']=='cutbolt':command += ['--tools','Skill','--allowedTools',*[f'Skill(cutbolt:{s})' for s in SKILLS],'mcp__plugin_cutbolt_cutbolt__*','mcp__plugin:cutbolt:cutbolt__*']
            else:command += ['--tools','Bash,Read,Write','--allowedTools','Bash','Read','Write','--disable-slash-commands','--strict-mcp-config','--mcp-config','{"mcpServers":{}}']
            save(case/'client-command.json',command)
            save(case/'client-start.json',{'utc':time.strftime('%Y-%m-%dT%H:%M:%SZ',time.gmtime()),'requested_model':trial['model']})
            print('Starting '+trial['name'],flush=True)
            began=time.monotonic();timed_out=False;stop=threading.Event();samples=[];first_file=[]
            def observe():
                while not stop.wait(.5):
                    samples.append(psutil.cpu_percent())
                    if not first_file and (work/'delivery.mp4').exists():first_file.append(time.monotonic()-began)
            observer=threading.Thread(target=observe,daemon=True);observer.start()
            with (case/'stream.jsonl').open('w',encoding='utf-8') as out,(case/'client-stderr.log').open('w',encoding='utf-8') as err:
                process=subprocess.Popen(command,cwd=work,env=env,stdin=subprocess.PIPE,stdout=out,stderr=err,text=True,encoding='utf-8',
                                         **({'creationflags':subprocess.CREATE_NO_WINDOW} if os.name=='nt' else {}))
                try:process.communicate((case/'prompt.txt').read_text(),timeout=plan['timeout_seconds'])
                except subprocess.TimeoutExpired:
                    timed_out=True
                    for child in psutil.Process(process.pid).children(recursive=True):
                        try:child.terminate()
                        except psutil.NoSuchProcess:pass
                    process.terminate();process.wait(timeout=15)
            stop.set();observer.join()
            state={'exit_code':process.returncode,'timed_out':timed_out,'wall_seconds':time.monotonic()-began}
            save(case/'client-exit.json',state)
            save(case/'observations.json',{'machine_cpu_percent':samples,'busy_fraction_over_50_percent':sum(v>50 for v in samples)/len(samples) if samples else None,
                                          'first_output_file_seconds':first_file[0] if first_file else None,'first_output_file_is_not_a_verified_preview':True})
            report=measure(case,plan,trial,state)
            print(json.dumps({'trial':trial['name'],'success':report['success'],'calls':report['all_tool_calls'],'peak_context':report['peak_input_context'],'seconds':state['wall_seconds']}),flush=True)
        except Exception as error:
            save(case/'coordinator-error.json',{'type':type(error).__name__,'message':str(error)})
            raise
        finally:
            credential.unlink(missing_ok=True)
    print('All scheduled trials have a recorded start; inspect measurements and incomplete attempts.',flush=True)


if __name__=='__main__':
    parser=argparse.ArgumentParser();subs=parser.add_subparsers(dest='action',required=True)
    p=subs.add_parser('prepare');p.add_argument('--root',type=Path,required=True);p.add_argument('--inputs',type=Path,required=True)
    p.add_argument('--engine',type=Path,required=True);p.add_argument('--client',type=Path,required=True)
    p.add_argument('--models',nargs='+',default=['opus','sonnet']);p.add_argument('--repeats',type=int,default=3);p.add_argument('--effort',default='high')
    p=subs.add_parser('run');p.add_argument('--root',type=Path,required=True);p.add_argument('--credential-source',type=Path,required=True)
    p=subs.add_parser('relay');p.add_argument('engine',type=Path);p.add_argument('workspace',type=Path);p.add_argument('directory',type=Path)
    args=parser.parse_args()
    if args.action=='prepare':prepare(args)
    elif args.action=='run':run(args)
    else:relay(args.engine,args.workspace,args.directory)
