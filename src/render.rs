use crate::{Result, error, media, model::Project, time::Time};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(crate) mod clock;

// Large native frames benefit from slice parallelism. Keep a fixed worker ceiling
// independent of host CPU count and retain the ordinary small-frame profile.
fn large_raster(width: u32, height: u32) -> bool {
    u64::from(width) * u64::from(height) >= 8_000_000 && width >= 4 && height >= 4
}

#[derive(Debug, Serialize, Clone)]
pub struct Source {
    pub path: PathBuf,
    pub sha256: String,
    pub frames: u64,
    pub samples: u64,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub profile: &'static str,
    pub source_quality: &'static str,
    pub project_revision: u64,
    pub frames: u64,
    pub samples: u64,
    pub output: PathBuf,
    pub sources: Vec<Source>,
    pub arguments: Vec<String>,
    /// Consecutive windows rendered separately and joined by stream copy, when one graph would
    /// hold more than MAX_GRAPH_CLIPS clips; empty for a single graph.
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "chunk_summary"
    )]
    pub chunks: Vec<Plan>,
    /// Gain streams and limited mixes this graph reads, written when it runs.
    #[serde(skip)]
    pub(crate) generated: Vec<Generated>,
    /// When top-level `alpha_over` clips show: the engine compositor whose frames `arguments`
    /// (the encoder) reads on stdin.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) compositor: Option<crate::track_composite::Compositor>,
}
/// An input a track graph reads that the engine writes just before the graph runs and removes
/// after.
#[derive(Debug, Clone)]
pub(crate) enum Generated {
    /// A per-sample gain stream: a clip's gain curve sampled on its source clock from `start`,
    /// `samples` long, as 16-bit mono PCM (gains are at most 4000, so every value is exact).
    Gain {
        path: PathBuf,
        curve: crate::animation::Curve,
        start: Time,
        samples: u64,
    },
    /// A window's audio mixed through track and master limiters.
    Mix(Box<crate::dynamics::Premix>),
}
impl Generated {
    fn write(&self, control: &dyn media::Control, timeout: Duration) -> Result<TempFile> {
        use std::io::Write;
        let (path, curve, start, samples) = match self {
            Generated::Gain {
                path,
                curve,
                start,
                samples,
            } => (path, curve, *start, *samples),
            Generated::Mix(premix) => {
                let file = TempFile(premix.path.clone());
                premix.write(control, timeout)?;
                return Ok(file);
            }
        };
        // Keys were validated against the clip's source; the latest key bounds evaluation here.
        let span = curve
            .keys
            .iter()
            .map(|k| k.time)
            .max_by(|a, b| a.compare(*b).expect("valid key times"))
            .unwrap_or(Time::ZERO);
        let sampler = curve.prepare_keys(span, 0, 4000, crate::tracks::MAX_GAIN_KEYS)?;
        let file = TempFile(path.clone());
        let bytes = u32::try_from(samples * 2)
            .map_err(|_| error("LIMIT_EXCEEDED", "Gain automation window exceeds 4 GiB"))?;
        let mut out = std::io::BufWriter::new(fs::File::create_new(path)?);
        out.write_all(b"RIFF")?;
        out.write_all(&(36 + bytes).to_le_bytes())?;
        out.write_all(b"WAVEfmt ")?;
        for value in [16u32, 1 | (1 << 16), 48_000, 96_000, 2 | (16 << 16)] {
            out.write_all(&value.to_le_bytes())?;
        }
        out.write_all(b"data")?;
        out.write_all(&bytes.to_le_bytes())?;
        for n in 0..samples {
            let gain = sampler.sample(start.plus(Time::new(n, 48_000)?)?)?;
            out.write_all(&(gain as i16).to_le_bytes())?;
        }
        out.flush()?;
        Ok(file)
    }
}

/// Write a graph's generated inputs; they are removed when the returned guards drop.
pub(crate) fn write_generated(
    generated: &[Generated],
    control: &dyn media::Control,
    timeout: Duration,
) -> Result<Vec<TempFile>> {
    generated
        .iter()
        .map(|g| g.write(control, timeout))
        .collect()
}

impl Plan {
    /// Gain reduction of the limiters this plan ran, once it has run; None without dynamics.
    pub(crate) fn dynamics(&self) -> Option<Value> {
        fn collect(plan: &Plan, report: &mut Option<crate::dynamics::Report>) {
            for generated in &plan.generated {
                if let Generated::Mix(premix) = generated
                    && let Some(part) = &*premix.report.lock().expect("dynamics report")
                {
                    report.get_or_insert_default().merge(part);
                }
            }
            for chunk in &plan.chunks {
                collect(chunk, report);
            }
        }
        let mut report = None;
        collect(self, &mut report);
        report.map(|r| r.value())
    }
}

/// Chunks are reported by size only; their graphs are internal.
fn chunk_summary<S: serde::Serializer>(
    chunks: &[Plan],
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use serde::ser::SerializeSeq;
    let mut seq = serializer.serialize_seq(Some(chunks.len()))?;
    for chunk in chunks {
        seq.serialize_element(&json!({"frames":chunk.frames,"samples":chunk.samples}))?;
    }
    seq.end()
}

/// Clips one FFmpeg graph may hold. Longer timelines render as chunks of at most this many clips,
/// joined losslessly into one output.
pub(crate) const MAX_GRAPH_CLIPS: usize = 64;

