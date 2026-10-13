# Fresh-agent footage pilot results

13 October 2026. This diagnostic ran three original short-footage tasks, two tools, two models and three repeats: 36 fresh client profiles. The [frozen protocol](FOOTAGE_BENCHMARK.md) and independent output auditor were fixed before model starts. All attempts remain included.

**Cutbolt passed 18/18 output checks; plain FFmpeg passed 14/18.** The sample is too small to establish a broad reliability lead. In Opus runs, FFmpeg used fewer calls and less peak context on all three tasks. Cutbolt reduced both for Sonnet trim/reorder; its inset workflow remained expensive.

The client was Claude Code 2.1.280 at high effort. Requested models resolved to claude-opus-5-5, claude-sonnet-5. Both arms used the same original media and delivery requirements. Cutbolt used the frozen engine from 2dbab3c and the original pre-f76b3ad skills. Raw traces and generated media remain external; [bounded evidence](../verification/footage-benchmark-2026-10-13.json) includes input, output and measurement identities.

## Completed attempts

Calls include skill loads. Each metric is median [minimum–maximum] over all three attempts, including failures. Context is peak input per request, including cached input; cumulative input and client-reported cost are in the evidence.

| Model/task | Tool | Success | Calls | Peak context | Seconds |
| --- | --- | --- | --- | --- | --- |
| opus / broll-inset | cutbolt | 3/3 | 25 [24–26] | 53,788 [47,955–54,150] | 100.9 [82.3–103.3] |
| opus / broll-inset | ffmpeg | 3/3 | 13 [9–15] | 20,801 [15,093–24,100] | 156.5 [86.0–209.4] |
| opus / reorder | cutbolt | 3/3 | 13 [12–16] | 31,186 [29,225–36,462] | 47.8 [38.1–49.2] |
| opus / reorder | ffmpeg | 3/3 | 7 [5–8] | 12,033 [10,917–13,284] | 67.9 [64.4–89.0] |
| opus / trim | cutbolt | 3/3 | 13 [12–19] | 30,455 [28,177–36,245] | 40.4 [38.2–62.6] |
| opus / trim | ffmpeg | 3/3 | 5 [4–6] | 9,679 [9,197–10,754] | 52.2 [44.0–52.6] |
| sonnet / broll-inset | cutbolt | 3/3 | 37 [32–42] | 102,061 [87,522–116,141] | 251.7 [188.0–361.9] |
| sonnet / broll-inset | ffmpeg | 2/3 | 33 [26–40] | 59,338 [44,720–62,364] | 341.1 [264.0–408.7] |
| sonnet / reorder | cutbolt | 3/3 | 14 [8–15] | 40,493 [35,669–46,951] | 78.2 [56.7–101.6] |
| sonnet / reorder | ffmpeg | 2/3 | 28 [21–30] | 61,849 [60,577–65,498] | 450.3 [385.4–460.5] |
| sonnet / trim | cutbolt | 3/3 | 12 [9–13] | 37,027 [36,745–39,349] | 61.9 [56.3–69.9] |
| sonnet / trim | ffmpeg | 1/3 | 23 [19–41] | 46,904 [44,665–55,206] | 240.9 [220.6–375.1] |

Observed median completion time was lower for Cutbolt in each row pair, but runs overlapped background verification and their CPU busy fractions differed. These are provisional observations, not a controlled speed win. The evidence retains per-trial load samples as summarized busy fractions. First-file appearance is not a verified preview; separate rendering time and first useful preview time were unavailable.

## Failure audit

- Sonnet trim repeats 1 and 3: AAC presentation lasted 2.986667 seconds instead of 3; RMS error was 298.11 against a 250 limit. Picture counters passed.
- Sonnet reorder repeat 1: AAC presentation lasted 4.992 seconds instead of 5 and decoded to 239,616 sample frames instead of the required 240,000. Correlation/RMS are unavailable for the short stream, not zero.
- Sonnet inset repeat 3: at output frame 75 (exactly 3 seconds), B frame 74 remained visible instead of A frame 125. The script used an inclusive B-roll endpoint. Minimum frame PSNR fell to 10.75 dB against the 26 dB gate. AAC presentation was 5.994667 seconds instead of 6.

