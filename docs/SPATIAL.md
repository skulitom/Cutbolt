# Interpolated and animated spatial transforms

Scenes can opt into `transform.spatial` for subpixel translation, independent horizontal/vertical scale, clockwise rotation, mirroring, declared pixel aspect ratio and aspect fitting. This works with PNG frames and generated text/shapes, including source masks and grading. Existing recipes that omit this field retain their original integer compositor and serialized shape.

Use the existing `scene.inspect` and `scene.render` commands. Inspection reports sampled transform values and forward/inverse matrices for every active frame. Scene compilation produces a new ordinary timeline asset; saved edits, previews, proxies and exports use that compiled asset. Keep the editable scene recipe and original sources externally. Changing a scene does not replace an older compiled asset automatically.

## Spatial declaration

Add this object inside a layer's existing `transform`:

```json
{
  "translate_milli": [250, -500],
  "scale_milli": [1000, 1000],
  "rotation_mdeg": 0,
  "flip": [false, false],
  "pixel_aspect": {"num": 16, "den": 15},
  "sampling": "bilinear",
  "edge": "transparent",
  "fit": {"size": [30, 20], "mode": "contain"},
  "viewport": [5, 5, 30, 20],
  "animation": {
    "rotation_mdeg": {
      "keys": [
        {"time":{"num":0,"den":1},"value":-15000,"interpolation":"ease_in_out"},
        {"time":{"num":12,"den":25},"value":35000,"interpolation":"hold"}
      ]
    },
    "scale_x_milli": {
      "keys": [
        {"time":{"num":0,"den":1},"value":1000,"interpolation":"linear"},
        {"time":{"num":12,"den":25},"value":1250,"interpolation":"hold"}
      ]
    }
  }
}
```

`translate_milli`, `scale_milli`, `rotation_mdeg`, `flip`, `pixel_aspect`, `sampling` and `edge` are required when this option is present. `fit`, `viewport` and `animation` are optional. The example requires a layer duration of at least 12/25 seconds; its viewport and fit size are logical scene pixels.

| Field | Meaning and bounds |
| --- | --- |
| `translate_milli` | Add to the existing sampled anchor position, in 1/1000 pixels; each axis −32,768,000…32,768,000 |
| `scale_milli` | Multiply each source axis by 1/1000 of this value; 1…16,000, strictly positive |
| `rotation_mdeg` | Add clockwise rotation to existing `quarter_turns`, in 1/1000 degrees; −3,600,000…3,600,000 |
| `flip` | Mirror the source's horizontal/vertical axes before rotation |
| `pixel_aspect` | Explicit source-pixel width/height, rational 1/16…16, reduced denominator at most 1,000,000 |
| `sampling` | `nearest` or `bilinear` |
| `edge` | Transparent tap extension or clamped crop-edge sampling |
| `fit.size` | Positive target width/height, each at most 32,768 |
| `fit.mode` | `contain`, `cover` or `stretch` |
| `fit.window_size` | Optional positive fitting reference width/height, each 1…32,768; defaults to the legacy source crop dimensions |
| `viewport` | Optional `[x,y,width,height]` destination clip; positions −32,768…32,768, sizes 1…32,768 |

Pixel aspect is a caller declaration for scene sources, not embedded-metadata inference. Output reference media always uses square pixels. A missing spatial field means the original square-pixel integer path; an explicit identity spatial object uses unit aspect, unit scale, zero translation/rotation and no flips/fit/viewport.

## Geometry and fitting order

Source coordinates use full-canvas pixel corners, including trimmed-image offsets. The selected frame's original `anchor` maps to the existing sampled `position` plus the spatial translation. Generated graphics use anchor `[0,0]`. Source crop and masks retain full source-canvas coordinates.

For source point `p` and selected source anchor `a`, the mapping is:

```text
destination = sampled_position + translate_milli / 1000
            + clockwise_rotation * axis_scale * (p - a)
```

Start the axis scale with `[legacy_scale * pixel_aspect, legacy_scale]`. Fitting compares the reference's resulting unrotated width/height with `fit.size`: the reference is `fit.window_size` when supplied, otherwise the legacy source crop dimensions. Contain uses the smaller ratio uniformly, cover the larger ratio uniformly, and stretch the two ratios independently. Multiply those scales by `scale_milli / 1000` and mirror signs. Finally rotate by `quarter_turns * 90 degrees + rotation_mdeg / 1000`. The selected anchor stays fixed through scaling and rotation.

Fitting computes scale before spatial scale/rotation. It does not choose an anchor, center the image or crop the destination. For centered fitting, supply the crop center as the frame anchor and the intended destination center as the legacy position. Use `viewport` for an explicit cover crop or letterbox boundary. The viewport is fixed in logical scene coordinates and clips before scene output enlargement. It does not allocate a separate canvas. Source crop always restricts the available sampling taps.

The optional fitting reference changes geometry only; it does not restrict source taps or translate the source. [Subject reframing](REFRAMING.md) combines an explicit fitting window with sampled source anchors and a matching output canvas. Omitting `fit.window_size` preserves the prior fitting behavior exactly. Independent cases check all three modes with pixel aspect and a reference window distinct from the source crop.

Inspection matrices are row-major six-element arrays `[a,b,c,d,e,f]`: `(x,y)` maps to `(a*x+b*y+c, d*x+e*y+f)`. Forward matrices map source corners into the scene; inverse matrices map scene corners back to source. Geometry uses bounded binary64 calculations, with exact sine/cosine landmarks at quadrant rotations. Timeline clocks and property interpolation remain exact rational calculations.

## Sampling, transparency and effects

Each logical destination pixel is sampled at its center. The inverse-mapped source coordinate is rounded to a 1/65,536-pixel grid, nearest with half ties away from zero. Nearest sampling chooses the containing source pixel. Bilinear sampling interpolates the four neighboring source pixel centers with exact integer weights from that coordinate grid.

