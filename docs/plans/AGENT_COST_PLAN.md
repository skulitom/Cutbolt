# Fix plan: make Cutbolt cheap for a model to use

Handoff plan, 11 October 2026. Written for an implementing agent. It changes no scope, criteria or points. This is agent-interface work: it earns **no** editing points (see `AGENTS.md` and [HYPERFRAMES_STRATEGY.md](../HYPERFRAMES_STRATEGY.md)). Line references are to `main` at `549e319`; re-check them before editing.

## Current acceptance audit

The [14 October requirement audit](PLAN_COMPLETION_AUDIT.md) maps implementation
and passing assertions to every stage below. All stages and acceptance requirements
are verified, including the placed comparison and the final 86-fixture thorough
milestone at `5d91c46`. Earlier checkpoint notes are retained as dated history;
the original problem description follows.

## The problem

1. **The default catalog is the full one.**
   - `cutbolt mcp` with no flag lists 84 tools and 218,196 bytes, about 55–62k tokens, before the first call (`main.rs:108-121` defaults `CUTBOLT_MCP_TOOLS` to `full`).
   - `--tools core` lists 32 tools in 66,621 bytes, about 17–19k tokens.
   - The strategy target (A6) is a default of at most 40,000 bytes.
2. **Authoring needs a schema models don't know.**
   - The title-card example is 6,420 bytes for a 160×90 canvas at `output_scale: 4`.
   - The `scene` schema is 75 KB in full. Agents read 8–13 KB outlines before writing anything.
   - Templates exist only as two example files.
   - Template bindings cannot set `width`, `height`, `output_scale`, `frame_rate` or `transparent` (`templates.rs:203-227`). So the shipped templates cannot produce a 1080p, 60 fps or transparent overlay without the agent editing the template itself.
   - Fonts must be supplied as identities, and none are bundled.
3. **Low-level commands instead of intent.**
   - There are 101 commands, plus 28 timeline `op` tags and 23 `tracks.edit` sub-ops.
   - Workflow commands exist (`audio.tighten`, `transcript.fillers`, `captions.draft`, `audio.normalize`, `export.review`, `timeline.outline`, `timeline.check`, `transcript.assemble`), but nothing tells a model the recipe for "recut this talking head".
   - There are no skills, no plugin and no `llms.txt`.

Measured cost, from `docs/PROGRESS_HISTORY.md:1649-1671` and the 5 October demo research in `C:\DEV\CutboltData\demo-progress2-20261005\research\RESEARCH.md`:
- On 4 October, three MCP-only agents fell back to the CLI. 60–90% of their response bytes went to schema and capability lookups.
- On 5 October an 80 s explainer took 299 calls and 1.4 MB received, about 400k tokens. The production manifest path did the same job in 2–3 calls and about 2k tokens.

## Rules that constrain the design

- Every command stays reachable. `--tools full` stays available and keeps today's behavior.
- MCP stays stdio-only. No listening service, no hosted API, no telemetry.
- No third-party assets: no bundled fonts, presets or templates copied from other products. Original templates only.
- Don't copy HyperFrames skills, docs or wording (Apache-2.0). Write original material ([RESEARCH.md](../RESEARCH.md) boundaries).
- Claim only what a test proves. Every command example in a skill must run in a fixture.
- Command names must not contain `_` (unit test at `mcp.rs:909-914`).

## How the catalog is built today

- **Full catalog.** `tools()` (`mcp.rs:449-470`) walks `schema::commands()` and filters out the 17 `BLOCKING` commands (`mcp.rs:324-342`).
  - Descriptions come from the hand-written `description()` match (`mcp.rs:13-320`).
  - Input schemas come from `schema::arguments(cmd, true)` and then `schema::listing()` (`schema.rs:535-608`), which collapses `ProjectInput`, drops numeric `format` and `default: null`, prunes `$defs` and stubs the seven deferred types (`schema.rs:8-16`, `121-168`).
- **Core catalog.** `CORE` (31 commands, `mcp.rs:384-416`) plus `cutbolt_run` (`mcp.rs:419-441`). `cutbolt_run` routes any other exposed command through `{command, arguments}`, with no listed schema, so an agent calls `cutbolt_schema` first.
- **Where core's 66.5 KB goes:**
  - descriptions inside schemas, 24.4 KB;
  - `$defs`, 12.4 KB, of which `Time` is 6.6 KB, repeated in 17 tools at 386 B each;
  - tool descriptions, 10.6 KB;
  - annotations, 2.9 KB;
  - a `$schema` dialect string on every tool;
  - the `project` stub, repeated 14 times.
