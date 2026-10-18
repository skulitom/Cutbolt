---
type: llm
weight: 2
focus: trace
---

PASS if the trace shows audio.normalize measuring the episode on a saved Cutbolt project with a target of about -14, its proposed operations applied with session.apply, and an export.run job that completed, and the final reply gives the new file's path, the measured loudness before (-24.6 LKFS) and the result after (about -14 LKFS, true peak no higher than -1 dBTP), and says the original is unchanged.

FAIL if the reply guesses the loudness instead of measuring it, applies a gain without the measured proposal, claims an export the trace doesn't show completing, overwrites the original, or only explains how to do it.
