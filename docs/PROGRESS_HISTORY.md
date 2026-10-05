# Progress history

## 5 October 2026: normalizing renders the mix twice instead of up to eleven times

The part-two research (`demo-progress2-20261005/research/RESEARCH.md`, E2 and E3) found that `audio.normalize` took 34 s of the 96 s film's 142 s build, and the cut waited 14 s for it. Normalize measured the mix up to 11 times:
- once at the start;
- up to three refinements;
- once through the proposed limiter, then up to six more.

Each measurement rendered the timeline's audio through FFmpeg to a scratch WAV (two FFmpeg runs with a limiter) and metered it in Rust.

The changes:
- **One render, then passes in memory.** The mix is rendered once at the current levels, as the unsaturated sums of its limiter groups: each limited track, and the other tracks together.
  - This is the premix a limited render already runs, now built for any timeline (`track_render::premix`, `dynamics::Capture`).
  - Each refinement pass replays those sums: scaled by the candidate factor, rounded to whole codes, then through the track limiters and the proposed master limiter, saturated and metered.
  - Renders and replays share the limiting, summing and saturating code (`dynamics::Output`).
- **The reported numbers stay exact.** The proposed levels are rendered once more and replayed unscaled, which is what the timeline plays.
  - On part two, `measured` and `result` equal `timeline.meters` of the mix before and after at full precision: integrated loudness, per-channel RMS over every sample, sample and true peaks, gating counts and limiter reduction.
  - The limiter's ceiling is settled on that exact replay, without a render.
  - A third render happens only when the exact result misses the target by more than 0.05 LU, or passes the limit that stopped the gain by more than 0.01 dB. On part two the passes predicted the exact result within 0.003 LU and 0.003 dB in four settings, so none needed one.
  - The passes see a clipping mix's true peaks, not its clipped ones.
- **Pieces render side by side.** The render splits the timeline into up to eight pieces of at least 2 s, each its own FFmpeg run. A premix gives the same sums wherever a run starts; chunked renders and previews already rely on this. On part two, rendered in eight pieces, the exact replay matched the single-graph render bit for bit. One render of part two's mix dropped from 3.7-4.9 s to 0.7-1.0 s.
- **Reported work.** `result.renders` counts the renders, and `measured_passes` now counts passes, the exact ones included. Timelines over 4 hours are refused, the limit `timeline.meters` has. The rendered sums need about 1.4 GB of scratch space per hour for each group.
- **Coordinator order (E3).** The alignment job is submitted as soon as the voice assets exist, not after the audio timeline, so it never waits for the music. Stage keys and receipts are unchanged.
  - On `83de4ca`, music preparation still encoded a black 1080p picture for 20-37 s. There this order made alignment slower: it ran beside that work and took 33-34 s instead of 17-22 s, so it ended at most a second earlier, and once 24 s later.
  - Since audio-only assets (`277f0d2`), music is ready in under a second at the start, so the order only matters when music preparation is slow.

Measured with release builds, run alternately while other sessions loaded the machine:
- **Normalize alone.** Five interleaved rounds on part two's pre-normalize snapshot, 12-88 % busy. The snapshot was rebuilt from the build's audio timeline as the mix stage builds it, and checked to reproduce the recorded mix.
  - `83de4ca`: 32.2-43.1 s, median 38.4 s.
  - This change: 3.3-5.4 s, median 3.5 s.
  - Both propose −14.00 LKFS and −1.01 dBTP through a −1.01 dBFS limiter. The voice level differs by one milli-unit, 2873 against 2872: exactly −14.0024 against −14.0039 LKFS.
- **The part-two film, rebuilt cold.** The manifest and `production-config.json` were copied to `C:\DEV\CutboltData\fast-normalize-20261005`, with `engine` set to this release build, and every build had an empty kit cache. On main `015ab6b`, in two alternating pairs ("busy" is the mean during the build):

| Cold build | Total | Narration | `audio.normalize` | Normalize done at | Scenes done at | Cut waits for the mix | After the narration |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Pair 1, `015ab6b` (40 % busy) | 134.1 s | 40.6 s | 37.6 s | 86.5 s | 67.7 s | 18.8 s | 93.5 s |
| Pair 1, now (44 % busy) | **114.6 s** | 38.4 s | 4.3 s | 51.7 s | 65.6 s | none | 76.2 s |
| Pair 2, now (37 % busy) | **105.5 s** | 38.4 s | 3.5 s | 50.4 s | 63.6 s | none | 67.2 s |
| Pair 2, `015ab6b` (26 % busy) | 115.4 s | 30.0 s | 35.6 s | 72.9 s | 53.2 s | 19.7 s | 85.4 s |

- **About 18 s less after the narration in both pairs**, because the cut no longer waits for the mix. The research's 141.7 s part-two build (`83de4ca`, a quiet machine) is now 105.5-114.6 s, most of the rest coming from audio-only assets.
- **On `83de4ca`.** This change against `83de4ca`, in three pairs at 50-70 % mean load, cut `audio.normalize` inside the build from 40.5-45.8 s to 7.7-14.6 s. Alignment's slowdown beside the music conversion cancelled the gain there: 173-220 s against 161-232 s. The first rebuild, at 68 % load, took 163.5 s against the original 141.7 s.
- **The same mix.** This engine renders the original part-two mix (voice at 2872) to the same PCM as `83de4ca` (SHA-256 `13e03599…`), so moving the limited render onto the shared code changes no sample.
- **The delivery.** The one milli-unit moves AAC's overshoot: the delivered MP4 now peaks at 0.0 dBFS with 2 clipped samples, against −0.6 dBFS. Both PCM mixes peak at −1.010 dBFS; a plain AAC 320 kbit/s encode of each gives −0.63 and +0.29 dB. The coordinator normalizes to `peak_dbfs` with no lossy headroom (part two's open issue, queued separately), so which film clips is chance. The speech check still fails on spoken numerals in every build, as before.

Tests:
- a unit test: a capture split into parts replays the plain sum, the limiter stage's own output and scaled input exactly;
- `transitions`: the normalize oracle replays its own unsaturated sums as the engine does, and must match the engine's operations, passes and renders exactly;
- `dynamics`: `measured` and `result` equal `timeline.meters` before and after at hundredths, also with a voice-track limiter (two groups), with two renders;
- `production` (offline): with stubbed stages, alignment must start while the music is still being prepared. With the old order the check fails after 30 s.

These fixtures pass in quick mode: dynamics, transitions, production. No scoring changed. Evidence stays stale until the next thorough run.

## 5 October 2026: audio-only assets for voice-overs and music

Audio tracks have played 48 kHz stereo PCM16 WAV files directly since recording landed. Every other WAV still became a timeline asset with a full-size black FFV1 picture under its sound (`media.prepare`, `src/readiness.rs` `conform_audio`). The part-two research (`demo-progress2-20261005/research/RESEARCH.md`, E1) measured the cost on the 96 s music bed: 14.1 s and 137 CPU-seconds, 117 of them in FFmpeg encoding and decoding the black picture and 19 in the engine hashing about 15 GB of decoded zeros. That is about 1.4 CPU-seconds per second of audio, and the build's alignment waited for it. It was the morning demo's issue 7 in its real form.

The changes:
- **An audio-only asset kind.** A 48 kHz stereo PCM16 WAV with no picture, recognized by an asset `path` ending in `.wav` (`Asset::audio_only`). Audio tracks already read such files with the engine's own WAV reader, so mixing, gain, fades, limiters, nesting, previews, renders and exports needed no change.
- **Pictures refuse it clearly.** A `tracks.edit` that leaves a clip of one on a video track fails with `UNSUPPORTED_MEDIA`, naming the clip and asset and saying to use an audio track. Sequential clips stay accepted as edits, because transcript and outline workflows build sequential projects of WAVs without rendering them. Rendering or previewing one fails with the same message before any source is inspected.
- **`media.conform` writes it.** A recipe for a WAV source without `width` and `height` resamples into a new `.wav`: whole 48 kHz samples up to an hour, no `frame_rate` or `decode`, `audio: resample`. The resampler is the existing one, now a shared function. The engine writes the WAV and hashes it as it writes; the renderer's WAV reader and FFmpeg's decoder must both read back that digest and sample count before it is published. No picture is encoded, decoded or hashed.
- **`media.prepare` uses it.** A 48 kHz stereo WAV that the renderer's reader accepts comes back as it is (`converted: false`), bound to its content identity. Other PCM16 WAVs, mono or stereo at 24, 44.1 or 48 kHz, are resampled to `<name>-prepared.wav`, with no project needed. Transcripts of the file are returned or moved onto the new file as before, so they bind by content. `media.inspect` reports the first kind `ready` with `audio_only: true` and proposes the audio-only recipe for the second. Batch and job claims name WAV outputs `.wav`.
- **Fewer waits on tool launches.** What remains is about eight short FFmpeg/ffprobe launches. For an audio-only source the strict full decode now runs beside the sample listing, and the receipt's tool versions are read while the samples convert.
- **Compatibility.** An explicit `.mkv` `output` with the `project` still makes the older black-picture asset, and existing projects that use such assets render unchanged. On an audio track the older asset plays exactly the same samples as the audio-only asset of the same sound; the fixture checks this.
- **The production coordinator** (`tools/cutbolt_production`) prepares voice takes and the music bed as audio-only assets, without a project. Music no longer waits for the narration, and the kit cache keeps only beds that had to be resampled. Stage keys changed (`asset: audio-only` replaces the frame size), so existing productions rebuild those stages once.

Measured with release builds of `83de4ca` (the part-two engine) and of this change, run alternately.

The research probe (`research/prepare_profile.py`, run unchanged except for its engine path) prepares the 96 s, 48 kHz stereo music bed: 15.7 s and 155 CPU-seconds before, 0.15 s and under 0.1 CPU-seconds now. An interleaved driver over the same harness ran five rounds of the bed and of one narration take, alternating the engines, with the machine 7-55 % busy before each run. Medians:

| `media.prepare` | `83de4ca` | Now |
| --- | ---: | ---: |
| 96 s music bed, 48 kHz stereo (used as it is) | 14.8 s, 150 CPU-s | 0.15 s, 0.1 CPU-s |
| 15.3 s narration take, 24 kHz mono (resampled) | 3.0 s, 25 CPU-s | 0.38 s, 0.2 CPU-s |

The narration take's remaining 0.38 s is mostly tool start-up. An earlier round, before the two overlaps above and under heavier load (23-100 % busy), measured 0.61 s.

**The part-two film, rebuilt cold.** The part-two manifest and `production-config.json` were copied to `demo-progress2-20261005/audio-only-rebuild/`, with `engine` set to this release build, and built into new folders. For a fair comparison, the same rebuild also ran with the part-two engine and its own coordinator (`83de4ca`) and a cold kit cache, in two alternating pairs. Other sessions shared the CPU and GPU. In pair 2 another job held the GPU, and the "before" build's narration took 97 s instead of about 45 s. The narration does not depend on this change, so the comparison also counts from the moment the narration finished:

| Cold build | Total | Narration done | Voice assets | Music | Cut ready, after narration | Finished, after narration |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Pair 1, `83de4ca` (37 % busy) | 203.1 s | 42.5 s | 18.5 s | 36.7 s | 102.0 s | 160.6 s |
| Pair 1, now (28 % busy) | **148.8 s** | 50.3 s | 2.0 s | 0.9 s | 51.0 s | 98.5 s |
| Pair 2, `83de4ca` (62 % busy) | 306.8 s | 97.4 s | 22.5 s | 38.8 s | 113.8 s | 209.4 s |
| Pair 2, now (34 % busy) | **197.3 s** | 48.6 s | 7.2 s | 2.9 s | 76.4 s | 148.7 s |

- **About 61 s less after the narration in both pairs, and 54 s less overall in pair 1.** That is more than the 30 s the research expected. The roughly 250 CPU-seconds of black-picture work also slowed the alignment, scenes, mix and export running beside it: in pair 1 the alignment took 14.3 s instead of 27.9 s.
- **The same film.** The delivered audio decodes bit-identically (`ebdd53f3…`) in all four builds and in the original part-two film. The reviews match: −14.0 LKFS, no black, silence or clipping, timing as the project. The coordinator made 64 engine calls instead of 71-72.
- **Against the original part-two build** (141.7 s on a quiet machine after a restart), pair 1 is not faster overall. Its narration took 11 s longer and its mix 8 s longer under the load. The mix (`audio.normalize`, 49-66 s here) is now the critical path after the narration, and another session is making it faster. The speech check still fails on spoken numerals in every build; that is a separate queued fix.

Tests:
- unit tests for the resampler's exact rounding (ties away from zero) and end clamp, and for the audio-only recipe rules, readiness proposals and job claims;
- the conform fixture compares four audio-only conversions sample by sample with an integer resampler computed from the formula that generated the sources. It reads each one back with Python's own WAV reader and with FFmpeg, and checks preparation as it is, resampled, in batches and with transcripts. It then mixes the assets on placed tracks and compares every rendered and exported sample, including the older black-picture asset in their place. It also runs meters, ducking, normalizing, tightening, beats, outline, an H.264 export and its review on them, and checks the picture refusals and recipe rejections.

These fixtures pass in the quick tier: conform, tracks, recording, proxies, cache_previews, synchronization, transcripts, integration, agent_ergonomics, production (offline part), overlays and multicam. No capability points change: this is a new asset kind and a speed-up, not a new editing checkpoint: this is a new asset kind and a speed-up, not a new editing checkpoint. Evidence stays stale until the next thorough run.
## 5 October 2026: compact MCP results and one wait for many jobs

The morning progress demo's agent received 1,412 KB over 299 MCP calls, about 400k tokens (`C:\DEV\CutboltData\demo-progress2-20261005\research\RESEARCH.md`, C1 and C2). `job.wait` returned 810 KB of it, mostly `scene.render` receipts of 30-42 KB, and `scene.inspect` returned 314 KB over 22 calls. About 90 % of a scene receipt was `timing`: per-layer selected frames, sampled-parameter runs and tile selections, which long scenes push to 0.5-2.7 MB. The agent also made 98 `job.wait` calls for 71 jobs, because a wait took one `job_id`.

**Compact results.** MCP results are now summaries unless a call gives `"detail": "full"`:
- Scene inspections and render receipts replace `timing` with `layers`, one line per layer: the frames it shows, its source frames, how each sampled parameter changes, and its blend, mask, graphics (with clipped pixels) and tilemap. `sources` becomes a count. The per-sample arrays of expressions, motion blur and 3D geometry become counts, without their echoed specifications. Tool versions are left out, and `frame_matte` keeps only `matted_pairs`.
- `graphics.instantiate` and `captions.scene` summarize their inspection the same way, and return their scenes whole.
- `job.wait` and `job.status` summarize a receipt as its command's own result would be.
- `resolved_identities`, `audio.beats` onset times and `export.run` sources become counts. `preview.cuts` leaves out its cell rectangles: each cut already lists its cells and times.

A summarized result names what it shortened in `detail`. Errors are never shortened, and `save_as` files get full results. The CLI keeps returning full results (scripts and fixtures read `timing`) and accepts `"detail": "summary"`. The job store keeps full receipts. `session.get` stays full: it is the call that reads the project document, and `timeline.outline` is the compact read of a cut.

**Batch waits.** `job.wait` takes `job_ids` (up to 64) and `until` (`all`, the default, or `any`). It waits on the jobs' named events, reads receipts once when it returns, and lists each job's status with its summarized receipt, error or progress, plus counts by status. Progress notifications now come from inside the wait, about once a second for one job or for several, instead of from repeated one-second waits.

Measured by replaying the morning's scene traffic against a debug build of this change in a scratch copy of the demo workspace. Bytes are `structuredContent`, as `research/call_logs.py` counts them:

| Traffic | Full | Summary |
| --- | ---: | ---: |
| The 22 logged `scene.inspect` calls | 359,491 B | 36,145 B |
| The 15 caption-scene renders, one `job.wait` with `job_ids` (13.3 s) | 470,222 B | 33,843 B |
| The same 15 renders, one `job.wait` each | 474,505 B | 39,596 B |

The morning spent 30 waits and 715 KB on scene renders. The tool catalog grows by the `detail` property on seven listings and by `job_ids`: with a workspace it is 222,907 bytes (85 % of its budget), and the core catalog is 67,930 bytes.

Tests:
- unit tests for scene, job and list summaries and the `detail` argument;
- the agents fixture checks summaries against full results and the CLI (`mcp.compact_receipts`), and a three-render batch wait, `until: "any"` and the batch argument errors (`jobs.batch_wait`);
- fixtures that compare full MCP results with the CLI now ask for `detail: "full"`.

A full quick verification of this change passed 56 fixtures, with 5 skipped for runtimes not installed here. Two failed under that run's parallel load and passed when rerun alone: reframing (twice), and recording, whose real-time capture reported packet timing gaps (`CAPTURE_DISCONTINUITY`) as it had in the background run of the catalog commit. No scoring changed. Evidence stays stale until the next thorough run.

## 5 October 2026: the MCP catalog regains 16 % headroom

With a workspace, the full MCP catalog was 261,499 bytes against the 262,144-byte budget of `tool_listings_fit_agent_context` (99.8 %), so the next feature adding schema text would have failed it. The evening research (`C:\DEV\CutboltData\demo-progress2-20261005\research\RESEARCH.md`, C3) measured where the bytes went: 30 tools each carried the saved-project reference as two definitions (820 bytes), five caption tools each carried the whole caption document (about 2.6 KB), and schemars adds a numeric `format` and `"default": null` to most fields.

Listings changed; `schema` lookups still return the full definitions:
- A `project` input is one described object: a full snapshot, or `{"project_id", "revision"}` naming a saved revision, with where `store_root` defaults.
- The caption document is a seventh abbreviated shared type, `captions` (definition `CaptionDocument`), like `transcript`: the caption commands return it and agents pass it on.
- Numeric `format` widths and `"default": null` are left out of listings, along with any definitions nothing references any more.
- Shorter wording: the deferred-type stubs, the `save_as` property and the file identity fields.

Measured with `research/mcp_surface.py` on debug builds of `83de4ca` and of this change:

| `tools/list` | Before | After |
| --- | ---: | ---: |
| Full catalog | 253,961 B | 214,352 B |
| Full, with a workspace | 261,499 B (99.8 % of budget) | 220,464 B (84.1 %) |
| Core (`--tools core`) | 73,979 B | 63,427 B |
| Core, with a workspace | 77,112 B | 65,957 B |

The core catalog with a workspace is about 20k tokens instead of 23k. A unit test checks that listings no longer carry the reference definitions or the dropped keywords, and that lookups still return them. No scoring changed. Evidence stays stale until the next thorough run.

## 5 October 2026: hue and saturation in grades, and `cutbolt --version`

The part-two progress demo (main `83de4ca`, `C:\DEV\CutboltData\demo-progress2-20261005`, ISSUES.md 5 and 6) found two gaps.
- **Recoloring took trial and error.** Its effects scene turns Pip's red scarf gold with a `selective_grade`. Grades had exposure, contrast, RGB gains and curves, but no hue control, so the gold came from extreme gains: green ×4, blue ×0.6 and +0.9 stop.
- **No version flag.** `cutbolt --version` was a usage error.

