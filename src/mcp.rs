//! Local MCP tools over newline-delimited JSON-RPC, pinned to the 2025-11-25 contract.
use crate::{Result, commands, workspace::Workspace};
use serde_json::{Value, json};
use std::{
    io::{self, BufRead, Write},
    sync::{Arc, Condvar, Mutex},
};
const MAX_LINE: usize = 4 * 1024 * 1024;
const VERSION: &str = "2025-11-25";
/// Longest edge of inline preview images: legible text at a modest image-token cost.
const PREVIEW_EDGE: u32 = 768;

pub(crate) fn description(command: &str) -> &'static str {
    match command {
        "native.import" => {
            "Validate an actual native project identity, an external public-interface transfer and its reviewed exact-build acceptance matrix. Return a proposed editing snapshot only after all nonblocking host/editorial losses are acknowledged. Does not parse the proprietary format, execute the adapter, fetch media or modify files/sessions."
        }
        "project.portable" => {
            "Validate identities and propose relative full-quality/proxy media paths under an explicit media root. Returns a snapshot and media.paths operations for saved-session preview/apply; does not copy media or write state."
        }
        "session.check" => {
            "Check the entire local editing history, revision links, snapshot/receipt checksums and stored diffs in one consistent read. Reports schema, heads and whether explicit migration is needed; does not migrate."
        }
        "session.migrate" => {
            "Explicitly migrate a checked version-1 editing store to version 2 in one transaction. Preserves snapshots, request receipts and undo/history; adds revision metadata checksums. Already-current stores return an unchanged result."
        }
        "session.backup" => {
            "Write a consistent, checked copy of the complete local editing history to a new SQLite file under an explicit output root. Preserves live edits and media; rejects existing output. Maximum store size 256 MiB."
        }
        "session.recover" => {
            "Recover an identity-bound checked history backup into a root without an existing project database or journals. Preserves backup, revisions and request receipts; does not migrate or copy media. Existing stores are never replaced."
        }
        "interchange.import" => {
            "Inspect an identity-bound local OTIO editorial document using explicit local media bindings. Returns a proposed native snapshot only after every reported loss is acknowledged; unsupported structures block conversion. Does not fetch URLs, write files or modify saved sessions."
        }
        "interchange.export.inspect" => {
            "Inspect a flat native timeline for bounded OTIO export. Returns the proposed document and path-specific losses without writing. Requires identity-bound local sources."
        }
        "interchange.export" => {
            "Publish a new bounded OTIO editorial document under an explicit output root. Requires exact acknowledgements for reported losses, rejects unsupported structure and existing outputs, and preserves media and saved sessions."
        }
        "transcript.inspect" => {
            "Validate a content-bound local transcript and its source identity. Report exact absolute source word times and estimated boundaries without writing files or saved state."
        }
        "transcript.correct" => {
            "Return a new transcript revision from explicit word text and timing corrections, preserving source and recognition provenance. Requires the expected document fingerprint; no files or saved state change."
        }
        "transcript.plan" => {
            "Plan selected-word cuts in a bound native media clip. Report clock rounding, estimated boundaries, collateral words and retained source spans. Returns a content-bound operation for session preview/apply; no files or saved state change."
        }
        "audio.inputs" => {
            "List active local capture endpoints and observed formats without starting capture or changing device settings."
        }
        "audio.record.inspect" => {
            "Validate an explicit local input selection and recording duration without starting capture. Record through the blocking CLI/library audio.record command."
        }
        "audio.record.place" => {
            "Inspect a bound recorded WAV and return validated native audio-track placement operations with an explicit latency shift. Apply through the existing saved-session workflow; this inspection writes no files or session state."
        }
        "audio.repair.inspect" => {
            "Evaluate a bounded local PCM cleanup recipe using explicitly selected noise-only regions, optional DC removal and ordered effects. Reports noise profiles, unchanged sample timing, PCM identity and meters without writing. Run audio.repair.render through CLI/library to publish a new WAV."
        }
        "stabilization.inspect" => {
            "Measure declared camera patches and return editable motion compensation, cut-aware smoothing and explicit crop decisions without writing files."
        }
        "reframe.inspect" => {
            "Inspect explicit subject boxes or local tracking and return every constrained crop decision plus an editable scene without writing files or saved state."
        }
        "tracking.inspect" => {
            "Measure a bounded source patch trajectory and return an editable masked scene; reject uncertain matches without writing files."
        }
        "sync.inspect" => {
            "Inspect identity-bound reference assets using declared timecode or bounded audio correlation windows. Report exact clock mapping, confidence, drift and source-preserving correction recipes. Read-only; run returned media.conform recipes explicitly to new outputs, then register their bound assets for multicam editing."
        }
        "hdr.inspect" => {
            "Inspect an identity-bound high-bit-depth/HDR conversion recipe, explicit color and display interpretation, tone policy and exact frame mapping. Read-only; hdr.conform writes a separate HDR intermediate or SDR editing asset through CLI/library calls."
        }
        "lut.inspect" => {
            "Read an identity-bound bounded .cube LUT, report its domains/interpolation and evaluate up to 256 sample colors. Read-only. Apply the LUT through media.conform with explicit SDR normalization."
        }
        "scopes.inspect" => {
            "Compute exact per-channel/luma histograms, waveform/parade and a chroma vectorscope for one full-quality timeline frame. Explicit color interpretation and missing-tag policy required; proxy selection is ignored. No files or saved state change."
        }
        "export.inspect" => {
            "Inspect an exact timeline range and full-quality sources for reference, lossless PNG movie/image-directory or H.264/AAC export. Lossless sequential output supports native clocks; H.264 and placed tracks retain 25 fps. PNG/H.264 video needs input_transfer unless the project declares its transfer with the project.transfer operation; sequence_first selects image numbering, h264 selects delivery controls and aac_bitrate selects audio rate. Returns timing, source identities and color/publication policy without writing. Run the export with job.start run export.run."
        }
        "effects.preset" => {
            "Return an independent editable scene effect chain from an original green/blue hard/soft chroma-key preset. Explicit strength and spill controls must be 0..1000. Copy effects into a layer, inspect the scene, then compile it. No files or saved state are changed."
        }
        "captions.import" => {
            "Import a bounded UTF-8 SRT or supported WebVTT file by source identity. Returns an editable caption document and exact timing/overlap inspection. Unsupported markup and settings fail explicitly; sources remain unchanged."
        }
        "captions.inspect" => {
            "Validate an immutable caption document and inspect exact cue timing, styles and simultaneous-cue count. No file or saved-state changes."
        }
        "captions.apply" => {
            "Apply a revision-guarded atomic batch to a caller-owned caption snapshot. Add/replace/remove cues, shift exact times and edit styles/overlap policy. Returns a new document and cue diffs; does not save a session."
        }
        "captions.encode" => {
            "Preview UTF-8 SRT or WebVTT text and per-cue loss reports without writing files. Export times must align to milliseconds; no implicit rounding."
        }
        "captions.export" => {
            "Write a new bounded SRT or WebVTT file under an explicit output root. Rejects existing output and lossy conversion unless allow_reported was explicitly selected after inspecting captions.encode."
        }
        "transcript.assemble" => {
            "Paper edit: turn transcript word selections (document, first and last word) into clip.append operations for a sequential timeline, in order, each covering its words' source plus optional padding, widened to whole frames so no word is clipped. The batch is checked to apply. Read-only; apply with session.apply, then promote to tracks for music and overlays."
        }
        "transcript.fillers" => {
            "Remove filler words: find listed words (default um, uh, erm, er, ah, uhm, umm, hmm, mm) wherever the timeline speaks them, from transcripts of its sources, and propose ripple deletions that cut them out without entering the neighbouring words, across every track; consecutive fillers make one cut. Each cut is checked on a working copy. Read-only; apply the operations with session.apply."
        }
        "captions.draft" => {
            "Draft captions for a timeline from transcripts of its sources: the whole words inside audible audio clips, at their timeline times, grouped into cues that break at pauses, sentence ends, a maximum duration and the line budget, with balanced lines and millisecond times ready for captions.export (SRT/WebVTT) or captions.scene. Read-only."
        }
        "captions.render" => {
            "Render a whole caption document over a timeline as one transparent overlay asset for an alpha_over video track, however long: it is compiled as caption windows of at most ten seconds and joined losslessly. Run it with job.start; media.add the returned asset and place it on an alpha_over track."
        }
        "captions.scene" => {
            "Append caption layers to a supplied scene using explicit per-style fonts/boxes and exact frame-start sampling. Reports unsampled and out-of-window cues. Returns an editable scene and inspection; does not render or save it."
        }
        "graphics.instantiate" => {
            "Expand an original scene template using explicit typed parameters. Validates bindings, complete scene semantics and external source identities; returns a fresh editable scene and inspection. No files or saved sessions are changed. Render the returned scene through the blocking CLI/library."
        }
        "proxy.status" => {
            "Check proxy availability and identities. Preview selection is saved with preview.proxy through session.apply; final exports always use full-quality media."
        }
        "proxy.relink" => {
            "Find an unambiguous proxy by bound content identity and return a proposal for session.apply. Does not move files or save edits. Proxy generation is a blocking CLI/library command."
        }
        "media.conform.inspect" => {
            "Validate a supported source identity, codec/container and exact timestamp mapping into a 25 fps editing asset. Reports constant or variable-speed source clocks, frame selection/blend weights and explicit audio pitch policy; media.conform rendering uses the blocking CLI/library."
        }
        "audio.inspect" => {
            "Validate and evaluate a bounded PCM mix: sample-aligned trims, gain/fade automation and multiple tracks, plus optional buses, explicit channel matrices, panning and named surround layouts. Reports output samples, PCM hash, layout, routing and clipping counts. Read-only; WAV rendering uses the blocking CLI/library command audio.render."
        }
        "registry.search" => {
            "Search project assets by text, bin prefix and exact tags. Stable ID order and pagination; content duplicates are reported without merging assets."
        }
        "registry.status" => {
            "Check local source availability and bound SHA-256 identities within an explicit input root. Read-only; reports missing, changed, unbound or unavailable media."
        }
        "registry.bind" => {
            "Read and hash selected sources, returning an updated snapshot and media.bind operations. Persist operations with session.apply at the original expected revision. Does not save or modify sources."
        }
        "registry.relink" => {
            "Match explicit candidate paths to a bound asset identity. Reject ambiguous or mismatching content. Returns an updated snapshot and operations for session.apply; does not save or move media."
        }
        "image.sequence.inspect" => {
            "Validate every numbered PNG identity and exact source window/repeat, alpha association, color interpretation and native output clock. Returns the explicit transparent or opaque compilation policy without writing. image.sequence.compile is blocking CLI/library-only."
        }
        "expression.inspect" => {
            "Evaluate a bounded typed property graph at explicit rational times. Reports exact values and rounded position/opacity bindings, with property links, cycle/type checks and deterministic seeded operations. Reads no media, executes no source code and writes no files."
        }
        "scene.inspect" => {
            "Validate a bounded pixel scene, source identities, rational frame selection, text layout, sampled grading/selection/keying parameters and WAV conversion. Read-only; returns explicit color/timing/audio policies. scene.render is available through the blocking CLI/library, not this MCP connection."
        }
        "cache.inspect" => {
            "Inspect the owned local cache, entry identities, logical and physical sizes without generating media."
        }
        "cache.prune" => {
            "Evict least-recently-used derived cache entries to explicit byte/count budgets; source media and exported files are not removed."
        }
        "scene.still" => {
            "See a scene before compiling it: render the one frame shown at time (default 0) to a new PNG at the scene's output size, returned as an inline image; straight RGBA for a transparent scene. Use it to check titles, lower thirds and thumbnails, then compile with scene.render."
        }
        "preview.frame" => {
            "Export one exact frame from a reference project timeline to an unused PNG and return it as an inline image. Requires absolute input/output roots and rational time on the project's supported native frame boundary. No source or existing output is overwritten."
        }
        "capabilities" => {
            "Discover implemented editing operations, render profile, job limits and unsupported features. Local only."
        }
        "project.create" => {
            "Create an empty versioned project snapshot. Does not save it. Times/frame rates use exact {num,den} rational values."
        }
        "project.validate" => {
            "Validate a supplied snapshot and return its revision and exact duration. No state changes."
        }
        "timeline.apply" => {
            "Transform a caller-owned snapshot with one atomic batch; returns a new snapshot. tracks.edit supports placed video/audio tracks, links, locks and explicit collisions. For saved edits and safe retries use session.apply instead."
        }
        "session.create" => {
            "Start a saved project at revision 0, from id/width/height/frame_rate or from a full snapshot. store_root must be an existing absolute local directory. Retry with the same request_id and arguments to retrieve the original receipt."
        }
        "session.get" => {
            "Read the saved head or a chosen immutable revision. Returns the full project. Get the head before editing; use a receipt revision for rendering."
        }
        "session.apply" => {
            "Commit an atomic editing batch against expected_revision. Reuse request_id only with identical arguments after a lost response. On REVISION_CONFLICT read and reconsider; returns a receipt with semantic changes."
        }
        "session.preview" => {
            "Inspect exact before/after clip placements, track layout, links and affected durations without saving. This is an edit diff, not a video image. Apply against the same revision or handle a conflict."
        }
        "session.undo" => {
            "Undo the latest applied state as a new revision, preserving history. Repeated undo walks backward. Retry with identical request_id/expected_revision."
        }
        "session.restore" => {
            "Restore target_revision as a new undoable revision. Use history to choose a state. Reuse request_id only for an identical retry."
        }
        "session.receipt" => {
            "Look up whether a request ID was committed, without resending its arguments. Returns the original receipt, or REQUEST_NOT_FOUND if it never took effect. Use after a lost response or restart before deciding to retry."
        }
        "session.history" => {
            "Read newest-first revision summaries. limit defaults to 50 (1-200). Pass next_before_revision as before_revision for the next page; null means finished."
        }
        "media.inspect" => {
            "Probe a local source inside an explicit absolute input_root. Returns its content identity (path relative to input_root, SHA-256, bytes), stream metadata and `timeline`: whether the file can go on a timeline as it is, with its exact frame rate, frames, rational duration and a media.add-ready asset, or the reasons it cannot and, for video, a media.conform recipe and output to run with job.start."
        }
        "render.plan" => {
            "Validate the narrow reference media profile and inspect an export plan. Requires absolute input/output roots and an unused .mkv output. May take time to decode source metadata."
        }
        "render.start" => {
            "Submit an asynchronous local Windows render. render contains a fixed snapshot and absolute input/output roots/path, with optional retry.max_attempts (1..3 total). Retry applies only to transient tool failures and interruptions. Reuse request_id only with identical arguments. Returns a durable ticket; poll job.status."
        }
        "job.status" => {
            "Read saved state, frame progress, attempt history and completion/error receipt. An exited worker triggers checked publication reconciliation or an opted-in retry within its saved limit. This can update internal job metadata but does not start a worker."
        }
        "job.cancel" => {
            "Request cancellation of a queued/running job without deleting final output. Poll until cancelled or completed; publication can win a simultaneous cancellation. Safe to repeat for the same job."
        }
        "job.resume" => {
            "Reconcile saved publication and wake a Windows worker to drain queued jobs, including explicitly opted-in interrupted retries within their saved attempt limit. Completed, cancelled and exhausted jobs are not rerun."
        }
        "timeline.meters" => {
            "Measure a timeline range's loudness without exporting: sample peak, RMS and BS.1770 integrated loudness (LKFS) of the mix of enabled audio tracks and, by default, of each enabled audio track alone. Use it to set clip gain_milli against a target such as -14 LKFS. With curve, also loudness per second and the silent and clipped runs, to find dead air, music that buries speech, or distortion. Read-only; at most 4 hours per call."
        }
        "timeline.check" => {
            "Find likely mistakes without rendering: flash frames, out-of-sync picture and sound, jump cuts, black or silent stretches, disabled-track clips, unused assets, missing or changed media (with input_root) and words cut by clip edges (with transcripts). Returns a text summary and findings; ok means no warnings or errors. Read-only."
        }
        "timeline.outline" => {
            "Read a timeline as compact text, one line per clip (spans, ID, source, link, levels, picture-in-picture), plus transitions and black or silent stretches. With transcripts of the sources, each audio clip shows what is said, a cut word marked *. start and end page through long timelines. Read-only."
        }
        "files.list" => {
            "List files and folders under input_root (the workspace by default), sorted, with sizes and paths relative to it, optionally recursive and filtered by extension; engine state folders are skipped. Read-only."
        }
        "job.start" => {
            "Queue a long-running command in the background and return a durable ticket: export.run (H.264/AAC or lossless delivery), export.review (a review folder for a rendered cut), captions.render (a whole caption track as one transparent overlay), media.transcribe (a whole file's speech into transcripts, or a known script aligned to it), media.prepare (any camera or phone file to a timeline asset, optionally for a project's rate and size), media.conform, scene.render, audio.render, audio.repair.render, hdr.conform, image.sequence.compile, proxy.generate, preview.range, cache.run or transcript.transcribe. Arguments are prepared and validated now. Follow with job.wait or job.status; the result holds the command's receipt. Cancellation stops a queued job; a running one finishes."
        }
        "job.wait" => {
            "Wait up to timeout_seconds (default 30, at most 120) for a queued or running job to finish, then return its status, progress and result, with finished true or false."
        }
        "schema" => {
            "Return the JSON Schema for one command's arguments, including CLI-only commands, or for a shared type that tool listings abbreviate: project, operation, scene, template, audio_routing or transcript. Large schemas come back as an outline of variants and definitions; pass select (for example operation + clip.append, or scene + Layer) for one part with everything it references. Read-only."
        }
        "image.sequence.compile" => {
            "Compile a validated numbered PNG recipe into a transparent lossless movie or an explicitly flattened native editing asset at an unused output path. Blocking CLI/library command."
        }
        "cache.run" => {
            "Produce or reuse one content-checked cached probe, proxy, frame, interval or contact sheet under an explicit cache root and byte/entry policy. Blocking CLI/library command."
        }
        "preview.cuts" => {
            "Review the edit: write a sheet of the last frame before and first frame after each cut (16 per page, two cuts per row), return it as an inline image, and list each cut's time, clips and source times. Continue with `next`."
        }
        "audio.duck" => {
            "Lower music under speech: analyze the voice track alone in 10 ms windows and propose gain curves for the music track's clips (ramp down before speech, hold, ramp back up after), as clip_audio operations to apply with session.apply. Read-only; nothing is changed until the operations are applied."
        }
        "export.review" => {
            "Review a rendered cut into a new folder: a contact sheet, a small H.264 copy to watch, loudness over time with silence and clipping, black frames, the duration against the project, and the words the timeline should say (from source transcripts) against the words heard in the cut (from a speech runtime or given transcripts). Run it with job.start; the result's summary is a short text report."
        }
        "audio.tighten" => {
            "Jump cuts: find pauses in speech (the voice track, or a sequential timeline's program) and propose ripple deletions of each pause's middle, keeping some silence, across every track. Each cut is checked on a working copy; refused cuts are listed with the reason. Read-only; apply the operations with session.apply."
        }
        "color.match" => {
            "Match one camera's colour to another's: sample frames of a reference and a target shot, build a per-channel 1D LUT (levels: mean and spread; histogram: whole distributions), write it as a new .cube and return the media.conform recipe that bakes it into a new target asset, with channel statistics before and after."
        }
        "audio.beats" => {
            "Find the beat of a music file for cutting to it: onsets (sudden rises in 10 ms loudness), the tempo in BPM (the metrical level nearest 120 BPM; other levels, such as half time, in tempo_alternatives), and a beat grid on the onsets, as exact file times and, with frame_rate, the nearest frame of each beat. Read-only."
        }
        "audio.normalize" => {
            "Bring the whole mix to a loudness target such as -14 LKFS: measure the timeline's integrated loudness and sample peak, then propose clip_audio operations that scale every audio clip's level (and gain curve) by one factor, stopping at a peak ceiling (default -1 dBFS) or the clip gain range. The proposed levels are measured before they are returned (two to four renders of the mix), so result is what applying them gives. Read-only; apply the operations with session.apply."
        }
        "media.transcribe" => {
            "Recognize a whole source file's speech with the local speech runtime, any length and any decodable format: its audio is extracted losslessly, recognized in overlapping 120 s windows, stitched at word boundaries into non-overlapping transcript documents bound to the source file, and saved as one JSON file. Sound that is not speech, such as [Music], is reported apart from the words. Given text (the script of a synthesized narration, up to 120 s), its words are aligned to the audio instead of recognized, keeping names and spelling exact. Run it with job.start."
        }
        "media.prepare" => {
            "Turn any decodable video file, such as a phone or camera MP4, into a timeline asset in one step: a ready file that fits is returned as it is; anything else is converted with the readiness recipe at the project's rate and size, or at the source's own rate. With paths, several files are prepared in one job and the result includes their media.add operations. Run it with job.start; the result's asset goes to media.add."
        }
        "media.sheet" => {
            "See a source file without adding it to a project: a sheet of frames at given times or spread evenly through it, from any format FFmpeg decodes, returned as an inline image with each cell's time."
        }
        "media.shots" => {
            "Log footage: find the shot boundaries of a source file and list each shot's start, end and cut score; with output, also write a sheet of each shot's middle frame, returned as an inline image."
        }
        "preview.sheet" => {
            "Write a contact sheet of exact timeline frames, laid out on a grid, to an unused PNG, and return it as an inline image so the edit can be seen. Uses the saved proxy selection."
        }
        "transcript.transcribe" => {
            "Run optional local speech recognition through the explicitly configured external runtime and return a content-bound transcript document; with text, align those known words instead. Blocking CLI/library command."
        }
        "audio.record" => {
            "Capture an explicitly selected local input to a new 48 kHz stereo PCM16 WAV for the requested duration. Blocking CLI/library command."
        }
        "audio.repair.render" => {
            "Publish the inspected dialogue-cleanup recipe as a fresh verified PCM WAV at an unused output path. Blocking CLI/library command."
        }
        "hdr.conform" => {
            "Convert an inspected high-bit-depth/HDR recipe into a tagged 16-bit intermediate or an explicitly tone-mapped 8-bit SDR editing asset at an unused output path. Blocking CLI/library command."
        }
        "export.run" => {
            "Export an inspected timeline range as reference, lossless PNG or H.264/AAC output at an unused path, validating timing and decoded counts. Blocking CLI/library command."
        }
        "proxy.generate" => {
            "Generate an identity-bound half, quarter or eighth-size preview variant of a reference asset with unchanged timing and audio. Blocking CLI/library command."
        }
        "media.conform" => {
            "Convert a supported source into a new verified reference editing asset using an inspected conform recipe. Sources are preserved. Blocking CLI/library command."
        }
        "audio.render" => {
            "Render an inspected PCM mix, including optional routing to named layouts, to a new verified WAV. Blocking CLI/library command."
        }
        "render.run" => {
            "Render a reference project synchronously to an unused FFV1/PCM .mkv and verify decoded frame and sample counts. Blocking; use render.start for background work."
        }
        "scene.render" => {
            "Compile an inspected pixel scene to a new reference editing asset at an unused output path. Blocking CLI/library command."
        }
        "preview.range" => {
            "Export an exact timeline interval as a reference .mkv preview, following the saved proxy selection. Blocking CLI/library command."
        }
        _ => UNDESCRIBED,
    }
}
pub(crate) const UNDESCRIBED: &str = "Unsupported command";

