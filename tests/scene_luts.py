"""Ordered scene LUTs: original charts, rational tables and Decimal grade references."""
import argparse
import copy
from decimal import getcontext
from fractions import Fraction as F
import hashlib
import itertools
import json
from pathlib import Path
import subprocess

from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from compositing import expected_frame
from engine import ENGINE
from grading import D, changed, curve, decoding, encoding, primary, resolve
from keying import key, matte
from luts_scopes import reference, rounded
from scenes import identity, time
from spatial import reference as spatial_frame

ROOT = Path(__file__).resolve().parents[1]
W, H, N = 64, 8, 3
getcontext().prec = 60


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, outputs, oracle = [root / name for name in ('sources', 'outputs', 'oracle')]
    for path in (sources, outputs, oracle):
        path.mkdir()
    (root/'jobs').mkdir()
    passed, cases, tables = [], [], {}
    chart = Image.new('RGBA', (W, H))
    colors = [(0,255,0), (0,0,255), (255,0,0), (0,0,0), (255,255,255), (128,128,128)]
    chart.putdata([(*((x*4,)*3 if y == 0 else colors[x%6] if y == 1 else
                    ((x*7+y*29)%256, (x*13+y*11)%256, (x*31+y*5)%256)),
                   [255,255,1,17,64,128,254,0][y]) for y in range(H) for x in range(W)])
    chart.save(sources / 'chart.png')
    premult = Image.new('RGBA', chart.size)
    premult.putdata([(*(int(F(v*a,255)+F(1,2)) for v in (r,g,b)),a) for r,g,b,a in chart.getdata()])
    premult.save(sources / 'premult.png')

    def table(kind, size, mode, low=(0,0,0), high=(1,1,1)):
        name = f'{kind}-{size}-' + '-'.join(str(v).replace('/','_') for v in (*low,*high)) + '.cube'
        path = sources / name
        if not path.exists():
            dim = 1 if kind.endswith('1') else 3
            values = (reference([F(i,size-1)]*3,kind,size,'linear') for i in range(size)) if dim == 1 else (
                reference([F(r,size-1),F(g,size-1),F(b,size-1)],kind,size,'trilinear')
                for b,g,r in itertools.product(range(size), repeat=3))
            text = f'LUT_{dim}D_SIZE {size}\nDOMAIN_MIN ' + ' '.join(str(float(v)) for v in low)
            text += '\nDOMAIN_MAX ' + ' '.join(str(float(v)) for v in high) + '\n'
            path.write_text(text + ''.join(' '.join(str(float(v)) for v in row)+'\n' for row in values), encoding='ascii')
        tables[name] = (kind,size,mode,low,high)
        return {'kind':'lut','file':identity(path,sources),'interpolation':mode}

    def scene(effects, path='chart.png'):
        return {'schema_version':1,'id':'lut-scene','width':W,'height':H,'output_scale':1,
                'duration':time(N,25),'background':[23,57,101],'color':'srgb_straight_encoded',
                'layers':[{'id':'chart','canvas':[W,H],'start':time(0),'duration':time(N,25),
                           'frames':[{'image':identity(sources/path,sources),'hold':time(N,25),'offset':[0,0],'anchor':[0,0]}],
                           'timing':'strict','end':'hold_last','effects':effects,
                           'transform':{'position':[0,0],'crop':[0,0,W,H],'scale':1,'quarter_turns':0,'opacity':255}}], 'audio':None}

    def call(value, error=None):
        result = subprocess.run([str(ENGINE)], input=json.dumps(value).encode(), capture_output=True, timeout=180)
        answer = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and answer['error']['code'] == error, (error,answer)
            return answer['error']
        assert result.returncode == 0 and answer['ok'], answer
        return answer['result']

    def pixel(p, effects, premultiplied):
        if not p[3]:
            return (0,0,0,0)
        encoded = [F(v,p[3] if premultiplied else 255) for v in p[:3]]
        alpha = F(p[3])
        for effect in effects:
            if effect['kind'] == 'lut':
                kind,size,_,low,high = tables[effect['file']['path']]
                encoded = [max(F(0),min(F(1),v)) for v in reference(encoded,kind,size,effect['interpolation'],low,high)]
            elif effect['kind'] == 'grade':
                encoded = [F(encoding(primary(decoding(D(v.numerator)/v.denominator),c,effect))) for c,v in enumerate(encoded)]
            else:
                # These cases use a hard/soft key without unmix/spill. Only coverage changes.
                alpha *= 1-F(effect['strength_milli'],1000)*(1-matte(tuple(rounded(v*255) for v in encoded),effect))
        return (*[rounded(v*255) for v in encoded],rounded(alpha))

    def expected(value, n, name):
        ref = copy.deepcopy(value)
        for i, layer in enumerate(ref['layers']):
            image = Image.open(sources/layer['frames'][0]['image']['path']).convert('RGBA')
            effects = [resolve(e,F(n,25)) if e['kind']=='grade' else e for e in layer.pop('effects')]
            image.putdata([pixel(p,effects,layer.get('alpha_mode')=='premultiplied') for p in image.getdata()])
            path = oracle / f'{name}-{n}-{i}.png'
            image.save(path)
            layer['frames'][0]['image'] = identity(path,oracle)
            layer['alpha_mode'] = 'straight'
        return spatial_frame(ref,oracle,n) if any('spatial' in layer['transform'] for layer in ref['layers']) else expected_frame(ref,oracle,n)

    def verify(value, name):
        output = outputs/(name+'.mkv')
        report = call({'command':'scene.render','scene':value,'input_root':str(sources),
                       'output_root':str(outputs),'output':str(output)})
        raw = subprocess.check_output(['ffmpeg','-v','error','-i',str(output),'-an','-pix_fmt','rgb24','-f','rawvideo','-'], timeout=90)
        assert len(raw) == N*W*H*3
        error = max(abs(a-b) for n in range(N) for a,b in zip(raw[n*W*H*3:(n+1)*W*H*3],expected(value,n,name)))
        assert error <= 1, (name,error)
        cases.append({'case':name,'frames':N,'maximum_rgb_error':error,'decoded_sha256':hashlib.sha256(raw).hexdigest()})
        return report,raw

    for kind,size in [('identity1',2),('curve1',3),('identity3',5),('cross3',5),('clip3',5)]:
        for mode in (('nearest','linear') if kind.endswith('1') else ('nearest','trilinear','tetrahedral')):
            verify(scene([table(kind,size,mode)]),f'{kind}-{mode}')
    verify(scene([table('cross3',5,'tetrahedral',(F(1,4),0,0),(F(3,4),1,1))]),'domain-clamp')
    passed.append('scene_luts.rational_interpolation_and_clipping')

    lut = table('cross3',5,'tetrahedral')
    grade = changed(exposure_milli=733,contrast_milli=1190,white_balance_milli=[900,1100,1300])
    _,before = verify(scene([grade,lut]),'grade-lut')
    _,after = verify(scene([lut,grade]),'lut-grade')
    assert before != after
    verify(scene([grade,lut,changed(exposure_milli=-417)]),'grade-lut-grade')
    verify(scene([table('curve1',3,'linear'),lut]),'lut-lut')
    animated = changed(animation={'exposure_milli':curve([(0,1,-417,'linear'),(2,25,733,'hold')])})
    verify(scene([animated,lut]),'animated-grade-lut')
    premultiplied = scene([lut,grade],'premult.png')
    premultiplied['layers'][0]['alpha_mode'] = 'premultiplied'
    verify(premultiplied,'premultiplied')
    verify(scene([key(inner_milli=100,outer_milli=100),lut]),'key-lut')
    verify(scene([table('identity1',2,'linear'),key()]),'lut-key')
    passed.append('scene_luts.ordered_float_chains_animation_and_alpha')

    multi = scene([lut])
    moving = copy.deepcopy(multi['layers'][0])
    moving['id'] = 'moving'
    moving['animation'] = {'position_x':curve([(0,1,-11,'linear'),(2,25,11,'hold')])}
    multi['layers'].append(moving)
    report,_ = verify(multi,'shared-static-moving')
    assert len(report['effect_resources']['lut_tables']) == 1
    assert 0 < report['effect_resources']['allocated_table_bytes'] <= 128*1024*1024
    assert report['effect_resources']['maximum_table_bytes'] == 128*1024*1024
    passed.append('scene_luts.shared_tables_static_and_moving_layers')

    spatial = scene([grade,lut])
    spatial['layers'][0]['transform']['spatial'] = {
        'translate_milli':[250,-500],'scale_milli':[900,1100],'rotation_mdeg':11000,
        'flip':[True,False],'pixel_aspect':time(1),'sampling':'bilinear','edge':'transparent'}
    verify(spatial,'lut-before-spatial-filter')
    passed.append('scene_luts.effects_before_bilinear_spatial_filter')

    value = scene([lut,grade])
    client = Client(ENGINE)
    try:
        client.initialize()
        listing = client.rpc('tools/list')['result']['tools']
        schema = next(t['inputSchema'] for t in listing if t['name']=='cutbolt_scene_inspect')
        Draft202012Validator(schema).validate({'scene':value,'input_root':str(sources)})
        shorthand = copy.deepcopy(value)
        shorthand['layers'][0]['effects'][0]['file'] = {'path':lut['file']['path']}
        result = client.call('scene.inspect',scene=shorthand,input_root=str(sources),detail='full')
        assert result['effect_resources']['lut_tables'][0]['file'] == lut['file']
        ticket = client.call('job.start',job_root=str(root/'jobs'),request_id='lut-render',run='scene.render',
                             arguments={'scene':shorthand,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/'queued.mkv')})
        waited = client.call('job.wait',job_root=str(root/'jobs'),job_id=ticket['job_id'],timeout_seconds=120,detail='full')
        assert waited['status'] == 'completed', waited
        assert waited['result']['effect_resources']['lut_tables'][0]['file'] == lut['file']
        queued = subprocess.check_output(['ffmpeg','-v','error','-i',str(outputs/'queued.mkv'),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=90)
        assert queued == after
    finally:
        client.close()
    passed.append('scene_luts.mcp_schema_path_identity_and_queued_output')

    hashes = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    output_hashes = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in outputs.iterdir()}
    rejected = []
    for field,bad,code in [('path','../outside.cube','INVALID_PATH'),('bytes',8388609,'INVALID_LUT'),
                           ('sha256','0'*64,'MEDIA_CHANGED'),('path','absent.cube','IO_ERROR')]:
        invalid = copy.deepcopy(value)
        invalid['layers'][0]['effects'][0]['file'][field] = bad
        rejected.append((invalid,code))
    invalid = scene([dict(lut,interpolation='linear')]); rejected.append((invalid,'INVALID_LUT'))
    invalid = scene([lut]*9); rejected.append((invalid,'INVALID_EFFECT'))
    invalid = scene([lut,dict(lut,file=dict(lut['file'],sha256='0'*64))]); rejected.append((invalid,'INVALID_IDENTITY'))
    for n,(invalid,code) in enumerate(rejected):
        call({'command':'scene.render','scene':invalid,'input_root':str(sources),'output_root':str(outputs),'output':str(outputs/f'reject-{n}.mkv')},code)
    assert hashes == {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert output_hashes == {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in outputs.iterdir()}
    assert not list(outputs.glob('.cutbolt-scene-*'))
    passed.append('scene_luts.validation_and_source_preservation')
    report = {'passed':passed,'render_cases':cases,'decoded_frames':N*(len(cases)+1),
              'rgb_tolerance':1,'maximum_rgb_error':max(c['maximum_rgb_error'] for c in cases),
              'rejected_cases':len(rejected),'reference':'Exact Fraction analytic LUT interpolation and 60-digit Decimal grading, independent forward alpha composition',
              'scope':'Scene LUT prerequisite for clip effects; no native clip effects or stage 3 completion claim'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
