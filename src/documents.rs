//! Large documents by file, inside a workspace. An argument given as `{"file": "scenes/title.json"}`
//! is read from that JSON file, or one top-level field of it with `"select"`; `save_as` writes a
//! command's whole result to a new file and returns a short summary. Scenes, caption documents and
//! projects can then move between calls without passing through an agent's context.
use crate::{Result, error, workspace::Workspace};
use serde_json::{Map, Value, json};
use std::{fs, io::Write, path::PathBuf};

/// Documents are bounded like everything else an agent hands the engine.
const MAX_DOCUMENT: u64 = 16 * 1024 * 1024;

/// A `{"file": ...}` reference, optionally with `select`, and nothing else.
fn reference(object: &Map<String, Value>) -> Option<(&str, Option<&str>)> {
    let file = object.get("file")?.as_str()?;
    let select = match object.get("select") {
        None => None,
        Some(Value::String(key)) => Some(key.as_str()),
        Some(_) => return None,
    };
    (object.len() == 1 + usize::from(select.is_some())).then_some((file, select))
}

/// Replace every document reference in a request with the document it names.
pub fn load(request: &mut Value, workspace: Option<&Workspace>) -> Result<()> {
    match request {
        Value::Object(object) => {
            if let Some((file, select)) = reference(object) {
                let Some(workspace) = workspace else {
                    return Err(error(
                        "INVALID_PATH",
                        "Document files ({\"file\": ...}) need a workspace",
                    ));
                };
                *request = read(workspace, file, select)?;
                return Ok(());
            }
            object.values_mut().try_for_each(|v| load(v, workspace))
        }
        Value::Array(items) => items.iter_mut().try_for_each(|v| load(v, workspace)),
        _ => Ok(()),
    }
}

fn read(workspace: &Workspace, file: &str, select: Option<&str>) -> Result<Value> {
    let path = workspace.file(file)?;
    let size = fs::metadata(&path)
        .map_err(|e| error("MISSING_MEDIA", format!("Document {file:?}: {e}")))?
        .len();
    if size > MAX_DOCUMENT {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!("Document {file:?} is {size} bytes; documents are limited to 16 MiB"),
        ));
    }
    let document: Value = serde_json::from_slice(&fs::read(&path)?)
        .map_err(|e| error("INVALID_JSON", format!("Document {file:?}: {e}")))?;
    match select {
        None => Ok(document),
        Some(key) => document.get(key).cloned().ok_or_else(|| {
            let keys: Vec<&String> = document
                .as_object()
                .into_iter()
                .flat_map(|o| o.keys())
                .collect();
            error(
                "INVALID_JSON",
                format!("Document {file:?} has no field {key:?}; it has {keys:?}"),
            )
        }),
    }
}

/// Write a result to a new workspace file and describe it briefly.
pub fn save(result: &Value, file: &str, workspace: &Workspace) -> Result<Value> {
    let path: PathBuf = workspace.file(file)?;
    if !path.extension().is_some_and(|e| e == "json") {
        return Err(error("INVALID_PATH", "save_as must name a .json file"));
    }
    let bytes = serde_json::to_vec(result)?;
    let mut output = fs::File::create_new(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => error(
            "OUTPUT_EXISTS",
            format!("{file:?} already exists; documents are never overwritten"),
        ),
        _ => error("INVALID_PATH", format!("save_as {file:?}: {e}")),
    })?;
    output.write_all(&bytes)?;
    output.sync_all()?;
    let fields: Map<String, Value> = result
        .as_object()
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), json!(value.to_string().len())))
        .collect();
    Ok(
        json!({"saved_as":file,"bytes":bytes.len(),"field_bytes":fields,
        "note":"Pass {\"file\": saved_as, \"select\": field} as an argument to use a field."}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_round_trip_through_workspace_files() {
        let root = std::env::temp_dir().join(format!("cutbolt-documents-{}", std::process::id()));
        std::fs::create_dir_all(root.join("scenes")).unwrap();
        let workspace = Workspace::open(&root).unwrap();
        let result = json!({"scene":{"id":"title","layers":[]},"inspection":{"frames":100}});
        let saved = save(&result, "scenes/title.json", &workspace).unwrap();
        assert_eq!(saved["saved_as"], "scenes/title.json");
        assert_eq!(
            save(&result, "scenes/title.json", &workspace)
                .unwrap_err()
                .code,
            "OUTPUT_EXISTS"
        );
        let mut request = json!({"command":"scene.inspect","scene":{"file":"scenes/title.json","select":"scene"},
            "list":[{"file":"scenes/title.json"}],"other":{"file":"x","sha256":"y"}});
        load(&mut request, Some(&workspace)).unwrap();
        assert_eq!(request["scene"], result["scene"]);
        assert_eq!(request["list"][0], result);
        // Objects with other keys are ordinary arguments.
        assert_eq!(request["other"]["sha256"], "y");
        let missing = load(
            &mut json!({"a":{"file":"scenes/title.json","select":"nope"}}),
            Some(&workspace),
        );
        assert!(missing.unwrap_err().message.contains("inspection"));
        assert_eq!(
            load(&mut json!({"a":{"file":"x.json"}}), None)
                .unwrap_err()
                .code,
            "INVALID_PATH"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
