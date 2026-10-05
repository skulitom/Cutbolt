//! Loudness normalization: scale every audio clip's level by one factor, so the timeline's mix
//! reaches a target integrated loudness without its peak passing a ceiling. When the ceiling
//! would stop the gain short of the target, a master limiter at the ceiling is proposed too and
//! the gain keeps rising through it. The result is a proposal of `clip_audio` (and
//! `audio_dynamics`) operations whose outcome has been measured; nothing changes until they are
//! applied.
use crate::{
    Result,
    animation::Curve,
    dynamics::{Dynamics, Limiter},
    error,
    model::Project,
    time::Time,
    tracks::{Kind, MAX_GAIN_MILLI},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Accepted targets, in LKFS, ceilings, in dBFS, and limiting, in dB.
const TARGETS: std::ops::RangeInclusive<f64> = -40.0..=-5.0;
const CEILINGS: std::ops::RangeInclusive<f64> = -20.0..=0.0;
const LIMITING: std::ops::RangeInclusive<f64> = 0.0..=24.0;
/// Refinements after the first measurement. Mixing is linear, so one usually suffices; a mix
/// that already clips hides its true peak until it is lowered, which takes another.
const REFINEMENTS: usize = 3;
/// Refinements through the limiter: loudness then grows more slowly than the gain, and the
/// limiter's ceiling may be lowered to keep the true peak under the ceiling.
const LIMITED_REFINEMENTS: usize = 6;
/// Loudness within this many LU of the target needs no further pass through the limiter.
const TOLERANCE: f64 = 0.05;

fn decibels(factor: f64) -> f64 {
    20.0 * factor.log10()
}

/// Round to hundredths for reporting.
fn hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// A level scaled by `factor`, rounded to the nearest milli-unit and kept in the clip range.
fn scaled(value: u32, factor: f64) -> u32 {
    ((value as f64 * factor).round() as u32).min(MAX_GAIN_MILLI)
}

/// Every audio clip's levels at `factor`: its gain, or its curve's key values.
fn levels(project: &Project, factor: f64) -> Vec<Vec<u32>> {
    let arrangement = project.tracks.as_ref().expect("tracks");
    let mut all = Vec::new();
    for track in arrangement.tracks.iter().filter(|t| t.kind == Kind::Audio) {
        for clip in &track.clips {
            all.push(match &clip.gain_curve {
                Some(curve) => curve
                    .keys
                    .iter()
                    .map(|k| scaled(k.value.max(0) as u32, factor))
                    .collect(),
                None => vec![scaled(clip.gain_milli, factor)],
            });
        }
    }
    all
}

/// The project with its audio clip levels at `factor`, and with `limiter` on the master when given.
fn at(project: &Project, factor: f64, limiter: Option<&Limiter>) -> Project {
    let mut copy = project.clone();
    let arrangement = copy.tracks.as_mut().expect("tracks");
    if let Some(limiter) = limiter {
        arrangement.master = Some(Dynamics {
            limiter: limiter.clone(),
        });
    }
    for track in arrangement.tracks.iter_mut() {
        if track.kind != Kind::Audio {
            continue;
        }
        for clip in &mut track.clips {
            match &mut clip.gain_curve {
                Some(curve) => {
                    for key in &mut curve.keys {
                        key.value = scaled(key.value.max(0) as u32, factor) as i32;
                    }
                }
                None => clip.gain_milli = scaled(clip.gain_milli, factor),
            }
        }
    }
    copy
}

/// One measurement of the whole mix.
#[derive(Clone)]
struct Measured {
    loudness: f64,
    /// The higher channel's sample peak and true peak; negative infinity for silence.
    peak: f64,
    true_peak: f64,
    /// Gain reduction of the limiters, when the mix has any.
    dynamics: Option<Value>,
}
impl Measured {
    /// The master limiter's deepest gain reduction in dB, zero without one.
    fn reduction(&self) -> f64 {
        self.dynamics
            .as_ref()
            .and_then(|d| d["master"]["max_reduction_db"].as_f64())
            .unwrap_or(0.0)
    }
    fn value(&self) -> Value {
        let peak = |p: f64| p.is_finite().then(|| hundredths(p));
        json!({"integrated_lkfs":hundredths(self.loudness),"sample_peak_dbfs":peak(self.peak),"true_peak_dbtp":peak(self.true_peak)})
    }
}

/// Integrated loudness, peaks and limiting of the whole mix.
fn measure(project: &Project, input_root: &Path) -> Result<Measured> {
    let meters =
        crate::meters::measure(project, input_root, Time::ZERO, project.duration()?, false)?;
    let Some(loudness) = meters["integrated_lkfs"].as_f64() else {
        return Err(error(
            "LOUDNESS_UNMEASURED",
            format!(
                "The mix has no measurable loudness ({}); nothing to normalize",
                meters["loudness_status"].as_str().unwrap_or("unmeasured")
            ),
        ));
    };
    let highest = |field: &str| {
        meters[field]
            .as_array()
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_f64)
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .unwrap_or(f64::NEG_INFINITY)
    };
    Ok(Measured {
        loudness,
        peak: highest("sample_peak_dbfs"),
        true_peak: highest("true_peak_dbtp"),
        dynamics: meters.get("dynamics").cloned(),
    })
}

