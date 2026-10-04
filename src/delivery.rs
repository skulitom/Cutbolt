//! Explicit range/stream export with verified reference and bounded delivery profiles.
use crate::{Result, error, media, model::Project, render, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const FPS: Time = Time { num: 25, den: 1 };
mod sequence;
mod settings;
pub use settings::{Compatibility, H264, RateControl};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Reference,
    H264Aac,
    PngMov,
    PngSequence,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Streams {
    AudioVideo,
    Video,
    Audio,
}
impl Streams {
    fn video(self) -> bool {
        self != Self::Audio
    }
    fn audio(self) -> bool {
        self != Self::Video
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Transfer {
    Srgb,
    Bt709,
}
impl Transfer {
    fn filter(self) -> String {
        let matrix = "scale=in_range=pc:out_range=tv:out_color_matrix=bt709:out_h_chr_pos=0:out_v_chr_pos=128:flags=bilinear+accurate_rnd,format=yuv420p";
        match self {
            Self::Bt709 => format!("format=rgb24,{matrix}"),
            Self::Srgb => {
                // Explicit public transfer equations; quantize the converted encoded RGB to 8 bits.
                let linear =
                    "if(lte(val/255,0.04045),val/255/12.92,pow((val/255+0.055)/1.055,2.4))";
                let transfer = format!(
                    "floor(255*if(lt({linear},0.018),4.5*{linear},1.099*pow({linear},0.45)-0.099)+0.5)"
                );
                format!("format=rgb24,lutrgb=r='{transfer}':g='{transfer}':b='{transfer}',{matrix}")
            }
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Range {
    pub start: Time,
    pub duration: Time,
}
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Export {
    pub project: Project,
    pub input_root: PathBuf,
    pub output_root: PathBuf,
    pub output: PathBuf,
    pub profile: Profile,
    pub streams: Streams,
    #[serde(default)]
    pub range: Option<Range>,
    #[serde(default)]
    pub input_transfer: Option<Transfer>,
    #[serde(default)]
    pub h264: Option<H264>,
    #[serde(default)]
    pub aac_bitrate: Option<u32>,
    #[serde(default)]
    pub sequence_first: Option<u32>,
}
struct Checked {
    project: Project,
    range: Range,
    output: PathBuf,
    reference: render::Plan,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_EXPORT", message)
}
fn verification(message: &str) -> crate::Error {
    error("RENDER_VALIDATION_FAILED", message)
}
fn nonce() -> Result<String> {
    Ok(format!(
        "{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
            .as_nanos()
    ))
}
impl Export {
    fn extension(&self) -> &'static str {
        match (self.profile, self.streams) {
            (Profile::Reference, Streams::Audio) => "wav",
            (Profile::Reference, _) => "mkv",
            (Profile::H264Aac, Streams::Audio) => "m4a",
            (Profile::H264Aac, _) => "mp4",
            (Profile::PngMov, _) => "mov",
            (Profile::PngSequence, _) => "frames",
        }
    }
    fn selected(&self) -> Result<(Project, Range)> {
        self.project.validate()?;
        if self.profile == Profile::H264Aac && self.project.frame_rate.compare(FPS)?.is_ne() {
            return Err(invalid(
                "H.264 delivery requires a 25 fps reference timeline",
            ));
        }
        let rate = render::clock::rate(self.project.frame_rate)?;
        let total = self.project.duration()?.units(rate)?;
        let range = self.range.unwrap_or(Range {
            start: Time::ZERO,
            duration: self.project.duration()?,
        });
        let begin = range.start.units(rate)?;
        let count = range.duration.units(rate)?;
        if count == 0 || count > 180000 || begin as u128 + count as u128 > total as u128 {
            return Err(invalid(
                "Export range must contain 1..180000 whole frames inside the timeline",
            ));
        }
        if matches!(self.profile, Profile::PngMov | Profile::PngSequence)
            && (!self.streams.video()
                || self.project.width > 4096
                || self.project.height > 4096
                || self.project.width as u64 * self.project.height as u64 > 8_847_360)
        {
            return Err(invalid(
                "PNG exports require video with dimensions at most 4096 and at most 8847360 pixels",
            ));
        }
        if self.profile == Profile::PngSequence {
            if !cfg!(windows) {
                return Err(error(
                    "UNSUPPORTED_PLATFORM",
                    "Atomic image directory publication currently requires Windows",
                ));
            }
            let first = self.sequence_first.unwrap_or(0) as u64;
            if first + count > 1_000_000 {
                return Err(invalid("Sequence numbers must fit six decimal digits"));
            }
        } else if self.sequence_first.is_some() {
            return Err(invalid("sequence_first requires png_sequence output"));
        }
        let color_needed = self.profile != Profile::Reference && self.streams.video();
        if color_needed != self.input_transfer.is_some() {
            return Err(invalid(
                "H.264 and PNG video require explicit input_transfer (srgb or bt709); reference/audio exports must omit it",
            ));
        }
        if self.h264.is_some() && !(self.profile == Profile::H264Aac && self.streams.video()) {
            return Err(invalid("h264 settings require H.264 video output"));
        }
        if self.profile == Profile::H264Aac && self.streams.video() {
            self.h264
                .unwrap_or_default()
                .validate(self.project.width, self.project.height)?;
        }
        if let Some(bitrate) = self.aac_bitrate
            && (self.profile != Profile::H264Aac
                || !self.streams.audio()
                || ![192000, 256000, 320000].contains(&bitrate))
        {
            return Err(invalid(
                "aac_bitrate requires AAC output and must be 192000, 256000 or 320000",
            ));
        }
        let mut selected = self.project.clone();
        selected.frame_rate = rate;
        selected.preview_scale = None;
        Ok((selected, range))
    }
    fn check(&self) -> Result<Checked> {
        let (project, range) = self.selected()?;
        let output =
            render::destination_extension(&self.output, &self.output_root, self.extension())?;
        let parent = output.parent().expect("validated output parent");
        // Plan the lossless intermediate without creating it or exposing a random path in the report.
        let virtual_output = parent.join(format!(".cutbolt-export-plan-{}.mkv", nonce()?));
        let reference = render::plan_range(
            &project,
            &self.input_root,
            parent,
            &virtual_output,
            range.start,
            range.duration,
        )?;
        Ok(Checked {
            project,
            range,
            output,
            reference,
        })
    }
}
fn report(request: &Export, c: &Checked) -> Value {
    json!({"profile":request.profile,"profile_version":if matches!(request.profile,Profile::PngMov|Profile::PngSequence) || c.project.frame_rate != FPS{3}else if request.h264.is_some() || request.aac_bitrate.is_some(){2}else{1},"streams":request.streams,"output":c.output,"range":c.range,"project_revision":c.project.revision,"source_quality":"original",
        "timeline_frames":c.reference.frames,"video_frames":if request.streams.video(){c.reference.frames}else{0},"audio_samples":if request.streams.audio(){c.reference.samples}else{0},
        "width":c.project.width,"height":c.project.height,"frame_rate":c.project.frame_rate,"input_transfer":request.input_transfer,"sources":c.reference.sources,
        "video":if !request.streams.video(){Value::Null}else if request.profile==Profile::Reference{json!({"codec":"ffv1","pixel_format":"bgr0","color":"encoded_values_preserved"})}else if matches!(request.profile,Profile::PngMov|Profile::PngSequence){json!({"codec":"png","pixel_format":"rgb24","alpha":"opaque","color":"encoded_values_preserved","transfer":request.input_transfer})}else{request.h264.unwrap_or_default().video_report()},
        "audio":if !request.streams.audio(){Value::Null}else if request.profile!=Profile::H264Aac{json!({"codec":"pcm_s16le","sample_rate":48000,"channels":2,"decoded_tail_padding":0})}else{json!({"codec":"aac","profile":"LC","bitrate":request.aac_bitrate.unwrap_or(320000),"sample_rate":48000,"channels":2,"presentation_samples":c.reference.samples,"decoder_tail_padding":"reported_separately_0_to_1023_samples"})},
        "encoder_passes":if request.profile==Profile::Reference{0}else if request.streams.video() && request.h264.unwrap_or_default().rate_control.two_pass(){2}else{1},
        "container":match request.profile { Profile::H264Aac=>"mp4",Profile::PngMov=>"mov",Profile::PngSequence=>"numbered_png_and_manifest",Profile::Reference=>if request.streams==Streams::Audio{"wav"}else{"matroska"}},"sequence_first":if request.profile==Profile::PngSequence{Some(request.sequence_first.unwrap_or(0))}else{None}})
}
pub fn inspect(request: &Export) -> Result<Value> {
    let c = request.check()?;
    Ok(report(request, &c))
}
struct Scratch(PathBuf);
impl Scratch {
    fn new(parent: &Path) -> Result<Self> {
        let path = parent.join(format!(".cutbolt-export-{}", nonce()?));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        for name in [
            "reference.mkv",
            "encoded.mkv",
            "encoded.mp4",
            "encoded.m4a",
            "encoded.wav",
            "encoded.mov",
            "decoded.pcm",
            "reference.pcm",
            "rate-control-0.log",
            "rate-control-0.log.mbtree",
            "rate-control-0.log.temp",
            "rate-control-0.log.mbtree.temp",
        ] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}
fn arguments(
    request: &Export,
    input: &Path,
    output: &Path,
    pass: Option<u8>,
    scratch: &Path,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "-hide_banner",
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(input.to_string_lossy().into_owned());
    args.extend(
        [
            "-map_metadata",
            "-1",
            "-map_chapters",
            "-1",
            "-filter_threads",
            "1",
        ]
        .map(str::to_owned),
    );
    if request.streams.video() {
        args.extend(["-map", "0:v:0"].map(str::to_owned));
    } else {
        args.push("-vn".into());
    }
    if request.streams.audio() && pass != Some(1) {
        args.extend(["-map", "0:a:0"].map(str::to_owned));
    } else {
        args.push("-an".into());
    }
    if request.profile == Profile::Reference {
        args.extend(
            [
                "-c",
                "copy",
                "-f",
                if request.streams == Streams::Audio {
                    "wav"
                } else {
                    "matroska"
                },
            ]
            .map(str::to_owned),
        );
    } else if request.profile == Profile::PngMov {
        let rate = render::clock::rate(request.project.frame_rate).expect("validated native rate");
        args.extend([
            "-vf".into(),
            format!("settb={}/{},setpts=N", rate.den, rate.num),
            "-r".into(),
            format!("{}/{}", rate.num, rate.den),
            "-enc_time_base:v".into(),
            format!("{}/{}", rate.den, rate.num),
            "-fps_mode".into(),
            "cfr".into(),
            "-c:v".into(),
            "png".into(),
            "-pix_fmt".into(),
            "rgb24".into(),
            "-threads".into(),
            "1".into(),
            "-c:a".into(),
            "pcm_s16le".into(),
            "-color_range".into(),
            "pc".into(),
            "-colorspace".into(),
            "rgb".into(),
            "-color_primaries".into(),
            "bt709".into(),
            "-color_trc".into(),
            match request.input_transfer.expect("validated transfer") {
                Transfer::Srgb => "iec61966-2-1",
                Transfer::Bt709 => "bt709",
            }
            .into(),
            "-movflags".into(),
            "+faststart+write_colr".into(),
            "-movie_timescale".into(),
            "48000".into(),
            "-video_track_timescale".into(),
            rate.num.to_string(),
            "-f".into(),
            "mov".into(),
        ]);
    } else {
        if request.streams.video() {
            args.extend([
                "-vf".into(),
                request.input_transfer.expect("validated transfer").filter(),
            ]);
            args.extend(
                [
                    "-c:v",
                    "libx264",
                    "-preset",
                    "medium",
                    "-g",
                    "50",
                    "-keyint_min",
                    "25",
                    "-sc_threshold",
                    "0",
                    "-pix_fmt",
                    "yuv420p",
                    "-color_range",
                    "tv",
                    "-colorspace",
                    "bt709",
                    "-color_primaries",
                    "bt709",
                    "-color_trc",
                    "bt709",
                    "-chroma_sample_location",
                    "left",
                    "-threads",
                    "1",
                ]
                .map(str::to_owned),
            );
            request
                .h264
                .unwrap_or_default()
                .arguments(&mut args, pass, scratch);
        }
        if request.streams.audio() && pass != Some(1) {
            args.extend(
                [
                    "-c:a",
                    "aac",
                    "-profile:a",
                    "aac_low",
                    "-aac_coder",
                    "twoloop",
                    "-aac_pns",
                    "1",
                    "-ar",
                    "48000",
                    "-ac",
                    "2",
                ]
                .map(str::to_owned),
            );
            args.extend([
                "-b:a".into(),
                request.aac_bitrate.unwrap_or(320000).to_string(),
            ]);
        }
        if pass == Some(1) {
            args.extend(["-f".into(), "null".into()]);
        } else {
            args.extend(
                [
                    "-movflags",
                    "+faststart",
                    "-use_editlist",
                    "1",
                    "-movie_timescale",
                    "48000",
                    "-video_track_timescale",
                    "12800",
                    "-f",
                    "mp4",
                ]
                .map(str::to_owned),
            );
        }
    }
    args.push(if pass == Some(1) {
        "-".into()
    } else {
        output.to_string_lossy().into_owned()
    });
    args
}
pub(crate) fn exact_time(value: &Value, ticks: &str) -> Result<Time> {
    let raw = value["time_base"]
        .as_str()
        .ok_or_else(|| verification("Missing stream time base"))?;
    let (n, d) = raw
        .split_once('/')
        .ok_or_else(|| verification("Malformed stream time base"))?;
    let tb = Time::new(
        n.parse().map_err(|_| verification("Invalid time base"))?,
        d.parse().map_err(|_| verification("Invalid time base"))?,
    )?;
    Time::new(
        value[ticks]
            .as_u64()
            .ok_or_else(|| verification("Missing or negative stream time"))?,
        1,
    )?
    .times(tb)
}
fn video_hash(path: &Path) -> Result<String> {
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
    args.extend(
        [
            "-map",
            "0:v:0",
            "-an",
            "-fps_mode",
            "passthrough",
            "-c:v",
            "rawvideo",
            "-pix_fmt",
            "rgb24",
            "-f",
            "hash",
            "-hash",
            "sha256",
            "-",
        ]
        .map(str::to_owned),
    );
    let result = String::from_utf8(media::capture(
        &media::tool("ffmpeg"),
        &args,
        Duration::from_secs(600),
    )?)
    .map_err(|_| verification("Invalid decoded digest"))?;
    let hash = result
        .trim()
        .strip_prefix("SHA256=")
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| verification("Missing decoded video digest"))?;
    Ok(hash.into())
}
fn decode_pcm(input: &Path, output: &Path) -> Result<u64> {
    let mut args: Vec<String> = [
        "-v",
        "error",
        "-xerror",
        "-nostdin",
        "-n",
        "-protocol_whitelist",
        "file,pipe",
        "-i",
    ]
    .map(str::to_owned)
    .to_vec();
    args.push(input.to_string_lossy().into_owned());
    args.extend(["-map", "0:a:0", "-vn", "-c:a", "pcm_s16le", "-f", "s16le"].map(str::to_owned));
    args.push(output.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(600))?;
    let bytes = fs::metadata(output)?.len();
    if !bytes.is_multiple_of(4) {
        return Err(verification("Incomplete stereo PCM sample"));
    }
    Ok(bytes / 4)
}
fn prefix_hash(path: &Path, bytes: u64) -> Result<String> {
    let mut input = File::open(path)?.take(bytes);
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    let mut read = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        read += count as u64;
    }
    if read != bytes {
        return Err(verification("Decoded PCM is shorter than the presentation"));
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn validate_output(
    request: &Export,
    c: &Checked,
    path: &Path,
    reference: &Path,
    scratch: &Path,
) -> Result<Value> {
    let metadata = media::probe(path)?;
    let streams = metadata["streams"]
        .as_array()
        .ok_or_else(|| verification("Missing output streams"))?;
    let wanted = request.streams.video() as usize + request.streams.audio() as usize;
    if streams.len() != wanted {
        return Err(verification("Unexpected output stream count"));
    }
    let container = metadata["format"]["format_name"].as_str().unwrap_or("");
    if !(if request.profile == Profile::H264Aac {
        container.split(',').any(|s| s == "mp4")
    } else if request.profile == Profile::PngMov {
        container.split(',').any(|s| s == "mov")
    } else if request.streams == Streams::Audio {
        container == "wav"
    } else {
        container.split(',').any(|s| s == "matroska")
    }) {
        return Err(verification("Unexpected output container"));
    }
    let mut result = json!({"decoded_video_sha256":null,"decoded_audio_prefix_sha256":null,"decoded_audio_samples":0,"audio_tail_padding_samples":0});
    if request.streams.video() {
        let v = streams
            .iter()
            .find(|s| s["codec_type"] == "video")
            .ok_or_else(|| verification("Missing video"))?;
        if v["width"] != c.project.width
            || v["height"] != c.project.height
            || !render::clock::rate_hint(v, &metadata, c.project.frame_rate)
        {
            return Err(verification("Output video geometry/rate mismatch"));
        }
        if request.profile == Profile::Reference {
            if v["codec_name"] != "ffv1" || v["pix_fmt"] != "bgr0" {
                return Err(verification("Unexpected reference video codec"));
            }
        } else if request.profile == Profile::PngMov {
            if v["codec_name"] != "png"
                || v["pix_fmt"] != "rgb24"
                || v["color_range"] != "pc"
                || v["color_space"] != "gbr"
                || v["color_primaries"] != "bt709"
                || v["color_transfer"]
                    != match request.input_transfer.expect("validated transfer") {
                        Transfer::Srgb => "iec61966-2-1",
                        Transfer::Bt709 => "bt709",
                    }
                || v["start_pts"] != 0
                || exact_time(v, "duration_ts")?
                    .compare(c.range.duration)?
                    .is_ne()
            {
                return Err(verification(
                    "Unexpected lossless MOV codec or exact duration",
                ));
            }
        } else if v["codec_name"] != "h264"
            || v["profile"]
                != request
                    .h264
                    .unwrap_or_default()
                    .compatibility
                    .decoded_profile()
            || v["level"] != request.h264.unwrap_or_default().compatibility.level().1
            || v["pix_fmt"] != "yuv420p"
            || v["color_range"] != "tv"
            || v["color_space"] != "bt709"
            || v["color_primaries"] != "bt709"
            || v["color_transfer"] != "bt709"
            || v["chroma_location"] != "left"
            || v["start_pts"] != 0
            || exact_time(v, "duration_ts")?.compare(c.range.duration)? != std::cmp::Ordering::Equal
        {
            return Err(verification(
                "Delivery video profile, color tags or exact duration mismatch",
            ));
        }
        let mut args: Vec<String> = [
            "-v",
            "error",
            "-protocol_whitelist",
            "file,pipe",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_entries",
            "frame=best_effort_timestamp,best_effort_timestamp_time",
            "-of",
            "json",
        ]
        .map(str::to_owned)
        .to_vec();
        args.push(path.to_string_lossy().into_owned());
        let info: Value = serde_json::from_slice(&media::capture(
            &media::tool("ffprobe"),
            &args,
            Duration::from_secs(600),
        )?)?;
        let frames = info["frames"]
            .as_array()
            .ok_or_else(|| verification("Missing decoded frames"))?;
        if frames.len() as u64 != c.reference.frames {
            return Err(verification("Decoded frame count mismatch"));
        }
        for (n, frame) in frames.iter().enumerate() {
            let mut timestamp = frame.clone();
            timestamp["time_base"] = v["time_base"].clone();
            if request.profile == Profile::Reference {
                render::clock::timestamp(frame, n, c.project.frame_rate, v)
                    .map_err(|_| verification("Decoded frame timestamp mismatch"))?;
            } else if exact_time(&timestamp, "best_effort_timestamp")?
                .compare(Time::new(
                    n as u64 * c.project.frame_rate.den,
                    c.project.frame_rate.num,
                )?)?
                .is_ne()
            {
                return Err(verification("Decoded frame timestamp mismatch"));
            }
        }
        let hash = video_hash(path)?;
        if request.profile != Profile::H264Aac && hash != video_hash(reference)? {
            return Err(verification(
                "Reference video samples changed during export",
            ));
        }
        result["decoded_video_sha256"] = json!(hash);
    }
    if request.streams.audio() {
        let a = streams
            .iter()
            .find(|s| s["codec_type"] == "audio")
            .ok_or_else(|| verification("Missing audio"))?;
        if a["sample_rate"] != "48000" || a["channels"] != 2 {
            return Err(verification("Unexpected audio rate/layout"));
        }
        if request.profile != Profile::H264Aac {
            if a["codec_name"] != "pcm_s16le" {
                return Err(verification("Unexpected reference audio codec"));
            }
        } else if a["codec_name"] != "aac"
            || a["profile"] != "LC"
            || a["start_pts"] != 0
            || exact_time(a, "duration_ts")?.compare(c.range.duration)? != std::cmp::Ordering::Equal
        {
            return Err(verification(
                "Delivery audio profile or exact presentation duration mismatch",
            ));
        }
        let decoded = scratch.join("decoded.pcm");
        let count = decode_pcm(path, &decoded)?;
        let samples = c.reference.samples;
        let expected = if request.profile != Profile::H264Aac {
            samples
        } else {
            samples.div_ceil(1024) * 1024
        };
        if count != expected {
            return Err(verification("Decoded audio count/padding mismatch"));
        }
        let hash = prefix_hash(&decoded, samples * 4)?;
        if request.profile != Profile::H264Aac {
            let original = scratch.join("reference.pcm");
            if decode_pcm(reference, &original)? != samples
                || hash != prefix_hash(&original, samples * 4)?
            {
                return Err(verification("Reference PCM changed during export"));
            }
        } else {
            let mut args: Vec<String> = [
                "-v",
                "error",
                "-protocol_whitelist",
                "file,pipe",
                "-select_streams",
                "a:0",
                "-read_intervals",
                "%+#1",
                "-show_packets",
                "-show_entries",
                "packet=pts,side_data_list",
                "-of",
                "json",
            ]
            .map(str::to_owned)
            .to_vec();
            args.push(path.to_string_lossy().into_owned());
            let packets: Value = serde_json::from_slice(&media::capture(
                &media::tool("ffprobe"),
                &args,
                Duration::from_secs(30),
            )?)?;
            let first = &packets["packets"][0];
            if first["pts"] != -1024
                || !first["side_data_list"].as_array().is_some_and(|v| {
                    v.iter()
                        .any(|d| d["side_data_type"] == "Skip Samples" && d["skip_samples"] == 1024)
                })
            {
                return Err(verification("AAC priming/edit metadata mismatch"));
            }
            result["aac_priming_samples"] = json!(1024);
        }
        result["decoded_audio_prefix_sha256"] = json!(hash);
        result["decoded_audio_samples"] = json!(count);
        result["audio_tail_padding_samples"] = json!(count - samples);
    }
    Ok(result)
}
pub fn run(request: &Export) -> Result<Value> {
    let c = request.check()?;
    let scratch = Scratch::new(c.output.parent().expect("validated output parent"))?;
    let reference = scratch.0.join("reference.mkv");
    render::run_range(
        &c.project,
        &request.input_root,
        &scratch.0,
        &reference,
        c.range.start,
        c.range.duration,
    )?;
    if request.profile == Profile::PngSequence {
        return sequence::run(request, &c, &reference, &scratch.0);
    }
    let encoded = scratch.0.join(format!("encoded.{}", request.extension()));
    let two_pass = request.profile == Profile::H264Aac
        && request.streams.video()
        && request.h264.unwrap_or_default().rate_control.two_pass();
    if two_pass {
        media::capture(
            &media::tool("ffmpeg"),
            &arguments(request, &reference, &encoded, Some(1), &scratch.0),
            Duration::from_secs(600),
        )?;
    }
    media::capture(
        &media::tool("ffmpeg"),
        &arguments(
            request,
            &reference,
            &encoded,
            two_pass.then_some(2),
            &scratch.0,
        ),
        Duration::from_secs(600),
    )?;
    let verified = validate_output(request, &c, &encoded, &reference, &scratch.0)?;
    for source in &c.reference.sources {
        if media::file_hash(&source.path)? != source.sha256 {
            return Err(error("MEDIA_CHANGED", "Source changed during export"));
        }
    }
    let mut result = report(request, &c);
    result["verification"] = verified;
    result["sha256"] = json!(media::file_hash(&encoded)?);
    result["ffmpeg"] = json!(media::version("ffmpeg")?);
    result["ffprobe"] = json!(media::version("ffprobe")?);
    media::publish(&encoded, &c.output)?;
    Ok(result)
}
pub fn capabilities() -> Value {
    json!({"profiles":["reference","h264_aac","png_mov","png_sequence"],"streams":["audio_video","video","audio"],"range":"exact_native_frame_and_sample_boundaries; h264_and_placed_tracks_25fps","maximum_frames":180000,"source_quality":"original","h264_maximum_dimensions":[1920,1080],"h264_even_dimensions":true,"h264_input_transfer_required":["srgb","bt709"],"h264_compatibility_profiles":["baseline720p","main_hd","high_hd"],"h264_rate_control":["quality","two_pass"],"aac_bitrates":[192000,256000,320000],"aac_presentation":"exact_track_duration_with_reported_decoder_tail_padding","runtime_dependencies":"external_ffmpeg_libx264_aac_lutrgb_scale_and_ffprobe","png":{"maximum_dimension":4096,"maximum_pixels":8847360,"pixel_format":"rgb24","alpha":"opaque","input_transfer_required":["srgb","bt709"],"sequence":{"platform":"windows","extension":"frames","number_digits":6,"first_number_default":0,"manifest":"complete_ordered_file_identities_and_exact_clock","publication":"complete_directory_without_replacement"}},"queued":false})
}
