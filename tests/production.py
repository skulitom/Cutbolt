"""Production coordinator: manifest checks, the pixel-stage template, exact timing, session reconciliation and stage reuse.

Always (no companions needed): strict manifest validation and its rejections; the template's PixelForge recipes and
their invalidation by palette and label changes; every beat type compiled against stand-in PNGs and checked by the
engine's scene.inspect, with cue layers starting on the exact frame of their word; the timing plan against hand-computed
boundaries; reconcile() checked by applying its operations with the engine and comparing the result with the target;
the TTS worker refusing a missing model before loading anything.

With CUTBOLT_PRODUCTION_CONFIG (PixelForge, the Qwen worker and the speech runtime installed): a complete four-scene
production, a repeated build that reuses every stage, a one-line script change that rebuilds only that line's chain
and the delivery, and a palette change that re-renders art and scenes but keeps every take.
"""
from engine import ENGINE
import argparse
import copy
import json
import os
import subprocess
import sys
from fractions import Fraction as F
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from cutbolt_production import manifest as manifest_module, pixel_stage  # noqa: E402
from cutbolt_production.pipeline import arrangement_differences, plan_timing, reconcile  # noqa: E402
from cutbolt_production.engine import ToolError  # noqa: E402
from cutbolt_production.state import State  # noqa: E402


def engine(command, root=None, **args):
    argv = [str(ENGINE)] + (["--workspace", str(root)] if root else [])
    done = subprocess.run(argv, input=json.dumps({"command": command, **args}).encode(), capture_output=True, timeout=300)
    reply = json.loads(done.stdout)
    assert reply.get("ok"), (command, reply)
    return reply["result"]


def base_manifest(inputs):
    return {
        "contract_version": "cutbolt-production-1", "production_id": "fixture-film", "template": {"id": "pixel-stage-explainer", "version": 1},
        "voice": {"speaker": "ryan", "language": "English", "seed": 7},
        "inputs": dict(inputs), "fonts": {"bold": "bold", "regular": "regular"}, "music": {"input": "music", "bpm": 125},
        "patterns": {"stiff": [["stand", 10], ["float", 14], ["stand", 12]]},
        "scenes": [
            {"id": "title", "beat": {"type": "title", "lines": ["A FIXTURE FILM", "IN FOUR SCENES"]}},
            {"id": "one", "script": "Pip waits. Now Pip hops, and the label changes.",
             "beat": {"type": "switch", "steps": [{"label": "WAIT", "motion": "idle"}, {"cue": "hops", "label": "HOP", "motion": "hop"}]}},
            {"id": "two", "script": "Stiff on the left, alive on the right.",
             "beat": {"type": "compare", "left": {"label": "STIFF", "motion": "stiff"}, "right": {"label": "ALIVE", "motion": "hop"}}},
            {"id": "end", "script": "Thanks for watching.", "beat": {"type": "end", "lines": ["Thanks for watching"]}},
        ]}


def every_beat(inputs):
    """One scene of every beat type, for the offline template checks."""
    m = base_manifest(inputs)
    m["scenes"] += [
        {"id": "cards", "script": "Crouch first, push next, then float.",
         "beat": {"type": "cards", "heading": {"label": "THREE POSES", "cue": "crouch"},
                  "cards": [{"pose": "crouch", "word": "CROUCH", "cue": "crouch"}, {"pose": "push", "word": "PUSH", "cue": "push"},
                            {"pose": "float", "word": "FLOAT", "cue": "float"}]}},
        {"id": "strip", "script": "In order it hops. Shuffle it and it stumbles.",
         "beat": {"type": "strip", "order": ["crouch", "push", "float", "land"], "label": "IN ORDER",
                  "then": {"order": ["land", "push", "crouch", "float"], "cue": "shuffle", "label": "SHUFFLED"}}},
        {"id": "recolor", "script": "Change the scarf colour and keep the rest.",
         "beat": {"type": "recolor", "slot": "scarf", "to": "#f2c14e", "part": "SCARF", "show": "change", "swap": {"word": "scarf", "edge": "end"}}},
        {"id": "travel", "script": "Pip travels across the stage.", "beat": {"type": "travel", "label": "TRAVEL", "hops": 3}},
    ]
    return m