/// Propose clip levels that bring the mix of enabled audio tracks to `target_lkfs`; with
/// `limiter`, also a master limiter when the peak ceiling would stop them short, applying at most
/// `max_limiting_db` of gain reduction.
pub fn propose(
    project: &Project,
    input_root: &Path,
    target_lkfs: f64,
    ceiling_dbfs: f64,
    limiter: bool,
    max_limiting_db: f64,
) -> Result<Value> {
    project.validate()?;
    if !TARGETS.contains(&target_lkfs)
        || !CEILINGS.contains(&ceiling_dbfs)
        || !LIMITING.contains(&max_limiting_db)
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "target_lkfs must be -40..-5, peak_ceiling_dbfs -20..0 and max_limiting_db 0..24",
        ));
    }
    let arrangement = project.tracks.as_ref().ok_or_else(|| {
        error(
            "UNSUPPORTED_TIMELINE",
            "Normalizing sets clip levels on placed audio tracks; promote the sequential timeline first",
        )
    })?;
    let audio: Vec<_> = arrangement
        .tracks
        .iter()
        .filter(|t| t.kind == Kind::Audio)
        .collect();
    let locked: Vec<&str> = audio
        .iter()
        .filter(|t| t.locked && !t.clips.is_empty())
        .map(|t| t.id.as_str())
        .collect();
    if !locked.is_empty() {
        return Err(error(
            "TRACK_LOCKED",
            format!(
                "Normalizing changes every audio clip; unlock audio track(s) {}",
                locked.join(", ")
            ),
        ));
    }
    // The clip gain range caps the factor: the largest level may rise only to the maximum.
    let largest = levels(project, 1.0)
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0);
    let by_range = if largest == 0 {
        f64::INFINITY
    } else {
        MAX_GAIN_MILLI as f64 / largest as f64
    };
    let measured = measure(project, input_root)?;
    // Each pass takes the smallest of the factor that reaches the target, the one that puts the
    // peak on the ceiling (both from the mix measured at the current factor) and the range cap,
    // then measures the mix at the new levels, until the levels stop changing.
    let mut factor = 1.0;
    let mut result = measured.clone();
    let mut limited_by = Value::Null;
    let mut passes = 1;
    for _ in 0..REFINEMENTS {
        let wanted = factor * 10f64.powf((target_lkfs - result.loudness) / 20.0);
        let by_peak = if result.peak.is_finite() {
            factor * 10f64.powf((ceiling_dbfs - result.peak) / 20.0)
        } else {
            f64::INFINITY
        };
        let next = wanted.min(by_peak).min(by_range);
        limited_by = if wanted <= by_peak.min(by_range) {
            Value::Null
        } else if by_peak <= by_range {
            json!("peak_ceiling")
        } else {
            json!("clip_gain_range")
        };
        if levels(project, next) == levels(project, factor) {
            break;
        }
        factor = next;
        result = measure(&at(project, factor, None), input_root)?;
        passes += 1;
    }
    // Through a master limiter at the ceiling the gain rises on toward the target. Loudness then
    // grows more slowly than the gain, so later passes step by the measured slope; a true peak
    // over the ceiling lowers the limiter's own ceiling by the excess.
    let mut proposed: Option<Limiter> = None;
    if limiter && limited_by == "peak_ceiling" {
        let mut settings = arrangement
            .master
            .as_ref()
            .map_or_else(|| Limiter::new(ceiling_dbfs), |d| d.limiter.clone());
        settings.ceiling_dbfs = ceiling_dbfs;
        let mut previous: Option<(f64, f64)> = None;
        result = measure(&at(project, factor, Some(&settings)), input_root)?;
        passes += 1;
        for _ in 0..LIMITED_REFINEMENTS {
            let over = result.true_peak - ceiling_dbfs;
            let lowered = over > 0.0 && settings.ceiling_dbfs > *CEILINGS.start();
            if lowered {
                settings.ceiling_dbfs = (((settings.ceiling_dbfs - over) * 100.0).floor() / 100.0)
                    .max(*CEILINGS.start());
            }
            let gain = decibels(factor);
            let slope = match previous {
                Some((g, l)) if (gain - g).abs() > 0.01 => {
                    ((result.loudness - l) / (gain - g)).clamp(0.2, 1.0)
                }
                _ => 1.0,
            };
            let missing = target_lkfs - result.loudness;
            let wanted = if missing.abs() <= TOLERANCE {
                factor
            } else {
                10f64.powf((gain + missing / slope) / 20.0)
            };
            let by_reduction = factor * 10f64.powf((max_limiting_db - result.reduction()) / 20.0);
            let next = wanted.min(by_reduction).min(by_range);
            limited_by = if wanted <= by_reduction.min(by_range) {
                Value::Null
            } else if by_reduction <= by_range {
                json!("limiter_reduction")
            } else {
                json!("clip_gain_range")
            };
            if !lowered && levels(project, next) == levels(project, factor) {
                break;
            }
            previous = Some((gain, result.loudness));
            factor = next;
            result = measure(&at(project, factor, Some(&settings)), input_root)?;
            passes += 1;
        }
        proposed = Some(settings);
    }
    // Clips sharing a level share one operation; curves are scaled key by key.
    let mut grouped: BTreeMap<u32, Vec<&str>> = BTreeMap::new();
    let mut curves = Vec::new();
    let mut changed = 0;
    for track in &audio {
        for clip in &track.clips {
            match &clip.gain_curve {
                Some(curve) => {
                    let mut keys = curve.keys.clone();
                    for key in &mut keys {
                        key.value = scaled(key.value.max(0) as u32, factor) as i32;
                    }
                    if keys != curve.keys {
                        let curve = Curve {
                            keys,
                            retime: curve.retime,
                        };
                        curves.push(json!({"op":"tracks.edit","edit":{"op":"clip_audio","clip_ids":[clip.id],"gain_curve":curve}}));
                        changed += 1;
                    }
                }
                None => {
                    let level = scaled(clip.gain_milli, factor);
                    if level != clip.gain_milli {
                        grouped.entry(level).or_default().push(&clip.id);
                        changed += 1;
                    }
                }
            }
        }
    }
    let mut operations: Vec<Value> = Vec::new();
    if let Some(settings) = &proposed
        && arrangement.master.as_ref().map(|d| &d.limiter) != Some(settings)
    {
        operations.push(json!({"op":"tracks.edit","edit":{"op":"audio_dynamics","dynamics":{"limiter":settings}}}));
    }
    operations.extend(
        grouped
            .into_iter()
            .map(|(level, ids)| json!({"op":"tracks.edit","edit":{"op":"clip_audio","clip_ids":ids,"gain_milli":level}})),
    );
    operations.append(&mut curves);
    let mut measured_value = measured.value();
    measured_value["duration"] = json!(project.duration()?);
    let mut result_value = result.value();
    result_value["measured_passes"] = json!(passes);
    if let Some(dynamics) = &result.dynamics {
        result_value["dynamics"] = dynamics.clone();
    }
    Ok(
        json!({"target_lkfs":target_lkfs,"peak_ceiling_dbfs":ceiling_dbfs,
        "measured":measured_value,
        "gain_db":hundredths(decibels(factor)),"limited_by":limited_by,"limiter":proposed,
        "result":result_value,
        "clips":changed,"operations":operations,
        "next":"apply operations with session.apply (or check them with session.preview); result is the measured mix with them applied"}),
    )
}