- **Largest core listings:** `export.inspect` 7,521 B; `captions.draft` 3,777; `transcript.fillers` 3,093; `transcript.assemble` 2,731; `preview.sheet` 2,604.
- **Budgets enforced in tests:**
  - 16 KiB per tool and 256 KiB per catalog (`mcp.rs:872-873`, checked by `check_catalog` at `1048-1077`);
  - core at most 96 KiB, without a workspace only (`mcp.rs:895-905`);
  - `tests/agents.py:209` requires `len(core) * 3 < len(full)`.
- **MCP instructions** are about 3.6 KB (`mcp.rs:599-625`).
- **Fixture coupling.** `tests/engine.py:11-12` sets `MCP_TOOLS = 84`. `tests/agents.py:24` `Client` defaults to `("mcp",)`, and 29 fixture files use it and expect the full catalog.

## Strategy

Measure first, then ship in six stages. Each stage is independently shippable.

| Stage | Result | Measure |
| --- | --- | --- |
| 0 | A replayable agent-cost harness with budgets in CI | Baseline bytes, calls and errors per workflow |
| 1 | Quick wins: doc and description drift, error naming, annotations | Fewer failed calls in replays |
| 2 | A default catalog of at most 40 KB; full stays opt-in | `tools/list` bytes |
| 3 | An error contract agents can act on: retry flag, field path, repair hint | Failed-call recovery in replays |
| 4 | Graphics without schema study: built-in templates, a shorthand, font discovery | Bytes sent to make a title; schema lookups per task |
| 5 | Intent-level skills, every example tested, and a plugin manifest | Calls and tokens per workflow vs stage 0 |

## Stage 0: the measurement harness (do this first)

Build `tests/agent_costs.py`, registered in `tools/verify.py` STAGES. It replays scripted MCP sessions for five workflows against original generated media, through the official Python MCP SDK (`mcp` 1.12.3, already a development dependency). The workflows match the strategy's flagship list (B2):
1. **Talking-head tighten:** transcribe, remove pauses and fillers, captions, H.264 export with review. On machines without a speech runtime, use a known-text alignment or a canned transcript document so the fixture still runs.
2. **Two-camera interview:** a multicam group, cuts and linked audio.
3. **Screen tutorial:** trim, ripple-delete sections, a title card, normalize loudness, export.
4. **Revise one section:** replace a range, undo, re-export only the range.
5. **Delivery handoff:** export, `export.review`, OTIO export.

For each workflow, record:
- the `tools/list` and `initialize` bytes;
- request and response bytes per call;
- the number of calls and failed calls;
- the number of `cutbolt_schema` and `capabilities` lookups.

Write the results to the fixture's report as JSON. Assert **budgets**: the current numbers plus a margin at first, tightened at each stage, so a regression fails verification.

The scripts are the *best-known* call sequences, not a model's run. Separately, document how to run a real model trial: the same tasks given to a fresh agent with only the MCP server, recording the same measures. Record the agent model and version. Keep raw transcripts outside the repository, and summaries in `docs/`.

**Exit:** `python -X utf8 tools/verify.py --only agent_costs` passes and prints the baseline table. Add it to progress history.

## Stage 1: quick wins (one or two sessions)

Fix what tells agents wrong things today. Each fix is small.

1. **MCP descriptions that say a command is CLI-only although `job.start` runs it:**
   - `graphics.instantiate` (`mcp.rs:121`) and `scene.inspect` (`mcp.rs:154`) say "blocking CLI/library, not this MCP connection". Say instead: compile with `job.start` `run: "scene.render"`.
2. **Doc drift:**
   - `docs/AGENT_INTERFACE.md:205` says type errors carry no field path, which contradicts `:194` and the code.
   - `docs/AGENT_INTERFACE.md:207` says calls are serial with no progress notifications, which contradicts `:196-203`.
   - `docs/SCENES.md:27-29` says absolute roots and `sha256` are required, and that `scene.render` is absent from MCP and jobs.
   - `docs/TEMPLATES.md:95` says compilation is CLI-only.
   - The README, `USAGE.md`, `NATIVE_TIMING.md` and `SCENES.md` still say placed tracks and scenes are limited to 25 fps (strategy item A7).
   - `HYPERFRAMES_STRATEGY.md:77`, `:193` quote catalog sizes from before the 5 October trim. Add a dated note instead of rewriting history.