/// Clips a render of `[start, end)` reads: placed clips and transition endpoints intersecting it,
/// or sequential items (gaps included) overlapping it.
fn clips_in(project: &Project, start: Time, end: Time) -> Result<usize> {
    if let Some(a) = &project.tracks {
        return crate::track_render::window_clips(a, start, end);
    }
    let mut offset = Time::ZERO;
    let mut count = 0;
    for clip in &project.clips {
        let last = offset.plus(clip.duration)?;
        if offset.compare(end)?.is_lt() && last.compare(start)?.is_gt() {
            count += 1;
        }
        offset = last;
    }
    Ok(count)
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Split `[start, start + duration)` into windows of at most MAX_GRAPH_CLIPS clips, or return
/// None when one graph holds it. Every window but the last lasts a whole number of frames that is
/// also a whole number of milliseconds (so of 48 kHz samples): stream-copied chunks then keep the
/// exact container clock, as a single render would.
fn windows(project: &Project, start: Time, duration: Time) -> Result<Option<Vec<(Time, Time)>>> {
    let end = start.plus(duration)?;
    if clips_in(project, start, end)? <= MAX_GRAPH_CLIPS {
        return Ok(None);
    }
    let rate = project.frame_rate;
    let step_frames = rate.num / gcd(rate.num, rate.den * 1000);
    let step = Time::new(step_frames * rate.den, rate.num)?;
    let mut windows = Vec::new();
    let mut cursor = start;
    while cursor.compare(end)?.is_lt() {
        let rest = end.minus(cursor)?;
        if clips_in(project, cursor, end)? <= MAX_GRAPH_CLIPS {
            windows.push((cursor, rest));
            break;
        }
        // The longest whole number of steps that still fits, by bisection over a monotone count.
        let (mut low, mut high) = (0, rest.units(rate)? / step_frames);
        while low < high {
            let middle = (low + high).div_ceil(2);
            let window_end = cursor.plus(step.times(Time::new(middle, 1)?)?)?;
            if clips_in(project, cursor, window_end)? <= MAX_GRAPH_CLIPS {
                low = middle;
            } else {
                high = middle - 1;
            }
        }
        if low == 0 {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "More than {MAX_GRAPH_CLIPS} clips overlap within {step_frames} frames at {cursor} s"
                ),
            ));
        }
        let length = step.times(Time::new(low, 1)?)?;
        windows.push((cursor, length));
        cursor = cursor.plus(length)?;
    }
    Ok(Some(windows))
}

/// A plan that renders each window with `single` to a hidden chunk and joins the chunks by stream
/// copy into `output`, or None when one graph holds the whole range.
fn chunked(
    project: &Project,
    output_root: &Path,
    output: &Path,
    extension: &str,
    start: Time,
    duration: Time,
    single: impl Fn(Time, Time, &Path) -> Result<Plan>,
) -> Result<Option<Plan>> {
    let Some(windows) = windows(project, start, duration)? else {
        return Ok(None);
    };
    let output = destination_extension(output, output_root, extension)?;
    let mut chunks = Vec::new();
    for (index, (start, duration)) in windows.into_iter().enumerate() {
        let placeholder = output.with_file_name(format!(".cutbolt-chunk-{index}.{extension}"));
        chunks.push(single(start, duration, &placeholder)?);
    }
    let mut sources: Vec<Source> = Vec::new();
    for source in chunks.iter().flat_map(|c| &c.sources) {
        if !sources.iter().any(|s| s.path == source.path) {
            sources.push(source.clone());
        }
    }
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some(Plan {
        profile: chunks[0].profile,
        source_quality: chunks[0].source_quality,
        project_revision: project.revision,
        frames: chunks.iter().map(|c| c.frames).sum(),
        samples: chunks.iter().map(|c| c.samples).sum(),
        arguments: join_arguments(Path::new("{chunk list}"), &output, extension),
        output,
        sources,
        chunks,
        generated: Vec::new(),
        compositor: None,
    }))
}

/// FFmpeg arguments that join a concat list of chunk files by stream copy.
fn join_arguments(list: &Path, output: &Path, extension: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(list.to_string_lossy().into_owned());
    args.extend(
        [
            "-map",
            "0",
            "-c",
            "copy",
            "-f",
            if extension == "wav" {
                "wav"
            } else {
                "matroska"
            },
        ]
        .map(str::to_owned),
    );
    args.push(output.to_string_lossy().into_owned());
    args
}

/// Run a plan's FFmpeg work into `temp`: one graph, or each chunk into a hidden file beside `temp`
/// followed by a stream-copy join. Callers verify the result.
pub(crate) fn run_plan(
    plan: &Plan,
    temp: &Path,
    timeout: Duration,
    progress: bool,
    control: &dyn media::Control,
) -> Result<()> {
    let tool = control.tool("ffmpeg");
    // One graph into `target`; `done` frames precede it in the whole render.
    let single = |plan: &Plan, target: &Path, done: u64| -> Result<()> {
        let _inputs = write_generated(&plan.generated, control, timeout)?;
        let mut arguments = plan.arguments.to_vec();
        *arguments.last_mut().expect("output argument") = target.to_string_lossy().into_owned();
        if let Some(compositor) = &plan.compositor {
            // Allow composition beside the graph's own allowance: at least 20 Mpx per second.
            let pixels = u64::from(compositor.width) * u64::from(compositor.height);
            let extra = Duration::from_millis(compositor.frames * pixels / 20_000);
            return crate::track_composite::encode(
                compositor,
                &arguments,
                timeout + extra,
                done,
                control,
            );
        }
        if progress {
            arguments.splice(
                0..0,
                ["-progress".into(), "pipe:1".into(), "-nostats".into()],
            );
        }
        media::capture_controlled(&tool, &arguments, timeout, control).map(|_| ())
    };
    if plan.chunks.is_empty() {
        return single(plan, temp, 0);
    }
    let extension = temp
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mkv")
        .to_owned();
    let mut parts = Vec::new();
    let mut done = 0;
    for (index, chunk) in plan.chunks.iter().enumerate() {
        let part = TempFile(temp.with_extension(format!("chunk{index}.{extension}")));
        if part.0.try_exists()? {
            return Err(error("OUTPUT_EXISTS", "Temporary chunk collision"));
        }
        single(chunk, &part.0, done)?;
        done += chunk.frames;
        control.frames(done)?;
        parts.push(part);
    }
    let list = TempFile(temp.with_extension("chunks.txt"));
    let mut text = String::new();
    for (index, (part, chunk)) in parts.iter().zip(&plan.chunks).enumerate() {
        let path = part
            .0
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''");
        text.push_str(&format!("file '{path}'\n"));
        // State each chunk's exact length: the join offsets the next chunk by it, and a container's
        // own duration may round a final packet up by a millisecond.
        if index + 1 < parts.len() {
            let micros = u128::from(chunk.samples) * 1_000_000 / 48_000;
            text.push_str(&format!(
                "duration {}.{:06}\n",
                micros / 1_000_000,
                micros % 1_000_000
            ));
        }
    }
    fs::write(&list.0, text)?;
    let extension = if extension == "wav" { "wav" } else { "mkv" };
    media::capture_controlled(
        &tool,
        &join_arguments(&list.0, temp, extension),
        timeout,
        control,
    )?;
    Ok(())
}

