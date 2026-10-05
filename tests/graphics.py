"""Original outline fonts and independent geometry/layout/coverage acceptance checks."""
from engine import ENGINE, per_frame
import argparse
import copy
from fractions import Fraction as F
import hashlib
import importlib.metadata
import json
import math
from pathlib import Path
import subprocess
from fontTools.fontBuilder import FontBuilder
from fontTools.pens.ttGlyphPen import TTGlyphPen
from PIL import Image

from agents import Client
from animation import animated_scene_at
from compositing import expected_frame
from scenes import audio_bytes, identity, time

ROOT = Path(__file__).resolve().parents[1]
# The glyphs are original abstract rectangle motifs, not copies of typeface designs.
# Each value is (advance, outline rectangles in font units, y measured upwards).
PRIMARY = {" ":(300, []), "A":(600, [(50,0,500,700)]),
           "B":(600, [(150,-100,550,600)]), "C":(700, [(0,0,200,700),(300,500,600,700)])}
FALLBACK = {"A":(700, [(0,0,300,300)]), "Ω":(700, [(0,100,600,800)]),
            "中":(1000, [(0,0,300,700),(500,0,800,700)])}


def make_font(path, glyphs):
    builder = FontBuilder(1000, isTTF=True)
    order = [".notdef"] + [f"g{ord(c):04x}" for c in glyphs]
    builder.setupGlyphOrder(order)
    builder.setupCharacterMap({ord(c):f"g{ord(c):04x}" for c in glyphs})
    outlines, metrics = {}, {}
    for name, (advance, rectangles) in [(".notdef", (600, []))] + [(f"g{ord(c):04x}", v) for c,v in glyphs.items()]:
        pen = TTGlyphPen(None)
        for x0,y0,x1,y1 in rectangles:
            pen.moveTo((x0,y0)); pen.lineTo((x0,y1)); pen.lineTo((x1,y1)); pen.lineTo((x1,y0)); pen.closePath()
        outlines[name] = pen.glyph()
        metrics[name] = (advance, min((r[0] for r in rectangles), default=0))
    builder.setupGlyf(outlines)
    builder.setupHorizontalMetrics(metrics)
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    builder.setupNameTable({"familyName":"Cutbolt Original Fixture", "styleName":"Regular",
                           "uniqueFontIdentifier":path.stem, "fullName":path.stem, "psName":path.stem})
    builder.setupPost(); builder.setupMaxp()
    # Fixed original fixture timestamps keep repeated fixture generation deterministic.
    builder.font["head"].created = builder.font["head"].modified = 3800000000
    builder.save(path)


def nearest(value):
    return int(abs(value)+F(1,2)) * (-1 if value < 0 else 1)


def paint(image, x, y, color, coverage=255):
    old = image.getpixel((x,y))
    sa = F(nearest(F(color[3]*coverage,255)),255)
    da = F(old[3],255)
    alpha = sa + da*(1-sa)
    if alpha:
        rgb = [nearest((color[c]*sa+old[c]*da*(1-sa))/alpha) for c in range(3)]
        image.putpixel((x,y), (*rgb,nearest(255*alpha)))


def shape_pixels(graphic, canvas):
    image = Image.new("RGBA", tuple(canvas))
    x,y,w,h = graphic["rect"]
    def points(rect):
        left,top,width,height = rect
        if width <= 0 or height <= 0:
            return set()
        values=set()
        for py in range(max(0,top), min(canvas[1],top+height)):
            for px in range(max(0,left), min(canvas[0],left+width)):
                # Independent normalized ellipse equation at pixel centers.
                if graphic["shape"] == "rectangle" or (F(2*px+1-2*left-width,width)**2+F(2*py+1-2*top-height,height)**2 <= 1):
                    values.add((px,py))
        return values
    outer=points((x,y,w,h))
    for px,py in outer:
        paint(image,px,py,graphic["fill"])
    if graphic.get("stroke"):
        stroke=graphic["stroke"]; d=stroke["width"]
        for px,py in outer-points((x+d,y+d,w-2*d,h-2*d)):
            paint(image,px,py,stroke["color"])
    return image


