pub mod acceleration;
pub mod animation;
pub mod assemble;
pub mod audio;
mod audio_cache;
pub mod audio_processing;
pub mod audio_repair;
pub mod audio_routing;
pub mod beats;
pub mod cache;
mod cache_store;
pub mod caption_overlay;
pub mod captions;
pub mod check;
pub mod color;
pub mod color_match;
pub mod commands;
pub mod composite;
pub mod conform;
pub mod cut_review;
pub mod delivery;
mod digest;
pub mod direct;
pub mod documents;
pub mod duck;
pub mod dynamics;
pub mod effects;
pub mod errors;
pub mod expressions;
pub mod files;
pub mod fillers;
pub mod fonts;
pub mod geometry;
pub mod graphics;
pub mod hdr;
pub mod hdr_color;
mod hdr_metadata;
pub mod identity;
mod identity_cache;
pub mod image_sequence;
pub mod inspection_cache;
pub mod interchange;
pub mod jobs;
pub mod keying;
pub mod lut;
pub mod mcp;
pub mod media;
pub mod media_transcribe;
pub mod meters;
pub mod model;
pub mod multicam;
pub mod native_project;
pub mod normalize;
mod numerals;
pub mod outline;
mod pcm_stream;
pub mod pcm_wave;
pub mod portable;
pub mod preview;
pub mod proxy;
pub mod readiness;
pub mod recording;
pub mod reference;
pub mod reframe;
pub mod registry;
pub mod remap;
pub mod render;
pub mod review;
pub mod scene;
pub mod schema;
pub mod scopes;
pub mod selection;
pub mod sequences;
mod source_index;
pub mod spatial;
mod spelling;
pub mod stabilize;
pub mod store;
pub mod summary;
pub mod sync;
pub mod templates;
pub mod temporal;
pub(crate) mod thumbnail;
pub mod tighten;
pub mod time;
mod track_composite;
pub mod track_edit;
mod track_render;
pub mod tracking;
pub mod tracks;
pub mod transcribe;
pub mod transcript;
pub mod transcript_captions;
pub mod transcript_cut;
pub mod workspace;

use serde::Serialize;

/// The Git commit checked out when this engine was built, and whether its sources differed from
/// it; `None` when the build could not ask Git.
pub const BUILD_COMMIT: Option<(&str, bool)> = include!(concat!(env!("OUT_DIR"), "/commit.rs"));

/// `cutbolt --version`: the package version and, when known, the build's commit.
pub fn version() -> String {
    let version = concat!("cutbolt ", env!("CARGO_PKG_VERSION"));
    match BUILD_COMMIT {
        Some((commit, false)) => format!("{version} (commit {commit})"),
        Some((commit, true)) => format!("{version} (commit {commit}, sources modified)"),
        None => version.to_owned(),
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    /// Advisory recovery class, derived centrally from the stable code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry: Option<errors::Retry>,
    /// Known request field path; human labels remain in message only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Box<str>>,
    /// Mechanical repair guidance, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<Box<str>>,
    /// Structured data a caller can act on, such as the observations accepted before a tracking
    /// failure; most errors carry none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn error(code: &'static str, message: impl Into<String>) -> Error {
    let message = message.into();
    let path = message
        .split_once(": ")
        .and_then(|(prefix, _)| machine_path(prefix));
    Error {
        code,
        message,
        retry: errors::retry_class(code),
        path: path.map(String::into_boxed_str),
        hint: errors::hint(code).map(Box::from),
        detail: errors::detail(code),
    }
}

impl Error {
    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into().into_boxed_str());
        self
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into().into_boxed_str());
        self
    }
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }
    /// Preserve structured errors across the engine's child-process boundary.
    pub(crate) fn reported(code: &'static str, value: &serde_json::Value) -> Self {
        let mut result = error(code, value["message"].as_str().unwrap_or_default());
        if let Some(path) = value["path"].as_str() {
            result.path = Some(Box::from(path));
        }
        if let Some(hint) = value["hint"].as_str() {
            result.hint = Some(Box::from(hint));
        }
        if let Some(detail) = value.get("detail").filter(|v| !v.is_null()) {
            result.detail = Some(detail.clone());
        }
        result
    }
}

/// Most IDs a missing-reference message lists before counting the rest.
pub(crate) const LISTED_IDS: usize = 20;

/// Error for a reference to an absent `kind` ID, listing the IDs that do exist so the caller can correct it.
pub(crate) fn missing<'a>(
    code: &'static str,
    kind: &str,
    id: &str,
    available: impl IntoIterator<Item = &'a str>,
) -> Error {
    let mut ids: Vec<&str> = available.into_iter().collect();
    ids.sort_unstable();
    ids.dedup();
    let total = ids.len();
    ids.truncate(LISTED_IDS);
    missing_listed(code, kind, id, &ids, total)
}

/// `missing` for callers that load only the first `LISTED_IDS` sorted IDs and the total count.
pub(crate) fn missing_listed(
    code: &'static str,
    kind: &str,
    id: &str,
    first: &[impl AsRef<str>],
    total: usize,
) -> Error {
    let mut listed: Vec<String> = first.iter().map(|v| format!("{:?}", v.as_ref())).collect();
    if listed.is_empty() {
        listed.push("none".into());
    }
    let mut message = format!(
        "Unknown {kind} {id:?}; available {kind} IDs: {}",
        listed.join(", ")
    );
    if total > first.len() {
        message += &format!(" … and {} more", total - first.len());
    }
    error(code, message).with_detail(serde_json::json!({
        "available":first.iter().map(AsRef::as_ref).collect::<Vec<&str>>(), "total":total
    }))
}

