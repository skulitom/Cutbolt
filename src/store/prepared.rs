//! Optimistic saved edits whose media preparation runs outside the database write lock.
use super::*;
use std::path::PathBuf;

pub(crate) enum Begin {
    Replayed(Box<Receipt>),
    Prepare(Box<Prepared>),
}

pub(crate) struct Prepared {
    pub project: Project,
    root: PathBuf,
    request_id: String,
    hash: String,
}

/// Fingerprint the caller's intent, before generated asset paths/content are known. This uses a
/// separate contract from ordinary session.apply, sharing its request-ID and history tables.
pub(crate) fn begin(
    root: &Path,
    project_id: &str,
    request_id: &str,
    expected_revision: u64,
    command: &str,
    request: &impl Serialize,
) -> Result<Begin> {
    check_id(project_id)?;
    check_id(request_id)?;
    let hash = digest(&serde_json::to_vec(&json!({
        "contract":"prepared-apply-v1","command":command,"project_id":project_id,
        "expected_revision":expected_revision,"request":request
    }))?);
    let mut connection = connect(root, false)?;
    let tx = connection.transaction()?;
    if let Some(receipt) = replay(&tx, project_id, request_id, &hash)? {
        return Ok(Begin::Replayed(Box::new(receipt)));
    }
    let current = head(&tx, project_id)?;
    if current != expected_revision {
        return Err(crate::errors::revision(
            expected_revision,
            current,
            format!("Expected {expected_revision}; current revision is {current}"),
        ));
    }
    Ok(Begin::Prepare(Box::new(Prepared {
        project: load(&tx, project_id, current)?.0,
        root: root.to_path_buf(),
        request_id: request_id.into(),
        hash,
    })))
}

impl Prepared {
    /// Recheck replay and revision under the short write transaction. `preserve` retains the
    /// already-verified prepared asset immediately before committing: a commit I/O error can be
    /// ambiguous, so the caller must then keep that asset for receipt/history recovery.
    pub fn commit(self, operations: Vec<Operation>, preserve: impl FnOnce()) -> Result<Receipt> {
        use crate::media::Control;
        crate::media::Uncontrolled.check()?;
        let mut connection = connect(&self.root, false)?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = &self.project.id;
        if let Some(receipt) = replay(&tx, id, &self.request_id, &self.hash)? {
            return Ok(receipt);
        }
        let current = head(&tx, id)?;
        if current != self.project.revision {
            return Err(crate::errors::revision(
                self.project.revision,
                current,
                format!(
                    "Expected {}; current revision is {current}",
                    self.project.revision
                ),
            ));
        }
        let before = load(&tx, id, current)?.0;
        let next = before.apply(current, operations)?;
        let receipt = Receipt {
            project_id: id.clone(),
            request_id: self.request_id,
            action: "apply".into(),
            revision: next.revision,
            parent_revision: Some(current),
            restored_from: None,
            changes: diff(&before, &next)?,
        };
        record(&tx, &next, Some(current), &receipt, &self.hash)?;
        crate::media::Uncontrolled.check()?;
        preserve();
        tx.commit()?;
        Ok(receipt)
    }
}
