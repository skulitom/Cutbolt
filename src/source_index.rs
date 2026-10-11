//! Compressed-source inspection. This index describes decoded presentation order; it does not
//! yet enable the timeline's direct decoder. Cache files and decoded PCM stay outside projects.
use crate::{
    Result, error, inspection_cache,
    media::{self, Control},
    registry::Identity,
    time::Time,
};
use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SCHEMA: u32 = 1;
const MAX_FRAMES: usize = 4_000_000;
const MAX_BYTES: u64 = 256 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(1800);

/// Native stream ticks, keyframe flag, picture type's ASCII byte, interlace flag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Frame(pub i64, pub bool, pub u8, pub bool);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Audio {
    pub rate: u64,
    pub channels: u64,
    pub samples: u64,
    /// First decoded PCM sample's PTS, in 1/rate seconds, with the original timestamps retained.
    pub first_pts: i64,
    pub continuous: bool,
    pub codec_initial_padding: Option<u64>,
    pub stream: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Index {
    pub source: Identity,
    pub video: Value,
    pub time_base: Time,
    pub rate: Option<Time>,
    pub frames: Vec<Frame>,
    pub audio: Option<Audio>,
    pub reasons: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    schema: u32,
    key: String,
    digest: String,
    index: Index,
}

fn invalid(message: impl Into<String>) -> crate::Error {
    error("UNSUPPORTED_MEDIA", message)
}

fn ratio(value: &Value) -> Result<Time> {
    let result: Time = serde_json::from_value(value.clone())?;
    result.validate()?;
    if result.num == 0 {
        return Err(invalid("Source index requires a positive time base"));
    }
    Ok(result)
}

/// Bound each tool record independently, rather than buffering a source-sized JSON document.
fn lines(reader: impl Read, mut consume: impl FnMut(&str) -> Result<()>) -> Result<()> {
    let mut reader = BufReader::new(reader);
    loop {
        let mut line = Vec::new();
        (&mut reader).take(8193).read_until(b'\n', &mut line)?;
        if line.is_empty() {
            return Ok(());
        }
        if line.len() > 8192 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Source index record exceeds 8192 bytes",
            ));
        }
        consume(
            std::str::from_utf8(&line)
                .map_err(|_| invalid("Non-UTF-8 index record"))?
                .trim(),
        )?;
    }
}

fn video_frames(path: &Path, video: &Value, control: &dyn Control) -> Result<(Vec<Frame>, bool)> {
    let program = control.tool("ffprobe");
    let args = vec!["-v".into(),"error".into(),"-protocol_whitelist".into(),"file,pipe".into(),
        "-select_streams".into(),"v:0".into(),"-show_frames".into(),"-show_entries".into(),
        "frame=best_effort_timestamp,key_frame,pict_type,interlaced_frame,width,height,pix_fmt,color_range,color_space,color_primaries,color_transfer,chroma_location:frame_side_data=".into(),
        "-of".into(),"compact=p=0".into(),path.to_string_lossy().into_owned()];
    let video = video.clone();
    media::feed_stdin_consuming(
        control.command(&program, &args)?,
        &program,
        TIMEOUT,
        media::Watch::default(),
        move |stdout| {
            let mut frames = Vec::new();
            let mut changed = false;
            lines(stdout, |line| {
                if line.is_empty() {
                    return Ok(());
                }
                let fields: std::collections::BTreeMap<_, _> =
                    line.split('|').filter_map(|s| s.split_once('=')).collect();
                let field = |name| {
                    fields
                        .get(name)
                        .copied()
                        .ok_or_else(|| invalid(format!("Missing frame {name}")))
                };
                let stamp = field("best_effort_timestamp")?
                    .parse()
                    .map_err(|_| invalid("Invalid presented frame timestamp"))?;
                let boolean = |name| match field(name)? {
                    "0" => Ok(false),
                    "1" => Ok(true),
                    _ => Err(invalid(format!("Invalid frame {name}"))),
                };
                let picture = match field("pict_type")? {
                    "I" => b'I',
                    "P" => b'P',
                    "B" => b'B',
                    "S" => b'S',
                    "SI" => b's',
                    "SP" => b'p',
                    "BI" => b'b',
                    "?" => b'?',
                    _ => return Err(invalid("Unknown picture type")),
                };
                for name in [
                    "width",
                    "height",
                    "pix_fmt",
                    "color_range",
                    "color_space",
                    "color_primaries",
                    "color_transfer",
                    "chroma_location",
                ] {
                    let expected = video[name]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| video[name].to_string());
                    if let Some(observed) = fields.get(name) {
                        changed |= *observed != expected;
                    }
                }
                if frames.len() == MAX_FRAMES {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Source index exceeds 4000000 presented frames",
                    ));
                }
                frames.push(Frame(
                    stamp,
                    boolean("key_frame")?,
                    picture,
                    boolean("interlaced_frame")?,
                ));
                Ok(())
            })?;
            if frames.is_empty() {
                return Err(invalid("Source has no presented video frames"));
            }
            Ok((frames, changed))
        },
        |_| Ok(()),
    )
}

