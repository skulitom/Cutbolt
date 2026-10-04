# Audio and declared timecode synchronization

`sync.inspect` estimates or declares a map between two identity-bound source clocks and returns explicit [media-conversion recipes](CONFORM.md). It is read-only through CLI, library and the `cutbolt_sync_inspect` MCP tool. It changes no source, saved project or output file. To correct clocks, explicitly run the returned recipes through `media.conform` into new files, add the returned bound assets to a project, and use them in [camera groups](MULTICAM.md).

The source profile is 25 fps FFV1/bgr0 with matching project dimensions, continuous zero-origin video and stereo PCM16 at 48 kHz. Both sources must be content-bound in `project.assets`. Each source is limited to 60 seconds. Convert other supported source formats explicitly first; this command does not infer their color interpretation.

## Request and result

The command requires `id` (1–100 UTF-8 bytes), `project`, absolute `input_root`, `reference_asset_id`, `candidate_asset_id`, frame-aligned `start`, positive frame-aligned `duration`, `rounding` and `method`. The requested interval is on the reference clock, up to 1,500 frames. The mapped output must fit both physical sources.

The returned `mapping` defines:

```text
candidate_time = candidate_anchor + (reference_time - reference_anchor) * rate
```

All three values are nonnegative rational times/rates. Arithmetic also handles reference times earlier than the anchor; a negative mapped candidate position rejects. Diagnostics include source identities, measurements, confidence, exact recipes and inspected frame selections. Floating-point confidence diagnostics are separate from exact timeline/recipe clocks.

`rounding: "reject"` requires an exactly sample-aligned mapped candidate start. `"nearest_sample"` rounds to the nearest 48 kHz sample, with half-sample ties upward. The result reports the ideal start, actual start and adjustment. The rate remains an exact ratio. Video conversion selects the latest source frame at or before each mapped time; audio uses linear interpolation and changes pitch with rate. This corrects constant recording-clock drift without claiming pitch-preserving time stretching. Inverted polarity is reported, not automatically reversed in the output recipe.

## Audio measurements

An audio method supplies bounded search windows and quality requirements:

```json
{
  "mode": "audio",
  "windows": [
    {"reference_center":{"num":1,"den":1},"candidate_center":{"num":1,"den":1},"length":{"num":32,"den":375},"search_radius":{"num":1,"den":5}},
    {"reference_center":{"num":5,"den":2},"candidate_center":{"num":5,"den":2},"length":{"num":32,"den":375},"search_radius":{"num":1,"den":5}},
    {"reference_center":{"num":4,"den":1},"candidate_center":{"num":4,"den":1},"length":{"num":32,"den":375},"search_radius":{"num":1,"den":5}}
  ],
  "reference_channel": 0,
  "candidate_channel": 0,
  "polarity": "same",
  "minimum_correlation_milli": 900,
  "minimum_margin_milli": 100,
  "maximum_drift_ppm": 2000,
  "maximum_residual_samples": 2
}
```

Select channel 0 or 1 independently in each source. `polarity` is `same` or `either`. The candidate center is the approximate position to search; the caller must provide windows likely to contain the same distinctive event. No whole-file match discovery is claimed.

Limits are 3–8 windows, even lengths of 2,048–8,192 samples and search radii of 32–48,000 samples. All centers/lengths/radii must align to samples and the complete windows/search intervals must fit the physical source. Reference centers must be distinct and span at least one second. Quality parameters require correlation 500–1,000 milliunits, separated-peak margin 1–1,000 milliunits, maximum drift 0–5,000 ppm and residual 0–16 samples.

The original estimator compares mean-subtracted moving averages. It searches 64-sample averages on a 16-sample coarse grid, keeps at most 32 peaks separated by more than 128 samples, then refines each within 24 samples using 16-sample averages and a one-sample lag grid. It rejects insufficient variation, low correlation or an inadequate margin against separated refined peaks. The margin compares the searched peaks; it is not proof of a unique global match. Narrow features, repeating content, noise or a search interval that misses the corresponding event can fail or mislead correlation. Returned confidence is estimated evidence, not ground truth.

The earliest/latest accepted matches define the exact constant rate. Every intermediate match must agree within the declared residual and polarity; excessive drift or inconsistent windows reject. The recipe can extrapolate beyond calibration windows within the requested source bounds, but measurements do not establish that unmeasured content has the same clock. Review the report and corrected output before incorporating it into an edit. Nonlinear drift, missing stretches, free-running video/audio clocks and time-varying polarity need separate treatment.

