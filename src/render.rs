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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Timing {
    Decoded,
    Packets,
}
/// Reference source for an alpha_over track: FFV1 with straight alpha (bgra) or opaque bgr0.
/// Returns whether the source carries an alpha plane.
pub(crate) fn inspect_overlay(
    path: &Path,
    width: u32,
    height: u32,
    control: &dyn media::Control,
) -> Result<(Source, bool)> {
    let source = inspect_source_at(
        path,
        width,
        height,
        Time::new(25, 1)?,
        control,
        true,
        Timing::Decoded,
    )?;
    let metadata = media::probe_controlled(path, control)?;
    let alpha = metadata["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .is_some_and(|v| v["pix_fmt"] == "bgra");
    Ok((source, alpha))
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
    let rate = clock::rate(rate)?;
    let before = media::file_hash_controlled(path, control)?;
    let metadata = media::probe_controlled(path, control)?;
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
        || !clock::rate_hint(video, &metadata, rate)
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
    let decoded = if timing == Timing::Packets {
        media::packet_times_controlled(path, "v:0", control)?
    } else if large_raster(width, height) {
        media::frame_info_with_budget(path, "v:0", Some("16"), Duration::from_secs(900), control)?
    } else {
        media::frame_info_controlled(path, "v:0", control)?
    };
    let frames = decoded["frames"]
        .as_array()
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing video frames"))?;
    if frames.is_empty() || frames.len() > 180_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Reference sources require 1-180000 video frames",
        ));
    }
    for (i, frame) in frames.iter().enumerate() {
        clock::timestamp(frame, i, rate, video)?;
    }
    let decoded_audio = media::frame_info_controlled(path, "a:0", control)?;
    let audio_frames = decoded_audio["frames"]
        .as_array()
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Missing audio frames"))?;
    let mut samples = 0u64;
    for frame in audio_frames {
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
    Ok(Source {
        path: path.into(),
        sha256: before,
        frames: frames.len() as u64,
        samples,
    })
}

pub(crate) fn destination(output: &Path, root: &Path) -> Result<PathBuf> {
    destination_extension(output, root, "mkv")
}

pub(crate) fn destination_extension(
    output: &Path,
    root: &Path,
    extension: &str,
) -> Result<PathBuf> {
    if !output.is_absolute() || !root.is_absolute() {
        return Err(error("INVALID_PATH", "Output and root must be absolute"));
    }
    let parent = output
        .parent()
        .ok_or_else(|| error("INVALID_PATH", "Missing output parent"))?
        .canonicalize()?;
    if !parent.starts_with(root.canonicalize()?) {
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
    let output = destination(output, output_root)?;
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(project.width, project.height);
    let large = large_raster(project.width, project.height);
    let ffv1_slices = if large { "16" } else { ffv1_slices };
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
    })
}

struct TempFile(PathBuf);
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
    execute(project, plan, temp_path, control)
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
    let mut plan = plan_audio_range(project, input_root, output_root, output, start, duration)?;
    let ffmpeg_version = media::version("ffmpeg")?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock is before epoch"))?
        .as_nanos();
    let temp = TempFile(plan.output.with_file_name(format!(
        ".cutbolt-{}-{nonce}.partial.wav",
        std::process::id()
    )));
    *plan.arguments.last_mut().expect("output argument") = temp.0.to_string_lossy().into_owned();
    media::capture(
        &media::tool("ffmpeg"),
        &plan.arguments,
        Duration::from_secs(600),
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
    Ok(
        json!({"output":plan.output,"profile":plan.profile,"source_quality":plan.source_quality,"frames":plan.frames,
        "samples":plan.samples,"project_revision":plan.project_revision,"sha256":rendered.sha256,"pcm_sha256":rendered.pcm_sha256,
        "sources":plan.sources,"ffmpeg":ffmpeg_version}),
    )
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
    execute(project, plan, None, &media::Uncontrolled)
}
fn execute(
    project: &Project,
    mut plan: Plan,
    temp_path: Option<&Path>,
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
    *plan.arguments.last_mut().expect("output argument") = temp.0.to_string_lossy().into_owned();
    control.phase("rendering")?;
    plan.arguments.splice(
        0..0,
        ["-progress".into(), "pipe:1".into(), "-nostats".into()],
    );
    media::capture_controlled(
        &control.tool("ffmpeg"),
        &plan.arguments,
        Duration::from_secs(
            if project.tracks.is_none() && large_raster(project.width, project.height) {
                1800
            } else {
                600
            },
        ),
        control,
    )?;
    control.frames(plan.frames)?;
    control.phase("verifying")?;
    let rendered = inspect_reference_at(
        &temp.0,
        project.width,
        project.height,
        project.frame_rate,
        control,
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
    let receipt = json!({"output":plan.output,"profile":plan.profile,"source_quality":plan.source_quality,"frames":plan.frames,"samples":plan.samples,
        "project_revision":plan.project_revision,"frame_rate":project.frame_rate,"sha256":rendered.sha256,"sources":plan.sources,
        "ffmpeg":ffmpeg_version,"ffprobe":ffprobe_version});
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
