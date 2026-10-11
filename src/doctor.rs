//! Local installation diagnostics. Inspect only: no workspace creation, media or downloads.
use crate::{Result, cache, error, media, workspace::DEFAULTS};
use serde_json::{Value, json};
use std::{collections::BTreeSet, fs, path::Path, time::Duration};

const ENCODERS: &[&str] = &["ffv1", "pcm_s16le", "png", "libx264", "aac"];
const DECODERS: &[&str] = &[
    "ffv1",
    "h264",
    "hevc",
    "aac",
    "pcm_s16le",
    "mp3",
    "flac",
    "opus",
];
const MUXERS: &[&str] = &["matroska", "mp4", "wav", "image2", "rawvideo", "s16le"];

fn listing(text: &str, codecs: bool) -> BTreeSet<String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let flags = fields.next()?;
            let name = fields.next()?;
            let entry = if codecs {
                flags.len() == 6 && matches!(flags.as_bytes()[0], b'V' | b'A' | b'S')
            } else {
                flags == "E"
            };
            (entry && name != "=").then(|| name.to_owned())
        })
        .collect()
}

fn query(path: &Path, argument: &str) -> Result<String> {
    let bytes = media::capture(
        &path.to_string_lossy(),
        &["-hide_banner".into(), argument.into()],
        Duration::from_secs(10),
    )?;
    String::from_utf8(bytes)
        .map_err(|_| error("TOOL_FAILED", "Media tool diagnostics are not valid UTF-8"))
}

fn required(path: &Path, argument: &str, names: &[&str], codecs: bool) -> Result<Value> {
    let observed = listing(&query(path, argument)?, codecs);
    let missing: Vec<_> = names
        .iter()
        .filter(|name| !observed.contains(**name))
        .collect();
    Ok(json!({"required":names,"missing":missing,"ready":missing.is_empty()}))
}

fn tool(name: &str) -> Value {
    let selected = media::tool(name);
    let checked = (|| -> Result<Value> {
        let path = cache::resolve_tool(&selected)?;
        let version = query(&path, "-version")?;
        let first = version.lines().next().unwrap_or_default();
        if !first.starts_with(&format!("{name} version ")) {
            return Err(error(
                "TOOL_FAILED",
                format!("Selected {name} did not report its expected version header"),
            ));
        }
        let mut result = json!({"ready":true,"selected":selected,"path":path,
            "version":first,"build_configuration":version.lines().find_map(|line|line.strip_prefix("configuration: ")),
            "identity":{"sha256":media::file_hash(&path)?,"bytes":fs::metadata(&path)?.len()}});
        if name == "ffmpeg" {
            result["encoders"] = required(&path, "-encoders", ENCODERS, true)?;
            result["decoders"] = required(&path, "-decoders", DECODERS, true)?;
            result["muxers"] = required(&path, "-muxers", MUXERS, false)?;
            result["ready"] = json!(
                result["encoders"]["ready"] == true
                    && result["decoders"]["ready"] == true
                    && result["muxers"]["ready"] == true
            );
        }
        Ok(result)
    })();
    let mut result =
        checked.unwrap_or_else(|cause| json!({"ready":false,"selected":selected,"error":cause}));
    if result["ready"] != true {
        result["hint"] = json!(format!(
            "Install an external {name} build with the listed features; put it on PATH or set CUTBOLT_{} to its executable path, then rerun cutbolt doctor",
            name.to_uppercase()
        ));
    }
    result
}

fn state_directory(root: &Path, field: &str, relative: &str) -> Value {
    let location = root.join(relative);
    let mut ancestor = root.to_path_buf();
    for component in relative.split('/') {
        ancestor.push(component);
        match fs::symlink_metadata(&ancestor) {
            Ok(_) => match ancestor.canonicalize() {
                Ok(canonical) if canonical.is_dir() && canonical.starts_with(root) => {}
                Ok(canonical) => {
                    return json!({"ready":false,"field":field,"path":location,
                        "problem_at":ancestor,"directory":canonical.is_dir(),
                        "inside_workspace":canonical.starts_with(root)});
                }
                Err(cause) => {
                    return json!({"ready":false,"field":field,"path":location,
                        "problem_at":ancestor,"error":cause.to_string()});
                }
            },
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                return json!({"ready":true,"field":field,"path":location,"exists":false});
            }
            Err(cause) => {
                return json!({"ready":false,"field":field,"path":location,
                    "problem_at":ancestor,"error":cause.to_string()});
            }
        }
    }
    json!({"ready":true,"field":field,"path":location,"exists":true,
        "directory":true,"inside_workspace":true})
}

