//! An optional workspace fixed when the CLI or MCP server starts. Omitted roots default inside it,
//! relative paths resolve against it, explicit roots must stay inside it, and engine-produced
//! paths are reported relative to it.
use crate::{At, Result, error};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Root fields a workspace supplies when a request omits them, with their location inside it.
/// Engine state lives in `.cutbolt`, beside the user's media and out of the way.
pub const DEFAULTS: [(&str, &str); 7] = [
    ("input_root", ""),
    ("output_root", ""),
    ("media_root", ""),
    ("store_root", ".cutbolt/store"),
    ("job_root", ".cutbolt/jobs"),
    ("cache_root", ".cutbolt/cache"),
    ("scratch_root", ".cutbolt/scratch"),
];

#[derive(Clone, Debug)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    /// Open an existing directory as the workspace, creating its engine-state folders.
    pub fn open(path: &Path) -> Result<Self> {
        let canonical = path
            .canonicalize()
            .map_err(|e| error("INVALID_PATH", format!("Workspace {}: {e}", path.display())))?;
        if !canonical.is_dir() {
            return Err(error("INVALID_PATH", "The workspace must be a directory"));
        }
        let workspace = Self {
            root: plain(&canonical),
        };
        for (_, relative) in DEFAULTS.iter().filter(|(_, r)| !r.is_empty()) {
            std::fs::create_dir_all(workspace.inside(relative))?;
        }
        Ok(workspace)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn store_root(&self) -> PathBuf {
        self.inside(".cutbolt/store")
    }

    /// A location inside the workspace, given with '/' separators.
    pub fn inside(&self, relative: &str) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(relative.split('/').filter(|part| !part.is_empty()));
        path
    }

    /// Fill omitted roots, resolve relative paths and keep explicit roots inside the workspace.
    pub fn prepare(&self, request: &mut Value) -> Result<()> {
        let Some(command) = request["command"].as_str().map(str::to_owned) else {
            return Ok(());
        };
        crate::reference::contexts(request, |context, object| {
            let at = |field: &str| match context {
                "" => field.to_owned(),
                nested => format!("{nested}.{field}"),
            };
            let accepted = crate::schema::context_properties(&command, context, object);
            let Some(object) = object.as_object_mut() else {
                return Ok(());
            };
            for (field, relative) in DEFAULTS {
                match object.get(field) {
                    None if accepted.iter().any(|p| p == field) => {
                        object.insert(field.into(), json!(self.inside(relative)));
                    }
                    Some(Value::String(path)) => {
                        let resolved = self.resolve(path).at(|| at(field))?;
                        self.contain(&resolved).at(|| at(field))?;
                        object.insert(field.into(), json!(resolved));
                    }
                    _ => {}
                }
            }
            // Root-relative files: outputs, probed paths and relink candidates.
            if let Some(Value::String(path)) = object.get("output") {
                let resolved = self.resolve(path).at(|| at("output"))?;
                object.insert("output".into(), json!(resolved));
            }
            if let Some(Value::String(path)) = object.get("path")
                && object.contains_key("input_root")
                && accepted.iter().any(|p| p == "path")
            {
                let resolved = self.resolve(path).at(|| at("path"))?;
                object.insert("path".into(), json!(resolved));
            }
            if let Some(Value::Array(candidates)) = object.get_mut("candidates") {
                for candidate in candidates.iter_mut() {
                    if let Value::String(path) = candidate {
                        *candidate = json!(self.resolve(path).at(|| at("candidates"))?);
                    }
                }
            }
            // A saved-project reference may name its store relative to the workspace.
            if let Some(Value::String(path)) =
                object.get("project").and_then(|p| p.get("store_root"))
            {
                let resolved = self.resolve(path).at(|| at("project.store_root"))?;
                self.contain(&resolved).at(|| at("project.store_root"))?;
                object["project"]["store_root"] = json!(resolved);
            }
            Ok(())
        })
    }

    /// An absolute path is kept as given; a relative one is joined to the workspace, without `..`.
    fn resolve(&self, path: &str) -> Result<PathBuf> {
        let given = Path::new(path);
        if given.is_absolute() {
            return Ok(given.to_path_buf());
        }
        let mut resolved = self.root.clone();
        for part in path.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    return Err(error(
                        "PATH_OUTSIDE_WORKSPACE",
                        "Relative paths may not use '..'",
                    ));
                }
                name => resolved.push(name),
            }
        }
        Ok(resolved)
    }

    /// Existing roots must resolve, through any links, to a location inside the workspace.
    fn contain(&self, path: &Path) -> Result<()> {
        let Ok(canonical) = path.canonicalize() else {
            // The command reports a missing root itself.
            return Ok(());
        };
        if plain(&canonical).starts_with(&self.root) {
            Ok(())
        } else {
            Err(error(
                "PATH_OUTSIDE_WORKSPACE",
                format!(
                    "{} is outside the workspace {}",
                    path.display(),
                    self.root.display()
                ),
            ))
        }
    }

    /// Report engine-produced paths without the Windows extended-length prefix, relative to the
    /// workspace when inside it. Paths the caller supplied, such as asset paths, are unchanged.
    pub fn present(&self, value: &mut Value) {
        match value {
            Value::String(text) => {
                if let Some(rest) = text.strip_prefix(r"\\?\") {
                    let path = match rest.strip_prefix(r"UNC\") {
                        Some(share) => PathBuf::from(format!(r"\\{share}")),
                        None => PathBuf::from(rest),
                    };
                    *text = match path.strip_prefix(&self.root) {
                        Ok(inside) => inside.to_string_lossy().replace('\\', "/"),
                        Err(_) => path.to_string_lossy().into_owned(),
                    };
                }
            }
            Value::Array(items) => items.iter_mut().for_each(|v| self.present(v)),
            Value::Object(object) => object.values_mut().for_each(|v| self.present(v)),
            _ => {}
        }
    }

    /// The defaults reported by capabilities and folded into schemas.
    pub fn describe(&self) -> Value {
        let defaults: serde_json::Map<String, Value> = DEFAULTS
            .iter()
            .map(|(field, relative)| {
                let shown = if relative.is_empty() { "." } else { relative };
                ((*field).to_owned(), json!(shown))
            })
            .collect();
        json!({"root":self.root,"defaults":defaults,"relative_paths":"resolved against the root; '..' is rejected",
            "explicit_roots":"must lie inside the root","reported_paths":"engine-produced paths inside the root are relative, with '/' separators"})
    }
}

