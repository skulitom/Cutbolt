"""Test impact analysis for tools/verify.py.

`build` runs every quick fixture against a coverage-instrumented engine and records which Rust
functions each fixture executes (verification/impact-map.json, per machine). `show` lists the fixtures
that the changes since the map's commit can affect. The verifier's gate uses `impacted()` to run only
those fixtures, and `fingerprint()` to reuse a pass whose inputs are unchanged.

Selection is conservative: a change outside any recorded function falls back to every fixture that
executed code in that file; fixtures without coverage data, build files and unknown paths select
everything they could reach. Only plain `//` comments and `mod`/`use` lines are ignored; `///` doc
comments feed the agent schemas and count as changes.
"""
import argparse
import ast
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def _shared_state():
    """Verifier state shared by every worktree of this repository (inside its common git directory)."""
    try:
        common = subprocess.run(["git", "rev-parse", "--git-common-dir"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()
        return (ROOT / common).resolve() / "cutbolt-verify"
    except (OSError, subprocess.CalledProcessError):
        return ROOT / "verification"


STATE = _shared_state()
MAP = STATE / "impact-map.json"
RESULTS = STATE / "results"
ENGINE_NAME = "cutbolt.exe" if os.name == "nt" else "cutbolt"
# Verifier stage name -> fixture scripts it runs (all other stages run tests/<name>.py).
STAGE_SCRIPTS = {"integration": ["integration", "agents"]}
EVERYTHING = {"Cargo.toml", "Cargo.lock", "build.rs", "rust-toolchain", "rust-toolchain.toml", ".cargo/config.toml"}
NO_FIXTURES = ("docs/", "progress/", "verification/", "LICENSE", ".gitignore", ".gitattributes", "tools/")
IGNORABLE = re.compile(r"^\s*(?:$|//(?![/!])|(?:pub(?:\([a-z]+\))?\s+)?(?:mod|use)\s)")


def git(*args):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=True).stdout


def llvm_tool(name):
    sysroot = Path(subprocess.run(["rustc", "--print", "sysroot"], capture_output=True, text=True, check=True).stdout.strip())
    found = sorted(sysroot.glob(f"lib/rustlib/*/bin/{name}*"))
    if not found:
        raise SystemExit(f"{name} not found; install it with `rustup component add llvm-tools-preview`")
    return str(found[0])


def scripts(stage):
    return STAGE_SCRIPTS.get(stage, [stage])


def python_closure(stage):
    """The stage's fixture scripts, the local test modules they import, and other tests/ files they name."""
    seen, pending = set(), list(scripts(stage))
    while pending:
        name = pending.pop()
        path = ROOT / "tests" / f"{name}.py"
        if name in seen or not path.is_file():
            continue
        seen.add(name)
        for node in ast.walk(ast.parse(path.read_text(encoding="utf-8"))):
            if isinstance(node, ast.Import):
                pending += [a.name.split(".")[0] for a in node.names]
            elif isinstance(node, ast.ImportFrom) and node.module and not node.level:
                pending.append(node.module.split(".")[0])
    files = {f"tests/{name}.py" for name in seen}
    text = "".join((ROOT / f).read_text(encoding="utf-8") for f in files)
    for other in (ROOT / "tests").iterdir():
        if other.suffix != ".py" and other.is_file() and other.name in text:
            files.add(f"tests/{other.name}")
    for other in (ROOT / "examples").glob("*"):
        if other.is_file() and (other.stem in text or other.name in text):
            files.add(f"examples/{other.name}")
    return files


# ---------------------------------------------------------------- building the map

def build(jobs, keep=False, device=None, from_profiles=None):
    target = ROOT / "target" / "coverage"
    engine = target / "debug" / ENGINE_NAME
    if from_profiles:
        profiles = from_profiles
        commit = (profiles / "commit.txt").read_text(encoding="utf-8").strip()
        if git("rev-parse", "HEAD").strip() != commit or git("status", "--porcelain", "--", "src").strip():
            raise SystemExit(f"Reprocessing needs the instrumented engine of {commit[:10]}; check out that commit with clean sources")
        return process(profiles, engine, commit, keep)
    if git("status", "--porcelain", "--", "src", "Cargo.toml", "Cargo.lock", "build.rs").strip():
        raise SystemExit("Commit or stash Rust source changes first: the map records line numbers of the commit it is built from")
    commit = git("rev-parse", "HEAD").strip()
    profiles = Path(tempfile.mkdtemp(prefix="cutbolt-coverage-"))
    (profiles / "commit.txt").write_text(commit, encoding="utf-8")
    # Instrumented build scripts and proc-macros also write profiles; keep them out of the checkout.
    # --cfg cutbolt_coverage makes the engine flush counters before process::exit (see src/main.rs).
    env = {**os.environ, "CARGO_TARGET_DIR": str(target), "RUSTFLAGS": "-C instrument-coverage --cfg cutbolt_coverage",
           "LLVM_PROFILE_FILE": str(profiles / "build" / "%p-%m.profraw")}
    subprocess.run(["cargo", "build", "--locked"], cwd=ROOT, env=env, check=True)
    report = profiles / "report.json"
    run = subprocess.run([sys.executable, "-X", "utf8", "tools/verify.py", "--engine", str(engine), "--no-rust", "--report", str(report),
                          "--coverage-dir", str(profiles), "--jobs", str(jobs)] + (["--decode-device", str(device)] if device is not None else []), cwd=ROOT)
    (profiles / "verify-exit.txt").write_text(str(run.returncode), encoding="utf-8")
    process(profiles, engine, commit, keep)


def process(profiles, engine, commit, keep):
    report = profiles / "report.json"
    summary = json.loads(report.read_text(encoding="utf-8")) if report.exists() else {"ran": [], "failed": []}
    passed = set(summary["ran"]) - set(summary["failed"])
    profdata, cov = llvm_tool("llvm-profdata"), llvm_tool("llvm-cov")
    stages = sorted(p.name for p in profiles.iterdir() if p.is_dir() and p.name != "build")
    files, covered, missing = {}, {}, []
    for index, stage in enumerate(stages):
        raw = sorted((profiles / stage).glob("*.profraw"))
        if not raw or stage not in passed:
            missing.append(stage)
            continue
        merged, listing = profiles / f"{stage}.profdata", profiles / f"{stage}.inputs"
        listing.write_text("\n".join(map(str, raw)) + "\n", encoding="utf-8")  # too many for one command line
        subprocess.run([profdata, "merge", "-sparse", "-f", str(listing), "-o", str(merged)], check=True)
        export = subprocess.run([cov, "export", "-format=text", "-skip-expansions", f"-instr-profile={merged}",
                                 "-ignore-filename-regex=(\\.cargo|\\.rustup|rustc|registry)", str(engine)],
                                capture_output=True, text=True, encoding="utf-8", check=True).stdout
        covered[stage] = set()
        for function in json.loads(export)["data"][0]["functions"]:
            if not function["count"]:
                continue
            path = Path(function["filenames"][0])
            try:
                relative = path.resolve().relative_to(ROOT).as_posix()
            except ValueError:
                continue
            lines = [r[0] for r in function["regions"] if r[5] == 0] + [r[2] for r in function["regions"] if r[5] == 0]
            if not lines:
                continue
            key = (min(lines), max(lines))
            files.setdefault(relative, {}).setdefault(key, set()).add(stage)
            covered[stage].add(relative)
    if not keep:
        shutil.rmtree(profiles, ignore_errors=True)
    document = {"commit": commit, "built": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "stages": stages, "missing": missing,
                "files": {f: [[s, e, sorted(st)] for (s, e), st in sorted(spans.items())] for f, spans in sorted(files.items())},
                "covered_files": {s: sorted(f) for s, f in covered.items()}}
    MAP.parent.mkdir(parents=True, exist_ok=True)
    MAP.write_text(json.dumps(document) + "\n", encoding="utf-8")
    print(f"Impact map for {commit[:10]}: {len(covered)} fixtures with coverage, {len(missing)} without ({', '.join(missing) or 'none'}), "
          f"{sum(len(v) for v in files.values())} executed functions in {len(files)} files")


# ---------------------------------------------------------------- selecting fixtures

_maps = {}


def load_map():
    """The impact map (read once per process), or None when it has not been built."""
    if MAP not in _maps:
        try:
            _maps[MAP] = json.loads(MAP.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            _maps[MAP] = None
    return _maps[MAP]


def changed_files(base):
    paths = set(git("diff", "--name-only", base).split())
    paths |= set(git("ls-files", "--others", "--exclude-standard").split())
    return sorted(paths)


def hunks(base, path):
    """Old-side line ranges of each hunk with its changed lines, for diffs against the map's commit."""
    diff = git("diff", "-U0", base, "--", path)
    result = []
    for block in re.split(r"(?m)^(?=@@ )", diff):
        header = re.match(r"@@ -(\d+)(?:,(\d+))? \+\d+(?:,\d+)? @@", block)
        if not header:
            continue
        start, count = int(header.group(1)), int(header.group(2) if header.group(2) is not None else 1)
        changed = [line[1:] for line in block.splitlines()[1:] if line[:1] in "+-"]
        span = (start, start + count - 1) if count else (start, start + 1)
        result.append((span, changed))
    return result


def impacted(stage_names, base=None):
    """Map each stage the changes can affect to its reasons. Returns (selection, map commit or None)."""
    document = load_map()
    if document is None:
        return {name: ["no impact map; run python tools/impact.py build"] for name in stage_names}, None
    base = base or document["commit"]
    files = document["files"]
    covered_by = {}
    for stage, paths in document["covered_files"].items():
        for path in paths:
            covered_by.setdefault(path, set()).add(stage)
    closures = {name: python_closure(name) for name in stage_names}
    selection = {}

    def add(stages, reason):
        for stage in stages:
            if stage in stage_names:
                selection.setdefault(stage, []).append(reason)

    unmapped = [name for name in stage_names if name not in document["covered_files"]]
    for path in changed_files(base):
        if path in EVERYTHING:
            add(stage_names, f"{path} changed")
        elif path.startswith("src/") and path.endswith(".rs"):
            add(unmapped, f"{path} changed and the fixture has no coverage data")
            spans = files.get(path, [])
            if not (ROOT / path).exists() and path in covered_by:
                add(covered_by[path], f"{path} removed")
                continue
            for (start, end), lines in hunks(base, path):
                if all(IGNORABLE.match(line) for line in lines):
                    continue
                hits = [stages for s, e, stages in spans if s <= end and e >= start]
                if hits:
                    for stages in hits:
                        add(stages, f"{path}:{start}-{end}")
                else:
                    add(covered_by.get(path, ()), f"{path}:{start}-{end} outside recorded functions")
        elif path.startswith(("tests/", "examples/")):
            add([name for name, closure in closures.items() if path in closure], f"{path} changed")
        elif path == "docs/MCP_EVALUATION.md":
            add(["integration"], f"{path} changed")
        elif path.startswith(NO_FIXTURES) or path.endswith(".md"):
            continue
        else:
            add(stage_names, f"{path} changed (unclassified)")
    return selection, document["commit"]


# ---------------------------------------------------------------- reusing passes

_tool_versions = None


def tool_versions():
    global _tool_versions
    if _tool_versions is None:
        _tool_versions = [subprocess.run([tool, "-version"], capture_output=True, text=True).stdout.splitlines()[:1] for tool in ("ffmpeg", "ffprobe")]
    return _tool_versions


def fingerprint(stage, mode_args):
    """Hash of every input a fixture's result depends on, as far as the map can tell."""
    document = load_map()
    if document is None or stage not in document["covered_files"]:
        return None
    digest = hashlib.sha256(json.dumps([stage, mode_args, tool_versions()]).encode())
    for path in sorted(python_closure(stage) | set(document["covered_files"][stage]) | {"Cargo.toml", "Cargo.lock", "build.rs"}):
        file = ROOT / path
        digest.update(path.encode() + b"\0" + (file.read_bytes() if file.is_file() else b"<missing>") + b"\0")
    return digest.hexdigest()


def cached_passes():
    """Fingerprints of fixture passes recorded by recent quick, gate and background runs."""
    passes = {}
    for result in sorted(RESULTS.glob("*.json"))[-400:]:
        try:
            for stage, value in json.loads(result.read_text(encoding="utf-8")).get("passes", {}).items():
                passes.setdefault(stage, set()).add(value)
        except (OSError, ValueError):
            continue
    return passes


def recent(count):
    """The newest run records, newest first."""
    found = []
    for result in sorted(RESULTS.glob("*.json"))[-count:][::-1]:
        try:
            found.append((result.name, json.loads(result.read_text(encoding="utf-8"))))
        except (OSError, ValueError):
            continue
    return found


def unresolved_failures():
    """Fixtures whose most recent recorded run failed (for example in background verification)."""
    latest = {}
    for result in sorted(RESULTS.glob("*.json")):
        try:
            data = json.loads(result.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        for stage in data.get("passes", {}):
            latest[stage] = None
        for stage in data.get("failures", []):
            latest[stage] = f"{data.get('run')} run of {str(data.get('commit') or '?')[:10]}, {result.name[:15]}"
    return {stage: where for stage, where in latest.items() if where}


def record(run, passes, **extra):
    RESULTS.mkdir(parents=True, exist_ok=True)
    path = RESULTS / f"{datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S')}-{run}-{os.getpid()}.json"
    path.write_text(json.dumps({"run": run, "passes": passes, **extra}, indent=1) + "\n", encoding="utf-8")
    return path


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build", help="Build the coverage map from the current commit")
    b.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) // 2))
    b.add_argument("--keep-profiles", action="store_true")
    b.add_argument("--decode-device", type=int, help="CUDA ordinal, so the hardware-decode fixtures are mapped too")
    b.add_argument("--from-profiles", type=Path, help="Reprocess profiles kept by an earlier build of the current commit")
    s = sub.add_parser("show", help="List fixtures affected by changes since the map's commit")
    s.add_argument("--base", help="Diff against this commit instead of the map's")
    args = parser.parse_args()
    if args.command == "build":
        build(args.jobs, args.keep_profiles, args.decode_device, args.from_profiles)
    else:
        document = load_map()
        names = sorted(document["covered_files"]) + document.get("missing", []) if document else []
        selection, commit = impacted(names, args.base)
        print(f"Map commit: {commit or 'none'}; {len(selection)} of {len(names)} fixtures affected")
        for name, reasons in sorted(selection.items()):
            print(f"  {name}: {'; '.join(reasons[:3])}{' ...' if len(reasons) > 3 else ''}")
