"""Local verification.

The default quick check runs Rust lint/tests and every correctness fixture concurrently, using each
long-form fixture's short mode; wall-clock budgets are recorded rather than enforced, and nothing is
written to the evidence. --thorough is the evidence run: every budget enforced, long-form cases complete,
budget-gated fixtures on a quiet machine, and the progress documents regenerated.
"""
from datetime import datetime, timezone
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

import psutil

from progress import ROOT, source_hashes

parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
parser.add_argument("--thorough", action="store_true", help="Evidence run: enforce every budget, run long-form cases on a quiet machine, regenerate progress")
parser.add_argument("--date", default=datetime.now(timezone.utc).strftime("%Y-%m-%d"), help="Verification date; pass the user's local date when different from UTC")
parser.add_argument("--decode-device", type=int, help="Explicit local CUDA device ordinal for real hardware-decode acceptance (required by --thorough; quick skips those fixtures without it)")
parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2), help="Concurrent correctness fixtures")
parser.add_argument("--only", help="Quick: comma-separated fixture names to run (Rust lint/tests are skipped; run cargo test separately)")
parser.add_argument("--last-failed", action="store_true", help="Quick: rerun the fixtures that failed in the previous quick run")
parser.add_argument("--fail-fast", action="store_true", help="Start no further fixtures after the first failure")
parser.add_argument("--strict", action="store_true", help="Quick: fail instead of skipping fixtures whose external runtime is not configured")
args = parser.parse_args()
datetime.strptime(args.date, "%Y-%m-%d")
MODE = "thorough" if args.thorough else "quick"
if args.thorough and (args.only or args.last_failed):
    raise SystemExit("--thorough always runs every fixture; --only and --last-failed are quick-mode options")
started_at = time.monotonic()
history_path = ROOT / "verification" / "last-run.json"
try:
    history = json.loads(history_path.read_text(encoding="utf-8"))
except (OSError, ValueError):
    history = {}

# External acceptance dependencies. The thorough run requires every one; the quick check skips the
# fixtures that need a missing one (unless --strict) and says so.
EXTERNAL = {
    "speech": ("CUTBOLT_TRANSCRIPTION_RUNTIME", "the explicit external speech runtime JSON. Missing optional runtime evidence cannot be skipped for coverage."),
    "otio": ("CUTBOLT_OTIO_PYTHON", "an explicit external Python with OpenTimelineIO 0.18.1. Reference-library interchange checks cannot be skipped for coverage."),
    "legacy": ("CUTBOLT_LEGACY_STORE_ENGINE", "a retained project-owned version-1 store writer. Real migration acceptance cannot be skipped."),
    "native": ("CUTBOLT_NATIVE_PROJECT_FIXTURE", "the reviewed external native application fixture. Native capture and exact-build evidence cannot be replaced by synthetic transfer JSON."),
    "segmentation": ("CUTBOLT_SEGMENTATION_PYTHON", "the explicit external segmentation runtime. Foreground-mask evidence cannot be skipped for coverage."),
}
available = {}
for key, (variable, description) in EXTERNAL.items():
    value = os.environ.get(variable)
    available[key] = value if value and Path(value).is_file() else None
    if args.thorough and not available[key]:
        raise SystemExit(f"Full verification requires {variable} pointing to {description}")
speech_runtime, otio_python, legacy_store_engine, native_fixture, segmentation_python = (available[k] for k in ("speech", "otio", "legacy", "native", "segmentation"))
if args.decode_device is not None and not 0 <= args.decode_device < 31:
    raise SystemExit("Hardware-decode acceptance requires a CUDA device ordinal 0..30; the bounded unavailable-device fixture reserves ordinal 31.")
if args.thorough and args.decode_device is None:
    raise SystemExit("Full verification requires --decode-device; there is no software substitution for hardware-decode acceptance.")
