//! Colour matching between cameras: sample frames of a reference shot and a target shot, build a
//! per-channel 1D LUT that maps the target's colour statistics onto the reference's, write it as a
//! `.cube` table and return the `media.conform` recipe that bakes it into a new target asset.
use crate::{Result, error, media, time::Time};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

/// Frames sampled per file when no times are given.
const DEFAULT_FRAMES: u64 = 9;
/// Most sampled frames per file.
const MAX_FRAMES: usize = 64;

/// How the target's channels are mapped onto the reference's.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Match each channel's mean and standard deviation with a straight line: robust when the shots differ in content.
    Levels,
    /// Match each channel's whole distribution by quantiles: closer when both shots show the same scene.
    Histogram,
}

/// What to match.
pub struct Request<'a> {
    pub reference: &'a Path,
    pub target: &'a Path,
    pub reference_times: Option<Vec<Time>>,
    pub target_times: Option<Vec<Time>>,
    pub input_root: &'a Path,
    pub output_root: &'a Path,
    pub output: &'a Path,
    pub method: Method,
    pub transfer: crate::color::Transfer,
}

/// The starts of `count` frames spread evenly through the file: frame `(2i+1)·F/(2·count)` of its
/// `F` whole frames, so every sample exists.
fn spread_frames(metadata: &Value, duration: Time, count: u64) -> Result<Vec<Time>> {
    let rate = metadata["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .and_then(|stream| stream["r_frame_rate"].as_str())
        .and_then(|r| r.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<u64>().ok()?, d.parse::<u64>().ok()?)))
        .filter(|&(n, d)| n > 0 && d > 0);
    let Some((num, den)) = rate else {
        return crate::review::spread(duration, count);
    };
    let frames = (duration.num as u128 * num as u128 / (duration.den as u128 * den as u128)) as u64;
    if frames == 0 {
        return crate::review::spread(duration, count);
    }
    (0..count)
        .map(|i| Time::new((2 * i + 1) * frames / (2 * count) * den, num))
        .collect()
}

/// Per-channel 256-bin histograms of the frames at `times` (default: spread through the file).
fn histograms(path: &Path, times: Option<Vec<Time>>) -> Result<([[u64; 256]; 3], usize)> {
    let (metadata, width, height, duration) = crate::review::video(path)?;
    let times = match times {
        Some(times) => times,
        None => spread_frames(&metadata, duration, DEFAULT_FRAMES)?,
    };
    if times.is_empty() || times.len() > MAX_FRAMES {
        return Err(error(
            "INVALID_ARGUMENT",
            format!("Sample 1-{MAX_FRAMES} frames per file"),
        ));
    }
    let mut counts = [[0u64; 256]; 3];
    let mut frames = 0;
    for time in times {
        let Some(pixels) = crate::review::grab(path, time, width, height)? else {
            continue;
        };
        for pixel in pixels.as_chunks::<3>().0 {
            for (channel, &value) in pixel.iter().enumerate() {
                counts[channel][value as usize] += 1;
            }
        }
        frames += 1;
    }
    if frames == 0 {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            format!("No frame could be read from {}", path.display()),
        ));
    }
    Ok((counts, frames))
}

fn mean_and_deviation(counts: &[u64; 256]) -> (f64, f64) {
    let total: u64 = counts.iter().sum();
    let mean = counts
        .iter()
        .enumerate()
        .map(|(v, &n)| v as f64 * n as f64)
        .sum::<f64>()
        / total as f64;
    let variance = counts
        .iter()
        .enumerate()
        .map(|(v, &n)| (v as f64 - mean).powi(2) * n as f64)
        .sum::<f64>()
        / total as f64;
    (mean, variance.sqrt())
}

/// The output code for each input code of one channel.
fn mapping(target: &[u64; 256], reference: &[u64; 256], method: Method) -> [u8; 256] {
    let mut table = [0u8; 256];
    match method {
        Method::Levels => {
            let (mean_t, sd_t) = mean_and_deviation(target);
            let (mean_r, sd_r) = mean_and_deviation(reference);
            let gain = if sd_t > 0.0 { sd_r / sd_t } else { 1.0 };
            for (v, out) in table.iter_mut().enumerate() {
                *out = ((v as f64 - mean_t) * gain + mean_r)
                    .round()
                    .clamp(0.0, 255.0) as u8;
            }
        }
        Method::Histogram => {
            // Each target code maps to the first reference code whose cumulative share reaches
            // the target code's own cumulative share; integer arithmetic keeps it exact.
            let (total_t, total_r): (u128, u128) = (
                target.iter().map(|&n| n as u128).sum(),
                reference.iter().map(|&n| n as u128).sum(),
            );
            let mut cumulative_r = [0u128; 256];
            let mut running = 0u128;
            for (r, &n) in reference.iter().enumerate() {
                running += n as u128;
                cumulative_r[r] = running;
            }
            let mut running_t = 0u128;
            for (v, &n) in target.iter().enumerate() {
                running_t += n as u128;
                // cumulative_r / total_r >= running_t / total_t
                let r = (0..256)
                    .find(|&r| cumulative_r[r] * total_t >= running_t * total_r)
                    .unwrap_or(255);
                table[v] = r as u8;
            }
        }
    }
    table
}

