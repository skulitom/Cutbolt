//! Local MCP tools over newline-delimited JSON-RPC, pinned to the 2025-11-25 contract.
use crate::{Result, commands, workspace::Workspace};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
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
            "Inspect an exact timeline range and full-quality sources for reference, lossless PNG movie/image-directory or H.264/AAC export. Lossless sequential output supports native clocks; H.264 and placed tracks retain 25 fps. PNG/H.264 video requires explicit source transfer; sequence_first selects image numbering, h264 selects delivery controls and aac_bitrate selects audio rate. Returns timing, source identities and color/publication policy without writing. export.run is blocking CLI/library-only."
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
            "Probe a local source inside an explicit absolute input_root. Returns stream metadata; inspection does not imply this format can be rendered."
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
        "schema" => {
            "Return the JSON Schema for one command's arguments, including CLI-only commands, or for a shared type that tool listings abbreviate: project, operation, scene, template or audio_routing. Large schemas come back as an outline of variants and definitions; pass select (for example operation + clip.append, or scene + Layer) for one part with everything it references. Read-only."
        }
        "image.sequence.compile" => {
            "Compile a validated numbered PNG recipe into a transparent lossless movie or an explicitly flattened native editing asset at an unused output path. Blocking CLI/library command."
        }
        "cache.run" => {
            "Produce or reuse one content-checked cached probe, proxy, frame, interval or contact sheet under an explicit cache root and byte/entry policy. Blocking CLI/library command."
        }
        "preview.sheet" => {
            "Write a contact sheet of exact timeline frames, laid out on a grid, to an unused PNG, and return it as an inline image so the edit can be seen. Uses the saved proxy selection."
        }
        "transcript.transcribe" => {
            "Run optional local speech recognition through the explicitly configured external runtime and return a content-bound transcript document. Blocking CLI/library command."
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
const BLOCKING: [&str; 13] = [
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
        }
        let read_only=matches!(command,"schema"|"expression.inspect"|"native.import"|"image.sequence.inspect"|"project.portable"|"session.check"|"interchange.import"|"interchange.export.inspect"|"cache.inspect"|"transcript.inspect"|"transcript.correct"|"transcript.plan"|"audio.inputs"|"audio.record.inspect"|"audio.record.place"|"audio.repair.inspect"|"stabilization.inspect"|"reframe.inspect"|"tracking.inspect"|"sync.inspect"|"hdr.inspect"|"lut.inspect"|"scopes.inspect"|"export.inspect"|"effects.preset"|"captions.import"|"captions.inspect"|"captions.apply"|"captions.encode"|"captions.scene"|"graphics.instantiate"|"proxy.status"|"proxy.relink"|"media.conform.inspect"|"audio.inspect"|"registry.search"|"registry.status"|"registry.bind"|"registry.relink"|"capabilities"|"project.create"|"project.validate"|"timeline.apply"|"session.get"|"session.preview"|"session.history"|"session.receipt"|"media.inspect"|"render.plan"|"scene.inspect");
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
    catalog: Option<Vec<Value>>,
}
impl Server {
    fn catalog(&mut self) -> &[Value] {
        let workspace = self.workspace.as_ref();
        self.catalog.get_or_insert_with(|| tools(workspace))
    }
    /// Frame and contact-sheet previews also come back as a downscaled inline image, so a
    /// multimodal client sees the edit without opening the file.
    fn preview_image(&self, command: &str, result: &Value) -> Option<Value> {
        if !matches!(command, "preview.frame" | "preview.sheet") {
            return None;
        }
        let output = std::path::Path::new(result["output"].as_str()?);
        let path = match &self.workspace {
            Some(workspace) if output.is_relative() => workspace.root().join(output),
            _ => output.to_path_buf(),
        };
        let png = crate::thumbnail::png(&path, PREVIEW_EDGE).ok()?;
        Some(json!({"type":"image","data":crate::thumbnail::base64(&png),"mimeType":"image/png"}))
    }
    fn instructions(&self) -> String {
        let mut text = "Cutbolt runs locally. Use session commands for saved editing, durable request IDs for retries, and render.start/job.status/job.cancel for background work. Unsupported editing semantics fail explicitly. No HTTP service is used. Tool listings abbreviate the large shared project, operation, scene, template and audio_routing schemas; cutbolt_schema returns any of them, or any command's arguments, in full. Wherever a tool takes a project, {\"project_id\":\"...\",\"revision\":N} loads that saved revision instead of a full snapshot.".to_owned();
        if let Some(workspace) = &self.workspace {
            text.push_str(&format!(" Workspace: {}. Paths may be relative to it, omitted roots default inside it (sessions in .cutbolt/store, jobs in .cutbolt/jobs, cache in .cutbolt/cache), and explicit roots must stay inside it.", workspace.root().display()));
        }
        text
    }
    fn message(&mut self, message: Value) -> Option<Value> {
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
                let mut arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let command = name.strip_prefix("cutbolt_").unwrap().replace('_', ".");
                let outcome = (|| {
                    let object = arguments.as_object_mut().ok_or_else(|| {
                        crate::error("INVALID_JSON", "Tool arguments must be an object")
                    })?;
                    if object.contains_key("command") {
                        return Err(crate::error(
                            "INVALID_JSON",
                            "Do not send command inside tool arguments",
                        ));
                    }
                    object.insert("command".into(), Value::String(command.clone()));
                    commands::handle_json(arguments, self.workspace.as_ref())
                })();
                let value = match outcome {
                    Ok(result) => json!({"ok":true,"result":result}),
                    Err(error) => json!({"ok":false,"error":error}),
                };
                let mut content = vec![json!({"type":"text","text":value.to_string()})];
                if value["ok"] == true {
                    content.extend(self.preview_image(&command, &value["result"]));
                }
                Some(response(
                    id,
                    json!({"isError":value["ok"]!=true,"structuredContent":value,"content":content}),
                ))
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

pub fn serve(workspace: Option<Workspace>) -> Result<()> {
    let mut server = Server {
        workspace,
        ..Server::default()
    };
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    while let Some((bytes, oversized)) = line(&mut input)? {
        let response = if oversized {
            Some(rpc_error(
                Value::Null,
                -32600,
                "MCP messages are limited to 4 MiB",
            ))
        } else {
            match serde_json::from_slice(&bytes) {
                Ok(value) => server.message(value),
                Err(_) => Some(rpc_error(Value::Null, -32700, "Invalid JSON")),
            }
        };
        if let Some(response) = response {
            writeln!(output, "{response}")?;
            output.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Agents load the whole catalog into context, so listings stay within explicit budgets.
    const TOOL_BYTES: usize = 16 * 1024;
    const CATALOG_BYTES: usize = 240 * 1024;

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
