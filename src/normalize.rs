//! Loudness normalization: scale every audio clip's level by one factor, so the timeline's mix
//! reaches a target integrated loudness without its peak passing a ceiling. When the ceiling
//! would stop the gain short of the target, a master limiter at the ceiling is proposed too and
//! the gain keeps rising through it. The result is a proposal of `clip_audio` (and
//! `audio_dynamics`) operations whose outcome has been measured; nothing changes until they are
//! applied.
//!
//! The mix is rendered once at the current levels, as the unsaturated sums of its limiter groups
//! ([`Capture`]). The refinement passes replay those sums in memory: scaled by the candidate
//! factor, through the track limiters and the proposed master limiter, saturated and metered.
//! Before clip levels are rounded the mix is linear in the factor, so the replay is a close model
//! of the mix at the new levels. The proposed levels are then rendered once more and replayed
//! unscaled, which is exactly what the timeline plays; the master limiter's ceiling is settled on
//! that exact replay, since it needs no render.
use crate::{
    Result,
    animation::Curve,
    audio_processing::Meter,
    dynamics::{Capture, Limiter},
    error, media,
    meters::Scratch,
    model::Project,
    time::Time,
    tracks::{Kind, MAX_GAIN_MILLI},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Duration};

/// Accepted targets, in LKFS, ceilings, in dBFS, and limiting, in dB.
const TARGETS: std::ops::RangeInclusive<f64> = -40.0..=-5.0;
const CEILINGS: std::ops::RangeInclusive<f64> = -20.0..=0.0;
const LIMITING: std::ops::RangeInclusive<f64> = 0.0..=24.0;
/// Refinements after the first measurement. Mixing is linear, so one usually suffices; rounding
/// and saturation can take another.
const REFINEMENTS: usize = 3;
/// Refinements through the limiter: loudness then grows more slowly than the gain, and the
/// limiter's ceiling may be lowered to keep the true peak under the ceiling.
const LIMITED_REFINEMENTS: usize = 6;
/// Loudness within this many LU of the target needs no further pass through the limiter, and an
/// exact result this close to the target needs no further render.
const TOLERANCE: f64 = 0.05;
/// How far an exact result may pass the limit that stopped the gain, in dB, before the levels are
/// rendered again: rounding each level to a milli-unit moves the mix by a few thousandths of a dB.
const SLACK: f64 = 0.01;
/// Renders of the mix at most: the current levels, the proposed levels and one correction when
/// the exact result misses what the in-memory passes predicted.
const RENDERS: usize = 3;
/// Renders run side by side in pieces of the timeline of at least this many seconds, at most this
/// many at once.
const PIECE: u64 = 2;
const LANES: usize = 8;
/// Longest timeline, as for meters. The capture needs about 1.4 GB of scratch space per hour for
/// each limited track, plus one for the other tracks.
const MAX_SECONDS: u64 = crate::meters::MAX_SECONDS;

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
    let arrangement = copy.tracks.as_mut().expect("tracks");
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
    fn new(meters: Value, dynamics: Option<Value>) -> Result<Self> {
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
        Ok(Self {
            loudness,
            peak: highest("sample_peak_dbfs"),
            true_peak: highest("true_peak_dbtp"),
            dynamics,
        })
    }
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

/// The mix rendered with every clip level at `factor`, as captured group sums.
struct Anchor {
    capture: Capture,
    factor: f64,
}
impl Anchor {
    /// Render the group sums of `project`'s mix with its levels at `factor` into the scratch
    /// directory, as its `render`th capture.
    fn render(
        project: &Project,
        input_root: &Path,
        scratch: &Path,
        factor: f64,
        render: usize,
    ) -> Result<Self> {
        // Pieces of at least PIECE seconds render side by side, up to LANES at once.
        let lanes = std::thread::available_parallelism().map_or(1, |n| n.get().min(LANES));
        let pieces = project.duration()?.units(Time::new(48_000, 1)?)? / (PIECE * 48_000);
        let path = scratch.join(format!("mix-{render}"));
        let (premix, sources) = crate::track_render::premix(
            &at(project, factor),
            input_root,
            path,
            pieces.clamp(1, lanes as u64),
        )?;
        let capture = Capture::record(
            &premix,
            &media::Uncontrolled,
            Duration::from_secs(MAX_SECONDS),
            lanes,
        )?;
        for source in &sources {
            if media::file_hash(&source.path)? != source.sha256 {
                return Err(error("MEDIA_CHANGED", "Source changed during measurement"));
            }
        }
        Ok(Self { capture, factor })
    }
    /// The mix with every clip level at `factor`, through the track limiters and `master`. At the
    /// anchor's own factor this is exact; elsewhere the captured sums are scaled by the ratio.
    fn measure(&self, factor: f64, master: Option<&Limiter>) -> Result<Measured> {
        let mut meter = Meter::new();
        let report = self
            .capture
            .replay(factor / self.factor, master, |pair| meter.push(pair))?;
        Measured::new(meter.finish(), report.map(|r| r.value()))
    }
}

