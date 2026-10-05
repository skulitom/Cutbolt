# Cutbolt

An original, local video editing engine for agents. Designed to let agents pick up where they left off.

**Verified acceptance coverage: 100/100 core and 108/108 total (100%), with 466 passing checks.** See the [implementation tracker](docs/IMPLEMENTATION_PROGRESS.md) for the unchanged scope and documented limits.

Full verification includes [typed property expressions](docs/EXPRESSIONS.md), [shutter sampling and motion blur](docs/TEMPORAL.md), [3D planes, cameras and lighting](docs/GEOMETRY.md), and [local annotated foreground masks](docs/SEGMENTATION.md). These use the existing scene and saved-edit workflows; the optional segmentation worker keeps its pinned dependencies external.

**Status: v0.4 native tracks, scenes, audio mixes, media conversion and previews, 4 October 2026.** An original Rust library and JSON command-line program can create project snapshots, validate timelines, append/trim/split/reorder/remove clips, inspect local media, plan renders, and produce verified lossless output. Local sessions now persist revisions, reject conflicting edits, replay successful requests safely, preview edit diffs, and support undo/restoration with history. See the [live progress checklist](docs/PROGRESS.md).

Agents use structured commands and local library calls. No hosted API, web server, account, or network connection is required at runtime. A native MCP stdio adapter exposes the same commands. On Windows, background render jobs persist status and progress, support cancellation, and detect interrupted workers. Full-size frame and exact-range previews are available. Optional [bounded render retries and publication recovery](docs/RENDER_RECOVERY.md) pass full verification with bounded retries and checked publication recovery.

[Editorial interchange](docs/INTERCHANGE.md) imports and exports a bounded public OTIO subset with explicit local media bindings, exact track/audio timing, generic dissolves and path-specific loss acknowledgements. Import returns a proposed snapshot for the existing saved-session workflow. Reference-library, decoded-output and full integration verification pass.

[Bounded native-project import](docs/NATIVE_PROJECTS.md) accepts content-bound transfers from an installed source application’s public interface, with exact build/adapter acceptance, explicit omissions and blocking unsupported structures. Full verification passes against content-bound recorded captures from one exact build, including decoded output and unsupported-feature diagnostics.

[Portable project history](docs/PORTABLE_PROJECTS.md) adds checked relative media/proxy paths, explicit editing-store migration, consistent backups during live editing and recovery with request receipts and undo intact. Full verification passes against an actual older engine and process/disk-full faults.

The [local asset registry](docs/REGISTRY.md) adds searchable bins, tags and metadata, explicit content identities and checked relinking after media moves. Metadata and path changes use saved-session diffs and undo/history. The 1,000-clip edit fixture measures latency and peak memory separately from the narrower renderer limits.

Sequential editing includes explicit insert, overwrite and ripple deletion, plus slip, slide and rolling edits. These preserve declared frame boundaries and linked audio/video source ranges, with inspectable before/after placements through saved sessions. A separate [audio mix recipe](docs/AUDIO.md) provides sample-based cuts, mono/stereo conversion, gain, mute, fades, automation and track mixing. It exports WAV or supplies a scene soundtrack; compiled scenes use the existing session contract. Ordered master EQ/compression and final PCM loudness, sample-peak and RMS meters are also available.

[Native tracks and linked groups](docs/TRACKS.md) add explicitly placed video/audio, track order, enabled state, locks, targeting and whole-clip collision policies. Video uses the highest active opaque track, with optional straight-alpha `alpha_over` tracks composited above it exactly; audio sums exact sample placements. Linked moves/removals and collateral replacements use saved diffs, retries, undo and history. Existing sequential projects can be promoted explicitly. [Native boundary edits](docs/TRACK_EDITS.md) add linked splits, slip/slide/roll and interval insert/overwrite/ripple with explicit survivor IDs, end policies and transition handling. Placed-track output retains its documented 25 fps restriction.

[Editable transitions](docs/TRANSITIONS.md) add dissolves, dips to black and directional wipes with explicit incoming/outgoing source handles. Stereo audio supports linear crossfades and dips through silence. Transition clocks survive occlusion, inside-transition range cuts, proxy previews and queued full-quality output; settings remain in saved sessions.

[Reusable nested sequences](docs/SEQUENCES.md) keep shared child timelines editable in the same project. Instances map exact video/audio source windows into child time, preserving child and parent transitions. Child edits propagate through every instance, with cycle, handle, depth and transitive lock checks. Saved diffs identify changed definitions and affected root instances; previews, proxies, ranges and queued output use the nested graph.

