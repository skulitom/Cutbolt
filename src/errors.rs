//! Stable error guidance. New engine codes must be registered and documented here.
use serde::Serialize;
use serde_json::{Value, json};
use std::path::Path;

/// Whether the failed operation may be retried with the same input, needs repair, or needs diagnosis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Retry {
    Same,
    Changed,
    Never,
}

/// Code, retry class and meaning; the documentation table is checked against this registry.
pub const REGISTRY: &[(&str, Retry, &str)] = &[
    (
        "ADJACENT_WORD_CUT",
        Retry::Changed,
        "A word cut touches another boundary without the required adjacency policy.",
    ),
    (
        "AMBIGUOUS_MEDIA",
        Retry::Changed,
        "More than one candidate matches the requested media identity.",
    ),
    (
        "ANALYSIS_CLOCK",
        Retry::Never,
        "Analysis output disagrees with the requested sample clock.",
    ),
    (
        "ANGLE_COVERAGE",
        Retry::Changed,
        "A camera angle does not cover the required interval.",
    ),
    (
        "ANGLE_IN_USE",
        Retry::Changed,
        "An angle still has saved cut decisions referencing it.",
    ),
    (
        "AUDIO_IO",
        Retry::Changed,
        "A PCM stream could not be read or written.",
    ),
    (
        "AUDIO_OVERFLOW",
        Retry::Changed,
        "The scene audio operation exceeds its bounded sample range.",
    ),
    (
        "AUDIO_PROCESSING_OVERFLOW",
        Retry::Changed,
        "Audio processing arithmetic or work exceeds its limit.",
    ),
    (
        "AUDIO_ROUTING_OVERFLOW",
        Retry::Changed,
        "Audio routing arithmetic or work exceeds its limit.",
    ),
    (
        "CACHE_CORRUPT",
        Retry::Never,
        "A derived cache entry fails integrity validation; inspect or rebuild it explicitly.",
    ),
    (
        "CACHE_MISSING",
        Retry::Changed,
        "The requested derived cache entry does not exist.",
    ),
    (
        "CAPTION_OVERLAP",
        Retry::Changed,
        "Caption intervals overlap contrary to the selected policy.",
    ),
    (
        "CAPTURE_DEVICE_CHANGED",
        Retry::Changed,
        "The selected capture device changed during recording.",
    ),
    (
        "CAPTURE_DEVICE_ERROR",
        Retry::Same,
        "The local capture backend reported a device error.",
    ),
    (
        "CAPTURE_DISCONTINUITY",
        Retry::Changed,
        "Capture packets contain a sample gap or discontinuity.",
    ),
    (
        "CAPTURE_PROCESS_EXITED",
        Retry::Changed,
        "The selected process ended during capture; select a running process.",
    ),
    (
        "CAPTURE_TIMEOUT",
        Retry::Same,
        "Capture did not produce data before the deadline.",
    ),
    (
        "CAPTURE_TIMESTAMP_ERROR",
        Retry::Never,
        "Capture timestamps violate the declared clock.",
    ),
    (
        "CAPTURE_VALIDATION_FAILED",
        Retry::Never,
        "Recorded output failed independent decoded validation.",
    ),
    (
        "CLIP_COLLISION",
        Retry::Changed,
        "A placement overlaps another clip under the reject policy.",
    ),
    (
        "CLOCK_ERROR",
        Retry::Never,
        "The system clock could not provide a required timestamp.",
    ),
    (
        "COMMAND_FAILED",
        Retry::Changed,
        "A child command ended without a valid successful result.",
    ),
    (
        "CUT_OUTSIDE_CLIP",
        Retry::Changed,
        "A requested word cut lies outside the clip interval.",
    ),
    (
        "DEVICE_UNAVAILABLE",
        Retry::Same,
        "The selected local compute or capture device is unavailable.",
    ),
    (
        "DIRECT_INDEX_STALE",
        Retry::Changed,
        "A saved direct-source index belongs to different source, decoder or engine identities; verify and refresh it with media.index.",
    ),
    (
        "DISK_FULL",
        Retry::Changed,
        "Storage is full; free space before retrying.",
    ),
    (
        "DUPLICATE_ID",
        Retry::Changed,
        "An identifier duplicates another object in the same scope.",
    ),
    (
        "ENCODE_FAILED",
        Retry::Changed,
        "The external encoder did not complete the requested output.",
    ),
    (
        "ESTIMATED_BOUNDARY",
        Retry::Changed,
        "A word boundary needs explicit acknowledgement of estimated timing.",
    ),
    (
        "EXPORT_VALIDATION_FAILED",
        Retry::Never,
        "Exported data did not pass independent validation.",
    ),
    (
        "EXPRESSION_CYCLE",
        Retry::Changed,
        "Expression dependencies contain a cycle.",
    ),
    (
        "EXPRESSION_DOMAIN",
        Retry::Changed,
        "An expression operation is outside its mathematical domain.",
    ),
    (
        "EXPRESSION_PRECISION",
        Retry::Changed,
        "Expression arithmetic exceeds the exact precision contract.",
    ),
    (
        "EXPRESSION_RANGE",
        Retry::Changed,
        "An expression result exceeds its permitted range.",
    ),
    (
        "EXPRESSION_TYPE",
        Retry::Changed,
        "An expression value or connection has an incompatible type.",
    ),
    (
        "GEOMETRY_CYCLE",
        Retry::Changed,
        "Geometry parent dependencies contain a cycle.",
    ),
    (
        "GEOMETRY_PRECISION",
        Retry::Changed,
        "Geometry evaluation exceeds its numeric precision bound.",
    ),
    (
        "IDENTITY_MISMATCH",
        Retry::Changed,
        "A supplied identity conflicts with the asset or proxy it describes.",
    ),
    (
        "IDENTITY_REQUIRED",
        Retry::Changed,
        "The operation requires an explicit bound content identity.",
    ),
    (
        "INCOMPLETE_CAPTURE",
        Retry::Changed,
        "Capture ended before all required samples arrived.",
    ),
    (
        "INPUT_LIMIT",
        Retry::Changed,
        "Speech input exceeds the configured worker input bound.",
    ),
    (
        "INSUFFICIENT_HANDLES",
        Retry::Changed,
        "A transition requires source frames outside available handles.",
    ),
    (
        "INTERNAL_ERROR",
        Retry::Never,
        "An engine invariant failed; retain the request for diagnosis.",
    ),
    (
        "INVALID_ACCELERATION",
        Retry::Changed,
        "The acceleration value or structure violates its declared constraints.",
    ),
    (
        "INVALID_ALIGNMENT",
        Retry::Changed,
        "The alignment value or structure violates its declared constraints.",
    ),
    (
        "INVALID_ALPHA",
        Retry::Changed,
        "The alpha value or structure violates its declared constraints.",
    ),
    (
        "INVALID_ANIMATION",
        Retry::Changed,
        "The animation value or structure violates its declared constraints.",
    ),
    (
        "INVALID_ARGUMENT",
        Retry::Changed,
        "The argument value or structure violates its declared constraints.",
    ),
    (
        "INVALID_AUDIO",
        Retry::Changed,
        "The audio value or structure violates its declared constraints.",
    ),
    (
        "INVALID_AUDIO_EFFECT",
        Retry::Changed,
        "The audio effect value or structure violates its declared constraints.",
    ),
    (
        "INVALID_AUDIO_REPAIR",
        Retry::Changed,
        "The audio repair value or structure violates its declared constraints.",
    ),
    (
        "INVALID_AUDIO_ROUTING",
        Retry::Changed,
        "The audio routing value or structure violates its declared constraints.",
    ),
    (
        "INVALID_BATCH",
        Retry::Changed,
        "The batch value or structure violates its declared constraints.",
    ),
    (
        "INVALID_BINDING",
        Retry::Changed,
        "The binding value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CACHE",
        Retry::Changed,
        "The cache value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CAPTIONS",
        Retry::Changed,
        "The captions value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CAPTURE_INPUT",
        Retry::Changed,
        "The capture input value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CAPTURE_PACKET",
        Retry::Changed,
        "The capture packet value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CAPTURE_PLACEMENT",
        Retry::Changed,
        "The capture placement value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CLIP_SOURCE",
        Retry::Changed,
        "The clip source value or structure violates its declared constraints.",
    ),
    (
        "INVALID_COLOR",
        Retry::Changed,
        "The color value or structure violates its declared constraints.",
    ),
    (
        "INVALID_CONFORM",
        Retry::Changed,
        "The conform value or structure violates its declared constraints.",
    ),
    (
        "INVALID_EDIT",
        Retry::Changed,
        "The edit value or structure violates its declared constraints.",
    ),
    (
        "INVALID_EFFECT",
        Retry::Changed,
        "The effect value or structure violates its declared constraints.",
    ),
    (
        "INVALID_EXPORT",
        Retry::Changed,
        "The export value or structure violates its declared constraints.",
    ),
    (
        "INVALID_EXPRESSION",
        Retry::Changed,
        "The expression value or structure violates its declared constraints.",
    ),
    (
        "INVALID_GAP",
        Retry::Changed,
        "The gap value or structure violates its declared constraints.",
    ),
    (
        "INVALID_GEOMETRY",
        Retry::Changed,
        "The geometry value or structure violates its declared constraints.",
    ),
    (
        "INVALID_GRAPHIC",
        Retry::Changed,
        "The graphic value or structure violates its declared constraints.",
    ),
    (
        "INVALID_HDR",
        Retry::Changed,
        "The hdr value or structure violates its declared constraints.",
    ),
    (
        "INVALID_ID",
        Retry::Changed,
        "The id value or structure violates its declared constraints.",
    ),
    (
        "INVALID_IDENTITY",
        Retry::Changed,
        "The identity value or structure violates its declared constraints.",
    ),
    (
        "INVALID_IMAGE_SEQUENCE",
        Retry::Changed,
        "The image sequence value or structure violates its declared constraints.",
    ),
    (
        "INVALID_INDEX",
        Retry::Changed,
        "The index value or structure violates its declared constraints.",
    ),
    (
        "INVALID_JSON",
        Retry::Changed,
        "The json value or structure violates its declared constraints.",
    ),
    (
        "INVALID_LIMIT",
        Retry::Changed,
        "The limit value or structure violates its declared constraints.",
    ),
    (
        "INVALID_LOSS_ACKNOWLEDGEMENT",
        Retry::Changed,
        "The loss acknowledgement value or structure violates its declared constraints.",
    ),
    (
        "INVALID_LUT",
        Retry::Changed,
        "The lut value or structure violates its declared constraints.",
    ),
    (
        "INVALID_MASK",
        Retry::Changed,
        "The mask value or structure violates its declared constraints.",
    ),
    (
        "INVALID_MATTE",
        Retry::Changed,
        "The matte value or structure violates its declared constraints.",
    ),
    (
        "INVALID_MEDIA",
        Retry::Changed,
        "The media value or structure violates its declared constraints.",
    ),
    (
        "INVALID_METADATA",
        Retry::Changed,
        "The metadata value or structure violates its declared constraints.",
    ),
    (
        "INVALID_MULTICAM",
        Retry::Changed,
        "The multicam value or structure violates its declared constraints.",
    ),
    (
        "INVALID_NATIVE_TRANSFER",
        Retry::Changed,
        "The native transfer value or structure violates its declared constraints.",
    ),
    (
        "INVALID_PARAMETER",
        Retry::Changed,
        "The parameter value or structure violates its declared constraints.",
    ),
    (
        "INVALID_PATH",
        Retry::Changed,
        "The path value or structure violates its declared constraints.",
    ),
    (
        "INVALID_PROXY",
        Retry::Changed,
        "The proxy value or structure violates its declared constraints.",
    ),
    (
        "INVALID_QUERY",
        Retry::Changed,
        "The query value or structure violates its declared constraints.",
    ),
    (
        "INVALID_RANGE",
        Retry::Changed,
        "The range value or structure violates its declared constraints.",
    ),
    (
        "INVALID_REFRAME",
        Retry::Changed,
        "The reframe value or structure violates its declared constraints.",
    ),
    (
        "INVALID_REMAP",
        Retry::Changed,
        "The remap value or structure violates its declared constraints.",
    ),
    (
        "INVALID_RESULT",
        Retry::Changed,
        "The result value or structure violates its declared constraints.",
    ),
    (
        "INVALID_RETRY",
        Retry::Changed,
        "The retry value or structure violates its declared constraints.",
    ),
    (
        "INVALID_REVISION",
        Retry::Changed,
        "The revision value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SCENE",
        Retry::Changed,
        "The scene value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SCOPE",
        Retry::Changed,
        "The scope value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SELECTION",
        Retry::Changed,
        "The selection value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SEQUENCE",
        Retry::Changed,
        "The sequence value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SEQUENCE_EDIT",
        Retry::Changed,
        "The sequence edit value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SHEET",
        Retry::Changed,
        "The sheet value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SPATIAL",
        Retry::Changed,
        "The spatial value or structure violates its declared constraints.",
    ),
    (
        "INVALID_STABILIZATION",
        Retry::Changed,
        "The stabilization value or structure violates its declared constraints.",
    ),
    (
        "INVALID_SYNC",
        Retry::Changed,
        "The sync value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TEMPLATE",
        Retry::Changed,
        "The template value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TEMPORAL",
        Retry::Changed,
        "The temporal value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TIME",
        Retry::Changed,
        "The time value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TIMECODE",
        Retry::Changed,
        "The timecode value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRACKING",
        Retry::Changed,
        "The tracking value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRACKS",
        Retry::Changed,
        "The tracks value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRACK_EDIT",
        Retry::Changed,
        "The track edit value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRANSCRIPT",
        Retry::Changed,
        "The transcript value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRANSCRIPTION",
        Retry::Changed,
        "The transcription value or structure violates its declared constraints.",
    ),
    (
        "INVALID_TRANSITION",
        Retry::Changed,
        "The transition value or structure violates its declared constraints.",
    ),
    (
        "IO_ERROR",
        Retry::Changed,
        "A local filesystem operation failed; repair the named path or permissions.",
    ),
    (
        "ISOLATION_REQUIRED",
        Retry::Changed,
        "The speech worker requires the configured process isolation.",
    ),
    (
        "JOB_CANCELLED",
        Retry::Changed,
        "The durable job was cancelled; the same start request will replay its ticket.",
    ),
    (
        "JOB_LAUNCH_FAILED",
        Retry::Same,
        "A local job worker could not be launched.",
    ),
    (
        "JOB_NOT_FOUND",
        Retry::Changed,
        "The requested job ID is absent from the queue.",
    ),
    (
        "JOB_STALLED",
        Retry::Changed,
        "A worker stopped reporting progress; inspect its terminal ticket before a new submission.",
    ),
    (
        "JOB_TIMEOUT",
        Retry::Changed,
        "A job exceeded its deadline; inspect its terminal ticket before a new submission.",
    ),
    (
        "LIMIT_EXCEEDED",
        Retry::Changed,
        "The requested work exceeds a declared engine limit.",
    ),
    (
        "LINKED_SELECTION",
        Retry::Changed,
        "The edit requires the complete linked clip selection.",
    ),
    (
        "LOSSY_CAPTION_EXPORT",
        Retry::Changed,
        "Caption export would lose unsupported semantics without acknowledgement.",
    ),
    (
        "LOUDNESS_UNMEASURED",
        Retry::Changed,
        "The selected sound has no usable loudness measurement.",
    ),
    (
        "MANAGED_SEQUENCE",
        Retry::Changed,
        "A managed sequence must be edited through its owning camera group.",
    ),
    (
        "MEDIA_CHANGED",
        Retry::Changed,
        "Current source bytes differ from the bound identity; inspect and rebind explicitly.",
    ),
    (
        "MEDIA_DURATION_MISMATCH",
        Retry::Changed,
        "Probed media duration disagrees with the declared source duration.",
    ),
    (
        "MIGRATION_REQUIRED",
        Retry::Changed,
        "The old store needs explicit session.migrate before new writes.",
    ),
    (
        "MISSING_ANGLE",
        Retry::Changed,
        "The requested angle is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_ASSET",
        Retry::Changed,
        "The requested asset is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_CLIP",
        Retry::Changed,
        "The requested clip is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_CUT",
        Retry::Changed,
        "The requested cut is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_GLYPH",
        Retry::Changed,
        "The requested glyph is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_LINK",
        Retry::Changed,
        "The requested link is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_MEDIA",
        Retry::Changed,
        "The requested media is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_NEIGHBOR",
        Retry::Changed,
        "The requested neighbor is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_PARAMETER",
        Retry::Changed,
        "The requested parameter is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_SEQUENCE",
        Retry::Changed,
        "The requested sequence is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_TRACK",
        Retry::Changed,
        "The requested track is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_TRANSCRIPT",
        Retry::Changed,
        "The requested transcript is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_TRANSITION",
        Retry::Changed,
        "The requested transition is absent; use available IDs or provide the required input.",
    ),
    (
        "MISSING_WORD",
        Retry::Changed,
        "The requested word is absent; use available IDs or provide the required input.",
    ),
    (
        "MODEL_CHANGED",
        Retry::Changed,
        "Speech model bytes differ from the configured identity.",
    ),
    (
        "MODEL_UNAVAILABLE",
        Retry::Changed,
        "The configured local speech model is missing or unavailable.",
    ),
    (
        "MULTICAM_PROJECTION_MISMATCH",
        Retry::Never,
        "Saved camera decisions and their projected sequence disagree.",
    ),
    (
        "NATIVE_ADAPTER_MISMATCH",
        Retry::Changed,
        "The declared native-project adapter identity does not match.",
    ),
    (
        "NATIVE_SOURCE_MISMATCH",
        Retry::Changed,
        "Native-project source identities do not match the supplied bindings.",
    ),
    (
        "NOTHING_TO_UNDO",
        Retry::Changed,
        "The saved session has no earlier edit to undo.",
    ),
    (
        "NO_WORDS",
        Retry::Changed,
        "Speech analysis returned no usable words in the requested interval.",
    ),
    (
        "OUTPUT_EXISTS",
        Retry::Changed,
        "The output path already exists and will not be overwritten.",
    ),
    (
        "OUTPUT_RESERVED",
        Retry::Changed,
        "Another queued job already reserves this output path.",
    ),
    (
        "OWNER_GONE",
        Retry::Same,
        "The process owning the speech request ended.",
    ),
    (
        "PATH_OUTSIDE_ROOT",
        Retry::Changed,
        "A resolved path leaves the explicitly allowed file root.",
    ),
    (
        "PATH_OUTSIDE_WORKSPACE",
        Retry::Changed,
        "A resolved path leaves the configured workspace.",
    ),
    (
        "PROJECT_EXISTS",
        Retry::Changed,
        "A saved project with this ID already exists.",
    ),
    (
        "PROJECT_NOT_FOUND",
        Retry::Changed,
        "The requested saved project is absent.",
    ),
    (
        "PROXY_MISSING",
        Retry::Changed,
        "An enabled bound proxy cannot be found.",
    ),
    (
        "PUBLICATION_CONFLICT",
        Retry::Never,
        "Existing publication bytes or receipts conflict; preserve them for diagnosis.",
    ),
    (
        "PUBLISH_FAILED",
        Retry::Changed,
        "The verified temporary output could not be published without overwrite.",
    ),
    (
        "QUEUE_FULL",
        Retry::Same,
        "The local queue has reached its outstanding-job bound.",
    ),
    (
        "REFRAME_INFEASIBLE",
        Retry::Changed,
        "The authored boxes and crop constraints have no feasible framing.",
    ),
    (
        "REFRAME_UNAVAILABLE",
        Retry::Changed,
        "The requested reframing mode cannot use this source.",
    ),
    (
        "RENDER_VALIDATION_FAILED",
        Retry::Never,
        "Rendered pixels, samples, metadata or counts failed validation.",
    ),
    (
        "REQUEST_ID_CONFLICT",
        Retry::Changed,
        "An idempotency key was already used with different arguments.",
    ),
    (
        "REQUEST_NOT_FOUND",
        Retry::Changed,
        "No saved receipt exists for this request ID.",
    ),
    (
        "RESOURCE_LIMIT",
        Retry::Changed,
        "The speech worker exceeds its configured resource bound.",
    ),
    (
        "RESULT_LIMIT",
        Retry::Changed,
        "The worker result exceeds its declared bounded size.",
    ),
    (
        "REVISION_CONFLICT",
        Retry::Changed,
        "The expected revision is stale; refresh, reconsider, and change the request.",
    ),
    (
        "REVISION_NOT_FOUND",
        Retry::Changed,
        "The requested saved revision does not exist.",
    ),
    (
        "RUNTIME_UNAVAILABLE",
        Retry::Changed,
        "The configured local speech runtime is unavailable.",
    ),
    (
        "RUNTIME_VERSION",
        Retry::Changed,
        "The installed speech runtime does not match the required version.",
    ),
    (
        "SEQUENCE_CYCLE",
        Retry::Changed,
        "Nested sequence references contain a cycle.",
    ),
    (
        "SEQUENCE_IN_USE",
        Retry::Changed,
        "A sequence still has references and cannot be removed.",
    ),
    (
        "SPLIT_ID_REQUIRED",
        Retry::Changed,
        "The edit creates a surviving clip that needs an explicit ID.",
    ),
    (
        "STABILIZATION_CROP_LIMIT",
        Retry::Changed,
        "Stabilization would exceed the declared crop or zoom limit.",
    ),
    (
        "STABILIZATION_UNRELIABLE",
        Retry::Changed,
        "Measured camera motion does not meet the declared confidence limits.",
    ),
    (
        "STORE_BUSY",
        Retry::Same,
        "Another local transaction holds the store lock.",
    ),
    (
        "STORE_CORRUPT",
        Retry::Never,
        "Saved storage fails integrity checks; use explicit recovery, not blind retries.",
    ),
    (
        "STORE_ERROR",
        Retry::Changed,
        "A database operation failed for a non-transient storage reason.",
    ),
    (
        "STORE_NOT_FOUND",
        Retry::Changed,
        "The requested saved store does not exist.",
    ),
    (
        "SYNC_AMBIGUOUS",
        Retry::Changed,
        "Multiple synchronization offsets fit the measured signal.",
    ),
    (
        "SYNC_CONFLICT",
        Retry::Changed,
        "Linked or synchronized clip placements disagree.",
    ),
    (
        "SYNC_DRIFT_EXCEEDED",
        Retry::Changed,
        "Measured clock drift exceeds the allowed correction bound.",
    ),
    (
        "SYNC_INCONSISTENT",
        Retry::Changed,
        "Independent synchronization windows disagree.",
    ),
    (
        "SYNC_INSUFFICIENT_SIGNAL",
        Retry::Changed,
        "The selected sound does not provide enough synchronization evidence.",
    ),
    (
        "TEXT_OVERFLOW",
        Retry::Changed,
        "Text exceeds its declared box, line or glyph bound.",
    ),
    (
        "TEXT_PLAN_STALE",
        Retry::Changed,
        "The transcript edit proposal no longer matches the saved timeline.",
    ),
    (
        "TIME_OVERFLOW",
        Retry::Changed,
        "Exact time arithmetic exceeds the supported integer range.",
    ),
    (
        "TOOL_ABORTED",
        Retry::Changed,
        "An external tool was cancelled by the controlling operation.",
    ),
    (
        "TOOL_CHANGED",
        Retry::Changed,
        "An external executable differs from the identity pinned to the job.",
    ),
    (
        "TOOL_FAILED",
        Retry::Changed,
        "An external tool returned an unsuccessful result.",
    ),
    (
        "TOOL_TIMEOUT",
        Retry::Same,
        "An external tool exceeded its deadline.",
    ),
    (
        "TOOL_UNAVAILABLE",
        Retry::Changed,
        "A required external executable could not be started.",
    ),
    (
        "TOO_MANY_COLORS",
        Retry::Changed,
        "The lossless palette output exceeds the supported color count.",
    ),
    (
        "TRACKING_UNRELIABLE",
        Retry::Changed,
        "Tracking confidence failed; detail may retain accepted observations.",
    ),
    (
        "TRACK_LOCKED",
        Retry::Changed,
        "The edit would alter a locked track.",
    ),
    (
        "TRANSCRIPTION_FAILED",
        Retry::Changed,
        "The speech worker reported an unrecognized failure.",
    ),
    (
        "TRANSCRIPT_CONFLICT",
        Retry::Changed,
        "The transcript revision or fingerprint differs from the expected value.",
    ),
    (
        "TRANSCRIPT_SOURCE_MISMATCH",
        Retry::Changed,
        "Transcript words refer to a different bound media source.",
    ),
    (
        "TRANSITION_COLLISION",
        Retry::Changed,
        "Transition intervals overlap contrary to the supported layout.",
    ),
    (
        "TRANSITION_CONFLICT",
        Retry::Changed,
        "The edit would invalidate an existing transition without a policy.",
    ),
    (
        "UNALIGNED_TIME",
        Retry::Changed,
        "A time does not fall on the required exact frame or sample boundary.",
    ),
    (
        "UNKNOWN_PARAMETER",
        Retry::Changed,
        "A template value names a parameter the template does not declare.",
    ),
    (
        "UNKNOWN_SCHEMA",
        Retry::Changed,
        "The requested schema name or selection is absent.",
    ),
    (
        "UNKNOWN_SECTION",
        Retry::Changed,
        "The requested capabilities section is absent.",
    ),
    (
        "UNSUPPORTED_ACCELERATION",
        Retry::Changed,
        "The requested acceleration is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_ALIGNMENT_TEXT",
        Retry::Changed,
        "The requested alignment text is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_AUDIO",
        Retry::Changed,
        "The requested audio is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_CAPTIONS",
        Retry::Changed,
        "The requested captions is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_FONT",
        Retry::Changed,
        "The requested font is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_GEOMETRY",
        Retry::Changed,
        "The requested geometry is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_IMAGE",
        Retry::Changed,
        "The requested image is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_INTERCHANGE",
        Retry::Changed,
        "The requested interchange is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_JOB",
        Retry::Changed,
        "The requested job is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_MEDIA",
        Retry::Changed,
        "The requested media is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_MULTICAM",
        Retry::Changed,
        "The requested multicam is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_NATIVE_BUILD",
        Retry::Changed,
        "The requested native build is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_OUTPUT",
        Retry::Changed,
        "The requested output is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_PLATFORM",
        Retry::Never,
        "The requested platform is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_PREVIEW",
        Retry::Changed,
        "The requested preview is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_SCENE",
        Retry::Changed,
        "The requested scene is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_SCHEMA",
        Retry::Changed,
        "The requested schema is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_STORE",
        Retry::Changed,
        "The requested store is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_TEXT",
        Retry::Changed,
        "The requested text is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_TIMECODE",
        Retry::Changed,
        "The requested timecode is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_TIMELINE",
        Retry::Changed,
        "The requested timeline is outside the supported profile.",
    ),
    (
        "UNSUPPORTED_TRANSCRIPT_BINDING",
        Retry::Changed,
        "The requested transcript binding is outside the supported profile.",
    ),
    (
        "UNUSED_SPLIT_ID",
        Retry::Changed,
        "The edit supplied a split ID but produces no corresponding survivor.",
    ),
    (
        "WORD_OUTSIDE_CLIP",
        Retry::Changed,
        "A selected transcript word lies outside the clip source interval.",
    ),
    (
        "WORKER_CHANGED",
        Retry::Changed,
        "The local speech worker differs from its configured identity.",
    ),
    (
        "WORKER_FAILED",
        Retry::Changed,
        "The speech worker process did not return a valid result.",
    ),
    (
        "WORKER_INTERRUPTED",
        Retry::Same,
        "The durable worker was interrupted; inspect and resume its ticket.",
    ),
    (
        "WORKER_LIFETIME",
        Retry::Never,
        "The worker could not be contained within the required process lifetime.",
    ),
    (
        "WORKER_SETUP_FAILED",
        Retry::Changed,
        "The background worker could not initialize its required local environment.",
    ),
    (
        "WORKER_TIMEOUT",
        Retry::Same,
        "The local speech worker exceeded its request deadline.",
    ),
];