available["device"] = None if args.decode_device is None else str(args.decode_device)
# Tools on PATH, checked in seconds rather than discovered by a failing fixture an hour later.
TOOLS = {"ffmpeg": None, "ffprobe": None, "cargo": None, "pwsh": "speech"}
for tool, needed_by in TOOLS.items():
    if shutil.which(tool):
        continue
    if needed_by is None or args.thorough:
        raise SystemExit(f"Verification requires `{tool}` on PATH" + (" (PowerShell 7 generates the speech fixtures)" if tool == "pwsh" else ""))
    available[needed_by] = None


def command(args):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if result.returncode:
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        raise SystemExit(result.returncode)
    return result.stdout


starting_hashes = source_hashes()
targeted = bool(args.only or args.last_failed)
if not targeted:
    command([sys.executable, "tests/repository.py"])
    command(["cargo", "fmt", "--check"])
    command(["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"])
    rust = command(["cargo", "test", "--locked"])
    tests = ["rust:" + match for match in re.findall(r"^test (\S+) \.\.\. ok$", rust, re.MULTILINE)]
    tests = [name for name in tests if name not in {"rust:store::tests::crash_worker", "rust:store::integrity::tests::crash_worker", "rust:cache_store::tests::crash_child", "rust:jobs::recovery::tests::crash_child"}]
    if not tests:
        raise SystemExit("No Rust test evidence was collected")
command(["cargo", "build", "--locked"])
# Fixtures run a private copy of the engine, so target/debug stays free to rebuild during verification.
run_directory = Path(tempfile.mkdtemp(prefix="cutbolt-verify-run-"))
engine_copy = run_directory / ("cutbolt.exe" if os.name == "nt" else "cutbolt")
shutil.copy2(ROOT / "target" / "debug" / engine_copy.name, engine_copy)
print(f"{MODE} verification of {engine_copy.name} copied to {run_directory}", flush=True)

# Fixture scheduling. Correctness fixtures share the machine in a memory-aware pool. In the thorough run,
# fixtures that assert wall-clock or memory budgets, or report throughput, then run in one sequential
# lane; the long-form 4K render, whose budget leaves a wide margin, runs beside that lane rather than
# beside the pool. The registry edit-latency budget and the sustained real-time capture run last on a
# quiet machine: beside other work the 5-second edit budget is marginal, and a delayed capture packet is
# (correctly) rejected as a discontinuity. The quick check runs everything except the capture in the
# pool, records budgets instead of enforcing them and keeps a short capture for the quiet phase.
PY = [sys.executable, "-X", "utf8"]


def fixture(name, *extra, lane="pool", quick_lane="pool", result="verification.json", check=None, utf8=True, long=(), quick=(), needs=()):
    script = (PY if utf8 else [sys.executable]) + [f"tests/{name}.py"]
    output = "{dir}" if result == "verification.json" else "{dir}/" + result.split("/")[0]
    mode_args = list(long) if args.thorough else list(quick)
    return {"name": name, "lane": lane if args.thorough else quick_lane, "commands": [script + ["--output", output, *extra, *mode_args]],
            "results": {name: result}, "check": check if args.thorough else None, "needs": list(needs)}


def value_at(value, field):
    for key in field.split("."):
        value = value[key]
    return value


def require(field, message):
    return lambda values: None if all(value_at(v, field) for v in values.values()) else message


