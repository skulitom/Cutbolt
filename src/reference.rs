//! Saved-revision references. Commands that read a project also accept `{project_id, revision}`
//! and load that revision from the session store, so agents need not resend whole snapshots.
use crate::{At, Result, error, model::Project, store};
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// A saved project revision to load in place of a full snapshot.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SavedProject {
    /// Session store; defaults to the workspace store.
    #[serde(default)]
    pub store_root: Option<PathBuf>,
    /// Saved project ID.
    pub project_id: String,
    /// Saved revision; omit for the current head. Give one to pin a render or preview.
    #[serde(default)]
    pub revision: Option<u64>,
}

/// A project snapshot, or `{project_id, revision}` naming a saved revision to load instead.
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
#[allow(dead_code)]
pub enum ProjectInput {
    /// Full snapshot.
    Snapshot(Project),
    Saved(SavedProject),
}

/// Visit the objects of a request that may hold roots, paths or a `project` input: the request
/// itself, then the nested `render` of render.start or `task` of cache.run.
pub(crate) fn contexts(
    request: &mut Value,
    mut visit: impl FnMut(&str, &mut Value) -> Result<()>,
) -> Result<()> {
    let nested = match request["command"].as_str() {
        Some("render.start") => Some("render"),
        Some("cache.run") => Some("task"),
        _ => None,
    };
    visit("", request)?;
    if let Some(inner) = nested
        .and_then(|key| request.get_mut(key))
        .filter(|v| v.is_object())
    {
        visit(nested.unwrap(), inner)?;
    }
    Ok(())
}

/// Replace every saved-project reference in a request with the snapshot it names.
pub fn resolve(request: &mut Value, default_store: Option<&Path>) -> Result<()> {
    // A new session is created from a snapshot, never from another saved project.
    if request["command"] == "session.create" {
        return Ok(());
    }
    contexts(request, |context, object| {
        let Some(slot) = object.get_mut("project") else {
            return Ok(());
        };
        if slot.get("project_id").is_none() || slot.get("schema_version").is_some() {
            return Ok(());
        }
        let field = || match context {
            "" => "project".to_owned(),
            nested => format!("{nested}.project"),
        };
        let saved: SavedProject = serde_json::from_value(slot.take())
            .map_err(crate::Error::from)
            .at(field)?;
        let store_root = saved
            .store_root
            .as_deref()
            .or(default_store)
            .ok_or_else(|| {
                error(
                    "INVALID_PATH",
                    "A saved project reference needs store_root when no workspace is configured",
                )
            })
            .at(field)?;
        let project = store::get(store_root, &saved.project_id, saved.revision).at(field)?;
        *slot = serde_json::to_value(project)?;
        Ok(())
    })
}
