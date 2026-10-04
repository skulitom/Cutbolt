"""Native-resolution scenes (output_scale 1 up to 4096 per axis) and tilemap layers.

Independent checks: a vectorised integer oracle for every decoded 1080p frame, exact equivalence between a
4x enlarged 480x270 scene and its native 1920x1080 counterpart, analytic large-text coverage, captions at
native size, downstream preview/export, explicit limits and measured time/memory/scratch use.
"""
import argparse
import copy
import hashlib
import json
from fractions import Fraction as F
from pathlib import Path
import subprocess
import threading
import time as clock
import wave

import numpy as np
import psutil
from PIL import Image

from animation import animated_scene_at
from graphics import PRIMARY, make_font, shape_pixels, text_pixels
from scenes import audio_bytes, decode_check, identity, selected, time

ROOT = Path(__file__).resolve().parents[1]
EXE = ROOT / "target" / "debug" / "cutbolt.exe"
W, H = 1920, 1080
TILE = (240, 216)


def call(request, code=None):
    result = json.loads(subprocess.run([str(EXE)], input=json.dumps(request), text=True, encoding="utf-8",
                                       capture_output=True, timeout=3600).stdout)
    if code:
        assert not result["ok"] and result["error"]["code"] == code, (code, result)
        return result["error"]
    assert result["ok"], result
    return result["result"]


def png(path, array):
    Image.fromarray(np.asarray(array, dtype=np.uint8), "RGBA").save(path)


def frame(path, sources, hold=time(1, 25), offset=(0, 0), anchor=(0, 0)):
    return {"image": identity(path, sources), "hold": hold, "offset": list(offset), "anchor": list(anchor)}


def layer(id, canvas, frames, position=(0, 0), crop=None, scale=1, turns=0, opacity=255, frames_total=225, **extra):
    base = {"id": id, "canvas": list(canvas), "start": time(0), "duration": time(frames_total, 25), "frames": frames,
            "timing": "strict", "end": "loop",
            "transform": {"position": list(position), "crop": list(crop or [0, 0, *canvas]), "scale": scale,
                          "quarter_turns": turns, "opacity": opacity}}
    base.update(extra)
    return base


def keys(*pairs, mode="linear"):
    return {"keys": [{"time": time(*t) if isinstance(t, tuple) else time(t), "value": v, "interpolation": mode} for t, v in pairs]}