/// Long renders belong in persisted jobs so the MCP connection stays usable.
const BLOCKING: [&str; 17] = [
    "media.prepare",
    "media.transcribe",
    "captions.render",
    "export.review",
    "image.sequence.compile",
    "cache.run",
    "transcript.transcribe",
    "audio.record",
    "audio.repair.render",
    "hdr.conform",
    "export.run",
    "proxy.generate",
    "media.conform",
    "audio.render",
    "render.run",
    "scene.render",
    "preview.range",
];

/// Commands whose results are documents an agent passes on; with a workspace they accept save_as.
const DOCUMENTS: [&str; 22] = [
    "audio.duck",
    "audio.normalize",
    "audio.tighten",
    "transcript.fillers",
    "transcript.assemble",
    "transcript.plan",
    "job.wait",
    "captions.import",
    "captions.draft",
    "captions.apply",
    "captions.scene",
    "graphics.instantiate",
    "tracking.inspect",
    "stabilization.inspect",
    "reframe.inspect",
    "scene.inspect",
    "timeline.apply",
    "session.get",
    "interchange.import",
    "native.import",
    "project.portable",
    "transcript.correct",
];

/// The everyday tools a compact catalog (`mcp --tools core`) lists in full; `cutbolt_run` runs
/// every other tool command by name.
pub(crate) const CORE: [&str; 31] = [
    "capabilities",
    "schema",
    "files.list",
    "project.create",
    "session.create",
    "session.get",
    "session.apply",
    "session.preview",
    "session.undo",
    "session.history",
    "media.inspect",
    "media.sheet",
    "preview.frame",
    "preview.sheet",
    "preview.cuts",
    "timeline.outline",
    "timeline.check",
    "timeline.meters",
    "job.start",
    "job.wait",
    "job.status",
    "transcript.assemble",
    "transcript.fillers",
    "captions.draft",
    "audio.duck",
    "audio.normalize",
    "audio.tighten",
    "audio.beats",
    "scene.inspect",
    "scene.still",
    "export.inspect",
];

