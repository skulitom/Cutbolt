"""Visible sequential edit semantics checked against independent frame/sample lists."""
import argparse
import copy
import hashlib
import json
from pathlib import Path
import struct
import subprocess

from agents import Client
from scenes import decode_check, time

ROOT=Path(__file__).resolve().parents[1]


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources=root/"sources";out=root/"output";store=root/"store"
    for p in (sources,out,store):p.mkdir()
    frame=lambda s,n:bytes([(s*97+n)%256,(n*7)%256,180-s*90])*(32*24)
    audio=[]
    for s in range(2):
        pcm=b"".join(struct.pack("<hh",s*2000+n%1000-500,3000-s*700-n%300) for n in range(480000));audio.append(pcm)
        (sources/f"{s}.rgb").write_bytes(b"".join(frame(s,n) for n in range(250)))
        (sources/f"{s}.pcm").write_bytes(pcm)
        subprocess.run(["ffmpeg","-v","error","-n","-f","rawvideo","-pixel_format","rgb24","-video_size","32x24","-framerate","25","-i",str(sources/f"{s}.rgb"),"-f","s16le","-ar","48000","-ac","2","-i",str(sources/f"{s}.pcm"),"-c:v","ffv1","-level","3","-pix_fmt","bgr0","-threads","1","-c:a","pcm_s16le",str(sources/f"{s}.mkv")],check=True)
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ROOT/"target/debug/cutbolt.exe";passed=[];cases=[];frame_count=0
    def request(value,error=None):
        p=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=120)
        r=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and r["error"]["code"]==error,r
            return r
        assert p.returncode==0 and r["ok"],r
        return r["result"]
    def clip(name,s,n,d):return {"id":name,"asset_id":f"source-{s}","source_in":time(n,25),"duration":time(d,25)}
    base=request({"command":"project.create","id":"editing-fixture","width":32,"height":24,"frame_rate":time(25)})
    base=request({"command":"timeline.apply","project":base,"expected_revision":0,"operations":[
        *[{"op":"media.add","asset":{"id":f"source-{s}","path":str(sources/f"{s}.mkv"),"duration":time(10)}} for s in range(2)],
        *[{"op":"clip.append","clip":c} for c in (clip("left",0,25,50),clip("middle",1,50,50),clip("right",0,100,50))]]})
    serial=json.dumps(base,sort_keys=True)
    span=lambda s,a,b:[(s,n) for n in range(a,b)]
    original=span(0,25,75)+span(1,50,100)+span(0,100,150)
    def edit(op,error=None):return request({"command":"timeline.apply","project":base,"expected_revision":base["revision"],"operations":[op]},error)
    def verify(name,op,expected):
        nonlocal frame_count
        project=edit(op)
        result=request({"command":"render.run","project":project,"input_root":str(sources),"output_root":str(out),"output":str(out/(name+".mkv"))})
        assert result["frames"]==len(expected) and result["samples"]==len(expected)*1920
        decode_check(out/(name+".mkv"),32,24,lambda n:frame(*expected[n]),len(expected))
        actual=subprocess.run(["ffmpeg","-v","error","-i",str(out/(name+".mkv")),"-map","0:a:0","-f","s16le","-"],capture_output=True,check=True).stdout
        assert actual==b"".join(audio[s][n*7680:(n+1)*7680] for s,n in expected)
        assert json.dumps(base,sort_keys=True)==serial
        frame_count+=len(expected);cases.append(name)
        return project
    for at in (0,30,50,150):
        verify(f"insert-{at}",{"op":"clip.insert","at":time(at,25),"clip":clip("inserted",1,5,15),"right_id":"left-tail" if at==30 else None},original[:at]+span(1,5,20)+original[at:])
    verify("overwrite-cross",{"op":"clip.overwrite","at":time(40,25),"clip":clip("replacement",1,150,60)},original[:40]+span(1,150,210)+original[100:])
    verify("overwrite-inside",{"op":"clip.overwrite","at":time(10,25),"clip":clip("replacement",1,5,15),"right_id":"left-tail"},original[:10]+span(1,5,20)+original[25:])
    verify("overwrite-extend",{"op":"clip.overwrite","at":time(140,25),"clip":clip("replacement",1,0,30)},original[:140]+span(1,0,30))
    for start,duration,right in ((30,80,None),(10,20,"left-tail")):
        verify(f"ripple-{start}",{"op":"timeline.ripple_delete","start":time(start,25),"duration":time(duration,25),"right_id":right},original[:start]+original[start+duration:])
    passed.append("editing.insert_overwrite_ripple")
    verify("slip",{"op":"clip.slip","clip_id":"middle","source_in":time(75,25)},span(0,25,75)+span(1,75,125)+span(0,100,150))
    for duration in (30,70):
        delta=duration-50
        verify(f"roll-{duration}",{"op":"clip.roll","left_id":"left","left_duration":time(duration,25)},span(0,25,25+duration)+span(1,50+delta,100)+span(0,100,150))
        verify(f"slide-{duration}",{"op":"clip.slide","clip_id":"middle","previous_duration":time(duration,25)},span(0,25,25+duration)+span(1,50,100)+span(0,100+delta,150))
    passed.append("editing.slip_slide_roll")

    common={"store_root":str(store),"project_id":base["id"]}
    request({"command":"session.create","store_root":str(store),"project":base,"request_id":"create"})
    current=request({"command":"session.get",**common})
    op={"op":"clip.roll","left_id":"left","left_duration":time(70,25)}
    preview=request({"command":"session.preview",**common,"expected_revision":0,"operations":[op]})
    assert preview["duration_before"]==preview["duration_after"]==time(6)
    assert [c["clip_id"] for c in preview["clips"]]==["left","middle"]
    assert preview["clips"][1]["before"]["timeline_start"]==time(2)
    assert preview["clips"][1]["after"]["timeline_start"]==time(14,5)
    assert request({"command":"session.get",**common})==current
    args={"command":"session.apply",**common,"expected_revision":0,"request_id":"roll","operations":[op]}
    assert request(args)==request(args)
    request({"command":"session.undo",**common,"expected_revision":1,"request_id":"undo"})
    assert request({"command":"session.get",**common})["clips"]==base["clips"]
    client=Client(exe)
    try:
        client.initialize()
        for op in ({"op":"clip.slip","clip_id":"middle","source_in":time(75,25)}, {"op":"clip.slide","clip_id":"middle","previous_duration":time(30,25)}):
            diff=client.call("session.preview",**common,expected_revision=2,operations=[op])
            assert diff["clips"] and diff["duration_before"]==diff["duration_after"]
        assert client.call("session.get",**common)["clips"]==base["clips"]
    finally:client.close()
    passed.append("editing.visible_diffs_replay_undo")
    bad=[
        ({"op":"clip.insert","at":time(30,25),"clip":clip("x",1,0,10)},"SPLIT_ID_REQUIRED"),
        ({"op":"clip.insert","at":time(0),"clip":clip("x",1,0,10),"right_id":"unused"},"UNUSED_SPLIT_ID"),
        ({"op":"clip.insert","at":time(7),"clip":clip("x",1,0,10)},"INVALID_RANGE"),
        ({"op":"clip.insert","at":time(1,100),"clip":clip("x",1,0,10)},"UNALIGNED_TIME"),
        ({"op":"clip.insert","at":time(0),"clip":clip("left",1,0,10)},"DUPLICATE_ID"),
        ({"op":"clip.overwrite","at":time(10,25),"clip":clip("x",1,0,10)},"SPLIT_ID_REQUIRED"),
        ({"op":"timeline.ripple_delete","start":time(0),"duration":time(0)},"INVALID_RANGE"),
        ({"op":"timeline.ripple_delete","start":time(5),"duration":time(2)},"INVALID_RANGE"),
        ({"op":"clip.slip","clip_id":"middle","source_in":time(9)},"INVALID_RANGE"),
        ({"op":"clip.roll","left_id":"right","left_duration":time(1)},"INVALID_EDIT"),
        ({"op":"clip.roll","left_id":"left","left_duration":time(4)},"INVALID_RANGE"),
        ({"op":"clip.slide","clip_id":"left","previous_duration":time(1)},"INVALID_EDIT"),
        ({"op":"clip.slide","clip_id":"middle","previous_duration":time(0)},"INVALID_RANGE"),
    ]
    for op,code in bad:edit(op,code)
    invalid_batch={"command":"session.apply",**common,"expected_revision":2,"request_id":"bad-batch","operations":[{"op":"clip.slip","clip_id":"middle","source_in":time(75,25)},bad[-1][0]]}
    request(invalid_batch,"INVALID_RANGE")
    assert request({"command":"session.get",**common})["clips"]==base["clips"]
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    passed.append("editing.invalid_boundaries_atomicity")
    report={"passed":passed,"decoded_frames":frame_count,"pcm_sample_frames":frame_count*1920,"render_cases":cases,"rejected_cases":len(bad)+1,"reference":"Independent flattened source-frame IDs and exact original PCM slices","limits":"This fixture covers one media-only linked audiovisual sequence; tracks, locks, transitions and subframe edits are outside this fixture"}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
