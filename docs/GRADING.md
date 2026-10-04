# Primary grading and ordered scene effects

Scene layers accept up to eight ordered effects. The `grade` type provides primary correction; [selective grades](SELECTIVE_COLOR.md) restrict the same controls with color qualifiers and correction masks. Primary grades provide exposure, contrast, explicit RGB white-balance gains and master/channel curves, with optional animation of the five numerical controls. PNG sequences, text and shape layers use the same path. Use `scene.inspect` through CLI/library or MCP for validation and sampled values, then `scene.render` through CLI/library to compile a new asset for ordinary saved-session editing. Keep the editable scene recipe and external originals.

This is the declared sRGB scene path. General video-track effect evaluation, camera/log/HDR processing, automatic white balance, imported LUT files and scopes remain separate work. [Chroma keying](KEYING.md) is a separate ordered effect that can reduce alpha. Floating-point grading and 16-bit curve coordinates do not make the existing 8-bit input/output pipeline high bit depth.

## Layer schema

Add an optional `effects` list to an ordinary [scene layer](SCENES.md). Omission or an empty list retains the original compositor path. A complete grade example is:

```json
{
  "kind": "grade",
  "exposure_milli": 500,
  "contrast_milli": 1100,
  "white_balance_milli": [1100, 1000, 900],
  "master_curve": {"points": [[0, 0], [16000, 14000], [42000, 46000], [65535, 65535]]},
  "red_curve": {"points": [[0, 0], [65535, 65535]]},
  "animation": {
    "exposure_milli": {
      "keys": [
        {"time": {"num": 0, "den": 1}, "value": 0, "interpolation": "ease_in_out"},
        {"time": {"num": 1, "den": 1}, "value": 1000, "interpolation": "hold"}
      ]
    }
  }
}
```

The example's layer must last at least one second. Only `kind`, `exposure_milli`, `contrast_milli` and `white_balance_milli` are required. Curves and animation are optional; omitted curves are identity. Unknown effects, properties and unsupported numeric types fail explicitly.

| Field | Allowed values | Meaning |
| --- | --- | --- |
| `exposure_milli` | Integer -8000..8000 | Exposure in thousandths of a stop: 1000 doubles linear light; -1000 halves it |
| `contrast_milli` | Integer 0..4000 | Linear slope divided by 1000, around a fixed linear-light pivot of 0.18; 1000 is neutral |
| `white_balance_milli` | Three integers, each 100..4000 | Red, green and blue linear-light gains divided by 1000; `[1000,1000,1000]` is neutral |
| `master_curve` | `{points: [[input,output], ...]}` | Apply one scalar tone curve equally to RGB |
| `red_curve`, `green_curve`, `blue_curve` | Same curve shape | Apply a channel curve after the master curve |
| `animation` | One or more property curves below | Override static controls on the layer-local clock |

Tone curves have 2-32 ordered knots. Both coordinates are integers 0..65535 representing normalized linear light 0..1. Input knots must be strictly increasing, with first input 0 and last input 65535. Output knots need not be monotonic: inversion and deliberate stylized curves are supported. The endpoints may lift black or lower white. Values between knots use straight-line interpolation. Duplicate, unsorted, missing-endpoint, empty and oversized curves fail. Curve points themselves are static in this version.

White balance is an explicit diagonal RGB correction, not a temperature/tint estimator or chromatic-adaptation matrix. For a measured neutral patch, convert its encoded channels to linear sRGB and choose gains that bring them to a common positive target. For example, approximately `[0.1,0.2,0.4]` linear RGB needs gains `[2000,1000,500]` to approach neutral `[0.2,0.2,0.2]`. Source quantization and the supported gain range limit the correction; inspection never infers gains from an image.

## Processing contract

Source colors use the existing scene's explicit sRGB interpretation. For stored premultiplied input, unpremultiply in encoded sRGB before decoding the transfer function. At zero alpha, the pixel remains invisible. Grading never changes alpha.

Decode each straight encoded channel to linear light using the standard sRGB transfer. Each grade then uses this original control order:

1. Multiply by `2^(exposure_milli/1000)` and the corresponding white-balance gain.
2. Apply contrast around 0.18: `0.18 + (value - 0.18) * contrast_milli/1000`.
3. Clamp to `[0,1]`.
4. Apply the master tone curve, then the channel's tone curve.

Feed that linear result into the next grade in list order. No intermediate 8-bit encoding occurs between grades. Clipping happens inside every grade, so changing effect order can change the result. For example, brightening then darkening a clipped highlight cannot recover its original value; darkening first can avoid that clipping. Exposure and white balance retain intermediate headroom until the contrast clamp within the same grade.

