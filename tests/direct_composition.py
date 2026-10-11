"""Direct pictures inside the real compositor, compared with complete explicit conform."""
from copy import deepcopy
from fractions import Fraction as F
import json
import os
import shutil
import subprocess

from PIL import Image

from footage_sources import identity
from engine import ENGINE
from source_clocks import call, decoded, time


def verify(root, case, asset, client):
    name = asset['id']
    rate = F(case['rate'])
    width, height = case['size']
    out = root/'binding-previews'
    source = deepcopy(asset)
    source['path'] = 'sources/' + source['path']
    native = {**source, 'path': 'conform/' + case['name'] + '.mkv'}
    native.pop('decode')
    native['identity'] = identity(root/native['path'])
    # Separate asset IDs let an original play both as an opaque base and an overlay.
    def project(direct, effect='dissolve', mixed=False):
        sources = [dict(source if direct else native, id='base'),
                   dict(source if direct and not mixed else native, id='over')]
        def clip(id, asset_id, start, first, count):
            return dict(id=id, asset_id=asset_id, start=time(F(start)/rate),
                        source_in=time(F(first)/rate), duration=time(F(count)/rate))
        def track(id, clips, **extra):
            return dict(id=id, kind='video', enabled=True, locked=False, clips=clips, **extra)
        # Read across a GOP boundary, through a transition, crop/shrink an overlaid original,
        # and cover it briefly with a higher opaque native track.
        sources.append(dict(native, id='cover'))
        overlay = clip('overlay', 'over', 0, 20, 20)
        overlay['transform'] = dict(crop=[4, 2, width-8, height-4], divisor=2,
                                    opacity=137, position=[-2, 5])
        return dict(schema_version=1, id='direct-composition', revision=0, width=width,
                    height=height, frame_rate=time(rate), assets=sources, clips=[],
                    tracks=dict(duration=time(F(20)/rate), links=[], tracks=[
                        track('base', [clip('left','base',0,55,10), clip('right','base',10,75,10)],
                              transitions=[dict(id='mix', left_id='left', right_id='right',
                                                before=time(F(5)/rate), after=time(F(5)/rate), kind=effect)]),
                        track('over', [overlay], composite='alpha_over'),
                        track('cover', [clip('cover', 'cover', 8, 31, 4)])]))
    effects = ['dissolve','dip_black','wipe_left','wipe_right'] if case['name']=='h264-25' else ['dissolve']
    comparisons = []
    for effect in effects:
        for mixed in ([False, True] if case['name']=='h264-25' else [False]):
            p, reference = project(True,effect,mixed), project(False,effect,mixed)
            label = f"{case['name']}-{effect}-{'mixed' if mixed else 'direct'}"
            expected_path = out/(label+'-reference.mkv')
            actual_path = out/(label+'.mkv')
            for snapshot, target in [(reference,expected_path),(p,actual_path)]:
                receipt=call(dict(command='render.run',project=snapshot,input_root=str(root),
                                  output_root=str(out),output=str(target)))
                assert receipt['frames']==20
            expected = decoded(expected_path,True)
            assert decoded(actual_path,True)==expected, label
            assert decoded(actual_path,False)==decoded(expected_path,False), label
            size=width*height*3
            for frame in [0,6,10,13,19]:
                target=out/f'{label}-{frame}.png'
                client.call('preview.frame',project=p,input_root=str(root),output_root=str(out),
                            output=str(target),time=time(F(frame)/rate))
                with Image.open(target) as image:
                    assert image.convert('RGB').tobytes()==expected[frame*size:(frame+1)*size], (label,frame)
            comparisons.append(dict(effect=effect,mixed=mixed,frames=20,previews=[0,6,10,13,19]))
    assert identity(root/source['path'])==source['identity']
    return dict(comparisons=comparisons, source_preserved=True,
                audio='silence; direct audio timeline integration pending')


def failures(root, original, case):
    """A real direct decoder's late failure must cross the worker queue and prevent publication."""
    root.mkdir()
    original_identity = identity(original)
    source = root/'original-copy.mp4'
    shutil.copy2(original,source)
    helper = root/'direct-fault.exe'
    code = root/'direct-fault.rs'
    code.write_text(r'''
use std::{env, fs, io::{self, Read, Write}, process::{Command, Stdio, exit}};
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let real=env::var("DIRECT_REAL").unwrap();
    let mode=fs::read_to_string(env::var("DIRECT_MODE").unwrap()).unwrap_or_default();
    if mode.is_empty() || !args.iter().any(|a| a=="-copyts") || !args.iter().any(|a| a=="rawvideo") {
        exit(Command::new(real).args(&args).status().unwrap().code().unwrap_or(1));
    }
    let mut child=Command::new(real).args(&args).stdout(Stdio::piped()).spawn().unwrap();
    let mut bytes=Vec::new();
    child.stdout.take().unwrap().take(16*1024*1024+1).read_to_end(&mut bytes).unwrap();
    assert!(bytes.len()<=16*1024*1024 && !bytes.is_empty() && child.wait().unwrap().success());
    if mode=="short" { bytes.pop(); }
    if mode=="extra" { bytes.push(17); }
    io::stdout().write_all(&bytes).unwrap();
    if mode=="changed" {
        let path=env::var("DIRECT_SOURCE").unwrap();
        let mut bytes=fs::read(&path).unwrap(); let last=bytes.len()-1;bytes[last]^=1;fs::write(path,bytes).unwrap();
    }
    if mode=="failed" { exit(23); }
}
''',encoding='utf-8')
    subprocess.run(['rustc','--edition=2024',str(code),'-o',str(helper)],check=True,capture_output=True)
    mode=root/'mode'
    env={**os.environ,'CUTBOLT_FFMPEG':str(helper),'DIRECT_REAL':shutil.which('ffmpeg'),
         'DIRECT_MODE':str(mode),'DIRECT_SOURCE':str(source),'CUTBOLT_INSPECTION_CACHE':str(root/'cache')}
    def request(command, **args):
        p=subprocess.run([str(ENGINE)],input=json.dumps(dict(command=command,**args)),env=env,
                         capture_output=True,text=True,encoding='utf-8',timeout=90)
        return json.loads(p.stdout)
    asset=request('media.inspect',path=str(source),input_root=str(root))['result']['source_index']['asset']
    rate=F(case['rate'])
    project=dict(schema_version=1,id='decoder-failure',revision=0,width=case['size'][0],height=case['size'][1],
                 frame_rate=time(rate),assets=[asset],clips=[],tracks=dict(duration=time(F(20)/rate),links=[],
                 tracks=[dict(id='v',kind='video',enabled=True,locked=False,clips=[dict(id='shot',asset_id=asset['id'],
                              start=time(0),source_in=time(F(61)/rate),duration=time(F(20)/rate))])]))
    observed={}
    for label,expected in [('short','RENDER_VALIDATION_FAILED'),('extra','RENDER_VALIDATION_FAILED'),
                           ('failed','TOOL_FAILED'),('changed','MEDIA_CHANGED')]:
        mode.write_text(label)
        target=root/(label+'.mkv')
        result=request('render.run',project=project,input_root=str(root),output_root=str(root),output=str(target))
        assert not result['ok'] and result['error']['code']==expected, result
        assert not target.exists() and not list(root.glob('*.partial.mkv')), label
        observed[label]=expected
    assert identity(original)==original_identity
    return dict(errors=observed,no_failed_output_published=True,original_preserved=True)
