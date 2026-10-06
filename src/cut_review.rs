//! Review of a rendered cut, written to one new folder: a contact sheet, a small watchable copy,
//! loudness over time, black frames, duration against the project, and whether the cut says what
//! the timeline intended, by comparing the words expected from the source transcripts with the
//! words heard in the cut.
use crate::{
    Result, error, media,
    model::Project,
    numerals,
    outline::{Clock, Said},
    registry::Identity,
    time::Time,
    transcribe,
    transcript::{self, Document, Language},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

/// Longest single tool run: four hours covers decoding and encoding long cuts.
const TOOL_SECONDS: u64 = 4 * 3600;
/// Listed black runs, differences and cut words; counts stay exact.
const MAX_LISTED: usize = 50;
/// Runs named per line of the text summary; the JSON lists up to `MAX_LISTED`.
const SUMMARY_RUNS: usize = 8;
/// Recognition windows stay within the transcription limit and overlap, so no word is lost at a seam.
const WINDOW_SAMPLES: u64 = 120 * 48_000;
const OVERLAP_SAMPLES: u64 = 5 * 48_000;
/// AAC encoders may add up to two 1024-sample frames of padding at the end.
const AUDIO_SLACK: u64 = 2048;

/// Local speech recognition of the cut's audio.
pub struct Recognize {
    pub runtime: transcribe::Runtime,
    pub language: Language,
}

/// What to review and what to produce.
pub struct Request<'a> {
    pub path: &'a Path,
    pub input_root: &'a Path,
    pub output_root: &'a Path,
    pub output: &'a Path,
    pub project: Option<&'a Project>,
    pub transcripts: &'a [Document],
    pub heard: &'a [Document],
    pub recognize: Option<&'a Recognize>,
    pub frames: u32,
    pub rendition_height: u32,
    pub tolerance: Time,
}

/// The review folder; removed again unless the review completes.
struct Folder {
    path: PathBuf,
    keep: bool,
}
impl Drop for Folder {
    fn drop(&mut self) {
        if !self.keep {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn create_folder(output: &Path, root: &Path) -> Result<Folder> {
    if !output.is_absolute() || !root.is_absolute() {
        return Err(error("INVALID_PATH", "Output and root must be absolute"));
    }
    let name = output
        .file_name()
        .ok_or_else(|| error("INVALID_PATH", "The review folder needs a name"))?;
    let parent = crate::render::output_folder(output, root)?;
    if !parent.starts_with(crate::render::output_root(root)?) {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            "The review folder must be inside the output root",
        ));
    }
    let path = parent.join(name);
    fs::create_dir(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => error(
            "OUTPUT_EXISTS",
            format!("{} already exists; reviews never overwrite", path.display()),
        ),
        _ => error("INVALID_PATH", format!("{}: {e}", path.display())),
    })?;
    Ok(Folder { path, keep: false })
}

fn ffmpeg(args: &[String]) -> Result<Vec<u8>> {
    media::capture(
        &media::tool("ffmpeg"),
        args,
        Duration::from_secs(TOOL_SECONDS),
    )
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| (*s).to_owned()).collect()
}

fn rate(stream: &Value) -> Option<Time> {
    for key in ["avg_frame_rate", "r_frame_rate"] {
        if let Some((num, den)) = stream[key].as_str().and_then(|r| r.split_once('/'))
            && let (Ok(num), Ok(den)) = (num.parse::<u64>(), den.parse::<u64>())
            && num > 0
            && den > 0
        {
            return Time::new(num, den).ok();
        }
    }
    None
}

/// Nearest frame boundary at `rate`, for times FFmpeg prints as rounded decimals.
fn snap(text: &str, rate: Time) -> Result<Time> {
    let time: Time = serde_json::from_value(json!(text.trim()))?;
    let n = time.num as u128 * rate.num as u128;
    let d = time.den as u128 * rate.den as u128;
    let frame = ((2 * n + d) / (2 * d)) as u64;
    Time::new(frame * rate.den, rate.num)
}

/// Decode the first audio stream to 48 kHz stereo PCM16 WAV, then meter it as it streams from
/// disk. Returns the sample count, integrated meters and loudness over time.
fn audio(path: &Path, wav: &Path) -> Result<(u64, Value, Value)> {
    let mut args = strings(&["-nostdin", "-v", "error", "-n", "-i"]);
    args.push(path.to_string_lossy().into_owned());
    args.extend(strings(&[
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-ac",
        "2",
        "-ar",
        "48000",
        "-c:a",
        "pcm_s16le",
        "-map_metadata",
        "-1",
        "-fflags",
        "+bitexact",
        "-flags:a",
        "+bitexact",
        "-f",
        "wav",
    ]));
    args.push(wav.to_string_lossy().into_owned());
    ffmpeg(&args)?;
    let (frames, meters, over_time) = crate::audio_processing::measure_wav(wav, true)?;
    Ok((frames, meters, over_time.expect("curve requested")))
}

