//! Content identities on request. A file identity given as `{"path": ...}`, without `sha256` or
//! `bytes`, is hashed when the request runs, so agents never compute digests themselves. Giving
//! both values still pins exact content, and the engine checks it as before.
use crate::{Result, error, media};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// SHA-256 and byte count of a file, as identities record them.
pub fn of(path: &Path) -> Result<(String, u64)> {
    let bytes = std::fs::metadata(path)?.len();
    Ok((media::file_hash(path)?, bytes))
}

/// The identity of a file inside `root`, with its path relative to that root.
pub fn relative(path: &Path, root: &Path) -> Result<Value> {
    let root = root.canonicalize()?;
    let file = path.canonicalize()?;
    let inside = file.strip_prefix(&root).map_err(|_| {
        error(
            "PATH_OUTSIDE_ROOT",
            format!("{} is not inside {}", path.display(), root.display()),
        )
    })?;
    let (sha256, bytes) = of(&file)?;
    Ok(json!({"path":inside.to_string_lossy().replace('\\', "/"),"sha256":sha256,"bytes":bytes}))
}

/// Fill the missing `sha256`/`bytes` of every file identity in a request from the file under the
/// request's `input_root`, and return the identities it completed.
pub fn complete(request: &mut Value) -> Result<Vec<Value>> {
    let Some(root) = request
        .get("input_root")
        .and_then(Value::as_str)
        .map(PathBuf::from)
    else {
        return Ok(Vec::new());
    };
    let mut seen = BTreeMap::new();
    if let Value::Object(object) = request {
        for value in object.values_mut() {
            fill(value, &root, &mut seen)?;
        }
    }
    Ok(seen.into_values().collect())
}

fn fill(value: &mut Value, root: &Path, seen: &mut BTreeMap<String, Value>) -> Result<()> {
    match value {
        Value::Object(object) if incomplete(object) => {
            let path = object["path"].as_str().unwrap_or_default().to_owned();
            if !seen.contains_key(&path) {
                let file = resolve(root, &path)?;
                let (sha256, bytes) = of(&file)?;
                seen.insert(
                    path.clone(),
                    json!({"path":path,"sha256":sha256,"bytes":bytes}),
                );
            }
            for key in ["sha256", "bytes"] {
                if !object.contains_key(key) {
                    object.insert(key.into(), seen[&path][key].clone());
                }
            }
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                fill(value, root, seen)?;
            }
        }
        Value::Array(items) => {
            for value in items {
                fill(value, root, seen)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A file identity missing its digest or size: only `path`, `sha256` and `bytes` keys, with a path.
fn incomplete(object: &Map<String, Value>) -> bool {
    object.get("path").is_some_and(Value::is_string)
        && object
            .keys()
            .all(|k| matches!(k.as_str(), "path" | "sha256" | "bytes"))
        && !(object.contains_key("sha256") && object.contains_key("bytes"))
}

fn resolve(root: &Path, path: &str) -> Result<PathBuf> {
    let file = root.join(path);
    let missing = |e: std::io::Error| {
        error(
            "MISSING_MEDIA",
            format!("Identity path {path:?} under {}: {e}", root.display()),
        )
    };
    let canonical = file.canonicalize().map_err(missing)?;
    if !canonical.starts_with(root.canonicalize().map_err(missing)?) {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            format!("Identity path {path:?} is outside {}", root.display()),
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_only_identities_are_completed_once_and_reported() {
        let root = std::env::temp_dir().join(format!("cutbolt-identity-{}", std::process::id()));
        std::fs::create_dir_all(root.join("fonts")).unwrap();
        std::fs::write(root.join("fonts").join("a.ttf"), b"font bytes").unwrap();
        let mut request = json!({"command":"scene.inspect","input_root":root,"scene":{"layers":[
            {"graphic":{"fonts":[{"path":"fonts/a.ttf"}]}},
            {"graphic":{"fonts":[{"path":"fonts/a.ttf","bytes":10}]}},
            {"frames":[{"image":{"path":"fonts/a.ttf","sha256":"kept","bytes":3}}]}]}});
        let filled = complete(&mut request).unwrap();
        assert_eq!(filled.len(), 1);
        assert_eq!(filled[0]["bytes"], 10);
        let first = &request["scene"]["layers"][0]["graphic"]["fonts"][0];
        assert_eq!(first["sha256"].as_str().unwrap().len(), 64);
        assert_eq!(
            request["scene"]["layers"][1]["graphic"]["fonts"][0]["sha256"],
            first["sha256"]
        );
        // Complete identities are left for the engine to check.
        assert_eq!(
            request["scene"]["layers"][2]["frames"][0]["image"]["sha256"],
            "kept"
        );
        let missing =
            complete(&mut json!({"input_root":root,"x":{"path":"nope.png"}})).unwrap_err();
        assert_eq!(missing.code, "MISSING_MEDIA");
        let outside = complete(&mut json!({"input_root":root,"x":{"path":"../escape.png"}}));
        assert!(outside.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