device = available["device"] or ""
STAGES = [
    {"name": "integration", "lane": "pool", "check": None, "needs": [],
     "commands": [[sys.executable, "tests/integration.py", "--output", "{dir}"], [sys.executable, "tests/agents.py", "--fixture", "{dir}"]],
     "results": {"integration": "verification.json", "agents": "agents/verification.json"}},
    fixture("native_timing", long=["--long-form"], check=require("long_form", "Full timing verification requires the complete long-form fractional render")),
    fixture("image_sequences", long=["--long-form"], check=require("long_form", "Full lossless alpha acceptance requires the complete long-form render")),
    fixture("queue_recovery"),
    fixture("interchange", "--reference-python", otio_python or "", needs=["otio"]),
    fixture("native_projects", "--fixture", native_fixture or "", needs=["native"]),
    fixture("portable_projects", "--legacy-engine", legacy_store_engine or "", needs=["legacy"]),
    fixture("render_failures"),
    fixture("delivery_profiles", "--device", device, needs=["device"]),
    fixture("export_formats"),
    fixture("sessions", result="sessions/verification.json", utf8=False),
    fixture("scenes"), fixture("animation"), fixture("compositing"), fixture("easing"), fixture("editing"), fixture("audio"),
    fixture("audio_processing"), fixture("conform"), fixture("proxies"), fixture("timeline_edges"), fixture("graphics"),
    fixture("templates"), fixture("captions"), fixture("grading"), fixture("selection"), fixture("keying"), fixture("delivery"),
    fixture("color"), fixture("luts_scopes"), fixture("hdr"), fixture("tracks"), fixture("transitions"), fixture("track_edits"),
    fixture("sequences"), fixture("multicam"), fixture("synchronization"), fixture("spatial"), fixture("tracking"),
    fixture("stabilization"), fixture("overlays", result="run/verification.json"), fixture("large_imports", result="run/verification.json"),
    fixture("transcripts"), fixture("transcription", "--runtime", speech_runtime or "", needs=["speech"]), fixture("remapping"),
    fixture("recording", lane="quiet", quick_lane="quiet", quick=["--native-seconds", "30"],
            check=require("sustained.long_gate_passed", "Full recording verification requires the sustained native capture gate")),
    fixture("long_form_4k", lane="long", long=["--long-form"],
            check=require("long_gate_passed", "Full performance acceptance requires every frame/sample of the 30-minute moving 4K fixture")),
    fixture("long_form_stress", lane="gated"), fixture("expressions", lane="gated"), fixture("temporal", lane="gated"),
    fixture("geometry", lane="gated"), fixture("segmentation", "--runtime-python", segmentation_python or "", lane="gated", needs=["segmentation"]),
    fixture("registry", lane="quiet"), fixture("acceleration", "--device", device, lane="gated", needs=["device"]),
    fixture("unicode_text", lane="gated"), fixture("reframing", lane="gated"), fixture("cache_previews", lane="gated"),
    fixture("audio_routing", lane="gated"), fixture("audio_repair", lane="gated"),
    fixture("native_scenes", lane="gated", result="run/verification.json"),
    fixture("agent_ergonomics", lane="gated", result="run/verification.json"),
]
assert len({s["name"] for s in STAGES}) == len(STAGES)
NAMES = [s["name"] for s in STAGES]
previous = history.get(MODE, {}).get("stages", {})
if args.only:
    wanted = [n.strip() for n in args.only.split(",") if n.strip()]
    unknown = sorted(set(wanted) - set(NAMES))
    if unknown:
        raise SystemExit(f"Unknown fixtures: {', '.join(unknown)}. Choose from: {', '.join(NAMES)}")
    STAGES = [s for s in STAGES if s["name"] in wanted]
elif args.last_failed:
    failed_before = [name for name, stage in previous.items() if stage.get("ok") is False]
    if not failed_before:
        raise SystemExit("No fixture failed in the previous quick run")
    STAGES = [s for s in STAGES if s["name"] in failed_before]
skipped = {}
for stage in list(STAGES):
    missing = [need for need in stage["needs"] if not available[need]]
    if missing:
        reason = "needs " + ", ".join(EXTERNAL[m][0] if m in EXTERNAL else "--decode-device" if m == "device" else m for m in missing)
        if args.strict:
            raise SystemExit(f"{stage['name']} {reason} (--strict)")
        skipped[stage["name"]] = reason
        STAGES.remove(stage)
lock = threading.Lock()
outcomes = {}
stop_starting = threading.Event()
fixture_environment = {**os.environ, "CUTBOLT_EXE": str(engine_copy)}
if not args.thorough:
    fixture_environment["CUTBOLT_BUDGETS"] = "record"


def log(message):
    elapsed = int(time.monotonic() - started_at)
    with lock:
        print(f"[{elapsed // 60:3d}:{elapsed % 60:02d}] {message}", flush=True)


