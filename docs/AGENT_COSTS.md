# Agent workflow cost measurements

12 October 2026. Measurements for [the agent-cost plan](plans/AGENT_COST_PLAN.md).
Scripted recipes through official Python MCP SDK **1.12.3** establish regression ceilings.
The valid plugin-only trials below complete three bounded workflows. The latest talking-head
trial meets 15 total calls and the user-approved 40,000-token maximum request context. Earlier trials with extra client tools are retained
but excluded from plugin-only acceptance. Neither measurement establishes a comparative lead.

## Replay

```powershell
python -X utf8 tools/verify.py --only agent_costs --report C:\DEV\CutboltData\agent-cost-summary.json
python -X utf8 tests/agent_costs.py --output C:\DEV\CutboltData\agent-cost-retained
```

The first command prints the table and saves aggregate and per-call measures in the requested report.
The second retains media, projects, rendered outputs and wire transcripts in a fresh external directory.
On Linux, use an absolute external output path such as `/tmp/agent-cost-retained`.
`--engine` selects a saved executable for an A/B comparison. `CUTBOLT_COST_CATALOG=core` or `full`
selects that catalog explicitly; omission measures the engine's default, independent of a user's
`CUTBOLT_MCP_TOOLS` setting. No model, downloaded footage, installed font or network is needed.

The relay records exact UTF-8 JSON-RPC line lengths in each direction, including newlines, duplicate
text/structured results, image blocks and notifications. Initialization and tool listing response
bytes are reported separately and included in total received bytes. Workflow call counts count only
`tools/call`. Each call has its actual command, request bytes, response bytes and failed status;
failed queued work also counts as a failure. The fixture counts schema and capabilities requests.
Raw wire logs remain outside the repository. Bytes are **not token counts**: image content and
tokenizers differ, so do not convert these totals into a claimed model usage result.

Workflows:

- Talking head: inspect an explicitly canned source-bound transcript, shorten authored pauses,
  remove an `um`, draft and burn captions, export H.264, review picture/audio/timing. The source is
  synthetic tones, not human speech; this measures the edit workflow, not recognition accuracy.
- Interview: retain two distinct cameras in sequences, create a camera group with a switch at 3 s,
  keep camera 1 audio, link root picture/sound, export. Encoder input hashes must equal an independent
  assembly of the two source pictures and unchanged master PCM.
- Tutorial: trim the beginning, ripple-delete a middle section, instantiate the original title-card
  template, compile/place it, apply a measured loudness proposal, export.
- Revision: replace a range, undo, assert the clips/assets are restored, export only the 1 s range.
- Handoff: export, review, inspect OTIO losses, explicitly acknowledge omission of editing links,
  publish OTIO. This workflow does not claim external-editor compatibility beyond the existing fixture.

These six-second, 160 x 90 cases isolate interface cost. Long-duration and quality acceptance stay
in their separate fixtures. All input identities must remain unchanged. Request/response ceilings
and call ceilings live in `tests/agent_costs.py`; changes require a recorded reason and before/after
measurement, never an unnoticed increase.

## Initial measurements

Windows and WSL2 Ubuntu runs at base `c4064df` plus the native preview duration fix. No MCP catalog or
response behavior changed in this preparation stage. The saved unmodified release also completed
all five recipes. Small byte differences include paths, build strings, worker identifiers and images.

| Workflow | Calls | Sent bytes | Received, Windows | Received, Linux |
| --- | ---: | ---: | ---: | ---: |
| Talking head | 15 | 8,730 | 412,758 | 412,300 |
| Interview | 6 | 4,223 | 249,511 | 249,385 |
| Tutorial | 12 | 7,860 | 265,568 | 265,438 |
| Revision | 8 | 2,320 | 243,704 | 243,578 |
| Handoff | 8 | 2,291 | 388,563 | 388,435 |

All five have zero failed calls, schema requests and capabilities requests. The full workspace
listing response costs **224,321 bytes** per workflow. Windows initialization is about **4.5 KB**.
The title portion of the tutorial takes **4 calls / 5,043 sent bytes**. These are initial values;
the later catalog and placement measurements below supersede these costs.

## Compact default, stage 2

The default now lists 17 direct tools and one router. Five direct tools occur in at least two
baseline workflows: `session.create`, `session.get`, `session.apply`, `job.start`, `job.wait`.
The other twelve support recovery: `schema` repairs invalid requests; `files.list` and
`media.inspect` find and diagnose sources; `timeline.check` diagnoses the edit; session
undo/history/receipt recover edits or lost responses; job status/cancel/resume recover interrupted
work; cache inspect/prune diagnose and free derived storage. Other exposed tools remain available
through the router; long-running commands still use `job.start`. Grouped routing hints and forthcoming skills supply intent-level guidance.

