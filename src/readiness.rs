//! Whether a probed file can go on a timeline as it is, and if not, the media.conform recipe that
//! makes it usable, so media.inspect answers the agent's next step instead of only listing streams.
use crate::{media, render, time::Time};
use serde_json::{Value, json};
use std::path::Path;

/// Sequential timeline rates; placed tracks use 25 fps.
const RATES: [(u64, u64); 8] = [
    (24, 1),
    (25, 1),
    (30, 1),
    (50, 1),
    (60, 1),
    (24000, 1001),
    (30000, 1001),
    (60000, 1001),
];
/// media.conform output limits: at most 4096x2160, 8M pixels and 45000 frames at 25 fps.
const CONFORM_FRAMES: u64 = 45_000;

/// Timeline readiness of `path`, whose metadata was already probed; `relative` is its path under
/// `input_root`. Ready sources are checked by the renderer's own packet-timed source inspection.
pub fn timeline(path: &Path, relative: &str, metadata: &Value) -> Value {
    let streams = metadata["streams"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let text = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => "unknown".into(),
        other => other.to_string(),
    };
    if metadata["format"]["format_name"]
        .as_str()
        .is_some_and(|f| f == "image2" || f.ends_with("_pipe"))
    {
        return json!({"ready":false,"reasons":["still image: show it with a scene image layer and scene.render, or use it in captions or graphics"]});
    }
    let mut reasons = Vec::new();
    match video {
        None => reasons.push("no video stream; timeline sources need FFV1 video".to_string()),
        Some(v) => {
            if v["codec_name"] != "ffv1" || !(v["pix_fmt"] == "bgr0" || v["pix_fmt"] == "bgra") {
                reasons.push(format!(
                    "video is {} {}; timeline video must be FFV1 bgr0 (bgra for alpha_over overlay tracks)",
                    text(&v["codec_name"]),
                    text(&v["pix_fmt"])
                ));
            }
            if v.get("side_data_list").is_some() {
                reasons.push("video carries side data such as rotation or HDR metadata".into());
            }
            if let Some(sar) = v["sample_aspect_ratio"].as_str()
                && sar != "1:1"
                && sar != "N/A"
            {
                reasons.push(format!("pixels are not square (aspect {sar})"));
            }
        }
    }
    match audio {
        None => reasons.push(
            "no audio stream; timeline sources need 48000 Hz stereo PCM s16 audio".to_string(),
        ),
        Some(a) => {
            if a["codec_name"] != "pcm_s16le" || a["sample_rate"] != "48000" || a["channels"] != 2 {
                reasons.push(format!(
                    "audio is {} at {} Hz with {} channels; timeline audio must be PCM s16 at 48000 Hz, 2 channels",
                    text(&a["codec_name"]),
                    text(&a["sample_rate"]),
                    text(&a["channels"])
                ));
            }
        }
    }
    if streams.len() > 2 {
        reasons.push(format!(
            "{} streams; timeline sources have exactly one video and one audio stream",
            streams.len()
        ));
    }
    let rate = video.and_then(|v| native_rate(v, metadata));
    if let Some(v) = video
        && rate.is_none()
    {
        reasons.push(format!(
            "frame rate {} is not a timeline rate (24, 25, 30, 50 or 60 fps, or the 1000/1001 rates)",
            text(&v["r_frame_rate"])
        ));
    }
    if reasons.is_empty()
        && let (Some(v), Some(rate)) = (video, rate)
    {
        let (width, height) = (dimension(&v["width"]), dimension(&v["height"]));
        match render::inspect_timeline_source(path, width, height, rate, &media::Uncontrolled)
            .and_then(|source| {
                Ok((
                    source.frames,
                    Time::new(source.frames * rate.den, rate.num)?,
                ))
            }) {
            Ok((frames, duration)) => {
                let alpha = v["pix_fmt"] == "bgra";
                return json!({"ready":true,"width":width,"height":height,"frame_rate":rate,"frames":frames,
                    "duration":duration,"alpha":alpha,"use":if alpha {"alpha_over overlay tracks"} else {"any timeline with this size and frame rate"},
                    "asset":{"id":asset_id(relative),"path":relative,"duration":duration}});
            }
            Err(e) => reasons.push(e.message),
        }
    }
    let mut result = json!({"ready":false,"reasons":reasons});
    let proposal = match (video, audio) {
        (Some(v), _) => Some(conform(relative, v, audio, rate, None)),
        (None, Some(a)) => Some(conform_audio(
            relative,
            a,
            metadata,
            Time { num: 25, den: 1 },
            (1920, 1080),
        )),
        (None, None) => None,
    };
    match proposal {
        Some(Ok(recipe)) => {
            let next = if video.is_some() {
                "check the recipe with media.conform.inspect, run it with job.start run media.conform, then media.add the returned asset"
            } else {
                "audio-only: the asset gets a silent black picture, so set width, height and frame_rate to the project's (media.prepare with the project does all of this in one step), then place it on an audio track"
            };
            result["conform"] = json!({"recipe":recipe,"output":format!("{}-conformed.mkv",asset_id(relative)),"next":next})
        }
        Some(Err(why)) => result["reasons"]
            .as_array_mut()
            .expect("reasons")
            .push(why.into()),
        None => {}
    }
    result
}