#[derive(Default)]
struct Clock {
    first: Option<i64>,
    end: i64,
    samples: u64,
    continuous: bool,
    time_base: Option<Time>,
}

/// FFmpeg decodes once, then splits PCM to a byte counter and a small framecrc clock stream.
/// The checksums themselves are unused; PTS/duration/size establish the decoded audio clock.
fn audio(path: &Path, stream: &Value, control: &dyn Control) -> Result<Audio> {
    let number = |name| {
        stream[name]
            .as_u64()
            .or_else(|| stream[name].as_str()?.parse().ok())
            .ok_or_else(|| invalid(format!("Missing audio {name}")))
    };
    let (rate, channels) = (number("sample_rate")?, number("channels")?);
    if rate == 0 || ![1, 2].contains(&channels) {
        return Err(invalid(
            "Index audio requires a positive sample rate and mono/stereo",
        ));
    }
    let program = control.tool("ffmpeg");
    let args: Vec<String> = [
        "-v",
        "error",
        "-nostdin",
        "-protocol_whitelist",
        "file,pipe",
        "-copyts",
        "-i",
    ]
    .into_iter()
    .map(str::to_owned)
    .chain([path.to_string_lossy().into_owned()])
    .chain(
        [
            "-filter_complex",
            "[0:a:0]asplit=2[pcm][clock]",
            "-map",
            "[pcm]",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
            "-map",
            "[clock]",
            "-c:a",
            "pcm_s16le",
            "-f",
            "framecrc",
            "pipe:2",
        ]
        .into_iter()
        .map(str::to_owned),
    )
    .collect();
    let (bytes, clock) = media::feed_stdin_pipes(
        control.command(&program, &args)?,
        &program,
        TIMEOUT,
        media::Watch::default(),
        (
            |mut stdout| Ok(std::io::copy(&mut stdout, &mut std::io::sink())?),
            move |stderr| {
                let mut clock = Clock {
                    continuous: true,
                    ..Clock::default()
                };
                let mut diagnostics = Vec::new();
                let parsed = lines(stderr, |line| {
                    if let Some(value) = line.strip_prefix("#tb 0: ") {
                        clock.time_base = Some(ratio(&json!(value))?);
                    } else if line.starts_with("0,") {
                        let fields: Vec<_> = line.split(',').map(str::trim).collect();
                        if fields.len() < 6 {
                            return Err(invalid("Incomplete decoded PCM clock record"));
                        }
                        let pts: i64 = fields[2]
                            .parse()
                            .map_err(|_| invalid("Invalid decoded PCM timestamp"))?;
                        let count: u64 = fields[3]
                            .parse()
                            .map_err(|_| invalid("Invalid decoded PCM duration"))?;
                        let bytes: u64 = fields[4]
                            .parse()
                            .map_err(|_| invalid("Invalid decoded PCM size"))?;
                        if count == 0 || count.checked_mul(channels * 2) != Some(bytes) {
                            return Err(invalid("Decoded PCM size and duration disagree"));
                        }
                        clock.continuous &= clock.first.is_none() || clock.end == pts;
                        clock.first.get_or_insert(pts);
                        clock.end = pts
                            .checked_add(
                                i64::try_from(count)
                                    .map_err(|_| invalid("PCM duration overflow"))?,
                            )
                            .ok_or_else(|| invalid("PCM timestamp overflow"))?;
                        clock.samples = clock
                            .samples
                            .checked_add(count)
                            .ok_or_else(|| invalid("PCM count overflow"))?;
                    } else if !line.starts_with('#') {
                        let keep = line.len().min(65536usize.saturating_sub(diagnostics.len()));
                        diagnostics.extend_from_slice(&line.as_bytes()[..keep]);
                    }
                    Ok(())
                });
                (parsed.map(|()| clock), diagnostics)
            },
        ),
        |_| Ok(()),
    )?;
    if clock.time_base != Some(Time::new(1, rate)?)
        || clock.samples.checked_mul(channels * 2) != Some(bytes)
    {
        return Err(invalid("PCM counter and decoded clock disagree"));
    }
    Ok(Audio {
        rate,
        channels,
        samples: clock.samples,
        first_pts: clock.first.ok_or_else(|| invalid("Empty decoded audio"))?,
        continuous: clock.continuous,
        codec_initial_padding: stream["initial_padding"].as_u64(),
        stream: stream.clone(),
    })
}

