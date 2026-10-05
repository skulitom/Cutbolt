"""Production manifest: parse, validate strictly and normalize.

A manifest is portable production data: the script, one visual beat per scene, palette and style, music and
delivery settings. Machine facts (engine, PixelForge and Qwen locations) live in a separate local config.
Unknown fields fail; nothing is guessed. See docs/PRODUCTION.md for the format.
"""
import hashlib
import json
import re
from fractions import Fraction as F
from pathlib import Path, PureWindowsPath

from . import CONTRACT_VERSION, override, pixel_stage

ID = re.compile(r"^[a-z0-9][a-z0-9-]{0,47}$")
NAME = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")
GATES = ("script", "storyboard", "narration", "rough_cut", "final_export")


class ManifestError(ValueError):
    pass


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False)


def digest(value):
    return hashlib.sha256(canonical(value).encode("utf-8")).hexdigest()


def fields(obj, allowed, where, required=()):
    if not isinstance(obj, dict):
        raise ManifestError(f"{where}: expected an object")
    unknown = sorted(set(obj) - set(allowed))
    if unknown:
        raise ManifestError(f"{where}: unknown field(s) {unknown}; expected {sorted(allowed)}")
    missing = [k for k in required if k not in obj]
    if missing:
        raise ManifestError(f"{where}: missing required field(s) {missing}")


def exact_time(value, where, minimum=F(0), maximum=F(600)):
    """Exact seconds from 3, "6/25", "0.24" or {"num", "den"}; floats are read as their decimal text."""
    try:
        if isinstance(value, dict):
            fields(value, {"num", "den"}, where, ["num", "den"])
            t = F(value["num"], value["den"])
        elif isinstance(value, bool):
            raise ValueError
        elif isinstance(value, (int, str)):
            t = F(value)
        elif isinstance(value, float):
            t = F(repr(value))
        else:
            raise ValueError
    except (ValueError, ZeroDivisionError, TypeError):
        raise ManifestError(f"{where}: {value!r} is not an exact time in seconds") from None
    if not minimum <= t <= maximum:
        raise ManifestError(f"{where}: {t} s is outside {minimum}..{maximum} s")
    return t


def local_path(value, where):
    if not isinstance(value, str) or not value:
        raise ManifestError(f"{where}: give an absolute local file path")
    win = PureWindowsPath(value)
    if value.startswith(("\\\\", "//")) or "://" in value or not win.is_absolute() or not win.drive:
        raise ManifestError(f"{where}: {value!r} must be an absolute local path (no network or relative paths)")
    if ":" in value[2:]:
        raise ManifestError(f"{where}: {value!r} names an alternate data stream")
    return str(Path(value))


def load(path):
    try:
        text = Path(path).read_text(encoding="utf-8")
    except OSError as error:
        raise ManifestError(f"cannot read manifest {path}: {error}") from None
    try:
        raw = json.loads(text)
    except json.JSONDecodeError as error:
        raise ManifestError(f"manifest is not JSON: {error}") from None
    return validate(raw)


