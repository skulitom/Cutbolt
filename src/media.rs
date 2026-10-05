use crate::{Result, error};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub fn tool(name: &str) -> String {
    std::env::var(format!("CUTBOLT_{}", name.to_uppercase())).unwrap_or_else(|_| name.into())
}

/// The selected encoder's version-3 slice grid corrupts sub-four-pixel dimensions.
/// Version 1 preserves those narrow frames; normal frames use version 3/four slices.
pub(crate) fn ffv1_encoding(width: u32, height: u32) -> (&'static str, &'static str) {
    if width < 4 || height < 4 {
        ("1", "1")
    } else {
        ("3", "4")
    }
}

/// Progress of the command this process runs for a queued job: a phase name and frames done out
/// of a total (0 when unknown).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub phase: String,
    pub frames: u64,
    pub total: u64,
}
/// Process-wide cancellation and progress. A command queued with job.start runs in its own
/// process (`job-command`); the queue worker cancels it through this state, which every
/// default `Control` polls, so tools started by any command stop. Elsewhere it is never set.
struct Ambient {
    cancelled: std::sync::atomic::AtomicBool,
    progress: std::sync::Mutex<(Progress, bool)>,
}
static AMBIENT: Ambient = Ambient {
    cancelled: std::sync::atomic::AtomicBool::new(false),
    progress: std::sync::Mutex::new((
        Progress {
            phase: String::new(),
            frames: 0,
            total: 0,
        },
        false,
    )),
};
/// Ask the command running in this process to stop.
pub fn cancel_ambient() {
    AMBIENT
        .cancelled
        .store(true, std::sync::atomic::Ordering::SeqCst);
}
pub(crate) fn ambient_cancelled() -> bool {
    AMBIENT.cancelled.load(std::sync::atomic::Ordering::SeqCst)
}
fn ambient_check() -> Result<()> {
    if ambient_cancelled() {
        Err(error(
            "JOB_CANCELLED",
            "Job cancelled; no output was published",
        ))
    } else {
        Ok(())
    }
}
fn ambient_update(update: impl FnOnce(&mut Progress)) {
    if let Ok(mut state) = AMBIENT.progress.lock() {
        let before = state.0.clone();
        update(&mut state.0);
        state.1 |= state.0 != before;
    }
}
/// The ambient progress, when it changed since the last call.
pub fn ambient_progress() -> Option<Progress> {
    let mut state = AMBIENT.progress.lock().ok()?;
    std::mem::take(&mut state.1).then(|| state.0.clone())
}

/// Cancellation/progress hooks are shared by synchronous rendering and persisted jobs. The
/// defaults follow the process-wide job state above.
pub trait Control {
    fn command(&self, program: &str, args: &[String]) -> Result<Command> {
        let mut command = Command::new(program);
        command.args(args);
        Ok(command)
    }
    fn tool(&self, name: &str) -> String {
        tool(name)
    }
    fn check(&self) -> Result<()> {
        ambient_check()
    }
    /// Start a named phase; its frame count restarts at zero.
    fn phase(&self, phase: &str) -> Result<()> {
        ambient_update(|p| {
            p.phase = phase.into();
            p.frames = 0;
        });
        self.check()
    }
    fn frames(&self, frames: u64) -> Result<()> {
        ambient_update(|p| p.frames = frames);
        self.check()
    }
    /// Frames the current work will produce.
    fn total(&self, frames: u64) {
        ambient_update(|p| p.total = frames);
    }
    fn sources(&self, _sources: &[crate::render::Source]) -> Result<()> {
        self.check()
    }
    fn publish(&self, temp: &Path, output: &Path, _receipt: &Value) -> Result<()> {
        self.check()?;
        publish(temp, output)
    }
}
pub struct Uncontrolled;
impl Control for Uncontrolled {}

pub fn publish(temp: &Path, output: &Path) -> Result<()> {
    fs::hard_link(temp, output).map_err(|e| {
        error(
            "PUBLISH_FAILED",
            format!("Output was not published (requires hard-link support): {e}"),
        )
    })
}

