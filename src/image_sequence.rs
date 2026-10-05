//! Original bounded numbered-footage compilation, with explicit alpha and exact native clocks.
use crate::color::Transfer;
use crate::composite::{self, AlphaMode, BlendMode};
use crate::{
    Result, error, media, render,
    scene::{self, Identity},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{BufWriter, Cursor, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Output profile: `rgba_ffv1` (straight RGBA FFV1 `.mkv`), `rgba_png_mov` (straight RGBA PNG `.mov`), `reference_rgb` (FFV1/bgr0 `.mkv` flattened over `background`; returns a timeline asset). All include silent 48 kHz stereo PCM16.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    RgbaFfv1,
    RgbaPngMov,
    ReferenceRgb,
}
/// One numbered PNG source image.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// Image number; must equal `first_number` plus this entry's index.
    pub number: u32,
    /// PNG file identity; path relative to `input_root`.
    pub image: Identity,
}
/// Numbered PNG sequence recipe for image.sequence.inspect and image.sequence.compile.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    /// Recipe format version; must be 1.
    pub schema_version: u32,
    /// Asset ID for the asset returned by `reference_rgb` (1-128 bytes).
    pub id: String,
    /// Number of the first image; the last number must stay below 1000000.
    pub first_number: u32,
    /// Complete ordered source list, 1 to 1024 same-size PNGs; all are validated.
    pub frames: Vec<Frame>,
    /// Output frame rate as a rational; one of the supported native rates.
    pub frame_rate: Time,
    /// Index in `frames` of the first image in the half-open selected window.
    pub source_start: u32,
    /// Number of images in the selected window; at least 1 and within `frames`.
    pub source_count: u32,
    /// Times to play the window; at least 1, at most 180000 output frames in total.
    pub repeat: u32,
    /// Declared transfer of the PNG RGB values; sRGB-tagged PNGs reject `bt709`.
    pub input_transfer: Transfer,
    /// Whether source RGB is straight or premultiplied by alpha.
    pub alpha_mode: AlphaMode,
    /// Output container, codec and alpha handling.
    pub profile: Profile,
    /// Opaque `[r, g, b]` background (0-255); required for `reference_rgb`, omitted otherwise.
    #[serde(default)]
    pub background: Option<[u8; 3]>,
}
struct Prepared {
    pixels: Vec<Vec<u8>>,
    width: u32,
    height: u32,
    rate: Time,
    frames: u64,
    samples: u64,
    report: Value,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_IMAGE_SEQUENCE", message)
}
fn prepare(recipe: &Recipe, root: &Path) -> Result<Prepared> {
    media::input_root(root)?;
    if recipe.schema_version != 1
        || recipe.id.is_empty()
        || recipe.id.len() > 128
        || recipe.frames.is_empty()
        || recipe.frames.len() > 1024
        || recipe.source_count == 0
        || recipe.repeat == 0
        || recipe.source_start as u64 + recipe.source_count as u64 > recipe.frames.len() as u64
        || recipe.first_number as u64 + recipe.frames.len() as u64 > 1_000_000
    {
        return Err(invalid(
            "Version 1 needs an id, 1..1024 complete numbered PNG frames and a positive selected source window/repeat",
        ));
    }
    if (recipe.profile == Profile::ReferenceRgb) != recipe.background.is_some() {
        return Err(invalid(
            "reference_rgb requires an explicit opaque background; transparent profiles must omit it",
        ));
    }
    let rate = render::clock::rate(recipe.frame_rate)?;
    let frames = recipe.source_count as u64 * recipe.repeat as u64;
    if frames > 180_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Image compilation supports at most 180000 output frames",
        ));
    }
    let duration = Time::new(frames * rate.den, rate.num)?;
    let samples = duration.units(Time::new(48000, 1)?)?;
    let mut pixels = Vec::new();
    let mut dimensions = None;
    let mut encoded = 0u64;
    let mut decoded = 0u64;
    for (index, frame) in recipe.frames.iter().enumerate() {
        if frame.number as u64 != recipe.first_number as u64 + index as u64 {
            return Err(invalid(
                "Every expected frame number must occur once in order",
            ));
        }
        encoded = encoded
            .checked_add(frame.image.bytes)
            .ok_or_else(|| invalid("Image size overflow"))?;
        if encoded > 64 * 1024 * 1024 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "The complete encoded source list must fit 64 MiB",
            ));
        }
        let (_, bytes) = scene::identity_bytes(&frame.image, root)?;
        let decoder = png::Decoder::new(Cursor::new(&bytes));
        let reader = decoder
            .read_info()
            .map_err(|e| error("UNSUPPORTED_IMAGE", e.to_string()))?;
        if reader.info().srgb.is_some() && matches!(recipe.input_transfer, Transfer::Bt709) {
            return Err(invalid(
                "An sRGB-tagged PNG conflicts with declared BT.709 interpretation",
            ));
        }
        let image = scene::decode_png(&bytes)?;
        if dimensions.is_some_and(|d| d != (image.width, image.height)) {
            return Err(invalid(
                "All numbered source frames must have matching dimensions",
            ));
        }
        dimensions = Some((image.width, image.height));
        decoded += image.rgba.len() as u64;
        if decoded > 64 * 1024 * 1024 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "The complete decoded source list must fit 64 MiB",
            ));
        }
        composite::validate_alpha(&image.rgba, recipe.alpha_mode)?;
        let mut output = Vec::with_capacity(image.rgba.len());
        for pixel in image.rgba.as_chunks::<4>().0 {
            if let Some(background) = recipe.background {
                for c in 0..3 {
                    output.push(composite::channel(
                        background[c],
                        pixel[c],
                        pixel[3],
                        255,
                        BlendMode::Normal,
                        recipe.alpha_mode,
                    ));
                }
            } else {
                for channel in &pixel[..3] {
                    output.push(if recipe.alpha_mode.is_straight() {
                        *channel
                    } else if pixel[3] == 0 {
                        0
                    } else {
                        ((*channel as u32 * 255 + pixel[3] as u32 / 2) / pixel[3] as u32) as u8
                    });
                }
                output.push(pixel[3]);
            }
        }
        pixels.push(output);
    }
    let (width, height) = dimensions.expect("nonempty frames");
    let raw_bytes =
        frames * width as u64 * height as u64 * if recipe.background.is_some() { 3 } else { 4 }
            + samples * 4;
    if raw_bytes > 2 * 1024 * 1024 * 1024 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Video and silent PCM staging together must fit 2 GiB",
        ));
    }
    let report = json!({"profile":recipe.profile,"schema_version":1,"recipe_sha256":format!("{:x}",Sha256::digest(serde_json::to_vec(recipe)?)),
        "frame_rate":rate,"frames":frames,"duration":duration,"samples":samples,"width":width,"height":height,"first_number":recipe.first_number,
        "source_start":recipe.source_start,"source_count":recipe.source_count,"repeat":recipe.repeat,"sources":recipe.frames,
        "input_transfer":recipe.input_transfer,"source_alpha_mode":recipe.alpha_mode,"output_alpha":if recipe.background.is_some(){"opaque"}else{"straight"},
        "alpha_conversion":if recipe.alpha_mode.is_straight(){"none"}else if recipe.background.is_some(){"premultiplied_over_declared_background_single_round"}else{"unassociate_to_straight_nearest_integer_zero_alpha_rgb_zero"},
        "background":recipe.background,"audio_policy":"explicit_silence_48000_hz_stereo_pcm16","raw_staging_bytes":raw_bytes,"complete_sequence_validated":true,"native_timeline_compatible":recipe.profile==Profile::ReferenceRgb});
    Ok(Prepared {
        pixels,
        width,
        height,
        rate,
        frames,
        samples,
        report,
    })
}
pub fn inspect(recipe: &Recipe, input_root: &Path) -> Result<Value> {
    Ok(prepare(recipe, input_root)?.report)
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        for name in ["video.raw", "audio.pcm", "output.mkv", "output.mov"] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
pub fn run(recipe: &Recipe, input_root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let extension = if recipe.profile == Profile::RgbaPngMov {
        "mov"
    } else {
        "mkv"
    };
    let output = render::destination_extension(output, output_root, extension)?;
    let mut p = prepare(recipe, input_root)?;
    let path = output.parent().expect("validated parent").join(format!(
        ".cutbolt-images-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| invalid("Clock before epoch"))?
            .as_nanos()
    ));
    fs::create_dir(&path)?;
    let scratch = Scratch(path);
    let mut video = BufWriter::new(File::create_new(scratch.0.join("video.raw"))?);
    let mut expected = Sha256::new();
    for n in 0..p.frames {
        let index = recipe.source_start as usize + (n % recipe.source_count as u64) as usize;
        video.write_all(&p.pixels[index])?;
        expected.update(&p.pixels[index]);
    }
    video.flush()?;
    drop(video);
    let mut audio = BufWriter::new(File::create_new(scratch.0.join("audio.pcm"))?);
    let zeros = [0u8; 65536];
    let mut remaining = p.samples * 4;
    while remaining > 0 {
        let count = remaining.min(zeros.len() as u64) as usize;
        audio.write_all(&zeros[..count])?;
        remaining -= count as u64;
    }
    audio.flush()?;
    drop(audio);
    let temp = scratch.0.join(format!("output.{extension}"));
    let transparent = recipe.profile != Profile::ReferenceRgb;
    let transfer = recipe.input_transfer.tag();
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-f",
        "rawvideo",
        "-pixel_format",
        if transparent { "rgba" } else { "rgb24" },
        "-video_size",
    ]
    .map(str::to_owned)
    .to_vec();
    args.extend([
        format!("{}x{}", p.width, p.height),
        "-framerate".into(),
        format!("{}/{}", p.rate.num, p.rate.den),
        "-i".into(),
        scratch.0.join("video.raw").to_string_lossy().into_owned(),
        "-f".into(),
        "s16le".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-i".into(),
        scratch.0.join("audio.pcm").to_string_lossy().into_owned(),
    ]);
    args.extend(
        [
            "-map",
            "0:v:0",
            "-map",
            "1:a:0",
            "-c:a",
            "pcm_s16le",
            "-threads",
            "1",
            "-color_range",
            "pc",
            "-colorspace",
            "rgb",
            "-color_primaries",
            "bt709",
            "-color_trc",
            transfer,
        ]
        .map(str::to_owned),
    );
    if recipe.profile == Profile::RgbaPngMov {
        args.extend([
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "rgba".into(),
            "-movflags".into(),
            "+faststart+write_colr".into(),
            "-movie_timescale".into(),
            "48000".into(),
            "-video_track_timescale".into(),
            (p.rate.num * 512).to_string(),
            "-f".into(),
            "mov".into(),
        ]);
    } else {
        let (level, slices) = media::ffv1_encoding(p.width, p.height);
        args.extend(
            [
                "-c:v",
                "ffv1",
                "-level",
                level,
                "-slices",
                slices,
                "-pix_fmt",
                if transparent { "bgra" } else { "bgr0" },
                "-f",
                "matroska",
            ]
            .map(str::to_owned),
        );
    }
    args.push(temp.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(600))?;
    let metadata = media::probe(&temp)?;
    let container = metadata["format"]["format_name"].as_str().unwrap_or("");
    if !container.split(',').any(|s| {
        s == if recipe.profile == Profile::RgbaPngMov {
            "mov"
        } else {
            "matroska"
        }
    }) {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Unexpected sequence container",
        ));
    }
    let streams = metadata["streams"]
        .as_array()
        .ok_or_else(|| invalid("Missing output streams"))?;
    let video = streams
        .iter()
        .find(|s| s["codec_type"] == "video")
        .ok_or_else(|| invalid("Missing output video"))?;
    let audio = streams
        .iter()
        .find(|s| s["codec_type"] == "audio")
        .ok_or_else(|| invalid("Missing output audio"))?;
    if streams.len() != 2
        || video["width"] != p.width
        || video["height"] != p.height
        || !render::clock::rate_hint(video, &metadata, p.rate)
        || video["codec_name"]
            != if recipe.profile == Profile::RgbaPngMov {
                "png"
            } else {
                "ffv1"
            }
        || video["pix_fmt"]
            != if recipe.profile == Profile::RgbaPngMov {
                "rgba"
            } else if transparent {
                "bgra"
            } else {
                "bgr0"
            }
        || video["color_range"] != "pc"
        || video["color_space"] != "gbr"
        || video["color_primaries"] != "bt709"
        || video["color_transfer"] != transfer
        || audio["codec_name"] != "pcm_s16le"
        || audio["sample_rate"] != "48000"
        || audio["channels"] != 2
    {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Sequence codec, color, alpha, geometry or sample format differs",
        ));
    }
    let info = media::frame_info_controlled(&temp, "v:0", &media::Uncontrolled)?;
    let frames = info["frames"]
        .as_array()
        .ok_or_else(|| invalid("Missing decoded frames"))?;
    if frames.len() as u64 != p.frames {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Image sequence frame count differs",
        ));
    }
    for (n, frame) in frames.iter().enumerate() {
        render::clock::timestamp(frame, n, p.rate, video)?;
    }
    if recipe.profile == Profile::RgbaPngMov {
        let duration = Time::new(p.frames * p.rate.den, p.rate.num)?;
        for stream in [video, audio] {
            if stream["start_pts"] != 0
                || crate::delivery::exact_time(stream, "duration_ts")?
                    .compare(duration)?
                    .is_ne()
            {
                return Err(error(
                    "RENDER_VALIDATION_FAILED",
                    "Movie track duration differs from exact source clock",
                ));
            }
        }
    }
    let audio_info = media::frame_info_controlled(&temp, "a:0", &media::Uncontrolled)?;
    let mut samples = 0u64;
    for frame in audio_info["frames"]
        .as_array()
        .ok_or_else(|| invalid("Missing decoded audio"))?
    {
        let expected = (samples as u128 * 1_000_000 / 48_000) as i64;
        if (render::micros(&frame["best_effort_timestamp_time"])? - expected).abs() > 1000 {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Sequence audio clock is discontinuous",
            ));
        }
        samples = samples
            .checked_add(
                frame["nb_samples"]
                    .as_u64()
                    .ok_or_else(|| invalid("Missing audio sample count"))?,
            )
            .ok_or_else(|| invalid("Audio count overflow"))?;
    }
    if samples != p.samples {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Sequence audio duration differs",
        ));
    }
    let digest = decoded_hash(
        &temp,
        "0:v:0",
        if transparent { "rgba" } else { "rgb24" },
        fs::metadata(scratch.0.join("video.raw"))?.len(),
    )?;
    if digest != format!("{:x}", expected.finalize()) {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Lossless sequence RGBA/RGB pixels changed",
        ));
    }
    let expected_audio = media::file_hash(&scratch.0.join("audio.pcm"))?;
    let audio_bytes = fs::metadata(scratch.0.join("audio.pcm"))?.len();
    if decoded_hash(&temp, "0:a:0", "s16le", audio_bytes)? != expected_audio {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Silent soundtrack count or samples changed",
        ));
    }
    for frame in &recipe.frames {
        scene::identity_bytes(&frame.image, input_root)?;
    }
    p.report["output"] = json!(output);
    p.report["sha256"] = json!(media::file_hash(&temp)?);
    p.report["decoded_pixels_sha256"] = json!(digest);
    p.report["decoded_pcm_sha256"] = json!(expected_audio);
    p.report["ffmpeg"] = json!(media::version("ffmpeg")?);
    p.report["ffprobe"] = json!(media::version("ffprobe")?);
    if recipe.profile == Profile::ReferenceRgb {
        p.report["asset"] = json!({"id":recipe.id,"path":output,"duration":p.report["duration"],"identity":{"bytes":fs::metadata(&temp)?.len(),"sha256":p.report["sha256"]}});
    }
    File::options().write(true).open(&temp)?.sync_all()?;
    media::publish(&temp, &output)?;
    Ok(p.report)
}
/// SHA-256 of `bytes` decoded raw bytes of one stream, hashed in Rust (crate::digest) rather than
/// by FFmpeg's much slower hash muxer.
fn decoded_hash(path: &Path, stream: &str, format: &str, bytes: u64) -> Result<String> {
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(path.to_string_lossy().into_owned());
    args.extend(["-map".into(), stream.into()]);
    if stream == "0:v:0" {
        args.extend(
            [
                "-an",
                "-fps_mode",
                "passthrough",
                "-c:v",
                "rawvideo",
                "-pix_fmt",
                format,
            ]
            .map(str::to_owned),
        );
    } else {
        args.extend(["-vn", "-c:a", "pcm_s16le"].map(str::to_owned));
    }
    args.extend(
        [
            "-f",
            if stream == "0:v:0" {
                "rawvideo"
            } else {
                "s16le"
            },
            "-",
        ]
        .map(str::to_owned),
    );
    crate::digest::raw_sha256(&args, bytes, Duration::from_secs(600))
}

pub fn capabilities() -> Value {
    json!({"recipe_version":1,"profiles":["rgba_ffv1","rgba_png_mov","reference_rgb"],"maximum_source_frames":1024,
        "maximum_source_dimension":512,"maximum_encoded_source_bytes":67108864,"maximum_decoded_source_bytes":67108864,
        "maximum_output_frames":180000,"maximum_raw_staging_bytes":2147483648u64,"frame_rates":"native_sequential_frame_rates",
        "audio":"explicit_48000_hz_stereo_pcm16_silence","source_alpha":["straight","premultiplied"],"transparent_output_alpha":"straight",
        "input_transfer":["srgb","bt709"],"native_timeline_compatible_profiles":["reference_rgb"],"compile":"blocking_CLI_or_library"})
}
