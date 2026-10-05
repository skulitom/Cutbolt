# Pixel scenes and timeline previews (v0.4)

Each image frame may additionally provide a content-bound binary `matte` PNG. Its black pixels clear source RGBA before effects and transforms, while white preserves it. The matte follows the same source hold and uses the existing source/derived-pixel budgets; see [foreground masks](SEGMENTATION.md).

Optional [3D plane geometry](GEOMETRY.md) projects source layers with cameras, depth and declared lighting. Geometry replaces neutral 2D placement; opacity, masks, effects, source timing and shutter samples remain supported.

Scenes may carry an optional [typed property graph](EXPRESSIONS.md). Position/opacity bindings replace sampled base values before spatial mapping; media timing, soundtracks and the scene's frame clock remain governed by this contract.

Optional [shutter sampling](TEMPORAL.md) evaluates bounded exact subframe times and averages complete RGB samples, with explicit phase, scene-boundary, source-hold and effect limitations. Output frame rate and audio clocks remain unchanged.

Cutbolt can compile an editable, bounded scene with PNG animation, text/shapes and a separate WAV or audio mix into its existing lossless FFV1/PCM asset profile. The returned `asset` can be added with `media.add` and edited through ordinary saved sessions. Keep the scene JSON and original external image/audio/font files: the compiled asset is flattened, and scene-layer edits currently require explicitly recompiling to a new output. Scene recipes are separate from version-1 timeline snapshots and the planned production manifest.

## Commands

For repeated titles and graphics layouts, [typed templates](TEMPLATES.md) provide `graphics.instantiate`. It returns a validated, editable scene for the commands below, with explicit parameter bindings and independent instances.

For subtitles, [caption documents](CAPTIONS.md) provide `captions.scene`. It appends sampled text layers to a base scene, preserving exact native cue times and reporting unsampled/outside cues. Supply explicit font/layout maps and keep the resulting scene within the existing duration and total-layer limits.

| Command | Fields besides `command` | Result |
| --- | --- | --- |
| `scene.inspect` | `scene`, `input_root` | Validate image/audio/font identities, decode media, report exact selected animation frames, text layout and audio conversion counts; no output |
| `scene.still` | `scene`, `input_root`, `output_root`, `output`, optional `time` | Render the one frame shown at `time` (default 0) to a new `.png` at output size, RGB or straight RGBA for a transparent scene, without compiling; over MCP also an inline image |
| `scene.render` | `scene`, `input_root`, `output_root`, `output` | Compile and verify a new `.mkv`; return source identities, scene digest, tool versions, counts and a timeline asset |
| `preview.frame` | `project`, `input_root`, `output_root`, `output`, `time` | Export one timeline frame as a new full-size `.png` with its source/timeline frame numbers |
| `preview.range` | `project`, `input_root`, `output_root`, `output`, `start`, `duration` | Export an exact half-open timeline range as a new reference `.mkv`, including cuts and sample-exact audio |

All roots and outputs must be absolute existing local directories/paths; the output's parent must exist. Source image/WAV identities contain a **relative** path made of normal components, lowercase SHA-256 and byte count. Traversal, absolute asset paths, alternate data streams and resolved paths outside the input root fail. Sources are read into bounded buffers and checked against identities; scene rendering checks them again before publishing. Existing files are never overwritten. Publication uses a hard link to a verified temporary output and requires filesystem support, as for normal exports.

`scene.render` and `preview.range` are blocking CLI/library commands. They are deliberately absent from the MCP catalog. They do not yet participate in persisted render jobs, cancellation, automatic retry or generation-stage recovery. MCP exposes `cutbolt_scene_inspect` and `cutbolt_preview_frame`; full reference-project exports continue to use `render.start`. A caller retrying a scene render after a lost result must inspect any output and its receipt rather than overwrite it. A forced process exit can leave a `.cutbolt-scene-*` scratch directory; normal errors clean up only the owned files.

[Motion tracking](TRACKING.md) adds read-only `tracking.inspect` for confidence-bearing source-patch measurements and an explicit replacement masked scene recipe. Compile the returned recipe separately to apply it.

[Camera stabilization](STABILIZATION.md) adds read-only `stabilization.inspect` and editable `transform.spatial.compensation`. Measurement returns a new recipe; compensation and constant zoom compose before the authored spatial mapping, with an explicit compensated viewport.