pub fn retry_class(code: &str) -> Option<Retry> {
    REGISTRY
        .binary_search_by_key(&code, |entry| entry.0)
        .ok()
        .map(|i| REGISTRY[i].1)
}

pub(crate) fn hint(code: &str) -> Option<&'static str> {
    match code {
        "OUTPUT_EXISTS" => {
            Some("Choose an unused output path; existing files are never overwritten.")
        }
        "IO_ERROR" => {
            Some("Check the named local path, its parent directory and access permissions.")
        }
        "UNSUPPORTED_JOB" => Some(
            "Choose a run command from detail.queueable_commands; use render.start for a reference render.",
        ),
        "MIGRATION_REQUIRED" => {
            Some("Run session.migrate on this store explicitly before retrying the write.")
        }
        "REVISION_CONFLICT" => Some(
            "Read the current snapshot, reconsider the edit, then use detail.current_revision as expected_revision.",
        ),
        "MISSING_CLIP" | "MISSING_TRACK" | "MISSING_MEDIA" | "MISSING_ASSET" => Some(
            "Choose an existing ID from detail.available when present, or add/relink the missing object first.",
        ),
        "INVALID_JSON" => {
            Some("Correct the field named by path to match the command's argument contract.")
        }
        "WORKER_INTERRUPTED" => Some(
            "Inspect the ticket; job.resume runs queued or opted-in interrupted attempts within their limit. A terminal ticket needs an explicit new submission after diagnosis.",
        ),
        "JOB_STALLED" | "JOB_TIMEOUT" => Some(
            "Inspect the terminal ticket and reduce the work or resolve the stall before submitting a new request ID; job.resume cannot restart terminal failures.",
        ),
        "TOOL_TIMEOUT" | "WORKER_TIMEOUT" => Some(
            "Retry after temporary load clears. For durable jobs, inspect the ticket first: job.resume runs queued eligible attempts, but terminal failures need an explicit new submission.",
        ),
        "STORE_BUSY" => {
            Some("Wait briefly for the current transaction, then retry the identical request.")
        }
        "QUEUE_FULL" => {
            Some("Wait for outstanding jobs to finish, then retry the identical request.")
        }
        _ => None,
    }
}

