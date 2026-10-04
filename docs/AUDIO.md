# Sample-based audio mixes

For fixed-profile noise suppression and basic cleanup before mixing, use the separate [dialogue-repair recipe](DIALOGUE_REPAIR.md). It produces a new PCM source without changing the original recording or sample timing.

The original `pcm-mix-v1` recipe edits PCM WAV sources on a sample clock. Optional [audio routing](AUDIO_ROUTING.md) adds named surround layouts, buses, channel matrices, panning and bus effects; the unrouted stereo contract below remains unchanged. It supports source cuts, placement with silence, mono duplication or stereo preservation, clip/track gain and mute, clip fades, gain curves, overlapping clips and multiple tracks. It exports a new WAV or supplies a pixel scene's soundtrack. Keep the mix JSON and source WAVs: a compiled scene is flattened, and subsequent mix changes require compiling a new output before updating a saved session.

## Commands and recipe

| Command | Fields besides `command` | Result |
| --- | --- | --- |
| `audio.inspect` | `mix`, `input_root` | Evaluate the full mix and return counts, identities, PCM digest, channel peaks and clipping counts |
| `audio.render` | `mix`, `input_root`, `output_root`, `output` | Write and read back a new 48 kHz stereo PCM16 `.wav`; return the inspection report plus output/recipe hashes |

`audio.inspect` is also available through MCP. Rendering is a blocking CLI/library operation and is outside the persisted job queue. All roots must be existing absolute directories. Output must be a new absolute path beneath the output root, with an existing parent. Source identities use normal relative paths, exact byte counts and lowercase SHA-256. Traversal, identity mismatches, unknown fields and unsupported audio fail explicitly. Rendering rechecks source identities before publication and never overwrites a source or existing output. Normal failures remove owned scratch files; a forced process exit can leave an owned scratch directory, as with scene compilation.

A mix has `schema_version: 1`, a nonblank `id`, rational `duration`, `tracks` and optional ordered master `effects` (default empty). Each track has a unique `id`, `clips`, optional `gain_milli` (default 1000) and `mute` (default false). Each clip has:

