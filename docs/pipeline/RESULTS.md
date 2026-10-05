# First technical scene pilot — 2 October 2026

P1 (artifact handoff) and the bounded P2 (single-scene video) exit criteria pass for the fixture described below. This is a technical fixture with synthetic impulses, not the six-scene explainer, a voice-quality demonstration, a production coordinator or an editorially approved video. See [SCENES.md](../SCENES.md) for implemented commands and limits.

## Retained evidence

External run: `C:\DEV\CutboltData\pipeline\scene-pilot-20261002\verified-final`.

- `scene.json`: editable original scene configuration; `project.json`: ordinary two-clip timeline assembled from its compiled asset.
- `sources/original-recipe.json`, ordinary PNG/WAV fixtures and `sources/pixelforge/`: generated original sources and live companion exports. None are in Git.
- `pixelforge-handoff.json`: exact frame order, durations, identities, anchors and atlas provenance.
- `output/scene.mkv` and `output/pixelforge-scene.mkv`: five-second 1920 x 1080 / 25 fps FFV1/bgr0, 48 kHz stereo PCM. All 125 decoded RGB frames and 240,000 samples per channel agree with independent expectations; companion substitution agrees as well.
- `output/frame-*.png`, `output/mcp-preview.png`, `output/range.mkv`: full-resolution timeline previews, including an interval across a reordered cut.
- `verification.json`: fourteen passing scene/preview checks, including the optional live PixelForge check; source identities, backend versions, scene receipt and explicit `human_editorial_approval: false`.

PixelForge was exercised at commit `911d4fa167bdea5949669c20679693257e3cc170` (MIT) using its existing external Node CLI. The core verification suite also runs without PixelForge or Qwen, using ordinary generated PNG/WAV files. No source or assets were copied from either companion into Cutbolt. The new native build dependencies are png 0.18.1 and hound 3.5.1; the independent image oracle uses external Pillow 10.3.0. Exact licensing and dependency inventory are in [DEPENDENCIES.md](../DEPENDENCIES.md).

The first fixture-generation attempt used the wrong PixelForge `pixels` shape and failed validation before export. Two early scene-suite attempts passed pixel/audio/preview checks but failed in the test harness: junction creation argument passing, then the expected error code for a missing FFmpeg executable. Those were corrected and retained externally in the sibling preparation/`verified`/`verified-2` directories. `verified-3` was the first complete companion run. After visual review, the final fixture strengthened mixed-color overlap with a shifted layer and delayed start, and added an assertion that compositing differs from either layer alone. `verified-final` records that stronger rerun and exact Node/package versions. No failed attempt is recorded as a success.

## Acceptance status

`partial` means the named subset passed while the entire Y case remains open. No human review gate was approved by these automated tests.

| Case | Status | Evidence and limits |
| --- | --- | --- |
| Y01 | passed for bounded scene inputs | Digests, missing files, traversal/absolute/ADS paths, an escaping Windows junction, original preservation and existing-output rejection tested. Sources rechecked before publication. |
| Y02 | passed | Independent Pillow forward transforms plus rational alpha compare every decoded pixel. Includes all four quarter turns, cropped/trimmed source reconstruction, anchors, partially off-canvas placement, integer scale and overlapping opacity. |
| Y03 | passed | Reordered/repeated 40/80/120 ms holds, 100 ms strict rejection and explicit output-frame-start sampling; loop/hold-last/transparent endings and nonzero layer start. Rational selection without accumulated drift. |
| Y04 | passed for declared input matrix | PCM16 mono/stereo at 24/44.1/48 kHz; every decoded output sample checked, including signed impulses, channel mapping, endpoint extension, count rounding and silence. Narration overflow rejected. Tolerance is exact integer equality for this deterministic linear converter. This does not establish perceptual resampling quality. |
| Y05 | partial | Exact timeline frame previews and cross-cut range preview agree with final reference pixels/audio. MCP preview checked. Contact-sheet sampling and cache behavior remain unimplemented. |
| Y06 | not_run | No accepted speech inference or listening review. Local weights alone do not count. A later external demonstration ran the CustomVoice checkpoint offline (see [YOUTUBE_PIPELINE.md](../YOUTUBE_PIPELINE.md)); that is an observation, not this acceptance case. |
| Y07 | not_run | Caption alignment remains unimplemented. |
| Y08 | not_run | No selective production rebuild/cache or approval invalidation. |
| Y09 | partial | Oversized narration explicitly fails; regeneration, scene-four replacement and coordinated retiming remain unimplemented. |
| Y10 | not_run | Production approval records and invalidation remain unimplemented. |
| Y11 | not_run | Scene/generation coordinator retry and interruption recovery remain unimplemented. Existing session/reference-job checks are separate. |
| Y12 | not_run | This pilot produces lossless RGB reference media, not a tested Rec.709 H.264/AAC delivery preset. |
| Y13 | passed | Native scene engine consumes ordinary PNG/WAV without PixelForge/Qwen. Live companion exports substitute with identical decoded content. |
| Y14 | partial | Missing tool has a structured failure, leaves no final output and cleans owned scratch files. Scene-stage timeout/kill/publication recovery remains unverified; existing reference-job tests do not close this case. |

