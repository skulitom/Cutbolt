"""Measure a Cutbolt editing round trip over MCP stdio, on the synthetic frame-coded test clip.

Usage: python -I cutbolt_probe.py <cutbolt.exe> <ffmpeg.exe> <ffprobe.exe> <fresh-workspace-with-source.mp4> <result.json>
Steps: tools/list; media.inspect; session.create (two excerpts: source 0-3 s and 5-8 s, picture and sound linked);
preview.frame at 3 s; session.apply ripple_delete of timeline 1-2 s; the same request resent; preview.frame at 1 s;
H.264/AAC export twice and a lossless reference export of that revision; session.undo; session.get; session.history;
an H.264 export of the undone head. Every exported frame is mapped back to its source frame number
(cutbolt_frame_ids.py) and compared with the expected list.
"""
import hashlib
import json
import os
import platform
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from mcp_session import Session  # noqa: E402

exe, ffmpeg, ffprobe, ws, out_path = sys.argv[1:6]
here = os.path.dirname(os.path.abspath(__file__))
steps = []
_h = hashlib.sha256()
with open(os.path.join(ws, "source.mp4"), "rb") as _f:
    _h.update(_f.read())
source_before = _h.hexdigest()  # hashed before the server starts
s = Session([exe, "--workspace", ws, "mcp"])


def payload(msg):
    res = msg.get("result") or {}
    if "structuredContent" in res:
        return res["structuredContent"]
    for c in res.get("content", []):
        if c.get("type") == "text":
            return json.loads(c["text"])
    return res


def step(name, tool, args, timeout=600):
    msg, line, ms = s.call(tool, args, timeout=timeout)
    body = payload(msg)
    kinds = [c.get("type") for c in (msg.get("result") or {}).get("content", [])]
    steps.append({"step": name, "tool": tool, "ms": ms, "reply_bytes": len(line), "ok": body.get("ok"),
                  "content_types": kinds})
    if not body.get("ok"):
        print(json.dumps(body)[:3000])
        raise SystemExit(f"{name} failed")
    return body["result"], msg


def run_job(name, run, arguments):
    t0 = time.perf_counter()
    ticket, _ = step(name + " (job.start)", "cutbolt_job_start",
                     {"request_id": name, "run": run, "arguments": arguments})
    while True:
        res, _ = step(name + " (job.wait)", "cutbolt_job_wait", {"job_id": ticket["job_id"], "timeout_seconds": 120})
        if res.get("finished"):
            break
    total = round((time.perf_counter() - t0) * 1000, 1)
    if res.get("status") != "completed":
        print(json.dumps(res)[:3000])
        raise SystemExit(f"{name} job did not succeed")
    steps.append({"step": name + " (start to finished)", "ms": total})
    return res


