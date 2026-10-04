# Implementation plan

Planning baseline: 2 October 2026. Project name: Cutbolt (user selected).

Implementation has started. The current snapshot CLI, sequential editing operations and reference renderer are described in [USAGE.md](USAGE.md). Track verified capability coverage in [PROGRESS.md](PROGRESS.md); the milestones below remain the broader roadmap. Original project code is now MIT licensed.

The [research implementation roadmap](IMPLEMENTATION_ROADMAP.md) now maps the full core scope and four additional research-derived areas into eight delivery phases. Its [generated combined tracker](IMPLEMENTATION_PROGRESS.md) uses 108 explicit checkpoints while preserving the original 100-point core baseline. The first delivered slice adds bounded position/opacity keyframes to scenes; later phases remain planned.

## Objective and assumptions

Build an original video editing engine that an agent can reliably operate on a local Windows computer. It must turn explicit editing instructions into inspectable timeline changes and reproducible render jobs without GUI automation. A visual editor can be added later.

The user confirmed that a local programmatic interface is acceptable and does not want a hosted API. The recommended baseline is Windows first, CPU rendering first, one local project writer at a time, and multiple callers protected by revision checks. Network access is unnecessary for normal operation once the tools are installed.

Project interoperability is a separate compatibility target. Proprietary effect implementations, third-party plugin hosting, application linking, cloud features and licensing components are outside the initial deliverable. Underlying editing operations remain on the broader roadmap.

## Original implementation and research boundaries

The desired product needs an original timeline model, edit semantics, agent interface, job control, and renderer integration. Whole-application decompilation does not directly specify those requirements or produce maintainable original source. It also creates a substantial provenance problem under the user's requirement to keep copied materials out of the repository.

Develop the core from original requirements and public technical documentation. Investigate a particular interoperability obstacle only when it blocks a concrete feature. The [research plan](RESEARCH.md) explains the conditions and bounded inspection workflow for that separate workstream.

## Initial feature scope

| Area | First supported behavior | Deferred |
| --- | --- | --- |
| Media | Inspect local files; reference originals; generate proxies and thumbnails | Network media, camera RAW workflows |
| Timeline | Add, trim, split, move, remove; explicit gaps; linked audio/video; multiple tracks | Nested sequences, multicam, variable speed |
| Video | Cut, scale, crop, position, opacity, cross-dissolve | Proprietary effects, advanced grading, HDR |
| Audio | Gain, mute, fades, mix; explicit sample-rate conversion | Plugin hosting, advanced restoration |
| Inspection | Timeline summaries, warnings, frame previews, contact sheets, audio peaks | Built-in speech/vision model downloads |
| Output | Lossless reference output; one validated delivery preset | Broad codec and device certification |
| Agent control | Typed commands, batch edits, dry run, revision checks, retry safety, undo | Cloud collaboration and remote workers |
| Interchange | Native versioned project format | OTIO adapter and third-party interchange after core correctness |

Initial delivery target: 1920 x 1080 SDR, progressive output, 25 fps and 48 kHz stereo audio. Timing tests also cover 24000/1001 and 30000/1001 frame rates. VFR sources must either pass a tested timestamp normalization path or be rejected with a specific diagnostic; never silently treat them as constant frame rate.

## Milestones and exit criteria

Effort ranges below are provisional focused engineering days for one experienced developer, excluding legal review, tool installation delays, codec distribution work, and unforeseen media edge cases. They are not delivery promises. Budget roughly 6-10 calendar weeks for a dependable small MVP, then revise after the first render slice.

### 0. Establish the development boundary (1-2 days)

- Select the Rust toolchain and a reviewed local FFmpeg/ffprobe build; record exact versions, build flags, and licenses. No binaries or dependency source in Git.
- Establish external media/output/cache roots and a source/provenance ledger.
- Implement a staged-file check for excluded artifacts, large binaries, and missing fixture provenance. A filename check supplements human review; it cannot prove originality.
- Add original synthetic fixture generation: color panels, frame identifiers, moving shapes, sine tones, and synchronization impulses. Generated media stays outside Git.

Exit: a fresh checkout can validate its tools and generate fixtures locally; repository contents and exclusions can be audited. Binary-analysis tools are not dependencies of this milestone.

### 1. Timeline and edit semantics (3-5 days)

- Implement exact rational time, media references, tracks, clips, source ranges, timeline ranges, and linked audio/video groups.
- Implement versioned serialization, atomic command batches, validation, revision checks, durable idempotency records, and undo through prior snapshots.
- Define separate overwrite and ripple operations; never guess the caller's intent when neighboring clips would move.

Exit: serialization preserves a project; split then join preserves covered source ranges; invalid edits leave state unchanged; stale writers are rejected; a repeated request cannot apply the same edit twice.

### 2. End-to-end render slice (5-10 days)

- Compile a small timeline into a render plan and invoke local media tools using argument arrays, without a shell.
- Support precise source trimming, timestamp rebasing, concatenation, explicit output duration, and audio alignment.
- Produce a Matroska/FFV1/PCM reference output if supported by the selected build. Add an MP4 H.264/AAC preset only after verifying the selected encoders and intended distribution conditions.
- Provide structured progress, cancellation, bounded diagnostics, and a render manifest.

