//! Loudness normalization: scale every audio clip's level by one factor, so the timeline's mix
//! reaches a target integrated loudness without its sample peak passing a ceiling. The result is
//! a proposal of `clip_audio` operations whose outcome has been measured; nothing changes until
//! they are applied.
use crate::{
    Result,
    animation::Curve,
    error,
    model::Project,
    time::Time,
    tracks::{Kind, MAX_GAIN_MILLI},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// Accepted targets, in LKFS, and ceilings, in dBFS.
const TARGETS: std::ops::RangeInclusive<f64> = -40.0..=-5.0;
const CEILINGS: std::ops::RangeInclusive<f64> = -20.0..=0.0;
/// Refinements after the first measurement. Mixing is linear, so one usually suffices; a mix
/// that already clips hides its true peak until it is lowered, which takes another.
const REFINEMENTS: usize = 3;

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

/// The project with its audio clip levels at `factor`.
fn at(project: &Project, factor: f64) -> Project {
    let mut copy = project.clone();
    for track in copy.tracks.as_mut().expect("tracks").tracks.iter_mut() {
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

/// Integrated loudness and the higher channel's sample peak of the whole mix.
fn measure(project: &Project, input_root: &Path) -> Result<(f64, f64)> {
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
    let peak = meters["sample_peak_dbfs"]
        .as_array()
        .map(|p| {
            p.iter()
                .filter_map(Value::as_f64)
                .fold(f64::NEG_INFINITY, f64::max)
        })
        .unwrap_or(f64::NEG_INFINITY);
    Ok((loudness, peak))
}

/// Propose clip levels that bring the mix of enabled audio tracks to `target_lkfs`.
pub fn propose(
    project: &Project,
    input_root: &Path,
    target_lkfs: f64,
    ceiling_dbfs: f64,
) -> Result<Value> {
    project.validate()?;
    if !TARGETS.contains(&target_lkfs) || !CEILINGS.contains(&ceiling_dbfs) {
        return Err(error(
            "INVALID_ARGUMENT",
            "target_lkfs must be -40..-5 and peak_ceiling_dbfs -20..0",
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
    let (measured, measured_peak) = measure(project, input_root)?;
    // Each pass takes the smallest of the factor that reaches the target, the one that puts the
    // peak on the ceiling (both from the mix measured at the current factor) and the range cap,
    // then measures the mix at the new levels, until the levels stop changing.
    let (mut factor, mut loudness, mut peak) = (1.0, measured, measured_peak);
    let mut limited_by = Value::Null;
    let mut passes = 1;
    for _ in 0..REFINEMENTS {
        let wanted = factor * 10f64.powf((target_lkfs - loudness) / 20.0);
        let by_peak = if peak.is_finite() {
            factor * 10f64.powf((ceiling_dbfs - peak) / 20.0)
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
        (loudness, peak) = measure(&at(project, factor), input_root)?;
        passes += 1;
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
    let mut operations: Vec<Value> = grouped
        .into_iter()
        .map(|(level, ids)| json!({"op":"tracks.edit","edit":{"op":"clip_audio","clip_ids":ids,"gain_milli":level}}))
        .collect();
    operations.append(&mut curves);
    let peak_value = |p: f64| p.is_finite().then(|| hundredths(p));
    Ok(
        json!({"target_lkfs":target_lkfs,"peak_ceiling_dbfs":ceiling_dbfs,
        "measured":{"integrated_lkfs":hundredths(measured),"sample_peak_dbfs":peak_value(measured_peak),"duration":project.duration()?},
        "gain_db":hundredths(decibels(factor)),"limited_by":limited_by,
        "result":{"integrated_lkfs":hundredths(loudness),"sample_peak_dbfs":peak_value(peak),"measured_passes":passes},
        "clips":changed,"operations":operations,
        "next":"apply operations with session.apply (or check them with session.preview); result is the measured mix at the proposed levels"}),
    )
}
