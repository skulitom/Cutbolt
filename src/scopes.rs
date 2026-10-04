//! Exact full-quality frame histograms, waveform/parade and encoded chroma populations.
use crate::{Result, color, error, media, model::Project, preview, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    pub project: Project,
    pub input_root: PathBuf,
    pub time: Time,
    pub input_transfer: color::Transfer,
    pub missing_tags: color::MissingTags,
    pub columns: usize,
}
#[derive(Serialize)]
struct Plane {
    histogram: Vec<u64>,
    waveform: Vec<Vec<u64>>,
    minimum: u8,
    maximum: u8,
    sum: u64,
}
impl Plane {
    fn new(columns: usize) -> Self {
        Self {
            histogram: vec![0; 256],
            waveform: vec![vec![0; 256]; columns],
            minimum: 255,
            maximum: 0,
            sum: 0,
        }
    }
    fn add(&mut self, column: usize, value: u8) {
        self.histogram[value as usize] += 1;
        self.waveform[column][value as usize] += 1;
        self.minimum = self.minimum.min(value);
        self.maximum = self.maximum.max(value);
        self.sum += value as u64;
    }
}
fn evaluate(pixels: &[u8], width: usize, columns: usize) -> Value {
    let mut planes = std::array::from_fn::<_, 4, _>(|_| Plane::new(columns));
    let mut vectors = vec![vec![0u64; 256]; 256];
    for (i, pixel) in pixels.as_chunks::<3>().0.iter().enumerate() {
        let [r, g, b] = pixel.map(u64::from);
        let y = 2126 * r + 7152 * g + 722 * b;
        let luma = ((y + 5000) / 10000) as u8;
        let column = (i % width) * columns / width;
        for c in 0..3 {
            planes[c].add(column, pixel[c]);
        }
        planes[3].add(column, luma);
        let cb = ((128 * 18556 + 10000 * b as i64 - y as i64 + 18556 / 2) / 18556).clamp(0, 255)
            as usize;
        let cr = ((128 * 15748 + 10000 * r as i64 - y as i64 + 15748 / 2) / 15748).clamp(0, 255)
            as usize;
        vectors[cr][cb] += 1;
    }
    let [red, green, blue, luma] = planes;
    json!({"red":red,"green":green,"blue":blue,"luma":luma,"vectorscope":vectors})
}
pub fn inspect(request: &Inspect) -> Result<Value> {
    if request.columns == 0
        || request.columns > 256
        || request.columns > request.project.width as usize
    {
        return Err(error(
            "INVALID_SCOPE",
            "Scope columns must be 1..min(width,256)",
        ));
    }
    let mut project = request.project.clone();
    project.preview_scale = None;
    let (pixels, frame) = preview::read_frame(&project, &request.input_root, request.time)?;
    let width = frame["width"].as_u64().expect("width") as usize;
    let values = evaluate(&pixels, width, request.columns);
    let sources: Vec<&Value> = if let Some(sources) = frame["sources"].as_array() {
        sources.iter().collect()
    } else if frame["source"]["path"].is_string() {
        vec![&frame["source"]]
    } else {
        vec![]
    };
    let mut interpretations = Vec::new();
    for source in sources {
        let path = std::path::Path::new(source["path"].as_str().expect("source path"));
        let metadata = media::probe(path)?;
        let video = metadata["streams"]
            .as_array()
            .and_then(|streams| streams.iter().find(|s| s["codec_type"] == "video"))
            .ok_or_else(|| {
                error(
                    "MEDIA_CHANGED",
                    "Source video disappeared during scope inspection",
                )
            })?;
        let (_, report) = color::Input {
            matrix: color::Matrix::Rgb,
            range: color::Range::Full,
            transfer: request.input_transfer,
            missing_tags: request.missing_tags,
        }
        .inspect(video, true, false)?;
        if media::file_hash(path)? != source["sha256"].as_str().expect("source identity") {
            return Err(error(
                "MEDIA_CHANGED",
                "Source changed during scope inspection",
            ));
        }
        interpretations.push(json!({"path":source["path"],"interpretation":report}));
    }
    let interpretation = if interpretations.len() == 1 {
        interpretations[0]["interpretation"].clone()
    } else {
        Value::Null
    };
    Ok(
        json!({"profile":"encoded-rgb-scopes-v1","frame":frame,"input_transfer":request.input_transfer,"interpretation":interpretation,"source_interpretations":interpretations,"pixels":pixels.len()/3,"rgb_sha256":format!("{:x}",Sha256::digest(&pixels)),"columns":request.columns,"scope_domain":"encoded_full_range_rgb_8bit","luma_coefficients":[2126,7152,722],"luma_divisor":10000,"rounding":"nearest_ties_up","waveform_axes":"column_then_code_value","vectorscope_axes":"cr_then_cb_full_range_128_center","values":values}),
    )
}
pub fn capabilities() -> Value {
    json!({"profile":"encoded-rgb-scopes-v1","histograms":["red","green","blue","luma"],"waveform_parade":true,"vectorscope_bins":[256,256],"columns":[1,256],"sampling":"every_pixel_of_one_exact_25fps_frame","source_quality":"original","maximum_pixels":8000000,"writes_files":false})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn primary_and_neutral_scope_anchors() {
        let result = evaluate(
            &[0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255],
            5,
            5,
        );
        assert_eq!(result["luma"]["histogram"][54], 1);
        assert_eq!(result["luma"]["histogram"][182], 1);
        assert_eq!(result["luma"]["histogram"][18], 1);
        assert_eq!(result["vectorscope"][128][128], 2);
        assert_eq!(result["red"]["waveform"][2][255], 1);
        assert_eq!(result["red"]["sum"], 510);
    }
}
