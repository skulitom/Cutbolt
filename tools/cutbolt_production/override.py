"""Hand-written scene recipes (`overrides.scenes`): word cues and input references, resolved by the coordinator.

An override recipe is an ordinary Cutbolt scene recipe in which a few values may name production data instead of
fixed numbers and paths. The coordinator resolves them before the engine sees the recipe:

* A cue time, `{"cue": "zooms"}` or `{"cue": {"word", "nth", "edge"}, "offset": "-4/25"}`, names a word of the scene's
  own script. It lands where a template cue lands: on the frame nearest the time the narrator says the word, after the
  scene's lead, plus an optional offset in whole frames. As a layer's `start` it is a time in the scene; as a `time`
  anywhere inside a layer (keyframes of position, opacity, masks, effects) it is measured from that layer's start.
* A layer's `duration` may be `{"until": <cue time>}` or `{"until": "end"}`: the layer lasts until that word, or until
  the scene ends.
* The scene's `duration` may be `"scene"`: the length the timing plan gives the scene.
* `{"input": "<name>"}` stands, wherever the engine expects a file identity (frame images and mattes, fonts, audio,
  LUTs), for the full identity `{path, sha256, bytes}` of the production's copy of that input.

`prepare` checks all of this against the manifest: cue words against the script, input names against `inputs`.
Given a `Resolution` (the aligned narration's cue times, the input identities and the scene's length) it also returns
the resolved recipe, with a report of every cue time and identity it used. Nothing here reads files or runs tools.
"""
import copy
from fractions import Fraction as F

from . import pixel_stage
from .engine import ToolError

FPS = pixel_stage.FPS
SUFFIXES = {"image": (".png",), "matte": (".png",), "fonts": (".ttf", ".otf")}


class OverrideError(ValueError):
    pass


class Resolution:
    """What resolving needs from a build: `at(cue)`, the snapped scene time of a checked cue; `identity(name)`, an
    input's {path, sha256, bytes}; and `duration`, the scene's length in the timing plan."""

    def __init__(self, at, identity, duration):
        self.at, self.identity, self.duration = at, identity, F(duration)


def unwrap(document):
    """A recipe file holds the scene itself, or {"scene": ...} as the engine's commands save it."""
    return document.get("scene", document) if isinstance(document, dict) else document


def fstr(x):
    return f"{x.numerator}/{x.denominator}"


def rt(x):
    return {"num": x.numerator, "den": x.denominator}


def exact(value, where, signed=False):
    """Exact seconds in any form the engine reads: 3, "6/25", "0.24", 0.24 or {"num", "den"}."""
    try:
        if isinstance(value, bool):
            raise ValueError
        if isinstance(value, dict):
            if set(value) != {"num", "den"} or not all(type(value[k]) is int for k in value):
                raise ValueError
            t = F(value["num"], value["den"])
        elif isinstance(value, (int, str)):
            t = F(value)
        elif isinstance(value, float):
            t = F(repr(value))
        else:
            raise ValueError
    except (ValueError, ZeroDivisionError, TypeError):
        raise OverrideError(f"{where}: {value!r} is not an exact time in seconds") from None
    if t < 0 and not signed:
        raise OverrideError(f"{where}: {t} s is negative")
    return t


def is_cue(value):
    return isinstance(value, dict) and "cue" in value


def is_until(value):
    return isinstance(value, dict) and "until" in value


def has_cue(value):
    if isinstance(value, dict):
        return is_cue(value.get("time")) or any(has_cue(v) for v in value.values())
    if isinstance(value, list):
        return any(has_cue(v) for v in value)
    return False


def timing_error(where, message):
    return ToolError("production", "OVERRIDE_TIMING", f"{where}: {message}")


