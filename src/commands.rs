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
        /// Project whose media paths to make relative.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Existing absolute media root; proposed relative paths resolve inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "session.check")]
    SessionCheck {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
    },
    #[serde(rename = "session.migrate")]
    SessionMigrate {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
    },
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
        /// Project to describe as OTIO.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    /// Publish an OTIO timeline to an unused local output after explicit loss acknowledgement.
    #[serde(rename = "interchange.export")]
    InterchangeExport(crate::interchange::Export),
    #[serde(rename = "cache.run")]
    CacheRun(crate::cache::Request),
    #[serde(rename = "cache.inspect")]
    CacheInspect {
        /// Absolute local cache directory used by cache.run.
        cache_root: PathBuf,
    },
    #[serde(rename = "cache.prune")]
    CachePrune {
        /// Absolute local cache directory used by cache.run.
        cache_root: PathBuf,
        /// Byte and entry budgets; least-recently-used derived entries are evicted until both are met.
        policy: crate::cache::Policy,
    },
    #[serde(rename = "preview.sheet")]
    PreviewSheet {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Frame times and grid layout of the contact sheet.
        spec: preview::Sheet,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .png file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "preview.cuts")]
    PreviewCuts {
        /// Project whose cuts to review.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .png file inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// List cuts at or after this timeline time; default zero. Pass the previous result's `next` to continue.
        #[serde(default)]
        start: Option<Time>,
        /// Cuts on this sheet, 1-16; default 16.
        #[serde(default)]
        limit: Option<usize>,
        /// Cell width in pixels, 16-480; default 192. Cell height follows the project's aspect ratio.
        #[serde(default)]
        tile_width: Option<u32>,
    },
    #[serde(rename = "audio.normalize")]
    AudioNormalize {
        /// Project whose mix to normalize; it needs placed tracks, and audio tracks with clips must be unlocked.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Integrated loudness to reach, -40 to -5 LKFS; default -14, common for online video.
        #[serde(default)]
        target_lkfs: Option<f64>,
        /// Highest allowed sample peak after the change, -20 to 0 dBFS; default -1. When the target would pass it, the gain stops at the ceiling.
        #[serde(default)]
        peak_ceiling_dbfs: Option<f64>,
    },
    #[serde(rename = "audio.tighten")]
    AudioTighten {
        /// Project to tighten: placed tracks with a voice track, or a sequential timeline analysed whole.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Audio track whose speech sets the pauses; required for placed tracks, omitted for sequential timelines.
        #[serde(default)]
        voice_track_id: Option<String>,
        /// Mean power over 10 ms windows that counts as speech, -80 to 0 dBFS; default -45.
        #[serde(default)]
        threshold_db: Option<i32>,
        /// Shortest silence that counts as a pause, 0.2-10 s; default 3/4.
        #[serde(default)]
        min_pause: Option<Time>,
        /// Silence kept on each side of a cut; twice it must be shorter than min_pause. Default 1/5.
        #[serde(default)]
        keep: Option<Time>,
        /// Also cut silence before the first and after the last speech, keeping `keep` next to the speech; default true.
        #[serde(default = "crate::commands::yes")]
        edges: bool,
        /// Cut only pauses after this timeline time; default the start.
        #[serde(default)]
        start: Option<Time>,
        /// Cut only pauses before this timeline time; default the end.
        #[serde(default)]
        end: Option<Time>,
    },
    #[serde(rename = "color.match")]
    ColorMatch {
        /// Shot whose colour to match: a video file, absolute or relative to input_root; prepared FFV1 RGB assets measure exactly.
        reference: PathBuf,
        /// Shot to correct, in the same forms.
        target: PathBuf,
        /// Reference times to sample, 1-64; default nine frames spread through the file.
        #[serde(default)]
        reference_times: Option<Vec<Time>>,
        /// Target times to sample, 1-64; default nine frames spread through the file.
        #[serde(default)]
        target_times: Option<Vec<Time>>,
        /// Existing absolute directory containing both files; the .cube output must also lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .cube table inside output_root.
        output: PathBuf,
        /// `levels` (default) matches each channel's mean and spread; `histogram` matches whole distributions.
        #[serde(default)]
        method: Option<crate::color_match::Method>,
        /// Encoded transfer of an RGB target asset, used as its declared and working transfer; default bt709.
        #[serde(default)]
        transfer: Option<crate::color::Transfer>,
    },
    #[serde(rename = "audio.beats")]
    AudioBeats {
        /// Music to analyse: any file FFmpeg decodes with an audio stream.
        path: PathBuf,
        /// Existing absolute directory that must contain `path`.
        input_root: PathBuf,
        /// File time to start at, on the 48 kHz grid; default zero. Reported times are file times.
        #[serde(default)]
        start: Option<Time>,
        /// Length to analyse, at most one hour; default to the end.
        #[serde(default)]
        duration: Option<Time>,
        /// Slowest tempo considered, 30-300 BPM; default 60.
        #[serde(default)]
        min_bpm: Option<u32>,
        /// Fastest tempo considered, 30-300 BPM and at least 1.5 times min_bpm; default 200.
        #[serde(default)]
        max_bpm: Option<u32>,
        /// Also give each listed beat's nearest frame start at this native rate, for placing cuts.
        #[serde(default)]
        frame_rate: Option<Time>,
    },
    #[serde(rename = "audio.duck")]
    AudioDuck {
        /// Project whose music to lower under speech; it needs placed tracks.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Audio track carrying speech, such as dialogue; its audio alone is analyzed.
        voice_track_id: String,
        /// Audio track whose clips are lowered while speech plays.
        music_track_id: String,
        /// Speech threshold in dBFS over 10 ms windows of the voice track, -80..0; default -45.
        #[serde(default)]
        threshold_db: Option<i32>,
        /// Music gain during speech, in milli-units of each clip's gain (1000 = unchanged), 0..1000; default 250, about -12 dB.
        #[serde(default)]
        duck_milli: Option<u32>,
        /// Ramp down before speech starts, on the 48 kHz grid, at most 10 s; default 0.25 s.
        #[serde(default)]
        attack: Option<Time>,
        /// Ramp back up after speech ends, on the 48 kHz grid, at most 10 s; default 0.6 s.
        #[serde(default)]
        release: Option<Time>,
        /// Pauses shorter than this stay ducked; default 1 s.
        #[serde(default)]
        bridge: Option<Time>,
    },
    #[serde(rename = "media.prepare")]
    MediaPrepare {
        /// File to prepare: any video FFmpeg decodes, such as a phone or camera MP4, or a PCM16 WAV voice-over or music track (with `project`; it gets a silent black picture for an audio track). Give this or `paths`.
        #[serde(default)]
        path: Option<PathBuf>,
        /// Several video files to prepare in one job, 1-200, absolute or relative to input_root. Each is prepared as `path` would be; asset IDs come from the file names, made unique, and the result includes `media.add` operations for the prepared assets.
        #[serde(default)]
        paths: Option<Vec<PathBuf>>,
        /// Existing absolute directory that must contain `path`.
        input_root: PathBuf,
        /// Existing absolute directory for the converted asset.
        output_root: PathBuf,
        /// New .mkv path inside output_root for a conversion; default `<name>-prepared.mkv` there.
        #[serde(default)]
        output: Option<PathBuf>,
        /// Target project: the asset takes its frame rate and size. Omit to keep the source's own rate (when it is a timeline rate) and size.
        #[serde(default)]
        #[schemars(with = "Option<crate::reference::ProjectInput>")]
        project: Option<Project>,
    },
    #[serde(rename = "export.review")]
    ExportReview {
        /// Rendered cut to review: any file FFmpeg decodes, such as an export.run or render.start output.
        path: PathBuf,
        /// Existing absolute directory that must contain `path`; project media paths also resolve against it.
        input_root: PathBuf,
        /// Existing absolute directory; the review folder is created inside it.
        output_root: PathBuf,
        /// Absolute path of the new review folder inside output_root; it must not exist.
        output: PathBuf,
        /// Project the cut was rendered from: enables the duration check and, with `transcripts`, the speech check.
        #[serde(default)]
        #[schemars(with = "Option<crate::reference::ProjectInput>")]
        project: Option<Project>,
        /// Transcripts of the project's source media, as for timeline.outline; the words inside audible audio clips are what the cut should say.
        #[serde(default)]
        transcripts: Vec<crate::transcript::Document>,
        /// Transcripts of the reviewed file itself, for example from an earlier review; their source identity must be the file's. Give these or `runtime`.
        #[serde(default)]
        heard: Vec<crate::transcript::Document>,
        /// Local speech runtime, as for transcript.transcribe without `alignment_root` changes: the cut's audio is transcribed in overlapping 120 s windows and saved with the documents in the review folder.
        #[serde(default)]
        runtime: Option<crate::transcribe::Runtime>,
        /// Spoken language for `runtime`; required with it.
        #[serde(default)]
        language: Option<crate::transcript::Language>,
        /// Frames on the contact sheet, 1-64; default 16, spread evenly through the cut.
        #[serde(default)]
        frames: Option<u32>,
        /// Height of the small H.264/AAC copy, 120-1080 pixels and never above the source; default 360. 0 skips it.
        #[serde(default)]
        rendition_height: Option<u32>,
        /// Largest time between an expected and a heard word that still match, in rational seconds; default 1/2, at most 5.
        #[serde(default)]
        tolerance: Option<Time>,
    },
    #[serde(rename = "media.transcribe")]
    MediaTranscribe {
        /// Source to recognize: any file FFmpeg decodes with an audio stream, such as a timeline asset.
        path: PathBuf,
        /// Existing absolute directory that must contain `path`; documents name the source relative to it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .json file inside output_root for `{"transcripts": [documents]}`.
        output: PathBuf,
        /// Local speech runtime, as for transcript.transcribe.
        runtime: crate::transcribe::Runtime,
        /// Spoken language.
        language: crate::transcript::Language,
        /// Channel to analyse; default `mean`.
        #[serde(default)]
        channel: Option<crate::transcribe::Channel>,
        /// Document ID prefix; documents are `<id>-1`, `<id>-2`, and so on. Default `transcript`.
        #[serde(default)]
        id: Option<String>,
        /// Source time to start at, on the 48 kHz grid; default zero.
        #[serde(default)]
        start: Option<Time>,
        /// Length to recognize, on the 48 kHz grid; default to the end of the audio.
        #[serde(default)]
        duration: Option<Time>,
        /// Deadline per 120 s window in seconds, 1-600; default 600.
        #[serde(default)]
        timeout_seconds: Option<u32>,
        /// Known spoken text, such as the script of a synthesized narration: its words are aligned to the audio instead of recognized, so names and spelling stay as written and nothing is misheard. The range must be at most 120 s. Words split at whitespace; write numbers out in words. At most 32 KiB.
        #[serde(default)]
        text: Option<String>,
    },
    #[serde(rename = "media.sheet")]
    MediaSheet {
        /// Video file to sample; any format FFmpeg decodes, not only timeline sources.
        path: PathBuf,
        /// Existing absolute directory that must contain `path`.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .png file inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// 1-64 source times in rational seconds, in cell order; omit to spread `count` frames evenly.
        #[serde(default)]
        times: Option<Vec<Time>>,
        /// Evenly spread frames, 1-64, when `times` is omitted; default 16.
        #[serde(default)]
        count: Option<u32>,
        /// Cells per row, 1-8; default 4.
        #[serde(default)]
        columns: Option<u32>,
        /// Cell width in pixels, 16-480; default 192. Cell height follows the source's aspect ratio.
        #[serde(default)]
        tile_width: Option<u32>,
    },
    #[serde(rename = "media.shots")]
    MediaShots {
        /// Video file to analyze; any format FFmpeg decodes.
        path: PathBuf,
        /// Existing absolute directory that must contain `path`.
        input_root: PathBuf,
        /// Cut threshold on the 0-255 mean absolute difference of 64x36 gray frames; default 20.
        #[serde(default)]
        threshold: Option<u8>,
        /// Shortest shot in frames; closer cuts are ignored. Default 6.
        #[serde(default)]
        minimum_frames: Option<u32>,
        /// Existing absolute directory for the optional shot sheet.
        #[serde(default)]
        output_root: Option<PathBuf>,
        /// New .png for a sheet of each shot's middle frame (first 64 shots); omit for the list alone.
        #[serde(default)]
        output: Option<PathBuf>,
        /// Sheet cell width in pixels, 16-480; default 192.
        #[serde(default)]
        tile_width: Option<u32>,
    },
    #[serde(rename = "transcript.transcribe")]
    Transcribe(crate::transcribe::Transcribe),
    #[serde(rename = "transcript.inspect")]
    TranscriptInspect {
        /// Transcript document as returned by transcript.transcribe or transcript.correct.
        document: crate::transcript::Document,
        /// Existing absolute directory containing the transcript's bound source media.
        input_root: PathBuf,
    },
    #[serde(rename = "transcript.correct")]
    TranscriptCorrect {
        /// Transcript document as returned by transcript.transcribe or transcript.correct.
        document: crate::transcript::Document,
        /// Fingerprint of the supplied document as reported by transcript.inspect; a mismatch rejects the edit.
        expected_fingerprint: String,
        /// Word text and timing corrections, applied in order.
        edits: Vec<crate::transcript::Edit>,
        /// Existing absolute directory containing the transcript's bound source media.
        input_root: PathBuf,
    },
    #[serde(rename = "transcript.plan")]
    TranscriptPlan {
        /// Project containing the clip to cut.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Transcript document as returned by transcript.transcribe or transcript.correct.
        document: crate::transcript::Document,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Fingerprint of the supplied transcript as reported by transcript.inspect.
        expected_document_fingerprint: String,
        /// The clip and the words to cut from it.
        spec: crate::transcript_cut::Spec,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "audio.inputs")]
    AudioInputs {},
    #[serde(rename = "audio.record.inspect")]
    AudioRecordInspect {
        /// Explicit capture endpoint or process selection, as listed by audio.inputs.
        input: crate::recording::Input,
        /// Recording length in rational seconds.
        duration: Time,
    },
    #[serde(rename = "audio.record")]
    AudioRecord(crate::recording::Record),
    #[serde(rename = "audio.record.place")]
    AudioRecordPlace(crate::recording::Place),
    #[serde(rename = "audio.repair.inspect")]
    AudioRepairInspect {
        /// Cleanup recipe: bound PCM source, output and noise-only ranges, and processing settings.
        recipe: crate::audio_repair::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "audio.repair.render")]
    AudioRepairRender {
        /// Cleanup recipe, normally checked first with audio.repair.inspect.
        recipe: crate::audio_repair::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .wav file inside output_root; existing files are never overwritten.
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
        /// Conversion recipe: bound source, color and display interpretation, tone policy and frame mapping.
        recipe: crate::hdr::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "hdr.conform")]
    HdrConform {
        /// Conversion recipe, normally checked first with hdr.inspect.
        recipe: crate::hdr::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "lut.inspect")]
    LutInspect {
        /// Identity-bound .cube file and its interpolation.
        transform: crate::lut::Transform,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Up to 256 RGB input colors to evaluate through the table; omit for none.
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
        /// Preset to return: green or blue screen, with a soft or hard edge.
        name: crate::keying::Preset,
        /// Key strength from 0 to 1000, where 1000 removes the key color fully.
        strength_milli: u16,
        /// Spill suppression strength from 0 to 1000.
        spill_milli: u16,
    },
    #[serde(rename = "captions.import")]
    CaptionsImport {
        /// Identity-bound subtitle file: path inside input_root, SHA-256 and byte count.
        source: scene::Identity,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Format of the source file.
        format: crate::captions::Format,
        /// ID for the new caption document.
        id: String,
        /// Whether cues may overlap in time (allow) or are rejected when they do (reject).
        overlap: crate::captions::Overlap,
    },
    #[serde(rename = "captions.inspect")]
    CaptionsInspect {
        /// Caption document as returned by captions.import or captions.apply.
        document: crate::captions::Document,
    },
    #[serde(rename = "captions.apply")]
    CaptionsApply {
        /// Caption document as returned by captions.import or captions.apply.
        document: crate::captions::Document,
        /// Must equal the supplied document's revision.
        expected_revision: u64,
        /// Caption edits applied in order as one atomic batch.
        operations: Vec<crate::captions::Operation>,
    },
    #[serde(rename = "captions.encode")]
    CaptionsEncode {
        /// Caption document as returned by captions.import or captions.apply.
        document: crate::captions::Document,
        /// Sidecar format to preview.
        format: crate::captions::Format,
    },
    #[serde(rename = "captions.export")]
    CaptionsExport {
        /// Caption document as returned by captions.import or captions.apply.
        document: crate::captions::Document,
        /// Sidecar format to write.
        format: crate::captions::Format,
        /// reject fails on any reported formatting loss; allow_reported accepts losses already reviewed with captions.encode.
        loss_policy: crate::captions::LossPolicy,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .srt or .vtt file (matching format) inside output_root; never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "transcript.assemble")]
    TranscriptAssemble {
        /// Sequential project to append to; its assets must include the transcripts' sources.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Transcripts of the source media, as for timeline.outline.
        transcripts: Vec<crate::transcript::Document>,
        /// Word runs to use, in timeline order, 1-500.
        selections: Vec<crate::assemble::Selection>,
        /// Existing absolute directory; used only to match absolute asset paths to transcript source paths.
        #[serde(default)]
        input_root: Option<PathBuf>,
        /// Extra source kept before the first and after the last word of each run, at most 2 s; default zero.
        #[serde(default)]
        padding: Option<Time>,
        /// Prefix for new clip IDs `<prefix><n>`; default `s`.
        #[serde(default)]
        clip_prefix: Option<String>,
    },
    #[serde(rename = "transcript.fillers")]
    TranscriptFillers {
        /// Project to clean up.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Transcripts of the project's source media, as for timeline.outline.
        transcripts: Vec<crate::transcript::Document>,
        /// Existing absolute directory; used only to match absolute asset paths to transcript source paths.
        #[serde(default)]
        input_root: Option<PathBuf>,
        /// Audio tracks whose speech to search; default every enabled audio track. Not for sequential timelines.
        #[serde(default)]
        track_ids: Option<Vec<String>>,
        /// Words to remove, 1-64, matched on letters and digits ignoring case and punctuation; default um, uh, erm, er, ah, uhm, umm, hmm and mm.
        #[serde(default)]
        words: Option<Vec<String>>,
        /// Extra time cut on each side of a filler, never into a neighbouring word, at most 1/4 s; default zero.
        #[serde(default)]
        padding: Option<Time>,
        /// Only fillers after this timeline time; default the start.
        #[serde(default)]
        start: Option<Time>,
        /// Only fillers before this timeline time; default the end.
        #[serde(default)]
        end: Option<Time>,
        /// Silence the fillers on track_ids, moving nothing, instead of ripple-deleting them from every track (which also cuts music and picture).
        #[serde(default)]
        lift: bool,
    },
    #[serde(rename = "captions.draft")]
    CaptionsDraft {
        /// Project whose speech to caption.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Transcripts of the project's source media, as for timeline.outline; whole words inside audible audio clips become caption text at their timeline times.
        transcripts: Vec<crate::transcript::Document>,
        /// Existing absolute directory; used only to match absolute asset paths to transcript source paths.
        #[serde(default)]
        input_root: Option<PathBuf>,
        /// Audio tracks whose speech to caption; default every enabled audio track. Not for sequential timelines.
        #[serde(default)]
        track_ids: Option<Vec<String>>,
        /// Caption words that start at or after this timeline time; default zero.
        #[serde(default)]
        start: Option<Time>,
        /// Caption words that start before this timeline time; default the timeline end.
        #[serde(default)]
        end: Option<Time>,
        /// Caption document ID: 1-64 ASCII letters, digits, `_` or `-`; default `captions`.
        #[serde(default)]
        id: Option<String>,
        /// Text color `[r, g, b]` of the document's one style, `default`; default white.
        #[serde(default)]
        color: Option<[u8; 3]>,
        /// Line alignment of every cue; default `center`.
        #[serde(default)]
        align: Option<crate::graphics::Align>,
        /// Longest line in characters (Unicode scalars), 10-80; default 42. A longer single word gets its own line.
        #[serde(default)]
        line_chars: Option<usize>,
        /// Lines per cue, 1-3; default 2.
        #[serde(default)]
        lines: Option<usize>,
        /// Longest cue from its first word's start to its last word's end, 1-10 s; default 6.
        #[serde(default)]
        max_duration: Option<Time>,
        /// Shortest display time, 0-5 s; default 1. A cue is held this long unless the next one starts first.
        #[serde(default)]
        min_duration: Option<Time>,
        /// Silence between words that starts a new cue, 0.1-5 s; default 1/2.
        #[serde(default)]
        pause: Option<Time>,
    },
    #[serde(rename = "captions.render")]
    CaptionsRender {
        /// Caption document to render, as from captions.draft, captions.import or captions.apply.
        document: crate::captions::Document,
        /// Layout per style ID, as for captions.scene; every style with a visible cue needs one.
        layouts: std::collections::BTreeMap<String, crate::captions::Layout>,
        /// Project whose canvas, frame rate and timeline the overlay matches.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory containing the layout fonts.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv overlay inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// Timeline time the overlay starts at, on a frame boundary; default zero. Captions at that time appear on its first frame.
        #[serde(default)]
        start: Option<Time>,
        /// Length to cover, whole frames; default to the timeline end. The asset may run a few frames longer to keep windows sample- and millisecond-exact.
        #[serde(default)]
        duration: Option<Time>,
        /// ID of the returned asset; default the document ID.
        #[serde(default)]
        asset_id: Option<String>,
    },
    #[serde(rename = "captions.scene")]
    CaptionsScene(crate::captions::SceneRequest),
    #[serde(rename = "graphics.instantiate")]
    GraphicsInstantiate {
        /// Template to expand: a scene with typed, bound parameters.
        template: crate::templates::Template,
        /// ID for the returned scene instance.
        instance_id: String,
        /// Values keyed by parameter ID; a parameter without a value uses its declared default.
        values: std::collections::BTreeMap<String, crate::templates::ParameterValue>,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "proxy.generate")]
    ProxyGenerate(proxy::Generate),
    #[serde(rename = "proxy.status")]
    ProxyStatus {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "proxy.relink")]
    ProxyRelink {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Asset whose proxy to find.
        asset_id: String,
        /// Absolute candidate proxy paths inside input_root; exactly one must match the bound identity.
        candidates: Vec<PathBuf>,
    },
    #[serde(rename = "media.conform.inspect")]
    ConformInspect {
        /// Conversion recipe: bound source, speed/reverse/freeze or remap, audio policy and optional color normalization.
        recipe: conform::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "media.conform")]
    Conform {
        /// Conversion recipe, normally checked first with media.conform.inspect.
        recipe: conform::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "audio.inspect")]
    AudioInspect {
        /// PCM mix recipe: tracks, clips, gain and fades, optional routing and master effects.
        mix: audio::Mix,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "audio.render")]
    AudioRender {
        /// PCM mix recipe, normally checked first with audio.inspect.
        mix: audio::Mix,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .wav file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "registry.search")]
    RegistrySearch {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Text, bin prefix, tag filters and pagination.
        query: registry::Query,
    },
    #[serde(rename = "registry.status")]
    RegistryStatus {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "registry.bind")]
    RegistryBind {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// 1-1000 unique asset IDs to hash and bind.
        asset_ids: Vec<String>,
    },
    #[serde(rename = "registry.relink")]
    RegistryRelink {
        /// Project to read.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Bound asset to relink.
        asset_id: String,
        /// Absolute candidate paths inside input_root; exactly one must match the bound identity.
        candidates: Vec<PathBuf>,
    },
    #[serde(rename = "image.sequence.inspect")]
    ImageSequenceInspect {
        /// Numbered PNG recipe: frames, source window, repeat, rate, alpha and color interpretation.
        recipe: crate::image_sequence::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "image.sequence.compile")]
    ImageSequenceCompile {
        /// Numbered PNG recipe, normally checked first with image.sequence.inspect.
        recipe: crate::image_sequence::Recipe,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new file inside output_root: .mov for the rgba_png_mov profile, otherwise .mkv; never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "expression.inspect")]
    ExpressionInspect {
        #[serde(flatten)]
        request: crate::expressions::Inspect,
    },
    #[serde(rename = "scene.inspect")]
    SceneInspect {
        /// Scene to validate and sample.
        scene: scene::Scene,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "scene.render")]
    SceneRender {
        /// Scene to compile, normally checked first with scene.inspect.
        scene: scene::Scene,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "scene.still")]
    SceneStill {
        /// Scene to show, as for scene.inspect and scene.render.
        scene: scene::Scene,
        /// Existing absolute directory containing the scene's media and fonts.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .png file inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// Scene time to show, in rational seconds; the frame on screen then. Default zero.
        #[serde(default)]
        time: Option<Time>,
    },
    #[serde(rename = "preview.frame")]
    PreviewFrame {
        /// Project to preview; its saved proxy selection applies.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .png file inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// Timeline time in rational seconds; must fall on a frame boundary of the project rate.
        time: Time,
    },
    #[serde(rename = "preview.range")]
    PreviewRange {
        /// Project to preview; its saved proxy selection applies.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
        /// Range start in rational seconds, on a frame boundary.
        start: Time,
        /// Positive range length in rational seconds, on frame boundaries; the range must end inside the timeline.
        duration: Time,
    },
    #[serde(rename = "capabilities")]
    Capabilities {
        /// One area to describe in full, such as `renderer`, `scenes` or `export`, or `all` for everything. Omit for a short summary that lists the areas.
        #[serde(default)]
        section: Option<String>,
    },
    #[serde(rename = "schema")]
    Schema {
        /// A command such as `scene.render` or its MCP tool name, or a shared type: `project`, `operation`, `scene`, `template`, `audio_routing` or `transcript`.
        name: String,
        /// One part to return: a variant tag such as `clip.append`, or a definition name such as `Layer`. Omit for the whole schema, or an outline of it when it is large.
        #[serde(default)]
        select: Option<String>,
        /// Return the whole schema even when it is large; it is otherwise outlined.
        #[serde(default)]
        full: bool,
    },
    #[serde(rename = "project.create")]
    Create {
        /// Project ID, 1-128 bytes.
        id: String,
        /// Frame width in pixels, 1-8192.
        width: u32,
        /// Frame height in pixels, 1-8192.
        height: u32,
        /// Frame rate as a rational, for example {num:25,den:1} or {num:30000,den:1001}.
        frame_rate: Time,
    },
    #[serde(rename = "project.validate")]
    Validate {
        /// Project to check.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
    },
    #[serde(rename = "timeline.apply")]
    Apply {
        /// Project to transform; neither the input nor a saved revision is modified.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal the supplied project's revision.
        expected_revision: u64,
        /// Editing operations applied in order as one atomic batch; any failure commits nothing.
        operations: Vec<Operation>,
    },
    #[serde(rename = "session.create")]
    SessionCreate {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// Valid snapshot saved as revision 0 of a new project; its own revision is ignored. Omit it to start an empty project from `id`, `width`, `height` and `frame_rate`.
        #[serde(default)]
        project: Option<Project>,
        /// New empty project ID, 1-128 bytes; use with width, height and frame_rate instead of `project`.
        #[serde(default)]
        id: Option<String>,
        /// New empty project frame width in pixels, 1-8192.
        #[serde(default)]
        width: Option<u32>,
        /// New empty project frame height in pixels, 1-8192.
        #[serde(default)]
        height: Option<u32>,
        /// New empty project frame rate, for example 25 or "30000/1001".
        #[serde(default)]
        frame_rate: Option<Time>,
        /// Caller-chosen ID for this change, 1-128 bytes, unique within the project. Resend identical arguments with it to retry safely.
        request_id: String,
    },
    #[serde(rename = "session.get")]
    SessionGet {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Saved revision to read; omit for the current head.
        revision: Option<u64>,
    },
    #[serde(rename = "session.apply")]
    SessionApply {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Caller-chosen ID for this change, 1-128 bytes, unique within the project. Resend identical arguments with it to retry safely.
        request_id: String,
        /// Head revision this change was prepared against; a different head fails with REVISION_CONFLICT.
        expected_revision: u64,
        /// Editing operations applied in order as one atomic batch; any failure commits nothing.
        operations: Vec<Operation>,
    },
    #[serde(rename = "session.undo")]
    SessionUndo {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Caller-chosen ID for this change, 1-128 bytes, unique within the project. Resend identical arguments with it to retry safely.
        request_id: String,
        /// Head revision this change was prepared against; a different head fails with REVISION_CONFLICT.
        expected_revision: u64,
    },
    #[serde(rename = "session.restore")]
    SessionRestore {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Caller-chosen ID for this change, 1-128 bytes, unique within the project. Resend identical arguments with it to retry safely.
        request_id: String,
        /// Head revision this change was prepared against; a different head fails with REVISION_CONFLICT.
        expected_revision: u64,
        /// Saved revision whose state becomes the new head.
        target_revision: u64,
    },
    #[serde(rename = "session.preview")]
    SessionPreview {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Head revision this change was prepared against; a different head fails with REVISION_CONFLICT.
        expected_revision: u64,
        /// Editing operations applied in order as one atomic batch; any failure commits nothing.
        operations: Vec<Operation>,
    },
    /// Look up the committed receipt for a request ID; REQUEST_NOT_FOUND when it never took effect.
    #[serde(rename = "session.receipt")]
    SessionReceipt {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Request ID of the change to look up.
        request_id: String,
    },
    #[serde(rename = "session.history")]
    SessionHistory {
        /// Existing absolute local directory holding the session database (projects.sqlite3).
        store_root: PathBuf,
        /// ID of a project saved in store_root.
        project_id: String,
        /// Return entries older than this revision; pass the previous page's next_before_revision. Omit for the newest page.
        before_revision: Option<u64>,
        /// Maximum entries to return, 1-200; default 50.
        #[serde(default = "history_limit")]
        limit: u16,
    },
    /// Queue a render and return a durable ticket immediately. Reuse request_id only with identical arguments. Poll job.status; use job.cancel to stop it.
    #[serde(rename = "render.start")]
    Start {
        /// Existing absolute local directory holding the job queue (jobs.sqlite3).
        job_root: PathBuf,
        /// Caller-chosen ID unique within job_root, 1-128 bytes. An identical resubmission returns the original ticket.
        request_id: String,
        /// Render to queue: project snapshot, roots, output path and optional retry policy.
        render: jobs::RenderRequest,
    },
    /// Return saved job state, rendering frame progress, result receipt or failure. Reconcile workers that have exited.
    #[serde(rename = "job.status")]
    JobStatus {
        /// Job queue directory used with render.start.
        job_root: PathBuf,
        /// Job ID from the render.start ticket.
        job_id: String,
    },
    /// Cancel a queued/running job. Poll status until terminal; completion may win a simultaneous cancel.
    #[serde(rename = "job.cancel")]
    JobCancel {
        /// Job queue directory used with render.start.
        job_root: PathBuf,
        /// Job ID from the render.start ticket.
        job_id: String,
    },
    /// Reconcile saved publication and wake queued work, including explicitly opted-in interrupted retries within the saved attempt limit.
    #[serde(rename = "job.resume")]
    JobResume {
        /// Job queue directory used with render.start.
        job_root: PathBuf,
    },
    #[serde(rename = "job.start")]
    JobStart {
        /// Existing absolute local directory holding the job queue (jobs.sqlite3).
        job_root: PathBuf,
        /// Caller-chosen ID unique within job_root, 1-128 bytes. An identical resubmission returns the original ticket.
        request_id: String,
        /// Command to run in the background: export.run, export.review, captions.render, media.prepare, media.transcribe, media.conform, scene.render, audio.render, audio.repair.render, hdr.conform, image.sequence.compile, proxy.generate, preview.range, cache.run or transcript.transcribe.
        run: String,
        /// That command's arguments exactly as for a direct call, without `command`; cutbolt_schema with its name gives the schema.
        arguments: serde_json::Map<String, Value>,
    },
    #[serde(rename = "job.wait")]
    JobWait {
        /// Job queue directory used with render.start or job.start.
        job_root: PathBuf,
        /// Job ID from the ticket.
        job_id: String,
        /// Longest wait in seconds, 1 to 120; default 30. The status is returned either way, with `finished`.
        #[serde(default)]
        timeout_seconds: Option<u32>,
    },
    #[serde(rename = "timeline.meters")]
    TimelineMeters {
        /// Project to measure; its enabled audio is rendered exactly as an audio-only export would be.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Range start in rational seconds on a frame boundary; default zero.
        #[serde(default)]
        start: Option<Time>,
        /// Range length in rational seconds, at most 600; default to the timeline end.
        #[serde(default)]
        duration: Option<Time>,
        /// Also meter each enabled audio track alone; default true. Each track costs one more audio pass.
        #[serde(default = "crate::commands::yes")]
        tracks: bool,
        /// Add `over_time`: per-second short-term and loudest momentary LKFS, silent runs (-60 dBFS for 0.5 s or more) and clipped runs, with exact times; default false.
        #[serde(default)]
        curve: bool,
    },
    #[serde(rename = "timeline.check")]
    TimelineCheck {
        /// Project to check.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory that project media paths resolve against; with it, missing and changed media are reported.
        #[serde(default)]
        input_root: Option<PathBuf>,
        /// Transcripts of the source media, as for timeline.outline; with them, words cut by clip edges are reported.
        #[serde(default)]
        transcripts: Vec<crate::transcript::Document>,
        /// Shorter picture clips are flash frames, 1-100 frames; default 3.
        #[serde(default)]
        min_clip_frames: Option<u64>,
        /// A cut between touching clips of one source that skips or repeats less than this is a jump cut; default 10 s.
        #[serde(default)]
        jump_window: Option<Time>,
    },
    #[serde(rename = "timeline.outline")]
    TimelineOutline {
        /// Project to outline.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Transcripts of source media, as returned by transcript.transcribe or transcript.correct, at most 256. Each is matched to the assets of its source (by content identity when the asset is bound, otherwise by path), and audio clips then show what is said in them.
        #[serde(default)]
        transcripts: Vec<crate::transcript::Document>,
        /// Existing absolute directory; used only to match absolute asset paths to transcript source paths.
        #[serde(default)]
        input_root: Option<PathBuf>,
        /// Outline this child sequence instead of the project's timeline.
        #[serde(default)]
        sequence_id: Option<String>,
        /// List clips that end after this timeline time; default zero.
        #[serde(default)]
        start: Option<Time>,
        /// List clips that start before this timeline time; default the timeline end.
        #[serde(default)]
        end: Option<Time>,
        /// Transcript words shown per clip, 0-2048; default 12. Longer speech shows its first and last words and the word count; 0 shows only the count.
        #[serde(default)]
        words: Option<usize>,
    },
    #[serde(rename = "files.list")]
    FilesList {
        /// Existing absolute directory to list; with a workspace it defaults to the workspace.
        input_root: PathBuf,
        /// Relative folder inside input_root to list; omit for input_root itself.
        #[serde(default)]
        dir: Option<String>,
        /// Also list files in every subfolder; default false.
        #[serde(default)]
        recursive: bool,
        /// Only files with these extensions, such as `["mkv", "mp4", "wav"]`; folders are then omitted.
        #[serde(default)]
        extensions: Vec<String>,
        /// Most entries to return, 1-1000; default 200. `total` and `truncated` report the rest.
        #[serde(default)]
        limit: Option<usize>,
    },
    #[serde(rename = "media.inspect")]
    Inspect {
        /// Absolute path of the file to probe; it must lie inside input_root.
        path: PathBuf,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
    },
    #[serde(rename = "render.plan")]
    Plan {
        /// Project to plan; give a saved `revision` for a stable render.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
    #[serde(rename = "render.run")]
    Render {
        /// Project to render; give a saved `revision` for a stable render.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Existing absolute directory; every source file must lie inside it.
        input_root: PathBuf,
        /// Existing absolute directory; the output must lie inside it.
        output_root: PathBuf,
        /// Absolute path of a new .mkv file inside output_root; existing files are never overwritten.
        output: PathBuf,
    },
}

