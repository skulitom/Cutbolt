# Cutbolt

Cutbolt is a local video editing engine for AI agents. Turn recorded footage into a finished edit: arrange shots, tighten dialogue, add captions and graphics, mix sound, and export. Every saved edit has a revision, so an agent can inspect changes, retry a request or undo a decision.

Use it through JSON commands, a Rust library or MCP over stdio. The engine runs on your machine, preserves source media, and needs no account or hosted service.

- **Who it's for:** people who turn recorded footage (interviews, talking heads, tutorials, screen recordings and presentations) into finished videos and want Claude Code or another MCP agent to do the editing on their own machine.
- **Use it when** an agent should cut, caption, mix and export real footage with exact timing, and you want to inspect, retry or undo each change it makes.
- **Don't use it when** you want to edit by hand in a visual timeline (Cutbolt is driven by an agent, JSON commands or a Rust library), need a hosted or cloud service, want new video generated from a text prompt (it edits footage you already have), or need formats beyond its documented limits (see [current scope](#current-scope)).
- **Compared with** Kinocut (formerly mcp-video), claudeclip, video-edit-mcp, Video Jungle MCP, Vlexor and HyperFrames in [Local video editing MCP servers for AI agents, and hosted alternatives: 7 compared](https://skulitom.github.io/Cutbolt/compare/local-video-editing-mcp-servers/) (checked 17 October 2026): Cutbolt measured over MCP (frame-exact cuts, safe retries, undo, byte-identical repeat exports on one FFmpeg build), the others read from their code and docs, and when to choose each instead.

## What you can make

- **Interviews and talking heads:** cut by time or transcript, shorten pauses, switch between synchronized cameras, and add timed captions.
- **Tutorials and recorded presentations:** combine screen recordings, camera footage, titles and overlays; preview a frame or an exact section before exporting.
- **Layered edits:** arrange video and audio tracks, reuse nested sequences, and save transforms, masks, color adjustments and transitions with the project.
- **Finished deliveries:** mix and measure audio, export a lossless master or H.264 video, review the result, and hand off a supported timeline through OTIO.
- **Repeatable workflows:** keep revisions, request receipts and undo history; resume local render jobs and revise individual parts of a production.

Eligible H.264 and HEVC footage can be edited directly. Other supported sources prepare automatically during queued export. Titles, graphics and captions can remain editable scene recipes on the timeline. Optional speech recognition and media workers run locally with separately installed dependencies.

## Get started

Cutbolt is in development. Windows and Linux are the current verification platforms. Every install needs FFmpeg and ffprobe on PATH, or `CUTBOLT_FFMPEG` and `CUTBOLT_FFPROBE` set to them; Cutbolt bundles neither.

**Prebuilt (early release):** the [latest release](https://github.com/skulitom/Cutbolt/releases/latest) has the CLI for Windows x64, Linux x64 and macOS ARM64 (macOS is not a verification platform: background jobs, which MCP exports go through, need Windows or Linux, and Windows-only engine features stay Windows-only), and `cutbolt-windows-x64.mcpb`, an MCP bundle for Claude Desktop on Windows. Check downloads against `SHA256SUMS`. Cutbolt is in the official MCP Registry as `io.github.skulitom/cutbolt`.

**From source,** with Rust and a C build toolchain, in this repository's checkout:

```text
cargo install --path . --locked
cutbolt --version
cutbolt doctor
```

Create a separate workspace for your footage and outputs, then configure your agent's MCP client to launch:

```text
cutbolt --workspace /absolute/path/to/workspace mcp
```

Replace the path with your workspace directory; on Windows, use a path such as `C:\Videos\MyProject`. The default MCP catalog is compact, with detailed commands available on demand.

See [installation and client setup](docs/INSTALLATION.md) for prerequisites, configuration and the optional Claude Code plugin. The plugin includes recipes for recuts, tutorials, multicam editing, captions and delivery. Other agents can start with [llms.txt](llms.txt).

To add the plugin to Claude Code from GitHub, once the CLI and FFmpeg are installed:

```text
claude plugin marketplace add skulitom/Cutbolt
claude plugin install cutbolt@cutbolt-local
```

Then restart Claude Code. The plugin starts `cutbolt` from PATH and installs nothing itself.

Once connected, try a request such as:

> Inspect the footage in this workspace, trim the first clip to its opening ten seconds, and show me a preview. Save the edit so I can revise it later.

For direct CLI use, [the usage guide](docs/USAGE.md) describes JSON requests and saved projects. [The agent interface](docs/AGENT_INTERFACE.md) covers revisions, retries, jobs and workspace boundaries.

## Current scope

Version 0.4.0 is an actively developed engine. Supported formats and operations have explicit limits; the timeline uses 8-bit SDR processing by default. See [direct footage](docs/DIRECT_SOURCES.md), [clip effects](docs/CLIP_EFFECTS.md), [generated clips](docs/GENERATED_CLIPS.md) and [export formats](docs/EXPORT.md) for the current contracts.

Verification checks decoded pictures, audio samples, timing and saved-edit behavior against independent expectations. [Published evidence](docs/PROGRESS.md) records the last full acceptance run; [progress history](docs/PROGRESS_HISTORY.md) records subsequent changes, targeted checks and open work. [Agent workflow measurements](docs/AGENT_COSTS.md) and [footage benchmark results](docs/FOOTAGE_BENCHMARK_RESULTS.md) describe the tested tasks and their limits.

## Development and license

See the [development workflow](docs/DEVELOPMENT_LOOP.md), [architecture](docs/ARCHITECTURE.md) and [1.0 roadmap](docs/ROADMAP_1_0.md) for implementation and planning details.

Original project code is licensed under [MIT](LICENSE). External dependencies retain their own licenses; tested versions and requirements are in the [dependency ledger](docs/DEPENDENCIES.md). Repository material and attribution rules are documented in [research and publication boundaries](docs/RESEARCH.md).

## Sibling tools

Cutbolt is one of five free, MIT-licensed tools for agents by the same author. Each runs locally as an MCP server:

- [Anode](https://github.com/skulitom/Anode): Windows desktop automation in a hidden background session, so an agent can click, type and take screenshots without taking over your screen.
- [Pixelbolt](https://github.com/skulitom/Pixelbolt): pixel art and sprite animation from editable JSON recipes, exported as sprite sheets, atlases, animated PNGs and GIFs.
- [Inkbolt](https://github.com/skulitom/Inkbolt): vector and raster image editing for diagrams and illustrations, with SVG, PNG, JPEG and TIFF import and SVG and PDF export.
- [Nestbolt](https://github.com/skulitom/Nestbolt): 2D nesting of parts on sheets for laser, CNC, plasma and waterjet cutting, also [in your browser](https://skulitom.github.io/Nestbolt/).
