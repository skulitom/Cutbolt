"""Agent ergonomics: committed-request lookup, field-named errors, scopes on native tracks, audio-only export
and workspace-relative paths.

Independent checks: stored receipts equal the original responses; unaligned times name the field and value;
scope populations match an exact reference computed from separately decoded source frames; audio-only
exports of 1080p sequential and native-track timelines match integer PCM assembled from the known sources,
finish without rendering pictures and publish no video stream; in a workspace no reported string names the
workspace directory, and the reported relative files exist and are accepted back.
"""
from engine import ENGINE
import argparse
import budgets
from fractions import Fraction as F
import hashlib
import json
import math
from pathlib import Path
import shutil
import subprocess
import time as clock

import numpy as np

from agents import Client
from luts_scopes import scope_reference
from scenes import time

ROOT = Path(__file__).resolve().parents[1]
EXE = ENGINE
WIDE, SMALL = (1920, 1080), (160, 90)
SECONDS = 30


def call(command, error=None, workspace=None, **fields):
    process = subprocess.run([str(EXE)] + (["--workspace", str(workspace)] if workspace else []),
                             input=json.dumps({"command": command, **fields}), text=True, encoding="utf-8", capture_output=True, timeout=1800)
    result = json.loads(process.stdout)
    if error:
        assert process.returncode == 1 and not result["ok"] and result["error"]["code"] == error, (error, result)
        return result["error"]
    assert process.returncode == 0 and result["ok"], result
    return result["result"]


def strings(value):
    if isinstance(value, str):
        yield value
    elif isinstance(value, (list, dict)):
        for item in value.values() if isinstance(value, dict) else value:
            yield from strings(item)


def tone(seconds, frequencies, amplitude=6000):
    n = np.arange(seconds * 48000)
    channels = [np.round(amplitude * np.sin(2 * np.pi * f * n / 48000)) for f in frequencies]
    return np.stack(channels, axis=1).astype(np.int16)


def ffmpeg(args, timeout=600):
    return subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-n"] + args, capture_output=True, check=True, timeout=timeout).stdout


def source(path, video, size, seconds, audio):
    pcm = path.with_suffix(".pcm")
    pcm.write_bytes(audio.tobytes())
    ffmpeg(["-f", "lavfi", "-i", f"{video}{':' if '=' in video else '='}s={size[0]}x{size[1]}:r=25:d={seconds}", "-f", "s16le", "-ar", "48000", "-ac", "2", "-i", str(pcm),
            "-map", "0:v:0", "-map", "1:a:0", "-vf", "setsar=1", "-c:v", "ffv1", "-pix_fmt", "bgr0", "-c:a", "pcm_s16le", str(path)])
    pcm.unlink()
    return {"id": path.stem, "path": str(path), "duration": time(seconds),
            "identity": {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}}