P1/P2 do not require closing every broader Y case. The fixture proves the specific file handoff and single-scene exit criteria; scenes now last up to 120 seconds, so each proposed twelve-second storyboard slot compiles as one scene; the 5 October 2026 demo, made under the former ten-second limit, had to split every story scene into two shots. Do not silently shorten production slots to fit an implementation limit.

## Progress accounting and next work

The unchanged 100-point checklist gains **V01 basic** (position, integer scale, quarter-turn rotation and crop of PNG animation clips), **V02 basic** (layered PNG clips with opacity) and **E03 basic** (timeline frame/still previews). General video transforms, animated parameters, blend modes, premultiplied alpha, preview caches/contact sheets and other extended criteria stay open. Audio conversion alone does not satisfy A01 extended; the PixelForge adapter itself earns no editing point. Coverage increases 11/100 to 14/100 only with a passing fresh `tools/verify.py` report.

The next useful slices are durable scene compilation with production-sized limits, a reviewed local speech take, explicit dependency/review invalidation, and a verified delivery encoder. Keep each step independently usable through local files/commands and retain the complete editing scope.

## Production coordinator, 5 October 2026

[`tools/production.py`](../PRODUCTION.md) runs a whole narrated explainer from one manifest. Evidence is retained outside Git in `C:\DEV\CutboltData\production-20261005`: manifests, production folders with every stage receipt, call log and revision record, exports, reviews and the fixture runs.

The baseline is the 5 October progress demo (`C:\DEV\CutboltData\demo-progress-20261005`), an 80.64 s video that took about 2 h 12 min:
- about 75 of the first 97 minutes were agent authoring;
- about 170 MCP calls;
- an art generator, a scene builder and ten step scripts written by hand.

### The proof: a new video from a manifest

A new script, "Anticipation", in the demo's style:
- six scenes: a title, four narrated beats (switch, cards, compare with a custom motion, recolour) and a narrated end card;
- music, fonts and the speaker reused from the demo;
- engine: a release build of main `c0fe223` (the job pool, faster scene compositing and timeline limiters) plus this change's one-sentence MCP instruction; PixelForge 0.7.1 at `a5473c6`; Qwen3-TTS 0.6B CustomVoice `85e237c1` in WSL; the RTX 4090.

The agent's part was three tool calls, plus one call to start a timer:
1. write the manifest (2.9 KB);
2. run `build`;
3. look at the review sheet.

| | Run 1 | Run 2 (same manifest, fresh folder, after reordering alignment) |
| --- | ---: | ---: |
| Timer start to reviewed delivery | **252.5 s** | (build only) |
| Build | 236.4 s | 221.8 s |
| Narration: 5 lines, 33 s of speech, one model load | 49.4 s (load 16.1, generation 31.4) | 53.2 s (load 13.5, generation 37.3) |
| Alignment, one job | 27.5 s | 32.4 s |
| Voice assets, one job per take | 7.6 s | 14.8 s |
| Scenes, 6 at once | 8.8 s | 11.7 s |
| Mix: meters, duck, normalize with the master limiter | 40.9 s | 44.7 s (beside alignment, captions and scenes) |
| Cut ready (one session revision) | at 126.5 s | at 115.7 s |
| H.264 export of 44.64 s | 99.2 s | 96.3 s |
| Speech check on the audio-only mix (beside the export) | 61.4 s | 53.8 s |
| Final review (sheet, loudness, black, silence, clipping, timing) | 10.7 s | 9.7 s |
| Engine requests made by the coordinator | 60 | 61 |

