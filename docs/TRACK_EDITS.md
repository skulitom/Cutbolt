# Native boundary edits

Native tracks support linked splits, source slips, rolling cuts, three-clip slides, interval insertion, interval overwrite and ripple deletion. These are typed `tracks.edit` operations in the existing snapshot and saved-session contract. All times are exact rational seconds. Video edits align to the project frame clock; audio-only edits can use individual 48 kHz samples. Rendering retains the [native track limits](TRACKS.md), including 25 fps reference sources.

Every operation validates the complete resulting arrangement before it is committed. A failed operation discards the whole batch, including earlier operations in that batch. Use `session.preview` to inspect clip placements, source ranges, link groups, transitions and the explicit end before applying. Successful requests retain normal revision checks, safe retries, undo and restoration.

## Clip and boundary operations

Wrap each edit as `{"op":"tracks.edit","edit":{...}}`:

| Edit | Required fields | Result |
| --- | --- | --- |
| `split` | `clip_ids`, `at`, `links`, `right_clip_ids`, `right_link_ids` | Split every selected clip at the same absolute timeline time; preserve source mapping and output |
| `slip` | `clip_ids`, `shift`, `links` | Shift the selected source ranges; keep timeline starts and durations |
| `roll` | `left_ids`, `shift`, `links` | Shift each selected cut and the incoming source-in; compensate both durations |
| `slide` | `clip_ids`, `shift`, `links` | Move each selected middle clip, extend/trim its previous neighbor and compensate its next neighbor |

`shift` is `{"backward":false,"amount":{"num":1,"den":25}}` for one frame forward. Set `backward` to true for a backward shift. It changes source time for slip and timeline boundaries for roll/slide. Amounts are nonnegative; zero is valid.

`links: "include"` expands clip selections to linked partners. Roll and slide also discover linked neighbors, then their corresponding adjacent clips, until the entire boundary edit is included. `"reject_partial"` rejects a selection that needs an omitted partner. Every affected track must be unlocked, even when it is disabled. A clip cannot serve as both a middle and a neighbor, or both sides of a rolling selection. Roll needs an exactly adjacent next clip; slide needs exactly adjacent clips on both sides. Gaps do not supply synthetic neighbors.

All surviving durations must be positive. Source handles, alignment, synchronization, collisions and transition intervals must remain valid. Transition lengths and styles remain unchanged during slip/roll/slide; a moved cut carries its effect clock. An edit that exhausts a source handle or makes two effects overlap fails atomically. The arrangement's explicit end stays unchanged for these four operations.

For split, `at` must be strictly inside every expanded member. The left child keeps its old ID; the right child needs a new clip ID unique within this arrangement. Supply mappings as `[{"id":"old","new_id":"new"}]`. A split link keeps its ID for the left group and needs an explicit new ID for the right group. All original synchronization anchors remain intact; no edit silently rebases them. Missing required mappings fail with `SPLIT_ID_REQUIRED`, surplus mappings with `UNUSED_SPLIT_ID`, and duplicate new IDs with `DUPLICATE_ID`.

This example assumes `v0` and `a0` are linked by `link0`, both contain time 9/25, and the new IDs are unused:

```json
{"op":"tracks.edit","edit":{"op":"split","clip_ids":["v0"],"at":{"num":9,"den":25},"links":"include","right_clip_ids":[{"id":"v0","new_id":"v0-r"},{"id":"a0","new_id":"a0-r"}],"right_link_ids":[{"id":"link0","new_id":"link0-r"}]}}
```

Splits can fall inside an existing transition. The outgoing endpoint becomes its last surviving fragment and the incoming endpoint its first. A transition may span several touching fragments only when they retain the same asset and exact continuous source mapping on their respective sides of the cut. Its original clock and every rendered frame/sample remain unchanged. A gap or discontinuous source mapping inside that interval fails validation. Repeated splits obey the same rule.

For example, these individual operations shift the middle clip's content, the first cut, or the middle clip's position by one frame:

```json
{"op":"tracks.edit","edit":{"op":"slip","clip_ids":["v1"],"shift":{"backward":false,"amount":{"num":1,"den":25}},"links":"include"}}
```

```json
{"op":"tracks.edit","edit":{"op":"roll","left_ids":["v0"],"shift":{"backward":false,"amount":{"num":1,"den":25}},"links":"include"}}
```

```json
{"op":"tracks.edit","edit":{"op":"slide","clip_ids":["v1"],"shift":{"backward":false,"amount":{"num":1,"den":25}},"links":"include"}}
```

## Interval operations

| Edit | Interval fields | New placements | Result |
| --- | --- | --- | --- |
| `insert` | `at`, `duration` | Required `clips` array, which may be empty | Open space, split crossing clips, shift later selected content right, and place new clips inside the space |
| `overwrite` | `at`, `duration` | Required `clips` array, which may be empty | Remove selected content inside the interval and place replacements; preserve outside timeline positions |
| `ripple_delete` | `start`, `duration` | No `clips` field | Remove the interval and shift later selected content left |

