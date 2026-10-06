"""Production coordinator: manifest checks, the pixel-stage template, exact timing, session reconciliation and stage reuse.

Always (no companions needed): strict manifest validation and its rejections; the template's PixelForge recipes and
their invalidation by palette and label changes; every beat type compiled against stand-in PNGs and checked by the
engine's scene.inspect, with cue layers starting on the exact frame of their word; the timing plan against hand-computed
boundaries; reconcile() checked by applying its operations with the engine and comparing the result with the target;
the TTS worker refusing a missing model before loading anything; the build's order, with stubbed stages, starting
alignment on the voice assets while the music is still being prepared; a hand-written scene override with word cues
and input references, checked against its script, resolved from a stand-in alignment, rendered by the coordinator's
scene stage and re-keyed when a cue time or an input changes; delivery warnings from speech checks and reviews
(an engine-shaped recognition failure, a delivered peak over its target, a short music bed at check and build time);
music looped on whole bars; and the mix's AAC trial encode through the engine, whose decoded audio must equal the
delivered MP4's, with each correction of the limiter ceiling following the documented rule.

With CUTBOLT_PRODUCTION_CONFIG (PixelForge, the Qwen worker and the speech runtime installed): a complete four-scene
production, a repeated build that reuses every stage, a one-line script change that rebuilds only that line's chain
and the delivery, and a palette change that re-renders art and scenes but keeps every take.
"""
from engine import ENGINE, per_frame
import argparse
import copy
import hashlib
import io
import json
import os
import subprocess
import sys
import threading
import wave
from fractions import Fraction as F
from pathlib import Path

import numpy as np
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from cutbolt_production import manifest as manifest_module, override, pixel_stage, quality  # noqa: E402
from cutbolt_production.pipeline import (CODEC_CHECK, Production, arrangement_differences, codec_step, cue_at, estimate, music_clips,  # noqa: E402
                                         plan_timing, reconcile)
from cutbolt_production.engine import Engine, ToolError  # noqa: E402
from cutbolt_production.state import State, sha256_file  # noqa: E402


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
    # final_export shows the delivery's open warnings. An approval records the warnings it was given over, and a
    # warning that appears later (here a peak check of the same export) makes it stale.
    state.save({"stage": "export", "key": "e1", "state": "completed", "attempt": 1, "request": {},
                "outputs": [{"path": "exports/film.mp4", "sha256": "e" * 64, "bytes": 1}], "result": {"project_revision": 2}})
    failed = quality.warning("SPEECH_CHECK_FAILED", "speech-review", "the speech check failed")
    over = quality.warning("PEAK_OVER_TARGET", "review", "the delivered file's true peak is -0.60 dBTP")

    def delivered(warnings):
        record = state.start_revision("m", manifest_module.summary(m))
        state.finish_revision({**record, "outcome": "completed", "warnings": warnings, "delivery": {"export_sha256": "e" * 64}})
    delivered([failed])
    approval = review("final_export", "approved", "human:editor")
    assert approval["note"] == "approved over 1 open warning(s): SPEECH_CHECK_FAILED", approval
    gate = approval["gates"]["final_export"]
    assert gate["status"] == "approved" and [w["code"] for w in gate["warnings"]] == ["SPEECH_CHECK_FAILED"], gate
    assert state.reviews()[-1]["warnings"] == ["SPEECH_CHECK_FAILED"]
    delivered([failed, over])
    status = json.loads(subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "status", "--root", str(production)],
                                       capture_output=True, timeout=60).stdout)["result"]
    gate = status["review_gates"]["final_export"]
    assert gate["status"] == "stale" and "PEAK_OVER_TARGET" in gate["stale_because"], gate
    assert [w["code"] for w in status["last_build"]["warnings"]] == ["SPEECH_CHECK_FAILED", "PEAK_OVER_TARGET"], status["last_build"]
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

    # 8. Order: alignment needs only the voice assets, so it starts while the music is still being prepared; captions
    #    wait for the audio timeline and the alignment. The stages are stubs: nothing runs, no receipt is written.
    order, aligning = [], threading.Event()
    # The build reads these fields between stages (music-length and codec warnings), so the stubs return their shapes.
    music_stub, timing_stub, mixed_stub = {"duration": "8/1"}, {"result": {"total": "8/1"}}, {"result": {"report": {}}}

    class Stubbed(Production):
        def stage_inputs(self):
            return {}

        def stage_art(self):
            return "art"

        def stage_tts(self, inputs):
            return {"one": "take"}

        def stage_timing(self, takes):
            return timing_stub

        def stage_prepare_voice(self, takes):
            order.append("voice")
            return {"one": "voice asset"}

        def stage_music(self, inputs):
            order.append("music started")
            assert aligning.wait(30), "alignment waited for the music"
            order.append("music done")
            return music_stub

        def stage_align(self, voice):
            assert voice == {"one": "voice asset"}, voice
            order.append("align")
            aligning.set()
            return {"one": "aligned"}

        def stage_audio_timeline(self, timing, voice, music):
            order.append("audio timeline")
            return {"timing": timing, "voice": voice, "music": music}

        def stage_mix(self, audio):
            return mixed_stub

        def stage_captions(self, audio, aligned):
            assert audio["music"] is music_stub and aligned == {"one": "aligned"}, (audio, aligned)
            order.append("captions")
            return "captions"

        def stage_scenes(self, timing, aligned, captions, art, inputs):
            assert (timing, captions, art) == (timing_stub, "captions", "art")
            order.append("scenes")
            return "scenes"

    stubbed = Stubbed(m, out / "order", {"engine": str(sources / "music.wav"), "lanes": 2}, until="scenes", log=io.StringIO())
    stubbed.revision = {"revision": 1}
    assert stubbed._build()["stopped_after"] == "scenes"
    assert order.index("voice") < order.index("align") < order.index("music done") < order.index("audio timeline")         < order.index("captions") < order.index("scenes"), order
    passed.append("production.align_starts_before_music_finishes")

    check_overrides(out, inputs, font, passed)


EFFECTS_SCRIPT = "The camera zooms in, then the gold label lands on gold."


def layer(id, canvas, start, duration, position, **extra):
    return {"id": id, "canvas": canvas, "start": start, "duration": duration, "timing": "strict", "end": "hold_last",
            "transform": {"position": position, "crop": [0, 0, *canvas], "scale": 1, "quarter_turns": 0, "opacity": 255}, **extra}


def held(name):
    return [{"image": {"input": name}, "hold": "1/25", "offset": [0, 0], "anchor": [0, 0]}]


def recipe(id, layers):
    return {"scene": {"schema_version": 1, "id": id, "width": 320, "height": 180, "output_scale": 1, "duration": "scene",
                      "background": [20, 20, 40], "color": "srgb_straight_encoded", "audio": None, "layers": layers}}


