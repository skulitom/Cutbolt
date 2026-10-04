# Motion tracking and layer-mask edges

`tracking.inspect` measures an original local image patch and returns an editable rectangular mask animation plus a replacement scene recipe. The command is read-only through CLI, the library and MCP. Explicitly inspect and compile its returned `scene` to a new file with `scene.render`, then register the compiled asset through the existing session operations. Measurements alone do not change a saved project.

## Input and coordinate contract

The request contains `scene`, absolute `input_root`, `layer_id`, `model: "translation"`, `region: [x,y,width,height]`, the six search/confidence controls below and a static `mask`. The scene keeps its content-bound relative image paths. The full scene is validated before analysis and again before returning a result, including hidden and unselected dependencies.

The reference is the specified patch at the first active layer frame. Coordinates refer to the **full source canvas before effects, masks and transforms**. Trimmed PNG offsets restore pixels into that canvas; absent pixels have zero associated luminance. Frame anchors, layer placement, crop, grading and spatial transforms do not move the measurement region. They affect the subsequently rendered mask in the usual way. Source selection uses the same exact rational holds, loop/hold-last rules and 25 fps sample-start clock as scene compilation. A transparent source ending within the tracked interval rejects the trajectory.

| Input | Bound and interpretation |
| --- | --- |
| Layer | Image frames; 2–128 active output frames. Generated graphics are unsupported as tracking evidence. |
| `region` | Initially inside the source canvas; width and height each 4–64 pixels. |
| `search_radius` | 1–32 pixels around the preceding accepted position, clipped to whole patches inside the source canvas. |
| `maximum_step` | 1–search radius; Euclidean distance between consecutive output samples. |
| `maximum_acceleration` | 1–64 pixels; Euclidean change in step after the first measured step. |
| `minimum_correlation_milli` | 850–1000; minimum normalized correlation against the fixed initial template. |
| `minimum_margin_milli` | 20–1000; minimum difference from every other candidate, including adjacent positions. |
| `maximum_frame_change_milli` | 1–1000; maximum mean absolute associated-luminance difference over the whole canvas, divided by 255. |
| Work | At most 64 million patch-pixel comparisons, conservatively counted before analysis. |
| `mask` | Existing rectangle/inversion/feather declaration, with no initial animation. The returned x/y curves follow measured displacement; size remains fixed. |

Matching uses alpha-associated encoded luminance, with RGB weights 54/256, 183/256 and 19/256, rounded to one byte. Straight input is associated before rounding; explicitly premultiplied input keeps its stored associated values. Fully transparent RGB cannot influence motion. Both the reference and a candidate require at least five encoded levels of standard deviation. Original integer sums calculate zero-mean normalized correlation; a floating-point square root completes the score. Tied or insufficiently separated peaks reject.

The original reference patch stays fixed throughout the run. This avoids adapting the template to an occluder, at the cost of rejecting substantial appearance changes. Every accepted observation reports exact global/layer time, selected source-frame index, patch rectangle, displacement, correlation, uniqueness margin and whole-frame change. Returned mask keys use exact layer-local times and hold interpolation at every sampled frame. Both curves remain ordinary editable scene properties. The response includes the content identities and `applied: false`.

`TRACKING_UNRELIABLE` identifies the first failed layer frame. There is no partial trajectory, automatic relock, gap filling or fabricated confidence. `INVALID_TRACKING` rejects invalid settings, `LIMIT_EXCEEDED` rejects excessive work, and existing scene/media errors remain explicit. The returned scene is an explicit replacement value; a caller may choose to replace an existing layer mask by submitting the requested new static mask.

## Feathering

Every layer rectangle now accepts an optional field:

```json
"feather": {"radius": 3, "edge": "centered"}
```

Omission retains the original hard half-open rectangle exactly. Radius is 1–4096 **source pixels**, before scaling or rotation. Feather edge is `inner`, `centered` or `outer`. At each source pixel center, let `d` be the minimum signed distance to the four rectangle edges and `r` the radius:

| Edge | Non-inverted coverage |
| --- | --- |
| `inner` | clamp(d/r, 0, 1) |
| `centered` | clamp((1+d/r)/2, 0, 1) |
| `outer` | clamp(1+d/r, 0, 1) |

