//! Explicit bounded midpoint exposures on exact scene clocks.
use crate::{Result, error, expressions::Number, scene::Scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Integration {
    EncodedRgb,
}

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Exposure {
    pub shutter_angle: Number,
    pub phase: Number,
    pub samples: u8,
    pub integration: Integration,
}

pub(crate) struct Plan {
    pub times: Vec<Option<Time>>,
    pub samples: usize,
    pub report: Option<Value>,
}

impl Plan {
    pub(crate) fn new(scene: &Scene, frames: u64) -> Result<Self> {
        let Some(spec) = &scene.temporal else {
            return Ok(Self {
                times: (0..frames)
                    .map(|n| Time::new(n, 25).map(Some))
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
        let work = scene.width as u64
            * scene.height as u64
            * scene.layers.len() as u64
            * frames
            * samples as u64;
        if work > 67_108_864 || scene.layers.len() as u64 * frames * samples as u64 > 32768 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Temporal exposure exceeds 67108864 layer-pixel sample visits or 32768 layer sample records",
            ));
        }
        let duration = Number::make(scene.duration.num as i128, scene.duration.den as u128)?;
        let mut signed = Vec::new();
        let mut times = Vec::new();
        for frame in 0..frames {
            for k in 0..samples {
                let position = Number::make((2 * k + 1) as i128, (2 * samples) as u128)?;
                let offset = phase
                    .add(angle.mul(position)?)?
                    .div(Number::integer(9000))?;
                let time = Number::make(frame as i128, 25)?.add(offset)?;
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
            "layer_pixel_sample_visits":work,"outside_scene":"background","source_frames":"piecewise_hold",
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
        "maximum_samples_per_frame":32,"maximum_layer_pixel_sample_visits":67108864,"maximum_layer_sample_records":32768,
        "integration":["encoded_rgb"],"sampling":"exact_rational_midpoints","outside_scene":"background",
        "source_interpolation":"held_images","optical_flow":false,"history_effects":false,"changes_audio":false})
}
