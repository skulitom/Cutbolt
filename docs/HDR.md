# High-bit-depth and HDR conversion

`hdr.inspect` validates an identity-bound recipe without writing output. `hdr.conform` creates a separate FFV1/PCM file from native 10/12/16-bit samples, with explicit transfer, gamut, exposure and tone policy. HDR output uses RGB16 and standard declared display metadata; SDR RGB8 output returns an ordinary editing asset for saved sessions, previews and delivery. The general timeline remains 8-bit SDR. This explicit conversion path does not change the precision of existing timeline effects.

## Commands and recipe

Both commands accept `recipe` and `input_root`. Conversion also requires `output_root` and an unused absolute `.mkv` `output` path. Roots must already exist outside the repository. The CLI accepts the request as JSON; Rust callers use the shared dispatcher or `hdr` module. The read-only `cutbolt_hdr_inspect` MCP tool uses the same recipe and result. Conversion is synchronous CLI/library work, without a background queue or network service.

```json
{
  "schema_version": 1,
  "id": "tone-mapped-shot",
  "source": {
    "file": {"path":"shot.mkv", "sha256":"SOURCE_SHA256", "bytes":123456},
    "encoding": {
      "transfer":"pq", "primaries":"bt2020",
      "display":{"peak_nits":1000, "black_millinits":0}
    },
    "matrix":"rgb", "range":"full", "missing_tags":"reject"
  },
  "output": {
    "encoding": {
      "transfer":"srgb", "primaries":"bt709",
      "display":{"peak_nits":100, "black_millinits":0}
    },
    "depth":"rgb8"
  },
  "source_in":{"num":0,"den":1},
  "duration":{"num":1,"den":1},
  "rate":{"num":1,"den":1},
  "reverse":false, "freeze":false,
  "width":640, "height":360,
  "exposure_milliev":0,
  "tone":{"mode":"reinhard", "reference_white_nits":203, "source_peak_nits":1000},
  "audio":"resample"
}
```

Every field shown is required. Unknown fields/modes are rejected. The example's identity and duration are placeholders for the caller's own supported media; the runnable synthetic example below supplies real values.

| Field | Supported contract |
| --- | --- |
| `encoding.transfer` | `pq`, `hlg`, `srgb`, `bt709` |
| `encoding.primaries` | `bt709` or `bt2020`, both D65; PQ/HLG require `bt2020` |
| `display.peak_nits` | Integer 48..10000; SDR 48..400; HLG 400..2000 |
| `display.black_millinits` | Integer 0..1000; 5 means 0.005 nit |
| `source.matrix` | `rgb`, `bt709`, `bt2020_ncl`; YUV matrix must match primaries |
| `source.range` | `full` or `limited`, scaled to the native bit depth |
| `source.missing_tags` | `reject` or explicit `use_declared`; known conflicts always fail |
| `output.depth` | `rgb16` or `rgb8`; RGB8 requires SDR and BT.709 primaries |
| `exposure_milliev` | Integer -8000..8000; 1000 means one stop |
| `audio` | `resample` or `mute`; reverse/freeze require mute |

`source.encoding.display` is an explicit interpretation. Known source mastering minimum/maximum luminance must agree with it; mastering primary coordinates describe a display and are not substituted for content primaries. Content-light metadata is reported but never used to infer the tone mapper's source peak. Missing display metadata does not establish a measured peak. The user must supply the intended values.

## Color calculation

The engine decodes native integer samples without an RGB8 intermediate. Full-range code normalization uses `(2^bits)-1`; limited range scales the 8-bit luma endpoints 16/235 and chroma span 224 by `2^(bits-8)`. YUV444 uses the declared non-constant-luminance matrix. Reconstructed encoded RGB is clamped to 0..1 before transfer conversion; extended headroom and negative signal values are outside this profile.

Original float64 calculations convert PQ to absolute display nits using the public PQ EOTF/inverse. HLG uses the public OETF, display system gamma `1.2 + 0.42 log10(peak/1000)` and black-level lift from the declared display. The 400..2000-nit HLG bound matches the selected gamma formula. sRGB/BT.709 decode relative light scaled by declared display peak. Nonzero black affects HLG's lift; it does not add a black offset to PQ/SDR transfer equations.