3. **Template errors that do not name the parameter:**
   - `UNKNOWN_PARAMETER` (`templates.rs:482-486`) and "Binding and parameter types differ" (`templates.rs:476-478`). Name the parameter, its declared type and the given type, as `MISSING_PARAMETER` already does (`templates.rs:460-465`).
4. **`cutbolt_run` annotations:** it is marked `readOnlyHint: false, destructiveHint: true` (`mcp.rs:439`), so clients that gate on annotations treat every routed read-only call as destructive.
   - The MCP annotation applies to the tool, not the call, so it cannot be exact.
   - Set `destructiveHint: false`, because the destructive commands (`job.cancel`, `cache.prune`) stay direct tools (see stage 2).
   - State in the description that routed commands never delete.
5. **`cutbolt_run` routing core commands:** it accepts commands that are already direct tools (`mcp.rs:725-742`). Keep this, since it is harmless, but the description should tell agents to call direct tools directly.

**Exit:** the stage 0 replays show no schema lookups caused by wrong descriptions. Run the `tests/agents.py`, `tests/templates.py` and `tests/agent_ergonomics.py` fixtures.

## Stage 2: a small default catalog

### 2.1 Shrink the compact catalog to at most 40,000 bytes

Make these listing-only changes in `schema::listing()` and `mcp::tools()`. Lookups through `cutbolt_schema` keep full definitions.

1. Drop `$schema` from every listed `inputSchema`. The protocol default is Draft 2020-12. Check that the official SDK and `jsonschema` validation in `tests/agents.py:186-200` still pass.
2. Define `Time` once:
   - Shorten the listed `Time` definition to a one-line description and its accepted shapes (`2.5`, `"5/2"`, `{num, den}`).
   - Explain exact time once in the MCP `instructions`.
   - Target: under 120 B per occurrence, from 386.
3. Write annotations only when they differ from the defaults (`readOnlyHint` false, `destructiveHint` true for non-read-only tools per the spec). Today every tool carries all of them. Check how each client treats missing hints, and record the result.
4. **Description budget for listed schemas:**
   - Required fields keep their full descriptions.
   - Optional fields keep one clause, at most 120 characters.
   - The full text stays in `cutbolt_schema`.
   - Enforce this with a unit test over listings, beside `every_schema_field_is_documented` (`schema.rs:740-751`).
5. **Re-pick core with data from stage 0.** Keep a direct tool only when the replays use it in at least two workflows, or it is needed to recover (`session.undo`, `job.cancel`). Expect about 20 direct tools plus `cutbolt_run`. Candidates to route:
   - `export.inspect` (7.5 KB): route it, and make sure `job.start` validation gives the same field errors;
   - `scene.inspect`, `audio.beats`, `audio.duck`, `timeline.meters`.
6. Add `cutbolt_run` routing hints: its `command` enum stays, but its description groups the routed commands by area, one line per area, instead of listing all 53 in prose.

Add a unit test in `src/mcp.rs`: the default listing is at most **40,000 B** with and without a workspace, and each core tool is at most **4 KiB**. Keep the full-catalog budget tests.

### 2.2 Flip the default

1. `main.rs:108-121`: with no flag and no `CUTBOLT_MCP_TOOLS`, use `core`. `--tools full` and `CUTBOLT_MCP_TOOLS=full` keep today's catalog.
2. Fixtures:
   - Change `tests/agents.py` `Client`'s default args to `("mcp", "--tools", "full")` in one place, so the 29 fixtures that expect the full catalog keep working.
   - Add explicit tests that a bare `mcp` lists the compact catalog, and that the variable and the flag still override it.
   - Keep `tests/engine.py` `MCP_TOOLS` for the full count, and add `MCP_CORE_TOOLS`.
3. Update `docs/AGENT_INTERFACE.md:99` (it says `--tools full` is the default), the README's run section and the MCP `instructions`.
4. Record the change in progress history as a user-visible default change.

**Exit:** the default `tools/list` is at most 40,000 B. The stage 0 replays pass with fewer or equal calls, and the full catalog is unchanged under `--tools full`.

## Stage 3: an error contract agents can act on

Today errors are `{code, message, detail?}` (`lib.rs:107-125`). Field paths exist only inside messages for parse errors (`commands.rs:1301-1318`; `schema.rs:318-483`). Only `TRACKING_UNRELIABLE` carries structured `detail` (`tracking.rs:86`). There is no retry flag.