/// Drop the Windows extended-length prefix that canonicalization adds to drive paths.
fn plain(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with(r"UNC\") => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::handle_json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cutbolt-workspace-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            assert!(self.0.starts_with(std::env::temp_dir()));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(id: &str) -> Value {
        json!({"schema_version":1,"id":id,"revision":0,"width":32,"height":24,
            "frame_rate":{"num":25,"den":1},"assets":[],"clips":[]})
    }

    #[test]
    fn roots_default_inside_and_paths_stay_inside() {
        let temp = Temp::new();
        let workspace = Workspace::open(&temp.0).unwrap();
        let mut request = json!({"command":"render.start","request_id":"r",
            "render":{"project":project("p"),"output":"renders/a.mkv"}});
        workspace.prepare(&mut request).unwrap();
        assert_eq!(
            request["job_root"],
            json!(workspace.inside(".cutbolt/jobs"))
        );
        assert_eq!(request["render"]["input_root"], json!(workspace.root()));
        assert_eq!(
            request["render"]["output"],
            json!(workspace.root().join("renders").join("a.mkv"))
        );
        let mut task =
            json!({"command":"cache.run","policy":{},"task":{"type":"probe","path":"media/a.mkv"}});
        workspace.prepare(&mut task).unwrap();
        assert_eq!(
            task["cache_root"],
            json!(workspace.inside(".cutbolt/cache"))
        );
        assert_eq!(
            task["task"]["path"],
            json!(workspace.root().join("media").join("a.mkv"))
        );
        let mut climb = json!({"command":"media.inspect","path":"../secret.mkv"});
        let error = workspace.prepare(&mut climb).unwrap_err();
        assert_eq!(
            (error.code, error.message.starts_with("path:")),
            ("PATH_OUTSIDE_WORKSPACE", true)
        );
        let outside = Temp::new();
        let mut escape = json!({"command":"session.get","project_id":"p","store_root":outside.0});
        assert_eq!(
            workspace.prepare(&mut escape).unwrap_err().code,
            "PATH_OUTSIDE_WORKSPACE"
        );
    }

    #[test]
    fn saved_references_replace_snapshots() {
        let temp = Temp::new();
        let workspace = Workspace::open(&temp.0).unwrap();
        let ws = Some(&workspace);
        handle_json(
            json!({"command":"session.create","request_id":"c","project":project("p")}),
            ws,
        )
        .unwrap();
        let checked = handle_json(
            json!({"command":"project.validate","project":{"project_id":"p"}}),
            ws,
        )
        .unwrap();
        assert_eq!(checked["revision"], 0);
        let edited = handle_json(json!({"command":"timeline.apply","expected_revision":0,
            "project":{"project_id":"p","revision":0},
            "operations":[{"op":"clip.append","clip":{"id":"g","gap":true,"source_in":{"num":0,"den":1},"duration":{"num":1,"den":1}}}]}), ws).unwrap();
        assert_eq!(
            (
                edited["revision"].as_u64(),
                edited["clips"][0]["id"].as_str()
            ),
            (Some(1), Some("g"))
        );
        let missing = handle_json(
            json!({"command":"project.validate","project":{"project_id":"p","revision":7}}),
            ws,
        )
        .unwrap_err();
        assert!(
            missing.message.starts_with("project:"),
            "{}",
            missing.message
        );
        let unknown = handle_json(
            json!({"command":"project.validate","project":{"project_id":"p","rev":0}}),
            ws,
        )
        .unwrap_err();
        assert!(unknown.message.starts_with("project:") && unknown.message.contains("rev"));
        // Without a workspace a reference must name its store; with one it works from anywhere.
        let bare = handle_json(
            json!({"command":"project.validate","project":{"project_id":"p"}}),
            None,
        )
        .unwrap_err();
        assert_eq!(bare.code, "INVALID_PATH");
        let named = handle_json(json!({"command":"project.validate","project":{"project_id":"p","store_root":workspace.store_root()}}), None).unwrap();
        assert_eq!(named["valid"], true);
        // session.create still requires a snapshot.
        let copy = handle_json(
            json!({"command":"session.create","request_id":"d","project":{"project_id":"p"}}),
            ws,
        )
        .unwrap_err();
        assert_eq!(copy.code, "INVALID_JSON");
        // A new session can start from dimensions alone, and a retry replays the same receipt.
        let direct = json!({"command":"session.create","request_id":"n","id":"n","width":32,"height":24,"frame_rate":{"num":25,"den":1}});
        let created = handle_json(direct.clone(), ws).unwrap();
        assert_eq!(created["revision"], 0);
        assert_eq!(created["project_id"], "n");
        assert_eq!(handle_json(direct, ws).unwrap(), created);
        let mixed = handle_json(
            json!({"command":"session.create","request_id":"m","project":project("m"),"width":32}),
            ws,
        )
        .unwrap_err();
        assert_eq!(mixed.code, "INVALID_ARGUMENT");
    }

    #[test]
    fn engine_paths_are_reported_relative() {
        let temp = Temp::new();
        let workspace = Workspace::open(&temp.0).unwrap();
        let inside = format!(r"\\?\{}\renders\a.mkv", workspace.root().display());
        let mut result = json!({"output":inside,"source":"C:/caller/given.mkv","outside":r"\\?\Z:\elsewhere\b.png"});
        workspace.present(&mut result);
        assert_eq!(
            result,
            json!({"output":"renders/a.mkv","source":"C:/caller/given.mkv","outside":r"Z:\elsewhere\b.png"})
        );
        let capabilities =
            handle_json(json!({"command":"capabilities"}), Some(&workspace)).unwrap();
        assert_eq!(
            capabilities["workspace"]["defaults"]["store_root"],
            ".cutbolt/store"
        );
        assert!(
            handle_json(json!({"command":"capabilities"}), None).unwrap()["workspace"].is_null()
        );
    }
}