Exposure multiplies absolute light by `2^(exposure_milliev/1000)`. The engine derives a primary matrix from public chromaticities and D65, converts linear RGB and clips negative components. It does not perform perceptual gamut compression. Tone mapping follows that conversion:

| Tone mode | Defined behavior |
| --- | --- |
| `preserve` | Preserve absolute light before target encoding; permitted for HDR output and SDR-to-SDR/HDR conversions. HDR-to-SDR requires an explicit mapper. |
| `clip` | SDR output only. Divide each component by `reference_white_nits`, clamp to 0..1, then multiply by target display peak. |
| `reinhard` | SDR output only. With `x = max(R,G,B)/white` and `p = source_peak_nits/white`, map `y = min(1, x*(1+x/(p*p))/(1+x))`; scale all components by `y/x` and target peak/white. Zero remains zero. |

`reference_white_nits` is 48..1000; Reinhard's `source_peak_nits` is at least white and at most 10000. This is an explicit project-defined global extended-Reinhard policy using maximum RGB. It is not automatic exposure selection or a local tone operator. Target transfer encoding follows tone mapping. Output is clamped to 0..1, rounded once to RGB8/16, and reports pixels outside the final output range. That count does not include intentional earlier gamut clipping or the mapper's own clipping.

## Timing, outputs and preservation

Input requires one progressive, square-pixel FFV1 video stream and one PCM16 48 kHz stereo audio stream in MKV, starting at zero with continuous 25 fps timing and equal durations. Supported high-depth layouts are planar RGB and YUV444 at 10, 12 or 16 bits, little endian. SDR `bgr0` input also permits explicit conversion into HDR. Rotation, dynamic side data, subsampled HDR, other codecs and variable/fractional frame rates are rejected by this path.

Source identity is SHA-256 plus byte length, checked before work, after inspection and before publication. Input size is bounded at 64 MiB. Source and output each require 1..1500 frames (60 seconds), at most 4096x2160 and eight million pixels, and at most 4 GiB decoded/raw pixels. The output bound conservatively assumes RGB16 even for RGB8. Individual external operations have fixed timeouts; these are limits, not throughput guarantees. Processing uses frame buffers and scratch files; decoded PCM is held in memory within the duration bound.

Duration must land on a 25 fps output boundary. Each exact rational source time chooses the latest source frame at or before it. Speed is 1/16..16; freeze requires unit speed and cannot combine with reverse. Forward audio uses exact rational linear sample interpolation and an explicit pitch-changing resample policy; `source_in` must align to a 48 kHz sample when audio is enabled. Nearest-neighbor resizing uses top-left source selection. There is no motion interpolation, pitch preservation or high-quality resizer in this profile.

Every output is tagged full-range RGB with declared primaries/transfer. PQ/HLG RGB16 output additionally writes standard Matroska mastering fields for declared primary/white coordinates and display peak/black. These are authored declarations, not a measurement of a mastering monitor. No MaxCLL/MaxFALL values are invented. The writer modifies only reserved space in its own newly encoded temporary file, preserving element lengths and all block/seek offsets.

Before publication the engine decodes the result, compares complete video/audio hashes with the generated raw samples and checks dimensions, layout, timing, tags and HDR display fields. It rechecks source identity, then publishes without overwriting an existing path. Normal failures remove scratch output. Original source files are never modified. Inspect/output reports include source selection, assumptions, precision, tone policy and actual external tool versions.

RGB8 returns `asset` for `media.add` in the existing saved-session contract. RGB16 returns `media_identity` for another HDR recipe, without advertising a general-timeline asset. HDR distribution codecs, dynamic metadata, calibrated display output, alpha, ICC profiles and general high-precision timeline effects remain outside this bounded path.

## Runnable retained fixture

Use a fresh external directory; the test creates original source charts and audio, converts them and saves `recipe.json`:

