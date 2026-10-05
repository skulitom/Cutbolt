//! Original ordered scene effects with explicit linear-light grading equations.
use crate::{
    Result,
    animation::{Curve, Sampler as CurveSampler},
    composite::AlphaMode,
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Piecewise-linear tone curve on normalized linear light; output knots need not be monotonic.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ToneCurve {
    /// 2..32 `[input, output]` knots where 0..65535 means 0..1; inputs strictly increase from 0 to 65535.
    pub points: Vec<[u16; 2]>,
}
impl ToneCurve {
    fn validate(&self) -> Result<()> {
        if !(2..=32).contains(&self.points.len())
            || self.points[0][0] != 0
            || self.points.last().unwrap()[0] != u16::MAX
            || self.points.windows(2).any(|p| p[0][0] >= p[1][0])
        {
            return Err(invalid(
                "Tone curves require 2-32 ordered distinct input knots spanning 0..65535",
            ));
        }
        Ok(())
    }
    fn identity(&self) -> bool {
        self.points.iter().all(|p| p[0] == p[1])
    }
    fn evaluate(&self, value: f64) -> f64 {
        let x = value.clamp(0.0, 1.0) * 65535.0;
        let i = self
            .points
            .partition_point(|p| (p[0] as f64) < x)
            .clamp(1, self.points.len() - 1);
        let [x0, y0] = self.points[i - 1].map(f64::from);
        let [x1, y1] = self.points[i].map(f64::from);
        (y0 + (y1 - y0) * (x - x0) / (x1 - x0)) / 65535.0
    }
}

/// Primary grade in linear sRGB: exposure and white balance, contrast around 0.18, clamp to 0..1, then master and channel curves.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Grade {
    /// Exposure in thousandths of a stop, -8000..8000; 1000 doubles linear light, 0 is neutral.
    pub exposure_milli: i32,
    /// Contrast slope around linear 0.18 in thousandths, 0..4000; 1000 is neutral.
    pub contrast_milli: u16,
    /// Linear `[r, g, b]` gains in thousandths, each 100..4000; `[1000, 1000, 1000]` is neutral.
    pub white_balance_milli: [u16; 3],
    /// Tone curve applied to all channels after contrast; omit for identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master_curve: Option<ToneCurve>,
    /// Red tone curve applied after the master curve; omit for identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red_curve: Option<ToneCurve>,
    /// Green tone curve applied after the master curve; omit for identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub green_curve: Option<ToneCurve>,
    /// Blue tone curve applied after the master curve; omit for identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blue_curve: Option<ToneCurve>,
    /// Optional curves overriding the numeric controls on the layer-local clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<GradeAnimation>,
}
/// Keyframe curves overriding grade controls, using the same ranges; declare at least one.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GradeAnimation {
    /// Curve for `exposure_milli`, -8000..8000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_milli: Option<Curve>,
    /// Curve for `contrast_milli`, 0..4000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contrast_milli: Option<Curve>,
    /// Curve for the red white-balance gain, 100..4000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub red_balance_milli: Option<Curve>,
    /// Curve for the green white-balance gain, 100..4000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub green_balance_milli: Option<Curve>,
    /// Curve for the blue white-balance gain, 100..4000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blue_balance_milli: Option<Curve>,
}
/// One ordered layer effect, tagged by `kind`. Effects run in list order on source pixels before compositing.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Effect {
    /// Primary grade of every layer pixel; alpha is preserved.
    Grade(Grade),
    /// Grade limited by a color qualifier and/or correction mask, blended by mix; alpha is preserved.
    SelectiveGrade(SelectiveGrade),
    /// Chroma key that lowers alpha near a screen color.
    ChromaKey(crate::keying::ChromaKey),
}
impl Effect {
    /// Whether the recipe alone fixes every sampled value: no curves and no animated mask.
    pub(crate) fn is_constant(&self) -> bool {
        let still = |mask: &Option<Box<crate::selection::Mask>>| {
            mask.as_ref().is_none_or(|m| m.animation.is_none())
        };
        match self {
            Self::Grade(g) => g.animation.is_none(),
            Self::SelectiveGrade(s) => {
                s.grade.animation.is_none() && s.mix_curve.is_none() && still(&s.mask)
            }
            Self::ChromaKey(k) => k.strength_curve.is_none() && still(&k.mask),
        }
    }
}
/// Grade applied only where a color qualifier and/or correction mask select pixels.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectiveGrade {
    /// Grade controls, as in a `grade` effect but without `kind`.
    pub grade: Grade,
    /// Correction strength in thousandths, 0..1000; 0 bypasses, 1000 applies the full selected grade.
    pub mix_milli: u16,
    /// Curve overriding `mix_milli` on the layer-local clock, 0..1000.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mix_curve: Option<Curve>,
    /// HSL color selection; at least one of `qualifier` or `mask` is required.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qualifier: Option<crate::selection::Qualifier>,
    /// Source-canvas correction mask; it weights the grade and never changes alpha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<crate::selection::Mask>>,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Sample {
    ChromaKey {
        strength_milli: i32,
        mask_rect: Option<[i32; 4]>,
    },
    Grade {
        exposure_milli: i32,
        contrast_milli: i32,
        white_balance_milli: [i32; 3],
    },
    SelectiveGrade {
        exposure_milli: i32,
        contrast_milli: i32,
        white_balance_milli: [i32; 3],
        mix_milli: i32,
        mask_rect: Option<[i32; 4]>,
    },
}
pub(crate) enum Sampler {
    Grade(Box<GradeSampler>),
    Key(Box<crate::keying::Sampler>),
}
pub(crate) struct GradeSampler {
    values: [i32; 5],
    curves: [Option<CurveSampler>; 5],
    selection: Option<SelectionSampler>,
}
struct SelectionSampler {
    mix_milli: u16,
    mix_curve: Option<CurveSampler>,
    mask: Option<crate::composite::MaskSampler>,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_EFFECT", message)
}
pub(crate) fn prepare(effects: &[Effect], duration: Time) -> Result<Vec<Sampler>> {
    if effects.len() > 8 {
        return Err(invalid("Use at most eight ordered effects per layer"));
    }
    effects.iter().map(|effect| {
        if let Effect::ChromaKey(key) = effect {
            return Ok(Sampler::Key(Box::new(key.prepare(duration)?)));
        }
        let (g, selection) = match effect {
            Effect::Grade(g) => (g, None),
            Effect::SelectiveGrade(s) => {
                if s.mix_milli > 1000 || (s.qualifier.is_none() && s.mask.is_none()) {
                    return Err(invalid("Selective grades require a qualifier or mask and mix 0..1000"));
                }
                if let Some(q) = &s.qualifier { q.validate()?; }
                ( &s.grade, Some(SelectionSampler { mix_milli:s.mix_milli,
                    mix_curve:s.mix_curve.as_ref().map(|c|c.prepare(duration,0,1000)).transpose()?,
                    mask:s.mask.as_ref().map(|m|m.prepare(duration)).transpose()? }))
            }
            Effect::ChromaKey(_) => unreachable!("key prepared above"),
        };
        let values = [g.exposure_milli, g.contrast_milli as i32, g.white_balance_milli[0] as i32, g.white_balance_milli[1] as i32, g.white_balance_milli[2] as i32];
        let bounds = [(-8000,8000),(0,4000),(100,4000),(100,4000),(100,4000)];
        for (value,(minimum,maximum)) in values.iter().zip(bounds) {
            if !(minimum..=maximum).contains(value) {
                return Err(invalid("Grade exposure must be -8000..8000, contrast 0..4000 and white-balance gains 100..4000"));
            }
        }
        for curve in [&g.master_curve,&g.red_curve,&g.green_curve,&g.blue_curve].into_iter().flatten() {
            curve.validate()?;
        }
        let mut curves = [None,None,None,None,None];
        if let Some(a) = &g.animation {
            let inputs = [&a.exposure_milli,&a.contrast_milli,&a.red_balance_milli,&a.green_balance_milli,&a.blue_balance_milli];
            if inputs.iter().all(|c| c.is_none()) {
                return Err(invalid("Grade animation requires at least one property"));
            }
            for (i,input) in inputs.into_iter().enumerate() {
                curves[i] = input.as_ref().map(|c| c.prepare(duration,bounds[i].0,bounds[i].1)).transpose()?;
            }
        }
        Ok(Sampler::Grade(Box::new(GradeSampler { values, curves, selection })))
    }).collect()
}
impl Sampler {
    pub(crate) fn sample(&self, time: Time) -> Result<Sample> {
        match self {
            Self::Grade(grade) => grade.sample(time),
            Self::Key(key) => key.sample(time),
        }
    }
}
impl GradeSampler {
    pub(crate) fn sample(&self, time: Time) -> Result<Sample> {
        let mut v = self.values;
        for (value, curve) in v.iter_mut().zip(&self.curves) {
            if let Some(curve) = curve {
                *value = curve.sample(time)?;
            }
        }
        if let Some(s) = &self.selection {
            return Ok(Sample::SelectiveGrade {
                exposure_milli: v[0],
                contrast_milli: v[1],
                white_balance_milli: [v[2], v[3], v[4]],
                mix_milli: s
                    .mix_curve
                    .as_ref()
                    .map(|c| c.sample(time))
                    .transpose()?
                    .unwrap_or(s.mix_milli as i32),
                mask_rect: s.mask.as_ref().map(|m| m.sample(time)).transpose()?,
            });
        }
        Ok(Sample::Grade {
            exposure_milli: v[0],
            contrast_milli: v[1],
            white_balance_milli: [v[2], v[3], v[4]],
        })
    }
}

