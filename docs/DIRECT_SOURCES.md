# Compressed-source inspection

The source index is the first part of [direct-footage stage 1](plans/DIRECT_FOOTAGE_PLAN.md).
It does **not yet enable direct timeline playback**. Continue to use the existing preparation
workflow; `timeline.ready` and `source_index.direct_decode_available` remain false for these files.

`media.inspect` adds `source_index` for H.264, HEVC and ProRes video. It records decoded presentation
order, rather than treating compressed packets as frames. The summary includes the presented frame
count, first timestamp and stream time base, native frame rate when detected, keyframe count,
maximum GOP length, B-frame count, qualification reasons and cache key/hit/bytes. `eligible` reports
qualification for the planned decoder, not current render support. An indexing error is nested in
this summary, leaving the metadata inspection available.

One incremental ffprobe pass reads frame timestamps, key flags, picture types and decoded format
and color changes. One FFmpeg audio decode supplies a PCM byte counter and a small frame-clock
stream concurrently. Neither PCM nor a whole decoded-video stream is written to disk. The audio
summary reports actual decoded samples, native rate/channels, first PCM timestamp, continuity,
optional codec initial padding, and mono duplication/resampling needs. Packet counts cannot replace
this measurement for AAC. Padding/trimming relative to video and playback remain subsequent work.

Qualification currently requires one video stream, at most one mono/stereo audio stream and no
other stream types; MP4/MOV/MKV; H.264 or HEVC Main, even-sized 8-bit 4:2:0 progressive pictures;
square pixels without a display matrix; and one of the eight native CFR clocks relative to its
first presented frame, rounded at container precision. It requires stable, explicit BT.709 matrix
and primaries, full/limited range, BT.709 or sRGB transfer, and left chroma placement. Missing SAR
uses the existing converter's square-pixel assumption and sets `square_pixels_assumed`. AAC, MP3,
FLAC, Opus and PCM are candidates; decoded audio must be continuous. Unsupported cases report
reasons, never a successful render claim.

Indexes are gzip-compressed JSON in the inspection-cache directory: by default a workspace's
`.cutbolt/cache/inspections`, or `CUTBOLT_INSPECTION_CACHE`. Without a configured directory they are
ephemeral. Keys bind the source SHA-256/byte count, both tool executables' SHA-256/byte counts,
engine producer and index schema. Payload digests, key/schema/source checks and bounded decompression
guard reuse. Atomic no-overwrite publication retains existing entries. Invalid entries are ignored
and recomputed; deleting an invalid derived entry permits persistent replacement. No index enters
project JSON. Source bytes are rehashed after indexing, before publishing its derived cache entry.

Bounds are 4,000,000 presented frames, 8,192 bytes per tool record, 256 MiB expanded cache data and
30 minutes per indexing tool. Per-key serialization and the existing four inspection lanes bound
concurrent work. The original synthetic `direct_sources` fixture compares independent decoded
frame/PCM observations, preserves input hashes and counts tool launches: the first inspection uses
metadata, frame and audio passes; a fresh-process cache hit uses only metadata. Cache unit tests
reject wrong identity/key/schema, payload-digest changes and corrupt compressed data.
