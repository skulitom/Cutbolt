# Compressed-source inspection

The source index, saved direct bindings and exact frame previews implement part of
[direct-footage stage 1](plans/DIRECT_FOOTAGE_PLAN.md). Original compressed clips can now supply
single-frame previews on sequential or ordinary opaque tracks. Range rendering, transitions,
overlays and exports still need the preparation workflow; `timeline.ready` and
`source_index.direct_decode_available` remain false until the full render path is integrated.

`media.inspect` adds `source_index` for H.264, HEVC and ProRes video. It records decoded presentation
order, rather than treating compressed packets as frames. The summary includes the presented frame
count, first timestamp and stream time base, native frame rate when detected, keyframe count,
maximum GOP length, B-frame count, qualification reasons and cache key/hit/bytes. `eligible` reports
qualification for the indexed decoder. Eligible files also return a bound `source_index.asset`
and `frame_preview_available: true`; these fields do not claim range/export support. An indexing
error is nested in this summary, leaving the metadata inspection available.

One incremental ffprobe pass reads frame timestamps, key flags, picture types and decoded format
and color changes. One FFmpeg audio decode supplies a PCM byte counter and a small frame-clock
stream concurrently. Video frames are not written to disk. With an explicitly configured inspection cache, AAC also
retains bounded canonical PCM chunks as described below. The audio summary reports actual decoded
samples, native rate/channels, first PCM timestamp, continuity,
optional codec initial padding, and mono duplication/resampling needs. Packet counts cannot replace
this measurement for AAC. `audio_policy` now reports the exact first-audio offset relative to video,
leading/trailing padding or trimming, mono duplication and linear 48 kHz resampling choices.
These policies are verified by [explicit conform clock normalization](CONFORM.md#explicit-source-clock);
range rendering remains subsequent work.

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
converter remains unchanged and is still used by explicit conform. Single-frame direct timeline
previews now use the same table as the library reader.

Each table maps all 2^24 Y/U/V triples to RGB and uses exactly 48 MiB. Tables are built lazily and
shared in memory. Equal source/output transfers have identical arithmetic and share storage;
the two ranges and three distinct transfer mappings bound retained table storage to 288 MiB.
Construction can temporarily hold another table-sized buffer while transferring ownership.
Media tag validation remains the caller's responsibility. The converter rejects an RGB matrix,
nonpositive/odd dimensions and buffers whose lengths do not match one complete frame.

The ordinary Rust unit suite exhaustively compares every table entry with the unchanged f64
converter for all eight range/input-transfer/output-transfer declarations on Windows and Linux.
A separate planar 4:2:0 frame test checks layout/chroma addressing, bounds and invalid buffers.
This proves numerical equality; range-renderer wiring and measured long-source performance remain open.

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
does not publish files. Saved bindings and single-frame previews use this library. Full timeline
render/export integration remains pending; the general CLI/MCP readiness flags remain false.

The 26 original short fixtures include all eight native rates, full-range YUV and both H.264 and
HEVC open GOPs. For each of the 21 qualifying sources, the fixture compares nine decoded windows
with full explicit conform RGB, including first/key/B/last frames, a cut across a GOP and the whole
source. It inspects actual decoder arguments, exercises invalid ranges, and checks that sources
changed before or during reading cannot return success. The explicit conform reference also
checks every RGB byte and PCM sample against independent arithmetic. These are correctness checks;
long-source seek latency and complete direct timeline acceptance remain unverified.

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

## Exact audio ranges and approved bounded cache

`direct::Video::read_audio(first, count, control, callback)` reads a range on the first-video-frame
clock, producing stereo PCM16 at 48 kHz. Native PCM block timestamps, cumulative sample positions
and SHA-256 values in index schema 2 anchor sought decodes to the complete source decode. It uses
exact rational linear resampling, duplicates mono, trims audio before/after the video and pads
missing samples with silence. A video without audio produces bounded silent blocks. The callback's
bytes are provisional until the call succeeds and the original source has been fully rehashed.

PCM, MP3, FLAC and Opus use a two-second preroll before the required block. Both raw PCM bytes and
framehash records are consumed concurrently through a bounded queue. The reader requires exact
block contents and a unique indexed match within the existing 1 ms container-clock tolerance;
that permits Opus timestamp rounding without shifting a sample. It rejects a skipped block,
changed PCM, excess/missing data, decoder failure or source change. It never assumes that preroll
alone proves equality.

AAC has a reproducible exception with the original fixture and Windows FFmpeg 7.0: seeking
samples 283,200 through 287,999 of `h264-25` changed 1,949 channel samples compared with full
decoding, by at most 13 PCM16 levels. Extra preroll did not make every later block equal. On
11 October 2026, the user approved a bounded, content-verified cache for this case, explicitly
amending stage 1.1's prohibition on PCM on disk.

The original full AAC indexing pass records hashes of fixed 1 MiB chunks of native interleaved
PCM16, plus the final short chunk. With a configured inspection directory, their payloads live
in its `audio/cutbolt-cache.sqlite` sub-cache. A workspace therefore uses
`.cutbolt/cache/inspections/audio`. This is an owned cache using the existing SQLite integrity,
transactional eviction and readback checks: at most 1 GiB of payload and 4,096 entries (database
and journal overhead are additional), at most 1 MiB per chunk and temporary copy. Index metadata
is bounded to 65,536 chunk descriptors. Sources and decoded video stay untouched.

Chunk keys include the source-index key (source identity, decoder/probe executable identities,
engine producer and index schema), native byte range and canonical chunk hash. Reads compare
against the trusted source index as well as the cache's own integrity fields. Chunk data emitted
by an unsuccessful indexing pass is never sufficient to establish a valid index or receipt;
its content-dependent key cannot substitute different bytes for a subsequently verified chunk.
Missing or corrupt entries are rebuilt from a complete AAC decode and must match every indexed
chunk before successful completion. This fallback streams through a two-chunk queue; it does
not buffer a whole recording. Once present, a requested range reads only the needed chunks and
launches no audio decoder. Without an explicitly configured cache directory, AAC ranges use a
complete decode with those same index checks and retain no PCM on disk.

Receipts distinguish `silence`, `verified_seek`, `canonical_cache` and `canonical_decode`, report
sought seconds where applicable, native decoded samples, matched seek blocks, cached chunks and
rebuilt chunks. `cache.inspect`/`cache.prune` accept the explicit audio sub-cache root. A prune or
source/tool/engine change cannot authorize stale PCM reuse. This library work does not yet make
direct sources ready for saved timeline rendering; the stage-1 integration and long-source
performance gates remain open.

Windows and Linux pass all 189 audio windows across the 21 qualifying synthetic sources against
complete explicit-conform PCM, including offset/short/long audio and 44.1 kHz mono. Tests also cover
cache reuse across processes without an audio decoder, full repair after corruption, public prune
and rebuild, the 1 MiB chunk/1 GiB payload bounds, source changes, decoder-byte corruption and
preservation of a callback failure. A unit check rejects even self-consistent cache tampering
against the trusted index and verifies LRU eviction. These checks accompany the existing video
oracles and cache-preview regression fixture on both platforms.

## Saved bindings and frame previews

Pass `media.inspect`'s `source_index.asset` to `media.add`, or give that operation an explicit
`input_root` to inspect and bind an original compressed source while adding it. Without the root,
legacy metadata-only adds retain their existing behavior. With the root, qualification, actual
size/rate, complete duration and any supplied identity/index must agree before the edit commits.
The snapshot stores the original identity and `decode: {"mode":"direct","index":"<key>"}`;
it contains no index frames or prepared-media copy. The additive optional field retains project
schema 1; older engines reject it through their existing unknown-field rule.

A minimal operation inside `timeline.apply` or `session.apply` is:

```json
{"op":"media.add","input_root":"C:/Media","asset":{"id":"shot","path":"shot.mp4","duration":{"num":12,"den":1}}}
```

The duration must be the complete indexed video length. Place its clips with the usual operations.
`preview.frame` reads original frames on sequential timelines and the single opaque-clip path of
placed tracks, with the exact native clock, seek preroll and Rust color conversion. It uses the
project's declared transfer, defaulting to BT.709 as the existing preparation recipe does. Its
receipt includes the source frame, bound source identity and direct decoder receipt. No video
intermediate is created, and publication still requires a full original-file hash.

With an input root, `timeline.check` reports missing/corrupt or stale direct indexes without
rebuilding them. A missing cache entry is rebuilt on the next preview. A binding for a different
engine/decoder identity fails with `DIRECT_INDEX_STALE`, with a repair hint. Apply
`{"op":"media.index","asset_id":"shot","input_root":"C:/Media"}` to verify the unchanged
original and refresh its index. It cannot replace the bound original identity. Workspace calls
resolve explicit operation roots inside the workspace; `media.index` defaults its omitted root
there. Refreshes participate in asset diffs and undo. Saved-request replay precedes all source I/O,
so retrying a committed add or refresh returns the original receipt even if the source later changes.

The Windows/Linux fixture compares 168 actual sequential/opaque-track PNG previews across all
21 eligible original sources with the complete explicit-conform RGB reference: first, key,
B and last frames at all eight native clocks. An additional declared-sRGB preview matches the
unchanged f64 conform path. It checks missing/stale index diagnosis and repair, asset diffs,
undo, replay after source mutation, failed-refresh rollback, source identity preservation,
size/rate/duration mismatches and absence of publication for rejected sources. The unmodified
native_timing, sessions, proxies and agent_costs fixtures pass on both platforms. No complete
stage-1 exit or long-source latency result is claimed yet.
