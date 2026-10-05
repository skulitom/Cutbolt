"""The pixel-stage explainer template: Pip, a small character on a pixel stage, explains an idea scene by scene.

This is the 5 October 2026 progress demo's art generator and scene builder turned into a template.
A manifest chooses one beat per scene (title, switch, cards, strip, compare, recolor, travel, end),
its labels and cue words, an optional palette and custom motion patterns. The template turns that into:

* PixelForge recipes (original art: Pip's poses, the stage tiles, labels in PixelForge's pixel font,
  pose cards, film strips, timing bars and colour swatches), and
* native 1920x1080 Cutbolt scene recipes in which pixel layers are enlarged six times and every visual
  cue lands on the frame where the narrator says its word.

Everything here is pure: no files, no processes. Times are exact fractions of a second.
"""
import math
import re
from fractions import Fraction as F

TEMPLATE_ID = "pixel-stage-explainer"
TEMPLATE_VERSION = 1
FPS = 25
FRAME = F(1, FPS)
LOGICAL = (320, 180)
FACTOR = 6
SIZE = (LOGICAL[0] * FACTOR, LOGICAL[1] * FACTOR)
GROUND_Y = 124
PIP_CANVAS = [32, 48]
PIP_ANCHOR = [16, 46]

# ---------------------------------------------------------------------------------------- palette
# Named colour slots and the PixelForge palette letters that use them.
SLOTS = {
    "outline": "#1d1b2e", "body": "#7b8cde", "body_shade": "#5563b0", "body_light": "#a9b5f5", "body_glint": "#e3e8ff",
    "feet": "#f29e4c", "feet_shade": "#c46f2a", "scarf": "#e04f5f", "scarf_shade": "#a3324a", "eye_glint": "#ffffff",
    "cheek": "#f49ab0", "dust": "#e6e0d2", "dust_shade": "#bfb6a3",
    "sky_1": "#5fa8e8", "sky_2": "#79bbef", "sky_3": "#96cdf4", "sky_4": "#b7def8",
    "hills_far": "#8cc7a8", "hills_far_shade": "#78b496", "hills_near": "#5ea35a", "hills_near_shade": "#4c8a4a",
    "hills_near_light": "#79bd6a", "grass": "#63b84f", "grass_shade": "#4e9a3e", "grass_light": "#8fd36a",
    "soil": "#9b6a45", "soil_shade": "#7d5236", "soil_dark": "#5f3d28", "pebble": "#c4a27e",
    "cloud": "#ffffff", "cloud_shade": "#dbe9f7",
    "label": "#ffffff", "label_accent": "#ffd166", "ink": "#2b2840", "card": "#f4ecd8", "card_shade": "#d8ccb0",
    "bar_1": "#e07a5f", "bar_2": "#f2cc8f", "bar_3": "#81b29a", "bar_4": "#3d85c6", "bar_5": "#9c89b8",
    "highlight": "#ffd166", "caption_box": "#1d1b2e",
}
PIP_KEYS = {"k": "outline", "B": "body", "D": "body_shade", "H": "body_light", "h": "body_glint", "F": "feet",
            "f": "feet_shade", "s": "scarf", "S": "scarf_shade", "w": "eye_glint", "c": "cheek", "u": "dust", "U": "dust_shade"}
BG_KEYS = {"a1": "sky_1", "a2": "sky_2", "a3": "sky_3", "a4": "sky_4", "g1": "hills_far", "g2": "hills_far_shade",
           "n1": "hills_near", "n2": "hills_near_shade", "n3": "hills_near_light", "G1": "grass", "G2": "grass_shade",
           "G3": "grass_light", "d1": "soil", "d2": "soil_shade", "d3": "soil_dark", "p1": "pebble", "c1": "cloud",
           "c2": "cloud_shade", "k": "outline"}
TEXT_KEYS = {"w": "label", "k": "outline", "y": "label_accent", "i": "ink"}
CARD_KEYS = {**PIP_KEYS, "p": "card", "q": "card_shade", "i": "ink", "y": "highlight"}
BAR_KEYS = {"1": "bar_1", "2": "bar_2", "3": "bar_3", "4": "bar_4", "5": "bar_5", "k": "outline"}


def palette(keys, colors, **extra):
    return {**{key: colors[slot] for key, slot in keys.items()}, **extra}


def rgb(hex_color):
    return [int(hex_color[i:i + 2], 16) for i in (1, 3, 5)]


# ---------------------------------------------------------------------------------------- Pip
POSES = {
    #        body box          feet boxes                         eyes            scarf row  tail
    "stand": ((7, 27, 18, 17), [(10, 43, 5, 3), (17, 43, 5, 3)], ("open", 31), 37, "hang"),
    "blink": ((7, 27, 18, 17), [(10, 43, 5, 3), (17, 43, 5, 3)], ("closed", 31), 37, "hang"),
    "crouch": ((6, 31, 20, 14), [(9, 44, 6, 2), (17, 44, 6, 2)], ("squint", 34), 40, "droop"),
    "push": ((8, 21, 16, 20), [(11, 40, 4, 6), (17, 40, 4, 6)], ("wide", 26), 32, "down"),
    "float": ((7, 10, 18, 17), [(10, 26, 5, 3), (17, 26, 5, 3)], ("happy", 15), 20, "up"),
    "land": ((5, 33, 22, 12), [(8, 44, 6, 2), (18, 44, 6, 2)], ("squint", 36), 41, "flat"),
}
TAILS = {
    "hang": (-1, 2, ["ss", "sS", ".s", ".S"]),
    "droop": (-1, 2, ["ss", "sS", "sS"]),
    "down": (0, 3, ["s", "s", "S", "s", "S"]),
    "up": (-4, -2, ["..ss", ".sS.", "sS..", "S..."]),
    "flat": (-3, 1, ["sss", "SS."]),
}
# Frame holds at 25 fps. A pattern is a cycle of (pose, frames).
HOLDS = {"crouch": 4, "push": 3, "float": 12, "land": 5, "stand": 16, "blink": 4}
PATTERNS = {
    "hop": [("crouch", 4), ("push", 3), ("float", 12), ("land", 5), ("stand", 16)],
    "light": [("crouch", 4), ("push", 2), ("float", 16), ("land", 6), ("stand", 12)],
    "heavy": [(p, 8) for p in ("crouch", "push", "float", "land", "stand")],
    "idle": [("stand", 28), ("blink", 4)],
    "still": [("stand", 25)],
}
BAR_COLOR = {"crouch": "1", "push": "2", "float": "3", "land": "4", "stand": "5", "blink": "5"}
HOP_ORDER = ["crouch", "push", "float", "land"]


def ellipse_span(box, row):
    x, y, w, h = box
    cy = y + (h - 1) / 2
    t = (row - cy) / (h / 2)
    if abs(t) >= 1:
        return None
    half = (w / 2) * math.sqrt(1 - t * t)
    cx = x + (w - 1) / 2
    return int(math.floor(cx - half + 0.5)), int(math.ceil(cx + half - 0.5))


