"""Local CLI session acceptance checks; every request starts a fresh process."""
from engine import ENGINE
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import sqlite3
import subprocess
import sys
from threading import Barrier

ROOT = Path(__file__).resolve().parents[1]


def run(executable, directory):
    root = directory.resolve()
    if root == ROOT or ROOT in root.parents:
        raise ValueError("Session test files must stay outside the repository")
    root.mkdir(parents=True, exist_ok=False)
    passed = []

    def check(name, condition):
        if not condition:
            raise AssertionError(name)
        passed.append("session." + name)

    def request(payload, expected_error=None):
        process = subprocess.run([str(executable)], input=json.dumps(payload).encode(), capture_output=True, timeout=20)
        response = json.loads(process.stdout)
        if expected_error:
            assert process.returncode == 1 and response["error"]["code"] == expected_error, response
            return response
        assert process.returncode == 0 and response["ok"], response
        return response["result"]

    def call(command, project_id="edit", **fields):
        return request({"command": command, "store_root": str(root), "project_id": project_id, **fields})

    def time(n):
        return {"num": n, "den": 1}

    def trim(n):
        return [{"op": "clip.trim", "clip_id": "a", "source_in": time(0), "duration": time(n)}]

    def apply(key, revision, n, project_id="edit"):
        return {"command": "session.apply", "store_root": str(root), "project_id": project_id,
                "request_id": key, "expected_revision": revision, "operations": trim(n)}

    project = request({"command": "project.create", "id": "edit", "width": 320, "height": 180, "frame_rate": time(25)})
    project = request({"command": "timeline.apply", "project": project, "expected_revision": 0, "operations": [
        {"op": "media.add", "asset": {"id": "source", "path": str(root / "external.mkv"), "duration": time(20)}},
        *[{"op": "clip.append", "clip": {"id": name, "asset_id": "source", "source_in": time(0), "duration": time(5)}} for name in ("a", "b")],
    ]})
    create = {"command": "session.create", "store_root": str(root), "project": project, "request_id": "create"}
    first = request(create)
    saved = call("session.get")
    expected = {**project, "revision": 0}
    check("durable_reopen", first["revision"] == 0 and saved == expected)
    request({**create, "request_id": "other-create"}, "PROJECT_EXISTS")

    preview = call("session.preview", expected_revision=0, operations=trim(3))
    check("preview_without_mutation", preview["duration_after"] == time(8)
          and len(preview["clips"]) == 2 and preview["clips"][1]["after"]["timeline_start"] == time(3)
          and call("session.get") == expected)
    edit = request(apply("one", 0, 3))
    request(apply("two", 1, 2))
    # Parsed JSON is identical despite key order/whitespace changes on the wire.
    retry = dict(reversed(list(apply("one", 0, 3).items())))
    check("retry_after_advance", request(retry) == edit and request(create) == first
          and call("session.get")["revision"] == 2)
    request(apply("one", 0, 4), "REQUEST_ID_CONFLICT")
    request(apply("stale", 0, 4), "REVISION_CONFLICT")
    check("conflicts_rejected", call("session.get")["revision"] == 2)

    bad = apply("fixed", 2, 4)
    bad["operations"].append({"op": "clip.remove", "clip_id": "missing"})
    request(bad, "MISSING_CLIP")
    check("failed_batch_rollback", call("session.get")["clips"][0]["duration"] == time(2)
          and len(call("session.history")["entries"]) == 3)
    check("failed_key_reusable", request(apply("fixed", 2, 4))["revision"] == 3)

    undo = {"command": "session.undo", "store_root": str(root), "project_id": "edit", "request_id": "undo1", "expected_revision": 3}
    receipt = request(undo)
    assert request(undo) == receipt
    assert call("session.get")["clips"][0]["duration"] == time(2)
    call("session.undo", request_id="undo2", expected_revision=4)
    assert call("session.get")["clips"][0]["duration"] == time(3)
    call("session.undo", request_id="undo3", expected_revision=5)
    assert call("session.get")["clips"][0]["duration"] == time(5)
    request({**undo, "request_id": "undo4", "expected_revision": 6}, "NOTHING_TO_UNDO")
    restore = {"command": "session.restore", "store_root": str(root), "project_id": "edit", "request_id": "restore", "expected_revision": 6, "target_revision": 2}
    restored = request(restore)
    check("undo_restore", restored["revision"] == 7 and request(restore) == restored
          and call("session.get")["clips"][0]["duration"] == time(2)
          and call("session.get", revision=3)["clips"][0]["duration"] == time(4))
    cursor = None
    revisions = []
    while True:
        page = call("session.history", before_revision=cursor, limit=3)
        revisions.extend(entry["revision"] for entry in page["entries"])
        cursor = page["next_before_revision"]
        if cursor is None:
            break
    check("paginated_history", revisions == list(reversed(range(8))))

    def race(payloads, errors):
        barrier = Barrier(len(payloads))
        def launch(pair):
            payload, error = pair
            barrier.wait(timeout=10)
            # Return both success/failure for the two different competing requests.
            if error == "either":
                process = subprocess.run([str(executable)], input=json.dumps(payload).encode(), capture_output=True, timeout=20)
                response = json.loads(process.stdout)
                assert process.returncode == (0 if response["ok"] else 1)
                return response
            return request(payload, error)
        with ThreadPoolExecutor(max_workers=len(payloads)) as pool:
            return list(pool.map(launch, zip(payloads, errors)))

    # Include simultaneous initialization of the same, previously absent database.
    concurrent_root = root / "simultaneous"
    concurrent_root.mkdir()
    concurrent_create = {**create, "store_root": str(concurrent_root)}
    results = race([concurrent_create] * 4, [None] * 4)
    check("concurrent_create", all(result == results[0] for result in results))
    same = {**apply("same", 0, 3), "store_root": str(concurrent_root)}
    results = race([same] * 4, [None] * 4)
    check("concurrent_retry", all(result == results[0] for result in results)
          and request({"command": "session.get", "store_root": str(concurrent_root), "project_id": "edit"})["revision"] == 1)
    results = race([apply("writer-a", 7, 3), apply("writer-b", 7, 4)], ["either"] * 2)
    winners = [response for response in results if response["ok"]]
    losers = [response for response in results if not response["ok"]]
    check("concurrent_writers", len(winners) == len(losers) == 1
          and losers[0]["error"]["code"] == "REVISION_CONFLICT" and call("session.get")["revision"] == 8)

    with sqlite3.connect(root / "projects.sqlite3") as connection:
        connection.execute("BEGIN IMMEDIATE")
        request(apply("locked", 8, 3), "STORE_BUSY")
        connection.rollback()
    check("busy_retry", request(apply("locked", 8, 3))["revision"] == 9)
    request({**apply("unknown", 9, 2), "surprise": True}, "INVALID_JSON")
    request({**apply("unknown", 9, 2), "store_root": "relative"}, "INVALID_PATH")
    request({"command": "session.get", "store_root": str(root), "project_id": "missing"}, "PROJECT_NOT_FOUND")
    check("strict_requests", call("session.get")["revision"] == 9)
    (root / "verification.json").write_text(json.dumps({"passed": passed}, indent=2) + "\n", encoding="utf-8")
    return passed


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--executable", type=Path, default=ENGINE)
    parser.add_argument("--output", type=Path, required=True, help="New directory outside the repository")
    args = parser.parse_args()
    result = run(args.executable, args.output)
    print(f"Passed {len(result)} local session checks")
