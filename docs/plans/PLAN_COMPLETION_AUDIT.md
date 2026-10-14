# Direct footage and agent cost: acceptance audit

14 October 2026. Requirements are taken from the two dated plans, including the
user's approved audio-cache and identity-cache amendments and peak-context token
metric. This audit checks implementation and assertions, not just earlier stage
labels. It changes no scope, release gate or editing points.

Status: every implementation stage and acceptance requirement in both plans is
verified. The final strict thorough milestone at `5d91c46` passes all 86 fixtures,
with no failures, skips, reused passes or deferred checks. Formatting, lint and
Rust tests pass; source fingerprints match the tested checkout. The completed
placed-footage comparison retains its slower preview medians and passing declared
ceiling. No scope or points definition changes accompany this completion.

Earlier evidence files describe their own checkpoints. A later complete gate can
close an earlier open regression check without changing that historical record.
Linux checkpoint records identify the copied source hashes as well as the older
checkout metadata; they do not claim the Git label alone identifies the tested code.

## Direct footage

| Requirement | Implementation and acceptance evidence |
| --- | --- |
| 0.1–0.3: isolated work, Windows/Linux, impact map and retained release | Work uses the isolated `claude/footage-agent-cost` checkout and ships to main. The stage-0 history records the rebuilt map at c4064df, 59 passing fixtures and 2,969 executed functions. Original and later comparison binaries remain external with content identities. All new decode/precision paths have Windows and Linux targeted checks. |
| 0.4: original source corpus | `tests/footage_sources.py` records synthetic patterns and encoder settings, including native clocks, edit lists, B/open GOPs, HEVC Main/Main10, ProRes, VFR, rotation, sample aspect and an external on-demand 20-minute 1080p30 source. Tool versions and licensing remain in DEPENDENCIES; no media is committed. |
| 0.5: native single-frame preview duration | `preview.rs` uses the project clock; `native_timing` exercises the native-rate single-clip path. |
| 1.1: cached presentation index | `source_index.rs` incrementally reads presented timestamps, key/picture/interlace metadata and native format/color/geometry; decoded audio supplies samples/start offset. Compressed entries bind source, both tools, producer and schema, with payload/dimension limits. `direct_sources` independently checks frames/audio and counted cold/warm launches. |
| 1.1 audio amendment | The approved bounded canonical PCM cache addresses the measured AAC seek mismatch. Direct video still reads original YUV; audio caches and temporary render PCM preserve source identity and are fully verified. DIRECT_SOURCES records the counterexample, cache limits and exact resampling/padding policy. |
| 1.2–1.3: eligibility, gate and clocks | The separate indexed profile enforces H.264/HEVC Main 8-bit 4:2:0, declared SDR color, progressive square unrotated pictures, native CFR clocks and supported mono/stereo audio. Nonqualifiers carry named reasons; FFV1 checks remain separate. The 21 qualifying types match explicit conform, including nonzero starts, priming, padding/trimming, mono and resampling. Mixed native size/rate is rejected unless explicitly normalized. |
| 1.4: exact seek and conversion | `direct.rs` seeks from indexed keyframes, with an additional GOP for B-picture streams; `direct/picture.rs` checks native-plane frame counts. `color/lookup.rs` exhaustively compares all 2^24 YUV triples for every supported range/transfer combination with the f64 reference. `direct_decode`, `direct_composition` and `direct_timeline` are exercised by `direct_sources`. |
| 1.5: saved identity-bound assets | `media.add` saves the original plus a derived-index binding; `media.index` repairs only an unchanged original. `timeline.check` diagnoses stale/missing indexes. `media.prepare` reports ready/direct without writing a conversion. Binding checks cover diffs, undo, request replay, changed sources and rollback. This optional field does not upgrade snapshot schema. |
| 1.6: identity cost | Approved process-local metadata reuse checks path, size, modification time and file ID. Full content hashing still gates publication and hash receipts. Unit tests document the same-metadata tradeoff and full-hash detection; workflow fixtures reject changed sources before publication. |
| 1.7: complete pictures and sound | First/key/B/last previews, sequential and placed edits, mid-GOP windows, linked slip/roll, reference/range/H.264 delivery and queued retries match explicit conform, with exact RGB/PCM and preserved input hashes. The original stage-1 thorough milestone was 338e114. |
| 1.7: long source and compositor risk | The first recorded stage-1 long-source result and its baseline remain in DIRECT_SOURCES. The final [placed-footage comparison](../PLACED_FOOTAGE_PERFORMANCE.md) measures f13453d versus 04255ee over forty alternating observations. All twenty previews and twenty lossless exports equal explicit conform; both final-build H.264 pairs agree. Every request uses one late-seeking original video decoder. All current plain previews meet the ten-second ceiling. Preview medians are 16.7% and 7.9% slower; lossless export medians are 0.02% and 1.02% slower. Load/ranges and the default-path, short-range scope are retained; no general speed win is claimed. |
| 1.7: repeatable H.264 comparison | `export_repeatability` runs 48 exports/1,440 frames per platform over direct/conformed sources and four profiles. Export profile 4 sets x264's CPU-independent mode after standalone identical-input experiments reproduced decoded-picture differences. RGB/PCM equality is still exact. Actual older profile-3 job receipts replay unchanged. See export-repeatability-2026-10-14.json. |
| 2.1: broader explicit normalization | `color`, `conform` and geometry cases independently check 10-bit 4:2:0, 4:2:2, ProRes straight alpha, exact quarter-turn rotation and sample-aspect fitting. Decimal references bound RGB error; PCM/alpha and declared geometry remain exact. VFR uses the explicit frame map. |
| 2.2: verified conversion cache | `conform_cache` tests source/recipe/tool/LUT invalidation, encoded and decoded tampering, atomic concurrent publication, reader leases, LRU pruning and a hard byte ceiling. Entries are content-addressed and never replace originals. |
| 2.3–2.5: automatic preparation/provenance/cap | `automatic_prepare` takes VFR, rotated footage, HEVC10 and ProRes from media.add to H.264 without a manual prepare call, matching explicit normalization. Counted cold/warm conversions are 1/0. Missing synchronous caches return runnable repair requests; queued paths expose preparation progress, cancellation and cleanup. Saved recipes retain original provenance and prepared results report conversion identities. Inspection reports expected bytes and rejects the configured ceiling. |
| Stage 2 exit | Targeted Windows/Linux evidence plus the complete 76-fixture f13453d gate close the stage. The earlier automatic-preparation checkpoint remains accurately labelled as partial. |
| 3.1–3.2: saved source-clock effects | Grade, selective grade, LUT and key use shared effect types and a maximum eight pixel effects. `clip_effects` edits replace a diffable stack. Store tests cover schema-2 upgrades of new revisions only, history, retry, undo and rejection on audio. Curves follow source time across trims, slips and splits. Later nested support supersedes the stage-3 temporary rejection. |
| 3.3–3.5: frame processing/oracle | `clip_effects` extracts source frames into independently constructed scene recipes and compares full RGB/PCM, animation, grade+LUT, preview, proxy, range/queued H.264 delivery and source/LUT publication checks. Effects evaluate per frame; long sources do not precompute a duration-sized table. Complete f13453d gate: 76 fixtures. |
| 4.1: structural properties and legacy sugar | `clip_properties`, called by `native_layers`, checks transform/opacity/mask/blend, source clocks, saved state, original/proxy coordinates, nesting, transitions, bounds and exact OverlayTransform equivalence. Its scene reference is backed by separate Fraction/integer calculations. |
| 4.2–4.4: ordered layering and nesting | The native stack composites over lower tracks and black, retains key alpha and skips provably hidden windows. PIP, keyed presenter and feathered reveal match scene and independent integer references. Nested frames use the same Rust path. Old private selectors/FFmpeg video graphs were removed only after both paths agreed on Windows/Linux. |
| Stage 4 exit | native-layers, nested-effects, clip-properties and native-only-timeline evidence plus the complete 77-fixture 73e2b2a gate establish correctness. The final placed-footage measurement is recorded separately, including slower preview medians and nearly unchanged lossless export medians; see the long-source row. |
| Stage 5: scene clip source | Asset, child sequence and pinned inline/document scene are exclusive clip sources. `generated_clips` compares direct scene frames/PCM with compiled assets, including alpha, nested/proxy/queued use, clocks, retries, dependencies and resource bounds. It invokes `generated_resources`; `captions` invokes `generated_captions`. |
| Stage 5: captions and production | `graphics.place`, generated captions and production titles/captions now save scenes with no intermediate movie. Existing compiled defaults and actual older job/coordinator receipts remain compatible. Scene duration/layer/pixel bounds remain enforced. Complete 1b7f269 gate: 78 fixtures and 255 Rust tests. |
| Stage 6: saved working space/default | `project.working_space` accepts all four depth/light combinations; absent or explicit 8-bit encoded selection retains the old rendering path. Store checks cover schema, validation, receipts, retry, undo and restore. Wide scene/generated/recursive frames are retained until one final transfer/dither step. Normalization remains native RGB8; this does not recover already-quantized source detail. |
| Stage 6: independent numerical references | `working_color`, `working_effects`, `working_layers` and `working_scenes` use independent f64, Fraction and Decimal equations. Declared tolerances are one UNORM16 code per primitive/effect, two for complete scenes and explicitly bounded accumulated layers. `working_timelines` allows one delivered RGB8 code; its 27 cases/324 frames observed zero error on Windows/Linux. |
| Stage 6: grade/fade banding | `working_banding` independently computes transfer, grade, coverage, depth stores and Bayer thresholds. The 1,024-frame temporal ramp has 1,024 distinct means; the 256-level spatial gradient retains all levels through 33 fade frames. Reference bytes/tile means are exact, with no added plateaus. This is a numerical SDR precision result. |
| Stage 6: real workflow boundaries | `working_footage` adds 93 cases/1,044 exact frames per platform for original/direct/cached sources, effects/structural properties, native alpha, nested scenes, proxies, generated PCM, queues, caches and changed-source publication. The complete 85-fixture default-path quick gate at 26cb066 passes, including these cases. The final placed comparison passes correctness and its preview ceiling. The final strict thorough milestone at 5d91c46 passes all 86 fixtures, including all precise-mode workflows. |