/// Match a target shot's colour to a reference shot's.
pub fn propose(request: &Request) -> Result<Value> {
    let inside = |path: &Path| {
        if path.is_relative() {
            request.input_root.join(path)
        } else {
            path.to_path_buf()
        }
    };
    let reference = media::allowed_file(&inside(request.reference), request.input_root)?;
    let target = media::allowed_file(&inside(request.target), request.input_root)?;
    let output = crate::render::destination_extension(request.output, request.output_root, "cube")?;
    if output.try_exists()? {
        return Err(error(
            "OUTPUT_EXISTS",
            format!("{} already exists", output.display()),
        ));
    }
    let (reference_counts, reference_frames) =
        histograms(&reference, request.reference_times.clone())?;
    let (target_counts, target_frames) = histograms(&target, request.target_times.clone())?;
    let tables: Vec<[u8; 256]> = (0..3)
        .map(|c| mapping(&target_counts[c], &reference_counts[c], request.method))
        .collect();
    // A 1D table with one entry per 8-bit code: linear sampling then lands exactly on entries.
    let mut text = String::from("TITLE \"cutbolt colour match\"\nLUT_1D_SIZE 256\n");
    for ((red, green), blue) in tables[0].iter().zip(&tables[1]).zip(&tables[2]) {
        text.push_str(&format!(
            "{:.6} {:.6} {:.6}\n",
            *red as f64 / 255.0,
            *green as f64 / 255.0,
            *blue as f64 / 255.0
        ));
    }
    let mut file = fs::File::create_new(&output)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    // The table must be readable by media.conform under the same input root.
    let identity = crate::identity::relative(&output, request.input_root).map_err(|_| {
        let _ = fs::remove_file(&output);
        error(
            "PATH_OUTSIDE_ROOT",
            "The .cube output must lie inside input_root so media.conform can read it",
        )
    })?;
    let lut = json!({"file":identity,"interpolation":"linear"});
    // The target's conform recipe, with an identity normalization for an encoded RGB asset.
    let relative = crate::identity::relative(&target, request.input_root)?;
    let metadata = media::probe(&target)?;
    let streams = metadata["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .cloned()
        .unwrap_or_default();
    let audio = streams.iter().find(|s| s["codec_type"] == "audio").cloned();
    let rate = crate::readiness::native_rate(&video, &metadata);
    let mut recipe = crate::readiness::conform(
        relative["path"].as_str().unwrap_or_default(),
        &video,
        audio.as_ref(),
        rate,
        None,
    )
    .map_err(|why| error("UNSUPPORTED_MEDIA", why))?;
    recipe["source"]["file"] = relative;
    if recipe["source"]["color"] == "encoded_rgb" {
        recipe["source"]
            .as_object_mut()
            .expect("source")
            .remove("color");
        recipe["source"]["sdr"] = json!({"matrix":"rgb","range":"full","transfer":request.transfer,"missing_tags":"use_declared"});
        recipe["working_transfer"] = json!(request.transfer);
    }
    recipe["id"] = json!(format!(
        "{}-matched",
        recipe["id"].as_str().unwrap_or("target")
    ));
    recipe["lut"] = lut.clone();
    let stats = |counts: &[[u64; 256]; 3]| -> Value {
        json!(
            (0..3)
                .map(|c| {
                    let (mean, deviation) = mean_and_deviation(&counts[c]);
                    json!({"mean":(mean * 100.0).round() / 100.0,"deviation":(deviation * 100.0).round() / 100.0})
                })
                .collect::<Vec<_>>()
        )
    };
    let matched: Vec<[u64; 256]> = (0..3)
        .map(|c| {
            let mut out = [0u64; 256];
            for (v, &n) in target_counts[c].iter().enumerate() {
                out[tables[c][v] as usize] += n;
            }
            out
        })
        .collect();
    let matched: [[u64; 256]; 3] = [matched[0], matched[1], matched[2]];
    Ok(
        json!({"output":output,"method":request.method,"lut":lut,"recipe":recipe,
        "frames":{"reference":reference_frames,"target":target_frames},
        "channels":["red","green","blue"],
        "reference":stats(&reference_counts),"target":stats(&target_counts),"matched":stats(&matched),
        "next":"job.start run media.conform with recipe (and a new output), then media.add the matched asset and use it in place of the target"}),
    )
}
