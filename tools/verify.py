"""One local verification command; generates evidence and the progress document."""
from datetime import datetime, timezone
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

from progress import ROOT, source_hashes

parser = argparse.ArgumentParser()
parser.add_argument("--date", default=datetime.now(timezone.utc).strftime("%Y-%m-%d"), help="Verification date; pass the user's local date when different from UTC")
parser.add_argument("--decode-device", type=int, required=True, help="Explicit local CUDA device ordinal for real hardware-decode acceptance; no software substitution")
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
with tempfile.TemporaryDirectory(prefix="cutbolt-verify-") as directory:
    command([sys.executable, "tests/integration.py", "--output", directory])
    integration = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
    command([sys.executable, "tests/agents.py", "--fixture", directory])
    agents = json.loads((Path(directory) / "agents/verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-queue-recovery-") as directory:
    command([sys.executable, "-X", "utf8", "tests/queue_recovery.py", "--output", directory])
    queue_recovery = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-interchange-") as directory:
    command([sys.executable, "-X", "utf8", "tests/interchange.py", "--output", directory, "--reference-python", otio_python])
    interchange = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-native-projects-") as directory:
    command([sys.executable, "-X", "utf8", "tests/native_projects.py", "--output", directory, "--fixture", native_fixture])
    native_projects = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-portable-") as directory:
    command([sys.executable, "-X", "utf8", "tests/portable_projects.py", "--output", directory, "--legacy-engine", legacy_store_engine])
    portable_projects = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-render-failures-") as directory:
    command([sys.executable, "-X", "utf8", "tests/render_failures.py", "--output", directory])
    render_failures = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-delivery-profiles-") as directory:
    command([sys.executable, "-X", "utf8", "tests/delivery_profiles.py", "--output", directory, "--device", str(args.decode_device)])
    delivery_profiles = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-native-timing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/native_timing.py", "--output", directory, "--long-form"])
    native_timing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
    if not native_timing["long_form"]:
        raise SystemExit("Full timing verification requires the complete long-form fractional render")
with tempfile.TemporaryDirectory(prefix="cutbolt-export-formats-") as directory:
    command([sys.executable, "-X", "utf8", "tests/export_formats.py", "--output", directory])
    export_formats = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-image-sequences-") as directory:
    command([sys.executable, "-X", "utf8", "tests/image_sequences.py", "--output", directory, "--long-form"])
    image_sequences = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
    if not image_sequences["long_form"]:
        raise SystemExit("Full lossless alpha acceptance requires the complete long-form render")
with tempfile.TemporaryDirectory(prefix="cutbolt-long-form-4k-") as directory:
    command([sys.executable, "-X", "utf8", "tests/long_form_4k.py", "--output", directory, "--long-form"])
    long_form_4k = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
    if not long_form_4k["long_gate_passed"]:
        raise SystemExit("Full performance acceptance requires every frame/sample of the 30-minute moving 4K fixture")
with tempfile.TemporaryDirectory(prefix="cutbolt-long-form-stress-") as directory:
    command([sys.executable, "-X", "utf8", "tests/long_form_stress.py", "--output", directory])
    long_form_stress = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-sessions-") as directory:
    output = Path(directory) / "sessions"
    command([sys.executable, "tests/sessions.py", "--output", str(output)])
    sessions = json.loads((output / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-scenes-") as directory:
    command([sys.executable, "-X", "utf8", "tests/scenes.py", "--output", directory])
    scenes = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-expressions-") as directory:
    command([sys.executable, "-X", "utf8", "tests/expressions.py", "--output", directory])
    expressions = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-temporal-") as directory:
    command([sys.executable, "-X", "utf8", "tests/temporal.py", "--output", directory])
    temporal = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-geometry-") as directory:
    command([sys.executable, "-X", "utf8", "tests/geometry.py", "--output", directory])
    geometry = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-segmentation-") as directory:
    command([sys.executable, "-X", "utf8", "tests/segmentation.py", "--output", directory, "--runtime-python", segmentation_python])
    segmentation = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-animation-") as directory:
    command([sys.executable, "-X", "utf8", "tests/animation.py", "--output", directory])
    animation = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-compositing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/compositing.py", "--output", directory])
    compositing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-easing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/easing.py", "--output", directory])
    easing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-registry-") as directory:
    command([sys.executable, "-X", "utf8", "tests/registry.py", "--output", directory])
    registry = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-editing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/editing.py", "--output", directory])
    editing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-audio-") as directory:
    command([sys.executable, "-X", "utf8", "tests/audio.py", "--output", directory])
    audio = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-audio-processing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/audio_processing.py", "--output", directory])
    audio_processing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-conform-") as directory:
    command([sys.executable, "-X", "utf8", "tests/conform.py", "--output", directory])
    conform = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-acceleration-") as directory:
    command([sys.executable, "-X", "utf8", "tests/acceleration.py", "--output", directory, "--device", str(args.decode_device)])
    acceleration = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-proxies-") as directory:
    command([sys.executable, "-X", "utf8", "tests/proxies.py", "--output", directory])
    proxies = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-timeline-edges-") as directory:
    command([sys.executable, "-X", "utf8", "tests/timeline_edges.py", "--output", directory])
    timeline_edges = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-graphics-") as directory:
    command([sys.executable, "-X", "utf8", "tests/graphics.py", "--output", directory])
    graphics = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-templates-") as directory:
    command([sys.executable, "-X", "utf8", "tests/templates.py", "--output", directory])
    templates = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-unicode-") as directory:
    command([sys.executable, "-X", "utf8", "tests/unicode_text.py", "--output", directory])
    unicode_text = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-captions-") as directory:
    command([sys.executable, "-X", "utf8", "tests/captions.py", "--output", directory])
    captions = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-grading-") as directory:
    command([sys.executable, "-X", "utf8", "tests/grading.py", "--output", directory])
    grading = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-selection-") as directory:
    command([sys.executable, "-X", "utf8", "tests/selection.py", "--output", directory])
    selection = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-keying-") as directory:
    command([sys.executable, "-X", "utf8", "tests/keying.py", "--output", directory])
    keying = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-delivery-") as directory:
    command([sys.executable, "-X", "utf8", "tests/delivery.py", "--output", directory])
    delivery = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-color-") as directory:
    command([sys.executable, "-X", "utf8", "tests/color.py", "--output", directory])
    color = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-luts-scopes-") as directory:
    command([sys.executable, "-X", "utf8", "tests/luts_scopes.py", "--output", directory])
    luts_scopes = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-hdr-") as directory:
    command([sys.executable, "-X", "utf8", "tests/hdr.py", "--output", directory])
    hdr = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-tracks-") as directory:
    command([sys.executable, "-X", "utf8", "tests/tracks.py", "--output", directory])
    tracks = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-transitions-") as directory:
    command([sys.executable, "-X", "utf8", "tests/transitions.py", "--output", directory])
    transitions = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-track-edits-") as directory:
    command([sys.executable, "-X", "utf8", "tests/track_edits.py", "--output", directory])
    track_edits = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-sequences-") as directory:
    command([sys.executable, "-X", "utf8", "tests/sequences.py", "--output", directory])
    sequences = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-multicam-") as directory:
    command([sys.executable, "-X", "utf8", "tests/multicam.py", "--output", directory])
    multicam = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-synchronization-") as directory:
    command([sys.executable, "-X", "utf8", "tests/synchronization.py", "--output", directory])
    synchronization = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-spatial-") as directory:
    command([sys.executable, "-X", "utf8", "tests/spatial.py", "--output", directory])
    spatial = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-tracking-") as directory:
    command([sys.executable, "-X", "utf8", "tests/tracking.py", "--output", directory])
    tracking = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-stabilization-") as directory:
    command([sys.executable, "-X", "utf8", "tests/stabilization.py", "--output", directory])
    stabilization = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-reframing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/reframing.py", "--output", directory])
    reframing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-transcripts-") as directory:
    command([sys.executable, "-X", "utf8", "tests/transcripts.py", "--output", directory])
    transcripts = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-cache-previews-") as directory:
    command([sys.executable, "-X", "utf8", "tests/cache_previews.py", "--output", directory])
    cache_previews = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-transcription-") as directory:
    command([sys.executable, "-X", "utf8", "tests/transcription.py", "--output", directory, "--runtime", speech_runtime])
    transcription = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-remapping-") as directory:
    command([sys.executable, "-X", "utf8", "tests/remapping.py", "--output", directory])
    remapping = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-audio-routing-") as directory:
    command([sys.executable, "-X", "utf8", "tests/audio_routing.py", "--output", directory])
    audio_routing = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-audio-repair-") as directory:
    command([sys.executable, "-X", "utf8", "tests/audio_repair.py", "--output", directory])
    audio_repair = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
with tempfile.TemporaryDirectory(prefix="cutbolt-recording-") as directory:
    command([sys.executable, "-X", "utf8", "tests/recording.py", "--output", directory])
    recording = json.loads((Path(directory) / "verification.json").read_text(encoding="utf-8"))
    if not recording["sustained"]["long_gate_passed"]:
        raise SystemExit("Full recording verification requires the sustained native capture gate")
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
