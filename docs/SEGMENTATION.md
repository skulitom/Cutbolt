# Local annotated foreground masks

The optional `tools/segmentation.py` worker produces original versioned binary masks through the public OpenCV GrabCut API. The Rust scene renderer consumes the resulting ordinary PNG masks without Python or OpenCV. The integrated run passes all seven foreground-mask checks, awarding both X04 checkpoints.

## External runtime and invocation

Use an external Windows CPython 3.12.14 environment with `opencv-python-headless==4.13.0.92` and `numpy==2.2.6`. The selected versions/licenses are in [DEPENDENCIES.md](DEPENDENCIES.md). Setup is explicit; the worker never installs packages or fetches models. No service or listening port is introduced. Invoke the external Python with `-I tools/segmentation.py`, passing one JSON request on stdin or one request-file path. Output is one `{"ok":true,"result":...}` response, or a typed error with exit status 1. Requests are at most 4 MiB; duplicate and unknown fields reject.

All commands take an existing absolute `input_root`. Sources, documents and masks use the existing relative `path`, lowercase `sha256`, and `bytes` identity shape. Paths cannot escape the root through traversal, alternate streams or resolved links. `segment` and `correct` also take `output_root` and an absolute `output` naming a **new directory** inside both roots. That directory holds `document.json` and numbered masks. Existing files/directories are never replaced. A sibling scratch directory holds only worker-owned outputs; source identities are rechecked before Windows atomic rename publishes the new bundle. Ordinary errors remove only known owned files. A forcibly terminated worker can leave its scratch directory; there is no durable background job or automatic retry for this tool.

## Annotation recipe

`segment` accepts `recipe` with these fields:

```json
{
  "schema_version": 1,
  "id": "subject",
  "automation": "annotated_frames",
  "iterations": 4,
  "seed": 819,
  "frames": [{
    "source": {"path":"sources/frame.png","sha256":"<actual lowercase digest>","bytes":1234},
    "hold": {"num":1,"den":25},
    "annotations": {
      "region": [10,10,60,40],
      "foreground": [[30,25,5,5]],
      "background": [[40,30,4,4]]
    }
  }]
}
```

Each rectangle is `[x,y,width,height]` in source pixels. The region begins as probable foreground; outside is definite background. Foreground/background rectangles supply hard constraints. Foreground must be inside the region, and contradictory hard constraints reject. At least five definite foreground and five definite background pixels are required. Background rectangles can remove a hole or unwanted area inside the region. Every frame requires its own explicit annotations; no bounding box, object identity or motion is inferred. Rectangle annotations remain editable and can be as small as a pixel when combined with enough definite samples.

Inputs are plain or sRGB opaque RGB8 PNGs up to 512×512, with equal sequence dimensions. Alpha, other metadata, animation and other image encodings reject in this worker. The renderer has its separate broader PNG contract. There are 1–64 annotated frames, at most 4,194,304 total source pixels, at most 256 rectangles per hard class per frame, 1–10 iterations, and an integer seed in 0–2,147,483,647. Positive exact holds must align with 25 fps; total duration is at most ten seconds. These are binary foreground masks, without soft alpha matting or synthesized intermediate images. `automation` values requesting tracking or automatic object selection return `UNSUPPORTED_AUTOMATION`.

## Revisions, fitted models and provenance

The first immutable document has schema version 1 and revision 1. It retains the complete recipe, each mask identity, foreground pixel count, producer identities and the two fitted 65-value foreground/background GMM arrays returned by the selected public API. Each model digest hashes the 130 values as little-endian IEEE-754 float64, background first. These are actual models fitted to the supplied pixels and constraints; no pretrained weights are used. Seed for frame index i is `(seed+i) modulo 2^31`, with fresh zero-initialized models, one CPU thread and OpenCL disabled.

Provenance records exact package versions and installed-file inventory digests, the helper and Python executable/base-runtime hashes, Python/OpenCV versions and execution settings. Third-party packages and notices stay external. This is content identity and reproducibility evidence, not a signed attestation or a claim that any user-authored JSON is trustworthy.

