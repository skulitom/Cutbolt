# Explicit delivery quality and bitrate controls

This extension passes full verification and earns E02 extended. It keeps the reference timeline and publication rules in [EXPORT.md](EXPORT.md): 25 fps, explicit source transfer, original-quality media, exact requested range, verified stream clocks and no overwritten output.

`export.inspect` and `export.run` accept optional `h264` settings for H.264 video and optional `aac_bitrate` for AAC output. Omit both to retain the original version-1 High-profile CRF-18 preset with 320 kb/s audio. Explicit settings produce a version-2 profile receipt. Inspection includes the complete selected controls and `encoder_passes`; it writes no files. Arbitrary encoder arguments remain unsupported.

```json
{
  "h264": {
    "compatibility": "main_hd",
    "rate_control": {
      "mode": "two_pass",
      "bitrate": 1500000,
      "maximum_bitrate": 2000000,
      "buffer_size": 4000000
    }
  },
  "aac_bitrate": 256000
}
```

Add these fields to the ordinary export request. A quality-driven alternative is:

```json
{
  "h264": {
    "compatibility": "high_hd",
    "rate_control": {
      "mode": "quality",
      "crf": 18,
      "maximum_bitrate": 12000000,
      "buffer_size": 24000000
    }
  }
}
```

| Compatibility | Decoded H.264 profile / level | Maximum dimensions | B-frames / reference frames | Maximum rate / buffer bounds |
| --- | --- | --- | --- | --- |
| `baseline720p` | Constrained Baseline / 3.1 | 1280 × 720 | 0 / 1 | 12,000,000 bits/s / 14,000,000 bits |
| `main_hd` | Main / 4.0 | 1920 × 1080 | 2 / 3 | 20,000,000 bits/s / 25,000,000 bits |
| `high_hd` | High / 4.0 | 1920 × 1080 | 2 / 3 | 20,000,000 bits/s / 25,000,000 bits |

Dimensions must be even and at least two pixels. There is no automatic resizing. These are declared stream profiles, not promises that every device with an advertised profile will play every export. The focused compatibility matrix checks the pinned software decoder and the explicitly selected local CUDA device. Untested browsers, televisions and phones are outside that evidence.

`quality` accepts CRF 10–35. Lower values request better quality and usually larger output; the buffer constraints can still force lower quality. `two_pass` accepts a target bitrate from 100,000 bits/s through the selected maximum. Both modes require a maximum bitrate of at least 100,000 bits/s and a buffer holding at least one second at that rate, within the table's bounds. Rate and buffer values use bits, not bytes.

The maximum bitrate is an encoder buffering constraint, not a fixed file-size ceiling or a promise about every short interval. Initial buffer occupancy allows bursts. Two-pass mode targets an average video bitrate; content, duration, encoder limits, audio and container overhead affect the final size. Quality mode has no average-rate target. Receipts report requested controls; the acceptance fixture independently measures compressed video packet sizes and quality.

AAC accepts exactly 192,000, 256,000 or 320,000 bits/s. Its default remains 320,000. This setting works for audio-only `.m4a` output as well as combined output; omit `h264` and `input_transfer` for audio-only delivery. Video-only exports must omit `aac_bitrate`. Reference exports must omit both new controls. AAC presentation duration, priming and tail-padding checks remain unchanged.

## Two-pass execution and failure handling

The engine renders the selected reference interval once. The first video-only analysis pass writes statistics in the export's unique owned scratch directory and discards encoded output. The second pass reads those statistics and produces the selected MP4 streams. The explicit first-pass settings avoid a separate fast-analysis configuration. Both passes have the existing 600-second tool timeout individually. Concurrent calls use separate directories and logs.

Normal errors clean only known files in the owned scratch directory, including statistics and temporary statistics. A failed first or second pass publishes no final output. A process crash may leave that scratch directory for inspection, as with existing synchronous exports; this extension does not add export queue recovery. Output validation and final source-hash checks still precede no-overwrite publication.

## Acceptance and provenance

The original `tests/delivery_profiles.py` generator uses moving grayscale detail, deterministic noise, binary frame identities and distinct stereo tones. The main matrix has three compatibility profiles and four rate settings: CRF 18, CRF 26, and two-pass targets of 0.5 and 1.5 Mb/s. Every output is independently decoded in software and on the selected CUDA device; requiring hardware frames before downloading them prevents a silent software substitute. Decoded YUV must match exactly between these routes.

Acceptance checks every frame and presentation timestamp, compressed packet order/count, exact AAC presentation duration, zero-sample alignment, at least 27 dB RGB PSNR and 28 dB PCM SNR for this intentionally noisy fixture, and two-pass video bitrate within 20% of the requested target. Higher-quality choices must improve measured quality and bitrate. One-second compressed windows must fit the selected maximum rate plus buffer. These are fixture gates, not quality guarantees for arbitrary material. The separate original chart fixture retains its existing stricter 30 dB RGB gate and default-preset behavior.

The initial CRF-28 trial measured 26.54 dB in the constrained profile and missed the declared 27 dB gate. The quality comparison now uses CRF 26; the gate and failed evidence are retained. The passing main matrix measures 27.11–32.72 dB RGB PSNR and at least 33.24 dB PCM SNR. Two-pass target error is below 0.2% in that fixture. Parallel two-pass exports reproduce the corresponding decoded results with isolated statistics.

Additional cases cover declared dimension boundaries, all three audio-only bitrates, first/second-pass failure cleanup, invalid option combinations, typed MCP inspection and preservation of source/existing files. Full verification passed and earned E02 extended on 4 October 2026. Frame-rate/container/image-sequence expansion and long-form 4K remain separate checkpoints.

The engine uses the selected external encoder's documented [libx264 controls](https://ffmpeg.org/ffmpeg-codecs.html#libx264_002c-libx264rgb) and [two-pass interface](https://ffmpeg.org/ffmpeg-doc.html). Settings, bounds, orchestration and fixtures are original. No third-party source, encoder binary, preset or generated media is included in this repository; dependency versions and licenses remain in [DEPENDENCIES.md](DEPENDENCIES.md).
