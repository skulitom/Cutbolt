---
name: cutbolt
description: "Set up, revise and recover general Cutbolt timelines, and route specialized edits to their workflow skills. For a complete talking-head cut with captions and delivery, use recut-talking-head directly."
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

Inspect the source before choosing the project clock and canvas. $SOURCE is a workspace
path; use the returned size/rate for $WIDTH, $HEIGHT and $FRAME_RATE unless the user requested
another delivery. A mismatch requires preparation before editing.

<!-- example: inspect -->
~~~json
{"command":"media.inspect","path":"$SOURCE"}
~~~

When timeline.ready is true and the size/rate fit, use timeline.asset as $ASSET and its
id/duration as $ASSET_ID/$DURATION. If asset.identity is absent, add the inspection identity's
bytes and sha256 (not its path). Preserve the returned decode binding. For preparation-required
sources, queue the returned media.prepare proposal and wait for the ready asset; do not change
metadata to label an unsupported original ready. $TRANSFER declares the working RGB transfer,
for example bt709, from the media/project policy.

For a new project, create its initial linked picture/sound arrangement in one validated
snapshot. $PROJECT_ID is a new stable name. The clip and link IDs below are new IDs chosen
here; when revising an existing project, read and use its actual IDs instead.

<!-- example: create -->
~~~json
{"command":"session.create","request_id":"create","project":{"schema_version":1,"id":"$PROJECT_ID","revision":0,"width":"$WIDTH","height":"$HEIGHT","frame_rate":"$FRAME_RATE","transfer":"$TRANSFER","assets":["$ASSET"],"clips":[],"tracks":{"duration":"$DURATION","tracks":[{"id":"v","kind":"video","enabled":true,"locked":false,"clips":[{"id":"shot","asset_id":"$ASSET_ID","start":0,"source_in":0,"duration":"$DURATION"}]},{"id":"a","kind":"audio","enabled":true,"locked":false,"clips":[{"id":"shot-audio","asset_id":"$ASSET_ID","start":0,"source_in":0,"duration":"$DURATION"}]}],"links":[{"id":"shot-link","members":[{"clip_id":"shot","start":0,"source_in":0},{"clip_id":"shot-audio","start":0,"source_in":0}]}]}}}
~~~

Keep the returned revision as $REVISION (zero for a new session). The creation receipt records
its initial assets and tracks. Subsequent changes use session.apply with that revision and a
new request ID; retry an uncertain creation with identical arguments and the same request ID.

## Choose the next recipe

- Talking head through captions and delivery: recut-talking-head is self-contained; load it
  directly for that request. Its setup is only for a new project, so retain an existing head.
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
