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
parser.add_argument("--engine", type=Path, help="Quick: verify this prebuilt engine executable instead of building target/debug")
parser.add_argument("--no-rust", action="store_true", help="Quick: skip formatting, lint and Rust tests")
parser.add_argument("--coverage-dir", type=Path, help="Quick: write each fixture's LLVM coverage profiles under this directory (needs an instrumented --engine)")
parser.add_argument("--gate", action="store_true", help="Push gate: run only fixtures the changes can affect (tools/impact.py), reuse unchanged passes, fit a time budget and defer the rest")
parser.add_argument("--budget", type=float, default=85, help="Gate: fixture wall-clock budget in seconds; fixtures still running then are stopped and deferred")
parser.add_argument("--report", type=Path, help="Write a JSON summary of this run (used by tools/ship.py)")
parser.add_argument("--background", action="store_true", help="Deferred verification of a pushed commit (tools/ship.py)")
parser.add_argument("--commit", help="Commit being verified, for the run record")
parser.add_argument("--results-dir", type=Path, help="Directory of run records (default verification/results)")
parser.add_argument("--impact-map", type=Path, help="Impact map to use (default verification/impact-map.json)")
parser.add_argument("--cleanup-worktree", action="store_true", help="Background: remove this git worktree when finished")
parser.add_argument("--status", action="store_true", help="Show the impact map, recent runs and unresolved failures, then exit")
args = parser.parse_args()
import impact  # noqa: E402  (tools/ is on sys.path as the script directory)

if args.impact_map:
    impact.MAP = args.impact_map.resolve()
if args.results_dir:
    impact.RESULTS = args.results_dir.resolve()
if args.status:
    document = impact.load_map()
    summary = "none (python tools/impact.py build)" if not document else f"{document['commit'][:10]}, built {document['built']}, {len(document['covered_files'])} fixtures"
    print(f"Impact map: {summary}")
    for name, data in impact.recent(12):
        problems = data.get("failures") or []
        print(f"  {name[:15]} {data.get('run'):10} {str(data.get('commit') or '')[:10]:10} passed {len(data.get('passes', {}))}"
              + (f", FAILED {', '.join(problems)}" if problems else "") + (f", deferred {len(data.get('deferred', []))}" if data.get("deferred") else ""))
    for stage, where in impact.unresolved_failures().items():
        print(f"  unresolved failure: {stage} ({where})")
    raise SystemExit(0)
datetime.strptime(args.date, "%Y-%m-%d")
MODE = "thorough" if args.thorough else "quick"
if args.thorough and (args.only or args.last_failed or args.engine or args.no_rust or args.coverage_dir or args.gate or args.background):
    raise SystemExit("--thorough always builds the engine and runs every check; the other selection options are quick-mode only")
if args.gate and (args.only or args.last_failed or args.background):
    raise SystemExit("--gate selects fixtures itself")
RUN = "thorough" if args.thorough else "gate" if args.gate else "background" if args.background else "quick"
started_at = time.monotonic()
history_path = impact.STATE / "last-run.json"  # shared by every worktree of this repository
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


def rust_checks():
    command([sys.executable, "tests/repository.py"])
    command(["cargo", "fmt", "--check"])
    command(["cargo", "clippy", "--all-targets", "--locked", "--", "-D", "warnings"])
    rust = command(["cargo", "test", "--locked"])
    found = ["rust:" + match for match in re.findall(r"^test (\S+) \.\.\. ok$", rust, re.MULTILINE)]
    found = [name for name in found if name not in {"rust:store::tests::crash_worker", "rust:store::integrity::tests::crash_worker", "rust:cache_store::tests::crash_child", "rust:jobs::recovery::tests::crash_child"}]
    if not found:
        raise SystemExit("No Rust test evidence was collected")
    return found


run_rust = not (args.only or args.last_failed or args.no_rust or args.background)
rust_outcome = {}
if run_rust and not args.gate:
    tests = rust_checks()
ENGINE_NAME = "cutbolt.exe" if os.name == "nt" else "cutbolt"
if args.background:
    # A fresh worktree has no build artifacts; fixtures that compile helpers against the library or build
    # examples use this shared, incremental build directory (tests/engine.py BUILD).
    os.environ["CARGO_TARGET_DIR"] = str(impact.STATE / "target")
    command(["cargo", "build", "--locked"])
if args.engine:
    source_engine = args.engine.resolve()
else:
    command(["cargo", "build", "--locked"])
    source_engine = ROOT / "target" / "debug" / ENGINE_NAME
