# YouTube pilot: implementation and acceptance plan

Prepared 2 October 2026; updated after the first technical pilot. **Bounded P1/P2 exit criteria now pass** with original synthetic media; P3-P6 remain planned. See [RESULTS.md](RESULTS.md) for exact evidence, partial Y cases and limitations, and the [handoff](../YOUTUBE_PIPELINE.md) for the environment and weights.

## Preparation delivered separately from implementation

- [x] Identify the existing Cutbolt reliability foundation and media limitations.
- [x] Inspect the local PixelForge interface and record its exact commit.
- [x] Specify the six-scene brief, local artifact boundary and review semantics.
- [x] Define compatibility, revision and recovery acceptance cases.
- [x] Provide an explicit pinned-model download helper and external receipt format.
- [x] Execute the bounded P1/P2 file handoff and five-second scene fixture; evidence in [RESULTS.md](RESULTS.md).
- [x] Build the local workflow coordinator ([PRODUCTION.md](../PRODUCTION.md)): manifest, stage receipts, invalidation by content keys, resumption, review records, batched offline narration and delivery. Evidence and the partial Y cases are in [RESULTS.md](RESULTS.md#production-coordinator-5-october-2026).
- [ ] Finish P3-P6: a permitted reference voice with the Base model, human review gates on a real production, every interruption point of Y11, and full playback review.

Download completion is recorded by the external `download-receipt.json`, not by treating an unchecked pipeline test as passed. Preparation does not change `progress/capabilities.json`.

## Ordered implementation slices

| Slice | Work | Exit evidence |
| --- | --- | --- |
| P1: artifact handoff | Original small PixelForge recipe, PNG/animation-timing export, manifest validator and input-root checks; deterministic synthetic WAV before live speech | Independent decoder agrees on pixels/alpha; correct animation order and exact durations; invalid paths/identities rejected; external fixture recipe/provenance recorded |
| P2: single-scene video | PNG sequence ingestion, declared SDR/alpha handling, integer scaling, separate WAV input, explicit resampling/channel mapping, frame/range preview | A 5-10-second scene renders at 1080p/25 fps/48 kHz; decoded frame/sample assertions, correct alpha edges and preview/final agreement; source hashes unchanged |
| P3: real local Qwen | Dedicated external environment with locked packages; pinned local weights; permitted reference WAV/transcript; per-scene synthesis receipts | With networking unavailable, synthesize two takes, record measured durations/sample rates, elapsed time and peak GPU use; listen for missing/repeated words and pronunciation; missing weight fails locally |
| P4: production revisions and reviews | Implement draft manifest/state contract, dependency invalidation, scene duration choices, captions with actual alignment, selected artifact reuse | Recolor one scene and replace scene four's take; only appropriate dependencies become stale; unrelated approved assets are reused; longer take yields explicit retiming decision/diff |
| P5: six-scene delivery | Complete original 60-90-second explainer, fast review artifacts, subtitle export, validated opaque SDR MP4 H.264/AAC preset | Human-reviewed script/storyboard/voice/rough cut; decoded export duration, A/V synchronization, caption boundaries and playback checked; exact final export gets human approval |
| P6: interrupted work and measurement | Durable coordinator stage receipts, failure reconciliation, lost-response retries, resumed generation/render | Stop at defined boundaries; restart reaches correct state without duplicate timeline edits or false completion; publish reuse/rebuild counts and honest task-success measurements |

P3 runtime setup can proceed independently of P2; P4 builds on the proven single-scene path, and P5 requires P3/P4. Design durable stage identities in P1/P4; P6 validates interruption behavior rather than deferring all recovery design to the end. Reuse existing session crash/concurrency tests without claiming they cover the new coordinator.

Do not require a full MCP adapter, GUI studio, general effects catalogue, music generator or cloud uploader to finish this pilot. Do not remove them or other desirable editing features from the broader roadmap solely because this pilot is narrower.

## Acceptance cases and independent oracles

| ID | Case | Required observation |
| --- | --- | --- |
| Y01 | Asset identity and containment | Changed/missing files, wrong digest, path traversal and a link/junction escaping the input root fail before publication; originals and existing outputs remain unchanged |
| Y02 | Pixel frames | Synthetic labeled frames with transparent colored edges and trimmed sprites reconstruct at the correct anchor; a separately derived expected image catches alpha/placement/scaling errors |
| Y03 | Animation timing | Repeated frames, reordered animation entries, 40/80/120 ms holds, a 100 ms hold and loop/end behavior follow the selected rule; rejection or reported resampling is explicit; no cumulative drift |
| Y04 | Audio conversion | Synthetic mono and stereo impulses at distinct source rates land at independently computed output sample positions; declared sample-count rounding and conversion tolerance are met; no accidental channel gain or clipping |
| Y05 | Preview correspondence | Preview frames and final reference frames at the same rational times agree under the declared preview transform; sampled contact sheets report omissions |
| Y06 | Local speech | Real pinned model loads locally with networking unavailable; local reference voice used; missing files do not trigger a download; measured metadata and listening review recorded |
| Y07 | Caption alignment | Text checked against actual speech; manually annotated anchors bound measured alignment error; cues ordered and in range; replaced voice takes stale the affected captions |
| Y08 | Visual-only revision | One scene's recolor changes expected pixels; unrelated decoded reference scene frames and all unchanged narration files remain equal; no new TTS calls; timing unchanged |
| Y09 | Narration revision | New text regenerates only dependent speech/captions and reconsidered visuals; longer speech cannot silently truncate, stretch or overlap; selected ripple reports every changed downstream placement |
| Y10 | Review integrity | Approvals tied to exact content/dependencies; a stale approval cannot clear a new version; a shared palette change finds every dependent visual; agent checks cannot impersonate human approval |
| Y11 | Retry and interruption | Kill worker before output, after output but before receipt, and after receipt before response; restart reuses/reconciles complete artifacts; partial files never count as complete; repeated edit has one effect |
| Y12 | Delivery and source protection | Actual 1920 x 1080/25 fps SDR H.264/AAC output, 48 kHz stereo; decoded frame count and A/V timing checked, including encoder delay/padding; no source/output overwrite; full playback review |
| Y13 | Substitution | A plain user-provided PNG sequence and WAV can use the same handoff with no PixelForge/Qwen installation required for the core renderer |
| Y14 | Resource or process failure | Missing tool, nonzero worker exit, timeout and interrupted publication yield bounded actionable diagnostics; previous approved export and originals survive; resumable state is explicit |

Set concrete timing/color/audio tolerances before accepting the corresponding slice and record their rationale. Exact equality is appropriate for unchanged assets and the pinned lossless reference; lossy delivery and resampling need stated numerical tolerances, plus actual playback/listening. Never assert byte-identical neural output or MP4 files across tool versions or GPUs. Do not use the same renderer calculation as its own sole oracle.

## Evidence to retain outside Git

Per run, save tool/version/license inventory, GPU and runtime information, model revision, original fixture recipes/scripts, stage requests/receipts, source/output digests, project snapshots, review events, previews, final export and a result report. Use local paths in local reports; avoid publishing private voice references or machine details unnecessarily.

The result report must distinguish `passed`, `failed`, `not_run` and `blocked` for each Y case. Separate automated mechanical checks, agent critique and human editorial decisions. Record limitations instead of treating an unavailable dependency as a successful skip. A download receipt demonstrates file integrity, not inference or editing quality.

Measure initial production and each revision separately: completion without manual repair, unintended changes, source preservation, regenerated/reused stages, time to useful preview, total elapsed time, model time, render time, agent tool calls/tokens when available, and recovery outcome. Report cold versus warm/cache conditions and all attempts. This pilot is a baseline; superiority requires matched comparison against alternatives under the [competitive benchmark plan](../COMPETITIVE_ANALYSIS.md).

## Progress accounting

Y-case results are workflow evidence and do not automatically award points. Potential underlying work touches M01 (media), T05/T06/T08 (timing/ripple/tracks), V01/V02 (transforms/compositing), A01/A02 (audio), G03 (captions), E02/E03/E04/E05/E06 (delivery/previews/jobs/ranges/protection) and P01 (cache). Read each exact criterion in [capabilities.json](../../progress/capabilities.json); a narrow pipeline test may cover only part of it.

Only add passing evidence IDs when the actual acceptance criterion is met, then run `tools/verify.py` and record the change in [PROGRESS_HISTORY.md](../PROGRESS_HISTORY.md). The pilot does not shrink, reweight or replace the existing 50 capability groups. Historical preparation left coverage at 10/100 and agent foundations at 8/10; v0.3 advanced those separately. Current verified figures belong in [PROGRESS.md](../PROGRESS.md).