enum Stage<'a> {
    Grade(GradeStage<'a>),
    Key(crate::keying::Stage<'a>),
}
impl Stage<'_> {
    fn identity(&self) -> bool {
        match self {
            Self::Grade(g) => g.identity,
            Self::Key(k) => k.strength == 0.0,
        }
    }
    fn per_pixel(&self) -> bool {
        match self {
            Self::Grade(g) => g.selection.is_some(),
            Self::Key(_) => true,
        }
    }
}
struct GradeStage<'a> {
    grade: &'a Grade,
    multiplier: [f64; 3],
    contrast: f64,
    identity: bool,
    selection: Option<SelectionStage<'a>>,
}
struct SelectionStage<'a> {
    effect: &'a SelectiveGrade,
    mix: f64,
    mask_rect: Option<[i32; 4]>,
}
impl SelectionStage<'_> {
    /// `bytes`, when known, are `color.map(encoded_byte)`.
    fn weight(&self, color: [f64; 3], bytes: Option<[u8; 3]>, position: [i64; 2]) -> f64 {
        self.mix
            * self
                .effect
                .qualifier
                .as_ref()
                .map(|q| q.weight(bytes.unwrap_or_else(|| color.map(encoded_byte))))
                .unwrap_or(1.0)
            * self
                .effect
                .mask
                .as_ref()
                .map(|m| m.weight(self.mask_rect.expect("prepared correction mask"), position))
                .unwrap_or(1.0)
    }
}
impl GradeStage<'_> {
    fn apply(&self, value: f64, channel: usize) -> f64 {
        let mut value =
            ((value * self.multiplier[channel] - 0.18) * self.contrast + 0.18).clamp(0.0, 1.0);
        if let Some(curve) = &self.grade.master_curve {
            value = curve.evaluate(value);
        }
        if let Some(curve) = [
            &self.grade.red_curve,
            &self.grade.green_curve,
            &self.grade.blue_curve,
        ][channel]
        {
            value = curve.evaluate(value);
        }
        value
    }
}
// Standard sRGB transfer equations on the nonnegative, bounded display domain.
fn linear(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}
fn encoded(value: f64) -> f64 {
    let value = value.clamp(0.0, 1.0);
    if value <= 0.0031308 {
        12.92 * value
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}
fn encoded_byte(value: f64) -> u8 {
    (encoded(value) * 255.0).round().clamp(0.0, 255.0) as u8
}
/// `linear` of each 8-bit straight value, and `encoded` and `encoded_byte` of that, computed once
/// by the same functions, so a lookup returns the very value a call would.
struct Transfer {
    linear: [f64; 256],
    encoded: [f64; 256],
    byte: [u8; 256],
}
fn transfer() -> &'static Transfer {
    static TABLES: std::sync::OnceLock<Transfer> = std::sync::OnceLock::new();
    TABLES.get_or_init(|| {
        let linear: [f64; 256] = std::array::from_fn(|v| linear(v as f64 / 255.0));
        Transfer {
            encoded: linear.map(encoded),
            byte: linear.map(encoded_byte),
            linear,
        }
    })
}
pub(crate) struct Processor<'a> {
    stages: Vec<Stage<'a>>,
    tables: [[u8; 256]; 3],
    per_pixel: bool,
}
impl<'a> Processor<'a> {
    pub(crate) fn new(effects: &'a [Effect], samples: &[Sample]) -> Option<Self> {
        let stages: Vec<_> = effects
            .iter()
            .zip(samples)
            .map(|(effect, sample)| {
                if let (
                    Effect::ChromaKey(effect),
                    Sample::ChromaKey {
                        strength_milli,
                        mask_rect,
                    },
                ) = (effect, sample)
                {
                    return Stage::Key(crate::keying::Stage {
                        effect,
                        strength: *strength_milli as f64 / 1000.0,
                        mask_rect: *mask_rect,
                    });
                }
                let (grade, exposure_milli, contrast_milli, white_balance_milli, selection) =
                    match (effect, sample) {
                        (
                            Effect::Grade(grade),
                            Sample::Grade {
                                exposure_milli,
                                contrast_milli,
                                white_balance_milli,
                            },
                        ) => (
                            grade,
                            exposure_milli,
                            contrast_milli,
                            white_balance_milli,
                            None,
                        ),
                        (
                            Effect::SelectiveGrade(s),
                            Sample::SelectiveGrade {
                                exposure_milli,
                                contrast_milli,
                                white_balance_milli,
                                mix_milli,
                                mask_rect,
                            },
                        ) => (
                            &s.grade,
                            exposure_milli,
                            contrast_milli,
                            white_balance_milli,
                            Some(SelectionStage {
                                effect: s,
                                mix: *mix_milli as f64 / 1000.0,
                                mask_rect: *mask_rect,
                            }),
                        ),
                        _ => unreachable!("effect and its prepared sample must have the same kind"),
                    };
                let exposure = 2.0f64.powf(*exposure_milli as f64 / 1000.0);
                let identity = (selection.as_ref().is_some_and(|s| s.mix == 0.0))
                    || (*exposure_milli == 0
                        && *contrast_milli == 1000
                        && *white_balance_milli == [1000; 3]
                        && [
                            &grade.master_curve,
                            &grade.red_curve,
                            &grade.green_curve,
                            &grade.blue_curve,
                        ]
                        .into_iter()
                        .flatten()
                        .all(ToneCurve::identity));
                Stage::Grade(GradeStage {
                    grade,
                    multiplier: white_balance_milli.map(|v| exposure * v as f64 / 1000.0),
                    contrast: *contrast_milli as f64 / 1000.0,
                    identity,
                    selection,
                })
            })
            .collect();
        // Preserve the original integer compositor exactly when the chain is neutral.
        if stages.iter().all(Stage::identity) {
            return None;
        }
        let mut processor = Self {
            per_pixel: stages.iter().any(Stage::per_pixel),
            stages,
            tables: [[0; 256]; 3],
        };
        if !processor.per_pixel {
            for channel in 0..3 {
                for value in 0..256 {
                    processor.tables[channel][value] =
                        processor.channel(value as f64 / 255.0, channel);
                }
            }
        }
        Some(processor)
    }
    fn channel(&self, value: f64, channel: usize) -> u8 {
        let value = self
            .stages
            .iter()
            .fold(linear(value), |value, stage| match stage {
                Stage::Grade(g) => g.apply(value, channel),
                Stage::Key(_) => unreachable!("keys require per-pixel processing"),
            });
        encoded_byte(value)
    }
    /// The per-channel tables of a chain without per-pixel stages: `pixel` maps a straight
    /// pixel with nonzero alpha channel by channel through them.
    pub(crate) fn tables(&self) -> Option<&[[u8; 256]; 3]> {
        (!self.per_pixel).then_some(&self.tables)
    }
    pub(crate) fn pixel(
        &self,
        rgba: &[u8],
        alpha: AlphaMode,
        position: [i64; 2],
    ) -> Option<[u8; 4]> {
        if rgba[3] == 0 {
            return None;
        }
        if self.per_pixel {
            // While no stage has changed a straight pixel, its linear, encoded and quantized
            // values are those of its stored bytes, which the transfer tables hold.
            let tables = transfer();
            let mut source = match alpha {
                AlphaMode::Straight => Some([rgba[0], rgba[1], rgba[2]].map(usize::from)),
                AlphaMode::Premultiplied => None,
            };
            let mut color = match source {
                Some(rgb) => rgb.map(|v| tables.linear[v]),
                None => {
                    let denominator = rgba[3] as f64;
                    [
                        linear(rgba[0] as f64 / denominator),
                        linear(rgba[1] as f64 / denominator),
                        linear(rgba[2] as f64 / denominator),
                    ]
                }
            };
            let mut changed = false;
            let mut coverage = rgba[3] as f64;
            for stage in &self.stages {
                if stage.identity() {
                    continue;
                }
                let stage = match stage {
                    Stage::Grade(g) => g,
                    Stage::Key(k) => {
                        let encoded_color = match source {
                            Some(rgb) => rgb.map(|v| tables.encoded[v]),
                            None => color.map(encoded),
                        };
                        if let Some((candidate, retention, weight)) =
                            k.apply(encoded_color, position)
                        {
                            coverage *= retention;
                            changed = true;
                            source = None;
                            // A fully keyed pixel is returned transparent whatever its color.
                            if coverage != 0.0 {
                                for (value, candidate) in color.iter_mut().zip(candidate) {
                                    *value = *value * (1.0 - weight) + linear(candidate) * weight;
                                }
                            }
                        }
                        if coverage == 0.0 {
                            return Some([0; 4]);
                        }
                        continue;
                    }
                };
                let weight = stage
                    .selection
                    .as_ref()
                    .map(|s| {
                        let bytes = source.map(|rgb| rgb.map(|v| tables.byte[v]));
                        s.weight(color, bytes, position)
                    })
                    .unwrap_or(1.0);
                if weight <= 0.0 {
                    continue;
                }
                for (c, value) in color.iter_mut().enumerate() {
                    *value = *value * (1.0 - weight) + stage.apply(*value, c) * weight;
                }
                changed = true;
                source = None;
            }
            if !changed {
                return None;
            }
            let [r, g, b] = color.map(encoded_byte);
            return Some([r, g, b, coverage.round().clamp(0.0, 255.0) as u8]);
        }
        let mut result = [0, 0, 0, rgba[3]];
        for c in 0..3 {
            result[c] = match alpha {
                AlphaMode::Straight => self.tables[c][rgba[c] as usize],
                AlphaMode::Premultiplied => self.channel(rgba[c] as f64 / rgba[3] as f64, c),
            };
        }
        Some(result)
    }
}
pub fn capabilities() -> Value {
    json!({"scope":"scene_layer","types":["grade","selective_grade","chroma_key"],"maximum_per_layer":8,"working_space":"linear_srgb_f64","output":"straight_srgb_u8_before_compositing","order":"exposure_and_white_balance_then_contrast_then_master_curve_then_channel_curve","contrast_pivot":0.18,"clipping":"after_contrast_in_each_grade","exposure_milli":[-8000,8000],"contrast_milli":[0,4000],"white_balance_milli":[100,4000],"tone_curve":{"domain":[0,65535],"maximum_points":32,"interpolation":"linear","animated_points":false},"animated_properties":["exposure_milli","contrast_milli","red_balance_milli","green_balance_milli","blue_balance_milli"],"clock":"layer_local","alpha":"unpremultiply_before_processing_grades_preserve_keys_reduce","neutral_chain":"exact_integer_bypass","keying":crate::keying::capabilities(),"selection":{"qualifier":"quantized_srgb8_hsl_integer","hue_unit":"millidegrees","hue_range":[0,359999],"saturation_lightness_unit":"thousandths","achromatic_hue":"excluded_before_inversion","combination":"product_then_qualifier_inversion_then_mask_then_mix","mask":"source_canvas_rectangle_inward_linear_feather","mask_maximum_feather":4096,"animated":["grade_controls","mix_milli","mask_rect"],"unselected_pixels":"preserve_original_alpha_encoding"}})
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Processor<'_> {
        /// The former `pixel`, as the reference for the table lookups.
        fn reference_pixel(
            &self,
            rgba: &[u8],
            alpha: AlphaMode,
            position: [i64; 2],
        ) -> Option<[u8; 4]> {
            if rgba[3] == 0 {
                return None;
            }
            if self.per_pixel {
                let denominator = match alpha {
                    AlphaMode::Straight => 255.0,
                    AlphaMode::Premultiplied => rgba[3] as f64,
                };
                let mut color = [
                    linear(rgba[0] as f64 / denominator),
                    linear(rgba[1] as f64 / denominator),
                    linear(rgba[2] as f64 / denominator),
                ];
                let mut changed = false;
                let mut coverage = rgba[3] as f64;
                for stage in &self.stages {
                    if stage.identity() {
                        continue;
                    }
                    let stage = match stage {
                        Stage::Grade(g) => g,
                        Stage::Key(k) => {
                            if let Some((candidate, retention, weight)) =
                                k.apply(color.map(encoded), position)
                            {
                                for (value, candidate) in color.iter_mut().zip(candidate) {
                                    *value = *value * (1.0 - weight) + linear(candidate) * weight;
                                }
                                coverage *= retention;
                                changed = true;
                            }
                            if coverage == 0.0 {
                                return Some([0; 4]);
                            }
                            continue;
                        }
                    };
                    let weight = stage
                        .selection
                        .as_ref()
                        .map(|s| s.weight(color, None, position))
                        .unwrap_or(1.0);
                    if weight <= 0.0 {
                        continue;
                    }
                    for (c, value) in color.iter_mut().enumerate() {
                        *value = *value * (1.0 - weight) + stage.apply(*value, c) * weight;
                    }
                    changed = true;
                }
                if !changed {
                    return None;
                }
                let [r, g, b] = color.map(encoded_byte);
                return Some([r, g, b, coverage.round().clamp(0.0, 255.0) as u8]);
            }
            let mut result = [0, 0, 0, rgba[3]];
            for c in 0..3 {
                result[c] = match alpha {
                    AlphaMode::Straight => self.tables[c][rgba[c] as usize],
                    AlphaMode::Premultiplied => self.channel(rgba[c] as f64 / rgba[3] as f64, c),
                };
            }
            Some(result)
        }
    }

    #[test]
    fn transfer_tables_hold_the_function_values() {
        let tables = transfer();
        for v in 0..256 {
            let value = linear(v as f64 / 255.0);
            assert_eq!(tables.linear[v].to_bits(), value.to_bits());
            assert_eq!(tables.encoded[v].to_bits(), encoded(value).to_bits());
            assert_eq!(tables.byte[v], encoded_byte(value));
        }
    }

    #[test]
    fn per_pixel_chains_match_the_former_processing() {
        let effect = |value: Value| -> Effect { serde_json::from_value(value).unwrap() };
        let key = |rgb: [u8; 3], strength: i32, unmix: u16, mask: Value| {
            let mut key = json!({"kind":"chroma_key","key_rgb":rgb,"inner_milli":80,"outer_milli":260,
                "strength_milli":1000,"unmix_milli":unmix,"spill":{"channel":"green","strength_milli":600}});
            if !mask.is_null() {
                key["mask"] = mask;
            }
            (
                effect(key),
                Sample::ChromaKey {
                    strength_milli: strength,
                    mask_rect: Some([3, 2, 9, 7]),
                },
            )
        };
        let grade = |exposure: i32| {
            (
                effect(
                    json!({"kind":"grade","exposure_milli":exposure,"contrast_milli":1150,"white_balance_milli":[1000,1050,900],
                    "master_curve":{"points":[[0,0],[20000,16000],[65535,65535]]}}),
                ),
                Sample::Grade {
                    exposure_milli: exposure,
                    contrast_milli: 1150,
                    white_balance_milli: [1000, 1050, 900],
                },
            )
        };
        let selective = |mix: i32, inverted: bool, mask: Value| {
            let mut value = json!({"kind":"selective_grade","grade":{"exposure_milli":600,"contrast_milli":900,
                "white_balance_milli":[1100,1000,950]},"mix_milli":mix,"qualifier":{"hue":{"center":120000,"inner":30000,"outer":80000},
                "saturation":{"low":150,"high":1000,"feather":120},"inverted":inverted}});
            if !mask.is_null() {
                value["mask"] = mask;
            }
            (
                effect(value),
                Sample::SelectiveGrade {
                    exposure_milli: 600,
                    contrast_milli: 900,
                    white_balance_milli: [1100, 1000, 950],
                    mix_milli: mix,
                    mask_rect: Some([3, 2, 9, 7]),
                },
            )
        };
        let mask = json!({"rect":[3,2,9,7],"feather":3});
        let selective_mask = json!({"rect":[3,2,9,7],"feather":4,"inverted":true});
        let chains = vec![
            vec![key([40, 200, 90], 1000, 0, Value::Null)],
            vec![key([40, 200, 90], 700, 500, mask.clone())],
            vec![selective(800, false, Value::Null)],
            vec![selective(350, true, selective_mask.clone())],
            vec![key([20, 60, 230], 1000, 300, Value::Null), grade(400)],
            vec![grade(-300), selective(1000, false, Value::Null)],
            vec![
                key([40, 200, 90], 0, 0, Value::Null),
                selective(600, false, Value::Null),
            ],
            vec![
                selective(0, false, Value::Null),
                key([200, 40, 60], 900, 0, Value::Null),
            ],
            vec![
                key([40, 200, 90], 600, 0, Value::Null),
                key([30, 220, 70], 1000, 800, mask),
            ],
        ];
        for chain in chains {
            let (effects, samples): (Vec<_>, Vec<_>) = chain.into_iter().unzip();
            let processor = Processor::new(&effects, &samples).expect("a per-pixel chain");
            assert!(processor.tables().is_none());
            for r in (0..=255u8).step_by(7) {
                for g in (0..=255u8).step_by(5) {
                    for b in (0..=255u8).step_by(9) {
                        for a in [1u8, 77, 200, 255] {
                            let position = [(r / 7 % 12) as i64, (g / 5 % 10) as i64];
                            let straight = [r, g, b, a];
                            let premultiplied = [r.min(a), g.min(a), b.min(a), a];
                            for (rgba, mode) in [
                                (straight, AlphaMode::Straight),
                                (premultiplied, AlphaMode::Premultiplied),
                            ] {
                                assert_eq!(
                                    processor.pixel(&rgba, mode, position),
                                    processor.reference_pixel(&rgba, mode, position),
                                    "{effects:?} {rgba:?} {mode:?} {position:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