pub(crate) fn micros(value: &Value) -> Result<i64> {
    let value = value
        .as_str()
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing frame timestamp"))?;
    if value.starts_with('-') {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Negative timestamps are unsupported",
        ));
    }
    let (whole, frac) = value.split_once('.').unwrap_or((value, ""));
    if frac.len() > 6 {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Timestamp precision unsupported",
        ));
    }
    let whole: i64 = whole
        .parse()
        .map_err(|_| error("UNSUPPORTED_MEDIA", "Invalid timestamp"))?;
    let frac: i64 = format!("{frac:0<6}")
        .parse()
        .map_err(|_| error("UNSUPPORTED_MEDIA", "Invalid timestamp"))?;
    whole
        .checked_mul(1_000_000)
        .and_then(|w| w.checked_add(frac))
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Timestamp overflow"))
}

/// The first backend deliberately accepts one narrow, testable lossless profile.
pub(crate) fn inspect_reference(
    path: &Path,
    width: u32,
    height: u32,
    control: &dyn media::Control,
) -> Result<Source> {
    inspect_reference_at(path, width, height, Time::new(25, 1)?, control)
}
pub(crate) fn inspect_reference_at(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
) -> Result<Source> {
    inspect_source_at(path, width, height, rate, control, false, Timing::Decoded)
}
/// The same reference profile for paths that only read audio: identity, stream metadata, audio
/// samples and video timestamps are checked, but video timing comes from FFV1 packets (one frame
/// each, never reordered) instead of decoding every picture.
pub(crate) fn inspect_reference_audio(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
) -> Result<Source> {
    inspect_source_at(path, width, height, rate, control, false, Timing::Packets)
}
/// A timeline source at its own size and rate, for media.inspect: the reference profile, or a
/// bgra alpha_over source. Like `inspect_reference_audio`, video timing comes from packets.
pub(crate) fn inspect_timeline_source(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
) -> Result<Source> {
    inspect_source_at(path, width, height, rate, control, true, Timing::Packets)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Timing {
    Decoded,
    Packets,
}
/// What one source inspection checks: the timeline's size and clock, whether straight alpha
/// (`bgra`) is accepted, and how video frames are timed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Check {
    pub width: u32,
    pub height: u32,
    pub rate: Time,
    pub alpha: bool,
    pub timing: Timing,
}
/// An inspection's parameters in its cache key; `rate` is the normalized native rate.
fn parameters(width: u32, height: u32, rate: Time, alpha: bool, timing: Timing) -> String {
    format!(
        "reference-source:{width}x{height}:{}/{}:alpha={alpha}:{}",
        rate.num,
        rate.den,
        if timing == Timing::Decoded {
            "decoded"
        } else {
            "packets"
        }
    )
}
/// A source with its video pixel format, or why its inspection failed.
pub(crate) type Inspected = Result<(Source, String)>;
/// Inspect as `check` says.
pub(crate) fn inspect_checked(
    path: &Path,
    check: Check,
    control: &dyn media::Control,
) -> Inspected {
    let Check {
        width,
        height,
        rate,
        alpha,
        timing,
    } = check;
    inspect_source_with(path, width, height, rate, control, alpha, timing)
}
/// Inspect several sources: largest file first and up to `inspection_cache::LANES` at a time
/// when `control` can be shared between threads, otherwise one after another. A cold timeline
/// then waits for about its slowest file rather than for all of them. Results follow `requests`.
pub(crate) fn inspect_many(
    requests: &[(PathBuf, Check)],
    control: &dyn media::Control,
) -> Vec<Inspected> {
    let Some(shared) = control.shared().filter(|_| requests.len() > 1) else {
        return requests
            .iter()
            .map(|(path, check)| inspect_checked(path, *check, control))
            .collect();
    };
    let mut order: Vec<usize> = (0..requests.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(fs::metadata(&requests[i].0).map_or(0, |m| m.len())));
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<_> = requests
        .iter()
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    std::thread::scope(|scope| {
        for _ in 0..crate::inspection_cache::LANES.min(requests.len()) {
            scope.spawn(|| {
                while let Some(&i) =
                    order.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                {
                    let (path, check) = &requests[i];
                    let result = inspect_checked(path, *check, shared);
                    *results[i]
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(result);
                }
            });
        }
    });
    results
        .into_iter()
        .map(|result| {
            result
                .into_inner()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .expect("every request is inspected")
        })
        .collect()
}
/// Reference source for an alpha_over track: FFV1 with straight alpha (bgra) or opaque bgr0.
/// Returns whether the source carries an alpha plane.
pub(crate) fn inspect_overlay(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
) -> Result<(Source, bool)> {
    let (source, pix_fmt) =
        inspect_source_with(path, width, height, rate, control, true, Timing::Decoded)?;
    Ok((source, pix_fmt == "bgra"))
}
fn inspect_source_at(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
    allow_alpha: bool,
    timing: Timing,
) -> Result<Source> {
    inspect_source_with(path, width, height, rate, control, allow_alpha, timing).map(|(s, _)| s)
}
/// One packet listing supplies the metadata, FFV1 video timing and exact PCM16 sample counts without
/// decoding; strict (`Decoded`) inspection then also decodes every video frame. Metadata is validated
/// before any decoding, so unsupported media is rejected exactly as by a plain probe. A passed
/// inspection is remembered by content identity (see `inspection_cache`): the file is still
/// hashed, and identical bytes with identical parameters, ffprobe and engine skip the probe.
/// A pass also records the other parameters it proves (see `implied`).
/// Returns the video pixel format with the source.
fn inspect_source_with(
    path: &Path,
    width: u32,
    height: u32,
    rate: Time,
    control: &dyn media::Control,
    allow_alpha: bool,
    timing: Timing,
) -> Result<(Source, String)> {
    let rate = clock::rate(rate)?;
    let bytes = fs::metadata(path)?.len();
    let before = media::file_hash_controlled(path, control)?;
    let ffprobe = control.tool("ffprobe");
    let key_of = |alpha, timing| {
        crate::inspection_cache::key(
            &before,
            bytes,
            &parameters(width, height, rate, alpha, timing),
            &ffprobe,
        )
    };
    let key = key_of(allow_alpha, timing);
    let cached = || {
        key.as_deref()
            .and_then(crate::inspection_cache::get)
            .map(|verified| {
                let source = Source {
                    path: path.into(),
                    sha256: before.clone(),
                    frames: verified.frames,
                    samples: verified.samples,
                };
                (source, verified.pix_fmt)
            })
    };
    if let Some(found) = cached() {
        return Ok(found);
    }
    let _turn = key.as_deref().map(crate::inspection_cache::turn);
    if let Some(found) = cached() {
        return Ok(found);
    }
    let _slot = crate::inspection_cache::slot();
    let inspection = media::packet_inspection_controlled(path, control)?;
    let metadata = &inspection.metadata;
    let streams = metadata["streams"]
        .as_array()
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing streams"))?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Video stream required"))?;
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Audio stream required"))?;
    if streams.len() != 2
        || video["codec_name"] != "ffv1"
        || !(video["pix_fmt"] == "bgr0" || (allow_alpha && video["pix_fmt"] == "bgra"))
        || video["width"] != width
        || video["height"] != height
        || !clock::rate_hint(video, metadata, rate)
        || audio["codec_name"] != "pcm_s16le"
        || audio["sample_rate"] != "48000"
        || audio["channels"] != 2
        || video.get("side_data_list").is_some()
    {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Reference renderer requires matching dimensions and frame rate, FFV1/bgr0 video and 48 kHz stereo PCM s16 audio",
        ));
    }
    if let Some(sar) = video["sample_aspect_ratio"].as_str()
        && sar != "1:1"
        && sar != "N/A"
    {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Only square pixels are supported",
        ));
    }
    let decoded;
    let frames = if timing == Timing::Packets {
        &inspection.video
    } else {
        // FFV1 decodes slices in parallel, and without `-threads` ffprobe uses one thread at every
        // size: a high-entropy 1080p frame then takes about 87 ms single-threaded, 20 ms on 16
        // threads. The decoded values are the same. Long sources, such as two-minute 60 fps scenes,
        // get 20 Mpx/s of allowance beyond the base.
        let pixels = inspection.video.len() as u64 * u64::from(width) * u64::from(height);
        let budget = |base: u64| Duration::from_secs(base.max(pixels / 20_000_000));
        let base = if large_raster(width, height) {
            900
        } else {
            120
        };
        decoded = media::frame_info_with_budget(path, "v:0", Some("16"), budget(base), control)?;
        decoded["frames"]
            .as_array()
            .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing video frames"))?
    };
    if frames.is_empty() || frames.len() > 180_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Reference sources require 1-180000 video frames",
        ));
    }
    for (i, frame) in frames.iter().enumerate() {
        clock::timestamp(frame, i, rate, video)?;
    }
    let packets_agree = timing == Timing::Decoded
        && inspection.video.len() == frames.len()
        && (inspection.video.iter().enumerate())
            .all(|(i, packet)| clock::timestamp(packet, i, rate, video).is_ok());
    let mut samples = 0u64;
    for frame in &inspection.audio {
        let expected = (samples as u128 * 1_000_000 / 48_000) as i64;
        if (micros(&frame["best_effort_timestamp_time"])? - expected).abs() > 1000 {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                "Audio timestamps must be continuous and aligned (1 ms container tolerance)",
            ));
        }
        samples = samples
            .checked_add(
                frame["nb_samples"]
                    .as_u64()
                    .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing audio sample count"))?,
            )
            .ok_or_else(|| error("LIMIT_EXCEEDED", "Sample count overflow"))?;
    }
    let expected_samples = Time::new(frames.len() as u64, 1)?
        .times(Time::new(rate.den, rate.num)?)?
        .units(Time::new(48000, 1)?)
        .map_err(|_| {
            error(
                "UNSUPPORTED_MEDIA",
                "Native source video duration must land on a whole audio sample",
            )
        })?;
    if samples != expected_samples {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            "Audio and video duration must match exactly",
        ));
    }
    if media::file_hash_controlled(path, control)? != before {
        return Err(error("MEDIA_CHANGED", "Source changed during inspection"));
    }
    let verified = crate::inspection_cache::Verified {
        frames: frames.len() as u64,
        samples,
        pix_fmt: video["pix_fmt"].as_str().unwrap_or_default().to_owned(),
    };
    if let Some(key) = &key {
        crate::inspection_cache::put(key, &verified);
    }
    for (alpha, timing) in implied(allow_alpha, timing, &verified.pix_fmt, packets_agree) {
        if let Some(key) = key_of(alpha, timing) {
            crate::inspection_cache::put(&key, &verified);
        }
    }
    let source = Source {
        path: path.into(),
        sha256: before,
        frames: verified.frames,
        samples,
    };
    Ok((source, verified.pix_fmt))
}
/// The other inspections that a pass with `alpha` and `timing` proves for the same bytes, as
/// (alpha, timing). Accepting `bgra` only widens the accepted pixel formats, so a `bgr0` pass
/// holds with or without it. A strict pass whose packets carry the same frame times
/// (`packets_agree`) proves the packet-timed inspection, which reads its audio from the same
/// packets; a packet-timed pass never proves a strict one.
fn implied(alpha: bool, timing: Timing, pix_fmt: &str, packets_agree: bool) -> Vec<(bool, Timing)> {
    let alphas = if pix_fmt == "bgr0" {
        vec![false, true]
    } else {
        vec![alpha]
    };
    let timings = if timing == Timing::Decoded && packets_agree {
        vec![Timing::Decoded, Timing::Packets]
    } else {
        vec![timing]
    };
    alphas
        .iter()
        .flat_map(|&a| timings.iter().map(move |&t| (a, t)))
        .filter(|&other| other != (alpha, timing))
        .collect()
}

