"""Alpha overlay tracks: straight-alpha clips composited over lower video on the native timeline.

Independent oracle: sources are decoded separately with FFmpeg and combined with the documented integer
equation round((s*a + d*(255-a)) / 255); every decoded frame of full renders, previews and range exports
is compared exactly.
"""
from engine import ENGINE
import argparse
import copy
import hashlib
import json
from fractions import Fraction as F
from pathlib import Path
import subprocess

import numpy as np
from PIL import Image

from scenes import identity, time

ROOT = Path(__file__).resolve().parents[1]
EXE = ENGINE
W, H = 320, 180


def call(command, error=None, **fields):
    result = json.loads(subprocess.run([str(EXE)], input=json.dumps({"command": command, **fields}), text=True,
                                       encoding="utf-8", capture_output=True, timeout=1800).stdout)
    if error:
        assert not result["ok"] and result["error"]["code"] == error, (error, result)
        return result["error"]
    assert result["ok"], result
    return result["result"]


def decode(path, pix_fmt, channels):
    raw = subprocess.run(["ffmpeg", "-v", "error", "-i", str(path), "-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", pix_fmt, "-"],
                         capture_output=True, check=True).stdout
    return np.frombuffer(raw, np.uint8).reshape(-1, H, W, channels)


def over(dest, src):
    d = dest.astype(np.int64)
    s = src[..., :3].astype(np.int64)
    a = src[..., 3:4].astype(np.int64)
    return ((2 * (s * a + d * (255 - a)) + 255) // 510).astype(np.uint8)


def opaque_scene(sources, name, colour, seconds):
    frames = int(seconds * 25)
    yy, xx = np.mgrid[0:H, 0:W]
    background = np.zeros((H, W, 4), np.uint8)
    background[..., 0] = (xx * 255 // W + colour[0]) % 256
    background[..., 1] = (yy * 255 // H + colour[1]) % 256
    background[..., 2] = colour[2]
    background[..., 3] = 255
    path = sources / f"{name}.png"
    Image.fromarray(background, "RGBA").save(path)
    marker = np.zeros((12, 12, 4), np.uint8)
    marker[...] = (255, 255, 255, 255)
    marker_path = sources / f"{name}-marker.png"
    Image.fromarray(marker, "RGBA").save(marker_path)
    layer = lambda id, p, canvas, position, animation=None: {
        "id": id, "canvas": list(canvas), "start": time(0), "duration": time(frames, 25),
        "frames": [{"image": identity(p, sources), "hold": time(1, 25), "offset": [0, 0], "anchor": [0, 0]}],
        "timing": "strict", "end": "loop",
        "transform": {"position": list(position), "crop": [0, 0, *canvas], "scale": 1, "quarter_turns": 0, "opacity": 255},
        **({"animation": animation} if animation else {})}
    scene = {"schema_version": 1, "id": name, "width": W, "height": H, "output_scale": 1, "duration": time(frames, 25),
             "background": [0, 0, 0], "color": "srgb_straight_encoded", "audio": None,
             "layers": [layer("bg", path, (W, H), (0, 0)),
                        layer("marker", marker_path, (12, 12), (0, 80),
                              {"position_x": {"keys": [{"time": time(0), "value": 0, "interpolation": "linear"},
                                                       {"time": time(frames, 25), "value": W - 12, "interpolation": "hold"}]}})]}
    return scene


def alpha_sequence(sources, name, count, hue):
    entries, frames = [], []
    yy, xx = np.mgrid[0:H, 0:W]
    for n in range(count):
        rgba = np.zeros((H, W, 4), np.uint8)
        rgba[..., 0] = (xx * 3 + n * 40 + hue) % 256
        rgba[..., 1] = (yy * 5 + hue) % 256
        rgba[..., 2] = (xx + yy + n * 17) % 256
        # Every alpha level class: transparent, faint, half, strong, opaque bands.
        rgba[..., 3] = np.take(np.array([0, 1, 64, 127, 128, 200, 254, 255], np.uint8), (xx // 9 + yy // 7 + n) % 8)
        rgba[60:100, 100 + n * 8:180 + n * 8, 3] = 255     # a moving opaque box (lower-third style)
        path = sources / f"{name}-{n:06}.png"
        Image.fromarray(rgba, "RGBA").save(path)
        entries.append({"number": n, "image": identity(path, sources)})
        frames.append(rgba)
    recipe = {"schema_version": 1, "id": name, "first_number": 0, "frames": entries, "frame_rate": time(25), "source_start": 0,
              "source_count": count, "repeat": 1, "input_transfer": "srgb", "alpha_mode": "straight", "profile": "rgba_ffv1"}
    return recipe


def clip(id, asset, start, source_in, duration):
    return {"id": id, "asset_id": asset, "start": time(*start), "source_in": time(*source_in), "duration": time(*duration)}


def place(track, c):
    return {"op": "tracks.edit", "edit": {"op": "place", "track_id": track, "clip": c, "collision": "reject"}}


def track(id, composite=None, kind="video"):
    t = {"id": id, "kind": kind, "locked": False, "enabled": True, "clips": []}
    if composite:
        t["composite"] = composite
    return {"op": "tracks.edit", "edit": {"op": "add", "track": t}}


def run(root):
    root.mkdir(parents=True, exist_ok=False)
    sources, output = root / "sources", root / "output"
    sources.mkdir(); output.mkdir()
    passed = []
    # Opaque base material and two straight-alpha overlays, all original synthetic sources.
    assets = {}
    for name, colour in (("base-a", (0, 0, 60)), ("base-b", (90, 40, 200)), ("cover", (30, 160, 20))):
        receipt = call("scene.render", scene=opaque_scene(sources, name, colour, 3), input_root=str(sources),
                       output_root=str(sources), output=str(sources / f"{name}.mkv"))
        assets[name] = receipt["asset"]
    for name, hue in (("title", 10), ("bug", 130)):
        recipe = alpha_sequence(sources, name, 75, hue)
        receipt = call("image.sequence.compile", recipe=recipe, input_root=str(sources), output_root=str(sources),
                       output=str(sources / f"{name}.mkv"))
        assets[name] = {"id": name, "path": str(sources / f"{name}.mkv"), "duration": time(3)}
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    decoded = {n: decode(sources / f"{n}.mkv", "rgb24", 3) for n in ("base-a", "base-b", "cover")}
    decoded.update({n: decode(sources / f"{n}.mkv", "rgba", 4) for n in ("title", "bug")})
    assert all(decoded[n].shape[0] == 75 for n in decoded)

    # Timeline (seconds): V1 opaque base-a [0,2.6) gap [2.6,3) base-b [3,6); V2 alpha title [1,3) + [4,5);
    # V3 alpha bug [2,4.4); V4 opaque cover [5,5.4) hides lower overlays; V5 alpha bug [5.2,5.6) over the cover.
    project = call("project.create", id="overlays", width=W, height=H, frame_rate=time(25))
    store = root / "store"; store.mkdir()
    call("session.create", store_root=str(store), project=project, request_id="create")
    ops = [{"op": "media.add", "asset": a} for a in assets.values()]
    ops += [{"op": "tracks.edit", "edit": {"op": "create", "duration": time(6)}},
            track("V1"), track("V2", "alpha_over"), track("V3", "alpha_over"), track("V4"), track("V5", "alpha_over"),
            place("V1", clip("a", "base-a", (0,), (0,), (13, 5))), place("V1", clip("b", "base-b", (3,), (0,), (3,))),
            place("V2", clip("t1", "title", (1,), (0,), (2,))), place("V2", clip("t2", "title", (4,), (1,), (1,))),
            place("V3", clip("g1", "bug", (2,), (10, 25), (12, 5))),
            place("V4", clip("c", "cover", (5,), (0,), (2, 5))),
            place("V5", clip("g2", "bug", (26, 5), (0,), (2, 5)))]
    call("session.apply", store_root=str(store), project_id="overlays", request_id="assemble", expected_revision=0, operations=ops)
    project = call("session.get", store_root=str(store), project_id="overlays")
    (root / "project.json").write_text(json.dumps(project, indent=1), encoding="utf-8")
    passed.append("overlays.saved_session_alpha_over_tracks")

    tracks = {t["id"]: t for t in project["tracks"]["tracks"]}

    def active(t, n):
        for c in tracks[t]["clips"]:
            start, dur, src = (F(c[k]["num"], c[k]["den"]) for k in ("start", "duration", "source_in"))
            if start <= F(n, 25) < start + dur:
                return c["asset_id"], int((src + F(n, 25) - start) * 25)
        return None

    def expected(n):
        frame = np.zeros((H, W, 3), np.uint8)
        base_index = None
        for i, t in enumerate(("V1", "V2", "V3", "V4", "V5")):
            if tracks[t].get("composite", "opaque") == "opaque" and active(t, n):
                base_index = i
        if base_index is not None:
            asset, index = active(("V1", "V2", "V3", "V4", "V5")[base_index], n)
            frame = decoded[asset][index].copy()
        for i, t in enumerate(("V1", "V2", "V3", "V4", "V5")):
            if tracks[t].get("composite") == "alpha_over" and (base_index is None or i > base_index) and active(t, n):
                asset, index = active(t, n)
                frame = over(frame, decoded[asset][index])
        return frame

    rendered = call("render.run", project=project, input_root=str(sources), output_root=str(output), output=str(output / "render.mkv"))
    frames = decode(output / "render.mkv", "rgb24", 3)
    assert frames.shape[0] == 150 == rendered["frames"]
    for n in range(150):
        assert np.array_equal(frames[n], expected(n)), f"frame {n} differs"
    passed.append("overlays.every_frame_exact_stacking_gap_and_cover")

    for t in (F(1), F(52, 25), F(67, 25), F(103, 25), F(131, 25), F(5)):
        receipt = call("preview.frame", project=project, input_root=str(sources), output_root=str(output),
                       output=str(output / f"preview-{t.numerator}-{t.denominator}.png"), time=time(t.numerator, t.denominator))
        with Image.open(output / f"preview-{t.numerator}-{t.denominator}.png") as image:
            assert np.array_equal(np.asarray(image.convert("RGB")), expected(int(t * 25)))
    passed.append("overlays.frame_previews_match")

    call("export.run", project=project, input_root=str(sources), output_root=str(output), output=str(output / "range.mkv"),
         profile="reference", streams="video", range={"start": time(48, 25), "duration": time(60, 25)})
    ranged = decode(output / "range.mkv", "rgb24", 3)
    assert ranged.shape[0] == 60 and all(np.array_equal(ranged[i], expected(48 + i)) for i in range(60))
    scope = call("scopes.inspect", project=project, input_root=str(sources), time=time(2), input_transfer="srgb",
                 missing_tags="use_declared", columns=8)
    assert scope["rgb_sha256"] == hashlib.sha256(expected(50).tobytes()).hexdigest()
    passed.append("overlays.range_export_and_scopes")

    proxied = copy.deepcopy(project); proxied["preview_scale"] = 2
    call("preview.frame", "UNSUPPORTED_PREVIEW", project=proxied, input_root=str(sources), output_root=str(output),
         output=str(output / "proxy.png"), time=time(1))

    def reject(name, mutate, code):
        p = copy.deepcopy(project)
        mutate(p)
        call("render.run", code, project=p, input_root=str(sources), output_root=str(output), output=str(output / f"bad-{name}.mkv"))
        assert not (output / f"bad-{name}.mkv").exists()
    ids = {t["id"]: i for i, t in enumerate(project["tracks"]["tracks"])}
    reject("transition", lambda p: p["tracks"]["tracks"][ids["V2"]].update(transitions=[{"id": "x", "left_id": "t1", "right_id": "t2", "before": time(0), "after": time(1, 25), "kind": "dissolve"}]), "UNSUPPORTED_TIMELINE")
    def alpha_only_on_opaque(p):
        for c in p["tracks"]["tracks"][ids["V2"]]["clips"]:
            c["asset_id"] = "bug"
        p["tracks"]["tracks"][ids["V4"]]["clips"][0]["asset_id"] = "title"
    reject("alpha-on-opaque", alpha_only_on_opaque, "UNSUPPORTED_MEDIA")
    reject("alpha-mixed-use", lambda p: p["tracks"]["tracks"][ids["V4"]]["clips"][0].update(asset_id="title"), "UNSUPPORTED_TIMELINE")
    reject("mixed-use", lambda p: p["tracks"]["tracks"][ids["V1"]]["clips"].__setitem__(1, {**p["tracks"]["tracks"][ids["V1"]]["clips"][1], "asset_id": "base-b"}) or p["tracks"]["tracks"][ids["V2"]]["clips"][1].update(asset_id="base-b"), "UNSUPPORTED_TIMELINE")
    audio_overlay = {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "A9", "kind": "audio", "locked": False, "enabled": True, "clips": [], "composite": "alpha_over"}}}
    call("session.apply", "UNSUPPORTED_TIMELINE", store_root=str(store), project_id="overlays", request_id="bad-audio", expected_revision=1, operations=[audio_overlay])
    passed.append("overlays.explicit_rejections_without_output")

    queue = root / "queue"; queue.mkdir()
    ticket = call("render.start", job_root=str(queue), request_id="overlay-render",
                  render={"project": project, "input_root": str(sources), "output_root": str(output), "output": str(output / "queued.mkv")})
    import time as clock
    deadline = clock.time() + 900
    while (status := call("job.status", job_root=str(queue), job_id=ticket["job_id"]))["progress"]["phase"] not in ("completed", "failed", "cancelled", "interrupted"):
        assert clock.time() < deadline, status
        clock.sleep(1)
    assert status["progress"]["phase"] == "completed", status
    assert np.array_equal(decode(output / "queued.mkv", "rgb24", 3), frames)
    passed.append("overlays.queued_render_matches")

    # Picture-in-picture: crop, shrink by the floor of each block's mean, fade and place overlay clips.
    def transformed(src, tf):
        if src.shape[2] == 3:
            src = np.concatenate([src, np.full(src.shape[:2] + (1,), 255, np.uint8)], axis=2)
        x, y, w, h = tf.get("crop", [0, 0, W, H])
        k = tf.get("divisor", 1)
        part = src[y:y + h, x:x + w].astype(np.int64)
        if k > 1:
            part = part.reshape(h // k, k, w // k, k, 4).sum(axis=(1, 3)) // (k * k)
        part[..., 3] = (part[..., 3] * tf.get("opacity", 255) + 127) // 255
        canvas = np.zeros((H, W, 4), np.int64)
        px, py = tf.get("position", [0, 0])
        sh, sw = part.shape[:2]
        x0, y0, x1, y1 = max(px, 0), max(py, 0), min(px + sw, W), min(py + sh, H)
        if x0 < x1 and y0 < y1:
            canvas[y0:y1, x0:x1] = part[y0 - py:y1 - py, x0 - px:x1 - px]
        return canvas.astype(np.uint8)

    def composed(p, n):
        layers = p["tracks"]["tracks"]
        def at(t):
            for c in t["clips"]:
                start, dur, src = (F(c[k]["num"], c[k]["den"]) for k in ("start", "duration", "source_in"))
                if start <= F(n, 25) < start + dur:
                    return c, int((src + F(n, 25) - start) * 25)
            return None
        base_index = max((i for i, t in enumerate(layers) if t["kind"] == "video" and t["enabled"]
                          and t.get("composite", "opaque") == "opaque" and at(t)), default=None)
        frame = np.zeros((H, W, 3), np.uint8)
        if base_index is not None:
            c, index = at(layers[base_index])
            frame = decoded[c["asset_id"]][index].copy()
        for i, t in enumerate(layers):
            if t.get("composite") == "alpha_over" and t["enabled"] and (base_index is None or i > base_index) and at(t):
                c, index = at(t)
                source = decoded[c["asset_id"]][index]
                if "transform" in c:
                    source = transformed(source, c["transform"])
                elif source.shape[2] == 3:
                    source = transformed(source, {})
                frame = over(frame, source)
        return frame

    edit = lambda op, **fields: {"op": "tracks.edit", "edit": {"op": op, **fields}}
    pip_ops = [edit("remove", clip_ids=["c"], links="reject_partial"), track("V6", "alpha_over"),
               place("V6", {**clip("p", "cover", (2, 5), (0,), (11, 5)), "transform": {"crop": [8, 4, 288, 160], "divisor": 4, "position": [230, 128]}}),
               place("V6", clip("q", "cover", (3,), (1, 5), (8, 5))),
               edit("clip_transform", clip_ids=["q"], transform={"divisor": 2, "opacity": 180, "position": [-20, 150]}),
               edit("clip_transform", clip_ids=["t1"], transform={"crop": [40, 20, 200, 120], "opacity": 128, "position": [100, -30]}),
               edit("clip_transform", clip_ids=["g2"], transform={"position": [400, 0]})]
    pip = call("timeline.apply", project=project, expected_revision=project["revision"], operations=pip_ops)
    placed = {c["id"]: c for t in pip["tracks"]["tracks"] for c in t["clips"]}
    assert placed["q"]["transform"] == {"divisor": 2, "opacity": 180, "position": [-20, 150]} and "transform" not in placed["t2"]
    call("render.run", project=pip, input_root=str(sources), output_root=str(output), output=str(output / "pip.mkv"))
    pip_frames = decode(output / "pip.mkv", "rgb24", 3)
    assert pip_frames.shape[0] == 150
    for n in range(150):
        assert np.array_equal(pip_frames[n], composed(pip, n)), f"picture-in-picture frame {n} differs"
    for t in (F(1), F(77, 25), F(133, 25)):
        name = f"pip-{t.numerator}-{t.denominator}.png"
        call("preview.frame", project=pip, input_root=str(sources), output_root=str(output), output=str(output / name),
             time=time(t.numerator, t.denominator))
        with Image.open(output / name) as image:
            assert np.array_equal(np.asarray(image.convert("RGB")), composed(pip, int(t * 25)))
    call("export.run", project=pip, input_root=str(sources), output_root=str(output), output=str(output / "pip-range.mkv"),
         profile="reference", streams="video", range={"start": time(70, 25), "duration": time(20, 25)})
    assert all(np.array_equal(f, composed(pip, 70 + i)) for i, f in enumerate(decode(output / "pip-range.mkv", "rgb24", 3)))
    call("timeline.apply", "INVALID_TRACKS", project=pip, expected_revision=pip["revision"],
         operations=[edit("clip_transform", clip_ids=["a"], transform={"position": [1, 1]})])
    call("timeline.apply", "INVALID_TRACKS", project=pip, expected_revision=pip["revision"],
         operations=[edit("clip_transform", clip_ids=["q"], transform={"divisor": 3})])
    call("timeline.apply", "INVALID_TRACKS", project=pip, expected_revision=pip["revision"],
         operations=[edit("clip_transform", clip_ids=["q"], transform={"divisor": 9, "crop": [0, 0, 288, 162]})])
    shrunk_alpha = call("timeline.apply", project=pip, expected_revision=pip["revision"],
                        operations=[edit("clip_transform", clip_ids=["t2"], transform={"divisor": 2})])
    call("render.run", "UNSUPPORTED_MEDIA", project=shrunk_alpha, input_root=str(sources), output_root=str(output),
         output=str(output / "bad-shrunk-alpha.mkv"))
    assert not (output / "bad-shrunk-alpha.mkv").exists()
    passed.append("overlays.picture_in_picture_transforms_exact")
    undo = call("session.undo", store_root=str(store), project_id="overlays", request_id="undo", expected_revision=1)
    head = call("session.get", store_root=str(store), project_id="overlays")
    assert undo["revision"] == 2 and head.get("tracks") is None and head["assets"] == []
    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir() if p.name in originals} == originals
    passed.append("overlays.sources_preserved")
    report = {"passed": passed, "frames_compared": 150 + 6 + 60 + 1 + 150 + 150 + 3 + 20,
              "oracle": "separately decoded sources; exact integer straight-alpha over; top opaque track as base"}
    (root / "verification.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", required=True, type=Path)
    run(parser.parse_args().output)
