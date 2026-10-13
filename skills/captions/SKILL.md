---
name: captions
description: Draft captions from source-bound transcripts onto an edited Cutbolt timeline, render a transparent caption asset and attach it as an overlay.
---

# Caption the current cut

Use a saved project with placed tracks and a transcript document at transcript.json.
$PROJECT_ID and $REVISION refer to the current cut. Draft after timing edits.
Replace $PLACEHOLDERS with actual values; send listed cutbolt_* calls without command,
or use cutbolt_run with command/arguments. Keep blocking rendering in job.start.

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

For a supplied local font, use a path object; strings name installed font families.
Missing or ambiguous installed fonts need a usable identity from fonts.find. For text overflow,
adjust the box/font/cue layout and use a new output filename. For a stale revision,
check whether timeline timing changed before attaching. Reuse an uncertain edit's request ID
and inspect session.receipt; job.wait returning a ticket is not proof of a completed render.

This workflow runs on Windows and Linux with external FFmpeg/ffprobe. Speech recognition,
when needed, requires a separately configured local runtime and model. Recording and hardware
decode have no Linux backend. The plugin bundles no engine, fonts, models or media tools.
