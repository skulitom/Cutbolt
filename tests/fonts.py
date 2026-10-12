"""Installed-font discovery with original generated fonts and isolated per-process font folders."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys

from PIL import Image
from fontTools.ttLib import TTCollection, TTFont
from agents import Client
from engine import ENGINE, MCP_TOOLS
from graphics import PRIMARY, make_font, text_pixels
from scenes import time

ROOT = Path(__file__).resolve().parents[1]
FAMILY = "Cutbolt Original Fixture"


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    inputs, output, store, home = [root / name for name in ("inputs", "output", "store", "home")]
    for folder in (inputs, output, store, home):
        folder.mkdir()
    env = os.environ.copy()
    env["HOME"] = str(home)
    if os.name == "nt":
        env["WINDIR"] = str(home / "Windows")
        env["LOCALAPPDATA"] = str(home / "Local")
        fonts = home / "Local/Microsoft/Windows/Fonts"
        (home / "Windows/Fonts").mkdir(parents=True)
    elif sys.platform == "darwin":
        fonts = home / "Library/Fonts"
    else:
        fonts = home / ".local/share/fonts"
    fonts.mkdir(parents=True)
    first, second = [fonts / (name + ".ttf") for name in ("CutboltFixtureRegular", "CutboltFixtureBold")]
    for path in (first, second):
        make_font(path, PRIMARY)
    collection = TTCollection()
    collection.fonts = [TTFont(first), TTFont(second)]
    for i, font in enumerate(collection.fonts):
        font["name"].names = []
        font["name"].setName("Cutbolt Collection Fixture", 1, 3, 1, 0x409)
        font["name"].setName("Regular", 2, 3, 1, 0x409)
        font["name"].setName(f"Cutbolt Collection Face {i}", 4, 3, 1, 0x409)
    collection.save(fonts / "collection.ttc")
    collection.close()
    (fonts / "not-a-font.ttf").write_bytes(b"original invalid font fixture")
    outside = root / "outside.ttf"
    make_font(outside, PRIMARY)
    originals = {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (first, second, outside)}
    passed = []

    def request(command, error=None):
        process = subprocess.run([str(ENGINE)], input=json.dumps(command).encode(), env=env,
                                 capture_output=True, timeout=90)
        data = json.loads(process.stdout)
        assert process.returncode == (1 if error else 0), (process.stderr, data)
        if error:
            assert data["error"]["code"] == error, data
            return data["error"]
        assert data["ok"], data
        return data["result"]

    found = request({"command": "fonts.find", "family": FAMILY, "limit": 1})
    assert found["matched"] == 2 and found["next_offset"] == 1, found
    assert found["skipped_count"] >= 1
    page = request({"command": "fonts.find", "family": FAMILY.lower(), "offset": 1, "limit": 1})
    assert page["next_offset"] is None
    entries = found["fonts"] + page["fonts"]
    assert all(entry["usable"] and entry["reason"] is None for entry in entries)
    selected = next(entry for entry in entries if first.stem in entry["full_names"])
    pinned = selected["identity"]
    assert os.path.samefile(pinned["path"], first)
    assert pinned["sha256"] == originals[str(first)] and pinned["bytes"] == first.stat().st_size
    assert request({"command": "fonts.find", "family": "CutboltNoSuchFamily"})["fonts"] == []
    unsupported = request({"command": "fonts.find", "family": "Cutbolt Collection Fixture"})
    assert unsupported["matched"] == 1 and not unsupported["fonts"][0]["usable"]
    assert "collections" in unsupported["fonts"][0]["reason"]
    request({"command": "fonts.find", "limit": 0}, "INVALID_PARAMETER")
    passed.append("fonts.filtered_pagination_and_content_identities")

    graphic = {"kind": "text", "text": "ABC", "fonts": [first.stem], "size": 20,
               "color": [71, 139, 233, 207], "rect": [3, 2, 58, 24], "line_height": 24,
               "letter_spacing": 0, "align": "left", "wrap": "none", "overflow": "reject"}
    scene = {"schema_version": 1, "id": "font-title", "width": 64, "height": 32,
             "output_scale": 1, "duration": time(1), "background": [0, 0, 0],
             "color": "srgb_straight_encoded", "audio": None, "transparent": True,
             "layers": [{"id": "label", "canvas": [64, 32], "start": time(0), "duration": time(1),
                         "frames": [], "graphics": graphic, "timing": "strict", "end": "hold_last",
                         "transform": {"position": [0, 0], "scale": 1, "quarter_turns": 0,
                                       "opacity": 255, "crop": [0, 0, 64, 32]}}]}

    def inspect(value, error=None):
        return request({"command": "scene.inspect", "scene": value, "input_root": str(inputs)}, error)

    named = inspect(scene)
    assert named["resolved_identities"] == [{**pinned, "font_name": first.stem}]
    explicit = copy.deepcopy(scene)
    explicit["layers"][0]["graphics"]["fonts"] = [pinned]
    assert {k: v for k, v in named.items() if k != "resolved_identities"} == inspect(explicit)
    ambiguous = copy.deepcopy(scene)
    ambiguous["layers"][0]["graphics"]["fonts"] = [FAMILY]
    failure = inspect(ambiguous, "UNSUPPORTED_FONT")
    assert failure["path"] == "scene.layers[0].graphics.fonts[0]" and len(failure["detail"]["matches"]) == 2
    assert "fonts.find" in failure["hint"]
    ambiguous["layers"][0]["graphics"]["fonts"] = ["CutboltNoSuchFamily"]
    assert inspect(ambiguous, "UNSUPPORTED_FONT")["detail"]["matches"] == []
    ambiguous["layers"][0]["graphics"]["fonts"] = ["Cutbolt Collection Fixture"]
    assert not inspect(ambiguous, "UNSUPPORTED_FONT")["detail"]["matches"][0]["usable"]
    passed.append("fonts.explicit_full_name_and_ambiguous_family")

    for name, value in (("named", scene), ("explicit", explicit)):
        request({"command": "scene.still", "scene": value, "input_root": str(inputs),
                 "output_root": str(output), "output": str(output / (name + ".png"))})
    expected_graphic = copy.deepcopy(explicit["layers"][0]["graphics"])
    expected_graphic["fonts"] = [{"path": "original.ttf"}]
    expected = text_pixels(expected_graphic, [64, 32], {"original.ttf": PRIMARY})[0]
    assert Image.open(output / "named.png").tobytes() == Image.open(output / "explicit.png").tobytes()
    opaque = copy.deepcopy(scene)
    opaque["transparent"] = False
    request({"command": "scene.still", "scene": opaque, "input_root": str(inputs),
             "output_root": str(output), "output": str(output / "opaque.png")})
    # The documented encoded-sRGB blend over black, independently computed from original glyph geometry.
    expected_rgb = bytes((c * a + 127) // 255 for r, g, b, a in expected.getdata() for c in (r, g, b))
    assert Image.open(output / "opaque.png").convert("RGB").tobytes() == expected_rgb
    passed.append("fonts.absolute_installed_file_and_exact_original_glyph_pixels")

    path_only = copy.deepcopy(explicit)
    path_only["layers"][0]["graphics"]["fonts"] = [{"path": pinned["path"]}]
    assert inspect(path_only)["resolved_identities"] == [pinned]
    bad = copy.deepcopy(explicit)
    bad["layers"][0]["graphics"]["fonts"][0]["path"] = str(outside)
    inspect(bad, "INVALID_PATH")
    bad["layers"][0]["graphics"]["fonts"] = [{"path": str(outside)}]
    inspect(bad, "PATH_OUTSIDE_ROOT")
    request({"command": "scene.inspect", "scene": bad, "input_root": str(root)}, "INVALID_PATH")
    bad = copy.deepcopy(explicit)
    bad["layers"][0]["graphics"]["fonts"][0]["sha256"] = "0" * 64
    inspect(bad, "MEDIA_CHANGED")
    # An installed path is an exception for text fonts alone, never an image or audio source.
    bad = copy.deepcopy(explicit)
    layer = bad["layers"][0]
    layer.pop("graphics")
    layer["frames"] = [{"image": pinned, "hold": time(1), "offset": [0, 0], "anchor": [0, 0]}]
    inspect(bad, "INVALID_PATH")
    passed.append("fonts.path_only_completion_hash_and_containment")

    template = json.loads((ROOT / "examples/title-card-template.json").read_text(encoding="utf-8"))
    values = {"fonts": {"type": "fonts", "value": [first.stem]}, "label": {"type": "text", "value": "ABC"}}
    instance = request({"command": "graphics.instantiate", "template": template, "instance_id": "named-template",
                        "values": values, "input_root": str(inputs)})
    text = next(layer["graphics"] for layer in instance["scene"]["layers"] if layer["graphics"]["kind"] == "text")
    assert text["fonts"] == [pinned] and instance["resolved_identities"][0]["font_name"] == first.stem
    values["fonts"]["value"] = [pinned]
    resolved = request({"command": "graphics.instantiate", "template": template, "instance_id": "named-template",
                        "values": values, "input_root": str(inputs)})
    assert instance["scene_sha256"] == resolved["scene_sha256"]
    passed.append("fonts.template_resolution_preserves_scene_digest")

    document = {"schema_version": 1, "id": "font-captions", "revision": 0, "overlap": "reject",
                "styles": {"main": {"color": [255, 255, 255]}},
                "cues": [{"id": "cue", "start": time(0), "end": time(1), "text": "ABC",
                          "style": "main", "align": "left", "speaker": None}]}
    captioned = request({"command": "captions.scene", "scene": explicit, "scene_id": "captioned",
                         "document": document, "offset": time(0), "sampling": "sample_start", "layer_prefix": "cc",
                         "layouts": {"main": {k: graphic[k] for k in ("fonts", "size", "rect", "line_height", "letter_spacing", "wrap", "overflow")}},
                         "input_root": str(inputs)})
    assert captioned["scene"]["layers"][-1]["graphics"]["fonts"] == [pinned]
    assert captioned["resolved_identities"][0]["font_name"] == first.stem
    passed.append("fonts.caption_layout_resolution")

    client = Client(ENGINE, env=env)
    try:
        client.initialize()
        tools = client.rpc("tools/list")["result"]["tools"]
        assert len(tools) == MCP_TOOLS
        tool = next(tool for tool in tools if tool["name"] == "cutbolt_fonts_find")
        assert tool["annotations"]["readOnlyHint"]
        assert client.call("fonts.find", family=FAMILY)["matched"] == 2
        ticket = client.call("job.start", job_root=str(store), request_id="named-font-job", run="scene.render",
                             arguments={"scene": scene, "input_root": str(inputs), "output_root": str(output),
                                        "output": str(output / "queued.mkv")})
        # Inspect the durable request: the worker gets a content identity, never a family string.
        with sqlite3.connect(store / "jobs.sqlite3") as database:
            saved = json.loads(database.execute("SELECT request FROM jobs WHERE id=?", (ticket["job_id"],)).fetchone()[0])
        assert saved["command"]["scene"]["layers"][0]["graphics"]["fonts"] == [pinned], saved
        for _ in range(5):
            finished = client.call("job.wait", job_root=str(store), job_id=ticket["job_id"], timeout_seconds=30, detail="full")
            if finished["finished"]:
                break
        assert finished["status"] == "completed", finished
    finally:
        client.close()
    client = Client(ENGINE, env=env, args=("--workspace", str(inputs), "mcp", "--tools", "core"))
    try:
        client.initialize()
        result = client.rpc("tools/call", {"name": "cutbolt_run", "arguments": {
            "command": "fonts.find", "arguments": {"family": FAMILY}}})["result"]["structuredContent"]
        assert result["ok"] and result["result"]["matched"] == 2, result
        result = client.rpc("tools/call", {"name": "cutbolt_run", "arguments": {
            "command": "scene.inspect", "arguments": {"scene": scene, "detail": "full"}}})["result"]["structuredContent"]
        assert result["ok"] and os.path.samefile(result["result"]["resolved_identities"][0]["path"], first), result
    finally:
        client.close()
    passed.append("fonts.mcp_read_only_discovery_and_queued_content_pinning")
    assert originals == {str(p): hashlib.sha256(p.read_bytes()).hexdigest() for p in (first, second, outside)}
    assert not any(p.is_file() for p in inputs.rglob("*")), "Discovery or rendering copied a file into the input root"
    report = {"passed": passed, "oracle": "Original generated rectangle glyphs, analytic coverage and exact RGB blending; named and explicit RGBA are identical",
              "scope": "Isolated per-process OS font folders, CLI and MCP discovery, names and identities, saved-job pinning, containment, source preservation"}
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
