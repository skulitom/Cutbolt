# Local cache and contact-sheet previews

`cache.run` reuses inspected metadata, proxy media, timeline frames, preview intervals and contact sheets by content identity. `preview.sheet` creates a contact sheet directly. Both are blocking JSON CLI/library commands. `cache.inspect` and `cache.prune` are also available over local MCP stdio. No listening service, download or background cache worker is introduced.

Both cache checkpoints and contact-sheet acceptance pass the full verifier completed 4 October 2026. The generated [core tracker](PROGRESS.md) remains authoritative.

## Explicit cache requests

Supply an existing absolute `cache_root`, a `policy` with `max_bytes` and `max_entries`, and one `task`. Keep the cache outside the repository, separate from original media and saved-session stores. Use trusted local storage with normal file locking and hard-link support; Windows NTFS is the tested filesystem.

| Task type | Required fields besides `type` |
| --- | --- |
| `probe` | `path`, `input_root` |
| `proxy` | `project`, `expected_revision`, `asset_id`, `scale`, `input_root`, `output_root`, `output` |
| `frame` | `project`, `time`, `input_root`, `output_root`, `output` |
| `interval` | `project`, `start`, `duration`, `input_root`, `output_root`, `output` |
| `sheet` | `project`, `spec`, `input_root`, `output_root`, `output` |

`scale` follows existing half/quarter/eighth proxy generation. Proxy requests require a bound original asset. Frames and sheets publish PNG; proxies and intervals publish the existing lossless MKV profile. Every file request needs a new unused destination. All underlying source, timeline, frame-count, dimension and native 25 fps/48 kHz restrictions still apply. Unsupported inputs reject explicitly.

```json
{
  "command": "cache.run",
  "cache_root": "C:/DEV/CutboltData/cache",
  "policy": {"max_bytes": 268435456, "max_entries": 128},
  "task": {
    "type": "probe",
    "path": "C:/DEV/CutboltData/media/shot.mkv",
    "input_root": "C:/DEV/CutboltData/media"
  }
}
```

The response contains `result` and `cache`. A probe returns the current source path, identity and metadata. File tasks return the published path, payload identity and task details. A proxy additionally returns its binding, proposed attachment operations and resulting pure project snapshot; save the operations through the existing session contract. A hit never commits editing state implicitly.

`cache` reports the key, producer fingerprint, hit/miss, storage decision, evicted keys, actually verified source identities and elapsed microseconds. A successful cache hit still verifies sources, selected external tools and cached payload bytes. Repeated calls therefore retain I/O costs; they are not constant-time lookups. Cold/warm figures measure missing/present application cache entries, not cold/warm operating-system file caches.

## Identity and invalidation

Keys include the task kind and parameters, normalized project snapshot, source byte counts and SHA-256 identities, selected external tool byte identities and compiled producer identity. Project normalization removes source/proxy locations but retains IDs, revisions, source windows, transitions, nested definitions, preview selection and other project state. Identical content at a relocated path can reuse an entry. A new revision conservatively invalidates even if rendered pixels would be unchanged; undo/restoration uses its new revision. The cache does not try to prove two different editing graphs equivalent.

The producer fingerprint includes original compiled Rust source, embedded worker source, build source, Cargo manifests/lockfile, compiler version, target and declared build configuration. It is compiled into the executable, so installed binaries do not need the development checkout. Changing source/dependency selection invalidates old entries. This is a reproducibility identity, not a signature or a guarantee for arbitrary modifications to external dependency caches or compiler internals.

All source bytes are hashed independently of timestamps or file sizes. Bound source/proxy mismatches reject; unbound probe changes produce a new key. External executable paths may relocate with identical bytes; changed bytes invalidate even when version text is unchanged. Preview tasks follow the saved full/proxy selection, including nested sources and full-quality WAV audio. Final exports continue to use originals. Source and tool identities are rechecked before publication. This does not lock external media against a hostile writer after the final check.

Payloads and metadata have separate hashes; metadata is also bound to its key. An accidentally corrupted owned entry is removed and rebuilt before returning output. These hashes detect corruption, not deliberate rewriting by an attacker who can also rewrite the hashes. Database-wide corruption produces an explicit error. Unrelated databases are not adopted or cleared.

## Storage and failure behavior

The root contains one owned `cutbolt-cache.sqlite` database. Payload BLOBs stream through 64 KiB buffers. SQLite transactions protect entry insertion, least-recently-used eviction and hit recency. Limits are **0–1 GiB of payload bytes and 0–4,096 entries**. Metadata is at most 64 KiB per entry. A zero budget clears stored entries and bypasses storing the current result; an output larger than the payload budget is produced normally without being cached.

`max_bytes` counts payloads only. Metadata, indexes, SQLite pages, rollback journals and temporary production/publication files require additional disk space. `cache.inspect` reports logical `payload_bytes` and physical `database_bytes` separately. Pruning reclaims database payload pages through automatic vacuuming. It removes only derived entries, never source media, delivered outputs or editing history. A lower policy is applied at the start of a request, so a later failed production can still have pruned older derived entries.

