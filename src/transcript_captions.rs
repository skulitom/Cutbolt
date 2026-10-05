//! Caption cues drafted from what a timeline says: transcript words of its sources, moved to
//! timeline time, grouped into readable cues and laid out in balanced lines.
use crate::{
    Result,
    captions::{self, Cue, Document, Overlap, Style},
    error,
    graphics::Align,
    model::Project,
    outline::Said,
    time::Time,
    transcript,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

/// How words become cues.
pub struct Rules {
    /// Longest line in Unicode scalars, 10-80; a single longer word still gets its own line.
    pub line_chars: usize,
    /// Lines per cue, 1-3.
    pub lines: usize,
    /// Longest cue from its first word's start to its last word's end.
    pub max_duration: Time,
    /// Shortest display time; a cue is held this long unless the next cue starts first.
    pub min_duration: Time,
    /// A silence at least this long between two words starts a new cue.
    pub pause: Time,
}

/// What to caption.
pub struct Request<'a> {
    pub project: &'a Project,
    pub transcripts: &'a [transcript::Document],
    pub input_root: Option<&'a Path>,
    pub track_ids: Option<&'a [String]>,
    pub start: Option<Time>,
    pub end: Option<Time>,
    pub id: &'a str,
    pub color: [u8; 3],
    pub align: Align,
    pub rules: Rules,
}

fn chars(text: &str) -> usize {
    text.chars().count()
}

/// Whether a word ends a sentence: its last character, after closing quotes and brackets, is
/// `.`, `?`, `!` or `…`.
fn sentence_end(text: &str) -> bool {
    text.trim_end_matches(['"', '\'', '”', '’', ')', ']', '»'])
        .ends_with(['.', '?', '!', '…'])
}

/// Lines needed when words fill each line in turn.
fn greedy_lines(words: &[&str], width: usize) -> usize {
    let (mut lines, mut length) = (1, 0);
    for word in words {
        let size = chars(word);
        if length == 0 {
            length = size;
        } else if length + 1 + size <= width {
            length += 1 + size;
        } else {
            lines += 1;
            length = size;
        }
    }
    lines
}

