# Local installation

Cutbolt is a local executable and Rust library. An MCP client starts the executable over stdio;
there is no server to deploy. Windows x86-64 and Linux x86-64 are the current verification
platforms. The source checkout includes an optional skills/MCP plugin; tagged binary releases
are still in preparation.

## Build and install from source

Install a Rust toolchain supporting edition 2024, a C compiler/linker for SQLite, and an external
FFmpeg/ffprobe build. Keep both media executables on PATH, or set `CUTBOLT_FFMPEG` and
`CUTBOLT_FFPROBE` to their absolute executable paths. Cutbolt does not download or bundle them.
See [DEPENDENCIES.md](DEPENDENCIES.md) for tested versions and licenses. Python and the optional
speech/model environments are development or optional workflow dependencies, not requirements
for the core executable.

From a checkout of this repository:

```text
cargo install --path . --locked
cutbolt --version
cutbolt doctor
```

Cargo prints the installation directory; add its `bin` directory to PATH if needed. Keep the
reported commit with your project records. `publish = false` prevents a crates.io publication;
it does not prevent installation from the local source tree.

## Check setup

`cutbolt doctor` reports the actual selected FFmpeg/ffprobe paths, versions, executable hashes,
required encoders/decoders/muxers, and the engine's compiled platform features. It executes
bounded diagnostic queries locally. A `ready: true` result confirms these setup checks;
rendering, optional speech/models, GPU availability and store integrity require their own tests.

To inspect an existing workspace, use `cutbolt --workspace DIR doctor`, or set
`CUTBOLT_WORKSPACE`. Doctor checks existing engine-state directories and their parent folders
for directory type and workspace containment. Missing state folders are acceptable; normal
workspace use creates them. Doctor creates no workspace, files or state directories, and does
not test write permissions or open the session database.

The JSON envelope is `{"ok":true,"result":{...}}` when diagnostics ran. Read `result.ready`
for setup status: exit code 0 means ready, and exit code 1 means at least one setup check failed.
Failed tool checks include an error or missing features and a repair hint. Malformed CLI
arguments use the ordinary `{"ok":false,"error":{...}}` response.

## Connect an agent

Create a workspace directory for your media and outputs. Configure your MCP client to launch
the installed executable with these arguments, replacing both paths:

```json
{
  "mcpServers": {
    "cutbolt": {
      "command": "/absolute/path/to/cutbolt",
      "args": ["--workspace", "/absolute/path/to/workspace", "mcp"]
    }
  }
}
```

On Windows, use an absolute path to `cutbolt.exe` and escape JSON backslashes, or use forward
slashes. The default catalog has 18 tools including `cutbolt_run`; `mcp --tools full` exposes
all 86 direct tools. See [AGENT_INTERFACE.md](AGENT_INTERFACE.md) for saved revisions, jobs and
workspace boundaries, and [USAGE.md](USAGE.md) for editing commands.

## Install the optional agent plugin

Build/install the executable first and check `cutbolt doctor` from the same environment as
Claude Code. The plugin uses `cutbolt` on PATH and does not install the engine, FFmpeg, fonts,
models or other dependencies. It launches local stdio MCP in the directory where you start
Claude Code. Start in the workspace containing your media and outputs, not the engine checkout.

Register the local checkout, then install its plugin:

```text
claude plugin validate /absolute/path/to/Cutbolt/.claude-plugin/plugin.json --strict
claude plugin validate /absolute/path/to/Cutbolt/.claude-plugin/marketplace.json --strict
claude plugin marketplace add /absolute/path/to/Cutbolt
claude plugin install cutbolt@cutbolt-local --scope user
claude plugin list
claude mcp list
```

Replace the checkout path with a Windows or Linux absolute path; quote paths containing spaces.
Restart an already-open client after installation. `claude mcp list` should show the plugin's
Cutbolt connection as connected. A failed connection usually needs the executable's directory
on PATH; `cutbolt --version` and `cutbolt doctor` in that terminal identify the selected build.
The client needs its own working model authentication before an agent can execute a recipe;
a connected MCP server alone does not establish model access.
The marketplace entry uses local files. It is not a hosted registry or a published binary release.
See the client's [plugin format](https://code.claude.com/docs/en/plugins-reference) and
[marketplace installation](https://code.claude.com/docs/en/plugin-marketplaces) documentation.

The six original skills are `cutbolt`, `recut-talking-head`, `screen-tutorial`,
`interview-multicam`, `captions` and `deliver-and-review`. The client exposes names such as
`/cutbolt:cutbolt` and `/cutbolt:recut-talking-head`. The router selects a recipe for existing
footage; each recipe includes requests, expected results, revision handling and recovery.
A new graphics-led video may start in an HTML authoring tool, with Cutbolt finishing its output.

For a clean-profile installation check, set `CLAUDE_CONFIG_DIR` to a new, empty external
folder in the testing shell before the commands above. Run from a separate media workspace
without project configuration, and do not copy credentials or existing client settings into
that profile. This environment override isolates settings, history and plugins as described
in the client's [environment reference](https://code.claude.com/docs/en/env-vars). Normal
installation does not need the override. The tested Windows client is Claude Code **2.1.280**;
other client builds and Linux plugin installation have not yet been verified.

`python -X utf8 tools/verify.py --only skills` executes every JSON request in the skills
against original synthetic sources. It verifies eight bounded workflows, including automatic preparation and insets, plus outputs and source
preservation on Windows and Linux. This is scripted documentation verification: fresh-model
workflow success and model token usage are separately tracked in [AGENT_COSTS.md](AGENT_COSTS.md).
