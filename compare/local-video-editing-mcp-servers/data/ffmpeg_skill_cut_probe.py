"""Run ffmpeg-skill's cut on the colour-coded test clip and check every output frame against the expected source frames.

Usage: python -I ffmpeg_skill_cut_probe.py <ffmpeg-skill package dir> <ffmpeg.exe> <ffprobe.exe> <source.mp4> <out dir>
       <label> <runs> [--resend] [extra cut.py args...]
The cut keeps source seconds 0-1, 2-3 and 5-8 (the same final cut as Cutbolt's revision 1 on the comparison page), so
the expected frames are 0-29, 60-89 and 150-239. Each run gets a new folder with a copy of the source. cut.py runs as
its own MCP server runs a tool (a child `python scripts/cut.py ... --json`), with -s -E (no user site-packages, no
Python environment variables), and with the given FFmpeg's folder first on PATH. cutbolt_frame_ids.py must sit next to
this script. Writes <out dir>/<label>-runN.json; with --resend, also sends run 1's cut again to the same output path
and writes <out dir>/<label>-resend.json. Local paths are replaced by <run>, <ffmpeg-skill> and <ffmpeg-dir>.
"""
import hashlib
import json
import os
import pathlib
import platform
import re
import shutil
import subprocess
import sys
import time

pkg, ffmpeg, ffprobe, source, out_dir, label, runs = sys.argv[1:8]
rest = sys.argv[8:]
resend = "--resend" in rest
extra = [a for a in rest if a != "--resend"]
here = pathlib.Path(__file__).resolve().parent
# Absolute paths, so the scrub below always has a folder to replace (a bare "ffmpeg" is looked up on PATH).
ffmpeg = shutil.which(ffmpeg) or ffmpeg
ffprobe = shutil.which(ffprobe) or ffprobe
pkg = str(pathlib.Path(pkg).resolve())
out_dir = pathlib.Path(out_dir).resolve()
EXPECTED = list(range(0, 30)) + list(range(60, 90)) + list(range(150, 240))
FFMPEG_VERSION = subprocess.run([ffmpeg, "-version"], capture_output=True, text=True).stdout.splitlines()[0]


def sha(p):
    return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()


def scrub(text, folder):
    """Replace each local folder, written with any run of slashes or backslashes between its parts."""
    for path, name in ((folder, "<run>"), (pathlib.Path(pkg), "<ffmpeg-skill>"), (pathlib.Path(ffmpeg).parent, "<ffmpeg-dir>")):
        parts = [re.escape(p.strip("\\/")) for p in pathlib.Path(path).parts if p.strip("\\/")]
        if not parts:
            continue
        text = re.sub(r"[\\/]*".join(parts), lambda _m: name, text, flags=re.I)
    return text


def run_cut(folder, out):
    cmd = [sys.executable, "-s", "-E", str(pathlib.Path(pkg) / "scripts" / "cut.py"), str(folder / "source.mp4"),
           "--segments", "0-1,2-3,5-8", *extra, "-o", str(out), "--json"]
    env = dict(os.environ)
    env["PATH"] = str(pathlib.Path(ffmpeg).parent) + os.pathsep + env.get("PATH", "")
    t0 = time.perf_counter()
    proc = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", env=env)
    ms = round((time.perf_counter() - t0) * 1000, 1)
    reply = json.loads(proc.stdout) if proc.stdout.strip().startswith("{") else {"raw": proc.stdout[-2000:]}
    return proc, ms, json.loads(scrub(json.dumps(reply), folder))


def header(n):
    return {"tool": "ffmpeg-skill 2.16.1 (npm, gitHead 88e2f42), scripts/cut.py", "label": label, "run": n,
            "date": time.strftime("%Y-%m-%d %H:%M:%S %z"), "ffmpeg": FFMPEG_VERSION,
            "os": platform.platform(), "cpu": platform.processor(), "python": platform.python_version(),
            "args": ["cut.py", "<run>/source.mp4", "--segments", "0-1,2-3,5-8", *extra, "-o", "<run>/out.mp4", "--json"]}


for n in range(1, int(runs) + 1):
    folder = out_dir / f"{label}-run{n}"
    folder.mkdir(parents=True, exist_ok=False)
    shutil.copyfile(source, folder / "source.mp4")
    before = sha(folder / "source.mp4")
    out = folder / "out.mp4"
    proc, ms, reply = run_cut(folder, out)
    ids = json.loads(subprocess.run([sys.executable, "-I", str(here / "cutbolt_frame_ids.py"), ffmpeg, str(out), "yuv"],
                                    capture_output=True, text=True, check=True).stdout)
    streams = json.loads(subprocess.run([ffprobe, "-v", "error", "-show_entries",
                                         "stream=codec_type,codec_name,duration,nb_frames", "-of", "json", str(out)],
                                        capture_output=True, text=True, check=True).stdout)["streams"]
    decoded = subprocess.run([ffmpeg, "-v", "error", "-i", str(out), "-map", "0:v:0", "-f", "hash", "-hash", "sha256",
                              "-"], capture_output=True, text=True, check=True).stdout.strip()
    result = {**header(n), "exit_code": proc.returncode, "wall_ms": ms,
              "source_sha256_before": before, "source_sha256_after": sha(folder / "source.mp4"),
              "output_sha256": sha(out), "output_bytes": out.stat().st_size, "decoded_video_sha256": decoded,
              "frames": ids["frames"], "frame_ids": ids["ids"], "worst_palette_distance": ids["worst_palette_distance"],
              "frames_match_expected": ids["ids"] == EXPECTED, "streams": streams, "reply": reply,
              "stderr_tail": scrub(proc.stderr[-1500:], folder)}
    (out_dir / f"{label}-run{n}.json").write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8", newline="\n")
    a = [s for s in streams if s["codec_type"] == "audio"]
    print(f"{label} run{n}: exit {proc.returncode}, {ms} ms, frames {ids['frames']}, match {result['frames_match_expected']}, "
          f"audio {a[0]['duration'] if a else None}, sha {result['output_sha256'][:12]}, decoded {decoded[-12:]}")

if resend:
    folder = out_dir / f"{label}-run1"
    out = folder / "out.mp4"
    before = sha(out)
    proc, ms, reply = run_cut(folder, out)
    result = {**header(1), "what": "run 1's cut sent again to the same output path", "exit_code": proc.returncode,
              "wall_ms": ms, "output_sha256_before": before, "output_sha256_after": sha(out),
              "output_unchanged": sha(out) == before, "reply": reply, "stderr": scrub(proc.stderr[-1500:], folder)}
    (out_dir / f"{label}-resend.json").write_text(json.dumps(result, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(f"{label} resend: exit {proc.returncode}, unchanged {result['output_unchanged']}: {result['stderr'].strip()[:160]}")