[Editable camera groups](docs/MULTICAM.md) retain synchronized alternatives, recoverable cut decisions and fixed/follow/mute audio in reusable sequences. [Audio/timecode inspection](docs/SYNCHRONIZATION.md) estimates bounded constant clock drift or maps declared 25 fps labels, returning explicit conversion recipes for new aligned media. Camera edits use the same saved history, locks, previews and export paths.

[Long-form reference rendering](docs/LONG_FORM.md) adds explicit large-raster thread/deadline budgets and bounded Windows temporary-file cleanup. The complete 30-minute moving 4K fixture and queue/write-failure gates pass full verification.

Explicit timeline gaps produce black video and silence, including in previews and queued renders. They can be split, trimmed, moved, inserted or overwritten through the same editing commands. Mixed-rate compressed video and audio-only sources use the declared conversion path before precise timeline trimming; first/last-frame and keyframe boundaries are covered by independent output checks.

[Validated media conversion](docs/CONFORM.md) brings a bounded matrix of FFV1/PCM MKV, tagged BT.709 H.264/AAC MP4/MOV and PCM16 WAV into the reference timeline. It preserves source files, checks identities and decoded timestamps, and provides constant speed, reverse and freeze recipes with an explicit audio policy. Sources up to 16 GiB and outputs up to 30 minutes stream only the frames they need; reverse mappings use a bounded random-access window. Converted assets use ordinary saved-session editing and export.

[Optional hardware decoding](docs/ACCELERATION.md) selects an explicit local CUDA device for bounded progressive H.264 sources. Software remains the default; receipts expose the selected backend, fallback and timings. Full acceptance verifies actual device decoding, fallback, pixel/audio agreement and measured throughput.

[Explicit SDR normalization](docs/COLOR.md) converts tagged or explicitly interpreted RGB/YUV sources into a consistent sRGB or BT.709 working transfer. It checks metadata conflicts, performs original transfer/matrix/range math and produces tagged editing assets for saved timelines.

[LUTs and numerical scopes](docs/LUTS_SCOPES.md) load external 1D/3D color tables with explicit interpolation, apply them after normalization and inspect full-quality timeline histograms, waveform/parade and vectorscope counts. Color assumptions, table identities and clipping are explicit.

[High-bit-depth and HDR conversion](docs/HDR.md) processes native 10/12/16-bit RGB/YUV444 with declared PQ, HLG or SDR interpretation, exposure, gamut conversion and tone mapping. It produces tagged 16-bit intermediates with standard display metadata, or 8-bit SDR assets for the existing timeline. The general timeline remains 8-bit SDR.

[Local cache and contact sheets](docs/CACHE_PREVIEWS.md) add explicit content-checked reuse for probes, proxies, frames and preview intervals, with bounded LRU storage, corruption checks and caller-selected thumbnail grids. Saved revisions and source changes invalidate stale entries. Both cache checkpoints and contact-sheet acceptance pass full verification.

[Proxy previews](docs/PROXIES.md) provide half, quarter and eighth-size variants with unchanged frame timing and audio. Attachments, preview selection and relinking use saved sessions with undo/history. Final synchronous and queued exports always read the full-quality media, even when a proxy preview is selected.

[Scenes](docs/SCENES.md) render natively up to 4096 pixels per axis (output scale 1) or enlarge small artwork exactly; `tilemap` layers assemble large canvases from reusable, independently animated tiles, and frames stream to the encoder without a raw-video buffer.

[Text and shapes](docs/GRAPHICS.md) add external TrueType fonts, explicit fallback, bounded LTR layout, rectangles and ellipses to scenes. They use the same transform, mask and position/opacity animation as image layers. [Reusable templates](docs/TEMPLATES.md) expose typed parameters with validated, independent instances; original lower-third and title-card recipes are included. [Timed captions](docs/CAPTIONS.md) now import/edit/export a bounded SRT/WebVTT subset and compile explicit styled windows into scenes. Optional [Unicode shaping and bidirectional layout](docs/UNICODE_TEXT.md) now adds contextual forms, combining marks, cluster-safe fallback and word wrapping with explicit external fonts.

[Spatial transforms](docs/SPATIAL.md) add animated scale/rotation, subpixel movement, mirroring, declared pixel aspect and contain/cover/stretch fitting to scenes. Nearest or bilinear sampling handles alpha before blending; existing integer scenes retain identical output. Inspection exposes each sampled transform and its matrices.

[Motion tracking and mask feathering](docs/TRACKING.md) provide confidence-bearing patch trajectories and editable mask curves, with inner/centered/outer soft edges that preserve transparency through transforms and blending. Compilation applies the returned scene explicitly; ambiguous or lost matches reject.

