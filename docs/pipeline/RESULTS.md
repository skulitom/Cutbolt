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
| Y06 | not_run | No real speech inference or listening review. Local weights alone do not count. |
| Y07 | not_run | Caption alignment remains unimplemented. |
| Y08 | not_run | No selective production rebuild/cache or approval invalidation. |
| Y09 | partial | Oversized narration explicitly fails; regeneration, scene-four replacement and coordinated retiming remain unimplemented. |
| Y10 | not_run | Production approval records and invalidation remain unimplemented. |
| Y11 | not_run | Scene/generation coordinator retry and interruption recovery remain unimplemented. Existing session/reference-job checks are separate. |
| Y12 | not_run | This pilot produces lossless RGB reference media, not a tested Rec.709 H.264/AAC delivery preset. |
| Y13 | passed | Native scene engine consumes ordinary PNG/WAV without PixelForge/Qwen. Live companion exports substitute with identical decoded content. |
| Y14 | partial | Missing tool has a structured failure, leaves no final output and cleans owned scratch files. Scene-stage timeout/kill/publication recovery remains unverified; existing reference-job tests do not close this case. |

P1/P2 do not require closing every broader Y case. The fixture proves the specific file handoff and single-scene exit criteria; ten-second limits do not yet accommodate the proposed twelve-second storyboard slots. Do not silently shorten those production slots to fit this implementation.

## Progress accounting and next work

The unchanged 100-point checklist gains **V01 basic** (position, integer scale, quarter-turn rotation and crop of PNG animation clips), **V02 basic** (layered PNG clips with opacity) and **E03 basic** (timeline frame/still previews). General video transforms, animated parameters, blend modes, premultiplied alpha, preview caches/contact sheets and other extended criteria stay open. Audio conversion alone does not satisfy A01 extended; the PixelForge adapter itself earns no editing point. Coverage increases 11/100 to 14/100 only with a passing fresh `tools/verify.py` report.

The next useful slices are durable scene compilation with production-sized limits, a reviewed local speech take, explicit dependency/review invalidation, and a verified delivery encoder. Keep each step independently usable through local files/commands and retain the complete editing scope.
