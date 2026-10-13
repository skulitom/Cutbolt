# Saved clip effects and properties

Native video clips, including child-sequence instances, accept an `effects` array of
at most eight pixel effects and spatial properties: `grade`, `selective_grade`,
`chroma_key`, `lut`, `transform`, `opacity`, `mask` and `blend_mode`. These reuse the scene effect types,
validation and pixel equations. They render directly from decoded clip frames, with
no PNG sequence or scene compilation required by the caller. The PNG scenes in the
verification fixture are comparison oracles only.

Replace or clear a stack through `timeline.apply` or `session.apply`:

```json
{
  "op": "tracks.edit",
  "edit": {
    "op": "clip_effects",
    "clip_id": "presenter",
    "effects": [{
      "kind": "grade",
      "exposure_milli": 500,
      "contrast_milli": 1000,
      "white_balance_milli": [1000, 1000, 1000]
    }]
  }
}
```

An empty `effects` array clears the stack. The field is also accepted on a newly
placed clip. Use `sequence.edit` with the same `clip_effects` edit to change a clip
inside a child definition. Audio clips reject nonempty stacks with
`UNSUPPORTED_TIMELINE`, naming the clip. Track locks and transitive parent-reference
locks protect definition edits. Effects are independent of linked audio clips.

## Clock, pixels and bounds

Curve key times are on the clip's **source clock**, as with `gain_curve`.
The first visible frame evaluates at `source_in`; a later frame evaluates at
`source_in + timeline_time - clip.start`. Trimming, slipping and splitting preserve
the stack without shifting its keys. Keys may extend through the complete asset
or child-sequence duration, including source handles used by transitions. Each
sequence instance processes the child's already composited RGB8 output using that
child's source clock; the child clips keep their own source clocks. Child gaps remain
opaque black before instance effects. Curves are prepared
once for each decoder and sampled per frame; memory does not grow with source duration.
A transition spanning split fragments requires the same effect and placement recipe
through each endpoint span. A conflicting edit fails atomically with
`INVALID_TRANSITION`. To change those fragments together, remove the transition,
edit each fragment and restore the transition within one atomic batch; every
intermediate operation must still describe a valid timeline.

Processing happens before transition mixing and before the existing overlay
crop, shrink, opacity and placement operations. Correction masks use the source
canvas, including when the decoder supplies only a crop. Proxy previews process
the selected proxy's pixels with the same source clock. A proxy pixel `(x,y)` with
divisor `s` samples correction masks at source pixel
`(s*x + floor(s/2), s*y + floor(s/2))`, the nearest source pixel to the block center
with ties toward increasing coordinates. Final exports use originals.

The pixel equations are the existing scene equations: grades use linear sRGB,
qualifiers and keys use encoded sRGB, and LUTs operate on encoded RGB in list
order. This stage retains the current RGB8 path. Set `project.transfer` to `srgb`
to decode originals into the same declared working transfer as scenes. A LUT uses
`{"kind":"lut","file":{"path":"look.cube","sha256":"…","bytes":123},"interpolation":"linear"}`;
its path is relative to `input_root`. See [LUTs](LUTS_SCOPES.md) for accepted tables and
interpolation methods. Each file is bounded to 8 MiB; all stacks and decoder
branches and render chunks share one 128 MiB retained-table budget for the complete export. Repeated identical declarations
reuse the loaded table. Conflicting identities for the same path fail.

The ordered native compositor retains the key's resulting straight alpha. A keyed
clip on an opaque or `alpha_over` video track reveals the active tracks below it;
black lies below the lowest track. Pixel effects run before placement and blending.
Legacy overlay opacity still rounds alpha to a byte before blending. Its opaque
box shrink remains the floor of the channel mean; when a key introduces alpha,
the box uses alpha-weighted color and the floor of mean alpha.

Saved spatial/opacity/mask/blend properties use the same layer equations. Child sequences
use the same ordered compositor and shared LUT resources as the root, including
direct compressed pictures and effects at each nesting level.

## Transform, opacity, mask and blend

Each structural property may appear once. Pixel effects retain their list order;
structural entries have fixed phases regardless of list position: process source
pixels, apply source-mask coverage, map the source, apply opacity, then blend over
lower active tracks. These declarations work on ordinary video tracks and sequence
instances, with no `alpha_over` track or compiled scene required.