/// Balanced layout in the greedy line count: of all ways to break the words into that many lines,
/// where each line fits `width` or is a single word, the one whose longest line is shortest; ties
/// go to the shortest first line, then the shortest second.
fn layout(words: &[&str], width: usize) -> String {
    let count = greedy_lines(words, width).min(words.len());
    let length = |part: &[&str]| part.iter().map(|w| chars(w)).sum::<usize>() + part.len() - 1;
    let mut best: Option<(Vec<usize>, Vec<usize>)> = None;
    // Break positions are word indices where a new line begins.
    let mut breaks: Vec<usize> = (1..count).collect();
    loop {
        let mut bounds = vec![0];
        bounds.extend(&breaks);
        bounds.push(words.len());
        let lengths: Vec<usize> = bounds
            .windows(2)
            .map(|b| length(&words[b[0]..b[1]]))
            .collect();
        let fits = bounds
            .windows(2)
            .zip(&lengths)
            .all(|(b, &l)| l <= width || b[1] - b[0] == 1);
        if fits {
            let key = (lengths.iter().copied().max().unwrap_or(0), lengths.clone());
            let better = match &best {
                None => true,
                Some((_, best_lengths)) => {
                    let best_key = (
                        best_lengths.iter().copied().max().unwrap_or(0),
                        best_lengths,
                    );
                    (key.0, &key.1) < best_key
                }
            };
            if better {
                best = Some((breaks.clone(), lengths));
            }
        }
        // Next combination of `count - 1` break positions in 1..words.len().
        let slots = count - 1;
        let mut i = slots;
        loop {
            if i == 0 {
                let (breaks, _) = best.expect("the greedy layout fits");
                let mut bounds = vec![0];
                bounds.extend(&breaks);
                bounds.push(words.len());
                return bounds
                    .windows(2)
                    .map(|b| words[b[0]..b[1]].join(" "))
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            i -= 1;
            if breaks[i] < words.len() - (slots - i) {
                breaks[i] += 1;
                for j in i + 1..slots {
                    breaks[j] = breaks[j - 1] + 1;
                }
                break;
            }
        }
    }
}

fn floor_ms(time: Time) -> Result<Time> {
    Time::new((time.num as u128 * 1000 / time.den as u128) as u64, 1000)
}
fn ceil_ms(time: Time) -> Result<Time> {
    Time::new(
        (time.num as u128 * 1000).div_ceil(time.den as u128) as u64,
        1000,
    )
}

/// Draft a caption document for a timeline from transcripts of its sources.
pub fn draft(request: &Request) -> Result<Value> {
    let rules = &request.rules;
    let between = |t: Time, low: Time, high: Time| -> Result<bool> {
        Ok(!t.compare(low)?.is_lt() && !t.compare(high)?.is_gt())
    };
    if !(10..=80).contains(&rules.line_chars)
        || !(1..=3).contains(&rules.lines)
        || !between(rules.max_duration, Time::new(1, 1)?, Time::new(10, 1)?)?
        || !between(rules.min_duration, Time::ZERO, Time::new(5, 1)?)?
        || !between(rules.pause, Time::new(1, 10)?, Time::new(5, 1)?)?
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "line_chars must be 10-80, lines 1-3, max_duration 1-10 s, min_duration 0-5 s and pause 0.1-5 s",
        ));
    }
    request.project.validate()?;
    let words = crate::outline::timeline_words(
        request.project,
        request.transcripts,
        request.input_root,
        request.track_ids,
    )?;
    let from = request.start.unwrap_or(Time::ZERO);
    let said: Vec<&Said> = words
        .said
        .iter()
        .filter(|w| {
            !w.start.compare(from).expect("valid").is_lt()
                && request
                    .end
                    .is_none_or(|end| w.start.compare(end).expect("valid").is_lt())
        })
        .collect();
    // Group words into cues.
    let mut groups: Vec<Vec<&Said>> = Vec::new();
    for word in said {
        if let Some(current) = groups.last_mut() {
            let last = current.last().expect("nonempty group");
            let texts: Vec<&str> = current
                .iter()
                .map(|w| w.text.as_str())
                .chain([word.text.as_str()])
                .collect();
            let continues = word
                .start
                .minus(last.end)
                .map_or(true, |gap| gap.compare(rules.pause).expect("valid").is_lt())
                && !sentence_end(&last.text)
                && !word
                    .end
                    .minus(current[0].start)?
                    .compare(rules.max_duration)?
                    .is_gt()
                && greedy_lines(&texts, rules.line_chars) <= rules.lines;
            if continues {
                current.push(word);
                continue;
            }
        }
        groups.push(vec![word]);
    }
    if groups.len() > 4096 {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!(
                "{} cues exceed the 4,096 a caption document holds; caption a shorter range with start and end",
                groups.len()
            ),
        ));
    }
    // Time each cue on the millisecond grid the subtitle formats use: starts round down, ends
    // round up, held to the minimum duration but never into the next cue.
    let mut cues = Vec::with_capacity(groups.len());
    let mut overlap = false;
    for (i, group) in groups.iter().enumerate() {
        let first = group[0].start;
        let last = group.last().expect("nonempty group").end;
        let next = groups.get(i + 1).map(|g| g[0].start);
        let mut end = last;
        let held = first.plus(rules.min_duration)?;
        let hold_until = match next {
            Some(next) if next.compare(held)?.is_lt() => next,
            _ => held,
        };
        if hold_until.compare(end)?.is_gt() {
            end = hold_until;
        }
        let start = floor_ms(first)?;
        let mut end_ms = ceil_ms(end)?;
        if let Some(next) = next {
            let next_ms = floor_ms(next)?;
            if !end.compare(next)?.is_gt() && end_ms.compare(next_ms)?.is_gt() {
                end_ms = next_ms;
            }
            if end_ms.compare(next_ms)?.is_gt() {
                overlap = true;
            }
        }
        if !end_ms.compare(start)?.is_gt() {
            end_ms = start.plus(Time::new(1, 1000)?)?;
        }
        let texts: Vec<&str> = group.iter().map(|w| w.text.as_str()).collect();
        cues.push(Cue {
            id: format!("c{}", i + 1),
            start,
            end: end_ms,
            text: layout(&texts, rules.line_chars),
            style: "default".into(),
            align: request.align.clone(),
            speaker: None,
        });
    }
    let document = Document {
        schema_version: 1,
        id: request.id.to_owned(),
        revision: 0,
        overlap: if overlap {
            Overlap::Allow
        } else {
            Overlap::Reject
        },
        styles: BTreeMap::from([(
            "default".to_owned(),
            Style {
                color: request.color,
            },
        )]),
        cues,
    };
    let inspection = captions::inspect(&document)?;
    let words_used = groups.iter().map(Vec::len).sum::<usize>();
    Ok(
        json!({"document":document,"inspection":inspection,"words":words_used,
        "cut_words":{"count":words.cut.len(),"listed":words.cut.iter().take(50).collect::<Vec<_>>()},
        "unused_transcripts":words.unused,
        "next":"export with captions.export (srt for upload), adjust text with captions.apply, or burn in with captions.scene"}),
    )
}
