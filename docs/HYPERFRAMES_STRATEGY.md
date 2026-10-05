# Winning agents over HyperFrames: strategy report

Research date: 5 October 2026. This is a strategy recommendation for agents and maintainers. It changes no scope, acceptance criteria or points. Read it before working on distribution, agent skills, the MCP surface, graphics authoring or positioning. [COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md) covers the wider field.

HyperFrames facts come from its public repository at commit [`9c7ff590`](https://github.com/heygen-com/hyperframes/tree/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b) (5 October 2026), its documentation site, npm and public issue pages. Nothing was installed or run, and no HyperFrames code, skills, docs or assets entered this repository. HyperFrames is Apache-2.0; do not copy its text or code into Cutbolt. Write original material. Cutbolt measurements were taken on Linux x86_64 (4 cores, FFmpeg 6.1.1, debug build of commit `d3c9140`) unless stated.

## Decision summary

1. **Do not fight HyperFrames on its home ground.** It generates new motion-graphics videos from HTML. It has 57k stars, about 813k npm downloads a week, 21 agent skills, 394 catalog blocks and a funded team shipping several releases a day. Cutbolt cannot win a race to be "HTML video".
2. **Own the lane HyperFrames has declined: editing real footage.** HyperFrames' own docs say its recut and caption workflows leave footage untouched, its timeline has no ripple/roll/slip/slide, and a request for them was closed as not planned. Several of its workflow routes cap output at about three minutes. Cutbolt already has exact NLE operations, transactional sessions, undo, interchange, loudness and review checks. Position Cutbolt as **the local, exact, undoable editor agents use on real footage, and the finishing stage that rendered graphics drop into.**
3. **Our biggest gap is distribution, not engine features.** An agent cannot find Cutbolt, cannot install it in one command, and on Linux or macOS cannot render through MCP at all (measured below). HyperFrames installs as a Claude Code plugin in two commands. Fix this first.
4. **Make Cutbolt cheap for a model to use.** Our default MCP catalog costs about 65k tokens before the first call. Scene JSON is unfamiliar to models and verbose. HyperFrames' central bet is a format models already know. We need small skills, a small default catalog and a terse authoring layer.
5. **Prove it with a public benchmark.** Run the same agent on the same footage tasks with both tools. Claim superiority only where results show it.

## At a glance

| Dimension | HyperFrames (v0.8.133) | Cutbolt (v0.4.0) |
| --- | --- | --- |
| What it makes | New videos authored as HTML/CSS/JS, animated with GSAP and similar runtimes | Edits of existing media on an exact timeline, plus JSON scenes for titles and captions |
| License | Apache-2.0; its primary animation library, GSAP, is under a non-OSI licence ([CREDITS.md](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/CREDITS.md)) | MIT |
| Runtime stack | Node 22+, downloaded headless Chrome, Puppeteer, FFmpeg, sharp/libvips | One Rust binary plus FFmpeg/ffprobe |
| Time model | Decimal seconds; frame time is `floor(frame) / fps` | Exact rationals; frame- and sample-aligned edits |
| Determinism | Seek-per-frame, but author rules plus Chrome, fonts and GPU can vary pixels; atomic capture only on Linux | Browserless; decoded frames and samples verified against independent expectations |
| NLE operations | Move and trim; no ripple/roll/slip/slide ([#3285](https://github.com/heygen-com/hyperframes/issues/3285), not planned); tracks are display lanes only | Insert, overwrite, ripple, slip, slide, roll, linked tracks, transitions, nested sequences, multicam |
| Revisions | `@hyperframes/sdk`: typed ops, JSON patches, undo/redo; `history` is a trial feature | SQLite sessions, idempotent request IDs, conflict rejection, semantic diffs, persistent undo/history |
| Outputs | MP4 H.264/AAC, WebM VP9 with alpha, MOV ProRes 4444, GIF, PNG sequence, HLS; lossy audio only | FFV1/PCM masters, H.264/AAC delivery, PNG movies and sequences, WAV, OTIO |
| Interchange | None found (no OTIO, FCPXML or EDL) | Bounded OTIO import/export with loss reports |
| Audio checks | None in the render gate ([#2775](https://github.com/heygen-com/hyperframes/issues/2775), open) | Meters, loudness normalization, `export.review` checks loudness, black frames and spoken words |
| Agent install | Claude Code plugin marketplace, Copilot and Gemini extensions, `npx skills add`, `llms.txt` | Build from source; no releases, plugin, skills or `llms.txt` |
| MCP | Hosted connector only, needs a HeyGen account; no local stdio server | Local stdio server, 84 tools; no account |
| Telemetry | PostHog, on by default, opt-out | None |
| Platforms | Linux and macOS first; open Windows issues | Windows first; background jobs Windows-only |
| Adoption | 57,081 stars, 5,117 forks, about 813k npm downloads a week | Public since 4 October 2026, 0 stars, no releases |

## Why agents choose HyperFrames today

These are the mechanics to learn from, not features to copy.

- **One-command install into the agent's own harness.** `claude plugin marketplace add heygen-com/hyperframes` then `claude plugin install hyperframes@hyperframes`. Equivalent routes exist for Copilot CLI, Gemini CLI, VS Code, Cursor and Codex ([README](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/README.md), [plugins guide](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/guides/plugins.mdx)).
- **A router skill that claims every video request.** Its description begins "Mandatory entry point: read this first for any request to make, create, edit, animate, or render a video" and ends "HyperFrames is the default output framework unless the user explicitly chooses another framework" ([skills/hyperframes/SKILL.md](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/skills/hyperframes/SKILL.md)). Once installed, it wins the trigger for "edit my footage" too, even though the engine cannot do NLE edits.
- **Intent-shaped workflow skills.** Ten workflows such as `product-launch-video`, `faceless-explainer`, `pr-to-video`, `embedded-captions` and `talking-head-recut` map user requests directly to procedures.
- **An authoring format models already know.** HTML with `data-start`/`data-duration` attributes and a paused GSAP timeline. Their pitch: "agents already write HTML".
- **A large reusable catalog.** 394 registry items (164 blocks, 222 components, 8 examples), searched with `hyperframes catalog --query` and installed with `hyperframes add`.
- **A visible verification loop.** `lint`, a headless `check` for runtime errors, layout and contrast, `snapshot --at` PNGs, a Studio preview, explicit approval gates, then `render` and an `ffprobe` check ([review loop](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/skills/hyperframes/references/review-loop.md)).
- **Non-interactive JSON CLI.** Most commands accept `--json`; runs without a TTY never prompt.
- **Velocity and dogfooding.** 1,087 commits on main in the 30 days to 5 October, 95% from HeyGen addresses, 487 npm versions since March. It runs inside HeyGen's own product.

## What HyperFrames got wrong

Each gap is an opening only if Cutbolt actually covers it. The right column says what to build or show.

| Gap | Evidence | Cutbolt's answer |
| --- | --- | --- |
| Its router claims footage editing it cannot do | Router claims "edit" requests including "existing footage"; [editing guide](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/prompting/editing-existing-videos.mdx) says Studio "does not yet offer split, slip, slide, ripple, or roll"; captions keep "footage untouched (no NLE-style editing)" ([AGENTS.md](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/AGENTS.md)); [#3285](https://github.com/heygen-com/hyperframes/issues/3285) closed as not planned | Skills that trigger precisely on footage edits, with tested recipes for each NLE operation |
| Short-form ceiling | The explainer and PR routes have a "hard cap about 3 minutes", with a "30–90s" sweet spot; some capture paths write about 25 GB of raw frames per minute at 1080p30; [#4060](https://github.com/heygen-com/hyperframes/issues/4060) (Windows, renders over 240 s); [#5069](https://github.com/heygen-com/hyperframes/issues/5069) (223 MB HTML crashes V8) | Long-form: 30-minute 4K and 1,000-clip fixtures already pass on Windows; make them pass on Linux and publish timings |
| Determinism is an author obligation | "No `Date.now()`, no unseeded `Math.random()`" rules; engine docs: "Exact pixels can still vary with Chrome, fonts, codecs, GPU"; atomic BeginFrame capture only on Linux; one-frame-off issues [#4555](https://github.com/heygen-com/hyperframes/issues/4555), [#4559](https://github.com/heygen-com/hyperframes/issues/4559), [#4435](https://github.com/heygen-com/hyperframes/issues/4435); Docker colour shift [#4899](https://github.com/heygen-com/hyperframes/issues/4899) | Exact rational time, browserless rendering and decoded-output verification; say so with receipts, not adjectives |
| No audio validation | [#2775](https://github.com/heygen-com/hyperframes/issues/2775): silent narration "passes every check" because "there is no audio validation gate anywhere in the pipeline" | `export.review`, meters and loudness targets as a default gate in every delivery skill |
| Lossy-only, no interchange | No FFV1/PCM output; no OTIO, FCPXML or EDL; NLE handoff is a ProRes or PNG render | Lossless masters plus OTIO handoff to a human editor |
| Account and cloud coupling | MCP is a hosted beta connector needing a HeyGen account; cloud render, HeyGen voices and avatars need sign-in | Local stdio MCP and no account, already required by our exclusions |
| Telemetry and credential reads | PostHog telemetry on by default ([config.ts](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/packages/cli/src/telemetry/config.ts)), recording the calling agent's name; skills run `usage --json`, which reads Claude Code and Codex OAuth credentials to query the providers' usage endpoints ([harnessUsage.ts](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/packages/cli/src/utils/harnessUsage.ts)). Opt-out exists and IPs are not recorded | State plainly: no telemetry, no network at runtime, no credential access. This matters to enterprise and privacy-sensitive users |
| Skill sprawl and contradictions | 330 Markdown files, about 400k words; [#5025](https://github.com/heygen-com/hyperframes/issues/5025) reports "10 cross-skill contradictions and 11 wrong facts or broken paths" | Few, short skills whose every command example runs in CI |
| Heavy, fast-churning stack | Node 22, Chrome download, sharp, FFmpeg, optional Python models; pre-1.0 with 34 npm versions between 1 and 5 October | One binary, versioned schemas and explicit migrations; promise a stable contract |

## What HyperFrames got right that we lack

- Agents can install it in one or two commands in the harness they already use.
- It has intent-level skills; we expose about 100 low-level commands.
- Models can author it without reading a schema.
- It has a catalog large enough that agents reuse rather than invent.
- It tells the agent exactly how to look at its work before rendering.
- It publishes `llms.txt`, a docs site and a quickstart prompt.
- Its `@hyperframes/sdk`, `timeline --json` and `history` commands are converging on our session model. Our lead is depth (exact NLE operations, verification, interchange), not the idea of transactional edits. Assume they will close part of this gap.

## What we got wrong

1. **MCP agents cannot finish a job on Linux or macOS.** `media.prepare`, `scene.render` and `export.run` are reachable over MCP only through `job.start`. On Linux, `job.start` returns `UNSUPPORTED_PLATFORM: Background jobs currently require Windows`. The same commands work through the blocking CLI. Most cloud coding agents run on Linux, so an MCP-only agent there can inspect and edit but cannot prepare media or deliver a file.
2. **No distribution.** No GitHub releases, no prebuilt binaries, `publish = false` in Cargo.toml, no plugin manifest, no skills, no `llms.txt`, no repository topics. A debug build takes 1 min 41 s here and needs a Rust toolchain. An agent asked to "edit this video" will never discover Cutbolt.
3. **Windows-first everything.** README and docs examples use PowerShell and `C:\DEV\...` paths. The README ends with notes about one maintainer machine's directory junction. Real-time recording is Windows-only. This tells Linux and macOS users the tool is not for them.
4. **Expensive context.** The default MCP listing is 84 tools and 260,655 bytes, about 65k tokens. `--tools core` is 32 tools and 76,268 bytes, about 19k tokens. On 4 October, three fresh agents using only MCP all fell back to CLI-only commands, and 60–90% of their response bytes went to schema and capability lookups ([PROGRESS_HISTORY.md](PROGRESS_HISTORY.md), "agent usability fixes from MCP-only trials"). Much was fixed, but the default is still the full catalog.
5. **Authoring that models do not know.** The original title-card template is 6,420 bytes of JSON at 160x90 pixels with `output_scale: 4`. Fonts and images need content identities. Every agent must learn our schema from `cutbolt_schema` before making a title.
6. **Friction on ordinary footage.** Timelines accept only FFV1/PCM, so every phone or camera file is converted first. A 4-second 720p H.264 clip (1.4 MB) became a 14 MB FFV1 asset and took 26 s in the debug build. The conversion is correct and cached, but agents must plan around it.
7. **Checklist completeness ahead of first-run success.** The README headline is "100/100 core and 108/108 total, with 466 passing checks". The next day's narrated demo recorded 25 problems, including an 80-second export that never finished, half-tempo beat detection and a ten-second scene cap ([PROGRESS_HISTORY.md](PROGRESS_HISTORY.md), 5 October entries). Those were fixed quickly, which is good, but they show the checklist does not measure what agents experience. The headline also invites a reader to assume completeness.
8. **Docs drift from the engine.** `cutbolt capabilities` says placed tracks run at eight frame rates and that scenes, captions and H.264 delivery follow the timeline rate. The README, [USAGE.md](USAGE.md) ("Placed tracks retain 25 fps") and [NATIVE_TIMING.md](NATIVE_TIMING.md) still say 25 fps only. Agents trust docs literally; contradictions cost them calls. Also observed: a workspace `media.prepare` returned absolute output paths, while [AGENT_INTERFACE.md](AGENT_INTERFACE.md) says paths inside the workspace come back relative.
9. **Documentation written for verifiers, not users.** 60 documents, long qualified sentences, and no "make your first edit in five minutes" page. HyperFrames leads with a prompt you can paste.
10. **No proof of the USP.** [COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md) proposed a cross-tool task benchmark on 2 October. It has not been run, so we cannot yet say Cutbolt is better at anything an agent cares about.

## Direction

**Positioning statement:** Cutbolt is the local engine agents use to edit real footage exactly, revise it safely and deliver it with proof. Graphics made anywhere, including HTML renderers, drop into it as overlay tracks.

**Pick the fights we can win:**

| Request shape | Better tool today | Why |
| --- | --- | --- |
| New promo, explainer or PR video from a brief, URL or Figma | HyperFrames | HTML authoring, catalog, shader transitions, TTS and avatars |
| Tighten a talking head, remove pauses and fillers, add captions | Cutbolt | Word-level transcript cuts, ripple edits, `audio.tighten`, `transcript.fillers`, caption drafting |
| Podcast or interview with several cameras | Cutbolt | Multicam groups, sync inspection, linked audio |
| Screen-recording tutorial, 10–60 minutes | Cutbolt | Long-form limits, exact ranges, proxies, range exports |
| Revise one section after feedback without disturbing the rest | Cutbolt | Sessions, semantic diffs, undo, idempotent retries |
| Hand the edit to a human editor | Cutbolt | OTIO export, lossless masters |
| Offline or private work, no telemetry | Cutbolt | Local only, no account, no network |
| Titles and lower thirds over footage | Both | HyperFrames renders graphics with alpha; Cutbolt composites them on an `alpha_over` track |

**Coexist where we cannot win.** Agents will keep HyperFrames installed. Make "render graphics anywhere, finish in Cutbolt" a tested, documented path. Do not ask the agent to choose one tool for the whole job.

**Do not:**

- Clone HTML/GSAP rendering into the engine core or race on catalog size.
- Add telemetry, accounts, a hosted MCP or any listening service. These remain excluded by [AGENTS.md](../AGENTS.md).
- Write an over-claiming router skill. Claim only what a test proves. A skill that fails on its own trigger trains agents to avoid the tool.
- Copy HyperFrames skills, docs, registry items or code. Write original material and keep [RESEARCH.md](RESEARCH.md) boundaries.
- Count any of this work as editing coverage points. Distribution and skills are agent-interface work.

## Roadmap

Each item lists what done means. Award no capability points for these; record finished items in [PROGRESS_HISTORY.md](PROGRESS_HISTORY.md).

```mermaid
flowchart LR
    subgraph P0["P0: install and finish anywhere"]
        A1[POSIX job backend] --> A2[Release binaries and doctor]
        A2 --> A3[Plugin and skills]
        A4[Small default catalog]
        A5[Docs: quickstart, llms.txt, drift fixes]
    end
    subgraph P1["P1: win footage workflows"]
        B1[Implicit ingest]
        B2[Five flagship workflows]
        B3[Public agent benchmark]
        B4[Scene shorthand and templates]
        B5[External graphics overlays]
    end
    subgraph P2["P2: own finishing"]
        C1[Optional HTML layer companion]
        C2[Stable 1.0 contract]
        C3[Library embedding]
    end
    P0 -->|"gate: MCP-only agent edits and exports on Linux, macOS, Windows"| P1
    P1 -->|"gate: benchmark shows a footage-task lead"| P2
```

### P0: an agent can install Cutbolt and finish a job on any platform

| ID | Work | Done means |
| --- | --- | --- |
| A1 | Background jobs on Linux and macOS: process groups, `flock` on `worker.lock`, signal-based cancellation with a grace period, the same watchdog, progress and recovery rules as Windows | `queue_recovery` and the MCP job fixtures pass on Linux; an MCP-only agent on Linux prepares an MP4, edits, exports H.264 and cancels a second export |
| A2 | Tagged GitHub releases with binaries for linux-x64, linux-arm64, macos-arm64 and windows-x64, plus checksums; document `cargo install --git ... --locked`; add `cutbolt doctor` reporting FFmpeg/ffprobe paths and versions, platform features and workspace state | On a clean Linux container, download to first MCP tool call in under two minutes |
| A3 | Original plugin packaging for the major harnesses whose formats are public: a Claude Code marketplace manifest registering the MCP server and skills, then Codex, Gemini and Copilot equivalents | One documented install command per harness, tested on at least Claude Code |
| A4 | Original skills, each under about 300 lines: a `cutbolt` router for footage editing, and workflows for recut, tutorial, multicam, captions and delivery. Every command example in a skill runs in CI against a fixture | A skill-lint check fails on a broken example; no two skills disagree on a command |
| A5 | Make `core` the default MCP catalog and target under 10k tokens for the listing; keep `--tools full` | A unit test bounds the default listing; MCP fixtures pass |
| A6 | Rewrite the README opening for outsiders: one paragraph, a quickstart for each OS, a paste-ready agent prompt. Move maintainer-machine notes and checklist scores lower. Fix the frame-rate drift and the absolute workspace path. Publish `llms.txt` | A fresh agent given only the README completes the quickstart without reading other docs |

Draft router description to adapt (original wording, test before shipping): "Use for editing existing video or audio: trimming, cutting pauses and filler words, ripple/roll/slip/slide edits, multicam, captions from transcripts, loudness, long recordings, revising an edit with undo, or exporting a master or an OTIO handoff. Works offline with no account. For generating a new animated video from scratch, an HTML video tool may suit better; Cutbolt can then composite its rendered graphics over footage."

### P1: win the footage workflows and prove it

| ID | Work | Done means |
| --- | --- | --- |
| B1 | Implicit ingest: `media.add` accepts any decodable file and prepares it on demand, cached by content identity, reporting conversion time and size | Agents add a phone MP4 to a timeline in one call; exactness and source preservation tests unchanged |
| B2 | Five flagship workflows end to end, each a skill plus a fixture: talking-head tighten with captions and a 9:16 reframe; two-camera interview; 20-minute screen tutorial with chapters; revise one section after feedback; deliver with review and OTIO handoff | Each passes on Linux and Windows from the skill alone |
| B3 | Public benchmark: about 10 tasks, 4 graphics-first and 6 footage-first, run with the same model and harness for Cutbolt and HyperFrames, 3 runs each. Measure completion without manual repair, unintended changes, A/V sync, tokens, tool calls and wall time. Publish raw results, losses included | Reproducible results committed; claims in docs cite them |
| B4 | Scene shorthand: a terse form that compiles to the existing scene schema (seconds as numbers, hex colours, defaults for transforms, named anchors such as lower-third, named easings). Grow original templates from 2 to about 20: lower thirds, caption styles, title and end cards, callouts, progress bars | The title card fits in under 1 KB; shorthand and expanded scenes render identically |
| B5 | External graphics overlays: a tested path for alpha graphics from any renderer (PNG sequences first, then ProRes 4444 and VP9-alpha conversion) onto `alpha_over` tracks, with frame-rate and duration checks | A 10-second overlay rendered by an external HTML tool lands frame-exact over footage |

### P2: own the finishing layer

| ID | Work | Gate |
| --- | --- | --- |
| C1 | Optional HTML layer through a separate local companion with pinned external dependencies, seek-per-frame capture and identity-bound PNG output, like the segmentation worker | Only if B3 shows graphics authoring is the main reason agents lose with Cutbolt |
| C2 | Stable 1.0 contract: frozen request schemas, semantic versioning, migration guarantees | After P1 workflows stop changing the schema weekly |
| C3 | Library embedding: publish the crate and consider language bindings | Demand from a real integrator |

## Measures to track

| Measure | Now | Target |
| --- | --- | --- |
| Clean Linux machine to first successful MCP export | Not possible over MCP | Under 5 minutes |
| Default MCP listing | About 65k tokens | Under 10k tokens |
| Platforms where MCP-only agents can export | Windows | Linux, macOS, Windows |
| Benchmark footage-task completion, Cutbolt vs HyperFrames | Not measured | Published, with a lead on footage tasks |
| Install commands per harness | None | One |

## Open questions for the maintainer

- Should P0 take priority over the PixelForge/Qwen pilot work in [YOUTUBE_PIPELINE.md](YOUTUBE_PIPELINE.md)? This report recommends yes, because the pilot cannot be reproduced by outside agents until P0 lands.
- Is shipping prebuilt binaries acceptable given the FFmpeg licensing review in [RESEARCH.md](RESEARCH.md)? Binaries would not bundle FFmpeg; users install it separately as today.
- Which harnesses matter most after Claude Code: Codex, Gemini CLI, Copilot or Cursor?

## Sources

Opened 5 October 2026. Stars, forks and download counts change daily.

- [HyperFrames repository](https://github.com/heygen-com/hyperframes) and files at commit `9c7ff590`: [README](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/README.md), [AGENTS.md](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/AGENTS.md), [CREDITS.md](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/CREDITS.md), [router skill](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/skills/hyperframes/SKILL.md), [review loop](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/skills/hyperframes/references/review-loop.md), [editing existing videos](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/prompting/editing-existing-videos.mdx), [determinism](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/concepts/determinism.mdx), [engine](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/packages/engine.mdx), [producer](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/packages/producer.mdx), [CLI](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/packages/cli.mdx), [SDK](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/packages/sdk.mdx), [MCP guide](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/guides/mcp.mdx), [plugins guide](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/guides/plugins.mdx), [vs Remotion](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/docs/guides/hyperframes-vs-remotion.mdx), [telemetry config](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/packages/cli/src/telemetry/config.ts), [harness usage](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/packages/cli/src/utils/harnessUsage.ts), [registry](https://github.com/heygen-com/hyperframes/blob/9c7ff590fe9e1f3fa7bf06cb3bf67ae78c2a993b/registry/registry.json).
- HyperFrames issues: [#2107](https://github.com/heygen-com/hyperframes/issues/2107), [#2775](https://github.com/heygen-com/hyperframes/issues/2775), [#3285](https://github.com/heygen-com/hyperframes/issues/3285), [#4060](https://github.com/heygen-com/hyperframes/issues/4060), [#4435](https://github.com/heygen-com/hyperframes/issues/4435), [#4555](https://github.com/heygen-com/hyperframes/issues/4555), [#4559](https://github.com/heygen-com/hyperframes/issues/4559), [#4899](https://github.com/heygen-com/hyperframes/issues/4899), [#5025](https://github.com/heygen-com/hyperframes/issues/5025), [#5069](https://github.com/heygen-com/hyperframes/issues/5069).
- [npm registry entry](https://registry.npmjs.org/hyperframes) and [weekly downloads](https://api.npmjs.org/downloads/point/last-week/hyperframes).
- [HyperFrames docs llms.txt](https://hyperframes.heygen.com/llms.txt), [HeyGen research post on HTML-to-video](https://www.heygen.com/research/html-to-video), [Show HN thread](https://news.ycombinator.com/item?id=47902856).
- Cutbolt: [PROGRESS_HISTORY.md](PROGRESS_HISTORY.md), [AGENT_INTERFACE.md](AGENT_INTERFACE.md), [USAGE.md](USAGE.md), [NATIVE_TIMING.md](NATIVE_TIMING.md), [COMPETITIVE_ANALYSIS.md](COMPETITIVE_ANALYSIS.md), and measurements in this container listed at the top.
