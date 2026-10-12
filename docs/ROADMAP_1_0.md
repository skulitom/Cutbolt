# Cutbolt 1.0 roadmap

**Draft, 4 October 2026. This is a proposal for review. It adopts no baseline, awards no points and changes no acceptance criteria.** The [core tracker](PROGRESS.md) and [combined tracker](IMPLEMENTATION_PROGRESS.md) remain authoritative for 0.4. Record adoption of any part of this plan in [progress history](PROGRESS_HISTORY.md).

## What 1.0 means

On 4 October 2026 the user defined 1.0 as features and quality high enough that agents prefer Cutbolt over any other available video software: the fastest and most efficient option, with the best output quality.

This is a comparative claim. As the [competitive analysis](COMPETITIVE_ANALYSIS.md) requires, Cutbolt may make it only where measured results show it. Each claim is scoped to the exact alternatives, versions, agent models and hardware tested, and re-measured against the alternatives available at release.

1.0 is far enough away that no single headline demonstration is fixed in advance. Instead, a [comparative benchmark](#comparative-benchmark) that grows with the engine becomes the release gate. The PixelForge/Qwen explainer pilot and [Inkbolt](#sister-tool-inkbolt) integration join the suite as tasks when they are ready.

Platforms: Windows and Linux (user decision, 4 October 2026). macOS follows 1.0.

## Starting point

0.4 verifies 100/100 core and 108/108 total checkpoints with 466 checks. Each capability passes inside explicit bounds. Many advanced operations work by compiling a new editing asset that the narrow reference timeline then consumes. The v1 checklist is saturated, so it can no longer guide the work. The documented bounds below are where 1.0 starts.

| Area | 0.4 bound | Reference |
| --- | --- | --- |
| Timeline video | FFV1/bgr0 sources with matching dimensions; 8-bit SDR in encoded sRGB/BT.709 | [TRACKS](TRACKS.md), [COLOR](COLOR.md) |
| Frame rates | Sequential timelines support eight native rates. Placed tracks, proxies, nested sequences, scenes and H.264 delivery stay at 25 fps. Only 25 fps non-drop timecode is supported. | [NATIVE_TIMING](NATIVE_TIMING.md), [SYNCHRONIZATION](SYNCHRONIZATION.md) |
| Effects | Applied by compiling scenes into new assets. Scenes are at most 250 frames (10 s) at 25 fps, with 16 layers and 8 effects per layer. General video-track effects remain open. | [SCENES](SCENES.md), [GRADING](GRADING.md) |
| Render size | The model holds 32 tracks and 1,000 clips; renders take 1–64 clips. Ranges are up to 180,000 frames. | [TRACKS](TRACKS.md), [EXPORT](EXPORT.md) |
| Audio | 48 kHz stereo PCM16 timeline. Surround exists only as separately routed WAV outputs, without surround loudness. True-peak metering is open. | [AUDIO_ROUTING](AUDIO_ROUTING.md), [AUDIO](AUDIO.md) |
| Color | High bit depth and HDR are conversion paths into 8-bit SDR editing assets. Transitions and compositing are not linear-light. | [HDR](HDR.md), [TRANSITIONS](TRANSITIONS.md) |
| Ingest | A bounded conversion matrix accepts one video and one audio stream (mono/stereo, 24–48 kHz) and outputs 25 fps assets. Rotation, interlacing, non-square pixels, HDR and VFR sources fail explicitly or must be converted first. | [CONFORM](CONFORM.md) |
| Graphics | Rectangles, ellipses and text. Paths, gradients and shape antialiasing are open. | [GRAPHICS](GRAPHICS.md) |
| Delivery | One H.264/AAC profile family at 25 fps with explicit controls; lossless FFV1 and PNG profiles. Captions are not embedded in containers. | [DELIVERY_CONTROLS](DELIVERY_CONTROLS.md), [CAPTIONS](CAPTIONS.md) |
| Retiming | Optical flow, pitch-preserving stretch and reverse audio remain open | [REMAPPING](REMAPPING.md) |
| Speech | Optional local recognition for English and Greek, through a worker running under WSL with CUDA | [TRANSCRIPTS](TRANSCRIPTS.md) |
| Speed | CPU reference path through external FFmpeg processes. Optional CUDA decoding showed no useful overall speedup. The 30-minute 4K gate does not imply real-time playback. | [ACCELERATION](ACCELERATION.md), [LONG_FORM](LONG_FORM.md) |
| Jobs | One render and up to 32 queued jobs per root. Only reference renders queue; conversion, compilation and export block the caller. No mid-stream resume. | [AGENT_INTERFACE](AGENT_INTERFACE.md), [RENDER_RECOVERY](RENDER_RECOVERY.md) |
| Agent interface | 65 MCP tools, called serially, with no progress notifications. Listings are documented field by field, and unit tests hold the catalog to 240 KiB. Errors are `{code, message}` without a retry flag or a field path for type errors. There is no versioning or deprecation policy. | [AGENT_INTERFACE](AGENT_INTERFACE.md) |
| Platform | Tested on Windows only, with no CI. The job queue, recording, numbered-image publication and transcription are Windows-only; other platforms have stubs. | [DEPENDENCIES](DEPENDENCIES.md), [RECORDING](RECORDING.md) |
| Distribution | Built from source (`publish = false`). FFmpeg comes from PATH or environment variables; the tested build is GPL-3. There is no install or `doctor` command. | [DEPENDENCIES](DEPENDENCIES.md), [USAGE](USAGE.md) |

## Principles that carry into 1.0 unchanged

- Exact rational time, and explicit rejection of unsupported semantics.
- Saved sessions, idempotent retries, revision conflicts, undo and history.
- Explicit input and output roots. Sources are never modified and outputs are never overwritten.
- Local only: library, CLI and MCP over stdio. No listening service, hosted API, account or telemetry.
- Independent references for every acceptance claim. Faster paths keep the CPU reference path and declared tolerances.
- Original implementation within the [research boundary](RESEARCH.md). Dependencies stay external, with recorded versions and licenses.
- The accepted exclusions stand: accounts/activation, cloud services, stock marketplaces, telemetry and GUI-only workflows.

## Release gates

1.0 ships when a single build passes all five gates.

| Gate | Requirement |
| --- | --- |
| G1 Editing depth | The [draft `cutbolt-local-v2` baseline](#draft-v2-acceptance-criteria) is adopted and fully verified, including all carried-forward v1 and supplemental checkpoints. |
| G2 Comparative benchmark | On the [benchmark](#comparative-benchmark), Cutbolt has the highest task success rate overall. It is fastest, uses the fewest agent resources and produces the best measured output quality, as defined below. Any category where an alternative is better is reported, not hidden. |
| G3 Platforms | The full verifier passes on recorded Windows and Linux environments. A platform-specific suite is replaced by its equivalent on the other platform, never skipped. |
| G4 Stable contract | Project and command schemas are frozen at a declared version. Error codes are stable and documented. Errors carry a field path and a retry flag. A written semantic-versioning and deprecation policy exists, and every 0.x store migrates. |
| G5 Installable | Release builds exist for both platforms. A `doctor` command checks external tool versions, builds and required features. Core editing needs no repository checkout or Python; optional workers declare their own runtimes. |

Public distribution is a separate [open decision](#open-decisions). The gates make 1.0 releasable; they do not decide whether or how it is published.

## Comparative benchmark

The benchmark replaces a fixed demo as the evidence for 1.0. It starts in 0.5 with the roughly 25 tasks proposed in the competitive analysis and grows each phase. Tasks are added when a capability is ready, and removed only with a recorded reason.

### Task categories

1. **Footage editing:** remove pauses, trim, reorder, insert B-roll and cut between cameras in existing recordings.
2. **Speech and captions:** transcribe, correct words, cut by text, and produce burned-in and sidecar captions.
3. **Audio:** normalize loudness, reduce noise, duck music under speech and repair levels.
4. **Graphics and motion:** titles, lower thirds and animated scenes, including the explainer pilot and, later, Inkbolt graphics.
5. **Color:** normalize mixed sources, apply LUTs and match shots.
6. **Delivery:** produce format and aspect variants (16:9, 9:16, 1:1) with reframing and platform presets.
7. **Revision:** replace one scene or narration take and rebuild only what changed.
8. **Recovery:** interrupted renders, lost responses followed by retries, conflicting writers and missing media.

### Measures

| Axis | Measured as |
| --- | --- |
| Success | Completed without manual repair. Mechanical checks pass: frame-exact cuts, A/V sync, source preservation, no unintended changes. |
| Speed | Time to first useful preview, total task time and render throughput, with rendering reported separately from agent reasoning. |
| Efficiency | Agent tool calls, input/output tokens, retries and failed calls per successful task, plus peak memory and CPU/GPU time. |
| Quality | Objective scores against a lossless master at matched codec and bitrate, color error on reference charts, loudness compliance, caption timing error and frame/sample timing error. |

Lossy encoders are external, so quality gains must come from the pipeline itself: color handling, scaling, resampling, avoiding repeated encoding and encoder settings. Compare at matched settings; never claim byte-identical lossy output.

### Protocol

- Use the same agent model(s), instructions, hardware and inputs for every system. Record exact versions.
- Use only original generated or self-recorded media.
- Run each task at least five times and report variance. A tie is reported as a tie.
- Keep raw runs in the external data root and summaries in this repository.
- Run alternatives only through their public interfaces. Any proprietary-application inspection follows [RESEARCH.md](RESEARCH.md).

### Alternatives

Start with the systems named in the competitive analysis: mcp-video, direct FFmpeg/MoviePy scripting by the agent, an agent-capable desktop editor such as OpenReel or WeftCut, and Remotion for motion-graphics tasks. Review the field at every phase; "any other available software" means whatever is available when each run is made.

## Phases

Each phase ends with a minor version, a Git tag and a progress-history entry. Phases are ordered by dependency. V11, V12 and A08 can land in any phase after 0.6.

### 0.5 Measure and port

Goal: measure where Cutbolt stands, and stop adding Windows-only code before the large architecture change.

- Build the benchmark harness and run 0.4 against the alternatives. The gaps it finds set the priorities within later phases.
- Port to Linux. Replace the Windows-only pieces:
  - the job queue (`CreateProcessW` and Job Objects in `src/jobs.rs`), with process groups and parent-death handling;
  - WASAPI recording;
  - numbered-image directory publication;
  - transcription worker launch.
- Make hardware decoding work on both platforms, and add a CI matrix that runs the verifier on Windows and Linux.
- Fix contradictions between feature documents and the code or trackers, so the benchmark and the v2 review start from accurate limits.
- Measure preview latency, render throughput and memory, to set the numeric targets in P04–P07.
- Review and adopt the v2 baseline with those targets filled in.

Exit: the full verifier passes on Linux; benchmark v0 results are recorded; v2 is adopted.

### 0.6 One timeline

Goal: one render graph evaluates the whole timeline, and effects are clip properties instead of a compile step.

- Clip effect stacks on native tracks: transforms, opacity and blending, masks, grades, selective color, keys, LUTs, stabilization, reframing and retiming. Keyframes and expressions apply to all of them.
- Scenes become a generated clip type. The 10-second, 16-layer and 25 fps scene bounds and the 64-clip render bound are removed. This also unblocks the explainer pilot's 12-second storyboard slots.
- Native rates on placed tracks, proxies, transitions, nested sequences and delivery.
- Mixed source sizes and rates, with explicit per-clip fit and rate-conversion policies.
- A working space of at least 16 bits, with optional linear-light compositing and dithered 8-bit output.
- Project schema v2, with migration of v1 stores through the existing migration path.

Exit: every 0.4 fixture renders identically, or within its declared tolerance, through the new graph. The compile-to-asset path stays as a regression reference. T11, T12, V09, V10 and C06 basic checkpoints pass.

### 0.7 Real media in and out

- Edit common phone, camera and screen-recording files directly, with conversion automatic, cached and inspectable (M05, M06).
- HDR timeline and delivery (C07), and surround through the timeline and delivery (A07).
- A delivery codec matrix and platform-ready presets with loudness targets and embedded captions (E07, E08).

Exit: benchmark footage tasks use their original files with no manual conversion requests.

### 0.8 Fast iteration

- Preview latency targets (P04).
- Re-render only the segments an edit affected, and resume interrupted renders from completed segments (P05).
- GPU compositing within declared tolerance of the CPU reference, on both platforms (P06).
- Long timelines with many clips (P07).
- Queue every long-running operation (conversion, export, analysis), not only reference renders, with bounded concurrency per root and MCP progress notifications.

Exit: benchmark speed results meet or beat every alternative, or the remaining gaps are recorded with causes.

### 0.9 Agent efficiency and contract freeze

- Consolidate the 65-tool MCP catalog into fewer, composable tools, and tighten the existing catalog-size budget.
- Compact responses with pagination and field selection; repair hints on errors; visual and semantic differences between revisions.
- Content analysis that lets agents decide without watching footage (G06), and broader speech support (G07).
- Freeze schemas and error codes; publish the versioning and deprecation policy; test migration from every 0.x release.
- Broader editorial interchange (I05), and the Inkbolt handoff (I04) if [adopted](#open-decisions).
- Release builds and the `doctor` command.

Exit: a release candidate is benchmarked, and any gate it misses is listed.

### 1.0

All five gates pass on one build. Benchmark results are published with exact versions.

## Sister tool: Inkbolt

Inkbolt is the user's sister project: a local vector and raster editing engine for agents. Its stated split is that Cutbolt handles time, while Inkbolt handles paths, pixels and layered documents. It already shares Cutbolt's conventions: JSON envelope, saved sessions, retry receipts, create-only publication and MCP stdio. Its own plan includes a still and sequence handoff to Cutbolt with explicit losses. Inkbolt is unfinished, so this section plans the interface only.

- **Handoff manifest:** a versioned, content-bound record of document identity and revision, artboard, dimensions, color interpretation, alpha mode, frame timing for sequences, and explicit losses. Cutbolt rejects a manifest whose content does not match.
- **Linked graphics clips:** a clip references a specific Inkbolt document revision. A newer revision never replaces it silently. Relinking to a new revision is a saved edit with a diff, and it invalidates dependent previews and renders.
- **Color agreement:** both tools declare transfer, primaries and alpha explicitly, and the handoff fixture checks them independently.
- **Benchmark:** joint tasks join the suite once both sides exist.

## Draft v2 acceptance criteria

**Proposed, not adopted.** Adopting `cutbolt-local-v2` carries forward all 100 core and 8 supplemental checkpoints as regression requirements and adds the groups below. Twenty-two groups add 44 points, giving 152. If the existing evidence still passes, the starting score is 108/152 (71.1%). The v1 baseline and its history stay unchanged. Numeric targets marked *TBD* are set from 0.5 measurements before adoption.

| ID | Capability | Basic checkpoint | Broader checkpoint |
| --- | --- | --- | --- |
| M05 | Direct source ingest | Edit common H.264 and HEVC files (8/10-bit, with AAC) directly, through an automatic, cached and inspectable conversion | Rotation metadata, VFR timestamp preservation, non-square pixels, declared deinterlacing, ProRes/DNx/AV1/VP9 sources and a malformed-file matrix |
| M06 | Source audio formats | Decode AAC, MP3, FLAC, Opus and PCM at 44.1–96 kHz with declared resampling | Multichannel layouts, unusual rates and long-duration sync against VFR video |
| T11 | Mixed-format timeline | One timeline mixes sizes, rates and pixel formats with explicit per-clip fit and rate policies | Changing an existing timeline's size or rate reports its consequences; previews, proxies and exports agree |
| T12 | Native rates everywhere | Placed tracks, transitions and nested sequences render at all eight native rates | Native-rate delivery, with sample-aligned audio, for every timeline feature; drop-frame and embedded timecode |
| V09 | Timeline-native effects | Transform, opacity/blend, mask, grade and key as clip properties, rendered in one pass, matching the scene-compiled result | Every 0.4 effect available on clips with keyframes and expressions, reorderable stacks, and agreement across saved diffs, undo, previews, proxies and queued export |
| V10 | Unbounded generated content | Scenes and graphics act as timeline clips beyond 10 s and 16 layers, at native rates | 30-minute generated sequences stream with bounded memory |
| V11 | Motion-compensated retiming | Optical-flow frame interpolation checked against independent references | Occlusion and failure diagnostics, and comparison with frame blending |
| V12 | General effect library | Blur, sharpen, noise reduction, vignette and glow, with numerical references | Masks, tracking and animation on every effect, with quality regression fixtures |
| A07 | Multichannel timeline audio | Surround tracks through the timeline and lossless delivery | Surround loudness measurement, downmix checks and encoded surround delivery |
| A08 | Time-stretch and pitch | Pitch-preserving speed changes and reversed audio | Artifact limits and controlled quality comparisons |
| C06 | High-precision working space | Composite at 16 bits or more, with optional linear light | No banding regressions; dithered 8-bit output; declared equivalence with v1 fixtures |
| C07 | HDR timeline and delivery | Edit PQ/HLG on the timeline and deliver with standard metadata | Mixed SDR/HDR with declared mapping, HDR scopes and independent references |
| G06 | Content analysis for agents | Shot boundaries, silence and speech activity, and loudness maps checked against ground truth | Speaker turns, automatic subject detection for reframing, and quality warnings (black frames, clipping, flashes) |
| G07 | Broader speech support | Additional languages with word timing checked independently | Speaker attribution, confidence-driven review and mixed-language media |
| E07 | Delivery codec matrix | HEVC, AV1 and a mastering codec at native rates and common sizes, including vertical | Compatibility confirmed with several independent decoders; hardware encoders within declared tolerance |
| E08 | Platform-ready deliverables | Presets with declared loudness and true-peak targets, and embedded or sidecar captions | Every preset validated by independent measurement, including chapters and thumbnails |
| I04 | Inkbolt linked graphics | Place an Inkbolt document revision through a content-bound handoff | Revision changes invalidate dependents; explicit losses; edits round-trip through both sessions |
| I05 | Broader editorial interchange | Markers, metadata and nested stacks through public interchange, with loss reports | Speed effects and multi-track audio round trips, and additional public formats chosen from evidence |
| P04 | Interactive previews | p95 single-frame preview latency within *TBD* ms on a 1080p multi-track timeline | 4K with effects; ranges; cold and warm cache; both platforms |
| P05 | Incremental rendering | After an edit, re-render only the affected segments, identical to a full render | Segment cache across sessions; interrupted renders resume from completed segments |
| P06 | GPU rendering | GPU compositing of the common graph within declared tolerance of the CPU reference | Both platforms, fallback, and measured speedup of at least *TBD* |
| P07 | Long timelines | A two-hour, 5,000-clip timeline renders whole within *TBD* memory | Long-form stress with failure injection and performance regression limits |

Scores reported separately, which add no editing points:

- **Agent foundations:** the existing 10/10 checks remain required.
- **Platform parity:** every verifier suite passes on Windows and Linux.
- **Agent efficiency:** tool calls, tokens, catalog size and first-attempt success, taken from benchmark runs.

## Open decisions

| Decision | Recommendation |
| --- | --- |
| Public distribution: whether and how 1.0 is published | Decide before 0.9. FFmpeg stays an external install. The tested build is GPL-3, so shipping or bundling one needs a license review. Publishing also needs a provenance review of the full Git history. |
| Inkbolt boundary: whether advanced vector and raster graphics come from Inkbolt instead of growing Cutbolt's text and shapes | Yes. Cutbolt's shapes lack paths, gradients and antialiasing, which Inkbolt already has. Cutbolt keeps the text, shapes and captions video needs; Inkbolt owns documents. |
| Whether I04 is required for 1.0 | Required only if Inkbolt's handoff checkpoint is verified first. Otherwise I04 moves to the next baseline, with a recorded reason. |
| Media backend: external FFmpeg processes or linked libraries | Keep processes until 0.5 measurements show a bottleneck. Linking changes licensing and distribution. |
| GPU API: NVIDIA-only or vendor-neutral | Decide from 0.5 measurements. G3 requires the same behavior on both platforms. |
| Linux recording | Implement a Linux capture backend. The alternative is to declare recording Windows-only, which is an explicit, recorded scope reduction. |
| Cloud-only alternatives in the benchmark | Include them where their terms allow original media, and report them separately. Local-only is Cutbolt's requirement, not theirs. |
| Benchmark agent models | Use at least two current models, so results are not tuned to one agent. |

## Maintaining this roadmap

This document stays a draft until the user adopts it. Adopting v2 means:

1. adding a criteria file to `progress/` in the existing feature shape (`id`, `name`, `basic`, `extended`, and evidence lists);
2. adding a v2 generator, output document and check to `tools/progress.py`, and the baseline to `tools/verify.py`. The existing generators assume exactly 50 core features and the 100- and 108-point totals; leave those invariants intact so v1 keeps generating unchanged;
3. recording the change in [progress history](PROGRESS_HISTORY.md).

Points follow the existing rules: only specific verified acceptance criteria earn them; plans, infrastructure and benchmark results earn none. Changes to scope, gates or targets are logged in progress history. Never shrink a denominator silently.
