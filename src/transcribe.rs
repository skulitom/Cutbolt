//! Original local speech orchestration: exact parent media, owned derivatives and
//! a fixed optional worker with OS-level network/descendant lifetime boundaries.
use crate::{
    Result, error, media, pcm_stream,
    registry::{self, Identity},
    render,
    time::Time,
    transcript,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Child, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const WORKER: &str = include_str!("../tools/transcribe_worker.py");
const SUPERVISOR: &str = include_str!("../tools/transcribe_supervisor.py");
const FILES: [&str; 3] = [
    "analysis.wav",
    "transcribe_worker.py",
    "transcribe_supervisor.py",
];
const MODEL_HASH: &str = "9ecf779972d90ba49c06d968637d720dd632c55bbf19d441fb42bf17a411e794";
const MODEL_BYTES: u64 = 483617219;
const PROTOCOL: &str = "cutbolt-transcription-v2";
/// Analyses one worker launch takes; the worker and supervisor own `analysis-<n>.wav` up to this.
const BATCH: usize = 16;
/// Bound of one worker launch's result, matching the supervisor and worker.
const RESULT_BYTES: usize = 16 * 1048576;
const PROFILE: &str = "local-en-el-context-v1";
/// Known text aligned to the audio instead of recognized.
const ALIGN_PROFILE: &str = "local-en-el-align-v1";
/// Bytes of known text one request may align.
const MAX_TEXT: usize = 32 * 1024;

/// Trusted local speech runtime configuration; nothing is installed or downloaded.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    /// WSL distribution name: 1-64 ASCII letters, digits, `-`, `_` or `.`.
    pub distribution: String,
    /// Absolute Linux path of the Python interpreter inside the distribution.
    pub python: String,
    /// 1-8 absolute Linux package directories added to the Python path.
    pub python_paths: Vec<String>,
    /// Absolute Windows path of the profile's pinned speech model file.
    pub model: PathBuf,
    /// Absolute Windows directory holding the acoustic alignment model for `language`.
    pub alignment_root: PathBuf,
    /// Worker thread count, 1-8.
    pub threads: u32,
}
/// Source media profile, tagged by `type`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Format {
    /// 48 kHz stereo PCM16 WAV file.
    StereoWav,
    /// Reference movie in the 25 fps FFV1/PCM profile.
    ReferenceMovie {
        /// Frame width in pixels.
        width: u32,
        /// Frame height in pixels.
        height: u32,
    },
}
/// Analysed channel: `left`, `right`, or `mean` (equal mix of both).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Left,
    Right,
    Mean,
}
/// Request for transcript.transcribe: local speech recognition of one source interval into a transcript document.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transcribe {
    /// ID of the returned transcript document, 1-128 bytes.
    pub id: String,
    /// Identity-bound source; its duration must equal the actual 48 kHz sample count.
    pub source: transcript::Source,
    /// Source media profile.
    pub format: Format,
    /// Absolute source start, reduced rational seconds on the 48 kHz clock.
    pub start: Time,
    /// Analysed length, 25 ms to 120 s on the 48 kHz clock; must end within the source.
    pub duration: Time,
    /// Source channel to analyse.
    pub channel: Channel,
    /// Spoken language.
    pub language: transcript::Language,
    /// Absolute directory containing the source.
    pub input_root: PathBuf,
    /// Existing absolute directory for an owned scratch directory, removed afterwards.
    pub scratch_root: PathBuf,
    /// Local speech runtime configuration.
    pub runtime: Runtime,
    /// Analysis deadline in seconds, 1-600.
    pub timeout_seconds: u32,
    /// Known spoken text, such as a narration script: its words are aligned to the audio
    /// instead of recognized, so names and spelling stay exactly as given. Words split at
    /// whitespace; write numbers out in words. At most 32 KiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Names and terms the speech may contain, spelled as wanted (PixelForge): the recognizer is
    /// prompted with them, and words it splits or spells differently become them. Include um and
    /// uh to have fillers written down. 1-32 terms of 1-64 bytes; not with `text`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vocabulary: Vec<String>,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_TRANSCRIPTION", message)
}
/// The words of known text: whitespace-separated, with a token of only punctuation (such as a
/// dash between spaces) joined to the word before it, or to the next word at the start.
pub(crate) fn text_words(text: &str) -> Result<Vec<String>> {
    if text.len() > MAX_TEXT {
        return Err(invalid("Text to align is at most 32 KiB"));
    }
    let mut words: Vec<String> = Vec::new();
    let mut leading = String::new();
    for token in text.split_whitespace() {
        if token.chars().any(char::is_control) {
            return Err(invalid("Text to align has control characters"));
        }
        if !token.chars().any(char::is_alphanumeric) {
            match words.last_mut() {
                Some(last) => last.push_str(token),
                None => leading.push_str(token),
            }
            continue;
        }
        words.push(format!("{}{token}", std::mem::take(&mut leading)));
    }
    if words.is_empty()
        || words.len() > transcript::MAX_WORDS
        || words.iter().any(|w| w.len() > 512)
    {
        return Err(invalid(
            "Text to align needs 1-2048 words with a letter or digit, each at most 512 bytes",
        ));
    }
    Ok(words)
}
/// Check a recognition vocabulary: 1-32 terms of 1-64 bytes, for recognition only. Its prompt
/// must also fit the recognizer's context, which the worker checks with the recognizer's tokens.
pub(crate) fn check_vocabulary(vocabulary: &[String], aligning: bool) -> Result<()> {
    if vocabulary.is_empty() {
        return Ok(());
    }
    if aligning {
        return Err(invalid(
            "vocabulary guides recognition; known text is aligned exactly as given, so give one or the other",
        ));
    }
    if vocabulary.len() > transcript::MAX_TERMS
        || !vocabulary.iter().all(|t| transcript::term_ok(t))
    {
        return Err(invalid(
            "vocabulary takes 1-32 terms of 1-64 bytes, without control characters or surrounding spaces",
        ));
    }
    Ok(())
}
/// Cheap checks of a runtime configuration, before any media work.
pub(crate) fn check_runtime(runtime: &Runtime) -> Result<()> {
    if !cfg!(windows) {
        return Err(error(
            "UNSUPPORTED_PLATFORM",
            "This optional speech profile uses Windows with explicitly configured WSL/CUDA",
        ));
    }
    if !runtime.model.is_absolute()
        || !runtime.alignment_root.is_absolute()
        || !runtime.model.is_file()
        || !runtime.alignment_root.is_dir()
    {
        return Err(error(
            "MODEL_UNAVAILABLE",
            "Explicit local speech model and alignment directory are required",
        ));
    }
    if runtime.distribution.is_empty()
        || runtime.distribution.len() > 64
        || !runtime
            .distribution
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        || !posix(&runtime.python)
        || runtime.python_paths.is_empty()
        || runtime.python_paths.len() > 8
        || runtime.python_paths.iter().any(|p| !posix(p))
        || !(1..=8).contains(&runtime.threads)
    {
        return Err(invalid(
            "Explicit bounded WSL distribution, absolute Python/package paths and 1..8 threads required",
        ));
    }
    Ok(())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn exact(time: Time) -> Result<u64> {
    time.validate()?;
    if Time::new(time.num, time.den)? != time {
        return Err(invalid("Use reduced source-time rationals"));
    }
    time.units(transcript::RATE)
}
fn posix(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 4096
        && !path.contains(':')
        && !path.chars().any(char::is_control)
        && path
            .split('/')
            .skip(1)
            .all(|p| !p.is_empty() && p != "." && p != "..")
}
fn linux_path(path: &Path) -> Result<String> {
    let path = path.canonicalize()?;
    let text = path
        .to_str()
        .ok_or_else(|| invalid("WSL paths require Unicode"))?;
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);
    let bytes = text.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || &bytes[1..3] != b":\\" {
        return Err(invalid(
            "This WSL profile requires a local drive mounted under /mnt/<drive>",
        ));
    }
    Ok(format!(
        "/mnt/{}/{}",
        (bytes[0] as char).to_ascii_lowercase(),
        text[3..].replace('\\', "/")
    ))
}
fn source_file(request: &Transcribe, control: &dyn media::Control) -> Result<PathBuf> {
    request.source.identity.validate()?;
    if request.source.path.as_os_str().is_empty()
        || request
            .source
            .path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || request.source.path.to_string_lossy().contains(':')
    {
        return Err(invalid(
            "Source paths must be normal relative paths under input_root",
        ));
    }
    let path = media::allowed_file(
        &request.input_root.join(&request.source.path),
        &request.input_root,
    )?;
    let (label, expected) = (
        request.source.path.to_string_lossy(),
        &request.source.identity,
    );
    let size = fs::metadata(&path)?.len();
    if size != expected.bytes {
        return Err(registry::changed_size(
            &label,
            expected.bytes,
            size,
            registry::UPDATE_IDENTITY,
        ));
    }
    let digest = media::file_hash_controlled(&path, control)?;
    if digest != expected.sha256 {
        return Err(registry::changed_digest(
            &label,
            &expected.sha256,
            &digest,
            registry::UPDATE_IDENTITY,
        ));
    }
    Ok(path)
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(root: &Path) -> Result<Self> {
        let root = media::input_root(root)?;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("Invalid system clock"))?
            .as_nanos();
        let path = root.join(format!("cutbolt-transcribe-{}-{nonce}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        for name in FILES {
            let _ = fs::remove_file(self.0.join(name));
        }
        for slot in 0..BATCH {
            let _ = fs::remove_file(self.0.join(analysis_name(slot)));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
/// Owned analysis file of a launch's `slot`.
fn analysis_name(slot: usize) -> String {
    format!("analysis-{slot}.wav")
}
struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        self.0.stdin.take();
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn reader(mut input: impl Read + Send + 'static, limit: usize) -> mpsc::Receiver<Result<Vec<u8>>> {
    let (tx, rx) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let output = (|| {
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            let mut excessive = false;
            loop {
                let count = input.read(&mut chunk)?;
                if count == 0 {
                    break;
                }
                let keep = count.min(limit.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&chunk[..keep]);
                excessive |= keep != count;
            }
            if excessive {
                Err(error(
                    "RESULT_LIMIT",
                    "Speech process output exceeded its bound",
                ))
            } else {
                Ok(bytes)
            }
        })();
        let _ = tx.send(output);
    });
    rx
}
fn capture(
    args: &[String],
    request: &Value,
    timeout: u32,
    control: &dyn media::Control,
) -> Result<Value> {
    control.check()?;
    let mut command = control.command(&control.tool("wsl"), args)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = Worker(
        command
            .spawn()
            .map_err(|e| error("RUNTIME_UNAVAILABLE", e.to_string()))?,
    );
    let stdout = reader(child.0.stdout.take().expect("piped"), RESULT_BYTES);
    let stderr = reader(child.0.stderr.take().expect("piped"), 65536);
    let mut encoded = serde_json::to_vec(request)?;
    encoded.push(b'\n');
    if encoded.len() > 65536 {
        return Err(invalid("Speech worker request exceeds 64 KiB"));
    }
    child.0.stdin.as_mut().expect("piped").write_all(&encoded)?;
    let start = Instant::now();
    let outcome = (|| {
        loop {
            control.check()?;
            if let Some(status) = child.0.try_wait()? {
                return Ok(status);
            }
            if start.elapsed() > Duration::from_secs(timeout as u64 + 20) {
                return Err(error(
                    "WORKER_TIMEOUT",
                    "Speech runtime startup/deadline exceeded",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    child.0.stdin.take();
    if outcome.is_err() {
        let grace = Instant::now();
        while child.0.try_wait()?.is_none() && grace.elapsed() < Duration::from_secs(2) {
            thread::sleep(Duration::from_millis(20));
        }
        if child.0.try_wait()?.is_none() {
            let _ = child.0.kill();
            let _ = child.0.wait();
        }
    }
    let out = stdout.recv_timeout(Duration::from_secs(5)).map_err(|_| {
        error(
            "WORKER_LIFETIME",
            "Speech stdout did not close after owner cancellation",
        )
    })??;
    let err = stderr.recv_timeout(Duration::from_secs(5)).map_err(|_| {
        error(
            "WORKER_LIFETIME",
            "Speech stderr did not close after owner cancellation",
        )
    })??;
    let status = outcome?;
    let value: Value = serde_json::from_slice(&out).map_err(|_| {
        error(
            "WORKER_FAILED",
            format!(
                "Speech runtime did not return JSON: {}",
                String::from_utf8_lossy(&err[..err.len().min(512)])
            ),
        )
    })?;
    if value["ok"] == false {
        return Err(worker_error(&value["error"]));
    }
    if !status.success() || value["ok"] != true {
        return Err(error(
            "WORKER_FAILED",
            "Speech process/result status mismatch",
        ));
    }
    Ok(value["result"].clone())
}

/// The engine error for a worker-reported failure.
fn worker_error(reported: &Value) -> crate::Error {
    {
        let code = match reported["code"].as_str().unwrap_or("") {
            "WORKER_TIMEOUT" => "WORKER_TIMEOUT",
            "OWNER_GONE" => "OWNER_GONE",
            "MODEL_UNAVAILABLE" => "MODEL_UNAVAILABLE",
            "IDENTITY_MISMATCH" => "MODEL_CHANGED",
            "RUNTIME_VERSION" => "RUNTIME_VERSION",
            "RUNTIME_UNAVAILABLE" => "RUNTIME_UNAVAILABLE",
            "DEVICE_UNAVAILABLE" => "DEVICE_UNAVAILABLE",
            "ISOLATION_REQUIRED" => "ISOLATION_REQUIRED",
            "NO_WORDS" => "NO_WORDS",
            "RESULT_LIMIT" => "RESULT_LIMIT",
            "RESOURCE_LIMIT" => "RESOURCE_LIMIT",
            "INVALID_ALIGNMENT" => "INVALID_ALIGNMENT",
            "UNSUPPORTED_ALIGNMENT_TEXT" => "UNSUPPORTED_ALIGNMENT_TEXT",
            "WORKER_CHANGED" => "WORKER_CHANGED",
            "UNSUPPORTED_AUDIO" => "UNSUPPORTED_AUDIO",
            "INPUT_LIMIT" => "INPUT_LIMIT",
            _ => "TRANSCRIPTION_FAILED",
        };
        error(
            code,
            reported["message"]
                .as_str()
                .unwrap_or("Speech worker failed"),
        )
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Word {
    id: String,
    text: String,
    start_sample: u64,
    end_sample: u64,
    probability_milli: Option<u16>,
    ctc_start_sample: u64,
    ctc_end_sample: u64,
    acoustic_start_sample: u64,
    acoustic_end_sample: u64,
    alignment_score_milli: u16,
}
/// Speech no word covers, on the analysis clock, with the letters the acoustic model read there.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sound {
    start_sample: u64,
    end_sample: u64,
    letters: String,
}
/// A request's vocabulary as the worker takes and echoes it: null when there is none.
fn vocabulary(request: &Transcribe) -> Value {
    if request.vocabulary.is_empty() {
        Value::Null
    } else {
        json!(request.vocabulary)
    }
}
/// Recognized text that is not speech, such as "[Music]", on the analysis clock.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Note {
    text: String,
    kind: String,
    start_sample: u64,
    end_sample: u64,
}
pub fn run(request: &Transcribe) -> Result<Value> {
    run_controlled(request, &media::Uncontrolled)
}
pub fn run_controlled(request: &Transcribe, control: &dyn media::Control) -> Result<Value> {
    run_many(std::slice::from_ref(request), control)?
        .pop()
        .expect("one outcome per request")
}

/// One analysis made ready for a worker launch.
struct Prepared {
    file: String,
    analysis_hash: String,
    bytes: usize,
    count: u64,
    start: u64,
    length: u64,
    partial_padding: u64,
    given: Option<Vec<String>>,
}

/// The checked known text of a request, or None to recognize.
fn validate(request: &Transcribe) -> Result<Option<Vec<String>>> {
    crate::tracks::id(&request.id)?;
    let start = exact(request.start)?;
    let length = exact(request.duration)?;
    let source_samples = exact(request.source.duration)?;
    if !(1200..=5760000).contains(&length)
        || start
            .checked_add(length)
            .is_none_or(|end| end > source_samples)
        || !(1..=600).contains(&request.timeout_seconds)
    {
        return Err(invalid(
            "Analysis requires 25 ms..120 s inside the source and an explicit 1..600 s deadline",
        ));
    }
    check_vocabulary(&request.vocabulary, request.text.is_some())?;
    request.text.as_deref().map(text_words).transpose()
}

/// Several analyses with one runtime and language. A worker launch takes up to 16 of them,
/// within the 64 KiB request and 600 s deadline bounds, so its models load once per launch
/// rather than once per analysis. Each request keeps its own outcome; a failure of the shared
/// runtime or model fails them all.
pub fn run_many(
    requests: &[Transcribe],
    control: &dyn media::Control,
) -> Result<Vec<Result<Value>>> {
    if !cfg!(windows) {
        return Err(error(
            "UNSUPPORTED_PLATFORM",
            "This optional speech profile uses Windows with explicitly configured WSL/CUDA",
        ));
    }
    let first = requests
        .first()
        .ok_or_else(|| invalid("No analyses were requested"))?;
    let runtime = &first.runtime;
    let shared = serde_json::to_value(runtime)?;
    if requests.iter().any(|r| {
        r.language != first.language
            || r.scratch_root != first.scratch_root
            || serde_json::to_value(&r.runtime).ok().as_ref() != Some(&shared)
    }) {
        return Err(invalid(
            "One batch of analyses shares its runtime, language and scratch root",
        ));
    }
    let mut outcomes: Vec<Option<Result<Value>>> = requests.iter().map(|_| None).collect();
    let mut checked = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        match validate(request) {
            Ok(given) => checked.push((index, given)),
            Err(e) => outcomes[index] = Some(Err(e)),
        }
    }
    if checked.is_empty() {
        return Ok(outcomes
            .into_iter()
            .map(|o| o.expect("every request has an outcome"))
            .collect());
    }
    check_runtime(runtime)?;
    control.phase("transcribing windows")?;
    control.total(requests.len() as u64);
    // Aligning known text needs no recognition model.
    let model_path = if checked.iter().any(|(_, given)| given.is_none()) {
        if fs::metadata(&runtime.model)?.len() != MODEL_BYTES
            || media::file_hash_controlled(&runtime.model, control)? != MODEL_HASH
        {
            return Err(error(
                "MODEL_CHANGED",
                "Selected speech profile requires its pinned local model",
            ));
        }
        Some(linux_path(&runtime.model)?)
    } else {
        None
    };
    let alignment_root = linux_path(&runtime.alignment_root)?;
    let worker_hash = hash(WORKER.as_bytes());
    // Groups that fit one launch: at most 16 items, a 64 KiB request and ten minutes of audio,
    // which the profile analyses well inside its 600 s deadline (about a third of real time).
    let mut groups: Vec<Vec<(usize, Option<Vec<String>>)>> = Vec::new();
    let (mut bytes, mut samples) = (0usize, 0u64);
    for (index, given) in checked {
        let size = 512
            + serde_json::to_vec(&given)?.len()
            + serde_json::to_vec(&requests[index].vocabulary)?.len();
        let length = exact(requests[index].duration)?;
        let full = groups.last().is_none_or(|group| {
            group.len() == BATCH || bytes + size > 60 * 1024 || samples + length > 600 * 48_000
        });
        if full {
            groups.push(Vec::new());
            (bytes, samples) = (0, 0);
        }
        bytes += size;
        samples += length;
        groups.last_mut().expect("group").push((index, given));
    }
    for group in groups {
        // The supervisor removes its scratch directory when a launch ends, so each launch
        // gets its own.
        let scratch = Scratch::new(&first.scratch_root)?;
        let root = linux_path(&scratch.0)?;
        for (name, content) in [(FILES[1], WORKER), (FILES[2], SUPERVISOR)] {
            let mut file = File::create_new(scratch.0.join(name))?;
            file.write_all(content.as_bytes())?;
        }
        let mut prepared = Vec::new();
        for (slot, (index, given)) in group.into_iter().enumerate() {
            match prepare(&requests[index], given, &scratch, slot, control) {
                Ok(item) => prepared.push((index, item)),
                Err(e) => outcomes[index] = Some(Err(e)),
            }
        }
        if prepared.is_empty() {
            continue;
        }
        let items: Vec<Value> = prepared
            .iter()
            .map(|(index, p)| {
                json!({"source_path":format!("{root}/{}", p.file),"source_sha256":p.analysis_hash,
                "source_bytes":p.bytes,"sample_count":p.count,"text":p.given,
                "vocabulary":vocabulary(&requests[*index])})
            })
            .collect();
        let recognizing = prepared.iter().any(|(_, p)| p.given.is_none());
        let payload = json!({"protocol":PROTOCOL,"items":items,"model_path":if recognizing { model_path.clone() } else { None },
            "alignment_root":alignment_root,"language":first.language,"threads":runtime.threads});
        let timeout = prepared
            .iter()
            .map(|(index, _)| requests[*index].timeout_seconds)
            .sum::<u32>()
            .min(600);
        let args = vec![
            "--distribution".into(),
            runtime.distribution.clone(),
            "--exec".into(),
            "unshare".into(),
            "-Urnpf".into(),
            "--kill-child=KILL".into(),
            "--mount-proc".into(),
            "env".into(),
            format!("PYTHONPATH={}", runtime.python_paths.join(":")),
            "PYTHONDONTWRITEBYTECODE=1".into(),
            runtime.python.clone(),
            "-B".into(),
            format!("{root}/transcribe_supervisor.py"),
        ];
        control.check()?;
        let launched = capture(
            &args,
            &json!({"request":payload,"worker_sha256":worker_hash,"timeout_seconds":timeout}),
            timeout,
            control,
        );
        let results = launched.and_then(|result| match result["items"].as_array() {
            Some(items) if result["protocol"] == PROTOCOL && items.len() == prepared.len() => {
                Ok(items.clone())
            }
            _ => Err(error(
                "INVALID_RESULT",
                "Speech results do not match the launched analyses",
            )),
        });
        match results {
            Ok(items) => {
                for ((index, item), outcome) in prepared.iter().zip(items) {
                    outcomes[*index] = Some(if outcome["ok"] == true {
                        finish(
                            &requests[*index],
                            item,
                            &outcome["result"],
                            &worker_hash,
                            control,
                        )
                    } else {
                        Err(worker_error(&outcome["error"]))
                    });
                }
            }
            Err(e) => {
                for (index, _) in &prepared {
                    outcomes[*index] = Some(Err(error(e.code, e.message.clone())));
                }
            }
        }
        control.frames(outcomes.iter().filter(|o| o.is_some()).count() as u64)?;
    }
    Ok(outcomes
        .into_iter()
        .map(|o| o.expect("every request has an outcome"))
        .collect())
}

/// Check one request's source and write its 16 kHz mono analysis to the launch's `slot`.
fn prepare(
    request: &Transcribe,
    given: Option<Vec<String>>,
    scratch: &Scratch,
    slot: usize,
    control: &dyn media::Control,
) -> Result<Prepared> {
    let start = exact(request.start)?;
    let length = exact(request.duration)?;
    let source_samples = exact(request.source.duration)?;
    control.check()?;
    let source = source_file(request, control)?;
    let actual = match request.format {
        Format::StereoWav => pcm_stream::inspect(&source, control)?.frames,
        Format::ReferenceMovie { width, height } => {
            render::inspect_reference(&source, width, height, control)?.samples
        }
    };
    if actual != source_samples {
        return Err(error(
            "MEDIA_DURATION_MISMATCH",
            "Declared transcription duration differs from actual source samples",
        ));
    }
    control.check()?;
    let channel = match request.channel {
        Channel::Left => "c0=c0",
        Channel::Right => "c0=c1",
        Channel::Mean => "c0=0.5*c0+0.5*c1",
    };
    let filter = format!(
        "atrim=start_sample={start}:end_sample={},asetpts=PTS-STARTPTS,pan=mono|{channel},aresample=16000:resampler=swr:dither_method=none:filter_size=32:phase_shift=10",
        start + length
    );
    let args = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-i".into(),
        source.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-af".into(),
        filter,
        "-f".into(),
        "s16le".into(),
        "-acodec".into(),
        "pcm_s16le".into(),
        "-".into(),
    ];
    let mut pcm = media::capture_controlled(
        &control.tool("ffmpeg"),
        &args,
        Duration::from_secs(120),
        control,
    )?;
    let count = length.div_ceil(3);
    // SWR can round a fractional final analysis sample down. Preserve the
    // declared ceiling clock with one explicit silent partial sample, reported
    // to callers, rather than allowing tool rounding to change source duration.
    let partial_padding =
        u64::from(!length.is_multiple_of(3) && pcm.len() as u64 == (count - 1) * 2);
    if partial_padding == 1 {
        pcm.extend_from_slice(&[0, 0]);
    }
    if pcm.len() as u64 != count * 2 {
        return Err(error(
            "ANALYSIS_CLOCK",
            "Resampled speech count differs from the declared clock",
        ));
    }
    let file = analysis_name(slot);
    let analysis = scratch.0.join(&file);
    {
        let mut file = File::create_new(&analysis)?;
        let size = pcm.len() as u32;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + size).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&16000u32.to_le_bytes())?;
        file.write_all(&32000u32.to_le_bytes())?;
        file.write_all(&2u16.to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&size.to_le_bytes())?;
        file.write_all(&pcm)?;
    }
    let analysis_hash = media::file_hash_controlled(&analysis, control)?;
    Ok(Prepared {
        file,
        analysis_hash,
        bytes: pcm.len() + 44,
        count,
        start,
        length,
        partial_padding,
        given,
    })
}

/// The transcript document and receipt of one analysis from its worker result.
fn finish(
    request: &Transcribe,
    prepared: &Prepared,
    result: &Value,
    worker_hash: &str,
    control: &dyn media::Control,
) -> Result<Value> {
    let Prepared {
        analysis_hash,
        count,
        start,
        length,
        partial_padding,
        given,
        ..
    } = prepared;
    let (analysis_hash, count, start, length, partial_padding) = (
        analysis_hash.clone(),
        *count,
        *start,
        *length,
        *partial_padding,
    );
    let (profile, model_hash) = match given {
        Some(_) => (ALIGN_PROFILE, Value::Null),
        None => (PROFILE, json!(MODEL_HASH)),
    };
    if result["protocol"] != PROTOCOL
        || result["profile"] != profile
        || result["source_sha256"] != analysis_hash
        || result["sample_count"] != count
        || result["model_sha256"] != model_hash
        || result["language"] != json!(request.language)
        || result["network_interfaces"] != json!(["lo"])
        || result["python_network_attempts"] != 0
        || result["context_samples"] != json!({"leading":1280,"trailing":320})
        || result["review_required"] != true
        || result["vocabulary"] != vocabulary(request)
    {
        return Err(error(
            "INVALID_RESULT",
            "Speech result does not match its bound request/profile",
        ));
    }
    let raw: Vec<Word> = serde_json::from_value(result["words"].clone())?;
    // Sound without speech gives no words; known text comes back word for word.
    if raw.len() > transcript::MAX_WORDS
        || given.as_ref().is_some_and(|given| {
            given.len() != raw.len() || given.iter().zip(&raw).any(|(a, b)| *a != b.text)
        })
    {
        return Err(error(
            "INVALID_RESULT",
            "Speech returned an invalid word count",
        ));
    }
    let source_time = |n: u64| -> Result<Time> {
        if n > count {
            return Err(error(
                "INVALID_RESULT",
                "Word evidence exceeds the analysis clock",
            ));
        }
        Time::new(start + n.saturating_mul(3).min(length), 48000)
    };
    let words = raw
        .into_iter()
        .map(|word| {
            Ok(transcript::Word {
                id: word.id,
                text: word.text.trim().to_owned(),
                start: source_time(word.start_sample)?,
                end: source_time(word.end_sample)?,
                origin: transcript::Origin::Estimated,
                probability_milli: word.probability_milli,
                alignment: Some(transcript::WordAlignment {
                    ctc_start: source_time(word.ctc_start_sample)?,
                    ctc_end: source_time(word.ctc_end_sample)?,
                    acoustic_start: source_time(word.acoustic_start_sample)?,
                    acoustic_end: source_time(word.acoustic_end_sample)?,
                    score_milli: word.alignment_score_milli,
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let notes: Vec<Note> = serde_json::from_value(result["non_speech"].clone())?;
    let non_speech = notes
        .into_iter()
        .map(|note| {
            if note.start_sample > note.end_sample || note.text.len() > 4096 {
                return Err(error(
                    "INVALID_RESULT",
                    "Speech returned an invalid non-speech note",
                ));
            }
            Ok(json!({"text":note.text,"kind":note.kind,
                "start":source_time(note.start_sample)?,"end":source_time(note.end_sample)?}))
        })
        .collect::<Result<Vec<_>>>()?;
    let sounds: Vec<Sound> = serde_json::from_value(result["uncovered"].clone())?;
    let uncovered = sounds
        .into_iter()
        .map(|sound| {
            Ok(transcript::Uncovered {
                start: source_time(sound.start_sample)?,
                end: source_time(sound.end_sample)?,
                letters: sound.letters,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let reported: BTreeMap<String, (u64, String)> =
        serde_json::from_value(result["alignment_files"].clone())?;
    let files: BTreeMap<String, Identity> = reported
        .into_iter()
        .map(|(name, (bytes, sha256))| (name, Identity { bytes, sha256 }))
        .collect();
    // Aligned text records the acoustic model it was aligned with.
    let model = match given {
        Some(_) => files
            .get(match request.language {
                transcript::Language::En => "model.safetensors",
                transcript::Language::El => "pytorch_model.bin",
            })
            .cloned()
            .ok_or_else(|| error("INVALID_RESULT", "Alignment weights are not reported"))?,
        None => Identity {
            bytes: MODEL_BYTES,
            sha256: MODEL_HASH.into(),
        },
    };
    let alignment = transcript::AlignmentProfile {
        files,
        leading_context: Time::new(2, 25)?,
        trailing_context: Time::new(1, 50)?,
    };
    let document = transcript::Document {
        schema_version: 1,
        id: request.id.clone(),
        revision: 0,
        parent_fingerprint: None,
        source: request.source.clone(),
        range_start: request.start,
        range_duration: request.duration,
        language: request.language,
        recognition: transcript::Recognition {
            profile: profile.into(),
            model,
            worker_sha256: worker_hash.to_owned(),
            supervisor_sha256: Some(hash(SUPERVISOR.as_bytes())),
            analysis_sha256: analysis_hash.clone(),
            versions: serde_json::from_value(result["versions"].clone())?,
            alignment: Some(alignment),
            vocabulary: request.vocabulary.clone(),
        },
        words,
        uncovered,
    };
    document.validate()?;
    source_file(request, control)?;
    control.check()?;
    Ok(
        json!({"document":document,"fingerprint":document.fingerprint()?,"applied":false,"review_required":true,
        "non_speech":non_speech,"analysis":{"sha256":analysis_hash,"source_start":request.start,"source_duration":request.duration,"sample_rate":16000,"sample_count":count,"channel":request.channel,
            "resampler":"swr_filter32_phase10_no_dither","last_sample_clamp_48000":count*3-length,
            "partial_tail_padding_16000":partial_padding},"worker":result}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_text_splits_into_words_with_punctuation_attached() {
        assert_eq!(
            text_words("  A tiny character —  stands still,\nnow ... it hops. ").unwrap(),
            [
                "A",
                "tiny",
                "character—",
                "stands",
                "still,",
                "now...",
                "it",
                "hops."
            ]
        );
        assert_eq!(text_words("“ Hello world").unwrap(), ["“Hello", "world"]);
        assert_eq!(text_words("Cutbolt 2026").unwrap(), ["Cutbolt", "2026"]);
        for bad in ["", "  \n ", "- ... !", "a\u{7}b"] {
            assert_eq!(
                text_words(bad).unwrap_err().code,
                "INVALID_TRANSCRIPTION",
                "{bad:?}"
            );
        }
        assert!(text_words(&"x".repeat(513)).is_err());
        assert!(text_words(&"a ".repeat(2049)).is_err());
        assert_eq!(text_words(&"a ".repeat(2048)).unwrap().len(), 2048);
    }

    #[test]
    fn a_vocabulary_guides_recognition_within_its_bounds() {
        let terms = |list: &[&str]| list.iter().map(|t| t.to_string()).collect::<Vec<_>>();
        check_vocabulary(&[], true).unwrap();
        check_vocabulary(&terms(&["PixelForge", "Cutbolt", "um", "New York"]), false).unwrap();
        check_vocabulary(&terms(&[&"x".repeat(64)]), false).unwrap();
        let many: Vec<String> = (0..33).map(|n| format!("t{n}")).collect();
        for (bad, aligning) in [
            (terms(&["PixelForge"]), true),
            (terms(&[""]), false),
            (terms(&["um "]), false),
            (terms(&[&"x".repeat(65)]), false),
            (terms(&["a\u{7}"]), false),
            (many, false),
        ] {
            assert_eq!(
                check_vocabulary(&bad, aligning).unwrap_err().code,
                "INVALID_TRANSCRIPTION",
                "{bad:?}"
            );
        }
    }
}