def pip_layers(pose, dx=0, dy=0):
    body, feet, (eye_style, eye_y), scarf_y, tail = POSES[pose]
    bx, by, bw, bh = body
    cx = bx + bw // 2
    feet_ops = []
    for (x, y, w, h) in feet:
        feet_ops += [{"op": "rect", "x": x, "y": y, "w": w, "h": h, "color": "F"},
                     {"op": "rect", "x": x, "y": y + h - 1, "w": w, "h": 1, "color": "f"}]
    feet_ops.append({"op": "outline", "color": "k"})
    layers = [{"name": "feet", "x": dx, "y": dy, "ops": feet_ops}]
    ops = [{"op": "ellipse", "x": bx, "y": by, "w": bw, "h": bh, "color": "B"},
           {"op": "outline", "color": "D", "position": "inside", "width": 2, "directions": ["...", "..x", ".xx"]},
           {"op": "outline", "color": "H", "position": "inside", "directions": ["xx.", "x..", "..."]},
           {"op": "rect", "x": bx + 4, "y": by + 3, "w": 2, "h": 1, "color": "h"},
           {"op": "pixel", "x": bx + 4, "y": by + 4, "color": "h"}]
    left = None
    for i, color in enumerate(["s", "s", "S"]):
        span = ellipse_span(body, scarf_y + i)
        if span is None:
            continue
        lo, hi = span[0] - 1, span[1] + 1
        ops.append({"op": "line", "x": lo, "y": scarf_y + i, "x2": hi, "y2": scarf_y + i, "color": color})
        if i == 0:
            left = lo
    tx, ty, rows = TAILS[tail]
    ops += [{"op": "grid", "x": left + tx, "y": scarf_y + ty, "rows": rows}, {"op": "outline", "color": "k"}]
    layers.append({"name": "body", "x": dx, "y": dy, "ops": ops})
    face = []
    lx, rx = cx - 5, cx + 2
    if eye_style in ("open", "wide"):
        for ex in (lx, rx + 1):
            face += [{"op": "rect", "x": ex, "y": eye_y, "w": 2, "h": 3 if eye_style == "open" else 4, "color": "k"},
                     {"op": "pixel", "x": ex, "y": eye_y, "color": "w"}]
    elif eye_style == "closed":
        for ex in (lx, rx + 1):
            face.append({"op": "line", "x": ex, "y": eye_y + 2, "x2": ex + 1, "y2": eye_y + 2, "color": "k"})
    elif eye_style == "squint":
        face += [{"op": "grid", "x": lx, "y": eye_y, "rows": ["k..", ".kk", "k.."]},
                 {"op": "grid", "x": rx, "y": eye_y, "rows": ["..k", "kk.", "..k"]}]
    elif eye_style == "happy":
        face += [{"op": "grid", "x": lx, "y": eye_y, "rows": [".k.", "k.k"]},
                 {"op": "grid", "x": rx, "y": eye_y, "rows": [".k.", "k.k"]}]
    cheek_y = eye_y + (4 if eye_style == "wide" else 3)
    face += [{"op": "rect", "x": lx - 1, "y": cheek_y, "w": 2, "h": 1, "color": "c"},
             {"op": "rect", "x": rx + 2, "y": cheek_y, "w": 2, "h": 1, "color": "c"}]
    layers.append({"name": "face", "x": dx, "y": dy, "ops": face})
    if pose == "land":
        layers.append({"name": "dust", "x": dx, "y": dy, "ops": [
            {"op": "ellipse", "x": 0, "y": 41, "w": 5, "h": 4, "color": "u"},
            {"op": "ellipse", "x": 27, "y": 41, "w": 5, "h": 4, "color": "u"},
            {"op": "pixel", "x": 2, "y": 44, "color": "U"}, {"op": "pixel", "x": 29, "y": 44, "color": "U"}]})
    return layers


# ---------------------------------------------------------------------------------------- text sizes
# PixelForge's built-in font: capitals and digits are 5 pixels wide, a space 3, one pixel between glyphs.
LABEL_TEXT = re.compile(r"^[A-Z0-9 +&!?.,:'/-]+$")


def text_width(text):
    return sum(3 if ch == " " else 5 for ch in text) + max(0, len(text) - 1)


# ---------------------------------------------------------------------------------------- manifest checks
BEATS = ("title", "switch", "cards", "strip", "compare", "recolor", "travel", "end")


class TemplateError(ValueError):
    pass


def words_of(text):
    """Script words as the aligner spells them, lowercased letters and digits only."""
    return [w for w in (re.sub(r"[^a-z0-9]", "", token.lower()) for token in text.split()) if w]


def check_label(value, where, limit=124):
    if not isinstance(value, str) or not value.strip():
        raise TemplateError(f"{where}: a label must be nonblank text")
    text = value.strip().upper()
    if not LABEL_TEXT.match(text):
        raise TemplateError(f"{where}: {value!r} uses characters outside A-Z, 0-9, space and + & ! ? . , : ' / -")
    if text_width(text) > limit:
        raise TemplateError(f"{where}: {value!r} is {text_width(text)} pixels wide in the pixel font; at most {limit} fit")
    return text


def check_pattern_name(name, patterns, where):
    if name not in patterns:
        raise TemplateError(f"{where}: unknown motion {name!r}; choose from {sorted(patterns)}")
    return name


def check_cue(cue, scene, where):
    """A cue is a word of the scene's script: "word", or {"word", "nth", "edge"}."""
    if cue is None:
        return None
    if scene.get("script") is None:
        raise TemplateError(f"{where}: cues need narration; scene {scene['id']!r} has no script")
    if isinstance(cue, str):
        cue = {"word": cue}
    if not isinstance(cue, dict) or set(cue) - {"word", "nth", "edge"} or "word" not in cue:
        raise TemplateError(f"{where}: a cue is a word or {{\"word\", \"nth\", \"edge\"}}")
    word = re.sub(r"[^a-z0-9]", "", str(cue["word"]).lower())
    nth = cue.get("nth", 0)
    edge = cue.get("edge", "start")
    if type(nth) is not int or nth < 0 or edge not in ("start", "end"):
        raise TemplateError(f"{where}: nth must be a nonnegative integer and edge start or end")
    count = words_of(scene["script"]).count(word)
    if count <= nth:
        raise TemplateError(f"{where}: the script of {scene['id']!r} says {word!r} {count} time(s); cue needs occurrence {nth + 1}")
    return {"word": word, "nth": nth, "edge": edge}


def check_poses(values, where, length=None):
    if not isinstance(values, list) or not values or (length and len(values) != length):
        raise TemplateError(f"{where}: give {length or 'one or more'} poses")
    for pose in values:
        if pose not in POSES:
            raise TemplateError(f"{where}: unknown pose {pose!r}; poses are {sorted(POSES)}")
    return list(values)


