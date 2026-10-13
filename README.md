# Cutbolt

Cutbolt is a local video editing engine for AI agents. Turn recorded footage into a finished edit: arrange shots, tighten dialogue, add captions and graphics, mix sound, and export. Every saved edit has a revision, so an agent can inspect changes, retry a request or undo a decision.

Use it through JSON commands, a Rust library or MCP over stdio. The engine runs on your machine, preserves source media, and needs no account or hosted service.

## What you can make

- **Interviews and talking heads:** cut by time or transcript, shorten pauses, switch between synchronized cameras, and add timed captions.
- **Tutorials and recorded presentations:** combine screen recordings, camera footage, titles and overlays; preview a frame or an exact section before exporting.
- **Layered edits:** arrange video and audio tracks, reuse nested sequences, and save transforms, masks, color adjustments and transitions with the project.
- **Finished deliveries:** mix and measure audio, export a lossless master or H.264 video, review the result, and hand off a supported timeline through OTIO.
- **Repeatable workflows:** keep revisions, request receipts and undo history; resume local render jobs and revise individual parts of a production.

Eligible H.264 and HEVC footage can be edited directly. Other supported sources prepare automatically during queued export. Titles, graphics and captions can remain editable scene recipes on the timeline. Optional speech recognition and media workers run locally with separately installed dependencies.

## Get started

Cutbolt is in development. Install from source with Rust, a C build toolchain, and FFmpeg/ffprobe available on PATH. Windows and Linux are the current verification platforms; prebuilt releases are still in preparation.

From this repository's checkout:

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

Once connected, try a request such as:

> Inspect the footage in this workspace, trim the first clip to its opening ten seconds, and show me a preview. Save the edit so I can revise it later.

For direct CLI use, [the usage guide](docs/USAGE.md) describes JSON requests and saved projects. [The agent interface](docs/AGENT_INTERFACE.md) covers revisions, retries, jobs and workspace boundaries.

## Current scope

Version 0.4.0 is an actively developed engine. Supported formats and operations have explicit limits; the timeline uses 8-bit SDR processing by default. See [direct footage](docs/DIRECT_SOURCES.md), [clip effects](docs/CLIP_EFFECTS.md), [generated clips](docs/GENERATED_CLIPS.md) and [export formats](docs/EXPORT.md) for the current contracts.

Verification checks decoded pictures, audio samples, timing and saved-edit behavior against independent expectations. [Published evidence](docs/PROGRESS.md) records the last full acceptance run; [progress history](docs/PROGRESS_HISTORY.md) records subsequent changes, targeted checks and open work. [Agent workflow measurements](docs/AGENT_COSTS.md) and [footage benchmark results](docs/FOOTAGE_BENCHMARK_RESULTS.md) describe the tested tasks and their limits.

## Development and license

See the [development workflow](docs/DEVELOPMENT_LOOP.md), [architecture](docs/ARCHITECTURE.md) and [1.0 roadmap](docs/ROADMAP_1_0.md) for implementation and planning details.

Original project code is licensed under [MIT](LICENSE). External dependencies retain their own licenses; tested versions and requirements are in the [dependency ledger](docs/DEPENDENCIES.md). Repository material and attribution rules are documented in [research and publication boundaries](docs/RESEARCH.md).
