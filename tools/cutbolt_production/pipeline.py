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
    timing, voice prepare, music → audio timeline → mix (meters, duck, normalize)
    audio timeline, align → captions → scenes (lanes, with the art)
    mix, scenes → cut → export → review, beside the speech check (an audio-only render of the same revision)
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

from . import COORDINATOR_VERSION, CONTRACT_VERSION, override as override_module, pixel_stage
from .engine import Engine, PixelForge, Qwen, ToolError
from .manifest import digest, summary, templated
from .state import State, Timer, now, sha256_file

STAGE_VERSION = "1"
FPS = pixel_stage.FPS


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

    # ------------------------------------------------------------------ helpers
    def log(self, message):
        print(f"[{time.perf_counter() - self.t0:7.1f}s] {message}", file=self.log_stream, flush=True)

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

    def stage(self, name, key, request, run, deps=None):
        """Reuse the completed receipt for `key`, or run `run(attempt)` -> (outputs, result) and record it."""
        if not self.forced(name):
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
            mix_future = pool.submit(self.stage_mix, audio)
            aligned = align_future.result()
            captions = self.stage_captions(audio, aligned)
            art = art_future.result()
            rendered = self.stage_scenes(timing, aligned, captions, art, inputs)
            mixed = mix_future.result()
        if self.until == "scenes":
            return self.finish(stopped="scenes")
        cut = self.stage_cut(mixed, rendered, timing)
        if self.until == "cut":
            return self.finish(stopped="cut", cut=cut)
        speech_check = self.m["delivery"]["review"]["speech"] and bool(aligned)
        with ThreadPoolExecutor(max_workers=2) as pool:
            # The speech check listens to the same revision's mix, rendered audio-only, while the picture exports.
            heard_future = pool.submit(self.stage_speech_review, cut, aligned) if speech_check else None
            exported = self.stage_export(cut)
            reviewed = self.stage_review(exported, cut, aligned)
            heard = heard_future.result() if heard_future else None
        return self.finish(cut=cut, exported=exported, reviewed=reviewed, heard=heard, timing=timing, captions=captions, scenes=scenes)

    def finish(self, stopped=None, cut=None, exported=None, reviewed=None, heard=None, timing=None, captions=None, scenes=None):
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
        self.state.event({"event": "build_finished", "revision": self.revision["revision"], "built": built, "reused": reused})
        return result

    # ------------------------------------------------------------------ inputs
    def stage_inputs(self):
        out = {}
        for name, source in sorted(self.m["inputs"].items()):
            path = Path(source)
            if not path.is_file():
                raise ToolError("production", "INPUT_MISSING", f"inputs.{name}: {source} does not exist")
            sha = sha256_file(path)
            target = f"sources/{name}-{sha[:12]}{path.suffix.lower()}"
            key = self.key("input", {"name": name, "sha256": sha, "target": target})

            def run(attempt, path=path, target=target, sha=sha):
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
                return [self.ident(target)], {"source": str(path), "path": target}
            out[name] = self.stage(f"input:{name}", key, {"name": name, "source": str(path), "sha256": sha}, run)["result"]["path"]
        return out

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
            length = min(fr(music["duration"]), total)
            ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "music", "collision": "reject", "clip": {
                "id": "m-music", "asset_id": music["id"], "start": rt(0), "source_in": rt(0), "duration": rt(length)}}})
            fade = min(self.m["music"]["fade_out"], length)
            if fade > 0:
                ops.append({"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": ["m-music"], "fade_out": rt(fade)}})
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
        request = {"audio": audio["key"], "music": None if not music else {"bed_db": fstr(music["bed_db_under_voice"]), "duck": music["duck_milli"]},
                   "target": self.m["delivery"]["loudness_lkfs"], "peak": self.m["delivery"]["peak_dbfs"]}
        key = self.key("mix", request, {"engine": self.engine_identity})

        def run(attempt):
            snapshot = json.loads((self.root / audio["result"]["snapshot"]).read_text(encoding="utf-8"))
            report = {}
            if music:
                meters = self.engine.call("timeline.meters", {"project": snapshot, "tracks": True}, label="mix")
                per = {t["track_id"]: t["meters"] for t in meters.get("tracks", [])}
                voice_lk, music_lk = per["voice"]["integrated_lkfs"], per["music"]["integrated_lkfs"]
                gain = round(1000 * 10 ** ((voice_lk - float(music["bed_db_under_voice"]) - music_lk) / 20))
                gain = max(1, min(4000, gain))
                report["before"] = {"voice_lkfs": voice_lk, "music_lkfs": music_lk, "music_gain_milli": gain}
                snapshot = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": [
                    {"op": "tracks.edit", "edit": {"op": "clip_audio", "clip_ids": ["m-music"], "gain_milli": gain}}]})
                duck = self.engine.call("audio.duck", {"project": snapshot, "voice_track_id": "voice", "music_track_id": "music",
                                                       "duck_milli": music["duck_milli"], "attack": "3/10", "release": "4/5", "bridge": "6/5"}, label="mix")
                if duck.get("operations"):
                    snapshot = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": duck["operations"]})
                report["duck"] = {"speech_runs": len(duck.get("speech", duck.get("runs", [])) or []), "operations": len(duck.get("operations", []))}
            norm = self.engine.call("audio.normalize", {"project": snapshot, "target_lkfs": self.m["delivery"]["loudness_lkfs"],
                                                        "peak_ceiling_dbfs": self.m["delivery"]["peak_dbfs"]}, label="mix")
            if norm.get("operations"):
                snapshot = self.engine.call("timeline.apply", {"project": snapshot, "expected_revision": snapshot["revision"], "operations": norm["operations"]})
            report["normalize"] = {k: norm.get(k) for k in ("result", "limited_by", "factor") if k in norm}
            path = f"generated/timeline/mixed-{key[:12]}-{attempt}.json"
            write_new(self.root / path, snapshot)
            return [self.ident(path)], {"snapshot": path, "report": report}
        receipt = self.stage("mix", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("mix") == "built":
            self.log(f"mix: bed, duck and loudness set ({timer.seconds():.1f}s)")
        return receipt

    # ------------------------------------------------------------------ scenes
    def stage_scenes(self, timing, aligned, captions, art, inputs):
        timer = Timer()
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        patterns = pixel_stage.check_patterns(self.m["patterns"])
        fonts = {k: inputs[v] for k, v in self.m["fonts"].items()}
        layouts = pixel_stage.caption_layouts(fonts, self.m["colors"])
        self.mkdir("scenes")

        def cue_resolver(scene_id):
            words = aligned.get(scene_id, {}).get("words", [])
            norm = [(pixel_stage.words_of(w["text"]) or [""])[0] for w in words]

            def at(cue):
                hits = [i for i, w in enumerate(norm) if w == cue["word"]]
                if len(hits) <= cue["nth"]:
                    raise ToolError("production", "CUE_NOT_HEARD", f"scene {scene_id}: cue word {cue['word']!r} is not in the aligned narration")
                word = words[hits[cue["nth"]]]
                t = fr(word["start"] if cue["edge"] == "start" else word["end"])
                return pixel_stage.snap(self.m["timing"]["lead"] + t)
            return at

        def build_one(item):
            number, scene = item
            row = rows[scene["id"]]
            duration = F(row["duration"])
            override = self.m["overrides"]["scenes"].get(scene["id"])
            resolved = None
            if override:
                base, resolved = self.resolve_override(scene, override, duration, inputs, cue_resolver(scene["id"]))
            else:
                base = pixel_stage.SceneBuilder(scene, duration, art, self.art_index, patterns, self.m["colors"], cue_resolver(scene["id"]),
                                                fonts, number).build()
            base_path = f"scenes/{scene['id']}-base-{digest(base)[:12]}.json"
            if not (self.root / base_path).exists():
                write_new(self.root / base_path, base)
            final_path = base_path
            if self.m["delivery"]["captions"]["burn_in"] and captions:
                captioned = f"scenes/{scene['id']}-{digest([base, captions['key'], row['start']])[:12]}.json"
                if not (self.root / captioned).exists():
                    self.engine.call("captions.scene", {"document": {"file": captions["result"]["draft"], "select": "document"},
                                                        "scene": {"file": base_path}, "scene_id": scene["id"], "offset": rt(F(row["start"])),
                                                        "layouts": layouts, "sampling": "sample_start", "layer_prefix": "cap", "save_as": captioned},
                                     label=f"scene:{scene['id']}")
                final_path = captioned
            recipe = json.loads((self.root / final_path).read_text(encoding="utf-8"))
            recipe = recipe.get("scene", recipe)
            # The key is the recipe's content alone: a neighbour's retiming or a caption change elsewhere leaves it alone.
            # An override's key also names its source file, every cue time it resolved and every input identity it used.
            request = {"recipe": digest(recipe)}
            if resolved:
                request["override"] = resolved
            key = self.key(f"scene:{scene['id']}", request, {"engine": self.engine_identity})

            def run(attempt):
                out = f"renders/{scene['id']}-{key[:12]}-{attempt}.mkv"
                scene_arg = {"file": final_path, "select": "scene"} if final_path != base_path else {"file": final_path}
                result = self.engine.job("scene.render", {"scene": scene_arg, "output": out}, request_id=f"scene-{scene['id']}-{key[:16]}-{attempt}",
                                         lane=f"lane-{number % self.lanes}", label=f"scene:{scene['id']}")
                # Asset IDs name content, so a revised scene enters the saved project beside the old one, never as it.
                asset = dict(result["asset"], id=f"{scene['id']}-{key[:12]}")
                return [self.ident(asset["path"])], {"asset": asset, "recipe": final_path, "layers": len(recipe["layers"])}
            receipt = self.stage(f"scene:{scene['id']}", key, request, run, {"engine": self.engine_identity})
            return scene["id"], receipt["result"]["asset"]

        with ThreadPoolExecutor(max_workers=self.lanes) as pool:
            rendered = dict(pool.map(build_one, enumerate(self.m["scenes"])))
        built = sum(1 for s in self.m["scenes"] if self.outcomes.get(f"scene:{s['id']}") == "built")
        self.log(f"scenes: {built} rendered, {len(rendered) - built} reused ({timer.seconds():.1f}s)")
        return rendered

    def resolve_override(self, scene, override, duration, inputs, at):
        """A hand-written recipe with its cue times, input identities and scene length filled in (see override.py).
        It is read from the production's own copy and checked again, so the build uses exactly what it hashed."""
        source = inputs[override["input"]]
        where = f"overrides.scenes.{scene['id']}"
        identities = {}

        def identity(name):
            if name not in identities:
                identities[name] = {k: v for k, v in self.ident(inputs[name]).items() if k in ("path", "sha256", "bytes")}
            return identities[name]
        try:
            document = json.loads((self.root / source).read_text(encoding="utf-8"))
            recipe, report = override_module.prepare(document, scene, self.m["inputs"], where, override_module.Resolution(at, identity, duration))
        except (json.JSONDecodeError, UnicodeDecodeError) as error:
            raise ToolError("production", "INVALID_OVERRIDE", f"{where}: {source} is not a JSON scene recipe: {error}") from None
        except override_module.OverrideError as error:
            raise ToolError("production", "INVALID_OVERRIDE", str(error)) from None
        if report["cues"]:
            self.log(f"scene {scene['id']}: " + ", ".join(f"{c['word']} at frame {c['frame']}" for c in report["cues"]))
        return recipe, {"source": self.ident(source)["sha256"], **report}

    # ------------------------------------------------------------------ cut (saved session)
    def stage_cut(self, mixed, rendered, timing):
        timer = Timer()
        rows = {r["id"]: r for r in timing["result"]["scenes"]}
        snapshot = json.loads((self.root / mixed["result"]["snapshot"]).read_text(encoding="utf-8"))
        ops = [{"op": "media.add", "asset": rendered[s["id"]]} for s in self.m["scenes"]]
        for s in self.m["scenes"]:
            row = rows[s["id"]]
            ops.append({"op": "tracks.edit", "edit": {"op": "place", "track_id": "picture", "collision": "reject", "clip": {
                "id": f"v-{s['id']}", "asset_id": rendered[s["id"]]["id"], "start": rt(F(row["start"])), "source_in": rt(0),
                "duration": rt(F(row["duration"]))}}})
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
        receipt = self.stage("cut", key, request, run, {"engine": self.engine_identity})
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
            return [self.ident(audio), self.ident(f"{folder}/review.json")], {
                "folder": folder, "audio": audio, "summary": result.get("summary"),
                "speech": {k: speech.get(k) for k in ("match_ratio", "matched", "expected", "heard", "differences") if k in speech}}
        receipt = self.stage("speech-review", key, request, run, {"engine": self.engine_identity})
        if self.outcomes.get("speech-review") == "built":
            self.log(f"speech check: {receipt['result']['folder']} ({timer.seconds():.1f}s)")
        return receipt


# ---------------------------------------------------------------------------------------- helpers
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
    """What a snapshot plays: its settings, tracks with clips keyed by ID, links, the end, limiters, and the assets its clips use.
    Array order inside a track and assets no clip uses do not change playback, so they are left out."""
    tracks = snapshot.get("tracks") or {}
    clips = {c["id"]: c for t in tracks.get("tracks", []) for c in t["clips"]}
    used = {c.get("asset_id") for c in clips.values()}
    return {
        "settings": {k: v for k, v in snapshot.items() if k not in ("revision", "assets", "tracks", "clips")},
        "sequence": snapshot.get("clips"),
        "assets": {a["id"]: a for a in snapshot.get("assets", []) if a["id"] in used},
        "end": tracks.get("duration"), "master": tracks.get("master"),
        "links": sorted((json.dumps(link, sort_keys=True) for link in tracks.get("links", []))),
        "tracks": {t["id"]: {**{k: v for k, v in t.items() if k != "clips"}, "clips": {c["id"]: c for c in t["clips"]}}
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
                if field != "clips" and lt.get(field) != rt_.get(field):
                    out.append(f"tracks.{tid}.{field}")
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
    placements, then levels and fades. Unchanged clips get no operation, so a one-scene revision touches one clip."""
    head_assets = {a["id"] for a in head.get("assets", [])}
    ops = [{"op": "media.add", "asset": a} for a in desired.get("assets", []) if a["id"] not in head_assets]
    if desired.get("transfer") and head.get("transfer") != desired.get("transfer"):
        ops.append({"op": "project.transfer", "transfer": desired["transfer"]})

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
    ops += [{"op": "tracks.edit", "edit": change} for change in levels]
    # Timeline limiters: the master (from audio.normalize) and any audio track's.
    if (head.get("tracks") or {}).get("master") != desired["tracks"].get("master"):
        ops.append({"op": "tracks.edit", "edit": {"op": "audio_dynamics", "dynamics": desired["tracks"].get("master")}})
    head_tracks = {t["id"]: t for t in (head.get("tracks") or {}).get("tracks", [])}
    for track in desired["tracks"]["tracks"]:
        if track["kind"] == "audio" and head_tracks.get(track["id"], {}).get("dynamics") != track.get("dynamics"):
            ops.append({"op": "tracks.edit", "edit": {"op": "audio_dynamics", "track_id": track["id"], "dynamics": track.get("dynamics")}})
    return ops
