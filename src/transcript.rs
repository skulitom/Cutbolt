//! Original content-bound transcript records and explicit exact-time correction.
//! Recognition is optional; a returned estimate is not a reviewed cut boundary.
use crate::{Result, error, media, registry::Identity, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

pub(crate) const RATE: Time = Time {
    num: 48_000,
    den: 1,
};
pub(crate) const MAX_WORDS: usize = 2048;
pub(crate) const MAX_RANGE: Time = Time { num: 120, den: 1 };
/// Terms one recognition vocabulary may hold.
pub(crate) const MAX_TERMS: usize = 32;

/// A vocabulary term: 1-64 bytes, without control characters or surrounding whitespace.
pub(crate) fn term_ok(term: &str) -> bool {
    !term.is_empty()
        && term.len() <= 64
        && term.trim() == term
        && !term.chars().any(char::is_control)
}

/// Spoken language: `en` (English) or `el` (Greek).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    En,
    El,
}
/// Word provenance: `estimated` (recognizer output, needs review) or `corrected` (text and times supplied by the caller).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Estimated,
    Corrected,
}

/// Identity-bound local source media of a transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Relative path of normal components under `input_root`.
    pub path: PathBuf,
    /// Content identity: SHA-256 hex plus byte count of the source file.
    pub identity: Identity,
    /// Positive total source duration in rational seconds, a whole number of 48 kHz samples.
    pub duration: Time,
}
/// Caller-asserted recognition provenance; it does not authenticate a recognizer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recognition {
    /// Recognition profile name, 1-128 bytes without control characters.
    pub profile: String,
    /// Content identity of the recognition model file.
    pub model: Identity,
    /// SHA-256 hex of the recognition worker.
    pub worker_sha256: String,
    /// Optional SHA-256 hex of the worker supervisor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supervisor_sha256: Option<String>,
    /// SHA-256 hex of the analysis audio given to the recognizer.
    pub analysis_sha256: String,
    /// 1-64 component name to version entries, each string 1-128 bytes.
    pub versions: BTreeMap<String, String>,
    /// Optional acoustic alignment provenance; required for words carrying `alignment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<AlignmentProfile>,
    /// Terms the recognizer was prompted with and respelled to, up to 32 of 1-64 bytes; empty when it had none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vocabulary: Vec<String>,
}
/// Speech no word covers, as recognition found it: sound the acoustic model read as letters where
/// no word is, such as a filler the recognizer left out. A candidate to review, never a word.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Uncovered {
    /// Start of the voiced sound, absolute source time on the 48 kHz clock.
    pub start: Time,
    /// End of the voiced sound; after `start` and within the analysis range.
    pub end: Time,
    /// Letters the acoustic model read there, such as `AM`; spaces where it heard word breaks, at most 256 bytes.
    pub letters: String,
}
/// Acoustic alignment models and the context added around estimated word intervals.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlignmentProfile {
    /// 1-8 alignment model files keyed by name (1-128 bytes), as content identities.
    pub files: BTreeMap<String, Identity>,
    /// Context added before each word, rational seconds on the 48 kHz clock; at most 0.5 s.
    pub leading_context: Time,
    /// Context added after each word, rational seconds on the 48 kHz clock; at most 0.5 s.
    pub trailing_context: Time,
}
/// Raw acoustic evidence for one estimated word, in absolute source time within the analysis range.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WordAlignment {
    /// Raw CTC alignment start, rational seconds on the 48 kHz clock.
    pub ctc_start: Time,
    /// Raw CTC alignment end; after `ctc_start`.
    pub ctc_end: Time,
    /// Energy-refined start, rational seconds on the 48 kHz clock.
    pub acoustic_start: Time,
    /// Energy-refined end; after `acoustic_start`.
    pub acoustic_end: Time,
    /// Alignment score in milli-units, 0-1000.
    pub score_milli: u16,
}
/// One transcript word with exact absolute source times.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Word {
    /// Unique word ID within the document, 1-128 bytes.
    pub id: String,
    /// One whitespace-separated token with a letter or digit, at most 512 bytes; punctuation is kept.
    pub text: String,
    /// Absolute source start, reduced rational seconds on the 48 kHz clock; not before the previous word's end.
    pub start: Time,
    /// Absolute source end; after `start` and within the analysis range.
    pub end: Time,
    /// Whether the word is a recognizer estimate or a caller correction.
    pub origin: Origin,
    /// Model confidence in milli-units 0-1000, not a probability of correctness; null for corrected words.
    pub probability_milli: Option<u16>,
    /// Optional raw acoustic evidence; only on estimated words with recognition alignment provenance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alignment: Option<WordAlignment>,
}
impl Word {
    /// The word without surrounding whitespace. Older recognized documents keep the
    /// recognizer's leading space (" tiny"); readers use this to join words with single spaces.
    pub(crate) fn said(&self) -> &str {
        self.text.trim()
    }
}
/// Content-bound transcript of one source range; returned by transcript.transcribe and edited by transcript.correct.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Document {
    /// Document format version; must be 1.
    pub schema_version: u32,
    /// Document ID, 1-128 bytes.
    pub id: String,
    /// 0 for a new document; each transcript.correct increments it.
    pub revision: u64,
    /// SHA-256 hex fingerprint of the previous revision; null exactly when `revision` is 0.
    pub parent_fingerprint: Option<String>,
    /// Source media the word times refer to.
    pub source: Source,
    /// Absolute source start of the analysed range, rational seconds on the 48 kHz clock.
    pub range_start: Time,
    /// Positive analysed duration, at most 120 s; the range must end within the source.
    pub range_duration: Time,
    /// Spoken language.
    pub language: Language,
    /// Recognition provenance, preserved by corrections.
    pub recognition: Recognition,
    /// Up to 2048 ordered, nonoverlapping words; at most 128 KiB of text in total.
    pub words: Vec<Word>,
    /// Up to 2048 ordered, nonoverlapping sounds no word covers; candidates for review, such as left-out fillers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered: Vec<Uncovered>,
}
pub(crate) fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_TRANSCRIPT", message)
}
pub(crate) fn hash_ok(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn fingerprint<T: Serialize>(value: &T) -> Result<String> {
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(value)?)))
}
fn exact(time: Time) -> Result<()> {
    time.validate()?;
    if Time::new(time.num, time.den)? != time {
        return Err(invalid("Transcript times must use reduced rationals"));
    }
    Ok(())
}
impl Document {
    pub fn validate(&self) -> Result<()> {
        crate::tracks::id(&self.id)?;
        self.source.identity.validate()?;
        self.recognition.model.validate()?;
        exact(self.source.duration)?;
        exact(self.range_start)?;
        exact(self.range_duration)?;
        if self.schema_version != 1
            || self.revision > 9_007_199_254_740_991
            || self.words.len() > MAX_WORDS
            || (self.revision == 0) != self.parent_fingerprint.is_none()
            || self
                .parent_fingerprint
                .as_ref()
                .is_some_and(|v| !hash_ok(v))
            || self.source.duration.num == 0
            || self.range_duration.num == 0
            || self.range_duration.compare(MAX_RANGE)?.is_gt()
            || self
                .range_start
                .plus(self.range_duration)?
                .compare(self.source.duration)?
                .is_gt()
        {
            return Err(invalid(
                "Transcript v1 requires a safe revision, a valid parent, <=2048 words and a positive source range <=120 seconds",
            ));
        }
        self.range_start.units(RATE)?;
        self.range_duration.units(RATE)?;
        self.source.duration.units(RATE)?;
        if self.source.path.as_os_str().is_empty()
            || self
                .source
                .path
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
            || self.source.path.to_string_lossy().contains(':')
            || self.recognition.profile.is_empty()
            || self.recognition.profile.len() > 128
            || self.recognition.profile.chars().any(char::is_control)
            || !hash_ok(&self.recognition.worker_sha256)
            || self
                .recognition
                .supervisor_sha256
                .as_ref()
                .is_some_and(|value| !hash_ok(value))
            || !hash_ok(&self.recognition.analysis_sha256)
            || self.recognition.versions.is_empty()
            || self.recognition.versions.len() > 64
            || self.recognition.versions.iter().any(|(k, v)| {
                k.is_empty()
                    || v.is_empty()
                    || k.len() > 128
                    || v.len() > 128
                    || k.chars().chain(v.chars()).any(char::is_control)
            })
        {
            return Err(invalid(
                "Transcript source paths and recognition provenance must be explicit, relative and bounded",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut previous = self.range_start;
        let end = self.range_start.plus(self.range_duration)?;
        if let Some(profile) = &self.recognition.alignment {
            if profile.files.is_empty() || profile.files.len() > 8 {
                return Err(invalid(
                    "Alignment requires 1..8 explicit model-file identities",
                ));
            }
            for (name, identity) in &profile.files {
                if name.is_empty() || name.len() > 128 || name.chars().any(char::is_control) {
                    return Err(invalid("Invalid alignment model-file name"));
                }
                identity.validate()?;
            }
            for time in [profile.leading_context, profile.trailing_context] {
                exact(time)?;
                time.units(RATE)?;
                if time.compare(Time::new(1, 2)?)?.is_gt() {
                    return Err(invalid(
                        "Alignment context cannot exceed half a second per edge",
                    ));
                }
            }
        }
        let mut bytes = 0;
        for word in &self.words {
            crate::tracks::id(&word.id)?;
            exact(word.start)?;
            exact(word.end)?;
            word.start.units(RATE)?;
            word.end.units(RATE)?;
            if let Some(alignment) = &word.alignment {
                if word.origin != Origin::Estimated
                    || self.recognition.alignment.is_none()
                    || alignment.score_milli > 1000
                {
                    return Err(invalid(
                        "Acoustic evidence belongs to an estimated word with alignment provenance",
                    ));
                }
                for (a, b) in [
                    (alignment.ctc_start, alignment.ctc_end),
                    (alignment.acoustic_start, alignment.acoustic_end),
                ] {
                    exact(a)?;
                    exact(b)?;
                    a.units(RATE)?;
                    b.units(RATE)?;
                    if a.compare(self.range_start)?.is_lt()
                        || !a.compare(b)?.is_lt()
                        || b.compare(end)?.is_gt()
                    {
                        return Err(invalid(
                            "Acoustic evidence must stay within the exact source analysis range",
                        ));
                    }
                }
            }
            bytes += word.text.len();
            if !seen.insert(&word.id)
                || word.text.len() > 512
                || word.text.split_whitespace().count() != 1
                || !word.text.chars().any(char::is_alphanumeric)
                || word.text.chars().any(char::is_control)
                || word.start.compare(previous)?.is_lt()
                || !word.start.compare(word.end)?.is_lt()
                || word.end.compare(end)?.is_gt()
                || word.probability_milli.is_some_and(|p| p > 1000)
                || (word.origin == Origin::Corrected && word.probability_milli.is_some())
            {
                return Err(invalid(
                    "Words need unique IDs, one bounded text unit, ordered nonempty source-sample intervals and honest confidence provenance",
                ));
            }
            previous = word.end;
        }
        if bytes > 128 * 1024 {
            return Err(invalid("Transcript text exceeds 128 KiB"));
        }
        if self.recognition.vocabulary.len() > MAX_TERMS
            || !self.recognition.vocabulary.iter().all(|t| term_ok(t))
        {
            return Err(invalid(
                "A recognition vocabulary has at most 32 terms of 1-64 bytes, without control characters or surrounding spaces",
            ));
        }
        let mut previous = self.range_start;
        for sound in &self.uncovered {
            exact(sound.start)?;
            exact(sound.end)?;
            sound.start.units(RATE)?;
            sound.end.units(RATE)?;
            if self.uncovered.len() > MAX_WORDS
                || sound.start.compare(previous)?.is_lt()
                || !sound.start.compare(sound.end)?.is_lt()
                || sound.end.compare(end)?.is_gt()
                || sound.letters.len() > 256
                || !sound.letters.chars().any(char::is_alphabetic)
                || sound.letters.chars().any(char::is_control)
            {
                return Err(invalid(
                    "Uncovered sounds need at most 2048 ordered, nonoverlapping intervals within the analysis range, each with 1-256 bytes of letters",
                ));
            }
            previous = sound.end;
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> Result<String> {
        self.validate()?;
        fingerprint(self)
    }
    pub(crate) fn verify_source(&self, input_root: &Path) -> Result<PathBuf> {
        self.validate()?;
        let path = media::allowed_file(&input_root.join(&self.source.path), input_root)?;
        crate::registry::check_file(
            &path,
            &self.source.path.to_string_lossy(),
            &self.source.identity,
            "restore the original file or transcribe the current one (transcript.transcribe)",
        )?;
        Ok(path)
    }
}
/// The document on a new source that plays this source's audio unchanged from `offset` on, such
/// as a unit-speed media.conform or media.prepare output. Words and their acoustic evidence move by
/// `-offset`; the analysed range is clipped to the new source, and words outside it are dropped
/// (evidence reaching past the clipped range is dropped from its word). The result is the next
/// revision, its parent the original's fingerprint; recognition provenance is unchanged.
pub(crate) fn rebind(document: &Document, source: Source, offset: Time) -> Result<Document> {
    let parent = document.fingerprint()?;
    let later =
        |a: Time, b: Time| -> Result<Time> { Ok(if a.compare(b)?.is_lt() { b } else { a }) };
    let earlier =
        |a: Time, b: Time| -> Result<Time> { Ok(if a.compare(b)?.is_lt() { a } else { b }) };
    let end = document.range_start.plus(document.range_duration)?;
    if !offset.compare(end)?.is_lt() {
        return Err(invalid(
            "The transcript's analysed range ends before the new source begins",
        ));
    }
    let start = later(document.range_start, offset)?.minus(offset)?;
    let stop = earlier(end.minus(offset)?, source.duration)?;
    if !start.compare(stop)?.is_lt() {
        return Err(invalid(
            "The transcript's analysed range lies outside the new source",
        ));
    }
    let inside = |a: Time, b: Time| -> Result<bool> {
        Ok(!a.compare(offset)?.is_lt()
            && !a.minus(offset)?.compare(start)?.is_lt()
            && !b.minus(offset)?.compare(stop)?.is_gt())
    };
    let mut next = document.clone();
    next.words = Vec::new();
    for word in &document.words {
        if !inside(word.start, word.end)? {
            continue;
        }
        let mut moved = word.clone();
        moved.text = word.said().to_owned();
        moved.start = word.start.minus(offset)?;
        moved.end = word.end.minus(offset)?;
        moved.alignment = match &word.alignment {
            Some(a)
                if inside(a.ctc_start, a.ctc_end)? && inside(a.acoustic_start, a.acoustic_end)? =>
            {
                Some(WordAlignment {
                    ctc_start: a.ctc_start.minus(offset)?,
                    ctc_end: a.ctc_end.minus(offset)?,
                    acoustic_start: a.acoustic_start.minus(offset)?,
                    acoustic_end: a.acoustic_end.minus(offset)?,
                    score_milli: a.score_milli,
                })
            }
            _ => None,
        };
        next.words.push(moved);
    }
    next.uncovered = Vec::new();
    for sound in &document.uncovered {
        if inside(sound.start, sound.end)? {
            next.uncovered.push(Uncovered {
                start: sound.start.minus(offset)?,
                end: sound.end.minus(offset)?,
                letters: sound.letters.clone(),
            });
        }
    }
    next.source = source;
    next.range_start = start;
    next.range_duration = stop.minus(start)?;
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("Transcript revision exhausted"))?;
    next.parent_fingerprint = Some(parent);
    next.validate()?;
    Ok(next)
}

/// The words either side of an uncovered sound, by its middle: the last word ending by the
/// middle and the first word starting after it. A word's context can reach into the sound; a
/// word over the middle covers it and is neither.
pub(crate) fn around<'a>(
    words: impl IntoIterator<Item = &'a Word>,
    sound: &Uncovered,
) -> Result<(Option<&'a Word>, Option<&'a Word>)> {
    let middle = sound.start.plus(sound.end)?.times(Time::new(1, 2)?)?;
    let (mut after, mut before) = (None, None);
    for word in words {
        if !word.end.compare(middle)?.is_gt() {
            after = Some(word);
        } else if before.is_none() && word.start.compare(middle)?.is_gt() {
            before = Some(word);
        }
    }
    Ok((after, before))
}

/// The documents' uncovered sounds for a receipt: their count, and up to `limit` of them with
/// the document and the words either side.
pub(crate) fn uncovered_listing(documents: &[Document], limit: usize) -> Result<Value> {
    let mut listed = Vec::new();
    let mut count = 0;
    for document in documents {
        for sound in &document.uncovered {
            count += 1;
            if listed.len() < limit {
                let (after, before) = around(&document.words, sound)?;
                listed.push(json!({"document":document.id,"start":sound.start,"end":sound.end,"letters":sound.letters,
                    "after":after.map(Word::said),"before":before.map(Word::said)}));
            }
        }
    }
    Ok(json!({"count":count,"listed":listed}))
}

pub fn inspect(document: &Document, input_root: &Path) -> Result<Value> {
    document.verify_source(input_root)?;
    Ok(
        json!({"document":document,"fingerprint":document.fingerprint()?,"word_count":document.words.len(),
              "estimated_word_count":document.words.iter().filter(|w|w.origin==Origin::Estimated).count(),
              "clock":"absolute_source_samples_48000","applied":false}),
    )
}
pub fn capabilities() -> Value {
    json!({"schema_version":1,"maximum_range_seconds":120,"maximum_words":2048,
        "maximum_correction_batch":128,"maximum_cut_ranges":128,"languages":["en","el"],
        "clock":"absolute_source_samples_48000","correction":"explicit_content_bound_revision",
        "cut_binding":"concrete_native_media_clip","cut_clocks":["video","audio"],
        "rounding":["strict","outward"],"estimates":"explicit_reject_or_allow_reported",
        "collateral_words":"explicit_reject_or_allow_reported","overlapping_cuts":"merge_union",
        "recognition":{"command":"transcript.transcribe","mode":"blocking_cli_library","profile":"local-en-el-context-v1",
            "platform":"windows_wsl_cuda","minimum_range_seconds":{"num":1,"den":40},"maximum_range_seconds":120,
            "maximum_seconds":600,"network":"isolated_namespace","source_profiles":["stereo_wav","reference_movie"],
            "analysis_rate":16000,"source_clock":48000,"channel_selection":["left","right","mean"],
            "leading_context":{"num":2,"den":25},"trailing_context":{"num":1,"den":50},"review_required":true,
            "window_policies":["source_end","quiet_gap","word_gap","hard_12s"],"non_speech":"removed_before_alignment_and_reported",
            "word_text":"no_surrounding_whitespace","vocabulary":{"field":"vocabulary","maximum_terms":MAX_TERMS,"maximum_term_bytes":64,
                "use":"recognizer_prompt_and_one_word_respelling","recorded":"recognition.vocabulary"},
            "uncovered":"acoustic_letters_outside_every_word_reported_for_review"},
        "text_alignment":{"field":"text","commands":["transcript.transcribe","media.transcribe"],"profile":"local-en-el-align-v1",
            "maximum_range_seconds":120,"maximum_bytes":32768,"maximum_words":2048,"numbers":"spelled_out_only"},"writes_state":false})
}

/// Caller-supplied word for transcript.correct; the result is marked `corrected` with no model confidence.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Correction {
    /// Word ID: an existing ID for `replace`, a fresh ID for `insert`.
    pub id: String,
    /// One whitespace-separated token with a letter or digit, at most 512 bytes.
    pub text: String,
    /// Absolute source start, reduced rational seconds on the 48 kHz clock.
    pub start: Time,
    /// Absolute source end; after `start` and within the analysis range.
    pub end: Time,
}
impl Correction {
    fn word(&self) -> Word {
        Word {
            id: self.id.clone(),
            text: self.text.trim().to_owned(),
            start: self.start,
            end: self.end,
            origin: Origin::Corrected,
            probability_milli: None,
            alignment: None,
        }
    }
}
/// One word edit in a transcript.correct batch of 1-128, tagged by `op`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// Replace the existing word that has the same ID.
    Replace {
        /// Complete replacement word; its `id` selects the word.
        word: Correction,
    },
    /// Insert a new word.
    Insert {
        /// ID of the existing word to insert before; null appends.
        before_id: Option<String>,
        /// New word with an unused ID.
        word: Correction,
    },
    /// Remove an existing word.
    Remove {
        /// ID of the word to remove.
        id: String,
    },
}
/// Drop the uncovered sounds a word now covers, such as one an inserted "um" accounts for: those
/// whose middle lies inside a word.
fn drop_covered(document: &mut Document) -> Result<()> {
    let mut kept = Vec::new();
    for sound in document.uncovered.drain(..) {
        let middle = sound.start.plus(sound.end)?.times(Time::new(1, 2)?)?;
        let mut covered = false;
        for word in &document.words {
            covered |= !middle.compare(word.start)?.is_lt() && middle.compare(word.end)?.is_lt();
        }
        if !covered {
            kept.push(sound);
        }
    }
    document.uncovered = kept;
    Ok(())
}

pub fn correct(
    document: &Document,
    expected_fingerprint: &str,
    edits: &[Edit],
    input_root: &Path,
) -> Result<Value> {
    document.verify_source(input_root)?;
    if document.fingerprint()? != expected_fingerprint {
        return Err(error(
            "TRANSCRIPT_CONFLICT",
            "Expected transcript fingerprint does not match",
        ));
    }
    if edits.is_empty() || edits.len() > 128 {
        return Err(invalid(
            "Correction batch requires 1..128 explicit word edits",
        ));
    }
    let mut next = document.clone();
    let mut used: BTreeSet<_> = next.words.iter().map(|w| w.id.clone()).collect();
    for edit in edits {
        match edit {
            Edit::Replace { word } => {
                let target = next
                    .words
                    .iter_mut()
                    .find(|w| w.id == word.id)
                    .ok_or_else(|| error("MISSING_WORD", &word.id))?;
                *target = word.word();
            }
            Edit::Insert { before_id, word } => {
                if !used.insert(word.id.clone()) {
                    return Err(error("DUPLICATE_ID", &word.id));
                }
                let index = match before_id {
                    Some(id) => next
                        .words
                        .iter()
                        .position(|w| &w.id == id)
                        .ok_or_else(|| error("MISSING_WORD", id))?,
                    None => next.words.len(),
                };
                next.words.insert(index, word.word());
            }
            Edit::Remove { id } => {
                let index = next
                    .words
                    .iter()
                    .position(|w| &w.id == id)
                    .ok_or_else(|| error("MISSING_WORD", id))?;
                next.words.remove(index);
            }
        }
    }
    // A new revision drops the whitespace older recognizer output kept around words.
    for word in &mut next.words {
        if word.said().len() != word.text.len() {
            word.text = word.said().to_owned();
        }
    }
    drop_covered(&mut next)?;
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or_else(|| invalid("Transcript revision exhausted"))?;
    next.parent_fingerprint = Some(expected_fingerprint.to_owned());
    next.validate()?;
    document.verify_source(input_root)?;
    Ok(
        json!({"document":next,"fingerprint":next.fingerprint()?,"previous_fingerprint":expected_fingerprint,
              "source_unchanged":true,"recognition_provenance_unchanged":true,"applied":false}),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn rebinding_moves_words_onto_a_trimmed_copy() {
        let original = fixture();
        let offset = original.words[1].start;
        let source = Source {
            path: "media/copy.mkv".into(),
            identity: Identity {
                sha256: "e".repeat(64),
                bytes: 10,
            },
            duration: original.source.duration.minus(offset).unwrap(),
        };
        let moved = rebind(&original, source.clone(), offset).unwrap();
        assert_eq!(moved.source, source);
        assert_eq!(moved.revision, original.revision + 1);
        assert_eq!(
            moved.parent_fingerprint,
            Some(original.fingerprint().unwrap())
        );
        assert_eq!(moved.words.len(), original.words.len() - 1);
        assert_eq!(moved.words[0].start, Time::ZERO);
        assert_eq!(moved.words[0].text, original.words[1].said());
        assert_eq!(
            moved.words[0].end,
            original.words[1].end.minus(offset).unwrap()
        );
        assert_eq!(moved.recognition, original.recognition);
        // A source that starts after the analysed range has nothing to carry.
        let end = original.range_start.plus(original.range_duration).unwrap();
        assert!(rebind(&original, source, end).is_err());
    }

    pub(crate) fn fixture() -> Document {
        let t = |n, d| Time::new(n, d).unwrap();
        Document {
            schema_version: 1,
            id: "speech".into(),
            revision: 0,
            parent_fingerprint: None,
            source: Source {
                path: "voice.mkv".into(),
                identity: Identity {
                    sha256: "a".repeat(64),
                    bytes: 100,
                },
                duration: t(4, 1),
            },
            range_start: t(1, 1),
            range_duration: t(2, 1),
            language: Language::En,
            recognition: Recognition {
                profile: "fixture".into(),
                model: Identity {
                    sha256: "b".repeat(64),
                    bytes: 100,
                },
                worker_sha256: "c".repeat(64),
                supervisor_sha256: None,
                analysis_sha256: "d".repeat(64),
                versions: BTreeMap::from([("fixture".into(), "1".into())]),
                alignment: None,
                vocabulary: Vec::new(),
            },
            words: vec![
                Word {
                    id: "first".into(),
                    text: " One".into(),
                    start: t(48001, 48000),
                    end: t(3, 2),
                    origin: Origin::Estimated,
                    probability_milli: Some(900),
                    alignment: None,
                },
                Word {
                    id: "second".into(),
                    text: " two.".into(),
                    start: t(2, 1),
                    end: t(5, 2),
                    origin: Origin::Corrected,
                    probability_milli: None,
                    alignment: None,
                },
            ],
            uncovered: Vec::new(),
        }
    }
    #[test]
    fn source_clocks_and_provenance_are_part_of_content_identity() {
        let a = fixture();
        a.validate().unwrap();
        let original = a.fingerprint().unwrap();
        let mut b = a.clone();
        b.words[0].start = Time::new(48002, 48000).unwrap();
        assert_ne!(original, b.fingerprint().unwrap());
        b = a.clone();
        b.source.identity.sha256 = "e".repeat(64);
        assert_ne!(original, b.fingerprint().unwrap());
        b = a.clone();
        b.language = Language::El;
        assert_ne!(original, b.fingerprint().unwrap());
        b = a.clone();
        b.words[0].origin = Origin::Corrected;
        assert!(b.validate().is_err());
        b.words[0].probability_milli = None;
        b.validate().unwrap();
        assert_ne!(original, b.fingerprint().unwrap());
    }
    #[test]
    fn words_read_without_the_recognizer_spacing_and_corrections_write_it_so() {
        let a = fixture();
        assert_eq!(a.words[0].text, " One");
        assert_eq!(a.words[0].said(), "One");
        assert_eq!(a.words[1].said(), "two.");
        let correction = Correction {
            id: "first".into(),
            text: " Uno ".into(),
            start: a.words[0].start,
            end: a.words[0].end,
        };
        assert_eq!(correction.word().text, "Uno");
    }
    #[test]
    fn uncovered_sounds_and_vocabulary_are_bounded_and_follow_edits() {
        let t = |n, d| Time::new(n, d).unwrap();
        let sound = |a: Time, b: Time, letters: &str| Uncovered {
            start: a,
            end: b,
            letters: letters.into(),
        };
        // Documents without them serialize, and so fingerprint, exactly as before.
        let plain = serde_json::to_value(fixture()).unwrap();
        assert!(
            plain.get("uncovered").is_none() && plain["recognition"].get("vocabulary").is_none()
        );
        let mut a = fixture();
        a.recognition.vocabulary = vec!["PixelForge".into(), "um".into()];
        a.uncovered = vec![sound(t(8, 5), t(19, 10), "AM")];
        a.validate().unwrap();
        assert_ne!(a.fingerprint().unwrap(), fixture().fingerprint().unwrap());
        let back: Document = serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        for bad in [
            vec![sound(t(19, 10), t(8, 5), "AM")],
            vec![sound(t(29, 10), t(31, 10), "AM")],
            vec![sound(t(1, 2), t(11, 10), "AM")],
            vec![
                sound(t(8, 5), t(19, 10), "AM"),
                sound(t(9, 5), t(39, 20), "UH"),
            ],
            vec![sound(t(8, 5), t(19, 10), "")],
            vec![sound(t(8, 5), t(19, 10), "...")],
            vec![sound(t(8, 5), t(19, 10), "A\u{7}M")],
            vec![sound(t(8, 5), t(19, 10), &"M".repeat(257))],
            vec![sound(t(8, 5), t(1, 7), "AM")],
        ] {
            let mut b = a.clone();
            b.uncovered = bad;
            assert!(b.validate().is_err(), "{:?}", b.uncovered);
        }
        for bad in [
            vec![String::new()],
            vec![" um".into()],
            vec!["x".repeat(65)],
            vec!["a\u{7}b".into()],
            (0..33).map(|n| format!("t{n}")).collect(),
        ] {
            let mut b = a.clone();
            b.recognition.vocabulary = bad;
            assert!(b.validate().is_err(), "{:?}", b.recognition.vocabulary);
        }
        // A corrected word over a sound accounts for it; sounds elsewhere stay.
        let mut b = a.clone();
        b.uncovered.push(sound(t(13, 5), t(27, 10), "UH"));
        b.words.insert(
            1,
            Correction {
                id: "um".into(),
                text: "um".into(),
                start: t(3, 2),
                end: t(2, 1),
            }
            .word(),
        );
        drop_covered(&mut b).unwrap();
        assert_eq!(b.uncovered, [sound(t(13, 5), t(27, 10), "UH")]);
        // Rebinding moves sounds with the words and drops those outside the new source.
        let offset = t(17, 10);
        let source = Source {
            duration: a.source.duration.minus(offset).unwrap(),
            ..a.source.clone()
        };
        let mut c = a.clone();
        c.uncovered.push(sound(t(13, 5), t(27, 10), "UH"));
        let moved = rebind(&c, source, offset).unwrap();
        assert_eq!(moved.uncovered, [sound(t(9, 10), t(1, 1), "UH")]);
        assert_eq!(moved.recognition.vocabulary, a.recognition.vocabulary);
    }
    #[test]
    fn invalid_intervals_duplicate_ids_and_hidden_fields_reject() {
        let a = fixture();
        let mut b = a.clone();
        b.words[1].id = b.words[0].id.clone();
        assert!(b.validate().is_err());
        b = a.clone();
        b.words[1].start = Time::new(7, 5).unwrap();
        assert!(b.validate().is_err());
        b = a.clone();
        b.words[1].end = Time::new(4, 1).unwrap();
        assert!(b.validate().is_err());
        b = a.clone();
        b.words[0].start = Time::new(1, 7).unwrap();
        assert!(b.validate().is_err());
        b = a.clone();
        b.source.path = "../voice.mkv".into();
        assert!(b.validate().is_err());
        b = a.clone();
        b.words[0].text = "two words".into();
        assert!(b.validate().is_err());
        let mut data = serde_json::to_value(a).unwrap();
        data["hidden"] = json!(true);
        assert!(serde_json::from_value::<Document>(data).is_err());
    }
}