After the final grade, encode to sRGB and round to the nearest 8-bit level, with half ties upward. The resulting straight RGB and preserved alpha enter the existing crop/transform/mask/opacity/blend behavior. Layer blending still uses encoded sRGB over the opaque scene background; grading does not change the compositor to linear-light blending. Missing regions of trimmed images stay transparent, even when a curve lifts black.

Calculations use bounded `f64` values. Straight 8-bit source channels use a per-frame table of all 256 exact input levels; premultiplied pixels retain their fractional unpremultiplied values through grading. Quantization to straight 8-bit before compositing can produce a one-level difference from ideal arithmetic, which the fixture bounds explicitly. A wholly neutral chain uses the original integer compositor exactly, including fractional stored premultiplied colors. Identity curves may contain any valid knots with equal input/output coordinates.

## Animation and inspection

Animation keys may target `exposure_milli`, `contrast_milli`, `red_balance_milli`, `green_balance_milli` or `blue_balance_milli`. A supplied property curve replaces that property's static value. Static values still must be valid. Every curve uses the same bounds as its property and the [ordinary exact-rational animation contract](SCENES.md): hold/linear/quadratic easing, nearest-integer parameter rounding, 1-128 keys, insertion-order independence, endpoint holds and optional delay/rate/reversal. The clock is relative to layer start and continues across looping or held source frames. Tone-curve knots are not keyframed.

The corresponding layer timing report contains the declared `effects`, its `effect_processing` policy, and an `effects` array inside each active frame's `sampled_parameters`. Each sampled grade reports its exposure, contrast and three white-balance gains. Inactive frames remain `null`; recipes without effects omit the extra fields. Curves are validated even on transparent or zero-opacity layers. Inspection does not write images or change the recipe.

The `capabilities` response exposes supported effect types, ranges, ordering, curve limits, working/output spaces and alpha behavior. There are no new MCP tools: effects are part of the shared scene schema. Templates can preserve and instantiate complete grade recipes; typed template bindings do not yet target grade parameters. Compiled graded assets retain ordinary saved-session trimming, retries and export behavior. Changing the editable recipe requires recompilation to a new output.

All existing scene limits remain in force, including ten seconds, 16 layers, bounded source canvases/output dimensions, source identity checks and no-overwrite publication. There is no runtime download or new dependency.

## Runnable workflow and acceptance

```powershell
cargo build --locked
python -X utf8 tests/grading.py --output C:\DEV\CutboltData\new-grading-test
```

This produces original external charts/fonts and a retained `animated-scene.json`. For example, replace that scene's grade with a new static correction and render a fresh output:

```python
from pathlib import Path
import json, subprocess

root = Path(r"C:\DEV\CutboltData\new-grading-test")
scene = json.loads((root / "animated-scene.json").read_text(encoding="utf-8"))
scene["id"] = "grade-copy"
scene["layers"][0]["effects"] = [{
    "kind": "grade", "exposure_milli": 500, "contrast_milli": 1100,
    "white_balance_milli": [1100, 1000, 900]
}]
request = {"command": "scene.render", "scene": scene,
           "input_root": str(root / "sources"), "output_root": str(root / "output"),
           "output": str(root / "output" / "grade-copy.mkv")}
result = subprocess.run([r"C:\DEV\Cutbolt\target\debug\cutbolt.exe"],
                        input=json.dumps(request), text=True, encoding="utf-8",
                        capture_output=True, check=True)
print(json.loads(result.stdout)["result"]["asset"])
```

The acceptance fixture compares all **128 decoded frames and 245,760 silent stereo sample frames** across 24 scene renders and one saved-session cut. Its independent reference evaluates standard transfer and original grading equations with 48-digit Decimal arithmetic, uses analytical exposure/gray anchors, exact Fraction parameter clocks, original font/shape geometry and a forward image compositor. It covers full grayscale ramps, original RGB charts, corrected neutral patches, extrema, clipping, nonmonotonic/master/channel curves, eight-stage and reversed-order chains, low/zero/full alpha, stored premultiplied colors, neutral bypass, transparent trim regions, all blend modes, looped source frames, five animated controls, text/shapes, template reuse and MCP. Maximum allowed RGB error is one 8-bit level; neutral bypass is checked byte-for-byte. Audio must match exactly. Thirty rejected cases verify invalid controls/curves/alpha and source/output preservation.

These checks earn C02 basic and extended only. They do not award general color management, LUT/scopes, HDR or keying points. Selective grading has its own [acceptance fixture](SELECTIVE_COLOR.md). The unchanged checklist continues to require those capabilities.

The transfer-function facts follow [W3C CSS Color 4, section 10.2, draft of 30 September 2026](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#predefined-sRGB). The grading controls, ordering, clipping and fixture charts are original project design. No third-party implementation or document copies are included.