/// The compact catalog: the core tools, plus `cutbolt_run` for every other tool command.
pub fn core_tools(workspace: Option<&Workspace>) -> Vec<Value> {
    let all = tools(workspace);
    let others: Vec<String> = crate::schema::commands()
        .filter(|c| exposed(c) && !CORE.contains(c))
        .map(str::to_owned)
        .collect();
    let mut listed: Vec<Value> = all
        .into_iter()
        .filter(|t| {
            t["name"]
                .as_str()
                .and_then(|n| n.strip_prefix("cutbolt_"))
                .is_some_and(|n| CORE.contains(&n.replace('_', ".").as_str()))
        })
        .collect();
    listed.push(json!({"name":"cutbolt_run",
        "description":format!("Run any other Cutbolt tool command by name, with its arguments exactly as for a direct call (cutbolt_schema {{\"name\": command}} returns their schema): {}. Long-running commands still go through job.start.", others.join(", ")),
        "inputSchema":{"type":"object","additionalProperties":false,"required":["command"],"properties":{
            "command":{"type":"string","enum":others,"description":"Command to run."},
            "arguments":{"type":"object","description":"The command's arguments, without command; default none."}}},
        "annotations":{"readOnlyHint":false,"destructiveHint":true,"idempotentHint":false,"openWorldHint":false}}));
    listed
}