`inspect` takes `document` identity, rechecks source/mask identities, dimensions, constraints and model-array digests, and reports whether the current producer identity matches the recorded one. It does not traverse all earlier revisions. `correct` takes `document`, `expected_revision` and a nonempty `corrections` array of `{"frame":0,"annotations":{...}}`. Each listed frame receives a complete replacement annotation set. The worker recomputes the sequence into a new bundle, increments the revision and records the prior document identity under `parent`. Sources/settings remain fixed. A stale revision returns `REVISION_CONFLICT`; duplicate frame edits reject. These are immutable snapshot revisions, not a shared authoritative mask database. Preserve the parent bundles if their history is needed.

Fresh processes execute with Python socket operations rejected by an audit hook. Tests also use separate empty user/cache directories and offline settings, and verify that no cache/model files appear. This is the exercised Python network boundary; it is not an operating-system sandbox for arbitrary native extensions. The selected operation uses only explicit local source arrays and has no model-download step.

## Render integration

`scene` takes `document`, `scene_id` and RGB-byte `background`, returning an editable ordinary scene. Every frame references its original image plus a `matte` identity, retaining its exact hold. Feed this scene to existing `scene.inspect` and `scene.render` with the same input root. Keep the document/scene recipes for future corrections and compile each revision to a new output. Compiled assets use ordinary saved edits, retries, undo and previews. The helper is a local CLI tool, not an additional MCP tool; typed MCP scene inspection accepts its results.

Native `scene.layers[].frames[].matte` accepts a same-size opaque black/white RGB or RGBA PNG. White preserves the source RGBA pixel; black zeros all four channels. This happens before effects, spatial sampling, geometric projection and layer composition, and follows the source's held frame under temporal sampling. It works with both declared source alpha modes. Rectangular masks continue to apply separately. Image/mask identity conflicts reject; source bytes and decoded plus derived pixels count against the existing 256 MiB and 16,000,000-pixel scene budgets. A shared source with different mattes retains separate derived images. Inspection reports the `binary-source-matte-v1` profile and number of distinct derived pairs. Existing tracking analysis remains explicitly in original source-canvas space before effects/masks; it does not infer foreground motion from these masks.

## Acceptance

`tests/segmentation.py` authors textured moving silhouettes with holes, thin features and an excluded foreground-colored distractor. Ground-truth masks come from original analytic shape equations, independently of the segmentation library. Fixed gates require IoU ≥0.98, exact-boundary F1 ≥0.95 and known-motion-compensated disagreement ≤0.005. These tests do not claim natural-footage quality or automatic temporal propagation. Separate fresh workers must reproduce mask bytes and fitted model arrays. A deliberately incorrect hard annotation must create an observable error, and a revision must correct it while preserving its parent.

The fixture also compares complete decoded native raster, crop, temporal and 3D outputs, saved edits, retries, undo and previews; checks source preservation and typed invalid requests; and actually processes 64 frames totaling 4,194,304 pixels within a fixed 120-second gate. The 12-frame normal workload has a separate 90-second gate. Dependencies, generated media, masks, models, logs and private research stay outside Git.

Public interface references: [OpenCV 4.13 GrabCut guide](https://docs.opencv.org/4.13.0/d8/d83/tutorial_py_grabcut.html), [segmentation API](https://docs.opencv.org/4.13.0/d7/d1b/group__imgproc__misc.html). Original project code defines the document, bounds, provenance, correction and publication behavior; no external implementation is copied.

Full verification on 4 October 2026 passes all 12 authored silhouette comparisons with IoU and boundary F1 1.0 and zero motion-compensated disagreement. It also passes 64 complete rendered frames, 122,880 stereo sample frames, four previews and 26 rejection cases. The actual maximum workload completes in 2.156 seconds within its 120-second gate. These results retain the bounded synthetic-fixture scope above.
