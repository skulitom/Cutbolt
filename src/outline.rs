//! Compact text outline of a timeline: one line per clip with its place, source, levels and, given
//! transcripts of the sources, what is said in it. Read-only; nothing is rendered.
use crate::{
    Result, error,
    model::Project,
    time::Time,
    tracks::{Arrangement, Composite, Kind, TrackClip, TransitionKind},
    transcript::{Document, Word},
};
use serde_json::{Value, json};
use std::{cmp::Ordering, collections::HashMap, fmt::Write, path::Path};

const MAX_DOCUMENTS: usize = 256;
pub(crate) const MAX_WORDS: usize = 2048;
pub(crate) const DEFAULT_WORDS: usize = 12;
/// Black and silent intervals listed per kind; the count stays exact.
const MAX_HOLES: usize = 20;

/// What to outline and how much transcript text to show per clip.
pub struct Request<'a> {
    pub project: &'a Project,
    pub transcripts: &'a [Document],
    pub input_root: Option<&'a Path>,
    pub sequence_id: Option<&'a str>,
    pub start: Option<Time>,
    pub end: Option<Time>,
    pub words: usize,
}

/// Seconds for reading: exact decimals up to milliseconds, otherwise rounded to the millisecond
/// and marked with `~`.
pub(crate) struct Clock {
    pub(crate) rounded: bool,
}
impl Clock {
    pub(crate) fn new() -> Self {
        Self { rounded: false }
    }
    pub(crate) fn at(&mut self, time: Time) -> String {
        let scaled = time.num as u128 * 1000;
        let den = time.den as u128;
        let millis = if scaled.is_multiple_of(den) {
            scaled / den
        } else {
            self.rounded = true;
            (scaled + den / 2) / den
        };
        let mut text = format!("{}", millis / 1000);
        let fraction = millis % 1000;
        if fraction != 0 {
            let digits = format!("{fraction:03}");
            text.push('.');
            text.push_str(digits.trim_end_matches('0'));
        }
        if !scaled.is_multiple_of(den) {
            text.push('~');
        }
        text
    }
    pub(crate) fn span(&mut self, start: Time, end: Time) -> String {
        format!("{}-{}", self.at(start), self.at(end))
    }
}

fn before(a: Time, b: Time) -> Result<bool> {
    Ok(a.compare(b)?.is_lt())
}
fn max(a: Time, b: Time) -> Result<Time> {
    Ok(if before(a, b)? { b } else { a })
}
fn min(a: Time, b: Time) -> Result<Time> {
    Ok(if before(a, b)? { a } else { b })
}

/// Transcript words of one source, from every document bound to it.
pub(crate) struct Spoken<'a> {
    pub(crate) ranges: Vec<(Time, Time)>,
    pub(crate) words: Vec<&'a Word>,
}

fn normalized(path: &str) -> String {
    path.replace('\\', "/")
}

/// Match each document to the assets of its source: by content identity when the asset is bound,
/// otherwise by path relative to `input_root`.
pub(crate) struct Matches<'a> {
    /// Asset IDs each document matched, by document ID.
    pub(crate) by_document: HashMap<&'a str, Vec<String>>,
    pub(crate) by_asset: HashMap<String, Spoken<'a>>,
    pub(crate) unused: Vec<Value>,
}

