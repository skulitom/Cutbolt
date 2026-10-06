# Editable track transitions

Native tracks support four encoded-RGB8 video transitions: `dissolve`, `dip_black`, `wipe_left` and `wipe_right`. Audio tracks support `dissolve` and `dip_black` (a dip through silence). Transitions stay in the saved project and use the existing atomic edit, preview, history, undo and render contracts. They do not flatten or change source media.

## Intervals and source handles

Each track has an optional `transitions` array. An entry requires a transition `id` unique within its arrangement, `left_id`, `right_id`, `before`, `after` and `kind`. IDs use the existing 1–128-byte limit. Both endpoint clips must be distinct, on this track and exactly adjacent: the left clip's end equals the right clip's start. This common time is the cut.

The effect covers `[cut - before, cut + after)`. Both lengths are nonnegative rational seconds, aligned to video frames or audio samples according to track kind, with a positive sum. Each side must be covered by its endpoint's body or touching fragments with the same asset and exact continuous source mapping. This permits content-preserving splits inside an effect without changing its clock; gaps and discontinuities are rejected. Either side can be zero. Intervals on the same track cannot overlap; exactly touching intervals are valid. A track may contain at most 1,000 transitions, subject to the existing clip and request limits.

The incoming source needs at least `before` of preroll before its selected source-in. The outgoing source needs at least `after` of postroll beyond its selected source-out. The engine checks declared bounds when editing and actual decoded bounds when rendering. Missing handles fail with `INSUFFICIENT_HANDLES`; invalid adjacency or style fails with `INVALID_TRANSITION`; intersecting intervals fail with `TRANSITION_COLLISION`. No source freezing, implicit shortening or synthetic handle generation occurs.

Both sources retain their timeline mapping throughout the effect: source time is `source_in + timeline_time - clip_start`. Handles extend the readable source interval without moving the clip, changing its duration or changing the arrangement's explicit end. For asymmetric handles, the visual midpoint is the midpoint of the complete effect interval; it need not equal the nominal cut.

## Exact sampling

For an interval containing `n` frames or samples, sample index `k` uses `p = (2k + 1) / (2n)`, with `k` starting at zero. This samples each frame/sample center and also defines one-unit effects. Interpolation continues at the original interval position when a preview or export starts in its middle, or an upper video track temporarily covers it.

For outgoing value `L` and incoming value `R`:

| Style | Evaluation |
| --- | --- |
| `dissolve` | `(1 - p) * L + p * R` |
| `dip_black` | `max(1 - 2p, 0) * L + max(2p - 1, 0) * R` |
| `wipe_left` | Incoming pixels where `(x + 0.5) / width <= p`; outgoing elsewhere |
| `wipe_right` | Incoming pixels where `(width - x - 0.5) / width <= p`; outgoing elsewhere |

The wipe names indicate the edge from which incoming pixels appear. RGB channels use encoded 8-bit values, rounded to nearest with ties upward. Audio uses the same temporal weights independently on both PCM16 channels, rounded to nearest with ties away from zero. Each transition's stereo result is rounded before summing enabled audio tracks; the final mix saturates once to the PCM16 range. A dip uses black for video and zero for audio. An odd-length dip has a sample at exactly `p = 0.5`; an even-length dip samples on either side of that midpoint.

This profile is 25 fps RGB8 with 48 kHz stereo PCM, using the same source, dimension, clip-count, duration, process-time and filter-argument limits as [native tracks](TRACKS.md). It interpolates encoded samples; normalize sources explicitly before editing when a consistent working color interpretation is required. No linear-light or high-bit-depth transition profile is claimed here. Unsupported precision and resource sizes fail explicitly.

## Editing

Use `tracks.edit` with these typed edits:

| Edit | Additional fields | Result |
| --- | --- | --- |
| `transition_set` | `track_id`, complete `transition` | Add a transition or replace the same ID on that track |
| `transition_remove` | `track_id`, `id` | Remove that transition and restore the underlying cut |

Both operations require an unlocked track. Video and audio transitions are specified separately, so their intervals and styles may differ. Linking clips preserves content synchronization; it does not implicitly add or change effects on another track.

For existing adjacent `vl` and `vr` clips on track `v`, this operation requests two frames before the cut and three after it:

```json
{"op":"tracks.edit","edit":{"op":"transition_set","track_id":"v","transition":{"id":"vt","left_id":"vl","right_id":"vr","before":{"num":2,"den":25},"after":{"num":3,"den":25},"kind":"dissolve"}}}
```

Place it in the `operations` array of `session.preview`, then `session.apply` with the inspected revision and a unique request ID. `timeline.apply` accepts the same operation for caller-owned snapshots. To remove it:

```json
{"op":"tracks.edit","edit":{"op":"transition_remove","track_id":"v","id":"vt"}}
```

