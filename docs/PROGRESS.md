# Progress against the selected editing capabilities

**Verified checklist coverage: 100.0% (100/100 acceptance points).**

Baseline: `cutbolt-local-v1`. Verified: 2026-10-04. All 50 capability groups remain in the denominator.

This measures our explicitly scoped checklist, not equal engineering effort, cross-product equivalence or completion of every professional editing feature. Each capability has two explicit checkpoints worth one point each. A working narrow subset earns one; its broader acceptance cases must pass to earn the second. Plans and code without passing evidence earn zero.

## Accepted exclusions

- Accounts and activation
- Cloud services and collaboration
- Stock marketplaces
- Telemetry
- GUI-only workflows (underlying editing operations remain in scope)

Offline AI-assisted editing and project interoperability remain in scope. Third-party plugin loading and proprietary implementations are not required; original equivalent editing behavior is the target. Excluding GUI workflows does not exclude the editing operations behind them.

## Coverage by area

| Area | Verified points | Coverage |
| --- | ---: | ---: |
| Media | 8/8 | 100.0% |
| Timeline | 20/20 | 100.0% |
| Video | 16/16 | 100.0% |
| Audio | 12/12 | 100.0% |
| Color | 10/10 | 100.0% |
| Text and graphics | 10/10 | 100.0% |
| Export | 12/12 | 100.0% |
| Projects and interchange | 6/6 | 100.0% |
| Performance | 6/6 | 100.0% |

## Acceptance checklist

A score of 1/2 means the basic checkpoint is verified; the broader checkpoint remains open.

| ID | Capability | Points | Basic checkpoint | Broader checkpoint |
| --- | --- | ---: | --- | --- |
| M01 | Local media inspection | 2/2 | Inspect streams and dimensions of local sources | Validated codec/container matrix including malformed files |
| M02 | Bins, search and metadata | 2/2 | Organize and search a local asset registry | Metadata round trips, large collections and duplicate handling |
| M03 | Relinking and offline media | 2/2 | Detect and relink missing source media | Content identity, moved projects and ambiguous replacement handling |
| M04 | Proxy workflow | 2/2 | Generate and switch to proxies | Relink, mixed rates and final-quality export preserve timing |
| T01 | Sequential assembly | 2/2 | Render ordered clips into one continuous sequence | Gaps, overwrite/insert interaction and mixed source formats |
| T02 | Source in/out trimming | 2/2 | Render exact source ranges on a frame boundary | Mixed frame rates, keyframe codecs and audio-only edges |
| T03 | Split edits | 2/2 | Split a clip without changing rendered content or duration | Linked media, transitions and subframe audio splits |
| T04 | Move and remove clips | 2/2 | Reorder and remove sequential clips with verified output | Multiple tracks, selections, locks and collision policies |
| T05 | Exact timing | 2/2 | Exact rational arithmetic and verified 25 fps render boundaries | Rendered fractional-rate, VFR and long-form sync fixtures |
| T06 | Insert, overwrite and ripple | 2/2 | Explicit insert/overwrite/ripple semantics | Cross-track locks, sync groups and boundary collisions |
| T07 | Slip, slide and rolling edits | 2/2 | Implement each operation with visible edit diffs | Transitions, handles and linked audio remain valid |
| T08 | Tracks and linked groups | 2/2 | Multiple tracks with linked audio/video | Track targeting, locking, gaps and sync protection |
| T09 | Nested sequences | 2/2 | Render reusable nested sequences | Time mapping, cycles, changes and nested effects |
| T10 | Multicam and synchronization | 2/2 | Switch between synchronized camera angles | Audio/timecode sync, drift and recoverable angle edits |
| V01 | Transform and crop | 2/2 | Position, scale, rotate and crop a clip | Aspect ratios, interpolation and animated transforms |
| V02 | Opacity and compositing | 2/2 | Composite layered clips with opacity | Blend modes, premultiplied alpha and edge correctness |
| V03 | Transitions | 2/2 | Cross-dissolve with explicit source handles | Transition set, asymmetric handles and mixed content |
| V04 | Keyframes and easing | 2/2 | Animate effect parameters | Interpolation modes, retiming and deterministic evaluation |
| V05 | Masks and tracking | 2/2 | Static and keyframed effect masks | Motion tracking and robust feather/edge behavior |
| V06 | Stabilization | 2/2 | Stabilize a handheld fixture | Scene cuts, rolling motion and crop-quality tradeoffs |
| V07 | Speed and time remapping | 2/2 | Constant speed, reverse and freeze frame | Variable speed ramps, interpolation and audio pitch policy |
| V08 | Keying and reusable effects | 2/2 | Chroma key and independent effect chains | Spill control, presets and representative effect quality tests |
| A01 | Synchronized audio cuts | 2/2 | Sample-exact linked audio cuts and concatenation | Audio-only edits, sample rates and channel layouts |
| A02 | Gain, mute and fades | 2/2 | Gain, mute and fades on clips | Automation curves, crossfades and clipping behavior |
| A03 | Track mixing and routing | 2/2 | Mix multiple audio tracks | Buses, channel mapping, pan and surround layouts |
| A04 | Audio processing and meters | 2/2 | EQ, dynamics and loudness/peak measurements | Meter accuracy and effect-chain regression fixtures |
| A05 | Dialogue repair | 2/2 | Local denoise and basic dialogue cleanup | Noise types, artifact limits and controlled quality comparisons |
| A06 | Audio recording | 2/2 | Local input recording onto a sequence | Latency compensation, device changes and long capture |
| C01 | SDR color management | 2/2 | Explicit transfer, matrix and range conversions | Mixed tagged/untagged media and reference chart tolerances |
| C02 | Primary grading | 2/2 | Exposure, contrast, white balance and curves | Numerical color-chart tests and animated grading |
| C03 | Secondary grading | 2/2 | Selective color correction | Qualifiers, masks and difficult boundary tests |
| C04 | LUTs and scopes | 2/2 | Load supported LUTs and provide scopes | Interpolation, out-of-range values and validated scope math |
| C05 | HDR and high bit depth | 2/2 | Explicit high-bit-depth/HDR processing path | Tone mapping, display/output metadata and reference comparisons |
| G01 | Text and shapes | 2/2 | Original text and shape overlays using user-supplied fonts | Layout, Unicode, font fallback and transparency |
| G02 | Animated graphics | 2/2 | Animate text and shape properties | Reusable original templates and parameter validation |
| G03 | Captions and subtitles | 2/2 | Import, edit and export timed captions | Styles, multiline layout, overlap and accessible output formats |
| G04 | Transcription and text edits | 2/2 | Optional local transcription and text-range cuts | Word alignment, languages and synchronized edit corrections |
| G05 | Reframing and content selection | 2/2 | User-directed tracking/reframing with explicit crop decisions | Subject changes, motion and optional local analysis |
| E01 | Lossless reference export | 2/2 | Verified FFV1/PCM output from the sequence | Additional lossless profiles, long renders and color/alpha cases |
| E02 | Delivery encoding | 2/2 | One validated H.264/AAC delivery preset | Bitrate, rate control, quality and device compatibility matrix |
| E03 | Previews and stills | 2/2 | Timeline frame previews and still export | Contact sheets, cached intervals and preview/final agreement |
| E04 | Batch export and job control | 2/2 | Queued exports with progress and cancellation | Crash recovery, retries and bounded concurrency |
| E05 | Format and range options | 2/2 | Selected range and audio/video-only exports | Broader containers, frame sizes, rates and image sequences |
| E06 | Validation and source protection | 2/2 | Verify output counts and refuse source/output overwrites | Crash, disk-full and changed-source failure injection |
| I01 | Native project save/load | 2/2 | Round-trip a portable versioned project snapshot | Migrations, relative media paths, history and recovery |
| I02 | Editorial interchange | 2/2 | Supported OTIO/XML/EDL subset with loss reports | Round-trip tracks, transitions, audio and unsupported features |
| I03 | Native third-party project compatibility | 2/2 | Import an agreed native project subset where legally supported | Version matrix and transparent handling of unsupported constructs |
| P01 | Caching and responsive inspection | 2/2 | Cache probes/proxies/previews by content identity | Eviction, invalidation and measured cold/warm latency |
| P02 | Hardware acceleration | 2/2 | One accelerated decode/render path | Device fallback, numerical tolerances and measured throughput |
| P03 | Large projects and long media | 2/2 | Bounded memory and usable 1000-clip operation latency | Long-form 4K fixtures, stress failures and performance regression limits |

