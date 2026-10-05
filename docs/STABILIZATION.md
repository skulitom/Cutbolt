# Measured camera stabilization

`stabilization.inspect` returns measured camera motion, an editable correction and a replacement scene. It is read-only through CLI, the library and MCP. Explicitly compile the returned scene with `scene.render`, then register the new asset through ordinary saved-session operations. The operation uses original local image processing, with no external model or runtime download.

The supported models are translation and rigid planar motion (translation plus camera roll). Stabilization measures several declared stationary background patches, fits their common motion, and compensates the source image before the authored layer transform. It can lock a shot or smooth its path while retaining slower intentional movement. Declared cuts start new measurement and smoothing segments.

## Request and bounds

The request contains `scene`, absolute `input_root`, `layer_id`, `model`, `segments`, `tracking`, `maximum_fit_error_milli`, `maximum_roll_mdeg`, `smoothing`, `strength_milli`, `crop` and `sampling`. Inputs retain the existing scene identity, exact timing, decoding and source-root rules. Both the complete original scene and the returned scene are validated; unselected and hidden dependencies are included.

| Field | Contract |
| --- | --- |
| `model` | `translation` or `rigid`. |
| Tracked layer | 2–128 active output frames at 25 fps, using image frames. Generated graphics and already-compensated inputs reject. |
| `segments` | 1–16 entries, each `{start, regions}`. Starts are increasing, frame-aligned layer-local rational times; the first is zero. Each segment contains at least two frames. |
| `regions` | 3–8 patches per segment, each `[x,y,width,height]` in the full source canvas. Dimensions are 4–64 pixels and the initial patches must fit the canvas. Their centers must form a triangle of at least 64 square pixels. |
| `tracking.search_radius` | 1–32 pixels around the preceding accepted integer location. |
| `tracking.maximum_step` | 1–search radius, measured as Euclidean displacement between consecutive output samples. |
| `tracking.maximum_acceleration` | 1–64 pixels, bounding the Euclidean change in step. |
| `tracking.minimum_correlation_milli` | 850–1000. |
| `tracking.minimum_margin_milli` | 20–1000. |
| `tracking.maximum_frame_change_milli` | 1–1000, limiting normalized mean absolute change over the measurement canvas. |
| `maximum_fit_error_milli` | 1–2000; maximum RMS patch-center fit error in millipixels. All patches participate; outliers are rejected rather than silently discarded. |
| `maximum_roll_mdeg` | 0–15000, bounding measured roll relative to each segment's initial image. |
| `smoothing` | `{mode: "lock"}` or `{mode: "smooth", radius: 1..32}`. |
| `strength_milli` | 0–1000; zero leaves the measured motion unchanged, 1000 applies the selected target completely. |
| `crop` | `{mode: "preserve"}` or `{mode: "zoom", maximum_zoom_milli: 1000..4000}`. |
| `sampling` | `nearest` or `bilinear`, explicitly selected for the returned spatial transform. |
| Work | At most 128 million conservative patch-pixel comparisons overall and 64 million for any individual patch run. |

Choose patches on a common rigid background, distributed across the shot. A moving foreground patch, parallax, insufficient texture or inconsistent matches can fail the motion fit. The roll limit is a rejection bound, not a guarantee that every appearance change within that angle will match. The scene retains its existing maximum duration, canvas and media limits; source images follow the scene's PNG limits, up to 4096 pixels per axis within its 64-million decoded-pixel budget.

## Measurement and confidence

The original full-canvas image is restored from each trimmed PNG and its offset. Exact source holds, loops and sample-start selection determine which image is used at each output frame. Anchors, layer masks, effects and authored transforms do not affect the measurement coordinates. A transparent source ending inside the interval rejects. Each cut segment uses its first selected image as a new reference; smoothing never crosses the declared boundary.

The measurement backend forms alpha-associated encoded luminance using the existing RGB weights 54/256, 183/256 and 19/256. A symmetric 3×3 binomial filter with weights `[1,2,1]` on each axis reduces sampling-phase artifacts. Its sum divides by 16 with half-up byte rounding, and canvas boundaries clamp. This filter affects measurement only. The renderer always reads the original source pixels.

