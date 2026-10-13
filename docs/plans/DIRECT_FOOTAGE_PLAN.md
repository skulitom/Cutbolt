# Fix plan: edit real footage directly, with effects as clip properties

Handoff plan, 11 October 2026. Written for an implementing agent. It changes no scope, criteria or points by itself. Line references are to `main` at `549e319`; re-check them before editing.

## The problem

Agents cannot edit a phone or camera file directly.

- The timeline renders only FFV1 `bgr0`/`bgra` video with 48 kHz stereo PCM16 audio, at the project's exact size and rate. Every other file is converted first with `media.prepare` or `media.conform`.
  - A 1.4 MB H.264 clip becomes a 14 MB FFV1 asset. A 20-minute 1080p recording becomes tens of gigabytes.
  - Conversion caps output at 45,000 frames.
  - The conform matrix accepts only 8-bit H.264 in MP4/MOV. HEVC, ProRes, 10-bit and 4:2:2 fail with `INVALID_COLOR` (`color.rs:137-153`, `conform.rs:279-306`), even though `media.inspect`/`media.prepare` propose recipes for them (`readiness.rs:708-786`).
- Effects are not clip properties.
  - Grade, selective grade, key, mask, spatial transform and blend exist only as scene layer properties (`scene.rs:227-268`).
  - Scene layers can only show PNG frames, graphics or tilemaps; video is never a scene layer.
  - So an agent must extract frames or compile a scene into a new asset, then place that asset.
  - The only clip-level picture property is `OverlayTransform` (crop, divisor shrink, opacity, position), and only on `alpha_over` clips (`tracks.rs:60-119`, `670-680`).
- The picture path is 8-bit throughout, compositing happens in encoded values, and only grades work in linear light (`track_composite.rs:27-50`, `412-417`; `effects.rs:499-534`).

The target is the draft [1.0 roadmap](../ROADMAP_1_0.md), phases 0.6 and 0.7: checkpoints M05, V09 and C06, basic level first. The v2 baseline is **not adopted**, so this work earns no points. Record it in `docs/PROGRESS_HISTORY.md` and never inflate the v1 tracker.

## Rules that constrain the design

These come from `AGENTS.md` and existing practice. Do not trade them away for speed.

- Time stays exact and rational. Unsupported semantics are rejected explicitly, with field, value and bound named (see commit `1084f5a`).
- Sources are never modified, and outputs are never overwritten.
- Every claim needs an independent reference. Faster paths keep the old path as a regression oracle and must match it bit for bit, or within a declared, tested tolerance.
- FFmpeg stays an external process. No linked libraries, vendored code or bundled binaries.
- Saved sessions, diffs, undo, retries and receipts cover every new property.
- No listening service, no network at runtime, no telemetry.

## Current pipeline in one page

- **Source gate.** `render::inspect_source_with` (`render.rs:584-753`) is the single gate for renders, previews and exports. Its main profile test (`render.rs:640-655`) requires:
  - exactly two streams, FFV1 `bgr0`/`bgra` and PCM s16 48 kHz stereo;
  - the project's size and an exact rate;
  - no side data;
  - zero-origin, exactly periodic timestamps (`render/clock.rs:59-97`);
  - audio continuity within 1 ms and exactly equal audio/video lengths.

  Error codes are listed at `render.rs:631-732`.
- **FFV1-only assumptions.**
  - Packets equal frames (`Timing::Packets`, `media.rs:681-682`).
  - Every frame is a keyframe, which the seek exactness argument relies on (`render/clock.rs:110-121`).
  - The PCM sample count comes from packet bytes (`media.rs:727-748`); AAC fails at `render.rs:709`.
  - Negative or offset timestamps are rejected (`render.rs:397-402`).
