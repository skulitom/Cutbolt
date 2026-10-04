//! Original local speech orchestration: exact parent media, owned derivatives and
//! a fixed optional worker with OS-level network/descendant lifetime boundaries.
use crate::{Result, error, media, pcm_stream, registry::Identity, render, time::Time, transcript};
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
const PROTOCOL: &str = "cutbolt-transcription-v1";
const PROFILE: &str = "local-en-el-context-v1";

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Runtime {
    pub distribution: String,
    pub python: String,
    pub python_paths: Vec<String>,
    pub model: PathBuf,
    pub alignment_root: PathBuf,
    pub threads: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Format {
    StereoWav,
    ReferenceMovie { width: u32, height: u32 },
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Left,
    Right,
    Mean,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transcribe {
    pub id: String,
    pub source: transcript::Source,
    pub format: Format,
    pub start: Time,
    pub duration: Time,
    pub channel: Channel,
    pub language: transcript::Language,
    pub input_root: PathBuf,
    pub scratch_root: PathBuf,
    pub runtime: Runtime,
    pub timeout_seconds: u32,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_TRANSCRIPTION", message)
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
    if fs::metadata(&path)?.len() != request.source.identity.bytes
        || media::file_hash_controlled(&path, control)? != request.source.identity.sha256
    {
        return Err(error(
            "MEDIA_CHANGED",
            "Transcription parent source identity differs",
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
        let _ = fs::remove_dir(&self.0);
    }
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
    let stdout = reader(child.0.stdout.take().expect("piped"), 1048576);
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
        let code = match value["error"]["code"].as_str().unwrap_or("") {
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
            _ => "TRANSCRIPTION_FAILED",
        };
        return Err(error(
            code,
            value["error"]["message"]
                .as_str()
                .unwrap_or("Speech worker failed"),
        ));
    }
    if !status.success() || value["ok"] != true {
        return Err(error(
            "WORKER_FAILED",
            "Speech process/result status mismatch",
        ));
    }
    Ok(value["result"].clone())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Word {
    id: String,
    text: String,
    start_sample: u64,
    end_sample: u64,
    probability_milli: u16,
    ctc_start_sample: u64,
    ctc_end_sample: u64,
    acoustic_start_sample: u64,
    acoustic_end_sample: u64,
    alignment_score_milli: u16,
}
pub fn run(request: &Transcribe) -> Result<Value> {
    run_controlled(request, &media::Uncontrolled)
}
pub fn run_controlled(request: &Transcribe, control: &dyn media::Control) -> Result<Value> {
    if !cfg!(windows) {
        return Err(error(
            "UNSUPPORTED_PLATFORM",
            "This optional speech profile uses Windows with explicitly configured WSL/CUDA",
        ));
    }
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
    let runtime = &request.runtime;
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
    control.phase("transcription.source")?;
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
    if fs::metadata(&runtime.model)?.len() != MODEL_BYTES
        || media::file_hash_controlled(&runtime.model, control)? != MODEL_HASH
    {
        return Err(error(
            "MODEL_CHANGED",
            "Selected speech profile requires its pinned local model",
        ));
    }
    let model_path = linux_path(&runtime.model)?;
    let alignment_root = linux_path(&runtime.alignment_root)?;
    let scratch = Scratch::new(&request.scratch_root)?;
    let root = linux_path(&scratch.0)?;
    for (name, content) in [(FILES[1], WORKER), (FILES[2], SUPERVISOR)] {
        let mut file = File::create_new(scratch.0.join(name))?;
        file.write_all(content.as_bytes())?;
    }
    control.phase("transcription.analysis")?;
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
    let analysis = scratch.0.join(FILES[0]);
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
    let worker_hash = hash(WORKER.as_bytes());
    let payload = json!({"protocol":PROTOCOL,"source_path":format!("{root}/analysis.wav"),"source_sha256":analysis_hash,
        "source_bytes":pcm.len()+44,"sample_count":count,"model_path":model_path,"alignment_root":alignment_root,
        "language":request.language,"threads":runtime.threads});
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
    control.phase("transcription.recognition")?;
    let result = capture(
        &args,
        &json!({"request":payload,"worker_sha256":worker_hash,"timeout_seconds":request.timeout_seconds}),
        request.timeout_seconds,
        control,
    )?;
    if result["protocol"] != PROTOCOL
        || result["profile"] != PROFILE
        || result["source_sha256"] != analysis_hash
        || result["sample_count"] != count
        || result["model_sha256"] != MODEL_HASH
        || result["language"] != json!(request.language)
        || result["network_interfaces"] != json!(["lo"])
        || result["python_network_attempts"] != 0
        || result["context_samples"] != json!({"leading":1280,"trailing":320})
        || result["review_required"] != true
    {
        return Err(error(
            "INVALID_RESULT",
            "Speech result does not match its bound request/profile",
        ));
    }
    let raw: Vec<Word> = serde_json::from_value(result["words"].clone())?;
    if raw.is_empty() || raw.len() > transcript::MAX_WORDS {
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
                text: word.text,
                start: source_time(word.start_sample)?,
                end: source_time(word.end_sample)?,
                origin: transcript::Origin::Estimated,
                probability_milli: Some(word.probability_milli),
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
    let files: BTreeMap<String, (u64, String)> =
        serde_json::from_value(result["alignment_files"].clone())?;
    let alignment = transcript::AlignmentProfile {
        files: files
            .into_iter()
            .map(|(name, (bytes, sha256))| (name, Identity { bytes, sha256 }))
            .collect(),
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
            profile: PROFILE.into(),
            model: Identity {
                bytes: MODEL_BYTES,
                sha256: MODEL_HASH.into(),
            },
            worker_sha256: worker_hash,
            supervisor_sha256: Some(hash(SUPERVISOR.as_bytes())),
            analysis_sha256: analysis_hash.clone(),
            versions: serde_json::from_value(result["versions"].clone())?,
            alignment: Some(alignment),
        },
        words,
    };
    document.validate()?;
    source_file(request, control)?;
    control.check()?;
    Ok(
        json!({"document":document,"fingerprint":document.fingerprint()?,"applied":false,"review_required":true,
        "analysis":{"sha256":analysis_hash,"source_start":request.start,"source_duration":request.duration,"sample_rate":16000,"sample_count":count,"channel":request.channel,
            "resampler":"swr_filter32_phase10_no_dither","last_sample_clamp_48000":count*3-length,
            "partial_tail_padding_16000":partial_padding},"worker":result}),
    )
}
