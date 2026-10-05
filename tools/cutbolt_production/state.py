"""Durable coordinator state beside the production: one lock, stage receipts, revisions, review events.

Layout under <root>/state:
  coordinator.lock        the live coordinator's PID; a second coordinator on the same root fails
  stages/<stage>.json     the latest receipt per stage (written whole, then renamed into place)
  revisions/<n>.json      one immutable record per build: manifest digest, parent, stage outcomes
  events.jsonl            append-only history: stage starts, completions, reuse, failures, reviews
  calls.jsonl             every engine/companion call with its elapsed time

A receipt counts as complete only when its state is `completed`, its key matches the stage's current inputs,
and every output file still has its recorded size and modification time (or, if those changed, its SHA-256).
"""
import ctypes
import hashlib
import json
import os
import threading
import time
from datetime import datetime, timezone
from pathlib import Path


class StateError(RuntimeError):
    def __init__(self, code, message):
        super().__init__(f"{code}: {message}")
        self.code = code


def now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def pid_alive(pid):
    if os.name == "nt":
        handle = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
        if not handle:
            return False
        code = ctypes.c_ulong()
        ctypes.windll.kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
        ctypes.windll.kernel32.CloseHandle(handle)
        return code.value == 259  # STILL_ACTIVE
    try:
        os.kill(pid, 0)
        return True
    except OSError:
        return False


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def write_json_atomic(path, value):
    path = Path(path)
    temp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    with open(temp, "w", encoding="utf-8") as f:
        json.dump(value, f, indent=1, ensure_ascii=False)
        f.flush()
        os.fsync(f.fileno())
    os.replace(temp, path)


class State:
    def __init__(self, root):
        self.root = Path(root)
        self.dir = self.root / "state"
        self.stages_dir = self.dir / "stages"
        self.revisions_dir = self.dir / "revisions"
        for d in (self.dir, self.stages_dir, self.revisions_dir):
            d.mkdir(parents=True, exist_ok=True)
        self.lock_path = self.dir / "coordinator.lock"
        self.locked = False
        self.log_lock = threading.Lock()

    # ------------------------------------------------------------------ lock
    def acquire(self):
        record = {"pid": os.getpid(), "started": now()}
        for _ in range(2):
            try:
                with open(self.lock_path, "x", encoding="utf-8") as f:
                    json.dump(record, f)
                self.locked = True
                return None
            except FileExistsError:
                try:
                    holder = json.loads(self.lock_path.read_text(encoding="utf-8"))
                except (OSError, json.JSONDecodeError):
                    holder = {"pid": -1}
                if holder.get("pid", -1) > 0 and pid_alive(holder["pid"]):
                    raise StateError("PRODUCTION_BUSY", f"coordinator PID {holder['pid']} is already working on {self.root} "
                                                        f"(since {holder.get('started')}); wait for it or run `status`")
                # The previous coordinator died; its stages are reconciled from their receipts, and its build record says so.
                self.event({"event": "stale_lock_cleared", "holder": holder})
                for number in self.revisions():
                    record = self.revision(number)
                    if record.get("outcome") == "running" and record.get("pid") == holder.get("pid"):
                        record.update(outcome="interrupted", finished=None)
                        write_json_atomic(self.revisions_dir / f"{number}.json", record)
                self.lock_path.unlink(missing_ok=True)
        raise StateError("PRODUCTION_BUSY", f"could not take {self.lock_path}")

    def release(self):
        if self.locked:
            self.lock_path.unlink(missing_ok=True)
            self.locked = False

    # ------------------------------------------------------------------ logs
    def event(self, record):
        record = {"time": now(), **record}
        with self.log_lock, open(self.dir / "events.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")

    def call_log(self, record):
        with self.log_lock, open(self.dir / "calls.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")

    # ------------------------------------------------------------------ receipts
    def stage_path(self, stage):
        return self.stages_dir / (stage.replace(":", "__") + ".json")

    def receipt(self, stage):
        path = self.stage_path(stage)
        if not path.exists():
            return None
        return json.loads(path.read_text(encoding="utf-8"))

    def receipts(self):
        out = {}
        for path in sorted(self.stages_dir.glob("*.json")):
            value = json.loads(path.read_text(encoding="utf-8"))
            out[value["stage"]] = value
        return out

    def save(self, receipt):
        write_json_atomic(self.stage_path(receipt["stage"]), receipt)

    def identity(self, relative):
        """Content identity of a file under the root: relative path, SHA-256, bytes, plus mtime for quick rechecks."""
        path = self.root / relative
        st = path.stat()
        return {"path": Path(relative).as_posix(), "sha256": sha256_file(path), "bytes": st.st_size, "mtime_ns": st.st_mtime_ns}

    def verify(self, output):
        path = self.root / output["path"]
        try:
            st = path.stat()
        except OSError:
            return False
        if st.st_size != output["bytes"]:
            return False
        if st.st_mtime_ns == output.get("mtime_ns"):
            return True
        return sha256_file(path) == output["sha256"]

    def reusable(self, stage, key):
        receipt = self.receipt(stage)
        if not receipt or receipt.get("state") != "completed" or receipt.get("key") != key:
            return None
        if not all(self.verify(o) for o in receipt.get("outputs", [])):
            self.event({"event": "outputs_changed", "stage": stage})
            return None
        return receipt

    # ------------------------------------------------------------------ revisions
    def revisions(self):
        return sorted(int(p.stem) for p in self.revisions_dir.glob("*.json"))

    def revision(self, number):
        return json.loads((self.revisions_dir / f"{number}.json").read_text(encoding="utf-8"))

    def start_revision(self, manifest_sha, manifest_summary):
        existing = self.revisions()
        parent = existing[-1] if existing else None
        number = (parent + 1) if parent is not None else 0
        record = {"revision": number, "parent_revision": parent, "manifest_sha256": manifest_sha, "manifest": manifest_summary,
                  "started": now(), "finished": None, "outcome": "running", "pid": os.getpid(), "stages": {}}
        path = self.revisions_dir / f"{number}.json"
        with open(path, "x", encoding="utf-8") as f:
            json.dump(record, f, indent=1, ensure_ascii=False)
        return record

    def finish_revision(self, record):
        write_json_atomic(self.revisions_dir / f"{record['revision']}.json", record)

    # ------------------------------------------------------------------ reviews
    def reviews(self):
        path = self.dir / "reviews.jsonl"
        if not path.exists():
            return []
        return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line.strip()]

    def add_review(self, record):
        with open(self.dir / "reviews.jsonl", "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")
        self.event({"event": "review", **record})


class Timer:
    def __init__(self):
        self.t0 = time.perf_counter()

    def seconds(self):
        return round(time.perf_counter() - self.t0, 3)
