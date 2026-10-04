"""Original transcript correction/cut integration; no recognition accuracy credit.

Synthetic coded pixels/PCM expose every exact deletion independently. Authored
word anchors exercise the editing contract; actual recognizer acceptance is separate.
"""
import argparse
from array import array
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from tracks import time, seconds, edit, placement, track

ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / 'target/debug/cutbolt.exe'
W, H, N = 24, 16, 100


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    source, output, store = [root / name for name in ('source', 'output', 'store')]
    for path in (source, output, store):
        path.mkdir()
    passed, cases = [], []
    rejected = frames = samples = previews = 0

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], capture_output=True, check=True, timeout=120).stdout

    def call(request, error=None):
        nonlocal rejected
        result = subprocess.run([str(EXE)], input=json.dumps(request).encode(), capture_output=True, timeout=120)
        data = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and data['error']['code'] == error, (error, data)
            rejected += 1
            return data
        if result.returncode:
            (root / 'failed-request.json').write_text(json.dumps(request, indent=2), encoding='utf-8')
        assert result.returncode == 0 and data['ok'], data
        return data['result']

    pictures = [bytes(v for y in range(H) for x in range(W) for v in ((n*13+x*7)%256, (n*3+y*11)%256, (n*19+x+y)%256)) for n in range(N)]
    sound = array('h', (v for n in range(N*1920) for v in ((n*79)%60000-30000, (n*31)%58000-29000)))
    (source/'pixels.rgb').write_bytes(b''.join(pictures))
    (source/'audio.pcm').write_bytes(sound.tobytes())
    movie = source/'voice.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(source/'pixels.rgb'),
        '-f','s16le','-ar','48000','-ac','2','-i',str(source/'audio.pcm'),'-map','0:v','-map','1:a','-c:v','ffv1','-level','3',
        '-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le','-colorspace','rgb','-color_range','pc',
        '-color_primaries','bt709','-color_trc','iec61966-2-1',str(movie)])
    digest = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
    identity = {'sha256':digest(movie),'bytes':movie.stat().st_size}
    original = {p.name:digest(p) for p in source.iterdir()}
    def preserved():
        assert original == {p.name:digest(p) for p in source.iterdir()}
    asset = {'id':'voice','path':str(movie),'duration':time(4),'identity':identity}
    project = call({'command':'project.create','id':'text-edits','width':W,'height':H,'frame_rate':time(25)})
    def apply(project, operations, error=None):
        return call({'command':'timeline.apply','project':project,'expected_revision':project['revision'],'operations':operations},error)
    project = apply(project,[{'op':'media.add','asset':asset},edit('create',duration=time(4)),
        edit('add',track=track('v','video')),edit('add',track=track('a','audio')),
        edit('place',track_id='v',clip=placement('video','voice',10,75,5),collision='reject'),
        edit('place',track_id='a',clip=placement('audio','voice',10*1920+7,75*1920,5*1920+7,48000),collision='reject'),
        edit('link',id='av',clip_ids=['video','audio'])])
    document = {'schema_version':1,'id':'words','revision':0,'parent_fingerprint':None,
        'source':{'path':'voice.mkv','identity':identity,'duration':time(4)},'range_start':time(0),'range_duration':time(4),'language':'en',
        'recognition':{'profile':'synthetic-contract-fixture','model':{'sha256':'a'*64,'bytes':1},'worker_sha256':'b'*64,
            'analysis_sha256':'c'*64,'versions':{'original-fixture':'1'}},
        'words':[{'id':f'w{i}','text':word,'start':time(10+10*i,25),'end':time(14+10*i,25),'origin':'estimated','probability_milli':900}
            for i,word in enumerate(['red,','square','κύκλος','blue','circle.'])]}
    def inspect(doc, error=None):
        return call({'command':'transcript.inspect','document':doc,'input_root':str(source)},error)
    fingerprint = inspect(document)['fingerprint']
    spec = {'clip_id':'video','track_ids':['v'],'ranges':[{'first_id':'w1','last_id':'w1'},{'first_id':'w3','last_id':'w3'}],
        'clock':'video','rounding':'strict','estimates':'reject','collateral':'reject','links':'include','end_policy':'resize','transitions':'reject_affected'}
    def plan(p=project, doc=document, value=spec, error=None, expected=None):
        return call({'command':'transcript.plan','project':p,'document':doc,'expected_revision':p['revision'],
            'expected_document_fingerprint':expected or inspect(doc)['fingerprint'],'spec':value,'input_root':str(source)},error)
    plan(error='ESTIMATED_BOUNDARY')
    estimates = copy.deepcopy(spec); estimates['estimates']='allow_reported'
    estimated_plan = plan(value=estimates)
    assert estimated_plan['estimated_word_ids']==['w1','w3']
    def correct(doc, edits, error=None, expected=None):
        return call({'command':'transcript.correct','document':doc,'expected_fingerprint':expected or inspect(doc)['fingerprint'],
            'edits':edits,'input_root':str(source)},error)
    correction=[{'op':'replace','word':{'id':'w1','text':'square!','start':time(21,25),'end':time(26,25)}},
                {'op':'replace','word':{'id':'w3','text':'blue','start':time(40,25),'end':time(44,25)}}]
    corrected = correct(document,correction)
    doc = corrected['document']
    assert doc['revision']==1 and doc['parent_fingerprint']==fingerprint
    assert doc['source']==document['source'] and doc['recognition']==document['recognition']
    assert doc['words'][1]['origin']=='corrected' and doc['words'][1]['probability_milli'] is None
    assert document['revision']==0 and inspect(document)['fingerprint']==fingerprint
    corrected_plan = plan(doc=doc)
    assert corrected_plan['estimated_word_ids']==[] and len(corrected_plan['merged_cuts'])==2
    assert [c['start'] for c in corrected_plan['merged_cuts']]==[time(26,25),time(45,25)]
    assert corrected_plan['word_projection'][4]['fragments'][0]['timeline_start']==time(46,25)
    assert corrected_plan['result_duration']==time(91,25)
    passed.append('transcript.exact_source_corrections_and_projection')

    # Independent original timeline bytes, then literal deletion of stated intervals.
    base_rgb = bytes(10*W*H*3)+b''.join(pictures[5:80])+bytes(15*W*H*3)
    base_pcm = bytes((10*1920+7)*4)+sound[(5*1920+7)*2:(80*1920+7)*2].tobytes()+bytes((15*1920-7)*4)
    def reference(cuts, keep=False):
        rgb,pcm=base_rgb,base_pcm
        for a,b in sorted(cuts,reverse=True):
            rgb=rgb[:a*W*H*3]+rgb[b*W*H*3:]
            pcm=pcm[:a*1920*4]+pcm[b*1920*4:]
        if keep:
            deleted=sum(b-a for a,b in cuts)
            rgb+=bytes(deleted*W*H*3);pcm+=bytes(deleted*1920*4)
        return rgb,pcm
    def compare(path,expected,label):
        nonlocal frames,samples
        rgb,pcm=expected
        assert ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==rgb,(label,'pixels')
        assert ff(['-i',str(path),'-vn','-f','s16le','-'])==pcm,(label,'samples')
        frames+=len(rgb)//(W*H*3); samples+=len(pcm)//4; cases.append(label);preserved()
    def render(p,expected,label):
        path=output/(label+'.mkv')
        call({'command':'render.run','project':p,'input_root':str(source),'output_root':str(output),'output':str(path)})
        compare(path,expected,label)
    render(project,(base_rgb,base_pcm),'original')
    edited=apply(project,[corrected_plan['operation']])
    expected=reference([(26,31),(45,49)])
    render(edited,expected,'corrected-word-cuts')
    render(apply(project,[estimated_plan['operation']]),reference([(25,29),(45,49)]),'explicit-estimated-cuts')
    keep=copy.deepcopy(spec);keep['end_policy']='keep'
    render(apply(project,[plan(doc=doc,value=keep)['operation']]),reference([(26,31),(45,49)],True),'fixed-end-cuts')
    merge=copy.deepcopy(estimates);merge['ranges']=[{'first_id':'w0','last_id':'w1'},{'first_id':'w1','last_id':'w2'}]
    merged=plan(value=merge);assert len(merged['merged_cuts'])==1
    render(apply(project,[merged['operation']]),reference([(15,39)]),'overlapping-selections')
    passed.append('transcript.linked_rendered_word_cuts')

    client=Client(EXE)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==63
        for command in ['inspect','correct','plan']:
            item=next(t for t in catalog if t['name']=='cutbolt_transcript_'+command)
            assert item['annotations']['readOnlyHint'] and not item['annotations']['openWorldHint']
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        common={'store_root':str(store),'project_id':project['id']}
        saved=client.call('session.get',**common)
        planned=client.call('transcript.plan',project=saved,document=doc,expected_revision=0,
            expected_document_fingerprint=corrected['fingerprint'],spec=spec,input_root=str(source))
        assert client.call('transcript.inspect',document=doc,input_root=str(source))==inspect(doc)
        assert client.call('transcript.correct',document=document,expected_fingerprint=fingerprint,edits=correction,input_root=str(source))==corrected
        fields={**common,'expected_revision':0,'request_id':'word-cuts','operations':[planned['operation']]}
        schema=next(t['inputSchema'] for t in catalog if t['name']=='cutbolt_session_apply')
        Draft202012Validator(schema).validate(fields)
        preview=client.call('session.preview',**{k:v for k,v in fields.items() if k!='request_id'})
        assert len(preview['clips'])==6 and preview['duration_after']==time(91,25)
        assert client.call('session.get',**common)==saved
        receipt=client.call('session.apply',**fields)
        assert client.call('session.apply',**fields)==receipt
        changed=client.call('session.get',**common)
        render(changed,expected,'saved-word-cuts')
        history=client.call('session.history',**common)
        client.call('session.apply','TEXT_PLAN_STALE',**common,expected_revision=1,request_id='stale',operations=[planned['operation']])
        assert client.call('session.history',**common)==history
        client.call('session.undo',**common,expected_revision=1,request_id='undo')
        assert client.call('session.get',**common)['tracks']==saved['tracks']
        client.call('session.restore',**common,expected_revision=2,request_id='restore',target_revision=1)
        restored=client.call('session.get',**common)
        render(restored,expected,'restored-word-cuts')
        for n in [0,25,26,40,41,46,80,90]:
            path=output/f'preview-{n}.png'
            client.call('preview.frame',project=restored,input_root=str(source),output_root=str(output),output=str(path),time=time(n,25))
            with Image.open(path) as image:assert image.tobytes()==expected[0][n*W*H*3:(n+1)*W*H*3]
            previews+=1
        path=output/'range.mkv'
        call({'command':'preview.range','project':restored,'input_root':str(source),'output_root':str(output),'output':str(path),'start':time(23,25),'duration':time(25,25)})
        compare(path,(expected[0][23*W*H*3:48*W*H*3],expected[1][23*1920*4:48*1920*4]),'edited-range')
        passed.append('transcript.saved_mcp_retry_undo_preview')
    finally:
        client.close()

    # Revision, source, clipping, policies and malformed records fail explicitly.
    correct(document,correction,'TRANSCRIPT_CONFLICT','f'*64)
    plan(doc=doc,expected=fingerprint,error='TRANSCRIPT_CONFLICT')
    stale=copy.deepcopy(corrected_plan['operation']);stale['document']['words'][0]['text']='changed'
    apply(project,[stale],'TRANSCRIPT_CONFLICT')
    apply(project,[corrected_plan['operation'],corrected_plan['operation']],'TEXT_PLAN_STALE')
    assert inspect(doc)['fingerprint']==corrected['fingerprint']
    locked=apply(project,[edit('state',track_id='a',locked=True,enabled=False)])
    plan(p=locked,doc=doc,error='TRACK_LOCKED')
    partial=copy.deepcopy(spec);partial['links']='reject_partial'
    plan(doc=doc,value=partial,error='LINKED_SELECTION')
    for mutation,code in [({'track_ids':['a']},'INVALID_TRANSCRIPT'),({'ranges':[]},'INVALID_TRANSCRIPT'),
        ({'ranges':[{'first_id':'missing','last_id':'w2'}]},'MISSING_WORD'),
        ({'ranges':[{'first_id':'w2','last_id':'w1'}]},'INVALID_TRANSCRIPT')]:
        bad=copy.deepcopy(spec);bad.update(mutation);plan(doc=doc,value=bad,error=code)
    for key,value in [('schema_version',2),('revision',1),('range_duration',time(121)),('range_start',time(1,7)),('language','xx'),('secret',True)]:
        bad=copy.deepcopy(document);bad[key]=value
        inspect(bad,'INVALID_JSON' if key in ('language','secret') else 'INVALID_TRANSCRIPT' if key!='range_start' else 'INVALID_TRANSCRIPT')
    invalid_words=[{'id':'w1'},{'end':time(0)},{'end':time(5)},{'start':time(11,7)},
                   {'text':'two words'},{'text':'\u0000'},{'text':'!'}, {'origin':'corrected'}, {'probability_milli':1001}]
    for changes in invalid_words:
        bad=copy.deepcopy(document);bad['words'][0].update(changes)
        inspect(bad,'INVALID_TRANSCRIPT' if changes.get('start')!=time(11,7) else 'UNALIGNED_TIME')
    for edits,code in [([], 'INVALID_TRANSCRIPT'),([{'op':'remove','id':'absent'}],'MISSING_WORD'),
        ([{'op':'remove','id':'w1'},{'op':'insert','before_id':'w2','word':correction[0]['word']}],'DUPLICATE_ID'),
        ([correction[0],{'op':'replace','word':{'id':'w3','text':'late','start':time(3),'end':time(4)}}],'INVALID_TRANSCRIPT')]:
        correct(document,edits,code)
        assert inspect(document)['fingerprint']==fingerprint
    insertion={'op':'insert','before_id':'w2','word':{'id':'new','text':'νέο','start':time(27,25),'end':time(29,25)}}
    inserted=correct(doc,[insertion])['document'];assert inserted['words'][2]['id']=='new'
    removed=correct(inserted,[{'op':'remove','id':'new'}])['document']
    assert removed['words']==doc['words'] and removed['revision']==3
    # Exercise the advertised document/batch bounds with independent sample clocks.
    maximum=copy.deepcopy(document)
    maximum['words']=[{'id':f'm{i}','text':'x'*64,'start':time(i*2,48000),'end':time(i*2+1,48000),
        'origin':'corrected','probability_milli':None} for i in range(2048)]
    assert inspect(maximum)['word_count']==2048
    batch=[{'op':'replace','word':{k:maximum['words'][i][k] for k in ('id','text','start','end')}} for i in range(128)]
    assert correct(maximum,batch)['document']['revision']==1
    correct(maximum,batch+[batch[0]],'INVALID_TRANSCRIPT')
    bad=copy.deepcopy(maximum);bad['words'][-1]['text']+='x';inspect(bad,'INVALID_TRANSCRIPT')
    bad=copy.deepcopy(maximum);bad['words'].append({**bad['words'][-1],'id':'overflow','start':time(4096,48000),'end':time(4097,48000)})
    inspect(bad,'INVALID_TRANSCRIPT')
    sample_doc=copy.deepcopy(document);sample_doc['words']=copy.deepcopy(maximum['words'][:128])
    for i,word in enumerate(sample_doc['words']):
        word['start']=time(20000+20*i,48000);word['end']=time(20003+20*i,48000)
    sample_project=copy.deepcopy(project);sample_project['tracks']['links']=[]
    sample_spec={**spec,'clip_id':'audio','track_ids':['a'],'clock':'audio','end_policy':'keep',
        'ranges':[{'first_id':f'm{i}','last_id':f'm{i}'} for i in range(128)]}
    sample_plan=plan(p=sample_project,doc=sample_doc,value=sample_spec)
    assert len(sample_plan['merged_cuts'])==128 and len(sample_plan['native_operations'])==128
    sample_result=apply(sample_project,[sample_plan['operation']])
    assert len(sample_result['tracks']['tracks'][1]['clips'])==129
    assert sample_result['tracks']['tracks'][0]==sample_project['tracks']['tracks'][0]
    assert sum(seconds(c['duration']) for c in sample_result['tracks']['tracks'][1]['clips'])==F(3)-F(384,48000)
    too_many=copy.deepcopy(sample_spec);too_many['ranges'].append(too_many['ranges'][0])
    plan(p=sample_project,doc=sample_doc,value=too_many,error='INVALID_TRANSCRIPT')
    # Video participation refuses those sample-only boundaries.
    video_samples=copy.deepcopy(sample_spec);video_samples['clip_id']='video';video_samples['track_ids']=['v']
    plan(doc=sample_doc,value=video_samples,error='UNALIGNED_TIME')
    passed.append('transcript.document_batch_and_sample_cut_bounds')
    # A separate identical copy may bind, but actual stale bytes at that path reject.
    other=output/'copy.mkv';other.write_bytes(movie.read_bytes())
    p=copy.deepcopy(project);p['assets'][0]['path']=str(other)
    request={'command':'transcript.plan','project':p,'document':doc,'expected_revision':p['revision'],
        'expected_document_fingerprint':corrected['fingerprint'],'spec':spec,'input_root':str(root)}
    # Root-relative document path must follow the wider allowed input root.
    request['document']=copy.deepcopy(doc);request['document']['source']['path']='source/voice.mkv'
    request['expected_document_fingerprint']=call({'command':'transcript.inspect','document':request['document'],'input_root':str(root)})['fingerprint']
    call(request)
    with other.open('r+b') as stream:stream.seek(100);value=stream.read(1);stream.seek(100);stream.write(bytes([value[0]^1]))
    call(request,'MEDIA_CHANGED')
    call({'command':'render.run','project':edited,'input_root':str(source),'output_root':str(source),'output':str(movie)},'OUTPUT_EXISTS')
    preserved();passed.append('transcript.atomic_rejections_and_source_preservation')
    report={'passed':passed,'frames_compared':frames,'stereo_sample_frames_compared':samples,'previews':previews,
        'rejected_cases':rejected,'cases':cases,'recognition_accuracy':'not exercised; authored contract fixture',
        'reference':'Original coded pixels and stereo samples with literal interval deletion; exact source clocks and a seven-sample linked offset.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report,indent=2))
    return report


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