fn history_limit() -> u16 {
    50
}

/// Handle one JSON request as the CLI and MCP adapter receive it. Saved-project references are
/// loaded first; with a workspace, omitted roots default inside it, relative paths resolve
/// against it, and engine-produced paths in the result are reported relative to it.
pub fn handle_json(
    mut request: Value,
    workspace: Option<&crate::workspace::Workspace>,
) -> Result<Value> {
    let save_as = match request.as_object_mut().and_then(|o| o.remove("save_as")) {
        None => None,
        Some(Value::String(file)) if workspace.is_some() => Some(file),
        Some(Value::String(_)) => {
            return Err(crate::error("INVALID_PATH", "save_as needs a workspace"));
        }
        Some(_) => {
            return Err(crate::error(
                "INVALID_JSON",
                "save_as: expected a .json path",
            ));
        }
    };
    crate::documents::load(&mut request, workspace)?;
    let mut completed = prepare(&mut request, workspace)?;
    // A queued command's arguments get the same preparation now, in the caller's workspace.
    if request["command"] == "job.start"
        && let Some(run) = request["run"].as_str()
        && !jobs::QUEUED_COMMANDS.contains(&run)
    {
        return Err(crate::error(
            "UNSUPPORTED_JOB",
            format!(
                "{run:?} cannot be queued; job.start runs {}. Use render.start for reference renders",
                jobs::QUEUED_COMMANDS.join(", ")
            ),
        ));
    }
    if request["command"] == "job.start"
        && let Some(Value::Object(arguments)) = request.get_mut("arguments")
    {
        let mut queued = Value::Object(std::mem::take(arguments));
        queued["command"] = request["run"].clone();
        completed.extend(prepare(&mut queued, workspace)?);
        parse(queued.clone())
            .map_err(|e| crate::error(e.code, format!("arguments: {}", e.message)))?;
        queued.as_object_mut().unwrap().remove("command");
        request["arguments"] = queued;
    }
    let request = parse(request)?;
    let schema = matches!(request, Request::Schema { .. });
    let capabilities = matches!(&request, Request::Capabilities { section } if section.as_deref().is_none_or(|s| s == "all"));
    let mut result = handle(request)?;
    if !completed.is_empty() && result.is_object() {
        result["resolved_identities"] = Value::Array(completed);
    }
    if let Some(workspace) = workspace {
        if schema {
            crate::schema::relax_roots(&mut result["schema"]);
            crate::schema::relax_roots(&mut result["outline"]);
            if let Some(text) = result["description"].as_str() {
                result["description"] = Value::String(crate::schema::workspace_wording(text));
            }
        }
        workspace.present(&mut result);
        if capabilities {
            result["workspace"] = workspace.describe();
        }
    } else if capabilities {
        result["workspace"] = Value::Null;
    }
    if let (Some(file), Some(workspace)) = (save_as, workspace) {
        result = crate::documents::save(&result, &file, workspace)?;
    }
    Ok(result)
}

