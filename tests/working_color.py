"""Independent f64 equations for the in-development linear UNORM16 library primitives.

This is a arithmetic foundation fixture, not acceptance of project working_space,
timeline effects, generated scenes, source precision, or the grade/fade banding gate.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import subprocess

from engine import BUILD

ROOT = Path(__file__).resolve().parents[1]
Q = 65535


def quantize(x, scale=Q):
    return math.floor(max(0.0, min(1.0, x)) * scale + 0.5)


def decode(v, transfer):
    if transfer == "srgb":
        return v / 12.92 if v <= 0.04045 else ((v + 0.055) / 1.055) ** 2.4
    return v / 4.5 if v < 0.081 else ((v + 0.099) / 1.099) ** (1 / 0.45)


def encode(v, transfer):
    if transfer == "srgb":
        return 12.92 * v if v <= 0.0031308 else 1.055 * v ** (1 / 2.4) - 0.055
    return 4.5 * v if v < 0.018 else 1.099 * v ** 0.45 - 0.099


def over(case):
    src = [v / Q for v in case["source"]]
    dst = [v / Q for v in case["backdrop"]]
    a = src[3] * (case["opacity"] / Q) * (case["mask"] / 65536)
    if a == 0:
        return case["backdrop"]
    out_alpha = a + dst[3] * (1 - a)
    result = []
    for s, d in zip(src[:3], dst[:3]):
        blend = {"normal": s, "multiply": s * d, "screen": 1 - (1 - s) * (1 - d)}[case["mode"]]
        # Straight source-over with a blending function: first the effective source color,
        # then its associated contribution and the surviving backdrop, then unassociate.
        effective = (1 - dst[3]) * s + dst[3] * blend
        result.append(quantize((a * effective + dst[3] * (1 - a) * d) / out_alpha))
    return result + [quantize(out_alpha)]


def mix(case):
    a, b, w = case["first"], case["second"], case["weight"] / Q
    if w == 0:
        return a
    if w == 1:
        return b
    alpha = (a[3] / Q) * (1 - w) + (b[3] / Q) * w
    if alpha == 0:
        return [0] * 4
    return [quantize(((a[c] / Q) * (a[3] / Q) * (1 - w) + (b[c] / Q) * (b[3] / Q) * w) / alpha)
            for c in range(3)] + [quantize(alpha)]


def bayer(size):
    # Recursive construction, independent of the Rust literal table.
    if size == 1:
        return [[0]]
    previous = bayer(size // 2)
    quadrant = [[0, 2], [3, 1]]
    return [[4 * previous[y % (size // 2)][x % (size // 2)]
             + quadrant[y // (size // 2)][x // (size // 2)]
             for x in range(size)] for y in range(size)]


def reference(case, thresholds):
    op = case["op"]
    if op == "over":
        return over(case)
    if op == "mix":
        return mix(case)
    rgba = case["rgba"]
    if op == "store":
        return [quantize(v) for v in rgba]
    if op == "decode":
        denominator = rgba[3] if case["alpha"] == "premultiplied" else 255
        if denominator == 0:
            return [0] * 4
        return [quantize(decode(v / denominator, case["transfer"])) for v in rgba[:3]] + [rgba[3] * 257]
    x, y = case["position"]
    threshold = (thresholds[y % 8][x % 8] + 0.5) / 64 if case["ordered"] else 0.5
    return [max(0, min(255, math.floor(encode(v / Q, case["transfer"]) * 255 + threshold)))
            for v in rgba[:3]] + [quantize(rgba[3] / Q, 255)]


def run(root):
    root.mkdir(parents=True, exist_ok=True)
    subprocess.run(["cargo", "build", "--locked", "--example", "working_color_fixture"], cwd=ROOT, check=True)
    helper = BUILD / "examples" / ("working_color_fixture.exe" if os.name == "nt" else "working_color_fixture")
    rng = random.Random(631046)
    edge = [0, 1, 2, 127, 128, 255, 256, 257, 32767, 32768, 65534, 65535]
    cases = []
    for transfer in ("srgb", "bt709"):
        for v in range(256):
            cases.append(dict(op="decode", rgba=[v, 255 - v, v // 2, v], transfer=transfer, alpha="straight"))
        # Every possible stored premultiplied channel/alpha pair, spread across channels.
        for alpha in range(256):
            for value in range(alpha + 1):
                cases.append(dict(op="decode", rgba=[value, alpha - value, value // 2, alpha],
                                  transfer=transfer, alpha="premultiplied"))
    for _ in range(4000):
        cases.append(dict(op="store", rgba=[rng.uniform(-0.1, 1.1) for _ in range(4)]))
    pixel = lambda: [rng.choice(edge) if rng.random() < 0.5 else rng.randrange(Q + 1) for _ in range(4)]
    for mode in ("normal", "multiply", "screen"):
        for _ in range(6000):
            cases.append(dict(op="over", source=pixel(), backdrop=pixel(), opacity=rng.choice(edge),
                              mask=rng.choice([0, 1, 32768, 65535, 65536, rng.randrange(65537)]), mode=mode))
    for _ in range(6000):
        cases.append(dict(op="mix", first=pixel(), second=pixel(), weight=rng.choice(edge)))
    for transfer in ("srgb", "bt709"):
        for _ in range(8000):
            cases.append(dict(op="deliver", rgba=pixel(), transfer=transfer,
                              position=[rng.randrange(100000), rng.randrange(100000)], ordered=bool(rng.randrange(2))))
    maximum = {}
    count = {}
    digest = hashlib.sha256()
    thresholds = bayer(8)

    def call(batch):
        request = json.dumps(batch, separators=(",", ":"))
        result = subprocess.run([str(helper)], input=request, text=True, encoding="utf-8", capture_output=True, check=True)
        values = json.loads(result.stdout)
        assert len(values) == len(batch)
        digest.update(result.stdout.encode("utf-8"))
        return values

    for start in range(0, len(cases), 16000):
        batch = cases[start:start + 16000]
        for case, actual in zip(batch, call(batch)):
            expected = reference(case, thresholds)
            difference = max(abs(a - b) for a, b in zip(actual, expected))
            operation = case["op"]
            maximum[operation] = max(maximum.get(operation, 0), difference)
            count[operation] = count.get(operation, 0) + 1
            # One working code accommodates float half-ties; delivery has no tolerance.
            tolerance = 0 if operation == "deliver" else 1
            assert difference <= tolerance, (case, actual, expected)
    ramp_reports = {}
    for transfer in ("srgb", "bt709"):
        ramp = [round(Q * i / 1023) for i in range(1024)]
        ordered = [dict(op="deliver", rgba=[v, v, v, Q], transfer=transfer, position=[x + 104, y + 208], ordered=True)
                   for v in ramp for y in range(8) for x in range(8)]
        actual = call(ordered)
        sums = []
        max_mean_error = 0.0
        for i, value in enumerate(ramp):
            tile = actual[i * 64:(i + 1) * 64]
            assert all(p[0] == p[1] == p[2] and p[3] == 255 for p in tile)
            total = sum(p[0] for p in tile)
            sums.append(total)
            max_mean_error = max(max_mean_error, abs(total / 64 - 255 * encode(value / Q, transfer)))
        assert all(a < b for a, b in zip(sums, sums[1:])), "The tile-averaged ramp must retain all 1024 levels"
        assert max_mean_error <= 1 / 128 + 1e-12
        assert sums[0] == 0 and sums[-1] == 255 * 64
        ramp_reports[transfer] = dict(input_levels=1024, distinct_tile_means=len(set(sums)),
                                      maximum_tile_mean_error_in_delivery_codes=max_mean_error)
    report = dict(
        passed=["working_color.transfer_and_premultiplication", "working_color.finite_linear_store",
                "working_color.transparent_source_over", "working_color.premultiplied_interpolation",
                "working_color.delivery_and_spatial_dither", "working_color.monotonic_tile_mean_ramp"],
        operation_counts=count, maximum_code_error=maximum,
        reference="Independent normalized Python f64 equations; recursive Bayer construction",
        tolerance="At most 1 UNORM16 code per working operation; exact 8-bit delivery; tile means within 1/128 delivery code",
        ramps=ramp_reports, response_sha256=digest.hexdigest(),
        limits="Library arithmetic only. No project mode, renderer integration, generated/nested frames, effects/grade-fade banding, higher-depth source import or speed claim.",
    )
    (root / "verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, required=True)
    run(parser.parse_args().output)
