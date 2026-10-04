//! Explicit optional decode-device selection through the external media tool.
use crate::{Result, error, media};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

/// Policy when the CUDA device fails to initialize: `error` returns DEVICE_UNAVAILABLE; `cpu` decodes in software and reports the reason.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Unavailable {
    Error,
    Cpu,
}
/// Source video decode backend, tagged by `backend`; only source decoding moves to the GPU.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "backend", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decode {
    /// Software decoding (the default when `decode` is omitted).
    Cpu,
    /// CUDA hardware decoding; supports progressive H.264 yuv420p at even sizes 128x128 to 1920x1080.
    Cuda {
        /// Local CUDA device ordinal, 0 to 31.
        device: u32,
        /// Behavior when device initialization fails.
        unavailable: Unavailable,
    },
}

#[derive(Debug, Serialize)]
pub(crate) struct Selected {
    requested: Decode,
    selected_backend: &'static str,
    device: Option<u32>,
    fallback: Option<Value>,
    availability_micros: u128,
}
impl Selected {
    pub(crate) fn input_args(&self, args: &mut Vec<String>) {
        if self.selected_backend == "cuda" {
            let at = args.iter().position(|v| v == "-i").expect("input argument");
            args.splice(
                at..at,
                [
                    "-hwaccel".into(),
                    "cuda".into(),
                    "-hwaccel_device".into(),
                    self.device.expect("selected device").to_string(),
                    "-hwaccel_output_format".into(),
                    "cuda".into(),
                ],
            );
        }
    }
    /// Whether frames come from a hardware decoder (always decoded into a bounded scratch window).
    pub(crate) fn hardware(&self) -> bool {
        self.selected_backend == "cuda"
    }
    pub(crate) fn filter(&self) -> Option<&'static str> {
        // Requiring a hardware frame here prevents an unnoticed software decode.
        (self.selected_backend == "cuda").then_some("hwdownload,format=nv12")
    }
    pub(crate) fn report(&self) -> Value {
        json!(self)
    }
}

pub(crate) fn select(request: Option<Decode>, metadata: &Value) -> Result<Selected> {
    let requested = request.unwrap_or(Decode::Cpu);
    let mut selected = Selected {
        requested,
        selected_backend: "cpu",
        device: None,
        fallback: None,
        availability_micros: 0,
    };
    if let Decode::Cuda {
        device,
        unavailable,
    } = requested
    {
        if device > 31 {
            return Err(error(
                "INVALID_ACCELERATION",
                "CUDA device ordinal must be 0..31",
            ));
        }
        let video = metadata["streams"]
            .as_array()
            .and_then(|streams| streams.iter().find(|v| v["codec_type"] == "video"));
        if !video.is_some_and(|v| {
            v["codec_name"] == "h264"
                && v["pix_fmt"] == "yuv420p"
                && v["width"]
                    .as_u64()
                    .is_some_and(|n| (128..=1920).contains(&n) && n.is_multiple_of(2))
                && v["height"]
                    .as_u64()
                    .is_some_and(|n| (128..=1080).contains(&n) && n.is_multiple_of(2))
        }) {
            return Err(error(
                "UNSUPPORTED_ACCELERATION",
                "CUDA decode supports progressive H.264 yuv420p at even 128..1920 by 128..1080 dimensions",
            ));
        }
        let started = Instant::now();
        let args = ["-hide_banner", "-v", "error", "-nostdin", "-init_hw_device"]
            .map(str::to_owned)
            .into_iter()
            .chain([format!("cuda=cutbolt:{device}")])
            .chain(
                [
                    "-f",
                    "lavfi",
                    "-i",
                    "color=size=32x32:rate=1",
                    "-frames:v",
                    "1",
                    "-f",
                    "null",
                    "-",
                ]
                .map(str::to_owned),
            )
            .collect::<Vec<_>>();
        match media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(10)) {
            Ok(_) => {
                selected.selected_backend = "cuda";
                selected.device = Some(device);
            }
            Err(e) if e.code != "TOOL_FAILED" => return Err(e),
            Err(e) => {
                if matches!(unavailable, Unavailable::Error) {
                    return Err(error(
                        "DEVICE_UNAVAILABLE",
                        format!("CUDA device {device} initialization failed: {}", e.message),
                    ));
                }
                selected.fallback = Some(
                    json!({"reason":"device_initialization_failed","requested_device":device,"underlying_error":e}),
                );
            }
        }
        selected.availability_micros = started.elapsed().as_micros();
    }
    Ok(selected)
}

pub fn capabilities() -> Value {
    json!({"default":"cpu","optional":"cuda_nvdec","selection":"explicit_device_0_to_31",
        "fallback":"explicit_error_or_cpu_on_device_initialization_failure",
        "runtime_decode_failure":"error_without_implicit_retry","video":"progressive_h264_yuv420p",
        "source_even_dimensions":{"width":[128,1920],"height":[128,1080]},
        "accelerated_stage":"source_video_decode_only","other_stages":"cpu",
        "runtime_network":false})
}
