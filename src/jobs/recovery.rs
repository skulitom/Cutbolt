//! Durable attempt and publication records for the local queue.
use super::*;

pub(super) fn migrate(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "ALTER TABLE jobs ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0 CHECK(attempt BETWEEN 0 AND 3);
        ALTER TABLE jobs ADD COLUMN max_attempts INTEGER NOT NULL DEFAULT 1 CHECK(max_attempts BETWEEN 1 AND 3);
        ALTER TABLE jobs ADD COLUMN source_manifest TEXT;
        ALTER TABLE jobs ADD COLUMN source_manifest_hash TEXT;
        CREATE TABLE job_attempts(job_id TEXT NOT NULL, attempt INTEGER NOT NULL,
            status TEXT NOT NULL, failure TEXT, PRIMARY KEY(job_id,attempt));
        CREATE TABLE job_publications(job_id TEXT NOT NULL, attempt INTEGER NOT NULL,
            receipt TEXT NOT NULL, receipt_hash TEXT NOT NULL, temp TEXT NOT NULL,
            output TEXT NOT NULL, PRIMARY KEY(job_id,attempt));
        UPDATE jobs SET attempt=1 WHERE status!='queued';
        INSERT INTO job_attempts SELECT id,attempt,status,failure FROM jobs WHERE attempt=1;",
    )?;
    Ok(())
}

pub(super) fn partial_name(id: &str, attempt: i64) -> String {
    if attempt == 1 {
        format!(".cutbolt-job-{id}.partial.mkv")
    } else {
        format!(".cutbolt-job-{id}.attempt-{attempt}.partial.mkv")
    }
}

