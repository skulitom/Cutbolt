# Native video and audio tracks

The `native-tracks-v1` model stores explicitly placed clips in the existing project snapshot and local session history. It supports opaque and straight-alpha overlay video tracks, stereo audio tracks, linked selections, track targeting, locks, enabled state and explicit collision policies. Use the existing `timeline.apply` or `session.preview`/`session.apply` commands with `tracks.edit` operations. The MCP tools expose the same schema; no separate service or database is introduced.

## Model and clocks

A project uses either its original sequential `clips` list or an optional `tracks` arrangement. An arrangement requires an explicit rational `duration`, an ordered `tracks` array and `links`. When present, the sequential `clips` list must be empty. Existing snapshots and saved receipts remain readable.

Each track has a unique `id`, `kind` (`video` or `audio`), explicit `locked` and `enabled` booleans, a `clips` array and optional [transitions](TRANSITIONS.md). Each clip has an `id` unique across its arrangement's tracks, exactly one source reference (`asset_id` or [nested `sequence_id`](SEQUENCES.md)), `start`, `source_in` and positive `duration`. Time fields use exact nonnegative `{num,den}` seconds. Video placements and source ranges align to the project frame rate; audio aligns to 48,000 samples per second. Arrangement duration aligns to the project frame rate. Declared source ranges and clip ends must fit the source and arrangement duration. Intervals include their start and exclude their end, so touching clips are legal. Clips may overlap across tracks but never within one track. Array order within a track does not control playback.

Video tracks are ordered bottom to top; the last enabled track with a clip at the requested time supplies the entire opaque frame. An upper gap exposes the lower video. No video produces black.