## Scene schema and behavior

The native schema is derived from `scene::Scene` and is discoverable through the `scene.inspect` MCP input schema. Unknown fields fail. A scene contains:

- `schema_version: 1`, a nonblank `id`, `width`, `height`, `output_scale`, exact rational `duration`, opaque RGB `background`, `color: "srgb_straight_encoded"`, ordered `layers` and nullable `audio`.
- Each layer: unique `id`, full source `canvas: [width,height]`, exact `start` and `duration`, ordered `frames`, `timing`, `end` and `transform`. Later layers composite over earlier layers.
- A layer may instead have `graphics` and empty `frames`, with strict timing, hold-last ending and straight alpha. This supports [text with supplied fonts, rectangles and ellipses](GRAPHICS.md), including existing position/opacity and mask animation. A layer chooses exactly one of `frames`, `graphics` or `tilemap`.
- A `tilemap` layer (also with empty `frames`, strict timing and hold-last ending) assembles its source canvas from a grid: `tile_size: [w,h]`, up to 256 `tiles` (each with its own ordered `frames`, `timing` and `end`, like a layer) and `cells`, rows top to bottom whose entries index `tiles` or are `null` for transparent. The canvas must equal `tile_size` times the grid, with at most 4,096 cells. Every tile frame must fit the tile size. Each tile animates on its own clock from the layer start; repeated indices share pixels without extra decoding. Straight RGBA tiles are copied into the canvas before masks, effects and transforms. 3D geometry scenes, reframing, tracking and stabilization reject tilemap layers explicitly. Reports include the grid, tile size, tile count, occupied cells and every tile's selected frame per sample.
- Each frame: `image: {path,sha256,bytes}`, exact `hold`, `offset: [x,y]` restoring a trimmed image inside the declared source canvas, and `anchor: [x,y]` in full-canvas pixel-corner coordinates. Repeated references are preserved, including distinct holds/anchors.
- Each transform: `crop: [x,y,width,height]` inside the full source canvas, clockwise `quarter_turns: 0..3`, integer nearest-neighbor `scale: 1..16`, destination anchor `position: [x,y]`, and `opacity: 0..255`.
- An optional `transform.spatial` adds [interpolated/animated spatial mapping](SPATIAL.md), with subpixel translation, independent axis scales, rotation, mirroring, declared pixel aspect and contain/cover/stretch fitting. Omission preserves the existing integer path.
- A layer may also declare `animation` with any of `position_x`, `position_y` and `opacity` curves. Omitted properties keep the static transform value. Existing recipes without animation retain their behavior.
- Optional layer fields `alpha_mode: "straight" | "premultiplied"` and `blend_mode: "normal" | "multiply" | "screen"` default to straight and normal. An optional `mask` limits the layer's opacity using a static or keyframed rectangle in source-canvas coordinates.
- An optional `effects` list provides up to eight ordered effects in total, including [primary grades](GRADING.md), with exposure, contrast, RGB white balance, master/channel curves and five animated controls. Grades process source colors in linear sRGB and return straight encoded 8-bit RGB before layer compositing; alpha is preserved.
- [Chroma keys](KEYING.md) reduce alpha with hard/soft color thresholds, optional screen subtraction/spill suppression, feathered masks and animated strength. `effects.preset` returns original editable green/blue recipes. All effects share the layer order and source-canvas coordinates.
- The same list supports [selective grades](SELECTIVE_COLOR.md) using HSL qualifiers, feathered/inverted correction masks and animated mix/mask controls. The eight-effect limit covers all three types together. Correction masks change grading strength; the ordinary layer mask still controls opacity independently.
- Audio: `file` identity, exact `start`, `channels: "duplicate_mono" | "preserve_stereo"`, `resampling: "linear"`, and `padding: "silence"`. `audio: null` produces silence unless `audio_mix` is present.
- Optional `audio_mix` accepts the [PCM mix recipe](AUDIO.md). Its duration must equal the scene duration and `audio` must be null/omitted. It replaces the legacy single-source soundtrack with sample cuts, envelopes and multiple tracks; old recipes retain their behavior.

