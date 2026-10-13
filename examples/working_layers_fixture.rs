//! Bounded adapter for an independent oracle of the actual working-layer compositor.
use cutbolt::{
    color::working::{
        Pixel,
        layer::{Description, Image, Layer},
    },
    time::Time,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    io::{self, Read},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Draw {
    layer: Description,
    duration: Time,
    time: Time,
    anchor: [i32; 2],
    size: [u32; 2],
    offset: [u32; 2],
    pixels: Vec<[u16; 4]>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    size: [u32; 2],
    backdrop: Vec<[u16; 4]>,
    draws: Vec<Draw>,
}
fn run(root: &Path) -> cutbolt::Result<serde_json::Value> {
    let mut input = Vec::new();
    io::stdin()
        .take(32 * 1024 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 32 * 1024 * 1024 {
        return Err(cutbolt::error(
            "LIMIT_EXCEEDED",
            "Fixture request exceeds 32 MiB",
        ));
    }
    let request: Request = serde_json::from_slice(&input)?;
    if request.draws.len() > 64
        || request.backdrop.len() > 1048576
        || request.draws.iter().map(|d| d.pixels.len()).sum::<usize>() > 1048576
    {
        return Err(cutbolt::error(
            "LIMIT_EXCEEDED",
            "Fixture exceeds 64 layers or one million pixels",
        ));
    }
    let mut destination = request.backdrop.into_iter().map(Pixel).collect::<Vec<_>>();
    for draw in request.draws {
        let layer = Layer::new(draw.layer, draw.duration, root)?;
        let prepared = layer.prepare()?;
        let pixels = draw.pixels.into_iter().map(Pixel).collect::<Vec<_>>();
        prepared.draw(
            draw.time,
            Image {
                size: draw.size,
                offset: draw.offset,
                pixels: &pixels,
            },
            draw.anchor,
            request.size,
            &mut destination,
        )?;
    }
    Ok(json!({"pixels":destination.into_iter().map(|p|p.0).collect::<Vec<_>>()}))
}
fn main() {
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let result = root
        .as_deref()
        .ok_or_else(|| cutbolt::error("INVALID_PATH", "Fixture requires an input root"))
        .and_then(run);
    match result {
        Ok(value) => println!("{}", json!({"ok":true,"result":value})),
        Err(error) => {
            println!("{}", json!({"ok":false,"error":error}));
            std::process::exit(1);
        }
    }
}
