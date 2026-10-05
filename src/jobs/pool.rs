//! How one job root runs independent jobs at once: a bounded pool with resource classes, path
//! claims that keep conflicting jobs in submission order, and named events that wake the worker
//! and waiters at once instead of polling.
use super::*;

/// Environment variable that sets the pool size, 1 to 32 jobs.
const WORKERS_VARIABLE: &str = "CUTBOLT_JOB_WORKERS";

/// How many jobs one worker runs at once: `CUTBOLT_JOB_WORKERS`, or a quarter of the logical
/// processors from 1 to 8. A scene render keeps about three cores busy, so eight fill 32.
pub(super) fn workers() -> u32 {
    configured(
        std::env::var(WORKERS_VARIABLE).ok().as_deref(),
        thread::available_parallelism().map_or(1, |n| n.get()),
    )
}
fn configured(value: Option<&str>, processors: usize) -> u32 {
    value
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|n| (1..=32).contains(n))
        .unwrap_or_else(|| (processors / 4).clamp(1, 8) as u32)
}

/// Commands that hold one place in the pool. Every other queued command is heavy: an export,
/// conversion, review or reference render already runs multi-threaded FFmpeg or Rust compositing.
const LIGHT: [&str; 5] = [
    "scene.render",
    "audio.render",
    "audio.repair.render",
    "media.transcribe",
    "transcript.transcribe",
];

/// What a job needs from the pool.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Class {
    /// Holds half the pool (rounded up), so at most two heavy jobs run at once.
    pub heavy: bool,
    /// Loads speech models onto one device; speech jobs run one at a time.
    pub speech: bool,
    /// Normalized paths the job writes: its output (a file or folder) and any shared database.
    pub claims: Vec<String>,
}
impl Class {
    fn weight(&self, pool: u32) -> u32 {
        if self.heavy { pool.div_ceil(2) } else { 1 }
    }
}

/// The class of a saved job request.
pub(super) fn class(saved: &Value) -> Class {
    let Some(command) = saved.get("command") else {
        // A reference render queued by render.start.
        return Class {
            heavy: true,
            speech: false,
            claims: saved["render"]["output"]
                .as_str()
                .map(normalize)
                .into_iter()
                .collect(),
        };
    };
    let name = command["command"].as_str().unwrap_or_default();
    // Saved projects are pinned as snapshots at submission, so no queued command writes a
    // session; a store or cache database it names is claimed like an output.
    let mut claims: Vec<String> = [
        &command["output"],
        &command["cache_root"],
        &command["store_root"],
        &command["task"]["output"],
    ]
    .into_iter()
    .filter_map(Value::as_str)
    .map(normalize)
    .collect();
    if name == "media.prepare" && command["output"].is_null() {
        claims.extend(
            prepared_outputs(command)
                .iter()
                .map(|p| normalize(&p.to_string_lossy())),
        );
    }
    Class {
        heavy: !LIGHT.contains(&name),
        speech: matches!(name, "media.transcribe" | "transcript.transcribe")
            || (name == "export.review" && !command["runtime"].is_null()),
        claims,
    }
}

/// The files a media.prepare request without `output` may write.
fn prepared_outputs(command: &Value) -> Vec<PathBuf> {
    let path = |v: &Value| v.as_str().map(PathBuf::from);
    let (Some(input_root), Some(output_root)) =
        (path(&command["input_root"]), path(&command["output_root"]))
    else {
        return Vec::new();
    };
    let assets = command["project"]["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| a["id"].as_str().map(str::to_owned));
    match (path(&command["path"]), command["paths"].as_array()) {
        (Some(single), _) => vec![crate::readiness::prepared_output(&single, &output_root)],
        (None, Some(paths)) => {
            let paths: Vec<PathBuf> = paths.iter().filter_map(path).collect();
            crate::readiness::batch_names(&paths, &input_root, assets.collect())
                .into_iter()
                .map(|(_, id)| output_root.join(format!("{id}-prepared.mkv")))
                .collect()
        }
        _ => Vec::new(),
    }
}

/// A path as claims compare it: `/` separators, no verbatim prefix or trailing separator, and
/// case-folded on Windows, whose file names ignore case.
fn normalize(path: &str) -> String {
    let mut text = path.replace('\\', "/");
    if let Some(rest) = text.strip_prefix("//?/") {
        text = rest.to_owned();
    }
    while text.len() > 1 && text.ends_with('/') && !text.ends_with(":/") {
        text.pop();
    }
    if cfg!(windows) {
        text.to_lowercase()
    } else {
        text
    }
}

/// Whether two claimed paths are the same or one lies inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    let inside = |inner: &str, outer: &str| {
        inner.len() > outer.len()
            && inner.starts_with(outer)
            && (outer.ends_with('/') || inner.as_bytes()[outer.len()] == b'/')
    };
    a == b || inside(a, b) || inside(b, a)
}