/// Whether a command is offered as an MCP tool.
pub(crate) fn exposed(command: &str) -> bool {
    !BLOCKING.contains(&command)
}

/// The tool catalog. With a workspace, the roots it supplies are optional in every schema.
pub fn tools(workspace: Option<&Workspace>) -> Vec<Value> {
    crate::schema::commands().filter(|c| exposed(c)).map(|command| {
        let mut input = crate::schema::arguments(command, true).expect("command schema");
        if workspace.is_some() {
            crate::schema::relax_roots(&mut input);
            if DOCUMENTS.contains(&command) {
                input["properties"]["save_as"] = json!({"type":"string","description":"Write the result to this new .json workspace file and return a summary; later arguments read it as {\"file\": path, \"select\": field or dotted.path}."});
            }
        }
        let read_only=matches!(command,"timeline.check"|"transcript.assemble"|"transcript.fillers"|"audio.beats"|"captions.draft"|"audio.tighten"|"audio.duck"|"audio.normalize"|"timeline.meters"|"timeline.outline"|"files.list"|"schema"|"expression.inspect"|"native.import"|"image.sequence.inspect"|"project.portable"|"session.check"|"interchange.import"|"interchange.export.inspect"|"cache.inspect"|"transcript.inspect"|"transcript.correct"|"transcript.plan"|"audio.inputs"|"audio.record.inspect"|"audio.record.place"|"audio.repair.inspect"|"stabilization.inspect"|"reframe.inspect"|"tracking.inspect"|"sync.inspect"|"hdr.inspect"|"lut.inspect"|"scopes.inspect"|"export.inspect"|"effects.preset"|"captions.import"|"captions.inspect"|"captions.apply"|"captions.encode"|"captions.scene"|"graphics.instantiate"|"proxy.status"|"proxy.relink"|"media.conform.inspect"|"audio.inspect"|"registry.search"|"registry.status"|"registry.bind"|"registry.relink"|"capabilities"|"project.create"|"project.validate"|"timeline.apply"|"session.get"|"session.preview"|"session.history"|"session.receipt"|"media.inspect"|"render.plan"|"scene.inspect");
        let text = match workspace {
            Some(_) => crate::schema::workspace_wording(description(command)),
            None => description(command).to_owned(),
        };
        json!({"name":format!("cutbolt_{}",command.replace('.',"_")),"description":text,"inputSchema":input,
            "annotations":{"readOnlyHint":read_only,"destructiveHint":matches!(command,"job.cancel"|"cache.prune"),"idempotentHint":!matches!(command,"preview.frame"|"preview.sheet"|"captions.export"|"interchange.export"|"session.backup"|"session.recover"),"openWorldHint":false}})
    }).collect()
}
fn rpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn response(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}