/// Prefix an error with the JSON path of the request field that caused it, keeping its code.
pub(crate) trait At<T> {
    fn at(self, path: impl FnOnce() -> String) -> Result<T>;
    /// Like `at`, for an error from a nested part that may already name its own field relative to
    /// `path`: `layers[2] (title)` and `animation.opacity.keys[3].time: ...` join with a dot, as
    /// `layers[2] (title).animation.opacity.keys[3].time: ...`. Other messages read as with `at`.
    fn under(self, path: impl FnOnce() -> String) -> Result<T>;
}
impl<T> At<T> for Result<T> {
    fn at(self, path: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| {
            let prefix = path();
            Error {
                path: prefixed_path(&prefix, e.path.as_deref()),
                message: format!("{prefix}: {}", e.message),
                ..e
            }
        })
    }
    fn under(self, path: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| {
            let nested = e
                .message
                .split_once(": ")
                .is_some_and(|(head, _)| relative_path(head));
            let separator = if nested { "." } else { ": " };
            let prefix = path();
            Error {
                path: prefixed_path(&prefix, e.path.as_deref()),
                message: format!("{prefix}{separator}{}", e.message),
                ..e
            }
        })
    }
}

/// Remove human layer labels, while refusing to invent a path for arbitrary prose.
fn machine_path(text: &str) -> Option<String> {
    let mut result = String::new();
    let mut rest = text;
    while let Some((prefix, label)) = rest.split_once(" (") {
        result.push_str(prefix);
        rest = label.split_once(')')?.1;
    }
    result.push_str(rest);
    relative_path(&result).then_some(result)
}

fn prefixed_path(prefix: &str, nested: Option<&str>) -> Option<Box<str>> {
    let prefix = machine_path(prefix)?;
    Some(
        match nested {
            Some(path) if path != prefix && !prefix.ends_with(&format!(".{path}")) => {
                format!("{prefix}.{path}")
            }
            _ => prefix,
        }
        .into_boxed_str(),
    )
}

/// A field path relative to some object, such as `animation.opacity` or `keys[3].time`.
fn relative_path(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_lowercase())
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '[' | ']'))
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        error("IO_ERROR", value.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        error("INVALID_JSON", value.to_string())
    }
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        let code = match value.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                "STORE_BUSY"
            }
            Some(rusqlite::ErrorCode::DiskFull) => "DISK_FULL",
            Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
                "STORE_CORRUPT"
            }
            _ => "STORE_ERROR",
        };
        error(code, value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_references_list_sorted_bounded_candidates() {
        let none = missing("MISSING_TRACK", "track", "v9", []);
        assert_eq!(
            (none.code, none.message.as_str()),
            (
                "MISSING_TRACK",
                r#"Unknown track "v9"; available track IDs: none"#
            )
        );
        let some = missing("MISSING_CLIP", "clip", "nope", ["c2", "c1", "c2"]);
        assert_eq!(
            none.detail,
            Some(serde_json::json!({"available":[],"total":0}))
        );
        assert_eq!(
            some.detail,
            Some(serde_json::json!({"available":["c1","c2"],"total":2}))
        );
        assert_eq!(
            some.message,
            r#"Unknown clip "nope"; available clip IDs: "c1", "c2""#
        );
        let names: Vec<String> = (0..25).map(|i| format!("a{i:02}")).collect();
        let many = missing(
            "MISSING_MEDIA",
            "asset",
            "x",
            names.iter().map(String::as_str),
        );
        assert!(
            many.message.contains(r#""a00", "a01""#)
                && many.message.contains(r#""a19" … and 5 more"#)
                && !many.message.contains("a20"),
            "{}",
            many.message
        );
        assert_eq!(
            many.detail.as_ref().unwrap()["available"]
                .as_array()
                .unwrap()
                .len(),
            LISTED_IDS
        );
        assert_eq!(many.detail.as_ref().unwrap()["total"], 25);
    }

    #[test]
    fn nested_field_paths_join_with_a_dot() {
        let nested: Result<()> = Err(error("X", "keys[3].time: 7 s is late"));
        let joined = nested.under(|| "animation.opacity".into()).unwrap_err();
        assert_eq!(
            joined.path.as_deref(),
            Some("animation.opacity.keys[3].time")
        );
        assert_eq!(
            joined.message,
            "animation.opacity.keys[3].time: 7 s is late"
        );
        let layer = Err::<(), _>(joined)
            .under(|| "layers[2] (title)".into())
            .unwrap_err();
        assert_eq!(
            layer.path.as_deref(),
            Some("layers[2].animation.opacity.keys[3].time")
        );
        assert_eq!(
            layer.message,
            "layers[2] (title).animation.opacity.keys[3].time: 7 s is late"
        );
        let plain: Result<()> = Err(error("X", "Layer frame 3: lost"));
        assert_eq!(
            plain.under(|| "segments[1]".into()).unwrap_err().message,
            "segments[1]: Layer frame 3: lost"
        );
        let queued =
            Err::<(), _>(error("IO_ERROR", "Output parent is missing").with_path("output"))
                .at(|| "arguments.output".into())
                .unwrap_err();
        assert_eq!(queued.path.as_deref(), Some("arguments.output"));
    }
}
