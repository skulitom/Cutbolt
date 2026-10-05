"""Command line: build, check, status, show and review. Results are one JSON object on stdout, like the engine's."""
import argparse
import json
import os
import sys
import uuid
from pathlib import Path

from . import manifest as manifest_module
from .engine import ToolError
from .pipeline import Production
from .state import State, StateError, now

CONFIG_FIELDS = {"engine", "ffmpeg", "ffprobe", "pixelforge", "tts", "speech_runtime", "lanes", "cache"}
GATE_STAGES = {
    "script": lambda r: {k: v["request"].get("text") for k, v in r.items() if k.startswith("tts:")},
    "storyboard": lambda r: {k: v["key"] for k, v in r.items() if k.startswith(("art:", "scene:"))},
    "narration": lambda r: {k: v["outputs"][0]["sha256"] for k, v in r.items() if k.startswith(("tts:", "narration:")) and v.get("outputs")},
    "rough_cut": lambda r: {"cut": r.get("cut", {}).get("key"), "revision": r.get("cut", {}).get("result", {}).get("revision")},
    "final_export": lambda r: {"export": (r.get("export", {}).get("outputs") or [{}])[0].get("sha256"),
                               "review": r.get("review", {}).get("key"),
                               "revision": r.get("export", {}).get("result", {}).get("project_revision")},
}


def emit(value, code=0):
    print(json.dumps(value, indent=1, ensure_ascii=False))
    sys.exit(code)


def fail(code, message, **extra):
    emit({"ok": False, "error": {"code": code, "message": message, **extra}}, 1)


def load_config(path):
    path = path or os.environ.get("CUTBOLT_PRODUCTION_CONFIG")
    if not path:
        fail("CONFIG_REQUIRED", "give --config or set CUTBOLT_PRODUCTION_CONFIG to the local tool configuration (see docs/PRODUCTION.md)")
    try:
        config = json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail("CONFIG_INVALID", f"{path}: {error}")
    unknown = set(config) - CONFIG_FIELDS - {"notes"}
    missing = {"engine", "pixelforge", "speech_runtime"} - set(config)
    if unknown or missing:
        fail("CONFIG_INVALID", f"{path}: unknown {sorted(unknown)}, missing {sorted(missing)}")
    return config


def root_of(value):
    root = Path(value)
    if not root.is_absolute():
        fail("INVALID_ROOT", "--root must be an absolute local directory")
    repo = Path(__file__).resolve().parents[2]
    resolved = root.resolve()
    if resolved == repo or repo in resolved.parents:
        fail("INVALID_ROOT", "keep productions outside the source repository")
    return resolved


def gates(root, receipts=None):
    state = State(root)
    receipts = receipts if receipts is not None else state.receipts()
    revisions = state.revisions()
    policy = state.revision(revisions[-1])["manifest"]["review_policy"] if revisions else {"gates": list(GATE_STAGES), "human_required": []}
    out = {}
    for gate in policy["gates"]:
        subject = GATE_STAGES[gate](receipts)
        events = [e for e in state.reviews() if e["gate"] == gate]
        counted = [e for e in events if not e.get("advisory")]
        latest = counted[-1] if counted else None
        if latest is None:
            status = "pending"
        elif latest["subject"] != subject:
            status = "stale"
        else:
            status = latest["decision"]
        out[gate] = {"status": status, "human_required": gate in policy["human_required"],
                     "last": {k: latest[k] for k in ("id", "reviewer", "decision", "time")} if latest else None,
                     "advisory": [{k: e[k] for k in ("reviewer", "decision", "time", "reason")} for e in events if e.get("advisory")][-3:]}
    return out


