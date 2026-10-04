use crate::{
    Result, audio, conform, jobs, media,
    model::{Operation, Project},
    preview, proxy, registry, render, scene, store,
    time::Time,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Deserialize, schemars::JsonSchema)]
#[serde(tag = "command", deny_unknown_fields)]
pub enum Request {
    #[serde(rename = "native.import")]
    NativeImport(crate::native_project::Import),
    #[serde(rename = "project.portable")]
    ProjectPortable {
        project: Project,
        expected_revision: u64,
        input_root: PathBuf,
    },
    #[serde(rename = "session.check")]
    SessionCheck { store_root: PathBuf },
    #[serde(rename = "session.migrate")]
    SessionMigrate { store_root: PathBuf },
    #[serde(rename = "session.backup")]
    SessionBackup(store::Backup),
    #[serde(rename = "session.recover")]
    SessionRecover(store::Recover),
    /// Inspect a content-bound OTIO file and return a new project only after acknowledging its nonblocking losses.
    #[serde(rename = "interchange.import")]
    InterchangeImport(crate::interchange::Import),
    /// Inspect the OTIO export document and exact unsupported-feature report without writing a file.
    #[serde(rename = "interchange.export.inspect")]
    InterchangeInspect {
        project: Project,
        input_root: PathBuf,
    },
    /// Publish an OTIO timeline to an unused local output after explicit loss acknowledgement.
    #[serde(rename = "interchange.export")]
    InterchangeExport(crate::interchange::Export),
    #[serde(rename = "cache.run")]
    CacheRun(crate::cache::Request),
    #[serde(rename = "cache.inspect")]
    CacheInspect { cache_root: PathBuf },
    #[serde(rename = "cache.prune")]
    CachePrune {
        cache_root: PathBuf,
        policy: crate::cache::Policy,
    },
    #[serde(rename = "preview.sheet")]
    PreviewSheet {
        project: Project,
        spec: preview::Sheet,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "transcript.transcribe")]
    Transcribe(crate::transcribe::Transcribe),
    #[serde(rename = "transcript.inspect")]
    TranscriptInspect {
        document: crate::transcript::Document,
        input_root: PathBuf,
    },
    #[serde(rename = "transcript.correct")]
    TranscriptCorrect {
        document: crate::transcript::Document,
        expected_fingerprint: String,
        edits: Vec<crate::transcript::Edit>,
        input_root: PathBuf,
    },
    #[serde(rename = "transcript.plan")]
    TranscriptPlan {
        project: Project,
        document: crate::transcript::Document,
        expected_revision: u64,
        expected_document_fingerprint: String,
        spec: crate::transcript_cut::Spec,
        input_root: PathBuf,
    },
    #[serde(rename = "audio.inputs")]
    AudioInputs {},
    #[serde(rename = "audio.record.inspect")]
    AudioRecordInspect {
        input: crate::recording::Input,
        duration: Time,
    },
    #[serde(rename = "audio.record")]
    AudioRecord(crate::recording::Record),
    #[serde(rename = "audio.record.place")]
    AudioRecordPlace(crate::recording::Place),
    #[serde(rename = "audio.repair.inspect")]
    AudioRepairInspect {
        recipe: crate::audio_repair::Recipe,
        input_root: PathBuf,
    },
    #[serde(rename = "audio.repair.render")]
    AudioRepairRender {
        recipe: crate::audio_repair::Recipe,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "stabilization.inspect")]
    StabilizationInspect(crate::stabilize::Inspect),
    #[serde(rename = "reframe.inspect")]
    ReframeInspect(crate::reframe::Inspect),
    #[serde(rename = "tracking.inspect")]
    TrackingInspect(crate::tracking::Inspect),
    #[serde(rename = "sync.inspect")]
    SyncInspect(crate::sync::Inspect),
    #[serde(rename = "hdr.inspect")]
    HdrInspect {
        recipe: crate::hdr::Recipe,
        input_root: PathBuf,
    },
    #[serde(rename = "hdr.conform")]
    HdrConform {
        recipe: crate::hdr::Recipe,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "lut.inspect")]
    LutInspect {
        transform: crate::lut::Transform,
        input_root: PathBuf,
        #[serde(default)]
        samples: Vec<[f64; 3]>,
    },
    #[serde(rename = "scopes.inspect")]
    ScopesInspect(crate::scopes::Inspect),
    #[serde(rename = "export.inspect")]
    ExportInspect(crate::delivery::Export),
    #[serde(rename = "export.run")]
    ExportRun(crate::delivery::Export),
    #[serde(rename = "effects.preset")]
    EffectsPreset {
        name: crate::keying::Preset,
        strength_milli: u16,
        spill_milli: u16,
    },
    #[serde(rename = "captions.import")]
    CaptionsImport {
        source: scene::Identity,
        input_root: PathBuf,
        format: crate::captions::Format,
        id: String,
        overlap: crate::captions::Overlap,
    },
    #[serde(rename = "captions.inspect")]
    CaptionsInspect { document: crate::captions::Document },
    #[serde(rename = "captions.apply")]
    CaptionsApply {
        document: crate::captions::Document,
        expected_revision: u64,
        operations: Vec<crate::captions::Operation>,
    },
    #[serde(rename = "captions.encode")]
    CaptionsEncode {
        document: crate::captions::Document,
        format: crate::captions::Format,
    },
    #[serde(rename = "captions.export")]
    CaptionsExport {
        document: crate::captions::Document,
        format: crate::captions::Format,
        loss_policy: crate::captions::LossPolicy,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "captions.scene")]
    CaptionsScene(crate::captions::SceneRequest),
    #[serde(rename = "graphics.instantiate")]
    GraphicsInstantiate {
        template: crate::templates::Template,
        instance_id: String,
        values: std::collections::BTreeMap<String, crate::templates::ParameterValue>,
        input_root: PathBuf,
    },
    #[serde(rename = "proxy.generate")]
    ProxyGenerate(proxy::Generate),
    #[serde(rename = "proxy.status")]
    ProxyStatus {
        project: Project,
        input_root: PathBuf,
    },
    #[serde(rename = "proxy.relink")]
    ProxyRelink {
        project: Project,
        expected_revision: u64,
        input_root: PathBuf,
        asset_id: String,
        candidates: Vec<PathBuf>,
    },
    #[serde(rename = "media.conform.inspect")]
    ConformInspect {
        recipe: conform::Recipe,
        input_root: PathBuf,
    },
    #[serde(rename = "media.conform")]
    Conform {
        recipe: conform::Recipe,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "audio.inspect")]
    AudioInspect {
        mix: audio::Mix,
        input_root: PathBuf,
    },
    #[serde(rename = "audio.render")]
    AudioRender {
        mix: audio::Mix,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "registry.search")]
    RegistrySearch {
        project: Project,
        query: registry::Query,
    },
    #[serde(rename = "registry.status")]
    RegistryStatus {
        project: Project,
        input_root: PathBuf,
    },
    #[serde(rename = "registry.bind")]
    RegistryBind {
        project: Project,
        expected_revision: u64,
        input_root: PathBuf,
        asset_ids: Vec<String>,
    },
    #[serde(rename = "registry.relink")]
    RegistryRelink {
        project: Project,
        expected_revision: u64,
        input_root: PathBuf,
        asset_id: String,
        candidates: Vec<PathBuf>,
    },
    #[serde(rename = "image.sequence.inspect")]
    ImageSequenceInspect {
        recipe: crate::image_sequence::Recipe,
        input_root: PathBuf,
    },
    #[serde(rename = "image.sequence.compile")]
    ImageSequenceCompile {
        recipe: crate::image_sequence::Recipe,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "expression.inspect")]
    ExpressionInspect {
        #[serde(flatten)]
        request: crate::expressions::Inspect,
    },
    #[serde(rename = "scene.inspect")]
    SceneInspect {
        scene: scene::Scene,
        input_root: PathBuf,
    },
    #[serde(rename = "scene.render")]
    SceneRender {
        scene: scene::Scene,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "preview.frame")]
    PreviewFrame {
        project: Project,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
        time: Time,
    },
    #[serde(rename = "preview.range")]
    PreviewRange {
        project: Project,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
        start: Time,
        duration: Time,
    },
    #[serde(rename = "capabilities")]
    Capabilities {},
    #[serde(rename = "project.create")]
    Create {
        id: String,
        width: u32,
        height: u32,
        frame_rate: Time,
    },
    #[serde(rename = "project.validate")]
    Validate { project: Project },
    #[serde(rename = "timeline.apply")]
    Apply {
        project: Project,
        expected_revision: u64,
        operations: Vec<Operation>,
    },
    #[serde(rename = "session.create")]
    SessionCreate {
        store_root: PathBuf,
        project: Project,
        request_id: String,
    },
    #[serde(rename = "session.get")]
    SessionGet {
        store_root: PathBuf,
        project_id: String,
        revision: Option<u64>,
    },
    #[serde(rename = "session.apply")]
    SessionApply {
        store_root: PathBuf,
        project_id: String,
        request_id: String,
        expected_revision: u64,
        operations: Vec<Operation>,
    },
    #[serde(rename = "session.undo")]
    SessionUndo {
        store_root: PathBuf,
        project_id: String,
        request_id: String,
        expected_revision: u64,
    },
    #[serde(rename = "session.restore")]
    SessionRestore {
        store_root: PathBuf,
        project_id: String,
        request_id: String,
        expected_revision: u64,
        target_revision: u64,
    },
    #[serde(rename = "session.preview")]
    SessionPreview {
        store_root: PathBuf,
        project_id: String,
        expected_revision: u64,
        operations: Vec<Operation>,
    },
    #[serde(rename = "session.history")]
    SessionHistory {
        store_root: PathBuf,
        project_id: String,
        before_revision: Option<u64>,
        #[serde(default = "history_limit")]
        limit: u16,
    },
    /// Queue a render and return a durable ticket immediately. Reuse request_id only with identical arguments. Poll job.status; use job.cancel to stop it.
    #[serde(rename = "render.start")]
    Start {
        job_root: PathBuf,
        request_id: String,
        render: jobs::RenderRequest,
    },
    /// Return saved job state, rendering frame progress, result receipt or failure. Reconcile workers that have exited.
    #[serde(rename = "job.status")]
    JobStatus { job_root: PathBuf, job_id: String },
    /// Cancel a queued/running job. Poll status until terminal; completion may win a simultaneous cancel.
    #[serde(rename = "job.cancel")]
    JobCancel { job_root: PathBuf, job_id: String },
    /// Reconcile saved publication and wake queued work, including explicitly opted-in interrupted retries within the saved attempt limit.
    #[serde(rename = "job.resume")]
    JobResume { job_root: PathBuf },
    #[serde(rename = "media.inspect")]
    Inspect { path: PathBuf, input_root: PathBuf },
    #[serde(rename = "render.plan")]
    Plan {
        project: Project,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
    #[serde(rename = "render.run")]
    Render {
        project: Project,
        input_root: PathBuf,
        output_root: PathBuf,
        output: PathBuf,
    },
}

