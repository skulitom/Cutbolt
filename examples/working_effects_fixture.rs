//! Bounded adapter for independent working-effect and exact-curve fixture references.
use cutbolt::{
    animation::Curve,
    color::working::Pixel,
    effects::{Effect, working::Effects},
    time::Time,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{self, Read},
    path::Path,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Point {
    rgba: [u16; 4],
    position: [i64; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Frame {
    time: Time,
    pixels: Vec<Point>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Effects {
        effects: Vec<Effect>,
        duration: Time,
        frames: Vec<Frame>,
    },
    Curve {
        curve: Curve,
        duration: Time,
        bounds: [i32; 2],
        scale: u16,
        times: Vec<Time>,
    },
}

fn run(root: &Path) -> cutbolt::Result<Value> {
    let mut input = Vec::new();
    io::stdin()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 16 * 1024 * 1024 {
        return Err(cutbolt::error(
            "LIMIT_EXCEEDED",
            "Fixture input exceeds 16 MiB",
        ));
    }
    let request: Request = serde_json::from_slice(&input)?;
    match request {
        Request::Effects {
            effects,
            duration,
            frames,
        } => {
            if frames.len() > 1024 || frames.iter().map(|f| f.pixels.len()).sum::<usize>() > 65536 {
                return Err(cutbolt::error(
                    "LIMIT_EXCEEDED",
                    "Fixture exceeds 1024 frames or 65536 pixels",
                ));
            }
            let processor = Effects::new(effects, duration, root)?;
            let mut result = Vec::with_capacity(frames.len());
            for frame in frames {
                let sampled = processor.frame(frame.time)?;
                result.push(
                    frame
                        .pixels
                        .iter()
                        .map(|p| sampled.pixel(Pixel(p.rgba), p.position).map(|p| p.0))
                        .collect::<cutbolt::Result<Vec<_>>>()?,
                );
            }
            Ok(json!({"frames": result, "identities": processor.identities().collect::<Vec<_>>()}))
        }
        Request::Curve {
            curve,
            duration,
            bounds,
            scale,
            times,
        } => {
            if times.len() > 65536 {
                return Err(cutbolt::error(
                    "LIMIT_EXCEEDED",
                    "Fixture exceeds 65536 curve samples",
                ));
            }
            let sampler = curve.prepare(duration, bounds[0], bounds[1])?;
            let mut result = Vec::with_capacity(times.len());
            for time in times {
                result.push(json!({"integer":sampler.sample(time)?, "scaled":sampler.sample_scaled(time, scale)?,
                                   "continuous":sampler.sample_f64(time)?}));
            }
            Ok(json!({"samples":result}))
        }
    }
}

fn main() {
    let root = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let result = root
        .as_deref()
        .ok_or_else(|| cutbolt::error("INVALID_PATH", "Fixture requires an input root"))
        .and_then(run);
    match result {
        Ok(value) => println!("{}", json!({"ok": true, "result": value})),
        Err(error) => {
            println!("{}", json!({"ok": false, "error": error}));
            std::process::exit(1);
        }
    }
}