pub(crate) fn spoken<'a>(
    project: &Project,
    transcripts: &'a [Document],
    input_root: Option<&Path>,
) -> Result<Matches<'a>> {
    if transcripts.len() > MAX_DOCUMENTS {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!("At most {MAX_DOCUMENTS} transcripts per outline"),
        ));
    }
    let mut by_document: HashMap<&str, Vec<String>> = HashMap::new();
    let mut by_asset: HashMap<String, Spoken> = HashMap::new();
    let mut unused = Vec::new();
    for document in transcripts {
        document.validate()?;
        if by_document.contains_key(document.id.as_str()) {
            return Err(error(
                "INVALID_ARGUMENT",
                format!("Transcript {:?} is given twice", document.id),
            ));
        }
        let source = normalized(&document.source.path.to_string_lossy());
        let mut reason = "no asset has its source path";
        let mut assets = Vec::new();
        for asset in &project.assets {
            let path = match input_root {
                Some(root) if Path::new(&asset.path).is_absolute() => Path::new(&asset.path)
                    .strip_prefix(root)
                    .map(|p| normalized(&p.to_string_lossy()))
                    .unwrap_or_default(),
                _ => normalized(&asset.path),
            };
            let matched = match &asset.identity {
                Some(identity) if *identity == document.source.identity => true,
                Some(_) => {
                    if path == source {
                        reason = "the asset at its source path has a different content identity";
                    }
                    false
                }
                None => path == source,
            };
            if matched {
                assets.push(asset.id.clone());
            }
        }
        if assets.is_empty() {
            unused.push(json!({"id":document.id,"source":source,"reason":reason}));
        }
        let end = document.range_start.plus(document.range_duration)?;
        for asset in &assets {
            let entry = by_asset.entry(asset.clone()).or_insert(Spoken {
                ranges: Vec::new(),
                words: Vec::new(),
            });
            entry.ranges.push((document.range_start, end));
            entry.words.extend(document.words.iter());
        }
        by_document.insert(&document.id, assets);
    }
    for (asset, spoken) in &mut by_asset {
        spoken
            .ranges
            .sort_by(|a, b| a.0.compare(b.0).unwrap_or(Ordering::Equal));
        for pair in spoken.ranges.windows(2) {
            if before(pair[1].0, pair[0].1)? {
                return Err(error(
                    "INVALID_ARGUMENT",
                    format!("Transcripts of asset {asset:?} cover overlapping source ranges"),
                ));
            }
        }
        spoken
            .words
            .sort_by(|a, b| a.start.compare(b.start).unwrap_or(Ordering::Equal));
    }
    Ok(Matches {
        by_document,
        by_asset,
        unused,
    })
}

/// What a source range says: its words, with `*` on words partly outside it, shortened to the
/// first and last words when longer than `limit`.
fn snippet(spoken: &Spoken, from: Time, to: Time, limit: usize) -> Result<String> {
    let mut covered = Time::ZERO;
    for &(a, b) in &spoken.ranges {
        let (a, b) = (max(a, from)?, min(b, to)?);
        if before(a, b)? {
            covered = covered.plus(b.minus(a)?)?;
        }
    }
    if covered.num == 0 {
        return Ok(" (not transcribed)".into());
    }
    let mut words = Vec::new();
    for word in &spoken.words {
        if before(word.start, to)? && before(from, word.end)? {
            let partial = before(word.start, from)? || before(to, word.end)?;
            words.push(format!("{}{}", word.text, if partial { "*" } else { "" }));
        }
    }
    let part = if covered.compare(to.minus(from)?)?.is_lt() {
        " (partly transcribed)"
    } else {
        ""
    };
    if words.is_empty() {
        return Ok(format!(" (no speech){part}"));
    }
    let count = words.len();
    if limit == 0 {
        return Ok(format!(" ({}){part}", plural(count, "word")));
    }
    if count <= limit {
        return Ok(format!(" \"{}\"{part}", words.join(" ")));
    }
    let head = limit.div_ceil(2);
    let tail = &words[count - (limit - head)..];
    Ok(format!(
        " \"{} ...{}{}\" ({}){part}",
        words[..head].join(" "),
        if tail.is_empty() { "" } else { " " },
        tail.join(" "),
        plural(count, "word")
    ))
}

fn plural(count: usize, noun: &str) -> String {
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn transition_name(kind: TransitionKind) -> &'static str {
    match kind {
        TransitionKind::Dissolve => "dissolve",
        TransitionKind::DipBlack => "dip_black",
        TransitionKind::WipeLeft => "wipe_left",
        TransitionKind::WipeRight => "wipe_right",
    }
}

/// Levels and placement that differ from a plain clip.
fn extras(clip: &TrackClip, kind: Kind, clock: &mut Clock) -> String {
    let mut text = String::new();
    if kind == Kind::Audio {
        if let Some(curve) = &clip.gain_curve {
            let _ = write!(text, " gain curve({} keys)", curve.keys.len());
        } else if clip.gain_milli == 0 {
            text.push_str(" muted");
        } else if clip.gain_milli != 1000 {
            let _ = write!(
                text,
                " gain {:+.1}dB",
                20.0 * (clip.gain_milli as f64 / 1000.0).log10()
            );
        }
        if clip.fade_in.num != 0 {
            let _ = write!(text, " fade_in {}", clock.at(clip.fade_in));
        }
        if clip.fade_out.num != 0 {
            let _ = write!(text, " fade_out {}", clock.at(clip.fade_out));
        }
    }
    if let Some(t) = &clip.transform {
        text.push_str(" pip");
        if let Some([x, y, w, h]) = t.crop {
            let _ = write!(text, " crop {x},{y},{w},{h}");
        }
        if t.divisor != 1 {
            let _ = write!(text, " /{}", t.divisor);
        }
        if t.opacity != 255 {
            let _ = write!(text, " opacity {}", t.opacity);
        }
        let _ = write!(text, " at {},{}", t.position[0], t.position[1]);
    }
    text
}