def check_fields(obj, allowed, where, required=()):
    if not isinstance(obj, dict):
        raise TemplateError(f"{where}: expected an object")
    unknown = sorted(set(obj) - set(allowed))
    if unknown:
        raise TemplateError(f"{where}: unknown field(s) {unknown}; expected {sorted(allowed)}")
    missing = [k for k in required if k not in obj]
    if missing:
        raise TemplateError(f"{where}: missing {missing}")


def check_beat(scene, patterns, colors):
    """Validate one scene's beat against the template and return its normalized form."""
    beat = scene.get("beat")
    where = f"scenes[{scene['id']}].beat"
    if not isinstance(beat, dict) or beat.get("type") not in BEATS:
        raise TemplateError(f"{where}: type must be one of {list(BEATS)}")
    kind = beat["type"]
    out = {"type": kind}
    if kind == "title":
        check_fields(beat, {"type", "lines"}, where, ["lines"])
        if not isinstance(beat["lines"], list) or not 1 <= len(beat["lines"]) <= 2:
            raise TemplateError(f"{where}.lines: one or two lines")
        out["lines"] = [check_label(t, f"{where}.lines[{i}]", 156) for i, t in enumerate(beat["lines"])]
    elif kind == "switch":
        check_fields(beat, {"type", "steps"}, where, ["steps"])
        steps = beat["steps"]
        if not isinstance(steps, list) or not 1 <= len(steps) <= 6:
            raise TemplateError(f"{where}.steps: 1-6 steps")
        out["steps"] = []
        for i, step in enumerate(steps):
            w = f"{where}.steps[{i}]"
            check_fields(step, {"cue", "label", "motion", "strip"}, w)
            if i == 0 and step.get("cue") is not None:
                raise TemplateError(f"{w}.cue: the first step starts with the scene")
            if i > 0 and step.get("cue") is None:
                raise TemplateError(f"{w}.cue: every later step starts on a cue word")
            out["steps"].append({
                "cue": check_cue(step.get("cue"), scene, f"{w}.cue"),
                "label": check_label(step["label"], f"{w}.label") if step.get("label") else None,
                "motion": check_pattern_name(step["motion"], patterns, f"{w}.motion") if step.get("motion") else None,
                "strip": check_poses(step["strip"], f"{w}.strip", 4) if step.get("strip") else None})
        if out["steps"][0]["motion"] is None:
            out["steps"][0]["motion"] = "idle"
    elif kind == "cards":
        check_fields(beat, {"type", "heading", "cards", "motion"}, where, ["cards"])
        cards = beat["cards"]
        if not isinstance(cards, list) or not 1 <= len(cards) <= 4:
            raise TemplateError(f"{where}.cards: 1-4 cards")
        out["cards"] = []
        for i, card in enumerate(cards):
            w = f"{where}.cards[{i}]"
            check_fields(card, {"pose", "word", "cue"}, w, ["pose", "word", "cue"])
            check_poses([card["pose"]], f"{w}.pose")
            out["cards"].append({"pose": card["pose"], "word": check_label(card["word"], f"{w}.word", 50),
                                 "cue": check_cue(card["cue"], scene, f"{w}.cue")})
        heading = beat.get("heading")
        if heading is not None:
            check_fields(heading, {"label", "cue"}, f"{where}.heading", ["label"])
            out["heading"] = {"label": check_label(heading["label"], f"{where}.heading.label"),
                              "cue": check_cue(heading.get("cue"), scene, f"{where}.heading.cue")}
        else:
            out["heading"] = None
        out["motion"] = check_pattern_name(beat.get("motion", "idle"), patterns, f"{where}.motion")
    elif kind == "strip":
        check_fields(beat, {"type", "order", "label", "then"}, where, ["order"])
        out["order"] = check_poses(beat["order"], f"{where}.order", 4)
        out["label"] = check_label(beat["label"], f"{where}.label") if beat.get("label") else None
        then = beat.get("then")
        if then is not None:
            check_fields(then, {"order", "cue", "label"}, f"{where}.then", ["order", "cue"])
            out["then"] = {"order": check_poses(then["order"], f"{where}.then.order", 4),
                           "cue": check_cue(then["cue"], scene, f"{where}.then.cue"),
                           "label": check_label(then["label"], f"{where}.then.label") if then.get("label") else None}
        else:
            out["then"] = None
    elif kind == "compare":
        check_fields(beat, {"type", "left", "right", "bars"}, where, ["left", "right"])
        for side in ("left", "right"):
            item = beat[side]
            check_fields(item, {"label", "motion"}, f"{where}.{side}", ["label", "motion"])
            out[side] = {"label": check_label(item["label"], f"{where}.{side}.label", 120),
                         "motion": check_pattern_name(item["motion"], patterns, f"{where}.{side}.motion")}
        out["bars"] = bool(beat.get("bars", True))
        if out["bars"]:
            for side in ("left", "right"):
                frames = sum(n for _, n in patterns[out[side]["motion"]])
                if frames > 58:
                    raise TemplateError(f"{where}.{side}: timing bars fit motions of at most 58 frames; "
                                        f"{out[side]['motion']!r} has {frames} (set bars false)")
    elif kind == "recolor":
        check_fields(beat, {"type", "slot", "to", "part", "show", "swap", "labels", "motion"}, where, ["slot", "to", "swap"])
        if beat["slot"] not in ("scarf", "body", "feet", "cheek"):
            raise TemplateError(f"{where}.slot: recolour one of scarf, body, feet, cheek")
        out["slot"] = beat["slot"]
        out["to"] = check_color(beat["to"], f"{where}.to")
        out["part"] = check_label(beat.get("part", beat["slot"]), f"{where}.part", 34)
        out["show"] = check_cue(beat.get("show"), scene, f"{where}.show")
        out["swap"] = check_cue(beat["swap"], scene, f"{where}.swap")
        labels = beat.get("labels", ["BEFORE", "AFTER"])
        if not isinstance(labels, list) or len(labels) != 2:
            raise TemplateError(f"{where}.labels: two labels, before and after")
        out["labels"] = [check_label(t, f"{where}.labels[{i}]") for i, t in enumerate(labels)]
        out["motion"] = check_pattern_name(beat.get("motion", "hop"), patterns, f"{where}.motion")
    elif kind == "travel":
        check_fields(beat, {"type", "label", "hops"}, where)
        out["label"] = check_label(beat["label"], f"{where}.label") if beat.get("label") else None
        hops = beat.get("hops")
        if hops is not None and (type(hops) is not int or not 1 <= hops <= 6):
            raise TemplateError(f"{where}.hops: 1-6 hops")
        out["hops"] = hops
    elif kind == "end":
        check_fields(beat, {"type", "lines"}, where, ["lines"])
        lines = beat["lines"]
        if not isinstance(lines, list) or not 1 <= len(lines) <= 4 or any(not isinstance(t, str) or not t.strip() or len(t) > 80 for t in lines):
            raise TemplateError(f"{where}.lines: 1-4 lines of 1-80 characters")
        out["lines"] = [t.strip() for t in lines]
    return out


