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

Angles and phase are exact signed rationals in degrees. Angle must be 0..360, phase −360..360, and sample count 1..32. Zero angle requires one sample. At frame rate `R`, sample `k` of output frame `n` is:

`t = n/R + (phase + shutter_angle * (2*k+1)/(2*samples)) / (360*R)`, which at 25 fps divides by 9000.

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

The scene profile permits up to 120 seconds and 64 layers; its canvas, output and compositing limits are listed in [SCENES.md](SCENES.md#limits). Shutter sampling is bounded by:

- 32 samples per output frame;
- the scene's 64-billion composited-pixel budget, which counts every changing layer at every sample and adds the whole scene once per sample for averaging. Unchanging bottom layers count once, as without sampling;
- 460,800 `layers * frames * samples` parameter records, as many as 64 layers keep over the longest scene without sampling (7,200 frames at 60 fps);
- with expressions, 1,843,200 node evaluations and 230,400 bound values over all samples ([EXPRESSIONS.md](EXPRESSIONS.md#work-limits-and-diagnostics)).

These limits count all samples, including inactive layers and out-of-scene times, and reject before media reads. Images and fonts are loaded once. Each frame reuses one RGB sample buffer and 16-bit sums (32 samples of 255 fit exactly), and a render runs fewer workers when needed to keep that scratch within about 512 MiB.

Records cost memory mainly through the receipt. When every layer value changes at every sample, 460,800 records peaked at 1.3 GB while preparing and returned a 27 MB receipt, the same as 64 layers changing every frame for two minutes at 60 fps without sampling. Values that hold still are run-length encoded.

Measured on the release build of the development machine (32 logical processors), with other sessions keeping it 45-92 % busy: a 1920 x 1080 scene of 250 frames has an unchanging background and one moving 512-pixel sprite. With 32 samples per frame it rendered in 9.6-10.0 s, using 42-69 s of engine CPU and peaking at 426-543 MB in the engine. Before the sample buffer was reused and the sums narrowed, it took 15.3-16.1 s, 132-141 s and 955-972 MB (runs alternated). A 1080p sample now costs about 5-9 ms of CPU, roughly one full-frame layer. At 1080p the budget therefore allows, for example, 8 samples per frame for 60 seconds or 4 for two minutes, each with a dozen moving 512-pixel sprites over an unchanging background. That 60-second scene (62.6 billion pixels) rendered in 47 s, peaking at 525 MB in the engine and 1.05 GB with FFmpeg.

Scalar calculations use the same checked exact rational profile as [expressions](EXPRESSIONS.md). Intermediate reduced numerator magnitude is at most 9,007,199,254,740,991; reduced denominator is at most 1,000,000,000,000. Invalid shutter ranges use `INVALID_TEMPORAL`; work limits use `LIMIT_EXCEEDED`. Zero denominators and excess rational precision use the shared `EXPRESSION_DOMAIN` and `EXPRESSION_PRECISION` errors. There is no floating-point time approximation or automatic reduction of sample count.

## Acceptance

`tests/temporal.py` compares complete decoded pixels and nonzero PCM with independent Fraction sample enumeration, high-precision forward geometry and point-effect references. A moving box also has an independent analytic continuous-exposure solution. Tests compare 1, 2, 4, 8, 16 and 32 samples, enforce a predetermined quantization-error bound and require exact analytic agreement for that fixture at eight or more samples. They cover fractional angles/phases, scene/layer/held-image boundaries, mask/effect curves, graph links, reordered keys, legacy equivalence, saved edits, retries, undo, previews and source/output preservation.

A fixed heavy raster fixture renders 256 samples of a 512 x 512 scene: its unchanging layer once and 67,108,864 averaged pixels, which was the maximum before 5 October 2026. Rendering plus complete RGB/PCM decoding must finish within 90 seconds. Inspection accepts 249 frames of a 4000 x 2000 scene at 32 samples within the 64-billion budget and rejects 250. Another fixture renders all 460,800 parameter records and rejects 462,848. The integrated run passes these fixed gates and all seven temporal checks, awarding both original X02 checkpoints; the record and budget checks changed with the limits on 5 October 2026.

The retained focused run passes 284 complete frames, 545,280 stereo sample frames, four previews and 17 rejection checks with exact RGB/PCM agreement. The continuous moving-box reference has zero channel error at 8, 16 and 32 samples; 2 and 4 samples have a maximum 31-level approximation error, while one sample reaches 217 at the scene boundary. These values describe this authored fixture, not a general convergence guarantee. In that run, the former maximum raster render and complete decoding took 10.142 seconds within the 90-second gate. A prior test incorrectly called a CLI-only range-preview command through MCP; that test call was corrected, and the full focused fixture was rerun successfully.
