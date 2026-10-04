# Explicit SDR normalization

The optional `source.sdr` declaration in `media.conform.inspect` / `media.conform` converts supported native samples into a declared full-range RGB working transfer. Use it before combining media with different interpretations. Inspection reports assumptions and exact time mapping; rendering creates a new tagged FFV1/PCM editing asset. Sources stay immutable and compiled assets use the existing saved-session, preview and export commands. This adds no command, service or runtime dependency.

For native 10/12/16-bit samples, wide-gamut conversion or PQ/HLG, use the separate [high-bit-depth/HDR contract](HDR.md). Its RGB8 SDR outputs feed this existing timeline; its RGB16 intermediates remain in the explicit HDR conversion path.

## Recipe

Use `source.sdr` instead of the legacy `source.color` field and supply `working_transfer`:

```json
{
  "schema_version": 1,
  "id": "normalized-shot",
  "source": {
    "file": {"path":"shot.mkv", "sha256":"SOURCE_SHA256", "bytes":123456},
    "sdr": {"matrix":"bt709", "range":"limited", "transfer":"bt709", "missing_tags":"use_declared"}
  },
  "working_transfer": "srgb",
  "source_in": {"num":0,"den":1},
  "duration": {"num":1,"den":1},
  "rate": {"num":1,"den":1},
  "reverse": false,
  "freeze": false,
  "width": 640,
  "height": 360,
  "audio": "resample"
}
```

Replace the source identity and dimensions with actual values. `sdr` and `working_transfer` must appear together. Legacy `color` must be omitted or null; supplying both interpretations fails. Omitting both new fields preserves the legacy conversion behavior, including its strict tagged H.264 route. Audio-only input has no color interpretation and cannot use normalization.

An optional [LUT transform](LUTS_SCOPES.md) can follow this normalization before resizing. It reads the encoded working-transfer RGB8 values and retains the same color interpretation. Numerical scopes inspect exact full-quality timeline frames with an explicit transfer and missing-tag policy.

| Input | Matrix | Range | Transfer |
| --- | --- | --- | --- |
| FFV1/bgr0 MKV | `rgb` | `full` or `limited` | `srgb` or `bt709` |
| FFV1/yuv444p MKV | `bt709` | `full` or `limited` | `srgb` or `bt709` |
| H.264/yuv420p or yuvj420p MP4/MOV | `bt709` | `full` or `limited`; yuvj420p requires full | `srgb` or `bt709` |

All normalized inputs use BT.709 primaries and D65. The working transfer is `srgb` or `bt709`; output matrix is RGB, range full, primaries BT.709, with matching transfer tags. This is an 8-bit path. Existing conform restrictions on progressive square pixels, timing, audio, source identity, local roots, duration and no-overwrite publication still apply. Native decoded samples are capped at 4 GiB (four bytes per bgr0 pixel, three per YUV444 pixel, or 1.5 per YUV420 pixel); output RGB retains its separate 4 GiB bound.

## Metadata policy

`missing_tags` is required. `reject` requires matching matrix, range, primaries and transfer tags. `use_declared` permits absent/unknown tags to use the supplied interpretation; every assumed stream tag is listed in `normalization.assumed_tags`. Known conflicting tags always fail, including under `use_declared`. The same check applies to decoded frame color tags when present, so a contradictory midstream declaration cannot silently replace the chosen interpretation.

YUV420 additionally requires even dimensions and left chroma location, either tagged or explicitly assumed through `use_declared`. Its reconstruction copies each native chroma sample across its corresponding 2x2 luma block. Other chroma locations fail. This declared nearest-block reconstruction favors deterministic sample behavior; it is not a high-quality interpolation claim. YUV444 has one chroma pair per pixel.

HDR/unsupported primaries, ICC/camera/log conversion, gamut mapping, display calibration and automatic scene detection are outside this profile. There is no override that relabels contradictory metadata as correct. Unspecified primaries do not trigger a guess: choosing `use_declared` explicitly assigns the profile's BT.709 primaries.

## Numerical contract

The engine decodes native bgr0 or planar YUV bytes without asking an external scaler to interpret their color. It converts those values in original Rust code before the existing resize/time selection output is encoded.