fn workspace(path: Option<&Path>) -> Value {
    let Some(path) = path else {
        return json!({"ready":true,"configured":false,"note":"Pass --workspace DIR or set CUTBOLT_WORKSPACE to inspect an existing directory"});
    };
    let inspected = (|| -> Result<Value> {
        let root = path.canonicalize()?;
        if !root.is_dir() {
            return Err(error("INVALID_PATH", "Workspace must be a directory"));
        }
        let mut ready = true;
        let mut state = Vec::new();
        for (field, relative) in DEFAULTS.iter().filter(|(_, p)| !p.is_empty()) {
            let status = state_directory(&root, field, relative);
            ready &= status["ready"] == true;
            state.push(status);
        }
        Ok(
            json!({"ready":ready,"configured":true,"path":root,"state_directories":state,
            "write_access":"not_tested","store_integrity":"not_checked",
            "note":"Missing state directories are created by normal workspace use, not by doctor"}),
        )
    })();
    let mut result = inspected
        .unwrap_or_else(|cause| json!({"ready":false,"configured":true,"path":path,"error":cause}));
    if result["ready"] != true {
        result["hint"] = json!(
            "Use an existing workspace directory with engine-state directories inside it; doctor does not create, move or repair files"
        );
    }
    result
}

/// Report local executable discovery/features and workspace structure. `ready` is a setup
/// check, not a render test, hardware-device probe, optional-runtime test or compatibility claim.
pub fn inspect(path: Option<&Path>) -> Value {
    let (ffmpeg, ffprobe) = std::thread::scope(|scope| {
        let encoder = scope.spawn(|| tool("ffmpeg"));
        let probe = tool("ffprobe");
        (encoder.join().expect("doctor tool inspection"), probe)
    });
    let workspace = workspace(path);
    json!({"profile":"local-installation-check-v1","ready":ffmpeg["ready"]==true && ffprobe["ready"]==true && workspace["ready"]==true,
        "engine":{"version":env!("CARGO_PKG_VERSION"),"build":crate::version()},
        "platform":{"os":std::env::consts::OS,"architecture":std::env::consts::ARCH,
            "background_jobs":cfg!(any(windows,target_os="linux")),
            "image_directory_publication":cfg!(any(windows,target_os="linux")),
            "audio_capture":cfg!(windows),"cuda_decode_backend":cfg!(windows),
            "speech_backend":if cfg!(windows){"wsl"}else if cfg!(target_os="linux"){"native_linux"}else{"unavailable"}},
        "tools":{"ffmpeg":ffmpeg,"ffprobe":ffprobe},"workspace":workspace,
        "scope":"Executable versions and required codec/muxer listings; no media render, GPU initialization or optional speech/model execution",
        "runtime_network":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_listings_distinguish_real_entries_from_headers_and_similar_names() {
        let codecs = "Encoders:\n V..... = Video\n A..... = Audio\n ------\n V.S..D ffv1 FFV1\n A....D aac AAC\n V....D libx264 H.264\n V....D libx264rgb RGB\n";
        assert_eq!(
            listing(codecs, true),
            ["ffv1", "aac", "libx264", "libx264rgb"]
                .map(str::to_owned)
                .into_iter()
                .collect()
        );
        assert_eq!(
            listing(
                " E = Muxing supported\n --\n E mp4 MP4\n E matroska Matroska\n D mov MOV\n",
                false
            ),
            ["mp4", "matroska"].map(str::to_owned).into_iter().collect()
        );
        assert!(!listing(" V....D libx264rgb RGB\n", true).contains("libx264"));
    }
}