- **Decode.**
  - Sequential timelines use one `-i` per clip with `trim`/`concat`, never sought (`render.rs:1001-1017`).
  - Placed tracks build an FFmpeg base graph that emits planar `gbrp` (`track_render.rs:1486-1526`), with exact `-ss` seeking from `b2b88a8` (`track_render.rs:398-446`, `render/clock.rs:99-140`).
  - Top-level overlays and transitions are separate decoders, composited in Rust (`track_composite.rs`; `Ahead` 99-140, `Decoders` 142-248, `Effect` 551-661, blend 412-417).
  - Nested overlays and transitions still run as FFmpeg `geq`/`blend` graphs (`track_render.rs:575-591`, `714-731`).
- **Conform.** `conform.rs:983-1348` decodes the needed window to native YUV and converts it in Rust (`color.rs:212-268`; f64 math, nearest 2x2 chroma). It writes FFV1 `bgr0` + PCM16 and verifies by decoded digests. There is no conversion cache, and `Asset` keeps no provenance (`model.rs:9-27`).
- **Inspection cache.** Results are keyed by file sha256, parameters, the ffprobe build and the engine producer (`inspection_cache.rs:162-179`), but the file is still hashed on every use.
- **Effects code that can run on video frames.**
  - `effects::prepare` and `Processor::pixel` (`effects.rs:199-208`, `535-757`) are pure per-pixel functions.
  - `spatial::draw` (`spatial.rs:450-461`), and `composite::normal_row` / `masked_channel` (`composite.rs:56-188`).

  Three couplings block reuse:
  - `Prepared` keyed by PNG path (`scene.rs:401-425`);
  - whole-scene parameter precomputation (`sample_parameters`, `scene.rs:1240+`, bounded by 120 s);
  - `compose_layers` (`scene.rs:1535+`).

## Strategy

Ship this as six stages. Each stage is useful alone, keeps every existing fixture passing, and has its own oracle. Stages 1–2 remove the conversion step. Stages 3–5 make effects clip properties. Stage 6 adds precision.

| Stage | Agent-visible result | Oracle |
| --- | --- | --- |
| 0 | Nothing (baselines and harness) | — |
| 1 | `media.add` takes an MP4/MOV directly; previews and exports read it without conversion | Bit-exact equality with the existing `media.conform` `sdr` output of the same source |
| 2 | Files that cannot be decoded directly (VFR, rotation, HEVC 10-bit, ProRes…) are converted automatically, cached and reported | Equality with today's explicit conform; cache-hit and invalidation tests |
| 3 | Pixel effects (grade, selective grade, LUT, key) on any video clip | Equality with the scene-compiled result of the same frames |
| 4 | Spatial and transparency effects (transform, opacity, mask, blend) on clips, compositing over lower tracks | Equality with the scene-compiled result; independent integer reference for layering |
| 5 | Scenes and graphics as generated clips, with no compile step | Equality with `scene.render` output |
| 6 | Optional 16-bit / linear-light working space with dithered output | Tolerance against an f64 reference; v1 fixtures unchanged at the default |

Do the stages in order. Stages 3 and 4 depend on 1, because effects on a directly decoded clip are the point.

## Stage 0: preparation (about one session)

1. Work in your own worktree on a `claude/<name>` branch. Ship with `python -X utf8 tools/ship.py --to main` (see `docs/DEVELOPMENT_LOOP.md`).
   - The Linux port of jobs, image-directory publication and speech landed on `main` as `1ea6447`, so start from it. It left `src/media.rs` untouched; the Linux process handling lives in `src/jobs.rs`.
   - Linux is now a verification platform. A WSL Ubuntu setup is described in the 11 October entry of `docs/PROGRESS_HISTORY.md` and in `docs/DEPENDENCIES.md`. New decode paths must work there too: use `cfg(target_os = "linux")` beside `cfg(windows)`, never Windows-only code.
