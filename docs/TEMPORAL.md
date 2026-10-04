# Shutter sampling and temporal integration

Scenes support explicit bounded temporal integration. Both X02 checkpoints pass full verification. Existing scenes omit `temporal` and retain their original frame-start sampling.

## Exposure contract

Add this optional scene field to render eight evenly weighted midpoint samples through a full-frame exposure centered on the output frame's timestamp:

```json
"temporal": {
  "shutter_angle": {"num":360,"den":1},
  "phase": {"num":-180,"den":1},
  "samples": 8,
  "integration": "encoded_rgb"
}
```

Angles and phase are exact signed rationals in degrees. Angle must be 0..360, phase −360..360, and sample count 1..32. Zero angle requires one sample. At 25 fps, sample `k` of output frame `n` is:

`t = n/25 + (phase + shutter_angle * (2*k+1)/(2*samples)) / 9000`.

Phase marks the shutter's opening offset relative to the frame timestamp. A 180-degree shutter with phase −90 is centered on that timestamp; phase zero opens there. A zero-degree shutter is an instantaneous sample at the phase offset. Zero angle, zero phase and one sample reproduce the legacy frame-start result.

Sample times outside the scene's half-open `[0,duration)` interval contribute the declared background. They do not extend the first or last image or evaluate property expressions. Inside the scene, each layer's half-open active interval, source holds and end policy are evaluated at the exact sample time. Audio is rendered once on its original sample clock and is unaffected by shutter settings.

## Composition and effects

Each sample is a complete composited encoded-sRGB RGB image. The output is the equal-weight channel average, rounded to nearest, with exact half ties upward. This explicit encoded-color approximation does not claim a physically linear-light exposure. `linear_rgb` and other integration modes reject instead of silently using encoded values.

Position/opacity curves, typed expressions, spatial curves, rectangular masks and supported effect parameters use their existing exact local clocks at each subframe sample. Existing quantization still applies: integer position/opacity/mask values round as before, spatial translations/scales use thousandths and rotation uses millidegrees. Shutter sampling does not increase parameter precision. A rounded step or held source image can remain visibly discontinuous at low sample counts.

Source PNG frames remain piecewise held images; there is no optical-flow interpolation or inference of intermediate source images. Static text/shape rasterization is reused. Grade, selective grade and chroma-key effects are stateless and evaluated independently per sample, before complete sample composition and exposure averaging. History-based filters, temporal denoising, image morphing and optical-flow effects are unsupported. Unknown effect kinds, temporal fields or integration modes produce explicit schema errors; inspection also reports the supported effect sampling and limitations.

## Inspection, compilation and editing

Use `scene.inspect` to validate sources and inspect exact exposure times before rendering. Its `temporal` report includes signed `sample_times`, in-range `active_sample_times` (null outside the scene), sample count, work estimate and limitations. Arrays are frame-major, then increasing shutter time. With temporal enabled, each layer's `selected_frames` and `sampled_parameters`, and expression `frame_bindings`, follow this same flattened layout. Outside-scene expression entries are empty arrays. Output `frames` and audio `samples` retain their original meaning.

`expression.inspect` remains a read-only graph sampler at caller-selected scene times. It does not validate or apply the optional shutter configuration. Its 256-time request limit is distinct from the aggregate scene-rendering limit.

`scene.render` compiles a new lossless asset through the existing root, source-identity, collision and atomic-publication checks. Keep the original scene recipe for later shutter edits. Compiled results support ordinary timeline cuts, saved retries/undo, and exact frame/range previews; shutter integration is already included in those frames.

## Work and precision limits

The scene profile still permits at most 250 output frames and 16 layers; its canvas and output limits are listed in [SCENES.md](SCENES.md#limits). Temporal evaluation additionally limits:

- 32 samples per output frame;
- 67,108,864 `width * height * layers * frames * samples` visits;
- 32,768 `layers * frames * samples` parameter records;
- 8,000 scene sample slots and 65,536 total node evaluations when expressions are present.

Work limits count all slots conservatively, including inactive layers and out-of-scene times, and reject before media reads. Images and fonts are loaded once. Exposure accumulation holds one RGB sample plus an integer accumulator instead of all rendered subframes. The prepared parameter report is bounded separately by the record limit.

Scalar calculations use the same checked exact rational profile as [expressions](EXPRESSIONS.md). Intermediate reduced numerator magnitude is at most 9,007,199,254,740,991; reduced denominator is at most 1,000,000,000,000. Invalid shutter ranges use `INVALID_TEMPORAL`; work limits use `LIMIT_EXCEEDED`. Zero denominators and excess rational precision use the shared `EXPRESSION_DOMAIN` and `EXPRESSION_PRECISION` errors. There is no floating-point time approximation or automatic reduction of sample count.

## Acceptance

`tests/temporal.py` compares complete decoded pixels and nonzero PCM with independent Fraction sample enumeration, high-precision forward geometry and point-effect references. A moving box also has an independent analytic continuous-exposure solution. Tests compare 1, 2, 4, 8, 16 and 32 samples, enforce a predetermined quantization-error bound and require exact analytic agreement for that fixture at eight or more samples. They cover fractional angles/phases, scene/layer/held-image boundaries, mask/effect curves, graph links, reordered keys, legacy equivalence, saved edits, retries, undo, previews and source/output preservation.

The maximum raster fixture actually renders 67,108,864 layer-pixel sample visits; rendering plus complete RGB/PCM decoding must finish within 90 seconds. Another fixture exercises all 32,768 parameter records. The integrated run passes these fixed gates and all seven temporal checks, awarding both original X02 checkpoints.

The retained focused run passes 284 complete frames, 545,280 stereo sample frames, four previews and 17 rejection checks with exact RGB/PCM agreement. The continuous moving-box reference has zero channel error at 8, 16 and 32 samples; 2 and 4 samples have a maximum 31-level approximation error, while one sample reaches 217 at the scene boundary. These values describe this authored fixture, not a general convergence guarantee. Maximum raster rendering and complete decoding take 10.142 seconds within the 90-second gate. A prior test incorrectly called a CLI-only range-preview command through MCP; that test call was corrected, and the full focused fixture was rerun successfully.
