//! Whole-file speech recognition. A source's audio is extracted losslessly, recognized in
//! overlapping windows within the 120 s transcription limit, stitched at word boundaries into
//! documents that do not overlap, bound to the source file itself and saved as one JSON file.
use crate::{
    Result, error, media,
    registry::Identity,
    time::Time,
    transcribe,
    transcript::{Document, Language, Source},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const WINDOW: u64 = 120 * 48_000;
const OVERLAP: u64 = 5 * 48_000;
/// The transcription minimum: 25 ms.
const SHORTEST: u64 = 1200;

/// What to recognize and where to save it.
pub struct Request<'a> {
    pub path: &'a Path,
    pub input_root: &'a Path,
    pub output_root: &'a Path,
    pub output: &'a Path,
    pub runtime: &'a transcribe::Runtime,
    pub language: Language,
    pub channel: transcribe::Channel,
    pub id: &'a str,
    pub start: Option<Time>,
    pub duration: Option<Time>,
    pub timeout_seconds: u32,
}

/// Owned scratch folder; removed afterwards.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Windows over `[start, end)` in samples: 120 s long, each starting 5 s before the previous
/// one ends, the last one ending at `end`.
pub(crate) fn windows(start: u64, end: u64) -> Vec<(u64, u64)> {
    let mut found = Vec::new();
    let mut at = start;
    while at < end {
        let stop = (at + WINDOW).min(end);
        found.push((at, stop));
        if stop == end {
            break;
        }
        at = stop - OVERLAP;
    }
    found
}

fn sample(time: Time) -> Result<u64> {
    time.units(Time::new(48_000, 1)?)
}

/// Make consecutive documents meet instead of overlapping. Where one ends inside the next, the
/// seam starts at the overlap's middle (on the sample grid): the earlier document keeps its words
/// whose middle lies before it, the seam moves to the end of its last kept word if that is later,
/// and the later document keeps its words starting at or after the seam.
pub(crate) fn stitch(mut documents: Vec<Document>) -> Result<Vec<Document>> {
    for i in 1..documents.len() {
        let (before, after) = documents.split_at_mut(i);
        let (left, right) = (before.last_mut().expect("left"), &mut after[0]);
        let left_end = sample(left.range_start.plus(left.range_duration)?)?;
        let right_start = sample(right.range_start)?;
        let right_end = sample(right.range_start.plus(right.range_duration)?)?;
        if right_start >= left_end {
            continue;
        }
        let middle = (right_start + left_end) / 2;
        let mut kept = Vec::new();
        for word in left.words.drain(..) {
            if sample(word.start)? + sample(word.end)? < 2 * middle {
                kept.push(word);
            }
        }
        left.words = kept;
        let mut seam = middle;
        if let Some(last) = left.words.last() {
            seam = seam.max(sample(last.end)?);
        }
        let mut kept = Vec::new();
        for word in right.words.drain(..) {
            if sample(word.start)? >= seam {
                kept.push(word);
            }
        }
        right.words = kept;
        left.range_duration = Time::new(seam - sample(left.range_start)?, 48_000)?;
        right.range_start = Time::new(seam, 48_000)?;
        right.range_duration = Time::new(right_end - seam, 48_000)?;
    }
    for document in &documents {
        document.validate()?;
    }
    Ok(documents)
}

