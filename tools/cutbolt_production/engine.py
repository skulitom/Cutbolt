"""Clients for the three tools the coordinator drives, all through their public local interfaces.

* Cutbolt: one JSON request per process on the engine CLI, inside the production root as its workspace.
  Long commands go through the engine's own durable job queue (job.start/job.wait), so a coordinator crash
  leaves them running and a restart collects them by request ID instead of starting them again.
* PixelForge: its CLI `render` command, one recipe per process, into a fresh folder.
* Qwen3-TTS: tools/qwen_tts_worker.py inside the configured speech environment (for example WSL).
Arguments are passed as arrays, never through a shell.
"""
import json
import os
import subprocess
import threading
import time
from pathlib import Path, PureWindowsPath

from .state import now

NO_WINDOW = 0x08000000 if os.name == "nt" else 0
REPO = Path(__file__).resolve().parents[2]


class ToolError(RuntimeError):
    def __init__(self, tool, code, message, detail=None):
        super().__init__(f"{tool} {code}: {message}")
        self.tool, self.code, self.message, self.detail = tool, code, message, detail


def linux_path(path):
    """C:\\DEV\\x -> /mnt/c/DEV/x for a WSL process."""
    win = PureWindowsPath(path)
    if not win.drive or len(win.drive) != 2:
        raise ToolError("tts", "INVALID_PATH", f"{path} is not on a lettered drive")
    return "/mnt/" + win.drive[0].lower() + "/" + "/".join(win.parts[1:])


class Engine:
    def __init__(self, exe, root, env, state, lanes):
        self.exe, self.root, self.state = str(exe), str(root), state
        self.env = {**os.environ, **env}
        self.calls = 0
        self.lock = threading.Lock()
        self.job_roots = {}
        for lane in ["speech", "music", "export", "review", "captions"] + [f"lane-{i}" for i in range(lanes)]:
            path = Path(root) / "state" / "jobs" / lane
            path.mkdir(parents=True, exist_ok=True)
            self.job_roots[lane] = f"state/jobs/{lane}"

    def call(self, command, args, label=None, timeout=3600):
        request = json.dumps({"command": command, **args}, ensure_ascii=False).encode("utf-8")
        if len(request) > 4 * 1024 * 1024:
            raise ToolError("cutbolt", "REQUEST_TOO_LARGE", f"{command} request is {len(request)} bytes; pass documents as files")
        started = time.perf_counter()
        process = subprocess.run([self.exe, "--workspace", self.root], input=request, capture_output=True, env=self.env,
                                 timeout=timeout, creationflags=NO_WINDOW)
        elapsed = round(time.perf_counter() - started, 3)
        with self.lock:
            self.calls += 1
        try:
            reply = json.loads(process.stdout.decode("utf-8"))
        except (json.JSONDecodeError, UnicodeDecodeError):
            raise ToolError("cutbolt", "BAD_REPLY", f"{command}: exit {process.returncode}: "
                            f"{process.stdout[:400]!r} {process.stderr[:400]!r}") from None
        self.state.call_log({"time": now(), "tool": "cutbolt", "command": command, "label": label, "elapsed_s": elapsed, "ok": reply.get("ok")})
        if not reply.get("ok"):
            error = reply.get("error", {})
            raise ToolError("cutbolt", error.get("code", "ERROR"), f"{command}: {error.get('message', reply)}", error)
        return reply["result"]

    def job(self, command, args, request_id, lane, label=None, wait_seconds=30):
        root = self.job_roots[lane]
        ticket = self.call("job.start", {"job_root": root, "request_id": request_id, "run": command, "arguments": args}, label=label)
        while True:
            status = self.call("job.wait", {"job_root": root, "job_id": ticket["job_id"], "timeout_seconds": wait_seconds}, label=label)
            if status.get("finished"):
                break
        if status.get("status") != "completed":
            error = status.get("error") or {}
            raise ToolError("cutbolt", error.get("code", status.get("status", "FAILED")),
                            f"{command} job {ticket['job_id']} {status.get('status')}: {error.get('message', '')}", status)
        return status["result"]


class PixelForge:
    def __init__(self, config, state):
        self.node = config.get("node", "node")
        self.script = config["script"]
        self.state = state
        package = Path(self.script).resolve().parents[1] / "package.json"
        self.version = json.loads(package.read_text(encoding="utf-8"))["version"] if package.exists() else None
        commit = None
        try:
            commit = subprocess.run(["git", "-C", str(package.parent), "rev-parse", "HEAD"], capture_output=True, text=True,
                                    timeout=10, creationflags=NO_WINDOW).stdout.strip() or None
        except OSError:
            pass
        self.identity = {"package": "pixelforge-agent", "version": self.version, "commit": commit}

    def render(self, recipe_path, out_dir):
        started = time.perf_counter()
        process = subprocess.run([self.node, self.script, "render", str(recipe_path), "--out", str(out_dir)], capture_output=True,
                                 timeout=300, creationflags=NO_WINDOW)
        elapsed = round(time.perf_counter() - started, 3)
        self.state.call_log({"time": now(), "tool": "pixelforge", "command": "render", "label": Path(recipe_path).name, "elapsed_s": elapsed,
                             "ok": process.returncode == 0})
        if process.returncode != 0:
            raise ToolError("pixelforge", "RENDER_FAILED", f"{Path(recipe_path).name}: {process.stderr.decode('utf-8', 'replace')[:2000]}")
        return json.loads(process.stdout.decode("utf-8"))


class Qwen:
    """The batched narration worker, launched in the configured speech environment."""

    def __init__(self, config, state):
        self.config, self.state = config, state
        self.worker = REPO / "tools" / "qwen_tts_worker.py"

    def synthesize(self, request_path, timeout=1800):
        cfg = self.config
        command = ["wsl", "-d", cfg["distribution"], "--exec", "env", "PYTHONPATH=" + ":".join(cfg["python_paths"]),
                   cfg.get("python", "/usr/bin/python3"), linux_path(self.worker), linux_path(request_path)]
        started = time.perf_counter()
        env = {**os.environ, "MSYS_NO_PATHCONV": "1"}
        process = subprocess.run(command, capture_output=True, timeout=timeout, env=env, creationflags=NO_WINDOW)
        elapsed = round(time.perf_counter() - started, 3)
        lines = process.stdout.decode("utf-8", "replace").strip().splitlines()
        reply = None
        for line in reversed(lines):
            if line.startswith("{"):
                try:
                    reply = json.loads(line)
                    break
                except json.JSONDecodeError:
                    continue
        self.state.call_log({"time": now(), "tool": "qwen-tts", "command": "synthesize", "elapsed_s": elapsed,
                             "ok": bool(reply and reply.get("ok"))})
        if not reply or not reply.get("ok"):
            error = (reply or {}).get("error") or {}
            tail = process.stderr.decode("utf-8", "replace").strip().splitlines()[-12:]
            raise ToolError("qwen-tts", error.get("code", "WORKER_FAILED"),
                            error.get("message") or f"exit {process.returncode}: " + " | ".join(tail))
        return reply["result"], elapsed
