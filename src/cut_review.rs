//! Review of a rendered cut, written to one new folder: a contact sheet, a small watchable copy,
//! loudness over time, black frames, duration against the project, and whether the cut says what
//! the timeline intended, by comparing the words expected from the source transcripts with the
//! words heard in the cut.
use crate::{
    Result, error, media,
    model::Project,
    outline::Clock,
    registry::Identity,
    time::Time,
    tracks::Kind,
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
    let parent = output
        .parent()
        .ok_or_else(|| error("INVALID_PATH", "Missing output parent"))?
        .canonicalize()?;
    if !parent.starts_with(root.canonicalize()?) {
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

/// A word on the cut's clock.
#[derive(Clone)]
struct Said {
    start: Time,
    end: Time,
    text: String,
}
impl Said {
    fn middle(&self) -> Result<Time> {
        self.start.plus(self.end)?.times(Time::new(1, 2)?)
    }
}

/// Lowercase letters and digits only, so punctuation and case do not count as differences.
fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Words the timeline should say: every whole transcript word inside an audible audio clip,
/// moved to timeline time. Words a clip edge cuts through are reported instead.
fn expected(
    project: &Project,
    transcripts: &[Document],
    input_root: &Path,
) -> Result<(Vec<Said>, Vec<Value>, Vec<Value>)> {
    let matches = crate::outline::spoken(project, transcripts, Some(input_root))?;
    // (clip ID, timeline start, source in, duration, asset)
    let mut clips: Vec<(&str, Time, Time, Time, &str)> = Vec::new();
    match &project.tracks {
        Some(arrangement) => {
            for track in &arrangement.tracks {
                if !track.enabled || track.kind != Kind::Audio {
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
    Ok((said, cut, matches.unused))
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
                text: word.text.clone(),
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

/// Recognize the cut's audio in overlapping windows with the local speech runtime.
fn recognize(
    folder: &Path,
    samples: u64,
    identity: &Identity,
    settings: &Recognize,
) -> Result<Vec<Document>> {
    let source = transcript::Source {
        path: PathBuf::from("audio.wav"),
        identity: identity.clone(),
        duration: Time::new(samples, 48_000)?,
    };
    let mut documents = Vec::new();
    let mut start = 0;
    while start < samples {
        let end = (start + WINDOW_SAMPLES).min(samples);
        if end - start < 1200 && start > 0 {
            break;
        }
        let result = transcribe::run(&transcribe::Transcribe {
            id: format!("heard-{}", documents.len()),
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
        })?;
        documents.push(serde_json::from_value(result["document"].clone())?);
        if end == samples {
            break;
        }
        start = end - OVERLAP_SAMPLES;
    }
    Ok(documents)
}

/// Match expected and heard words in order: a heard word matches when its text is the same and
/// its middle lies within `tolerance` of the expected word's middle. Unmatched words between two
/// matches form one difference.
fn compare(expected: &[Said], heard: &[Said], tolerance: Time) -> Result<(Value, Vec<Value>)> {
    let mut matched_expected = vec![false; expected.len()];
    let mut matched_heard = vec![false; heard.len()];
    let mut next = 0;
    let mut matches = 0;
    for (i, word) in expected.iter().enumerate() {
        let middle = word.middle()?;
        let text = normalized(&word.text);
        let mut k = next;
        while k < heard.len() {
            let candidate = heard[k].middle()?;
            if candidate.compare(middle.plus(tolerance)?)?.is_gt() {
                break;
            }
            let near = middle.compare(candidate.plus(tolerance)?)?.is_le();
            if near && normalized(&heard[k].text) == text {
                matched_expected[i] = true;
                matched_heard[k] = true;
                matches += 1;
                next = k + 1;
                break;
            }
            k += 1;
        }
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
            "match_ratio":ratio(matches, expected.len()),"tolerance":tolerance,
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
            Some(expected(project, request.transcripts, request.input_root)?)
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
    if request.recognize.is_some() && !has_audio {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Recognition needs an audio stream",
        ));
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

    // Speech: what the timeline should say against what the cut says.
    let mut speech = Value::Null;
    let recognized = match (request.recognize, samples) {
        (Some(settings), Some(count)) => {
            let value = crate::identity::relative(&wav, &dir)?;
            let wav_identity = Identity {
                sha256: value["sha256"].as_str().unwrap_or_default().to_owned(),
                bytes: value["bytes"].as_u64().unwrap_or_default(),
            };
            let documents = recognize(&dir, count, &wav_identity, settings)?;
            fs::write(
                dir.join("transcripts.json"),
                serde_json::to_vec_pretty(&json!({"transcripts":documents}))?,
            )?;
            files.push("audio.wav".to_owned());
            files.push("transcripts.json".to_owned());
            Some(heard(&documents, &wav_identity)?)
        }
        _ => None,
    };
    if !files.iter().any(|f| f == "audio.wav") {
        let _ = fs::remove_file(&wav);
    }
    let heard_words = heard_given.or(recognized);
    if script.is_some() || heard_words.is_some() {
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
                lines.push(format!(
                    "speech: {} words expected; not compared (give heard transcripts or a recognition runtime)",
                    said.len()
                ));
                section["expected_words"] = json!(said.len());
            }
            (None, Some(heard_words)) => {
                lines.push(format!(
                    "speech: {} words heard; not compared (give the project and source transcripts)",
                    heard_words.len()
                ));
                section["heard_words"] = json!(heard_words.len());
            }
            (None, None) => {}
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
