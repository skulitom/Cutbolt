# Cutbolt local engine

The optional [foreground segmentation workflow](SEGMENTATION.md) accepts annotated PNG frames, retains versioned masks and returns ordinary scenes. The native renderer accepts content-bound per-frame binary mattes without the optional worker runtime.

Optional scene [geometry](GEOMETRY.md) provides textured planes with perspective/orthographic cameras, parent transforms and declared lighting through the existing scene inspection/render workflow.

The [property-expression contract](EXPRESSIONS.md) describes optional scene graphs and `expression.inspect`: read-only exact value sampling with typed links, cycle checks and reproducible seeded inputs. Complete media validation and compilation remain `scene.inspect` and `scene.render`.

The executable accepts one JSON request on stdin, or a request-file path as its only argument. `cutbolt capabilities` is a shorthand. It returns a short summary: essentials, commands, operations and the names of the detailed sections. Add `"section"` with one of those names, or `"all"`, for details. It returns one JSON object on stdout: `{"ok":true,"result":...}` on success or `{"ok":false,"error":{"code":"...","message":"..."}}` with exit code 1 on failure. Unknown fields and operations are rejected. Requests are limited to 4 MiB.

Snapshot/media commands: `capabilities`, `schema`, `project.create`, `project.validate`, `timeline.apply`, `timeline.meters`, `files.list`, `media.inspect`, `media.sheet`, `media.shots`, `preview.cuts`, `render.plan`, `render.run`, `export.inspect`, and `export.run`. Saved editing sessions add `session.create`, `session.get`, `session.apply`, `session.preview`, `session.undo`, `session.restore`, `session.history`, and `session.receipt`. Background commands are `render.start`, `job.start` (any long-running command, such as `export.run`, `media.conform` or `scene.render`), `job.status`, `job.wait`, `job.cancel`, and `job.resume`. `cutbolt mcp` provides a local stdio adapter; `cutbolt mcp --tools core` lists a compact catalog of everyday tools plus `cutbolt_run` for the rest. There is no listening socket, installed service, or network request in the engine's normal execution path. See [AGENT_INTERFACE.md](AGENT_INTERFACE.md) for jobs and MCP setup.

`{"command":"schema","name":"scene.render"}` returns a command's argument schema, with a description on every field. Large schemas come back as an outline; `select` returns one variant or definition, and `full` returns everything. It also accepts the shared types `project`, `operation`, `scene`, `template`, `audio_routing` and `transcript`.

