"""Saved working timelines: independent f64 storage, composition and final delivery equations."""
import argparse
import copy
import hashlib
import json
import math
from pathlib import Path
import subprocess

from PIL import Image
from engine import ENGINE
from scenes import identity
from tracks import edit, placement, time, track
from working_color import decode, encode, quantize, bayer

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 32, 24, 12
STRIDE = W * H * 3


class Context:
    def __init__(self, bits, light, transfer='srgb'):
        self.bits, self.light, self.transfer = bits, light, transfer
        self.q = 2**bits-1

    def store(self, p):
        # Stabilize exact half-code ties against f64 noise from rational frame-center weights.
        return [math.floor(max(0,min(1,v))*self.q+0.5+1e-10)/self.q for v in p]

    def source(self, rgba):
        rgb = [v/255 for v in rgba[:3]]
        if self.light == 'linear': rgb = [decode(v, self.transfer) for v in rgb]
        return self.store(rgb + [rgba[3]/255])

    def over(self, s, d, opacity=255):
        a = s[3] * quantize(opacity/255, self.q)/self.q
        alpha = a + d[3]*(1-a)
        return self.store([(s[c]*a+d[c]*d[3]*(1-a))/alpha for c in range(3)]+[alpha]) if alpha else [0]*4

    def grade(self, p, exposure):
        linear = p[:3] if self.light == 'linear' else [decode(v, self.transfer) for v in p[:3]]
        linear = [min(1, max(0, v*2**(exposure/1000))) for v in linear]
        rgb = linear if self.light == 'linear' else [encode(v, self.transfer) for v in linear]
        return self.store(rgb+p[3:])

    def output(self, pixels, transfer=None):
        transfer = transfer or self.transfer
        ranks = bayer(8)
        out = bytearray()
        for i,p in enumerate(pixels):
            rgb = p[:3]
            if self.light == 'linear': rgb = [encode(v, transfer) for v in rgb]
            elif transfer != self.transfer: rgb = [encode(decode(v,self.transfer),transfer) for v in rgb]
            threshold = (ranks[(i//W)%8][i%8]+0.5)/64
            out.extend(max(0,min(255,math.floor(v*255+threshold))) for v in rgb)
        return bytes(out)


def run(root):
    root = root.resolve(); assert root != ROOT and ROOT not in root.parents
    sources, outputs = root/'sources', root/'outputs'
    sources.mkdir(parents=True); outputs.mkdir()
    cases = []

    def ff(args):
        return subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args], check=True, capture_output=True, timeout=180).stdout

    def call(command, error=None, **fields):
        request = dict(command=command, **fields)
        result = subprocess.run([str(ENGINE)],input=json.dumps(request).encode(),capture_output=True,timeout=240)
        answer = json.loads(result.stdout)
        if error:
            assert result.returncode and answer['error']['code']==error, answer
            return answer['error']
        if result.returncode or not answer['ok']: (root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert result.returncode==0 and answer['ok'], answer
        return answer['result']

    def apply(p,ops): return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def decoded(path): return ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])
    pictures = [[[(x*7+n*11+s*43)%256,(y*9+n*3+s*17)%256,(x+y+n*17+s*97)%256,255]
                 for n in range(N) for y in range(H) for x in range(W)] for s in range(2)]
    assets=[]
    for s,pixels in enumerate(pictures):
        raw=sources/f'{s}.rgb'; raw.write_bytes(bytes(c for p in pixels for c in p[:3]))
        path=sources/f'{s}.mkv'
        ff(['-f','rawvideo','-pixel_format','rgb24','-video_size',f'{W}x{H}','-framerate','25','-i',str(raw),
            '-f','lavfi','-i','anullsrc=r=48000:cl=stereo','-t',str(N/25),'-map','0:v','-map','1:a',
            '-c:v','ffv1','-level','3','-slices','4','-pix_fmt','bgr0','-threads','1','-c:a','pcm_s16le',str(path)])
        assets.append(dict(id=str(s),path=str(path),duration=time(N,25)))
    rgba = [[(x*11+n*13)%256,(y*17+n*19)%256,(x+y+n*31)%256,(x*13+y*7+n*5)%256]
            for n in range(N) for y in range(H) for x in range(W)]
    frames=[]
    for n in range(N):
        path=sources/f'alpha-{n}.png'; image=Image.new('RGBA',(W,H)); image.putdata([tuple(p) for p in rgba[n*W*H:(n+1)*W*H]]); image.save(path)
        frames.append(dict(image=identity(path,sources),hold=time(1,25),offset=[0,0],anchor=[0,0]))
    scene=dict(schema_version=1,id='wide',width=W,height=H,output_scale=1,duration=time(N,25),transparent=True,
               background=[0,0,0],color='srgb_straight_encoded',audio=None,layers=[dict(id='layer',canvas=[W,H],
               start=time(0),duration=time(N,25),frames=frames,timing='strict',end='hold_last',
               transform=dict(position=[0,0],crop=[0,0,W,H],scale=1,quarter_turns=0,opacity=219))])
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    base=call('project.create',id='working',width=W,height=H,frame_rate=time(25))
    base=apply(base,[dict(op='media.add',asset=a) for a in assets]+[dict(op='project.transfer',transfer='srgb')])
    grade=dict(kind='grade',exposure_milli=431,contrast_milli=1000,white_balance_milli=[1000]*3)
    def layered(p):
        return apply(p,[edit('create',duration=time(N,25)),edit('add',track=track('v','video')),
            edit('place',track_id='v',clip=placement('base','0',0,N),collision='reject')])
    def render(p,name,expected,preview=False):
        path=outputs/(name+'.mkv'); call('render.run',project=p,input_root=str(sources),output_root=str(outputs),output=str(path))
        pixels=decoded(path); assert len(pixels)==len(expected),(name,len(pixels),len(expected))
        differences=[abs(a-b) for a,b in zip(pixels,expected)]
        maximum=max(differences,default=0)
        assert maximum<=1,(name,maximum,next((i,a,b) for i,(a,b) in enumerate(zip(pixels,expected)) if abs(a-b)>1))
        if preview:
            png=outputs/(name+'.png'); call('preview.frame',project=p,input_root=str(sources),output_root=str(outputs),output=str(png),time=time(3,25))
            assert Image.open(png).convert('RGB').tobytes()==pixels[3*STRIDE:4*STRIDE],name
            ranged=outputs/(name+'-range.mkv'); call('preview.range',project=p,input_root=str(sources),output_root=str(outputs),output=str(ranged),start=time(2,25),duration=time(4,25))
            assert decoded(ranged)==pixels[2*STRIDE:6*STRIDE],name
        cases.append(dict(name=name,frames=len(pixels)//STRIDE,maximum_delivery_error=maximum,sha256=hashlib.sha256(pixels).hexdigest()))
        return pixels

    # Old and explicitly selected default snapshots use exactly the same renderer.
    legacy=apply(base,[dict(op='clip.append',clip=dict(id='seq',asset_id='0',gap=False,source_in=time(0),duration=time(N,25)))])
    old=render(legacy,'legacy',bytes(c for p in pictures[0] for c in p[:3]),True)
    explicit=apply(legacy,[dict(op='project.working_space',working_space=dict(bits=8,light='encoded'))])
    assert explicit['schema_version']==2
    assert render(explicit,'explicit-default',old)==old
    cleared=apply(explicit,[dict(op='project.working_space',working_space=None)])
    assert 'working_space' not in cleared and render(cleared,'cleared-default',old)==old
    for bits,light in [(8,'linear'),(16,'encoded'),(16,'linear')]:
        ctx=Context(bits,light); label=f'{bits}-{light}'
        mode=dict(op='project.working_space',working_space=dict(bits=bits,light=light))
        p=apply(base,[mode]); assert p['schema_version']==2
        a,b=[[ctx.source(pixel) for pixel in source] for source in pictures]
        sequential=apply(legacy,[mode]); render(sequential,label+'-sequential',ctx.output(a),True)
        p=layered(p)
        overlay=apply(p,[edit('add',track=track('top','video')),edit('place',track_id='top',clip=placement('top','1',0,N),collision='reject'),
            edit('clip_effects',clip_id='top',effects=[grade,dict(kind='opacity',value=137)])])
        expected=[ctx.over(ctx.grade(s,431),d,137) for s,d in zip(b,a)]
        render(overlay,label+'-grade-opacity',ctx.output(expected),True)
        generated=apply(p,[edit('add',track=track('top','video')),edit('place',track_id='top',
            clip=dict(id='generated',scene=scene,start=time(0),source_in=time(0),duration=time(N,25)),collision='reject'),
            edit('clip_effects',clip_id='generated',effects=[grade,dict(kind='opacity',value=137)])])
        g=[ctx.over(ctx.source(s),[0]*4,219) for s in rgba]
        expected=[ctx.over(ctx.grade(s,431),d,137) for s,d in zip(g,a)]
        reference=render(generated,label+'-generated',ctx.output(expected),True)
        movie=outputs/(label+'-generated.mp4')
        delivered=call('export.run',project=generated,input_root=str(sources),output_root=str(outputs),output=str(movie),profile='h264_aac',streams='video')
        assert delivered['verification']['timeline_video_sha256']==hashlib.sha256(ctx.output(expected,'bt709')).hexdigest(), label
        png_movie=outputs/(label+'-generated.mov')
        call('export.run',project=generated,input_root=str(sources),output_root=str(outputs),output=str(png_movie),profile='png_mov',streams='video')
        assert decoded(png_movie)==reference
        # Define children through the public operation, then replace only root placement.
        child=generated['tracks']; nested=apply(base,[mode,dict(op='sequence.create',id='child',duration=time(N,25))])
        nested['sequences'][0]['arrangement']=copy.deepcopy(child)
        nested=apply(nested,[edit('create',duration=time(N,25)),edit('add',track=track('parent','video')),
            edit('place',track_id='parent',clip=dict(id='instance',sequence_id='child',start=time(0),source_in=time(0),duration=time(N,25)),collision='reject')])
        assert render(nested,label+'-nested',ctx.output(expected),True)==reference
        for kind in ['dissolve','dip_black','wipe_left','wipe_right']:
            transition=layered(apply(base,[mode]))
            transition['tracks']['tracks'][0]['clips']=[placement('left','0',0,6,0),placement('right','1',6,6,3)]
            transition=apply(transition,[edit('transition_set',track_id='v',transition=dict(id='join',left_id='left',right_id='right',before=time(2,25),after=time(2,25),kind=kind))])
            expected=[]
            for n in range(N):
                for i in range(W*H):
                    left=a[n*W*H+i]; right=b[(n-3)*W*H+i] if n>=4 else None
                    if n<4: value=left
                    elif n>=8: value=right
                    else:
                        weight=(2*(n-4)+1)/8
                        if kind=='dissolve': value=ctx.store([left[c]*(1-weight)+right[c]*weight for c in range(3)]+[1])
                        elif kind=='dip_black': value=ctx.store([left[c]*max(0,1-2*weight)+right[c]*max(0,2*weight-1) for c in range(3)]+[1])
                        else:
                            x=i%W if kind=='wipe_left' else W-1-i%W
                            value=right if (x+0.5)/W<=weight else left
                    expected.append(value)
            render(transition,label+'-'+kind,ctx.output(expected),kind=='dissolve')
        bad=copy.deepcopy(sequential); bad['schema_version']=1
        call('project.validate',error='UNSUPPORTED_SCHEMA',project=bad)
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    report=dict(passed=['working_timelines.saved_modes_and_default_identity','working_timelines.sequential_preview_and_range',
        'working_timelines.grade_opacity_generated_and_nested','working_timelines.exact_transition_clocks',
        'working_timelines.h264_single_transfer_and_lossless_png'],cases=cases,
        frames=sum(c['frames'] for c in cases),maximum_delivery_error=max(c['maximum_delivery_error'] for c in cases),
        reference='Independent f64 source transfer, declared-depth stores, exposure, coverage, frame-center transitions and Bayer delivery; tolerance one delivered RGB8 code',
        limits='Synthetic SDR8 inputs at 25 fps. Broad direct/proxy/queue/session, structural effects and final banding gates remain separate requirements.')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8'); print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser(); parser.add_argument('--output',type=Path,required=True); run(parser.parse_args().output)