pub(super) fn begin_attempt(connection: &Connection, id: &str) -> Result<()> {
    connection.execute("INSERT INTO job_attempts(job_id,attempt,status)
        SELECT id,attempt,'running' FROM jobs WHERE id=?1 AND attempt BETWEEN 1 AND max_attempts AND max_attempts BETWEEN 1 AND 3",[id])?
        .eq(&1).then_some(()).ok_or_else(||error("STORE_CORRUPT","Invalid saved attempt limit"))
}

pub(super) fn complete_attempt(connection: &Connection, id: &str) -> Result<()> {
    connection.execute(
        "UPDATE job_attempts SET status='completed',failure=NULL
        WHERE job_id=?1 AND attempt=(SELECT attempt FROM jobs WHERE id=?1)",
        [id],
    )?;
    Ok(())
}

pub(super) fn attempts(connection: &Connection, id: &str) -> Result<Value> {
    let (current, maximum): (i64, i64) = connection.query_row(
        "SELECT attempt,max_attempts FROM jobs WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut statement = connection.prepare(
        "SELECT attempt,status,failure FROM job_attempts WHERE job_id=?1 ORDER BY attempt",
    )?;
    let rows = statement.query_map([id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut history = Vec::new();
    for row in rows {
        let (number, status, failure) = row?;
        history.push(json!({"attempt":number,"status":status,"error":failure.map(|s|serde_json::from_str::<Value>(&s)).transpose()?}));
    }
    Ok(json!({"current":current,"maximum":maximum,"history":history}))
}

pub(super) fn pin_sources(
    connection: &Connection,
    id: &str,
    sources: &[render::Source],
) -> Result<()> {
    let mut sources = sources.to_vec();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    let encoded = serde_json::to_string(&sources)?;
    let tx = connection.unchecked_transaction()?;
    tx.execute("UPDATE jobs SET source_manifest=?2,source_manifest_hash=?3 WHERE id=?1 AND source_manifest IS NULL",params![id,encoded,digest(encoded.as_bytes())])?;
    let (stored, hash): (String, String) = tx.query_row(
        "SELECT source_manifest,source_manifest_hash FROM jobs WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if digest(stored.as_bytes()) != hash {
        return Err(error(
            "STORE_CORRUPT",
            "Saved source manifest checksum mismatch",
        ));
    }
    if encoded != stored {
        return Err(error(
            "MEDIA_CHANGED",
            "Render sources differ from the first validated attempt",
        ));
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn publication_intent(
    connection: &Connection,
    id: &str,
    temp: &Path,
    output: &Path,
    receipt: &Value,
) -> Result<()> {
    let encoded = serde_json::to_string(receipt)?;
    let tx = connection.unchecked_transaction()?;
    tx.execute("UPDATE jobs SET phase='publishing' WHERE id=?1", [id])?;
    let cancelled: bool =
        tx.query_row("SELECT cancel_requested FROM jobs WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    if cancelled {
        return Err(error(
            "JOB_CANCELLED",
            "Job cancelled; no output was published",
        ));
    }
    tx.execute(
        "INSERT INTO job_publications(job_id,attempt,receipt,receipt_hash,temp,output)
        SELECT id,attempt,?2,?3,?4,?5 FROM jobs WHERE id=?1",
        params![
            id,
            encoded,
            digest(encoded.as_bytes()),
            temp.to_string_lossy(),
            output.to_string_lossy()
        ],
    )?;
    tx.commit()?;
    Ok(())
}

/// None means no final file was published. An existing file is accepted only
/// against the durable, validated receipt for this exact request and attempt.
fn published(connection: &Connection, id: &str) -> Result<Option<Value>> {
    type Row = (String, String, String, String, String, String, i64, i64);
    let row:Option<Row>=connection.query_row("SELECT p.receipt,p.receipt_hash,p.temp,p.output,j.request,j.request_hash,j.attempt,j.total_frames
        FROM jobs j JOIN job_publications p ON p.job_id=j.id AND p.attempt=j.attempt WHERE j.id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))).optional()?;
    let Some((encoded, hash, temp, output, request, request_hash, attempt, frames)) = row else {
        return Ok(None);
    };
    if digest(encoded.as_bytes()) != hash || digest(request.as_bytes()) != request_hash {
        return Err(error(
            "STORE_CORRUPT",
            "Publication or request checksum mismatch",
        ));
    }
    let saved: SavedRequest = serde_json::from_str(&request)?;
    let receipt: Value = serde_json::from_str(&encoded)?;
    let output = PathBuf::from(output);
    let duration = saved
        .render
        .project
        .duration()
        .map_err(|_| error("STORE_CORRUPT", "Invalid saved publication duration"))?;
    let expected_frames = duration
        .units(saved.render.project.frame_rate)
        .map_err(|_| error("STORE_CORRUPT", "Invalid saved publication frame clock"))?;
    let samples = duration
        .units(crate::time::Time::new(48000, 1)?)
        .map_err(|_| error("STORE_CORRUPT", "Invalid publication sample count"))?;
    if frames <= 0 || expected_frames != frames as u64 {
        return Err(error(
            "STORE_CORRUPT",
            "Publication frame count differs from saved timeline",
        ));
    }
    if output != saved.render.output
        || Path::new(&temp) != output.with_file_name(partial_name(id, attempt))
        || receipt["output"] != json!(output)
        || receipt["frames"] != frames
        || receipt["samples"] != samples
        || receipt["project_revision"] != saved.render.project.revision
    {
        return Err(error(
            "STORE_CORRUPT",
            "Publication record differs from saved request",
        ));
    }
    let metadata = match output.symlink_metadata() {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(error(
            "PUBLICATION_CONFLICT",
            "Published path is not a regular output file",
        ));
    }
    media::allowed_file(&output, &saved.render.output_root)?;
    if receipt["sha256"] != media::file_hash(&output)? {
        return Err(error(
            "PUBLICATION_CONFLICT",
            "Existing output differs from the validated publication; file retained",
        ));
    }
    Ok(Some(receipt))
}

#[cfg(test)]
pub(super) fn crash_point(stage: &str) {
    if std::env::var("CUTBOLT_TEST_PUBLICATION_CRASH").as_deref() == Ok(stage) {
        std::process::exit(89);
    }
}

#[cfg(test)]
mod tests;

pub(super) fn failed(connection: &Connection, id: &str, failure: &crate::Error) -> Result<()> {
    let publication = published(connection, id);
    let tx = connection.unchecked_transaction()?;
    // Obtain the writer before reading cancellation, including after recovery hashing.
    tx.execute("UPDATE jobs SET phase=phase WHERE id=?1", [id])?;
    if let Ok(Some(receipt)) = publication {
        tx.execute("UPDATE jobs SET status='completed',phase='completed',frames=total_frames,result=?2,failure=NULL WHERE id=?1 AND status='running'",params![id,receipt.to_string()])?;
        complete_attempt(&tx, id)?;
    } else {
        let failure = publication
            .err()
            .unwrap_or_else(|| error(failure.code, &failure.message));
        let (cancelled, attempt, maximum): (bool, i64, i64) = tx.query_row(
            "SELECT cancel_requested,attempt,max_attempts FROM jobs WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let interrupted = failure.code == "WORKER_INTERRUPTED";
        let terminal = if cancelled {
            "cancelled"
        } else if interrupted {
            "interrupted"
        } else {
            "failed"
        };
        let retry = !cancelled
            && (1..=3).contains(&maximum)
            && attempt < maximum
            && matches!(
                failure.code,
                "TOOL_FAILED" | "TOOL_TIMEOUT" | "WORKER_INTERRUPTED"
            );
        let status = if retry { "queued" } else { terminal };
        tx.execute("UPDATE jobs SET status=?2,phase=?2,failure=?3,worker_pid=NULL WHERE id=?1 AND status='running'",params![id,status,serde_json::to_string(&failure)?])?;
        tx.execute(
            "UPDATE job_attempts SET status=?2,failure=?3 WHERE job_id=?1 AND attempt=?4",
            params![id, terminal, serde_json::to_string(&failure)?, attempt],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(super) fn reconcile(connection: &Connection) -> Result<()> {
    let mut statement =
        connection.prepare("SELECT id FROM jobs WHERE status='running' ORDER BY sequence")?;
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    drop(statement);
    for id in ids {
        failed(
            connection,
            &id,
            &error(
                "WORKER_INTERRUPTED",
                "Worker exited before recording completion. Validated publication is reconciled; opted-in retries use a new attempt. Crash partials are retained.",
            ),
        )?;
    }
    Ok(())
}
