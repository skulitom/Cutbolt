"""Original easing/retiming fixtures, with independent Fraction and image references."""
from engine import ENGINE
import argparse
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image

from agents import Client
from animation import animated_scene_at, sample
from compositing import expected_frame, rect_at
from scenes import decode_check, identity, selected, time

ROOT = Path(__file__).resolve().parents[1]
MODES = ("hold", "linear", "ease_in", "ease_out", "ease_in_out")


def curves(layer):
    return [*layer["animation"].values(), *layer["mask"]["animation"].values()]


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/"sources", root/"output"
    sources.mkdir(); output.mkdir()
    for i in range(2):
        image = Image.new("RGBA", (5, 3))
        for y in range(3):
            for x in range(5):
                image.putpixel((x, y), (30+x*40, 210-y*65, 60+i*120, [0, 64, 173, 255][(x+y+i)%4]))
        image.save(sources/f"frame-{i}.png")
    original = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def curve(values, mode):
        return {"keys":[{"time":time(n, 5), "value":v, "interpolation":mode} for n,v in zip((1,4,7), values)]}

    def scene_for(mode):
        layer = {"id":"moving", "canvas":[8,6], "start":time(2,25), "duration":time(46,25),
            "frames":[{"image":identity(sources/f"frame-{i}.png", sources), "hold":time(h,25), "offset":[1,2], "anchor":[2,2]} for i,h in enumerate((3,2))],
            "timing":"strict", "end":"loop", "blend_mode":"screen",
            "transform":{"crop":[1,1,6,5], "quarter_turns":1, "scale":2, "position":[8,8], "opacity":255},
            "animation":{"position_x":curve((-4,21,6),mode), "position_y":curve((3,15,9),mode), "opacity":curve((60,240,128),mode)},
            "mask":{"rect":[1,2,5,3], "inverted":False, "animation":{"x":curve((0,3,1),mode), "width":curve((2,6,4),mode)}}}
        return {"schema_version":1,"id":"easing-fixture","width":32,"height":24,"output_scale":2,"duration":time(2),"background":[13,71,119],"color":"srgb_straight_encoded","layers":[layer],"audio":None}

    exe=ENGINE; passed=[]; cases=[]; frame_count=0

    def request(value, error=None, from_file=False):
        if from_file:
            path=root/"request.json";path.write_text(json.dumps(value),encoding="utf-8")
            result=subprocess.run([str(exe),str(path)],capture_output=True,timeout=90)
        else:
            result=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=90)
        data=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and data["error"]["code"]==error,data
            return data
        assert result.returncode==0 and data["ok"],data
        return data["result"]

    def inspect(scene):
        return request({"command":"scene.inspect","scene":scene,"input_root":str(sources)})

    def render(scene,name,error=None,from_file=False):
        return request({"command":"scene.render","scene":scene,"input_root":str(sources),"output_root":str(output),"output":str(output/name)},error,from_file)

    def verify(scene,name,from_file=False):
        nonlocal frame_count
        info=inspect(scene); receipt=render(scene,name+".mkv",from_file=from_file)
        assert receipt["timing"]==info["timing"]
        layer=scene["layers"][0]
        for n,actual in enumerate(info["timing"][0]["sampled_parameters"]):
            assert info["timing"][0]["selected_frames"][n]==selected(layer,n)
            if selected(layer,n) is None:
                assert actual is None
            else:
                tr=animated_scene_at(scene,n)["layers"][0]["transform"]
                assert actual=={"position":tr["position"],"opacity":tr["opacity"],"mask_rect":rect_at(layer,n)},(name,n,actual,tr)
        decode_check(output/(name+".mkv"),64,48,lambda n:expected_frame(scene,sources,n),50)
        frame_count+=50; cases.append(name)
        return info

    # Exact independent landmarks keep the reference formula itself reviewable.
    for mode,values in [("ease_in",[0,10,40,90,160]),("ease_out",[0,70,120,150,160]),("ease_in_out",[0,20,80,140,160])]:
        c={"keys":[{"time":time(0),"value":0,"interpolation":mode},{"time":time(1),"value":160,"interpolation":"hold"}]}
        assert [sample(c,Fraction(n,4)) for n in range(5)]==values
        baseline=scene_for(mode); verify(baseline,mode)
    passed.append("easing.exact_modes_and_pixels")

    for mode in MODES:
        for reverse in (False,True):
            scene=scene_for(mode)
            for c in curves(scene["layers"][0]):
                c["retime"]={"start":time(1,5),"rate":time(3,2),"reverse":reverse}
            info=verify(scene,f"{mode}-retime-{reverse}")
            # Retiming changes properties only; source loops and layer visibility are identical.
            assert info["timing"][0]["selected_frames"]==inspect(scene_for(mode))["timing"][0]["selected_frames"]
    passed.append("easing.forward_reverse_retiming")

    for name,start,rate in [("slower-clipped",time(3,25),time(1,2)),("minimum-rate",time(1,10),time(1,16)),("maximum-rate",time(1,10),time(16))]:
        scene=scene_for("ease_in_out")
        for c in curves(scene["layers"][0]):
            c["retime"]={"start":start,"rate":rate}
        verify(scene,name)
    identity_scene=scene_for("ease_in_out")
    for c in curves(identity_scene["layers"][0]):
        c["retime"]={"start":time(1,5),"rate":{"num":1000001,"den":1000001}}
    assert inspect(identity_scene)["timing"]==inspect(scene_for("ease_in_out"))["timing"]
    constant=scene_for("ease_in")
    for c in curves(constant["layers"][0]):
        c["keys"]=c["keys"][1:2]
        c["retime"]={"start":time(1,2),"rate":time(1,16),"reverse":True}
    verify(constant,"constant-retimed")
    passed.append("easing.retime_limits_and_clamps")

    mixed=scene_for("ease_out")
    for i,c in enumerate(curves(mixed["layers"][0])):
        c["keys"][1]["interpolation"]="hold" if i%2 else "ease_in"
        c["retime"]={"start":time(i+1,25),"rate":time(i+1,2),"reverse":bool(i%2)}
    before=verify(mixed,"independent-property-clocks")
    reloaded=json.loads(json.dumps(mixed))
    for c in curves(reloaded["layers"][0]):
        c["keys"].reverse()
    assert inspect(reloaded)["timing"]==before["timing"]
    verify(reloaded,"reloaded-shuffled",from_file=True)
    passed.append("easing.mask_curves_and_roundtrip")

    client=Client(exe)
    try:
        client.initialize()
        for scene in (mixed,scene_for("linear"),reloaded,mixed):
            assert client.call("scene.inspect",scene=scene,input_root=str(sources))["timing"]==inspect(scene)["timing"]
    finally:
        client.close()
    passed.append("easing.deterministic_mcp_replay")

    invalid=[]
    def bad(change,code="INVALID_ANIMATION"):
        scene=copy.deepcopy(mixed); change(scene["layers"][0]["animation"]["position_x"]);invalid.append((scene,code))
    bad(lambda c:c["keys"][0].update(interpolation="cubic_bezier"),"INVALID_JSON")
    bad(lambda c:c["retime"].update(start=time(2)))
    bad(lambda c:c["retime"].update(rate=time(0)))
    bad(lambda c:c["retime"].update(rate=time(1,17)))
    bad(lambda c:c["retime"].update(rate=time(17)))
    bad(lambda c:c["retime"].update(start=time(1,1000001)))
    bad(lambda c:c["retime"].update(rate=time(999983,1000001)))
    bad(lambda c:c["retime"].update(rate={"num":1,"den":0}),"INVALID_TIME")
    bad(lambda c:c["retime"].update(rate={"num":9007199254740992,"den":1}),"INVALID_TIME")
    bad(lambda c:c["retime"].update(rate={"num":-1,"den":1}),"INVALID_JSON")
    bad(lambda c:c["retime"].update(start={"num":-1,"den":1}),"INVALID_JSON")
    bad(lambda c:c["retime"].update(reverse="yes"),"INVALID_JSON")
    bad(lambda c:c["retime"].update(duration=time(1)),"INVALID_JSON")
    bad(lambda c:c.update(keys=[{"time":time(1,999983),"value":0,"interpolation":"ease_in"},{"time":time(1),"value":20,"interpolation":"hold"}],retime={"start":time(1,999979),"rate":time(999961,999959)}),"TIME_OVERFLOW")
    for i,(scene,code) in enumerate(invalid):
        render(scene,f"invalid-{i}.mkv",error=code)
        assert not (output/f"invalid-{i}.mkv").exists()
    passed.append("easing.invalid_inputs_and_precision")
    saved=hashlib.sha256((output/"reloaded-shuffled.mkv").read_bytes()).hexdigest()
    render(reloaded,"reloaded-shuffled.mkv",error="OUTPUT_EXISTS")
    assert saved==hashlib.sha256((output/"reloaded-shuffled.mkv").read_bytes()).hexdigest()
    assert original=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("easing.source_and_output_protection")

    report={"passed":passed,"decoded_frames":frame_count,"render_cases":cases,"rejected_cases":len(invalid),"reference":"Independent Fraction polynomial/clock evaluation and forward image/mask operations; complete RGB comparisons","scope":"Quadratic easing and bounded per-curve affine retiming; media clocks, audio and layer duration remain unchanged; finite exact precision rejects overflow"}
    (root/"scene.json").write_text(json.dumps(mixed,indent=2)+"\n",encoding="utf-8")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