- **The delivery.** `proof-1/exports/anticipation-r0-846143d8-1.mp4` is 44.64 s of 1920x1080 25 fps H.264/AAC at −14.0 LKFS and −0.9 dBFS sample peak, with no black, silence or clipping, and timing matching the project. SRT and WebVTT sidecars sit beside it.
- **The speech check** heard 83 of 85 words. The two differences are the recognizer's spelling of the product names ("Pixel Forge", "Cutbull"), not narration errors.
- **Reproducibility.** Both runs produced byte-identical takes (same seed, batch layout and GPU). Their caption timings differ by tens of milliseconds, because run 2 aligns the 48 kHz prepared assets instead of the 24 kHz takes.
- **Load.** Both runs shared the machine with other sessions' background verification and benchmarks: two transcription fixtures, native scenes, remapping, large imports and export benchmarks. CPU load was 43-97%, and other processes held 8.6-9.9 GB of GPU memory. On a quieter machine earlier the same day, five lines generated in 30 s, alignment took 16 s, and a 51.84 s export took 79 s.
- **Authoring time.** The 16 s between the timer starting and the build starting understates a fresh agent's authoring: this manifest was composed while the previous run finished. A fresh agent would also read [PRODUCTION.md](../PRODUCTION.md) first, for about five calls in all.

**Target not met.** The under-3-minute target is not met under this load: 4 min 12 s end to end. What remains is mostly engine and model time:
- the H.264 export takes about 2.2 times real time (another session is working on export speed);
- the narration's model load and generation;
- alignment's start-up;
- normalization's limiter passes, which reached their target exactly.

### Other runs

