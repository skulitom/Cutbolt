"""Explicit gaps and mixed-source boundaries against independent RGB/PCM timelines."""
import argparse
from bisect import bisect_right
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import wave

from PIL import Image
from agents import Client, until
from scenes import identity, time

ROOT=Path(__file__).resolve().parents[1]
W,H=32,24
SIZE=W*H*3


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,masters,output,store,jobs=[root/name for name in ("sources","masters","output","store","jobs")]
    for p in (sources,masters,output,store,jobs):p.mkdir()
    exe=ROOT/"target/debug/cutbolt.exe";passed=[];compared=0;samples=0;rejected=0;cases=[]
    def ff(args):return subprocess.run(["ffmpeg","-v","error","-nostdin","-n"]+args,capture_output=True,check=True,timeout=120).stdout
    def request(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(exe)],input=json.dumps({"command":command,**fields}).encode(),capture_output=True,timeout=120)
        value=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and value["error"]["code"]==error,(command,value)
            rejected+=1;return value
        assert p.returncode==0 and value["ok"],value
        return value["result"]
    def video(path):return ff(["-i",str(path),"-map","0:v:0","-pix_fmt","rgb24","-f","rawvideo","-"])
    def audio(path):return ff(["-i",str(path),"-map","0:a:0","-f","s16le","-"])
    for name,rate,channels,count in (("stereo.wav",48000,2,144000),("mono.wav",24000,1,7680)):
        with wave.open(str(sources/name),"wb") as f:
            f.setnchannels(channels);f.setsampwidth(2);f.setframerate(rate)
            f.writeframes(b"".join(struct.pack("<"+"h"*channels,*[(n*17+c*931)%22001-11000 for c in range(channels)]) for n in range(count)))
    for name,count,rate,seed in (("a.mkv",48,"24",0),("b.mp4",60,"30000/1001",71)):
        raw=sources/(name+".rgb")
        raw.write_bytes(b"".join(bytes(((n*7+x*3+seed)%256,(n*11+y*5+seed)%256,(n*13+x+y+seed)%256)) for n in range(count) for y in range(H) for x in range(W)))
        lossy=name.endswith("mp4")
        args=["-f","rawvideo","-pixel_format","rgb24","-video_size",f"{W}x{H}","-framerate",rate,"-i",str(raw),"-i",str(sources/"stereo.wav"),"-map","0:v:0","-map","1:a:0","-vf","setsar=1","-c:v","libx264" if lossy else "ffv1","-pix_fmt","yuv420p" if lossy else "bgr0","-c:a","aac" if lossy else "pcm_s16le","-threads","1"]
        if lossy:args += ["-g","12","-keyint_min","12","-sc_threshold","0","-bf","2","-colorspace","bt709","-color_primaries","bt709","-color_trc","bt709","-color_range","tv"]
        ff(args+[str(sources/name)])
    native_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    decoded={}
    def source(name):
        if name in decoded:return decoded[name]
        path=sources/name;rgb=b"";pts=[]
        if not name.endswith("wav"):
            probe=json.loads(subprocess.run(["ffprobe","-v","error","-select_streams","v:0","-show_streams","-show_frames","-show_entries","stream=time_base:frame=best_effort_timestamp,key_frame","-of","json",str(path)],capture_output=True,check=True).stdout)
            tb=Fraction(probe["streams"][0]["time_base"]);pts=[Fraction(f["best_effort_timestamp"])*tb for f in probe["frames"]]
            if name=="b.mp4":assert probe["frames"][11]["key_frame"]==0 and probe["frames"][12]["key_frame"]==1
            args=["-noautorotate","-i",str(path),"-map","0:v:0","-fps_mode","passthrough"]
            if name.endswith("mp4"):args += ["-vf","scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd"]
            rgb=ff(args+["-pix_fmt","rgb24","-f","rawvideo","-"])
        channels=1 if name=="mono.wav" else 2;rate=24000 if channels==1 else 48000
        pcm=list(struct.iter_unpack("<"+"h"*channels,audio(path)))
        decoded[name]=(rgb,pts,pcm,rate,channels);return decoded[name]
    expected={};assets={};recipes={}
    definitions=[("a","a.mkv",time(2),time(0),"resample"),("b","b.mp4",time(51,25),time(0),"resample"),("c","mono.wav",time(8,25),time(0),"resample"),("keycut","b.mp4",time(3,25),time(11011,30000),"mute"),("tail","a.mkv",time(1,25),time(47,24),"resample")]
    for id_,name,duration,start,policy in definitions:
        recipe={"schema_version":1,"id":id_,"source":{"file":identity(sources/name,sources),"color":None if name.endswith("wav") else "bt709_limited" if name.endswith("mp4") else "encoded_rgb"},"source_in":start,"duration":duration,"rate":time(1),"reverse":False,"freeze":False,"width":W,"height":H,"audio":policy}
        receipt=request("media.conform",recipe=recipe,input_root=str(sources),output_root=str(masters),output=str(masters/(id_+".mkv")))
        rgb,pts,pcm,rate,channels=source(name);start=Fraction(start["num"],start["den"]);duration=Fraction(duration["num"],duration["den"])
        indices=[bisect_right(pts,start+Fraction(n,25))-1 for n in range(int(duration*25))] if pts else []
        pixels=b"".join(rgb[n*SIZE:(n+1)*SIZE] for n in indices) if pts else bytes(int(duration*25)*SIZE)
        sound=bytearray()
        for n in range(int(duration*48000)):
            pos=(start+Fraction(n,48000))*rate;low=int(pos);rem=pos-low;high=min(low+1,len(pcm)-1)
            for c in range(2):
                value=Fraction(0) if policy=="mute" else pcm[low][min(c,channels-1)]*(1-rem)+pcm[high][min(c,channels-1)]*rem
                sample=int(abs(value)+Fraction(1,2))*(-1 if value<0 else 1);sound.extend(struct.pack("<h",sample))
        assert receipt["source_frame_indices"]==indices
        assert video(receipt["output"])==pixels and audio(receipt["output"])==sound
        if id_=="keycut":assert indices==[11,12,13]
        expected[id_]=(pixels,bytes(sound));assets[id_]=receipt["asset"];recipes[id_]=recipe
        compared+=int(duration*25);samples+=int(duration*48000)
    def gap(id_,frames):return {"id":id_,"gap":True,"source_in":time(0),"duration":time(frames,25)}
    def clip(id_,asset,start,frames):return {"id":id_,"asset_id":asset,"source_in":time(start,25),"duration":time(frames,25)}
    span=lambda id_,start,count:[(id_,n) for n in range(start,start+count)]
    base=request("project.create",id="timeline-edges",width=W,height=H,frame_rate=time(25))
    contents=[gap("g0",3),clip("a-clip","a",1,5),gap("g1",6),clip("b-clip","b",9,10),clip("c-clip","c",0,8),gap("g2",2)]
    base=request("timeline.apply",project=base,expected_revision=0,operations=[*({"op":"media.add","asset":a} for a in assets.values()),*({"op":"clip.append","clip":c} for c in contents)])
    original=[None]*3+span("a",1,5)+[None]*6+span("b",9,10)+span("c",0,8)+[None]*2
    def reference(tokens):
        pixels=b"".join(bytes(SIZE) if token is None else expected[token[0]][0][token[1]*SIZE:(token[1]+1)*SIZE] for token in tokens)
        sound=b"".join(bytes(7680) if token is None else expected[token[0]][1][token[1]*7680:(token[1]+1)*7680] for token in tokens)
        return pixels,sound
    def verify(name,project,tokens):
        nonlocal compared,samples
        path=output/(name+".mkv");receipt=request("render.run",project=project,input_root=str(root),output_root=str(output),output=str(path))
        pixels,sound=reference(tokens)
        assert receipt["frames"]==len(tokens) and receipt["samples"]==len(tokens)*1920
        assert video(path)==pixels and audio(path)==sound,name
        compared+=len(tokens);samples+=len(tokens)*1920;cases.append(name)
        return receipt
    def edited(op):return request("timeline.apply",project=base,expected_revision=base["revision"],operations=[op])
    verify("base",base,original)
    edits=[
        ("insert-in-gap",{"op":"clip.insert","at":time(10,25),"clip":clip("new","b",28,3),"right_id":"g-tail"},original[:10]+span("b",28,3)+original[10:]),
        ("gap-in-gap",{"op":"clip.insert","at":time(10,25),"clip":gap("new",2),"right_id":"g-tail"},original[:10]+[None]*2+original[10:]),
        ("gap-in-media",{"op":"clip.insert","at":time(5,25),"clip":gap("new",4),"right_id":"a-tail"},original[:5]+[None]*4+original[5:]),
        ("overwrite-with-gap",{"op":"clip.overwrite","at":time(18,25),"clip":gap("new",3),"right_id":"b-tail"},original[:18]+[None]*3+original[21:]),
        ("overwrite-gap-boundary",{"op":"clip.overwrite","at":time(12,25),"clip":clip("new","a",20,5)},original[:12]+span("a",20,5)+original[17:]),
        ("overwrite-extend",{"op":"clip.overwrite","at":time(32,25),"clip":clip("new","c",0,8)},original[:32]+span("c",0,8)),
        ("ripple-in-gap",{"op":"timeline.ripple_delete","start":time(9,25),"duration":time(3,25),"right_id":"g-tail"},original[:9]+original[12:]),
        ("ripple-cross-gap",{"op":"timeline.ripple_delete","start":time(6,25),"duration":time(11,25)},original[:6]+original[17:]),
        ("split-gap",{"op":"clip.split","clip_id":"g1","new_clip_id":"g-tail","offset":time(2,25)},original),
        ("trim-gap",{"op":"clip.trim","clip_id":"g1","source_in":time(0),"duration":time(2,25)},original[:8]+[None]*2+original[14:]),
        ("move-gap",{"op":"clip.move","clip_id":"g1","to_index":4},original[:8]+original[14:32]+[None]*8),
        ("remove-gap",{"op":"clip.remove","clip_id":"g1"},original[:8]+original[14:]),
        ("roll-gap-left",{"op":"clip.roll","left_id":"g1","left_duration":time(4,25)},original[:8]+[None]*4+span("b",7,12)+original[24:]),
        ("roll-gap-right",{"op":"clip.roll","left_id":"a-clip","left_duration":time(7,25)},[None]*3+span("a",1,7)+[None]*4+original[14:]),
        ("slide-gap",{"op":"clip.slide","clip_id":"g1","previous_duration":time(3,25)},[None]*3+span("a",1,3)+[None]*6+span("b",7,12)+original[24:]),
    ]
    for name,op,tokens in edits:
        project=edited(op);verify(name,project,tokens)
        assert all(c["source_in"]==time(0) and "asset_id" not in c for c in project["clips"] if c.get("gap"))
    passed.extend(["timeline_edges.explicit_gaps","timeline_edges.insert_overwrite_ripple"])
    # Exact first/last frames and keyframe crossing after explicit native conversion.
    for asset,start,count in (("a",0,1),("a",49,1),("b",9,10),("b",50,1),("c",0,1),("c",7,1),("keycut",0,3),("tail",0,1)):
        project=copy.deepcopy(base);project["clips"]=[clip("trim",asset,0,len(expected[asset][0])//SIZE)]
        project=request("timeline.apply",project=project,expected_revision=project["revision"],operations=[{"op":"clip.trim","clip_id":"trim","source_in":time(start,25),"duration":time(count,25)}])
        verify(f"edge-{asset}-{start}",project,span(asset,start,count))
    passed.append("timeline_edges.mixed_rate_codec_audio_edges")
    # Gap-only output, including odd dimensions, never requires a media file.
    for width,height,count in ((W,H,1),(33,25,3)):
        project=request("project.create",id=f"gap-only-{width}",width=width,height=height,frame_rate=time(25));project["clips"]=[gap("blank",count)]
        path=output/f"gap-only-{width}.mkv"
        result=request("render.run",project=project,input_root=str(root),output_root=str(output),output=str(path))
        assert not result["sources"] and video(path)==bytes(width*height*3*count) and audio(path)==bytes(count*7680)
        compared+=count;samples+=count*1920
    # Frame/range previews at half-open boundaries, including cuts within a gap.
    for frame in (0,2,3,7,8,13,14,23,24,31,32,33):
        path=output/f"frame-{frame}.png"
        result=request("preview.frame",project=base,input_root=str(root),output_root=str(output),output=str(path),time=time(frame,25))
        with Image.open(path) as img:assert img.tobytes()==reference(original[frame:frame+1])[0]
        if original[frame] is None:assert result["gap"] and result["source"] is None and result["source_frame"] is None
        compared+=1
    path=output/"range.mkv"
    request("preview.range",project=base,input_root=str(root),output_root=str(output),output=str(path),start=time(9,25),duration=time(8,25))
    assert (video(path),audio(path))==reference(original[9:17]);compared+=8;samples+=8*1920
    proxy=request("proxy.generate",project=base,expected_revision=base["revision"],asset_id="a",scale=2,input_root=str(root),output_root=str(output),output=str(output/"proxy-a.mkv"))
    project=proxy["project"];project["clips"]=[gap("blank",2),clip("media","a",0,1)]
    project=request("timeline.apply",project=project,expected_revision=project["revision"],operations=[{"op":"preview.proxy","scale":2}])
    path=output/"proxy-gap.png"
    request("preview.frame",project=project,input_root=str(root),output_root=str(output),output=str(path),time=time(0))
    with Image.open(path) as img:assert img.size==(W//2,H//2) and img.tobytes()==bytes(W//2*(H//2)*3)
    compared+=1;passed.append("timeline_edges.gap_previews")
    # Saved diffs/replay/undo and queued mixed output use the same representation.
    request("session.create",store_root=str(store),project=base,request_id="create")
    common={"store_root":str(store),"project_id":base["id"]};op=edits[0][1]
    diff=request("session.preview",**common,expected_revision=0,operations=[op])
    assert diff["duration_before"]==time(34,25) and diff["duration_after"]==time(37,25)
    assert any(c["after"] and c["after"]["clip"].get("gap") for c in diff["clips"])
    mutation={**common,"expected_revision":0,"request_id":"insert","operations":[op]}
    assert request("session.apply",**mutation)==request("session.apply",**mutation)
    request("session.undo",**common,expected_revision=1,request_id="undo")
    restored=request("session.get",**common);assert restored["clips"]==base["clips"]
    (root/"project.json").write_text(json.dumps(restored,indent=2)+"\n",encoding="utf-8")
    restored=json.loads((root/"project.json").read_text(encoding="utf-8"))
    client=Client(exe)
    try:
        client.initialize();assert client.call("project.validate",project=restored)["valid"]
        receipt=client.call("preview.frame",project=restored,input_root=str(root),output_root=str(output),output=str(output/"mcp-gap.png"),time=time(10,25))
        with Image.open(receipt["output"]) as img:assert img.tobytes()==bytes(SIZE)
        compared+=1
        ticket=client.call("render.start",job_root=str(jobs),request_id="gaps",render={"project":restored,"input_root":str(root),"output_root":str(output),"output":str(output/"queued.mkv")})
        done=until(lambda:client.call("job.status",job_root=str(jobs),job_id=ticket["job_id"]),lambda s:s["status"] in {"completed","failed","interrupted","cancelled"})
        assert done["status"]=="completed",done
        assert (video(output/"queued.mkv"),audio(output/"queued.mkv"))==reference(original)
        compared+=34;samples+=34*1920
    finally:client.close()
    passed.append("timeline_edges.saved_and_queued")
    invalid=[({**gap("bad",2),"asset_id":"a"},"INVALID_GAP"),({**gap("bad",2),"source_in":time(1,25)},"INVALID_GAP"),({**gap("bad",2),"gap":False},"MISSING_MEDIA"),(gap("bad",0),"INVALID_RANGE"),({**gap("bad",2),"duration":time(1,26)},"UNALIGNED_TIME")]
    for i,(bad,code) in enumerate(invalid):
        request("session.apply",error=code,**common,expected_revision=2,request_id=f"bad-{i}",operations=[{"op":"clip.append","clip":gap("valid",1)},{"op":"clip.append","clip":bad}])
        assert request("session.get",**common)==restored
    request("timeline.apply",error="INVALID_EDIT",project=base,expected_revision=base["revision"],operations=[{"op":"clip.slip","clip_id":"g1","source_in":time(0)}])
    for asset,start,duration in (("a",50,1),("b",51,1),("c",8,1),("c",7,2)):
        project=copy.deepcopy(base);project["clips"]=[clip("bad",asset,start,duration)]
        request("project.validate",error="INVALID_RANGE",project=project)
    for id_,start,code in (("a",time(2),"INVALID_RANGE"),("b",time(1001,500),"INVALID_RANGE"),("c",time(8,25),"INVALID_RANGE"),("c",time(1,48000),"UNALIGNED_TIME")):
        recipe={**recipes[id_],"source_in":start,"duration":time(1,25)}
        request("media.conform",error=code,recipe=recipe,input_root=str(sources),output_root=str(output),output=str(output/f"bad-source-{id_}-{start['num']}.mkv"))
    gap_only=copy.deepcopy(base);gap_only["clips"]=[gap("large",180001)]
    request("render.run",error="LIMIT_EXCEEDED",project=gap_only,input_root=str(root),output_root=str(output),output=str(output/"large.mkv"))
    gap_only["clips"]=[gap("only",1)]
    for invalid_root in ("relative",str(root/"absent")):
        request("render.run",error="INVALID_PATH",project=gap_only,input_root=invalid_root,output_root=str(output),output=str(output/"bad-root.mkv"))
    request("render.run",error="OUTPUT_EXISTS",project=base,input_root=str(root),output_root=str(output),output=str(output/"base.mkv"))
    assert not list(output.glob("bad-*.mkv")) and not (output/"large.mkv").exists()
    assert native_hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    for a in assets.values():assert hashlib.sha256(Path(a["path"]).read_bytes()).hexdigest()==a["identity"]["sha256"]
    assert not list(root.rglob(".cutbolt-scene-*")) and not list(output.glob("*.partial.mkv"))
    passed.append("timeline_edges.invalid_atomicity_preservation")
    report={"passed":passed,"decoded_frames":compared,"pcm_sample_frames":samples,"rejected_cases":rejected,"edit_cases":cases,"reference":"Independent Fraction native clocks, decoded source RGB/PCM and explicit frame/sample lists with black/silent gaps; codec decoding uses external FFmpeg"}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