/// Intervals of `[from, to)` that no span covers.
fn holes(mut spans: Vec<(Time, Time)>, from: Time, to: Time) -> Result<Vec<(Time, Time)>> {
    spans.sort_by(|a, b| a.0.compare(b.0).unwrap_or(Ordering::Equal));
    let mut found = Vec::new();
    let mut at = from;
    for (a, b) in spans {
        if before(at, a)? {
            found.push((at, min(a, to)?));
        }
        at = max(at, b)?;
        if !before(at, to)? {
            return Ok(found);
        }
    }
    if before(at, to)? {
        found.push((at, to));
    }
    Ok(found)
}

fn hole_line(name: &str, found: &[(Time, Time)], clock: &mut Clock) -> String {
    if found.is_empty() {
        return format!("{name}: none");
    }
    let mut listed: Vec<String> = found
        .iter()
        .take(MAX_HOLES)
        .map(|&(a, b)| clock.span(a, b))
        .collect();
    if found.len() > MAX_HOLES {
        listed.push(format!("+{} more", found.len() - MAX_HOLES));
    }
    format!("{name}: {}", listed.join(", "))
}

fn arrangement_lines(
    request: &Request,
    arrangement: &Arrangement,
    spoken: &HashMap<String, Spoken>,
    from: Time,
    to: Time,
    clock: &mut Clock,
    lines: &mut Vec<String>,
) -> Result<usize> {
    let mut linked: HashMap<&str, &str> = HashMap::new();
    for link in &arrangement.links {
        for member in &link.members {
            linked.insert(&member.clip_id, &link.id);
        }
    }
    let mut listed = 0;
    let (mut picture, mut sound) = (Vec::new(), Vec::new());
    for track in &arrangement.tracks {
        let mut clips: Vec<&TrackClip> = Vec::new();
        for clip in &track.clips {
            let end = clip.end()?;
            if before(clip.start, to)? && before(from, end)? {
                clips.push(clip);
            }
            if track.enabled {
                match track.kind {
                    Kind::Video if track.composite == Composite::Opaque => {
                        picture.push((clip.start, end))
                    }
                    Kind::Audio => sound.push((clip.start, end)),
                    Kind::Video => {}
                }
            }
        }
        clips.sort_by(|a, b| a.start.compare(b.start).unwrap_or(Ordering::Equal));
        let mut flags = vec![match track.kind {
            Kind::Video => "video",
            Kind::Audio => "audio",
        }];
        if track.composite == Composite::AlphaOver {
            flags.push("alpha_over");
        }
        if !track.enabled {
            flags.push("disabled");
        }
        if track.locked {
            flags.push("locked");
        }
        lines.push(format!(
            "track {} ({}): {}",
            track.id,
            flags.join(", "),
            plural(clips.len(), "clip")
        ));
        for clip in clips {
            let end = clip.end()?;
            let source_end = clip.source_in.plus(clip.duration)?;
            let source = match &clip.sequence_id {
                Some(id) => format!("seq:{id}"),
                None => clip.asset_id.clone(),
            };
            let mut line = format!(
                "  {} {} {} {}",
                clock.span(clip.start, end),
                clip.id,
                source,
                clock.span(clip.source_in, source_end)
            );
            if let Some(link) = linked.get(clip.id.as_str()) {
                let _ = write!(line, " link {link}");
            }
            line.push_str(&extras(clip, track.kind, clock));
            if track.kind == Kind::Audio
                && clip.sequence_id.is_none()
                && let Some(words) = spoken.get(&clip.asset_id)
            {
                line.push_str(&snippet(words, clip.source_in, source_end, request.words)?);
            }
            lines.push(line);
            listed += 1;
            for effect in track.transitions.iter().filter(|t| t.left_id == clip.id) {
                let cut = end;
                let (a, b) = (cut.minus(effect.before)?, cut.plus(effect.after)?);
                lines.push(format!(
                    "  {} [{} {} {}>{}]",
                    clock.span(a, b),
                    effect.id,
                    transition_name(effect.kind),
                    effect.left_id,
                    effect.right_id
                ));
            }
        }
    }
    lines.push(hole_line(
        "black (no opaque video clip)",
        &holes(picture, from, to)?,
        clock,
    ));
    lines.push(hole_line("no audio clip", &holes(sound, from, to)?, clock));
    Ok(listed)
}

