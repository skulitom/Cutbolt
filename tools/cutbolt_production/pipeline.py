"""The production stages, their keys and the order they run in.

Every stage has a key: the SHA-256 of its stage version, its normalized request and the identities of what it
depends on. A completed receipt with the same key and intact outputs is reused; anything else runs again. So
the CONTRACT's invalidation rules fall out of the data: a changed script line changes only that line's speech,
alignment, captions and scene, and whatever their timing moves; a palette change re-renders the art and every
scene but keeps every narration take; a new delivery setting only re-exports.

Order (independent branches run at the same time; each stage waits only for the ones named before its arrow):

    inputs → art (PixelForge, every recipe at once), music (an audio-only WAV: as it is, resampled, or the kit cache)
             and tts (one batched Qwen run)
    tts → timing and voice prepare (one job per take)
    voice prepare → align (one job on the voice assets; it waits for nothing else)
    timing, voice prepare, music → audio timeline → mix (meters, duck, normalize, AAC trial)
    align, art → scenes (lanes); audio timeline, align → captions → caption overlay (one transparent render)
    mix, scenes, caption overlay → cut → export → review, beside the speech check (an audio-only render of the same revision)

What the checks find (a speech check that failed, a delivered peak over its target, a music bed that ends early)
becomes a warning: a WARNING line on stderr, the result's `warnings`, the build record and `status`. See quality.py.
"""
import json
import os
import shutil
import sys
import time
import wave
from concurrent.futures import ThreadPoolExecutor
from fractions import Fraction as F
from pathlib import Path

from . import COORDINATOR_VERSION, CONTRACT_VERSION, override as override_module, pixel_stage, quality
from .engine import Engine, PixelForge, Qwen, ToolError
from .manifest import digest, sequence_frame, summary, templated
from .state import State, Timer, now, sha256_file

STAGE_VERSION = "1"
FPS = pixel_stage.FPS
# AAC encoding moves peaks by an amount that depends on the signal. Since the export's encoder runs without noise
# substitution (whose bursts read up to 8 dB over the mix), 15 production mixes measured at most 0.22 dB of codec
# overshoot, more under heavier limiting. The mix therefore aims the limiter ceiling `headroom_db` under the delivery
# peak, encodes the mix to AAC exactly as the export will and measures the decoded true peak. A trial over the target
# lowers the ceiling by the measured excess plus a margin; see codec_step for when the trials stop.
CODEC_CHECK = {"version": 2, "profile": "h264_aac", "headroom_db": 0.3, "margin_db": 0.05, "trials": 3}


def rt(x):
    x = F(x)
    return {"num": x.numerator, "den": x.denominator}


def fr(value):
    return F(value["num"], value["den"]) if isinstance(value, dict) else F(value)


def fstr(x):
    x = F(x)
    return f"{x.numerator}/{x.denominator}"


