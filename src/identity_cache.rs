//! User-approved, process-local reuse of verified file hashes. A cache hit requires the same
//! canonical path, length, modification time and OS file identity. Publication still hashes.
use std::{
    collections::HashMap,
    fs::{File, Metadata},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{Instant, SystemTime},
};

const MAX_ENTRIES: usize = 4096;
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint {
    path: PathBuf,
    bytes: u64,
    modified: SystemTime,
    id: [u64; 3],
}
struct Entry {
    fingerprint: Fingerprint,
    digest: String,
    used: Instant,
}
fn entries() -> &'static Mutex<HashMap<PathBuf, Entry>> {
    static ENTRIES: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
    ENTRIES.get_or_init(Default::default)
}

#[cfg(unix)]
fn file_id(_file: &File, metadata: &Metadata) -> Option<[u64; 3]> {
    use std::os::unix::fs::MetadataExt;
    Some([metadata.dev(), metadata.ino(), 0])
}
#[cfg(windows)]
fn file_id(file: &File, _metadata: &Metadata) -> Option<[u64; 3]> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ID_128, FILE_ID_INFO, FileIdInfo, GetFileInformationByHandleEx,
    };
    let mut info = FILE_ID_INFO {
        VolumeSerialNumber: 0,
        FileId: FILE_ID_128 {
            Identifier: [0; 16],
        },
    };
    // The live handle pins the file object while Windows returns its full 128-bit ID.
    let success = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle(),
            FileIdInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    if success == 0 {
        return None;
    }
    Some([
        info.VolumeSerialNumber,
        u64::from_le_bytes(info.FileId.Identifier[..8].try_into().ok()?),
        u64::from_le_bytes(info.FileId.Identifier[8..].try_into().ok()?),
    ])
}
#[cfg(not(any(unix, windows)))]
fn file_id(_file: &File, _metadata: &Metadata) -> Option<[u64; 3]> {
    None
}

pub(crate) fn fingerprint(path: &Path, file: &File) -> Option<Fingerprint> {
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(Fingerprint {
        path: path.canonicalize().ok()?,
        bytes: metadata.len(),
        modified: metadata.modified().ok()?,
        id: file_id(file, &metadata)?,
    })
}

pub(crate) fn lookup(fingerprint: &Fingerprint) -> Option<String> {
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if entries
        .get(&fingerprint.path)
        .is_some_and(|e| e.fingerprint != *fingerprint)
    {
        entries.remove(&fingerprint.path);
        return None;
    }
    let entry = entries.get_mut(&fingerprint.path)?;
    entry.used = Instant::now();
    Some(entry.digest.clone())
}

/// Only remember a full hash when both the open object and current path still match its
/// pre-hash fingerprint. Missing metadata/file IDs simply disable reuse.
pub(crate) fn remember(path: &Path, file: &File, before: Option<Fingerprint>, digest: &str) {
    let Some(before) = before else {
        return;
    };
    if fingerprint(path, file).as_ref() != Some(&before) {
        return;
    }
    let current = File::open(path).ok().and_then(|f| fingerprint(path, &f));
    if current.as_ref() != Some(&before) {
        return;
    }
    let mut entries = entries()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if entries.len() >= MAX_ENTRIES
        && !entries.contains_key(&before.path)
        && let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(p, _)| p.clone())
    {
        entries.remove(&oldest);
    }
    entries.insert(
        before.path.clone(),
        Entry {
            fingerprint: before,
            digest: digest.into(),
            used: Instant::now(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media;
    fn cached(path: &Path) -> Option<String> {
        lookup(&fingerprint(path, &File::open(path).unwrap())?)
    }
    #[test]
    fn reuse_requires_metadata_and_file_id_and_full_hash_detects_spoofed_mtime() {
        let scratch = crate::tracking::Scratch::new("identity-cache");
        let path = scratch.0.join("source.bin");
        std::fs::write(&path, b"original").unwrap();
        let old_time = path.metadata().unwrap().modified().unwrap();
        let original = media::file_hash(&path).unwrap();
        assert_eq!(cached(&path), Some(original.clone()));
        std::fs::write(&path, b"modified").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(old_time + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(cached(&path), None);
        media::file_hash(&path).unwrap();
        std::fs::rename(&path, scratch.0.join("old.bin")).unwrap();
        std::fs::write(&path, b"replaced").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(old_time + std::time::Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(
            cached(&path),
            None,
            "replacement file ID must invalidate equal size and mtime"
        );
        let replaced = media::file_hash(&path).unwrap();
        let time = path.metadata().unwrap().modified().unwrap();
        std::fs::write(&path, b"mutated!").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(time))
            .unwrap();
        assert_eq!(
            cached(&path),
            Some(replaced.clone()),
            "explicitly approved metadata-reuse tradeoff"
        );
        assert_eq!(
            media::file_hash_reused_controlled(&path, &media::Uncontrolled).unwrap(),
            replaced
        );
        let actual = media::file_hash(&path).unwrap();
        assert_ne!(
            actual, replaced,
            "publication must use the full-hash function"
        );
        assert_eq!(cached(&path), Some(actual));
    }
}
