"""Ordered native layers: scene oracles, independent integer alpha and source-window evidence."""
import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from PIL import Image
from engine import ENGINE
from keying import key
from scenes import identity
from tracks import edit, placement, time, track
from sequences import nested, seqedit

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 24, 12
STRIDE = W * H * 3


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, outputs = root/'sources', root/'outputs'
    sources.mkdir(); outputs.mkdir()
    cases = []

    def ff(args):
        return subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-n', *args], check=True, capture_output=True, timeout=180).stdout

    def call(command, **fields):
        request = {'command': command, **fields}
        result = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, timeout=240)
        answer = json.loads(result.stdout)
        if result.returncode or not answer['ok']:
            (root/'failed-request.json').write_text(json.dumps(request, indent=2))
        assert result.returncode == 0 and answer['ok'], answer
        return answer['result']

    def apply(project, ops):
        return call('timeline.apply', project=project, expected_revision=project['revision'], operations=ops)

    def decoded(path):
        return ff(['-i', str(path), '-an', '-pix_fmt', 'rgb24', '-f', 'rawvideo', '-'])

    raw = {
        'background': bytes(c for n in range(N) for y in range(H) for x in range(W) for c in ((x*7+n*11)%256, (y*9+n*3)%256, (x+y+n*17)%256)),
        'presenter': bytes(c for n in range(N) for y in range(H) for x in range(W) for c in ((0,255,0) if x < W//2 else (220+n*2,15+y,30+x))),
    }
    (sources/'silence.pcm').write_bytes(bytes(N*1920*4))
    media, pictures = {}, {}
    for name, pixels in list(raw.items()):
        (sources/(name+'.rgb')).write_bytes(pixels)
        path = sources/(name+'.mkv')
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(sources/(name+'.rgb')),
            '-f','s16le','-ar','48000','-ac','2','-i',str(sources/'silence.pcm'),'-map','0:v','-map','1:a',
            '-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le',
            '-colorspace','rgb','-color_range','pc','-color_primaries','bt709','-color_trc','iec61966-2-1',str(path)])
        assert decoded(path) == pixels
        phone = sources/(name+'.mp4')
        ff(['-i',str(path),'-vf','scale=in_range=pc:out_range=tv:out_color_matrix=bt709','-c:v','libx264','-crf','18','-g','8','-bf','2',
            '-pix_fmt','yuv420p','-c:a','aac','-ar','48000','-color_range','tv','-colorspace','bt709','-color_primaries','bt709','-color_trc','bt709',str(phone)])
        reference = sources/(name+'-reference.mkv')
        call('media.conform',input_root=str(sources),output_root=str(sources),output=str(reference),recipe={
            'schema_version':1,'id':'reference-'+name,'source':{'file':identity(phone,sources),'sdr':{'matrix':'bt709','range':'limited','transfer':'bt709','missing_tags':'reject'},'clock':'first_video_frame'},
            'source_in':time(0),'duration':time(N,25),'rate':time(1),'reverse':False,'freeze':False,'width':W,'height':H,'audio':'resample','working_transfer':'srgb','frame_rate':time(25)})
        for codec, file, rgb in [('ffv1',path,pixels),('phone',phone,decoded(reference))]:
            media[codec,name] = file
            pictures[codec,name] = []
            for n in range(N):
                image = sources/f'{codec}-{name}-{n}.png'
                Image.frombytes('RGB',(W,H),rgb[n*STRIDE:(n+1)*STRIDE]).save(image)
                pictures[codec,name].append({'image':identity(image,sources),'hold':time(1,25),'offset':[0,0],'anchor':[0,0]})
            if codec == 'phone': raw['phone-'+name] = rgb
    original_hashes = {p:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def project(codec):
        p = call('project.create',id='layers-'+codec,width=W,height=H,frame_rate=time(25))
        ops = [{'op':'media.add','input_root':str(sources),'asset':{'id':name,'path':str(media[codec,name]),'duration':time(N,25)}} for name in ('background','presenter')]
        ops += [{'op':'project.transfer','transfer':'srgb'}, edit('create',duration=time(N,25))]
        for name in ('background','presenter'):
            ops += [edit('add',track=track(name,'video')),edit('place',track_id=name,clip=placement(name,name,0,N),collision='reject')]
        return apply(p,ops)

    def scene(codec, effect, name):
        recipe = {'schema_version':1,'id':name,'width':W,'height':H,'output_scale':1,'duration':time(N,25),'background':[0,0,0],'color':'srgb_straight_encoded','audio':None,'layers':[]}
        for source in ('background','presenter'):
            recipe['layers'].append({'id':source,'canvas':[W,H],'start':time(0),'duration':time(N,25),'frames':pictures[codec,source],
                'timing':'strict','end':'hold_last','effects':[] if source=='background' else [effect],
                'transform':{'position':[0,0],'crop':[0,0,W,H],'scale':1,'quarter_turns':0,'opacity':255}})
        path=outputs/(name+'-scene.mkv')
        call('scene.render',scene=recipe,input_root=str(sources),output_root=str(outputs),output=str(path))
        return decoded(path)

    def render(p, name, expected):
        path=outputs/(name+'.mkv')
        call('render.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(path))
        actual=decoded(path)
        assert actual==expected,(name,len(actual),len(expected),max(abs(a-b) for a,b in zip(actual,expected)))
        cases.append({'case':name,'frames':len(actual)//STRIDE,'rgb_sha256':hashlib.sha256(actual).hexdigest()})
        print('verified '+name,flush=True)

    for codec in ('ffv1','phone'):
        base=project(codec)
        if codec=='phone': assert all(a['decode']['mode']=='direct' for a in base['assets'])
        for strength in (1000,500):
            effect=key(strength_milli=strength)
            p=apply(base,[edit('clip_effects',clip_id='presenter',effects=[effect])])
            expected=scene(codec,effect,f'{codec}-key-{strength}')
            if codec=='ffv1':
                independent=bytearray()
                alpha=(255*(1000-strength)+500)//1000
                for i,(s,d) in enumerate(zip(raw['presenter'],raw['background'])):
                    a=alpha if (i//3)%W<W//2 else 255
                    independent.append((s*a+d*(255-a)+127)//255)
                assert expected==independent,'Scene disagrees with independent integer alpha equation'
            render(p,f'{codec}-key-{strength}',expected)
            preview=outputs/f'{codec}-{strength}-preview.png'
            call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(preview),time=time(5,25))
            assert Image.open(preview).convert('RGB').tobytes()==expected[5*STRIDE:6*STRIDE]
            ranged=outputs/f'{codec}-{strength}-range.mkv'
            call('export.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(ranged),profile='reference',streams='video',range={'start':time(3,25),'duration':time(4,25)})
            assert decoded(ranged)==expected[3*STRIDE:7*STRIDE]
        # A full opaque cover omits the lower decoder, including before/after window splits.
        plan=call('render.plan',project=base,input_root=str(sources),output_root=str(outputs),output=str(outputs/f'{codec}-plan.mkv'))
        assert [c['clip_id'] for c in plan['compositor']['stack']['clips']]==['presenter']
        partial=apply(base,[edit('remove',clip_ids=['presenter'],links='include'),edit('place',track_id='presenter',clip=placement('presenter','presenter',3,4,3),collision='reject')])
        plan=call('render.plan',project=partial,input_root=str(sources),output_root=str(outputs),output=str(outputs/f'{codec}-partial-plan.mkv'))
        assert [(c['first'],c['end']) for c in plan['compositor']['stack']['clips'] if c['clip_id']=='background']==[(0,3),(7,N)]
        prefix='' if codec=='ffv1' else 'phone-'
        render(partial,f'{codec}-opaque-windows',raw[prefix+'background'][:3*STRIDE]+raw[prefix+'presenter'][3*STRIDE:7*STRIDE]+raw[prefix+'background'][7*STRIDE:])
        # Child gaps stay opaque black; direct child pictures use the same stream implementation.
        child=apply(base,[edit('remove',clip_ids=['presenter'],links='include'),{'op':'sequence.create','id':'shot','duration':time(N,25)},
            seqedit('shot',edit('add',track=track('v','video'))),
            seqedit('shot',edit('place',track_id='v',clip=placement('child','presenter',2,N-4,2),collision='reject')),
            edit('place',track_id='presenter',clip=nested('instance','shot',0,N),collision='reject')])
        render(child,f'{codec}-nested',bytes(2*STRIDE)+raw[prefix+'presenter'][2*STRIDE:(N-2)*STRIDE]+bytes(2*STRIDE))
    assert all(hashlib.sha256(p.read_bytes()).hexdigest()==digest for p,digest in original_hashes.items())
    report={'passed':['native_layers.key_retains_lower_tracks','native_layers.scene_and_integer_oracles','native_layers.preview_and_range','native_layers.opaque_cover_omits_hidden_decoder_windows','native_layers.nested_direct_frames_and_black_gaps','native_layers.source_preservation'],
            'cases':cases,'rgb_tolerance':0,'scope':'Ordered stack, retained key alpha and native child sources; spatial clip property acceptance remains separate.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
