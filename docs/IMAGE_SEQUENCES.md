# Numbered footage and transparent lossless compilation

This profile validates a complete numbered PNG source and compiles a selected window into a new local movie. Full alpha/timing verification passes, including E01 extended. `image.sequence.inspect` is available through CLI/library and MCP. `image.sequence.compile` is a blocking CLI/library command.

The original version-1 recipe declares `id`, `first_number`, the complete ordered `frames` list, rational `frame_rate`, `source_start`, `source_count`, positive `repeat`, `input_transfer`, `alpha_mode` and `profile`. Every frame has `number` and an `image` identity containing a relative path, byte size and lowercase SHA-256. All source files are validated, including unused frames outside the selected window. Missing numbers, changed/corrupt files, inconsistent dimensions and unsupported PNG metadata reject before staging begins.

The source window is half-open and measured in image indices. Repetition starts each cycle at the same selected image and emits exactly `source_count * repeat` frames. Repeated identical images still occupy separate intervals. The declared rate is one of the [eight supported native clocks](NATIVE_TIMING.md); there is no automatic rate conversion, interpolation or added final frame. Duration follows exact frame count/rate. The output includes explicitly silent 48 kHz stereo PCM16, with a whole-sample duration. This slice does not import or infer a soundtrack.

| Profile | Output | Alpha and timeline behavior |
| --- | --- | --- |
| `rgba_ffv1` | FFV1/bgra and PCM in `.mkv` | Straight RGBA output; transparency preserved |
| `rgba_png_mov` | PNG/rgba and PCM in `.mov` | Straight RGBA output; transparency preserved |
| `reference_rgb` | FFV1/bgr0 and PCM in `.mkv` | Requires explicit RGB `background`; returns an identity-bound native asset |

Opaque results can be added to ordinary saved projects, edited, previewed and rendered with the matching native frame clock. Transparent movies are separate lossless intermediates; native timeline compositing remains opaque. These commands do not imply native RGBA timeline support. Keep the original recipe and ordered identities for editable regeneration and checked relocation. Moving identical source files and changing the explicit input root preserves the declared source window; different replacements need new reviewed identities.

## Alpha and color

`alpha_mode` explicitly declares straight or premultiplied source bytes. Straight transparent output preserves every RGBA value, including hidden RGB at zero alpha. Premultiplied sources must have RGB no greater than alpha and zero RGB at zero alpha. Transparent compilation explicitly unassociates RGB with nearest-integer rounding; zero alpha produces zero RGB. That conversion can change stored RGB and is reported in the receipt. The lossless codec preserves the resulting declared straight pixels exactly.

`reference_rgb` composites over the supplied opaque background using the existing encoded-value normal-over rule, retaining premultiplied precision until the final rounding. It does not perform a linear-light or color-space conversion. Transparent profiles reject a background, so flattening is never implicit.

`input_transfer` declares `srgb` or `bt709` with the existing standard RGB primaries. An sRGB-tagged PNG conflicts with a BT.709 declaration and rejects. The selected output carries full-range RGB, standard primaries and the declared transfer tags; all are verified after encoding. Unsupported ICC, HDR, orientation and alternate gamma/chromaticity metadata reject under the existing bounded PNG contract.

## Bounds and publication

Recipes accept 1–1,024 complete source entries, each plain/sRGB 8-bit RGB/RGBA PNG at most 512 pixels per side. The complete encoded and decoded source lists must each fit 64 MiB. Output has at most 180,000 frames; raw video and silent PCM staging together must fit 2 GiB. These are explicit limits, not evidence of general image-format support or long-form 4K performance.

Staging stays in a uniquely owned directory beneath the explicit output root. The engine verifies codec/container, color/alpha format, every decoded frame timestamp, exact video/audio counts and complete decoded pixel/PCM digests. Movie track durations must also match the exact rational clock. Every original source identity is checked again before publishing with the existing no-overwrite mechanism. Ordinary failure cleans only this invocation's known scratch files; existing sources and outputs are preserved.

The independent fixture covers straight and premultiplied alpha, zero and near-zero alpha, opaque edges, declared color, multiple backgrounds, repeated source windows, four native rates, both transparent codecs and saved native editing after explicit flattening. Its 30-minute case uses intentionally small images and checks all 54,000 transparent frames and 86,486,400 stereo sample frames. This establishes long lossless alpha/timing behavior, not the separate P03 long-form 4K requirement. Source arrays, generated movies and failed diagnostics remain external. No new dependency is selected.
