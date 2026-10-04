"""Ship the committed HEAD in about two minutes.

1. Builds the engine and copies it out of target/debug.
2. Runs the verifier's push gate: Rust lint/tests beside the fixtures the changes can affect, reusing
   unchanged passes and deferring whatever does not fit the time budget.
3. Pushes the current branch.
4. Verifies the deferred fixtures in the background against a detached git worktree of the pushed commit,
   with its own engine copy, at below-normal priority. Nothing in this checkout is locked or blocked;
   results go to verification/results and `python tools/verify.py --status` reports them. A failure there
   is fixed forward with a new commit; the next gate warns about it until it passes.
"""
import argparse
from datetime import datetime
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
ENGINE_NAME = "cutbolt.exe" if os.name == "nt" else "cutbolt"
CODE = ["src", "tests", "tools", "examples", "Cargo.toml", "Cargo.lock", "build.rs", "docs/MCP_EVALUATION.md"]
sys.path.insert(0, str(Path(__file__).resolve().parent))
import impact  # noqa: E402

# Engine copies, logs, reports and background worktrees live with the shared verifier state in the
# common git directory: visible to every session and worktree, and outside any virtualized app folder.
STATE = impact.STATE / "ship"


def git(*args, check=True):
    return subprocess.run(["git", *args], cwd=ROOT, capture_output=True, text=True, encoding="utf-8", check=check)


def acquire_ship_lock():
    """Serialize gate-and-push across every session and worktree of this repository."""
    import psutil
    lock = impact.STATE / "ship.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    announced = False
    while True:
        try:
            descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
            os.write(descriptor, str(os.getpid()).encode())
            os.close(descriptor)
            return lock
        except FileExistsError:
            try:
                holder = int(lock.read_text(encoding="utf-8") or 0)
            except (OSError, ValueError):
                holder = 0
            if not holder or not psutil.pid_exists(holder):
                lock.unlink(missing_ok=True)  # left by a ship that ended abnormally
                continue
            if not announced:
                print(f"Waiting for another ship (pid {holder}) to finish its gate and push...", flush=True)
                announced = True
            time.sleep(2)


def target_branch(args):
    if args.to:
        return args.to
    upstream = git("rev-parse", "--abbrev-ref", "@{u}", check=False).stdout.strip()
    return upstream.split("/", 1)[1] if upstream.startswith("origin/") else None


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--budget", type=float, default=85, help="Gate fixture budget in seconds")
    # Process creation saturates near eight concurrent fixtures here; more lanes slow every fixture.
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--background-jobs", type=int, default=4, help="Concurrent fixtures in the background verification (it also yields to running gates)")
    parser.add_argument("--no-push", action="store_true", help="Run the gate and background verification without pushing")
    parser.add_argument("--decode-device", type=int, help="CUDA ordinal for the hardware-decode fixtures (otherwise they are skipped)")
    parser.add_argument("--to", help="Push HEAD to this branch of origin (for example main from a session branch); default: the current branch's upstream")
    args = parser.parse_args()

    dirty = git("status", "--porcelain", "--", *CODE).stdout.strip()
    if dirty:
        raise SystemExit("Commit (or stash) these code changes first; ship verifies and pushes HEAD:\n" + dirty)
    lock = None if args.no_push else acquire_ship_lock()
    try:
        commit, summary, engine, run_id, device = gate_and_push(args)
    finally:
        if lock:
            lock.unlink(missing_ok=True)
    start_background(args, commit, summary, engine, run_id, device)


def gate_and_push(args):
    branch = target_branch(args)
    if branch and not args.no_push:
        # Inside the lock: build on the latest target so the gated commit is exactly what is pushed.
        git("fetch", "origin", branch)
        behind = git("merge-base", "--is-ancestor", f"origin/{branch}", "HEAD", check=False).returncode
        if behind:
            rebased = git("rebase", "--autostash", f"origin/{branch}", check=False)
            if rebased.returncode:
                git("rebase", "--abort", check=False)
                raise SystemExit(f"Rebasing onto origin/{branch} conflicts; resolve it, then ship again:\n{rebased.stdout}{rebased.stderr}")
            print(f"Rebased onto origin/{branch}", flush=True)
    commit = git("rev-parse", "HEAD").stdout.strip()
    stamp = datetime.now().strftime("%Y%m%dT%H%M%S")
    run_id = f"{commit[:10]}-{stamp}"
    for directory in ("engines", "worktrees", "logs", "reports"):
        (STATE / directory).mkdir(parents=True, exist_ok=True)
        # Keep three days of engine copies, logs and reports; worktrees remove themselves.
        for old in (STATE / directory).iterdir():
            if directory != "worktrees" and old.is_file() and datetime.now().timestamp() - old.stat().st_mtime > 3 * 86400:
                old.unlink(missing_ok=True)
    git("worktree", "prune", check=False)

    subprocess.run(["cargo", "build", "--locked"], cwd=ROOT, check=True)
    engine = STATE / "engines" / f"{run_id}-{ENGINE_NAME}"
    shutil.copy2(ROOT / "target" / "debug" / ENGINE_NAME, engine)
    report = STATE / "reports" / f"{run_id}-gate.json"
    device = ["--decode-device", str(args.decode_device)] if args.decode_device is not None else []
    gate = subprocess.run([sys.executable, "-X", "utf8", "tools/verify.py", "--gate", "--engine", str(engine), "--budget", str(args.budget),
                           "--jobs", str(args.jobs), "--commit", commit, "--report", str(report)] + device, cwd=ROOT)
    summary = json.loads(report.read_text(encoding="utf-8")) if report.exists() else {"ok": False}
    if gate.returncode or not summary.get("ok"):
        raise SystemExit(f"Gate failed for {commit[:10]}; nothing was pushed")

    if not args.no_push:
        pushed = git("push", "origin", f"HEAD:{args.to}", check=False) if args.to else git("push", check=False)
        if pushed.returncode:
            raise SystemExit("Push failed (fetch and rebase, then ship again):\n" + pushed.stderr)
        print(f"Pushed {commit[:10]}", flush=True)
    return commit, summary, engine, run_id, device


def start_background(args, commit, summary, engine, run_id, device):
    deferred = summary.get("deferred", [])
    if not deferred:
        engine.unlink(missing_ok=True)
        print("Nothing deferred: every affected fixture ran or was reused in the gate.")
        return
    worktree = STATE / "worktrees" / run_id
    git("worktree", "add", "--detach", str(worktree), commit)
    log = STATE / "logs" / f"{run_id}.log"
    command = [sys.executable, "-X", "utf8", str(worktree / "tools" / "verify.py"), "--background", "--no-rust", "--only", ",".join(deferred),
               "--engine", str(engine), "--commit", commit, "--jobs", str(args.background_jobs), "--cleanup-worktree"] + device
    flags = 0
    if os.name == "nt":
        # A hidden console shared by every child (DETACHED_PROCESS would make each console child open its
        # own window); a separate process group keeps console control events from reaching the run.
        flags = subprocess.CREATE_NO_WINDOW | subprocess.CREATE_NEW_PROCESS_GROUP | subprocess.BELOW_NORMAL_PRIORITY_CLASS
    with log.open("w", encoding="utf-8") as output:
        process = subprocess.Popen(command, cwd=worktree, stdout=output, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL,
                                   creationflags=flags, start_new_session=os.name != "nt")
    print(f"Background verification of {len(deferred)} deferred fixtures started (pid {process.pid}): {', '.join(deferred)}")
    print(f"  log: {log}\n  status: python tools/verify.py --status")


if __name__ == "__main__":
    main()