/// One step from any decodable video file to a timeline asset. A ready file that fits the target
/// is returned as it is. Anything else is converted with the readiness recipe: at the project's
/// frame rate and size when a project is given, otherwise at the source's own rate when that is a
/// timeline rate (25 fps otherwise) and its own size.
pub fn prepare(
    path: &Path,
    input_root: &Path,
    output_root: &Path,
    output: Option<&Path>,
    project: Option<&crate::model::Project>,
    transcripts: &[crate::transcript::Document],
) -> crate::Result<Value> {
    use crate::error;
    let path = media::allowed_file(path, input_root)?;
    let identity = crate::identity::relative(&path, input_root)?;
    let relative = identity["path"]
        .as_str()
        .expect("identity path")
        .to_string();
    // Only this file's transcripts travel with it.
    let transcripts: Vec<crate::transcript::Document> = transcripts
        .iter()
        .filter(|d| {
            identity["sha256"] == d.source.identity.sha256.as_str()
                && identity["bytes"] == d.source.identity.bytes
        })
        .cloned()
        .collect();
    let moved = |recipe: &crate::conform::Recipe, receipt: &Value| -> crate::Result<Value> {
        Ok(json!(follow(&transcripts, recipe, receipt, output_root)?))
    };
    let metadata = media::probe(&path)?;
    let readiness = timeline(&path, &relative, &metadata);
    let fits = |r: &Value| match project {
        None => true,
        Some(p) => {
            r["width"] == p.width
                && r["height"] == p.height
                && serde_json::from_value::<Time>(r["frame_rate"].clone())
                    .is_ok_and(|rate| rate == p.frame_rate)
        }
    };
    if readiness["ready"] == true && fits(&readiness) {
        return Ok(
            json!({"converted":false,"asset":readiness["asset"],"readiness":readiness,"transcripts":transcripts}),
        );
    }
    let streams = metadata["streams"]
        .as_array()
        .map_or(&[][..], Vec::as_slice);
    let video = streams.iter().find(|s| s["codec_type"] == "video");
    let audio = streams.iter().find(|s| s["codec_type"] == "audio");
    let still = metadata["format"]["format_name"]
        .as_str()
        .is_some_and(|f| f == "image2" || f.ends_with("_pipe"));
    if let (None, Some(audio), false) = (video, audio, still) {
        // Voice-overs and music: a PCM WAV becomes an asset with a silent black picture of the
        // project's size, for an audio track.
        let Some(p) = project else {
            return Err(error(
                "UNSUPPORTED_MEDIA",
                format!(
                    "{relative} is audio-only: give the project so its silent picture matches the timeline's size and frame rate"
                ),
            ));
        };
        let rate = crate::render::clock::rate(p.frame_rate)?;
        let mut recipe = conform_audio(&relative, audio, &metadata, rate, (p.width, p.height))
            .map_err(|why| error("UNSUPPORTED_MEDIA", format!("{relative}: {why}")))?;
        recipe["source"]["file"] = identity.clone();
        let recipe: crate::conform::Recipe = serde_json::from_value(recipe)?;
        let output = match output {
            Some(output) => output.to_path_buf(),
            None => output_root.join(format!("{}-prepared.mkv", asset_id(&relative))),
        };
        let receipt = crate::conform::run(&recipe, input_root, output_root, &output)?;
        return Ok(
            json!({"converted":true,"asset":receipt["asset"],"recipe":recipe,"output":receipt["output"],
            "frames":receipt["frames"],"frame_rate":p.frame_rate,"reasons":readiness["reasons"],
            "transcripts":moved(&recipe, &receipt)?,
            "note":"audio-only source: the asset has a silent black picture; place it on an audio track"}),
        );
    }
    let (Some(video), false) = (video, still) else {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            format!(
                "{relative} cannot become a timeline asset: {}",
                readiness["reasons"]
            ),
        ));
    };
    let rate = match project {
        Some(p) => Some(crate::render::clock::rate(p.frame_rate)?),
        None => native_rate(video, &metadata),
    };
    let known = serde_json::from_value::<Time>(readiness["duration"].clone()).ok();
    let mut recipe = conform(&relative, video, audio, rate, known)
        .map_err(|why| error("UNSUPPORTED_MEDIA", format!("{relative}: {why}")))?;
    // The request-level identity completion does not reach a recipe built here.
    recipe["source"]["file"] = identity.clone();
    if let Some(p) = project {
        recipe["width"] = json!(p.width);
        recipe["height"] = json!(p.height);
    }
    let recipe: crate::conform::Recipe = serde_json::from_value(recipe)?;
    let output = match output {
        Some(output) => output.to_path_buf(),
        None => output_root.join(format!("{}-prepared.mkv", asset_id(&relative))),
    };
    let receipt = crate::conform::run(&recipe, input_root, output_root, &output)?;
    Ok(
        json!({"converted":true,"asset":receipt["asset"],"recipe":recipe,"output":receipt["output"],
        "frames":receipt["frames"],"frame_rate":recipe.frame_rate.unwrap_or(Time { num: 25, den: 1 }),
        "reasons":readiness["reasons"],"transcripts":moved(&recipe, &receipt)?}),
    )
}

