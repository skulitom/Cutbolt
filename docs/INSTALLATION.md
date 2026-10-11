# Local installation

Cutbolt is a local executable and Rust library. An MCP client starts the executable over stdio;
there is no server to deploy. Windows x86-64 and Linux x86-64 are the current verification
platforms. Tagged binary releases and packaged agent skills are still in preparation.

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
