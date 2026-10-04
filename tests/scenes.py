"""Independent decoded-pixel/audio acceptance for bounded scenes and timeline previews."""
from engine import ENGINE
import argparse
import copy
from fractions import Fraction
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import struct
import subprocess
import sys
import wave
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from scene_fixtures import generate, identity, recipe, time, wav
from pixelforge_handoff import load_animation
from agents import Client

def rational(t):
    return Fraction(t["num"], t["den"])

def selected(layer, n):
    t = Fraction(n, 25) - rational(layer["start"])
    if not 0 <= t < rational(layer["duration"]):
        return None
    holds = [rational(f["hold"]) for f in layer["frames"]]
    cycle = sum(holds)
    if t >= cycle:
        if layer["end"] == "transparent":
            return None
        if layer["end"] == "hold_last":
            return len(holds) - 1
        t %= cycle
    for i, hold in enumerate(holds):
        if t < hold:
            return i
        t -= hold
    raise AssertionError("time outside source cycle")

def expected_frame(scene, root, n):
    # Forward image operations via Pillow, unlike the engine's inverse destination sampler.
    canvas = Image.new("RGB", (scene["width"], scene["height"]), tuple(scene["background"]))
    for layer in scene["layers"]:
        index = selected(layer, n)
        if index is None:
            continue
        f = layer["frames"][index]; tr = layer["transform"]
        with Image.open(root / f["image"]["path"]) as source:
            image = Image.new("RGBA", tuple(layer["canvas"]))
            image.paste(source.convert("RGBA"), tuple(f["offset"]))
        x, y, w, h = tr["crop"]
        image = image.crop((x, y, x + w, y + h))
        anchor = [f["anchor"][0] - x, f["anchor"][1] - y]
        for _ in range(tr["quarter_turns"]):
            anchor = [image.height - anchor[1], anchor[0]]
            image = image.transpose(Image.Transpose.ROTATE_270)
        scale = tr["scale"]
        image = image.resize((image.width * scale, image.height * scale), Image.Resampling.NEAREST)
        left, top = [tr["position"][i] - anchor[i] * scale for i in range(2)]
        for sy in range(max(0, -top), min(image.height, canvas.height - top)):
            for sx in range(max(0, -left), min(image.width, canvas.width - left)):
                pixel = image.getpixel((sx, sy)); dest = (left + sx, top + sy)
                alpha = Fraction(pixel[3], 255) * Fraction(tr["opacity"], 255)
                old = canvas.getpixel(dest)
                canvas.putpixel(dest, tuple(int(Fraction(old[c]) * (1-alpha) + pixel[c]*alpha + Fraction(1,2)) for c in range(3)))
    return canvas.resize((canvas.width * scene["output_scale"], canvas.height * scene["output_scale"]), Image.Resampling.NEAREST).tobytes()

def audio_expected(scene, root):
    total = int(rational(scene["duration"]) * 48000)
    expected = [0] * total * 2
    a = scene["audio"]
    if a is None:
        return struct.pack("<" + "h" * len(expected), *expected)
    with wave.open(str(root / a["file"]["path"]), "rb") as source:
        rate, channels, count = source.getframerate(), source.getnchannels(), source.getnframes()
        samples = struct.unpack("<" + "h" * count * channels, source.readframes(count))
    end = int(Fraction(count * 48000, rate) + Fraction(1,2))
    start = int(rational(a["start"]) * 48000)
    # Impulse fixtures: only nonzero support needs evaluation. Include endpoint extension.
    candidates = set()
    for i in range(count):
        if any(samples[i*channels+c] for c in range(channels)):
            center = Fraction(i*48000, rate)
            for n in range(max(0, int(center)-4), min(end, int(center)+5)):
                candidates.add(n)
    for n in candidates:
        pos = Fraction(n*rate, 48000); left = int(pos); fraction = pos-left
        for ch in range(2):
            c = ch if channels == 2 else 0
            value = samples[left*channels+c]*(1-fraction) + samples[min(left+1,count-1)*channels+c]*fraction
            rounded = int(abs(value)+Fraction(1,2)) * (-1 if value < 0 else 1)
            expected[(start+n)*2+ch] = rounded
    return struct.pack("<" + "h" * len(expected), *expected)