/// Prepare several files in one job, each as [`prepare`] does. Asset IDs come from the file names,
/// made unique within the batch and against the project's assets (`<stem>`, `<stem>-2`, …), and a
/// conversion is written to `<id>-prepared.mkv`. One file's failure is reported without stopping
/// the others; the prepared assets come back as `media.add` operations.
pub fn prepare_many(
    paths: &[std::path::PathBuf],
    input_root: &Path,
    output_root: &Path,
    project: Option<&crate::model::Project>,
    transcripts: &[crate::transcript::Document],
) -> crate::Result<Value> {
    use crate::error;
    if paths.is_empty() || paths.len() > 200 {
        return Err(error("INVALID_ARGUMENT", "paths must list 1-200 files"));
    }
    let mut used: std::collections::BTreeSet<String> = project
        .map(|p| p.assets.iter().map(|a| a.id.clone()).collect())
        .unwrap_or_default();
    let mut results = Vec::new();
    let mut operations = Vec::new();
    let (mut converted, mut ready) = (0, 0);
    for path in paths {
        let path = if path.is_relative() {
            input_root.join(path)
        } else {
            path.clone()
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let base = asset_id(&name);
        let mut id = base.clone();
        let mut n = 2;
        while !used.insert(id.clone()) {
            id = format!("{base}-{n}");
            n += 1;
        }
        let output = output_root.join(format!("{id}-prepared.mkv"));
        match prepare(
            &path,
            input_root,
            output_root,
            Some(&output),
            project,
            transcripts,
        ) {
            Ok(mut result) => {
                result["asset"]["id"] = json!(id);
                if result["converted"] == true {
                    converted += 1;
                } else {
                    ready += 1;
                }
                operations.push(json!({"op":"media.add","asset":result["asset"]}));
                results.push(json!({"path":path,"asset_id":id,"result":result}));
            }
            Err(e) => {
                results.push(json!({"path":path,"error":{"code":e.code,"message":e.message}}));
            }
        }
    }
    let failed = results.len() - converted - ready;
    // Every moved transcript in one list, for timeline.outline, captions.draft and the rest.
    let moved: Vec<Value> = results
        .iter()
        .flat_map(|r| {
            r["result"]["transcripts"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .collect();
    Ok(
        json!({"prepared":results,"converted":converted,"ready":ready,"failed":failed,"operations":operations,
        "transcripts":moved,"next":"apply operations with session.apply to add the prepared assets"}),
    )
}

fn dimension(value: &Value) -> u32 {
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(0)
}

/// The timeline rate this video stream declares, using the renderer's own rate test.
pub(crate) fn native_rate(video: &Value, metadata: &Value) -> Option<Time> {
    RATES
        .iter()
        .map(|&(num, den)| Time { num, den })
        .find(|&rate| render::clock::rate_hint(video, metadata, rate))
}

/// A valid asset ID from the file name: its stem, at most 128 bytes.
fn asset_id(relative: &str) -> String {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    let stem = name.rsplit_once('.').map_or(name, |(stem, _)| stem);
    let mut id = if stem.trim().is_empty() {
        "media"
    } else {
        stem
    }
    .to_string();
    while id.len() > 128 {
        id.pop();
    }
    id
}

/// A stream's length: exact from `duration_ts` and `time_base`, else its decimal `duration`, else
/// a Matroska `DURATION` tag such as `00:00:03.000000000`.
fn stream_duration(stream: &Value) -> Option<Time> {
    if let (Some(ticks), Some((num, den))) = (
        stream["duration_ts"].as_u64(),
        stream["time_base"].as_str().and_then(|t| t.split_once('/')),
    ) {
        return Time::new(ticks.checked_mul(num.parse().ok()?)?, den.parse().ok()?).ok();
    }
    if let Some(seconds) = stream["duration"].as_str() {
        return serde_json::from_value(json!(seconds)).ok();
    }
    let tag = stream["tags"]["DURATION"].as_str()?;
    let mut parts = tag.split(':');
    let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
    let seconds: Time = serde_json::from_value(json!(seconds)).ok()?;
    let whole = hours.parse::<u64>().ok()? * 3600 + minutes.parse::<u64>().ok()? * 60;
    seconds.plus(Time::new(whole, 1).ok()?).ok()
}

/// A stream's length in whole frames at `rate`.
fn frames_at(stream: &Value, rate: Time) -> Option<u64> {
    let d = stream_duration(stream)?;
    Some(
        (u128::from(d.num) * u128::from(rate.num) / (u128::from(d.den) * u128::from(rate.den)))
            as u64,
    )
}

/// Frames per step that make a whole number of 48 kHz samples at `rate` (5 at 30000/1001).
fn sample_step(rate: Time) -> u64 {
    let (mut a, mut b) = (rate.num, 48_000 * rate.den);
    while b != 0 {
        (a, b) = (b, a % b);
    }
    rate.num / a
}

/// Transcripts of a conform's source, moved onto its output so they need not be recognized again.
/// Only a forward, unit-speed conversion keeps audio time unchanged, so others are refused. A
/// document of another source is refused too. The new source path is the output's, relative to
/// `output_root`.
pub fn follow(
    transcripts: &[crate::transcript::Document],
    recipe: &crate::conform::Recipe,
    receipt: &Value,
    output_root: &Path,
) -> crate::Result<Vec<crate::transcript::Document>> {
    if transcripts.is_empty() {
        return Ok(Vec::new());
    }
    can_follow(transcripts, recipe)?;
    moved_onto(transcripts, recipe, receipt, output_root)
}

/// Whether `transcripts` can follow `recipe`'s conversion; checked before converting, so a refusal
/// publishes nothing.
pub fn can_follow(
    transcripts: &[crate::transcript::Document],
    recipe: &crate::conform::Recipe,
) -> crate::Result<()> {
    use crate::error;
    if transcripts.is_empty() {
        return Ok(());
    }
    if recipe.rate != (Time { num: 1, den: 1 })
        || recipe.reverse
        || recipe.freeze
        || recipe.remap.is_some()
        || !matches!(recipe.audio, crate::conform::Audio::Resample)
    {
        return Err(error(
            "INVALID_ARGUMENT",
            "transcripts can follow only a forward, unit-speed conversion that keeps its audio",
        ));
    }
    for document in transcripts {
        if document.source.identity.sha256 != recipe.source.file.sha256
            || document.source.identity.bytes != recipe.source.file.bytes
        {
            return Err(error(
                "MEDIA_CHANGED",
                format!(
                    "transcript {} is of another file than this conversion's source {}",
                    document.id,
                    recipe.source.file.path.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Transcripts moved onto a finished conversion's output.
fn moved_onto(
    transcripts: &[crate::transcript::Document],
    recipe: &crate::conform::Recipe,
    receipt: &Value,
    output_root: &Path,
) -> crate::Result<Vec<crate::transcript::Document>> {
    let asset = &receipt["asset"];
    let identity = crate::registry::Identity {
        sha256: asset["identity"]["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        bytes: asset["identity"]["bytes"].as_u64().unwrap_or(0),
    };
    // Both sides resolved, so a verbatim (\\?\) output path still strips its root.
    let output = std::fs::canonicalize(asset["path"].as_str().unwrap_or_default())?;
    let root = std::fs::canonicalize(output_root)?;
    let path = match output.strip_prefix(&root) {
        Ok(relative) => relative.to_path_buf(),
        Err(_) => output
            .file_name()
            .map(std::path::PathBuf::from)
            .unwrap_or_default(),
    };
    transcripts
        .iter()
        .map(|document| {
            crate::transcript::rebind(
                document,
                crate::transcript::Source {
                    path: path.clone(),
                    identity: identity.clone(),
                    duration: recipe.duration,
                },
                recipe.source_in,
            )
        })
        .collect()
}

/// A media.conform recipe that turns a whole PCM16 WAV into an asset with a silent black picture
/// of `size` at `rate`, or why there is none. Its length is the audio's whole frames (and whole
/// 48 kHz samples); a tail shorter than one frame is left out.
pub(crate) fn conform_audio(
    relative: &str,
    audio: &Value,
    metadata: &Value,
    rate: Time,
    size: (u32, u32),
) -> Result<Value, &'static str> {
    let format = metadata["format"]["format_name"].as_str().unwrap_or("");
    let wav = format == "wav" && relative.to_ascii_lowercase().ends_with(".wav");
    if !wav || audio["codec_name"] != "pcm_s16le" {
        return Err(
            "audio-only sources must be PCM16 WAV for media.conform; convert other audio formats to WAV first",
        );
    }
    let sample_rate = audio["sample_rate"].as_str().unwrap_or("");
    if !["24000", "44100", "48000"].contains(&sample_rate)
        || !matches!(audio["channels"].as_u64(), Some(1 | 2))
    {
        return Err("audio-only WAV must be mono or stereo at 24000, 44100 or 48000 Hz");
    }
    let step = sample_step(rate);
    let frames = frames_at(audio, rate)
        .ok_or("the audio duration is not exact, so no conform duration can be proposed")?
        .min(CONFORM_FRAMES)
        / step
        * step;
    if frames == 0 {
        return Err("the audio is shorter than one frame");
    }
    let mut recipe = json!({"schema_version":1,"id":asset_id(relative),"source":{"file":{"path":relative},"color":null},
        "source_in":0,"duration":Time::new(frames * rate.den, rate.num).map_err(|_| "the proposed duration overflows")?,
        "rate":1,"reverse":false,"freeze":false,"width":size.0,"height":size.1,"audio":"resample"});
    if rate != (Time { num: 25, den: 1 }) {
        recipe["frame_rate"] = json!(rate);
    }
    Ok(recipe)
}

/// A media.conform recipe for the whole source at 25 fps, from its tags, or why there is none.
pub(crate) fn conform(
    relative: &str,
    video: &Value,
    audio: Option<&Value>,
    rate: Option<Time>,
    known: Option<Time>,
) -> Result<Value, &'static str> {
    let tag = |key: &str| {
        video[key]
            .as_str()
            .filter(|v| !v.is_empty() && *v != "unknown")
    };
    if matches!(tag("color_transfer"), Some("smpte2084" | "arib-std-b67")) {
        return Err("HDR source: convert it with hdr.conform instead of media.conform");
    }
    let pix_fmt = video["pix_fmt"].as_str().unwrap_or("");
    let rgb = matches!(pix_fmt, "bgr0" | "rgb24" | "bgra" | "rgba" | "gbrp");
    let tagged_601 = matches!(tag("color_space"), Some("smpte170m" | "bt470bg"))
        || matches!(tag("color_primaries"), Some(p) if p != "bt709");
    if !rgb && tagged_601 {
        return Err(
            "the source is tagged with non-BT.709 color, which media.conform does not convert",
        );
    }
    // Keep the source's own rate when it is a timeline rate, so every frame survives.
    let rate = rate.unwrap_or(Time { num: 25, den: 1 });
    let step = sample_step(rate);
    // A verified source duration (from the renderer's own inspection) wins over stream metadata.
    let from_known = known.map(|d| {
        (u128::from(d.num) * u128::from(rate.num) / (u128::from(d.den) * u128::from(rate.den)))
            as u64
    });
    let frames = from_known
        .or_else(|| {
            [Some(video), audio]
                .into_iter()
                .flatten()
                .map(|s| frames_at(s, rate))
                .collect::<Option<Vec<_>>>()
                .and_then(|f| f.into_iter().min())
        })
        .ok_or("the stream durations are not exact, so no conform duration can be proposed")?
        .min(CONFORM_FRAMES);
    let frames = frames / step * step;
    if frames == 0 {
        return Err("the source is shorter than one frame at the proposed rate");
    }
    let (mut width, mut height) = (dimension(&video["width"]), dimension(&video["height"]));
    if width > 4096 || height > 2160 || u64::from(width) * u64::from(height) > 8_294_400 {
        // Fit 3840x2160, keeping the aspect ratio with even dimensions.
        let scale = (3840.0 / f64::from(width)).min(2160.0 / f64::from(height));
        width = ((f64::from(width) * scale / 2.0).floor() as u32 * 2).max(2);
        height = ((f64::from(height) * scale / 2.0).floor() as u32 * 2).max(2);
    }
    let mut recipe = json!({"schema_version":1,"id":asset_id(relative),"source":{"file":{"path":relative}},
        "source_in":0,"duration":Time::new(frames * rate.den, rate.num).map_err(|_| "the proposed duration overflows")?,"rate":1,"reverse":false,"freeze":false,
        "width":width,"height":height,"audio":if audio.is_some() {"resample"} else {"mute"}});
    if rate != (Time { num: 25, den: 1 }) {
        recipe["frame_rate"] = json!(rate);
    }
    if video["codec_name"] == "ffv1" && pix_fmt == "bgr0" {
        recipe["source"]["color"] = json!("encoded_rgb");
    } else {
        let full = tag("color_range") == Some("pc") || pix_fmt.starts_with("yuvj") || rgb;
        let untagged = [
            "color_range",
            "color_space",
            "color_transfer",
            "color_primaries",
        ]
        .iter()
        .any(|key| tag(key).is_none());
        recipe["source"]["sdr"] = json!({"matrix":if rgb {"rgb"} else {"bt709"},"range":if full {"full"} else {"limited"},
            "transfer":if tag("color_transfer") == Some("iec61966-2-1") {"srgb"} else {"bt709"},
            "missing_tags":if untagged {"use_declared"} else {"reject"}});
        recipe["working_transfer"] = json!("bt709");
    }
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proposes_a_tagged_conform_recipe() {
        let video = json!({"codec_type":"video","codec_name":"h264","pix_fmt":"yuv420p","width":640,"height":360,
            "r_frame_rate":"30/1","time_base":"1/15360","duration_ts":122880,"color_range":"tv","color_space":"bt709",
            "color_transfer":"bt709","color_primaries":"bt709","sample_aspect_ratio":"1:1"});
        let audio = json!({"codec_type":"audio","codec_name":"aac","sample_rate":"44100","channels":2,
            "time_base":"1/44100","duration_ts":352800});
        let metadata = json!({"streams":[video,audio],"format":{"format_name":"mov,mp4"}});
        let result = timeline(Path::new("unused.mp4"), "clips/phone.mp4", &metadata);
        assert_eq!(result["ready"], false);
        assert_eq!(result["reasons"].as_array().unwrap().len(), 2, "{result}");
        let recipe = &result["conform"]["recipe"];
        assert_eq!(recipe["id"], "phone");
        // The 30 fps source keeps its own rate, so all 240 frames survive.
        assert_eq!(recipe["duration"], json!({"num":8,"den":1}));
        assert_eq!(recipe["frame_rate"], json!({"num":30,"den":1}));
        assert_eq!(recipe["source"]["sdr"]["range"], "limited");
        assert_eq!(recipe["source"]["sdr"]["missing_tags"], "reject");
        assert_eq!(result["conform"]["output"], "phone-conformed.mkv");
    }

    #[test]
    fn proposes_a_silent_picture_for_audio_only_wav() {
        // 10.64 s of 24 kHz mono narration: 266 frames at 25 fps.
        let audio = json!({"codec_type":"audio","codec_name":"pcm_s16le","sample_rate":"24000","channels":1,
            "time_base":"1/24000","duration_ts":255360});
        let metadata = json!({"streams":[audio],"format":{"format_name":"wav"}});
        let result = timeline(Path::new("unused.wav"), "voice/s1.wav", &metadata);
        assert_eq!(result["ready"], false);
        let recipe = &result["conform"]["recipe"];
        assert_eq!(
            recipe["source"],
            json!({"file":{"path":"voice/s1.wav"},"color":null})
        );
        assert_eq!(recipe["duration"], json!({"num":266,"den":25}));
        assert_eq!(
            (
                recipe["width"].clone(),
                recipe["height"].clone(),
                recipe["audio"].clone()
            ),
            (json!(1920), json!(1080), json!("resample"))
        );
        assert!(
            result["conform"]["next"]
                .as_str()
                .unwrap()
                .contains("media.prepare")
        );
        serde_json::from_value::<crate::conform::Recipe>(recipe.clone()).expect("a valid recipe");
        // At 30000/1001 the length is whole multiples of five frames (whole 48 kHz samples).
        let recipe = conform_audio(
            "s1.wav",
            &metadata["streams"][0],
            &metadata,
            Time {
                num: 30000,
                den: 1001,
            },
            (1280, 720),
        )
        .unwrap();
        assert_eq!(recipe["duration"], json!({"num":21021,"den":2000})); // 315 frames
        let mp3 = json!({"streams":[{"codec_type":"audio","codec_name":"mp3","sample_rate":"44100","channels":2,"time_base":"1/14112000","duration_ts":14112000}],"format":{"format_name":"mp3"}});
        let result = timeline(Path::new("unused.mp3"), "music.mp3", &mp3);
        assert!(
            result.get("conform").is_none() && result["reasons"].to_string().contains("PCM16 WAV"),
            "{result}"
        );
    }

    #[test]
    fn declines_hdr_and_bt601_sources() {
        let mut video = json!({"codec_type":"video","codec_name":"hevc","pix_fmt":"yuv420p10le","width":3840,
            "height":2160,"r_frame_rate":"25/1","time_base":"1/25","duration_ts":50,"color_transfer":"smpte2084"});
        let metadata = json!({"streams":[video.clone()],"format":{}});
        let result = timeline(Path::new("unused"), "hdr.mov", &metadata);
        assert!(result.get("conform").is_none());
        assert!(result["reasons"].to_string().contains("hdr.conform"));
        video["color_transfer"] = json!("bt709");
        video["color_space"] = json!("smpte170m");
        let result = timeline(
            Path::new("unused"),
            "sd.mov",
            &json!({"streams":[video],"format":{}}),
        );
        assert!(result.get("conform").is_none(), "{result}");
        let still = json!({"streams":[{"codec_type":"video","codec_name":"png"}],"format":{"format_name":"png_pipe"}});
        let result = timeline(Path::new("unused"), "logo.png", &still);
        assert_eq!(result["reasons"].as_array().unwrap().len(), 1);
        assert!(result.get("conform").is_none());
    }
}
