"""Command line: build, check, resolve, status, show and review. Results are one JSON object on stdout, like the engine's."""
import argparse
import json
import os
import sys
import uuid
from fractions import Fraction as F
from pathlib import Path

from . import manifest as manifest_module
from .engine import Engine, ToolError
from .pipeline import Production, estimate
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


def warn(warnings):
    for w in warnings:
        print(f"WARNING {w['code']} ({w['stage']}): {w['message']}", file=sys.stderr, flush=True)


def delivery_warnings(state, receipts):
    """The warnings of the build that delivered the current export, or None when no recorded build did."""
    sha = (receipts.get("export", {}).get("outputs") or [{}])[0].get("sha256")
    if not sha:
        return None
    for number in reversed(state.revisions()):
        record = state.revision(number)
        if (record.get("delivery") or {}).get("export_sha256") == sha:
            return record.get("warnings", [])
    return None


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
        entry = {"status": status, "human_required": gate in policy["human_required"],
                 "last": {k: latest[k] for k in ("id", "reviewer", "decision", "time")} if latest else None,
                 "advisory": [{k: e[k] for k in ("reviewer", "decision", "time", "reason")} for e in events if e.get("advisory")][-3:]}
        if gate == "final_export":
            # The delivery's open warnings are shown with the gate. An approval holds while every warning was on the
            # list it was given over; one that appeared since makes it stale.
            warnings = delivery_warnings(state, receipts)
            entry["warnings"] = [{"code": w["code"], "stage": w["stage"], "message": w["message"]} for w in warnings or []]
            if warnings is None and receipts.get("export"):
                entry["warnings_unknown"] = "no recorded build delivered this export; build again to check it"
            seen = set((latest or {}).get("warnings", []))
            new = sorted({w["code"] for w in warnings or []} - seen)
            if status == "approved" and new:
                entry["status"] = "stale"
                entry["stale_because"] = f"warning(s) {new} appeared after the approval"
        out[gate] = entry
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
    v = sub.add_parser("resolve", help="resolve a hand-written scene from the last build's take and alignment, optionally with stills")
    v.add_argument("manifest")
    v.add_argument("scene")
    v.add_argument("--root", required=True, help="the production folder of an earlier build")
    v.add_argument("--still", action="append", default=[], help="render the frame on screen at this scene time (repeatable): 2.4, 61/25")
    v.add_argument("--config", help="local tool configuration JSON, for --still; default $CUTBOLT_PRODUCTION_CONFIG")
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
        recipes, index = pixel_stage.art_plan(manifest_module.templated(m), m["colors"], pixel_stage.check_patterns(m["patterns"]))
        scenes = []
        for s in m["scenes"]:
            row = {"id": s["id"], "beat": s["beat"]["type"] if s["beat"] else None, "narrated": bool(s["script"]),
                   "words": len(pixel_stage.words_of(s["script"] or ""))}
            if s["id"] in m["overrides"]["scenes"]:
                row["override"] = m["overrides"]["scenes"][s["id"]]
            scenes.append(row)
        length, warnings = estimate(m)
        warn(warnings)
        result = {"production_id": m["production_id"], "manifest_sha256": m["manifest_sha256"], "scenes": scenes,
                  "art_recipes": sorted(recipes), "labels": sorted(index["labels"]), "estimate": length, "warnings": warnings}
        sequences = {name: {**spec, "frames": spec["last"] - spec["first"] + 1} for name, spec in m["inputs"].items() if isinstance(spec, dict)}
        if sequences:
            result["sequences"] = sequences
        emit({"ok": True, "result": result})

    if args.action == "resolve":
        try:
            m = manifest_module.load(args.manifest)
        except manifest_module.ManifestError as error:
            fail("INVALID_MANIFEST", str(error))
        try:
            stills = [manifest_module.exact_time(t, f"--still {t}", maximum=F(120)) for t in args.still]
        except manifest_module.ManifestError as error:
            fail("INVALID_TIME", str(error))
        root = root_of(args.root)
        if not (root / "state" / "stages").is_dir():
            fail("NOT_BUILT", f"{root} holds no production; build it first (build --until scenes is enough)")
        production = Production(m, root, {})
        state = production.state
        revisions = state.revisions()
        built = state.revision(revisions[-1])["manifest"]["production_id"] if revisions else None
        if built != m["production_id"]:
            fail("WRONG_PRODUCTION", f"{root} holds production {built!r}, not {m['production_id']!r}")
        if stills:
            config = load_config(args.config)
            env = {k: v for k, v in (("CUTBOLT_FFMPEG", config.get("ffmpeg")), ("CUTBOLT_FFPROBE", config.get("ffprobe"))) if v}
            production.engine = Engine(config["engine"], root, env, state, 1)
        try:
            state.acquire()
            result = production.preview(args.scene, stills)
        except (ToolError, StateError) as error:
            fail(getattr(error, "code", "FAILED"), str(error))
        finally:
            state.release()
        emit({"ok": True, "result": result})

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
            fail(getattr(error, "code", "FAILED"), str(error), stages=production.outcomes, warnings=production.warnings)
        except KeyboardInterrupt:
            fail("INTERRUPTED", "stopped; run build again to resume", stages=production.outcomes, warnings=production.warnings)
        if result["warnings"]:
            print(f"build finished with {len(result['warnings'])} warning(s): {', '.join(w['code'] for w in result['warnings'])}",
                  file=sys.stderr, flush=True)
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
                                                "engine_calls", "stages", "error", "warnings") if k in last} if last else None,
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
        notes = ["agent decisions on a human-required gate are kept as advice and do not approve it"] if advisory else []
        if args.gate == "final_export":
            # The decision is made over the delivery's open warnings; a warning that appears later makes an approval stale.
            open_warnings = delivery_warnings(state, receipts) or []
            record["warnings"] = sorted({w["code"] for w in open_warnings})
            if open_warnings and args.decision == "approved":
                notes.append(f"approved over {len(open_warnings)} open warning(s): {', '.join(record['warnings'])}")
        state.add_review(record)
        emit({"ok": True, "result": {"recorded": record["id"], "advisory": advisory, "note": "; ".join(notes) or None, "gates": gates(root)}})


if __name__ == "__main__":
    main()
