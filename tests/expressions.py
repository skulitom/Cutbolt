"""Original closed-form property fixtures and independent complete pixel references.

This fixture deliberately does not implement an expression interpreter. Its
authored graphs have explicit Fraction formulae, fixed truth tables and hash
vectors, then use the existing forward pixel/geometry references.
"""
from engine import ENGINE, MCP_TOOLS, per_frame
import argparse
import budgets
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import random
import subprocess
import time as clock
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time, expected_frame
from spatial import reference as spatial_reference

ROOT = Path(__file__).resolve().parents[1]


def number(n, d=1):
    v = F(n, d)
    return {'num': v.numerator, 'den': v.denominator}


def value(v, kind='scalar'):
    return {'type': kind, 'value': [number(x) for x in v] if kind == 'vector2' else bool(v) if kind == 'boolean' else number(v)}


def node(label, kind='scalar', op='literal', **fields):
    return {'id': label, 'kind': kind, 'expression': {'op': op, **fields}}


def constant(label, v, kind='scalar'):
    return node(label, kind, value=value(v, kind))


def rounded(v):
    return int(abs(v) + F(1, 2)) * (-1 if v < 0 else 1)


def random_value(seed, index, stream=7):
    # Normative byte contract; compared against independent fixed digest vectors.
    data = b'cutbolt-property-seed-v1\0' + seed.to_bytes(8, 'little') + stream.to_bytes(4, 'little') + index.to_bytes(4, 'little')
    return F(int.from_bytes(hashlib.sha256(data).digest()[:4], 'little'), 2**32)


def expected(t, seed=17):
    r = random_value(seed, int(10*t))
    pos = [F(rounded(3+5*t)) + 5*t + int(3*r), F(3)]
    opacity = 255-100*t
    return {'A': (pos, opacity), 'B': ([pos[0]-4, pos[1]+10], opacity/2)}


def resolved(scene, n):
    s = copy.deepcopy(scene)
    result = expected(F(n, 25), s.pop('expressions')['seed'])
    for layer in s['layers']:
        layer.pop('animation', None)
        p, a = result[layer['id']]
        layer['transform'].update(position=list(map(rounded, p)), opacity=rounded(a))
    return s


