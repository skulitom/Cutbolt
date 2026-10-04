# Cutbolt local engine

The optional [foreground segmentation workflow](SEGMENTATION.md) accepts annotated PNG frames, retains versioned masks and returns ordinary scenes. The native renderer accepts content-bound per-frame binary mattes without the optional worker runtime.

Optional scene [geometry](GEOMETRY.md) provides textured planes with perspective/orthographic cameras, parent transforms and declared lighting through the existing scene inspection/render workflow.

The [property-expression contract](EXPRESSIONS.md) describes optional scene graphs and `expression.inspect`: read-only exact value sampling with typed links, cycle checks and reproducible seeded inputs. Complete media validation and compilation remain `scene.inspect` and `scene.render`.

The executable accepts one JSON request on stdin, or a request-file path as its only argument. `cutbolt capabilities` is a shorthand. It returns a short summary: essentials, commands, operations and the names of the detailed sections. Add `"section"` with one of those names, or `"all"`, for details. It returns one JSON object on stdout: `{"ok":true,"result":...}` on success or `{"ok":false,"error":{"code":"...","message":"..."}}` with exit code 1 on failure. Unknown fields and operations are rejected. Requests are limited to 4 MiB.

Snapshot/media commands: `capabilities`, `schema`, `project.create`, `project.validate`, `timeline.apply`, `media.inspect`, `render.plan`, `render.run`, `export.inspect`, and `export.run`. Saved editing sessions add `session.create`, `session.get`, `session.apply`, `session.preview`, `session.undo`, `session.restore`, `session.history`, and `session.receipt`. Background commands are `render.start`, `job.start` (any long-running command, such as `export.run`, `media.conform` or `scene.render`), `job.status`, `job.wait`, `job.cancel`, and `job.resume`. `cutbolt mcp` provides a local stdio adapter. There is no listening socket, installed service, or network request in the engine's normal execution path. See [AGENT_INTERFACE.md](AGENT_INTERFACE.md) for jobs and MCP setup.

`{"command":"schema","name":"scene.render"}` returns a command's argument schema, with a description on every field. Large schemas come back as an outline; `select` returns one variant or definition, and `full` returns everything. It also accepts the shared types `project`, `operation`, `scene`, `template` and `audio_routing`.

