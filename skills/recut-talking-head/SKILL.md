---
name: recut-talking-head
description: Tighten an existing talking-head edit by proposing and applying pause and filler cuts while preserving linked picture and sound.
---

# Tighten a talking head

Start from a saved project with linked picture/sound tracks and bound sources, using the
cutbolt setup recipe if needed. $PROJECT_ID identifies it; $REVISION is its actual head.
The examples assume the voice track is a. Substitute the actual voice track.
Use listed cutbolt_* tools, or cutbolt_run for an unlisted command. Examples are engine
requests; replace $PLACEHOLDERS with observed values. Queue blocking work.

Use a source-bound word transcript. A document at transcript.json can be referenced below
without copying its words into context. If none exists, first queue media.transcribe with
the configured local runtime, then save its transcript document. Recognition is an estimate:
review suspicious words and uncovered sounds before cutting speech.

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

The result is a shorter linked timeline. Use the captions recipe on this new head so cue
times follow the edits, then deliver-and-review to export and inspect timing and playback.
For the provided-transcript case, setup + this recipe + captions + export/review takes
15 editing calls in the bounded recipe; loading skills, source discovery, recognition and repairs add calls.
This is a recipe target, not a guarantee for arbitrary material.

On REVISION_CONFLICT, refresh and recompute the proposal; never apply old cut operations
to an unrelated head. After uncertain session.apply, query session.receipt with that
edit's request_id before retrying. Check retry/path/hint for other failures.