pub(crate) fn destination(output: &Path, root: &Path) -> Result<PathBuf> {
    destination_extension(output, root, "mkv")
}

/// The resolved, existing folder an output goes in. Outputs never create folders, so a missing
/// one is named, with the root to create it in, rather than left to the bare OS error.
pub(crate) fn output_folder(output: &Path, root: &Path) -> Result<PathBuf> {
    let parent = output
        .parent()
        .ok_or_else(|| error("INVALID_PATH", "Missing output parent"))?;
    parent.canonicalize().map_err(|e| {
        let problem = if e.kind() == std::io::ErrorKind::NotFound {
            format!(
                "does not exist ({e}); outputs go only into existing folders, so create it inside output_root {} first",
                root.display()
            )
        } else {
            format!("cannot be resolved ({e})")
        };
        error(
            "IO_ERROR",
            format!(
                "Output folder {} for {} {problem}",
                parent.display(),
                output.display()
            ),
        )
    })
}

/// `root` resolved, naming it when it cannot be.
pub(crate) fn output_root(root: &Path) -> Result<PathBuf> {
    root.canonicalize().map_err(|e| {
        error(
            "IO_ERROR",
            format!("output_root {} cannot be resolved ({e})", root.display()),
        )
    })
}

pub(crate) fn destination_extension(
    output: &Path,
    root: &Path,
    extension: &str,
) -> Result<PathBuf> {
    if !output.is_absolute() || !root.is_absolute() {
        return Err(error("INVALID_PATH", "Output and root must be absolute"));
    }
    let parent = output_folder(output, root)?;
    if !parent.starts_with(output_root(root)?) {
        return Err(error(
            "PATH_OUTSIDE_ROOT",
            "Output must be inside the allowed output root",
        ));
    }
    let output = parent.join(
        output
            .file_name()
            .ok_or_else(|| error("INVALID_PATH", "Missing filename"))?,
    );
    if output.try_exists()? {
        return Err(error(
            "OUTPUT_EXISTS",
            "Existing outputs are never overwritten",
        ));
    }
    if output.extension().and_then(|s| s.to_str()) != Some(extension) {
        return Err(error(
            "UNSUPPORTED_OUTPUT",
            format!("Output must have .{extension} extension"),
        ));
    }
    Ok(output)
}