# ---------------------------------------------------------------- original synthetic sources
def generate(sources):
    rng = np.random.default_rng(20261004)
    tw, th = TILE
    yy, xx = np.mgrid[0:th, 0:tw]
    checker = np.zeros((th, tw, 4), np.uint8)
    dark = ((xx // 24 + yy // 24) % 2).astype(bool)
    checker[..., :3] = np.where(dark[..., None], [18, 22, 48], [28, 34, 70])
    checker[..., 3] = 255
    png(sources / "repeat.png", checker)
    distinct = []
    for i in range(39):
        hue = np.array([(37 * i) % 256, (91 * i + 40) % 256, (53 * i + 120) % 256])
        tile = np.zeros((th, tw, 4), np.uint8)
        tile[..., :3] = np.clip(hue[None, None, :] * 0.6 + (xx + yy)[..., None] * 0.25, 0, 255)
        tile[..., 3] = 255
        tile[:20, :, 3] = 0                                 # transparent strip reveals the repeated layer
        circle = (xx - 120) ** 2 + (yy - 108) ** 2 < 50 ** 2
        tile[circle, 3] = 96                               # semi-transparent disc
        tile[150:170, 10:10 + 5 * (i + 1), :3] = 250       # per-tile identity bar
        png(sources / f"tile-{i:02d}.png", tile)
        distinct.append(sources / f"tile-{i:02d}.png")
    animated = []
    for j, color in enumerate([(240, 80, 60), (60, 220, 120), (70, 110, 250)]):
        a = np.zeros((180, 200, 4), np.uint8)              # trimmed: placed with an offset inside the tile
        a[..., :3] = color
        a[..., 3] = 200
        a[60:120, 50:150, 3] = 255
        png(sources / f"anim-{j}.png", a)
        animated.append(sources / f"anim-{j}.png")
    sprite = np.zeros((30, 40, 4), np.uint8)
    sy, sx = np.mgrid[0:30, 0:40]
    sprite[..., 0] = 40 + sx * 5
    sprite[..., 1] = 200 - sy * 4
    sprite[..., 2] = 90 + (sx * sy) % 120
    sprite[..., 3] = np.where((sx - 20) ** 2 / 400 + (sy - 15) ** 2 / 225 < 1, 255, np.where(sx < 6, 128, 0))
    png(sources / "sprite.png", sprite)
    second = sprite.copy()
    second[..., :3] = 255 - second[..., :3]
    png(sources / "sprite-b.png", second)
    green = np.zeros((30, 30, 4), np.uint8)
    green[...] = (0, 255, 0, 255)
    green[8:22, 8:22] = (230, 90, 140, 255)
    green[12:18, 12:18] = (40, 200, 60, 255)                # a greenish detail the key must treat by distance
    png(sources / "green.png", green)
    pre = np.zeros((24, 24, 4), np.uint8)
    alpha = (np.mgrid[0:24, 0:24][1] * 10 + 15).clip(0, 255).astype(np.uint8)
    pre[..., 3] = alpha
    for c, v in enumerate((250, 120, 30)):
        pre[..., c] = (v * alpha.astype(np.int64) // 255).astype(np.uint8)  # valid premultiplied payload
    png(sources / "premultiplied.png", pre)
    make_font(sources / "primary.ttf", PRIMARY)
    with wave.open(str(sources / "tone.wav"), "wb") as w:
        n = np.arange(48000 * 10)
        left = (np.sin(2 * np.pi * 440 * n / 48000) * 9000).astype("<i2")
        right = (((n * 7) % 2000) * 8 - 8000).astype("<i2")
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(48000)
        w.writeframes(np.stack([left, right], 1).tobytes())
    return distinct, animated


# ---------------------------------------------------------------- independent oracle
def tile_selected(tile, layer, n):
    t = F(n, 25) - F(layer["start"]["num"], layer["start"]["den"])
    if not 0 <= t < F(layer["duration"]["num"], layer["duration"]["den"]):
        return None
    return selected({**layer, "start": time(0), "duration": time(10 ** 6), "frames": tile["frames"], "end": tile["end"]}, int(t * 25))


GRAPHICS = {}


def source_rgba(layer, n, sources, fonts):
    if "graphics" in layer and layer["graphics"]:
        g = layer["graphics"]
        key = json.dumps([g, layer["canvas"]], sort_keys=True)
        if key not in GRAPHICS:  # static per layer; the analytic oracle is exact but slow
            image = shape_pixels(g, layer["canvas"]) if g["kind"] == "shape" else text_pixels(g, layer["canvas"], fonts)[0]
            GRAPHICS[key] = np.asarray(image)
        return GRAPHICS[key], (0, 0)
    canvas = Image.new("RGBA", tuple(layer["canvas"]))
    if "tilemap" in layer and layer["tilemap"]:
        m = layer["tilemap"]
        tw, th = m["tile_size"]
        for r, row in enumerate(m["cells"]):
            for c, cell in enumerate(row):
                if cell is None:
                    continue
                tile = m["tiles"][cell]
                index = tile_selected(tile, layer, n)
                if index is None:
                    continue
                f = tile["frames"][index]
                with Image.open(sources / f["image"]["path"]) as image:
                    canvas.paste(image.convert("RGBA"), (c * tw + f["offset"][0], r * th + f["offset"][1]))
        return np.asarray(canvas), (0, 0)
    index = selected(layer, n)
    if index is None:
        return None, None
    f = layer["frames"][index]
    with Image.open(sources / f["image"]["path"]) as image:
        canvas.paste(image.convert("RGBA"), tuple(f["offset"]))
    return np.asarray(canvas), tuple(f["anchor"])


def blend(dest, src, opacity, mode, premultiplied):
    """The documented integer compositing equations, evaluated over whole arrays."""
    d = dest.astype(np.int64)
    s = src[..., :3].astype(np.int64)
    a = src[..., 3:4].astype(np.int64)
    o = int(opacity)
    unit = 65536
    coverage = a * o * unit
    weighted = (s * 255 * o if premultiplied else s * a * o) * unit
    if mode == "normal":
        num, den = weighted + d * (65025 * unit - coverage), 65025 * unit
    elif mode == "multiply":
        num, den = d * weighted + 255 * d * (65025 * unit - coverage), 16581375 * unit
    else:
        num, den = d * 16581375 * unit + (255 - d) * weighted, 16581375 * unit
    return ((num + den // 2) // den).astype(np.uint8)


def expected_frame(scene, n, sources, fonts):
    scene = animated_scene_at(scene, n)
    out = np.empty((scene["height"], scene["width"], 3), np.uint8)
    out[...] = scene["background"]
    for l in scene["layers"]:
        if not (F(l["start"]["num"], l["start"]["den"]) <= F(n, 25) < F(l["start"]["num"], l["start"]["den"]) + F(l["duration"]["num"], l["duration"]["den"])):
            continue
        rgba, anchor = source_rgba(l, n, sources, fonts)
        if rgba is None:
            continue
        rgba = rgba.copy()
        if l.get("mask"):
            x, y, w, h = l["mask"]["rect"]
            inside = np.zeros(rgba.shape[:2], bool)
            inside[max(0, y):max(0, y + h), max(0, x):max(0, x + w)] = True
            if l["mask"].get("inverted"):
                inside = ~inside
            rgba[~inside] = 0
        tr = l["transform"]
        x, y, w, h = tr["crop"]
        img = rgba[y:y + h, x:x + w]
        ax, ay = anchor[0] - x, anchor[1] - y
        for _ in range(tr["quarter_turns"]):
            ax, ay = img.shape[0] - ay, ax
            img = np.rot90(img, k=-1)
        s = tr["scale"]
        img = img.repeat(s, 0).repeat(s, 1)
        left, top = tr["position"][0] - ax * s, tr["position"][1] - ay * s
        x0, y0 = max(0, left), max(0, top)
        x1, y1 = min(scene["width"], left + img.shape[1]), min(scene["height"], top + img.shape[0])
        if x0 >= x1 or y0 >= y1:
            continue
        region = img[y0 - top:y1 - top, x0 - left:x1 - left]
        out[y0:y1, x0:x1] = blend(out[y0:y1, x0:x1], region, tr["opacity"], l.get("blend_mode", "normal"),
                                  l.get("alpha_mode") == "premultiplied")
    return out.tobytes()


# ---------------------------------------------------------------- scenes
def native_scene(sources, distinct, animated, frames_total=225):
    repeat = {"tile_size": list(TILE), "tiles": [{"frames": [frame(sources / "repeat.png", sources)], "timing": "strict", "end": "hold_last"}],
              "cells": [[0] * 8 for _ in range(5)]}
    tiles = [{"frames": [frame(a, sources, hold=time(h, 25), offset=(20, 18)) for a, h in zip(animated, (1, 2, 3))],
              "timing": "strict", "end": "loop"}]
    tiles += [{"frames": [frame(p, sources)], "timing": "strict", "end": "hold_last"} for p in distinct]
    cells = [[r * 8 + c for c in range(8)] for r in range(5)]
    cells[2][3] = None                                     # empty cell shows the repeated layer through
    mosaic = {"tile_size": list(TILE), "tiles": tiles, "cells": cells}
    font = identity(sources / "primary.ttf", sources)
    text = {"kind": "text", "text": "AB CA", "fonts": [font], "size": 300, "color": [250, 240, 200, 255],
            "rect": [60, 700, 1800, 360], "line_height": 360, "letter_spacing": 0, "align": "left", "wrap": "none", "overflow": "reject"}
    layers = [
        layer("repeat", (W, H), [], frames_total=frames_total, timing="strict", end="hold_last", tilemap=repeat),
        layer("mosaic", (W, H), [], opacity=230, frames_total=frames_total, end="hold_last", tilemap=mosaic),
        layer("sprite6", (40, 30), [frame(sources / "sprite.png", sources, anchor=(20, 15)),
                                    frame(sources / "sprite-b.png", sources, hold=time(2, 25), anchor=(20, 15))],
              position=(200, 600), scale=6, frames_total=frames_total,
              animation={"position_x": keys((0, 200), ((frames_total, 25), 1500)), "position_y": keys((0, 600), ((frames_total, 25), 700))}),
        layer("sprite4", (40, 30), [frame(sources / "sprite.png", sources, anchor=(4, 2))], position=(960, 300),
              crop=[4, 2, 30, 24], scale=4, turns=1, opacity=200, frames_total=frames_total, blend_mode="multiply"),
        layer("sprite1", (40, 30), [frame(sources / "sprite.png", sources)], position=(100, 100), frames_total=frames_total,
              blend_mode="screen", mask={"rect": [5, 5, 20, 15], "inverted": False}),
        layer("title", (W, H), [], frames_total=frames_total, end="hold_last", graphics=text,
              animation={"opacity": keys((0, 0), ((min(50, frames_total), 25), 255))}),
    ]
    mix = {"schema_version": 1, "id": "tone", "duration": time(frames_total, 25),
           "tracks": [{"id": "t", "clips": [{"id": "c", "file": identity(sources / "tone.wav", sources), "channels": "preserve_stereo",
                                              "start": time(0), "source_in": time(0), "duration": time(frames_total, 25)}]}]}
    return {"schema_version": 1, "id": "native", "width": W, "height": H, "output_scale": 1, "duration": time(frames_total, 25),
            "background": [10, 12, 30], "color": "srgb_straight_encoded", "layers": layers, "audio": None, "audio_mix": mix}


def equivalence_pair(sources, key_effects, frames_total=50):
    """A 480x270 scene enlarged 4x must equal the native scene whose layer scales and positions are 4x larger."""
    grade = {"kind": "grade", "exposure_milli": 300, "contrast_milli": 1150, "white_balance_milli": [1100, 1000, 900],
             "animation": {"exposure_milli": keys((0, -500), (2, 600))}}
    low_layers = [
        layer("graded", (40, 30), [frame(sources / "sprite.png", sources, hold=time(1, 25), anchor=(20, 15)),
                                   frame(sources / "sprite-b.png", sources, hold=time(1, 25), anchor=(20, 15))],
              position=(100, 60), crop=[2, 1, 36, 28], scale=2, turns=2, frames_total=frames_total, effects=[grade],
              animation={"opacity": keys((0, 120), (2, 255))}),
        layer("keyed", (30, 30), [frame(sources / "green.png", sources, anchor=(15, 15))], position=(300, 150), scale=3,
              frames_total=frames_total, effects=key_effects),
        layer("premultiplied", (24, 24), [frame(sources / "premultiplied.png", sources)], position=(30, 180),
              frames_total=frames_total, alpha_mode="premultiplied", blend_mode="screen"),
        layer("feathered", (40, 30), [frame(sources / "sprite.png", sources, anchor=(0, 0))], position=(380, 20), scale=4, turns=3,
              frames_total=frames_total, blend_mode="multiply",
              mask={"rect": [6, 4, 24, 18], "inverted": False, "feather": {"radius": 3, "edge": "centered"}}),
        layer("badge", (60, 40), [], position=(20, 20), scale=2, frames_total=frames_total, end="hold_last",
              graphics={"kind": "shape", "shape": "ellipse", "rect": [4, 4, 52, 32], "fill": [40, 160, 220, 180],
                        "stroke": {"width": 3, "color": [240, 240, 240, 255]}}),
    ]
    low = {"schema_version": 1, "id": "low", "width": 480, "height": 270, "output_scale": 4, "duration": time(frames_total, 25),
           "background": [70, 40, 90], "color": "srgb_straight_encoded", "layers": low_layers, "audio": None}
    high = copy.deepcopy(low)
    high.update(id="high", width=1920, height=1080, output_scale=1)
    for l in high["layers"]:
        l["transform"]["scale"] *= 4
        l["transform"]["position"] = [v * 4 for v in l["transform"]["position"]]
    return low, high


class Watch:
    """Sample the engine's peak working set and the largest scene scratch directory while it runs."""
    def __init__(self, directory):
        self.directory, self.peak_rss, self.peak_scratch, self.stop = directory, 0, 0, False

    def run(self, request):
        child = subprocess.Popen([str(EXE)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, encoding="utf-8")
        process = psutil.Process(child.pid)
        thread = threading.Thread(target=self.poll, args=(process,), daemon=True)
        thread.start()
        started = clock.perf_counter()
        out, _ = child.communicate(json.dumps(request), timeout=3600)
        elapsed = clock.perf_counter() - started
        self.stop = True
        thread.join()
        result = json.loads(out)
        assert result["ok"], result
        return result["result"], elapsed

    def poll(self, process):
        while not self.stop:
            try:
                tree = [process] + process.children(recursive=True)
                self.peak_rss = max(self.peak_rss, sum(p.memory_info().rss for p in tree))
            except psutil.Error:
                pass
            scratch = sum(f.stat().st_size for d in self.directory.glob(".cutbolt-scene-*") for f in d.glob("*") if f.is_file())
            self.peak_scratch = max(self.peak_scratch, scratch)
            clock.sleep(0.1)


def run(root):
    root.mkdir(parents=True, exist_ok=False)
    sources, output = root / "sources", root / "output"
    sources.mkdir(); output.mkdir()
    distinct, animated = generate(sources)
    fonts = {"primary.ttf": PRIMARY}
    passed, measurements = [], {}
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    caps = call({"command": "capabilities"})["scenes"]["limits"]
    assert caps["canvas_per_axis"] == [1, 4096] and caps["maximum_output_pixels"] == 8_000_000 and caps["text_size"] == [1, 512]
    assert caps["tilemap"]["maximum_tiles"] == 256 and caps["tilemap"]["maximum_cells"] == 4096
    passed.append("native.capability_limits")

    # 1. Native 1920x1080 scene, every decoded frame and sample against the independent oracle.
    scene = native_scene(sources, distinct, animated)
    (root / "native-scene.json").write_text(json.dumps(scene, indent=1), encoding="utf-8")
    inspection = call({"command": "scene.inspect", "scene": scene, "input_root": str(sources)})
    mosaic = next(l for l in inspection["timing"] if l["layer_id"] == "mosaic")["tilemap"]
    assert mosaic["grid"] == [8, 5] and mosaic["tiles"] == 40 and mosaic["occupied_cells"] == 39
    assert mosaic["tile_selected_frames"][0][:7] == [0, 1, 1, 2, 2, 2, 0]
    watch = Watch(output)
    result, seconds = watch.run({"command": "scene.render", "scene": scene, "input_root": str(sources), "output_root": str(output),
                                 "output": str(output / "native.mkv")})
    out_bytes = (output / "native.mkv").stat().st_size
    measurements["native_1080p"] = {"frames": 225, "layers": 6, "tile_pieces": 79, "seconds": round(seconds, 3),
                                    "peak_process_tree_bytes": watch.peak_rss, "peak_scratch_bytes": watch.peak_scratch,
                                    "output_bytes": out_bytes}
    assert result["width"] == W and result["height"] == H and result["frames"] == 225
    # Scratch holds only the encoder's own output and the PCM soundtrack, never a raw video copy.
    assert watch.peak_scratch <= out_bytes + 225 * 1920 * 4 + 16 * 1024 * 1024, watch.peak_scratch
    decode_check(output / "native.mkv", W, H, lambda n: expected_frame(scene, n, sources, fonts), 225)
    passed.append("native.every_1080p_frame_matches_oracle")
    passed.append("native.tilemap_repeat_distinct_empty_animated_trimmed")
    with wave.open(str(sources / "tone.wav")) as w:
        tone = w.readframes(225 * 1920)
    assert audio_bytes(output / "native.mkv") == tone
    passed.append("native.audio_mix_exact")

    # 2. Scale equivalence with effects, keys, masks, premultiplied alpha and generated shapes.
    key = call({"command": "effects.preset", "name": "green_soft", "strength_milli": 1000, "spill_milli": 650})["effects"]
    low, high = equivalence_pair(sources, key)
    timings = {}
    for s in (low, high):
        (root / f"{s['id']}.json").write_text(json.dumps(s, indent=1), encoding="utf-8")
        w = Watch(output)
        _, timings[s["id"]] = w.run({"command": "scene.render", "scene": s, "input_root": str(sources), "output_root": str(output),
                                     "output": str(output / f"{s['id']}.mkv")})
    decoded = [subprocess.run(["ffmpeg", "-v", "error", "-i", str(output / f"{n}.mkv"), "-an", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                              capture_output=True, check=True).stdout for n in ("low", "high")]
    assert len(decoded[0]) == 50 * W * H * 3 and decoded[0] == decoded[1], "native render differs from 4x enlargement"
    measurements["equivalence"] = {"frames": 50, "low_480x270x4_seconds": round(timings["low"], 3),
                                   "native_1920x1080_seconds": round(timings["high"], 3),
                                   "ratio": round(timings["high"] / timings["low"], 3)}
    assert timings["high"] <= 3 * timings["low"] + 10, measurements["equivalence"]
    passed.append("native.scale_equivalence_effects_masks_alpha")
    passed.append("native.time_within_three_times_enlarged")

    # 3. Captions at native size, with text above the former 128-pixel limit.
    font = identity(sources / "primary.ttf", sources)
    base = {"schema_version": 1, "id": "caption-base", "width": W, "height": H, "output_scale": 1, "duration": time(1),
            "background": [20, 20, 24], "color": "srgb_straight_encoded", "audio": None,
            "layers": [layer("bg", (W, H), [], frames_total=25, end="hold_last",
                             graphics={"kind": "shape", "shape": "rectangle", "rect": [0, 760, W, 320], "fill": [0, 0, 0, 160], "stroke": None})]}
    document = {"schema_version": 1, "id": "native", "revision": 0, "overlap": "reject", "styles": {"big": {"color": [255, 220, 120]}},
                "cues": [{"id": "one", "start": time(0), "end": time(1), "text": "ABBA", "style": "big", "align": "center", "speaker": None}]}
    layout = {"fonts": [font], "size": 200, "rect": [100, 800, 1720, 260], "line_height": 240, "letter_spacing": 0, "wrap": "none", "overflow": "reject"}
    captioned = call({"command": "captions.scene", "document": document, "scene": base, "scene_id": "native-captions", "offset": time(0),
                      "layouts": {"big": layout}, "sampling": "sample_start", "layer_prefix": "cap", "input_root": str(sources)})["scene"]
    call({"command": "scene.render", "scene": captioned, "input_root": str(sources), "output_root": str(output), "output": str(output / "captions.mkv")})
    decode_check(output / "captions.mkv", W, H, lambda n: expected_frame(captioned, n, sources, fonts), 25)
    passed.append("native.captions_large_text_exact")

    # 4. Downstream: saved editing, preview and delivery use the native asset unchanged.
    project = call({"command": "project.create", "id": "native-edit", "width": W, "height": H, "frame_rate": time(25)})
    project = call({"command": "timeline.apply", "project": project, "expected_revision": 0, "operations": [
        {"op": "media.add", "asset": result["asset"]},
        {"op": "clip.append", "clip": {"id": "c", "asset_id": "native", "source_in": time(2), "duration": time(3)}}]})
    call({"command": "preview.frame", "project": project, "input_root": str(output), "output_root": str(output),
          "output": str(output / "preview.png"), "time": time(1)})
    with Image.open(output / "preview.png") as preview:
        assert preview.convert("RGB").tobytes() == expected_frame(scene, 75, sources, fonts)
    exported = call({"command": "export.run", "project": project, "input_root": str(output), "output_root": str(output),
                     "output": str(output / "delivery.mp4"), "profile": "h264_aac", "streams": "audio_video", "input_transfer": "srgb",
                     "range": {"start": time(0), "duration": time(1)}})
    assert exported["video_frames"] == 25 and exported["width"] == W
    passed.append("native.downstream_preview_and_delivery")

    # 5. Limits and actionable errors; nothing is published for rejected requests.
    def reject(name, mutate, code, contains=None, base_scene=None):
        s = copy.deepcopy(base_scene or native_scene(sources, distinct, animated, 25))
        mutate(s)
        error = call({"command": "scene.render", "scene": s, "input_root": str(sources), "output_root": str(output),
                      "output": str(output / f"reject-{name}.mkv")}, code)
        assert not (output / f"reject-{name}.mkv").exists()
        for part in ([contains] if isinstance(contains, str) else contains or []):
            assert part in error["message"], (name, error["message"])
        return error
    reject("canvas", lambda s: s.update(width=4097), "INVALID_SCENE")
    reject("output-pixels", lambda s: s.update(width=3840, height=2160), "INVALID_SCENE", "8M output pixels")
    reject("scaled-pixels", lambda s: s.update(width=2048, height=1080, output_scale=2), "INVALID_SCENE")
    reject("text-size", lambda s: s["layers"][5]["graphics"].update(size=513), "INVALID_GRAPHIC", "size 1-512")
    reject("tile-canvas", lambda s: s["layers"][0].update(canvas=[1920, 1000]), "INVALID_SCENE", "tile_size x grid")
    reject("tile-index", lambda s: s["layers"][1]["tilemap"]["cells"][0].__setitem__(0, 40), "INVALID_SCENE", "cells[0][0]")
    reject("tile-ragged", lambda s: s["layers"][1]["tilemap"]["cells"][4].pop(), "INVALID_SCENE")
    reject("tile-count", lambda s: s["layers"][1]["tilemap"]["tiles"].extend(s["layers"][1]["tilemap"]["tiles"][1:2] * 217), "INVALID_SCENE")
    reject("tile-cells", lambda s: s["layers"][0].update(canvas=[65, 64], transform={**s["layers"][0]["transform"], "crop": [0, 0, 65, 64]},
                                                         tilemap={**s["layers"][0]["tilemap"], "tile_size": [1, 1], "cells": [[0] * 65 for _ in range(64)]}),
           "INVALID_SCENE")
    reject("tile-identity", lambda s: s["layers"][1]["tilemap"]["tiles"][0]["frames"][0]["image"].update(sha256="0" * 64), "MEDIA_CHANGED")
    reject("tile-empty-frames", lambda s: s["layers"][1]["tilemap"]["tiles"][0].update(frames=[]), "INVALID_SCENE")
    reject("tile-too-big", lambda s: s["layers"][1]["tilemap"]["tiles"][0]["frames"][0].update(offset=[60, 60]), "INVALID_SCENE", "tile size")
    reject("two-sources", lambda s: s["layers"][0].update(frames=[frame(sources / "repeat.png", sources)]), "INVALID_SCENE", "exactly one")
    reject("tile-layer-timing", lambda s: s["layers"][0].update(timing="sample_start"), "INVALID_SCENE")
    reject("tile-hold", lambda s: s["layers"][1]["tilemap"]["tiles"][0]["frames"][0].update(hold=time(1, 10)), "UNALIGNED_TIME",
           "tilemap.tiles[0].frames[0].hold")
    reject("layer-hold", lambda s: s["layers"][2]["frames"][0].update(hold=time(1, 10)), "UNALIGNED_TIME",
           ["layers[2] (sprite6).frames[0].hold (strict timing)", "1/10 s is 5/2 units at 25/1 per second"])
    large = sources / "large.png"
    png(large, np.zeros((10, 4097, 4), np.uint8))
    reject("large-png", lambda s: s["layers"][4]["frames"][0].update(image=identity(large, sources)), "UNSUPPORTED_IMAGE")
    error = call({"command": "preview.frame", "project": project, "input_root": str(output), "output_root": str(output),
                  "output": str(output / "bad.png"), "time": time(1, 10)}, "UNALIGNED_TIME")
    assert error["message"].startswith("time:"), error
    srt = call({"command": "captions.encode", "format": "srt",
                "document": {**document, "cues": [{**document["cues"][0], "id": "amp", "text": "A & B"}]}}, "UNSUPPORTED_CAPTIONS")
    assert '"amp"' in srt["message"], srt
    passed.append("native.limits_rejected_without_output")
    passed.append("native.errors_name_fields_and_cues")

    # 6. Spatial rotation at native size: bilinear and nearest sampling, flip and crop, against the
    # independent spatial oracle (imported here because it sets its own 60-digit Decimal context).
    from spatial import reference as spatial_reference

    def spatial(**changes):
        value = {"translate_milli": [250, -500], "scale_milli": [1000, 1000], "rotation_mdeg": 0, "flip": [False, False],
                 "pixel_aspect": time(1), "sampling": "bilinear", "edge": "transparent"}
        value.update(changes)
        return value

    rotated = layer("rotated", (40, 30), [frame(sources / "sprite.png", sources, hold=time(2, 25), anchor=(20, 15))], position=(700, 500),
                    scale=6, opacity=230, frames_total=2)
    rotated["transform"]["spatial"] = spatial(rotation_mdeg=30000)
    tilted = layer("tilted", (40, 30), [frame(sources / "sprite.png", sources, hold=time(2, 25), anchor=(4, 2))], position=(1400, 700),
                   crop=[4, 2, 30, 24], scale=3, frames_total=2)
    tilted["transform"]["spatial"] = spatial(rotation_mdeg=-12500, scale_milli=[1500, 1250], sampling="nearest", flip=[True, False])
    rotation = {"schema_version": 1, "id": "native-spatial", "width": W, "height": H, "output_scale": 1, "duration": time(2, 25),
                "background": [10, 12, 30], "color": "srgb_straight_encoded", "layers": [rotated, tilted], "audio": None}
    started = clock.perf_counter()
    call({"command": "scene.render", "scene": rotation, "input_root": str(sources), "output_root": str(output), "output": str(output / "spatial.mkv")})
    measurements["native_spatial_rotation"] = {"frames": 2, "seconds": round(clock.perf_counter() - started, 3)}
    still = spatial_reference(rotation, sources, 0)
    assert int(np.any(np.frombuffer(still, np.uint8).reshape(H, W, 3) != [10, 12, 30], axis=2).sum()) > 40000
    decode_check(output / "spatial.mkv", W, H, lambda n: still, 2)
    passed.append("native.spatial_rotation_exact")
    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir() if p.name in originals} == originals
    passed.append("native.sources_preserved")
    report = {"passed": passed, "measurements": measurements, "frames_compared": 225 + 50 + 25 + 1 + 2,
              "oracle": "vectorised documented integer blend equations, Pillow tile assembly, analytic text coverage, Fraction keyframes"}
    (root / "verification.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    run(parser.parse_args().output)
