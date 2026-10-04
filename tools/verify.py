"""One local verification command; generates evidence and the progress document."""
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

parser = argparse.ArgumentParser()
parser.add_argument("--date", default=datetime.now(timezone.utc).strftime("%Y-%m-%d"), help="Verification date; pass the user's local date when different from UTC")
parser.add_argument("--decode-device", type=int, required=True, help="Explicit local CUDA device ordinal for real hardware-decode acceptance; no software substitution")
parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2), help="Concurrent correctness fixtures; budget-gated fixtures always run in their own lane")
args = parser.parse_args()
datetime.strptime(args.date, "%Y-%m-%d")
if not 0 <= args.decode_device < 31:
    raise SystemExit("Full verification requires a CUDA device ordinal 0..30; the bounded unavailable-device fixture reserves ordinal 31.")
speech_runtime = os.environ.get("CUTBOLT_TRANSCRIPTION_RUNTIME")
if not speech_runtime or not Path(speech_runtime).is_file():
    raise SystemExit("Full verification requires CUTBOLT_TRANSCRIPTION_RUNTIME pointing to the explicit external speech runtime JSON. Missing optional runtime evidence cannot be skipped for coverage.")

otio_python = os.environ.get("CUTBOLT_OTIO_PYTHON")
if not otio_python or not Path(otio_python).is_file():
    raise SystemExit("Full verification requires CUTBOLT_OTIO_PYTHON pointing to an explicit external Python with OpenTimelineIO 0.18.1. Reference-library interchange checks cannot be skipped for coverage.")

legacy_store_engine = os.environ.get("CUTBOLT_LEGACY_STORE_ENGINE")
if not legacy_store_engine or not Path(legacy_store_engine).is_file():
    raise SystemExit("Full verification requires CUTBOLT_LEGACY_STORE_ENGINE pointing to a retained project-owned version-1 store writer. Real migration acceptance cannot be skipped.")

native_fixture = os.environ.get("CUTBOLT_NATIVE_PROJECT_FIXTURE")
if not native_fixture or not Path(native_fixture).is_file():
    raise SystemExit("Full verification requires CUTBOLT_NATIVE_PROJECT_FIXTURE pointing to the reviewed external native application fixture. Native capture and exact-build evidence cannot be replaced by synthetic transfer JSON.")

segmentation_python = os.environ.get("CUTBOLT_SEGMENTATION_PYTHON")
if not segmentation_python or not Path(segmentation_python).is_file():
    raise SystemExit("Full verification requires CUTBOLT_SEGMENTATION_PYTHON pointing to the explicit external segmentation runtime. Foreground-mask evidence cannot be skipped for coverage.")

def command(args):
    result = subprocess.run(args, cwd=ROOT, capture_output=True, text=True)
    if result.returncode:
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        raise SystemExit(result.returncode)
    return result.stdout


starting_hashes = source_hashes()
command([sys.executable, "tests/repository.py"])
command(["cargo", "fmt", "--check"])
command(["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"])
rust = command(["cargo", "test", "--locked"])
tests = ["rust:" + match for match in re.findall(r"^test (\S+) \.\.\. ok$", rust, re.MULTILINE)]
tests = [name for name in tests if name not in {"rust:store::tests::crash_worker", "rust:store::integrity::tests::crash_worker", "rust:cache_store::tests::crash_child", "rust:jobs::recovery::tests::crash_child"}]
if not tests:
    raise SystemExit("No Rust test evidence was collected")
command(["cargo", "build", "--locked"])

# Fixture scheduling. Correctness fixtures share the machine in a memory-aware pool. Fixtures that
# assert wall-clock or memory budgets, or report throughput, run afterwards in one sequential lane;
# the long-form 4K render, whose budget leaves a wide margin, runs beside that lane rather than beside
# the pool. The registry edit-latency budget and the sustained real-time capture run last on a quiet
# machine: beside other work the 5-second edit budget is marginal, and a delayed capture packet is
# (correctly) rejected as a discontinuity. Every failure is reported as soon as it happens.
PY = [sys.executable, "-X", "utf8"]


def fixture(name, *extra, lane="pool", result="verification.json", check=None, utf8=True):
    script = (PY if utf8 else [sys.executable]) + [f"tests/{name}.py"]
    output = "{dir}" if result == "verification.json" else "{dir}/" + result.split("/")[0]
    return {"name": name, "lane": lane, "commands": [script + ["--output", output, *extra]], "results": {name: result}, "check": check}


def value_at(value, field):
    for key in field.split("."):
        value = value[key]
    return value


def require(field, message):
    return lambda values: None if all(value_at(v, field) for v in values.values()) else message


