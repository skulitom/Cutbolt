pub mod acceleration;
pub mod animation;
pub mod audio;
pub mod audio_processing;
pub mod audio_repair;
pub mod audio_routing;
pub mod cache;
mod cache_store;
pub mod captions;
pub mod color;
pub mod commands;
pub mod composite;
pub mod conform;
pub mod delivery;
pub mod effects;
pub mod expressions;
pub mod geometry;
pub mod graphics;
pub mod hdr;
pub mod hdr_color;
mod hdr_metadata;
pub mod identity;
pub mod image_sequence;
pub mod interchange;
pub mod jobs;
pub mod keying;
pub mod lut;
pub mod mcp;
pub mod media;
pub mod model;
pub mod multicam;
pub mod native_project;
mod pcm_stream;
pub mod pcm_wave;
pub mod portable;
pub mod preview;
pub mod proxy;
pub mod recording;
pub mod reference;
pub mod reframe;
pub mod registry;
pub mod remap;
pub mod render;
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
pub mod time;
pub mod track_edit;
mod track_render;
pub mod tracking;
pub mod tracks;
pub mod transcribe;
pub mod transcript;
pub mod transcript_cut;
pub mod workspace;

use serde::Serialize;

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