def prepare(document, scene, inputs, where, resolution=None):
    """Check an override recipe against the manifest, and with a `Resolution` resolve it. Returns (recipe, report).

    `scene` is the manifest's normalized scene (its id and script), `inputs` the manifest's name -> path map.
    Without a resolution the recipe comes back unchanged and the report lists the cues and inputs it uses."""
    recipe = unwrap(document)
    if not isinstance(recipe, dict) or not isinstance(recipe.get("layers"), list) or not recipe["layers"]:
        raise OverrideError(f"{where}: give a scene recipe with a nonempty layers list")
    recipe = copy.deepcopy(recipe)
    resolving = resolution is not None
    cues, names = [], set()

    def cue_time(value, at):
        """Check a cue time; when resolving, return its time in the scene. Every use is recorded in the report."""
        if set(value) - {"cue", "offset"}:
            raise OverrideError(f"{at}: a cue time has cue and an optional offset; got {sorted(value)}")
        try:
            cue = pixel_stage.check_cue(value["cue"], scene, at)
        except pixel_stage.TemplateError as error:
            raise OverrideError(str(error)) from None
        offset = exact(value.get("offset", 0), f"{at}.offset", signed=True)
        if (offset * FPS).denominator != 1 or abs(offset) > 120:
            raise OverrideError(f"{at}.offset: {offset} s is not a whole number of {FPS} fps frames within 120 s")
        record = {"at": at, **cue, "offset": fstr(offset)}
        cues.append(record)
        if not resolving:
            return None
        t = resolution.at(cue) + offset
        record.update(time=fstr(t), frame=int(t * FPS))
        return t

    def reference(value, at, key):
        name = value["input"]
        if set(value) != {"input"}:
            raise OverrideError(f"{at}: an input reference is {{\"input\": name}} alone; got {sorted(value)}")
        if not isinstance(name, str) or name not in inputs:
            raise OverrideError(f"{at}: {name!r} is not declared in inputs ({sorted(inputs)})")
        suffixes = SUFFIXES.get(key)
        if suffixes and not inputs[name].lower().endswith(suffixes):
            raise OverrideError(f"{at}: input {name!r} must be a {'/'.join(suffixes)} file here")
        names.add(name)
        return resolution.identity(name) if resolving else value

    def walk(value, at, key=None, clock=None):
        """Replace input references; inside a layer (`clock` is its start and length, or () when only checking),
        resolve the cue times of `time` fields on the layer's own clock."""
        if isinstance(value, list):
            return [walk(v, f"{at}[{i}]", key, clock) for i, v in enumerate(value)]
        if not isinstance(value, dict):
            return value
        if "input" in value:
            return reference(value, at, key)
        if "cue" in value or "until" in value:
            raise OverrideError(f"{at}: cue times may be a layer's start, a time inside a layer, or what a layer's duration lasts until")
        out = {}
        for k, v in value.items():
            if k == "time" and clock is not None and is_cue(v):
                t = cue_time(v, f"{at}.time")
                if not resolving:
                    out[k] = v
                    continue
                begin, length = clock
                if not 0 <= t - begin <= length:
                    raise timing_error(f"{at}.time", f"cue {v['cue']!r} lands at {float(t):.2f} s, outside the layer "
                                                     f"({float(begin):.2f}-{float(begin + length):.2f} s)")
                out[k] = rt(t - begin)
            else:
                out[k] = walk(v, f"{at}.{k}", k, clock)
        return out

    D = resolution.duration if resolving else None
    if recipe.get("duration") == "scene":
        report_duration = "scene"
        if resolving:
            recipe["duration"] = rt(D)
    else:
        own = exact(recipe.get("duration"), f"{where}.duration")
        report_duration = fstr(own)
        if resolving and own != D:
            raise ToolError("production", "OVERRIDE_DURATION", f"{where}: the recipe lasts {own} s but the timing plan gives scene "
                            f"{scene['id']!r} {D} s; set the recipe's duration to \"scene\", or give the scene a fixed duration")
    for k in list(recipe):
        if k not in ("duration", "layers"):
            recipe[k] = walk(recipe[k], f"{where}.{k}", k)

    for i, layer in enumerate(recipe["layers"]):
        if not isinstance(layer, dict):
            raise OverrideError(f"{where}.layers[{i}]: a layer is an object")
        at = f"{where}.layers[{i}] ({layer.get('id')})"
        start, length = layer.get("start"), layer.get("duration")
        if is_cue(start):
            t = cue_time(start, f"{at}.start")
            if resolving:
                if not 0 <= t < D:
                    raise timing_error(f"{at}.start", f"cue {start['cue']!r} lands at {float(t):.2f} s, outside the {float(D):.2f} s scene")
                layer["start"] = rt(t)
        if is_until(length):
            until = length["until"]
            if set(length) != {"until"} or not (until == "end" or is_cue(until)):
                raise OverrideError(f"{at}.duration: {{\"until\": \"end\"}} or {{\"until\": <cue time>}}")
            end = cue_time(until, f"{at}.duration.until") if until != "end" else D
            if resolving:
                begin = exact(layer.get("start"), f"{at}.start")
                if end <= begin:
                    raise timing_error(f"{at}.duration", f"the layer would end at {float(end):.2f} s, no later than it starts ({float(begin):.2f} s)")
                if end > D:
                    raise timing_error(f"{at}.duration", f"the layer would end at {float(end):.2f} s, after the scene ends ({float(D):.2f} s)")
                layer["duration"] = rt(end - begin)
        clock = ()
        if resolving:
            if any(has_cue(v) for k, v in layer.items() if k not in ("start", "duration")):
                clock = (exact(layer.get("start"), f"{at}.start"), exact(layer.get("duration"), f"{at}.duration"))
            else:
                clock = None
        for k in list(layer):
            if k in ("start", "duration") and (is_cue(layer[k]) or is_until(layer[k])):
                continue   # only while checking: resolving has replaced them
            layer[k] = walk(layer[k], f"{at}.{k}", k, None if k in ("start", "duration") else clock)

    report = {"cues": cues, "inputs": sorted(names), "duration": report_duration}
    if resolving:
        report["inputs"] = {name: resolution.identity(name) for name in sorted(names)}
        report["duration"] = fstr(D)
    return recipe, report
