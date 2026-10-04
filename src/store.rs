//! Local transactional projects. Snapshot, head, undo pointer and receipt commit together.
use crate::{
    Result, error,
    model::{Clip, Operation, Project},
    time::Time,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

const APPLICATION_ID: i64 = 0x45565431;
const STORE_VERSION: i64 = 2;
mod backup;
mod integrity;
pub use backup::{Backup, Recover, backup, recover};
pub use integrity::{check, migrate};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Placement {
    pub index: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    pub timeline_start: Time,
    pub clip: Clip,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipChange {
    pub clip_id: String,
    pub before: Option<Placement>,
    pub after: Option<Placement>,
}

/// More clips than this that only moved in time are summarized instead of listed one by one.
const SHIFT_LISTING: usize = 32;

/// Clips an edit only moved in time (same content, track and sequence), summarized when there are
/// more than SHIFT_LISTING of them so a ripple through a long timeline stays a small receipt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Shifted {
    pub count: usize,
    pub clip_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub earlier_by: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub later_by: Option<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Changes {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<SequenceChange>,
    pub duration_before: Time,
    pub duration_after: Time,
    pub clips: Vec<ClipChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shifted: Option<Shifted>,
    pub added_assets: Vec<String>,
    pub removed_assets: Vec<String>,
    pub modified_assets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<PreviewChange>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track_layout: Option<TrackLayoutChange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequenceChange {
    pub id: String,
    pub before: Option<crate::sequences::Sequence>,
    pub after: Option<crate::sequences::Sequence>,
    pub root_instances: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackState {
    pub id: String,
    pub kind: crate::tracks::Kind,
    pub enabled: bool,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<crate::tracks::Transition>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackLayout {
    pub tracks: Vec<TrackState>,
    pub links: Vec<crate::tracks::Link>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrackLayoutChange {
    pub before: Option<TrackLayout>,
    pub after: Option<TrackLayout>,
}
fn track_layout(project: &Project) -> Option<TrackLayout> {
    project.tracks.as_ref().map(|a| TrackLayout {
        tracks: a
            .tracks
            .iter()
            .map(|t| TrackState {
                id: t.id.clone(),
                kind: t.kind,
                enabled: t.enabled,
                locked: t.locked,
                transitions: t.transitions.clone(),
            })
            .collect(),
        links: a.links.clone(),
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PreviewChange {
    pub scale_before: Option<u32>,
    pub scale_after: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Receipt {
    pub project_id: String,
    pub request_id: String,
    pub action: String,
    pub revision: u64,
    pub parent_revision: Option<u64>,
    pub restored_from: Option<u64>,
    pub changes: Changes,
}

#[derive(Debug, Serialize)]
pub struct HistoryEntry {
    pub revision: u64,
    pub parent_revision: Option<u64>,
    pub restored_from: Option<u64>,
    pub request_id: String,
    pub action: String,
    pub changed_clips: u64,
}

#[derive(Debug, Serialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
    pub next_before_revision: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Mutation {
    Apply { operations: Vec<Operation> },
    Undo,
    Restore { target_revision: u64 },
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn check_id(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(error(
            "INVALID_ID",
            "Project/request IDs must contain 1-128 bytes",
        ));
    }
    Ok(())
}

fn connect(root: &Path, create: bool) -> Result<Connection> {
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Store root must be an existing absolute local directory",
        ));
    }
    let root = root.canonicalize()?;
    let path = root.join("projects.sqlite3");
    if path.exists() {
        if path.symlink_metadata()?.file_type().is_symlink()
            || path.canonicalize()? != path
            || !path.is_file()
        {
            return Err(error(
                "INVALID_PATH",
                "Database must be a regular file directly inside the store root",
            ));
        }
    } else if !create {
        return Err(error("STORE_NOT_FOUND", "This root has no project store"));
    }
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if create {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let mut connection = Connection::open_with_flags(path, flags)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "synchronous", "EXTRA")?;
    let journal: String = connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
    if journal != "delete" {
        return Err(error(
            "UNSUPPORTED_STORE",
            "Store requires DELETE journal mode",
        ));
    }
    if create {
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application: i64 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application == 0 && version == 0 {
            let tables: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if tables != 0 {
                return Err(error(
                    "UNSUPPORTED_STORE",
                    "Refusing to initialize an unrelated database",
                ));
            }
            tx.execute_batch(
                "CREATE TABLE projects (id TEXT PRIMARY KEY, head INTEGER NOT NULL);
                 CREATE TABLE revisions (
                   project_id TEXT NOT NULL REFERENCES projects(id), revision INTEGER NOT NULL,
                   parent_revision INTEGER, undo_target INTEGER, restored_from INTEGER,
                   action TEXT NOT NULL, request_id TEXT NOT NULL, changed_clips INTEGER NOT NULL,
                   snapshot TEXT NOT NULL, snapshot_hash TEXT NOT NULL,
                   PRIMARY KEY(project_id, revision), UNIQUE(project_id, request_id),
                   FOREIGN KEY(project_id, undo_target) REFERENCES revisions(project_id, revision));
                 CREATE TABLE requests (
                   project_id TEXT NOT NULL REFERENCES projects(id), request_id TEXT NOT NULL,
                   payload_hash TEXT NOT NULL, receipt TEXT NOT NULL, receipt_hash TEXT NOT NULL,
                   PRIMARY KEY(project_id, request_id));",
            )?;
            tx.execute_batch(integrity::TABLE)?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", STORE_VERSION)?;
        } else if application != APPLICATION_ID || ![1, STORE_VERSION].contains(&version) {
            return Err(error(
                "UNSUPPORTED_STORE",
                "Unrecognized database or unsupported store version",
            ));
        }
        tx.commit()?;
    } else {
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application != APPLICATION_ID || ![1, STORE_VERSION].contains(&version) {
            return Err(error(
                "UNSUPPORTED_STORE",
                "Unrecognized database or unsupported store version",
            ));
        }
    }
    #[cfg(test)]
    if std::env::var_os("VIDEO_ENGINE_TEST_POINT").is_some() {
        connection.pragma_update(None, "cache_size", 1)?;
        connection.pragma_update(None, "cache_spill", true)?;
    }
    Ok(connection)
}

fn sql_number(value: u64) -> Result<i64> {
    if value > 9_007_199_254_740_991 {
        return Err(error(
            "INVALID_REVISION",
            "Revision exceeds JSON integer range",
        ));
    }
    Ok(value as i64)
}

fn saved_number(value: i64) -> Result<u64> {
    if !(0..=9_007_199_254_740_991).contains(&value) {
        return Err(error("STORE_CORRUPT", "Invalid stored number"));
    }
    Ok(value as u64)
}

/// PROJECT_NOT_FOUND naming `project_id` and the first project IDs saved in this store.
fn missing_project(connection: &Connection, project_id: &str) -> Result<crate::Error> {
    let total: i64 = connection.query_row("SELECT count(*) FROM projects", [], |r| r.get(0))?;
    let first = connection
        .prepare("SELECT id FROM projects ORDER BY id LIMIT ?1")?
        .query_map([crate::LISTED_IDS as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(crate::missing_listed(
        "PROJECT_NOT_FOUND",
        "project",
        project_id,
        &first,
        total as usize,
    ))
}

fn head(connection: &Connection, project_id: &str) -> Result<u64> {
    check_id(project_id)?;
    let value: Option<i64> = connection
        .query_row("SELECT head FROM projects WHERE id=?1", [project_id], |r| {
            r.get(0)
        })
        .optional()?;
    let Some(value) = value else {
        return Err(missing_project(connection, project_id)?);
    };
    let value = saved_number(value)?;
    let latest: Option<i64> = connection.query_row(
        "SELECT max(revision) FROM revisions WHERE project_id=?1",
        [project_id],
        |r| r.get(0),
    )?;
    if latest != Some(sql_number(value)?) {
        return Err(error(
            "STORE_CORRUPT",
            "Project head does not match its latest revision",
        ));
    }
    Ok(value)
}

fn load(
    connection: &Connection,
    project_id: &str,
    revision: u64,
) -> Result<(Project, Option<u64>)> {
    let row: Option<(String, String, Option<i64>)> = connection.query_row(
        "SELECT snapshot,snapshot_hash,undo_target FROM revisions WHERE project_id=?1 AND revision=?2",
        params![project_id, sql_number(revision)?], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))
    ).optional()?;
    let (snapshot, hash, undo) = row.ok_or_else(|| {
        error(
            "REVISION_NOT_FOUND",
            format!("Revision {revision} not found"),
        )
    })?;
    if digest(snapshot.as_bytes()) != hash {
        return Err(error("STORE_CORRUPT", "Snapshot checksum mismatch"));
    }
    integrity::verify_row(connection, project_id, revision)?;
    let project: Project = serde_json::from_str(&snapshot)
        .map_err(|_| error("STORE_CORRUPT", "Invalid saved project JSON"))?;
    project
        .validate()
        .map_err(|_| error("STORE_CORRUPT", "Invalid saved project"))?;
    if project.id != project_id || project.revision != revision {
        return Err(error("STORE_CORRUPT", "Snapshot identity mismatch"));
    }
    Ok((project, undo.map(saved_number).transpose()?))
}

fn replay(
    connection: &Connection,
    project_id: &str,
    request_id: &str,
    payload_hash: &str,
) -> Result<Option<Receipt>> {
    match recorded(connection, project_id, request_id)? {
        Some((recorded, _)) if recorded != payload_hash => Err(error(
            "REQUEST_ID_CONFLICT",
            "Request ID was already committed with different arguments",
        )),
        found => Ok(found.map(|(_, receipt)| receipt)),
    }
}

/// The committed request fingerprint and verified receipt for a request ID, if any.
fn recorded(
    connection: &Connection,
    project_id: &str,
    request_id: &str,
) -> Result<Option<(String, Receipt)>> {
    let row: Option<(String, String, String)> = connection.query_row(
        "SELECT payload_hash,receipt,receipt_hash FROM requests WHERE project_id=?1 AND request_id=?2",
        params![project_id,request_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))
    ).optional()?;
    if let Some((recorded, receipt, hash)) = row {
        if digest(receipt.as_bytes()) != hash {
            return Err(error("STORE_CORRUPT", "Receipt checksum mismatch"));
        }
        let receipt: Receipt = serde_json::from_str(&receipt)
            .map_err(|_| error("STORE_CORRUPT", "Invalid saved receipt"))?;
        if receipt.project_id != project_id || receipt.request_id != request_id {
            return Err(error("STORE_CORRUPT", "Saved receipt identity mismatch"));
        }
        integrity::verify_row(connection, project_id, receipt.revision)?;
        return Ok(Some((recorded, receipt)));
    }
    Ok(None)
}

/// Read-only lookup of a committed request: lets an agent resuming after a lost response or crash
/// learn whether a request ID took effect, without resending its arguments.
pub fn receipt(root: &Path, project_id: &str, request_id: &str) -> Result<Receipt> {
    check_id(request_id)?;
    let mut connection = connect(root, false)?;
    let tx = connection.transaction()?;
    head(&tx, project_id)?;
    recorded(&tx, project_id, request_id)?
        .map(|(_, receipt)| receipt)
        .ok_or_else(|| {
            error(
                "REQUEST_NOT_FOUND",
                format!("No committed request {request_id:?} in project {project_id:?}"),
            )
        })
}

fn placements(project: &Project) -> Result<BTreeMap<String, Placement>> {
    let mut start = Time::ZERO;
    let mut result = BTreeMap::new();
    if let Some(a) = &project.tracks {
        for track in &a.tracks {
            for (index, clip) in track.clips.iter().enumerate() {
                result.insert(
                    clip.id.clone(),
                    Placement {
                        index,
                        track_id: Some(track.id.clone()),
                        sequence_id: clip.sequence_id.clone(),
                        timeline_start: clip.start,
                        clip: clip.legacy(),
                    },
                );
            }
        }
        return Ok(result);
    }
    for (index, clip) in project.clips.iter().enumerate() {
        result.insert(
            clip.id.clone(),
            Placement {
                index,
                track_id: None,
                sequence_id: None,
                timeline_start: start,
                clip: clip.clone(),
            },
        );
        start = start.plus(clip.duration)?;
    }
    Ok(result)
}

pub fn diff(before: &Project, after: &Project) -> Result<Changes> {
    before.validate()?;
    after.validate()?;
    let old = placements(before)?;
    let new = placements(after)?;
    let ids: BTreeSet<_> = old.keys().chain(new.keys()).collect();
    let clips = ids
        .into_iter()
        .filter(|id| match (old.get(*id), new.get(*id)) {
            (Some(a), Some(b)) if a.track_id.is_some() && b.track_id.is_some() => {
                // Array order within a placed track does not change its playback.
                a.track_id != b.track_id
                    || a.sequence_id != b.sequence_id
                    || a.timeline_start != b.timeline_start
                    || a.clip != b.clip
            }
            (a, b) => a != b,
        })
        .map(|id| ClipChange {
            clip_id: id.clone(),
            before: old.get(id).cloned(),
            after: new.get(id).cloned(),
        })
        .collect();
    let (clips, shifted) = summarize_shifts(clips)?;
    let old_assets: BTreeMap<_, _> = before.assets.iter().map(|a| (&a.id, a)).collect();
    let new_assets: BTreeMap<_, _> = after.assets.iter().map(|a| (&a.id, a)).collect();
    Ok(Changes {
        sequences: {
            let old: BTreeMap<_, _> = before.sequences.iter().map(|s| (&s.id, s)).collect();
            let new: BTreeMap<_, _> = after.sequences.iter().map(|s| (&s.id, s)).collect();
            let ids: BTreeSet<_> = old.keys().chain(new.keys()).copied().collect();
            ids.into_iter()
                .filter(|id| old.get(id) != new.get(id))
                .map(|id| SequenceChange {
                    id: id.clone(),
                    before: old.get(id).map(|s| (*s).clone()),
                    after: new.get(id).map(|s| (*s).clone()),
                    root_instances: crate::sequences::root_instances(before, id)
                        .into_iter()
                        .chain(crate::sequences::root_instances(after, id))
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect(),
                })
                .collect()
        },
        track_layout: {
            let before = track_layout(before);
            let after = track_layout(after);
            (before != after).then_some(TrackLayoutChange { before, after })
        },
        preview: (before.preview_scale != after.preview_scale).then_some(PreviewChange {
            scale_before: before.preview_scale,
            scale_after: after.preview_scale,
        }),
        duration_before: before.duration()?,
        duration_after: after.duration()?,
        clips,
        shifted,
        added_assets: new_assets
            .keys()
            .filter(|id| !old_assets.contains_key(*id))
            .map(|id| (*id).clone())
            .collect(),
        removed_assets: old_assets
            .keys()
            .filter(|id| !new_assets.contains_key(*id))
            .map(|id| (*id).clone())
            .collect(),
        modified_assets: new_assets
            .iter()
            .filter(|(id, a)| old_assets.get(*id).is_some_and(|old| old != *a))
            .map(|(id, _)| (*id).clone())
            .collect(),
    })
}

/// Split off clips that only moved in time when there are many of them.
fn summarize_shifts(clips: Vec<ClipChange>) -> Result<(Vec<ClipChange>, Option<Shifted>)> {
    let moved_only = |c: &ClipChange| match (&c.before, &c.after) {
        (Some(a), Some(b)) => {
            a.clip == b.clip && a.track_id == b.track_id && a.sequence_id == b.sequence_id
        }
        _ => false,
    };
    if clips.iter().filter(|c| moved_only(c)).count() <= SHIFT_LISTING {
        return Ok((clips, None));
    }
    let (moved, kept): (Vec<_>, Vec<_>) = clips.into_iter().partition(|c| moved_only(c));
    let mut offsets = Vec::new();
    for change in &moved {
        let (a, b) = (
            change.before.as_ref().unwrap(),
            change.after.as_ref().unwrap(),
        );
        offsets.push(match a.timeline_start.compare(b.timeline_start)? {
            std::cmp::Ordering::Greater => (true, a.timeline_start.minus(b.timeline_start)?),
            _ => (false, b.timeline_start.minus(a.timeline_start)?),
        });
    }
    let uniform = offsets.iter().all(|o| *o == offsets[0]).then(|| offsets[0]);
    let shifted = Shifted {
        count: moved.len(),
        clip_ids: moved.into_iter().map(|c| c.clip_id).collect(),
        earlier_by: uniform.and_then(|(earlier, by)| earlier.then_some(by)),
        later_by: uniform.and_then(|(earlier, by)| (!earlier).then_some(by)),
    };
    Ok((kept, Some(shifted)))
}

fn record(
    connection: &Connection,
    project: &Project,
    undo_target: Option<u64>,
    receipt: &Receipt,
    payload_hash: &str,
) -> Result<()> {
    if integrity::version(connection)? != STORE_VERSION {
        return Err(error(
            "MIGRATION_REQUIRED",
            "Run session.migrate explicitly before writing this older store",
        ));
    }
    let snapshot = serde_json::to_string(project)?;
    connection.execute(
        "INSERT INTO revisions(project_id,revision,parent_revision,undo_target,restored_from,action,request_id,changed_clips,snapshot,snapshot_hash)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![project.id,sql_number(project.revision)?,receipt.parent_revision.map(sql_number).transpose()?,undo_target.map(sql_number).transpose()?,receipt.restored_from.map(sql_number).transpose()?,receipt.action,receipt.request_id,
            receipt.changes.clips.len() as i64,snapshot,digest(snapshot.as_bytes())]
    )?;
    #[cfg(test)]
    test_point("after_snapshot");
    connection.execute(
        "UPDATE projects SET head=?1 WHERE id=?2",
        params![sql_number(project.revision)?, project.id],
    )?;
    let encoded = serde_json::to_string(receipt)?;
    connection.execute("INSERT INTO requests(project_id,request_id,payload_hash,receipt,receipt_hash) VALUES(?1,?2,?3,?4,?5)",
        params![project.id,receipt.request_id,payload_hash,encoded,digest(encoded.as_bytes())])?;
    integrity::seal(connection, &project.id, project.revision)?;
    #[cfg(test)]
    test_point("before_commit");
    Ok(())
}

pub fn create(root: &Path, mut project: Project, request_id: &str) -> Result<Receipt> {
    project.validate()?;
    check_id(request_id)?;
    let hash = digest(&serde_json::to_vec(
        &json!({"contract":1,"action":"create","project":project}),
    )?);
    let mut connection = connect(root, true)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(receipt) = replay(&tx, &project.id, request_id, &hash)? {
        return Ok(receipt);
    }
    let exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
        [&project.id],
        |r| r.get(0),
    )?;
    if exists {
        return Err(error(
            "PROJECT_EXISTS",
            "Project already exists; use a new ID or open its session",
        ));
    }
    project.revision = 0;
    let empty = Project::new(
        project.id.clone(),
        project.width,
        project.height,
        project.frame_rate,
    )?;
    let receipt = Receipt {
        project_id: project.id.clone(),
        request_id: request_id.into(),
        action: "create".into(),
        revision: 0,
        parent_revision: None,
        restored_from: None,
        changes: diff(&empty, &project)?,
    };
    tx.execute("INSERT INTO projects(id,head) VALUES(?1,0)", [&project.id])?;
    record(&tx, &project, None, &receipt, &hash)?;
    tx.commit()?;
    #[cfg(test)]
    test_point("after_commit");
    Ok(receipt)
}

pub fn get(root: &Path, project_id: &str, revision: Option<u64>) -> Result<Project> {
    let mut connection = connect(root, false)?;
    let tx = connection.transaction()?;
    let current = head(&tx, project_id)?;
    Ok(load(&tx, project_id, revision.unwrap_or(current))?.0)
}

pub fn mutate(
    root: &Path,
    project_id: &str,
    request_id: &str,
    expected_revision: u64,
    mutation: Mutation,
) -> Result<Receipt> {
    check_id(project_id)?;
    check_id(request_id)?;
    let hash = digest(&serde_json::to_vec(
        &json!({"contract":1,"project_id":project_id,"expected_revision":expected_revision,"mutation":mutation}),
    )?);
    let mut connection = connect(root, false)?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Replay precedes revision checks: the project may have advanced after a lost reply.
    if let Some(receipt) = replay(&tx, project_id, request_id, &hash)? {
        return Ok(receipt);
    }
    let current = head(&tx, project_id)?;
    if current != expected_revision {
        return Err(error(
            "REVISION_CONFLICT",
            format!("Expected {expected_revision}; current revision is {current}"),
        ));
    }
    let (before, prior_undo) = load(&tx, project_id, current)?;
    let (mut next, undo_target, action, restored_from) = match mutation {
        Mutation::Apply { operations } => (
            before.apply(current, operations)?,
            Some(current),
            "apply",
            None,
        ),
        Mutation::Undo => {
            let target = prior_undo
                .ok_or_else(|| error("NOTHING_TO_UNDO", "The session is at its initial state"))?;
            let (snapshot, next_undo) = load(&tx, project_id, target)?;
            (snapshot, next_undo, "undo", Some(target))
        }
        Mutation::Restore { target_revision } => (
            load(&tx, project_id, target_revision)?.0,
            Some(current),
            "restore",
            Some(target_revision),
        ),
    };
    next.revision = current
        .checked_add(1)
        .ok_or_else(|| error("LIMIT_EXCEEDED", "Revision overflow"))?;
    next.validate()?;
    let receipt = Receipt {
        project_id: project_id.into(),
        request_id: request_id.into(),
        action: action.into(),
        revision: next.revision,
        parent_revision: Some(current),
        restored_from,
        changes: diff(&before, &next)?,
    };
    record(&tx, &next, undo_target, &receipt, &hash)?;
    tx.commit()?;
    #[cfg(test)]
    test_point("after_commit");
    Ok(receipt)
}

pub fn preview(
    root: &Path,
    project_id: &str,
    expected_revision: u64,
    operations: Vec<Operation>,
) -> Result<Changes> {
    let before = get(root, project_id, None)?;
    let after = before.apply(expected_revision, operations)?;
    diff(&before, &after)
}

pub fn history(
    root: &Path,
    project_id: &str,
    before_revision: Option<u64>,
    limit: u16,
) -> Result<History> {
    if limit == 0 || limit > 200 {
        return Err(error("INVALID_LIMIT", "History limit must be 1-200"));
    }
    let mut connection = connect(root, false)?;
    let tx = connection.transaction()?;
    let current = head(&tx, project_id)?;
    let before = before_revision.unwrap_or(current + 1).min(current + 1);
    let mut statement = tx.prepare("SELECT revision,parent_revision,restored_from,request_id,action,changed_clips FROM revisions
        WHERE project_id=?1 AND revision<?2 ORDER BY revision DESC LIMIT ?3")?;
    let rows = statement
        .query_map(
            params![project_id, sql_number(before)?, u32::from(limit) + 1],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, Option<i64>>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut entries = rows
        .into_iter()
        .map(
            |(revision, parent, restored, request_id, action, changed)| {
                integrity::verify_row(&tx, project_id, saved_number(revision)?)?;
                Ok(HistoryEntry {
                    revision: saved_number(revision)?,
                    parent_revision: parent.map(saved_number).transpose()?,
                    restored_from: restored.map(saved_number).transpose()?,
                    request_id,
                    action,
                    changed_clips: saved_number(changed)?,
                })
            },
        )
        .collect::<Result<Vec<_>>>()?;
    let more = entries.len() > usize::from(limit);
    entries.truncate(usize::from(limit));
    let next_before_revision = if more {
        entries.last().map(|e| e.revision)
    } else {
        None
    };
    Ok(History {
        entries,
        next_before_revision,
    })
}

#[cfg(test)]
fn test_point(point: &str) {
    if std::env::var("VIDEO_ENGINE_TEST_POINT").as_deref() == Ok(point) {
        std::process::exit(86);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Asset;
    use std::{
        fs,
        path::PathBuf,
        process::Command,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            static COUNT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "cutbolt-store-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                COUNT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            // Only remove the known database files, never recursively traverse a test path.
            for name in [
                "projects.sqlite3",
                "projects.sqlite3-journal",
                "projects.sqlite3-wal",
                "projects.sqlite3-shm",
            ] {
                let _ = fs::remove_file(self.0.join(name));
            }
            let _ = fs::remove_dir(&self.0);
        }
    }
    fn t(n: u64) -> Time {
        Time::new(n, 1).unwrap()
    }
    fn sample() -> Project {
        let mut project = Project::new("test".into(), 320, 180, t(25)).unwrap();
        project.assets.push(Asset {
            metadata: Default::default(),
            identity: None,
            proxy: None,
            id: "a".into(),
            path: "external.mkv".into(),
            duration: t(20),
        });
        for id in ["c", "d"] {
            project.clips.push(Clip {
                id: id.into(),
                asset_id: Some("a".into()),
                gap: false,
                source_in: t(0),
                duration: t(5),
            });
        }
        project
    }
    #[test]
    fn long_ripples_summarize_clips_that_only_moved() {
        let long = |count: usize| {
            let mut project = sample();
            project.clips = (0..count)
                .map(|i| Clip {
                    id: format!("c{i}"),
                    asset_id: Some("a".into()),
                    gap: false,
                    source_in: t(0),
                    duration: t(2),
                })
                .collect();
            project
        };
        for (count, listed) in [
            (SHIFT_LISTING + 1, SHIFT_LISTING + 1),
            (SHIFT_LISTING + 2, 1),
        ] {
            let before = long(count);
            let mut after = before.clone();
            after.clips[0].duration = t(1);
            let changes = diff(&before, &after).unwrap();
            assert_eq!(changes.clips.len(), listed, "{count} clips");
            assert_eq!(changes.clips[0].clip_id, "c0");
            if listed == 1 {
                let shifted = changes.shifted.unwrap();
                assert_eq!(shifted.count, count - 1);
                assert_eq!(shifted.earlier_by, Some(t(1)));
                assert_eq!(shifted.later_by, None);
                assert_eq!(shifted.clip_ids[0], "c1");
            } else {
                assert!(changes.shifted.is_none());
            }
        }
    }

    fn trim(duration: u64) -> Mutation {
        Mutation::Apply {
            operations: vec![Operation::Trim {
                clip_id: "c".into(),
                source_in: t(0),
                duration: t(duration),
            }],
        }
    }
    fn seed() -> Temp {
        let dir = Temp::new();
        create(&dir.0, sample(), "create").unwrap();
        dir
    }

    #[test]
    fn durable_replay_after_advance_and_conflicting_key() {
        let dir = seed();
        let first = mutate(&dir.0, "test", "edit", 0, trim(3)).unwrap();
        mutate(&dir.0, "test", "edit2", 1, trim(2)).unwrap();
        assert_eq!(mutate(&dir.0, "test", "edit", 0, trim(3)).unwrap(), first);
        assert_eq!(get(&dir.0, "test", None).unwrap().revision, 2);
        assert_eq!(
            get(&dir.0, "test", Some(1)).unwrap().clips[0].duration,
            t(3)
        );
        assert_eq!(
            mutate(&dir.0, "test", "edit", 0, trim(4)).unwrap_err().code,
            "REQUEST_ID_CONFLICT"
        );
        assert_eq!(
            mutate(&dir.0, "test", "stale", 0, trim(4))
                .unwrap_err()
                .code,
            "REVISION_CONFLICT"
        );
        assert_eq!(create(&dir.0, sample(), "create").unwrap().revision, 0);
        assert_eq!(get(&dir.0, "test", None).unwrap().revision, 2);
        assert_eq!(history(&dir.0, "test", None, 50).unwrap().entries.len(), 3);
    }

    #[test]
    fn failed_batch_and_database_failure_leave_no_state_or_receipt() {
        let dir = seed();
        let failed = Mutation::Apply {
            operations: vec![
                Operation::Remove {
                    clip_id: "c".into(),
                },
                Operation::Remove {
                    clip_id: "missing".into(),
                },
            ],
        };
        assert_eq!(
            mutate(&dir.0, "test", "edit", 0, failed).unwrap_err().code,
            "MISSING_CLIP"
        );
        assert_eq!(get(&dir.0, "test", None).unwrap(), sample());
        let connection = connect(&dir.0, false).unwrap();
        connection.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON requests BEGIN SELECT RAISE(ABORT, 'simulated write failure'); END;").unwrap();
        assert!(mutate(&dir.0, "test", "edit", 0, trim(3)).is_err());
        assert_eq!(get(&dir.0, "test", None).unwrap(), sample());
        assert_eq!(history(&dir.0, "test", None, 50).unwrap().entries.len(), 1);
        connection
            .execute_batch("DROP TRIGGER fail_receipt")
            .unwrap();
        drop(connection);
        assert_eq!(
            mutate(&dir.0, "test", "edit", 0, trim(3)).unwrap().revision,
            1
        );
    }

    #[test]
    fn repeated_undo_restore_and_paginated_history() {
        let dir = seed();
        mutate(&dir.0, "test", "one", 0, trim(3)).unwrap();
        mutate(&dir.0, "test", "two", 1, trim(2)).unwrap();
        let undo = mutate(&dir.0, "test", "undo1", 2, Mutation::Undo).unwrap();
        assert_eq!(undo.restored_from, Some(1));
        assert_eq!(get(&dir.0, "test", None).unwrap().clips[0].duration, t(3));
        assert_eq!(
            mutate(&dir.0, "test", "undo1", 2, Mutation::Undo).unwrap(),
            undo
        );
        assert_eq!(
            mutate(&dir.0, "test", "undo2", 3, Mutation::Undo)
                .unwrap()
                .restored_from,
            Some(0)
        );
        assert_eq!(get(&dir.0, "test", None).unwrap().clips[0].duration, t(5));
        assert_eq!(
            mutate(&dir.0, "test", "undo3", 4, Mutation::Undo)
                .unwrap_err()
                .code,
            "NOTHING_TO_UNDO"
        );
        mutate(
            &dir.0,
            "test",
            "restore",
            4,
            Mutation::Restore { target_revision: 2 },
        )
        .unwrap();
        assert_eq!(get(&dir.0, "test", None).unwrap().clips[0].duration, t(2));
        mutate(&dir.0, "test", "undo-restore", 5, Mutation::Undo).unwrap();
        assert_eq!(get(&dir.0, "test", None).unwrap().clips[0].duration, t(5));
        // A new edit after undo branches content while retaining all old revisions.
        mutate(&dir.0, "test", "branch", 6, trim(4)).unwrap();
        mutate(&dir.0, "test", "undo-branch", 7, Mutation::Undo).unwrap();
        assert_eq!(get(&dir.0, "test", None).unwrap().clips[0].duration, t(5));
        let mut before = None;
        let mut revisions = Vec::new();
        loop {
            let page = history(&dir.0, "test", before, 2).unwrap();
            revisions.extend(page.entries.iter().map(|e| e.revision));
            before = page.next_before_revision;
            if before.is_none() {
                break;
            }
        }
        assert_eq!(revisions, (0..=8).rev().collect::<Vec<_>>());
        assert_eq!(
            history(&dir.0, "test", None, 0).unwrap_err().code,
            "INVALID_LIMIT"
        );
        assert_eq!(
            get(&dir.0, "test", Some(99)).unwrap_err().code,
            "REVISION_NOT_FOUND"
        );
        assert_eq!(
            get(&dir.0, "test", Some(u64::MAX)).unwrap_err().code,
            "INVALID_REVISION"
        );
    }

    #[test]
    fn preview_reports_ripple_positions_without_writes() {
        let dir = seed();
        let Mutation::Apply { operations } = trim(3) else {
            unreachable!()
        };
        let changes = preview(&dir.0, "test", 0, operations).unwrap();
        assert_eq!(changes.duration_before, t(10));
        assert_eq!(changes.duration_after, t(8));
        assert_eq!(changes.clips.len(), 2);
        let ripple = changes.clips.iter().find(|c| c.clip_id == "d").unwrap();
        assert_eq!(ripple.before.as_ref().unwrap().timeline_start, t(5));
        assert_eq!(ripple.after.as_ref().unwrap().timeline_start, t(3));
        assert_eq!(get(&dir.0, "test", None).unwrap(), sample());
        assert_eq!(history(&dir.0, "test", None, 50).unwrap().entries.len(), 1);
    }

    #[test]
    fn rejects_unrelated_future_and_corrupt_stores() {
        let dir = Temp::new();
        let connection = Connection::open(dir.0.join("projects.sqlite3")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE unrelated (value TEXT); INSERT INTO unrelated VALUES('keep');",
            )
            .unwrap();
        assert_eq!(
            create(&dir.0, sample(), "create").unwrap_err().code,
            "UNSUPPORTED_STORE"
        );
        assert_eq!(
            connection
                .query_row("SELECT value FROM unrelated", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "keep"
        );
        drop(connection);
        let dir = seed();
        let connection = connect(&dir.0, false).unwrap();
        connection.pragma_update(None, "user_version", 3).unwrap();
        assert_eq!(
            get(&dir.0, "test", None).unwrap_err().code,
            "UNSUPPORTED_STORE"
        );
        assert_eq!(
            create(&dir.0, sample(), "create").unwrap_err().code,
            "UNSUPPORTED_STORE"
        );
        connection
            .pragma_update(None, "user_version", STORE_VERSION)
            .unwrap();
        connection
            .execute("UPDATE revisions SET snapshot='{}'", [])
            .unwrap();
        assert_eq!(get(&dir.0, "test", None).unwrap_err().code, "STORE_CORRUPT");
        connection
            .execute("UPDATE requests SET receipt='{}'", [])
            .unwrap();
        assert_eq!(
            create(&dir.0, sample(), "create").unwrap_err().code,
            "STORE_CORRUPT"
        );
    }

    #[test]
    fn crash_worker() {
        let Some(root) = std::env::var_os("VIDEO_ENGINE_TEST_ROOT") else {
            return;
        };
        mutate(Path::new(&root), "test", "crash-edit", 0, trim(3)).unwrap();
        panic!("Crash injection did not fire");
    }
    fn crash(root: &Path, point: &str) {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "store::tests::crash_worker", "--nocapture"])
            .env("VIDEO_ENGINE_TEST_ROOT", root)
            .env("VIDEO_ENGINE_TEST_POINT", point)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(86), "{output:?}");
    }
    #[test]
    fn process_crash_before_commit_rolls_back() {
        for point in ["after_snapshot", "before_commit"] {
            let dir = seed();
            crash(&dir.0, point);
            assert_eq!(get(&dir.0, "test", None).unwrap(), sample());
            assert_eq!(history(&dir.0, "test", None, 50).unwrap().entries.len(), 1);
            let receipt = mutate(&dir.0, "test", "crash-edit", 0, trim(3)).unwrap();
            assert_eq!(receipt.revision, 1);
            assert_eq!(
                mutate(&dir.0, "test", "crash-edit", 0, trim(3)).unwrap(),
                receipt
            );
        }
    }
    #[test]
    fn process_crash_after_commit_replays_lost_response() {
        let dir = seed();
        crash(&dir.0, "after_commit");
        assert_eq!(get(&dir.0, "test", None).unwrap().revision, 1);
        let receipt = mutate(&dir.0, "test", "crash-edit", 0, trim(3)).unwrap();
        assert_eq!(receipt.revision, 1);
        assert_eq!(receipt.changes.duration_after, t(8));
        assert_eq!(history(&dir.0, "test", None, 50).unwrap().entries.len(), 2);
    }
}
