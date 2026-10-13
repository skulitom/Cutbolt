# Saved source preparation

Adding a supported original video with `media.add` and `input_root` now chooses a source
profile for the project's canvas and frame rate. Matching indexed H.264/HEVC originals stay
direct. Matching reference files stay native. Other supported videos save
`decode: {"mode":"converted","recipe":...}` without writing converted media during the edit.
Omitting `input_root` retains metadata-only addition of an already described asset.

The saved recipe is visible, identity-bound and checked before the edit commits. It keeps
the source path and complete original identity. It declares the target dimensions, exact
frame rate, duration, color interpretation and any display fit. The asset duration must equal
the proposed complete prepared duration; a mismatch reports `actual_duration` instead of
silently changing an edit. The existing conversion matrix, timing rules and bounds in
[CONFORM](CONFORM.md) still apply. HDR and premultiplied alpha are unsupported here. Native
alpha conversion declares straight alpha, whose RGB is independent of coverage; use an
explicit recipe when that interpretation needs review. Audio-only normalization retains the
explicit preparation path.

`job.start` commands such as `export.run` prepare missing conversions automatically. Their
`preparing sources` phase counts source assets. A working set is prepared before read leases
are acquired; its members cannot evict each other during production. The renderer retains
the leases while reading, and fully rechecks original and tool identities before publishing.
Reference, H.264, audio, frame and range rendering use the same prepared content once present.
Preparations are derived files: they do not create new saved-project revisions.

A synchronous command with a missing or corrupt conversion returns `PREPARATION_REQUIRED`,
the affected asset ID and a complete `job.start` request in `detail.request`. That request uses
`run: "media.prepare"` with `cached: true`, the project and its input root. Its `job_root` is the created engine-state directory `input_root/.cutbolt/jobs`, so the
request runs unchanged outside a workspace too. Each new error supplies a new request ID
so a previously completed preparation ticket cannot mask a later eviction. Retry the same
ticket normally until it finishes, then retry the preview.

`media.prepare` with `cached: true` prepares the project's referenced converted sources, with
no delivery copy. It requires `project`, `input_root` and `output_root`; omit `path`, `paths`,
`output` and transcripts. Its result includes prepared assets with `provenance.original`
and `provenance.conversion`, plus current source/cache receipts. Original identity remains
authoritative; a conversion identity describes that materialization, which can be rebuilt.
The ordinary file-based `media.prepare` behavior is unchanged. The older `render.start`
worker uses the same supervised preparation subprocess, then restores render frame progress.
Its cancellation and watchdog also cover preparation; it does not complete the parent ticket
until the requested render is published.

The workspace default cache is `.cutbolt/cache/conform`, beside its inspection cache, even
when a request selects a narrower input root. Without a workspace it defaults to
`input_root/.cutbolt/cache/conform`. The cap is 50,000,000,000 bytes and 4096 entries. `CUTBOLT_CONFORM_CACHE`
selects another absolute directory; `CUTBOLT_CONFORM_CACHE_BYTES` selects a positive cap up to
1 TiB. These settings are local process configuration. This is a ceiling, not an allocation.
The full working set must fit. Explicit `cache.run` tasks retain their explicit policies.
`cache.inspect` and `cache.prune` with `kind: "conform"` expose the same cache family.

`media.inspect` reports `timeline.conform.cache.expected_bytes` for proposed video recipes.
It is the uncompressed reference picture plus PCM payload used for planning, not an encoded
size guarantee. Actual encoding enforces the configured byte ceiling, including in-flight
reservations; index metadata, existing bounded raw decode scratch and requested delivery
copies are separate. Conversion or cancellation can already have evicted unrelated derived
entries. Originals and completed outputs are preserved.

The `automatic_prepare` fixture compares queued exports of original VFR, rotated H.264,
HEVC Main10 and ProRes 422 files with explicit conversions of their recorded recipes. It
compares all decoded H.264 frames and PCM samples, counts one first-use conversion and zero
second-use conversions, checks persisted asset round-trips, runs the missing-cache repair
request, prepares two absent sources together, names a missing second source correctly, automatically
prepares background reference renders, tests cancellation during encoding and enforces a
reduced byte cap. Saved-recipe rejection, workspace defaults and a transparent ProRes
overlay against its explicit conversion are also checked. These are
small original synthetic sources; no broad codec, long-form conversion speed or new editing
points are claimed.