/// Which queued jobs start now, given the running ones; `queued` is in submission order.
/// - A job whose claims overlap a running job's, or an earlier waiting job's, waits: jobs that
///   write the same path or database run one at a time, in submission order.
/// - A speech job waits while another speech job runs or waits ahead of it.
/// - Room is first come, first served: once a job waits for room, later ones wait behind it, so a
///   stream of light jobs never starves a heavy one. A job heavier than the pool runs alone.
pub(super) fn schedule<'a>(
    pool: u32,
    running: &[&Class],
    queued: &'a [(String, Class)],
) -> Vec<&'a str> {
    let mut used: u32 = running.iter().map(|c| c.weight(pool).min(pool)).sum();
    let mut claimed: Vec<&str> = running
        .iter()
        .flat_map(|c| c.claims.iter().map(String::as_str))
        .collect();
    let mut speech = running.iter().any(|c| c.speech);
    let mut started = Vec::new();
    for (id, class) in queued {
        let blocked = (class.speech && speech)
            || class
                .claims
                .iter()
                .any(|c| claimed.iter().any(|b| overlaps(c, b)));
        if !blocked {
            let weight = class.weight(pool).min(pool);
            if used + weight > pool {
                break;
            }
            used += weight;
            started.push(id.as_str());
        }
        claimed.extend(class.claims.iter().map(String::as_str));
        speech |= class.speech;
    }
    started
}