def rejected(raw, fragment):
    try:
        manifest_module.validate(raw)
    except manifest_module.ManifestError as error:
        assert fragment in str(error), (fragment, str(error))
        return str(error)
    raise AssertionError(f"accepted a manifest that should fail with {fragment!r}")


def stand_in_art(root, recipes):
    """Solid stand-in PNGs at every recipe's canvas, where PixelForge would write them (no PixelForge needed)."""
    art = {}
    for i, (name, recipe) in enumerate(sorted(recipes.items())):
        folder = root / "art" / name / "frames"
        folder.mkdir(parents=True)
        for frame in recipe["frames"]:
            Image.new("RGBA", (recipe["width"], recipe["height"]), (40 * i % 256, 90, 200, 255)).save(folder / f"{frame['name']}.png")
        art[name] = f"art/{name}"
    return art


def check_offline(out, passed):
    sources = out / "sources"
    sources.mkdir(parents=True)
    for name in ("music.wav", "bold.ttf", "regular.ttf"):
        (sources / name).write_bytes(b"stand-in")
    inputs = {"music": str(sources / "music.wav"), "bold": str(sources / "bold.ttf"), "regular": str(sources / "regular.ttf")}

    # 1. Manifest: a valid one normalizes; each broken one fails with the field named.
    m = manifest_module.validate(every_beat(inputs))
    assert [s["id"] for s in m["scenes"]] == ["title", "one", "two", "end", "cards", "strip", "recolor", "travel"]
    assert m["timing"]["beat"] == F(12, 25) and m["timing"]["snap"] == "beat"
    assert manifest_module.validate(every_beat(inputs))["manifest_sha256"] == m["manifest_sha256"]
    cases = []

    def broken(fragment, change):
        raw = every_beat(inputs)
        change(raw)
        cases.append(rejected(raw, fragment))
    broken("unknown field(s) ['scnes']", lambda r: r.update(scnes=[]))
    broken("contract_version", lambda r: r.update(contract_version="cutbolt-production-draft-1"))
    broken("says 'jumps' 0 time(s)", lambda r: r["scenes"][1]["beat"]["steps"][1].update(cue="jumps"))
    broken("uses characters outside", lambda r: r["scenes"][1]["beat"]["steps"][0].update(label="ÉTÉ"))
    broken("pixels wide in the pixel font", lambda r: r["scenes"][1]["beat"]["steps"][0].update(label="A LABEL MUCH TOO WIDE TO FIT"))
    broken("each step is [pose, 1-250 frames", lambda r: r["patterns"].update(wobble=[["dance", 4]]))
    broken("not a whole number of 25 fps frames", lambda r: r["music"].update(bpm=120))
    broken("absolute local path", lambda r: r["inputs"].update(music="music.wav"))
    broken("absolute local path", lambda r: r["inputs"].update(music=r"\\server\share\music.wav"))
    broken("a unique 1-48 character lowercase id", lambda r: r["scenes"][2].update(id="one"))
    broken("only 'pixel-stage-explainer'", lambda r: r["template"].update(id="other"))
    broken("one or two lines", lambda r: r["scenes"][0]["beat"].update(lines=["A", "B", "C"]))
    broken("timing bars fit motions of at most 58 frames", lambda r: r["patterns"].update(stiff=[["stand", 60]]))
    broken("cues need narration", lambda r: r["scenes"][0].update(beat={"type": "travel", "label": "GO"}) or r["scenes"].append(
        {"id": "silent", "beat": {"type": "switch", "steps": [{"label": "A"}, {"cue": "x", "label": "B"}]}}))
    broken("the scene has no script", lambda r: r.update(overrides={"narration": {"title": {"input": "music"}}}))
    broken("is not declared in inputs", lambda r: r["fonts"].update(bold="missing"))
    broken("first step starts with the scene", lambda r: r["scenes"][1]["beat"]["steps"][0].update(cue="waits"))
    passed.append(f"production.manifest_rejections ({len(cases)} cases)")

    # 2. Template art: every beat type's pictures; a palette change touches every recipe, a label change only the labels.
    patterns = pixel_stage.check_patterns(m["patterns"])
    recipes, index = pixel_stage.art_plan(m["scenes"], m["colors"], patterns)
    assert sorted(recipes) == ["cards", "clouds", "ground", "highlight", "hills-far", "hills-near", "labels", "pip", "pip-scarf-f2c14e",
                               "sky", "strip", "swatch", "timing", "title"], sorted(recipes)
    assert recipes["pip-scarf-f2c14e"]["palette"]["s"] == "#f2c14e" and recipes["pip"]["palette"]["s"] == pixel_stage.SLOTS["scarf"]
    recolored = every_beat(inputs)
    recolored["palette"] = {"outline": "#101010"}
    other, _ = pixel_stage.art_plan(manifest_module.validate(recolored)["scenes"], manifest_module.validate(recolored)["colors"], patterns)
    changed = sorted(n for n in recipes if manifest_module.digest(recipes[n]) != manifest_module.digest(other[n]))
    assert changed == sorted(recipes), changed
    relabeled = every_beat(inputs)
    relabeled["scenes"][1]["beat"]["steps"][1]["label"] = "JUMP"
    m2 = manifest_module.validate(relabeled)
    other, _ = pixel_stage.art_plan(m2["scenes"], m2["colors"], patterns)
    assert [n for n in recipes if manifest_module.digest(recipes[n]) != manifest_module.digest(other[n])] == ["labels"]
    passed.append("production.template_art_invalidation")

    # 3. Every beat compiles to a scene the engine accepts, with cue layers on the frame of their word.
    work = out / "work"
    work.mkdir()
    (work / "fonts").mkdir()
    font = Path(os.environ.get("SYSTEMROOT", r"C:\Windows")) / "Fonts" / "arial.ttf"
    (work / "fonts" / "bold.ttf").write_bytes(font.read_bytes())
    (work / "fonts" / "regular.ttf").write_bytes(font.read_bytes())
    art = stand_in_art(work, recipes)
    lead = m["timing"]["lead"]
    checked = 0
    for number, scene in enumerate(m["scenes"]):
        words = pixel_stage.words_of(scene["script"] or "")
        # A stand-in alignment: word k starts at 0.3 k + 0.013 s, deliberately off the frame grid.
        times = {}
        for k, w in enumerate(words):
            times.setdefault(w, []).append((F(3, 10) * k + F(13, 1000), F(3, 10) * k + F(1, 4)))

        def cue(c, times=times):
            start, end = times[c["word"]][c["nth"]]
            return pixel_stage.snap(lead + (start if c["edge"] == "start" else end))
        duration = F(12) if scene["script"] else F(96, 25)
        built = pixel_stage.SceneBuilder(scene, duration, art, index, patterns, m["colors"], cue,
                                         {"bold": "fonts/bold.ttf", "regular": "fonts/regular.ttf"}, number).build()
        for layer in built["layers"]:
            length = F(layer["duration"]["num"], layer["duration"]["den"])
            assert F(layer["start"]["num"], layer["start"]["den"]) * 25 % 1 == 0, (scene["id"], layer["id"])
            if layer["frames"]:
                assert sum(F(f["hold"]["num"], f["hold"]["den"]) for f in layer["frames"]) == length, (scene["id"], layer["id"])
        starts = {layer["id"]: F(layer["start"]["num"], layer["start"]["den"]) for layer in built["layers"]}
        if scene["id"] == "one":
            # "hops" is word 4: 6/25 + 0.3 * 4 + 0.013 = 1.453 s, nearest frame 36.
            assert starts["label-1"] == starts["pip-8"] == cue({"word": "hops", "nth": 0, "edge": "start"}) == F(36, 25), starts
        if scene["id"] == "cards":
            assert [starts[f"card-{i}"] for i in range(3)] == [cue({"word": w, "nth": 0, "edge": "start"}) for w in ("crouch", "push", "float")]
        if scene["id"] == "recolor":
            assert starts["pip-after"] == cue({"word": "scarf", "nth": 0, "edge": "end"})
        report = engine("scene.inspect", work, scene=built)
        assert report, scene["id"]
        checked += 1
    passed.append(f"production.template_scenes_inspected ({checked} beats)")

    # 4. Timing: boundaries from takes, on the beat grid, exactly; a fixed scene that cannot hold its take fails.
    t = dict(m["timing"])
    rows = plan_timing(t, [
        {"id": "title", "duration": None, "type": "title", "take": None},
        {"id": "a", "duration": None, "type": "switch", "take": {"samples": 24000 * 7 + 5, "rate": 24000}},
        {"id": "end", "duration": None, "type": "end", "take": {"samples": 24000 * 2, "rate": 24000}}])
    # title: 2 bars = 96/25 s; a: 6/25 + 7.0002 + 2/5 = 7.6402 -> 16 beats = 192/25; end: 4 bars = 192/25.
    assert [(r["start"], r["duration"]) for r in rows["scenes"]] == [("0/1", "96/25"), ("96/25", "192/25"), ("288/25", "192/25")], rows
    assert rows["scenes"][1]["voice_start"] == "102/25" and rows["total"] == "96/5"
    try:
        plan_timing(t, [{"id": "a", "duration": "4/1", "type": "switch", "take": {"samples": 24000 * 4, "rate": 24000}}])
        raise AssertionError("an overflowing take was accepted")
    except ToolError as error:
        assert error.code == "NARRATION_OVERFLOW"
    passed.append("production.timing_plan")

    # 5. reconcile(): its operations, applied by the engine to the saved head, give exactly the target arrangement.
    project = engine("project.create", id="fixture", width=320, height=180, frame_rate=25)

    def asset(name, seconds):
        return {"id": name, "path": f"media/{name}.mkv", "duration": {"num": seconds, "den": 1}}

    def place(track, cid, aid, start, length):
        return {"op": "tracks.edit", "edit": {"op": "place", "track_id": track, "collision": "reject", "clip": {
            "id": cid, "asset_id": aid, "start": {"num": start, "den": 1}, "source_in": {"num": 0, "den": 1}, "duration": {"num": length, "den": 1}}}}
    setup = [{"op": "media.add", "asset": asset(n, 30)} for n in ("v1", "v2", "a1", "m1")]
    setup += [{"op": "tracks.edit", "edit": {"op": "create", "duration": {"num": 20, "den": 1}}}]
    setup += [{"op": "tracks.edit", "edit": {"op": "add", "track": {"id": t, "kind": k, "locked": False, "enabled": True, "clips": []}}}
              for t, k in (("picture", "video"), ("voice", "audio"), ("music", "audio"))]
    setup += [place("picture", "v-a", "v1", 0, 10), place("picture", "v-b", "v2", 10, 10), place("voice", "a-a", "a1", 1, 5),
              place("music", "m", "m1", 0, 20),
              {"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": ["m"], "gain_milli": 500, "fade_out": {"num": 2, "den": 1}}}]
    head = engine("timeline.apply", project=project, expected_revision=0, operations=setup)
    change = [{"op": "media.add", "asset": asset("v3", 30)}, {"op": "tracks.edit", "edit": {"op": "remove", "clip_ids": ["v-b", "a-a", "m"], "links": "reject_partial"}},
              {"op": "tracks.edit", "edit": {"op": "duration", "duration": {"num": 22, "den": 1}}},
              place("picture", "v-b", "v3", 10, 12), place("voice", "a-a", "a1", 2, 5), place("music", "m", "m1", 0, 22),
              {"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": ["m"], "gain_curve": {"keys": [
                  {"time": {"num": 0, "den": 1}, "value": 400, "interpolation": "linear"}, {"time": {"num": 3, "den": 1}, "value": 150, "interpolation": "hold"}]},
                  "fade_out": {"num": 2, "den": 1}}},
              {"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": ["a-a"], "gain_milli": 1200}},
              {"op": "tracks.edit", "edit": {"op": "audio_dynamics", "dynamics": {"limiter": {"ceiling_dbfs": -1}}}},
              {"op": "tracks.edit", "edit": {"op": "audio_dynamics", "track_id": "voice", "dynamics": {"limiter": {"ceiling_dbfs": -3, "lookahead_ms": 5, "release_ms": 150}}}}]
    desired = engine("timeline.apply", project=head, expected_revision=head["revision"], operations=change)
    ops = reconcile(head, desired)
    assert reconcile(desired, desired) == []
    result = engine("timeline.apply", project=head, expected_revision=head["revision"], operations=ops)
    strip = lambda s: {k: v for k, v in s.items() if k != "revision"}  # noqa: E731
    assert strip(result) == strip(desired), (ops, result["tracks"], desired["tracks"])
    touched = {c for op in ops if op["op"] == "tracks.edit" for c in (op["edit"].get("clip_ids") or [op["edit"].get("clip", {}).get("id")]) if c}
    assert "v-a" not in touched, touched
    # The same target built from scratch, in another order, as a production build does: reconcile reaches it in what
    # plays, while the saved head keeps its unused asset and its own clip order.
    fresh = [{"op": "media.add", "asset": asset(n, 30)} for n in ("m1", "v3", "a1", "v1")]
    fresh += [op for op in setup if op["op"] == "tracks.edit" and op["edit"]["op"] == "add"]
    fresh.insert(4, {"op": "tracks.edit", "edit": {"op": "create", "duration": {"num": 22, "den": 1}}})
    fresh += [place("music", "m", "m1", 0, 22), place("voice", "a-a", "a1", 2, 5), place("picture", "v-b", "v3", 10, 12), place("picture", "v-a", "v1", 0, 10)]
    fresh += [op for op in change if op["op"] == "tracks.edit" and op["edit"]["op"] in ("clip_audio", "audio_dynamics")]
    target = engine("timeline.apply", project=project, expected_revision=0, operations=fresh)
    assert arrangement_differences(desired, target) == [], arrangement_differences(desired, target)
    trial = engine("timeline.apply", project=head, expected_revision=head["revision"], operations=reconcile(head, target))
    assert strip(trial) != strip(target) and arrangement_differences(trial, target) == []
    moved = copy.deepcopy(target)
    moved["tracks"]["tracks"][1]["clips"][0]["start"] = {"num": 3, "den": 1}
    assert arrangement_differences(trial, moved) == [f"tracks.voice.{moved['tracks']['tracks'][1]['clips'][0]['id']}.start"], arrangement_differences(trial, moved)
    passed.append(f"production.reconcile_round_trip ({len(ops)} operations, unchanged clip untouched)")

    # 6. Review gates bind to identities: a human approval holds until its subject changes; an agent's decision on a
    #    human-required gate is advice; history is appended, never rewritten.
    production = out / "gates"
    state = State(production)
    state.start_revision("m", manifest_module.summary(m))
    take = {"stage": "tts:one", "key": "k1", "state": "completed", "attempt": 1, "request": {"text": "a"},
            "outputs": [{"path": "generated/tts/one.wav", "sha256": "a" * 64, "bytes": 1}]}
    state.save(take)

    def review(gate, decision, reviewer):
        done = subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "review", "--root", str(production),
                               "--gate", gate, "--decision", decision, "--reviewer", reviewer], capture_output=True, timeout=60)
        return json.loads(done.stdout)["result"]
    first = review("final_export", "approved", "agent:checker")
    assert first["advisory"] and first["gates"]["final_export"]["status"] == "pending", first
    assert review("narration", "approved", "human:editor")["gates"]["narration"]["status"] == "approved"
    state.save({**take, "key": "k2", "outputs": [{"path": "generated/tts/one-2.wav", "sha256": "b" * 64, "bytes": 1}]})
    status = json.loads(subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "status", "--root", str(production)],
                                       capture_output=True, timeout=60).stdout)["result"]
    assert status["review_gates"]["narration"]["status"] == "stale", status["review_gates"]
    assert [r["decision"] for r in state.reviews()] == ["approved", "approved"]
    passed.append("production.review_gates_bind_and_stale")

    # 7. The narration worker refuses a missing model before importing any model code (nothing is downloaded).
    request = out / "tts-request.json"
    (out / "tts-out").mkdir()
    request.write_text(json.dumps({"schema": "cutbolt-tts-request-1", "model": str(out / "no-model"), "revision": "x", "speaker": "ryan",
                                   "language": "English", "seed": 1, "output_dir": str(out / "tts-out"),
                                   "lines": [{"id": "a", "text": "Hello.", "file": "a.wav"}]}), encoding="utf-8")
    done = subprocess.run([sys.executable, str(ROOT / "tools" / "qwen_tts_worker.py"), str(request)], capture_output=True, timeout=60)
    reply = json.loads(done.stdout.decode().strip().splitlines()[-1])
    assert done.returncode == 1 and reply["error"]["code"] == "MODEL_MISSING", reply
    assert not any((out / "tts-out").iterdir())
    passed.append("production.worker_missing_model")


