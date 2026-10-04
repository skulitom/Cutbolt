//! Explicit timestamp sampling into a lossless editing asset; originals stay external.
use crate::{
    Result, color, error, media, remap, render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const FPS: Time = Time { num: 25, den: 1 };
/// Random-access decode window (reverse/freeze/non-monotonic speed maps) kept in scratch.
const VIDEO_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Decoded PCM window held in memory for resampling.
const AUDIO_BYTES: u64 = 512 * 1024 * 1024;
/// Sources are hashed by streaming, never loaded whole.
const SOURCE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// One hour at 30 fps of decoded source timestamps.
const SOURCE_FRAMES: usize = 108_000;
const SOURCE_AUDIO_SECONDS: u64 = 3600;
/// Thirty minutes of 25 fps output, streamed into the encoder.
const OUTPUT_FRAMES: u64 = 45_000;

/// Tool time grows with media length; bounded so a stuck tool still fails.
fn tool_timeout(seconds: f64) -> Duration {
    Duration::from_secs((180.0 + 2.0 * seconds.max(0.0)).min(4.0 * 3600.0) as u64)
}
/// Legacy video interpretation: `encoded_rgb` keeps MKV FFV1/bgr0 values as stored; `bt709_limited` converts tagged limited-range BT.709 H.264 yuv420p MP4/MOV to full-range RGB.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    EncodedRgb,
    Bt709Limited,
}
/// Audio policy: `resample` linearly resamples source audio (pitch follows speed); `mute` writes exact silence and is required for reverse or freeze.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Audio {
    Resample,
    Mute,
}
/// Identity-bound conform source and its declared color interpretation.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Source file identity; path relative to `input_root`, at most 16 GiB (MKV, MP4, MOV or WAV).
    pub file: Identity,
    /// Legacy interpretation; null for audio-only WAV and when `sdr` is used (never both).
    pub color: Option<Color>,
    /// Explicit SDR normalization; requires recipe `working_transfer`. Omit for the legacy path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sdr: Option<color::Input>,
}
/// Media conversion recipe for media.conform: samples a source into a new FFV1/PCM16 editing asset at `frame_rate` (default 25 fps).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Output frame rate, one of 24, 25, 30, 50, 60, 24000/1001, 30000/1001 or 60000/1001; default 25. Use the source's own rate to keep every frame, which avoids judder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<Time>,
    /// Recipe format version; must be 1.
    pub schema_version: u32,
    /// Asset ID for the returned asset (1-128 bytes, not blank).
    pub id: String,
    /// Source file and color interpretation.
    pub source: Source,
    /// Source position in rational seconds at output time zero; on a source audio sample when resampling without remap.
    pub source_in: Time,
    /// Output duration in rational seconds; whole frames at `frame_rate` and whole 48 kHz samples, 1 to 45000 frames.
    pub duration: Time,
    /// Constant speed factor from 1/16 to 16; must be 1 with `freeze` or `remap`.
    pub rate: Time,
    /// Play backwards from `source_in`; requires `audio: mute`.
    pub reverse: bool,
    /// Hold `source_in` for every frame; requires unit `rate`, `reverse: false` and `audio: mute`.
    pub freeze: bool,
    /// Output width in pixels; at most 4096 and 8M pixels in total.
    pub width: u32,
    /// Output height in pixels; at most 2160 and 8M pixels in total.
    pub height: u32,
    /// Audio policy; must agree with `remap.audio_pitch` when remapping.
    pub audio: Audio,
    /// Variable speed map replacing constant `rate`; omit for constant speed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remap: Option<remap::Remap>,
    /// Output RGB transfer; required with `source.sdr` and omitted otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working_transfer: Option<color::Transfer>,
    /// Optional .cube LUT applied after SDR normalization; requires `source.sdr`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lut: Option<crate::lut::Transform>,
    /// Source video decode backend; omit for CPU decoding.
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
/// Decoded frame list in compact `key=value|...` lines (one per frame), converted to JSON objects.
/// Numeric values become numbers and unavailable values are omitted, matching ffprobe's JSON writer.
fn frames(path: &Path, selector: &str, timeout: Duration) -> Result<Vec<Value>> {
    let args = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file,pipe",
        "-select_streams",
        selector,
        "-show_entries",
        "frame=best_effort_timestamp,duration,pkt_duration,nb_samples,width,height,interlaced_frame,color_space,color_range,color_primaries,color_transfer,chroma_location",
        "-of",
        "compact=p=0:nk=0",
    ];
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    args.push(path.to_string_lossy().into_owned());
    let output = media::capture(&media::tool("ffprobe"), &args, timeout)?;
    let mut result = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut object = serde_json::Map::new();
        for field in line.split('|') {
            let Some((key, value)) = field.split_once('=') else {
                continue;
            };
            if value == "N/A" || value.is_empty() {
                continue;
            }
            object.insert(
                key.into(),
                value
                    .parse::<u64>()
                    .map(Value::from)
                    .unwrap_or_else(|_| Value::from(value)),
            );
        }
        result.push(Value::Object(object));
        if result.len() > SOURCE_FRAMES * 2 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Decoded frame list exceeds the source bound",
            ));
        }
    }
    Ok(result)
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
    let path = scene::identity_file(&source.file, root, SOURCE_BYTES)?;
    let metadata = media::probe(&path)?;
    let seconds = metadata["format"]["duration"]
        .as_str()
        .and_then(|d| d.parse::<f64>().ok())
        .unwrap_or(0.0);
    let timeout = tool_timeout(seconds);
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
        let decoded = frames(&checked.path, "v:0", timeout)?;
        if decoded.is_empty() || decoded.len() > SOURCE_FRAMES {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!("Decoded video exceeds {SOURCE_FRAMES} frames"),
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
        let decoded = frames(&checked.path, "a:0", timeout)?;
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
            if n == 0 || checked.samples > checked.audio_rate * SOURCE_AUDIO_SECONDS {
                return Err(error("LIMIT_EXCEEDED", "Decoded audio exceeds one hour"));
            }
        }
        if checked.samples == 0 {
            return Err(unsupported("Audio stream has no samples"));
        }
    }
    // Probe output alone can conceal recoverable codec errors; require a strict full decode.
    let mut args = input_args(&checked.path);
    args.extend(["-map", "0", "-f", "null", "-"].map(str::to_owned));
    media::capture(&media::tool("ffmpeg"), &args, timeout)?;
    scene::identity_file(&source.file, root, SOURCE_BYTES)?;
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
impl Recipe {
    /// The validated output frame rate.
    fn clock(&self) -> Result<Time> {
        render::clock::rate(self.frame_rate.unwrap_or(FPS))
    }
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
    let rate = recipe.clock()?;
    let count = recipe.duration.units(rate)?;
    recipe.duration.units(Time::new(48000, 1)?)?;
    if recipe.schema_version != 1
        || recipe.id.trim().is_empty()
        || recipe.id.len() > 128
        || count == 0
        || count > OUTPUT_FRAMES
        || recipe.width == 0
        || recipe.height == 0
        || recipe.width > 4096
        || recipe.height > 2160
        || recipe.width as u64 * recipe.height as u64 > 8_000_000
        || recipe.rate.compare(Time::new(1, 16)?)? == Ordering::Less
        || recipe.rate.compare(Time::new(16, 1)?)? == Ordering::Greater
    {
        return Err(error(
            "INVALID_CONFORM",
            "Conform v1 requires 1..45000 output frames (30 minutes), dimensions up to 4096x2160/8M pixels and rate 1/16..16",
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
    // Container timestamps are rounded to the source time base (whole milliseconds in Matroska),
    // so a frame within half a tick after an output time is the frame shown at that time.
    let half_tick = source.metadata["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .and_then(|v| v["time_base"].as_str()?.split_once('/'))
        .and_then(|(n, d)| Time::new(n.parse().ok()?, 2 * d.parse::<u64>().ok()?).ok())
        .unwrap_or(Time::ZERO);
    for n in 0..count {
        let output_time = Time::new(n * rate.den, rate.num)?;
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
            let reach = t.plus(half_tick)?;
            let first = source
                .pts
                .partition_point(|pts| {
                    pts.compare(reach).expect("validated rational") != Ordering::Greater
                })
                .max(1)
                - 1;
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
                let weight = if t.compare(source.pts[first])?.is_lt() {
                    Time::ZERO
                } else {
                    t.minus(source.pts[first])?
                        .times(Time::new(gap.den, gap.num)?)?
                };
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
    let mut report = json!({"profile":"media-conform-v1","source":{"path":source.path,"identity":recipe.source.file,"metadata":source.metadata,"video_frames":source.pts.len(),"video_end":source.video_end,"audio_samples":source.samples,"audio_rate":source.audio_rate,"audio_channels":source.channels},"source_frame_indices":selected,"source_frame_times":selected.iter().map(|i|source.pts[*i]).collect::<Vec<_>>(),"frame_rate":recipe.clock().expect("validated"),"frames":recipe.duration.units(recipe.clock().expect("validated")).expect("validated"),"samples":recipe.duration.units(Time{num:48000,den:1}).expect("validated"),"width":recipe.width,"height":recipe.height,"duration":recipe.duration,"rate":recipe.rate,"reverse":recipe.reverse,"freeze":recipe.freeze,"audio":recipe.audio,"video_sampling":"latest_source_timestamp_at_or_before_output_clock_plus_half_source_tick","resize":"nearest_top_left","audio_sampling":"linear_pitch_changes_with_rate","color":recipe.source.color,"normalization":source.normalization,"working_transfer":recipe.working_transfer});
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
fn decoded_hash(path: &Path, video: bool, timeout: Duration) -> Result<String> {
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
    let value = media::capture(&media::tool("ffmpeg"), &args, timeout)?;
    String::from_utf8_lossy(&value)
        .trim()
        .strip_prefix("SHA256=")
        .map(str::to_owned)
        .ok_or_else(|| error("RENDER_VALIDATION_FAILED", "Decoded hash missing"))
}

fn seconds(t: Time) -> f64 {
    t.num as f64 / t.den as f64
}

/// Inclusive source sample range read by the resampler (each output sample interpolates a, a+1).
fn audio_window(recipe: &Recipe, source: &Checked, mapped: &Mapping) -> Result<(u64, u64)> {
    let rate = Time::new(source.audio_rate, 1)?;
    let (first, last) = if let Some(remap) = &mapped.remap {
        // Speed is non-negative with a fixed direction within each span, so every source
        // position lies between the extreme span endpoints.
        let mut low: Option<Time> = None;
        let mut high: Option<Time> = None;
        for span in &remap.spans {
            for t in [span.source_start, span.source_end] {
                if low.is_none_or(|l| t.compare(l).expect("rational").is_lt()) {
                    low = Some(t);
                }
                if high.is_none_or(|h| t.compare(h).expect("rational").is_gt()) {
                    high = Some(t);
                }
            }
        }
        let low = low.expect("spans").times(rate)?;
        let high = high.expect("spans").times(rate)?;
        (low.num / low.den, high.num.div_ceil(high.den) + 1)
    } else {
        let start = recipe.source_in.units(rate)?;
        let last_output = recipe.duration.units(Time::new(48000, 1)?)? - 1;
        let numerator = last_output as u128 * source.audio_rate as u128 * recipe.rate.num as u128;
        let denominator = 48000u128 * recipe.rate.den as u128;
        (start, start + (numerator / denominator) as u64 + 1)
    };
    Ok((first.min(source.samples - 1), last.min(source.samples - 1)))
}

/// Decoded source frames, read in nondecreasing index order (stream) or by random access (window).
enum SourceFrames {
    Window {
        file: File,
        first: usize,
    },
    Stream {
        reader: media::StreamReader,
        next: usize,
    },
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
    let rate = recipe.clock()?;
    let count = recipe.duration.units(rate)?;
    let media_seconds = seconds(source.video_end)
        .max(source.samples as f64 / source.audio_rate.max(1) as f64)
        + count as f64 * rate.den as f64 / rate.num as f64;
    let timeout = tool_timeout(media_seconds);
    let mut decode_video_micros = 0;

    // Audio: decode only the sample window the resampler reads.
    let mut decoded = Vec::new();
    let mut audio_first = 0u64;
    if source.samples > 0 && matches!(recipe.audio, Audio::Resample) {
        let (first, last) = audio_window(recipe, &source, &selected)?;
        let bytes = (last - first + 1) * source.channels * 2;
        if bytes > AUDIO_BYTES {
            return Err(error(
                "LIMIT_EXCEEDED",
                "The source audio read by this recipe exceeds the 512 MiB decode window",
            ));
        }
        let mut args = input_args(&source.path);
        args.extend([
            "-map".into(),
            "0:a:0".into(),
            "-vn".into(),
            "-af".into(),
            format!("atrim=start_sample={first}:end_sample={}", last + 1),
            "-c:a".into(),
            "pcm_s16le".into(),
            "-f".into(),
            "s16le".into(),
        ]);
        args.push(source_audio.to_string_lossy().into_owned());
        media::capture(&media::tool("ffmpeg"), &args, timeout)?;
        if fs::metadata(&source_audio)?.len() != bytes {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Decoded source audio count changed",
            ));
        }
        let raw = fs::read(&source_audio)?;
        decoded = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| i16::from_le_bytes(*v))
            .collect::<Vec<_>>();
        audio_first = first;
    }
    let raw_audio = scratch.0.join("audio.pcm");
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
                let x = decoded[((a - audio_first) * source.channels + ch) as usize] as i128;
                let y = decoded[((b - audio_first) * source.channels + ch) as usize] as i128;
                let sum = x * (denominator - remainder) + y * remainder;
                ((sum.abs() + denominator / 2) / denominator * sum.signum()) as i16
            };
            audio.write_all(&value.to_le_bytes())?;
        }
    }
    audio.flush()?;
    drop(audio);
    drop(decoded);

