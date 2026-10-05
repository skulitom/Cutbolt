//! Verified source inspections reused by content identity.
//!
//! Renders, previews and exports inspect every source they read. The slow part, decoding every
//! video frame to check its timestamps, depends only on the file's bytes, the inspection
//! parameters, the ffprobe build and this engine build. Every command still hashes each source,
//! so changed bytes never reuse an entry. Entries live in process memory and, when
//! `CUTBOLT_INSPECTION_CACHE` names a directory (a workspace sets `.cutbolt/cache/inspections`),
//! as small self-checking JSON files there. A damaged or foreign entry is ignored and rebuilt.
//! Inspections of different files may run in parallel (`LANES` at a time); concurrent requests
//! for the same key wait for one inspection.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Condvar, Mutex, MutexGuard, OnceLock, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

/// Bump when inspection rules change in a way the engine source identity would not capture.
const SCHEMA: u32 = 1;
pub const DIRECTORY_VARIABLE: &str = "CUTBOLT_INSPECTION_CACHE";
/// Inspections that may probe or decode at once in one process. Process creation stops scaling
/// at about four launchers (docs/DEVELOPMENT_LOOP.md), and each strict decode uses its own
/// decoder threads.
pub(crate) const LANES: usize = 4;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keys being inspected in this process.
fn running() -> &'static (Mutex<HashSet<String>>, Condvar) {
    static RUNNING: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();
    RUNNING.get_or_init(Default::default)
}

/// The right to inspect one key: a concurrent caller with the same key waits for it and then
/// finds the entry, instead of decoding the same bytes again.
pub(crate) struct Turn(String);
pub(crate) fn turn(key: &str) -> Turn {
    let (keys, changed) = running();
    let mut keys = lock(keys);
    while keys.contains(key) {
        keys = changed.wait(keys).unwrap_or_else(PoisonError::into_inner);
    }
    keys.insert(key.to_owned());
    Turn(key.to_owned())
}
impl Drop for Turn {
    fn drop(&mut self) {
        let (keys, changed) = running();
        lock(keys).remove(&self.0);
        changed.notify_all();
    }
}

/// Inspections probing or decoding in this process.
fn busy() -> &'static (Mutex<usize>, Condvar) {
    static BUSY: OnceLock<(Mutex<usize>, Condvar)> = OnceLock::new();
    BUSY.get_or_init(Default::default)
}

/// One of the `LANES` places for an inspection's tool work, held until dropped. Nested parallel
/// callers (contact-sheet cells that each inspect their window) therefore share one limit.
pub(crate) struct Slot;
pub(crate) fn slot() -> Slot {
    let (count, freed) = busy();
    let mut count = lock(count);
    while *count >= LANES {
        count = freed.wait(count).unwrap_or_else(PoisonError::into_inner);
    }
    *count += 1;
    Slot
}
impl Drop for Slot {
    fn drop(&mut self) {
        let (count, freed) = busy();
        *lock(count) -= 1;
        freed.notify_one();
    }
}

/// What a passed inspection established about one file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Verified {
    pub frames: u64,
    pub samples: u64,
    /// The video stream's pixel format (`bgr0` or `bgra`).
    pub pix_fmt: String,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    schema: u32,
    key: String,
    verified: Verified,
    check: String,
}
impl Entry {
    fn check(key: &str, verified: &Verified) -> String {
        let mut hash = Sha256::new();
        hash.update(SCHEMA.to_le_bytes());
        hash.update(key.as_bytes());
        hash.update(serde_json::to_vec(verified).expect("inspection entry"));
        format!("{:x}", hash.finalize())
    }
}

fn memory() -> &'static Mutex<HashMap<String, Verified>> {
    static MEMORY: OnceLock<Mutex<HashMap<String, Verified>>> = OnceLock::new();
    MEMORY.get_or_init(Default::default)
}

fn directory() -> Option<&'static Path> {
    static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIRECTORY
        .get_or_init(|| {
            std::env::var_os(DIRECTORY_VARIABLE)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
        })
        .as_deref()
}

/// Executable identities by path, size and modification time.
type Tools = Mutex<HashMap<(PathBuf, u64, u128), String>>;