# Fixtures run a private copy of the engine, so target/debug stays free to rebuild during verification.
run_directory = Path(tempfile.mkdtemp(prefix="cutbolt-verify-run-"))
engine_copy = run_directory / ENGINE_NAME
shutil.copy2(source_engine, engine_copy)
rust_thread = None
if run_rust and args.gate:
    def gate_rust():
        try:
            rust_outcome["tests"] = rust_checks()
        except SystemExit as error:
            rust_outcome["error"] = f"Rust checks failed ({error})"
    # Lint and Rust tests use target/debug; fixtures use the copied engine, so both run at once.
    rust_thread = threading.Thread(target=gate_rust, daemon=True)
    rust_thread.start()
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
# Durations depend on concurrency, so each kind of run predicts from its own history (quick as fallback).
previous = {**history.get("quick", {}).get("stages", {}), **history.get(RUN, {}).get("stages", {})}
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
reused, deferred, not_impacted, selection = [], [], [], {}
if args.gate:
    for stage_name, where in impact.unresolved_failures().items():
        print(f"WARNING: {stage_name} failed in {where} and has not passed since; fix forward", flush=True)
    selection, map_commit = impact.impacted([s["name"] for s in STAGES])
    passes = impact.cached_passes()
    candidates = []
    for stage in STAGES:
        if stage["name"] not in selection:
            not_impacted.append(stage["name"])
        elif (fp := impact.fingerprint(stage["name"], stage["commands"])) and fp in passes.get(stage["name"], ()):
            reused.append(stage["name"])
        else:
            candidates.append(stage)

    def predicted(stage):
        return previous.get(stage["name"], {}).get("seconds") or 60
    pooled = [s for s in candidates if s["lane"] == "pool"]
    loads, fitted = [0.0] * max(1, min(args.jobs, len(pooled))), []
    for stage in sorted(pooled, key=predicted):
        lane_index = loads.index(min(loads))
        if loads[lane_index] + predicted(stage) > args.budget:
            deferred.append(stage["name"])
            continue
        loads[lane_index] += predicted(stage)
        fitted.append(stage)
    # Quiet-phase fixtures run one at a time after the pool, so they only fit in what remains.
    sequential = max(loads)
    for stage in sorted((s for s in candidates if s["lane"] != "pool"), key=predicted):
        if sequential + predicted(stage) > args.budget:
            deferred.append(stage["name"])
            continue
        sequential += predicted(stage)
        fitted.append(stage)
    STAGES = fitted
    print(f"Gate (map {map_commit[:10] if map_commit else 'missing'}): {len(fitted)} to run, {len(reused)} reused, {len(deferred)} deferred, "
          f"{len(not_impacted)} not affected", flush=True)
    for stage in fitted:
        print(f"  run {stage['name']}: {'; '.join(selection[stage['name']][:2])}", flush=True)
fingerprints = {s["name"]: impact.fingerprint(s["name"], s["commands"]) for s in STAGES}
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


deadline_passed = threading.Event()
gate_deadline = None
GATES = impact.STATE / "gates"


def gate_running():
    """Whether any live gate is running; background verification starts nothing new meanwhile."""
    for marker in GATES.glob("*.pid"):
        try:
            if psutil.pid_exists(int(marker.stem)):
                return True
            marker.unlink(missing_ok=True)
        except (ValueError, OSError):
            continue
    return False


if args.gate:
    GATES.mkdir(parents=True, exist_ok=True)
    gate_marker = GATES / f"{os.getpid()}.pid"
    gate_marker.write_text(args.commit or "", encoding="utf-8")


def run_command(argv, environment):
    """Run one fixture command; the gate stops it (and its process tree) at the deadline."""
    process = subprocess.Popen(argv, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                               encoding="utf-8", errors="replace", env=environment)
    while True:
        try:
            out, err = process.communicate(timeout=0.5)
            return process.returncode, out, err
        except subprocess.TimeoutExpired:
            if deadline_passed.is_set():
                try:
                    tree = psutil.Process(process.pid).children(recursive=True)
                except psutil.Error:
                    tree = []
                for child in [*tree, process]:
                    try:
                        child.kill()
                    except (psutil.Error, OSError):
                        pass
                out, err = process.communicate()
                return None, out, err


def execute(stage):
    directory = Path(tempfile.mkdtemp(prefix=f"cutbolt-{stage['name'].replace('_', '-')}-"))
    began = time.monotonic()
    outcome = {"ok": False, "budget_misses": []}
    try:
        environment = fixture_environment
        if args.coverage_dir:
            profiles = args.coverage_dir / stage["name"]
            profiles.mkdir(parents=True, exist_ok=True)
            # %8m: processes merge online into eight pool files instead of one large file each.
            environment = {**fixture_environment, "LLVM_PROFILE_FILE": str(profiles / "%8m.profraw")}
        for argv in stage["commands"]:
            code, out, err = run_command([a.replace("{dir}", str(directory)) for a in argv], environment)
            outcome["budget_misses"] += [line.split(" ", 1)[1] for line in err.splitlines() if line.startswith("CUTBOLT_BUDGET_MISS ")]
            if code is None:
                outcome.update(deferred=True, log="stopped at the gate deadline; deferred")
                return
            if code:
                outcome["log"] = f"exit {code}\n{out[-4000:]}\n{err[-8000:]}"
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
        status = "passed" if outcome["ok"] else "deferred at the deadline" if outcome.get("deferred") else "FAILED"
        log(f"{stage['name']} {status} in {outcome['seconds']:.0f} s{note}")
        if not outcome["ok"] and not outcome.get("deferred"):
            if args.fail_fast:
                stop_starting.set()
            with lock:
                print(f"----- {stage['name']} output -----\n{outcome['log']}\n-----", flush=True)


