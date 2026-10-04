"""Independent rational blend equations and forward image/mask operations."""
from engine import ENGINE, per_frame
import argparse
import copy
from fractions import Fraction
import hashlib
import json
from pathlib import Path
import subprocess
from PIL import Image

from scenes import decode_check, identity, selected, time
from animation import animated_scene_at, sample
from agents import Client

ROOT = Path(__file__).resolve().parents[1]


def rect_at(layer, n):
    mask = layer.get("mask")
    if mask is None:
        return None
    rect = list(mask["rect"])
    clock = Fraction(n, 25)-Fraction(layer["start"]["num"], layer["start"]["den"])
    for key, curve in mask.get("animation", {}).items():
        rect[["x", "y", "width", "height"].index(key)] = sample(curve, clock)
    return rect


def expected_frame(scene, sources, n):
    scene = animated_scene_at(scene, n)
    canvas = Image.new("RGB", (scene["width"], scene["height"]), tuple(scene["background"]))
    for layer in scene["layers"]:
        index = selected(layer, n)
        if index is None:
            continue
        frame, transform = layer["frames"][index], layer["transform"]
        with Image.open(sources/frame["image"]["path"]) as source:
            image = Image.new("RGBA", tuple(layer["canvas"]))
            image.paste(source.convert("RGBA"), tuple(frame["offset"]))
        rect = rect_at(layer, n)
        if rect is not None:
            left, top, width, height = rect
            for y in range(image.height):
                for x in range(image.width):
                    inside = left <= x < left+width and top <= y < top+height
                    if inside == layer["mask"].get("inverted", False):
                        image.putpixel((x, y), (0, 0, 0, 0))
        x, y, w, h = transform["crop"]
        image = image.crop((x, y, x+w, y+h))
        anchor = [frame["anchor"][0]-x, frame["anchor"][1]-y]
        for _ in range(transform["quarter_turns"]):
            anchor = [image.height-anchor[1], anchor[0]]
            image = image.transpose(Image.Transpose.ROTATE_270)
        scale = transform["scale"]
        image = image.resize((image.width*scale, image.height*scale), Image.Resampling.NEAREST)
        left, top = [transform["position"][i]-anchor[i]*scale for i in range(2)]
        mode = layer.get("blend_mode", "normal")
        premultiplied = layer.get("alpha_mode", "straight") == "premultiplied"
        for y in range(max(0, -top), min(image.height, canvas.height-top)):
            for x in range(max(0, -left), min(image.width, canvas.width-left)):
                pixel = image.getpixel((x, y)); dest = (left+x, top+y)
                if not pixel[3]:
                    continue
                alpha = Fraction(pixel[3]*transform["opacity"], 255*255)
                old = canvas.getpixel(dest)
                values = []
                for c in range(3):
                    source = Fraction(pixel[c], pixel[3] if premultiplied else 255)
                    backdrop = Fraction(old[c], 255)
                    blend = {"normal":source, "multiply":source*backdrop, "screen":1-(1-source)*(1-backdrop)}[mode]
                    result = 255*(alpha*blend+(1-alpha)*backdrop)
                    values.append(int(result+Fraction(1, 2)))
                canvas.putpixel(dest, tuple(values))
    return canvas.resize((canvas.width*scene["output_scale"], canvas.height*scene["output_scale"]), Image.Resampling.NEAREST).tobytes()


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources, output = root/"sources", root/"output"
    sources.mkdir(); output.mkdir()
    background = Image.new("RGB", (64, 48))
    for y in range(48):
        for x in range(64):
            background.putpixel((x, y), (x*255//63, y*255//47, [0, 255, 77, 169][(x//4+y//4)%4]))
    background.save(sources/"background.png")
    a_values = [0, 1, 17, 64, 127, 128, 254, 255]
    straight, premul, fractional, hidden = [Image.new("RGBA", (16, 8)) for _ in range(4)]
    for y in range(8):
        for x in range(16):
            a = a_values[x//2]
            color = [255 if (x+y+c)%3 else 0 for c in range(3)]
            straight.putpixel((x,y), (*color,a))
            premul.putpixel((x,y), (*(a if c else 0 for c in color),a))
            fractional.putpixel((x,y), (min(a,17), a*2//3, a//2, a))
            hidden.putpixel((x,y), ((x*17)%256, (y*31)%256, 233, a))
    for name, image in [("straight",straight),("premul",premul),("fractional",fractional),("hidden",hidden)]:
        image.save(sources/(name+".png"))
    invalid = Image.new("RGBA", (1,1), (1,0,0,0));invalid.save(sources/"bad-zero.png")
    invalid.putpixel((0,0),(50,0,0,49));invalid.save(sources/"bad-coverage.png")

    def layer(name, canvas, path):
        return {"id":name,"canvas":canvas,"start":time(0),"duration":time(3,5),
            "frames":[{"image":identity(sources/path,sources),"hold":time(3,5),"offset":[0,0],"anchor":[0,0]}],
            "timing":"strict","end":"hold_last","transform":{"position":[0,0],"crop":[0,0,*canvas],"scale":1,"quarter_turns":0,"opacity":255}}
    bottom = layer("backdrop",[64,48],"background.png")
    top = layer("foreground",[20,12],"straight.png")
    top["frames"][0].update(offset=[2,2],anchor=[3,3])
    top["transform"].update(position=[7,8],crop=[1,1,18,10],scale=2,opacity=173)
    scene={"schema_version":1,"id":"compositing-fixture","width":64,"height":48,"output_scale":1,"duration":time(3,5),"background":[11,87,161],"color":"srgb_straight_encoded","layers":[bottom,top],"audio":None}
    original_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE;passed=[];count=0;cases=[]

    def request(value,error=None):
        result=subprocess.run([str(exe)],input=json.dumps(value).encode(),capture_output=True,timeout=90)
        reply=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and reply["error"]["code"]==error,reply
            return reply
        assert result.returncode==0 and reply["ok"],reply
        return reply["result"]
    def inspect(value):
        return request({"command":"scene.inspect","scene":value,"input_root":str(sources)})
    def render(value,name,error=None):
        return request({"command":"scene.render","scene":value,"input_root":str(sources),"output_root":str(output),"output":str(output/name)},error)
    def verify(value,name):
        nonlocal count
        info=inspect(value);render(value,name+".mkv")
        for i,item in enumerate(info["timing"]):
            for n,params in enumerate(per_frame(item["sampled_parameters"])):
                if params is not None:
                    assert params.get("mask_rect")==rect_at(value["layers"][i],n)
        frames=info["frames"]
        decode_check(output/(name+".mkv"),64,48,lambda n:expected_frame(value,sources,n),frames)
        count+=frames;cases.append(name)
        return info

    # Every blend mode, alpha encoding and low/zero/full coverage; paired encodings are exactly equivalent.
    for mode in ("normal","multiply","screen"):
        pair=[]
        for encoding,path in [("straight","straight.png"),("premultiplied","premul.png")]:
            value=copy.deepcopy(scene);t=value["layers"][1]
            t.update(blend_mode=mode,alpha_mode=encoding)
            t["frames"][0]["image"]=identity(sources/path,sources)
            verify(value,mode+"-"+encoding);pair.append(expected_frame(value,sources,0))
        assert pair[0]==pair[1]
    passed.append("composite.blend_alpha_equations")
    for mode in ("normal","multiply","screen"):
        value=copy.deepcopy(scene);t=value["layers"][1]
        t.update(blend_mode=mode,alpha_mode="premultiplied")
        t["frames"][0]["image"]=identity(sources/"fractional.png",sources)
        verify(value,mode+"-fractional-alpha")
    value=copy.deepcopy(scene);value["layers"][1]["frames"][0]["image"]=identity(sources/"hidden.png",sources)
    verify(value,"straight-hidden-colors")
    for opacity in (0,255):
        for mode in ("normal","multiply","screen"):
            value=copy.deepcopy(scene);value["layers"][1]["transform"]["opacity"]=opacity
            value["layers"][1]["blend_mode"]=mode;verify(value,f"opacity-{opacity}-{mode}")
    passed.append("composite.transparent_edges_and_limits")

    static=[]
    for inverted,rect in [(False,[4,3,8,5]),(True,[4,3,8,5]),(False,[0,0,0,8]),(True,[0,0,0,8]),(False,[-20,-20,4,4]),(True,[-20,-20,4,4])]:
        value=copy.deepcopy(scene);value["layers"][1]["mask"]={"rect":rect,"inverted":inverted}
        verify(value,f"static-mask-{len(static)}");static.append(value)
    assert expected_frame(static[0],sources,0)!=expected_frame(static[1],sources,0)
    passed.append("mask.static_and_inverted_rectangles")

    def curve(entries):
        return {"keys":[{"time":time(n,d),"value":v,"interpolation":mode} for n,d,v,mode in entries]}
    for turns in range(4):
        for inverted in (False,True):
            animated=copy.deepcopy(scene);t=animated["layers"][1]
            t.update(start=time(2,25),duration=time(13,25),blend_mode="screen" if inverted else "multiply")
            t["transform"].update(quarter_turns=turns,position=[12,10],scale=2)
            t["animation"]={"opacity":curve([(0,1,255,"linear"),(1,2,32,"hold")])}
            t["mask"]={"rect":[3,2,10,7],"inverted":inverted,"animation":{
                "x":curve([(1,2,9,"hold"),(0,1,-2,"linear")]),
                "y":curve([(0,1,2,"hold"),(7,25,5,"linear")]),
                "width":curve([(0,1,0,"linear"),(1,5,12,"hold"),(2,5,4,"hold")]),
                "height":curve([(0,1,8,"hold"),(3,10,3,"linear"),(1,2,9,"hold")])}}
            verify(animated,f"animated-mask-{turns}-{inverted}")
    passed.append("mask.keyframes_and_transform_order")
    repeated=json.loads(json.dumps(animated))
    for c in repeated["layers"][1]["mask"]["animation"].values():
        c["keys"].reverse()
    assert inspect(repeated)["timing"]==inspect(animated)["timing"]
    verify(repeated,"mask-reloaded-reordered")
    passed.append("mask.roundtrip_and_key_order")
    client=Client(exe)
    try:
        client.initialize()
        assert client.call("scene.inspect",scene=animated,input_root=str(sources))["timing"]==inspect(animated)["timing"]
    finally:
        client.close()
    passed.append("composite.mcp_inspection")

    bad=[]
    def reject(change,code):
        value=copy.deepcopy(animated);change(value["layers"][1]);bad.append((value,code))
    reject(lambda t:t.update(blend_mode="overlay"),"INVALID_JSON")
    reject(lambda t:t.update(alpha_mode="automatic"),"INVALID_JSON")
    reject(lambda t:t["mask"].update(feather=1),"INVALID_JSON")
    reject(lambda t:t["mask"].update(rect=[0,0,-1,1]),"INVALID_MASK")
    reject(lambda t:t["mask"].update(rect=[32769,0,1,1]),"INVALID_MASK")
    reject(lambda t:t["mask"].update(animation={}),"INVALID_MASK")
    reject(lambda t:t["mask"]["animation"]["width"]["keys"][0].update(value=-1),"INVALID_ANIMATION")
    reject(lambda t:t["mask"]["animation"].update(x=curve([(1,5,1,"hold"),(2,10,2,"linear") ])),"INVALID_ANIMATION")
    for path in ("bad-zero.png","bad-coverage.png"):
        value=copy.deepcopy(scene);t=value["layers"][1];t["alpha_mode"]="premultiplied"
        t["frames"][0]["image"]=identity(sources/path,sources);bad.append((value,"INVALID_ALPHA"))
    # A source used first as straight must still be validated when reused as premultiplied.
    value=copy.deepcopy(scene);value["layers"][1]["frames"][0]["image"]=identity(sources/"hidden.png",sources)
    duplicate=copy.deepcopy(value["layers"][1]);duplicate.update(id="same-source-premultiplied",alpha_mode="premultiplied")
    value["layers"].append(duplicate);bad.append((value,"INVALID_ALPHA"))
    for i,(value,code) in enumerate(bad):
        render(value,f"invalid-{i}.mkv",code)
        assert not (output/f"invalid-{i}.mkv").exists()
    passed.append("composite.invalid_inputs")
    saved=hashlib.sha256((output/"mask-reloaded-reordered.mkv").read_bytes()).hexdigest()
    render(repeated,"mask-reloaded-reordered.mkv","OUTPUT_EXISTS")
    assert saved==hashlib.sha256((output/"mask-reloaded-reordered.mkv").read_bytes()).hexdigest()
    assert original_hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("composite.source_and_output_protection")
    report={"passed":passed,"decoded_frames":count,"render_cases":cases,"rejected_cases":len(bad),"reference":"Independent normalized Fraction blend equations and forward source-canvas masks/Pillow transforms; exact full RGB comparison","limits":"Opaque encoded-sRGB backdrop; normal/multiply/screen; explicit stored premultiplied input; one hard rectangular mask per layer, no feather/tracking"}
    (root/"scene.json").write_text(json.dumps(animated,indent=2)+"\n",encoding="utf-8")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
