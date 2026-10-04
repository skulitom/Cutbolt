# Range, stream and delivery exports

`export.inspect` validates a selected timeline range, full-quality sources and a declared export profile without writing files. `export.run` writes and verifies a new result through the blocking CLI/library. Inspection is also available through MCP; export execution is not yet queued. Existing `render.run` and queued reference renders keep their original behavior.

Exports preserve source media, ignore proxy preview selection and refuse existing output files. The input remains an FFV1/bgr0 timeline with matching dimensions and 48 kHz stereo PCM. Lossless sequential exports support the [eight exact native rates](NATIVE_TIMING.md); H.264 delivery and [native tracks](TRACKS.md) retain 25 fps, including gaps and [editable transitions](TRANSITIONS.md). Native compressed inputs first use [media conversion](CONFORM.md); scene effects are compiled before export. The operation does not alter the supplied snapshot or save a session revision.

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

The selected range is limited to 180,000 frames. Sequential exports support 1–64 intersecting clips/gaps; native tracks support at most 64 model clips and can render entirely empty intervals as black/silence. Transition ranges retain their original interval clock and required source handles, including an endpoint clip whose body lies outside the selected range. Existing source inspection, filter-argument and tool-time limits still apply; the size cap does not establish long-form performance. Audio-only exports still validate/render the selected reference timeline before extracting audio. They do not add an unrelated WAV-only project model.

## Fixed delivery preset

The version-1 `h264_aac` profile selects the existing external FFmpeg build's `libx264` encoder and native AAC encoder. It remains the default fixed preset. Optional [explicit quality, two-pass bitrate, compatibility and audio settings](DELIVERY_CONTROLS.md) produce version-2 receipts and have separate acceptance evidence.

| Setting | Value |
| --- | --- |
| Video rate | 25 fps, unchanged dimensions and square pixels |
| Size | Even width 2..1920 and even height 2..1080; no automatic resize |
| H.264 | High profile, level 4.0, `medium`, CRF 18 |
| Video buffering | Maximum rate 12,000,000 bits/s, buffer 24,000,000 bits |
| GOP | 50 frames, minimum key interval 25, scene-cut insertion disabled, two B-frames, three reference frames |
| Output pixels | 8-bit YUV 4:2:0, limited range, BT.709 matrix/primaries/transfer, left chroma location |
| AAC | LC, 48 kHz stereo, target 320,000 bits/s, native two-loop coder with perceptual noise substitution enabled |
| MP4 | Faststart index, edit lists, 48,000-unit movie clock and 12,800-unit video clock |

Encoding uses one video thread. Source chapters and copied metadata are removed; output encoder/container metadata is still generated. Binary files are not promised to be identical across tool builds. Decoded repeatability is tested on the recorded build. The optional controls have a bounded software/CUDA compatibility matrix; broader browser/device playback remains unverified.

### Explicit color interpretation

H.264 exports containing video require `input_transfer: "srgb" | "bt709"`. Omit it for reference exports and audio-only delivery.

Choosing between them: `bt709` passes the timeline's encoded RGB values through unchanged before the matrix conversion, so ordinary players show approximately the code values that were authored. Prefer it for screen-designed material such as PNG artwork, pixel art, text and UI captures. `srgb` converts transfer functions (sRGB decode, then BT.709 encode), which lowers dark and mid-tone code values, so typical players display shadows darker than designed; in one dark scene a sky value of about 28 dropped to about 13 levels, while `bt709` stayed within 1.5 levels of the design. Use `srgb` only when the downstream workflow explicitly expects that conversion. This is an explicit interpretation of the reference timeline's encoded RGB values; it does not infer a per-asset color space from names or metadata. The caller must supply a consistently interpreted sequence. Existing scene RGB normally uses the declared sRGB scene contract; media conformed from BT.709 sources can retain BT.709-encoded RGB values. Use [explicit SDR normalization](COLOR.md) to bring mixed interpretations into one working transfer before assembly. The timeline does not infer or enforce that normalization automatically.