/// Workspace defaults, saved-project references and path-only identities, resolved in place.
/// Returns the identities that were completed.
fn prepare(
    request: &mut Value,
    workspace: Option<&crate::workspace::Workspace>,
) -> Result<Vec<Value>> {
    if let Some(workspace) = workspace {
        workspace.prepare(request)?;
    }
    let store = workspace.map(crate::workspace::Workspace::store_root);
    crate::reference::resolve(request, store.as_deref())?;
    crate::identity::complete(request)
}

/// Parse a request, naming the first field its schema rejects when it fails.
fn parse(request: Value) -> Result<Request> {
    let located = crate::schema::locate(&request);
    let original = located.as_ref().map(|_| request.clone());
    serde_json::from_value(request).map_err(|e| {
        // Tagged requests and operations lose serde's position, so name the field the schema
        // rejects, such as operations[2].clip.duration, and what was given there when serde's
        // wording would mislead.
        match (located, original) {
            (Some(field), Some(original)) if !field.is_empty() => {
                let message = e.to_string();
                let note = crate::schema::given(&original, &field, &message).unwrap_or_default();
                crate::error("INVALID_JSON", format!("{field}: {message}{note}"))
            }
            _ => crate::Error::from(e),
        }
    })
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
        Request::PreviewCuts {
            project,
            input_root,
            output_root,
            output,
            start,
            limit,
            tile_width,
        } => crate::review::cut_sheet(
            &project,
            &input_root,
            &output_root,
            &output,
            start,
            limit,
            tile_width,
        ),
        Request::AudioDuck {
            project,
            input_root,
            voice_track_id,
            music_track_id,
            threshold_db,
            duck_milli,
            attack,
            release,
            bridge,
        } => crate::duck::propose(
            &project,
            &input_root,
            &voice_track_id,
            &music_track_id,
            &crate::duck::Settings {
                threshold_db: threshold_db.unwrap_or(-45),
                duck_milli: duck_milli.unwrap_or(250),
                attack: attack.unwrap_or(Time { num: 1, den: 4 }),
                release: release.unwrap_or(Time { num: 3, den: 5 }),
                bridge: bridge.unwrap_or(Time { num: 1, den: 1 }),
            },
        ),
        Request::ExportReview {
            path,
            input_root,
            output_root,
            output,
            project,
            transcripts,
            heard,
            runtime,
            language,
            frames,
            rendition_height,
            tolerance,
        } => {
            let recognize = match (runtime, language) {
                (Some(runtime), Some(language)) => {
                    Some(crate::cut_review::Recognize { runtime, language })
                }
                (None, None) => None,
                _ => {
                    return Err(crate::error(
                        "INVALID_ARGUMENT",
                        "runtime and language go together",
                    ));
                }
            };
            crate::cut_review::review(&crate::cut_review::Request {
                path: &path,
                input_root: &input_root,
                output_root: &output_root,
                output: &output,
                project: project.as_ref(),
                transcripts: &transcripts,
                heard: &heard,
                recognize: recognize.as_ref(),
                frames: frames.unwrap_or(16),
                rendition_height: rendition_height.unwrap_or(360),
                tolerance: tolerance.unwrap_or(Time::new(1, 2)?),
            })
        }
        Request::MediaTranscribe {
            path,
            input_root,
            output_root,
            output,
            runtime,
            language,
            channel,
            id,
            start,
            duration,
            timeout_seconds,
            text,
        } => crate::media_transcribe::run(&crate::media_transcribe::Request {
            path: &path,
            input_root: &input_root,
            output_root: &output_root,
            output: &output,
            runtime: &runtime,
            language,
            channel: channel.unwrap_or(crate::transcribe::Channel::Mean),
            id: id.as_deref().unwrap_or("transcript"),
            start,
            duration,
            timeout_seconds: timeout_seconds.unwrap_or(600),
            text: text.as_deref(),
        }),
        Request::MediaPrepare {
            path,
            paths,
            input_root,
            output_root,
            output,
            project,
        } => match (path, paths, output) {
            (Some(path), None, output) => crate::readiness::prepare(
                &path,
                &input_root,
                &output_root,
                output.as_deref(),
                project.as_ref(),
            ),
            (None, Some(paths), None) => {
                crate::readiness::prepare_many(&paths, &input_root, &output_root, project.as_ref())
            }
            _ => Err(crate::error(
                "INVALID_ARGUMENT",
                "Give path (with an optional output) or paths, not both",
            )),
        },
        Request::MediaSheet {
            path,
            input_root,
            output_root,
            output,
            times,
            count,
            columns,
            tile_width,
        } => crate::review::source_sheet(
            &path,
            &input_root,
            &output_root,
            &output,
            times,
            count,
            columns,
            tile_width,
        ),
        Request::MediaShots {
            path,
            input_root,
            threshold,
            minimum_frames,
            output_root,
            output,
            tile_width,
        } => crate::review::shots(
            &path,
            &input_root,
            threshold,
            minimum_frames,
            output_root.as_deref(),
            output.as_deref(),
            tile_width,
        ),
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
        Request::Capabilities { section } => capabilities(section.as_deref()),
        Request::Schema { name, select, full } => {
            crate::schema::lookup(&name, select.as_deref(), full)
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
            id,
            width,
            height,
            frame_rate,
            request_id,
        } => {
            let project = match (project, id, width, height, frame_rate) {
                (Some(project), None, None, None, None) => project,
                (None, Some(id), Some(width), Some(height), Some(frame_rate)) => {
                    Project::new(id, width, height, frame_rate)?
                }
                _ => {
                    return Err(crate::error(
                        "INVALID_ARGUMENT",
                        "Give either project, or all of id, width, height and frame_rate",
                    ));
                }
            };
            Ok(serde_json::to_value(store::create(
                &store_root,
                project,
                &request_id,
            )?)?)
        }
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
        Request::SessionReceipt {
            store_root,
            project_id,
            request_id,
        } => Ok(serde_json::to_value(store::receipt(
            &store_root,
            &project_id,
            &request_id,
        )?)?),
        Request::Start {
            job_root,
            request_id,
            render,
        } => jobs::start(&job_root, &request_id, render),
        Request::JobStatus { job_root, job_id } => jobs::status(&job_root, &job_id),
        Request::JobCancel { job_root, job_id } => jobs::cancel(&job_root, &job_id),
        Request::JobResume { job_root } => jobs::resume(&job_root),
        Request::JobStart {
            job_root,
            request_id,
            run,
            arguments,
        } => {
            let mut request = Value::Object(arguments);
            request["command"] = Value::String(run);
            // Validate the queued command now, so a bad argument fails before it waits in line.
            serde_json::from_value::<Request>(request.clone())?;
            jobs::start_command(&job_root, &request_id, request)
        }
        Request::JobWait {
            job_root,
            job_id,
            timeout_seconds,
        } => jobs::wait(&job_root, &job_id, timeout_seconds.unwrap_or(30)),
        Request::TimelineMeters {
            project,
            input_root,
            start,
            duration,
            tracks,
            curve,
        } => crate::meters::inspect(&project, &input_root, start, duration, tracks, curve),
        Request::TimelineCheck {
            project,
            input_root,
            transcripts,
            min_clip_frames,
            jump_window,
        } => {
            let min_clip_frames = min_clip_frames.unwrap_or(3);
            if !(1..=100).contains(&min_clip_frames) {
                return Err(crate::error(
                    "INVALID_ARGUMENT",
                    "min_clip_frames must be 1-100",
                ));
            }
            crate::check::check(&crate::check::Request {
                project: &project,
                input_root: input_root.as_deref(),
                transcripts: &transcripts,
                min_clip_frames,
                jump_window: jump_window.unwrap_or(Time::new(10, 1)?),
            })
        }
        Request::TimelineOutline {
            project,
            transcripts,
            input_root,
            sequence_id,
            start,
            end,
            words,
        } => crate::outline::outline(&crate::outline::Request {
            project: &project,
            transcripts: &transcripts,
            input_root: input_root.as_deref(),
            sequence_id: sequence_id.as_deref(),
            start,
            end,
            words: words.unwrap_or(crate::outline::DEFAULT_WORDS),
        }),
        Request::AudioNormalize {
            project,
            input_root,
            target_lkfs,
            peak_ceiling_dbfs,
        } => crate::normalize::propose(
            &project,
            &input_root,
            target_lkfs.unwrap_or(-14.0),
            peak_ceiling_dbfs.unwrap_or(-1.0),
        ),
        Request::CaptionsDraft {
            project,
            transcripts,
            input_root,
            track_ids,
            start,
            end,
            id,
            color,
            align,
            line_chars,
            lines,
            max_duration,
            min_duration,
            pause,
        } => crate::transcript_captions::draft(&crate::transcript_captions::Request {
            project: &project,
            transcripts: &transcripts,
            input_root: input_root.as_deref(),
            track_ids: track_ids.as_deref(),
            start,
            end,
            id: id.as_deref().unwrap_or("captions"),
            color: color.unwrap_or([255, 255, 255]),
            align: align.unwrap_or(crate::graphics::Align::Center),
            rules: crate::transcript_captions::Rules {
                line_chars: line_chars.unwrap_or(42),
                lines: lines.unwrap_or(2),
                max_duration: max_duration.unwrap_or(Time::new(6, 1)?),
                min_duration: min_duration.unwrap_or(Time::new(1, 1)?),
                pause: pause.unwrap_or(Time::new(1, 2)?),
            },
        }),
        Request::CaptionsRender {
            document,
            layouts,
            project,
            input_root,
            output_root,
            output,
            start,
            duration,
            asset_id,
        } => crate::caption_overlay::render(&crate::caption_overlay::Request {
            document: &document,
            layouts: &layouts,
            project: &project,
            input_root: &input_root,
            output_root: &output_root,
            output: &output,
            start,
            duration,
            asset_id: asset_id.as_deref(),
        }),
        Request::AudioTighten {
            project,
            input_root,
            voice_track_id,
            threshold_db,
            min_pause,
            keep,
            edges,
            start,
            end,
        } => crate::tighten::propose(
            &project,
            &input_root,
            voice_track_id.as_deref(),
            &crate::tighten::Settings {
                threshold_db: threshold_db.unwrap_or(-45),
                min_pause: min_pause.unwrap_or(Time::new(3, 4)?),
                keep: keep.unwrap_or(Time::new(1, 5)?),
                edges,
                window: (start, end),
            },
        ),
        Request::AudioBeats {
            path,
            input_root,
            start,
            duration,
            min_bpm,
            max_bpm,
            frame_rate,
        } => crate::beats::analyse(&crate::beats::Request {
            path: &path,
            input_root: &input_root,
            start,
            duration,
            min_bpm: min_bpm.unwrap_or(60),
            max_bpm: max_bpm.unwrap_or(200),
            frame_rate,
        }),
        Request::SceneStill {
            scene,
            input_root,
            output_root,
            output,
            time,
        } => crate::scene::still(
            &scene,
            &input_root,
            &output_root,
            &output,
            time.unwrap_or(Time::ZERO),
        ),
        Request::TranscriptFillers {
            project,
            transcripts,
            input_root,
            track_ids,
            words,
            padding,
            start,
            end,
            lift,
        } => {
            let words =
                words.unwrap_or_else(|| crate::fillers::DEFAULT_WORDS.map(str::to_owned).to_vec());
            crate::fillers::propose(&crate::fillers::Request {
                project: &project,
                transcripts: &transcripts,
                input_root: input_root.as_deref(),
                track_ids: track_ids.as_deref(),
                words: &words,
                padding: padding.unwrap_or(Time::ZERO),
                window: (start, end),
                lift,
            })
        }
        Request::TranscriptAssemble {
            project,
            transcripts,
            selections,
            input_root,
            padding,
            clip_prefix,
        } => crate::assemble::propose(&crate::assemble::Request {
            project: &project,
            transcripts: &transcripts,
            selections: &selections,
            input_root: input_root.as_deref(),
            padding: padding.unwrap_or(Time::ZERO),
            clip_prefix: clip_prefix.as_deref().unwrap_or("s"),
        }),
        Request::ColorMatch {
            reference,
            target,
            reference_times,
            target_times,
            input_root,
            output_root,
            output,
            method,
            transfer,
        } => crate::color_match::propose(&crate::color_match::Request {
            reference: &reference,
            target: &target,
            reference_times,
            target_times,
            input_root: &input_root,
            output_root: &output_root,
            output: &output,
            method: method.unwrap_or(crate::color_match::Method::Levels),
            transfer: transfer.unwrap_or(crate::color::Transfer::Bt709),
        }),
        Request::FilesList {
            input_root,
            dir,
            recursive,
            extensions,
            limit,
        } => crate::files::list(&input_root, dir.as_deref(), recursive, &extensions, limit),
        Request::Inspect { path, input_root } => {
            let path = media::allowed_file(&path, &input_root)?;
            let identity = crate::identity::relative(&path, &input_root)?;
            let metadata = media::probe(&path)?;
            let relative = identity["path"].as_str().expect("identity path");
            let timeline = crate::readiness::timeline(&path, relative, &metadata);
            Ok(json!({"path":path,"identity":identity,"timeline":timeline,"metadata":metadata}))
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

pub(crate) fn yes() -> bool {
    true
}

/// Everything the engine can do and every limit, by area.
fn all_capabilities() -> Value {
    let mut result = json!({"version":env!("CARGO_PKG_VERSION"),"license":"MIT","local_only":true,"reframing":crate::reframe::capabilities(),
    "interchange":crate::interchange::capabilities(),
    "project_store":{"schema_version":2,"read_versions":[1,2],"migration":"explicit_transactional","backup_maximum_bytes":268435456,"relative_media":true},
    "commands":["expression.inspect","native.import","image.sequence.inspect","image.sequence.compile","project.portable","session.check","session.migrate","session.backup","session.recover","interchange.import","interchange.export.inspect","interchange.export","cache.run","cache.inspect","cache.prune","preview.sheet","transcript.transcribe","transcript.inspect","transcript.correct","transcript.plan","transcript.fillers","transcript.assemble","audio.inputs","audio.record.inspect","audio.record","audio.record.place","audio.repair.inspect","audio.repair.render","stabilization.inspect","reframe.inspect","tracking.inspect","sync.inspect","hdr.inspect","hdr.conform","lut.inspect","scopes.inspect","export.inspect","export.run","export.review","effects.preset","captions.import","captions.inspect","captions.apply","captions.encode","captions.export","captions.draft","captions.render","captions.scene","graphics.instantiate","proxy.generate","proxy.status","proxy.relink","media.conform.inspect","media.conform","audio.inspect","audio.render","registry.search","registry.status","registry.bind","registry.relink","scene.inspect","scene.still","scene.render","preview.frame","preview.range","capabilities","schema","project.create","project.validate","timeline.apply","session.create","session.get","session.apply","session.undo","session.restore","session.preview","session.history","session.receipt","files.list","timeline.meters","timeline.outline","timeline.check","preview.cuts","media.sheet","media.shots","media.prepare","media.transcribe","color.match","audio.beats","audio.duck","audio.normalize","audio.tighten","media.inspect","render.plan","render.run","render.start","job.status","job.cancel","job.resume","job.start","job.wait"],
    "operations":["media.paths","transcript.cut","multicam.create","multicam.edit","sequence.create","sequence.edit","sequence.remove","tracks.edit","media.proxy.attach","media.proxy.detach","media.proxy.relink","preview.proxy","project.transfer","clip.insert","clip.overwrite","timeline.ripple_delete","clip.slip","clip.roll","clip.slide","media.metadata","media.bind","media.relink","media.add","clip.append","clip.split","clip.trim","clip.move","clip.remove"],
    "state":"immutable snapshots plus local transactional sessions with durable request IDs, revision conflicts and undo/history",
    "mcp":{"transport":"stdio","protocol_versions":["2025-11-25","2025-06-18"]},"jobs":{"platform":"windows","available":cfg!(windows),"maximum_active_per_root":32,"concurrent_renders_per_root":1,"default_attempts":1,"maximum_attempts":3,"retry_errors":["TOOL_FAILED","TOOL_TIMEOUT","WORKER_INTERRUPTED"],"source_pinning":"first_validated_plan","tool_content_pinning":true,"publication_recovery":"validated_receipt_and_output_hash","queue_schema_version":2},
    "transcripts":crate::transcript::capabilities(),
    "audio_repair":crate::audio_repair::capabilities(),
    "audio_recording":{"platform":"windows","sample_rate":48000,"channels":2,"sample_format":"s16le","maximum_seconds":7200,"input_selection":"explicit_endpoint_or_process","recording":"blocking_cli_library","placement":"existing_native_audio_tracks","latency_compensation":"explicit_exact_shift","source_preservation":true},
    "stabilization":crate::stabilize::capabilities(),
    "native_projects":crate::native_project::capabilities(),"image_sequences":crate::image_sequence::capabilities(),"scenes":{"spatial":crate::spatial::capabilities(),"profile":"pixel-scene-v1","maximum_seconds":10,"limits":crate::scene::limits(),"color":"srgb_straight_encoded","render_mode":"blocking CLI/library or job.start","transparent_output":"straight_alpha_ffv1_bgra_for_alpha_over_tracks_normal_blend_without_geometry","alpha_modes":["straight","premultiplied"],"blend_modes":["normal","multiply","screen"],"mask":{"shape":"rectangle","space":"source_canvas_before_transform","inversion":true,"animated_properties":["x","y","width","height"],"maximum_per_layer":1,"feather":{"radius":[1,4096],"edges":["inner","centered","outer"],"coverage_grid":65536},"tracking":crate::tracking::capabilities()},"animation":{"properties":["position_x","position_y","opacity"],"interpolation":["hold","linear","ease_in","ease_out","ease_in_out"],"easing_profile":"quadratic","clock":"layer_local","retime":{"scope":"per_property_or_mask_curve","start":"layer_local_playback_anchor","rate_minimum":{"num":1,"den":16},"rate_maximum":{"num":16,"den":1},"reverse":true,"changes_media_time":false},"rounding":"nearest_ties_away_from_zero","maximum_keys_per_curve":128,"maximum_reduced_time_denominator":1000000,"precision":"checked_exact_integer_overflow_rejected"}},"renderer":{"profile":"reference-ffv1-pcm-v1","frame_rate":25,"sequential_frame_rates":[{"num":24,"den":1},{"num":25,"den":1},{"num":30,"den":1},{"num":50,"den":1},{"num":60,"den":1},{"num":24000,"den":1001},{"num":30000,"den":1001},{"num":60000,"den":1001}],"sequential_cut_policy":"whole_video_frames_and_48000_hz_samples","placed_track_frame_rates":"same_as_sequential_frame_rates","maximum_sequential_frames":180000,"large_raster":{"minimum_pixels":8000000,"minimum_dimension":4,"sequential_encoder_threads":16,"sequential_slices":16,"sequential_input_decoder_threads":4,"video_inspection_threads":16,"video_inspection_timeout_seconds":900,"sequential_encode_timeout_seconds":1800},"audio_rate":48000,"channels":2,"maximum_clips":64,"long_timelines":"chunks_of_at_most_64_clips_joined_by_stream_copy_on_whole_frame_sample_millisecond_boundaries","gaps":{"explicit":true,"video":"black","audio":"silence","maximum_frames":180000}},
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
    result
}

fn rate(time: &Value) -> String {
    match (time["num"].as_u64(), time["den"].as_u64()) {
        (Some(num), Some(1)) => num.to_string(),
        (Some(num), Some(den)) => format!("{num}/{den}"),
        _ => time.to_string(),
    }
}

/// A short summary by default, one area by name, or everything with `all`.
fn capabilities(section: Option<&str>) -> Result<Value> {
    let all = all_capabilities();
    match section {
        Some("all") => return Ok(all),
        Some(name) => {
            return match all.get(name) {
                Some(value) if !matches!(name, "version" | "license" | "local_only") => {
                    Ok(json!({ name: value }))
                }
                _ => Err(crate::error(
                    "UNKNOWN_SECTION",
                    format!(
                        "No capabilities section {name:?}; choose all or one of: {}",
                        sections(&all).join(", ")
                    ),
                )),
            };
        }
        None => {}
    }
    let renderer = &all["renderer"];
    let scenes = &all["scenes"]["limits"];
    let rates: Vec<String> = renderer["sequential_frame_rates"]
        .as_array()
        .into_iter()
        .flatten()
        .map(rate)
        .collect();
    let essentials = [
        "Timeline sources must be FFV1 video with 48 kHz stereo PCM16 audio at the project size (the reference profile); convert other media with media.conform into editing assets.".to_owned(),
        format!(
            "Timelines (sequential or placed tracks) run at {} fps; scenes, captions and H.264 delivery follow the timeline's rate, while proxies and camera groups stay at 25 fps.",
            rates.join(", ")
        ),
        format!(
            "Timelines hold up to 1000 clips and {} frames; a range with more than {} clips renders as exactly joined chunks. render.start queues reference .mkv renders; job.start queues export.run (H.264/AAC or lossless delivery), media.conform, scene.render and the other long commands.",
            renderer["maximum_sequential_frames"], renderer["maximum_clips"]
        ),
        format!(
            "Scenes (titles, graphics, captions) last up to {} frames at {} fps, with up to {} layers and {} px per axis; scene.render compiles one into an editing asset.",
            scenes["frames"][1],
            rate(&scenes["frame_rate"]),
            scenes["layers"][1],
            scenes["canvas_per_axis"][1]
        ),
        "Times are exact: 2.5, \"5/2\" or {num, den} seconds, on frame boundaries, and on whole 48 kHz samples for audio cuts.".to_owned(),
    ];
    Ok(
        json!({"version":all["version"],"license":all["license"],"local_only":all["local_only"],
        "essentials":essentials,"commands":all["commands"],"operations":all["operations"],
        "sections":sections(&all),
        "note":"Pass section with one of these names for its details, or all for everything."}),
    )
}

fn sections(all: &Value) -> Vec<String> {
    let summary = ["version", "license", "local_only", "commands", "operations"];
    all.as_object()
        .into_iter()
        .flat_map(|o| o.keys())
        .filter(|k| !summary.contains(&k.as_str()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_summarize_by_default_and_detail_by_section() {
        let summary = capabilities(None).unwrap();
        assert!(summary.to_string().len() < 6 * 1024);
        assert_eq!(summary["local_only"], true);
        assert!(
            summary["commands"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c == "render.run")
        );
        let sections = summary["sections"].as_array().unwrap();
        assert!(sections.iter().any(|s| s == "scenes"));
        let scenes = capabilities(Some("scenes")).unwrap();
        assert!(scenes["scenes"]["limits"].is_object());
        assert_eq!(capabilities(Some("all")).unwrap(), all_capabilities());
        assert_eq!(
            capabilities(Some("nope")).unwrap_err().code,
            "UNKNOWN_SECTION"
        );
    }
}