pub fn plan(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
) -> Result<Plan> {
    plan_controlled(
        project,
        input_root,
        output_root,
        output,
        &media::Uncontrolled,
    )
}

fn plan_controlled(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    control: &dyn media::Control,
) -> Result<Plan> {
    control.phase("inspecting")?;
    project.validate()?;
    media::input_root(input_root)?;
    if let Some(plan) = chunked(
        project,
        output_root,
        output,
        "mkv",
        Time::ZERO,
        project.duration()?,
        |start, duration, chunk| {
            plan_range(project, input_root, output_root, chunk, start, duration)
        },
    )? {
        return Ok(plan);
    }
    if project.tracks.is_some() {
        return crate::track_render::plan(project, input_root, output_root, output, control);
    }
    let rate = clock::rate(project.frame_rate)?;
    if project.clips.is_empty() || project.clips.len() > 64 {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Reference renderer requires 1-64 sequential clips",
        ));
    }
    let frames = project.duration()?.units(rate)?;
    if frames == 0 || frames > 180000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Native sequential renders require 1..180000 frames",
        ));
    }
    let samples = project.duration()?.units(Time::new(48000, 1)?)?;
    for clip in &project.clips {
        clip.source_in.units(Time::new(48000, 1)?)?;
        clip.duration.units(Time::new(48000, 1)?)?;
    }
    sequential_pictures(project)?;
    let output = destination(output, output_root)?;
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(project.width, project.height);
    let large = large_raster(project.width, project.height);
    let ffv1_slices = if large { "16" } else { ffv1_slices };
    // Inspect the clips' sources together first; the loop below then finds each one verified.
    let mut wanted: Vec<(PathBuf, Check)> = Vec::new();
    for clip in &project.clips {
        if let Some(asset) = (project.assets.iter()).find(|a| Some(&a.id) == clip.asset_id.as_ref())
            && let Ok(path) = media::project_file(Path::new(&asset.path), input_root)
            && !wanted.iter().any(|(p, _)| *p == path)
        {
            let (width, height) = (project.width, project.height);
            let (alpha, timing) = (false, Timing::Decoded);
            wanted.push((
                path,
                Check {
                    width,
                    height,
                    rate,
                    alpha,
                    timing,
                },
            ));
        }
    }
    if wanted.len() > 1 && control.shared().is_some() {
        inspect_many(&wanted, control);
    }
    let mut sources = HashMap::new();
    let mut args: Vec<String> = ["-hide_banner", "-v", "error", "-nostdin", "-n"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut filters = Vec::new();
    let mut concat = String::new();
    let mut input = 0;
    for (i, clip) in project.clips.iter().enumerate() {
        control.check()?;
        concat.push_str(&format!("[v{i}][a{i}]"));
        if clip.gap {
            let frames = clip.duration.units(project.frame_rate)?;
            if frames > 180_000 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "A rendered gap supports at most 180000 frames",
                ));
            }
            filters.push(format!("color=c=black:s={}x{}:r={}/{},format=pix_fmts=rgb24,trim=end_frame={frames},setpts=PTS-STARTPTS[v{i}]", project.width, project.height,rate.num,rate.den));
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=end_sample={},asetpts=PTS-STARTPTS[a{i}]",
                clip.duration.units(Time::new(48000, 1)?)?
            ));
            continue;
        }
        let asset = project
            .assets
            .iter()
            .find(|a| Some(&a.id) == clip.asset_id.as_ref())
            .expect("validated asset");
        if !sources.contains_key(&asset.id) {
            let path = media::project_file(Path::new(&asset.path), input_root)?;
            sources.insert(
                asset.id.clone(),
                inspect_reference_at(&path, project.width, project.height, rate, control)?,
            );
            crate::registry::verify_source(asset, &sources[&asset.id])?;
        }
        let source = &sources[&asset.id];
        let start = clip.source_in.units(project.frame_rate)?;
        let end = clip
            .source_in
            .plus(clip.duration)?
            .units(project.frame_rate)?;
        if end > source.frames {
            return Err(error(
                "INVALID_RANGE",
                format!("Clip {} exceeds decoded media length", clip.id),
            ));
        }
        if large {
            args.extend(["-threads".into(), "4".into()]);
        }
        args.extend([
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-i".into(),
            source.path.to_string_lossy().into_owned(),
        ]);
        filters.push(format!(
            "[{input}:v:0]trim=start_frame={start}:end_frame={end},settb=expr={}/{},setpts=N[v{i}]",
            rate.den, rate.num
        ));
        filters.push(format!(
            "[{input}:a:0]atrim=start_sample={}:end_sample={},asetpts=PTS-STARTPTS[a{i}]",
            clip.source_in.units(Time::new(48000, 1)?)?,
            clip.source_in
                .plus(clip.duration)?
                .units(Time::new(48000, 1)?)?
        ));
        input += 1;
    }
    filters.push(format!(
        "{concat}concat=n={}:v=1:a=1[vout][aout]",
        project.clips.len()
    ));
    args.extend([
        "-filter_complex_threads".into(),
        "1".into(),
        "-filter_complex".into(),
        filters.join(";"),
    ]);
    args.extend(
        [
            "-map",
            "[vout]",
            "-map",
            "[aout]",
            "-c:v",
            "ffv1",
            "-level",
            ffv1_level,
            "-slices",
            ffv1_slices,
            "-pix_fmt",
            "bgr0",
            "-threads",
            if large { "16" } else { "1" },
            "-c:a",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-f",
            "matroska",
        ]
        .iter()
        .map(|s| s.to_string()),
    );
    args.extend([
        "-r".into(),
        format!("{}/{}", rate.num, rate.den),
        "-fps_mode".into(),
        "cfr".into(),
        "-enc_time_base:v".into(),
        format!("{}/{}", rate.den, rate.num),
    ]);
    args.push(output.to_string_lossy().into_owned());
    if args.iter().map(|s| s.len() + 3).sum::<usize>() > 24_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Render arguments exceed supported size",
        ));
    }
    let mut sources: Vec<_> = sources.into_values().collect();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Plan {
        profile: if rate == (Time { num: 25, den: 1 }) {
            "reference-ffv1-pcm-v1"
        } else {
            "reference-ffv1-pcm-rational-v2"
        },
        source_quality: "original",
        project_revision: project.revision,
        frames,
        samples,
        output,
        sources,
        arguments: args,
        chunks: Vec::new(),
        generated: Vec::new(),
        compositor: None,
    })
}

