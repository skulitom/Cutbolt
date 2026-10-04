# Native-rate lossless movies and numbered images

The extended `export.inspect` and `export.run` profiles pass full verification, including the broader format checkpoint. The unchanged H.264 delivery profile retains 25 fps. Lossless `reference`, `png_mov` and `png_sequence` exports use the [eight native sequential clocks](NATIVE_TIMING.md); placed tracks retain 25 fps. Cuts/ranges must align with both video frames and whole 48 kHz audio samples. No rate conversion, resizing or frame interpolation occurs during export.

| Profile | Output | Streams | Encoded image values |
| --- | --- | --- | --- |
| `reference` | `.mkv` or audio-only `.wav` | Video, audio or both | Existing FFV1/bgr0 and PCM16 |
| `png_mov` | `.mov` | Video or video and audio | PNG/rgb24 and optional PCM16 |
| `png_sequence` | New `.frames` directory | Video or video and audio | Numbered RGB PNGs and optional `audio.wav` |

The new PNG profiles need `input_transfer: "srgb" | "bt709"`, or the project's declared `transfer`. This declares the interpretation of the already consistent timeline RGB values. The operation preserves those values exactly. MOV output carries full-range RGB, standard primaries and the chosen transfer tags, and validates them after encoding. The image directory manifest records the chosen interpretation; individual PNG files must be used with that manifest. These are opaque RGB exports; existing scene compositing has already flattened transparency into its selected background. Transparent output, arbitrary color-space conversion and native RGBA timelines remain separate requirements.

The PNG profiles accept positive dimensions up to 4,096 per side and 8,847,360 pixels, including odd dimensions, portrait frames and DCI 4K. The selected half-open range must contain 1–180,000 frames. H.264's existing size/rate/profile limits still apply. A size bound and a short 4K fixture do not establish long-form 4K performance.

## Numbered directory contract

Use the existing export fields, `profile: "png_sequence"` and an unused output path ending in `.frames`. Optional `sequence_first` selects the first number; omission means zero. Every number must fit six decimal digits. Filenames are fixed as `frame-000000.png`, `frame-000001.png`, and so on. Each native video frame occupies one entry, even when two decoded images are identical. The final frame belongs to the requested half-open interval; no endpoint image is added.

`manifest.json` contains the exact rational rate/duration, dimensions, selected source range, source identities, encoded RGB interpretation, complete ordered frame list and optional soundtrack identity. Each entry contains its number, relative filename, byte count and SHA-256. A decoded RGB digest binds all frames in order. The optional WAV has exactly the selected number of stereo PCM16 sample frames at 48 kHz, with no priming or padding. The receipt identifies the manifest by size/hash without embedding the complete list in the command response.

Encoding occurs in a uniquely owned sibling scratch directory. Every PNG is independently decoded through the selected PNG library; the concatenated pixels must match the rendered reference. The WAV is decoded and compared sample-for-sample by digest/count. Missing, corrupt, reordered or additional files fail verification. The source identities are checked again before publication.

On Windows the complete directory is published with a same-volume move that enables neither replacement nor copy/delete fallback. An existing file or directory, including one appearing after inspection, is preserved and causes failure. Ordinary failures remove only the invocation's fixed scratch names. An actual process crash can leave private scratch output; it does not publish an incomplete destination. Other platforms explicitly reject directory export until an equivalent publication mechanism is implemented. Single-file exports retain the existing no-overwrite hard-link publication rule.

The implementation uses the public [FFmpeg image2/MOV interfaces](https://ffmpeg.org/ffmpeg-formats.html) and Windows [MoveFileExW contract](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw). Existing selected FFmpeg 7.0 and PNG 0.18.1 dependencies retain their licenses; no new dependency or external source is copied into the project.

## Acceptance

The original `tests/export_formats.py` fixture compares every selected RGB frame and PCM sample across eight rates and the three lossless output profiles. It checks repeated images, cuts/gaps, first/last frames, rational timestamps, stream selection, nonzero numbering, Unicode/percent paths, equivalent unreduced rates and six dimension cases. Concurrent real writers and a test-only adapter exercise complete-directory publication, missing/corrupt/swapped images, encoding failure, a late occupied destination and changed source content. The adapter runs the real encoder before perturbing its own synthetic fixture files. Original source files, occupied output and diagnostic evidence remain external.

Numbered-source ingestion/relinking, transparent export, long-form 4K performance and native third-party project compatibility are not established by these export cases. Their existing acceptance criteria remain required.
