"""Original mixed-rate sources, identity-bound proxies, offline edits and final media."""
from engine import ENGINE
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
W,H=64,32


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,masters,proxies,moved,output,store,jobs=[root/p for p in ("sources","masters","proxies","moved","output","store","jobs")]
    for p in (sources,masters,proxies,moved,output,store,jobs):p.mkdir()
    executable=ENGINE
    rejected=0
    def ff(args):return subprocess.run(["ffmpeg","-v","error","-nostdin","-n"]+args,capture_output=True,check=True,timeout=90).stdout
    def probe(path):return json.loads(subprocess.run(["ffprobe","-v","error","-select_streams","v:0","-show_streams","-show_frames","-show_entries","stream=time_base:frame=best_effort_timestamp","-of","json",str(path)],capture_output=True,check=True).stdout)
    def rgb(path):return ff(["-i",str(path),"-map","0:v:0","-pix_fmt","rgb24","-f","rawvideo","-"])
    def pcm(path):return ff(["-i",str(path),"-map","0:a:0","-f","s16le","-"])
    def request(command,error=None,**fields):
        nonlocal rejected
        p=subprocess.run([str(executable)],input=json.dumps({"command":command,**fields}).encode(),capture_output=True,timeout=120)
        v=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and v["error"]["code"]==error,(command,v)
            rejected+=1
            return v
        assert p.returncode==0 and v["ok"],v
        return v["result"]
    def relocate(src,dst):
        assert root in src.resolve().parents and root in dst.resolve().parents and not dst.exists()
        src.rename(dst)
    def downsample(data,scale):
        result=bytearray();size=W*H*3
        for start in range(0,len(data),size):
            for y in range(H//scale):
                for x in range(W//scale):
                    p=start+(y*scale*W+x*scale)*3;result.extend(data[p:p+3])
        return bytes(result)
    raw=b"".join(bytes((n*7+x*3)%256 if c==0 else (n*11+y*5)%256 if c==1 else (x+y+n*13)%256 for y in range(H) for x in range(W) for c in range(3)) for n in range(60))
    (sources/"motion.rgb").write_bytes(raw)
    with wave.open(str(sources/"clock.wav"),"wb") as f:
        f.setnchannels(2);f.setsampwidth(2);f.setframerate(48000)
        f.writeframes(b"".join(struct.pack("<hh",(n*13)%18001-9000,(n*7)%16001-8000) for n in range(144000)))
    for name,rate in (("native24.mkv","24"),("fractional.mp4","30000/1001"),("vfr.mkv","25")):
        lossy=name.endswith("mp4")
        args=["-f","rawvideo","-pixel_format","rgb24","-video_size",f"{W}x{H}","-framerate",rate,"-i",str(sources/"motion.rgb"),"-i",str(sources/"clock.wav"),"-map","0:v:0","-map","1:a:0","-vf","setsar=1"]
        if name=="vfr.mkv":
            args[-1]="setsar=1,settb=1/1000,setpts=N*40+floor(N/5)*20"
            args += ["-fps_mode","passthrough","-enc_time_base","1/1000"]
        args += ["-c:v","libx264" if lossy else "ffv1","-pix_fmt","yuv420p" if lossy else "bgr0","-c:a","aac" if lossy else "pcm_s16le","-threads","1"]
        if lossy:args += ["-g","12","-bf","2","-colorspace","bt709","-color_primaries","bt709","-color_trc","bt709","-color_range","tv"]
        ff(args+[str(sources/name)])
    native_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assets=[];expected={};conversions=[]
    names=("native24.mkv","fractional.mp4","vfr.mkv")
    for i,name in enumerate(names):
        asset_id=f"a{i}";path=sources/name
        recipe={"schema_version":1,"id":asset_id,"source":{"file":identity(path,sources),"color":"bt709_limited" if name.endswith("mp4") else "encoded_rgb"},"source_in":time(0),"duration":time(8,5),"rate":time(1),"reverse":False,"freeze":False,"width":W,"height":H,"audio":"resample"}
        converted=request("media.conform",recipe=recipe,input_root=str(sources),output_root=str(masters),output=str(masters/f"{asset_id}.mkv"))
        v=probe(path);tb=Fraction(v["streams"][0]["time_base"]);pts=[Fraction(f["best_effort_timestamp"])*tb for f in v["frames"]]
        args=["-noautorotate","-i",str(path),"-map","0:v:0","-fps_mode","passthrough"]
        if name.endswith("mp4"):args += ["-vf","scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd"]
        source_rgb=ff(args+["-pix_fmt","rgb24","-f","rawvideo","-"])
        indices=[bisect_right(pts,Fraction(n,25))-1 for n in range(40)]
        full=b"".join(source_rgb[n*W*H*3:(n+1)*W*H*3] for n in indices)
        audio=pcm(path)[:76800*4]
        assert rgb(masters/f"{asset_id}.mkv")==full and pcm(masters/f"{asset_id}.mkv")==audio
        assert converted["source_frame_indices"]==indices
        expected[asset_id]=(full,audio);assets.append(converted["asset"]);conversions.append({"source":name,"indices":indices})
    project=request("project.create",id="proxy-demo",width=W,height=H,frame_rate=time(25))
    request("session.create",store_root=str(store),project=project,request_id="create")
    def saved():return request("session.get",store_root=str(store),project_id="proxy-demo")
    serial=0
    def commit(operations):
        nonlocal serial
        serial+=1;p=saved()
        fields={"store_root":str(store),"project_id":p["id"],"expected_revision":p["revision"],"request_id":f"edit-{serial}","operations":operations}
        receipt=request("session.apply",**fields)
        assert request("session.apply",**fields)==receipt
        return receipt
    ranges=[(3,12),(7,15),(11,10)]
    operations=[]
    for asset_,(start,count) in zip(assets,ranges):
        operations += [{"op":"media.add","asset":asset_},{"op":"clip.append","clip":{"id":asset_["id"],"asset_id":asset_["id"],"source_in":time(start,25),"duration":time(count,25)}}]
    commit(operations);unattached=saved()
    assert all(a["state"]=="no_proxy" for a in request("proxy.status",project=unattached,input_root=str(root))["assets"])
    operations=[];bindings={}
    for asset_ in assets:
        receipt=request("proxy.generate",project=unattached,expected_revision=unattached["revision"],asset_id=asset_["id"],scale=2,input_root=str(root),output_root=str(proxies),output=str(proxies/(asset_["id"]+".mkv")))
        binding=receipt["proxy"];bindings[asset_["id"]]=binding;operations+=receipt["operations"]
        full,audio=expected[asset_["id"]]
        assert binding["source_identity"]==asset_["identity"] and binding["frames"]==40
        assert rgb(binding["path"])==downsample(full,2) and pcm(binding["path"])==audio
    attached=commit(operations)
    assert sorted(attached["changes"]["modified_assets"])==["a0","a1","a2"] and not attached["changes"]["clips"]
    p=saved();select=[{"op":"preview.proxy","scale":2}]
    diff=request("session.preview",store_root=str(store),project_id=p["id"],expected_revision=p["revision"],operations=select)
    assert diff["preview"]=={"scale_before":None,"scale_after":2} and diff["duration_before"]==diff["duration_after"] and not diff["clips"]
    selected=commit(select);selected_revision=selected["revision"]
    request("session.undo",store_root=str(store),project_id=p["id"],expected_revision=selected_revision,request_id="undo-mode")
    assert saved().get("preview_scale") is None
    request("session.restore",store_root=str(store),project_id=p["id"],expected_revision=saved()["revision"],target_revision=selected_revision,request_id="restore-mode")
    assert saved()["preview_scale"]==2
    passed=["proxy.generate_bind_select","proxy.session_history_replay"]
    frames_compared=240;samples_compared=460800
    sequence=b"".join(expected[f"a{i}"][0][start*W*H*3:(start+count)*W*H*3] for i,(start,count) in enumerate(ranges))
    sequence_audio=b"".join(expected[f"a{i}"][1][start*1920*4:(start+count)*1920*4] for i,(start,count) in enumerate(ranges))
    def preview(label):
        nonlocal frames_compared,samples_compared
        p=saved();path=output/f"{label}.mkv"
        receipt=request("preview.range",project=p,input_root=str(root),output_root=str(output),output=str(path),start=time(0),duration=time(37,25))
        scale=p.get("preview_scale",1)
        assert receipt["source_quality"]==("proxy" if scale!=1 else "original")
        assert (receipt["width"],receipt["height"])==(W//scale,H//scale)
        assert rgb(path)==downsample(sequence,scale) and pcm(path)==sequence_audio
        frames_compared+=37;samples_compared+=71040
    preview("proxy-preview")
    for scale in (2,None):
        if scale is None:commit([{"op":"preview.proxy","scale":None}])
        receipt=request("preview.frame",project=saved(),input_root=str(root),output_root=str(output),output=str(output/f"still-{scale}.png"),time=time(12,25))
        full=expected["a1"][0][7*W*H*3:8*W*H*3]
        with Image.open(receipt["output"]) as img:assert img.tobytes()==downsample(full,scale or 1)
        assert receipt["source_frame"]==7 and receipt["clip_id"]=="a1"
        frames_compared+=1
    preview("original-preview");commit(select)
    passed.append("proxy.preview_frame_range")
    # All declared proxy scales preserve full frame/sample clocks.
    for scale in (4,8):
        receipt=request("proxy.generate",project=unattached,expected_revision=unattached["revision"],asset_id="a0",scale=scale,input_root=str(root),output_root=str(proxies),output=str(proxies/f"scale-{scale}.mkv"))
        assert rgb(receipt["output"])==downsample(expected["a0"][0],scale) and pcm(receipt["output"])==expected["a0"][1]
        frames_compared+=40;samples_compared+=76800
        one=receipt["project"];one["clips"]=one["clips"][:1]
        one=request("timeline.apply",project=one,expected_revision=one["revision"],operations=[{"op":"preview.proxy","scale":scale}])
        path=output/f"scale-{scale}-preview.mkv"
        request("preview.range",project=one,input_root=str(root),output_root=str(output),output=str(path),start=time(0),duration=time(12,25))
        assert rgb(path)==downsample(expected["a0"][0][3*W*H*3:15*W*H*3],scale)
        assert pcm(path)==expected["a0"][1][3*1920*4:15*1920*4]
        frames_compared+=12;samples_compared+=23040
    # Master can be offline during proxy preview; export must require the master.
    relocate(masters/"a1.mkv",moved/"master-a1.mkv")
    preview("offline-master-preview")
    request("render.run",error="IO_ERROR",project=saved(),input_root=str(root),output_root=str(output),output=str(output/"missing-master.mkv"))
    assert not (output/"missing-master.mkv").exists()
    relocate(proxies/"a1.mkv",moved/"proxy-a1.mkv")
    client=Client(executable)
    try:
        client.initialize()
        states=client.call("proxy.status",project=saved(),input_root=str(root))["assets"]
        assert next(a for a in states if a["asset_id"]=="a1")["state"]=="missing"
        fields={"project":saved(),"expected_revision":saved()["revision"],"input_root":str(root),"asset_id":"a1"}
        client.call("proxy.relink",expected_error="IDENTITY_MISMATCH",candidates=[str(proxies/"a0.mkv")],**fields)
        rejected+=1
        (moved/"proxy-copy.mkv").write_bytes((moved/"proxy-a1.mkv").read_bytes())
        client.call("proxy.relink",expected_error="AMBIGUOUS_MEDIA",candidates=[str(moved/"proxy-copy.mkv"),str(moved/"proxy-a1.mkv")],**fields)
        rejected+=1
        proposed=client.call("proxy.relink",candidates=[str(moved/"proxy-a1.mkv")],**fields);commit(proposed["operations"])
        p=saved();master=client.call("registry.relink",project=p,expected_revision=p["revision"],input_root=str(root),asset_id="a1",candidates=[str(moved/"master-a1.mkv")]);commit(master["operations"])
        preview("relinked-preview")
        passed.append("proxy.relink_mixed_rate_identity")
        (root/"project.json").write_text(json.dumps(saved(),indent=2)+"\n",encoding="utf-8")
        restored=json.loads((root/"project.json").read_text(encoding="utf-8"));request("project.validate",project=restored)
        relocate(proxies/"a2.mkv",moved/"proxy-a2.mkv")
        request("preview.range",error="IO_ERROR",project=restored,input_root=str(root),output_root=str(output),output=str(output/"missing-proxy.mkv"),start=time(0),duration=time(37,25))
        fields={"project":restored,"input_root":str(root),"output_root":str(output),"output":str(output/"final.mkv")}
        plan=request("render.plan",**fields)
        final=request("render.run",**fields)
        assert plan["source_quality"]==final["source_quality"]=="original"
        assert {s["sha256"] for s in final["sources"]}=={a["identity"]["sha256"] for a in assets}
        assert rgb(output/"final.mkv")==sequence and pcm(output/"final.mkv")==sequence_audio
        fields["output"]=str(output/"queued-final.mkv")
        ticket=client.call("render.start",job_root=str(jobs),request_id="proxy-mode-final",render=fields)
        done=until(lambda:client.call("job.status",job_root=str(jobs),job_id=ticket["job_id"]),lambda s:s["status"] in {"completed","failed","interrupted","cancelled"})
        assert done["status"]=="completed" and done["result"]["source_quality"]=="original",done
        assert rgb(output/"queued-final.mkv")==sequence and pcm(output/"queued-final.mkv")==sequence_audio
        frames_compared+=74;samples_compared+=142080
        passed.append("proxy.final_quality_export")
    finally:client.close()
    # Invalid bindings fail atomically without changing saved revisions.
    before=saved();bad=[]
    for key,value,code in (("frames",41,"INVALID_PROXY"),("scale",3,"INVALID_PROXY"),("source_identity",{"bytes":1,"sha256":"0"*64},"IDENTITY_MISMATCH")):
        binding={**bindings["a0"],key:value}
        bad.append(([{"op":"media.proxy.attach","asset_id":"a0","proxy":binding}],code))
    bad.append(([{"op":"preview.proxy","scale":3}],"INVALID_PROXY"))
    for i,(ops,code) in enumerate(bad):
        request("session.apply",error=code,store_root=str(store),project_id=before["id"],expected_revision=before["revision"],request_id=f"bad-{i}",operations=ops)
        assert saved()==before
    generate={"project":unattached,"expected_revision":unattached["revision"],"asset_id":"a0","scale":2,"input_root":str(root),"output_root":str(proxies),"output":str(proxies/"bad.mkv")}
    request("proxy.generate",error="REVISION_CONFLICT",**{**generate,"expected_revision":99})
    changed=copy.deepcopy(unattached);changed["revision"]=9007199254740990
    request("proxy.generate",error="LIMIT_EXCEEDED",**{**generate,"project":changed,"expected_revision":changed["revision"]})
    request("proxy.generate",error="INVALID_PROXY",**{**generate,"scale":3})
    changed=copy.deepcopy(unattached);changed["assets"][0].pop("identity")
    request("proxy.generate",error="IDENTITY_REQUIRED",**{**generate,"project":changed})
    changed=copy.deepcopy(unattached);changed["assets"][0]["duration"]=time(1)
    request("proxy.generate",error="INVALID_PROXY",**{**generate,"project":changed})
    request("proxy.generate",error="OUTPUT_EXISTS",**{**generate,"output":str(proxies/"a0.mkv")})
    missing=copy.deepcopy(before);missing["assets"][0].pop("proxy")
    request("preview.frame",error="PROXY_MISSING",project=missing,input_root=str(root),output_root=str(output),output=str(output/"no-binding.png"),time=time(0))
    changed=copy.deepcopy(before);changed["assets"][0]["proxy"]["scale"]=4
    request("preview.frame",error="INVALID_PROXY",project=changed,input_root=str(root),output_root=str(output),output=str(output/"wrong-scale.png"),time=time(0))
    (moved/"changed-proxy.mkv").write_bytes((proxies/"a0.mkv").read_bytes()+b"changed")
    changed=copy.deepcopy(before);changed["assets"][0]["proxy"]["path"]=str(moved/"changed-proxy.mkv")
    request("preview.frame",error="IDENTITY_MISMATCH",project=changed,input_root=str(root),output_root=str(output),output=str(output/"changed-proxy.png"),time=time(0))
    detached=commit([{"op":"media.proxy.detach","asset_id":"a0"}])
    assert "proxy" not in saved()["assets"][0] and detached["changes"]["modified_assets"]==["a0"]
    request("session.undo",store_root=str(store),project_id=before["id"],expected_revision=detached["revision"],request_id="undo-detach")
    assert saved()["assets"]==before["assets"]
    assert not (proxies/"bad.mkv").exists() and not list(root.rglob(".cutbolt-scene-*"))
    assert not any((output/name).exists() for name in ("missing-master.mkv","missing-proxy.mkv","no-binding.png","wrong-scale.png","changed-proxy.png"))
    assert native_hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    for asset_ in saved()["assets"]:
        assert hashlib.sha256(Path(asset_["path"]).read_bytes()).hexdigest()==asset_["identity"]["sha256"]
    for id_,binding in bindings.items():
        path=(proxies/f"{id_}.mkv") if id_=="a0" else (moved/f"proxy-{id_}.mkv")
        assert hashlib.sha256(path.read_bytes()).hexdigest()==binding["identity"]["sha256"]
    assert hashlib.sha256((output/"final.mkv").read_bytes()).hexdigest()==final["sha256"]
    passed.append("proxy.failures_and_preservation")
    report={"passed":passed,"native_conversions":conversions,"scales":[2,4,8],"rejected_cases":rejected,"frames_compared":frames_compared,"sample_frames_compared":samples_compared,"reference":"Independent timestamp selection of decoded native sources and integer spatial sampling; complete decoded PCM, saved/offline/relinked previews and synchronous/queued full-quality exports"}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