/// Sequential clips play their asset's picture and sound together, so audio-only assets are
/// refused before any source is inspected.
fn sequential_pictures(project: &Project) -> Result<()> {
    for clip in &project.clips {
        if let Some(id) = &clip.asset_id
            && project.asset(id)?.audio_only()
        {
            return Err(crate::model::no_picture(
                id,
                &format!("sequential clip {:?}", clip.id),
            ));
        }
    }
    Ok(())
}

pub(crate) struct TempFile(pub(crate) PathBuf);
impl Drop for TempFile {
    fn drop(&mut self) {
        // Windows job termination can close inherited pipes before the encoder's
        // file handles finish closing. Retry only sharing/lock violations on this
        // exact owned temporary path; unrelated failures are never swept away.
        let started = Instant::now();
        loop {
            match fs::remove_file(&self.0) {
                Err(e)
                    if cfg!(windows)
                        && matches!(e.raw_os_error(), Some(32 | 33))
                        && started.elapsed() < Duration::from_secs(5) =>
                {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => break,
            }
        }
    }
}

pub fn run(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
) -> Result<Value> {
    run_controlled(
        project,
        input_root,
        output_root,
        output,
        None,
        &media::Uncontrolled,
    )
}

pub(crate) fn run_controlled(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    temp_path: Option<&Path>,
    control: &dyn media::Control,
) -> Result<Value> {
    let plan = plan_controlled(project, input_root, output_root, output, control)?;
    execute(project, plan, temp_path, Timing::Decoded, control)
}

pub(crate) fn plan_range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Plan> {
    project.validate()?;
    start.units(project.frame_rate)?;
    if duration.units(project.frame_rate)? == 0
        || start.plus(duration)?.compare(project.duration()?)?.is_gt()
    {
        return Err(error(
            "INVALID_RANGE",
            "Render range must be nonempty and inside the timeline",
        ));
    }
    if let Some(plan) = chunked(
        project,
        output_root,
        output,
        "mkv",
        start,
        duration,
        |start, duration, chunk| {
            plan_range(project, input_root, output_root, chunk, start, duration)
        },
    )? {
        return Ok(plan);
    }
    if project.tracks.is_some() {
        crate::track_render::plan_window(
            project,
            input_root,
            output_root,
            output,
            start,
            duration,
            &media::Uncontrolled,
        )
    } else {
        let selected = crate::tracks::range(project, start, duration)?;
        plan(&selected, input_root, output_root, output)
    }
}
/// Plan a lossless stereo PCM WAV of a timeline range without compositing or encoding video.
pub(crate) fn plan_audio_range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Plan> {
    project.validate()?;
    media::input_root(input_root)?;
    start.units(project.frame_rate)?;
    if duration.units(project.frame_rate)? == 0
        || start.plus(duration)?.compare(project.duration()?)?.is_gt()
    {
        return Err(error(
            "INVALID_RANGE",
            "Render range must be nonempty and inside the timeline",
        ));
    }
    if let Some(plan) = chunked(
        project,
        output_root,
        output,
        "wav",
        start,
        duration,
        |start, duration, chunk| {
            plan_audio_range(project, input_root, output_root, chunk, start, duration)
        },
    )? {
        return Ok(plan);
    }
    if project.tracks.is_some() {
        return crate::track_render::plan_audio_window(
            project,
            input_root,
            output_root,
            output,
            start,
            duration,
        );
    }
    let project = crate::tracks::range(project, start, duration)?;
    let rate = clock::rate(project.frame_rate)?;
    let samples_clock = Time::new(48000, 1)?;
    if project.clips.is_empty() || project.clips.len() > 64 {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Reference renderer requires 1-64 sequential clips",
        ));
    }
    let frames = project.duration()?.units(rate)?;
    if frames == 0 || frames > 180000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Native sequential renders require 1..180000 frames",
        ));
    }
    let samples = project.duration()?.units(samples_clock)?;
    sequential_pictures(&project)?;
    let output = destination_extension(output, output_root, "wav")?;
    let mut sources = HashMap::new();
    let mut args: Vec<String> = ["-hide_banner", "-v", "error", "-nostdin", "-n"]
        .map(str::to_owned)
        .to_vec();
    let mut filters = Vec::new();
    let mut concat = String::new();
    let mut input = 0;
    for (i, clip) in project.clips.iter().enumerate() {
        let first = clip.source_in.units(samples_clock)?;
        let count = clip.duration.units(samples_clock)?;
        concat.push_str(&format!("[a{i}]"));
        if clip.gap {
            filters.push(format!(
                "anullsrc=r=48000:cl=stereo,atrim=end_sample={count},asetpts=PTS-STARTPTS[a{i}]"
            ));
            continue;
        }
        let asset = project
            .assets
            .iter()
            .find(|a| Some(&a.id) == clip.asset_id.as_ref())
            .expect("validated asset");
        if !sources.contains_key(&asset.id) {
            // Validate each source exactly as a full reference render would.
            let path = media::project_file(Path::new(&asset.path), input_root)?;
            sources.insert(
                asset.id.clone(),
                inspect_reference_audio(
                    &path,
                    project.width,
                    project.height,
                    rate,
                    &media::Uncontrolled,
                )?,
            );
            crate::registry::verify_source(asset, &sources[&asset.id])?;
        }
        let source = &sources[&asset.id];
        if clip
            .source_in
            .plus(clip.duration)?
            .units(project.frame_rate)?
            > source.frames
        {
            return Err(error(
                "INVALID_RANGE",
                format!("Clip {} exceeds decoded media length", clip.id),
            ));
        }
        args.extend([
            "-protocol_whitelist".into(),
            "file,pipe".into(),
            "-i".into(),
            source.path.to_string_lossy().into_owned(),
        ]);
        filters.push(format!(
            "[{input}:a:0]atrim=start_sample={first}:end_sample={},asetpts=PTS-STARTPTS[a{i}]",
            first + count
        ));
        input += 1;
    }
    filters.push(format!(
        "{concat}concat=n={}:v=0:a=1[aout]",
        project.clips.len()
    ));
    args.extend([
        "-filter_complex_threads".into(),
        "1".into(),
        "-filter_complex".into(),
        filters.join(";"),
    ]);
    args.extend(
        [
            "-map",
            "[aout]",
            "-c:a",
            "pcm_s16le",
            "-ar",
            "48000",
            "-ac",
            "2",
            "-f",
            "wav",
        ]
        .map(str::to_owned),
    );
    args.push(output.to_string_lossy().into_owned());
    let mut sources: Vec<_> = sources.into_values().collect();
    sources.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Plan {
        profile: "reference-pcm-audio-v1",
        source_quality: "original",
        project_revision: project.revision,
        frames,
        samples,
        output,
        sources,
        arguments: args,
        chunks: Vec::new(),
        generated: Vec::new(),
        compositor: None,
    })
}