fn periodic(frames: &[Frame], tb: Time, rate: Time) -> bool {
    if frames.is_empty()
        || tb.num as u128 * 1000 > tb.den as u128
        || tb.den as u128 > tb.num as u128 * 1_000_000_000
    {
        return false;
    }
    frames.iter().enumerate().all(|(n, frame)| {
        let top = n as u128 * rate.den as u128 * tb.den as u128;
        let bottom = rate.num as u128 * tb.num as u128;
        let expected = (top + bottom / 2) / bottom;
        frame.0 as i128 - frames[0].0 as i128 == expected as i128
            && (n == 0 || frames[n - 1].0 < frame.0)
    })
}

fn build(path: &Path, source: Identity, metadata: &Value, control: &dyn Control) -> Result<Index> {
    let streams = metadata["streams"]
        .as_array()
        .ok_or_else(|| invalid("No streams"))?;
    let videos: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "video")
        .collect();
    let audios: Vec<_> = streams
        .iter()
        .filter(|s| s["codec_type"] == "audio")
        .collect();
    let video = videos.first().ok_or_else(|| invalid("No video stream"))?;
    let time_base = ratio(&video["time_base"])?;
    let (frames, changed) = video_frames(path, video, control)?;
    control.check()?;
    let mut reasons = Vec::new();
    let mut reject = |valid: bool, reason: &str| {
        if !valid {
            reasons.push(reason.to_owned());
        }
    };
    reject(
        videos.len() == 1 && audios.len() <= 1 && streams.len() == videos.len() + audios.len(),
        "requires one video and at most one audio stream, without other stream types",
    );
    reject(
        metadata["format"]["format_name"].as_str().is_some_and(|s| {
            s.split(',')
                .any(|s| matches!(s, "mp4" | "mov" | "matroska"))
        }),
        "container must be MP4, MOV or MKV",
    );
    reject(
        video["codec_name"] == "h264"
            || video["codec_name"] == "hevc" && video["profile"] == "Main",
        "codec must be H.264 or HEVC Main",
    );
    reject(
        video["pix_fmt"] == "yuv420p" || video["pix_fmt"] == "yuvj420p",
        "requires 8-bit 4:2:0 samples",
    );
    reject(
        video["width"]
            .as_u64()
            .is_some_and(|n| n > 0 && n.is_multiple_of(2))
            && video["height"]
                .as_u64()
                .is_some_and(|n| n > 0 && n.is_multiple_of(2)),
        "requires positive even dimensions",
    );
    reject(
        !changed,
        "decoded frame format or color tags change within the source",
    );
    reject(
        matches!(
            video["field_order"].as_str(),
            Some("progressive" | "unknown") | None
        ) && frames.iter().all(|f| !f.3),
        "interlaced frames need conversion",
    );
    reject(
        matches!(
            video["sample_aspect_ratio"].as_str(),
            Some("1:1" | "N/A") | None
        ),
        "non-square pixels need conversion",
    );
    reject(
        video["side_data_list"]
            .as_array()
            .is_none_or(|s| s.iter().all(|v| v["side_data_type"] != "Display Matrix")),
        "display rotation or matrix needs conversion",
    );
    reject(
        video["color_space"] == "bt709"
            && video["color_primaries"] == "bt709"
            && matches!(
                video["color_transfer"].as_str(),
                Some("bt709" | "iec61966-2-1")
            )
            && matches!(video["color_range"].as_str(), Some("tv" | "pc"))
            && video["chroma_location"] == "left",
        "requires explicit SDR BT.709 matrix/primaries, full or limited range, BT.709 or sRGB transfer and left chroma",
    );
    let rate = crate::readiness::native_rate(video, metadata)
        .filter(|&rate| periodic(&frames, time_base, rate));
    reject(
        rate.is_some(),
        "presented frames must follow one of the eight native CFR clocks at container precision",
    );
    reject(
        frames[0].1 && frames.iter().all(|f| matches!(f.2, b'I' | b'P' | b'B')),
        "requires an initial keyframe and known I/P/B picture types",
    );
    let audio = if let Some(stream) = audios.first() {
        let channels = stream["channels"].as_u64().unwrap_or(0);
        let codec = stream["codec_name"].as_str().unwrap_or("");
        reject(
            matches!(codec, "aac" | "mp3" | "flac" | "opus") || codec.starts_with("pcm_"),
            "audio codec must be AAC, PCM, MP3, FLAC or Opus",
        );
        reject([1, 2].contains(&channels), "audio requires mono or stereo");
        if [1, 2].contains(&channels) {
            let audio = audio(path, stream, control)?;
            reject(
                audio.continuous,
                "decoded audio has a timestamp discontinuity",
            );
            Some(audio)
        } else {
            None
        }
    } else {
        None
    };
    // No cache entry is published if the input changed while its index was being built.
    if fs::metadata(path)?.len() != source.bytes
        || media::file_hash_controlled(path, control)? != source.sha256
    {
        return Err(error(
            "MEDIA_CHANGED",
            "Source changed while its presentation index was being built",
        ));
    }
    Ok(Index {
        source,
        video: (*video).clone(),
        time_base,
        rate,
        frames,
        audio,
        reasons,
    })
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn cache_read(path: &Path, key: &str, source: &Identity) -> Option<Index> {
    if fs::symlink_metadata(path).ok()?.file_type().is_symlink()
        || fs::metadata(path).ok()?.len() > MAX_BYTES
    {
        return None;
    }
    let mut bytes = Vec::new();
    GzDecoder::new(fs::File::open(path).ok()?)
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() as u64 > MAX_BYTES {
        return None;
    }
    let entry: Entry = serde_json::from_slice(&bytes).ok()?;
    if entry.schema != SCHEMA
        || entry.key != key
        || &entry.index.source != source
        || entry.index.frames.is_empty()
        || entry.index.frames.len() > MAX_FRAMES
        || digest(&serde_json::to_vec(&entry.index).ok()?) != entry.digest
    {
        return None;
    }
    Some(entry.index)
}
fn cache_write(path: &Path, key: &str, index: &Index) -> Result<()> {
    let payload = serde_json::to_vec(index)?;
    let entry = Entry {
        schema: SCHEMA,
        key: key.into(),
        digest: digest(&payload),
        index: index.clone(),
    };
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Missing index cache parent"))?;
    fs::create_dir_all(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp = parent.join(format!(".index-{}-{nonce}.tmp", std::process::id()));
    let written = (|| {
        let mut encoder = GzEncoder::new(fs::File::create_new(&temp)?, Compression::fast());
        serde_json::to_writer(&mut encoder, &entry)?;
        encoder.finish()?.sync_all()?;
        // Immutable, content-addressed publication. An existing bad entry is ignored, not trusted.
        match fs::hard_link(&temp, path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e.into()),
        }
    })();
    let _ = fs::remove_file(temp);
    written
}