def check_color(value, where):
    if not isinstance(value, str) or not re.fullmatch(r"#[0-9a-fA-F]{6}", value):
        raise TemplateError(f"{where}: colours are #rrggbb")
    return value.lower()


def check_patterns(custom):
    patterns = {k: list(v) for k, v in PATTERNS.items()}
    for name, cycle in (custom or {}).items():
        where = f"patterns.{name}"
        if not re.fullmatch(r"[a-z][a-z0-9-]{0,31}", name) or name in PATTERNS:
            raise TemplateError(f"{where}: names are 1-32 lowercase letters, digits or -, and not a built-in pattern")
        if not isinstance(cycle, list) or not 1 <= len(cycle) <= 16:
            raise TemplateError(f"{where}: 1-16 [pose, frames] steps")
        steps = []
        for step in cycle:
            if not isinstance(step, list) or len(step) != 2 or step[0] not in POSES or type(step[1]) is not int or not 1 <= step[1] <= 250:
                raise TemplateError(f"{where}: each step is [pose, 1-250 frames at 25 fps]; poses are {sorted(POSES)}")
            steps.append((step[0], step[1]))
        patterns[name] = steps
    return patterns


# ---------------------------------------------------------------------------------------- art recipes
def art_plan(scenes, colors, patterns):
    """PixelForge recipes for this production, and where each picture the scenes use lives in them."""
    labels, cards, strips, bars, swatches, variants = [], [], [], [], [], {}

    def add(items, value):
        if value not in items:
            items.append(value)
        return items.index(value)

    title_lines = None
    for scene in scenes:
        beat = scene["beat"]
        kind = beat["type"]
        if kind == "title":
            title_lines = beat["lines"]
        for step in beat.get("steps") or []:
            if step["label"]:
                add(labels, step["label"])
            if step["strip"]:
                add(strips, tuple(step["strip"]))
        for card in beat.get("cards") or []:
            add(cards, (card["pose"], card["word"]))
        if beat.get("heading"):
            add(labels, beat["heading"]["label"])
        if kind == "strip":
            add(strips, tuple(beat["order"]))
            if beat["label"]:
                add(labels, beat["label"])
            if beat["then"]:
                add(strips, tuple(beat["then"]["order"]))
                if beat["then"]["label"]:
                    add(labels, beat["then"]["label"])
        if kind == "compare":
            for side in ("left", "right"):
                add(labels, beat[side]["label"])
                if beat["bars"]:
                    add(bars, beat[side]["motion"])
        if kind == "recolor":
            for text in beat["labels"]:
                add(labels, text)
            add(swatches, (beat["part"], colors[beat["slot"]], beat["to"]))
            variants[variant_name(beat["slot"], beat["to"])] = (beat["slot"], beat["to"])
        if kind == "travel" and beat["label"]:
            add(labels, beat["label"])
    recipes = {"pip": pip_recipe("pip", colors), "sky": sky_recipe(colors), "hills-far": hills_far_recipe(colors),
               "hills-near": hills_near_recipe(colors), "ground": ground_recipe(colors), "clouds": clouds_recipe(colors)}
    for name, (slot, color) in sorted(variants.items()):
        shade = shade_of(color)
        recipes[name] = pip_recipe(name, {**colors, slot: color, f"{slot}_shade": shade} if f"{slot}_shade" in colors else {**colors, slot: color})
    if labels:
        recipes["labels"] = {"version": 1, "name": "labels", "width": 128, "height": 16, "palette": palette(TEXT_KEYS, colors),
                             "frames": [label_frame(f"l{i}", text) for i, text in enumerate(labels)]}
    if title_lines:
        recipes["title"] = {"version": 1, "name": "title", "width": 160, "height": 14, "palette": palette(TEXT_KEYS, colors),
                            "frames": [{"name": f"line{i + 1}", "layers": [{"name": "t", "ops": [
                                {"op": "text", "x": 80, "y": 3, "text": text, "color": "w" if i == 0 else "y", "align": "center"},
                                {"op": "outline", "color": "k", "diagonal": True}]}]} for i, text in enumerate(title_lines)]}
    if cards:
        recipes["cards"] = {"version": 1, "name": "cards", "width": 56, "height": 72, "palette": palette(CARD_KEYS, colors),
                            "frames": [card_frame(f"c{i}", pose, word) for i, (pose, word) in enumerate(cards)]}
    if strips:
        recipes["strip"] = {"version": 1, "name": "strip", "width": 176, "height": 60, "palette": palette(CARD_KEYS, colors),
                            "frames": [strip_frame(f"s{i}", list(order)) for i, order in enumerate(strips)]}
        recipes["highlight"] = {"version": 1, "name": "highlight", "width": 46, "height": 64, "palette": palette(CARD_KEYS, colors),
                                "frames": [{"name": "hl", "ops": [
                                    {"op": "rect", "x": 1, "y": 0, "w": 44, "h": 64, "color": "y", "filled": False},
                                    {"op": "rect", "x": 0, "y": 1, "w": 46, "h": 62, "color": "y", "filled": False},
                                    {"op": "rect", "x": 2, "y": 1, "w": 42, "h": 62, "color": "y", "filled": False}]}]}
    if bars:
        width = max(4 + 2 * sum(n for _, n in patterns[name]) for name in bars)
        recipes["timing"] = {"version": 1, "name": "timing", "width": width, "height": 10, "palette": palette(BAR_KEYS, colors),
                             "frames": [bar_frame(f"b{i}", patterns[name]) for i, name in enumerate(bars)]}
    if swatches:
        # Each swatch frame draws its own two colours, so each gets its own palette entries.
        swatch_palette = palette(CARD_KEYS, colors)
        for i, (_, before, after) in enumerate(swatches):
            swatch_palette[f"from{i}"], swatch_palette[f"to{i}"] = before, after
        recipes["swatch"] = {"version": 1, "name": "swatch", "width": 100, "height": 28, "palette": swatch_palette,
                             "frames": [swatch_frame(f"w{i}", part, f"from{i}", f"to{i}") for i, (part, _, _) in enumerate(swatches)]}
    index = {"labels": {text: f"l{i}" for i, text in enumerate(labels)},
             "cards": {card: f"c{i}" for i, card in enumerate(cards)},
             "strips": {order: f"s{i}" for i, order in enumerate(strips)},
             "bars": {name: f"b{i}" for i, name in enumerate(bars)},
             "swatches": {item: f"w{i}" for i, item in enumerate(swatches)},
             "bar_width": recipes["timing"]["width"] if bars else 0}
    return recipes, index


def variant_name(slot, color):
    return f"pip-{slot}-{color.lstrip('#')}"


