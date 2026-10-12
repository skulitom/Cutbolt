# Agent workflow cost measurements

11 October 2026. Measurements through stage 2 of [the agent-cost plan](plans/AGENT_COST_PLAN.md).
These are scripted recipes through official Python MCP SDK **1.12.3**, not independent model trials.
They establish regression ceilings and an input corpus; they do not prove a model can discover the recipes.

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
flag precedence, schema validation and annotation defaults.

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
   expose only the tested plugin and its skills. Do not provide the replay implementation or a
   prebuilt session. Give it the matching workflow description above and the input filenames.
   Use the canned transcript on machines without speech recognition; disclose that in the result.
4. Run the server through this same relay to record comparable wire messages:
   `python tests/agent_costs.py --relay --engine <absolute-engine> --output <workflow-workspace> --wire <external-log.jsonl>`.
   Record the model provider's actual input/output token usage separately, including initialization
   and tool schemas. Count failures and repairs, not just the final successful sequence.
5. Independently check requested edit decisions, preserved sources, delivered frames/audio, captions,
   reviewed output, undo/range behavior or OTIO handoff as appropriate. Record incomplete tasks and
   unsupported steps as such. Summarize completion, tool calls, tokens and failures in this document.

The user approved the final talking-head trial targets: at most **15 calls**, at most **40,000 received
tokens**, and **no schema lookup** in a skill-driven run. There are no completed model trials yet.

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
