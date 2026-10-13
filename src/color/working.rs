//! SDR working pixels for the precise renderer under construction.
//!
//! These library primitives do not enable a new project or scene mode. RGB is straight,
//! linear-light UNORM16; alpha is linear coverage. Every compositing operation rounds once
//! to this representation. Encoded delivery and its spatially fixed dither are separate.
use super::Transfer;
pub mod context;
pub mod layer;
use crate::{
    Result,
    composite::{AlphaMode, BlendMode},
    error,
};
pub use context::{Context, Depth, Light, Space};

const Q: u128 = u16::MAX as u128;
const MASK: u128 = crate::composite::MASK_WEIGHT as u128;

/// Straight working RGB and coverage in 0..65535. Context-free methods use linear light;
/// frames with an explicit Context use its declared light and depth. Hidden RGB is allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pixel(pub [u16; 4]);

/// Quantization used only when delivering encoded 8-bit samples.
#[derive(Clone, Copy, Debug)]
pub enum Dither {
    /// Nearest integer, with exact half ties rounded upward.
    Nearest,
    /// An 8 by 8 ordered threshold, fixed to absolute canvas coordinates across frames.
    Ordered8x8,
}

fn rounded(numerator: u128, denominator: u128) -> u16 {
    ((numerator + denominator / 2) / denominator) as u16
}

impl Pixel {
    pub const TRANSPARENT: Self = Self([0; 4]);
    pub const BLACK: Self = Self([0, 0, 0, u16::MAX]);

    /// Decode encoded source bytes once. Premultiplication belongs to the encoded source:
    /// remove it before transfer decoding. Alpha never passes through a transfer curve.
    pub fn from_encoded8(rgba: [u8; 4], transfer: Transfer, alpha: AlphaMode) -> Result<Self> {
        crate::composite::validate_alpha(&rgba, alpha)?;
        let denominator = match alpha {
            AlphaMode::Straight => 255.0,
            AlphaMode::Premultiplied if rgba[3] == 0 => return Ok(Self::TRANSPARENT),
            AlphaMode::Premultiplied => f64::from(rgba[3]),
        };
        Self::from_linear([
            transfer.decode(f64::from(rgba[0]) / denominator),
            transfer.decode(f64::from(rgba[1]) / denominator),
            transfer.decode(f64::from(rgba[2]) / denominator),
            f64::from(rgba[3]) / 255.0,
        ])
    }

    /// Quantize finite linear values, clipping to the SDR working interval. Effects can keep
    /// their floating-point chain until this one store; no intermediate encoded bytes are used.
    pub fn from_linear(rgba: [f64; 4]) -> Result<Self> {
        if rgba.iter().any(|v| !v.is_finite()) {
            return Err(error(
                "INVALID_COLOR",
                "Working pixels require finite linear values",
            ));
        }
        Ok(Self(
            rgba.map(|v| (v.clamp(0.0, 1.0) * Q as f64).round() as u16),
        ))
    }

    /// Unquantized linear values for pixel effects, without a transfer round trip.
    pub fn linear(self) -> [f64; 4] {
        self.0.map(|v| f64::from(v) / Q as f64)
    }

    /// Composite this source over a possibly transparent backdrop. Opacity is UNORM16;
    /// mask coverage is 0..65536, matching the source-canvas mask sampler. Products retain
    /// all bits until the final RGB/alpha store. Normal, multiply and screen operate in
    /// linear light, including the uncovered-source term of a transparent backdrop.
    pub fn over(self, backdrop: Self, opacity: u16, mask: u32, mode: BlendMode) -> Result<Self> {
        if u128::from(mask) > MASK {
            return Err(error(
                "INVALID_MASK",
                "Working mask coverage must be in 0..65536",
            ));
        }
        let source = self.0.map(u128::from);
        let dest = backdrop.0.map(u128::from);
        let denominator = Q * Q * MASK;
        let coverage = source[3] * u128::from(opacity) * u128::from(mask);
        if coverage == 0 {
            return Ok(backdrop);
        }
        let rest = denominator - coverage;
        let alpha = coverage * Q + dest[3] * rest;
        let mut out = [0; 4];
        for c in 0..3 {
            let blend = match mode {
                BlendMode::Normal => source[c] * Q,
                BlendMode::Multiply => source[c] * dest[c],
                BlendMode::Screen => (source[c] + dest[c]) * Q - source[c] * dest[c],
            };
            let weighted = rest * dest[3] * dest[c] * Q
                + coverage * (Q - dest[3]) * source[c] * Q
                + coverage * dest[3] * blend;
            out[c] = rounded(weighted, alpha * Q);
        }
        out[3] = rounded(alpha, denominator);
        Ok(Self(out))
    }

