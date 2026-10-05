# Selective color correction

The `selective_grade` scene effect applies a [primary grade](GRADING.md) through color qualifiers, a correction mask, or both. It supports hard/soft selection boundaries, inversion, an adjustable mix and animated grade controls, mix and mask rectangles. Pixels outside the selection keep their original color and alpha interpretation. This changes color without creating transparency; [chroma keying](KEYING.md) is a separate ordered effect, and tracking remains open.

Use the existing `scene.inspect` CLI/library/MCP command to validate and inspect sampled parameters, then `scene.render` through CLI/library to compile a new asset. No new command or dependency is required. The scene limit is still eight ordered effects per layer in total, 120 seconds and 64 layers. This path processes bounded PNG/graphics scenes, not arbitrary native video tracks.

## Effect schema

Add this object to a layer's `effects` list:

```json
{
  "kind": "selective_grade",
  "grade": {
    "exposure_milli": 500,
    "contrast_milli": 1000,
    "white_balance_milli": [1100, 1000, 900]
  },
  "mix_milli": 800,
  "qualifier": {
    "hue": {"center": 0, "inner": 15000, "outer": 40000},
    "saturation": {"low": 200, "high": 1000, "feather": 100},
    "lightness": {"low": 100, "high": 900, "feather": 100},
    "inverted": false
  },
  "mask": {
    "rect": [8, 4, 40, 24],
    "feather": 4,
    "inverted": false
  }
}
```