- Full RGB divides each channel by 255. Limited RGB subtracts 16 and divides by 219.
- Limited YUV uses `(Y-16)/219`, `(Cb-128)/224` and `(Cr-128)/224`. Full YUV uses `Y/255`, `(Cb-128)/255` and `(Cr-128)/255`; this full-range integer convention is explicitly selected by this profile.
- BT.709 luma coefficients are 0.2126/0.7152/0.0722. The color differences reconstruct red with `Y + 1.5748*Cr` and blue with `Y + 1.8556*Cb`; green follows from the luma equation.
- Reconstructed encoded RGB is clipped to [0,1] before transfer conversion. No intermediate 8-bit RGB rounding occurs in the YUV path.
- sRGB decoding uses the 0.04045 piecewise threshold, 12.92 linear divisor and 2.4 exponent; encoding uses the corresponding 0.0031308 threshold. BT.709 interpretation uses its OETF and inverse, with inverse threshold 0.081 and forward threshold 0.018. Matching input/output transfers bypass the round trip.
- Calculations use f64, followed by clipping and nearest 8-bit quantization. Full-range RGB with an unchanged transfer preserves all sample values exactly.

The public transfer/primary definitions are documented in [W3C CSS Color 4 section 10.2, draft of 30 September 2026](https://www.w3.org/TR/2026/CRD-css-color-4-20260930/#predefined-sRGB) and [ITU-R BT.709-6 sections 1 and 3](https://www.itu.int/rec/R-REC-BT.709-6-201506-I/en). BT.709 here means source signal OETF interpretation; this is not a BT.1886 reference-display transform. Clipping, missing-tag policy, full-range integer convention, quantization and supported format bounds are explicit project choices. No specification copy or third-party implementation is included.

## Workflow and verification

Normalize every source intended for one sequence to the same working transfer, retain each recipe/receipt, and add the returned identity-bound `asset` through `session.apply`. The existing timeline does not enforce a global working space or convert mismatched assets automatically. Declare that working transfer once with the `project.transfer` operation, or pass `input_transfer: "srgb"` or `"bt709"`, to match those normalized values in [delivery exports](EXPORT.md) and scopes. Scene compilation retains its separate sRGB contract. This workflow normalizes input signals; it does not add live color effects or a color-managed display.

Normalization retains exact conform frame mapping, resize order and audio policy. Receipts include `working_transfer`, `normalization.input`, any assumed tags and conversion/clipping/precision policy. Before publication, all decoded RGB/PCM must match the generated buffers, output color tags must match the declared working space, and the source identity must remain unchanged. Existing outputs are never replaced. Inspection is available through the existing MCP tool; execution remains blocking CLI/library work.

Create a retained original fixture outside the repository:

```powershell
cargo build --locked
python -X utf8 tests/color.py --output C:\DEV\CutboltData\new-color-test
```

Then run this documented example against that fixture:

```python
from pathlib import Path
import json, subprocess
root = Path(r"C:\DEV\CutboltData\new-color-test")
request = {"command":"media.conform.inspect",
    "recipe":json.loads((root / "recipe.json").read_text(encoding="utf-8")),
    "input_root":str(root / "sources")}
exe = r"C:\DEV\Cutbolt\target\debug\cutbolt.exe"
def call(value):
    result = subprocess.run([exe], input=json.dumps(value), text=True,
        encoding="utf-8", capture_output=True, check=True)
    return json.loads(result.stdout)["result"]
print(call(request)["normalization"])
request.update(command="media.conform", output_root=str(root / "output"),
    output=str(root / "output" / "documented-normalization.mkv"))
print(call(request)["asset"])
```

The independent acceptance reference uses original native RGB/YUV charts, 48-digit Decimal transfer/matrix equations and rational frame selection. Every decoded output channel must be within one 8-bit level; unchanged full RGB and every PCM sample must match exactly. Tests cover full/limited ranges, both transfer directions, out-of-nominal clipping, tagged/untagged sources, native YUV420 reconstruction, output tags, reverse/freeze/rate changes, deterministic repeats, MCP schemas and mixed normalized sources in saved sessions and delivery. Invalid metadata, undeclared interpretations and existing outputs must fail without publication or source changes. These tests target C01 basic and extended; LUT/scopes and high-bit-depth/HDR retain separate criteria.

The retained `C:\DEV\CutboltData\color-20261003-02` run compared **252 decoded video frames and 483,840 stereo sample frames**, covering 30 normalization renders plus a mixed saved-session reference sequence. Every color sample matched the independent reference exactly within the permitted one-level tolerance; PCM also matched exactly. The mixed sequence additionally passed delivery validation. Twenty-five rejected/failure cases passed with original sources and existing outputs preserved.

The documented workflow also passed. The full verifier passed **207 checks** for this milestone; current authoritative evidence and totals are in [verification/latest.json](../verification/latest.json) and [PROGRESS.md](PROGRESS.md).
