"""Original transparent-source checks, shared by the SDR color fixture."""
from array import array
import hashlib
import json
from pathlib import Path
import subprocess
import wave

from engine import ENGINE
from scenes import identity, time

W, H, N = 32, 16, 6


def run(root, convert_pixel):
    root = Path(root).resolve()
    root.mkdir(parents=True, exist_ok=False)
    sources, output, store = [root / name for name in ("sources", "output", "store")]
    for path in (sources, output, store):
        path.mkdir()

    def ff(args):
        process = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-n", *map(str, args)],
                                 capture_output=True, timeout=180)
        assert process.returncode == 0, process.stderr.decode(errors="replace")
        return process.stdout

    def metadata(path):
        value = json.loads(subprocess.check_output(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)]))
        return next(s for s in value["streams"] if s["codec_type"] == "video")

    def call(request, error=None):
        process = subprocess.run([str(ENGINE)], input=json.dumps(request).encode(), capture_output=True, timeout=180)
        value = json.loads(process.stdout)
        if error:
            assert process.returncode == 1 and value["error"]["code"] == error, value
            return value["error"]
        assert process.returncode == 0 and value["ok"], value
        return value["result"]

    pcm = array("h", [(n * 41 + c * 109) % 17001 - 8500 for n in range(N * 1920) for c in range(2)]).tobytes()
    with wave.open(str(sources / "sound.wav"), "wb") as file:
        file.setparams((2, 2, 48000, 0, "NONE", "not compressed"))
        file.writeframes(pcm)
    # Code-value charts are independent of the engine and include hidden color at zero alpha.
    codes = []
    bgra = bytearray()
    for n in range(N):
        planes = [[], [], [], []]
        for y in range(H):
            for x in range(W):
                planes[0].append((64 + x * 27 + y * 13 + n * 31) % 1024)
                planes[1].append((400 + x * 7 + n * 29) % 1024)
                planes[2].append((600 - y * 9 + n * 17) % 1024)
                alpha = [0, 1, 341, 512, 682, 1022, 1023][(x + y + n) % 7]
                planes[3].append(alpha)
                bgra.extend(((x * 7 + n * 13) % 256, (y * 11 + n * 19) % 256,
                             (x * 3 + y * 5 + n * 23) % 256, (alpha * 255 + 511) // 1023))
        codes.append(b"".join(v.to_bytes(2, "little") for plane in planes for v in plane))
    (sources / "original.yuva").write_bytes(b"".join(codes))
    (sources / "original.bgra").write_bytes(bgra)
    table = sources / "invert-red.cube"
    table.write_text("LUT_1D_SIZE 2\n1 0 0\n0 1 1\n", encoding="ascii")
    fixtures = {}
    for name, codec, profile in (("prores4444", "prores_ks", 4), ("prores4444xq", "prores_ks", 5), ("ffv1-bgra", "ffv1", None)):
        packed = codec == "ffv1"
        fmt = "bgra" if packed else "yuva444p10le"
        path = sources / (name + (".mkv" if packed else ".mov"))
        args = ["-f", "rawvideo", "-pixel_format", fmt, "-video_size", f"{W}x{H}", "-framerate", "25",
                "-i", sources / ("original.bgra" if packed else "original.yuva"), "-i", sources / "sound.wav",
                "-map", "0:v:0", "-map", "1:a:0", "-vf", "setsar=1", "-c:v", codec, "-threads", "1",
                "-pix_fmt", fmt, "-colorspace", "rgb" if packed else "bt709", "-color_range", "pc" if packed else "tv",
                "-color_trc", "bt709", "-color_primaries", "bt709", "-c:a", "pcm_s16le"]
        if not packed:
            args += ["-profile:v", str(profile), "-alpha_bits", "16", "-qscale:v", "4"]
        ff(args + [path])
        meta = metadata(path)
        assert meta["pix_fmt"] in ({"bgra"} if packed else {"yuva444p10le", "yuva444p12le"}), meta
        native = ff(["-i", path, "-map", "0:v:0", "-an", "-fps_mode", "passthrough", "-pix_fmt", meta["pix_fmt"], "-f", "rawvideo", "-"])
        if packed:
            assert native == bgra
        fixtures[name] = (path, meta["pix_fmt"], native)
    opaque = sources / "opaque.mkv"
    ff(["-i", fixtures["ffv1-bgra"][0], "-map", "0", "-c:v", "ffv1", "-pix_fmt", "bgr0", "-threads", "1",
        "-colorspace", "rgb", "-color_range", "pc", "-color_trc", "bt709", "-color_primaries", "bt709", "-c:a", "copy", opaque])
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}

    def recipe(name, transfer="bt709", **changes):
        path, fmt, _ = fixtures[name]
        packed = fmt == "bgra"
        value = {"schema_version": 1, "id": name + "-" + transfer,
                 "source": {"file": identity(path, sources), "alpha": "straight",
                            "sdr": {"matrix": "rgb" if packed else "bt709", "range": "full" if packed else "limited",
                                    "transfer": "bt709", "missing_tags": "reject" if packed else "use_declared"}},
                 "working_transfer": transfer, "source_in": time(0), "duration": time(N, 25), "rate": time(1),
                 "reverse": False, "freeze": False, "width": W, "height": H, "audio": "resample"}
        value.update(changes)
        return value

    def reference(name, transfer):
        _, fmt, native = fixtures[name]
        packed = fmt == "bgra"
        bits = 8 if packed else (12 if "12" in fmt else 10)
        maximum = (1 << bits) - 1
        stride = W * H * (4 if packed else 8)
        assert len(native) == N * stride
        frames = []
        for n in range(N):
            frame = native[n * stride:(n + 1) * stride]
            rgba = bytearray()
            for i in range(W * H):
                if packed:
                    r, g, b = frame[4 * i + 2], frame[4 * i + 1], frame[4 * i]
                    a = frame[4 * i + 3]
                    rgb = convert_pixel(r, g, b, "rgb", "full", "bt709", transfer)
                else:
                    sample = lambda plane: int.from_bytes(frame[(plane * W * H + i) * 2:(plane * W * H + i) * 2 + 2], "little")
                    rgb = convert_pixel(sample(0), sample(1), sample(2), "bt709", "limited", "bt709", transfer, bits)
                    a = (sample(3) * 255 + maximum // 2) // maximum
                rgba.extend((*rgb, a))
            frames.append(bytes(rgba))
        return frames

    receipts, actuals, cases = {}, {}, []

    def check(name, request, label, samples=None):
        samples = [(n, n, 0, 1) for n in range(N)] if samples is None else samples
        plan = call({"command": "media.conform.inspect", "input_root": str(sources), "recipe": request})
        path = output / (label + ".mkv")
        receipt = call({"command": "media.conform", "input_root": str(sources), "output_root": str(output), "output": str(path), "recipe": request})
        assert plan["alpha"] == receipt["alpha"] and receipt["alpha"]["output"] == "straight"
        assert metadata(path)["pix_fmt"] == "bgra"
        assert receipt["normalization"]["assumed_tags"] == ([] if name == "ffv1-bgra" else ["color_range"])
        frames = reference(name, request["working_transfer"])
        if "lut" in request:
            frames = [bytes(255 - value if i % 4 == 0 else value for i, value in enumerate(frame)) for frame in frames]
        want = bytearray()
        for first, second, numerator, denominator in samples:
            for y in range(request["height"]):
                for x in range(request["width"]):
                    p = (y * H // request["height"] * W + x * W // request["width"]) * 4
                    for c in range(4):
                        want.append((frames[first][p + c] * (denominator - numerator) + frames[second][p + c] * numerator + denominator // 2) // denominator)
        rgba = ff(["-i", path, "-map", "0:v:0", "-an", "-pix_fmt", "rgba", "-f", "rawvideo", "-"])
        assert len(rgba) == len(want)
        assert rgba[3::4] == want[3::4], label
        error = max(abs(a - b) for i, (a, b) in enumerate(zip(rgba, want)) if i % 4 != 3)
        assert error <= 1, (label, error)
        sound = ff(["-i", path, "-map", "0:a:0", "-vn", "-f", "s16le", "-"])
        assert sound == (bytes(len(samples) * 1920 * 4) if request["audio"] == "mute" else pcm)
        assert receipt["sha256"] == hashlib.sha256(path.read_bytes()).hexdigest()
        receipts[label], actuals[label] = receipt, rgba
        cases.append({"case": label, "frames": len(samples), "maximum_rgb_error": error, "alpha_exact": True,
                      "assumed_tags": receipt["normalization"]["assumed_tags"]})
        return rgba

    for name in fixtures:
        for transfer in ("bt709", "srgb"):
            r = recipe(name, transfer)
            check(name, r, r["id"])
    r = recipe("prores4444", source_in=time(5, 25), duration=time(4, 25), reverse=True, audio="mute", width=17, height=9)
    check("prores4444", r, "alpha-reverse-resize", [(n, n, 0, 1) for n in (5, 4, 3, 2)])
    r = recipe("prores4444", "srgb", duration=time(4, 25), audio="mute",
               remap={"segments": [{"duration": time(4, 25), "start_rate": time(1, 2), "end_rate": time(1, 2), "reverse": False}],
                      "video_sampling": "linear", "audio_pitch": "mute"},
               lut={"file": identity(table, sources), "interpolation": "linear"})
    check("prores4444", r, "alpha-lut-linear", [(0, 0, 0, 1), (0, 1, 1, 2), (1, 1, 0, 1), (1, 2, 1, 2)])

    # Use a prepared transparent clip through the real saved-session compositor over black.
    label = "prores4444-srgb"
    project = call({"command": "project.create", "id": "prepared-alpha", "width": W, "height": H, "frame_rate": time(25)})
    call({"command": "session.create", "store_root": str(store), "project": project, "request_id": "create"})
    call({"command": "session.apply", "store_root": str(store), "project_id": "prepared-alpha", "expected_revision": 0,
          "request_id": "place", "operations": [
              {"op": "media.add", "asset": receipts[label]["asset"]},
              {"op": "tracks.edit", "edit": {"op": "create", "duration": time(N, 25)}},
              {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "alpha", "kind": "video", "locked": False,
                                                                         "enabled": True, "composite": "alpha_over", "clips": []}}},
              {"op": "tracks.edit", "edit": {"op": "place", "track_id": "alpha", "collision": "reject",
                 "clip": {"id": "prepared", "asset_id": receipts[label]["asset"]["id"], "start": time(0), "source_in": time(0), "duration": time(N, 25)}}}]})
    project = call({"command": "session.get", "store_root": str(store), "project_id": "prepared-alpha"})
    render_path = output / "composited.mkv"
    call({"command": "render.run", "project": project, "input_root": str(output), "output_root": str(output), "output": str(render_path)})
    rgb = ff(["-i", render_path, "-map", "0:v:0", "-an", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"])
    rgba = actuals[label]
    assert rgb == bytes((rgba[i + c] * rgba[i + 3] + 127) // 255 for i in range(0, len(rgba), 4) for c in range(3))
    hashes = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    invalid = []
    r = recipe("ffv1-bgra"); r["source"]["file"] = identity(opaque, sources); invalid.append((r, "INVALID_ALPHA"))
    r = recipe("prores4444"); r["source"].pop("alpha"); invalid.append((r, "INVALID_ALPHA"))
    r = recipe("prores4444"); r["source"]["alpha"] = "premultiplied"; invalid.append((r, "INVALID_JSON"))
    r = recipe("ffv1-bgra"); r["source"].pop("sdr"); r["source"]["color"] = "encoded_rgb"; invalid.append((r, "INVALID_CONFORM"))
    r = recipe("prores4444"); r["source"]["sdr"]["range"] = "full"; invalid.append((r, "INVALID_COLOR"))
    for i, (r, code) in enumerate(invalid):
        error = call({"command": "media.conform", "recipe": r, "input_root": str(sources), "output_root": str(output), "output": str(output / f"bad-{i}.mkv")}, code)
        if code == "INVALID_ALPHA":
            assert error["path"] == "recipe.source.alpha" and error["retry"] == "changed"
            assert "pixel_format" in error["detail"] and error["hint"]
    assert originals == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    assert hashes == {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in output.iterdir()}
    assert not list(output.glob(".cutbolt-*"))
    result = {"cases": cases, "rejected_cases": len(invalid), "saved_session_composite_exact": True,
              "sources_preserved": True, "limits": "explicit straight alpha; native ProRes YUVA444 or FFV1 BGRA to RGBA8; Decimal RGB tolerance 1, alpha/PCM exact"}
    (root / "verification.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result
