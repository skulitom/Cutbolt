//! File-backed conversion payloads, with an owned index and cross-process reader leases.
use crate::{
    Result,
    cache::Policy,
    cache_store::{Temporary, digest, key_ok},
    error, media,
    registry::Identity,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const APP: u32 = 0x43424346;
const INDEX: &str = "cutbolt-conform.sqlite";
const LOCK: &str = ".cutbolt-conform.lock";
const MAGIC: &[u8] = b"Cutbolt conform cache lock v1\n";
pub(crate) const MAX_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_ENTRIES: u32 = 4096;
const RESERVATION: u64 = 8 * 1024 * 1024;
#[cfg(test)]
mod tests;

pub(crate) fn validate(policy: &Policy) -> Result<()> {
    if policy.max_bytes > MAX_BYTES || policy.max_entries > MAX_ENTRIES {
        return Err(error(
            "INVALID_CACHE",
            "Conform cache supports at most 1 TiB and 4096 entries",
        ));
    }
    Ok(())
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_CACHE", message)
}
fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn regular(path: &Path) -> Result<()> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.file_type().is_symlink() {
        return Err(invalid(
            "Cache control and payload files must be regular files",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(invalid("Cache reparse points are unsupported"));
        }
    }
    Ok(())
}
fn lock(root: &Path, exclusive: bool, create: bool) -> Result<File> {
    let path = root.join(LOCK);
    if !path.try_exists()? {
        if !create {
            return Err(error(
                "CACHE_MISSING",
                "No conform cache exists at this root",
            ));
        }
        let temp = Temporary::new(root, "lock")?;
        let mut file = OpenOptions::new().write(true).open(&temp.0)?;
        file.write_all(MAGIC)?;
        file.sync_all()?;
        drop(file);
        if let Err(e) = fs::hard_link(&temp.0, &path)
            && !path.try_exists()?
        {
            return Err(e.into());
        }
    }
    regular(&path)?;
    let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
    let start = Instant::now();
    loop {
        let result = if exclusive {
            file.try_lock()
        } else {
            file.try_lock_shared()
        };
        match result {
            Ok(()) => break,
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
            Err(std::fs::TryLockError::WouldBlock) => {}
        }
        if media::ambient_cancelled() {
            return Err(error(
                "JOB_CANCELLED",
                "Cancelled while waiting for conversion cache",
            ));
        }
        if start.elapsed() > Duration::from_secs(4 * 3600) {
            return Err(error(
                "STORE_BUSY",
                "Conform cache still has an active reader or producer",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    // Windows range locks also deny reads: inspect the marker only after acquiring our lease.
    let mut bytes = Vec::new();
    Read::by_ref(&mut file).take(128).read_to_end(&mut bytes)?;
    if bytes != MAGIC {
        return Err(invalid(
            "Existing conform lock is foreign; it was not changed",
        ));
    }
    Ok(file)
}

pub(crate) struct Store {
    // A reader holds this until its caller finishes using the payload. Writers/pruners wait.
    _lease: File,
    connection: Connection,
    pub(crate) root: PathBuf,
    exclusive: bool,
    protected: BTreeSet<String>,
}
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct Entry {
    pub path: PathBuf,
    pub identity: Identity,
    pub metadata: Value,
}
impl Store {
    pub(crate) fn open(root: &Path, exclusive: bool, create: bool) -> Result<Self> {
        let root = media::input_root(root)?;
        let lease = lock(&root, exclusive, create)?;
        let path = root.join(INDEX);
        if !path.try_exists()? {
            if !exclusive || !create {
                return Err(error("CACHE_MISSING", "No conform cache index exists"));
            }
            let temp = Temporary::new(&root, "sqlite")?;
            let c = Connection::open_with_flags(&temp.0, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            c.execute_batch(&format!("PRAGMA auto_vacuum=FULL; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; PRAGMA application_id={APP}; PRAGMA user_version=1;
                CREATE TABLE clock(value INTEGER NOT NULL); INSERT INTO clock VALUES(0);
                CREATE TABLE entries(key TEXT PRIMARY KEY, extension TEXT NOT NULL, ready INTEGER NOT NULL,
                    bytes INTEGER NOT NULL, digest TEXT NOT NULL, metadata TEXT NOT NULL, metadata_digest TEXT NOT NULL, touched INTEGER NOT NULL);
                CREATE INDEX conform_lru ON entries(touched,key);"))?;
            c.close().map_err(|(_, e)| e)?;
            fs::hard_link(&temp.0, &path)?;
        }
        regular(&path)?;
        let mut header = [0u8; 100];
        if File::open(&path)?.read_exact(&mut header).is_err()
            || &header[..16] != b"SQLite format 3\0"
            || u32::from_be_bytes(header[60..64].try_into().expect("header")) != 1
            || u32::from_be_bytes(header[68..72].try_into().expect("header")) != APP
        {
            return Err(invalid(
                "Existing index is not an owned version-1 conform cache; it was not changed",
            ));
        }
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(Duration::from_secs(30))?;
        connection.execute_batch("PRAGMA trusted_schema=OFF; PRAGMA synchronous=EXTRA;")?;
        let foreign:i64=connection.query_row("SELECT count(*) FROM sqlite_schema WHERE NOT ((type='table' AND name IN ('clock','entries')) OR (type='index' AND name IN ('conform_lru','sqlite_autoindex_entries_1')))",[],|r|r.get(0))?;
        if foreign != 0 {
            return Err(invalid("Unexpected conform cache database objects"));
        }
        let clock: i64 = connection.query_row("SELECT value FROM clock", [], |r| r.get(0))?;
        let clocks: i64 = connection.query_row("SELECT count(*) FROM clock", [], |r| r.get(0))?;
        if clocks != 1 || !(0..i64::MAX - 4096).contains(&clock) {
            return Err(invalid("Malformed conform cache clock"));
        }
        let store = Self {
            _lease: lease,
            connection,
            root,
            exclusive,
            protected: BTreeSet::new(),
        };
        let rows = store.rows()?;
        if rows.len() > MAX_ENTRIES as usize || rows.iter().map(|r| r.3).sum::<u64>() > MAX_BYTES {
            return Err(invalid("Conform cache index exceeds its absolute bounds"));
        }
        if exclusive {
            for (key, extension, ready, _) in rows {
                if !ready {
                    store.remove(&key, &extension)?;
                } else {
                    store.remove_stage(&key)?;
                }
            }
        }
        Ok(store)
    }
    /// Pin this request's working set while a writer makes missing entries.
    pub(crate) fn protect(&mut self, keys: impl IntoIterator<Item = String>) -> Result<()> {
        self.writer()?;
        let keys: BTreeSet<_> = keys.into_iter().collect();
        if keys.iter().any(|key| !key_ok(key)) {
            return Err(invalid("Malformed protected conform key"));
        }
        self.protected = keys;
        Ok(())
    }
    /// Keep the shared filesystem lease after closing the metadata connection.
    pub(crate) fn into_lease(self) -> File {
        self._lease
    }
    fn budget_error() -> crate::Error {
        error(
            "LIMIT_EXCEEDED",
            "The required conversion working set exceeds its cache policy",
        )
        .with_path("policy")
        .with_hint("Raise the conversion cache byte/entry policy or use fewer source assets.")
    }
    fn writer(&self) -> Result<()> {
        if self.exclusive {
            Ok(())
        } else {
            Err(invalid("Conform cache mutation requires its writer lease"))
        }
    }
    fn path(&self, key: &str, extension: &str) -> Result<PathBuf> {
        if !key_ok(key) || !matches!(extension, "mkv" | "wav") {
            return Err(invalid("Malformed conform cache entry name"));
        }
        Ok(self
            .root
            .join(format!(".cutbolt-conform-{key}.{extension}")))
    }
    fn stage(&self, key: &str) -> Result<PathBuf> {
        if !key_ok(key) {
            return Err(invalid("Malformed conform cache key"));
        }
        Ok(self.root.join(format!(".cutbolt-conform-stage-{key}")))
    }
    fn rows(&self) -> Result<Vec<(String, String, bool, u64)>> {
        let mut statement = self
            .connection
            .prepare("SELECT key,extension,ready,bytes FROM entries ORDER BY touched,key")?;
        let mut output = Vec::new();
        for row in statement.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })? {
            let (key, extension, ready, bytes) = row?;
            self.path(&key, &extension)?;
            if !matches!(ready, 0 | 1)
                || bytes < 0
                || bytes as u64 > MAX_BYTES
                || (ready == 1 && bytes == 0)
            {
                return Err(invalid("Malformed conform cache row"));
            }
            output.push((key, extension, ready == 1, bytes as u64));
            if output.len() > MAX_ENTRIES as usize {
                return Err(invalid("Too many conform cache entries"));
            }
        }
        Ok(output)
    }
    fn remove_stage(&self, key: &str) -> Result<()> {
        let path = self.stage(key)?;
        if path.try_exists()? {
            let m = fs::symlink_metadata(&path)?;
            if !m.is_dir() || m.file_type().is_symlink() {
                return Err(invalid("Conform staging directory was replaced"));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    return Err(invalid("Conform staging directory is a reparse point"));
                }
            }
            // Exact owned staging name derived from a validated indexed key, inside the fixed root.
            fs::remove_dir_all(path)?;
        }
        Ok(())
    }
    fn remove(&self, key: &str, extension: &str) -> Result<()> {
        self.writer()?;
        let path = self.path(key, extension)?;
        if path.try_exists()? {
            regular(&path)?;
            fs::remove_file(path)?;
        }
        self.remove_stage(key)?;
        self.connection
            .execute("DELETE FROM entries WHERE key=?1", [key])?;
        Ok(())
    }
    pub(crate) fn discard(&self, key: &str) -> Result<()> {
        self.writer()?;
        for (k, e, _, _) in self.rows()? {
            if k == key {
                return self.remove(&k, &e);
            }
        }
        Ok(())
    }
    pub(crate) fn inspect(&self) -> Result<Value> {
        let rows = self.rows()?;
        Ok(
            json!({"kind":"conform","payload_bytes":rows.iter().map(|r|r.3).sum::<u64>(),
            "entries":rows.iter().map(|(k,e,ready,bytes)|json!({"key":k,"extension":e,"ready":ready,"bytes":bytes})).collect::<Vec<_>>(),
            "maximum_bytes":MAX_BYTES,"maximum_entries":MAX_ENTRIES,"raw_decode_scratch":"separate_existing_limits","eviction_order":"least_recently_used"}),
        )
    }
    pub(crate) fn prune(&self, policy: &Policy) -> Result<Value> {
        self.writer()?;
        validate(policy)?;
        let mut evicted = Vec::new();
        loop {
            let rows = self.rows()?;
            if rows.len() <= policy.max_entries as usize
                && rows.iter().map(|r| r.3).sum::<u64>() <= policy.max_bytes
            {
                break;
            }
            let (key, ext, _, _) = rows
                .iter()
                .find(|r| !self.protected.contains(&r.0))
                .ok_or_else(Self::budget_error)?;
            self.remove(key, ext)?;
            evicted.push(key.clone());
        }
        Ok(json!({"evicted":evicted,"remaining":self.inspect()?}))
    }
    pub(crate) fn read(&mut self, key: &str) -> Result<Option<Entry>> {
        if !key_ok(key) {
            return Err(invalid("Malformed conform cache key"));
        }
        let row:Option<(String,u64,String,String,String)>=self.connection.query_row(
            "SELECT extension,bytes,digest,CASE WHEN length(metadata)<=65536 THEN metadata ELSE '' END,metadata_digest FROM entries WHERE key=?1 AND ready=1",[key],
            |r|Ok((r.get(0)?,number(r,1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((ext, bytes, sha256, metadata, hash)) = row else {
            return Ok(None);
        };
        let corrupt = || {
            error(
                "CACHE_CORRUPT",
                "Conform cache payload or recorded metadata changed",
            )
        };
        if metadata.len() > 65536 || digest(metadata.as_bytes()) != hash || !key_ok(&sha256) {
            return Err(corrupt());
        }
        let envelope: Value = serde_json::from_str(&metadata).map_err(|_| corrupt())?;
        if envelope["key"] != key || envelope.get("value").is_none() {
            return Err(corrupt());
        }
        let path = self.path(key, &ext)?;
        if !path.try_exists()? {
            return Err(corrupt());
        }
        regular(&path)?;
        if fs::metadata(&path)?.len() != bytes || media::file_hash(&path)? != sha256 {
            return Err(corrupt());
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("UPDATE clock SET value=value+1", [])?;
        tx.execute(
            "UPDATE entries SET touched=(SELECT value FROM clock) WHERE key=?1",
            [key],
        )?;
        tx.commit()?;
        Ok(Some(Entry {
            path,
            identity: Identity { bytes, sha256 },
            metadata: envelope["value"].clone(),
        }))
    }
    pub(crate) fn begin(&self, key: &str, extension: &str, policy: &Policy) -> Result<PathBuf> {
        self.writer()?;
        validate(policy)?;
        if policy.max_bytes == 0 || policy.max_entries == 0 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Conform caching needs a positive byte and entry budget",
            )
            .with_path("policy"));
        }
        let path = self.path(key, extension)?;
        let stage = self.stage(key)?;
        if path.try_exists()? || stage.try_exists()? {
            return Err(invalid(
                "Unindexed conform destination exists; it was not overwritten",
            ));
        }
        self.prune(&Policy {
            max_bytes: policy.max_bytes,
            max_entries: policy.max_entries - 1,
        })?;
        self.connection.execute(
            "INSERT INTO entries VALUES(?1,?2,0,0,'','','',0)",
            params![key, extension],
        )?;
        fs::create_dir(&stage)?;
        Ok(stage.join(format!("output.{extension}")))
    }
    pub(crate) fn reserve(&self, key: &str, bytes: u64, policy: &Policy) -> Result<()> {
        self.writer()?;
        if bytes > policy.max_bytes {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Converted output exceeds its cache budget",
            )
            .with_path("policy.max_bytes"));
        }
        let reserved: u64 = self.connection.query_row(
            "SELECT bytes FROM entries WHERE key=?1 AND ready=0",
            [key],
            |r| number(r, 0),
        )?;
        if bytes <= reserved {
            return Ok(());
        }
        let quantum = RESERVATION.min((policy.max_bytes / 64).max(1));
        let wanted = bytes
            .div_ceil(quantum)
            .saturating_mul(quantum)
            .min(policy.max_bytes);
        loop {
            let rows = self.rows()?;
            let other = rows.iter().filter(|r| r.0 != key).map(|r| r.3).sum::<u64>();
            if other + wanted <= policy.max_bytes {
                break;
            }
            let (old, ext, _, _) = rows
                .iter()
                .find(|r| r.0 != key && r.2 && !self.protected.contains(&r.0))
                .ok_or_else(Self::budget_error)?;
            self.remove(old, ext)?;
        }
        self.connection.execute(
            "UPDATE entries SET bytes=?2 WHERE key=?1 AND ready=0",
            params![key, wanted as i64],
        )?;
        Ok(())
    }
    pub(crate) fn finish(
        &self,
        key: &str,
        extension: &str,
        output: &Path,
        identity: &Identity,
        metadata: &Value,
    ) -> Result<()> {
        self.writer()?;
        identity.validate()?;
        let expected = self.stage(key)?.join(format!("output.{extension}"));
        if output != expected {
            return Err(invalid("Unexpected conform staging output"));
        }
        let reserved: u64 = self.connection.query_row(
            "SELECT bytes FROM entries WHERE key=?1 AND ready=0",
            [key],
            |r| number(r, 0),
        )?;
        if identity.bytes > reserved
            || fs::metadata(output)?.len() != identity.bytes
            || media::file_hash(output)? != identity.sha256
        {
            return Err(error(
                "CACHE_CORRUPT",
                "Conform output changed before cache publication",
            ));
        }
        let metadata = serde_json::to_string(&json!({"key":key,"value":metadata}))?;
        if metadata.len() > 65536 {
            return Err(invalid("Conform cache metadata exceeds 64 KiB"));
        }
        // Same filesystem hard-link publication never overwrites, and duplicates no payload bytes.
        fs::hard_link(output, self.path(key, extension)?)?;
        self.connection
            .execute("UPDATE clock SET value=value+1", [])?;
        self.connection.execute("UPDATE entries SET ready=1,bytes=?2,digest=?3,metadata=?4,metadata_digest=?5,touched=(SELECT value FROM clock) WHERE key=?1",
            params![key,identity.bytes as i64,identity.sha256,metadata,digest(metadata.as_bytes())])?;
        self.remove_stage(key)?;
        Ok(())
    }
}