[Local speech recognition and transcript editing](docs/TRANSCRIPTS.md) add optional offline English/Greek recognition, source-bound word records, explicit corrections and reviewed word-based cuts. Estimated contextual intervals retain acoustic evidence; exact native edits use saved preview/apply, retry and undo. `timeline.outline` reads a cut as compact text, one line per clip, with the words each clip contains. `scene.still` shows one frame of a title or graphic before it is compiled. `captions.draft` turns transcripts of the sources into timed caption cues for the cut, and `captions.render` burns a whole caption track into one transparent overlay. `audio.tighten` proposes jump cuts that shorten pauses in speech and `transcript.fillers` removes um and uh, both checked to apply, and `audio.beats` finds a music track's tempo and beats for cutting to it. `audio.normalize` proposes clip levels that bring the mix to a loudness target such as -14 LKFS, measured before they are returned. `export.review` checks a delivered file: a sheet, a small preview copy, loudness, black frames, timing, and the words heard against the words intended. The external runtime is selected explicitly and remains outside the repository. `media.transcribe` recognizes a whole file of any length and format in one queued job, as stitched documents bound to the file.

[Subject reframing](docs/REFRAMING.md) combines authored boxes or optional local tracking with explicit crop windows, subject changes and bounded pan motion. Inspection returns each decision and an editable scene, preserving the soundtrack. Its independent rendered fixture verifies both reframing checkpoints within the documented bounds.

[Camera stabilization](docs/STABILIZATION.md) measures local image motion and returns editable translation/roll compensation. Lock or smooth a shot, restart at declared cuts, and choose visible source edges or a bounded constant zoom; rendering applies the inspected recipe explicitly.

[Variable speed ramps](docs/REMAPPING.md) integrate an exact source clock for acceleration, stops and reversals. Choose held, nearest or blended frames and explicit pitch-following audio or silence; completed assets use the existing saved-session workflow.

[Audio buses and channel routing](docs/AUDIO_ROUTING.md) add explicit matrices, panning, bus processing and named mono/stereo/surround WAV outputs. Stereo downmixes compile into the existing scene and saved-session workflow.

[Local dialogue cleanup](docs/DIALOGUE_REPAIR.md) uses explicitly selected noise-only regions, optional mean removal and ordered processing to produce a fresh PCM WAV. Exact sample timing, independent noise/voice quality comparisons and saved-scene integration are verified separately.

[Local recording](docs/RECORDING.md) provides explicit Windows input selection, streamed PCM WAV capture and exact native audio-track placement through the existing saved-session workflow.

[Primary grading](docs/GRADING.md) adds exposure, contrast, explicit RGB white balance and master/channel curves to scene layers. Ordered grades use linear-light sRGB, preserve alpha and support animated controls; inspection reports sampled values before compilation into ordinary editing assets. [Selective correction](docs/SELECTIVE_COLOR.md) adds HSL color bands, feathered masks and animated correction strength while preserving alpha and unselected pixels. [Chroma keying](docs/KEYING.md) removes screen colors with spill control, optional soft-edge color recovery and four editable green/blue presets.

[Range and delivery exports](docs/EXPORT.md) select exact timeline intervals and audio-only, video-only or combined output. The fixed H.264/AAC preset validates timing, declared BT.709 conversion and decoded output; reference exports preserve exact decoded pixels and samples. Exports always use full-quality sources.

[Optional delivery controls](docs/DELIVERY_CONTROLS.md) add explicit quality and two-pass bitrate settings, constrained/main/high stream profiles and AAC bitrates. Full verification checks the selected GPU and software decoder, measured quality/rates and failure cleanup.

[Native-rate lossless export formats](docs/EXPORT_FORMATS.md) add RGB PNG movies and complete numbered-image directories with exact clocks, optional PCM soundtracks and checked publication. The existing reference export also accepts native sequential rates. Full verification includes portrait, odd-sized and DCI-4K output.

[Numbered footage and transparent intermediates](docs/IMAGE_SEQUENCES.md) validate the complete source list, preserve declared straight RGBA through lossless movie profiles and explicitly flatten into native editing assets when requested. Full verification passes alpha, saved-edit and complete 30-minute synchronization checks.

## Run locally

Requires Rust/Cargo with a C build toolchain for SQLite, Python 3 with the development packages in [the dependency ledger](docs/DEPENDENCIES.md) for verification, and separately installed FFmpeg/ffprobe on PATH. The tested environment is recorded in [verification/latest.json](verification/latest.json); dependency details are in [docs/DEPENDENCIES.md](docs/DEPENDENCIES.md).

