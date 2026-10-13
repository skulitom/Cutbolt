# Generated scene clips

A native track clip can save a scene as its source. Its pictures are composed per
frame in the timeline pass, with no intermediate scene movie. Place the same recipe
on an audio track to include its soundtrack. Scene PCM is mixed through the existing
clip gain, fades, transitions, track and master audio rules.

After adding a video track named `titles`, place an authored scene with the existing
editing command:

```json
{"op":"tracks.edit","edit":{"op":"place","track_id":"titles","collision":"reject","clip":{"id":"title","scene":{"file":"scenes/title.json"},"start":0,"source_in":0,"duration":2}}}
```

The `file` form uses the ordinary workspace document loader. The file contains a
full [scene recipe](SCENES.md); `select` can choose a scene inside a saved result.
An inline recipe is accepted in the same `scene` slot. A document is resolved before
the edit and the resulting recipe is saved in the snapshot. Later changes to that
authoring file affect a new edit, while existing revisions retain their content.
PNG, WAV, LUT and font inputs are content-pinned. Workspace requests resolve omitted
identities and installed font names before saving; library callers supply pinned
identities. Relative media paths stay under the input root, with the existing
installed-font exception limited to font fields.

Exactly one of `asset_id`, `sequence_id` or `scene` is required. The scene's enlarged
output dimensions and frame rate must match the original project canvas and clock.
`source_in` and duration select its source window, including transition handles.
Trims, splits and slips retain that clock. Scene-layer animation keeps its existing
layer-local meaning inside the scene. Saved clip effects and spatial properties
then process the generated picture using the clip's source clock.

Transparent scenes retain straight alpha and reveal lower tracks, including on an
ordinary video track. Child sequences use the same source. Frame/range previews,
queued renders and delivery use the same frame stream. Proxy previews generate a
reduced scene picture without a separate proxy file: an opaque block uses the floor
of its channel mean; transparent blocks use alpha-weighted color and mean alpha.
Final exports use the original canvas. Existing `alpha_over` track restrictions
remain separate from ordinary-track generated clips.

Adding a generated source upgrades the new revision to schema 2. Session receipts
include the saved scene; retries, undo and older revisions retain the existing
transaction rules. A scene can be replaced by removing and placing its clip in one
atomic batch. Interchange analysis reports an explicit unsupported generated-scene
loss until it has been rendered to a media asset.

Scenes retain their own duration, layer, source-pixel and composited-pixel caps.
Repeated identical recipes share one prepared source across video, audio, nested
branches and render chunks. The complete export caps retained scene payload at
1 GiB, charged for decoded images, PCM, LUT tables and sampled/recipe data; this is
separate from the native compositor's frame-buffer bound. Render plans report
`scene_resources` with the shared recipe count and payload charge. Uniform cached
color/matte planes retain one RGB value each. A silent soundtrack retains its exact
sample count and produces zero PCM in bounded blocks; even a silent source file
keeps its identity checks. These are storage optimizations: source clocks, pixels,
samples, cached-layer work accounting and the 1 GiB bound remain unchanged.
The charge measures retained prepared payload, not total process memory or temporary
preparation/frame buffers. Scene audio windows use owned temporary WAV inputs,
cleaned up with other generated audio.
No generated video file is written.

Every external scene dependency is fully hashed before publication. `timeline.check`
also reports changed or missing scene inputs when an input root is supplied. Cache hits
also check dependencies throughout the saved sequence catalog, including unused
definitions. A changed source cannot silently reuse a cached frame.

The generated-clip fixture compares native video and exact PCM with separately
compiled scene assets, including source windows, transparent layering, child
sequences, previews, proxies, queued export, document loading, saved-history
recovery, multiple render chunks and changed cache dependencies. Standalone scene
rendering remains available. [Graphics placement](TEMPLATES.md#place-a-graphic-in-one-job)
now saves a generated clip in one edit, with no movie; actual older compiled
receipts retain their replay behavior. `captions.render` with `mode: "generated"`
writes revision-checked scene operations to a JSON file, using the same bounded
caption windows and trimming the final placed clip to exact coverage; see
[CAPTIONS](CAPTIONS.md#generated-captions-without-a-compiled-movie). Production-title
migration and the complete stage-5 quick gate remain open. The caption and talking-head
skills now use generated scene operations, with Windows/Linux schema-validated examples
and a fresh-model rerun meeting 15 calls and 33,441 peak input tokens; see
[workflow evidence](../verification/generated-caption-skills-2026-10-13.json).