All three also require `track_ids`, `links`, `right_clip_ids`, `right_link_ids`, `end_policy` and `transitions`. Select 1–32 unique existing tracks. The interval is nonempty and cannot start after the explicit end. Ripple deletion must also end within the arrangement. Endpoints align to every selected track's clock.

With `links: "include"`, an affected linked member expands the selection to its partner's **whole track**. Expansion repeats through affected links on those tracks. This may edit other clips on the added tracks; the session preview exposes the complete result. `"reject_partial"` requires callers to select all needed tracks explicitly. Every selected or expanded track must be unlocked.

Each new placement is `{"track_id":"v","clip":{"id":"new","asset_id":"asset","start":{"num":4,"den":25},"source_in":{"num":0,"den":1},"duration":{"num":2,"den":25}}}`. It must lie entirely inside the insertion/replacement interval on a selected or expanded track. Its ID must be new, including relative to clips removed by the same operation and newly allocated fragments. Same-track overlaps reject the whole edit. New clips are not linked automatically; follow with an explicit `link` operation if needed.

When an old clip survives on both sides, use the same explicit right-child mappings as split. A single surviving piece keeps its old ID; a removed clip has no survivor. Linked members must retain equal numbers of fragments with consistent synchronization changes, or the edit fails with `SYNC_CONFLICT`. Explicitly unlink first only when an independent edit is intended. Supplying IDs for pieces that do not exist is an error.

The required end policy controls the arrangement duration:

| `end_policy` | Insert | Overwrite | Ripple deletion |
| --- | --- | --- | --- |
| `keep` | Keep explicit end | Keep explicit end | Keep explicit end; vacated tail becomes a gap |
| `resize` | Add interval duration | Extend only if replacement ends later | Subtract interval duration |

No policy truncates clips outside the selected tracks. If a retained clip would extend past the resulting explicit end, the operation fails. For example, deleting from selected tracks with `resize` can fail when an unselected track has late content; `keep` preserves that end. Insertion with `keep` succeeds only when all shifted clips still fit. Audio-only sample edits can use `keep` when resizing would make the global end unaligned to video frames. A zero-duration result is editable but cannot be rendered.

The `transitions` policy is explicit:

- `reject_affected` rejects an insertion strictly inside an effect, or an overwrite/deletion interval overlapping one.
- `remove_affected` removes each such effect in full before editing its clips. This changes output wherever that effect previously applied, including outside the edited interval.

Untouched transitions retain their settings and move with their cut where appropriate. Deleted endpoints remove their incident effects. Final continuity and handle checks still apply. This policy does not authorize otherwise invalid synchronization or collisions.

This example opens one frame of empty space at 4/25 on linked tracks, splitting their first pair and increasing the explicit end. The old clips and link must exist and straddle the insertion point:

```json
{"op":"tracks.edit","edit":{"op":"insert","track_ids":["v"],"at":{"num":4,"den":25},"duration":{"num":1,"den":25},"clips":[],"links":"include","right_clip_ids":[{"id":"v0","new_id":"v0-after"},{"id":"a0","new_id":"a0-after"}],"right_link_ids":[{"id":"link0","new_id":"link0-after"}],"end_policy":"resize","transitions":"reject_affected"}}
```

## Verification and boundaries

`tests/track_edits.py` generates original frames and stereo signals with linked video/audio starts and source offsets differing by seven samples. Independent rational source mapping and integer transition equations establish base output. Split output must be identical; insertion, overwrite and ripple output must equal explicitly spliced byte sequences. Slip/slide/roll placements are computed separately before comparing every decoded pixel and PCM sample, with zero tolerance.

The fixture also covers repeated splits across continuous handles, individual-sample audio cuts, neighbor-link discovery, fixed/resized ends, explicit transition removal, locked partners, collisions, unequal survivors, source-handle exhaustion, exact saved diffs, all seven MCP schemas, retries, rollback, undo/restoration, frame/range previews, proxies, reference exports and queued full-quality output. Source hashes and existing destinations remain protected, including a changed-input failure after encoding.

Run `python -X utf8 tests/track_edits.py --output C:\DEV\CutboltData\new-track-edit-test` with a fresh external output directory. The complete `tools/verify.py` suite supplies score evidence for T03, T06 and T07 extended. These checks do not award fractional-rate/VFR/long-form timing, nested sequences or multicam checkpoints. No source files, dependencies or research artifacts are copied into the engine.

The complete run verified **725 decoded frames, 1,392,000 stereo sample frames and 15 frame previews** across 25 renders, plus 34 rejection/failure cases. The initial retained fixture at `C:\DEV\CutboltData\track-edits-20261003-01` includes 695 frames and 1,334,400 sample frames; the full verifier adds a repeated aligned split and all saved interval schemas. All five documented edit examples also executed successfully. Full repository verification passed 250 unique checks, producing **64/100 core** and **64/108 total (59.3%)**. The generated progress files remain authoritative.
