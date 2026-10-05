//! Bounded high-bit-depth/HDR media processing with exact time selection and explicit color.
use crate::{
    Result,
    color::{MissingTags, Range},
    conform::Audio,
    error,
    hdr_color::{Encoding, Primaries, Processor, Tone, invalid},
    hdr_metadata, media,
    model::Project,
    render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const FPS: Time = Time { num: 25, den: 1 };
const RAW_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
/// Source sample matrix: `rgb` (planar RGB, or bgr0 for SDR), `bt709` or `bt2020_ncl` (YUV444, non-constant luminance); a YUV matrix must match the declared primaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Matrix {
    Rgb,
    Bt709,
    Bt2020Ncl,
}
/// Output sample depth: `rgb16` keeps HDR or wide gamut; `rgb8` requires SDR BT.709 output and returns a timeline asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Depth {
    Rgb8,
    Rgb16,
}
/// Identity-bound HDR or high-bit-depth source (25 fps FFV1 with 48 kHz stereo PCM16 in MKV) and its declared interpretation.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// Source file identity; path relative to `input_root`.
    pub file: Identity,
    /// Declared transfer, primaries and display of the source values.
    pub encoding: Encoding,
    /// Sample matrix of the stored pixels.
    pub matrix: Matrix,
    /// Code range, scaled to the native bit depth.
    pub range: Range,
    /// Policy for absent or unknown color tags.
    pub missing_tags: MissingTags,
}
/// Target encoding and sample depth of an HDR conversion.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Output {
    /// Target transfer, primaries and display.
    pub encoding: Encoding,
    /// Output RGB sample depth.
    pub depth: Depth,
}
/// HDR conversion recipe for hdr.inspect and hdr.conform; writes a tagged 25 fps FFV1/PCM16 MKV. Every field is required.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Recipe format version; must be 1.
    pub schema_version: u32,
    /// Asset ID for the asset returned by RGB8 output (1-128 bytes, not blank).
    pub id: String,
    /// Source file and its declared interpretation.
    pub source: Source,
    /// Target encoding and depth.
    pub output: Output,
    /// Source position in rational seconds at output time zero; on a 48 kHz sample when `audio` is resample.
    pub source_in: Time,
    /// Output duration in rational seconds; whole 25 fps frames, 1 to 1500 frames.
    pub duration: Time,
    /// Constant speed factor from 1/16 to 16; must be 1 with `freeze`.
    pub rate: Time,
    /// Play backwards from `source_in`; requires `audio: mute`.
    pub reverse: bool,
    /// Hold `source_in` for every frame; requires unit `rate`, `reverse: false` and `audio: mute`.
    pub freeze: bool,
    /// Output width in pixels; at most 4096 and 8M pixels in total.
    pub width: u32,
    /// Output height in pixels; at most 2160 and 8M pixels in total.
    pub height: u32,
    /// Exposure gain on absolute light in milli-EV, -8000 to 8000 (1000 is one stop).
    pub exposure_milliev: i32,
    /// Tone-mapping policy; HDR to SDR needs `clip` or `reinhard`.
    pub tone: Tone,
    /// Audio policy; reverse and freeze require `mute`.
    pub audio: Audio,
}
#[derive(Clone, Copy)]
enum Layout {
    Bgr8,
    Planar { bits: u32, rgb: bool },
}
impl Layout {
    fn stride(self, pixels: usize) -> usize {
        pixels * if matches!(self, Self::Bgr8) { 4 } else { 6 }
    }
    fn rgb(self, raw: &[u8], p: usize, n: usize, matrix: Matrix, range: Range) -> [f64; 3] {
        let (codes, bits) = match self {
            Self::Bgr8 => (
                [
                    raw[p * 4 + 2] as f64,
                    raw[p * 4 + 1] as f64,
                    raw[p * 4] as f64,
                ],
                8,
            ),
            Self::Planar { bits, rgb } => {
                let order = if rgb { [2, 0, 1] } else { [0, 1, 2] };
                (
                    order.map(|c| {
                        u16::from_le_bytes(
                            raw[(c * n + p) * 2..(c * n + p) * 2 + 2]
                                .try_into()
                                .expect("sample"),
                        ) as f64
                    }),
                    bits,
                )
            }
        };
        let factor = (1u32 << (bits - 8)) as f64;
        let maximum = ((1u32 << bits) - 1) as f64;
        let (offset, span, chroma) = if range == Range::Full {
            (0.0, maximum, maximum)
        } else {
            (16.0 * factor, 219.0 * factor, 224.0 * factor)
        };
        if matrix == Matrix::Rgb {
            return codes.map(|v| ((v - offset) / span).clamp(0.0, 1.0));
        }
        let y = (codes[0] - offset) / span;
        let cb = (codes[1] - 128.0 * factor) / chroma;
        let cr = (codes[2] - 128.0 * factor) / chroma;
        let (kr, kb) = if matrix == Matrix::Bt709 {
            (0.2126, 0.0722)
        } else {
            (0.2627, 0.0593)
        };
        let r = y + 2.0 * (1.0 - kr) * cr;
        let b = y + 2.0 * (1.0 - kb) * cb;
        [r, (y - kr * r - kb * b) / (1.0 - kr - kb), b].map(|v| v.clamp(0.0, 1.0))
    }
}
struct Checked {
    path: PathBuf,
    width: u32,
    height: u32,
    frames: u64,
    layout: Layout,
    pix_fmt: String,
    metadata: Value,
    assumed: BTreeSet<String>,
}
fn matrix_tag(m: Matrix) -> &'static str {
    match m {
        Matrix::Rgb => "gbr",
        Matrix::Bt709 => "bt709",
        Matrix::Bt2020Ncl => "bt2020nc",
    }
}
fn rational_float(v: &Value) -> Option<f64> {
    let (n, d) = v.as_str()?.split_once('/')?;
    let n = n.parse::<f64>().ok()?;
    let d = d.parse::<f64>().ok()?;
    let x = n / d;
    if x.is_finite() { Some(x) } else { None }
}
fn metadata(
    encoding: Encoding,
    matrix: Matrix,
    range: Range,
    policy: MissingTags,
    video: &Value,
    assumed: &mut BTreeSet<String>,
) -> Result<()> {
    for (key, expected) in [
        ("color_space", matrix_tag(matrix)),
        (
            "color_range",
            if range == Range::Full { "pc" } else { "tv" },
        ),
        ("color_primaries", encoding.primaries.tag()),
        ("color_transfer", encoding.transfer.tag()),
    ] {
        match video[key].as_str() {
            None | Some("unknown" | "unspecified") => {
                if policy == MissingTags::Reject {
                    return Err(invalid(format!(
                        "Missing {key}; declare use_declared to interpret missing metadata"
                    )));
                }
                assumed.insert(key.into());
            }
            Some(actual) if actual == expected => (),
            _ => return Err(invalid(format!("Conflicting {key}"))),
        }
    }
    if let Some(data) = video["side_data_list"].as_array() {
        for item in data {
            match item["side_data_type"].as_str() {
                Some("Mastering display metadata") => {
                    for (key, expected) in [
                        ("max_luminance", encoding.display.peak_nits as f64),
                        (
                            "min_luminance",
                            encoding.display.black_millinits as f64 / 1000.0,
                        ),
                    ] {
                        if let Some(value) = item.get(key)
                            && rational_float(value).is_none_or(|v| (v - expected).abs() > 0.00011)
                        {
                            return Err(invalid(format!("Conflicting display {key}")));
                        }
                    }
                }
                Some("Content light level metadata") => (),
                _ => {
                    return Err(invalid(
                        "Unsupported orientation, dynamic metadata or other side data",
                    ));
                }
            }
        }
    }
    Ok(())
}
fn input_args(path: &Path) -> Vec<String> {
    vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-n".into(),
        "-xerror".into(),
        "-err_detect".into(),
        "explode".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-noautorotate".into(),
        "-i".into(),
        path.to_string_lossy().into_owned(),
    ]
}
fn check_file(
    path: &Path,
    encoding: Encoding,
    matrix: Matrix,
    range: Range,
    policy: MissingTags,
) -> Result<Checked> {
    encoding.validate()?;
    if (matrix == Matrix::Bt709 && encoding.primaries != Primaries::Bt709)
        || (matrix == Matrix::Bt2020Ncl && encoding.primaries != Primaries::Bt2020)
    {
        return Err(invalid("YUV matrix must match declared primaries"));
    }
    let meta = media::probe(path)?;
    let streams = meta["streams"]
        .as_array()
        .ok_or_else(|| invalid("Streams required"))?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| invalid("Video required"))?;
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .ok_or_else(|| invalid("Stereo PCM audio required"))?;
    let width = video["width"].as_u64().unwrap_or(0);
    let height = video["height"].as_u64().unwrap_or(0);
    if !path
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("mkv"))
        || !meta["format"]["format_name"]
            .as_str()
            .unwrap_or("")
            .split(',')
            .any(|s| s == "matroska")
        || streams.len() != 2
        || video["codec_name"] != "ffv1"
        || video["r_frame_rate"] != "25/1"
        || audio["codec_name"] != "pcm_s16le"
        || audio["sample_rate"] != "48000"
        || audio["channels"] != 2
        || width == 0
        || height == 0
        || width > 4096
        || height > 2160
        || width * height > 8_000_000
        || video["tags"].get("rotate").is_some()
        || !matches!(
            video["sample_aspect_ratio"].as_str(),
            None | Some("1:1" | "N/A")
        )
        || !matches!(
            video["field_order"].as_str(),
            None | Some("progressive" | "unknown")
        )
    {
        return Err(invalid(
            "HDR path requires progressive 25 fps FFV1/PCM16 stereo MKV, square pixels and at most 4096x2160/8M pixels",
        ));
    }
    let fmt = video["pix_fmt"].as_str().unwrap_or("");
    let layout = match fmt {
        "bgr0" if matrix == Matrix::Rgb && !encoding.transfer.hdr() => Layout::Bgr8,
        "gbrp10le" | "gbrp12le" | "gbrp16le" if matrix == Matrix::Rgb => Layout::Planar {
            bits: fmt[4..6].parse().expect("format"),
            rgb: true,
        },
        "yuv444p10le" | "yuv444p12le" | "yuv444p16le" if matrix != Matrix::Rgb => Layout::Planar {
            bits: fmt[7..9].parse().expect("format"),
            rgb: false,
        },
        _ => {
            return Err(invalid(
                "Declare native RGB or YUV444 10/12/16-bit samples; 8-bit RGB is accepted for SDR only",
            ));
        }
    };
    let mut assumed = BTreeSet::new();
    metadata(encoding, matrix, range, policy, video, &mut assumed)?;
    let args = [
        "-v".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-select_streams".into(),
        "v:0".into(),
        "-show_frames".into(),
        "-of".into(),
        "json".into(),
        path.to_string_lossy().into_owned(),
    ];
    let decoded: Value = serde_json::from_slice(&media::capture(
        &media::tool("ffprobe"),
        &args,
        Duration::from_secs(120),
    )?)?;
    let frames = decoded["frames"]
        .as_array()
        .ok_or_else(|| invalid("Decoded frames missing"))?;
    if frames.is_empty()
        || frames.len() > 1500
        || frames.len() as u64 * layout.stride((width * height) as usize) as u64 > RAW_LIMIT
    {
        return Err(invalid(
            "HDR sources require 1..1500 frames and at most 4 GiB decoded samples",
        ));
    }
    for (i, f) in frames.iter().enumerate() {
        if render::micros(&f["best_effort_timestamp_time"])? != i as i64 * 40_000
            || f["width"] != width
            || f["height"] != height
            || f["interlaced_frame"] != 0
            || f["pix_fmt"] != fmt
        {
            return Err(invalid("HDR source frame timing or layout changed"));
        }
        let mut profile = video.clone();
        for key in [
            "color_space",
            "color_range",
            "color_transfer",
            "color_primaries",
            "side_data_list",
        ] {
            if let Some(v) = f.get(key) {
                profile[key] = v.clone();
            }
        }
        metadata(encoding, matrix, range, policy, &profile, &mut assumed)?;
    }
    let sound = media::frame_info(path, "a:0")?;
    let sound = sound["frames"]
        .as_array()
        .ok_or_else(|| invalid("Audio frames missing"))?;
    let mut samples = 0u64;
    for f in sound {
        if (render::micros(&f["best_effort_timestamp_time"])?
            - (samples * 1_000_000 / 48000) as i64)
            .abs()
            > 1000
        {
            return Err(invalid("HDR audio timestamps must be continuous"));
        }
        samples = samples
            .checked_add(
                f["nb_samples"]
                    .as_u64()
                    .ok_or_else(|| invalid("Audio count missing"))?,
            )
            .ok_or_else(|| invalid("Audio count overflow"))?;
    }
    if samples != frames.len() as u64 * 1920 {
        return Err(invalid("HDR source audio and video durations must match"));
    }
    Ok(Checked {
        path: path.into(),
        width: width as u32,
        height: height as u32,
        frames: frames.len() as u64,
        layout,
        pix_fmt: fmt.into(),
        metadata: meta,
        assumed,
    })
}
fn prepare(recipe: &Recipe, root: &Path) -> Result<(Checked, Vec<usize>, Processor)> {
    let processor = Processor::new(
        recipe.source.encoding,
        recipe.output.encoding,
        recipe.exposure_milliev,
        recipe.tone,
    )?;
    Project::new(recipe.id.clone(), recipe.width, recipe.height, FPS)?;
    if recipe.schema_version != 1
        || recipe.width > 4096
        || recipe.height > 2160
        || recipe.width as u64 * recipe.height as u64 > 8_000_000
    {
        return Err(invalid("Invalid HDR schema or dimensions"));
    }
    if recipe.output.depth == Depth::Rgb8
        && (recipe.output.encoding.transfer.hdr()
            || recipe.output.encoding.primaries != Primaries::Bt709)
    {
        return Err(invalid(
            "RGB8 output requires SDR BT.709 primaries; preserve HDR/wide gamut with RGB16",
        ));
    }
    let count = recipe.duration.units(FPS)?;
    if count == 0
        || count > 1500
        || count * recipe.width as u64 * recipe.height as u64 * 6 > RAW_LIMIT
    {
        return Err(invalid(
            "HDR output requires 1..1500 frames and at most 4 GiB raw RGB16",
        ));
    }
    recipe.rate.validate()?;
    recipe.source_in.validate()?;
    if recipe.rate.compare(Time { num: 1, den: 16 })?.is_lt()
        || recipe.rate.compare(Time { num: 16, den: 1 })?.is_gt()
        || (recipe.freeze
            && (recipe.reverse || recipe.rate.compare(Time { num: 1, den: 1 })?.is_ne()))
        || ((recipe.freeze || recipe.reverse) && !matches!(recipe.audio, Audio::Mute))
    {
        return Err(invalid(
            "Rate must be 1/16..16; freeze requires unit rate; freeze/reverse require mute",
        ));
    }
    let (path, bytes) = scene::identity_bytes(&recipe.source.file, root)?;
    drop(bytes);
    let checked = check_file(
        &path,
        recipe.source.encoding,
        recipe.source.matrix,
        recipe.source.range,
        recipe.source.missing_tags,
    )?;
    let mut selected = Vec::new();
    let end = Time {
        num: checked.frames,
        den: 25,
    };
    for n in 0..count {
        let step = if recipe.freeze {
            Time::ZERO
        } else {
            Time { num: n, den: 25 }.times(recipe.rate)?
        };
        let t = if recipe.reverse {
            recipe.source_in.minus(step)?
        } else {
            recipe.source_in.plus(step)?
        };
        if !t.compare(end)?.is_lt() {
            return Err(invalid("HDR frame selection exceeds source"));
        }
        selected.push(((t.num as u128 * 25) / t.den as u128) as usize);
    }
    if matches!(recipe.audio, Audio::Resample) {
        recipe.source_in.units(Time { num: 48000, den: 1 })?;
        if recipe
            .source_in
            .plus(recipe.duration.times(recipe.rate)?)?
            .compare(end)?
            .is_gt()
        {
            return Err(invalid("Mapped audio interval exceeds HDR source"));
        }
    }
    let mut args = input_args(&path);
    args.extend(["-map", "0", "-f", "null", "-"].map(str::to_owned));
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    scene::identity_bytes(&recipe.source.file, root)?;
    Ok((checked, selected, processor))
}
fn report(recipe: &Recipe, source: &Checked, selected: &[usize]) -> Value {
    json!({"profile":"hdr-conform-v1","source":recipe.source,"source_metadata":source.metadata,"assumed_tags":source.assumed,"output_profile":recipe.output,"source_frame_indices":selected,"frames":selected.len(),"samples":selected.len()*1920,"width":recipe.width,"height":recipe.height,"duration":recipe.duration,"tone":recipe.tone,"exposure_milliev":recipe.exposure_milliev,"precision":"native_10_12_16bit_to_f64_absolute_nits_then_single_output_quantization","gamut":"D65_primary_matrix_then_negative_clip","yuv_reconstruction":"native_444_no_subsampling","resize":"nearest_top_left","display_metadata":"declared_not_measured;standard_Matroska_mastering_fields_for_HDR_output","source_peak_policy":"explicit_tone_parameter_never_inferred_from_content_light_metadata","audio":recipe.audio})
}
pub fn inspect(recipe: &Recipe, root: &Path) -> Result<Value> {
    let (source, selected, _) = prepare(recipe, root)?;
    Ok(report(recipe, &source, &selected))
}
/// SHA-256 of `bytes` decoded raw video (`pixel` format) or PCM16 audio, hashed in Rust
/// (crate::digest) rather than by FFmpeg's much slower hash muxer.
fn decoded_hash(path: &Path, pixel: Option<&str>, bytes: u64) -> Result<String> {
    let mut args = input_args(path);
    if let Some(fmt) = pixel {
        args.extend(
            [
                "-map", "0:v:0", "-an", "-pix_fmt", fmt, "-c:v", "rawvideo", "-f", "rawvideo", "-",
            ]
            .map(str::to_owned),
        );
    } else {
        args.extend(
            [
                "-map",
                "0:a:0",
                "-vn",
                "-c:a",
                "pcm_s16le",
                "-f",
                "s16le",
                "-",
            ]
            .map(str::to_owned),
        );
    }
    crate::digest::raw_sha256(&args, bytes, Duration::from_secs(180))
}
pub fn run(recipe: &Recipe, root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let output = render::destination_extension(output, output_root, "mkv")?;
    let (source, selected, processor) = prepare(recipe, root)?;
    let scratch = scene::Scratch::new(output.parent().expect("destination"))?;
    let src = scratch.0.join("source.rgb");
    let snd = scratch.0.join("source.pcm");
    let mut args = input_args(&source.path);
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            &source.pix_fmt,
            "-f",
            "rawvideo",
        ]
        .map(str::to_owned),
    );
    args.push(src.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    let n = source.width as usize * source.height as usize;
    let stride = source.layout.stride(n);
    if src.metadata()?.len() != source.frames * stride as u64 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Decoded HDR input count differs",
        ));
    }
    let mut decoded = Vec::new();
    if matches!(recipe.audio, Audio::Resample) {
        let mut args = input_args(&source.path);
        args.extend(
            ["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "s16le"].map(str::to_owned),
        );
        args.push(snd.to_string_lossy().into_owned());
        media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
        if snd.metadata()?.len() != source.frames * 1920 * 4 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Decoded HDR audio count differs",
            ));
        }
        decoded = fs::read(&snd)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|v| i16::from_le_bytes(*v))
            .collect::<Vec<_>>();
    }
    let raw_video = scratch.0.join("video.rgb");
    let raw_audio = scratch.0.join("audio.pcm");
    let mut input = File::open(&src)?;
    let mut video = BufWriter::new(File::create_new(&raw_video)?);
    let mut native = vec![0; stride];
    let bits = if recipe.output.depth == Depth::Rgb16 {
        16
    } else {
        8
    };
    let maximum = ((1u32 << bits) - 1) as f64;
    let mut clipped = 0u64;
    for index in &selected {
        input.seek(SeekFrom::Start(*index as u64 * stride as u64))?;
        input.read_exact(&mut native)?;
        for y in 0..recipe.height as usize {
            for x in 0..recipe.width as usize {
                let p = (y * source.height as usize / recipe.height as usize)
                    * source.width as usize
                    + x * source.width as usize / recipe.width as usize;
                let rgb = processor.sample(source.layout.rgb(
                    &native,
                    p,
                    n,
                    recipe.source.matrix,
                    recipe.source.range,
                ));
                if rgb.iter().any(|v| *v < -1e-9 || *v > 1.0 + 1e-9) {
                    clipped += 1;
                }
                for v in rgb {
                    let code = (v.clamp(0.0, 1.0) * maximum).round() as u16;
                    if bits == 16 {
                        video.write_all(&code.to_le_bytes())?;
                    } else {
                        video.write_all(&[code as u8])?;
                    }
                }
            }
        }
    }
    video.flush()?;
    drop(video);
    drop(input);
    let mut audio = BufWriter::new(File::create_new(&raw_audio)?);
    let start = if decoded.is_empty() {
        0
    } else {
        recipe.source_in.units(Time { num: 48000, den: 1 })?
    };
    for i in 0..selected.len() as u64 * 1920 {
        for c in 0..2 {
            let value = if decoded.is_empty() {
                0
            } else {
                let pos = i as u128 * recipe.rate.num as u128;
                let den = recipe.rate.den as u128;
                let a = start + (pos / den) as u64;
                let b = (a + 1).min(source.frames * 1920 - 1);
                let f = (pos % den) as i128;
                let mix = decoded[(a * 2 + c) as usize] as i128 * (den as i128 - f)
                    + decoded[(b * 2 + c) as usize] as i128 * f;
                ((mix.abs() + den as i128 / 2) / den as i128 * mix.signum()) as i16
            };
            audio.write_all(&value.to_le_bytes())?;
        }
    }
    audio.flush()?;
    drop(audio);
    let temp = scratch.0.join("output.mkv");
    let pixel = if bits == 16 { "rgb48le" } else { "rgb24" };
    let encoded = if bits == 16 { "gbrp16le" } else { "bgr0" };
    let target = recipe.output.encoding;
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(recipe.width, recipe.height);
    let mut args = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-n".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        pixel.into(),
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
    ];
    args.extend(
        [
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-vf",
            "setsar=1",
            "-c:v",
            "ffv1",
            "-level",
            ffv1_level,
            "-slices",
            ffv1_slices,
            "-pix_fmt",
            encoded,
            "-threads",
            "1",
            "-c:a",
            "pcm_s16le",
            "-map_metadata",
            "-1",
            "-colorspace",
            "rgb",
            "-color_range",
            "pc",
            "-color_primaries",
            target.primaries.tag(),
            "-color_trc",
            target.transfer.tag(),
            "-write_crc32",
            "0",
        ]
        .map(str::to_owned),
    );
    if target.transfer.hdr() {
        args.extend([
            "-metadata:s:v:0".into(),
            format!("title={}", hdr_metadata::reservation()),
        ]);
    }
    args.push(temp.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    if target.transfer.hdr() {
        hdr_metadata::write(&temp, target)?;
    }
    let checked = check_file(&temp, target, Matrix::Rgb, Range::Full, MissingTags::Reject)?;
    if checked.width != recipe.width
        || checked.height != recipe.height
        || checked.frames != selected.len() as u64
        || checked.pix_fmt != encoded
        || decoded_hash(&temp, Some(pixel), fs::metadata(&raw_video)?.len())?
            != media::file_hash(&raw_video)?
        || decoded_hash(&temp, None, fs::metadata(&raw_audio)?.len())?
            != media::file_hash(&raw_audio)?
    {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "HDR output metadata, count or decoded content differs",
        ));
    }
    if target.transfer.hdr() {
        let v = checked.metadata["streams"]
            .as_array()
            .expect("checked streams")
            .iter()
            .find(|v| v["codec_type"] == "video")
            .expect("video");
        let display = v["side_data_list"].as_array().and_then(|items| {
            items
                .iter()
                .find(|v| v["side_data_type"] == "Mastering display metadata")
        });
        let expected: Vec<_> = target
            .primaries
            .xy()
            .into_iter()
            .flatten()
            .chain([
                0.3127,
                0.3290,
                target.display.peak_nits as f64,
                target.display.black_millinits as f64 / 1000.0,
            ])
            .collect();
        let fields = [
            "red_x",
            "red_y",
            "green_x",
            "green_y",
            "blue_x",
            "blue_y",
            "white_point_x",
            "white_point_y",
            "max_luminance",
            "min_luminance",
        ];
        if !display.is_some_and(|d| {
            fields
                .iter()
                .zip(&expected)
                .all(|(k, e)| rational_float(&d[*k]).is_some_and(|v| (v - e).abs() < 0.00011))
        }) {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "HDR display metadata missing from encoded output",
            ));
        }
    }
    let mut result = report(recipe, &source, &selected);
    let hash = media::file_hash(&temp)?;
    result["output"] = json!(output);
    result["sha256"] = json!(hash);
    result["clipped_output_pixels"] = json!(clipped);
    result["output_metadata"] = checked.metadata;
    result["output_metadata"]["format"]["filename"] = json!(output);
    result["ffmpeg"] = json!(media::version("ffmpeg")?);
    result["ffprobe"] = json!(media::version("ffprobe")?);
    if bits == 8 {
        result["asset"] = json!({"id":recipe.id,"path":output,"duration":recipe.duration,"identity":{"sha256":hash,"bytes":temp.metadata()?.len()}});
    }
    result["media_identity"] = json!({"sha256":hash,"bytes":temp.metadata()?.len()});
    scene::identity_bytes(&recipe.source.file, root)?;
    media::publish(&temp, &output)?;
    Ok(result)
}
pub fn capabilities() -> Value {
    json!({"profile":"hdr-conform-v1","input":"FFV1/PCM16_stereo_MKV","native_bits":[10,12,16],"input_layouts":["planar_rgb","yuv444"],"sdr_rgb8_input":true,"transfers":["pq","hlg","srgb","bt709"],"primaries":["bt709","bt2020"],"output_bits":[8,16],"hdr_requires_rgb16":true,"tone_mapping":["preserve","clip","reinhard"],"maximum_seconds":60,"maximum_decoded_bytes":RAW_LIMIT,"timeline_asset":"SDR_BT709_primaries_RGB8_only","hdr_intermediate":"tagged_RGB16_with_standard_declared_display_metadata","network":false})
}
