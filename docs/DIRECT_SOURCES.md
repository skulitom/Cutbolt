# Compressed-source inspection

The source index and video library implement part of [direct-footage stage 1](plans/DIRECT_FOOTAGE_PLAN.md).
It does **not yet enable direct timeline playback**. Continue to use the existing preparation
workflow; `timeline.ready` and `source_index.direct_decode_available` remain false for these files.

`media.inspect` adds `source_index` for H.264, HEVC and ProRes video. It records decoded presentation
order, rather than treating compressed packets as frames. The summary includes the presented frame
count, first timestamp and stream time base, native frame rate when detected, keyframe count,
maximum GOP length, B-frame count, qualification reasons and cache key/hit/bytes. `eligible` reports
qualification for the indexed decoder, not current timeline support. An indexing error is nested in
this summary, leaving the metadata inspection available.

One incremental ffprobe pass reads frame timestamps, key flags, picture types and decoded format
and color changes. One FFmpeg audio decode supplies a PCM byte counter and a small frame-clock
stream concurrently. Neither PCM nor a whole decoded-video stream is written to disk. The audio
summary reports actual decoded samples, native rate/channels, first PCM timestamp, continuity,
optional codec initial padding, and mono duplication/resampling needs. Packet counts cannot replace
this measurement for AAC. `audio_policy` now reports the exact first-audio offset relative to video,
leading/trailing padding or trimming, mono duplication and linear 48 kHz resampling choices.
These policies are verified by [explicit conform clock normalization](CONFORM.md#explicit-source-clock);
direct playback remains subsequent work.

Qualification currently requires one video stream, at most one mono/stereo audio stream and no
other stream types; MP4/MOV/MKV; H.264 or HEVC Main, even-sized 8-bit 4:2:0 progressive pictures;
square pixels without a display matrix; and one of the eight native CFR clocks relative to its
first presented frame, rounded at container precision. It requires stable, explicit BT.709 matrix
and primaries, full/limited range, BT.709 or sRGB transfer, and left chroma placement. Missing SAR
uses the existing converter's square-pixel assumption and sets `square_pixels_assumed`. AAC, MP3,
FLAC, Opus and PCM are candidates; decoded audio must follow its cumulative sample clock within
1 ms, allowing container timestamp quantization without permitting accumulated drift. Unsupported cases report
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

The retained original 20-minute 1080p30 fixture also passes these checks: 36,000 presented frames,
600 keyframes, 26,400 B-frames and 57,600,000 decoded stereo samples. Its compressed index occupies
176,032 bytes in the original index-foundation build for a 2,768,314,141-byte MP4. A fresh-process second inspection launches only metadata
probing. These are storage/correctness observations, not a preview or seek-performance result.

## Exact color conversion foundation

The library's `color::Yuv420Converter` provides the indexed decoder's exact lookup path.
It converts native planar 8-bit 4:2:0 to packed RGB with the same nearest-2x2 chroma interpretation,
BT.709 matrix, full/limited range, clipping and rounding as `color::Input::convert`. The f64
converter remains unchanged and is still used by explicit conform. This component is not yet wired
into timeline playback.

Each table maps all 2^24 Y/U/V triples to RGB and uses exactly 48 MiB. Tables are built lazily and
shared in memory. Equal source/output transfers have identical arithmetic and share storage;
the two ranges and three distinct transfer mappings bound retained table storage to 288 MiB.
Construction can temporarily hold another table-sized buffer while transferring ownership.
Media tag validation remains the caller's responsibility. The converter rejects an RGB matrix,
nonpositive/odd dimensions and buffers whose lengths do not match one complete frame.

The ordinary Rust unit suite exhaustively compares every table entry with the unchanged f64
converter for all eight range/input-transfer/output-transfer declarations on Windows and Linux.
A separate planar 4:2:0 frame test checks layout/chroma addressing, bounds and invalid buffers.
This proves numerical equality; renderer wiring and measured preview performance remain open.

## Indexed video library

`direct::Video::open(input_root, identity, control)` opens a pinned relative source using its cached
presentation index. `read(first, count, output_transfer, control, callback)` streams packed RGB24
frames to the callback with their source frame indices. It retains at most one native YUV frame
and one RGB frame, plus the index and shared color table. Dimensions are limited to 4096 per axis
and 8,000,000 pixels. Empty, overflowing and out-of-source ranges fail before a decoder starts.

The decoder preserves native `yuv420p` or `yuvj420p`, disables autorotation, retains the source's
absolute timestamps and trims in the index's integer time base. Seeking uses the existing
one-third-frame margin with the signed first-video timestamp added before microsecond rounding.
B pictures conservatively require a preceding GOP as preroll, covering open-GOP references. The
receipt records the index key, requested range, selected seek frame and actual seek seconds. The
decoder must end successfully with exactly the expected number of frames; no frame-count output
limit hides extra frames. Its raw bytes pass through the exact Rust table, never FFmpeg RGB scaling.

The preliminary check can reuse an unchanged source's verified hash as described below; every read
fully hashes the source before returning success. Callback output is provisional until `read`
succeeds; a caller must delay publication accordingly. The API creates no intermediate media and
does not publish files. Audio decoding, saved direct
asset bindings and timeline/render/preview/export integration are also pending; the video library
does not change the CLI/MCP readiness flags.

The 26 original short fixtures include all eight native rates, full-range YUV and both H.264 and
HEVC open GOPs. For each of the 21 qualifying sources, the fixture compares nine decoded windows
with full explicit conform RGB, including first/key/B/last frames, a cut across a GOP and the whole
source. It inspects actual decoder arguments, exercises invalid ranges, and checks that sources
changed before or during reading cannot return success. The explicit conform reference also
checks every RGB byte and PCM sample against independent arithmetic. These are correctness checks;
long-source seek latency and direct timeline acceptance remain unverified.

## Approved identity reuse

The user approved metadata-based reuse with a full hash before publication. The process now
remembers up to 4,096 full file hashes, evicting the least recently used entry when needed. Reuse
requires the same canonical path, byte count, modification time and OS file identity: the volume
serial and full 128-bit file ID on Windows, or device and inode on Linux. Missing metadata or file
ID support disables reuse. A full hash is remembered only if both its open handle and current
path retain the original fingerprint after hashing.

The direct video library uses reuse only for its preliminary read check. Its final verification,
all existing publication checks and public file-hash functions still read every byte. Other
render gates retain full preliminary hashing until their publication paths are integrated and
audited. The cache is in memory; a new process starts empty. No source bytes or identities enter a
new persistent file. Same-size in-place edits with deliberately restored modification times can
pass the preliminary check; the mandatory full hash detects them before a successful receipt.
The unit test covers this approved tradeoff, modified metadata, replacement file IDs and refresh
after a full hash.

Both open-GOP fixtures now span six GOPs (360 frames), so the near-end windows exercise later
random-access points beyond initial preroll. Existing frame-digest oracles remain unchanged.

## Approved bounded audio cache

Direct audio experiments found a reproducible AAC seek counterexample with the original fixture
and FFmpeg 7.0-full_build-www.gyan.dev: for samples 283,200 through 287,999 of `h264-25`, a late
seek changed 1,949 channel samples compared with full decoding, by at most 13 PCM16 levels. Extra
preroll did not make every later block equal. The proposed audio reader detects this with indexed
PCM block hashes and rejects the range rather than returning different samples. It also handles
Opus container-clock rounding by matching block content and sample positions.

That audio work is not shipped. On 11 October 2026, the user approved a bounded, content-verified audio cache for codecs whose seeks cannot reproduce the full decode. This explicitly changes stage 1.1's prohibition on storing PCM on disk for that fallback. The cache must preserve the exact full-decode reference, verify cached content, enforce storage bounds and preserve original sources and direct video decoding. Implementation and direct timeline integration remain pending; this approval is not a direct-audio or stage-1 completion claim.
