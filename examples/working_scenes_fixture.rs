//! Bounded test adapter for prepared scene frames in the precise working representation.
use cutbolt::{
    error,
    scene::{Scene, working::Source},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::{self, Read},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    scene: Scene,
    frames: Vec<(u64, u32)>,
    #[serde(default)]
    audio: Vec<(u64, u64)>,
}
fn run(root: &Path) -> cutbolt::Result<serde_json::Value> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(error("LIMIT_EXCEEDED", "Fixture request exceeds 16 MiB"));
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    if request.frames.len() > 256
        || request.audio.len() > 16
        || request.audio.iter().any(|(_, n)| *n > 48000)
    {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Fixture exceeds frame/audio request bounds",
        ));
    }
    let source = Source::new(&request.scene, root)?;
    if request.frames.len() as u64
        * u64::from(request.scene.width)
        * u64::from(request.scene.height)
        * u64::from(request.scene.output_scale).pow(2)
        > 1048576
    {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Fixture exceeds one million output pixels",
        ));
    }
    let frames = request
        .frames
        .into_iter()
        .map(|(n, d)| {
            source
                .frame(n, d)
                .map(|p| p.into_iter().map(|v| v.0).collect::<Vec<_>>())
        })
        .collect::<cutbolt::Result<Vec<_>>>()?;
    let audio = request
        .audio
        .into_iter()
        .map(|(n, c)| source.pcm(n, c))
        .collect::<cutbolt::Result<Vec<_>>>()?;
    source.verify_sources(root)?;
    Ok(
        json!({"frames":frames,"audio":audio,"audio_samples":source.audio_samples(),"report":source.report(),"source_count":source.sources().len(),"retained_bytes":source.retained_bytes(),"frame_buffer_bytes":source.frame_buffer_bytes()}),
    )
}
fn main() {
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let result = root
        .as_deref()
        .ok_or_else(|| error("INVALID_PATH", "Fixture requires an input root"))
        .and_then(run);
    match result {
        Ok(value) => println!("{}", json!({"ok":true,"result":value})),
        Err(error) => {
            println!("{}", json!({"ok":false,"error":error}));
            std::process::exit(1);
        }
    }
}
