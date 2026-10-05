//! Beat and onset detection for cutting to music: onsets from rises in 10 ms energy, the period
//! from the onset strength's autocorrelation, phase and exact period from beats tracked through
//! the whole range, the beat chosen among the metrical levels the onsets support, and a beat grid
//! moved onto the onsets that agree with it.
use crate::{Result, error, media, time::Time};
use serde_json::{Value, json};
use std::{io::Read, path::Path, process::Stdio};

/// Analysis hop: 10 ms at 48 kHz.
const HOP: usize = 480;
/// A rise of at least this many dB from one hop to the next is an onset candidate.
const RISE_DB: f64 = 6.0;
/// Onsets at least this many hops apart; a candidate must also be the largest rise within it.
const SPACING: usize = 5;
/// A rise counts at most this many dB more than the rise from two hops back, so the recovery from
/// a one-hop dip is no onset.
const DIP_DB: f64 = 3.0;
/// Beat tracking's cost of an interval, per squared natural log of its ratio to the period,
/// against onset strength in standard deviations.
const TIGHTNESS: f64 = 100.0;
/// Of the metrical levels the onsets support, the beat is the one nearest this tempo in octaves.
const PREFERRED_BPM: f64 = 120.0;
/// Longest analysed range: one hour.
const MAX_SECONDS: u64 = 3600;
/// Times listed per kind; counts stay exact.
const MAX_LISTED: usize = 2000;

/// What to analyse.
pub struct Request<'a> {
    pub path: &'a Path,
    pub input_root: &'a Path,
    pub start: Option<Time>,
    pub duration: Option<Time>,
    pub min_bpm: u32,
    pub max_bpm: u32,
    pub frame_rate: Option<Time>,
}

/// Mean power per hop of the first audio stream mixed to mono, in dB (floored at -100).
fn levels(path: &Path, start: Time, duration: Option<Time>) -> Result<Vec<f64>> {
    let mut args: Vec<String> = ["-nostdin", "-v", "error"].map(str::to_owned).to_vec();
    args.push("-i".into());
    args.push(path.to_string_lossy().into_owned());
    // Trim after resampling, on exact 48 kHz samples; `-ac 1` mixes the channels.
    let mut filter = format!(
        "aresample=48000,atrim=start_sample={}",
        start.units(Time::new(48_000, 1)?)?
    );
    if let Some(duration) = duration {
        filter.push_str(&format!(
            ":end_sample={}",
            start.plus(duration)?.units(Time::new(48_000, 1)?)?
        ));
    }
    args.extend(
        [
            "-map", "0:a:0", "-af", &filter, "-f", "s16le", "-ac", "1", "-",
        ]
        .map(str::to_owned),
    );
    let mut child = std::process::Command::new(media::tool("ffmpeg"))
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| error("TOOL_UNAVAILABLE", format!("ffmpeg: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut found = Vec::new();
    let mut buffer = vec![0u8; HOP * 2];
    let limit = (MAX_SECONDS * 100) as usize;
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = stdout.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled < buffer.len() {
            break;
        }
        let power: f64 = buffer
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                let x = f64::from(i16::from_le_bytes(*b)) / 32768.0;
                x * x
            })
            .sum::<f64>()
            / HOP as f64;
        found.push((10.0 * power.max(1e-10).log10()).max(-100.0));
        if found.len() > limit {
            let _ = child.kill();
            return Err(error(
                "LIMIT_EXCEEDED",
                format!("Beat analysis covers at most {MAX_SECONDS} s; select a range"),
            ));
        }
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(error(
            "TOOL_FAILED",
            "FFmpeg could not decode the file's first audio stream",
        ));
    }
    Ok(found)
}

