# Local dialogue cleanup

`audio.repair.inspect` evaluates an explicit repair recipe without writing files. `audio.repair.render` writes a separate verified WAV under an explicit output root. Both use the original local Rust implementation; no model, service or runtime download is required. The native MCP catalog exposes inspection as `cutbolt_audio_repair_inspect`. Rendering remains a blocking CLI/library command.

The supported profile is 48 kHz mono/stereo PCM16, from classic or supported extensible WAV, with a selected output range from one sample to 60 seconds. The source is bound by path, size and SHA-256 and must fit within 64 MiB. Output retains the selected sample count and channel layout. It does not trim pauses, normalize peaks or move words. Preserve the original recording and editable recipe.

## Recipe and clocks

```json
{
  "schema_version": 1,
  "id": "dialogue-cleanup",
  "source": {"path":"recording.wav","bytes":960044,"sha256":"<actual SHA-256>"},
  "source_in": {"num":1,"den":1},
  "duration": {"num":8,"den":1},
  "noise": {
    "regions": [{"start":{"num":0,"den":1},"duration":{"num":1,"den":1}}],
    "strength_milli": 1250,
    "floor_milli": 100
  },
  "remove_dc": true,
  "effects": [{"type":"high_pass","frequency_hz":80,"q":0.707}]
}
```

Use the actual source identity and ranges. All times are exact rational seconds aligned to the original 48 kHz sample grid. `source_in` and every noise-region `start` refer to the original file. The output starts at zero and contains exactly `duration` samples. A noise region may precede or follow the selected output, provided it lies within the same bound source.

`noise`, `remove_dc` and `effects` are optional. Without them the command extracts the selected PCM exactly. `remove_dc` defaults to false; `effects` defaults to empty. Unknown fields reject.

| Noise setting | Meaning and bounds |
| --- | --- |
| `regions` | One to eight caller-declared noise-only ranges. Each contains at least 4,096 samples; total duration is at most ten seconds. Regions must not overlap. |
| `strength_milli` | Noise-power subtraction scale from 0 to 3,000; default 1,250. A value of 1,000 subtracts the estimated noise power before the gain floor. |
| `floor_milli` | Minimum amplitude gain from 0 to 1,000; default 100, corresponding to a 0.1 gain floor. A higher floor retains more background and may preserve weak foreground detail. |

Select background-only regions deliberately. The engine does not identify speech or verify that the chosen region contains only noise. Speech or transients in a profile can suppress wanted material. Region order does not change evaluation. Splitting a region can change its complete analysis windows; regions are not silently merged. Strength zero, floor 1,000, or an all-zero learned profile bypasses spectral attenuation exactly, while still validating the full recipe and source.

## Processing definition

The original transform uses 4,096 samples and a 1,024-sample hop. Its window at index `i` is `sin(pi*(i+0.5)/4096)^2`. Each channel has an independent noise estimate: average the squared complex magnitudes of every complete window in the selected noise regions. Windows begin at each region's start and advance by one hop. Reports expose these unnormalized bin powers and the number of contributing windows; they are not a power-per-Hz measurement. Nonnegative bin `k` represents `k*48000/4096` Hz.

For an output bin with observed power `P` and learned noise power `Q`, the raw amplitude gain is `sqrt(max(floor^2, 1-strength*Q/P))`. When `P` is zero, gain is one if `Q` is also zero, otherwise the declared floor. The gain is smoothed across frequency by `[1,2,3,2,1]/9` with replicated endpoint bins. The same real gain multiplies both members of each conjugate pair; original phase is retained.

Output analysis uses zero extension outside the selected range and a fixed grid beginning three hops before output sample zero. An original radix-2 transform and inverse perform analysis/reconstruction. A bounded overlap accumulator adds inverse samples multiplied by the window, then divides each retained sample by its accumulated window-square weight. There is no output delay, extra tail or endpoint fade. Changing the selected range starts a new grid and padding boundary; independently repairing a shorter range need not equal cropping a longer repair.

Processing order is:

1. Spectral suppression, separately per channel.
2. Optional subtraction of each reconstructed channel's mean over the complete selected output. This is a declared mean-removal operation, not automatic estimation of changing electrical interference.
3. Nearest-integer rounding with signed ties away from zero.
4. Existing ordered EQ/compression, with zero initial filter state. Parameters and maximum eight effects follow [audio processing](AUDIO.md). Mono uses one channel; stereo compression links both.
5. Final PCM16 saturation, clipping counts and output meters.

The existing high-pass effect can reduce low-frequency rumble; choose its cutoff for the source. Inspection reports the removed mean, effects, per-channel peaks/clipping, output PCM digest and meters. Stereo retains the existing loudness meter. Mono integrated loudness is explicitly unmeasured in this profile. Floating-point transforms/effects do not promise cross-platform bit identity; the retained environment is recorded in verification.

