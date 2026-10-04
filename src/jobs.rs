//! Persisted local render queue. One worker owns a store's OS file lock.
use crate::{
    Result, error,
    media::{self, Control},
    model::Project,
    render,
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
mod recovery;

/// Retry policy for a queued render.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Retry {
    /// Total attempts including the first, 1 to 3. Only transient tool failure or interruption retries.
    pub max_attempts: u32,
}

/// Background reference render request queued by render.start.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderRequest {
    /// Project to render; the job pins its snapshot at submission.
    #[schemars(with = "crate::reference::ProjectInput")]
    pub project: Project,
    /// Existing absolute directory that project media paths resolve against.
    pub input_root: PathBuf,
    /// Existing absolute directory that must contain `output`.
    pub output_root: PathBuf,
    /// Unused absolute `.mkv` path inside `output_root`, not reserved by another active job.
    pub output: PathBuf,
    /// Attempt policy; omit for a single attempt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}
#[derive(Serialize, Deserialize)]
struct SavedRequest {
    render: RenderRequest,
    ffmpeg: String,
    ffprobe: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ffmpeg_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ffprobe_sha256: Option<String>,
}

/// A queued long-running command other than a reference render. It has no publication
/// recovery of its own: the command publishes its output atomically, and an interrupted run is
/// reported rather than retried.
#[derive(Serialize, Deserialize)]
struct SavedCommand {
    command: Value,
    ffmpeg: String,
    ffprobe: String,
    ffmpeg_sha256: String,
    ffprobe_sha256: String,
}

/// Commands that job.start can queue: each writes new files and can take minutes.
pub const QUEUED_COMMANDS: [&str; 11] = [
    "export.run",
    "media.conform",
    "scene.render",
    "audio.render",
    "audio.repair.render",
    "hdr.conform",
    "image.sequence.compile",
    "proxy.generate",
    "preview.range",
    "cache.run",
    "transcript.transcribe",
];

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn root_path(root: &Path) -> Result<PathBuf> {
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Job root must be an existing absolute local directory",
        ));
    }
    Ok(root.canonicalize()?)
}
fn safe_file(root: &Path, name: &str) -> Result<PathBuf> {
    let path = root.join(name);
    if let Ok(metadata) = path.symlink_metadata()
        && (!metadata.is_file()
            || metadata.file_type().is_symlink()
            || path.canonicalize()? != path)
    {
        return Err(error(
            "INVALID_PATH",
            "Job storage must use regular files directly inside its root",
        ));
    }
    Ok(path)
}
fn connect(root: &Path, create: bool) -> Result<Connection> {
    let root = root_path(root)?;
    let path = safe_file(&root, "jobs.sqlite3")?;
    if !create && !path.exists() {
        return Err(error("STORE_NOT_FOUND", "This root has no job store"));
    }
    let mut connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "synchronous", "EXTRA")?;
    let journal: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if journal != "delete" {
        return Err(error(
            "UNSUPPORTED_STORE",
            "Job store requires DELETE journaling",
        ));
    }
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let app: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let mut version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if create && app == 0 && version == 0 {
        let count: i64 = tx.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if count != 0 {
            return Err(error(
                "UNSUPPORTED_STORE",
                "Refusing to initialize an unrelated job database",
            ));
        }
        tx.execute_batch("CREATE TABLE jobs (
            sequence INTEGER PRIMARY KEY, id TEXT UNIQUE NOT NULL, request_id TEXT UNIQUE NOT NULL,
            payload_hash TEXT NOT NULL, request TEXT NOT NULL, request_hash TEXT NOT NULL, ticket TEXT NOT NULL,
            output TEXT NOT NULL, status TEXT NOT NULL, phase TEXT NOT NULL, cancel_requested INTEGER NOT NULL DEFAULT 0,
            frames INTEGER NOT NULL DEFAULT 0, total_frames INTEGER NOT NULL, result TEXT, failure TEXT, worker_pid INTEGER);
            CREATE UNIQUE INDEX active_output ON jobs(output) WHERE status IN ('queued','running');")?;
        tx.pragma_update(None, "application_id", 0x43554A31i64)?;
        tx.pragma_update(None, "user_version", 1)?;
        version = 1;
    } else if app != 0x43554A31 || !matches!(version, 1 | 2) {
        return Err(error(
            "UNSUPPORTED_STORE",
            "Unrecognized job store or unsupported version",
        ));
    }
    if version == 1 {
        recovery::migrate(&tx)?;
        tx.pragma_update(None, "user_version", 2)?;
    }
    tx.commit()?;
    Ok(connection)
}
fn lock_file(root: &Path) -> Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(safe_file(&root_path(root)?, "worker.lock")?)?)
}
fn try_lock(file: &File) -> Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}
fn reconcile(connection: &Connection) -> Result<()> {
    recovery::reconcile(connection)
}
fn recover_if_idle(root: &Path, connection: &Connection) -> Result<()> {
    let lock = lock_file(root)?;
    if try_lock(&lock)? {
        reconcile(connection)?;
    }
    Ok(())
}
fn supported() -> Result<()> {
    if cfg!(windows) {
        Ok(())
    } else {
        Err(error(
            "UNSUPPORTED_PLATFORM",
            "Background jobs currently require Windows; synchronous rendering and MCP remain available",
        ))
    }
}
fn resolve_tool(name: &str) -> Result<String> {
    let selected = media::tool(name);
    let path = Path::new(&selected);
    if path.is_absolute() || path.components().count() > 1 {
        return Ok(path
            .canonicalize()
            .map_err(|e| error("TOOL_UNAVAILABLE", format!("{name}: {e}")))?
            .to_string_lossy()
            .into_owned());
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for candidate in [
            directory.join(&selected),
            directory.join(format!("{selected}.exe")),
        ] {
            if candidate.is_file() {
                return Ok(candidate.canonicalize()?.to_string_lossy().into_owned());
            }
        }
    }
    Err(error(
        "TOOL_UNAVAILABLE",
        format!(
            "Cannot find local {name}; configure CUTBOLT_{}",
            name.to_uppercase()
        ),
    ))
}
fn kick(root: &Path) -> Result<()> {
    supported()?;
    let executable = std::env::var_os("CUTBOLT_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    launch_worker(&executable, &root_path(root)?).map_err(|e| {
        error(
            "JOB_LAUNCH_FAILED",
            format!("Queue saved; retry submission or job.resume: {e}"),
        )
    })?;
    Ok(())
}

