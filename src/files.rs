//! Listing the files an agent can use, so an MCP-only client can find its media.
use crate::{Result, error};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path},
};

/// Most entries one listing returns.
const MAX_ENTRIES: usize = 1000;

/// Files and folders under `dir` inside `root`, sorted, with sizes; engine state is skipped.
pub fn list(
    root: &Path,
    dir: Option<&str>,
    recursive: bool,
    extensions: &[String],
    limit: Option<usize>,
) -> Result<Value> {
    let root = crate::media::input_root(root)?;
    let relative = dir.unwrap_or("");
    if Path::new(relative)
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(error(
            "INVALID_PATH",
            "dir must be a relative folder inside input_root, without '..'",
        ));
    }
    let start = root.join(relative);
    if !start.is_dir() {
        return Err(error(
            "INVALID_PATH",
            format!("{relative:?} is not a folder inside input_root"),
        ));
    }
    let limit = limit.unwrap_or(200).clamp(1, MAX_ENTRIES);
    let wanted: Vec<String> = extensions
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .collect();
    let mut entries = Vec::new();
    let mut pending = vec![start];
    let mut total = 0usize;
    while let Some(folder) = pending.pop() {
        let mut children: Vec<_> = fs::read_dir(&folder)?.collect::<std::io::Result<_>>()?;
        children.sort_by_key(|c| c.file_name());
        for child in children {
            let name = child.file_name().to_string_lossy().into_owned();
            if name == ".cutbolt" || name.starts_with(".cutbolt-") {
                continue;
            }
            let kind = child.file_type()?;
            let path = child.path();
            let shown = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if kind.is_dir() {
                if recursive {
                    pending.push(path.clone());
                } else if wanted.is_empty() {
                    total += 1;
                    if entries.len() < limit {
                        entries.push(json!({"path":shown,"kind":"folder"}));
                    }
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let extension = path
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if !wanted.is_empty() && !wanted.contains(&extension) {
                continue;
            }
            total += 1;
            if entries.len() < limit {
                entries.push(json!({"path":shown,"kind":"file","bytes":child.metadata()?.len()}));
            }
        }
    }
    entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
    Ok(json!({"entries":entries,"total":total,"truncated":total > entries.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_media_without_engine_state() {
        let root = std::env::temp_dir().join(format!("cutbolt-files-{}", std::process::id()));
        fs::create_dir_all(root.join("media/deep")).unwrap();
        fs::create_dir_all(root.join(".cutbolt/store")).unwrap();
        for name in ["a.mkv", "media/b.MP4", "media/deep/c.wav", "notes.txt"] {
            fs::write(root.join(name), b"x").unwrap();
        }
        let top = list(&root, None, false, &[], None).unwrap();
        let paths: Vec<&str> = top["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["path"].as_str().unwrap())
            .collect();
        assert_eq!(paths, ["a.mkv", "media", "notes.txt"]);
        let media = list(&root, None, true, &["mkv".into(), ".mp4".into()], None).unwrap();
        assert_eq!(media["total"], 2);
        assert_eq!(media["entries"][1]["path"], "media/b.MP4");
        let capped = list(&root, None, true, &[], Some(1)).unwrap();
        assert_eq!(
            (capped["total"].as_u64(), capped["truncated"].as_bool()),
            (Some(4), Some(true))
        );
        assert!(list(&root, Some("../x"), false, &[], None).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
