---
name: recut-talking-head
description: "Edit a talking-head video from original footage through pause and filler cuts, captions, H.264 export and review, or tighten an existing saved cut. This complete workflow needs no other Cutbolt skill."
---

# Edit and deliver a talking head

This self-contained recipe covers a new cut through reviewed delivery. For an existing saved
project, use session.get to obtain its head and track IDs, then skip initial setup. Perform
only the stages requested by the user; captions and delivery are optional when revising timing.
Sources and outputs stay in the chosen workspace, and every output uses a new name.

Use the listed cutbolt_* tool for a command when present (omit command from its arguments);
otherwise use cutbolt_run with command and arguments. Replace $PLACEHOLDERS with actual
values. Requests below use engine syntax. Read structured results and keep every returned
revision. Distinct edits need distinct request IDs; uncertain retries reuse the same arguments
and ID. Queue blocking work with job.start and continue only after job.wait reports completed.

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

## Use a transcript

Use a source-bound word transcript, referenced as transcript.json below. If none is supplied,
first queue media.transcribe with the configured local runtime, wait and save the transcript.
Recognition is approximate; review suspicious words and uncovered sounds before cutting.
The voice track in this initial arrangement is a; substitute the actual ID for an existing cut.

## Propose pauses and apply once

audio.tighten is read-only. Read the cut summary before applying: the user's pacing and
meaning decide whether a pause belongs. Keep the returned operations as $PAUSE_OPERATIONS.

<!-- example: pauses -->
~~~json
{"command":"audio.tighten","project":{"project_id":"$PROJECT_ID"},"voice_track_id":"a"}
~~~

Skip the edit if operations is empty. For an accepted proposal, apply the proposed operations
at the revision on which it was computed. Pass the returned array unchanged.

<!-- example: apply-pauses -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"apply-pauses","operations":"$PAUSE_OPERATIONS"}
~~~

## Propose fillers on the new head

<!-- example: fillers -->
~~~json
{"command":"transcript.fillers","project":{"project_id":"$PROJECT_ID"},"transcripts":[{"file":"transcript.json"}]}
~~~

The proposal maps source words into the edited timeline. Check that removals are actual
fillers in context; do not cut meaningful repetitions just to reach a target duration.
Keep operations as $FILLER_OPERATIONS. Update $REVISION from the preceding edit, then apply
an accepted nonempty proposal.

<!-- example: apply-fillers -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"apply-fillers","operations":"$FILLER_OPERATIONS"}
~~~

## Caption the edited head when requested

Draft after timing edits so source words follow the retained picture and sound. The caption
document remains editable before rendering; correct recognition errors before burning it.

<!-- example: draft -->
~~~json
{"command":"captions.draft","project":{"project_id":"$PROJECT_ID"},"transcripts":[{"file":"transcript.json"}]}
~~~

Review cue text and timing in the returned document, then keep it as $CAPTION_DOCUMENT.
Correct recognition errors before rendering. $FONT is a font identity/path object (for example {"path":"font.ttf"}) or an
unambiguous installed name from fonts.find. $FONT_SIZE, $LINE_HEIGHT and $CAPTION_RECT
are pixel layout values fitting the project. Choose enough room for the actual cues.
Overflow is rejected so cut-off text cannot silently pass.

<!-- example: render -->
~~~json
{"command":"job.start","request_id":"caption-render","run":"captions.render","arguments":{"document":"$CAPTION_DOCUMENT","project":{"project_id":"$PROJECT_ID"},"layouts":{"default":{"fonts":["$FONT"],"size":"$FONT_SIZE","rect":"$CAPTION_RECT","line_height":"$LINE_HEIGHT","letter_spacing":0,"wrap":"none","overflow":"reject"}},"output":"captions.mkv"}}
~~~

$JOB_ID is the returned ticket.

<!-- example: wait -->
~~~json
{"command":"job.wait","job_id":"$JOB_ID","timeout_seconds":120}
~~~

Continue only when status is completed. Keep result.asset as $CAPTION_ASSET, and read its id and duration
as $CAPTION_ASSET_ID and $CAPTION_DURATION. Attach the generated asset and a fresh overlay
track together. If a captions track already exists, choose fresh IDs or explicitly replace
its intended clip; do not duplicate captions or overwrite unrelated graphics.