def audio_bytes(path):
    return subprocess.check_output(["ffmpeg", "-v", "error", "-i", str(path), "-map", "0:a:0", "-f", "s16le", "-c:a", "pcm_s16le", "-"], timeout=120)

def decode_check(path, width, height, expected, count):
    child = subprocess.Popen(["ffmpeg", "-v", "error", "-i", str(path), "-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        for n in range(count):
            actual = child.stdout.read(width*height*3)
            wanted = expected(n)
            assert actual == wanted, f"Decoded frame {n} differs ({len(actual)} bytes)"
        assert child.stdout.read(1) == b"", "Unexpected extra frame"
        assert child.wait(timeout=60) == 0, child.stderr.read().decode()
    finally:
        if child.poll() is None:
            child.kill(); child.wait()
        child.stdout.close(); child.stderr.close()

def run(root, pixelforge=None):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources = root / "sources"; output = root / "output"; output.mkdir(exist_ok=True)
    scene = generate(sources)
    passed = []
    exe = ENGINE
    def check(name, condition=True):
        assert condition, name
        passed.append(name)
    def request(command, expected_error=None, env=None):
        result = subprocess.run([str(exe)], input=json.dumps(command).encode(), capture_output=True, timeout=240, env=env)
        data = json.loads(result.stdout)
        if expected_error:
            assert result.returncode == 1 and data["error"]["code"] == expected_error, data
            return data
        assert result.returncode == 0 and data["ok"], data
        return data["result"]
    def inspect(s, error=None):
        return request({"command":"scene.inspect", "scene":s, "input_root":str(sources)}, error)
    def render(s, name, error=None, env=None):
        return request({"command":"scene.render", "scene":s, "input_root":str(sources), "output_root":str(output), "output":str(output/name)}, error, env)

    before = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    info = inspect(scene)
    for layer, report in zip(scene["layers"], info["timing"]):
        assert report["selected_frames"] == [selected(layer,n) for n in range(125)]
    check("scene.rational_animation_order")
    receipt = render(scene,"scene.mkv")
    cache = {}
    def expected(n):
        key = tuple(selected(layer,n) for layer in scene["layers"])
        if key not in cache:
            cache[key] = expected_frame(scene,sources,n)
        return cache[key]
    decode_check(output/"scene.mkv",1920,1080,expected,125)
    check("video.scene_transform_crop", receipt["width"] == 1920 and receipt["height"] == 1080 and receipt["frames"] == 125)
    # Prove the oracle fixture actually exercises overlap between different colors.
    small=copy.deepcopy(scene);small["output_scale"]=1;small["layers"]=small["layers"][:2]
    both=expected_frame(small,sources,4)
    one=copy.deepcopy(small);one["layers"]=one["layers"][:1]
    two=copy.deepcopy(small);two["layers"]=two["layers"][1:]
    bottom=expected_frame(one,sources,4);top=expected_frame(two,sources,4)
    bg=bytes(scene["background"])
    overlap=any(bottom[i:i+3]!=bg and top[i:i+3]!=bg and both[i:i+3] not in (bottom[i:i+3],top[i:i+3]) for i in range(0,len(both),3))
    check("video.scene_alpha_layers",overlap)
    check("scene.mono_audio_and_silence",audio_bytes(output/"scene.mkv")==audio_expected(scene,sources))

    # Explicit slice count must preserve tiny frames as well as the 1080p fixture.
    tiny_path=sources/"slice-boundary.png"
    tiny_pixels=bytes(v for y in range(2) for x in range(128) for v in ((x*17+y*43)%256,(x*31+y*53)%256,(x*7+y*97)%256,255))
    Image.frombytes("RGBA",(128,2),tiny_pixels).save(tiny_path)
    before[tiny_path.name]=hashlib.sha256(tiny_path.read_bytes()).hexdigest()
    tiny={"schema_version":1,"id":"slice-boundary","width":128,"height":2,"output_scale":1,"duration":time(2,25),"background":[0,0,0],"color":"srgb_straight_encoded","audio":None,
          "layers":[{"id":"chart","canvas":[128,2],"start":time(0),"duration":time(2,25),"timing":"strict","end":"hold_last",
                     "frames":[{"image":identity(tiny_path,sources),"hold":time(2,25),"offset":[0,0],"anchor":[0,0]}],
                     "transform":{"position":[0,0],"crop":[0,0,128,2],"scale":1,"quarter_turns":0,"opacity":255}}]}
    render(tiny,"slice-boundary.mkv")
    tiny_rgb=Image.frombytes("RGBA",(128,2),tiny_pixels).convert("RGB").tobytes()
    decode_check(output/"slice-boundary.mkv",128,2,lambda n:tiny_rgb,2)
    check("scene.small_frame_slice_integrity",audio_bytes(output/"slice-boundary.mkv")==bytes(2*1920*4))

    # Ingest the compiled result in an ordinary project; cuts and previews use timeline coordinates.
    project = request({"command":"project.create","id":"scene-preview","width":1920,"height":1080,"frame_rate":time(25)})
    project = request({"command":"timeline.apply","project":project,"expected_revision":0,"operations":[
        {"op":"media.add","asset":receipt["asset"]},
        {"op":"clip.append","clip":{"id":"later","asset_id":scene["id"],"source_in":time(1),"duration":time(1)}},
        {"op":"clip.append","clip":{"id":"earlier","asset_id":scene["id"],"source_in":time(0),"duration":time(1)}}]})
    fields={"project":project,"input_root":str(output),"output_root":str(output)}
    for n, source_n in [(0,25),(24,49),(25,0),(49,24)]:
        preview=request({"command":"preview.frame",**fields,"output":str(output/f"frame-{n}.png"),"time":time(n,25)})
        with Image.open(preview["output"]) as image:
            assert image.tobytes()==expected(source_n)
    check("preview.timeline_frames")
    client = Client(exe)
    try:
        client.initialize()
        assert client.call("scene.inspect",scene=scene,input_root=str(sources))["frames"] == 125
        client.call("preview.frame",**fields,output=str(output/"mcp-preview.png"),time=time(1))
        with Image.open(output/"mcp-preview.png") as image:
            assert image.tobytes()==expected(0)
    finally:
        client.close()
    check("scene.mcp_inspect_and_preview")
    request({"command":"preview.frame",**fields,"output":str(output/"frame-0.png"),"time":time(0)},"OUTPUT_EXISTS")
    request({"command":"preview.frame",**fields,"output":str(output/"bad.png"),"time":time(2)},"INVALID_RANGE")
    request({"command":"preview.frame",**fields,"output":str(output/"bad.png"),"time":time(1,100)},"UNALIGNED_TIME")
    request({"command":"preview.range",**fields,"output":str(output/"range.mkv"),"start":time(4,5),"duration":time(2,5)})
    decode_check(output/"range.mkv",1920,1080,lambda n:expected(45+n if n<5 else n-5),10)
    pcm=audio_expected(scene,sources)
    assert audio_bytes(output/"range.mkv")==pcm[45*1920*4:50*1920*4]+pcm[:5*1920*4]
    check("preview.range_boundary_sync")

    # All declared rate/channel pairs, including endpoints, signed peaks, sample rounding and silence.
    for rate in (24000,44100,48000):
        for channels in (1,2):
            name=f"audio-{rate}-{channels}"
            wav(sources/(name+".wav"),rate,channels)
            variant=copy.deepcopy(scene);variant.update(width=16,height=12,output_scale=1,duration=time(2))
            variant["layers"]=variant["layers"][:1];variant["layers"][0]["duration"]=time(2)
            variant["audio"]["file"]=identity(sources/(name+".wav"),sources)
            variant["audio"]["channels"]="duplicate_mono" if channels==1 else "preserve_stereo"
            render(variant,name+".mkv")
            assert audio_bytes(output/(name+".mkv"))==audio_expected(variant,sources),name
    check("scene.audio_conversion_matrix")

    # Trimmed input restoration, nonaligned holds, nonzero start, loop/hold/transparent endings.
    with Image.open(sources/"0.png") as image:
        image.crop((2,1,5,3)).save(sources/"trimmed.png")
    variant=copy.deepcopy(scene);variant.update(width=32,height=24,output_scale=2,duration=time(1),audio=None)
    variant["layers"]=variant["layers"][:1]
    layer=variant["layers"][0];layer.update(start=time(1,25),duration=time(24,25),timing="sample_start")
    layer["transform"].update(position=[15,12],scale=3)
    layer["frames"][1]["image"]=identity(sources/"trimmed.png",sources);layer["frames"][1]["offset"]=[2,1]
    layer["frames"][0]["hold"]=time(1,10)
    strict=copy.deepcopy(variant);strict["layers"][0]["timing"]="strict";inspect(strict,"UNALIGNED_TIME")
    for end in ("loop","hold_last","transparent"):
        layer["end"]=end
        assert inspect(variant)["timing"][0]["selected_frames"]==[selected(layer,n) for n in range(25)]
        render(variant,end+".mkv")
        decode_check(output/(end+".mkv"),64,48,lambda n:expected_frame(variant,sources,n),25)
    check("scene.trimmed_anchor_and_end_policies")

    broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["image"]["sha256"]="0"*64
    inspect(broken,"MEDIA_CHANGED")
    for path in ("../escape.png",str(sources/"0.png"),"0.png:stream"):
        broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["image"]["path"]=path;inspect(broken,"INVALID_PATH")
    broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["image"]["path"]="missing.png";inspect(broken,"IO_ERROR")
    if os.name=="nt":
        outside=root/"outside";outside.mkdir();(outside/"0.png").write_bytes((sources/"0.png").read_bytes())
        junction=sources/"escape"
        # Native PowerShell owns junction creation; remove only this known link, never its target.
        subprocess.run(["powershell","-NoProfile","-Command","$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:CUTBOLT_TEST_JUNCTION -Target $env:CUTBOLT_TEST_TARGET | Out-Null"],env={**os.environ,"CUTBOLT_TEST_JUNCTION":str(junction),"CUTBOLT_TEST_TARGET":str(outside)},capture_output=True,check=True)
        try:
            broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["image"]["path"]="escape/0.png";inspect(broken,"PATH_OUTSIDE_ROOT")
        finally:
            os.rmdir(junction)
    check("scene.identity_and_containment")
    broken=copy.deepcopy(scene);broken["audio"]["start"]=time(5);inspect(broken,"AUDIO_OVERFLOW")
    broken=copy.deepcopy(scene);broken["audio"]["channels"]="preserve_stereo";inspect(broken,"UNSUPPORTED_AUDIO")
    broken=copy.deepcopy(scene);broken["layers"][0]["transform"]["crop"]=[0,0,100,100];inspect(broken,"INVALID_SCENE")
    broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["hold"]={"num":1,"den":0};inspect(broken,"INVALID_TIME")
    with Image.open(sources/"0.png") as image:
        image.convert("P").save(sources/"palette.png")
    broken=copy.deepcopy(scene);broken["layers"][0]["frames"][0]["image"]=identity(sources/"palette.png",sources);inspect(broken,"UNSUPPORTED_IMAGE")
    extensible=bytearray((sources/"narration.wav").read_bytes());extensible[20:22]=b"\xfe\xff"
    (sources/"extensible.wav").write_bytes(extensible)
    broken=copy.deepcopy(scene);broken["audio"]["file"]=identity(sources/"extensible.wav",sources);inspect(broken,"UNSUPPORTED_AUDIO")
    broken=copy.deepcopy(scene);broken["layers"][1]["frames"]=copy.deepcopy(broken["layers"][1]["frames"])
    broken["layers"][1]["frames"][0]["image"]["sha256"]="0"*64;inspect(broken,"INVALID_IDENTITY")
    check("scene.unsupported_semantics_rejected")
    render(scene,"scene.mkv","OUTPUT_EXISTS")
    missing_env={**os.environ,"CUTBOLT_FFMPEG":str(root/"missing-ffmpeg.exe")}
    render(scene,"failed.mkv","TOOL_UNAVAILABLE",missing_env)
    assert not (output/"failed.mkv").exists() and not list(output.glob(".cutbolt-scene-*"))
    check("scene.output_protection_and_failure_cleanup")
    check("scene.sources_unchanged",all(hashlib.sha256((sources/name).read_bytes()).hexdigest()==digest for name,digest in before.items()))

    companion=None
    if pixelforge:
        exported=sources/"pixelforge"
        subprocess.run(["node",str(pixelforge/"bin/pixelforge.js"),"render",str(sources/"original-recipe.json"),"--out",str(exported)],capture_output=True,check=True,timeout=30)
        handoff=load_animation(exported/"CutboltTiming.atlas.json",sources,"pulse")
        assert handoff["provenance"]["order"]==["c","a","b","a"]
        assert [rational(f["hold"]) for f in handoff["frames"]]==[Fraction(3,25),Fraction(1,25),Fraction(2,25),Fraction(1,25)]
        for name,index in zip("abc",range(3)):
            with Image.open(exported/f"frames/{name}.png") as actual,Image.open(sources/f"{index}.png") as wanted:
                assert actual.convert("RGBA").tobytes()==wanted.tobytes()
        substitute=copy.deepcopy(scene)
        for entry in substitute["layers"]:
            entry.update({key:handoff[key] for key in ("canvas","frames","end")})
        inspect(substitute);render(substitute,"pixelforge-scene.mkv")
        decode_check(output/"pixelforge-scene.mkv",1920,1080,expected,125)
        assert audio_bytes(output/"pixelforge-scene.mkv")==pcm
        companion={"node":subprocess.check_output(["node","--version"],text=True).strip(),"package_version":json.loads((pixelforge/"package.json").read_text(encoding="utf-8"))["version"],"commit":subprocess.check_output([r"C:\Program Files\Git\cmd\git.exe","-C",str(pixelforge),"rev-parse","HEAD"],text=True).strip(),"license":"MIT","handoff":handoff["provenance"]}
        (root/"pixelforge-handoff.json").write_text(json.dumps(handoff,indent=2)+"\n",encoding="utf-8")
        check("pipeline.pixelforge_file_handoff")
    report={"development_dependencies":{"Pillow":importlib.metadata.version("Pillow")},"passed":passed,"scene":receipt,"pixel_oracle":"Pillow forward crop/rotate/scale + exact rational alpha; every decoded frame", "audio_oracle":"Independent rational impulse support and all decoded PCM samples", "companion":companion,"human_editorial_approval":False}
    for name,data in [("scene.json",scene),("project.json",project),("verification.json",report)]:
        (root/name).write_text(json.dumps(data,indent=2)+"\n",encoding="utf-8")
    print(json.dumps({"passed":len(passed),"output":str(output/"scene.mkv")},indent=2))

if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True);parser.add_argument("--pixelforge",type=Path)
    args=parser.parse_args();run(args.output,args.pixelforge)