```powershell
python -X utf8 tests/hdr.py --output C:\DEV\CutboltData\my-hdr-demo
$demo = 'C:\DEV\CutboltData\my-hdr-demo'
$recipe = Get-Content -Raw "$demo\recipe.json" | ConvertFrom-Json
@{command='hdr.inspect'; recipe=$recipe; input_root="$demo\sources"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
@{command='hdr.conform'; recipe=$recipe; input_root="$demo\sources";
    output_root="$demo\output"; output="$demo\output\another-hdr.mkv"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The original fixture compares all decoded pixels using 50-digit Decimal transfer/tone equations and exact Fraction Gaussian primary conversion; time/audio expectations use exact rational arithmetic. It covers PQ/HLG native RGB10/12/16, full/limited YUV444, every 16-bit code for both HDR transfers, HLG black/peak variations, exposure, gamut conversion, both tone modes, SDR upconversion, re-import, seeking, retiming, reverse/freeze and resize. SDR results pass through saved sessions, MCP inspection, exact timeline previews and delivery exports. Invalid metadata/recipes, unchanged existing outputs and a controlled changed-source failure are checked separately.

Acceptance thresholds are at most two RGB16 code values or one RGB8 value; identity conversions and PCM are exact. An external `zscale` PQ-to-linear reference is compared in absolute nits with tolerance `0.5 + 0.0002 * expected_nits`, covering its float32 conversion. Codec decoding is supplied by the same separately installed media tools; independent equations test this engine's processing, not the decoder implementation. `ffprobe` independently recognizes the standard authored display metadata.

The retained run at `C:\DEV\CutboltData\hdr-20261003-03` compared 131 frames and 251,520 stereo sample frames, rejected 31 cases and exercised all 65,536 code values per channel for both PQ and HLG. This includes a two-row SDR conversion, saved timeline and selected-range preview, plus single-pixel, tall, wide and 1080p HDR outputs. Maximum observed RGB16 error was one code value; RGB8, identities and PCM were exact. The three external transfer comparisons stayed below 0.489 nit. Full verification reruns the fixture before assigning C05 evidence.

The two-row fixture exposed corruption from the selected external FFV1 encoder's version-3 slice layout. One version-3 slice also fails at larger resolutions. Native conversion, sequential rendering and scene compilation now share an explicit choice: FFV1 version 1 with one slice when either output dimension is below four pixels, otherwise version 3 with four slices, or 16 slices for frames of at least 640 x 360 pixels and 64 per axis. Slices are FFV1's unit of parallel encoding and decoding; the decoded values are the same. Codec versions describe the encoded video; they do not change the Matroska display metadata or recipe schema. External native RGB8/RGB16 comparisons, tiny/tall/wide HDR conversions, a 1080p HDR frame and the scene/timeline regressions exercise this choice without discarding supported dimensions.

## Public references

- [ITU-R BT.2100-3, February 2025](https://www.itu.int/rec/R-REC-BT.2100-3-202502-I/en): primary chromaticities, PQ and HLG transfer/display equations. This bounded integer profile is not a claim of full television-system conformance.
- [RFC 9559, Matroska](https://www.rfc-editor.org/rfc/rfc9559.html): Colour and MasteringMetadata fields, EBML lengths, Void padding and seek offsets.
- [Reinhard, Stark, Shirley and Ferwerda, 2002](https://www-old.cs.utah.edu/docs/techreports/2002/pdf/UUCS-02-001.pdf): the public extended global tone curve. This engine supplies explicit white/peak parameters and applies the curve to maximum RGB; it does not implement that paper's automatic exposure or local operator.
- [FFmpeg zscale interface](https://www.ffmpeg.org/ffmpeg-filters.html#zscale): the independently invoked transfer reference. The selected external executable and license are recorded in [DEPENDENCIES.md](DEPENDENCIES.md).
- [SDR public references](COLOR.md): sRGB and BT.709 transfer facts used by the existing normalization path.

Only public format/equation facts inform this original implementation. No external implementation, specification copy, media fixture or binary is included in the repository.
