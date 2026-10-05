pub mod acceleration;
pub mod animation;
pub mod assemble;
pub mod audio;
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
pub mod documents;
pub mod duck;
pub mod dynamics;
pub mod effects;
pub mod expressions;
pub mod files;
pub mod fillers;
pub mod geometry;
pub mod graphics;
pub mod hdr;
pub mod hdr_color;
mod hdr_metadata;
pub mod identity;
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
pub mod spatial;
pub mod stabilize;
pub mod store;
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

#[derive(Debug, Serialize)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn error(code: &'static str, message: impl Into<String>) -> Error {
    Error {
        code,
        message: message.into(),
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
    error(code, message)
}

/// Prefix an error with the JSON path of the request field that caused it, keeping its code.
pub(crate) trait At<T> {
    fn at(self, path: impl FnOnce() -> String) -> Result<T>;
}
impl<T> At<T> for Result<T> {
    fn at(self, path: impl FnOnce() -> String) -> Result<T> {
        self.map_err(|e| Error {
            code: e.code,
            message: format!("{}: {}", path(), e.message),
        })
    }
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
    }
}
