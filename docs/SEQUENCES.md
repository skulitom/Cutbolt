# Reusable nested sequences

A native track clip can reference a named child sequence instead of a media asset. Every instance uses the same editable definition. Changing that definition updates all direct and indirect instances through the existing snapshot and local saved-session contract. Rendering evaluates child timelines directly; it does not replace their definitions with generated media assets.

## Definitions and references

The optional project `sequences` array contains entries with `id` and `arrangement`. Each arrangement has the same `duration`, `tracks`, `links`, clips and transitions as the root native timeline. Children share the project's dimensions, frame rate and media registry. They may reference other children. Existing projects that omit `sequences` keep their behavior.

A native clip has exactly one nonempty source reference:

- `asset_id` selects a physical registered media source.
- `sequence_id` selects a child definition; omit `asset_id`.

These are distinct namespaces, so an asset and child may have the same ID without ambiguity. Clip, track, link and transition IDs are local to each arrangement. Sequence IDs are unique within the project. A child reference uses the normal clip `id`, `start`, `source_in` and `duration`; every native placement, split, linked move and boundary edit retains its source-reference type.

The model permits 32 child definitions, eight child levels and 1,000 total native clips across the root and catalog. Each arrangement retains its existing track/link limits. Empty child definitions are editable; positive-duration empty definitions render black video and silence. Cycles, missing definitions, invalid references and excessive depth fail when validating or saving, including in unused definitions and disabled tracks. Removing a referenced definition is prohibited.

## Time, effects and compositing

The mapping is exact unit-rate playback:

`child_time = parent_time - instance_start + source_in`

Video source windows align to the project frame clock; audio references may use individual 48 kHz samples. The complete source window must fit the child duration. Parent transitions may read child preroll/postroll only within that duration. Child transitions keep their own clocks and source handles, even when a parent range starts inside one. Multiple nesting levels, repeated instances and parent/child effects follow the same rule. Speed changes, independent child frame rates and transparent child output are not implicit conversions.

A video child produces its own highest enabled opaque track, with black in its gaps. That entire output is opaque in the parent: a gap inside an active child hides lower parent video with black. A gap between parent instances exposes lower parent tracks normally.

Each child's audio tracks mix and saturate to PCM16 at that child's output boundary. The parent then uses those PCM16 values in its own transitions and mix. Saturation is therefore part of the reusable child's sound; nesting does not silently flatten all original voices into one unbounded mix. Child sample offsets and effect rounding remain exact. A video reference reads child video and an audio reference reads child audio; use separate linked instances to carry both streams.

Physical sources retain the reference profile: 25 fps RGB8 FFV1 and 48 kHz stereo PCM16 at matching dimensions. Other inputs use explicit media conversion. Root renders retain the existing 64-root-clip, 8M-pixel and 180,000-frame bounds. Recursive inspection allows at most 256 expanded clip/handle checks; graph construction permits at most 2,048 segments and the existing 24,000-character argument budget. Reuse can exceed these expansion limits despite a small saved catalog and then fails explicitly. Fractional-rate/VFR/long-form timing and multicam keep their separate acceptance requirements.

## Saved editing

Use ordinary `timeline.apply`, `session.preview` and `session.apply` with these operations:

| Operation | Fields | Behavior |
| --- | --- | --- |
| `sequence.create` | `id`, `duration` | Add an empty native child definition |
| `sequence.edit` | `id`, `edit` | Apply one existing native track edit inside that definition |
| `sequence.remove` | `id` | Remove an unreferenced definition whose tracks are unlocked |

The `edit` field is the inner edit object from `tracks.edit`. It supports track creation/order/state, placements, links, transitions and boundary edits. `create` and `promote` are rejected there because the definition already has a native arrangement. Each operation must leave a valid project; batches remain atomic. A shortened child cannot leave any instance or transition handle out of bounds.

Child track locks retain their ordinary protection. A definition edit also checks every direct and transitive parent reference: a locked reference track rejects the edit, even when disabled or inside an otherwise unused definition. Unlock the applicable parent tracks explicitly before changing shared content. This prevents a child edit from bypassing a lock elsewhere in the dependency graph.

Session diffs include a `sequences` list with each changed definition's `before`, `after` and `root_instances`. The latter lists all root clips referencing it directly or through another child, including disabled instances. Root placement diffs carry `sequence_id`, so switching between children remains visible even when times are unchanged. Unchanged parent placements are not reported as moved merely because their child content changed. The history summary's `changed_clips` counts direct root placement changes; inspect the receipt's `sequences` entries for shared-content changes. History snapshots, retry receipts, undo and restoration preserve the full catalog. Older receipts without sequence fields remain readable.

