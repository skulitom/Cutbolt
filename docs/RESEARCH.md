# Research boundaries and provenance

Updated 2 October 2026. Product-specific investigations, audit reports and detailed provenance are private local research records. Keep them outside the repository; accidental local copies are excluded from Git. Public documentation describes original project behavior, specifications, dependencies and acceptance criteria.

## Repository contents

Commit original code, original documentation, schemas and synthetic fixture generators. Do not commit third-party application files, installers, plugins, assets, presets, documentation copies, SDK source, native projects, memory dumps, disassembly, decompiler output, analysis databases, captures or generated media. Preserve required dependency licenses and attribution; moving research out of the repository does not remove distribution obligations.

The ignore rules prevent common accidental additions. `python tools/check_repo.py` checks the candidate tree; `python tools/check_repo.py --staged` checks actual indexed bytes, including forced additions and files whose working copies differ. Local content restrictions can be configured in the Git-private `private-content-policy.json` file as a `patterns` array of regular expressions. Set local `cutbolt.requireContentPolicy=true` to fail closed if that policy is missing. A local pre-commit hook runs the staged check in this checkout. Hooks and local policy do not automatically transfer to clones; establish them for each checkout before publishing.

These checks supplement provenance review. They cannot prove originality or prevent a user from deliberately disabling hooks or using `--no-verify`. Review the complete repository history before distribution.

## Research ladder

1. Define a concrete interoperability need and measurable acceptance criteria.
2. Review public specifications and supported interfaces first. Record licenses and exact versions externally when selecting tools.
3. Use original fixtures to test a bounded supported route. Retain exact settings, hashes, outputs, failures and confidence in private records.
4. Identify any remaining gap. Prefer a supported conversion or explicit unsupported-feature diagnostic where it meets the requirement.
5. Consider binary inspection only for a necessary, bounded question with an established access basis and applicable prerequisites. Define covered modules/builds and a stop condition. Contacting another party requires separate authorization.
6. Implement from original design, public specifications and appropriately reviewed factual requirements. Do not translate, paraphrase or port decompiled implementation, control flow, private data tables or shaders.

Stop inspection once the specific fact is established. Do not use general application extraction as an engine implementation strategy. Renaming code or keeping the original file out of Git does not establish independent authorship.

A formal clean-room process requires independent researchers and implementers with controlled information flow and appropriate review. Separate folders or conversations alone do not establish it; an implementer exposed to proprietary implementation is not independent of that exposure. No formal clean-room process is claimed here.

## Scope and evidence

Underlying editing operations, offline assistance and project interoperability remain in scope. Accounts/activation, cloud services, stock marketplaces, telemetry and GUI-only workflows remain excluded. Test exact builds for compatibility; do not generalize one bounded result to all releases.

Private research completion earns no engine implementation points. Maintain the full acceptance denominator and award points only for verified engine behavior. Repository wording changes must not erase unimplemented capabilities or silently change their acceptance criteria.

Keep raw research in an access-controlled directory outside this checkout. Retain its original provenance and failed results there. Admit only reviewed, minimal factual requirements necessary to the original design and consistent with the publication boundary.

## Dependencies and release provenance

Dependencies remain external, with exact versions and licenses in [the dependency ledger](DEPENDENCIES.md). Do not vendor them. Preserve source media and use explicit input/output roots; never render over an input.

For each release, review the resolved dependency graph, required notices and corresponding distribution obligations. Process separation does not remove those obligations. External media tools require review of the actual build and configuration; see [FFmpeg's license guidance](https://ffmpeg.org/legal.html).