/// What the proposal asks for.
struct Goal {
    target_lkfs: f64,
    ceiling_dbfs: f64,
    limiter: bool,
    max_limiting_db: f64,
    /// The factor that lifts the highest clip level to the maximum.
    by_range: f64,
}

/// Where the refinement passes end.
struct Found {
    factor: f64,
    limiter: Option<Limiter>,
    limited_by: Value,
}

/// Refine the factor from `measured`, the mix at `anchor`'s factor through the project's own
/// master limiter, replaying the anchor for each pass.
fn search(
    project: &Project,
    anchor: &Anchor,
    measured: Measured,
    goal: &Goal,
    passes: &mut usize,
) -> Result<Found> {
    let arrangement = project.tracks.as_ref().expect("tracks");
    let own = arrangement.master.as_ref().map(|d| &d.limiter);
    // Each pass takes the smallest of the factor that reaches the target, the one that puts the
    // peak on the ceiling (both from the mix measured at the current factor) and the range cap,
    // then measures the mix at the new levels, until the levels stop changing.
    let mut factor = anchor.factor;
    let mut result = measured;
    let mut limited_by = Value::Null;
    for _ in 0..REFINEMENTS {
        let wanted = factor * 10f64.powf((goal.target_lkfs - result.loudness) / 20.0);
        let by_peak = if result.peak.is_finite() {
            factor * 10f64.powf((goal.ceiling_dbfs - result.peak) / 20.0)
        } else {
            f64::INFINITY
        };
        let next = wanted.min(by_peak).min(goal.by_range);
        limited_by = if wanted <= by_peak.min(goal.by_range) {
            Value::Null
        } else if by_peak <= goal.by_range {
            json!("peak_ceiling")
        } else {
            json!("clip_gain_range")
        };
        if levels(project, next) == levels(project, factor) {
            break;
        }
        factor = next;
        result = anchor.measure(factor, own)?;
        *passes += 1;
    }
    // Through a master limiter at the ceiling the gain rises on toward the target. Loudness then
    // grows more slowly than the gain, so later passes step by the measured slope; a true peak
    // over the ceiling lowers the limiter's own ceiling by the excess.
    if !(goal.limiter && limited_by == "peak_ceiling") {
        return Ok(Found {
            factor,
            limiter: None,
            limited_by,
        });
    }
    let mut settings = own.map_or_else(|| Limiter::new(goal.ceiling_dbfs), Limiter::clone);
    settings.ceiling_dbfs = goal.ceiling_dbfs;
    let mut previous: Option<(f64, f64)> = None;
    result = anchor.measure(factor, Some(&settings))?;
    *passes += 1;
    for _ in 0..LIMITED_REFINEMENTS {
        let lowered = lower(&mut settings, &result, goal);
        let gain = decibels(factor);
        let slope = match previous {
            Some((g, l)) if (gain - g).abs() > 0.01 => {
                ((result.loudness - l) / (gain - g)).clamp(0.2, 1.0)
            }
            _ => 1.0,
        };
        let missing = goal.target_lkfs - result.loudness;
        let wanted = if missing.abs() <= TOLERANCE {
            factor
        } else {
            10f64.powf((gain + missing / slope) / 20.0)
        };
        let by_reduction = factor * 10f64.powf((goal.max_limiting_db - result.reduction()) / 20.0);
        let next = wanted.min(by_reduction).min(goal.by_range);
        limited_by = if wanted <= by_reduction.min(goal.by_range) {
            Value::Null
        } else if by_reduction <= goal.by_range {
            json!("limiter_reduction")
        } else {
            json!("clip_gain_range")
        };
        if !lowered && levels(project, next) == levels(project, factor) {
            break;
        }
        previous = Some((gain, result.loudness));
        factor = next;
        result = anchor.measure(factor, Some(&settings))?;
        *passes += 1;
    }
    Ok(Found {
        factor,
        limiter: Some(settings),
        limited_by,
    })
}

