//! Original local cache storage. One owned SQLite database, incremental payload
//! IO, transactional least-recently-used eviction and checked readback.
use crate::{Result, error, media, registry::Identity};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const APP: i64 = 0x43424348;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ENTRIES: u32 = 4096;
pub(crate) const FILE: &str = "cutbolt-cache.sqlite";

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub max_bytes: u64,
    pub max_entries: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::env::temp_dir()
                .join(format!("cutbolt-cache-unit-{}-{nonce}", std::process::id()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
        fn input(&self, bytes: &[u8]) -> (PathBuf, Identity) {
            let path = self.0.join("source.bin");
            fs::write(&path, bytes).unwrap();
            (
                path,
                Identity {
                    bytes: bytes.len() as u64,
                    sha256: digest(bytes),
                },
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            for name in [
                FILE,
                "cutbolt-cache.sqlite-journal",
                "source.bin",
                "read.bin",
                "read2.bin",
                "child-ready",
            ] {
                let _ = fs::remove_file(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn put(store: &mut Store, root: &Fixture, name: &str, bytes: &[u8], policy: &Policy) {
        let (path, id) = root.input(bytes);
        assert_eq!(
            store
                .put(
                    &digest(name.as_bytes()),
                    "fixture",
                    &path,
                    &id,
                    &json!({"name":name}),
                    policy
                )
                .unwrap()["stored"],
            true
        );
    }
    #[test]
    fn exact_payloads_lru_eviction_and_corrupt_entry_rebuild() {
        let root = Fixture::new();
        let mut store = Store::open(&root.0, true).unwrap();
        let policy = Policy {
            max_bytes: 100,
            max_entries: 2,
        };
        put(&mut store, &root, "a", b"first original payload", &policy);
        put(&mut store, &root, "b", b"second original payload", &policy);
        let a = digest(b"a");
        let b = digest(b"b");
        let c = digest(b"c");
        let read = root.0.join("read.bin");
        let hit = store.read(&a, "fixture", &read).unwrap().unwrap();
        assert_eq!(fs::read(&read).unwrap(), b"first original payload");
        assert_eq!(hit.metadata["name"], "a");
        put(&mut store, &root, "c", b"third original payload", &policy);
        let second = root.0.join("read2.bin");
        assert!(store.read(&b, "fixture", &second).unwrap().is_none());
        assert!(!second.exists());
        store
            .connection
            .execute("UPDATE entries SET data=zeroblob(bytes) WHERE key=?1", [&c])
            .unwrap();
        assert!(store.read(&c, "fixture", &second).unwrap().is_none());
        assert!(!second.exists());
        put(&mut store, &root, "c", b"third original payload", &policy);
        store
            .connection
            .execute("UPDATE entries SET metadata='{}' WHERE key=?1", [&c])
            .unwrap();
        assert!(store.read(&c, "fixture", &second).unwrap().is_none());
        assert_eq!(
            store.inspect().unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        let old = fs::read(&read).unwrap();
        assert_eq!(
            store.read(&a, "fixture", &read).unwrap_err().code,
            "IO_ERROR"
        );
        assert_eq!(fs::read(read).unwrap(), old);
        store
            .prune(&Policy {
                max_bytes: 0,
                max_entries: 0,
            })
            .unwrap();
        assert_eq!(store.inspect().unwrap()["payload_bytes"], 0);
    }
    #[test]
    fn failed_insert_rolls_back_eviction_and_keeps_sources() {
        let root = Fixture::new();
        let mut store = Store::open(&root.0, true).unwrap();
        let policy = Policy {
            max_bytes: 100,
            max_entries: 1,
        };
        put(
            &mut store,
            &root,
            "a",
            b"keep this committed payload",
            &policy,
        );
        let (path, mut id) = root.input(b"new payload with wrong identity");
        id.sha256 = digest(b"something else");
        assert_eq!(
            store
                .put(&digest(b"b"), "fixture", &path, &id, &json!({}), &policy)
                .unwrap_err()
                .code,
            "MEDIA_CHANGED"
        );
        let entry = store
            .read(&digest(b"a"), "fixture", &root.0.join("read.bin"))
            .unwrap()
            .unwrap();
        assert_eq!(
            entry.identity.sha256,
            digest(b"keep this committed payload")
        );
        assert_eq!(fs::read(path).unwrap(), b"new payload with wrong identity");
    }
    #[test]
    fn unrelated_database_is_never_adopted() {
        let root = Fixture::new();
        let path = root.0.join(FILE);
        let other = Connection::open(&path).unwrap();
        other
            .execute("CREATE TABLE user_data(value TEXT)", [])
            .unwrap();
        drop(other);
        let before = fs::read(&path).unwrap();
        assert!(Store::open(&root.0, true).is_err());
        assert_eq!(fs::read(path).unwrap(), before);
    }
    #[test]
    fn sqlite_full_rolls_back_eviction_and_payload_write() {
        let root = Fixture::new();
        let mut store = Store::open(&root.0, true).unwrap();
        let policy = Policy {
            max_bytes: 1024 * 1024,
            max_entries: 1,
        };
        put(&mut store, &root, "a", b"original committed entry", &policy);
        let pages: i64 = store
            .connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .unwrap();
        store
            .connection
            .pragma_update(None, "max_page_count", pages)
            .unwrap();
        let (path, id) = root.input(&vec![0x6b; 256 * 1024]);
        let failure = store
            .put(&digest(b"b"), "fixture", &path, &id, &json!({}), &policy)
            .unwrap_err();
        assert!(failure.message.contains("full"), "{failure:?}");
        let output = root.0.join("read.bin");
        store
            .read(&digest(b"a"), "fixture", &output)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(output).unwrap(), b"original committed entry");
        assert_eq!(fs::read(path).unwrap(), vec![0x6b; 256 * 1024]);
        assert_eq!(
            store.inspect().unwrap()["entries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }
    #[test]
    fn pruning_reclaims_owned_database_payload_pages() {
        let root = Fixture::new();
        let mut store = Store::open(&root.0, true).unwrap();
        put(
            &mut store,
            &root,
            "large",
            &vec![0x4c; 4 * 1024 * 1024],
            &Policy {
                max_bytes: 8 * 1024 * 1024,
                max_entries: 2,
            },
        );
        let before = store.inspect().unwrap()["database_bytes"].as_u64().unwrap();
        let result = store
            .prune(&Policy {
                max_bytes: 0,
                max_entries: 0,
            })
            .unwrap();
        assert_eq!(result["cache"]["payload_bytes"], 0);
        let after = result["cache"]["database_bytes"].as_u64().unwrap();
        assert!(
            before > 4 * 1024 * 1024 && after < 65536,
            "{before} -> {after}"
        );
    }
    #[test]
    fn crash_child() {
        let Some(root) = std::env::var_os("CUTBOLT_CACHE_CRASH_FIXTURE_ROOT") else {
            return;
        };
        let root = PathBuf::from(root);
        assert!(
            root.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cutbolt-cache-unit-")
        );
        let mut store = Store::open(&root, false).unwrap();
        store
            .connection
            .execute_batch("PRAGMA cache_size=4;")
            .unwrap();
        let tx = store
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        tx.execute("UPDATE entries SET data=randomblob(bytes)", [])
            .unwrap();
        fs::write(root.join("child-ready"), b"uncommitted data spilled").unwrap();
        std::process::exit(77);
    }
    #[test]
    fn hot_journal_recovery_preserves_last_complete_payload() {
        let root = Fixture::new();
        let bytes = vec![0x5a; 256 * 1024];
        let mut store = Store::open(&root.0, true).unwrap();
        put(
            &mut store,
            &root,
            "a",
            &bytes,
            &Policy {
                max_bytes: 1024 * 1024,
                max_entries: 2,
            },
        );
        drop(store);
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "cache_store::tests::crash_child", "--nocapture"])
            .env("CUTBOLT_CACHE_CRASH_FIXTURE_ROOT", &root.0)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(77));
        assert!(root.0.join("child-ready").is_file());
        assert!(
            fs::metadata(root.0.join("cutbolt-cache.sqlite-journal"))
                .unwrap()
                .len()
                > 512
        );
        let mut store = Store::open(&root.0, false).unwrap();
        let output = root.0.join("read.bin");
        store
            .read(&digest(b"a"), "fixture", &output)
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(output).unwrap(), bytes);
    }
}
impl Policy {
    pub fn validate(&self) -> Result<()> {
        if self.max_bytes > MAX_BYTES || self.max_entries > MAX_ENTRIES {
            return Err(error(
                "INVALID_CACHE",
                "Cache budgets support at most 1 GiB of payloads and 4096 entries",
            ));
        }
        Ok(())
    }
}
pub(crate) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn number(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
pub(crate) fn key_ok(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) struct Temporary(pub PathBuf);
impl Temporary {
    pub(crate) fn new(root: &Path, extension: &str) -> Result<Self> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
            .as_nanos();
        let path = root.join(format!(
            ".cutbolt-cache-{}-{stamp}.{extension}",
            std::process::id()
        ));
        File::create_new(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let mut journal = self.0.as_os_str().to_os_string();
        journal.push("-journal");
        let _ = fs::remove_file(PathBuf::from(journal));
    }
}

pub(crate) struct Store {
    connection: Connection,
    pub(crate) root: PathBuf,
}
#[derive(Debug)]
pub(crate) struct Hit {
    pub(crate) identity: Identity,
    pub(crate) metadata: Value,
}
impl Store {
    fn validate_database(connection: &Connection) -> Result<()> {
        let app: i64 = connection.pragma_query_value(None, "application_id", |row| row.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if app != APP || version != 1 {
            return Err(error(
                "INVALID_CACHE",
                "Existing file is not a supported Cutbolt cache; it was not changed",
            ));
        }
        let (count, bytes): (u64, u64) = connection.query_row(
            "SELECT count(*),coalesce(sum(bytes),0) FROM entries",
            [],
            |r| Ok((number(r, 0)?, number(r, 1)?)),
        )?;
        if count > MAX_ENTRIES as u64 || bytes > MAX_BYTES {
            return Err(error(
                "INVALID_CACHE",
                "Cache index exceeds its declared absolute bounds",
            ));
        }
        Ok(())
    }
    pub(crate) fn open(root: &Path, create: bool) -> Result<Self> {
        let root = media::input_root(root)?;
        let path = root.join(FILE);
        if !path.try_exists()? {
            if !create {
                return Err(error(
                    "CACHE_MISSING",
                    "No cache exists at the selected root",
                ));
            }
            let temp = Temporary::new(&root, "sqlite")?;
            let connection =
                Connection::open_with_flags(&temp.0, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
            connection.execute_batch(&format!("PRAGMA auto_vacuum=FULL; PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;
                PRAGMA application_id={APP}; PRAGMA user_version=1;
                CREATE TABLE clock(value INTEGER NOT NULL CHECK(value>=0)); INSERT INTO clock VALUES(0);
                CREATE TABLE entries(id INTEGER PRIMARY KEY, key TEXT UNIQUE NOT NULL, kind TEXT NOT NULL,
                    bytes INTEGER NOT NULL CHECK(bytes>0 AND bytes<=1073741824), digest TEXT NOT NULL,
                    metadata TEXT NOT NULL, metadata_digest TEXT NOT NULL, data BLOB NOT NULL, touched INTEGER NOT NULL);
                CREATE INDEX cache_lru ON entries(touched,key);"))?;
            connection.close().map_err(|(_, e)| e)?;
            if let Err(e) = fs::hard_link(&temp.0, &path)
                && !path.try_exists()?
            {
                return Err(e.into());
            }
        }
        if !path.canonicalize()?.starts_with(&root) {
            return Err(error(
                "PATH_OUTSIDE_ROOT",
                "Cache database must remain inside its explicit root",
            ));
        }
        // SQLite's public fixed header identifies ownership before opening for
        // recovery. A read-only SQLite connection cannot recover a hot journal.
        // Header facts: sqlite.org/fileformat.html, offsets 60 and 68, big endian.
        let mut header = [0u8; 100];
        if File::open(&path)?.read_exact(&mut header).is_err()
            || &header[..16] != b"SQLite format 3\0"
            || u32::from_be_bytes(header[60..64].try_into().expect("fixed slice")) != 1
            || u32::from_be_bytes(header[68..72].try_into().expect("fixed slice")) as i64 != APP
        {
            return Err(error(
                "INVALID_CACHE",
                "Existing file is not an owned version-1 cache; it was not changed",
            ));
        }
        let connection = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        Self::validate_database(&connection)?;
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA;")?;
        Ok(Self { connection, root })
    }
    pub(crate) fn read(
        &mut self,
        key: &str,
        kind: &str,
        destination: &Path,
    ) -> Result<Option<Hit>> {
        if !key_ok(key) {
            return Err(error("INVALID_CACHE", "Malformed cache key"));
        }
        // Reserving the short copy/read transaction prevents concurrent eviction
        // from removing the blob while it is being copied and verified.
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let entry: Option<(i64, u64, String, String, String, String)> = tx
            .query_row(
                "SELECT id,bytes,digest,metadata,kind,metadata_digest FROM entries WHERE key=?1",
                [key],
                |r| {
                    Ok((
                        r.get(0)?,
                        number(r, 1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, bytes, hash, metadata, actual_kind, metadata_digest)) = entry else {
            return Ok(None);
        };
        let mut output = File::create_new(destination)?;
        let checked = (|| -> Result<Hit> {
            if actual_kind != kind
                || bytes == 0
                || bytes > MAX_BYTES
                || !key_ok(&hash)
                || metadata.len() > 65536
                || digest(metadata.as_bytes()) != metadata_digest
            {
                return Err(error("CACHE_CORRUPT", "Cache entry metadata is invalid"));
            }
            let envelope: Value = serde_json::from_str(&metadata)?;
            if envelope["cache_key"] != key || envelope.get("value").is_none() {
                return Err(error(
                    "CACHE_CORRUPT",
                    "Cache metadata does not match its key",
                ));
            }
            let metadata = envelope["value"].clone();
            let mut blob = tx.blob_open("main", "entries", "data", id, true)?;
            if blob.len() as u64 != bytes {
                return Err(error("CACHE_CORRUPT", "Cache payload length differs"));
            }
            let mut digest = Sha256::new();
            let mut buffer = [0; 65536];
            let mut copied = 0;
            loop {
                let n = blob.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                digest.update(&buffer[..n]);
                output.write_all(&buffer[..n])?;
                copied += n as u64;
            }
            if copied != bytes || format!("{:x}", digest.finalize()) != hash {
                return Err(error("CACHE_CORRUPT", "Cache payload identity differs"));
            }
            output.sync_all()?;
            Ok(Hit {
                identity: Identity {
                    bytes,
                    sha256: hash,
                },
                metadata,
            })
        })();
        drop(output);
        match checked {
            Ok(hit) => {
                tx.execute("UPDATE clock SET value=value+1", [])?;
                tx.execute(
                    "UPDATE entries SET touched=(SELECT value FROM clock) WHERE id=?1",
                    [id],
                )?;
                tx.commit()?;
                Ok(Some(hit))
            }
            Err(e) if matches!(e.code, "CACHE_CORRUPT" | "INVALID_JSON") => {
                fs::remove_file(destination)?;
                tx.execute("DELETE FROM entries WHERE id=?1", [id])?;
                tx.commit()?;
                Ok(None)
            }
            Err(e) => {
                let _ = fs::remove_file(destination);
                Err(e)
            }
        }
    }
    fn evict(
        tx: &rusqlite::Transaction<'_>,
        policy: &Policy,
        reserve_bytes: u64,
        reserve_entries: u64,
    ) -> Result<Vec<String>> {
        let mut removed = Vec::new();
        loop {
            let (count, bytes): (u64, u64) = tx.query_row(
                "SELECT count(*),coalesce(sum(bytes),0) FROM entries",
                [],
                |r| Ok((number(r, 0)?, number(r, 1)?)),
            )?;
            if count + reserve_entries <= policy.max_entries as u64
                && bytes + reserve_bytes <= policy.max_bytes
            {
                return Ok(removed);
            }
            let key: Option<String> = tx
                .query_row(
                    "SELECT key FROM entries ORDER BY touched,key LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(key) = key else {
                return Err(error(
                    "INVALID_CACHE",
                    "Reserved cache entry exceeds its budget",
                ));
            };
            tx.execute("DELETE FROM entries WHERE key=?1", [&key])?;
            removed.push(key);
        }
    }
    pub(crate) fn put(
        &mut self,
        key: &str,
        kind: &str,
        file: &Path,
        identity: &Identity,
        metadata: &Value,
        policy: &Policy,
    ) -> Result<Value> {
        policy.validate()?;
        identity.validate()?;
        if !key_ok(key) {
            return Err(error("INVALID_CACHE", "Malformed cache key"));
        }
        let metadata = serde_json::to_string(&json!({"cache_key":key,"value":metadata}))?;
        if metadata.len() > 65536 {
            return Err(error("LIMIT_EXCEEDED", "Cache metadata exceeds 64 KiB"));
        }
        if identity.bytes > policy.max_bytes || policy.max_entries == 0 {
            return Ok(json!({"stored":false,"reason":"budget","evicted":[]}));
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM entries WHERE key=?1)",
            [key],
            |r| r.get(0),
        )?;
        if existing {
            return Ok(json!({"stored":false,"reason":"concurrent_entry","evicted":[]}));
        }
        let removed = Self::evict(&tx, policy, identity.bytes, 1)?;
        tx.execute("UPDATE clock SET value=value+1", [])?;
        tx.execute(
            "INSERT INTO entries(key,kind,bytes,digest,metadata,metadata_digest,data,touched)
            VALUES(?1,?2,?3,?4,?5,?6,zeroblob(?3),(SELECT value FROM clock))",
            params![
                key,
                kind,
                identity.bytes as i64,
                identity.sha256,
                metadata,
                digest(metadata.as_bytes())
            ],
        )?;
        let id = tx.last_insert_rowid();
        {
            let mut blob = tx.blob_open("main", "entries", "data", id, false)?;
            let mut input = File::open(file)?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            let mut count = 0;
            loop {
                let n = input.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                count += n as u64;
                if count > identity.bytes {
                    return Err(error("MEDIA_CHANGED", "Cache input grew before commit"));
                }
                blob.write_all(&buffer[..n])?;
                hash.update(&buffer[..n]);
            }
            if count != identity.bytes || format!("{:x}", hash.finalize()) != identity.sha256 {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Cache input identity changed before commit",
                ));
            }
        }
        tx.commit()?;
        Ok(json!({"stored":true,"evicted":removed}))
    }
    pub(crate) fn prune(&mut self, policy: &Policy) -> Result<Value> {
        policy.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = Self::evict(&tx, policy, 0, 0)?;
        tx.commit()?;
        Ok(json!({"evicted":removed,"cache":self.inspect()?}))
    }
    pub(crate) fn inspect(&self) -> Result<Value> {
        let mut statement = self.connection.prepare(
            "SELECT key,kind,bytes,digest,touched FROM entries ORDER BY touched DESC,key",
        )?;
        let entries = statement
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    number(r, 2)?,
                    r.get::<_, String>(3)?,
                    number(r, 4)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let total: u64 = entries.iter().map(|e| e.2).sum();
        Ok(
            json!({"schema_version":1,"payload_bytes":total,"database_bytes":fs::metadata(self.root.join(FILE))?.len(),
            "entries":entries.iter().map(|e| json!({"key":e.0,"kind":e.1,"bytes":e.2,"sha256":e.3,"last_use":e.4})).collect::<Vec<_>>()}),
        )
    }
}