| Field | Meaning |
| --- | --- |
| `id` | Unique across every track in this mix |
| `file` | `{path, sha256, bytes}` identity relative to the input root |
| `channels` | `duplicate_mono` for mono input or `preserve_stereo` for stereo input |
| `start`, `source_in`, `duration` | Nonnegative rational seconds; positive clip duration |
| `gain_milli`, `mute` | Linear amplitude gain, default 1000, and mute, default false |
| `fade_in`, `fade_out` | Rational durations, default zero, each no longer than the clip |
| `gain_curve` | Optional [property curve](SCENES.md#property-keyframes), with values 0..4000 and a clip-local clock |

Mix duration, clip placement/duration and fades align exactly to 48 kHz. Source-in aligns to the source's sample rate. The selected source interval must fit the WAV and the placed interval must fit the mix. Gaps produce zero samples; overlapping clips sum. Muted clips still undergo source and parameter validation.

## Defined sample behavior

For output sample index `n` within a clip, read source position `source_in_samples + n * source_rate / 48000`. Linear interpolation between the adjacent source samples rounds to the nearest integer, with ties away from zero. At the physical source end, the last sample extends through its final sample interval. Mono input duplicates into both output channels; stereo input retains left/right order. This is a bounded linear resampler, not a high-quality reconstruction filter.

Gain 1000 is unity, 500 is half amplitude, and 4000 is four times amplitude. A gain curve replaces the clip's static gain and is evaluated at `n/48000`; track gain multiplies it. Curves support the existing hold, linear and quadratic easing modes, endpoint holds and optional property retiming. Retiming changes the gain clock only; it does not change audio speed or pitch. Key values and sampled gain are integers in linear milli-units.

For a clip of `N` samples, fade-in of `Fi` and fade-out of `Fo`:

- Fade-in weight is `min(1, n/Fi)` when `Fi > 0`, otherwise 1.
- Fade-out weight is `min(1, (N-n)/Fo)` when `Fo > 0`, otherwise 1. Its zero endpoint lies at the exclusive clip end.
- Both weights multiply when fades overlap. Two equal signals overlapping for `F` samples, with outgoing fade-out `F` and incoming fade-in `F`, have complementary linear weights.

Each voice's resampled sample is multiplied by clip gain / 1000, track gain / 1000 and the two exact rational fade weights, then rounded once to nearest with ties away from zero. Contributions accumulate in a wide integer buffer. Only the final sum is saturated to signed PCM16 `[-32768,32767]`, so cancellation is preserved and track order does not change the result. Two voices rounded separately can differ by one unit from scaling their sum once.

Inspection reports `peak_absolute` per output channel (integer PCM magnitude, maximum 32768), and `clipped_samples` per channel counted before final saturation. These fields remain sample peaks and clipping diagnostics. A separate `meters` object measures final PCM loudness, sample peaks and RMS as described below. The PCM hash covers all interleaved little-endian stereo output samples. Rendering reopens the WAV and verifies every sample before publishing it.

## Master processing and meters

Optional `effects` run in array order after wide voice summation and before final saturation. An empty array preserves the original integer path exactly. A nonempty chain converts the sum to normalized 64-bit floating point, processes with state reset at mix start, and rounds once back to integer samples before saturation. Filters continue through gaps, but output duration never grows to include a tail. Receipts include the selected effects and processing profile. In this unrouted profile, effects are on the master mix. The routing extension also supports bus effects and explicit multichannel processing.

| Effect `type` | Required parameters and inclusive limits |
| --- | --- |
| `low_pass`, `high_pass` | `frequency_hz`: 20..20000, `q`: 0.1..20 |
| `peaking` | The same frequency/Q fields, plus `gain_db`: -24..24 |
| `compressor` | `threshold_db`: -60..0, `ratio`: 1..20, `attack_ms`: 0..1000, `release_ms`: 0..5000, `makeup_db`: 0..24 |

All values must be finite. At most eight effects are accepted. Intermediate normalized magnitude above `1e12` or a nonfinite result fails with `AUDIO_PROCESSING_OVERFLOW`; invalid parameters fail with `INVALID_AUDIO_EFFECT`. Unknown fields/types fail during request decoding. Floating-point results have declared numerical tolerances; cross-platform bit identity is not promised for this processing path.

The two-pole EQ uses the public low-pass, high-pass and peaking equations in the [W3C Audio EQ Cookbook, 8 June 2021](https://www.w3.org/TR/2021/NOTE-audio-eq-cookbook-20210608/). Each stereo channel has separate filter state. The implementation is original Rust; no third-party filter source is incorporated.

The compressor is an original hard-knee, linked-stereo peak design. For each sample pair it takes the larger absolute channel value and calculates the amount above the threshold in dB. Desired reduction is `-over * (1 - 1/ratio)`. Reduction starts at zero and approaches that target with coefficient `exp(-1/(48*milliseconds))`, using attack when reduction deepens and release otherwise. A zero time applies the target immediately. Both channels receive gain `10^((reduction+makeup_db)/20)`. It has no lookahead, sidechain or soft knee and is not a true-peak limiter; attack transients can clip.

For example, add this field to a mix recipe:

```json
"effects": [
  {"type":"high_pass", "frequency_hz":80, "q":0.707},
  {"type":"peaking", "frequency_hz":2500, "q":1, "gain_db":2},
  {"type":"compressor", "threshold_db":-18, "ratio":3,
   "attack_ms":10, "release_ms":100, "makeup_db":2}
]
```

Meters observe the final saturated PCM, normalized by 32768. `sample_peak_dbfs` and `rms_dbfs` are per-channel arrays; silence is null. `integrated_lkfs` uses the stereo portion of [ITU-R BS.1770-5 Annex 1, November 2023](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I): published 48 kHz K-weighting, equal left/right energy weights, 400 ms windows at 100 ms spacing, an absolute -70 LKFS gate, then a relative gate 10 dB below the absolute-gated mean. Incomplete final windows are excluded. No passing blocks yields null and `loudness_status: "below_gate"`; fewer than 19,200 samples yields `"insufficient_duration"`. The report includes both gate counts, relative threshold and unmeasured tail size. Sample peaks and RMS still use the whole signal. There is no true-peak, loudness-range or surround measurement and no meter certification claim.

Run `python -X utf8 tests/audio_processing.py --output C:\DEV\CutboltData\new-processing-test` for original impulses, tones, level steps and quiet/silent intervals. EQ output is checked against external FFmpeg filters, compressor timing against closed-form step responses, and meters against independent PCM arithmetic, 997 Hz calibration and FFmpeg's separate loudness meter. Acceptance allows at most one PCM unit per sample and 0.11 LU against its rounded meter output. Ordered-chain, clipping/headroom, reload/MCP, invalid parameter and source-preservation cases are included. No additional dependency is required.

## Limits and scene use

Sources are classic PCM16 mono/stereo WAV at 24, 44.1 or 48 kHz. A mix contains 1 sample to 60 seconds, at most 16 tracks, 128 clips, eight million summed clip sample frames and 64 MiB of distinct declared source files. IDs have 1..128 nonblank UTF-8 bytes; clip/track gain is 0..4000. Curves retain the shared 128-key, rational precision and checked-overflow limits. Other direct mix codecs, true-peak metering and inline mix speed/pitch changes remain open. Buses, pan and named surround PCM are available through the explicit [routing extension](AUDIO_ROUTING.md). Source conversion can separately apply [variable-speed remapping](REMAPPING.md) with explicit pitch-following resampling or silence; this does not change the mix recipe clock.

Set a scene's optional `audio_mix` to the complete recipe and omit/set `audio` to null. Mix duration must exactly equal scene duration. Scene limits still apply, including its 10-second maximum and frame-aligned total duration; individual audio cuts remain sample aligned. The resulting FFV1/PCM asset enters `media.add`, saved sessions, timeline edits and the existing render queue normally. Mix recipes themselves are not saved-session revisions.

## Reproduce the acceptance fixture

```powershell
cargo build --locked
python -X utf8 tests/audio.py --output C:\DEV\CutboltData\new-audio-test
$mix = Get-Content -Raw C:\DEV\CutboltData\new-audio-test\mix.json | ConvertFrom-Json
@{command='audio.inspect'; mix=$mix; input_root='C:\DEV\CutboltData\new-audio-test\sources'} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The fixture generates original signals outside the repository and independently computes resampling, envelopes and wide summation with Python rational arithmetic. It compares 10,543 stereo sample frames across 22 WAV renders, verifies scene-to-session pixels and audio, checks MCP recipe reload/order invariance, rejects 16 malformed or unsupported cases, and preserves original hashes/existing outputs. The one-frame scene/session path also checks that reference export retains its declared 25 fps profile.