def graph():
    nodes = [constant('zero', 0), constant('two', 2), constant('three', 3), constant('five', 5),
             constant('ten', 10), constant('hundred', 100), constant('opaque', 255), constant('offset', [-4, 10], 'vector2'),
             node('t', op='time'), node('local_b', op='time', layer='B'),
             node('base_a', 'vector2', 'base', layer='A', property='position'),
             node('phase', op='multiply', a='t', b='ten'), node('index', op='floor', value='phase'),
             node('random', op='seeded', stream=7, index='index'), node('jitter', op='multiply', a='random', b='three'),
             node('step', op='floor', value='jitter'), node('motion', op='multiply', a='t', b='five'),
             node('dx', op='add', a='motion', b='step'), node('shift', 'vector2', 'vector', x='dx', y='zero'),
             node('pos_a', 'vector2', 'add', a='base_a', b='shift'),
             node('fade', op='multiply', a='t', b='hundred'), node('alpha_a', op='subtract', a='opaque', b='fade'),
             node('linked_position', 'vector2', 'property', layer='A', property='position'),
             node('pos_b', 'vector2', 'add', a='linked_position', b='offset'),
             node('linked_alpha', op='property', layer='A', property='opacity'),
             node('alpha_b', op='divide', a='linked_alpha', b='two')]
    return {'schema_version': 1, 'seed': 17, 'nodes': nodes, 'bindings': [
        {'layer': layer, 'property': prop, 'node': label}
        for layer, prop, label in [('A','position','pos_a'), ('A','opacity','alpha_a'), ('B','position','pos_b'), ('B','opacity','alpha_b')]]}


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output, store = [root/n for n in ('sources', 'output', 'store')]
    for p in (sources, output, store):
        p.mkdir()
    for k in range(2):
        im = Image.new('RGBA', (8, 6))
        im.putdata([((x*27+y*11+k*19)%256, (y*39+x*13+k*17)%256, (201-x*9+k*31)%256,
                     [0,85,170,255][(x+y+k)%4]) for y in range(6) for x in range(8)])
        im.save(sources/f'original-{k}.png')
    before = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    a = {'id':'A', 'canvas':[8,6], 'start':time(0), 'duration':time(2),
         'frames':[{'image':identity(sources/f'original-{k}.png',sources), 'hold':time(1,5), 'offset':[0,0], 'anchor':[0,0]} for k in range(2)],
         'timing':'strict', 'end':'loop', 'transform':{'position':[3,3], 'crop':[0,0,8,6], 'scale':1, 'quarter_turns':0, 'opacity':255},
         'animation':{'position_x':{'keys':[{'time':time(0),'value':3,'interpolation':'linear'}, {'time':time(2),'value':13,'interpolation':'hold'}]}}}
    b = copy.deepcopy(a)
    b.update(id='B',start=time(2,5),duration=time(6,5))
    b.pop('animation')
    scene = {'schema_version':1,'id':'property-graph','width':64,'height':48,'output_scale':1,'duration':time(2),
             'background':[7,11,19],'color':'srgb_straight_encoded','audio':None,'layers':[a,b],'expressions':graph()}
    exe = ENGINE
    passed = []; rejected = 0; frames = 0; samples = 0; previews = 0

    def call(command, error=None, **fields):
        nonlocal rejected
        request = {'command':command, **fields}
        result = subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=180)
        reply = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and reply['error']['code'] == error, (error, reply)
            rejected += 1
            return reply['error']
        if result.returncode:
            (root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert result.returncode == 0 and reply['ok'], reply
        return reply['result']

    def inspect(s, times, error=None):
        return call('expression.inspect',error,scene=s,times=[number(t) for t in times])

    def ff(path, kind):
        args = ['-an','-pix_fmt','rgb24','-f','rawvideo'] if kind == 'video' else ['-vn','-f','s16le']
        return subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),*args,'-'],timeout=120)

    def check_media(path, wanted, count):
        nonlocal frames, samples
        assert ff(path,'video') == b''.join(wanted), str(path)
        assert ff(path,'audio') == bytes(count*1920*4), str(path)
        frames += count; samples += count*1920

    def render(s, name, oracle=None):
        result = call('scene.render',scene=s,input_root=str(sources),output_root=str(output),output=str(output/(name+'.mkv')))
        count = int(F(s['duration']['num'],s['duration']['den'])*25)
        oracle = oracle or (lambda n: expected_frame(resolved(s,n),sources,n))
        wanted = [oracle(n) for n in range(count)]
        check_media(output/(name+'.mkv'),wanted,count)
        assert result['frames'] == count
        return result, wanted

    times = [F(n,25) for n in range(51)] + [F(1,1000), F(3,20), F(399,1000), F(1601,1000)]
    report = inspect(scene,times)
    assert not report['reads_media'] and not report['writes_files']
    for t, sample in zip(times,report['samples']):
        values = expected(t)
        assert sample['values']['local_b'] == value(t-F(2,5))
        assert sample['values']['random'] == value(random_value(17,int(10*t)))
        for bound in sample['bindings']:
            pos, alpha = values[bound['layer']]
            target = pos if bound['property'] == 'position' else alpha
            kind = 'vector2' if bound['property'] == 'position' else 'scalar'
            assert bound['value'] == value(target,kind), (t,bound,target)
            assert bound['rounded'] == (list(map(rounded,pos)) if kind == 'vector2' else [rounded(alpha)])
    receipt, wanted = render(scene,'linked')
    info = call('scene.inspect',scene=scene,input_root=str(sources))
    assert info['expressions']['frame_bindings'] == [s['bindings'] for s in report['samples'][:50]]
    for i, timing in enumerate(info['timing']):
        for n, parameters in enumerate(per_frame(timing['sampled_parameters'])):
            if parameters is not None:
                tr = resolved(scene,n)['layers'][i]['transform']
                assert parameters['position'] == tr['position'] and parameters['opacity'] == tr['opacity']
    passed.append('expressions.closed_form_rational_links_and_complete_rendered_pixels')

    reordered = copy.deepcopy(scene)
    rng = random.Random(928371)
    rng.shuffle(reordered['expressions']['nodes']); rng.shuffle(reordered['expressions']['bindings'])
    shuffled = list(times); rng.shuffle(shuffled)
    canonical = {json.dumps(s['time'],sort_keys=True):s for s in report['samples']}
    for sample in inspect(reordered,shuffled)['samples']:
        target = canonical[json.dumps(sample['time'],sort_keys=True)]
        assert sample['values'] == target['values']
        assert sorted(sample['bindings'],key=lambda b:(b['layer'],b['property'])) == sorted(target['bindings'],key=lambda b:(b['layer'],b['property']))
    render(reordered,'reordered')
    changed = copy.deepcopy(scene); changed['expressions']['seed'] = 991
    changed_report = inspect(changed,times)
    assert any(a['values']['random'] != b['values']['random'] for a,b in zip(report['samples'],changed_report['samples']))
    render(changed,'other-seed')
    # Inspection is independent of filesystem availability, unlike compilation.
    absent = copy.deepcopy(scene)
    for layer in absent['layers']:
        for frame in layer['frames']: frame['image']['path'] = 'absent.png'
    assert inspect(absent,times) == report
    vectors = [(0,0,F(3332816085,4294967296)),(17,0,F(592190159,1073741824)),
               (17,19,F(3603378843,4294967296)),(2**64-1,2**32-1,F(561718435,1073741824))]
    for seed,index,wanted_random in vectors:
        v = copy.deepcopy(scene); v['expressions']['seed'] = seed
        v['expressions']['nodes'] = [constant('i',index),node('r',op='seeded',stream=7,index='i')]
        v['expressions']['bindings'] = [{'layer':'A','property':'opacity','node':'r'}]
        assert inspect(v,[F(0)])['samples'][0]['values']['r'] == value(wanted_random)
    passed.append('expressions.seeded_random_access_and_order_independence')

    # Additional operators have authored truth-table/closed-form expectations.
    algebra = copy.deepcopy(scene)
    extras = [constant('negative',F(-7,2)),constant('half',F(1,2)),constant('truth',True,'boolean'),
              node('neg_floor',op='floor',value='negative'),node('remainder',op='modulo',a='negative',b='three'),
              node('minimum',op='minimum',a='negative',b='half'),node('maximum',op='maximum',a='negative',b='half'),
              node('is_less','boolean','less',a='negative',b='half'),node('is_equal','boolean','equal',a='two',b='two'),
              node('both','boolean','and',a='is_less',b='is_equal'),node('opposite','boolean','not',value='both'),
              node('chosen','vector2','select',condition='both',yes='base_a',no='offset'),
              node('alias','vector2','link',node='chosen'),node('component',op='component',vector='alias',axis='x'),
              node('scaled','vector2','multiply',a='offset',b='half'),node('divided','vector2','divide',a='offset',b='two'),
              node('difference','vector2','subtract',a='offset',b='base_a'),
              node('unbound',op='property',layer='B',property='opacity')]
    algebra['expressions']['nodes'] += extras
    # Explicit unbound property uses its ordinary value, with base clock clamping.
    algebra['expressions']['bindings'] = [v for v in algebra['expressions']['bindings'] if not (v['layer']=='B' and v['property']=='opacity')]
    for t, sample in zip(times,inspect(algebra,times)['samples']):
        base_x = rounded(3+5*t)
        checks = {'neg_floor':value(-4),'remainder':value(F(5,2)),'minimum':value(F(-7,2)),'maximum':value(F(1,2)),
                  'is_less':value(True,'boolean'),'is_equal':value(True,'boolean'),'both':value(True,'boolean'),
                  'opposite':value(False,'boolean'),'component':value(base_x),'scaled':value([-2,5],'vector2'),
                  'divided':value([-2,5],'vector2'),'difference':value([-4-base_x,7],'vector2'),'unbound':value(255)}
        assert all(sample['values'][k] == v for k,v in checks.items()), sample
    clocks = copy.deepcopy(scene)
    clocks['layers'][1]['animation'] = {'opacity':{'keys':[
        {'time':time(0),'value':0,'interpolation':'linear'},
        {'time':time(6,5),'value':120,'interpolation':'hold'}]}}
    clocks['expressions']['nodes'].append(node('base_b',op='base',layer='B',property='opacity'))
    for t,sample in zip(times,inspect(clocks,times)['samples']):
        assert sample['values']['base_b'] == value(rounded(min(F(6,5),max(F(0),t-F(2,5)))*100))
    # Negative half ties and normalized literals must reach actual bindings.
    ties = copy.deepcopy(scene)
    ties['expressions'] = {'schema_version':1,'seed':0,'nodes':[constant('p',[F(-7,2),F(7,2)],'vector2'),constant('a',F(7,2))],
        'bindings':[{'layer':'A','property':'position','node':'p'},{'layer':'A','property':'opacity','node':'a'}]}
    tie_sample = inspect(ties,[F(0)])['samples'][0]
    assert [b['rounded'] for b in tie_sample['bindings']] == [[-4,4],[4]]
    ties['expressions']['nodes'][1]['expression']['value']['value'] = {'num':70,'den':20}
    assert inspect(ties,[F(0)])['samples'][0] == tie_sample
    spatial = copy.deepcopy(scene)
    for layer in spatial['layers']:
        layer['transform']['spatial'] = {'translate_milli':[250,500],'scale_milli':[1500,1250], 'rotation_mdeg':15000,
            'flip':[False,False], 'pixel_aspect':time(1),'sampling':'bilinear','edge':'transparent'}
    render(spatial,'spatial-links',lambda n:spatial_reference(resolved(spatial,n),sources,n))
    passed.append('expressions.typed_algebra_base_clocks_and_spatial_composition')

    client = Client(exe)
    try:
        client.initialize(); catalog = client.rpc('tools/list')['result']['tools']; assert len(catalog) == MCP_TOOLS
        tool = next(t for t in catalog if t['name'] == 'cutbolt_expression_inspect')
        args = {'scene':scene,'times':[time(0),time(1,10)]}
        Draft202012Validator(tool['inputSchema']).validate(args)
        assert tool['annotations']['readOnlyHint'] and tool['annotations']['idempotentHint']
        assert client.call('expression.inspect',**args) == inspect(scene,[F(0),F(1,10)])
        template = {'schema_version':1,'id':'graph-template','scene':scene,'parameters':[]}
        instantiated = client.call('graphics.instantiate',template=template,values={},instance_id='graph-instance',input_root=str(sources))['scene']
        assert instantiated['expressions'] == scene['expressions']
        template_samples = inspect(instantiated,times)['samples']
        assert template_samples == report['samples']
        project = client.call('project.create',id='expression-edit',width=64,height=48,frame_rate=time(25))
        common = {'store_root':str(store),'project_id':project['id']}
        client.call('session.create',store_root=str(store),project=project,request_id='create')
        operations = [{'op':'media.add','asset':receipt['asset']},
                      {'op':'clip.append','clip':{'id':'tail','asset_id':scene['id'],'source_in':time(1),'duration':time(1)}},
                      {'op':'clip.append','clip':{'id':'head','asset_id':scene['id'],'source_in':time(0),'duration':time(1)}}]
        request = {**common,'expected_revision':0,'operations':operations,'request_id':'add'}
        applied = client.call('session.apply',**request); assert client.call('session.apply',**request) == applied
        saved = client.call('session.get',**common)
        fields = {'project':saved,'input_root':str(output),'output_root':str(output)}
        result = call('render.run',**fields,output=str(output/'saved.mkv'))
        check_media(output/'saved.mkv',wanted[25:]+wanted[:25],50)
        for n,source_n in [(0,25),(24,49),(25,0),(49,24)]:
            path = output/f'preview-{n}.png'
            client.call('preview.frame',**fields,output=str(path),time=time(n,25))
            with Image.open(path) as im: assert im.tobytes() == wanted[source_n]
            previews += 1
        call('preview.range',**fields,output=str(output/'range.mkv'),start=time(24,25),duration=time(3,25))
        check_media(output/'range.mkv',[wanted[49],wanted[0],wanted[1]],3)
        client.call('session.undo',**common,expected_revision=saved['revision'],request_id='undo')
        restored = client.call('session.get',**common)
        assert restored.get('tracks') == project.get('tracks') and restored['assets'] == project['assets'] and restored['clips'] == project['clips']
    finally:
        client.close()
    passed.append('expressions.typed_inspection_saved_edits_retry_undo_and_previews')

    invalid = []
    def bad(label,change,code='INVALID_EXPRESSION'):
        s = copy.deepcopy(scene); change(s['expressions']); invalid.append((label,s,code))
    bad('schema',lambda g:g.update(schema_version=2),'LIMIT_EXCEEDED')
    bad('no-nodes',lambda g:g.update(nodes=[]),'LIMIT_EXCEEDED')
    bad('no-bindings',lambda g:g.update(bindings=[]),'LIMIT_EXCEEDED')
    bad('too-many-nodes',lambda g:g.update(nodes=[constant(f'n{i}',0) for i in range(257)]),'LIMIT_EXCEEDED')
    bad('too-many-bindings',lambda g:g.update(bindings=g['bindings']*9),'LIMIT_EXCEEDED')
    bad('duplicate-id',lambda g:g['nodes'].append(copy.deepcopy(g['nodes'][0])))
    bad('empty-id',lambda g:g['nodes'][0].update(id=''))
    bad('long-id',lambda g:g['nodes'][0].update(id='x'*129))
    bad('control-id',lambda g:g['nodes'][0].update(id='bad\nname'))
    bad('missing-node',lambda g:g['bindings'][0].update(node='missing'))
    bad('missing-layer',lambda g:g['bindings'][0].update(layer='missing'),'EXPRESSION_TYPE')
    bad('wrong-binding-type',lambda g:g['bindings'][0].update(node='zero'),'EXPRESSION_TYPE')
    bad('duplicate-binding',lambda g:g['bindings'].append(copy.deepcopy(g['bindings'][0])))
    bad('unknown-property-layer',lambda g:g['nodes'].append(node('bad',op='property',layer='missing',property='opacity')))
    bad('unknown-time-layer',lambda g:g['nodes'].append(node('bad',op='time',layer='missing')))
    bad('unknown-link',lambda g:g['nodes'].append(node('bad',op='link',node='missing')))
    bad('declared-type',lambda g:g['nodes'][0].update(kind='boolean'),'EXPRESSION_TYPE')
    bad('operator-type',lambda g:g['nodes'].append(node('bad',op='add',a='zero',b='offset')),'EXPRESSION_TYPE')
    bad('scalar-left-vector-scale',lambda g:g['nodes'].append(node('bad',op='multiply',a='zero',b='offset')),'EXPRESSION_TYPE')
    bad('divide-zero-unused',lambda g:g['nodes'].append(node('bad',op='divide',a='two',b='zero')),'EXPRESSION_DOMAIN')
    bad('modulo-zero',lambda g:g['nodes'].append(node('bad',op='modulo',a='two',b='zero')),'EXPRESSION_DOMAIN')
    bad('denominator-zero',lambda g:g['nodes'][0]['expression']['value'].update(value={'num':0,'den':0}),'EXPRESSION_DOMAIN')
    bad('denominator-limit',lambda g:g['nodes'][0]['expression']['value'].update(value={'num':1,'den':1000000000001}),'EXPRESSION_PRECISION')
    bad('numerator-limit',lambda g:g['nodes'][0]['expression']['value'].update(value={'num':9007199254740992,'den':1}),'EXPRESSION_PRECISION')
    bad('arithmetic-overflow',lambda g:g['nodes'].extend([constant('huge',9007199254740991),node('overflow',op='multiply',a='huge',b='huge')]),'EXPRESSION_PRECISION')
    for label,n in [('negative-seed-index',-1),('fraction-seed-index',F(1,2)),('large-seed-index',2**32)]:
        bad(label,lambda g,n=n:g['nodes'].extend([constant('index-bad',n),node('bad',op='seeded',stream=0,index='index-bad')]),'EXPRESSION_DOMAIN')
    bad('position-exact-range',lambda g:(g['nodes'].append(constant('outside',[F(65537,2),0],'vector2')),g['bindings'][0].update(node='outside')),'EXPRESSION_RANGE')
    bad('opacity-exact-range',lambda g:(g['nodes'].append(constant('outside',F(2551,10))),g['bindings'][1].update(node='outside')),'EXPRESSION_RANGE')
    bad('node-cycle',lambda g:g['nodes'].extend([node('cycle-a',op='link',node='cycle-b'),node('cycle-b',op='link',node='cycle-a')]),'EXPRESSION_CYCLE')
    bad('property-self-cycle',lambda g:g['nodes'][19].update(expression={'op':'property','layer':'A','property':'position'}),'EXPRESSION_CYCLE')
    bad('property-cross-cycle',lambda g:g['nodes'][19].update(expression={'op':'property','layer':'B','property':'position'}),'EXPRESSION_CYCLE')
    bad('unknown-operation',lambda g:g['nodes'][0].update(expression={'op':'eval','source':'anything'}),'INVALID_JSON')
    bad('unknown-field',lambda g:g.update(source='anything'),'INVALID_JSON')
    # All branches are domain checked, including a select arm not taken.
    bad('unselected-divide-zero',lambda g:g['nodes'].extend([constant('no',False,'boolean'),node('bad',op='divide',a='two',b='zero'),node('choice',op='select',condition='no',yes='bad',no='two')]),'EXPRESSION_DOMAIN')
    for reverse in (False,True):
        chain = [constant('depth-0',1)] + [node(f'depth-{i}',op='link',node=f'depth-{i-1}') for i in range(1,65)]
        if reverse: chain.reverse()
        bad('depth-'+str(reverse),lambda g,chain=chain:g['nodes'].extend(chain),'LIMIT_EXCEEDED')
    for label,s,code in invalid:
        inspect(s,[F(0)],code)
        path = output/(label+'.mkv')
        call('scene.render',code,scene=s,input_root=str(sources),output_root=str(output),output=str(path))
        assert not path.exists()
    for times_,code in [([], 'LIMIT_EXCEEDED'),([F(0)]*257,'LIMIT_EXCEEDED'),([F(201,100)],'INVALID_EXPRESSION'),([F(-1,25)],'INVALID_JSON')]:
        inspect(scene,times_,code)
    no_graph = copy.deepcopy(scene); no_graph.pop('expressions'); inspect(no_graph,[F(0)],'INVALID_EXPRESSION')
    call('scene.render','OUTPUT_EXISTS',scene=scene,input_root=str(sources),output_root=str(output),output=str(output/'linked.mkv'))
    changed_identity = copy.deepcopy(scene)
    for layer in changed_identity['layers']:
        layer['frames'][0]['image']['sha256'] = '0'*64
    call('scene.render','MEDIA_CHANGED',scene=changed_identity,input_root=str(sources),output_root=str(output),output=str(output/'changed-source.mkv'))
    assert not (output/'changed-source.mkv').exists()
    assert before == {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob('.cutbolt-*'))
    assert ff(output/'linked.mkv','video') == b''.join(wanted)
    passed.append('expressions_cycles_types_domains_precision_and_atomic_preservation')

    # Exercise actual maximum graph and bindings, with 250 rendered frames and
    # 256 read-only samples. Limits are selected before timing, never adapted.
    maximum = copy.deepcopy(scene)
    maximum.update(id='maximum-graph',width=16,height=12,duration=time(10))
    maximum['layers'] = []
    for i in range(16):
        layer = copy.deepcopy(a); layer.pop('animation'); layer.update(id=f'L{i}',duration=time(10)); layer['transform']['position']=[i%9,i%7]
        maximum['layers'].append(layer)
    nodes = [constant('p',[1,1],'vector2'),constant('o',128)]
    nodes += [node(f'd{i}',op='link',node='o' if i==0 else f'd{i-1}') for i in range(63)]
    nodes += [constant(f'c{i}',i) for i in range(191)]
    assert len(nodes)==256
    maximum['expressions'] = {'schema_version':1,'seed':0,'nodes':nodes,'bindings':[
        {'layer':f'L{i}','property':prop,'node':'p' if prop=='position' else 'd62'} for i in range(16) for prop in ('position','opacity')]}
    begun = clock.perf_counter(); max_report = inspect(maximum,[F(i,100) for i in range(256)]); elapsed = clock.perf_counter()-begun
    budgets.check(elapsed < 15, elapsed)
    assert max_report['node_evaluations']==65536 and len(max_report['samples'])==256
    assert all(s['values']['d62']==value(128) and len(s['bindings'])==32 for s in max_report['samples'])
    static = copy.deepcopy(maximum); static.pop('expressions')
    for layer in static['layers']: layer['transform'].update(position=[1,1],opacity=128)
    cache = {n:expected_frame(static,sources,n) for n in range(10)}
    begun = clock.perf_counter(); render(maximum,'maximum',lambda n:cache[n%10]); render_elapsed = clock.perf_counter()-begun
    budgets.check(render_elapsed < 60, render_elapsed)
    passed.append('expressions.maximum_graph_depth_bindings_samples_and_render_work')
    result = {'passed':passed,'frames_compared':frames,'samples_compared':samples,'previews':previews,'rejections':rejected,
              'maximum_inspect_seconds':elapsed,'maximum_render_seconds':render_elapsed,'maximum_nodes':256,'maximum_bindings':32,
              'maximum_dependency_depth':64,'maximum_samples':256,'maximum_node_evaluations':65536,
              'references':'Authored closed-form Fraction motion/opacity; fixed operator truth tables; forward pixels and high-precision geometry',
              'source_preserved':True,'execution_profile':'closed typed graph; no source code, IO or ambient random state'}
    (root/'scene.json').write_text(json.dumps(scene,indent=2)+'\n')
    (root/'verification.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result,indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(); parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args().output)
