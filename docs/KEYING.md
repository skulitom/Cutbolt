# Chroma keying and reusable effect presets

The original `chroma_key` scene effect removes a specified screen color, with hard or soft boundaries, optional spill suppression, optional screen-color subtraction, feathered masks and animated strength. Each layer has its own ordered effect chain; keys can run before, between or after primary/selective grades. The existing limit is eight effects per layer in total, 16 scene layers and ten seconds. PNG sequences and generated text/shapes use the same path. Direct general video-track effects remain open.

Keying changes the layer's transparency before it is composited against lower layers and the scene background. The compiled reference MKV is flattened RGB; this feature does not add an alpha-bearing delivery format. Keep the editable scene and original images for later changes.

## Effect schema

Add this object to a layer's `effects` list:

```json
{
  "kind": "chroma_key",
  "key_rgb": [0, 255, 0],
  "inner_milli": 60,
  "outer_milli": 180,
  "strength_milli": 1000,
  "unmix_milli": 0,
  "spill": {"channel": "green", "strength_milli": 650},
  "mask": {"rect": [8, 4, 40, 24], "feather": 4, "inverted": false}
}
```

Required fields are `key_rgb`, `inner_milli`, `outer_milli` and `strength_milli`. Key colors are three encoded sRGB bytes and must not be exactly gray. Thresholds must satisfy `0 <= inner_milli <= outer_milli <= 1000`. Strength, unmix and spill strength use integer thousandths in 0..1000. `unmix_milli` defaults to zero; `spill` and `mask` are optional. Unknown fields, unsupported channels, malformed values and invalid animation fail before output is published, even on disabled or hidden layers.

## Distance and transparency

The key measures the straight color entering this effect, after preceding effects. For measurement only, encode the current linear-light RGB to sRGB and round to three bytes with nearest rounding and positive half ties upward. Let these bytes be `r,g,b` and the screen bytes be `kr,kg,kb`. Distance in thousandths is:

```text
d = 1000/510 * max(
    abs((r-g) - (kr-kg)),
    abs((g-b) - (kg-kb)),
    abs((b-r) - (kb-kr)))
```

This is a project-defined opponent-channel distance, not a perceptual color metric. Equal gray offsets cancel, but brightness scaling and hue changes generally change distance. A gray key color is rejected; gray foreground can still be removed by sufficiently broad thresholds. No screen color is inferred automatically.

The retained matte `m` is 0 at/below the inner threshold, 1 at/above the outer threshold, and linear between them. With equal thresholds, the boundary itself is fully removed and the next greater distance is retained. Integer comparisons avoid rounding distance before the boundary decision. For a pure green key, `[0,204,0]` lies exactly at 100 thousandths and `[0,203,0]` is just outside. A hard threshold of 100 deliberately distinguishes them.

Let `w = strength_milli/1000 * mask_coverage`, using coverage 1 when no mask is present. The new alpha is `old_alpha * (1 - w*(1-m))`. Multiple keys multiply the existing alpha without intermediate byte quantization; only the complete chain's final alpha is rounded to an 8-bit value. Keys never increase alpha. Later grading preserves the alpha established by earlier keys.

Zero strength disables the entire key, including spill and unmix. Zero mask coverage also bypasses it. Pixels with full retention and no active spill excess retain their original sample and alpha interpretation when no other stage changes them. Thus a wholly bypassed chain preserves stored premultiplied colors exactly. Input premultiplied pixels are otherwise unpremultiplied before processing; final changed colors use straight sRGB bytes before the existing encoded-sRGB layer compositor.

## Soft-edge color recovery and spill

Removing transparency alone can leave screen color in soft foreground edges. Optional `unmix_milli` subtracts the declared screen contribution from encoded, straight RGB using the computed matte:

```text
recovered = clamp((incoming - (1-m)*screen) / m, 0, 1)
candidate = lerp(incoming, recovered, unmix_milli/1000)
```

At `m=0`, recovered RGB is defined as zero; there is no division. Subtraction uses the unquantized encoded working color, while the matte comes from the explicitly quantized distance. At `m=1`, unmix is a no-op. The effect then optionally suppresses spill: for the named `red`, `green` or `blue` channel, subtract `spill_strength * max(channel - max(other_two_channels), 0)`. Other channels are unchanged. Spill runs after unmix and applies throughout the mask, including fully retained pixels; it is not limited to partially transparent edges. This allows removal of a reflected screen tint but can also alter legitimate colors in the selected region.

Decode this candidate to linear sRGB, blend it with the incoming linear RGB by `w`, and pass the result to later effects. Alpha reduction uses the same `w`. Full key strength applies the complete color candidate; partial strength or mask feathering mixes the correction in linear light. Unmix/spill amounts and key thresholds are static; only overall strength and the mask rectangle are animated.

Screen subtraction assumes the inferred matte correctly describes an encoded-color mixture against this exact screen. It can clip or amplify quantization/noise when the inferred coverage is small or the assumed screen is wrong. Presets leave unmix disabled. The synthetic known-edge fixture proves recovery for its declared neutral foreground mixtures; it does not establish universal hair, glass, motion-blur or uneven-lighting quality. There is no spatial denoise, edge erosion/dilation, temporal smoothing, automatic background estimation or machine-learning matte in this effect.