def text_pixels(graphic, canvas, font_data):
    image = Image.new("RGBA", tuple(canvas)); lines=[[]]; widths=[F(0)]
    x,y,w,h = graphic["rect"]; size=graphic["size"]
    for c in graphic["text"]:
        if c == "\n":
            lines.append([]); widths.append(F(0)); continue
        font_index = next(i for i,font in enumerate(graphic["fonts"]) if c in font_data[font["path"]])
        advance, rectangles = font_data[graphic["fonts"][font_index]["path"]][c]
        advance=F(advance*size,1000)
        spacing = graphic["letter_spacing"] if lines[-1] else 0
        if graphic["wrap"] == "character" and lines[-1] and widths[-1]+spacing+advance > w:
            lines.append([]); widths.append(F(0)); spacing=0
        pen=widths[-1]+spacing
        lines[-1].append((c,font_index,pen,rectangles,advance)); widths[-1]=pen+advance
    glyphs=[];spans=[]
    for line, entries in enumerate(lines):
        shift={"left":F(0),"center":(w-widths[line])/2,"right":w-widths[line]}[graphic["align"]]
        block=len(lines)*graphic["line_height"]
        baseline=y+{"top":0,"middle":(h-block)//2,"bottom":h-block}[graphic.get("valign","top")]+size+line*graphic["line_height"]
        spans.append((x+nearest(shift),x+nearest(shift+widths[line]),baseline))
        for c,fi,pen,rectangles,advance in entries:
            origin=x+nearest(pen+shift)
            glyphs.append({"scalar":c,"font_index":fi,"line":line,"baseline":baseline,"advance":float(advance)})
            # Analytic pixel-area coverage of the known original outlines, no font rasterizer.
            coverage={}
            for a,b,d,e in rectangles:
                left,right=origin+F(a*size,1000),origin+F(d*size,1000)
                top,bottom=baseline-F(e*size,1000),baseline-F(b*size,1000)
                for py in range(math.floor(top),math.ceil(bottom)):
                    for px in range(math.floor(left),math.ceil(right)):
                        area=(min(right,px+1)-max(left,px))*(min(bottom,py+1)-max(top,py))
                        coverage[(px,py)]=coverage.get((px,py),F(0))+area
            for (px,py),area in coverage.items():
                if x <= px < x+w and y <= py < y+h:
                    paint(image,px,py,graphic["color"],nearest(area*255))
    if graphic.get("background") or graphic.get("outline"):
        # Independent decoration: padded line boxes once, a disc-dilated alpha outline, then the fill over both.
        decorated=Image.new("RGBA", tuple(canvas)); cw,ch=canvas
        if graphic.get("background"):
            b=graphic["background"]; pad=b["padding"]; covered=set()
            for x0,x1,baseline in spans:
                if x1 <= x0:continue
                for py in range(max(0,baseline-size-pad),min(ch,baseline-size+graphic["line_height"]+pad)):
                    for px in range(max(0,x0-pad),min(cw,x1+pad)):covered.add((px,py))
            for px,py in covered:paint(decorated,px,py,b["color"])
        if graphic.get("outline"):
            o=graphic["outline"]; r=o["width"]
            alpha={(px,py):image.getpixel((px,py))[3] for py in range(ch) for px in range(cw) if image.getpixel((px,py))[3]}
            near={(px+dx,py+dy) for px,py in alpha for dy in range(-r,r+1) for dx in range(-r,r+1) if dx*dx+dy*dy<=r*r and 0<=px+dx<cw and 0<=py+dy<ch}
            for px,py in near:
                paint(decorated,px,py,o["color"],max(alpha.get((px+dx,py+dy),0) for dy in range(-r,r+1) for dx in range(-r,r+1) if dx*dx+dy*dy<=r*r))
        for py in range(ch):
            for px in range(cw):
                pixel=image.getpixel((px,py))
                if pixel[3]:paint(decorated,px,py,pixel)
        image=decorated
    return image, glyphs, list(map(float,widths))


def run(root):
    root=root.resolve(); assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True,exist_ok=True)
    sources, output, oracle = root/"sources", root/"output", root/"oracle"
    sources.mkdir(); output.mkdir(); oracle.mkdir()
    make_font(sources/"primary.ttf", PRIMARY); make_font(sources/"fallback.ttf",FALLBACK)
    fonts=[identity(sources/name,sources) for name in ("primary.ttf","fallback.ttf")]
    font_data={"primary.ttf":PRIMARY,"fallback.ttf":FALLBACK}
    (sources/"invalid.ttf").write_bytes(b"not a font")
    (sources/"truncated.ttf").write_bytes(b"\x00\x01\x00\x00"+b"\x00"*8)
    original_hashes={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    exe=ENGINE; passed=[]; cases=[]; decoded=0
    def request(command,error=None):
        result=subprocess.run([str(exe)],input=json.dumps(command).encode(),capture_output=True,timeout=120)
        data=json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and data["error"]["code"] == error,(error,data)
            return data
        assert result.returncode == 0 and data["ok"],data
        return data["result"]
    def inspect(s,error=None):
        return request({"command":"scene.inspect","scene":s,"input_root":str(sources)},error)
    def render(s,name,error=None):
        return request({"command":"scene.render","scene":s,"input_root":str(sources),"output_root":str(output),"output":str(output/(name+".mkv"))},error)
    def layer(graphic,name="title"):
        return {"id":name,"canvas":[96,64],"start":time(0),"duration":time(5,25),"frames":[],"graphics":graphic,
                "timing":"strict","end":"hold_last","transform":{"position":[0,0],"crop":[0,0,96,64],"scale":1,"quarter_turns":0,"opacity":255}}
    def text(**fields):
        return {"kind":"text","text":"ABC Ω中","fonts":copy.deepcopy(fonts),"size":20,"color":[213,79,151,173],"rect":[2,1,92,62],
                "line_height":24,"letter_spacing":1,"align":"left","wrap":"none","overflow":"reject",**fields}
    def scene(layers):
        return {"schema_version":1,"id":"original-graphics","width":96,"height":64,"output_scale":1,"duration":time(5,25),
                "background":[17,53,119],"color":"srgb_straight_encoded","layers":layers,"audio":None}
    def expectation(s,name):
        expected=copy.deepcopy(s); layout=[]
        for i,item in enumerate(expected["layers"]):
            g=item.pop("graphics")
            if g["kind"] == "text":
                im,glyphs,widths=text_pixels(g,item["canvas"],font_data)
                layout.append((glyphs,widths))
            else:
                im=shape_pixels(g,item["canvas"]);layout.append(None)
            path=oracle/f"{name}-{i}.png";im.save(path)
            item["frames"]=[{"image":identity(path,oracle),"hold":item["duration"],"offset":[0,0],"anchor":[0,0]}]
        return expected,layout
    def verify_movie(path,s,reference,tolerance=0):
        nonlocal decoded
        frames=int(F(s["duration"]["num"],s["duration"]["den"])*25)
        actual=subprocess.check_output(["ffmpeg","-v","error","-i",str(path),"-an","-pix_fmt","rgb24","-f","rawvideo","-"],timeout=90)
        size=s["width"]*s["height"]*s["output_scale"]**2*3
        assert len(actual)==frames*size
        largest=0
        for n in range(frames):
            wanted=expected_frame(reference,oracle,n);frame=actual[n*size:(n+1)*size]
            largest=max(largest,max(abs(a-b) for a,b in zip(frame,wanted)))
            if largest>tolerance:
                mismatch=next((i,a,b) for i,(a,b) in enumerate(zip(frame,wanted)) if abs(a-b)>tolerance)
                raise AssertionError((path.name,n,largest,mismatch))
        assert audio_bytes(path)==b"\0"*(frames*1920*4)
        decoded+=frames
        return largest
    def check_case(s,name,tolerance=0):
        reference,layouts=expectation(s,name)
        info=inspect(s)
        for item,layout in zip(info["timing"],layouts):
            if layout:
                actual=item["graphics"]
                assert [{k:g[k] for k in layout[0][0]} for g in actual["glyphs"]] == layout[0]
                assert actual["line_widths"] == layout[1]
        receipt=render(s,name)
        delta=verify_movie(output/(name+".mkv"),s,reference,tolerance)
        cases.append({"name":name,"frames":receipt["frames"],"maximum_rgb_delta":delta,"allowed_delta":tolerance})
        return info,receipt,reference

    caps=request({"command":"capabilities","section":"all"})
    assert caps["graphics"]["complex_shaping"] is True and caps["graphics"]["layout"] == "ltr_scalar_v1"
    static=scene([layer(text())])
    info,_,_=check_case(static,"fonts-fallback")
    assert len(info["sources"])==2 and [g["font_index"] for g in info["timing"][0]["graphics"]["glyphs"]]==[0,0,0,0,1,1]
    reverse=scene([layer(text(text="A",fonts=list(reversed(fonts))))])
    check_case(reverse,"fallback-order")
    for align in ("left","center","right"):
        check_case(scene([layer(text(text="AB CΩ\n中A",rect=[2,1,38,62],align=align,wrap="character",line_height=20))]),"wrap-"+align)
    for valign in ("middle","bottom"):
        info,_,_=check_case(scene([layer(text(text="AB CΩ\n中A",rect=[2,0,38,64],align="center",valign=valign,wrap="character",line_height=20))]),"valign-"+valign)
        assert info["timing"][0]["graphics"]["glyphs"][0]["baseline"]>20
    request({"command":"scene.inspect","scene":scene([layer(text(text="A\nB\nC",rect=[2,1,92,40],valign="bottom"))]),"input_root":str(sources)},"TEXT_OVERFLOW")
    decor=text(text="AB CΩ\n\n中A",rect=[4,2,88,60],align="center",valign="middle",wrap="character",line_height=18,
               background={"color":[10,20,30,150],"padding":3},outline={"color":[240,200,40,220],"width":2})
    check_case(scene([layer(decor)]),"background-outline")
    for bad in [{"background":{"color":[0,0,0,0],"padding":257}},{"outline":{"color":[0,0,0,0],"width":0}},{"outline":{"color":[0,0,0,0],"width":9}}]:
        request({"command":"scene.inspect","scene":scene([layer(text(**bad))]),"input_root":str(sources)},"INVALID_GRAPHIC")
    clipped=scene([layer(text(text="ABCC\n中Ω",rect=[2,1,20,23],overflow="clip",align="right"))])
    clip_info,_,_=check_case(clipped,"explicit-clip")
    assert clip_info["timing"][0]["graphics"]["clipped_coverage_pixels"]>0
    check_case(scene([layer(text(text="ABC",size=25,line_height=30))]),"fractional-coverage",1)
    check_case(scene([layer(text(text="A",size=40,rect=[0,0,96,64],color=[0,0,0,0]))]),"transparent-text")
    passed.append("graphics.original_fonts_layout_fallback")

    shapes=[]
    for name,shape,rect in [("box","rectangle",[-3,2,41,33]),("oval","ellipse",[20,10,45,37]),("thin","ellipse",[70,12,1,15])]:
        shapes.append(layer({"kind":"shape","shape":shape,"rect":rect,"fill":[77,201,43,129],"stroke":{"width":3,"color":[211,23,92,181]}},name))
    shape_scene=scene(shapes)
    check_case(shape_scene,"shapes-strokes")
    shape_scene["layers"][1]["blend_mode"]="multiply"
    shape_scene["layers"][2]["blend_mode"]="screen"
    check_case(shape_scene,"shapes-blends")
    passed.append("graphics.shapes_transparency_strokes")

    def curve(a,b,mode="linear"):
        return {"keys":[{"time":time(0),"value":a,"interpolation":mode},{"time":time(16,25),"value":b,"interpolation":"hold"}]}
    moving=scene([copy.deepcopy(shapes[1]),layer(text(text="AB Ω",rect=[0,0,70,32]),"moving-text")])
    moving["duration"]=time(20,25);moving["output_scale"]=2
    for i,item in enumerate(moving["layers"]):
        item.update(start=time(i+1,25),duration=time(18-i,25),animation={"position_x":curve(-5,25,"ease_in_out"),"position_y":curve(0,-6),"opacity":curve(0,231)})
        item["mask"]={"rect":[0,0,96,64],"inverted":bool(i),"animation":{"width":curve(0,20 if i else 96,"ease_out")}}
    moving["layers"][0]["transform"].update(crop=[3,4,80,50],quarter_turns=1,scale=1,position=[80,0])
    moving["layers"][0]["animation"]["position_x"]=curve(80,96,"ease_in_out")
    move_info,move_receipt,move_reference=check_case(moving,"animated")
    for i,item in enumerate(move_info["timing"]):
        for n,parameters in enumerate(per_frame(item["sampled_parameters"])):
            active=i+1 <= n < 19
            assert per_frame(item["selected_frames"])[n] == (0 if active else None)
            if active:
                t=animated_scene_at(moving,n)["layers"][i]["transform"]
                assert parameters["position"] == t["position"] and parameters["opacity"] == t["opacity"]
    assert expected_frame(move_reference,oracle,4)!=expected_frame(move_reference,oracle,12)
    for item in move_reference["layers"]:
        single={**move_reference,"layers":[item]}
        assert expected_frame(single,oracle,4)!=expected_frame(single,oracle,12),item["id"]
    reordered=copy.deepcopy(moving)
    for item in reordered["layers"]:
        for c in item["animation"].values(): c["keys"].reverse()
    assert inspect(reordered)==move_info
    check_case(json.loads(json.dumps(reordered)),"reloaded-animation")
    passed.append("graphics.animated_text_shapes_masks")
    # Stills: the frame on screen at a time, at output size, equal to the independent expectation
    # (and, for a transparent scene, to the compiled movie's RGBA frame).
    def still(s,name,t,error=None):
        return request({"command":"scene.still","scene":s,"input_root":str(sources),"output_root":str(output),"output":str(output/(name+".png")),"time":t},error)
    for n,t in ((0,time(0)),(4,time(4,25)),(12,time(62,125)),(19,time(19,25))):
        shown=still(moving,f"still-{n}",t);assert shown["frame"]==n and shown["time"]==time(n,25) and (shown["width"],shown["height"])==(192,128)
        with Image.open(output/f"still-{n}.png") as image:assert image.mode=="RGB" and image.tobytes()==expected_frame(move_reference,oracle,n),n
    clear=copy.deepcopy(moving);clear.update(id="moving-alpha",transparent=True)
    for item in clear["layers"]:item["blend_mode"]="normal"
    request({"command":"scene.render","scene":clear,"input_root":str(sources),"output_root":str(output),"output":str(output/"moving-alpha.mkv")})
    rgba=subprocess.check_output(["ffmpeg","-v","error","-i",str(output/"moving-alpha.mkv"),"-an","-pix_fmt","rgba","-f","rawvideo","-"],timeout=90)
    size=192*128*4
    for n in (2,9):
        still(clear,f"still-alpha-{n}",time(n,25))
        with Image.open(output/f"still-alpha-{n}.png") as image:assert image.mode=="RGBA" and image.tobytes()==rgba[n*size:(n+1)*size],n
    still(moving,"still-late",time(20,25),"INVALID_RANGE")
    still(moving,"still-0",time(0),"OUTPUT_EXISTS")
    request({"command":"scene.still","scene":moving,"input_root":str(sources),"output_root":str(output),"output":str(output/"still.jpg")},"UNSUPPORTED_OUTPUT")
    viewer=Client(exe)
    try:
        viewer.initialize()
        reply=viewer.rpc("tools/call",{"name":"cutbolt_scene_still","arguments":{"scene":moving,"input_root":str(sources),"output_root":str(output),"output":str(output/"still-mcp.png"),"time":time(4,25)}})["result"]
        assert [c["type"] for c in reply["content"]]==["text","image"] and reply["structuredContent"]["result"]["frame"]==4
    finally:viewer.close()
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("graphics.scene_stills_match_frames")

    client=Client(exe)
    try:
        client.initialize()
        assert client.call("scene.inspect",scene=moving,input_root=str(sources))==move_info
        project=request({"command":"project.create","id":"graphics-edit","width":192,"height":128,"frame_rate":time(25)})
        store=root/"store";store.mkdir()
        client.call("session.create",store_root=str(store),project=project,request_id="create")
        edit={"store_root":str(store),"project_id":"graphics-edit","request_id":"compiled-title","expected_revision":0,"operations":[
            {"op":"media.add","asset":move_receipt["asset"]},
            {"op":"clip.append","clip":{"id":"title","asset_id":moving["id"],"source_in":time(3,25),"duration":time(10,25)}}]}
        receipt=client.call("session.apply",**edit);assert client.call("session.apply",**edit)==receipt
        saved=client.call("session.get",store_root=str(store),project_id="graphics-edit")
        request({"command":"render.run","project":saved,"input_root":str(output),"output_root":str(output),"output":str(output/"timeline.mkv")})
        raw=subprocess.check_output(["ffmpeg","-v","error","-i",str(output/"timeline.mkv"),"-an","-pix_fmt","rgb24","-f","rawvideo","-"],timeout=90)
        assert raw==b"".join(expected_frame(move_reference,oracle,n) for n in range(3,13));decoded+=10
        assert audio_bytes(output/"timeline.mkv")==b"\0"*(10*1920*4)
    finally: client.close()
    passed.append("graphics.mcp_saved_session_replay")

    invalid=[]
    def bad(change,code="INVALID_GRAPHIC"):
        s=copy.deepcopy(static);change(s["layers"][0]);invalid.append((s,code))
    bad(lambda l:l.pop("graphics"),"INVALID_SCENE")
    bad(lambda l:l.update(frames=[{"image":fonts[0],"hold":time(1,25),"offset":[0,0],"anchor":[0,0]}]),"INVALID_SCENE")
    for field,value in [("end","loop"),("timing","sample_start"),("alpha_mode","premultiplied")]:
        bad(lambda l,f=field,v=value:l.update({f:v}),"INVALID_SCENE")
    for key,value in [("text",""),("text","A"*1025),("size",0),("size",513),("line_height",0),("letter_spacing",129),("fonts",[]),("fonts",fonts*3),("rect",[-1,0,30,30]),("rect",[0,0,97,64])]:
        bad(lambda l,k=key,v=value:l["graphics"].update({k:v}))
    for value in ["A\tB","A\rB","A\u0301","مرحبا","A\u202eB","😀"]:
        bad(lambda l,v=value:l["graphics"].update(text=v),"UNSUPPORTED_TEXT")
    bad(lambda l:l["graphics"].update(text="Z"),"MISSING_GLYPH")
    bad(lambda l:l["graphics"].update(rect=[0,0,10,20]),"TEXT_OVERFLOW")
    bad(lambda l:l["graphics"].update(text="B",rect=[0,0,20,20]),"TEXT_OVERFLOW")
    bad(lambda l:l["graphics"]["fonts"][0].update(sha256="0"*64),"MEDIA_CHANGED")
    bad(lambda l:l["graphics"]["fonts"][0].update(bytes=8388609),"LIMIT_EXCEEDED")
    for path,code in [("../primary.ttf","INVALID_PATH"),(str(sources/"primary.ttf"),"INVALID_PATH"),("missing.ttf","IO_ERROR")]:
        bad(lambda l,p=path:l["graphics"]["fonts"][0].update(path=p),code)
    for path in ["invalid.ttf","truncated.ttf"]:
        bad(lambda l,p=path:l["graphics"].update(fonts=[identity(sources/p,sources)]),"UNSUPPORTED_FONT")
    bad(lambda l:l["graphics"].update(fonts=[fonts[0],{**fonts[0],"sha256":"0"*64}]),"INVALID_IDENTITY")
    bad(lambda l:l["graphics"].update(fill=[0,0,0,0]),"INVALID_JSON")
    for shape in [{"kind":"shape","shape":"rectangle","rect":[0,0,0,2],"fill":[0,0,0,0],"stroke":None},
                  {"kind":"shape","shape":"ellipse","rect":[0,0,3,2],"fill":[0,0,0,0],"stroke":{"width":0,"color":[1,2,3,4]}}]:
        bad(lambda l,g=shape:l.update(graphics=g))
    for i,(s,code) in enumerate(invalid):
        render(s,f"invalid-{i}",code);assert not (output/f"invalid-{i}.mkv").exists()
    saved_hash=hashlib.sha256((output/"animated.mkv").read_bytes()).hexdigest()
    render(moving,"animated","OUTPUT_EXISTS")
    assert saved_hash==hashlib.sha256((output/"animated.mkv").read_bytes()).hexdigest()
    assert original_hashes=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert not list(output.glob(".cutbolt-scene-*"))
    passed.append("graphics.validation_source_output_preservation")
    report={"passed":passed,"decoded_frames":decoded,"silent_stereo_sample_frames":decoded*1920,"cases":cases,"rejected_cases":len(invalid),
            "oracle":"Original generated TrueType rectangle outlines; analytic pixel-area glyph coverage, rational layout/blending and forward Pillow transforms; every RGB frame and silent PCM sample compared",
            "scope":"Default LTR scalar text, explicit ordered fallback and shapes; position/opacity/mask animation. Unicode shaping is verified separately by unicode_text.py; templates and captions are outside this fixture.",
            "development_dependencies":{"fonttools":importlib.metadata.version("fonttools"),"Pillow":importlib.metadata.version("Pillow")}}
    (root/"scene.json").write_text(json.dumps(moving,indent=2)+"\n",encoding="utf-8")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser=argparse.ArgumentParser();parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