def main(argv=None):
    parser = argparse.ArgumentParser(prog="production", description="Build a narrated video from a production manifest.")
    sub = parser.add_subparsers(dest="action", required=True)
    b = sub.add_parser("build", help="run or resume the whole production")
    b.add_argument("manifest")
    b.add_argument("--root", required=True, help="absolute production folder (the engine workspace), outside the repository")
    b.add_argument("--config", help="local tool configuration JSON; default $CUTBOLT_PRODUCTION_CONFIG")
    b.add_argument("--rebuild", action="append", default=[], help="rerun a stage even if its receipt matches (repeatable): tts:s2, scene, export ...")
    b.add_argument("--until", choices=["scenes", "cut"], help="stop after the scenes, or after the saved cut, before the export")
    c = sub.add_parser("check", help="validate a manifest and show its plan without running anything")
    c.add_argument("manifest")
    s = sub.add_parser("status", help="stages, the last build and review gates")
    s.add_argument("--root", required=True)
    w = sub.add_parser("show", help="one stage's receipt")
    w.add_argument("stage")
    w.add_argument("--root", required=True)
    r = sub.add_parser("review", help="record a review decision bound to the current artifacts")
    r.add_argument("--root", required=True)
    r.add_argument("--gate", required=True, choices=list(GATE_STAGES))
    r.add_argument("--decision", required=True, choices=["approved", "changes_requested"])
    r.add_argument("--reviewer", required=True, help="human:<name> or agent:<name>")
    r.add_argument("--reason", default="")
    args = parser.parse_args(argv)

    if args.action == "check":
        try:
            m = manifest_module.load(args.manifest)
        except manifest_module.ManifestError as error:
            fail("INVALID_MANIFEST", str(error))
        from . import pixel_stage
        recipes, index = pixel_stage.art_plan(m["scenes"], m["colors"], pixel_stage.check_patterns(m["patterns"]))
        emit({"ok": True, "result": {"production_id": m["production_id"], "manifest_sha256": m["manifest_sha256"],
                                     "scenes": [{"id": s["id"], "beat": s["beat"]["type"], "narrated": bool(s["script"]),
                                                 "words": len(pixel_stage.words_of(s["script"] or ""))} for s in m["scenes"]],
                                     "art_recipes": sorted(recipes), "labels": sorted(index["labels"])}})

    if args.action == "build":
        try:
            m = manifest_module.load(args.manifest)
        except manifest_module.ManifestError as error:
            fail("INVALID_MANIFEST", str(error))
        config = load_config(args.config)
        root = root_of(args.root)
        root.mkdir(parents=True, exist_ok=True)
        production = Production(m, root, config, force=args.rebuild, until=args.until)
        try:
            result = production.build()
        except (ToolError, StateError) as error:
            fail(getattr(error, "code", "FAILED"), str(error), stages=production.outcomes)
        except KeyboardInterrupt:
            fail("INTERRUPTED", "stopped; run build again to resume", stages=production.outcomes)
        emit({"ok": True, "result": result})

    root = root_of(args.root)
    state = State(root)
    if args.action == "show":
        receipt = state.receipt(args.stage)
        if receipt is None:
            fail("STAGE_NOT_FOUND", f"no receipt for {args.stage}; stages: {sorted(state.receipts())}")
        emit({"ok": True, "result": receipt})
    if args.action == "status":
        receipts = state.receipts()
        revisions = state.revisions()
        last = state.revision(revisions[-1]) if revisions else None
        emit({"ok": True, "result": {
            "root": str(root), "revisions": len(revisions),
            "last_build": {k: last[k] for k in ("revision", "parent_revision", "manifest_sha256", "started", "finished", "outcome", "elapsed_s",
                                                "engine_calls", "stages", "error") if k in last} if last else None,
            "stages": {name: {"state": r["state"], "attempt": r.get("attempt"), "elapsed_s": r.get("elapsed_s"), "finished": r.get("finished"),
                              "outputs": [o["path"] for o in r.get("outputs", [])][:4]} for name, r in sorted(receipts.items())},
            "review_gates": gates(root, receipts)}})
    if args.action == "review":
        kind, _, name = args.reviewer.partition(":")
        if kind not in ("human", "agent") or not name:
            fail("INVALID_REVIEWER", "--reviewer is human:<name> or agent:<name>")
        receipts = state.receipts()
        revisions = state.revisions()
        if not revisions:
            fail("NOTHING_TO_REVIEW", "build the production first")
        last = state.revision(revisions[-1])
        policy = last["manifest"]["review_policy"]
        if args.gate not in policy["gates"]:
            fail("GATE_NOT_IN_POLICY", f"{args.gate} is not a gate of policy {policy['version']}")
        subject = GATE_STAGES[args.gate](receipts)
        advisory = kind == "agent" and args.gate in policy["human_required"]
        record = {"id": str(uuid.uuid4()), "gate": args.gate, "reviewer_type": kind, "reviewer": args.reviewer, "decision": args.decision,
                  "subject": subject, "policy_version": policy["version"], "production_revision": last["revision"], "time": now(),
                  "reason": args.reason, "advisory": advisory}
        state.add_review(record)
        emit({"ok": True, "result": {"recorded": record["id"], "advisory": advisory,
                                     "note": "agent decisions on a human-required gate are kept as advice and do not approve it" if advisory else None,
                                     "gates": gates(root)}})


if __name__ == "__main__":
    main()