def effects_recipe():
    """A hand-written scene that times its labels to the narration and names its files by input."""
    return recipe("effects", [
        layer("stage", [320, 180], 0, {"until": "end"}, [0, 0], frames=held("stage")),
        layer("zoom-label", [64, 16], {"cue": "zooms"}, {"until": {"cue": "gold", "offset": "-2/25"}}, [128, 20], frames=held("label"),
              animation={"opacity": {"keys": [{"time": 0, "value": 0, "interpolation": "linear"},
                                              {"time": "4/25", "value": 255, "interpolation": "hold"}]}}),
        layer("gold-label", [64, 16], {"cue": "gold"}, {"until": "end"}, [128, 40], frames=held("label"),
              animation={"position_y": {"keys": [{"time": 0, "value": 40, "interpolation": "linear"},
                                                 {"time": {"cue": {"word": "lands", "edge": "end"}}, "value": 60, "interpolation": "hold"}]}}),
        layer("caption", [120, 30], {"cue": {"word": "gold", "nth": 1}}, {"until": "end"}, [100, 140], frames=[], graphics={
            "kind": "text", "text": "GOLD", "fonts": [{"input": "bold"}], "size": 20, "color": [255, 209, 102, 255], "rect": [0, 0, 120, 30],
            "line_height": 26, "letter_spacing": 0, "align": "left", "wrap": "none", "overflow": "reject"}),
    ])


GALLERY_SCRIPT = "The camera drifts left, then the lamp warms the second frame."


def gallery_recipe():
    """A hand-written 3D scene timed to the narration: the camera drifts and the lamp warms on words, a numbered footage
    frame holds until a word, and an expression dims the wall on another."""
    def vector(value, **curves):
        return {"value": value, **({"animation": curves} if curves else {})}

    def keys(*pairs):
        return {"keys": [{"time": t, "value": v, "interpolation": "linear" if i + 1 < len(pairs) else "hold"} for i, (t, v) in enumerate(pairs)]}

    def plane(node, layer, position, size):
        return {"id": node, "transform": {"position_milli": vector(position), "rotation_mdeg": vector([0, 0, 0]),
                                          "scale_milli": vector([1000, 1000, 1000])},
                "plane": {"layer": layer, "size_milli": size, "material": "lambert", "double_sided": False}}

    def node(id, kind, expression):
        return {"id": id, "kind": kind, "expression": expression}

    def literal(id, value):
        return node(id, "scalar", {"op": "literal", "value": {"type": "scalar", "value": value}})
    reel = [{"image": {"input": "track", "frame": 1}, "hold": "4/25", "offset": [0, 0], "anchor": [0, 0]},
            {"image": {"input": "track", "frame": 2}, "hold": {"until": {"cue": "second"}}, "offset": [0, 0], "anchor": [0, 0]},
            {"image": {"input": "track", "frame": 3}, "hold": "1/25", "offset": [0, 0], "anchor": [0, 0]}]
    document = recipe("gallery", [layer("wall", [320, 180], 0, {"until": "end"}, [0, 0], frames=held("stage")),
                                   layer("reel", [64, 16], {"cue": "camera"}, {"until": "end"}, [0, 0], frames=reel)])
    document["scene"]["geometry"] = {
        "nodes": [plane("wall-plane", "wall", [0, 0, 0], [160000, 90000]), plane("reel-plane", "reel", [0, -20000, 10000], [64000, 16000])],
        "camera": {"position_milli": vector([0, 0, 100000], x=keys(({"cue": "drifts"}, 0), ({"cue": "left"}, -20000))),
                   "target_milli": vector([0, 0, 0]), "up_milli": vector([0, 1000, 0]),
                   "projection": {"kind": "perspective", "vertical_fov_mdeg": {"value": 90000}}, "near_milli": 1000, "far_milli": 1000000},
        "lights": [{"kind": "ambient", "color": [255, 255, 255],
                    "intensity_milli": {"value": 400, "animation": keys(({"cue": "lamp"}, 400), ({"cue": {"word": "warms", "edge": "end"}}, 1200))}}],
        "shadows": "none"}
    document["scene"]["expressions"] = {"schema_version": 1, "seed": 1, "nodes": [
        node("now", "scalar", {"op": "time"}), literal("dims", {"cue": "frame"}), literal("full", {"num": 255, "den": 1}),
        literal("dim", {"num": 128, "den": 1}), node("before", "boolean", {"op": "less", "a": "now", "b": "dims"}),
        node("opacity", "scalar", {"op": "select", "condition": "before", "yes": "full", "no": "dim"})],
        "bindings": [{"layer": "wall", "property": "opacity", "node": "opacity"}]}
    return document


def stand_in_words(script, moved=None):
    """A stand-in alignment: word k from 0.3 k + 0.013 s to 0.3 k + 0.25 s, off the frame grid; `moved` comes 2 frames later."""
    def rt(x):
        return {"num": x.numerator, "den": x.denominator}
    words = []
    for k, text in enumerate(script.split()):
        at = F(3, 10) * k + (F(2, 25) if pixel_stage.words_of(text) == [moved] else 0)
        words.append({"text": text, "start": rt(at + F(13, 1000)), "end": rt(at + F(1, 4))})
    return words


class DirectEngine(Engine):
    """Runs queued commands directly: the job queue's workers need Windows, and these checks are about the coordinator."""

    def job(self, command, args, request_id, lane, label=None, wait_seconds=30):
        return self.call(command, args, label=label)


