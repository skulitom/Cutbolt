"""Original high-depth charts with independent Decimal transfer/color and Fraction time references."""
from engine import ENGINE
import argparse
from array import array
import copy
from decimal import Decimal as D, getcontext
from fractions import Fraction as F
from functools import lru_cache
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import wave
from PIL import Image
from jsonschema import Draft202012Validator
from agents import Client
from scenes import identity, time

ROOT=Path(__file__).resolve().parents[1]
getcontext().prec=50
W,H,N=64,8,4
ZERO,ONE=D(0),D(1)

def enc(transfer='pq',primaries='bt2020',peak=1000,black=0):
    return {'transfer':transfer,'primaries':primaries,'display':{'peak_nits':peak,'black_millinits':black}}

def config(e):return (e['transfer'],e['primaries'],e['display']['peak_nits'],e['display']['black_millinits'])
def clamp(x):return max(ZERO,min(ONE,x))
def quant(x,maximum):return int(clamp(x)*maximum+D('0.5'))
def weights(p):return list(map(D,('0.2627','0.6780','0.0593') if p=='bt2020' else ('0.2126','0.7152','0.0722')))
def luminance(rgb,p):return sum(w*v for w,v in zip(weights(p),rgb))

@lru_cache(None)
def scalar_decode(v,transfer):
    if transfer=='pq':
        power=v**(D(32)/2523)
        return 10000*(max(ZERO,power-D(3424)/4096)/(D(2413)/128-D(2392)/128*power))**(D(16384)/2610)
    if transfer=='srgb':return v/D('12.92') if v<=D('0.04045') else ((v+D('0.055'))/D('1.055'))**D('2.4')
    if transfer=='bt709':return v/D('4.5') if v<D('0.081') else ((v+D('0.099'))/D('1.099'))**(D(1)/D('0.45'))
    a=D('0.17883277');b=1-4*a;c=D('.5')-a*(4*a).ln()
    return v*v/3 if v<=D('.5') else (((v-c)/a).exp()+b)/12

@lru_cache(None)
def scalar_encode(v,transfer):
    if transfer=='pq':
        power=(v/10000)**(D(2610)/16384)
        return ((D(3424)/4096+D(2413)/128*power)/(1+D(2392)/128*power))**(D(2523)/32)
    if transfer=='srgb':return D('12.92')*v if v<=D('0.0031308') else D('1.055')*v**(D(1)/D('2.4'))-D('0.055')
    if transfer=='bt709':return D('4.5')*v if v<D('0.018') else D('1.099')*v**D('0.45')-D('0.099')
    a=D('0.17883277');b=1-4*a;c=D('.5')-a*(4*a).ln()
    return (3*v).sqrt() if v<=D(1)/12 else a*(12*v-b).ln()+c

def decode(rgb,e):
    t,p,peak,black=e
    if t=='hlg':
        gamma=D('1.2')+D('.42')*(D(peak)/1000).log10()
        beta=(3*(D(black)/1000/peak)**(1/gamma)).sqrt()
        scene=[scalar_decode((1-beta)*v+beta,t) for v in rgb]
        gain=D(peak)*luminance(scene,p)**(gamma-1)
        return [v*gain for v in scene]
    return [scalar_decode(v,t)*(1 if t=='pq' else peak) for v in rgb]

def encode(rgb,e):
    t,p,peak,black=e
    if t=='hlg':
        gamma=D('1.2')+D('.42')*(D(peak)/1000).log10()
        beta=(3*(D(black)/1000/peak)**(1/gamma)).sqrt()
        relative=[v/peak for v in rgb];y=luminance(relative,p)
        # Solve Y_scene ** gamma = Y_display, then recover chromatic ratios.
        scene=[ZERO]*3 if y==0 else [v/y*y**(1/gamma) for v in relative]
        return [(scalar_encode(v,t)-beta)/(1-beta) for v in scene]
    return [scalar_encode(v if t=='pq' else v/peak,t) for v in rgb]

