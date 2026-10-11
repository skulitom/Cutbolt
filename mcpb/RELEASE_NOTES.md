Cutbolt is a local video editing engine for agents, with exact timelines, saved sessions and native MCP over stdio. It makes no network requests at runtime.

The `cutbolt-vX.Y.Z-<target>` archives contain the CLI, MIT license, README and dependency inventory for Windows x64, Linux x64 or macOS ARM64. Windows-only engine features remain Windows-only. `cutbolt-windows-x64.mcpb` contains the Windows x64 server, its 32 core tool descriptions and license/dependency information for Claude Desktop. Every archive and the bundle also carries the target's dependency license texts in `THIRD_PARTY_NOTICES.md`. Install the bundle through **Settings > Extensions > Install from file** and choose an existing workspace.

Install **FFmpeg and ffprobe separately** on PATH, then restart the client, or set `CUTBOLT_FFMPEG` and `CUTBOLT_FFPROBE` to their executable paths. Neither program is bundled.

Check downloads against `SHA256SUMS`, or use the bundle's individual checksum:

```sh
sha256sum --check cutbolt-windows-x64.mcpb.sha256
gh attestation verify cutbolt-windows-x64.mcpb --repo skulitom/Cutbolt --signer-workflow skulitom/Cutbolt/.github/workflows/release.yml
```

In PowerShell, compare `(Get-FileHash cutbolt-windows-x64.mcpb -Algorithm SHA256).Hash` with the `.sha256` file. Substitute an archive name in the attestation command to verify a CLI download. Registry publication is a separate manual step after testing; see [Publishing](https://github.com/skulitom/Cutbolt/blob/main/mcpb/PUBLISHING.md).
