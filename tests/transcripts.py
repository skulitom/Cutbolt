"""Original transcript correction/cut integration; no recognition accuracy credit.

Synthetic coded pixels/PCM expose every exact deletion independently. Authored
word anchors exercise the editing contract; actual recognizer acceptance is separate.
"""
from engine import ENGINE, MCP_TOOLS
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
EXE = ENGINE
W, H, N = 24, 16, 100


def outline_text(p, docs=(), words=12, start=None, end=None, sequence=None, root=None):
    """Independent reading of the timeline.outline text contract."""
    import math
    rounded = [False]
    def at(v):
        t = seconds(v) if isinstance(v, dict) else F(v)
        n, d = t.numerator*1000, t.denominator
        ms = n//d if n % d == 0 else (n + d//2)//d
        if n % d:rounded[0] = True
        text = str(ms//1000) + ('.'+f'{ms%1000:03}'.rstrip('0') if ms % 1000 else '')
        return text + ('~' if n % d else '')
    def span(a, b):return f'{at(a)}-{at(b)}'
    def plural(n, noun):return f'{n} {noun}{"" if n == 1 else "s"}'
    spoken, matched, unused = {}, {}, []
    for d in docs:
        src = d['source']['path'].replace('\\', '/');hits = [];reason = 'no asset has its source path'
        for a in p['assets']:
            path = a['path']
            if root is not None and Path(path).is_absolute():
                try:path = str(Path(path).relative_to(root))
                except ValueError:path = ''
            path = path.replace('\\', '/')
            if a.get('identity') is None:
                if path == src:hits.append(a['id'])
            elif a['identity'] == d['source']['identity']:hits.append(a['id'])
            elif path == src:reason = 'the asset at its source path has a different content identity'
        if not hits:unused.append((d['id'], reason, src))
        matched[d['id']] = hits
        for h in hits:
            entry = spoken.setdefault(h, ([], []))
            entry[0].append((seconds(d['range_start']), seconds(d['range_start'])+seconds(d['range_duration'])))
            entry[1].extend(d['words'])
    for ranges, ws in spoken.values():
        ranges.sort();ws.sort(key=lambda w:seconds(w['start']))
    def snippet(asset, a, b):
        ranges, ws = spoken[asset]
        covered = sum((max(F(0), min(e, b)-max(s, a)) for s, e in ranges), F(0))
        if covered == 0:return ' (not transcribed)'
        part = ' (partly transcribed)' if covered < b-a else ''
        inside = [w['text']+('*' if seconds(w['start']) < a or seconds(w['end']) > b else '') for w in ws if seconds(w['start']) < b and a < seconds(w['end'])]
        n = len(inside)
        if not inside:return ' (no speech)'+part
        if words == 0:return f' ({plural(n, "word")}){part}'
        if n <= words:return ' "'+' '.join(inside)+'"'+part
        head = (words+1)//2;tail = inside[n-(words-head):]
        return ' "'+' '.join(inside[:head])+' ...'+(' ' if tail else '')+' '.join(tail)+f'" ({plural(n, "word")}){part}'
    arrangement = p.get('tracks') if sequence is None else next(s['arrangement'] for s in p['sequences'] if s['id'] == sequence)
    duration = seconds(arrangement['duration']) if arrangement else sum((seconds(c['duration']) for c in p['clips']), F(0))
    lo = seconds(start) if start else F(0);hi = min(seconds(end), duration) if end else duration
    lines = [];listed = 0
    def holes(spans):
        found = [];cur = lo
        for s, e in sorted(spans):
            if cur < s:found.append((cur, min(s, hi)))
            cur = max(cur, e)
            if cur >= hi:return found
        if cur < hi:found.append((cur, hi))
        return found
    def hole_line(name, found):
        if not found:return f'{name}: none'
        shown = [span(a, b) for a, b in found[:20]]+([f'+{len(found)-20} more'] if len(found) > 20 else [])
        return f'{name}: '+', '.join(shown)
    if arrangement:
        linked = {m['clip_id']:l['id'] for l in arrangement['links'] for m in l['members']}
        picture, sound = [], []
        for t in arrangement['tracks']:
            clips = []
            for c in t['clips']:
                s, e = seconds(c['start']), seconds(c['start'])+seconds(c['duration'])
                if s < hi and lo < e:clips.append(c)
                if t['enabled'] and t['kind'] == 'video' and t.get('composite', 'opaque') == 'opaque':picture.append((s, e))
                if t['enabled'] and t['kind'] == 'audio':sound.append((s, e))
            clips.sort(key=lambda c:seconds(c['start']))
            flags = [t['kind']]+(['alpha_over'] if t.get('composite') == 'alpha_over' else [])+([] if t['enabled'] else ['disabled'])+(['locked'] if t['locked'] else [])
            lines.append(f"track {t['id']} ({', '.join(flags)}): {plural(len(clips), 'clip')}")
            for c in clips:
                s = seconds(c['start']);e = s+seconds(c['duration']);si = seconds(c['source_in']);so = si+seconds(c['duration'])
                source = 'seq:'+c['sequence_id'] if c.get('sequence_id') else c['asset_id']
                line = f"  {span(s, e)} {c['id']} {source} {span(si, so)}"
                if c['id'] in linked:line += f" link {linked[c['id']]}"
                if t['kind'] == 'audio':
                    if c.get('gain_curve'):line += f" gain curve({len(c['gain_curve']['keys'])} keys)"
                    elif c.get('gain_milli', 1000) == 0:line += ' muted'
                    elif c.get('gain_milli', 1000) != 1000:line += f" gain {20*math.log10(c['gain_milli']/1000):+.1f}dB"
                    if c.get('fade_in') and seconds(c['fade_in']):line += f" fade_in {at(c['fade_in'])}"
                    if c.get('fade_out') and seconds(c['fade_out']):line += f" fade_out {at(c['fade_out'])}"
                if c.get('transform'):
                    tr = c['transform'];line += ' pip'
                    if tr.get('crop'):line += ' crop '+','.join(map(str, tr['crop']))
                    if tr.get('divisor', 1) != 1:line += f" /{tr['divisor']}"
                    if tr.get('opacity', 255) != 255:line += f" opacity {tr['opacity']}"
                    x, y = tr.get('position', [0, 0]);line += f' at {x},{y}'
                if t['kind'] == 'audio' and not c.get('sequence_id') and c['asset_id'] in spoken:line += snippet(c['asset_id'], si, so)
                lines.append(line);listed += 1
                for x in t.get('transitions', []):
                    if x['left_id'] == c['id']:
                        lines.append(f"  {span(e-seconds(x['before']), e+seconds(x['after']))} [{x['id']} {x['kind']} {x['left_id']}>{x['right_id']}]")
        lines.append(hole_line('black (no opaque video clip)', holes(picture)))
        lines.append(hole_line('no audio clip', holes(sound)))
    else:
        body = [];cur = F(0)
        for c in p['clips']:
            e = cur+seconds(c['duration'])
            if cur < hi and lo < e:
                line = f"  {span(cur, e)} {c['id']}"
                if c.get('asset_id'):
                    si = seconds(c['source_in']);so = si+seconds(c['duration'])
                    line += f" {c['asset_id']} {span(si, so)}"
                    if c['asset_id'] in spoken:line += snippet(c['asset_id'], si, so)
                else:line += ' gap'
                body.append(line)
            cur = e
        lines.append(f"sequential timeline (video with its audio): {plural(len(body), 'clip')}")
        lines += body;listed = len(body)
    rate = seconds(p['frame_rate'])
    whole = lo == 0 and hi == duration
    header = [f"{'sequence '+sequence+' of project' if sequence else 'project'} {p['id']} rev {p['revision']}: {p['width']}x{p['height']} {rate} fps, "
              + (f'duration {at(duration)}' if whole else f'range {span(lo, hi)} of {at(duration)}')
              + (', tracks bottom to top' if arrangement else '')]
    header.append('assets: '+(', '.join(f"{a['id']}={a['path']} ({at(a['duration'])})" for a in p['assets']) or 'none'))
    if not sequence and p.get('sequences'):
        header.append('sequences (outline one with sequence_id): '+', '.join(f"{s['id']}{' multicam' if s.get('multicam') else ''} ({at(s['arrangement']['duration'])})" for s in p['sequences']))
    legend = ['times in seconds, clip lines: timeline span, id, source, source span']+(['~ rounded to the millisecond'] if rounded[0] else [])+(['* word partly outside the clip'] if docs else [])
    header.append('legend: '+'; '.join(legend))
    if docs:
        used = [f"{d['id']}>{'+'.join(matched[d['id']])}" for d in docs if matched[d['id']]] or ['none']
        header.append('transcripts: '+', '.join(used)+''.join(f'; unused {i} ({r}: {s})' for i, r, s in unused))
    text = '\n'.join(header+lines)+'\n'
    return text, listed, len(header+lines)


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
        client.initialize();catalog=client.rpc('tools/list')['result']['tools'];assert len(catalog)==MCP_TOOLS
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

        # Outline: the cut as text. Every line is recomputed here from the project and transcripts,
        # through MCP (whose text content is the outline itself) and the CLI.
        def outlined(p, docs=(), **options):
            fields = {'project':p, **({'transcripts':list(docs)} if docs else {}), **options}
            text, listed, count = outline_text(p, docs, **{k:v for k, v in options.items() if k in ('words', 'start', 'end')},
                sequence=options.get('sequence_id'), root=Path(options['input_root']) if 'input_root' in options else None)
            got = client.call('timeline.outline', **fields)
            assert got['outline'] == text and got['clips'] == listed and got['lines'] == count, (got['outline'], text)
            assert call({'command':'timeline.outline', **fields}) == got
            return got
        tool = next(t for t in catalog if t['name'] == 'cutbolt_timeline_outline')
        assert tool['annotations']['readOnlyHint'] and not tool['annotations']['openWorldHint']
        pieces = [sorted(t['clips'], key=lambda c:seconds(c['start'])) for t in restored['tracks']['tracks']]
        dressed = apply(restored, [
            edit('transition_set', track_id='v', transition={'id':'mix', 'left_id':pieces[0][0]['id'], 'right_id':pieces[0][1]['id'],
                'before':time(1, 25), 'after':time(2, 25), 'kind':'dissolve'}),
            edit('clip_audio', clip_ids=[pieces[1][0]['id']], gain_milli=700, fade_in=time(1, 10)),
            edit('clip_audio', clip_ids=[pieces[1][1]['id']], gain_curve={'keys':[
                {'time':time(26, 25), 'value':1000, 'interpolation':'linear'}, {'time':time(30, 25), 'value':0, 'interpolation':'hold'}]}),
            edit('clip_audio', clip_ids=[pieces[1][2]['id']], gain_milli=0),
            edit('add', track={**track('pip', 'video'), 'composite':'alpha_over'}),
            edit('place', track_id='pip', clip=placement('inset', 'voice', 2, 10, 0), collision='reject'),
            edit('clip_transform', clip_ids=['inset'], transform={'crop':[0, 0, 12, 8], 'divisor':2, 'opacity':128, 'position':[3, -2]}),
            edit('add', track={**track('mute', 'audio'), 'enabled':False, 'locked':True})])
        text = outlined(dressed, [doc])['outline']
        assert '"red,"' in text and '"κύκλος"' in text and '"circle."' in text and 'square' not in text and 'blue' not in text, text
        assert ' [mix dissolve ' in text and ' pip crop 0,0,12,8 /2 opacity 128 at 3,-2' in text and 'gain curve(2 keys)' in text and ' muted' in text, text
        assert '~ rounded to the millisecond' in text and 'track mute (audio, disabled, locked): 0 clips' in text, text
        outlined(dressed)
        outlined(dressed, [doc], words=0)
        outlined(dressed, [doc], start=time(27, 25), end=time(46, 25))
        outlined(dressed, [doc], start=time(80, 25))
        # A sequential timeline at 29.97 fps: long speech is shortened, words cut by a clip end carry
        # *, and transcripts meet without overlapping. Paths match when the asset is unbound.
        ntsc = time(30000, 1001)
        talk = call({'command':'project.create', 'id':'talk', 'width':W, 'height':H, 'frame_rate':ntsc})
        talk = apply(talk, [{'op':'media.add', 'asset':{'id':'talk', 'path':'talk.wav', 'duration':time(40)}},
            {'op':'media.add', 'asset':{'id':'abs', 'path':str(source/'abs.wav'), 'duration':time(40)}},
            {'op':'clip.append', 'clip':{'id':'c1', 'asset_id':'talk', 'source_in':time(0), 'duration':time(100*1001, 30000)}},
            {'op':'clip.append', 'clip':{'id':'g', 'gap':True, 'source_in':time(0), 'duration':time(30*1001, 30000)}},
            {'op':'clip.append', 'clip':{'id':'c2', 'asset_id':'talk', 'source_in':time(300*1001, 30000), 'duration':time(700*1001, 30000)}},
            {'op':'clip.append', 'clip':{'id':'c3', 'asset_id':'abs', 'source_in':time(900*1001, 30000), 'duration':time(30*1001, 30000)}}])
        def speech(name, path, first, count, offset=0):
            return {**document, 'id':name, 'source':{'path':path, 'identity':{'sha256':'e'*64, 'bytes':9}, 'duration':time(40)},
                'range_start':time(first), 'range_duration':time(count),
                'words':[{'id':f'{name}{i}', 'text':f'{name}{i}', 'start':time(first*4+i+offset, 4), 'end':time(first*20+5*(i+offset)+4, 20),
                    'origin':'estimated', 'probability_milli':500} for i in range(count*4-1-offset)]}
        early, late = speech('a', 'talk.wav', 0, 20), speech('b', 'talk.wav', 20, 20)
        far = speech('z', 'abs.wav', 30, 10)
        for options in ({}, {'words':1}, {'words':5}, {'words':2048}, {'words':0}, {'start':time(4), 'end':time(12)}):
            outlined(talk, [early, late, far], **options)
        text = outlined(talk, [early, late, far])['outline']
        assert 'a13*' in text and '(no speech)' not in text and '~' in text and 'unused z (no asset has its source path: abs.wav)' in text, text
        outlined(talk, [far, late], input_root=str(source))
        outlined(talk, [speech('q', 'talk.wav', 25, 1, 2)])
        # A child sequence is outlined on its own clock, and the parent lists it.
        nest = apply(dressed, [{'op':'sequence.create', 'id':'inner', 'duration':time(20, 25)},
            {'op':'sequence.edit', 'id':'inner', 'edit':{'op':'add', 'track':track('sa', 'audio')}},
            {'op':'sequence.edit', 'id':'inner', 'edit':{'op':'place', 'track_id':'sa', 'collision':'reject',
                'clip':placement('said', 'voice', 0, 20*1920, 8*1920, 48000)}}])
        assert 'sequences (outline one with sequence_id): inner (0.8)' in outlined(nest, [doc])['outline']
        outlined(nest, [doc], sequence_id='inner')
        other_identity = copy.deepcopy(doc);other_identity['id'] = 'stranger';other_identity['source']['identity']['bytes'] += 1
        text = outlined(dressed, [doc, other_identity], input_root=str(source))['outline']
        assert 'unused stranger (the asset at its source path has a different content identity: voice.mkv)' in text, text
        for p, fields, code in ((dressed, {'words':2049}, 'INVALID_ARGUMENT'), (dressed, {'start':time(4), 'end':time(4)}, 'INVALID_RANGE'),
                                (dressed, {'start':time(91, 25)}, 'INVALID_RANGE'), (dressed, {'sequence_id':'none'}, 'MISSING_SEQUENCE'),
                                (dressed, {'transcripts':[doc, doc]}, 'INVALID_ARGUMENT'),
                                (talk, {'transcripts':[early, speech('c', 'talk.wav', 19, 2)]}, 'INVALID_ARGUMENT')):
            client.call('timeline.outline', code, project=p, **fields)
        passed.append('transcript.timeline_outline_matches_independent_reading')
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