Only the compact listings change. They omit redundant dialect/default hints, use a 96-byte exact-time
shape summary, preserve required descriptions and shorten optional descriptions. Strict parsing and
full lookups remain. Full-mode tool arrays are byte-identical to `f8d680d`, with and without a
workspace. Official SDK and wire tests cover both modes, bare default, environment selection,
flag precedence, schema validation and annotation defaults. Python MCP SDK 1.12.3
retains omitted hints as `None`; the fixture applies protocol defaults explicitly
and requires full/core effective hints to agree. Fresh Claude Code 2.1.280 plugin
trials accept the compact catalog. Those trials do not establish how arbitrary
clients present permissions for missing hints.

| Workflow | Calls | Sent bytes | Received, Windows | Received, Linux |
| --- | ---: | ---: | ---: | ---: |
| Talking head | 15 | 8,850 | 209,465 | 209,341 |
| Interview | 6 | 4,223 | 46,552 | 46,426 |
| Tutorial | 12 | 7,920 | 62,609 | 62,479 |
| Revision | 8 | 2,320 | 40,745 | 40,619 |
| Handoff | 8 | 2,351 | 185,604 | 185,476 |

Both platforms have zero failed calls or schema/capability lookups. The workspace listing costs
21,043 response bytes, versus preparation's 224,321 (90.6% fewer). Without a workspace it is
21,201 bytes. The largest compact tool is 2,191 bytes. Initialization is about 4.8 KB.
Talking-head total received bytes fall 49.3%; inline image/result bytes remain in these totals.
Calls stay equal; routed arguments add 30 bytes for each routed call. The title still needs four
calls and 5,073 bytes. There is no model-token or wall-time improvement claim.

Default/core ceilings are now the actual 15/6/12/8/8 calls, with sent-byte limits
9,500/4,500/8,500/2,500/2,600 and received-byte limits 235,000/55,000/75,000/48,000/210,000.
Every default listing must also stay at most 40,000 bytes. Explicit `CUTBOLT_COST_CATALOG=full`
retains the original ceilings for compatibility and saved-baseline runs. No baseline ceiling
was raised. Use that setting when replaying an older binary that defaults to the full catalog.

## Structured recovery, stage 3

A separate replay deliberately supplies a string for `width`, uses a stale `expected_revision`,
and requests an already occupied preview filename. Its recovery helper removes both `code` and
`message` before acting: `retry`, `path` and `hint` identify the repair, while structured `detail`
supplies the current revision and unused filename. It reads the saved head and reconsiders the
append before resubmitting. The original preview remains byte-identical after the repaired call.

| Platform | Calls | Deliberate failures, repaired | Sent bytes | Received bytes |
| --- | ---: | ---: | ---: | ---: |
| Windows | 9 | 3 | 2,235 | 34,120 |
| Linux | 9 | 3 | 2,191 | 33,974 |

There are no schema/capability lookups. The recovery replay has its own ceilings (nine calls,
4,000 sent bytes, listing ceiling plus 35,000 received bytes); it does not relax any of the five
normal-workflow budgets. Their calls stay 15/6/12/8/8. Current Windows received bytes are
209,471/46,552/62,607/40,745/185,606; Linux receives 209,341/46,426/62,479/40,619/185,476.
Their 21,043-byte workspace listing is unchanged; the tiny total-result differences from stage 2
are variable receipt fields. No extra error fields appear on successful responses.