def execute(stage):
    directory = Path(tempfile.mkdtemp(prefix=f"cutbolt-{stage['name'].replace('_', '-')}-"))
    began = time.monotonic()
    outcome = {"ok": False, "budget_misses": []}
    try:
        for argv in stage["commands"]:
            result = subprocess.run([a.replace("{dir}", str(directory)) for a in argv], cwd=ROOT, capture_output=True,
                                    text=True, encoding="utf-8", errors="replace", env=fixture_environment)
            outcome["budget_misses"] += [line.split(" ", 1)[1] for line in result.stderr.splitlines() if line.startswith("CUTBOLT_BUDGET_MISS ")]
            if result.returncode:
                outcome["log"] = f"exit {result.returncode}\n{result.stdout[-4000:]}\n{result.stderr[-8000:]}"
                return
        values = {key: json.loads((directory / path).read_text(encoding="utf-8")) for key, path in stage["results"].items()}
        problem = stage["check"](values) if stage["check"] else None
        if problem:
            outcome["log"] = problem
            return
        outcome.update(ok=True, values=values)
    except Exception as error:  # Report harness problems with the other failures.
        outcome["log"] = f"{type(error).__name__}: {error}"
    finally:
        outcome["seconds"] = round(time.monotonic() - began, 3)
        shutil.rmtree(directory, ignore_errors=True)
        with lock:
            outcomes[stage["name"]] = outcome
        note = f" ({len(outcome['budget_misses'])} budget(s) recorded over)" if outcome["budget_misses"] else ""
        log(f"{stage['name']} {'passed' if outcome['ok'] else 'FAILED'} in {outcome['seconds']:.0f} s{note}")
        if not outcome["ok"]:
            if args.fail_fast:
                stop_starting.set()
            with lock:
                print(f"----- {stage['name']} output -----\n{outcome['log']}\n-----", flush=True)


def run_phase(lanes):
    """Run lanes concurrently; each lane starts its stages in order up to its own concurrency limit."""
    queues = [(list(stages), limit, memory_aware) for stages, limit, memory_aware in lanes]
    running = [[] for _ in queues]
    while (any(queue for queue, _, _ in queues) and not stop_starting.is_set()) or any(t.is_alive() for active in running for t in active):
        for (queue, limit, memory_aware), active in zip(queues, running):
            active[:] = [t for t in active if t.is_alive()]
            while queue and len(active) < limit and not stop_starting.is_set():
                if memory_aware and active and psutil.virtual_memory().available < 6 * 1024**3:
                    break
                stage = queue.pop(0)
                log(f"{stage['name']} started")
                thread = threading.Thread(target=execute, args=(stage,), daemon=True)
                thread.start()
                active.append(thread)
        time.sleep(0.5)


def lane(name):
    stages = [s for s in STAGES if s["lane"] == name]
    # Longest first, using the previous run's stage times in this mode when available.
    return sorted(stages, key=lambda s: -previous.get(s["name"], {}).get("seconds", 0)) if name == "pool" else stages


run_phase([(lane("pool"), args.jobs, True)])
run_phase([(lane("long"), 1, False), (lane("gated"), 1, False)])
run_phase([(lane("quiet"), 1, False)])
shutil.rmtree(run_directory, ignore_errors=True)
failures = {name: outcome for name, outcome in outcomes.items() if not outcome["ok"]}
not_run = [s["name"] for s in STAGES if s["name"] not in outcomes]
wall = round(time.monotonic() - started_at, 1)
recorded = {**previous, **{name: {"ok": o["ok"], "seconds": o["seconds"], "budget_misses": o["budget_misses"]} for name, o in outcomes.items()}}
history[MODE] = {"finished": datetime.now(timezone.utc).isoformat(timespec="seconds"), "wall_seconds": wall, "stages": recorded,
                 "last_failed": sorted(failures), "skipped": skipped}
