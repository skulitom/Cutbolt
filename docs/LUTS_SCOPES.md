# LUT conversion and numerical scopes

`lut.inspect` reads an external, identity-bound `.cube` table and evaluates optional sample colors. `media.conform` can apply that table after [explicit SDR normalization](COLOR.md), before resizing. The resulting tagged FFV1/PCM asset uses ordinary saved-session editing, previews and exports. `scopes.inspect` returns numerical histogram, waveform/parade and vectorscope populations for one exact full-quality timeline frame.

Both inspection commands are available through CLI/library calls and MCP stdio. Conversion remains a blocking CLI/library operation. These functions use original Rust arithmetic; there is no new runtime dependency, plugin loader, network request or bundled LUT.

## Table contract

`transform` contains `file: {path, sha256, bytes}` and `interpolation`. The relative file path resolves under the explicit absolute `input_root`; links escaping the root, changed identities and oversized files fail. Tables are at most 8 MiB, use the `.cube` extension and contain printable ASCII, tabs and line endings. Each line is at most 4096 bytes. A `#` begins a comment outside a quoted title.

Supported headers precede all data and occur at most once:

| Header | Contract |
| --- | --- |
| `TITLE "text"` | Optional, at most 256 ASCII bytes; no embedded quotes |
| `LUT_1D_SIZE N` | Exactly N RGB triples; N from 2 through 65536 |
| `LUT_3D_SIZE N` | Exactly N cubed RGB triples; N from 2 through 65 |
| `DOMAIN_MIN R G B` | Optional per-channel lower bounds, default 0 |
| `DOMAIN_MAX R G B` | Optional per-channel upper bounds, default 1 |

Declare exactly one size. Numbers must be finite and within -16 through 16; each domain span must be at least 0.000001. Missing/extra data, duplicate/unknown headers and combined shaper/3D tables fail explicitly. The 3D table varies red fastest, then green, then blue.

One-dimensional tables support `nearest` or `linear`; three-dimensional tables support `nearest`, `trilinear` or `tetrahedral`. Each input channel maps its declared domain to the table grid and clamps at the endpoints. Nearest sampling rounds a grid midpoint upward. Trilinear interpolation weights the eight surrounding corners. Tetrahedral interpolation selects the four-vertex simplex from the fractional-coordinate ordering; equal coordinates use stable R/G/B ordering. Arithmetic uses f64. Inspection returns unbounded table results plus their clipped/rounded RGB8 equivalents; rendering clips results to 0..1 and rounds once to the nearest 8-bit value.

LUTs operate on the declared **encoded working transfer**, after normalization has produced 8-bit full-range RGB. They retain that transfer and the BT.709/D65 primaries interpretation; there is no inferred gamut or transfer conversion from a table's title. A LUT cannot be combined with the older implicit conversion path: `source.sdr` and `working_transfer` are required. A recipe has one LUT. Compose additional conversion stages explicitly if needed; this introduces another 8-bit quantization step.

The existing conversion duration, pixel, source-size and frame-count bounds still apply. LUTs do not alter audio, time selection or source media. Source and table identities are rechecked before no-overwrite publication. Inspection reports the exact table identity, domain, dimensions, channel minima/maxima, interpolation and clipping policy.

Public interface references: [FFmpeg LUT filter descriptions](https://ffmpeg.org/ffmpeg-filters.html#lut3d) and [supported color table formats](https://opencolorio.readthedocs.io/en/stable/guides/using_ocio/using_ocio.html). These are interface facts, not imported implementations or additional dependencies.

## Numerical frame scopes

`scopes.inspect` takes `project`, `input_root`, rational `time`, `input_transfer: "srgb" | "bt709"`, `missing_tags: "reject" | "use_declared"`, and `columns`. The timeline must satisfy the existing 25 fps reference profile: 1–64 sequential items or a [native-track](TRACKS.md) arrangement, including alpha overlay tracks and gaps. Time is a frame boundary before the timeline end; each frame is at most 8 million pixels. `columns` is 1 through `min(width, 256)`.

The source must have full-range RGB/BT.709 primaries with the declared transfer. Stream metadata conflicts fail; missing tags require explicit interpretation. The scopes measure encoded RGB values, not luminance in nits or a display transform. A gap supplies exact black; its source interpretation report is null. Scopes always use the original asset even if a reduced or offline proxy is selected. They do not change the project, write an image, or save a cache.

Every pixel contributes once, without subsampling. For each of red, green, blue and luma, `values` contains:

- `histogram[code]`: population of each integer code 0..255.
- `waveform[column][code]`: population in horizontal bin `floor(x * columns / width)`. The three RGB waveforms form a numerical parade.
- `minimum`, `maximum` and exact integer `sum`.

Let `Y = (2126*R + 7152*G + 722*B)/10000` on stored 8-bit channels. Luma is `round(Y)`. The vectorscope uses `Cb = round(128 + (B-Y)/1.8556)` and `Cr = round(128 + (R-Y)/1.5748)`, clamped to 0..255; `vectorscope[Cr][Cb]` counts pixels. All rounding is nearest, ties upward. The color-difference equations use the unrounded Y. Coefficients follow [ITU-R BT.709-6, June 2015](https://www.itu.int/rec/R-REC-BT.709-6-201506-I/en); binning, center and quantization are this project's explicit scope contract.

The result also includes original-quality frame/source identity, source frame index, timeline revision, RGB SHA-256, dimensions and pixel count. There is no GUI scope, temporal accumulation, HDR scale, automatic exposure adjustment or live LUT scene effect in this profile.

## Runnable acceptance fixture

Use a fresh directory outside the checkout:

```powershell
cargo build --locked
python tests/luts_scopes.py --output C:\DEV\CutboltData\my-lut-test
$demo = 'C:\DEV\CutboltData\my-lut-test'
$recipe = Get-Content -Raw "$demo\recipe.json" | ConvertFrom-Json
@{command='lut.inspect'; transform=$recipe.lut; input_root="$demo\sources";
    samples=@(@(0.25,0.5,0.75),@(1.0,0.0,0.0))} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
@{command='media.conform'; recipe=$recipe; input_root="$demo\sources";
    output_root="$demo\output"; output="$demo\output\extra.mkv"} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
$project = Get-Content -Raw "$demo\project.json" | ConvertFrom-Json
@{command='scopes.inspect'; project=$project; input_root=$demo;
    time=@{num=0;den=1}; input_transfer='srgb'; missing_tags='reject'; columns=7} |
    ConvertTo-Json -Depth 30 -Compress | .\target\debug\cutbolt.exe
```

The original fixture compares every decoded RGB channel and PCM sample. Exact Fraction references cover one-dimensional knots, all six tetrahedral orderings, multilinear cross terms, domain clamping and clipped output; the existing external `lut1d`/`lut3d` filters provide a second comparison. LUT outputs allow at most one 8-bit level of arithmetic/quantization difference. Identity interpolation and PCM must match exactly. Every scope bin and aggregate must match exactly, including uneven horizontal bins, cuts, gaps, previews and saved sessions. Maximum table sizes, malformed tables, mismatched identities, a LUT modified during conversion, metadata conflicts and preservation of sources/existing outputs are exercised. The full verifier now records C04 basic and extended evidence in the [core checklist](PROGRESS.md). The retained fixture compared 83 decoded frames and 159,360 stereo sample frames, with maximum LUT error of one level, exact scope values and 55 rejected/failure cases. No acceptance threshold was weakened.