def sha256(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def frame_ids(path, yuv):
    args = [sys.executable, "-I", os.path.join(here, "cutbolt_frame_ids.py"), ffmpeg, path] + (["yuv"] if yuv else [])
    return json.loads(subprocess.run(args, check=True, capture_output=True, text=True).stdout)


def probe(path):
    out = subprocess.run([ffprobe, "-v", "error", "-count_frames", "-show_entries",
                          "stream=codec_name,profile,pix_fmt,width,height,r_frame_rate,nb_read_frames,sample_rate,"
                          "channels,duration", "-of", "json", path], check=True, capture_output=True, text=True)
    return json.loads(out.stdout)["streams"]


t_start = time.perf_counter()
init, init_line, init_ms = s.initialize()
steps.append({"step": "initialize", "ms": init_ms, "reply_bytes": len(init_line)})
tl, tl_line, tl_ms = s.request("tools/list")
steps.append({"step": "tools/list", "ms": tl_ms, "reply_bytes": len(tl_line),
              "tools": len(tl["result"]["tools"])})

inspect, _ = step("media.inspect", "cutbolt_media_inspect", {"path": "source.mp4"})
timeline = inspect["timeline"]
asset = timeline["asset"]
assert timeline["ready"] and timeline["direct"], timeline


def clip(cid, start, source_in, duration):
    return {"id": cid, "asset_id": asset["id"], "start": start, "source_in": source_in, "duration": duration}


project = {"schema_version": 1, "id": "cmp", "revision": 0, "width": 1280, "height": 720, "frame_rate": 30,
           "transfer": "bt709", "assets": [asset], "clips": [],
           "tracks": {"duration": 6, "tracks": [
               {"id": "v", "kind": "video", "enabled": True, "locked": False,
                "clips": [clip("a", 0, 0, 3), clip("b", 3, 5, 3)]},
               {"id": "a", "kind": "audio", "enabled": True, "locked": False,
                "clips": [clip("a-audio", 0, 0, 3), clip("b-audio", 3, 5, 3)]}],
               "links": [{"id": "a-link", "members": [{"clip_id": "a", "start": 0, "source_in": 0},
                                                      {"clip_id": "a-audio", "start": 0, "source_in": 0}]},
                         {"id": "b-link", "members": [{"clip_id": "b", "start": 3, "source_in": 5},
                                                      {"clip_id": "b-audio", "start": 3, "source_in": 5}]}]}}
created, _ = step("session.create", "cutbolt_session_create", {"request_id": "create", "project": project})

prev0, prev0_msg = step("preview.frame rev0 at 3 s", "cutbolt_run", {"command": "preview.frame", "arguments": {
    "project": {"project_id": "cmp", "revision": 0}, "output": "preview-rev0-3s.png", "time": 3}})

edit_args = {"project_id": "cmp", "expected_revision": 0, "request_id": "cut-1s", "operations": [
    {"op": "tracks.edit", "edit": {"op": "ripple_delete", "track_ids": ["v", "a"], "start": 1, "duration": 1,
                                   "links": "include",
                                   "right_clip_ids": [{"id": "a", "new_id": "a2"}, {"id": "a-audio", "new_id": "a2-audio"}],
                                   "right_link_ids": [{"id": "a-link", "new_id": "a2-link"}],
                                   "end_policy": "resize", "transitions": "reject_affected"}}]}
applied, _ = step("session.apply ripple_delete 1-2 s", "cutbolt_session_apply", edit_args)
retried, _ = step("session.apply same request resent", "cutbolt_session_apply", edit_args)
head_after_retry, _ = step("session.get after retry", "cutbolt_session_get", {"project_id": "cmp"})

prev1, _ = step("preview.frame rev1 at 1 s", "cutbolt_run", {"command": "preview.frame", "arguments": {
    "project": {"project_id": "cmp", "revision": 1}, "output": "preview-rev1-1s.png", "time": 1}})

h264 = {"profile": "h264_aac", "streams": "audio_video", "input_transfer": "bt709"}
exp_a = run_job("export-rev1-a", "export.run", dict(h264, project={"project_id": "cmp", "revision": 1},
                                                      output="rev1-a.mp4"))
exp_b = run_job("export-rev1-b", "export.run", dict(h264, project={"project_id": "cmp", "revision": 1},
                                                      output="rev1-b.mp4"))
exp_ref = run_job("export-rev1-reference", "export.run", {"project": {"project_id": "cmp", "revision": 1},
                                                          "output": "rev1-reference.mkv", "profile": "reference",
                                                          "streams": "audio_video"})
undone, _ = step("session.undo", "cutbolt_session_undo", {"project_id": "cmp", "request_id": "undo-1",
                                                           "expected_revision": 1})
head, _ = step("session.get head", "cutbolt_session_get", {"project_id": "cmp"})
rev0, _ = step("session.get rev0", "cutbolt_session_get", {"project_id": "cmp", "revision": 0})
history, _ = step("session.history", "cutbolt_session_history", {"project_id": "cmp"})
exp_head = run_job("export-head", "export.run", dict(h264, project={"project_id": "cmp"}, output="head.mp4"))
wall = round(time.perf_counter() - t_start, 2)
stderr = s.close()

expected_rev1 = list(range(0, 30)) + list(range(60, 90)) + list(range(150, 240))
expected_rev0 = list(range(0, 90)) + list(range(150, 240))
files = {}
for name, yuv, expected in [("rev1-a.mp4", True, expected_rev1), ("rev1-b.mp4", True, expected_rev1),
                            ("rev1-reference.mkv", False, expected_rev1), ("head.mp4", True, expected_rev0),
                            ("preview-rev0-3s.png", False, [150]), ("preview-rev1-1s.png", False, [60])]:
    path = os.path.join(ws, name)
    ids = frame_ids(path, yuv)
    files[name] = {"bytes": os.path.getsize(path), "sha256": sha256(path), "frames": ids["frames"],
                   "frames_match_expected": ids["ids"] == expected,
                   "worst_palette_distance": ids["worst_palette_distance"],
                   "first_ids": ids["ids"][:3], "last_ids": ids["ids"][-3:]}
    if not name.endswith(".png"):
        files[name]["ffprobe"] = probe(path)


def strip(p):
    p = json.loads(json.dumps(p))
    p.pop("revision", None)
    return p


head_project = head.get("project", head)
rev0_project = rev0.get("project", rev0)
result = {
    "date": time.strftime("%Y-%m-%d %H:%M:%S %z"),
    "cutbolt": subprocess.run([exe, "--version"], capture_output=True, text=True).stdout.strip(),
    "ffmpeg": subprocess.run([ffmpeg, "-version"], capture_output=True, text=True).stdout.splitlines()[0],
    "os": platform.platform(), "cpu": platform.processor(), "python": platform.python_version(),
    "source": {"path": "source.mp4", "sha256_before": source_before,
               "sha256_after": sha256(os.path.join(ws, "source.mp4")),
               "sha256_reported_by_media_inspect": inspect.get("identity", {}).get("sha256"),
               "description": "10 s, 1280x720, 30 fps H.264 High yuv420p, AAC stereo 48 kHz; frame N has its own flat colour"},
    "wall_seconds": wall,
    "steps": steps,
    "checks": {
        "retry_same_revision": applied.get("revision") == retried.get("revision"),
        "revision_after_edit": applied.get("revision"),
        "revision_after_retry": retried.get("revision"),
        "head_revision_after_retry": head_after_retry.get("project", {}).get("revision", head_after_retry.get("revision")),
        "undo_revision": undone.get("revision"),
        "head_equals_rev0_apart_from_revision": strip(head_project) == strip(rev0_project),
        "h264_exports_byte_identical": files["rev1-a.mp4"]["sha256"] == files["rev1-b.mp4"]["sha256"],
        "h264_timeline_video_sha256": [exp_a.get("result", {}).get("verification", {}).get("timeline_video_sha256"),
                                       exp_b.get("result", {}).get("verification", {}).get("timeline_video_sha256")],
        "reference_decoded_video_sha256": exp_ref.get("result", {}).get("verification", {}).get("decoded_video_sha256"),
        "source_unchanged": source_before == sha256(os.path.join(ws, "source.mp4")),
    },
    "files": files,
    "receipts": {"export_a": exp_a.get("result"), "export_b": exp_b.get("result"),
                 "export_reference": exp_ref.get("result"), "export_head": exp_head.get("result"),
                 "apply": applied, "retry": retried, "undo": undone, "history": history},
    "server_stderr_tail": stderr[-2000:],
}
with open(out_path, "w", encoding="utf-8", newline="\n") as f:
    json.dump(result, f, indent=1, ensure_ascii=False)
    f.write("\n")
print(json.dumps({"wall_seconds": wall, "checks": result["checks"],
                  "files": {k: {kk: v[kk] for kk in ("frames", "frames_match_expected", "worst_palette_distance", "bytes")}
                            for k, v in files.items()},
                  "steps": [(x["step"], x["ms"]) for x in steps]}, indent=1))
