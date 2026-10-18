---
type: llm
weight: 2
focus: trace
---

PASS if the trace shows the take was edited in a saved Cutbolt project (pauses proposed by audio.tighten and fillers by transcript.fillers, both applied with session.apply) and exported by an export.run job that completed, and the final reply gives the new MP4's path, says roughly how much was cut (about 26 s, from 252 s to about 226 s), and says raw/intro-take3.mp4 is unchanged.

FAIL if the reply claims an export the trace doesn't show completing, writes over or deletes the original, cuts without reading the proposals, or only tells the user how to do it.
