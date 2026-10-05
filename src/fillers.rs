//! Filler-word removal: find listed words (um, uh, …) where the timeline speaks them, from
//! transcripts of its sources, and propose ripple deletions that remove them without cutting into
//! the neighbouring words. Each cut is checked on a working copy, as for pause tightening.
use crate::{Result, error, model::Project, outline::Said, time::Time, transcript};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

/// Filler runs listed in the result; the count stays exact.
const MAX_LISTED: usize = 500;
/// Fillers looked for when the caller gives none: common English hesitations.
pub const DEFAULT_WORDS: [&str; 9] = ["um", "uh", "erm", "er", "ah", "uhm", "umm", "hmm", "mm"];

/// What to remove.
pub struct Request<'a> {
    pub project: &'a Project,
    pub transcripts: &'a [transcript::Document],
    pub input_root: Option<&'a Path>,
    pub track_ids: Option<&'a [String]>,
    pub words: &'a [String],
    pub padding: Time,
}

/// Lowercase letters and digits only, so "Um," matches "um".
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Propose ripple deletions of the listed words.
pub fn propose(request: &Request) -> Result<Value> {
    let project = request.project;
    project.validate()?;
    let targets: BTreeSet<String> = request.words.iter().map(|w| normalized(w)).collect();
    if request.words.is_empty()
        || request.words.len() > 64
        || targets.iter().any(String::is_empty)
        || request.padding.compare(Time::new(1, 4)?)?.is_gt()
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "words must be 1-64 entries with a letter or digit each, and padding at most 1/4 s",
        ));
    }
    let rate = crate::render::clock::rate(project.frame_rate)?;
    let step = crate::tighten::grid_step(project, rate);
    let duration = project.duration()?;
    let words = crate::outline::timeline_words(
        project,
        request.transcripts,
        request.input_root,
        request.track_ids,
    )?;
    let said: &[Said] = &words.said;
    // Runs of consecutive filler words become one cut.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, word) in said.iter().enumerate() {
        if !targets.contains(&normalized(&word.text)) {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.1 + 1 == i => run.1 = i,
            _ => runs.push((i, i)),
        }
    }
    // Grid indices: nearest, or rounded away from a neighbouring word.
    let index = |t: Time, mode: i8| -> u64 {
        let n = t.num as u128 * rate.num as u128;
        let d = t.den as u128 * rate.den as u128 * step as u128;
        (match mode {
            0 => (2 * n + d) / (2 * d),
            1 => n.div_ceil(d),
            _ => n / d,
        }) as u64
    };
    let at = |i: u64| Time::new(i * step * rate.den, rate.num);
    let later =
        |a: Time, b: Time| -> Result<Time> { Ok(if a.compare(b)?.is_lt() { b } else { a }) };
    let earlier =
        |a: Time, b: Time| -> Result<Time> { Ok(if a.compare(b)?.is_lt() { a } else { b }) };
    let mut listed = Vec::new();
    let mut cuts = Vec::new();
    for (n, &(first, last)) in runs.iter().enumerate() {
        let text = said[first..=last]
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let (word_start, word_end) = (said[first].start, said[last].end);
        listed.push(json!({"text":text,"start":word_start,"end":word_end}));
        // Padding never reaches into the neighbouring words.
        let floor = if first > 0 {
            said[first - 1].end
        } else {
            Time::ZERO
        };
        let ceiling = match said.get(last + 1) {
            Some(next) => next.start,
            None => duration,
        };
        let from = later(
            word_start.minus(request.padding).unwrap_or(Time::ZERO),
            floor,
        )?;
        let to = earlier(word_end.plus(request.padding)?, ceiling)?;
        let mut start = index(from, 0);
        if at(start)?.compare(floor)?.is_lt() {
            start = index(floor, 1);
        }
        let mut end = index(to, 0);
        if at(end)?.compare(ceiling)?.is_gt() {
            end = index(ceiling, -1);
        }
        if end > start {
            cuts.push((n, at(start)?, at(end)?));
        } else {
            listed[n]["skipped"] = json!("shorter than one cut-grid step between its neighbours");
        }
    }
    let crate::tighten::Applied {
        operations,
        outcomes,
        project: working,
        removed,
    } = crate::tighten::apply_cuts(project, &cuts)?;
    for (n, outcome) in outcomes {
        match outcome {
            Ok(()) => {
                let (_, start, end) = cuts.iter().find(|c| c.0 == n).expect("cut");
                listed[n]["cut"] = json!({"start":start,"end":end});
            }
            Err(reason) => listed[n]["skipped"] = json!(reason),
        }
    }
    let count = listed.len();
    listed.truncate(MAX_LISTED);
    Ok(json!({"words":targets,"padding":request.padding,
        "fillers":{"count":count,"cuts":operations.len(),"listed":listed},
        "removed":removed,"duration_before":duration,"duration_after":working.duration()?,
        "cut_words":words.cut.len(),"unused_transcripts":words.unused,"operations":operations,
        "next":"apply operations in order with session.apply (or check them with session.preview); later cuts come first, so each start is an original timeline time"}))
}