def solve(matrix,vector):
    a=[[F(v) for v in row]+[F(value)] for row,value in zip(matrix,vector)]
    for col in range(3):
        pivot=next(i for i in range(col,3) if a[i][col]);a[col],a[pivot]=a[pivot],a[col]
        divisor=a[col][col];a[col]=[v/divisor for v in a[col]]
        for row in range(3):
            if row!=col:
                factor=a[row][col];a[row]=[v-factor*w for v,w in zip(a[row],a[col])]
    return [row[3] for row in a]

@lru_cache(None)
def xyz_matrix(primaries):
    xy=[list(map(F,xy)) for xy in ([('0.708','0.292'),('0.170','0.797'),('0.131','0.046')] if primaries=='bt2020' else [('0.64','0.33'),('0.30','0.60'),('0.15','0.06')])]
    columns=[[x/y,F(1),(1-x-y)/y] for x,y in xy];basis=list(zip(*columns))
    scale=solve(basis,[F(3127,3290),F(1),F(3583,3290)])
    return [[v*s for v,s in zip(row,scale)] for row in basis]

@lru_cache(None)
def primary_matrix(source,target):
    if source==target:return [[ONE,ZERO,ZERO],[ZERO,ONE,ZERO],[ZERO,ZERO,ONE]]
    columns=[solve(xyz_matrix(target),column) for column in zip(*xyz_matrix(source))]
    return [[D(v.numerator)/D(v.denominator) for v in row] for row in zip(*columns)]

@lru_cache(None)
def interpreted(codes,bits,matrix,range_):
    maximum=D(2**bits-1);factor=D(2**(bits-8));offset,span,chroma=(ZERO,maximum,maximum) if range_=='full' else (16*factor,219*factor,224*factor)
    if matrix=='rgb':return tuple(clamp((D(v)-offset)/span) for v in codes)
    kr,kg,kb=weights('bt2020' if matrix=='bt2020_ncl' else 'bt709')
    y=(D(codes[0])-offset)/span;cb=(D(codes[1])-128*factor)/chroma;cr=(D(codes[2])-128*factor)/chroma
    r=y+2*(1-kr)*cr;b=y+2*(1-kb)*cb
    return tuple(map(clamp,(r,(y-kr*r-kb*b)/kg,b)))

@lru_cache(None)
def pixel(codes,bits,matrix,range_,source,target,exposure,tone,white,peak,depth):
    rgb=decode(interpreted(codes,bits,matrix,range_),source)
    gain=D(2)**(D(exposure)/1000);m=primary_matrix(source[1],target[1])
    rgb=[max(ZERO,sum(v*w for v,w in zip(row,rgb))*gain) for row in m]
    if tone=='clip':rgb=[min(ONE,v/white)*target[2] for v in rgb]
    elif tone=='reinhard':
        x=max(rgb)/white;ceiling=D(peak)/white
        mapped=min(ONE,(x+x*x/(ceiling*ceiling))/(1+x))
        rgb=[ZERO]*3 if x==0 else [v/(x*white)*mapped*target[2] for v in rgb]
    return tuple(quant(v,2**depth-1) for v in encode(rgb,target))