`cutbolt --workspace DIR ...`, or the `CUTBOLT_WORKSPACE` environment variable, fixes one directory as the boundary for requests. Omitted roots then default inside it (sessions in `.cutbolt/store`, jobs in `.cutbolt/jobs`), relative paths resolve against it, explicit roots must lie inside it, and paths the engine reports come back relative to it. Every command that takes a `project` also accepts `{"project_id": "...", "revision": N}` and loads that saved revision. See [the workspace and reference rules](AGENT_INTERFACE.md#workspace).

## Project snapshots

The [transcript editing contract](TRANSCRIPTS.md) adds `transcript.inspect`, `transcript.correct`, `transcript.plan` and the content-bound `transcript.cut` operation. The blocking `transcript.transcribe` command and both G04 checkpoints pass bounded local English/Greek acceptance; estimated intervals remain subject to explicit review.

`project.create` accepts `id`, `width`, `height`, and rational `frame_rate`. The response is a project snapshot containing schema version 1, revision 0, assets, and an ordered list of clips. Save its JSON directly, and send the same shape back to validate or edit it. For a shared authoritative local head and history, use the saved-session commands below.

`timeline.apply` accepts the entire `project`, `expected_revision`, and an `operations` array. Its result is a new snapshot with one higher revision. The supplied snapshot is immutable, and failed batches produce no new state. The revision check is against that snapshot, not a shared authoritative store; it does not prevent two callers from forking the same project. Persisting this pure snapshot result is the caller's responsibility. `timeline.apply` remains useful for offline transforms; use `session.apply` when retries and shared revision checks must survive process restarts.

| Operation | Additional fields | Meaning |
| --- | --- | --- |
| `media.add` | `asset`: `id`, `path`, `duration` | Declare an external source; rendering verifies its actual contents |
| `clip.append` | `clip`: `id`, `asset_id`, `source_in`, `duration` | Append a source interval |
| `clip.trim` | `clip_id`, `source_in`, `duration` | Replace a clip's selected source interval |
| `clip.split` | `clip_id`, `new_clip_id`, `offset` | Split at an offset measured from the start of this clip |
| `sequence.create` | `id`, `duration` | Create an empty reusable native child definition |
| `sequence.edit` | `id`, `edit` | Apply a native track edit inside a shared child; see [nested sequences](SEQUENCES.md) |
| `sequence.remove` | `id` | Remove an unreferenced child whose tracks are unlocked |
| `multicam.create` | `id`, `group` | Create a managed child with retained synchronized camera alternatives |
| `multicam.edit` | `id`, `edit` | Change camera cuts, alternatives, audio policy or lock state; see [camera groups](MULTICAM.md) |
| `clip.move` | `clip_id`, `to_index` | Move to a zero-based index in the resulting list |
| `clip.remove` | `clip_id` | Remove the clip and close the gap |
| `clip.insert` | `at`, `clip`, optional `right_id` | Insert at a timeline frame boundary and shift later content right |
| `clip.overwrite` | `at`, `clip`, optional `right_id` | Replace the covered interval; preserve later placement, extending the end if needed |
| `timeline.ripple_delete` | `start`, `duration`, optional `right_id` | Remove a timeline interval and close it |
| `clip.slip` | `clip_id`, `source_in` | Change source content while preserving clip placement/duration |
| `clip.roll` | `left_id`, `left_duration` | Move the boundary between this clip and its next neighbor without changing their combined duration |
| `clip.slide` | `clip_id`, `previous_duration` | Move a middle clip by trimming its two neighbors; keep its content/duration and the total sequence duration |
| `tracks.edit` | `edit`: typed track operation | Create/promote placed tracks, link, place, move/remove selections, target tracks and enforce locks/collision policies; see [TRACKS.md](TRACKS.md) |

The default sequence model is sequential, with explicit gap items. Changing item duration or removing an item shifts all subsequent items. Native placed tracks are an explicit alternative using `tracks.edit`: either create them on an empty timeline or promote the existing sequence into linked video/audio pairs. A project cannot combine the two models. Each operation must leave the candidate snapshot valid; a later operation cannot repair an invalid earlier step. Declare media before appending or placing its clips.

A gap uses `gap: true`, no `asset_id`, zero `source_in` and a positive frame-aligned duration. An omitted asset alone is an error. Existing media clips retain their usual representation and omit `gap`. For example, append one second of black video and exact silence:

```json
{"op":"clip.append","clip":{"id":"pause","gap":true,"source_in":{"num":0,"den":1},"duration":{"num":1,"den":1}}}
```

Gaps support append, trim, split, move, remove, insert, overwrite and ripple deletion, with the same explicit survivor IDs used for media. Splitting or slicing a gap keeps `source_in` at zero in every fragment; adjacent gaps retain their IDs and are not merged implicitly. Gap trims may change duration but require zero source-in. Roll and slide can move boundaries next to gaps while preserving nonempty neighbors and valid media handles; a gap itself cannot be slipped because it has no source interval. A sequence containing only gaps is valid. Gaps do not need files, registry entries or proxies. Frame/range previews, proxy preview dimensions and synchronous/queued exports follow the same gap timing.

Insert and overwrite accept a complete `clip` with an explicit new ID, source range and declared asset. `at` may equal the current end but cannot exceed it. Insert shifts following content by the inserted duration. Overwrite removes only the covered interval, keeps surviving source ranges, and may extend past the old end. Ripple deletion requires a positive duration wholly inside the timeline and can remove the whole sequence; an empty sequence is valid to edit but cannot be rendered by the current backend.

When insert, overwrite or ripple deletion leaves parts of the same original clip on both sides, supply a unique `right_id` for its right fragment. The left fragment retains its original ID. Missing IDs fail with `SPLIT_ID_REQUIRED`; supplying an unused `right_id` fails with `UNUSED_SPLIT_ID`. No implicit ID generation or merging occurs. A surviving fragment on only one side retains its original ID.

For roll, `left_duration` replaces the left clip's duration; the following clip's source-in shifts by the same signed difference and its duration compensates. For slide, `previous_duration` replaces the previous neighbor's duration; the next neighbor compensates in the same way. Roll needs a next neighbor, and slide needs both neighbors. Every surviving clip must stay positive, aligned and inside available source handles. Sequential clips carry their audio and video through the same source interval. These sequential operations do not edit native tracks. Native [transitions](TRANSITIONS.md), targeting, collisions and [linked boundary edits](TRACK_EDITS.md) use `tracks.edit`. The native forms have explicit linked selection, survivor mappings, transition and end policies. Saved `session.preview` and receipts expose every changed source range and placement, with normal retry and undo/history semantics.

`python -X utf8 tests/editing.py --output C:\DEV\CutboltData\new-editing-test` checks all 2,080 frames and 3,993,600 stereo sample frames across 14 rendered edit cases against independent source-index and PCM lists, plus visible diffs, retry, undo, invalid boundaries and atomic failure.

`python -X utf8 tests/timeline_edges.py --output C:\DEV\CutboltData\new-timeline-edges` checks gap/edit interactions and mixed-source boundaries, including first/last-frame trims, a non-keyframe start crossing an H.264 keyframe, 24 and 30000/1001 fps sources, audio-only end samples, gap-only/odd-size output, previews, saved edits and queued renders. Native inputs first use the explicit [media-conform contract](CONFORM.md); timeline source-in/out values then refer to the resulting 25 fps editing assets. This does not enable arbitrary direct native-codec timeline playback or subframe timeline audio edits.

Times use nonnegative exact seconds: `{"num":1,"den":25}` for one frame at 25 fps, or the literals `"1/25"`, `0.04`, `"0.04"` or `3`. Each converts exactly or is rejected; nothing is rounded. JSON numbers are exact up to 15 significant digits, and strings are always exact. Results always use the object form. Frame rates are rationals too. Editing rejects times between the sequence's frame boundaries. Sequential rendering supports [eight native frame rates](NATIVE_TIMING.md), including 24000/1001, 30000/1001 and 60000/1001. Audio-bearing cuts/ranges also require whole 48 kHz samples; 30000/1001 and 60000/1001 therefore use multiples of five video frames. Placed tracks retain 25 fps. Ranges are half-open, and source in/out positions never change source files.

## Asset registry

Project assets support searchable bins, tags and custom metadata, plus content-checked offline-media relinking. `registry.search`, `registry.status`, `registry.bind` and `registry.relink` work through CLI/library and MCP. Bind/relink return proposals that can be committed through the existing `session.apply` contract. See [REGISTRY.md](REGISTRY.md).

## Proxy previews

The [proxy workflow](PROXIES.md) generates identity-bound preview variants of reference assets at half, quarter or eighth dimensions. Attach them with `media.proxy.attach`, then save `preview.proxy` with the desired `scale` (or `null` for full quality). `preview.frame` and `preview.range` follow this setting; final renders always use full-quality assets. `proxy.status` and `proxy.relink` support offline-media recovery. Generation is the blocking `proxy.generate` command.

## Audio mixes

`audio.inspect` validates and evaluates a sample-based PCM mix; `audio.render` writes a new verified WAV. Clips support source cuts, gain, mute, fades and gain automation across multiple tracks. Optional ordered master EQ/compression runs before final clipping; inspection/render receipts include final PCM loudness, sample peaks and RMS. An optional scene `audio_mix` compiles the soundtrack into an ordinary timeline asset. Keep the editable recipe and source WAVs for later changes. See [AUDIO.md](AUDIO.md) for the exact equations, limits and runnable fixture.

## Validated media import and retiming

`media.conform.inspect` validates a source's bound identity, codec/container and exact frame mapping. `media.conform` renders a new reference asset for ordinary saved-session edits. The declared matrix covers bounded FFV1/PCM MKV, tagged BT.709 H.264/AAC MP4/MOV and PCM16 WAV, including fractional/VFR source timestamps. Recipes provide constant rational speed, reverse, freeze and explicit audio conversion/mute policy. Forward speed changes audio pitch; reverse/freeze require mute. Optional `remap` adds [variable speed ramps](REMAPPING.md), previous/nearest/linear frame sampling and an explicit follow-speed or muted pitch policy. Keep the original source and editable recipe. See [CONFORM.md](CONFORM.md) for the complete matrix, limits and runnable fixture. Optional `source.sdr` plus `working_transfer` adds [explicit SDR normalization](COLOR.md), including RGB/YUV full/limited ranges, sRGB/BT.709 transfer conversion and declared handling of missing tags. Conflicting tags fail, and normalized assets carry their actual output color tags.

Optional `decode` selects [local hardware source decoding](ACCELERATION.md), with a specific CUDA device and `unavailable` policy. Omission retains the CPU. Only source-video decoding is accelerated; the full acceptance suite requires an explicit real device through `--decode-device`.

## Pixel scenes and image previews

`graphics.instantiate` creates a validated scene from a [reusable typed template](TEMPLATES.md) and explicit parameter values. `scene.inspect` and `scene.render` ingest bounded PNG layers, text/shapes and separate WAV audio or an audio mix. [Text and shapes](GRAPHICS.md) use explicit external font identities and existing scene animation. Select the optional [Unicode profile](UNICODE_TEXT.md) for shaping, mixed writing directions, grapheme fallback and word wrapping; caption layouts accept the same object as `text_layout`. `preview.frame` exports an exact timeline frame as PNG; `preview.range` exports a selected interval as reference MKV. See [SCENES.md](SCENES.md) for the schema, timing/color/audio rules, limits and runnable examples. Scene compilation is a separate blocking CLI/library operation; its flattened asset enters saved sessions through `media.add`.

## Grading

`reframe.inspect` proposes [subject-directed crop decisions](REFRAMING.md) using authored focus boxes or optional bounded local tracking. Declare an integer source window, matching output aspect ratio, padding, pan limits and subject/cut segments. Inspect each decision, then compile its returned scene to a new asset; the original soundtrack and source identities are preserved.

`tracking.inspect` returns an editable rectangular mask trajectory and replacement scene for a declared local source patch. Supply content-bound scene images, the source-canvas patch, explicit confidence/motion limits and a static mask. Inspect the measurements, then compile the returned `scene` to a new asset. [Tracking and feathering](TRACKING.md) includes a runnable fixture workflow and all bounds. Layer masks also accept static inner/centered/outer feather ramps independently of tracking.

`stabilization.inspect` fits declared background patches to translation or rigid camera motion and returns an editable compensated scene. Supply segment starts, confidence/fit limits, lock or smoothing policy, strength, sampler and explicit crop policy. Compile the returned scene to a new asset. [STABILIZATION.md](STABILIZATION.md) records the exact clocks, subpixel measurement, crop tradeoffs and runnable workflow.

[Primary grading](GRADING.md) is available through each scene layer's `effects` list. It supports exposure, contrast, explicit white balance, master/RGB curves and animated controls, with declared linear-sRGB processing, alpha preservation and clipping. `scene.inspect` reports sampled controls; `scene.render` compiles a graded asset for normal timeline edits. [Selective grades](SELECTIVE_COLOR.md) restrict the correction with HSL color qualifiers and feathered/inverted masks, including animated strength and rectangle controls. No separate service or tool installation is needed.

## Chroma keying and effect presets

[Chroma keying](KEYING.md) joins the scene effect list, with hard/soft color removal, spill suppression, optional screen-color subtraction, feathered masks and animated strength. `effects.preset` returns an independent editable green/blue key recipe through CLI/library/MCP. Copy its effect list into a layer, inspect, then compile; the flattened result uses normal saved sessions.

## Timed captions

For captions, use [the caption workflow](CAPTIONS.md): `captions.import`, `captions.inspect`, `captions.apply`, `captions.encode`, `captions.export` and `captions.scene`. Caption documents retain exact rational cue times, styles and explicit overlap policy. They are immutable snapshots saved by the caller. Sidecar export previews and reports formatting losses before writing a new SRT/WebVTT file; scene conversion samples a bounded caption window using explicit external fonts and layout boxes. The returned scene compiles to an ordinary asset for the saved-session commands below.

## Saved editing sessions

Sessions use one `projects.sqlite3` file in an explicit, existing absolute `store_root`. Choose a local directory outside the source repository. The database can contain multiple projects. Sources are still external references; saving or undoing an edit never copies, edits or deletes media.

| Command | Fields besides `command`, `store_root` | Result |
| --- | --- | --- |
| `session.create` | `project`, or `id`, `width`, `height` and `frame_rate`; `request_id` | Import a valid snapshot, or start an empty project, as revision 0 and return its receipt |
| `session.get` | `project_id`, optional `revision` | Full saved snapshot; omitted revision selects the current head |
| `session.apply` | `project_id`, `request_id`, `expected_revision`, `operations` | Commit one batch and return its receipt |
| `session.preview` | `project_id`, `expected_revision`, `operations` | Semantic before/after diff without saving anything |
| `session.undo` | `project_id`, `request_id`, `expected_revision` | Restore the preceding undo state as a new revision |
| `session.restore` | `project_id`, `request_id`, `expected_revision`, `target_revision` | Restore a chosen saved revision as a new revision |
| `session.history` | `project_id`, optional `before_revision`, optional `limit` | Newest-first summaries; limit defaults to 50, allowed range 1-200 |
| `session.receipt` | `project_id`, `request_id` | The stored receipt of a committed request, or `REQUEST_NOT_FOUND`; read-only, never replays |

Create a saved session from the supplied empty-project example:

```powershell
$store = 'C:\DEV\CutboltData\sessions'
New-Item -ItemType Directory -Path $store -Force | Out-Null
$project = (.\target\debug\cutbolt.exe examples/create-project.json | ConvertFrom-Json).result
@{command='session.create'; store_root=$store; project=$project; request_id='create-demo-001'} |
    ConvertTo-Json -Depth 20 -Compress | .\target\debug\cutbolt.exe
@{command='session.get'; store_root=$store; project_id=$project.id} |
    ConvertTo-Json -Compress | .\target\debug\cutbolt.exe
```

Use the same operation objects listed above for `session.apply`; omit the entire `project` field and instead send its ID and the revision read from `session.get`. A receipt contains `project_id`, `request_id`, `action`, `revision`, `parent_revision`, `restored_from` and `changes`. Fetch that exact receipt revision with `session.get` when a renderer needs a stable snapshot. If a response is lost, `session.receipt` with the same `request_id` reports whether the request committed without sending its operations again. Validation errors name the field path and exact value, for example `track "picture" clip "late" source_in` with the unaligned time and clock. Send the retrieved snapshot to the existing `render.plan` / `render.run` commands.

`changes` includes exact old/new sequence duration, changed clip IDs with before/after placements (index, rational timeline start, and full clip), and added/removed/modified asset IDs. Removing or shortening a clip reports shifted subsequent placements too. When more than 32 clips only moved in time, with unchanged content, track and sequence, they appear once in `shifted` (count, clip IDs, and `earlier_by` or `later_by` when the offset is uniform) instead of individually. `session.preview` previews edit semantics, not video pixels. A preview does not reserve a revision: apply with the same `expected_revision` and handle a conflict if another writer commits first.

### Retry and conflict contract

- Save a unique `request_id` with the planned mutation before sending it. IDs are scoped to a project, limited to 1-128 nonblank bytes, and retained with history.
- On a lost response, send the **same command, request ID, expected revision and arguments** again. A successful retry returns the original receipt even if the project has since advanced or been undone. It does not reapply the edit. Read the latest head separately when needed.
- Reusing a committed ID with different arguments returns `REQUEST_ID_CONFLICT`. JSON key order and whitespace do not affect matching; different rational representations are not promised to match.
- A new request with a stale `expected_revision` returns `REVISION_CONFLICT`. Read the latest project, reconsider the edit, then issue a new request. Do not automatically transplant an edit onto an unfamiliar revision.
- Failed batches commit no revision or receipt. After a known failure, a corrected request may reuse its ID. `STORE_BUSY` means the local lock did not become available within five seconds; it can be retried with the original arguments.
- `session.create` starts a new history at revision 0, even if its imported snapshot has a higher revision. It retains the input snapshot in its request fingerprint. Replaying the exact create returns its original receipt; a different create request cannot replace an existing project.

### Undo and history

Every successful apply, undo or restore increments the head revision. Undo preserves all prior revisions. Repeated undo walks back through applied states; it does not alternate between an edit and its reversal. At the imported state it returns `NOTHING_TO_UNDO`. A new edit after undo retains the older branch in history.

Restore an explicit prior revision to recover a discarded branch or redo a chosen state. Restores are themselves undoable. This version has no implicit redo stack or history deletion/compaction. History includes creation and undo/restore actions; use `next_before_revision` as the next page's `before_revision`, ending when it is null.

### Durability and limits

SQLite commits the snapshot, head/undo pointer and request receipt in one immediate transaction, serializing writers. The store uses DELETE rollback journaling and `synchronous=EXTRA`. Tests exit a child process after snapshot insertion, just before commit, and just after commit: reopening recovers the previous state or the committed state and receipt. Separate CLI-process tests cover concurrent creates, duplicate requests and conflicting edits. These tests cover process termination, not power loss, defective storage hardware or network filesystems.

Use a trusted local filesystem that honors SQLite locking and flushes. Keep the database and any recovery journal together, and never delete a journal to clear a lock. Use `session.backup` for a checked consistent copy during live editing. Network/shared-drive storage is unsupported. Do not manually edit database contents. Snapshot, receipt and revision-metadata checksums detect accidental corruption; they are not tamper authentication or a repair system. Unrelated databases and unknown store versions reject. Version-1 stores remain readable; run `session.migrate` explicitly before new writes. `session.check`, `session.backup` and `session.recover` preserve the checked history contract; see [portable projects](PORTABLE_PROJECTS.md).

Snapshots/history are stored in full, without compaction, encryption, automatic backup or bounded total database size. Session recovery is separate from render jobs. Background jobs retain submission tickets and status, support cancellation, and detect worker interruption. Optional `render.retry.max_attempts` enables bounded retries and [checked publication recovery](RENDER_RECOVERY.md); omission retains one attempt. Source paths are preserved as supplied; automatic relative-path relocation/relinking is future work.

## Camera groups and synchronization

Use `multicam.create` and `multicam.edit` inside `timeline.apply` or saved `session.apply` to retain camera alternatives, cut choices and fixed/follow/mute audio. Each angle references a reusable child sequence with explicit video/audio source offsets and coverage. The group is placed by `sequence_id` like other children. Its managed native tracks must match its camera decisions; locks and dependency checks include unused alternatives. See [MULTICAM.md](MULTICAM.md).

`sync.inspect` takes two bound project assets, an input root, shared output interval, rounding policy and audio-window or declared timecode method. It returns a clock map and inspected conversion recipes. To correct constant drift, explicitly run `media.conform` on those recipes to new files, register the returned assets, and construct the aligned camera group. Inspection is read-only and available over MCP; conversion remains CLI/library-only. See [SYNCHRONIZATION.md](SYNCHRONIZATION.md) for strict limits, confidence and examples.

## Local media and rendering

`media.inspect` takes absolute `path` and `input_root` paths. It resolves the file under that root and returns ffprobe metadata. General probing is broader than rendering support.

`render.plan` and `render.run` accept `project`, `input_root`, `output_root`, and `output` as absolute paths. The output root/parent must already exist. The file must be new and end in `.mkv`. Existing files, including source media, are never overwritten. Inputs outside the declared root are rejected after resolving filesystem links.

The reference profile requires:

- One sequence of 1–64 media or explicit gap items at a supported [native frame rate](NATIVE_TIMING.md), or 25 fps [placed tracks](TRACKS.md) with at most 64 clips. Both render paths require 1–180,000 total frames; the reference source bound is also 180,000 frames.
- Exactly one FFV1/bgr0 video stream and one stereo PCM s16 audio stream in each source.
- Source dimensions matching the sequence, square pixels, zero-origin continuous video timestamps, and no rotation side data.
- 48 kHz audio with matching total duration and continuous timestamps (up to 1 ms Matroska timestamp quantization).

The engine inspects decoded frame timestamps and sample counts rather than trusting declared duration/rate metadata. It trims on exact video-frame and audio-sample boundaries, concatenates, and checks the output counts. It records source/output SHA-256 values and backend versions. Sources are checked for changes across inspection/rendering.

Rendering is synchronous, with a 10-minute encoding timeout. Probing has separate bounds. `render.run` remains a blocking call; use `render.start` on Windows for background work, polled phase/frame progress and cancellation. Interrupted workers use the declared [attempt and recovery policy](RENDER_RECOVERY.md); retries start a fresh encode and codec-stream resume remains unimplemented. Tool output is bounded. This is not an operating-system sandbox or a guarantee of bounded decoder memory.

Output is written to a temporary file in the destination directory, verified, and published with an atomic hard link that refuses to replace an existing filename. The target filesystem must support hard links (the tested NTFS filesystem does). A failed publication returns an error; there is no unsafe overwrite fallback. Normal failure paths remove the temporary file. Forced process termination can leave a `.partial.mkv` file; automatic crash cleanup is future work.

Use `CUTBOLT_FFMPEG` and `CUTBOLT_FFPROBE` to select alternative local tool executables. Dependencies are never downloaded at engine runtime. Only `file` and `pipe` media protocols are allowed when inspecting/reading inputs.

## Range, stream and delivery exports

`export.inspect` and `export.run` accept a reference `project`, explicit input/output roots, unused output path, `profile` and `streams`. Optional `range` selects exact rational start/duration; lossless sequential output uses the supported native clock, while H.264 and placed tracks retain 25 fps. Omitting it selects the whole timeline. `streams` chooses `audio_video`, `video` or `audio`. The `reference` profile produces FFV1/PCM MKV, FFV1-only MKV or PCM WAV. The `h264_aac` profile produces MP4 or audio-only M4A, with explicit `input_transfer: "srgb" | "bt709"` required for video. Optional `h264` and `aac_bitrate` fields select [bounded quality, two-pass bitrate and compatibility settings](DELIVERY_CONTROLS.md); omission preserves the original fixed preset.

Export inspection is read-only and available through MCP. Execution is blocking CLI/library work; the existing background queue continues to render reference projects only. Output validation checks stream structure, exact timestamps and decoded counts; AAC priming and trailing decoder padding are reported separately from exact presentation duration. Source files, existing output files and saved snapshots remain unchanged. See [EXPORT.md](EXPORT.md) for the preset, color equations, format/range limits, quality evidence and runnable example.

## High-bit-depth and HDR conversion

For native 10/12/16-bit sources and PQ/HLG, use the separate [high-bit-depth/HDR path](HDR.md): `hdr.inspect` accepts `recipe` and `input_root`; `hdr.conform` additionally takes `output_root` and an unused `output` path. Inspection is read-only and available over MCP; conversion is CLI/library-only. Choose a tagged RGB16 intermediate or an explicitly tone-mapped RGB8 SDR timeline asset. General timeline processing remains 8-bit SDR.

## Spatial scene transforms

Add `transform.spatial` to a scene layer for animated axis scales, rotation and subpixel translation, with explicit pixel aspect, fitting, nearest/bilinear sampling and crop-edge policy. `scene.inspect` reports sampled values and matrices through the same CLI/MCP schema; `scene.render` compiles a new editing asset. Old recipes without this option keep their integer rendering path. See [SPATIAL.md](SPATIAL.md) for transform order, source anchors, alpha filtering, limits and the runnable fixture.

## LUTs and numerical scopes

`lut.inspect` accepts an identity-bound external `transform`, `input_root` and optional RGB `samples`. `media.conform` can apply that table after explicit SDR normalization. `scopes.inspect` accepts `project`, `input_root`, rational `time`, declared `input_transfer`, `missing_tags` policy and `columns`; it returns exact per-frame histogram, waveform/parade and vectorscope populations from full-quality media. Both inspection commands are read-only MCP tools. See [LUTS_SCOPES.md](LUTS_SCOPES.md) for limits and runnable examples.

## Verification and demo

Verification has three tiers (see [the development-loop plan](DEVELOPMENT_LOOP.md)):

| When | Command | What it does |
| --- | --- | --- |
| While working | `cargo test` and `python tools/verify.py --only conform,overlays` | Builds the engine and runs only the named fixtures; `--last-failed` reruns the previous failures |
| Shipping a commit | `python tools/ship.py` | About two minutes: builds, runs the push gate (below), pushes, then verifies deferred fixtures in the background |
| Full pre-merge check | `python tools/verify.py` | Formatting, lint, Rust tests, then every correctness fixture concurrently in its short mode; wall-clock budgets are recorded, not enforced; no evidence is written |
| Milestones and progress claims | `python tools/verify.py --thorough --decode-device 0` | The evidence run: every budget enforced, long-form cases complete, budget-gated fixtures on a quiet machine, progress documents regenerated |

The quick check skips fixtures whose external runtime (below) or CUDA device is not configured and lists them; `--strict` makes that an error. `--fail-fast` stops starting fixtures after the first failure. Fixtures run against a private copy of the engine, so `target/debug` can be rebuilt while verification runs. Stage times, run records and the impact map are kept in `cutbolt-verify/` inside the repository's common git directory, so every worktree of the checkout shares them.

**Push gate and background verification.** `python tools/impact.py build` runs every fixture against a coverage-instrumented engine (separate `target/coverage` directory) and records which Rust functions each fixture executes (instrumented builds flush coverage at exit through `--cfg cutbolt_coverage`). Rebuild it occasionally, or after large refactors, from a commit with no uncommitted Rust changes; `python tools/impact.py show` lists what current changes affect. `python tools/verify.py --gate` maps the diff since the map's commit to fixtures: changed lines inside recorded functions select the fixtures that executed them, other changes in a Rust file select every fixture that executed code in it, test-helper changes select the fixtures that import them, and build files select everything. Plain `//` comments and `mod`/`use` lines are ignored; `///` doc comments are not, because they feed the agent schemas. Fixtures whose inputs (covered sources, test scripts, build files and tool versions) match a recorded pass are reused. The rest run shortest first within `--budget` seconds (default 100), beside formatting, lint and Rust tests; anything that does not fit is deferred.

`tools/ship.py` requires committed code, runs the gate, pushes, and verifies the deferred fixtures in the background against a detached worktree of the pushed commit, with its own engine copy and below-normal priority. It never locks this checkout or blocks other sessions. Use `--to main` to push a session branch's HEAD to main. `python tools/verify.py --status` shows recent run records, and the next gate warns about any fixture that failed and has not passed since, so it can be fixed forward.

Thorough verification requires explicit external acceptance dependencies: `CUTBOLT_TRANSCRIPTION_RUNTIME` (speech runtime JSON), `CUTBOLT_OTIO_PYTHON` (pinned interchange-reference Python), `CUTBOLT_LEGACY_STORE_ENGINE` (retained original schema-1 store writer), `CUTBOLT_NATIVE_PROJECT_FIXTURE` (private reviewed exact-build capture fixture) and `CUTBOLT_SEGMENTATION_PYTHON` (pinned local foreground worker Python). The corresponding capability documents specify their versions and contracts. Verification never installs or downloads them; missing evidence fails rather than silently skipping a criterion. Use an otherwise idle machine for fixed performance gates.

The thorough run schedules fixtures itself. Correctness fixtures run concurrently (`--jobs`, default half the logical processors), and no new fixture starts while less than 6 GiB of memory is free. Fixtures that assert wall-clock or memory budgets, or report throughput, then run one at a time, with the long-form 4K render in a second lane beside them. The registry edit-latency fixture and the 15-minute real-time recording capture run last on a quiet machine: beside other work the five-second edit budget is marginal, and scheduling delays are correctly rejected as capture discontinuities. Every failing fixture's output tail is printed as soon as it fails, and the run continues to report all failures. Wall-clock budgets live in `tests/budgets.py`; correctness and memory checks are ordinary assertions in every tier.

```powershell
cargo build --locked
python tools/verify.py
python tools/verify.py --thorough --decode-device 0
python tests/integration.py --output C:\DEV\CutboltData\new-demo-directory
```

The demo generator refuses to overwrite existing source fixtures; choose a fresh directory for a retained rerun. It creates three 12-second original video/audio sources with frame identifiers, motion, colored panels, tones, and sync impulses. The script adds, trims, splits, moves and removes clips through the CLI, renders a 30-second result, and independently checks every RGB frame and PCM sample. It writes a project snapshot and verification result beside the output, outside Git.

The successful first retained run is under `C:\DEV\AgentCutData\first-render-20261002`. That historical convenience MP4 did not establish delivery support. The current fixed delivery preset has separate independent acceptance evidence in [EXPORT.md](EXPORT.md).

See [PROGRESS.md](PROGRESS.md) for the current score. The single verification command checks formatting, lints, unit/crash tests, separate-process session tests, MCP protocol/SDK interoperability, queued rendering/cancellation tests, end-to-end rendering, and repository material candidates, then updates code fingerprints and progress evidence. Changing implementation/test code invalidates the previous evidence until verification runs again.

Optional [audio buses and routing](AUDIO_ROUTING.md) extend `audio.inspect` and `audio.render` with explicit channel matrices, pan/balance automation, bus gain/effects and mono through 7.1 PCM WAV layouts. Declare every track and output layout; scenes require an explicitly routed stereo master. Existing stereo recipes remain unchanged.

`audio.repair.inspect` and `audio.repair.render` provide separate [local dialogue cleanup](DIALOGUE_REPAIR.md). Bind a 48 kHz mono/stereo PCM16 source, declare exact output and noise-profile ranges, and optionally remove the reconstructed channel mean or apply ordered EQ/compression. Inspection reports learned noise powers, sample timing, PCM identity and meters. Rendering writes a fresh verified WAV; use its identity in a routed scene soundtrack for saved editing. Fixed-profile noise suppression does not detect speech or adapt to changing backgrounds.

## Local recording

Use `audio.inputs` and `audio.record.inspect` to select and inspect an explicit local source. The blocking CLI/library `audio.record` publishes a new 48 kHz stereo PCM16 WAV; `audio.record.place` returns validated native audio-track operations with an explicit timing correction. Apply through existing saved sessions. See [recording limits, input privacy, timing and examples](RECORDING.md).

## Content cache and contact sheets

Use blocking `cache.run` with an explicit external `cache_root`, byte/entry `policy` and typed `task` for a probe, proxy, frame, interval or sheet. Use `preview.sheet` for an uncached contact sheet. `cache.inspect` reports entries and logical/physical sizes; `cache.prune` evicts only derived entries under an explicit policy. Those two inspection/storage commands also use MCP. Every output is a new file; proxy attachment proposals still go through saved sessions. See [cache requests, limits and failure behavior](CACHE_PREVIEWS.md).

## Editorial interchange

Use `interchange.import` to inspect a local identity-bound OTIO document and propose a new native snapshot from explicit media bindings. Use `interchange.export.inspect` before `interchange.export` to review exact loss IDs. Unacknowledged losses withhold the import snapshot or output file; unknown structure blocks conversion. All three commands also support MCP stdio. Save ready imports explicitly through `session.create`. See [the bounded mapping, requests and independent acceptance](INTERCHANGE.md).

## Portable media and history

`project.portable` verifies a snapshot and proposes relative original/proxy paths under an explicit media root. Apply its `media.paths` operations through saved preview/apply; new renders resolve paths under their supplied root. `session.check` validates complete history, `session.migrate` explicitly upgrades version 1 to 2, `session.backup` publishes a checked consistent history copy and `session.recover` restores it into an unused database location. All five commands support MCP stdio. See [requests, bounds and migration/relocation behavior](PORTABLE_PROJECTS.md).

[Additional lossless exports](EXPORT_FORMATS.md) select `png_mov` for an RGB/PCM movie or `png_sequence` for a complete numbered-image directory with an exact-rate manifest. Both require explicit `input_transfer` and support odd/portrait/DCI-4K dimensions; `sequence_first` applies only to numbered images. Their declared acceptance matrix passes full verification.

`image.sequence.inspect` validates a complete numbered PNG recipe with explicit source window/repeat, rate, alpha association and color interpretation. `image.sequence.compile` produces a transparent FFV1/PNG movie or an explicitly flattened identity-bound native asset. The [numbered-footage contract](IMAGE_SEQUENCES.md) defines exact clocks, silent audio, limits and output preservation.

## Long-form reference rendering

See [the large-raster profile](LONG_FORM.md) for thread/deadline budgets, unchanged sequential limits, complete 30-minute 4K acceptance and cancellation/write-failure gates. No new command or runtime dependency is required.

## Native project transfers

Use `native.import` with reviewed external native-file, transfer and exact-build acceptance identities, explicit media bindings and loss acknowledgements. The installed source application reads its own format during a separate preparation step; the engine returns a proposed snapshot. See [the bounded contract and exact limitations](NATIVE_PROJECTS.md).
