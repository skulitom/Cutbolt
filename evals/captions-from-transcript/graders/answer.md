---
type: llm
weight: 2
focus: trace
---

PASS if the trace shows captions drafted from talks/demo.transcript.json with captions.draft, rendered onto a saved Cutbolt timeline of the talk (a captions.render job, then session.apply), and exported by an export.run job that completed, and the final reply gives the captioned MP4's path under talks/ and says the original is unchanged.

FAIL if the captions are invented rather than drafted from the transcript, the reply claims an export the trace doesn't show completing, the original is overwritten, or the reply only explains how to do it.
