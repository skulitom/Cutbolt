# Subject reframing

`reframe.inspect` selects an explicit source window around user-authored boxes or an optionally tracked patch. It returns every subject/crop decision and a new editable scene, without writing media or saved state. Compile that scene with `scene.render`, then use the resulting asset in the existing saved-session workflow. The same inspection is available as `cutbolt_reframe_inspect` over MCP stdio. No external model, network service or automatic subject recognition is involved.

The complete acceptance suite verifies G05 basic and extended within this contract. The [generated progress tracker](PROGRESS.md) remains authoritative.

## Input and coordinates

The input is a normal [scene](SCENES.md) with one image layer spanning its entire duration from time zero. Reframing operates on that layer's full untransformed source canvas. Source canvases are 1–512 pixels in each dimension, and this operation accepts 1–128 exact 25 fps output frames. Nonzero frame anchors, authored position/position animation, scale, rotation, partial legacy crop and an existing spatial transform reject explicitly. Source image offsets, holds, loop/hold-last behavior, straight/premultiplied alpha, masks, effects, opacity animation, blend mode and scene audio retain their original semantics.

The output has a constant integer source `window: [width,height]` and a declared `output_size: [width,height]` on the logical scene canvas. The window must fit the source. Both output dimensions are 1–512 and their aspect ratio must equal the source window's exactly; resizing is isotropic. Existing `scene.output_scale` and scene memory/output-size limits remain in force. A different zoom requires another explicitly inspected window size. This operation does not estimate scale, rotation or an automatically changing zoom.

`padding` is 0–4096 integer source pixels on every side of the selected box. The complete padded box must fit inside both source and crop. `maximum_step: [x,y]` declares each axis's maximum crop-origin movement per output frame, with each value 1–4096. It is an axis limit, not a bound on Euclidean velocity or acceleration. `smoothing_radius` is 0–16 frames; zero follows the unsmoothed desired center subject to retention and motion constraints. `sampling` is `nearest` or `bilinear`.

All selected boxes and crop origins use integer source-pixel corners. Time is exact rational seconds. The crop rectangle describes the source geometry of the output pixel centers. As in the existing [spatial sampler](SPATIAL.md), bilinear reconstruction uses neighbouring source taps, including neighbours just outside an interior crop window. Taps beyond the outer source canvas are transparent. This is not strict exclusion of every filter-support pixel outside the declared window and does not fill transparent borders with invented opaque pixels. Alpha is preserved through resizing before compositing over the scene background.

## Subject segments

Supply 1–16 segments. Each has an exact frame-aligned `start`, a 1–64-character ASCII `subject_id` using letters, digits, `_` or `-`, `cut`, and `selection`. Starts are sorted and must be unique; the first is zero with `cut: true`. Each segment ends at the next start or scene end, so all frames are covered. Input ordering does not change the result.

`cut: true` resets crop smoothing and movement limits at that boundary, allowing an explicit framing jump. `cut: false` changes the selected subject while preserving one continuous crop path across the boundary. If the chosen subjects cannot both be retained under that path's speed limit, the entire request fails. A cut is a declared editing decision, not automatic scene-cut detection. No frames or soundtrack intervals are removed by this inspection.

A manual selection has segment-local keys:

```json
{
  "start": {"num":0,"den":1},
  "subject_id": "speaker",
  "cut": true,
  "selection": {
    "mode": "manual",
    "keys": [
      {"time":{"num":0,"den":1},"rect":[16,20,6,5],"interpolation":"linear"},
      {"time":{"num":15,"den":25},"rect":[46,26,11,9],"interpolation":"hold"}
    ]
  }
}
```

Every manual segment requires 1–128 keys, including time zero. The four rectangle components use the existing exact [scene animation](SCENES.md) interpolation modes and signed rounding; dimensions remain positive. Duplicate times, unsupported precision, keys beyond the segment, invalid authored rectangles and invalid sampled/padded rectangles reject. Boxes may change size, but the crop window stays fixed. One-frame manual segments are valid.

Optional local analysis instead uses `selection.mode: "track"`, a reference `focus` rectangle, an initial `region` patch inside that focus, and `controls`:

```json
{
  "mode": "track",
  "focus": [6,19,16,14],
  "region": [8,21,12,10],
  "controls": {
    "search_radius": 4,
    "maximum_step": 4,
    "maximum_acceleration": 4,
    "minimum_correlation_milli": 900,
    "minimum_margin_milli": 40,
    "maximum_frame_change_milli": 200
  }
}
```

These controls follow [patch tracking](TRACKING.md). At least two frames are required for a tracked segment. Each segment starts a fresh reference patch; it translates that segment's fixed focus box using the measured integer displacement. Measurement uses original source pixels before authored effects, masks and output geometry. Original frame holds, sampling policy and offsets determine the analyzed source frame. Total tracking work across all segments is bounded to 64 million patch-pixel comparisons, in addition to the tracker's own limits.