class Production:
    def __init__(self, manifest, root, config, force=(), until=None, log=None):
        self.m = manifest
        self.root = Path(root).resolve()
        self.cfg = config
        self.force = set(force)
        self.until = until
        self.state = State(self.root)
        self.log_stream = log or sys.stderr
        self.t0 = time.perf_counter()
        self.outcomes = {}
        self.elapsed = {}
        self.warnings = []
        self.delivery = None
        self.codec = None
        self.source_ids = {}

    # ------------------------------------------------------------------ helpers
    def log(self, message):
        print(f"[{time.perf_counter() - self.t0:7.1f}s] {message}", file=self.log_stream, flush=True)

    def note(self, warnings):
        """Record warnings: a WARNING line on stderr, an event, and the build's result and record."""
        for w in warnings:
            self.warnings.append(w)
            self.log(f"WARNING {w['code']} ({w['stage']}): {w['message']}")
            self.state.event({"event": "warning", **w})

    def review_json(self, folder):
        path = self.root / folder / "review.json"
        return json.loads(path.read_text(encoding="utf-8")) if path.exists() else None

    def heard_documents(self, folder, review):
        """The transcripts the speech check heard, kept beside its review.json; empty when there are none."""
        name = ((review or {}).get("speech") or {}).get("transcripts")
        path = self.root / folder / name if isinstance(name, str) else None
        return json.loads(path.read_text(encoding="utf-8"))["transcripts"] if path and path.is_file() else []

    def unused(self, relative):
        """A workspace-relative path that does not exist yet: <relative>, or <stem>-2, -3 ... beside it."""
        base = self.root / relative
        candidate, n = base, 1
        while candidate.exists():
            n += 1
            candidate = base.with_name(f"{base.stem}-{n}{base.suffix}")
        return self.rel(candidate)

    def rel(self, path):
        return Path(path).resolve().relative_to(self.root).as_posix()

    def rel_any(self, path):
        """A path the engine reported, absolute or workspace-relative, as a workspace-relative POSIX path."""
        path = Path(str(path).removeprefix("\\\\?\\"))
        return self.rel(path if path.is_absolute() else self.root / path)

    def mkdir(self, relative):
        path = self.root / relative
        path.mkdir(parents=True, exist_ok=True)
        return path

    def fresh(self, relative):
        """A new directory that did not exist before: <relative>, or <relative>-2, -3 ... when an orphan is in the way."""
        base = self.root / relative
        candidate, n = base, 1
        while candidate.exists():
            n += 1
            candidate = base.with_name(f"{base.name}-{n}")
        candidate.mkdir(parents=True)
        return candidate

    def key(self, stage, request, deps=None):
        return digest({"stage": stage.split(":")[0], "stage_version": STAGE_VERSION, "request": request, "deps": deps or {}})

    def stage(self, name, key, request, run, deps=None, reuse=True):
        """Reuse the completed receipt for `key` (unless `reuse` is false: the caller found it stale), or run
        `run(attempt)` -> (outputs, result) and record it."""
        if reuse and not self.forced(name):
            receipt = self.state.reusable(name, key)
            if receipt:
                self.outcomes[name] = "reused"
                self.state.event({"event": "reused", "stage": name, "key": key})
                return receipt
        prior = self.state.receipt(name)
        # A receipt left `running` or `interrupted` by an earlier coordinator, for the same inputs, is resumed as the same
        # attempt: its outputs and engine job request IDs derive from (key, attempt), so a job that kept running, or
        # finished, while no coordinator watched is collected rather than started again.
        resume = bool(prior) and prior.get("state") in ("running", "interrupted") and prior.get("key") == key and prior.get("pid") != os.getpid()
        attempt = prior["attempt"] if resume else (prior.get("attempt", 0) + 1) if prior else 1
        running = {"stage": name, "key": key, "contract_version": CONTRACT_VERSION, "coordinator_version": COORDINATOR_VERSION,
                   "request": request, "dependencies": deps or {}, "state": "running", "attempt": attempt, "started": now(),
                   "pid": os.getpid(), "outputs": [], "resumed": resume}
        self.state.save(running)
        self.state.event({"event": "resumed" if resume else "started", "stage": name, "key": key, "attempt": attempt})
        timer = Timer()
        try:
            outputs, result = run(attempt)
        except BaseException as error:
            code = getattr(error, "code", type(error).__name__)
            failed = {**running, "state": "interrupted" if isinstance(error, KeyboardInterrupt) else "failed", "finished": now(),
                      "elapsed_s": timer.seconds(), "error": {"code": code, "message": str(error)[:4000]}}
            self.state.save(failed)
            self.state.event({"event": failed["state"], "stage": name, "error": failed["error"]})
            self.outcomes[name] = failed["state"]
            raise
        done = {**running, "state": "completed", "finished": now(), "elapsed_s": timer.seconds(), "outputs": outputs, "result": result}
        self.state.save(done)
        self.state.event({"event": "completed", "stage": name, "key": key, "elapsed_s": done["elapsed_s"]})
        self.outcomes[name] = "built"
        self.elapsed[name] = done["elapsed_s"]
        return done

    def ident(self, relative):
        return self.state.identity(relative)

    def attempt_dir(self, base, attempt):
        """The folder owned by one attempt of a stage: created if missing, kept when that attempt resumes."""
        return self.mkdir(f"{base}-a{attempt}")

    # ------------------------------------------------------------------ run
    def build(self):
        self.state.acquire()
        record = self.state.start_revision(self.m["manifest_sha256"], summary(self.m))
        self.revision = record
        self.log(f"production {self.m['production_id']} revision {record['revision']} in {self.root}")
        try:
            result = self._build()
            record["outcome"] = "stopped" if result.get("stopped_after") else "completed"
            return result
        except BaseException as error:
            record["outcome"] = "failed"
            record["error"] = {"code": getattr(error, "code", type(error).__name__), "message": str(error)[:4000]}
            raise
        finally:
            record["finished"] = now()
            record["stages"] = dict(self.outcomes)
            record["warnings"] = list(self.warnings)
            if self.delivery:
                record["delivery"] = self.delivery
            record["elapsed_s"] = round(time.perf_counter() - self.t0, 2)
            record["engine_calls"] = getattr(getattr(self, "engine", None), "calls", 0)
            self.state.finish_revision(record)
            self.state.release()

    def _build(self):
        cfg = self.cfg
        lanes = int(cfg.get("lanes", 4))
        env = {k: v for k, v in (("CUTBOLT_FFMPEG", cfg.get("ffmpeg")), ("CUTBOLT_FFPROBE", cfg.get("ffprobe"))) if v}
        self.engine = Engine(cfg["engine"], self.root, env, self.state, lanes)
        self.engine_identity = {"sha256": sha256_file(cfg["engine"])}
        self.lanes = lanes
        for d in ("sources", "generated", "media", "scenes", "renders", "reviews", "exports"):
            self.mkdir(d)
        inputs = self.stage_inputs()
        scenes = self.m["scenes"]
        with ThreadPoolExecutor(max_workers=6) as pool:
            art_future = pool.submit(self.stage_art)
            # Music becomes an audio-only asset with no picture to encode, which takes a fraction of a second, so it
            # no longer waits for the narration.
            music_future = pool.submit(self.stage_music, inputs) if self.m["music"] else None
            takes = self.stage_tts(inputs)
            timing = self.stage_timing(takes)
            # Takes become voice assets first and are aligned as those assets, so their transcripts bind to what the
            # timeline plays. Alignment needs nothing else, so it never waits for the music or the audio timeline,
            # and the mix (which needs only the audio) runs while alignment, captions and scenes do.
            voice = self.stage_prepare_voice(takes)
            align_future = pool.submit(self.stage_align, voice)
            music = music_future.result() if music_future else None
            audio = self.stage_audio_timeline(timing, voice, music)
            if music:
                self.note(quality.music_warnings("audio-timeline", float(fr(music["duration"])), float(F(timing["result"]["total"])),
                                                 self.m["music"]["loop"]))
            mix_future = pool.submit(self.stage_mix, audio)
            aligned = align_future.result()
            captions = self.stage_captions(audio, aligned)
            # Burned-in captions are one transparent overlay over the whole film, on a track above the scenes, so they
            # suit every kind of scene (3D scenes bind every layer to a plane) and render while the scenes do.
            burn = self.m["delivery"]["captions"]["burn_in"] and captions["result"]["cues"]
            overlay_future = pool.submit(self.stage_caption_overlay, captions, audio, inputs) if burn else None
            art = art_future.result()
            rendered = self.stage_scenes(timing, aligned, art, inputs)
            overlay = overlay_future.result() if overlay_future else None
            mixed = mix_future.result()
        self.codec = mixed["result"]["report"].get("codec")
        if self.until == "scenes":
            return self.finish(stopped="scenes")
        cut = self.stage_cut(mixed, rendered, timing, overlay)
        if self.until == "cut":
            return self.finish(stopped="cut", cut=cut)
        speech_check = self.m["delivery"]["review"]["speech"] and bool(aligned)
        with ThreadPoolExecutor(max_workers=2) as pool:
            # The speech check listens to the same revision's mix, rendered audio-only, while the picture exports.
            heard_future = pool.submit(self.stage_speech_review, cut, aligned) if speech_check else None
            exported = self.stage_export(cut)
            reviewed = self.stage_review(exported, cut, aligned)
            heard = heard_future.result() if heard_future else None
        review = self.m["delivery"]["review"]
        heard_review = self.review_json(heard["result"]["folder"]) if heard else None
        self.note(quality.speech_warnings(review["speech"], bool(aligned), heard_review, review["min_speech_match"]))
        if heard_review:
            narration = [(F(r["voice_start"]), F(r["voice_start"]) + F(r["take"])) for r in timing["result"]["scenes"] if "take" in r]
            self.note(quality.unnarrated_warnings(heard_review, self.heard_documents(heard["result"]["folder"], heard_review), narration))
        delivered = self.review_json(reviewed["result"]["folder"])
        if delivered is None:
            self.note([quality.warning("REVIEW_MISSING", "review", f"{reviewed['result']['folder']}/review.json is missing")])
        else:
            self.note(quality.review_warnings(delivered, self.m["delivery"]["loudness_lkfs"], self.m["delivery"]["peak_dbfs"],
                                              bool(self.m["music"]), self.codec))
        self.delivery = {"export_sha256": exported["outputs"][0]["sha256"], "review": reviewed["key"],
                         "speech_check": heard["key"] if heard else None}
        return self.finish(cut=cut, exported=exported, reviewed=reviewed, heard=heard, timing=timing, captions=captions, scenes=scenes)

    def finish(self, stopped=None, cut=None, exported=None, reviewed=None, heard=None, timing=None, captions=None, scenes=None):
        if not reviewed:
            # No delivered file was reviewed, so the mix's own AAC trial is the only peak evidence.
            self.note(quality.codec_warnings(self.codec, self.m["delivery"]["peak_dbfs"]))
        built = sorted(k for k, v in self.outcomes.items() if v == "built")
        reused = sorted(k for k, v in self.outcomes.items() if v == "reused")
        result = {"production_id": self.m["production_id"], "root": str(self.root), "revision": self.revision["revision"],
                  "elapsed_s": round(time.perf_counter() - self.t0, 2), "engine_calls": self.engine.calls,
                  "stages": {"built": built, "reused": reused}, "stage_seconds": self.elapsed}
        if stopped:
            result["stopped_after"] = stopped
        if cut:
            result["project"] = {"project_id": self.m["production_id"], "revision": cut["result"]["revision"]}
        if exported:
            result["export"] = exported["result"]
        if captions:
            result["captions"] = captions["result"]["sidecars"]
        if timing:
            result["duration"] = timing["result"]["total"]
            result["scenes"] = [{"id": s["id"], "start": s["start"], "duration": s["duration"]} for s in timing["result"]["scenes"]]
            if timing["result"].get("moved"):
                result["moved"] = timing["result"]["moved"]
        if reviewed:
            result["review"] = {"folder": reviewed["result"]["folder"], "summary": reviewed["result"]["summary"]}
        if heard:
            result["speech_check"] = {"folder": heard["result"]["folder"], "summary": heard["result"]["summary"]}
        if self.codec:
            chosen = self.codec["trials"][self.codec["chosen"]]
            result["mix"] = {"limiter_ceiling_dbfs": chosen["ceiling_dbfs"], "aac_true_peak_dbtp": chosen["true_peak_dbtp"],
                             "aac_trials": len(self.codec["trials"]), "aac_under_peak": self.codec["passed"],
                             "aac_stopped": (self.codec.get("decision") or {}).get("stop")}
        result["warnings"] = list(self.warnings)
        self.state.event({"event": "build_finished", "revision": self.revision["revision"], "built": built, "reused": reused,
                          "warnings": [w["code"] for w in self.warnings]})
        return result

    # ------------------------------------------------------------------ inputs
    def stage_inputs(self, names=None):
        """Copy each input (or only `names`) into sources/ by content: name -> its copy's path, or for an image sequence
        frame number -> path. The copies' identities are kept in `source_ids`, so resolving a recipe hashes nothing again."""
        out = {}
        for name, source in sorted(self.m["inputs"].items()):
            if names is not None and name not in names:
                continue
            if isinstance(source, dict):
                out[name] = self.stage_sequence(name, source)
                continue
            path = Path(source)
            if not path.is_file():
                raise ToolError("production", "INPUT_MISSING", f"inputs.{name}: {source} does not exist")
            sha = sha256_file(path)
            target = f"sources/{name}-{sha[:12]}{path.suffix.lower()}"
            key = self.key("input", {"name": name, "sha256": sha, "target": target})

            def run(attempt, path=path, target=target, sha=sha):
                return [self.import_file(path, target, sha)], {"source": str(path), "path": target}
            receipt = self.stage(f"input:{name}", key, {"name": name, "source": str(path), "sha256": sha}, run)
            self.source_ids.update((o["path"], o) for o in receipt["outputs"])
            out[name] = receipt["result"]["path"]
        return out

    def stage_sequence(self, name, spec):
        """An image sequence input: every frame copied by content to sources/<name>/<number>-<sha12>.png, in one receipt
        whose key covers every frame's SHA-256, so a changed frame re-keys exactly the scenes that show it."""
        numbers = range(spec["first"], spec["last"] + 1)
        paths = {n: sequence_frame(spec, n) for n in numbers}
        missing = [str(p) for p in paths.values() if not p.is_file()]
        if missing:
            raise ToolError("production", "INPUT_MISSING", f"inputs.{name}: {len(missing)} frame(s) of {spec['sequence']} do not exist, "
                            f"first {missing[0]}")
        with ThreadPoolExecutor(max_workers=8) as pool:
            shas = dict(zip(numbers, pool.map(sha256_file, paths.values())))
        digits = len(str(spec["last"]))
        targets = {n: f"sources/{name}/{n:0{digits}d}-{shas[n][:12]}.png" for n in numbers}
        frames = {str(n): shas[n] for n in numbers}
        key = self.key("input", {"name": name, "sequence": frames, "targets": {str(n): t for n, t in targets.items()}})

        def run(attempt):
            self.mkdir(f"sources/{name}")
            with ThreadPoolExecutor(max_workers=8) as pool:
                outputs = list(pool.map(lambda n: self.import_file(paths[n], targets[n], shas[n]), numbers))
            return outputs, {"source": spec["sequence"], "frames": {str(n): targets[n] for n in numbers}}
        receipt = self.stage(f"input:{name}", key, {"name": name, "source": spec["sequence"], "first": spec["first"], "last": spec["last"],
                                                    "sha256": digest(frames)}, run)
        self.source_ids.update((o["path"], o) for o in receipt["outputs"])
        return {int(n): path for n, path in receipt["result"]["frames"].items()}

    def import_file(self, path, target, sha):
        """Copy `path` to the workspace-relative `target` unless an identical copy is there; returns its identity."""
        dest = self.root / target
        if dest.exists():
            if sha256_file(dest) != sha:
                raise ToolError("production", "SOURCE_CHANGED", f"{target} exists with different content")
        else:
            temp = dest.with_name(f".{dest.name}.partial")
            shutil.copyfile(path, temp)
            if sha256_file(temp) != sha:
                temp.unlink()
                raise ToolError("production", "SOURCE_CHANGED", f"{path} changed while it was imported")
            os.replace(temp, dest)
        return self.ident(target)

    # ------------------------------------------------------------------ art
    def stage_art(self):
        timer = Timer()
        # Scenes replaced by a recipe draw nothing from the template.
        recipes, index = pixel_stage.art_plan(templated(self.m), self.m["colors"], pixel_stage.check_patterns(
            {k: v for k, v in self.m["patterns"].items()}))
        self.art_index = index
        pf = PixelForge(self.cfg["pixelforge"], self.state)
        self.mkdir("generated/art")

        def one(name):
            recipe = recipes[name]
            key = self.key(f"art:{name}", recipe, {"pixelforge": pf.identity})

            def run(attempt):
                folder = self.fresh(f"generated/art/{name}-{key[:12]}")
                recipe_path = folder.with_name(folder.name + ".recipe.json")
                recipe_path.write_text(json.dumps(recipe, indent=1), encoding="utf-8")
                report = pf.render(recipe_path, folder)
                frames = sorted((folder / "frames").glob("*.png"))
                if not frames:
                    raise ToolError("pixelforge", "NO_FRAMES", f"{name}: render wrote no frames")
                outputs = [self.ident(self.rel(recipe_path))] + [self.ident(self.rel(f)) for f in frames]
                return outputs, {"dir": self.rel(folder), "frames": [f.stem for f in frames], "warnings": report.get("warnings", [])}
            return name, self.stage(f"art:{name}", key, {"recipe_sha256": digest(recipe)}, run, {"pixelforge": pf.identity})["result"]["dir"]

        with ThreadPoolExecutor(max_workers=8) as pool:
            art = dict(pool.map(one, sorted(recipes)))
        self.log(f"art: {len(art)} PixelForge recipes ready ({timer.seconds():.1f}s)")
        return art

    # ------------------------------------------------------------------ narration
    def stage_tts(self, inputs):
        timer = Timer()
        voice, tts_cfg = self.m["voice"], self.cfg.get("tts") or {}
        worker_sha = sha256_file(Path(__file__).resolve().parents[1] / "qwen_tts_worker.py")
        plan, missing, takes = {}, [], {}
        for scene in self.m["scenes"]:
            if not scene["script"]:
                continue
            override = self.m["overrides"]["narration"].get(scene["id"])
            if override:
                takes[scene["id"]] = self.stage_supplied_take(scene, inputs[override["input"]])
                continue
            request = {"text": scene["script"], "speaker": voice["speaker"], "language": voice["language"], "seed": voice["seed"],
                       "instruct": voice["instruct"], "model_revision": tts_cfg.get("revision"), "worker_sha256": worker_sha}
            if voice["split"] == "sentence":
                request["parts"] = sentences(scene["script"])
                request["pause"] = fstr(voice["pause"])
            key = self.key(f"tts:{scene['id']}", request)
            plan[scene["id"]] = (key, request)
            if self.wanted(f"tts:{scene['id']}", key):
                missing.append(scene["id"])
        generated = {}
        if missing:
            batch_request = {"lines": {s: plan[s][0] for s in missing}}
            batch_key = self.key("tts-batch", batch_request)

            def run_batch(attempt):
                folder = self.attempt_dir(f"generated/tts/{batch_key[:12]}", attempt)
                if not (folder / "receipt.json").exists():
                    if any(folder.iterdir()):
                        # An earlier worker of this attempt stopped part way; its files stay, a new folder takes the run.
                        folder = self.fresh(f"generated/tts/{batch_key[:12]}-a{attempt}-r")
                    request_path = self.root / "state" / f"tts-request-{folder.name}.json"
                    request = {"schema": "cutbolt-tts-request-1", "model": tts_cfg["model"], "revision": tts_cfg.get("revision"),
                               "speaker": voice["speaker"], "language": voice["language"], "seed": voice["seed"], "instruct": voice["instruct"],
                               "batch_size": int(tts_cfg.get("batch_size", 8)), "output_dir": _linux(folder),
                               "lines": [line_request(s, plan[s][0], plan[s][1]) for s in missing]}
                    request_path.write_text(json.dumps(request, indent=1), encoding="utf-8")
                    self.log(f"tts: synthesizing {len(missing)} line(s) in one model load")
                    receipt, elapsed = Qwen(tts_cfg, self.state).synthesize(request_path)
                    self.log(f"tts: {len(missing)} take(s) in {elapsed:.1f}s (model load {receipt['load_seconds']}s, "
                             f"generation {sum(b['seconds'] for b in receipt['batches']):.1f}s)")
                else:
                    # The worker finished while no coordinator was watching: adopt its complete, receipted takes.
                    receipt = json.loads((folder / "receipt.json").read_text(encoding="utf-8"))
                    self.log(f"tts: adopted {len(receipt['lines'])} finished take(s) from {self.rel(folder)}")
                outputs = [self.ident(self.rel(folder / "receipt.json"))] + [self.ident(self.rel(folder / line["file"])) for line in receipt["lines"]]
                return outputs, {"folder": self.rel(folder), "receipt": receipt}
            batch = self.stage("tts-batch", batch_key, batch_request, run_batch)
            folder, receipt = batch["result"]["folder"], batch["result"]["receipt"]
            for line in receipt["lines"]:
                generated[line["id"]] = (f"{folder}/{line['file']}", line, receipt)
        for scene_id, (key, request) in plan.items():
            def run(attempt, scene_id=scene_id):
                path, line, receipt = generated[scene_id]
                identity = self.ident(path)
                if identity["sha256"] != line["sha256"]:
                    raise ToolError("qwen-tts", "TAKE_CHANGED", f"{path} does not match the worker's receipt")
                result = {"path": path, "sample_rate": line["sample_rate"], "channels": line["channels"], "samples": line["samples"],
                          "float_peak": line["float_peak"], "worker": {k: receipt[k] for k in ("model", "revision", "torch", "cuda_device",
                                                                                                "load_seconds", "batches", "peak_gpu_allocated_mib")}}
                return [identity], result
            receipt = self.stage(f"tts:{scene_id}", key, request, run)
            takes[scene_id] = receipt["result"]
        if not missing:
            self.log(f"tts: all {len(plan)} take(s) reused ({timer.seconds():.1f}s)" if plan else "tts: nothing to synthesize (every take is supplied)")
        return takes

    def forced(self, name):
        """--rebuild names a stage (tts:s2), a kind (tts, scene) or a batch; forcing lines of a kind forces its batch too."""
        kind = name.split(":")[0]
        if name in self.force or kind in self.force:
            return True
        return kind.endswith("-batch") and any(f.split(":")[0] == kind[:-len("-batch")] for f in self.force)

    def wanted(self, name, key):
        """True when a stage must run: forced, or no reusable receipt for this key."""
        return self.forced(name) or not self.state.reusable(name, key)

    def stage_supplied_take(self, scene, path):
        identity = self.ident(path)
        key = self.key(f"narration:{scene['id']}", {"sha256": identity["sha256"]})

        def run(attempt):
            with wave.open(str(self.root / path)) as w:
                if w.getsampwidth() != 2:
                    raise ToolError("production", "UNSUPPORTED_MEDIA", f"{path}: narration must be PCM16 WAV")
                result = {"path": path, "sample_rate": w.getframerate(), "channels": w.getnchannels(), "samples": w.getnframes(), "supplied": True}
            return [identity], result
        return self.stage(f"narration:{scene['id']}", key, {"sha256": identity["sha256"], "path": path}, run)["result"]

    def stage_align(self, voice):
        """Word times for every voice asset, by aligning its known script (one engine job for every take that needs it).
        The documents are bound to the assets themselves, so captions, checks and reviews match them directly."""
        timer = Timer()
        runtime = self.cfg["speech_runtime"]
        runtime_digest = digest(runtime)
        plan, missing = {}, []
        for scene in self.m["scenes"]:
            if scene["id"] not in voice:
                continue
            asset = voice[scene["id"]]["asset"]
            request = {"asset_sha256": asset["identity"]["sha256"] if asset.get("identity") else self.state.identity(asset["path"])["sha256"],
                       "text": scene["script"], "language": self.m["language"], "runtime": runtime_digest}
            key = self.key(f"align:{scene['id']}", request)
            take = {"path": asset["path"]}
            plan[scene["id"]] = (key, request, take)
            if self.wanted(f"align:{scene['id']}", key):
                missing.append(scene["id"])
        documents = {}
        if missing:
            batch_request = {"lines": {s: plan[s][0] for s in missing}}
            batch_key = self.key("align-batch", batch_request, {"engine": self.engine_identity})

            def run_batch(attempt):
                self.mkdir("generated/align")
                out = f"generated/align/{batch_key[:12]}-a{attempt}.json"
                self.engine.job("media.transcribe", {"paths": [plan[s][2]["path"] for s in missing], "texts": [plan[s][1]["text"] for s in missing],
                                                     "output": out, "runtime": runtime, "language": self.m["language"]},
                                request_id=f"align-{batch_key[:20]}-{attempt}", lane="speech", label="align")
                return [self.ident(out)], {"output": out}
            batch = self.stage("align-batch", batch_key, batch_request, run_batch, {"engine": self.engine_identity})
            out = batch["result"]["output"]
            docs = json.loads((self.root / out).read_text(encoding="utf-8"))["transcripts"]
            for scene_id in missing:
                path = plan[scene_id][2]["path"]
                mine = [d for d in docs if d["source"]["path"] == path]
                if len(mine) != 1:
                    raise ToolError("cutbolt", "ALIGNMENT_MISSING", f"{path}: expected one aligned document, got {len(mine)}")
                documents[scene_id] = (out, mine[0])
            if self.outcomes.get("align-batch") == "built":
                self.log(f"align: {len(missing)} known script(s) aligned in one job ({timer.seconds():.1f}s)")
        aligned = {}
        for scene_id, (key, request, take) in plan.items():
            def run(attempt, scene_id=scene_id, key=key):
                out, document = documents[scene_id]
                path = f"generated/align/{scene_id}-{key[:12]}.json"
                if not (self.root / path).exists():
                    write_new(self.root / path, {"transcripts": [document]})
                words = [{"text": w["text"], "start": w["start"], "end": w["end"]} for w in document["words"]]
                return [self.ident(path)], {"path": path, "batch": out, "words": words}
            aligned[scene_id] = self.stage(f"align:{scene_id}", key, request, run)["result"]
        return aligned

    # ------------------------------------------------------------------ timing
    def stage_timing(self, takes):
        t = self.m["timing"]
        beat = t["beat"]
        grid = {"frame": F(1, FPS), "beat": beat, "bar": 4 * beat}[t["snap"]]
        request = {"snap": t["snap"], "grid": fstr(grid), "lead": fstr(t["lead"]), "tail": fstr(t["tail"]), "min": fstr(t["min_scene"]),
                   "title_bars": t["title_bars"], "end_min_bars": t["end_min_bars"],
                   "scenes": [{"id": s["id"], "duration": fstr(s["duration"]) if s["duration"] else None,
                               "type": s["beat"]["type"] if s["beat"] else "override",
                               "take": {"samples": takes[s["id"]]["samples"], "rate": takes[s["id"]]["sample_rate"]} if s["id"] in takes else None}
                              for s in self.m["scenes"]]}
        key = self.key("timing", request)
        previous = self.state.receipt("timing")

        def run(attempt):
            result = plan_timing(t, request["scenes"])
            rows = result["scenes"]
            if previous and previous.get("state") == "completed":
                old = {r["id"]: r for r in previous["result"]["scenes"]}
                moved = [{"id": r["id"], "old": {"start": old[r["id"]]["start"], "duration": old[r["id"]]["duration"]},
                          "new": {"start": r["start"], "duration": r["duration"]}}
                         for r in rows if r["id"] in old and (old[r["id"]]["start"], old[r["id"]]["duration"]) != (r["start"], r["duration"])]
                if moved:
                    result["moved"] = moved
            return [], result
        receipt = self.stage("timing", key, request, run)
        if receipt["result"].get("moved") and self.outcomes.get("timing") == "built":
            self.log(f"timing: {len(receipt['result']['moved'])} scene(s) moved or changed length (ripple); see the timing receipt")
        return receipt

    # ------------------------------------------------------------------ media
    def empty_project(self):
        if not hasattr(self, "_empty"):
            self._empty = self.engine.call("project.create", {"id": self.m["production_id"], "width": pixel_stage.SIZE[0],
                                                              "height": pixel_stage.SIZE[1], "frame_rate": FPS})
        return self._empty

    def music_request(self, inputs):
        path = inputs[self.m["music"]["input"]]
        request = {"sha256": self.state.identity(path)["sha256"], "path": path, "asset": "audio-only"}
        return path, request, self.key("prepare:music", request, {"engine": self.engine_identity})

    def stage_music(self, inputs):
        """The music bed as an audio-only asset: a 48 kHz stereo PCM16 WAV is used as it is, and other PCM16 WAVs are
        resampled into one. The kit cache (config `cache`) keeps one resampled copy per music file and engine build, so a
        second production in the same style copies it instead of converting again."""
        timer = Timer()
        path, request, key = self.music_request(inputs)
        cache = Path(self.cfg["cache"]) / "music" / key if self.cfg.get("cache") else None

        def run(attempt):
            if cache and (cache / "record.json").exists():
                record = json.loads((cache / "record.json").read_text(encoding="utf-8"))
                target = self.root / record["asset"]["path"]
                if not target.exists():
                    target.parent.mkdir(parents=True, exist_ok=True)
                    temp = target.with_name(f".{target.name}.partial")
                    shutil.copyfile(cache / target.name, temp)
                    os.replace(temp, target)
                identity = self.ident(record["asset"]["path"])
                if identity["sha256"] == record["sha256"]:
                    return [identity], {"asset": record["asset"], "kit_cache": str(cache)}
                self.state.event({"event": "cache_mismatch", "stage": "prepare:music", "path": record["asset"]["path"]})
            folder = self.attempt_dir(f"media/music-{key[:12]}", attempt)
            result = self.engine.job("media.prepare", {"path": path, "output_root": self.rel(folder)},
                                     request_id=f"music-{key[:20]}-{attempt}", lane="music", label="music")
            asset = result["asset"]
            identity = self.ident(asset["path"])
            if cache and result["converted"] and not cache.exists():
                temp = cache.with_name(f".{cache.name}.partial-{os.getpid()}")
                temp.mkdir(parents=True)
                shutil.copyfile(self.root / asset["path"], temp / Path(asset["path"]).name)
                (temp / "record.json").write_text(json.dumps({"asset": asset, "sha256": identity["sha256"], "request": request}), encoding="utf-8")
                try:
                    os.replace(temp, cache)
                except OSError:
                    shutil.rmtree(temp, ignore_errors=True)
            return [identity], {"asset": asset}
        receipt = self.stage("prepare:music", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("prepare:music") == "built":
            self.log(f"music: {'copied from the kit cache' if receipt['result'].get('kit_cache') else 'prepared'} ({timer.seconds():.1f}s)")
        return receipt["result"]["asset"]

    def stage_prepare_voice(self, takes):
        """Each take as an audio-only voice asset: a 48 kHz stereo PCM16 WAV, as it is or resampled, with no picture.
        One job per take, on separate lanes."""
        timer = Timer()

        def one(item):
            number, scene_id = item
            take = takes[scene_id]
            request = {"take": self.state.identity(take["path"])["sha256"], "asset": "audio-only"}
            key = self.key(f"prepare:{scene_id}", request, {"engine": self.engine_identity})

            def run(attempt):
                folder = self.attempt_dir(f"media/voice-{scene_id}-{key[:12]}", attempt)
                result = self.engine.job("media.prepare", {"path": take["path"], "output_root": self.rel(folder)},
                                         request_id=f"voice-{key[:20]}-{attempt}", lane=f"lane-{number % self.lanes}", label="voice")
                asset = result["asset"]
                return [self.ident(asset["path"])], {"asset": asset}
            return scene_id, self.stage(f"prepare:{scene_id}", key, request, run, {"engine": self.engine_identity})["result"]

        order = [s["id"] for s in self.m["scenes"] if s["id"] in takes]
        with ThreadPoolExecutor(max_workers=self.lanes) as pool:
            voice = dict(pool.map(one, enumerate(order)))
        built = sum(1 for s in order if self.outcomes.get(f"prepare:{s}") == "built")
        if built:
            self.log(f"voice: {built} take(s) prepared for the voice track ({timer.seconds():.1f}s)")
        return voice

    # ------------------------------------------------------------------ timeline (audio)
    def stage_audio_timeline(self, timing, voice, music):
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        total = F(timing["result"]["total"])
        ops = [{"op": "media.add", "asset": voice[s]["asset"]} for s in sorted(voice)]
        if music:
            ops.append({"op": "media.add", "asset": music})
        ops += [{"op": "project.transfer", "transfer": "bt709"},
                {"op": "tracks.edit", "edit": {"op": "create", "duration": rt(total)}}]
        for track, kind in (("picture", "video"), ("voice", "audio"), ("music", "audio")):
            ops.append({"op": "tracks.edit", "edit": {"op": "add", "track": {"id": track, "kind": kind, "locked": False, "enabled": True, "clips": []}}})
        for scene_id in [s["id"] for s in self.m["scenes"] if s["id"] in voice]:
            row = rows[scene_id]
            asset = voice[scene_id]["asset"]
            length = min(fr(asset["duration"]), total - F(row["voice_start"]))
            ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "voice", "collision": "reject", "clip": {
                "id": f"a-{scene_id}", "asset_id": asset["id"], "start": rt(F(row["voice_start"])), "source_in": rt(0), "duration": rt(length)}}})
        if music:
            clips = music_clips(fr(music["duration"]), total, self.m["music"]["loop"], self.m["timing"]["beat"])
            for clip_id, start, length in clips:
                ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "music", "collision": "reject", "clip": {
                    "id": clip_id, "asset_id": music["id"], "start": rt(start), "source_in": rt(0), "duration": rt(length)}}})
            last, _, length = clips[-1]
            fade = min(self.m["music"]["fade_out"], length)
            if fade > 0:
                ops.append({"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": [last], "fade_out": rt(fade)}})
        request = {"ops": ops}
        key = self.key("audio-timeline", request, {"engine": self.engine_identity})

        def run(attempt):
            snapshot = self.engine.call("timeline.apply", {"project": self.empty_project(), "expected_revision": 0, "operations": ops})
            path = f"generated/timeline/audio-{key[:12]}.json"
            self.mkdir("generated/timeline")
            write_new(self.root / path, snapshot)
            return [self.ident(path)], {"snapshot": path}
        return self.stage("audio-timeline", key, request, run, {"engine": self.engine_identity})

    def transcripts_file(self, aligned):
        """Every voice asset's aligned transcript in one file, for captions, checks and reviews."""
        docs = []
        for scene_id in sorted(aligned):
            docs += json.loads((self.root / aligned[scene_id]["path"]).read_text(encoding="utf-8"))["transcripts"]
        path = f"generated/timeline/transcripts-{digest(docs)[:12]}.json"
        if not (self.root / path).exists():
            write_new(self.root / path, {"transcripts": docs})
        return path

    def stage_captions(self, audio, aligned):
        cap = self.m["delivery"]["captions"]
        transcripts = self.transcripts_file(aligned)
        request = {"audio": audio["key"], "transcripts": self.state.identity(transcripts)["sha256"], "line_chars": cap["line_chars"],
                   "lines": cap["lines"], "sidecars": cap["sidecars"]}
        key = self.key("captions", request, {"engine": self.engine_identity})

        def run(attempt):
            self.mkdir("generated/captions")
            draft = f"generated/captions/draft-{key[:12]}-{attempt}.json"
            self.engine.call("captions.draft", {"project": {"file": audio["result"]["snapshot"]}, "transcripts": {"file": transcripts, "select": "transcripts"},
                                                "track_ids": ["voice"], "line_chars": cap["line_chars"], "lines": cap["lines"], "save_as": draft})
            document = json.loads((self.root / draft).read_text(encoding="utf-8"))
            document = document.get("document", document)
            outputs, sidecars = [self.ident(draft)], []
            for fmt in cap["sidecars"]:
                out = f"exports/{self.m['production_id']}-{key[:8]}-{attempt}.{fmt}"
                self.engine.call("captions.export", {"document": {"file": draft, "select": "document"}, "format": {"vtt": "webvtt"}.get(fmt, fmt),
                                                     "loss_policy": "allow_reported", "output": out})
                outputs.append(self.ident(out))
                sidecars.append(out)
            return outputs, {"draft": draft, "cues": len(document.get("cues", [])), "sidecars": sidecars, "transcripts": transcripts}
        return self.stage("captions", key, request, run, {"engine": self.engine_identity})

    # ------------------------------------------------------------------ mix
    def stage_mix(self, audio):
        timer = Timer()
        music = self.m["music"]
        delivery = self.m["delivery"]
        request = {"audio": audio["key"], "music": None if not music else {"bed_db": fstr(music["bed_db_under_voice"]), "duck": music["duck_milli"]},
                   "target": delivery["loudness_lkfs"], "peak": delivery["peak_dbfs"], "codec": CODEC_CHECK}
        key = self.key("mix", request, {"engine": self.engine_identity})

        def run(attempt):
            snapshot = json.loads((self.root / audio["result"]["snapshot"]).read_text(encoding="utf-8"))
            report = {}
            music_ids = [c["id"] for t in snapshot["tracks"]["tracks"] if t["id"] == "music" for c in t["clips"]]
            if music:
                meters = self.engine.call("timeline.meters", {"project": snapshot, "tracks": True}, label="mix")
                per = {t["track_id"]: t["meters"] for t in meters.get("tracks", [])}
                voice_lk, music_lk = per["voice"]["integrated_lkfs"], per["music"]["integrated_lkfs"]
                gain = round(1000 * 10 ** ((voice_lk - float(music["bed_db_under_voice"]) - music_lk) / 20))
                gain = max(1, min(4000, gain))
                report["before"] = {"voice_lkfs": voice_lk, "music_lkfs": music_lk, "music_gain_milli": gain}
                snapshot = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": [
                    {"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": music_ids, "gain_milli": gain}}]})
                duck = self.engine.call("audio.duck", {"project": snapshot, "voice_track_id": "voice", "music_track_id": "music",
                                                       "duck_milli": music["duck_milli"], "attack": "3/10", "release": "4/5", "bridge": "6/5"}, label="mix")
                if duck.get("operations"):
                    snapshot = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": duck["operations"]})
                report["duck"] = {"speech_runs": len(duck.get("speech", duck.get("runs", [])) or []), "operations": len(duck.get("operations", []))}
            # Normalize under the delivery peak by the codec headroom, then hear the mix as the export will encode it:
            # the same AAC encoder, bitrate and samples (an audio-only M4A decodes identically to the MP4's audio). A true
            # peak over the target lowers the limiter ceiling by the excess and tries again, while that still helps.
            peak = delivery["peak_dbfs"]
            ceiling = max(-20.0, round(peak - CODEC_CHECK["headroom_db"], 2))
            trials = []
            while True:
                n = len(trials) + 1
                norm = self.engine.call("audio.normalize", {"project": snapshot, "target_lkfs": delivery["loudness_lkfs"],
                                                            "peak_ceiling_dbfs": ceiling}, label="mix")
                mixed = snapshot
                if norm.get("operations"):
                    mixed = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"],
                                                                "operations": norm["operations"]})
                path = f"generated/timeline/mixed-{key[:12]}-{attempt}-t{n}.json"
                write_new(self.root / path, mixed)
                trial = {"ceiling_dbfs": ceiling, "snapshot": path, "normalize": {k: norm.get(k) for k in ("result", "limited_by", "factor") if k in norm},
                         **self.aac_trial(path, f"{key[:12]}-{attempt}-t{n}")}
                # What the encode added to the mix's own true peak: the codec headroom this mix needs.
                mix_peak = trial["mix_true_peak_dbtp"] = (norm.get("result") or {}).get("true_peak_dbtp")
                if trial["true_peak_dbtp"] is not None and mix_peak is not None:
                    trial["codec_overshoot_db"] = round(trial["true_peak_dbtp"] - mix_peak, 2)
                trials.append(trial)
                self.log(f"mix: AAC trial {n} at a {ceiling:g} dBFS ceiling: {db(trial['integrated_lkfs'], 'LKFS')}, "
                         f"true peak {db(trial['true_peak_dbtp'], 'dBTP')}")
                stop, value = codec_step(trials, peak)
                if stop:
                    break
                ceiling = value
            passed = stop == "under_target"
            measured = [i for i, t in enumerate(trials) if t["true_peak_dbtp"] is not None]
            chosen = len(trials) - 1 if passed or not measured else min(measured, key=lambda i: trials[i]["true_peak_dbtp"])
            if not passed:
                self.log(f"mix: AAC trials stopped ({stop}): {value}")
            report["normalize"] = trials[chosen]["normalize"]
            report["codec"] = {"profile": CODEC_CHECK["profile"], "passed": passed, "chosen": chosen, "peak_dbfs": peak,
                               "headroom_db": CODEC_CHECK["headroom_db"], "decision": {"stop": stop, "reason": value},
                               "trials": [{k: v for k, v in t.items() if k != "normalize"} for t in trials]}
            path = trials[chosen]["snapshot"]
            return [self.ident(path)], {"snapshot": path, "report": report}
        receipt = self.stage("mix", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("mix") == "built":
            codec = receipt["result"]["report"]["codec"]
            chosen = codec["trials"][codec["chosen"]]
            self.log(f"mix: bed, duck and loudness set; limiter ceiling {chosen['ceiling_dbfs']:g} dBFS, AAC true peak "
                     f"{db(chosen['true_peak_dbtp'], 'dBTP')} after {len(codec['trials'])} trial(s), "
                     f"{codec['decision']['stop']} ({timer.seconds():.1f}s)")
        return receipt

    def aac_trial(self, snapshot, tag):
        """Encode a mixed snapshot's audio to AAC as the delivery will, and meter the decoded file like the final review."""
        out = self.unused(f"reviews/aac-{tag}.m4a")
        self.engine.call("export.run", {"project": {"file": snapshot}, "output": out, "profile": CODEC_CHECK["profile"], "streams": "audio"}, label="mix")
        folder = self.unused(f"reviews/aac-{tag}")
        self.engine.call("export.review", {"path": out, "output": folder, "frames": 1, "rendition_height": 0}, label="mix")
        meters = ((self.review_json(folder) or {}).get("sound") or {}).get("meters") or {}
        return {"file": out, "review": folder, "integrated_lkfs": meters.get("integrated_lkfs"),
                "sample_peak_dbfs": quality.highest(meters.get("sample_peak_dbfs")), "true_peak_dbtp": quality.highest(meters.get("true_peak_dbtp"))}

    # ------------------------------------------------------------------ scenes
    def transitions(self, timing):
        """Each scene's transition into the next, as {scene: (transition, next scene)}, checked against the timing plan.
        The effect covers the first `frames` of the next scene, read from a handle rendered past this scene's end, so
        neither scene moves and the incoming one needs no handle of its own."""
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        out = {}
        for scene, following in zip(self.m["scenes"], self.m["scenes"][1:]):
            transition = scene["transition"]
            if not transition:
                continue
            handle, length, room = F(transition["frames"], FPS), F(rows[scene["id"]]["duration"]), F(rows[following["id"]]["duration"])
            if handle > room:
                raise ToolError("production", "TRANSITION_TOO_LONG", f"scene {scene['id']}: its {transition['frames']}-frame transition is longer "
                                f"than the next scene {following['id']!r} ({room * FPS} frames)")
            if length + handle > 120:
                raise ToolError("production", "SCENE_TOO_LONG", f"scene {scene['id']} lasts {float(length):.2f} s, and the handle of its transition "
                                f"would render it {float(length + handle):.2f} s; scenes stop at 120 s, so split its script")
            out[scene["id"]] = (transition, following["id"])
        return out

    def stage_scenes(self, timing, aligned, art, inputs):
        """Every scene's recipe, from the template or a hand-written override, rendered one per lane. Captions are not part
        of a scene: they are one overlay over the film (stage_caption_overlay)."""
        timer = Timer()
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        transitions = self.transitions(timing)
        patterns = pixel_stage.check_patterns(self.m["patterns"])
        fonts = {k: inputs[v] for k, v in self.m["fonts"].items()}
        self.mkdir("scenes")

        def cue_resolver(scene_id):
            return cue_at(scene_id, aligned.get(scene_id, {}).get("words", []), self.m["timing"]["lead"])

        def build_one(item):
            scene_id = item[1]["id"]
            try:
                return render_one(item)
            except ToolError as error:
                if f"scene {scene_id}" in error.message or f"scenes.{scene_id}" in error.message:
                    raise
                raise ToolError(error.tool, error.code, f"scene {scene_id}: {error.message}", error.detail) from None

        def render_one(item):
            number, scene = item
            row = rows[scene["id"]]
            duration = F(row["duration"])
            # A transition into the next scene reads this scene past its end: render it that much longer.
            tail = F(transitions[scene["id"]][0]["frames"], FPS) if scene["id"] in transitions else F(0)
            override = self.m["overrides"]["scenes"].get(scene["id"])
            resolved = None
            if override:
                base, resolved = self.resolve_override(scene, override, duration, inputs, cue_resolver(scene["id"]))
                if tail:
                    base = override_module.with_tail(base, duration, tail)
            else:
                base = pixel_stage.SceneBuilder(scene, duration, art, self.art_index, patterns, self.m["colors"], cue_resolver(scene["id"]),
                                                fonts, number, tail).build()
            base_path = self.save_base(scene["id"], base)
            # The key is the recipe's content alone: a neighbour's retiming or a caption change leaves it alone.
            # An override's key also names its source file, every cue time it resolved and every input identity it used.
            request = {"recipe": digest(base)}
            if resolved:
                request["override"] = resolved
            if tail:
                request["handle"] = fstr(tail)
            key = self.key(f"scene:{scene['id']}", request, {"engine": self.engine_identity})

            def run(attempt):
                out = f"renders/{scene['id']}-{key[:12]}-{attempt}.mkv"
                result = self.engine.job("scene.render", {"scene": {"file": base_path}, "output": out}, request_id=f"scene-{scene['id']}-{key[:16]}-{attempt}",
                                         lane=f"lane-{number % self.lanes}", label=f"scene:{scene['id']}")
                # Asset IDs name content, so a revised scene enters the saved project beside the old one, never as it.
                asset = dict(result["asset"], id=f"{scene['id']}-{key[:12]}")
                return [self.ident(asset["path"])], {"asset": asset, "recipe": base_path, "layers": len(base["layers"]), "handle": fstr(tail)}
            receipt = self.stage(f"scene:{scene['id']}", key, request, run, {"engine": self.engine_identity})
            return scene["id"], receipt["result"]["asset"]

        with ThreadPoolExecutor(max_workers=self.lanes) as pool:
            rendered = dict(pool.map(build_one, enumerate(self.m["scenes"])))
        built = sum(1 for s in self.m["scenes"] if self.outcomes.get(f"scene:{s['id']}") == "built")
        self.log(f"scenes: {built} rendered, {len(rendered) - built} reused ({timer.seconds():.1f}s)")
        return rendered

    def stage_caption_overlay(self, captions, audio, inputs):
        """Burned-in captions: the whole caption draft rendered as one transparent overlay (captions.render) for the cut's
        `captions` track. It needs no scene layers and suits every kind of scene; a caption change renders it again and
        leaves every scene alone."""
        timer = Timer()
        fonts = {k: inputs[v] for k, v in self.m["fonts"].items()}
        layouts = pixel_stage.caption_layouts(fonts, self.m["colors"])
        # The captions key covers the draft and the audio timeline (the film's length); the layouts name the font copies by content.
        request = {"captions": captions["key"], "project": audio["key"], "layouts": layouts}
        key = self.key("caption-overlay", request, {"engine": self.engine_identity})

        def run(attempt):
            out = f"renders/captions-{key[:12]}-{attempt}.mkv"
            result = self.engine.job("captions.render", {"document": {"file": captions["result"]["draft"], "select": "document"},
                                                         "layouts": layouts, "project": {"file": audio["result"]["snapshot"]}, "output": out,
                                                         "asset_id": f"captions-{key[:12]}"},
                                     request_id=f"captions-{key[:20]}-{attempt}", lane="captions", label="captions")
            asset = dict(result["asset"], path=self.rel_any(result["asset"]["path"]))
            return [self.ident(asset["path"])], {"asset": asset, "windows": result.get("windows"), "covers": result.get("covers"),
                                                 "cues": result.get("cues")}
        receipt = self.stage("caption-overlay", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("caption-overlay") == "built":
            self.log(f"captions: {captions['result']['cues']} cue(s) rendered as one overlay, {receipt['result']['asset']['path']} "
                     f"({timer.seconds():.1f}s)")
        return receipt["result"]["asset"]

    def save_base(self, scene_id, base):
        """Write a scene's base recipe (before captions) under scenes/, named by its content; returns its path."""
        base_path = f"scenes/{scene_id}-base-{digest(base)[:12]}.json"
        if not (self.root / base_path).exists():
            write_new(self.root / base_path, base)
        return base_path

    def resolve_override(self, scene, override, duration, inputs, at):
        """A hand-written recipe with its cue times, input identities and scene length filled in (see override.py).
        It is read from the production's own copy and checked again, so the build uses exactly what it hashed."""
        source = inputs[override["input"]]
        where = f"overrides.scenes.{scene['id']}"
        identities = {}

        def identity(name, frame=None):
            path = inputs[name] if frame is None else inputs[name][frame]
            if path not in identities:
                known = self.source_ids.get(path) or self.ident(path)
                identities[path] = {k: known[k] for k in ("path", "sha256", "bytes")}
            return identities[path]
        try:
            document = json.loads((self.root / source).read_text(encoding="utf-8"))
            recipe, report = override_module.prepare(document, scene, self.m["inputs"], where, override_module.Resolution(at, identity, duration))
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ToolError("production", "INVALID_OVERRIDE", f"{where}: {source} is not a JSON scene recipe: {error}") from None
        except override_module.OverrideError as error:
            raise ToolError("production", "INVALID_OVERRIDE", str(error)) from None
        if report["cues"]:
            # Each distinct cue once: a recipe can use one word for dozens of keys.
            distinct = list(dict.fromkeys(f"{c['word']} at frame {c['frame']}" for c in report["cues"]))
            uses = f" ({len(report['cues'])} uses)" if len(report["cues"]) > len(distinct) else ""
            self.log(f"scene {scene['id']}: " + ", ".join(distinct) + uses)
        return recipe, {"source": self.ident(source)["sha256"], **report}

    def preview(self, scene_id, stills=()):
        """`production.py resolve`: one hand-written scene resolved without a build, from the last build's take and
        alignment and the current copies of its inputs. The recipe is written where a build writes it, so the next build
        finds it; each still is the frame on screen at a scene time, rendered by the engine's scene.still."""
        scene = next((s for s in self.m["scenes"] if s["id"] == scene_id), None)
        if scene is None:
            raise ToolError("production", "SCENE_NOT_FOUND", f"no scene {scene_id!r}; scenes: {[s['id'] for s in self.m['scenes']]}")
        override = self.m["overrides"]["scenes"].get(scene_id)
        if override is None:
            raise ToolError("production", "NOT_AN_OVERRIDE", f"scene {scene_id!r} has no hand-written recipe in overrides.scenes; "
                            f"a build writes the template's recipes under scenes/")
        words, take, alignment = [], None, None
        if scene["script"]:
            aligned, timing = self.state.receipt(f"align:{scene_id}"), self.state.receipt("timing")
            rows = timing["request"]["scenes"] if timing and timing.get("state") == "completed" else []
            row = next((r for r in rows if r["id"] == scene_id), None)
            if not aligned or aligned.get("state") != "completed" or not row or not row["take"]:
                raise ToolError("production", "NOT_ALIGNED", f"scene {scene_id!r} has no timed and aligned take yet; build first "
                                f"(build --until scenes is enough, and a build that failed on this recipe has done it)")
            if aligned["request"]["text"] != scene["script"]:
                raise ToolError("production", "STALE_ALIGNMENT", f"scene {scene_id!r}: the last aligned take says {aligned['request']['text']!r}, "
                                f"not the manifest's script; build again (--until scenes) to narrate and align it")
            words, take = aligned["result"]["words"], row["take"]
            alignment = {"stage": f"align:{scene_id}", "key": aligned["key"], "asset_sha256": aligned["request"]["asset_sha256"]}
        # The scene's length is planned as a build plans it, from the manifest's timing rules and the take's length.
        planned = plan_timing(self.m["timing"], [{"id": scene_id, "duration": fstr(scene["duration"]) if scene["duration"] else None,
                                                  "type": scene["beat"]["type"] if scene["beat"] else "override", "take": take}])
        duration = F(planned["scenes"][0]["duration"])
        for d in ("sources", "scenes"):
            self.mkdir(d)
        inputs = self.stage_inputs({override["input"], *override["inputs"], *override.get("frames", {})})
        base, resolved = self.resolve_override(scene, override, duration, inputs, cue_at(scene_id, words, self.m["timing"]["lead"]))
        if scene["transition"]:
            # As a build renders it: with the handle its transition into the next scene reads.
            base = override_module.with_tail(base, duration, F(scene["transition"]["frames"], FPS))
        base_path = self.save_base(scene_id, base)
        shown = []
        for t in stills:
            if not t < duration:
                raise ToolError("production", "INVALID_TIME", f"--still {fstr(t)}: the scene lasts {fstr(duration)} s")
            frame = int(t * FPS)
            out = f"previews/{scene_id}-{digest(base)[:12]}-f{frame:04d}.png"
            if not (self.root / out).exists():
                self.mkdir("previews")
                self.engine.call("scene.still", {"scene": {"file": base_path}, "output": out, "time": rt(F(frame, FPS))}, label=f"still:{scene_id}")
            shown.append({"time": fstr(t), "frame": frame, "path": out})
        self.state.event({"event": "resolved", "scene": scene_id, "recipe": base_path, "stills": [s["path"] for s in shown]})
        return {"scene": scene_id, "recipe": base_path, "alignment": alignment, **resolved, "stills": shown}

    # ------------------------------------------------------------------ cut (saved session)
    def stage_cut(self, mixed, rendered, timing, overlay=None):
        timer = Timer()
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        snapshot = json.loads((self.root / mixed["result"]["snapshot"]).read_text(encoding="utf-8"))
        ops = [{"op": "media.add", "asset": rendered[s["id"]]} for s in self.m["scenes"]]
        # Burned-in captions: an alpha_over track above the picture, empty when captions are not burned in.
        ops.append({"op": "tracks.edit", "edit": {"op": "add", "track": {"id": "captions", "kind": "video", "locked": False, "enabled": True,
                                                                         "clips": [], "composite": "alpha_over"}}})
        for s in self.m["scenes"]:
            row = rows[s["id"]]
            ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "picture", "collision": "reject", "clip": {
                "id": f"v-{s['id']}", "asset_id": rendered[s["id"]]["id"], "start": rt(F(row["start"])), "source_in": rt(0),
                "duration": rt(F(row["duration"]))}}})
        # Each transition starts on the cut and reads the outgoing scene's handle: `before` is zero, `after` its frames.
        for scene_id, (transition, following) in self.transitions(timing).items():
            ops.append({"op": "tracks.edit", "edit": {"op": "transition_set", "track_id": "picture", "transition": {
                "id": f"x-{scene_id}", "left_id": f"v-{scene_id}", "right_id": f"v-{following}", "before": rt(0),
                "after": rt(F(transition["frames"], FPS)), "kind": transition["kind"]}}})
        if overlay:
            ops.append({"op": "media.add", "asset": overlay})
            ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "captions", "collision": "reject", "clip": {
                "id": "c-captions", "asset_id": overlay["id"], "start": rt(0), "source_in": rt(0), "duration": rt(F(timing["result"]["total"]))}}})
        desired = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": ops})
        request = {"desired": digest(strip_revision(desired))}
        key = self.key("cut", request, {"engine": self.engine_identity})
        project_id = self.m["production_id"]

        def head():
            try:
                return self.engine.call("session.get", {"project_id": project_id})
            except ToolError as error:
                if error.code in ("PROJECT_NOT_FOUND", "NOT_FOUND", "STORE_NOT_FOUND"):
                    return None
                raise

        def run(attempt):
            current = head()
            if current is None:
                receipt = self.engine.call("session.create", {"project": desired, "request_id": f"create-{key[:24]}"})
                return [], {"revision": receipt["revision"], "action": "created", "operations": len(ops)}
            current = current.get("project", current)
            diff = reconcile(current, desired)
            if not diff:
                return [], {"revision": current["revision"], "action": "unchanged", "operations": 0}
            # Prove the operations reach the target on a pure copy before saving anything.
            trial = self.engine.call("timeline.apply", {"project": current, "expected_revision": current["revision"], "operations": diff})
            differences = arrangement_differences(trial, desired)
            if differences:
                raise ToolError("production", "RECONCILE_INCOMPLETE", f"the saved project would differ from the target in {differences}; "
                                f"nothing was saved. Rebuild into a new --root, or report the difference")
            receipt = self.engine.call("session.apply", {"project_id": project_id, "request_id": f"build-r{current['revision']}-{key[:20]}",
                                                         "expected_revision": current["revision"], "operations": diff})
            changes = receipt.get("changes", {})
            return [], {"revision": receipt["revision"], "action": "applied", "operations": len(diff),
                        "changed_clips": sorted({c.get("id") or c.get("clip_id") for c in changes.get("clips", []) if isinstance(c, dict)} - {None})}
        receipt = self.state.reusable("cut", key)
        if receipt and not self.forced("cut"):
            current = head()
            if current is not None and current.get("project", current)["revision"] == receipt["result"]["revision"]:
                self.outcomes["cut"] = "reused"
                return receipt
        # Otherwise the saved head moved (an edit made directly to the project) or is gone: reconcile it to the target.
        receipt = self.stage("cut", key, request, run, {"engine": self.engine_identity}, reuse=False)
        self.log(f"cut: project {project_id} revision {receipt['result']['revision']} ({receipt['result']['action']}, "
                 f"{receipt['result']['operations']} operation(s), {timer.seconds():.1f}s)")
        return receipt

    # ------------------------------------------------------------------ delivery
    def stage_export(self, cut):
        timer = Timer()
        revision = cut["result"]["revision"]
        request = {"cut": cut["key"], "revision": revision, "profile": "h264_aac", "streams": "audio_video"}
        key = self.key("export", request, {"engine": self.engine_identity})

        def run(attempt):
            out = f"exports/{self.m['production_id']}-r{revision}-{key[:8]}-{attempt}.mp4"
            self.log(f"export: rendering {out}")
            result = self.engine.job("export.run", {"project": {"project_id": self.m["production_id"], "revision": revision}, "output": out,
                                                    "profile": "h264_aac", "streams": "audio_video"},
                                     request_id=f"export-{key[:20]}-{attempt}", lane="export", label="export")
            return [self.ident(out)], {"path": out, "project_revision": revision,
                                       "receipt": {k: v for k, v in result.items() if k in ("duration", "frames", "video", "audio", "output", "profile")}}
        receipt = self.stage("export", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("export") == "built":
            self.log(f"export: {receipt['result']['path']} ({timer.seconds():.1f}s)")
        return receipt

    def stage_review(self, exported, cut, aligned):
        """Picture, sound and timing of the delivered file (sheet, black/silence/clipping, loudness, duration)."""
        timer = Timer()
        review = self.m["delivery"]["review"]
        transcripts = self.transcripts_file(aligned) if aligned else None
        request = {"export": exported["outputs"][0]["sha256"], "revision": cut["result"]["revision"], "frames": review["frames"], "preview": review["preview"],
                   "transcripts": self.state.identity(transcripts)["sha256"] if transcripts else None}
        key = self.key("review", request, {"engine": self.engine_identity})

        def run(attempt):
            folder = f"reviews/review-{key[:12]}-{attempt}"
            args = {"path": exported["result"]["path"], "project": {"project_id": self.m["production_id"], "revision": cut["result"]["revision"]},
                    "output": folder, "frames": review["frames"], "rendition_height": 360 if review["preview"] else 0}
            if transcripts:
                args["transcripts"] = {"file": transcripts, "select": "transcripts"}
            result = self.engine.job("export.review", args, request_id=f"review-{key[:20]}-{attempt}", lane="review", label="review")
            outputs = [self.ident(f"{folder}/{name}") for name in ("review.json", "sheet.png") if (self.root / folder / name).exists()]
            return outputs, {"folder": folder, "summary": result.get("summary")}
        receipt = self.stage("review", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("review") == "built":
            self.log(f"review: {receipt['result']['folder']} ({timer.seconds():.1f}s)")
        return receipt

    def stage_speech_review(self, cut, aligned):
        """What the cut is heard to say against what it should say, from an audio-only render of the same revision."""
        timer = Timer()
        transcripts = self.transcripts_file(aligned)
        revision = cut["result"]["revision"]
        request = {"cut": cut["key"], "revision": revision, "transcripts": self.state.identity(transcripts)["sha256"],
                   "runtime": digest(self.cfg["speech_runtime"]), "language": self.m["language"]}
        key = self.key("speech-review", request, {"engine": self.engine_identity})

        def run(attempt):
            audio = f"reviews/mix-r{revision}-{key[:8]}-{attempt}.wav"
            self.engine.job("export.run", {"project": {"project_id": self.m["production_id"], "revision": revision}, "output": audio,
                                           "profile": "reference", "streams": "audio"},
                            request_id=f"mix-{key[:20]}-{attempt}", lane="speech", label="speech-review")
            folder = f"reviews/speech-{key[:12]}-{attempt}"
            result = self.engine.job("export.review", {"path": audio, "project": {"project_id": self.m["production_id"], "revision": revision},
                                                       "transcripts": {"file": transcripts, "select": "transcripts"}, "runtime": self.cfg["speech_runtime"],
                                                       "language": self.m["language"], "output": folder, "frames": 1, "rendition_height": 0},
                                     request_id=f"speech-{key[:20]}-{attempt}", lane="speech", label="speech-review")
            speech = result.get("speech") or {}
            comparison, recognition = speech.get("comparison") or {}, speech.get("recognition") or {}
            return [self.ident(audio), self.ident(f"{folder}/review.json")], {
                "folder": folder, "audio": audio, "summary": result.get("summary"),
                "speech": {"recognition_ok": recognition.get("ok"), "recognition_error": recognition.get("error"),
                           **{k: comparison.get(k) for k in ("match_ratio", "matched", "expected_words", "heard_words") if k in comparison},
                           "differences": (comparison.get("differences") or {}).get("count")}}
        receipt = self.stage("speech-review", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("speech-review") == "built":
            self.log(f"speech check: {receipt['result']['folder']} ({timer.seconds():.1f}s)")
        return receipt


# ---------------------------------------------------------------------------------------- helpers
def cue_at(scene_id, words, lead):
    """A scene's cue resolver: a checked cue's time in the scene, on the frame nearest the aligned word's start (or end)
    plus the lead."""
    norm = [(pixel_stage.words_of(w["text"]) or [""])[0] for w in words]

    def at(cue):
        hits = [i for i, w in enumerate(norm) if w == cue["word"]]
        if len(hits) <= cue["nth"]:
            raise ToolError("production", "CUE_NOT_HEARD", f"scene {scene_id}: cue word {cue['word']!r} is not in the aligned narration")
        word = words[hits[cue["nth"]]]
        t = fr(word["start"] if cue["edge"] == "start" else word["end"])
        return pixel_stage.snap(lead + t)
    return at


def plan_timing(t, scenes):
    """Scene boundaries from measured takes, exactly.

    A narrated scene lasts its lead-in, its take and a tail, at least `min_scene`, rounded up to the snap grid (a frame,
    a beat or a bar of the music) and to whole frames. A fixed duration must hold its take; nothing is ever cut or
    stretched. A silent title lasts `title_bars` bars; an end card at least `end_min_bars`. Scenes follow one another."""
    beat = t["beat"]
    grid = {"frame": F(1, FPS), "beat": beat, "bar": 4 * beat}[t["snap"]]
    start, rows = F(0), []
    for s in scenes:
        take = F(s["take"]["samples"], s["take"]["rate"]) if s["take"] else None
        if s["duration"]:
            d = F(s["duration"])
            if take is not None and t["lead"] + take > d:
                raise ToolError("production", "NARRATION_OVERFLOW",
                                f"scene {s['id']}: its {float(take):.2f} s take does not fit the fixed {float(d):.2f} s duration; "
                                f"remove duration to let the scene fit its narration, or shorten the script")
        elif take is not None:
            need = max(t["lead"] + take + t["tail"], t["min_scene"])
            if s["type"] == "end":
                need = max(need, 4 * beat * t["end_min_bars"])
            d = F(-(-need // grid)) * grid
        elif s["type"] == "title":
            d = 4 * beat * t["title_bars"]
        else:
            d = F(-(-t["min_scene"] // grid)) * grid
        d = F(-(-d * FPS // 1), FPS)
        if d > 120:
            raise ToolError("production", "SCENE_TOO_LONG", f"scene {s['id']} would last {float(d):.1f} s; scenes stop at 120 s, so split its script")
        row = {"id": s["id"], "start": fstr(start), "duration": fstr(d)}
        if take is not None:
            row["voice_start"] = fstr(start + t["lead"])
            row["take"] = fstr(take)
        rows.append(row)
        start += d
    if start > 600:
        raise ToolError("production", "TOO_LONG", f"the film would last {float(start):.1f} s; this template stops at 600 s")
    return {"scenes": rows, "total": fstr(start)}


def codec_step(trials, peak, check=CODEC_CHECK):
    """After each AAC trial: (None, the next ceiling) to try again, or (code, reason) to stop. The trials stop
    - `under_target`: the last trial delivers at or under `peak`;
    - `not_better`: its true peak is no lower than the best earlier trial's (a lower ceiling squeezes the mix harder,
      and the codec can overshoot more, so trying lower again would waste an encode);
    - `trial_cap`: `check["trials"]` encodes ran;
    - `ceiling_floor`: the ceiling cannot go lower;
    - `unmeasured`: the encode's true peak could not be measured."""
    n, last = len(trials), trials[-1]
    ceiling = last["ceiling_dbfs"]
    if last["true_peak_dbtp"] is None:
        return "unmeasured", f"trial {n}'s encode has no measured true peak"
    measured = round(last["true_peak_dbtp"], 2)
    if measured <= peak:
        return "under_target", f"trial {n} at a {ceiling:g} dBFS ceiling delivers {measured:.2f} dBTP, at or under the {peak:g} dBTP target"
    earlier = [round(t["true_peak_dbtp"], 2) for t in trials[:-1] if t["true_peak_dbtp"] is not None]
    if earlier and measured >= min(earlier):
        return "not_better", (f"trial {n} at a {ceiling:g} dBFS ceiling delivers {measured:.2f} dBTP, no lower than the best "
                              f"earlier trial's {min(earlier):.2f} dBTP; a lower ceiling only limits the mix harder")
    if n >= check["trials"]:
        return "trial_cap", f"all {n} trials, the most allowed, deliver over {peak:g} dBTP"
    lower = max(-20.0, round(ceiling - (measured - peak) - check["margin_db"], 2))
    if lower >= ceiling:
        return "ceiling_floor", f"the limiter ceiling cannot go below {ceiling:g} dBFS"
    return None, lower


def db(value, unit):
    return "unmeasured" if value is None else f"{value:.2f} {unit}"


def music_clips(duration, total, loop, beat):
    """The music track's clips as (id, start, length). Without a loop the bed plays once from the start and stops at its
    end or the film's. With `bars` it repeats from its start every whole number of 4/4 bars that fits the bed."""
    if loop != "bars":
        return [("m-music", F(0), min(duration, total))]
    bar = 4 * beat
    period = (duration // bar) * bar
    if period <= 0:
        raise ToolError("production", "MUSIC_TOO_SHORT", f"music.loop bars: the bed lasts {float(duration):.2f} s, less than one "
                        f"{float(bar):.2f} s bar at music.bpm")
    clips, start = [], F(0)
    while start < total:
        clips.append(("m-music" if not clips else f"m-music-{len(clips) + 1}", start, min(period, total - start)))
        start += period
    return clips


def wav_seconds(path):
    """A WAV file's length from its header, or None when it cannot be read."""
    try:
        with wave.open(str(path)) as w:
            return F(w.getnframes(), w.getframerate())
    except (OSError, EOFError, wave.Error):
        return None


def estimate(m):
    """The film's length before anything is synthesized, with a range. Each narrated line is estimated from the
    speaker's take model (a fixed overhead plus its letters at a measured rate; see quality.TAKE_MODEL), a supplied take
    at its own length, and the film is planned as a build plans it; the range plans it again with every estimated take
    FILM_SPREAD shorter and longer. Warns about a music bed shorter than the range's top, and a fixed-duration scene
    whose estimated take, TAKE_SPREAD longer, would not fit."""
    (overhead, rate), measured = quality.take_model(m["voice"])
    t, warnings, takes = m["timing"], [], []
    for s in m["scenes"]:
        seconds, guessed = None, False
        if s["script"]:
            override = m["overrides"]["narration"].get(s["id"])
            seconds = wav_seconds(m["inputs"][override["input"]]) if override else None
            if seconds is None:
                guessed = True
                seconds = F(str(overhead)) + F(sum(len(w) for w in pixel_stage.words_of(s["script"]))) / F(str(rate))
                if m["voice"] and m["voice"]["split"] == "sentence":
                    seconds += m["voice"]["pause"] * (len(sentences(s["script"])) - 1)
            longest = seconds * F(str(1 + quality.TAKE_SPREAD)) if guessed else seconds
            if s["duration"] and t["lead"] + longest > s["duration"]:
                warnings.append(quality.warning("NARRATION_MAY_OVERFLOW", "check", f"scene {s['id']}: its narration is estimated at "
                                                f"{float(seconds):.1f} s" + (f" (up to {float(longest):.1f} s)" if guessed else "")
                                                + f", which may not fit its fixed {float(s['duration']):.2f} s duration"))
        takes.append((s, seconds, guessed))

    def plan(scale):
        """The timing plan and total narration with every estimated take scaled by `scale`."""
        rows, narration = [], F(0)
        for s, seconds, guessed in takes:
            take = None
            if seconds is not None:
                seconds = seconds * scale if guessed else seconds
                narration += seconds
                if not s["duration"]:
                    take = {"samples": -(-seconds * 24000 // 1), "rate": 24000}
            rows.append({"id": s["id"], "duration": fstr(s["duration"]) if s["duration"] else None,
                         "type": s["beat"]["type"] if s["beat"] else "override", "take": take})
        return plan_timing(t, rows), narration
    spread = F(str(quality.FILM_SPREAD))
    out = {"take_model": {"overhead_seconds": overhead, "letters_per_second": rate}, "rate_measured": measured}
    try:
        planned, narration = plan(F(1))
    except ToolError as error:
        warnings.append(quality.warning(error.code, "check", f"estimated: {error.message}"))
        return out, warnings
    low_plan, low_narration = plan(1 - spread)
    try:
        high_plan, high_narration = plan(1 + spread)
    except ToolError as error:
        warnings.append(quality.warning(error.code, "check", f"estimated at the top of its range: {error.message}"))
        high_plan, high_narration = planned, narration
    total = lambda p: round(float(F(p["total"])), 2)  # noqa: E731
    out.update(narration_seconds=round(float(narration), 2), narration_range=[round(float(low_narration), 2), round(float(high_narration), 2)],
               film_seconds=total(planned), film_range=[total(low_plan), total(high_plan)])
    out["scenes"] = [{"id": r["id"], "start": round(float(F(r["start"])), 2), "duration": round(float(F(r["duration"])), 2)} for r in planned["scenes"]]
    music = m["music"]
    if music:
        seconds = wav_seconds(m["inputs"][music["input"]])
        out["music_seconds"] = None if seconds is None else round(float(seconds), 3)
        speaker = (m["voice"] or {}).get("speaker")
        basis = ("narration from the supplied takes" if not any(g for _, _, g in takes) else
                 f"each take {overhead:g} s plus {rate:g} letters/s, " + (f"as measured for {speaker}" if measured else
                                                                          f"as measured for ryan; {speaker} is unmeasured"))
        warnings += quality.music_warnings("check", None if seconds is None else float(seconds), out["film_seconds"],
                                           music["loop"], estimated=True, basis=basis, film_range=out["film_range"])
    return out, warnings


def sentences(text):
    """Split a script line after . ! or ? followed by a space."""
    import re
    return [part for part in re.split(r"(?<=[.!?])\s+", text.strip()) if part]


def line_request(scene_id, key, request):
    line = {"id": scene_id, "text": request["text"], "file": f"{scene_id}-{key[:12]}.wav"}
    if "parts" in request:
        line["parts"], line["pause"] = request["parts"], float(F(request["pause"]))
    return line


def _linux(path):
    from .engine import linux_path
    return linux_path(path)


def write_new(path, value):
    path = Path(path)
    temp = path.with_name(f".{path.name}.partial")
    with open(temp, "w", encoding="utf-8") as f:
        json.dump(value, f, ensure_ascii=False)
    os.replace(temp, path)


def strip_revision(snapshot):
    return {k: v for k, v in snapshot.items() if k != "revision"}


def arrangement(snapshot):
    """What a snapshot plays: its settings, tracks with clips and transitions keyed by ID, links, the end, limiters, and the
    assets its clips use. Array order inside a track and assets no clip uses do not change playback, so they are left out."""
    tracks = snapshot.get("tracks") or {}
    clips = {c["id"]: c for t in tracks.get("tracks", []) for c in t["clips"]}
    used = {c.get("asset_id") for c in clips.values()}
    return {
        "settings": {k: v for k, v in snapshot.items() if k not in ("revision", "assets", "tracks", "clips")},
        "sequence": snapshot.get("clips"),
        "assets": {a["id"]: a for a in snapshot.get("assets", []) if a["id"] in used},
        "end": tracks.get("duration"), "master": tracks.get("master"), "order": [t["id"] for t in tracks.get("tracks", [])],
        "links": sorted((json.dumps(link, sort_keys=True) for link in tracks.get("links", []))),
        "tracks": {t["id"]: {**{k: v for k, v in t.items() if k not in ("clips", "transitions")}, "clips": {c["id"]: c for c in t["clips"]},
                             "transitions": {x["id"]: x for x in t.get("transitions", [])}}
                   for t in tracks.get("tracks", [])},
    }


def arrangement_differences(a, b):
    """Where the two arrangements differ (a part, or a track's clip and field); empty when they play the same."""
    left, right = arrangement(a), arrangement(b)
    out = []
    for part in left:
        if left[part] == right[part]:
            continue
        if part != "tracks" or set(left["tracks"]) != set(right["tracks"]):
            out.append(part)
            continue
        for tid in sorted(left["tracks"]):
            lt, rt_ = left["tracks"][tid], right["tracks"][tid]
            for field in sorted(set(lt) | set(rt_)):
                if field not in ("clips", "transitions") and lt.get(field) != rt_.get(field):
                    out.append(f"tracks.{tid}.{field}")
            out += [f"tracks.{tid}.transitions.{x}" for x in sorted(set(lt["transitions"]) | set(rt_["transitions"]))
                    if lt["transitions"].get(x) != rt_["transitions"].get(x)]
            for cid in sorted(set(lt["clips"]) | set(rt_["clips"])):
                lc, rc = lt["clips"].get(cid), rt_["clips"].get(cid)
                if lc is None or rc is None:
                    out.append(f"tracks.{tid}.{cid}")
                else:
                    out += [f"tracks.{tid}.{cid}.{f}" for f in sorted(set(lc) | set(rc)) if lc.get(f) != rc.get(f)]
    return out


CLIP_PLACEMENT = ("asset_id", "start", "source_in", "duration")
ZERO = {"num": 0, "den": 1}


def audio_state(clip):
    """A clip's level, gain curve and fades as compared for reconciliation; an omitted gain is unity, an omitted fade zero.
    A clip keeps its gain_milli beside a curve that overrides it, so both are compared and both are set."""
    return (clip.get("gain_milli", 1000), clip.get("gain_curve"), clip.get("fade_in") or ZERO, clip.get("fade_out") or ZERO)


DEFAULT_AUDIO = (1000, None, ZERO, ZERO)


def reconcile(head, desired):
    """Operations that turn the saved head into the desired arrangement: new assets, removed or moved clips, the end,
    placements, transitions, then levels and fades. Unchanged clips and transitions get no operation, so a one-scene
    revision touches one clip and the transitions at its ends."""
    head_assets = {a["id"] for a in head.get("assets", [])}
    ops = [{"op": "media.add", "asset": a} for a in desired.get("assets", []) if a["id"] not in head_assets]
    if desired.get("transfer") and head.get("transfer") != desired.get("transfer"):
        ops.append({"op": "project.transfer", "transfer": desired["transfer"]})
    # Tracks the target has and the head lacks (a project saved before the captions track) are added empty, on top.
    order = [t["id"] for t in (head.get("tracks") or {}).get("tracks", [])]
    for track in desired["tracks"]["tracks"]:
        if track["id"] not in order:
            empty = {k: v for k, v in track.items() if k not in ("clips", "transitions", "dynamics")}
            ops.append({"op": "tracks.edit", "edit": {"op": "add", "track": {**empty, "clips": [],
                                                                             **({"transitions": []} if "transitions" in track else {})}}})
            order.append(track["id"])

    def clips(snapshot):
        out = {}
        for track in (snapshot.get("tracks") or {}).get("tracks", []):
            for clip in track["clips"]:
                out[clip["id"]] = (track["id"], clip)
        return out
    kinds = {t["id"]: t["kind"] for t in desired["tracks"]["tracks"]}
    now_clips, want_clips = clips(head), clips(desired)
    remove = sorted(clip_id for clip_id, (track, clip) in now_clips.items()
                    if clip_id not in want_clips or want_clips[clip_id][0] != track
                    or any(clip.get(k) != want_clips[clip_id][1].get(k) for k in CLIP_PLACEMENT))
    place, levels = [], []
    for clip_id, (track, clip) in want_clips.items():
        placed = clip_id not in now_clips or clip_id in remove
        if placed:
            place.append((track, clip))
        if kinds.get(track) != "audio":
            continue
        before = DEFAULT_AUDIO if placed else audio_state(now_clips[clip_id][1])
        after = audio_state(clip)
        if after == before:
            continue
        change = {"op": "clip_audio", "clip_ids": [clip_id], "gain_milli": after[0], "fade_in": after[2], "fade_out": after[3]}
        if after[1]:
            change["gain_curve"] = after[1]
        elif before[1]:
            change["clear_gain_curve"] = True
        levels.append(change)
    if remove:
        ops.append({"op": "tracks.edit", "edit": {"op": "remove", "clip_ids": remove, "links": "reject_partial"}})
    if (head.get("tracks") or {}).get("duration") != desired["tracks"]["duration"]:
        ops.append({"op": "tracks.edit", "edit": {"op": "duration", "duration": desired["tracks"]["duration"]}})
    for track, clip in place:
        ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": track, "collision": "reject", "clip": {
            k: clip[k] for k in ("id", "asset_id", "start", "source_in", "duration")}}})
    # Transitions: removing a clip removes the transitions at its ends, so those are set again with every one that changed.
    def transitions(snapshot):
        return {x["id"]: (track["id"], x) for track in (snapshot.get("tracks") or {}).get("tracks", []) for x in track.get("transitions", [])}
    gone = set(remove)
    kept = {tid: value for tid, value in transitions(head).items() if not {value[1]["left_id"], value[1]["right_id"]} & gone}
    wanted = transitions(desired)
    ops += [{"op": "tracks.edit", "edit": {"op": "transition_remove", "track_id": track, "id": tid}}
            for tid, (track, _) in sorted(kept.items()) if tid not in wanted or wanted[tid][0] != track]
    ops += [{"op": "tracks.edit", "edit": {"op": "transition_set", "track_id": track, "transition": transition}}
            for tid, (track, transition) in sorted(wanted.items()) if kept.get(tid) != (track, transition)]
    ops += [{"op": "tracks.edit", "edit": change} for change in levels]
    # Timeline limiters: the master (from audio.normalize) and any audio track's.
    if (head.get("tracks") or {}).get("master") != desired["tracks"].get("master"):
        ops.append({"op": "tracks.edit", "edit": {"op": "audio_dynamics", "dynamics": desired["tracks"].get("master")}})
    head_tracks = {t["id"]: t for t in (head.get("tracks") or {}).get("tracks", [])}
    for track in desired["tracks"]["tracks"]:
        if track["kind"] == "audio" and head_tracks.get(track["id"], {}).get("dynamics") != track.get("dynamics"):
            ops.append({"op": "tracks.edit", "edit": {"op": "audio_dynamics", "track_id": track["id"], "dynamics": track.get("dynamics")}})
    # Video tracks composite bottom to top, so the order is part of what plays.
    wanted = [t["id"] for t in desired["tracks"]["tracks"]]
    if order != wanted and sorted(order) == sorted(wanted):
        ops.append({"op": "tracks.edit", "edit": {"op": "order", "track_ids": wanted}})
    return ops
