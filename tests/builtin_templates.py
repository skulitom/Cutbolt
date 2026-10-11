"""Every original catalog design, independent glyph/shape pixels, fitting and native clocks."""
import argparse
import copy
from fractions import Fraction as F
import hashlib
import json
from pathlib import Path
import subprocess

from PIL import Image
from fontTools.ttLib import TTFont
from jsonschema import Draft202012Validator
from agents import Client
from engine import ENGINE, MCP_CORE_TOOLS, MCP_TOOLS
from graphics import make_font
from native_scenes import expected_frame, GRAPHICS
from scenes import audio_bytes, identity, time
from templates import binding, parameter, value

ROOT = Path(__file__).resolve().parents[1]
RATES = [F(24), F(25), F(30), F(50), F(60), F(24000, 1001), F(30000, 1001), F(60000, 1001)]


def rational(t):
    return F(t["num"], t["den"])


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    inputs, output = root / "inputs", root / "output"
    inputs.mkdir(); output.mkdir()
    # Original abstract glyphs, exact quarter-em edges; no external typeface or rasterizer oracle.
    glyphs = {chr(n): (500, [(250 * (n % 2), 0, 250 * (n % 2 + 1), 500 + 250 * (n % 3 == 0))])
              for n in range(33, 127)}
    glyphs[" "] = (250, [])
    # A power-of-two font em keeps the rasterizer's floating-point scale exact at every integer
    # text size. The analytic oracle uses the equivalent quarter-em coordinates above.
    actual_glyphs = {c: (a * 1024 // 1000, [tuple(v * 1024 // 1000 for v in rect) for rect in rectangles])
                     for c, (a, rectangles) in glyphs.items()}
    make_font(inputs / "original.ttf", actual_glyphs)
    with TTFont(inputs / "original.ttf", recalcTimestamp=False) as font:
        font["head"].unitsPerEm = 1024
        font.save(inputs / "original.ttf")
    alpha_grid = Image.new("RGBA", (256, 2))
    alpha_grid.putdata([(17, 83, 231, a) for _ in range(2) for a in range(256)])
    alpha_grid.save(inputs / "alpha.png")
    fonts = [identity(inputs / "original.ttf", inputs)]
    font_data = {"original.ttf": glyphs}
    paths = [ROOT / "examples" / (n + "-template.json") for n in ("lower-third", "title-card")]
    paths += sorted((ROOT / "src/templates/designs").glob("*.json"))
    originals = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in [*paths, *inputs.iterdir()]}
    recipes = {t["id"]: t for t in (json.loads(p.read_text()) for p in paths)}
    passed, cases = [], []

    def request(command, error=None):
        process = subprocess.run([str(ENGINE)], input=json.dumps(command).encode(), capture_output=True, timeout=180)
        reply = json.loads(process.stdout)
        assert process.returncode == (1 if error else 0), (process.stderr, reply)
        if error:
            assert reply["error"]["code"] == error, reply
            return reply["error"]
        assert reply["ok"], reply
        return reply["result"]

    def instantiate(template_id, values=None, project=None, error=None, **extra):
        return request({"command": "graphics.instantiate", "template_id": template_id,
                        "values": {"fonts": value("fonts", fonts), **(values or {})},
                        "instance_id": "catalog-instance", "input_root": str(inputs),
                        **({"project": project} if project else {}), **extra}, error)

    def still(scene, name, frame=0):
        rate = rational(scene.get("frame_rate") or time(25))
        at = F(frame) / rate
        path = output / (name + ".png")
        request({"command": "scene.still", "scene": scene, "time": time(at.numerator, at.denominator),
                 "input_root": str(inputs), "output_root": str(output), "output": str(path)})
        with Image.open(path) as image:
            return image.convert("RGBA" if scene.get("transparent") else "RGB").tobytes()

    listing = request({"command": "templates.list"})
    assert listing["count"] == 20 and {t["id"] for t in listing["templates"]} == set(recipes)
    for entry in listing["templates"]:
        assert entry["purpose"] and "scene" not in entry
        definitions = {p["name"]: p["definition"] for p in entry["parameters"]}
        assert definitions["transparent"]["type"] == "boolean"
        assert definitions["width"]["type"] == definitions["output_scale"]["type"] == "integer"
        assert definitions["frame_rate"]["type"] == "time" and definitions["fonts"]["default"] is None
        assert request({"command": "templates.list", "template_id": entry["id"]})["templates"] == [entry]
    passed.append("builtin_templates.catalog_and_typed_defaults")

    for template_id, recipe in recipes.items():
        result = instantiate(template_id)
        scene = result["scene"]
        # Existing inline expansion independently supplies the authored default values and curves.
        inline = request({"command": "graphics.instantiate", "template": recipe,
                          "values": {"fonts": value("fonts", fonts)}, "instance_id": "catalog-instance",
                          "input_root": str(inputs)})["scene"]
        inline["frame_rate"] = time(25)
        assert scene == inline, template_id
        frames = int(rational(scene["duration"]) * 25)
        selected = sorted({0, frames // 2, frames - 1, *[int(rational(l["start"]) * 25) for l in scene["layers"]]})
        # Independent geometry/compositing case: snap synthetic glyph sizes to multiples of four
        # so their quarter-em edges are whole pixels. This isolates exact layout/compositing from
        # the font library's subpixel coverage quantization. Default glyph sizes are tested below.
        opaque = copy.deepcopy(inline); opaque["transparent"] = False; opaque["output_scale"] = 1
        for layer in opaque["layers"]:
            if layer["graphics"]["kind"] == "text":
                layer["graphics"]["size"] = max(4, layer["graphics"]["size"] // 4 * 4)
        for n in selected:
            assert still(opaque, f"{template_id}-opaque-{n}", n) == expected_frame(opaque, n, inputs, font_data), (template_id, n)
        # Actual default output (including alpha and scale) is compiled in full; selected decoded
        # frames must equal stills. This is a path-equivalence check, separate from the RGB oracle.
        movie = output / (template_id + ".mkv")
        receipt = request({"command": "scene.render", "scene": scene, "input_root": str(inputs),
                           "output_root": str(output), "output": str(movie)})
        assert receipt["scene_sha256"] == result["scene_sha256"] and receipt["frames"] == frames
        channels = 4 if scene.get("transparent") else 3
        size = scene["width"] * scene["height"] * scene["output_scale"] ** 2 * channels
        decode = subprocess.run(["ffmpeg", "-v", "error", "-i", str(movie), "-map", "0:v:0", "-fps_mode", "passthrough",
                                 "-pix_fmt", "rgba" if channels == 4 else "rgb24", "-f", "rawvideo", "-"],
                                capture_output=True, timeout=120, check=True).stdout
        assert len(decode) == frames * size
        for n in selected:
            assert decode[n * size:(n + 1) * size] == still(scene, f"{template_id}-default-{n}", n), (template_id, n)
        assert audio_bytes(movie) == b"\0" * frames * 1920 * 4
        cases.append({"id": template_id, "decoded_frames": frames, "oracle_and_still_frames": selected,
                      "transparent": bool(scene.get("transparent")), "maximum_rgb_delta": 0})
        GRAPHICS.clear()
        print(f"Verified {template_id}", flush=True)
    passed.append("builtin_templates.every_design_renders_with_independent_rgb_and_default_still_movie_equivalence")

    # Regression found by translucent catalog panels: scaler alpha rounding changed 238 to 239.
    # Exercise every input alpha and every supported integer output scale through both paths.
    alpha_scene = {"schema_version": 1, "id": "alpha-enlargement", "width": 256, "height": 2,
                   "output_scale": 1, "duration": time(1, 25), "background": [0, 0, 0],
                   "color": "srgb_straight_encoded", "transparent": True, "audio": None,
                   "layers": [{"id": "alpha", "canvas": [256, 2], "start": time(0), "duration": time(1, 25),
                               "frames": [{"image": identity(inputs / "alpha.png", inputs), "hold": time(1, 25),
                                           "offset": [0, 0], "anchor": [0, 0]}], "timing": "strict", "end": "hold_last",
                               "transform": {"position": [0, 0], "crop": [0, 0, 256, 2], "scale": 1,
                                             "quarter_turns": 0, "opacity": 255}}]}
    native = still(alpha_scene, "alpha-native")
    assert native[3::4] == bytes(range(256)) * 2
    for scale in range(1, 9):
        alpha_scene["output_scale"] = scale
        expected = Image.frombytes("RGBA", (256, 2), native).resize((256 * scale, 2 * scale), Image.Resampling.NEAREST).tobytes()
        assert still(alpha_scene, f"alpha-{scale}") == expected
        movie = output / f"alpha-{scale}.mkv"
        request({"command": "scene.render", "scene": alpha_scene, "input_root": str(inputs),
                 "output_root": str(output), "output": str(movie)})
        actual = subprocess.check_output(["ffmpeg", "-v", "error", "-i", str(movie), "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
        assert actual == expected, scale
    passed.append("builtin_templates.all_alpha_values_survive_every_output_scale")

    project = request({"command": "project.create", "id": "target", "width": 1920, "height": 1080, "frame_rate": time(30)})
    fitted = instantiate("cutbolt-title-card-v1", project=project)
    s = fitted["scene"]
    assert (s["width"], s["height"], s["output_scale"], s["frame_rate"]) == (1920, 1080, 1, time(30))
    assert fitted["project_defaulted_parameters"] == ["frame_rate", "height", "output_scale", "width"]
    assert fitted["target_project"] == {"id": "target", "revision": 0}
    assert s["layers"][0]["graphics"]["rect"] == [96, 156, 1728, 768]
    assert s["layers"][1]["graphics"]["rect"] == [192, 288, 1536, 468]
    assert s["layers"][1]["graphics"]["size"] == 240
    assert s["layers"][1]["animation"]["position_y"]["keys"][0]["value"] == 72
    override = instantiate("cutbolt-title-card-v1", {"width": value("integer", 320), "height": value("integer", 180),
                           "output_scale": value("integer", 2), "frame_rate": value("time", time(50)),
                           "transparent": value("boolean", True), "font_size": value("integer", 26)}, project)
    assert override["project_defaulted_parameters"] == []
    assert override["scene"]["transparent"] and override["scene"]["frame_rate"] == time(50)
    assert override["scene"]["layers"][1]["graphics"]["size"] == 26  # explicit final-pixel value
    still(override["scene"], "override", 5)
    portrait = copy.deepcopy(project); portrait.update(width=360, height=640)
    s = instantiate("cutbolt-title-center-v1", project=portrait)["scene"]
    assert (s["width"], s["height"], s["output_scale"]) == (360, 640, 1)
    still(s, "portrait")
    passed.append("builtin_templates.project_size_clock_and_explicit_overrides")

    timed = []
    for rate in RATES:
        for name, beats in (("countdown", 4), ("list-reveal", 3)):
            result = instantiate(f"cutbolt-{name}-v1", {"frame_rate": value("time", time(rate.numerator, rate.denominator))})
            s = result["scene"]
            beat = F(1001, 1000) if rate.denominator == 1001 else F(1)
            assert rational(s["duration"]) == beats * beat
            for layer in s["layers"]:
                assert (rational(layer["start"]) * rate).denominator == 1
                assert (rational(layer["duration"]) * rate).denominator == 1
                authored = next(l for l in recipes[f"cutbolt-{name}-v1"]["scene"]["layers"] if l["id"] == layer["id"])
                assert rational(layer["start"]) == rational(authored["start"]) * beat
                assert rational(layer["duration"]) == rational(authored["duration"]) * beat
            still(s, f"{name}-{rate.numerator}-{rate.denominator}", int(beat * rate))
            timed.append({"id": name, "rate": s["frame_rate"], "duration": s["duration"]})
    shortened = instantiate("cutbolt-countdown-v1", {"duration": value("time", time(6, 25))})["scene"]
    assert [rational(l["start"]) * 25 for l in shortened["layers"] if l["graphics"]["kind"] == "text"] == [0, 2, 3, 5]
    passed.append("builtin_templates.eight_native_clocks_and_shortened_layer_boundaries")

    # All five new output properties also work for portable inline templates, with no fitting.
    inline = copy.deepcopy(recipes["cutbolt-title-center-v1"])
    for name, kind, default in (("width", "integer", 320), ("height", "integer", 180), ("output_scale", "integer", 2),
                                ("frame_rate", "time", time(25)), ("transparent", "boolean", False)):
        inline["parameters"].append(parameter(name, kind, default, [binding("scene_" + name)],
                                               **({"min": 1, "max": 4096} if kind == "integer" else {})))
    def inline_call(t, values=None, error=None):
        return request({"command": "graphics.instantiate", "template": t, "project": project,
                        "instance_id": "inline-output", "input_root": str(inputs),
                        "values": {"fonts": value("fonts", fonts), **(values or {})}}, error)
    expanded = inline_call(inline)["scene"]
    assert expanded["width"] == 1920 and expanded["layers"][0]["canvas"] == [320, 180]
    invalid = copy.deepcopy(inline); invalid["parameters"][-1]["bindings"][0]["layer"] = "label"
    inline_call(invalid, error="INVALID_TEMPLATE")
    invalid = copy.deepcopy(inline); invalid["parameters"][-1]["definition"] = {"type": "integer", "default": 1, "min": 0, "max": 1}
    inline_call(invalid, error="INVALID_TEMPLATE")
    invalid = copy.deepcopy(inline); invalid["parameters"] = [p for p in invalid["parameters"] if p["name"] != "height"]
    next(p for p in invalid["parameters"] if p["name"] == "width")["bindings"].append(binding("scene_height"))
    inline_call(invalid, error="INVALID_TEMPLATE")
    assert inline_call(invalid, {"width": value("integer", 360)})["scene"]["height"] == 360
    passed.append("builtin_templates.inline_output_bindings_and_conflicting_defaults")

    before = {str(p) for p in root.rglob("*") if p.is_file()}
    for params, code in [({"width": value("integer", 0)}, "INVALID_PARAMETER"),
                         ({"height": value("integer", 4097)}, "INVALID_PARAMETER"),
                         ({"output_scale": value("integer", 9)}, "INVALID_PARAMETER"),
                         ({"transparent": value("integer", 1)}, "INVALID_PARAMETER"),
                         ({"frame_rate": value("time", time(27))}, "UNSUPPORTED_TIMELINE"),
                         ({"duration": value("time", time(1, 100))}, "UNALIGNED_TIME"),
                         ({"duration": value("time", time(1, 25))}, "INVALID_PARAMETER"),
                         ({"typo": value("text", "x")}, "UNKNOWN_PARAMETER")]:
        instantiate("cutbolt-countdown-v1", params, error=code)
    instantiate("missing", error="INVALID_TEMPLATE")
    instantiate(None, error="INVALID_TEMPLATE")
    instantiate("cutbolt-title-card-v1", template=inline, error="INVALID_TEMPLATE")
    request({"command": "graphics.instantiate", "template_id": "cutbolt-title-card-v1", "instance_id": "missing-font",
             "input_root": str(inputs)}, "MISSING_PARAMETER")
    assert before == {str(p) for p in root.rglob("*") if p.is_file()}
    assert originals == {str(p): hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in originals}
    passed.append("builtin_templates.rejected_inputs_and_preservation")

    catalog_bytes = {}
    for catalog in ("core", "full"):
        client = Client(ENGINE, args=("mcp", "--tools", catalog))
        try:
            client.initialize()
            descriptor = client.rpc("tools/list")["result"]
            assert len(descriptor["tools"]) == (MCP_CORE_TOOLS if catalog == "core" else MCP_TOOLS)
            catalog_bytes[catalog] = len(json.dumps(descriptor, separators=(",", ":"), ensure_ascii=False).encode())
            fields = {"template_id": "cutbolt-title-card-v1"}
            if catalog == "full":
                tool = next(t for t in descriptor["tools"] if t["name"] == "cutbolt_templates_list")
                assert tool["annotations"]["readOnlyHint"]
                Draft202012Validator(tool["inputSchema"]).validate(fields)
                found = client.call("templates.list", **fields)
                actual = client.call("graphics.instantiate", **fields, project=project, instance_id="catalog-instance",
                                     input_root=str(inputs), values={"fonts": value("fonts", fonts)}, detail="full")
            else:
                found = client.rpc(
                    "tools/call", {"name": "cutbolt_run", "arguments": {"command": "templates.list", "arguments": fields}})["result"]["structuredContent"]["result"]
                actual = client.rpc("tools/call", {"name": "cutbolt_run", "arguments": {"command": "graphics.instantiate",
                    "arguments": {**fields, "project": project, "instance_id": "catalog-instance", "input_root": str(inputs),
                                  "values": {"fonts": value("fonts", fonts)}, "detail": "full"}}})["result"]["structuredContent"]["result"]
            assert found["count"] == 1 and actual == fitted
        finally:
            client.close()
    passed.append("builtin_templates.cli_core_full_mcp_parity")
    report = {"passed": passed, "designs": cases, "native_clock_cases": timed, "catalog_bytes": catalog_bytes,
              "decoded_frames": sum(c["decoded_frames"] for c in cases),
              "oracle": "Original synthetic whole-pixel glyph variants, analytic shape coverage and independent integer RGB compositing at selected frames; actual default RGB/RGBA movie/still equivalence and all silent PCM samples",
              "scope": "20 original templates, typed output bindings, project fitting, native clocks and read-only catalog; graphics.place and scene shorthand remain separate work"}
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
