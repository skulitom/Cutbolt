"""Original keyframe fixtures; independent Fraction values and forward image operations."""
from engine import ENGINE, per_frame
import argparse
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image

from scenes import decode_check, expected_frame, identity, time
from agents import Client

ROOT = Path(__file__).resolve().parents[1]


def sample(curve, when):
    keys = sorted(curve["keys"], key=lambda k: Fraction(k["time"]["num"], k["time"]["den"]))
    times = [Fraction(k["time"]["num"], k["time"]["den"]) for k in keys]
    if "retime" in curve:
        r = curve["retime"]
        progress = max(Fraction(0), when-Fraction(r["start"]["num"], r["start"]["den"])) * Fraction(r["rate"]["num"], r["rate"]["den"])
        when = times[-1]-progress if r.get("reverse", False) else times[0]+progress
    if when <= times[0]:
        value = Fraction(keys[0]["value"])
    elif when >= times[-1]:
        value = Fraction(keys[-1]["value"])
    else:
        interval = next(i for i in range(len(keys)-1) if times[i] <= when < times[i+1])
        left, right = keys[interval:interval+2]
        weight = (when-times[interval]) / (times[interval+1]-times[interval])
        mode = left["interpolation"]
        if mode == "ease_in":
            weight = weight**2
        elif mode == "ease_out":
            weight = weight*(2-weight)
        elif mode == "ease_in_out":
            weight = 2*weight**2 if weight < Fraction(1,2) else -2*weight**2+4*weight-1
        value = Fraction(left["value"]) if left["interpolation"] == "hold" else left["value"]*(1-weight)+right["value"]*weight
    return int(abs(value)+Fraction(1, 2)) * (-1 if value < 0 else 1)