fn drain(
    mut reader: impl Read,
    limit: usize,
    progress: Option<mpsc::SyncSender<u64>>,
) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let mut truncated = false;
    let mut line = Vec::new();
    let mut buf = [0; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                let keep = n.min(limit.saturating_sub(bytes.len()));
                bytes.extend_from_slice(&buf[..keep]);
                truncated |= keep < n;
                if let Some(sender) = &progress {
                    for byte in &buf[..n] {
                        if *byte == b'\n' {
                            if let Ok(text) = std::str::from_utf8(&line)
                                && let Some(value) = text.trim().strip_prefix("frame=")
                                && let Ok(frame) = value.trim().parse()
                            {
                                let _ = sender.try_send(frame);
                            }
                            line.clear();
                        } else if line.len() < 1024 {
                            line.push(*byte);
                        }
                    }
                }
            }
        }
    }
    (bytes, truncated)
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn capture(program: &str, args: &[String], timeout: Duration) -> Result<Vec<u8>> {
    capture_controlled(program, args, timeout, &Uncontrolled)
}

pub fn capture_controlled(
    program: &str,
    args: &[String],
    timeout: Duration,
    control: &dyn Control,
) -> Result<Vec<u8>> {
    control.check()?;
    let mut command = control.command(program, args)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut child = ChildGuard(
        command
            .spawn()
            .map_err(|e| error("TOOL_UNAVAILABLE", format!("{program}: {e}")))?,
    );
    let stdout = child.0.stdout.take().expect("piped stdout");
    let stderr = child.0.stderr.take().expect("piped stderr");
    let (sender, receiver) = mpsc::sync_channel(1);
    let progress = args.iter().any(|a| a == "-progress").then_some(sender);
    let out_thread = thread::spawn(move || drain(stdout, 32 * 1024 * 1024, progress));
    let err_thread = thread::spawn(move || drain(stderr, 64 * 1024, None));
    let start = Instant::now();
    let outcome = (|| {
        loop {
            control.check()?;
            if let Ok(frames) = receiver.try_recv() {
                control.frames(frames)?;
            }
            if let Some(status) = child.0.try_wait()? {
                return Ok(status);
            }
            if start.elapsed() > timeout {
                return Err(error(
                    "TOOL_TIMEOUT",
                    format!("{program} exceeded {} seconds", timeout.as_secs()),
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    if outcome.is_err() {
        let _ = child.0.kill();
        let _ = child.0.wait();
    }
    let (out, truncated) = out_thread
        .join()
        .map_err(|_| error("TOOL_FAILED", "Output reader failed"))?;
    let (err, _) = err_thread
        .join()
        .map_err(|_| error("TOOL_FAILED", "Diagnostic reader failed"))?;
    if !outcome?.success() {
        return Err(error(
            "TOOL_FAILED",
            String::from_utf8_lossy(&err).into_owned(),
        ));
    }
    if truncated {
        return Err(error("LIMIT_EXCEEDED", "Tool output exceeded 32 MiB"));
    }
    control.check()?;
    Ok(out)
}

/// Supervision beyond a streamed tool's timeout.
#[derive(Clone, Default)]
pub(crate) struct Watch {
    /// Set by the owner to stop the tool, for example when a sibling tool in a pipeline failed.
    pub abort: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

/// Kills a tool when it exceeds its timeout, when the queued job is cancelled, or when its owner
/// aborts it, and remembers which happened.
struct Watchdog {
    finished: std::sync::Arc<std::sync::atomic::AtomicBool>,
    timed_out: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    program: String,
    timeout: Duration,
}
impl Watchdog {
    fn start(
        child: std::sync::Arc<std::sync::Mutex<ChildGuard>>,
        program: &str,
        timeout: Duration,
        watch: Watch,
    ) -> Self {
        use std::sync::{Arc, atomic::AtomicBool, atomic::Ordering};
        let finished = Arc::new(AtomicBool::new(false));
        let timed_out = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        {
            let (finished, timed_out, stopped) =
                (finished.clone(), timed_out.clone(), stopped.clone());
            thread::spawn(move || {
                let start = Instant::now();
                while !finished.load(Ordering::SeqCst) {
                    let flag = if start.elapsed() > timeout {
                        &timed_out
                    } else if ambient_cancelled()
                        || watch
                            .abort
                            .as_ref()
                            .is_some_and(|a| a.load(Ordering::SeqCst))
                    {
                        &stopped
                    } else {
                        thread::sleep(Duration::from_millis(50));
                        continue;
                    };
                    flag.store(true, Ordering::SeqCst);
                    let _ = child.lock().expect("child lock").0.kill();
                    return;
                }
            });
        }
        Self {
            finished,
            timed_out,
            stopped,
            program: program.into(),
            timeout,
        }
    }
    fn finish(&self) {
        self.finished
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// The reason the watchdog killed the tool, if it did.
    fn failure(&self) -> Option<crate::Error> {
        use std::sync::atomic::Ordering;
        if self.timed_out.load(Ordering::SeqCst) {
            Some(error(
                "TOOL_TIMEOUT",
                format!(
                    "{} exceeded {} seconds",
                    self.program,
                    self.timeout.as_secs()
                ),
            ))
        } else if self.stopped.load(Ordering::SeqCst) {
            Some(
                ambient_check().err().unwrap_or_else(|| {
                    error("TOOL_ABORTED", format!("{} was stopped", self.program))
                }),
            )
        } else {
            None
        }
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.finish();
    }
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    #[cfg(not(windows))]
    let _ = command;
}

/// Run a tool whose stdin receives bytes from `produce`, so large raw inputs never touch disk.
/// A watchdog enforces the timeout even while a write is blocked; diagnostics stay bounded.
pub(crate) fn feed_stdin(
    program: &str,
    args: &[String],
    timeout: Duration,
    produce: impl FnOnce(&mut dyn std::io::Write) -> Result<()>,
) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args);
    feed_stdin_with(command, program, timeout, Watch::default(), produce)
}
/// `feed_stdin` for a prepared command (for example from `Control::command`), with extra supervision.
pub(crate) fn feed_stdin_with(
    mut command: Command,
    program: &str,
    timeout: Duration,
    watch: Watch,
    produce: impl FnOnce(&mut dyn std::io::Write) -> Result<()>,
) -> Result<()> {
    use std::sync::{Arc, Mutex};
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden(&mut command);
    let mut child = command
        .spawn()
        .map_err(|e| error("TOOL_UNAVAILABLE", format!("{program}: {e}")))?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out_thread = thread::spawn(move || drain(stdout, 64 * 1024, None));
    let err_thread = thread::spawn(move || drain(stderr, 64 * 1024, None));
    let child = Arc::new(Mutex::new(ChildGuard(child)));
    let watchdog = Watchdog::start(child.clone(), program, timeout, watch);
    let produced = produce(&mut stdin);
    drop(stdin);
    let status = loop {
        if let Some(status) = child.lock().expect("child lock").0.try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(20));
    };
    watchdog.finish();
    let _ = out_thread.join();
    let (err, _) = err_thread
        .join()
        .map_err(|_| error("TOOL_FAILED", "Diagnostic reader failed"))?;
    if let Some(failure) = watchdog.failure() {
        return Err(failure);
    }
    if !status.success() {
        return Err(error(
            "TOOL_FAILED",
            String::from_utf8_lossy(&err).into_owned(),
        ));
    }
    // A producer failure (e.g. a composition error) wins over a broken pipe from the tool.
    produced
}

/// A tool whose stdout is read incrementally (e.g. decoded frames), so large outputs never reach disk.
/// A watchdog kills the tool on timeout; `finish` checks its exit status and bounded diagnostics.
pub(crate) struct StreamReader {
    child: std::sync::Arc<std::sync::Mutex<ChildGuard>>,
    stdout: std::io::BufReader<std::process::ChildStdout>,
    errors: Option<thread::JoinHandle<(Vec<u8>, bool)>>,
    watchdog: Watchdog,
    program: String,
}
impl StreamReader {
    pub(crate) fn spawn(program: &str, args: &[String], timeout: Duration) -> Result<Self> {
        let mut command = Command::new(program);
        command.args(args);
        Self::spawn_with(command, program, timeout, Watch::default())
    }
    /// `spawn` for a prepared command (for example from `Control::command`), with extra supervision.
    pub(crate) fn spawn_with(
        mut command: Command,
        program: &str,
        timeout: Duration,
        watch: Watch,
    ) -> Result<Self> {
        use std::sync::{Arc, Mutex};
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hidden(&mut command);
        let mut child = command
            .spawn()
            .map_err(|e| error("TOOL_UNAVAILABLE", format!("{program}: {e}")))?;
        let stdout = std::io::BufReader::with_capacity(
            4 * 1024 * 1024,
            child.stdout.take().expect("piped stdout"),
        );
        let stderr = child.stderr.take().expect("piped stderr");
        let errors = thread::spawn(move || drain(stderr, 64 * 1024, None));
        let child = Arc::new(Mutex::new(ChildGuard(child)));
        let watchdog = Watchdog::start(child.clone(), program, timeout, watch);
        Ok(Self {
            child,
            stdout,
            errors: Some(errors),
            watchdog,
            program: program.into(),
        })
    }
    /// Fill `buffer` exactly. A short stream reports the tool's timeout, cancellation or failed
    /// exit when it has one, and otherwise a validation failure.
    pub(crate) fn read_exact(&mut self, buffer: &mut [u8]) -> Result<()> {
        use std::io::Read;
        if self.stdout.read_exact(buffer).is_ok() {
            return Ok(());
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            match self.child.lock().expect("child lock").0.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {}
                _ => break None,
            }
            thread::sleep(Duration::from_millis(20));
        };
        self.watchdog.finish();
        if let Some(failure) = self.watchdog.failure() {
            return Err(failure);
        }
        if let Some(status) = status
            && !status.success()
        {
            let diagnostics = self
                .errors
                .take()
                .and_then(|reader| reader.join().ok())
                .map(|(err, _)| String::from_utf8_lossy(&err).into_owned())
                .unwrap_or_default();
            return Err(error("TOOL_FAILED", diagnostics));
        }
        Err(error(
            "RENDER_VALIDATION_FAILED",
            format!("{} produced fewer bytes than expected", self.program),
        ))
    }
    /// Require the tool to end cleanly with no unread output.
    pub(crate) fn finish(mut self) -> Result<()> {
        use std::io::Read;
        let mut extra = [0u8; 1];
        let trailing = self.stdout.read(&mut extra).unwrap_or(0);
        let status = loop {
            if let Some(status) = self.child.lock().expect("child lock").0.try_wait()? {
                break status;
            }
            thread::sleep(Duration::from_millis(20));
        };
        self.watchdog.finish();
        let (err, _) = self
            .errors
            .take()
            .expect("diagnostic reader")
            .join()
            .map_err(|_| error("TOOL_FAILED", "Diagnostic reader failed"))?;
        if let Some(failure) = self.watchdog.failure() {
            return Err(failure);
        }
        if !status.success() {
            return Err(error(
                "TOOL_FAILED",
                String::from_utf8_lossy(&err).into_owned(),
            ));
        }
        if trailing != 0 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                format!("{} produced more output than expected", self.program),
            ));
        }
        Ok(())
    }
}

pub(crate) fn input_root(root: &Path) -> Result<PathBuf> {
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Input root must be an existing absolute directory",
        ));
    }
    Ok(root.canonicalize()?)
}
pub fn allowed_file(path: &Path, root: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || !root.is_absolute() {
        return Err(error(
            "INVALID_PATH",
            "Input file and root must be absolute paths",
        ));
    }
    let root = input_root(root)?;
    let path = path.canonicalize()?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            "Input must be a file inside the allowed root",
        ));
    }
    Ok(path)
}

