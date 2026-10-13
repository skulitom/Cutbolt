---
name: screen-tutorial
description: Trim and revise a saved screen tutorial, add a project-sized title and normalize its narration using Cutbolt's linked track edits.
---

# Edit a screen tutorial

Start from the cutbolt setup recipe with picture on v and narration on linked track a.
Use actual track IDs if different. $PROJECT_ID is the saved project; replace placeholders
from its current state. Use listed cutbolt_* tools or cutbolt_run; long jobs use job.start.

<!-- example: current -->
~~~json
{"command":"session.get","project_id":"$PROJECT_ID"}
~~~

Choose frame-aligned intervals from the user's requested changes. In this worked recipe,
$LEAD removes the beginning; $MISTAKE_START and $MISTAKE_DURATION describe a later deletion
on the timeline after that leading trim. Recompute those times after any earlier edit.
$RIGHT_CLIP_IDS maps each clip split by the second deletion to a fresh right-side ID:
objects with id and new_id. $RIGHT_LINK_IDS does the same for links crossing that split.
Read the actual clip/link IDs above; don't construct them from track names.

<!-- example: trim -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"tutorial-trims","operations":[{"op":"tracks.edit","edit":{"op":"ripple_delete","track_ids":["v","a"],"start":0,"duration":"$LEAD","links":"include","right_clip_ids":[],"right_link_ids":[],"end_policy":"resize","transitions":"reject_affected"}},{"op":"tracks.edit","edit":{"op":"ripple_delete","track_ids":["v","a"],"start":"$MISTAKE_START","duration":"$MISTAKE_DURATION","links":"include","right_clip_ids":"$RIGHT_CLIP_IDS","right_link_ids":"$RIGHT_LINK_IDS","end_policy":"resize","transitions":"reject_affected"}}]}
~~~

This boundary trim assumes it doesn't split another linked selection; for more complex
placements, supply the required split IDs too. Rejecting affected transitions makes an
unsupported transition change explicit.

Add a title with a current revision and stable inner edit request ID. $FONT is an external
font identity/path object (for example {"path":"font.ttf"}) or an installed font name.
A string names an installed family; it does not identify a workspace file. fonts.find
searches OS fonts; media.inspect does not inspect font files. $TITLE is the requested text, and $TITLE_DURATION must fit the timeline
and its native clock. A transparent title preserves the footage underneath.

<!-- example: title -->
~~~json
{"command":"job.start","request_id":"tutorial-title-job","run":"graphics.place","arguments":{"project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"tutorial-title-edit","instance_id":"title","track_id":"titles","template_id":"cutbolt-title-card-v1","values":{"fonts":{"type":"fonts","value":["$FONT"]},"label":{"type":"text","value":"$TITLE"},"duration":{"type":"time","value":"$TITLE_DURATION"},"transparent":{"type":"boolean","value":true}}}}
~~~

Set $JOB_ID from the ticket. When completed,
take result.revision as the new head.

<!-- example: wait -->
~~~json
{"command":"job.wait","job_id":"$JOB_ID","timeout_seconds":120}
~~~

Choose $TARGET_LKFS for the delivery request (for example -18). Read the measured levels
and proposed gain before applying; do not normalize already-silent material. Keep the
returned operations as $LEVEL_OPERATIONS.

<!-- example: normalize -->
~~~json
{"command":"audio.normalize","project":{"project_id":"$PROJECT_ID"},"target_lkfs":"$TARGET_LKFS"}
~~~

<!-- example: apply-levels -->
~~~json
{"command":"session.apply","project_id":"$PROJECT_ID","expected_revision":"$REVISION","request_id":"tutorial-levels","operations":"$LEVEL_OPERATIONS"}
~~~

Use deliver-and-review next. Test important instructional clicks and speech around each cut;
the original may be long, but a cut that changes the demonstrated sequence is still wrong.
The automated recipe fixture uses a bounded six-second original, not a twenty-minute trial.

On conflicts, refresh the head and reconsider timing. graphics.place failures include an
inner session-receipt lookup; use tutorial-title-edit to recover a possibly committed title.
For locked tracks or overlaps, change the intended edit rather than unlocking unrelated work.

This workflow runs on Windows and Linux with external FFmpeg/ffprobe. Speech recognition,
when needed, requires a separately configured local runtime and model. Recording and hardware
decode have no Linux backend. The plugin bundles no engine, fonts, models or media tools.