/// A named event shared by the processes of one job root, so the worker and waiters sleep until
/// something changes instead of polling. Every wait still has a timeout, so a lost or missing
/// signal only delays.
pub(super) struct Signal(#[cfg(windows)] windows_sys::Win32::Foundation::HANDLE);

/// The worker's event: set when a job is queued, finishes or is cancelled.
pub(super) fn root_signal(root: &Path) -> Option<Signal> {
    Signal::open(
        &format!("cutbolt-jobs-{}", &name_digest(root, "")[..32]),
        false,
    )
}
/// A job's event: set when the job reaches a terminal state or is queued again for a retry.
pub(super) fn job_signal(root: &Path, id: &str) -> Option<Signal> {
    Signal::open(
        &format!("cutbolt-job-{}", &name_digest(root, id)[..32]),
        true,
    )
}
fn name_digest(root: &Path, id: &str) -> String {
    let mut bytes = root.as_os_str().as_encoded_bytes().to_vec();
    bytes.push(0);
    bytes.extend_from_slice(id.as_bytes());
    digest(&bytes)
}
/// Wake the worker and everyone waiting for `id`.
pub(super) fn notify(root: &Path, id: &str) {
    for signal in [job_signal(root, id), root_signal(root)]
        .into_iter()
        .flatten()
    {
        signal.set();
    }
}

#[cfg(windows)]
impl Signal {
    fn open(name: &str, manual: bool) -> Option<Self> {
        use windows_sys::Win32::System::Threading::CreateEventW;
        let wide: Vec<u16> = format!("Local\\{name}").encode_utf16().chain([0]).collect();
        // SAFETY: a valid NUL-terminated name; the handle is owned and closed on drop.
        let handle = unsafe { CreateEventW(std::ptr::null(), i32::from(manual), 0, wide.as_ptr()) };
        (!handle.is_null()).then_some(Self(handle))
    }
    pub(super) fn set(&self) {
        // SAFETY: the handle is valid until drop.
        unsafe { windows_sys::Win32::System::Threading::SetEvent(self.0) };
    }
    pub(super) fn reset(&self) {
        // SAFETY: the handle is valid until drop.
        unsafe { windows_sys::Win32::System::Threading::ResetEvent(self.0) };
    }
    /// Wait until the event is set or `timeout` passes.
    pub(super) fn wait(&self, timeout: Duration) {
        let milliseconds = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX);
        // SAFETY: the handle is valid until drop.
        unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(self.0, milliseconds) };
    }
}
#[cfg(windows)]
impl Drop for Signal {
    fn drop(&mut self) {
        // SAFETY: closing the owned handle.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}
#[cfg(not(windows))]
impl Signal {
    fn open(_name: &str, _manual: bool) -> Option<Self> {
        None
    }
    pub(super) fn set(&self) {}
    pub(super) fn reset(&self) {}
    pub(super) fn wait(&self, timeout: Duration) {
        thread::sleep(timeout);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(heavy: bool, speech: bool, claims: &[&str]) -> Class {
        Class {
            heavy,
            speech,
            claims: claims.iter().map(|c| normalize(c)).collect(),
        }
    }
    fn queue(jobs: &[(&str, Class)]) -> Vec<(String, Class)> {
        jobs.iter()
            .map(|(id, c)| (id.to_string(), c.clone()))
            .collect()
    }

    #[test]
    fn pool_size_defaults_to_a_quarter_of_the_processors_and_accepts_an_override() {
        assert_eq!(configured(None, 32), 8);
        assert_eq!(configured(None, 64), 8);
        assert_eq!(configured(None, 8), 2);
        assert_eq!(configured(None, 2), 1);
        assert_eq!(configured(Some("12"), 32), 12);
        assert_eq!(configured(Some(" 1 "), 32), 1);
        for ignored in ["0", "33", "-1", "four", ""] {
            assert_eq!(configured(Some(ignored), 32), 8, "{ignored:?}");
        }
    }

    #[test]
    fn independent_light_jobs_fill_the_pool_in_order() {
        let queued = queue(&[
            ("a", job(false, false, &["C:/w/a.mkv"])),
            ("b", job(false, false, &["C:/w/b.mkv"])),
            ("c", job(false, false, &["C:/w/c.mkv"])),
        ]);
        assert_eq!(schedule(8, &[], &queued), ["a", "b", "c"]);
        assert_eq!(schedule(2, &[], &queued), ["a", "b"]);
        let running = job(false, false, &["C:/w/r.mkv"]);
        assert_eq!(schedule(2, &[&running], &queued), ["a"]);
        assert_eq!(schedule(1, &[&running], &queued), Vec::<&str>::new());
    }

    #[test]
    fn heavy_jobs_hold_half_the_pool_and_wait_first_come_first_served() {
        let heavy = |p: &str| job(true, false, &[p]);
        let light = |p: &str| job(false, false, &[p]);
        let queued = queue(&[
            ("e1", heavy("C:/w/e1.mp4")),
            ("e2", heavy("C:/w/e2.mp4")),
            ("e3", heavy("C:/w/e3.mp4")),
            ("s1", light("C:/w/s1.mkv")),
        ]);
        // Two heavy jobs fill a pool of 8; the third waits, and the light job waits behind it.
        assert_eq!(schedule(8, &[], &queued), ["e1", "e2"]);
        // Seven light jobs leave one place: the heavy job at the head waits for four.
        let lights: Vec<Class> = (0..7).map(|n| light(&format!("C:/w/r{n}.mkv"))).collect();
        let running: Vec<&Class> = lights.iter().collect();
        assert_eq!(schedule(8, &running, &queued), Vec::<&str>::new());
        // A pool of one runs everything one at a time, heavy or not.
        assert_eq!(schedule(1, &[], &queued), ["e1"]);
        assert_eq!(schedule(3, &[], &queued), ["e1"]);
    }

    #[test]
    fn jobs_writing_the_same_path_keep_submission_order() {
        let queued = queue(&[
            ("first", job(false, false, &["C:/w/out/Shot.mkv"])),
            ("second", job(false, false, &["c:\\w\\out\\shot.mkv"])),
            ("other", job(false, false, &["C:/w/out/other.mkv"])),
        ]);
        let started = schedule(8, &[], &queued);
        if cfg!(windows) {
            // Windows file names ignore case, so both name one file: the second waits.
            assert_eq!(started, ["first", "other"]);
            let running = queued[0].1.clone();
            assert_eq!(schedule(8, &[&running], &queued[1..]), ["other"]);
        }
        // A folder output (export.review) conflicts with any path inside it, and a waiting job
        // holds its claims, so a later job on the same path cannot overtake it.
        let review = job(true, false, &["C:/w/review"]);
        let queued = queue(&[
            ("inside", job(false, false, &["C:/w/review/sheet.png"])),
            ("sibling", job(false, false, &["C:/w/reviewed.mkv"])),
            ("again", job(false, false, &["C:/w/review/sheet.png"])),
        ]);
        assert_eq!(schedule(8, &[&review], &queued), ["sibling"]);
        assert!(overlaps("c:/w", "c:/w/x") && !overlaps("c:/w/a", "c:/w/ab"));
        assert!(overlaps("c:/", "c:/w") && overlaps(&normalize("C:\\w\\"), "c:/w/x"));
        assert_eq!(normalize(r"\\?\C:\W\Out\"), normalize("C:/W/Out"));
    }

    #[test]
    fn speech_jobs_run_one_at_a_time_without_holding_up_others() {
        let queued = queue(&[
            ("words1", job(false, true, &["C:/w/a.json"])),
            ("words2", job(false, true, &["C:/w/b.json"])),
            ("scene", job(false, false, &["C:/w/s.mkv"])),
        ]);
        assert_eq!(schedule(8, &[], &queued), ["words1", "scene"]);
        let running = job(true, true, &["C:/w/review"]);
        assert_eq!(schedule(8, &[&running], &queued), ["scene"]);
    }

    #[test]
    fn classes_follow_the_saved_request() {
        let render = json!({"render":{"output":"C:\\w\\out.mkv"},"ffmpeg":"f","ffprobe":"p"});
        assert_eq!(class(&render), job(true, false, &["C:/w/out.mkv"]));
        let command = |c: Value| class(&json!({"command": c}));
        assert_eq!(
            command(json!({"command":"scene.render","output":"C:/w/s.mkv"})),
            job(false, false, &["C:/w/s.mkv"])
        );
        assert_eq!(
            command(json!({"command":"export.run","output":"C:/w/e.mp4"})),
            job(true, false, &["C:/w/e.mp4"])
        );
        assert_eq!(
            command(json!({"command":"media.transcribe","output":"C:/w/t.json"})),
            job(false, true, &["C:/w/t.json"])
        );
        assert_eq!(
            command(json!({"command":"export.review","output":"C:/w/r","runtime":{"file":"x"}})),
            job(true, true, &["C:/w/r"])
        );
        assert_eq!(
            command(json!({"command":"export.review","output":"C:/w/r"})),
            job(true, false, &["C:/w/r"])
        );
        assert_eq!(
            command(
                json!({"command":"cache.run","cache_root":"C:/w/.cutbolt/cache",
                "task":{"type":"proxy","output":"C:/w/p.mkv"}})
            ),
            job(true, false, &["C:/w/.cutbolt/cache", "C:/w/p.mkv"])
        );
        assert_eq!(
            command(json!({"command":"transcript.transcribe","scratch_root":"C:/w/s"})),
            job(false, true, &[])
        );
        // A prepare without output claims the files it would convert to, named as it names them.
        let batch = command(
            json!({"command":"media.prepare","input_root":"C:/w","output_root":"C:/w/out",
            "paths":["phone/clip.mp4","cam/clip.mov","talk.mp4"],"project":{"assets":[{"id":"talk"}]}}),
        );
        assert_eq!(
            batch.claims,
            [
                "C:/w/out/clip-prepared.mkv",
                "C:/w/out/clip-2-prepared.mkv",
                "C:/w/out/talk-2-prepared.mkv"
            ]
            .map(normalize)
        );
        let single = command(json!({"command":"media.prepare","input_root":"C:/w",
            "output_root":"C:/w/out","path":"phone/clip.mp4"}));
        assert_eq!(single.claims, [normalize("C:/w/out/clip-prepared.mkv")]);
    }
}