/// Project media can be absolute or use normal components under the caller's root.
/// This does not relax the absolute-path contract of explicit file commands.
pub(crate) fn project_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.to_string_lossy().contains('\0')
        || (!path.is_absolute()
            && (path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
                || path.to_string_lossy().contains(':')))
    {
        return Err(error(
            "INVALID_PATH",
            "Project media requires an absolute path or relative normal components",
        ));
    }
    Ok(())
}

pub(crate) fn project_file(path: &Path, root: &Path) -> Result<PathBuf> {
    project_path(path)?;
    allowed_file(
        &if path.is_absolute() {
            path.into()
        } else {
            root.join(path)
        },
        root,
    )
}

pub fn probe(path: &Path) -> Result<Value> {
    probe_controlled(path, &Uncontrolled)
}
pub fn probe_controlled(path: &Path, control: &dyn Control) -> Result<Value> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-show_streams".into(),
        "-show_format".into(),
        "-of".into(),
        "json".into(),
        path.to_string_lossy().into_owned(),
    ];
    Ok(serde_json::from_slice(&capture_controlled(
        &control.tool("ffprobe"),
        &args,
        Duration::from_secs(30),
        control,
    )?)?)
}

pub fn frame_info(path: &Path, selector: &str) -> Result<Value> {
    frame_info_controlled(path, selector, &Uncontrolled)
}
pub fn frame_info_controlled(path: &Path, selector: &str, control: &dyn Control) -> Result<Value> {
    frame_info_with_budget(path, selector, None, Duration::from_secs(120), control)
}