def animated_scene_at(scene, n):
    result = copy.deepcopy(scene)
    for layer in result["layers"]:
        clock = Fraction(n, 25)-Fraction(layer["start"]["num"], layer["start"]["den"])
        for name, curve in layer.pop("animation", {}).items():
            value = sample(curve, clock)
            if name == "opacity":
                layer["transform"]["opacity"] = value
            else:
                layer["transform"]["position"][0 if name == "position_x" else 1] = value
    return result


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/"sources", root/"output"
    sources.mkdir(); output.mkdir()
    for i in range(2):
        im = Image.new("RGBA", (6, 4))
        for y in range(4):
            for x in range(6):
                im.putpixel((x, y), (35+x*30, 20+y*50, 200-i*100, [0, 90, 180, 255][(x+y+i)%4]))
        im.save(sources/f"frame-{i}.png")

    def curve(entries):
        return {"keys":[{"time":time(n, d), "value":v, "interpolation":mode} for n,d,v,mode in entries]}

    layer = {"id":"moving", "canvas":[6,4], "start":time(2,25), "duration":time(44,25),
        "frames":[{"image":identity(sources/f"frame-{i}.png", sources), "hold":time(n,100), "offset":[0,0], "anchor":[2,1]} for i,n in enumerate((10,14))],
        "timing":"sample_start", "end":"loop", "transform":{"crop":[0,0,6,4], "quarter_turns":1, "scale":2, "position":[0,0], "opacity":255},
        "animation":{
            "position_x":curve([(9,10,25,"hold"),(1,10,-2,"linear"),(7,5,7,"linear")]),
            "position_y":curve([(0,1,10,"hold"),(1,2,4,"linear"),(3,2,15,"hold")]),
            "opacity":curve([(0,1,0,"linear"),(8,25,255,"hold"),(9,10,64,"linear"),(3,2,200,"hold")])}}
    top = copy.deepcopy(layer)
    top.update(id="overlay", start=time(6,25), duration=time(40,25), end="hold_last")
    top["transform"].update(position=[10,9], quarter_turns=3, scale=1)
    top["animation"]={"opacity":curve([(1,2,128,"hold")])}
    scene={"schema_version":1,"id":"keyframe-fixture","width":32,"height":24,"output_scale":2,"duration":time(2),"background":[10,20,30],"color":"srgb_straight_encoded","layers":[layer,top],"audio":None}
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE; passed=[]

    def request(command, error=None, from_file=False):
        if from_file:
            path=root/"request.json";path.write_text(json.dumps(command),encoding="utf-8")
            result=subprocess.run([str(exe),str(path)],capture_output=True,timeout=90)
        else:
            result=subprocess.run([str(exe)],input=json.dumps(command).encode(),capture_output=True,timeout=90)
        data=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and data["error"]["code"]==error,data
            return data
        assert result.returncode==0 and data["ok"],data
        return data["result"]

    def inspect(value):
        return request({"command":"scene.inspect","scene":value,"input_root":str(sources)})

    def render(value, name, error=None, from_file=False):
        return request({"command":"scene.render","scene":value,"input_root":str(sources),"output_root":str(output),"output":str(output/name)},error,from_file)

    info=inspect(scene)
    for i, item in enumerate(info["timing"]):
        for n, actual in enumerate(per_frame(item["sampled_parameters"])):
            if per_frame(item["selected_frames"])[n] is None:
                assert actual is None
            else:
                tr=animated_scene_at(scene,n)["layers"][i]["transform"]
                assert actual=={"position":tr["position"],"opacity":tr["opacity"]},(i,n,actual,tr)
    # Fixed independent landmarks: endpoint clamp, exact hold change and negative half rounding.
    assert per_frame(info["timing"][0]["sampled_parameters"])[2]=={"position":[-2,10],"opacity":0}
    assert sample(curve([(0,1,-1,"linear"),(2,25,0,"hold")]),Fraction(1,25))==-1
    passed.append("animation.sampled_parameters")
    render(scene,"animated.mkv")
    expected=lambda n:expected_frame(animated_scene_at(scene,n),sources,n)
    decode_check(output/"animated.mkv",64,48,expected,50)
    passed.append("animation.rendered_parameter_curves")

    reordered=copy.deepcopy(scene)
    for item in reordered["layers"]:
        for c in item["animation"].values():
            c["keys"].reverse()
    assert inspect(reordered)["timing"]==info["timing"]
    render(reordered,"reordered.mkv",from_file=True)
    decode_check(output/"reordered.mkv",64,48,expected,50)
    render(json.loads(json.dumps(scene)),"reloaded.mkv",from_file=True)
    decode_check(output/"reloaded.mkv",64,48,expected,50)
    passed.append("animation.order_and_roundtrip")

    client=Client(exe)
    try:
        client.initialize()
        assert client.call("scene.inspect",scene=scene,input_root=str(sources))["timing"]==info["timing"]
    finally:
        client.close()
    passed.append("animation.mcp_inspection")

    invalid=[]
    def bad(change, code="INVALID_ANIMATION"):
        value=copy.deepcopy(scene);change(value["layers"][0]);invalid.append((value,code))
    bad(lambda l:l.update(animation={}))
    bad(lambda l:l["animation"]["opacity"].update(keys=[]))
    bad(lambda l:l["animation"]["position_x"].update(keys=curve([(1,2,1,"hold"),(2,4,2,"linear")])["keys"]))
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(value=-1))
    bad(lambda l:l["animation"]["position_y"]["keys"][0].update(value=32769))
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(time=time(2)))
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(time=time(1,1000001)))
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(time={"num":1,"den":0}),"INVALID_TIME")
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(time={"num":9007199254740992,"den":1}),"INVALID_TIME")
    bad(lambda l:l["animation"]["opacity"].update(keys=l["animation"]["opacity"]["keys"]*33))
    bad(lambda l:l["animation"]["opacity"]["keys"][0].update(interpolation="bezier"),"INVALID_JSON")
    bad(lambda l:l["animation"].update(scale=curve([(0,1,1,"hold") ])),"INVALID_JSON")
    for i,(value,code) in enumerate(invalid):
        render(value,f"invalid-{i}.mkv",code)
        assert not (output/f"invalid-{i}.mkv").exists()
    passed.append("animation.invalid_inputs")
    saved=hashlib.sha256((output/"animated.mkv").read_bytes()).hexdigest()
    render(scene,"animated.mkv","OUTPUT_EXISTS")
    assert saved==hashlib.sha256((output/"animated.mkv").read_bytes()).hexdigest()
    assert not list(output.glob(".cutbolt-scene-*"))
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    passed.append("animation.source_and_output_protection")
    report={"passed":passed,"decoded_frames":150,"rejected_cases":len(invalid),"reference":"Independent Fraction interpolation and forward Pillow transforms; full RGB frame comparison","scope":"Layer-local position/opacity hold and linear curves; no broader easing or retiming claim"}
    (root/"scene.json").write_text(json.dumps(scene,indent=2)+"\n",encoding="utf-8")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