2. Rebuild the impact map: `python tools/impact.py`. It was built at `bd84caf`, 85 commits ago, and has no entries for `track_composite.rs`, `readiness.rs`, `delivery/stream.rs` or `inspection_cache.rs`. Without it, verification selects the wrong fixtures.
3. Save release binaries of the base commit for A/B work (`cargo build --release`, copied outside the repository). Follow the load-aware protocol in the `benchmark-under-load` memory: alternate before and after runs, record the busy fraction, and never compile during a baseline.
4. Write original fixture media generators, extending `tests/support` or `tools/scene_fixtures.py`. Use FFmpeg encoders over original synthetic patterns, never downloaded clips:
   - H.264 High, CFR 25/30/60 and 30000/1001, GOP 60 with B-frames, AAC LC 48 kHz stereo and 44.1 kHz mono;
   - H.264 with an edit list (non-zero start, AAC priming);
   - HEVC Main and Main10, ProRes 422 HQ (`prores_ks`), and a VFR screen-recording-style file;
   - a phone-style file with a `rotate` display matrix, and one with non-square SAR;
   - a 20-minute 1080p30 H.264 file for long-source tests, generated on demand into the external data root, never committed.

   Record the encoder settings, and record any new tool versions in `docs/DEPENDENCIES.md`.
5. Fix a bug the investigation found (not yet run): the single-clip track preview uses a one-frame duration of `Time::new(1, 25)` (`preview.rs:124` and `133`). At 24, 30 or 60 fps it is not frame-aligned and can fail with `UNALIGNED_TIME`. Add a native-rate single-clip preview case to `tests/native_timing.py`.

## Stage 1: direct decode of common compressed sources

Goal: an asset can point at the original H.264 or HEVC 8-bit 4:2:0 CFR file. Track renders, previews and exports decode it directly, with exactly the pixels and samples `media.conform` would have produced.

### 1.1 Source index

Add a one-time, cached index of a compressed source, beside `inspection_cache.rs`. Keep it in the same cache directory and under the same key rules, plus the index schema version.

- Video: per presented frame, in presentation order, record the best-effort timestamp, the keyframe flag and the picture type. Get it from one `ffprobe -show_frames -select_streams v:0` pass, reading entries incrementally. Record the decoder's native `pix_fmt`, color tags, `field_order`, SAR and any display matrix.
- Audio: the decoded sample count and start padding, taken by decoding once to PCM (`-f s16le` to a counter, never to disk). Packets do not give AAC sample counts.
  - **Approved amendment, 11 October 2026:** the user allows a bounded, content-verified audio cache when codec seeking cannot reproduce full-decoded samples. The fallback may store PCM, preserves source files and direct video, and must retain full content verification before publication. See [DIRECT_SOURCES.md](../DIRECT_SOURCES.md) for the measured AAC counterexample and implementation status.
- Store the index compressed in the cache directory, keyed by content identity. A 20-minute 60 fps source is 72,000 entries. Never put the index in the project.
- Return a summary from `media.inspect`: `direct: true|false` with reasons, frame count, rate, GOP length and `audio_policy` needs.

### 1.2 What qualifies for direct decode

Accept in this stage:
- H.264 and HEVC Main, 8-bit 4:2:0;
- MP4, MOV or MKV;
- progressive, square pixels, no display rotation;
- frame times equal `first + n × period` exactly at container precision, at one of the eight native rates (`readiness.rs:8-17`);
- audio AAC, PCM, MP3, FLAC or Opus at any rate, mono or stereo.

Everything else returns `direct: false` with named reasons, and goes to stage 2's automatic conversion.

The source's size and rate must equal the project's in this stage, just as today. Mixed sizes and rates come with T11 later; reject them explicitly with the existing errors.

### 1.3 Gate

Extend `inspect_source_with` with a second accepted profile, `Indexed`, beside the existing FFV1 profile. Do not loosen the FFV1 path.