#[derive(Default)]
struct Server {
    initialized: bool,
    ready: bool,
    workspace: Option<Workspace>,
    /// List the core tools and `cutbolt_run` instead of every tool.
    core: bool,
    catalog: Option<Vec<Value>>,
    /// A validated tool call left by `route` for the caller to run.
    pending: Option<Call>,
}
/// What one incoming message needs: an immediate reply, if any, or a tool call to run.
enum Step {
    Reply(Option<Value>),
    Call(Call),
}
/// A validated tool call. `serve` runs calls on worker threads, so a slow tool does not hold up
/// the others; JSON-RPC IDs pair each response with its request.
struct Call {
    id: Value,
    command: String,
    arguments: Value,
    workspace: Option<Workspace>,
    /// The request's `_meta.progressToken`, if the client asked for progress.
    progress_token: Option<Value>,
    /// Sends a notification to the client; set by `serve`.
    notify: Option<Box<dyn Fn(Value) + Send>>,
}
impl Call {
    fn run(self) -> Value {
        if self.command == "job.wait"
            && let (Some(token), Some(notify)) = (&self.progress_token, &self.notify)
            && let Some(seconds) = self.arguments["timeout_seconds"]
                .as_u64()
                .filter(|s| (1..=120).contains(s))
        {
            // Wait one second at a time and report each second, so the client sees the phase
            // and can keep a long wait alive. The final result is the same as one long wait.
            let mut slice = self.arguments.clone();
            slice["timeout_seconds"] = json!(1);
            for elapsed in 1..seconds {
                let state = Self::outcome("job.wait", slice.clone(), self.workspace.as_ref());
                let Some(job) = state.ok().filter(|job| job["finished"] != true) else {
                    break;
                };
                let progress = &job["progress"];
                let mut message = progress["phase"].as_str().unwrap_or("waiting").to_string();
                if let (Some(done), Some(total)) = (
                    progress["frames"].as_u64(),
                    progress["total_frames"].as_u64(),
                ) && total > 0
                {
                    message.push_str(&format!(", {done} of {total} frames"));
                }
                notify(json!({"jsonrpc":"2.0","method":"notifications/progress",
                    "params":{"progressToken":token,"progress":elapsed,"message":message}}));
            }
            return Call {
                arguments: slice,
                notify: None,
                ..self
            }
            .run();
        }
        let Call {
            id,
            command,
            arguments,
            workspace,
            ..
        } = self;
        let value = match Self::outcome(&command, arguments, workspace.as_ref()) {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        // An outline is text meant to be read; it is the content, and the JSON stays structured.
        let text = match value["result"]["outline"].as_str() {
            Some(outline) if command == "timeline.outline" => outline.to_owned(),
            _ => value.to_string(),
        };
        let mut content = vec![json!({"type":"text","text":text})];
        if value["ok"] == true {
            content.extend(preview_image(
                workspace.as_ref(),
                &command,
                &value["result"],
            ));
        }
        response(
            id,
            json!({"isError":value["ok"]!=true,"structuredContent":value,"content":content}),
        )
    }
    fn outcome(
        command: &str,
        mut arguments: Value,
        workspace: Option<&Workspace>,
    ) -> Result<Value> {
        let object = arguments
            .as_object_mut()
            .ok_or_else(|| crate::error("INVALID_JSON", "Tool arguments must be an object"))?;
        if object.contains_key("command") {
            return Err(crate::error(
                "INVALID_JSON",
                "Do not send command inside tool arguments",
            ));
        }
        object.insert("command".into(), Value::String(command.to_string()));
        commands::handle_json(arguments, workspace)
    }
}
/// A preview command's PNG as an image content block, downscaled to PREVIEW_EDGE.
fn preview_image(workspace: Option<&Workspace>, command: &str, result: &Value) -> Option<Value> {
    let output = match command {
        "preview.frame" | "preview.sheet" | "preview.cuts" | "media.sheet" | "media.shots"
        | "scene.still" => result["output"].as_str()?,
        // A finished export.review job shows its contact sheet.
        "job.wait" | "job.status" => result["result"]["picture"]["sheet"]["output"].as_str()?,
        _ => return None,
    };
    let output = std::path::Path::new(output);
    let path = match workspace {
        Some(workspace) if output.is_relative() => workspace.root().join(output),
        _ => output.to_path_buf(),
    };
    let png = crate::thumbnail::png(&path, PREVIEW_EDGE).ok()?;
    Some(json!({"type":"image","data":crate::thumbnail::base64(&png),"mimeType":"image/png"}))
}
impl Server {
    fn catalog(&mut self) -> &[Value] {
        let workspace = self.workspace.as_ref();
        let core = self.core;
        self.catalog.get_or_insert_with(|| {
            if core {
                core_tools(workspace)
            } else {
                tools(workspace)
            }
        })
    }
    /// Frame and contact-sheet previews also come back as a downscaled inline image, so a
    /// multimodal client sees the edit without opening the file.
    fn instructions(&self) -> String {
        let mut text = concat!(
            "Cutbolt is a local video editing engine; no HTTP service is used. Edits are saved sessions with revisions, durable request IDs for safe retries, previews and undo. ",
            "Typical cut: session.create with id, width, height and frame_rate (30 or 60 fps footage keeps every frame on a 30 or 60 fps project); ",
            "job.start run media.prepare with each source's path (camera or phone video, or a PCM WAV voice-over or music track for an audio track) and the project, then job.wait, gives an asset for media.add; session.apply with media.add, project.transfer once (bt709 suits most material), then clip.append, clip.insert or clip.trim; look with preview.sheet or preview.frame, ",
            "which return images; deliver with job.start run export.run (H.264/AAC) or render.start (reference), then job.wait; check the delivered file with job.start run export.review. ",
            "Titles and graphics: write a scene (cutbolt_schema scene, select Layer or Graphic), check it with scene.inspect, look at it with scene.still, compile it with job.start run scene.render, ",
            "and media.add the returned asset. Captions: captions.draft drafts cues from transcripts, or captions.import reads a file; job.start run captions.render burns a whole track into one overlay; then captions.scene onto a scene. ",
            "Music and voice levels: put clips on audio tracks (tracks.edit place) and set gain_milli, gain_curve, fade_in and fade_out with tracks.edit clip_audio; audio.duck proposes curves that lower music under speech, audio.normalize proposes levels that bring the mix to a loudness target such as -14 LKFS, audio.tighten proposes jump cuts that shorten pauses in speech, and transcript.fillers removes um and uh (lift silences them on the voice track without moving anything); start and end limit both to a window. ",
            "Picture-in-picture: place a clip on a video track with composite alpha_over and give it a transform (crop, divisor 1-8, opacity, position) with tracks.edit clip_transform. ",
            "Review your work: timeline.check lists likely mistakes without rendering; timeline.outline reads the whole cut as text, one line per clip, with what is said in it when given transcripts; preview.cuts pages through every cut as before/after images; media.sheet and media.shots show and log source footage before it is added; timeline.meters with curve finds dead air, buried speech and clipping. ",
            "Wherever a tool takes a project, {\"project_id\":\"...\",\"revision\":N} loads that saved revision. File identities may be {\"path\":...} alone. ",
            "Times may be 2.5, \"5/2\" or {num, den} seconds on frame boundaries. Tool listings abbreviate the project, operation, scene, template, audio_routing and transcript schemas; ",
            "cutbolt_schema returns them, outlined when large, and capabilities summarizes limits."
        )
        .to_owned();
        if self.core {
            text.push_str(" This compact catalog lists the everyday tools; cutbolt_run runs any other command named here, with the same arguments.");
        }
        if let Some(workspace) = &self.workspace {
            text.push_str(&format!(" Workspace: {}. Paths may be relative to it, omitted roots default inside it (sessions in .cutbolt/store, jobs in .cutbolt/jobs, cache in .cutbolt/cache), and explicit roots must stay inside it. Any argument may be given as {{\"file\": \"name.json\", \"select\": \"field\"}} to read it from a workspace file (select may be a dotted path such as result.operations), and save_as writes a large result to a new .json file instead of returning it. Proposals (audio.duck, audio.normalize, audio.tighten, transcript.fillers, transcript.assemble) take save_as; then pass session.apply operations as {{\"file\": that file, \"select\": \"operations\"}} instead of copying them.", workspace.root().display()));
        }
        text
    }
    /// Handle one message, running a tool call inline.
    #[cfg(test)]
    fn message(&mut self, message: Value) -> Option<Value> {
        match self.step(message) {
            Step::Reply(reply) => reply,
            Step::Call(call) => Some(call.run()),
        }
    }
    fn step(&mut self, message: Value) -> Step {
        let reply = self.route(message);
        match self.pending.take() {
            Some(call) => Step::Call(call),
            None => Step::Reply(reply),
        }
    }
    /// Answer a message directly, or leave a validated tool call in `pending`.
    fn route(&mut self, message: Value) -> Option<Value> {
        if !message.is_object() || message["jsonrpc"] != "2.0" || !message["method"].is_string() {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "Expected one JSON-RPC 2.0 request object",
            ));
        }
        let method = message["method"].as_str().unwrap();
        let Some(id) = message.get("id").cloned() else {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            // Notifications never invoke tools or receive responses.
            return None;
        };
        if !(id.is_string() || id.is_i64()) {
            return Some(rpc_error(
                Value::Null,
                -32600,
                "Request ID must be a string or integer",
            ));
        }
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(rpc_error(id, -32602, "Parameters must be an object"));
        }
        if method == "ping" {
            return Some(response(id, json!({})));
        }
        if method == "initialize" {
            if self.initialized {
                return Some(rpc_error(id, -32600, "Already initialized"));
            }
            if !params["protocolVersion"].is_string()
                || !params["capabilities"].is_object()
                || !params["clientInfo"]["name"].is_string()
                || !params["clientInfo"]["version"].is_string()
            {
                return Some(rpc_error(
                    id,
                    -32602,
                    "initialize requires protocolVersion, capabilities and clientInfo name/version",
                ));
            }
            self.initialized = true;
            return Some(response(
                id,
                json!({"protocolVersion":if params["protocolVersion"]=="2025-06-18" {"2025-06-18"} else {VERSION},"capabilities":{"tools":{"listChanged":false}},
                "serverInfo":{"name":"cutbolt","version":env!("CARGO_PKG_VERSION")},
                "instructions":self.instructions()}),
            ));
        }
        if !self.ready {
            return Some(rpc_error(
                id,
                -32002,
                "Initialize and send notifications/initialized before calling tools",
            ));
        }
        match method {
            "tools/list" => {
                if params.get("cursor").is_some() {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "The complete catalog fits one page; no cursor is accepted",
                    ));
                }
                Some(response(id, json!({"tools":self.catalog()})))
            }
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(rpc_error(id, -32602, "Tool name is required"));
                };
                if !self.catalog().iter().any(|t| t["name"] == name) {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "Unknown tool; discover names with tools/list",
                    ));
                }
                // cutbolt_run names the command and nests its arguments.
                let (command, arguments) = if name == "cutbolt_run" {
                    let command = params["arguments"]["command"].as_str().unwrap_or_default();
                    if !crate::schema::commands().any(|c| c == command) || !exposed(command) {
                        return Some(rpc_error(
                            id,
                            -32602,
                            &format!(
                                "cutbolt_run cannot run {command:?}; its description lists the commands, and long-running ones go through job.start"
                            ),
                        ));
                    }
                    (
                        command.to_owned(),
                        params["arguments"]
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )
                } else {
                    (
                        name.strip_prefix("cutbolt_").unwrap().replace('_', "."),
                        params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({})),
                    )
                };
                self.pending = Some(Call {
                    id,
                    command,
                    arguments,
                    workspace: self.workspace.clone(),
                    progress_token: Some(&params["_meta"]["progressToken"])
                        .filter(|t| t.is_string() || t.is_i64())
                        .cloned(),
                    notify: None,
                });
                None
            }
            _ => Some(rpc_error(id, -32601, "Method not found")),
        }
    }
}