The [error contract and registry](AGENT_INTERFACE.md#structured-error-recovery) classify 234 engine
codes. A source scan checks registration and the documentation table. Existing codes/messages and
the MCP envelope remain. Optional fields survive queued command processes, persisted job errors and
batch failures. Separate queue tests retain an occupied output and verify the persisted repair
suggestion. Nested request paths, bounded missing-ID lists, revision details and non-overwriting
filename suggestions have Rust checks. A terminal failed job is not resumable merely because its
cause was transient; the guidance distinguishes eligible queued attempts from a new submission.

## Independent model trials

1. Build/save the exact engine and record its commit, executable identity, OS, FFmpeg build,
   SDK/harness version, model ID/version, reasoning setting and prompt. Keep raw transcripts outside Git.
2. Run `python -X utf8 tests/agent_costs.py --fixtures-only --output <fresh-external-directory>`.
   Each workflow gets new footage, an original test font, `assets.json`, `font.json`, `transcript.json`
   and the project-owned `title-template.json`, with no editing session or completed output.
3. Start a fresh agent for each task, with only the workspace-bound MCP server. For the plugin gate,
   expose only the tested plugin and its skills. Audit the client's complete initial tool/skill
   inventory and every actual invocation; permission allow rules alone do not hide other tools.
   For Claude Code 2.1.280, use an isolated profile, child-process
   `ENABLE_CLAUDEAI_MCP_SERVERS=false`, `DISABLE_DOCTOR_COMMAND=1` and settings `disableClaudeAiConnectors: true`,
   `disableBundledSkills: true`; require exactly the 18 plugin MCP tools, Skill and six plugin
   skills. These are [documented connector controls](https://code.claude.com/docs/en/env-vars)
   and [skill controls](https://code.claude.com/docs/en/skills), not changes to the user's normal
   profile. Fail the isolation check if unrelated tools/skills remain. Do not provide the replay implementation or a
   prebuilt session. Give it the matching workflow description above and the input filenames.
   Use the canned transcript on machines without speech recognition; disclose that in the result.
4. Run the server through this same relay to record comparable wire messages:
   `python tests/agent_costs.py --relay --engine <absolute-engine> --output <workflow-workspace> --wire <external-log.jsonl>`.
   Record the model provider's actual input/output token usage separately, including initialization
   and tool schemas. Count failures and repairs, not just the final successful sequence.
5. Independently check requested edit decisions, preserved sources, delivered frames/audio, captions,
   reviewed output, undo/range behavior or OTIO handoff as appropriate. Record incomplete tasks and
   unsupported steps as such. Summarize completion, tool calls, tokens and failures in this document.

The user approved the final talking-head trial targets: at most **15 calls**, at most **40,000 tokens
of maximum input context per request**, and **no schema lookup** in a skill-driven run. The token
metric was clarified on 13 October 2026. Historical entries below retain their original cumulative
accounting; the dated acceptance update applies the approved metric without changing measurements.

## Stage 4 font discovery

`fonts.find` adds one read-only tool to the full catalog (85 total); the default core catalog still
has 18 tools and routes discovery through `cutbolt_run`. At that checkpoint its listing was 21,231 bytes without
a workspace and 21,073 bytes with one: 30 bytes above stage 2, within the unchanged 40,000-byte
ceiling. Fonts remain external, with exact content identities resolved before queueing. See
[FONTS.md](FONTS.md) and the synthetic `fonts` fixture. Placement
and scene shorthand remain separate unfinished stage-4 work; this does not establish the final
model-driven title workflow or talking-head token target.

## Stage 4 built-in template checkpoint

`templates.list` adds a read-only catalog of 20 original graphics, and `graphics.instantiate`
accepts either a built-in ID or an inline recipe. Output bindings and optional target-project
fitting remove manual canvas, scale and frame-clock edits. Fonts remain external. See
[TEMPLATES.md](TEMPLATES.md) for fitting and exact timing rules.

The default catalog still has 18 tools; the full catalog has 86. Compact JSON-RPC listing bytes
(including the final newline) are 21,248 without a workspace and 21,090 with one, an increase
of 17 bytes over font discovery and within the unchanged 40,000-byte ceiling. Full listings
are 225,000 / 231,141 bytes respectively. The replay retains the original title recipe so this
checkpoint does not claim a two-call title workflow or fresh-agent token reduction. One-job
placement and scene shorthand remain to be implemented and measured.

The five existing scripted replays remain at 15/6/12/8/8 calls, 8,850/4,223/7,920/2,320/2,351
sent bytes, no failed calls and zero schema lookups. Received bytes were
209,520/46,599/62,656/40,792/185,653 on Windows and
209,388/46,473/62,526/40,666/185,523 on Linux. These include the updated catalog and retain all
previous budgets. Raw replay messages remain external; byte counts are not model token counts.

The subsequent direct-source binding checkpoint adds `media.index` to the operation catalog.
Measured current listings are 21,262 / 21,104 bytes for core (without/with a workspace), and
226,394 / 232,535 for full, with 18/86 tools and unchanged ceilings. The core increase is 14
bytes. The five scripted workflows remain at 15/6/12/8/8 calls with zero failures or schema
lookups; received bytes on this checkpoint are 209,534/46,613/62,670/40,806/185,667 on Windows
and 209,408/46,489/62,544/40,682/185,541 on Linux. These remain scripted byte measurements.

## Generated graphics placement update, 13 October 2026

New `graphics.place` requests save a scene source directly and create no movie.
Actual older compiled placements retain their exact receipt replay, including
after undo. Both platforms pass the placement oracle, cancellation and concurrent
edit checks. See [evidence](../verification/generated-graphics-2026-10-13.json).

The scripted tutorial remains 10 calls and 3,619 sent bytes,
with 53,724 / 53,598 received bytes on Windows / Linux.
Its title portion remains two calls / 775 sent bytes with no schema
lookup. The default tool listing is 22,367 bytes. This updates the
placement behavior; the dated measurements below describe earlier checkpoints.

## Stage 4 queued graphic placement, 12 October 2026

`graphics.place` takes the built-in template, typed values and saved revision in one queued
request, then returns the revision containing the compiled asset and placed clip. The tutorial
replay now uses a built-in title fitted to its project instead of sending and manually rewriting
an inline scene. The user task remains a one-second title over the edited tutorial.

| Tutorial replay | Calls | Sent bytes | Received bytes, Windows / Linux |
| --- | ---: | ---: | ---: |
| Direct-source checkpoint, explicit title workflow | 12 | 7,920 | 62,670 / 62,544 |
| Queued built-in placement | 10 | 3,619 | 53,287 / 53,177 |

The measured title portion now takes **2 calls / 775 sent bytes** through the SDK, with no schema
lookup. Its independent placement fixture uses 833 bytes on Windows and 830 on Linux (different
IDs/paths and JSON-RPC serialization), also for two calls. The default tutorial ceiling tightens
to 10 calls, 4,000 sent bytes and 60,000 received bytes; full-catalog historical ceilings stay.
The other four workflows retain their call counts, with no failed calls or schema lookups.
Core listing size in the SDK run is 21,198 bytes with a workspace, within the 40,000-byte bound;
the catalogs still have 18 core / 86 full tools.

Placement is verified against the explicit instantiate/render/apply workflow over every picture
and stereo sample of an original two-second source. Other checks cover undo/restore, repeat and
concurrent requests, intervening edits, locks, overlaps, bounds, rendering failure, cancellation,
preserved source identities and unchanged history integrity. See [TEMPLATES.md](TEMPLATES.md)
for durable generated-asset ownership and interrupted-job recovery.

Scene shorthand and fresh-agent/model token measurements remain open. These replay bytes do
not establish the approved talking-head received-token target or a comparative engine lead.

## Stage 4 shorthand acceptance, 12 October 2026

The explicit `form:short` text/shape form now expands into ordinary scenes for inspection,
stills, queued compilation and inline templates, including project-sized placement. It covers
exact numeric seconds, hex colors, default transforms, safe-area anchors, named easings and
installed font names. The original title example is **384 bytes**, versus the earlier 6,420-byte
template. Expanded recipes retain pinned font identities and use the existing renderer.

Six synthetic acceptance groups pass on Windows and Linux, comparing 40 decoded frames plus
their complete PCM across shorthand/full, transparent/opaque, fractional-clock and placed
template cases. Scene and template digests agree, still pixels agree, invalid clocks/colors/
bounds/identities are rejected, and placement retries return the same receipt. The original
example renders. See [SCENE_SHORTHAND.md](SCENE_SHORTHAND.md) for the supported shorthand fields.

Both platforms also pass the scene, template, font, placement and agent-cost regressions.
The five replays retain 15/6/10/8/8 calls, 8850/4223/3619/2320/2351 sent bytes, zero failures
and zero schema lookups. Received bytes are 209841/46918/53491/41111/185972 on Windows and
209705/46790/53357/40983/185840 on Linux; updated initialization guidance accounts for the
increase. The default workspace tool listing stays 21198 bytes, with 18 core / 86 full tools.
No budget is relaxed. Together with two-call title placement, this satisfies stage 4's bounded
scripted acceptance. Stage 5's tested skills/plugin and independent agent token trials remain.

## Stage 5 recipe and installation checkpoint, 12 October 2026

Six original skills now cover setup/routing, talking-head recuts, tutorials, synchronized
camera edits, captions and delivery. The `skills` fixture parses and executes all 26 fenced
JSON examples, compares repeated command argument keys and checks the local plugin manifests.
The recipes take IDs and revisions from actual returned results. They include source inspection
and preserve the originals. Saved results return file receipts rather than job status/proposals;
the recipes return those results directly, with optional file handoff explained separately.

| Scripted skill recipe | Editing calls | Output frames | Schema lookups |
| --- | ---: | ---: | ---: |
| Talking head, supplied transcript, captions, export and review | 15 | 124 | 0 |
| Screen tutorial, title, loudness and export | 11 | 120 | 0 |
| Synchronized two-camera cut and export | 8 | 150 | 0 |
| Export, review and accepted-loss OTIO handoff | 9 | 150 | 0 |

These pass on Windows and Linux with original six-second sources. The camera-switch output
matches independently decoded source RGB and complete fixed-camera PCM; all recipes check
output timing, required artifacts, source preservation and saved-session integrity. The handoff
fixture explicitly accepts omission of edit links and the working transfer declaration. These
counts exclude skill-loading calls and model interaction. They are not a fresh-agent success
claim or token measurement, and do not replace the five existing SDK replay budgets.

The optional local marketplace/plugin installs in an empty profile with the documented
commands, discovers all six skills and connects to the workspace through stdio. The installed
skill/manifest bytes match the tested source. No engine, fonts or models are bundled. See
[installation and tested client version](INSTALLATION.md#install-the-optional-agent-plugin).
The fresh-model workflows 1, 3 and 5, actual token measurements and the approved talking-head
target remain open. No comparative lead or editing points are claimed.

## First fresh-plugin startup attempt, 12 October 2026

The isolated talking-head trial used the installed six-skill plugin and a pinned 10367b9
engine, original generated inputs, a supplied transcript and no existing session. The client
connected to the workspace's MCP server. Its first model request then returned `Login expired`;
the process exited with code 1 after 2.72 seconds. Provider-reported inference tokens and tool
calls were both zero, and no delivery was created. Source identities were unchanged.

This establishes a working plugin connection and a failed client startup, not a model workflow
result or a passing cost target. The complete raw startup/wire records and binary identities
are retained outside Git; the failed startup record remains separate.

A follow-up found that the empty test profile used a different authentication source from
the normal client. The normal credential source was still valid. Using it in another isolated
profile produced an actual successful model response, with no tools: 2 input tokens, 2178 cache
creation input tokens, 4 output tokens and USD 0.017512 reported by the provider. The temporary
credential copy was removed. Renewing the user's sign-in was unnecessary. This probe validates
the corrected client setup only; it is not a workflow success or part of the workflow budget.

Three fresh plugin-only trials (talking head, tutorial and handoff) now have separate original
input copies, empty sessions and a pinned 6c3447a engine. They are queued after milestone and
alpha verification so compilation cannot distort their elapsed-time records. Each records
skill-loading calls as well as editing calls, actual provider input/cache/output usage, wire
bytes, errors and source identities. Output correctness requires a separate audit. Workflows
1, 3 and 5 and the Cutbolt/plain-FFmpeg comparison still have no passing model result.

## Initial fresh-agent trials: output passes, isolation fails, 12 October 2026

All three workflows completed and passed a separate output audit on
Windows, using engine `6c3447a`, Claude Code **2.1.280** and reported model
**claude-opus-5-5[1m]**, with default model settings. Each began with a new isolated profile,
the documented installed plugin, original input copies and no session. The initial report incorrectly said only plugin tools were exposed: the full client inventory
also contained **58 unrelated connected-app tools** (77 tools total). None was invoked, but
their schemas contaminated context cost. These runs do **not** satisfy the plugin-only exit. The supplied transcript excludes recognition from this result.
Raw prompts, wire/model records, binary identities, outputs and audit records remain external
under the retained `fresh-plugin-trials-02` run. The failed startup above is retained separately.

| Workflow | All calls | Skill / MCP | Failed MCP calls | All input tokens | Input excluding cache reads | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Talking head | 21 | 4 / 17 | 2 | 1,252,335 | 79,939 | 6,094 | 61.265 |
| Tutorial | 21 | 3 / 18 | 3 | 1,690,393 | 56,341 | 6,173 | 124.610 |
| Handoff | 12 | 2 / 10 | 1 | 577,819 | 18,460 | 2,876 | 56.328 |

All have zero schema lookups. All input tokens includes the provider's input, cache creation
and cache reads across model requests, as declared before the run; repeated context therefore
counts again. The other input column is reported separately and does not replace that budget.
Provider cache reuse can cross fresh client profiles. Counts come from actual tool-use/wire
records, including failed child jobs returned by a batch wait, rather than the model's summary.
Reported list-price costs were USD 0.9957272, 0.9008384 and 0.3169998 respectively.
The talking-head trial **fails both the 15-call and 40,000-token targets**. No target changed.

Independent checks reconstruct the exact requested source-frame spans and edited PCM, verify
saved snapshot integrity, source preservation, actual H.264/AAC dimensions/rate/duration and
encoder-input hashes. Talking head retains 124 frames with the four A/B words and correctly
timed caption coverage; tutorial retains 120 frames, a one-second transparent title and measured
-18.0 LUFS narration (predeclared tolerance 0.2 LU); handoff retains 150 frames and exports only
the two accepted OTIO losses, with bt709 in its final notes. The 19, 17 and 21 automated checks
pass. The actual review sheets and a delivered second-caption frame were inspected visually;
they show the original synthetic glyphs over footage. No delivered output was manually repaired.
These six-second patterns and test glyphs do not establish human-speech or long-form editorial
quality, comparative leadership, external-editor compatibility or repeated-run reliability.

Observed repairs explain some avoidable calls: a missing export profile and a review started
before export completed; treating a workspace font filename as an installed font name; and
passing a file path to audio.inspect, which expects a mix. The font and delivery skills now
explicitly distinguish local path objects and export/review dependencies. Larger call-count
and response reductions remain necessary. The proposed plain-FFmpeg comparison, second model
and repeated trials remain open.

## Startup probes: corrected interpretation and compact bootstrap

Initial no-edit probes appeared to isolate initialization instructions: default client 56,507
input tokens, minimal system prompt 54,668, skills only 5,717, MCP only 53,352, neither 2,172,
and a shortened-instructions relay 13,637. **That causal interpretation was incorrect.**
The first and MCP-only probes also exposed the 58 unrelated connector tools, while the short
relay run did not. Tool inventories were not held constant; the 42,870-token difference cannot
be attributed to Cutbolt instructions. Raw records remain retained with this correction.

A follow-up with the changed engine and 19 visible tools used 13,746 tokens forwarding raw
messages, 13,748 reserializing identical JSON values and 13,629 additionally using the short
118-character text. Each replied READY with no tools or edits. These establish observed
startup counts only. The corrected plugin-only workflow setup explicitly disables unrelated
connectors and bundled skills and audits the full initial inventory before accepting a result.

The engine now uses brief startup instructions in both catalogs, retaining exact-time guidance
and the workspace root. Detailed workflows remain in the optional plugin and documentation;
tool arrays and command schemas are unchanged. Rust checks cap base instructions at 256 bytes
and the workspace suffix separately; the SDK replay initialization ceiling tightens from 6,000
to 1,600 bytes. The measured Windows replay is **555 initialization bytes**, versus the earlier
roughly five-kilobyte response, with the same **21,198-byte** tool list. No other budget increased.
Windows: 232 Rust tests and integration, agent ergonomics, agent costs and skills pass (59 s).
Linux: all six MCP tests plus agent costs and skills pass (13 s). Format/lint checks pass.
Fresh workflow trials using the actual changed engine are recorded separately. Neither a
confounded probe nor a run with extra tools substitutes for plugin-only acceptance. Full milestone evidence remains pinned to 338e114.

## Second non-isolated run retained, 12 October 2026

The actual changed engine and clarified skills completed the same three output audits again,
with no failed MCP calls or schema lookups. The 58 extra tools still appeared in initialization
and were never invoked. This run (`fresh-plugin-trials-03`) is also excluded from plugin-only
cost acceptance; it helped discover the isolation defect and remains retained.

| Workflow | All calls | Skill / MCP | All input tokens | Input excluding cache reads | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Talking head | 20 | 4 / 16 | 1,104,777 | 28,762 | 5,839 | 88.344 |
| Tutorial | 15 | 3 / 12 | 920,669 | 22,748 | 3,631 | 45.016 |
| Handoff | 11 | 2 / 9 | 504,318 | 17,518 | 2,510 | 30.875 |

Initial input was still 56,334 / 56,295 / 56,275 tokens despite the smaller wire response.
The independent 19 / 17 / 21 output checks and contact-sheet inspection pass; this does not
repair the isolation failure or justify attributing single-run changes to one code change.
All original sources remain unchanged. The talking-head targets remain unmet.

## Corrected startup comparison with exact inventories

The final no-edit A/B probe (`fresh-context-probe-05`) explicitly disables connectors, bundled
skills and the separately controlled built-in doctor command. Both initial inventories contain
exactly 18 Cutbolt MCP tools, Skill and the six plugin skills. Same client, prompt and installed
skill bytes: old engine 6c3447a uses **12,178 input tokens**, changed bootstrap uses **11,646**,
a reduction of **532 tokens (4.4%)**. Both reply READY with no tool calls. This is the supported
bootstrap effect; the earlier 42,870-token attribution is withdrawn.

The first isolation correction (`fresh-plugin-trials-04`) still exposed the built-in doctor
skill. Its exact-inventory assertion rejected the talking-head run and stopped the batch.
It is retained separately and does not count as a valid plugin-only workflow. The corrected
six-skill trial uses the same prompts, original inputs and unchanged budget accounting.

## Valid plugin-only workflows, 12 October 2026

`fresh-plugin-trials-05` passes exact initialization checks: **18 Cutbolt MCP tools plus Skill,
six Cutbolt skills**, no other tools/skills. All actual invocations use that plugin. Each task
starts a new profile and saved project with the same original inputs and prompt as before.
The frozen executable reports d6f2367; its identity and retained source patch are recorded in
the [machine-readable summary](../verification/agent-cost-trials-2026-10-12.json). Claude Code
2.1.280 reports claude-opus-5-5[1m]. No shell, schema lookup or manual output repair was used.

| Workflow | All calls | Skill / MCP | Failed calls | Total input with cache rereads | Peak request input | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Talking head | 19 | 4 / 15 | 0 | 384,627 | 34,247 | 5,735 | 73.781 |
| Tutorial | 16 | 3 / 13 | 0 | 307,050 | 29,561 | 3,859 | 74.828 |
| Handoff | 11 | 2 / 9 | 0 | 151,321 | 23,901 | 2,678 | 30.594 |

Input excluding cache reads is 25,102 / 20,412 / 14,740; these totals can benefit from prefix
cache reuse across profiles, so they do not replace the declared total-input accounting.
Reported list-price costs are USD 0.387293 / 0.2976916 / 0.1987322. Initial inputs are
12,007 / 11,968 / 11,947 tokens. Time and cost are single observations, not repeated-run bounds.

All **19 / 17 / 21 independent output checks pass**, including exact requested source spans,
PCM and encoder-input composition, captions/title coverage, -18.0 LUFS tutorial narration,
actual delivery timing, accepted OTIO losses and source preservation. Delivered review sheets
and actual talking-head frame 100 show the original synthetic caption/title glyphs over the
footage. The same original inputs remain intact, and no model output was repaired afterward.

This satisfies bounded completion of workflows 1, 3 and 5. It **does not yet satisfy the full
stage-5 exit**: talking head uses 19 total calls, and total processed input exceeds 40,000 under
the declared cumulative accounting. The largest request is below 40,000, but the user has
been asked to clarify which token metric the target means; no target is silently redefined.
Call reduction remains necessary either way. Recognition, long-form quality, a second model,
repeatability and comparison with plain FFmpeg remain separate unfinished measurements.

## Standalone talking-head skill, 12 October 2026

The talking-head recipe now includes setup, captions, export and review in one skill. Setup
uses the existing validated `session.create` snapshot to bind inspected footage and linked
picture/sound tracks together. This removes a separate setup edit without changing engine
interfaces or source verification. Existing-project edits still require the actual saved IDs
and revision. The modular caption and delivery skills remain independently usable.

Trial `fresh-plugin-trials-06` uses the same prompts and original inputs, frozen f5fe95f
engine and exact 19-tool/six-skill inventory. All independent 19/17/21 output checks and
visual audits pass, with zero failed calls, zero schema lookups and no manual output repair.
The [retained summary](../verification/agent-cost-trials-2026-10-12-standalone.json) records
the tested skill patch, engine identity and unchanged accounting.

| Workflow | All calls | Skill / MCP | Total input with cache rereads | Peak request input | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| talking-head | 17 | 1 / 16 | 376,172 | 33,901 | 5,577 | 55.156 |
| tutorial | 15 | 3 / 12 | 304,795 | 29,517 | 3,826 | 48.516 |
| handoff | 10 | 2 / 8 | 150,626 | 23,766 | 2,634 | 36.406 |

Talking head loads one skill instead of four. Its initial four-frame contact sheet missed
the second cue, so the agent correctly requested a denser review, adding two calls. Both
caption cues are visible in that actual-delivery sheet. The observed 17 calls still exceed
15, and cumulative input still exceeds 40,000. The scripted recipe's 14 MCP calls plus one
skill load are not substituted for the measured result. Sampling guidance is the next
improvement; additional review remains required whenever it is needed for correctness.

Windows and Linux execute all 35 fenced examples across six scripted cases, including
direct H.264/PCM input and modular caption reuse. Talking-head cases independently assemble
the requested source frames/samples and caption alpha composition and match both encoder-input
hashes exactly. These tests validate the instructions, not autonomous model cost or recognition.
No comparative benchmark, new points or full stage-5 completion is claimed.

## Caption coverage in the initial review, 12 October 2026

The standalone talking-head recipe starts with the engine's default 16-frame contact sheet
and tells the agent to check its sampling against the first/last caption intervals. Targeted
additional review remains necessary for cues a sheet misses; the call budget never excuses
skipping a quality check. Windows and Linux examples pass, and checks against their actual
review cells confirm both fixture cues have samples away from their boundaries for both
native and directly decoded footage.

Fresh trial `fresh-plugin-trials-07` keeps the original inputs, prompts and strict accounting.
All three workflows pass isolation and independent output audits; no failed calls, schema
lookups, manual repairs or source changes occur. The
[machine-readable summary](../verification/agent-cost-trials-2026-10-12-sampling.json)
records the frozen engine, tested skill patch and provider metrics.

| Workflow | All calls | Skill / MCP | Total input with cache rereads | Peak request input | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| talking-head | 15 | 1 / 14 | 313,036 | 32,221 | 4,681 | 44.359 |
| tutorial | 15 | 3 / 12 | 287,191 | 29,357 | 3,674 | 45.094 |
| handoff | 10 | 2 / 8 | 134,028 | 23,856 | 2,554 | 28.188 |

The talking-head result **meets the 15-call target**, counting skill loading. The
cumulative-input target remains open pending the requested clarification of its metric;
the peak-request value is reported separately. These are single observations from one model,
not a repeatability bound or a comparison with another editing tool. Stage 5 is not marked
complete, and no points or full milestone evidence are changed.

## Approved token metric, 13 October 2026

The user clarified that the 40,000-token limit applies to **maximum context per request**.
Evaluate the recorded provider input, cache-write input and cache-read input for each request,
then take the maximum; do not sum repeated context for this target. Cumulative input remains
reported for actual provider usage. Historical trials and excluded tool inventories are unchanged.
This is an acceptance interpretation of retained trial 07, not a new model run.

Trial 07 meets the talking-head targets: **15 total tool calls**, **32,221 peak input tokens**,
and **zero schema lookups**. Its 313,036 cumulative input tokens and 4,681 output tokens remain
visible. Tutorial and handoff also finish independently with no schema calls or errors, using
15 and 10 total calls. Their output audits pass 17 and 21 checks; talking head passes 19.
All three used only the six-skill plugin with the 18-tool MCP catalog and the client's Skill tool.

Against the stage-0 scripted baseline, talking head is 15 vs 15 calls, tutorial 15 vs 12,
and handoff 10 vs 8. The fresh counts include skill loads and the agent's choices; stage 0
counted MCP calls only and has byte measurements, not provider tokens. No token improvement
over that baseline can therefore be claimed. The fresh trial proves bounded single-run
completion on one model, not comparative superiority, recognition accuracy or repeatability.

The stage-5 audit confirms six small original skills, executable JSON with cross-skill key
checks, a local plugin/marketplace and a documented clean Windows install, plus Windows/Linux
recipe execution. The audit found and corrected two documentation details: all standalone skills now
state platform boundaries, and the router discovery description explicitly names captions,
loudness and delivery. Trial 07 used the earlier wording; its executable requests are unchanged.
The updated wording and examples pass Windows/Linux checks, and fresh trials 08/09 below
complete final acceptance. The handoff-only setup clarification changes no executable example. Release binaries and the
comparative benchmark stay separate; no editing points or generated milestone evidence change.

## Stage 5 acceptance, 13 October 2026

Fresh trial 08 uses the clarified platform/router wording and unchanged executable examples.
The client is Claude Code 2.1.280, model claude-opus-5-5[1m]. Each fresh Windows profile
installs the plugin and exposes exactly 18 MCP tools, Skill and six plugin skills. Talking
head and tutorial pass their 19/17 independent checks without failed calls or schema lookup.

The handoff agent initially guessed setup operations and recovered five failed calls. Its
valid sequential timeline exposed an auditor assumption: the auditor expected separate tracks.
The original incomplete audit is retained. An independent bounded sequential-clip interpretation
now checks the same expected picture/PCM hashes, timing, OTIO references/intervals, accepted
losses and source preservation; all 21 checks pass without changing the model's outputs.
Sequential clips inherently share picture/sound intervals, satisfying the requested linked edit.

The delivery skill now explicitly routes a missing saved project through the cutbolt setup
skill. Fresh trial 09 reruns only handoff with the same prompt and originals: ten calls,
two skill loads, no failed calls or schema lookup, and all 21 checks pass. Actual delivered
sheets for both trials were inspected, including the talking-head cues and tutorial title.

| Workflow / trial | All calls | Skill / MCP | Failed calls | Total input with cache rereads | Peak request input | Output tokens | Seconds |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| talking-head (08) | 15 | 1 / 14 | 0 | 331,682 | 32,180 | 5,320 | 55.578 |
| tutorial (08) | 15 | 3 / 12 | 0 | 306,422 | 29,677 | 3,754 | 49.797 |
| handoff (08) | 17 | 1 / 16 | 5 | 274,751 | 24,408 | 4,262 | 55.14 |
| handoff (09) | 10 | 2 / 8 | 0 | 151,589 | 24,022 | 2,598 | 32.25 |

Talking head meets **15 total calls**, **32,180 peak input tokens**
and **zero schema lookups**. Final fresh call counts are 15/15/10 versus stage-0 scripted
15/12/8; fresh counts include skill loading and autonomous choices. Stage 0 measured bytes,
so it cannot establish a provider-token baseline. Current five-workflow regression budgets pass.

Stage 5's specified bounded acceptance is satisfied. See the [requirement audit](../verification/agent-cost-stage5-2026-10-13.json),
[trial 08](../verification/agent-cost-trials-2026-10-13.json) and [handoff follow-up](../verification/agent-cost-handoff-2026-10-13.json)
for identities, cumulative usage and scope. The initial attempt stopped at credential-expiry
preflight; a separate normal-client probe renewed login before measurement. Neither it nor
the auditor correction is counted as an agent workflow. No raw media/transcripts enter Git.

These observations do not establish repeatability, speech accuracy or a comparative lead.
Release binaries remain separate work. The [two-model comparative pilot](FOOTAGE_BENCHMARK_RESULTS.md)
is now complete, with losses and limitations reported. At that checkpoint direct-footage
stage 2 had passed its full quick check and stages 3-6 remained unfinished. The
[14 October audit](plans/PLAN_COMPLETION_AUDIT.md) records subsequent acceptance.
No editing points were awarded by these agent-interface changes.

## Generated caption workflow update, 13 October 2026

The caption and talking-head skills now save native caption scenes through a JSON
operations file. Their Windows/Linux examples pass with client-side schema validation,
14 editing calls and exact picture/PCM comparisons against the compiled reference.

Fresh plugin-only trial 01 exposed a dedicated-tool schema mismatch: file operations
were accepted by the engine but rejected by the client. Recovery produced correct
output in 16 calls, with one failure and 34,107 peak input tokens. The workspace MCP
schema now advertises file/select references. Trial 02 repeats the same prompt and
original inputs: **15 total calls (one skill, 14 MCP), zero failures or schema lookups,
33,441 peak input tokens**, 323,339 cumulative input tokens including cache reads,
4,911 output tokens, 51.672 seconds. Claude Code 2.1.280 used claude-opus-5-5[1m].

All 20 independent output checks passed and the actual delivery sheet was inspected.
The [evidence](../verification/generated-caption-skills-2026-10-13.json) retains both
attempts, source/binary identities and separate preflight/harness corrections. This
is bounded single-run evidence; earlier tutorial/handoff results remain historical,
and no repeatability or provider-token reduction from stage zero is claimed.
