//! Original bounded audio clock estimation and explicit non-drop clock-label alignment.
use crate::{
    Result, conform, error, media, model::Project, registry, render, scene::Identity, time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
const RATE: Time = Time { num: 48000, den: 1 };
const FPS: Time = Time { num: 25, den: 1 };

/// Audio search window pairing approximate positions of one distinctive event in both sources; the full search interval must fit each source.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Window {
    /// Window center in the reference source, rational seconds on the 48 kHz grid.
    pub reference_center: Time,
    /// Approximate matching position in the candidate source, rational seconds on the 48 kHz grid.
    pub candidate_center: Time,
    /// Correlation window length, an even 2048..8192 samples, as rational seconds.
    pub length: Time,
    /// Lag searched either side of `candidate_center`, 32..48000 samples, as rational seconds.
    pub search_radius: Time,
}
/// Accepted correlation sign: `same` requires positive correlation; `either` also accepts inverted polarity, which is reported but not corrected.
#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Polarity {
    Same,
    Either,
}
/// Mapped candidate start policy: `reject` requires an exactly sample-aligned start; `nearest_sample` rounds to the nearest 48 kHz sample, half ties upward.
#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    Reject,
    NearestSample,
}
/// Declared 25 fps non-drop clock label at one position in a source.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stamp {
    /// Explicit day number, 0..1000000; midnight wrap is never inferred.
    pub day: u32,
    /// Clock label `HH:MM:SS:FF`: hours 0..23, minutes and seconds 0..59, frames 0..24.
    pub label: String,
    /// In-source position the label names, frame-aligned rational seconds at 25 fps, inside the source.
    pub media_at: Time,
}
/// How the clock mapping is obtained, tagged by `mode`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Method {
    /// Estimate offset and constant drift by correlating audio in declared windows.
    Audio {
        /// 3..8 windows; reference centers must be distinct and span at least one second.
        windows: Vec<Window>,
        /// Reference source channel, 0 (left) or 1 (right).
        reference_channel: u8,
        /// Candidate source channel, 0 (left) or 1 (right).
        candidate_channel: u8,
        /// Accepted correlation sign.
        polarity: Polarity,
        /// Minimum peak correlation per window, 500..1000 milli.
        minimum_correlation_milli: u16,
        /// Minimum lead of the best peak over the next separated peak, 1..1000 milli.
        minimum_margin_milli: u16,
        /// Maximum accepted clock drift in parts per million, 0..5000.
        maximum_drift_ppm: u32,
        /// Maximum disagreement of any window with the fitted linear clock, 0..16 samples.
        maximum_residual_samples: u32,
    },
    /// Align by declared equal-rate clock labels; drift is not measured.
    Timecode {
        /// Clock label in the reference source.
        reference: Stamp,
        /// Clock label in the candidate source.
        candidate: Stamp,
    },
}
/// `sync.inspect` request: maps a candidate source clock onto a reference and returns `media.conform` recipes; read-only.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    /// Alignment ID used to name the returned recipes; 1..100 bytes, not blank.
    pub id: String,
    /// Project snapshot at 25 fps that contains both content-bound assets.
    pub project: Project,
    /// Absolute directory containing both asset files.
    pub input_root: PathBuf,
    /// ID of the reference asset in `project.assets`; 25 fps video with 48 kHz stereo PCM16, at most 60 seconds.
    pub reference_asset_id: String,
    /// ID of the asset to align, with the same source profile.
    pub candidate_asset_id: String,
    /// Interval start on the reference clock, frame-aligned rational seconds at 25 fps.
    pub start: Time,
    /// Interval length, frame-aligned at 25 fps, 1..1500 frames.
    pub duration: Time,
    /// Rounding policy for the mapped candidate start.
    pub rounding: Rounding,
    /// Audio-correlation or declared-timecode method.
    pub method: Method,
}
#[derive(Clone, Debug, Serialize)]
struct Mapping {
    reference_anchor: Time,
    candidate_anchor: Time,
    rate: Time,
}
impl Mapping {
    fn at(&self, reference: Time) -> Result<Time> {
        if reference.compare(self.reference_anchor)?.is_ge() {
            self.candidate_anchor
                .plus(reference.minus(self.reference_anchor)?.times(self.rate)?)
        } else {
            self.candidate_anchor
                .minus(self.reference_anchor.minus(reference)?.times(self.rate)?)
        }
    }
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_SYNC", message)
}
fn source(request: &Inspect, id: &str) -> Result<render::Source> {
    let asset = request
        .project
        .assets
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| error("MISSING_MEDIA", id))?;
    if asset.identity.is_none() {
        return Err(error(
            "IDENTITY_REQUIRED",
            "Bind both synchronization sources before inspection",
        ));
    }
    let path = media::project_file(Path::new(&asset.path), &request.input_root)?;
    let source = render::inspect_reference(
        &path,
        request.project.width,
        request.project.height,
        &media::Uncontrolled,
    )?;
    registry::verify_source(asset, &source)?;
    if source.samples > 60 * 48000 || source.samples == 0 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Clock inspection supports at most 60 seconds of stereo reference audio",
        ));
    }
    Ok(source)
}
fn prefix(source: &render::Source, channel: u8) -> Result<Vec<i64>> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-i".into(),
        source.path.to_string_lossy().into_owned(),
        "-map".into(),
        "0:a:0".into(),
        "-vn".into(),
        "-f".into(),
        "s16le".into(),
        "pipe:1".into(),
    ];
    let bytes = media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(120))?;
    if bytes.len() as u64 != source.samples * 4 {
        return Err(error(
            "MEDIA_CHANGED",
            "Decoded audio count differs from inspection",
        ));
    }
    let mut values = Vec::with_capacity(source.samples as usize + 1);
    values.push(0);
    let mut sum = 0;
    for frame in bytes.as_chunks::<4>().0 {
        let at = channel as usize * 2;
        sum += i16::from_le_bytes([frame[at], frame[at + 1]]) as i64;
        values.push(sum);
    }
    Ok(values)
}
#[allow(clippy::too_many_arguments)]
fn correlation(
    a: &[i64],
    b: &[i64],
    x: usize,
    y: usize,
    length: usize,
    width: usize,
    stride: usize,
) -> Option<f64> {
    let (mut n, mut sx, mut sy, mut xx, mut yy, mut xy) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for k in (0..=length - width).step_by(stride) {
        let u = (a[x + k + width] - a[x + k]) as f64 / width as f64;
        let v = (b[y + k + width] - b[y + k]) as f64 / width as f64;
        n += 1.0;
        sx += u;
        sy += v;
        xx += u * u;
        yy += v * v;
        xy += u * v;
    }
    let vx = xx - sx * sx / n;
    let vy = yy - sy * sy / n;
    if vx < n * 128.0 * 128.0 || vy < n * 128.0 * 128.0 {
        return None;
    }
    Some(((xy - sx * sy / n) / (vx * vy).sqrt()).clamp(-1.0, 1.0))
}
#[derive(Clone, Debug, Serialize)]
struct Match {
    reference_center: Time,
    candidate_center: Time,
    lag_samples: i64,
    correlation: f64,
    separated_peak_margin: f64,
    inverted: bool,
}
fn measure(
    a: &[i64],
    b: &[i64],
    window: &Window,
    polarity: Polarity,
    minimum: f64,
    margin: f64,
) -> Result<Match> {
    let x = window.reference_center.units(RATE)? as usize;
    let y = window.candidate_center.units(RATE)? as usize;
    let length = window.length.units(RATE)? as usize;
    let radius = window.search_radius.units(RATE)? as usize;
    if !(2048..=8192).contains(&length)
        || !length.is_multiple_of(2)
        || !(32..=48000).contains(&radius)
        || x < length / 2
        || y < length / 2 + radius
        || x >= a.len()
        || y >= b.len()
        || x + length / 2 >= a.len()
        || y + length / 2 + radius >= b.len()
    {
        return Err(invalid(
            "Audio windows require 2048..8192 even samples, radius 32..48000 and complete in-bounds search intervals",
        ));
    }
    let x = x - length / 2;
    let y = y - length / 2;
    let rank = |c: f64| {
        if matches!(polarity, Polarity::Either) {
            c.abs()
        } else {
            c
        }
    };
    let mut coarse = Vec::new();
    for lag in (-(radius as i64)..=radius as i64).step_by(16) {
        if let Some(c) = correlation(a, b, x, (y as i64 + lag) as usize, length, 64, 16) {
            coarse.push((rank(c), lag));
        }
    }
    coarse.sort_by(|a, b| b.0.total_cmp(&a.0));
    if coarse.is_empty() {
        return Err(error(
            "SYNC_INSUFFICIENT_SIGNAL",
            "Audio lacks sufficient varying signal for alignment",
        ));
    }
    let mut peaks: Vec<(f64, i64)> = Vec::new();
    for (score, lag) in coarse {
        if peaks.iter().all(|(_, old)| (lag - old).abs() > 128) {
            peaks.push((score, lag));
        }
        if peaks.len() == 32 {
            break;
        }
    }
    let mut refined = Vec::new();
    for (_, lag) in peaks {
        let mut best: Option<(f64, i64, f64)> = None;
        for at in (lag - 24).max(-(radius as i64))..=(lag + 24).min(radius as i64) {
            if let Some(c) = correlation(a, b, x, (y as i64 + at) as usize, length, 16, 4)
                && best.as_ref().is_none_or(|old| rank(c) > old.0)
            {
                best = Some((rank(c), at, c));
            }
        }
        if let Some(best) = best {
            refined.push(best);
        }
    }
    refined.sort_by(|a, b| b.0.total_cmp(&a.0));
    let Some(&(score, lag, signed)) = refined.first() else {
        return Err(error(
            "SYNC_INSUFFICIENT_SIGNAL",
            "No usable refined audio match",
        ));
    };
    let runner = refined
        .iter()
        .skip(1)
        .filter(|(_, at, _)| (at - lag).abs() > 128)
        .map(|(s, _, _)| *s)
        .fold(0.0, f64::max);
    if score < minimum || score - runner < margin {
        return Err(error(
            "SYNC_AMBIGUOUS",
            format!(
                "Audio correlation {score:.6} or separated-peak margin {:.6} is below the declared threshold",
                score - runner
            ),
        ));
    }
    Ok(Match {
        reference_center: window.reference_center,
        candidate_center: Time::new(
            (window.candidate_center.units(RATE)? as i64 + lag) as u64,
            48000,
        )?,
        lag_samples: lag,
        correlation: score,
        separated_peak_margin: score - runner,
        inverted: signed < 0.0,
    })
}
#[allow(clippy::too_many_arguments)]
fn audio_mapping(
    reference: &render::Source,
    candidate: &render::Source,
    windows: &[Window],
    reference_channel: u8,
    candidate_channel: u8,
    polarity: Polarity,
    minimum: u16,
    margin: u16,
    max_ppm: u32,
    residual: u32,
) -> Result<(Mapping, Value)> {
    if !(3..=8).contains(&windows.len())
        || reference_channel > 1
        || candidate_channel > 1
        || !(500..=1000).contains(&minimum)
        || !(1..=1000).contains(&margin)
        || max_ppm > 5000
        || residual > 16
    {
        return Err(invalid(
            "Audio alignment requires 3..8 windows, channels 0/1, correlation 500..1000, margin 1..1000, drift <=5000 ppm and residual <=16 samples",
        ));
    }
    let a = prefix(reference, reference_channel)?;
    let b = prefix(candidate, candidate_channel)?;
    let mut matches = windows
        .iter()
        .map(|w| {
            measure(
                &a,
                &b,
                w,
                polarity,
                minimum as f64 / 1000.0,
                margin as f64 / 1000.0,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    matches.sort_by(|a, b| {
        a.reference_center
            .compare(b.reference_center)
            .expect("checked centers")
    });
    if matches.windows(2).any(|p| {
        p[0].reference_center
            .compare(p[1].reference_center)
            .expect("checked")
            .is_eq()
    }) {
        return Err(invalid("Reference centers must be distinct"));
    }
    let first = &matches[0];
    let last = matches.last().expect("three matches");
    let dr = last
        .reference_center
        .minus(first.reference_center)?
        .units(RATE)?;
    let dc = last
        .candidate_center
        .minus(first.candidate_center)?
        .units(RATE)?;
    if dr < 48000 || dc == 0 {
        return Err(invalid(
            "Calibration windows must span at least one second with increasing candidate time",
        ));
    }
    if (dc.abs_diff(dr) as u128) * 1_000_000 > max_ppm as u128 * dr as u128 {
        return Err(error(
            "SYNC_DRIFT_EXCEEDED",
            "Measured clock drift exceeds the declared limit",
        ));
    }
    let mapping = Mapping {
        reference_anchor: first.reference_center,
        candidate_anchor: first.candidate_center,
        rate: Time::new(dc, dr)?,
    };
    let mut errors = Vec::new();
    for m in &matches {
        let expected = mapping.at(m.reference_center)?;
        let difference = if expected.compare(m.candidate_center)?.is_ge() {
            expected.minus(m.candidate_center)?
        } else {
            m.candidate_center.minus(expected)?
        };
        if difference
            .compare(Time::new(residual as u64, 48000)?)?
            .is_gt()
            || m.inverted != first.inverted
        {
            return Err(error(
                "SYNC_INCONSISTENT",
                "Audio windows disagree with one linear clock or polarity",
            ));
        }
        errors.push(difference.num as f64 * 48000.0 / difference.den as f64);
    }
    Ok((
        mapping,
        json!({"mode":"audio","estimator":"bounded_coarse_fine_normalized_correlation_v1","reference_channel":reference_channel,"candidate_channel":candidate_channel,"windows":matches,"residual_samples":errors,"drift_ppm":(dc as f64/dr as f64-1.0)*1_000_000.0,"clock_model":"constant_rate","uncertainty":"estimated_alignment_within_declared_search_windows;not_ground_truth"}),
    ))
}
fn stamp(s: &Stamp) -> Result<Time> {
    let b = s.label.as_bytes();
    if b.len() != 11
        || [2, 5, 8].iter().any(|i| b[*i] != b':')
        || b.iter()
            .enumerate()
            .any(|(i, c)| ![2, 5, 8].contains(&i) && !c.is_ascii_digit())
        || s.day > 1_000_000
    {
        return Err(error(
            "UNSUPPORTED_TIMECODE",
            "Use explicit day and 25 fps non-drop HH:MM:SS:FF labels",
        ));
    }
    let pair = |i| ((b[i] - b'0') as u64) * 10 + (b[i + 1] - b'0') as u64;
    let (h, m, sec, f) = (pair(0), pair(3), pair(6), pair(9));
    if h >= 24 || m >= 60 || sec >= 60 || f >= 25 {
        return Err(error(
            "INVALID_TIMECODE",
            "Clock-label field is out of range",
        ));
    }
    s.media_at.units(FPS)?;
    Time::new(
        ((s.day as u64 * 86400 + h * 3600 + m * 60 + sec) * 25) + f,
        25,
    )
}
fn timecode_mapping(reference: &Stamp, candidate: &Stamp) -> Result<(Mapping, Value)> {
    let r = stamp(reference)?;
    let c = stamp(candidate)?;
    let common = if r.compare(c)?.is_ge() { r } else { c };
    Ok((
        Mapping {
            reference_anchor: reference.media_at.plus(common.minus(r)?)?,
            candidate_anchor: candidate.media_at.plus(common.minus(c)?)?,
            rate: Time::new(1, 1)?,
        },
        json!({"mode":"timecode","frame_rate":25,"drop_frame":false,"reference_clock":r,"candidate_clock":c,"day_rollover":"explicit","clock_model":"declared_equal_rate","drift_estimated":false}),
    ))
}
fn snap(time: Time, rounding: Rounding) -> Result<Time> {
    if matches!(rounding, Rounding::Reject) {
        return Time::new(time.units(RATE)?, 48000);
    }
    let n = time.num as u128 * 48000;
    let d = time.den as u128;
    let rounded = n / d + u128::from((n % d) * 2 >= d);
    Time::new(
        u64::try_from(rounded).map_err(|_| error("TIME_OVERFLOW", "Sample rounding overflow"))?,
        48000,
    )
}
fn recipe(
    request: &Inspect,
    source: &render::Source,
    root: &Path,
    start: Time,
    rate: Time,
    suffix: &str,
) -> Result<conform::Recipe> {
    Ok(conform::Recipe {
        decode: None,
        schema_version: 1,
        id: format!("{}-{suffix}", request.id),
        source: conform::Source {
            file: Identity {
                path: source
                    .path
                    .strip_prefix(root)
                    .map_err(|_| {
                        error("PATH_OUTSIDE_ROOT", "Synchronization source left its root")
                    })?
                    .to_path_buf(),
                sha256: source.sha256.clone(),
                bytes: source.path.metadata()?.len(),
            },
            color: Some(conform::Color::EncodedRgb),
            sdr: None,
        },
        source_in: start,
        duration: request.duration,
        rate,
        reverse: false,
        freeze: false,
        width: request.project.width,
        height: request.project.height,
        audio: conform::Audio::Resample,
        remap: None,
        working_transfer: None,
        lut: None,
    })
}
pub fn inspect(request: &Inspect) -> Result<Value> {
    request.project.validate()?;
    let root = media::input_root(&request.input_root)?;
    request.start.units(FPS)?;
    let frames = request.duration.units(FPS)?;
    if request.id.trim().is_empty()
        || request.id.len() > 100
        || request.project.frame_rate.compare(FPS)?.is_ne()
        || !(1..=1500).contains(&frames)
    {
        return Err(invalid(
            "Clock inspection requires an ID of 1..100 bytes and 1..1500 reference frames at 25 fps",
        ));
    }
    let reference = source(request, &request.reference_asset_id)?;
    let candidate = source(request, &request.candidate_asset_id)?;
    let (mapping, measurement) = match &request.method {
        Method::Timecode {
            reference: r,
            candidate: c,
        } => {
            if r.media_at.units(FPS)? >= reference.frames
                || c.media_at.units(FPS)? >= candidate.frames
            {
                return Err(invalid(
                    "Clock-label anchors must be inside their source media",
                ));
            }
            timecode_mapping(r, c)?
        }
        Method::Audio {
            windows,
            reference_channel,
            candidate_channel,
            polarity,
            minimum_correlation_milli,
            minimum_margin_milli,
            maximum_drift_ppm,
            maximum_residual_samples,
        } => audio_mapping(
            &reference,
            &candidate,
            windows,
            *reference_channel,
            *candidate_channel,
            *polarity,
            *minimum_correlation_milli,
            *minimum_margin_milli,
            *maximum_drift_ppm,
            *maximum_residual_samples,
        )?,
    };
    let ideal = mapping.at(request.start)?;
    let actual = snap(ideal, request.rounding)?;
    let reference_recipe = recipe(
        request,
        &reference,
        &root,
        request.start,
        Time::new(1, 1)?,
        "reference",
    )?;
    let candidate_recipe = recipe(
        request,
        &candidate,
        &root,
        actual,
        mapping.rate,
        "candidate",
    )?;
    let reference_plan = conform::inspect(&reference_recipe, &root)?;
    let candidate_plan = conform::inspect(&candidate_recipe, &root)?;
    for s in [&reference, &candidate] {
        if media::file_hash(&s.path)? != s.sha256 {
            return Err(error(
                "MEDIA_CHANGED",
                "Synchronization source changed during inspection",
            ));
        }
    }
    Ok(
        json!({"profile":"clock-alignment-v1","project_revision":request.project.revision,"id":request.id,"reference_asset_id":request.reference_asset_id,"candidate_asset_id":request.candidate_asset_id,"reference_source":reference,"candidate_source":candidate,"mapping":mapping,"measurement":measurement,"requested_reference_start":request.start,"ideal_candidate_start":ideal,"candidate_start":actual,"candidate_start_adjustment_samples":(actual.num as f64/actual.den as f64-ideal.num as f64/ideal.den as f64)*48000.0,"reference_recipe":reference_recipe,"candidate_recipe":candidate_recipe,"reference_plan":reference_plan,"candidate_plan":candidate_plan,"publication":"Run media.conform explicitly to new outputs; bind its returned assets before adding aligned camera angles","audio_policy":"linear_resampling;rate_changes_pitch","network":false}),
    )
}
pub fn capabilities() -> Value {
    json!({"command":"sync.inspect","profile":"clock-alignment-v1","source":"bound_25fps_reference_assets_with_stereo_PCM48k","maximum_source_seconds":60,"methods":["audio","timecode"],"audio_windows":[3,8],"maximum_drift_ppm":5000,"timecode":"explicit_day_25fps_non_drop","correction":"explicit_media_conform_recipes","writes_files":false,"network":false})
}