/// Stream/format metadata and packet timing from one `ffprobe` run that decodes nothing.
pub(crate) struct PacketInspection {
    /// Identical to `probe` output.
    pub metadata: Value,
    /// The first video stream's packets, shaped like `frame_info` entries.
    pub video: Vec<Value>,
    /// The first audio stream's packets with PCM16 sample counts (bytes / (2 x channels)).
    pub audio: Vec<Value>,
}

/// Packets equal frames only for codecs without reordering or empty packets: callers must check
/// for FFV1 video and PCM s16 audio before relying on `video`/`audio` timing or sample counts.
pub(crate) fn packet_inspection_controlled(
    path: &Path,
    control: &dyn Control,
) -> Result<PacketInspection> {
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-show_streams",
        "-show_format",
        "-show_packets",
        "-show_entries",
        "packet=stream_index,pts_time,size",
        "-of",
        "json",
    ]
    .map(str::to_owned)
    .into_iter()
    .chain([path.to_string_lossy().into_owned()])
    .collect::<Vec<_>>();
    let mut metadata: Value = serde_json::from_slice(&capture_controlled(
        &control.tool("ffprobe"),
        &args,
        Duration::from_secs(120),
        control,
    )?)?;
    let packets = metadata
        .as_object_mut()
        .and_then(|o| o.remove("packets"))
        .unwrap_or_default();
    // Listing packets adds a per-stream counter that a plain probe does not report.
    for stream in metadata["streams"].as_array_mut().into_iter().flatten() {
        if let Some(fields) = stream.as_object_mut() {
            fields.remove("nb_read_packets");
        }
    }
    let first = |kind: &str| {
        metadata["streams"]
            .as_array()
            .and_then(|s| s.iter().find(|s| s["codec_type"] == kind))
            .cloned()
    };
    let (video_stream, audio_stream) = (first("video"), first("audio"));
    let pcm16_frame_bytes = audio_stream
        .as_ref()
        .filter(|a| a["codec_name"] == "pcm_s16le")
        .and_then(|a| a["channels"].as_u64())
        .map(|channels| channels * 2)
        .filter(|&bytes| bytes > 0);
    let (mut video, mut audio) = (Vec::new(), Vec::new());
    for packet in packets.as_array().into_iter().flatten() {
        let index = packet["stream_index"].as_u64();
        if index.is_some() && index == video_stream.as_ref().and_then(|s| s["index"].as_u64()) {
            video.push(serde_json::json!({"best_effort_timestamp_time": packet["pts_time"]}));
        } else if index.is_some()
            && index == audio_stream.as_ref().and_then(|s| s["index"].as_u64())
        {
            let size = packet["size"]
                .as_u64()
                .or_else(|| packet["size"].as_str().and_then(|s| s.parse().ok()));
            let samples = pcm16_frame_bytes
                .zip(size)
                .filter(|(bytes, size)| size % bytes == 0)
                .map(|(bytes, size)| size / bytes);
            audio.push(serde_json::json!({"best_effort_timestamp_time": packet["pts_time"], "nb_samples": samples}));
        }
    }
    Ok(PacketInspection {
        metadata,
        video,
        audio,
    })
}