- The `Indexed` profile uses the index from 1.1 instead of `Timing::Packets`.
- It maps timeline frame `n` to presented frame `n`, so a non-zero first timestamp (edit lists) is handled by the index, not rejected.
- Audio policy is explicit:
  - audio shorter than the video pads with silence;
  - audio longer than the video is trimmed to it;
  - other rates are resampled to 48 kHz stereo with the same resampler `media.conform` uses (linear, `conform.rs`), so the results are equal;
  - mono is duplicated to both channels.

  Report each choice in the inspection result. Today's rule that lengths must match exactly stays for FFV1 sources.

### 1.4 Decoding exact frames

- Seek with the existing `clock::seek_input` arithmetic. FFmpeg's accurate seek decodes from the previous keyframe and discards earlier frames, so the frame-exact argument now rests on the index's timestamps rather than on "every frame is a keyframe". Update the comment at `render/clock.rs:110-121` to say so.
- Open-GOP HEVC (RASL) frames after a seek can be undecodable. Detect this with the index's picture types. Seek one GOP earlier for any target that needs a previous GOP, and test it.
- Decode to the **native** YUV format (`-f rawvideo -pix_fmt yuv420p`), never `gbrp`. Convert to RGB in Rust with `color.rs`, the same code `media.conform`'s `sdr` path uses, so the output is bit-identical to conform. That equality is the whole oracle for this stage.
  - So a directly decoded clip cannot go through the FFmpeg base graph's `format=gbrp`.
  - Treat it like today's top-level overlay clips: its own decoder (`track_render.rs:1197-1310` `layers()` pattern), composited in Rust (`track_composite.rs`).
  - The base graph keeps the FFV1 clips.
- Performance: `color.rs` converts with f64 per pixel.
  - Add an exact table-driven fast path. For 8-bit 4:2:0 one output pixel depends on (Y, U, V), so a 2^24-entry table of RGB triples (48 MB, built once per matrix/range/transfer declaration and cached in memory) is exact.
  - Prove it with an exhaustive unit test against the f64 path over all 2^24 inputs for each supported declaration.
  - Keep the f64 path as the reference.
- Hardware decode (`acceleration.rs`) may later feed this path. H.264/HEVC decoding is bit-exact by specification, so the oracle is equality with CPU decode. It is not required for this stage.

### 1.5 Model and edit commands

- `media.add` (an `Operation` in `model.rs`) accepts a direct source. Record the original file's identity, as for any asset. Do not invent a separate "prepared" asset.
- Add an optional `decode` field on `Asset`: `{ "mode": "direct", "index": "<index key>" }`.
  - It is filled by `media.add`.
  - It is checked against the file at every use, like `identity`.
  - With `deny_unknown_fields`, older engines reject projects that contain it. That is acceptable, and is the explicit failure the rules want.
  - Record in progress history whether this bumps `Project.schema_version` (`model.rs:375-381`). Recommendation: no bump for an additive optional field; bump to 2 when stage 3 adds clip effects, with a migration that rewrites nothing in v1 projects.
- `timeline.check` reports direct clips whose index is missing or stale.
- `media.prepare` keeps working unchanged. Its result should now say when a file needs no preparation (`ready: true, direct: true`).

### 1.6 Identity cost

Every use hashes the whole file (see the inspection cache notes). For a 4 GB phone video that is several seconds per preview.

- Propose a metadata-gated identity cache: if path, size, modification time and the OS file ID are unchanged since a verified hash in this process or cache, reuse the hash.
- Keep a full re-hash before an export publishes, and wherever a receipt claims a hash.
- **This weakens an existing safety check. Ask the user before implementing it,** and record the decision in progress history either way.

### 1.7 Tests (new fixture `direct_sources`, registered in `tools/verify.py` STAGES)

