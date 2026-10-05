"""Codec/container matrix and exact timestamp retiming of original synthetic sources."""
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

from agents import Client
from scenes import identity, time

ROOT=Path(__file__).resolve().parents[1]
W,H=64,32


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources=root/"sources";output=root/"output";store=root/"store"
    for p in (sources,output,store):p.mkdir()
    def ff(args):return subprocess.run(["ffmpeg","-v","error","-nostdin","-n"]+args,capture_output=True,check=True,timeout=90).stdout
    def probe(path,stream):
        result=subprocess.run(["ffprobe","-v","error","-select_streams",stream,"-show_streams","-show_frames","-show_entries","stream=time_base:frame=best_effort_timestamp,duration,nb_samples","-of","json",str(path)],capture_output=True,check=True)
        return json.loads(result.stdout)
    raw=b"".join(bytes((n*7+x*3)%256 if c==0 else (n*11+y*5)%256 if c==1 else (x+y+n*13)%256 for y in range(H) for x in range(W) for c in range(3)) for n in range(75))
    (sources/"original.rgb").write_bytes(raw)
    for rate,channels in ((48000,2),(44100,2),(24000,1)):
        with wave.open(str(sources/f"audio-{rate}-{channels}.wav"),"wb") as f:
            f.setnchannels(channels);f.setsampwidth(2);f.setframerate(rate)
            f.writeframes(b"".join(struct.pack("<"+"h"*channels,*[(n*13+c*917)%18001-9000 for c in range(channels)]) for n in range(rate*4)))
    definitions=[("rgb25.mkv","25",48000,2,False), ("rgb24.mkv","24",44100,2,False), ("fractional.mp4","30000/1001",44100,2,True), ("mono.mov","30",24000,1,True), ("silent.mp4","25",None,None,True)]
    tags=["-colorspace","bt709","-color_primaries","bt709","-color_trc","bt709","-color_range","tv"]
    for name,rate,audio_rate,channels,lossy in definitions:
        args=["-f","rawvideo","-pixel_format","rgb24","-video_size",f"{W}x{H}","-framerate",rate,"-i",str(sources/"original.rgb")]
        if audio_rate:args += ["-i",str(sources/f"audio-{audio_rate}-{channels}.wav")]
        args += ["-map","0:v:0"]
        if audio_rate:args += ["-map","1:a:0","-c:a","aac" if lossy else "pcm_s16le"]
        args += ["-vf","setsar=1","-c:v","libx264" if lossy else "ffv1","-pix_fmt","yuv420p" if lossy else "bgr0","-threads","1"]
        if lossy:args += ["-g","12","-bf","2"]+tags
        args += [str(sources/name)]
        ff(args)
    ff(["-i",str(sources/"rgb25.mkv"),"-map","0:v:0","-vf","settb=1/1000,setpts=N*40+floor(N/5)*20","-fps_mode","passthrough","-enc_time_base","1/1000","-c:v","ffv1","-pix_fmt","bgr0",str(sources/"vfr.mkv")])
    ff(["-i",str(sources/"rgb25.mkv"),"-map","0:v:0","-vf","setsar=2","-c:v","ffv1","-pix_fmt","bgr0",str(sources/"aspect.mkv")])
    ff(["-display_rotation:v:0","90","-i",str(sources/"fractional.mp4"),"-map","0","-c","copy",str(sources/"rotated.mp4")])
    rotation=json.loads(subprocess.run(["ffprobe","-v","error","-show_streams","-select_streams","v:0","-of","json",str(sources/"rotated.mp4")],capture_output=True,check=True).stdout)
    assert any(s.get("rotation")==90 for s in rotation["streams"][0]["side_data_list"])
    ff(["-i",str(sources/"rgb25.mkv"),"-map","0:v:0","-c:v","libx264","-pix_fmt","yuv420p",str(sources/"untagged.mp4")])
    ff(["-i",str(sources/"rgb25.mkv"),"-map","0:v:0","-c:v","ffv1","-pix_fmt","bgr0","-color_trc","smpte2084",str(sources/"hdr.mkv")])
    ff(["-i",str(sources/"rgb25.mkv"),"-map","0:v:0","-vf","setfield=tff","-c:v","ffv1","-pix_fmt","bgr0",str(sources/"interlaced.mkv")])
    ff(["-i",str(sources/"audio-48000-2.wav"),"-ac","4","-c:a","pcm_s16le",str(sources/"surround.wav")])
    (sources/"truncated.mp4").write_bytes((sources/"fractional.mp4").read_bytes()[:97])
    (sources/"malformed.wav").write_bytes(b"RIFF\xff\xff\xff\xffWAVEfmt broken")
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    executable=ENGINE
    passed=[];comparisons=[];expected_outputs={};receipts={}
    def request(value,error=None):
        p=subprocess.run([str(executable)],input=json.dumps(value).encode(),capture_output=True,timeout=120)
        data=json.loads(p.stdout)
        if error:
            assert p.returncode==1 and data["error"]["code"]==error,data
            return data
        assert p.returncode==0 and data["ok"],data
        return data["result"]
    decoded={}
    def reference(name):
        if name in decoded:return decoded[name]
        path=sources/name
        v=probe(path,"v:0");a=probe(path,"a:0")
        rgb=b"";pts=[];pcm=[];rate=0;channels=0;tb=Fraction(0)
        if v["streams"]:
            tb=Fraction(v["streams"][0]["time_base"])
            pts=[Fraction(f["best_effort_timestamp"])*tb for f in v["frames"]]
            args=["-noautorotate","-i",str(path),"-map","0:v:0","-fps_mode","passthrough"]
            if path.suffix in (".mp4",".mov"):args += ["-vf","scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd"]
            rgb=ff(args+["-pix_fmt","rgb24","-f","rawvideo","-"])
            assert len(rgb)==len(pts)*W*H*3
        if a["streams"]:
            meta=json.loads(subprocess.run(["ffprobe","-v","error","-show_streams","-select_streams","a:0","-of","json",str(path)],capture_output=True,check=True).stdout)["streams"][0]
            rate=int(meta["sample_rate"]);channels=meta["channels"]
            pcm=list(struct.iter_unpack("<"+"h"*channels,ff(["-i",str(path),"-map","0:a:0","-c:a","pcm_s16le","-f","s16le","-"])))
        decoded[name]=(rgb,pts,pcm,rate,channels,tb)
        return decoded[name]
    def recipe(name,label,**changes):
        result={"schema_version":1,"id":label,"source":{"file":identity(sources/name,sources),"color":None if name.endswith(".wav") else "bt709_limited" if name.endswith((".mov",".mp4")) else "encoded_rgb"},"source_in":time(0),"duration":time(1),"rate":time(1),"reverse":False,"freeze":False,"width":W,"height":H,"audio":"resample"}
        result.update(changes);return result
    def render(value,label,error=None):return request({"command":"media.conform","recipe":value,"input_root":str(sources),"output_root":str(output),"output":str(output/(label+".mkv"))},error)
    def expected(name,value):
        rgb,pts,pcm,source_rate,channels,tb=reference(name)
        fraction=lambda v:Fraction(v["num"],v["den"])
        start=fraction(value["source_in"]);rate=fraction(value["rate"]);duration=fraction(value["duration"])
        clock=fraction(value.get("frame_rate",time(25)))
        frames=int(duration*clock);width,height=value["width"],value["height"]
        selected=[];pixels=bytearray()
        for n in range(frames):
            t=start if value["freeze"] else start+Fraction(n,1)/clock*rate*(-1 if value["reverse"] else 1)
            if pts:
                # A frame within half a source tick after t is the frame at t (rounded container times).
                i=max(bisect_right(pts,t+tb/2)-1,0);selected.append(i)
                frame=rgb[i*W*H*3:(i+1)*W*H*3]
                for y in range(height):
                    for x in range(width):
                        p=(y*H//height*W+x*W//width)*3;pixels.extend(frame[p:p+3])
            else:pixels.extend(bytes(width*height*3))
        audio=bytearray()
        for n in range(int(duration*48000)):
            pos=(start+Fraction(n,48000)*rate)*source_rate
            if value["audio"]=="mute" or not pcm:audio.extend(bytes(4));continue
            low=int(pos);high=min(low+1,len(pcm)-1);rem=pos-low
            for c in range(2):
                ch=0 if channels==1 else c
                value_=pcm[low][ch]*(1-rem)+pcm[high][ch]*rem
                sample=int(abs(value_)+Fraction(1,2))*(-1 if value_<0 else 1)
                audio.extend(struct.pack("<h",sample))
        return bytes(pixels),bytes(audio),selected
    cases=[(name,name.split('.')[0],{}) for name,*_ in definitions]
    cases += [("audio-24000-1.wav","wav-mono",{}),("audio-44100-2.wav","wav-stereo",{}),("vfr.mkv","variable",{}),
              ("fractional.mp4","fast",{"rate":time(3,2),"source_in":time(1,5)}),
              ("rgb24.mkv","slow",{"rate":time(1,2),"duration":time(2)}),
              ("rgb25.mkv","reverse",{"source_in":time(49,25),"reverse":True,"audio":"mute"}),
              ("rgb25.mkv","freeze",{"source_in":time(27,25),"freeze":True,"audio":"mute"}),
              ("rgb25.mkv","rate-min",{"rate":time(1,16),"duration":time(8,25)}),
              ("rgb25.mkv","rate-max",{"rate":time(16),"duration":time(1,25)}),
              ("rgb25.mkv","resize",{"width":31,"height":17,"duration":time(2,25)}),
              # Native output rates keep every source frame: 30000/1001 H.264, 30 fps MOV and 24 fps FFV1.
              ("fractional.mp4","native-fractional",{"frame_rate":time(30000,1001),"duration":time(1001,1000)}),
              ("mono.mov","native-30",{"frame_rate":time(30),"duration":time(1)}),
              ("rgb24.mkv","native-24",{"frame_rate":time(24),"duration":time(1)})]
    for name,label,changes in cases:
        value=recipe(name,label,**changes)
        plan=request({"command":"media.conform.inspect","recipe":value,"input_root":str(sources)})
        receipt=render(value,label)
        rgb,pcm,indices=expected(name,value)
        actual_rgb=ff(["-i",str(output/(label+".mkv")),"-map","0:v:0","-pix_fmt","rgb24","-f","rawvideo","-"])
        actual_pcm=ff(["-i",str(output/(label+".mkv")),"-map","0:a:0","-f","s16le","-"])
        assert actual_rgb==rgb,label
        assert actual_pcm==pcm,label
        assert receipt["source_frame_indices"]==plan["source_frame_indices"]==indices
        if "frame_rate" in changes:
            assert indices==list(range(len(indices))),(label,indices)
            stream=json.loads(subprocess.run(["ffprobe","-v","error","-select_streams","v:0","-show_entries","stream=r_frame_rate","-of","json",str(output/(label+".mkv"))],capture_output=True,check=True).stdout)["streams"][0]
            assert Fraction(stream["r_frame_rate"])==Fraction(changes["frame_rate"]["num"],changes["frame_rate"]["den"]),stream
        assert receipt["asset"]["identity"]["sha256"]==hashlib.sha256((output/(label+".mkv")).read_bytes()).hexdigest()
        expected_outputs[label]=(rgb,pcm);receipts[label]=receipt
        comparisons.append({"case":label,"source":name,"frames":receipt["frames"],"sample_frames":receipt["samples"]})
    passed.extend(["conform.validated_format_matrix","conform.exact_timestamp_mapping","conform.speed_reverse_freeze","conform.audio_rates_layouts"])
    # One-step preparation: a fitting ready source is returned as it is; anything else is converted
    # with the readiness recipe, identical to running that recipe through media.conform.
    def prepare(name,label=None,project=None,error=None):
        value={"command":"media.prepare","path":str(sources/name),"input_root":str(sources),"output_root":str(output)}
        if label:value["output"]=str(output/(label+".mkv"))
        if project:value["project"]=project
        return request(value,error)
    def frame_digests(path):return ff(["-i",str(path),"-map","0","-f","framemd5","-"])
    p25=request({"command":"project.create","id":"prepare25","width":W,"height":H,"frame_rate":time(25)})
    p30=request({"command":"project.create","id":"prepare30","width":W,"height":H,"frame_rate":time(30)})
    # An earlier conversion is timeline-ready at 25 fps and fits p25, so it comes back unchanged.
    ready=request({"command":"media.prepare","path":str(output/"rgb25.mkv"),"input_root":str(output),"output_root":str(output),"project":p25})
    assert ready["converted"] is False and ready["asset"]["path"]=="rgb25.mkv" and ready["asset"]["duration"]==time(1),ready
    for name,label,project,rate in [("fractional.mp4","prepared-own",None,time(30000,1001)),("rgb25.mkv","prepared-30",p30,time(30))]:
        prepared=prepare(name,label,project)
        assert prepared["converted"] and prepared["frame_rate"]==rate and prepared["recipe"]["width"]==W,prepared
        again=render(prepared["recipe"],label+"-explicit")
        assert frame_digests(output/(label+".mkv"))==frame_digests(output/(label+"-explicit.mkv")) and again["frames"]==prepared["frames"]
    assert prepare("fractional.mp4")["converted"] and (output/"fractional-prepared.mkv").exists()
    prepare("audio-48000-2.wav",error="UNSUPPORTED_MEDIA")
    prepare("hdr.mkv",error="UNSUPPORTED_MEDIA")
    passed.append("conform.one_step_preparation")
    # Batch preparation: one job for several files, IDs unique within the batch and the project,
    # one file's failure reported without stopping the others, and media.add operations returned.
    inputs=root/"batch-sources";(inputs/"sub").mkdir(parents=True)
    for name in ("fractional.mp4","rgb25.mkv","audio-48000-2.wav"):(inputs/name).write_bytes((sources/name).read_bytes())
    (inputs/"sub"/"fractional.mp4").write_bytes((sources/"fractional.mp4").read_bytes())
    batch=output/"batch";batch.mkdir()
    many=request({"command":"media.prepare","paths":["fractional.mp4",str(inputs/"rgb25.mkv"),"audio-48000-2.wav","sub/fractional.mp4"],
                  "input_root":str(inputs),"output_root":str(batch)})
    assert [item.get("asset_id") for item in many["prepared"]]==["fractional","rgb25",None,"fractional-2"],many
    assert many["prepared"][2]["error"]["code"]=="UNSUPPORTED_MEDIA" and many["failed"]==1 and many["converted"]+many["ready"]==3
    assert [o["asset"]["id"] for o in many["operations"]]==["fractional","rgb25","fractional-2"]
    single=output/"single";single.mkdir()
    for item in (many["prepared"][0],many["prepared"][1],many["prepared"][3]):
        alone=request({"command":"media.prepare","path":item["path"],"input_root":str(inputs),"output_root":str(single),"output":str(single/(item["asset_id"]+"-prepared.mkv"))})
        assert alone["converted"]==item["result"]["converted"]
        if alone["converted"]:
            assert frame_digests(single/(item["asset_id"]+"-prepared.mkv"))==frame_digests(batch/(item["asset_id"]+"-prepared.mkv")),item["asset_id"]
    added=request({"command":"timeline.apply","project":p25,"expected_revision":0,"operations":many["operations"]})
    assert sorted(a["id"] for a in added["assets"])==["fractional","fractional-2","rgb25"]
    again=request({"command":"media.prepare","paths":["rgb25.mkv"],"input_root":str(inputs),"output_root":str(batch),"project":added})
    assert again["prepared"][0]["asset_id"]=="rgb25-2" and again["operations"][0]["asset"]["id"]=="rgb25-2"
    for fields in ({"path":str(inputs/"rgb25.mkv"),"paths":["rgb25.mkv"]},{"paths":["rgb25.mkv"],"output":str(batch/"x.mkv")},{"paths":[]},{}):
        request({"command":"media.prepare","input_root":str(inputs),"output_root":str(batch),**fields},"INVALID_ARGUMENT")
    passed.append("conform.batch_preparation")
    # Use conformed media as ordinary identity-bound assets in saved timeline edits.
    project=request({"command":"project.create","id":"conformed","width":W,"height":H,"frame_rate":time(25)})
    request({"command":"session.create","store_root":str(store),"project":project,"request_id":"create"})
    operations=[]
    for label in ("fast","wav-mono","reverse"):
        operations += [{"op":"media.add","asset":receipts[label]["asset"]},{"op":"clip.append","clip":{"id":label,"asset_id":label,"source_in":time(1,5),"duration":time(3,5)}}]
    request({"command":"session.apply","store_root":str(store),"project_id":"conformed","expected_revision":0,"request_id":"add","operations":operations})
    saved=request({"command":"session.get","store_root":str(store),"project_id":"conformed"})
    request({"command":"render.run","project":saved,"input_root":str(output),"output_root":str(output),"output":str(output/"sequence.mkv")})
    assert ff(["-i",str(output/"sequence.mkv"),"-map","0:v:0","-pix_fmt","rgb24","-f","rawvideo","-"])==b"".join(expected_outputs[label][0][5*W*H*3:20*W*H*3] for label in ("fast","wav-mono","reverse"))
    assert ff(["-i",str(output/"sequence.mkv"),"-map","0:a:0","-f","s16le","-"])==b"".join(expected_outputs[label][1][9600*4:38400*4] for label in ("fast","wav-mono","reverse"))
    client=Client(executable)
    try:
        client.initialize()
        value=recipe("fractional.mp4","mcp",rate=time(3,2))
        assert client.call("media.conform.inspect",recipe=value,input_root=str(sources))["source_frame_indices"]==expected("fractional.mp4",value)[2]
    finally:client.close()
    passed.append("conform.saved_session_and_mcp")
    bad=[(recipe("truncated.mp4","bad"),"TOOL_FAILED"),(recipe("malformed.wav","bad"),"TOOL_FAILED"),(recipe("aspect.mkv","bad"),"UNSUPPORTED_MEDIA"),(recipe("rotated.mp4","bad"),"UNSUPPORTED_MEDIA"),(recipe("untagged.mp4","bad"),"UNSUPPORTED_MEDIA")]
    bad.extend((recipe(name,"bad"),"UNSUPPORTED_MEDIA") for name in ("hdr.mkv","interlaced.mkv","surround.wav"))
    base=recipe("rgb25.mkv","bad")
    for changes,code in [({"rate":time(0)},"INVALID_CONFORM"),({"rate":time(17)},"INVALID_CONFORM"),({"reverse":True},"INVALID_CONFORM"),({"freeze":True,"rate":time(2),"audio":"mute"},"INVALID_CONFORM"),({"duration":time(1,26)},"UNALIGNED_TIME"),({"source_in":time(9)},"INVALID_RANGE"),({"source_in":time(1,96000)},"UNALIGNED_TIME"),({"width":4097},"INVALID_CONFORM")]:
        bad.append(({**base,**changes},code))
    bad.extend([(recipe("audio-44100-2.wav","bad",source_in=time(7,2)),"INVALID_RANGE"),
                ({**base,"reverse":True,"audio":"mute"},"INVALID_RANGE"),
                ({**base,"source_in":time(3),"freeze":True,"audio":"mute"},"INVALID_RANGE")])
    wrong=copy.deepcopy(base);wrong["source"]["file"]["sha256"]="0"*64;bad.append((wrong,"MEDIA_CHANGED"))
    wrong=copy.deepcopy(base);wrong["source"]["color"]="bt709_limited";bad.append((wrong,"UNSUPPORTED_MEDIA"))
    wrong=copy.deepcopy(base);wrong["source"]["file"]["path"]="../escape.mkv";bad.append((wrong,"INVALID_PATH"))
    for i,(value,code) in enumerate(bad):
        render(value,f"bad-{i}",code);assert not (output/f"bad-{i}.mkv").exists()
    render(recipe("rgb25.mkv","rgb25"),"rgb25","OUTPUT_EXISTS")
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("conform.malformed_boundaries_preservation")
    report={"passed":passed,"cases":comparisons,"rejected_cases":len(bad),"reference":"All decoded output pixels and PCM compared with independent Fraction clocks, source timestamp lookup and source RGB/PCM; source codec decoding uses external FFmpeg","frames_compared":sum(v["frames"] for v in comparisons)+45,"sample_frames_compared":sum(v["sample_frames"] for v in comparisons)+86400}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    (root/"recipe.json").write_text(json.dumps(recipe("fractional.mp4","fast-example",rate=time(3,2)),indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