(ROOT / "verification").mkdir(exist_ok=True)
history_path.write_text(json.dumps(history, indent=2) + "\n", encoding="utf-8")
misses = {name: o["budget_misses"] for name, o in outcomes.items() if o["budget_misses"]}
if not args.thorough:
    print(f"\nQuick verification: {len(outcomes) - len(failures)} passed, {len(failures)} failed, {len(skipped)} skipped, "
          f"{len(not_run)} not started, {wall:.0f} s. No evidence was written; run --thorough for progress evidence.")
    for name, reason in skipped.items():
        print(f"  skipped {name}: {reason}")
    for name, lines in misses.items():
        print(f"  budget recorded over in {name}: {len(lines)} ({lines[0][:160]})")
    if not_run:
        print(f"  not started after a failure (--fail-fast): {', '.join(not_run)}")
    if failures:
        raise SystemExit(f"Failed: {', '.join(failures)} (rerun with --last-failed)")
    raise SystemExit(0)
if failures or not_run:
    raise SystemExit(f"{len(failures)} of {len(STAGES)} verification stages failed: {', '.join(failures)}" + (f"; not started: {', '.join(not_run)}" if not_run else ""))
results = {key: value for outcome in outcomes.values() for key, value in outcome["values"].items()}
# The report below refers to each fixture's evidence by its stage name.
globals().update(results)
verification_timing = {"jobs": args.jobs, "fixture_wall_seconds": wall,
                       "stages": {name: outcome["seconds"] for name, outcome in sorted(outcomes.items())}}
if source_hashes() != starting_hashes:
    raise SystemExit("Source changed during verification; rerun against a stable checkout")
