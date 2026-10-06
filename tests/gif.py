"""Exact animated GIF delivery: every decoded pixel against the reference render, palettes, delays and loops."""
from engine import ENGINE, MCP_TOOLS
import argparse
from fractions import Fraction as F
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess

import numpy as np
from agents import Client

ROOT=Path(__file__).resolve().parents[1]
# Neighbours one level apart, near black and near white: a palette that moves a color by one level fails.
NEIGHBOURS=[(31,31,31),(32,31,31),(31,32,31),(31,31,32),(30,31,31),(0,0,0),(1,0,0),(255,255,255),(254,255,255)]


def palette(n,count):
    """`count` distinct colors for frame n, led by the one-level neighbors."""
    chosen=list(NEIGHBOURS[:count]);used=set(chosen);k=0
    while len(chosen)<count:
        color=((k*7+n*13)%256,(k*29+n*3)%256,(n*17+k*5)%256);k+=1
        if color not in used:used.add(color);chosen.append(color)
    return np.array(chosen,dtype=np.uint8)


def art(n,w,h,count):
    """Blocky pixel art with every palette entry present and a moving sprite."""
    y,x=np.indices((h,w));index=(x//3*5+y//2*3+n)%count
    index.flat[:count]=np.arange(count)
    image=palette(n,count)[index]
    side=max(1,min(8,w//4,h//4));left=(n*3)%(w-side);top=2+(n*2)%(h-side-2)
    image[top:top+side,left:left+side]=image[0,0]
    return image.astype(np.uint8)


def lzw(minimum,data,count):
    """GIF's LZW, read independently: codes grow when the table reaches 1 << size, and stop at 12 bits."""
    clear=1<<minimum;size=minimum+1;table=None;previous=None;out=bytearray();value=have=at=0
    while True:
        while have<size:
            assert at<len(data),'image data ended without an end code'
            value|=data[at]<<have;at+=1;have+=8
        code=value&((1<<size)-1);value>>=size;have-=size
        if code==clear:
            table=[bytes([i]) for i in range(clear)]+[b'',b''];size=minimum+1;previous=None;continue
        assert table is not None,'image data must start with a clear code'
        if code==clear+1:break
        if previous is None:entry=table[code]
        else:
            assert code<=len(table),'code beyond the table'
            entry=table[code] if code<len(table) else table[previous]+table[previous][:1]
            if len(table)<4096:
                table.append(table[previous]+entry[:1])
                if len(table)==1<<size and size<12:size+=1
        out+=entry;previous=code
    assert len(out)==count and at==len(data),(len(out),count,at,len(data))
    return bytes(out)


def read_gif(data):
    """Parse and decode a GIF89a file. Returns its size, repeat count, decoded RGB frames, delays and
    each frame's (rectangle, number of distinct colors used)."""
    assert data[:6]==b'GIF89a';w,h,flags,_,_=struct.unpack('<HHBBB',data[6:13]);assert not flags&0x80,'no global table'
    at=13;canvas=np.zeros((h,w,3),np.uint8);frames=[];delays=[];images=[];repeat=None;control=None
    def blocks():
        nonlocal at;parts=[]
        while data[at]:parts.append(data[at+1:at+1+data[at]]);at+=1+data[at]
        at+=1;return parts
    while True:
        kind=data[at];at+=1
        if kind==0x3B:assert at==len(data);break
        if kind==0x21:
            label=data[at];at+=1;parts=blocks()
            if label==0xFF:
                assert parts[0]==b'NETSCAPE2.0' and parts[1][0]==1 and len(parts[1])==3 and repeat is None and not frames
                repeat=struct.unpack('<H',parts[1][1:])[0]
            else:
                assert label==0xF9 and control is None and len(parts)==1 and len(parts[0])==4;control=parts[0]
            continue
        assert kind==0x2C and control is not None,kind
        packed,delay,_=struct.unpack('<BHB',control);control=None
        assert packed==0x04,'keep the previous picture, no transparency'
        left,top,iw,ih,packed=struct.unpack('<HHHHB',data[at:at+9]);at+=9
        assert packed&0x80 and not packed&0x40,'local color table, not interlaced'
        size=2<<(packed&7);table=np.frombuffer(data[at:at+3*size],np.uint8).reshape(size,3);at+=3*size
        minimum=data[at];at+=1;indices=np.frombuffer(lzw(minimum,b''.join(blocks()),iw*ih),np.uint8)
        assert left+iw<=w and top+ih<=h and (frames or (left,top,iw,ih)==(0,0,w,h)) and int(indices.max())<size
        used=np.unique(indices);assert len({tuple(c) for c in table[used]})==len(used),'one entry per color'
        canvas[top:top+ih,left:left+iw]=table[indices.reshape(ih,iw)]
        frames.append(canvas.tobytes());delays.append(delay);images.append(((left,top,iw,ih),len(used)))
    return (w,h),repeat,frames,delays,images


def start(rate,n):
    """Frame n's start in centiseconds: its exact time, rounded half up."""
    exact=F(n)/rate*100;return int(exact+F(1,2))


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents;root.mkdir(parents=True,exist_ok=True)
    source=root/'sources';output=root/'output';source.mkdir();output.mkdir()
    exe=ENGINE;passed=[];exports=[];frames_compared=0;rejected=0;originals={}
    def t(n):n=F(n);return {'num':n.numerator,'den':n.denominator}
    def sha(p):
        with Path(p).open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()
    def call(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({'command':command,**fields}).encode(),capture_output=True,timeout=600)
        v=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and v['error']['code']==error,(error,v);rejected+=1;return v['error']
        assert p.returncode==0 and v['ok'],v;return v['result']
    def decode(path):
        return subprocess.run(['ffmpeg','-v','error','-nostdin','-i',str(path),'-map','0:v:0','-fps_mode','passthrough','-pix_fmt','rgb24','-f','rawvideo','-'],capture_output=True,check=True,timeout=600).stdout
    def create(label,rate,w,h,images):
        """A source movie of `images` (arrays) at `rate`, with silent audio, and a project holding it."""
        video=source/(label+'.rgb');audio=source/(label+'.pcm');movie=source/(label+'.mkv')
        video.write_bytes(b''.join(i.tobytes() for i in images))
        samples=F(len(images))/rate*48000;assert samples.denominator==1;audio.write_bytes(bytes(int(samples)*4))
        subprocess.run(['ffmpeg','-v','error','-nostdin','-n','-f','rawvideo','-pixel_format','rgb24','-video_size',f'{w}x{h}','-framerate',str(rate),'-i',str(video),
                        '-f','s16le','-ar','48000','-ac','2','-i',str(audio),'-c:v','ffv1','-level','3','-pix_fmt','bgr0','-c:a','pcm_s16le',str(movie)],check=True,timeout=600)
        for path in [video,audio,movie]:originals[str(path)]=sha(path)
        p=call('project.create',id=label,width=w,height=h,frame_rate=t(rate))
        return call('timeline.apply',project=p,expected_revision=0,operations=[{'op':'media.add','asset':{'id':'source','path':str(movie),'duration':t(F(len(images))/rate),'identity':{'bytes':movie.stat().st_size,'sha256':sha(movie)}}}])
    def append(p,rate,segments):
        ops=[]
        for i,(first,count) in enumerate(segments):
            clip={'id':f'clip-{i}','source_in':t(F(first or 0)/rate),'duration':t(F(count)/rate)};clip.update({'gap':True} if first is None else {'asset_id':'source'})
            ops.append({'op':'clip.append','clip':clip})
        return call('timeline.apply',project=p,expected_revision=p['revision'],operations=ops)
    def request(p,name,**extra):
        return {'project':p,'input_root':str(source),'output_root':str(output),'output':str(output/(name+'.gif')),'profile':'gif','streams':'video',**extra}
    def export(p,rate,name,images,segments,selected=None,gif=None):
        """Export a GIF and prove it: every pixel against the reference render and the generator."""
        nonlocal frames_compared
        w,h=p['width'],p['height'];timeline=[None if first is None else first+n for first,count in segments for n in range(count)]
        req=request(p,name,**({'gif':gif} if gif is not None else {}),**({'range':selected} if selected else {}))
        planned=call('export.inspect',**req);assert not Path(req['output']).exists()
        receipt=call('export.run',**req);path=Path(req['output'])
        assert receipt['video_frames']==planned['timeline_frames']==len(timeline) and receipt['audio_samples']==0 and receipt['container']=='gif'
        assert receipt['sha256']==sha(path) and receipt['input_transfer'] is None and receipt['video']['codec']=='gif'
        # The lossless reference render of the same range, and the generator's own frames.
        reference=output/(name+'-reference.mkv')
        call('export.run',**{**{k:v for k,v in req.items() if k!='gif'},'profile':'reference','output':str(reference)})
        rendered=decode(reference)
        expected=b''.join(bytes(w*h*3) if n is None else images[n].tobytes() for n in timeline)
        assert rendered==expected,'reference render differs from the generator'
        assert decode(path)==rendered,'FFmpeg decode of the GIF differs from the reference render'
        size,repeat,frames,delays,rectangles=read_gif(path.read_bytes())
        assert size==(w,h) and b''.join(frames)==rendered,'independent decode differs from the reference render'
        assert receipt['verification']['decoded_video_sha256']==hashlib.sha256(rendered).hexdigest()
        # Exact colors: counts per frame and over the animation, against the rendered pixels.
        pictures=np.frombuffer(rendered,np.uint8).reshape(len(timeline),h*w,3)
        packed=pictures[...,0].astype(np.int64)<<16|pictures[...,1].astype(np.int64)<<8|pictures[...,2]
        counts=[len(np.unique(f)) for f in packed]
        assert receipt['video']['colors_per_frame']=={'minimum':min(counts),'maximum':max(counts)}
        assert receipt['video']['distinct_colors']==len(np.unique(packed))
        for (rectangle,used),frame,count in zip(rectangles,packed,counts):assert used<=count<=256
        # Delays and repetition as declared, and the decoder's clock.
        settings=gif or {};plays=settings.get('plays')
        assert repeat==(0 if plays is None else None if plays==1 else plays-1)==receipt['video']['netscape_repeat_count']
        wanted=[start(rate,n+1)-start(rate,n) for n in range(len(timeline))]
        assert delays==wanted and min(wanted)>=2,(delays,wanted)
        assert receipt['video']['delay_centiseconds']=={'minimum':min(wanted),'maximum':max(wanted)}
        assert receipt['video']['duration_centiseconds']==start(rate,len(timeline))
        probe=json.loads(subprocess.check_output(['ffprobe','-v','error','-select_streams','v:0','-show_frames','-show_streams','-of','json',str(path)],timeout=120))
        stream=probe['streams'][0];assert stream['codec_name']=='gif' and stream['time_base']=='1/100' and stream['duration_ts']==start(rate,len(timeline))
        assert [f['best_effort_timestamp'] for f in probe['frames']]==[start(rate,n) for n in range(len(timeline))]
        error=max(abs(F(start(rate,n),100)-F(n)/rate) for n in range(len(timeline)))
        assert F(receipt['video']['maximum_start_error']['num'],receipt['video']['maximum_start_error']['den'])==error
        if settings.get('timing','exact')=='exact':assert error==0
        frames_compared+=len(timeline)
        exports.append({'name':path.name,'rate':t(rate),'dimensions':[w,h],'frames':len(timeline),'bytes':path.stat().st_size,
                        'colors_per_frame':receipt['video']['colors_per_frame'],'distinct_colors':receipt['video']['distinct_colors'],
                        'delays_centiseconds':sorted(set(wanted)),'repeat_count':repeat,'changed_rectangle_frames':sum(r!=(0,0,w,h) for r,_ in rectangles[1:])})
        return receipt

    # A 25 fps pixel-art loop like the demos: 108-122 colors per frame, thousands over the loop, cut
    # with a gap; whole timeline and a range that starts and ends inside clips.
    rate=F(25);w,h=96,64;images=[art(n,w,h,108+(n*7)%15) for n in range(60)]
    p=append(create('loop',rate,w,h,images),rate,[(0,24),(None,4),(30,30)])
    whole=export(p,rate,'loop',images,[(0,24),(None,4),(30,30)])
    assert whole['video']['distinct_colors']>4000 and whole['video']['colors_per_frame']=={'minimum':1,'maximum':122}
    export(p,rate,'loop-range',images,[(5,19),(None,4),(30,7)],selected={'start':t(F(5,25)),'duration':t(F(30,25))})
    # 256 colors of noise per frame fill LZW's table and force clears; one frame repeats (a one-pixel
    # image) and one changes a single pixel.
    rng=np.random.default_rng(20261006);w,h=200,150;noise=[]
    for n in range(5):
        colors=rng.choice(1<<24,256,replace=False);index=rng.integers(0,256,(h,w));index.flat[:256]=np.arange(256)
        noise.append(np.stack(((colors>>16)&255,(colors>>8)&255,colors&255),axis=-1).astype(np.uint8)[index])
    noise[2]=noise[1].copy();noise[3]=noise[2].copy();noise[3][77,123]=noise[3][0,0]
    p=append(create('noise',rate,w,h,noise),rate,[(0,5)])
    receipt=export(p,rate,'noise',noise,[(0,5)])
    assert receipt['video']['colors_per_frame']['maximum']==256 and exports[-1]['changed_rectangle_frames']>=2
    passed.append('gif.every_pixel_matches_the_reference_render')

    # 50 fps plays once; 30 and 24000/1001 fps start each frame at the nearest centisecond.
    for label,rate,gif in [('fifty',F(50),{'plays':1}),('thirty',F(30),{'plays':3,'timing':'nearest_centisecond'}),
                           ('film',F(24000,1001),{'plays':65536,'timing':'nearest_centisecond'})]:
        w,h=40,24;images=[art(n,w,h,20+n%5) for n in range(30)]
        p=append(create(label,rate,w,h,images),rate,[(0,30)])
        export(p,rate,label,images,[(0,30)],gif=gif)
    passed.append('gif.delays_repeat_counts_and_native_rates')

    # More than 256 colors in a frame is refused, never quantized: no output and no scratch.
    rate=F(25);w,h=32,24;images=[art(n,w,h,40) for n in range(8)]
    for n,count in [(3,257),(5,300)]:
        colors=rng.choice(1<<24,count,replace=False);index=np.arange(w*h)%count
        images[n]=np.stack(((colors>>16)&255,(colors>>8)&255,colors&255),axis=-1).astype(np.uint8)[index].reshape(h,w,3)
    p=append(create('colorful',rate,w,h,images),rate,[(0,8)])
    refused=call('export.run',error='TOO_MANY_COLORS',**request(p,'colorful'))
    assert 'Frame 3 of the range (timeline frame 3) has 257 distinct colors' in refused['message'] and '2 of 8 frames' in refused['message'],refused
    refused=call('export.run',error='TOO_MANY_COLORS',**request(p,'colorful',range={'start':t(F(4,25)),'duration':t(F(3,25))}))
    assert 'Frame 1 of the range (timeline frame 5) has 300 distinct colors' in refused['message'] and '1 of 3 frames' in refused['message'],refused
    assert not (output/'colorful.gif').exists() and not list(output.glob('.cutbolt-export-*'))
    export(p,rate,'colorful-allowed',images,[(0,3)],selected={'start':t(0),'duration':t(F(3,25))})
    passed.append('gif.frames_over_256_colors_are_refused')

    # Invalid requests fail at inspection; existing outputs and sources are preserved.
    base=request(p,'invalid')
    def gaps(label,rate,w=8,h=8):
        q=call('project.create',id=label,width=w,height=h,frame_rate=t(rate))
        return call('timeline.apply',project=q,expected_revision=0,operations=[{'op':'clip.append','clip':{'id':'gap','gap':True,'source_in':t(0),'duration':t(F(10)/F(rate))}}])
    for change,code in [({'streams':'audio_video'},'INVALID_EXPORT'),({'streams':'audio'},'INVALID_EXPORT'),({'input_transfer':'bt709'},'INVALID_EXPORT'),
                        ({'gif':{'plays':0}},'INVALID_EXPORT'),({'gif':{'plays':65537}},'INVALID_EXPORT'),({'gif':{'loop':0}},'INVALID_JSON'),
                        ({'gif':{'timing':'fastest'}},'INVALID_JSON'),({'output':str(output/'invalid.mp4')},'UNSUPPORTED_OUTPUT'),
                        ({'profile':'png_mov','input_transfer':'srgb','output':str(output/'invalid.mov'),'gif':{}},'INVALID_EXPORT'),
                        ({'range':{'start':t(F(7,25)),'duration':t(F(2,25))}},'INVALID_EXPORT'),
                        ({'project':gaps('twenty-four',24)},'INVALID_EXPORT'),({'project':gaps('thirty-exact',30)},'INVALID_EXPORT'),
                        ({'project':gaps('sixty',60),'gif':{'timing':'nearest_centisecond'}},'INVALID_EXPORT'),
                        ({'project':gaps('sixty-ntsc',F(60000,1001)),'gif':{'timing':'nearest_centisecond'}},'INVALID_EXPORT'),
                        ({'project':gaps('wide',25,4098,2)},'INVALID_EXPORT'),({'output':str(root/'outside.gif')},'PATH_OUTSIDE_ROOT')]:
        call('export.inspect',error=code,**{**base,**change})
    hint=call('export.inspect',error='INVALID_EXPORT',**{**base,'project':gaps('twenty-four-hint',24)})
    assert 'nearest_centisecond' in hint['message'] and '25/6 cs' in hint['message'],hint
    gaps_only=export(gaps('black',25),F(25),'black',[],[(None,10)])
    assert gaps_only['video']['colors_per_frame']=={'minimum':1,'maximum':1}
    occupied=output/'loop.gif';before=sha(occupied)
    call('export.run',error='OUTPUT_EXISTS',**request(p,'loop'));assert sha(occupied)==before
    client=Client(exe)
    try:
        client.initialize();tools=client.rpc('tools/list')['result']['tools'];assert len(tools)==MCP_TOOLS
        typed=client.call('export.inspect',**request(p,'mcp',gif={'plays':2}))
        assert typed['profile']=='gif' and typed['video']['plays']==2 and typed['video']['netscape_repeat_count']==1
    finally:client.close()
    assert originals=={name:sha(name) for name in originals}
    passed.append('gif.invalid_requests_mcp_and_preservation')
    report={'passed':passed,'exports':exports,'frames_compared':frames_compared,'rejected':rejected,'source_files_preserved':len(originals),
            'reference':'Original pixel-art, noise and one-level-neighbor colors; every frame decoded by FFmpeg and by an independent Python GIF/LZW reader and compared with the lossless reference render; delays, repeat counts and decoder timestamps checked'}
    (root/'verification.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8');print(json.dumps(report,indent=2))


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--output',type=Path,required=True);run(parser.parse_args().output)