`cutbolt --workspace DIR ...`, or the `CUTBOLT_WORKSPACE` environment variable, fixes one directory as the boundary for requests. Omitted roots then default inside it (sessions in `.cutbolt/store`, jobs in `.cutbolt/jobs`, verified source inspections in `.cutbolt/cache/inspections`), relative paths resolve against it, explicit roots must lie inside it, and paths the engine reports come back relative to it. Every command that takes a `project` also accepts `{"project_id": "...", "revision": N}` and loads that saved revision. See [the workspace and reference rules](AGENT_INTERFACE.md#workspace).

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

`graphics.instantiate` creates a validated scene from a [reusable typed template](TEMPLATES.md) and explicit parameter values. `scene.still` renders one frame of a scene to PNG for checking a title, lower third or thumbnail before compiling. `scene.inspect` and `scene.render` ingest bounded PNG layers, text/shapes and separate WAV audio or an audio mix. [Text and shapes](GRAPHICS.md) use explicit external font identities and existing scene animation. Select the optional [Unicode profile](UNICODE_TEXT.md) for shaping, mixed writing directions, grapheme fallback and word wrapping; caption layouts accept the same object as `text_layout`. `preview.frame` exports an exact timeline frame as PNG; `preview.range` exports a selected interval as reference MKV. See [SCENES.md](SCENES.md) for the schema, timing/color/audio rules, limits and runnable examples. Scene compilation is a separate blocking CLI/library operation; its flattened asset enters saved sessions through `media.add`.

## Grading

`reframe.inspect` proposes [subject-directed crop decisions](REFRAMING.md) using authored focus boxes or optional bounded local tracking. Declare an integer source window, matching output aspect ratio, padding, pan limits and subject/cut segments. Inspect each decision, then compile its returned scene to a new asset; the original soundtrack and source identities are preserved.

`tracking.inspect` returns an editable rectangular mask trajectory and replacement scene for a declared local source patch. Supply content-bound scene images, the source-canvas patch, explicit confidence/motion limits and a static mask. Inspect the measurements, then compile the returned `scene` to a new asset. [Tracking and feathering](TRACKING.md) includes a runnable fixture workflow and all bounds. Layer masks also accept static inner/centered/outer feather ramps independently of tracking.

`stabilization.inspect` fits declared background patches to translation or rigid camera motion and returns an editable compensated scene. Supply segment starts, confidence/fit limits, lock or smoothing policy, strength, sampler and explicit crop policy. Compile the returned scene to a new asset. [STABILIZATION.md](STABILIZATION.md) records the exact clocks, subpixel measurement, crop tradeoffs and runnable workflow.

[Primary grading](GRADING.md) is available through each scene layer's `effects` list. It supports exposure, contrast, explicit white balance, master/RGB curves and animated controls, with declared linear-sRGB processing, alpha preservation and clipping. `scene.inspect` reports sampled controls; `scene.render` compiles a graded asset for normal timeline edits. [Selective grades](SELECTIVE_COLOR.md) restrict the correction with HSL color qualifiers and feathered/inverted masks, including animated strength and rectangle controls. No separate service or tool installation is needed.

## Chroma keying and effect presets

[Chroma keying](KEYING.md) joins the scene effect list, with hard/soft color removal, spill suppression, optional screen-color subtraction, feathered masks and animated strength. `effects.preset` returns an independent editable green/blue key recipe through CLI/library/MCP. Copy its effect list into a layer, inspect, then compile; the flattened result uses normal saved sessions.

## Timed captions

For captions, use [the caption workflow](CAPTIONS.md): `captions.draft` (cues from transcripts of the timeline's sources), `captions.render` (a whole track burned into one transparent overlay, through `job.start`), `captions.import`, `captions.inspect`, `captions.apply`, `captions.encode`, `captions.export` and `captions.scene`. Caption documents retain exact rational cue times, styles and explicit overlap policy. They are immutable snapshots saved by the caller. Sidecar export previews and reports formatting losses before writing a new SRT/WebVTT file; scene conversion samples a bounded caption window using explicit external fonts and layout boxes. The returned scene compiles to an ordinary asset for the saved-session commands below.

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

Snapshots/history are stored in full, without compaction, encryption, automatic backup or bounded total database size. Session recovery is separate from render jobs. Background jobs retain submission tickets and status, support cancellation, detect worker interruption, and run independent jobs at once ([background jobs](#background-jobs)). Optional `render.retry.max_attempts` enables bounded retries and [checked publication recovery](RENDER_RECOVERY.md); omission retains one attempt. Source paths are preserved as supplied; automatic relative-path relocation/relinking is future work.

## Camera groups and synchronization

Use `multicam.create` and `multicam.edit` inside `timeline.apply` or saved `session.apply` to retain camera alternatives, cut choices and fixed/follow/mute audio. Each angle references a reusable child sequence with explicit video/audio source offsets and coverage. The group is placed by `sequence_id` like other children. Its managed native tracks must match its camera decisions; locks and dependency checks include unused alternatives. See [MULTICAM.md](MULTICAM.md).

`sync.inspect` takes two bound project assets, an input root, shared output interval, rounding policy and audio-window or declared timecode method. It returns a clock map and inspected conversion recipes. To correct constant drift, explicitly run `media.conform` on those recipes to new files, register the returned assets, and construct the aligned camera group. Inspection is read-only and available over MCP; conversion remains CLI/library-only. See [SYNCHRONIZATION.md](SYNCHRONIZATION.md) for strict limits, confidence and examples.

## Local media and rendering

`files.list` lists files and folders under an absolute `input_root`, sorted by path relative to it, with file sizes. `dir` picks a relative subfolder, `recursive: true` walks every subfolder, `extensions` keeps only matching files (case-insensitive, folders omitted) and `limit` caps entries at 1-1000 (default 200), with `total` and `truncated` reporting the rest. Engine state folders (`.cutbolt`) are skipped and `..` is rejected.

`media.inspect` takes absolute `path` and `input_root` paths. It resolves the file under that root and returns its identity and ffprobe metadata. General probing is broader than rendering support, so the result also carries a `timeline` decision.

- **Ready.** The file already meets the source profile: FFV1 `bgr0` video, or `bgra` for alpha overlay tracks, plus 48 kHz stereo PCM16 audio, at a supported frame rate. `timeline` then holds `ready: true`, the size, `frame_rate`, exact `frames` and rational `duration`, checked with the renderer's own packet-timed source inspection, and an `asset` ready for `media.add`. The project's size and frame rate must match it.
- **Not ready.** `reasons` lists each mismatch. For video, `conform` proposes a whole-source [`media.conform`](CONFORM.md) recipe with an `output` name. It keeps the source's own frame rate when that is a timeline rate, and 25 fps otherwise. The recipe's color interpretation comes from the stream's tags, using `use_declared` when tags are missing. Check it with `media.conform.inspect` and run it with `job.start`.
- **No proposal.** Still images, HDR sources and sources tagged with non-BT.709 color get no recipe. Their reason names the right command instead.

`render.plan` and `render.run` accept `project`, `input_root`, `output_root`, and `output` as absolute paths. The output root/parent must already exist. The file must be new and end in `.mkv`. Existing files, including source media, are never overwritten. Inputs outside the declared root are rejected after resolving filesystem links.

The reference profile requires:

- One sequence of up to 1,000 media or explicit gap items at a supported [native frame rate](NATIVE_TIMING.md), or [placed tracks](TRACKS.md) at the same rates with up to 1,000 clips. Ranges with more than 64 clips render as chunks; see [long timelines](#long-timelines). Both render paths require 1–180,000 total frames; the reference source bound is also 180,000 frames.

### Long timelines

One FFmpeg graph holds at most 64 clips. A full render, range render, preview range, export or meter of a range with more clips is planned as consecutive chunks of at most 64 clips each. The clips counted are those intersecting the window, plus both endpoints of any transition crossing it. Each chunk renders through the ordinary single-graph path into a hidden file beside the output. The chunks are then joined by stream copy, with each chunk's exact length stated to the join.

Every chunk but the last lasts a whole number of frames that is also a whole number of milliseconds, and so of 48 kHz samples: 1 frame at 25 fps, 30 frames at 30000/1001. The joined container clock therefore matches a single render's exactly. The joined output then passes the same full decode verification as any render. `render.plan` reports `chunks` as their frame and sample counts. Progress advances chunk by chunk. A window where more than 64 clips overlap within one alignment step fails with `LIMIT_EXCEEDED`.
- Exactly one FFV1/bgr0 video stream and one stereo PCM s16 audio stream in each source.
- Source dimensions matching the sequence, square pixels, zero-origin continuous video timestamps, and no rotation side data.
- 48 kHz audio with matching total duration and continuous timestamps (up to 1 ms Matroska timestamp quantization).

The engine inspects decoded frame timestamps and sample counts rather than trusting declared duration/rate metadata. It trims on exact video-frame and audio-sample boundaries, concatenates, and checks the output counts. It records source/output SHA-256 values and backend versions. Sources are checked for changes across inspection/rendering.

A passed source inspection is remembered by content identity: the source's SHA-256 and size, the inspection's parameters (size, frame rate, alpha, decoded or packet timing), the ffprobe executable's SHA-256 and the engine build. Every command still hashes each source it reads, so changed bytes never reuse an entry, but identical bytes skip the frame-by-frame decode (about 23 s for an 80 s 1080p source). Entries live in the process, and in `.cutbolt/cache/inspections` when a workspace is open, or in the absolute directory named by `CUTBOLT_INSPECTION_CACHE`. Renders, scene compiles and caption overlays record their verified outputs the same way, so a fresh asset is not decoded again when a timeline first uses it. Each entry checks itself; a damaged one is ignored and rebuilt, and deleting the folder only costs time.

Rendering is synchronous, with a 10-minute encoding timeout. Probing has separate bounds. `render.run` remains a blocking call; use `render.start` on Windows for background work, polled phase/frame progress and cancellation. Interrupted workers use the declared [attempt and recovery policy](RENDER_RECOVERY.md); retries start a fresh encode and codec-stream resume remains unimplemented. Tool output is bounded. This is not an operating-system sandbox or a guarantee of bounded decoder memory.

Output is written to a temporary file in the destination directory, verified, and published with an atomic hard link that refuses to replace an existing filename. The target filesystem must support hard links (the tested NTFS filesystem does). A failed publication returns an error; there is no unsafe overwrite fallback. Normal failure paths remove the temporary file. Forced process termination can leave a `.partial.mkv` file; automatic crash cleanup is future work.

Use `CUTBOLT_FFMPEG` and `CUTBOLT_FFPROBE` to select alternative local tool executables. Dependencies are never downloaded at engine runtime. Only `file` and `pipe` media protocols are allowed when inspecting/reading inputs.

### Background jobs

`job.start` queues a long-running command and `render.start` a reference render. Both return a ticket at once, and `job.wait` returns when the job finishes. One hidden worker per job root runs independent jobs at the same time, so queue every scene render or prepare before waiting on any:

- **Pool size.** At most `CUTBOLT_JOB_WORKERS` jobs run at once, 1 to 32. The default is a quarter of the logical processors, from 1 to 8: eight on a 32-thread machine, where one 1080p scene render keeps about three cores busy. The worker reads the setting when it starts, from the environment of the CLI or MCP server that queued the job; values out of range are ignored.
- **Heavy jobs.** `export.run`, `export.review`, `captions.render`, `media.prepare`, `media.conform`, `hdr.conform`, `proxy.generate`, `image.sequence.compile`, `preview.range`, `cache.run` and `render.start` already run multi-threaded FFmpeg or Rust compositing. Each holds half the pool, rounded up, so at most two run at once. Light jobs (`scene.render`, `audio.render`, `audio.repair.render` and the speech commands) hold one place each.
- **Speech.** `media.transcribe`, `transcript.transcribe`, and `export.review` with a `runtime`, load speech models, so they run one at a time.
- **Order.** Jobs start in submission order as room allows. A job waiting for room holds back the jobs behind it, so a stream of light jobs never starves a heavy one.
- **Shared outputs.** Jobs that write the same path run one at a time, in submission order, while independent jobs behind them still start. That covers the same file in another spelling or letter case, a path inside another job's output folder, and the same cache database. A `media.prepare` without `output` claims the `<id>-prepared.mkv` names it would write. Queued commands read saved projects as snapshots pinned at submission and never write a session, so session edits neither wait for jobs nor race with them.
- **Latency.** The worker and `job.wait` wake on named events instead of polling. A job starts as soon as there is room, and `job.wait` returns within milliseconds of the job finishing. Each process hashes FFmpeg and ffprobe once, not once per submission.

Replaying a `request_id` returns the original ticket, and `job.wait` on it returns the original result. If the worker dies, every job it was running is interrupted; `job.status`, `job.resume` or the next worker reconcile each of them, and opted-in retries requeue. Cancellation, the stall watchdog and the 12-hour deadline apply to each job separately. See [the job contract](AGENT_INTERFACE.md#local-background-renders).

## Preparing camera and phone files

`media.prepare` takes any video file FFmpeg decodes, such as a phone or camera MP4, plus an optional target `project`, and returns a timeline asset in one step. It is queued with `job.start`, like other long-running commands.

- A file that is already a timeline source and fits the project's frame rate and size, or any ready file when no project is given, comes back unchanged as `{"converted": false, "asset": ...}`.
- Anything else is converted with the [`media.inspect` readiness recipe](#local-media-and-rendering) through [`media.conform`](CONFORM.md). With a project, it uses the project's frame rate and size. Without one, it keeps the source's own size and rate when that is a timeline rate, and uses 25 fps otherwise. The output defaults to `<name>-prepared.mkv` in `output_root`.
- The result holds the `asset` for `media.add`, the `recipe` it ran, `frames` and `frame_rate`.
- **Transcripts.** Optional `transcripts` of the files being prepared come back moved onto their assets, as the next revision of each document, so the prepared asset needs no recognition of its own. With `paths`, each file takes the documents of its own source, and the result lists them all.
- With `paths` instead of `path`, one job prepares up to 200 files, absolute or relative to `input_root`.
  - **IDs and outputs.** Asset IDs come from the file names, made unique within the batch and against the project's assets (`clip`, `clip-2`, and so on). A conversion is written to `<id>-prepared.mkv`.
  - **Failures.** A file that fails is listed with its error code and message, and the others continue.
  - **Result.** It lists each file's outcome and returns the prepared assets as `media.add` `operations` for one `session.apply`.

A 30 fps phone clip prepared for a 30 fps project keeps every frame. It then renders and delivers H.264 at 30 fps. A PCM16 WAV voice-over or music track, mono or stereo at 24, 44.1 or 48 kHz, needs the `project`: it becomes an asset with a silent black picture of the project's size and rate, ready for an audio track, its length the audio's whole frames. `media.inspect` proposes the same recipe at 1920x1080 and 25 fps. Stills, other audio-only formats, HDR sources and sources tagged with non-BT.709 color are refused with `UNSUPPORTED_MEDIA` and the reason. Use scenes for stills and `hdr.conform` for HDR. Stretching to a different aspect ratio is not avoided; give a project of the source's aspect, or convert explicitly with `media.conform` for other shapes.

## Ducking music under speech

`audio.duck` is read-only. It renders the `voice_track_id` audio track alone over the timeline and finds speech in 10 ms windows. A window counts as speech when its mean power reaches `threshold_db` (default -45 dBFS), and pauses shorter than `bridge` (default 1 s) stay ducked.

It then proposes a gain curve for every clip on `music_track_id` that overlaps speech. The curve ramps linearly down to `duck_milli` of the clip's `gain_milli` (default 250, about -12 dB) over `attack` (default 0.25 s) before speech starts. It holds there, then ramps back over `release` (default 0.6 s) after speech ends. Ramps that meet merge into one span. Keys are placed on the clip's source clock and may lie just before its start, so a ramp completes as speech begins.

The result lists the speech runs with exact times, a summary per clip, and `clip_audio` `operations`. Check them with `session.preview`, apply them with `session.apply`, and listen back with `timeline.meters` `curve` or a preview range.

## Paper edits from transcripts

`transcript.assemble` turns transcript word selections into frame-aligned `clip.append` operations, so a rough cut can be built from what people say. See [TRANSCRIPTS.md](TRANSCRIPTS.md#paper-edits).

## Removing filler words

`transcript.fillers` proposes ripple deletions of um, uh and other listed words wherever the timeline speaks them, from transcripts of its sources, without cutting into the neighbouring words. See [TRANSCRIPTS.md](TRANSCRIPTS.md#removing-filler-words).

`start` and `end` limit the proposal to fillers wholly inside that timeline window. A ripple deletion cuts every track, music and picture included. With `lift: true` and the `track_ids` to silence (such as the voice track), each filler is instead lifted: removed from those tracks with an empty `overwrite`, leaving silence. The explicit end and every other placement stay as they were, so a cut timed to music or picture keeps its timing. The result reports `silenced` instead of `removed`. Lifting a clip linked to picture is refused as a synchronization conflict and listed with the reason; unlink it first if that is intended.

## Finding the beat

`audio.beats` is read-only and analyses a music file for cutting to it. The file can be any format FFmpeg decodes, up to an hour (`start` and `duration` select a range). It reports the onsets, the tempo and a beat grid as exact file times. With `frame_rate`, it also gives each listed beat's nearest frame start.

The analysis works on 10 ms hops of the first audio stream, mixed to mono and resampled to 48 kHz:
- **Onsets.** An onset is a rise in level of at least 6 dB from the previous hop that is also the largest rise within 50 ms on either side. A rise counts at most 3 dB more than the rise from two hops back. So a level recovering from a one-hop dip is not an onset; such dips come from a low note, such as a kick's tail, beating against the 10 ms window.
- **Period.** The autocorrelation of onset strength over the tempo range (`min_bpm` 60 to `max_bpm` 200 by default) gives a first period. Strength is the rise weighted by the hop's amplitude, so loud hits count more than quiet ones such as hi-hats. Each hop pairs with the strongest of the three hops around the lag, so a period between whole hops still scores as one lag.
- **Phase.** Beats at that period are tracked through the whole range by dynamic programming: the chain of hops that collects the most onset strength while its intervals stay near the period. Where tracking moves to another position in the bar, it starts a new run. The tracked beats on onsets fit one exact period by least squares, with a phase per run, and the run with the most strength sets the phase. A small period error therefore cannot drift the grid across a long file.
- **Tempo.** Music repeats at several metrical levels at once: bars, half notes, beats and eighths. The analysis lists the levels within the tempo range that the onsets support:
  - A faster level divides each period in two or three, where onsets fall at least half as often as on the period itself.
  - A slower level groups two or three periods when one position is accented, with 1.5 times the others' mean strength.

  `tempo_bpm` is the level nearest 120 BPM. `tempo_alternatives` lists the others, each with its tempo `ratio` to the beat. A slower level also gives `first_beat`, the index of the beat on its accented position, such as a backbeat's kick.
- **Grid.** Each grid beat moves to an onset within 30 ms when there is one, and `on_onsets` counts those beats. A least-squares line through them then refines the period and places the grid again.

Beats on onsets are accurate to one hop. Fewer than four onsets give no tempo. Choosing between levels is a convention, not a measurement: the reported tempo is between 85 and 170 BPM whenever the music has a level there. A ballad felt at 70 BPM with steady eighths therefore reads as 140, and fast music near 175 BPM reads at half. The other reading is in `tempo_alternatives`. To choose a level, narrow `min_bpm` and `max_bpm` to it, and the beats follow.

## Tightening pauses (jump cuts)

`audio.tighten` is read-only. It finds the pauses in speech and proposes ripple deletions that shorten them, the jump cuts of talking-head video.

Speech is detected as for `audio.duck`:
- on placed tracks, in the `voice_track_id` audio track played alone;
- on a sequential timeline, in the whole program.

A 10 ms window is speech when its mean power reaches `threshold_db` (default -45 dBFS). A silence of at least `min_pause` (default 0.75 s) between speech is a pause. With `edges` (default true), silence before the first and after the last speech counts too.

Each cut removes the middle of a pause and keeps `keep` (default 0.2 s) of silence next to the speech; twice `keep` must be shorter than `min_pause`. Cut ends are rounded inward to whole frames. On placed tracks they must also be whole 48 kHz samples, which at 29.97 fps means multiples of five frames.

On placed tracks every cut ripples every track, with linked partners, so picture, voice and anything else stay in sync. Clips the cut splits get new right-hand IDs `<id>-j<n>`, and so do links whose members are all split. On a sequential timeline the cut is a `timeline.ripple_delete`.

The cuts are ordered from the latest back, so each start is an original timeline time. Each is applied to a working copy first. A cut the editor refuses is listed with its reason and left out, so the returned batch always applies. Refusals include a cut inside a fade or a transition, or one that would split a link unevenly.

The result lists each pause, its cut or the reason it was skipped, the time removed, and the durations before and after. Ripple deletions also cut music on other tracks, so tighten the speech before laying a music bed, or accept the jumps. `start` and `end` limit the cuts to a timeline window, such as one take or one scene. Pauses outside it are not listed, and a pause crossing an edge is cut only inside the window.

## Normalizing loudness

`audio.normalize` is read-only. It brings the mix of enabled audio tracks to `target_lkfs` (default -14 LKFS, common for online video) by scaling every audio-track clip's level by one factor. This includes clips on disabled tracks, so the balance between tracks is kept. A clip's `gain_milli`, or every key of its `gain_curve`, is multiplied and rounded to the nearest milli-unit; fades are unchanged. Audio tracks with clips must be unlocked, and a sequential timeline must be promoted to tracks first.

The factor is the smallest of three values:
- the one that reaches the target;
- the one that puts the higher channel's sample peak on `peak_ceiling_dbfs` (default -1 dBFS);
- the one that lifts the highest clip level to the 4,000 maximum (+12 dB).

`limited_by` names the limit that applied, or is null. The engine measures the mix at the proposed levels and refines the factor from that measurement, up to three times, until the levels stop changing. A mix that already clips hides its true peak until it is lowered, which is why the refinement exists.

**Through a limiter.** Speech often peaks far above its loudness; synthesized narration can peak at -2 dBFS while measuring -21 LKFS. When the ceiling is what stops the factor and `limiter` is true (the default), the proposal also sets a master [timeline limiter](AUDIO.md#timeline-limiters) at `peak_ceiling_dbfs`. It keeps the lookahead and release of an existing master limiter, or uses 5 ms and 150 ms. The factor then keeps rising through the limiter toward the target:
- Loudness grows more slowly than the gain once the limiter works, so the engine measures up to six more times and steps by the measured slope.
- When a measured true peak passes the ceiling, the limiter's own ceiling is lowered by the excess, to hundredths of a dB.
- The gain stops at the clip gain range, or where the limiter's deepest gain reduction would pass `max_limiting_db` (default 12 dB); `limited_by` is then `"limiter_reduction"`.

`limiter` in the proposal holds the proposed settings, and its `audio_dynamics` operation comes first. With `limiter: false` the gain stops at the ceiling, as before.

`result` is the measured outcome of the operations, not a prediction: integrated loudness, sample peak, `true_peak_dbtp`, the number of measured passes and, with a limiter, its gain reduction under `dynamics` (`max_reduction_db`, `reduced_seconds`, `reduced_fraction`). `measured` gives the same before any change. Clips that share a level share one `clip_audio` operation. Apply the operations with `session.apply`.

## Reviewing edits and footage

These commands let an agent check its own work from text, still images and numbers.

- **`timeline.check`** lists likely mistakes without rendering, errors first, then warnings, then notes, each with times and clip IDs. `ok` is true when there are no warnings or errors.
  - **Errors.** Missing or changed media, with `input_root`; changed means a bound identity no longer matches.
  - **Warnings.**
    - Black stretches with no opaque video clip.
    - Flash frames: picture clips shorter than `min_clip_frames` (default 3).
    - Picture and sound of one source that play together, unlinked, at different source times. It reports which is late and by how much.
    - Words cut by clip edges, with `transcripts`.
    - Transcripts were given but none matches an asset, so no words were checked.
  - **Notes.**
    - Clips on disabled tracks.
    - Stretches with no audio clip.
    - Jump cuts between touching clips of one source that skip or repeat less than `jump_window` (default 10 s); deliberate in jump-cut editing.
    - Unused assets.
    - A given transcript that matches no asset while others do.
- **`timeline.outline`** reads a timeline as compact text without rendering anything:
  - **Header.** It gives the canvas, frame rate, duration and assets.
  - **Clip lines.** Each track lists one line per clip, in time order: timeline span, clip ID, asset (or `seq:` and a child sequence), source span, link, and any gain, fades or picture-in-picture transform. A transition gets its own bracketed line after its outgoing clip.
  - **Gap lines.** Two closing lines list the stretches with no opaque video clip (black) and with no audio clip.
  - **Times.** Times are in seconds, exact to the millisecond or marked `~` when rounded. Exact values stay in the project JSON.
  - **Transcripts.** Give `transcripts` (documents from `transcript.transcribe` or `transcript.correct`) and each audio clip shows the words inside its source span. With at most `words` words (default 12; 0 shows only the count) the text is complete; otherwise it shows the first and last words and the count. A word cut by the clip's edge is marked `*`, so a cut in the middle of a word stands out. A clip whose span the transcripts do not cover says so.
  - **Matching.** A transcript matches the assets of its source by content identity, or by path when the asset is unbound; absolute asset paths need `input_root`. Unmatched transcripts are named with the reason, and two transcripts of one asset may not overlap.
  - **Clips with words.** On a sequential timeline every clip carries its own audio, so every clip gets words; on placed tracks only audio-track clips do.
  - **Paging and sequences.** `start` and `end` page through long timelines; `sequence_id` outlines a child sequence.
  - **Over MCP.** The tool's text content is the outline itself.
- **`preview.cuts`** pages through every cut of a project, 16 at a time. A cut is a change of the visible clip. Each page is a sheet of the last frame before and the first frame after each cut, two cuts per row, written to a new PNG. It is identical to `preview.sheet` at those times, with cells `tile_width` wide (default 192) and the project's aspect. The result lists each cut's `time` and the `before` and `after` clips with their source times. It marks a `continuous` split of one source as an edit point that is not a visible cut, gives `total_cuts`, and gives `next` to pass as `start` for the following page.
- **`media.sheet`** reads any file FFmpeg decodes, not only timeline sources. It writes a sheet of frames at the given `times`, or `count` frames (default 16) spread through the file at the middles of equal parts. Use it to look at footage before adding it.
- **`media.shots`** logs footage. Frames are shrunk to 64x36 gray and compared with the previous frame by mean absolute difference (0-255). A frame starts a new shot when its difference reaches `threshold` (default 20) and is at least twice the difference of each of its two neighbors on either side. Motion and single-frame flashes therefore do not count; gradual dissolves are not detected. Shots shorter than `minimum_frames` (default 6) are merged. The result lists each shot's `start`, `end`, frames and `cut_score`. With `output`, it also writes a sheet of each shot's middle frame for the first 64 shots.
- **`timeline.meters` with `curve: true`** adds `over_time` to each meter. For every whole second it gives the short-term loudness of the 3 s ending there and the loudest momentary (400 ms) loudness ending inside it. Both use the same K-weighting as integrated loudness, are rounded to 0.1 LKFS, and are null below the -70 LKFS gate. It also lists silent runs (every channel within 32 codes of zero for at least 0.5 s) and clipped runs (full-scale codes), with exact 48 kHz sample times and exact counts; the first 50 runs of each kind are listed. Use it to find dead air, music that buries speech, or distortion.

Over MCP, the sheets come back as inline images.

## Reviewing a delivered cut

`export.review` checks a rendered file, normally the output of `export.run` or `render.start`. Queue it with `job.start`. It writes a new folder at `output` inside `output_root` and never overwrites one. A review that fails removes its folder.

- **Picture.** `sheet.png` holds `frames` frames (default 16) spread evenly through the file, identical to `media.sheet`. Black runs come from FFmpeg's `blackdetect` with its default thresholds (pixels at most 10% luma over 98% of the picture) and are snapped to frame boundaries. `preview.mp4` is a small H.264/AAC copy for a person to watch, `rendition_height` pixels tall (default 360, never above the source; 0 skips it).
- **Sound.** The first audio stream is decoded to 48 kHz stereo PCM16 and measured as it streams, so length is not limited. It gets the same measurements as `timeline.meters`: integrated loudness, sample peaks, short-term loudness per second, and silent and clipped runs.
- **Timing.** Given the `project` the file was rendered from, video frames and frame rate must match exactly. Audio may differ by at most 2,048 samples (two AAC blocks of decoder padding).
- **Expected speech.** With the `project` and `transcripts` of its sources (as for `timeline.outline`), the review lists what the cut should say. These are the whole words inside audio clips on enabled tracks, moved to timeline time; muted clips and child sequences are skipped. Words a clip edge cuts through are listed separately as `cut_words`.
- **Heard speech.** What the cut actually says comes from one of two sources:
  - `heard`: transcripts of the reviewed file itself, whose source identity must be the file's.
  - `runtime` and `language`: the local speech runtime transcribes the cut's audio in 120 s windows that overlap by 5 s. `audio.wav` and `transcripts.json` stay in the folder, so the documents remain bound to an existing file. Sound the recognizer writes down that is not speech, such as `[Music]` under a narration, is not compared; it is listed in `speech.recognition.non_speech` and on a `heard besides speech` summary line.

  A missing model or an invalid runtime is rejected before any decoding. If recognition itself fails, the review still completes with its picture, sound and timing sections. The speech section then reports `recognition.ok: false` with the error, and the summary says why the words were not compared.

  Overlapping transcripts split their overlap at its middle.
- **Comparison.** The two lists are matched in time order. A heard word matches the next expected word when both have the same letters and digits, ignoring case and punctuation, and their middles are at most `tolerance` apart (default 0.5 s). Unmatched words between two matches form one difference, reported as missing, extra or changed words with their times. The result gives the match ratio and the first 50 differences.
- **Result.** `summary` is a short text report, and the JSON gives each section. `review.json` in the folder holds everything, including per-second loudness and full meters. Over MCP, `job.wait` shows the sheet as an inline image once the job finishes.

## Range, stream and delivery exports

`export.inspect` and `export.run` accept a reference `project`, explicit input/output roots, unused output path, `profile` and `streams`. Optional `range` selects exact rational start/duration; every profile uses the timeline's native clock, for sequential timelines and placed tracks. Omitting it selects the whole timeline. `streams` chooses `audio_video`, `video` or `audio`. The `reference` profile produces FFV1/PCM MKV, FFV1-only MKV or PCM WAV. The `h264_aac` profile produces MP4 or audio-only M4A, with `input_transfer: "srgb" | "bt709"` required for video unless the project declares its `transfer` once with the `project.transfer` operation. Optional `h264` and `aac_bitrate` fields select [bounded quality, two-pass bitrate and compatibility settings](DELIVERY_CONTROLS.md); omission preserves the original fixed preset.

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

[Additional lossless exports](EXPORT_FORMATS.md) select `png_mov` for an RGB/PCM movie or `png_sequence` for a complete numbered-image directory with an exact-rate manifest. Both need `input_transfer` or a declared project `transfer`, and support odd/portrait/DCI-4K dimensions; `sequence_first` applies only to numbered images. Their declared acceptance matrix passes full verification.

`image.sequence.inspect` validates a complete numbered PNG recipe with explicit source window/repeat, rate, alpha association and color interpretation. `image.sequence.compile` produces a transparent FFV1/PNG movie or an explicitly flattened identity-bound native asset. The [numbered-footage contract](IMAGE_SEQUENCES.md) defines exact clocks, silent audio, limits and output preservation.

## Long-form reference rendering

See [the large-raster profile](LONG_FORM.md) for thread/deadline budgets, unchanged sequential limits, complete 30-minute 4K acceptance and cancellation/write-failure gates. No new command or runtime dependency is required.

## Native project transfers

Use `native.import` with reviewed external native-file, transfer and exact-build acceptance identities, explicit media bindings and loss acknowledgements. The installed source application reads its own format during a separate preparation step; the engine returns a proposed snapshot. See [the bounded contract and exact limitations](NATIVE_PROJECTS.md).
