//! Explicit bounded midpoint exposures on exact scene clocks.
use crate::{Result, error, expressions::Number, scene::Scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// How shutter samples combine; only `encoded_rgb`: an equal-weight average of complete encoded sRGB frames.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Integration {
    EncodedRgb,
}

/// Shutter sampling: composites several exact subframe samples per output frame and averages them. Audio is unchanged.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    /// Shutter angle in degrees as a signed rational, 0..360; 360 spans one full frame.
    pub shutter_angle: Number,
    /// Shutter opening offset from the frame time in degrees as a signed rational, -360..360; minus half the angle centers it.
    pub phase: Number,
    /// Midpoint samples per output frame, 1..32; a zero angle requires 1.
    pub samples: u8,
    /// Sample combination method.
    pub integration: Integration,
}

/// Layer parameter records, layers x frames x samples per frame: as many as 64 layers keep over
/// the longest scene without shutter sampling (7,200 frames at 60 fps). When every value changes
/// at every sample this is about 1.3 GB while preparing and a 27 MB receipt.
pub(crate) const MAX_RECORDS: u64 =
    crate::scene::MAX_LAYERS as u64 * crate::scene::MAX_UNSAMPLED_FRAMES;

pub(crate) struct Plan {
    pub times: Vec<Option<Time>>,
    pub samples: usize,
    pub report: Option<Value>,
}

impl Plan {
    pub(crate) fn new(scene: &Scene, frames: u64) -> Result<Self> {
        let rate = scene.clock()?;
        let Some(spec) = &scene.temporal else {
            return Ok(Self {
                times: (0..frames)
                    .map(|n| Time::new(n * rate.den, rate.num).map(Some))
                    .collect::<Result<_>>()?,
                samples: 1,
                report: None,
            });
        };
        let angle = spec.shutter_angle.normalized()?;
        let phase = spec.phase.normalized()?;
        if angle.less(Number::integer(0))
            || Number::integer(360).less(angle)
            || phase.less(Number::integer(-360))
            || Number::integer(360).less(phase)
            || !(1..=32).contains(&spec.samples)
            || (angle.num == 0 && spec.samples != 1)
        {
            return Err(error(
                "INVALID_TEMPORAL",
                "Exposure requires angle 0..360, phase -360..360 degrees and 1..32 samples; zero angle requires one sample",
            ));
        }
        let samples = spec.samples as usize;
        let records = scene.layers.len() as u64 * frames * samples as u64;
        if records > MAX_RECORDS {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "Shutter sampling needs {records} layer sample records ({} layers x {frames} frames x {samples} samples), above the {MAX_RECORDS} that 64 layers keep over 7200 frames without it. Use fewer samples per frame, fewer layers or a shorter scene, or split the scene",
                    scene.layers.len()
                ),
            ));
        }
        let duration = Number::make(scene.duration.num as i128, scene.duration.den as u128)?;
        let mut signed = Vec::new();
        let mut times = Vec::new();
        for frame in 0..frames {
            for k in 0..samples {
                let position = Number::make((2 * k + 1) as i128, (2 * samples) as u128)?;
                // Degrees of shutter over 360 x rate degrees per second (9000 at 25 fps).
                let offset = phase
                    .add(angle.mul(position)?)?
                    .mul(Number::make(rate.den as i128, 360 * rate.num as u128)?)?;
                let time =
                    Number::make((frame * rate.den) as i128, rate.num as u128)?.add(offset)?;
                signed.push(time);
                times.push(if time.num < 0 || !time.less(duration) {
                    None
                } else {
                    Some(Time::new(time.num as u64, time.den)?)
                });
            }
        }
        let report = json!({"profile":"midpoint-exposure-v1","specification":spec,
            "sample_times":signed,"active_sample_times":times,"samples_per_frame":samples,
            "sample_layout":"frame_major_then_increasing_shutter_time","sample_count":times.len(),
            "layer_sample_records":records,"outside_scene":"background","source_frames":"piecewise_hold",
            "integration":"equal_weight_encoded_rgb_nearest_rounding_after_complete_compositing",
            "audio":"unchanged","effects":{"grade":"per_sample_stateless","selective_grade":"per_sample_stateless",
                "chroma_key":"per_sample_stateless","history_filters":"unsupported","optical_flow":"unsupported"},
            "geometry":"subframe_parameters_with_existing_quantization","text_layout":"static"});
        Ok(Self {
            times,
            samples,
            report: Some(report),
        })
    }
}

pub fn capabilities() -> Value {
    json!({"profile":"midpoint-exposure-v1","shutter_angle_degrees":[0,360],"phase_degrees":[-360,360],
        "maximum_samples_per_frame":32,"maximum_layer_sample_records":MAX_RECORDS,
        "compositing_work":"scenes.limits.maximum_composited_pixels_counts_every_sample_and_its_accumulation",
        "integration":["encoded_rgb"],"sampling":"exact_rational_midpoints","outside_scene":"background",
        "source_interpolation":"held_images","optical_flow":false,"history_effects":false,"changes_audio":false})
}
