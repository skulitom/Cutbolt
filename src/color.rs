//! Declared SDR sample interpretation and original transfer/matrix/range conversion.
use crate::{Result, error};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

mod lookup;
pub use lookup::Yuv420Converter;

/// SDR transfer function of encoded RGB or YUV values: `srgb` (sRGB piecewise curve) or `bt709` (BT.709 OETF).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    Srgb,
    Bt709,
}
impl Transfer {
    /// The name used in requests.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Srgb => "srgb",
            Self::Bt709 => "bt709",
        }
    }
    /// The transfer a request applies: its own `input_transfer`, else the project's declared
    /// `transfer`. A request value that contradicts the declaration fails with `code`.
    pub(crate) fn declared(
        code: &'static str,
        request: Option<Self>,
        project: Option<Self>,
    ) -> Result<Option<Self>> {
        match (request, project) {
            (Some(given), Some(declared)) if given != declared => Err(error(
                code,
                format!(
                    "input_transfer {} contradicts the project's declared transfer {}; omit input_transfer, or change the declaration with project.transfer",
                    given.name(),
                    declared.name()
                ),
            )),
            _ => Ok(request.or(project)),
        }
    }
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Srgb => "iec61966-2-1",
            Self::Bt709 => "bt709",
        }
    }
    fn decode(self, v: f64) -> f64 {
        match self {
            Self::Srgb => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Self::Bt709 => {
                if v < 0.081 {
                    v / 4.5
                } else {
                    ((v + 0.099) / 1.099).powf(1.0 / 0.45)
                }
            }
        }
    }
    fn encode(self, v: f64) -> f64 {
        match self {
            Self::Srgb => {
                if v <= 0.0031308 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                }
            }
            Self::Bt709 => {
                if v < 0.018 {
                    4.5 * v
                } else {
                    1.099 * v.powf(0.45) - 0.099
                }
            }
        }
    }
}
/// Sample matrix: `rgb` for FFV1/bgr0 RGB, `bt709` for the declared YUV source formats in color.capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Matrix {
    Rgb,
    Bt709,
}
/// Code range: `full` (0-255 at 8 bits) or `limited` (luma 16-235, chroma 16-240 at 8 bits, scaled for higher depths).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Range {
    Full,
    Limited,
}
/// Absent or unknown color tags: `reject` fails; `use_declared` assumes the declared values and reports them. Conflicting tags always fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MissingTags {
    Reject,
    UseDeclared,
}
/// Declared SDR interpretation of a conform source (`source.sdr`); BT.709 primaries and D65 are implied.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Input {
    /// Sample matrix; must match the stored pixel format.
    pub matrix: Matrix,
    /// Code range of the stored samples; YUVJ formats require `full`.
    pub range: Range,
    /// Transfer function of the source values.
    pub transfer: Transfer,
    /// Policy for absent or unknown stream and frame color tags.
    pub missing_tags: MissingTags,
}
#[derive(Clone, Copy)]
pub(crate) enum Layout {
    Bgr0,
    Yuv444,
    Yuv420,
    /// Other accepted planar 4:2:0/4:2:2 formats. Wider samples are little-endian words.
    SubsampledYuv {
        bits: u8,
        vertical: usize,
    },
}
impl Layout {
    pub(crate) fn bytes(self, width: u32, height: u32) -> usize {
        let n = width as usize * height as usize;
        match self {
            Self::Bgr0 => n * 4,
            Self::Yuv444 => n * 3,
            Self::Yuv420 => n * 3 / 2,
            Self::SubsampledYuv { bits, vertical } => {
                (n + n / vertical) * if bits == 8 { 1 } else { 2 }
            }
        }
    }
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_COLOR", message)
}
impl Input {
    pub(crate) fn inspect(self, stream: &Value, mkv: bool, mp4: bool) -> Result<(Layout, Value)> {
        let layout = match (self.matrix, stream["pix_fmt"].as_str()) {
            (Matrix::Rgb, Some("bgr0")) if mkv && stream["codec_name"] == "ffv1" => Layout::Bgr0,
            (Matrix::Bt709, Some("yuv444p")) if mkv && stream["codec_name"] == "ffv1" => {
                Layout::Yuv444
            }
            (Matrix::Bt709, Some("yuv420p" | "yuvj420p"))
                if (mp4 || mkv)
                    && (stream["codec_name"] == "h264"
                        || stream["codec_name"] == "hevc" && stream["profile"] == "Main") =>
            {
                Layout::Yuv420
            }
            (Matrix::Bt709, Some("yuv420p10le"))
                if (mp4 || mkv)
                    && (stream["codec_name"] == "h264"
                        || stream["codec_name"] == "hevc" && stream["profile"] == "Main 10") =>
            {
                Layout::SubsampledYuv {
                    bits: 10,
                    vertical: 2,
                }
            }
            (Matrix::Bt709, Some("yuv422p" | "yuvj422p"))
                if (mp4 || mkv) && stream["codec_name"] == "h264" =>
            {
                Layout::SubsampledYuv {
                    bits: 8,
                    vertical: 1,
                }
            }
            (Matrix::Bt709, Some("yuv422p10le"))
                if (mp4 || mkv)
                    && matches!(stream["codec_name"].as_str(), Some("h264" | "prores")) =>
            {
                Layout::SubsampledYuv {
                    bits: 10,
                    vertical: 1,
                }
            }
            _ => {
                return Err(invalid(
                    "SDR normalization requires declared FFV1/bgr0 or YUV444 MKV; H.264/HEVC 8/10-bit YUV420; or H.264 8/10-bit/ProRes 10-bit YUV422 in MP4/MOV/MKV",
                ));
            }
        };
        let vertical = match layout {
            Layout::Yuv420 => Some(2),
            Layout::SubsampledYuv { vertical, .. } => Some(vertical as u64),
            _ => None,
        };
        if let Some(vertical) = vertical
            && (!stream["width"].as_u64().unwrap_or(0).is_multiple_of(2)
                || !stream["height"]
                    .as_u64()
                    .unwrap_or(0)
                    .is_multiple_of(vertical))
        {
            return Err(invalid(if vertical == 2 {
                "SDR YUV420 normalization requires even dimensions"
            } else {
                "SDR YUV422 normalization requires an even width"
            }));
        }
        // This deprecated pixel-format name itself declares full range even if metadata is absent.
        if matches!(stream["pix_fmt"].as_str(), Some("yuvj420p" | "yuvj422p"))
            && self.range != Range::Full
        {
            return Err(invalid(
                "YUVJ pixel format conflicts with a limited-range declaration",
            ));
        }
        let mut assumed = Vec::new();
        let mut check = |field: &str, expected: &str| -> Result<()> {
            match stream[field].as_str() {
                None | Some("unknown" | "unspecified") => {
                    if self.missing_tags == MissingTags::Reject {
                        return Err(invalid(format!(
                            "Missing {field}; explicitly use declared values to interpret untagged media"
                        )));
                    }
                    assumed.push(field.to_owned());
                }
                Some(value) if value == expected => (),
                Some(value) => {
                    return Err(invalid(format!(
                        "Conflicting {field}: observed {value}, declared {expected}"
                    )));
                }
            }
            Ok(())
        };
        check(
            "color_space",
            if self.matrix == Matrix::Rgb {
                "gbr"
            } else {
                "bt709"
            },
        )?;
        check(
            "color_range",
            if self.range == Range::Full {
                "pc"
            } else {
                "tv"
            },
        )?;
        check("color_primaries", "bt709")?;
        check("color_transfer", self.transfer.tag())?;
        if vertical.is_some() {
            check("chroma_location", "left")?;
        }
        Ok((
            layout,
            json!({"input":self,"assumed_tags":assumed,"primaries":"bt709","white_point":"D65","chroma_reconstruction":match vertical {Some(2)=>"nearest_2x2_block",Some(1)=>"nearest_2x1_block",_=>"none"},"clipping":"encoded_rgb_to_0_1_before_transfer","precision":"f64_then_nearest_8bit","output_matrix":"gbr","output_range":"full"}),
        ))
    }
    fn byte(self, value: f64, output: Transfer) -> u8 {
        let value = value.clamp(0.0, 1.0);
        let encoded = if self.transfer == output {
            value
        } else {
            output.encode(self.transfer.decode(value))
        };
        (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
    }
    pub(crate) fn convert(
        self,
        layout: Layout,
        width: u32,
        height: u32,
        source: &[u8],
        output: Transfer,
        rgb: &mut [u8],
    ) {
        let n = width as usize * height as usize;
        for i in 0..n {
            let values = match layout {
                Layout::Bgr0 => {
                    let (offset, divisor) = if self.range == Range::Full {
                        (0.0, 255.0)
                    } else {
                        (16.0, 219.0)
                    };
                    [2, 1, 0].map(|c| (source[i * 4 + c] as f64 - offset) / divisor)
                }
                Layout::SubsampledYuv { bits, vertical } => {
                    let plane = n / 2 / vertical;
                    let chroma = i / width as usize / vertical * (width as usize / 2)
                        + i % width as usize / 2;
                    let sample = |index: usize| -> f64 {
                        if bits == 8 {
                            source[index] as f64
                        } else {
                            u16::from_le_bytes([source[2 * index], source[2 * index + 1]]) as f64
                        }
                    };
                    let scale = (1u32 << (bits - 8)) as f64;
                    let full = ((1u32 << bits) - 1) as f64;
                    let (offset, yscale, cscale) = if self.range == Range::Limited {
                        (16.0 * scale, 219.0 * scale, 224.0 * scale)
                    } else {
                        (0.0, full, full)
                    };
                    let y = (sample(i) - offset) / yscale;
                    let cb = (sample(n + chroma) - 128.0 * scale) / cscale;
                    let cr = (sample(n + plane + chroma) - 128.0 * scale) / cscale;
                    let r = y + 1.5748 * cr;
                    let b = y + 1.8556 * cb;
                    [r, (y - 0.2126 * r - 0.0722 * b) / 0.7152, b]
                }
                Layout::Yuv444 | Layout::Yuv420 => {
                    let (chroma, plane) = if matches!(layout, Layout::Yuv444) {
                        (i, n)
                    } else {
                        (
                            (i / width as usize / 2) * (width as usize / 2)
                                + i % width as usize / 2,
                            n / 4,
                        )
                    };
                    let (offset, yscale, cscale) = if self.range == Range::Limited {
                        (16.0, 219.0, 224.0)
                    } else {
                        (0.0, 255.0, 255.0)
                    };
                    let y = (source[i] as f64 - offset) / yscale;
                    let cb = (source[n + chroma] as f64 - 128.0) / cscale;
                    let cr = (source[n + plane + chroma] as f64 - 128.0) / cscale;
                    let r = y + 1.5748 * cr;
                    let b = y + 1.8556 * cb;
                    [r, (y - 0.2126 * r - 0.0722 * b) / 0.7152, b]
                }
            };
            for c in 0..3 {
                rgb[i * 3 + c] = self.byte(values[c], output);
            }
        }
    }
}
pub fn capabilities() -> Value {
    json!({"profile":"sdr-normalization-v1","inputs":["ffv1_bgr0_yuv444_mkv","h264_yuv420_mp4_mov_mkv","hevc_main_yuv420_mp4_mov_mkv","h264_hevc_10bit_yuv420_mp4_mov_mkv","h264_8_10bit_yuv422_mp4_mov_mkv","prores_10bit_yuv422_mp4_mov_mkv"],"input_matrix":["rgb","bt709"],"input_range":["full","limited"],"transfer":["srgb","bt709"],"primaries":"bt709","missing_tags":["reject","use_declared"],"conflicting_tags":"reject","output":"tagged_full_range_rgb_ffv1","precision":"8_or_10bit_samples_f64_conversion_to_8bit","hdr":false,"icc":false})
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wider_samples_preserve_limited_codes_and_chroma_rows() {
        let input = Input {
            matrix: Matrix::Bt709,
            range: Range::Limited,
            transfer: Transfer::Bt709,
            missing_tags: MissingTags::Reject,
        };
        let original = [16u8, 235, 81, 100, 16, 235, 81, 100, 90, 128, 240, 128];
        let mut expected = [0; 24];
        input.convert(
            Layout::Yuv420,
            4,
            2,
            &original,
            Transfer::Srgb,
            &mut expected,
        );
        for (bits, vertical) in [(10, 2), (8, 1), (10, 1)] {
            let codes = if vertical == 2 {
                original.to_vec()
            } else {
                [
                    original[..8].to_vec(),
                    vec![90, 128, 90, 128, 240, 128, 240, 128],
                ]
                .concat()
            };
            let bytes = if bits == 8 {
                codes
            } else {
                codes
                    .into_iter()
                    .flat_map(|v| (v as u16 * 4).to_le_bytes())
                    .collect()
            };
            let layout = Layout::SubsampledYuv { bits, vertical };
            assert_eq!(layout.bytes(4, 2), bytes.len());
            let mut got = [0; 24];
            input.convert(layout, 4, 2, &bytes, Transfer::Srgb, &mut got);
            assert_eq!(got, expected);
        }
        let mut stream = json!({"codec_name":"prores","pix_fmt":"yuv422p10le",
            "width":4,"height":3,"color_space":"bt709","color_range":"tv",
            "color_primaries":"bt709","color_transfer":"bt709","chroma_location":"left"});
        assert!(input.inspect(&stream, false, true).is_ok());
        stream["width"] = json!(3);
        assert!(input.inspect(&stream, false, true).is_err());
        stream["width"] = json!(4);
        stream["pix_fmt"] = json!("yuv420p10le");
        stream["codec_name"] = json!("hevc");
        stream["profile"] = json!("Main 10");
        assert!(input.inspect(&stream, false, true).is_err()); // odd 4:2:0 height
        stream["height"] = json!(2);
        assert!(input.inspect(&stream, false, true).is_ok());
        stream["profile"] = json!("Main");
        assert!(input.inspect(&stream, false, true).is_err());
    }

    #[test]
    fn transfer_anchors_and_range_endpoints() {
        let input = Input {
            matrix: Matrix::Rgb,
            range: Range::Limited,
            transfer: Transfer::Srgb,
            missing_tags: MissingTags::Reject,
        };
        let mut rgb = [0; 6];
        input.convert(
            Layout::Bgr0,
            2,
            1,
            &[16, 16, 16, 0, 235, 235, 235, 0],
            Transfer::Srgb,
            &mut rgb,
        );
        assert_eq!(rgb, [0, 0, 0, 255, 255, 255]);
        assert_eq!(input.byte(128.0 / 255.0, Transfer::Bt709), 115);
        assert_eq!(
            Input {
                transfer: Transfer::Bt709,
                ..input
            }
            .byte(128.0 / 255.0, Transfer::Srgb),
            140
        );
        assert!((Transfer::Srgb.decode(0.04045) - 0.0031308).abs() < 1e-7);
        assert_eq!(Transfer::Bt709.decode(0.0), 0.0);
        assert!((Transfer::Bt709.encode(1.0) - 1.0).abs() < 1e-12);
    }
}
