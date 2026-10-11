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
the compact-catalog measurements below supersede the listing cost; the two-call/1.5-KB title target remains unimplemented.

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
