# Exact native sequential frame clocks

Full verification passes both exact-timing checkpoints. Native sequential `render.plan`, `render.run`, queued renders, frame/range previews, cached previews, scopes and contact sheets accept these project frame rates:

| Frame rate | Stereo sample frames per video frame at 48 kHz | Required shared cut boundaries |
| --- | ---: | --- |
| 24 | 2,000 | Every frame |
| 25 | 1,920 | Every frame |
| 30 | 1,600 | Every frame |
| 50 | 960 | Every frame |
| 60 | 800 | Every frame |
| 24000/1001 | 2,002 | Every frame |
| 30000/1001 | 8,008 per five frames | Multiples of five frames |
| 60000/1001 | 4,004 per five frames | Multiples of five frames |

Set the snapshot's existing rational `frame_rate` field, for example `{"num":30000,"den":1001}`. Timeline positions and source windows remain exact rational seconds. Source dimensions/rate must match the timeline, with FFV1/bgr0 video and 48 kHz stereo PCM. Cuts and gaps require whole audio samples as well as whole video frames. Unsupported rates and cuts between samples reject explicitly; there is no implicit audio rounding or resampling at native cuts.

For 30000/1001, five frames last exactly 1001/6000 seconds and contain exactly 8,008 stereo sample frames. The same exact arithmetic is used after splits, trims, moves, selected ranges and undo. A single video-frame preview can inspect any native frame; a rendered range containing audio must meet the shared cut rule above.

The sequential bound is 1–64 clips and 1–180,000 total output frames. Every inspected source has the same 180,000-frame bound. Frame/contact-sheet previews retain their existing pixel limits. Proxies, placed tracks, nested sequences, scene compilation and H.264 delivery retain their documented 25 fps restrictions. [Lossless sequential exports](EXPORT_FORMATS.md) now use these native clocks. This extension does not silently change their clocks or claim fractional-rate support for those paths.

## Container timestamps

The project clock remains an exact rational value. Matroska's ordinary millisecond block timestamps cannot represent every fractional-rate instant exactly. Verification checks each decoded timestamp against the expected frame clock rounded to the declared container tick. Time bases must be between one nanosecond and one millisecond; the textual probe representation has an additional one-microsecond precision allowance. The original 25 fps profile retains its exact 40-millisecond timestamp rule.

Matroska's nominal `DefaultDuration` is an integer count of nanoseconds, as specified in its [element definition](https://www.matroska.org/technical/elements.html#DefaultDuration). At 60000/1001, the selected probe reports a nearby nominal fraction after that quantization. A Matroska rate hint within one nanosecond per frame may agree with the declared project rate, but every decoded timestamp must still pass the independent exact-clock check. The hint never replaces or changes the project rate. Other mismatched sources reject.

Native rendering resets video frame indices onto the chosen rational clock. The video encoder time-base option is explicitly restricted to video; audio keeps its independent sample clock. Receipt `frame_rate` reports the project rate. Non-25-fps sequential plans use profile `reference-ffv1-pcm-rational-v2`; the original 25 fps profile identifier remains unchanged.

Queued publication recovery derives both expected frame count and sample count from the saved timeline. It does not assume 1,920 samples per frame. A validated published fractional-rate output can therefore be recovered after process interruption without re-encoding or changing its audio duration. Existing queue retry and source/publication checks still apply.

## Variable-rate sources

Direct native rendering rejects a source whose timestamps do not match the declared constant frame clock. Use the existing explicit `media.conform` path for variable-rate sources: it samples the observed source presentation timestamps into the documented 25 fps intermediate and preserves the declared audio policy. That intermediate can then be edited and rendered normally. This is explicit VFR input conversion; native projects in this slice remain constant-rate timelines.

## Independent acceptance

The original `tests/native_timing.py` fixture creates eight native rates with frame identities and independent sample-index patterns. It checks every decoded RGB pixel, PCM sample and video timestamp through cuts/gaps, single-frame and range previews, fractional saved edits, replay, undo and queued output. Fractional cases also verify cold/warm cached frames and ranges, selected contact-sheet tiles and independent RGB histograms. A separate actual process-crash check verifies fractional publication recovery with five frames and 8,008 samples.

The long fixture renders 54,000 frames at 30000/1001, exactly 1,801.8 seconds, and verifies every output frame and all 86,486,400 stereo sample frames. Its intentionally small 32 × 18 image isolates clock behavior and bounded streaming; it earns no long-form 4K or production-performance claim. P03 extended remains separate. A VFR fixture verifies declared source-clock selection through conversion and subsequent rendered edits while direct unconverted input rejects.

The first development run caught a video time-base option applying to audio; it was restricted to the video stream without changing the audio gate. Another run exposed Matroska's nominal-rate quantization at 60000/1001; the exact timestamp and sample checks remain mandatory. Failed development evidence is retained outside the repository. No new dependency, generated media, third-party implementation or copied specification is included.