## Quality and limits

This is fixed-profile suppression for a known background. It does not adapt to changing noise, isolate overlapping speakers, restore clipped speech, remove clicks or dereverberate a room. Weak foreground energy close to the noise spectrum can be attenuated. The 85.3 ms analysis window can alter sharp transients, including energy before an onset; sample timing and perceptual preservation are separate properties.

Controlled quality fixtures use original passages synthesized by installed local voices, then add independently seeded white/colored noise, 50/100 Hz hum and mixed 60 Hz hum/noise at multiple levels. Candidate settings were chosen using separate preparation passages. The acceptance suite compares the result with each known clean waveform and declares these gates before running its held-out passages:

| Measurement | Required gate |
| --- | --- |
| Speech-interval signal/error improvement | At least 2 dB for every case; at least 4 dB for white noise. |
| Noise-only interval reduction | At least 6 dB. |
| Aligned voice projection gain | 0.8–1.05 across the speech interval. |
| High-band voice projection gain, 3–10 kHz | 0.65–1.15, with no increase in high-band error power. |
| Added white-noise tonal concentration | At most 6 dB in the 95th-percentile short-window measurement. |
| Known impulse timing and gain | Zero sample shift; gain 0.8–1.05. |
| Foreground-induced pre-echo peak | At most 2% of the known impulse amplitude. |
| Clean signal with zero noise profile, explicit bypass | Every rounded PCM sample unchanged. |
| Known 20 Hz rumble with 1 kHz foreground | At least 24 dB rumble reduction; foreground amplitude change below 0.1 dB. |

These are bounded numerical quality comparisons, not a listening panel, recognition/intelligibility score or a guarantee for arbitrary dialogue. The report retains every case and metric, including attenuation tradeoffs. A lower noise level alone does not pass the quality gate.

## Bounds, publication and editing

Output duration, source size, channel count, eight profile regions and ten seconds of profile material bound processing work. The overlap state stores one transform window rather than a full-duration buffer per analysis window. Source decoding and reconstructed/final output buffers remain bounded. A 60-second stereo fixture with ten seconds of profile material executes the actual spectral path and must inspect within 40 seconds and 256 MiB peak working set on the recorded test environment.

Sources are checked before and after evaluation and again immediately before publication. The engine writes into an owned scratch directory, decodes and compares the WAV's layout/rate/samples, then publishes without overwriting an existing destination. Normal failure removes owned scratch files. Forced process termination can leave scratch files; broader export-recovery work remains separate.

Output is an extensible PCM16 WAV with an explicit mono/stereo speaker mask. To use it in a scene, bind its new identity in an [audio routing recipe](AUDIO_ROUTING.md), use `preserve_layout` or `duplicate_mono` as appropriate, and explicitly route to a stereo output. Compile the scene as a fresh asset for existing saved sessions, previews, retries and undo/history. The fixture verifies this entire path. Keep the repair recipe alongside the original source; the WAV contains rendered samples, not editable repair parameters or copied source metadata.

## Verification and example

```powershell
cargo build --locked
python -X utf8 tests/audio_repair.py --output C:\DEV\CutboltData\my-dialogue-test
$demo = 'C:\DEV\CutboltData\my-dialogue-test'
$recipe = Get-Content -Raw "$demo\spoken-a-colored-recipe.json" | ConvertFrom-Json
$inspection = @{command='audio.repair.inspect';recipe=$recipe;input_root="$demo\sources"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe | ConvertFrom-Json
$inspection.result | Select-Object profile,samples,layout,pcm_sha256
@{command='audio.repair.render';recipe=$recipe;input_root="$demo\sources";
  output_root="$demo\output";output="$demo\output\cleaned-dialogue.wav"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The independent reference uses externally installed [NumPy 1.26.4 transforms](https://numpy.org/doc/1.26/reference/generated/numpy.fft.rfft.html), whole-buffer overlap accumulation and separately pinned external EQ. It compares every output PCM sample and independently parses/decodes the published WAV. Tests cover exact trims and original-source profile clocks, unordered regions, channel independence, clean/bypass identity, effect order, clipping, quality/artifact gates, typed MCP discovery, templates, saved edits, frame/range previews, maximum bounds and invalid inputs. A Rust test changes the source after preparation and checks failed publication, scratch cleanup and existing-output preservation. Full verification is required before either A05 checkpoint earns points. Test-only tools and voice identities are recorded in the [dependency ledger](DEPENDENCIES.md); they are not repair-engine runtime dependencies.
