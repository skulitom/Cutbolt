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

/// Cancellation/progress hooks are shared by synchronous rendering and persisted jobs.
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
        Ok(())
    }
    fn phase(&self, _phase: &str) -> Result<()> {
        self.check()
    }
    fn frames(&self, _frames: u64) -> Result<()> {
        self.check()
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