1. Add two optional fields to `Error`, serialized only when set:
   - `retry`: `"same"` (transient; the same request may succeed, e.g. `TOOL_TIMEOUT`, `WORKER_INTERRUPTED`, `REVISION_CONFLICT` after a refresh), `"changed"` (fix the request first) or `"never"`. Derive it from the code with one table, `fn retry_class(code)`, and unit-test that every code used in `src/` is classified.
   - `path`: the JSON path of the offending field, when known: parse errors, the `At::at`/`under` prefixes (`lib.rs:167-203`), and `job.start` `arguments.` (`commands.rs:1237-1245`).
2. Add `hint` where a fix is mechanical. Make it a short string, or a ready request in `detail.next`, as the `next` field already does on 14 successful proposals (for example `tighten.rs:384`). Candidates:
   - `OUTPUT_EXISTS`: suggest a free name;
   - `IO_ERROR` for a missing output folder;
   - `UNSUPPORTED_JOB`: list the queueable commands;
   - `MIGRATION_REQUIRED`;
   - `REVISION_CONFLICT`: give the current revision;
   - `MISSING_CLIP`, `MISSING_TRACK`, `MISSING_MEDIA`: put the available IDs, which messages already list (`lib.rs:127-165`), in `detail.available`.
3. Add an error code registry: a table in `docs/AGENT_INTERFACE.md`, generated or checked by a test that scans `error("...")` call sites, listing code, retry class and meaning. This is the start of the 1.0 gate G4 "stable error codes".
4. In MCP, keep the envelope unchanged apart from the new optional fields. Measure catalog and result size before and after.

**Exit:** the stage 0 harness includes deliberate mistakes (a wrong field, an existing output, a stale revision), and the scripted recovery uses only `retry`, `path` and `hint`, never message parsing.

## Stage 4: graphics without schema study

### 4.1 Built-in original templates

1. Move templates from `examples/` into the binary with `include_str!`, as a small catalog of about 20 **original** designs:
   - lower thirds (3);
   - title cards (3);
   - end card;
   - callout arrow and box;
   - chapter marker;
   - progress bar;
   - captions (3 styles);
   - quote card;
   - subscribe-style reminder (original design, no platform logos);
   - countdown;
   - name tag;
   - list reveal.

   Each has typed parameters, sensible defaults and a test that it renders.
2. Let bindings set `width`, `height`, `output_scale`, `frame_rate` and `transparent` (`templates.rs:203-227` today forbids it). Default these from the target project when an instance is created against a project, so the default output is the project's size and rate.
3. New read-only command `templates.list`: id, one-line purpose, parameters with types and defaults, and output size. Change `graphics.instantiate` to accept `template_id` (built-in) as an alternative to an inline `template` object.
4. Add `graphics.place`: instantiate, compile and place on an `alpha_over` track in one queued job, returning the session revision. This saves about four calls per title.
   - If the direct-footage plan's stage 5 (generated clips) has landed, place the scene as a clip instead and skip the compile. This conditional is implemented and verified on Windows/Linux in the [13 October generated-placement update](../../verification/generated-graphics-2026-10-13.json).

### 4.2 Fonts without hunting

No fonts may be bundled. Add `fonts.find`: list TrueType/OpenType files in the OS font directories (Windows `%WINDIR%\Fonts` and the user font folder; Linux `/usr/share/fonts`, `~/.local/share/fonts`; macOS `/System/Library/Fonts`, `/Library/Fonts`), filtered by family name. Return identities ready for scenes. Fonts are read, never copied. Document that output embeds rasterized glyphs, and that licensing of a font stays the user's responsibility. Accept a family name wherever a font identity is accepted only if resolution is unambiguous, and report the resolved file.

### 4.3 Scene shorthand (strategy item B4)

A terse form that expands to the existing `Scene`, accepted by `scene.inspect`, `scene.still`, `scene.render` and templates, and returned expanded so an agent can inspect it:
- seconds as plain numbers, hex colors, default transforms;
- named anchors (`lower_third`, `center`, `top_left` with a safe-area margin);
- named easings;
- `font: "Inter"` resolved through 4.2;
- `size: "project"`.

Prefer an explicit marker such as `"form": "short"` over guessing.

**Tests:** shorthand and expanded scenes render identically (decoded digests). The title-card example fits in under 1 KB, from 6.4 KB.

**Exit:** in the stage 0 harness, making a title costs at most two calls and at most 1.5 KB sent, with no `cutbolt_schema` lookup.

## Stage 5: intent-level skills and a plugin