This operation batch assumes a registered compatible media asset `0`, an existing native root timeline at least eight frames long, and unused IDs `intro`, `intro-over`, `intro-one` and `intro-two`. It creates a four-frame child and places two instances on a new upper video track:

```json
[
  {"op":"sequence.create","id":"intro","duration":{"num":4,"den":25}},
  {"op":"sequence.edit","id":"intro","edit":{"op":"add","track":{"id":"v","kind":"video","locked":false,"enabled":true,"clips":[]}}},
  {"op":"sequence.edit","id":"intro","edit":{"op":"place","track_id":"v","clip":{"id":"source","asset_id":"0","start":{"num":0,"den":1},"source_in":{"num":0,"den":1},"duration":{"num":4,"den":25}},"collision":"reject"}},
  {"op":"tracks.edit","edit":{"op":"add","track":{"id":"intro-over","kind":"video","locked":false,"enabled":true,"clips":[]}}},
  {"op":"tracks.edit","edit":{"op":"place","track_id":"intro-over","clip":{"id":"intro-one","sequence_id":"intro","start":{"num":0,"den":1},"source_in":{"num":0,"den":1},"duration":{"num":4,"den":25}},"collision":"reject"}},
  {"op":"tracks.edit","edit":{"op":"place","track_id":"intro-over","clip":{"id":"intro-two","sequence_id":"intro","start":{"num":4,"den":25},"source_in":{"num":0,"den":1},"duration":{"num":4,"den":25}},"collision":"reject"}}
]
```

This subsequent operation advances the shared source content by one frame, changing both instances without moving them:

```json
{"op":"sequence.edit","id":"intro","edit":{"op":"slip","clip_ids":["source"],"shift":{"backward":false,"amount":{"num":1,"den":25}},"links":"include"}}
```

## Rendering, previews and preservation

Frame/range previews, reference exports, delivery range exports and queued full-quality output evaluate the same nested graph. A root containing nested references uses graph-based frame previews; receipts report `source: null`, inspected `sources`, and the visible root `track_id`, `sequence_id` and `transition_id` when present. Scope inspection can use the listed physical source interpretations. Proxy selection follows enabled references of the appropriate video/audio kind through the catalog and requires the existing bound proxy for each used source. Final renders ignore preview proxy selection.

Every enabled clip used by the requested window is inspected, including covered video. Inspection follows referenced child windows and checks entire selected clip bodies plus intersecting transition handles against decoded physical media. Source identities are checked before encoding and again before publication. Missing/changed sources, invalid decoded ranges, existing destinations and expansion limits fail without publishing partial output. Original sources remain unchanged.

## Independent acceptance

`tests/sequences.py` generates original frames and signed stereo signals. Its reference constructs complete RGB/PCM buffers for every child using independent integer equations, then selects source windows from those buffers for parent composition. This differs from the engine's interval/filter graph and verifies exact per-child saturation, transition rounding and rational offsets. All output comparisons use zero pixel or PCM tolerance.

The fixture covers repeated and multilevel reuse, effects in both parent and child, individual-sample offsets, shared source/effect/mix edits, splitting instances inside effects, opaque child gaps, explicit source namespaces, eight levels, cycle/depth/expansion rejection, direct/transitive/disabled locks, missing references, insufficient handles, saved diffs/retries/rollback/undo/restoration, all three operation schemas, exact frame/range/proxy previews, exports, queued original quality and source-preservation failures. Run `python -X utf8 tests/sequences.py --output C:\DEV\CutboltData\new-sequence-test` with a fresh external directory. The complete verifier supplies the evidence for T09 basic and extended; this feature adds no points for multicam, cache acceleration or broader rate/synchronization work.

The retained `C:\DEV\CutboltData\sequences-20261003-03` run and complete verifier each compared **423 decoded frames, 812,160 stereo sample frames and 17 frame previews** across 16 rendered cases, and passed 23 rejection/failure cases. Both documented examples executed successfully. A separate numerical-scope check matched the independently verified nested frame and checked both physical source interpretations. Full repository verification passed **256 unique checks**, producing **66/100 core** and **66/108 total (61.1%)**; generated progress remains authoritative.

## Managed camera definitions

[Camera groups](MULTICAM.md) extend a sequence with retained `multicam` metadata. Their native arrangement is a checked projection of camera cuts and audio policy. Use `multicam.edit` for these definitions; `sequence.edit` rejects them. All alternate cameras participate in cycle, deletion, ancestor-lock and affected-instance checks, even when unselected.