Moving both endpoints together retains the transition and its interval relative to the cut. Retargeting both to the same track carries the transition with them. Moving one endpoint so adjacency is broken rejects the whole batch. Explicitly remove the transition first when the intended edit breaks that relationship. Removing or replacing an endpoint clip also removes its incident transitions, including transitions attached to linked clips removed by the same selection. These collateral changes appear in `track_layout` diffs. Locked source, target and affected linked-partner tracks retain their existing protection.

Transitions are included in saved track state, retry receipts, undo/restoration and JSON schemas. Changes to transition settings alone produce a track-layout diff without pretending that clip placement changed. Old snapshots and receipts that omit transitions remain readable.

## Previews, ranges and output

The renderer evaluates the requested interval against the original arrangement. Cropping a range does not recreate a transition at the crop boundary or drop the off-range clip whose source handle is still needed. The resulting file starts at time zero, with exact requested frame/sample counts, while the saved project remains unchanged.

At a transition frame, `preview.frame` returns `transition_id`, `track_id`, `source: null` and a `sources` list of inspected inputs. The list can include covered video tracks whose identities were also checked. Ordinary single-source frame receipts keep their previous shape. Final preview publication rechecks every listed identity. Scope inspection validates the declared color interpretation of all listed sources and exposes `source_interpretations`; it reports the same RGB values as the preview. Its existing single-source `interpretation` remains available when there is exactly one source.

Proxy frame/range previews evaluate transitions at reduced dimensions with unchanged clocks. Full-quality synchronous and queued reference renders ignore proxy selection, as before. Reference and H.264/AAC range exports use the same transition interval evaluation. Existing outputs and original media remain protected, including when an input changes after encoding.

The engine mixes top-level video transitions itself, before any [overlay track](TRACKS.md) above them:
- FFmpeg decodes the base picture, which shows each transition's outgoing side, to raw planar RGB. One more decoder per visible run of a transition supplies its incoming side. Decoders start up to 25 frames ahead, at most two early at a time, so their startup overlaps earlier frames.
- The engine evaluates the equations above in integers, in parallel row bands. A dissolve frame computes `L + floor(((R - L)k + n) / 2n)` for k = 2i + 1 from a 511-entry table of differences. A dip frame maps one side through a 256-level table, because at most one of its weights is positive. A wipe frame copies whole columns of every row.
- `render.plan` lists these runs under `compositor.transitions`, with each decoder's arguments.

On the effects-reel demo, a 172 s 1080p25 film with 15 top-level transitions of 12–16 frames and three overlay tracks, the H.264 export took 91–144 s instead of 362–486 s, with identical files. A 12-frame wipe range took 5.5 s instead of 13.3 s, against 4.4 s for 12 plain frames at the same place. These are back-to-back release builds while other sessions kept the machine 15–85 % busy; the [progress history](PROGRESS_HISTORY.md) has the details.

Video transitions inside nested sequences are still compiled into that sequence's FFmpeg graph as expressions of the existing external build's public [blend](https://www.ffmpeg.org/ffmpeg-filters.html#blend_002c-tblend) interface, evaluated per pixel on one filter thread, which is much slower. Audio transitions use its [audio expression](https://www.ffmpeg.org/ffmpeg-filters.html#aeval) and [channel join](https://www.ffmpeg.org/ffmpeg-filters.html#join) interfaces, with inputs mapped explicitly by channel and sample index. No third-party implementation is copied or added; dependency versions and licenses are unchanged.

## Verification and remaining work

Run `python -X utf8 tests/transitions.py --output C:\DEV\CutboltData\new-transition-test` with a fresh external directory. The fixture generates original motion frames, a geometric still card, black video and distinct stereo signals. Independent rational interval selection and integer pixel/PCM equations check every output value with zero tolerance.

Coverage includes all styles, asymmetric and one-sided handles, one-frame effects, touching intervals, temporarily covered video, summed audio, subframe audio offsets, linked moves/retargeting/removal/replacement, locked tracks, saved preview/retry/undo/restoration, atomic rollback, frame and inside-transition range previews, scopes, reference/delivery exports, proxies and queued original-quality output. Failure checks cover insufficient declared/decoded handles, conflicting color declarations, invalid references/intervals/styles, duplicate IDs, extreme clocks, changed sources and existing destinations.

The retained `C:\DEV\CutboltData\transitions-20261003-02` run compared **510 decoded frames, 979,200 stereo sample frames and 29 frame previews**, across 32 rendered cases, all exactly. It checked 20 rejection/failure cases; a separately executed extreme-clock regression adds the 21st case to the full verifier. Both documented operations were executed successfully and preserved clip placement and the separate audio transition. The full verifier passed all seven transition checks and all 21 rejection/failure cases, bringing the repository total to 243 passing checks and core coverage to 61/100. The complete verifier remains the authority for the score.

This slice targets V03 basic and extended. [Native linked boundary edits](TRACK_EDITS.md) have their own implementation and acceptance fixture for split, interval insert/overwrite/ripple and slip/slide/roll. Nested effects and fractional-rate/VFR timing retain their separate requirements. No points for those checkpoints are awarded by the original transition fixture.