<!-- example: attach -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"attach-captions","operations":[{"op":"media.add","asset":"$CAPTION_ASSET"},{"op":"tracks.edit","edit":{"op":"add","track":{"id":"captions","kind":"video","clips":[],"enabled":true,"locked":false,"composite":"alpha_over"}}},{"op":"tracks.edit","edit":{"op":"place","track_id":"captions","clip":{"id":"caption","asset_id":"$CAPTION_ASSET_ID","start":0,"source_in":0,"duration":"$CAPTION_DURATION"},"collision":"reject"}}]}
~~~

Keep the new revision. Export and inspect the actual captions at representative times,
including the first/last cues. A timing edit after this step needs regenerated captions.
This recipe burns one transparent overlay; it does not claim editable generated clips.

## Deliver when requested

$OUTPUT is a new filename such as delivery.mp4. $INPUT_TRANSFER matches the working RGB
declaration; do not choose another transfer to make validation pass.

## Export and wait

<!-- example: export -->
~~~json
{"command":"job.start","request_id":"delivery-export","run":"export.run","arguments":{"project":{"project_id":"$PROJECT_ID"},"output":"$OUTPUT","profile":"h264_aac","streams":"audio_video","input_transfer":"$INPUT_TRANSFER"}}
~~~

Use its ticket as $JOB_ID. A running job needs
another bounded wait; failed, interrupted or cancelled is not a successful delivery.

<!-- example: wait-export -->
~~~json
{"command":"job.wait","job_id":"$JOB_ID","timeout_seconds":120}
~~~

## Review the actual delivered file

Start review only after job.wait reports the export completed. Queuing export and review
together does not create a dependency; a review can run before the file exists.
The export receipt verifies the encoded file. Review adds contact sheets, timing checks
and audio measurements. Set $REVIEW_OUTPUT to a new directory name, such as review.
If the project has changed since export, use the exact exported revision in the project
reference rather than comparing the file with an unrelated new head.
Choose the initial sheet to cover the caption cues. Start with 16 evenly spread frames
(the engine default), and compare their times with the first/last cue intervals before
requesting review. Increase frames within 1-64 if needed. Very short cues or long cuts may
still need targeted delivered-file frames; inspect those rather than claiming full coverage.

<!-- example: review -->
~~~json
{"command":"job.start","request_id":"delivery-review","run":"export.review","arguments":{"path":"$OUTPUT","output":"$REVIEW_OUTPUT","project":{"project_id":"$PROJECT_ID"},"frames":16,"rendition_height":0}}
~~~

Use its new ticket as $JOB_ID.

<!-- example: wait-review -->
~~~json
{"command":"job.wait","job_id":"$JOB_ID","timeout_seconds":120}
~~~

Inside result, check timing.video.ok and timing.audio.ok, the review summary, image sheet and relevant
audio/playback. A passing duration check alone does not establish editorial quality.
The full measurements remain in review.json inside the review folder.

## Recovery and scope

For local font files use a path object; a string names an installed font family. fonts.find
searches OS fonts. For text overflow, adjust the box/font/cue layout and choose a new output.
For revision conflicts, refresh and recompute proposals against the new head. After an
uncertain saved edit, session.receipt with its request_id distinguishes commit from retry.
Failed or cancelled jobs need the reported retry/path/hint; a ticket alone is not completion.

With ready matching footage and a supplied transcript, this recipe takes 14 editing calls
plus one skill load, including caption rendering and actual delivery review. Recognition,
preparation, editorial changes and error recovery add calls; correctness takes precedence.
Use extra delivered frames when a contact sheet misses a cue; a timeline preview alone is
not a visual check of the encoded file. Report actual paths and material limitations.

Windows and Linux support this workflow; speech requires a separately configured local runtime
and model. Recording and hardware decode have no Linux backend. The plugin bundles no engine,
models, fonts or media tools. This recipe burns a transparent caption asset, not an editable
generated clip, and does not establish general recognition or long-form editorial quality.
