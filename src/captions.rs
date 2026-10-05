//! Original strict caption interchange, immutable editing and exact scene sampling.
use crate::{
    Result, error,
    graphics::{Align, Graphic, Overflow, Wrap},
    media, render,
    scene::{self, Identity, Scene},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashSet},
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
};

const LIMIT: usize = 2 * 1024 * 1024;
const DAY: Time = Time { num: 86400, den: 1 };
/// Subtitle file format: `srt` (plain-text SRT profile) or `webvtt` (bounded WebVTT profile).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Srt,
    Webvtt,
}
/// Simultaneous-cue policy: `allow` keeps overlapping cues; `reject` fails validation if any cues overlap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Overlap {
    Allow,
    Reject,
}
/// Export loss policy: `reject` refuses a format that drops cue fields; `allow_reported` permits the losses captions.encode reports.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LossPolicy {
    Reject,
    AllowReported,
}
/// Caption style: a whole-cue text color.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Style {
    /// Text color as `[r, g, b]`, each 0-255 (sRGB).
    pub color: [u8; 3],
}
/// One timed caption over the half-open interval `[start, end)`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cue {
    /// Unique cue ID: 1-64 ASCII letters, digits, `_` or `-`; not `STYLE`, `NOTE` or `REGION`.
    pub id: String,
    /// Inclusive start in rational seconds `{num, den}`; cues must be in nondecreasing start order.
    pub start: Time,
    /// Exclusive end in rational seconds; after `start` and at most 24 hours.
    pub end: Time,
    /// 1-1024 Unicode scalars (at most 4096 bytes); LF between nonblank lines, no other control characters.
    pub text: String,
    /// ID of a style defined in the document's `styles`.
    pub style: String,
    /// Horizontal alignment of each line within the caption box.
    pub align: Align,
    /// Optional speaker metadata, 1-128 trimmed bytes without controls, `<`, `>` or `&`; never rendered.
    pub speaker: Option<String>,
}
/// Native caption document: styles and time-ordered cues, edited immutably by captions.apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Document format version; must be 1.
    pub schema_version: u32,
    /// Document ID: 1-64 ASCII letters, digits, `_` or `-`.
    pub id: String,
    /// Edit counter incremented by each captions.apply; pass it as `expected_revision`.
    pub revision: u64,
    /// Whether cues may overlap in time.
    pub overlap: Overlap,
    /// Up to 32 styles keyed by style ID (cue ID rules, starting with a letter or `_`).
    pub styles: BTreeMap<String, Style>,
    /// Up to 4096 cues sorted by start; at most 1 MiB of text in total.
    pub cues: Vec<Cue>,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_CAPTIONS", message)
}
fn unsupported(message: &str) -> crate::Error {
    error("UNSUPPORTED_CAPTIONS", message)
}
fn id_ok(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        && !matches!(id, "STYLE" | "NOTE" | "REGION")
}
fn style_ok(id: &str) -> bool {
    id_ok(id)
        && id
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
}
fn text_ok(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 4096
        && text.chars().count() <= 1024
        && !text.ends_with('\n')
        && text.split('\n').all(|l| !l.trim().is_empty())
        && text.chars().all(|c| c == '\n' || !c.is_control())
}
fn speaker_ok(text: &str) -> bool {
    !text.trim().is_empty()
        && text.len() <= 128
        && text.trim() == text
        && !text
            .chars()
            .any(|c| c.is_control() || matches!(c, '<' | '>' | '&'))
}
impl Document {
    pub fn validate(&self) -> Result<usize> {
        if self.schema_version != 1
            || !id_ok(&self.id)
            || self.revision > 9_007_199_254_740_991
            || self.cues.len() > 4096
            || self.styles.len() > 32
            || self.styles.keys().any(|s| !style_ok(s))
        {
            return Err(invalid(
                "Caption v1 requires bounded IDs, safe revision, at most 4096 cues and 32 styles",
            ));
        }
        let mut ids = HashSet::new();
        let mut previous = Time::ZERO;
        let mut bytes = 0usize;
        let mut events = Vec::new();
        for cue in &self.cues {
            cue.start.validate()?;
            cue.end.validate()?;
            if !id_ok(&cue.id)
                || !ids.insert(&cue.id)
                || !self.styles.contains_key(&cue.style)
                || !text_ok(&cue.text)
                || cue.speaker.as_ref().is_some_and(|s| !speaker_ok(s))
                || cue.start.compare(cue.end)? != Ordering::Less
                || cue.end.compare(DAY)? == Ordering::Greater
                || cue.start.compare(previous)? == Ordering::Less
            {
                return Err(invalid(
                    "Cues need unique IDs, defined styles, supported text, increasing nonempty intervals within 24 hours, and chronological start order",
                ));
            }
            previous = cue.start;
            bytes += cue.text.len();
            events.push((cue.start, 1i32));
            events.push((cue.end, -1i32));
        }
        if bytes > 1024 * 1024 {
            return Err(error("LIMIT_EXCEEDED", "Caption text exceeds 1 MiB"));
        }
        // End events precede starts at the same exact instant: intervals are half-open.
        events.sort_by(|a, b| {
            a.0.compare(b.0)
                .expect("validated bounded times")
                .then(a.1.cmp(&b.1))
        });
        let (mut count, mut maximum) = (0i32, 0i32);
        for (_, delta) in events {
            count += delta;
            maximum = maximum.max(count);
        }
        if self.overlap == Overlap::Reject && maximum > 1 {
            return Err(error(
                "CAPTION_OVERLAP",
                "Overlapping cues require overlap: allow",
            ));
        }
        Ok(maximum as usize)
    }
}
pub fn inspect(document: &Document) -> Result<Value> {
    let maximum = document.validate()?;
    let mut usage = BTreeMap::new();
    for cue in &document.cues {
        *usage.entry(&cue.style).or_insert(0usize) += 1;
    }
    let end = document
        .cues
        .iter()
        .map(|c| c.end)
        .max_by(|a, b| a.compare(*b).expect("validated time"))
        .unwrap_or(Time::ZERO);
    Ok(
        json!({"document_id":document.id,"revision":document.revision,"cue_count":document.cues.len(),"end":end,"maximum_simultaneous":maximum,"overlap":document.overlap,"style_usage":usage,"text_bytes":document.cues.iter().map(|c|c.text.len()).sum::<usize>(),"clock":"exact_rational_half_open"}),
    )
}
fn digits(s: &str, width: usize) -> Result<u64> {
    if (width > 0 && s.len() != width) || s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return Err(invalid("Malformed caption timestamp or counter"));
    }
    s.parse().map_err(|_| invalid("Caption integer overflow"))
}
fn timestamp(s: &str, format: Format) -> Result<Time> {
    let delim = if format == Format::Srt { ',' } else { '.' };
    let (clock, millis) = s
        .split_once(delim)
        .ok_or_else(|| invalid("Expected millisecond timestamp"))?;
    let parts: Vec<_> = clock.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] if h.len() >= 2 => (digits(h, 0)?, digits(m, 2)?, digits(s, 2)?),
        [m, s] if format == Format::Webvtt => (0, digits(m, 2)?, digits(s, 2)?),
        _ => return Err(invalid("Timestamp must use hh:mm:ss or WebVTT mm:ss")),
    };
    let ms = digits(millis, 3)?;
    if h > 24 || m >= 60 || sec >= 60 {
        return Err(invalid("Timestamp component out of range"));
    }
    Time::new(((h * 60 + m) * 60 + sec) * 1000 + ms, 1000)
}
fn unescape(s: &str) -> Result<String> {
    let mut result = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        result.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest
            .find(';')
            .ok_or_else(|| unsupported("Unterminated character reference"))?;
        let reference = &rest[..=end];
        result.push(match reference {
            "&amp;" => '&',
            "&lt;" => '<',
            "&gt;" => '>',
            "&nbsp;" => '\u{a0}',
            "&lrm;" => '\u{200e}',
            "&rlm;" => '\u{200f}',
            _ => {
                return Err(unsupported(
                    "Only amp/lt/gt/nbsp/lrm/rlm character references are supported",
                ));
            }
        });
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
fn escaped(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn css(block: &str, styles: &mut BTreeMap<String, Style>) -> Result<()> {
    let mut rest = block.trim();
    if rest.is_empty() {
        return Err(unsupported("Empty style block"));
    }
    while !rest.is_empty() {
        rest = rest
            .strip_prefix("::cue(.")
            .ok_or_else(|| unsupported("Only class color rules are supported"))?;
        let (name, tail) = rest
            .split_once(')')
            .ok_or_else(|| invalid("Unclosed style selector"))?;
        if !style_ok(name) || styles.contains_key(name) {
            return Err(invalid("Invalid or repeated caption style"));
        }
        let tail = tail
            .trim_start()
            .strip_prefix('{')
            .ok_or_else(|| invalid("Missing style body"))?;
        let (body, tail) = tail
            .split_once('}')
            .ok_or_else(|| invalid("Unclosed style body"))?;
        let color = body
            .trim()
            .strip_prefix("color")
            .and_then(|s| s.trim_start().strip_prefix(':'))
            .ok_or_else(|| unsupported("Only color style declarations are supported"))?
            .trim();
        let color = color.strip_suffix(';').unwrap_or(color).trim();
        if color.len() != 7
            || !color.starts_with('#')
            || !color[1..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(unsupported("Color must use exactly six hexadecimal digits"));
        }
        let color = [
            u8::from_str_radix(&color[1..3], 16).unwrap(),
            u8::from_str_radix(&color[3..5], 16).unwrap(),
            u8::from_str_radix(&color[5..7], 16).unwrap(),
        ];
        styles.insert(name.into(), Style { color });
        rest = tail.trim_start();
    }
    Ok(())
}
fn payload(s: &str, format: Format) -> Result<(String, String, Option<String>)> {
    if format == Format::Srt {
        if s.contains(['<', '>', '&']) {
            return Err(unsupported(
                "SRT profile accepts plain text without markup or entities",
            ));
        }
        return Ok((s.into(), "default".into(), None));
    }
    let mut rest = s;
    let mut style = None;
    let mut speaker = None;
    for _ in 0..2 {
        if let Some(start) = rest.strip_prefix("<v ") {
            if speaker.is_some() {
                return Err(unsupported("Nested voice spans are unsupported"));
            }
            let (name, text) = start
                .split_once('>')
                .ok_or_else(|| invalid("Unclosed voice span"))?;
            if !speaker_ok(name) {
                return Err(unsupported("Unsupported speaker annotation"));
            }
            speaker = Some(name.into());
            rest = text.strip_suffix("</v>").unwrap_or(text);
        } else if let Some(start) = rest.strip_prefix("<c.") {
            if style.is_some() {
                return Err(unsupported("Nested class spans are unsupported"));
            }
            let (name, text) = start
                .split_once('>')
                .ok_or_else(|| invalid("Unclosed class span"))?;
            if !style_ok(name) {
                return Err(unsupported("One bounded class name per cue is supported"));
            }
            style = Some(name.into());
            rest = text
                .strip_suffix("</c>")
                .ok_or_else(|| invalid("Unclosed class span"))?;
        } else {
            break;
        }
    }
    if rest.contains('<') || rest.contains("-->") {
        return Err(unsupported(
            "Inline markup, timestamps and cue arrows are unsupported in text",
        ));
    }
    Ok((unescape(rest)?, style.unwrap_or_default(), speaker))
}
fn parse(text: &str, format: Format, id: &str, overlap: Overlap) -> Result<Document> {
    if text.len() > LIMIT {
        return Err(error("LIMIT_EXCEEDED", "Caption source exceeds 2 MiB"));
    }
    let text = text
        .strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut blocks = Vec::new();
    let mut block = Vec::new();
    for line in text.split('\n') {
        if line.trim().is_empty() {
            if !block.is_empty() {
                blocks.push(std::mem::take(&mut block));
            }
        } else {
            block.push(line);
        }
    }
    if !block.is_empty() {
        blocks.push(block);
    }
    if format == Format::Webvtt {
        if blocks.first() != Some(&vec!["WEBVTT"]) {
            return Err(unsupported(
                "Expected a standalone WEBVTT header and blank line",
            ));
        }
        blocks.remove(0);
    }
    let mut document = Document {
        schema_version: 1,
        id: id.into(),
        revision: 0,
        overlap,
        styles: BTreeMap::new(),
        cues: Vec::new(),
    };
    let mut missing_ids = Vec::new();
    for block in blocks {
        if block[0] == "STYLE" && format == Format::Webvtt {
            if !document.cues.is_empty() {
                return Err(unsupported("Style blocks must precede cues"));
            }
            css(&block[1..].join("\n"), &mut document.styles)?;
            continue;
        }
        let timing = if block[0].contains("-->") && format == Format::Webvtt {
            0
        } else {
            1
        };
        if block.len() < timing + 2 {
            return Err(invalid("Caption block needs timing and nonempty text"));
        }
        let id = if timing == 0 {
            missing_ids.push(document.cues.len());
            String::new()
        } else {
            if format == Format::Srt && digits(block[0], 0)? == 0 {
                return Err(invalid("SRT counters must be positive"));
            }
            block[0].into()
        };
        let fields: Vec<_> = block[timing].split_ascii_whitespace().collect();
        if fields.len() < 3 || fields[1] != "-->" {
            return Err(invalid("Malformed cue timing line"));
        }
        let mut align = Align::Center;
        if fields.len() > 3 {
            if format != Format::Webvtt || fields.len() != 4 {
                return Err(unsupported("Only one align setting is supported"));
            }
            align = match fields[3] {
                "align:left" => Align::Left,
                "align:center" => Align::Center,
                "align:right" => Align::Right,
                _ => return Err(unsupported("Unsupported cue setting")),
            };
        }
        let (text, style, speaker) = payload(&block[timing + 1..].join("\n"), format)?;
        document.cues.push(Cue {
            id,
            start: timestamp(fields[0], format)?,
            end: timestamp(fields[2], format)?,
            text,
            style,
            align,
            speaker,
        });
    }
    let mut used: HashSet<_> = document
        .cues
        .iter()
        .filter(|c| !c.id.is_empty())
        .map(|c| c.id.clone())
        .collect();
    for i in missing_ids {
        let mut n = i + 1;
        loop {
            let id = format!("cue-{n}");
            if used.insert(id.clone()) {
                document.cues[i].id = id;
                break;
            }
            n += 1;
        }
    }
    // A CSS class named "default" applies only to explicitly classed text.
    // Give unclassed cues a separate white style when that name is already colored.
    if document.cues.iter().any(|c| c.style.is_empty()) {
        let mut name = "default".to_string();
        let mut suffix = 0;
        while document
            .styles
            .get(&name)
            .is_some_and(|s| s.color != [255; 3])
            || (suffix > 0 && document.cues.iter().any(|c| c.style == name))
        {
            suffix += 1;
            name = format!("plain-{suffix}");
        }
        document
            .styles
            .entry(name.clone())
            .or_insert(Style { color: [255; 3] });
        for cue in &mut document.cues {
            if cue.style.is_empty() {
                cue.style = name.clone();
            }
        }
    }
    if document.cues.iter().any(|c| c.style == "default") {
        document
            .styles
            .entry("default".into())
            .or_insert(Style { color: [255; 3] });
    }
    for cue in &document.cues {
        if !document.styles.contains_key(&cue.style) {
            let color = match cue.style.as_str() {
                "white" => Some([255, 255, 255]),
                "lime" => Some([0, 255, 0]),
                "cyan" => Some([0, 255, 255]),
                "red" => Some([255, 0, 0]),
                "yellow" => Some([255, 255, 0]),
                "magenta" => Some([255, 0, 255]),
                "blue" => Some([0, 0, 255]),
                "black" => Some([0, 0, 0]),
                _ => None,
            };
            if let Some(color) = color {
                document.styles.insert(cue.style.clone(), Style { color });
            }
        }
    }
    document.validate()?;
    Ok(document)
}
pub fn import(
    source: &Identity,
    root: &Path,
    format: Format,
    id: &str,
    overlap: Overlap,
) -> Result<Value> {
    if source.bytes > LIMIT as u64 {
        return Err(error("LIMIT_EXCEEDED", "Caption source exceeds 2 MiB"));
    }
    let (_, bytes) = scene::identity_bytes(source, root)?;
    let text =
        std::str::from_utf8(&bytes).map_err(|_| unsupported("Caption files must be UTF-8"))?;
    let document = parse(text, format, id, overlap)?;
    Ok(
        json!({"inspection":inspect(&document)?,"document":document,"source":source,"format":format}),
    )
}
/// One caption edit for captions.apply, tagged by `op`; the final document must validate.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", deny_unknown_fields)]
#[schemars(rename = "CaptionOperation")]
pub enum Operation {
    /// Add a complete cue whose ID is not already used.
    #[serde(rename = "cue.add")]
    Add {
        /// The new cue.
        cue: Cue,
    },
    /// Replace the existing cue that has the same ID.
    #[serde(rename = "cue.replace")]
    Replace {
        /// Complete replacement cue; its `id` selects the cue to replace.
        cue: Cue,
    },
    /// Remove an existing cue.
    #[serde(rename = "cue.remove")]
    Remove {
        /// ID of the cue to remove.
        cue_id: String,
    },
    /// Add or replace a style definition.
    #[serde(rename = "style.set")]
    SetStyle {
        /// Style ID to add or replace.
        style_id: String,
        /// New style definition.
        style: Style,
    },
    /// Remove a style; no cue in the final document may still reference it.
    #[serde(rename = "style.remove")]
    RemoveStyle {
        /// ID of an existing style.
        style_id: String,
    },
    /// Change the document's overlap policy.
    #[serde(rename = "overlap.set")]
    SetOverlap {
        /// New overlap policy.
        overlap: Overlap,
    },
    /// Move existing cues by an exact time offset.
    #[serde(rename = "cues.shift")]
    Shift {
        /// 1-4096 unique IDs of existing cues to move.
        cue_ids: Vec<String>,
        /// Nonnegative shift in rational seconds `{num, den}`.
        offset: Time,
        /// True moves cues earlier; false moves them later.
        backward: bool,
    },
}
pub fn apply(
    document: &Document,
    expected_revision: u64,
    operations: &[Operation],
) -> Result<Value> {
    document.validate()?;
    if document.revision != expected_revision {
        return Err(error(
            "REVISION_CONFLICT",
            "Caption revision differs from expected_revision",
        ));
    }
    if operations.is_empty() || operations.len() > 256 {
        return Err(invalid("Use 1-256 caption operations per batch"));
    }
    let mut result = document.clone();
    for op in operations {
        match op {
            Operation::Add { cue } => {
                if result.cues.iter().any(|c| c.id == cue.id) {
                    return Err(invalid("Cue ID already exists"));
                }
                result.cues.push(cue.clone());
            }
            Operation::Replace { cue } => {
                let old = result
                    .cues
                    .iter_mut()
                    .find(|c| c.id == cue.id)
                    .ok_or_else(|| invalid("Cue ID not found"))?;
                *old = cue.clone();
            }
            Operation::Remove { cue_id } => {
                let i = result
                    .cues
                    .iter()
                    .position(|c| &c.id == cue_id)
                    .ok_or_else(|| invalid("Cue ID not found"))?;
                result.cues.remove(i);
            }
            Operation::SetStyle { style_id, style } => {
                result.styles.insert(style_id.clone(), style.clone());
            }
            Operation::RemoveStyle { style_id } => {
                if result.styles.remove(style_id).is_none() {
                    return Err(invalid("Style ID not found"));
                }
            }
            Operation::SetOverlap { overlap } => result.overlap = *overlap,
            Operation::Shift {
                cue_ids,
                offset,
                backward,
            } => {
                offset.validate()?;
                let mut seen = HashSet::new();
                if cue_ids.is_empty() || cue_ids.len() > 4096 {
                    return Err(invalid("Shift requires 1-4096 cue IDs"));
                }
                for id in cue_ids {
                    if !seen.insert(id) {
                        return Err(invalid("Repeated shift cue ID"));
                    }
                    let cue = result
                        .cues
                        .iter_mut()
                        .find(|c| &c.id == id)
                        .ok_or_else(|| invalid("Cue ID not found"))?;
                    cue.start = if *backward {
                        cue.start.minus(*offset)?
                    } else {
                        cue.start.plus(*offset)?
                    };
                    cue.end = if *backward {
                        cue.end.minus(*offset)?
                    } else {
                        cue.end.plus(*offset)?
                    };
                }
            }
        }
    }
    // Validate before sorting so malformed times never enter an infallible comparator.
    for cue in &result.cues {
        cue.start.validate()?;
        cue.end.validate()?;
    }
    result
        .cues
        .sort_by(|a, b| a.start.compare(b.start).expect("validated times"));
    result.revision += 1;
    let inspection = inspect(&result)?;
    let old: BTreeMap<_, _> = document.cues.iter().map(|c| (&c.id, c)).collect();
    let new: BTreeMap<_, _> = result.cues.iter().map(|c| (&c.id, c)).collect();
    let changes: Vec<_> = old
        .keys()
        .chain(new.keys())
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter(|id| old.get(id) != new.get(id))
        .map(|id| json!({"cue_id":id,"before":old.get(id),"after":new.get(id)}))
        .collect();
    Ok(
        json!({"document":result,"inspection":inspection,"changes":changes,"previous_revision":document.revision}),
    )
}
fn stamp(time: Time, format: Format) -> Result<String> {
    let n = time.units(Time { num: 1000, den: 1 })?;
    Ok(format!(
        "{:02}:{:02}:{:02}{}{:03}",
        n / 3_600_000,
        n / 60_000 % 60,
        n / 1000 % 60,
        if format == Format::Srt { ',' } else { '.' },
        n % 1000
    ))
}
pub fn encode(document: &Document, format: Format) -> Result<Value> {
    document.validate()?;
    if format == Format::Srt && document.cues.is_empty() {
        return Err(invalid("An SRT export needs at least one cue"));
    }
    let mut text = if format == Format::Webvtt {
        "WEBVTT\n\n".to_string()
    } else {
        String::new()
    };
    let mut losses = Vec::new();
    let mut ids = Vec::new();
    if format == Format::Srt {
        let dropped: Vec<_> = document
            .styles
            .iter()
            .filter(|(id, s)| id.as_str() != "default" || s.color != [255; 3])
            .map(|(id, _)| id)
            .collect();
        if !dropped.is_empty() {
            losses.push(json!({"style_ids":dropped,"fields":["style_definitions"]}));
        }
    }
    if format == Format::Webvtt && !document.styles.is_empty() {
        text.push_str("STYLE\n");
        for (name, style) in &document.styles {
            let c = style.color;
            text.push_str(&format!(
                "::cue(.{name}) {{ color: #{:02x}{:02x}{:02x}; }}\n",
                c[0], c[1], c[2]
            ));
        }
        text.push('\n');
    }
    for (i, cue) in document.cues.iter().enumerate() {
        let id = if format == Format::Srt {
            (i + 1).to_string()
        } else {
            cue.id.clone()
        };
        let mut lost = Vec::new();
        if format == Format::Srt {
            if cue.id != id {
                lost.push("cue_id");
            }
            if cue.style != "default" || document.styles[&cue.style].color != [255; 3] {
                lost.push("style");
            }
            if cue.align != Align::Center {
                lost.push("alignment");
            }
            if cue.speaker.is_some() {
                lost.push("speaker");
            }
            if cue.text.contains(['<', '>', '&']) {
                return Err(unsupported(&format!(
                    "Cue {:?}: the SRT plain-text profile cannot encode angle brackets or ampersands; use WebVTT",
                    cue.id
                )));
            }
        }
        if !lost.is_empty() {
            losses.push(json!({"cue_id":cue.id,"fields":lost}));
        }
        ids.push(json!({"cue_id":cue.id,"exported_id":id}));
        let align = match cue.align {
            Align::Left => "left",
            Align::Center => "center",
            Align::Right => "right",
        };
        let settings = if format == Format::Webvtt {
            format!(" align:{align}")
        } else {
            String::new()
        };
        text.push_str(&format!(
            "{id}\n{} --> {}{settings}\n",
            stamp(cue.start, format)?,
            stamp(cue.end, format)?
        ));
        if format == Format::Webvtt {
            if let Some(s) = &cue.speaker {
                text.push_str(&format!("<v {s}>"));
            }
            text.push_str(&format!("<c.{}>{}</c>", cue.style, escaped(&cue.text)));
            if cue.speaker.is_some() {
                text.push_str("</v>");
            }
        } else {
            text.push_str(&cue.text);
        }
        text.push_str("\n\n");
    }
    if text.len() > LIMIT {
        return Err(error("LIMIT_EXCEEDED", "Encoded captions exceed 2 MiB"));
    }
    Ok(
        json!({"format":format,"text":text,"bytes":text.len(),"sha256":format!("{:x}",Sha256::digest(text.as_bytes())),"document_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(document)?)),"losses":losses,"cue_id_map":ids,"cue_count":document.cues.len()}),
    )
}
pub fn export(
    document: &Document,
    format: Format,
    loss_policy: LossPolicy,
    output_root: &Path,
    output: &Path,
) -> Result<Value> {
    let output = render::destination_extension(
        output,
        output_root,
        if format == Format::Srt { "srt" } else { "vtt" },
    )?;
    let mut report = encode(document, format)?;
    if matches!(loss_policy, LossPolicy::Reject) && !report["losses"].as_array().unwrap().is_empty()
    {
        return Err(error(
            "LOSSY_CAPTION_EXPORT",
            "Requested format loses cue fields; inspect captions.encode and explicitly use allow_reported",
        ));
    }
    let scratch = scene::Scratch::new(output.parent().expect("validated output"))?;
    let temp = scratch.0.join("captions.txt");
    let mut file = File::create_new(&temp)?;
    let text = report["text"].as_str().unwrap();
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    if fs::read(&temp)? != text.as_bytes() {
        return Err(error(
            "EXPORT_VALIDATION_FAILED",
            "Caption output bytes differ",
        ));
    }
    media::publish(&temp, &output)?;
    report["output"] = json!(output);
    Ok(report)
}
/// Text layout applied to every visible cue of one style in captions.scene.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    /// 1-4 font file identities `{path, bytes, sha256}` under `input_root`, in fallback order.
    pub fonts: Vec<Identity>,
    /// Font em size in pixels, 1-512.
    pub size: u16,
    /// Text box `[x, y, width, height]` in scene pixels; must lie inside the canvas.
    pub rect: [i32; 4],
    /// Baseline-to-baseline distance in pixels, 1-2048.
    pub line_height: u16,
    /// Extra pixels between characters on the same line, 0-128.
    pub letter_spacing: u16,
    /// Line wrapping mode.
    pub wrap: Wrap,
    /// Behavior when text does not fit the box.
    pub overflow: Overflow,
    /// Optional Unicode text profile for shaping and mixed directions; omit for the scalar layout.
    #[serde(default)]
    pub text_layout: Option<crate::graphics::TextLayout>,
    /// Vertical alignment of each cue's lines in `rect`; `bottom` keeps one- and two-line cues on the same bottom line. Default `top`.
    #[serde(default, skip_serializing_if = "crate::graphics::VAlign::is_top")]
    pub valign: crate::graphics::VAlign,
    /// Optional box behind each cue line for legibility over video; omit for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background: Option<crate::graphics::TextBackground>,
    /// Optional outline around the glyphs; omit for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outline: Option<crate::graphics::TextOutline>,
}
/// Caption sampling policy; only `sample_start`: output frame n shows caption time `offset + n/25`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    SampleStart,
}
/// Request for captions.scene: render one caption window as text layers appended to a base scene.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SceneRequest {
    /// Caption document to sample; it is not changed.
    pub document: Document,
    /// Base scene to extend; duration must be 1 frame to 120 seconds of whole frames at its frame_rate.
    pub scene: Scene,
    /// ID of the returned scene.
    pub scene_id: String,
    /// Caption time in rational seconds `{num, den}` shown at output frame 0; at most 24 hours.
    pub offset: Time,
    /// Layout per style ID; keys must be defined styles, and every style with a sampled cue needs one.
    pub layouts: BTreeMap<String, Layout>,
    /// Frame sampling policy.
    pub sampling: Sampling,
    /// Prefix for generated layer IDs `<prefix>-<cue_id>`: 1-32 ASCII letters, digits, `_` or `-`.
    pub layer_prefix: String,
    /// Absolute directory containing the base scene's media and the layout fonts.
    pub input_root: PathBuf,
}
fn ceil_frames(t: Time, rate: Time) -> u64 {
    (t.num as u128 * rate.num as u128).div_ceil(t.den as u128 * rate.den as u128) as u64
}
pub fn to_scene(request: &SceneRequest) -> Result<Value> {
    let d = &request.document;
    d.validate()?;
    request.offset.validate()?;
    let rate = request.scene.clock()?;
    let frames = request.scene.duration.units(rate)?;
    if !(1..=scene::max_frames(rate)).contains(&frames)
        || request.offset.compare(DAY)? == Ordering::Greater
        || !id_ok(&request.layer_prefix)
        || request.layer_prefix.len() > 32
        || request.layouts.keys().any(|k| !d.styles.contains_key(k))
    {
        return Err(invalid(&format!(
            "Caption scenes require 1 frame to {} seconds at the scene's frame_rate, a bounded offset/prefix and known layout style IDs",
            scene::MAX_SECONDS
        )));
    }
    let window_end = request.offset.plus(request.scene.duration)?;
    let mut result = request.scene.clone();
    result.id = request.scene_id.clone();
    let mut cues = Vec::new();
    let mut layer_ids: HashSet<_> = result.layers.iter().map(|l| l.id.clone()).collect();
    for cue in &d.cues {
        if cue.end.compare(request.offset)? != Ordering::Greater
            || cue.start.compare(window_end)? != Ordering::Less
        {
            cues.push(json!({"cue_id":cue.id,"status":"outside_window"}));
            continue;
        }
        let start = if cue.start.compare(request.offset)? == Ordering::Less {
            Time::ZERO
        } else {
            cue.start.minus(request.offset)?
        };
        let end = if cue.end.compare(window_end)? == Ordering::Greater {
            request.scene.duration
        } else {
            cue.end.minus(request.offset)?
        };
        let (first, last) = (ceil_frames(start, rate), ceil_frames(end, rate));
        if first == last {
            cues.push(json!({"cue_id":cue.id,"status":"no_sampled_frame"}));
            continue;
        }
        let layout = request
            .layouts
            .get(&cue.style)
            .ok_or_else(|| invalid("Visible cue has no style layout"))?;
        let id = format!("{}-{}", request.layer_prefix, cue.id);
        if !layer_ids.insert(id.clone()) {
            return Err(invalid("Caption layer ID collides with the base scene"));
        }
        let rgb = d.styles[&cue.style].color;
        result.layers.push(scene::Layer {
            id: id.clone(),
            canvas: [result.width, result.height],
            start: Time::new(first * rate.den, rate.num)?,
            duration: Time::new((last - first) * rate.den, rate.num)?,
            frames: Vec::new(),
            tilemap: None,
            graphics: Some(Graphic::Text {
                text: cue.text.clone(),
                fonts: layout.fonts.clone(),
                size: layout.size,
                color: [rgb[0], rgb[1], rgb[2], 255],
                rect: layout.rect,
                line_height: layout.line_height,
                letter_spacing: layout.letter_spacing,
                align: cue.align.clone(),
                valign: layout.valign,
                background: layout.background.clone(),
                outline: layout.outline.clone(),
                wrap: layout.wrap.clone(),
                overflow: layout.overflow.clone(),
                layout: layout.text_layout.clone(),
            }),
            timing: scene::Timing::Strict,
            end: scene::End::HoldLast,
            transform: scene::Transform {
                position: [0, 0],
                crop: [0, 0, result.width, result.height],
                scale: 1,
                quarter_turns: 0,
                opacity: 255,
                spatial: None,
            },
            animation: None,
            alpha_mode: crate::composite::AlphaMode::Straight,
            blend_mode: crate::composite::BlendMode::Normal,
            mask: None,
            effects: Vec::new(),
        });
        cues.push(json!({"cue_id":cue.id,"status":"sampled","layer_id":id,"first_frame":first,"end_frame":last,"source_start":cue.start,"source_end":cue.end}));
    }
    let inspection = scene::inspect(&result, &request.input_root)?;
    Ok(
        json!({"scene":result,"inspection":inspection,"cues":cues,"sampling":"sample_start","offset":request.offset,"overlap_order":"later_document_cues_over_earlier","speaker_rendering":"metadata_only"}),
    )
}
pub fn capabilities() -> Value {
    json!({"schema_version":1,"formats":["srt","webvtt"],"encoding":"utf8","time":"exact_rational","export_time":"exact_milliseconds_no_rounding","maximum_cues":4096,"maximum_styles":32,"maximum_hours":24,"style_fields":["color"],"cue_fields":["text","start","end","style","align","speaker"],"rendering":"explicit_per_style_fonts_and_boxes_sample_start","overlap_policies":["allow","reject"],"export_loss_policies":["reject","allow_reported"]})
}