report = {
    "date": args.date,
    "baseline": "cutbolt-local-v1",
    "passed": tests + integration["passed"] + sessions["passed"] + agents["passed"] + scenes["passed"] + animation["passed"] + compositing["passed"] + easing["passed"] + registry["passed"] + editing["passed"] + audio["passed"] + audio_processing["passed"] + conform["passed"] + proxies["passed"] + timeline_edges["passed"] + graphics["passed"] + templates["passed"] + captions["passed"] + grading["passed"] + selection["passed"] + keying["passed"] + delivery["passed"] + color["passed"],
    "source_hashes": starting_hashes,
    "environment": {"rust": command(["rustc", "--version"]).strip(),
                    "development_python": agents["development_dependencies"],
                    "ffmpeg": integration["render"]["ffmpeg"], "ffprobe": integration["render"]["ffprobe"]},
    "render_evidence": {"frames": integration["render"]["frames"], "audio_samples_per_channel": integration["render"]["samples"],
                        "verification": "All 750 decoded RGB frames and all 1440000 stereo PCM samples match independent expected source slices"},
    "scene_evidence": {"frames":scenes["scene"]["frames"], "width":1920, "height":1080,
                       "pixel_oracle":scenes["pixel_oracle"], "audio_oracle":scenes["audio_oracle"],
                       "development_dependencies":scenes["development_dependencies"], "companion_integration":"Optional separate retained run; not required by core verification"},
    "animation_evidence": animation,
    "expression_evidence": expressions,
    "temporal_evidence": temporal,
    "geometry_evidence": geometry,
    "segmentation_evidence": segmentation,
    "compositing_evidence": compositing,
    "easing_evidence": easing,
    "registry_evidence": registry,
    "editing_evidence": editing,
    "audio_mix_evidence": audio,
    "audio_processing_evidence": audio_processing,
    "conform_evidence": conform,
    "acceleration_evidence": acceleration,
    "proxy_evidence": proxies,
    "timeline_edges_evidence": timeline_edges,
    "graphics_evidence": graphics,
    "template_evidence": templates,
    "caption_evidence": captions,
    "grading_evidence": grading,
    "selection_evidence": selection,
    "keying_evidence": keying,
    "delivery_evidence": delivery,
    "color_evidence": color,
    "luts_scopes_evidence": luts_scopes,
    "hdr_evidence": hdr,
    "tracks_evidence": tracks,
    "transition_evidence": transitions,
    "track_edit_evidence": track_edits,
    "sequence_evidence": sequences,
    "multicam_evidence": multicam,
    "synchronization_evidence": synchronization,
    "spatial_evidence": spatial,
    "tracking_evidence": tracking,
    "stabilization_evidence": stabilization,
    "remapping_evidence": remapping,
    "audio_routing_evidence": audio_routing,
    "audio_repair_evidence": audio_repair,
    "recording_evidence": recording,
    "unicode_text_evidence": unicode_text,
    "reframing_evidence": reframing,
    "transcript_editing_evidence": transcripts,
    "transcription_evidence": transcription,
    "cache_preview_evidence": cache_previews,
    "queue_recovery_evidence": queue_recovery,
    "interchange_evidence": interchange,
    "native_project_evidence": native_projects,
    "portable_project_evidence": portable_projects,
    "render_failure_evidence": render_failures,
    "delivery_profiles_evidence": delivery_profiles,
    "native_timing_evidence": native_timing,
    "export_formats_evidence": export_formats,
    "image_sequence_evidence": image_sequences,
    "long_form_4k_evidence": long_form_4k,
    "long_form_stress_evidence": long_form_stress,
    "verification_timing": verification_timing,
    "native_scene_evidence": native_scenes,
    "overlay_evidence": overlays,
    "large_import_evidence": large_imports,
    "agent_ergonomics_evidence": agent_ergonomics,
}
report["passed"] += cache_previews["passed"]
report["passed"] += expressions["passed"]
report["passed"] += temporal["passed"]
report["passed"] += geometry["passed"]
report["passed"] += segmentation["passed"]
report["passed"] += acceleration["passed"]
report["passed"] += queue_recovery["passed"]
report["passed"] += interchange["passed"]
report["passed"] += native_projects["passed"]
report["passed"] += portable_projects["passed"]
report["passed"] += render_failures["passed"]
report["passed"] += delivery_profiles["passed"]
report["passed"] += native_timing["passed"]
report["passed"] += export_formats["passed"]
report["passed"] += image_sequences["passed"]
report["passed"] += long_form_4k["passed"]
report["passed"] += long_form_stress["passed"]
report["passed"] += luts_scopes["passed"]
report["passed"] += transcripts["passed"]
report["passed"] += transcription["passed"]
report["passed"] += hdr["passed"]
report["passed"] += tracks["passed"]
report["passed"] += transitions["passed"]
report["passed"] += track_edits["passed"]
report["passed"] += sequences["passed"]
report["passed"] += multicam["passed"]
report["passed"] += synchronization["passed"]
report["passed"] += spatial["passed"]
report["passed"] += tracking["passed"]
report["passed"] += stabilization["passed"]
report["passed"] += remapping["passed"]
report["passed"] += audio_routing["passed"]
report["passed"] += audio_repair["passed"]
report["passed"] += recording["passed"]
report["passed"] += unicode_text["passed"]
report["passed"] += reframing["passed"]
report["passed"] += native_scenes["passed"]
report["passed"] += overlays["passed"]
report["passed"] += large_imports["passed"]
report["passed"] += agent_ergonomics["passed"]
(ROOT / "verification").mkdir(exist_ok=True)
(ROOT / "verification/latest.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
print(command([sys.executable, "tools/progress.py"]).strip())
command([sys.executable, "tools/check_repo.py"])
print(f"Passed {len(luts_scopes['passed'])} LUT/scope checks.")
print(f"Passed {len(hdr['passed'])} high-bit-depth/HDR checks.")
print(f"Passed {len(tracks['passed'])} native-track/linked-edit checks.")
print(f"Passed {len(transitions['passed'])} transition/handle checks.")
print(f"Passed {len(track_edits['passed'])} native boundary-edit checks.")
print(f"Passed {len(sequences['passed'])} nested-sequence checks.")
print(f"Passed {len(tests)} Rust tests, {len(integration['passed'])} render checks, {len(sessions['passed'])} session checks, {len(agents['passed'])} MCP/job checks, {len(scenes['passed'])} scene/preview checks, {len(animation['passed'])} animation checks, {len(compositing['passed'])} compositing/mask checks, {len(easing['passed'])} easing/retiming checks, {len(registry['passed'])} registry/performance checks, {len(editing['passed'])} sequential-edit checks, {len(audio['passed'])} audio-mix checks, {len(audio_processing['passed'])} audio-processing checks, {len(conform['passed'])} media-conform checks, {len(proxies['passed'])} proxy checks, {len(timeline_edges['passed'])} timeline-edge checks, {len(graphics['passed'])} graphics checks, {len(templates['passed'])} template checks, {len(captions['passed'])} caption checks, {len(grading['passed'])} grading checks, {len(selection['passed'])} selective-correction checks, {len(keying['passed'])} keying checks, {len(delivery['passed'])} delivery checks and {len(color['passed'])} SDR-normalization checks; format, lint and repository checks passed.")

print(f"Passed {len(multicam['passed'])} camera-selection checks and {len(synchronization['passed'])} synchronization checks.")

print(f"Passed {len(spatial['passed'])} spatial-transform checks.")

print(f"Passed {len(tracking['passed'])} motion-tracking and mask-feather checks.")

print(f"Passed {len(stabilization['passed'])} stabilization checks.")

print(f"Passed {len(remapping['passed'])} variable-speed remapping checks.")

print(f"Passed {len(audio_routing['passed'])} audio routing checks.")
print(f"Passed {len(audio_repair['passed'])} dialogue repair checks.")
print(f"Passed {len(recording['passed'])} recording checks, including sustained native capture.")
print(f"Passed {len(unicode_text['passed'])} Unicode layout and rendered-integration checks.")
print(f"Passed {len(reframing['passed'])} subject-reframing checks.")
print(f"Passed {len(native_scenes['passed'])} native-resolution scene/tilemap checks and {len(overlays['passed'])} alpha overlay-track checks.")
print(f"Passed {len(large_imports['passed'])} large-import checks and {len(agent_ergonomics['passed'])} agent-ergonomics checks.")
print(f"Passed {len(transcripts['passed'])} transcript-editing checks; recognition acceptance is separate.")
print(f"Passed {len(transcription['passed'])} native offline recognition, correction and lifetime checks.")
print(f"Passed {len(cache_previews['passed'])} cache, contact-sheet and cold/warm latency checks.")
print(f"Passed {len(acceleration['passed'])} actual hardware decode, fallback, numerical and throughput checks.")
print(f"Passed {len(queue_recovery['passed'])} real-media queue retry, interruption and capacity checks.")

print(f"Passed {len(interchange['passed'])} public-reference interchange, loss-report and decoded round-trip checks.")

print(f"Passed {len(portable_projects['passed'])} actual migration, portable-media and complete-history recovery checks.")
print(f"Passed {len(render_failures['passed'])} real encoding with injected write faults, changed sources and actual worker termination.")
print(f"Passed {len(delivery_profiles['passed'])} delivery rate-control, quality, actual device compatibility and failure checks.")
print(f"Passed {len(native_timing['passed'])} native-rate, VFR conversion and complete long-form synchronization checks.")
print(f"Passed {len(export_formats['passed'])} native-rate lossless containers, large dimensions and complete numbered-image checks.")
print(f"Passed {len(image_sequences['passed'])} complete numbered footage, lossless color/alpha and long-form clock checks.")

print(f"Passed {len(long_form_4k['passed'])+len(long_form_stress['passed'])} complete long-form 4K, cancellation, write-fault and performance-gate checks.")

print(f"Passed {len(native_projects['passed'])} bound native-capture, exact-build, loss and editorial output checks.")
print(f"Passed {len(expressions['passed'])} typed expression, linked rendering, reproducibility and bounded-work checks.")
print(f"Passed {len(temporal['passed'])} exact shutter, subframe, analytic exposure, effect and bounded-work checks.")

print(f"Passed {len(geometry['passed'])} independent camera, depth, clipping, hierarchy, lighting and geometry-limit checks.")

print(f"Passed {len(segmentation['passed'])} local foreground, correction, edge/temporal quality, model identity and native-mask checks.")