Acceptance satisfied, 13 October 2026: six tested skills, clean Windows plugin installation,
Windows/Linux recipe checks, and fresh workflows 1/3/5 with the approved peak-context metric.
See the [requirement audit](../../verification/agent-cost-stage5-2026-10-13.json) and
[measured limits](../AGENT_COSTS.md#stage-5-acceptance-13-october-2026). This does not claim a
comparative benchmark, recognition accuracy or distribution-release completion.

1. Add original skills under `skills/`, each a `SKILL.md` under about 300 lines:
   - a `cutbolt` router for editing existing footage, using the draft description in `HYPERFRAMES_STRATEGY.md`, section P0, once tested;
   - `recut-talking-head`, `screen-tutorial`, `interview-multicam`, `captions`, `deliver-and-review`.

   Each lists the call sequence from the stage 0 replay, the expected results and the recovery steps. It says what it cannot do on the current platform (speech on Linux, until the Linux port lands its speech path).
2. Add a `skills` fixture: parse every fenced JSON request in every skill, run it against generated media in order, and fail on any error. Fail if two skills describe the same command differently, by comparing command names and argument keys.
3. Add a plugin manifest for Claude Code (`.claude-plugin/plugin.json` plus a marketplace entry) that registers `cutbolt mcp` and the skills. Test the documented install in a clean profile.
   - `tools/check_repo.py:9-10` allows `.md` and `.json`, so these pass.
   - An `llms.txt` would need the allow-list extended.
   - Release binaries and `cutbolt doctor` belong to the distribution track (strategy A3). Coordinate rather than duplicate.
4. Give the router skill an honest description:
   - claim footage editing, captions, loudness and delivery;
   - point new motion-graphics videos to an HTML tool, with Cutbolt as the finishing stage.

   Never write an over-claiming router.

**Exit:**
- A fresh agent with only the plugin completes workflows 1, 3 and 5 from the stage 0 list. Record calls, tokens and failures against the stage 0 baseline, and add them to progress history.
- Approved targets: at most 15 calls (including skill loads) and 40,000 tokens of maximum input context per request for a talking-head tighten with captions and export, and no `cutbolt_schema` lookup in any skill-driven run. The user clarified the token metric on 13 October 2026; keep cumulative provider input, including repeated cached context, separately reported. Trial 07 meets these measured targets; see [the acceptance update](../AGENT_COSTS.md#approved-token-metric-13-october-2026).

## Verification and shipping, every stage

- Work in your own worktree on a `claude/<name>` branch.
- While working, run `cargo test` and `python -X utf8 tools/verify.py --only integration,agent_ergonomics,agent_costs,<touched>`. Commit, then run `python -X utf8 tools/ship.py --to main`, and fix forward anything `tools/verify.py --status` reports.
- The Linux port of jobs and speech landed on `main` as `1ea6447`, so start from it. MCP-only agents on Linux can now queue jobs, so run the stage 0 harness and the skills fixture on Linux as well as Windows. Skills must not promise audio recording or hardware decode on Linux; neither has a Linux backend yet.
- Line endings: `src/main.rs`, `src/commands.rs`, `tests/agents.py`, `tools/verify.py` and `docs/AGENT_INTERFACE.md` are CRLF.
  - Edit with byte-preserving scripts or the Edit tool.
  - Restore endings after `cargo fmt`.
  - Never `sed -i` tracked files.
- Catalog budgets live in unit tests. Run `cargo test mcp` after any schema or description change.
- Add a progress-history entry per stage, with the stage 0 measurements before and after. No editing points.

## Open decisions for the user

| Decision | Recommendation |
| --- | --- |
| Flip the MCP default to the compact catalog | Yes, at stage 2, once it is 40 KB or less. `--tools full` stays. |
| Budgets for the five workflows | Set from stage 0. Then tighten each stage, never loosen silently. |
| `fonts.find` reading OS font folders | Yes. Read-only, no copying, licensing noted. |
| Number of built-in templates | About 20 original designs. Never copy or imitate another product's catalog. |
| Plugin harnesses after Claude Code | Decide with the distribution track. The strategy report suggests Codex, then Gemini CLI. |

## Final acceptance, 14 October 2026

All stages 0–5 are verified by the [requirement audit](PLAN_COMPLETION_AUDIT.md).
The strict thorough milestone at `5d91c46` passes all 86 fixtures, including the
scripted workflow budgets, compact catalog, structured errors, original templates,
font discovery, shorthand and executable skills. Separate retained fresh-agent
trials satisfy the approved call, peak-context and no-schema-lookup targets.
The milestone refresh does not turn byte replays into provider-token measurements
or award editing points. Earlier checkpoint notes remain historical evidence.