## Masks and animation

The optional mask uses the same [source-canvas inward feather rule](SELECTIVE_COLOR.md#correction-masks-and-mixing) as selective correction. Rectangles are evaluated at pixel centers before crop/scale/rotation/position, including PNG trim offsets. Positions are -32768..32768, dimensions 0..32768 and feather 0..4096. Inversion, empty and outside rectangles have the same explicit behavior. A separate ordinary layer mask still clips the completed effect result during composition.

`strength_curve` is an optional ordinary property curve with values 0..1000. `mask.animation` accepts `x`, `y`, `width` and `height` curves. Both use the exact rational layer-local clock, key sorting, easing, endpoint holds and optional delay/rate/reversal. Source-frame looping does not restart the property clock. Static values must be valid even if a curve overrides them.

`scene.inspect` reports each key's sampled `strength_milli` and nullable `mask_rect` alongside the declared effect. `capabilities.effects.keying` exposes distance, alpha, spill, mask, animation and preset policies. The complete scene schema is shared by CLI/library and MCP.

## Presets and reuse

`effects.preset` is a read-only CLI/library/MCP command returning a fresh editable effect list. Strength and spill arguments are explicit, both 0..1000:

```json
{"command":"effects.preset","name":"green_soft","strength_milli":1000,"spill_milli":650}
```

The result has `schema_version: 1`, `preset` and `effects`. Copy the `effects` array into a layer, or combine it with other effects while respecting the total of eight. Modify the returned copy to change key color, thresholds, unmix, masks or animation. Save the complete effects inside the caller-owned scene or a reusable scene template. Each returned preset and each layer is independent; editing one does not change later preset calls or other layers. Presets do not reference an external application or load plugins.

| Name | Key RGB | Inner / outer | Spill channel |
| --- | --- | --- | --- |
| `green_soft` | `[0,255,0]` | 60 / 180 | green |
| `blue_soft` | `[0,0,255]` | 60 / 180 | blue |
| `green_hard` | `[0,255,0]` | 100 / 100 | green |
| `blue_hard` | `[0,0,255]` | 100 / 100 | blue |

All four original presets use unmix 0 and no mask/animation. They are explicit starting recipes; their thresholds are not automatically calibrated to a source. `scene.inspect` validates the edited scene and its content-checked sources before compilation. Compiled assets enter normal saved-session editing, retries, undo and export.

## Runnable workflow and evidence

```powershell
cargo build --locked
python -X utf8 tests/keying.py --output C:\DEV\CutboltData\new-keying-test
```

The fixture writes an editable `animated-scene.json`. This example replaces its first layer's effects with an independent blue-screen preset and creates a new output:

```python
from pathlib import Path
import json, subprocess

root = Path(r"C:\DEV\CutboltData\new-keying-test")
exe = r"C:\DEV\Cutbolt\target\debug\cutbolt.exe"
def call(request):
    result = subprocess.run([exe], input=json.dumps(request), text=True,
                            encoding="utf-8", capture_output=True, check=True)
    return json.loads(result.stdout)["result"]

scene = json.loads((root / "animated-scene.json").read_text(encoding="utf-8"))
scene["id"] = "blue-preset-example"
scene["layers"][0]["effects"] = call({"command":"effects.preset",
    "name":"blue_soft", "strength_milli":1000, "spill_milli":650})["effects"]
call({"command":"scene.inspect", "scene":scene, "input_root":str(root / "sources")})
print(call({"command":"scene.render", "scene":scene,
    "input_root":str(root / "sources"), "output_root":str(root / "output"),
    "output":str(root / "output" / "blue-preset-example.mkv")})["asset"])
```

Acceptance uses original color charts, stored premultiplied images and a known neutral foreground with soft edges and single-pixel strands. Exact Fraction geometry computes independent opponent distances, masks and accumulated alpha; 48-digit Decimal arithmetic checks color subtraction, spill, grades and composition. Analytical checks cover hard boundary ties, preserved grays/contrasting colors, screen-color removal, spill caps and the final alpha after eight stages. Full decoded frames and PCM are compared, with a maximum allowed RGB error of one level and exact silent PCM.

The fixture also checks all four presets, independent copies/layers, all blend modes, low alpha, disabled/outside bypass, inverted/thin/empty masks, crop/trim offsets and four rotations, animated strength/masks, looped sources, mixed effect order, generated graphics, MCP schemas, template reuse, saved-session retries, rejection and source/output preservation. It provides V08 evidence without claiming tracking, high bit depth, general color management or a general native video-track effects path. No dependency or third-party implementation was added.

The retained run compares **182 decoded frames and 349,440 silent stereo sample frames** across 42 scene renders and one saved-session cut, with 29 rejected/output-preservation cases. Maximum RGB error against the mathematical reference is one level. Known neutral edges recover the original foreground composite exactly; omitting unmix leaves up to 64 levels of error in that same deliberately constructed case. These are bounded fixture results with the limitations above, not measurements on arbitrary screen footage.