    // Video: decode only frames lo..=hi. Forward mappings stream through a pipe; reverse and
    // other non-monotonic mappings use a bounded random-access window in scratch.
    let native_size = source
        .layout
        .map_or(source.width as usize * source.height as usize * 3, |l| {
            l.bytes(source.width, source.height)
        });
    let mut frames_in = None;
    if !selected.frames.is_empty() {
        let lo = selected
            .frames
            .iter()
            .map(|f| f.first)
            .min()
            .expect("frames");
        let hi = selected
            .frames
            .iter()
            .map(|f| f.first.max(f.second))
            .max()
            .expect("frames");
        let monotonic = selected
            .frames
            .windows(2)
            .all(|w| w[1].first >= w[0].first && w[1].second >= w[0].second);
        let window = (hi - lo + 1) as u64 * native_size as u64;
        // CPU decoding of forward/freeze mappings streams; hardware decoding keeps its verified
        // scratch-file stage, so it shares the random-access window bound.
        let stream = monotonic && !decoder.hardware();
        if !stream && window > VIDEO_BYTES {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Reverse, non-monotonic or hardware-decoded mappings decode at most 4 GiB of source frames; shorten the range",
            ));
        }
        let mut args = input_args(&source.path);
        decoder.input_args(&mut args);
        args.extend(["-map", "0:v:0", "-an", "-fps_mode", "passthrough"].map(str::to_owned));
        let mut filters = decoder
            .filter()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        filters.push(format!("trim=start_frame={lo}:end_frame={}", hi + 1));
        if matches!(recipe.source.color, Some(Color::Bt709Limited)) {
            filters.push(
                "scale=in_color_matrix=bt709:in_range=tv:out_range=pc:flags=accurate_rnd".into(),
            );
        }
        args.extend(["-vf".into(), filters.join(",")]);
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
        if stream {
            args.push("pipe:1".into());
            frames_in = Some(SourceFrames::Stream {
                reader: media::StreamReader::spawn(&media::tool("ffmpeg"), &args, timeout)?,
                next: lo,
            });
        } else {
            args.push(source_video.to_string_lossy().into_owned());
            let decoding = Instant::now();
            media::capture(&media::tool("ffmpeg"), &args, timeout)?;
            decode_video_micros = decoding.elapsed().as_micros();
            if fs::metadata(&source_video)?.len() != window {
                return Err(error(
                    "RENDER_VALIDATION_FAILED",
                    "Decoded source video count changed",
                ));
            }
            frames_in = Some(SourceFrames::Window {
                file: File::open(&source_video)?,
                first: lo,
            });
        }
    }
    let temp = scratch.0.join("output.mkv");
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(recipe.width, recipe.height);
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-n".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "rgb24".into(),
        "-video_size".into(),
        format!("{}x{}", recipe.width, recipe.height),
        "-framerate".into(),
        format!("{}/{}", rate.num, rate.den),
        "-i".into(),
        "pipe:0".into(),
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
        ffv1_slices.into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-f".into(),
        "matroska".into(),
    ];
    if rate != FPS {
        // The rendering clock options: exact frame timestamps at fractional and high rates.
        args.extend([
            "-r".into(),
            format!("{}/{}", rate.num, rate.den),
            "-fps_mode".into(),
            "cfr".into(),
            "-enc_time_base:v".into(),
            format!("{}/{}", rate.den, rate.num),
        ]);
    }
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
    let source_size = source.width as usize * source.height as usize * 3;
    let mut video_hash = Sha256::new();
    media::feed_stdin(&media::tool("ffmpeg"), &args, timeout, |stdin| {
        let mut first_pixels = vec![0u8; source_size];
        let mut second_pixels = vec![0u8; source_size];
        let mut first_index = None;
        let mut second_index = None;
        let mut native_pixels = vec![0u8; native_size];
        let mut pixels = vec![0u8; recipe.width as usize * recipe.height as usize * 3];
        for n in 0..count {
            if let Some(frames_in) = &mut frames_in {
                let sample = &selected.frames[n as usize];
                let mut load = |index: usize, buffer: &mut [u8]| -> Result<()> {
                    match frames_in {
                        SourceFrames::Window { file, first } => {
                            file.seek(SeekFrom::Start(
                                (index - *first) as u64 * native_pixels.len() as u64,
                            ))?;
                            file.read_exact(&mut native_pixels)?;
                        }
                        SourceFrames::Stream { reader, next } => {
                            let waiting = Instant::now();
                            if index < *next {
                                return Err(error(
                                    "RENDER_VALIDATION_FAILED",
                                    "Streamed source frames were requested out of order",
                                ));
                            }
                            while *next <= index {
                                reader.read_exact(&mut native_pixels)?;
                                *next += 1;
                            }
                            decode_video_micros += waiting.elapsed().as_micros();
                        }
                    }
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
            video_hash.update(&pixels);
            stdin.write_all(&pixels)?;
        }
        Ok(())
    })?;
    if let Some(SourceFrames::Stream { reader, .. }) = frames_in {
        reader.finish()?;
    }
    let video_hash = format!("{:x}", video_hash.finalize());
    let verified = render::inspect_reference_at(
        &temp,
        recipe.width,
        recipe.height,
        rate,
        &media::Uncontrolled,
    )?;
    if verified.frames != count
        || verified.samples != recipe.duration.units(Time::new(48000, 1)?)?
        || decoded_hash(&temp, true, timeout)? != video_hash
        || decoded_hash(&temp, false, timeout)? != media::file_hash(&raw_audio)?
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
    scene::identity_file(&recipe.source.file, root, SOURCE_BYTES)?;
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
    let mut value = json!({"profile":"media-conform-v1","containers":["mkv_ffv1_pcm16","mp4_mov_h264_aac","wav_pcm16"],"source_identity_required":true,"output":"reference-ffv1-pcm-v1","output_frame_rates":"eight_native_rates_default_25","video_selection":"latest_timestamp_at_or_before_output_time_plus_half_source_tick","maximum_seconds":OUTPUT_FRAMES/25,"maximum_output_frames":OUTPUT_FRAMES,"source_maximum_bytes":SOURCE_BYTES,"source_maximum_frames":SOURCE_FRAMES,"source_maximum_audio_seconds":SOURCE_AUDIO_SECONDS,"decoding":{"forward":"streamed_needed_frames","reverse_or_non_monotonic":"random_access_window","maximum_window_bytes":VIDEO_BYTES,"maximum_audio_window_bytes":AUDIO_BYTES,"source_hash":"streamed_sha256"},"output_encoding":"streamed_with_running_decoded_hash","rate_minimum":{"num":1,"den":16},"rate_maximum":{"num":16,"den":1},"reverse":true,"freeze":true,"reverse_freeze_audio":"explicit_mute","forward_audio":"linear_resampling_changes_pitch","runtime_network":false,"remap":{"maximum_segments":64,"segment_clock":48000,"rate_minimum":0,"rate_maximum":16,"speed_interpolation":"linear_exact_integral","video_sampling":["previous","nearest","linear"],"audio_pitch":["follow_speed","mute"],"reverse_freeze_pitch":"mute","continuous_source_position":true},"sdr_normalization":color::capabilities(),"lut":crate::lut::capabilities()});
    value["decode"] = crate::acceleration::capabilities();
    value
}