The integer path restores the full canvas, applies the optional mask, crops, rotates, scales, then places the transformed anchor at `position`. The optional [spatial path](SPATIAL.md) samples source taps through an explicit inverse map, applying masks/effects before premultiplied interpolation. Output clips at the scene edges. Layer activation intervals are half-open and aligned to 25 fps. The compositor works in **encoded sRGB**, over the required opaque background. Linear-light blending, transparent output and HDR remain unsupported. A separate [delivery export](EXPORT.md) with explicit `input_transfer: "srgb"` converts compiled scene RGB to declared limited-range BT.709 H.264.

PNG inputs must be 8-bit RGB or RGBA, plain or sRGB-tagged. Palette/grayscale/16-bit PNG, APNG, ICC profiles, untagged alternate gamma/chromaticities, orientation and HDR metadata are rejected. Untagged inputs are assigned sRGB by the explicit scene contract. The reference MKV and its PNG previews preserve RGB sample values. The separate delivery profile performs explicit transfer/matrix/range conversion; it does not infer per-asset color spaces or establish project-wide color management.

### Blending and alpha

Ordinary PNG uses straight alpha and needs no `alpha_mode` field. `"premultiplied"` is an explicit opt-in interpretation of numeric RGB payloads already multiplied by alpha and stored in a PNG container; it is not detected from PNG metadata and does not convert straight input automatically. The scene-level `color: "srgb_straight_encoded"` identifier remains the original profile/default; explicit layer alpha modes override its default input interpretation. Each decoded RGB channel must be no greater than alpha, including zero RGB at zero alpha, or inspection/rendering fails with `INVALID_ALPHA`. Validation covers every declared image, even hidden pixels and images reused by layers with different alpha modes. For straight input, hidden RGB at zero alpha is allowed and cannot alter the backdrop.