#[cfg(windows)]
fn launch_worker(executable: &Path, root: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::{Foundation::CloseHandle, System::Threading::*};
    fn quoted(path: &Path) -> std::io::Result<Vec<u16>> {
        let units: Vec<_> = path.as_os_str().encode_wide().collect();
        if units.iter().any(|v| matches!(v, 0 | 34)) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "Invalid worker path",
            ));
        }
        let mut value = vec![34];
        value.extend(&units);
        // Double trailing backslashes before the closing quote in a Windows argument.
        value.extend(std::iter::repeat_n(
            92,
            units.iter().rev().take_while(|v| **v == 92).count(),
        ));
        value.push(34);
        Ok(value)
    }
    let executable = executable.canonicalize()?;
    let mut command = quoted(&executable)?;
    command.extend(" job-worker ".encode_utf16());
    command.extend(quoted(root)?);
    command.push(0);
    let application: Vec<_> = executable.as_os_str().encode_wide().chain([0]).collect();
    // A detached job must inherit no caller handles, including any extra pipe
    // handles beyond the three standard handles. Otherwise a CLI reader can
    // wait for EOF until the background render exits.
    unsafe {
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            ..std::mem::zeroed()
        };
        let mut process: PROCESS_INFORMATION = std::mem::zeroed();
        if CreateProcessW(
            application.as_ptr(),
            command.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            CREATE_NO_WINDOW,
            std::ptr::null(),
            std::ptr::null(),
            &startup,
            &mut process,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        CloseHandle(process.hThread);
        CloseHandle(process.hProcess);
    }
    Ok(())
}

#[cfg(not(windows))]
fn launch_worker(_executable: &Path, _root: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "Background workers require Windows",
    ))
}