Exit: an agent assembles three synthetic clips into a 30-second sequence, creates previews, and renders it without opening a GUI. Automated checks verify cut-frame identity, frame count, sample count, and synchronization impulses. Cancellation leaves no apparently completed output; source files are unchanged.

### 3. Useful editing and inspection (5-10 days)

- Add multiple video/audio tracks, transforms, opacity, gain, fades, and dissolves with explicit handle requirements.
- Add proxies, frame previews, contact sheets, audio peak summaries, and cached inspections.
- Define and test SDR color conversions, aspect-ratio behavior, alpha compositing, and audio clipping policy.

Exit: a short montage combines cuts, a dissolve, an overlay, and mixed audio. The engine detects insufficient transition handles, unsupported color inputs, missing media, and invalid overlaps. Preview and final render implement the same timeline semantics.

### 4. Agent integration and recovery (3-5 days)

- Add CLI JSON commands and optional Python convenience bindings; expose the same contract through an optional MCP stdio adapter.
- Add capability discovery, project summaries, edit diffs, structured errors, and local job recovery.
- Exercise concurrent callers, crashes between state transitions, interrupted renders, and a lost response followed by a retry.

Exit: an agent can create, inspect, edit, undo, preview, render, cancel, and resume work using machine-readable results. Invalid operations produce actionable diagnostics without partial edits. No listening socket or hosted component is needed.

### 5. Interchange and targeted compatibility (separate estimate)

Start with an OTIO adapter for the supported subset and test a documented interchange route before considering native project import. A candidate format does not establish compatibility. Verify an exact selected build and report every unsupported construct.

Exit: agreed clips, timing, tracks, and media references survive a controlled interchange test. Unsupported effects are reported, never silently discarded. Native third-party project support requires its own compatibility matrix and the research prerequisites below.

## Verification strategy

Use original generated fixtures so tests do not depend on third-party application content or downloaded sample media. Test semantic correctness before visual polish or speed.

- **Time:** exact cut boundaries; fractional frame rates; long-duration arithmetic; sample alignment; nonzero source timestamps and VFR handling.
- **Edits:** atomic batches; explicit gap/overlap behavior; undo; stale revisions; duplicate requests after lost responses.
- **Rendering:** expected frame identifiers and synchronization impulses; decoded frame/sample comparison in a pinned CPU reference environment; named tolerances for delivery encoders. Do not promise byte-identical files across GPUs, tool versions, or lossy encoders.
- **Resilience:** media missing or changed after planning; insufficient disk space; worker crash; cancellation; restart; no mutation of originals.
- **Agent usability:** discover capabilities, repair an invalid request, inspect a preview, and finish a project without reading console prose or clicking a UI.

Record performance on the actual machine after milestone 2: cold/warm preview latency, elapsed render time, peak memory, source/project sizes, and cache hit rate. No claim of real-time 4K performance until measured. A two-track 1080p fixture and a 1,000-clip synthetic timeline provide initial scale checks.

## Key tradeoffs and unresolved decisions

| Decision | Baseline | Revisit when |
| --- | --- | --- |
| Core language | Rust for the model, command executor, and CLI | Tooling cost outweighs reliability benefits in the first slice |
| Media backend | External FFmpeg processes | Profiling shows process boundaries or filter graphs limit previews/compositing |
| Project model | Our small versioned schema | Interchange tests establish a reason to adopt more OTIO semantics |
| Acceleration | CPU reference path first | Correctness is stable and actual performance measurements identify a bottleneck |
| Rendering granularity | Whole export or explicit preview interval | Measured workload justifies chunk caching and transition boundary complexity |
| Repository license | MIT for original code (user selected) | Dependency and binary distribution requirements need separate review |

We still need a target machine/GPU and representative editing tasks to tune performance. These do not block the initial design. Further proprietary application research requires an exact build, an established access basis, applicable prerequisites and a specific interoperability requirement, retained in private records.

## Concrete next implementation task

The first crate, schema, rational-time types, command validator, synthetic fixtures, and verified three-source render are implemented. The user prioritized reliability first: local transactional sessions, durable retries, undo/restoration, semantic diffs and paginated history now exist. Process-crash and concurrent-writer tests verify this slice. The persisted local queue and native MCP stdio adapter now have protocol, interoperability, cancellation and worker-interruption evidence. Next add supported everyday source formats and video previews. Milestones 0-2 remain partially complete: the broader timing/media matrix, schema migration/relinking, previews, automatic render retry/publication reconciliation and broader recovery still need implementation.

## Agreed compatibility pilot: PixelForge and Qwen3-TTS

On 2 October 2026 the user agreed to prepare a local 60-90-second, six-scene YouTube explainer workflow, with PixelForge visuals, Qwen3-TTS narration, staged review and selective revisions. The tools remain independent and communicate through local files/adapters. Model weights live outside the repository. Uploading or hosting is not part of the pilot.

Read [YOUTUBE_PIPELINE.md](YOUTUBE_PIPELINE.md) for the agent handoff and [pipeline/ACCEPTANCE.md](pipeline/ACCEPTANCE.md) for ordered P1-P6 slices. The concrete next slice is an original PixelForge frame/timing fixture and a one-scene PNG/WAV ingestion and preview path, before six-scene coordination. The existing session store is the timeline reliability foundation. Preparation does not claim inference or cross-product compatibility; the broader roadmap, weights and accepted exclusions stay unchanged.
