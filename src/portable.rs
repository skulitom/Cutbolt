//! Original relative-media proposal; applying it uses existing revision/history rules.
use crate::{
    Result, error, media,
    model::{Operation, Project},
    registry::Identity,
};
use serde_json::{Value, json};
use std::path::Path;

fn relative(path: &str, identity: &Identity, root: &Path) -> Result<String> {
    let path = media::project_file(Path::new(path), root)?;
    if path.metadata()?.len() != identity.bytes || media::file_hash(&path)? != identity.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            "Portable media differs from its bound identity",
        ));
    }
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|v| v.replace('\\', "/"))
        .ok_or_else(|| {
            error(
                "INVALID_PATH",
                "Portable media must have a Unicode path under the media root",
            )
        })
}

pub fn inspect(project: &Project, expected_revision: u64, input_root: &Path) -> Result<Value> {
    project.validate()?;
    if project.revision != expected_revision {
        return Err(error(
            "REVISION_CONFLICT",
            "Expected revision does not match the supplied project",
        ));
    }
    let root = media::input_root(input_root)?;
    let mut operations = Vec::new();
    for asset in &project.assets {
        let identity = asset.identity.as_ref().ok_or_else(|| {
            error(
                "IDENTITY_REQUIRED",
                "Bind all media before making portable paths",
            )
        })?;
        let path = relative(&asset.path, identity, &root)?;
        let proxy_path = asset
            .proxy
            .as_ref()
            .map(|p| relative(&p.path, &p.identity, &root))
            .transpose()?;
        if path != asset.path
            || proxy_path.as_deref() != asset.proxy.as_ref().map(|p| p.path.as_str())
        {
            operations.push(Operation::Paths {
                asset_id: asset.id.clone(),
                path,
                proxy_path,
            });
        }
    }
    let proposed = if operations.is_empty() {
        project.clone()
    } else {
        project.apply(expected_revision, operations.clone())?
    };
    Ok(
        json!({"project":proposed,"operations":operations,"input_root":root,"writes_files":false,"updates_session":false,"media_copied":false}),
    )
}