/// Lower the limiter's ceiling by the amount `result`'s true peak passes the goal's ceiling, to
/// hundredths of a dB; false when it is under the ceiling or the limiter is already at its floor.
fn lower(settings: &mut Limiter, result: &Measured, goal: &Goal) -> bool {
    let over = result.true_peak - goal.ceiling_dbfs;
    let lowered = over > 0.0 && settings.ceiling_dbfs > *CEILINGS.start();
    if lowered {
        settings.ceiling_dbfs =
            (((settings.ceiling_dbfs - over) * 100.0).floor() / 100.0).max(*CEILINGS.start());
    }
    lowered
}

/// Whether an exact result keeps what the passes found: the target within the tolerance, or the
/// limit that stopped the gain within the slack.
fn kept(found: &Found, result: &Measured, goal: &Goal) -> bool {
    match found.limited_by.as_str() {
        None => (result.loudness - goal.target_lkfs).abs() <= TOLERANCE,
        Some("peak_ceiling") => result.peak <= goal.ceiling_dbfs + SLACK,
        Some("limiter_reduction") => result.reduction() <= goal.max_limiting_db + SLACK,
        _ => true,
    }
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
    if project
        .duration()?
        .compare(Time::new(MAX_SECONDS, 1)?)?
        .is_gt()
    {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!("Normalizing measures at most {MAX_SECONDS} s of timeline"),
        ));
    }
    // The clip gain range caps the factor: the largest level may rise only to the maximum.
    let largest = levels(project, 1.0)
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0);
    let goal = Goal {
        target_lkfs,
        ceiling_dbfs,
        limiter,
        max_limiting_db,
        by_range: if largest == 0 {
            f64::INFINITY
        } else {
            MAX_GAIN_MILLI as f64 / largest as f64
        },
    };
    let own = arrangement.master.as_ref().map(|d| &d.limiter);
    let scratch = Scratch::new("cutbolt-normalize")?;
    let mut renders = 1;
    let mut anchor = Anchor::render(project, input_root, &scratch.0, 1.0, renders)?;
    let measured = anchor.measure(1.0, own)?;
    let mut passes = 1;
    let mut start = measured.clone();
    // The passes run on the latest render; the levels they settle on are rendered and measured
    // exactly, and only an exact result that misses what they found is refined again.
    let (found, result) = loop {
        let mut found = search(project, &anchor, start, &goal, &mut passes)?;
        if levels(project, found.factor) != levels(project, anchor.factor) {
            renders += 1;
            anchor = Anchor::render(project, input_root, &scratch.0, found.factor, renders)?;
        }
        // The capture plays exactly these levels; the limiter's ceiling needs no render to settle.
        let mut result = anchor.measure(anchor.factor, found.limiter.as_ref().or(own))?;
        passes += 1;
        if let Some(settings) = &mut found.limiter {
            for _ in 0..LIMITED_REFINEMENTS {
                if !lower(settings, &result, &goal) {
                    break;
                }
                result = anchor.measure(anchor.factor, Some(settings))?;
                passes += 1;
            }
        }
        if renders == RENDERS || kept(&found, &result, &goal) {
            break (found, result);
        }
        // Refine again from the exact mix through the project's own master limiter.
        start = match found.limiter {
            Some(_) => {
                passes += 1;
                anchor.measure(anchor.factor, own)?
            }
            None => result,
        };
    };
    let factor = found.factor;
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
    if let Some(settings) = &found.limiter
        && own != Some(settings)
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
    result_value["renders"] = json!(renders);
    if let Some(dynamics) = &result.dynamics {
        result_value["dynamics"] = dynamics.clone();
    }
    Ok(
        json!({"target_lkfs":target_lkfs,"peak_ceiling_dbfs":ceiling_dbfs,
        "measured":measured_value,
        "gain_db":hundredths(decibels(factor)),"limited_by":found.limited_by,"limiter":found.limiter,
        "result":result_value,
        "clips":changed,"operations":operations,
        "next":"apply operations with session.apply (or check them with session.preview); result is the measured mix with them applied"}),
    )
}
