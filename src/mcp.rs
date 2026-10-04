//! Local MCP tools over newline-delimited JSON-RPC, pinned to the 2025-11-25 contract.
use crate::{
    Result,
    commands::{self, Request},
};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
const MAX_LINE: usize = 4 * 1024 * 1024;
const VERSION: &str = "2025-11-25";

fn description(command: &str) -> &'static str {
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
            "Export one exact frame from a reference project timeline to an unused PNG. Requires absolute input/output roots and rational time on the project's supported native frame boundary. No source or existing output is overwritten."
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
            "Save a project snapshot as revision 0. store_root must be an existing absolute local directory. Retry with the same request_id and arguments to retrieve the original receipt."
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
        _ => "Unsupported command",
    }
}

pub fn tools() -> Vec<Value> {
    let schema = serde_json::to_value(schemars::schema_for!(Request)).expect("request schema");
    schema["oneOf"].as_array().expect("tagged request variants").iter().filter_map(|variant| {
        let command=variant["properties"]["command"]["const"].as_str().expect("command tag");
        // Long renders belong in persisted jobs so the MCP connection stays usable.
        if matches!(command,"image.sequence.compile"|"cache.run"|"preview.sheet"|"transcript.transcribe"|"audio.record"|"audio.repair.render"|"hdr.conform"|"export.run"|"proxy.generate"|"media.conform"|"audio.render"|"render.run"|"scene.render"|"preview.range") { return None; }
        let mut input=variant.clone();
        input.as_object_mut().unwrap().remove("description");
        input["properties"].as_object_mut().unwrap().remove("command");
        input["required"].as_array_mut().unwrap().retain(|p|p!="command");
        input["$defs"]=schema["$defs"].clone();
        input["$schema"]=schema["$schema"].clone();
        let read_only=matches!(command,"expression.inspect"|"native.import"|"image.sequence.inspect"|"project.portable"|"session.check"|"interchange.import"|"interchange.export.inspect"|"cache.inspect"|"transcript.inspect"|"transcript.correct"|"transcript.plan"|"audio.inputs"|"audio.record.inspect"|"audio.record.place"|"audio.repair.inspect"|"stabilization.inspect"|"reframe.inspect"|"tracking.inspect"|"sync.inspect"|"hdr.inspect"|"lut.inspect"|"scopes.inspect"|"export.inspect"|"effects.preset"|"captions.import"|"captions.inspect"|"captions.apply"|"captions.encode"|"captions.scene"|"graphics.instantiate"|"proxy.status"|"proxy.relink"|"media.conform.inspect"|"audio.inspect"|"registry.search"|"registry.status"|"registry.bind"|"registry.relink"|"capabilities"|"project.create"|"project.validate"|"timeline.apply"|"session.get"|"session.preview"|"session.history"|"media.inspect"|"render.plan"|"scene.inspect");
        Some(json!({"name":format!("cutbolt_{}",command.replace('.',"_")),"description":description(command),"inputSchema":input,
            "outputSchema":{"type":"object","properties":{"ok":{"type":"boolean"},"result":{},"error":{"type":"object","properties":{"code":{"type":"string"},"message":{"type":"string"}},"required":["code","message"]}},"required":["ok"],"additionalProperties":false},
            "annotations":{"readOnlyHint":read_only,"destructiveHint":matches!(command,"job.cancel"|"cache.prune"),"idempotentHint":!matches!(command,"preview.frame"|"captions.export"|"interchange.export"|"session.backup"|"session.recover"),"openWorldHint":false}}))
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
}
impl Server {
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
                "instructions":"Cutbolt runs locally. Use session commands for saved editing, durable request IDs for retries, and render.start/job.status/job.cancel for background work. Unsupported editing semantics fail explicitly. No HTTP service is used."}),
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
                Some(response(id, json!({"tools":tools()})))
            }
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(rpc_error(id, -32602, "Tool name is required"));
                };
                let catalog = tools();
                if !catalog.iter().any(|t| t["name"] == name) {
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
                    let command = name.strip_prefix("cutbolt_").unwrap().replace('_', ".");
                    object.insert("command".into(), Value::String(command));
                    commands::handle(serde_json::from_value(arguments)?)
                })();
                let value = match outcome {
                    Ok(result) => json!({"ok":true,"result":result}),
                    Err(error) => json!({"ok":false,"error":error}),
                };
                Some(response(
                    id,
                    json!({"isError":value["ok"]!=true,"structuredContent":value,"content":[{"type":"text","text":value.to_string()}]}),
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

pub fn serve() -> Result<()> {
    let mut server = Server::default();
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