Outside corners remain square, using the minimum signed edge distance (Chebyshev distance outside). This is a declared rectangular ramp, not a Gaussian blur or rounded-distance field. Coverage rounds once to a 1/65536 grid, nearest with positive half ties upward; inversion then takes the exact complement. Zero width or height is empty before inversion, regardless of radius. Oversized feathers can leave no fully opaque interior. Feathering never creates pixels outside the actual source image or crop.

Mask coverage multiplies associated source color and alpha without an intermediate RGBA8 round. The integer renderer retains that precision until the final RGB blend. Spatial sampling applies coverage independently to every source tap before premultiplied bilinear interpolation. Normal/multiply/screen blending and layer opacity retain their existing order. Effects keep their own documented quantization; feathering does not unpremultiply or rewrite their source pixels. Inspection reports sampled rectangles and the static `mask_feather` declaration. Effect correction/key masks remain separate controls with their existing semantics.

## Reproducible workflow

Generate the original fixture with `python -X utf8 tests/tracking.py --output <new-external-directory>`. Its `tracked-moving-request.json` is a complete runnable request, with bound synthetic PNG identities and explicit roots. For example, in PowerShell, after building the executable:

```powershell
$fixture = 'C:\DEV\CutboltData\tracking-20261003-02'
$request = Get-Content -Raw -LiteralPath "$fixture\tracked-moving-request.json"
$response = $request | .\target\debug\cutbolt.exe | ConvertFrom-Json
if (-not $response.ok) { throw $response.error.message }
$render = @{
    command = 'scene.render'
    scene = $response.result.scene
    input_root = "$fixture\sources"
    output_root = "$fixture\renders"
    output = "$fixture\renders\documented-tracking.mkv"
}
$render | ConvertTo-Json -Depth 100 -Compress | .\target\debug\cutbolt.exe
```

Use a new output name each time. `scene.render` remains CLI/library-only; MCP exposes `cutbolt_tracking_inspect` and `cutbolt_scene_inspect`. The result is an opaque reference RGB8/PCM asset under the existing scene limits. Save the editable returned recipe separately when future trajectory changes are needed.

## Scope and acceptance

The motion model is **integer translation of a fixed-size patch**. It does not estimate subpixel motion, scale, rotation, perspective, optical flow, subject identity or occlusion recovery. Confidence is evidence of patch similarity within the declared search, not proof of object identity. A similar object outside the search or an indistinguishable cut cannot be ruled out. Inspect the observations and output for the intended shot. Stabilization, subject-aware reframing and segmentation retain separate acceptance gates.

The original fixture checks authored trajectories, exact clocks, brightness changes, partial occlusion, canvas boundaries, trims, changing anchors and source-space invariance under effects/transforms. It renders the returned masks against independent Fraction coverage/blend equations and high-precision geometry. Feather cases cover all three profiles, inversion, every supported blend, exact straight/premultiplied equivalence, alpha zero/one, arbitrary rotations, integer quarter turns, animated empty masks, extreme radii/coordinates and generated graphics. Lost texture, repeated patterns, duplicate subjects, heavy occlusion, jumps, acceleration violations, cuts and unsupported rotation reject. MCP schemas, templates, saved retries/undo, frame/range previews, malformed settings, identities, no-overwrite and post-encode source changes are included. This evidence targets **V05 extended only**.

The retained `C:\DEV\CutboltData\tracking-20261003-02` fixture compares **486 decoded frames, 933,120 stereo sample frames and five previews**, with **38 rejection/failure cases**. Every observed RGB difference was zero; the declared geometric/color tolerance is at most one unit and equivalent straight/premultiplied encodings must match exactly. All silent PCM samples are exact. The full verifier reruns these six evidence groups against the frozen source fingerprint before awarding coverage.

A separate retained maximum-length check drives 128 mask keys across repeated source-loop boundaries and renders 132 frames including inactive ends. Its 253,440 stereo sample frames and all pixels match the independently verified four-frame cycle repeated 32 times; source hashes are unchanged. This supplements the six full-verifier groups and does not add a checkpoint. The documented command sequence also compiled a fresh 20-frame output successfully.

A separate mean-centered-vector calculation checked correlation and all-candidate uniqueness margins for 80 observations across original, brightness-shifted, partly occluded and both alpha encodings. Maximum score difference was below 2.1e-15. Its retained report is `confidence-verification.json` beside the fixture. This is a numerical cross-check, not a calibrated probability of object identity.