fn history_limit() -> u16 {
    50
}

pub fn handle(request: Request) -> Result<Value> {
    match request {
        Request::NativeImport(request) => crate::native_project::import(&request),
        Request::ProjectPortable {
            project,
            expected_revision,
            input_root,
        } => crate::portable::inspect(&project, expected_revision, &input_root),
        Request::SessionCheck { store_root } => store::check(&store_root),
        Request::SessionMigrate { store_root } => store::migrate(&store_root),
        Request::SessionBackup(request) => store::backup(&request),
        Request::SessionRecover(request) => store::recover(&request),
        Request::InterchangeImport(request) => crate::interchange::import(&request),
        Request::InterchangeInspect {
            project,
            input_root,
        } => crate::interchange::inspect_export(&project, &input_root),
        Request::InterchangeExport(request) => crate::interchange::export(&request),
        Request::CacheRun(request) => crate::cache::run(&request),
        Request::CacheInspect { cache_root } => crate::cache::inspect(&cache_root),
        Request::CachePrune { cache_root, policy } => crate::cache::prune(&cache_root, &policy),
        Request::PreviewSheet {
            project,
            spec,
            input_root,
            output_root,
            output,
        } => preview::sheet(&project, &spec, &input_root, &output_root, &output),
        Request::AudioInputs {} => crate::recording::inputs(),
        Request::AudioRecordInspect { input, duration } => {
            crate::recording::inspect(&input, duration)
        }
        Request::Transcribe(request) => crate::transcribe::run(&request),
        Request::TranscriptInspect {
            document,
            input_root,
        } => crate::transcript::inspect(&document, &input_root),
        Request::TranscriptCorrect {
            document,
            expected_fingerprint,
            edits,
            input_root,
        } => crate::transcript::correct(&document, &expected_fingerprint, &edits, &input_root),
        Request::TranscriptPlan {
            project,
            document,
            expected_revision,
            expected_document_fingerprint,
            spec,
            input_root,
        } => crate::transcript_cut::plan(
            &project,
            &document,
            expected_revision,
            &expected_document_fingerprint,
            &spec,
            &input_root,
        ),
        Request::AudioRecord(request) => crate::recording::run(&request),
        Request::AudioRecordPlace(request) => crate::recording::place(&request),
        Request::StabilizationInspect(request) => crate::stabilize::inspect(&request),
        Request::ReframeInspect(request) => crate::reframe::inspect(&request),
        Request::TrackingInspect(request) => crate::tracking::inspect(&request),
        Request::SyncInspect(request) => crate::sync::inspect(&request),
        Request::HdrInspect { recipe, input_root } => crate::hdr::inspect(&recipe, &input_root),
        Request::HdrConform {
            recipe,
            input_root,
            output_root,
            output,
        } => crate::hdr::run(&recipe, &input_root, &output_root, &output),
        Request::LutInspect {
            transform,
            input_root,
            samples,
        } => crate::lut::inspect(&transform, &input_root, &samples),
        Request::ScopesInspect(request) => crate::scopes::inspect(&request),
        Request::ExportInspect(request) => crate::delivery::inspect(&request),
        Request::ImageSequenceInspect { recipe, input_root } => {
            crate::image_sequence::inspect(&recipe, &input_root)
        }
        Request::ImageSequenceCompile {
            recipe,
            input_root,
            output_root,
            output,
        } => crate::image_sequence::run(&recipe, &input_root, &output_root, &output),
        Request::ExportRun(request) => crate::delivery::run(&request),
        Request::EffectsPreset {
            name,
            strength_milli,
            spill_milli,
        } => crate::keying::preset(name, strength_milli, spill_milli),
        Request::CaptionsImport {
            source,
            input_root,
            format,
            id,
            overlap,
        } => crate::captions::import(&source, &input_root, format, &id, overlap),
        Request::CaptionsInspect { document } => crate::captions::inspect(&document),
        Request::CaptionsApply {
            document,
            expected_revision,
            operations,
        } => crate::captions::apply(&document, expected_revision, &operations),
        Request::CaptionsEncode { document, format } => crate::captions::encode(&document, format),
        Request::CaptionsExport {
            document,
            format,
            loss_policy,
            output_root,
            output,
        } => crate::captions::export(&document, format, loss_policy, &output_root, &output),
        Request::CaptionsScene(request) => crate::captions::to_scene(&request),
        Request::ProxyGenerate(request) => proxy::generate(&request),
        Request::ProxyStatus {
            project,
            input_root,
        } => proxy::status(&project, &input_root),
        Request::ProxyRelink {
            project,
            expected_revision,
            input_root,
            asset_id,
            candidates,
        } => proxy::relink(
            &project,
            expected_revision,
            &input_root,
            &asset_id,
            &candidates,
        ),
        Request::ConformInspect { recipe, input_root } => conform::inspect(&recipe, &input_root),
        Request::Conform {
            recipe,
            input_root,
            output_root,
            output,
        } => conform::run(&recipe, &input_root, &output_root, &output),
        Request::AudioRepairInspect { recipe, input_root } => {
            crate::audio_repair::inspect(&recipe, &input_root)
        }
        Request::AudioRepairRender {
            recipe,
            input_root,
            output_root,
            output,
        } => crate::audio_repair::run(&recipe, &input_root, &output_root, &output),
        Request::AudioInspect { mix, input_root } => audio::inspect(&mix, &input_root),
        Request::AudioRender {
            mix,
            input_root,
            output_root,
            output,
        } => audio::run(&mix, &input_root, &output_root, &output),
        Request::RegistrySearch { project, query } => registry::search(&project, &query),
        Request::RegistryStatus {
            project,
            input_root,
        } => registry::status(&project, &input_root),
        Request::RegistryBind {
            project,
            expected_revision,
            input_root,
            asset_ids,
        } => registry::bind(&project, expected_revision, &input_root, &asset_ids),
        Request::RegistryRelink {
            project,
            expected_revision,
            input_root,
            asset_id,
            candidates,
        } => registry::relink(
            &project,
            expected_revision,
            &input_root,
            &asset_id,
            &candidates,
        ),
        Request::GraphicsInstantiate {
            template,
            instance_id,
            values,
            input_root,
        } => crate::templates::instantiate(&template, &instance_id, &values, &input_root),
        Request::ExpressionInspect { request } => crate::expressions::inspect(&request),
        Request::SceneInspect { scene, input_root } => scene::inspect(&scene, &input_root),
        Request::SceneRender {
            scene,
            input_root,
            output_root,
            output,
        } => scene::run(&scene, &input_root, &output_root, &output),
        Request::PreviewFrame {
            project,
            input_root,
            output_root,
            output,
            time,
        } => preview::frame(&project, &input_root, &output_root, &output, time),
        Request::PreviewRange {
            project,
            input_root,
            output_root,
            output,
            start,
            duration,
        } => preview::range(
            &project,
            &input_root,
            &output_root,
            &output,
            start,
            duration,
        ),
        Request::Capabilities {} => {
            let mut result = json!({"version":env!("CARGO_PKG_VERSION"),"license":"MIT","local_only":true,"reframing":crate::reframe::capabilities(),
            "interchange":crate::interchange::capabilities(),
            "project_store":{"schema_version":2,"read_versions":[1,2],"migration":"explicit_transactional","backup_maximum_bytes":268435456,"relative_media":true},
            "commands":["expression.inspect","native.import","image.sequence.inspect","image.sequence.compile","project.portable","session.check","session.migrate","session.backup","session.recover","interchange.import","interchange.export.inspect","interchange.export","cache.run","cache.inspect","cache.prune","preview.sheet","transcript.transcribe","transcript.inspect","transcript.correct","transcript.plan","audio.inputs","audio.record.inspect","audio.record","audio.record.place","audio.repair.inspect","audio.repair.render","stabilization.inspect","reframe.inspect","tracking.inspect","sync.inspect","hdr.inspect","hdr.conform","lut.inspect","scopes.inspect","export.inspect","export.run","effects.preset","captions.import","captions.inspect","captions.apply","captions.encode","captions.export","captions.scene","graphics.instantiate","proxy.generate","proxy.status","proxy.relink","media.conform.inspect","media.conform","audio.inspect","audio.render","registry.search","registry.status","registry.bind","registry.relink","scene.inspect","scene.render","preview.frame","preview.range","capabilities","project.create","project.validate","timeline.apply","session.create","session.get","session.apply","session.undo","session.restore","session.preview","session.history","media.inspect","render.plan","render.run","render.start","job.status","job.cancel","job.resume"],
            "operations":["media.paths","transcript.cut","multicam.create","multicam.edit","sequence.create","sequence.edit","sequence.remove","tracks.edit","media.proxy.attach","media.proxy.detach","media.proxy.relink","preview.proxy","clip.insert","clip.overwrite","timeline.ripple_delete","clip.slip","clip.roll","clip.slide","media.metadata","media.bind","media.relink","media.add","clip.append","clip.split","clip.trim","clip.move","clip.remove"],
            "state":"immutable snapshots plus local transactional sessions with durable request IDs, revision conflicts and undo/history",
            "mcp":{"transport":"stdio","protocol_versions":["2025-11-25","2025-06-18"]},"jobs":{"platform":"windows","available":cfg!(windows),"maximum_active_per_root":32,"concurrent_renders_per_root":1,"default_attempts":1,"maximum_attempts":3,"retry_errors":["TOOL_FAILED","TOOL_TIMEOUT","WORKER_INTERRUPTED"],"source_pinning":"first_validated_plan","tool_content_pinning":true,"publication_recovery":"validated_receipt_and_output_hash","queue_schema_version":2},
            "transcripts":crate::transcript::capabilities(),
            "audio_repair":crate::audio_repair::capabilities(),
            "audio_recording":{"platform":"windows","sample_rate":48000,"channels":2,"sample_format":"s16le","maximum_seconds":7200,"input_selection":"explicit_endpoint_or_process","recording":"blocking_cli_library","placement":"existing_native_audio_tracks","latency_compensation":"explicit_exact_shift","source_preservation":true},
            "stabilization":crate::stabilize::capabilities(),
            "native_projects":crate::native_project::capabilities(),"image_sequences":crate::image_sequence::capabilities(),"scenes":{"spatial":crate::spatial::capabilities(),"profile":"pixel-scene-v1","maximum_seconds":10,"color":"srgb_straight_encoded","render_mode":"blocking CLI/library","alpha_modes":["straight","premultiplied"],"blend_modes":["normal","multiply","screen"],"mask":{"shape":"rectangle","space":"source_canvas_before_transform","inversion":true,"animated_properties":["x","y","width","height"],"maximum_per_layer":1,"feather":{"radius":[1,4096],"edges":["inner","centered","outer"],"coverage_grid":65536},"tracking":crate::tracking::capabilities()},"animation":{"properties":["position_x","position_y","opacity"],"interpolation":["hold","linear","ease_in","ease_out","ease_in_out"],"easing_profile":"quadratic","clock":"layer_local","retime":{"scope":"per_property_or_mask_curve","start":"layer_local_playback_anchor","rate_minimum":{"num":1,"den":16},"rate_maximum":{"num":16,"den":1},"reverse":true,"changes_media_time":false},"rounding":"nearest_ties_away_from_zero","maximum_keys_per_curve":128,"maximum_reduced_time_denominator":1000000,"precision":"checked_exact_integer_overflow_rejected"}},"renderer":{"profile":"reference-ffv1-pcm-v1","frame_rate":25,"sequential_frame_rates":[{"num":24,"den":1},{"num":25,"den":1},{"num":30,"den":1},{"num":50,"den":1},{"num":60,"den":1},{"num":24000,"den":1001},{"num":30000,"den":1001},{"num":60000,"den":1001}],"sequential_cut_policy":"whole_video_frames_and_48000_hz_samples","placed_track_frame_rate":25,"maximum_sequential_frames":180000,"large_raster":{"minimum_pixels":8000000,"minimum_dimension":4,"sequential_encoder_threads":16,"sequential_slices":16,"sequential_input_decoder_threads":4,"video_inspection_threads":16,"video_inspection_timeout_seconds":900,"sequential_encode_timeout_seconds":1800},"audio_rate":48000,"channels":2,"maximum_clips":64,"gaps":{"explicit":true,"video":"black","audio":"silence","maximum_frames":180000}},
            "graphics":crate::graphics::capabilities(),
            "tracks":crate::tracks::capabilities(),"sequences":crate::sequences::capabilities(),"multicam":crate::multicam::capabilities(),"synchronization":crate::sync::capabilities(),
            "templates":crate::templates::capabilities(),
            "captions":crate::captions::capabilities(),
            "effects":crate::effects::capabilities(),
            "export":crate::delivery::capabilities(),
            "hdr":crate::hdr::capabilities(),"luts":crate::lut::capabilities(),"scopes":crate::scopes::capabilities(),"proxies":proxy::capabilities(),"audio":audio::capabilities(),"conform":conform::capabilities(),
            "not_implemented":["timeline_and_delivery_surround","general_video_effects","delivery_device_matrix"]});
            result["expressions"] = crate::expressions::capabilities();
            result["temporal"] = crate::temporal::capabilities();
            result["geometry"] = crate::geometry::capabilities();
            result["frame_matte"] = json!({"profile":"binary-source-matte-v1","field":"scene.layers[].frames[].matte","identity":"relative path, SHA-256 and bytes","pixels":"opaque black/white RGB or RGBA PNG matching source dimensions","processing":"held with source, before effects and transforms","limits":"included in existing source byte and decoded/derived pixel budgets","optional_producer":"tools/segmentation.py; annotated frames; external pinned CPU runtime; no automatic tracking"});
            Ok(result)
        }
        Request::Create {
            id,
            width,
            height,
            frame_rate,
        } => Ok(serde_json::to_value(Project::new(
            id, width, height, frame_rate,
        )?)?),
        Request::Validate { project } => {
            project.validate()?;
            Ok(json!({"valid":true,"revision":project.revision,"duration":project.duration()?}))
        }
        Request::Apply {
            project,
            expected_revision,
            operations,
        } => Ok(serde_json::to_value(
            project.apply(expected_revision, operations)?,
        )?),
        Request::SessionCreate {
            store_root,
            project,
            request_id,
        } => Ok(serde_json::to_value(store::create(
            &store_root,
            project,
            &request_id,
        )?)?),
        Request::SessionGet {
            store_root,
            project_id,
            revision,
        } => Ok(serde_json::to_value(store::get(
            &store_root,
            &project_id,
            revision,
        )?)?),
        Request::SessionApply {
            store_root,
            project_id,
            request_id,
            expected_revision,
            operations,
        } => Ok(serde_json::to_value(store::mutate(
            &store_root,
            &project_id,
            &request_id,
            expected_revision,
            store::Mutation::Apply { operations },
        )?)?),
        Request::SessionUndo {
            store_root,
            project_id,
            request_id,
            expected_revision,
        } => Ok(serde_json::to_value(store::mutate(
            &store_root,
            &project_id,
            &request_id,
            expected_revision,
            store::Mutation::Undo,
        )?)?),
        Request::SessionRestore {
            store_root,
            project_id,
            request_id,
            expected_revision,
            target_revision,
        } => Ok(serde_json::to_value(store::mutate(
            &store_root,
            &project_id,
            &request_id,
            expected_revision,
            store::Mutation::Restore { target_revision },
        )?)?),
        Request::SessionPreview {
            store_root,
            project_id,
            expected_revision,
            operations,
        } => Ok(serde_json::to_value(store::preview(
            &store_root,
            &project_id,
            expected_revision,
            operations,
        )?)?),
        Request::SessionHistory {
            store_root,
            project_id,
            before_revision,
            limit,
        } => Ok(serde_json::to_value(store::history(
            &store_root,
            &project_id,
            before_revision,
            limit,
        )?)?),
        Request::Start {
            job_root,
            request_id,
            render,
        } => jobs::start(&job_root, &request_id, render),
        Request::JobStatus { job_root, job_id } => jobs::status(&job_root, &job_id),
        Request::JobCancel { job_root, job_id } => jobs::cancel(&job_root, &job_id),
        Request::JobResume { job_root } => jobs::resume(&job_root),
        Request::Inspect { path, input_root } => {
            let path = media::allowed_file(&path, &input_root)?;
            Ok(json!({"path":path,"metadata":media::probe(&path)?}))
        }
        Request::Plan {
            project,
            input_root,
            output_root,
            output,
        } => Ok(serde_json::to_value(render::plan(
            &project,
            &input_root,
            &output_root,
            &output,
        )?)?),
        Request::Render {
            project,
            input_root,
            output_root,
            output,
        } => render::run(&project, &input_root, &output_root, &output),
    }
}