def check_overrides(out, inputs, font, passed):
    """Hand-written scene overrides: cue words and input references checked with the manifest, resolved by the build."""
    folder = out / "override-inputs"
    folder.mkdir()
    Image.new("RGBA", (320, 180), (60, 120, 200, 255)).save(folder / "stage.png")
    Image.new("RGBA", (64, 16), (255, 209, 102, 255)).save(folder / "label.png")
    (folder / "bold.ttf").write_bytes(font.read_bytes())
    (folder / "effects.json").write_text(json.dumps(effects_recipe()), encoding="utf-8")
    (folder / "still.json").write_text(json.dumps(recipe("travel", [layer("stage", [320, 180], 0, {"until": "end"}, [0, 0], frames=held("stage"))])),
                                       encoding="utf-8")
    (folder / "gallery.json").write_text(json.dumps(gallery_recipe()), encoding="utf-8")
    (folder / "track").mkdir()
    for n, color in ((1, (230, 57, 70, 255)), (2, (42, 157, 143, 255)), (3, (244, 162, 97, 255))):
        Image.new("RGBA", (64, 16), color).save(folder / "track" / f"track_{n:04d}.png")
    files = {name: folder / f"{name}{suffix}" for name, suffix in (("stage", ".png"), ("label", ".png"), ("bold", ".ttf"), ("effects", ".json"),
                                                                   ("still", ".json"), ("gallery", ".json"))}
    track = str(folder / "track" / "track_####.png")

    # 1. The manifest: a scene may have a recipe and no beat; its cues and inputs are checked against the script and inputs.
    def film(recipe_file=None, scene=None):
        raw = every_beat({**inputs, **{k: str(v) for k, v in files.items()}})
        raw["inputs"]["effects"] = str(recipe_file or files["effects"])
        raw["inputs"]["track"] = {"sequence": track}
        raw["scenes"].append(scene or {"id": "effects", "script": EFFECTS_SCRIPT})
        raw["scenes"].append({"id": "gallery", "script": GALLERY_SCRIPT})
        raw["overrides"] = {"scenes": {"effects": {"input": "effects"}, "travel": {"input": "still"}, "gallery": {"input": "gallery"}}}
        return raw
    m = manifest_module.validate(film())
    effects = next(s for s in m["scenes"] if s["id"] == "effects")
    assert effects["beat"] is None, effects
    uses = m["overrides"]["scenes"]["effects"]
    assert [(c["word"], c["nth"], c["edge"], c["offset"]) for c in uses["cues"]] == [
        ("zooms", 0, "start", "0/1"), ("gold", 0, "start", "-2/25"), ("gold", 0, "start", "0/1"), ("lands", 0, "end", "0/1"),
        ("gold", 1, "start", "0/1")], uses["cues"]
    assert uses["inputs"] == ["bold", "label", "stage"] and uses["duration"] == "scene", uses
    # A recipe replaces the whole scene: neither a scene without a beat nor an overridden beat draws template art.
    assert [s["id"] for s in manifest_module.templated(m)] == ["title", "one", "two", "end", "cards", "strip", "recolor"]
    _, art_index = pixel_stage.art_plan(manifest_module.templated(m), m["colors"], pixel_stage.check_patterns(m["patterns"]))
    assert "TRAVEL" not in art_index["labels"], art_index["labels"]
    # check's length estimate plans a scene without a beat as the build's timing does.
    length, _ = estimate(m)
    assert "scenes" in length and [r["id"] for r in length["scenes"]][-2:] == ["effects", "gallery"], length
    # A sequence input is every numbered frame of its pattern; a recipe names frames of it, and cue times may sit in
    # geometry curves, expression literals and frame holds.
    sequenced = m["overrides"]["scenes"]["gallery"]
    assert m["inputs"]["track"] == {"sequence": track, "first": 1, "last": 3}, m["inputs"]["track"]
    assert sequenced["frames"] == {"track": [1, 2, 3]} and sequenced["inputs"] == ["stage"], sequenced
    assert [(c["word"], c["edge"]) for c in sequenced["cues"]] == [("drifts", "start"), ("left", "start"), ("lamp", "start"), ("warms", "end"),
                                                                  ("frame", "start"), ("camera", "start"), ("second", "start")], sequenced["cues"]
    assert [c["at"].removeprefix("overrides.scenes.gallery.") for c in sequenced["cues"][3:]] == [
        "geometry.lights[0].intensity_milli.animation.keys[1].time", "expressions.nodes[1].expression.value.value", "layers[1] (reel).start",
        "layers[1] (reel).frames[1].hold.until"], sequenced["cues"]

    cases = []

    def broken(fragment, change=None, edit=None, scene=None):
        recipe_file = None
        if edit is not None:
            document = effects_recipe()
            edit(document["scene"])
            recipe_file = folder / f"broken-{len(cases)}.json"
            recipe_file.write_text(json.dumps(document), encoding="utf-8")
        raw = film(recipe_file, scene)
        if change:
            change(raw)
        cases.append(rejected(raw, fragment))

    def layers(scene):
        return scene["layers"]
    broken("says 'spins' 0 time(s)", edit=lambda s: layers(s)[1].update(start={"cue": "spins"}))
    broken("cue needs occurrence 2", edit=lambda s: layers(s)[1].update(start={"cue": {"word": "zooms", "nth": 1}}))
    broken("cue needs occurrence 3", edit=lambda s: layers(s)[2]["animation"]["position_y"]["keys"][1].update(time={"cue": {"word": "gold", "nth": 2}}))
    broken("not a whole number of 25 fps frames", edit=lambda s: layers(s)[1].update(start={"cue": "zooms", "offset": "1/50"}))
    broken("a cue time has cue and an optional offset", edit=lambda s: layers(s)[1].update(start={"cue": "zooms", "lead": 1}))
    broken("a hold is a length; hold until a word", edit=lambda s: layers(s)[1]["frames"][0].update(hold={"cue": "zooms"}))
    broken("a hold is an exact time or", edit=lambda s: layers(s)[1]["frames"][0].update(hold={"until": "end"}))
    broken("cue times may be a layer's start", edit=lambda s: s.update(audio={"file": {"input": "stage"}, "start": {"cue": "zooms"}}))
    broken("cue times may be a layer's start", edit=lambda s: s.update(temporal={"shutter_angle": {"cue": "zooms"}, "phase": {"num": 0, "den": 1},
                                                                                 "samples": 1, "integration": "encoded_rgb"}))
    broken("cue times may be a layer's start", edit=lambda s: layers(s)[2]["animation"]["position_y"].update(
        retime={"start": {"cue": "zooms"}, "rate": 1, "reverse": False}))
    broken('{"until": "end"} or {"until": <cue time>}', edit=lambda s: layers(s)[0].update(duration={"until": "later"}))
    broken("'missing' is not declared in inputs", edit=lambda s: layers(s)[0]["frames"][0].update(image={"input": "missing"}))
    broken("input 'bold' must be a .png file here", edit=lambda s: layers(s)[0]["frames"][0].update(image={"input": "bold"}))
    broken("input 'stage' must be a .ttf/.otf file here", edit=lambda s: layers(s)[3]["graphics"].update(fonts=[{"input": "stage"}]))
    broken('an input reference is {"input": name}, or', edit=lambda s: layers(s)[0]["frames"][0].update(image={"input": "stage", "bytes": 1}))
    broken("input 'track' is an image sequence; name one of its frames", edit=lambda s: layers(s)[1]["frames"][0].update(image={"input": "track"}))
    broken("9 is not a frame of sequence 'track' (1-3)", edit=lambda s: layers(s)[1]["frames"][0].update(image={"input": "track", "frame": 9}))
    broken("input 'label' is one file, not an image sequence", edit=lambda s: layers(s)[1]["frames"][0].update(image={"input": "label", "frame": 1}))
    broken("input 'track' is an image sequence; give one file", change=lambda r: r["fonts"].update(bold="track"))
    for name, names in (("gappy", ["g_0001.png", "g_0003.png"]), ("padded", ["p_001.png"])):
        (folder / name).mkdir()
        for file in names:
            Image.new("RGBA", (64, 16)).save(folder / name / file)
    broken("frame 2 is missing; a sequence numbers every frame from 1 to 3", change=lambda r: r["inputs"].update(
        track={"sequence": str(folder / "gappy" / "g_####.png")}))
    broken("p_001.png does not write its number as the pattern does (4 digits", change=lambda r: r["inputs"].update(
        track={"sequence": str(folder / "padded" / "p_####.png")}))
    broken("one run of # in the file name", change=lambda r: r["inputs"].update(track={"sequence": str(folder / "track" / "track_##_##.png")}))
    broken("an image sequence is numbered .png files", change=lambda r: r["inputs"].update(track={"sequence": str(folder / "track" / "track_####.jpg")}))
    broken("no file in", change=lambda r: r["inputs"].update(track={"sequence": str(folder / "track" / "reel_####.png")}))
    broken("is not an exact time", edit=lambda s: s.update(duration="whole"))
    broken("cues need narration", scene={"id": "effects"})
    broken("a scene without a beat needs overrides.scenes.effects", change=lambda r: r["overrides"]["scenes"].pop("effects"))
    broken("cannot read the scene recipe", change=lambda r: r["inputs"].update(effects=str(folder / "absent.json")))
    passed.append(f"production.override_manifest_checks ({len(cases)} rejections)")

    # 2. The scene stage resolves the recipe from the take's alignment and renders it; cue layers start on their words.
    root = out / "override-film"
    for d in ("sources", "renders"):
        (root / d).mkdir(parents=True)
    log = io.StringIO()
    production = Production(m, root, {"engine": str(ENGINE)}, log=log)
    production.engine = DirectEngine(ENGINE, root, {}, production.state, 1)
    production.engine_identity = {"sha256": sha256_file(ENGINE)}
    production.lanes = 1
    production.m = {**m, "scenes": [effects], "overrides": {"narration": {}, "scenes": {"effects": uses}}}
    copies = production.stage_inputs()
    assert copies["stage"] == f"sources/stage-{sha256_file(files['stage'])[:12]}.png", copies

    def scenes(words, copies=copies, length="5/1"):
        production.outcomes.clear()
        production.stage_scenes({"result": {"scenes": [{"id": "effects", "start": "0/1", "duration": length}]}},
                                {"effects": {"words": words}}, None, {}, copies)
        return production.outcomes["scene:effects"], production.state.receipt("scene:effects")
    outcome, receipt = scenes(stand_in_words(EFFECTS_SCRIPT))
    assert outcome == "built", outcome
    resolved = receipt["request"]["override"]
    # Lead 6/25 plus the word's time, to the nearest frame: zooms (word 2) 0.853 s -> 21; gold (6) 2.053 s -> 51, two frames
    # early for the zoom label's end -> 49; lands (8) ends at 2.89 s -> 72; the second gold (10) 3.253 s -> 81.
    assert [(c["word"], c["frame"]) for c in resolved["cues"]] == [("zooms", 21), ("gold", 49), ("gold", 51), ("lands", 72), ("gold", 81)], resolved
    assert resolved["inputs"]["label"] == {"path": copies["label"], "sha256": sha256_file(files["label"]), "bytes": files["label"].stat().st_size}
    assert resolved["source"] == sha256_file(files["effects"]) and resolved["duration"] == "5/1", resolved
    assert "zooms at frame 21" in log.getvalue(), log.getvalue()
    base = json.loads(next((root / "scenes").glob("effects-base-*.json")).read_text(encoding="utf-8"))
    placed = {layer["id"]: (layer["start"], layer["duration"]) for layer in base["layers"]}
    assert placed == {"stage": (0, {"num": 5, "den": 1}), "zoom-label": ({"num": 21, "den": 25}, {"num": 28, "den": 25}),
                      "gold-label": ({"num": 51, "den": 25}, {"num": 74, "den": 25}),
                      "caption": ({"num": 81, "den": 25}, {"num": 44, "den": 25})}, placed
    assert base["layers"][2]["animation"]["position_y"]["keys"][1]["time"] == {"num": 21, "den": 25}
    assert base["layers"][3]["graphics"]["fonts"] == [resolved["inputs"]["bold"]] and base["duration"] == {"num": 5, "den": 1}
    report = engine("scene.inspect", root, scene=base)
    sampled = {layer["layer_id"]: per_frame(layer["sampled_parameters"]) for layer in report["timing"]}
    shown = {k: [i for i, v in enumerate(values) if v is not None] for k, values in sampled.items()}
    assert (shown["zoom-label"], shown["gold-label"], shown["caption"]) == (list(range(21, 49)), list(range(51, 125)), list(range(81, 125))), \
        {k: (v[:1], v[-1:]) for k, v in shown.items()}
    # The gold label settles on the frame where "lands" ends.
    assert [sampled["gold-label"][f]["position"][1] for f in (51, 71, 72, 124)] == [40, 59, 60, 60], [sampled["gold-label"][f] for f in (51, 71, 72)]
    passed.append("production.override_cues_land_on_words (zooms 21, gold 49/51, lands 72, gold 81)")

    # 3. Keys: the same take and inputs reuse the render; a word said later, or a changed image, re-keys the scene.
    assert scenes(stand_in_words(EFFECTS_SCRIPT))[0] == "reused"
    outcome, moved = scenes(stand_in_words(EFFECTS_SCRIPT, moved="lands"))
    assert outcome == "built" and moved["key"] != receipt["key"], outcome
    assert [c["frame"] for c in moved["request"]["override"]["cues"]] == [21, 49, 51, 74, 81], moved["request"]["override"]["cues"]
    Image.new("RGBA", (64, 16), (240, 90, 90, 255)).save(files["label"])
    recopied = production.stage_inputs()
    assert recopied["label"] != copies["label"]
    outcome, relabeled = scenes(stand_in_words(EFFECTS_SCRIPT), recopied)
    assert outcome == "built" and relabeled["key"] not in (receipt["key"], moved["key"]), outcome
    assert relabeled["request"]["override"]["inputs"]["label"]["sha256"] == sha256_file(files["label"])
    passed.append("production.override_keys_follow_cues_and_inputs")

    # 4. Build-time refusals: a cue word the take never says, a recipe longer than its timed scene, a cue after the scene's
    #    end, and a layer that would end where it starts.
    def refused(code, words=None, edit=None, length="5/1"):
        document = effects_recipe()
        if edit:
            edit(document["scene"])
        name = f"sources/refused-{len(cases)}.json"
        (root / name).write_text(json.dumps(document), encoding="utf-8")
        production.m["overrides"]["scenes"]["effects"] = {"input": "refused"}
        try:
            scenes(words or stand_in_words(EFFECTS_SCRIPT), {**recopied, "refused": name}, length)
        except ToolError as error:
            assert error.code == code, (code, error)
            cases.append(str(error))
            return
        raise AssertionError(f"a build accepted an override that should fail with {code}")
    refused("CUE_NOT_HEARD", words=[w for w in stand_in_words(EFFECTS_SCRIPT) if w["text"] != "lands"])
    refused("OVERRIDE_DURATION", edit=lambda s: s.update(duration=4))
    refused("OVERRIDE_TIMING", length="3/1")
    refused("OVERRIDE_TIMING", edit=lambda s: layers(s)[1].update(duration={"until": {"cue": "zooms"}}))
    passed.append("production.override_build_refusals")

    # 5. A 3D gallery: geometry curves and an expression literal on the scene's clock, a footage frame that holds until a word,
    #    and frames of a sequence input, each copied by content.
    def at(frame):
        return {"num": F(frame, 25).numerator, "den": F(frame, 25).denominator}
    gallery = next(s for s in m["scenes"] if s["id"] == "gallery")
    production.m = {**m, "scenes": [gallery], "overrides": {"narration": {}, "scenes": {"gallery": sequenced}}}
    copies = production.stage_inputs()
    assert copies["track"] == {n: f"sources/track/{n}-{sha256_file(folder / 'track' / f'track_{n:04d}.png')[:12]}.png" for n in (1, 2, 3)}, copies
    gallery_words = stand_in_words(GALLERY_SCRIPT)

    def render(copies, words=gallery_words):
        production.outcomes.clear()
        production.stage_scenes({"result": {"scenes": [{"id": "gallery", "start": "0/1", "duration": "132/25"}]}},
                                {"gallery": {"words": words}}, None, {}, copies)
        return production.outcomes["scene:gallery"], production.state.receipt("scene:gallery")
    outcome, shown = render(copies)
    assert outcome == "built", outcome
    resolved = shown["request"]["override"]
    # Lead 6/25 plus the word's time, to the nearest frame: camera (word 1) -> 14, drifts (2) -> 21, left (3) -> 29, lamp (6) -> 51,
    # warms (7) ends at 2.35 s -> 65, second (9) -> 74, frame (10) -> 81.
    cue_frames = [("drifts", 21), ("left", 29), ("lamp", 51), ("warms", 65), ("frame", 81), ("camera", 14), ("second", 74)]
    assert [(c["word"], c["frame"]) for c in resolved["cues"]] == cue_frames, resolved["cues"]
    assert resolved["frames"]["track"]["2"] == {"path": copies["track"][2], "sha256": sha256_file(folder / "track" / "track_0002.png"),
                                                "bytes": (folder / "track" / "track_0002.png").stat().st_size}, resolved["frames"]
    base = json.loads((root / shown["result"]["recipe"]).read_text(encoding="utf-8"))
    geometry = base["geometry"]
    assert [k["time"] for k in geometry["camera"]["position_milli"]["animation"]["x"]["keys"]] == [at(21), at(29)], geometry["camera"]
    assert [k["time"] for k in geometry["lights"][0]["intensity_milli"]["animation"]["keys"]] == [at(51), at(65)], geometry["lights"]
    assert base["expressions"]["nodes"][1]["expression"]["value"]["value"] == at(81), base["expressions"]["nodes"][1]
    reel = base["layers"][1]
    # The second footage frame starts after the first's 4 frames (18) and holds until "second" (74): 56 frames.
    assert reel["start"] == at(14) and [f["hold"] for f in reel["frames"]] == ["4/25", at(56), "1/25"], reel
    assert [f["image"] for f in reel["frames"]] == [resolved["frames"]["track"][str(n)] for n in (1, 2, 3)], reel["frames"]
    report = engine("scene.inspect", root, scene=base)
    states = report["geometry"]["sample_states"]
    assert [states[n]["camera"]["position"][0] for n in (20, 21, 25, 29, 40)] == [0, 0, -10, -20, -20], [states[n]["camera"] for n in (21, 25)]
    assert [round(states[n]["lights"][0]["gain"][0], 3) for n in (50, 51, 58, 65, 70)] == [0.4, 0.4, 0.8, 1.2, 1.2]
    assert [b[0]["rounded"] for b in report["expressions"]["frame_bindings"][79:83]] == [[255], [255], [128], [128]]
    selected = {item["layer_id"]: per_frame(item["selected_frames"]) for item in report["timing"]}
    assert selected["reel"][13:19] == [None, 0, 0, 0, 0, 1] and selected["reel"][72:76] == [1, 1, 2, 2], selected["reel"][:80]
    # A tile's frames play from the layer's start too: a hold until "left" (29) on a layer that starts on "camera" (14) lasts 15 frames.
    tiles = recipe("tiles", [layer("grid", [128, 16], {"cue": "camera"}, {"until": "end"}, [0, 0], frames=[], tilemap={
        "tile_size": [64, 16], "cells": [[0, 0]], "tiles": [{"timing": "strict", "end": "hold_last", "frames": [
            {"image": {"input": "track", "frame": 1}, "hold": {"until": {"cue": "left"}}, "offset": [0, 0], "anchor": [0, 0]},
            {"image": {"input": "track", "frame": 3}, "hold": "1/25", "offset": [0, 0], "anchor": [0, 0]}]}]})])
    tiled, _ = override.prepare(tiles, gallery, m["inputs"], "tiles", override.Resolution(
        cue_at("gallery", gallery_words, m["timing"]["lead"]), lambda name, frame=None: {"input": name, "frame": frame}, F(132, 25)))
    assert [f["hold"] for f in tiled["layers"][0]["tilemap"]["tiles"][0]["frames"]] == [at(15), "1/25"], tiled["layers"][0]["tilemap"]
    passed.append("production.override_scene_clock_and_holds (camera 21-29, lamp 51-65, dimmed from 81, footage frame 2 held 18-73)")

    # 6. A changed footage frame re-copies that frame alone and re-keys the scene; the same frames reuse it.
    Image.new("RGBA", (64, 16), (29, 53, 87, 255)).save(folder / "track" / "track_0002.png")
    production.outcomes.clear()
    recopied = production.stage_inputs()
    assert production.outcomes["input:track"] == "built" and recopied["track"][1] == copies["track"][1] and recopied["track"][2] != copies["track"][2]
    outcome, reshown = render(recopied)
    assert outcome == "built" and reshown["key"] != shown["key"], outcome
    assert reshown["request"]["override"]["frames"]["track"]["2"]["path"] == recopied["track"][2]
    assert render(recopied)[0] == "reused"
    passed.append("production.override_sequence_frames_rekey")

    # 7. Build-time refusals on the new clocks: a hold that would end before its frame starts or after its layer, a geometry
    #    key or expression literal outside the scene, and a geometry cue the take never says.
    def refused_gallery(code, fragment, edit, words=gallery_words):
        document = gallery_recipe()
        edit(document["scene"])
        name = f"sources/refused-{len(cases)}.json"
        (root / name).write_text(json.dumps(document), encoding="utf-8")
        production.m["overrides"]["scenes"]["gallery"] = {"input": "refused"}
        try:
            render({**recopied, "refused": name}, words)
        except ToolError as error:
            assert error.code == code and fragment in str(error), (code, fragment, error)
            cases.append(str(error))
            return
        raise AssertionError(f"a build accepted a gallery that should fail with {code}: {fragment}")
    refused_gallery("OVERRIDE_TIMING", "no later than the frame starts (0.72 s)", lambda s: s["layers"][1]["frames"][1].update(
        hold={"until": {"cue": "camera"}}))
    refused_gallery("OVERRIDE_TIMING", "after the layer ends (5.28 s)", lambda s: s["layers"][1]["frames"][1].update(
        hold={"until": {"cue": "frame", "offset": "3"}}))
    refused_gallery("OVERRIDE_TIMING", "outside the scene (0.00-5.28 s)",
                    lambda s: s["geometry"]["camera"]["position_milli"]["animation"]["x"]["keys"][0].update(time={"cue": "drifts", "offset": "-1"}))
    refused_gallery("OVERRIDE_TIMING", "outside the scene (0.00-5.28 s)", lambda s: s["expressions"]["nodes"][1]["expression"]["value"].update(
        value={"cue": "frame", "offset": "3"}))
    refused_gallery("CUE_NOT_HEARD", "cue word 'lamp'", lambda s: None, words=[w for w in gallery_words if w["text"] != "lamp"])
    production.m["overrides"]["scenes"]["gallery"] = sequenced
    passed.append("production.override_scene_clock_refusals")

    # 8. `resolve` previews the scene from the last build's take and alignment, without a build. It writes the recipe the build
    #    then uses, and its stills are exactly the frames the build renders.
    manifest_path = out / "override.production.json"
    manifest_path.write_text(json.dumps(film()), encoding="utf-8")
    checked = json.loads(subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "check", str(manifest_path)],
                                        capture_output=True, timeout=120).stdout)["result"]
    assert checked["sequences"] == {"track": {"sequence": track, "first": 1, "last": 3, "frames": 3}}, checked.get("sequences")
    assert next(s for s in checked["scenes"] if s["id"] == "gallery")["override"]["frames"] == {"track": [1, 2, 3]}
    preview_root = out / "override-preview"
    builder = Production(m, preview_root, {}, log=io.StringIO())
    record = builder.state.start_revision(m["manifest_sha256"], manifest_module.summary(m))
    builder.state.finish_revision({**record, "outcome": "stopped"})
    builder.stage("timing", "stand-in", {"scenes": [{"id": "gallery", "duration": None, "type": "override", "take": {"samples": 108000, "rate": 24000}}]},
                  lambda attempt: ([], {}))
    builder.stage("align:gallery", "stand-in", {"text": GALLERY_SCRIPT, "asset_sha256": "0" * 64}, lambda attempt: ([], {"words": gallery_words}))
    config = out / "override-config.json"
    config.write_text(json.dumps({"engine": str(ENGINE), "pixelforge": {}, "speech_runtime": {}}), encoding="utf-8")

    def resolve(*extra, manifest=manifest_path, at_root=preview_root):
        done = subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "resolve", str(manifest), *extra,
                               "--root", str(at_root), "--config", str(config)], capture_output=True, timeout=300)
        assert done.stdout, done.stderr.decode("utf-8", "replace")[-3000:]
        return json.loads(done.stdout)
    reply = resolve("gallery", "--still", "3/5", "--still", "3")
    assert reply["ok"], reply
    preview = reply["result"]
    # The 4.5 s take with its lead and tail fills 11 beats at 125 BPM: 5.28 s.
    assert preview["duration"] == "132/25" and [(c["word"], c["frame"]) for c in preview["cues"]] == cue_frames, preview
    assert preview["alignment"]["stage"] == "align:gallery" and [s["frame"] for s in preview["stills"]] == [15, 75], preview["stills"]
    assert preview["frames"]["track"]["2"]["sha256"] == sha256_file(folder / "track" / "track_0002.png"), preview["frames"]
    builder.engine = DirectEngine(ENGINE, preview_root, {}, builder.state, 1)
    builder.engine_identity = {"sha256": sha256_file(ENGINE)}
    builder.lanes = 1
    builder.m = production.m
    builder.mkdir("renders")
    builder.outcomes.clear()
    built = builder.stage_inputs()
    builder.stage_scenes({"result": {"scenes": [{"id": "gallery", "start": "0/1", "duration": "132/25"}]}}, {"gallery": {"words": gallery_words}},
                         None, {}, built)
    rendered = builder.state.receipt("scene:gallery")["result"]
    assert builder.outcomes["input:track"] == "reused" and rendered["recipe"] == preview["recipe"], (builder.outcomes, rendered, preview["recipe"])
    for still in preview["stills"]:
        decoded = subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(preview_root / rendered["asset"]["path"]), "-vf",
                                  f"select=eq(n\\,{still['frame']})", "-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"],
                                 capture_output=True, check=True).stdout
        assert decoded == Image.open(preview_root / still["path"]).convert("RGB").tobytes(), still
    assert resolve("one")["error"]["code"] == "NOT_AN_OVERRIDE"
    assert resolve("gallery", "--still", "6")["error"]["code"] == "INVALID_TIME"
    assert resolve("gallery", at_root=folder)["error"]["code"] == "NOT_BUILT"
    restated = film()
    next(s for s in restated["scenes"] if s["id"] == "gallery")["script"] = GALLERY_SCRIPT + " Twice."
    (out / "override-restated.production.json").write_text(json.dumps(restated), encoding="utf-8")
    assert resolve("gallery", manifest=out / "override-restated.production.json")["error"]["code"] == "STALE_ALIGNMENT"
    passed.append("production.resolve_previews_override (stills at frames 15 and 75 equal the rendered frames)")