/// Source identity must be freshly verified by the calling inspection. Both decoder builds and
/// the engine producer participate in the key; indexes never enter the project JSON.
pub(crate) fn inspect(path: &Path, source: Identity, metadata: &Value) -> Result<Value> {
    let control = &media::Uncontrolled;
    source.validate()?;
    let mut tools = Vec::new();
    for name in ["ffprobe", "ffmpeg"] {
        let path = crate::cache::resolve_tool(&control.tool(name))?;
        tools.push(json!({"sha256":media::file_hash_controlled(&path, control)?,"bytes":fs::metadata(path)?.len()}));
    }
    let key = digest(&serde_json::to_vec(&json!([
        SCHEMA,
        source,
        tools,
        crate::cache::producer()
    ]))?);
    let _turn = inspection_cache::turn(&key);
    let cache = inspection_cache::directory().map(|d| d.join(format!("{key}.index.json.gz")));
    let cached = cache.as_ref().and_then(|p| cache_read(p, &key, &source));
    let hit = cached.is_some();
    let index = match cached {
        Some(index) => index,
        None => {
            let _slot = inspection_cache::slot();
            let index = build(path, source, metadata, control)?;
            if let Some(path) = &cache {
                let _ = cache_write(path, &key, &index);
            }
            index
        }
    };
    let keys: Vec<_> = index
        .frames
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.1.then_some(i))
        .collect();
    let longest_gop = keys
        .iter()
        .copied()
        .chain([index.frames.len()])
        .collect::<Vec<_>>()
        .windows(2)
        .map(|p| p[1] - p[0])
        .max();
    let audio_policy = index.audio.as_ref().map(|a| json!({"decoded_samples":a.samples,"sample_rate":a.rate,"channels":a.channels,
        "first_pts":a.first_pts,"time_base":{"num":1,"den":a.rate},"continuous":a.continuous,"codec_initial_padding":a.codec_initial_padding,
        "mono_to_stereo":a.channels == 1,"resample_to_48000":a.rate != 48000}));
    Ok(
        json!({"schema_version":SCHEMA,"key":key,"cache_hit":hit,"cache_bytes":cache.and_then(|p| fs::metadata(p).ok()).map(|m| m.len()),
        "eligible":index.reasons.is_empty(),"reasons":index.reasons,"frames":index.frames.len(),"first_timestamp":index.frames[0].0,
        "time_base":index.time_base,"frame_rate":index.rate,"keyframes":keys.len(),"maximum_gop_frames":longest_gop,
        "b_frames":index.frames.iter().filter(|f| f.2 == b'B').count(),"audio":audio_policy,
        "square_pixels_assumed":!matches!(index.video["sample_aspect_ratio"].as_str(),Some("1:1")),
        "direct_decode_available":false}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clocks_use_integer_ticks_relative_to_the_first_presented_frame() {
        let frames = [400, 442, 483, 525].map(|n| Frame(n, false, b'B', false));
        assert!(periodic(
            &frames,
            Time::new(1, 1000).unwrap(),
            Time::new(24, 1).unwrap()
        ));
        assert!(!periodic(
            &frames,
            Time::new(1, 1000).unwrap(),
            Time::new(25, 1).unwrap()
        ));
        let mut missing = frames.to_vec();
        missing[2].0 += 1;
        assert!(!periodic(
            &missing,
            Time::new(1, 1000).unwrap(),
            Time::new(24, 1).unwrap()
        ));
        assert!(lines(&b"valid\n"[..], |_| Ok(())).is_ok());
        assert!(lines(&vec![b'x'; 8193][..], |_| Ok(())).is_err());
    }
    #[test]
    fn compressed_entries_reject_wrong_identity_key_schema_and_corruption() {
        let scratch = crate::tracking::Scratch::new("source-index");
        let path = scratch.0.join("entry.json.gz");
        let source = Identity {
            bytes: 1,
            sha256: "a".repeat(64),
        };
        let index = Index {
            source: source.clone(),
            video: json!({}),
            time_base: Time::new(1, 25).unwrap(),
            rate: None,
            frames: vec![Frame(0, true, b'I', false)],
            audio: None,
            reasons: vec!["fixture".into()],
        };
        cache_write(&path, "key", &index).unwrap();
        assert!(cache_read(&path, "key", &source).is_some());
        assert!(cache_read(&path, "other", &source).is_none());
        assert!(
            cache_read(
                &path,
                "key",
                &Identity {
                    bytes: 2,
                    ..source.clone()
                }
            )
            .is_none()
        );
        let original = fs::read(&path).unwrap();
        cache_write(&path, "other", &index).unwrap();
        assert_eq!(fs::read(&path).unwrap(), original);
        let rewrite = |schema, digest: &str| {
            let entry = Entry {
                schema,
                key: "key".into(),
                digest: digest.into(),
                index: index.clone(),
            };
            let mut encoder = GzEncoder::new(fs::File::create(&path).unwrap(), Compression::fast());
            serde_json::to_writer(&mut encoder, &entry).unwrap();
            encoder.finish().unwrap();
        };
        let payload_digest = digest(&serde_json::to_vec(&index).unwrap());
        rewrite(SCHEMA + 1, &payload_digest);
        assert!(cache_read(&path, "key", &source).is_none());
        rewrite(SCHEMA, &"0".repeat(64));
        assert!(cache_read(&path, "key", &source).is_none());
        fs::write(&path, b"bad derived cache").unwrap();
        assert!(cache_read(&path, "key", &source).is_none());
    }
}
