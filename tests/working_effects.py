"""Independent Fraction clocks and f64 references for precise effect/opacity components.

Exercises the real Rust library. Project, scene, spatial and nested integration remain
separate acceptance work; these component checks do not close stage six.
"""
import argparse
import colorsys
import copy
from fractions import Fraction as F
import hashlib
import itertools
import json
import math
import os
from pathlib import Path
import random
import subprocess

from engine import BUILD
from luts_scopes import reference as lut_reference
from scenes import identity, time
from working_color import Q, decode, encode, quantize

ROOT = Path(__file__).resolve().parents[1]


def exact_sample(curve, when):
    keys = sorted(curve["keys"], key=lambda k: F(k["time"]["num"], k["time"]["den"]))
    times = [F(k["time"]["num"], k["time"]["den"]) for k in keys]
    if "retime" in curve:
        r = curve["retime"]
        progress = max(F(0), when - F(r["start"]["num"], r["start"]["den"])) * F(r["rate"]["num"], r["rate"]["den"])
        when = times[-1] - progress if r.get("reverse", False) else times[0] + progress
    if when <= times[0]:
        return F(keys[0]["value"])
    if when >= times[-1]:
        return F(keys[-1]["value"])
    left = next(i for i in range(len(times) - 1) if times[i] <= when < times[i + 1])
    a, b = keys[left:left + 2]
    if a["interpolation"] == "hold":
        return F(a["value"])
    w = (when - times[left]) / (times[left + 1] - times[left])
    mode = a["interpolation"]
    if mode == "ease_in":
        w *= w
    elif mode == "ease_out":
        w = 1 - (1 - w) ** 2
    elif mode == "ease_in_out":
        w = 2 * w ** 2 if w < F(1, 2) else 1 - 2 * (1 - w) ** 2
    return F(a["value"]) * (1 - w) + F(b["value"]) * w


def nearest(value):
    return int(abs(value) + F(1, 2)) * (-1 if value < 0 else 1)


def curve(values, mode="linear"):
    return {"keys": [{"time": time(n, d), "value": v, "interpolation": mode} for n, d, v in values]}


def mask_weight(mask, when, position):
    if mask is None:
        return 1.0
    rect = list(mask["rect"])
    for key, animation in mask.get("animation", {}).items():
        rect[("x", "y", "width", "height").index(key)] = nearest(exact_sample(animation, when))
    x, y = [v + 0.5 for v in position]
    left, top, width, height = rect
    inside = max(0.0, min(x - left, left + width - x, y - top, top + height - y)) if width > 0 and height > 0 else 0.0
    weight = min(1.0, inside / mask["feather"]) if mask["feather"] else float(inside > 0)
    return 1 - weight if mask.get("inverted", False) else weight


def band_weight(value, band):
    distance = max(0.0, band["low"] - value, value - band["high"])
    if distance == 0:
        return 1.0
    return max(0.0, 1 - distance / band["feather"]) if band["feather"] else 0.0


def qualified(rgb, qualifier):
    if qualifier is None:
        return 1.0
    h, light, saturation = colorsys.rgb_to_hls(*rgb)
    weight = 1.0
    if "hue" in qualifier:
        band = qualifier["hue"]
        if max(rgb) == min(rgb):
            weight = 0.0
        else:
            distance = abs((h * 360000 - band["center"] + 180000) % 360000 - 180000)
            weight = 1.0 if distance <= band["inner"] else 0.0 if distance >= band["outer"] else (
                (band["outer"] - distance) / (band["outer"] - band["inner"]))
    for key, value in (("saturation", saturation * 1000), ("lightness", light * 1000)):
        if key in qualifier:
            weight *= band_weight(value, qualifier[key])
    return 1 - weight if qualifier.get("inverted", False) else weight


def tone(value, knots):
    if knots is None:
        return value
    x = min(1.0, max(0.0, value)) * Q
    points = knots["points"]
    left = next((i for i in range(len(points) - 1) if x <= points[i + 1][0]), len(points) - 2)
    (a, av), (b, bv) = points[left:left + 2]
    w = (x - a) / (b - a)
    return (av * (1 - w) + bv * w) / Q