/// Dynamic-programming beat tracking: the chain of hops that collects the most onset strength (in
/// standard deviations) while each interval costs `TIGHTNESS` × ln(interval / period)².
fn track(strength: &[f64], period: f64) -> Vec<usize> {
    let n = strength.len();
    let mean = strength.iter().sum::<f64>() / n as f64;
    let deviation = (strength.iter().map(|s| (s - mean).powi(2)).sum::<f64>() / n as f64).sqrt();
    let low = ((period / 2.0).round() as usize).max(1);
    let high = (period * 2.0).round() as usize;
    let cost: Vec<f64> = (0..=high)
        .map(|d| TIGHTNESS * (d as f64 / period).ln().powi(2))
        .collect();
    let mut score = vec![0.0; n];
    let mut previous = vec![usize::MAX; n];
    for t in 0..n {
        // A chain may also start here.
        let mut best = 0.0;
        for d in low..=high.min(t) {
            let value = score[t - d] - cost[d];
            if value > best {
                best = value;
                previous[t] = t - d;
            }
        }
        score[t] = strength[t] / deviation + best;
    }
    let tail = n.saturating_sub(period.round() as usize);
    let mut t = (tail..n).fold(tail, |b, k| if score[k] > score[b] { k } else { b });
    let mut beats = vec![t];
    while previous[t] != usize::MAX {
        t = previous[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Weighted least squares of runs of (beat index, hop, weight) with one shared slope and an
/// intercept per run: the period, and the hop of index zero in the run with the most weight (of
/// those with two points or more). Points more than 1.5 hops off their run's line are dropped and
/// the lines fitted again, at most twice.
fn fit(runs: &[Vec<(f64, f64, f64)>]) -> Option<(f64, f64)> {
    let mut kept: Vec<Vec<(f64, f64, f64)>> =
        runs.iter().filter(|r| !r.is_empty()).cloned().collect();
    let mut line = None;
    for _ in 0..3 {
        // Each run's weighted mean index and hop, its weight and its size.
        let means: Vec<(f64, f64, f64, usize)> = kept
            .iter()
            .map(|run| {
                let total: f64 = run.iter().map(|p| p.2).sum();
                let x = run.iter().map(|p| p.2 * p.0).sum::<f64>() / total;
                let y = run.iter().map(|p| p.2 * p.1).sum::<f64>() / total;
                (x, y, total, run.len())
            })
            .collect();
        let (mut cross, mut spread) = (0.0, 0.0);
        for (run, &(x, y, _, _)) in kept.iter().zip(&means) {
            for p in run {
                cross += p.2 * (p.0 - x) * (p.1 - y);
                spread += p.2 * (p.0 - x).powi(2);
            }
        }
        if spread <= 0.0 {
            break;
        }
        let slope = cross / spread;
        let heaviest = means.iter().filter(|m| m.3 >= 2).fold(
            None,
            |best: Option<&(f64, f64, f64, usize)>, m| match best {
                Some(b) if b.2 >= m.2 => best,
                _ => Some(m),
            },
        )?;
        line = Some((slope, heaviest.1 - slope * heaviest.0));
        let close: Vec<Vec<(f64, f64, f64)>> = kept
            .iter()
            .zip(&means)
            .map(|(run, &(x, y, _, _))| {
                run.iter()
                    .copied()
                    .filter(|p| (p.1 - y - slope * (p.0 - x)).abs() <= 1.5)
                    .collect::<Vec<_>>()
            })
            .filter(|run| !run.is_empty())
            .collect();
        let count = |runs: &[Vec<(f64, f64, f64)>]| runs.iter().map(Vec::len).sum::<usize>();
        if count(&close) == count(&kept) || count(&close) < 4 {
            break;
        }
        kept = close;
    }
    line
}

/// Tempo to two decimals.
fn bpm(period: f64) -> f64 {
    (6000.0 / period * 100.0).round() / 100.0
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Analyse a file's music: onsets, tempo and a beat grid, with exact sample times.
pub fn analyse(request: &Request) -> Result<Value> {
    if !(30..=300).contains(&request.min_bpm)
        || !(30..=300).contains(&request.max_bpm)
        || request.max_bpm < request.min_bpm * 3 / 2
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "min_bpm and max_bpm must be 30-300, with max_bpm at least 1.5 times min_bpm",
        ));
    }
    let path = media::allowed_file(request.path, request.input_root)?;
    let start = request.start.unwrap_or(Time::ZERO);
    let rate = match request.frame_rate {
        Some(rate) => Some(crate::render::clock::rate(rate)?),
        None => None,
    };
    let db = levels(&path, start, request.duration)?;
    let n = db.len();
    // Rise: the increase in level from the previous hop, never negative, and at most `DIP_DB` more
    // than from the hop before that, so a one-hop dip (a low note beating against the 10 ms
    // window) is no onset. Onsets are rises of at least 6 dB that are the largest within 50 ms
    // either side.
    let rise: Vec<f64> = (0..n)
        .map(|k| match k {
            0 => 0.0,
            1 => (db[1] - db[0]).max(0.0),
            _ => (db[k] - db[k - 1]).min(db[k] - db[k - 2] + DIP_DB).max(0.0),
        })
        .collect();
    let mut onsets: Vec<usize> = Vec::new();
    for k in 1..n {
        let low = k.saturating_sub(SPACING - 1);
        let high = (k + SPACING).min(n);
        let peak = rise[low..high].iter().all(|&s| s <= rise[k]);
        if rise[k] >= RISE_DB && peak && onsets.last().is_none_or(|&o| k - o >= SPACING) {
            onsets.push(k);
        }
    }
    // Strength, for tempo and phase: the rise weighted by the hop's amplitude, so loud hits (kicks,
    // accents) count more than quiet ones (hi-hats) that rise as steeply from silence.
    let strength: Vec<f64> = (0..n).map(|k| rise[k] * 10f64.powf(db[k] / 20.0)).collect();
    let sample_time = |hop: f64| -> Result<Time> {
        start.plus(Time::new((hop * HOP as f64).round() as u64, 48_000)?)
    };
    let mut result = json!({"start":start,"hops":n,"hop_seconds":{"num":1,"den":100},
        "onsets":{"count":onsets.len(),"times":onsets.iter().take(MAX_LISTED).map(|&k| sample_time(k as f64)).collect::<Result<Vec<_>>>()?}});
    if onsets.len() < 4 {
        result["tempo_bpm"] = Value::Null;
        result["tempo_alternatives"] = json!([]);
        result["beats"] = json!({"count":0,"on_onsets":0,"times":[]});
        result["note"] = json!("fewer than four onsets; no tempo");
        return Ok(result);
    }
    // The onset nearest a hop position, if one is within `within` hops.
    let nearest = |x: f64, within: f64| -> Option<usize> {
        let i = onsets.partition_point(|&o| (o as f64) < x);
        [i.checked_sub(1), Some(i)]
            .into_iter()
            .flatten()
            .filter_map(|j| onsets.get(j).copied())
            .filter(|&o| (o as f64 - x).abs() <= within)
            .min_by(|a, b| (*a as f64 - x).abs().total_cmp(&(*b as f64 - x).abs()))
    };
    // Strength at a hop position: the largest within one hop either side.
    let peak = |x: f64| -> f64 {
        let k = x.round().max(0.0) as usize;
        strength[k.saturating_sub(1)..(k + 2).min(n)]
            .iter()
            .fold(0.0f64, |a, &b| a.max(b))
    };
    // Mean strength at every `step`th position from `first`.
    let mean_peak = |xs: &[f64], first: usize, step: usize| -> f64 {
        let picked: Vec<f64> = xs
            .iter()
            .skip(first)
            .step_by(step)
            .map(|&x| peak(x))
            .collect();
        picked.iter().sum::<f64>() / picked.len().max(1) as f64
    };
    // Of each group of `size` positions, the one with the most mean strength.
    let accented = |xs: &[f64], size: usize| -> usize {
        (0..size)
            .map(|o| (o, mean_peak(xs, o, size)))
            .fold((0, -1.0), |b, item| if item.1 > b.1 { item } else { b })
            .0
    };
    // Positions origin + k × period from the first at or after half a hop before the start.
    let grid = |period: f64, origin: f64| -> Vec<f64> {
        let first = origin - ((origin + 0.5) / period).floor() * period;
        (0..)
            .map(|k| first + k as f64 * period)
            .take_while(|&x| x < n as f64 - 0.5)
            .collect()
    };
    // Periodicity: the autocorrelation of onset strength, strongest lag within the BPM range,
    // refined between hops by a parabola through its neighbours. Each hop pairs with the strongest
    // of the three hops around the lag, so a period between whole hops is not split across two
    // lags. Repeating music correlates at every metrical level, so this finds one of them.
    let shortest = (6000.0 / request.max_bpm as f64).floor().max(2.0) as usize;
    let longest = (6000.0 / request.min_bpm as f64).ceil() as usize;
    let correlation = |lag: usize| -> f64 {
        (0..n)
            .map(|k| {
                let partner = (k + lag - 1..=k + lag + 1)
                    .filter_map(|j| strength.get(j))
                    .fold(0.0f64, |a, &b| a.max(b));
                strength[k] * partner
            })
            .sum()
    };
    let scores: Vec<(usize, f64)> = (shortest - 1..=longest + 1)
        .map(|lag| (lag, correlation(lag)))
        .collect();
    let (index, &(lag, best)) = scores[1..scores.len() - 1]
        .iter()
        .enumerate()
        .fold(
            None,
            |found: Option<(usize, &(usize, f64))>, (i, item)| match found {
                Some((_, current)) if current.1 >= item.1 => found,
                _ => Some((i, item)),
            },
        )
        .expect("lags");
    let (before, after) = (scores[index].1, scores[index + 2].1);
    let curvature = before - 2.0 * best + after;
    let rough = if curvature < 0.0 {
        lag as f64 + 0.5 * (before - after) / curvature
    } else {
        lag as f64
    };
    // Phase and exact period: track beats at that level through the whole range. Tracking may
    // move to another position of the bar, so it is cut into runs at intervals more than 10% off
    // the period; the tracked beats on onsets fit one period with a phase per run, and the run
    // with the most onset strength gives the phase.
    let tracked = track(&strength, rough);
    let mut runs: Vec<Vec<(f64, f64, f64)>> = vec![Vec::new()];
    let mut number = 0.0;
    for (i, &hop) in tracked.iter().enumerate() {
        if i > 0 && ((hop - tracked[i - 1]) as f64 - rough).abs() > 0.1 * rough {
            runs.push(Vec::new());
            number = 0.0;
        } else if i > 0 {
            number += 1.0;
        }
        if let Some(onset) = nearest(hop as f64, 1.0) {
            runs.last_mut()
                .expect("a run")
                .push((number, onset as f64, strength[onset]));
        }
    }
    let (period, origin) = match fit(&runs) {
        Some((period, origin)) if (period - rough).abs() < 0.1 * rough => (period, origin),
        _ => (rough, tracked[0] as f64),
    };
    // Metrical levels in the BPM range, as multiples num/den of that period. A faster level divides
    // each period in two or three where onsets fall (at least half as often as on the period's
    // own positions), the division with the stronger new positions; a slower level groups two or
    // three periods when one position of the group is accented, with 1.5 times the mean strength
    // of the others.
    let in_range =
        |period: f64| (request.min_bpm as f64..=request.max_bpm as f64).contains(&bpm(period));
    let presence = |xs: &[f64]| -> f64 {
        xs.iter().filter(|&&x| nearest(x, 2.0).is_some()).count() as f64 / xs.len().max(1) as f64
    };
    let own = presence(&grid(period, origin));
    let mut found = vec![(1u64, 1u64)];
    let mut den = 1;
    loop {
        let current = period / den as f64;
        let mut division: Option<(u64, f64)> = None;
        for parts in [2u64, 3] {
            if !in_range(current / parts as f64) {
                continue;
            }
            // The finer grid's positions that are not on the current one.
            let fine = current / parts as f64;
            let before = ((origin + 0.5) / fine).floor() as i64;
            let new: Vec<f64> = grid(fine, origin)
                .into_iter()
                .enumerate()
                .filter(|(i, _)| (*i as i64 - before).rem_euclid(parts as i64) != 0)
                .map(|(_, x)| x)
                .collect();
            let share = presence(&new);
            if share > 0.0 && share >= 0.5 * own {
                let salience = mean_peak(&new, 0, 1);
                if division.is_none_or(|d| salience > d.1) {
                    division = Some((parts, salience));
                }
            }
        }
        let Some((parts, _)) = division else { break };
        den *= parts;
        found.push((1, den));
    }
    let mut num = 1;
    loop {
        let current = period * num as f64;
        let positions = grid(current, origin);
        let mut group: Option<(u64, f64)> = None;
        for size in [2u64, 3] {
            if !in_range(current * size as f64) {
                continue;
            }
            let means: Vec<f64> = (0..size as usize)
                .map(|o| mean_peak(&positions, o, size as usize))
                .collect();
            let top = means.iter().fold(0.0f64, |a, &b| a.max(b));
            let rest = (means.iter().sum::<f64>() - top) / (size - 1) as f64;
            if top > 0.0 && top >= 1.5 * rest {
                let accent = top / rest.max(1e-9);
                if group.is_none_or(|g| accent > g.1) {
                    group = Some((size, accent));
                }
            }
        }
        let Some((size, _)) = group else { break };
        num *= size;
        found.push((num, 1));
    }
    // The beat: the level nearest `PREFERRED_BPM` in octaves. A slower level starts on its accented
    // position.
    let level_bpm = |(num, den): (u64, u64)| bpm(period * num as f64 / den as f64);
    let chosen = found
        .iter()
        .copied()
        .min_by(|&a, &b| {
            (level_bpm(a) / PREFERRED_BPM)
                .log2()
                .abs()
                .total_cmp(&(level_bpm(b) / PREFERRED_BPM).log2().abs())
        })
        .expect("levels");
    let mut beat_period = period * chosen.0 as f64 / chosen.1 as f64;
    let mut beat_origin = origin;
    if chosen.0 > 1 {
        let positions = grid(period, origin);
        let offset = accented(&positions, chosen.0 as usize);
        beat_origin = positions.get(offset).copied().unwrap_or(origin);
    }
    // Beats on the grid, each moved to an onset within 3 hops when there is one; then the line
    // through the beats on onsets refines the period and the grid is placed again.
    let place = |origin: f64, period: f64| -> Vec<(f64, bool)> {
        grid(period, origin)
            .into_iter()
            .map(|x| match nearest(x, 3.0) {
                Some(onset) => (onset as f64, true),
                None => (x.max(0.0), false),
            })
            .collect()
    };
    let mut beats = place(beat_origin, beat_period);
    let landed: Vec<(f64, f64, f64)> = beats
        .iter()
        .enumerate()
        .filter(|(_, (_, on))| *on)
        .map(|(k, (hop, _))| (k as f64, *hop, 1.0))
        .collect();
    if landed.len() >= 4
        && let Some((slope, origin)) = fit(&[landed])
        && (slope - beat_period).abs() < 0.5
    {
        beat_period = slope;
        beats = place(origin, slope);
    }
    let on_onsets = beats.iter().filter(|(_, on)| *on).count();
    let beats: Vec<f64> = beats.into_iter().map(|(hop, _)| hop).collect();
    // The other levels, slowest first, with their tempo as a fraction of the beat's; a slower
    // level's first_beat is the beat its accented position falls on.
    let mut alternatives: Vec<((u64, u64), Value)> = found
        .iter()
        .copied()
        .filter(|&level| level != chosen)
        .map(|(num, den)| {
            let (top, bottom) = (chosen.0 * den, chosen.1 * num);
            let common = gcd(top, bottom);
            let (top, bottom) = (top / common, bottom / common);
            let mut item = json!({"bpm":bpm(beat_period * bottom as f64 / top as f64),"ratio":{"num":top,"den":bottom}});
            if top == 1 {
                item["first_beat"] = json!(accented(&beats, bottom as usize));
            }
            ((top, bottom), item)
        })
        .collect();
    alternatives.sort_by(|((a, b), _), ((c, d), _)| (a * d).cmp(&(c * b)));
    result["tempo_bpm"] = json!(bpm(beat_period));
    result["period"] = json!(Time::new(
        (beat_period * HOP as f64).round() as u64,
        48_000
    )?);
    result["tempo_alternatives"] = Value::Array(alternatives.into_iter().map(|(_, v)| v).collect());
    let times = beats
        .iter()
        .take(MAX_LISTED)
        .map(|&b| sample_time(b))
        .collect::<Result<Vec<_>>>()?;
    result["beats"] = json!({"count":beats.len(),"on_onsets":on_onsets,"times":times});
    if let Some(rate) = rate {
        // Nearest frame start of each listed beat, for placing cuts.
        let frames: Vec<u64> = times
            .iter()
            .map(|t| {
                let n = t.num as u128 * rate.num as u128;
                let d = t.den as u128 * rate.den as u128;
                ((2 * n + d) / (2 * d)) as u64
            })
            .collect();
        result["beat_frames"] = json!({"frame_rate":rate,"frames":frames});
    }
    Ok(result)
}
