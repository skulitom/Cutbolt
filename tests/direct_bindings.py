"""Saved original-source bindings and actual frame previews, independently compared with conform."""
from copy import deepcopy
from fractions import Fraction as F
import os
import shutil

from PIL import Image

from agents import Client
from engine import ENGINE
from footage_sources import identity
from source_clocks import call, decoded, time


def verify(root, manifest, indexed):
    cache = root/'binding-cache'
    output = root/'binding-previews'
    output.mkdir()
    client = Client(ENGINE, env={**os.environ, 'CUTBOLT_INSPECTION_CACHE':str(cache)})
    client.initialize()
    cases = {}
    try:
        for name, case in manifest['cases'].items():
            if name == 'h264-20min':
                continue
            if not indexed[name]['index']['eligible']:
                rejected = client.call('project.create', id=name, width=case['size'][0], height=case['size'][1], frame_rate=time(25))
                client.call('timeline.apply', expected_error='UNSUPPORTED_MEDIA', project=rejected, expected_revision=0,
                    operations=[{'op':'media.add','input_root':str(root/'sources'),
                        'asset':{'id':name,'path':case['path'],'duration':time(1)}}])
                continue
            path = root/'sources'/case['path']
            inspected = client.call('media.inspect', path=str(path), input_root=str(root/'sources'))
            assert not inspected['timeline']['ready']  # full timeline rendering still pending
            source = inspected['source_index']
            assert source['frame_preview_available'] and not source['direct_decode_available']
            asset = source['asset']
            assert asset['identity'] == identity(path)
            assert asset['decode'] == {'mode':'direct', 'index':source['key']}
            rate, count = F(case['rate']), source['frames']
            project = client.call('project.create', id=name, width=case['size'][0], height=case['size'][1], frame_rate=time(rate))
            # Half the sources enter through an already-bound inspect result; half through
            # media.add's explicit input root, which fills both identity and decode itself.
            if len(cases) % 2:
                unbound = {k:v for k,v in asset.items() if k not in ('identity','decode')}
                add = {'op':'media.add', 'asset':unbound, 'input_root':str(root/'sources')}
            else:
                add = {'op':'media.add', 'asset':asset}
            project = client.call('timeline.apply', project=project, expected_revision=0, operations=[add,
                {'op':'clip.append','clip':{'id':'shot','asset_id':asset['id'],'source_in':time(0),'duration':asset['duration']}}])
            assert project['schema_version'] == 1 and project['assets'][0] == asset
            tracked = client.call('timeline.apply', project=project, expected_revision=1,
                                  operations=[{'op':'tracks.edit','edit':{'op':'promote','video_track_id':'v','audio_track_id':'a'}}])
            reference = decoded(root/'conform'/(name+'.mkv'), True)
            size = case['size'][0]*case['size'][1]*3
            selected = sorted({0, case['gop'], case['gop']+1, count-1})
            for layout, snapshot in [('sequential',project),('tracks',tracked)]:
                for frame in selected:
                    target = output/f'{name}-{layout}-{frame}.png'
                    receipt = client.call('preview.frame', project=snapshot, input_root=str(root/'sources'),
                        output_root=str(output), output=str(target), time=time(F(frame)/rate))
                    assert receipt['source_frame'] == frame and receipt['decode']['index'] == asset['decode']['index']
                    assert receipt['decode']['transfer'] == 'bt709'
                    with Image.open(target) as image:
                        assert image.convert('RGB').tobytes() == reference[frame*size:(frame+1)*size], (name,layout,frame)
            assert identity(path) == asset['identity']
            cases[name] = {'sequential_frames':selected, 'track_frames':selected, 'source_preserved':True}
            if name == 'h264-25':
                cases['binding_lifecycle'] = lifecycle(client, root, project, cache, output)
        return cases
    finally:
        client.close()


