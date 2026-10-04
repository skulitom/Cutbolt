//! Original revision-metadata checks and explicit migration of genuine version-1 stores.
use super::*;
use serde_json::Value;

pub(super) const TABLE: &str = "CREATE TABLE revision_integrity (
    project_id TEXT NOT NULL, revision INTEGER NOT NULL, digest TEXT NOT NULL,
    PRIMARY KEY(project_id,revision),
    FOREIGN KEY(project_id,revision) REFERENCES revisions(project_id,revision))";

struct Row {
    project: String,
    revision: i64,
    parent: Option<i64>,
    undo: Option<i64>,
    restored: Option<i64>,
    action: String,
    request: String,
    changed: i64,
    snapshot: String,
    snapshot_hash: String,
    payload_hash: String,
    receipt: String,
    receipt_hash: String,
}
const COLUMNS: &str = "r.project_id,r.revision,r.parent_revision,r.undo_target,r.restored_from,r.action,r.request_id,r.changed_clips,r.snapshot,r.snapshot_hash,q.payload_hash,q.receipt,q.receipt_hash";
fn read(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok(Row {
        project: r.get(0)?,
        revision: r.get(1)?,
        parent: r.get(2)?,
        undo: r.get(3)?,
        restored: r.get(4)?,
        action: r.get(5)?,
        request: r.get(6)?,
        changed: r.get(7)?,
        snapshot: r.get(8)?,
        snapshot_hash: r.get(9)?,
        payload_hash: r.get(10)?,
        receipt: r.get(11)?,
        receipt_hash: r.get(12)?,
    })
}
fn select() -> String {
    format!(
        "SELECT {COLUMNS} FROM revisions r JOIN requests q ON r.project_id=q.project_id AND r.request_id=q.request_id"
    )
}
fn corrupt(message: &str) -> crate::Error {
    error("STORE_CORRUPT", message)
}
fn row(connection: &Connection, project: &str, revision: u64) -> Result<Row> {
    connection
        .query_row(
            &format!("{} WHERE r.project_id=?1 AND r.revision=?2", select()),
            params![project, sql_number(revision)?],
            read,
        )
        .optional()?
        .ok_or_else(|| corrupt("Revision or corresponding request is missing"))
}
fn checksum(row: &Row) -> Result<String> {
    Ok(digest(&serde_json::to_vec(&json!([
        row.project,
        row.revision,
        row.parent,
        row.undo,
        row.restored,
        row.action,
        row.request,
        row.changed,
        row.snapshot_hash,
        row.payload_hash,
        row.receipt_hash
    ]))?))
}
pub(super) fn version(connection: &Connection) -> Result<i64> {
    Ok(connection.pragma_query_value(None, "user_version", |r| r.get(0))?)
}
pub(super) fn seal(connection: &Connection, project: &str, revision: u64) -> Result<()> {
    let row = row(connection, project, revision)?;
    connection.execute(
        "INSERT INTO revision_integrity(project_id,revision,digest) VALUES(?1,?2,?3)",
        params![project, sql_number(revision)?, checksum(&row)?],
    )?;
    Ok(())
}
fn verify(connection: &Connection, row: &Row) -> Result<()> {
    if digest(row.snapshot.as_bytes()) != row.snapshot_hash
        || digest(row.receipt.as_bytes()) != row.receipt_hash
    {
        return Err(corrupt("Snapshot or receipt checksum mismatch"));
    }
    let stored: Option<String> = connection
        .query_row(
            "SELECT digest FROM revision_integrity WHERE project_id=?1 AND revision=?2",
            params![row.project, row.revision],
            |r| r.get(0),
        )
        .optional()?;
    if stored.as_ref() != Some(&checksum(row)?) {
        return Err(corrupt("Revision metadata checksum mismatch"));
    }
    Ok(())
}
pub(super) fn verify_row(connection: &Connection, project: &str, revision: u64) -> Result<()> {
    if version(connection)? == STORE_VERSION {
        verify(connection, &row(connection, project, revision)?)?;
    }
    Ok(())
}