All four final failed outputs used Windows AAC_MF after the models tried to make the measured bitrate equal 320 kb/s. Native AAC often produced about 250 kb/s on these synthetic sounds despite a 320 kb/s target. The prompt said “AAC at … 320 kb/s”; the intended requirement was the encoder setting. That ambiguity may have encouraged the detour and is a limitation of this pilot. Preserve these outcomes and clarify target versus measured bitrate before a new benchmark version.

Every final H.264 bitstream declares CRF 18. All 18 Cutbolt AAC targets match completed receipts for the final output hash; all 18 FFmpeg final producing commands were separately reviewed for the 320 kb/s target. All source hashes and frozen materials match, all final output hashes are unchanged, and every model identity resolved.

## Category assessment

| Observed category | Cutbolt outcome relative to FFmpeg |
| --- | --- |
| Output success | Tie for each Opus task (3/3); win in each Sonnet task in this sample |
| Calls and peak context | Loss for every Opus task; win for Sonnet trim/reorder; loss for Sonnet inset |
| Completion time | Lower median in all pairs; provisional because load was uncontrolled |
| Numerical picture similarity | Mixed on successful insets; lower for Cutbolt trim/reorder against the declared FFmpeg RGB reference |
| General quality or rate–distortion | Unestablished; requested CRF is not matched bitrate, and the reference favors its own decode/scaling choices |

The isolation check verifies the exposed tools/skills and fresh profiles. It is not an OS filesystem sandbox: some FFmpeg attempts used shared temporary directories despite the workspace-only prompt. This limits a strong isolation claim. The results also cover tiny synthetic sources, not long recordings, captions, visual taste or noisy speech.

## What this changes

Retain plain FFmpeg as the baseline and report losses. Prioritize complete preview/edit/export skill recipes and automatic source preparation before adding more catalog entries. Keep the source-clock, layering and generated-clip work in the direct-footage plan: the inset traces expose real placement/discovery friction, but this pilot does not prove that a new engine stage alone will reduce agent cost. Selective schema discovery remains measurable follow-up work.

The six revised-skill trials are a separate diagnostic, not replacements or extra successful repeats for the baseline. Their engine, sources, prompts and auditor stayed fixed. The existing accepted talking-head trial and its 15-call/40,000-peak-context target are separate from these three tasks. No release gate, roadmap scope, editing points or overall superiority claim is adopted here.

## Revised skills: one attempt per model/task

All six outputs passed, all frozen identities matched, and there were no failed tool results.
The engine is unchanged; only the skills moved to f76b3ad. Calls include skill loads.

| Model/task | Calls | Peak context | Schema lookups |
| --- | --- | --- | --- |
| opus / trim | 9 | 25,152 | 0 |
| opus / reorder | 11 | 30,372 | 1 |
| opus / broll-inset | 23 | 43,870 | 2 |
| sonnet / trim | 7 | 32,060 | 0 |
| sonnet / reorder | 7 | 33,793 | 0 |
| sonnet / broll-inset | 18 | 66,128 | 9 |

These single attempts used fewer calls than their respective baseline medians, but do not
establish a repeatable improvement. Both inset trials still used more than 15 calls and
40,000 peak tokens; those thresholds belong to the separate accepted talking-head task,
not a newly imposed gate here. Opus still discovered preview schemas, and Sonnet used nine
schema lookups for the inset. Complete inspection recipes remain useful follow-up work.

The protocol gives Cutbolt a saved editable project while FFmpeg only owes the delivered
file. That reflects the stated product workflows, but is another reason not to interpret
call counts as equal internal workloads or claim one tool wins every simple trim.
