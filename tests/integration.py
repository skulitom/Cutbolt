"""End-to-end checks against original frames/audio, independent of render filters."""
from engine import ENGINE
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from fixtures import FPS, HEIGHT, WIDTH, frame_bytes, generate


def run(executable, root):
    root = root.resolve()
    if root == ROOT or ROOT in root.parents:
        raise ValueError("Generated media must be outside the repository")
    root.mkdir(parents=True, exist_ok=True)
    media = root / "media"
    output = root / "output"
    output.mkdir(exist_ok=True)
    sources = generate(media)
    passed = []

    def check(name, condition):
        if not condition:
            raise AssertionError(name)
        passed.append(name)

    def request(command, expected_error=None):
        process = subprocess.run([str(executable)], input=json.dumps(command).encode(), capture_output=True, timeout=180)
        response = json.loads(process.stdout)
        if expected_error:
            assert process.returncode == 1 and response["error"]["code"] == expected_error, response
            return response
        assert process.returncode == 0 and response["ok"], response
        return response["result"]

    def time(n):
        return {"num": n, "den": 1}

    caps = request({"command": "capabilities"})
    check("agent.capabilities", caps["local_only"] and "render.run" in caps["commands"])
    request({"command": "capabilities", "unrecognized": True}, "INVALID_JSON")
    check("agent.strict_requests", True)
    inspected = request({"command": "media.inspect", "path": str(sources[0]), "input_root": str(media)})
    ready = inspected["timeline"]
    check("media.inspect", len(inspected["metadata"]["streams"]) == 2 and ready["ready"] and ready["frames"] == FPS * 12
          and ready["asset"] == {"id": sources[0].stem, "path": sources[0].name, "duration": time(12)})
    before_hashes = [hashlib.sha256(path.read_bytes()).hexdigest() for path in sources]
    project = request({"command": "project.create", "id": "first-demo", "width": WIDTH, "height": HEIGHT, "frame_rate": time(FPS)})
    operations = []
    for index, path in enumerate(sources):
        operations += [
            {"op": "media.add", "asset": {"id": f"asset-{index}", "path": str(path), "duration": time(12)}},
            {"op": "clip.append", "clip": {"id": f"clip-{index}", "asset_id": f"asset-{index}", "source_in": time(0), "duration": time(12)}},
            {"op": "clip.trim", "clip_id": f"clip-{index}", "source_in": time(1), "duration": time(10)},
        ]
    operations += [
        {"op": "clip.split", "clip_id": "clip-0", "new_clip_id": "clip-0-right", "offset": time(4)},
        {"op": "clip.move", "clip_id": "clip-2", "to_index": 0},
        {"op": "clip.append", "clip": {"id": "discard", "asset_id": "asset-1", "source_in": time(0), "duration": time(1)}},
        {"op": "clip.remove", "clip_id": "discard"},
    ]
    project = request({"command": "timeline.apply", "project": project, "expected_revision": 0, "operations": operations})
    # Persist and reload the portable project through the public command contract.
    project_path = root / "project.json"
    project_path.write_text(json.dumps(project, indent=2) + "\n", encoding="utf-8")
    project = json.loads(project_path.read_text(encoding="utf-8"))
    valid = request({"command": "project.validate", "project": project})
    check("project.roundtrip", valid["duration"] == time(30) and valid["revision"] == 1)
    request({"command": "timeline.apply", "project": project, "expected_revision": 0,
             "operations": [{"op": "clip.remove", "clip_id": "clip-1"}]}, "REVISION_CONFLICT")
    check("agent.revision_guard", True)
    bad = copy.deepcopy(project)
    bad["clips"][0]["source_in"] = {"num": 1, "den": 1000}
    request({"command": "project.validate", "project": bad}, "UNALIGNED_TIME")
    bad["clips"][0]["source_in"] = time(10)
    request({"command": "project.validate", "project": bad}, "INVALID_RANGE")
    check("timeline.reject_invalid_ranges", True)
    request({"command": "media.inspect", "path": str(sources[0]), "input_root": str(output)}, "PATH_OUTSIDE_ROOT")
    check("agent.input_roots", True)

    render_request = {"command": "render.run", "project": project, "input_root": str(media),
                      "output_root": str(output), "output": str(output / "first-edit.mkv")}
    plan_request = dict(render_request, command="render.plan")
    plan = request(plan_request)
    check("agent.dry_render_plan", plan["frames"] == 750 and not (output / "first-edit.mkv").exists())
    rendered = request(render_request)
    check("export.reference_counts", rendered["frames"] == 750 and rendered["samples"] == 1_440_000)
    result = Path(rendered["output"])
    ffmpeg = os.environ.get("CUTBOLT_FFMPEG", "ffmpeg")

    # Decode every output frame and compare to independently constructed source frames.
    frames = subprocess.Popen([ffmpeg, "-v", "error", "-i", str(result), "-map", "0:v:0", "-pix_fmt", "rgb24", "-f", "rawvideo", "pipe:1"], stdout=subprocess.PIPE)
    try:
        for source in [2, 0, 1]:
            for frame in range(25, 275):
                expected = frame_bytes(source, frame)
                actual = frames.stdout.read(len(expected))
                if actual != expected:
                    raise AssertionError(f"Wrong frame at source {source}, frame {frame}")
        assert frames.stdout.read(1) == b""
        assert frames.wait(timeout=30) == 0
    finally:
        if frames.poll() is None:
            frames.kill()
            frames.wait()
    for test in ["timeline.assemble", "timeline.trim", "timeline.split", "timeline.reorder_remove"]:
        check(test, True)

    def audio_bytes(path):
        return subprocess.check_output([ffmpeg, "-v", "error", "-i", str(path), "-map", "0:a:0", "-f", "s16le", "pipe:1"], timeout=30)
    # Byte slices select exact samples independently of FFmpeg's atrim filter.
    expected_audio = b"".join(audio_bytes(sources[index])[48_000 * 4:528_000 * 4] for index in [2, 0, 1])
    check("audio.cut_sync", audio_bytes(result) == expected_audio)
    check("media.originals_preserved", before_hashes == [hashlib.sha256(path.read_bytes()).hexdigest() for path in sources])
    output_hash = hashlib.sha256(result.read_bytes()).hexdigest()
    request(render_request, "OUTPUT_EXISTS")
    check("export.no_overwrite", hashlib.sha256(result.read_bytes()).hexdigest() == output_hash)
    escaped = dict(render_request, output=str(root / "outside.mkv"))
    request(escaped, "PATH_OUTSIDE_ROOT")
    check("agent.output_roots", not (root / "outside.mkv").exists())
    # Even if explicitly addressed as output, original media is protected.
    source_output = dict(render_request, output_root=str(media), output=str(sources[0]))
    request(source_output, "OUTPUT_EXISTS")
    check("export.source_overwrite_rejected", before_hashes[0] == hashlib.sha256(sources[0].read_bytes()).hexdigest())
    # Exercise cleanup after an actual encoder-process failure.
    saved = os.environ.get("CUTBOLT_FFMPEG")
    os.environ["CUTBOLT_FFMPEG"] = os.environ.get("CUTBOLT_FFPROBE", "ffprobe")
    failure = dict(render_request, output=str(output / "failed.mkv"))
    try:
        request(failure, "TOOL_FAILED")
    finally:
        if saved is None:
            os.environ.pop("CUTBOLT_FFMPEG", None)
        else:
            os.environ["CUTBOLT_FFMPEG"] = saved
    check("export.failure_cleanup", not (output / "failed.mkv").exists() and not list(output.glob("*.partial.mkv")))
    report = {"passed": passed, "count": len(passed), "render": rendered, "project": str(project_path)}
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, default=ENGINE)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.output:
        run(args.executable.resolve(), args.output)
    else:
        with tempfile.TemporaryDirectory(prefix="cutbolt-test-") as temporary:
            run(args.executable.resolve(), Path(temporary))