/// The ffprobe build in a key. With a cache directory it is the resolved executable's SHA-256 and
/// size, hashed once per process for each path, size and modification time (a 140 MB build takes
/// about 0.1 s). Entries kept only in this process use that path, size and time directly. None when
/// the tool cannot be resolved, which disables caching.
fn tool_identity(selected: &str) -> Option<String> {
    static TOOLS: OnceLock<Tools> = OnceLock::new();
    let path = crate::cache::resolve_tool(selected).ok()?;
    let metadata = fs::metadata(&path).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_nanos();
    if directory().is_none() {
        return Some(format!("{}:{}:{modified}", path.display(), metadata.len()));
    }
    let slot = (path.clone(), metadata.len(), modified);
    let tools = TOOLS.get_or_init(Default::default);
    if let Some(known) = tools.lock().ok()?.get(&slot) {
        return Some(known.clone());
    }
    let identity = format!(
        "{}:{}",
        metadata.len(),
        crate::media::file_hash(&path).ok()?
    );
    tools.lock().ok()?.insert(slot, identity.clone());
    Some(identity)
}

/// The cache key of one inspection: content identity, parameters, ffprobe and engine builds.
/// None when the ffprobe executable cannot be identified.
pub(crate) fn key(sha256: &str, bytes: u64, parameters: &str, ffprobe: &str) -> Option<String> {
    let tool = tool_identity(ffprobe)?;
    let mut hash = Sha256::new();
    for part in [
        &SCHEMA.to_string(),
        sha256,
        &bytes.to_string(),
        parameters,
        &tool,
        crate::cache::producer(),
    ] {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part.as_bytes());
    }
    Some(format!("{:x}", hash.finalize()))
}

pub(crate) fn get(key: &str) -> Option<Verified> {
    if let Some(found) = memory().lock().ok()?.get(key) {
        return Some(found.clone());
    }
    let path = directory()?.join(format!("{key}.json"));
    let entry: Entry = serde_json::from_slice(&fs::read(&path).ok()?).ok()?;
    if entry.schema != SCHEMA
        || entry.key != key
        || entry.check != Entry::check(key, &entry.verified)
    {
        return None;
    }
    memory()
        .lock()
        .ok()?
        .insert(key.to_owned(), entry.verified.clone());
    Some(entry.verified)
}

/// Remember a passed inspection. Persisting is best effort: a failed write only costs a later
/// inspection.
pub(crate) fn put(key: &str, verified: &Verified) {
    if let Ok(mut memory) = memory().lock() {
        memory.insert(key.to_owned(), verified.clone());
    }
    let Some(directory) = directory() else {
        return;
    };
    let entry = Entry {
        schema: SCHEMA,
        key: key.to_owned(),
        verified: verified.clone(),
        check: Entry::check(key, verified),
    };
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = directory.join(format!(".{key}.{}-{nonce}.tmp", std::process::id()));
    let written = fs::create_dir_all(directory)
        .and_then(|()| fs::write(&temp, serde_json::to_vec(&entry).expect("inspection entry")))
        .and_then(|()| fs::rename(&temp, directory.join(format!("{key}.json"))));
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_check_their_key_and_content() {
        let verified = Verified {
            frames: 3,
            samples: 5760,
            pix_fmt: "bgr0".into(),
        };
        let check = Entry::check("k", &verified);
        assert_eq!(check, Entry::check("k", &verified));
        assert_ne!(check, Entry::check("other", &verified));
        assert_ne!(
            check,
            Entry::check(
                "k",
                &Verified {
                    frames: 4,
                    ..verified.clone()
                }
            )
        );
        put("unit-test-key", &verified);
        assert_eq!(get("unit-test-key"), Some(verified));
        assert_eq!(get("unit-test-missing"), None);
    }

    #[test]
    fn inspections_share_lanes_and_one_key_runs_once_at_a_time() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let (running, most, same) = (
            AtomicUsize::new(0),
            AtomicUsize::new(0),
            AtomicUsize::new(0),
        );
        std::thread::scope(|scope| {
            for i in 0..12 {
                let (running, most, same) = (&running, &most, &same);
                scope.spawn(move || {
                    let shared = i % 3 == 0;
                    let _turn = shared.then(|| turn("unit-test-shared-key"));
                    let _slot = slot();
                    most.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                    if shared {
                        assert_eq!(same.fetch_add(1, SeqCst), 0, "one inspection per key");
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    if shared {
                        same.fetch_sub(1, SeqCst);
                    }
                    running.fetch_sub(1, SeqCst);
                });
            }
        });
        let most = most.load(SeqCst);
        assert!(
            (2..=LANES).contains(&most),
            "{most} inspections ran at once"
        );
    }
}