pub(super) fn validate(connection: &Connection) -> Result<Value> {
    let version = version(connection)?;
    let app: i64 = connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    if app != APPLICATION_ID || ![1, STORE_VERSION].contains(&version) {
        return Err(error(
            "UNSUPPORTED_STORE",
            "Unrecognized database or unsupported store version",
        ));
    }
    let quick: String = connection.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
    if quick != "ok" {
        return Err(corrupt("SQLite integrity check failed"));
    }
    if connection.prepare("PRAGMA foreign_key_check")?.exists([])? {
        return Err(corrupt("Broken history reference"));
    }
    let objects = connection.prepare("SELECT type || ':' || name FROM sqlite_schema WHERE type!='index' AND name NOT LIKE 'sqlite_%' ORDER BY type,name")?
        .query_map([],|r|r.get::<_,String>(0))?.collect::<std::result::Result<BTreeSet<_>,_>>()?;
    let expected: BTreeSet<String> = if version == 1 {
        vec!["projects", "revisions", "requests"]
    } else {
        vec!["projects", "revisions", "requests", "revision_integrity"]
    }
    .into_iter()
    .map(|name| format!("table:{name}"))
    .collect();
    if objects != expected {
        return Err(error("UNSUPPORTED_STORE", "Unexpected database objects"));
    }
    let project_rows = connection
        .prepare("SELECT id,head FROM projects ORDER BY id")?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut revisions = 0i64;
    for (id, head) in &project_rows {
        check_id(id).map_err(|_| corrupt("Invalid project ID"))?;
        let head = saved_number(*head)?;
        let mut statement = connection.prepare(&format!(
            "{} WHERE r.project_id=?1 ORDER BY r.revision",
            select()
        ))?;
        let mut previous: Option<(Project, Option<i64>)> = None;
        let mut expected_revision = 0u64;
        for entry in statement.query_map([id], read)? {
            let entry = entry?;
            check_id(&entry.request).map_err(|_| corrupt("Invalid request ID"))?;
            if saved_number(entry.revision)? != expected_revision || entry.revision > head as i64 {
                return Err(corrupt("History has a missing revision or an invalid head"));
            }
            if digest(entry.snapshot.as_bytes()) != entry.snapshot_hash
                || digest(entry.receipt.as_bytes()) != entry.receipt_hash
            {
                return Err(corrupt("Snapshot or receipt checksum mismatch"));
            }
            if entry.payload_hash.len() != 64
                || !entry
                    .payload_hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(corrupt("Invalid request fingerprint"));
            }
            let project: Project = serde_json::from_str(&entry.snapshot)
                .map_err(|_| corrupt("Invalid snapshot JSON"))?;
            project
                .validate()
                .map_err(|_| corrupt("Invalid saved project"))?;
            let receipt: Receipt = serde_json::from_str(&entry.receipt)
                .map_err(|_| corrupt("Invalid receipt JSON"))?;
            if project.id != *id
                || project.revision != expected_revision
                || receipt.project_id != *id
                || receipt.revision != expected_revision
                || receipt.request_id != entry.request
                || receipt.action != entry.action
                || receipt.parent_revision.map(|v| v as i128) != entry.parent.map(i128::from)
                || receipt.restored_from.map(|v| v as i128) != entry.restored.map(i128::from)
                || receipt.changes.clips.len() as i64 != entry.changed
            {
                return Err(corrupt("Revision and receipt identities disagree"));
            }
            let empty = Project::new(
                id.clone(),
                project.width,
                project.height,
                project.frame_rate,
            )?;
            let before = previous.as_ref().map_or(&empty, |v| &v.0);
            if receipt.changes != diff(before, &project)? {
                return Err(corrupt("Stored changes do not match the revision"));
            }
            match entry.action.as_str() {
                "create"
                    if expected_revision == 0
                        && entry.parent.is_none()
                        && entry.undo.is_none()
                        && entry.restored.is_none() => {}
                "apply"
                    if expected_revision > 0
                        && entry.parent == Some(entry.revision - 1)
                        && entry.undo == entry.parent
                        && entry.restored.is_none() => {}
                "undo" | "restore"
                    if expected_revision > 0 && entry.parent == Some(entry.revision - 1) =>
                {
                    let target = entry
                        .restored
                        .ok_or_else(|| corrupt("Missing restored revision"))?;
                    if target < 0 || target >= entry.revision {
                        return Err(corrupt("Invalid restored revision"));
                    }
                    let (mut restored, target_undo) = load(connection, id, target as u64)?;
                    restored.revision = expected_revision;
                    if restored != project
                        || (entry.action == "restore" && entry.undo != entry.parent)
                        || (entry.action == "undo"
                            && (previous.as_ref().and_then(|v| v.1) != Some(target)
                                || entry.undo.map(saved_number).transpose()? != target_undo))
                    {
                        return Err(corrupt("Undo or restoration does not match history"));
                    }
                }
                _ => return Err(corrupt("Invalid history action or pointers")),
            }
            if version == STORE_VERSION {
                verify(connection, &entry)?;
            }
            previous = Some((project, entry.undo));
            expected_revision += 1;
            revisions += 1;
        }
        if expected_revision != head + 1 {
            return Err(corrupt("Project head has missing history"));
        }
    }
    for table in if version == 1 {
        vec!["revisions", "requests"]
    } else {
        vec!["revisions", "requests", "revision_integrity"]
    } {
        let count: i64 =
            connection.query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?;
        if count != revisions {
            return Err(corrupt("Orphaned or missing history records"));
        }
    }
    Ok(
        json!({"valid":true,"store_schema_version":version,"projects":project_rows.iter().map(|(id,head)|json!({"project_id":id,"revision":head})).collect::<Vec<_>>(),"revisions":revisions,"requests":revisions,"migration_required":version==1}),
    )
}

pub fn check(root: &Path) -> Result<Value> {
    let mut connection = connect(root, false)?;
    let tx = connection.transaction()?;
    validate(&tx)
}
pub fn migrate(root: &Path) -> Result<Value> {
    let mut connection = connect(root, false)?;
    migrate_connection(&mut connection)
}
fn migrate_connection(connection: &mut Connection) -> Result<Value> {
    let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let before = validate(&tx)?;
    if version(&tx)? == STORE_VERSION {
        return Ok(
            json!({"changed":false,"from_version":STORE_VERSION,"to_version":STORE_VERSION,"store":before}),
        );
    }
    tx.execute_batch(TABLE)?;
    {
        let mut statement =
            tx.prepare("SELECT project_id,revision FROM revisions ORDER BY project_id,revision")?;
        for row in statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (project, revision) = row?;
            seal(&tx, &project, saved_number(revision)?)?;
            #[cfg(test)]
            test_point("migration_after_seal");
        }
    }
    tx.pragma_update(None, "user_version", STORE_VERSION)?;
    #[cfg(test)]
    test_point("migration_before_commit");
    tx.commit()?;
    #[cfg(test)]
    test_point("migration_after_commit");
    Ok(
        json!({"changed":true,"from_version":1,"to_version":STORE_VERSION,"projects":before["projects"],"revisions":before["revisions"],"requests_preserved":true,"snapshots_preserved":true}),
    )
}

#[cfg(test)]
mod tests;