def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    sources,output,store=[root/p for p in ('sources','output','store')]
    for p in (sources,output,store):p.mkdir()
    exe=ENGINE;passed=[];cases=[];rejected=0;fixtures={};originals={};receipts={};recipes={};expected={}
    def ff(args,cwd=None):
        p=subprocess.run(['ffmpeg','-v','error','-nostdin','-n',*args],capture_output=True,timeout=180,cwd=cwd)
        assert p.returncode==0,p.stderr.decode(errors='replace');return p.stdout
    def call(request,error=None,env=None):
        p=subprocess.run([str(exe)],input=json.dumps(request).encode(),capture_output=True,timeout=240,env=env);v=json.loads(p.stdout)
        if error:
            nonlocal rejected
            assert p.returncode==1 and v['error']['code']==error,(error,v);rejected+=1;return v
        assert p.returncode==0 and v['ok'],v;return v['result']
    def probe(path):return json.loads(subprocess.check_output(['ffprobe','-v','error','-show_streams','-of','json',str(path)]))['streams'][0]
    def remember(path):originals[path.name]=hashlib.sha256(path.read_bytes()).hexdigest()
    def generate(name,bits=16,matrix='rgb',range_='full',encoding=None,tagged=True,w=W,h=H,n=N,all_codes=False):
        encoding=encoding or enc();maximum=2**bits-1;triples=[]
        for frame in range(n):
            chart=[]
            for y in range(h):
                for x in range(w):
                    if all_codes:
                        v=y*w+x;triple=(v,maximum-v,(v*17)%65536)
                    elif y<2:
                        v=(x+frame*w)*maximum//(w*n-1);triple=(v,v,v) if matrix=='rgb' else (v,2**(bits-1),2**(bits-1))
                    elif y==2:
                        light=[D(0),D('.005'),D('.1'),D(1),D(10),D(50),D(100),D(203),D(400),D(1000),D(2000),D(4000),D(10000)][x%13]
                        triple=tuple(quant(v,maximum) for v in encode([light]*3,config(encoding))) if matrix=='rgb' else (maximum if x%2 else 0,(x*17)%maximum,(x*59)%maximum)
                    else:triple=((x*997+frame*199+y*103)%maximum,(x*271+frame*331+y*307)%maximum,(x*73+frame*211+y*97)%maximum)
                    chart.append(triple)
            triples.append(chart)
        if bits==8:
            fmt='bgr0';raw=b''.join(bytes([b,g,r,0]) for frame in triples for r,g,b in frame)
        else:
            fmt=('gbrp' if matrix=='rgb' else 'yuv444p')+str(bits)+'le';order=(1,2,0) if matrix=='rgb' else (0,1,2)
            raw=b''.join(array('H',[rgb[c] for rgb in frame]).tobytes() for frame in triples for c in order)
        raw_path=sources/(name+'.raw');raw_path.write_bytes(raw);remember(raw_path)
        pcm=array('h',[(i*37+c*181)%20001-10000 for i in range(n*1920) for c in range(2)]).tobytes()
        sound=sources/(name+'.wav')
        with wave.open(str(sound),'wb') as f:f.setnchannels(2);f.setsampwidth(2);f.setframerate(48000);f.writeframes(pcm)
        remember(sound);path=sources/(name+'.mkv')
        tags=['-colorspace',{'rgb':'rgb','bt709':'bt709','bt2020_ncl':'bt2020nc'}[matrix],'-color_range','pc' if range_=='full' else 'tv','-color_primaries',encoding['primaries'],'-color_trc',{'pq':'smpte2084','hlg':'arib-std-b67','srgb':'iec61966-2-1','bt709':'bt709'}[encoding['transfer']]] if tagged else []
        ff(['-f','rawvideo','-pixel_format',fmt,'-video_size',f'{w}x{h}','-framerate','25','-i',str(raw_path),'-i',str(sound),'-map','0:v:0','-map','1:a:0','-vf','setsar=1','-c:v','ffv1','-level','3','-slices','1','-threads','1','-pix_fmt',fmt,'-c:a','pcm_s16le',*tags,str(path)])
        remember(path);assert ff(['-i',str(path),'-an','-pix_fmt',fmt,'-f','rawvideo','-'])==raw
        fixtures[name]={'path':path,'bits':bits,'matrix':matrix,'range':range_,'encoding':encoding,'w':w,'h':h,'n':n,'triples':triples,'pcm':pcm}
    for transfer in ('pq','hlg'):
        for bits in (10,12,16):generate(f'{transfer}-{bits}',bits,encoding=enc(transfer))
    for bits in (10,12,16):
        for range_ in ('full','limited'):generate(f'yuv-{bits}-{range_}',bits,'bt2020_ncl',range_,enc())
    generate('rgb-limited',12,range_='limited')
    generate('untagged',12,tagged=False)
    generate('sdr8',8,encoding=enc('srgb','bt709',100))
    for transfer in ('pq','hlg'):generate('codes-'+transfer,encoding=enc(transfer),w=256,h=256,n=1,all_codes=True)
    generate('hlg-black',encoding=enc('hlg',peak=2000,black=5))

    def recipe(name,target=None,depth='rgb16',tone=None,**changes):
        f=fixtures[name];r={'schema_version':1,'id':name,'source':{'file':identity(f['path'],sources),'encoding':f['encoding'],'matrix':f['matrix'],'range':f['range'],'missing_tags':'reject'},
         'output':{'encoding':target or f['encoding'],'depth':depth},'source_in':time(0),'duration':time(f['n'],25),'rate':time(1),'reverse':False,'freeze':False,'width':f['w'],'height':f['h'],'exposure_milliev':0,'tone':tone or {'mode':'preserve'},'audio':'resample'}
        r.update(changes);return r
    def request(r,label,command='hdr.conform'):
        return {'command':command,'recipe':r,'input_root':str(sources),**({} if command=='hdr.inspect' else {'output_root':str(output),'output':str(output/(label+'.mkv'))})}
    def check(name,r,label,exact=False):
        f=fixtures[name];count=int(F(r['duration']['num'],r['duration']['den'])*25)
        start=F(r['source_in']['num'],r['source_in']['den']);rate=F(r['rate']['num'],r['rate']['den'])
        positions=[start+(0 if r['freeze'] else -1 if r['reverse'] else 1)*F(n,25)*rate for n in range(count)]
        indices=[int(t*25) for t in positions]
        plan=call(request(r,label,'hdr.inspect'));receipt=call(request(r,label));path=output/(label+'.mkv')
        assert receipt['source_frame_indices']==plan['source_frame_indices']==indices
        depth=16 if r['output']['depth']=='rgb16' else 8;fmt='rgb48le' if depth==16 else 'rgb24'
        rgb=ff(['-i',str(path),'-an','-pix_fmt',fmt,'-f','rawvideo','-']);actual=array('H' if depth==16 else 'B');actual.frombytes(rgb)
        wanted=[];ton=r['tone'];reference_white=ton.get('reference_white_nits',0);peak=ton.get('source_peak_nits',0)
        for n in indices:
            for y in range(r['height']):
                for x in range(r['width']):
                    codes=f['triples'][n][y*f['h']//r['height']*f['w']+x*f['w']//r['width']]
                    wanted.extend(codes if exact else pixel(codes,f['bits'],f['matrix'],f['range'],config(f['encoding']),config(r['output']['encoding']),r['exposure_milliev'],ton['mode'],reference_white,peak,depth))
        assert len(actual)==len(wanted)
        maximum=max(abs(a-b) for a,b in zip(actual,wanted));assert maximum<=(0 if exact else 2 if depth==16 else 1),(label,maximum)
        if r['audio']=='mute':audio_want=bytes(count*1920*4)
        else:
            original=array('h');original.frombytes(f['pcm']);audio=[]
            for n in range(count*1920):
                pos=start*48000+n*rate;a=int(pos);b=min(a+1,f['n']*1920-1);fraction=pos-a
                for c in range(2):
                    v=original[a*2+c]*(1-fraction)+original[b*2+c]*fraction
                    audio.append(int(abs(v)+F(1,2))*(1 if v>=0 else -1))
            audio_want=array('h',audio).tobytes()
        assert ff(['-i',str(path),'-vn','-f','s16le','-'])==audio_want
        meta=probe(path);target=r['output']['encoding'];assert meta['color_space']=='gbr' and meta['color_range']=='pc' and meta['color_primaries']==target['primaries']
        assert meta['color_transfer']=={'pq':'smpte2084','hlg':'arib-std-b67','srgb':'iec61966-2-1','bt709':'bt709'}[target['transfer']]
        assert os.path.samefile(receipt['output_metadata']['format']['filename'],path)
        if target['transfer'] in ('pq','hlg'):
            display=next(v for v in meta['side_data_list'] if v['side_data_type']=='Mastering display metadata')
            assert [F(display[k]) for k in ['red_x','red_y','green_x','green_y','blue_x','blue_y','white_point_x','white_point_y']]==[F(v) for v in ('0.708','0.292','0.170','0.797','0.131','0.046','0.3127','0.3290')]
            assert F(display['max_luminance'])==target['display']['peak_nits'] and F(display['min_luminance'])==F(target['display']['black_millinits'],1000)
            assert 'title' not in meta.get('tags',{}) and 'asset' not in receipt
            # Fast seek exercises original cue/seek positions after metadata authoring.
            seek=ff(['-ss',str(float(F(count-1,25))),'-i',str(path),'-frames:v','1','-an','-pix_fmt',fmt,'-f','rawvideo','-'])
            assert seek==rgb[-r['width']*r['height']*3*(2 if depth==16 else 1):]
        else:assert not meta.get('side_data_list')
        cases.append({'case':label,'frames':count,'bits':depth,'maximum_code_error':maximum});receipts[label]=receipt;recipes[label]=r;expected[label]=(rgb,audio_want)
        assert originals=={name:hashlib.sha256((sources/name).read_bytes()).hexdigest() for name in originals}
        return receipt
    for transfer in ('pq','hlg'):
        for bits in (10,12,16):
            name=f'{transfer}-{bits}';check(name,recipe(name),name,exact=bits==16)
        name='codes-'+transfer;check(name,recipe(name),name,exact=True)
    check('hlg-black',recipe('hlg-black'),'hlg-black-identity',exact=True)
    passed.append('hdr.native_depth_transfer_precision')
    for name in [n for n in fixtures if n.startswith('yuv-')]+['rgb-limited']:
        check(name,recipe(name),name)
    passed.append('hdr.high_depth_ranges')
    for name,target,depth,tone,exposure in [
      ('pq-16',enc('hlg',peak=1000),'rgb16',{'mode':'preserve'},0),
      ('hlg-black',enc('pq',peak=4000),'rgb16',{'mode':'preserve'},0),
      ('pq-16',enc('pq',peak=4000),'rgb16',{'mode':'preserve'},1500),
      ('sdr8',enc('pq',peak=1000),'rgb16',{'mode':'preserve'},1000),
      ('pq-16',enc('srgb','bt709',100),'rgb16',{'mode':'clip','reference_white_nits':100},0),
      ('pq-12',enc('bt709','bt709',100),'rgb8',{'mode':'clip','reference_white_nits':203},-1000),
      ('pq-16',enc('srgb','bt709',100),'rgb8',{'mode':'reinhard','reference_white_nits':203,'source_peak_nits':4000},0),
      ('hlg-12',enc('srgb','bt709',200),'rgb8',{'mode':'reinhard','reference_white_nits':203,'source_peak_nits':1000},500),
      ('hlg-black',enc('hlg',peak=400,black=5),'rgb16',{'mode':'preserve'},-1000)]:
        label=f'process-{len(cases)}';check(name,recipe(name,target,depth,tone,exposure_milliev=exposure),label)
    passed.append('hdr.exposure_gamut_tone_charts')

    # Re-read authored standard HDR metadata as an ordinary explicitly interpreted source.
    selected_label=next(label for label in recipes if label.startswith('process-') and recipes[label]['output']['encoding']['transfer']=='pq');master=output/(selected_label+'.mkv')
    assert master.exists()
    copied=sources/'reimport.mkv';copied.write_bytes(master.read_bytes());remember(copied)
    f=copy.deepcopy(fixtures['pq-16']);f['path']=copied;f['encoding']=recipes[selected_label]['output']['encoding'];f['bits']=16
    samples=array('H');samples.frombytes(expected[selected_label][0]);f['triples']=[[tuple(samples[i:i+3]) for i in range(n*W*H*3,(n+1)*W*H*3,3)] for n in range(N)]
    fixtures['reimport']=f;check('reimport',recipe('reimport'),'reimport',exact=True)
    passed.append('hdr.standard_display_metadata_and_reimport')

    external=[]
    for bits in (10,12,16):
        f=fixtures[f'pq-{bits}'];raw=ff(['-i',str(f['path']),'-an','-vf','zscale=matrixin=gbr:rangein=full:primariesin=bt2020:transferin=smpte2084:transfer=linear:npl=100:agamma=0,format=gbrpf32le','-f','rawvideo','-'])
        values=array('f');values.frombytes(raw);worst=0.0
        for n in range(N):
            for i,rgb in enumerate(f['triples'][n]):
                light=decode(interpreted(rgb,bits,'rgb','full'),config(f['encoding']))
                for c,plane in enumerate((2,0,1)):
                    actual=values[n*W*H*3+plane*W*H+i]*100;want=float(light[c]);difference=abs(actual-want)
                    assert difference<=0.5+abs(want)*0.0002,(bits,actual,want);worst=max(worst,difference)
        external.append({'bits':bits,'maximum_nit_error':worst})
    passed.append('hdr.external_transfer_reference')

    for label,changes in [('reverse',{'source_in':time(3,25),'duration':time(3,25),'reverse':True,'audio':'mute'}),('freeze',{'source_in':time(2,25),'duration':time(3,25),'freeze':True,'audio':'mute'}),('retime',{'duration':time(2,25),'rate':time(3,2),'width':W//2,'height':H//2})]:
        check('pq-16',recipe('pq-16',**changes),label)
    for width,height in [(1,1),(3,2),(4096,2),(2,2160),(1920,1080)]:
        check('pq-16',recipe('pq-16',width=width,height=height,duration=time(1,25)),f'geometry-{width}-{height}')
    passed.append('hdr.exact_retime_resize_audio')
    client=Client(exe)
    try:
        client.initialize();catalog=client.rpc('tools/list')['result']['tools']
        assert len(catalog)==65 and not any(t['name']=='cutbolt_hdr_conform' for t in catalog)
        tool=next(t for t in catalog if t['name']=='cutbolt_hdr_inspect');assert tool['annotations']['readOnlyHint']
        assert client.rpc('tools/call',{'name':'cutbolt_hdr_conform','arguments':{}})['error']['code']==-32602
        arguments={'recipe':recipes['retime'],'input_root':str(sources)};Draft202012Validator(tool['inputSchema']).validate(arguments)
        assert client.call('hdr.inspect',**arguments)==call({'command':'hdr.inspect',**arguments})
        labels=[label for label,r in recipes.items() if r['output']['depth']=='rgb8'];assert len(labels)==3
        project=call({'command':'project.create','id':'hdr-sdr-edit','width':W,'height':H,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=project,request_id='create');ops=[]
        for i,label in enumerate(labels):
            asset=dict(receipts[label]['asset'],id=label)
            ops += [{'op':'media.add','asset':asset},{'op':'clip.append','clip':{'id':label,'asset_id':label,'source_in':time(1,25),'duration':time(2,25)}}]
        fields={'store_root':str(store),'project_id':project['id'],'expected_revision':0,'request_id':'assemble','operations':ops}
        receipt=client.call('session.apply',**fields);assert receipt==client.call('session.apply',**fields)
        project=client.call('session.get',store_root=str(store),project_id=project['id'])
        want=b''.join(expected[label][0][W*H*3:W*H*9] for label in labels);sound=b''.join(expected[label][1][1920*4:3*1920*4] for label in labels)
        movie=output/'timeline.mkv';call({'command':'render.run','project':project,'input_root':str(output),'output_root':str(output),'output':str(movie)})
        assert ff(['-i',str(movie),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==want and ff(['-i',str(movie),'-vn','-f','s16le','-'])==sound
        for i in range(len(labels)):
            preview=call({'command':'preview.frame','project':project,'input_root':str(output),'output_root':str(output),'output':str(output/f'preview-{i}.png'),'time':time(i*2,25)})
            with Image.open(preview['output']) as image:assert image.tobytes()==want[i*2*W*H*3:(i*2+1)*W*H*3]
        # Each delivery range retains its explicit transfer, including mixed SDR interpretations.
        for i,label in enumerate(labels):call({'command':'export.run','project':project,'input_root':str(output),'output_root':str(output),'output':str(output/f'delivery-{i}.mp4'),'range':{'start':time(i*2,25),'duration':time(2,25)},'profile':'h264_aac','streams':'audio_video','input_transfer':recipes[label]['output']['encoding']['transfer']})
        assert client.call('session.get',store_root=str(store),project_id=project['id'])==project
        # Two-row FFV1 frames exercise the encoder's slice boundary through the timeline.
        tiny=recipe('pq-16',enc('srgb','bt709',100),'rgb8',{'mode':'clip','reference_white_nits':100},width=128,height=2)
        tiny_receipt=check('pq-16',tiny,'tiny-sdr')
        tiny_project=call({'command':'project.create','id':'hdr-tiny-edit','width':128,'height':2,'frame_rate':time(25)})
        client.call('session.create',store_root=str(store),project=tiny_project,request_id='tiny-create')
        client.call('session.apply',store_root=str(store),project_id=tiny_project['id'],expected_revision=0,request_id='tiny-assemble',operations=[
            {'op':'media.add','asset':tiny_receipt['asset']},
            {'op':'clip.append','clip':{'id':'tiny-shot','asset_id':tiny_receipt['asset']['id'],'source_in':time(0),'duration':time(4,25)}}])
        tiny_project=client.call('session.get',store_root=str(store),project_id=tiny_project['id'])
        for command,label,extra,first,count in [('render.run','tiny-timeline',{},0,4),('preview.range','tiny-range',{'start':time(1,25),'duration':time(2,25)},1,2)]:
            path=output/(label+'.mkv')
            call({'command':command,'project':tiny_project,'input_root':str(output),'output_root':str(output),'output':str(path),**extra})
            assert ff(['-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'])==expected['tiny-sdr'][0][first*128*2*3:(first+count)*128*2*3]
            assert ff(['-i',str(path),'-vn','-f','s16le','-'])==expected['tiny-sdr'][1][first*1920*4:(first+count)*1920*4]
    finally:client.close()
    passed.append('hdr.saved_session_mcp_preview_delivery')

    base=recipe('pq-16');bad=[]
    def reject(r,code='INVALID_HDR'):bad.append((r,code))
    for key,value in [('schema_version',2),('exposure_milliev',8001),('width',4097),('height',2161),('duration',time(0)),('rate',time(17)),('source_in',time(100))]:
        r=copy.deepcopy(base);r[key]=value;reject(r)
    for field,value in [('matrix','bt709'),('range','limited'),('encoding',enc('hlg')),('encoding',enc('pq','bt709')),('encoding',enc('pq',peak=0))]:
        r=copy.deepcopy(base);r['source'][field]=value;reject(r)
    for target in [enc('hlg',peak=300),enc('srgb','bt709',1000),enc('pq',black=1001)]:
        r=copy.deepcopy(base);r['output']['encoding']=target;reject(r)
    r=copy.deepcopy(base);r['output']['depth']='rgb8';reject(r)
    r=copy.deepcopy(base);r['output']['encoding']=enc('srgb','bt709',100);reject(r)
    for tone in [{'mode':'clip','reference_white_nits':100},{'mode':'reinhard','reference_white_nits':100,'source_peak_nits':50},{'mode':'clip','reference_white_nits':0}]:
        r=copy.deepcopy(base);r['tone']=tone;reject(r)
    r=copy.deepcopy(base);r['reverse']=True;reject(r)
    r=copy.deepcopy(base);r['freeze']=True;reject(r)
    r=copy.deepcopy(base);r['source']['file']['sha256']='0'*64;reject(r,'MEDIA_CHANGED')
    r=copy.deepcopy(base);r['source']['file']['path']='../outside.mkv';reject(r,'INVALID_PATH')
    r=copy.deepcopy(base);r['source']['missing_tags']='auto';reject(r,'INVALID_JSON')
    r=copy.deepcopy(base);r['output']['depth']='rgb10';reject(r,'INVALID_JSON')
    r=recipe('untagged');reject(r)
    r=recipe('reimport');r['source']['encoding']=copy.deepcopy(r['source']['encoding']);r['source']['encoding']['display']['peak_nits']=900;reject(r)
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    for i,(r,code) in enumerate(bad):call(request(r,'bad-'+str(i)),code)
    call(request(base,'pq-16'),'OUTPUT_EXISTS')
    req=request(base,'bad');req.update(output_root=str(sources),output=str(fixtures['pq-16']['path']));call(req,'OUTPUT_EXISTS')
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    r=recipe('untagged');r['source']['missing_tags']='use_declared';check('untagged',r,'explicit-untagged')
    # Change only our synthetic source after encoding, then require no final publication.
    wrapper=root/'mutate_hdr.rs';tool=root/'mutate_hdr.exe'
    wrapper.write_text('''use std::{env,process::Command,fs::OpenOptions,io::Write};
fn main(){let args:Vec<String>=env::args().skip(1).collect();let status=Command::new(env::var("REFERENCE_FFMPEG").unwrap()).args(&args).status().unwrap();
if status.success() && args.last().is_some_and(|p|p.ends_with("output.mkv")){let mut f=OpenOptions::new().append(true).open(env::var("FIXTURE_SOURCE").unwrap()).unwrap();f.write_all(b"changed").unwrap();}
std::process::exit(status.code().unwrap_or(1));}''',encoding='utf-8')
    subprocess.run(['rustc',str(wrapper),'-o',str(tool)],check=True,capture_output=True)
    source=fixtures['pq-16']['path'].resolve();assert sources in source.parents;saved=source.read_bytes()
    try:call(request(base,'changed-source'),'MEDIA_CHANGED',{**os.environ,'CUTBOLT_FFMPEG':str(tool),'REFERENCE_FFMPEG':shutil.which('ffmpeg'),'FIXTURE_SOURCE':str(source)})
    finally:source.write_bytes(saved)
    assert not (output/'changed-source.mkv').exists() and not list(output.glob('.cutbolt-scene-*'))
    assert originals=={name:hashlib.sha256((sources/name).read_bytes()).hexdigest() for name in originals}
    passed.append('hdr.metadata_validation_preservation_faults')
    report={'passed':passed,'cases':cases,'frames_compared':sum(c['frames'] for c in cases)+12,'stereo_sample_frames_compared':(sum(c['frames'] for c in cases)+12)*1920,
            'external_transfer_reference':external,'rejected_cases':rejected,'all_code_identity_samples_per_transfer':65536*3,
            'threshold':{'maximum_rgb16_code_error':2,'maximum_rgb8_code_error':1,'identity_code_error':0,'pcm_error':0,'external_nits':'0.5 + 0.0002 * expected_nits'},
            'reference':'Original native 10/12/16-bit RGB/YUV444 charts. Independent 50-digit Decimal transfer/tone equations, exact Fraction Gaussian primary conversion and time/audio mapping. Every decoded channel/sample checked; external zscale PQ-to-light comparisons and ffprobe standard display metadata/fast seeking.',
            'limits':'Bounded FFV1/PCM 25fps high-depth processing and HDR intermediates, up to 60 seconds. Existing general timeline remains 8-bit SDR; tone-map HDR to an explicit SDR editing asset. No dynamic HDR metadata, subsampled HDR input, HDR distribution codec or calibrated display control.'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    (root/'recipe.json').write_text(json.dumps(base,indent=2)+'\n',encoding='utf-8')
    (root/'project.json').write_text(json.dumps(project,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);run(p.parse_args().output)
