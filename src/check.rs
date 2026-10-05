//! A static check of a project for the mistakes that are easy to make and expensive to find by
//! rendering: flash frames, picture and sound of one source out of sync, jump cuts within one
//! source, black or silent stretches, unused assets, missing or changed media and words cut by
//! clip edges. Nothing is rendered or changed.
use crate::{
    Result,
    model::Project,
    outline::Clock,
    time::Time,
    tracks::{Composite, Kind},
    transcript::Document,
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

/// Findings listed in the result; counts stay exact.
const MAX_LISTED: usize = 200;

/// What to check and how strictly.
pub struct Request<'a> {
    pub project: &'a Project,
    pub input_root: Option<&'a Path>,
    pub transcripts: &'a [Document],
    pub min_clip_frames: u64,
    pub jump_window: Time,
}

/// A clip on the picture or sound of the timeline.
struct Placed<'a> {
    id: &'a str,
    asset: Option<&'a str>,
    start: Time,
    source_in: Time,
    duration: Time,
}

fn before(a: Time, b: Time) -> Result<bool> {
    Ok(a.compare(b)?.is_lt())
}

/// Check a project; the result's `ok` is false when any warning or error was found.
pub fn check(request: &Request) -> Result<Value> {
    let project = request.project;
    project.validate()?;
    let rate = crate::render::clock::rate(project.frame_rate)?;
    let duration = project.duration()?;
    let mut clock = Clock::new();
    let mut findings: Vec<Value> = Vec::new();
    let mut find = |severity: &str, kind: &str, message: String, detail: Value| {
        let mut finding = json!({"severity":severity,"kind":kind,"message":message});
        if let Value::Object(map) = detail {
            for (k, v) in map {
                finding[k] = v;
            }
        }
        findings.push(finding);
    };
    // Picture tracks (in order), sound clips, and every used asset.
    let mut pictures: Vec<(String, Vec<Placed>)> = Vec::new();
    let mut sounds: Vec<Placed> = Vec::new();
    let mut used: BTreeSet<&str> = BTreeSet::new();
    let mut linked: Vec<BTreeSet<&str>> = Vec::new();
    let mut black = Vec::new();
    let mut audible = Vec::new();
    match &project.tracks {
        Some(arrangement) => {
            for sequence in &project.sequences {
                for track in &sequence.arrangement.tracks {
                    used.extend(track.clips.iter().map(|c| c.asset_id.as_str()));
                }
            }
            for link in &arrangement.links {
                linked.push(link.members.iter().map(|m| m.clip_id.as_str()).collect());
            }
            for track in &arrangement.tracks {
                used.extend(track.clips.iter().map(|c| c.asset_id.as_str()));
                if !track.enabled {
                    if !track.clips.is_empty() {
                        find(
                            "info",
                            "disabled_track",
                            format!(
                                "track {} is disabled; its {} clips do not play",
                                track.id,
                                track.clips.len()
                            ),
                            json!({"track_id":track.id}),
                        );
                    }
                    continue;
                }
                let mut placed: Vec<Placed> = track
                    .clips
                    .iter()
                    .map(|c| Placed {
                        id: &c.id,
                        asset: c.sequence_id.is_none().then_some(c.asset_id.as_str()),
                        start: c.start,
                        source_in: c.source_in,
                        duration: c.duration,
                    })
                    .collect();
                placed.sort_by(|a, b| a.start.compare(b.start).expect("valid times"));
                for clip in &placed {
                    let span = (clip.start, clip.start.plus(clip.duration)?);
                    match track.kind {
                        Kind::Video if track.composite == Composite::Opaque => black.push(span),
                        Kind::Audio => audible.push(span),
                        Kind::Video => {}
                    }
                }
                match track.kind {
                    Kind::Video => pictures.push((track.id.clone(), placed)),
                    Kind::Audio => sounds.extend(placed),
                }
            }
        }
        None => {
            let mut at = Time::ZERO;
            let mut placed = Vec::new();
            for clip in &project.clips {
                if let Some(asset) = &clip.asset_id {
                    used.insert(asset);
                    placed.push(Placed {
                        id: &clip.id,
                        asset: Some(asset),
                        start: at,
                        source_in: clip.source_in,
                        duration: clip.duration,
                    });
                    black.push((at, at.plus(clip.duration)?));
                    audible.push((at, at.plus(clip.duration)?));
                }
                at = at.plus(clip.duration)?;
            }
            pictures.push(("sequential".into(), placed));
        }
    }
    // Black and audio-free stretches.
    for (from, to) in crate::outline::holes(black, Time::ZERO, duration)? {
        find(
            "warning",
            "black",
            format!("{} has no picture", clock.span(from, to)),
            json!({"start":from,"end":to}),
        );
    }
    if project.tracks.is_some() {
        for (from, to) in crate::outline::holes(audible, Time::ZERO, duration)? {
            find(
                "info",
                "no_audio",
                format!("{} has no audio clip", clock.span(from, to)),
                json!({"start":from,"end":to}),
            );
        }
    }
    // Flash frames and jump cuts on each picture track.
    let shortest = Time::new(request.min_clip_frames * rate.den, rate.num)?;
    for (track, clips) in &pictures {
        for clip in clips {
            if clip.asset.is_some() && before(clip.duration, shortest)? {
                find(
                    "warning",
                    "flash_frame",
                    format!(
                        "clip {} on {track} lasts {} frames at {}",
                        clip.id,
                        clip.duration.units(rate)?,
                        clock.at(clip.start)
                    ),
                    json!({"clip_id":clip.id,"track_id":track,"start":clip.start,"duration":clip.duration}),
                );
            }
        }
        for pair in clips.windows(2) {
            let (left, right) = (&pair[0], &pair[1]);
            let left_end = left.start.plus(left.duration)?;
            if left.asset.is_none() || left.asset != right.asset || left_end != right.start {
                continue;
            }
            let left_source_end = left.source_in.plus(left.duration)?;
            let jump = if before(right.source_in, left_source_end)? {
                left_source_end.minus(right.source_in)?
            } else {
                right.source_in.minus(left_source_end)?
            };
            if jump.num != 0 && before(jump, request.jump_window)? {
                let backward = before(right.source_in, left_source_end)?;
                find(
                    "info",
                    "jump_cut",
                    format!(
                        "{} cuts within {} from {} to {} ({} {} s)",
                        clock.at(right.start),
                        left.asset.unwrap_or_default(),
                        left.id,
                        right.id,
                        if backward { "back" } else { "skipping" },
                        clock.at(jump)
                    ),
                    json!({"time":right.start,"left_id":left.id,"right_id":right.id,"source_jump":jump,"backward":backward}),
                );
            }
        }
    }
    // Picture and sound of one source that play together but are offset.
    for (track, clips) in &pictures {
        for picture in clips {
            let Some(asset) = picture.asset else { continue };
            for sound in sounds.iter().filter(|s| s.asset == Some(asset)) {
                let overlaps = before(picture.start, sound.start.plus(sound.duration)?)?
                    && before(sound.start, picture.start.plus(picture.duration)?)?;
                let together = linked
                    .iter()
                    .any(|group| group.contains(picture.id) && group.contains(sound.id));
                if !overlaps || together {
                    continue;
                }
                let (a, b) = (
                    picture.start.plus(sound.source_in)?,
                    sound.start.plus(picture.source_in)?,
                );
                if a == b {
                    continue;
                }
                let (offset, late) = if before(a, b)? {
                    (b.minus(a)?, "sound")
                } else {
                    (a.minus(b)?, "picture")
                };
                find(
                    "warning",
                    "out_of_sync",
                    format!(
                        "clips {} ({track}) and {} play {} at different source times: the {late} is {} s late",
                        picture.id,
                        sound.id,
                        asset,
                        clock.at(offset)
                    ),
                    json!({"picture_id":picture.id,"sound_id":sound.id,"asset_id":asset,"offset":offset,"late":late}),
                );
            }
        }
    }
    // Assets no clip uses.
    for asset in &project.assets {
        if !used.contains(asset.id.as_str()) {
            find(
                "info",
                "unused_asset",
                format!("asset {} is not used", asset.id),
                json!({"asset_id":asset.id}),
            );
        }
    }
    // Media present and unchanged.
    if let Some(root) = request.input_root {
        for asset in &project.assets {
            let path = if Path::new(&asset.path).is_absolute() {
                std::path::PathBuf::from(&asset.path)
            } else {
                root.join(&asset.path)
            };
            if !path.is_file() {
                find(
                    "error",
                    "missing_media",
                    format!("asset {} file {} is missing", asset.id, asset.path),
                    json!({"asset_id":asset.id,"path":asset.path}),
                );
            } else if let Some(identity) = &asset.identity
                && let Err(e) = crate::registry::check_file(
                    &path,
                    &asset.path,
                    identity,
                    "relink it with registry.relink",
                )
            {
                find(
                    "error",
                    "changed_media",
                    e.message,
                    json!({"asset_id":asset.id,"path":asset.path}),
                );
            }
        }
    }
    // Words cut by clip edges.
    if !request.transcripts.is_empty() {
        let words =
            crate::outline::timeline_words(project, request.transcripts, request.input_root, None)?;
        // A transcript that matches no asset cannot be checked; say so rather than pass silently.
        let unmatched = |entry: &Value| {
            format!(
                "transcript {} matches no asset ({})",
                entry["id"].as_str().unwrap_or_default(),
                entry["reason"].as_str().unwrap_or_default()
            )
        };
        if !words.unused.is_empty() && words.unused.len() == request.transcripts.len() {
            find(
                "warning",
                "unmatched_transcripts",
                format!(
                    "no words were checked: {}{}",
                    unmatched(&words.unused[0]),
                    match words.unused.len() {
                        1 => String::new(),
                        n => format!(", and {} more", n - 1),
                    }
                ),
                json!({"transcripts":words.unused}),
            );
        } else {
            for entry in &words.unused {
                find(
                    "info",
                    "unmatched_transcript",
                    format!("{}; its words were not checked", unmatched(entry)),
                    entry.clone(),
                );
            }
        }
        for cut in &words.cut {
            let time: Time = serde_json::from_value(cut["time"].clone())?;
            find(
                "warning",
                "cut_word",
                format!(
                    "clip {} {} inside \"{}\" at {}",
                    cut["clip_id"].as_str().unwrap_or_default(),
                    if cut["edge"] == "start" {
                        "starts"
                    } else {
                        "ends"
                    },
                    cut["word"].as_str().unwrap_or_default(),
                    clock.at(time)
                ),
                cut.clone(),
            );
        }
    }
    // Errors first, then warnings, then notes; each in the order found.
    let rank = |f: &Value| match f["severity"].as_str() {
        Some("error") => 0,
        Some("warning") => 1,
        _ => 2,
    };
    findings.sort_by_key(rank);
    let count = |severity: &str| {
        findings
            .iter()
            .filter(|f| f["severity"] == severity)
            .count()
    };
    let (errors, warnings, infos) = (count("error"), count("warning"), count("info"));
    let mut lines = vec![format!(
        "check of {} rev {}: {errors} errors, {warnings} warnings, {infos} notes",
        project.id, project.revision
    )];
    lines.extend(findings.iter().take(MAX_LISTED).map(|f| {
        format!(
            "{} {}: {}",
            f["severity"].as_str().unwrap_or_default(),
            f["kind"].as_str().unwrap_or_default(),
            f["message"].as_str().unwrap_or_default()
        )
    }));
    if clock.rounded {
        lines.push("times in seconds; ~ rounded to the millisecond".into());
    }
    let total = findings.len();
    findings.truncate(MAX_LISTED);
    Ok(
        json!({"ok":errors + warnings == 0,"errors":errors,"warnings":warnings,"notes":infos,
        "summary":lines.join("\n") + "\n","findings":{"count":total,"listed":findings}}),
    )
}
