"""Saved linear16 grade/fade banding: independent f64 references and Bayer tile means."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import subprocess
from PIL import Image
from engine import ENGINE
from scenes import identity
from tracks import edit, track, time
from working_timelines import Context
from working_color import bayer, encode

ROOT=Path(__file__).resolve().parents[1]


def output(pixel, x, y):
    return max(0,min(255,math.floor(255*encode(pixel[0],'srgb')+(bayer(8)[y%8][x%8]+.5)/64)))


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    sources,outputs=root/'sources',root/'outputs';sources.mkdir(parents=True);outputs.mkdir()
    cases=[];ctx=Context(16,'linear')
    def call(command,**fields):
        request=dict(command=command,**fields)
        result=subprocess.run([str(ENGINE)],input=json.dumps(request).encode(),capture_output=True,timeout=600)
        response=json.loads(result.stdout)
        if result.returncode or not response['ok']:(root/'failed-request.json').write_text(json.dumps(request,indent=2))
        assert result.returncode==0 and response['ok'],response
        return response['result']
    def apply(p,ops):return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def curve(first,last,n):return dict(keys=[dict(time=time(0),value=first,interpolation='linear'),dict(time=time(n-1,25),value=last,interpolation='hold')])
    def project(name,width,height,n,temporal):
        path=sources/(name+'.png');image=Image.new('RGB',(width,height))
        image.putdata([(230,)*3 if temporal else (x//8,)*3 for y in range(height) for x in range(width)]);image.save(path)
        layer=dict(id='ramp',canvas=[width,height],start=time(0),duration=time(n,25),frames=[dict(image=identity(path,sources),hold=time(1,25),offset=[0,0],anchor=[0,0])],timing='strict',end='hold_last',transform=dict(position=[0,0],crop=[0,0,width,height],scale=1,quarter_turns=0,opacity=255))
        if temporal:layer['animation']=dict(opacity=curve(0,255,n))
        scene=dict(schema_version=1,id=name,width=width,height=height,output_scale=1,duration=time(n,25),transparent=True,background=[0,0,0],color='srgb_straight_encoded',audio=None,layers=[layer])
        opacity=dict(kind='opacity',value=137) if temporal else dict(kind='opacity',value=255,curve=curve(64,255,n))
        p=call('project.create',id=name,width=width,height=height,frame_rate=time(25))
        return apply(p,[dict(op='project.transfer',transfer='srgb'),edit('create',duration=time(n,25)),edit('add',track=track('v','video')),edit('place',track_id='v',clip=dict(id='ramp',scene=scene,start=time(0),source_in=time(0),duration=time(n,25)),collision='reject'),edit('clip_effects',clip_id='ramp',effects=[dict(kind='grade',exposure_milli=-315,contrast_milli=1000,white_balance_milli=[1000]*3),opacity])])
    for name,width,height,n,temporal in [('temporal_ramp',32,24,1024,True),('spatial_gradient',2048,16,33,False)]:
        p=project(name,width,height,n,temporal);stride=width*height*3
        reference=bytearray();ideal_means=[]
        # One expected 8x8 tile per gradient value; extend these tiles to complete frames.
        for frame in range(n):
            values=[]
            for v in ([230] if temporal else range(256)):
                pixel=ctx.source([v,v,v,255])
                if temporal:pixel[3]=math.floor(frame/(n-1)*65535+.5)/65535
                opacity=137 if temporal else 64+191*frame/(n-1)
                values.append(ctx.over(ctx.grade(pixel,-315),[0,0,0,1],opacity))
            tiles=[[[output(pixel,x,y) for x in range(8)] for y in range(8)] for pixel in values]
            ideal_means.append([sum(map(sum,t))/64 for t in tiles])
            for y in range(height):
                row=(tiles[0][y%8]*(width//8)) if temporal else [c for tile in tiles for c in tile[y%8]]
                reference.extend(c for c in row for _ in range(3))
        modes={}
        for label,mode in [('legacy',None),('linear16',dict(bits=16,light='linear'))]:
            selected=apply(p,[dict(op='project.working_space',working_space=mode)])
            path=outputs/(name+'-'+label+'.mkv');call('render.run',project=selected,input_root=str(sources),output_root=str(outputs),output=str(path))
            pixels=subprocess.check_output(['ffmpeg','-v','error','-nostdin','-i',str(path),'-an','-pix_fmt','rgb24','-f','rawvideo','-'],timeout=300)
            assert len(pixels)==n*stride
            means=[]
            for frame in range(n):
                view=pixels[frame*stride:(frame+1)*stride]
                means.append([sum(view[(y*width+x)*3] for y in range(8) for x in range(left,left+8))/64 for left in ([0] if temporal else range(0,width,8))])
            series=[row[0] for row in means] if temporal else None
            result=dict(sha256=hashlib.sha256(pixels).hexdigest(),distinct_temporal_means=len(set(series)) if series else None,
                minimum_distinct_spatial_means=min(len(set(row)) for row in means),maximum_distinct_spatial_means=max(len(set(row)) for row in means))
            if mode:
                error=max(abs(a-b) for a,b in zip(pixels,reference));assert error<=1,(name,error)
                assert means==ideal_means,name
                if temporal:assert all(a<b for a,b in zip(series,series[1:])),name
                else:assert all(all(a<b for a,b in zip(row,row[1:])) for row in means),name
                result.update(maximum_delivery_error=error,reference_sha256=hashlib.sha256(reference).hexdigest(),tile_means_exact=True,added_plateaus=0)
            modes[label]=result
        cases.append(dict(name=name,width=width,height=height,frames=n,modes=modes))
    report=dict(passed=['working_banding.saved_grade_opacity_fade_and_spatial_gradient'],cases=cases,
        reference='Independent f64 transfer/exposure/coverage with declared UNORM16 stores and recursive Bayer thresholds; <=1 RGB8 code, exact 8x8 means, strictly increasing ramp means',
        limits='Synthetic SDR sRGB grayscale under one -0.315-stop grade, generated source ramp and saved opacity. Encoded and linear paths intentionally differ in brightness; this is a precision gate, not a subjective quality, HDR or performance claim.')
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