## Declared timecode

Timecode alignment uses caller-supplied labels and equal-rate clocks:

```json
{
  "mode": "timecode",
  "reference": {"day":1,"label":"00:00:00:02","media_at":{"num":28,"den":25}},
  "candidate": {"day":0,"label":"23:59:59:21","media_at":{"num":1,"den":1}}
}
```

Each label names the absolute clock at the given in-source `media_at`. Use exactly `HH:MM:SS:FF` at 25 fps, with hours 0–23, minutes/seconds 0–59 and frames 0–24. `day` is explicit, from 0 through 1,000,000; there is no inferred midnight wrap. Anchors must be frame-aligned and strictly inside their source. This example maps candidate media three frames ahead of reference media, despite crossing midnight in the declarations.

The format reference is the public [FFmpeg timecode option documentation](https://ffmpeg.org/ffmpeg-filters.html#drawtext-1). This implementation deliberately supports only the colon-separated 25 fps non-drop subset. Drop-frame/fractional labels, embedded timecode extraction and rate inference are unsupported. Declared timecode does not measure drift or verify that supplied labels describe the recording; use audio evidence when clock accuracy needs measurement.

## Run an original retained fixture

Use a new directory outside the repository:

```powershell
python tests/synchronization.py --output C:\DEV\CutboltData\clock-demo
.\target\debug\cutbolt.exe C:\DEV\CutboltData\clock-demo\request.json
```

The fixture saves its complete request and verification report. To publish another corrected candidate explicitly, read that request, inspect it, and run its returned candidate recipe to an unused file:

```powershell
$request = Get-Content C:\DEV\CutboltData\clock-demo\request.json -Raw | ConvertFrom-Json
$response = $request | ConvertTo-Json -Depth 100 -Compress | .\target\debug\cutbolt.exe | ConvertFrom-Json
if (-not $response.ok) { throw ($response.error | ConvertTo-Json) }
$conversion = @{
  command = 'media.conform'
  recipe = $response.result.candidate_recipe
  input_root = $request.input_root
  output_root = 'C:\DEV\CutboltData\clock-demo\output'
  output = 'C:\DEV\CutboltData\clock-demo\output\reviewed-candidate.mkv'
}
$conversion | ConvertTo-Json -Depth 100 -Compress | .\target\debug\cutbolt.exe
```

Keep the inspection report and conversion receipt externally for provenance. Register the conversion receipt's `asset` through `media.add`; make a normal child sequence containing that asset's video/audio and declare zero source offsets in the aligned camera group. Corrected output duration is the requested shared interval. Saved edits do not replace original assets automatically.

## Acceptance evidence

The retained run is `C:\DEV\CutboltData\synchronization-20261003-03`. Its original seeded aperiodic signal is sampled under known camera offsets and rates. Audio measurements locate windows within 0.6 samples and estimate true +800/−600 ppm drift as +798.611/−597.222 ppm. Corrected stereo PCM RMS differences from the independent physical reference are 166.95 and 54.24 units; the corresponding uncorrected left-channel errors are 7,429.24 and 7,284.32. The acceptance limits are one sample per measured window, 15 ppm drift error, corrected RMS below 256 PCM16 units, and at least a twentyfold reduction. These tolerances account for integer-sample lag estimates and explicit start rounding.

Separately, every decoded pixel and PCM sample of the produced corrections matches an independent integer-fraction resampling oracle exactly. The complete fixture compares 600 frames and 1,152,000 stereo sample frames, including a camera program assembled from corrected sources. It checks other-channel inversion, declared midnight/nonzero anchors, saved/MCP schemas, immutable inspection, camera decision recovery and 41 rejection/failure cases. Silence, repetitive signals, inconsistent clocks/polarity, excessive drift, invalid timecode, invalid windows, identities, changed sources and existing destinations are covered. All original source hashes are preserved. The [camera fixture](MULTICAM.md) independently covers selection/edit/render behavior.

A separate retained maximum-search check used eight 8,192-sample windows with 48,000-sample radii and completed in 1.844 seconds on the recorded development machine. It estimated +800 ppm as +793.651 ppm, with residuals below one sample. This is an observed bounded check, not a throughput guarantee or an additional performance acceptance point. Both documented methods and the camera-creation example also executed successfully.

A separate reversed-reference check also found negative audio offsets with at most 0.628 samples of known-clock error. It estimated the reciprocal clock drift as -791.667 ppm, within 15 ppm of the exact -799.361 ppm reference. Its request and report are retained beside the maximum-search evidence.
