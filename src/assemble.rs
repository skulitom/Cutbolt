//! Paper edits: assemble a sequential timeline from transcript word selections. Each selection
//! becomes one `clip.append` of the selection's source, widened by optional padding and to whole
//! frames so no selected word is clipped.
use crate::{Result, error, model::Project, time::Time, transcript::Document};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

/// Selections per request.
const MAX_SELECTIONS: usize = 500;

/// A run of words to use, inclusive, from one transcript.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    /// ID of one of the given transcripts.
    pub document_id: String,
    /// First word of the run.
    pub first_word_id: String,
    /// Last word of the run, at or after the first.
    pub last_word_id: String,
}

/// What to assemble.
pub struct Request<'a> {
    pub project: &'a Project,
    pub transcripts: &'a [Document],
    pub selections: &'a [Selection],
    pub input_root: Option<&'a Path>,
    pub padding: Time,
    pub clip_prefix: &'a str,
}

/// Propose `clip.append` operations for the selections, in order.
pub fn propose(request: &Request) -> Result<Value> {
    let project = request.project;
    project.validate()?;
    if project.tracks.is_some() {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Assemble into a sequential timeline, then promote it to tracks",
        ));
    }
    if request.selections.is_empty() || request.selections.len() > MAX_SELECTIONS {
        return Err(error(
            "INVALID_ARGUMENT",
            format!("selections must list 1-{MAX_SELECTIONS} word runs"),
        ));
    }
    if request.padding.compare(Time::new(2, 1)?)?.is_gt() {
        return Err(error("INVALID_ARGUMENT", "padding must be at most 2 s"));
    }
    crate::tracks::id(request.clip_prefix)?;
    let rate = crate::render::clock::rate(project.frame_rate)?;
    let matches = crate::outline::spoken(project, request.transcripts, request.input_root)?;
    let mut used: BTreeSet<String> = project.clips.iter().map(|c| c.id.clone()).collect();
    let mut next = 1;
    let mut operations = Vec::new();
    let mut clips = Vec::new();
    let mut total = Time::ZERO;
    for (index, selection) in request.selections.iter().enumerate() {
        let at = || format!("selections[{index}]");
        let document = request
            .transcripts
            .iter()
            .find(|d| d.id == selection.document_id)
            .ok_or_else(|| {
                crate::missing(
                    "MISSING_TRANSCRIPT",
                    "transcript",
                    &selection.document_id,
                    request.transcripts.iter().map(|d| d.id.as_str()),
                )
            })?;
        let asset_id = matches.by_document[document.id.as_str()]
            .first()
            .ok_or_else(|| {
                error(
                    "MISSING_ASSET",
                    format!(
                        "{}: no project asset has transcript {:?}'s source {}; add it with media.add",
                        at(),
                        document.id,
                        document.source.path.display()
                    ),
                )
            })?
            .clone();
        let position = |id: &str| {
            document
                .words
                .iter()
                .position(|w| w.id == id)
                .ok_or_else(|| {
                    crate::missing(
                        "MISSING_WORD",
                        "word",
                        id,
                        document.words.iter().map(|w| w.id.as_str()),
                    )
                })
        };
        let (first, last) = (
            position(&selection.first_word_id)?,
            position(&selection.last_word_id)?,
        );
        if last < first {
            return Err(error(
                "INVALID_ARGUMENT",
                format!("{}: the last word comes before the first", at()),
            ));
        }
        let asset = project.asset(&asset_id)?;
        let start = document.words[first]
            .start
            .minus(request.padding)
            .unwrap_or(Time::ZERO);
        let mut end = document.words[last].end.plus(request.padding)?;
        if end.compare(asset.duration)?.is_gt() {
            end = asset.duration;
        }
        // Whole frames, widened so every selected word is kept, but inside the asset.
        let frame = |t: Time, up: bool| -> u64 {
            let n = t.num as u128 * rate.num as u128;
            let d = t.den as u128 * rate.den as u128;
            (if up { n.div_ceil(d) } else { n / d }) as u64
        };
        let first_frame = frame(start, false);
        let last_frame = frame(end, true).min(frame(asset.duration, false));
        if last_frame <= first_frame {
            return Err(error(
                "INVALID_RANGE",
                format!("{}: the selection is shorter than one frame", at()),
            ));
        }
        let source_in = Time::new(first_frame * rate.den, rate.num)?;
        let duration = Time::new((last_frame - first_frame) * rate.den, rate.num)?;
        let id = loop {
            let id = format!("{}{next}", request.clip_prefix);
            next += 1;
            if used.insert(id.clone()) {
                break id;
            }
        };
        let words: Vec<&str> = document.words[first..=last]
            .iter()
            .map(|w| w.said())
            .collect();
        let text = if words.len() <= 12 {
            words.join(" ")
        } else {
            format!(
                "{} ... {}",
                words[..6].join(" "),
                words[words.len() - 6..].join(" ")
            )
        };
        total = total.plus(duration)?;
        clips.push(
            json!({"clip_id":id,"asset_id":asset_id,"source_in":source_in,"duration":duration,
            "words":words.len(),"text":text}),
        );
        operations.push(json!({"op":"clip.append","clip":{"id":id,"asset_id":asset_id,"source_in":source_in,"duration":duration}}));
    }
    // The batch is checked to apply as a whole.
    let parsed: Vec<crate::model::Operation> = serde_json::from_value(json!(operations))?;
    let assembled = project.apply(project.revision, parsed)?;
    Ok(
        json!({"operations":operations,"clips":clips,"added":total,"duration_after":assembled.duration()?,
        "next":"apply operations with session.apply, then read the cut with timeline.outline"}),
    )
}
