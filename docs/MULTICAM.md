# Editable camera selections

`multicam.create` and `multicam.edit` keep synchronized camera alternatives and cut decisions in a reusable sequence. They use the existing `timeline.apply` and saved-session tools. A group produces ordinary native video/audio tracks, so previews, ranges, proxies, child/parent effects and queued reference output use the existing renderer.

Synchronization can be declared as exact source windows or estimated with [audio/timecode inspection](SYNCHRONIZATION.md). Estimated clock correction produces new reference assets through explicit media conversion. Original sources remain unchanged.

## Group contract

Each camera angle references a child `sequence_id`. It declares a covered group interval `[start, start + duration)` and separate `source_in` and `audio_in` positions in that child. At group time `t`, video selects `source_in + t - start`; audio selects `audio_in + t - start`. Video positions and group cuts must align to 25 fps. Audio positions can align to individual 48 kHz samples. All clocks are exact rational seconds.

The complete declared angle windows must fit the child, including unused alternatives. Selected angles must cover their entire cut interval. Unequal camera coverage is allowed; choosing a camera outside its coverage rejects the edit. A fixed audio angle must cover the whole group.

`cuts` retain stable IDs, times and angle IDs. There must be a selection at zero, and no two cuts may share a time. Input order is retained as metadata; the projection orders cuts by time. Cutting back to an earlier camera does not discard any alternative.

Choose an explicit audio policy:

| Policy | Behavior |
| --- | --- |
| `{"mode":"fixed","angle_id":"wide"}` | Keep the selected master camera's audio through every video cut |
| `{"mode":"follow_video"}` | Select each camera's independently mapped audio with its picture |
| `{"mode":"mute"}` | Produce silence |

For existing child definitions `cam0` and `cam2`, this operation creates a 30-frame selection. The second camera has a five-frame video prefix and a five-frame-plus-17-sample audio prefix:

```json
{
  "op": "multicam.create",
  "id": "program-copy",
  "group": {
    "duration": {"num": 6, "den": 5},
    "locked": false,
    "angles": [
      {"id":"wide","sequence_id":"cam0","start":{"num":0,"den":1},"duration":{"num":6,"den":5},"source_in":{"num":0,"den":1},"audio_in":{"num":0,"den":1}},
      {"id":"side","sequence_id":"cam2","start":{"num":0,"den":1},"duration":{"num":6,"den":5},"source_in":{"num":1,"den":5},"audio_in":{"num":9617,"den":48000}}
    ],
    "cuts": [
      {"id":"first","at":{"num":0,"den":1},"angle_id":"wide"},
      {"id":"second","at":{"num":3,"den":5},"angle_id":"side"}
    ],
    "audio": {"mode":"follow_video"}
  }
}
```

Place the returned group by `sequence_id` on native video and audio tracks as described in [nested sequences](SEQUENCES.md). Each placement may select its own child window. Ordinary parent links can protect the paired root placements.

## Recoverable edits

`multicam.edit` takes the group `id` and one tagged `edit`:

| `edit.op` | Fields | Behavior |
| --- | --- | --- |
| `cut_set` | `cut: {id, at, angle_id}` | Insert a decision or replace the decision with that ID |
| `cut_remove` | `id` | Remove a decision; the preceding selection continues |
| `angle_set` | `angle` | Insert or update an alternative's full declaration |
| `angle_remove` | `id` | Remove an unused alternative explicitly |
| `audio` | `policy` | Change fixed/follow/mute audio |
| `duration` | `duration` | Change group length after resolving affected parent windows |
| `state` | `locked` | Lock or explicitly unlock the group |

Removing the first cut, a selected camera or the fixed audio camera rejects. Removing a cut preserves its alternative camera. Reintroducing the cut recovers that choice. Saved preview diffs include before/after camera metadata and affected root instances; retries, rollback, undo and restoration retain the full catalog.

Every angle is a dependency, including currently unselected ones. Cycles and removal of referenced definitions reject. Changing an unused angle's child is still blocked by a locked group or a transitive locked parent. A group's own lock blocks content changes; only its `state` operation can unlock it. Ordinary `sequence.edit` rejects managed camera groups with `MANAGED_SEQUENCE`. Use `sequence.remove` to remove an unreferenced, unlocked group.

The serialized `multicam` decisions are authoritative. The native `arrangement` must exactly match their deterministic projection. Manually changing that projection fails with `MULTICAM_PROJECTION_MISMATCH`.

## Limits and evidence

The profile supports 2–16 angles, 1–128 cuts, cut IDs up to 120 UTF-8 bytes, and positive frame-aligned duration at 25 fps. Existing [sequence](SEQUENCES.md) catalog, depth, clip-count, graph-expansion and renderer limits still apply; 128 decisions do not guarantee every resulting graph fits the renderer. Sources use the bounded RGB8/stereo PCM reference profile. Camera cuts are instantaneous; use explicit parent transitions when desired. General fractional/VFR timeline output and nonlinear clock correction retain separate requirements.

`tests/multicam.py` independently checks the camera metadata against original video event clocks and PCM signals. Its retained fixture is `C:\DEV\CutboltData\multicam-20261003-01`. It compares 543 frames, 1,042,560 stereo sample frames and 11 still previews across 18 renders, with zero pixel/PCM tolerance, plus 29 rejection/failure cases. It exercises all audio policies, coverage boundaries, unused dependencies, transitive locks, projection tampering, camera recovery, saved/MCP edits, proxy/range/export output, queued original quality and source changes during encoding. The separate synchronization fixture verifies audio/timecode alignment and actual drift-corrected camera use. These are bounded acceptance fixtures, not a universal synchronization guarantee.
