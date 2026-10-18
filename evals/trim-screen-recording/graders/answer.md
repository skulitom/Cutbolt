---
type: llm
weight: 2
focus: trace
---

PASS if the trace shows screen/setup-cli.mp4 inspected, a saved Cutbolt project whose clip starts at source time 12 s and lasts 148 s (0:12 to 2:40), and an export.run job that completed, and the final reply gives the new MP4's path, its length of 2:28 (148 s), and says the original is unchanged.

FAIL if the kept range is wrong by more than one frame, the reply claims an export the trace doesn't show completing, the original is overwritten, or the reply only explains how to do it.