pub(crate) fn detail(code: &str) -> Option<Value> {
    (code == "UNSUPPORTED_JOB").then(|| json!({"queueable_commands":crate::jobs::QUEUED_COMMANDS}))
}

/// Preserve the caller's existing message while exposing the actual current revision.
pub(crate) fn revision(expected: u64, current: u64, message: impl Into<String>) -> crate::Error {
    crate::error("REVISION_CONFLICT", message)
        .with_path("expected_revision")
        .with_detail(json!({"expected_revision":expected,"current_revision":current}))
}

/// A free name is a suggestion only; ordinary no-overwrite checks still decide publication.
pub(crate) fn output_exists(output: &Path, message: impl Into<String>) -> crate::Error {
    let mut failure = crate::error("OUTPUT_EXISTS", message).with_path("output");
    if let Some(stem) = output.file_stem() {
        for number in 1..=1000 {
            let mut name = stem.to_os_string();
            name.push(format!("-{number}"));
            if let Some(extension) = output.extension() {
                name.push(".");
                name.push(extension);
            }
            let candidate = output.with_file_name(name);
            if candidate
                .symlink_metadata()
                .is_err_and(|failure| failure.kind() == std::io::ErrorKind::NotFound)
            {
                failure.detail = Some(json!({"suggested_output":candidate}));
                break;
            }
        }
    }
    failure
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn output_suggestions_are_unused_siblings_and_preserve_existing_bytes() {
        let scratch = crate::tracking::Scratch::new("error-output-suggestion");
        let output = scratch.0.join("delivery.mp4");
        let occupied = scratch.0.join("delivery-1.mp4");
        std::fs::write(&output, b"original").unwrap();
        std::fs::write(&occupied, b"other original").unwrap();
        let failure = output_exists(&output, "old output message");
        assert_eq!(failure.path.as_deref(), Some("output"));
        assert_eq!(failure.retry, Some(Retry::Changed));
        assert_eq!(
            failure.detail.unwrap()["suggested_output"],
            json!(scratch.0.join("delivery-2.mp4"))
        );
        assert_eq!(std::fs::read(output).unwrap(), b"original");
        assert_eq!(std::fs::read(occupied).unwrap(), b"other original");
        assert_eq!(std::fs::read_dir(&scratch.0).unwrap().count(), 2);
    }

    fn source_codes(directory: &Path, found: &mut BTreeSet<String>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                source_codes(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                // Includes literal error/missing call sites and codes chosen through a match or
                // forwarded from a worker. All public engine codes use upper-case snake case.
                // Scanning every quoted candidate also catches new branches not hit by a test.
                for word in source.split('"') {
                    if word.bytes().next().is_some_and(|c| c.is_ascii_uppercase())
                        && word.contains('_')
                        && word
                            .bytes()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
                        && !word.starts_with("CUTBOLT_")
                        && !word.starts_with("CARGO_")
                        && !word.starts_with("VIDEO_ENGINE_TEST_")
                        && !matches!(
                            word,
                            "DEFAULT_MEDIA"
                                | "DOMAIN_MAX"
                                | "DOMAIN_MIN"
                                | "LUT_1D_SIZE"
                                | "LUT_3D_SIZE"
                                | "OTIO_SCHEMA"
                                | "OUT_DIR"
                        )
                    {
                        found.insert(word.to_owned());
                    }
                }
            }
        }
    }

    #[test]
    fn every_source_error_code_has_a_class_and_documented_meaning() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut used = BTreeSet::new();
        source_codes(&root.join("src"), &mut used);
        assert!(used.len() >= 234);
        for code in &used {
            assert!(
                retry_class(code).is_some(),
                "Unregistered error code {code}"
            );
        }
        assert!(REGISTRY.windows(2).all(|pair| pair[0].0 < pair[1].0));
        let docs = std::fs::read_to_string(root.join("docs/AGENT_INTERFACE.md")).unwrap();
        let table = docs
            .split("<!-- error-registry:start -->")
            .nth(1)
            .unwrap()
            .split("<!-- error-registry:end -->")
            .next()
            .unwrap();
        for &(code, class, meaning) in REGISTRY {
            assert!(!meaning.is_empty());
            let class = serde_json::to_value(class).unwrap();
            let row = format!("| `{code}` | `{}` | {meaning} |", class.as_str().unwrap());
            assert!(
                table.lines().any(|line| line == row),
                "Missing or stale documentation for {code}"
            );
        }
        assert_eq!(
            table.lines().filter(|line| line.starts_with("| `")).count(),
            REGISTRY.len()
        );
        assert_eq!(retry_class("unregistered"), None);
    }

    #[test]
    fn recovery_guidance_is_structured_and_unknown_codes_are_not_guessed() {
        let conflict = revision(2, 7, "old message");
        let value = serde_json::to_value(conflict).unwrap();
        assert_eq!(value["message"], "old message");
        assert_eq!(value["retry"], "changed");
        assert_eq!(value["path"], "expected_revision");
        assert_eq!(value["detail"]["current_revision"], 7);
        assert_eq!(
            crate::error("TOOL_TIMEOUT", "late").retry,
            Some(Retry::Same)
        );
        assert_eq!(
            crate::error("STORE_CORRUPT", "bad").retry,
            Some(Retry::Never)
        );
        let unknown = serde_json::to_value(crate::error("private", "old private message")).unwrap();
        assert_eq!(
            unknown,
            json!({"code":"private","message":"old private message"})
        );
        let reported = json!({"code":"INVALID_JSON","message":"old message","path":"scene.width","hint":"Correct width","detail":{"given":"bad"}});
        let restored = crate::Error::reported("INVALID_JSON", &reported);
        assert_eq!(restored.path.as_deref(), Some("scene.width"));
        assert_eq!(restored.hint.as_deref(), Some("Correct width"));
        assert_eq!(restored.detail.unwrap()["given"], "bad");
    }
}
