//! Beat and onset detection for cutting to music: onsets from rises in 10 ms energy, the tempo
//! from the onset strength's autocorrelation, and a beat grid placed where onsets agree with it.
use crate::{Result, error, media, time::Time};
use serde_json::{Value, json};
use std::{io::Read, path::Path, process::Stdio};

/// Analysis hop: 10 ms at 48 kHz.
const HOP: usize = 480;
/// A rise of at least this many dB from one hop to the next is an onset candidate.
const RISE_DB: f64 = 6.0;
/// Onsets at least this many hops apart; a candidate must also be the largest rise within it.
const SPACING: usize = 5;
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
    // Rise: the increase in level from the previous hop, never negative. Onsets are rises of at
    // least 6 dB that are the largest within 50 ms either side.
    let rise: Vec<f64> = (0..db.len())
        .map(|k| {
            if k == 0 {
                0.0
            } else {
                (db[k] - db[k - 1]).max(0.0)
            }
        })
        .collect();
    let mut onsets: Vec<usize> = Vec::new();
    for k in 1..rise.len() {
        let low = k.saturating_sub(SPACING - 1);
        let high = (k + SPACING).min(rise.len());
        let peak = rise[low..high].iter().all(|&s| s <= rise[k]);
        if rise[k] >= RISE_DB && peak && onsets.last().is_none_or(|&o| k - o >= SPACING) {
            onsets.push(k);
        }
    }
    // Strength, for tempo and phase: the rise weighted by the hop's amplitude, so loud hits (kicks,
    // accents) count more than quiet ones (hi-hats) that rise as steeply from silence.
    let strength: Vec<f64> = (0..db.len())
        .map(|k| rise[k] * 10f64.powf(db[k] / 20.0))
        .collect();
    let sample_time = |hop: f64| -> Result<Time> {
        start.plus(Time::new((hop * HOP as f64).round() as u64, 48_000)?)
    };
    let mut result = json!({"start":start,"hops":db.len(),"hop_seconds":{"num":1,"den":100},
        "onsets":{"count":onsets.len(),"times":onsets.iter().take(MAX_LISTED).map(|&k| sample_time(k as f64)).collect::<Result<Vec<_>>>()?}});
    if onsets.len() < 4 {
        result["tempo_bpm"] = Value::Null;
        result["beats"] = json!({"count":0,"times":[]});
        result["note"] = json!("fewer than four onsets; no tempo");
        return Ok(result);
    }
    // Tempo: the autocorrelation of onset strength, strongest lag within the BPM range, refined
    // between hops by a parabola through its neighbours. Each hop pairs with the strongest of the
    // three hops around the lag, so a period between whole hops is not split across two lags.
    let shortest = (6000.0 / request.max_bpm as f64).floor().max(2.0) as usize;
    let longest = (6000.0 / request.min_bpm as f64).ceil() as usize;
    let correlation = |lag: usize| -> f64 {
        (0..strength.len())
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
    let period = if curvature < 0.0 {
        lag as f64 + 0.5 * (before - after) / curvature
    } else {
        lag as f64
    };
    // Phase: the grid offset (in whole hops) whose positions collect the most onset strength.
    let phase = (0..period.ceil() as usize)
        .map(|offset| {
            let mut total = 0.0;
            let mut n = 0.0;
            loop {
                let at = (offset as f64 + n * period).round() as usize;
                if at >= strength.len() {
                    break;
                }
                total += strength[at];
                n += 1.0;
            }
            (offset, total)
        })
        .fold(
            (0, f64::MIN),
            |best, item| if item.1 > best.1 { item } else { best },
        )
        .0;
    // Beats on the grid, each moved to an onset within 3 hops when there is one.
    let place = |origin: f64, period: f64| -> Vec<(f64, bool)> {
        let mut beats = Vec::new();
        let mut n = 0.0;
        loop {
            let grid = origin + n * period;
            if grid < -0.5 {
                n += 1.0;
                continue;
            }
            if grid.round() as usize >= strength.len() {
                break;
            }
            let nearest = onsets
                .iter()
                .copied()
                .filter(|&o| (o as f64 - grid).abs() <= 3.0)
                .min_by(|a, b| {
                    (*a as f64 - grid)
                        .abs()
                        .total_cmp(&(*b as f64 - grid).abs())
                });
            beats.push(match nearest {
                Some(onset) => (onset as f64, true),
                None => (grid.max(0.0), false),
            });
            n += 1.0;
        }
        beats
    };
    let mut beats = place(phase as f64, period);
    // Refine the grid by least squares through the beats that landed on onsets: the hop grid
    // limits the autocorrelation's precision, while many onsets average it out.
    let landed: Vec<(f64, f64)> = beats
        .iter()
        .enumerate()
        .filter(|(_, (_, on))| *on)
        .map(|(n, (hop, _))| (n as f64, *hop))
        .collect();
    let mut period = period;
    if landed.len() >= 4 {
        let count = landed.len() as f64;
        let (mean_n, mean_h) = (
            landed.iter().map(|p| p.0).sum::<f64>() / count,
            landed.iter().map(|p| p.1).sum::<f64>() / count,
        );
        let spread: f64 = landed.iter().map(|p| (p.0 - mean_n).powi(2)).sum();
        let slope = landed
            .iter()
            .map(|p| (p.0 - mean_n) * (p.1 - mean_h))
            .sum::<f64>()
            / spread;
        if slope > 0.0 && (slope - period).abs() < 1.0 {
            period = slope;
            beats = place(mean_h - slope * mean_n, period);
        }
    }
    let bpm = (6000.0 / period * 100.0).round() / 100.0;
    let beats: Vec<f64> = beats.into_iter().map(|(hop, _)| hop).collect();
    result["tempo_bpm"] = json!(bpm);
    result["period"] = json!(Time::new((period * HOP as f64).round() as u64, 48_000)?);
    let times = beats
        .iter()
        .take(MAX_LISTED)
        .map(|&b| sample_time(b))
        .collect::<Result<Vec<_>>>()?;
    result["beats"] = json!({"count":beats.len(),"times":times});
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
