//! Original bounded color qualifiers and source-canvas correction masks.
use crate::{
    Result,
    composite::{MaskAnimation, MaskSampler, RectMask},
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};

/// Circular hue band in millidegrees: weight 1 within `inner` of center, falling linearly to 0 at `outer`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HueBand {
    /// Center hue in millidegrees, 0..359999.
    pub center: u32,
    /// Full-weight radius in millidegrees; at most `outer`.
    pub inner: u32,
    /// Zero-weight radius in millidegrees, at most 180000; equal to inner for a hard edge.
    pub outer: u32,
}
/// Scalar band in thousandths: weight 1 inside the inclusive low..high range, falling linearly to 0 over `feather`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Band {
    /// Inclusive lower bound in thousandths, 0..1000; at most `high`.
    pub low: u16,
    /// Inclusive upper bound in thousandths, 0..1000.
    pub high: u16,
    /// Falloff distance outside each bound in thousandths, 0..1000; 0 is a hard edge.
    pub feather: u16,
}
impl Band {
    fn validate(&self) -> bool {
        self.low <= self.high && self.high <= 1000 && self.feather <= 1000
    }
    fn weight(&self, value: u16) -> f64 {
        let distance = if value < self.low {
            self.low - value
        } else {
            value.saturating_sub(self.high)
        };
        if distance == 0 {
            1.0
        } else if self.feather == 0 {
            0.0
        } else {
            (1.0 - distance as f64 / self.feather as f64).max(0.0)
        }
    }
}
/// HSL color selection measured on 8-bit encoded sRGB; weight is the product of present bands. Needs at least one band.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Qualifier {
    /// Hue band; black, white and exact gray have no hue and get weight 0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hue: Option<HueBand>,
    /// HSL saturation band in thousandths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub saturation: Option<Band>,
    /// HSL lightness band in thousandths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightness: Option<Band>,
    /// Use 1 minus the combined weight; default false.
    #[serde(default)]
    pub inverted: bool,
}
impl Qualifier {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.hue.is_none() && self.saturation.is_none() && self.lightness.is_none()
            || self
                .hue
                .as_ref()
                .is_some_and(|h| h.center >= 360000 || h.inner > h.outer || h.outer > 180000)
            || [&self.saturation, &self.lightness]
                .into_iter()
                .flatten()
                .any(|b| !b.validate())
        {
            return Err(error(
                "INVALID_EFFECT",
                "A qualifier needs a hue/saturation/lightness band; hue center 0..359999, inner <= outer <=180000, scalar bands 0..1000 with low <= high and feather <=1000",
            ));
        }
        Ok(())
    }
    pub(crate) fn weight(&self, rgb: [u8; 3]) -> f64 {
        let (hue, saturation, lightness) = hsl(rgb);
        let mut weight = 1.0;
        if let Some(band) = &self.hue {
            weight = if let Some(hue) = hue {
                let direct = hue.abs_diff(band.center);
                let distance = direct.min(360000 - direct);
                if distance <= band.inner {
                    1.0
                } else if distance >= band.outer {
                    0.0
                } else {
                    (band.outer - distance) as f64 / (band.outer - band.inner) as f64
                }
            } else {
                0.0
            };
        }
        if let Some(band) = &self.saturation {
            weight *= band.weight(saturation);
        }
        if let Some(band) = &self.lightness {
            weight *= band.weight(lightness);
        }
        if self.inverted { 1.0 - weight } else { weight }
    }
}
fn rounded(num: i32, den: i32) -> i32 {
    let magnitude = (num.abs() * 2 + den) / (2 * den);
    if num < 0 { -magnitude } else { magnitude }
}
// Exact HSL coordinates from quantized encoded RGB, with integer ties away from zero.
// Hue uses millidegrees; saturation and lightness use thousandths. Gray has no hue.
fn hsl(rgb: [u8; 3]) -> (Option<u32>, u16, u16) {
    let [r, g, b] = rgb.map(i32::from);
    let hi = r.max(g).max(b);
    let lo = r.min(g).min(b);
    let delta = hi - lo;
    let lightness = rounded((hi + lo) * 1000, 510) as u16;
    if delta == 0 {
        return (None, 0, lightness);
    }
    let saturation = rounded(delta * 1000, 255 - (hi + lo - 255).abs()) as u16;
    let numerator = if hi == r {
        (g - b) * 60000
    } else if hi == g {
        (b - r) * 60000 + 120000 * delta
    } else {
        (r - g) * 60000 + 240000 * delta
    };
    // Wrap before rounding so a near-360-degree value never becomes a negative half tie.
    let hue = rounded(numerator.rem_euclid(360000 * delta), delta).rem_euclid(360000) as u32;
    (Some(hue), saturation, lightness)
}

