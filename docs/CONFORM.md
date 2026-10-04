# Validated media import and retiming

`media.conform.inspect` validates a source and reports the exact source frames selected by a recipe. `media.conform` writes a new lossless editing asset using that recipe. Both use explicit local roots and require the original source's SHA-256 and byte count. Sources are never rewritten. The output can enter ordinary saved sessions through `media.add` and can then be trimmed, assembled, previewed or queued for reference export.

Rendering is a blocking CLI/library operation. MCP exposes inspection as `cutbolt_media_conform_inspect`; the existing background queue accepts the completed asset in a reference project. Keep the source, recipe and receipt outside the repository to revise or reproduce the conversion. Session undo changes timeline edits; it does not regenerate an earlier conversion recipe.

The optional [hardware-decode recipe](ACCELERATION.md) selects a local CUDA device for the bounded progressive H.264 source subset, with an explicit unavailable-device policy. Omission uses the CPU. Only source-video decoding moves to the GPU; inspection, color conversion, audio, timeline mapping and output verification retain their existing behavior. Receipts identify the selected backend and measured stages. Hardware acceptance is pending full-suite promotion.

## Legacy source matrix

| Container | Video | Audio | Required interpretation |
| --- | --- | --- | --- |
| MKV | FFV1, `bgr0` | Optional PCM16 | `encoded_rgb` |
| MP4 or MOV | H.264, `yuv420p` | Optional AAC | `bt709_limited`, with matching BT.709 matrix, primaries, transfer and TV-range metadata |
| WAV | None | PCM16 | `color: null` |

Audio is mono or stereo at 24,000, 44,100 or 48,000 Hz. Explicit nonstandard channel layouts fail; absent layout metadata uses channel count. Sources have at most one video and one audio stream and no other streams. Video must be progressive with fixed square-pixel dimensions, zero-origin strictly increasing decoded timestamps and a positive final-frame duration. Audio must begin at zero and remain continuous within 1 ms of its exact decoded sample clock. Both constant and variable video frame rates use decoded timestamps. Packet order and codec keyframes do not determine selected output frames.

RGB accepts absent/unknown or GBR matrix, absent/unknown or full range, absent/unknown or BT.709 primaries, and absent/unknown, BT.709 or sRGB transfer tags. Its encoded channel values are preserved. Limited BT.709 YUV converts to full-range RGB with an explicit matrix/range conversion. Neither path linearizes transfer functions or claims a color-managed working space. HDR, transform side data, rotation tags, interlacing, non-square pixels and other codec/container combinations fail explicitly. The optional [SDR normalization profile](COLOR.md) separately adds explicit transfer conversion, tagged full-range RGB output and checked tagged/untagged inputs. The legacy modes in this table preserve their existing behavior.

The source file must be at most 16 GiB, referenced by a relative path inside an existing absolute input root; its identity is checked with a streamed SHA-256. Sources may have up to 108,000 video frames and one hour of audio. Only what a recipe needs is decoded: forward and freeze mappings stream the selected frame range straight from the decoder, while reverse or other non-monotonic mappings decode one random-access window of at most 4 GiB of RGB into scratch (longer reversed ranges fail with `LIMIT_EXCEEDED`). Audio decodes only the mapped window, at most 512 MiB. Tool timeouts scale with the source duration, up to four hours. Source and output dimensions are limited to 4096 by 2160 and eight million pixels. Full strict decoding checks malformed streams; hashes are checked again after inspection and before publishing output. Tool execution and captured metadata have bounded time/size limits.

## Explicit SDR normalization

Use `source.sdr` and `working_transfer` together to normalize RGB or YUV media before assembling a mixed-source timeline. This adds FFV1/YUV444 MKV and declared full/limited H.264 YUV420 input handling, with strict conflict checks and an explicit policy for absent tags. It bypasses external color-scaler defaults by converting native samples in original Rust code. See [COLOR.md](COLOR.md) for all fields, numerical rules and independent chart evidence.

## Recipe and timing

Example recipe shape (replace identity fields with the actual source values):

```json
{
  "schema_version": 1,
  "id": "shot-fast",
  "source": {
    "file": {"path": "shot.mp4", "sha256": "SOURCE_SHA256", "bytes": 123456},
    "color": "bt709_limited"
  },
  "source_in": {"num": 1, "den": 5},
  "duration": {"num": 1, "den": 1},
  "rate": {"num": 3, "den": 2},
  "reverse": false,
  "freeze": false,
  "width": 640,
  "height": 360,
  "audio": "resample"
}
```