`cache.prune` takes `cache_root` and `policy`; it evicts the least recently used entries until both limits are met. Every successful hit updates a monotonic access order. Concurrent duplicate misses may both compute an output; one complete entry is retained. Each caller still publishes to its own unused destination. A destination collision fails at preflight or atomic publication without overwriting it. Cache and output roots may be on different filesystems: delivery copies to an owned temporary file beside the destination before hard-link publication.

Rollback journaling, full synchronization and checked initialization protect completed entries during failed inserts or process interruption. Tests cover an actual hot rollback journal and SQLite's full-database error; they do not guarantee hardware or power-loss behavior. The five-second lock timeout can return `STORE_BUSY` under contention. A killed producer may leave a known `.cutbolt-cache-*` temporary file or a native producer's scratch directory; cleanup of orphaned temporary files is not automatic. Normal return/error paths remove their owned temporary files. Cache initialization/publication never recursively deletes a directory.

## Source inspections

Renders, previews and exports also reuse **verified source inspections**, separately from `cache.run`. No request field is involved: a workspace keeps them in `.cutbolt/cache/inspections`, and `CUTBOLT_INSPECTION_CACHE` can name another absolute directory. An entry is keyed by the source's SHA-256 and size, the inspection's parameters (size, frame rate, alpha acceptance, frame-by-frame or packet timing), the ffprobe executable's SHA-256 and the engine build. Every command still hashes every source it reads, so changed bytes, even at the same path, size and timestamps, are always inspected again.

- **Outputs are entered as they are written.** `scene.render`, `captions.render`, `render.run` and `media.conform`/`media.prepare` verify their output before publishing it, and that pass becomes an entry. It also records the inspections it proves:
  - an opaque `bgr0` file passes the alpha-overlay check unchanged, since that check only also accepts `bgra`;
  - a frame-by-frame pass whose packets carry the same frame times proves the packet-timed check, which reads its audio from the same packets;
  - a packet-timed pass never stands in for a frame-by-frame one.
- **Audio-only use is packet-timed.** A source that only feeds audio tracks in a render is timed from its FFV1 packets, as in audio-only exports, and its pictures are not decoded. The same source on a video track is decoded frame by frame.
- **Parallel inspection.** One render, preview or export inspects the sources it reads up to four at a time, largest file first, and contact sheets read four cells at a time. A request for an inspection already running in the process waits for it rather than decoding the file again. The queue's own render worker keeps inspecting one source at a time.

On the 80.64 s 1080p25 progress-demo timeline (14 scene shots, a full-length caption overlay, a picture-in-picture clip, 7 narration assets and an 80 s music asset), inspecting all 25 sources with no entries took 5.7–6.2 s, against 19.9–22.3 s one at a time and about 100 s on one decoder thread. The same machine was busy with other work. The cold and warm review calls are in [PROGRESS_HISTORY.md](PROGRESS_HISTORY.md).

## Contact sheets

`preview.sheet` accepts `project`, `spec`, `input_root`, `output_root` and an unused PNG `output`. The cached `sheet` task uses the same fields. A spec is explicit:

```json
{
  "times": [{"num": 0, "den": 1}, {"num": 12, "den": 25}],
  "columns": 2,
  "tile_width": 320,
  "tile_height": 180,
  "gap": 8,
  "background": [24, 24, 24]
}
```

Times are exact timeline positions, retained in caller order. Duplicates are allowed. Every time must be a native frame boundary before the timeline end. Limits are 1–64 times, 1–8 columns, 1–1,920 tile width, 1–1,080 tile height, 0–32 gap pixels and at most eight million final pixels. The last row retains unused cells in the declared background color. Frames are contained and centered within each tile, preserving aspect with integer output dimensions and nearest center sampling. This is an inspection thumbnail, not a new color/grading transform. Receipts report sheet dimensions and each cell's timeline frame, rectangle and original frame dimensions. Gaps, nested sequences, transitions and full/proxy selection use the same timeline frame path as other previews. Cells are read four at a time and placed in order; when cells fail, the first failing cell's error is reported.

## Acceptance evidence

`tests/cache_previews.py` generates original coded RGB and stereo PCM sources. It independently composes timeline gaps, nested transitions and contact-sheet sampling, then compares every decoded pixel/sample. It checks cold/hit byte agreement, full-quality exports while proxy preview is selected, moved media, equal-size/equal-timestamp changes, edited revisions and source windows, tool content changes, corrupt entries, saved retries/undo/restoration, LRU count/byte eviction, budget bypass, actual concurrent native callers, publication collisions, roots, limits and typed MCP. It renders actual 64-cell and eight-million-pixel sheets and measures three cold/warm repetitions per task kind. The full verifier also runs storage crash/full-database recovery tests and all earlier editing acceptance.

The fixture validates the 1 GiB/4,096-entry policy boundaries; it does not claim a full 1 GiB cache load benchmark. Measured timings are for the documented local development environment and fixture sizes. Larger or slower media retain the cost of source/tool content checks.
