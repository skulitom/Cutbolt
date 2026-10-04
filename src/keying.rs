//! Original bounded chroma-distance keying, screen unmixing and spill suppression.
use crate::{
    Result,
    animation::{Curve, Sampler as CurveSampler},
    composite::MaskSampler,
    effects::{Effect, Sample},
    error,
    selection::Mask,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Color channel whose spill is suppressed: `red`, `green` or `blue`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    Red,
    Green,
    Blue,
}
impl Channel {
    fn index(self) -> usize {
        match self {
            Self::Red => 0,
            Self::Green => 1,
            Self::Blue => 2,
        }
    }
}
/// Spill suppression: lowers the named channel's excess above the larger of the other two channels.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spill {
    /// Channel to suppress.
    pub channel: Channel,
    /// Share of the excess removed in thousandths, 0..1000.
    pub strength_milli: u16,
}
/// Chroma key lowering alpha by opponent-channel distance from a screen color, with optional unmix and spill suppression.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChromaKey {
    /// Screen color as encoded sRGB `[r, g, b]` bytes; must not be gray (all three equal).
    pub key_rgb: [u8; 3],
    /// Distance in thousandths at or below which pixels are fully removed; at most `outer_milli`.
    pub inner_milli: u16,
    /// Distance in thousandths at or above which pixels are fully kept, 0..1000; equal to inner for a hard key.
    pub outer_milli: u16,
    /// Key strength in thousandths, 0..1000; 0 disables the key, including spill and unmix.
    pub strength_milli: u16,
    /// Screen-color subtraction from soft edges in thousandths, 0..1000; default 0.
    #[serde(default)]
    pub unmix_milli: u16,
    /// Optional spill suppression, applied throughout the mask, including fully kept pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill: Option<Spill>,
    /// Curve overriding `strength_milli` on the layer-local clock, 0..1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strength_curve: Option<Curve>,
    /// Optional source-canvas mask limiting where the key applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<Mask>>,
}
pub(crate) struct Sampler {
    strength: u16,
    curve: Option<CurveSampler>,
    mask: Option<MaskSampler>,
}
impl ChromaKey {
    pub(crate) fn prepare(&self, duration: Time) -> Result<Sampler> {
        if self.key_rgb.iter().all(|v| *v == self.key_rgb[0])
            || self.inner_milli > self.outer_milli
            || self.outer_milli > 1000
            || self.strength_milli > 1000
            || self.unmix_milli > 1000
            || self.spill.as_ref().is_some_and(|s| s.strength_milli > 1000)
        {
            return Err(error(
                "INVALID_EFFECT",
                "Chroma key requires a non-gray key color, 0 <= inner <= outer <=1000, and strength/unmix/spill values 0..1000",
            ));
        }
        Ok(Sampler {
            strength: self.strength_milli,
            curve: self
                .strength_curve
                .as_ref()
                .map(|c| c.prepare(duration, 0, 1000))
                .transpose()?,
            mask: self
                .mask
                .as_ref()
                .map(|m| m.prepare(duration))
                .transpose()?,
        })
    }
    fn retention(&self, rgb: [u8; 3]) -> f64 {
        // Maximum difference between opponent-channel contrasts; gray offsets cancel.
        let delta: [i32; 3] = std::array::from_fn(|c| rgb[c] as i32 - self.key_rgb[c] as i32);
        let distance = (delta.iter().max().unwrap() - delta.iter().min().unwrap()) * 1000;
        let inner = self.inner_milli as i32 * 510;
        let outer = self.outer_milli as i32 * 510;
        if distance <= inner {
            0.0
        } else if distance >= outer {
            1.0
        } else {
            (distance - inner) as f64 / (outer - inner) as f64
        }
    }
}
impl Sampler {
    pub(crate) fn sample(&self, time: Time) -> Result<Sample> {
        Ok(Sample::ChromaKey {
            strength_milli: self
                .curve
                .as_ref()
                .map(|c| c.sample(time))
                .transpose()?
                .unwrap_or(self.strength as i32),
            mask_rect: self.mask.as_ref().map(|m| m.sample(time)).transpose()?,
        })
    }
}
pub(crate) struct Stage<'a> {
    pub effect: &'a ChromaKey,
    pub strength: f64,
    pub mask_rect: Option<[i32; 4]>,
}
impl Stage<'_> {
    // Encoded, straight RGB input/output. The effect processor keeps other stages in linear light.
    pub(crate) fn apply(
        &self,
        color: [f64; 3],
        position: [i64; 2],
    ) -> Option<([f64; 3], f64, f64)> {
        let weight = self.strength
            * self
                .effect
                .mask
                .as_ref()
                .map(|m| m.weight(self.mask_rect.expect("prepared key mask"), position))
                .unwrap_or(1.0);
        if weight <= 0.0 {
            return None;
        }
        let matte = self
            .effect
            .retention(color.map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8));
        let mut candidate = color;
        if matte < 1.0 && self.effect.unmix_milli > 0 {
            let mix = self.effect.unmix_milli as f64 / 1000.0;
            for (c, value) in candidate.iter_mut().enumerate() {
                let unmixed = if matte == 0.0 {
                    0.0
                } else {
                    ((color[c] - (1.0 - matte) * self.effect.key_rgb[c] as f64 / 255.0) / matte)
                        .clamp(0.0, 1.0)
                };
                *value = color[c] * (1.0 - mix) + unmixed * mix;
            }
        }
        let mut spill_changed = false;
        if let Some(spill) = &self.effect.spill {
            let c = spill.channel.index();
            let excess =
                (candidate[c] - candidate[(c + 1) % 3].max(candidate[(c + 2) % 3])).max(0.0);
            spill_changed = excess > 0.0 && spill.strength_milli > 0;
            candidate[c] -= excess * spill.strength_milli as f64 / 1000.0;
        }
        if matte == 1.0 && !spill_changed {
            return None;
        }
        Some((candidate, 1.0 - weight * (1.0 - matte), weight))
    }
}