def primary(rgb, specification, when):
    g = copy.deepcopy(specification)
    for key, animation in g.pop("animation", {}).items():
        v = float(exact_sample(animation, when))
        if key.endswith("_balance_milli"):
            g["white_balance_milli"][("red_balance_milli", "green_balance_milli", "blue_balance_milli").index(key)] = v
        else:
            g[key] = v
    result = []
    for c, value in enumerate(rgb):
        v = min(1.0, max(0.0, (value * 2 ** (g["exposure_milli"] / 1000) * g["white_balance_milli"][c] / 1000 - .18)
                         * g["contrast_milli"] / 1000 + .18))
        result.append(tone(tone(v, g.get("master_curve")), g.get(("red_curve", "green_curve", "blue_curve")[c])))
    hue = g.get("hue_shift_mdeg", 0) % 360000
    saturation = g.get("saturation_milli", 1000)
    if hue or saturation != 1000:
        h, light, s = colorsys.rgb_to_hls(*(encode(v, "srgb") for v in result))
        result = [decode(v, "srgb") for v in colorsys.hls_to_rgb((h + hue / 360000) % 1, light, min(1.0, s * saturation / 1000))]
    return result


def effect_pixel(pixel, position, effects, when, tables):
    if pixel[3] == 0:
        return pixel
    rgb, alpha = [v / Q for v in pixel[:3]], pixel[3] / Q
    for effect in effects:
        kind = effect["kind"]
        if kind == "lut":
            spec = tables[effect["file"]["path"]]
            values = lut_reference([encode(v, "srgb") for v in rgb], *spec[:2], effect["interpolation"], *spec[2:])
            rgb = [decode(min(1.0, max(0.0, float(v))), "srgb") for v in values]
        elif kind == "chroma_key":
            strength = float(exact_sample(effect["strength_curve"], when)) if "strength_curve" in effect else effect["strength_milli"]
            weight = strength / 1000 * mask_weight(effect.get("mask"), when, position)
            if weight == 0:
                continue
            encoded = [encode(v, "srgb") for v in rgb]
            delta = [v - key / 255 for v, key in zip(encoded, effect["key_rgb"])]
            distance = (max(delta) - min(delta)) / 2
            low, high = effect["inner_milli"] / 1000, effect["outer_milli"] / 1000
            matte = 0.0 if distance <= low else 1.0 if distance >= high else (distance - low) / (high - low)
            candidate = encoded.copy()
            unmix = effect.get("unmix_milli", 0) / 1000
            if matte < 1 and unmix:
                for c in range(3):
                    recovered = min(1.0, max(0.0, (encoded[c] - (1 - matte) * effect["key_rgb"][c] / 255) / matte)) if matte else 0.0
                    candidate[c] = (1 - unmix) * encoded[c] + unmix * recovered
            spill = effect.get("spill")
            if spill:
                c = ("red", "green", "blue").index(spill["channel"])
                other = max(candidate[i] for i in range(3) if i != c)
                candidate[c] -= max(0.0, candidate[c] - other) * spill["strength_milli"] / 1000
            alpha *= 1 - weight * (1 - matte)
            if alpha == 0:
                return [0] * 4
            rgb = [v * (1 - weight) + decode(k, "srgb") * weight for v, k in zip(rgb, candidate)]
        else:
            g = effect if kind == "grade" else effect["grade"]
            weight = 1.0
            if kind == "selective_grade":
                mix = float(exact_sample(effect["mix_curve"], when)) if "mix_curve" in effect else effect["mix_milli"]
                weight = mix / 1000 * qualified([encode(v, "srgb") for v in rgb], effect.get("qualifier"))
                weight *= mask_weight(effect.get("mask"), when, position)
            result = primary(rgb, g, when)
            rgb = [a * (1 - weight) + b * weight for a, b in zip(rgb, result)]
    return [quantize(v) for v in [*rgb, alpha]]


