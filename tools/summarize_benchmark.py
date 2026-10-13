"""Read-only aggregation of frozen diagnostic trials; never modify attempts or delivery files."""
import argparse,hashlib,json,math,re,statistics
from pathlib import Path

def identity(path):
 data=path.read_bytes();return {'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
def stats(values):
 values=[float(v) for v in values if v is not None and math.isfinite(float(v))]
 return {'n':len(values),'minimum':min(values),'median':statistics.median(values),'maximum':max(values)} if values else None
def summarize(root):
 plan=json.loads((root/'plan.json').read_text());rows=[];groups={};missing=[];integrity={}
 engine=root/('cutbolt.exe' if (root/'cutbolt.exe').exists() else 'cutbolt')
 integrity['engine']=identity(engine)==plan['engine']
 integrity['protocol']=identity(root/'protocol.md')==plan['protocol']
 integrity['oracle']=identity(root/'harness/tests/footage_benchmark.py')==plan['oracle']
 integrity['runner']=identity(root/'harness/tools/benchmark_agents.py')==plan['runner']
 integrity['input_manifest']=identity(Path(plan['inputs'])/'manifest.json')==plan['input_manifest']
 manifest=json.loads((Path(plan['inputs'])/'manifest.json').read_text())
 for name,record in manifest['sources'].items():
  integrity['source:'+name]=identity(Path(plan['inputs'])/name)=={k:record[k] for k in ('bytes','sha256')}
 names=[t['name'] for t in plan['trials']]
 if len(set(names))!=len(names):raise ValueError('Trial names must be unique')
 for trial in plan['trials']:
  case=root/'trials'/trial['name'];f=case/'measurement.json'
  frozen=identity(case/'prompt.txt')==trial['prompt']
  frozen=frozen and all(identity(case/'plugin/skills'/name/'SKILL.md')==v for name,v in trial.get('skills',{}).items())
  integrity[trial['name']]=frozen
  if not f.is_file():missing.append({'name':trial['name'],'started':(case/'client-start.json').exists()});continue
  r=json.loads(f.read_text());a=r['audit'];output=case/'workspace/delivery.mp4'
  if r['trial']!=trial:raise ValueError('Measurement does not belong to the frozen trial: '+trial['name'])
  same_output=output.is_file() and identity(output)==a.get('output_identity')
  # The frozen pixel/clock auditor does not validate the requested encoder control.
  # Report a separate bitstream check; AAC target still needs trace/receipt review.
  sei=re.findall(rb'x264 - core [\x20-\x7e]{0,3000}',output.read_bytes()) if output.is_file() else []
  crf18=any(re.search(rb'\bcrf=18\.0\b',s) for s in sei)
  aac=[]
  if trial['arm']=='cutbolt':
   for wire in (case/'wire').glob('*.jsonl'):
    for line in wire.read_text().splitlines():
     v=json.loads(line)['message'].get('result',{}).get('structuredContent',{}).get('result',{})
     if isinstance(v,dict) and v.get('status')=='completed':
      outcome=v.get('result',{})
      if outcome.get('profile')=='h264_aac' and outcome.get('sha256')==a.get('output_identity',{}).get('sha256'):
       aac.append(outcome.get('audio',{}).get('bitrate'))
  failures=[k for k in ('source_preservation','output_exists','frame_counters','inset_counters','profile','color','audio_clock') if a.get(k) is False]
  if a.get('frame_count')!=a.get('expected_frames'):failures.append('frame_count')
  if a.get('audio_correlation') is None or a['audio_correlation']<.995:failures.append('audio_correlation')
  if a.get('audio_rms_error') is None or a['audio_rms_error']>250:failures.append('audio_rms_error')
  if a.get('frame_psnr_min') is None or a['frame_psnr_min']<26:failures.append('frame_psnr_min')
  row={'name':trial['name'],'model':trial['model'],'task':trial['task'],'arm':trial['arm'],'repeat':trial['repeat'],
   'success':r['success'],'client_result':r['client_result'],'resolved_model':r['resolved_model'],
   'isolated':r['isolation']['passed'],'frozen_inputs':frozen,'output_unchanged':same_output,
   'calls':r['all_tool_calls'],'skill_calls':r['skill_calls'],'schema_lookups':r['schema_lookups'],
   'failed_tool_results':r['failed_tool_results'],'mcp_failed_calls':r['mcp_failed_calls'],
   'peak_context':r['peak_input_context'],'cumulative_input':r['cumulative_input'],'output_tokens':r['output_tokens'],
   'reported_usd':r['reported_cost_usd'],'seconds':r['exit']['wall_seconds'],
   'busy_fraction':r['observations']['busy_fraction_over_50_percent'],'first_output_file_seconds':r['observations']['first_output_file_seconds'],
   'audit':a,'failure_checks':failures,'x264_crf18_sei':crf18,'aac_target_receipts':aac or None,
   'aac_target_review':'matched_output_receipt' if aac and all(v==320000 for v in aac) else 'pending_trace_review',
   'measurement_identity':identity(f)}
  rows.append(row);groups.setdefault((trial['model'],trial['task'],trial['arm']),[]).append(row)
 summary=[]
 for (model,task,arm),entries in sorted(groups.items()):
  summary.append({'model':model,'task':task,'arm':arm,'completed':len(entries),'successes':sum(r['success'] for r in entries),
   'all_completed_metrics':{key:stats(r[key] for r in entries) for key in ('calls','peak_context','cumulative_input','reported_usd','seconds','busy_fraction','schema_lookups')},
   'successful_output_metrics':{key:stats(r['audit'].get(key) for r in entries if r['success']) for key in ('frame_psnr_min','frame_psnr_median','audio_rms_error','video_bit_rate','audio_bit_rate')}})
 return {'planned':len(plan['trials']),'completed':len(rows),'matrix_complete':not missing,'missing':missing,'integrity':integrity,
  'client_version':plan['client_version'],'resolved_models':sorted({r['resolved_model'] for r in rows if r['resolved_model'] is not None}),
  'unresolved_model_trials':[r['name'] for r in rows if r['resolved_model'] is None],
  'groups':summary,'trials':rows,'all_material_frozen':all(integrity.values()),
  'limits':['Descriptive diagnostic results only; limited repeats do not establish statistical significance.',
   'Machine load overlaps verification; speed observations remain provisional.',
   'Quality scores use the declared FFmpeg RGB reference; requested CRF is not matched bitrate.',
   'AAC target controls require separate receipt/trace review; actual AAC bitrate need not equal its target.',
   'No release gate or implementation scope is changed.']}
if __name__=='__main__':
 p=argparse.ArgumentParser(description=__doc__)
 p.add_argument('root',type=Path);p.add_argument('output',type=Path)
 p.add_argument('--allow-partial',action='store_true',help='Write an explicitly incomplete diagnostic snapshot')
 a=p.parse_args();result=summarize(a.root)
 if not result['matrix_complete'] and not a.allow_partial:raise SystemExit('Matrix incomplete; retain all pending/failed attempts and wait, or use --allow-partial')
 # Never overwrite another report or a trial artifact with an aggregation.
 with a.output.open('x',encoding='utf-8') as out:out.write(json.dumps(result,indent=2,allow_nan=False)+'\n')
 print({k:result[k] for k in ('planned','completed','matrix_complete','all_material_frozen')})