/// Render the planned audio-only intermediate; verify its exact stereo sample count and sources.
pub(crate) fn run_audio_range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Value> {
    let plan = plan_audio_range(project, input_root, output_root, output, start, duration)?;
    let ffmpeg_version = media::version("ffmpeg")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock is before epoch"))?
        .as_nanos();
    let temp = TempFile(plan.output.with_file_name(format!(
        ".cutbolt-{}-{nonce}.partial.wav",
        std::process::id()
    )));
    run_plan(
        &plan,
        &temp.0,
        Duration::from_secs(600),
        false,
        &media::Uncontrolled,
    )?;
    let rendered = crate::pcm_stream::inspect(&temp.0, &media::Uncontrolled)?;
    if rendered.frames != plan.samples {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Rendered sample count does not match the timeline",
        ));
    }
    for source in &plan.sources {
        if media::file_hash(&source.path)? != source.sha256 {
            return Err(error("MEDIA_CHANGED", "Source changed during rendering"));
        }
    }
    media::publish(&temp.0, &plan.output)?;
    let mut receipt = json!({"output":plan.output,"profile":plan.profile,"source_quality":plan.source_quality,"frames":plan.frames,
        "samples":plan.samples,"project_revision":plan.project_revision,"sha256":rendered.sha256,"pcm_sha256":rendered.pcm_sha256,
        "sources":plan.sources,"ffmpeg":ffmpeg_version});
    if let Some(dynamics) = plan.dynamics() {
        receipt["dynamics"] = dynamics;
    }
    Ok(receipt)
}