A video track may declare `"composite": "alpha_over"` (the default, omitted from snapshots, is `"opaque"`). Opaque tracks choose the base frame as above. Enabled `alpha_over` tracks above that base are then composited bottom to top with straight-alpha "over" in encoded RGB: each channel is `floor((2*(s*a + d*(255-a)) + 255) / 510)`, the exact nearest integer with no ties. Overlay sources are 25 fps FFV1 `bgra` (straight alpha, for example from a [transparent scene](SCENES.md#transparent-output) or [`image.sequence.compile`](IMAGE_SEQUENCES.md) with an alpha profile) or opaque `bgr0`, read as alpha 255. Gaps on an overlay track are transparent. An overlay clip covers the whole frame unless it has a [picture-in-picture transform](#picture-in-picture). An asset cannot be both an overlay and an opaque source in one render, and an alpha source on an opaque track fails with `UNSUPPORTED_MEDIA`. Overlay tracks must be video, without transitions or nested sequences; proxy previews reject them, so use full-quality previews. Frame/range previews, scopes, reference renders, queued renders and exports include overlays. Receipts list `overlay_track_ids` for an overlaid preview frame. Enabled audio tracks sum stereo PCM, after [clip gain and fades](#clip-gain-and-fades), with one final signed-16-bit saturation; gaps produce exact silence. Enabled state affects playback, while locks protect edits. The arrangement's explicit duration includes leading and trailing gaps. Removing clips leaves their space and keeps that duration.

The editable model permits up to 32 tracks, 1,000 clips and 500 links. The current renderer and previews support 25 fps reference assets and at most 8 million pixels. A window with more than 64 clips renders as exactly joined chunks ([long timelines](USAGE.md#long-timelines)). A full render supports 1–180,000 frames. Sources require matching dimensions, FFV1 RGB8 and 48 kHz stereo PCM16; use the existing media conversion path for other inputs. An empty arrangement with positive duration renders black and silence. Zero duration is editable but cannot be rendered. Unsupported rates, sources and resource limits fail explicitly.

## Editing operations

Wrap every edit as `{"op":"tracks.edit","edit":{...}}`. Each operation must leave a valid candidate; a later operation cannot repair an invalid intermediate state. The entire batch is atomic, including collateral changes from linked selections and replacement.

| Edit `op` | Additional fields | Behavior |
| --- | --- | --- |
| `create` | `duration` | Create an empty arrangement from an empty sequential timeline |
| `promote` | `video_track_id`, `audio_track_id` | Convert a sequential timeline into linked video/audio pairs with identical playback and implicit gaps |
| `add` | `track` | Add an empty track with explicit kind, enabled and locked state |
| `state` | `track_id`, `locked`, `enabled` | Change track state; unlock before changing a locked track's enabled state |
| `order` | `track_ids` | Supply all track IDs once in the new bottom-to-top order; locked tracks cannot change index |
| `duration` | `duration` | Change the explicit end without truncating clips; an end before any clip is rejected |
| `place` | `track_id`, `clip`, `collision` | Place a new uniquely identified clip |
| `move` | `clip_ids`, `shift`, `targets`, `links`, `collision` | Move a selection by one common signed offset, optionally to tracks of the same kind |
| `remove` | `clip_ids`, `links` | Remove the selection and leave gaps |
| `link` | `id`, `clip_ids` | Record the current content synchronization of 2–32 selected clips |
| `unlink` | `id` | Explicitly remove a link before independent edits |
| `transition_set` | `track_id`, `transition` | Add/replace an editable transition with explicit adjacent endpoint clips and source handles |
| `transition_remove` | `track_id`, `id` | Remove a transition; its underlying cut remains |
| `split`, `slip`, `roll`, `slide` | See [boundary edit fields](TRACK_EDITS.md) | Split linked clips or move content/boundaries while retaining valid source handles and effects |
| `insert`, `overwrite`, `ripple_delete` | See [interval fields and policies](TRACK_EDITS.md#interval-operations) | Edit intervals across selected/linked tracks with explicit survivor IDs, end policy and transition policy |
| `clip_audio` | `clip_ids`, optional `gain_milli`, `fade_in`, `fade_out` | Set the [gain and fades](#clip-gain-and-fades) of audio-track clips; omitted fields keep their values |
| `clip_transform` | `clip_ids`, `transform` | Set the [picture-in-picture transform](#picture-in-picture) of `alpha_over` track clips; null restores the full frame |

`shift` requires `backward` and nonnegative rational `amount`; zero supports retargeting alone. `targets` is an explicit array of `{clip_id,track_id}`. Omitted members keep their existing track; included linked partners may have explicit targets too. All placements are calculated together, so simultaneous swaps are evaluated against the final placement. A video clip cannot be retargeted to an audio track or vice versa.

`links: "include"` expands the selection to every linked partner. `"reject_partial"` requires callers to select the entire group themselves. Each clip belongs to at most one group. Creation records each member's current start/source-in anchor, including pre-existing offsets or different sources. Validation requires every member to preserve the same change in `start - source_in` relative to its own anchor. A forged snapshot that shifts only one linked member fails with `SYNC_CONFLICT`. Saved anchors persist across moves. Explicit unlinking is required before intentionally independent movement.

`collision: "reject"` rejects overlap with any existing clip on a destination track. `"replace_clips"` removes each whole colliding clip and its entire linked group, even when a partner is on another track. This policy never keeps a surviving interval of a colliding clip. Overlapping selected placements always fail. Locks on every source, target and collateral linked-partner track are checked before a batch can commit. Inspect `session.preview` to see every affected clip before applying replacement.

Promotion preserves original video clip IDs and source timing. Audio IDs use unique `promoted-audio-N` names, skipping collisions with original IDs; links use `promoted-link-N`. Sequential gap IDs disappear because space is represented by placement and explicit duration. Conversion is explicit, undoable in a session, and never changes source files. Legacy `clip.*`/ripple operations reject track projects; use track operations after promotion.

## Example

Add a bound reference asset with the usual `media.add` operation first. This batch creates one-second linked picture and sound tracks:

```json
[
  {"op":"tracks.edit","edit":{"op":"create","duration":{"num":1,"den":1}}},
  {"op":"tracks.edit","edit":{"op":"add","track":{"id":"picture","kind":"video","locked":false,"enabled":true,"clips":[]}}},
  {"op":"tracks.edit","edit":{"op":"add","track":{"id":"sound","kind":"audio","locked":false,"enabled":true,"clips":[]}}},
  {"op":"tracks.edit","edit":{"op":"place","track_id":"picture","clip":{"id":"shot","asset_id":"source","start":{"num":0,"den":1},"source_in":{"num":0,"den":1},"duration":{"num":1,"den":1}},"collision":"reject"}},
  {"op":"tracks.edit","edit":{"op":"place","track_id":"sound","clip":{"id":"soundtrack","asset_id":"source","start":{"num":0,"den":1},"source_in":{"num":0,"den":1},"duration":{"num":1,"den":1}},"collision":"reject"}},
  {"op":"tracks.edit","edit":{"op":"link","id":"shot-sync","clip_ids":["shot","soundtrack"]}}
]
```

To leave room and move both members five frames later, extend the end before moving:

```json
[
  {"op":"tracks.edit","edit":{"op":"duration","duration":{"num":6,"den":5}}},
  {"op":"tracks.edit","edit":{"op":"move","clip_ids":["shot"],"shift":{"backward":false,"amount":{"num":1,"den":5}},"targets":[],"links":"include","collision":"reject"}}
]
```

## Picture-in-picture

A clip on an `alpha_over` track may carry a `transform`, also accepted in a `place` clip. Its steps apply in this order:

1. `crop` keeps the source region `[x, y, width, height]`. It must lie inside the frame, and its sizes must be multiples of `divisor`. By default it is the whole frame.
2. `divisor` (1-8, default 1) shrinks the region. Each output pixel is the floor of the mean of its `divisor x divisor` block, per channel. Only opaque `bgr0` sources can be shrunk, because a straight-alpha source needs alpha-weighted averaging. A shrunk alpha source fails at render with `UNSUPPORTED_MEDIA`; scenes can render titles at the size they need instead.
3. `opacity` (0-255, default 255) sets alpha to `floor((alpha x opacity + 127) / 255)`.
4. `position` (default `[0, 0]`) places the result's top-left corner on the canvas. It may be negative or past the edge, and the parts outside the canvas are clipped.

Everywhere else the canvas is transparent. The overlay is then composited with the same exact straight-alpha over as a full-frame overlay. For example, a 1920x1080 camera on an `alpha_over` track with `{"divisor": 4, "position": [1424, 48]}` becomes a 480x270 inset in the top-right corner.

Transforms are rejected on other tracks. Session diffs list a clip's `transform`, and interchange export reports it as a critical loss. The steps map to the pinned FFmpeg build's `crop`, block-average `pixelize` with neighbor decimation, `lutrgb` and transparent `pad` filters. The overlays fixture compares every rendered frame, previews and a range export against an independent implementation of these equations, with zero tolerance.

## Clip gain and fades

Audio-track clips carry three optional fields, also accepted in a `place` clip. They are omitted from snapshots at their defaults:

- `gain_milli`: linear gain from 0 to 4000, where 1000 (the default) is unity.
- `fade_in`: a linear fade from the clip start.
- `fade_out`: a linear fade ending at the clip end.

Fades align to the 48 kHz clock and last at most 60 seconds each. Together they must fit the clip, so they never overlap. The semantics match a [mix recipe](AUDIO.md) voice. At clip sample `i` of `n`, the weight is `gain_milli x min(i, fade_in) / fade_in` inside the fade-in, `gain_milli x min(n - i, fade_out) / fade_out` inside the fade-out, and `gain_milli` elsewhere, divided by 1000. Each sample is rounded once to the nearest PCM16 value, ties away from zero. This happens before transitions and track summation, so every product stays an exact integer.

Video clips reject these fields. A clip edge with a [transition](TRANSITIONS.md) rejects a fade there, because the transition already shapes the cut. Transition handles play at the clip's plain gain.

Edits keep the audible result:

- Fades follow clip edges through `roll`, `slide` and `slip`.
- A `split` or interval edit gives each part the fades at the edges it shares with the original clip.
- A cut that falls inside a fade fails with `INVALID_RANGE`; shorten the fade with `clip_audio` first.
- Range previews and exports keep the original clip's envelope, so a window that starts inside a fade plays exactly as the full render does.
- Interchange export reports clip gain and fades as a critical loss.

`timeline.meters` measures levels without exporting. It takes a `project`, `input_root`, an optional frame-aligned `start` and `duration` (at most 600 seconds; by default to the timeline end), and `tracks` (default true). It renders the range's audio exactly as an audio-only export would. It then reports sample peak, RMS and BS.1770 integrated loudness (`integrated_lkfs`) for the mix of enabled audio tracks and, with `tracks`, for each enabled audio track played alone. Each track costs one more audio pass. Use it to set `gain_milli` against a loudness target. Halving a clip's gain lowers its measured loudness by 6.02 dB.

Session diffs and previews list a clip's `levels` (`gain_milli`, `fade_in`, `fade_out`) when it has gain or fades, so a `clip_audio` change appears as a clip change.

## Sessions, previews and export

Session diffs include `track_id` in clip placements and a `track_layout` before/after record when track order, kind, state, transitions or link anchors change. Placement `index` reports its array position; array-order-only differences within a track are not playback changes and do not create clip diff entries. Duration, assets and proxy state retain their existing diff fields. Retry receipts, revision guards, undo, restoration and atomic history behave as for sequential projects.

`preview.frame` selects the visible track and returns its ID with the source-frame receipt; uncovered video returns black. `preview.range` and range exports intersect placed source intervals without modifying the saved snapshot. Proxy previews retain clip/link timing and reduced dimensions. Synchronous and queued reference renders and delivery exports always use original-quality media. Enabled sources are inspected, identity checked and rechecked before publication, including enabled clips hidden by higher video. Disabled sources are not opened for a render. Existing output files and original inputs remain protected.

The track compiler uses the already selected external FFmpeg build's public [sample-delay](https://www.ffmpeg.org/ffmpeg-filters.html#adelay), [audio mixing](https://www.ffmpeg.org/ffmpeg-filters.html#amix), trim and concat interfaces. It assigns the final video frame clock from exact frame counts so one-frame segments cannot collapse. All project editing, synchronization, collision and validation behavior is original project code. No new dependency is added; exact tool identity and licenses remain in [DEPENDENCIES.md](DEPENDENCIES.md).

## Acceptance and remaining scope

Run `python -X utf8 tests/tracks.py --output C:\DEV\CutboltData\new-tracks-test` with a fresh external directory. The original fixture compares decoded frames with an independent rational interval/pixel reference and audio with independent integer sums and final saturation, with zero pixel/sample tolerance. It covers overlaps, one-frame boundaries, leading/trailing gaps, subframe audio offsets, 32 simultaneous voices, linked move/remove, retargeting, locks, collision/replacement, saved diffs/retry/undo/restore, atomic rollback, previews, range/delivery output, proxies, queued full quality, sequential promotion, malformed inputs, limits, missing/changed sources and original-media preservation.

This slice targets T08 basic/extended and T04 extended. [Native transitions](TRANSITIONS.md), [linked boundary edits](TRACK_EDITS.md) and [nested sequences](SEQUENCES.md) have separate implementations and acceptance fixtures. [Multicam and synchronization](MULTICAM.md) also have separate acceptance fixtures. Fractional-rate/VFR rendering and long-form synchronization remain required work. This model does not replace the richer separate scene/audio recipes; those still compile to editing assets. None of these other criteria is credited by the original track fixture.

The retained `C:\DEV\CutboltData\tracks-20261003-05` run passed all eight track checks: 21 render cases, 348 decoded frames and 668,160 stereo sample frames matched exactly; 38 invalid/failure cases returned the expected error. Twelve original-size frame previews and a half-size proxy preview matched their expected pixels. Both operation batches in this document were also executed and verified for exact linked placement. The authoritative score requires the complete repository verifier, not this retained run alone.

[Camera groups](MULTICAM.md) can also be placed by `sequence_id`. They retain editable alternatives and exact source offsets in a managed child definition; camera decisions use `multicam.edit`. [Synchronization inspection](SYNCHRONIZATION.md) prepares explicit new aligned assets when measured clock correction is required.