For `srgb`, the engine specifies the public piecewise sRGB decode followed by the BT.709 opto-electronic transfer function. Each resulting encoded RGB channel is rounded to an 8-bit value before the matrix conversion. For `bt709`, encoded RGB is unchanged at this step. Both paths then apply a declared BT.709 RGB-to-YCbCr matrix and limited-range conversion, using bilinear chroma reduction with horizontal position 0 and vertical position 128 in units of 1/256 luma pixels. H.264 receives YUV 4:2:0 and matching color tags. Conversion is performed, not merely indicated by changing tags.

The original transfer expressions run through FFmpeg's RGB lookup filter; the matrix/subsampling uses its software scaler. These choices avoid relying on an unspecified transfer-conversion default. Precision is intentionally 8-bit and chroma is subsampled. This does not implement project-wide color management, ICC/camera/log interpretation, HDR, gamut mapping or a display/viewing transform.

The numerical acceptance reference uses public sRGB facts from [W3C CSS Color 4 section 10.2](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#predefined-sRGB) and BT.709 transfer/matrix/range facts from [ITU-R BT.709-6, sections 1 and 3](https://www.itu.int/rec/R-REC-BT.709-6-201506-I/en). No specification copy or third-party implementation is included. Filter interfaces are documented by FFmpeg's [RGB lookup](https://ffmpeg.org/ffmpeg-filters.html#lut_002c-lutrgb_002c-lutyuv) and [scale](https://ffmpeg.org/ffmpeg-filters.html#scale-1) references.

### AAC timing and padding

AAC is lossy and encodes blocks of 1,024 samples. The output records a 1,024-sample priming interval with skip/edit metadata, and an exact presentation duration equal to the selected timeline range. The verified decoder skips the priming interval. It can still return a final partial block as padding beyond the presentation end.

Receipts distinguish `audio_samples` (the exact intended presentation length), `verification.decoded_audio_samples`, `aac_priming_samples` and `audio_tail_padding_samples`. The tested decoder returns `ceil(audio_samples/1024)*1024` sample frames, so trailing padding is 0..1023. It must not be interpreted as added timeline content. Verification checks exact track duration and priming metadata as well as decoded count. Consumers extracting raw PCM must use the declared presentation length; use the reference WAV profile when exact uncompressed samples are required.

The one-frame fixture is 1,920 presentation sample frames and decodes to 2,048; an eight-frame fixture is exactly 15,360 and needs no trailing padding. Audio remains lossy inside that interval. The 320 kb/s setting was selected after the initial 192 kb/s candidate failed the short-clip quality threshold; the threshold was retained.

## Verification and publication

Inspection reports the selected range, dimensions, stream policy, source identities, full-quality selection and profile settings. It does not create an intermediate or output file.

Audio-only exports (`streams: "audio"`) never decode, composite or encode pictures. Sequential timelines are trimmed and concatenated directly from source PCM with exact silence for gaps; track timelines mix only their enabled audio tracks with the same placement and saturation rules as a full render. Sources are still identity-checked and their stream metadata, audio samples and video timestamps validated, but video timing is read from FFV1 packets instead of decoding every frame. The intermediate is a lossless PCM WAV, so cost scales with audio duration rather than picture size.

Rendering then:

1. Compiles the selected full-quality reference interval under a private scratch directory beside the output.
2. Extracts reference streams or encodes the fixed delivery profile.
3. Checks container, exact stream count, codec, dimensions, frame rate and every decoded video's presentation timestamp. Delivery metadata must match the declared profile and exact track duration.
4. Decodes complete video/audio, checks counts and records decoded digests. Reference video and PCM must match the intermediate's decoded samples exactly. Delivery audio padding is reported separately.
5. Rechecks every used original source hash, then publishes by a no-overwrite hard link and cleans up known scratch files.

Runtime validation proves structure, timing, successful decoding and reference equality. It does not compute a content-quality score for every lossy export. Quality thresholds are exercised by the original acceptance fixture. The receipt includes output SHA-256, original source hashes, exact tool versions, decoded RGB hash and presentation-PCM-prefix hash. The RGB digest uses the recorded FFmpeg build's ordinary RGB24 decoding; its codec/scaler behavior remains an external dependency.

Missing tools, malformed input, changed sources or validation failure leave no final output. Existing outputs are never replaced. A process crash can leave scratch files for inspection; automatic export retry/recovery and queue integration remain open. The intermediate and validation PCM require temporary disk space, in addition to the final output.

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