/// The same request ID and arguments always return the original submission ticket.
pub fn start(root: &Path, request_id: &str, request: RenderRequest) -> Result<Value> {
    supported()?;
    let max_attempts = request.retry.as_ref().map_or(1, |v| v.max_attempts);
    if !(1..=3).contains(&max_attempts) {
        return Err(error("INVALID_RETRY", "Total attempts must be 1..3"));
    }
    if request_id.trim().is_empty() || request_id.len() > 128 {
        return Err(error("INVALID_ID", "Request ID requires 1-128 bytes"));
    }
    let payload_hash = digest(&serde_json::to_vec(&request)?);
    let mut connection = connect(root, true)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<(String, String, String)> = tx
        .query_row(
            "SELECT payload_hash,ticket,status FROM jobs WHERE request_id=?1",
            [request_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((hash, ticket, state)) = existing {
        if hash != payload_hash {
            return Err(error(
                "REQUEST_ID_CONFLICT",
                "Job request ID was already used with different arguments",
            ));
        }
        tx.commit()?;
        if state == "queued" {
            kick(root)?;
        }
        return Ok(serde_json::from_str(&ticket)?);
    }
    request.project.validate()?;
    if !request.input_root.is_absolute() || !request.input_root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Input root must be an existing absolute directory",
        ));
    }
    let output = render::destination(&request.output, &request.output_root)?;
    let total = request
        .project
        .duration()?
        .units(request.project.frame_rate)?;
    let total =
        i64::try_from(total).map_err(|_| error("LIMIT_EXCEEDED", "Frame count too large"))?;
    let active: i64 = tx.query_row(
        "SELECT count(*) FROM jobs WHERE status IN ('queued','running')",
        [],
        |r| r.get(0),
    )?;
    if active >= 32 {
        return Err(error(
            "QUEUE_FULL",
            "At most 32 active jobs per root; wait or cancel a queued job",
        ));
    }
    let reserved: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE output=?1 AND status IN ('queued','running'))",
        [output.to_string_lossy().as_ref()],
        |r| r.get(0),
    )?;
    if reserved {
        return Err(error(
            "OUTPUT_RESERVED",
            "An active job already reserves this output",
        ));
    }
    let id = digest(request_id.as_bytes());
    let ticket = json!({"job_id":id,"project_id":request.project.id,"project_revision":request.project.revision});
    let mut request = request;
    request.input_root = request.input_root.canonicalize()?;
    request.output_root = request.output_root.canonicalize()?;
    request.output = output.clone();
    let ffmpeg = resolve_tool("ffmpeg")?;
    let ffprobe = resolve_tool("ffprobe")?;
    let saved = SavedRequest {
        render: request,
        ffmpeg_sha256: Some(media::file_hash(Path::new(&ffmpeg))?),
        ffprobe_sha256: Some(media::file_hash(Path::new(&ffprobe))?),
        ffmpeg,
        ffprobe,
    };
    let encoded = serde_json::to_string(&saved)?;
    tx.execute("INSERT INTO jobs(id,request_id,payload_hash,request,request_hash,ticket,output,status,phase,total_frames,max_attempts)
        VALUES(?1,?2,?3,?4,?5,?6,?7,'queued','queued',?8,?9)",params![id,request_id,payload_hash,encoded,digest(encoded.as_bytes()),ticket.to_string(),output.to_string_lossy(),total,max_attempts])?;
    tx.commit()?;
    kick(root)?;
    Ok(ticket)
}