def validate(raw):
    fields(raw, {"contract_version", "production_id", "title", "language", "template", "voice", "music", "inputs", "fonts",
                 "palette", "patterns", "timing", "delivery", "review_policy", "scenes", "overrides", "notes"},
           "manifest", ["contract_version", "production_id", "template", "inputs", "fonts", "scenes"])
    if raw["contract_version"] != CONTRACT_VERSION:
        raise ManifestError(f"contract_version must be {CONTRACT_VERSION!r}; got {raw['contract_version']!r}")
    if not isinstance(raw["production_id"], str) or not ID.match(raw["production_id"]):
        raise ManifestError("production_id: 1-48 lowercase letters, digits or -, starting with a letter or digit")
    out = {"contract_version": CONTRACT_VERSION, "production_id": raw["production_id"],
           "title": str(raw.get("title") or raw["production_id"])[:200], "language": raw.get("language", "en")}
    if out["language"] not in ("en",):
        raise ManifestError("language: the pixel-stage template supports English narration alignment only (en)")

    template = raw["template"]
    fields(template, {"id", "version"}, "template", ["id", "version"])
    if template["id"] != pixel_stage.TEMPLATE_ID or template["version"] != pixel_stage.TEMPLATE_VERSION:
        raise ManifestError(f"template: only {pixel_stage.TEMPLATE_ID!r} version {pixel_stage.TEMPLATE_VERSION} exists")
    out["template"] = {"id": template["id"], "version": template["version"]}

    inputs = raw["inputs"]
    fields(inputs, set(inputs) if isinstance(inputs, dict) else set(), "inputs")
    out["inputs"] = {}
    for name, value in inputs.items():
        if not NAME.match(name):
            raise ManifestError(f"inputs.{name}: names are 1-32 lowercase letters, digits, _ or -")
        out["inputs"][name] = local_path(value, f"inputs.{name}")

    def input_ref(name, where, suffixes):
        if name not in out["inputs"]:
            raise ManifestError(f"{where}: {name!r} is not declared in inputs ({sorted(out['inputs'])})")
        if not out["inputs"][name].lower().endswith(suffixes):
            raise ManifestError(f"{where}: input {name!r} must be a {'/'.join(suffixes)} file")
        return name

    fonts = raw["fonts"]
    fields(fonts, {"bold", "regular"}, "fonts", ["bold", "regular"])
    out["fonts"] = {k: input_ref(fonts[k], f"fonts.{k}", (".ttf", ".otf")) for k in ("bold", "regular")}

    colors = dict(pixel_stage.SLOTS)
    pal = raw.get("palette") or {}
    fields(pal, set(pixel_stage.SLOTS), "palette")
    for slot, value in pal.items():
        try:
            colors[slot] = pixel_stage.check_color(value, f"palette.{slot}")
        except pixel_stage.TemplateError as error:
            raise ManifestError(str(error)) from None
    out["palette"] = {k: colors[k] for k in sorted(pal)}
    out["colors"] = colors
    try:
        patterns = pixel_stage.check_patterns(raw.get("patterns"))
    except pixel_stage.TemplateError as error:
        raise ManifestError(str(error)) from None
    out["patterns"] = {k: [list(s) for s in v] for k, v in patterns.items() if k not in pixel_stage.PATTERNS}

    music = raw.get("music")
    if music is not None:
        fields(music, {"input", "bpm", "bed_db_under_voice", "duck_milli", "fade_out"}, "music", ["input"])
        bpm = music.get("bpm")
        out["music"] = {"input": input_ref(music["input"], "music.input", (".wav",)),
                        "bpm": None if bpm is None else exact_time(bpm, "music.bpm", F(40), F(240)),
                        "bed_db_under_voice": exact_time(music.get("bed_db_under_voice", 6), "music.bed_db_under_voice", F(0), F(30)),
                        "duck_milli": int(music.get("duck_milli", 300)),
                        "fade_out": exact_time(music.get("fade_out", "48/25"), "music.fade_out", F(0), F(10))}
        if not 0 <= out["music"]["duck_milli"] <= 1000:
            raise ManifestError("music.duck_milli: 0-1000")
    else:
        out["music"] = None

    timing = raw.get("timing") or {}
    fields(timing, {"lead", "tail", "snap", "title_bars", "end_min_bars", "min_scene"}, "timing")
    snap = timing.get("snap", "beat" if out["music"] and out["music"]["bpm"] else "frame")
    if snap not in ("frame", "beat", "bar"):
        raise ManifestError("timing.snap: frame, beat or bar")
    out["timing"] = {"lead": exact_time(timing.get("lead", "6/25"), "timing.lead", F(0), F(5)),
                     "tail": exact_time(timing.get("tail", "2/5"), "timing.tail", F(0), F(10)),
                     "snap": snap, "title_bars": int(timing.get("title_bars", 2)), "end_min_bars": int(timing.get("end_min_bars", 4)),
                     "min_scene": exact_time(timing.get("min_scene", 3), "timing.min_scene", F(1, 25), F(120))}
    beat_seconds = F(60) / out["music"]["bpm"] if out["music"] and out["music"]["bpm"] else F(12, 25)
    if snap != "frame" and not (out["music"] and out["music"]["bpm"]):
        raise ManifestError("timing.snap: beat and bar snapping need music.bpm")
    if (beat_seconds * pixel_stage.FPS).denominator != 1:
        raise ManifestError(f"music.bpm: a beat of {beat_seconds} s is not a whole number of 25 fps frames; "
                            f"use a tempo such as 125 or 100 BPM, or timing.snap frame")
    out["timing"]["beat"] = beat_seconds
    for key in ("lead", "tail"):
        if (out["timing"][key] * pixel_stage.FPS).denominator != 1:
            raise ManifestError(f"timing.{key}: {out['timing'][key]} s is not a whole number of frames")

    delivery = raw.get("delivery") or {}
    fields(delivery, {"captions", "loudness_lkfs", "peak_dbfs", "review"}, "delivery")
    captions = delivery.get("captions") or {}
    fields(captions, {"burn_in", "sidecars", "line_chars", "lines"}, "delivery.captions")
    sidecars = captions.get("sidecars", ["srt", "vtt"])
    if not isinstance(sidecars, list) or set(sidecars) - {"srt", "vtt"}:
        raise ManifestError("delivery.captions.sidecars: a list of srt and/or vtt")
    review = delivery.get("review") or {}
    fields(review, {"speech", "frames", "preview"}, "delivery.review")
    out["delivery"] = {
        "captions": {"burn_in": bool(captions.get("burn_in", True)), "sidecars": sorted(set(sidecars)),
                     "line_chars": int(captions.get("line_chars", 40)), "lines": int(captions.get("lines", 2))},
        "loudness_lkfs": float(delivery.get("loudness_lkfs", -14)), "peak_dbfs": float(delivery.get("peak_dbfs", -1)),
        "review": {"speech": bool(review.get("speech", True)), "frames": int(review.get("frames", 16)), "preview": bool(review.get("preview", False))}}
    if not -40 <= out["delivery"]["loudness_lkfs"] <= -5 or not -20 <= out["delivery"]["peak_dbfs"] <= 0:
        raise ManifestError("delivery: loudness_lkfs -40..-5 and peak_dbfs -20..0")

    policy = raw.get("review_policy") or {}
    fields(policy, {"version", "gates", "human_required"}, "review_policy")
    gates = policy.get("gates", list(GATES))
    human = policy.get("human_required", ["final_export"])
    if not isinstance(gates, list) or set(gates) - set(GATES) or not isinstance(human, list) or set(human) - set(gates):
        raise ManifestError(f"review_policy: gates from {list(GATES)}; human_required must be a subset of gates")
    out["review_policy"] = {"version": str(policy.get("version", "production-gates-v1")), "gates": [g for g in GATES if g in gates],
                            "human_required": [g for g in GATES if g in human]}

    scenes = raw["scenes"]
    if not isinstance(scenes, list) or not 1 <= len(scenes) <= 24:
        raise ManifestError("scenes: 1-24 scenes")
    overrides = raw.get("overrides") or {}
    fields(overrides, {"narration", "scenes"}, "overrides")
    narration_over = overrides.get("narration") or {}
    scene_over = overrides.get("scenes") or {}
    out["scenes"] = []
    seen = set()
    for i, scene in enumerate(scenes):
        where = f"scenes[{i}]"
        fields(scene, {"id", "title", "script", "beat", "duration"}, where, ["id"])
        if not isinstance(scene["id"], str) or not ID.match(scene["id"]) or scene["id"] in seen:
            raise ManifestError(f"{where}.id: a unique 1-48 character lowercase id")
        if "beat" not in scene and scene["id"] not in scene_over:
            raise ManifestError(f"{where}: missing required field(s) ['beat']; a scene without a beat needs overrides.scenes.{scene['id']}")
        seen.add(scene["id"])
        script = scene.get("script")
        if script is not None and (not isinstance(script, str) or not script.strip() or len(script) > 600):
            raise ManifestError(f"{where}.script: 1-600 characters, or omit for a silent scene")
        norm = {"id": scene["id"], "title": str(scene.get("title") or ""), "script": " ".join(script.split()) if script else None}
        if script is not None and not pixel_stage.words_of(script):
            raise ManifestError(f"{where}.script: no words to narrate")
        duration = scene.get("duration")
        if duration is not None:
            if isinstance(duration, dict) and set(duration) == {"bars"}:
                if not (out["music"] and out["music"]["bpm"]):
                    raise ManifestError(f"{where}.duration: bars need music.bpm")
                norm["duration"] = out["timing"]["beat"] * 4 * int(duration["bars"])
            else:
                norm["duration"] = exact_time(duration, f"{where}.duration", F(1, 25), F(120))
            if (norm["duration"] * pixel_stage.FPS).denominator != 1:
                raise ManifestError(f"{where}.duration: whole frames at 25 fps")
        else:
            norm["duration"] = None
        try:
            norm["beat"] = pixel_stage.check_beat({**norm, "beat": scene["beat"]}, patterns, colors) if "beat" in scene else None
        except pixel_stage.TemplateError as error:
            raise ManifestError(str(error)) from None
        if norm["beat"] and norm["beat"]["type"] == "end":
            norm["fonts_needed"] = True
        out["scenes"].append(norm)
    for key in narration_over:
        if key not in seen:
            raise ManifestError(f"overrides.narration.{key}: no such scene")
    for key in scene_over:
        if key not in seen:
            raise ManifestError(f"overrides.scenes.{key}: no such scene")
    out["overrides"] = {"narration": {}, "scenes": {}}
    for key, value in narration_over.items():
        fields(value, {"input"}, f"overrides.narration.{key}", ["input"])
        if next(s for s in out["scenes"] if s["id"] == key)["script"] is None:
            raise ManifestError(f"overrides.narration.{key}: the scene has no script to align the take to")
        out["overrides"]["narration"][key] = {"input": input_ref(value["input"], f"overrides.narration.{key}.input", (".wav",))}
    for key, value in scene_over.items():
        where = f"overrides.scenes.{key}"
        fields(value, {"input"}, where, ["input"])
        name = input_ref(value["input"], f"{where}.input", (".json",))
        # The recipe's cue words and input references are checked now, as a template beat's are.
        try:
            document = json.loads(Path(out["inputs"][name]).read_text(encoding="utf-8"))
        except (OSError, UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ManifestError(f"{where}.input: cannot read the scene recipe {out['inputs'][name]}: {error}") from None
        scene = next(s for s in out["scenes"] if s["id"] == key)
        try:
            _, uses = override.prepare(document, scene, out["inputs"], where)
        except override.OverrideError as error:
            raise ManifestError(str(error)) from None
        out["overrides"]["scenes"][key] = {"input": name, **uses}

    voice = raw.get("voice")
    needs_voice = any(s["script"] and s["id"] not in out["overrides"]["narration"] for s in out["scenes"])
    if voice is None:
        if needs_voice:
            raise ManifestError("voice: required to synthesize narration (speaker, language, seed)")
        out["voice"] = None
    else:
        fields(voice, {"speaker", "language", "seed", "instruct", "split", "pause"}, "voice", ["speaker"])
        out["voice"] = {"speaker": str(voice["speaker"]), "language": str(voice.get("language", "English")),
                        "seed": int(voice.get("seed", 11)), "instruct": voice.get("instruct"), "split": voice.get("split", "none"),
                        "pause": exact_time(voice.get("pause", "3/10"), "voice.pause", F(0), F(2))}
        if out["voice"]["split"] not in ("none", "sentence"):
            raise ManifestError("voice.split: none (each line in one breath) or sentence (each sentence on its own, joined by pause)")
    out["notes"] = raw.get("notes")
    out["manifest_sha256"] = digest(raw)
    return out


def templated(manifest):
    """The scenes the template draws: those with a beat and no override recipe."""
    return [s for s in manifest["scenes"] if s["beat"] and s["id"] not in manifest["overrides"]["scenes"]]


def summary(manifest):
    """A JSON-safe copy (fractions as strings) for receipts."""
    def clean(value):
        if isinstance(value, F):
            return f"{value.numerator}/{value.denominator}"
        if isinstance(value, dict):
            return {k: clean(v) for k, v in value.items()}
        if isinstance(value, (list, tuple)):
            return [clean(v) for v in value]
        return value
    return clean(manifest)