def lifecycle(client, root, project, cache, output):
    sources = root/'sources'
    asset = project['assets'][0]
    key = asset['decode']['index']
    def check(snapshot):
        return client.call('timeline.check', project=snapshot, input_root=str(sources))
    assert not [f for f in check(project)['findings']['listed'] if f['kind']=='direct_index']
    entry = cache/(key+'.index.json.gz')
    entry.unlink()
    missing = [f for f in check(project)['findings']['listed'] if f['kind']=='direct_index']
    assert len(missing)==1 and missing[0]['index']['status']=='missing'
    assert not entry.exists()  # timeline.check does not silently rebuild
    client.call('preview.frame', project=project, input_root=str(sources), output_root=str(output),
                output=str(output/'rebuilt.png'), time=time(0))
    assert entry.exists()
    stale = deepcopy(project)
    stale['assets'][0]['decode']['index'] = '0'*64
    stale_findings = [f for f in check(stale)['findings']['listed'] if f['kind']=='direct_index']
    assert stale_findings[0]['index']['status']=='stale'
    denied = output/'stale.png'
    error = client.call('preview.frame', expected_error='DIRECT_INDEX_STALE', project=stale,
        input_root=str(sources), output_root=str(output), output=str(denied), time=time(0))['error']
    assert error['path']=='asset.decode.index' and 'media.index' in error['hint'] and not denied.exists()
    for field, value in [('width', project['width']+2),('frame_rate',time(30))]:
        wrong = deepcopy(project)
        wrong[field] = value
        client.call('preview.frame', expected_error='UNSUPPORTED_MEDIA', project=wrong,
            input_root=str(sources), output_root=str(output), output=str(denied), time=time(0))
    wrong = deepcopy(project)
    wrong['assets'][0]['duration'] = time(100)
    client.call('preview.frame', expected_error='INVALID_MEDIA', project=wrong,
        input_root=str(sources), output_root=str(output), output=str(denied), time=time(0))
    # A declared working transfer reaches the converter; compare it with the unchanged f64 conform path.
    srgb_reference = output/'srgb-reference.mkv'
    recipe = {'schema_version':1,'id':'srgb-reference','source':{'file':{'path':asset['path'],**asset['identity']},
        'sdr':{'matrix':'bt709','range':'limited','transfer':'bt709','missing_tags':'reject'},'clock':'first_video_frame'},
        'source_in':time(0),'duration':asset['duration'],'rate':time(1),'reverse':False,'freeze':False,
        'width':project['width'],'height':project['height'],'audio':'resample','working_transfer':'srgb',
        'frame_rate':project['frame_rate']}
    call({'command':'media.conform','recipe':recipe,'input_root':str(sources),'output_root':str(output),'output':str(srgb_reference)})
    srgb = client.call('timeline.apply',project=project,expected_revision=project['revision'],
        operations=[{'op':'project.transfer','transfer':'srgb'}])
    changed_color = output/'srgb-preview.png'
    receipt=client.call('preview.frame', project=srgb,input_root=str(sources),output_root=str(output),output=str(changed_color),time=time(F(61,25)))
    assert receipt['decode']['transfer']=='srgb'
    pixels=decoded(srgb_reference,True); size=project['width']*project['height']*3
    with Image.open(changed_color) as image: assert image.convert('RGB').tobytes()==pixels[61*size:62*size]
    # Refresh, diff, undo and replay cover the new property with no new project schema version.
    store = root/'binding-sessions'
    store.mkdir()
    stale['revision']=0
    saved = dict(store_root=str(store), project_id=stale['id'])
    client.call('session.create', store_root=str(store), project=stale, request_id='create')
    request = dict(**saved, expected_revision=0, request_id='refresh',
        operations=[stale_findings[0]['repair']])
    receipt = client.call('session.apply', **request)
    assert receipt['changes']['modified_assets']==[asset['id']]
    refreshed = client.call('session.get', **saved)
    assert refreshed['assets'][0]['decode']==asset['decode']
    client.call('session.undo', **saved, expected_revision=1, request_id='undo')
    assert client.call('session.get', **saved)['assets'][0]['decode']['index']=='0'*64
    assert client.call('session.apply', **request)==receipt
    # Adding with source I/O occurs only after the committed-request replay check.
    copy = sources/'retry-source.mp4'
    shutil.copy2(sources/asset['path'], copy)
    retry = deepcopy(project)
    retry.update(id='retry-binding',revision=0,assets=[],clips=[])
    client.call('session.create', store_root=str(store),project=retry,request_id='create')
    addition = {'id':'copy','path':copy.name,'duration':asset['duration']}
    request = dict(store_root=str(store),project_id='retry-binding',expected_revision=0,request_id='add',
        operations=[{'op':'media.add','asset':addition,'input_root':str(sources)}])
    receipt = client.call('session.apply', **request)
    pinned = client.call('session.get', store_root=str(store),project_id='retry-binding')['assets'][0]
    assert pinned['identity']==identity(copy) and pinned['decode']['mode']=='direct'
    data=bytearray(copy.read_bytes()); data[-1]^=1; copy.write_bytes(data)
    assert client.call('session.apply', **request)==receipt
    client.call('session.apply', expected_error='MEDIA_CHANGED',store_root=str(store),project_id='retry-binding',
        expected_revision=1,request_id='reject-refresh',operations=[{'op':'media.index','asset_id':'copy','input_root':str(sources)}])
    assert client.call('session.get',store_root=str(store),project_id='retry-binding')['revision']==1
    failed_preview = deepcopy(project); failed_preview['assets'][0]=pinned; failed_preview['clips'][0]['asset_id']='copy'
    client.call('preview.frame', expected_error='MEDIA_CHANGED',project=failed_preview,
        input_root=str(sources),output_root=str(output),output=str(denied),time=time(0))
    assert not denied.exists()
    return {'missing_index_reported_and_rebuilt':True,'stale_index_rejected_and_refreshable':True,
            'saved_diff_undo_and_replay':True,'add_replay_preserves_first_identity':True,
            'changed_original_rejected_before_publication':True,'dimension_rate_duration_rejected':True,'declared_srgb_matches_f64_conform':True}