With `edge: "transparent"`, taps outside the crop are transparent. Bilinear support can extend half a source pixel beyond a crop boundary; this is a filtered edge. With `edge: "clamp"`, the mapped sample must remain inside the half-open crop footprint, and taps clamp to its edge pixels. This avoids extending the edge across the entire scene. Restored trim-canvas regions outside the actual image remain transparent, even when the crop tap is clamped.

At each valid source tap, the ordinary source mask is applied, then the ordered effect chain is evaluated at that tap's source coordinate. Effects retain their documented processing and output quantization. Sampling accumulates premultiplied RGB and alpha without an intermediate RGBA8 rounding step. Straight input is multiplied by alpha for filtering; valid stored premultiplied input retains its stored color values. Hidden RGB under zero alpha cannot leak into filtered edges.

After filtering, layer opacity and the existing normal/multiply/screen blend are applied to the opaque backdrop, with one final channel rounding. Interpolation and compositing remain in encoded sRGB, matching the scene contract. This does not introduce linear-light filtering or transparent scene output. The ordinary final `output_scale` remains nearest-neighbor enlargement after logical scene compositing.

Nearest sampling preserves pixel-art cells. Bilinear provides local interpolation, including fractional movement and rotation; it is not an area filter for strong minification, a general polygon coverage integrator, or motion blur. Those distinctions remain visible in capability discovery. Shear, perspective and 3D transforms are not implied by this option.

Rendering reaches these values through faster paths: direct reads of source pixels, effects evaluated once per sampled source pixel rather than per tap, per-column and per-row mapping of axis-aligned transforms, and parallel row bands when processors are idle. The values are unchanged. See [the scene limits](SCENES.md#limits) and the 5 October 2026 entry in [PROGRESS_HISTORY.md](PROGRESS_HISTORY.md) for measured costs.

## Animated parameters

`spatial.animation` supports `translate_x_milli`, `translate_y_milli`, `scale_x_milli`, `scale_y_milli` and `rotation_mdeg`. Each curve uses the existing [exact property keyframe contract](SCENES.md#property-keyframes), including hold/linear/quadratic easing, independent retiming, endpoint holds and integer half-away rounding. Values use the same units/bounds as their static fields. Scale remains positive; mirroring is explicit static state. Rotation interpolates the supplied signed angle values, not an inferred shortest arc.

The animation clock is local to the layer, independent of source-frame loops and changing trimmed-frame anchors. Existing integer position and opacity animation still works; spatial translation adds to that sampled position. Crop, fit, pixel aspect, flip, sampling, edge and viewport are static in this version. Empty animation objects, duplicate keys, invalid times, zero scale and unsupported fields fail during inspection, including hidden layers. Each curve retains the existing 128-key and exact arithmetic limits.

`sampled_parameters.spatial` reports the resolved translation, scale and rotation plus both matrices. Static spatial metadata preserves authored key insertion order; sampled parameters and rendered output are independent of that order. The existing MCP schema and typed template scene payload retain the new fields; no additional MCP tool is introduced. Template bindings retain their existing explicit property set.

## Run and verify

Generate original fixtures outside the repository:

```powershell
python -X utf8 tests/spatial.py --output C:\DEV\CutboltData\spatial-demo
$scene = Get-Content C:\DEV\CutboltData\spatial-demo\scene.json -Raw | ConvertFrom-Json
@{
  command = 'scene.inspect'
  scene = $scene
  input_root = 'C:\DEV\CutboltData\spatial-demo\sources'
} | ConvertTo-Json -Depth 100 -Compress | .\target\debug\cutbolt.exe
```

To compile an edited scene, send `scene.render` with the same scene/input root plus an existing `output_root` and unused `.mkv` output. Register its returned `asset` through the existing saved-session contract. Existing source/output files are never overwritten; sources are rechecked after encoding and before publication. All existing scene duration, layer, image and pixel budgets still apply.

The retained fixture is `C:\DEV\CutboltData\spatial-20261003-03`. Its independent oracle transforms basis points using 60-digit Decimal geometry, obtains the inverse analytically, then filters/composites with rational arithmetic. It verifies four exact legacy quadrant cases; nine wide/tall/fractional aspect fits; crop/trim/anchor changes; both mirror axes; composed arbitrary rotations; both edge policies; straight/premultiplied equivalence; all three blends; animated scale/translation/rotation/opacity and retiming; generated graphics; effects and masks before filtering; layered output enlargement; templates and MCP; saved retries/undo; full/range/proxy previews; large-coordinate bounds and source preservation.

The fixture compares **602 decoded frames, 1,155,840 stereo sample frames and seven still previews**, and passes **31 rejection/failure cases**. Actual pixel error was zero throughout this retained run. Acceptance requires exact pixels for quadrants, aspect fitting and alpha-equivalence cases; arbitrary rotations and graded colors allow at most one RGB unit against the high-precision reference. Every silent PCM sample is exact. This evidence targets **V01 extended** only; tracking, stabilization, reframing, temporal integration and general timeline effects retain their own checkpoints.

Full repository verification passed **275 unique checks**, awarding V01 extended and producing **69/100 core** and **69/108 total (63.9%)**. The generated progress files remain authoritative.

## Measured source compensation

Optional `transform.spatial.compensation` carries [camera stabilization](STABILIZATION.md) as editable translation/roll curves, a fixed source center, constant zoom and compensated viewport. It composes before the existing authored mapping, so nonuniform scales, flips, aspect fitting, anchors and animation remain distinct. Inspection reports each correction sample, the composed source/scene matrices and the inverse authored mapping used for viewport clipping. Omission retains the original mapping and serialized report.