The [stage-six quick record](../../verification/stage6-full-quick-2026-10-14.json)
identifies 26cb066 exactly: 85 fixtures, 261 Rust tests, no failures/skips/reuse,
2,864 seconds. It predates the profile-4 fix. The final
[thorough milestone](../../verification/final-plans-milestone-2026-10-14.json) at
`5d91c46` includes that fix and all 86 fixtures, with matching source identities
and refreshed generated progress. H.264 profile 4 can
change lossy output relative to earlier profiles; the default working-color path
and saved completed receipts remain unchanged.

Primary direct-footage evidence: [source contract](../DIRECT_SOURCES.md),
[automatic preparation](../AUTOMATIC_PREPARATION.md), [clip effects](../CLIP_EFFECTS.md),
[generated clips](../GENERATED_CLIPS.md), [working color](../WORKING_COLOR.md), and the
dated JSON records under [verification](../../verification).

The routine `direct_sources` fixture deliberately leaves `stage_one_complete` and
`long_source_latency_verified` false: it does not run the separate long-source
performance experiment. Stage-one closure uses that fixture together with the
retained stage-one milestone and the final placed-footage comparison above.

## Agent cost

| Requirement | Implementation and acceptance evidence |
| --- | --- |
| Stage 0: five measured SDK workflows | `agent_costs` uses official Python MCP 1.12.3 with original generated inputs and a supplied transcript. It counts raw initialize/list/request/response/notification/image bytes, failures and schema/capability calls. Its five workflows and independent titles/recovery budgets are verifier-enforced on Windows/Linux. Historical baseline and fresh-trial procedure are in AGENT_COSTS. |
| Stage 1: descriptions/docs/errors/router | Queued scene compilation is correctly advertised; workspace identities/roots, native clocks, progress and field paths are documented. Strategy numbers retain a dated correction. Template errors name parameters/types, tested without mutation. The router says to prefer direct tools and cannot route destructive operations; cancel/prune remain direct. |
| Stage 2.1: compact schemas/annotations | Compact listings alone omit dialect/default hints, use a 96-byte Time description, preserve required descriptions and cap optional descriptions to one clause/120 characters. SDK/schema and unit tests cover exact shapes and full lookups. Protocol defaults keep full/core effective annotation values equal; Python SDK 1.12.3 exposes omitted hints as None. Actual plugin trials accept the catalog; arbitrary clients' permission interfaces are not claimed. |
| Stage 2.1: measured core and budgets | Five core tools occur in at least two baseline workflows; twelve support recovery, plus one grouped router. Unit checks enforce <=40,000 bytes with/without a workspace and <=4 KiB per core tool. The latest targeted SDK replay lists 22,726 bytes with a workspace, 18 tools, within the unchanged bound. |
| Stage 2.2: default and compatibility | Bare mcp selects core; explicit flag/environment full mode and precedence are tested. Common fixture clients explicitly select full. At the catalog-only checkpoint full tool arrays matched f8d680d byte-for-byte. Subsequent new engine features add their documented schemas normally. Startup and public docs name the default change. |
| Stage 3: actionable error contract | `errors.rs` centrally classifies all 236 current codes; unit tests scan source codes and check the documentation table. Optional retry/path/hint preserve the envelope. Prefixes/job arguments, free output name, available IDs/current revision and mechanical repair details are covered. The replay removes code/message before recovering wrong width, stale revision and existing output. |
| Stage 4.1: original graphics | Twenty original templates render in `builtin_templates`, with typed parameters/defaults, project output defaults, all eight clocks, overrides, invalid-request atomicity and CLI/core/full parity. `templates.list` is read-only. `graphics.place` now saves a generated scene in one queued job, as the plan's conditional requires. |
| Stage 4.2: fonts | `fonts` checks OS-root discovery, identity, pagination, exact/full versus ambiguous family matching, containment, glyph rendering and queue-time identity pinning. Fonts are read in place and never bundled. Windows/Linux execution is verified; macOS directory handling is implemented but no macOS runtime claim is made. |
| Stage 4.3: explicit shorthand | `scene_short` independently constructs the expanded scene and compares RGB/alpha/PCM, stills and digests. It covers hex colors, defaults, anchors/easing, project canvas/clock, font names, templates and queued/generated placement. The compact title fits 1 KB. |
| Stage 4 exit | Latest tutorial replay creates its title in two calls and 775 sent bytes, below the unchanged two-call/1,500-byte limit, with no schema lookup. |
| Stage 5.1–5.2: executable intent skills | Six original skills remain below 300 lines and state results/recovery/platform limits. Current catalog contains 41 fenced JSON examples and 15 consistent command shapes. `skills` schema-validates and executes every example across eight cases; independent picture/PCM assertions cover direct, normalized, generated-caption and B-roll recipes. |
| Stage 5.3–5.4: installed plugin and honest router | The original local plugin/marketplace registers stdio plus six skills. Clean Windows profiles successfully install the documented plugin; Linux executes recipes. The router covers footage, captions, loudness and delivery and directs graphics-first authoring appropriately. Distribution binaries/doctor are coordinated separately. |
| Stage 5 fresh-agent exit | Plugin-only trial 08 talking head/tutorial and 09 handoff pass 19/17/21 independent output checks with 15/15/10 total calls and no errors/schema calls. Peak input is 32,180/29,677/24,022. Updated generated-caption talking head passes 20 checks in 15 calls and 33,441 peak input. User-approved <=40,000 metric is peak request context; cumulative cache-inclusive usage remains reported separately. |

