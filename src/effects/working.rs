//! Validated effects for linear UNORM16 working pixels. This library path is the precise
//! renderer's effect component; it does not change scene or project rendering defaults.
use super::{Effect, GradeStage, Resources, Sampler, SelectionStage, Stage, ToneCurve};
use crate::{Result, color::working::Pixel, error, scene::Identity, time::Time};
use std::path::Path;

/// One validated effect stack and its identity-verified LUT tables, for local library use.
/// Curves are sampled per frame, without duration-sized tables. Timeline renderers can use
/// the same frame builder with their shared resource pool rather than loading duplicate LUTs.
pub struct Effects {
    effects: Vec<Effect>,
    samplers: Vec<Sampler>,
    resources: Resources,
}

impl Effects {
    /// Prepare at most eight effects and load identity-bound LUTs under the explicit input root.
    pub fn new(effects: Vec<Effect>, duration: Time, input_root: &Path) -> Result<Self> {
        duration.validate()?;
        if duration.num == 0 {
            return Err(error("INVALID_EFFECT", "Effect duration must be positive"));
        }
        let samplers = super::prepare(&effects, duration)?;
        let mut resources = Resources::default();
        resources.add(&effects, input_root)?;
        Ok(Self {
            effects,
            samplers,
            resources,
        })
    }

    /// Sample numeric effect controls continuously at the exact source-clock time. Masks keep
    /// their declared integer source-canvas coordinates; no RGB or coverage is rounded to bytes.
    pub fn frame(&self, time: Time) -> Result<Frame<'_>> {
        frame(&self.effects, &self.samplers, &self.resources, time)
    }

    /// External files read by this stack. Publication remains the caller's responsibility;
    /// constructing this arithmetic component does not publish or mutate any media.
    pub fn identities(&self) -> impl Iterator<Item = &Identity> {
        self.resources.identities()
    }
}

/// Sampled effect parameters and borrowed resources, reusable for all pixels of one frame.
pub struct Frame<'a> {
    stages: Vec<Stage<'a>>,
}

pub(crate) fn frame<'a>(
    effects: &'a [Effect],
    samplers: &[Sampler],
    resources: &'a Resources,
    time: Time,
) -> Result<Frame<'a>> {
    time.validate()?;
    let mut stages = Vec::with_capacity(effects.len());
    for (effect, sampler) in effects.iter().zip(samplers) {
        stages.push(match (effect, sampler) {
            (Effect::Lut(spec), Sampler::Lut) => Stage::Lut(resources.get(spec)),
            (Effect::ChromaKey(effect), Sampler::Key(sampler)) => {
                let (strength, mask_rect) = sampler.sample_precise(time)?;
                Stage::Key(crate::keying::Stage {
                    effect,
                    strength,
                    mask_rect,
                })
            }
            (Effect::Grade(_) | Effect::SelectiveGrade(_), Sampler::Grade(sampler)) => {
                let mut values = sampler.values.map(f64::from);
                for (value, curve) in values.iter_mut().zip(&sampler.curves) {
                    if let Some(curve) = curve {
                        *value = curve.sample_f64(time)?;
                    }
                }
                let (grade, selection) = match effect {
                    Effect::Grade(g) => (g, None),
                    Effect::SelectiveGrade(s) => {
                        let sample = sampler
                            .selection
                            .as_ref()
                            .expect("prepared selective grade");
                        (
                            &s.grade,
                            Some(SelectionStage {
                                effect: s,
                                mix: sample
                                    .mix_curve
                                    .as_ref()
                                    .map(|c| c.sample_f64(time))
                                    .transpose()?
                                    .unwrap_or(f64::from(sample.mix_milli))
                                    / 1000.0,
                                mask_rect: sample
                                    .mask
                                    .as_ref()
                                    .map(|m| m.sample(time))
                                    .transpose()?,
                            }),
                        )
                    }
                    _ => unreachable!("grade sampler"),
                };
                let turn = values[5].rem_euclid(360000.0);
                let hue_saturation = (turn != 0.0 || values[6] != 1000.0)
                    .then(|| (turn / 60000.0, values[6] / 1000.0));
                let exposure = 2.0f64.powf(values[0] / 1000.0);
                let identity = selection.as_ref().is_some_and(|s| s.mix == 0.0)
                    || (values[..5] == [0.0, 1000.0, 1000.0, 1000.0, 1000.0]
                        && hue_saturation.is_none()
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
                    multiplier: [values[2], values[3], values[4]].map(|v| exposure * v / 1000.0),
                    contrast: values[1] / 1000.0,
                    hue_saturation,
                    identity,
                    selection,
                })
            }
            _ => unreachable!("effect and validated sampler kinds agree"),
        });
    }
    Ok(Frame { stages })
}

impl Frame<'_> {
    /// Apply the ordered stack in floating-point linear light, then store once as UNORM16.
    /// HSL qualifiers, hue/saturation, LUTs and keys operate on continuous encoded sRGB;
    /// alpha stays linear coverage. `position` is the original source-canvas pixel coordinate.
    pub fn pixel(&self, input: Pixel, position: [i64; 2]) -> Result<Pixel> {
        if input.0[3] == 0 {
            return Ok(input);
        }
        let [r, g, b, mut coverage] = input.linear();
        let mut color = [r, g, b];
        let mut encoded_lut = None;
        let mut changed = false;
        for stage in &self.stages {
            if stage.identity() {
                continue;
            }
            match stage {
                Stage::Grade(g) => {
                    let weight = g.selection.as_ref().map_or(1.0, |s| {
                        s.mix
                            * s.effect.qualifier.as_ref().map_or(1.0, |q| {
                                q.weight_precise(
                                    encoded_lut.unwrap_or_else(|| color.map(super::encoded)),
                                )
                            })
                            * s.effect.mask.as_ref().map_or(1.0, |m| {
                                m.weight(s.mask_rect.expect("prepared correction mask"), position)
                            })
                    });
                    if weight <= 0.0 {
                        continue;
                    }
                    g.blend(&mut color, weight);
                    encoded_lut = None;
                    changed = true;
                }
                Stage::Lut(table) => {
                    let encoded_color = encoded_lut.unwrap_or_else(|| color.map(super::encoded));
                    let result = table
                        .sample(encoded_color)
                        .expect("validated effects supply bounded finite encoded RGB")
                        .map(|v| v.clamp(0.0, 1.0));
                    color = result.map(super::linear);
                    encoded_lut = Some(result);
                    changed = true;
                }
                Stage::Key(k) => {
                    let encoded_color = encoded_lut.unwrap_or_else(|| color.map(super::encoded));
                    if let Some((candidate, retention, weight)) =
                        k.apply_precise(encoded_color, position)
                    {
                        coverage *= retention;
                        if coverage == 0.0 {
                            return Ok(Pixel::TRANSPARENT);
                        }
                        for (value, candidate) in color.iter_mut().zip(candidate) {
                            *value = *value * (1.0 - weight) + super::linear(candidate) * weight;
                        }
                        encoded_lut = None;
                        changed = true;
                    }
                }
            }
        }
        if changed {
            Pixel::from_linear([color[0], color[1], color[2], coverage])
        } else {
            Ok(input)
        }
    }
}