def write_wav(path, samples, rate):
    """Original synthetic PCM16 from floats in -1..1, mono (n,) or stereo (n, 2)."""
    data = np.clip(np.round(np.asarray(samples) * 32767), -32768, 32767).astype("<i2")
    with wave.open(str(path), "wb") as w:
        w.setnchannels(1 if data.ndim == 1 else 2)
        w.setsampwidth(2)
        w.setframerate(rate)
        w.writeframes(data.tobytes())


def speech_like(seconds, seed, rate=24000):
    """Voiced bursts with sharp onsets: a sawtooth-rich buzz and hiss, gated like syllables."""
    rng = np.random.default_rng(seed)
    t = np.arange(int(seconds * rate)) / rate
    gate = (np.sin(2 * np.pi * 3.1 * t) > -0.3) * np.minimum(1, t / 0.01)
    buzz = 2 * ((t * (118 + 15 * np.sin(2 * np.pi * 0.6 * t))) % 1) - 1
    signal = gate * (0.7 * buzz + 0.3 * rng.standard_normal(t.size))
    return signal / np.max(np.abs(signal)) * 0.9


def harsh_bed(seconds, seed, rate=48000):
    """Square-wave chords with noise hits on every eighth at 125 BPM: hard on a lossy encoder's peaks."""
    rng = np.random.default_rng(seed)
    t = np.arange(int(seconds * rate)) / rate
    tone = np.sign(np.sin(2 * np.pi * 330 * t)) * 0.3 + np.sign(np.sin(2 * np.pi * 440.5 * t)) * 0.3
    bed = np.stack([tone, np.roll(tone, 37)], 1)
    for hit in np.arange(0, seconds, 0.24):
        i = int(hit * rate)
        bed[i:i + 960] += rng.standard_normal((min(960, t.size - i), 2)) * 0.6
    return bed / np.max(np.abs(bed)) * 0.95


