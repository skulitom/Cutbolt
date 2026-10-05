//! Pause tightening (jump cuts): find the pauses in speech and propose ripple deletions that remove
//! each pause's middle, keeping some silence on either side. Every proposed cut is applied to a
//! working copy first, so the batch is known to apply; cuts the editor would refuse are listed
//! with the reason instead. Nothing changes until the operations are applied.
use crate::{
    Result, error,
    model::{Operation, Project},
    time::Time,
    tracks::Kind,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

/// Pauses listed in the result; the count stays exact.
const MAX_LISTED: usize = 200;

/// How pauses are found and cut.
pub struct Settings {
    /// Mean power, in dBFS over 10 ms windows, that counts as speech.
    pub threshold_db: i32,
    /// Shortest silence that counts as a pause.
    pub min_pause: Time,
    /// Silence kept on each side of a cut.
    pub keep: Time,
    /// Also cut silence before the first and after the last speech.
    pub edges: bool,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// The smallest unused ID of the form `<base>-j<n>`.
fn fresh(base: &str, used: &mut BTreeSet<String>) -> String {
    let mut n = 1;
    loop {
        let id = format!("{base}-j{n}");
        if used.insert(id.clone()) {
            return id;
        }
        n += 1;
    }
}

fn all_ids(project: &Project) -> BTreeSet<String> {
    let mut used: BTreeSet<String> = project.clips.iter().map(|c| c.id.clone()).collect();
    if let Some(arrangement) = &project.tracks {
        for track in &arrangement.tracks {
            used.extend(track.clips.iter().map(|c| c.id.clone()));
        }
        used.extend(arrangement.links.iter().map(|l| l.id.clone()));
    }
    used
}

/// The ripple deletion of `[start, end)` on `project`, with IDs for the right parts of split clips
/// and links.
fn ripple(project: &Project, start: Time, end: Time) -> Result<Value> {
    let duration = end.minus(start)?;
    let mut used = all_ids(project);
    let spans = |clip_start: Time, clip_end: Time| -> Result<bool> {
        Ok(clip_start.compare(start)?.is_lt() && clip_end.compare(end)?.is_gt())
    };
    match &project.tracks {
        Some(arrangement) => {
            let mut right_clips = Vec::new();
            let mut split = BTreeSet::new();
            for track in &arrangement.tracks {
                for clip in &track.clips {
                    if spans(clip.start, clip.end()?)? {
                        split.insert(clip.id.clone());
                        right_clips.push(json!({"id":clip.id,"new_id":fresh(&clip.id, &mut used)}));
                    }
                }
            }
            let mut right_links = Vec::new();
            for link in &arrangement.links {
                if link.members.iter().all(|m| split.contains(&m.clip_id)) {
                    right_links.push(json!({"id":link.id,"new_id":fresh(&link.id, &mut used)}));
                }
            }
            let tracks: Vec<&str> = arrangement.tracks.iter().map(|t| t.id.as_str()).collect();
            Ok(
                json!({"op":"tracks.edit","edit":{"op":"ripple_delete","track_ids":tracks,"start":start,
                "duration":duration,"links":"include","right_clip_ids":right_clips,"right_link_ids":right_links,
                "end_policy":"resize","transitions":"reject_affected"}}),
            )
        }
        None => {
            let mut at = Time::ZERO;
            let mut right = None;
            for clip in &project.clips {
                let clip_end = at.plus(clip.duration)?;
                if spans(at, clip_end)? {
                    right = Some(fresh(&clip.id, &mut used));
                }
                at = clip_end;
            }
            let mut op = json!({"op":"timeline.ripple_delete","start":start,"duration":duration});
            if let Some(id) = right {
                op["right_id"] = json!(id);
            }
            Ok(op)
        }
    }
}

/// Propose ripple deletions that shorten the pauses of `voice` (or of the whole program on a
/// sequential timeline).
pub fn propose(
    project: &Project,
    input_root: &Path,
    voice: Option<&str>,
    settings: &Settings,
) -> Result<Value> {
    project.validate()?;
    let rate = crate::render::clock::rate(project.frame_rate)?;
    let between = |t: Time, low: Time, high: Time| -> Result<bool> {
        Ok(!t.compare(low)?.is_lt() && !t.compare(high)?.is_gt())
    };
    if !(-80..=0).contains(&settings.threshold_db)
        || !between(settings.min_pause, Time::new(1, 5)?, Time::new(10, 1)?)?
        || !settings
            .keep
            .plus(settings.keep)?
            .compare(settings.min_pause)?
            .is_lt()
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "threshold_db must be -80..0, min_pause 0.2-10 s, and twice keep shorter than min_pause",
        ));
    }
    match (&project.tracks, voice) {
        (Some(arrangement), Some(voice)) => {
            if !arrangement
                .tracks
                .iter()
                .any(|t| t.id == voice && t.kind == Kind::Audio)
            {
                return Err(crate::missing(
                    "MISSING_TRACK",
                    "audio track",
                    voice,
                    arrangement
                        .tracks
                        .iter()
                        .filter(|t| t.kind == Kind::Audio)
                        .map(|t| t.id.as_str()),
                ));
            }
            let locked: Vec<&str> = arrangement
                .tracks
                .iter()
                .filter(|t| t.locked)
                .map(|t| t.id.as_str())
                .collect();
            if !locked.is_empty() {
                return Err(error(
                    "TRACK_LOCKED",
                    format!(
                        "Tightening ripples every track to keep them in sync; unlock {}",
                        locked.join(", ")
                    ),
                ));
            }
        }
        (Some(_), None) => {
            return Err(error(
                "INVALID_ARGUMENT",
                "voice_track_id is required for placed tracks",
            ));
        }
        (None, Some(_)) => {
            return Err(error(
                "INVALID_ARGUMENT",
                "A sequential timeline is analysed whole; omit voice_track_id",
            ));
        }
        (None, None) => {}
    }
    let samples = Time::new(48_000, 1)?;
    let minimum = settings.min_pause.units(samples)?;
    let keep = settings.keep.units(samples)?;
    let duration = project.duration()?;
    let total = (duration.num as u128 * 48_000 / duration.den as u128) as u64;
    let regions = crate::duck::speech(
        project,
        input_root,
        voice,
        settings.threshold_db,
        settings.min_pause,
    )?;
    // Pauses in samples, with what each keeps: (start, end, kind).
    let mut pauses: Vec<(u64, u64, &str)> = Vec::new();
    if let (Some(first), Some(last)) = (regions.first(), regions.last()) {
        if settings.edges && first.0 >= minimum {
            pauses.push((0, first.0, "leading"));
        }
        for pair in regions.windows(2) {
            pauses.push((pair[0].1, pair[1].0, "between"));
        }
        if settings.edges && total.saturating_sub(last.1) >= minimum {
            pauses.push((last.1, total, "trailing"));
        }
    }
    // Cuts sit on a grid of whole frames that are also whole samples on placed tracks.
    let step = match project.tracks {
        Some(_) => rate.num / gcd(rate.num, 48_000 * rate.den),
        None => 1,
    };
    let grid = |sample: u64, up: bool| -> u64 {
        // Grid index of the time `sample / 48000`, rounded up or down.
        let n = sample as u128 * rate.num as u128;
        let d = 48_000u128 * rate.den as u128 * step as u128;
        (if up { n.div_ceil(d) } else { n / d }) as u64
    };
    let end_index = duration.units(rate)? / step;
    let at = |index: u64| Time::new(index * step * rate.den, rate.num);
    let mut listed: Vec<Value> = Vec::new();
    let mut cuts: Vec<(usize, Time, Time)> = Vec::new();
    for (index, &(a, b, kind)) in pauses.iter().enumerate() {
        let first = if kind == "leading" {
            0
        } else {
            grid(a + keep, true)
        };
        let last = if kind == "trailing" {
            end_index
        } else {
            grid(b.saturating_sub(keep), false)
        };
        listed.push(json!({"start":Time::new(a, 48_000)?,"end":Time::new(b, 48_000)?,"kind":kind}));
        if last > first {
            cuts.push((index, at(first)?, at(last)?));
        } else {
            listed[index]["skipped"] = json!("shorter than the cut grid after keeping silence");
        }
    }
    // Apply from the latest cut back, so every cut's times are original timeline times.
    let mut working = project.clone();
    let mut operations = Vec::new();
    let mut removed = Time::ZERO;
    for &(index, start, end) in cuts.iter().rev() {
        let op = ripple(&working, start, end)?;
        let operation: Operation = serde_json::from_value(op.clone())?;
        match working.apply(working.revision, vec![operation]) {
            Ok(next) => {
                working = next;
                removed = removed.plus(end.minus(start)?)?;
                listed[index]["cut"] = json!({"start":start,"end":end});
                operations.push(op);
            }
            Err(e) => {
                listed[index]["skipped"] = json!(format!("{}: {}", e.code, e.message));
            }
        }
    }
    let count = listed.len();
    listed.truncate(MAX_LISTED);
    Ok(json!({"voice_track_id":voice,
        "settings":{"threshold_db":settings.threshold_db,"min_pause":settings.min_pause,"keep":settings.keep,"edges":settings.edges},
        "speech_regions":regions.len(),"pauses":{"count":count,"cuts":operations.len(),"listed":listed},
        "removed":removed,"duration_before":duration,"duration_after":working.duration()?,
        "operations":operations,
        "next":"apply operations in order with session.apply (or check them with session.preview); later cuts come first, so each start is an original timeline time"}))
}