// Bound memory while draining an oversized line, then recover at the next newline.
fn line(reader: &mut impl BufRead) -> io::Result<Option<(Vec<u8>, bool)>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    let mut seen = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(seen.then_some((bytes, oversized)));
        }
        seen = true;
        let end = available.iter().position(|b| *b == b'\n');
        let count = end.map_or(available.len(), |p| p + 1);
        let keep = count.min(MAX_LINE.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&available[..keep]);
        oversized |= keep < count;
        reader.consume(count);
        if end.is_some() {
            return Ok(Some((bytes, oversized)));
        }
    }
}

/// Tool calls that may run at once; a further call waits for a free slot before it starts.
const MAX_CALLS: usize = 8;

/// Write one whole message line; workers share stdout, so lines never interleave.
fn send(output: &Mutex<io::Stdout>, message: &Value) -> io::Result<()> {
    let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
    writeln!(output, "{message}")?;
    output.flush()
}

pub fn serve(workspace: Option<Workspace>, core: bool) -> Result<()> {
    let mut server = Server {
        workspace,
        core,
        ..Server::default()
    };
    let output = Arc::new(Mutex::new(io::stdout()));
    let slots = Arc::new((Mutex::new(0usize), Condvar::new()));
    let mut workers = Vec::new();
    let mut input = io::stdin().lock();
    while let Some((bytes, oversized)) = line(&mut input)? {
        let step = if oversized {
            Step::Reply(Some(rpc_error(
                Value::Null,
                -32600,
                "MCP messages are limited to 4 MiB",
            )))
        } else {
            match serde_json::from_slice(&bytes) {
                Ok(value) => server.step(value),
                Err(_) => Step::Reply(Some(rpc_error(Value::Null, -32700, "Invalid JSON"))),
            }
        };
        match step {
            Step::Reply(Some(response)) => send(&output, &response)?,
            Step::Reply(None) => {}
            Step::Call(mut call) => {
                if call.progress_token.is_some() {
                    let output = Arc::clone(&output);
                    call.notify = Some(Box::new(move |message| {
                        let _ = send(&output, &message);
                    }));
                }
                let (running, freed) = &*slots;
                let mut count = running.lock().unwrap_or_else(|e| e.into_inner());
                while *count >= MAX_CALLS {
                    count = freed.wait(count).unwrap_or_else(|e| e.into_inner());
                }
                *count += 1;
                drop(count);
                let (output, slots) = (Arc::clone(&output), Arc::clone(&slots));
                let worker = std::thread::Builder::new()
                    .stack_size(8 << 20)
                    .spawn(move || {
                        let id = call.id.clone();
                        let response =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| call.run()))
                                .unwrap_or_else(|_| rpc_error(id, -32603, "Internal error"));
                        let _ = send(&output, &response);
                        let (running, freed) = &*slots;
                        *running.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
                        freed.notify_one();
                    })?;
                workers.push(worker);
            }
        }
        workers.retain(|w| !w.is_finished());
    }
    // Answer every accepted call before exiting.
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Agents load the whole catalog into context, so listings stay within explicit budgets.
    const TOOL_BYTES: usize = 16 * 1024;
    const CATALOG_BYTES: usize = 256 * 1024;

    /// Listed properties, including those of generated stubs, all describe themselves.
    fn undescribed(schema: &Value) -> usize {
        match schema {
            Value::Object(object) => {
                let own = object
                    .get("properties")
                    .and_then(Value::as_object)
                    .map_or(0, |p| {
                        p.values()
                            .filter(|p| p.get("const").is_none() && p.get("description").is_none())
                            .count()
                    });
                own + object.values().map(undescribed).sum::<usize>()
            }
            Value::Array(items) => items.iter().map(undescribed).sum(),
            _ => 0,
        }
    }

    /// The compact catalog names real tool commands only, and stays small.
    #[test]
    fn compact_catalog_lists_core_tools_and_run() {
        let commands: Vec<&str> = crate::schema::commands().collect();
        for command in CORE {
            assert!(commands.contains(&command) && exposed(command), "{command}");
        }
        let compact = core_tools(None);
        assert_eq!(compact.len(), CORE.len() + 1);
        let bytes: usize = compact.iter().map(|t| t.to_string().len()).sum();
        assert!(bytes <= 96 * 1024, "compact catalog uses {bytes} bytes");
    }

    /// Tool names replace dots with underscores, and calls map them back, so a command name with
    /// an underscore would be unreachable over MCP.
    #[test]
    fn command_names_survive_the_tool_name_round_trip() {
        for command in crate::schema::commands() {
            assert!(!command.contains('_'), "{command} contains an underscore");
        }
    }

    #[test]
    fn tool_listings_fit_agent_context() {
        let total = check_catalog(&tools(None));
        eprintln!("catalog: {} tools, {total} bytes", tools(None).len());
        let temp = std::env::temp_dir().join(format!("cutbolt-mcp-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let workspace = Workspace::open(&temp).unwrap();
        eprintln!(
            "workspace catalog: {} bytes",
            check_catalog(&tools(Some(&workspace)))
        );
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn workspace_servers_need_no_roots() {
        let temp =
            std::env::temp_dir().join(format!("cutbolt-mcp-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let mut server = Server {
            workspace: Some(Workspace::open(&temp).unwrap()),
            ..Server::default()
        };
        let mut call = |id: i64, method: &str, params: Value| {
            server
                .message(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .unwrap()
        };
        let init = call(
            1,
            "initialize",
            json!({"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"t","version":"0"}}),
        );
        assert!(
            init["result"]["instructions"]
                .as_str()
                .unwrap()
                .contains("Workspace: ")
        );
        server.message(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        let mut call = |id: i64, name: &str, arguments: Value| {
            server
                .message(json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
                .unwrap()["result"]["structuredContent"]
                .clone()
        };
        let project = json!({"schema_version":1,"id":"w","revision":0,"width":32,"height":24,"frame_rate":{"num":25,"den":1},"assets":[],"clips":[]});
        let created = call(
            2,
            "cutbolt_session_create",
            json!({"request_id":"c","project":project}),
        );
        assert_eq!(created["result"]["revision"], 0, "{created}");
        let read = call(
            3,
            "cutbolt_session_get",
            json!({"project_id":"w","store_root":".cutbolt/store"}),
        );
        assert_eq!(read["result"]["id"], "w", "{read}");
        let checked = call(
            4,
            "cutbolt_project_validate",
            json!({"project":{"project_id":"w"}}),
        );
        assert_eq!(checked["result"]["valid"], true, "{checked}");
        let listed = server
            .message(json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}))
            .unwrap();
        let start = listed["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == "cutbolt_render_start")
            .unwrap();
        assert_eq!(
            start["inputSchema"]["required"],
            json!(["request_id", "render"])
        );
        // Workspace listings never claim that paths must be absolute.
        let text = listed["result"]["tools"].to_string().to_lowercase();
        for phrase in [
            "absolute path",
            "absolute dir",
            "absolute local",
            "absolute input",
            "absolute output",
            "absolute `",
            "absolute candidate",
            "absolute media",
            "unused absolute",
            "new absolute",
        ] {
            assert!(
                !text.contains(phrase),
                "workspace listing still says {phrase:?}"
            );
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    fn check_catalog(catalog: &[Value]) -> usize {
        let mut total = 0;
        for tool in catalog {
            let bytes = tool.to_string().len();
            assert!(bytes <= TOOL_BYTES, "{} uses {bytes} bytes", tool["name"]);
            total += bytes;
            let schema = &tool["inputSchema"];
            for target in crate::schema::references(schema) {
                assert!(
                    schema["$defs"][&target].is_object(),
                    "{} lacks {target}",
                    tool["name"]
                );
                assert!(
                    !crate::schema::DEFERRED.iter().any(|(_, d)| *d == target),
                    "{} embeds deferred {target}",
                    tool["name"]
                );
            }
            assert_ne!(tool["description"], UNDESCRIBED);
            assert_eq!(
                undescribed(schema),
                0,
                "{} has undescribed fields",
                tool["name"]
            );
        }
        assert!(total <= CATALOG_BYTES, "catalog uses {total} bytes");
        total
    }
}
