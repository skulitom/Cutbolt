"""Wire-level MCP and persisted job acceptance against original generated media."""
from engine import ENGINE, MCP_TOOLS
import argparse
import asyncio
from datetime import timedelta
import ctypes
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import queue
import subprocess
import sys
import threading
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
TERMINAL = {"completed", "cancelled", "failed", "interrupted"}


class Client:
    def __init__(self, executable, env=None, args=("mcp",)):
        self.process = subprocess.Popen([str(executable), *args], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
        self.lines = queue.Queue()
        self.reader = threading.Thread(target=self._read, daemon=True)
        self.reader.start()
        self.next_id = 1

    def _read(self):
        for line in self.process.stdout:
            self.lines.put(line)
        self.lines.put(None)

    def write(self, data):
        self.process.stdin.write(data)
        self.process.stdin.flush()

    def receive(self):
        line = self.lines.get(timeout=45)
        if line is None:
            raise AssertionError(f"MCP process exited: {self.process.stderr.read()!r}")
        return json.loads(line)

    def rpc(self, method, params=None):
        number = self.next_id
        self.next_id += 1
        request = {"jsonrpc": "2.0", "id": number, "method": method}
        if params is not None:
            request["params"] = params
        self.write(json.dumps(request).encode() + b"\n")
        response = self.receive()
        assert response["id"] == number, response
        return response

    def initialize(self, version="2025-11-25"):
        result = self.rpc("initialize", {"protocolVersion": version, "capabilities": {}, "clientInfo": {"name": "cutbolt-verification", "version": "1"}})
        assert result["result"]["protocolVersion"] in {"2025-11-25", "2025-06-18"}, result
        self.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
        return result

    def call(self, command, expected_error=None, **fields):
        response = self.rpc("tools/call", {"name": "cutbolt_" + command.replace(".", "_"), "arguments": fields})
        assert "error" not in response, response
        tool = response["result"]
        result = tool["structuredContent"]
        # An outline's text content is the outline itself; every other result is its JSON.
        if command == "timeline.outline" and result["ok"] and "outline" in result["result"]:
            assert tool["content"][0]["text"] == result["result"]["outline"]
        else:
            assert json.loads(tool["content"][0]["text"]) == result
        assert tool["isError"] == (not result["ok"])
        if expected_error:
            assert not result["ok"] and result["error"]["code"] == expected_error, result
            return result
        assert result["ok"], result
        return result["result"]

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                raise AssertionError("MCP did not exit on EOF")
        self.reader.join(timeout=2)
        assert self.process.returncode == 0, self.process.stderr.read()


def until(action, predicate, seconds=45):
    deadline = time.monotonic() + seconds
    value = None
    while time.monotonic() < deadline:
        value = action()
        if predicate(value):
            return value
        time.sleep(0.05)
    raise AssertionError(f"Timed out waiting: {value}")


def file_hash(path):
    return hashlib.file_digest(path.open("rb"), "sha256").hexdigest()


def decoded_hash(path, audio=False):
    args = [os.environ.get("CUTBOLT_FFMPEG", "ffmpeg"), "-v", "error", "-i", str(path)]
    args += ["-map", "0:a:0", "-f", "s16le", "-acodec", "pcm_s16le"] if audio else ["-map", "0:v:0", "-f", "rawvideo", "-pix_fmt", "rgb24"]
    process = subprocess.Popen(args + ["pipe:1"], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    digest = hashlib.file_digest(process.stdout, "sha256").hexdigest()
    assert process.wait(timeout=20) == 0, process.stderr.read()
    return digest


class ProcessHandle:
    """Hold a real process handle so PID reuse cannot affect interruption tests."""
    def __init__(self, pid):
        self.api = ctypes.WinDLL("kernel32", use_last_error=True)
        self.api.OpenProcess.argtypes = [ctypes.c_uint32, ctypes.c_int, ctypes.c_uint32]
        self.api.OpenProcess.restype = ctypes.c_void_p
        self.api.WaitForSingleObject.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.api.TerminateProcess.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        self.api.CloseHandle.argtypes = [ctypes.c_void_p]
        self.handle = self.api.OpenProcess(0x00100001, False, pid)
        assert self.handle, ctypes.get_last_error()

    def stop(self):
        assert self.api.TerminateProcess(self.handle, 87), ctypes.get_last_error()

    def exited(self):
        return self.api.WaitForSingleObject(self.handle, 0) == 0

    def close(self):
        self.api.CloseHandle(self.handle)


async def sdk_roundtrip(executable):
    # Exercise an independently maintained client, not only our wire test helper.
    from mcp import ClientSession, StdioServerParameters
    from mcp.client.stdio import stdio_client
    params = StdioServerParameters(command=str(executable), args=["mcp"])
    async with stdio_client(params) as (read, write):
        async with ClientSession(read, write, read_timeout_seconds=timedelta(seconds=20)) as session:
            initialized = await session.initialize()
            assert initialized.protocolVersion == "2025-06-18"
            listing = await session.list_tools()
            assert len(listing.tools) == MCP_TOOLS
            created = await session.call_tool("cutbolt_project_create", {"id": "sdk", "width": 320, "height": 180, "frame_rate": {"num": 25, "den": 1}})
            assert not created.isError and created.structuredContent["result"]["id"] == "sdk"
            invalid = await session.call_tool("cutbolt_project_validate", {"project": {}})
            assert invalid.isError and invalid.structuredContent["error"]["code"] == "INVALID_JSON"
            await session.send_ping()


def run(executable, fixture):
    from jsonschema import Draft202012Validator
    executable = executable.resolve()
    fixture = fixture.resolve()
    root = fixture / "agents"
    root.mkdir()
    passed = []
    clients = []
    handles = []
    jobs = []

    def check(name, condition):
        assert condition, name
        passed.append(name)

    def client(env=None):
        value = Client(executable, env)
        clients.append(value)
        return value

    normal = client()
    try:
        check("mcp.initialization_guard", normal.rpc("tools/list")["error"]["code"] == -32002)
        assert normal.rpc("initialize", {})["error"]["code"] == -32602
        init = normal.initialize("2099-01-01")
        check("mcp.version_negotiation", init["result"]["protocolVersion"] == "2025-11-25")
        catalog = normal.rpc("tools/list")["result"]["tools"]
        for tool in catalog:
            Draft202012Validator.check_schema(tool["inputSchema"])
            # Results are the {ok, result | error} envelope in structuredContent; no per-tool output schema is listed.
            assert "outputSchema" not in tool
        names = [tool["name"] for tool in catalog]
        check("mcp.discoverable_schemas", len(names) == len(set(names)) == MCP_TOOLS
              and {"cutbolt_registry_search","cutbolt_registry_status","cutbolt_registry_bind","cutbolt_registry_relink"}.issubset(names)
              and "cutbolt_schema" in names and "cutbolt_render_start" in names and "cutbolt_render_run" not in names
              and "cutbolt_audio_inspect" in names and "cutbolt_audio_render" not in names
              and "cutbolt_proxy_status" in names and "cutbolt_proxy_relink" in names and "cutbolt_proxy_generate" not in names
              and "cutbolt_media_conform_inspect" in names and "cutbolt_media_conform" not in names and "cutbolt_media_prepare" not in names
              and "cutbolt_hdr_inspect" in names and "cutbolt_hdr_conform" not in names
              and "cutbolt_graphics_instantiate" in names
              and {"cutbolt_captions_"+operation for operation in ("import","inspect","apply","encode","export","scene")}.issubset(names))
        # The compact catalog: the core tools in full, and cutbolt_run for every other tool command.
        compact = Client(executable, args=("mcp", "--tools", "core"));clients.append(compact)
        compact.initialize()
        core = compact.rpc("tools/list")["result"]["tools"]
        core_names = [tool["name"] for tool in core]
        others = next(t for t in core if t["name"] == "cutbolt_run")["inputSchema"]["properties"]["command"]["enum"]
        routed = {"cutbolt_" + c.replace(".", "_") for c in others}
        check("mcp.compact_catalog", len(core_names) == len(set(core_names)) == 32 and not routed & set(core_names)
              and (set(core_names) - {"cutbolt_run"}) | routed == set(names) and len(json.dumps(core)) * 3 < len(json.dumps(catalog)))
        made = normal.call("project.create", id="compact", width=16, height=16, frame_rate={"num":25, "den":1})
        routed_call = compact.rpc("tools/call", {"name":"cutbolt_run", "arguments":{"command":"project.validate", "arguments":{"project":made}}})["result"]
        assert routed_call["structuredContent"]["result"] == normal.call("project.validate", project=made) and not routed_call["isError"]
        assert compact.call("files.list", input_root=str(root))["entries"] == normal.call("files.list", input_root=str(root))["entries"]
        for command in ("export.run", "nope"):
            assert compact.rpc("tools/call", {"name":"cutbolt_run", "arguments":{"command":command, "arguments":{}}})["error"]["code"] == -32602
        by_env = Client(executable, env={**os.environ, "CUTBOLT_MCP_TOOLS":"core"});clients.append(by_env)
        by_env.initialize()
        assert [t["name"] for t in by_env.rpc("tools/list")["result"]["tools"]] == core_names
        assert subprocess.run([str(executable), "mcp", "--tools", "nope"], capture_output=True, timeout=30).returncode == 1
        (root / "listed").mkdir()
        (root / "listed" / "clip.MKV").write_bytes(b"x" * 3)
        listed = normal.call("files.list", input_root=str(root), recursive=True, extensions=["mkv"])
        assert listed["entries"] == [{"path": "listed/clip.MKV", "kind": "file", "bytes": 3}], listed
        check("mcp.capabilities", normal.call("capabilities")["local_only"])
        assert normal.rpc("ping")["result"] == {}
        assert normal.rpc("unknown")["error"]["code"] == -32601
        assert normal.rpc("tools/call", {"name": "does_not_exist"})["error"]["code"] == -32602
        result = normal.rpc("tools/call", {"name": "cutbolt_capabilities", "arguments": {"command": "job.cancel"}})["result"]
        assert result["isError"] and result["structuredContent"]["error"]["code"] == "INVALID_JSON"
        normal.call("project.create", "INVALID_JSON", surprise=True)
        check("mcp.error_contract", True)
        normal.write(b'{broken}\n')
        assert normal.receive()["error"]["code"] == -32700
        normal.write(b'[]\n')
        assert normal.receive()["error"]["code"] == -32600
        normal.write(b' ' * (4 * 1024 * 1024 + 1) + b'\n')
        assert normal.receive()["error"]["code"] == -32600
        normal.write(b'{"jsonrpc":"2.0","method":"tools/call","params":{"name":"cutbolt_project_create","arguments":{}}}\n')
        check("mcp.framing_recovery", normal.rpc("ping")["result"] == {})

        time_value = lambda n: {"num": n, "den": 1}
        project = normal.call("project.create", id="mcp-demo", width=320, height=180, frame_rate=time_value(25))
        project_schema = next(t["inputSchema"] for t in catalog if t["name"] == "cutbolt_session_create")
        session_root = root / "sessions"
        session_root.mkdir()
        create_args = {"store_root": str(session_root), "request_id": "create", "project": project}
        Draft202012Validator(project_schema).validate(create_args)
        normal.call("session.create", **create_args)
        common = {"store_root": str(session_root), "project_id": "mcp-demo"}
        operations = [
            {"op": "media.add", "asset": {"id": "source", "path": str(fixture / "media/source-0.mkv"), "duration": time_value(12)}},
            {"op": "clip.append", "clip": {"id": "a", "asset_id": "source", "source_in": time_value(1), "duration": time_value(10)}},
        ]
        edit_args = {**common, "request_id": "assemble", "expected_revision": 0, "operations": operations}
        receipt = normal.call("session.apply", **edit_args)
        split = [{"op": "clip.split", "clip_id": "a", "new_clip_id": "b", "offset": time_value(4)}]
        preview = normal.call("session.preview", **common, expected_revision=1, operations=split)
        normal.call("session.apply", **common, request_id="split", expected_revision=1, operations=split)
        check("mcp.shared_session_semantics", normal.call("session.apply", **edit_args) == receipt
              and len(preview["clips"]) == 2 and normal.call("session.get", **common)["revision"] == 2)
        normal.call("session.undo", **common, request_id="undo", expected_revision=2)
        normal.call("session.restore", **common, request_id="restore", expected_revision=3, target_revision=2)
        page = normal.call("session.history", **common, limit=2)
        rest = normal.call("session.history", **common, limit=3, before_revision=page["next_before_revision"])
        check("mcp.undo_history", [e["revision"] for e in page["entries"] + rest["entries"]] == [4, 3, 2, 1, 0])

        history_entries = page["entries"] + rest["entries"]
        snapshots = {entry["revision"]: normal.call("session.get", **common, revision=entry["revision"]) for entry in history_entries}
        seconds = lambda value: value["num"] // value["den"]
        duration = lambda snapshot: sum(seconds(clip["duration"]) for clip in snapshot["clips"])
        head = snapshots[4]
        right = head["clips"][1]
        proposed = normal.call("session.preview", **common, expected_revision=4, operations=[{"op":"clip.trim","clip_id":"b","source_in":time_value(5),"duration":time_value(3)}])
        assert normal.call("session.get", **common)["revision"] == 4
        answers = [
            str(history_entries[0]["restored_from"]),
            str(seconds(right["source_in"]) + seconds(right["duration"])),
            str(sum(duration(snapshots[e["revision"]]) for e in history_entries if e["action"] == "apply")),
            str(sum(len(p["clips"]) == 2 for p in snapshots.values())),
            str(len(snapshots[next(e["revision"] for e in history_entries if e["action"] == "undo")]["clips"])),
            max(head["clips"], key=lambda c: seconds(c["duration"]))["id"],
            str(seconds(head["clips"][0]["duration"]) * seconds(head["frame_rate"])),
            str(seconds(head["assets"][0]["duration"]) - duration(head)),
            str({**head, "revision": 2} == snapshots[2]).lower(),
            str(seconds(proposed["duration_after"])),
        ]
        evaluation = (ROOT / "docs/MCP_EVALUATION.md").read_text(encoding="utf-8").split("```xml\n", 1)[1].split("```", 1)[0]
        expected = [pair.findtext("answer") for pair in ET.fromstring(evaluation)]
        check("mcp.readonly_evaluation_fixture", len(expected) == 10 and answers == expected)

        original = json.loads((fixture / "project.json").read_text())
        source_hashes = {Path(a["path"]): file_hash(Path(a["path"])) for a in original["assets"]}
        render = {"project": original, "input_root": str(fixture / "media"), "output_root": str(root), "output": str(root / "async.mkv")}
        job_root = root / "queue"
        job_root.mkdir()

        def start(cli, key, output, queue_root=job_root):
            request = {"job_root": str(queue_root), "request_id": key, "render": {**render, "output": str(output)}}
            ticket = cli.call("render.start", **request)
            jobs.append((queue_root, ticket["job_id"]))
            return ticket, request

        def status(ticket, queue_root=job_root):
            return normal.call("job.status", job_root=str(queue_root), job_id=ticket["job_id"])

        def terminal(ticket, queue_root=job_root):
            return until(lambda: status(ticket, queue_root), lambda s: s["status"] in TERMINAL)

        ticket, request = start(normal, "render", root / "async.mkv")
        # A long job.wait runs on its own worker: a later call answers first, and the wait reports progress.
        wait = {"name": "cutbolt_job_wait", "arguments": {"job_root": str(job_root), "job_id": ticket["job_id"], "timeout_seconds": 90}, "_meta": {"progressToken": "render"}}
        normal.write(json.dumps({"jsonrpc": "2.0", "id": "wait", "method": "tools/call", "params": wait}).encode() + b"\n")
        normal.write(json.dumps({"jsonrpc": "2.0", "id": "quick", "method": "tools/call", "params": {"name": "cutbolt_capabilities", "arguments": {}}}).encode() + b"\n")
        arrivals, progress = [], []
        while "wait" not in arrivals:
            message = normal.receive()
            if message.get("method") == "notifications/progress":
                assert message["params"]["progressToken"] == "render", message
                progress.append(message["params"]["progress"])
            else:
                arrivals.append(message["id"])
                if message["id"] == "wait":
                    assert message["result"]["structuredContent"]["result"]["status"] == "completed", message
        check("mcp.concurrent_calls_and_progress", progress == sorted(set(progress)) and (arrivals == ["quick", "wait"] or not progress))
        result = terminal(ticket)
        check("jobs.durable_completion", result["status"] == "completed" and result["progress"]["frames"] == 750
              and result["result"]["samples"] == 1440000)
        check("jobs.render_equivalence", all(decoded_hash(root / "async.mkv", audio) == decoded_hash(fixture / "output/first-edit.mkv", audio) for audio in [False, True]))
        check("jobs.idempotent_submission", normal.call("render.start", **request) == ticket and status(ticket) == result)
        normal.call("render.start", "REQUEST_ID_CONFLICT", **{**request, "render": {**render, "output": str(root / "different.mkv")}})
        normal.call("render.start", "OUTPUT_EXISTS", **{**request, "request_id": "overwrite"})
        normal.call("job.cancel", job_root=str(job_root), job_id=ticket["job_id"])
        check("jobs.completed_output_preserved", status(ticket) == result and file_hash(root / "async.mkv") == result["result"]["sha256"])

        fake = root / "controlled_tool.exe"
        subprocess.run(["rustc", "--edition=2024", str(ROOT / "tests/support/controlled_tool.rs"), "-o", str(fake)], check=True, capture_output=True)
        real_ffmpeg = os.environ.get("CUTBOLT_FFMPEG", "ffmpeg")

        def controlled(label):
            marker = root / (label + "-pids.json")
            gate = root / (label + "-release")
            env = {**os.environ, "CUTBOLT_FFMPEG": str(fake), "CUTBOLT_TEST_REAL_FFMPEG": real_ffmpeg,
                   "CUTBOLT_TEST_MARKER": str(marker), "CUTBOLT_TEST_GATE": str(gate)}
            cli = client(env)
            cli.initialize()
            return cli, marker, gate

        slow, marker, _ = controlled("cancel")
        slow_ticket, slow_request = start(slow, "slow", root / "cancelled.mkv")
        until(lambda: marker.exists(), bool)
        pids = json.loads(marker.read_text())
        tree = [ProcessHandle(pids[key]) for key in ["parent", "child"]]
        handles.extend(tree)
        progress = until(lambda: status(slow_ticket), lambda s: s["progress"]["frames"] == 25)
        check("jobs.live_progress", progress["status"] == "running" and not Path(slow_request["render"]["output"]).exists())
        # Closing the agent connection must not cancel the persisted job.
        slow.close()
        clients.remove(slow)
        check("jobs.survives_client_exit", status(slow_ticket)["status"] == "running")
        queued, _ = start(normal, "queued-cancel", root / "queued-cancelled.mkv")
        check("jobs.bounded_queue", status(queued)["status"] == "queued")
        normal.call("render.start", "OUTPUT_RESERVED", **{**slow_request, "request_id": "same-output"})
        normal.call("job.cancel", job_root=str(job_root), job_id=queued["job_id"])
        check("jobs.queued_cancel", status(queued)["status"] == "cancelled" and not (root / "queued-cancelled.mkv").exists())
        following, _ = start(normal, "following", root / "following.mkv")
        normal.call("job.cancel", job_root=str(job_root), job_id=slow_ticket["job_id"])
        cancelled = terminal(slow_ticket)
        until(lambda: all(p.exited() for p in tree), bool, seconds=5)
        check("jobs.running_cancel_tree", cancelled["status"] == "cancelled" and not (root / "cancelled.mkv").exists()
              and not (root / (".cutbolt-job-" + slow_ticket["job_id"] + ".partial.mkv")).exists())
        check("jobs.queue_continues", terminal(following)["status"] == "completed")

        crash_root = root / "crash-queue"
        crash_root.mkdir()
        crash_client, marker, _ = controlled("crash")
        crashed, crash_request = start(crash_client, "crash", root / "crashed.mkv", crash_root)
        until(lambda: marker.exists(), bool)
        running = status(crashed, crash_root)
        pids = json.loads(marker.read_text())
        tree = [ProcessHandle(pids[key]) for key in ["parent", "child"]]
        worker = ProcessHandle(running["worker_pid"])
        handles.extend(tree + [worker])
        worker.stop()
        until(lambda: worker.exited() and all(p.exited() for p in tree), bool, seconds=5)
        interrupted = status(crashed, crash_root)
        check("jobs.worker_interruption", interrupted["status"] == "interrupted" and not (root / "crashed.mkv").exists()
              and normal.call("render.start", **crash_request) == crashed)
        recovered, _ = start(normal, "recovered", root / "recovered.mkv", crash_root)
        normal.call("job.resume", job_root=str(crash_root))
        check("jobs.explicit_recovery", terminal(recovered, crash_root)["status"] == "completed")
        malformed = json.loads(json.dumps(render))
        malformed["project"]["assets"][0]["path"] = str(root / "absent.mkv")
        malformed["output"] = str(root / "failure.mkv")
        failed_ticket = normal.call("render.start", job_root=str(job_root), request_id="failure", render=malformed)
        jobs.append((job_root, failed_ticket["job_id"]))
        failed = terminal(failed_ticket)
        check("jobs.failure_receipt", failed["status"] == "failed" and failed["error"]["code"] == "IO_ERROR" and not (root / "failure.mkv").exists())
        check("jobs.sources_preserved", all(file_hash(path) == value for path, value in source_hashes.items()))
        asyncio.run(sdk_roundtrip(executable))
        check("mcp.official_sdk_interop", True)
        report = {"passed": passed, "development_dependencies": {name: importlib.metadata.version(name) for name in ["jsonschema", "mcp"]}}
        (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n")
        return report
    finally:
        for job_root, job_id in jobs:
            try:
                normal.call("job.cancel", job_root=str(job_root), job_id=job_id)
            except Exception:
                pass
        for job_root, job_id in jobs:
            try:
                until(lambda: normal.call("job.status", job_root=str(job_root), job_id=job_id), lambda s: s["status"] in TERMINAL, seconds=10)
            except Exception:
                pass
        for cli in reversed(clients):
            cli.close()
        for handle in handles:
            handle.close()
        time.sleep(0.3)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, default=ENGINE)
    parser.add_argument("--fixture", type=Path, required=True)
    args = parser.parse_args()
    print(f"Passed {len(run(args.executable, args.fixture)['passed'])} MCP/job checks")
