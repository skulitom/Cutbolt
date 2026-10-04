//! Bounded, explicit delivery settings; no arbitrary encoder arguments.
use super::*;

/// Frames per two-second GOP at a native rate, rounding the rate to whole frames per second.
pub(super) fn gop(rate: Time) -> u64 {
    2 * ((rate.num + rate.den / 2) / rate.den)
}

/// H.264 stream profile: `baseline720p` (Constrained Baseline 3.1, or 3.2 above 30 fps at 720p, up to 1280x720), `main_hd` (Main 4.0, or 4.2 above 30 fps at 1080p, up to 1920x1080), `high_hd` (High, levels as Main). Dimensions must be even; no resizing.
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
    /// The lowest level that carries this frame size and rate: 3.1 or 3.2 for Constrained
    /// Baseline, 4.0 or 4.2 otherwise, by macroblocks per frame and per second.
    pub(super) fn level(self, width: u32, height: u32, rate: Time) -> Result<(&'static str, u32)> {
        let blocks = u64::from(width.div_ceil(16)) * u64::from(height.div_ceil(16));
        let per_second = (blocks * rate.num).div_ceil(rate.den);
        let levels: &[(&str, u32, u64, u64)] = if self == Self::Baseline720p {
            &[("3.1", 31, 3600, 108_000), ("3.2", 32, 5120, 216_000)]
        } else {
            &[("4.0", 40, 8192, 245_760), ("4.2", 42, 8704, 522_240)]
        };
        levels
            .iter()
            .find(|l| blocks <= l.2 && per_second <= l.3)
            .map(|l| (l.0, l.1))
            .ok_or_else(|| {
                invalid(
                    "Frame size and rate exceed the chosen compatibility profile's highest level",
                )
            })
    }
    pub(super) fn b_frames(self) -> u32 {
        if self == Self::Baseline720p { 0 } else { 2 }
    }
    pub(super) fn references(self) -> u32 {
        if self == Self::Baseline720p { 1 } else { 3 }
    }
}

/// H.264 rate control, tagged by `mode`. Maximum bitrate is 100000 to 12000000 bits/s (baseline720p) or 20000000; buffer is at least the maximum and at most 14000000 or 25000000 bits.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RateControl {
    /// Constant-quality encoding with a buffering cap.
    Quality {
        /// Constant rate factor, 10 to 35; lower means higher quality and larger output.
        crf: u8,
        /// Encoder maximum rate in bits/s.
        maximum_bitrate: u32,
        /// Rate-control buffer in bits.
        buffer_size: u32,
    },
    /// Two-pass encoding toward an average video bitrate.
    TwoPass {
        /// Target average video bitrate in bits/s, 100000 to `maximum_bitrate`.
        bitrate: u32,
        /// Encoder maximum rate in bits/s.
        maximum_bitrate: u32,
        /// Rate-control buffer in bits.
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

/// Explicit H.264 encoder settings; omitting them selects high_hd with CRF 18, 12000000 bits/s maximum and a 24000000-bit buffer.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct H264 {
    /// Stream profile and level, which bound dimensions and rates.
    pub compatibility: Compatibility,
    /// Quality or two-pass bitrate control.
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
    pub(super) fn validate(self, width: u32, height: u32, rate: Time) -> Result<()> {
        self.compatibility.level(width, height, rate)?;
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
    pub(super) fn video_report(self, width: u32, height: u32, rate: Time) -> Value {
        let level = self
            .compatibility
            .level(width, height, rate)
            .map(|l| l.0)
            .unwrap_or("unsupported");
        let mut report = json!({"codec":"h264","encoder":"libx264","profile":self.compatibility.decoded_profile(),"level":level,"preset":"medium",
            "compatibility":self.compatibility,"rate_control":self.rate_control,"maximum_bitrate":self.rate_control.maximum(),"buffer_size":self.rate_control.buffer(),
            "pixel_format":"yuv420p","matrix":"bt709","primaries":"bt709","transfer":"bt709","range":"limited","chroma_location":"left","chroma_filter":"bilinear",
            "gop_frames":gop(rate),"b_frames":self.compatibility.b_frames(),"reference_frames":self.compatibility.references()});
        if let RateControl::Quality { crf, .. } = self.rate_control {
            report["crf"] = json!(crf);
        }
        report
    }
    pub(super) fn arguments(
        self,
        args: &mut Vec<String>,
        pass: Option<u8>,
        scratch: &Path,
        level: &str,
    ) {
        args.extend([
            "-profile:v".into(),
            self.compatibility.profile().into(),
            "-level:v".into(),
            level.into(),
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