/// One decoding pass over the picture: black frames (`blackdetect` defaults: pixels at most 10%
/// luma, 98% of the picture) and, with a height, a small H.264/AAC copy.
fn picture(
    path: &Path,
    rendition: Option<(&Path, u32)>,
    sound: bool,
) -> Result<Vec<(String, Option<String>)>> {
    let detect = "blackdetect=d=0,metadata=mode=print:file='pipe\\:1'";
    let mut args = strings(&["-nostdin", "-v", "error", "-n", "-i"]);
    args.push(path.to_string_lossy().into_owned());
    args.extend(strings(&["-map", "0:v:0"]));
    match rendition {
        Some((output, height)) => {
            args.push("-vf".into());
            args.push(format!(
                "{detect},scale=w=-2:h=trunc(min(ih\\,{height})/2)*2,setsar=1"
            ));
            if sound {
                args.extend(strings(&[
                    "-map", "0:a:0", "-c:a", "aac", "-b:a", "96k", "-ac", "2",
                ]));
            }
            args.extend(strings(&[
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "28",
                "-pix_fmt",
                "yuv420p",
                "-movflags",
                "+faststart",
            ]));
            args.push(output.to_string_lossy().into_owned());
        }
        None => {
            args.extend(strings(&["-vf", detect, "-an", "-f", "null", "-"]));
        }
    }
    let printed = ffmpeg(&args)?;
    let mut runs: Vec<(String, Option<String>)> = Vec::new();
    for line in String::from_utf8_lossy(&printed).lines() {
        if let Some(start) = line.strip_prefix("lavfi.black_start=") {
            runs.push((start.to_owned(), None));
        } else if let Some(end) = line.strip_prefix("lavfi.black_end=")
            && let Some(last) = runs.last_mut()
        {
            last.1 = Some(end.to_owned());
        }
    }
    Ok(runs)
}

/// Lowercase letters and digits only, so punctuation and case do not count as differences.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Terms to prompt recognition of the cut with, up to 32: the source transcripts' own vocabularies,
/// then expected words that look like names, spelled as the transcripts spell them. A name has a
/// capital after its first letter (PixelForge) or a capital first letter away from a sentence
/// start (Cutbolt in "using Cutbolt."); "I" and its contractions are not names.
fn known_terms(documents: &[Document], expected: &[Said]) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();
    let mut add = |term: &str| {
        if terms.len() < transcript::MAX_TERMS
            && transcript::term_ok(term)
            && !terms.iter().any(|t| t == term)
        {
            terms.push(term.to_owned());
        }
    };
    for document in documents {
        document.recognition.vocabulary.iter().for_each(|t| add(t));
    }
    let mut sentence_start = true;
    for word in expected {
        let core = word.text.trim_matches(|c: char| !c.is_alphanumeric());
        let capital = core.chars().next().is_some_and(char::is_uppercase);
        let inner =
            core.chars().skip(1).any(char::is_uppercase) && core.chars().any(char::is_lowercase);
        let pronoun = core == "I" || core.starts_with("I'");
        if (inner || capital && !sentence_start && !pronoun)
            && core.chars().any(char::is_alphabetic)
        {
            add(core);
        }
        sentence_start = word.text.ends_with(['.', '!', '?', ':']);
    }
    terms
}

/// The uncovered sounds of transcripts of the cut, listed for the review. Overlapping transcripts
/// split their overlap at its middle, as their words do, so a sound near a seam is listed once.
fn heard_uncovered(documents: &[Document]) -> Result<Value> {
    let mut sorted = documents.to_vec();
    sorted.sort_by(|a, b| a.range_start.compare(b.range_start).expect("valid times"));
    let half = Time::new(1, 2)?;
    for i in 1..sorted.len() {
        let previous_end = sorted[i - 1]
            .range_start
            .plus(sorted[i - 1].range_duration)?;
        if !sorted[i].range_start.compare(previous_end)?.is_lt() {
            continue;
        }
        let seam = sorted[i].range_start.plus(previous_end)?.times(half)?;
        for (document, before) in [(i - 1, true), (i, false)] {
            let mut kept = Vec::new();
            for sound in sorted[document].uncovered.drain(..) {
                let middle = sound.start.plus(sound.end)?.times(half)?;
                if middle.compare(seam)?.is_lt() == before {
                    kept.push(sound);
                }
            }
            sorted[document].uncovered = kept;
        }
    }
    transcript::uncovered_listing(&sorted, MAX_LISTED)
}

/// Words heard in transcripts of the cut itself. Overlapping transcripts split their overlap at
/// its middle, so a word near a seam is counted once.
fn heard(documents: &[Document], identity: &Identity) -> Result<Vec<Said>> {
    let mut documents: Vec<&Document> = documents.iter().collect();
    for document in &documents {
        document.validate()?;
        if document.source.identity != *identity {
            return Err(error(
                "INVALID_ARGUMENT",
                format!(
                    "Heard transcript {:?} is not a transcript of the reviewed file",
                    document.id
                ),
            ));
        }
    }
    documents.sort_by(|a, b| a.range_start.compare(b.range_start).expect("valid times"));
    let end = |d: &Document| d.range_start.plus(d.range_duration);
    let mut seams = Vec::new();
    for pair in documents.windows(2) {
        let previous_end = end(pair[0])?;
        seams.push(if pair[1].range_start.compare(previous_end)?.is_lt() {
            Some(
                pair[1]
                    .range_start
                    .plus(previous_end)?
                    .times(Time::new(1, 2)?)?,
            )
        } else {
            None
        });
    }
    let mut said = Vec::new();
    for (i, document) in documents.iter().enumerate() {
        let after = if i > 0 { seams[i - 1] } else { None };
        let before = seams.get(i).copied().flatten();
        for word in &document.words {
            let entry = Said {
                start: word.start,
                end: word.end,
                text: word.said().to_owned(),
            };
            let middle = entry.middle()?;
            if after.is_some_and(|t| middle.compare(t).is_ok_and(|o| o.is_lt()))
                || before.is_some_and(|t| !middle.compare(t).is_ok_and(|o| o.is_lt()))
            {
                continue;
            }
            said.push(entry);
        }
    }
    said.sort_by(|a, b| a.start.compare(b.start).expect("valid times"));
    Ok(said)
}

/// What recognition of the cut found: documents, sound it heard that is not speech (such as
/// "[Music]") and windows that were digital silence.
struct Recognized {
    documents: Vec<Document>,
    non_speech: Vec<Value>,
    silent: Vec<Value>,
}