Confidence measures patch similarity and uniqueness within the declared search. It is not a probability of semantic subject identity. Low texture, ambiguous matches, excessive motion, occlusion and declared frame-change violations reject without a partial plan. There is no automatic relocking or recovery. To correct a measured path, turn the reported focus boxes into manual keys at their reported segment-local times, edit the intended boxes and inspect again using the original source scene. Keep that source recipe and the chosen selection alongside compiled media.

## Crop decisions

For each axis, the desired crop origin centers the padded focus box in the selected window. A truncated triangular filter averages those half-pixel targets within each explicit cut group. It rounds once to the nearest integer, with half ties away from zero.

The engine first establishes all positions that keep the padded subject inside the crop and the crop inside the source. It then works backwards to preserve the possibility of reaching every later frame under the pan limit. The forward pass chooses the closest current smoothed target that still has such a continuation. This can begin moving before a later subject change. It is a deterministic feasible path, not a claim of globally optimal cinematic motion. The axes are solved separately; smoothing may be overridden to retain the subject and satisfy the declared motion bounds.

The result includes:

- Exact global and segment-local frame times, subject/segment IDs and explicit cut flags.
- The selected original source-frame index, selection method, focus and padded focus boxes.
- Desired half-pixel origin, rounded smoothed origin, feasible intervals, actual crop rectangle, per-axis movement and constraint adjustment.
- Correlation, uniqueness margin and frame-change measurements for tracked frames; `null` for authored boxes.
- Exact resize ratio, sampling/edge policy, aggregate analysis work, input/result scene inspection and the new scene. `applied` is always false.

The replacement scene contains one frame reference per output frame, held for exactly 1/25 second. Image identities and offsets come from the originally sampled frame; anchors record the selected crop origin. The spatial fit uses the explicit window size while the legacy source crop remains the full canvas. Masks/effects and original soundtrack clocks are retained. All declared original sources, including unused frame references, are checked again after analysis; the returned scene is validated before success. Rendering performs its own existing identity and no-overwrite checks.

## Use and verification

Generate the original acceptance fixture outside the repository:

```powershell
python -X utf8 tests/reframing.py --output C:\DEV\CutboltData\new-reframe-check
.\target\debug\cutbolt.exe C:\DEV\CutboltData\new-reframe-check\local-tracked-subject-request.json
```

The second command reports the complete proposed scene and decisions. Send `result.scene` to `scene.render` with the fixture's `sources` as input root, an explicit output root and a new output filename. This blocking compile is CLI/library work. Register its asset through `media.add`; saved cuts, retries, undo/restoration and previews use the ordinary session contract. Templates retain the explicit fitting-window metadata.

The fixture uses original generated images and PCM. An independent enumerated graph checks feasible crop positions and motion; exact source clocks and high-precision forward geometry/alpha filtering check decoded output. It covers authored paths and corrections, subject cuts/continuous pans, smoothing tradeoffs, local tracking/failures, source timing/offsets, alpha, masks/effects, sound, saved workflows, limits and publication preservation. The complete frozen-source verifier passes 340 unique checks and earns G05 basic/extended. No dependency or acceptance-scope change was required.

The retained `C:\DEV\CutboltData\reframing-20261003-03` fixture and full verifier compare **513 decoded frames, 984,960 stereo sample frames and four previews**, with **70 rejection/failure cases**. Every observed RGB/PCM difference is zero; the declared RGB tolerance remains one unit. Independent enumerated crop graphs, authored subject trajectories, exact source clocks and high-precision forward geometry/alpha filtering verify complete output. Subject cuts and continuous changes, changing boxes, all easing modes, source offsets/holds, alpha, masks/effects, templates, saved retries/undo/restoration, typed MCP and preview ranges pass. Jitter-following motion falls from **186 to two pixels of total pan** while retaining the full padded subject. Known local tracks are exact, with minimum correlation **1.0** and uniqueness margin **0.769574**; unreliable observations reject explicitly.

The maximum fixture actually renders **128 frames and 16 subject segments** from a **512 × 512** source canvas. Full-run inspection uses **13,488,128 bytes peak working set** and **0.078 seconds**, within the 128 MiB/30-second bounds. The aggregate tracking-work gate, invalid requests, changed sources, output collisions and publication cleanup pass. The fitting-window regression separately compares 650 frames and 1,248,000 stereo sample frames across all three fitting modes, with explicit pixel aspect and edge sampling; it adds no extra spatial checkpoint.

Errors include `INVALID_REFRAME` for this contract, `REFRAME_INFEASIBLE` for an impossible retention/motion path, `REFRAME_UNAVAILABLE` for transparent source-end frames, existing `TRACKING_UNRELIABLE`/`INVALID_TRACKING` for measurement failures, and the existing detailed scene, time, animation, identity and output errors. Unknown fields and variants fail as invalid JSON. No inspection error publishes a replacement recipe, media file or session edit.