pub(crate) fn run_range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Value> {
    let plan = plan_range(project, input_root, output_root, output, start, duration)?;
    execute(project, plan, None, Timing::Decoded, &media::Uncontrolled)
}
/// `run_range` for an export's lossless intermediate. Its output is checked from packets rather
/// than by decoding every picture: the export decodes it in full anyway, and then decodes and
/// verifies the delivered file frame by frame.
pub(crate) fn run_intermediate_range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Value> {
    let plan = plan_range(project, input_root, output_root, output, start, duration)?;
    execute(project, plan, None, Timing::Packets, &media::Uncontrolled)
}
fn execute(
    project: &Project,
    plan: Plan,
    temp_path: Option<&Path>,
    timing: Timing,
    control: &dyn media::Control,
) -> Result<Value> {
    control.sources(&plan.sources)?;
    let ffmpeg_version = media::version_controlled("ffmpeg", control)?;
    let ffprobe_version = media::version_controlled("ffprobe", control)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock is before epoch"))?
        .as_nanos();
    let temp = plan.output.with_file_name(format!(
        ".cutbolt-{}-{nonce}.partial.mkv",
        std::process::id()
    ));
    let temp = temp_path.map(Path::to_path_buf).unwrap_or(temp);
    if temp.try_exists()? {
        return Err(error("OUTPUT_EXISTS", "Temporary output collision"));
    }
    let temp = TempFile(temp);
    control.phase("rendering")?;
    control.total(plan.frames);
    run_plan(
        &plan,
        &temp.0,
        Duration::from_secs(
            if project.tracks.is_none() && large_raster(project.width, project.height) {
                1800
            } else {
                600
            },
        ),
        true,
        control,
    )?;
    control.frames(plan.frames)?;
    control.phase("verifying")?;
    let rendered = inspect_source_at(
        &temp.0,
        project.width,
        project.height,
        project.frame_rate,
        control,
        false,
        timing,
    )?;
    if rendered.frames != plan.frames || rendered.samples != plan.samples {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Rendered frame/sample count does not match the timeline",
        ));
    }
    for source in &plan.sources {
        if media::file_hash_controlled(&source.path, control)? != source.sha256 {
            return Err(error("MEDIA_CHANGED", "Source changed during rendering"));
        }
    }
    let mut receipt = json!({"output":plan.output,"profile":plan.profile,"source_quality":plan.source_quality,"frames":plan.frames,"samples":plan.samples,
        "project_revision":plan.project_revision,"frame_rate":project.frame_rate,"sha256":rendered.sha256,"sources":plan.sources,
        "ffmpeg":ffmpeg_version,"ffprobe":ffprobe_version});
    if let Some(dynamics) = plan.dynamics() {
        receipt["dynamics"] = dynamics;
    }
    control.publish(&temp.0, &plan.output, &receipt)?;
    Ok(receipt)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn cancelled_partial_waits_for_real_windows_file_handle_release() {
        let root = std::env::temp_dir().join(format!(
            "cutbolt-sharing-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let partial = root.join("owned.partial.mkv");
        let sentinel = root.join("unrelated.mkv");
        fs::write(&partial, b"encoded partial").unwrap();
        fs::write(&sentinel, b"preserve this file").unwrap();
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(1 | 2) // Read/write sharing, deliberately no delete sharing.
            .open(&partial)
            .unwrap();
        assert_eq!(
            fs::remove_file(&partial).unwrap_err().raw_os_error(),
            Some(32)
        );
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(120));
            drop(held);
        });
        drop(TempFile(partial.clone()));
        assert!(!partial.exists());
        release.join().unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve this file");
        fs::remove_file(sentinel).unwrap();
        fs::remove_dir(root).unwrap();
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn a_missing_output_folder_is_named_with_its_root() {
        let scratch = crate::tracking::Scratch::new("missing-output-folder");
        let root = &scratch.0;
        let folder = root.join("renders");
        let output = folder.join("final.mkv");
        let error = destination(&output, root).unwrap_err();
        assert_eq!(error.code, "IO_ERROR");
        assert!(
            error.message.starts_with(&format!(
                "Output folder {} for {} does not exist (",
                folder.display(),
                output.display()
            )),
            "{}",
            error.message
        );
        assert!(
            error.message.ends_with(&format!(
                "); outputs go only into existing folders, so create it inside output_root {} first",
                root.display()
            )),
            "{}",
            error.message
        );
        // Rendering never creates folders.
        assert!(!folder.exists());
        fs::create_dir(&folder).unwrap();
        assert_eq!(
            destination(&output, root).unwrap(),
            folder.canonicalize().unwrap().join("final.mkv")
        );
    }
}

#[cfg(test)]
mod inspection_tests {
    use super::{Timing::*, implied};

    #[test]
    fn passes_record_only_the_inspections_they_prove() {
        // An opaque strict pass holds with or without alpha acceptance and, when its packets
        // agree, with packet timing.
        let opaque = implied(false, Decoded, "bgr0", true);
        assert_eq!(opaque, [(false, Packets), (true, Decoded), (true, Packets)]);
        assert_eq!(implied(false, Decoded, "bgr0", false), [(true, Decoded)]);
        let overlay = implied(true, Decoded, "bgr0", true);
        assert_eq!(
            overlay,
            [(false, Decoded), (false, Packets), (true, Packets)]
        );
        // Straight alpha never satisfies an opaque-only inspection.
        assert_eq!(implied(true, Decoded, "bgra", true), [(true, Packets)]);
        assert!(implied(true, Decoded, "bgra", false).is_empty());
        // Packet timing never proves the strict decode.
        assert_eq!(implied(false, Packets, "bgr0", false), [(true, Packets)]);
        assert!(implied(true, Packets, "bgra", false).is_empty());
    }
}
