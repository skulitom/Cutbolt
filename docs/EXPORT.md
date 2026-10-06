# Range, stream and delivery exports

`export.inspect` validates a selected timeline range, full-quality sources and a declared export profile without writing files. `export.run` writes and verifies a new result through the blocking CLI/library. Inspection is also available through MCP; export execution is not yet queued. Existing `render.run` and queued reference renders keep their original behavior.

Exports preserve source media, ignore proxy preview selection and refuse existing output files. The input remains an FFV1/bgr0 timeline with matching dimensions and 48 kHz stereo PCM. Lossless exports, H.264 delivery and [native tracks](TRACKS.md), including gaps and [editable transitions](TRANSITIONS.md), support the [eight exact native rates](NATIVE_TIMING.md). Native compressed inputs first use [media conversion](CONFORM.md); scene effects are compiled before export. The operation does not alter the supplied snapshot or save a session revision.

## Request and formats

Both commands accept the same fields:

```json
{
  "command": "export.inspect",
  "project": {},
  "input_root": "C:\\DEV\\CutboltData\\media",
  "output_root": "C:\\DEV\\CutboltData\\output",
  "output": "C:\\DEV\\CutboltData\\output\\delivery.mp4",
  "profile": "h264_aac",
  "streams": "audio_video",
  "range": {"start": {"num": 33, "den": 25}, "duration": {"num": 49, "den": 25}},
  "input_transfer": "srgb"
}
```

Replace `project` with a complete snapshot, such as one returned by `session.get`. Roots must be existing absolute directories, and the output's parent must already exist under its allowed root. Inspection also requires an unused output path. Unknown fields, unsupported profiles and incorrect extensions fail explicitly; arbitrary encoder/filter arguments are not accepted.

| Profile | Streams | Extension | Contents |
| --- | --- | --- | --- |
| `reference` | `audio_video` | `.mkv` | FFV1/bgr0 and PCM16 stereo |
| `reference` | `video` | `.mkv` | FFV1/bgr0 only |
| `reference` | `audio` | `.wav` | PCM16 stereo only |
| `h264_aac` | `audio_video` | `.mp4` | H.264 High and AAC-LC stereo |
| `h264_aac` | `video` | `.mp4` | H.264 High only |
| `h264_aac` | `audio` | `.m4a` | AAC-LC stereo in an MP4 container |
| `png_mov` | `audio_video` or `video` | `.mov` | Lossless RGB PNG video and optional PCM16 |
| `png_sequence` | `audio_video` or `video` | New `.frames` directory | Numbered RGB PNGs, complete timing/identity manifest and optional PCM WAV |

The new PNG profiles require explicit source transfer interpretation, preserve encoded RGB values and accept up to DCI 4K. See [native-rate export formats](EXPORT_FORMATS.md) for dimensions, numbering and complete-directory publication. Their broader acceptance passes full verification and earns E05 extended.

Omit `range` or use null to export the whole timeline. Otherwise `start` and `duration` are exact nonnegative rational seconds on the supported native project frame boundaries; duration must be positive and the half-open interval must fit wholly inside the sequence. Ranges can cross cuts and gaps, start/end on any frame, or contain one frame. Output time starts at zero. Audio selection follows the same rational range at 48 kHz. Whole samples are required, so 30000/1001 and 60000/1001 cuts use multiples of five video frames; 25 fps retains exactly 1,920 sample frames per video frame. There is no implicit rounding, end padding or clamping of the requested range.