/// Recognize the cut's audio in overlapping windows with the local speech runtime.
fn recognize(
    folder: &Path,
    samples: u64,
    identity: &Identity,
    settings: &Recognize,
    vocabulary: &[String],
) -> Result<Recognized> {
    let source = transcript::Source {
        path: PathBuf::from("audio.wav"),
        identity: identity.clone(),
        duration: Time::new(samples, 48_000)?,
    };
    let mut found = Recognized {
        documents: Vec::new(),
        non_speech: Vec::new(),
        silent: Vec::new(),
    };
    // Each window's notes, kept between the middles of its overlaps with its neighbours.
    let mut notes: Vec<(u64, u64, Vec<Value>)> = Vec::new();
    let mut plan = Vec::new();
    let mut start = 0;
    while start < samples {
        let end = (start + WINDOW_SAMPLES).min(samples);
        if end - start < 1200 && start > 0 {
            break;
        }
        plan.push((start, end));
        if end == samples {
            break;
        }
        start = end - OVERLAP_SAMPLES;
    }
    // Every window in as few worker launches as possible: the models load once per launch.
    let requests = plan
        .iter()
        .enumerate()
        .map(|(window, &(start, end))| {
            Ok(transcribe::Transcribe {
                id: format!("heard-{window}"),
                source: source.clone(),
                format: transcribe::Format::StereoWav,
                start: Time::new(start, 48_000)?,
                duration: Time::new(end - start, 48_000)?,
                channel: transcribe::Channel::Mean,
                language: settings.language,
                input_root: folder.to_path_buf(),
                scratch_root: std::env::temp_dir(),
                runtime: settings.runtime.clone(),
                timeout_seconds: 600,
                text: None,
                vocabulary: vocabulary.to_vec(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let outcomes = transcribe::run_many(&requests, &crate::media::Uncontrolled)?;
    for (&(start, end), outcome) in plan.iter().zip(outcomes) {
        let from = Time::new(start, 48_000)?;
        match outcome {
            Ok(result) => {
                found
                    .documents
                    .push(serde_json::from_value(result["document"].clone())?);
                let listed = result["non_speech"].as_array().cloned().unwrap_or_default();
                notes.push((start, end, listed));
            }
            // A window of digital silence has nothing to recognize.
            Err(e) if e.code == "NO_WORDS" => {
                found
                    .silent
                    .push(json!({"start":from,"end":Time::new(end, 48_000)?}));
            }
            Err(e) => return Err(e),
        }
    }
    for (i, (start, end, listed)) in notes.iter().enumerate() {
        let low = match i.checked_sub(1).map(|j| &notes[j]) {
            Some(previous) if previous.1 > *start => (start + previous.1) / 2,
            _ => 0,
        };
        let high = match notes.get(i + 1) {
            Some(next) if next.0 < *end => (next.0 + end) / 2,
            _ => samples,
        };
        let (low, high) = (Time::new(low, 48_000)?, Time::new(high, 48_000)?);
        for note in listed {
            let at: Time = serde_json::from_value(note["start"].clone())?;
            if !at.compare(low)?.is_lt() && at.compare(high)?.is_lt() {
                found.non_speech.push(note.clone());
            }
        }
    }
    Ok(found)
}

/// The letters and digits of `count` consecutive words from `first`, run together.
fn joined(words: &[Said], first: usize, count: usize) -> String {
    words[first..first + count]
        .iter()
        .map(|w| normalized(&w.text))
        .collect()
}

/// Most words one side of a match may join: a name split in two to four words, or a number
/// spoken in up to eight ("one thousand nine hundred and ninety").
const MAX_JOINED: usize = 8;

/// The words of one list as review matching reads them: each word's letter-and-digit runs with
/// numerals read out, and whether it can take part in a number.
struct Reading<'a> {
    words: &'a [Said],
    runs: Vec<Vec<String>>,
    numeric: Vec<bool>,
}
impl<'a> Reading<'a> {
    fn new(words: &'a [Said]) -> Self {
        let runs: Vec<Vec<String>> = words.iter().map(|w| numerals::runs(&w.text)).collect();
        let numeric = words
            .iter()
            .zip(&runs)
            .map(|(w, r)| numerals::numeric(&w.text, r))
            .collect();
        Reading {
            words,
            runs,
            numeric,
        }
    }
    /// The middle of the span of `count` words from `first`.
    fn middle(&self, first: usize, count: usize) -> Result<Time> {
        self.words[first]
            .start
            .plus(self.words[first + count - 1].end)?
            .times(Time::new(1, 2)?)
    }
    fn numeric(&self, first: usize, count: usize) -> bool {
        self.numeric[first..first + count].iter().any(|n| *n)
    }
    fn key(&self, first: usize, count: usize) -> Vec<numerals::Atom> {
        numerals::key(self.runs[first..first + count].iter().flatten())
    }
}

/// Match expected and heard words in order. A group of expected words matches a group of heard
/// words when the middles of their spans lie within `tolerance` and they say the same thing: the
/// same letters and digits, ignoring case and punctuation, or the same numbers. One side is a
/// single word; the other is one word or up to `MAX_JOINED` words run together, so a name split in
/// one list and whole in the other ("pixel forge" and "PixelForge") matches, and so does a numeral
/// and its spoken words ("80" and "eighty", "170" and "a hundred and seventy"). Unmatched words
/// between two matches form one difference.
fn compare(expected: &[Said], heard: &[Said], tolerance: Time) -> Result<(Value, Vec<Value>)> {
    let (wanted_words, heard_words) = (Reading::new(expected), Reading::new(heard));
    let mut matched_expected = vec![false; expected.len()];
    let mut matched_heard = vec![false; heard.len()];
    let mut next = 0;
    let mut matches = 0;
    let mut joins = 0;
    let mut numbers = 0;
    let mut i = 0;
    while i < expected.len() {
        // A heard word whose middle is later than this cannot start a match of expected word i.
        let reach = expected[(i + MAX_JOINED - 1).min(expected.len() - 1)]
            .middle()?
            .plus(tolerance)?;
        let mut k = next;
        let mut step = 1;
        while k < heard.len() {
            if heard[k].middle()?.compare(reach)?.is_gt() {
                break;
            }
            // (expected words, heard words, matched as numbers) that match here.
            let mut found = None;
            let shapes = std::iter::once((1, 1))
                .chain((2..=MAX_JOINED).map(|n| (1, n)))
                .chain((2..=MAX_JOINED).map(|n| (n, 1)));
            for (wanted, got) in shapes {
                if i + wanted > expected.len() || k + got > heard.len() {
                    continue;
                }
                let a = wanted_words.middle(i, wanted)?;
                let b = heard_words.middle(k, got)?;
                if a.compare(b.plus(tolerance)?)?.is_gt() || b.compare(a.plus(tolerance)?)?.is_gt()
                {
                    continue;
                }
                if joined(expected, i, wanted) == joined(heard, k, got) {
                    found = Some((wanted, got, false));
                    break;
                }
                if (wanted_words.numeric(i, wanted) || heard_words.numeric(k, got))
                    && wanted_words.key(i, wanted) == heard_words.key(k, got)
                {
                    found = Some((wanted, got, true));
                    break;
                }
            }
            if let Some((wanted, got, number)) = found {
                matched_expected[i..i + wanted].fill(true);
                matched_heard[k..k + got].fill(true);
                matches += wanted;
                joins += usize::from(wanted + got > 2);
                numbers += usize::from(number);
                next = k + got;
                step = wanted;
                break;
            }
            k += 1;
        }
        i += step;
    }
    // Group unmatched words by how many matches precede them; matching is in order, so equal
    // counts sit between the same two matches in both lists.
    let mut groups: BTreeMap<usize, (Vec<&Said>, Vec<&Said>)> = BTreeMap::new();
    let mut seen = 0;
    for (word, matched) in expected.iter().zip(&matched_expected) {
        if *matched {
            seen += 1;
        } else {
            groups.entry(seen).or_default().0.push(word);
        }
    }
    seen = 0;
    for (word, matched) in heard.iter().zip(&matched_heard) {
        if *matched {
            seen += 1;
        } else {
            groups.entry(seen).or_default().1.push(word);
        }
    }
    let mut differences = Vec::new();
    for (wanted, got) in groups.values() {
        let all: Vec<&&Said> = wanted.iter().chain(got.iter()).collect();
        let mut start = all[0].start;
        let mut end = all[0].end;
        for word in &all {
            if word.start.compare(start)?.is_lt() {
                start = word.start;
            }
            if word.end.compare(end)?.is_gt() {
                end = word.end;
            }
        }
        let join = |words: &[&Said]| {
            words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
        differences
            .push(json!({"start":start,"end":end,"expected":join(wanted),"heard":join(got)}));
    }
    differences.sort_by(|a, b| {
        serde_json::from_value::<Time>(a["start"].clone())
            .expect("time")
            .compare(serde_json::from_value(b["start"].clone()).expect("time"))
            .expect("valid times")
    });
    let ratio =
        |n: usize, d: usize| (d > 0).then(|| (n as f64 / d as f64 * 1000.0).round() / 1000.0);
    Ok((
        json!({"expected_words":expected.len(),"heard_words":heard.len(),"matched":matches,
            "match_ratio":ratio(matches, expected.len()),"tolerance":tolerance,"joined_matches":joins,"number_matches":numbers,
            "differences":{"count":differences.len(),"listed":differences.iter().take(MAX_LISTED).collect::<Vec<_>>()}}),
        differences,
    ))
}

fn more(count: usize) -> String {
    if count > SUMMARY_RUNS {
        format!(", +{} more", count - SUMMARY_RUNS)
    } else {
        String::new()
    }
}

fn decibels(value: &Value) -> Option<f64> {
    value.as_f64().map(|v| (v * 10.0).round() / 10.0)
}

fn time_of(value: &Value) -> Time {
    serde_json::from_value(value.clone()).unwrap_or(Time::ZERO)
}

/// Review a rendered cut into a new folder under `output_root`.
pub fn review(request: &Request) -> Result<Value> {
    if !(1..=64).contains(&request.frames) {
        return Err(error("INVALID_ARGUMENT", "frames must be 1-64"));
    }
    if request.rendition_height != 0 && !(120..=1080).contains(&request.rendition_height) {
        return Err(error(
            "INVALID_ARGUMENT",
            "rendition_height must be 0 (none) or 120-1080",
        ));
    }
    if request.tolerance.num == 0 || request.tolerance.compare(Time::new(5, 1)?)?.is_gt() {
        return Err(error(
            "INVALID_ARGUMENT",
            "tolerance must be positive and at most 5 s",
        ));
    }
    if !request.heard.is_empty() && request.recognize.is_some() {
        return Err(error(
            "INVALID_ARGUMENT",
            "Give heard transcripts or a recognition runtime, not both",
        ));
    }
    if let Some(project) = request.project {
        project.validate()?;
    } else if !request.transcripts.is_empty() {
        return Err(error(
            "INVALID_ARGUMENT",
            "Source transcripts need the project the cut was rendered from",
        ));
    }
    let path = media::allowed_file(request.path, request.input_root)?;
    let identity: Identity = {
        let value = crate::identity::relative(&path, request.input_root)?;
        Identity {
            sha256: value["sha256"].as_str().unwrap_or_default().to_owned(),
            bytes: value["bytes"].as_u64().unwrap_or_default(),
        }
    };
    // Matching transcripts is cheap; do it before any decoding so bad input fails fast.
    let script = match request.project {
        Some(project) if !request.transcripts.is_empty() => {
            let words = crate::outline::timeline_words(
                project,
                request.transcripts,
                Some(request.input_root),
                None,
            )?;
            Some((words.said, words.cut, words.unused))
        }
        _ => None,
    };
    let heard_given = if request.heard.is_empty() {
        None
    } else {
        Some(heard(request.heard, &identity)?)
    };
    let metadata = media::probe(&path)?;
    let streams = metadata["streams"].as_array().cloned().unwrap_or_default();
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video" && s["disposition"]["attached_pic"] != 1)
        .cloned();
    let has_audio = streams.iter().any(|s| s["codec_type"] == "audio");
    if video.is_none() && !has_audio {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "The file has no video or audio stream",
        ));
    }
    if let Some(settings) = request.recognize {
        if !has_audio {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                "Recognition needs an audio stream",
            ));
        }
        // A bad configuration fails here; a recognition failure later only loses the speech check.
        transcribe::check_runtime(&settings.runtime)?;
    }
    let mut folder = create_folder(request.output, request.output_root)?;
    let dir = folder.path.clone();
    let mut files = Vec::new();
    let mut clock = Clock::new();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Picture: frame count, black runs, contact sheet and the small copy.
    let mut file = json!({"source":crate::identity::relative(&path, request.input_root)?});
    let mut lines = Vec::new();
    let mut picture_section = Value::Null;
    let mut frame_rate = None;
    let mut frames = None;
    if let Some(stream) = &video {
        let rate = rate(stream)
            .ok_or_else(|| error("UNSUPPORTED_MEDIA", "The video stream has no frame rate"))?;
        frame_rate = Some(rate);
        let count = crate::review::frame_times(&path)?.len() as u64;
        frames = Some(count);
        let rendition = (request.rendition_height > 0).then(|| dir.join("preview.mp4"));
        let runs = picture(
            &path,
            rendition.as_deref().map(|p| (p, request.rendition_height)),
            has_audio,
        )?;
        let end = Time::new(count * rate.den, rate.num)?;
        let mut black = Vec::new();
        for (start, stop) in &runs {
            let start = snap(start, rate)?;
            let stop = match stop {
                Some(stop) => snap(stop, rate)?,
                None => end,
            };
            if start.compare(stop)?.is_lt() {
                black.push((start, stop));
            }
        }
        let sheet = crate::review::source_sheet(
            &path,
            request.input_root,
            &dir,
            &dir.join("sheet.png"),
            None,
            Some(request.frames),
            None,
            None,
        )?;
        files.push("sheet.png".to_owned());
        let rendition = match rendition {
            Some(output) => {
                let probed = media::probe(&output)?;
                let stream = probed["streams"]
                    .as_array()
                    .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
                    .cloned()
                    .unwrap_or_default();
                files.push("preview.mp4".to_owned());
                json!({"output":output,"width":stream["width"],"height":stream["height"],"bytes":fs::metadata(&output)?.len()})
            }
            None => Value::Null,
        };
        file["video"] = json!({"width":stream["width"],"height":stream["height"],"codec":stream["codec_name"],
            "frame_rate":rate,"frames":count,"duration":end});
        lines.push(format!(
            "review of {name}: {}x{} {} fps {}, {} frames ({} s){}",
            stream["width"],
            stream["height"],
            rate,
            stream["codec_name"].as_str().unwrap_or("video"),
            count,
            clock.at(end),
            if has_audio { "" } else { ", no audio" }
        ));
        lines.push(match black.len() {
            0 => "black: none".to_owned(),
            n => format!(
                "black: {n} run{}: {}{}",
                if n == 1 { "" } else { "s" },
                black
                    .iter()
                    .take(SUMMARY_RUNS)
                    .map(|&(a, b)| clock.span(a, b))
                    .collect::<Vec<_>>()
                    .join(", "),
                more(n)
            ),
        });
        picture_section = json!({"sheet":sheet,"rendition":rendition,
            "black":{"count":black.len(),"runs":black.iter().take(MAX_LISTED).map(|(a,b)| json!({"start":a,"end":b})).collect::<Vec<_>>()}});
    } else {
        lines.push(format!("review of {name}: audio only"));
    }

    // Sound: exact sample count, integrated meters and loudness over time.
    let mut sound_section = Value::Null;
    let mut samples = None;
    let wav = dir.join("audio.wav");
    if has_audio {
        let (count, meters, over_time) = audio(&path, &wav)?;
        samples = Some(count);
        let duration = Time::new(count, 48_000)?;
        file["audio"] = json!({"samples":count,"duration":duration});
        let peak = meters["sample_peak_dbfs"]
            .as_array()
            .map(|p| {
                p.iter()
                    .filter_map(Value::as_f64)
                    .fold(f64::NEG_INFINITY, f64::max)
            })
            .filter(|p| p.is_finite());
        let short: Vec<f64> = over_time["short_term_lkfs"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let range = match (
            short.iter().copied().reduce(f64::min),
            short.iter().copied().reduce(f64::max),
        ) {
            (Some(low), Some(high)) => format!("; short-term {low:.1} to {high:.1} LKFS"),
            _ => String::new(),
        };
        lines.push(format!(
            "audio: {} s; loudness {}, sample peak {}{range}",
            clock.at(duration),
            decibels(&meters["integrated_lkfs"])
                .map_or("unmeasured".to_owned(), |v| format!("{v:.1} LKFS")),
            peak.map_or("silent".to_owned(), |v| format!("{v:.1} dBFS")),
        ));
        for (label, key) in [("silence", "silence"), ("clipping", "clipping")] {
            let section = &over_time[key];
            let count = section["count"].as_u64().unwrap_or(0);
            let runs: Vec<String> = section["runs"]
                .as_array()
                .map(|runs| {
                    runs.iter()
                        .take(SUMMARY_RUNS)
                        .map(|r| clock.span(time_of(&r["start"]), time_of(&r["end"])))
                        .collect()
                })
                .unwrap_or_default();
            let samples = match section["clipped_samples"].as_u64() {
                Some(n) => format!(" ({n} samples)"),
                None => String::new(),
            };
            lines.push(if count == 0 {
                format!("{label}: none")
            } else {
                format!(
                    "{label}: {count} run{}{samples}: {}{}",
                    if count == 1 { "" } else { "s" },
                    runs.join(", "),
                    more(count as usize)
                )
            });
        }
        sound_section = json!({"meters":meters,"over_time":over_time});
    }

    // Timing against the project.
    let mut timing = Value::Null;
    if let Some(project) = request.project {
        let duration = project.duration()?;
        let expected_samples = (duration.num as u128 * 48_000 / duration.den as u128) as u64;
        let mut problems = Vec::new();
        let mut video_ok = Value::Null;
        if let (Some(count), Some(rate)) = (frames, frame_rate) {
            let wanted = duration.units(project.frame_rate)?;
            let ok = rate == project.frame_rate && count == wanted;
            if !ok {
                problems.push(format!(
                    "video {count} frames at {rate} fps, expected {wanted} at {} fps",
                    project.frame_rate
                ));
            }
            video_ok = json!({"frames":count,"expected_frames":wanted,"frame_rate_matches":rate == project.frame_rate,"ok":ok});
        }
        let mut audio_ok = Value::Null;
        if let Some(count) = samples {
            let ok = count.abs_diff(expected_samples) <= AUDIO_SLACK;
            if !ok {
                problems.push(format!(
                    "audio {count} samples, expected {expected_samples}"
                ));
            }
            audio_ok = json!({"samples":count,"expected_samples":expected_samples,"slack_samples":AUDIO_SLACK,"ok":ok});
        }
        lines.push(if problems.is_empty() {
            format!(
                "timing: matches project {} rev {} ({} s)",
                project.id,
                project.revision,
                clock.at(duration)
            )
        } else {
            format!(
                "timing: differs from project {} rev {} ({} s): {}",
                project.id,
                project.revision,
                clock.at(duration),
                problems.join("; ")
            )
        });
        timing = json!({"project_id":project.id,"revision":project.revision,"duration":duration,"video":video_ok,"audio":audio_ok});
    }

    // Speech: what the timeline should say against what the cut says. A recognition failure
    // loses only this comparison; the picture and sound above are already reviewed.
    let mut speech = Value::Null;
    let mut recognition = Value::Null;
    let mut failure = None;
    let mut heard_documents: Vec<Document> = request.heard.to_vec();
    let recognized = match (request.recognize, samples) {
        (Some(settings), Some(count)) => {
            let value = crate::identity::relative(&wav, &dir)?;
            let wav_identity = Identity {
                sha256: value["sha256"].as_str().unwrap_or_default().to_owned(),
                bytes: value["bytes"].as_u64().unwrap_or_default(),
            };
            let vocabulary = known_terms(
                request.transcripts,
                script.as_ref().map_or(&[][..], |s| &s.0[..]),
            );
            match recognize(&dir, count, &wav_identity, settings, &vocabulary) {
                Ok(found) => {
                    fs::write(
                        dir.join("transcripts.json"),
                        serde_json::to_vec_pretty(
                            &json!({"transcripts":found.documents,"non_speech":found.non_speech}),
                        )?,
                    )?;
                    files.push("audio.wav".to_owned());
                    files.push("transcripts.json".to_owned());
                    if !found.non_speech.is_empty() {
                        lines.push(format!(
                            "heard besides speech: {}{}",
                            found
                                .non_speech
                                .iter()
                                .take(SUMMARY_RUNS)
                                .map(|n| format!(
                                    "{}{} {}",
                                    n["text"].as_str().unwrap_or_default(),
                                    // Text the recognizer wrote where the acoustic model heard no speech.
                                    if n["kind"] == "unheard" {
                                        " (no speech heard)"
                                    } else {
                                        ""
                                    },
                                    clock.span(time_of(&n["start"]), time_of(&n["end"]))
                                ))
                                .collect::<Vec<_>>()
                                .join(", "),
                            more(found.non_speech.len())
                        ));
                    }
                    recognition = json!({"ok":true,"documents":found.documents.len(),"vocabulary":vocabulary,
                        "non_speech":{"count":found.non_speech.len(),
                            "listed":found.non_speech.iter().take(MAX_LISTED).collect::<Vec<_>>()},
                        "silent_windows":found.silent});
                    heard_documents = found.documents.clone();
                    Some(heard(&found.documents, &wav_identity)?)
                }
                Err(e) => {
                    failure = Some(format!("recognition failed with {}: {}", e.code, e.message));
                    recognition = json!({"ok":false,"error":e});
                    None
                }
            }
        }
        _ => None,
    };
    if !files.iter().any(|f| f == "audio.wav") {
        let _ = fs::remove_file(&wav);
    }
    let heard_words = heard_given.or(recognized);
    if script.is_some() || heard_words.is_some() || !recognition.is_null() {
        let (said, cut, unused) = match script {
            Some((said, cut, unused)) => (Some(said), cut, unused),
            None => (None, Vec::new(), Vec::new()),
        };
        let mut section = json!({"unused_transcripts":unused,
            "cut_words":{"count":cut.len(),"listed":cut.iter().take(MAX_LISTED).collect::<Vec<_>>()}});
        match (&said, &heard_words) {
            (Some(said), Some(heard_words)) => {
                let (comparison, differences) = compare(said, heard_words, request.tolerance)?;
                lines.push(format!(
                    "speech: {} of {} expected words heard{}, {} heard in all; {} difference{}{}",
                    comparison["matched"],
                    said.len(),
                    comparison["match_ratio"]
                        .as_f64()
                        .map_or(String::new(), |r| format!(" ({:.1}%)", r * 100.0)),
                    heard_words.len(),
                    differences.len(),
                    if differences.len() == 1 { "" } else { "s" },
                    if differences.is_empty() { "" } else { ":" }
                ));
                for difference in differences.iter().take(MAX_LISTED) {
                    let wanted = difference["expected"].as_str().unwrap_or_default();
                    let got = difference["heard"].as_str().unwrap_or_default();
                    let what = match (wanted.is_empty(), got.is_empty()) {
                        (false, true) => format!("missing \"{wanted}\""),
                        (true, false) => format!("extra \"{got}\""),
                        _ => format!("expected \"{wanted}\" heard \"{got}\""),
                    };
                    lines.push(format!(
                        "  {} {what}",
                        clock.span(time_of(&difference["start"]), time_of(&difference["end"]))
                    ));
                }
                if differences.len() > MAX_LISTED {
                    lines.push(format!("  +{} more", differences.len() - MAX_LISTED));
                }
                section["comparison"] = comparison;
            }
            (Some(said), None) => {
                lines.push(match &failure {
                    Some(failure) => format!(
                        "speech: {} words expected; not compared because {failure}",
                        said.len()
                    ),
                    None => format!(
                        "speech: {} words expected; not compared (give heard transcripts or a recognition runtime)",
                        said.len()
                    ),
                });
                section["expected_words"] = json!(said.len());
            }
            (None, Some(heard_words)) => {
                lines.push(format!(
                    "speech: {} words heard; not compared (give the project and source transcripts)",
                    heard_words.len()
                ));
                section["heard_words"] = json!(heard_words.len());
            }
            (None, None) => {
                if let Some(failure) = &failure {
                    lines.push(format!("speech: {failure}"));
                }
            }
        }
        if !recognition.is_null() {
            section["recognition"] = recognition;
        }
        // Sounds no heard word covers: a filler left in the cut, or a word recognition missed.
        let uncovered = heard_uncovered(&heard_documents)?;
        if let Some(listed) = uncovered["listed"].as_array().filter(|l| !l.is_empty()) {
            lines.push(format!(
                "uncovered speech (a left-in filler or a missed word): {}{}",
                listed
                    .iter()
                    .take(SUMMARY_RUNS)
                    .map(|u| format!(
                        "\"{}\" {}",
                        u["letters"].as_str().unwrap_or_default(),
                        clock.span(time_of(&u["start"]), time_of(&u["end"]))
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                more(uncovered["count"].as_u64().unwrap_or_default() as usize)
            ));
        }
        if uncovered["count"] != 0 {
            section["uncovered"] = uncovered;
        }
        if !cut.is_empty() {
            lines.push(format!(
                "cut words: {}",
                cut.iter()
                    .take(MAX_LISTED)
                    .map(|c| format!(
                        "{} {} inside \"{}\" at {}",
                        c["clip_id"].as_str().unwrap_or_default(),
                        if c["edge"] == "start" {
                            "starts"
                        } else {
                            "ends"
                        },
                        c["word"].as_str().unwrap_or_default(),
                        clock.at(time_of(&c["time"]))
                    ))
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        if let Some(documents) = files.iter().find(|f| *f == "transcripts.json") {
            section["transcripts"] = json!(documents);
        }
        speech = section;
    }
    files.push("review.json".to_owned());
    lines.push(format!(
        "files in {}: {}",
        dir.file_name().unwrap_or_default().to_string_lossy(),
        files.join(", ")
    ));
    if clock.rounded {
        lines.push("times in seconds; ~ rounded to the millisecond".to_owned());
    }
    let mut summary = lines.join("\n");
    summary.push('\n');
    let mut result = json!({"output":dir,"summary":summary,"file":file,"timing":timing,
        "picture":picture_section,"sound":sound_section,"speech":speech,"files":files});
    fs::write(dir.join("review.json"), serde_json::to_vec_pretty(&result)?)?;
    folder.keep = true;
    // The reply stays short; per-second loudness and full meters are in review.json.
    if let Some(sound) = result["sound"].as_object().cloned() {
        let short: Vec<f64> = sound["over_time"]["short_term_lkfs"]
            .as_array()
            .map(|v| v.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        let range = match (
            short.iter().copied().reduce(f64::min),
            short.iter().copied().reduce(f64::max),
        ) {
            (Some(low), Some(high)) => json!([low, high]),
            _ => Value::Null,
        };
        result["sound"] = json!({"integrated_lkfs":sound["meters"]["integrated_lkfs"],
            "loudness_status":sound["meters"]["loudness_status"],"sample_peak_dbfs":sound["meters"]["sample_peak_dbfs"],
            "short_term_lkfs_range":range,"silence":sound["over_time"]["silence"],"clipping":sound["over_time"]["clipping"]});
    }
    result["details"] = json!("review.json");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(words: &[(u64, &str)]) -> Vec<Said> {
        words
            .iter()
            .map(|&(at, text)| Said {
                start: Time::new(at, 10).unwrap(),
                end: Time::new(at + 2, 10).unwrap(),
                text: text.into(),
            })
            .collect()
    }

    #[test]
    fn names_from_the_expected_words_prompt_recognition() {
        let mut document = crate::transcript::tests::fixture();
        document.recognition.vocabulary = vec!["um".into(), "Cutbolt".into()];
        let expected = said(&[
            (0, "Drawn"),
            (2, "with"),
            (4, "PixelForge,"),
            (6, "then"),
            (8, "cut."),
            (10, "I"),
            (12, "used"),
            (14, "Cutbolt"),
            (16, "and"),
            (18, "Greek."),
            (20, "Then"),
            (22, "I'm"),
            (24, "done."),
        ]);
        assert_eq!(
            known_terms(&[document], &expected),
            ["um", "Cutbolt", "PixelForge", "Greek"]
        );
        assert!(known_terms(&[], &said(&[(0, "Plain"), (2, "words.")])).is_empty());
    }

    #[test]
    fn split_and_joined_names_still_match() {
        let tolerance = Time::new(1, 2).unwrap();
        let expected = said(&[
            (0, "drawn"),
            (2, "with"),
            (4, "PixelForge,"),
            (8, "using"),
            (10, "cut"),
            (12, "bolt."),
        ]);
        let heard = said(&[
            (0, "drawn"),
            (2, "with"),
            (4, "pixel"),
            (6, "forge"),
            (8, "using"),
            (11, "Cutbolt"),
        ]);
        let (comparison, differences) = compare(&expected, &heard, tolerance).unwrap();
        assert_eq!(comparison["matched"], 6);
        assert_eq!(comparison["joined_matches"], 2);
        assert!(differences.is_empty(), "{differences:?}");
        // A join still needs the right letters.
        let heard = said(&[
            (0, "drawn"),
            (2, "with"),
            (4, "pixel"),
            (6, "forged"),
            (8, "using"),
        ]);
        let (comparison, differences) = compare(&expected, &heard, tolerance).unwrap();
        assert_eq!(
            (
                comparison["matched"].as_u64(),
                comparison["joined_matches"].as_u64()
            ),
            (Some(3), Some(0))
        );
        assert_eq!(differences[0]["expected"], "PixelForge,");
        assert_eq!(differences[0]["heard"], "pixel forged");
    }

    #[test]
    fn spoken_numbers_match_their_numerals() {
        let tolerance = Time::new(1, 2).unwrap();
        // The part-two demo's narration against what recognition wrote down.
        let expected = said(&[
            (0, "This"),
            (2, "morning,"),
            (4, "an"),
            (6, "eighty"),
            (8, "second"),
            (10, "video"),
            (12, "took"),
            (14, "over"),
            (16, "two"),
            (18, "hours"),
            (20, "and"),
            (22, "a"),
            (24, "hundred"),
            (26, "and"),
            (28, "seventy"),
            (30, "tool"),
            (32, "calls."),
        ]);
        let heard = said(&[
            (0, "This"),
            (2, "morning,"),
            (4, "an"),
            (6, "80"),
            (8, "second"),
            (10, "video"),
            (12, "took"),
            (14, "over"),
            (16, "2"),
            (18, "hours"),
            (20, "and"),
            (25, "170"),
            (30, "tool"),
            (32, "calls."),
        ]);
        let (comparison, differences) = compare(&expected, &heard, tolerance).unwrap();
        assert!(differences.is_empty(), "{differences:?}");
        assert_eq!(comparison["matched"], 17);
        assert_eq!(comparison["number_matches"], 3);
        assert_eq!(comparison["joined_matches"], 1);
        // Either side may be the numeral, and a number spoken over many words still matches by the
        // middle of its whole span.
        let (comparison, differences) = compare(&heard, &expected, tolerance).unwrap();
        assert!(differences.is_empty(), "{differences:?}");
        assert_eq!(comparison["matched"], 14);
        let long = said(&[
            (0, "in"),
            (2, "one"),
            (4, "thousand"),
            (6, "nine"),
            (8, "hundred"),
            (10, "and"),
            (12, "ninety."),
        ]);
        let short = said(&[(0, "in"), (7, "1990.")]);
        let (comparison, differences) = compare(&long, &short, tolerance).unwrap();
        assert!(differences.is_empty(), "{differences:?}");
        assert_eq!(comparison["matched"], 7);
        // A different number is a difference.
        let wrong = said(&[(0, "in"), (7, "1999.")]);
        let (comparison, differences) = compare(&long, &wrong, tolerance).unwrap();
        assert_eq!(comparison["matched"], 1);
        assert_eq!(differences[0]["heard"], "1999.");
    }

    #[test]
    fn overlapping_heard_transcripts_list_a_sound_once() {
        let t = |s: u64| Time::new(s, 1).unwrap();
        let sound = |a: u64| crate::transcript::Uncovered {
            start: t(a),
            end: t(a + 1),
            letters: "AM".into(),
        };
        let window = |start: u64, sounds: Vec<crate::transcript::Uncovered>| {
            let mut document = crate::transcript::tests::fixture();
            document.source.duration = t(300);
            document.range_start = t(start);
            document.range_duration = t(120);
            document.words.clear();
            document.uncovered = sounds;
            document
        };
        // Windows of 0-120 s and 115-235 s meet at 117.5 s; both heard the sound across it.
        let later = window(115, vec![sound(117), sound(130)]);
        let earlier = window(0, vec![sound(100), sound(117)]);
        let listing = heard_uncovered(&[later, earlier]).unwrap();
        let starts: Vec<&Value> = listing["listed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| &s["start"])
            .collect();
        assert_eq!(listing["count"], 3);
        assert_eq!(starts, [&json!(t(100)), &json!(t(117)), &json!(t(130))]);
    }
}