/// Correction mask weighting a selective grade or chroma key, in source-canvas pixels before crop and transform.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mask {
    /// `[x, y, width, height]`; x/y -32768..32768, size 0..32768; may extend outside the canvas.
    pub rect: [i32; 4],
    /// Inward linear ramp from each edge in source pixels, 0..4096; 0 is a hard edge.
    pub feather: u16,
    /// Use 1 minus coverage; default false.
    #[serde(default)]
    pub inverted: bool,
    /// Optional curves overriding rectangle components on the layer-local clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<MaskAnimation>,
}
impl Mask {
    pub(crate) fn prepare(&self, duration: Time) -> Result<MaskSampler> {
        if self.feather > 4096 {
            return Err(error(
                "INVALID_EFFECT",
                "Correction-mask feather must be 0..4096 source pixels",
            ));
        }
        RectMask {
            rect: self.rect,
            inverted: self.inverted,
            animation: self.animation.clone(),
            feather: None,
        }
        .prepare(duration)
    }
    pub(crate) fn weight(&self, rect: [i32; 4], position: [i64; 2]) -> f64 {
        let [left, top, width, height] = rect.map(f64::from);
        let [x, y] = position.map(|p| p as f64 + 0.5);
        let distance = (x - left)
            .min(left + width - x)
            .min(y - top)
            .min(top + height - y);
        let weight = if width <= 0.0 || height <= 0.0 || distance <= 0.0 {
            0.0
        } else if self.feather == 0 {
            1.0
        } else {
            (distance / self.feather as f64).min(1.0)
        };
        if self.inverted { 1.0 - weight } else { weight }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hsl_anchors_and_half_ties() {
        for (rgb, hue) in [
            ([255, 0, 0], 0),
            ([255, 255, 0], 60000),
            ([0, 255, 0], 120000),
            ([0, 255, 255], 180000),
            ([0, 0, 255], 240000),
            ([255, 0, 255], 300000),
        ] {
            assert_eq!(hsl(rgb), (Some(hue), 1000, 500));
        }
        assert_eq!(hsl([0; 3]), (None, 0, 0));
        assert_eq!(hsl([128; 3]), (None, 0, 502));
        assert_eq!(hsl([255; 3]), (None, 0, 1000));
        assert_eq!(hsl([64, 1, 0]), (Some(938), 1000, 125));
        assert_eq!(hsl([64, 0, 1]), (Some(359063), 1000, 125));
        assert_eq!(hsl([68, 60, 60]), (Some(0), 63, 251));
    }
    #[test]
    fn correction_mask_centers_and_empty_inversion() {
        let mut mask = Mask {
            rect: [2, 2, 6, 6],
            feather: 2,
            inverted: false,
            animation: None,
        };
        assert_eq!(mask.weight(mask.rect, [2, 2]), 0.25);
        assert_eq!(mask.weight(mask.rect, [4, 4]), 1.0);
        assert_eq!(mask.weight(mask.rect, [1, 2]), 0.0);
        assert_eq!(mask.weight([2, 2, 0, 6], [2, 2]), 0.0);
        mask.inverted = true;
        assert_eq!(mask.weight(mask.rect, [2, 2]), 0.75);
        assert_eq!(mask.weight([2, 2, 0, 6], [2, 2]), 1.0);
    }
}