def build(manifest_path, root, config, *extra):
    done = subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "build", str(manifest_path), "--root", str(root),
                           "--config", str(config), *extra], capture_output=True, timeout=3600)
    reply = json.loads(done.stdout)
    assert reply["ok"], (reply, done.stderr.decode("utf-8", "replace")[-3000:])
    return reply["result"]


def check_full(out, config, passed):
    """A real production with PixelForge, Qwen and the speech runtime, then reuse and selective rebuilds."""
    music = Path(os.environ.get("CUTBOLT_PRODUCTION_MUSIC", ""))
    bold = Path(os.environ.get("CUTBOLT_PRODUCTION_FONT", ""))
    assert music.is_file() and bold.is_file(), "set CUTBOLT_PRODUCTION_MUSIC (PCM16 WAV) and CUTBOLT_PRODUCTION_FONT (TrueType) for the full run"
    inputs = {"music": str(music), "bold": str(bold), "regular": str(bold)}
    root = out / "film"
    raw = base_manifest(inputs)
    path = out / "film.production.json"
    path.write_text(json.dumps(raw), encoding="utf-8")
    first = build(path, root, config)
    review = first["review"]["summary"]
    assert "black: none" in review and "timing: matches project" in review, review
    speech = first["speech_check"]["summary"]
    assert "100.0%" in speech or "of " in speech, speech
    passed.append(f"production.full_build ({first['duration']} s, {first['elapsed_s']} s)")

    again = build(path, root, config)
    assert again["stages"]["built"] == [], again["stages"]["built"]
    passed.append("production.repeat_reuses_every_stage")

    raw2 = copy.deepcopy(raw)
    raw2["scenes"][2]["script"] = "Stiff on the left, and alive on the right."
    path.write_text(json.dumps(raw2), encoding="utf-8")
    third = build(path, root, config)
    built = set(third["stages"]["built"])
    assert {"tts:two", "align:two", "prepare:two", "scene:two"} <= built, built
    assert not built & {"tts:one", "tts:end", "align:one", "prepare:one", "scene:title", "scene:one", "scene:end"}, built
    assert not any(b.startswith("art:") for b in built), built
    passed.append("production.script_change_rebuilds_one_line")

    raw3 = copy.deepcopy(raw2)
    raw3["palette"] = {"scarf": "#2a9d8f"}
    path.write_text(json.dumps(raw3), encoding="utf-8")
    fourth = build(path, root, config)
    built = set(fourth["stages"]["built"])
    assert "art:pip" in built and "scene:one" in built and not any(b.startswith(("tts:", "align:", "prepare:")) for b in built), built
    passed.append("production.palette_change_keeps_takes")


def run(out, config):
    out = out.resolve()
    assert out != ROOT and ROOT not in out.parents
    out.mkdir(parents=True, exist_ok=True)
    passed = []
    check_offline(out, passed)
    # The full run needs the companions and two inputs; verification without them runs the offline checks only.
    full = bool(config) and bool(os.environ.get("CUTBOLT_PRODUCTION_MUSIC")) and bool(os.environ.get("CUTBOLT_PRODUCTION_FONT"))
    if full:
        check_full(out, config, passed)
    report = {"passed": passed, "full_run": full,
              "scope": "Production manifest, pixel-stage template, exact timing plan, session reconciliation, worker refusal; with a "
                       "production config, a real PixelForge/Qwen/Cutbolt build with stage reuse and selective rebuilds",
              "oracle": "Hand-computed boundaries and cue frames, the engine's own scene.inspect and timeline.apply, recipe digests"}
    (out / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--config", default=os.environ.get("CUTBOLT_PRODUCTION_CONFIG"))
    args = parser.parse_args()
    run(args.output, args.config)