STAGES = [
    {"name": "integration", "lane": "pool", "check": None,
     "commands": [[sys.executable, "tests/integration.py", "--output", "{dir}"], [sys.executable, "tests/agents.py", "--fixture", "{dir}"]],
     "results": {"integration": "verification.json", "agents": "agents/verification.json"}},
    fixture("native_timing", "--long-form", check=require("long_form", "Full timing verification requires the complete long-form fractional render")),
    fixture("image_sequences", "--long-form", check=require("long_form", "Full lossless alpha acceptance requires the complete long-form render")),
    fixture("queue_recovery"),
    fixture("interchange", "--reference-python", otio_python),
    fixture("native_projects", "--fixture", native_fixture),
    fixture("portable_projects", "--legacy-engine", legacy_store_engine),
    fixture("render_failures"),
    fixture("delivery_profiles", "--device", str(args.decode_device)),
    fixture("export_formats"),
    fixture("sessions", result="sessions/verification.json", utf8=False),
    fixture("scenes"), fixture("animation"), fixture("compositing"), fixture("easing"), fixture("editing"), fixture("audio"),
    fixture("audio_processing"), fixture("conform"), fixture("proxies"), fixture("timeline_edges"), fixture("graphics"),
    fixture("templates"), fixture("captions"), fixture("grading"), fixture("selection"), fixture("keying"), fixture("delivery"),
    fixture("color"), fixture("luts_scopes"), fixture("hdr"), fixture("tracks"), fixture("transitions"), fixture("track_edits"),
    fixture("sequences"), fixture("multicam"), fixture("synchronization"), fixture("spatial"), fixture("tracking"),
    fixture("stabilization"), fixture("overlays", result="run/verification.json"), fixture("large_imports", result="run/verification.json"),
    fixture("transcripts"), fixture("transcription", "--runtime", speech_runtime), fixture("remapping"),
    fixture("recording", lane="quiet", check=require("sustained.long_gate_passed", "Full recording verification requires the sustained native capture gate")),
    fixture("long_form_4k", "--long-form", lane="long", check=require("long_gate_passed", "Full performance acceptance requires every frame/sample of the 30-minute moving 4K fixture")),
    fixture("long_form_stress", lane="gated"), fixture("expressions", lane="gated"), fixture("temporal", lane="gated"),
    fixture("geometry", lane="gated"), fixture("segmentation", "--runtime-python", segmentation_python, lane="gated"),
    fixture("registry", lane="quiet"), fixture("acceleration", "--device", str(args.decode_device), lane="gated"),
    fixture("unicode_text", lane="gated"), fixture("reframing", lane="gated"), fixture("cache_previews", lane="gated"),
    fixture("audio_routing", lane="gated"), fixture("audio_repair", lane="gated"),
    fixture("native_scenes", lane="gated", result="run/verification.json"),
    fixture("agent_ergonomics", lane="gated", result="run/verification.json"),
]
assert len({s["name"] for s in STAGES}) == len(STAGES)
try:
    previous = json.loads((ROOT / "verification/latest.json").read_text(encoding="utf-8"))["verification_timing"]["stages"]
except (OSError, KeyError, ValueError):
    previous = {}
started_at = time.monotonic()
lock = threading.Lock()
outcomes = {}


def log(message):
    elapsed = int(time.monotonic() - started_at)
    with lock:
        print(f"[{elapsed // 60:3d}:{elapsed % 60:02d}] {message}", flush=True)


def execute(stage):
    directory = Path(tempfile.mkdtemp(prefix=f"cutbolt-{stage['name'].replace('_', '-')}-"))
    began = time.monotonic()
    outcome = {"ok": False}
    try:
        for argv in stage["commands"]:
            result = subprocess.run([a.replace("{dir}", str(directory)) for a in argv], cwd=ROOT, capture_output=True,
                                    text=True, encoding="utf-8", errors="replace")
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
        log(f"{stage['name']} {'passed' if outcome['ok'] else 'FAILED'} in {outcome['seconds']:.0f} s")
        if not outcome["ok"]:
            with lock:
                print(f"----- {stage['name']} output -----\n{outcome['log']}\n-----", flush=True)


def run_phase(lanes):
    """Run lanes concurrently; each lane starts its stages in order up to its own concurrency limit."""
    queues = [(list(stages), limit, memory_aware) for stages, limit, memory_aware in lanes]
    running = [[] for _ in queues]
    while any(queue for queue, _, _ in queues) or any(t.is_alive() for active in running for t in active):
        for (queue, limit, memory_aware), active in zip(queues, running):
            active[:] = [t for t in active if t.is_alive()]
            while queue and len(active) < limit:
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
    # Longest first, using the previous verified run's stage times when available.
    return sorted(stages, key=lambda s: -previous.get(s["name"], 0)) if name == "pool" else stages


run_phase([(lane("pool"), args.jobs, True)])
run_phase([(lane("long"), 1, False), (lane("gated"), 1, False)])
run_phase([(lane("quiet"), 1, False)])
failures = {name: outcome for name, outcome in outcomes.items() if not outcome["ok"]}
if failures or len(outcomes) != len(STAGES):
    raise SystemExit(f"{len(failures)} of {len(STAGES)} verification stages failed: {', '.join(failures)}")
results = {key: value for outcome in outcomes.values() for key, value in outcome["values"].items()}
# The report below refers to each fixture's evidence by its stage name.
globals().update(results)
verification_timing = {"jobs": args.jobs, "fixture_wall_seconds": round(time.monotonic() - started_at, 3),
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