def samples(t):
    value = F(t["num"], t["den"]) * 48000
    assert value.denominator == 1
    return value.numerator


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=False)
    sources, output, store = root / "sources", root / "output", root / "store"
    for p in (sources, output, store):
        p.mkdir()
    audio = {"wide-0": tone(SECONDS, (437, 691)), "wide-1": tone(SECONDS, (563, 997), 5000), "small": tone(4, (311, 523))}
    assets = {"wide-0": source(sources / "wide-0.mkv", "color=c=0x335577", WIDE, SECONDS, audio["wide-0"]),
              "wide-1": source(sources / "wide-1.mkv", "color=c=0x775533", WIDE, SECONDS, audio["wide-1"]),
              "small": source(sources / "small.mkv", "testsrc2", SMALL, 4, audio["small"])}
    originals = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    small_rgb = np.frombuffer(ffmpeg(["-i", str(sources / "small.mkv"), "-map", "0:v:0", "-pix_fmt", "rgb24", "-f", "rawvideo", "-"]), np.uint8).reshape(-1, SMALL[1], SMALL[0], 3)
    passed, timings = [], {}

    # Committed requests can be looked up after a lost response, without replaying them.
    project = call("project.create", id="ergonomics", width=SMALL[0], height=SMALL[1], frame_rate=time(25))
    created = call("session.create", store_root=str(store), project=project, request_id="create")
    edits = [{"op": "media.add", "asset": assets["small"]},
             {"op": "tracks.edit", "edit": {"op": "create", "duration": time(3)}},
             {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "picture", "kind": "video", "locked": False, "enabled": True, "clips": []}}},
             {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "sound", "kind": "audio", "locked": False, "enabled": True, "clips": []}}},
             {"op": "tracks.edit", "edit": {"op": "place", "track_id": "picture", "clip": {"id": "shot", "asset_id": "small", "start": time(12, 25), "source_in": time(2, 5), "duration": time(2)}, "collision": "reject"}},
             {"op": "tracks.edit", "edit": {"op": "place", "track_id": "sound", "clip": {"id": "voice", "asset_id": "small", "start": time(12, 25), "source_in": time(2, 5), "duration": time(2)}, "collision": "reject"}}]
    applied = call("session.apply", store_root=str(store), project_id="ergonomics", request_id="tracks", expected_revision=0, operations=edits)
    assert call("session.receipt", store_root=str(store), project_id="ergonomics", request_id="tracks") == applied
    assert call("session.receipt", store_root=str(store), project_id="ergonomics", request_id="create") == created
    retried = call("session.apply", store_root=str(store), project_id="ergonomics", request_id="tracks", expected_revision=0, operations=edits)
    assert retried["revision"] == applied["revision"] == 1
    missing = call("session.receipt", "REQUEST_NOT_FOUND", store_root=str(store), project_id="ergonomics", request_id="never-sent")
    assert "never-sent" in missing["message"]
    call("session.receipt", "PROJECT_NOT_FOUND", store_root=str(store), project_id="absent", request_id="tracks")
    client = Client(EXE)
    try:
        client.initialize()
        tools = {tool["name"]: tool for tool in client.rpc("tools/list")["result"]["tools"]}
        assert tools["cutbolt_session_receipt"]["annotations"]["readOnlyHint"] is True
        assert client.call("session.receipt", store_root=str(store), project_id="ergonomics", request_id="tracks") == applied
        client.call("session.receipt", "REQUEST_NOT_FOUND", store_root=str(store), project_id="ergonomics", request_id="never-sent")
    finally:
        client.close()
    assert call("session.history", store_root=str(store), project_id="ergonomics")["entries"][0]["revision"] == 1
    passed.append("ergonomics.committed_request_lookup")

    # Rejected times name the field, the exact value and the clock.
    saved = call("session.get", store_root=str(store), project_id="ergonomics")
    late = call("session.apply", "UNALIGNED_TIME", store_root=str(store), project_id="ergonomics", request_id="late", expected_revision=1,
                operations=[{"op": "tracks.edit", "edit": {"op": "place", "track_id": "picture", "clip": {"id": "late", "asset_id": "small", "start": time(0), "source_in": time(1, 3), "duration": time(1, 5)}, "collision": "reject"}}])
    assert 'clip "late"' in late["message"] and "source_in" in late["message"] and "1/3 s" in late["message"] and "25/1 per second" in late["message"], late
    frame = call("preview.frame", "UNALIGNED_TIME", project=saved, input_root=str(sources), output_root=str(output), output=str(output / "bad.png"), time=time(1, 50))
    assert frame["message"].startswith("time:") and "1/50 s" in frame["message"], frame
    call("session.receipt", "REQUEST_NOT_FOUND", store_root=str(store), project_id="ergonomics", request_id="late")
    passed.append("ergonomics.errors_name_fields_and_values")

    # Scopes measure native-track frames, including gaps, and agree with the equivalent sequential timeline.
    sequential = call("project.create", id="sequential", width=SMALL[0], height=SMALL[1], frame_rate=time(25))
    sequential = call("timeline.apply", project=sequential, expected_revision=0, operations=[
        {"op": "media.add", "asset": assets["small"]},
        {"op": "clip.append", "clip": {"id": "pause", "gap": True, "source_in": time(0), "duration": time(12, 25)}},
        {"op": "clip.append", "clip": {"id": "shot", "asset_id": "small", "source_in": time(2, 5), "duration": time(2)}}])
    scope_checks = 0
    for n, columns in [(0, 1), (12, 7), (13, 160), (40, 9), (62, 32)]:
        fields = {"input_root": str(sources), "time": time(n, 25), "input_transfer": "srgb", "missing_tags": "use_declared", "columns": columns}
        report = call("scopes.inspect", project=saved, **fields)
        rgb = bytes(SMALL[0] * SMALL[1] * 3) if n < 12 or n >= 62 else small_rgb[n - 12 + 10].tobytes()
        assert report["values"] == scope_reference(rgb, SMALL[0], columns), n
        assert report["rgb_sha256"] == hashlib.sha256(rgb).hexdigest() and report["pixels"] == SMALL[0] * SMALL[1]
        if n < 62:
            assert call("scopes.inspect", project=sequential, **fields)["values"] == report["values"]
        scope_checks += 1
    passed.append("ergonomics.scopes_on_native_tracks_exact")

    # Audio-only exports of long 1080p timelines read PCM only and never render pictures.
    wide = call("project.create", id="wide-sequential", width=WIDE[0], height=WIDE[1], frame_rate=time(25))
    wide = call("timeline.apply", project=wide, expected_revision=0, operations=[
        {"op": "media.add", "asset": assets["wide-0"]}, {"op": "media.add", "asset": assets["wide-1"]},
        {"op": "clip.append", "clip": {"id": "a", "asset_id": "wide-0", "source_in": time(2), "duration": time(20)}},
        {"op": "clip.append", "clip": {"id": "pause", "gap": True, "source_in": time(0), "duration": time(1)}},
        {"op": "clip.append", "clip": {"id": "b", "asset_id": "wide-1", "source_in": time(5), "duration": time(20)}},
        {"op": "clip.append", "clip": {"id": "a2", "asset_id": "wide-0", "source_in": time(12, 25), "duration": time(10)}}])
    sequence_pcm = np.concatenate([audio["wide-0"][2 * 48000:22 * 48000], np.zeros((48000, 2), np.int16),
                                   audio["wide-1"][5 * 48000:25 * 48000], audio["wide-0"][23040:23040 + 10 * 48000]])
    tracks = call("project.create", id="wide-tracks", width=WIDE[0], height=WIDE[1], frame_rate=time(25))
    music_start = F(10) + F(7, 48000)
    voice, music = {"start": time(0), "source_in": time(1), "duration": time(29)}, {"start": {"num": music_start.numerator, "den": music_start.denominator}, "source_in": time(0), "duration": time(29)}
    tracks = call("timeline.apply", project=tracks, expected_revision=0, operations=[
        {"op": "media.add", "asset": assets["wide-0"]}, {"op": "media.add", "asset": assets["wide-1"]},
        {"op": "tracks.edit", "edit": {"op": "create", "duration": time(40)}},
        {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "picture", "kind": "video", "locked": False, "enabled": True, "clips": []}}},
        {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "voice", "kind": "audio", "locked": False, "enabled": True, "clips": []}}},
        {"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "music", "kind": "audio", "locked": False, "enabled": True, "clips": []}}},
        {"op": "tracks.edit", "edit": {"op": "place", "track_id": "picture", "clip": {"id": "shot", "asset_id": "wide-0", "start": time(0), "source_in": time(0), "duration": time(30)}, "collision": "reject"}},
        {"op": "tracks.edit", "edit": {"op": "place", "track_id": "voice", "clip": {"id": "voice-clip", "asset_id": "wide-0", **voice}, "collision": "reject"}},
        {"op": "tracks.edit", "edit": {"op": "place", "track_id": "music", "clip": {"id": "music-clip", "asset_id": "wide-1", **music}, "collision": "reject"}}])
    mix = np.zeros((40 * 48000, 2), np.int64)
    for clip, name in ((voice, "wide-0"), (music, "wide-1")):
        start, offset, count = samples(clip["start"]), samples(clip["source_in"]), samples(clip["duration"])
        mix[start:start + count] += audio[name][offset:offset + count]
    track_pcm = np.clip(mix, -32768, 32767).astype(np.int16)

    def export(name, project, profile, expected, frames, range_=None):
        extension = "wav" if profile == "reference" else "m4a"
        fields = {"project": project, "input_root": str(sources), "output_root": str(output), "output": str(output / f"{name}.{extension}"),
                  "profile": profile, "streams": "audio"}
        if range_:
            fields["range"] = range_
        plan = call("export.inspect", **fields)
        assert plan["video_frames"] == 0 and plan["audio_samples"] == len(expected) and plan["video"] is None
        started = clock.perf_counter()
        receipt = call("export.run", **fields)
        timings[name] = round(clock.perf_counter() - started, 3)
        path = output / f"{name}.{extension}"
        streams = json.loads(subprocess.run(["ffprobe", "-v", "error", "-show_streams", "-of", "json", str(path)], capture_output=True, check=True).stdout)["streams"]
        assert [s["codec_type"] for s in streams] == ["audio"], streams
        decoded = np.frombuffer(ffmpeg(["-i", str(path), "-map", "0:a:0", "-f", "s16le", "-"]), np.int16).reshape(-1, 2)
        if profile == "reference":
            assert streams[0]["codec_name"] == "pcm_s16le" and np.array_equal(decoded, expected), name
        else:
            assert streams[0]["codec_name"] == "aac" and receipt["verification"]["aac_priming_samples"] == 1024
            assert len(decoded) == (len(expected) + 1023) // 1024 * 1024
            actual = decoded[:len(expected)].astype(np.float64)
            error = np.sum((actual - expected) ** 2)
            snr = 10 * math.log10(np.sum(expected.astype(np.float64) ** 2) / error)
            assert snr >= 28, (name, snr)
            lags = {lag: np.mean((actual[16 + lag:len(actual) - 16 + lag] - expected[16:-16]) ** 2) for lag in range(-4, 5)}
            assert min(lags, key=lags.get) == 0, lags
        # Pictures are never decoded or composited: the whole export costs less than a quarter of
        # rendering the same timeline's 1080p reference video, extrapolated from a measured range.
        budgets.check(timings[name] < 0.25 * frames * video_seconds_per_frame, (name, timings[name], video_seconds_per_frame))
        return receipt

    started = clock.perf_counter()
    call("export.run", project=wide, input_root=str(sources), output_root=str(output), output=str(output / "video-sample.mkv"),
         profile="reference", streams="video", range={"start": time(0), "duration": time(2)})
    video_seconds_per_frame = (clock.perf_counter() - started) / 50
    timings["video_reference_seconds_per_frame"] = round(video_seconds_per_frame, 4)

    export("sequence-audio", wide, "reference", sequence_pcm, 1275)
    start, length = F(18), F(5)
    export("sequence-range-audio", wide, "reference", sequence_pcm[int(start * 48000):int((start + length) * 48000)], 125,
           {"start": time(18), "duration": time(5)})
    passed.append("ergonomics.audio_only_sequential_exact_without_video")
    export("tracks-audio", tracks, "reference", track_pcm, 1000)
    export("tracks-aac", tracks, "h264_aac", track_pcm, 1000)
    passed.append("ergonomics.audio_only_tracks_exact_and_aac")

    # In a workspace, prepared and exported files, probed sources and job receipts are reported
    # relative to it, and the relative assets go straight back in; without one they stay absolute.
    space = root / "space"
    space.mkdir()
    for name in ("clip.mp4", "take.mp4"):
        ffmpeg(["-f", "lavfi", "-i", f"testsrc2=s={SMALL[0]}x{SMALL[1]}:r=25:d=1", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:d=1",
                "-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac", "-ac", "2", str(space / name)])
    inspected = call("media.inspect", workspace=space, path="clip.mp4")
    single = call("media.prepare", workspace=space, path="clip.mp4")
    batch = call("media.prepare", workspace=space, paths=["take.mp4"])
    assert inspected["path"] == "clip.mp4", inspected["path"]
    assert single["converted"] and single["output"] == single["asset"]["path"] == "clip-prepared.mkv", single
    prepared = batch["prepared"][0]
    assert prepared["path"] == "take.mp4" and prepared["result"]["output"] == prepared["result"]["asset"]["path"] == "take-prepared.mkv", batch
    assert batch["operations"] == [{"op": "media.add", "asset": prepared["result"]["asset"]}], batch
    call("session.create", workspace=space, request_id="create", id="space", width=SMALL[0], height=SMALL[1], frame_rate=time(25))
    call("session.apply", workspace=space, project_id="space", request_id="add", expected_revision=0, operations=batch["operations"] + [
        {"op": "media.add", "asset": single["asset"]},
        {"op": "clip.append", "clip": {"id": "a", "asset_id": "clip", "source_in": time(0), "duration": time(1)}},
        {"op": "clip.append", "clip": {"id": "b", "asset_id": "take", "source_in": time(0), "duration": time(1)}}])
    exported = call("export.run", workspace=space, project={"project_id": "space"}, output="cut.wav", profile="reference", streams="audio")
    assert exported["output"] == "cut.wav" and (space / "cut.wav").is_file(), exported
    reports = [inspected, single, batch, exported]
    if call("capabilities", section="jobs")["jobs"]["available"]:
        ticket = call("job.start", workspace=space, request_id="prepare", run="media.prepare", arguments={"path": "clip.mp4", "output": "queued.mkv"})
        waited = {"finished": False}
        while not waited["finished"]:
            waited = call("job.wait", workspace=space, job_id=ticket["job_id"], timeout_seconds=120)
        assert waited["status"] == "completed" and waited["result"]["output"] == waited["result"]["asset"]["path"] == "queued.mkv", waited
        reports += [ticket, waited]
    leaked = [s for s in strings(reports) if str(space).lower() in s.lower()]
    assert not leaked, leaked
    direct = call("media.prepare", path=str(space / "clip.mp4"), input_root=str(space), output_root=str(space), output=str(space / "direct.mkv"))
    assert Path(direct["output"]).is_absolute() and Path(direct["asset"]["path"]).is_absolute(), direct
    shutil.rmtree(space)
    passed.append("ergonomics.workspace_paths_relative")

    assert {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()} == originals
    passed.append("ergonomics.sources_preserved")
    report = {"passed": passed, "scope_frames": scope_checks, "audio_only_seconds": timings,
              "sample_frames_compared": len(sequence_pcm) + 5 * 48000 + 2 * len(track_pcm),
              "oracle": "stored responses; luts_scopes exact reference on separately decoded frames; integer PCM sums with final saturation"}
    shutil.rmtree(output)
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