/// Queue one long-running command, given as its complete request, in the background. The same
/// request ID and request always return the original ticket.
pub fn start_command(root: &Path, request_id: &str, request: Value) -> Result<Value> {
    supported()?;
    let command = request["command"].as_str().unwrap_or_default().to_owned();
    if !QUEUED_COMMANDS.contains(&command.as_str()) {
        return Err(error(
            "UNSUPPORTED_JOB",
            format!(
                "{command:?} cannot be queued; job.start runs {}. Use render.start for reference renders",
                QUEUED_COMMANDS.join(", ")
            ),
        ));
    }
    if request_id.trim().is_empty() || request_id.len() > 128 {
        return Err(error("INVALID_ID", "Request ID requires 1-128 bytes"));
    }
    let payload_hash = digest(&serde_json::to_vec(&request)?);
    let mut connection = connect(root, true)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<(String, String, String)> = tx
        .query_row(
            "SELECT payload_hash,ticket,status FROM jobs WHERE request_id=?1",
            [request_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    if let Some((hash, ticket, state)) = existing {
        if hash != payload_hash {
            return Err(error(
                "REQUEST_ID_CONFLICT",
                "Job request ID was already used with different arguments",
            ));
        }
        tx.commit()?;
        if state == "queued" {
            kick(root)?;
        }
        return Ok(serde_json::from_str(&ticket)?);
    }
    let id = digest(request_id.as_bytes());
    // Commands without an output file reserve their job instead.
    let output = request["output"]
        .as_str()
        .map_or_else(|| format!("job:{id}"), str::to_owned);
    let active: i64 = tx.query_row(
        "SELECT count(*) FROM jobs WHERE status IN ('queued','running')",
        [],
        |r| r.get(0),
    )?;
    if active >= 32 {
        return Err(error(
            "QUEUE_FULL",
            "At most 32 active jobs per root; wait or cancel a queued job",
        ));
    }
    let reserved: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE output=?1 AND status IN ('queued','running'))",
        [&output],
        |r| r.get(0),
    )?;
    if reserved {
        return Err(error(
            "OUTPUT_RESERVED",
            format!("An active job already reserves {output}"),
        ));
    }
    let ticket = json!({"job_id":id,"command":command});
    let ffmpeg = resolve_tool("ffmpeg")?;
    let ffprobe = resolve_tool("ffprobe")?;
    let saved = SavedCommand {
        command: request,
        ffmpeg_sha256: media::file_hash(Path::new(&ffmpeg))?,
        ffprobe_sha256: media::file_hash(Path::new(&ffprobe))?,
        ffmpeg,
        ffprobe,
    };
    let encoded = serde_json::to_string(&saved)?;
    tx.execute("INSERT INTO jobs(id,request_id,payload_hash,request,request_hash,ticket,output,status,phase,total_frames,max_attempts)
        VALUES(?1,?2,?3,?4,?5,?6,?7,'queued','queued',0,1)",params![id,request_id,payload_hash,encoded,digest(encoded.as_bytes()),ticket.to_string(),output])?;
    tx.commit()?;
    kick(root)?;
    Ok(ticket)
}

/// Run a queued command in the worker and record its result.
fn run_command(connection: &Connection, id: &str, encoded: &str) -> Result<()> {
    let saved: SavedCommand = serde_json::from_str(encoded)?;
    for (path, expected) in [
        (&saved.ffmpeg, &saved.ffmpeg_sha256),
        (&saved.ffprobe, &saved.ffprobe_sha256),
    ] {
        if media::file_hash(Path::new(path))? != *expected {
            return Err(error(
                "TOOL_CHANGED",
                "A queued media tool changed after submission",
            ));
        }
    }
    connection.execute("UPDATE jobs SET phase='running' WHERE id=?1", [id])?;
    let result = crate::commands::handle(serde_json::from_value(saved.command)?)?;
    let tx = connection.unchecked_transaction()?;
    tx.execute(
        "UPDATE jobs SET status='completed',phase='completed',result=?1 WHERE id=?2",
        params![result.to_string(), id],
    )?;
    recovery::complete_attempt(&tx, id)?;
    tx.commit()?;
    Ok(())
}

/// Wait up to `seconds` for a job to finish, then report its status either way.
pub fn wait(root: &Path, id: &str, seconds: u32) -> Result<Value> {
    if !(1..=120).contains(&seconds) {
        return Err(error(
            "INVALID_ARGUMENT",
            "timeout_seconds must be 1 to 120",
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(seconds.into());
    loop {
        let state = status(root, id)?;
        let done = matches!(
            state["status"].as_str(),
            Some("completed" | "failed" | "cancelled" | "interrupted")
        );
        if done || Instant::now() >= deadline {
            let mut state = state;
            state["finished"] = json!(done);
            return Ok(state);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

type JobRow = (
    String,
    String,
    bool,
    i64,
    i64,
    Option<String>,
    Option<String>,
    String,
    Option<i64>,
);

fn state(connection: &Connection, id: &str) -> Result<Value> {
    let row:Option<JobRow>=connection.query_row(
        "SELECT status,phase,cancel_requested,frames,total_frames,result,failure,ticket,worker_pid FROM jobs WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))).optional()?;
    let Some((status, phase, cancelled, frames, total, result, failure, ticket, worker_pid)) = row
    else {
        let count: i64 = connection.query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))?;
        let first = connection
            .prepare("SELECT id FROM jobs ORDER BY id LIMIT ?1")?
            .query_map([crate::LISTED_IDS as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        return Err(crate::missing_listed(
            "JOB_NOT_FOUND",
            "job",
            id,
            &first,
            count as usize,
        ));
    };
    let parse = |s: Option<String>| -> Result<Value> {
        Ok(s.map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or(Value::Null))
    };
    Ok(
        json!({"ticket":serde_json::from_str::<Value>(&ticket)?,"status":status,"cancel_requested":cancelled,
        "worker_pid":worker_pid,"progress":{"phase":phase,"frames":frames,"total_frames":total},"result":parse(result)?,"error":parse(failure)?,"attempts":recovery::attempts(connection,id)?}),
    )
}
pub fn status(root: &Path, id: &str) -> Result<Value> {
    let connection = connect(root, false)?;
    recover_if_idle(root, &connection)?;
    state(&connection, id)
}
pub fn cancel(root: &Path, id: &str) -> Result<Value> {
    let mut connection = connect(root, false)?;
    recover_if_idle(root, &connection)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    state(&tx, id)?;
    tx.execute(
        "UPDATE jobs SET cancel_requested=1,
        status=CASE WHEN status='queued' THEN 'cancelled' ELSE status END,
        phase=CASE WHEN status='queued' THEN 'cancelled' ELSE phase END
        WHERE id=?1 AND status IN ('queued','running')",
        [id],
    )?;
    let result = state(&tx, id)?;
    tx.commit()?;
    Ok(result)
}
pub fn resume(root: &Path) -> Result<Value> {
    let connection = connect(root, false)?;
    recover_if_idle(root, &connection)?;
    kick(root)?;
    Ok(
        json!({"started":true,"note":"Queued jobs and opted-in interrupted retries will run. Completed publication is reconciled from its saved identity; attempt limits and cancellation remain in force."}),
    )
}

struct JobControl<'a> {
    connection: &'a Connection,
    id: &'a str,
    checked: Cell<Instant>,
    ffmpeg: &'a str,
    ffprobe: &'a str,
}
impl JobControl<'_> {
    fn cancellation(&self) -> Result<()> {
        let cancelled: bool = self.connection.query_row(
            "SELECT cancel_requested FROM jobs WHERE id=?1",
            [self.id],
            |r| r.get(0),
        )?;
        if cancelled {
            Err(error(
                "JOB_CANCELLED",
                "Job cancelled; no output was published",
            ))
        } else {
            Ok(())
        }
    }
}
impl Control for JobControl<'_> {
    fn sources(&self, sources: &[render::Source]) -> Result<()> {
        self.cancellation()?;
        recovery::pin_sources(self.connection, self.id, sources)
    }
    fn command(&self, program: &str, args: &[String]) -> Result<Command> {
        let mut command = Command::new(std::env::current_exe()?);
        command.arg("job-tool").arg(program).args(args);
        Ok(command)
    }
    fn tool(&self, name: &str) -> String {
        if name == "ffmpeg" {
            self.ffmpeg.into()
        } else {
            self.ffprobe.into()
        }
    }
    fn check(&self) -> Result<()> {
        if self.checked.get().elapsed() >= Duration::from_millis(100) {
            self.cancellation()?;
            self.checked.set(Instant::now());
        }
        Ok(())
    }
    fn phase(&self, phase: &str) -> Result<()> {
        self.cancellation()?;
        self.connection.execute(
            "UPDATE jobs SET phase=?1 WHERE id=?2",
            params![phase, self.id],
        )?;
        Ok(())
    }
    fn frames(&self, frames: u64) -> Result<()> {
        self.cancellation()?;
        self.connection.execute(
            "UPDATE jobs SET frames=min(total_frames,max(frames,?1)) WHERE id=?2",
            params![i64::try_from(frames).unwrap_or(i64::MAX), self.id],
        )?;
        Ok(())
    }
    fn publish(&self, temp: &Path, output: &Path, receipt: &Value) -> Result<()> {
        // Persist the validated receipt before filesystem publication. Recovery can
        // distinguish a committed output from an unrelated or modified file.
        recovery::publication_intent(self.connection, self.id, temp, output, receipt)?;
        #[cfg(test)]
        recovery::crash_point("intent");
        // Serialize cancellation with the final publication decision.
        let tx = self.connection.unchecked_transaction()?;
        // Reserve the writer before checking cancellation (a deferred read-to-write upgrade is unsafe).
        tx.execute("UPDATE jobs SET phase='publishing' WHERE id=?1", [self.id])?;
        self.cancellation()?;
        media::publish(temp, output)?;
        #[cfg(test)]
        recovery::crash_point("published");
        tx.execute("UPDATE jobs SET status='completed',phase='completed',frames=total_frames,result=?1 WHERE id=?2",params![receipt.to_string(),self.id])?;
        recovery::complete_attempt(&tx, self.id)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(windows)]
fn contain_worker() -> Result<()> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{JobObjects::*, Threading::GetCurrentProcess},
    };
    // The process keeps this non-inheritable handle until exit. Windows then terminates
    // all remaining FFmpeg/ffprobe descendants, including on abrupt worker termination.
    unsafe {
        let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if handle.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        ) == 0
            || AssignProcessToJobObject(handle, GetCurrentProcess()) == 0
        {
            let err = std::io::Error::last_os_error();
            CloseHandle(handle);
            return Err(err.into());
        }
        // Intentionally retained: closing this handle while the worker runs kills it.
    }
    Ok(())
}
#[cfg(not(windows))]
fn contain_worker() -> Result<()> {
    supported()
}

pub fn worker(root: &Path) -> Result<()> {
    let mut connection = connect(root, false)?;
    if let Err(failure) = contain_worker() {
        connection.execute(
            "UPDATE jobs SET status='failed',phase='failed',failure=?1 WHERE status='queued'",
            [json!({"code":"WORKER_SETUP_FAILED","message":failure.message}).to_string()],
        )?;
        return Err(failure);
    }
    let lock = lock_file(root)?;
    let wait = Instant::now();
    while !try_lock(&lock)? {
        // A live owner drains all queued work. Briefly waiting also closes the idle-exit race.
        if wait.elapsed() > Duration::from_secs(2) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    reconcile(&connection)?;
    loop {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let row:Option<(String,String,String)>=tx.query_row("SELECT id,request,request_hash FROM jobs WHERE status='queued' ORDER BY sequence LIMIT 1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((id, encoded, hash)) = row else {
            return Ok(());
        };
        tx.execute(
            "UPDATE jobs SET status='running',phase='inspecting',worker_pid=?2,attempt=attempt+1,frames=0,failure=NULL WHERE id=?1",
            params![id, std::process::id()],
        )?;
        recovery::begin_attempt(&tx, &id)?;
        tx.commit()?;
        let outcome = (|| {
            if digest(encoded.as_bytes()) != hash {
                return Err(error("STORE_CORRUPT", "Job request checksum mismatch"));
            }
            if serde_json::from_str::<Value>(&encoded)?
                .get("command")
                .is_some()
            {
                return run_command(&connection, &id, &encoded).map(|()| Value::Null);
            }
            let saved: SavedRequest = serde_json::from_str(&encoded)?;
            for (path, expected) in [
                (&saved.ffmpeg, &saved.ffmpeg_sha256),
                (&saved.ffprobe, &saved.ffprobe_sha256),
            ] {
                if let Some(expected) = expected
                    && media::file_hash(Path::new(path))? != *expected
                {
                    return Err(error(
                        "TOOL_CHANGED",
                        "A queued media tool changed after submission",
                    ));
                }
            }
            let request = saved.render;
            let control = JobControl {
                connection: &connection,
                id: &id,
                checked: Cell::new(Instant::now() - Duration::from_secs(1)),
                ffmpeg: &saved.ffmpeg,
                ffprobe: &saved.ffprobe,
            };
            let attempt: i64 =
                connection
                    .query_row("SELECT attempt FROM jobs WHERE id=?1", [&id], |r| r.get(0))?;
            let temp = request
                .output
                .with_file_name(recovery::partial_name(&id, attempt));
            render::run_controlled(
                &request.project,
                &request.input_root,
                &request.output_root,
                &request.output,
                Some(&temp),
                &control,
            )
        })();
        if let Err(failure) = outcome {
            recovery::failed(&connection, &id, &failure)?;
        }
    }
}

/// Each media subprocess gets a nested lifetime scope before the external tool starts.
/// Killing this wrapper kills its entire tool tree, even while the queue worker continues.
pub fn tool_worker(program: &std::ffi::OsStr, args: &[std::ffi::OsString]) -> Result<i32> {
    contain_worker()?;
    let mut command = Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    Ok(command.status()?.code().unwrap_or(1))
}
