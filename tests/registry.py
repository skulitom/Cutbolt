"""Independent registry/relink/session and bounded 1000-clip acceptance checks."""
from engine import ENGINE
import argparse
import budgets
import copy
import ctypes
from ctypes import wintypes
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import time as clock

from agents import Client
from scenes import decode_check, time

ROOT=Path(__file__).resolve().parents[1]


def peak_memory(pid):
    class Counters(ctypes.Structure):
        _fields_=[("cb",wintypes.DWORD),("faults",wintypes.DWORD)]+[(name,ctypes.c_size_t) for name in ("peak","working","peak_pool","pool","peak_nonpaged","nonpaged","pagefile","peak_pagefile")]
    kernel=ctypes.WinDLL("kernel32",use_last_error=True)
    kernel.OpenProcess.argtypes=[wintypes.DWORD,wintypes.BOOL,wintypes.DWORD];kernel.OpenProcess.restype=wintypes.HANDLE
    kernel.CloseHandle.argtypes=[wintypes.HANDLE]
    psapi=ctypes.WinDLL("psapi",use_last_error=True)
    psapi.GetProcessMemoryInfo.argtypes=[wintypes.HANDLE,ctypes.POINTER(Counters),wintypes.DWORD]
    handle=kernel.OpenProcess(0x410,False,pid)
    assert handle,ctypes.get_last_error()
    try:
        counters=Counters();counters.cb=ctypes.sizeof(counters)
        assert psapi.GetProcessMemoryInfo(handle,ctypes.byref(counters),counters.cb)
        return counters.peak
    finally:
        kernel.CloseHandle(handle)


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources=root/"sources";moved=root/"moved";output=root/"output";store=root/"store"
    for p in (sources,moved,output,store):p.mkdir()
    frames=lambda n:bytes([n*7%256,30,190])*(32*24)
    pcm=b"".join(struct.pack("<hh",(n*53)%10000-5000,4000-(n*37)%8000) for n in range(48000))
    (sources/"video.rgb").write_bytes(b"".join(frames(n) for n in range(25)))
    (sources/"audio.pcm").write_bytes(pcm)
    subprocess.run(["ffmpeg","-v","error","-n","-f","rawvideo","-pixel_format","rgb24","-video_size","32x24","-framerate","25","-i",str(sources/"video.rgb"),"-f","s16le","-ar","48000","-ac","2","-i",str(sources/"audio.pcm"),"-c:v","ffv1","-level","3","-pix_fmt","bgr0","-threads","1","-c:a","pcm_s16le",str(sources/"source.mkv")],check=True)
    (sources/"wrong.bin").write_bytes(b"original wrong candidate")
    source_hash=hashlib.sha256((sources/"source.mkv").read_bytes()).hexdigest()
    exe=ENGINE;passed=[]
    def request(value,error=None):
        reply=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=120)
        data=json.loads(reply.stdout)
        if error:
            assert reply.returncode==1 and data["error"]["code"]==error,data
            return data
        assert reply.returncode==0 and data["ok"],data
        return data["result"]
    def apply(project,ops,error=None):
        return request({"command":"timeline.apply","project":project,"expected_revision":project["revision"],"operations":ops},error)
    project=request({"command":"project.create","id":"registry-fixture","width":32,"height":24,"frame_rate":time(25)})
    asset={"id":"source","path":str(sources/"source.mkv"),"duration":time(1)}
    metadata={"title":"ÉCLAIR title", "bin":["Series","Episode 1"],"tags":["approved","dialogue"],"fields":{"rights":"original synthetic fixture","note":"雪 café"}}
    project=apply(project,[{"op":"media.add","asset":asset},{"op":"clip.append","clip":{"id":"clip","asset_id":"source","source_in":time(0),"duration":time(1)}},{"op":"media.metadata","asset_id":"source","metadata":metadata}])
    search=lambda p,q:request({"command":"registry.search","project":p,"query":q})
    for query in ({"text":"éclair"},{"text":"雪"},{"bin":["Series"],"tags":["approved"]},{"text":"synthetic"}):
        assert search(project,query)["assets"][0]["metadata"]==metadata
    assert search(project,{"bin":["Series 2"]})["total"]==0
    assert search(project,{"tags":["Approved"]})["total"]==0
    assert search(json.loads(json.dumps(project)),{})["assets"]==project["assets"]
    passed.append("registry.organize_search")
    common={"store_root":str(store),"project_id":project["id"]}
    request({"command":"session.create","store_root":str(store),"project":project,"request_id":"create"})
    saved=request({"command":"session.get",**common})
    binding=request({"command":"registry.bind","project":saved,"expected_revision":0,"input_root":str(sources),"asset_ids":["source"]})
    expected={"sha256":source_hash,"bytes":(sources/"source.mkv").stat().st_size}
    assert binding["project"]["assets"][0]["identity"]==expected
    receipt=request({"command":"session.apply",**common,"expected_revision":0,"request_id":"bind","operations":binding["operations"]})
    assert receipt["changes"]["modified_assets"]==["source"]
    pinned=request({"command":"session.get",**common})
    duplicate=copy.deepcopy(pinned["assets"][0]);duplicate["id"]="duplicate"
    with_duplicate=apply(pinned,[{"op":"media.add","asset":duplicate}])
    assert search(with_duplicate,{})["duplicates"]==[{"identity":expected,"asset_ids":["duplicate","source"]}]
    apply(with_duplicate,[{"op":"media.add","asset":duplicate}],"DUPLICATE_ID")
    apply(pinned,[{"op":"media.metadata","asset_id":"source","metadata":{"tags":["x","x"]}}],"INVALID_METADATA")
    apply(pinned,[{"op":"media.metadata","asset_id":"source","metadata":{"title":"x"*1025}}],"INVALID_METADATA")
    apply(pinned,[{"op":"media.metadata","asset_id":"source","metadata":{"surprise":1}}],"INVALID_JSON")
    passed.append("registry.metadata_roundtrip_duplicates")

    assert request({"command":"registry.status","project":pinned,"input_root":str(sources)})["assets"][0]["state"]=="online"
    # Only original generated test media is moved; saved projects retain their old paths.
    shutil.move(str(sources/"source.mkv"),str(moved/"source.mkv"))
    missing=request({"command":"registry.status","project":pinned,"input_root":str(sources)})
    assert missing["assets"][0]["state"]=="missing"
    # A relocated serialized project still detects its old media paths and can propose relinks.
    (sources/"project.json").write_text(json.dumps(pinned,ensure_ascii=False),encoding="utf-8")
    shutil.move(str(sources/"project.json"),str(moved/"project.json"))
    pinned=json.loads((moved/"project.json").read_text(encoding="utf-8"))
    assert request({"command":"project.validate","project":pinned})["revision"]==1
    assert request({"command":"registry.status","project":pinned,"input_root":str(moved)})["assets"][0]["state"]=="missing"
    shutil.copyfile(moved/"source.mkv",moved/"copy.mkv")
    (moved/"wrong.bin").write_bytes(b"wrong relocated content")
    relink={"command":"registry.relink","project":pinned,"expected_revision":1,"input_root":str(moved),"asset_id":"source"}
    request({**relink,"candidates":[str(moved/"source.mkv"),str(moved/"copy.mkv")]},"AMBIGUOUS_MEDIA")
    request({**relink,"candidates":[str(moved/"wrong.bin")]},"IDENTITY_MISMATCH")
    request({**relink,"candidates":[str(sources/"wrong.bin")]},"PATH_OUTSIDE_ROOT")
    request({**relink,"expected_revision":0,"candidates":[str(moved/"source.mkv")]},"REVISION_CONFLICT")
    assert request({"command":"session.get",**common})==pinned
    proposal=request({**relink,"candidates":[str(moved/"wrong.bin"),str(moved/"source.mkv"),str(moved/"source.mkv")]})
    preview=request({"command":"session.preview",**common,"expected_revision":1,"operations":proposal["operations"]})
    assert preview["modified_assets"]==["source"] and not preview["clips"]
    args={"command":"session.apply",**common,"expected_revision":1,"request_id":"relink","operations":proposal["operations"]}
    assert request(args)==request(args)
    relocated=request({"command":"session.get",**common})
    assert relocated["assets"][0]["metadata"]==metadata and relocated["assets"][0]["identity"]==expected
    # Explicitly save to a new file; the transported original project is immutable.
    (moved/"relinked-project.json").write_text(json.dumps(relocated,ensure_ascii=False),encoding="utf-8")
    relocated=json.loads((moved/"relinked-project.json").read_text(encoding="utf-8"))
    assert json.loads((moved/"project.json").read_text(encoding="utf-8"))==pinned
    render={"command":"render.run","project":relocated,"input_root":str(moved),"output_root":str(output),"output":str(output/"relinked.mkv")}
    request(render);decode_check(output/"relinked.mkv",32,24,frames,25)
    audio=subprocess.run(["ffmpeg","-v","error","-i",str(output/"relinked.mkv"),"-map","0:a:0","-f","s16le","-"],capture_output=True,check=True).stdout
    assert audio==pcm
    passed.append("registry.missing_relink_render")
    # A forged pure edit cannot bypass the renderer/preview identity check.
    wrong=copy.deepcopy(relocated);wrong["assets"][0]["identity"]["sha256"]="0"*64
    request({**render,"project":wrong,"output":str(output/"wrong.mkv")},"IDENTITY_MISMATCH")
    request({"command":"preview.frame","project":wrong,"input_root":str(moved),"output_root":str(output),"output":str(output/"wrong.png"),"time":time(0)},"IDENTITY_MISMATCH")
    assert request({"command":"registry.status","project":wrong,"input_root":str(moved)})["assets"][0]["state"]=="changed"
    request({"command":"session.undo",**common,"expected_revision":2,"request_id":"undo"})
    assert request({"command":"session.get",**common})["assets"]==pinned["assets"]
    request({"command":"session.restore",**common,"expected_revision":3,"request_id":"restore","target_revision":2})
    assert request({"command":"session.get",**common})["assets"]==relocated["assets"]
    assert hashlib.sha256((moved/"source.mkv").read_bytes()).hexdigest()==source_hash
    passed.append("registry.identity_ambiguity_and_history")

    large=copy.deepcopy(project);large.update(id="large-registry",revision=0,assets=[],clips=[])
    for i in range(1000):
        large["assets"].append({**asset,"id":f"asset-{i:04d}","metadata":{"title":f"Shot {i}","bin":["even" if i%2==0 else "odd"],"tags":["selected"] if i%3==0 else [],"fields":{"index":str(i)}}})
        large["clips"].append({"id":f"clip-{i:04d}","asset_id":f"asset-{i:04d}","source_in":time(0),"duration":time(1,25)})
    client=Client(exe);measurements={}
    try:
        client.initialize()
        start=clock.perf_counter();valid=client.call("project.validate",project=large);measurements["validate_seconds"]=clock.perf_counter()-start
        assert valid["duration"]==time(40)
        ids=[];offset=0
        while True:
            page=client.call("registry.search",project=large,query={"bin":["even"],"limit":73,"offset":offset})
            ids.extend(a["id"] for a in page["assets"])
            if page["next_offset"] is None:break
            offset=page["next_offset"]
        assert ids==[f"asset-{i:04d}" for i in range(0,1000,2)]
        ops=[{"op":"clip.trim","clip_id":f"clip-{i:04d}","source_in":time(1,25),"duration":time(2,25)} for i in range(1000)]
        start=clock.perf_counter();edited=client.call("timeline.apply",project=large,expected_revision=0,operations=ops);measurements["batch_1000_edit_seconds"]=clock.perf_counter()-start
        assert edited["revision"]==1 and all(c["source_in"]==time(1,25) and c["duration"]==time(2,25) for c in edited["clips"])
        assert client.call("project.validate",project=edited)["duration"]==time(80)
        assert client.call("registry.search",project=edited,query={"text":"shot 999"})["assets"][0]["id"]=="asset-0999"
        measurements["peak_working_set_bytes"]=peak_memory(client.process.pid)
        budgets.check(measurements["validate_seconds"]<5 and measurements["batch_1000_edit_seconds"]<5,measurements)
        assert measurements["peak_working_set_bytes"]<256*1024*1024,measurements
    finally:client.close()
    passed.extend(["registry.large_collection_pagination","performance.thousand_clip_edits"])
    assert all(c["source_in"]==time(0) for c in large["clips"])
    request({"command":"registry.search","project":large,"query":{"limit":201}},"INVALID_QUERY")
    assert not (output/"wrong.mkv").exists() and not (output/"wrong.png").exists()
    passed.append("registry.invalid_inputs_and_source_protection")
    report={"passed":passed,"rendered_frames":25,"audio_samples_per_channel":48000,"performance":measurements,"limits":"1000 assets/clips; pure metadata/edit operations; render backend retains 64-clip bound; peak process memory <256 MiB and validate/1000-edit batch <5 s on recorded Windows environment"}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