## Agent interface (separate score)

**10/10 foundational checks verified.** These do not add points to editing coverage.

- [x] Local typed command interface
- [x] Capability discovery
- [x] Atomic snapshot edits and revision guards
- [x] Explicit local file boundaries
- [x] Inspectable render plans
- [x] Durable project store
- [x] Durable idempotent retries
- [x] Persistent undo/history
- [x] Asynchronous job control — Durable local queue; status/phase and frame progress; queued and running cancellation; no output overwrites; worker interruption detection. Windows backend, one render per root.
- [x] MCP stdio adapter — Version negotiation, discoverable typed tools, structured results/errors, shared saved-session semantics, clean stdio framing/EOF and official Python SDK interoperability; no listening service.

## Updating this tracker

1. Implement a defined checkpoint and add meaningful acceptance coverage.
2. Add passing check IDs to `progress/capabilities.json` only when the checkpoint's stated behavior is actually covered.
3. Run `python tools/verify.py`; it runs formatting, lint, unit/crash tests, session, MCP/job and render integrations, repository checks, and regenerates this file.
4. Record a change in `docs/PROGRESS_HISTORY.md`. Change the baseline ID and log old/new scope if exclusions or weights change; never silently shrink the denominator.

The generator checks source fingerprints and evidence IDs. Human review is still required to judge whether a test demonstrates the full checkpoint. Evidence is in [verification/latest.json](../verification/latest.json); criteria are in [progress/capabilities.json](../progress/capabilities.json).

## Scope references

The original acceptance criteria are in [progress/capabilities.json](../progress/capabilities.json). Product-specific research and source mappings remain private under the [research boundary](RESEARCH.md); they do not establish engine compatibility or add implementation points.