def check_quality(out, passed):
    """Warnings, the music loop, the check-time estimate and the mix's AAC trial, without the companions."""
    # 8. Speech checks: a recognition failure as export.review writes it (the demo's numeral "80"), a check turned off,
    #    one below the threshold, one above it, a silent film, and sounds no word covers.
    failure = {"summary": "speech: 140 words expected; not compared because recognition failed with UNSUPPORTED_ALIGNMENT_TEXT: ...\n",
               "speech": {"expected_words": 140, "unused_transcripts": [], "cut_words": {"count": 0, "listed": []},
                          "recognition": {"ok": False, "error": {"code": "UNSUPPORTED_ALIGNMENT_TEXT", "message": "\"80\" has no letters to align"}}}}
    found = quality.speech_warnings(True, True, failure, 0.95)
    assert [w["code"] for w in found] == ["SPEECH_CHECK_FAILED"] and "UNSUPPORTED_ALIGNMENT_TEXT" in found[0]["message"], found
    assert [w["code"] for w in quality.speech_warnings(False, True, None, 0.95)] == ["SPEECH_CHECK_SKIPPED"]
    assert quality.speech_warnings(False, False, None, 0.95) == [] and quality.speech_warnings(True, False, None, 0.95) == []

    def heard(matched, expected, differences=(), uncovered=0):
        listed = [{"expected": e, "heard": h, "start": {"num": 3 * i + 1, "den": 2}, "end": {"num": 3 * i + 2, "den": 2}}
                  for i, (e, h) in enumerate(differences)]
        speech = {"recognition": {"ok": True}, "comparison": {"expected_words": expected, "heard_words": matched, "matched": matched,
                  "match_ratio": round(matched / expected, 3), "differences": {"count": len(listed), "listed": listed}}}
        if uncovered:
            speech["uncovered"] = {"count": uncovered, "listed": [{"letters": "um", "start": {"num": 2973, "den": 100}, "end": {"num": 303, "den": 10}}]}
        return {"speech": speech}
    low = quality.speech_warnings(True, True, heard(18, 20, [("eighty", "80"), ("pixel", "")]), 0.95)
    assert [w["code"] for w in low] == ["SPEECH_MISMATCH"] and 'expected "eighty" heard "80" at 0.50 s' in low[0]["message"], low
    assert quality.speech_warnings(True, True, heard(136, 138, [("pixelforge", "pixel forge")]), 0.95) == []
    assert [w["code"] for w in quality.speech_warnings(True, True, heard(138, 138, uncovered=1), 0.95)] == ["UNCOVERED_SPEECH"]

    # Reviews of the delivered file: each finding is reported, and a clean review reports nothing.
    def delivered(peak=-1.2, lkfs=-14.0, black=0, clipping=0, silence=0, frames_ok=True, cut=0):
        run = {"start": {"num": 1, "den": 1}, "end": {"num": 2, "den": 1}}
        return {"picture": {"black": {"count": black, "runs": [run] * black}},
                "sound": {"meters": {"integrated_lkfs": lkfs, "sample_peak_dbfs": [peak - 0.4, peak - 0.5], "true_peak_dbtp": [peak, peak - 0.3]},
                          "over_time": {"clipping": {"count": clipping, "runs": [run] * clipping}, "silence": {"count": silence, "runs": [run] * silence}}},
                "timing": {"video": {"ok": frames_ok}, "audio": {"ok": True}}, "speech": {"cut_words": {"count": cut}}}
    assert quality.review_warnings(delivered(), -14, -1, True) == []
    over = quality.review_warnings(delivered(peak=-0.6), -14, -1, True)
    assert [w["code"] for w in over] == ["PEAK_OVER_TARGET"] and "-0.60 dBTP, above the manifest's peak_dbfs of -1" in over[0]["message"], over
    assert quality.review_warnings(delivered(peak=-1.004), -14, -1, True) == []
    every = quality.review_warnings(delivered(lkfs=-16.5, black=1, clipping=2, silence=1, frames_ok=False, cut=1), -14, -1, True)
    assert [w["code"] for w in every] == ["BLACK_PICTURE", "AUDIO_CLIPPING", "AUDIO_SILENCE", "TIMING_MISMATCH", "CUT_WORDS", "LOUDNESS_OFF_TARGET"], every
    assert "AUDIO_SILENCE" not in [w["code"] for w in quality.review_warnings(delivered(silence=1), -14, -1, False)]
    passed.append("production.delivery_warnings")

    # 9. Music: placed once, or repeated on whole bars with the fade on the last repeat; a short bed warns at build
    #    time, and at check time from narration estimated at the speaker's words per second.
    beat = F(12, 25)
    assert music_clips(F(8064, 100), F(9216, 100), "none", beat) == [("m-music", 0, F(8064, 100))]
    # 80.64 s holds 42 whole 1.92 s bars exactly; a 81 s bed loops after the same 42 bars.
    assert music_clips(F(81), F(180), "bars", beat) == [("m-music", 0, F(8064, 100)), ("m-music-2", F(8064, 100), F(8064, 100)),
                                                        ("m-music-3", F(16128, 100), F(180) - F(16128, 100))]
    try:
        music_clips(F(1), F(10), "bars", beat)
        raise AssertionError("a bed shorter than one bar was looped")
    except ToolError as error:
        assert error.code == "MUSIC_TOO_SHORT"
    early = quality.music_warnings("audio-timeline", 80.64, 92.16, "none")
    assert [w["code"] for w in early] == ["MUSIC_ENDS_EARLY"] and "ends 11.52 s before the film" in early[0]["message"], early
    assert quality.music_warnings("audio-timeline", 96.0, 92.16, "none") == [] and quality.music_warnings("audio-timeline", 80.64, 92.16, "bars") == []
    sources = out / "quality-sources"
    sources.mkdir()
    for name in ("bold.ttf", "regular.ttf"):
        (sources / name).write_bytes(b"stand-in")
    # The fixture film: title 2 bars (3.84 s); "one" 9 words / 2.6 + 0.24 + 0.4 s -> 9 beats (4.32 s); "two" 8 words -> 8 beats
    # (3.84 s); the end card at least 4 bars (7.68 s): 19.68 s in all.
    expected = {"words_per_second": 2.6, "rate_measured": True, "narration_seconds": 7.69, "film_seconds": 19.68}
    for seconds, loop, codes in ((16, "none", ["MUSIC_MAY_END_EARLY"]), (21, "none", ["MUSIC_MAY_END_EARLY"]), (22, "none", []), (16, "bars", [])):
        bed = sources / f"bed-{seconds}.wav"
        if not bed.exists():
            write_wav(bed, np.zeros(8000 * seconds), 8000)
        raw = base_manifest({"music": str(bed), "bold": str(sources / "bold.ttf"), "regular": str(sources / "regular.ttf")})
        raw["music"]["loop"] = loop
        length, warnings = estimate(manifest_module.validate(raw))
        assert {k: length[k] for k in expected} == expected and length["music_seconds"] == seconds, length
        assert [w["code"] for w in warnings] == codes, (seconds, loop, warnings)
    assert "would end about 3.7 s before it" in estimate(manifest_module.validate({**raw, "music": {**raw["music"], "loop": "none"}}))[1][0]["message"]
    path = out / "short-bed.production.json"
    path.write_text(json.dumps({**raw, "music": {**raw["music"], "loop": "none"}}), encoding="utf-8")
    done = subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "check", str(path)], capture_output=True, timeout=60)
    result = json.loads(done.stdout)["result"]
    assert [w["code"] for w in result["warnings"]] == ["MUSIC_MAY_END_EARLY"], result
    assert "WARNING MUSIC_MAY_END_EARLY (check): the music bed lasts 16.00 s" in done.stderr.decode("utf-8"), done.stderr
    rejected({**raw, "music": {"input": "music", "loop": "bars"}}, 'music.loop: "bars" needs music.bpm')
    rejected({**raw, "music": {**raw["music"], "loop": "forever"}}, "music.loop")
    rejected({**raw, "delivery": {"review": {"min_speech_match": 1.5}}}, "min_speech_match")
    passed.append("production.music_loop_and_estimate")

    # 10. When the AAC trials stop. A trial over the target lowers the ceiling by the excess plus the margin; one no
    #     better than the best before it stops them (the part-two demo's second trial read +1.40 dBTP after +0.02, and
    #     its third +1.62), as do the cap, the ceiling's floor and an unmeasured encode.
    def trial(ceiling, peak):
        return {"ceiling_dbfs": ceiling, "true_peak_dbtp": peak}
    assert codec_step([trial(-1.3, -1.37)], -1)[0] == "under_target"
    assert codec_step([trial(-1.3, -1.004)], -1)[0] == "under_target"
    assert codec_step([trial(-1.5, 0.02)], -1) == (None, round(-1.5 - 1.02 - CODEC_CHECK["margin_db"], 2))
    stop, reason = codec_step([trial(-1.5, 0.02), trial(-2.57, 1.40)], -1)
    assert stop == "not_better" and "no lower than the best earlier trial's 0.02 dBTP" in reason, reason
    assert codec_step([trial(-1.3, -0.5), trial(-1.85, -0.5)], -1)[0] == "not_better"
    assert codec_step([trial(-1.3, -0.5), trial(-1.85, -0.8)], -1) == (None, round(-1.85 - 0.2 - CODEC_CHECK["margin_db"], 2))
    capped = [trial(-1.3, -0.5), trial(-1.85, -0.8), trial(-2.1, -0.9)]
    assert codec_step(capped, -1)[0] == "trial_cap" and len(capped) == CODEC_CHECK["trials"]
    assert codec_step([trial(-20, -0.5)], -1)[0] == "ceiling_floor"
    assert codec_step([trial(-1.3, None)], -1)[0] == "unmeasured"
    note = quality.codec_note({"trials": [trial(-1.5, 0.02), trial(-2.57, 1.40)], "decision": {"stop": stop, "reason": reason}})
    assert "-1.5 dBFS -> 0.02 dBTP, -2.57 dBFS -> 1.40 dBTP; the trials stopped (not_better): trial 2" in note, note
    passed.append("production.aac_trial_rules")

    # 11. The mix through the engine: a supplied take over a harsh bed looped on bars, normalized loud. Each AAC trial
    #     encodes the mix as the export will; the rules above decide each next ceiling and when to stop, and the
    #     receipt records the decision. The chosen trial's decoded audio equals the delivered MP4's, and its review
    #     reads the same peak.
    root = out / "mix"
    root.mkdir()
    # Other encoder builds may need fewer or more trials, so the rules are checked rather than the count.
    write_wav(sources / "take.wav", speech_like(5.2, 41), 24000)
    write_wav(sources / "bed.wav", harsh_bed(4.0, 42), 48000)
    raw = {"contract_version": "cutbolt-production-1", "production_id": "mix-fixture", "template": {"id": "pixel-stage-explainer", "version": 1},
           "inputs": {"take": str(sources / "take.wav"), "music": str(sources / "bed.wav"), "bold": str(sources / "bold.ttf"),
                      "regular": str(sources / "regular.ttf")},
           "fonts": {"bold": "bold", "regular": "regular"}, "music": {"input": "music", "bpm": 125, "loop": "bars", "bed_db_under_voice": 0},
           "delivery": {"loudness_lkfs": -10, "peak_dbfs": -1},
           "scenes": [{"id": "one", "script": "Pip travels across the stage.", "beat": {"type": "travel", "label": "GO", "hops": 2}}],
           "overrides": {"narration": {"one": {"input": "take"}}}}
    m = manifest_module.validate(raw)
    production = Production(m, root, {"engine": str(ENGINE), "lanes": 2}, log=open(os.devnull, "w"))
    production.engine = Engine(str(ENGINE), root, {}, production.state, 2)
    production.engine_identity, production.lanes = {"sha256": sha256_file(ENGINE)}, 2
    for d in ("sources", "media", "reviews", "exports"):
        production.mkdir(d)
    inputs = production.stage_inputs()
    takes = production.stage_tts(inputs)
    timing = production.stage_timing(takes)
    audio = production.stage_audio_timeline(timing, production.stage_prepare_voice(takes), production.stage_music(inputs))
    total = F(timing["result"]["total"])
    snapshot = json.loads((root / audio["result"]["snapshot"]).read_text(encoding="utf-8"))
    music = [c for t in snapshot["tracks"]["tracks"] if t["id"] == "music" for c in t["clips"]]
    assert [(c["id"], F(c["start"]["num"], c["start"]["den"]), F(c["duration"]["num"], c["duration"]["den"])) for c in music] == \
        [("m-music", 0, F(384, 100)), ("m-music-2", F(384, 100), total - F(384, 100))], music
    assert "fade_out" not in music[0] and music[1]["fade_out"] == {"num": 48, "den": 25}, music
    receipt = production.stage_mix(audio)
    codec = receipt["result"]["report"]["codec"]
    trials = codec["trials"]
    assert trials[0]["ceiling_dbfs"] == round(-1 - CODEC_CHECK["headroom_db"], 2) and 1 <= len(trials) <= CODEC_CHECK["trials"], trials
    for n, (before, after) in enumerate(zip(trials, trials[1:]), 1):
        assert codec_step(trials[:n], -1) == (None, after["ceiling_dbfs"]), trials
    assert codec["decision"] == dict(zip(("stop", "reason"), codec_step(trials, -1))) and codec["headroom_db"] == CODEC_CHECK["headroom_db"], codec
    # Each trial records what the encode added to the mix's own true peak.
    assert all(t["codec_overshoot_db"] == round(t["true_peak_dbtp"] - t["mix_true_peak_dbtp"], 2) for t in trials), trials
    under = [i for i, t in enumerate(trials) if round(t["true_peak_dbtp"], 2) <= -1]
    assert codec["passed"] == bool(under) == (codec["decision"]["stop"] == "under_target"), codec
    assert codec["chosen"] == (under[0] if under else min(range(len(trials)), key=lambda i: trials[i]["true_peak_dbtp"])), codec
    chosen = trials[codec["chosen"]]
    production.engine.call("export.run", {"project": {"file": chosen["snapshot"]}, "output": "exports/mix.mp4", "profile": "h264_aac",
                                          "streams": "audio_video"})

    def pcm(path):
        return hashlib.sha256(subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-i", str(root / path), "-map", "0:a:0", "-f", "s16le", "-"],
                                             capture_output=True, check=True).stdout).hexdigest()
    assert pcm("exports/mix.mp4") == pcm(chosen["file"])
    production.engine.call("export.review", {"path": "exports/mix.mp4", "output": "reviews/mix-delivered", "frames": 1, "rendition_height": 0})
    review = json.loads((root / "reviews" / "mix-delivered" / "review.json").read_text(encoding="utf-8"))
    peak = max(review["sound"]["meters"]["true_peak_dbtp"])
    assert peak == chosen["true_peak_dbtp"], (peak, chosen)
    # The delivered file's true peak is what the review compares with the target.
    assert "PEAK_OVER_TARGET" in [w["code"] for w in quality.review_warnings(review, -10, round(peak - 0.1, 2), True, codec)]
    assert "PEAK_OVER_TARGET" not in [w["code"] for w in quality.review_warnings(review, -10, min(0, round(peak + 0.1, 2)), True, codec)]
    assert bool(quality.codec_warnings(codec, -1)) != codec["passed"]
    passed.append(f"production.mix_aac_trial ({len(trials)} trial(s), {codec['decision']['stop']}, ceiling {chosen['ceiling_dbfs']} dBFS, "
                  f"delivered {chosen['true_peak_dbtp']:.2f} dBTP, codec overshoot {chosen['codec_overshoot_db']:+.2f} dB)")


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
    # Every finding is a warning in the result and the build record; the delivered peak matches the mix's AAC trial.
    codes = [w["code"] for w in first["warnings"]]
    assert all(set(w) == {"code", "stage", "message", "detail"} for w in first["warnings"]), first["warnings"]
    assert first["mix"]["aac_under_peak"] == ("PEAK_OVER_TARGET" not in codes), (first["mix"], codes)
    delivered = json.loads((root / first["review"]["folder"] / "review.json").read_text(encoding="utf-8"))
    assert max(delivered["sound"]["meters"]["true_peak_dbtp"]) == first["mix"]["aac_true_peak_dbtp"], first["mix"]
    status = json.loads(subprocess.run([sys.executable, "-X", "utf8", str(ROOT / "tools" / "production.py"), "status", "--root", str(root)],
                                       capture_output=True, timeout=60).stdout)["result"]
    assert status["last_build"]["warnings"] == first["warnings"] and \
        [w["code"] for w in status["review_gates"]["final_export"]["warnings"]] == codes, status
    passed.append(f"production.full_build ({first['duration']} s, {first['elapsed_s']} s, warnings {codes})")

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
    check_quality(out, passed)
    # The full run needs the companions and two inputs; verification without them runs the offline checks only.
    full = bool(config) and bool(os.environ.get("CUTBOLT_PRODUCTION_MUSIC")) and bool(os.environ.get("CUTBOLT_PRODUCTION_FONT"))
    if full:
        check_full(out, config, passed)
    report = {"passed": passed, "full_run": full,
              "scope": "Production manifest, pixel-stage template, exact timing plan, session reconciliation, worker refusal, build order, "
                       "hand-written scene overrides with word cues and input references, delivery warnings, music loops and "
                       "length estimates, the mix's AAC trial; with a production config, a real PixelForge/Qwen/Cutbolt build "
                       "with stage reuse and selective rebuilds",
              "oracle": "Hand-computed boundaries, cue frames, loop placements and estimates, the engine's own scene.inspect, "
                        "timeline.apply and export.review, decoded AAC from FFmpeg, recipe digests and stage keys"}
    (out / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--config", default=os.environ.get("CUTBOLT_PRODUCTION_CONFIG"))
    args = parser.parse_args()
    run(args.output, args.config)