/// Outline a project's timeline (or one child sequence) as compact text.
pub fn outline(request: &Request) -> Result<Value> {
    let project = request.project;
    project.validate()?;
    if request.words > MAX_WORDS {
        return Err(error(
            "INVALID_ARGUMENT",
            format!("words must be 0-{MAX_WORDS}"),
        ));
    }
    let Matches {
        by_document: documents,
        by_asset: spoken,
        unused,
    } = spoken(project, request.transcripts, request.input_root)?;
    let arrangement = match request.sequence_id {
        Some(id) => Some(&crate::sequences::get(project, id)?.arrangement),
        None => project.tracks.as_ref(),
    };
    let duration = match arrangement {
        Some(a) => a.duration,
        None => project.duration()?,
    };
    let from = request.start.unwrap_or(Time::ZERO);
    let to = min(request.end.unwrap_or(duration), duration)?;
    if !before(from, to)? {
        return Err(error(
            "INVALID_RANGE",
            "The outline range must start before its end and the timeline end",
        ));
    }
    let mut clock = Clock::new();
    let mut lines = Vec::new();
    let listed = match arrangement {
        Some(arrangement) => arrangement_lines(
            request,
            arrangement,
            &spoken,
            from,
            to,
            &mut clock,
            &mut lines,
        )?,
        None => {
            let mut at = Time::ZERO;
            let mut body = Vec::new();
            for clip in &project.clips {
                let end = at.plus(clip.duration)?;
                if before(at, to)? && before(from, end)? {
                    let mut line = format!("  {} {}", clock.span(at, end), clip.id);
                    match &clip.asset_id {
                        Some(asset) => {
                            let source_end = clip.source_in.plus(clip.duration)?;
                            let _ =
                                write!(line, " {asset} {}", clock.span(clip.source_in, source_end));
                            if let Some(words) = spoken.get(asset) {
                                line.push_str(&snippet(
                                    words,
                                    clip.source_in,
                                    source_end,
                                    request.words,
                                )?);
                            }
                        }
                        None => line.push_str(" gap"),
                    }
                    body.push(line);
                }
                at = end;
            }
            lines.push(format!(
                "sequential timeline (video with its audio): {}",
                plural(body.len(), "clip")
            ));
            let count = body.len();
            lines.extend(body);
            count
        }
    };
    // Header last, so it can say whether any time was rounded.
    let mut header = vec![format!(
        "{} {} rev {}: {}x{} {} fps, {}{}",
        match request.sequence_id {
            Some(id) => format!("sequence {id} of project"),
            None => "project".into(),
        },
        project.id,
        project.revision,
        project.width,
        project.height,
        project.frame_rate,
        if (from, to) == (Time::ZERO, duration) {
            format!("duration {}", clock.at(duration))
        } else {
            format!("range {} of {}", clock.span(from, to), clock.at(duration))
        },
        if arrangement.is_some() {
            ", tracks bottom to top"
        } else {
            ""
        }
    )];
    let assets: Vec<String> = project
        .assets
        .iter()
        .map(|a| format!("{}={} ({})", a.id, a.path, clock.at(a.duration)))
        .collect();
    header.push(format!(
        "assets: {}",
        if assets.is_empty() {
            "none".into()
        } else {
            assets.join(", ")
        }
    ));
    if request.sequence_id.is_none() && !project.sequences.is_empty() {
        let sequences: Vec<String> = project
            .sequences
            .iter()
            .map(|s| {
                format!(
                    "{}{} ({})",
                    s.id,
                    if s.multicam.is_some() {
                        " multicam"
                    } else {
                        ""
                    },
                    clock.at(s.arrangement.duration)
                )
            })
            .collect();
        header.push(format!(
            "sequences (outline one with sequence_id): {}",
            sequences.join(", ")
        ));
    }
    let mut legend = vec!["times in seconds, clip lines: timeline span, id, source, source span"];
    if clock.rounded {
        legend.push("~ rounded to the millisecond");
    }
    if !request.transcripts.is_empty() {
        legend.push("* word partly outside the clip");
    }
    header.push(format!("legend: {}", legend.join("; ")));
    if !request.transcripts.is_empty() {
        let mut used: Vec<String> = request
            .transcripts
            .iter()
            .filter(|d| !documents[d.id.as_str()].is_empty())
            .map(|d| format!("{}>{}", d.id, documents[d.id.as_str()].join("+")))
            .collect();
        if used.is_empty() {
            used.push("none".into());
        }
        let mut line = format!("transcripts: {}", used.join(", "));
        for skipped in &unused {
            let _ = write!(
                line,
                "; unused {} ({}: {})",
                skipped["id"].as_str().unwrap_or_default(),
                skipped["reason"].as_str().unwrap_or_default(),
                skipped["source"].as_str().unwrap_or_default()
            );
        }
        header.push(line);
    }
    header.extend(lines);
    let mut text = header.join("\n");
    text.push('\n');
    Ok(json!({"outline":text,"clips":listed,"lines":header.len(),"unused_transcripts":unused}))
}