The selected range is limited to 180,000 frames. Ranges with more than 64 intersecting clips or gaps render as exactly joined chunks ([long timelines](USAGE.md#long-timelines)). Native tracks can render entirely empty intervals as black/silence. Transition ranges retain their original interval clock and required source handles, including an endpoint clip whose body lies outside the selected range. Existing source inspection, filter-argument and tool-time limits still apply; the size cap does not establish long-form performance. Audio-only exports still validate/render the selected reference timeline before extracting audio. They do not add an unrelated WAV-only project model.

## Fixed delivery preset

The version-1 `h264_aac` profile selects the existing external FFmpeg build's `libx264` encoder and native AAC encoder. It remains the default fixed preset. Optional [explicit quality, two-pass bitrate, compatibility and audio settings](DELIVERY_CONTROLS.md) produce version-2 receipts and have separate acceptance evidence.

| Setting | Value |
| --- | --- |
| Video rate | The timeline's native rate, unchanged dimensions and square pixels; the MP4 timescale holds a whole number of ticks per frame (12,800 at 25 fps) |
| Size | Even width 2..1920 and even height 2..1080; no automatic resize |
| H.264 | High profile, level 4.0 (4.2 when 1080p runs above about 30 fps), `medium`, CRF 18 |
| Video buffering | Maximum rate 12,000,000 bits/s, buffer 24,000,000 bits |
| GOP | Two seconds of frames at the rounded rate (50 at 25 fps, 60 at 29.97), minimum key interval one second, scene-cut insertion disabled, two B-frames, three reference frames |
| Output pixels | 8-bit YUV 4:2:0, limited range, BT.709 matrix/primaries/transfer, left chroma location |
| AAC | LC, 48 kHz stereo, target 320,000 bits/s, native two-loop coder without perceptual noise substitution, band cut at 20 kHz |
| MP4 | Faststart index, edit lists, 48,000-unit movie clock and 12,800-unit video clock |

x264 runs eight slice threads, coding each frame as up to eight slices, and the RGB-to-YUV scaler runs eight threads. Both counts are fixed rather than taken from the host, so the encoded stream is the same on every machine with the recorded build; the scaler's threads only divide rows and change no value. x264's frame threads would be slightly faster, but under the VBV cap their rate control depends on when frames arrive, and repeated exports of one timeline differed. Slices cost about 4 % more bytes at the same CRF. Exports made before 5 October 2026 used one thread and one slice, so their pictures can differ slightly from a new export of the same timeline; with one thread the new path decodes bit-identically to the old one, and audio is identical. The report's `video.slice_threads` records the count. Source chapters and copied metadata are removed; output encoder/container metadata is still generated. Binary files are not promised to be identical across tool builds. Decoded repeatability is tested on the recorded build. The optional controls have a bounded software/CUDA compatibility matrix; broader browser/device playback remains unverified.

### Explicit color interpretation

H.264 exports containing video need a transfer, `"srgb"` or `"bt709"`. Declare it once for the project with the `project.transfer` operation, or pass `input_transfer` with each export. A request value that contradicts the project's declaration is rejected, and the plan and receipt report the transfer used. Omit `input_transfer` for reference exports and audio-only delivery.

Choosing between them: `bt709` passes the timeline's encoded RGB values through unchanged before the matrix conversion, so ordinary players show approximately the code values that were authored. Prefer it for screen-designed material such as PNG artwork, pixel art, text and UI captures. `srgb` converts transfer functions (sRGB decode, then BT.709 encode), which lowers dark and mid-tone code values, so typical players display shadows darker than designed; in one dark scene a sky value of about 28 dropped to about 13 levels, while `bt709` stayed within 1.5 levels of the design. Use `srgb` only when the downstream workflow explicitly expects that conversion. This is an explicit interpretation of the reference timeline's encoded RGB values; it does not infer a per-asset color space from names or metadata. The caller must supply a consistently interpreted sequence. Existing scene RGB normally uses the declared sRGB scene contract; media conformed from BT.709 sources can retain BT.709-encoded RGB values. Use [explicit SDR normalization](COLOR.md) to bring mixed interpretations into one working transfer before assembly. The timeline does not infer or enforce that normalization automatically.

For `srgb`, the engine specifies the public piecewise sRGB decode followed by the BT.709 opto-electronic transfer function. Each resulting encoded RGB channel is rounded to an 8-bit value before the matrix conversion. For `bt709`, encoded RGB is unchanged at this step. Both paths then apply a declared BT.709 RGB-to-YCbCr matrix and limited-range conversion, using bilinear chroma reduction with horizontal position 0 and vertical position 128 in units of 1/256 luma pixels. H.264 receives YUV 4:2:0 and matching color tags. Conversion is performed, not merely indicated by changing tags.

The original transfer expressions run through FFmpeg's RGB lookup filter; the matrix/subsampling uses its software scaler. These choices avoid relying on an unspecified transfer-conversion default. Precision is intentionally 8-bit and chroma is subsampled. This does not implement project-wide color management, ICC/camera/log interpretation, HDR, gamut mapping or a display/viewing transform.

The numerical acceptance reference uses public sRGB facts from [W3C CSS Color 4 section 10.2](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#predefined-sRGB) and BT.709 transfer/matrix/range facts from [ITU-R BT.709-6, sections 1 and 3](https://www.itu.int/rec/R-REC-BT.709-6-201506-I/en). No specification copy or third-party implementation is included. Filter interfaces are documented by FFmpeg's [RGB lookup](https://ffmpeg.org/ffmpeg-filters.html#lut_002c-lutrgb_002c-lutyuv) and [scale](https://ffmpeg.org/ffmpeg-filters.html#scale-1) references.

### AAC peaks

Perceptual noise substitution has been off since 6 October 2026, and since later that day the band stops at 20 kHz at 320 kb/s. The receipt's `audio` records `"coder": "twoloop"`, `"noise_substitution": false` and `cutoff_hz`: 20000 at 320 kb/s, `null` at 192 and 256 kb/s, where the encoder chooses its band. With noise substitution on, the native encoder replaced noise-like high bands with generated noise, which decoded as bursts well above the input on percussive onsets and strong top-octave content. Over 15 production mixes, the decoded true peak rose by up to 7.96 dB above the mix's own, with full-scale clipping in four of them, often at quiet moments where no limiter ceiling can help. Without it the same mixes rose by at most 0.22 dB and never clipped. At these fixed bitrates the files are the same size either way, within 0.1 %.

It also changes waveform fidelity, mostly for the better:
- **Real mixes.** The same 15 mixes at 320 kb/s decode 2.3-4.5 dB closer to the mix (SNR), and four of them gained 0.7-2.3 dB at 192 and 256 kb/s.
- **Synthetic steady tones.** These lost: the delivery fixture's 40 ms one-frame clip at 320 kb/s fell from 33.9 to 20.5 dB (a clip that is almost all encoder start-up), and the delivery-controls fixture's tones at 192 kb/s fell from 33.3 to 26.0 dB. Longer clips at 320 kb/s still measure at least 37.6 dB.
- **Gates.** Those two cases have measured floors instead of 28 dB. The 192 kb/s tones have 25 dB. The one-frame clip had 20 dB until the 20 kHz cutoff lifted it to 25.9 dB, and now has 25 dB. Every other case keeps 28 dB.

**The 20 kHz band.** At 320 kb/s the native encoder codes up to 22 kHz. On heavily limited mixes strong near the top of the band, those two kilohertz moved decoded peaks:
- **Hard mixes.** Over 31 of them (a square-wave fixture bed limited at 20 ceilings, the part-two demo at 11), the codec added 0.57 dB to the mix's true peak on average and up to 1.43 dB. With the band cut at 20 kHz it added 0.25 dB on average, less on 29 of them, and at most 0.48 dB on all but one: the demo limited at -7 dBFS, which read 1.02 dB (1.12 dB before).
- **Production mixes.** On the 15 mixes above, the largest overshoot fell from 0.22 to 0.15 dB.
- **Fidelity.** The bits move to the band below. Those 15 mixes decode 2.0-3.1 dB closer to the mix below 18 kHz, and files are 1.1-3.5 % smaller. Their full-band SNR is 2.9-5.4 dB lower, because their synthetic beds carry -27 to -34 dB of their power above 20 kHz, which is no longer coded. The delivery fixture's tones, all below 3 kHz, changed by -0.23 to +5.4 dB.
- **Lower rates.** At 192 and 256 kb/s the encoder narrows its band frame by frame from the bits it has, and a fixed cutoff measured mixed. At 192 kb/s, 20 kHz raised the fixture bed's overshoot from 0.35 to 1.23 dB. At 256 kb/s it lowered one bed trial's from 0.59 to 0.16 dB and raised another's from 0.62 to 1.04 dB. Those rates keep the encoder's own band.

A lossy encode still moves peaks by a signal-dependent amount, more on heavily limited mixes and on content strong near the top of the band. On one such synthetic mix, -1.3 and -1.5 dBFS limiter ceilings decoded 0.46 and 1.19 dB higher with the 22 kHz band, and 0.13 and 0.40 dB higher with the 20 kHz one. A delivery with a true-peak target should therefore be measured after encoding: `export.review` reports the delivered file's `true_peak_dbtp`, and the [production coordinator](PRODUCTION.md#the-delivered-peak) trial-encodes its mix before the export. Exports made earlier, with noise substitution or with the 22 kHz band, can decode differently from a new export of the same timeline. The [dynamics fixture](AUDIO.md#timeline-limiters) checks that percussive bursts, which noise substitution decoded 2.3 dB over the timeline's true peak, now decode within 0.25 dB of it.

### AAC timing and padding

AAC is lossy and encodes blocks of 1,024 samples. The output records a 1,024-sample priming interval with skip/edit metadata, and an exact presentation duration equal to the selected timeline range. The verified decoder skips the priming interval. It can still return a final partial block as padding beyond the presentation end.

Receipts distinguish `audio_samples` (the exact intended presentation length), `verification.decoded_audio_samples`, `aac_priming_samples` and `audio_tail_padding_samples`. The tested decoder returns `ceil(audio_samples/1024)*1024` sample frames, so trailing padding is 0..1023. It must not be interpreted as added timeline content. Verification checks exact track duration and priming metadata as well as decoded count. Consumers extracting raw PCM must use the declared presentation length; use the reference WAV profile when exact uncompressed samples are required.

The one-frame fixture is 1,920 presentation sample frames and decodes to 2,048; an eight-frame fixture is exactly 15,360 and needs no trailing padding. Audio remains lossy inside that interval. The 320 kb/s setting was selected after the initial 192 kb/s candidate failed the short-clip quality threshold; the threshold was retained. Since noise substitution was turned off, that one-frame clip has a measured floor instead, now 25 dB (see [AAC peaks](#aac-peaks)).

## Reviewing the output

`export.review`, queued with `job.start`, checks a delivered file against its project. It produces a contact sheet, a small preview copy, black runs, loudness over time, frame and sample counts, and the words heard against the words intended. See [USAGE.md](USAGE.md#reviewing-a-delivered-cut).

## Verification and publication

Inspection reports the selected range, dimensions, stream policy, source identities, full-quality selection and profile settings. It does not create an intermediate or output file.

Audio-only exports (`streams: "audio"`) never decode, composite or encode pictures. Sequential timelines are trimmed and concatenated directly from source PCM with exact silence for gaps; track timelines mix only their enabled audio tracks with the same placement and saturation rules as a full render. Sources are still identity-checked and their stream metadata, audio samples and video timestamps validated, but video timing is read from FFV1 packets instead of decoding every frame. The intermediate is a lossless PCM WAV, so cost scales with audio duration rather than picture size.

H.264 exports with video are encoded straight from the timeline. No lossless intermediate is written, checked and decoded again:

1. One FFmpeg run executes the selected full-quality reference graph and converts and encodes its picture and mix. With engine-composited [transitions](TRANSITIONS.md#previews-ranges-and-output) or [overlays](TRACKS.md), the engine compositor feeds that run's picture on stdin instead.
2. The same run also returns exactly what it encoded. The packed RGB24 frames go to the engine on stdout, which counts and hashes them as they arrive, and the stereo PCM s16le samples go to a scratch file. Both must hold exactly the range's frames and samples, at the timeline's exact clock. The receipt reports `verification.encoder_input: "streamed"`, `timeline_video_sha256` and `timeline_audio_sha256`. These equal the `decoded_video_sha256` and `decoded_audio_prefix_sha256` of a `reference` export of the same range, so a delivery can be tied to a lossless render without making one. Two-pass encodes run the graph twice, and both passes must receive identical frames.

Reference and PNG exports, audio-only delivery, and ranges rendered as joined chunks (more than 64 clips per graph) still compile the selected interval into a lossless intermediate under a private scratch directory beside the output, verify it, then extract reference streams or encode from it.

Every export then:

1. Checks container, exact stream count, codec, dimensions, frame rate and every decoded video's presentation timestamp. Delivery metadata must match the declared profile and exact track duration.
2. Decodes complete video/audio, checks counts and records decoded digests. Reference video and PCM must match the intermediate's decoded samples exactly. Delivery audio padding is reported separately. These decodes run at the same time.
3. Rechecks every used original source hash, beside those decodes, then publishes by a no-overwrite hard link and cleans up known scratch files.

Runtime validation proves structure, timing, successful decoding and reference equality. It does not compute a content-quality score for every lossy export. Quality thresholds are exercised by the original acceptance fixture. The receipt includes output SHA-256, original source hashes, exact tool versions, decoded RGB hash and presentation-PCM-prefix hash. The RGB digest uses the recorded FFmpeg build's ordinary RGB24 decoding; its codec/scaler behavior remains an external dependency.

Missing tools, malformed input, changed sources or validation failure leave no final output. Existing outputs are never replaced. A process crash can leave scratch files for inspection; automatic export retry/recovery and queue integration remain open. An intermediate and the validation PCM require temporary disk space, in addition to the final output; H.264 video needs only the timeline and validation PCM.

### Export speed

Measured on the progress demo's final cut (80.64 s of 1080p25: 14 FFV1 scene shots, 7 narration clips and a ducked music bed) with release builds, back to back on the development machine while other sessions kept its 32 threads 8-37 % busy before most runs (99 % before one). Each figure is the median of 3 runs with warm source inspections. With overlays, each compositor decoder also reads ahead on its own thread.

| Export | Intermediate (`d3c9140`) | Streamed |
| --- | ---: | ---: |
| Final cut, no overlays | 115.2 s | 34.0 s |
| Same timeline with the full-length caption overlay and a picture-in-picture clip (engine-composited) | 118.0 s | 30.9 s |

The old path spent its time in the single-threaded FFV1 encode of the intermediate (83 s here) and the single-threaded x264 encode (49 s). The streamed run takes about 21 s, bounded by FFmpeg's one graph thread rather than by x264, so a faster x264 preset or hardware encoder gains little on this material. Verification takes about 6 s, bounded by SHA-256 of the 12.5 GB of decoded RGB (about 1.7 GB/s on one core).

## Runnable workflow and evidence

```powershell
cargo build --locked
python -X utf8 tests/delivery.py --output C:\DEV\CutboltData\new-delivery-test
```

The fixture leaves `project.json`, original media and verified exports outside the repository. This workflow inspects a saved snapshot and produces a new delivery file:

```python
from pathlib import Path
import json, subprocess

root = Path(r"C:\DEV\CutboltData\new-delivery-test")
request = {"command":"export.inspect",
    "project":json.loads((root / "project.json").read_text(encoding="utf-8")),
    "input_root":str(root / "sources"), "output_root":str(root / "output"),
    "output":str(root / "output" / "documented-delivery.mp4"),
    "profile":"h264_aac", "streams":"audio_video", "input_transfer":"srgb",
    "range":{"start":{"num":33,"den":25}, "duration":{"num":49,"den":25}}}
exe = r"C:\DEV\Cutbolt\target\debug\cutbolt.exe"
def call(value):
    done = subprocess.run([exe], input=json.dumps(value), text=True,
                          encoding="utf-8", capture_output=True, check=True)
    return json.loads(done.stdout)["result"]
print(call(request))
request["command"] = "export.run"
print(call(request)["verification"])
```

Acceptance uses original RGB charts, motion, binary frame IDs and different stereo tones. Exact Fraction slicing checks source intervals, cuts/gaps, first/last frames, single-frame ranges and full sequences. Lossless RGB and PCM must match byte for byte. Independent Decimal transfer/matrix calculations check flat color patches within four YUV levels. Delivery RGB must reach at least 30 dB PSNR against the original transfer-converted frames; presentation PCM must reach at least 28 dB SNR with best alignment at zero samples. These are fixture thresholds, not guarantees for arbitrary footage or listening quality.

The fixture checks both explicit input transfers, AAC priming/padding, stream-only outputs, metadata removal, faststart, deterministic decoded replay, full-HD/minimum dimensions, odd-size reference output, offline proxy selection, MCP inspection and saved-session immutability. Controlled tool failures corrupt only generated staged output or temporarily change an original synthetic fixture source; publication must fail, and the fixture restores its controlled source change. No third-party media, models, presets or implementation are used. These tests target **E02 basic and E05 basic**; broader format/device and recovery criteria remain unchanged and open.

Retained run `C:\DEV\CutboltData\delivery-20261003-04` checked **790 decoded video frames and 1,514,880 presentation stereo sample frames across 26 exports**, plus **28 rejected/failure cases**. Lossless samples matched exactly. The minimum measured delivery RGB PSNR was **34.351 dB**, the maximum flat-patch YUV error was **zero levels**, and the minimum nonsilent AAC SNR was **33.882 dB**, with best alignment at zero samples. The documented inspection/export example also ran successfully against that fixture. Full-suite evidence and current score are recorded in [verification/latest.json](../verification/latest.json) and [PROGRESS.md](PROGRESS.md).
