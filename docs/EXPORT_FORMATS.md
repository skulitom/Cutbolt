# Native-rate lossless movies and numbered images

The extended `export.inspect` and `export.run` profiles pass full verification, including the broader format checkpoint. The unchanged H.264 delivery profile retains 25 fps. Lossless `reference`, `png_mov` and `png_sequence` exports use the [eight native sequential clocks](NATIVE_TIMING.md); placed tracks retain 25 fps. Cuts/ranges must align with both video frames and whole 48 kHz audio samples. No rate conversion, resizing or frame interpolation occurs during export.

| Profile | Output | Streams | Encoded image values |
| --- | --- | --- | --- |
| `reference` | `.mkv` or audio-only `.wav` | Video, audio or both | Existing FFV1/bgr0 and PCM16 |
| `png_mov` | `.mov` | Video or video and audio | PNG/rgb24 and optional PCM16 |
| `png_sequence` | New `.frames` directory | Video or video and audio | Numbered RGB PNGs and optional `audio.wav` |
| `gif` | `.gif` | Video only | Each frame's exact colors in its own palette, at most 256 per frame ([animated GIF](#animated-gif)) |

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

## Animated GIF

`profile: "gif"` writes a new `.gif` from `streams: "video"`, for loops such as pixel art, flat UI and text. Every decoded pixel equals the timeline's. A frame that GIF cannot hold exactly is refused, never quantized or dithered. GIF has no transfer tag, and browsers show its values as sRGB, as they do PNG's. The export writes the timeline's encoded values unchanged and takes no `input_transfer`. Dimensions follow the PNG bounds: each side at most 4,096 and at most 8,847,360 pixels. Six of the eight native rates are accepted, as the delays below explain; there is no audio.

```json
{
  "profile": "gif",
  "streams": "video",
  "output": "C:\\DEV\\CutboltData\\output\\loop.gif",
  "gif": {"plays": 3, "timing": "exact"}
}
```

Add these fields to the ordinary export request. `gif` is optional; omitted, the animation loops endlessly with exact delays.

- **Colors.** Each frame has its own local color table holding exactly its distinct colors, in ascending order. A frame with more than 256 colors fails the export with `TOO_MANY_COLORS`. The message names the first such frame, in the range and on the timeline, with its color count, and says how many frames exceed 256. No file is published. The limit is per frame, not per animation: the fixture's pixel-art loop has 108-122 colors per frame and 5,742 over 58 frames. Gradients, blurs, photographs and anti-aliased edges usually exceed it.
- **Frames.** The first frame is stored whole. Each later frame stores only the smallest rectangle that differs from the picture before it and is drawn over that picture; an unchanged frame stores one pixel. Every timeline frame stays one GIF frame, so decoders return the exact frame count.
- **Delays.** GIF stores each frame's delay in whole centiseconds (1/100 s). 25 fps (4 cs) and 50 fps (2 cs) are exact, and the default `timing: "exact"` refuses other rates, naming the delays they would need. `timing: "nearest_centisecond"` accepts 24, 30, 24000/1001 and 30000/1001 fps: each frame starts at its exact time rounded to the nearest centisecond, within half a centisecond, so delays alternate (3 and 4 cs at 30 fps; 4 and 5 cs at 24 fps). 60 and 60000/1001 fps are refused either way: they need delays under 2 cs, and browsers play those as 10 cs. Nothing is dropped or resampled; make the animation at 25 or 50 fps for exact timing.
- **Repetition.** `plays` counts how many times the animation plays. Omitted or null loops endlessly (NETSCAPE2.0 repeat count 0). 1 writes no loop extension and plays once. 2 to 65,536 write a repeat count of `plays - 1`, which browsers commonly read as repetitions after the first play. Viewers differ, so check the target viewer when an exact count matters.

The receipt's `video` reports `plays`, `netscape_repeat_count`, `timing`, the shortest and longest `delay_centiseconds`, `duration_centiseconds` and `maximum_start_error` (rational seconds, zero for exact timing). After the export it also reports the fewest and most `colors_per_frame` and the `distinct_colors` over the animation. Inspection reports the settings without rendering, so it cannot know the colors yet.

The engine renders the range losslessly and encodes it frame by frame. It then decodes every frame with FFmpeg's GIF decoder, which is independent of the encoder, and requires the decoded RGB to equal the lossless render's. It checks every frame's timestamp and the animation's length in centiseconds, then rechecks the sources before no-overwrite publication. The encoder is original Rust written from the public [GIF89a specification](https://www.w3.org/Graphics/GIF/spec-gif89a.txt): LZW image data with variable-length codes, a graphic control extension per frame and the NETSCAPE2.0 application extension. No dependency was added.

GIF compresses the palette indices losslessly. Files are far larger than H.264 and grow with the picture size and with how much changes between frames. Fifty 1080p frames of 4 x 4 pixel art that changes everywhere made 17.3 MB, against 9.4 MB of lossless FFV1. Exporting those frames took 7.1 s with a release build, of which the lossless render alone takes 6.0 s, and 16.4 s with a debug build.

`tests/gif.py` exports original pixel-art, noise and one-level-neighbor colors at 25, 50, 30 and 24000/1001 fps. It covers cuts, gaps, ranges, unchanged frames and frames of exactly 256 colors that fill LZW's code table. Every frame is decoded by FFmpeg and by an independent Python GIF reader and compared with the lossless reference render. The fixture also checks the color tables, delays, repeat counts, decoder timestamps and receipts, refuses frames of 257 and 300 colors, and rejects invalid requests. It passes on FFmpeg 7.0.2 and 6.1.1. It is a focused fixture rather than part of a thorough run, and earns no points.

## Choosing a web or social delivery

| Material | Profile | Why |
| --- | --- | --- |
| Silent loops of pixel art, flat UI or text with at most 256 colors per frame | `gif` | Exact pixels; loops in browsers and chats; exact timing at 25 or 50 fps |
| Anything with sound, photographic or long material | `h264_aac` | Plays almost everywhere and is small; 4:2:0 color and lossy compression change pixels |
| Masters for editing or for a platform that takes lossless uploads | `png_mov`, `png_sequence` or `reference` | Lossless |

- **Transfer.** For H.264, use `input_transfer: "bt709"` for screen-designed material: PNG artwork, pixel art, text and UI. `srgb` darkens shadows in ordinary players, so a video looks darker than the same pixels as GIF or PNG ([explicit color interpretation](EXPORT.md#explicit-color-interpretation)).
- **Pixel art in H.264.** 4:2:0 color bleeds one-pixel color detail into its neighbors. Upscaling the art by a whole factor in the timeline reduces the damage but does not remove it ([pixel art and 4:2:0 color](EXPORT.md#pixel-art-and-420-color)). Use GIF when the pixels must be exact and the loop fits its limits.
- **Re-encoding.** Social platforms typically re-encode uploads, usually to 4:2:0 H.264 or similar. Upload the cleanest file you can: their encode adds its own loss on top of yours.
- **GIF size.** Services often limit GIF file size. Export at the smallest whole-factor size the destination displays well, and keep backgrounds still where the design allows. For a long or busy loop, export a short range first and check its receipt's colors and its file size.
