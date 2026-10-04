"""Typed template reuse checked against independently built scenes and original glyph geometry."""
from engine import ENGINE
import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
from jsonschema import Draft202012Validator

from agents import Client
from compositing import expected_frame
from graphics import PRIMARY, FALLBACK, make_font, text_pixels, shape_pixels
from scenes import audio_bytes, decode_check, identity, time

ROOT=Path(__file__).resolve().parents[1]


def value(kind,data): return {"type":kind,"value":data}
def binding(prop,layer=None): return {"property":prop,**({"layer":layer} if layer else {})}
def parameter(name,kind,default,bindings,**bounds):
    return {"name":name,"definition":{"type":kind,"default":default,**bounds},"bindings":bindings}
def curve(a,b,frames=11,mode="linear"):
    return {"keys":[{"time":time(0),"value":a,"interpolation":mode},{"time":time(frames,25),"value":b,"interpolation":"hold"}]}


def run(root):
    root=root.resolve();assert root!=ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources,output,oracle,store=[root/n for n in ("sources","output","oracle","store")]
    for folder in (sources,output,oracle,store):folder.mkdir()
    make_font(sources/"primary.ttf",PRIMARY);make_font(sources/"fallback.ttf",FALLBACK)
    fonts=[identity(sources/n,sources) for n in ("primary.ttf","fallback.ttf")]
    font_data={"primary.ttf":PRIMARY,"fallback.ttf":FALLBACK}
    hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    paths=[ROOT/"examples"/(name+"-template.json") for name in ("lower-third","title-card")]
    originals={p.name:p.read_bytes() for p in paths}
    templates=[json.loads(p.read_text()) for p in paths]
    exe=ENGINE;passed=[];decoded=0;rendered=[]
    def request(command,error=None):
        result=subprocess.run([str(exe)],input=json.dumps(command).encode(),capture_output=True,timeout=120)
        data=json.loads(result.stdout)
        if error:
            assert result.returncode==1 and data["error"]["code"]==error,(error,data)
            return data
        assert result.returncode==0 and data["ok"],data
        return data["result"]
    def instantiate(t,v,name="instance",error=None):
        return request({"command":"graphics.instantiate","template":t,"values":v,"instance_id":name,"input_root":str(sources)},error)
    def render(s,name,error=None):
        return request({"command":"scene.render","scene":s,"input_root":str(sources),"output_root":str(output),"output":str(output/(name+".mkv"))},error)
    def reference(s,name):
        expected=copy.deepcopy(s)
        for i,layer in enumerate(expected["layers"]):
            graphic=layer.pop("graphics")
            image=text_pixels(graphic,layer["canvas"],font_data)[0] if graphic["kind"]=="text" else shape_pixels(graphic,layer["canvas"])
            path=oracle/f"{name}-{i}.png";image.save(path)
            layer["frames"]=[{"image":identity(path,oracle),"hold":layer["duration"],"offset":[0,0],"anchor":[0,0]}]
        return expected
    def verify(result,expected,name):
        nonlocal decoded
        # Expected scenes are assembled explicitly below, never from the engine's returned bindings.
        assert result["scene"]==expected,(name,result["scene"],expected)
        assert request({"command":"scene.inspect","scene":expected,"input_root":str(sources)})==result["inspection"]
        ref=reference(expected,name);receipt=render(result["scene"],name)
        assert receipt["scene_sha256"]==result["scene_sha256"]
        w,h=expected["width"]*expected["output_scale"],expected["height"]*expected["output_scale"]
        decode_check(output/(name+".mkv"),w,h,lambda n:expected_frame(ref,oracle,n),receipt["frames"])
        assert audio_bytes(output/(name+".mkv"))==b"\0"*(receipt["frames"]*1920*4)
        decoded+=receipt["frames"];rendered.append({"name":name,"frames":receipt["frames"],"maximum_rgb_delta":0})
        return receipt,ref

    cap=request({"command":"capabilities","section":"all"})
    assert "graphics.instantiate" in cap["commands"] and cap["templates"]["maximum_parameters"]==64
    fade={"keys":[{"time":time(0),"value":0,"interpolation":"linear"},{"time":time(3,25),"value":255,"interpolation":"hold"},{"time":time(7,25),"value":128,"interpolation":"ease_out"},{"time":time(11,25),"value":0,"interpolation":"hold"}]}
    base_values={"fonts":value("fonts",fonts),"label":value("text","A Ω中"),"duration":value("time",time(12,25)),"fade":value("curve",fade)}
    instances=[]
    for i,t in enumerate(templates):
        for variant in range(2):
            name=f"preset-{i}-{variant}";v=copy.deepcopy(base_values)
            v["label"]=value("text","A Ω中" if variant==0 else "中A B")
            if variant:
                v.update(ink=value("color",[79,229,108,201]),panel=value("color",[211,29,71,117]))
                if i==0:v["horizontal_offset"]=value("integer",-4)
                else:v["rise"]=value("curve",curve(-8,6,11,"ease_in_out"))
            expected=copy.deepcopy(t["scene"]);expected.update(id=name,duration=time(12,25))
            panel,label=expected["layers"]
            for layer in (panel,label):
                layer["duration"]=time(12,25);layer["animation"]={"opacity":copy.deepcopy(fade)}
                if i==0:layer["transform"]["position"][0]=-4 if variant else 0
                else:layer["animation"]["position_y"]=copy.deepcopy(v.get("rise",value("curve",t["parameters"][-1]["definition"]["default"]))["value"])
            label["graphics"].update(text=v["label"]["value"],fonts=copy.deepcopy(fonts))
            if variant:
                label["graphics"]["color"]=[79,229,108,201];panel["graphics"]["fill"]=[211,29,71,117]
            result=instantiate(t,v,name)
            assert result["defaulted_parameters"]==sorted(p["name"] for p in t["parameters"] if p["name"] not in v)
            receipt,ref=verify(result,expected,name)
            instances.append((t,v,name,result,receipt,ref))
    passed.append("templates.original_presets_and_pixels")

    rich=copy.deepcopy(templates[0]);rich["id"]="original-multiline-template"
    rich["parameters"]=[parameter("fonts","fonts",None,[binding("fonts","label")]),
        parameter("caption","text","A\nB",[binding("text","label")],max_chars=16),
        parameter("clock","time",time(6,25),[binding("scene_duration")]),
        parameter("life","time",time(5,25),[binding("layer_duration","label"),binding("layer_duration","panel")]),
        parameter("start","time",time(1,25),[binding("layer_start","label"),binding("layer_start","panel")]),
        parameter("backdrop","color",[70,20,100,255],[binding("scene_background")]),
        parameter("box","rect",[8,25,80,52],[binding("rect","panel")]),
        parameter("caption_box","rect",[12,28,72,44],[binding("rect","label")]),
        parameter("outline","color",[243,15,193,127],[binding("stroke_color","panel")]),
        parameter("outline_width","integer",4,[binding("stroke_width","panel")],min=1,max=10),
        parameter("line_space","integer",22,[binding("line_height","label")],min=1,max=30),
        parameter("tracking","integer",2,[binding("letter_spacing","label")],min=0,max=8),
        parameter("text_size","integer",20,[binding("font_size","label")],min=8,max=28),
        parameter("vertical_offset","integer",-4,[binding("position_y","label"),binding("position_y","panel")],min=-90,max=90),
        parameter("panel_opacity","integer",192,[binding("opacity","panel")],min=0,max=255),
        parameter("caption_opacity","integer",171,[binding("opacity","label")],min=0,max=255),
        parameter("slide","curve",curve(0,16,5,"ease_in"),[binding("position_x_curve","label")])]
    expected=copy.deepcopy(rich["scene"]);expected.update(id="rich",duration=time(6,25),background=[70,20,100])
    panel,label=expected["layers"]
    for layer in (panel,label):
        layer.update(start=time(1,25),duration=time(5,25));layer["transform"]["position"][1]=-4
    panel["graphics"].update(rect=[8,25,80,52],stroke={"width":4,"color":[243,15,193,127]});panel["transform"]["opacity"]=192
    label["graphics"].update(text="A\nB",fonts=copy.deepcopy(fonts),rect=[12,28,72,44],line_height=22,letter_spacing=2,size=20)
    label["transform"]["opacity"]=171;label["animation"]={"position_x":curve(0,16,5,"ease_in")}
    result=instantiate(rich,{"fonts":value("fonts",fonts)},"rich");verify(result,expected,"rich")
    passed.append("templates.typed_values_and_bindings")

    client=Client(exe)
    try:
        client.initialize()
        listing=client.rpc("tools/list")["result"]["tools"]
        descriptor=next(t for t in listing if t["name"]=="cutbolt_graphics_instantiate")
        assert descriptor["annotations"]["readOnlyHint"] and descriptor["annotations"]["idempotentHint"] and not descriptor["annotations"]["openWorldHint"]
        Draft202012Validator(descriptor["inputSchema"]).validate({"template":instances[0][0],"values":instances[0][1],"instance_id":instances[0][2],"input_root":str(sources)})
        for t,v,name,wanted,_,_ in reversed(instances):
            actual=client.call("graphics.instantiate",template=t,values=dict(reversed(list(v.items()))),instance_id=name,input_root=str(sources))
            assert actual==wanted
            path=root/(name+".json");path.write_text(json.dumps(actual["scene"],indent=2)+"\n",encoding="utf-8")
        t,v,name,wanted,_,_=instances[0]
        reordered=copy.deepcopy(t);reordered["parameters"].reverse()
        again=instantiate(reordered,v,name)
        assert again["scene"]==wanted["scene"] and again["scene_sha256"]==wanted["scene_sha256"]
        assert again["template_sha256"]!=wanted["template_sha256"]
        assert len({instances[i][3]["scene_sha256"] for i in range(4)})==4
        assert instances[0][3]["template_sha256"]==instances[1][3]["template_sha256"]
        passed.append("templates.reuse_isolation_and_hashes")

        project=request({"command":"project.create","id":"template-edit","width":640,"height":360,"frame_rate":time(25)})
        client.call("session.create",store_root=str(store),project=project,request_id="create")
        operations=[]
        for i in (0,1):
            asset=instances[i][4]["asset"]
            operations += [{"op":"media.add","asset":asset},{"op":"clip.append","clip":{"id":f"title-{i}","asset_id":asset["id"],"source_in":time(2,25),"duration":time(8,25)}}]
        fields={"store_root":str(store),"project_id":"template-edit","request_id":"two-titles","expected_revision":0,"operations":operations}
        receipt=client.call("session.apply",**fields);assert client.call("session.apply",**fields)==receipt
        saved=client.call("session.get",store_root=str(store),project_id="template-edit")
        request({"command":"render.run","project":saved,"input_root":str(output),"output_root":str(output),"output":str(output/"saved.mkv")})
        decode_check(output/"saved.mkv",640,360,lambda n:expected_frame(instances[n//8][5],oracle,2+n%8),16)
        assert audio_bytes(output/"saved.mkv")==b"\0"*(16*1920*4);decoded+=16
    finally:client.close()

    invalid=[]
    def bad(change,code="INVALID_TEMPLATE"):
        t=copy.deepcopy(templates[0]);v=copy.deepcopy(base_values);change(t,v);invalid.append((t,v,code))
    bad(lambda t,v:v.pop("fonts"),"MISSING_PARAMETER")
    bad(lambda t,v:v.update(typo=value("text","A")),"UNKNOWN_PARAMETER")
    bad(lambda t,v:v.update(label=value("integer",1)),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(label=value("text","")),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(label=value("text","A"*65)),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(font_size=value("integer",29)),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(fonts=value("fonts",[])),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(fade=value("curve",{"keys":[]})),"INVALID_PARAMETER")
    bad(lambda t,v:v.update(duration=value("time",{"num":1,"den":0})),"INVALID_TIME")
    bad(lambda t,v:v.update(label=value("text","مرحبا")),"UNSUPPORTED_TEXT")
    bad(lambda t,v:v.update(label=value("text","Z")),"MISSING_GLYPH")
    bad(lambda t,v:v.update(label=value("text","A"*20)),"TEXT_OVERFLOW")
    bad(lambda t,v:v["fonts"]["value"][0].update(sha256="0"*64),"MEDIA_CHANGED")
    bad(lambda t,v:v["fonts"]["value"][0].update(path="../primary.ttf"),"INVALID_PATH")
    bad(lambda t,v:v.update(duration=value("time",time(3,25))),"INVALID_ANIMATION")
    bad(lambda t,v:v.update(duration=value("time",time(1,100))),"UNALIGNED_TIME")
    bad(lambda t,v:v.update(fade=value("curve",curve(0,256))),"INVALID_ANIMATION")
    bad(lambda t,v:t.update(schema_version=2))
    bad(lambda t,v:t.update(id=" "))
    bad(lambda t,v:t.update(parameters=[parameter(f"p{i}","integer",0,[binding("position_x",f"layer-{i}")],min=0,max=1) for i in range(65)]))
    bad(lambda t,v:t["parameters"].append(parameter("too_many_targets","integer",0,[binding("position_x",f"layer-{i}") for i in range(257)],min=0,max=1)))
    bad(lambda t,v:t["parameters"].append(copy.deepcopy(t["parameters"][0])))
    bad(lambda t,v:t["parameters"][0].update(name="invalid name"))
    bad(lambda t,v:t["parameters"][0].update(bindings=[]))
    bad(lambda t,v:t["parameters"][0]["bindings"][0].update(layer="missing"))
    bad(lambda t,v:t["parameters"][0]["bindings"][0].update(layer="panel"))
    bad(lambda t,v:t["parameters"][0]["bindings"][0].update(property="opacity"))
    bad(lambda t,v:t["parameters"][0]["bindings"].append(copy.deepcopy(t["parameters"][0]["bindings"][0])))
    bad(lambda t,v:t["parameters"][0]["definition"].update(max_chars=0))
    bad(lambda t,v:t["parameters"][4]["definition"].update(min=30,max=20))
    bad(lambda t,v:t["parameters"][4]["definition"].update(default=40),"INVALID_PARAMETER")
    bad(lambda t,v:t["parameters"][5]["bindings"][0].update(layer="panel"))
    bad(lambda t,v:t["scene"]["layers"][1].update(id="panel"))
    bad(lambda t,v:t["scene"]["layers"][0].update(animation={"position_x":curve(0,5)}))
    bad(lambda t,v:t["parameters"].append(parameter("conflict","curve",curve(0,5),[binding("position_x_curve","panel")])))
    bad(lambda t,v:t["parameters"].append(parameter("no_stroke","integer",2,[binding("stroke_width","label")],min=1,max=5)))
    def absent_stroke(t,v):
        t["scene"]["layers"][0]["graphics"]["stroke"]=None
        t["parameters"].append(parameter("stroke_width","integer",2,[binding("stroke_width","panel")],min=1,max=5))
    bad(absent_stroke)
    bad(lambda t,v:t["parameters"].append(parameter("alpha_background","color",[1,2,3,4],[binding("scene_background")])))
    bad(lambda t,v:t["parameters"][0]["bindings"][0].update(property="../../text"),"INVALID_JSON")
    bad(lambda t,v:v["label"].update(script="anything"),"INVALID_JSON")
    # The instantiation path must remain read-only even on deep post-binding failures.
    before_files={str(p.relative_to(root)) for p in root.rglob('*') if p.is_file()}
    for t,v,code in invalid:instantiate(t,v,error=code)
    assert before_files=={str(p.relative_to(root)) for p in root.rglob('*') if p.is_file()}
    assert instantiate(instances[0][0],instances[0][1],instances[0][2])==instances[0][3]
    passed.append("templates.invalid_constraints_and_atomicity")
    render(instances[0][3]["scene"],instances[0][2],"OUTPUT_EXISTS")
    assert hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert originals=={p.name:p.read_bytes() for p in paths}
    assert not list(output.glob('.cutbolt-scene-*'))
    passed.append("templates.saved_session_mcp_preservation")
    report={"passed":passed,"decoded_frames":decoded,"silent_stereo_sample_frames":decoded*1920,"renders":rendered,"rejected_cases":len(invalid),
            "oracle":"Explicit independently assembled expected scenes plus original font rectangle geometry, analytic coverage, Fraction blending and forward transforms; all RGB/PCM compared exactly",
            "scope":"Typed reusable scene templates, original lower-third/title-card examples, isolation, validation, MCP and compiled saved-session edits; no scripts, expressions or automatic layout/retiming"}
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
