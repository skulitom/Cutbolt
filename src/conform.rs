//! Explicit timestamp sampling into a lossless editing asset; originals stay external.
use crate::{
    Result, color, error, media, remap, render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    cmp::Ordering,
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FPS: Time = Time { num: 25, den: 1 };
const VIDEO_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const AUDIO_BYTES: u64 = 128 * 1024 * 1024;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    EncodedRgb,
    Bt709Limited,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Audio {
    Resample,
    Mute,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub file: Identity,
    pub color: Option<Color>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdr: Option<color::Input>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub id: String,
    pub source: Source,
    pub source_in: Time,
    pub duration: Time,
    pub rate: Time,
    pub reverse: bool,
    pub freeze: bool,
    pub width: u32,
    pub height: u32,
    pub audio: Audio,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remap: Option<remap::Remap>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_transfer: Option<color::Transfer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<crate::lut::Transform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decode: Option<crate::acceleration::Decode>,
}
struct Checked {
    path: PathBuf,
    width: u32,
    height: u32,
    pts: Vec<Time>,
    video_end: Time,
    audio_rate: u64,
    channels: u64,
    samples: u64,
    metadata: Value,
    layout: Option<color::Layout>,
    normalization: Value,
}
fn unsupported(message: &str) -> crate::Error {
    error("UNSUPPORTED_MEDIA", message)
}
fn number(value: &Value) -> Result<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .ok_or_else(|| unsupported("Missing or negative media integer"))
}
fn ratio(value: &Value) -> Result<Time> {
    let (n, d) = value
        .as_str()
        .and_then(|s| s.split_once('/'))
        .ok_or_else(|| unsupported("Missing rational media time base"))?;
    Time::new(
        n.parse()
            .map_err(|_| unsupported("Invalid time numerator"))?,
        d.parse()
            .map_err(|_| unsupported("Invalid time denominator"))?,
    )
}
fn frames(path: &Path, selector: &str) -> Result<Vec<Value>> {
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-select_streams",
        selector,
        "-show_frames",
        "-show_entries",
        "frame=best_effort_timestamp,duration,pkt_duration,nb_samples,width,height,interlaced_frame,color_space,color_range,color_primaries,color_transfer,chroma_location",
        "-of",
        "json",
    ];
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    args.push(path.to_string_lossy().into_owned());
    let value: Value = serde_json::from_slice(&media::capture(
        &media::tool("ffprobe"),
        &args,
        Duration::from_secs(120),
    )?)?;
    value["frames"]
        .as_array()
        .cloned()
        .ok_or_else(|| unsupported("Decoded frame list required"))
}
fn input_args(path: &Path) -> Vec<String> {
    [
        "-hide_banner",
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-xerror",
        "-err_detect",
        "explode",
        "-protocol_whitelist",
        "file,pipe",
        "-noautorotate",
        "-i",
    ]
    .iter()
    .map(|s| s.to_string())
    .chain(std::iter::once(path.to_string_lossy().into_owned()))
    .collect()
}
fn check(source: &Source, root: &Path) -> Result<Checked> {
    if source.color.is_some() && source.sdr.is_some() {
        return Err(error(
            "INVALID_COLOR",
            "Use either legacy color or explicit sdr, never both",
        ));
    }
    let (path, bytes) = scene::identity_bytes(&source.file, root)?;
    drop(bytes);
    let metadata = media::probe(&path)?;
    let streams = metadata["streams"]
        .as_array()
        .ok_or_else(|| unsupported("Streams required"))?;
    let video: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect();
    let audio: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    if video.len() > 1
        || audio.len() > 1
        || streams.len() != video.len() + audio.len()
        || streams.is_empty()
    {
        return Err(unsupported(
            "One video and/or one audio stream required; other streams are unsupported",
        ));
    }
    let format = metadata["format"]["format_name"].as_str().unwrap_or("");
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mkv = extension == "mkv" && format.split(',').any(|s| s == "matroska");
    let mp4 = ["mp4", "mov"].contains(&extension.as_str()) && format.split(',').any(|s| s == "mov");
    let wav = extension == "wav" && format == "wav";
    if (!mkv && !mp4 && !wav) || (wav && !video.is_empty()) || (!wav && video.is_empty()) {
        return Err(unsupported(
            "Supported containers: video MKV/MP4/MOV or audio WAV",
        ));
    }
    let mut checked = Checked {
        path,
        width: 0,
        height: 0,
        pts: vec![],
        video_end: Time::ZERO,
        audio_rate: 0,
        channels: 0,
        samples: 0,
        metadata: metadata.clone(),
        layout: None,
        normalization: Value::Null,
    };
    if let Some(v) = video.first() {
        let accepted = if let Some(sdr) = source.sdr {
            let (layout, report) = sdr.inspect(v, mkv, mp4)?;
            checked.layout = Some(layout);
            checked.normalization = report;
            true
        } else {
            match source.color {
                Some(Color::EncodedRgb) => {
                    mkv && v["codec_name"] == "ffv1"
                        && v["pix_fmt"] == "bgr0"
                        && matches!(
                            v["color_space"].as_str(),
                            None | Some("gbr") | Some("unknown")
                        )
                        && matches!(
                            v["color_range"].as_str(),
                            None | Some("pc") | Some("unknown")
                        )
                        && matches!(
                            v["color_primaries"].as_str(),
                            None | Some("bt709") | Some("unknown")
                        )
                        && matches!(
                            v["color_transfer"].as_str(),
                            None | Some("bt709") | Some("iec61966-2-1") | Some("unknown")
                        )
                }
                Some(Color::Bt709Limited) => {
                    mp4 && v["codec_name"] == "h264"
                        && v["pix_fmt"] == "yuv420p"
                        && v["color_space"] == "bt709"
                        && v["color_primaries"] == "bt709"
                        && v["color_transfer"] == "bt709"
                        && v["color_range"] == "tv"
                }
                _ => false,
            }
        };
        if !accepted
            || v.get("side_data_list").is_some()
            || v["tags"].get("rotate").is_some()
            || !matches!(
                v["field_order"].as_str(),
                Some("progressive") | Some("unknown") | None
            )
            || !matches!(
                v["sample_aspect_ratio"].as_str(),
                Some("1:1") | Some("N/A") | None
            )
        {
            return Err(unsupported(
                "Video requires the declared RGB or tagged limited BT.709 profile, progressive square pixels and no transform/HDR side data",
            ));
        }
        checked.width =
            u32::try_from(number(&v["width"])?).map_err(|_| unsupported("Video width overflow"))?;
        checked.height = u32::try_from(number(&v["height"])?)
            .map_err(|_| unsupported("Video height overflow"))?;
        if checked.width == 0
            || checked.height == 0
            || checked.width > 4096
            || checked.height > 2160
            || checked.width as u64 * checked.height as u64 > 8_000_000
        {
            return Err(unsupported("Video dimensions exceed 4096x2160/8M pixels"));
        }
        let tb = ratio(&v["time_base"])?;
        let decoded = frames(&checked.path, "v:0")?;
        if decoded.is_empty()
            || decoded.len() > 36000
            || decoded.len() as u64
                * checked.layout.map_or(
                    checked.width as usize * checked.height as usize * 3,
                    |layout| layout.bytes(checked.width, checked.height),
                ) as u64
                > VIDEO_BYTES
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Decoded video exceeds 36000 frames or 4 GiB native samples",
            ));
        }
        for (i, f) in decoded.iter().enumerate() {
            if let Some(sdr) = source.sdr {
                let mut profile = (*v).clone();
                for key in [
                    "color_space",
                    "color_range",
                    "color_primaries",
                    "color_transfer",
                    "chroma_location",
                ] {
                    if let Some(value) = f.get(key) {
                        profile[key] = value.clone();
                    }
                }
                sdr.inspect(&profile, mkv, mp4)?;
            }
            let t = Time::new(number(&f["best_effort_timestamp"])?, 1)?.times(tb)?;
            if (i == 0 && t.num != 0)
                || (i > 0 && t.compare(checked.pts[i - 1])? != Ordering::Greater)
                || f["width"] != checked.width
                || f["height"] != checked.height
                || f["interlaced_frame"] != 0
            {
                return Err(unsupported(
                    "Video requires zero-origin increasing timestamps and fixed progressive dimensions",
                ));
            }
            checked.pts.push(t);
        }
        let last = decoded.last().expect("nonempty");
        let duration = number(
            last.get("duration")
                .or_else(|| last.get("pkt_duration"))
                .unwrap_or(&Value::Null),
        )?;
        if duration == 0 {
            return Err(unsupported("Positive final frame duration required"));
        }
        checked.video_end = checked
            .pts
            .last()
            .expect("nonempty")
            .plus(Time::new(duration, 1)?.times(tb)?)?;
    } else if source.color.is_some() || source.sdr.is_some() {
        return Err(unsupported(
            "Audio-only sources have no video color interpretation",
        ));
    }
    if let Some(a) = audio.first() {
        let valid =
            (mp4 && a["codec_name"] == "aac") || ((mkv || wav) && a["codec_name"] == "pcm_s16le");
        checked.audio_rate = number(&a["sample_rate"])?;
        checked.channels = number(&a["channels"])?;
        if !valid
            || ![24000, 44100, 48000].contains(&checked.audio_rate)
            || ![1, 2].contains(&checked.channels)
            || !match a["channel_layout"].as_str() {
                None | Some("unknown") => true,
                Some("mono") => checked.channels == 1,
                Some("stereo") => checked.channels == 2,
                _ => false,
            }
        {
            return Err(unsupported(
                "Audio requires PCM16 or MP4/MOV AAC, mono/stereo, 24/44.1/48 kHz",
            ));
        }
        let tb = ratio(&a["time_base"])?;
        let decoded = frames(&checked.path, "a:0")?;
        for f in decoded {
            let t = Time::new(number(&f["best_effort_timestamp"])?, 1)?.times(tb)?;
            let expected = Time::new(checked.samples, checked.audio_rate)?;
            let diff = if t.compare(expected)? == Ordering::Less {
                expected.minus(t)?
            } else {
                t.minus(expected)?
            };
            if (checked.samples == 0 && t.num != 0)
                || diff.compare(Time::new(1, 1000)?)? == Ordering::Greater
            {
                return Err(unsupported(
                    "Audio must be zero-origin and continuous within container precision",
                ));
            }
            let n = number(&f["nb_samples"])?;
            checked.samples = checked
                .samples
                .checked_add(n)
                .ok_or_else(|| unsupported("Audio count overflow"))?;
            if n == 0
                || checked.samples > checked.audio_rate * 600
                || checked.samples * checked.channels * 2 > AUDIO_BYTES
            {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Decoded audio exceeds ten minutes or 128 MiB",
                ));
            }
        }
        if checked.samples == 0 {
            return Err(unsupported("Audio stream has no samples"));
        }
    }
    // Probe output alone can conceal recoverable codec errors; require a strict full decode.
    let mut args = input_args(&checked.path);
    args.extend(["-map", "0", "-f", "null", "-"].map(str::to_owned));
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    scene::identity_bytes(&source.file, root)?;
    Ok(checked)
}
#[derive(Serialize)]
struct FrameSample {
    source_time: Time,
    first: usize,
    second: usize,
    second_weight: Time,
}
struct Mapping {
    frames: Vec<FrameSample>,
    times: Vec<Time>,
    remap: Option<remap::Compiled>,
}
fn mapping(recipe: &Recipe, source: &Checked) -> Result<Mapping> {
    if recipe.source.sdr.is_some() != recipe.working_transfer.is_some() {
        return Err(error(
            "INVALID_COLOR",
            "SDR normalization requires working_transfer; legacy conversion must omit it",
        ));
    }
    if recipe.lut.is_some() && recipe.working_transfer.is_none() {
        return Err(error(
            "INVALID_LUT",
            "LUT application requires explicit SDR normalization and working_transfer",
        ));
    }
    let count = recipe.duration.units(FPS)?;
    if recipe.schema_version != 1
        || recipe.id.trim().is_empty()
        || recipe.id.len() > 128
        || count == 0
        || count > 1500
        || recipe.width == 0
        || recipe.height == 0
        || recipe.width > 4096
        || recipe.height > 2160
        || recipe.width as u64 * recipe.height as u64 > 8_000_000
        || count * recipe.width as u64 * recipe.height as u64 * 3 > VIDEO_BYTES
        || recipe.rate.compare(Time::new(1, 16)?)? == Ordering::Less
        || recipe.rate.compare(Time::new(16, 1)?)? == Ordering::Greater
    {
        return Err(error(
            "INVALID_CONFORM",
            "Conform v1 requires 1..1500 frames, bounded dimensions/4 GiB output and rate 1/16..16",
        ));
    }
    recipe.source_in.validate()?;
    if recipe.freeze
        && (recipe.reverse || recipe.rate.compare(Time::new(1, 1)?)? != Ordering::Equal)
    {
        return Err(error(
            "INVALID_CONFORM",
            "Freeze requires unit rate and reverse=false",
        ));
    }
    if (recipe.freeze || recipe.reverse) && !matches!(recipe.audio, Audio::Mute) {
        return Err(error(
            "INVALID_CONFORM",
            "Reverse and freeze require explicit muted audio",
        ));
    }
    let end = if source.pts.is_empty() {
        Time::new(source.samples, source.audio_rate)?
    } else {
        source.video_end
    };
    let remap = recipe.remap.as_ref().map(|r| {
        if recipe.rate.compare(Time::new(1, 1)?)? != Ordering::Equal
            || recipe.reverse || recipe.freeze
            || matches!(recipe.audio, Audio::Resample) != matches!(r.audio_pitch, remap::AudioPitch::FollowSpeed)
        {
            return Err(error("INVALID_REMAP", "Remapping requires unit legacy rate, reverse=false, freeze=false and matching audio/pitch policies"));
        }
        let compiled = r.compile(recipe.source_in, recipe.duration)?;
        for span in &compiled.spans {
            for t in [span.source_start, span.source_end] {
                if t.compare(end)? == Ordering::Greater {
                    return Err(error("INVALID_RANGE", "Speed map leaves the decoded source interval"));
                }
                if source.samples > 0 && matches!(recipe.audio, Audio::Resample)
                    && t.compare(Time::new(source.samples, source.audio_rate)?)? == Ordering::Greater
                {
                    return Err(error("INVALID_RANGE", "Speed map leaves the decoded audio interval"));
                }
            }
        }
        Ok(compiled)
    }).transpose()?;
    let mut mapped = Mapping {
        frames: Vec::new(),
        times: Vec::new(),
        remap,
    };
    for n in 0..count {
        let output_time = Time::new(n, 25)?;
        let offset = output_time.times(recipe.rate)?;
        let t = if let Some(remap) = &mapped.remap {
            remap.source_time(output_time)?
        } else if recipe.freeze {
            recipe.source_in
        } else if recipe.reverse {
            recipe.source_in.minus(offset)?
        } else {
            recipe.source_in.plus(offset)?
        };
        if t.compare(end)? != Ordering::Less {
            return Err(error(
                "INVALID_RANGE",
                "Mapped video time exceeds source duration",
            ));
        }
        mapped.times.push(t);
        if !source.pts.is_empty() {
            let first = source.pts.partition_point(|pts| {
                pts.compare(t).expect("validated rational") != Ordering::Greater
            }) - 1;
            let second = (first + 1).min(source.pts.len() - 1);
            let mut sample = FrameSample {
                source_time: t,
                first,
                second: first,
                second_weight: Time::ZERO,
            };
            if second != first
                && let Some(remap) = &recipe.remap
                && !matches!(remap.video_sampling, remap::Sampling::Previous)
            {
                let gap = source.pts[second].minus(source.pts[first])?;
                let weight = t
                    .minus(source.pts[first])?
                    .times(Time::new(gap.den, gap.num)?)?;
                match remap.video_sampling {
                    remap::Sampling::Previous => {}
                    remap::Sampling::Nearest => {
                        if weight.compare(Time::new(1, 2)?)? != Ordering::Less {
                            sample.first = second;
                            sample.second = second;
                        }
                    }
                    remap::Sampling::Linear if weight.num > 0 => {
                        sample.second = second;
                        sample.second_weight = weight;
                    }
                    remap::Sampling::Linear => {}
                }
            }
            mapped.frames.push(sample);
        }
    }
    if recipe.remap.is_none() && matches!(recipe.audio, Audio::Resample) && source.samples > 0 {
        recipe.source_in.units(Time::new(source.audio_rate, 1)?)?;
        if recipe
            .source_in
            .plus(recipe.duration.times(recipe.rate)?)?
            .compare(Time::new(source.samples, source.audio_rate)?)?
            == Ordering::Greater
        {
            return Err(error(
                "INVALID_RANGE",
                "Mapped audio interval exceeds decoded source",
            ));
        }
    }
    Ok(mapped)
}
fn report(recipe: &Recipe, source: &Checked, mapped: &Mapping) -> Value {
    let selected = mapped.frames.iter().map(|f| f.first).collect::<Vec<_>>();
    let mut report = json!({"profile":"media-conform-v1","source":{"path":source.path,"identity":recipe.source.file,"metadata":source.metadata,"video_frames":source.pts.len(),"video_end":source.video_end,"audio_samples":source.samples,"audio_rate":source.audio_rate,"audio_channels":source.channels},"source_frame_indices":selected,"source_frame_times":selected.iter().map(|i|source.pts[*i]).collect::<Vec<_>>(),"frames":recipe.duration.units(FPS).expect("validated"),"samples":recipe.duration.units(Time{num:48000,den:1}).expect("validated"),"width":recipe.width,"height":recipe.height,"duration":recipe.duration,"rate":recipe.rate,"reverse":recipe.reverse,"freeze":recipe.freeze,"audio":recipe.audio,"video_sampling":"latest_source_timestamp_at_or_before_output_clock","resize":"nearest_top_left","audio_sampling":"linear_pitch_changes_with_rate","color":recipe.source.color,"normalization":source.normalization,"working_transfer":recipe.working_transfer});
    if let Some(remap) = &mapped.remap {
        let request = recipe.remap.as_ref().expect("compiled remap");
        report["remap"] = json!({"segments":remap.spans,"source_times":mapped.times,"video_samples":mapped.frames,"video_sampling":request.video_sampling,"audio_pitch":request.audio_pitch,"clock":"exact_integral_of_linear_speed","interpolation_space":"working_rgb_values_after_normalization_and_lut"});
        report["video_sampling"] = json!(request.video_sampling);
        report["audio_sampling"] = json!(match request.audio_pitch {
            remap::AudioPitch::FollowSpeed => "linear_pitch_follows_instantaneous_speed",
            remap::AudioPitch::Mute => "silence",
        });
    }
    report
}
pub fn inspect(recipe: &Recipe, root: &Path) -> Result<Value> {
    let source = check(&recipe.source, root)?;
    let selected = mapping(recipe, &source)?;
    let mut result = report(recipe, &source, &selected);
    result["decode"] = crate::acceleration::select(recipe.decode, &source.metadata)?.report();
    result["lut"] = recipe
        .lut
        .as_ref()
        .map(|lut| lut.load(root).map(|v| v.report()))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(result)
}
fn decoded_hash(path: &Path, video: bool) -> Result<String> {
    let mut args = input_args(path);
    args.extend(
        if video {
            vec!["-map", "0:v:0", "-pix_fmt", "rgb24", "-c:v", "rawvideo"]
        } else {
            vec!["-map", "0:a:0", "-c:a", "pcm_s16le"]
        }
        .into_iter()
        .map(str::to_owned),
    );
    args.extend(["-f", "hash", "-hash", "sha256", "-"].map(str::to_owned));
    let value = media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    String::from_utf8_lossy(&value)
        .trim()
        .strip_prefix("SHA256=")
        .map(str::to_owned)
        .ok_or_else(|| error("RENDER_VALIDATION_FAILED", "Decoded hash missing"))
}
pub fn run(recipe: &Recipe, root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let started = Instant::now();
    let output = render::destination(output, output_root)?;
    let source = check(&recipe.source, root)?;
    let selected = mapping(recipe, &source)?;
    let decoder = crate::acceleration::select(recipe.decode, &source.metadata)?;
    let lut = recipe.lut.as_ref().map(|lut| lut.load(root)).transpose()?;
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let source_video = scratch.0.join("source.rgb");
    let source_audio = scratch.0.join("source.pcm");
    let mut decode_video_micros = 0;
    if !source.pts.is_empty() {
        let mut args = input_args(&source.path);
        decoder.input_args(&mut args);
        args.extend(["-map", "0:v:0", "-an", "-fps_mode", "passthrough"].map(str::to_owned));
        let mut filters = decoder.filter().into_iter().collect::<Vec<_>>();
        if matches!(recipe.source.color, Some(Color::Bt709Limited)) {
            filters.push("scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd");
        }
        if !filters.is_empty() {
            args.extend(["-vf".into(), filters.join(",")]);
        }
        let native_format = if source.layout.is_some() {
            source.metadata["streams"]
                .as_array()
                .expect("checked streams")
                .iter()
                .find(|s| s["codec_type"] == "video")
                .expect("video")["pix_fmt"]
                .as_str()
                .expect("pixel format")
        } else {
            "rgb24"
        };
        args.extend(["-pix_fmt", native_format, "-f", "rawvideo"].map(str::to_owned));
        args.push(source_video.to_string_lossy().into_owned());
        let decoding = Instant::now();
        media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
        decode_video_micros = decoding.elapsed().as_micros();
        if fs::metadata(&source_video)?.len()
            != source.pts.len() as u64
                * source
                    .layout
                    .map_or(source.width as usize * source.height as usize * 3, |l| {
                        l.bytes(source.width, source.height)
                    }) as u64
        {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Decoded source video count changed",
            ));
        }
    }
    let mut decoded = Vec::new();
    if source.samples > 0 && matches!(recipe.audio, Audio::Resample) {
        let mut args = input_args(&source.path);
        args.extend(
            ["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "s16le"].map(str::to_owned),
        );
        args.push(source_audio.to_string_lossy().into_owned());
        media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
        if fs::metadata(&source_audio)?.len() != source.samples * source.channels * 2 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Decoded source audio count changed",
            ));
        }
        let bytes = fs::read(&source_audio)?;
        decoded = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| i16::from_le_bytes(*v))
            .collect::<Vec<_>>();
    }
    let raw_video = scratch.0.join("video.rgb");
    let raw_audio = scratch.0.join("audio.pcm");
    let mut video = BufWriter::new(File::create_new(&raw_video)?);
    let mut input = if source.pts.is_empty() {
        None
    } else {
        Some(File::open(&source_video)?)
    };
    let source_size = source.width as usize * source.height as usize * 3;
    let mut first_pixels = vec![0u8; source_size];
    let mut second_pixels = vec![0u8; source_size];
    let mut first_index = None;
    let mut second_index = None;
    let mut native_pixels = vec![
        0u8;
        source
            .layout
            .map_or(source_size, |l| l.bytes(source.width, source.height))
    ];
    let mut pixels = vec![0u8; recipe.width as usize * recipe.height as usize * 3];
    for n in 0..recipe.duration.units(FPS)? {
        if let Some(input) = &mut input {
            let sample = &selected.frames[n as usize];
            let mut load = |index: usize, buffer: &mut [u8]| -> Result<()> {
                input.seek(SeekFrom::Start(index as u64 * native_pixels.len() as u64))?;
                input.read_exact(&mut native_pixels)?;
                if let Some(layout) = source.layout {
                    recipe.source.sdr.expect("checked SDR").convert(
                        layout,
                        source.width,
                        source.height,
                        &native_pixels,
                        recipe.working_transfer.expect("checked working transfer"),
                        buffer,
                    );
                } else {
                    buffer.copy_from_slice(&native_pixels);
                }
                if let Some(lut) = &lut {
                    lut.apply_rgb(buffer)?;
                }
                Ok(())
            };
            if first_index != Some(sample.first) {
                if second_index == Some(sample.first) {
                    std::mem::swap(&mut first_pixels, &mut second_pixels);
                    std::mem::swap(&mut first_index, &mut second_index);
                } else {
                    load(sample.first, &mut first_pixels)?;
                    first_index = Some(sample.first);
                }
            }
            if sample.second != sample.first && second_index != Some(sample.second) {
                load(sample.second, &mut second_pixels)?;
                second_index = Some(sample.second);
            }
            let weight = sample.second_weight;
            for y in 0..recipe.height as usize {
                for x in 0..recipe.width as usize {
                    let p = ((y * source.height as usize / recipe.height as usize)
                        * source.width as usize
                        + x * source.width as usize / recipe.width as usize)
                        * 3;
                    let out = (y * recipe.width as usize + x) * 3;
                    for c in 0..3 {
                        pixels[out + c] = if weight.num == 0 {
                            first_pixels[p + c]
                        } else {
                            let sum = first_pixels[p + c] as u128
                                * (weight.den - weight.num) as u128
                                + second_pixels[p + c] as u128 * weight.num as u128;
                            ((sum + weight.den as u128 / 2) / weight.den as u128) as u8
                        };
                    }
                }
            }
        }
        video.write_all(&pixels)?;
    }
    video.flush()?;
    drop(video);
    drop(input);
    let mut audio = BufWriter::new(File::create_new(&raw_audio)?);
    let source_in = if decoded.is_empty() || selected.remap.is_some() {
        0
    } else {
        recipe.source_in.units(Time::new(source.audio_rate, 1)?)?
    };
    for n in 0..recipe.duration.units(Time { num: 48000, den: 1 })? {
        let (a, remainder, denominator) = if decoded.is_empty() {
            (0, 0, 1)
        } else if let Some(remap) = &selected.remap {
            let position = remap
                .source_time(Time::new(n, 48000)?)?
                .times(Time::new(source.audio_rate, 1)?)?;
            (
                position.num / position.den,
                (position.num % position.den) as i128,
                position.den as i128,
            )
        } else {
            let numerator = n as u128 * source.audio_rate as u128 * recipe.rate.num as u128;
            let denominator = 48000u128 * recipe.rate.den as u128;
            (
                source_in + (numerator / denominator) as u64,
                (numerator % denominator) as i128,
                denominator as i128,
            )
        };
        if !decoded.is_empty() && a >= source.samples {
            return Err(error(
                "INVALID_RANGE",
                "Mapped audio sample exceeds source duration",
            ));
        }
        for channel in 0..2 {
            let value = if decoded.is_empty() {
                0
            } else {
                let b = (a + 1).min(source.samples - 1);
                let ch = if source.channels == 1 { 0 } else { channel };
                let x = decoded[(a * source.channels + ch) as usize] as i128;
                let y = decoded[(b * source.channels + ch) as usize] as i128;
                let sum = x * (denominator - remainder) + y * remainder;
                ((sum.abs() + denominator / 2) / denominator * sum.signum()) as i16
            };
            audio.write_all(&value.to_le_bytes())?;
        }
    }
    audio.flush()?;
    drop(audio);
    let temp = scratch.0.join("output.mkv");
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(recipe.width, recipe.height);
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-n".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "rgb24".into(),
        "-video_size".into(),
        format!("{}x{}", recipe.width, recipe.height),
        "-framerate".into(),
        "25".into(),
        "-i".into(),
        raw_video.to_string_lossy().into_owned(),
        "-f".into(),
        "s16le".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-i".into(),
        raw_audio.to_string_lossy().into_owned(),
        "-vf".into(),
        "setsar=1".into(),
        "-c:v".into(),
        "ffv1".into(),
        "-level".into(),
        ffv1_level.into(),
        "-slices".into(),
        ffv1_slices.into(),
        "-pix_fmt".into(),
        "bgr0".into(),
        "-threads".into(),
        "1".into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-f".into(),
        "matroska".into(),
    ];
    if let Some(transfer) = recipe.working_transfer {
        args.extend(
            [
                "-colorspace",
                "rgb",
                "-color_range",
                "pc",
                "-color_primaries",
                "bt709",
                "-color_trc",
                transfer.tag(),
            ]
            .map(str::to_owned),
        );
    }
    args.push(temp.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    let verified =
        render::inspect_reference(&temp, recipe.width, recipe.height, &media::Uncontrolled)?;
    if verified.frames != recipe.duration.units(FPS)?
        || verified.samples != verified.frames * 1920
        || decoded_hash(&temp, true)? != media::file_hash(&raw_video)?
        || decoded_hash(&temp, false)? != media::file_hash(&raw_audio)?
    {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Conformed output counts or decoded content differ",
        ));
    }
    if let Some(transfer) = recipe.working_transfer {
        let output_metadata = media::probe(&temp)?;
        let video = output_metadata["streams"]
            .as_array()
            .expect("verified streams")
            .iter()
            .find(|s| s["codec_type"] == "video")
            .expect("verified video");
        color::Input {
            matrix: color::Matrix::Rgb,
            range: color::Range::Full,
            transfer,
            missing_tags: color::MissingTags::Reject,
        }
        .inspect(video, true, false)
        .map_err(|_| {
            error(
                "RENDER_VALIDATION_FAILED",
                "Normalized output color tags differ from the declared working space",
            )
        })?;
    }
    scene::identity_bytes(&recipe.source.file, root)?;
    if let Some(lut) = &recipe.lut {
        scene::identity_bytes(&lut.file, root)?;
    }
    let mut result = report(recipe, &source, &selected);
    result["decode"] = decoder.report();
    result["timing"] = json!({"decode_video_micros":decode_video_micros,"elapsed_micros":started.elapsed().as_micros()});
    result["lut"] = lut.as_ref().map(|v| v.report()).unwrap_or(Value::Null);
    result["output"] = json!(output);
    result["sha256"] = json!(verified.sha256);
    result["ffmpeg"] = json!(media::version("ffmpeg")?);
    result["ffprobe"] = json!(media::version("ffprobe")?);
    result["asset"] = json!({"id":recipe.id,"path":output,"duration":recipe.duration,"identity":{"sha256":verified.sha256,"bytes":fs::metadata(&temp)?.len()}});
    media::publish(&temp, &output)?;
    Ok(result)
}
pub fn capabilities() -> Value {
    let mut value = json!({"profile":"media-conform-v1","containers":["mkv_ffv1_pcm16","mp4_mov_h264_aac","wav_pcm16"],"source_identity_required":true,"output":"reference-ffv1-pcm-v1","maximum_seconds":60,"maximum_decoded_video_bytes":VIDEO_BYTES,"source_maximum_bytes":67108864,"rate_minimum":{"num":1,"den":16},"rate_maximum":{"num":16,"den":1},"reverse":true,"freeze":true,"reverse_freeze_audio":"explicit_mute","forward_audio":"linear_resampling_changes_pitch","runtime_network":false,"remap":{"maximum_segments":64,"segment_clock":48000,"rate_minimum":0,"rate_maximum":16,"speed_interpolation":"linear_exact_integral","video_sampling":["previous","nearest","linear"],"audio_pitch":["follow_speed","mute"],"reverse_freeze_pitch":"mute","continuous_source_position":true},"sdr_normalization":color::capabilities(),"lut":crate::lut::capabilities()});
    value["decode"] = crate::acceleration::capabilities();
    value
}
