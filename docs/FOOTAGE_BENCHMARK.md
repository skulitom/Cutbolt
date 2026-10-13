# Footage benchmark pilot

Protocol frozen before model runs, 13 October 2026. This is a small diagnostic comparison
of the Cutbolt plugin with an agent using ordinary FFmpeg/ffprobe and shell scripting.
It does not adopt the draft 1.0 roadmap, change G2, or replace the larger comparative suite.
The existing fresh talking-head/captions trial remains separate.

## Cases and arms

Three tasks use original twelve-second, 320 by 180, 25 fps sources. Source A is H.264;
source B is HEVC Main10. Both contain stereo 48 kHz PCM in MOV, declared BT.709 SDR,
moving original graphics, a frame counter and a time-varying tone. No recognition,
downloaded content, application internals or supplied answer scripts are involved.

1. **Trim:** keep A's interval [1,4), preserving its picture and sound.
2. **Reorder:** join A's [8,10), [1,3), [5,6), in that order, with matching sound.
3. **B-roll and inset:** use A's [2,8) with uninterrupted matching A audio. Replace the
   picture at timeline [1,3) with B's [1,3). At timeline [4,6), show B's [4,6) as an
   80 by 45 opaque inset at [230,125], over A. Use area/block averaging when shrinking.

Deliver `delivery.mp4`: H.264 High, 320 by 180, 25 fps, BT.709 SDR, software CRF 18,
stereo AAC at 48 kHz and 320 kb/s. No fades, transitions, title cards or extra tail.
The task and output requirements are identical between arms. Tool-specific setup wording
differs: Cutbolt has its six intent skills and default local MCP catalog; FFmpeg has Bash,
Read and Write, no Cutbolt or plugin skills. Both can inspect and repair their own work.
No call target is supplied to either arm. All calls, including skill loads, count.

Use the same Claude Code client, model and effort for each paired run. Run the `opus` and
`sonnet` selections three times per task and arm, recording their resolved model names.
This is 36 fresh trials. Alternate arm order within pairs. Every trial has a fresh client
profile and workspace, cold engine state, the same input bytes, a ten-minute deadline and
the same $5 client-reported spending ceiling. Record ceilings as limits, not actual charges.
Do not substitute another model after a failed run. Auth/setup failures are recorded
separately; once a model starts, failed, timed-out and incomplete trials stay in the denominator.

## Independent checks

The auditor never reads or repairs a model's editing recipe. It decodes the source pictures
and sound, selects the specified frames/samples, and implements inset averaging/composition
in NumPy. Source counter blocks identify the actual frame and source in every output frame;
inset counters are checked separately. This catches shifted cuts even when images look close.
Require the exact video frame count/rate/canvas, matching frame/source counters, an AAC
presentation duration within one 48 kHz sample, stereo/48 kHz audio, original input hashes,
and no unexpected delivery frames. Compare AAC samples over the requested duration with the
independently cut PCM: correlation at least 0.995 and RMS error at most 250 PCM16 units.
Require each decoded picture's PSNR against the independent assembly to be at least 26 dB.
Report PSNR/error distributions; the threshold permits codec and color rounding, not wrong
edits. Counter checks remain exact. Calibration must test valid outputs and deliberately
wrong timing, audio and inset outputs before accepting model results.

The numerical reference uses FFmpeg's ordinary software RGB decode of the originals.
Consequently its color/scaling comparison is a declared reference choice, not proof that
one color implementation is intrinsically better. Matched requested CRF is not matched
bitrate: report actual video bytes/bitrate and do not claim a rate-distortion lead.

## Measurements and reporting

Record exact engine/client/tool identities, prompt/skill/source hashes, requested and
resolved models, effort, complete raw traces, per-call errors and permissions, tool calls,
peak input context per model request, cumulative input including repeated cached context,
output tokens, reported cost, total wall time, and machine busy samples. Tool/render time
and first output time are reported where observable; unavailable timing is null, not zero.
Raw traces and media stay outside Git. Publish only original harnesses and bounded summaries.

Report success count with the denominator, per-task/per-model ranges and medians, and every
failure. Keep speed comparisons provisional when machine load differs. Score observed
categories as win/tie/loss without changing the draft release gate. With three repeats,
report descriptive evidence only; no statistical significance or broad reliability claim.
Freeze inputs, prompts and engine for the matrix. If a flaw invalidates a trial, retain it
and explain the exclusion before starting a new version; never replace failures silently.

The results guide the ordering and design of the remaining direct-footage stages. They do
not remove any deliverable from the active implementation goal.
