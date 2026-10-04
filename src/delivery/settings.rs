//! Bounded, explicit delivery settings; no arbitrary encoder arguments.
use super::*;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Compatibility {
    Baseline720p,
    MainHd,
    HighHd,
}
impl Compatibility {
    pub(super) fn profile(self) -> &'static str {
        match self {
            Self::Baseline720p => "baseline",
            Self::MainHd => "main",
            Self::HighHd => "high",
        }
    }
    pub(super) fn decoded_profile(self) -> &'static str {
        match self {
            Self::Baseline720p => "Constrained Baseline",
            Self::MainHd => "Main",
            Self::HighHd => "High",
        }
    }
    pub(super) fn level(self) -> (&'static str, u32) {
        if self == Self::Baseline720p {
            ("3.1", 31)
        } else {
            ("4.0", 40)
        }
    }
    pub(super) fn b_frames(self) -> u32 {
        if self == Self::Baseline720p { 0 } else { 2 }
    }
    pub(super) fn references(self) -> u32 {
        if self == Self::Baseline720p { 1 } else { 3 }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RateControl {
    Quality {
        crf: u8,
        maximum_bitrate: u32,
        buffer_size: u32,
    },
    TwoPass {
        bitrate: u32,
        maximum_bitrate: u32,
        buffer_size: u32,
    },
}
impl RateControl {
    pub(super) fn two_pass(self) -> bool {
        matches!(self, Self::TwoPass { .. })
    }
    pub(super) fn maximum(self) -> u32 {
        match self {
            Self::Quality {
                maximum_bitrate, ..
            }
            | Self::TwoPass {
                maximum_bitrate, ..
            } => maximum_bitrate,
        }
    }
    pub(super) fn buffer(self) -> u32 {
        match self {
            Self::Quality { buffer_size, .. } | Self::TwoPass { buffer_size, .. } => buffer_size,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct H264 {
    pub compatibility: Compatibility,
    pub rate_control: RateControl,
}
impl Default for H264 {
    fn default() -> Self {
        Self {
            compatibility: Compatibility::HighHd,
            rate_control: RateControl::Quality {
                crf: 18,
                maximum_bitrate: 12_000_000,
                buffer_size: 24_000_000,
            },
        }
    }
}
impl H264 {
    pub(super) fn validate(self, width: u32, height: u32) -> Result<()> {
        let baseline = self.compatibility == Compatibility::Baseline720p;
        let (w, h) = if baseline { (1280, 720) } else { (1920, 1080) };
        if width < 2
            || height < 2
            || width > w
            || height > h
            || !width.is_multiple_of(2)
            || !height.is_multiple_of(2)
        {
            return Err(invalid(
                "H.264 requires even dimensions inside the chosen compatibility profile; no automatic resize",
            ));
        }
        let max = self.rate_control.maximum();
        let buffer = self.rate_control.buffer();
        let (max_cap, buffer_cap) = if baseline {
            (12_000_000, 14_000_000)
        } else {
            (20_000_000, 25_000_000)
        };
        if !(100_000..=max_cap).contains(&max) || buffer < max || buffer > buffer_cap {
            return Err(invalid(
                "Video maximum bitrate/buffer is outside the selected profile bounds; buffer must hold at least one second at maximum bitrate",
            ));
        }
        match self.rate_control {
            RateControl::Quality { crf, .. } if !(10..=35).contains(&crf) => {
                Err(invalid("Quality CRF must be 10..35"))
            }
            RateControl::TwoPass { bitrate, .. } if !(100_000..=max).contains(&bitrate) => Err(
                invalid("Two-pass target bitrate must be 100000..maximum_bitrate"),
            ),
            _ => Ok(()),
        }
    }
    pub(super) fn video_report(self) -> Value {
        let mut report = json!({"codec":"h264","encoder":"libx264","profile":self.compatibility.decoded_profile(),"level":self.compatibility.level().0,"preset":"medium",
            "compatibility":self.compatibility,"rate_control":self.rate_control,"maximum_bitrate":self.rate_control.maximum(),"buffer_size":self.rate_control.buffer(),
            "pixel_format":"yuv420p","matrix":"bt709","primaries":"bt709","transfer":"bt709","range":"limited","chroma_location":"left","chroma_filter":"bilinear",
            "gop_frames":50,"b_frames":self.compatibility.b_frames(),"reference_frames":self.compatibility.references()});
        if let RateControl::Quality { crf, .. } = self.rate_control {
            report["crf"] = json!(crf);
        }
        report
    }
    pub(super) fn arguments(self, args: &mut Vec<String>, pass: Option<u8>, scratch: &Path) {
        args.extend([
            "-profile:v".into(),
            self.compatibility.profile().into(),
            "-level:v".into(),
            self.compatibility.level().0.into(),
            "-maxrate".into(),
            self.rate_control.maximum().to_string(),
            "-bufsize".into(),
            self.rate_control.buffer().to_string(),
            "-bf".into(),
            self.compatibility.b_frames().to_string(),
            "-refs".into(),
            self.compatibility.references().to_string(),
        ]);
        match self.rate_control {
            RateControl::Quality { crf, .. } => args.extend(["-crf".into(), crf.to_string()]),
            RateControl::TwoPass { bitrate, .. } => args.extend([
                "-b:v".into(),
                bitrate.to_string(),
                "-pass".into(),
                pass.expect("two-pass phase").to_string(),
                "-passlogfile".into(),
                scratch.join("rate-control").to_string_lossy().into_owned(),
                "-fastfirstpass".into(),
                "0".into(),
            ]),
        }
    }
}