```json
[
  {"kind":"transform","value":{"position":[640,360],"crop":[0,0,1920,1080],"scale":1,"quarter_turns":0,"opacity":255,"spatial":{"translate_milli":[0,0],"scale_milli":[500,500],"rotation_mdeg":0,"flip":[false,false],"pixel_aspect":{"num":1,"den":1},"sampling":"bilinear","edge":"transparent"}},"anchor":[0,0]},
  {"kind":"opacity","value":224},
  {"kind":"mask","mask":{"rect":[0,0,1920,1080],"feather":{"radius":12,"edge":"inner"}}},
  {"kind":"blend_mode","mode":"normal"}
]
```

`transform.value` is the existing scene transform. `transform.animation` accepts
position/opacity curves; `value.spatial.animation` accepts subpixel position,
scale and rotation curves. Every curve uses clip source time. The optional
`anchor` is a source-canvas pivot, default `[0,0]`; cropping does not move it.
`opacity` takes `value` (0–255) and an optional `curve`. A separate opacity entry
requires transform opacity 255 and no transform opacity curve, preventing two
owners. `mask.mask` is the existing scene rectangle mask, with feather, inversion
and source-clock animation. Blend modes are `normal`, `multiply` and `screen` in
encoded RGB. Scene coverage combines alpha, opacity and mask before final rounding.

Existing `OverlayTransform` remains valid on `alpha_over` clips. It is normalized
to a transform with crop-origin anchor and `sampling:{"filter":"box_floor",
"divisor":N}`, plus `opacity` with `rounding:"byte_alpha"`. The box filter uses
associated-color means and floor-of-mean alpha; byte alpha rounds opacity into
alpha before blending. This preserves existing output exactly. Box sampling
requires divisible crop dimensions, scale 1, no rotation or spatial mapping,
and an anchor at the crop origin. A source mask weights the box's input pixels
before averaging. Legacy placement already owns transform and opacity; duplicate
explicit owners are rejected. The eight-entry cap applies to user entries.

Coordinates remain on the original source canvas in proxy previews. After pixel
processing, proxy blocks are expanded for property mapping and output block
centers are selected with ties upward. This preserves crop, pivot and mask units.
The renderer charges the temporary original-size RGB/RGBA buffers against the
same 1 GiB frame-memory bound. Full-quality exports always use original media.
Existing `alpha_over` track restrictions on proxy selection, transitions and
sequence instances remain; the new properties on ordinary tracks cover those
workflows. The former nested video graphs and private A/B selector have been removed
after both paths passed their shared Windows/Linux fixtures.
[Removal evidence](../verification/native-only-timeline-2026-10-13.json) records
the comparisons and subsequent native-only checks.

## Snapshots, receipts and verification

The first edit that adds a nonempty stack upgrades the new snapshot to
`schema_version: 2`. Version-1 snapshots with no effects retain their original
representation; prior saved revisions are never rewritten. Clearing effects keeps
version 2. Undo or restore can recover a version-1 snapshot. Old engines must not
be used to edit version-2 snapshots.

Session diffs include placement `effects` and a `schema_version` change when it
changes. Definition edits appear in sequence diffs with affected root instances.
Effects in an unused definition also require schema 2. Atomic batches, request replay,
undo and restore use the existing store
contract. Interchange analysis reports effects as a hard unsupported loss instead
of silently discarding them.

Frame and range previews, contact-sheet/scope frame readers, reference rendering
and queued/delivery exports share the timeline compositor. LUT files appear in
render `sources` with zero media frames/samples, and receive a full content hash
check before output publication. Cache hits also verify LUT identity and cannot
reuse an old preview after its table changes. Cache keys validate LUTs throughout
the saved sequence catalog, including unused definitions. Full compositor plans report table
resources and each decoder's effect recipe and starting source clock. Chunk summaries
include the shared table count, allocated bytes and maximum bytes under
`effect_resources`; their internal graphs remain omitted.

`tests/clip_effects.py` compares FFV1 and direct H.264 effects against scene renders
of the same extracted source frames, with zero RGB tolerance. It also checks
source-clock slips/splits, both transition inputs, cropped overlays, previews,
proxies, range and queued exports, final H.264 output, and LUT cache dependencies.
A 65-cut case checks one shared LUT allocation across render chunks and exact reference/range-delivery output. Multilevel child/instance effects are compared against separately rendered scene
boundaries to preserve each RGB8 rounding step, with trimmed/split clocks, frame/range
previews and rejection of changed nested LUTs before rendering or cache reuse.
The fixture's direct-source reference uses the existing explicit conform path.
`tests/native_layers.py` adds lower-track key coverage against multi-layer scenes and
an independent integer alpha equation, exact source-window occlusion and direct
child-sequence pictures. Its `clip_properties.py` cases compare saved transforms,
animated opacity, feathered masks and blend modes with scene oracles on reference
and direct footage, plus independent integer blends and exact overlay sugar.
Linear-light modes remain separate work.
