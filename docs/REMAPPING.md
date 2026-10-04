# Variable-speed source remapping

`media.conform.inspect` accepts optional `recipe.remap` through CLI/library and the existing read-only MCP tool. `media.conform` compiles the same recipe to a new identity-bound reference asset. Keep the source and recipe outside the repository. Register the returned asset to trim, assemble, preview or queue ordinary saved-session edits. Re-editing the speed map requires a new conversion; session undo does not regenerate media.

The existing [source matrix, color interpretation and publication rules](CONFORM.md) apply. Output remains 25 fps FFV1/RGB8 with 48 kHz stereo PCM16, up to 45,000 frames/30 minutes. Source duration, streaming and random-access window limits are those of [media conversion](CONFORM.md). This feature does not change native timeline frame rates or the separate long-form synchronization acceptance criterion.

## Recipe

Add this field to a conform recipe. Set legacy `rate` to `{num:1,den:1}`, `reverse` and `freeze` to `false`; `source_in` supplies the initial source position. Set top-level `audio` to `resample` for `follow_speed`, or `mute` for `mute`. Contradictory settings fail.

```json
{
  "remap": {
    "segments": [
      {
        "duration": {"num":1,"den":1},
        "start_rate": {"num":1,"den":4},
        "end_rate": {"num":9,"den":8},
        "reverse": false
      },
      {
        "duration": {"num":1,"den":1},
        "start_rate": {"num":9,"den":8},
        "end_rate": {"num":2,"den":1},
        "reverse": false
      }
    ],
    "video_sampling": "linear",
    "audio_pitch": "follow_speed"
  }
}
```

This accelerates continuously from quarter speed to double speed over two output seconds, consuming 9/4 source seconds. The enclosing recipe duration must be exactly two seconds. One equivalent two-second segment produces identical samples.

## Exact clocks and boundaries

Supply 1–64 segments in playback order. Their positive durations must lie on the 48 kHz output sample clock and sum exactly to the recipe duration. Segment boundaries can fall between output video frames. Endpoint rates are exact nonnegative rationals in 0–16; zero at either endpoint starts or ends at rest. Both rates zero hold one source time. Each segment declares its direction with `reverse`.

For elapsed segment time `t`, duration `d` and endpoint speeds `a,b`, source distance is the exact integral `a*t + (b-a)*t*t/(2*d)`. Forward adds that distance to the segment's initial source position; reverse subtracts it. Each next segment starts exactly where the previous one ends. The engine never accumulates rounded frame/sample increments. Rate discontinuities are allowed and inspectable; they do not jump the source position. Choose matching adjacent speeds and a zero-speed turning point when a smooth transition is desired.

The whole continuous source interval must remain within decoded media, including a brief excursion between output samples. Audio and video extents are checked separately when sound is enabled. Sampled video positions must be strictly before the final frame's end; an unsampled final endpoint may equal the source extent. Negative positions, empty maps, gaps/overruns, unaligned segment durations and checked rational precision overflow fail explicitly. As elsewhere, arithmetic is bounded by exact JSON integer precision; extremely fine rational combinations can fail during sampling before publication. No partially rendered asset is published.

## Video interpolation

At each exact output frame time, inspect the adjacent decoded source timestamps around its mapped position:

| `video_sampling` | Behavior |
| --- | --- |
| `previous` | Select the latest source timestamp at or before the mapped position. |
| `nearest` | Select the nearer adjacent timestamp; an exact midpoint chooses the later frame. |
| `linear` | Blend adjacent frames using the exact fractional distance between their timestamps. At an exact source timestamp, use that frame alone. |

The last source frame extends to its declared end; there is no following frame to extrapolate. VFR and fractional-rate source timestamps use their actual decoded spacing. Reverse uses the same position-based interpolation, and freezing between timestamps can retain an interpolated frame.

Each contributing source frame is independently normalized and LUT-processed before blending. Interpolation operates on the resulting RGB8 working values, with one positive nearest-integer rounding and ties upward. This is encoded-value blending under the existing SDR contract, not linear-light integration or motion-compensated optical flow. Moving objects and cuts can show double images; use `previous` to retain discrete frames. Existing normalization/LUT numerical tolerances remain applicable. Nearest spatial resizing follows temporal blending. Audio-only sources produce black video.

Inspection and receipts include every output frame's exact source time, contributing source indices and exact second-frame weight, plus each segment's source/output endpoints. Existing `source_frame_indices` names the first contributor; the new `remap.video_samples` describes both. No frame index is inferred from nominal source frame rate.

## Audio pitch

`audio_pitch: "follow_speed"` requires top-level `audio: "resample"`. Every output audio sample uses the same integrated source-time function, multiplied by the decoded source sample rate. Adjacent PCM16 samples are linearly interpolated with exact rational weights and rounded once, with signed ties away from zero. Unlike the legacy constant-speed recipe, a remap source-in may fall between source audio samples. Mono duplicates to stereo; stereo ordering is preserved; missing audio is silence.

Pitch follows instantaneous speed. A 220 Hz source tone at speed 1/2 sounds near 110 Hz; at speed 2 it sounds near 440 Hz. Pitch is not preserved, and the existing linear resampler is not a band-limited high-quality stretch algorithm. Abrupt speed changes can introduce audio artifacts. Reverse segments and full-segment freezes require the explicit `mute` policy for the entire conversion. A forward ramp may touch zero speed at an endpoint. `audio_pitch: "mute"` with top-level `audio: "mute"` produces exact silence. Pitch-preserving stretch, reverse audio and optical-flow interpolation remain open requirements; unsupported policy names fail rather than silently falling back.

## Verification and runnable fixture

Use a fresh external directory:

```powershell
cargo build --locked
python -X utf8 tests/remapping.py --output C:\DEV\CutboltData\my-remapping-test
$demo = 'C:\DEV\CutboltData\my-remapping-test'
$recipe = Get-Content -Raw "$demo\accelerate-recipe.json" | ConvertFrom-Json
@{command='media.conform.inspect'; recipe=$recipe; input_root="$demo\sources"} |
    ConvertTo-Json -Depth 40 -Compress | .\target\debug\cutbolt.exe
@{command='media.conform'; recipe=$recipe; input_root="$demo\sources";
    output_root="$demo\output"; output="$demo\output\extra.mkv"} |
    ConvertTo-Json -Depth 40 -Compress | .\target\debug\cutbolt.exe
```

Original generated RGB and tones drive independent Fraction polynomial integration, source-timestamp lookup, full frame and PCM comparisons. Analytic intensity and zero-crossing measurements distinguish smooth interpolation and changing pitch from simple repeated-frame or constant-rate behavior. Fixtures cover acceleration/deceleration, equivalent split ramps, sample and between-frame segment boundaries, zero/high/low rates, reverse/freeze transitions, end holds, VFR/fractional/reordered sources, mono/stereo/missing audio, normalization/LUT order and resize. Saved retries, trims, undo, frame/range previews and MCP schema/inspection are checked. Invalid maps, source excursions, unsupported policies, identities, source/LUT changes after encoding and existing destinations must preserve inputs and outputs. Codec decoding uses the external media tools on both sides; temporal arithmetic and sample expectations are independent of the engine.