`grade` and `mix_milli` are required, plus at least one of `qualifier` or `mask`. **The nested `grade` has no `kind` field.** Its controls, optional master/RGB curves, [hue shift and saturation](GRADING.md#hue-and-saturation), animation and limits are exactly those in the primary grading contract. `mix_milli` is an integer 0..1000: 0 bypasses the correction, 1000 applies the full selected correction. Unknown fields and unsupported effect kinds fail explicitly.

## Color qualification

Each qualifier needs at least one of `hue`, `saturation` or `lightness`; omitted bands contribute weight 1. `inverted` defaults to false. A completely empty qualifier is rejected; omit the qualifier for a mask-only correction.

Selection measures the color entering this effect, after preceding effects. Convert the current linear RGB to straight encoded sRGB and round each channel to an 8-bit value for qualification only. The working RGB itself retains floating-point precision. Calculate HSL from these three integer channels, round hue to millidegrees and saturation/lightness to thousandths using nearest rounding with positive half ties upward. Normalize hue to `[0,360)` before rounding and wrap a rounded 360 back to 0. Integer arithmetic makes thresholds deterministic. This deliberately quantized selection profile is distinct from continuous perceptual color distance or scene-linear luminance.

| Band | Fields and bounds | Weight |
| --- | --- | --- |
| Hue | `center` 0..359999 millidegrees; `0 <= inner <= outer <= 180000` | Use the shortest circular distance from center. Weight 1 at/below inner radius, 0 at/above outer radius, linear between |
| Saturation | `0 <= low <= high <= 1000`, `feather` 0..1000 | Weight 1 inside the inclusive low/high range, falling linearly to 0 over `feather` outside each edge |
| Lightness | Same scalar-band bounds | Same rule, applied to HSL lightness |

For a hard hue boundary, use equal inner and outer radii; equality is included. A hard scalar band uses feather 0. Scalar feathering can extend beyond the domain; only valid 0..1000 measurements are evaluated. Hue wraps through red: a center of 0 and radius 20000 covers colors up to 20 degrees on either side. Black, white and exact gray have no hue and receive hue weight 0, even with an outer radius of 180000. Omit hue to select them by saturation/lightness. Near-gray colors still have a hue; a saturation band can exclude them.

The qualifier weight is the product of its present hue/saturation/lightness weights. If `inverted` is true, replace that complete product with `1 - weight`. Inversion can therefore include achromatic colors that a hue band originally excluded. It does not invert individual bands separately or invert the spatial mask.

Quantization has explicit boundaries. For example, `[64,1,0]` rounds to hue 938 millidegrees, while `[64,0,1]` rounds to 359063. Around a center of 0 those distances are 938 and 937 respectively. A hard radius of 937 intentionally distinguishes them. Use a soft outer interval when that single-unit threshold change is undesirable. Saturation also has tested half-tie behavior, including `[68,60,60]` mapping to 63 thousandths.

## Correction masks and mixing

A correction mask has `rect: [left,top,width,height]`, integer `feather` 0..4096, optional `inverted` (default false) and optional `animation`. Coordinates use the full source canvas **before crop, scale, rotation and positioning**, including restored PNG trim offsets. Positions use -32768..32768 and dimensions 0..32768, matching the existing rectangular-mask bounds. A rectangle may extend outside the canvas. Empty dimensions produce zero coverage before inversion.

Hard masks include source pixel centers inside the rectangle. With feather greater than zero, coverage rises linearly inward from each edge: at a pixel center, take the nearest distance to any rectangle edge, divide by feather and clamp to 0..1. Outside coverage is zero. Corners use this nearest-edge rule rather than a rounded or Gaussian contour. A one-pixel-wide rectangle with feather 1 has coverage 0.5 at its pixel centers. Inversion replaces coverage by `1 - coverage`; an inverted empty mask covers every actual source pixel. Missing portions of a trimmed image remain absent.

Multiply qualifier weight, mask coverage and `mix_milli/1000` to get the final correction weight. Missing qualifier or mask contributes 1. Compute the candidate grade with its normal linear-light control order and clipping, then blend the incoming and corrected linear RGB by that weight. Later effects see the blended result. The source alpha is preserved. The ordinary layer `mask`, `opacity` and `blend_mode` still control final compositing independently; a correction mask never cuts a hole in the image.

If every effect leaves a pixel unselected, the original encoded sample and alpha interpretation reach the compositor without a straight-8-bit round trip. This preserves fractional stored premultiplied colors exactly. Entire zero-mix or neutral-grade chains use the existing bypass path. Selected results use the usual final straight-sRGB quantization, with a verified maximum tolerance of one 8-bit RGB level relative to the high-precision reference.

## Animation and inspection

- Nested `grade.animation` animates the seven primary grading controls.
- Optional effect-level `mix_curve` overrides `mix_milli`, using the ordinary property-curve shape and values 0..1000.
- `mask.animation` may contain `x`, `y`, `width` and `height` curves, with the same bounds as the static rectangle. Mask feather and color-band boundaries are static.

All curves use the existing exact rational layer-local clock, key ordering, easing, endpoint holds and optional delay/rate/reversal. Looping source frames do not restart this clock. Empty animation objects, bad bounds, duplicate times and invalid rational times fail, including on hidden/zero-opacity layers. Static values must remain valid even when overridden by animation.

Inspection retains the declared effect and includes its sampled grade controls, `mix_milli` and nullable `mask_rect` in each active frame's `sampled_parameters.effects`. Inactive frame parameters remain null. The `capabilities.effects.selection` object describes the qualifier, mask and preservation policies. No automatic subject detection, mask tracking, arbitrary paths, segmentation or qualifier histogram is provided by this effect.

## Runnable workflow and evidence

```powershell
cargo build --locked
python -X utf8 tests/selection.py --output C:\DEV\CutboltData\new-selection-test
```

The retained fixture includes an editable `animated-scene.json`. This example replaces its first effect with a static red selection and publishes a new render:

```python
from pathlib import Path
import json, subprocess

root = Path(r"C:\DEV\CutboltData\new-selection-test")
scene = json.loads((root / "animated-scene.json").read_text(encoding="utf-8"))
scene["id"] = "selected-red"
scene["layers"][0]["effects"] = [{
    "kind": "selective_grade", "mix_milli": 700,
    "grade": {"exposure_milli": 500, "contrast_milli": 1000,
              "white_balance_milli": [1100, 1000, 900]},
    "qualifier": {"hue": {"center": 0, "inner": 15000, "outer": 40000}}
}]
request = {"command": "scene.render", "scene": scene,
           "input_root": str(root / "sources"), "output_root": str(root / "output"),
           "output": str(root / "output" / "selected-red.mkv")}
result = subprocess.run([r"C:\DEV\Cutbolt\target\debug\cutbolt.exe"],
                        input=json.dumps(request), text=True, encoding="utf-8",
                        capture_output=True, check=True)
print(json.loads(result.stdout)["result"]["asset"])
```

To recolor rather than brighten a selection, rotate its hue. Measure the selected color's HSL hue, then shift by the difference to the target hue. For example, a red scarf near 350 degrees turns gold, near 45 degrees, with a turn of about 55 degrees. The qualifier sees the incoming color, so the turn cannot reselect pixels:

```json
{"kind": "selective_grade", "mix_milli": 1000,
 "grade": {"exposure_milli": 0, "contrast_milli": 1000, "white_balance_milli": [1000, 1000, 1000],
           "hue_shift_mdeg": 55000, "saturation_milli": 1100},
 "qualifier": {"hue": {"center": 350000, "inner": 8000, "outer": 16000},
               "saturation": {"low": 400, "high": 1000, "feather": 50}}}
```

Keep the editable recipe; rendered assets use normal saved sessions and need recompilation for later color changes. Existing files are never overwritten.

The independent acceptance fixture compares **176 decoded frames and 337,920 silent stereo sample frames** across 37 scene renders and a saved-session cut. It uses exact Fraction HSL geometry and selection/mask weights, 48-digit Decimal grading, original color charts and forward per-pixel composition. It covers hue wraparound and exact half ties, achromatic/near-gray colors, hard/soft scalar boundaries, product/inverted qualifiers, selected hue turns and saturation (static, soft-mixed and animated), thin/empty/outside/inverted masks, four rotations with crop/trim offsets, all blend modes, low/zero/full alpha, unchanged premultiplied pixels, eight ordered effects, different selections on shared images, animated controls/mix/masks, looping source frames, shape layers, template reuse, MCP/schema checks and saved-session retries. The retained run matched every RGB value exactly except in the hue cases, where values whose exact result is a half-level tie differ by one; the numerical contract permits at most one level. A selected pure red turned 55 degrees must give exactly `[255,234,0]`. All PCM samples must match exactly. Twenty-eight rejection/output-preservation cases and source hashes check failure safety. Two direct Rust tests additionally check known HSL values, half ties and mask pixel-center coverage.

This earns C03 basic and extended only. Chroma key has its own [acceptance fixture](KEYING.md); tracking and broader color management remain required by their own unchanged checkpoints. The HSL interpretation follows the public definitions in [W3C CSS Color 4, sections 7 and 7.2, draft of 30 September 2026](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#rgb-to-hsl). Quantization, weights, masks and correction order are original project design; no external implementation, copied document or media asset is included.
