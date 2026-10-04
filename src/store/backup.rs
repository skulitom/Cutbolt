//! Consistent local history copies, validated before no-overwrite publication.
use super::*;
use crate::{media, render, scene};
use serde_json::Value;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};
const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// session.backup request: writes a checked, consistent copy of a session store's history (up to 256 MiB).
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Backup {
    /// Existing session store directory to copy.
    pub store_root: PathBuf,
    /// Existing absolute directory that must contain `output`.
    pub output_root: PathBuf,
    /// Unused absolute `.sqlite3` path inside `output_root`.
    pub output: PathBuf,
}
/// session.recover request: restores a backup as `projects.sqlite3` in an empty store location, without migration.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recover {
    /// Backup file identity (bytes, SHA-256); path absolute or relative, inside `input_root`; at most 256 MiB.
    pub source: scene::Identity,
    /// Existing absolute directory containing the backup.
    pub input_root: PathBuf,
    /// Existing absolute directory with no projects.sqlite3 or journal, WAL or shared-memory sidecar.
    pub store_root: PathBuf,
}

struct Scratch(PathBuf);
impl Scratch {
    fn new(output: &Path) -> Result<Self> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| error("CLOCK_ERROR", "Clock is before epoch"))?
            .as_nanos();
        let path = output.with_file_name(format!(
            ".cutbolt-history-{}-{stamp}.sqlite3",
            std::process::id()
        ));
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn inspect(path: &Path) -> Result<Value> {
    if path.metadata()?.len() > MAX_BYTES {
        return Err(error("LIMIT_EXCEEDED", "History backup exceeds 256 MiB"));
    }
    let mut connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "trusted_schema", false)?;
    let tx = connection.transaction()?;
    integrity::validate(&tx)
}
fn receipt(path: &Path, store: Value) -> Result<Value> {
    let identity =
        json!({"path":path,"bytes":path.metadata()?.len(),"sha256":media::file_hash(path)?});
    Ok(
        json!({"output":path,"identity":identity,"store":store,"media_copied":false,"history_preserved":true}),
    )
}
pub fn backup(request: &Backup) -> Result<Value> {
    let output = render::destination_extension(&request.output, &request.output_root, "sqlite3")?;
    let connection = connect(&request.store_root, false)?;
    let source = request.store_root.join("projects.sqlite3");
    if source.metadata()?.len() > MAX_BYTES {
        return Err(error(
            "LIMIT_EXCEEDED",
            "History backup supports stores up to 256 MiB",
        ));
    }
    let scratch = Scratch::new(&output)?;
    let path = scratch
        .0
        .to_str()
        .ok_or_else(|| error("INVALID_PATH", "Backup path must be Unicode"))?;
    // The public SQLite statement creates a transactionally consistent copy
    // while leaving the live history and its committed request records intact.
    connection.execute("VACUUM main INTO ?1", [path])?;
    let store = inspect(&scratch.0)?;
    OpenOptions::new()
        .write(true)
        .open(&scratch.0)?
        .sync_all()?;
    let mut result = receipt(&scratch.0, store)?;
    #[cfg(test)]
    test_point("backup_before_publish");
    media::publish(&scratch.0, &output)?;
    #[cfg(test)]
    test_point("backup_after_publish");
    result["output"] = json!(output);
    result["identity"]["path"] = json!(output);
    Ok(result)
}
fn no_sidecars(root: &Path) -> Result<()> {
    for suffix in ["-journal", "-wal", "-shm"] {
        if root
            .join(format!("projects.sqlite3{suffix}"))
            .try_exists()?
        {
            return Err(error(
                "OUTPUT_EXISTS",
                "Recovery requires no existing project database or journal files",
            ));
        }
    }
    Ok(())
}
pub fn recover(request: &Recover) -> Result<Value> {
    let output = render::destination_extension(
        &request.store_root.join("projects.sqlite3"),
        &request.store_root,
        "sqlite3",
    )?;
    no_sidecars(&request.store_root)?;
    let identity = crate::registry::Identity {
        sha256: request.source.sha256.clone(),
        bytes: request.source.bytes,
    };
    identity.validate()?;
    if identity.bytes > MAX_BYTES {
        return Err(error("LIMIT_EXCEEDED", "History backup exceeds 256 MiB"));
    }
    let source = media::project_file(&request.source.path, &request.input_root)?;
    if source.metadata()?.len() != identity.bytes || media::file_hash(&source)? != identity.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            "Backup differs from its declared identity",
        ));
    }
    let scratch = Scratch::new(&output)?;
    let mut input = fs::File::open(&source)?.take(identity.bytes + 1);
    let mut file = OpenOptions::new().write(true).open(&scratch.0)?;
    let copied = std::io::copy(&mut input, &mut file)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    if copied != identity.bytes || media::file_hash(&scratch.0)? != identity.sha256 {
        return Err(error("MEDIA_CHANGED", "Backup changed during copying"));
    }
    let store = inspect(&scratch.0)?;
    no_sidecars(&request.store_root)?;
    if media::file_hash(&source)? != identity.sha256 {
        return Err(error("MEDIA_CHANGED", "Backup changed before publication"));
    }
    #[cfg(test)]
    test_point("recovery_before_publish");
    media::publish(&scratch.0, &output)?;
    #[cfg(test)]
    test_point("recovery_after_publish");
    Ok(
        json!({"store_root":request.store_root,"database":output,"store":store,"source":request.source,"history_preserved":true,"media_copied":false,"migration_performed":false}),
    )
}
