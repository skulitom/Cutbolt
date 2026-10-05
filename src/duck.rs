//! Ducking: lower music under speech by proposing gain curves for the music track's clips.
use crate::{
    Result,
    animation::{Curve, Interpolation, Keyframe},
    error, media,
    model::Project,
    render,
    time::Time,
    tracks::{Kind, MAX_GAIN_KEYS},
};
use serde_json::{Value, json};
use std::{
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Analysis window: 10 ms at 48 kHz.
const WINDOW: u64 = 480;
/// Speech runs listed in the result; the count stays exact.
const MAX_RUNS: usize = 50;

/// How speech lowers the music.
pub struct Settings {
    pub threshold_db: i32,
    pub duck_milli: u32,
    pub attack: Time,
    pub release: Time,
    pub bridge: Time,
}

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Speech regions, `[start, end)` in timeline samples on the 10 ms grid: of the `voice` audio
/// track played alone, or of the whole program when `voice` is `None`. A window is speech when its
/// mean power reaches `threshold_db`; pauses shorter than `bridge` are merged into speech.
pub(crate) fn speech(
    project: &Project,
    input_root: &Path,
    voice: Option<&str>,
    threshold_db: i32,
    bridge: Time,
) -> Result<Vec<(u64, u64)>> {
    let mut solo = project.clone();
    if let (Some(voice), Some(arrangement)) = (voice, solo.tracks.as_mut()) {
        for track in &mut arrangement.tracks {
            if track.kind == Kind::Audio {
                track.enabled = track.id == voice;
            }
        }
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
        .as_nanos();
    let scratch =
        Scratch(std::env::temp_dir().join(format!("cutbolt-duck-{}-{nonce}", std::process::id())));
    fs::create_dir(&scratch.0)?;
    let plan = render::plan_audio_range(
        &solo,
        input_root,
        &scratch.0,
        &scratch.0.join("voice.wav"),
        Time::ZERO,
        project.duration()?,
    )?;
    render::run_plan(
        &plan,
        &plan.output,
        Duration::from_secs(1800),
        false,
        &media::Uncontrolled,
    )?;
    let mut input = BufReader::new(File::open(&plan.output)?);
    // Walk the RIFF chunks to the PCM data.
    let mut header = [0u8; 12];
    input.read_exact(&mut header)?;
    loop {
        let mut chunk = [0u8; 8];
        input.read_exact(&mut chunk)?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().expect("chunk size")) as u64;
        if &chunk[..4] == b"data" {
            break;
        }
        std::io::copy(
            &mut (&mut input).take(size + size % 2),
            &mut std::io::sink(),
        )?;
    }
    // A window is speech when its mean power reaches the threshold: sum(L^2 + R^2) / (2 x 480)
    // against 32768^2 x 10^(threshold / 10).
    let limit = 2.0 * WINDOW as f64 * 32768.0 * 32768.0 * 10f64.powf(threshold_db as f64 / 10.0);
    let mut regions: Vec<(u64, u64)> = Vec::new();
    let mut buffer = vec![0u8; WINDOW as usize * 4];
    let mut at = 0u64;
    loop {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = input.read(&mut buffer[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        if filled == 0 {
            break;
        }
        let energy: f64 = buffer[..filled]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| {
                let x = f64::from(i16::from_le_bytes(*b));
                x * x
            })
            .sum();
        let length = filled as u64 / 4;
        if energy >= limit {
            match regions.last_mut() {
                Some(last) if last.1 == at => last.1 = at + length,
                _ => regions.push((at, at + length)),
            }
        }
        at += length;
        if filled < buffer.len() {
            break;
        }
    }
    // Pauses shorter than the bridge stay ducked.
    let bridge = bridge.units(Time::new(48_000, 1)?)?;
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for region in regions {
        match merged.last_mut() {
            Some(last) if region.0 - last.1 < bridge => last.1 = region.1,
            _ => merged.push(region),
        }
    }
    Ok(merged)
}

/// Propose gain curves that lower each enabled clip on `music` while `voice` carries speech.
pub fn propose(
    project: &Project,
    input_root: &Path,
    voice: &str,
    music: &str,
    settings: &Settings,
) -> Result<Value> {
    project.validate()?;
    let arrangement = project.tracks.as_ref().ok_or_else(|| {
        error(
            "UNSUPPORTED_TIMELINE",
            "Ducking needs placed tracks; promote the sequential timeline first",
        )
    })?;
    let track = |id: &str| {
        arrangement
            .tracks
            .iter()
            .find(|t| t.id == id && t.kind == Kind::Audio)
            .ok_or_else(|| {
                crate::missing(
                    "MISSING_TRACK",
                    "audio track",
                    id,
                    arrangement
                        .tracks
                        .iter()
                        .filter(|t| t.kind == Kind::Audio)
                        .map(|t| t.id.as_str()),
                )
            })
    };
    let (_, music_track) = (track(voice)?, track(music)?);
    if voice == music {
        return Err(error(
            "INVALID_ARGUMENT",
            "voice and music must be different tracks",
        ));
    }
    if !(-80..=0).contains(&settings.threshold_db) || settings.duck_milli > 1000 {
        return Err(error(
            "INVALID_ARGUMENT",
            "threshold_db must be -80..0 and duck_milli 0..1000",
        ));
    }
    let samples = Time::new(48_000, 1)?;
    let (attack, release) = (
        settings.attack.units(samples)?,
        settings.release.units(samples)?,
    );
    if attack > 480_000 || release > 480_000 {
        return Err(error(
            "INVALID_ARGUMENT",
            "attack and release last at most 10 s",
        ));
    }
    let regions = speech(
        project,
        input_root,
        Some(voice),
        settings.threshold_db,
        settings.bridge,
    )?;
    let mut operations = Vec::new();
    let mut clips = Vec::new();
    for clip in music_track.clips.iter() {
        let start = clip.start.units(samples)?;
        let end = clip.end()?.units(samples)?;
        let source_end = clip.source_duration(project)?;
        let gain = clip.gain_milli;
        let ducked = (gain * settings.duck_milli + 500) / 1000;
        // Ramped spans [down, low, high, up] in timeline samples, merged where they meet.
        let mut spans: Vec<[i64; 4]> = Vec::new();
        for &(s, e) in &regions {
            let span = [
                s as i64 - attack as i64,
                s as i64,
                e as i64,
                (e + release) as i64,
            ];
            if span[3] <= start as i64 || span[0] >= end as i64 {
                continue;
            }
            match spans.last_mut() {
                Some(last) if span[0] <= last[3] => {
                    last[2] = span[2];
                    last[3] = span[3];
                }
                _ => spans.push(span),
            }
        }
        if spans.is_empty() {
            continue;
        }
        let mut keys = Vec::new();
        let mut key = |t: i64, value: u32, interpolation: Interpolation| -> Result<()> {
            // Key times are source positions; ramps that would start before the source are cut.
            let offset = t - start as i64;
            let time = if offset >= 0 {
                clip.source_in.plus(Time::new(offset as u64, 48_000)?)?
            } else if clip
                .source_in
                .compare(Time::new(offset.unsigned_abs(), 48_000)?)?
                .is_lt()
            {
                return Ok(());
            } else {
                clip.source_in
                    .minus(Time::new(offset.unsigned_abs(), 48_000)?)?
            };
            if time.compare(source_end)?.is_gt() {
                return Ok(());
            }
            keys.push(Keyframe {
                time,
                value: value as i32,
                interpolation,
            });
            Ok(())
        };
        for [down, low, high, up] in &spans {
            if down < low {
                key(*down, gain, Interpolation::Linear)?;
            }
            key(*low, ducked, Interpolation::Hold)?;
            if high < up {
                key(*high, ducked, Interpolation::Linear)?;
                key(*up, gain, Interpolation::Hold)?;
            } else {
                key(*high, gain, Interpolation::Hold)?;
            }
        }
        if keys.is_empty() {
            continue;
        }
        if keys.len() > MAX_GAIN_KEYS {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "Ducking clip {:?} needs {} keys, more than {MAX_GAIN_KEYS}; raise bridge to merge short pauses",
                    clip.id,
                    keys.len()
                ),
            ));
        }
        let curve = Curve { keys, retime: None };
        clips.push(json!({"clip_id":clip.id,"keys":curve.keys.len(),"ducks":spans.len(),"gain_milli":gain,"ducked_milli":ducked}));
        operations.push(json!({"op":"tracks.edit","edit":{"op":"clip_audio","clip_ids":[clip.id],"gain_curve":curve}}));
    }
    // Exact sample times, reduced like every other engine time.
    let time = |n: u64| json!(Time::new(n, 48_000).expect("sample time"));
    let seconds: u64 = regions.iter().map(|(s, e)| e - s).sum();
    Ok(json!({"voice_track_id":voice,"music_track_id":music,
        "settings":{"threshold_db":settings.threshold_db,"duck_milli":settings.duck_milli,"attack":settings.attack,"release":settings.release,"bridge":settings.bridge,"window_seconds":{"num":1,"den":100}},
        "speech":{"regions":regions.len(),"duration":time(seconds),
            "runs":regions.iter().take(MAX_RUNS).map(|&(s, e)| json!({"start":time(s),"end":time(e)})).collect::<Vec<_>>()},
        "clips":clips,"operations":operations,
        "next":"apply operations with session.apply (or check them with session.preview), then listen back with timeline.meters curve or a preview range"}))
}