/// Recognize a source file's speech into stitched transcript documents saved at `output`.
pub fn run(request: &Request) -> Result<Value> {
    crate::tracks::id(request.id)?;
    if !(1..=600).contains(&request.timeout_seconds) {
        return Err(error(
            "INVALID_ARGUMENT",
            "timeout_seconds must be 1-600 per window",
        ));
    }
    let path = media::allowed_file(request.path, request.input_root)?;
    let output = crate::render::destination_extension(request.output, request.output_root, "json")?;
    if output.try_exists()? {
        return Err(error(
            "OUTPUT_EXISTS",
            format!("{} already exists", output.display()),
        ));
    }
    let source = crate::identity::relative(&path, request.input_root)?;
    let identity = Identity {
        sha256: source["sha256"].as_str().unwrap_or_default().to_owned(),
        bytes: source["bytes"].as_u64().unwrap_or_default(),
    };
    let relative = PathBuf::from(source["path"].as_str().unwrap_or_default());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
        .as_nanos();
    let scratch = Scratch(
        std::env::temp_dir().join(format!("cutbolt-transcribe-{}-{nonce}", std::process::id())),
    );
    fs::create_dir(&scratch.0)?;
    // Lossless 48 kHz stereo PCM16 of the first audio stream: word times are source times.
    let wav = scratch.0.join("audio.wav");
    let mut args: Vec<String> = ["-nostdin", "-v", "error", "-n", "-i"]
        .map(str::to_owned)
        .to_vec();
    args.push(path.to_string_lossy().into_owned());
    args.extend(
        [
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
        ]
        .map(str::to_owned),
    );
    args.push(wav.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(4 * 3600))?;
    let (total, _, _) = crate::audio_processing::measure_wav(&wav, false)?;
    let first = match request.start {
        Some(start) => sample(start)?,
        None => 0,
    };
    let last = match request.duration {
        Some(duration) => first + sample(duration)?,
        None => total,
    };
    if last > total || last.saturating_sub(first) < SHORTEST {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "The range must hold at least 25 ms of the file's {} s of audio",
                Time::new(total, 48_000)?
            ),
        ));
    }
    let wav_identity = {
        let value = crate::identity::relative(&wav, &scratch.0)?;
        Identity {
            sha256: value["sha256"].as_str().unwrap_or_default().to_owned(),
            bytes: value["bytes"].as_u64().unwrap_or_default(),
        }
    };
    let duration = Time::new(total, 48_000)?;
    let plan = windows(first, last);
    let mut documents = Vec::with_capacity(plan.len());
    for (index, &(from, to)) in plan.iter().enumerate() {
        let result = transcribe::run(&transcribe::Transcribe {
            id: format!("{}-{}", request.id, index + 1),
            source: Source {
                path: PathBuf::from("audio.wav"),
                identity: wav_identity.clone(),
                duration,
            },
            format: transcribe::Format::StereoWav,
            start: Time::new(from, 48_000)?,
            duration: Time::new(to - from, 48_000)?,
            channel: request.channel,
            language: request.language,
            input_root: scratch.0.clone(),
            scratch_root: scratch.0.clone(),
            runtime: request.runtime.clone(),
            timeout_seconds: request.timeout_seconds,
        })?;
        let mut document: Document = serde_json::from_value(result["document"].clone())?;
        // The extracted audio is the source's own audio, sample for sample.
        document.source = Source {
            path: relative.clone(),
            identity: identity.clone(),
            duration,
        };
        documents.push(document);
    }
    let documents = stitch(documents)?;
    let words: usize = documents.iter().map(|d| d.words.len()).sum();
    let saved = json!({"transcripts":documents});
    let mut file = fs::File::create_new(&output)?;
    file.write_all(&serde_json::to_vec_pretty(&saved)?)?;
    file.sync_all()?;
    Ok(
        json!({"output":output,"source":source,"duration":duration,"documents":documents.len(),"words":words,
        "ranges":documents.iter().map(|d| json!({"id":d.id,"start":d.range_start,"duration":d.range_duration,"words":d.words.len()})).collect::<Vec<_>>(),
        "review_required":true,
        "next":"pass the file's transcripts (with a workspace, {\"file\": output, \"select\": \"transcripts\"}) to timeline.outline, captions.draft, export.review or transcript.plan"}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{Origin, Word};

    fn word(id: &str, start: u64, end: u64) -> Word {
        Word {
            id: id.into(),
            text: id.into(),
            start: Time::new(start, 48_000).unwrap(),
            end: Time::new(end, 48_000).unwrap(),
            origin: Origin::Estimated,
            probability_milli: Some(900),
            alignment: None,
        }
    }

    fn document(id: &str, start: u64, end: u64, words: Vec<Word>) -> Document {
        let mut d = crate::transcript::tests::fixture();
        d.id = id.into();
        d.source.duration = Time::new(400 * 48_000, 1).unwrap();
        d.range_start = Time::new(start, 48_000).unwrap();
        d.range_duration = Time::new(end - start, 48_000).unwrap();
        d.words = words;
        d
    }

    #[test]
    fn windows_overlap_by_five_seconds_and_end_at_the_range_end() {
        assert_eq!(windows(0, 1200), vec![(0, 1200)]);
        assert_eq!(windows(0, WINDOW), vec![(0, WINDOW)]);
        assert_eq!(
            windows(7, 250 * 48_000),
            vec![
                (7, 7 + WINDOW),
                (7 + WINDOW - OVERLAP, 7 + 2 * WINDOW - OVERLAP),
                (7 + 2 * WINDOW - 2 * OVERLAP, 250 * 48_000)
            ]
        );
    }

    #[test]
    fn stitching_meets_at_word_boundaries_without_duplicates() {
        let s = 48_000;
        // Overlap [115 s, 120 s); its middle is 117.5 s. "b" has its middle before it and ends
        // after it, so it stays left and the seam moves to its end, 117.75 s.
        let left = document(
            "l",
            0,
            120 * s,
            vec![
                word("a", 114 * s, 115 * s),
                word("b", 116 * s + s / 2, 117 * s + 3 * s / 4),
                word("c", 119 * s, 120 * s),
            ],
        );
        let right = document(
            "r",
            115 * s,
            235 * s,
            vec![
                word("a2", 114 * s + s / 2, 115 * s),
                word("b2", 117 * s, 118 * s),
                word("c", 119 * s, 120 * s),
            ],
        );
        let stitched = stitch(vec![left, right]).unwrap();
        let ids = |d: &Document| d.words.iter().map(|w| w.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&stitched[0]), ["a", "b"]);
        assert_eq!(ids(&stitched[1]), ["c"]);
        assert_eq!(stitched[0].range_duration, Time::new(471, 4).unwrap());
        assert_eq!(stitched[1].range_start, Time::new(471, 4).unwrap());
        assert_eq!(stitched[1].range_duration, Time::new(469, 4).unwrap());
        // Documents that already meet are left alone.
        let apart = stitch(vec![
            document("x", 0, 10 * s, vec![word("x", s, 2 * s)]),
            document("y", 10 * s, 20 * s, vec![word("y", 11 * s, 12 * s)]),
        ])
        .unwrap();
        assert_eq!(apart[0].range_duration, Time::new(10, 1).unwrap());
        assert_eq!(apart[1].words.len(), 1);
    }
}