def run(root):
    root = root.resolve()
    assert root != ROOT and ROOT not in root.parents
    root.mkdir(parents=True, exist_ok=True)
    sources = root / "sources"
    sources.mkdir(exist_ok=True)
    subprocess.run(["cargo", "build", "--locked", "--example", "working_effects_fixture"], cwd=ROOT, check=True)
    helper = BUILD / "examples" / ("working_effects_fixture.exe" if os.name == "nt" else "working_effects_fixture")
    digest = hashlib.sha256()
    numerical_digest = hashlib.sha256()
    checks, cases, tables = [], [], {}

    def call(request, error=None):
        result = subprocess.run([str(helper), str(sources)], input=json.dumps(request), text=True, encoding="utf-8", capture_output=True, timeout=120)
        response = json.loads(result.stdout)
        if error:
            assert result.returncode == 1 and response["error"]["code"] == error, response
            return response["error"]
        assert result.returncode == 0 and response["ok"], response
        digest.update(result.stdout.encode("utf-8"))
        numerical = {k:v for k,v in response["result"].items() if k != "identities"}
        numerical_digest.update(json.dumps(numerical,sort_keys=True,separators=(",", ":")).encode("utf-8"))
        return response["result"]

    rng = random.Random(634718)
    clocks = [F(0), F(1, 30000), F(1, 2), F(1), F(2997, 1000), F(3), F(7, 2), F(4), F(10)]
    clocks += [F(rng.randrange(5000), 997) for _ in range(100)]
    maximum_curve_error, sampled_count = 0.0, 0
    for mode, reverse in itertools.product(("hold", "linear", "ease_in", "ease_out", "ease_in_out"), (False, True)):
        value = curve([(3, 1, 0), (0, 1, 255), (1, 1, 17), (4, 1, 127)], mode)
        value["retime"] = dict(start=time(1, 3), rate=time(7, 5), reverse=reverse)
        request = dict(kind="curve", curve=value, duration=time(12), bounds=[0, 255], scale=257,
                       times=[time(v.numerator, v.denominator) for v in clocks])
        for clock, sample in zip(clocks, call(request)["samples"]):
            expected = exact_sample(value, clock)
            assert sample["integer"] == nearest(expected)
            assert sample["scaled"] == nearest(expected * 257)
            maximum_curve_error = max(maximum_curve_error, abs(sample["continuous"] - float(expected)))
            assert math.isclose(sample["continuous"], float(expected), rel_tol=1e-14, abs_tol=1e-12)
            sampled_count += 1
    negative = curve([(0, 1, -255), (2, 1, 255)])
    request = dict(kind="curve", curve=negative, duration=time(2), bounds=[-255, 255], scale=65535,
                   times=[time(i, 510) for i in range(1021)])
    for i, sample in enumerate(call(request)["samples"]):
        expected = exact_sample(negative, F(i, 510))
        assert sample["integer"] == nearest(expected) and sample["scaled"] == nearest(expected * 65535)
        sampled_count += 1
    fade = curve([(0, 1, 0), (12, 1, 255)])
    request = dict(kind="curve", curve=fade, duration=time(12), bounds=[0, 255], scale=257,
                   times=[time(12 * i, 4095) for i in range(4096)])
    fade_samples = call(request)["samples"]
    assert len({s["integer"] for s in fade_samples}) == 256
    assert len({s["scaled"] for s in fade_samples}) == 4096
    assert [s["scaled"] for s in fade_samples] == [nearest(F(Q * i, 4095)) for i in range(4096)]
    sampled_count += len(fade_samples)
    checks.append("working_effects.exact_clocks_continuous_controls_and_scaled_opacity")

    def grade(**changes):
        return {"kind":"grade", "exposure_milli":0, "contrast_milli":1000, "white_balance_milli":[1000]*3, **changes}

    neutral = grade()
    graded = {**neutral, "exposure_milli": 347, "contrast_milli": 1173, "white_balance_milli": [920, 1115, 1070],
              "master_curve": {"points": [[0, 0], [15000, 19000], [41000, 37000], [65535, 65535]]},
              "red_curve": {"points": [[0, 0], [20000, 11000], [65535, 65535]]}}
    animated = {**graded, "animation": {
        "exposure_milli": curve([(0, 1, -100), (3, 1, 401)], "ease_in_out"),
        "contrast_milli": curve([(0, 1, 987), (3, 1, 1203)], "ease_out"),
        "red_balance_milli": curve([(0, 1, 890), (3, 1, 1113)]),
        "hue_shift_mdeg": curve([(0, 1, -1777), (3, 1, 23711)]),
        "saturation_milli": curve([(0, 1, 799), (3, 1, 1253)])}}
    mask = dict(rect=[2, 1, 27, 19], feather=5, inverted=False,
                animation={"x": curve([(0, 1, -3), (3, 1, 7)], "ease_in")})
    selective = dict(kind="selective_grade", grade={k:v for k,v in animated.items() if k!="kind"},
                     mix_milli=701, mix_curve=curve([(0,1,302),(3,1,907)], "ease_out"), mask=mask,
                     qualifier=dict(hue=dict(center=357777, inner=31234, outer=178123),
                                    saturation=dict(low=110, high=901, feather=213),
                                    lightness=dict(low=117, high=713, feather=227), inverted=False))
    key = dict(kind="chroma_key", key_rgb=[13, 237, 41], inner_milli=61, outer_milli=407,
               strength_milli=913, unmix_milli=639, spill=dict(channel="green", strength_milli=723),
               strength_curve=curve([(0, 1, 701), (3, 1, 979)], "ease_in_out"), mask=mask)
    chains = [("empty", []), ("identity", [neutral]*8), ("primary_and_curves", [graded]),
              ("animated_hue_saturation", [animated]), ("selective", [selective]),
              ("inverted_selection", [{**selective, "qualifier": {**selective["qualifier"], "inverted": True}}]),
              ("key", [key]), ("full_hard_key", [{**key, "key_rgb": [0,255,0], "inner_milli":100, "outer_milli":100,
                                                   "strength_milli":1000, "strength_curve":curve([(0,1,1000)])}]),
              ("ordered_grade_key_selective", [animated,key,selective]),
              ("eight_effect_chain", [graded,key,selective,neutral,graded,key,selective,animated])]

    for kind, size in [("curve1", 3), ("identity3", 5), ("cross3", 5), ("clip3", 5)]:
        dimension = 1 if kind.endswith("1") else 3
        low, high = (F(-1,4), F(1,4), F(0)), (F(3,4), F(1), F(1,2))
        path = sources / f"{kind}.cube"
        values = ([lut_reference([F(i,size-1)]*3,kind,size,"linear") for i in range(size)] if dimension==1 else
                  [lut_reference([F(r,size-1),F(g,size-1),F(b,size-1)],kind,size,"trilinear")
                   for b,g,r in itertools.product(range(size),repeat=3)])
        text = f"LUT_{dimension}D_SIZE {size}\nDOMAIN_MIN " + " ".join(str(float(v)) for v in low)
        text += "\nDOMAIN_MAX " + " ".join(str(float(v)) for v in high) + "\n"
        path.write_text(text + "".join(" ".join(str(float(v)) for v in row) + "\n" for row in values),encoding="ascii",newline="\n")
        tables[path.name] = (kind,size,low,high)
        for mode in (("nearest","linear") if dimension==1 else ("nearest","trilinear","tetrahedral")):
            lut = dict(kind="lut", file=identity(path,sources), interpolation=mode)
            chains.append((f"{kind}_{mode}", [lut]))
            if kind == "cross3" and mode == "tetrahedral":
                chains += [("grade_lut_key_selective", [animated,lut,key,selective]),
                           ("lut_repeated", [lut,lut]), ("lut_before_grade", [lut,animated])]
    originals={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    samples=[]
    for _ in range(192):
        samples.append(dict(rgba=[rng.randrange(Q+1) for _ in range(4)], position=[rng.randrange(-4,38),rng.randrange(-4,27)]))
    samples += [dict(rgba=[v,v,v,Q],position=[11,9]) for v in [0,1,2,63,254,255,256,257,32767,32768,65534,65535]]
    samples += [dict(rgba=[quantize(decode(v/255,"srgb")) for v in rgb]+[alpha],position=[13,9])
                for rgb in [(0,255,0),(13,237,41),(0,0,255),(255,0,0),(127,128,129),(128,128,128)]
                for alpha in [0,1,127,32768,Q]]
    times=[F(0), F(1,30000), F(1,17), F(733,997), F(3,2), F(29999,10000), F(3)]
    maximum_error=0
    for name, effects in chains:
        frames=[dict(time=time(t.numerator,t.denominator),pixels=samples) for t in times]
        result=call(dict(kind="effects",effects=effects,duration=time(4),frames=frames))
        max_case=0
        for when, pixels in zip(times,result["frames"]):
            for source, actual in zip(samples,pixels):
                expected=effect_pixel(source["rgba"],source["position"],effects,when,tables)
                difference=max(abs(a-b) for a,b in zip(actual,expected))
                max_case=max(max_case,difference)
                assert difference<=1,(name,str(when),source,actual,expected)
                if name in ("empty","identity"):
                    assert actual==source["rgba"]
        maximum_error=max(maximum_error,max_case)
        cases.append(dict(name=name,pixels=len(samples)*len(times),maximum_unorm16_error=max_case))
    checks += ["working_effects.primary_curves_and_hue_saturation", "working_effects.continuous_selective_correction",
               "working_effects.continuous_key_unmix_spill", "working_effects.lut_modes_domains_clipping_and_order",
               "working_effects.transparent_and_identity_preservation"]
    soft_key=dict(kind="chroma_key",key_rgb=[0,255,0],inner_milli=0,outer_milli=1000,strength_milli=1000)
    key_ramp=[dict(rgba=[0,quantize(decode(i/4095,"srgb")),0,Q],position=[0,0]) for i in range(4096)]
    keyed=call(dict(kind="effects",effects=[soft_key],duration=time(1),frames=[dict(time=time(0),pixels=key_ramp)]))["frames"][0]
    assert len({p[3] for p in keyed})==4096
    assert all(a[3]>b[3] for a,b in zip(keyed,keyed[1:]))
    key_maximum_error=0
    for source,actual in zip(key_ramp,keyed):
        expected=effect_pixel(source["rgba"],[0,0],[soft_key],F(0),tables)
        difference=max(abs(a-b) for a,b in zip(actual,expected))
        key_maximum_error=max(key_maximum_error,difference)
        assert difference<=1
    maximum_error=max(maximum_error,key_maximum_error)
    checks.append("working_effects.sub_byte_key_coverage")
    request=dict(kind="effects",effects=[neutral]*9,duration=time(4),frames=[])
    call(request,"INVALID_EFFECT")
    request["effects"]=[key]; request["duration"]=time(0); call(request,"INVALID_EFFECT")
    request["duration"]=time(4); request["effects"]=[{**key,"key_rgb":[22,22,22]}]; call(request,"INVALID_EFFECT")
    wrong=copy.deepcopy(next(e for _,chain in chains for e in chain if e["kind"]=="lut"))
    wrong["file"]["sha256"]="0"*64; request["effects"]=[wrong]; call(request,"MEDIA_CHANGED")
    bad_curve=dict(kind="curve",curve=fade,duration=time(12),bounds=[0,255],scale=0,times=[time(1)])
    call(bad_curve,"INVALID_ANIMATION")
    assert originals=={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sources.iterdir()}
    checks.append("working_effects.validation_and_source_identity")
    report=dict(passed=checks,curve_samples=sampled_count,maximum_curve_float_error=maximum_curve_error,
                opacity_levels=dict(byte=256,precise=4096),key_alpha_levels=4096,cases=cases,pixel_samples=sum(c["pixels"] for c in cases)+len(key_ramp),
                maximum_unorm16_error=maximum_error,key_maximum_unorm16_error=key_maximum_error,
                response_sha256=digest.hexdigest(),numerical_response_sha256=numerical_digest.hexdigest(),
                numerical_digest_scope="Canonical successful result objects excluding file identities",source_sha256=originals,rejected_cases=5,
                reference="Fraction clocks and scaled coverage; independent f64 grades/masks/keys, Python colorsys HSL, closed-form LUT products",
                tolerance="Exact legacy/scaled curve integers; continuous controls within 1e-12 absolute/1e-14 relative; effect output at most one UNORM16 code",
                limits="Library effects and opacity component only; no timeline/scene setting, spatial/nested integration, rendered grade/fade banding or stage-six exit")
    (root/"verification.json").write_text(json.dumps(report,indent=2)+"\n",encoding="utf-8")
    print(json.dumps(report))


if __name__=="__main__":
    parser=argparse.ArgumentParser(); parser.add_argument("--output",type=Path,required=True)
    run(parser.parse_args().output)
