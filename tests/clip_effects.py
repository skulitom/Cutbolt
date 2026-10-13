"""Saved source-clock pixel effects: extracted-frame scene oracles and exact composition."""
import argparse
import copy
import hashlib
import json
import subprocess
from fractions import Fraction as F
from pathlib import Path

from PIL import Image
from agents import Client
from jsonschema import Draft202012Validator
from engine import ENGINE
from grading import changed
from keying import key
from scenes import identity
from tracks import time, edit, placement, track

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 24, 12
STRIDE = W * H * 3


def curve(entries):
    return {"keys":[{"time":time(F(t).numerator,F(t).denominator), "value":v, "interpolation":mode} for t,v,mode in entries]}


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, outputs, jobs = [root / p for p in ('sources', 'outputs', 'jobs')]
    for path in (sources, outputs, jobs):
        path.mkdir()
    cases, passed = [], []

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args],
                              check=True, capture_output=True, timeout=180).stdout

    def call(command, error=None, **fields):
        request = {'command': command, **fields}
        result = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, timeout=240)
        answer = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and answer['error']['code'] == error, (error, answer)
            return answer['error']
        if result.returncode or not answer['ok']:
            (root / 'failed-request.json').write_text(json.dumps(request, indent=2))
        assert result.returncode == 0 and answer['ok'], answer
        return answer['result']

    def apply(project, operations):
        return call('timeline.apply', project=project, expected_revision=project['revision'], operations=operations)

    def decoded(path):
        return ff(['-i', str(path), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])

    raw = b''.join(bytes(v for y in range(H) for x in range(W) for v in
                        ((0, 255, 0) if x < 5 else
                         ((x*11+n*17)%256, (y*13+n*5)%256, (x*7+y*3+n*19)%256))) for n in range(N))
    (sources/'chart.rgb').write_bytes(raw)
    (sources/'silence.pcm').write_bytes(bytes(N*1920*4))
    source = sources/'source.mkv'
    ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(sources/'chart.rgb'),
        '-f','s16le','-ar','48000','-ac','2','-i',str(sources/'silence.pcm'),'-map','0:v','-map','1:a',
        '-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le',
        '-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(source)])
    assert decoded(source) == raw
    phone = sources/'phone.mp4'
    ff(['-i',str(source),'-vf','scale=in_range=pc:out_range=tv:out_color_matrix=bt709','-c:v','libx264',
        '-crf','18','-g','8','-bf','2','-pix_fmt','yuv420p','-c:a','aac','-ar','48000',
        '-color_range','tv','-colorspace','bt709','-color_primaries','bt709','-color_trc','bt709',str(phone)])
    # The oracle source is the existing explicit f64 conform, independent of the new effect path.
    conformed = sources/'phone-reference.mkv'
    call('media.conform', input_root=str(sources), output_root=str(sources), output=str(conformed),
         recipe={'schema_version':1,'id':'oracle-source','source':{'file':identity(phone,sources),
                 'sdr':{'matrix':'bt709','range':'limited','transfer':'bt709','missing_tags':'reject'},'clock':'first_video_frame'},
                 'source_in':time(0),'duration':time(N,25),'rate':time(1),'reverse':False,'freeze':False,
                 'width':W,'height':H,'audio':'resample','working_transfer':'srgb','frame_rate':time(25)})
    originals = {'ffv1':raw, 'phone':decoded(conformed)}
    assert all(len(p) == N*STRIDE for p in originals.values())
    frames = {}
    for name, pixels in originals.items():
        frames[name] = []
        for n in range(N):
            path = sources/f'{name}-{n}.png'
            Image.frombytes('RGB',(W,H),pixels[n*STRIDE:(n+1)*STRIDE]).save(path)
            frames[name].append({'image':identity(path,sources),'hold':time(1,25),'offset':[0,0],'anchor':[0,0]})
    table = sources/'channels.cube'
    table.write_text('LUT_1D_SIZE 2\n0 0.1 0.2\n1 0.8 0.6\n', encoding='ascii')
    lut = {'kind':'lut','file':identity(table,sources),'interpolation':'linear'}
    grade = changed(exposure_milli=850, contrast_milli=1200, white_balance_milli=[1100,900,1050])
    animated = changed(animation={'exposure_milli':curve([(0,-500,'linear'),(F(N,25),1800,'hold')])})
    selective = {'kind':'selective_grade','grade':{k:v for k,v in grade.items() if k!='kind'},
                 'mix_milli':700,'mask':{'rect':[8,4,16,12],'feather':2},
                 'mix_curve':curve([(0,0,'linear'),(F(N,25),1000,'hold')])}
    moving_mask = copy.deepcopy(selective)
    moving_mask['mask']['animation'] = {'x':curve([(0,4,'linear'),(F(N,25),20,'hold')])}
    stacks = {'bypass':[], 'grade':[grade], 'animated':[animated], 'selective':[selective],
              'key':[key()], 'animated-key':[key(strength_curve=curve([(0,0,'linear'),(F(N,25),1000,'hold')]))],
              'moving-mask':[moving_mask], 'lut':[lut], 'grade-lut':[grade,lut], 'lut-grade':[lut,grade], 'key-grade-lut':[key(),grade,lut]}
    source_hashes = {p:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def scene_render(name, stack, images=None, width=W, height=H):
        recipe = {'schema_version':1,'id':name,'width':width,'height':height,'output_scale':1,
                  'duration':time(N,25),'background':[0,0,0],'color':'srgb_straight_encoded','audio':None,
                  'layers':[{'id':'source','canvas':[width,height],'start':time(0),'duration':time(N,25),
                             'frames':images or frames[name.split('-')[0]],'timing':'strict','end':'hold_last',
                             'effects':stack,'transform':{'position':[0,0],'crop':[0,0,width,height],
                                                         'scale':1,'quarter_turns':0,'opacity':255}}]}
        path = outputs/(name+'-scene.mkv')
        call('scene.render',scene=recipe,input_root=str(sources),output_root=str(outputs),output=str(path))
        return decoded(path)

    def project(name, start=3, count=6):
        p = call('project.create',id=name,width=W,height=H,frame_rate=time(25))
        asset = {'id':'source','path':str(source if name=='ffv1' else phone),'duration':time(N,25)}
        p = apply(p,[{'op':'media.add','asset':asset,'input_root':str(sources)},
                     {'op':'project.transfer','transfer':'srgb'},edit('create',duration=time(count,25)),
                     edit('add',track=track('v','video')),edit('place',track_id='v',clip=placement('c','source',0,count,start),collision='reject')])
        return p

    def render(p, name, wanted):
        path = outputs/(name+'.mkv')
        receipt = call('render.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(path))
        actual = decoded(path)
        assert actual == wanted, (name, len(actual), len(wanted), max(abs(a-b) for a,b in zip(actual,wanted)))
        print('verified '+name, flush=True)
        cases.append({'case':name,'frames':len(actual)//STRIDE,'rgb_sha256':hashlib.sha256(actual).hexdigest()})
        return receipt

    snapshots, references = {}, {}
    for codec in ('ffv1','phone'):
        base = project(codec)
        if codec == 'phone':
            assert base['assets'][0]['decode']['mode'] == 'direct'
        for name, stack in stacks.items():
            p = apply(base,[edit('clip_effects',clip_id='c',effects=stack)])
            reference = scene_render(codec+'-'+name,stack)
            wanted = reference[3*STRIDE:9*STRIDE]
            receipt = render(p,codec+'-'+name,wanted)
            if any(e['kind']=='lut' for e in stack):
                assert any(s['sha256']==lut['file']['sha256'] and s['frames']==s['samples']==0 for s in receipt['sources'])
            snapshots[codec,name], references[codec,name] = p,reference
    passed.append('clip_effects.exact_scene_oracles_on_ffv1_and_direct_h264')

    client = Client(ENGINE)
    try:
        client.initialize()
        tools = client.rpc('tools/list')['result']['tools']
        schema = next(t['inputSchema'] for t in tools if t['name']=='cutbolt_timeline_apply')
        base = snapshots['ffv1','bypass']
        arguments = {'project':base,'expected_revision':base['revision'],
                     'operations':[edit('clip_effects',clip_id='c',effects=[grade,lut])]}
        Draft202012Validator(schema).validate(arguments)
        result = client.call('timeline.apply',**arguments)
        assert result['schema_version']==2 and result['tracks']['tracks'][0]['clips'][0]['effects']==[grade,lut]
    finally:
        client.close()
    passed.append('clip_effects.mcp_saved_effect_schema_and_edit')

    for codec in ('ffv1','phone'):
        p = snapshots[codec,'animated']
        slipped = apply(p,[edit('slip',clip_ids=['c'],shift={'backward':False,'amount':time(2,25)},links='include')])
        wanted = references[codec,'animated'][5*STRIDE:11*STRIDE]
        render(slipped,codec+'-slipped',wanted)
        split = apply(slipped,[edit('split',clip_ids=['c'],at=time(3,25),links='include',
                                   right_clip_ids=[{'id':'c','new_id':'right'}],right_link_ids=[])])
        render(split,codec+'-split',wanted)
    passed.append('clip_effects.slips_and_splits_preserve_effect_source_clock')

    p = snapshots['phone','grade-lut']
    preview = outputs/'preview.png'
    call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(2,25))
    assert Image.open(preview).convert('RGB').tobytes() == references['phone','grade-lut'][5*STRIDE:6*STRIDE]
    ranged = outputs/'range.mkv'
    args = dict(project=p,input_root=str(sources),output_root=str(outputs),profile='reference',streams='video',
                range={'start':time(1,25),'duration':time(3,25)})
    call('export.run',output=str(ranged),**args)
    expected = references['phone','grade-lut'][4*STRIDE:7*STRIDE]
    assert decoded(ranged) == expected
    ticket = call('job.start',job_root=str(jobs),request_id='effects',run='export.run',arguments={**args,'output':str(outputs/'queued.mkv')})
    waited = call('job.wait',job_root=str(jobs),job_id=ticket['job_id'],timeout_seconds=120,detail='full')
    assert waited['status']=='completed', waited
    assert decoded(outputs/'queued.mkv') == expected
    # The final H.264 profile uses the same effected RGB encoder input as the scene oracle.
    direct_export, scene_export = outputs/'direct.mp4', outputs/'scene.mp4'
    call('export.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(direct_export),profile='h264_aac',streams='video')
    reference_file = outputs/'phone-grade-lut-scene.mkv'
    ref = call('project.create',id='export-oracle',width=W,height=H,frame_rate=time(25))
    ref = apply(ref,[{'op':'media.add','input_root':str(outputs),'asset':{'id':'oracle','path':str(reference_file),'duration':time(N,25)}},
                    {'op':'project.transfer','transfer':'srgb'}, {'op':'clip.append','clip':{'id':'c','asset_id':'oracle','source_in':time(3,25),'duration':time(6,25)}}])
    call('export.run',project=ref,input_root=str(outputs),output_root=str(outputs),output=str(scene_export),profile='h264_aac',streams='video')
    assert decoded(direct_export) == decoded(scene_export)
    passed.append('clip_effects.preview_range_queue_and_h264_export')

    # Identical source clocks on both sides; independently mix their differently effected pixels.
    trans = project('ffv1',0,N)
    trans = apply(trans,[edit('remove',clip_ids=['c'],links='include'),
                        edit('place',track_id='v',clip=placement('left','source',0,6),collision='reject'),
                        edit('place',track_id='v',clip=placement('right','source',6,6,6),collision='reject'),
                        edit('clip_effects',clip_id='left',effects=[grade]),
                        edit('clip_effects',clip_id='right',effects=[animated]),
                        edit('transition_set',track_id='v',transition={'id':'blend','left_id':'left','right_id':'right','kind':'dissolve','before':time(2,25),'after':time(2,25)})])
    left,right = references['ffv1','grade'], references['ffv1','animated']
    expected = bytearray(left[:4*STRIDE])
    for n in range(4,8):
        k = 2*(n-4)+1
        expected.extend((a*(8-k)+b*k+4)//8 for a,b in zip(left[n*STRIDE:(n+1)*STRIDE],right[n*STRIDE:(n+1)*STRIDE]))
    expected.extend(right[8*STRIDE:])
    render(trans,'effected-transition',bytes(expected))
    passed.append('clip_effects.transition_inputs_use_source_clock')

    # Effects sample masks in source coordinates before the existing crop/shrink operation.
    over = project('ffv1',0,N)
    overlay = track('overlay','video'); overlay['composite']='alpha_over'
    clip = placement('top','overlay-source',0,N)
    clip['transform'] = {'crop':[4,4,16,12],'divisor':2,'position':[5,4],'opacity':255}
    overlay_source = copy.deepcopy(over['assets'][0]); overlay_source['id']='overlay-source'
    over = apply(over,[{'op':'media.add','asset':overlay_source},edit('add',track=overlay),edit('place',track_id='overlay',clip=clip,collision='reject'),
                      edit('clip_effects',clip_id='top',effects=[selective])])
    reference = references['ffv1','selective']
    expected = bytearray(raw)
    for n in range(N):
        for y in range(6):
            for x in range(8):
                for c in range(3):
                    value = sum(reference[n*STRIDE+((4+2*y+dy)*W+4+2*x+dx)*3+c] for dx in range(2) for dy in range(2))//4
                    expected[n*STRIDE+((4+y)*W+5+x)*3+c] = value
    render(over,'cropped-overlay',bytes(expected))
    passed.append('clip_effects.overlay_masks_before_crop_and_shrink')

    # Proxy processing uses the same source-clock stack; its mask samples the source-space center
    # of each proxy pixel. Use nearest-upsampled proxy frames in the full-size scene oracle.
    p = snapshots['ffv1','selective']
    proxy = sources/'proxy.mkv'
    generated = call('proxy.generate',project=p,expected_revision=p['revision'],asset_id='source',scale=2,
                     input_root=str(sources),output_root=str(sources),output=str(proxy))
    selected = apply(p,generated['operations']+[{'op':'preview.proxy','scale':2}])
    pixels = decoded(proxy); proxy_frames=[]
    for n in range(N):
        path = sources/f'proxy-{n}.png'
        Image.frombytes('RGB',(W//2,H//2),pixels[n*STRIDE//4:(n+1)*STRIDE//4]).resize((W,H),Image.Resampling.NEAREST).save(path)
        proxy_frames.append({'image':identity(path,sources),'hold':time(1,25),'offset':[0,0],'anchor':[0,0]})
    proxy_oracle = scene_render('proxy-oracle',[selective],images=proxy_frames)
    proxy_preview = outputs/'proxy-preview.png'
    call('preview.frame',project=selected,input_root=str(sources),output_root=str(outputs),output=str(proxy_preview),time=time(2,25))
    wanted = bytes(proxy_oracle[5*STRIDE+((2*y+1)*W+2*x+1)*3+c] for y in range(H//2) for x in range(W//2) for c in range(3))
    assert Image.open(proxy_preview).convert('RGB').tobytes() == wanted
    render(selected,'proxy-selected-final',references['ffv1','selective'][3*STRIDE:9*STRIDE])
    passed.append('clip_effects.proxy_preview_coordinates_and_original_quality_final')

    # Cached previews must hash LUT dependencies before accepting a hit as well as before publication.
    p = snapshots['phone','grade-lut']
    cache = root/'cache'; cache.mkdir()
    task = {'type':'frame','project':p,'input_root':str(sources),'output_root':str(outputs),'time':time(2,25)}
    policy = {'max_bytes':64*1024*1024,'max_entries':128}
    for n in range(2):
        cached = call('cache.run',cache_root=str(cache),policy=policy,task={**task,'output':str(outputs/f'cached-{n}.png')})
        assert cached['cache']['hit'] == (n==1)
        assert Image.open(outputs/f'cached-{n}.png').convert('RGB').tobytes() == references['phone','grade-lut'][5*STRIDE:6*STRIDE]
    # A same-length mutation is rejected even when an otherwise valid cached result exists.
    original = table.read_bytes()
    table.write_bytes(original.replace(b'0.8',b'0.7'))
    denied = outputs/'changed-lut.mkv'
    call('render.run',error='MEDIA_CHANGED',project=p,input_root=str(sources),output_root=str(outputs),output=str(denied))
    assert not denied.exists()
    denied_cache = outputs/'changed-lut-cached.png'
    call('cache.run',error='MEDIA_CHANGED',cache_root=str(cache),policy=policy,task={**task,'output':str(denied_cache)})
    assert not denied_cache.exists()
    table.write_bytes(original)
    passed.append('clip_effects.cache_hits_verify_lut_content')
    assert all(hashlib.sha256(p.read_bytes()).hexdigest()==value for p,value in source_hashes.items())
    passed.append('clip_effects.original_sources_preserved')
    report = {'passed':passed,'cases':cases,'rgb_tolerance':0,'oracle':'Existing scene processor on explicit-conform source frames; independent integer transition mix',
              'scope':'Pixel effects against a single-layer scene on black; lower-track key coverage is covered by native_layers'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