| Run | Film | Result |
| --- | --- | --- |
| The proof manifest again, fresh folder, main `fe807a1` (recognition vocabulary), cold kit cache | 44.64 s | Build 239.8 s at 91% CPU load. Narration 57.3 s, voice assets 9.9 s, music prepared in 27.3 s, alignment 26.8 s, scenes 9.3 s, mix 42.6 s, export 107.1 s, review 4.8 s. The speech check now hears 85 of 85 words: the recognizer is prompted with the script's names. |
| The part-two manifest (`demo-progress2-20261005`) rebuilt cold with [audio-only voice and music assets](../USAGE.md#voice-overs-and-music), beside the part-two engine `83de4ca` in two alternating pairs | 96 s | 148.8 s and 197.3 s, against 203.1 s and 306.8 s for `83de4ca` under the same load (28-62 % busy, the GPU shared in pair 2): about 61 s less after the narration in both pairs. Voice assets 2.0-7.2 s instead of 18.5-22.5 s, music 0.9-2.9 s instead of 36.7-38.8 s. The delivered audio is bit-identical to the original film's. The mix is now the longest step after the narration. Evidence: `audio-only-rebuild/`. |
| The part-two manifest rebuilt cold with [fast normalization](../USAGE.md#normalizing-loudness) and alignment started on the voice assets, on main `015ab6b`, beside `015ab6b` in two alternating pairs | 96 s | 114.6 s and 105.5 s, against 134.1 s and 115.4 s for `015ab6b` (26-44 % busy): about 18 s less after the narration in both pairs. `audio.normalize` took 3.5-4.3 s instead of 35.6-37.6 s, so the mix ends before the scenes and the cut no longer waits 19-20 s for it. The voice level is 2873 instead of 2872 milli-units (−14.002 against −14.004 LKFS, the same −1.01 dBTP); the AAC delivery then peaks at 0.0 dBFS with 2 clipped samples instead of −0.6 dBFS, because the coordinator leaves no lossy headroom. Evidence: `fast-normalize-20261005/`. |
| Six-scene pilot manifest ([example](example.production.json)), cold kit cache, same engine | 99.84 s, 8 scenes, every beat type | 454 s under load: narration 103 s, music 25 s, alignment 37 s, mix 80 s (normalize 66 s, 8 measured passes), export 211 s beside the speech check. −14.0 LKFS, 182 of 182 words heard. The review found 9.6 s of silence at the end: the 80.64 s bed is shorter than the film. |
| Supplied media only (`supplied.production.json`) | 15.36 s | One supplied WAV narration and one supplied scene recipe; no Qwen run. Aligned and placed, −14.0 LKFS, 23 of 23 words heard. |
| "Squash and stretch" development manifest, engine `d3c9140` | 51.84 s | Cold builds 267-317 s, warm kit 283 s. Then a one-line script change rebuilt only that line's narration, alignment, voice asset and scene, plus timing, captions, mix and delivery. The other five scenes, the art and four takes were reused, and the timing receipt listed the three moved boundaries. The session took one revision touching nine clips. |
| Kill during the export, then `build` again | same | The stale lock was cleared and logged. The export stage resumed as the same attempt and collected the job already running: the engine queue holds one job for that request ID, run once. |
| `tests/production.py` with the companions, final code, engine `c0fe223` | 21.12 s, 4 scenes | All 11 checks pass. First build 118.3 s with the warm kit cache. Repeat build 0.47 s with nothing rebuilt. A one-line script change 122.2 s, rebuilding 13 stages. A scarf palette change 58.2 s, rebuilding Pip's art, the scenes and delivery but no take. (On `d3c9140` before the reordering: 187.6, 0.7, 140.4 and 72.5 s.) |
| The pre-save check during development | | Twice it refused to save a rebuilt cut and saved nothing. First, array order and unused assets made the comparison too strict. Second, a music clip's `gain_milli` was left stale beside the gain curve that overrides it. Reconciliation now sets both, and the comparison is by what plays. |

The offline part of `tests/production.py` runs in about 3 s without the companions:
- 17 manifest rejections;
- art invalidation by palette and label;
- all eight beat types compiled against stand-in PNGs and accepted by `scene.inspect`, with cue layers on the frame of their word;
- the timing plan against hand-computed boundaries;
- `reconcile()` operations applied by the engine and compared with the target, limiters included;
- review gates going stale when their subject changes;
- the TTS worker refusing a missing model.

### Acceptance cases this adds to

The table at the top stays authoritative for P1/P2. This work changes:

| Case | Status | Evidence and limits |
| --- | --- | --- |
| Y01 | partial (production inputs added) | Inputs must be absolute local paths. Network, relative and alternate-stream paths are refused. Inputs are copied by content and re-hashed on every build; a changed copy fails with `SOURCE_CHANGED`. |
| Y06 | partial | The pinned CustomVoice checkpoint runs with the offline flags, from a verified copy on WSL's disk. A missing model fails before any model code loads. Still no permitted reference voice with the Base model, and no recorded human listening review. |
| Y07 | partial | Known-text alignment by the engine's aligner; captions drafted from it. An independent recognizer heard 182/182, 93/93 and 83/85 words in the cuts. A new take changes its alignment's key, so its captions are redrafted. No manually annotated anchors were measured. |
| Y08 | partial | A palette change re-rendered art and scenes with no TTS call and no timing change; takes were reused byte for byte. A recolour confined to one scene was not run separately. |
| Y09 | passed for the coordinator | New text regenerated only that line's speech, alignment, voice asset, captions and scene. The longer take rippled later scenes, with every moved boundary in the timing receipt; nothing was trimmed or stretched. A fixed `duration` that cannot hold its take fails with `NARRATION_OVERFLOW`. |
| Y10 | partial | Decisions bind to subject identities and go stale when they change. An agent's decision on a human-required gate is kept as advice. History is append-only. No human has reviewed a production. |
| Y11 | partial | A coordinator killed mid-export resumed without a second job. Takes finished while unwatched are adopted from the worker's receipt. Repeated builds make no new session revision. Killing at the other named points (before output, between output and receipt) was not exercised. |
| Y12 | partial | 1920x1080 25 fps H.264/AAC, 48 kHz stereo; frame count and duration checked by `export.review`. No full human playback review. |
| Y13 | passed for the coordinator | A supplied WAV and a supplied scene recipe entered through the same stages with no Qwen run. |
| Y14 | partial | Missing model, missing input, unsupported manifest values and engine errors return structured codes; a failed stage keeps its receipt and the previous delivery. Timeouts and worker crashes mid-take were not exercised. |

### Progress accounting

None of this changes the 100-point checklist. The coordinator is agent workflow built on existing editing capabilities, and [ACCEPTANCE.md](ACCEPTANCE.md#progress-accounting) keeps Y cases separate from capability points.
