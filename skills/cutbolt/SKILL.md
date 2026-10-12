---
name: cutbolt
description: "Edit existing local video or audio with Cutbolt: trim footage, tighten speech, add captions, switch cameras, adjust loudness, revise saved edits and deliver with review."
---

# Edit existing footage

Use Cutbolt's local MCP tools for saved, reversible edits. The engine needs an installed
cutbolt executable and external FFmpeg/ffprobe; cutbolt doctor checks that setup.
Keep footage and generated files in the chosen workspace. Outputs must use new names.
Files outside it need an explicitly chosen input root; never overwrite an original.

For a new animated video from a brief, an HTML graphics tool may be a better authoring
choice. Cutbolt can finish its rendered graphics over footage. For a title on footage,
use an original built-in template with graphics.place.

## Calls and results

The plugin starts local stdio MCP in the current project directory. A command such as
session.apply uses the listed cutbolt_session_apply tool without the command key.
When its direct tool is absent, use cutbolt_run with command and arguments.
Queue blocking work with job.start, then job.wait. Read the structured result.
For a large proposal, save_as writes it to a new JSON file and replaces the response with
a file receipt. Read that local file to review it before applying a file/select reference;
it does not also return the proposal or job status. This adds a file-reading call.
Prefer these recipes to schema discovery; request a schema only for a genuinely unknown operation.

Examples below and in the companion skills are engine requests. Replace uppercase
$PLACEHOLDERS with actual input or result values before calling. They are documentation
slots, not engine syntax. Use fresh request IDs for distinct edits; keep the same ID and
identical arguments when retrying an uncertain edit.

## Start a cut

Choose dimensions and a native frame rate matching the intended delivery. Compare inspected
source dimensions/rate with that project. $PROJECT_ID is a new stable project name; $SOURCE is a workspace
path. $WIDTH, $HEIGHT and $FRAME_RATE describe the project.

<!-- example: create -->
~~~json
{"command":"session.create","id":"$PROJECT_ID","width":"$WIDTH","height":"$HEIGHT","frame_rate":"$FRAME_RATE","request_id":"create"}
~~~

<!-- example: inspect -->
~~~json
{"command":"media.inspect","path":"$SOURCE"}
~~~

If timeline.ready is true and the size/rate fit, take timeline.asset as $ASSET and its
id/duration as $ASSET_ID/$DURATION. If asset.identity is absent, bind its identity using
the bytes and sha256 fields of the inspection's identity (not that object's path field).
Eligible indexed originals remain original files. If preparation is required, follow the
returned proposal through job.start run media.prepare and wait for its ready asset before
editing. Do not label unsupported or mismatched footage ready by changing its metadata.

$REVISION is the latest saved revision (zero just after creation). $TRANSFER declares
the working RGB transfer (for example bt709), based on the media/project policy.
This edit imports and appends the asset, then creates linked picture and sound tracks.

<!-- example: append -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"append-source","operations":[{"op":"media.add","asset":"$ASSET"},{"op":"project.transfer","transfer":"$TRANSFER"},{"op":"clip.append","clip":{"id":"shot","asset_id":"$ASSET_ID","source_in":0,"duration":"$DURATION"}},{"op":"tracks.edit","edit":{"op":"promote","video_track_id":"v","audio_track_id":"a"}}]}
~~~

Keep the returned revision. The promoted clip/link IDs come from the saved project;
do not infer them from track names. Additional sources require inspection and media.add.

## Choose the next recipe

- Tighten speech: recut-talking-head, then captions, then deliver-and-review.
- Screen capture: screen-tutorial, then deliver-and-review.
- Two synchronized cameras: interview-multicam, then deliver-and-review.
- Existing captions: captions; retain the same project and current revision.
- Feedback revision: session.get, make the smallest targeted session.apply, then preview
  the changed window. session.undo is another revision-checked edit, not a history deletion.

Load only the recipe needed for the request. The plugin exposes these as namespaced skills,
for example cutbolt:recut-talking-head.

## Recovery and boundaries

Use error retry, path, hint and detail; do not parse prose messages. For a stale revision,
read session.get, reconsider the edit against the new head, then submit the revised request.
For an uncertain saved edit, session.receipt with its request_id tells whether it committed.
For graphics.place use the inner edit request_id, not the enclosing job ID.
For an existing output, choose a new output name. A failed/cancelled job is not completion;
inspect its error before starting a replacement.

Windows and Linux support these editing, queue and speech workflows. Speech needs a separately
configured local runtime/model; a supplied transcript avoids recognition. Recording and
hardware decode currently require Windows. Do not promise those backends on Linux.
This plugin installs no models, fonts, media tools or engine binary, and starts no network server.