The changes:
- **`hue_shift_mdeg` and `saturation_milli` grade controls.** Both are optional, in `grade` and in a `selective_grade`'s nested grade, and animatable like the other five controls.
  - The hue shift is -360000..360000 millidegrees; saturation is a 0..4000 scale, with 1000 neutral.
  - They run after the channel curves, on HSL of the encoded sRGB color: the same space the selective qualifier measures, so a measured hue difference is the shift to use. Lightness is kept and saturation stops at 1. Achromatic colors are unchanged.
  - The shift is reduced modulo a whole turn in integers, so 0, ±360000 and saturation 1000 keep the exact integer bypass.
  - Sampled parameters report the two controls only when a grade declares them, so existing receipts are unchanged.
  - [GRADING.md](GRADING.md#hue-and-saturation) defines the arithmetic, with the scarf as the worked example.
- **Per-pixel where needed, unchanged elsewhere.** The step mixes channels, so a grade using it is processed per pixel and never through `Processor::tables()`. The row kernel, the per-frame source regions and the once-processed constant chains (`processed_sources`) all take that per-pixel path. Chains without the controls run exactly the code they did before.
- **`cutbolt --version`** prints one plain-text line: the package version and, when the build could ask Git, the commit checked out at build time, with `, sources modified` when engine sources differed from it. `build.rs` records the commit apart from the engine identity, so a new commit of the same sources shares caches. It reruns when this worktree's HEAD moves. `capabilities` reports the same under `build`.

Verification:
- **Rust.** Unit tests cover known rotations (red to green at 120 degrees, gray unchanged, saturation 0 and its limit), whole-turn bypass, bounds, sampling and the scarf value `[224,79,95]` to `[224,196,79]`. The per-pixel chain test now includes hue chains. The compositor tests compare the row kernel, spatial taps and once-processed sources with the per-pixel reference paths for hue grades and a selective hue turn, transparent and opaque, on 1 and 3 threads.
- **grading fixture.** 11 new renders and 7 new rejections; 183 decoded frames in all. The oracle evaluates the step with the CSS Color 4 RGB-to-HSL and HSL-to-RGB equations at 48 digits, not the engine's sextant rebuild. With neutral primary controls a turn is rational in encoded sRGB, so every opaque chart pixel of those cases, over 25 animated frames, must equal the exact result rounded to the nearest level, either way only at an exact half tie. Third turns must permute channels exactly; whole turns, eight eighth-turns and the neutral frame of an animated turn must equal the bypass byte for byte.
- **selection fixture.** 3 new renders and 1 new rejection; a selected pure red turned 55 degrees must give `[255,234,0]`. Values whose exact result is a half-level tie now differ from the oracle by one level, within the documented tolerance.
- **A/B.** `scripts/ab.py` in `C:\DEV\CutboltData\vfx-stress-20261005` rendered nine existing 10 s 1080p effect scenes on release builds of `83de4ca` and of this change, back to back, with the machine 10-100 % busy. Grades, animated grades, selective grades, keys, an eight-effect chain and spatial layers all decoded identically. Engine CPU was at parity, for example 189.9 s and 188.8 s for nine animated selective layers.
  - The first build of this change cost those per-pixel selective layers about 5-7 % more engine CPU, in three interleaved rounds. Moving the hue path out of line, so the per-channel loop inlines as before, removed it: 182.6-184.3 s against 183.3-185.1 s.
  - New hue scenes, added to `perf_scenes()`, show the per-pixel cost. A constant hue grade on eight full-frame layers is processed once per source: 24 engine CPU-s, against 21 for the same grade without hue. Animating that hue costs 891 CPU-s (47 s wall), against 31 for an animated plain grade, about 0.2 µs per pixel for nine transfer evaluations. Animated selective hue turns on eight layers took 313 CPU-s, against 189 without the turn. Like the animated selective grades and keys noted after `83de4ca`, this stays open.
- **Catalog.** Grades live in the abbreviated `scene` stub, so the listings do not grow: `tools/list` measured the same on `83de4ca` with and without this change. On main after the catalog trim, the unit test measures 214,257 bytes (220,369 with a workspace).

The demo's scarf was re-made with the control in `C:\DEV\CutboltData\demo-progress2-20261005\hue-remake` (`remake.py`). It keeps the qualifier and mix curve and replaces the gains with `hue_shift_mdeg: 55000` and `saturation_milli: 1100`, a turn computed from the measured scarf hues of 353.4 and 347.3 degrees.
- **Before.** The gains put the two scarf shades at different hues: `[255,201,100]` at 39 degrees, clipped to full saturation, and an orange `[225,134,78]` at 23 degrees, lifting both lightnesses.
- **After.** The turn gives golds `[231,200,72]` at 48 degrees and `[169,132,44]` at 42 degrees, at the original lightness of 0.594 and 0.418, so the shading survives.
- The old recipe renders identically on the new engine, and the whole 384-frame scene rendered in 4.3-7.4 s on the shared machine. `out/scarf-before-after.png` compares the two stills at 15 s; the folder's README.md tabulates the colors.

This extends C02/C03 controls without new capability points: no criteria, weights or scoring changed. Evidence stays stale until the next thorough run.
## 5 October 2026: numbers in narration keep the speech check

The part-two progress demo (`C:\DEV\CutboltData\demo-progress2-20261005`, ISSUES.md) lost its whole speech check. Recognition wrote "This morning, an eighty second video took over two hours and a hundred and seventy tool calls" with the token "80", and the worker rejected any word with a digit: `UNSUPPORTED_ALIGNMENT_TEXT: Recognized word '80' uses digits; this acoustic profile aligns numbers spelled out`. Any narration with a year, count, price or "10-second" failed the same way.

The changes:
- **English numerals align as they are read.** The worker reads a word's numerals as English words and aligns those letters, with the word separator between spoken words: 80 as EIGHTY, 170 as ONE|HUNDRED|SEVENTY, 10-second as TEN|SECOND. It covers:
  - counts up to 15 digits, with or without grouping commas;
  - years read in pairs (1100-1999 and 2010-2099), ordinals and decades;
  - decimals, clock times, $, £ and € amounts, percentages and signs.

  A leading zero or more than 15 digits is read digit by digit. The word keeps the text the recognizer wrote. Words without digits get exactly the labels they had.
- **Known text** with digits is read the same way and keeps its spelling. This was chosen over rejecting it.
- **Greek still rejects digits.** Greek number words agree with the noun they count and have spelling variants, so a numeral does not determine the letters to align. The message now says so and says to write the number in Greek words. Number signs other than 0-9, such as ½ or ², still reject and are named.
- **Review matching** in `export.review`, and so the production speech check, compares numbers as well as letters. Number phrases, written or spoken, become their values: `a hundred and seventy`, `one hundred seventy` and `170` are all 170. Years read in pairs, and an "and" before a number is ignored. One side may now join up to eight words (was four), and a group is near when the middles of the two whole spans are within the tolerance (was the first words' middles). The comparison counts `number_matches`.
- `src/numerals.rs` reads numerals exactly as the worker does. Both pass the shared table `tests/support/spoken_numbers.json` (79 tokens).

Tests:
- Rust unit tests cover the shared table and number keys. In `cut_review`, the demo's sentence matches 17 of 17 words with 3 number matches; a year spoken in six words matches, and a wrong number does not.
- The worker's pure checks (`WORKER_RULES`) cover the table, labels, unchanged labels of other words, and the Greek and number-sign rejections. They pass here, under Python 3.11 and NumPy 2.4 outside the pinned runtime.
- The `transcripts` fixture's independent reading of the matching rule follows the new rule. A new case reviews a spelled-out script against heard numerals, and against a wrong number. It ran in a scratch copy on Linux with FFmpeg 6.1, where every review assertion passed. That copy skips the decoded-sample comparison, because this FFmpeg differs from the pinned 7.0, and it stops at `job.start`, which requires Windows.
- The `transcription` fixture gains `transcription.numerals_align_and_match_their_spoken_words`: the fixture voice speaks the demo's line, which is recognized, aligned to the same script with digits, and reviewed by `export.review` with recognition. A Greek text with a digit must reject. **Neither has run yet:** this session had no speech runtime, WSL, CUDA or `pwsh`.
- `cargo test` passes here except two Windows-path unit tests, which fail the same way on the parent commit under Linux.

The demo's speech check has not been re-run; its data is on the development machine. No scoring changed. Evidence stays stale until the next thorough run.
## 5 October 2026: hand-written scenes follow the narration

The part-two progress demo (`C:\DEV\CutboltData\demo-progress2-20261005`) replaced its effects scene with a hand-written recipe through `overrides.scenes`. Its `ISSUES.md` records three workarounds:
- Timing the labels to the narration took a build, reading `film/generated/align/effects-*.json`, regenerating the recipe and a second build. Template beats start layers on spoken words; a recipe was fixed JSON.
- The generator recomputed SHA-256 prefixes to name images by the coordinator's copies, `sources/<input>-<sha12><ext>`.
- The overridden scene still needed a placeholder `beat`.

The changes (`tools/cutbolt_production/override.py`, documented in [PRODUCTION.md](PRODUCTION.md#hand-written-scenes)):
- **Cue times in recipes.** A layer's `start`, or any `time` inside a layer, may be `{"cue": "zooms"}` or `{"cue": {"word", "nth", "edge"}, "offset": "-2/25"}`. It resolves from the aligned take with the template's lead and snapping. Offsets are whole frames. A layer's `duration` may be `{"until": <cue time>}` or `{"until": "end"}`, and the scene's `duration` may be `"scene"`, the length the timing plan gives it.
- **Input references.** `{"input": "vfx_stage"}` stands for the full `{path, sha256, bytes}` identity of the production's copy wherever the engine expects a file identity: frame images and mattes, fonts, audio and LUTs.
- **Checked with the manifest.** `check` and `build` read the recipe and check its cue words and occurrences against the scene's script, its offsets, and its input names and file types. `check` lists each cue use. Cues anywhere else are refused with their path.
- **Honest keys.** The scene's key covers the resolved recipe, the recipe file's SHA-256, every cue's time and frame, and every input identity. `show scene:<id>` lists them under `request.override`, and the build log names each cue's frame. A take that moves a cue word, or a changed image, re-renders the scene. A recipe that does not fit its timed scene fails with `OVERRIDE_DURATION` or `OVERRIDE_TIMING`, and an unheard word with `CUE_NOT_HEARD`; nothing is rendered.
- **`beat` is optional** for a scene with a recipe. A beat beside a recipe only sets the timing rules, and overridden scenes no longer draw template art.

Verification: `tests/production.py` adds four offline checks, which need no models:
- 16 manifest rejections of bad cues, offsets, placements and inputs;
- the coordinator's own scene stage resolving a recipe from a stand-in alignment and rendering it, with `scene.inspect` showing each label from the frame of its word (zooms 21, gold 49/51, lands 72, gold 81);
- reuse, and re-keying when a word moves two frames or an image changes;
- four build-time refusals.

The offline part, old checks and new, passed in a Linux cloud session. There a local shim let the manifest take POSIX paths, since the coordinator accepts drive-letter paths only, and a DejaVu font stood in for Arial. The Windows verifier runs the fixture unchanged. The demo rebuild with a simplified recipe has not run yet: its folder is on the development machine, which this session could not reach.

No capability points change: this is agent workflow over existing editing capabilities. Evidence stays stale until the next thorough run.

## 5 October 2026: production builds report their delivery quality

The part-two progress demo (`C:\DEV\CutboltData\demo-progress2-20261005`, ISSUES.md items 3, 8 and 9) found three gaps in `tools/production.py`:
- **A hidden speech-check failure.** The build printed `{"ok": true}`, but recognition had failed (`UNSUPPORTED_ALIGNMENT_TEXT` on the numeral "80", fixed separately). Only `speech_check.summary` said so; stderr and `status` did not.
- **AAC overshoot.** The mix met -14.0 LKFS and a -1.0 dBFS peak, but the delivered H.264/AAC file read -0.6 dBFS. The mix had been normalized straight to the manifest's `peak_dbfs: -1`.
- **A short music bed.** The morning's 80.64 s bed was shorter than the film and simply ended, so the demo had to generate a 96 s variant.

The changes:
- **Warnings.** Each finding is now a warning, `{code, stage, message, detail}`, in four places:
  - a `WARNING` line on stderr;
  - `warnings` in the build's result, or in its error;
  - the build record, which `status` shows;
  - the `final_export` gate.

  Warnings are derived on every build from the stage results and review folders, so reused stages still report. They cover:
  - a speech check that failed, was turned off or compared nothing;
  - one below the new `delivery.review.min_speech_match` (default 0.95);
  - sounds no word covers;
  - in the delivered file: a true peak over `peak_dbfs`, loudness more than 1 LU off target, clipping, black runs, silence under a music bed, timing that differs from the project, and words cut by clip edges;
  - a bed shorter than the film.

  The `final_export` gate lists the delivery's warnings. A decision on it records the warnings it was made over, and a warning that appears later on the same export makes an approval stale.
- **The delivered peak is measured, not assumed.** The mix normalizes 0.5 dB under `peak_dbfs` (the headroom from `2313efd`). It then encodes an audio-only AAC M4A with the export's settings and meters it as `export.review` does. If the true peak is over the target, it lowers the limiter ceiling by the excess plus 0.05 dB, for up to three trials. The final review checks the delivered file's true peak and warns when it is over. The result's `mix` gives the chosen ceiling and the AAC true peak.
- **Music beds.**
  - `check` estimates the film's length: narration at 2.6 words/s (measured for `ryan`) or a supplied take's own length, through the build's timing plan. It warns `MUSIC_MAY_END_EARLY` when the bed is shorter than the estimate, or within 10% of it.
  - The build warns `MUSIC_ENDS_EARLY` once timing is known.
  - New `music.loop: "bars"` repeats the bed from its start on whole 4/4 bars at `bpm`. The repeats are butt-joined clips, the last one fades out, and each is ducked.
- The speech check's receipt now keeps its comparison (`match_ratio`, `matched` and so on). Before, it read them from the wrong level and stored nothing.

Measured on Linux with FFmpeg 6.1.1's AAC encoder at the default 320 kb/s, on original synthetic mixes (not the demo):
- The audio-only M4A decodes to exactly the samples of the MP4's audio from the same snapshot, by SHA-256 of the decoded PCM. So the trial predicts the delivery.
- Overshoot is not monotonic in the ceiling. One harsh bed delivered -0.83 dBTP from a -1.0 dBFS ceiling and -0.70 from -1.5. It delivered +1.54 from -1.85, -1.88 from -2.0 and +0.50 from -2.3. That is why the mix measures each encode instead of trusting a fixed headroom.

Verification:
- **New `tests/production.py` cases.**
  - Speech and review warnings, from engine-shaped review documents.
  - Exact loop placements, and `MUSIC_TOO_SHORT`.
  - The length estimate against a hand-computed 19.68 s for four bed lengths, and `check`'s stderr line.
  - The `final_export` gate's warnings and staleness.
  - The mix through the engine: a looped harsh bed at -10 LKFS. It took two trials here: -1.5 dBFS gave -0.30 dBTP, and -2.25 dBFS gave -1.45. Each correction must follow the rule. The chosen trial's decoded audio must equal an MP4 export's, and its review must read the same true peak.
- **Where it ran.** This session ran on Linux, where the fixture cannot run unchanged, even at the previous commit: manifests require Windows drive paths, the engine's job queue and speech runtime are Windows-only, and the template scenes need `arial.ttf`. The offline part passed here with a local shim that allowed POSIX paths and ran queued commands synchronously; it has not run on the Windows machine.
- **Linux harness.** The real coordinator built a 12.96 s film with a PixelForge stand-in, supplied narration, and stubbed alignment and recognition. It warned `MUSIC_ENDS_EARLY` and `AUDIO_SILENCE` with a 9 s bed, and `SPEECH_CHECK_FAILED` from an injected recognition failure. `status` and the gate showed all three. `music.loop: "bars"` cleared the music warnings. A harsh bed at -9 LKFS showed the trials failing honestly, with every trial listed in `PEAK_OVER_TARGET`.
- **Not done here.** The demo was not rebuilt, because its folder and the companions (PixelForge, Qwen, the WSL speech runtime) are on the Windows machine. `tools/ship.py` could not gate on Linux either: `cargo clippy -D warnings` fails on Windows-only dead code at the previous commit, and the fixtures need Windows. The full companion run of `tests/production.py` is also outstanding.

No capability points change: this is agent workflow over existing editing capabilities. Evidence in `verification/latest.json` stays stale until the next thorough run.
## 5 October 2026: H.264 exports stream straight from the timeline

On main `d3c9140`, the progress demo's final cut (`pip-explainer` revision 13: 80.64 s of 1080p25, 14 FFV1 scene shots, 7 narration clips and a ducked music bed) exported to H.264 in 127 s. Revision 10, with a caption overlay and a picture-in-picture clip, took 112 s warm. Export was the slowest single step of a new video. A logging shim on FFmpeg timed revision 13's warm export:
- The timeline graph wrote the lossless FFV1 intermediate with one encoder thread: 83 s. The graph alone runs in 14 s.
- x264 read the intermediate back with one thread: 49 s.
- Verification decoded the delivery twice, one decode after the other: 3.4 s for the timestamp scan and 6.1 s for the RGB digest.
- Probes, PCM checks and source hashes took about 1.5 s.

The changes:
- **No intermediate for H.264 video.** One FFmpeg run executes the reference graph and converts and encodes its picture and mix. With engine-composited overlays, the encoder fed by the compositor does the same. That run also returns exactly what it encoded:
  - the packed RGB24 frames on stdout, which the engine counts and hashes as they arrive;
  - the PCM samples, written to a scratch file.

  Both must hold exactly the range's frames and samples. The receipt's `verification.timeline_video_sha256` and `timeline_audio_sha256` equal a reference export's decoded digests of the same range. The delivery fixture checks them against its own independently generated frames and samples. Two-pass encodes run the graph twice, and both passes must receive identical frames. Reference and PNG profiles, audio-only delivery and joined chunks (more than 64 clips per graph) keep the verified intermediate.
- **Threaded encoding that still repeats.** x264 runs eight slice threads and the RGB-to-YUV scaler eight threads, both fixed counts. x264's frame threads were tried first and rejected: under the VBV cap their rate control depends on frame arrival timing. Three exports of one timeline gave three different pictures, which `delivery_profiles`' concurrent repeatability check caught. Slices cost about 4 % more bytes at the same CRF. With one x264 thread, the streamed path decodes bit-identically to the old path on the demo (decoded RGB SHA-256 `e07031bb…`), and AAC output is identical, so the threading mode is the only change to delivered content.
- **Parallel verification.** The timestamp scan, the RGB digest, the PCM decode, the AAC priming probe and the source re-hash now run at the same time. The documented checks themselves are unchanged.
- **Overlay decoders read ahead.** The engine compositor used to read the base picture and each overlay from their pipes one after the other, on one thread. For the demo's full-frame caption that is 14.5 MB per frame. Each decoder now has its own reader thread, up to two frames ahead, so the transfers overlap each other and the composition. The composition arithmetic is unchanged, and so is the overlay export's decoded output. In an alternating A/B on revision 10 the read-ahead alone took the export from 42.5 s to 35.6 s (medians of 3).
- `export.run` progress for H.264 video now reads `inspecting`, `encoding` and `verifying`. Peak FFmpeg memory is unchanged at about 4 GB for the demo.

Measured with release builds of `d3c9140` and of this change rebased on main `68dc052`, back to back, while other sessions kept the machine's 32 threads 8-37 % busy before most runs (99 % before one). Source inspections were warm, so the parallel cold inspection that landed meanwhile does not enter. Medians of 3 runs:

| Export of the demo | `d3c9140` | Now |
| --- | ---: | ---: |
| Final cut (revision 13) | 115.2 s | 34.0 s |
| Caption overlay + picture-in-picture (revision 10) | 118.0 s | 30.9 s |

The streamed run is now bounded by FFmpeg's single filter-graph thread (about 145 fps here), not by x264: a `veryfast` preset measured no faster. Verification is bounded by SHA-256 of the 12.5 GB of decoded RGB, at about 1.7 GB/s. For that reason a draft preset and an NVENC option were not added. Segment-parallel encoding was not built either: it would need VBV continuity across joins, and a per-segment digest instead of one.

Not changed:
- the documented checks;
- the output container, timing, profile, level and color tags;
- `export.review`'s inputs.

The FFV1 intermediates of reference and PNG exports still encode with one thread (`render.rs`, `track_render.rs`).

Tests:
- unit tests for the argument layout, plan parsing and the exact pipelined digest;
- the delivery fixture asserts the streamed digests;
- its fault injection now targets the call that writes the staged MP4;
- the dynamics fixture exports its master-limited timeline to H.264, and requires the PCM the encoder received to equal the limiter oracle exactly.

These fixtures pass: delivery, delivery_profiles, overlays, tracks, transitions, native_timing, export_formats, agent_ergonomics, color, hdr, native_scenes, transcripts, queue_recovery and dynamics (quick mode, with the CUDA decode device). No scoring changed. Evidence stays stale until the next thorough run.
## 5 October 2026: spatial and masked layers composite about three times faster

The visual-effects stress test (`C:\DEV\CutboltData\vfx-stress-20261005`, VFX-STRESS.md items 7 and 8) left two compositing paths slow after `d4eaf9d`:
- a bilinear Ken Burns zoom of one full-frame 1080p layer cost about 42 CPU-seconds per 10 s;
- eight rotating 512 px sprites cost about 35 CPU-seconds;
- a layer with a feathered or animated mask took the general per-pixel path, about 24 ms per 1080p layer-frame against 9 ms for the row kernel.

Profiling a single 1080p frame showed that libm `round`, suspected first, was not the cost: a rounding helper without it measured no faster, and was dropped. The time went into per-tap callbacks, 128-bit sums and per-pixel mapping.

The changes. Decoded frames and audio stay identical:
- **Masks by axis.** A rectangle mask's coverage depends on the smaller of the horizontal and vertical edge distances through a ramp that never decreases. Coverage is therefore the smaller of a per-column and a per-row value, inverted when the mask is. Each layer-frame computes those values once (`MaskAxes`) instead of evaluating the mask per pixel.
- **Masked rows.** Unscaled, unrotated normal-blend layers take the row path with a mask too. Runs at full coverage use the unmasked 32-bit row kernel, uncovered runs are skipped, and only feathered edges evaluate `masked_channel`.
- **Spatial taps.**
  - Sums of tap weight x coverage x premultiplied color stay below 2^64, so taps accumulate in 64 bits; only the final blend uses 128-bit products.
  - A spatial layer without per-frame effects reads its stored pixels directly wherever all of a sample's taps lie inside the image. Other pixels call the per-tap closure, as before.
  - Direct reads use exact closed forms. At full coverage the normal blend factors out 2^16, which removes the 128-bit products. One nearest tap reduces to the 32-bit row equation. Four opaque bilinear taps reduce to ((N + 255 x 2^31) >> 32) / 255. Bilinear weights interpolate each row, then the two rows.
- **Axis-aligned mappings.** When a cross term of the inverse mapping is zero (no rotation off the quarter turns), it adds the same signed zero in every row. Each column's source x, or each row's source y, is then rounded once with the same operations. This covers Ken Burns zooms, pans, flips and aspect fits.
- **Row spans.** Each row visits only the columns whose samples can land within a pixel of the crop, solved from the mapping with one source pixel and two destination pixels of margin. A rotated sprite no longer visits the empty corners of its bounding box.
- **Row bands.** Processors that no frame keeps busy draw a large spatial layer's rows in parallel bands. This applies to `scene.still`, static bases, and renders whose few frames or memory-bounded workers leave processors idle.
- **Effects once per sampled source pixel.** Spatial layers whose effects change over time used to evaluate the chain at every tap, four times per bilinear pixel. A straight layer now processes once per frame the source pixels its taps can reach, with the bound mapped back from the destination area, unless those pixels outnumber the taps. It then reads them directly. Effects read only a pixel and its position, so the values are unchanged, and any tap outside the region still evaluates the chain itself.
- **Transfer tables.** Animated keys and selective grades still run per pixel per frame. While a stage has not yet changed a straight pixel, its linear, encoded and quantized values come from 256-entry tables of the same functions. A fully keyed pixel returns transparent before its color is computed.

Release builds of `b546b1a` (before) and this change ran back to back on each scene, on the 32-thread development machine. Other sessions kept it 7-81 % busy. "Engine CPU" is the engine process's own CPU time for a 10 s 1080p25 render. Every row's decoded video and audio hashes match (`scripts/ab.py`, which now records the load and takes the builds from `CUTBOLT_AB_OLD`/`CUTBOLT_AB_NEW`).

| 10 s 1080p25 scene | Engine CPU before | After | Wall before | After |
| --- | ---: | ---: | ---: | ---: |
| Bilinear Ken Burns zoom of one full-frame layer | 39.1 s | 13.4 s | 5.9 s | 5.1 s |
| The same with a constant grade | 42.5 s | 13.6 s | 9.2 s | 6.2 s |
| The same with nearest sampling | 19.9 s | 10.2 s | 5.3 s | 5.5 s |
| The same with a feathered mask, over a full-frame layer | 49.2 s | 23.2 s | 5.8 s | 5.0 s |
| The same as a transparent overlay of an alpha layer | 86.6 s | 47.3 s | 5.3 s | 4.0 s |
| The same zooming a chroma key with animated strength | 618.6 s | 82.7 s | 27.3 s | 5.6 s |
| 8 rotating 512 px sprites | 35.0 s | 13.9 s | 6.3 s | 5.6 s |
| The same with feathered masks | 38.4 s | 20.1 s | 8.8 s | 8.0 s |
| The same with an animated selective grade | 238.0 s | 29.6 s | 14.6 s | 6.4 s |
| 9 animated layers with feathered moving masks | 69.5 s | 26.4 s | 5.7 s | 4.2 s |
| 9 animated layers with still feathered masks | 72.9 s | 25.3 s | 5.3 s | 4.1 s |
| 9 animated layers with hard inverted masks | 49.8 s | 20.6 s | 4.5 s | 3.9 s |
| One full-frame chroma key with animated strength, moving | 141.3 s | 79.5 s | 7.5 s | 6.0 s |
| 8 selective grades with animated mix on animated layers | 628.1 s | 188.0 s | 33.5 s | 12.8 s |

Unchanged paths stayed within noise: the explainer stage, 9 animated layers, animated grades, constant keys and selective grades, and 8 transparent layers. Most 10 s scenes still take 4-8 s of wall time, because encoding and verification cost about 25 CPU-seconds each.

One 1080p frame on one thread, timed with a temporary benchmark (minimum of 15 interleaved rounds):

| Frame | Before | After | After, 16 bands |
| --- | ---: | ---: | ---: |
| Bilinear Ken Burns at 1.1x | 75.8 ms | 24.1 ms | 3.3 ms |
| Nearest Ken Burns at 1.1x | 48.4 ms | 16.0 ms | 2.8 ms |
| One 512 px sprite rotated 37 degrees | 13.3 ms | 5.2 ms | 1.3 ms |

`scene.still` of these scenes (median of 7 alternating runs, `scripts/stills.py`) went from 0.69 to 0.67 s for the Ken Burns zoom and from 0.70 to 0.57 s with its mask. Decoding, hashing and PNG encoding dominate a still.

Tests:
- `spatial` compares `draw` with the former per-pixel implementation, kept as a test reference, on 3,000 random cases. They cover arbitrary and quadrant rotations, scales, flips, pixel aspect, both samplings and edges, viewports, compensation clipping, three blends, opacity, straight, premultiplied, opaque and matte sources, per-pixel and rectangle coverage, direct reads, and one and three bands.
- `composite` checks mask axes against per-pixel coverage for every feather edge, inversion and empty rectangle, and the masked row kernel against `masked_channel` for every alpha.
- `effects` checks the tables bit for bit, and nine per-pixel chains against the former processing on about 56,000 colors at four alphas in each encoding.
- `scene` composes a scene with masked rows, masked, keyed, graded and plain spatial layers, rotated and axis-aligned, opaque and transparent, with and without the static base and processed sources. Every frame equals the former per-pixel paths at one and three threads.
- Three injected faults were caught: a wrong mask axis value, an "unchanged pixel" flag left set after a grade, and a source position off by one in the per-frame effect region.

The scenes, compositing, spatial, keying, selection, grading, temporal, geometry, reframing, stabilization, tracking, captions, overlays, native_scenes, animation, easing and graphics fixtures pass with this change (quick tier, 17 of 17).

**Still slow (open):**
- Keys and selective grades that change over time on non-spatial layers still cost 80-190 CPU-seconds per 10 s of 1080p.
- Transparent scenes compose every layer twice, once for color and once for the matte.
- Encoding and verification take most of a scene's wall time.

These are speed improvements only and earn no capability points. Evidence stays stale until the next thorough run.

## 5 October 2026: recognition vocabulary and uncovered speech

Known-text alignment fixed two demo problems (ISSUES.md 5 and 6) for synthesized narration, but not for real recorded speech:
- **Names.** Whisper small heard "PixelForge" as "pixel forge" and "Cutbolt" as "cut bolt". `export.review` of the final cut matched 136 of 138 words; the two misses were these names.
- **Fillers.** It dropped the "um" from "Play the frames in order and, um, the character moves", so `transcript.fillers` had nothing to remove.

The changes:
- **Vocabulary.** `transcript.transcribe` and `media.transcribe` take `vocabulary`, up to 32 terms.
  - The terms become the recognizer's prompt for every decode of every window. The worker checks the prompt against the recognizer's 223-token context with its own tokenizer.
  - Two to four words that run together into a one-word term are respelled to it (`pixel forge,` becomes `PixelForge,`).
  - Documents record the terms in `recognition.vocabulary`. Documents without a vocabulary serialize, and so fingerprint, as before.
  - With `um` and `uh` in the vocabulary, the recognizer writes fillers down.
- **Uncovered speech.** After alignment, the worker takes the acoustic model's own best label in every 20 ms frame.
  - Letters outside every word's CTC span, grown over their voiced audio, become the document's new optional `uncovered` list, with the letters read (`AM`). They are never words.
  - A sound that runs on into a word without a dip in level is left to that word, as its onset or ending. Word intervals are unchanged.
  - A word's duration was tried as the signal and rejected: the two "and"s of that line last 0.73 s and 0.69 s, and only the first is followed by the filler.
- **Readers.**
  - `transcript.fillers` lists uncovered sounds with their neighbours and `filler_like`, and with `uncovered: true` cuts the filler-like ones like filler words.
  - `transcript.correct` drops a sound once a word covers it, and rebinding moves sounds with the words.
  - `media.transcribe` lists the sounds.
  - `export.review` lists those heard in the cut. It prompts recognition with known terms: the source transcripts' vocabularies, then expected words that look like names. A name split on one side matches the whole name on the other (`joined_matches`).

Measured on the demo with a release build. The machine was shared with other sessions, so the times are not comparable to earlier entries.
- **Narration lines.** Recognized without a vocabulary, s3 left its "um" out, and it was the one uncovered sound: `AM` at 2.615–3.145 s, between "and" and "the". The other six lines had none. With `["um", "uh", "PixelForge", "Cutbolt"]`, s3 reads "and um," and s7 has both names whole; every line kept its other words, and none had uncovered sounds.
- **Fillers on the final cut.** With the timeline's narration assets recognized without a vocabulary, `transcript.fillers` on revision 13 lists that `AM` at 29.735–30.265 s, between "and" and "the". With `uncovered: true` it proposes one ripple cut, 29.72–30.28 s (0.56 s).
- **Review.** `export.review` of the `d3c9140` final cut prompted recognition with the names it found in the transcripts (`PixelForge`, `Cutbolt`). It heard all 138 expected words (136 before), and listed the "um" still in the cut as uncovered speech at 29.735–30.3 s.

Fixture coverage:
- `transcripts`:
  - The review oracle reads the joined-match rule, and a split heard word and an uncovered sound are reviewed.
  - A filler left out of a transcript is listed and then cut exactly as the word was.
  - Words, short sounds and corrections are covered, and invalid vocabularies are rejected.
- `transcription`:
  - Pure checks of respelling, prompts, readings and uncovered sounds.
  - SAPI speech of "PixelForge … Cutbolt" is recognized as "Pyxel Forge" and "cut bolt" without a vocabulary, and as both names with one.
  - A script that leaves out its "um" leaves one uncovered `UM` between "circle," and "and", inside the synthesizer's word clock; `transcript.fillers` cuts it.
  - The English and Greek passages report no uncovered sounds.

Unit tests cover document bounds, stitching, known terms, joined matches and the review's seams. Both fixtures pass in the quick tier; no thorough run was made, so evidence and scoring are unchanged.
## 5 October 2026: a narrated explainer from one manifest

The progress demo (`C:\DEV\CutboltData\demo-progress-20261005`) made an 80.64 s narrated pixel-art explainer in about 2 h 12 min. About 75 of the first 97 minutes were agent authoring: a PixelForge art generator, a scene builder, ten step scripts and about 170 MCP calls. Engine compute for the same steps is now a few minutes. The target was a new video in the same style in under 3 minutes end to end.

The changes:
- **A production manifest, `cutbolt-production-1`.** Script lines, one visual beat per scene, palette, motion patterns, music, timing policy, delivery, review policy and overrides. It is validated strictly; unknown fields fail with the field named. [docs/pipeline/example.production.json](pipeline/example.production.json) is now the six-scene pilot as an executable manifest.
- **The pixel-stage template** (`tools/cutbolt_production/pixel_stage.py`). It is the demo's art generator and scene builder, generalized:
  - original PixelForge recipes, recoloured by named palette slots;
  - eight beat types: title, switch, cards, strip, compare, recolor, travel, end;
  - native 1920x1080 scenes whose cue layers start on the frame where the narrator says their word;
  - one scene per story beat, with no shot slicing, since scenes now last two minutes.
- **A local coordinator, `tools/production.py`, not an engine command.** It runs every stage as a durable, resumable job:
  - PixelForge art;
  - one batched Qwen load for all lines (`tools/qwen_tts_worker.py`), 43 s of speech generated in 30 s against 67 s line by line;
  - voice assets;
  - known-text alignment of those assets;
  - captions;
  - scenes on parallel job lanes;
  - mix;
  - one saved-session revision;
  - export, then a final review, with a speech check on an audio-only render of the same revision beside the export.

  Every stage has a content key and a receipt, so the contract's invalidation rules follow from the data. Interrupted stages resume as the same attempt and collect their engine jobs by request ID. Review decisions bind to identities, and agents' decisions on human-required gates stay advice. [PRODUCTION.md](PRODUCTION.md) records why this is a coordinator beside the engine.
- **One sentence in the MCP instructions** points agents to it. `tools/impact.py` now follows fixtures into the `tools/` modules they import, so changing the coordinator re-runs its fixture.

Verification:
- `tests/production.py` passes its offline checks: manifest rejections, template art and scenes, timing, reconciliation, review gates and worker refusal.
- With the companions installed and `CUTBOLT_PRODUCTION_CONFIG`, `_MUSIC` and `_FONT` set, it also passes a real build, a repeat that reuses every stage, a one-line rebuild and a palette rebuild. The verifier runs the offline part.

Measured: a new six-scene, 44.64 s video from a new manifest took **252.5 s** from the timer start to the reviewed delivery, with **3 agent calls**, on a machine busy with other sessions' verification. That is not under 3 minutes. The H.264 export (96-99 s) and narration (49-53 s) are the largest stages. Details, the other runs and the partial Y cases are in [RESULTS.md](pipeline/RESULTS.md#production-coordinator-5-october-2026).

No capability points change: this is agent workflow over existing editing capabilities. Evidence in `verification/latest.json` is stale until the next thorough run.
## 5 October 2026: cheaper first inspection of a timeline's sources

On main `d3c9140`, the first review of the progress demo's 80.64 s 1080p25 timeline was slow (`C:\DEV\CutboltData\demo-progress-20261005`, FINDINGS-speed.md "Before and after"). The timeline has 14 FFV1 scene shots, a full-length caption overlay, a picture-in-picture clip, 7 narration assets and an 80 s music asset. Its first `preview.frame` took 21.6 s against 0.3–2.5 s warm, and `preview.cuts` 61 s against 6.8 s. The overlay export took 217 s against 106 s.

Measured first, per file, on the demo's sources:
- **Hashing is not the cost.** SHA-256 takes at most 0.04 s per file, and the packet listing 0.1–0.4 s.
- **The strict decode is the cost.** It took 25.5 s for the caption overlay, 20.5 s for the music asset's black picture and 1.6 s per scene shot, decoding one source at a time on one decoder thread.
- **Decoder threads.** `-threads 16` gives byte-identical frame listings and cuts the overlay's decode to 3.2 s. That change shipped separately in `d4eaf9d`.
- **Parallel launches.** Four inspections at a time took the 14 scene shots from 6.3 s to 2.2 s; eight lanes gave little more.

The changes:
- **Parallel inspection.** A render, preview or export gathers the inspections its window needs and makes them up to four at a time, largest file first (`render::inspect_many`, `track_render::Graph::prefetch`). Legacy sequential timelines do the same.
  - The graph then takes each result in its usual order, so errors, receipts and identity checks are unchanged.
  - A request for an inspection already running in the process waits for it, so a file is never decoded twice at once.
  - The queue's render worker, whose control cannot be shared between threads, inspects one source at a time as before.
- **Contact sheets read four cells at a time** (`preview.cuts`, `preview.sheet`). Cells are placed in order, and the first failing cell's error is reported.
- **Audio-only sources are packet-timed.** A source that only feeds audio tracks in a render with pictures is timed from its FFV1 packets, as in audio-only exports, so its pictures are not decoded. Its audio samples come from the same packets either way, and a picture use of the same source still decodes it frame by frame. On the demo, 3,714 frames of the music and narration assets' black 1080p picture are no longer decoded: about 38 s on one decoder thread, or about 10 s on 16.
- **A pass records the inspections it proves** (`render::implied`):
  - an opaque `bgr0` file checked for an opaque track also passes the alpha-overlay check, which only also accepts `bgra`;
  - a frame-by-frame pass whose packets carry the same frame times proves the packet-timed check.

  So a scene rendered moments ago and placed as picture-in-picture, or a conformed file used on an audio track, is no longer decoded on first use. A packet-timed pass never stands in for a frame-by-frame one. Keys still bind the source's SHA-256 and size, so changed bytes are always inspected again, and entries stay in the workspace's `.cutbolt/cache/inspections`.

Two of the suggested ideas were not taken:
- **Hashing during the decode.** It would save at most about 0.2 s per pass over the demo's 375 MB.
- **Verifying only the ranges a preview reads.** After these changes the longest remaining cold inspection is the overlay's 3 s decode, and a range-limited check would make preview verification weaker than the documented frame-by-frame check.

Measured with release builds on copies of the demo workspace. Engines were interleaved step by step, and about half of the machine's 32 threads were busy with other sessions throughout, so warm times are well above the earlier quiet-machine figures. "Cold" means no inspection entry in memory or on disk; warm repeats the call in the same MCP server. Round 1 compared `d3c9140`, `d3c9140` with the 16 decoder threads that `d4eaf9d` shipped, and this change without and with those threads. Round 2 compared `d3c9140`, main `2313efd` and this change on `2313efd`. In round 1, a build and test run of this change overlapped the two baseline `preview.cuts` runs; round 2 had no such overlap, and its numbers agree.

Inspecting all 25 sources of the timeline (`render.plan`, which inspects everything and renders nothing), two runs each:

| | `d3c9140` | main `2313efd` | This change |
| --- | ---: | ---: | ---: |
| Cold | 105.8 s, 97.8 s | 19.9 s, 22.3 s | **5.7 s, 6.2 s** |
| Warm | 0.3 s | 0.3 s | 0.1 s |

Round 2, cold / warm:

| Call | `d3c9140` | main `2313efd` | This change |
| --- | ---: | ---: | ---: |
| `preview.frame` at 58.08 s | 39.3 / 4.6 s | 17.7 / 5.0 s | **10.8** / 4.8 s |
| `preview.cuts`, 13 cuts | 158.3 / 81 s | 129.4 / 85 s | **51.8 / 31 s** |
| Overlay H.264 export | 250.5 / 156 s | 166.2 / 178 s | **151.1** / 174 s |

Round 1, cold / warm:

| Call | `d3c9140` | + decoder threads | This change alone | This change + threads |
| --- | ---: | ---: | ---: | ---: |
| `preview.frame` | 29.6 / 7.1 s | 10.3 / 3.1 s | 21.5 / 2.9 s | **7.4** / 3.3 s |
| `preview.cuts` | 159.1 / 113 s | 139.8 / 78 s | 82.6 / 35 s | **48.0 / 32 s** |
| Overlay export | 285.2 / 167 s | 187.1 / 158 s | 206.7 / 154 s | **151.6** / 148 s |

In round 1 the export's cold penalty (cold minus warm) fell from 118 s to 30 s with decoder threads, and to 4 s with this change on top. In round 2 the warm exports varied by more than the remaining difference. `preview.cuts` is also faster when warm, because its cells are read four at a time. `preview.frame`'s warm path is unchanged. A first preview of a file Cutbolt has just written and verified is a warm one; the `overlays` check below shows no decode for it.

Tests:
- `overlays` gained a check with a logging ffprobe:
  - a freshly rendered opaque scene used as picture-in-picture is not decoded again;
  - a source on an audio track is packet-listed but never decoded;
  - a warm export launches no source inspection;
  - a 10-cell contact sheet read in parallel decodes each of its three sources exactly once;
  - replacing a source's bytes makes it inspected again, and the exports and sheet match the oracle pixel for pixel.

  Against `d3c9140` the check fails: the scene and the audio-only source are both decoded.
- New unit tests cover the implied-entry rules and the lane and per-key limits.

No scoring changed. Evidence stays stale until the next thorough run.
## 5 October 2026: scene work limits sized for the faster compositor

The visual-effects stress test (`C:\DEV\CutboltData\vfx-stress-20261005`, VFX-STRESS.md, "Limits that block real use") found scene limits still sized for the per-pixel compositor that `d4eaf9d` replaced. Its `scene.inspect` probes, on release builds of `c0fe223` and of this change:

| Probe | Before | After |
| --- | --- | --- |
| 11 unchanging full-frame 1080p layers, 120 s | rejected at 68.4 billion pixels | 22.8 million; rendered in 44.6 s, 277 MB engine peak |
| 2 layers, 8 shutter samples, 1080p, 1 s | rejected (2 frames passed) | accepted |
| 480 x 270, 4 samples, 10 s | rejected | accepted |
| 8 samples at 1080p for 120 s, background and one sprite | rejected | accepted, 56.1 billion |
| 8 samples at 1080p for 60 s, background and 12 moving sprites | rejected | 62.6 billion; rendered in 47 s, 525 MB engine peak |
| One 3D plane at 1080p, 1 s | rejected (about 8 frames passed) | accepted; four planes for 120 s too |
| 25 expression nodes over 120 s | rejected (21 passed) | accepted; every graph at every length |
| Tracking a 1920 x 1080 shot | rejected above 512 pixels per axis | tracked exactly, 0.6 s |

No scene the old limits accepted is now rejected.

The changes:
- **Unchanging bottom layers count once.** `static_base` composites them once per render, and now the work estimate counts them once too. A layer counts once when its recipe alone proves it never changes: it lasts the whole scene and shows graphics or one held image (one per tile) that does not end early. It also has no position or opacity curves, expression bindings, mask or effect curves, or spatial curves or compensation, and every layer below it qualifies too.
  - Receipts report `work.static_layers`, and the `LIMIT_EXCEEDED` message names the rule.
  - A render that finds fewer layers to cache than the estimate counted fails instead of exceeding the approved work.
  - `scene.still` caches them too.
- **Shutter samples outside the scene no longer disable that cache.** Those samples show the bare backdrop, as before; the cache compares only in-scene samples. A centered shutter therefore keeps an unchanging background.
- **Shutter sampling shares the scene budget.**
  - The 67,108,864 layer-pixel visit cap is gone. Every changing layer already counted at every sample, and averaging now adds the whole scene once per sample: at 1080p a sample costs about as much as a full-frame layer.
  - Parameter records rise from 32,768 to 460,800, as many as 64 layers keep over 7,200 frames without sampling. With every value changing at every sample, 460,800 records peaked at 1.3 GB and returned a 27 MB receipt, the same as the unsampled scene that was already allowed.
- **Cheaper averaging, identical output.** Each frame reuses one sample buffer and sums in 16 bits (32 x 255 fits), instead of allocating 6 MB per sample and 25 MB of 32-bit sums per frame. A render runs fewer workers if this scratch would pass about 512 MiB. On a 1080p scene of 250 frames with 32 samples, runs alternated:
  - wall time 15.3-16.1 s before, 9.6-10.0 s after;
  - engine CPU 132-141 s before, 42-69 s after;
  - engine peak 955-972 MB before, 426-543 MB after.

  Four shutter scenes decode identically on the old and new builds: centered, open, late-phase and transparent.
- **3D geometry.**
  - Each pixel no longer allocates a hit list, and a perspective camera's per-plane ray origin is computed once per frame from the same values. Decoded output is identical, with about a third less engine CPU at 1080p.
  - A ray-plane visit measured about 40 ns, half a bilinear spatial pixel, so visits rise from 16,777,216 to the scene's 64 billion. One plane at 1080p rendered 10 s in 7.8 s and 30 s in 11.1 s.
  - Node records stay at 32,768: each one reports matrices and plane state, about 0.8 KB of receipt.
- **Expressions.** A render may make 1,843,200 node evaluations and keep 230,400 bound values: every node and binding over 7,200 unsampled frames. Before, the limits were 65,536 evaluations and 8,000 samples. An evaluation measured about 0.4 µs. Read-only `expression.inspect` keeps its 256 times.
- **Tracking and stabilization** read sources up to the scene's 4096 pixels per axis instead of 512, within its 64-million decoded-pixel budget: up to 30 distinct 1080p images.

Measured on the 32-thread development machine while other sessions kept it 45-100 % busy. The measurement scripts and results are in `C:\DEV\CutboltData\vfx-stress-20261005\caps`.

Tests:
- Rust tests cover the counted-once rule against every disqualifier, and the cache finding at least the counted layers. They also check that a centered shutter keeps the cache with identical opaque and transparent frames, and they cover the work arithmetic.
- `native_scenes` gives `expected_work` an independent version of the rule. It accepts 32 still full-frame titles for two minutes, counted once, and rejects them once the bottom one fades.
- `temporal` renders the former 67,108,864-pixel maximum, now counted with its averaging, and all 460,800 records. It checks the 64-billion boundary at 4000 x 2000 with 32 samples (249 frames pass, 250 fail) and rejects 462,848 records and 1,848,000 evaluations.
- `geometry` rejects 64,128,000,000 visits from planes active for only 64 of 501 frames.
- `expressions` inspects the 256-node graph over 7,200 frames and rejects twice that work and 460,800 bound values.
- `tracking` follows a texture across a 1920 x 1080 shot.

These are limits, not features: no capability points change. X01-X03 keep their check names; the record and budget checks inside them changed with the limits. Evidence stays stale until the next thorough run.
## 5 October 2026: long sources are refused, never silently shortened

`media.inspect` readiness recipes and `media.prepare` capped a proposed whole-source conversion at `media.conform`'s 45,000-frame output limit without saying so. A 780 s, 60 fps MP4 got a 750 s recipe, so an agent lost the last 30 s of footage without being told. `color.match` built its target recipe the same way. Found while drafting roadmap item B1.

The changes:
- **Refuse, don't clamp.** The readiness recipe builders (`readiness::conform` and `conform_audio`) now refuse a source longer than 45,000 frames at the proposed rate. Readiness already refused other sources `media.conform` cannot take whole (HDR, non-BT.709 color), so this follows the same rule. The reason gives the source's frames and seconds at that rate, the limit, and the frames and seconds a whole-source recipe would drop. It also says to convert in parts with `source_in` and `duration`.
  - `media.inspect` returns that reason in `timeline.reasons`, with no `conform` proposal.
  - `media.prepare` with `path` fails with `UNSUPPORTED_MEDIA` and the same message before converting anything. With `paths`, the file is listed as a failure and the others continue.
  - The limit is counted at the conversion's rate, so a 13-minute 60 fps clip is refused at 60 fps but fits a 30 fps project. Exactly 45,000 frames is still proposed whole.
- **`color.match` checks its target first.** It builds the target's recipe before decoding histograms or writing the `.cube` table. A target it cannot convert whole is refused with no table left behind. Before, an HDR or BT.601 target was refused only after its table had been written.
- **Docs.** [USAGE.md](USAGE.md) (media readiness and `media.prepare`), [CONFORM.md](CONFORM.md), [LUTS_SCOPES.md](LUTS_SCOPES.md) and the MCP descriptions of `media.inspect` and `media.prepare` state the limit and the refusal.

Tests:
- Rust tests cover the 780 s, 60 fps case with its exact message, a source of exactly 45,000 frames, `media.prepare`'s verified duration at 24 and 25 fps, the 1501.5 s limit at 30000/1001, and an over-long PCM16 WAV.
- The `conform` fixture adds a 781 s, 60 fps FFV1 source (46,860 frames). `media.inspect` gives no recipe, `media.prepare` with `path` and with `paths` fails with `UNSUPPORTED_MEDIA`, and no output is written.

This is a correctness fix: no scoring changed. Evidence stays stale until the next thorough run.
## 5 October 2026: workspace paths come back relative on every platform

With `--workspace`, [AGENT_INTERFACE.md](AGENT_INTERFACE.md#workspace) promises that output files, probed sources and job receipts come back relative to the workspace. On Linux they came back absolute: `media.prepare` with `"path": "clip.mp4"` reported `output` and `asset.path` as `/…/ws/clip-prepared.mkv` (found while writing HYPERFRAMES_STRATEGY.md, roadmap item A7). `Workspace::present` recognized engine paths only by the `\\?\` prefix that canonicalization adds on Windows, so elsewhere it relativized nothing. That covered every publisher, not just `media.prepare`: `media.conform`, `scene.render`, `scene.still`, `export.run`, `preview.range`, `render.run` and `session.backup` outputs, and `media.inspect`'s `path` (its `identity.path` is relative by construction).

The changes:
- **Any absolute path inside the workspace is reported relative**, with `/` separators and `.` for the workspace itself, on every platform. The Windows prefix is still stripped from paths outside it. The documented exception stays: an unmarked path in a project snapshot's `assets` is the caller's own and comes back as written. Diagnostics that name an asset's file, such as `timeline.check`'s `missing_media` detail, report it relative like other paths. The rule lives in `present`, so it covers every command, including `job.wait` and `job.status`.
- **Relative relink paths resolve against the workspace.** `media.relink` and `media.proxy.relink` require absolute paths, so `session.apply` would refuse the relative paths that `registry.relink` and `proxy.relink` now propose (and already proposed on Windows). The workspace resolves them as it does relink `candidates`, rejecting `..`.

Tests:
- The `workspace.rs` reporting test built Windows-only strings and so failed on Linux. It now covers marked and unmarked paths, the workspace itself, a sibling folder sharing its name as a prefix, and project assets. A new test applies a `registry.relink` proposal through `session.apply` and recovers a `session.backup` from its reported identity. It also rejects `..` in a relink path, and checks that a backup without a workspace is still reported absolute.
- `agent_ergonomics` adds `ergonomics.workspace_paths_relative`. In a workspace, no string reported by `media.inspect`, `media.prepare` (with `path` and with `paths`) or an audio-only `export.run` names the workspace folder. The reported files exist, and a session is built from the reported assets. Where jobs are available (Windows), `job.start` with `media.prepare` and its `job.wait` are checked the same way. Without a workspace, `media.prepare` still reports absolute paths. The check fails on the previous engine.

Not verified here: the `job.start` case, because background jobs need Windows, and `captions.render`, which reports through the same step. `cargo test` on Linux still fails `jobs::pool::tests::jobs_writing_the_same_path_keep_submission_order`, as before this change: a case-insensitive assertion sits outside its Windows guard. The evidence is stale until the next thorough run. No scoring changed.

## 5 October 2026: independent jobs run at once in one job root

The progress demo (`C:\DEV\CutboltData\demo-progress-20261005`, ISSUES.md item 15) rendered 16 scenes through `job.start` strictly one after another, about 3 s each. Every job reported "completed after 3.0 s" whatever its length, but there was no 3-second timer:
- The worker drained one row at a time, and the demo client also waited for each job before submitting the next.
- In the first batch, 13 of the 15 scenes are 144 frames, and each took about 3 s of real work. The 96-frame and 192-frame scenes took 2.3 s and 3.8 s, and their captioned versions 2.5 s and 4.0-4.3 s.
- Every wait ended on a 250 ms grid (3.026-3.032 s for 12 of the 13 144-frame scenes, 3.285 s for the other), because `job.wait` polled the store every 250 ms. The supervisor also slept 200 ms between checks, and the command process held its exit for up to 250 ms.
- Each submission hashed FFmpeg and ffprobe (280 MB) inside the store's write transaction. For about 0.2 s, no other submission or progress write could proceed.

The changes:
- **A bounded pool per job root.** The worker runs up to `CUTBOLT_JOB_WORKERS` jobs at once, each on its own thread and store connection (`jobs/pool.rs`). The default is a quarter of the logical processors, 1 to 8. One 1080p scene render keeps about three cores busy (18.8 s of CPU in 6.3 s), so eight fill 32 threads.
  - Heavy jobs (exports, conversions, reviews, caption overlays, cache tasks and reference renders) already run multi-threaded FFmpeg or Rust compositing. Each holds half the pool, so at most two run at once. Speech jobs run one at a time.
  - Jobs start in submission order as room allows. A job waiting for room holds back the jobs behind it, so heavy jobs are never starved.
  - Jobs claim the paths they write: their output file or folder, or their cache database. A `media.prepare` without `output` claims its `<id>-prepared.mkv` names, computed by the function that names them. Overlapping claims run in submission order and are compared case-insensitively; independent jobs behind them still start.
  - Queued commands read saved projects as snapshots pinned at submission and never write a session.
- **No polling granularity.**
  - The worker sleeps on a named event, set by submissions, finished jobs and cancellations.
  - `job.wait` sleeps on an event that the worker sets when its job ends. Both keep a timeout as a safety net.
  - The supervisor wakes on each line from the command, and the command's progress reporter exits at once.
- **Cheaper submission.** Tools are hashed outside the write transaction, side by side, and once per process for each path, size and modification time.
- **Unchanged contracts.** `request_id` replay, output reservation, the 32-job limit, cancellation, the stall watchdog, the deadline and publication recovery all apply per job. Recovery reconciles every job a dead worker was running. Progress writes are best effort, so a store busy with other jobs never fails a job.
- **Agent guidance.** The MCP instructions and the `job.start` description now say to queue independent jobs before waiting on any. The description also drops the stale "a running one finishes".

Measured with release builds (`d3c9140` before) on the demo's 16 native 1080p scenes: 14 `-cap` scenes, `end-final` and `s5b-before`. They were submitted over MCP one after another and then waited for in order (`job.wait` with progress, as the demo client does), with outputs under `ws/remeasure/pool-*`. Other sessions loaded the machine throughout. "Busy" is the share of the 32 logical processors in use in the 3 s before each run; runs listed together ran back to back.

| Measure | Before | After |
| --- | ---: | ---: |
| 16 scene jobs queued together, all finished | 98.1-132.7 s in 4 runs, median 107.5 s | 21.5-27.4 s in 5 runs and 51.5 s in one under heavier load, median 26.4 s |
| Processors already busy before each run | 53-88 % | 71-100 % |
| `job.start`, mean of the 16 submissions | 0.24-0.40 s | 0.06-0.15 s |

Before and after runs alternated. Pools of 4, 8 and 16 took 30.8, 34.3 and 29.0 s (52-76 % busy): other work already saturated the processors, so the default stays at a quarter of them.

Per-job overhead, from a one-frame 64x36 scene with a cold worker each time (median of 8): `job.start` took 0.22 s before and 0.025 s after. The queued job took 1.24 s before, always on the 250 ms grid (1.22-1.25 s), against a 0.58 s synchronous render. After, it took 1.07 s against 0.61 s. A cold worker's first job still pays a process launch and one hash of both tools (about 0.1 s side by side).

Tests:
- `queue_recovery` adds a pool case with real processes. A pool of four runs two reference renders at once, and a job writing the same file in another letter case waits for the first even with room free, while a later independent job overtakes it. Killing the worker interrupts both running jobs; recovery requeues each, and all complete with exact pixels and samples. The waiting duplicate then fails with `OUTPUT_EXISTS`, leaving the first output unchanged.
- The 32-slot case, and the foundational job checks in `tests/agents.py`, pin `CUTBOLT_JOB_WORKERS=1`, because they check one worker's serial queue.
- Rust tests cover the pool size, weights and first-come room, path claims (case, folders, prepare names), speech exclusivity, request classes, and reconciling several running rows.

The "Asynchronous job control" acceptance text no longer says "one render per root". The new pool evidence is listed under E04's bounded concurrency. No scoring changed. Evidence stays stale until the next thorough run.

## 5 October 2026: timeline limiters, so normalizing reaches its target

On the progress demo (`C:\DEV\CutboltData\demo-progress-20261005`, ISSUES.md item 17), `audio.normalize` was asked for -14 LKFS and stopped at -20.95 LKFS with `limited_by: "peak_ceiling"`. The Qwen3-TTS narration peaks near -2 dBFS while measuring about -21 LKFS, and timeline tracks had no limiter or compressor. The mix-recipe compressor (`audio.render`) would have meant rendering, conforming and swapping every line and binding its transcripts again.

The changes:
- **Track and master limiters on placed-track timelines.** The `tracks.edit` operation `audio_dynamics` sets a limiter on an audio track or on the master mix: `ceiling_dbfs` -20..0, `lookahead_ms` 1..20 (default 5) and `release_ms` 10..2000 (default 150).
  - Snapshots store them as a track's `dynamics` and the arrangement's `master`, with schema, validation and session diffs.
  - Interchange export reports them as a blocking loss. Video tracks, locked tracks and sequence definitions refuse them.
- **An original limiter in integer arithmetic** (`dynamics.rs`, [AUDIO.md](AUDIO.md#timeline-limiters)).
  - It finds peaks at the samples and at 4x-interpolated points between them (a 16-tap Kaiser-windowed sinc), takes the smallest required gain over the lookahead, releases linearly and ramps down linearly over the lookahead.
  - No output sample passes the ceiling, and peaks between samples are held at it as well.
  - A sample's output depends only on a bounded neighbourhood (lookahead + release + 15 samples before, lookahead + 16 after). Every render, range, preview, chunk, export and meter therefore gives exactly the whole timeline's samples.
  - FFmpeg writes each group's unsaturated sum as exact 32-bit codes. The engine limits, adds and saturates them, and hands the render graph a PCM16 input.
  - Timelines without limiters render exactly as before, and a limiter that never engages changes no sample.
  - Receipts and meters report each limiter's `max_reduction_db`, `reduced_seconds` and `reduced_fraction`.
- **True peak.** Every stereo meter (`timeline.meters`, `audio.inspect`, `export.review`) now reports `true_peak_dbtp`, using the limiter's interpolation filter. On steady tones it agrees with FFmpeg's `ebur128` to that meter's 0.1 dB display; an abruptly starting 7 kHz tone reads 0.29 dB lower, because the filters ring differently. No certification is claimed.
- **`audio.normalize` proposes the limiter it needs.** When the peak ceiling would stop the gain, the proposal adds a master limiter at the ceiling and keeps raising the gain through it.
  - Each step is measured: up to six more passes, stepping by the measured loudness slope.
  - A true peak over the ceiling lowers the limiter's ceiling by the excess.
  - The gain stops at `max_limiting_db` of gain reduction (default 12).
  - `limiter: false` keeps the old behaviour. `measured` and `result` now include true peak, and `result.dynamics` gives the limiter's gain reduction.

On the demo, with a release build, on a copy of the saved session store. The demo's own store is unchanged (same SHA-256), and the outputs are in `ws\remeasure\timeline-limiter`:

| Final cut (revision 13) | Before | With the proposal applied |
| --- | ---: | ---: |
| Integrated loudness | -20.95 LKFS | -14.01 LKFS |
| Sample peak | -1.00 dBFS | -1.01 dBFS |
| True peak | -0.99 dBTP | -1.01 dBTP |
| Voice track / music track, each alone | -19.99 / -32.01 LKFS | -13.10 / -24.06 LKFS |
| Master limiter | none | ceiling -1.01 dBFS, 5 ms lookahead, 150 ms release |
| Gain reduction | none | at most 7.97 dB; active 7.76 s of 80.64 s (9.6 %) |

The proposal took 36.4 s and 5 measured passes, with other sessions running. It has three operations: the limiter, with its ceiling lowered 0.01 dB to keep the true peak under -1 dBTP, and `clip_audio` levels raised by 7.95 dB on 8 clips. The delivered H.264/AAC export (278 s on the loaded machine) measures -14.0 LUFS in FFmpeg's `ebur128`, but -0.8 dBTP: AAC encoding adds about 0.2 dB of overshoot. Normalizing with a -1.5 dBFS ceiling instead gives -14.03 LKFS on the timeline. Its audio-only AAC exports then read -14.1 LUFS, and -1.0 dBTP at 192 kb/s or -1.2 dBTP at 320 kb/s (`aac-check` in the same folder). Lossy deliveries that must stay under -1 dBTP should therefore normalize to a ceiling about 0.5 dB lower.

Verification:
- **New `tests/dynamics.py`.** An independent integer oracle of the documented design checks every sample of 13 renders (1,501,440 stereo sample frames):
  - master limiters, track limiters, and track and master limiters together;
  - three preview ranges, a meter range and a chunked 70-clip render;
  - inactive limiters, which change nothing.

  It also checks true peaks against its own filter arithmetic and FFmpeg's `ebur128`, the normalize proposal and its applied result, saved sessions and the schema, and 13 rejected requests.
- **Changed fixtures.** `audio_processing` now compares true peak with `ebur128`. `transitions` keeps its exact normalize oracle by passing `limiter: false`.

Quick runs of `dynamics`, `audio_processing`, `audio`, `tracks`, `transitions`, `track_edits`, `sequences` and `sessions` passed; `interchange` was skipped because no OTIO runtime is configured here. The 139 Rust tests, clippy and rustfmt pass.

No scoring changed; evidence stays stale until the next thorough run.

Follow-ups:
- A track compressor, the issue's optional part, is not implemented; the limiter alone reached the target here.
- Limiters inside nested sequences.
- Headroom that knows the delivery codec.
## 5 October 2026: scene compositing four to twelve times faster

A visual-effects stress test (`C:\DEV\CutboltData\vfx-stress-20261005`) rendered 10 s 1080p25 scenes. They covered:
- 1 to 64 full-frame layers, both static and animated;
- grades, selective grades, chroma keys and 8-effect chains;
- Ken Burns zooms and rotations, feathered masks and 40 caption cues;
- 60 fps, 120 s and transparent output.

Most of the time went into avoidable work:
- **Verification:** strict output verification decoded every frame on one ffprobe thread below 8 MP. A 10 s scene spent 27 s verifying after 6 s of rendering; a 120 s scene spent about 190 s of its 286 s.
- **FFV1 slices:** 1080p was encoded with four FFV1 slices, which caps both encoding and decoding at about four threads.
- **Per-pixel cost:** a full-frame layer cost about 33 ms per frame per core. Every channel did a 64-bit division by a run-time denominator, and every pixel did two scale divisions and closure calls.
- **Repeated work:**
  - Unchanging layers were composited again in every frame.
  - Effects with fixed settings ran per pixel per frame: a 1080p chroma key cost about 590 ms per frame, a selective grade about 320 ms.
- **Batching:** frames were composed in batches of at most 16 threads, each batch joined before being written.

The changes. Decoded frames and audio stay identical:
- **Verification and slices.** Strict verification decodes with 16 threads at every size. FFV1 writers use 16 slices for frames of 640 x 360 and larger (DEVELOPMENT_LOOP phase 3, item 2).
- **Static base.** The bottom layers that look the same in every frame (same source frame, tile frames and sampled values) are composited once per render. Each frame continues from that result.
- **Processed sources.** Layers above that base whose effects sample the same values throughout have their straight source images processed once per render, in parallel, within the decoded-pixel budget. Animated effects still run per frame.
- **Row kernel.** Unscaled, unrotated, unmasked normal-blend straight layers (and premultiplied layers without effects) blend whole rows in 32-bit arithmetic. (X + 32512) / 65025 equals the reference rounding because 65025 is odd.
- **Constant divisors.** The general blend divides by a constant per mode. Spatial blending shifts out its 2^48 factor and then divides 64-bit values by a constant.
- **Worker pool.** One worker per logical processor (at most 64) takes the next frame. Workers run at most two frames each, within about 256 MiB, ahead of an in-order writer.

Release builds of `d3c9140` (before) and this change, run back to back on the same machine while other sessions were working. "Compositing" is the engine's own CPU time. Every row's decoded video and audio hashes match.

| 10 s 1080p25 scene | Before | After | Compositing CPU |
| --- | ---: | ---: | ---: |
| 1 opaque layer | 35.1 s | 6.0 s | 11 s → 1.6 s |
| 64 static full-frame layers | 52.1 s | 4.2 s | 557 s → 1.7 s |
| 33 animated full-frame layers | 33.5 s | 7.6 s | 279 s → 78 s |
| Stage: 7 static layers, 12 moving sprites, 4 timed cards | 32.7 s | 6.9 s | 82 s → 6.5 s |
| 8 transparent layers | 25.3 s | 3.8 s | 145 s → 5.4 s |
| 8 selective grades on animated layers | 90.6 s | 7.7 s | 682 s → 25 s |
| One full-frame chroma key on a moving layer | 36.4 s | 4.3 s | 157 s → 3.7 s |
| Bilinear Ken Burns zoom of one full-frame layer | 39.1 s | 10.8 s | 47 s → 42 s |
| 9 animated layers with feathered moving masks | 38.9 s | 9.5 s | 101 s → 76 s |

Unit tests cover:
- the row kernel against the reference channel for every alpha, destination and a range of sources and opacities;
- cached and uncached composition producing identical frames (opaque and transparent, with graded, keyed, moving and fading layers);
- the ordered writer under uneven work, a frame error and a write error.

The scene, compositing, effect, spatial, temporal, geometry, graphics, caption, overlay and FFV1 fixtures are run with this change. These are speed improvements only and earn no capability points.

**Still slow or limited (open):**
- **Per-pixel cost:**
  - Spatial layers cost about 166 ms per full-frame 1080p frame.
  - Masked layers and animated per-pixel effects still take the general per-pixel path.
- **The work budget counts static layers in every frame.** 11 static full-frame layers for 120 s are rejected, although they now composite once.
- **Feature caps that bind at 1080p:**
  - shutter sampling: 1 s at 1080p is rejected;
  - 3D geometry: one plane for 1 s at 1080p is rejected;
  - expressions: 25 nodes over 120 s are rejected;
  - tracking and stabilization read sources at 512 px or less.
- **Encoding and verification** now take most of a scene's wall time: about 25 CPU-seconds each per 10 s of 1080p.

## 5 October 2026: fast overlay exports, stoppable queued commands, cached source checks

The progress demo (`C:\DEV\CutboltData\demo-progress-20261005`, ISSUES.md items 18-21) could not export its 80.64 s 1080p25 timeline, which has a full-length caption overlay and one picture-in-picture clip:
- The export wrote nothing in 15 minutes. FFmpeg held 6 GB and then sat at 0 % CPU.
- `job.cancel` could not stop it, no timeout fired, and progress read 0 of 0 frames throughout.
- The cause: overlays were composited inside the FFmpeg graph with per-pixel `geq` expressions on one filter thread, and the caption input was re-trimmed once per picture segment, which forced FFmpeg to buffer it.

The changes:
- **Overlays are composited by the engine.** FFmpeg decodes the opaque base picture and each shown overlay clip (cropped) to raw planar RGB. The base is cut only where the visible opaque content changes, and every source is read once, in order. The engine applies each clip's shrink, opacity, placement and the exact straight-alpha over in parallel row bands, and streams frames to the encoder while the decoders run (`track_composite.rs`).
  - The equation, rounding and visibility rules are unchanged. Unit tests compare against an independent reference for every alpha class, and the overlays fixture's zero-tolerance checks pass.
  - Overlay tracks inside nested sequences still composite in the graph.
- **Queued commands can be stopped.** A `job.start` command now runs in its own process inside a Windows job object (`cutbolt job-command`).
  - `job.cancel` stops a running command within seconds: its tools are killed and its partial files removed. Anything still alive 20 s later is terminated.
  - A watchdog stops a command whose processes together use less than 1 s of CPU in 180 s without progress (`JOB_STALLED`), or one that runs longer than 12 hours (`JOB_TIMEOUT`).
  - Streamed tools (frame decoders, encoders fed on stdin) also stop on cancellation now.
- **Progress.** Commands report their phase and frames to the queue. `export.run` reports `inspecting`, `rendering`, `verifying`, `encoding` and `verifying`, with `total_frames` set, so a slow export can be told from a hung one.
- **Cached source checks.**
  - A passed source inspection is remembered by the source's SHA-256 and size, the inspection's parameters, the ffprobe build and the engine build. Every command still hashes its sources, but identical bytes skip the frame-by-frame decode (about 23 s for an 80 s 1080p source).
  - With a workspace, entries persist in `.cutbolt/cache/inspections`. Renders, scene compiles and caption overlays record their verified outputs there, so a new asset is not decoded again when a timeline first reads it.
- **Leaner export verification.**
  - An export's lossless intermediate is checked from packets: the encoder decodes it in full anyway, and the delivered file is still decoded and verified frame by frame.
  - The decoded-video digest is hashed by the engine (`digest::raw_sha256`) instead of FFmpeg's hash muxer.
  - Encoders get time in proportion to the export's length.

Measured on the demo timeline with release builds (`ffc2ec9` before), on the same machine while other sessions were working. "Cold" means no cached inspections; "warm" means the sources were inspected before.

| Case | Before | After |
| --- | ---: | ---: |
| Whole 80.64 s H.264 export with both overlays | did not finish (about 2 h projected) | 216.8 s cold, 106.1 s warm |
| Whole export without overlays | 243-264 s in the demo, 335.1 s here | 113.9 s warm |
| 3.84 s range with the caption overlay | 338.2 s | 55.0 s cold, 9.0 s warm |
| 3.84 s range without overlays | 46.5 s | 8.7 s warm |
| `preview.frame` with both overlays | 32.0 s | 8.1 s cold |
| `preview.frame` without overlays | 1.9 s | 0.5 s |
| `preview.cuts`, 13 cuts | 58.7 s | 40.8 s cold, 10.1 s warm |

A queued overlay export reported its frames throughout: 2016 frames composited in 61 s, encoded in 37 s, and completed after 112.8 s. A `job.cancel` sent 12 s into another one stopped it 0.6 s later, with no output or partial files left.

`queue_recovery` now also checks:
- a queued export's relayed progress;
- cancelling a running export whose tool hangs;
- the stall watchdog failing a hung export.

Each case leaves no output and no leftover files. No scoring changed. The quick suite passed 54 fixtures, with 7 skipped for runtimes that are not configured here. Evidence stays stale until the next thorough run.

Follow-ups:
- `captions.render` and `media.transcribe` still report no frames.
- Overlays inside nested sequences still use the slow in-graph path.

## 5 October 2026: beats on arranged music

The demo's own music bed is exactly 125 BPM: a kick on beats 1 and 3, a snare on 2 and 4, hats on eighths, a sixteenth-note arpeggio, and two bars without drums before them. `audio.beats` read it as 62.82 BPM, half tempo with a 0.5 % period error. Its grid started 0.14 s early and drifted up to 0.24 s off, so beats missed the onsets they should have snapped to. There were three causes:
- The tempo came straight from the autocorrelation, which favours the slowest level that repeats: here, the kicks' half notes.
- The phase came from a whole-hop search at that inexact period, so the grid drifted across the file.
- 60 of the 388 onsets were not attacks: a kick's low tail beating against the 10 ms window made one-hop dips and recoveries.

The fixes:
- A rise now counts at most 3 dB more than the rise from two hops back.
- Beats are tracked by dynamic programming at the autocorrelation's period. The tracked beats on onsets fit the exact period, with a phase per run of steady tracking.
- The result lists the metrical levels in the tempo range that the onsets support. The one nearest 120 BPM is `tempo_bpm`, and the others are in `tempo_alternatives`; a slower level names the beat its accent falls on.
- `beats.on_onsets` counts the beats that landed on onsets.

The half/double caveat in the documentation now states the rule. The tempo is between 85 and 170 BPM whenever the music has a level there, so a 70 BPM ballad with steady eighths reads as 140, and fast music near 175 BPM reads at half.

On the demo file the result is now:
- 125 BPM, with a period of exactly 12/25 s;
- 168 beats, all on the true grid and 164 of them on onsets;
- the 62.5 BPM half-note level listed, starting on the kick;
- 328 onsets, all on eighths.

The audio-processing fixture now generates arranged music: that backbeat arrangement at 125 BPM, with intro and outro bars without drums, and four on the floor at 100 BPM after a 0.35 s lead-in. It checks exact beat times, frames, onsets and levels, a range, and a narrowed tempo range. The previous engine fails it at 62.5 BPM. Outside the fixture, a sweep of both arrangements from 72 to 174 BPM kept every beat within a hop of the reported level's true grid, and an hour of looped music took about 6 s in a debug build. No scoring changed.
## 5 October 2026: speech in few launches

Every speech window was its own isolated worker launch: WSL, Python and the models started again each time, about 20 s of each short narration line (ISSUES.md 4). The worker protocol is now `cutbolt-transcription-v2`. A launch takes up to 16 owned analysis files with their own text or recognition, loads each model at most once, and returns each item's result or error. Failures of shared setup, such as the runtime, device, models or memory gate, still fail the launch. The supervisor bounds the result at 16 MiB and removes its numbered analysis files. `transcribe::run_many` groups requests into launches of at most 16 items, a 64 KiB request and ten minutes of audio, and binds each result as before. A single request is a batch of one. `media.transcribe` sends every window of a file together and takes `paths`/`texts` for several files in one job. `export.review` sends its windows together too.

Measured on the demo narration: seven lines took 15 s aligned and 25 s recognized, against about 22 s each before. A two-window 136 s file took 37 s. The `transcription` fixture recognizes the English fixtures together with the per-file gates, aligns a script through `paths`/`texts`, and rejects five invalid forms. The supervisor guard's oversized output now exceeds the 16 MiB bound. No scoring changed.

## 5 October 2026: transcripts follow conversions

In the demo, every narration line was recognized twice: once as its WAV, and again as the MKV conformed from it (ISSUES.md 8). Transcripts bind to content, so they no longer matched. `media.conform` and `media.prepare` now take the source's `transcripts` and return them moved onto the output. `transcript::rebind` shifts words and acoustic evidence by the recipe's `source_in`, clips the analysed range, drops words outside it and makes the next revision under the original's fingerprint. Only forward, unit-speed recipes qualify, and the check runs before converting, so a refused request publishes nothing.

The MCP server instructions now mention WAV voice-overs and music in `media.prepare`, and the filler lift and windows.

The `transcripts` fixture conforms a two-second part of its source starting at 1 s. It checks the three moved words, their 1 s shift, the new binding and range, and that `timeline.outline` reads the words from the part. It also refuses a double-speed recipe and another file's document without publishing. A unit test covers rebinding and the out-of-range refusal. No scoring changed.

## 5 October 2026: windowed tightening and lifted fillers

The demo's finished timeline had a music bed and scenes cut on the bar line. `audio.tighten` proposed ripple-deleting 16.4 s across every track, music and picture included, and filler cuts would have done the same (ISSUES.md 25).
- Both proposals now take a `start`/`end` window.
- `transcript.fillers` takes `lift` with `track_ids`. Each filler is silenced on those tracks by an empty `overwrite`, and nothing moves.

The `transitions` fixture checks windowed and edge-clipped pause cuts and an empty window. The `transcripts` fixture checks the lift operation and renders it exactly: the picture is unchanged, and the audio is silent only over the filler. It also checks the required tracks and a window that excludes the filler. No scoring changed.

## 5 October 2026: fixes from the PixelForge progress demo

The narrated pixel-art demo (`C:\DEV\CutboltData\demo-progress-20261005`, ISSUES.md) recorded 25 problems. Four are being fixed in their own sessions: overlay export speed and cancellation, caption spacing and music labels, beat detection, and the scene caps. This batch fixes several others:
- **Capabilities.** The essentials no longer print "null fps" or the per-graph 64-clip limit as a timeline limit.
- **`timeline.check`.** It warns when transcripts were given but none matches an asset, and notes each unmatched one. Before, it reported "0 warnings" and silently skipped its word check.
- **`TEXT_OVERFLOW`.** The message names the line, its text, its measured width or baseline, and the box.
- **Type errors.** A type error says what was actually given when serde's wording would mislead. For example, a `graphics` list instead of an object read "invalid type: map, expected variant identifier".
- **Audio-only sources.** `media.inspect` proposes a silent-picture conform recipe for PCM16 WAV voice-overs and music, and `media.prepare` with a project runs it.
- **Faster `media.conform`.** Decoded content is hashed in Rust (`digest.rs`) instead of by FFmpeg's hash muxer (39 s for 80 s of 1080p). Output timing comes from packets instead of an `ffprobe -show_frames` decode (23 s). Audio-only sources get their black picture from FFmpeg's color source. The demo's 80.64 s music bed converts in 27 s instead of 74 s.

Fixtures: `transcripts` covers the unmatched-transcript findings, and `conform` covers WAV preparation and the proposed recipe, with each conversion identical to running its recipe through `media.conform`. Unit tests cover the overflow message, the type-error note, the audio recipe at 25 and 30000/1001 fps, and the zero-frame digests. No scoring changed.
## 5 October 2026: speech over music, caption spacing and known-text alignment

The 5 October demo found three speech problems (its issues 14, 23 and 16):
- **Caption spacing.** Recognized words kept the recognizer's leading space (`" tiny"`), and `captions.draft` joined them with another one. Every cue started with a space and had double spaces between words, and `timeline.outline` showed the same. Recognition now writes word text without surrounding whitespace, and `transcript.correct` trims every word of the revision it writes. Outline, captions, review, fillers and assembly read older documents' words trimmed, so those documents keep their fingerprints. `captions.draft` joins words with single spaces whatever the transcript holds.
- **Speech over music.** `export.review` with a runtime, and `media.transcribe` of the delivered cut, failed after about 30 s with `UNSUPPORTED_ALIGNMENT_TEXT`. The cause was not a `[Music]` label. A music bed leaves no quiet gap, so every recognition window ended at a fixed 12 s cut. One cut split "float" into "f" and a lone "-", and that punctuation-only token failed alignment. The fixes:
  - A window without a quiet gap now ends at the widest gap between the words recognized over the next 14 s (`word_gap`).
  - Bracketed annotations and music symbols are removed before alignment and reported as `non_speech` notes.
  - A punctuation token written against a word joins it.
  - A letter the acoustic vocabulary lacks aligns as its base letter.
  - Audio without speech gives an empty document instead of an error.
  - `export.review` checks the runtime configuration before decoding. If recognition fails after that, the picture, sound and timing sections are kept and the speech section reports the error.
- **Known text.** `transcript.transcribe` and `media.transcribe` accept `text`, such as a synthesized narration's script. Its words are aligned to up to 120 s of audio instead of recognized (profile `local-en-el-align-v1`, words without recognizer confidence).

On the demo's delivered cut, `media.transcribe` now returns 140 words, and every recognition window ends between words. `export.review` matches 136 of 138 expected words; the two differences are "PixelForge" and "Cutbolt" (demo issue 6). Aligning the scripts of two narration lines keeps "um," as its own word and "PixelForge" and "Cutbolt" as written.

Fixture coverage:
- `transcripts` adds recognizer-spaced words to its outline, caption, review and correction cases. A recognizer that fails after the configuration check leaves a complete review, and invalid `text` is rejected before any media work.
- `transcription` checks that words carry no whitespace, and aligns the English and Greek pilot passages within the recognition onset gates. It adds isolated words over a two-tone bed with no quiet gap: the old 12 s cuts would split four of them, and every window now ends between words, with all 54 words recognized. Pure checks cover annotation handling and word-gap cuts.
- The whole-file check that came with `media.transcribe` applied the 10% word-error gate to the six isolated Greek words. The recognizer hears "κύπος" for "κήπος" there, also with the previous engine, which is 17%. Gates skip this fixture when no speech runtime is configured, so the check had not run since it was added. It now uses the gates of direct recognition: every word found, and the error rate only for passages.

No scoring changed.
## 5 October 2026: two-minute scenes with 64 layers

The 5 October demo hit two scene limits. Scenes stopped at ten seconds, while its narration lines ran 7.8-11 s and the pilot brief plans 12-second scenes. Every story scene was therefore split into two shots, with loop phases and keyframes re-cut by hand across the split. A plain stage background took 7 of the 16 allowed layers, and `captions.scene` adds a layer per cue, so busy shots ran out. The ten-second cap dated from compositing into a raw-video scratch file; frames have since been streamed to the encoder.

Scenes now last up to 120 seconds at their frame rate (3,000 frames at 25 fps, 7,200 at 60 fps) and hold up to 64 layers:
- A compositing budget replaces the length cap as the bound on work. `scene.inspect` reports each scene's `work.composited_pixels`: every active layer's clipped destination, plus a tilemap's canvas, summed over frames. Scenes above 64 billion fail before any media is read, naming the layer with the largest share. The budget keeps the worst case near the former 600-frame, 16-layer, 8-megapixel limit.
- Text and shape layers keep only the bounding box of their visible pixels, and only that counts toward the 64-million decoded-pixel budget. Forty full-frame 1080p caption cues use 4.4 million instead of 83 million. Compositing a fully transparent straight-alpha pixel never changes the destination, so output is identical; layers with effects or spatial transforms, and 3D scenes, keep whole canvases.
- The decode that verifies every reference output frame now gets a timeout that grows with frames and pixels. A two-minute 60 fps output takes 85-100 s to decode, close to the former fixed 120 s.
- Each tile's `tile_selected_frames` is run-length encoded like the other per-frame arrays. Per-frame parameter records shrank from about 370 bytes to 64 by boxing the rarely used spatial mapping.
- Schema text no longer says scenes are 25 fps only.

Measured with the release build: a 120-second native 1080p stage of 18 layers built from the demo's art renders in 75-86 s at 156 MB engine peak. With 40 caption cues (58 layers) it takes 79 s and 189 MB, and at 60 fps 162 s and 170 MB. Receipts grow with movement (558 KB for that stage), so `SCENES.md` recommends `save_as` for long receipts.

Unit tests cover the limits at both rates, 64 and 65 layers, the work estimate and its rejection, run-length tile selections, record size, and trimmed graphics compositing exactly like whole canvases (blend modes, turns, crops, masks, transparent scenes, empty canvases). `tests/native_scenes.py` adds a 12-second scene of 24 layers (two minutes with `--long-form`, which the thorough run uses) with narration and 18 caption cues. It checks:
- every frame's count and the soundtrack exactly, and pixels against the oracle every second, on both sides of each caption boundary and at the last frame;
- the work estimate against an independent computation;
- that scratch holds only the output and PCM, with engine memory and time.
It also rejects 3,001 frames at 25 fps, 7,201 at 60 fps, 65 layers and an over-budget scene, and inspects a 64-layer, 3,000-frame scene. The captions fixture's layer-limit case now uses 64 cues. The evidence is stale until the next thorough run. No scoring changed.

## 5 October 2026: colour matching between cameras

Shots from two cameras rarely match, and grading applied only to scene layers, so an agent had no direct way to bring one camera's colour to another's. `color.match` samples frames of a reference and a target shot. It builds a per-channel 8-bit mapping, matching mean and spread or whole distributions, and writes it as a 256-entry `.cube`. It returns the `media.conform` recipe that bakes the table into a new target asset, with an identity normalization for encoded RGB assets.

The LUT fixture builds a target with per-channel gain, offset and gamma errors. It checks both methods:
- the table recomputed independently from the same sampled frames equals the written `.cube` byte for byte;
- the conformed asset equals that table applied to every target frame;
- with levels, the matched channel means land within one code of the reference's.

It also covers the rejections. The first version sampled mid-part times, and the last one fell past the final frame; default samples are now frame starts. No scoring changed.

## 5 October 2026: proposals piped by file

Proposals such as `audio.tighten` or `transcript.assemble` return `operations`, which an agent had to copy into `session.apply`. That spends output tokens on large batches and invites copying mistakes. Every command already accepted `save_as` in a workspace, but only document commands advertised it. Now the proposals and `job.wait` do too. A document reference's `select` can be a dotted path of fields and array indexes, such as `result.operations` inside a saved `job.wait` result. The MCP instructions tell agents to pass a saved proposal's `{"file", "select": "operations"}` to `session.apply`.

A documents unit test covers dotted and indexed paths and their errors. The agents fixture runs a workspace MCP session: a paper edit saved with `save_as` is applied by file reference, the saved session's clips equal the saved operations, and a `session.get` saved by file feeds `timeline.outline`.

To keep the workspace catalog within its 256 KiB budget, the shared `Time` description (repeated in 39 tools) and the `save_as` description were shortened. The full catalog is now 251 KB, and 258 KB with a workspace. No scoring changed.

## 5 October 2026: compact MCP catalog

The MCP catalog had grown to 83 tools and 255 KB of schemas, which a client loads into model context: about 60,000 tokens before any work. Most of that was repetition: every tool carries its own copy of shared definitions such as `Time`. `cutbolt mcp --tools core`, or `CUTBOLT_MCP_TOOLS=core`, now lists 31 everyday tools in full, plus `cutbolt_run`, which runs any other tool command by name with its usual arguments. That is 75 KB, 71% less, with every command still reachable. The full catalog remains the default.

A unit test checks that every core name is a real tool command and that the compact catalog stays under 96 KiB. The agents fixture covers:
- that the compact listing plus `cutbolt_run`'s commands equals the full catalog;
- that a routed call returns what the direct call does;
- that queued-only and unknown commands are refused;
- the environment-variable form and an invalid `--tools` value.

No scoring changed.

## 5 October 2026: static timeline check

Several kinds of mistake only showed up in a render, or not at all: a two-frame flash clip, unlinked sound placed a few frames off its picture, a missing file. The read-only `timeline.check` now reports them without rendering:
- errors: missing or changed media;
- warnings: black stretches, flash frames, unlinked picture and sound of one source out of sync (with which is late and by how much), and words cut by clip edges;
- notes: clips on disabled tracks, stretches with no audio, jump cuts within one source, and unused assets.

Findings come errors first, with a short text summary.

The transcripts fixture builds a project with one of each mistake and asserts the exact findings with their times, offsets and IDs. The clean paper edit reports only its deliberate jump cut. It also covers MCP and the argument rejection. No scoring changed.

## 5 October 2026: paper edits

Building a rough cut from transcripts meant converting word times to frame-aligned source ranges by hand for every selection. The read-only `transcript.assemble` takes word runs (transcript, first word, last word) and returns `clip.append` operations in order. Each covers its words plus optional padding, widened to whole frames so no word is clipped, with fresh clip IDs. The batch is checked to apply.

The transcripts fixture assembles three out-of-order runs with padding. The ranges are recomputed independently, and the render must equal the source frames and samples of those ranges in order. It also covers appending to an assembled project, which continues the IDs, and the rejections. A test variable that shadowed the fixture's source audio was renamed. No scoring changed.

## 5 October 2026: batch preparation

Bringing twenty camera files into a project took twenty `media.prepare` jobs, twenty waits and hand-built `media.add` operations. `media.prepare` now also takes `paths` and prepares them all in one job. Asset IDs come from the file names, made unique within the batch and against the project's existing assets. One file's failure is reported without stopping the others, and the result includes ready `media.add` operations.

The conform fixture batch-prepares four files: a duplicate name in a subfolder, an absolute path, and an unsupported audio-only file. Each converted output must be frame-identical to preparing that file alone. It also checks that the operations apply, that IDs avoid the project's assets on a second batch, and the argument rejections. No scoring changed.

## 5 October 2026: filler-word removal

Cutting the ums and uhs out of a talking-head edit took one hand-built ripple deletion per word. The read-only `transcript.fillers` now finds listed words (common English hesitations by default) wherever the timeline speaks them, using the shared transcript word projection. It proposes ripple deletions that merge consecutive fillers, keep optional padding out of the neighbouring words, and snap to the cut grid without entering them. It reuses the cut application from `audio.tighten`, now a shared helper, so each cut is checked on a working copy.

The transcripts fixture covers:
- a filler in the word-cut timeline, whose render must equal the original with exactly its frames deleted;
- a 29.97 fps sequential timeline, recomputed independently, with merged fillers, padding clamped at the neighbours, a custom word list and repeated split IDs;
- the rejections.

No scoring changed.

## 5 October 2026: scene stills

An agent designing a title, lower third or thumbnail could check its scene's structure with `scene.inspect`, but could not see it without compiling the whole movie. `scene.still` renders the one frame shown at a given time to a PNG at the scene's output size: RGB, or straight RGBA for a transparent scene. Over MCP it comes back as an inline image. It uses the same composition as `scene.render`. A thumbnail is a still of a one-frame scene.

The graphics fixture compares stills of the animated, masked, twice-enlarged scene with the independent expected frames at four times, one of them between frames. A transparent variant must equal its compiled movie's RGBA frames. The fixture also covers the MCP image, the range, overwrite and extension rejections, and leftover scratch files. No scoring changed.

## 5 October 2026: beats for cutting to music

An agent cutting a montage had no way to know where the beats of its music fell. The read-only `audio.beats` reports a music file's onsets, tempo and beat grid as exact file times, and optionally each beat's nearest frame. Onsets are sharp level rises on a 10 ms hop. The tempo comes from an amplitude-weighted onset-strength autocorrelation that tolerates periods between whole hops. The grid is aligned to the onsets and refined by least squares.

The first version read a 93 BPM pattern with accented downbeats and off-beat hi-hats at double tempo, and 128 BPM as 127.79. Weighting by amplitude, tolerant lags and the least-squares refinement fixed both. The audio-processing fixture checks against ground truth: synthetic drums at 120, 128 and 93 BPM. Every true beat must have a detected beat within one hop, with no extra beats, the tempo within 0.1 BPM, and exact frame snapping. It also covers a range, silence, validation and MCP. No scoring changed.

## 5 October 2026: whole-file speech recognition

Recognizing speech took one `transcript.transcribe` call per 120 s window, with `start` and `duration` worked out by hand. It also only read WAV files or 25 fps reference movies, so prepared 30 fps assets could not be transcribed. `media.transcribe`, queued with `job.start`, now extracts a file's audio losslessly and recognizes it in overlapping windows. It stitches the windows at word boundaries into documents that meet without overlapping, binds them to the source file, and saves them to one JSON file that outlines, captions, reviews and word cuts can use directly.

Verification is partial. Unit tests cover the window plan and the stitching, including a word straddling the overlap's middle. The transcripts fixture covers validation, the queued-only listing, and a recognition failure after extraction, which must leave no output. No speech runtime is installed on this machine, so the new runtime-fixture case is untested. That case transcribes each speech fixture whole and checks coverage, seams, binding, `transcript.inspect` and the reference word rate. No scoring changed.

## 5 October 2026: jump cuts from pauses in speech

Removing dead air from talking-head footage meant one ripple deletion per pause, each with hand-computed times and fresh IDs for every split clip and link. The read-only `audio.tighten` now proposes them all. It finds pauses in the voice track, or in a sequential timeline's whole program, with the same 10 ms detector as `audio.duck`. Each cut keeps some silence next to the speech, lies on a grid exact in frames and samples, and ripples every track so they stay in sync. Each cut is tried on a working copy first: refused cuts, for example inside a fade, are reported with the reason, so the returned batch always applies.

The transitions fixture adds a talking-head source with silences inside it. It recomputes the pauses, cuts and new IDs from the oracle's voice-only PCM. The tightened render must equal the original render with exactly the cut intervals deleted, on both a placed-track timeline (voice, linked picture and a music clip) and a sequential timeline. It also covers looser settings, a fade that refuses both cuts, and the rejections. No scoring changed.

## 5 October 2026: whole caption tracks as one overlay

Burned-in captions went through scenes, which hold at most ten seconds and 16 layers. A ten-minute video therefore needed about sixty caption scenes, each rendered, added and placed by hand. `captions.render`, queued with `job.start`, now renders a whole caption document as one transparent overlay asset for an `alpha_over` track. Internally it compiles consecutive transparent caption windows through the existing `captions.scene` path. Each window is as long as ten seconds and the 16-layer limit allow, in steps exact in samples and milliseconds. The windows are joined losslessly with exact lengths and checked before publication.

The captions fixture renders a 30-second, 22-cue track, a 3-second part and a 29.97 fps track. In the 30-second track, the first window shortens before its 16th cue, and one cue spans a window seam. Every frame is compared with the fixture's independent glyph rasterizer: each frame is empty or exactly the active cue's text. The audio must be silent, and the asset identity must be correct. Rejections leave no file behind. No scoring changed.

## 5 October 2026: captions drafted from transcripts

Captions for an edited timeline had to be written by hand, cue by cue, even when transcripts of every source existed. The read-only `captions.draft` takes the whole transcript words inside audible audio clips, at their timeline times, and groups them into cues. A new cue starts at a pause, after a sentence end, at a maximum duration, or when the line budget is full. Lines are balanced, and cue times are exact milliseconds, so the draft exports to SRT or WebVTT unchanged. Clip-edge words are left out and counted.

`export.review` and `captions.draft` now share one implementation of that word projection. Building this exposed an MCP trap: tool names map back to commands by turning underscores into dots, so a command name with an underscore cannot be called. A unit test now rejects such names.

The transcripts fixture recomputes every cue from the documented rules. It covers:
- a 29.97 fps sequential timeline with sentence ends, quotes, a pause, an overlong word and Greek text;
- tight and loose rule sets, a range, chosen tracks and a placed word-cut timeline;
- SRT export and re-import;
- the rejections.

No scoring changed.

## 5 October 2026: loudness normalization

Bringing a timeline to a delivery loudness took an agent several rounds of metering and editing gains by hand. The read-only `audio.normalize` now proposes `clip_audio` operations that scale every audio-track clip's level, and every gain-curve key, by one factor. The factor is the smallest of three: the one that reaches the target (default -14 LKFS), the one that keeps the sample peak under a ceiling (default -1 dBFS), and the one allowed by the clip gain range.

The first version predicted the outcome from one measurement, and the fixture caught the flaw. A mix that already clips reads 0 dBFS, so lowering it to a ceiling revealed a higher true peak. The engine now measures the mix at the proposed levels and refines the factor, up to three times, and reports the measured `result`.

`timeline.meters` now streams the rendered audio from disk, so its range limit rose from 600 s to 4 hours; reviews of delivered files use the same streaming meter.

The transitions fixture recomputes each proposal from meters of the oracle's independent mixes, following the same refinement. It covers:
- a quieter target;
- a peak-limited case on a clipping mix;
- a case limited by the clip gain range, with a disabled track;
- the rejections.

Each set of applied levels renders exactly, and `timeline.meters` on it equals the reported result. No scoring changed.

## 5 October 2026: reviewing a delivered cut

After an export, an agent could not easily check what it had made. The new `export.review` job reviews a rendered file into a new folder containing:
- a contact sheet and a small H.264 copy to watch;
- black runs;
- loudness over time, with silence and clipping;
- frame and sample counts against the project;
- the words the timeline should say, compared with the words heard in the file.

Heard words come from the local speech runtime, in overlapping windows, or from transcripts the caller supplies. The reply is a short text summary plus compact sections; `review.json` keeps the full detail.

Supporting changes:
- The loudness meters now stream, so reviews of long files do not hold the audio in memory.
- `job.wait` shows a finished review's sheet inline.
- A bug from the cut-review sheets is fixed: `preview.cuts`, `media.sheet` and `media.shots` left a hidden `.cutbolt-scene-*` folder, holding a second link to the sheet, beside each output. The scratch cleanup now removes it.

The transcripts fixture exports the word-cut timeline as H.264 and reviews it:
- expected words, cut words and differences are recomputed independently, including tolerance and muting;
- black runs match the timeline's video gaps exactly;
- integrated loudness is within 0.11 LU of FFmpeg's ebur128;
- peaks and silent and clipped runs are recomputed from the decoded PCM;
- the sheet is identical to `media.sheet`'s sheet of the same file;
- the same review runs as a queued job over MCP;
- rejections, including a recognition failure after decoding has started, leave no folder.

Speech recognition itself was not exercised, because no runtime is installed on this machine. No scoring changed.

## 5 October 2026: timeline outline

To read a cut, an agent had to scan the full project JSON or render previews. The new read-only `timeline.outline` returns a compact text reading instead. It lists, for each clip, its timeline and source spans, link, levels and transform. It also shows transitions and the stretches without picture or audio. Given transcripts of the sources, audio clips also show the words they contain, with words cut by a clip edge marked. This makes it cheap to check that a cut says what was intended. Over MCP the text is the tool's content.

The transcripts fixture recomputes every line independently from the project and the transcript documents:
- the word-cut timeline, with a transition, gain, a gain curve, a muted clip, picture-in-picture and a disabled, locked track;
- ranges and word limits;
- a 29.97 fps sequential timeline with adjacent transcripts and path matching;
- a child sequence;
- the rejections.

Tool listings now abbreviate the transcript document, as they do the project and scene schemas, which brings the catalog from about 262 KB back to 230 KB. `cutbolt_schema transcript` returns it in full. No scoring changed.

## 5 October 2026: gain automation and ducking on the timeline

Timeline audio clips had one constant gain plus linear fades, so lowering music under speech meant rendering a separate mix. Audio-track clips now take a `gain_curve` in the mix recipe's curve form, with up to 2,000 keys and any interpolation. Its key times are positions on the clip's source clock, which keeps it aligned through splits, trims and range renders.

The renderer computes each window's per-sample gains with the curve's own sampler and feeds them to the FFmpeg graph as a generated 16-bit stream. The existing single-rounding level expression reads that stream, so graph size does not grow with keys. The stream is written only when a render runs and removed afterwards.

The read-only `audio.duck` analyzes the voice track alone in 10 ms windows and proposes `clip_audio` operations. Each gives a music clip a curve that ramps down before speech, holds, and ramps back after.

The transitions fixture checks the curve and ducking against independent computations:
- A ramp-and-hold curve, its split, ranges starting inside it, and meters all match the independent PCM oracle exactly.
- Ducking speech regions and keys recomputed from the oracle's voice-only PCM equal the proposal, and the applied curves render exactly.

In a real-media run, ducking lowered the music bed by 12 dB during speech. Silence and clipping runs from `timeline.meters` now use reduced exact times like every other engine time. No scoring changed.

## 5 October 2026: one-step preparation of camera files

Getting a phone clip onto a timeline took three steps: inspect it, copy the proposed recipe, and queue a conversion. `media.prepare`, queued with `job.start`, now does this in one call. It returns a fitting ready file unchanged. Anything else it converts with the readiness recipe, at the target project's rate and size, or at the source's own rate. The readiness recipe now reads Matroska duration tags when streams carry no exact tick count.

In an end-to-end run, a 30 fps phone H.264 clip prepared for a 30 fps project kept all 240 frames, rendered at 30 fps and delivered H.264 at 30 fps. The conform fixture checks four things:
- A fitting ready file comes back unchanged.
- Prepared conversions at the source's own rate and at a project's rate are frame-identical to running their returned recipes through `media.conform`.
- An audio-only file and an HDR file are refused.
- The command is reachable only through jobs.

With this, the 30/60 fps path is complete: preparation, placed tracks, scenes and captions, and H.264 delivery all run at the native rate. Reading compressed video directly during render, without an FFV1 intermediate, remains future work. No scoring changed.

## 4 October 2026: scenes and captions at native frame rates

Scenes compiled to 25 fps only, so titles and caption windows could not be placed on the new 29.97 and 60 fps timelines. A scene now takes an optional `frame_rate`, one of the eight native rates, defaulting to 25. Durations, layer timing, strict holds, motion-blur sample times, soundtrack length, the encoder clock and output verification all use it. A scene may last up to ten seconds at its rate: 250 frames at 25 fps, 600 at 60. Caption windows sample cues on the base scene's clock.

The native timing fixture compiles a 29.97 fps scene and a transparent 60 fps scene whose strict one-frame holds alternate two images. Every frame matches exactly, both directly and when placed on a track timeline at the same rate, opaque or as an alpha overlay. The captions fixture samples a caption window on a 29.97 fps scene with the expected first and end frames.

Scene-layer tracking, stabilization and reframing still measure on their 25 fps clock. Existing 25 fps scenes serialize and render unchanged. No scoring changed.

## 4 October 2026: media conversion at the source's own frame rate

`media.conform` produced 25 fps assets only, so 30 and 60 fps footage lost or repeated frames even on a native-rate timeline. Recipes now take an optional `frame_rate`, one of the eight native rates, defaulting to 25 so existing recipes keep their fingerprints. `media.inspect` proposes the source's own rate when it is a timeline rate.

Frame selection now reads container timestamps at their own precision. Matroska rounds to whole milliseconds, so frame 1 of a 24 fps source is stored as 42 ms against an exact 41.67 ms. Strict "at or before" therefore repeated a frame and dropped the next. A frame within half a source tick after the output time now counts as at or before it. At 25 fps output times are whole milliseconds, so no 25 fps selection from a millisecond-based source changes.

The conform fixture now converts a 30000/1001 H.264 source, a 30 fps MOV and a 24 fps FFV1 source at their own rates. Every source frame is kept exactly once, and the output declares the native rate. The conform and remapping oracles apply the same selection rule, and all existing cases still match exactly. No scoring changed.

## 4 October 2026: placed tracks and H.264 at native frame rates

Placed tracks rendered at 25 fps only, and H.264 delivery required a 25 fps timeline. A 30 or 60 fps YouTube edit therefore had to be resampled, which made it judder. Track timelines, their previews, overlays, transitions and chunked renders now run on any of the eight native clocks. 25 fps renders keep their exact previous arguments.

H.264 delivery follows the timeline rate:
- a two-second GOP at the rounded rate;
- an MP4 timescale with a whole number of ticks per frame;
- the lowest H.264 level that carries the frame size and rate, 4.0 or 4.2, and 3.1 or 3.2 for baseline. Existing 25 fps outputs keep their level.

The native timing fixture now promotes 29.97 and 60 fps sequential timelines to tracks. Frames, samples and container timestamps match exactly. It also checks an upper track covering a gap, a native-rate dissolve against its integer equation, a preview frame inside it, and an H.264 export at the native rate.

Camera groups and proxies still require 25 fps and reject other rates explicitly. Scenes, captions and media conversion still produce 25 fps assets. The next steps are to let conversion keep a source's own rate and to render scenes at the timeline rate. No scoring changed.

## 4 October 2026: reviewing cuts, footage and loudness over time

An agent can only see stills and numbers, so it had no practical way to check an edit or log footage. This adds four read-only views:

- **`preview.cuts`** pages through every cut, 16 at a time, as a sheet of the frames on either side. The result says which clips and source times sit on each side, and which edit points are continuous splits rather than visible cuts.
- **`media.sheet`** shows frames from any decodable source file, before it is added to a project.
- **`media.shots`** finds shot boundaries with a frame-difference test that ignores motion and single-frame flashes. It can add a sheet with one frame per shot.
- **`timeline.meters` with `curve`** gives per-second short-term and momentary loudness, plus silent and clipped runs with exact sample times.

The new sheets return inline images over MCP. Frame previews also stop rejecting projects with more than 64 clips, because a preview reads only the frames it shows. This finishes the long-timeline change.

The tracks fixture checks each view:

- The cut table and paging on the 1,000-clip timeline, and pixel identity with the equivalent contact sheet.
- A continuous split.
- Source-sheet cells equal to the source frames.
- Three shots, with a flash ignored, in an FFmpeg test-pattern clip.
- Loudness curves within 0.11 dB of an independent K-weighted meter of the oracle PCM, with exact silence and clipping runs.

With 73 tools, the catalog budget unit test rises from 240 to 256 KiB. The new tools add about 11 KB, and the largest existing listings, `transcript.plan` and `captions.scene`, are the place to trim next. No scoring changed.

## 4 October 2026: long timelines render in exact chunks

One FFmpeg graph held at most 64 clips. A real-length edit with jump cuts and b-roll therefore failed to render at all, and track previews of any window failed once the whole arrangement passed 64 clips. Renders, range renders, previews, exports and meters now count only the clips a window reads. A window with more clips is planned as consecutive chunks of at most 64 clips. Each chunk renders through the ordinary path and they are joined by stream copy.

Chunk lengths are whole frames, samples and milliseconds, and the join is told each chunk's exact length. Without that, a container that rounds its duration up by a millisecond shifted every later frame. The joined output passes the same full decode verification as before.

The tracks fixture now renders a 1,000-clip timeline as 16 chunks, and it matches the independent oracle exactly. Before this change, the fixture asserted that this timeline was rejected. The native timing fixture renders 150-item sequential timelines with gaps at 30000/1001 and 24000/1001 as chunks. Frames, samples and every container timestamp match the exact clock, and the audio-only export matches too. No scoring changed.

## 4 October 2026: picture-in-picture overlay transforms

Overlay tracks could only cover the whole frame, so an inset camera or a corner b-roll had to be pre-rendered at its final size. Clips on `alpha_over` tracks now take an optional `transform`: a source `crop`, an integer `divisor` (1-8), an `opacity`, and a canvas `position` that may hang off the edge. The new `clip_transform` track edit sets it. The divisor shrinks each block to the floor of its mean, and opacity rounds to nearest. Shrinking is limited to opaque sources, because an exact alpha-weighted mean is not available in the pinned filters; this is rejected explicitly.

The steps map to FFmpeg's `crop`, `pixelize` average with neighbor decimation, `lutrgb` and `pad` filters. These were checked exact against independent integer arithmetic for divisors 2, 3, 4, 5 and 8 before use. The overlays fixture now builds a picture-in-picture timeline:

- a shrunk, cropped camera inset;
- a half-transparent inset hanging off the bottom-left edge;
- a cropped, faded and moved alpha title;
- a clip moved fully off the canvas.

Its independent numpy oracle matched all 150 rendered frames, three previews and a 20-frame range export exactly. The rejections are covered too. Existing projects serialize unchanged. No scoring changed.

## 4 October 2026: timeline loudness meters

Clip gain made levels editable, but an agent had no way to measure a timeline's loudness short of exporting it and probing the result. The read-only `timeline.meters` command (`cutbolt_timeline_meters`) renders a range's audio exactly as an audio-only export would. It reports sample peak, RMS and BS.1770 integrated loudness for the mix and for each enabled audio track played alone. A range is limited to 600 seconds.

The transitions fixture writes its oracle PCM to a WAV and meters it through an `audio.inspect` mix recipe. The meters of the leveled timeline, and of a sub-range, matched those values exactly. No scoring changed.

## 4 October 2026: run-length scene timing reports

Scene inspection and render receipts listed every layer's selected source frame and sampled parameters once per output frame. A ten-second title with several static layers cost tens of kilobytes of agent context to validate. Both arrays are now run-length encoded in frame order as `{count, value}` runs. The trial's four-second title card inspection dropped from 10,088 to 4,348 bytes, and a static layer is now one run however long the scene is. The scene fixtures expand the runs and keep their exact per-frame checks. No scoring changed.

## 4 October 2026: concurrent MCP tool calls and wait progress

The stdio server answered one request at a time, so a two-minute `job.wait` blocked every other tool call from the same agent. Tool calls now run on worker threads, up to eight at once, and responses are paired with requests by ID. Every accepted call is still answered when input ends. A `job.wait` sent with a progress token reports the job's phase once a second through `notifications/progress`.

A wire-level check in the agents fixture waits on a render job with a token while it sends a second call. The second call is answered first, and the progress values increase. No scoring changed.

## 4 October 2026: vertical text alignment and caption legibility

Text graphics always placed the first baseline at the box top plus the font size. Agents had to count wrapped lines themselves to center a title, and short caption cues sat at the top of their box instead of the bottom line. Text graphics and caption layouts now accept `valign`: `top` (the default, unchanged), `middle` or `bottom`. Each line takes a `line_height` slot. Bottom alignment leaves `line_height - size` below the last baseline for descenders. Overflow rejection also catches lines pushed above the box.

The captions trial also had no way to keep text readable over busy video. Text graphics and caption layouts now take an optional `background` (padded per-line boxes, drawn once where they overlap) and `outline` (the text's alpha dilated by a disc of radius 1-8). Both are composited under the unchanged fill.

The graphics fixture's independent oracle covers middle and bottom alignment, the new overflow rejection, and a decorated wrapped text with an empty line. That text matches with zero tolerance. All new fields are omitted at their defaults, so existing scenes keep their fingerprints. No scoring changed.

## 4 October 2026: clip gain and fades on audio tracks

Timeline audio tracks summed every clip at unity. In the trials, lowering a music bed or fading it out meant rendering a separate mix recipe and importing the result. Audio-track clips now carry optional `gain_milli`, `fade_in` and `fade_out`, set with the new `clip_audio` track edit or given at `place`. They share the mix recipe's linear semantics, with one rounding per sample before transitions and track summation. Fades are limited to 60 seconds and may not overlap, so the per-sample arithmetic stays exact.

Edits preserve the audible result:

- Fades follow edges through trims.
- Splits and interval edits keep only the fades at shared edges, and reject cuts inside a fade.
- Range previews and exports carry the original clip's envelope.
- A transition edge rejects a fade.
- Session diffs list a clip's levels.

The transitions fixture's independent PCM oracle now covers gain with fades next to a transition, a split that renders identically, ranges starting inside a fade, and the rejections. Defaults are omitted from snapshots, so existing projects and render graphs are unchanged. No scoring changed.

## 4 October 2026: media.inspect says what to do next

In the trials, `media.inspect` returned raw ffprobe output. Agents had to work out for themselves whether a file could be placed, compute its duration, and write a conform recipe. The result now also carries a `timeline` decision:

- A source that meets the renderer's profile is checked with the renderer's own packet-timed source inspection. The result reports the exact frame rate, frames and rational duration, plus an asset ready for `media.add`.
- Any other video gets its reasons and a whole-source `media.conform` recipe derived from its stream tags. For the trial's phone clip, that recipe passed `media.conform.inspect` unchanged.
- Stills, HDR sources and sources tagged with non-BT.709 color get a reason that points to the right command, and no recipe.

No scoring changed.

## 4 October 2026: declared project transfer

Every H.264 or PNG export and every scopes request had to repeat `input_transfer`, and agents in the trials guessed at it. A project can now declare the transfer of its encoded RGB values once, with the `project.transfer` operation. The declaration is a new optional `transfer` field, omitted when unset, so existing snapshots and fingerprints are unchanged.

`export.inspect`, `export.run` and `scopes.inspect` use the declaration when `input_transfer` is omitted. They reject a request value that contradicts it, and report the transfer they used. Session receipts show a declaration change. When neither is given, the error names the operation and explains the choice. Interchange export reports the declaration as a non-critical loss. The delivery and scopes fixtures cover the declared default and the contradiction. No scoring changed.

## 4 October 2026: listing workspace files

The MCP-only trials had no way to see which media existed, so every one of them fell back to a shell to list the folder. The new read-only `files.list` command (`cutbolt_files_list`) lists files and folders under `input_root`, which is the workspace in workspace mode. It returns sorted relative paths with sizes, can recurse and filter by extension, caps the entry count while reporting the total, and skips engine state. No scoring changed.

## 4 October 2026: transparent scenes for overlays

In the captions trial, burning captions into video meant rebuilding the clip as a scene of 250 PNG frames. That workaround hit the decoded-pixel limit at 640x360 and could not reach HD. A scene can now set `"transparent": true` and compile to a straight-alpha FFV1 `bgra` asset, which an `alpha_over` track composites over any video.

The compositor runs twice per frame: a color pass over black and a white matte pass, with the same weights. Straight color is derived from those two passes, and opaque or fully transparent pixels are exact.

An end-to-end check compiled the trial's title card as a transparent overlay through `job.start` and placed it over a clip. In the previewed frame, every pixel outside the title's text and logo area matched the underlying video exactly. Multiply and screen blending and 3D geometry are rejected for transparent output. Existing scenes serialize unchanged. No scoring changed.

## 4 October 2026: document files in a workspace

In the captions trial, an agent copied about 330 KB of scene and caption JSON from one call's result into the next call's arguments. In a workspace, any object argument can now be `{"file": "name.json", "select": "field"}`, read from a workspace file. `save_as` writes a call's whole result to a new `.json` file and returns a short summary. An inspected title scene went from a 10 KB response to 330 bytes and was compiled through `job.start` directly from its file. Existing files are never overwritten, documents are limited to 16 MiB, and a schema test keeps real request objects from looking like file references. No scoring changed.

## 4 October 2026: long-running commands as background jobs over MCP

In all three MCP-only trials the agents had to fall back to CLI-only commands. The blockers were H.264 delivery, media conversion and scene compilation.

`job.start` now queues `export.run`, `media.conform`, `scene.render`, `audio.render`, `audio.repair.render`, `hdr.conform`, `image.sequence.compile`, `proxy.generate`, `preview.range`, `cache.run` and `transcript.transcribe` in the existing background queue. That includes request-ID replay, output reservation and status. Arguments are prepared and validated at submission like a direct call. `job.wait` waits up to two minutes for a result instead of polling.

A queued job can be cancelled; a running one finishes, and an interrupted run is reported, not retried. The server instructions now describe the cut, graphics and caption workflows.

Two smaller fixes:
- Sequential-clip alignment errors name the clip and field.
- A command that can't be queued is refused before its arguments are checked.

A workspace end-to-end run exported H.264 from a saved-project reference and conformed a phone clip from a path-only identity, both over MCP. The catalog has 68 tools, about 230 KB. No baseline, criteria, evidence, weights, exclusions or denominators changed.
## 4 October 2026: serialized shipping

Concurrent sessions kept racing at the push: each gate passed, but another session's push landed during it, so the push was rejected. `tools/ship.py` now takes a lock in the shared state. Inside the lock it fetches, rebases onto the target branch, builds, gates and pushes, so every push is gated on exactly the commit it publishes. A waiting ship waits for at most one gate. Background verification starts after the lock is released.

## 4 October 2026: background runs give way to newer commits

Successive ships had left several background runs verifying overlapping fixtures for successive commits. Each background run now registers its commit and fixtures in the shared state. Before starting a fixture, it skips any fixture that a newer background run of a descendant commit will also verify; only the newest result matters. Skipped fixtures are listed as superseded and are neither passes nor failures.

## 4 October 2026: one retry for the real-time capture outside thorough runs

Background runs share the machine, and the recording fixture's real-time capture is correctly rejected when a packet is delayed by load: a 90 ms gap in one background run. Outside `--thorough`, that one fixture may now retry once, and its record notes the earlier failure. The thorough run still captures once, in its quiet phase.

## 4 October 2026: gate deadline and background yielding

Shipping while background verification ran showed that duration predictions fail under contention: one gate took six minutes.

- The gate's budget is now a hard deadline (85 s of fixtures by default). Fixtures still running at the deadline are stopped with their process trees and deferred; fixtures not yet started are deferred too.
- Timing history is kept per kind of run, so gate predictions come from earlier gates.
- A running gate leaves a marker in the shared state, and background verification starts no new fixtures while a live gate exists.
- Background runs default to four lanes and the gate to eight. Process creation saturates near eight concurrent fixtures on this machine, so more lanes only slow every fixture.

On a heavily loaded machine (two background runs) a broad gate finished in 125 s, reused 25 unchanged passes, and deferred the rest.

## 4 October 2026: background verification fixes

The first background runs exposed two harness problems, both now fixed:

- **Console windows.** The background verifier was started without a console, so every console program it launched opened its own window. Three fixtures were ended by console-close events. It now runs with one hidden console shared by all its children, in its own process group.
- **Missing build artifacts.** Fixtures that compile helpers against the library (`transcription`) or build examples (`recording`) expected `target/debug` in the worktree. Background runs now build into a shared, incremental directory in the common git directory, and `tests/engine.py` exposes it as `BUILD`, following `CARGO_TARGET_DIR`.

## 4 October 2026: agent usability fixes from MCP-only trials

Three fresh agents worked through an edit, a title card and a music-plus-captions job using only the MCP view. All three finished only by falling back to CLI-only commands, and 60 to 90% of their response bytes went to schema and capability lookups. This batch addresses most of what they hit:

- **Discovery:**
  - Large schemas come back as an outline, with `select` for one variant or definition; `scene.render`'s lookup drops from 72 KB to under 1 KB.
  - `capabilities` is a 3 KB summary with sections, down from 26 KB.
  - Workspace listings no longer claim that paths must be absolute.
  - The uninformative per-tool output schema is gone, saving about 18 KB of the catalog.
- **Inputs:**
  - Exact time literals: `2.5`, `"5/2"` or `3`.
  - File identities given as a path alone are hashed on request, and `media.inspect` returns identities.
  - `session.create` accepts dimensions directly.
- **Outputs:**
  - Previews return an inline downscaled image, and contact sheets are available over MCP.
  - Receipts summarize long ripples: one trim in a 500-clip timeline drops from 165 KB to 3.9 KB.
  - `scene.render` assets carry their identity.
- **Errors:**
  - Type errors name their field.
  - Missing references list the available IDs, and range errors state exact times.
  - Content mismatches give the file, its expected and actual values, and a remedy.

The catalog has 66 tools, 225 KB, inside its 240 KiB budget. No baseline, criteria, evidence, weights, exclusions or denominators changed. The recorded fingerprints are stale until the next thorough run.

## 4 October 2026: one packet listing per source inspection

Phase 1a of the [development-loop plan](DEVELOPMENT_LOOP.md). Reference-source inspection used three `ffprobe` runs per source: stream metadata, every decoded video frame, and every decoded audio frame. It now uses one packet listing, which decodes nothing. That listing returns metadata identical to a plain probe, FFV1 video packet timing, and exact PCM16 sample counts from packet sizes. Only strict inspection adds a second run, which decodes the video frames; validation still happens before any decoding, so unsupported media is rejected as before. Alpha overlays reuse the same metadata instead of probing again.

On the `tracks` fixture, engine tool launches fell from 454 to 345 (-24%), with no behaviour change. The impact map selected 59 of 61 fixtures for this core change, so the gate ran what fit in its budget and deferred the rest to background verification.

## 4 October 2026: two-minute push gate and background verification

A change can now be shipped in about two minutes while the full set of affected fixtures is still verified.

- **Impact map.** `tools/impact.py build` runs every fixture against a coverage-instrumented engine (in its own `target/coverage` directory) and records which Rust functions each fixture executes. Instrumented builds use `--cfg cutbolt_coverage`, which makes the engine flush coverage before `process::exit`; on Windows that call otherwise skips the profiler's exit hook, so most invocations recorded nothing. Normal builds are unchanged.
- **Gate.** `tools/verify.py --gate` maps the diff since the map's commit to fixtures:
  - changed lines inside recorded functions select the fixtures that executed them;
  - other changes in a Rust file select every fixture that executed code in that file;
  - test helpers select the fixtures that import them, and build files select everything.

  Passes with unchanged inputs are reused. The rest run shortest first within a 100-second budget, beside formatting, lint and Rust tests, and whatever does not fit is deferred.
- **Ship.** `tools/ship.py` gates the committed HEAD and pushes it, `--to main` included. It then verifies the deferred fixtures in a detached background worktree of the pushed commit, with its own engine copy at below-normal priority, so it never locks a checkout or blocks another session.
- **Shared state.** Run records, timing history and the map are kept in the repository's common git directory and shared by all worktrees. `tools/verify.py --status` lists recent runs, and the next gate warns about any fixture that failed and has not passed since.

No acceptance criteria or evidence changed; the recorded source fingerprints stay stale until the next thorough run.

## 4 October 2026: quick verification by default

An audit of the edit/test/verify loop is recorded in [DEVELOPMENT_LOOP.md](DEVELOPMENT_LOOP.md), with measurements and a phased plan. Its first phase is implemented here.

- **Quick by default.** `tools/verify.py` now runs Rust lint/tests and every correctness fixture concurrently, in each long-form fixture's short mode, with a 30-second capture. Wall-clock budgets are recorded rather than enforced, and nothing is written to the evidence. A complete quick run took 16.4 minutes for all 61 fixtures with every external runtime configured.
- **`--thorough` is the evidence run,** unchanged from the previous verifier: every budget enforced, long-form cases complete, budget-gated fixtures on a quiet machine, progress regenerated.
- **Targeted runs:**
  - `--only` runs named fixtures without the Rust prelude;
  - `--last-failed` reruns failures;
  - `--fail-fast` stops starting fixtures after the first failure;
  - `--strict` turns skipped external runtimes into errors.
- **Preflight.** External runtimes and tools (`ffmpeg`, `ffprobe`, `cargo`, `pwsh`) are checked before any work; the thorough run requires all of them.
- **Copied engine.** Fixtures run a private copy of the engine through `CUTBOLT_EXE` (`tests/engine.py`), so `target/debug` can be rebuilt while verification runs. Per-machine stage times and failures go to the ignored `verification/last-run.json`.
- **Budget helper.** All 19 wall-clock budgets in 15 fixtures go through `tests/budgets.py`. It enforces them by default, and only the quick check records them instead. Memory and correctness assertions are unchanged.

No acceptance criteria, budgets, evidence, weights, exclusions or denominators changed. The core and combined trackers remain 100/100 and 108/108. The recorded source fingerprints stay stale until the next thorough run.

## 4 October 2026: workspaces and saved project references

Every command that reads a project now also accepts `{"project_id", "revision"}`. That covers 27 fields, including the project nested in render.start and in cache tasks. The engine loads the saved revision itself, so agents no longer copy whole snapshots back into requests. An optional workspace, set with `--workspace DIR` or `CUTBOLT_WORKSPACE`:
- supplies omitted roots, with engine state under `.cutbolt/`;
- resolves relative paths;
- confines explicit roots;
- reports engine-produced paths relative to itself, without the Windows `\\?\` prefix.

Without a workspace, existing requests and results are unchanged. Request fingerprints hash the loaded snapshot, so stored receipts keep matching. New unit tests cover:
- defaults, containment and `..` rejection;
- reference loading and its errors;
- relative reporting;
- an MCP server answering without any roots.

A schema test fails if a root or project input appears where these passes do not look. The 65-tool catalog is 236,434 bytes, or 239,292 with a workspace. A full reference render ran end to end with relative paths and no roots. No baseline, criteria, evidence, weights, exclusions or denominators changed.

## 4 October 2026: native-resolution scenes, overlay tracks, large imports and agent ergonomics

These changes fix findings from two external capability demonstrations, a pixel-art reel and a narrated 1080p video. No acceptance criteria, weights, exclusions or denominators changed, and no points were awarded. The core and combined trackers remain 100/100 and 108/108; the new checks are additional regression evidence.

- **Scenes** render natively up to 4096 pixels per axis (eight million output pixels). Output scale 1 skips scaling, text reaches 512 px, and `tilemap` layers assemble canvases from up to 256 independently timed tiles in 4,096 cells. Frames are composited in parallel batches limited to each layer's footprint and streamed to the encoder, and `capabilities.scenes.limits` reports the bounds. Glyph rows were drawn one pixel low when rasterizer bounds fell just below a whole pixel; placement now follows the rasterizer's own offset.
- **Tracks:** `"composite": "alpha_over"` video tracks composite straight-alpha FFV1 sources over the opaque base with exact integer rounding. Previews, scopes, range exports and queued renders include them.
- **Media conversion** accepts 16 GiB, 108,000-frame, one-hour sources and 45,000-frame (30-minute) outputs. CPU forward and freeze mappings stream only the needed frames. Reverse, non-monotonic and hardware-decoded mappings use a bounded 4 GiB window, and audio decodes only its mapped window.
- **Audio-only exports** no longer decode, composite or encode pictures; source video timing comes from FFV1 packets.
- **Agent interface:**
  - `session.receipt` returns a committed request's stored receipt; MCP now has 64 tools, and the stale count of 61 in the docs is corrected.
  - Unaligned-time and scene errors name the field path and value, and SRT rejections name the cue.
  - Scopes on native tracks were already supported; the documentation now says so.
- **Verification** now schedules fixtures in a memory-aware parallel pool beside the real-time recording capture. Budget-gated fixtures run one at a time, with the long-form 4K render in its own lane, and every failure is reported. The 4K check hashes its 45,000 frames in eight parallel segments, with each frame's absolute index still asserted. Stage times are recorded under `verification_timing`.

New fixtures: `native_scenes` (12 checks, including exact native 1080p spatial rotation and tilemaps), `overlays` (7), `large_imports` (8) and `agent_ergonomics` (6). A full parallel verification of the combined tree, including the separately recorded schema change, took 45.5 minutes against 6,164 s for the previous serial run. 60 of 61 stages passed. The 15-minute sustained capture was rejected once for a 10 ms capture discontinuity and passed in an earlier run of the same capture code; it is a timing-sensitive real-time fixture, not a regression. Evidence and progress trackers were therefore not regenerated, and the recorded source fingerprints stay stale until the next thorough run. The speech fixtures needed PowerShell 7.6.5, which had to be reinstalled. Documentation records PixelForge 0.7.0 as a later observation and the Qwen CustomVoice checkpoint, now fetched and verified by `tools/download_qwen.py --repository`.

## 4 October 2026: right-sized agent schemas

Each MCP tool's `inputSchema` used to embed all 214 request type definitions. As a result, `tools/list` returned 5,417,140 bytes of compact JSON (roughly 1.4 million tokens) for 64 tools. Listings now carry only the definitions each tool uses, and five large shared types become short stubs: project, operation, scene, template and audio routing. A stub names the new read-only `schema` command, which returns the complete schema of any command or type, including CLI-only commands. The `operation` stub keeps every valid `op` tag.

Every request field, shared type and tagged variant now has a description taken from its Rust doc comment. Previously only 6 of 524 type properties had one. The 65-tool `tools/list` result is now 222,225 bytes, and the largest listing is 11,800 bytes. Unit tests fail when:
- a schema node lacks a description;
- a listing has an unresolved or deferred reference;
- a listing exceeds 16 KiB;
- the catalog exceeds 240 KiB.

The MCP fixtures' tool-count assertions move from 64 to 65. No baseline, criteria, evidence, weights, exclusions or denominators changed. The recorded source fingerprints are stale until the next full verification.

## 4 October 2026: verified 108/108 implementation completion

The complete verifier passes **466 unique checks**, reaching **100/100 core, 8/8 supplemental and 108/108 total (100.0%)**. Both original checkpoints now pass for X01 3D planes/cameras/lighting, X02 exact shutter sampling, X03 typed property expressions/links and X04 optional local annotated segmentation. The supplemental fixtures add 28 acceptance checks and one exact-arithmetic Rust test to the 437-check core milestone. No criterion, weight, exclusion, performance gate or denominator was relaxed. Agent foundations remain separate at 10/10.

The full run took **6,164.391 seconds** with unchanged source and capability fingerprints. The 30-minute 4K fixture matched all 45,000 decoded frames and 86,400,000 stereo sample frames, with exact timestamps and preserved sources. Native rendering took 1,190.813 seconds with a sampled process-tree peak of 2,347,204,608 bytes, inside the 2,700-second/4-GiB gates. Sustained native recording captured all 43,200,000 stereo sample frames over 900 seconds, with zero reported QPC/sample-clock deviation. All earlier media, timeline, audio, color, graphics, export, persistence, interchange, hardware, failure and recovery groups passed too.

The final source, executable and generated reports are preserved in an external immutable checkpoint. Documentation now marks the supplemental profiles verified while retaining their bounds: 3D planes and declared encoded-color lights; bounded encoded-RGB shutter sampling; pure typed position/opacity expressions; and annotated binary masks with an optional external CPU worker. Natural-footage matting, automatic segmentation tracking, arbitrary meshes/shadows and general scripting are not implied by this score. Original code and public interfaces underpin these additions; raw research, third-party implementations, models and media remain outside the publishable tree. Repository material and source-freshness checks pass. The requested 100/100-core-then-108/108 delivery sequence is complete.

## 4 October 2026: prepared local foreground segmentation

The original optional annotation workflow uses the public GrabCut API with an external pinned CPU runtime. It preserves original PNGs, immutable mask/annotation revisions, actual fitted-model arrays/digests and producer package/runtime identities. Native scene frames accept binary mattes before effects/transforms and retain exact source holds. Focused acceptance matches all authored moving outlines exactly (IoU and boundary F1 1.0, motion-compensated disagreement zero), and separate fresh workers reproduce masks/models without cache downloads. Correction history, 64 complete rendered frames, 122,880 stereo sample frames, four previews, saved edits/retry/undo and 26 rejections pass. The actual 64-frame/4,194,304-pixel workload takes 2.125 seconds against the fixed 120-second gate. These synthetic binary-mask results do not claim natural-footage matting or automatic tracking; unsupported automation rejects. Package versions/licenses remain external and recorded. X04 is prepared for full verification only; official coverage stays **100/100 core / 100/108 total (92.6%)**.

## 4 October 2026: verified core completion

Full verification passes **437 unique checks**, reaching **100/100 core and 100/108 total (92.6%)**. The remaining five core points now have integrated evidence: broader lossless/color/alpha export, native containers/rates/image sequences, bounded exact-build native-project transfer and long-form 4K performance/stress acceptance. The run took 6,035.4 seconds with unchanged source and capability fingerprints, including complete long-form pixel/audio comparisons and sustained 900-second recording. No acceptance gate, criterion, weight, exclusion or denominator was relaxed. The 100-point source, executable and generated reports are preserved externally. Supplemental expression, temporal and geometry candidates have focused evidence only; local foreground segmentation remains to implement. The requested endpoint remains **108/108**, so the active work continues.

## 4 October 2026: prepared 3D planes and lighting

Original bounded camera-ray rendering now supports textured planes, per-pixel alpha/depth composition, camera and node hierarchy, clipping, unlit/Lambert materials and animated ambient/directional/point lighting. Unsupported meshes, shadows and conflicting 2D placement reject. An independent 60-digit forward-projection/clipped-polygon reference passes 332 frames, 637,440 stereo sample frames, four previews and 34 rejection cases, with zero observed pixel error. The maximum 16,777,216-visit fixture takes 15.851 seconds against its fixed 180-second gate, and all 32 nodes, 16 hierarchy levels, eight lights and 32,768 state records are exercised. Typed inspection, saved retries/undo and source protection pass. The expression and temporal fixtures also pass against the same engine. No dependency, acceptance wording or denominator changed. X01 is prepared but unscored until integrated verification; see [geometry](GEOMETRY.md).

## 4 October 2026: prepared temporal sampling

The external supplemental candidate now samples exact rational shutter midpoints, source holds, animation, effects and expression links before averaging complete encoded-RGB frames. Source/audio clocks and ordinary scene behavior retain their existing contracts. Focused acceptance passes 284 complete frames, 545,280 stereo sample frames, four previews and 17 rejection checks. The independent continuous-exposure fixture is exact at eight or more samples, with lower-sample approximation errors reported explicitly. All 67,108,864 raster sample visits render and decode in 10.142 seconds, within the fixed 90-second gate; the separate maximum 32,768-record fixture passes. The prior failed fixture used MCP for a CLI-only preview command; its call was corrected and the fixture rerun. X03's complete focused fixture also passes after the sampling refactor. Integrated verification remains required; no points, criteria, weights or denominators change.

## 4 October 2026: prepared typed property expressions

An external development copy adds an original exact rational property graph, scene position/opacity bindings and read-only typed inspection. Its focused acceptance passes 503 complete frames, 965,760 stereo sample frames, four previews and 83 rejection checks against authored closed-form motion/fades and independent full-frame geometry. Seeded/order checks and maximum graph work pass; sources and prior outputs remain preserved. Maximum inspection takes 0.874 seconds and maximum rendering/decoding 0.702 seconds, within the fixed 15/60-second gates. The implementation awaits integrated verification and adds no points. X03 retains both original criteria; X01, X02 and X04 remain required. No baseline, denominator or criterion changes.

Unscored native-project continuation, 4 October 2026: implemented original `native.import`, validating a source application's public-interface transfer against three distinct content-bound inputs and an explicit exact-build/adapter matrix. Actual original native fixtures were authored, saved and reopened in the installed application's background test session. The external adapter preserves flat editorial timing and reports appearance/audio/metadata omissions; nesting and other unsupported structures block import. No proprietary implementation was inspected or ported. Application-specific files, captures and original private adapter remain outside the repository.

Focused acceptance compares **300 rendered frames and 576,000 stereo sample frames**, with **21 rejected requests**, typed MCP proposals, saved edits/retry/undo, disabled/muted content, multiple-sequence diagnostics and blocked nesting. The matrix contains one exact observed build; alternate labels/hashes reject rather than implying compatibility. The first capture exposed asynchronous media readiness and correctly blocked an offline clip; a bounded public-state poll fixed capture timing. Full verification replays hash-bound captures and rechecks installed application/adapter identities; it does not claim a fresh application launch. The tested external launch requires exclusive ownership and an audio-device dialog dismissal, so no unattended-launch guarantee is made. I03 basic/extended are prepared, not awarded. Official progress remains **95/100 core / 95/108 total (88.0%)**, with all 100 core and eight supplemental criteria preserved.

Unscored long-form continuation, 4 October 2026: the complete 30-minute moving 4K fixture passes all **45,000 RGB frames and 86,400,000 stereo sample frames**, with exact timestamps and source preservation. The native pipeline takes **1,311.844 seconds** and **2,347,646,976 bytes sampled process-tree peak**, within the unchanged 2,700-second/4-GiB gates. The independent RGB comparison takes a further 1,227.188 seconds. This focused long run predates the subsequent cleanup-only fix; full acceptance will repeat it against the integrated final source.

Repeated 12-second native edits take 10.453/10.843/11.391 seconds, with two injected real-encoder write failures, running cancellation in 0.141 seconds, four observed render-tool processes exiting and the following queued edit matching every pixel/sample. A genuine Windows sharing lock exposed a leftover partial; bounded exact-path cleanup now passes both the stress fixture and the forced-lock unit case. A strengthened process-exit test initially included the persistent queue console host; observed process identities established the distinction, and the corrected test checks all descendants of the active tool scope. No latency or memory gate was relaxed. Renderer/MCP regressions, 73 Rust tests (four process helpers) and repository checks pass. P03 extended is prepared for full verification only; official coverage remains **95/100 core / 95/108 total (88.0%)**. The original 100/108 denominators and remaining native-project and supplemental criteria are unchanged.

Full verification completed 4 October 2026: **420 unique checks pass**, reaching **95/100 core and 95/108 total (88.0%)**. E02 extended and T05 extended now have integrated delivery-profile and exact native-rate evidence, including complete long-form synchronization and sustained 900-second recording. The run took 3,186.4 seconds with unchanged source and capability fingerprints. Five core and eight supplemental checkpoints remain. Lossless output formats and transparent numbered footage have passed focused acceptance and are integrated for the next full verification; they earn no points yet. The original criteria and denominators remain unchanged.

Unscored lossless-alpha continuation, 4 October 2026: original complete numbered-footage validation and explicit source-window repetition compile straight/premultiplied PNG sources into transparent FFV1 or PNG movies, or explicitly flattened native editing assets. Color interpretation, alpha conversion and silent 48 kHz PCM are declared; every source identity, output pixel/sample, timestamp and movie track duration is checked. Four focused groups pass with 54,405 decoded frames, 87,141,996 stereo sample frames, 20 rejection/preservation cases and 12 preserved originals, including every frame/sample of a 30-minute transparent movie. Saved edits/replay/undo, 19 existing renderer checks, 24 MCP/job checks and 12 repository checks pass. The additional read-only inspection tool brings the catalog to 61; it earns no agent-only editing point. E01 extended is a candidate pending stable full verification. Transparent output remains separate from native opaque timeline compositing; source images retain explicit 512-pixel/64-MiB bounds and staging is capped at 2 GiB. This does not award long-form 4K or native application compatibility. An initial tool-name mapping error was corrected without relaxing gates. No dependency, criterion, denominator or exclusion changed.

Unscored lossless-format continuation, 4 October 2026: reference sequential exports now accept eight native clocks, and PNG MOV/numbered-directory profiles preserve exact RGB with optional PCM soundtracks. Original focused acceptance verifies 700 decoded frames and 1,078,696 stereo sample frames across 42 exports plus a concurrent publication case, with 13 explicit rejections and 45 source files preserved. Portrait, odd/single-pixel, UHD/DCI-4K sizes, complete ordered file identities, nonzero/last numbering, exact range endpoints, Unicode/percent paths and source/publication failures pass. Initial development exposed the selected encoder's color-option alias, Windows writable-handle flush requirement and verbatim-path separator behavior; all were corrected without relaxing pixel/timing gates. The old regression case rejecting all 24 fps reference output is now covered positively by the new matrix; rejection coverage retains unsupported 23 fps native output and 24 fps H.264 delivery. E05 extended is assigned candidate evidence pending stable full verification. This does not award E01 alpha/long-render or P03 long-form 4K coverage; no criterion, denominator or dependency changed.

Full verification completed 4 October 2026: **409 unique checks pass**, reaching **93/100 core and 93/108 total (86.1%)**. E04 extended, I02 basic/extended, I01 extended and E06 extended now have integrated evidence for bounded render recovery, public editorial interchange, portable complete-history migration/recovery and encoding/publication failure handling. The stable 900-second recording case captures 43,200,000 stereo sample frames with 12,836,864-byte peak working set and zero reported QPC/sample-clock deviation. The run took 3,054.5 seconds; source and capability fingerprints stayed unchanged. Failed earlier evidence remains retained. Seven core and eight supplemental checkpoints remain; delivery controls and native timing are the next tested candidates. No criterion, denominator, dependency license or accepted exclusion changed.

Unscored native-timing continuation, 4 October 2026: sequential renders, previews, caches, contact sheets and scopes now use eight declared exact frame clocks. Fractional cuts require whole stereo sample frames; incompatible sources, VFR direct input and unsupported rates reject. Focused acceptance compares 54,745 decoded frames and 87,597,460 stereo sample frames, including every frame/sample of a 30-minute fractional render, explicit VFR conversion, saved editing/replay/undo and queued output. An actual publication crash verifies fractional queue recovery. Existing 25 fps rendering and all 24 MCP/job checks pass. T05 extended is a candidate pending stable full verification. This does not award long-form 4K/performance coverage or expand placed-track, proxy, scene or delivery clocks. Failed development evidence is retained; original checkpoint wording, thresholds, denominators and dependencies are unchanged.

Unscored delivery-control continuation, 4 October 2026: explicit constrained/main/high compatibility settings, bounded quality and two-pass bitrate control, and selectable AAC bitrates preserve the default export preset. Six original native groups pass, including a 12-case quality/rate/profile matrix, two concurrent repeatability cases, native size boundaries, audio-only choices, actual selected CUDA/software decode equality, both encoding-phase failures, typed inspection and source preservation. Two-pass target error is below 0.2%; matrix RGB PSNR is at least 27.11 dB and audio SNR at least 33.24 dB. The initial CRF-28 comparison missed its 27 dB gate; CRF 26 was selected, with the gate and failed evidence retained. The original 26-export fixture still passes its unchanged stricter chart thresholds. E02 extended is a candidate pending stable full verification; no point, new dependency or changed acceptance scope is claimed. Broader frame rates, lossless formats, image sequences, long-form 4K and native application compatibility remain required separately.

Unscored render-output failure acceptance, 4 October 2026: three original native groups inject Windows disk-full errors through a bounded writer receiving real encoded bytes, terminate an actual encoding worker/tree, and change synthetic source content after a complete encode. Normal failed attempts leave no published/partial output, bounded retries retain failure history, changed sources block publication without retry, and crash partials remain intact during fresh encoding. Independent comparison covers 128 frames and 245,760 stereo sample frames; existing outputs and source identities are preserved. This is explicit write-fault injection, not a physical full-volume test. E06 extended is assigned this evidence plus publication-crash checks pending the full verifier; no point or scope change is claimed.

Full-verification attempt, 4 October 2026: the integrated recovery/interchange run stopped after 2,840.3 seconds during sustained recording. The selected synthetic audio source process exited with a Windows audio-device error, and capture rejected with `CAPTURE_PROCESS_EXITED`. Source and capability fingerprints stayed unchanged; the diagnostic record is retained. The verified baseline remains 88/100 core and 88/108 total. No recording gate was relaxed or result substituted. The next stable run includes portable history and explicit render failure-injection acceptance.

Unscored portable-history continuation, 4 October 2026: implemented root-bound relative source/proxy resolution, content-checked `project.portable` proposals and saved `media.paths` operations. Explicit editing-store version-1 to version-2 migration preserves original snapshots, receipts, requests and history while adding revision-metadata checksums. Consistent checked backups and recovery preserve complete history without overwriting stores. An actual retained older engine supplies migration fixtures; decoded moved media, proxy previews, live-writer backup/recovery and corrupt/occupied-root cases pass focused acceptance. Six substantive Rust checks exercise child-process exits and genuine SQLite disk-full migration rollback. I01 extended remains a candidate until the complete verifier passes. The five tools bring the catalog to 60; no agent-only point, new dependency, changed checkpoint or denominator is introduced.

Interchange portability correction before scoring, 4 October 2026: a follow-up check found that relative explicit media bindings were returned unresolved even though the native renderer requires resolved paths. The first integrated verification was stopped with source/capability hashes unchanged, and its diagnostic record is retained. Import now validates and resolves used source/proxy paths under the supplied media root. A newly relocated original fixture verifies relative bindings through full-quality rendering and proxy preview. This correction does not award points; full verification is rerun after it.

Full verification completed 4 October 2026 (run dated 3 October): **379 unique checks pass**, reaching **88/100 core and 88/108 total (81.5%)**. P01 basic/extended adds identity-checked cache reuse, invalidation, bounded eviction and measured cold/warm latency; E03 extended adds contact sheets; P02 basic/extended adds actual optional device decoding, explicit fallback, numerical agreement and measured throughput. The stable 900-second recording fixture captures 43,200,000 sample frames with 12,808,192-byte peak working set, 900.5-second wall time and zero reported QPC/sample-clock deviation. Source and capability fingerprints stayed unchanged throughout the full run. Earlier failed recording evidence is retained; no continuity threshold was relaxed. All 50 groups and original checkpoint wording/weights remain unchanged. Twelve core checkpoints and all eight supplemental checkpoints remain required; the target remains 100/100 core, then 108/108 total. Render recovery and editorial interchange are integrated next as unscored candidates pending their own stable full run.

Unscored public editorial interchange candidate, 4 October 2026: added an original bounded OTIO JSON mapping with explicit local identity-bound media, exact rational source/track clocks, transparent gaps, enabled tracks, generic dissolves and per-path loss acknowledgements. Unknown structures block conversion; imports propose snapshots and exports preserve existing files. The pinned external OpenTimelineIO 0.18.1 library authors/reads/normalizes original fixtures; independently calculated decoded RGB/PCM, saved edits/undo and loss/failure cases pass focused checks. The three typed tools bring the MCP catalog to 55. I02 assignments are candidates for full verification, not earned points. I03 native application projects remains unchanged and unimplemented. No proprietary implementation, external library source, generated media or new Rust dependency is added.

Unscored core continuation, queue recovery, 4 October 2026: a separate original-code checkout adds explicit 1–3 total attempts, source/tool pinning, attempt history, transactional queue migration and durable publication receipts. Six substantive Rust checks and four real-media groups pass, including actual process termination before/after publication, exact output after retry, all 32 queue slots and cancellation. The independent media oracle compares 32 frames and 61,440 stereo sample frames, with four rejected submissions. All 64 library tests pass (three crash helpers are excluded from scored evidence), plus the existing 19 renderer and 24 MCP/job regressions. E04 extended assignments await stable full verification; E06 and I01 migration criteria remain separate and unawarded.

The first native recovery fixture found that a background worker inherited a caller pipe, delaying CLI EOF until the job exited. The corrected hidden Windows launcher inherits no caller handles; the retained rerun passes with spaces and Unicode in external paths. No publication or numerical gate was relaxed. The main cache/hardware verifier runs separately against frozen sources; this recovery candidate is not merged into that run.

Full-verification attempt, 3 October 2026: the cache/contact-sheet candidate passed its earlier suites but stopped during sustained recording with `CAPTURE_DISCONTINUITY`. Source and capability fingerprints remained unchanged. The device packet timestamp exceeded the existing one-millisecond continuity gate; no recording, cache or preview gate was relaxed, and the baseline remains **83/100 core and 83/108 total**. A more detailed diagnostic now reports the observed/expected packet interval and sample position without changing the acceptance limit. The next stable run combines the cache and optional hardware candidates; focused results do not award points.

Unscored core continuation, optional hardware decoding, 3 October 2026: implemented explicit local CUDA source-video decoding for the documented progressive H.264 subset. Existing software defaults, source clocks, color/audio handling, identities and no-overwrite publication remain in force. The expanded original fixture passes six groups with 1,170 decoded frames, 2,246,400 stereo sample frames and eleven rejected cases. RGB differences are zero within the declared one-level gate; PCM agrees exactly. Actual unavailable-device fallback, strict failure, later decode failure, truncated output, source change, timeout, typed inspection and saved assets pass. Three complete 100-frame full-HD repetitions per backend measure about 3.33 CPU versus 3.35 CUDA output fps, with no useful overall speedup established. P02 basic/extended assignments await full verification; no points are awarded by this focused run. The full verifier now requires an explicit hardware device and never substitutes software evidence. No acceptance wording or scope changed.

Unscored core continuation, caching and contact sheets, 3 October 2026: implemented explicit content-checked probe/proxy/frame/interval/sheet reuse, transactional LRU budgets, checked payload/metadata recovery and contact-sheet layout. A separate original-code development copy preserved the prior verifier's frozen sources until its 83-point milestone completed. The focused fixture passes nine check groups, comparing **338 images/video frames and 576,000 stereo sample frames**, with 25 rejected cases. Actual 64-cell and eight-million-pixel sheets, nested transitions, proxy/final agreement, moved/changed media, external tool identity, corruption, saved history, concurrent callers and publication collisions pass. Six substantive storage tests pass, including hot-journal recovery, SQLite-full rollback and physical page reclamation; the child-process test helper is excluded from scored evidence.

Three cold/warm repetitions per task produce median times in seconds of **probe 0.364/0.191, frame 0.942/0.516, proxy 1.604/0.520, interval 1.312/0.526 and sheet 1.351/0.518** in the focused fixture. These include source/tool hashes and output publication, and do not imply cold operating-system caches. A selected dependency's development optimization reduced mandatory hash overhead without removing integrity checks or changing algorithms/versions. Storage budgets count payloads, with index/journal/temporary overhead reported separately. Broader limits and known interruption behavior are in [CACHE_PREVIEWS.md](CACHE_PREVIEWS.md).

P01 basic/extended and E03 extended evidence assignments are prepared for full verification. **No cache or contact-sheet points have been awarded yet**; the last stable baseline remains **83/100 core and 83/108 total (76.9%)**. Full verification against unchanged merged source is required before promotion. Acceptance wording, weights and denominators remain unchanged; the goal remains 100/100 core, then 108/108 total.

Verified core continuation, local speech and transcript editing, 3 October 2026: the stable full verifier passed **358 unique checks** with unchanged source fingerprints, including both G04 checkpoints. Core coverage rises **81 → 83/100** and total implementation rises **81 → 83/108 (76.9%)**. All acceptance wording, weights, exclusions and denominators remain unchanged. The requested endpoint remains **100/100 core, then 108/108 total**.

Actual recognition and corrected/estimated cuts compare **663 frames, 1,272,960 stereo sample frames and six previews**, with 30 rejected editing cases. The authored editing regression separately verifies 666 frames, 1,278,720 stereo sample frames, eight previews and 37 rejects. Six short English/Greek cases pass the unchanged contextual timing gates. Short Greek recognition still includes explicit substitutions and corrections; no perfect-accuracy claim is made. Two-minute English/Greek inputs retain all 240/234 words with zero word-edit distance; contextual onset maximum/95th-percentile errors are 150/80 ms and 135/125 ms. Full-run times are 48.6/69.5 seconds, CPU peaks 2.41/4.17 GB and allocated GPU peaks 1.47/1.87 GB. Twenty-eight native rejection cases, library cancellation, caller termination and eleven supervisor cases pass; the documented bridge-termination scratch limitation remains. The full run also repeats sustained 900-second recording and all earlier editing evidence.

The history below retains earlier partial results and failed candidates. Current bounded behavior and limits are in [TRANSCRIPTS.md](TRANSCRIPTS.md). Models, generated speech and raw investigation remain external. No files were staged or committed.

Unscored core continuation, native local speech, 3 October 2026: integrated the original source-bound blocking transcription command, optional external English/Greek runtime, exact 48 kHz to 16 kHz conversion, explicit channel selection and partial-sample reporting, acoustic evidence and reviewed word cuts. Six fresh short speech fixtures pass independent contextual onset gates; the short isolated fixtures also pass end-time gates. Both estimated and explicitly corrected word selections produce independent exact rendered results through native and saved editing. The authored editing regression still passes 666 frames, 1,278,720 stereo sample frames and eight previews, with 37 explicit rejects.

The maximum two-minute fixture initially exposed repeated-phrase omissions. Those failures remain in external evidence. The revised recognition windows use real quiet gaps and keep acoustic alignment inside the window that produced each word. The passing focused maximum run retains all 240 English and 234 Greek fixture words with zero word-edit distance; onset maxima are 150 ms and 135 ms, and 95th percentiles are 80 ms and 125 ms. Runtime is approximately 46.5/69.3 seconds, CPU peaks 2.41/4.13 GB and allocated GPU peaks 1.47/1.87 GB. This is fixture evidence, not a guarantee for arbitrary speech. The same original onset limits remain 250 ms maximum and 150 ms at the 95th percentile. Raw phonetic times are reported separately from the explicit contextual editing intervals.

Production native rejection/cancellation checks pass 28 rejected cases plus library cancellation and caller termination, preserving sources and removing owned temporary files with no worker survivors. Eleven supervisor fixtures verify bounded output, changed worker, malformed results, deadlines and detached descendants. Forcefully killing the WSL bridge can interrupt scratch cleanup; the fixture records that limitation separately while confirming no descendant survives. Models, runtime packages and generated fixtures remain external; the dependency ledger records exact selected versions and licenses.

G04 evidence assignments are now prepared for the full verifier. No new score has been awarded: generated evidence still reflects the last verified **81/100 core / 81/108 total (75.0%)** and is stale against this source. Full verification must pass unchanged sources before promoting the two G04 checkpoints. The objective remains **100/100 core, then 108/108 total**; no criteria or denominators changed.

Unscored core continuation, transcript editing, 3 October 2026: implemented content-bound word documents with exact source-sample clocks, atomic caller corrections and native linked word-range cuts. Three local CLI/library/MCP inspection tools return immutable records and plans; `transcript.cut` reuses native timeline validation and saved transactions. Focused evidence compares **666 decoded frames, 1,278,720 stereo sample frames and eight previews**, with **37 rejected cases**; all decoded differences are zero. Eight focused Rust tests pass; the preceding complete library run passed all 49 tests before the additional transition-policy check. The original synthetic editing fixture uses authored anchors and makes no recognition-accuracy claim. See [transcript limits and remaining acceptance](TRANSCRIPTS.md).

This earns **zero new points**. Optional recognition, accuracy/language acceptance, bounded work and worker lifetime integration remain unfinished. The manifest and denominators are unchanged; the last full verified baseline remains **81/100 core / 81/108 total (75.0%)**. Current source changes require another stable full verification before any promotion. The objective remains **100/100 core, then 108/108 total**.

Core continuation, subject reframing, 3 October 2026: added `reframe.inspect` with authored or optionally tracked focus boxes, explicit crop/aspect windows, subject changes, cut resets, bounded pan motion and smoothing. The returned editable scene preserves original image identities, audio and exact source clocks. Crop decisions expose feasibility, movement and confidence. The fitting-window extension retains legacy defaults and states source-tap/transparent-edge semantics. See [subject reframing](REFRAMING.md).

The retained `C:\DEV\CutboltData\reframing-20261003-03` fixture and full verifier compare **513 decoded frames, 984,960 stereo sample frames and four previews**, with **70 rejection/failure cases**. Every observed RGB/PCM difference is zero; the declared RGB tolerance remains one unit. Independent enumerated crop graphs, authored subject trajectories, exact source clocks and high-precision forward geometry/alpha filtering verify complete output. Subject cuts and continuous changes, changing boxes, all easing modes, source offsets/holds, alpha, masks/effects, templates, saved retries/undo/restoration, typed MCP and preview ranges pass. Jitter-following motion falls from **186 to two pixels of total pan** while retaining the full padded subject. Known local tracks are exact, with minimum correlation **1.0** and uniqueness margin **0.769574**; unreliable observations reject explicitly.

The maximum fixture actually renders **128 frames and 16 subject segments** from a **512 × 512** source canvas. Full-run inspection uses **13,488,128 bytes peak working set** and **0.078 seconds**, within the 128 MiB/30-second bounds. The aggregate tracking-work gate, invalid requests, changed sources, output collisions and publication cleanup pass. The fitting-window regression separately compares 650 frames and 1,248,000 stereo sample frames across all three fitting modes, with explicit pixel aspect and edge sampling; it adds no extra spatial checkpoint.

This earns **G05 basic/extended**, raising core coverage **79/100 to 81/100** and total implementation to **81/108 (75.0%)**. Full verification passes **340 unique checks**, formatting, lint and repository boundaries. Three new Rust checks include 10,000 enumerated candidate crop graphs and fitting-window/pixel-aspect/default validation. No dependency, acceptance wording, weights, exclusions or denominators changed. Agent foundations remain 10/10 separately and supplemental coverage 0/8. **19 core checkpoints**, then all eight supplemental checkpoints, remain required: **100/100 core, then 108/108 total**. Local transcription and text edits are next; preparation adds no points and the goal remains active.

Core continuation, Unicode text layout, 3 October 2026: added optional `unicode_v1` text layout with external static TrueType fonts, contextual shaping, mixed writing directions, ligatures/kerning, combining placement, canonical decomposition, language-specific forms, grapheme fallback and word wrapping. Original text, font identities and shaping clusters remain inspectable. Existing scalar recipes retain their layout; transparency, templates, caption scenes and ordinary compiled-asset workflows share the same checked renderer. See [Unicode text](UNICODE_TEXT.md).

The retained `C:\DEV\CutboltData\unicode-20261003-05` fixture and final verifier compare **144 decoded frames, 276,480 stereo sample frames and four previews**, with **35 rejection/failure cases**. Every observed RGB difference is at most one level, inside declared one-level coverage/two-level animated-compositing tolerances; silent PCM is exact. All 1,024 characters in the maximum-text fixture are actually rendered. Full-run inspection uses **14,581,760 bytes peak working set**, **0.672 seconds** and **387,043 shaped input scalars**, within the unchanged 256 MiB, 30-second and 1,048,576-work bounds. Manual glyph plans, authored original font tables and Fraction rectangle-area coverage form an independent pixel reference without invoking the shaper. Templates, styled captions, saved retries/undo/restoration, typed MCP, frame/range previews, malformed fonts, work limits, changed-font rejection and output/source preservation pass.

This earns **G01 extended**, raising core coverage **78/100 to 79/100** and total implementation to **79/108 (73.1%)**. Full verification passes **330 unique checks**, formatting, lint and repository boundaries. External HarfRust **0.13.3**, unicode-bidi **0.3.18**, unicode-segmentation **1.13.3**, unicode-script **0.5.8** and unicode-linebreak **0.1.5** are recorded with exact licenses and the resolved dependency graph. Original fixture fonts stay outside the repository. No acceptance wording, weights, exclusions or denominators changed. Agent foundations remain 10/10 separately and supplemental coverage 0/8. **21 core checkpoints**, then all eight supplemental checkpoints, remain required: **100/100 core, then 108/108 total**. Explicit subject reframing is next; preparation adds no points and the goal remains active.

Core continuation, local input recording, 3 October 2026: added explicit Windows capture input discovery, read-only settings inspection, streamed 48 kHz stereo PCM16 recording and sample-exact native audio-track placement. A held endpoint or process identity prevents silent reselection; packet counts, QPC timestamps, device positions and discontinuities are retained or rejected under the declared timing contract. Streaming WAV validation checks the same held file before and after PCM inspection; publication preserves existing outputs and source identity. Placement returns ordinary transactional operations with explicit advance/delay correction, existing lock/collision checks and nested-sequence support. The MCP catalog now has 46 tools; capture remains blocking CLI/library work. See [recording](RECORDING.md).

The retained `C:\DEV\CutboltData\recording-20261003-05` development fixture and final full verifier compare **475 rendered frames and 912,000 stereo timeline sample frames**, with **26 rejection cases**. Original 44.1/48 kHz playback tests verify the selected process's two channel frequencies and exclude a concurrent process's distinct signal. Independently authored impulses prove sample-level placement and both signs of correction. Saved retries/undo/restoration, native WAV ranges and reduced previews, nested sequences, typed schemas, identities, unsupported formats and source/output preservation pass. Actual playback-process loss fails without final publication; a newly selected process and an exact one-sample capture succeed. Development runs deliberately omit the long-capture evidence ID.

The final full verifier separately completes a **900-second native capture** of its own muted helper: **43,200,000 sample frames**, **12,673,024 bytes peak working set**, **913.344 seconds** capture plus verification, and **0 maximum 100-ns units** of accumulated timestamp/sample-clock deviation. All PCM values obey the declared two-unit silence/dither bound. The gates remain 128 MiB, duration plus 30 seconds and 20 ms accumulated deviation. Separate Rust tests cover actual bounded two-hour writer processing with an independently computed digest, invalid WAV boundaries, disk-write failure, cancellation, packet gaps/timestamp faults, device changes and concurrent output publication. Hardware microphones were inspected without starting capture; automatic physical latency calibration and crash recovery of unfinished takes are not claimed.

This earns **A06 basic and extended**, raising core coverage **76/100 to 78/100**, total implementation **78/108 (72.2%)**. Full verification passes **322 unique checks**, formatting, lint and repository boundaries; all 12 audio points now have verified evidence. Windows **0.62.2** and windows-core **0.62.2** (MIT OR Apache-2.0) supply external typed platform bindings; original code handles capture accounting, WAV streaming and placement. Dependencies and exact resolved versions are recorded without vendoring. Acceptance wording, weights, exclusions and denominators are unchanged. Agent foundations remain 10/10 separately and supplemental coverage 0/8. **22 remaining core checkpoints**, then the eight supplemental checkpoints, remain required: **100/100 core, then 108/108 total**. Unicode shaping, bidirectional layout and cluster-safe fallback are next; preparation earns no points and the goal remains active.

Core continuation, local dialogue repair, 3 October 2026: added `audio.repair.inspect` and `audio.repair.render` with original fixed-profile spectral suppression, independent mono/stereo noise estimates, a bounded overlap accumulator, optional reconstructed-mean removal and the existing ordered EQ/compression. Exact rational source cuts and noise-profile regions preserve the selected sample count. Inspection exposes every learned noise spectrum and output PCM identity; rendering verifies a new extensible WAV and source identity before no-overwrite publication. Repaired media feeds routed scene soundtracks, templates, saved edits, retries/undo and previews. The MCP catalog now contains 43 tools. See [dialogue repair](DIALOGUE_REPAIR.md).

The retained `C:\DEV\CutboltData\audio-repair-20261003-06` fixture and full verifier each compare **11,700,392 sample frames / 15,283,840 individual samples**, **nine scene/session frames**, **two previews** and **27 rejection cases**. Independent NumPy transforms and whole-buffer reconstruction, original analytic signals, external EQ and closed-form compressor references compare every PCM value; actual differences are zero, within declared one/two-unit processing tolerances. Tests cover original-source noise clocks outside trimmed output, unordered/multiple regions, exact first/last samples, channel independence, explicit/zero-profile bypass, DC removal, noncommuting effect order, final clipping and malformed/unsupported input. A Rust test changes the source after preparation and verifies failed publication, scratch cleanup and preservation of existing output.

Twelve held-out original spoken/noise cases cover white and colored noise at two levels, 50/100 Hz hum and mixed 60 Hz hum/noise. Speech-interval signal/error improves **4.0048–15.3284 dB**; noise-only reduction is **8.3810–17.9468 dB**. Overall aligned voice gain stays **0.8844–0.9945** and 3–10 kHz voice projection **0.6651–1.0**, inside predeclared quality gates. The white-noise tonal-concentration increase stays below 2.697 dB against a 6 dB limit. Clean inputs with zero noise profiles retain every sample. A sharp onset has **zero sample shift**, **0.87852 peak gain** and **1.56% relative pre-echo**, within its 2% limit. Known 20 Hz rumble falls **27.9645 dB**, with a **-0.000399 dB** change to the 1 kHz foreground. These controlled metrics expose attenuation/artifact tradeoffs; they do not claim a listening panel or universal speech intelligibility.

The 60-second stereo boundary fixture executes spectral processing with a ten-second profile and checks all **2,880,000 output sample frames**. Retained inspection takes **14.984 seconds** and **143,036,416 bytes peak working set**, within its 40-second/256-MiB gates. Maximum region count/settings, exact clocks and preserved originals/previous outputs also pass. The documented workflow creates a fresh **563,256-sample mono WAV** whose PCM identity matches inspection. These supplementary workflow/boundary measurements add no separate points.

This earns **A05 basic and extended**, raising core coverage **74/100 to 76/100**, and total implementation to **76/108 (70.4%)**. Full verification passes **311 unique checks**, formatting, lint and repository boundaries. Suppression remains an explicitly authored fixed-profile operation; changing backgrounds, overlapping speakers, clicks, clipped-speech restoration and reverberation remain outside this profile. The independent tests select external NumPy **1.26.4 (BSD three-clause)** and installed local system speech synthesis with recorded identities; no Rust/runtime dependency, proprietary implementation or generated media was added. Acceptance wording, weights, exclusions and denominators are unchanged. Agent foundations remain 10/10 separately; supplemental coverage remains 0/8. The **24 remaining core checkpoints** precede the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Local input recording is next; read-only endpoint preparation earns no implementation points and the goal remains active.

Core continuation, audio buses and surround routing, 3 October 2026: added optional `mix.routing` with explicit track/bus/output layouts, signed channel matrices, parallel routes, linear/equal-power mono panning, stereo balance, and exact-clock pan/bus-gain automation. A deterministic acyclic graph preserves wide summation headroom and applies final-only PCM16 saturation. Buses support gain, mute and the existing ordered EQ/compressor chain, with independent channel filter state and linked compression. Named mono, stereo, quad, two 5.1 layouts and 7.1 export through an original bounded WAVEFORMATEXTENSIBLE reader/writer. Existing stereo recipes retain their sample behavior. Stereo routed soundtracks compile into scenes and existing saved-session edits/previews. See [audio routing](AUDIO_ROUTING.md).

The retained `C:\DEV\CutboltData\audio-routing-20261003-03` fixture and full verifier compare **2,945,109 sample frames / 23,349,915 individual channel samples**, plus **nine scene/session frames, two previews and 47 rejection cases**. Independent Fraction graph algebra, 60-digit Decimal panning, externally decoded channel layouts/EQ, and closed-form compressor steps cover six layouts, three rates, mixed tracks, matrix rounding, fan-out, bus cancellation/mute/clipping, automation, processing order, templates, saved retries/undo and preview agreement. All observed PCM differences are zero; declared one/two-unit effect-chain tolerances remain explicit. Graph cycles/disconnections, hidden invalid parameters, ambiguous/malformed WAV data, identity conflicts, source/output boundaries and excessive work/headroom reject. A Rust publication test changes a source after preparation, verifies no output or owned scratch remains, restores the fixture and checks existing-output preservation.

The maximum-duration fixture verifies every sample in a **60-second 7.1 output**. Retained inspection uses **101,314,560 bytes peak working set and 7.234 seconds**, inside its 256-MiB/45-second gates; graph state holds 16 live values in that case. Maximum 16-bus depth and 64 routes also pass. The documented inspect/render workflow produces a fresh 1,920-sample stereo WAV with matching PCM identity. These boundary/workflow checks add no separate points.

This earns **A03 extended only**, raising core coverage **73/100 to 74/100**, and total implementation to **74/108 (68.5%)**. Full verification passes **303 unique checks**, formatting, lint and repository boundaries. Stereo scene output remains explicit; non-stereo integrated loudness is reported as unmeasured, and general surround delivery is not implied. No dependency version, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental coverage remains 0/8. The **26 remaining core checkpoints** precede the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Local dialogue cleanup is next. External noise-reduction design experiments earn no implementation points; the goal remains active.

Core continuation, exact variable-speed remapping, 3 October 2026: added optional `remap` to the existing conversion recipe. Up to 64 authored speed segments integrate linear endpoint rates exactly, with continuous source position, declared reversals, zero-speed holds and sample-aligned boundaries that may fall between video frames. Previous/nearest/linear temporal sampling uses decoded timestamps; interpolation follows each source frame's normalization/LUT and precedes spatial resize. Inspections expose every source clock, contributor and blend weight. Explicit audio pitch policy either follows instantaneous speed through exact PCM resampling or mutes the whole conversion. Reverse/frozen segments require mute. Existing constant-rate conversion, proxy and synchronization recipes retain their behavior. See [remapping](REMAPPING.md).

The retained `C:\DEV\CutboltData\remapping-20261003-03` fixture and full verifier each compare **735 decoded frames, 1,411,200 stereo sample frames and four previews**, with **30 rejection/failure checks**. Independent signed Fraction polynomial integration, full RGB/PCM references, known encoded-intensity behavior and analytic tone phase verify acceleration/deceleration, equivalent split ramps, starts/stops, very slow/high rates, all 64 segments, sample/between-frame boundaries, reverse/freeze continuity, end holds, fractional/VFR/reordered sources, mono/stereo/missing audio, normalization/LUT order, resize, MCP schemas, saved retries/undo and range previews. Source/LUT mutation after encoding, invalid maps/policies/identities and existing outputs fail without publication or changed originals.

The linear sampler's mean intensity error is **0.2567 RGB units**, versus 0.5333 for nearest and 0.9875 for previous-frame sampling on the authored chart. Six pitch measurements agree with the independently predicted changing frequency within 0.000012 Hz (declared limit 0.15 Hz). Every non-LUT RGB and all PCM values match exactly; the LUT case retains its pre-existing one-unit rounding tolerance and independently distinguishes the processing order. A separate maximum-duration ramp compares **1,500 frames and 2,880,000 stereo sample frames** exactly over 60 seconds. A deliberately excessive derived audio precision fails after successful frame inspection with no output or scratch files. The documented workflow also produces a fresh 50-frame/96,000-sample asset. These supplemental checks add no points.

This earns **V07 extended only**, raising core coverage **72/100 to 73/100**, and total implementation to **73/108 (67.6%)**. Full verification passes **295 unique checks**, formatting, lint and repository boundaries. Interpolation blends encoded working values; optical flow, pitch-preserving stretch and reverse audio remain open. Output remains the declared 25 fps reference profile, so T05's fractional-output/long-form timing gate is unchanged. No dependency, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental coverage remains 0/8. The **27 remaining core checkpoints** precede the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Audio buses, channel routing, panning and surround output are next; the goal remains active.

Core continuation, measured camera stabilization, 3 October 2026: added read-only `stabilization.inspect` over content-bound scene image layers. Distributed background patches measure translation or rigid planar camera motion. A symmetric binomial measurement filter and quarter-pixel refinement of distinct correlation peaks reduce sampling-phase artifacts while keeping original render pixels intact. The rigid fit checks every patch residual, measured roll, confidence, step and acceleration. Declared cut segments start fresh templates and separate smoothing windows. The public integer mask-tracking profile retains its previous behavior and passing evidence.

Lock and triangular-window smoothing produce exact-time editable correction curves with an explicit strength. Correction and constant zoom compose before the authored spatial mapping, preserving nonuniform scale, mirroring, aspect fitting, changing anchors, masks and animation. A compensated source viewport clips independently. Crop policy either preserves 1× scale with disclosed uncovered edges or selects the smallest bounded constant zoom that fills the selected image/crop footprints. The report retains measurement settings, confidence, residuals, target/correction poses, identities, zoom and retained-area tradeoffs. Compilation remains explicit; no source or session is changed by inspection. The native MCP catalog now contains 42 tools. See [stabilization](STABILIZATION.md).

The retained `C:\DEV\CutboltData\stabilization-20261003-06` fixture and full verifier each compare **248 decoded frames, 476,160 stereo sample frames and five previews**, with **31 rejection/failure cases**. Independent known camera poses, source-plane landmarks, rational smoothing and ideal image sampling check actual stabilization; a separate high-precision geometry/Fraction oracle checks every rendered pixel. Cases cover translation lock, camera roll up to ±4.8 degrees, intentional pan, partial/zero strength, rational source holds, declared cuts, crop/zoom limits, trimmed images, animated/nonuniform/mirrored authored transforms, masks, straight/premultiplied equivalence, templates, saved retries/undo and frame/range previews. Moving foreground patches, repeated/flat evidence, undeclared cuts, excessive roll/residuals, invalid settings/work budgets, changed media and existing outputs reject.

Translation lock has zero residual motion. The ordinary roll fixture improves from **1.9075 to 0.0772 source pixel RMS**, and the stronger roll case from **2.1916 to 0.0658**. Pan, fractional-hold and cut cases also pass the independent **0.25-pixel RMS, greater-than-fourfold reduction and eight-unit mean ideal-image error** limits. Maximum observed ideal-image mean error is 5.967 RGB units; complete renderer-reference differences are zero (one-unit arbitrary-geometry tolerance). PCM is exact, and equivalent alpha encodings match exactly. A separate 128-key loop renders 132 frames and 253,440 exact stereo sample frames, with all 128 active frames identical after known-motion cancellation. An original stereo soundtrack retains its sample-1920 start, all 30,720 output sample frames, source hash and unchanged stabilized video. The documented command also compiles a fresh 16-frame output. These supplemental checks add no points.

This earns **V06 basic and extended**, raising core coverage **70/100 to 72/100**, and total implementation to **72/108 (66.7%)**. Full verification passes **288 unique checks**, formatting, lint and repository boundaries. The declared model covers planar translation and camera roll; row-dependent rolling-shutter repair, projective/3D parallax correction, scale estimation, automatic cut recovery and subject-aware reframing are not implied. No dependency, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental coverage remains 0/8. All **28 remaining core checkpoints** remain required before the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Variable-speed remapping and its interpolation/audio pitch policy are next; the goal remains active.

Core continuation, original motion tracking and mask feathering, 3 October 2026: added read-only `tracking.inspect` over content-bound scene image layers. An original fixed-template normalized correlation search returns exact source-frame selections, layer/global clocks, displacement, confidence and ambiguity margins. Explicit search, motion, acceleration, texture and whole-canvas change limits reject unreliable trajectories without partial output. Tracking measures the restored source canvas before masks/effects/transforms; trim offsets, changing anchors, rational frame holds and alpha interpretation remain explicit. Returned hold-keyframe mask curves and replacement scene are editable values that require an explicit render to apply. The native MCP catalog now has 41 tools.

Layer rectangles now accept inner, centered and outer feather ramps with a bounded source-pixel radius, declared square outside corners and exact inversion/empty behavior. Coverage uses a 1/65,536 grid and scales associated color and alpha without intermediate RGBA quantization. The integer and spatial paths preserve legacy hard-mask output; bilinear sampling filters already-masked taps. Generated graphics, animated rectangles, all existing blends and both alpha encodings retain their documented order. No dependency, model download, proprietary material or runtime service was introduced. See [motion tracking and mask edges](TRACKING.md).

The retained `C:\DEV\CutboltData\tracking-20261003-02` fixture and full verifier each compare **486 decoded frames, 933,120 stereo sample frames and five previews**, with **38 rejection/failure cases**. Authored coordinates and independent Fraction/geometric references cover known motion, brightness changes, partial occlusion, boundaries, rational holds, transforms/effects, premultiplied inputs, feather profiles, inversion, zero/one alpha, all blends, quarter/arbitrary rotations, animated empty masks, extreme coordinates/radii and graphics. Ambiguous/repeated patches, low texture, transparent evidence, severe occlusion, jumps, acceleration violations, cuts and unsupported rotation reject. Typed MCP schemas, templates, saved retries/undo, frame/range previews, identities, no-overwrite and changed-source publication failures pass. Actual pixel error is zero throughout; the general geometric/color tolerance remains one RGB unit, and equivalent alpha encodings plus silent PCM must match exactly.

A separate retained boundary check renders **132 additional frames and 253,440 stereo sample frames** with 128 mask keys spanning repeated source loops; every pixel/sample and source hash matches the independently verified cycle. A separate mean-centered-vector calculation verifies correlation and uniqueness margins for 80 observations to below 2.1e-15. The documented command sequence also compiles a fresh 20-frame output successfully. These supplemental checks add no acceptance points.

This earns **V05 extended only**, raising core coverage **69/100 to 70/100**, and total implementation to **70/108 (64.8%)**. Full verification passes **281 unique checks**, formatting, lint and repository boundaries. The tracker supports integer translation, with explicit unsupported scale/rotation, subpixel motion and automatic reacquisition; confidence is similarity evidence rather than a probability of object identity. Stabilization, reframing and segmentation retain their own gates. No acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental coverage remains 0/8. The **30 remaining core checkpoints** precede the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Stabilization is next; the goal remains active.

Core continuation, interpolated and animated spatial transforms, 3 October 2026: added optional `transform.spatial` to original scene recipes. It supports subpixel translation, independent axis scale, clockwise rotation, mirroring, declared rational pixel aspect and contain/cover/stretch fitting, with an explicit destination viewport. Geometry preserves selected full-canvas anchors and trimmed-image offsets; inspection reports sampled values and forward/inverse matrices. The existing integer compositor remains unchanged when the option is absent. All four tested quadrant/scale mappings also matched the new identity path exactly.

The new sampler maps destination pixel centers onto a 1/65,536-source-pixel grid. Nearest or bilinear taps use transparent or clamped crop edges, apply source masks and ordered effects, then accumulate premultiplied color/alpha without intermediate RGBA rounding. Existing normal/multiply/screen blends round at the final opaque backdrop. Five new transform curves reuse exact rational layer clocks, easing, independent retiming and endpoint rules. No additional MCP tool, dependency or runtime service is introduced. Spatial metadata survives typed template scene payloads; compiled assets use existing saved edits, retries, undo and full/range/proxy previews.

The retained `C:\DEV\CutboltData\spatial-20261003-03` fixture and full verifier each compared **602 decoded frames, 1,155,840 stereo sample frames and seven still previews**, with **31 rejection/failure cases**. An independent 60-digit Decimal reference transforms basis points, derives the inverse analytically and performs rational premultiplied interpolation/compositing. Cases cover four exact legacy quadrants, nine aspect fits, both mirror axes, composed arbitrary rotations, changing trim anchors, both edge policies, straight/premultiplied equivalence, all blends, animated transforms/opacity/masks, graphics/effects, layered output enlargement, template/MCP schemas, saved history, large-coordinate bounds and source changes during encoding. Every observed pixel difference was zero; the declared arbitrary-angle/graded-color tolerance is one RGB unit, while exact cases and all PCM require zero. The documented spatial declaration also passed inspection and rendering.

This earns **V01 extended**, raising core coverage **68/100 to 69/100** and total implementation to **69/108 (63.9%)**. Full verification passed **275 unique checks**, formatting, lint, repository-boundary tests and material checks; source fingerprints remain current. Existing scene limits remain explicit, including 25 fps, bounded logical canvases, encoded-sRGB filtering, opaque output and bilinear-only interpolation without area minification or temporal integration; see [spatial transforms](SPATIAL.md). No proprietary material, dependency, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental work 0/8. All **31 remaining core checkpoints** remain required before the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Motion tracking and robust mask feathering are next; the goal remains active.

Core continuation, camera groups and synchronization, 3 October 2026: added `multicam.create` and `multicam.edit` to retain camera alternatives, source coverage, sample-aligned audio offsets and recoverable selection cuts in reusable sequences. Fixed-master, follow-video and muted audio project deterministically into native tracks. Every alternative participates in cycle, deletion and transitive lock checks, including unused cameras. Saved diffs, retries, atomic rollback, undo and restoration retain camera metadata; manual projection changes and ordinary child-track edits reject for managed groups. Previews, proxies, ranges and queued original-quality output use the existing nested renderer.

The new read-only `sync.inspect` CLI/library/MCP command measures bounded audio windows or maps declared 25 fps non-drop labels with explicit day and in-source anchors. Original normalized correlation reports confidence and ambiguity, fits an exact constant clock rate, and rejects insufficient signal, excessive drift and inconsistent measurements. Returned identity-bound recipes require explicit `media.conform` calls to new output files; source media and sessions remain unchanged. Actual drift-corrected recordings were registered and rendered as a saved camera program. The MCP catalog now contains 40 tools. See [camera groups](MULTICAM.md) and [synchronization](SYNCHRONIZATION.md) for the complete bounds and workflow.

The two new independent fixtures compared **1,143 decoded frames, 2,194,560 stereo sample frames and 11 still previews**, with zero tolerance against their declared camera/resampling references, plus **70 rejection/failure cases**. Physical-clock accuracy is separately measured: +800/-600 ppm input drift was estimated as +798.611/-597.222 ppm; matched windows were within 0.6 samples. Corrected stereo RMS errors were 166.95/54.24 PCM16 units, against uncorrected left-channel errors of 7,429.24/7,284.32, meeting the explicit 256-unit and twentyfold-improvement bounds. Timecode tests include day rollover and nonzero media anchors. Retained fixtures are `C:\DEV\CutboltData\multicam-20261003-01` and `C:\DEV\CutboltData\synchronization-20261003-03`. Separate maximum-search and reversed-reference checks passed, and the documented camera operation and both synchronization methods executed successfully.

This earns **T10 basic and extended**, raising verified core coverage **66/100 to 68/100** and total implementation to **68/108 (63.0%)**. Full verification passed **268 unique checks**, formatting, lint, repository-boundary tests and material checks. Source fingerprints are current. The profile remains bounded 25 fps RGB8/48 kHz stereo, up to 16 camera alternatives and explicit constant-rate correction; confidence is estimated, timecode is caller-declared, and nonlinear/general-rate synchronization remains outside this slice. No dependency, proprietary material, acceptance wording, weight, exclusion or denominator changed. The roadmap's remaining-work table was also corrected to remove already delivered boundary/nesting checkpoints from its arithmetic. Agent foundations remain 10/10 separately and supplemental coverage 0/8. All **32 remaining core checkpoints** precede the eight supplemental checkpoints for **100/100 core, then 108/108 total**. The active goal continues; animated transforms are next.

Core continuation, reusable nested sequences, 3 October 2026: added named native child definitions and explicit `sequence_id` clip references. New `sequence.create`, `sequence.edit` and `sequence.remove` operations retain child arrangements in the existing snapshot and saved-session contract. Every instance maps its exact source window into the shared definition; edits propagate through all direct and indirect instances. Catalog validation rejects cycles, missing references, invalid source windows, duplicate definitions and excessive depth. Definition edits protect child tracks and every transitive locked reference, including disabled or unused parents. Saved diffs record changed definitions, affected root instances and root reference changes; retries, rollback, undo and restoration retain the catalog.

The original track compiler now composes child video/audio windows recursively, preserving independent parent/child transition clocks and off-window handles. Child video is opaque, including black child gaps. Each child's stereo mix saturates to PCM16 before parent mixing, keeping reused sound deterministic. Proxy selection follows enabled references; exact frame/range previews, reference exports and queued original-quality output use the same dependency graph. Every used source and effect handle is checked, including covered child video, with identities rechecked before publication. The original transition regression caught a changed validation order for extreme clocks; explicit overflow checks now precede handle inspection, and all original transition tests pass again.

The retained `C:\DEV\CutboltData\sequences-20261003-03` fixture and the complete verifier each compared **423 decoded frames, 812,160 stereo sample frames and 17 frame previews** across 16 rendered cases, with zero pixel or PCM tolerance. The independent reference builds complete child buffers with integer effect/mix equations before selecting parent windows. Cases include repeated and multilevel reuse, opaque gaps over a background, source/effect/mix edits, distinct subframe offsets, split instances inside effects, explicit source namespaces, eight levels, cycle/depth/expansion limits, all lock paths, source bounds, saved/MCP behavior, proxy ranges, missing proxies and queued full quality. All **23 rejection/failure cases** passed. The documented six-operation creation/placement batch and shared-child edit also executed successfully; a separate nested numerical-scope check matched the verified frame and both physical source interpretations.

This earns **T09 basic and extended**, raising core coverage **64/100 to 66/100** and total implementation to **66/108 (61.1%)**. Full verification passed **256 unique checks**, plus formatting, lint, 12 repository-boundary tests and material checks. The contract remains bounded 25 fps RGB8/48 kHz stereo, exact unit-rate child windows, 32 definitions, eight child levels and explicit graph-expansion limits; see [nested sequences](SEQUENCES.md). No dependency, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately and supplemental work 0/8. All **34 remaining core checkpoints** remain required before the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Multicam, audio/timecode synchronization, drift handling and recoverable angle edits are next; the goal remains active.

Core continuation, native boundary edits, 3 October 2026: added linked splits, source slips, rolling cuts, three-clip slides, interval insertion/overwrite and ripple deletion through the existing typed `tracks.edit` contract. Splits retain original synchronization anchors, allocate explicit right-child clip/link IDs and preserve transition clocks across continuous source fragments, including repeated and individual-sample audio splits. Roll/slide expand through linked neighbors while preserving their distinct timeline/source offsets. Interval edits expand to whole linked partner tracks and require explicit end and affected-transition policies. Every affected lock, range, handle, collision and link remains subject to atomic validation.

Independent acceptance compared **725 decoded frames, 1,392,000 stereo sample frames and 15 frame previews** across 25 rendered cases, with zero pixel or PCM tolerance. References use rational source mapping, integer transition equations, hand-computed slip/slide/roll placements and byte splices for interval edits. Tests cover source offsets differing by seven samples, split output invariance, linked-neighbor discovery, fixed/resized ends, explicit effect removal, unselected late content, all seven MCP edit schemas, saved diffs/retries/rollback/undo/restoration, range previews, proxies, reference exports and queued full quality. All **34 rejection/failure cases** passed, including changed media after encoding and existing destination preservation. The initial retained fixture at `C:\DEV\CutboltData\track-edits-20261003-01` compared 695 frames and 1,334,400 sample frames; the complete verifier additionally covers a successful repeated split and saved execution of all interval schemas. Five documented operations were executed successfully against the retained original sources.

This earns **T03 extended, T06 extended and T07 extended**, raising core coverage **61/100 to 64/100**, and total implementation to **64/108 (59.3%)**. Full verification passed **250 unique checks**, plus formatting, lint, 12 repository-boundary tests and material checks. Documentation is in [native boundary edits](TRACK_EDITS.md). The reference renderer remains bounded to 25 fps RGB8 and 48 kHz stereo PCM; fractional-rate/VFR/long-form timing, nesting and multicam retain their own requirements. No dependency, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations remain 10/10 separately; supplemental coverage remains 0/8. All **36 remaining core checkpoints** are still required before the eight supplemental checkpoints: **100/100 core, then 108/108 total**. Reusable nested sequences and their dependency/time/effect validation are next; the goal remains active.

Core continuation, editable transitions and source handles, 3 October 2026: native tracks now retain explicit transition IDs, adjacent endpoint clips, before/after-cut intervals and styles in their saved state. Video supports encoded-RGB8 dissolve, dip to black and wipes revealing incoming pixels from either side. Stereo audio supports linear crossfades and dips through silence, with exact sample-center weights and declared signed rounding before track summation. Declared and decoded source handles are checked separately. Overlapping intervals, broken adjacency, unavailable handles and locked edits reject the complete batch. Linked endpoint moves/retargeting carry transitions; endpoint removal/replacement reports incident transition removal in the existing track-layout diff.

The original track compiler now evaluates selected windows against the intact arrangement, retaining the transition's clock and off-range source handles. Frame/range previews, full-quality reference/delivery output, proxy previews and queued renders share those interval rules. Transition frame receipts list inspected sources; final publication rechecks their identities. Numerical scopes validate every input's color declaration, including conflicting metadata on the second transition source. Existing sequential range behavior and saved-session schemas remain compatible. No additional MCP tool or dependency was introduced.

The retained `C:\DEV\CutboltData\transitions-20261003-02` fixture compared **510 decoded frames, 979,200 stereo sample frames and 29 frame previews** across 32 rendered cases with exact independent Fraction/integer references. All four video styles, both audio styles, asymmetric/one-sided handles, one-frame effects, touching transitions, original motion/still/black content, temporary occlusion, mixed audio, subframe offsets, linked movement/retargeting, transition/endpoint removal, collateral replacement, locks, saved preview/retry/undo/restore, atomic rollback, inside-transition ranges, scopes, reference/H.264 exports, proxies and queued full quality passed. The retained run checked 20 rejection/failure cases. Review of extreme-clock arithmetic found an unchecked doubling of an audio interval; checked arithmetic and its regression test now return `TIME_OVERFLOW` without publishing output. The separate regression and full suite pass all **21 rejection/failure cases**, including changed-source preservation. Both documented edit operations passed with clip placements and the independent audio transition intact.

This earns **V03 basic and extended**, raising core coverage **59/100 to 61/100** and total implementation to **61/108 (56.5%)**. Full verification passed **243 checks**, plus formatting, lint, 12 repository-boundary tests and material checks. The transition profile remains bounded 25 fps encoded RGB8 and 48 kHz stereo PCM; linear-light/HDR transitions, native linked split/trim/ripple/rolling extensions, nesting and broader rate/sync behavior retain their separate requirements. No proprietary material, dependency version/license, acceptance wording, weight, exclusion or denominator changed. Agent foundations stay 10/10 separately and supplemental coverage stays 0/8. All **39 remaining core checkpoints** precede the eight supplemental checkpoints for **100/100 core, then 108/108 total**. Native linked boundary edits, including preservation of transition clocks across splits, are next.

Core continuation, native tracks and linked editing, 3 October 2026: added an original explicitly placed video/audio track model alongside existing sequential projects. `tracks.edit` creates or promotes timelines, adds/reorders/enables/locks tracks, places and retargets clips, moves/removes linked selections, and applies explicit reject/whole-clip replacement policies. Links retain rational content anchors, including subframe audio offsets. Every source, target and collateral linked-partner lock is checked before the existing atomic session transaction can commit. Session diffs expose track IDs, ordered track state and links; array-order-only changes within a placed track do not report unrelated clips as edited. Retry, undo, restoration, MCP schemas and queued full-quality rendering retain their existing contracts.

The original render graph selects the highest enabled opaque video track, fills uncovered video with black, and sums enabled stereo PCM at exact sample placements with one final saturation. A one-frame concat boundary initially dropped an output frame; assigning the final frame clock from exact retained frame counts fixes it. Independent output comparisons cover this regression. Frame/range previews, proxy selection, reference/delivery exports, sequential promotion and source identity checks use the native model without flattening its saved editing structure.

The retained `C:\DEV\CutboltData\tracks-20261003-05` fixture and the full verifier each passed all eight new track checks. **348 decoded frames and 668,160 stereo sample frames** matched independent Fraction interval selection, original per-pixel frame identifiers and integer PCM references with zero tolerance across 21 render cases. Twelve full-size previews and a half-size proxy preview were exact. Coverage includes simultaneous retargeting, linked seven-sample offsets, whole-group replacement, locks, sync rejection, exact touching endpoints, explicit gaps, saved preview/retry/undo/restore, failed-batch rollback, 32 simultaneous audio voices, 1,000-clip model validation, renderer/model limits, disabled missing media and 38 invalid/failure cases. A controlled source change prevented publication, and the fixture restored its synthetic source. Both documented track-edit examples passed too.

This earns **T08 basic and extended plus T04 extended**, raising core coverage **56/100 to 59/100** and total implementation to **59/108 (54.6%)**. Full verification passed **236 checks**, plus formatting, lint, 12 repository-boundary tests and material checks. Native track rendering remains a bounded 25 fps RGB8/stereo PCM profile. Track transitions, linked trim/split extensions, interval overwrite/ripple, slip/slide/roll, nesting, multicam and broader rate/sync cases remain required; none earned points here. No dependency version/license, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations stay 10/10 separately and supplemental coverage stays 0/8. All **41 remaining core checkpoints** precede the eight supplemental checkpoints for **100/100 core, then 108/108 total**. The next slice establishes transition/source-handle behavior and extends linked boundary edits with independent output and atomicity checks.

Core continuation, high-bit-depth and HDR processing, 3 October 2026: added original native RGB/YUV444 10/12/16-bit conversion with explicit PQ, HLG, sRGB/BT.709, full/limited range, D65 primary conversion, exposure and clip/extended-Reinhard tone policies. `hdr.inspect` shares CLI/library/MCP schemas and reports source interpretation and exact rational selection; `hdr.conform` remains synchronous CLI/library work. RGB16 intermediates carry declared transfer/primaries and standard Matroska display metadata. RGB8 SDR results become ordinary saved-session editing assets. The catalog now contains 39 MCP tools.

The retained `C:\DEV\CutboltData\hdr-20261003-03` fixture checked **131 decoded frames and 251,520 stereo sample frames** with independent 50-digit Decimal transfer/tone equations and exact Fraction primary/time/audio references. Every 16-bit code is covered for both PQ and HLG identity conversion. Native RGB10/12/16, full/limited YUV444, HLG peak/black variation, exposure, gamut conversion, tone mapping, SDR upconversion, standard display metadata, re-import, fast seeking, reverse/freeze/rate/resize, single-pixel/tall/wide/1080p output, saved sessions, previews, range export and delivery are exercised. Maximum RGB16 error was one code value; RGB8, identities and PCM were exact. External PQ-to-linear comparisons stayed below 0.489 nit under the declared `0.5 + 0.0002 * expected_nits` tolerance. Thirty-one invalid/failure cases preserved existing outputs and source identities. A controlled source change prevented final publication; its original synthetic bytes were restored. The documented commands passed on the retained fixture, as did a separate nonblack single-pixel exactness check.

A two-row timeline/scene experiment exposed the selected encoder's version-3 slice-grid corruption; requesting one version-3 slice also fails at larger dimensions. All engine FFV1 writers now share an explicit version-1/one-slice choice when either dimension is below four, and version-3/four-slice choice otherwise. External RGB8/RGB16 dimension matrices and engine acceptance fixtures verify this without removing supported dimensions. The full two-row scene chart, saved timeline and range preview are exact. These correctness fixes and the MCP catalog boundary check earn no separate editing points.

This earns **C05 basic and extended**, raising core coverage **54/100 to 56/100** and total implementation to **56/108 (51.9%)**. Full verification passed **228 checks**, including three additional Rust checks, eight HDR checks and one scene regression, plus formatting, lint, 12 repository-boundary tests and material checks. The existing general timeline remains 8-bit SDR; high-bit-depth/HDR processing is an explicit bounded conversion path with declared display assumptions. HDR distribution codecs, dynamic metadata, calibrated display control and general high-precision timeline effects are outside this profile. No dependency package/version/license, proprietary material, acceptance wording, weight, exclusion or denominator changed. Agent foundations stay 10/10 separately; supplemental coverage stays 0/8. All 44 remaining core checkpoints remain required, followed by eight supplemental checkpoints for the requested **100/100 core, then 108/108 total** endpoint. Native timeline tracks and linked editing are next.

Core continuation, LUT conversion and numerical scopes, 3 October 2026: added original bounded ASCII `.cube` loading with source identity checks, 1D nearest/linear and 3D nearest/trilinear/tetrahedral interpolation. Optional media-conform LUTs apply after explicit SDR normalization and before resizing, with per-channel domain clamping and clipped/rounded RGB8 output. The read-only `lut.inspect` tool reports table/sample facts. `scopes.inspect` returns every-pixel histogram, waveform/parade, vectorscope and min/max/sum values for one exact full-quality timeline frame, with declared color interpretation. Both commands share CLI/library/MCP dispatch, bringing the catalog to 38 tools. Generated tables and media stay external; `.cube` files are ignored by Git.

The retained `C:\DEV\CutboltData\luts-scopes-20261003-05` fixture checked **83 decoded video frames and 159,360 stereo sample frames** across 18 LUT conversions, a seven-frame edited timeline and a four-frame two-row proxy. Exact Fraction references cover one-dimensional knots, all tetrahedral orderings, multilinear cross terms, nonunit domains, out-of-range inputs and clipped output. Thirteen external filter comparisons supply a second independent implementation reference. **Maximum RGB difference was one 8-bit level**, within the declared one-level tolerance; identity interpolation, PCM samples and every scope bin/aggregate matched exactly. Five exact frame previews agree with scope input and final timeline pixels. Coverage includes maximum 1D/3D table sizes, color normalization before lookup, 1/7/256-column scopes, uneven bins, cut/gap endpoints, original quality with an offline selected proxy, MCP schema/results, saved-session replay/immutability and 55 invalid/failure cases. A controlled LUT change during conversion prevented publication, and the fixture restored its synthetic table. Documented inspect/convert/scope examples passed.

The fixture exposed an existing tiny-proxy issue in the installed encoder's default FFV1 slice layout. An independent 128x2 encode/decode comparison failed with default/four slices and matched exactly with one slice. Media conversion now selects one slice explicitly; the regression fixture verifies every proxy pixel and sample. The verification runner also refuses to publish evidence if fingerprinted sources change during its run. These correctness fixes add no separate editing points.

This earns **C04 basic and extended**, raising core coverage **52/100 to 54/100** and combined implementation to **54/108 (50.0%)**. Full verification passed **216 checks**, including two additional Rust numerical checks and seven LUT/scope checks, plus formatting, lint, 12 repository-boundary tests and material checks. No dependency package/version/license, proprietary material, acceptance wording, weight, exclusion or denominator changed. The LUT/scopes contract remains 8-bit encoded SDR and numerical single-frame inspection; HDR, display management, combined shapers, temporal scopes and live LUT scene effects remain outside this bounded profile. Agent foundations stay 10/10 separately and supplemental coverage stays 0/8. High-bit-depth/HDR processing is next. All 46 remaining core checkpoints remain required, followed by the eight supplemental checkpoints for the requested **100/100 core, then 108/108 total** endpoint.

Core continuation, explicit SDR normalization, 3 October 2026: the existing media-conform commands now accept an optional `source.sdr` declaration and required matching `working_transfer`. Original Rust conversion reads native bgr0/YUV444/YUV420 samples and applies declared RGB/BT.709 matrix, full/limited range and sRGB/BT.709 transfer interpretation. It produces a new tagged full-range RGB FFV1/PCM editing asset. Missing tags require an explicit reject/use-declared policy; all assumptions are reported, and known conflicting tags fail. Available decoded-frame color tags are checked too. Legacy conversion behavior, saved-session editing, proxy generation and local-only execution remain intact.

The retained `C:\DEV\CutboltData\color-20261003-02` fixture checked **252 decoded video frames and 483,840 stereo sample frames**, covering 30 normalization renders plus a mixed saved-session reference sequence. An independent 48-digit Decimal reference applies public transfer/matrix equations to original native samples and checks every output channel; Fraction mapping checks retiming. **Maximum RGB error was zero**, within the unchanged one-level threshold, and all PCM matched exactly. Coverage includes both transfers and conversion directions, full/limited RGB and YUV, out-of-nominal clipping, identity precision, tagged/untagged media, native YUV420 block reconstruction, output tags, reverse/freeze/rate changes and decoded repeatability. A mixed normalized timeline passed saved-session retry/immutability, MCP schema/inspection equality, exact reference assembly and delivery validation. Twenty-five invalid/failure cases preserved sources and existing outputs. The documented normalization example passed. The initial fixture attempt exposed the encoder's RGB-matrix option spelling; the generator and output command were corrected before successful verification, with no threshold change.

This earns **C01 basic and extended**, raising core coverage **50/100 to 52/100** and combined implementation to **52/108 (48.1%)**. Full verification passed **207 checks**, including one new Rust numerical test and six normalization checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog remains at 36 tools. Public sRGB and BT.709 references and the original conversion policy are documented in [COLOR.md](COLOR.md). No dependency package/version/license, bundled material, acceptance wording, weight, exclusion or denominator changed.

This is 8-bit SDR input normalization to common BT.709 primaries/D65 with explicit OETF interpretation. The timeline does not automatically infer or enforce a global working space. HDR, ICC/wide-gamut conversion, display transforms and higher-quality chroma reconstruction remain outside this profile; their applicable broader criteria stay open. Agent foundations remain 10/10 separately and supplemental coverage 0/8. LUTs and numerical scopes are next. The requested endpoint remains **100/100 core, then 108/108 total**.

Core continuation, range/stream exports and a validated delivery preset, 3 October 2026: added read-only `export.inspect` and blocking `export.run` for exact 25 fps timeline intervals, whole sequences, and audio/video-only output. Reference formats preserve FFV1 RGB and PCM samples. The fixed H.264 High/AAC-LC preset exports opaque limited-range BT.709 MP4/M4A using the existing external tools, with an explicit caller choice of sRGB or BT.709 input interpretation. Original public transfer equations and declared matrix/range/chroma conversion replace implicit transfer defaults. Full-quality originals are selected even when an offline proxy preview is active. Supplied snapshots and saved-session revisions are unchanged.

The retained `C:\DEV\CutboltData\delivery-20261003-04` fixture checked **790 decoded video frames and 1,514,880 presentation stereo sample frames across 26 exports**, plus **28 rejected/failure cases**. Independent Fraction source slices and PCM expectations prove lossless equality, frame identity and exact range boundaries across cuts/gaps. Decimal public transfer/matrix calculations measured **zero flat-patch YUV error**, minimum delivery RGB PSNR **34.351 dB** and minimum nonsilent AAC SNR **33.882 dB**, above the unchanged 30 dB RGB and 28 dB audio thresholds; best audio alignment was zero samples. Tests cover both input transfers, one-frame and AAC-block-aligned ranges, 1080p/minimum dimensions, odd-size reference output, stream-only exports, metadata removal, faststart, decoded repeatability and original hashes. MCP inspection/schema equality and saved-session retries/immutability pass. Controlled generated-output corruption and a temporarily changed synthetic source both prevent publication; the fixture restores its controlled source change. Existing outputs and sources remain intact. The documented inspect/export workflow also passed.

An initial transfer-filter candidate failed the color threshold and was replaced with explicit public equations; the threshold was not weakened. An initial 192 kb/s AAC setting failed the one-frame quality check, so the fixed preset uses 320 kb/s. Runtime verification checks exact video timestamps and track presentation duration, a 1,024-sample AAC priming interval and separately reported 0..1023 trailing decoder-padding samples. It does not misreport raw decoded AAC padding as added timeline duration or promise lossless delivery audio.

This earns **E02 basic and E05 basic**, raising verified core coverage **48/100 to 50/100**, combined implementation **50/108 (46.3%)**. Full verification passed **200 checks**, including six delivery checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog now has 36 tools. Exact external encoder/component identities and the existing executable's reported GPL-3.0-or-later license are recorded in the dependency ledger; no dependency package/version or bundled material was added. See [the export contract](EXPORT.md) for the profile, evidence and limits.

This completes the initial 32-point route from 18/100 to the intermediate 50/100 milestone. No acceptance criteria, weights, exclusions or denominators changed. Agent foundations remain 10/10 separately and supplemental coverage remains 0/8. Broader device/bitrate/format matrices, project-wide color normalization and queued delivery/recovery remain open. The roadmap retains all 50 remaining core checkpoints; explicit SDR normalization is next, followed by LUTs/scopes. The requested endpoint remains **100/100 core, then 108/108 total**.

Core continuation, chroma keying and reusable presets, 3 October 2026: ordered scene effects now include an original opponent-channel chroma key with explicit hard/soft thresholds, existing-alpha multiplication, optional encoded screen-color subtraction and named-channel spill suppression. Overall key strength and source-canvas feathered mask rectangles share the rational layer-local animation clock. Keys, primary grades and selective corrections can be ordered independently on each layer; alpha is quantized only after the complete chain. Disabled/unaffected pixels preserve their original alpha interpretation. A new read-only `effects.preset` command returns four independent editable green/blue hard/soft recipes, available through CLI/library and MCP. Presets leave optional screen subtraction off and require explicit strength/spill arguments.

The retained `C:\DEV\CutboltData\keying-20261003-02` fixture compared **182 decoded frames and 349,440 silent stereo sample frames** across 42 scene renders and one saved-session cut. Independent Fraction opponent geometry, masks and alpha products, 48-digit Decimal color recovery/processing and forward composition check hard threshold ties, soft edges, low alpha, eight-stage alpha accumulation, all blend modes, mask transforms, mixed effect order, layer isolation and animated looping sources. Known original neutral foreground edges and single-pixel strands recovered exactly with screen subtraction; the same deliberately constructed case had up to 64 levels of error without it. General numerical comparison stayed within **one 8-bit RGB level**, and all PCM samples matched exactly. All four presets, independent copies, MCP schemas, template reuse and saved-session retries passed. Twenty-nine rejected/output-preservation cases and source hashes verified failure safety. One direct Rust test checks chroma anchors, gray-offset cancellation and hard threshold boundaries. The documented preset/inspect/render workflow also passed. An initial test expected an omitted mask default to remain omitted; the corrected fixture compares an explicit default before crediting template round-trip evidence.

This earns **V08 basic and extended**, raising verified core coverage **46/100 to 48/100**, combined implementation **48/108 (44.4%)**. Full verification passed **194 checks**, including eight keying checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog now has 35 tools. No dependency versions/licenses, acceptance criteria, weights, exclusions or denominators changed. See [the keying contract](KEYING.md) for original equations, presets, evidence and limits.

The PNG/graphics scene path remains bounded to ten seconds, 16 layers and eight effects per layer; compiled reference output is flattened RGB. These fixture results do not claim arbitrary-footage matting quality, automatic screen estimation, spatial/temporal matte cleanup, tracking or a general native video-track effects path. Agent foundations remain 10/10 and supplemental coverage 0/8. This completes the six planned grading/selective/keying checkpoints in the initial route to 50. A validated delivery preset and selected-range/audio/video exports are next. The requested endpoint remains **100/100 core, then 108/108 total**.

Core continuation, selective color correction, 3 October 2026: ordered scene effects now include an original `selective_grade` effect that applies primary grading through HSL color qualifiers, feathered source-canvas rectangular masks, or both. Color bands have explicit inclusive hard thresholds, soft falloff, hue wraparound, achromatic behavior and whole-qualifier inversion. Qualification measures the color entering each effect using deterministic integer HSL coordinates derived from quantized straight sRGB; the working correction retains linear-light precision. Mask rectangles, correction strength and grade controls use the existing rational layer-local animation clock. Unselected pixels retain their original alpha interpretation, including fractional stored premultiplied colors, and selected corrections preserve alpha. Inspection, templates and compiled assets in saved sessions share the implementation.

The retained `C:\DEV\CutboltData\selection-20261003-01` fixture compared **158 decoded frames and 303,360 silent stereo sample frames** across 34 scene renders and one saved-session cut. Independent Fraction HSL geometry, color-band and mask weights, 48-digit Decimal grading and forward composition verify hue wraparound, half-tie thresholds, grays/near-grays, inverted/product qualifiers, thin/empty/outside masks, all four rotations with crop/trim offsets, blend modes, low alpha, ordered chains and layer isolation. Animated mix/masks/grades, looping source frames, shapes, template reuse, MCP/schema validation and saved-session retries are covered. Every RGB value matched exactly within the allowed one-level tolerance; PCM and unselected/neutral bypass matched exactly. Twenty-seven rejection/output-preservation cases and source hashes verified failure safety. Two direct Rust tests check known HSL values, half ties and mask coverage. The documented replacement/render workflow also passed.

This earns **C03 basic and extended**, raising verified core coverage **44/100 to 46/100**, combined implementation **46/108 (42.6%)**. Full verification passed **185 checks**, including seven selective-correction checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog remains at 34 tools and no dependencies were added. Public HSL definitions are cited in [the selective-correction contract](SELECTIVE_COLOR.md); quantization, weights, masks, processing and fixtures are original project design.

This bounded PNG/graphics scene path retains its ten-second, 16-layer and eight-effect-per-layer limits. Chroma key, tracking, broader color management and direct general video-track effects remain open. No acceptance criteria, weights, exclusions or denominators changed; agent foundations remain 10/10 and supplemental coverage 0/8. Reusable keying/effects are next, followed by delivery presets and stream/range exports toward the intermediate 50-point milestone. The requested endpoint remains **100/100 core, then 108/108 total**.

Core continuation, primary grading, 3 October 2026: scene layers now support up to eight ordered original grade effects with exposure, contrast about linear 18% gray, explicit diagonal RGB white balance and master/channel tone curves. Five numerical controls share the existing rational layer-local animation clock, easing, delay/rate/reversal and endpoint rules. Grading decodes declared sRGB to linear light, applies the documented control order and per-grade clipping, then quantizes straight sRGB before the existing compositor. Alpha is preserved, stored premultiplied colors are unpremultiplied before processing, and wholly neutral chains retain the original integer compositor exactly. PNG sequences, text and shapes share the path. Scene inspection reports declared effects and sampled controls; templates retain complete grade recipes and compiled assets enter normal saved-session editing.

The retained `C:\DEV\CutboltData\grading-20261003-02` fixture compared **128 decoded frames and 245,760 silent stereo sample frames** across 24 scene renders and one saved-session cut. An independent 48-digit Decimal reference, analytical exposure/gray anchors, Fraction clocks, original glyph/shape geometry and forward composition check full grayscale ramps, original RGB charts, neutral-patch correction, extrema, nonmonotonic/master/channel curves, clipping and effect-order differences. Coverage includes eight-stage chains, all blend modes, low/zero/full alpha, fractional stored premultiplied colors, transparent trimmed-image regions, looped source frames, all five animated controls, text/shapes, MCP/schema validation, template reuse and saved-session retries. Maximum RGB error was **one 8-bit level**, within the one-level tolerance; neutral bypass and all PCM samples matched exactly. Thirty rejected cases and source/output hashes verified preservation. The documented grade-replacement/render workflow also passed.

This earns **C02 basic and extended**, raising verified core coverage **42/100 to 44/100**, combined implementation **44/108 (40.7%)**. Full verification passed **176 checks**, including seven grading checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog remains at 34 tools because effects extend the existing scene schema. Public sRGB transfer-function facts are cited in [the grading contract](GRADING.md); grading behavior and fixtures are original, with no new dependencies or bundled materials.

General color management, secondary qualifiers/masks, LUT/scopes, HDR, keying and direct general video-track effects remain open. Existing scene duration, layer, format and output limits remain explicit. No acceptance criteria, weights, exclusions or denominators changed. Agent foundations stay 10/10 and supplemental coverage 0/8. Selective correction and reusable keying/effects are next, followed by delivery work toward the intermediate 50-point milestone. The full target remains **100/100 core, then 108/108 total**.

Core continuation, timed captions, 3 October 2026: added six original CLI/library/MCP commands for bounded UTF-8 SRT/WebVTT import, native document inspection, immutable revision-guarded edits, encoding/loss previews, safe sidecar export and caption-to-scene conversion. Cues retain exact rational half-open intervals independently of video frames. Edits add/replace/remove cues, shift selected times and change color styles/overlap policy atomically. WebVTT preserves whole-cue colors, alignment and speaker metadata; SRT reports renumbering and removed formatting, requiring an explicit loss policy before publication. Existing outputs and original sources remain protected.

The retained `C:\DEV\CutboltData\captions-20261003-03` fixture compared **47 decoded frames and 90,240 silent stereo sample frames** with exact RGB/PCM agreement. Independent Fraction clocks, original font geometry and forward image composition verify multiline layout, fallback, simultaneous styled cues, frame-boundary and between-frame cues, a one-hour window, skipped cues and a saved-session cut. External FFprobe independently checks exported subtitle packet text and exact timing, including Unicode sidecars and reported-loss exports. The fixture covers atomic edits, deterministic replay, six MCP commands, schema validation, 43 invalid/output-preservation cases and source hashes. A class named `default` is explicitly tested to leave unclassed text white. The documented import/edit/encode/export example also passed against the retained fixture.

This earns **G03 basic and extended**, raising verified core coverage **40/100 to 42/100**, combined implementation **42/108 (38.9%)**. Full verification passed **169 checks** across the Rust, rendering, session, MCP/job and feature suites, including six new caption checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog now has 34 tools. No dependency versions, licenses, acceptance wording, weights, exclusions or denominators changed. See [the caption contract](CAPTIONS.md) for the supported format subset and public specification references.

Caption documents remain caller-managed snapshots; compiled assets use ordinary saved sessions. Scene rendering remains bounded to ten seconds and 16 total layers, with explicit external fonts/layouts and frame-start sampling. General WebVTT/CSS layout, complex text shaping, transcription, embedded delivery subtitle streams and production review integration remain open. Agent foundations stay 10/10 and supplemental coverage 0/8. This completes the five planned graphics/template/caption checkpoints in the initial route to 50. Grading, selective correction and reusable keying/effects are next, followed by delivery work. The requested endpoint remains **100/100 core, then 108/108 total**.

Core continuation, reusable graphics templates, 3 October 2026: `graphics.instantiate` now expands original portable scene templates through explicit typed bindings. Text, integers, RGBA colors, font identities, curves, rational times and rectangles have declared defaults/constraints and compatible target properties. One parameter may control several layers; duplicate targets, static/animated conflicts, unknown/missing values, incompatible fields and invalid resolved scenes fail before returning a result. Each instance is independent and editable, with template/scene digests, resolved values, default-use reporting and complete source/layout inspection. The operation is read-only and available through CLI/library and MCP. Original lower-third and title-card JSON recipes are included; fonts remain external.

The retained `C:\DEV\CutboltData\templates-20261003-02` fixture compared **70 decoded frames and 134,400 silent stereo sample frames**, with exact RGB/PCM agreement across five scene renders and one two-instance saved-session sequence. Independently assembled expected scenes and original font geometry verify preset variants, shared fades/movement, colors, multiline text, timing and typed layout bindings. Reordered requests, reordered parameter declarations, repeated MCP calls, schema validation, isolated instance digests and saved-session retries are exercised. Forty invalid cases reject malformed values, conflicting/missing bindings, limit violations, identity failures and post-binding timing/layout errors without partial results or filesystem writes. Original examples and external fonts remain unchanged.

This earns **G02 extended**, raising verified core coverage **39/100 to 40/100**, combined implementation **40/108 (37.0%)**. Full verification passed all existing suites plus five template checks, formatting, lint, 12 repository-boundary tests and material checks. Template examples are included in source fingerprints so changes require fresh verification. The MCP catalog now has 28 tools; dependency versions and licenses are unchanged. See [the template contract and runnable workflow](TEMPLATES.md).

Agent foundations remain 10/10 and supplemental coverage 0/8. All acceptance criteria, weights, exclusions and denominators remain unchanged. Template binding does not implement property expressions, complex shaping or captions. Timed caption import/edit/export, styles and overlap handling are next, followed by the remaining core work toward **100/100**, then the eight supplemental checkpoints for **108/108 total**.

Core continuation, original text and shape layers, 3 October 2026: scenes now accept text with explicit external TrueType font identities and original rectangle/ellipse shapes with fill and inside strokes. Bounded LTR scalar layout provides ordered font fallback, multiline alignment, scalar wrapping and explicit overflow behavior. Generated straight-alpha pixels reuse scene crop/rotation/scale, blending, masks and exact rational position/opacity animation. Inspection exposes glyph layout, chosen fonts and sampled properties; compiled assets enter ordinary saved-session editing. Existing PNG recipes keep their behavior. See [the graphics contract](GRAPHICS.md).

The retained `C:\DEV\CutboltData\graphics-20261003-03` fixture compared **100 decoded frames and 192,000 silent stereo sample frames**, covering 12 scene renders plus a saved-session timeline cut. Original abstract TrueType outlines are generated outside the repository. Independent known-outline geometry, analytic pixel-area coverage, rational layout/blending and forward transforms check every output pixel. Integer-aligned cases match exactly; fractional glyph edges differ by at most one 8-bit RGB level within the declared one-level tolerance. Individual text and shape layers are checked for visible animation, alongside combined mask/transform behavior. Thirty-five invalid inputs fail without publishing output; font/source identities and existing outputs remain unchanged. An earlier fixture whose mask completely hid the animated shape was corrected before the checkpoint was awarded.

This earns **G01 basic and G02 basic only**, raising verified core coverage **37/100 to 39/100**, combined implementation **39/108 (36.1%)**. Full verification passed all existing suites and five graphics checks, with formatting, lint, 12 repository-boundary tests and material checks. Fontdue 0.9.4 and its exact parser/math dependencies were selected for glyph rasterization from the external Cargo cache; FontTools 4.25.0 generates original test fonts externally. Versions and licenses are recorded in the dependency ledger. No font, copied implementation or dependency source was vendored.

Complex shaping, bidi/combining behavior, reusable templates and captions remain open; basic LTR fallback does not earn G01 extended. Agent foundations remain 10/10, supplemental work 0/8, and all acceptance criteria, weights, exclusions and denominators remain unchanged. Reusable graphics templates and captions are next. The requested endpoint remains **100/100 core followed by 108/108 total**.

Core continuation, explicit gaps and mixed-source boundaries, 3 October 2026: sequential timelines now accept explicit gap items without a media asset, producing exact black RGB and silent PCM. Gaps participate in append/trim/split/move/remove, insert/overwrite/ripple and neighboring roll/slide edits; source-in remains zero in every gap fragment. Frame/range and proxy-sized previews, saved diffs/replay/undo, gap-only output and queued renders share this representation. Missing media references do not silently become gaps. Existing media-only JSON retains its serialized form. Rendering still uses the declared 25 fps reference profile, with at most 64 items and 180,000 frames per gap.

The retained `C:\DEV\CutboltData\timeline-edges-20261003-01` fixture compared **727 decoded frames and 1,368,960 stereo sample frames**, covering 24 edit/trim render cases, gap-only odd-size output, boundary previews and queued rendering. Independent Fraction clocks and RGB/PCM lists check original 24 fps, 30000/1001 fps keyframe-coded and audio-only inputs through the explicit media-conform path, including first/last-frame trims, a non-keyframe start crossing a keyframe, and final converted mono samples. Eighteen invalid cases were rejected with saved-state atomicity and source/output preservation. Source decoding uses the existing external codec boundary; arbitrary direct native-codec timeline playback, subframe audio edits and general tracks are not claimed.

This earns **T01 extended and T02 extended**, raising verified core coverage **35/100 to 37/100**, combined implementation **37/108 (34.3%)**. All prior verification suites and six new timeline-edge checks passed, along with formatting, lint, 12 repository-boundary tests and material checks. Dependencies, criteria, exclusions and denominators are unchanged. Agent foundations remain 10/10; supplemental work remains 0/8. This completes the six planned source-format/proxy/timing checkpoints in the initial route to 50. Text/shape overlays, animated graphics and captions are next; every remaining core checkpoint remains required before the eight supplemental checkpoints and **108/108 total**.

Core continuation, proxy editing, 3 October 2026: added identity-bound half/quarter/eighth-dimension proxy generation for full-resolution reference assets. Optional proxy bindings and preview selection persist in existing saved sessions with visible diffs, retries, detach, undo and restore. Frame/range previews honor the selected scale and explicitly reject missing or mismatched variants. Separate content-checked proxy relinking supports moved media; final synchronous and queued exports always read full-quality assets and can finish while proxies are offline. Native mixed-rate inputs first use the declared media-conform path, preserving its exact selected frame clock. See [the proxy contract and bounds](PROXIES.md).

The retained fixture at `C:\DEV\CutboltData\proxies-20261003-03` compared **568 decoded frames and 1,086,720 stereo sample frames** against independent timestamp selection and spatial sampling. It covers original 24 fps, 30000/1001 fps and VFR inputs, all three scales, frame/range previews, source boundaries, saved switching/replay/undo/detach, offline masters/proxies, identity-based relinking and both export paths. Seventeen invalid cases were rejected, with original native/master/proxy bytes preserved through controlled fixture moves and existing outputs protected. Source decoding uses the existing external codec boundary; no general speed or long-form performance claim is made.

This earns **M04 basic and extended**, raising core coverage **33/100 to 35/100**, combined implementation **35/108 (32.4%)**. Full verification passed all existing Rust, render, session, MCP/job, scene/preview, animation, compositing/mask, easing/retiming, registry/performance, sequential-edit, audio and media-conform suites, plus six proxy checks, formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog now has 27 tools. Dependencies, criteria, exclusions and denominators are unchanged; agent foundations remain 10/10 and supplemental work 0/8. Gaps and broader mixed-source trim/edit boundaries are next. The active endpoint remains **100/100 core, then 108/108 total**.

Core continuation, validated media formats and retiming, 3 October 2026: added `media.conform.inspect` and blocking `media.conform` to validate identity-bound local sources and compile new reference editing assets. The declared matrix covers FFV1/PCM MKV, tagged limited BT.709 H.264/AAC MP4/MOV and PCM16 WAV, including fractional/VFR video timestamps and 24/44.1/48 kHz mono/stereo audio. Original rational timestamp selection provides constant speed, reverse and freeze, with explicit nearest resize and linear audio conversion. Forward speed changes pitch; reverse/freeze require mute. Source files stay unchanged and complete decoded output hashes are checked before no-overwrite publication. See [the complete contract and limits](CONFORM.md).

Independent acceptance compared **381 decoded frames and 731,520 stereo sample frames across 15 conversions and one saved-session sequence**, using Fraction clocks, source timestamp lookup and independent resize/resample expectations. The fixture covers B-frames, fractional/VFR selection, speed bounds, reverse/freeze, audio-only clips, saved trimming/assembly and MCP inspection. Twenty-two malformed, unsupported or invalid cases were rejected; source hashes and existing outputs were preserved. Source codec decoding is delegated to external FFmpeg on both sides; the reference independently tests engine mapping and conversion, not the codec implementation. Retained fixture: `C:\DEV\CutboltData\conform-20261003-03`.

This earns **M01 extended and V07 basic only**, raising verified core coverage **31/100 to 33/100**, combined implementation **33/108 (30.6%)**. Full verification passed 22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask, seven easing/retiming, seven registry/performance, four sequential-edit, seven audio-mix, five audio-processing and six media-conform checks, plus formatting, lint, 12 repository-boundary tests and material checks. The MCP catalog now exposes 25 tools and correctly routes names with multiple command separators. No package versions, criteria, exclusions or denominators changed. Agent foundations remain 10/10; supplemental work remains 0/8. Proxies, broader mixed-format timeline edges, speed ramps, motion interpolation, pitch-preserving audio and color management remain open. Continue toward **100/100 core, then 108/108 total**.

Requested delivery sequence, 3 October 2026: the user requested continuing from **100/100 core to 108/108 total implementation** once the core milestone is complete. The eight existing supplemental checkpoints (3D geometry/lighting, temporal integration, typed expressions/links and local foreground segmentation) are now explicitly the follow-on delivery milestone. No criteria, evidence, scores, exclusions or denominators changed. Current verified coverage remains **31/100 core, 0/8 supplemental, 31/108 total (28.7%)**, with agent foundations separate at 10/10. Core media formats, proxies and timing remain the immediate next work.

Core continuation, audio processing, 3 October 2026: mix recipes now accept up to eight ordered master effects: low-pass/high-pass/peaking EQ and a project-defined linked-stereo compressor with attack, release, ratio and makeup gain. Processing follows wide voice summation and precedes final clipping; empty chains retain the exact integer path. Final PCM reports stereo integrated loudness using public 48 kHz K-weighting/gating equations, per-channel sample peaks and RMS, explicit short/silent results and gate/tail counts. See [processing behavior and references](AUDIO.md#master-processing-and-meters). True-peak, surround metering and certification are not claimed.

Independent acceptance compared **470,400 stereo sample frames across 15 output comparisons**, with zero PCM error in the retained run against external EQ filters, closed-form compressor step responses and inverse-filter expectations (declared tolerance: one PCM unit). Seven loudness comparisons against a separate meter differed by at most **0.045871 LU** (tolerance: 0.11 LU, including its display rounding). Tests cover calibrated tones, absolute/relative gating, silent/short signals, incomplete final windows, output peak/RMS accuracy, eight-effect chains, noncommuting processing order, pre-clipping headroom, recipe/MCP replay, 12 rejected requests and unchanged source hashes. Retained fixture: `C:\DEV\CutboltData\audio-processing-20261003-03`.

This earns **A04 basic and extended**, raising core coverage **29/100 to 31/100**, combined implementation **31/108 (28.7%)**. Full verification passed 22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask, seven easing/retiming, seven registry/performance, four sequential-edit, seven audio-mix and five audio-processing checks, plus formatting, lint, 12 repository-boundary tests and material checks. No dependencies, criteria, exclusions or denominators changed. Agent foundations remain 10/10 and supplemental work 0/8. The **100/100 goal remains active**; broader source formats, proxies and media timing are next.

Core continuation, audio mixing, 2 October 2026: the original PCM recipe now supports sample-based source cuts and placement, 24/44.1/48 kHz mono/stereo inputs, explicit channel mapping, clip/track gain and mute, linear fades, gain automation, crossfades and wide multitrack summation with final saturation. It exports a verified WAV or compiles directly as a scene soundtrack; compiled assets retain the existing saved-session contract. The editable mix recipe remains separate from session revisions. Limits are 60 seconds, 16 tracks, 128 clips, eight million summed clip sample frames and 64 MiB of source files; scene limits still apply. See [audio behavior](AUDIO.md).

Independent acceptance compares **10,543 stereo sample frames across 22 WAV renders**, plus scene/session pixels and PCM, MCP recipe reload, 16 rejected cases, source hashes and no-overwrite behavior. The retained audio fixture is `C:\DEV\CutboltData\audio-20261002-02`. A one-frame integration fixture exposed missing output-rate metadata after concatenation; reference rendering now explicitly declares its required 25 fps and the complete regression suite passes.

This earns **A01 extended, A02 basic/extended and A03 basic**, increasing core coverage **25/100 to 29/100** and combined implementation **29/108 (26.9%)**. Full verification passed 22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask, seven easing/retiming, seven registry/performance, four sequential-edit and seven audio-mix checks, plus formatting, lint, 12 repository-boundary tests and material checks. Agent foundations remain 10/10; supplemental work remains 0/8. No dependencies or acceptance criteria changed. EQ/dynamics, loudness/true-peak metering, buses/pan/surround and broader timeline audio remain open.

Goal update: during this batch the user expanded the active target from **50/100 to 100/100** under the same 50 capability groups, criteria, exclusions and denominator. The initial route to 50 remains an intermediate milestone. All remaining core checkpoints, including compatibility and performance, are still required; no planning or research earns points. Next add and independently verify bounded EQ, dynamics and loudness/sample-peak meters before broader media-format work.

Core-50 milestone, second batch, 2 October 2026: explicit insert, overwrite and ripple-delete operations now preserve surviving source intervals and require caller-chosen IDs for split survivors. Slip changes selected source content, roll moves a shared cut, and slide trims both neighbors while preserving the middle clip and total duration. All use exact frame-aligned rational times, validate source handles, expose placement/source diffs and retain transactional retry/undo semantics. Independent acceptance compared **2,080 decoded frames and 3,993,600 stereo sample frames across 14 renders**, plus 14 rejected cases and atomic saved-state failure. The retained fixture is `C:\DEV\CutboltData\editing-20261002-01`.

This earns **T06 basic and T07 basic**, increasing core coverage **23/100 to 25/100** and combined implementation **25/108 (23.1%)**. The full suite passed 22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask, seven easing/retiming, seven registry/performance and four sequential-edit checks, plus formatting, lint, 12 repository-boundary tests and material checks. A final acceptance review strengthened the registry fixture to physically move/reload its serialized project, relink its moved media, save to a new project file and confirm the transported original remains unchanged; the complete rerun passed. Track locks, gaps, transitions, mixed media and subframe audio operations remain open. Scope, criteria, exclusions, dependencies and the 100-point denominator are unchanged. The **50/100 goal remains active**; sample-based audio editing/mixing is the next foundation before broader format work.

Core-50 milestone, first batch, 2 October 2026: searchable asset bins/tags/fields and content-identity relinking now use ordinary project snapshots and saved-session operations, diffs, retries and undo/history. Render and preview enforce bound identities; moved media must match an explicit candidate and ambiguous matches fail. A 1,000-asset/clip fixture verifies paged search, metadata/duplicate behavior, validation and a complete 1,000-edit batch under five seconds with Windows peak working set below 256 MiB. The retained run at `C:\DEV\CutboltData\registry-20261002-01` measured about 0.060 s validation, 2.442 s batch editing and 41 MiB peak working set, and compared all 25 relinked frames and 48,000 stereo sample frames. The renderer's separate 64-clip bound remains unchanged; long-form 4K performance is still open.

This earns **M02 basic/extended, M03 basic/extended and P03 basic**, increasing core coverage **18/100 to 23/100**, combined implementation **23/108 (21.3%)**. Agent foundations stay 10/10, supplemental work 0/8. Full verification passed 22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask, seven easing/retiming and seven registry/performance checks, plus formatting, lint, 12 repository-boundary tests and material checks. The initial full attempt caught a stale SDK tool-count assertion; both catalog checks now cover the four additional tools, and the complete rerun passed. No scope/criteria/denominator/exclusions or dependencies changed. The user-requested **50/100 target remains in progress**.

Research implementation continuation, 2 October 2026: added original quadratic ease-in, ease-out and ease-in-out interpolation plus independent per-curve retiming for layer position, opacity and rectangular masks. Retiming anchors playback at a declared layer-local time, supports rational rates from 1/16 to 16 and optional reversal, preserves authored key spacing and clamps endpoints. Source-frame selection, audio and layer visibility stay unchanged. The evaluator uses exact rational times and checked integer arithmetic; excessive derived precision fails explicitly before output publication. Independent full-frame references, reversed hold boundaries, shuffled-time evaluation, per-property clocks and repeated MCP inspection earn **V04 extended only**. Core coverage increases **17/100 to 18/100**; total implementation increases **17/108 (15.7%) to 18/108 (16.7%)**. Agent foundations remain **10/10**, supplemental work **0/8**. No acceptance criteria, denominators, exclusions or dependencies changed.

The retained fixture at `C:\DEV\CutboltData\easing-20261002-01` compared **950 decoded frames across 19 renders** against independent Fraction polynomials and forward image/mask operations. Fourteen invalid or precision-exceeding inputs were rejected without publishing output; original sources and existing outputs were preserved. Full verification passed **22 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation, eight compositing/mask and seven easing/retiming checks**, plus 12 repository-boundary tests, formatting, lint and material checks. The verifier refreshed both progress trackers and evidence. General transform interpolation, control-handle curves, media retiming, tracking/feathering and later phases remain open. See the [roadmap](IMPLEMENTATION_ROADMAP.md) and [scene contract](SCENES.md).

Research implementation continuation, 2 October 2026: added original normal/multiply/screen compositing with explicit straight or stored premultiplied alpha, single-round channel arithmetic, invalid premultiplied-pixel rejection, and one source-canvas rectangular opacity mask per layer. Masks support inversion and layer-local rational hold/linear curves for position and dimensions, applied before crop/rotation/scale. Full decoded-frame comparison against independent normalized blend equations and forward mask/image transforms earns **V02 extended and V05 basic only**. Core editing increases **15/100 to 17/100**, and the combined plan increases **15/108 (13.9%) to 17/108 (15.7%)**. Agent foundations stay **10/10**, supplemental work **0/8**. All criteria, denominators and exclusions are unchanged. Feathering/tracking, easing/retiming, general transforms, linear-light/HDR and transparent output remain open.

The retained compositor fixture at `C:\DEV\CutboltData\compositing-20261002-01` checks 465 decoded frames across 31 renders, including low/zero/full alpha, straight/premultiplied equivalence, hidden colors, rotated and inverted masks, save/reload, shuffled keys and MCP inspection. Eleven invalid inputs fail without publishing outputs; source and existing-output hashes are preserved. Full verification passed **19 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview, six animation and eight compositing/mask checks**, plus 12 repository-boundary tests, formatting, lint and material checks. No dependencies were added. Progress and evidence were generated by the verifier; the [roadmap](IMPLEMENTATION_ROADMAP.md) records the delivered limits and remaining phase-1 work.

Research implementation start, 2 October 2026: the user requested an implementation plan covering the reviewed investigations and an initial delivered slice. Established `research-implementation-v1` as a separate combined **108-point** plan: the unchanged `cutbolt-local-v1` core denominator of 100 plus eight new checkpoints for 3D geometry/lights, temporal integration, typed property expressions and local segmentation. All eight additions begin unimplemented at 0/8; no original capability, criterion, weight, evidence or exclusion was removed. The prior 14/100 core score therefore starts at 14/108 (13.0%) on the expanded plan. A private mapping covers all investigated areas; research setup and audit completion earn no editing points.

The first original implementation adds optional layer-local position/opacity keyframes to bounded pixel scenes, with exact rational hold/linear interpolation, clamped endpoints, integer rounding, insertion-order independence and explicit validation. Independent sampled-value and full decoded-pixel checks earn **V04 basic only**, raising core editing to **15/100** and combined implementation to **15/108 (13.9%)**; agent foundations stay **10/10**, supplemental work **0/8**. General easing/retiming, animated scale/rotation and other phases remain open. The retained animation fixture compared all 150 decoded frames and rejected twelve invalid cases without publishing outputs or changing sources. Full verification passed 16 Rust, 19 render, 13 session, 24 MCP/job, 13 scene/preview and six animation checks, plus the 12 repository-boundary tests, formatting, lint and material checks. Generated progress/evidence were refreshed through the verifier. See the [roadmap](IMPLEMENTATION_ROADMAP.md) and [combined tracker](IMPLEMENTATION_PROGRESS.md).

Percentages measure the fixed scoped acceptance checklist, not source-code coverage of another application, time spent, or cross-product output equivalence.

| Date | Baseline | Score | Change |
| --- | --- | ---: | --- |
| 2026-10-02 | cutbolt-local-v1 | 0.0% | Established 50 capability groups with 100 acceptance checkpoints. User accepted exclusions for accounts/activation, cloud services, stock marketplaces, telemetry and GUI-only workflows. |
| 2026-10-02 | cutbolt-local-v1 | 10.0% | First verified slice: local media inspection, assembly, trim, split, reorder/remove, exact timing foundation, synchronized audio cuts, reference export, source/output protection, and native snapshot round trips. All broader checkpoints remain open. |
| 2026-10-02 | cutbolt-local-v1 | 10.0% | Renamed the project to Cutbolt. Reliability first: local saved sessions, durable retries, undo/restoration, semantic diffs and history; process-crash and concurrent-writer evidence. Agent foundations increase 5/10 to 8/10, separately. No additional editing points: I01 extended still requires migrations and relative media paths. Scope, weights and exclusions unchanged. |
| 2026-10-02 | cutbolt-local-v1 | 10.0% | Prepared the user-agreed PixelForge/Qwen YouTube pilot: agent handoff, draft local production contract, original six-scene brief and P1-P6/Y01-Y14 acceptance plan. Downloaded and digest-verified the pinned Qwen Base snapshot with bundled speech tokenizer outside Git, using an explicit setup helper. No inference, cross-product compatibility or new editing acceptance claimed; agent foundations remain 8/10 and the full baseline/denominator/exclusions are unchanged. |
| 2026-10-02 | cutbolt-local-v1 | 11.0% | v0.3 adds persisted Windows render jobs, phase/frame progress, queued/running cancellation, interruption detection and MCP stdio. Agent foundations increase 8/10 to 10/10 against explicit bounded criteria. Editing gains only E04 basic (queued exports with progress/cancellation), independently verified against reference frames/audio; E04 extended stays open for automatic retry and publication-crash reconciliation. Denominator, exclusions and weights unchanged. |
| 2026-10-02 | cutbolt-local-v1 | 14.0% | v0.4 adds bounded PNG animation scenes with position, integer scale, quarter-turn rotation/crop (V01 basic), straight-alpha layered opacity (V02 basic), and timeline frame/still previews (E03 basic). Exact range previews, explicit WAV conversion and the separately verified live PixelForge handoff support the five-second 1080p pilot; they earn no additional points. Broader transforms/audio/tracks/color/delivery and pipeline review/recovery remain open. Agent foundations stay 10/10. All 50 groups, checkpoint wording, weights and exclusions remain unchanged. |

Original project code is MIT licensed. Agent interface checks are tracked separately and do not inflate editing coverage. Research evidence does not establish an implemented compatibility adapter.

Pipeline preparation verification note: document/example checks, model-file integrity, repeat-download preservation, output-root rejection and the repository material check passed. Full `tools/verify.py` attempts stopped at formatting checks in concurrently edited Rust sources, before engine tests. The 10/100 and 8/10 figures above remain the prior verified baseline, not a fresh verification of the working tree; regenerate evidence through `tools/verify.py` once that work is ready. No generated progress file or acceptance evidence was hand-edited to bypass the failure.

Subsequent v0.3 verification passed all 69 engine, session, renderer and MCP/job checks plus formatting, lint and repository checks. The current verified counts are 11/100 editing and 10/10 agent foundations. This supersedes the earlier engine-verification limitation above; it does not claim PixelForge/Qwen inference or pipeline compatibility.

The v0.4 suite adds 13 scene/preview checks to the 69 existing checks, plus a separate passing live PixelForge handoff check. All pixels in the 125-frame 1080p fixture and the six supported rate/channel conversion cases are checked independently. Evidence and partial pipeline cases are recorded in [pipeline/RESULTS.md](pipeline/RESULTS.md).

Publication-boundary update, 2 October 2026: product-specific audit records and original provenance were preserved privately and excluded from Git. Public docs now use project-owned capability names. The baseline identifier is relabeled `cutbolt-local-v1` without changing its 50 groups, 100 checkpoint criteria, evidence assignments, weights or accepted scope. I03 remains an unimplemented native third-party project-compatibility target; its exact private target is unchanged. Editing remains 14/100 and bounded agent foundations 10/10. Added an indexed-content guard and local commit policy. All 12 commit-boundary tests and the full verification suite passed: 13 Rust, 19 render, 13 session, 24 MCP/job and 13 scene/preview checks, plus format, lint and repository checks. Generated progress and evidence were refreshed through the verifier.


## 3 October 2026 — Generated-shot research scope expanded

The user authorized additional external investigations of numbered-image ingestion/relinking, stepped timing, pixel-preserving shot layout and transparent/opaque delivery. Original research criteria and prior results were retained; all new checkpoints began untested. Product-specific plans, scores, fixtures and results remain private. The new [workflow requirements](IMAGE_SEQUENCE_WORKFLOWS.md) refine existing engine acceptance areas without changing their denominator or awarding implementation points. The pipeline contract now distinguishes a planned numbered-frame adapter from the implemented atlas route.

The expanded external investigations have now been executed on pinned builds. Their selected verification checks pass, including preservation of all 259 original fixture files and 19 tracked installation files. Findings strengthen the requirements for complete source identity, exact output endpoints, explicit alpha/color interpretation and separately verified export presets/resolutions. Failed defaults and unresolved older offline testing remain in the private records. This research awards no engine implementation points, changes no engine acceptance criteria or weights, and does not replace verification of concurrent engine development.