```powershell
cargo build --locked
.\target\debug\cutbolt.exe capabilities
.\target\debug\cutbolt.exe examples/create-project.json
python tools/verify.py                              # quick pre-commit check
python tools/verify.py --thorough --decode-device 0  # evidence run
# An MCP client can launch this local command:
.\target\debug\cutbolt.exe mcp
# Or bind one workspace: roots default inside it and paths may be relative.
.\target\debug\cutbolt.exe --workspace C:\DEV\CutboltData\demo mcp
```

For a retained demo, use a new output directory outside the repository:

```powershell
python tests/integration.py --output C:\DEV\CutboltData\my-demo
```

This creates original synthetic clips, edits them, renders a 30-second sequence, and verifies every decoded frame and audio sample against independent expected source slices. The fixture uses only original generated media and the declared external media tools.

The current renderer accepts FFV1/bgr0 video with matching dimensions and 48 kHz stereo PCM audio, exporting Matroska. Sequential timelines support [eight exact native frame rates](docs/NATIVE_TIMING.md), including fractional clocks with sample-aligned cuts, for sequential timelines, placed tracks and H.264 delivery. Full verification passes native-rate, VFR conversion and complete 30-minute synchronization checks. The scene compiler accepts bounded PNG layers and separate WAV narration, with crop, integer scale, quarter-turn rotation and position/opacity keyframes. Curves support hold, linear and quadratic easing, with optional delay, speed adjustment and reversal for each property. Layers support normal/multiply/screen blending, explicit straight or stored premultiplied alpha, and static or keyframed rectangular masks with inversion. Its five-second reference fixture renders at 1080p. Arbitrary H.264 ingestion, general timeline effects and broader delivery/device profiles remain open. See [scene commands and the runnable example](docs/SCENES.md). See [command usage and limitations](docs/USAGE.md).

## Plan

- [Local jobs and MCP setup](docs/AGENT_INTERFACE.md)
- [Implementation plan and milestones](docs/PLAN.md)
- [Research implementation roadmap](docs/IMPLEMENTATION_ROADMAP.md) and [combined completion percentage](docs/IMPLEMENTATION_PROGRESS.md)
- [PixelForge + Qwen YouTube pilot: preparation and agent handoff](docs/YOUTUBE_PIPELINE.md)
- [Architecture and agent command contract](docs/ARCHITECTURE.md)
- [Research and publication boundaries](docs/RESEARCH.md)
- [Percentage tracker](docs/PROGRESS.md) and [progress history](docs/PROGRESS_HISTORY.md)
- [Competitive landscape and proposed USP](docs/COMPETITIVE_ANALYSIS.md)

The recommended path is an independently implemented timeline and execution engine, initially using a separately installed, reviewed FFmpeg build for media processing. Project compatibility remains a separate workstream with explicit acceptance criteria.

Reliability comes first: saved editing sessions, safe retries and persistent undo are implemented and exercised with process interruption and concurrent-writer tests. The ten foundational agent checks now cover saved sessions, local queued jobs, cancellation and MCP. This is a bounded acceptance checklist, not full production readiness or completion of the editing engine. Next extend scene duration/production handling and broaden source formats. Bounded render retry, checked publication reconciliation and editing-history migration/recovery pass full acceptance; broader platform validation remains open.

The agreed first compatibility pilot is a six-scene pixel-art explainer using external PixelForge assets and local Qwen3-TTS narration, with versioned review stages and selective revisions. The [handoff](docs/YOUTUBE_PIPELINE.md) records the preparation, pinned model download, implementation slices and acceptance cases. The bounded PixelForge handoff and single-scene rendering path are implemented; Qwen inference and the full production/review/recovery workflow remain open.

## Local project location

Use `C:\DEV\Cutbolt` on this machine. It is a Windows directory junction to the existing `C:\DEV\AgentCut` checkout because the desktop workspace holds that directory open. Both paths refer to the same files; there is no second copy. Package, executable and environment settings use `cutbolt` / `CUTBOLT_*`. A physical folder move can be done after the active workspace is closed.

## Repository boundary

Original project code is licensed under [MIT](LICENSE). Dependencies retain their own licenses. Keep original code, documentation, schemas, and fixture generators here; dependencies are fetched to the external package cache. Build output is ignored. No third-party application files, decompiled code, copied third-party implementation, or media assets are included in the committable tree.

The ignore file and `python tools/check_repo.py` catch common accidental additions; neither proves provenance. Record external dependencies and material origins, and review the full Git history before distribution.