An integer normalized-correlation search locates candidate peaks. In descending score order, integer candidates within one pixel in both axes of an already retained peak are grouped into that peak. Each distinct peak is refined on a quarter-pixel grid covering ±0.5 pixel around its integer position, using bilinear samples. The reported margin is the best refined score minus the next distinct refined peak's score. Scores and candidates require at least five encoded luminance levels of standard deviation after measurement filtering. Flat, dim, ambiguous or substantially changed evidence rejects rather than producing a partial correction. Confidence denotes local patch similarity; it is not a calibrated probability of object identity.

The conservative work bound counts 26 comparisons per integer candidate: the initial comparison plus up to 25 refinement positions. The existing `tracking.inspect` mask tool retains its original integer-only matching and all-other-integer-candidate margin; its public behavior is unchanged.

Translation fitting uses the mean patch displacement. Rigid fitting centers the reference/current patch centers, obtains the least-squares rotation from their dot/cross sums, and solves translation at the crop center. Translation is quantized to millipixels and angle to millidegrees. Residuals are checked against all measured centers after quantization. Every measurement reports exact global/layer time, the original source-frame index, segment, measured/target/correction poses, fit error and minimum patch confidence/margin. The result also retains analysis settings, source identities and `applied: false`.

## Smoothing and editable correction

`lock` targets the initial pose of each segment. `smooth` averages the measured translation and roll with triangular weights `radius + 1 - distance`. At a segment boundary the window is truncated and renormalized. This can soften the start/end of an intentional pan; the endpoint policy is explicit. Each target coordinate then blends the measured and smoothed coordinates using `strength_milli/1000`. Weighted values round to integers with half ties away from zero. Timeline/key times remain exact rationals.

For measured pose `F` and desired pose `D`, the correction is `D × inverse(F)`, with translation and rotation about the crop center. Correction translations round to millipixels. The returned `transform.spatial.compensation` contains:

- `center_milli`: the fixed center in full source-canvas millipixels.
- `translation_x_milli`, `translation_y_milli`, `rotation_mdeg`: ordinary editable curves with a hold key at every active frame.
- `zoom_milli`: one constant zoom for the whole tracked layer.
- `viewport`: the original crop rectangle in compensated source-canvas coordinates.

The normal curve/easing/retiming rules also apply when a caller edits these curves. Static center is bounded to ±32768 pixels, translation curves to the same range, angle curves to ±3600 degrees, zoom to 1–4×, viewport positions to ±32768 and viewport sizes to 1–32768. These are renderer bounds; the measurement command has the narrower motion-confidence bounds above. Edited curves must be inspected again, and the original measurement/crop report no longer certifies manually changed values.

If the authored mapping is `M`, correction is `C` and zoom about the same center is `Z`, the final source mapping is `M × Z × C`. The separate compensated viewport clips before `M`. Nonuniform scale, mirroring, aspect fitting, rotation, changing anchors and authored animation retain this order. Input crop, source masks and effects keep their source coordinates. Alpha is preserved through the existing premultiplied spatial sampling and final blend. The returned recipe uses the requested sampler and transparent source edges; it does not stretch border pixels to conceal missing image data. Inspection reports correction samples and both the final and pre-correction inverse matrices.

## Crop and quality tradeoffs

`preserve` keeps zoom at 1×. Stabilization can then reveal the backdrop at moving source edges. The report states whether the source footprint fills the requested viewport.

`zoom` selects the smallest whole-millizoom, shared by all frames and segments, that keeps the compensated viewport within every selected image/crop intersection. Bilinear sampling reserves an additional half-source-pixel guard; nearest uses no extra guard. The test uses the four viewport corners through the inverse correction, giving a conservative full-rectangle footprint check. Trimmed image offsets and dimensions participate. A declared maximum below the required zoom fails with `STABILIZATION_CROP_LIMIT`; there is no fallback to stretched edges or an undeclared crop.

The report includes selected zoom, minimum border-free zoom (null if none up to 4×), footprint coverage, guard size and retained area fraction `1/zoom²`. A constant zoom avoids per-frame pumping but sacrifices more source area to accommodate the worst frame. This is a geometric source-footprint statement: natural transparency, masks, opacity, effects, destination clipping and authored placement can still reveal the backdrop. Stabilization does not fill alpha holes or invent pixels. Larger zoom and repeated interpolation can reduce detail; the fixture compares actual output with an independently sampled ideal image as well as measuring residual motion.

## Reproducible workflow

