//! Filler-word removal: find listed words (um, uh, …) where the timeline speaks them, from
//! transcripts of its sources, and propose ripple deletions that remove them without cutting into
//! the neighbouring words. Each cut is checked on a working copy, as for pause tightening.
//! Recognizers often leave hesitations out; the transcripts' uncovered sounds that read like one
//! are listed, and cut too when the caller asks.
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
    /// Only fillers wholly inside this timeline window; `None` ends are the timeline's own.
    pub window: (Option<Time>, Option<Time>),
    /// Silence fillers on `track_ids` instead of ripple-deleting them from every track.
    pub lift: bool,
    /// Also cut uncovered sounds that read like a filler.
    pub uncovered: bool,
}

/// Uncovered sounds shorter than this are not called fillers: a dropped "a" reads like "uh".
const SHORTEST_FILLER: Time = Time { num: 1, den: 8 };

/// Whether letters the acoustic model read look like a hesitation, at most four of them: vowels
/// (A, E, U) and then an optional H and M or R (AM, UH, ER, ERM, UHM), or M with an optional
/// leading H (M, MM, HM, HMM).
pub(crate) fn filler_like(letters: &str) -> bool {
    let read: Vec<u8> = letters
        .bytes()
        .filter(|c| !c.is_ascii_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    let vowels = read.iter().take_while(|c| b"AEU".contains(c)).count();
    let rest = &read[vowels..];
    let rest = rest.strip_prefix(b"H").unwrap_or(rest);
    (1..=4).contains(&read.len())
        && if vowels > 0 {
            rest.iter().all(|c| b"MR".contains(c))
        } else {
            !rest.is_empty() && rest.iter().all(|&c| c == b'M')
        }
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
    if request.lift && request.track_ids.is_none_or(<[String]>::is_empty) {
        return Err(error(
            "INVALID_ARGUMENT",
            "lift silences the fillers on track_ids, so give the tracks to silence, such as the voice track",
        ));
    }
    let rate = crate::render::clock::rate(project.frame_rate)?;
    let step = crate::tighten::grid_step(project, rate);
    let duration = project.duration()?;
    let total = (duration.num as u128 * 48_000 / duration.den as u128) as u64;
    let (from, to) = crate::tighten::window_samples(request.window, total)?;
    let (from, to) = (Time::new(from, 48_000)?, Time::new(to, 48_000)?);
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
        if !targets.contains(&normalized(&word.text))
            || word.start.compare(from)?.is_lt()
            || word.end.compare(to)?.is_gt()
        {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.1 + 1 == i => run.1 = i,
            _ => runs.push((i, i)),
        }
    }
    // Uncovered sounds in the window: the words either side, and whether they read like a filler.
    // (sound, index in said of the word before it, of the word after it, covered by a word)
    let mut sounds = Vec::new();
    for sound in &words.uncovered {
        if sound.start.compare(from)?.is_lt() || sound.end.compare(to)?.is_gt() {
            continue;
        }
        // Neighbours by the sound's middle: a word's context may reach into the sound, which
        // only shortens its cut; a word over the middle covers it.
        let middle = sound.middle()?;
        let mut after = None;
        let mut before = None;
        let mut covered = false;
        for (i, word) in said.iter().enumerate() {
            if !word.end.compare(middle)?.is_gt() {
                after = Some(i);
            } else if word.start.compare(middle)?.is_gt() {
                before = before.or(Some(i));
            } else {
                covered = true;
            }
        }
        sounds.push((sound, after, before, covered));
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
    // The grid-snapped cut of [first, last] with padding, never into the neighbouring words.
    let span =
        |first: Time, last: Time, floor: Time, ceiling: Time| -> Result<Option<(Time, Time)>> {
            let from = later(first.minus(request.padding).unwrap_or(Time::ZERO), floor)?;
            let to = earlier(last.plus(request.padding)?, ceiling)?;
            let mut start = index(from, 0);
            if at(start)?.compare(floor)?.is_lt() {
                start = index(floor, 1);
            }
            let mut end = index(to, 0);
            if at(end)?.compare(ceiling)?.is_gt() {
                end = index(ceiling, -1);
            }
            Ok(if end > start {
                Some((at(start)?, at(end)?))
            } else {
                None
            })
        };
    let short = "shorter than one cut-grid step between its neighbours";
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
        let floor = if first > 0 {
            said[first - 1].end
        } else {
            Time::ZERO
        };
        let ceiling = match said.get(last + 1) {
            Some(next) => next.start,
            None => duration,
        };
        match span(word_start, word_end, floor, ceiling)? {
            Some((start, end)) => cuts.push((n, start, end)),
            None => listed[n]["skipped"] = json!(short),
        }
    }
    let mut heard = Vec::new();
    let mut filler_like_count = 0;
    for (j, &(sound, after, before, covered)) in sounds.iter().enumerate() {
        let reads = filler_like(&sound.text);
        let brief = sound
            .end
            .minus(sound.start)?
            .compare(SHORTEST_FILLER)?
            .is_lt();
        let like = reads && !brief;
        filler_like_count += usize::from(like);
        heard.push(
            json!({"letters":sound.text,"start":sound.start,"end":sound.end,"filler_like":like,
            "after":after.map(|i| &said[i].text),"before":before.map(|i| &said[i].text)}),
        );
        let reason = if covered {
            "a word covers it"
        } else if !reads {
            "does not read like a filler"
        } else if brief {
            "shorter than a hesitation (1/8 s)"
        } else if !request.uncovered {
            "not cut without uncovered: true"
        } else {
            let floor = after.map_or(Time::ZERO, |i| said[i].end);
            let ceiling = before.map_or(duration, |i| said[i].start);
            match span(sound.start, sound.end, floor, ceiling)? {
                Some((start, end)) => {
                    let mut clear = true;
                    for &(_, a, b) in &cuts {
                        clear &= !start.compare(b)?.is_lt() || !a.compare(end)?.is_lt();
                    }
                    if clear {
                        cuts.push((runs.len() + j, start, end));
                        continue;
                    }
                    "overlaps the cut of a listed filler"
                }
                None => short,
            }
        };
        heard[j]["skipped"] = json!(reason);
    }
    cuts.sort_by(|a, b| a.1.compare(b.1).expect("valid times"));
    let how = match (request.lift, request.track_ids) {
        (true, Some(tracks)) => crate::tighten::How::Lift(tracks),
        _ => crate::tighten::How::Ripple,
    };
    let crate::tighten::Applied {
        operations,
        outcomes,
        project: working,
        removed,
        silenced,
    } = crate::tighten::apply_cuts(project, &cuts, &how)?;
    let (mut word_cuts, mut sound_cuts) = (0, 0);
    for (n, outcome) in outcomes {
        let entry = match listed.get_mut(n) {
            Some(entry) => entry,
            None => &mut heard[n - runs.len()],
        };
        match outcome {
            Ok(()) => {
                let (_, start, end) = cuts.iter().find(|c| c.0 == n).expect("cut");
                entry["cut"] = json!({"start":start,"end":end});
                *(if n < runs.len() {
                    &mut word_cuts
                } else {
                    &mut sound_cuts
                }) += 1;
            }
            Err(reason) => entry["skipped"] = json!(reason),
        }
    }
    let count = listed.len();
    listed.truncate(MAX_LISTED);
    let sounds = heard.len();
    heard.truncate(MAX_LISTED);
    let mut next = "apply operations in order with session.apply (or check them with session.preview); later cuts come first, so each start is an original timeline time".to_owned();
    if filler_like_count > 0 && !request.uncovered {
        next.push_str(&format!("; {filler_like_count} uncovered sound(s) read like a filler the recognizer left out: check the words either side in uncovered.listed, then pass uncovered: true to cut them"));
    }
    Ok(
        json!({"words":targets,"padding":request.padding,"start":from,"end":to,"lift":request.lift,
        "fillers":{"count":count,"cuts":word_cuts,"listed":listed},
        "uncovered":{"count":sounds,"filler_like":filler_like_count,"cuts":sound_cuts,"listed":heard},
        "removed":removed,"silenced":silenced,"duration_before":duration,"duration_after":working.duration()?,
        "cut_words":words.cut.len(),"unused_transcripts":words.unused,"operations":operations,
        "next":next}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hesitations_read_like_fillers_and_words_do_not() {
        for letters in [
            "AM", "UM", "UH", "A", "ER", "ERM", "UHM", "AH", "EH", "M", "MM", "HM", "HMM", "a m",
        ] {
            assert!(filler_like(letters), "{letters}");
        }
        for letters in [
            "", "AN", "AND", "THE", "HE", "ME", "HA", "ARE", "MHM", "UMMMM", "T", "SO",
        ] {
            assert!(!filler_like(letters), "{letters}");
        }
    }
}
