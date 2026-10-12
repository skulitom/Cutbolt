# Publishing Cutbolt

Pushing a version tag runs **Release**: native Windows x64, Linux x64 and macOS ARM64 builds with Rust 1.98.1 (Windows links the C runtime statically, so no Visual C++ Redistributable is needed), archives, the Windows MCPB smoke test, manifest validation, checksums, provenance attestations and a GitHub release. Only the highest stable tag becomes Latest. Reruns skip identical assets and fail on different bytes; they never replace assets. The `cutbolt-mcpb` workflow artifact also contains `server.registry.json`. Pull requests touching the distribution files run the build and verification as a dry run, without attesting or releasing. Branch pushes do not trigger Release.

**Published registry versions are immutable. Publishing metadata-only permanently prevents that version from ever getting its bundle.** Registry publication is a separate manual dispatch after testing. It uses GitHub OIDC, requires an existing release and skips versions already listed. An attached bundle takes precedence over `metadata_only`.

## Release and test

Artem bumps `[package] version` in `Cargo.toml` in his usual reviewed commit. Tag that commit and push the tag, substituting its stable version:

```sh
git tag vX.Y.Z
git push origin vX.Y.Z
```

Leave `mcpb/manifest.json` versionless. Both the manifest builder and registry entry writer use Cargo.toml. The entry writer stamps the release version into a copy of `server.json`; the source metadata's version may be stale and needs no release edit. The tag, Cargo version and bundle manifest must agree. Metadata-only publication stamps the same release version. Never move an existing release tag to changed bytes.

The builder discovers every page of the binary's core tool catalog and logs added or dropped names; `mcpb/manifest.json` keeps curated descriptions as display overrides, not a fixed tool inventory. New tools use the first sentence of the server description, shortened to at most 160 characters on a word boundary.

From a Windows checkout of that tag (the verifier compares against its Cargo.toml), download into a fresh directory and test:

```sh
gh release download vX.Y.Z -R skulitom/Cutbolt -p cutbolt-windows-x64.mcpb -p cutbolt-windows-x64.mcpb.sha256 -D target/dist/release-test
gh attestation verify target/dist/release-test/cutbolt-windows-x64.mcpb --repo skulitom/Cutbolt --signer-workflow skulitom/Cutbolt/.github/workflows/release.yml
node mcpb/verify.mjs target/dist/release-test/cutbolt-windows-x64.mcpb
```

Compare PowerShell's `(Get-FileHash target/dist/release-test/cutbolt-windows-x64.mcpb -Algorithm SHA256).Hash` with the downloaded `.sha256`. The verifier extracts under `target/`, creates an existing empty workspace, starts the manifest command, checks initialize and the exact core tool set, and calls `capabilities` through `cutbolt_run` with media tools deliberately unavailable. It checks the checkout, extraction, working directory and redirected home/temp paths for stray writes, then kills the server and removes its temp folder. Build-cache contents and the rest of the machine are not comprehensively monitored; this is not an OS sandbox. On non-Windows hosts verification fails unless `--allow-skip` is explicit.

Optionally install the downloaded MCPB in Claude Desktop: **Settings > Extensions > Install from file**, choose an existing workspace and ask it to run the `capabilities` command (through `cutbolt_run`). The default `${HOME}` already exists; Documents may be absent or redirected. Cutbolt creates `.cutbolt/store`, `jobs`, `cache` and `scratch` inside the workspace at startup; omitted media and output roots use the workspace itself.

## Publish the tested bundle

```sh
gh workflow run publish-registry.yml -R skulitom/Cutbolt -f tag=vX.Y.Z
```

The workflow verifies the downloaded bundle's Release-workflow attestation, prepares its exact SHA-256 entry, validates with the pinned `mcp-publisher` 1.8.1, publishes, and reads back the checksum. Check its step summary and the [official listing](https://registry.modelcontextprotocol.io/v0.1/servers?search=io.github.skulitom/cutbolt). If the release lacks its bundle, rerun Release before publishing. Use `metadata_only` only when deliberately accepting the permanent loss of that version's bundle.

## Local packaging

With Node 20+ and an already-built Windows release binary:

```sh
node --test mcpb/mcpb.test.mjs mcpb/notices.test.mjs
node mcpb/notices.mjs --target x86_64-pc-windows-msvc --out target/THIRD_PARTY_NOTICES.md
node mcpb/build.mjs --binary target/release/cutbolt.exe --notices target/THIRD_PARTY_NOTICES.md
node mcpb/package.mjs x86_64-pc-windows-msvc target/release/cutbolt.exe target/dist/release --notices target/THIRD_PARTY_NOTICES.md
node mcpb/verify.mjs target/dist/mcpb/cutbolt-windows-x64.mcpb
node mcpb/build.mjs --registry-entry --tag vX.Y.Z --bundle target/dist/mcpb/cutbolt-windows-x64.mcpb
```

Tests use synthetic fixtures and also verify the real binary when available and matching Cargo.toml; a stale local binary is reported as skipped. The dependency-free scripts sort entries, fix timestamps and normalize text to LF. Identical inputs with the same Node/zlib yield identical archives. Existing different output requires a fresh `--out` directory. For MCPB CLI validation, extract the generated manifest and run `npx --yes @anthropic-ai/mcpb@2.1.2 validate <extracted-manifest.json>` (the Release workflow does this).

The notice generator runs `cargo metadata --format-version 1 --locked --offline --filter-platform <target>` from the repository root, using sources already fetched by the build. It follows normal dependency edges from `cutbolt`, excludes dev/build-only dependencies, and includes each package's declared license file and top-level license/notice files. Shared LF-normalized texts appear once under stable SHA-256 IDs. It fails with the package names if any dependency has no text; resolve that before packaging, without substituting invented text. Generate separately for each target (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-apple-darwin`); both packagers require `--notices <file>`. Release generates this after each matrix build, and includes `THIRD_PARTY_NOTICES.md` in every archive and the bundle. Keep generated notices in `target/` or temporary build output; never commit them.

FFmpeg and ffprobe are required for media work and are **not bundled**, per the project rule. Install them separately on PATH and restart the client, or set `CUTBOLT_FFMPEG` and `CUTBOLT_FFPROBE`. Archives and the bundle carry the project LICENSE, dependency inventory and the target's dependency license texts in `THIRD_PARTY_NOTICES.md`. The [inventory](../docs/DEPENDENCIES.md) does not replace those texts. Linux/macOS archives do not imply Windows-only features such as background jobs work there.

Glama and awesome-mcp-servers listings are submitted and maintained manually.