def shade_of(hex_color):
    r, g, b = rgb(hex_color)
    return "#%02x%02x%02x" % (r * 2 // 3, g * 2 // 3, b * 2 // 3)


def pip_recipe(name, colors):
    return {"version": 1, "name": name, "width": 32, "height": 48, "palette": palette(PIP_KEYS, colors), "anchor": PIP_ANCHOR,
            "frames": [{"name": p, "duration": 100, "layers": pip_layers(p)} for p in POSES],
            "animations": {"idle": {"frames": ["stand", "stand", "stand", "blink"]},
                           "hop": {"frames": ["crouch", "push", "float", "land", "stand"]}}}


def sky_recipe(colors):
    return {"version": 1, "name": "sky", "width": 32, "height": 120, "palette": palette(BG_KEYS, colors), "frames": [{"name": "sky", "ops": [
        {"op": "rect", "x": 0, "y": 0, "w": 32, "h": 120, "color": "a1"},
        {"op": "dither", "x": 0, "y": 20, "w": 32, "h": 24, "color": "a2", "density": [0, 1]},
        {"op": "rect", "x": 0, "y": 44, "w": 32, "h": 76, "color": "a2"},
        {"op": "dither", "x": 0, "y": 56, "w": 32, "h": 24, "color": "a3", "density": [0, 1]},
        {"op": "rect", "x": 0, "y": 80, "w": 32, "h": 40, "color": "a3"},
        {"op": "dither", "x": 0, "y": 88, "w": 32, "h": 20, "color": "a4", "density": [0, 1]},
        {"op": "rect", "x": 0, "y": 108, "w": 32, "h": 12, "color": "a4"}]}]}


def hills_far_recipe(colors):
    return {"version": 1, "name": "hills-far", "width": 160, "height": 60, "palette": palette(BG_KEYS, colors), "frames": [{"name": "hills", "wrap": True, "ops": [
        {"op": "ellipse", "x": -30, "y": 18, "w": 110, "h": 90, "color": "g1"}, {"op": "ellipse", "x": 130, "y": 18, "w": 110, "h": 90, "color": "g1"},
        {"op": "ellipse", "x": 60, "y": 28, "w": 80, "h": 70, "color": "g1"},
        {"op": "ellipse", "x": 118, "y": 8, "w": 90, "h": 100, "color": "g1"}, {"op": "ellipse", "x": -42, "y": 8, "w": 90, "h": 100, "color": "g1"},
        {"op": "dither", "x": 0, "y": 40, "w": 160, "h": 20, "color": "g2", "density": [0, 0.75], "over": "g1"}]}]}


def hills_near_recipe(colors):
    return {"version": 1, "name": "hills-near", "width": 160, "height": 50, "palette": palette(BG_KEYS, colors), "frames": [{"name": "hills", "wrap": True, "ops": [
        {"op": "ellipse", "x": -20, "y": 14, "w": 70, "h": 70, "color": "n1"}, {"op": "ellipse", "x": 140, "y": 14, "w": 70, "h": 70, "color": "n1"},
        {"op": "ellipse", "x": 40, "y": 24, "w": 60, "h": 60, "color": "n1"},
        {"op": "ellipse", "x": 92, "y": 10, "w": 84, "h": 80, "color": "n1"}, {"op": "ellipse", "x": -68, "y": 10, "w": 84, "h": 80, "color": "n1"},
        {"op": "outline", "color": "n3", "position": "inside", "directions": [".x.", "...", "..."]},
        {"op": "dither", "x": 0, "y": 30, "w": 160, "h": 20, "color": "n2", "density": [0, 0.8], "over": "n1"},
        {"op": "ellipse", "x": 30, "y": 30, "w": 9, "h": 7, "color": "n2"}, {"op": "ellipse", "x": 36, "y": 32, "w": 8, "h": 6, "color": "n2"},
        {"op": "ellipse", "x": 128, "y": 26, "w": 10, "h": 8, "color": "n2"}]}]}


def ground_recipe(colors):
    pal = palette(BG_KEYS, colors)
    pal.update({"1": pal["G1"], "G": pal["G3"], "3": pal["d3"], "p": pal["p1"], "d": pal["d2"]})
    return {"version": 1, "name": "ground", "width": 32, "height": 60, "palette": pal, "frames": [{"name": "ground", "ops": [
        {"op": "rect", "x": 0, "y": 4, "w": 32, "h": 8, "color": "G1"},
        {"op": "grid", "x": 0, "y": 0, "rows": [
            "......G..................G......",
            "..G...G1....G......G.....G....G.",
            "..G..1G1...1G1....1G1...1G1..1G1",
            "1G11111111111111111111111111G111"]},
        {"op": "rect", "x": 0, "y": 10, "w": 32, "h": 2, "color": "G2"},
        {"op": "rect", "x": 0, "y": 12, "w": 32, "h": 48, "color": "d1"},
        {"op": "dither", "x": 0, "y": 12, "w": 32, "h": 6, "color": "G2", "density": [0.6, 0]},
        {"op": "dither", "x": 0, "y": 36, "w": 32, "h": 24, "color": "d2", "density": [0, 1]},
        {"op": "grid", "x": 4, "y": 20, "rows": [".pp.", "pppd", ".dd."]},
        {"op": "grid", "x": 20, "y": 30, "rows": [".p", "pd"]},
        {"op": "grid", "x": 12, "y": 44, "rows": ["3d", "d3"]},
        {"op": "grid", "x": 26, "y": 50, "rows": [".33", "333"]}]}]}


def clouds_recipe(colors):
    edge = {"op": "outline", "color": "c2", "position": "inside", "width": 2, "directions": ["...", "...", ".x."]}
    return {"version": 1, "name": "clouds", "width": 64, "height": 24, "palette": palette(BG_KEYS, colors), "frames": [
        {"name": "cloud-a", "ops": [
            {"op": "ellipse", "x": 4, "y": 10, "w": 22, "h": 12, "color": "c1"}, {"op": "ellipse", "x": 16, "y": 3, "w": 24, "h": 18, "color": "c1"},
            {"op": "ellipse", "x": 34, "y": 9, "w": 24, "h": 13, "color": "c1"}, {"op": "rect", "x": 10, "y": 16, "w": 44, "h": 6, "color": "c1"}, edge]},
        {"name": "cloud-b", "ops": [
            {"op": "ellipse", "x": 8, "y": 9, "w": 20, "h": 11, "color": "c1"}, {"op": "ellipse", "x": 20, "y": 5, "w": 20, "h": 15, "color": "c1"},
            {"op": "rect", "x": 12, "y": 14, "w": 24, "h": 6, "color": "c1"}, edge]}]}


def label_frame(name, text):
    return {"name": name, "layers": [{"name": "t", "ops": [
        {"op": "text", "x": 64, "y": 4, "text": text, "color": "w", "align": "center"},
        {"op": "outline", "color": "k", "diagonal": True},
        {"op": "outline", "color": "k", "directions": ["...", "...", "..x"]}]}]}


def card_ops(w, h):
    return [{"op": "rect", "x": 1, "y": 0, "w": w - 2, "h": h, "color": "p"}, {"op": "rect", "x": 0, "y": 1, "w": w, "h": h - 2, "color": "p"},
            {"op": "outline", "color": "q", "position": "inside", "width": 2, "directions": ["...", "...", ".x."]},
            {"op": "outline", "color": "k"}]


def card_frame(name, pose, word):
    return {"name": name, "layers": [
        {"name": "panel", "x": 1, "y": 1, "ops": card_ops(54, 70)},
        {"name": "floor", "ops": [{"op": "line", "x": 7, "y": 54, "x2": 48, "y2": 54, "color": "q"}]},
        *pip_layers(pose, dx=12, dy=8),
        {"name": "word", "ops": [{"op": "text", "x": 28, "y": 59, "text": word, "color": "i", "align": "center"}]}]}


def strip_frame(name, order):
    layers = []
    for i, pose in enumerate(order):
        x = i * 44
        layers.append({"name": f"panel{i}", "x": x + 1, "y": 1, "ops": card_ops(40, 58)})
        layers += [dict(layer, name=f"{layer['name']}{i}") for layer in pip_layers(pose, dx=x + 5, dy=6)]
        # Each card shows its pose's place in a hop (crouch 1 ... land 4), so a shuffled strip reads as shuffled.
        if pose in HOP_ORDER:
            layers.append({"name": f"n{i}", "ops": [{"op": "text", "x": x + 5, "y": 4, "text": str(HOP_ORDER.index(pose) + 1), "color": "i"}]})
    return {"name": name, "layers": layers}


def bar_frame(name, pattern):
    ops, x = [], 2
    for pose, frames in pattern:
        ops.append({"op": "rect", "x": x, "y": 2, "w": 2 * frames, "h": 6, "color": BAR_COLOR[pose]})
        x += 2 * frames
    ops.append({"op": "outline", "color": "k"})
    return {"name": name, "ops": ops}


def swatch_frame(name, part, before, after):
    arrow = ["....k...", "....kk..", "kkkkkkk.", "kkkkkkkk", "kkkkkkk.", "....kk..", "....k..."]
    return {"name": name, "layers": [
        {"name": "panel", "x": 1, "y": 1, "ops": card_ops(98, 26)},
        {"name": "t", "ops": [{"op": "text", "x": 8, "y": 10, "text": part, "color": "i"},
                              {"op": "rect", "x": 46, "y": 8, "w": 12, "h": 12, "color": before},
                              {"op": "rect", "x": 46, "y": 8, "w": 12, "h": 12, "color": "k", "filled": False},
                              {"op": "grid", "x": 62, "y": 9, "rows": arrow},
                              {"op": "rect", "x": 76, "y": 8, "w": 12, "h": 12, "color": after},
                              {"op": "rect", "x": 76, "y": 8, "w": 12, "h": 12, "color": "k", "filled": False}]}]}


# ---------------------------------------------------------------------------------------- scenes
def rt(x):
    x = F(x)
    return {"num": x.numerator, "den": x.denominator}


def snap(x):
    return F(round(F(x) * FPS), FPS)


class Layer:
    """One scene layer on the logical 320x180 stage (or native, for text), active over [start, end) of the scene."""

    def __init__(self, id, canvas, start, end, frames=None, position=(0, 0), scale=1, opacity=255, curves=None,
                 tilemap=None, graphics=None, native=False):
        self.id, self.canvas, self.start, self.end = id, list(canvas), F(start), F(end)
        self.frames = frames or []          # [(path, hold, anchor)]
        self.position, self.scale, self.opacity = list(position), scale, opacity
        self.curves = curves or {}          # name -> [(t from layer start, value, interpolation)]
        self.tilemap, self.graphics, self.native = tilemap, graphics, native

    def to_json(self):
        length = self.end - self.start
        factor = 1 if self.native else FACTOR
        layer = {"id": self.id, "canvas": self.canvas, "start": rt(self.start), "duration": rt(length), "timing": "strict",
                 "transform": {"position": [v * factor for v in self.position], "crop": [0, 0, *self.canvas],
                               "scale": self.scale * factor, "quarter_turns": 0, "opacity": self.opacity},
                 "end": "hold_last"}
        if self.tilemap:
            layer["frames"] = []
            layer["tilemap"] = {"tile_size": self.tilemap["tile"], "cells": self.tilemap["cells"], "tiles": [{
                "frames": [{"image": {"path": self.tilemap["image"]}, "hold": rt(length), "offset": [0, 0], "anchor": [0, 0]}],
                "timing": "strict", "end": "hold_last"}]}
        elif self.graphics:
            layer["frames"] = []
            layer["graphics"] = self.graphics
        else:
            layer["frames"] = clip_frames(self.frames, length)
        if self.curves:
            layer["animation"] = {}
            for name, keys in self.curves.items():
                scale = factor if name in ("position_x", "position_y") else 1
                out = []
                for t, v, mode in sorted(keys, key=lambda k: k[0]):
                    if t > length:
                        continue
                    out.append({"time": rt(t), "value": int(v) * scale, "interpolation": mode})
                if out[-1]["time"] != rt(length):
                    out.append({"time": rt(length), "value": out[-1]["value"], "interpolation": "hold"})
                layer["animation"][name] = {"keys": out}
        return layer


def clip_frames(frames, length):
    """Frame list covering exactly `length`, the last hold shortened to fit."""
    out, t = [], F(0)
    for path, hold, anchor in frames:
        if t >= length:
            break
        hold = min(F(hold), length - t)
        out.append({"image": {"path": path}, "hold": rt(hold), "offset": [0, 0], "anchor": anchor})
        t += hold
    if t < length:
        raise TemplateError("internal: frame list shorter than its layer")
    return out


def cycle(pattern, total, frame_path, offset=0):
    """Repeat a (pose, frames) cycle to cover `total` seconds, starting `offset` frames into the cycle."""
    seq, t = [], F(0)
    steps = [(pose, F(n, FPS)) for pose, n in pattern]
    skip = F(offset, FPS) % sum(h for _, h in steps)
    while t < total:
        for pose, hold in steps:
            if skip >= hold:
                skip -= hold
                continue
            hold, skip = hold - skip, F(0)
            seq.append((frame_path(pose), hold, PIP_ANCHOR))
            t += hold
            if t >= total:
                break
    return seq


class SceneBuilder:
    """Builds one scene recipe. `art` maps recipe names to their rendered folders; `cue` resolves cue words."""

    def __init__(self, scene, duration, art, index, patterns, colors, cue, fonts, scene_number):
        self.scene, self.duration, self.art, self.index = scene, F(duration), art, index
        self.patterns, self.colors, self.cue, self.fonts = patterns, colors, cue, fonts
        self.number = scene_number

    def frame(self, recipe, frame):
        return f"{self.art[recipe]}/frames/{frame}.png"

    def pip_frame(self, bundle="pip"):
        return lambda pose: self.frame(bundle, pose)

    def at(self, cue):
        t = self.cue(cue)
        if not F(0) <= t < self.duration:
            raise TemplateError(f"scene {self.scene['id']!r}: cue {cue['word']!r} falls outside the scene")
        return t

    def background(self):
        length = self.duration
        layers = [
            Layer("sky", [320, 120], 0, length, tilemap={"tile": [32, 120], "cells": [[0] * 10], "image": self.frame("sky", "sky")}),
            Layer("hills-far", [320, 60], 0, length, position=(0, 58), tilemap={"tile": [160, 60], "cells": [[0, 0]], "image": self.frame("hills-far", "hills")}),
            Layer("hills-near", [320, 50], 0, length, position=(0, 80), tilemap={"tile": [160, 50], "cells": [[0, 0]], "image": self.frame("hills-near", "hills")}),
            Layer("ground", [320, 60], 0, length, position=(0, 120), tilemap={"tile": [32, 60], "cells": [[0] * 10], "image": self.frame("ground", "ground")}),
        ]
        # Clouds drift left 3 pixels a second from a starting place that depends only on the scene's position
        # in the film, so a retimed neighbour never changes this scene's recipe.
        speed = 3
        for i, (frame, y) in enumerate([("cloud-a", 12), ("cloud-b", 30), ("cloud-a", 22)]):
            x0 = (70 + 180 * i + 97 * self.number) % 380 - 30
            x1 = x0 - math.floor(speed * length)
            if x1 > 320 or x0 < -64:
                continue
            layers.append(Layer(f"cloud{i + 1}", [64, 24], 0, length, frames=[(self.frame("clouds", frame), length, [0, 0])],
                                position=(x0, y), curves={"position_x": [(F(0), x0, "linear"), (length, x1, "hold")]}))
        return layers

    def label(self, id, text, start, end, x=160, y=8, fade=F(4, 25)):
        curves = {"opacity": [(F(0), 0, "linear"), (min(fade, end - start), 255, "hold")]} if fade else None
        return Layer(id, [128, 16], start, end, frames=[(self.frame("labels", self.index["labels"][text]), end - start, [64, 0])],
                     position=(x, y), curves=curves)

    def pip(self, id, motion, start, end, x=160, bundle="pip", curves=None):
        return Layer(id, PIP_CANVAS, start, end, frames=cycle(self.patterns[motion], end - start, self.pip_frame(bundle)),
                     position=(x, GROUND_Y), curves=curves)

    def strip(self, id, order, start, end, y=6, fade=True):
        curves = {"opacity": [(F(0), 0, "linear"), (min(F(12, 25), end - start), 255, "hold")]} if fade else None
        return Layer(id, [176, 60], start, end, frames=[(self.frame("strip", self.index["strips"][tuple(order)]), end - start, [88, 0])],
                     position=(160, y), curves=curves)

    def build(self):
        beat = self.scene["beat"]
        layers = getattr(self, "beat_" + beat["type"])(beat)
        if len(layers) > 48:
            raise TemplateError(f"scene {self.scene['id']!r} has {len(layers)} layers; captions need room under the 64-layer limit")
        background = self.colors["outline"] if beat["type"] == "end" else self.colors["sky_1"]
        return {"schema_version": 1, "id": self.scene["id"], "width": SIZE[0], "height": SIZE[1], "output_scale": 1,
                "duration": rt(self.duration), "background": rgb(background), "color": "srgb_straight_encoded",
                "layers": [layer.to_json() for layer in layers], "audio": None}

    # ------------------------------------------------------------------ beats
    def beat_title(self, beat):
        d = self.duration
        beat_len = F(12, 25)
        half = snap(d / 2)
        layers = self.background() + [self.pip("pip-idle", "idle", 0, half), self.pip("pip-hop", "hop", half, d)]
        layers.append(Layer("title1", [160, 14], 0, d, frames=[(self.frame("title", "line1"), d, [80, 0])], position=(160, 22), scale=2,
                            curves={"opacity": [(F(0), 0, "linear"), (beat_len, 255, "hold")],
                                    "position_y": [(F(0), 12, "ease_out"), (beat_len, 22, "hold")]}))
        if len(beat["lines"]) > 1:
            layers.append(Layer("title2", [160, 14], beat_len, d, frames=[(self.frame("title", "line2"), d - beat_len, [80, 0])],
                                position=(160, 52), scale=2, curves={"opacity": [(F(0), 0, "linear"), (beat_len, 255, "hold")]}))
        return layers

    def beat_switch(self, beat):
        d = self.duration
        steps = beat["steps"]
        starts = [F(0)] + [self.at(step["cue"]) for step in steps[1:]]
        if any(b <= a for a, b in zip(starts, starts[1:])):
            raise TemplateError(f"scene {self.scene['id']!r}: switch cues must come in script order")
        layers = self.background()
        motion, motion_start = steps[0]["motion"], F(0)
        top = None   # (kind, value, start)
        tops = []
        for i, step in enumerate(steps):
            start = starts[i]
            if step["motion"] and i > 0 and step["motion"] != motion:
                layers.append(self.pip(f"pip-{len(layers)}", motion, motion_start, start))
                motion, motion_start = step["motion"], start
            new_top = ("label", step["label"]) if step["label"] else ("strip", tuple(step["strip"])) if step["strip"] else None
            if new_top:
                if top:
                    tops.append((*top, start))
                top = (*new_top, start)
        layers.append(self.pip(f"pip-{len(layers)}", motion, motion_start, d))
        if top:
            tops.append((*top, d))
        for n, (kind, value, start, end) in enumerate(tops):
            if kind == "label":
                layers.append(self.label(f"label-{n}", value, start, end))
            else:
                layers.append(self.strip(f"strip-{n}", value, start, end, y=8))
        return layers

    def beat_cards(self, beat):
        d = self.duration
        layers = self.background() + [self.pip("pip", beat["motion"], 0, d)]
        if beat["heading"]:
            start = self.at(beat["heading"]["cue"]) if beat["heading"]["cue"] else F(0)
            layers.append(self.label("heading", beat["heading"]["label"], start, d, y=6))
        n = len(beat["cards"])
        x0 = 160 - 36 * (n - 1)
        last = F(-1)
        for i, card in enumerate(beat["cards"]):
            at = self.at(card["cue"])
            if at <= last:
                raise TemplateError(f"scene {self.scene['id']!r}: card cues must come in script order")
            last = at
            frame = self.frame("cards", self.index["cards"][(card["pose"], card["word"])])
            layers.append(Layer(f"card-{i}", [56, 72], at, d, frames=[(frame, d - at, [28, 0])], position=(x0 + 72 * i, 26),
                                curves={"opacity": [(F(0), 0, "linear"), (F(4, 25), 255, "hold")],
                                        "position_y": [(F(0), 36, "ease_out"), (F(6, 25), 26, "hold")]}))
        return layers

    def strip_motion(self, order):
        return [(pose, HOLDS[pose]) for pose in order] + [("stand", HOLDS["stand"])]

    def highlight(self, id, order, start, end):
        pattern = self.strip_motion(order)
        xs, ops, t = [], [], F(0)
        length = end - start
        while t < length and len(xs) < 200:
            for slot, (pose, frames) in enumerate(pattern):
                if t >= length:
                    break
                xs.append((t, 70 + 44 * min(slot, 3), "hold"))
                ops.append((t, 255 if slot < 4 else 0, "hold"))
                t += F(frames, FPS)
        return Layer(id, [46, 64], start, end, frames=[(self.frame("highlight", "hl"), length, [0, 0])], position=(70, 4),
                     curves={"position_x": xs, "opacity": ops})

    def beat_strip(self, beat):
        d = self.duration
        switch = self.at(beat["then"]["cue"]) if beat["then"] else d
        layers = self.background() + [self.strip("strip-a", beat["order"], 0, switch, fade=False),
                                      self.highlight("hl-a", beat["order"], 0, switch)]
        pattern_a = self.strip_motion(beat["order"])
        layers.append(Layer("pip-a", PIP_CANVAS, 0, switch, frames=cycle(pattern_a, switch, self.pip_frame()), position=(160, GROUND_Y)))
        if beat["label"]:
            layers.append(self.label("label-a", beat["label"], 0, switch, y=68))
        if beat["then"]:
            then = beat["then"]
            layers += [self.strip("strip-b", then["order"], switch, d, fade=False), self.highlight("hl-b", then["order"], switch, d)]
            pattern_b = self.strip_motion(then["order"])
            layers.append(Layer("pip-b", PIP_CANVAS, switch, d, frames=cycle(pattern_b, d - switch, self.pip_frame()), position=(160, GROUND_Y)))
            if then["label"]:
                layers.append(self.label("label-b", then["label"], switch, d, y=68))
        return layers

    def beat_compare(self, beat):
        d = self.duration
        layers = self.background()
        for side, x in (("left", 96), ("right", 224)):
            item = beat[side]
            layers.append(self.label(f"label-{side}", item["label"], 0, d, x=x, y=6, fade=None))
            if beat["bars"]:
                # Every bar shares the recipe's canvas; each is drawn from its left edge, so centre on its own width.
                width = 4 + 2 * sum(n for _, n in self.patterns[item["motion"]])
                layers.append(Layer(f"bar-{side}", [self.index["bar_width"], 10], 0, d,
                                    frames=[(self.frame("timing", self.index["bars"][item["motion"]]), d, [width // 2, 0])], position=(x, 26)))
            layers.append(self.pip(f"pip-{side}", item["motion"], 0, d, x=x))
        return layers

    def beat_recolor(self, beat):
        d = self.duration
        swap = self.at(beat["swap"])
        show = self.at(beat["show"]) if beat["show"] else F(0)
        variant = variant_name(beat["slot"], beat["to"])
        frame = self.frame("swatch", self.index["swatches"][(beat["part"], self.colors[beat["slot"]], beat["to"])])
        before, after = beat["labels"]
        return self.background() + [
            self.label("label-before", before, 0, swap), self.label("label-after", after, swap, d),
            Layer("swatch", [100, 28], show, d, frames=[(frame, d - show, [50, 0])], position=(160, 30),
                  curves={"opacity": [(F(0), 0, "linear"), (F(4, 25), 255, "hold")]}),
            self.pip("pip-before", beat["motion"], 0, swap),
            # The new colour continues the same motion where the old one stopped.
            Layer("pip-after", PIP_CANVAS, swap, d, position=(160, GROUND_Y),
                  frames=cycle(self.patterns[beat["motion"]], d - swap, self.pip_frame(variant), offset=round(swap * FPS))),
        ]

    def beat_travel(self, beat):
        d = self.duration
        cycle_len = F(8, 5)
        hops = beat["hops"] or max(1, min(6, int((d - F(1)) // cycle_len)))
        hops = min(hops, max(1, int(d // cycle_len)))
        xs = [(F(0), 52, "hold")]
        for k in range(hops):
            c = cycle_len * k
            xs += [(c + F(4, 25), 52 + 36 * k, "linear"), (c + F(19, 25), 52 + 36 * (k + 1), "hold")]
        rest = cycle_len * hops
        frames = cycle(PATTERNS["hop"], rest, self.pip_frame()) + cycle(self.patterns["idle"], d - rest, self.pip_frame()) if rest < d \
            else cycle(PATTERNS["hop"], d, self.pip_frame())
        layers = self.background()
        if beat["label"]:
            layers.append(self.label("label", beat["label"], 0, d, y=6))
        layers.append(Layer("pip-travel", PIP_CANVAS, 0, d, frames=frames, position=(52, GROUND_Y), curves={"position_x": xs}))
        return layers

    def beat_end(self, beat):
        d = self.duration
        ground = Layer("ground", [320, 60], 0, d, position=(0, 720), scale=6, native=True,
                       tilemap={"tile": [32, 60], "cells": [[0] * 10], "image": self.frame("ground", "ground")})
        idle = min(F(48, 25), d)
        pip = Layer("pip", PIP_CANVAS, 0, d, position=(330, 744), scale=6, native=True,
                    frames=cycle(self.patterns["idle"], idle, self.pip_frame()) + (cycle(PATTERNS["hop"], d - idle, self.pip_frame()) if d > idle else []))
        layers = [ground, pip]
        styles = [(84, "label", "bold", 170), (46, "label_accent", "bold", 330), (46, "label_accent", "bold", 410), (36, "body_light", "regular", 520)]
        for i, text in enumerate(beat["lines"]):
            size, color, font, y = styles[i]
            start = F([6, 24, 36, 60][i], 25)
            if start >= d:
                break
            height = int(size * 1.5)
            graphic = {"kind": "text", "text": text, "fonts": [{"path": self.fonts[font]}], "size": size, "color": rgb(self.colors[color]) + [255],
                       "rect": [0, 0, 1280, height], "line_height": int(size * 1.3), "letter_spacing": 0, "align": "left",
                       "wrap": "none", "overflow": "reject"}
            layers.append(Layer(f"line{i + 1}", [1280, height], start, d, graphics=graphic, position=(620, y), native=True,
                                curves={"opacity": [(F(0), 0, "linear"), (F(8, 25), 255, "hold")],
                                        "position_x": [(F(0), 660, "ease_out"), (F(8, 25), 620, "hold")]}))
        return layers


def caption_layouts(fonts, colors):
    """Burned-in caption style: bold text in a dark box near the bottom of the frame (the demo's layout)."""
    return {"default": {"fonts": [{"path": fonts["bold"]}], "size": 50, "rect": [160, 870, 1600, 170], "line_height": 66,
                        "letter_spacing": 0, "wrap": "none", "overflow": "reject", "valign": "bottom",
                        "background": {"color": rgb(colors["caption_box"]) + [210], "padding": 14}}}
