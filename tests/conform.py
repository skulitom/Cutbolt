"""Codec/container matrix and exact timestamp retiming of original synthetic sources, and
audio-only WAV assets on placed audio tracks."""
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

import numpy as np

from agents import Client
from scenes import identity, time

ROOT=Path(__file__).resolve().parents[1]
W,H=64,32


def source_pcm(rate,channels):
    """The synthetic WAV sources' samples, from the formula that wrote them: (frames, channels) int64."""
    n=np.arange(rate*4,dtype=np.int64)[:,None];c=np.arange(channels,dtype=np.int64)[None,:]
    return (n*13+c*917)%18001-9000


def resampled(rate,channels,value):
    """Audio-only output samples for a constant-rate recipe, in exact integer arithmetic: output sample n
    reads source position source_in*rate + n*rate*speed/48000, interpolates linearly and rounds once to
    nearest, ties away from zero. Mono feeds both channels. Returns interleaved little-endian PCM16."""
    pcm=source_pcm(rate,channels)
    start=Fraction(value["source_in"]["num"],value["source_in"]["den"])*rate;assert start.denominator==1
    speed=Fraction(value["rate"]["num"],value["rate"]["den"])
    count=Fraction(value["duration"]["num"],value["duration"]["den"])*48000;assert count.denominator==1
    den=48000*speed.denominator
    position=np.arange(int(count),dtype=np.int64)*rate*speed.numerator
    low=int(start)+position//den;rem=(position%den)[:,None];high=np.minimum(low+1,len(pcm)-1)
    assert low.max()<len(pcm)
    pick=[0,0] if channels==1 else [0,1]
    total=pcm[low][:,pick]*(den-rem)+pcm[high][:,pick]*rem
    out=np.sign(total)*((2*np.abs(total)+den)//(2*den))
    return out.astype("<i2").tobytes()


def wav_pcm(path):
    """A 48 kHz stereo PCM16 WAV's samples, read with Python's own wave module."""
    with wave.open(str(path),"rb") as f:
        assert (f.getnchannels(),f.getsampwidth(),f.getframerate())==(2,2,48000),path
        return f.readframes(f.getnframes())


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
    def render(value,label,error=None,extension="mkv"):return request({"command":"media.conform","recipe":value,"input_root":str(sources),"output_root":str(output),"output":str(output/f"{label}.{extension}")},error)
    def audio_recipe(name,label,**changes):
        value=recipe(name,label,**changes);del value["width"],value["height"];return value
    def sha(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()
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
    # Audio-only output: no width and height make a 48 kHz stereo PCM16 WAV with no picture. Every
    # sample equals the integer resampler above, as Python's wave module and FFmpeg both read it.
    audio_cases=[("audio-24000-1.wav",24000,1,"wav-only-mono",{"duration":time(4)}),
                 ("audio-44100-2.wav",44100,2,"wav-only-fast",{"source_in":time(1,2),"rate":time(3,2),"duration":time(2)}),
                 ("audio-44100-2.wav",44100,2,"wav-only-slow",{"source_in":time(441,44100),"rate":time(1,3),"duration":time(12345,48000)}),
                 ("audio-48000-2.wav",48000,2,"wav-only-slice",{"source_in":time(1,4),"duration":time(3)})]
    for name,rate,channels,label,changes in audio_cases:
        value=audio_recipe(name,label,**changes)
        plan=request({"command":"media.conform.inspect","recipe":value,"input_root":str(sources)})
        receipt=render(value,label,extension="wav")
        pcm=resampled(rate,channels,value)
        path=output/(label+".wav")
        assert wav_pcm(path)==pcm and ff(["-i",str(path),"-map","0:a:0","-f","s16le","-"])==pcm,label
        assert plan["audio_only"] is True and plan["frames"] is None and plan["frame_rate"] is None and plan["width"] is None,plan
        assert receipt["samples"]==len(pcm)//4 and receipt["pcm_sha256"]==hashlib.sha256(pcm).hexdigest(),receipt
        assert receipt["asset"]["identity"]=={"sha256":sha(path),"bytes":path.stat().st_size} and receipt["asset"]["duration"]==value["duration"]
        streams=json.loads(subprocess.run(["ffprobe","-v","error","-show_streams","-of","json",str(path)],capture_output=True,check=True).stdout)["streams"]
        assert [s["codec_type"] for s in streams]==["audio"],streams
        comparisons.append({"case":label,"source":name,"frames":0,"sample_frames":receipt["samples"]})
    # The old resampler path (a picture asset) and the audio-only path give the same samples.
    assert resampled(24000,1,audio_recipe("audio-24000-1.wav","x",duration=time(1)))==expected_outputs["wav-mono"][1]
    wav_only=audio_recipe("audio-24000-1.wav","bad")
    for changes,code in [({"width":W},"INVALID_CONFORM"),({"frame_rate":time(25)},"INVALID_CONFORM"),({"audio":"mute"},"INVALID_CONFORM"),
                         ({"duration":time(1,96000)},"UNALIGNED_TIME"),({"duration":time(5)},"INVALID_RANGE"),({"source_in":time(1,48000)},"UNALIGNED_TIME"),
                         ({"decode":{"backend":"cpu"}},"INVALID_CONFORM")]:
        render({**wav_only,**changes},"bad-audio",code,extension="wav")
    picture_source=audio_recipe("rgb25.mkv","bad")
    render(picture_source,"bad-audio","INVALID_CONFORM",extension="wav")
    render(wav_only,"bad-audio","UNSUPPORTED_OUTPUT")
    render(recipe("audio-24000-1.wav","bad"),"bad-audio","UNSUPPORTED_OUTPUT",extension="wav")
    assert not list(output.glob("bad-audio*"))
    passed.append("conform.audio_only_wav_exact")
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
    # A 48 kHz stereo PCM16 WAV is an audio-only asset as it is, bound to its content; another PCM16
    # WAV is resampled into <id>-prepared.wav, with or without a project. No picture is made.
    ready_wav=prepare("audio-48000-2.wav")
    wav_identity=identity(sources/"audio-48000-2.wav",sources)
    assert ready_wav["converted"] is False and ready_wav["asset"]=={"id":"audio-48000-2","path":"audio-48000-2.wav","duration":time(4),
        "identity":{"sha256":wav_identity["sha256"],"bytes":wav_identity["bytes"]}},ready_wav
    assert prepare("audio-48000-2.wav",project=p30)["asset"]==ready_wav["asset"]
    inspected=request({"command":"media.inspect","path":str(sources/"audio-48000-2.wav"),"input_root":str(sources)})["timeline"]
    assert inspected["ready"] and inspected["audio_only"] and inspected["samples"]==192000 and inspected["asset"]==ready_wav["asset"],inspected
    spoken={"schema_version":1,"id":"voice-words","revision":0,"parent_fingerprint":None,
        "source":{"path":"audio-24000-1.wav","identity":{k:v for k,v in identity(sources/"audio-24000-1.wav",sources).items() if k!="path"},"duration":time(4)},
        "range_start":time(0),"range_duration":time(4),"language":"en",
        "recognition":{"profile":"synthetic-contract-fixture","model":{"sha256":"a"*64,"bytes":1},"worker_sha256":"b"*64,"analysis_sha256":"c"*64,"versions":{"original-fixture":"1"}},
        "words":[{"id":f"w{i}","text":text,"start":time(a,10),"end":time(b,10),"origin":"estimated","probability_milli":900}
                 for i,(text,a,b) in enumerate([("one",5,9),("two",12,16),("three",17,21),("four",30,34)])]}
    prepared_audio={}
    for name,rate,channels,project in (("audio-24000-1.wav",24000,1,None),("audio-44100-2.wav",44100,2,p30)):
        value={"command":"media.prepare","path":str(sources/name),"input_root":str(sources),"output_root":str(output),"transcripts":[spoken]}
        if project:value["project"]=project
        prepared=request(value)
        path=output/(name[:-4]+"-prepared.wav")
        assert prepared["converted"] and Path(prepared["output"]).name==path.name and "width" not in prepared["recipe"],prepared
        assert prepared["samples"]==192000 and wav_pcm(path)==resampled(rate,channels,prepared["recipe"]),name
        assert prepared["asset"]["identity"]=={"sha256":sha(path),"bytes":path.stat().st_size} and prepared["asset"]["duration"]==time(4)
        prepared_audio[name]=prepared
    # The transcript of the 24 kHz take moves onto its asset, bound to the new file's content.
    moved=prepared_audio["audio-24000-1.wav"]["transcripts"]
    assert len(moved)==1 and moved[0]["source"]["identity"]==prepared_audio["audio-24000-1.wav"]["asset"]["identity"],moved
    assert moved[0]["source"]["path"]=="audio-24000-1-prepared.wav" and [w["text"] for w in moved[0]["words"]]==["one","two","three","four"]
    assert prepared_audio["audio-44100-2.wav"]["transcripts"]==[]
    proposed=request({"command":"media.inspect","path":str(sources/"audio-24000-1.wav"),"input_root":str(sources)})["timeline"]
    assert not proposed["ready"] and proposed["conform"]["output"]=="audio-24000-1-conformed.wav" and "width" not in proposed["conform"]["recipe"],proposed
    assert proposed["conform"]["recipe"]["duration"]==time(4) and "media.prepare" in proposed["conform"]["next"]
    # An explicit .mkv output keeps the older kind: the sound under a silent picture of the project's
    # size and rate, which needs the project.
    prepare("audio-48000-2.wav","prepared-wav-unsized",error="UNSUPPORTED_MEDIA")
    voice=prepare("audio-48000-2.wav","prepared-wav",p25)
    assert voice["converted"] and voice["recipe"]["source"]["color"] is None and voice["recipe"]["width"]==W and voice["frame_rate"]==time(25),voice
    again=render(voice["recipe"],"prepared-wav-explicit")
    assert frame_digests(output/"prepared-wav.mkv")==frame_digests(output/"prepared-wav-explicit.mkv") and again["frames"]==voice["frames"]
    prepare("hdr.mkv",error="UNSUPPORTED_MEDIA")
    passed.append("conform.one_step_preparation")
    # Batch preparation: one job for several files, IDs unique within the batch and the project,
    # one file's failure reported without stopping the others, and media.add operations returned.
    inputs=root/"batch-sources";(inputs/"sub").mkdir(parents=True)
    for name in ("fractional.mp4","rgb25.mkv","audio-48000-2.wav","audio-24000-1.wav"):(inputs/name).write_bytes((sources/name).read_bytes())
    (inputs/"sub"/"fractional.mp4").write_bytes((sources/"fractional.mp4").read_bytes())
    batch=output/"batch";batch.mkdir()
    many=request({"command":"media.prepare","paths":["fractional.mp4",str(inputs/"rgb25.mkv"),"audio-48000-2.wav","sub/fractional.mp4","audio-24000-1.wav"],
                  "input_root":str(inputs),"output_root":str(batch)})
    batch_ids=["fractional","rgb25","audio-48000-2","fractional-2","audio-24000-1"]
    assert [item.get("asset_id") for item in many["prepared"]]==batch_ids,many
    assert many["failed"]==0 and many["converted"]+many["ready"]==5 and many["prepared"][2]["result"]["converted"] is False
    assert wav_pcm(batch/"audio-24000-1-prepared.wav")==resampled(24000,1,many["prepared"][4]["result"]["recipe"])
    assert [o["asset"]["id"] for o in many["operations"]]==batch_ids
    single=output/"single";single.mkdir()
    for item in (many["prepared"][0],many["prepared"][1],many["prepared"][3]):
        alone=request({"command":"media.prepare","path":item["path"],"input_root":str(inputs),"output_root":str(single),"output":str(single/(item["asset_id"]+"-prepared.mkv"))})
        assert alone["converted"]==item["result"]["converted"]
        if alone["converted"]:
            assert frame_digests(single/(item["asset_id"]+"-prepared.mkv"))==frame_digests(batch/(item["asset_id"]+"-prepared.mkv")),item["asset_id"]
    added=request({"command":"timeline.apply","project":p25,"expected_revision":0,"operations":many["operations"]})
    assert sorted(a["id"] for a in added["assets"])==sorted(batch_ids)
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
    # Audio-only assets on placed audio tracks: mixed sample-exactly with no picture source, refused
    # by pictures, and taken by meters, ducking, normalizing, tightening, beats, exports, reviews and
    # transcripts. The older black-picture asset of the same sound mixes to the same samples.
    voice_asset={**prepared_audio["audio-24000-1.wav"]["asset"],"id":"voice"}
    bed_asset={**ready_wav["asset"],"id":"bed","path":str(sources/"audio-48000-2.wav")}
    placed=request({"command":"project.create","id":"audio-only","width":W,"height":H,"frame_rate":time(25)})
    def track(id_,kind):return {"op":"tracks.edit","edit":{"op":"add","track":{"id":id_,"kind":kind,"locked":False,"enabled":True,"clips":[]}}}
    def place(track_id,id_,asset,start,source_in,duration):
        return {"op":"tracks.edit","edit":{"op":"place","track_id":track_id,"collision":"reject",
                "clip":{"id":id_,"asset_id":asset,"start":start,"source_in":source_in,"duration":duration}}}
    base=[{"op":"media.add","asset":voice_asset},{"op":"media.add","asset":bed_asset},{"op":"tracks.edit","edit":{"op":"create","duration":time(3)}},
          track("picture","video"),track("voice","audio"),track("bed","audio")]
    placed=request({"command":"timeline.apply","project":placed,"expected_revision":0,"operations":base+[
        place("voice","v1","voice",time(1,2),time(1,4),time(2)),place("bed","b1","bed",time(0),time(7,48000),time(3))]})
    refused=request({"command":"timeline.apply","project":placed,"expected_revision":placed["revision"],"operations":[place("picture","p1","bed",time(0),time(0),time(1))]},"UNSUPPORTED_MEDIA")
    assert "audio-only" in refused["error"]["message"] and "audio track" in refused["error"]["message"],refused
    voice48=np.frombuffer(wav_pcm(output/"audio-24000-1-prepared.wav"),"<i2").reshape(-1,2).astype(np.int64)
    mix=np.zeros((3*48000,2),dtype=np.int64)
    mix[24000:24000+96000]+=voice48[12000:12000+96000]
    mix+=source_pcm(48000,2)[7:7+3*48000]
    mix=np.clip(mix,-32768,32767).astype("<i2")
    rendered=request({"command":"render.run","project":placed,"input_root":str(root),"output_root":str(output),"output":str(output/"placed.mkv")})
    assert ff(["-i",str(output/"placed.mkv"),"-map","0:a:0","-f","s16le","-"])==mix.tobytes()
    assert ff(["-i",str(output/"placed.mkv"),"-map","0:v:0","-pix_fmt","rgb24","-f","rawvideo","-"])==bytes(75*W*H*3) and rendered["frames"]==75
    assert sorted(Path(s["path"]).name for s in rendered["sources"])==["audio-24000-1-prepared.wav","audio-48000-2.wav"]
    def export(project,name,**fields):
        return request({"command":"export.run","project":project,"input_root":str(root),"output_root":str(output),"output":str(output/name),**fields})
    export(placed,"placed-audio.wav",profile="reference",streams="audio")
    assert wav_pcm(output/"placed-audio.wav")==mix.tobytes()
    # The same sound as the older black-picture asset gives the same samples on an audio track.
    legacy={**voice["asset"],"id":"bed-legacy"}
    swapped=request({"command":"timeline.apply","project":placed,"expected_revision":placed["revision"],"operations":[{"op":"media.add","asset":legacy},
        {"op":"tracks.edit","edit":{"op":"remove","clip_ids":["b1"],"links":"reject_partial"}},place("bed","b2","bed-legacy",time(0),time(7,48000),time(3))]})
    export(swapped,"placed-legacy.wav",profile="reference",streams="audio")
    assert wav_pcm(output/"placed-legacy.wav")==mix.tobytes()
    peak=int(np.abs(mix.astype(np.int64)).max())
    meters=request({"command":"timeline.meters","project":placed,"input_root":str(root)})
    assert abs(max(meters["mix"]["sample_peak_dbfs"])-20*np.log10(peak/32768))<0.01 and [t["track_id"] for t in meters["tracks"]]==["voice","bed"],meters
    ducked=request({"command":"audio.duck","project":placed,"input_root":str(root),"voice_track_id":"voice","music_track_id":"bed"})
    assert ducked["operations"],ducked
    request({"command":"timeline.apply","project":placed,"expected_revision":placed["revision"],"operations":ducked["operations"]})
    normalized=request({"command":"audio.normalize","project":placed,"input_root":str(root),"target_lkfs":-20})
    request({"command":"timeline.apply","project":placed,"expected_revision":placed["revision"],"operations":normalized["operations"]})
    # The voice track is silent before and after its clip, so tightening proposes cuts there.
    tightened=request({"command":"audio.tighten","project":placed,"input_root":str(root),"voice_track_id":"voice","min_pause":time(9,20),"keep":time(1,10)})
    cuts=[(p["kind"],p["cut"]["start"],p["cut"]["end"]) for p in tightened["pauses"]["listed"]]
    assert cuts==[("leading",time(0),time(2,5)),("trailing",time(13,5),time(3))] and tightened["duration_after"]==time(11,5),tightened
    request({"command":"timeline.apply","project":placed,"expected_revision":placed["revision"],"operations":tightened["operations"]})
    request({"command":"audio.beats","path":str(output/"audio-24000-1-prepared.wav"),"input_root":str(root)})
    # Transcripts bind to the audio-only asset by content: the outline and a delivered cut's review
    # read the moved words, which play a quarter second later on the timeline.
    outlined=request({"command":"timeline.outline","project":placed,"transcripts":moved})
    assert all(word in outlined["outline"] for word in ("one","two","three")) and "four" not in outlined["outline"],outlined
    export(placed,"placed.mp4",profile="h264_aac",streams="audio_video",input_transfer="bt709")
    delivered=identity(output/"placed.mp4",root)
    heard={**spoken,"id":"heard","source":{"path":delivered["path"],"identity":{"sha256":delivered["sha256"],"bytes":delivered["bytes"]},"duration":time(3)},
           "range_start":time(0),"range_duration":time(3),
           "words":[{**w,"start":time(w["start"]["num"]*4+w["start"]["den"],w["start"]["den"]*4),"end":time(w["end"]["num"]*4+w["end"]["den"],w["end"]["den"]*4)}
                    for w in moved[0]["words"][:3]]}
    reviewed=request({"command":"export.review","path":str(output/"placed.mp4"),"input_root":str(root),"output_root":str(output),"output":str(output/"placed-review"),
                      "project":placed,"transcripts":moved,"heard":[heard],"rendition_height":0})
    speech=reviewed["speech"]
    assert speech["comparison"]["differences"]["listed"]==[] and speech["unused_transcripts"]==[],speech
    # Sequential clips play a picture, so an audio-only asset there is refused when rendered.
    sequential=request({"command":"timeline.apply","project":request({"command":"project.create","id":"sequential-wav","width":W,"height":H,"frame_rate":time(25)}),
        "expected_revision":0,"operations":[{"op":"media.add","asset":bed_asset},{"op":"clip.append","clip":{"id":"c","asset_id":"bed","source_in":time(0),"duration":time(1)}}]})
    refused=request({"command":"render.run","project":sequential,"input_root":str(root),"output_root":str(output),"output":str(output/"sequential-wav.mkv")},"UNSUPPORTED_MEDIA")
    assert "audio-only" in refused["error"]["message"] and not (output/"sequential-wav.mkv").exists(),refused
    passed.append("conform.audio_only_assets_on_tracks")
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
    report={"passed":passed,"cases":comparisons,"rejected_cases":len(bad),"reference":"All decoded output pixels and PCM compared with independent Fraction clocks, source timestamp lookup and source RGB/PCM; source codec decoding uses external FFmpeg. Audio-only WAVs compared with an integer resampler over the sources' generating formula","frames_compared":sum(v["frames"] for v in comparisons)+45+75,"sample_frames_compared":sum(v["sample_frames"] for v in comparisons)+86400+4*144000}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    (root/"recipe.json").write_text(json.dumps(recipe("fractional.mp4","fast-example",rate=time(3,2)),indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