For normalized source color `s` and backdrop `d`, normal uses `s`, multiply uses `s*d`, and screen uses `1-(1-s)*(1-d)`. These are the public [W3C compositing blend definitions](https://www.w3.org/TR/compositing-1/). The effective coverage is the source alpha after any key effects, times layer opacity and mask coverage; the result is `coverage*blend + (1-coverage)*d`. For pixels bypassed by the effect chain, original integer arithmetic preserves the stored premultiplied values through this calculation and rounds once at the final 8-bit channel, avoiding an intermediate unpremultiply rounding step. Changed effect results follow their documented straight-RGB/alpha quantization before this blend. Zero coverage preserves the backdrop exactly. Compositing follows layer order with rounding after each layer; other blend modes and undeclared effect types remain unsupported.

### Transparent output

`"transparent": true` renders over a transparent backdrop instead of `background`. The result is a straight-alpha FFV1 `bgra` asset that an `alpha_over` [track](TRACKS.md) composites over video, which is how a caption scene or a title becomes an overlay.

Each frame is composed twice with the same layer weights and per-layer rounding:
- a color pass over black, which holds premultiplied color;
- a matte pass in which every source pixel is white with its own alpha once the layer's effects have run, which holds 255 x alpha.

Straight color is then `round(color x 255 / alpha)`, and fully transparent pixels are stored as zero. Opaque pixels and fully transparent pixels are exact. Partly covered pixels follow the usual 8-bit straight-alpha quantization.

Only normal-blend layers are accepted, because multiply and screen depend on a backdrop that does not exist yet. 3D geometry is also rejected. Both fail with `UNSUPPORTED_SCENE`. Shutter sampling averages both passes.

### Rectangular masks

One optional `mask` per layer has `rect: [x,y,width,height]`, `inverted` (default false), optional `feather` and optional `animation` curves for `x`, `y`, `width` and `height`. It masks the layer's opacity before compositing. Coordinates refer to the full source canvas, including a trimmed image's offset, before crop/rotation/scale/placement. Without feathering, the rectangle includes its left/top edge and excludes right/bottom edges. [Feathering](TRACKING.md) declares a source-pixel radius and an inner, centered or outer ramp; coverage preserves associated color/alpha through interpolation and compositing. Inversion keeps pixels outside the rectangle; it never creates pixels outside the source image. Zero width or height gives an empty mask, or reveals the entire source when inverted. Rectangles may extend outside the canvas. Coordinates are bounded to -32768..32768, and dimensions to 0..32768.

For example, these optional layer fields multiply the layer onto its backdrop while progressively revealing a rectangle:

```json
{
  "blend_mode": "multiply",
  "mask": {
    "rect": [4, 3, 0, 5],
    "inverted": false,
    "animation": {
      "width": {
        "keys": [
          {"time": {"num": 0, "den": 1}, "value": 0, "interpolation": "linear"},
          {"time": {"num": 1, "den": 2}, "value": 8, "interpolation": "hold"}
        ]
      }
    }
  }
}
```

Mask curves use the property timing and validation rules below, including the layer-local clock, endpoint holds, integer rounding, 1-128 keys and rational times. Omitted properties retain their rectangle values. Invalid rectangles and empty mask animation objects fail with `INVALID_MASK`; invalid curves fail with `INVALID_ANIMATION` or `INVALID_TIME`. Static inner/centered/outer feather ramps and confidence-bearing translation tracking are described in [TRACKING.md](TRACKING.md). General paths, mask stacks and animated feather radii remain unsupported.

Inspection and render receipts report each layer's `alpha_mode`, `blend_mode` and `mask_inverted` (null when absent). Their output color label is `srgb-opaque-composited-in-encoded-srgb`, replacing the former straight-alpha-only description. Visible `sampled_parameters` include `mask_rect` only for masked layers. `capabilities` and MCP input schemas expose the supported options. Generate a retained independent fixture with `python -X utf8 tests/compositing.py --output C:\DEV\CutboltData\new-compositing-test`. It compares all 465 decoded frames across 31 renders with normalized rational blend equations and forward image/mask transforms, and rejects 11 invalid inputs without publishing outputs or changing sources.

### Timing

#### Property keyframes

Each property curve contains `keys`, each with rational `time`, integer `value` and `interpolation: "hold" | "linear" | "ease_in" | "ease_out" | "ease_in_out"`. Time is local to the layer, independent of the source-frame loop and anchored at the layer's `start`. Before the first key and after the last, its endpoint value is held. Within an interval, the left key selects the interpolation mode; at a key's exact time, that key's value wins. The last key's interpolation field has no following interval.

The easing modes use bounded quadratic weights. For normalized interval progress `u`, ease-in uses `u*u`, ease-out uses `1-(1-u)*(1-u)`, and ease-in-out uses `2*u*u` through the midpoint and `1-2*(1-u)*(1-u)` afterwards. Interpolation mixes the two key values with that weight. These curves do not overshoot their endpoints; they support both increasing and decreasing values. There are no user-defined control handles or spring/elastic modes.

For example, this optional layer field fades opacity from zero to full over half a second:

```json
{
  "animation": {
    "opacity": {
      "keys": [
        {"time": {"num": 0, "den": 1}, "value": 0, "interpolation": "linear"},
        {"time": {"num": 1, "den": 2}, "value": 255, "interpolation": "hold"}
      ]
    }
  }
}
```

Property interpolation uses exact rational arithmetic and rounds the final parameter to the nearest integer, with half ties away from zero. Position remains a pixel coordinate and opacity remains 0-255; this is a bounded pixel-scene animation path. Keys may be off the 25 fps grid, use reduced denominators up to 1,000,000, and must lie in `[0, layer.duration]`. Each curve requires 1-128 keys. Key insertion order is immaterial; equal-time duplicates (including equivalent fractions), an empty animation object, out-of-range values and unsupported fields/modes are rejected. Position values remain within -32768..32768.

##### Property retiming

Each position, opacity or mask curve can optionally contain `retime: {start, rate, reverse}`. `start` is a layer-local rational time within the layer's duration, `rate` is a positive rational multiplier from 1/16 through 16, and `reverse` defaults to false. Both rational denominators must reduce to at most 1,000,000. Omit `retime` to preserve the original key times. Explicit identity retiming uses `rate: {num:1,den:1}` and `start` equal to the curve's first key time.

At and before `retime.start`, forward playback holds the first key and reverse playback holds the last. Afterwards the source property clock is `first_key_time + (layer_time-start)*rate` for forward playback, or `last_key_time - (layer_time-start)*rate` for reverse playback. It clamps at the opposite endpoint. Intermediate key spacing and all interpolation modes are preserved; an exact reversed hold boundary evaluates the original key at that time. A single-key curve remains constant. Each curve has its own clock, so a mask reveal and layer movement can use different timing.

For example, this curve starts its eased fade one-fifth of a second after the layer begins and plays at twice its authored speed, reaching full opacity half a second later:

```json
{
  "keys": [
    {"time": {"num": 0, "den": 1}, "value": 0, "interpolation": "ease_in_out"},
    {"time": {"num": 1, "den": 1}, "value": 255, "interpolation": "hold"}
  ],
  "retime": {
    "start": {"num": 1, "den": 5},
    "rate": {"num": 2, "den": 1},
    "reverse": false
  }
}
```

Retiming changes property evaluation only. It does not retime source frames or audio, move the layer, extend its active duration or rewrite keys. A slowed curve can remain unfinished when the layer ends; use an explicit longer layer/scene when required. Invalid starts/rates fail with `INVALID_ANIMATION`, malformed rational times with `INVALID_TIME`. Derived rational times must fit the exact time representation and easing arithmetic uses checked 128-bit integers; very fine combinations that exceed these limits fail with `TIME_OVERFLOW` during inspection, before rendering publishes any output. Use coarser rational times in that case. There is no floating-point fallback or silent rounding of time.

Inspection and render receipts include `sampled_parameters` beside the selected source frames (`selected_frames`). Each holds a position/opacity value for every output frame, and null when no source frame is selected. Both arrays, and each tile's list in a tilemap layer's `tile_selected_frames`, are run-length encoded in frame order as `[{"count": n, "value": v}, ...]`: consecutive equal values form one run, so a held or static layer reports one entry however long the scene is. Expand the runs to get one value per frame. Animation is sampled at each output frame's start, then composited with the existing alpha and transform rules. Scene source files are preserved. Edited recipes must be rendered to a new output; existing saved timeline revisions continue referencing their previously compiled assets.

[Spatial transforms](SPATIAL.md) now provide animated scale/rotation and subpixel translation with declared image sampling. General control-handle curves, motion blur, variable media retiming and expression evaluation retain separate requirements. The original basic fixture is `python -X utf8 tests/animation.py --output C:\DEV\CutboltData\new-animation-test`. The broader interpolation/property-retiming checkpoint is exercised by `python -X utf8 tests/easing.py --output C:\DEV\CutboltData\new-easing-test`: it compares 950 decoded frames across 19 renders, checks independent property/mask clocks, replay and save/reload, and rejects 14 invalid or precision-exceeding inputs while preserving sources and existing outputs.

#### Source frame selection

The scene's optional `frame_rate` is one of the eight [native rates](NATIVE_TIMING.md), default 25 fps. Use the timeline's rate, because a compiled asset only plays on a timeline of the same rate. Duration, layer start and length, and strict holds use that clock, and the duration must also be a whole number of 48 kHz samples. `timing: "strict"` requires every source hold to land on the frame grid, so a 100 ms hold fails at 25 fps with `UNALIGNED_TIME`. Scene-layer [tracking](TRACKING.md), stabilization and reframing still measure layers on a 25 fps clock. `"sample_start"` selects the source frame containing each exact output time `n/25`, using half-open source intervals. It never rounds individual holds or accumulates their rounding errors. `scene.inspect` and render receipts list every selected source-frame index (or null), the exact source-cycle duration and selected policy, making temporal resampling visible.

`end: "loop"` uses rational modulo of the whole source cycle; `"hold_last"` holds its final frame; `"transparent"` reveals lower layers once that cycle ends. The explicit layer duration always limits activation. Repeated entries are significant, not deduplicated playback instructions.

### Audio

The current input matrix is classic PCM16 WAV, mono or stereo, at 24,000, 44,100 or 48,000 Hz. Extensible channel masks, float audio and other layouts/rates fail. Mono must explicitly request duplication to left/right; stereo must request preservation. There is one independent narration input, with no mix bus, gain, fades or audio-only timeline editing yet.

Output is 48 kHz stereo. Linear interpolation evaluates each exact destination sample position in the source; the last source sample extends to the final converted sample. Sample values round to nearest, half ties away from zero. Converted sample count is `round-half-up(source_count * 48000 / source_rate)`. Receipts retain both exact durations and counts. This simple upsampler is deterministic and tested for impulses/endpoints, but is not a high-quality band-limited resampler.

Start must land on a 48 kHz sample boundary. Padding before/after narration is silence. If converted audio extends beyond the scene, rendering fails with `AUDIO_OVERFLOW`; nothing trims or time-stretches speech to fit. An output video frame contains exactly 1,920 audio samples per channel.

### Limits

Scenes last from 1 frame to 120 seconds at their frame rate: 3,000 frames at 25 fps, 7,200 at 60 fps. They hold 1-64 layers with 1,024 animation references in total, logical dimensions of 1-4096 per axis, output scale 1-8 and at most eight million output pixels. Output scale 1 renders at native size, so a 1920 x 1080 scene can carry full-resolution artwork and text; the scale filter is skipped entirely. Individual image/audio files are limited to 64 MiB and PNGs to 4096 pixels per axis; PNG sources total at most 256 MiB. Decoded images, mattes, assembled tilemap canvases and the visible part of each text or shape canvas total at most 64 million pixels. A graphics layer keeps and counts only the bounding box of its nonzero alpha, so a caption cue on a full-frame canvas costs its text box rather than the frame. Layers with effects or a spatial transform, and 3D scenes, keep their whole canvas. A scene's `audio_mix` recipe lasts at most 60 seconds, so a longer scene takes its soundtrack as one `audio` WAV. `capabilities` reports these values under `scenes.limits`. [Fonts have separate limits](GRAPHICS.md#inspection-animation-and-limits). Positions/anchors and transforms are bounded. Validation errors name the field path and actual value, for example `layers[2] (title).start`.

Compositing work is bounded before any media is read. `scene.inspect` and `scene.render` report it under `work`. `composited_pixels` sums, over every layer and active frame (times shutter samples), the layer's transformed crop clipped to the scene, plus its whole canvas for a tilemap. Spatial and 3D layers count the whole scene, and a transparent scene counts twice, once for its color pass and once for its matte. Unchanging bottom layers count once (below). With [shutter sampling](TEMPORAL.md), averaging adds the whole scene once per sample and pass; at 1080p a sample measured about as costly as one full-frame layer. A scene above 64 billion fails with `LIMIT_EXCEEDED`, naming the layer with the largest share, the averaging share and how many bottom layers counted once. That keeps the worst case near the former limit of 600 frames of 16 full eight-megapixel layers. 3D geometry bounds its ray tests by the same 64 billion; shutter sampling and expressions also bound their per-sample records ([TEMPORAL.md](TEMPORAL.md#work-and-precision-limits), [EXPRESSIONS.md](EXPRESSIONS.md#work-limits-and-diagnostics), [GEOMETRY.md](GEOMETRY.md#bounds-and-inspection)).

Raw video is not buffered: frames are composited in parallel, restricted to each layer's destination rectangle, and streamed to the encoder in order. One worker per logical processor (at most 64) takes the next frame; workers stay at most two frames each, within about 256 MiB, ahead of the writer. Memory therefore grows with length only through small per-frame tables and the receipt, and scratch holds only the encoded output and the PCM soundtrack. After encoding, the engine decodes every output frame on 16 threads to check counts and timestamps; that check's timeout grows with the output's frames and pixels.

The bottom layers that look the same in every frame (same source frame, tile frames and sampled position, opacity, mask, effect and spatial values) are composited once per render, and each frame continues from that result. Because layers composite in order with rounding after each one, the output is identical. Shutter samples that fall outside the scene show the bare backdrop and do not prevent this.

The work limit counts a bottom layer once when its recipe alone proves that it never changes, and every layer below it qualifies too. Such a layer:
- lasts the whole scene;
- shows graphics, or one held image (one per tile) that does not end early with `end: "transparent"`;
- has no position or opacity curves and no expression bindings;
- has no mask curves, no effect curves or animated effect masks, and no spatial curves or stabilization compensation. A curve whose keys are all equal still counts as a curve.

`work.static_layers` reports how many layers counted once, and `capabilities` describes the rule under `scenes.limits.unchanging_layers`. For example, eleven unchanging full-frame layers at 1920 x 1080 for 120 seconds count 22.8 million pixels instead of 68.4 billion; that scene rendered in 44.6 s on the development machine, half busy with other work, peaking at 277 MB in the engine. A render composites at least those layers once; it fails with `LIMIT_EXCEEDED` rather than do more work than it counted. 3D scenes composite every sample. Unscaled, unrotated, unmasked normal-blend layers without effects take a row-by-row path with the same equation and rounding.

Measured with the release build on the development machine (32 logical processors) while another session's fixtures were running, for native 1920 x 1080 scenes built from the 5 October 2026 demo's art:

| Scene | Frames | Layers | Composited pixels | Render | Engine peak | Output | Receipt |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Story scene 1 as one 11.52 s shot | 288 | 11 | 1.1 billion | 5.7-11 s | 116 MB | 36 MB | 35 KB |
| Stage, 120 s at 25 fps | 3,000 | 18 | 11.2 billion | 75-86 s | 156 MB | 380 MB | 558 KB |
| The same with 40 caption cues | 3,000 | 58 | 17.0 billion | 79 s | 189 MB | 428 MB | 745 KB |
| Stage, 120 s at 60 fps | 7,200 | 18 | 26.8 billion | 162 s | 170 MB | 881 MB | 797 KB |
| 64 walking sprites, 120 s at 60 fps | 7,200 | 64 | 25.5 billion | 160 s | 250 MB | 697 MB | 2.7 MB |

About half of each render is the verification decode. With FFmpeg, the process tree peaks at 650-780 MB. The caption cues use 4.4 million decoded pixels; as whole canvases they would have needed 83 million, over the budget.

Receipts grow with what moves. `sampled_parameters` holds one run per change of a layer's values, so a busy two-minute stage reports about half a megabyte, and 64 sprites that move every frame about 2.7 MB. Over MCP, pass `save_as` to `job.wait` or `scene.inspect` to write a long receipt to a workspace file instead of returning it.

The former ten-second cap dated from compositing into a raw-video scratch file, which streaming removed. Render time, output size and receipt size still grow with length, and a running job cannot yet be cancelled, so the cap keeps one compile to a few minutes. A narrated story scene or a long title or explainer shot fits in one scene. Longer pieces are several scenes cut together on the timeline, and a whole caption track uses `captions.render`. The five-second pilot is 320 x 180 enlarged six times to 1920 x 1080. `tests/native_scenes.py` checks a native 1920 x 1080 tilemap scene, its exact equivalence with a 480 x 270 scene enlarged four times, and a 24-layer scene of 12 seconds (two minutes with `--long-form`).

Frame/range previews take ordinary reference projects. Times must land on frame boundaries and stay inside the timeline. Frame output is full resolution, capped at eight million pixels; there is no implicit low-resolution preview transform or contact-sheet sampling. Frame inspection validates the selected clip's source; it does not certify unrelated source files elsewhere in the project. Range previews validate/render the intersecting clips. Original snapshots/revisions are unchanged.

## Retained example and reproduction

Generate original fixtures and run the independent acceptance suite into a **new external directory**:

```powershell
cargo build --locked
python -X utf8 tests/scenes.py --output C:\DEV\CutboltData\my-scene
# Optional live companion handoff, requiring the separately installed PixelForge:
python -X utf8 tests/scenes.py --output C:\DEV\CutboltData\my-pixelforge-scene --pixelforge C:\DEV\PixelForge
```

This saves `scene.json`, `project.json`, input media, frame/range previews and `verification.json`. The test compares every decoded 1080p frame against independently assembled Pillow pixels and every audio sample against rational impulse expectations. It also tests all quarter turns, crops, offsets, alpha/opacity, timing modes, source preservation, containment, invalid input, existing outputs and missing tools. The optional companion test records its exact Git commit and proves that exported individual-frame PNGs preserve pixels and expanded animation order.

Render a saved scene to a new path:

```powershell
$run = 'C:\DEV\CutboltData\my-scene'
$scene = Get-Content -Raw "$run\scene.json" | ConvertFrom-Json
@{command='scene.render'; scene=$scene; input_root="$run\sources";
  output_root="$run\output"; output="$run\output\another-scene.mkv"} |
  ConvertTo-Json -Depth 40 -Compress | .\target\debug\cutbolt.exe
```

The optional `tools/pixelforge_handoff.py --atlas <atlas.json> --input-root <root> --animation <name> --output <new-external.json>` returns `canvas`, ordered `frames`, `end` and provenance. Copy the first three fields into a layer and choose its explicit transform/timing. Individual PixelForge PNGs are full-canvas even when atlas packing is trimmed; the adapter therefore uses offset zero and retains the atlas's trim metadata only as provenance. It never copies PixelForge implementation or requires it at engine runtime.

This is the technical P1/P2 pilot, with synthetic audio. Qwen inference, editorial review gates, selective production rebuilds, production caption integration and production delivery integration remain separate work. The standalone [H.264/AAC export profile](EXPORT.md) has separate verified evidence. The local [caption commands](CAPTIONS.md) independently support sidecars and bounded caption scenes.
