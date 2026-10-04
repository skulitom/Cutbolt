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
            "leading_context":{"num":2,"den":25},"trailing_context":{"num":1,"den":50},"review_required":true},"writes_state":false})
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
            text: self.text.clone(),
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