Run `python -X utf8 tests/stabilization.py --output <new-external-directory>` to generate original known-motion fixtures. `locked-roll-request.json` is a complete request with relative bound PNG identities and explicit input root. After building the executable, the retained fixture can be used as follows:

```powershell
$fixture = 'C:\DEV\CutboltData\stabilization-20261003-06'
$request = Get-Content -Raw -LiteralPath "$fixture\locked-roll-request.json"
$response = $request | .\target\debug\cutbolt.exe | ConvertFrom-Json
if (-not $response.ok) { throw $response.error.message }
$render = @{
    command = 'scene.render'
    scene = $response.result.scene
    input_root = "$fixture\sources"
    output_root = "$fixture\renders"
    output = "$fixture\renders\documented-stabilization.mkv"
}
$render | ConvertTo-Json -Depth 100 -Compress | .\target\debug\cutbolt.exe
```

Use an unused output name. MCP exposes `cutbolt_stabilization_inspect` and `cutbolt_scene_inspect`; scene compilation remains CLI/library-only. Retain the returned editable recipe separately from its compiled reference asset. Compiled assets use the existing timeline, saved retry/undo, preview and export operations. Source identities are rechecked after analysis and before render publication; failed analysis returns no partial result and rendering never overwrites an existing file.

## Acceptance evidence and limits

The retained `C:\DEV\CutboltData\stabilization-20261003-06` fixture compares **248 decoded frames, 476,160 stereo sample frames and five previews**, with **31 rejection/failure cases**. It renders translation lock, camera roll up to ±4.8 degrees, smoothed pan, partial/zero strength, rational source holds, declared cuts, revealed edges versus constant zoom, trimmed sources, animated/nonuniform/mirrored authored transforms, masks and equivalent straight/premultiplied inputs. Nonrigid patch motion, repeated/flat evidence, undeclared cuts, excessive roll/residuals, invalid fields, work budgets, source changes and existing outputs reject. Typed MCP schemas, template preservation and saved retries/undo/range previews are included.

Independent source-plane geometry and rational target smoothing measure residual motion; an independently sampled continuous-world image measures output quality. A separate 60-digit geometry/Fraction reference verifies complete rendered pixels and alpha filtering. Integer translation lock has zero residual displacement. Roll, pan and cut cases require at most **0.25 source pixel RMS**, more than **4× motion reduction**, and mean absolute ideal-image error at most **8 RGB units**. The retained maximum residual is **0.0772 pixel RMS**; ideal-image mean error is at most **5.967 units**. Complete renderer-reference differences were zero (declared arbitrary-geometry tolerance: one RGB unit). All PCM samples are exact silence, and equivalent alpha encodings match exactly.

This evidence targets **V06 basic and extended**: actual stabilized handheld-style output, declared scene cuts, camera roll and crop-quality tradeoffs. It does not implement row-dependent rolling-shutter repair, projective motion, 3D parallax correction, scale/zoom estimation, automatic cut recovery or subject-aware reframing. Those behaviors must not be inferred from the rigid planar model. No new dependency, service, copied implementation or acceptance denominator is introduced.

Additional retained checks render a 128-key source loop: all **128 active frames** are identical after exact cancellation of alternating camera translation, with **132 total frames and 253,440 exact stereo sample frames** including inactive boundaries. A separate original stereo soundtrack test preserves its start at sample 1920, all **30,720 output sample frames**, its source hash and the original stabilized video pixels. The documented command also compiled a new 16-frame output. These checks supplement the seven full-verifier groups without adding points.

### Measured fixture tradeoffs

Residuals are measured in source pixels relative to the declared target path, before display zoom and authored transforms. Retained area is the geometric 1/zoom² fraction.

| Fixture | Input RMS | Residual RMS | Zoom | Retained area |
| --- | ---: | ---: | ---: | ---: |
| Translation lock | 1.8028 | 0.0000 | 1.117× | 80.1% |
| Camera-roll lock | 1.9075 | 0.0771 | 1.171× | 72.9% |
| Roll up to 4.8 degrees | 2.1916 | 0.0658 | 1.228× | 66.3% |
| Smoothed intentional pan | 1.6068 | 0.0501 | 1.132× | 78.0% |
| Two declared shot segments | 1.5034 | 0.0342 | 1.114× | 80.6% |
