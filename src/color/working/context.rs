//! Explicit storage depth and blending light for working frames. Alpha is always coverage.
use super::{Dither, Pixel};
use crate::{
    Result,
    color::Transfer,
    composite::{AlphaMode, BlendMode},
    error,
};
use serde::{Deserialize, Serialize};

/// Stored RGB and coverage precision, either eight or sixteen bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u8", into = "u8")]
pub enum Depth {
    Eight,
    Sixteen,
}
impl From<Depth> for u8 {
    fn from(value: Depth) -> Self {
        match value {
            Depth::Eight => 8,
            Depth::Sixteen => 16,
        }
    }
}
impl TryFrom<u8> for Depth {
    type Error = &'static str;
    fn try_from(value: u8) -> std::result::Result<Self, Self::Error> {
        match value {
            8 => Ok(Self::Eight),
            16 => Ok(Self::Sixteen),
            _ => Err("Working depth must be 8 or 16"),
        }
    }
}
impl schemars::JsonSchema for Depth {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "WorkingDepth".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        serde_json::json!({"description":"Stored RGB and coverage precision: 8 or 16 bits.","type":"integer","enum":[8,16]})
            .try_into()
            .expect("depth schema")
    }
}
/// Blend and resample transfer-encoded values or linear light. Alpha remains coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "WorkingLight")]
pub enum Light {
    Encoded,
    Linear,
}
/// Storage precision and the space used for blending and resampling. The default remains
/// the existing byte renderer; the other combinations use explicit working frame storage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Space {
    /// Stored RGB/coverage depth: 8 or 16. Eight-bit working values occupy multiples of 257 in a u16 carrier.
    pub bits: Depth,
    /// Blend/resample encoded values or linear light. Alpha never passes through a transfer curve.
    pub light: Light,
}
impl Default for Space {
    fn default() -> Self {
        Self {
            bits: Depth::Eight,
            light: Light::Encoded,
        }
    }
}
impl Space {
    pub fn is_legacy(self) -> bool {
        self == Self::default()
    }
}

/// A frame's storage interpretation. A Pixel returned in this context contains straight RGB
/// in the selected light, plus coverage; component APIs without a context remain linear16.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub space: Space,
    /// Encoded working/delivery transfer. Source transfers are supplied separately at import.
    pub transfer: Transfer,
}
impl Context {
    pub const LINEAR16: Self = Self {
        space: Space {
            bits: Depth::Sixteen,
            light: Light::Linear,
        },
        transfer: Transfer::Srgb,
    };
    pub fn quantum(self, value: u16) -> u16 {
        match self.space.bits {
            Depth::Sixteen => value,
            Depth::Eight => ((u32::from(value) + 128) / 257 * 257) as u16,
        }
    }
    pub fn quantize(self, pixel: Pixel) -> Pixel {
        Pixel(pixel.0.map(|v| self.quantum(v)))
    }
    /// Store normalized values already in this context's blending light.
    pub fn from_components(self, values: [f64; 4]) -> Result<Pixel> {
        Ok(self.quantize(Pixel::from_linear(values)?))
    }
    /// Convert a continuous linear effect result to the selected storage representation once.
    pub fn from_linear(self, mut values: [f64; 4]) -> Result<Pixel> {
        if values.iter().any(|v| !v.is_finite()) {
            return Err(error(
                "INVALID_COLOR",
                "Working pixels require finite linear values",
            ));
        }
        if self.space.light == Light::Encoded {
            for v in &mut values[..3] {
                *v = self.transfer.encode(v.clamp(0.0, 1.0));
            }
        }
        self.from_components(values)
    }
    /// Continuous linear input for the existing effect equations; no temporary byte or u16 store.
    pub fn linear(self, pixel: Pixel) -> [f64; 4] {
        let mut values = pixel.linear();
        if self.space.light == Light::Encoded {
            for v in &mut values[..3] {
                *v = self.transfer.decode(*v);
            }
        }
        values
    }
    pub fn decode8(self, rgba: [u8; 4], source: Transfer, alpha: AlphaMode) -> Result<Pixel> {
        crate::composite::validate_alpha(&rgba, alpha)?;
        let denominator = match alpha {
            AlphaMode::Straight => 255.0,
            AlphaMode::Premultiplied if rgba[3] == 0 => return Ok(Pixel::TRANSPARENT),
            AlphaMode::Premultiplied => f64::from(rgba[3]),
        };
        self.from_linear([
            source.decode(f64::from(rgba[0]) / denominator),
            source.decode(f64::from(rgba[1]) / denominator),
            source.decode(f64::from(rgba[2]) / denominator),
            f64::from(rgba[3]) / 255.0,
        ])
    }
    pub fn over(
        self,
        source: Pixel,
        backdrop: Pixel,
        opacity: u16,
        mask: u32,
        mode: BlendMode,
    ) -> Result<Pixel> {
        source
            .over(backdrop, self.quantum(opacity), mask, mode)
            .map(|p| self.quantize(p))
    }
    /// One final transfer/ordered quantization. This is never used for child sequence frames.
    pub fn encoded8(self, pixel: Pixel, position: [u32; 2], dither: Dither) -> [u8; 4] {
        self.encoded8_as(pixel, self.transfer, position, dither)
    }
    /// Encode straight working RGB directly to the delivery transfer, without an intermediate
    /// store in the project's encoded transfer. Encoded identity delivery preserves exact codes.
    pub fn encoded8_as(
        self,
        pixel: Pixel,
        transfer: Transfer,
        position: [u32; 2],
        dither: Dither,
    ) -> [u8; 4] {
        if self.space.light == Light::Linear {
            return pixel.encoded8(transfer, position, dither);
        }
        let mut values = pixel.linear();
        if transfer != self.transfer {
            for v in &mut values[..3] {
                *v = transfer.encode(self.transfer.decode(*v));
            }
        }
        let threshold = match dither {
            Dither::Nearest => 0.5,
            Dither::Ordered8x8 => (f64::from(super::rank(position)) + 0.5) / 64.0,
        };
        let mut out = [0; 4];
        for (out, value) in out[..3].iter_mut().zip(values) {
            *out = (value * 255.0 + threshold).floor().clamp(0.0, 255.0) as u8;
        }
        out[3] = ((u32::from(pixel.0[3]) + 128) / 257) as u8;
        out
    }
}