/// A word on the timeline's (or a rendered cut's) clock.
#[derive(Clone)]
pub(crate) struct Said {
    pub(crate) start: Time,
    pub(crate) end: Time,
    pub(crate) text: String,
}
impl Said {
    pub(crate) fn middle(&self) -> Result<Time> {
        self.start.plus(self.end)?.times(Time::new(1, 2)?)
    }
}

/// What a timeline says, from transcripts of its sources.
pub(crate) struct TimelineWords {
    /// Whole words inside audible audio clips, on the timeline clock, in time order.
    pub(crate) said: Vec<Said>,
    /// Words a clip edge cuts through: clip ID, edge, word and timeline time.
    pub(crate) cut: Vec<Value>,
    /// Transcripts that matched no asset, with the reason.
    pub(crate) unused: Vec<Value>,
}

/// Words the timeline says: every whole transcript word inside an audible audio clip (on an
/// enabled audio track, not muted, not a child sequence), moved to timeline time. `tracks`
/// restricts placed timelines to those audio tracks. Words a clip edge cuts through are reported
/// instead.
pub(crate) fn timeline_words(
    project: &Project,
    transcripts: &[Document],
    input_root: Option<&Path>,
    tracks: Option<&[String]>,
) -> Result<TimelineWords> {
    let matches = spoken(project, transcripts, input_root)?;
    // (clip ID, timeline start, source in, duration, asset)
    let mut clips: Vec<(&str, Time, Time, Time, &str)> = Vec::new();
    match &project.tracks {
        Some(arrangement) => {
            if let Some(ids) = tracks {
                for id in ids {
                    if !arrangement
                        .tracks
                        .iter()
                        .any(|t| t.id == *id && t.kind == Kind::Audio)
                    {
                        return Err(crate::missing(
                            "MISSING_TRACK",
                            "audio track",
                            id,
                            arrangement
                                .tracks
                                .iter()
                                .filter(|t| t.kind == Kind::Audio)
                                .map(|t| t.id.as_str()),
                        ));
                    }
                }
            }
            for track in &arrangement.tracks {
                if !track.enabled
                    || track.kind != Kind::Audio
                    || tracks.is_some_and(|ids| !ids.contains(&track.id))
                {
                    continue;
                }
                for clip in &track.clips {
                    let muted = clip.gain_milli == 0 && clip.gain_curve.is_none();
                    if clip.sequence_id.is_none() && !muted {
                        clips.push((
                            &clip.id,
                            clip.start,
                            clip.source_in,
                            clip.duration,
                            &clip.asset_id,
                        ));
                    }
                }
            }
        }
        None => {
            if tracks.is_some() {
                return Err(error(
                    "INVALID_ARGUMENT",
                    "A sequential timeline has no tracks to choose; omit track_ids",
                ));
            }
            let mut at = Time::ZERO;
            for clip in &project.clips {
                if let Some(asset) = &clip.asset_id {
                    clips.push((&clip.id, at, clip.source_in, clip.duration, asset));
                }
                at = at.plus(clip.duration)?;
            }
        }
    }
    let mut said = Vec::new();
    let mut cut = Vec::new();
    for (id, start, source_in, duration, asset) in clips {
        let Some(spoken) = matches.by_asset.get(asset) else {
            continue;
        };
        let source_end = source_in.plus(duration)?;
        for word in &spoken.words {
            if !word.start.compare(source_end)?.is_lt() || !source_in.compare(word.end)?.is_lt() {
                continue;
            }
            if word.start.compare(source_in)?.is_lt() {
                cut.push(json!({"clip_id":id,"edge":"start","word":word.text,"time":start}));
            } else if word.end.compare(source_end)?.is_gt() {
                cut.push(json!({"clip_id":id,"edge":"end","word":word.text,"time":start.plus(duration)?}));
            } else {
                said.push(Said {
                    start: start.plus(word.start.minus(source_in)?)?,
                    end: start.plus(word.end.minus(source_in)?)?,
                    text: word.text.clone(),
                });
            }
        }
    }
    said.sort_by(|a, b| a.start.compare(b.start).expect("valid times"));
    Ok(TimelineWords {
        said,
        cut,
        unused: matches.unused,
    })
}