The latest targeted scripted replay uses 15/6/10/8/8 calls, with zero normal-workflow
failures, schema or capabilities calls. Windows received bytes are
207,359/43,992/54,133/38,185/183,076; title creation is two calls/775 bytes. These
observations come from the 06fe784 schema-description gate, not a new autonomous
trial. Its three strict workflow fixtures pass with no skips/reuse/defer; MCP and
schema Rust filters pass six and seventeen tests respectively (not necessarily
distinct). [Focused evidence](../../verification/schema-description-2026-10-14.json)
records the tested source identity.
Fresh trials used Claude Code 2.1.280 and claude-opus-5-5[1m] with original synthetic
six-second footage and a supplied transcript. Stage zero measured bytes and MCP
calls, not provider tokens or skill loads. It cannot establish a token improvement.

See [agent cost measurements](../AGENT_COSTS.md), the
[stage-5 audit](../../verification/agent-cost-stage5-2026-10-13.json) and
[generated-caption trial](../../verification/generated-caption-skills-2026-10-13.json).

## Scope of the final claim

Both plans concern bounded local editing and agent workflows. Passing them does not
adopt the v2 points baseline, prove unrestricted codec/project compatibility, add HDR
timeline delivery, guarantee arbitrary font/client/platform support, or establish a
repeatable comparative lead. The separate [comparative pilot](../FOOTAGE_BENCHMARK_RESULTS.md)
reports its wins, ties, losses and limitations. Original code licensing, dependency
notices and source-preservation rules remain intact.

Closure evidence: the [final milestone](../../verification/final-plans-milestone-2026-10-14.json)
pins `5d91c46`, the observed test binary, all 86 fixture names, full-report/log hashes
and the source fingerprint set. The [complete evidence](../../verification/latest.json)
and [generated progress](../PROGRESS.md) come from that successful thorough run.
Recording passes once without a retry; no fixture failures remain in this milestone.
The v1 capability definition is unchanged. Earlier targeted Windows/Linux results,
fresh-agent trials and the separate performance comparison retain their stated scopes.