    /// Interpolate premultiplied linear color and coverage, returning straight working pixels.
    /// A hidden color cannot bleed into a transition or a resampling tap. `weight` is the
    /// second pixel's UNORM16 weight; endpoints preserve the original pixel exactly.
    pub fn mix(self, other: Self, weight: u16) -> Self {
        if weight == 0 {
            return self;
        }
        if weight == u16::MAX {
            return other;
        }
        let a = self.0.map(u128::from);
        let b = other.0.map(u128::from);
        let w = u128::from(weight);
        let alpha = a[3] * (Q - w) + b[3] * w;
        if alpha == 0 {
            return Self::TRANSPARENT;
        }
        let mut out = [0; 4];
        for c in 0..3 {
            out[c] = rounded(a[c] * a[3] * (Q - w) + b[c] * b[3] * w, alpha);
        }
        out[3] = rounded(alpha, Q);
        Self(out)
    }

    /// Deliver straight encoded RGBA8 once. RGB receives the same threshold in each channel
    /// (no colored noise); alpha rounds independently without transfer or dither. Canvas x/y,
    /// rather than frame number or tile-local coordinates, keep the pattern stable.
    pub fn encoded8(self, transfer: Transfer, position: [u32; 2], dither: Dither) -> [u8; 4] {
        let threshold = match dither {
            Dither::Nearest => 0.5,
            Dither::Ordered8x8 => (f64::from(rank(position)) + 0.5) / 64.0,
        };
        let mut out = [0; 4];
        for (out, value) in out[..3].iter_mut().zip(self.0) {
            *out = (transfer.encode(f64::from(value) / Q as f64) * 255.0 + threshold)
                .floor()
                .clamp(0.0, 255.0) as u8;
        }
        out[3] = rounded(u128::from(self.0[3]) * 255, Q) as u8;
        out
    }
}

// Recursively expanding [[0,2],[3,1]] yields these 64 unique ordered thresholds.
fn rank([x, y]: [u32; 2]) -> u8 {
    const BAYER: [[u8; 8]; 8] = [
        [0, 32, 8, 40, 2, 34, 10, 42],
        [48, 16, 56, 24, 50, 18, 58, 26],
        [12, 44, 4, 36, 14, 46, 6, 38],
        [60, 28, 52, 20, 62, 30, 54, 22],
        [3, 35, 11, 43, 1, 33, 9, 41],
        [51, 19, 59, 27, 49, 17, 57, 25],
        [15, 47, 7, 39, 13, 45, 5, 37],
        [63, 31, 55, 23, 61, 29, 53, 21],
    ];
    BAYER[(y % 8) as usize][(x % 8) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_blend_and_transparency_do_not_pass_through_bytes() {
        let white = Pixel([65535; 4]);
        let half = white
            .over(Pixel::BLACK, 32768, 65536, BlendMode::Normal)
            .unwrap();
        assert_eq!(half.0, [32768, 32768, 32768, 65535]);
        assert_eq!(
            half.encoded8(Transfer::Srgb, [0; 2], Dither::Nearest),
            [188, 188, 188, 255]
        );
        let source = Pixel([34567, 12345, 45678, 27000]);
        for mode in [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen] {
            assert_eq!(
                source.over(Pixel::TRANSPARENT, 65535, 65536, mode).unwrap(),
                source
            );
        }
        assert_eq!(
            Pixel([65535, 0, 0, 0])
                .mix(Pixel([0, 0, 65535, 65535]), 32768)
                .0,
            [0, 0, 65535, 32768]
        );
    }

    #[test]
    fn invalid_inputs_and_alpha_are_explicit() {
        assert!(Pixel::from_linear([f64::NAN, 0.0, 0.0, 1.0]).is_err());
        assert!(Pixel::from_linear([0.0, f64::INFINITY, 0.0, 1.0]).is_err());
        assert!(
            Pixel::from_encoded8([200, 0, 0, 128], Transfer::Srgb, AlphaMode::Premultiplied)
                .is_err()
        );
        assert!(
            Pixel::BLACK
                .over(Pixel::BLACK, 0, 65537, BlendMode::Normal)
                .is_err()
        );
        let p = Pixel::from_encoded8([63, 32, 0, 128], Transfer::Srgb, AlphaMode::Premultiplied)
            .unwrap();
        assert_eq!(p.0[3], 128 * 257);
        assert_eq!(
            p.encoded8(Transfer::Srgb, [4, 7], Dither::Nearest),
            [126, 64, 0, 128]
        );
    }

    #[test]
    fn ordered_thresholds_cover_a_tile_and_never_change_with_tile_origin() {
        let mut ranks = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let p = Pixel([13000, 13000, 13000, 31000]);
                ranks.push(rank([x, y]));
                let output = p.encoded8(Transfer::Srgb, [x, y], Dither::Ordered8x8);
                assert_eq!(
                    output,
                    p.encoded8(Transfer::Srgb, [x + 24, y + 40], Dither::Ordered8x8)
                );
                assert_eq!(output[0], output[1]);
                assert_eq!(output[1], output[2]);
                assert_eq!(output[3], 121);
            }
        }
        ranks.sort_unstable();
        assert_eq!(ranks, (0..64).collect::<Vec<_>>());
    }
}