pub(crate) fn frame_info_with_budget(
    path: &Path,
    selector: &str,
    threads: Option<&str>,
    timeout: Duration,
    control: &dyn Control,
) -> Result<Value> {
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-select_streams".into(),
        selector.into(),
        "-show_frames".into(),
        "-show_entries".into(),
        "frame=best_effort_timestamp_time,nb_samples".into(),
        "-of".into(),
        "json".into(),
        path.to_string_lossy().into_owned(),
    ];
    if let Some(threads) = threads {
        args.splice(0..0, ["-threads".into(), threads.into()]);
    }
    Ok(serde_json::from_slice(&capture_controlled(
        &control.tool("ffprobe"),
        &args,
        timeout,
        control,
    )?)?)
}

pub fn file_hash(path: &Path) -> Result<String> {
    file_hash_controlled(path, &Uncontrolled)
}
pub fn file_hash_controlled(path: &Path, control: &dyn Control) -> Result<String> {
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65536];
    loop {
        control.check()?;
        let count = input.read(&mut buf)?;
        if count == 0 {
            break;
        }
        hash.update(&buf[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn version(name: &str) -> Result<String> {
    version_controlled(name, &Uncontrolled)
}
pub fn version_controlled(name: &str, control: &dyn Control) -> Result<String> {
    let bytes = capture_controlled(
        &control.tool(name),
        &["-version".into()],
        Duration::from_secs(10),
        control,
    )?;
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .next()
        .unwrap_or_default()
        .into())
}
