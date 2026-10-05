# Local production contract

Prepared 2 October 2026 as draft 1 for the [YouTube pilot](../YOUTUBE_PIPELINE.md). Since 5 October 2026 the coordinator [`tools/production.py`](../PRODUCTION.md) implements it as manifest version `cutbolt-production-1`, with the [example](example.production.json) as an executable manifest; [PRODUCTION.md](../PRODUCTION.md#contract-coverage) lists what is and is not implemented. The design below still governs; where the implementation chose between options, PRODUCTION.md says which.

## Document boundaries

Keep the production manifest, PixelForge source recipes, Qwen requests/receipts, and Cutbolt native snapshots separate. Their version fields are independent. A workflow coordinator translates between public interfaces and checks declared capabilities. Unknown manifest versions, unsupported media semantics and unavailable operations fail explicitly.

| Record | Minimum contents |
| --- | --- |
| Production | Contract version, stable ID, immutable revision, parent revision, scene order, output profile, explicit roots, dependency/policy identities |
| Scene | Stable ID, versioned script, planned duration, measured asset durations, visual/narration/caption artifact references, selected timing policy |
| Artifact | Stable logical ID and immutable content identity, role, relative path beneath a declared root, SHA-256, byte count, media metadata, provenance and generation request ID |
| Generation receipt | Tool/version or exact commit, model repository/revision when relevant, input hashes, parameters/seed, output identities, elapsed time, completion state and bounded diagnostics |
| Review event | Unique event ID, reviewer type and ID, gate, exact subject/dependency identities, policy version, decision, UTC timestamp and reason/evidence references |
| Stage receipt | Idempotency key and normalized request identity, dependencies, state, output identities, associated Cutbolt project/revision/receipt when applicable |

Paths are data, never executable instructions. Production roots are absolute local paths supplied at launch; artifact paths are relative and must resolve inside their declared root after following filesystem links. Reject traversal, network sources and an output that aliases an input. Paths are not content identities. Sources, including earlier generated takes, stay immutable; replacement creates a new version. Do not silently relink changed content.

The example's `null` artifact fields mean **not generated**, and planned duration is not measured duration. Approved artifacts require real hashes and review events; placeholder values cannot pass a gate. Seeds and pinned tools improve reproducibility but do not guarantee identical neural generation across devices. Reuse a validated generated WAV instead of regenerating it to simulate a retry.

## PixelForge handoff

Prefer individual RGBA PNG frames plus an explicit ordered frame/duration list derived from the selected animation's atlas metadata. A sprite sheet or contact sheet is not a video frame. Preserve repeated animation entries, trim offsets, original canvas sizes and selected loop policy. Recover trimmed sprites onto the original canvas before composition. APNG can be added later as an independently tested input; do not infer animation from the `.png` suffix alone.

Record recipe hash and any shared palette/scene dependency hashes, PixelForge commit, chosen animation and export settings. Never send this workflow metadata as unknown fields in a PixelForge recipe. Keep source recipes editable alongside their export receipts outside the Cutbolt source tree.

A companion's numbered video-frame export is a separate future input route; the existing atlas adapter does not parse that sequence sidecar. Preserve its exact rate, first number, count and sampling/loop notes, validate every file, and derive duration from count/rate. Do not reapply the original pose durations to frames that have already been sampled. Current scene input dimensions and frame-count limits still apply. See the [numbered-image workflow requirements](../IMAGE_SEQUENCE_WORKFLOWS.md); an export option alone does not establish an implemented adapter or compatible delivery.

Initially support a declared 8-bit sRGB, straight-alpha PNG path and nearest-neighbor integer scaling. Unspecified/unsupported profiles require a visible decision or rejection. Specify compositing space and test alpha edges before declaring support. The final delivery is opaque; transparent sprites must be composited over an explicit background, never implicitly treated as black or discarded. Color conversion into a declared SDR Rec.709 delivery profile requires a tested transform, not relabeling metadata.

## Speech handoff

Each scene produces its own narration WAV and receipt. Inputs identify exact script revision/text, language, local reference audio hash, reference transcript, model revision and generation parameters. Reference voice provenance/permission is recorded locally; do not publish private recordings or transcripts in the repository.

Probe the actual generated WAV's sample rate, channel layout and sample count. Store exact source duration as samples/sample-rate. Normalize into a new 48 kHz stereo derivative with explicit resampling and mono-to-stereo policy. Define gain/fades and clipping behavior rather than silently normalizing every take. Keep the original Qwen output.

Caption text can begin from the approved script. Timings must come from manual timing or a separately selected local alignment process and be reviewed against the actual take. Do not invent word timestamps from character counts or treat the script as proof of what Qwen spoke. A changed take invalidates its timing-dependent captions even when its text is unchanged.

## Exact timing and retiming choices

Use reduced rational seconds with positive denominators and half-open intervals. Preserve source samples and source frame durations before converting to the sequence clock. At the initial 25 fps / 48 kHz profile, a video frame spans exactly 1,920 output audio samples. No floating-point accumulation of frame or scene boundaries.

PixelForge timing expressed in milliseconds becomes an exact rational number of seconds. A 100 ms hold does not fit whole 25 fps frames. Support either a strict rejection or an explicitly selected sampling rule: sample the source animation at output time `n/25` using half-open source intervals. Declare the loop/hold-last/end behavior and frame-count rounding rule; report resulting duration differences. Never round every source hold independently and accumulate drift. Start the basic fixture with multiples of 40 ms, then exercise nonaligned timings separately.

A replacement narration can change duration. The proposed choices are:

- `preserve_scene_duration`: reject overflow; any silence or held visual at the end requires a declared padding policy. Do not trim words or time-stretch speech automatically.
- `ripple_following_scenes`: shift later scene placements and their linked captions/audio; return all old/new boundaries and invalidate affected cut reviews.
- `revise_take`: leave the approved scene untouched while requesting a shorter/longer take or script edit.

These are proposed workflow semantics, not current engine options. Current Cutbolt sequences automatically ripple on duration changes; do not advertise locked-duration edits until that constraint is implemented and verified. Preserving downstream content is different from preserving its absolute timeline position.

## Reviews and invalidation

Separate stage execution (`pending`, `running`, `completed`, `failed`, `interrupted`) from review decisions (`pending`, `approved`, `changes_requested`). `stale` is derived when an approval's subject or dependency identity no longer matches the selected production version. Keep historical decisions immutable; do not rewrite them to pretend they approved a new artifact.

| Change | What must be regenerated or reconsidered | What can retain approval |
| --- | --- | --- |
| Scene script | Scene script gate, speech, captions; visual coverage review; rough/final cut | Other scene assets with unchanged dependencies |
| Same-script new voice take | Narration review, measured timing, caption alignment, mix, rough/final cut | Script and compatible visuals |
| One sprite recolor | Dependent visual exports/storyboards/composites, rough/final cut | Speech and caption timing if placement/duration stay fixed |
| Shared palette | All visuals referencing that palette and their cut derivatives | Unrelated visuals and audio |
| Scene order/duration | Affected placements, linked audio/captions and cut reviews | Unchanged source assets and their asset-specific approvals |
| Output profile or renderer version | Dependent previews, exports and technical/final review | Approved script/source assets |
| Reference voice or generation model | Narration stages that adopt the changed dependency and their derivatives | Previously approved takes explicitly retained with their old provenance |

Dependencies form an explicit acyclic graph; reject cycles and missing artifact identities. A production pointer change does not automatically invalidate every asset. Review policy changes do invalidate gates evaluated under the old policy where required by the new policy.

Review publication uses an expected production revision/dependency set, so a late approval for an old preview cannot approve the new head. A human approval requires an actual human decision; agent critique cannot manufacture one. Gate policy changes are explicit versioned project choices.

## Durable execution and resumption

Reuse Cutbolt's `session.apply` request IDs and revision checks for timeline mutations. The workflow still needs its own durable receipts for external generation, reviews and publication. Do not claim a cross-process atomic transaction across PixelForge, Qwen and Cutbolt.

Persist stage intent and request identity before starting a child process. Generate into a unique temporary destination; validate and hash complete outputs before publication. Commit the stage receipt and selected artifact references together in the coordinator's state store. After a crash, reconcile any complete-but-unrecorded artifact using its recorded identity; incomplete files never satisfy a completed stage. Do not delete orphan files automatically until ownership is established.

An exact retry returns recorded artifacts/results. A reused key with different inputs fails. On restart, reuse completed, verified stages with matching dependencies, mark dead workers interrupted and rerun only incomplete or invalidated stages. Start with restarting an entire render from a fixed plan; mid-codec resume is unnecessary. Preserve the previous approved export until a replacement is verified. Full-export rerendering is acceptable initially and must be reported honestly.

Final review binds to the exact export hash, project revision and technical-report identity. Material changes after approval require a new review; no upload or publication step is implied by export approval.
