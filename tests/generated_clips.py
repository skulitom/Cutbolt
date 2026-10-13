"""Generated timeline clips against independently compiled scene assets and exact PCM windows."""
import argparse
import copy
import hashlib
import json
import struct
import subprocess
import wave
from pathlib import Path

from PIL import Image
from engine import ENGINE
from scenes import identity
from tracks import edit, placement, time, track
from sequences import nested, seqedit

ROOT = Path(__file__).resolve().parents[1]


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    sources, outputs = root/'sources', root/'outputs'
    sources.mkdir(parents=True); outputs.mkdir()
    passed = []

    def call(command, error=None, workspace=None, **fields):
        args = [str(ENGINE)] + (['--workspace', str(workspace)] if workspace else [])
        request = dict(command=command, **fields)
        result = subprocess.run(args, input=json.dumps(request).encode(), capture_output=True, timeout=240)
        value = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and value['error']['code'] == error, value
            return value['error']
        if result.returncode or not value['ok']:
            (root/'failed-request.json').write_text(json.dumps(request, indent=2))
        assert result.returncode == 0 and value['ok'], value
        return value['result']

    def apply(project, operations):
        return call('timeline.apply', project=project, expected_revision=project['revision'], operations=operations)

    def decode(path, audio=False):
        fields = ['-vn','-c:a','pcm_s16le','-f','s16le'] if audio else ['-an','-pix_fmt','rgb24','-f','rawvideo']
        return subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),*fields,'-'], timeout=120)

    def render(project, name):
        path = outputs/(name+'.mkv')
        call('render.run', project=project, input_root=str(sources), output_root=str(outputs), output=str(path))
        return decode(path), decode(path, True)

    frames = []
    for n in range(8):
        path = sources/f'frame-{n}.png'
        image = Image.new('RGBA', (16,12))
        image.putdata([((x*19+n*13)%256,(y*23+n*7)%256,(x+y+n*11)%256,(x*13+y*7+n*3)%256) for y in range(12) for x in range(16)])
        image.save(path)
        frames.append(dict(image=identity(path,sources),hold=time(1,25),offset=[0,0],anchor=[0,0]))
    audio = sources/'voice.wav'
    pcm = b''.join(struct.pack('<hh',(n*37)%18000-9000,(n*19)%16000-8000) for n in range(8*1920))
    with wave.open(str(audio),'wb') as out:
        out.setnchannels(2); out.setsampwidth(2); out.setframerate(48000); out.writeframes(pcm)
    scene = dict(schema_version=1,id='generated',width=16,height=12,output_scale=2,duration=time(8,25),
        background=[23,47,71],color='srgb_straight_encoded',layers=[dict(id='moving',canvas=[16,12],start=time(0),
        duration=time(8,25),frames=frames,timing='strict',end='hold_last',
        transform=dict(position=[0,0],crop=[0,0,16,12],scale=1,quarter_turns=0,opacity=219))],
        audio=dict(file=identity(audio,sources),start=time(0),channels='preserve_stereo',resampling='linear',padding='silence'))
    originals = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    reference = call('scene.render',scene=scene,input_root=str(sources),output_root=str(sources),output=str(sources/'reference.mkv'))['asset']

    def project(recipe, first=0, count=8, sound=True):
        base = call('project.create',id='generated-test',width=32,height=24,frame_rate=time(25))
        v = track('v','video'); a = track('a','audio')
        def clip(id):
            result = dict(id=id,start=time(0),source_in=time(first,25),duration=time(count,25))
            if isinstance(recipe, str): result['asset_id'] = recipe
            else: result['scene'] = recipe
            return result
        return apply(base, [dict(op='media.add',asset=reference)] + [edit('create',duration=time(count,25)),
            edit('add',track=v),edit('place',track_id='v',clip=clip('picture'),collision='reject')]
            + ([edit('add',track=a),edit('place',track_id='a',clip=clip('sound'),collision='reject')] if sound else []))

    native = project(scene)
    assert native['schema_version'] == 2 and native['tracks']['tracks'][0]['clips'][0]['scene'] == scene
    baseline = render(project(reference['id']), 'asset-full')
    assert baseline[1] == pcm
    assert render(native, 'generated-full') == baseline
    trimmed = project(scene,2,4)
    assert render(trimmed,'generated-trim') == render(project(reference['id'],2,4),'asset-trim')
    assert render(trimmed,'generated-trim-repeat')[1] == pcm[2*1920*4:6*1920*4]
    plan = call('render.plan',project=native,input_root=str(sources),output_root=str(outputs),output=str(outputs/'plan.mkv'))
    assert plan['compositor']['stack']['clips'][0]['input']['kind'] == 'generated'
    assert not any(s['path'].endswith('reference.mkv') for s in plan['sources'])
    assert len(plan['sources']) == 9, plan['sources']
    assert plan['compositor']['scene_resources']['scene_count'] == 1
    passed.append('generated.exact_scene_pixels_and_pcm_without_video_compile')

    unrelated = copy.deepcopy(native)
    sound = unrelated['tracks']['tracks'][1]['clips'][0]
    sound['scene']['id'] = 'different-audio-source'
    sound['source_in'] = time(1,25); sound['duration'] = time(7,25)
    checked = call('timeline.check',project=unrelated,input_root=str(sources))
    assert not any(f['kind'] in ('out_of_sync','jump_cut') for f in checked['findings']['listed']), checked
    document = dict(schema_version=1,id='words',revision=0,parent_fingerprint=None,
        source=dict(path='reference.mkv',identity=reference['identity'],duration=time(8,25)),
        range_start=time(0),range_duration=time(8,25),language='en',
        recognition=dict(profile='synthetic-contract-fixture',model=dict(sha256='a'*64,bytes=1),
            worker_sha256='b'*64,analysis_sha256='c'*64,versions={'original-fixture':'1'}),
        words=[dict(id='w',text='test',start=time(0),end=time(1,25),origin='estimated',probability_milli=900)])
    fingerprint = call('transcript.inspect',document=document,input_root=str(sources))['fingerprint']
    call('transcript.plan',error='UNSUPPORTED_TRANSCRIPT_BINDING',project=native,document=document,
        expected_revision=native['revision'],expected_document_fingerprint=fingerprint,input_root=str(sources),
        spec=dict(clip_id='picture',track_ids=['v'],ranges=[dict(first_id='w',last_id='w')],clock='video',
            rounding='strict',estimates='allow_reported',collateral='reject',links='include',
            end_policy='resize',transitions='reject_affected'))
    passed.append('generated.static_check_and_explicit_text_binding_boundary')

    preview = outputs/'preview.png'
    call('preview.frame',project=native,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(3,25))
    assert Image.open(preview).convert('RGB').tobytes() == baseline[0][3*32*24*3:4*32*24*3]
    ranged = outputs/'range.mkv'
    call('preview.range',project=native,input_root=str(sources),output_root=str(outputs),output=str(ranged),start=time(2,25),duration=time(4,25))
    assert (decode(ranged),decode(ranged,True)) == render(project(reference['id'],2,4),'asset-range')
    split = apply(native,[edit('split',clip_ids=['picture'],at=time(3,25),right_clip_ids=[{'id':'picture','new_id':'picture-tail'}],right_link_ids=[],links='include')])
    assert render(split,'generated-split') == baseline
    passed.append('generated.source_clocks_previews_ranges_and_split')

    properties = [{'kind':'opacity','value':153}, {'kind':'mask','mask':{'rect':[2,2,24,16],
        'feather':{'radius':2,'edge':'centered'}}}]
    styled = apply(native,[edit('clip_effects',clip_id='picture',effects=properties)])
    styled_asset = apply(project(reference['id']),[edit('clip_effects',clip_id='picture',effects=properties)])
    assert render(styled,'generated-properties') == render(styled_asset,'asset-properties')

    transparent = copy.deepcopy(scene); transparent['id']='generated-alpha'; transparent['transparent']=True; transparent['audio']=None
    alpha_asset = call('scene.render',scene=transparent,input_root=str(sources),output_root=str(sources),output=str(sources/'alpha.mkv'))['asset']
    base = project(reference['id'])
    generated_overlay = apply(base,[edit('add',track=track('overlay','video')),
        edit('place',track_id='overlay',clip=dict(id='overlay',scene=transparent,start=time(0),source_in=time(0),duration=time(8,25)),collision='reject')])
    overlay = track('overlay','video'); overlay['composite']='alpha_over'
    asset_overlay = apply(base,[dict(op='media.add',asset=alpha_asset),edit('add',track=overlay),
        edit('place',track_id='overlay',clip=placement('overlay',alpha_asset['id'],0,8),collision='reject')])
    assert render(generated_overlay,'generated-alpha') == render(asset_overlay,'asset-alpha')
    passed.append('generated.transparent_scene_reveals_lower_tracks')

    # Child pictures and sound keep the same source clock as the independently compiled asset.
    child = [dict(op='sequence.create',id='child',duration=time(8,25))]
    for name,kind in [('v','video'),('a','audio')]:
        child += [seqedit('child',edit('add',track=track(name,kind))),
            seqedit('child',edit('place',track_id=name,clip=dict(id=name,scene=scene,start=time(0),source_in=time(0),duration=time(8,25)),collision='reject'))]
    child += [edit('remove',clip_ids=['picture','sound'],links='include'),edit('duration',duration=time(4,25)),
        edit('place',track_id='v',clip=nested('nested-picture','child',0,4,2),collision='reject'),
        edit('place',track_id='a',clip=nested('nested-sound','child',0,4,2),collision='reject')]
    nested_project = apply(native,child)
    assert render(nested_project,'nested-generated') == render(project(reference['id'],2,4),'nested-asset')
    proxy = copy.deepcopy(native); proxy['preview_scale']=2
    proxy_path = outputs/'generated-proxy.png'
    call('preview.frame',project=proxy,input_root=str(sources),output_root=str(outputs),output=str(proxy_path),time=time(3,25))
    full = Image.frombytes('RGB',(32,24),baseline[0][3*32*24*3:4*32*24*3])
    expected_proxy = bytes(c for y in range(12) for x in range(16) for c in full.getpixel((2*x,2*y)))
    assert Image.open(proxy_path).size == (16,12) and Image.open(proxy_path).convert('RGB').tobytes() == expected_proxy
    jobs = root/'jobs'; jobs.mkdir()
    queued = outputs/'queued-generated.mkv'
    ticket = call('job.start',job_root=str(jobs),request_id='generated',run='export.run',arguments=dict(project=native,
        input_root=str(sources),output_root=str(outputs),output=str(queued),profile='reference',streams='audio_video'))
    waited = call('job.wait',job_root=str(jobs),job_id=ticket['job_id'],timeout_seconds=120,detail='full')
    assert waited['status'] == 'completed', waited
    assert (decode(queued),decode(queued,True)) == baseline
    passed.append('generated.nested_audio_picture_proxy_and_queued_export')

    # File references resolve once into pinned scene content before the transaction is saved.
    unbound = copy.deepcopy(scene)
    for frame in unbound['layers'][0]['frames']: frame['image']={'path':frame['image']['path']}
    unbound['audio']['file']={'path':'voice.wav'}
    authoring = sources/'authoring.json'; authoring.write_text(json.dumps(unbound))
    blank = call('project.create',id='document-source',width=32,height=24,frame_rate=time(25))
    ops = [edit('create',duration=time(8,25))]
    for name,kind in [('v','video'),('a','audio')]:
        ops += [edit('add',track=track(name,kind)),edit('place',track_id=name,
            clip=dict(id=name,scene={'file':'authoring.json'},start=time(0),source_in=time(0),duration=time(8,25)),collision='reject')]
    document_project = call('timeline.apply',workspace=sources,project=blank,expected_revision=0,operations=ops)
    resolved = document_project['tracks']['tracks'][0]['clips'][0]['scene']
    assert resolved['layers'][0]['frames'][0]['image'] == scene['layers'][0]['frames'][0]['image']
    authoring.write_text('{}')
    assert render(document_project,'resolved-document') == baseline
    store = root/'store'; store.mkdir()
    call('session.create',store_root=str(store),project=native,request_id='create')
    changed_scene = copy.deepcopy(scene); changed_scene['background']=[101,53,19]
    replacement = copy.deepcopy(native['tracks']['tracks'][0]['clips'][0]); replacement['scene']=changed_scene
    request = dict(store_root=str(store),project_id=native['id'],expected_revision=0,request_id='replace-scene',operations=[
        edit('remove',clip_ids=['picture'],links='include'),edit('place',track_id='v',clip=replacement,collision='reject')])
    receipt = call('session.apply',**request)
    assert receipt == call('session.apply',**request)
    change = next(c for c in receipt['changes']['clips'] if c['clip_id']=='picture')
    assert change['before']['scene']['background'] == scene['background']
    assert change['after']['scene']['background'] == changed_scene['background']
    call('session.undo',store_root=str(store),project_id=native['id'],request_id='undo',expected_revision=1)
    restored = call('session.get',store_root=str(store),project_id=native['id'])
    assert render(restored,'restored-scene') == baseline
    passed.append('generated.workspace_documents_pinned_snapshots_receipts_retry_undo')

    # A second recipe first appears in the second render chunk; both summaries report the
    # same complete-export pool, and repeated first-recipe cuts retain only one prepared copy.
    changed_asset = sources/'changed-reference.mkv'
    call('scene.render',scene=changed_scene,input_root=str(sources),output_root=str(sources),output=str(changed_asset))
    changed_rgb = decode(changed_asset)
    operations = [edit('remove',clip_ids=['picture','sound'],links='include'),edit('duration',duration=time(65,25))]
    for n in range(65):
        operations.append(edit('place',track_id='v',clip=dict(id=f'cut-{n}',scene=scene if n<64 else changed_scene,
            start=time(n,25),source_in=time(n%8,25),duration=time(1,25)),collision='reject'))
    many = apply(native,operations)
    many_plan = call('render.plan',project=many,input_root=str(sources),output_root=str(outputs),output=str(outputs/'many-plan.mkv'))
    assert len(many_plan['chunks']) == 2
    resources = [c['scene_resources'] for c in many_plan['chunks']]
    assert resources[0] == resources[1] and resources[0]['scene_count'] == 2
    wanted = b''.join((baseline[0] if n<64 else changed_rgb)[(n%8)*32*24*3:(n%8+1)*32*24*3] for n in range(65))
    assert render(many,'chunked-generated') == (wanted,bytes(65*1920*4))
    passed.append('generated.shared_scene_pool_and_exact_chunk_boundaries')

    # Cached images must verify every saved scene dependency, even an unused definition.
    extra = sources/'unused.png'; Image.new('RGB',(16,12),(11,31,51)).save(extra)
    unused = copy.deepcopy(scene); unused['id']='unused'; unused['audio']=None
    unused['layers'][0]['frames']=[dict(image=identity(extra,sources),hold=time(8,25),offset=[0,0],anchor=[0,0])]
    with_unused = apply(native,[dict(op='sequence.create',id='unused',duration=time(8,25)),
        seqedit('unused',edit('add',track=track('v','video'))),
        seqedit('unused',edit('place',track_id='v',clip=dict(id='unused',scene=unused,start=time(0),source_in=time(0),duration=time(8,25)),collision='reject'))])
    cache = root/'cache'; cache.mkdir()
    task = dict(type='frame',project=with_unused,input_root=str(sources),output_root=str(outputs),time=time(3,25))
    policy = dict(max_bytes=64*1024*1024,max_entries=128)
    for n in range(2):
        result = call('cache.run',cache_root=str(cache),policy=policy,task={**task,'output':str(outputs/f'cached-{n}.png')})
        assert result['cache']['hit'] == (n==1)
    saved = extra.read_bytes(); extra.write_bytes(saved[:-1]+bytes([saved[-1]^1]))
    denied = outputs/'changed-unused.png'
    call('cache.run',error='MEDIA_CHANGED',cache_root=str(cache),policy=policy,task={**task,'output':str(denied)})
    assert not denied.exists(); extra.write_bytes(saved)
    active = sources/'frame-0.png'; saved = active.read_bytes(); active.write_bytes(saved[:-1]+bytes([saved[-1]^1]))
    checked = call('timeline.check',project=native,input_root=str(sources))
    assert any(f['kind']=='generated_source' and f['error_code']=='MEDIA_CHANGED' for f in checked['findings']['listed']), checked
    call('render.run',error='MEDIA_CHANGED',project=native,input_root=str(sources),output_root=str(outputs),output=str(outputs/'changed-scene.mkv'))
    assert not (outputs/'changed-scene.mkv').exists()
    active.write_bytes(saved)
    passed.append('generated.cache_hits_check_unused_and_active_scene_dependencies')

    bad = copy.deepcopy(native); bad['tracks']['tracks'][0]['clips'][0]['asset_id']=reference['id']
    call('render.plan',error='INVALID_CLIP_SOURCE',project=bad,input_root=str(sources),output_root=str(outputs),output=str(outputs/'bad.mkv'))
    bad = copy.deepcopy(native); bad['schema_version']=1
    call('render.plan',error='UNSUPPORTED_SCHEMA',project=bad,input_root=str(sources),output_root=str(outputs),output=str(outputs/'bad-schema.mkv'))
    bad = copy.deepcopy(native); bad['tracks']['tracks'][0]['clips'][0]['scene']['width']=15
    call('render.plan',error='INVALID_SCENE',project=bad,input_root=str(sources),output_root=str(outputs),output=str(outputs/'bad-size.mkv'))
    passed.append('generated.exclusive_source_version_and_geometry_validation')

    assert originals == {name:hashlib.sha256((sources/name).read_bytes()).hexdigest() for name in originals}
    report = dict(passed=passed,profile='quick',source_preservation=True)
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    return report


if __name__ == '__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--root','--output',dest='root',type=Path,required=True)
    args=parser.parse_args()
    print(json.dumps(run(args.root),indent=2))