Output is FFV1/bgr0 with 48 kHz stereo PCM16 in MKV, at the recipe's optional `frame_rate`: one of the eight [native rates](NATIVE_TIMING.md), default 25. Converting a 30 or 60 fps source at its own rate keeps every source frame, so placed tracks at that rate show it without judder. Duration must be 1–45,000 exact output frames and a whole number of 48 kHz samples; at 30000/1001, for example, that means multiples of 5 frames. Output frames are streamed to the encoder; no raw output buffer is kept. `rate` is an exact positive rational from 1/16 to 16. All times use the engine's checked rational arithmetic.

At output frame `n`, forward playback samples source time `source_in + n/frame_rate * rate`; reverse uses subtraction. Freeze samples `source_in` for every output frame and requires unit rate and `reverse: false`. Selection uses the latest decoded source timestamp at or before that time plus half a tick of the source's video time base. Container timestamps are rounded to that time base: Matroska rounds to whole milliseconds, so a 24 fps frame at 41.67 ms is stored as 42 ms. A frame within half a tick after the output time is therefore the frame for it. At 25 fps output times are whole milliseconds, so this changes no 25 fps selection from a millisecond-based source. Every selected time must lie within the source's video extent. These legacy fields use discrete frame selection. Optional `remap` adds [variable speed ramps and timestamp-based frame interpolation](REMAPPING.md). Resize uses nearest sampling at `floor(x * source_width / output_width)` and the equivalent vertical coordinate. Audio-only sources produce black video.

`audio: "resample"` converts forward audio by linear sample interpolation with exact rational positions, rounding once to the nearest PCM16 value with ties away from zero. Mono duplicates into both channels; stereo order is preserved. Source-in must fall on a source audio sample boundary, and the full mapped audio interval must fit decoded samples. **Changing rate changes pitch.** Reverse and freeze require `audio: "mute"`. Missing or muted audio produces exact silence. Pitch-preserving stretch and reverse audio remain open.

Output roots and parent directories must already exist, and output must be an unused `.mkv` path within that root. Before publication, the engine verifies the reference profile, frame/sample counts and SHA-256 of all decoded RGB and PCM against running hashes of what it encoded. Receipts contain source metadata/identity, selected frame indices/timestamps, output identity, tool versions and a ready-to-add `asset` with bound identity. Normal failure removes owned scratch files and does not publish a partial output; a process crash can leave scratch files for inspection.

## LUT application

An optional `lut: {file: {path, sha256, bytes}, interpolation}` applies a validated external 1D/3D table after explicit SDR normalization and before resizing. It requires `source.sdr` and `working_transfer`; the older color path cannot infer a LUT's input space. When `remap` is present, normalization and the table are applied to each contributing source frame before temporal blending. Table identity is checked again before publication. See [LUTS_SCOPES.md](LUTS_SCOPES.md) for format, interpolation, clipping and numerical frame scopes.

## Runnable fixture

Use a fresh external directory:

```powershell
cargo build --locked
python tests/conform.py --output C:\DEV\CutboltData\my-conform-test
$demo = 'C:\DEV\CutboltData\my-conform-test'
$recipe = Get-Content -Raw "$demo\recipe.json" | ConvertFrom-Json
@{command='media.conform.inspect'; recipe=$recipe; input_root="$demo\sources"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
@{command='media.conform'; recipe=$recipe; input_root="$demo\sources";
    output_root="$demo\output"; output="$demo\output\extra.mkv"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The original generator covers MKV/MP4/MOV/WAV, 24/25/30 and 30000/1001 fps, VFR, H.264 B-frames, supported audio rates/layouts, speed bounds, reverse, freeze and resize. Independent Fraction clocks and timestamp lookup compare every decoded output pixel/sample, followed by saved-session trimming/assembly and MCP inspection. Codec decoding is delegated to the same external FFmpeg boundary on both sides; the test independently validates selection, timing, resizing and resampling, not FFmpeg's codec implementation. Rejections cover malformed files, unsupported metadata, range/alignment errors and changed identities; source and output preservation are checked. [Proxy previews](PROXIES.md) use completed reference assets. Broader mixed-format timeline edge cases remain separate work.