def run_phase(lanes):
    """Run lanes concurrently; each lane starts its stages in order up to its own concurrency limit."""
    queues = [(list(stages), limit, memory_aware) for stages, limit, memory_aware in lanes]
    running = [[] for _ in queues]
    while (any(queue for queue, _, _ in queues) and not stop_starting.is_set()) or any(t.is_alive() for active in running for t in active):
        if gate_deadline is not None and time.monotonic() > gate_deadline and not deadline_passed.is_set():
            deadline_passed.set()
            stop_starting.set()
            log("gate deadline reached; running fixtures stop and are deferred")
        for (queue, limit, memory_aware), active in zip(queues, running):
            active[:] = [t for t in active if t.is_alive()]
            while queue and len(active) < limit and not stop_starting.is_set():
                if memory_aware and active and psutil.virtual_memory().available < 6 * 1024**3:
                    break
                if args.background and gate_running():
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


if args.gate:
    gate_deadline = time.monotonic() + args.budget
run_phase([(lane("pool"), args.jobs, True)])
run_phase([(lane("long"), 1, False), (lane("gated"), 1, False)])
run_phase([(lane("quiet"), 1, False)])
if rust_thread:
    rust_thread.join()
if args.gate:
    gate_marker.unlink(missing_ok=True)
shutil.rmtree(run_directory, ignore_errors=True)
failures = {name: outcome for name, outcome in outcomes.items() if not outcome["ok"] and not outcome.get("deferred")}
deferred += [name for name, outcome in outcomes.items() if outcome.get("deferred")]
not_run = [s["name"] for s in STAGES if s["name"] not in outcomes]
if deadline_passed.is_set():  # Never started because the gate's time ran out: the background runs them.
    deferred += not_run
    not_run = []
if rust_outcome.get("error"):
    failures["rust"] = {"ok": False, "log": rust_outcome["error"], "seconds": 0, "budget_misses": []}
commit = args.commit or subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
passed_fingerprints = {name: fingerprints[name] for name, o in outcomes.items() if o["ok"] and fingerprints.get(name)}
if run_rust and "rust" not in failures:
    passed_fingerprints["rust"] = "passed"
impact.record(RUN, passed_fingerprints,
              commit=commit, failures=sorted(failures), deferred=deferred, reused=reused,
              not_impacted=len(not_impacted), skipped=sorted(skipped))
if args.report:
    args.report.write_text(json.dumps({"ok": not failures and not not_run, "run": RUN,
                                       "ran": sorted(outcomes), "failed": sorted(failures), "deferred": deferred, "reused": reused,
                                       "not_impacted": not_impacted, "skipped": skipped}, indent=1), encoding="utf-8")
if args.cleanup_worktree:
    common = Path(subprocess.run(["git", "rev-parse", "--git-common-dir"], cwd=ROOT, capture_output=True, text=True).stdout.strip())
    main_checkout = (ROOT / common).resolve().parent
    os.chdir(tempfile.gettempdir())
    subprocess.run(["git", "-C", str(main_checkout), "worktree", "remove", "--force", str(ROOT)], capture_output=True)
wall = round(time.monotonic() - started_at, 1)
recorded = {**history.get(RUN, {}).get("stages", {}),
            **{name: {"ok": o["ok"], "seconds": o["seconds"], "budget_misses": o["budget_misses"]} for name, o in outcomes.items() if not o.get("deferred")}}
history[RUN] = {"finished": datetime.now(timezone.utc).isoformat(timespec="seconds"), "wall_seconds": wall, "stages": recorded,
                 "last_failed": sorted(failures), "skipped": skipped}
history_path.parent.mkdir(parents=True, exist_ok=True)
if not args.coverage_dir:  # Instrumented timings would mislead later scheduling and gate budgets.
    history_path.write_text(json.dumps(history, indent=2) + "\n", encoding="utf-8")
misses = {name: o["budget_misses"] for name, o in outcomes.items() if o["budget_misses"]}
if not args.thorough:
    print(f"\n{RUN.capitalize()} verification: {len([o for o in outcomes.values() if o['ok']])} passed, {len(failures)} failed, {len(skipped)} skipped, "
          f"{len(not_run)} not started, {wall:.0f} s. No evidence was written; run --thorough for progress evidence.")
    if args.gate:
        print(f"  reused {len(reused)} unchanged passes; {len(not_impacted)} fixtures not affected; deferred to background: {', '.join(deferred) or 'none'}")
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