- For each stage-0 file that qualifies:
  - `media.conform` it explicitly (today's path) and render a placed-track timeline from the FFV1 result;
  - render the same timeline from the original;
  - require identical decoded frame digests and PCM digests.
  - Cover: single frame previews at the first, a keyframe, a B-frame and the last frame; a range export; an H.264 delivery export; a cut that starts mid-GOP; slip and roll edits across GOP boundaries.
- An edit-list file: the first presented frame and the first audio sample line up with conform.
- Every non-qualifying file: an explicit `direct: false` reason, never a silent fallback.
- Long source: the 20-minute file previews a frame near its end in under a declared time, measured with the load protocol, and exports a 10 s range without decoding from the start.
- Source preservation: input hashes are unchanged after every case.

**Exit:** a phone-style H.264 file goes from `media.add` to an H.264 export with no conversion job, and the output is bit-identical to the conform path. Run `tests/native_timing.py`, `tests/tracks.py`, `tests/overlays.py`, `tests/transitions.py`, `tests/delivery.py`, `tests/cache_previews.py` and `tests/integration.py` unchanged.

## Stage 2: automatic, cached conversion for everything else

Implementation note, 12 October 2026: explicit SDR conform now covers the listed 10-bit,
4:2:2 and straight-alpha formats, plus exact quarter-turn display rotation and fixed sample
aspect with declared contain/cover/stretch fitting. See [CONFORM](../CONFORM.md#explicit-display-geometry)
for behavior and independent geometry checks. This broadens explicit conversion only;
the conversion cache, automatic preparation phases, provenance and stage exit remain pending.
A 13 October checkpoint adds a hard encoded-output ceiling to explicit conform, plus verified
decoded digests for future cache entries. This is a prerequisite, not automatic cached conversion.

Goal: an agent never has to call `media.prepare` by hand. Files that cannot be decoded directly are converted once, cached and reused, and the conversion is visible.

1. Broaden the conform matrix:
   - HEVC Main10 and H.264 High10 (10-bit 4:2:0);
   - 4:2:2 (ProRes 422, H.264 4:2:2);
   - ProRes 4444 with alpha onto `bgra` (this also covers HyperFrames-style overlays; see strategy item B5);
   - display rotation, applied as an exact quarter-turn transform;
   - non-square SAR, through an explicit fit policy.

   Each needs an independent numerical reference in the style of `tests/color.py` (Decimal math). VFR stays on the existing conform mapping (`conform.rs:608-679`).
2. Add a conversion cache. It is content-addressed by:
   - source sha256 and bytes;
   - the recipe digest;
   - the FFmpeg build identity and the engine producer.

   Store it beside the inspection cache, with `cache.inspect` reporting and `cache.prune` LRU eviction (extend `cache.rs` `Task`). Never overwrite an entry. Verify an entry by its recorded decoded digests before use.
3. `media.add` of a non-direct file records `decode: { "mode": "converted", "recipe": … }`. Renders, previews and exports find the cached conversion. When it is missing:
   - a job (`export.run` and others) adds a `preparing sources` phase and converts first, with progress;
   - a synchronous preview returns a new `PREPARATION_REQUIRED` error, naming the asset, with a ready-to-run `job.start media.prepare` request in `detail`.
4. Add provenance to `Asset`: the original identity, plus the conversion identity when there is one. `media.prepare` keeps its explicit behavior for agents that want to manage files themselves.
5. Surface size: the inspection reports the expected cache bytes. Refuse beyond a configurable cap with a clear error, never a silent fill of the disk.

**Exit:** one screen recording (VFR), one rotated phone clip, one HEVC 10-bit clip and one ProRes clip all reach an H.264 export from `media.add` without the agent calling `media.prepare`. Each output equals today's explicit conform of the same file. A second export reuses the cache, which a counted-launch shim proves (as `tests/overlays.py` counts ffprobe launches).

## Stage 3: pixel effects on any video clip

Goal: grade, selective grade, LUT and chroma key are properties of any video clip on a native track, rendered in the same pass.

1. **Model.** Add `TrackClip.effects: Vec<ClipEffect>` (`tracks.rs:26-63`), with a serde default of empty, at most 8 entries. `ClipEffect` is a tagged enum that reuses the existing types verbatim, so schema descriptions and validation are shared:
   - `grade` (`effects::Grade`), `selective_grade`, `chroma_key`;
   - `lut` (identity of a `.cube` file, plus interpolation, from `lut.rs`).

   **Effect key clock:** curve key times are on the clip's **source clock**, exactly as `gain_curve` already documents (`tracks.rs:44-55`), so trims and slips keep effects aligned with the content. Write that sentence into the field description.
2. **Edit.** Add `tracks.edit` op `clip_effects`, taking `{clip_id, effects}` and replacing the whole stack. That is simple, diffable and undoable.
   - Add placement and receipt fields beside `levels` and `transform` (`store.rs:24-48`), so session diffs show effect changes.
   - Reject effects on audio clips, and inside nested sequences in this stage (`UNSUPPORTED_TIMELINE`, naming the clip).
3. **Render.**
   - A clip with effects is decoded like an overlay layer: its own decoder.
   - In the Rust compositor, each frame runs `effects::Processor` (`effects.rs:535-757`) before compositing.
   - Pixel effects keep full opacity, so nothing about which track shows changes in this stage.
   - Split `effects::prepare`'s whole-duration sampling into per-frame evaluation, so a 30-minute clip needs no precomputed table. Keep the scene path calling the old entry point until stage 5.
4. **Oracle.** For every effect type, take a short clip:
   - (a) render the timeline with the clip effect;
   - (b) extract the same source frames as PNGs, then build a scene whose single layer uses those frames (`Layer.frames`, `scene.rs:239`) and the same `effects`, and run `scene.render`;
   - (c) require identical decoded frame digests.

   Add animated-key cases and a trimmed or slipped clip to prove the source-clock rule.
5. Previews, proxies, range exports and queued exports all use the same path. Add one case each.

**Exit:** a grade plus LUT on a directly decoded phone clip exports without any compile step and matches the scene oracle exactly.

## Stage 4: spatial and transparency effects, and real layering

Goal: transform, opacity, mask, blend mode and chroma key transparency are clip properties. A clip that becomes partly transparent shows the tracks below it.

1. Add `transform` (scene `Transform` plus `spatial::Transform` with `animation`), `opacity`, `mask` (`RectMask` with feather) and `blend_mode` to `ClipEffect`.
   - The existing `OverlayTransform` (`tracks.rs:64-119`) stays valid.
   - Define it as sugar for the equivalent `transform` + `opacity` effects, and test that the two forms are equal.
2. Compositing rule, written into `docs/TRACKS.md`. A video clip on any track composites over the result of all lower active tracks at that frame:
   - normal blending, unless `blend_mode` says otherwise;
   - black below the lowest track;
   - a clip with no effects and full coverage is opaque, so lower tracks need not be decoded. Keep that fast path: today's "highest opaque track wins" is its special case.
3. Generalize the compositor (`track_composite.rs`) from "base plus top-level overlays" to an ordered stack per frame. Decode lower tracks only when an upper clip is not fully opaque at that frame; decide this from the recipe, as `Layer::unchanging` does for scenes.
4. Nested sequences: replace the FFmpeg `geq`/`blend` nested overlay and transition graphs (`track_render.rs:575-591`, `714-731`) with the same Rust path. Keep the old graphs as A/B oracles until the fixtures agree, then delete them.
5. **Oracle.**
   - The scene oracle from stage 3, extended to several layers.
   - An independent integer reference in Python for the layering rule itself, in the style of `tests/overlays.py`.

**Exit:** picture-in-picture, a keyed presenter over B-roll and a feathered mask reveal are built only with `tracks.edit` on directly decoded clips. They match the oracles, and the T08 / V01 / V02 / V05 fixtures still pass.

## Stage 5: generated clips

Goal: a scene (titles, captions, graphics) is a clip source, rendered per frame inside the timeline pass with no `scene.render` step.

- Add `TrackClip.scene: Option<SceneRef>`, holding an inline scene or a document file in a workspace, as the third exclusive source beside `asset_id` and `sequence_id`.
- Stream frames from the scene renderer into the compositor. It already streams frames to an encoder.
- This removes the compile step from caption overlays (`captions.render`) and production titles.
- Scenes keep their own caps (`MAX_SECONDS`, layers, composited-pixel budget).
- **Oracle:** the frames equal `scene.render` of the same recipe placed as an asset.

## Stage 6: working precision (C06)

- Add a project setting: `working_space: { "bits": 8 | 16, "light": "encoded" | "linear" }`.
  - The default, 8-bit encoded, is today's behavior and must leave every v1 fixture bit-identical.
  - 16-bit linear composites in u16 linear light and converts once at output, with an explicit ordered dither for 8-bit delivery.
- **Reference:** an f64 Python model per operation, with a declared tolerance, plus a banding test (a slow gradient through a grade and an opacity fade) that must show no new steps.
- HDR timeline delivery (C07) builds on this later and is out of scope here.

## Verification and shipping, every stage

- While working, run `cargo test` and `python -X utf8 tools/verify.py --only <touched fixtures>`. Commit, then `python -X utf8 tools/ship.py --to main`, and fix forward whatever `tools/verify.py --status` reports.
- Run the full quick `tools/verify.py` at the end of each stage. Run `--thorough` only for a milestone claim.
- Use the load-aware A/B protocol for any speed claim. Report ranges, medians and the busy fraction.
- Line endings: the repository has `* -text` and mixed CRLF/LF files.
  - Edit through byte-preserving scripts or the Edit tool.
  - Restore each file's endings after `cargo fmt` (check with `git ls-files --eol`).
  - Never use `sed -i` on tracked files.
- Docs: for each stage, update the feature docs (`TRACKS.md`, `CONFORM.md`, `SCENES.md`, `USAGE.md`, `AGENT_INTERFACE.md`), `cutbolt capabilities` and the MCP descriptions in `src/mcp.rs`. Fix drift as you go. Agents trust docs literally.
- The MCP catalog has a size budget (unit tests in `src/mcp.rs`). New schema fields must have descriptions, and must fit. Trim in `schema::listing()` if needed.
- Add a progress-history entry per stage that says what changed, what was measured and what is still bounded. No points: v2 is not adopted.

## Open decisions for the user

| Decision | Recommendation |
| --- | --- |
| Metadata-gated identity reuse (1.6) | Approved by the user on 11 October 2026, with a full re-hash before publication. See DIRECT_SOURCES.md for implementation and evidence. |
| Conversion cache size cap and default location | Workspace `.cutbolt/cache/conform`, or `CUTBOLT_CONFORM_CACHE`; default cap 50 GB, with an explicit error beyond it. |
| `Project.schema_version` bump | Not for stage 1's optional `decode` field; bump to 2 at stage 3, with migration. |
| Effect key clock | Source clock, matching `gain_curve`. |
| Linear-light default after stage 6 | Keep 8-bit encoded as the default until the benchmark shows a quality win. |

## Risks

- **Open-GOP and B-frame seeking.** The oracle (equality with sequential full decode) catches errors only for the cases the fixtures cover. Generate GOP structures deliberately: closed and open GOPs, B-pyramids, and long GOPs of 250 frames.
- **Compositor throughput.** Moving clips from the FFmpeg base graph to Rust decoders costs CPU. Measure stage 1 on the 20-minute file before and after. If it regresses badly, keep FFV1 clips on the FFmpeg base and only direct clips in Rust.
- **Index build time on long files.** Show progress, run it as a queued phase, and cache it.
- **Disk use of stage-2 conversions.** Report expected bytes up front, and enforce the cap.