/// Chroma key preset: `green_soft`/`blue_soft` use thresholds 60/180, `green_hard`/`blue_hard` 100/100, keying and de-spilling pure green or blue.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    GreenSoft,
    BlueSoft,
    GreenHard,
    BlueHard,
}
pub fn preset(name: Preset, strength_milli: u16, spill_milli: u16) -> Result<Value> {
    let blue = matches!(name, Preset::BlueSoft | Preset::BlueHard);
    let hard = matches!(name, Preset::GreenHard | Preset::BlueHard);
    let effect = ChromaKey {
        key_rgb: if blue { [0, 0, 255] } else { [0, 255, 0] },
        inner_milli: if hard { 100 } else { 60 },
        outer_milli: if hard { 100 } else { 180 },
        strength_milli,
        unmix_milli: 0,
        spill: Some(Spill {
            channel: if blue { Channel::Blue } else { Channel::Green },
            strength_milli: spill_milli,
        }),
        strength_curve: None,
        mask: None,
    };
    effect.prepare(Time::new(1, 1)?)?;
    Ok(json!({"schema_version":1,"preset":name,"effects":[Effect::ChromaKey(effect)]}))
}
pub fn capabilities() -> Value {
    json!({"distance":"maximum_opponent_difference_srgb8_divided_by_510","threshold_unit":"thousandths","threshold_range":[0,1000],"hard_boundary":"inclusive_transparent","alpha":"multiply_existing_alpha_without_intermediate_quantization","unmix":"optional_encoded_screen_subtraction_with_clipping","spill":"reduce_named_channel_excess_above_maximum_other_channel","mask":"source_canvas_inward_feathered_rectangle","animated":["strength_milli","mask_rect"],"presets":["green_soft","blue_soft","green_hard","blue_hard"],"preset_defaults":"explicit_strength_and_spill_arguments_no_unmix","spatial_filtering":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chroma_geometry_and_hard_boundary() {
        let mut key = ChromaKey {
            key_rgb: [0, 255, 0],
            inner_milli: 0,
            outer_milli: 500,
            strength_milli: 1000,
            unmix_milli: 0,
            spill: None,
            strength_curve: None,
            mask: None,
        };
        assert_eq!(key.retention([0, 255, 0]), 0.0);
        assert_eq!(key.retention([128; 3]), 1.0);
        assert_eq!(key.retention([50, 203, 50]), 0.4);
        key.inner_milli = 100;
        key.outer_milli = 100;
        assert_eq!(key.retention([0, 204, 0]), 0.0);
        assert_eq!(key.retention([0, 203, 0]), 1.0);
        key.key_rgb = [20, 140, 60];
        assert_eq!(key.retention([40, 160, 80]), 0.0);
    }
}
