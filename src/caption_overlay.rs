//! A whole caption document as compiled overlay media or saved generated scene edits.
//! A document can run for hours, longer than any scene. Both modes use caption windows bounded
//! to ten seconds and fifteen cues. Compiled mode joins their movies losslessly; generated mode
//! publishes a validated edit proposal retaining the scene recipes and exact placed coverage.
use crate::{
    Result,
    captions::{self, Document, Layout},
    error, media,
    model::Project,
    render,
    scene::{self, Scene},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Caption layers per window: the scene limit less the blank layer every window carries.
const CUES_PER_WINDOW: usize = 15;

/// Keep compiled requests compatible; generated mode publishes editable scene operations.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A transparent lossless movie asset, the existing behavior.
    #[default]
    Compiled,
    /// A JSON edit proposal containing generated scene clips, with no intermediate movie.
    Generated,
}
impl Mode {
    pub fn is_compiled(&self) -> bool {
        *self == Self::Compiled
    }
}

/// What to render.
pub struct Request<'a> {
    pub document: &'a Document,
    pub layouts: &'a BTreeMap<String, Layout>,
    pub project: &'a Project,
    pub input_root: &'a Path,
    pub output_root: &'a Path,
    pub output: &'a Path,
    pub start: Option<Time>,
    pub duration: Option<Time>,
    pub asset_id: Option<&'a str>,
    pub mode: Mode,
    pub track_id: Option<&'a str>,
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// Owned scratch folder for window scenes and the join; removed afterwards.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The window scene before captions: transparent, with one invisible layer so a window without
/// cues is still a valid scene.
fn base(id: &str, project: &Project, rate: Time, duration: Time) -> Result<Scene> {
    let blank = json!({"id":"blank","canvas":[1,1],"start":Time::ZERO,"duration":duration,"frames":[],
        "graphics":{"kind":"shape","shape":"rectangle","rect":[0,0,1,1],"fill":[0,0,0,0],"stroke":null},
        "timing":"strict","end":"hold_last",
        "transform":{"position":[0,0],"crop":[0,0,1,1],"scale":1,"quarter_turns":0,"opacity":255}});
    Ok(serde_json::from_value(
        json!({"schema_version":1,"id":id,"width":project.width,
        "height":project.height,"output_scale":1,"duration":duration,"frame_rate":rate,
        "background":[0,0,0],"color":"srgb_straight_encoded","layers":[blank],"audio":null,
        "transparent":true}),
    )?)
}

/// Render the document over `[start, start + duration)` of the project's timeline.
pub fn render(request: &Request) -> Result<Value> {
    let project = request.project;
    let document = request.document;
    project.validate()?;
    document.validate()?;
    let rate = render::clock::rate(project.frame_rate)?;
    let start = request.start.unwrap_or(Time::ZERO);
    let duration = match request.duration {
        Some(duration) => duration,
        None => project
            .duration()?
            .minus(start)
            .map_err(|_| error("INVALID_RANGE", "start is after the timeline end"))?,
    };
    // Both ends must fall on frame boundaries.
    start.units(rate)?;
    let frames = duration.units(rate)?;
    if frames == 0 {
        return Err(error(
            "INVALID_RANGE",
            "The overlay needs at least one frame",
        ));
    }
    let asset_id = request.asset_id.unwrap_or(&document.id);
    crate::tracks::id(asset_id)?;
    // Windows are whole numbers of 48 kHz samples and of milliseconds, so scenes are valid and
    // the join is exact: a multiple of `step` frames, at most ten seconds.
    let samples_step = rate.num / gcd(rate.num, 48_000 * rate.den);
    let millis_step = rate.num / gcd(rate.num, 1_000 * rate.den);
    let step = samples_step / gcd(samples_step, millis_step) * millis_step;
    let longest = 10 * rate.num / rate.den / step * step;
    let total = frames.div_ceil(step) * step;
    let at = |frame: u64| -> Result<Time> { start.plus(Time::new(frame * rate.den, rate.num)?) };
    // Plan windows: as long as possible, shortened so no window holds more than 15 cues.
    let mut windows = Vec::new();
    let mut frame = 0;
    while frame < total {
        let mut end = (frame + longest).min(total);
        let (from, to) = (at(frame)?, at(end)?);
        let inside: Vec<&captions::Cue> = document
            .cues
            .iter()
            .filter(|c| {
                c.end.compare(from).is_ok_and(|o| o.is_gt())
                    && c.start.compare(to).is_ok_and(|o| o.is_lt())
            })
            .collect();
        if inside.len() > CUES_PER_WINDOW {
            // End the window before the 16th cue starts.
            let cue = inside[CUES_PER_WINDOW];
            let offset = cue.start.minus(start)?;
            let cut =
                offset.num as u128 * rate.num as u128 / (offset.den as u128 * rate.den as u128);
            end = (cut as u64) / step * step;
            if end <= frame {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    format!(
                        "More than {CUES_PER_WINDOW} cues overlap {} frames from {} s; shorten or merge cues",
                        step, from
                    ),
                ));
            }
        }
        windows.push((frame, end));
        frame = end;
    }
    if request.mode == Mode::Generated {
        return generated(request, rate, start, duration, frames, &windows);
    }
    if request.track_id.is_some() {
        return Err(
            error("INVALID_ARGUMENT", "track_id requires mode generated").with_path("track_id"),
        );
    }
    let output = render::destination(request.output, request.output_root)?;
    let scratch = scratch(&output)?;
    let mut list = String::new();
    let mut sampled = 0;
    let mut skipped = Vec::new();
    for (index, &(from, to)) in windows.iter().enumerate() {
        let length = Time::new((to - from) * rate.den, rate.num)?;
        let scene_id = format!("{asset_id}-{index}");
        let converted = captions::to_scene(&captions::SceneRequest {
            document: document.clone(),
            scene: base(&scene_id, project, rate, length)?,
            scene_id: scene_id.clone(),
            offset: at(from)?,
            layouts: request.layouts.clone(),
            sampling: captions::Sampling::SampleStart,
            layer_prefix: "cue".into(),
            input_root: request.input_root.to_path_buf(),
        })?;
        for cue in converted["cues"].as_array().into_iter().flatten() {
            match cue["status"].as_str() {
                Some("sampled") => sampled += 1,
                Some("no_sampled_frame") => skipped.push(cue["cue_id"].clone()),
                _ => {}
            }
        }
        let window: Scene = serde_json::from_value(converted["scene"].clone())?;
        let part = scratch.0.join(format!("window-{index}.mkv"));
        scene::run(&window, request.input_root, &scratch.0, &part)?;
        let path = part
            .to_string_lossy()
            .replace('\\', "/")
            .replace('\'', "'\\''");
        list.push_str(&format!("file '{path}'\n"));
        // Each window's exact length; the join offsets the next window by it.
        if index + 1 < windows.len() {
            let micros = u128::from((to - from) * rate.den) * 1_000_000 / u128::from(rate.num);
            list.push_str(&format!(
                "duration {}.{:06}\n",
                micros / 1_000_000,
                micros % 1_000_000
            ));
        }
    }
    let listing = scratch.0.join("windows.txt");
    fs::write(&listing, list)?;
    let joined = scratch.0.join("overlay.mkv");
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
    args.push(listing.to_string_lossy().into_owned());
    args.extend(["-map", "0", "-c", "copy", "-f", "matroska"].map(str::to_owned));
    args.push(joined.to_string_lossy().into_owned());
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(3600))?;
    let (verified, alpha) = render::inspect_overlay(
        &joined,
        project.width,
        project.height,
        rate,
        &media::Uncontrolled,
    )?;
    let length = Time::new(total * rate.den, rate.num)?;
    if !alpha
        || verified.frames != total
        || verified.samples != length.units(Time::new(48_000, 1)?)?
    {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "The joined caption overlay does not have the planned frames, samples and alpha",
        ));
    }
    let bytes = fs::metadata(&joined)?.len();
    media::publish(&joined, &output)?;
    Ok(
        json!({"output":output,"frames":total,"windows":windows.len(),"start":start,"duration":length,
        "covers":duration,"cues":{"sampled_windows":sampled,"no_sampled_frame":skipped},
        "asset":{"id":asset_id,"path":output,"duration":length,"identity":{"sha256":verified.sha256,"bytes":bytes}},
        "next":format!("media.add the asset, add a video track with composite alpha_over, and place it at {} for {} s",start,duration)}),
    )
}

/// Save an atomic edit proposal containing bounded scene windows. No intermediate movie.
fn generated(
    request: &Request,
    rate: Time,
    start: Time,
    duration: Time,
    frames: u64,
    windows: &[(u64, u64)],
) -> Result<Value> {
    use crate::media::Control;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    let project = request.project;
    let prefix = request.asset_id.unwrap_or(&request.document.id);
    let track_id = request.track_id.unwrap_or(prefix);
    crate::tracks::id(track_id)?;
    let output = render::destination_extension(request.output, request.output_root, "json")?;
    if start.plus(duration)?.compare(project.duration()?)?.is_gt() {
        return Err(error(
            "INVALID_RANGE",
            "Generated captions end after the timeline; extend it explicitly first",
        ));
    }
    let edit = |value: Value| json!({"op":"tracks.edit","edit":value});
    let mut operations = Vec::new();
    if project.tracks.is_none() {
        operations.push(edit(
            json!({"op":"promote","video_track_id":format!("{prefix}-source-video"),
            "audio_track_id":format!("{prefix}-source-audio")}),
        ));
    }
    if let Some(track) = project
        .tracks
        .as_ref()
        .and_then(|a| a.tracks.iter().find(|t| t.id == track_id))
    {
        if track.kind != crate::tracks::Kind::Video
            || track.composite != crate::tracks::Composite::AlphaOver
            || !track.enabled
        {
            return Err(error(
                "INVALID_TRACKS",
                "Generated captions require an enabled video alpha_over track",
            )
            .with_path("track_id"));
        }
    } else {
        operations.push(edit(
            json!({"op":"add","track":{"id":track_id,"kind":"video",
            "locked":false,"enabled":true,"clips":[],"transitions":[],"composite":"alpha_over"}}),
        ));
    }
    if windows.len() + operations.len() > 1000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Generated captions require at most 1000 edits; split the requested range",
        ));
    }
    let control = media::Uncontrolled;
    control.phase("caption-scenes")?;
    control.total(windows.len() as u64);
    let mut dependencies = BTreeMap::new();
    let mut sampled = 0;
    let mut skipped = Vec::new();
    for (index, &(from, to)) in windows.iter().enumerate() {
        control.check()?;
        let scene_id = format!("{prefix}-{index}");
        let length = Time::new((to - from) * rate.den, rate.num)?;
        let at = start.plus(Time::new(from * rate.den, rate.num)?)?;
        let converted = captions::to_scene(&captions::SceneRequest {
            document: request.document.clone(),
            scene: base(&scene_id, project, rate, length)?,
            scene_id: scene_id.clone(),
            offset: at,
            layouts: request.layouts.clone(),
            sampling: captions::Sampling::SampleStart,
            layer_prefix: "cue".into(),
            input_root: request.input_root.to_path_buf(),
        })?;
        for cue in converted["cues"].as_array().into_iter().flatten() {
            match cue["status"].as_str() {
                Some("sampled") => sampled += 1,
                Some("no_sampled_frame") => skipped.push(cue["cue_id"].clone()),
                _ => {}
            }
        }
        let scene = crate::generated::SceneRef(Box::new(serde_json::from_value(
            converted["scene"].clone(),
        )?));
        for (source, bytes) in crate::generated::dependencies(&scene, request.input_root)? {
            if let Some(previous) = dependencies.insert(source.path, (source.sha256.clone(), bytes))
                && previous != (source.sha256, bytes)
            {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Caption source identity changed between windows",
                ));
            }
        }
        let covered = Time::new((to.min(frames) - from) * rate.den, rate.num)?;
        operations.push(edit(json!({"op":"place","track_id":track_id,"collision":"reject",
            "clip":{"id":scene_id,"scene":scene,"start":at,"source_in":Time::ZERO,"duration":covered}})));
        control.frames(index as u64 + 1)?;
    }
    // Prove the proposal applies atomically to this revision before publishing it. This checks
    // duplicate IDs, locked tracks, collisions, source clocks and the existing batch limits.
    project.apply(project.revision, serde_json::from_value(json!(operations))?)?;
    let artifact = json!({"schema_version":1,"kind":"generated_caption_operations","project_id":project.id,
        "expected_revision":project.revision,"track_id":track_id,"start":start,"duration":duration,
        "windows":windows.len(),"operations":operations});
    let bytes = serde_json::to_vec(&artifact)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Generated caption document exceeds the 16 MiB workspace document limit; split the requested range",
        ));
    }
    let scratch = scratch(&output)?;
    let temp = scratch.0.join("captions.json");
    let mut file = fs::File::create_new(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    let hash = format!("{:x}", Sha256::digest(&bytes));
    if media::file_hash(&temp)? != hash {
        return Err(error(
            "EXPORT_VALIDATION_FAILED",
            "Generated caption document bytes differ",
        ));
    }
    for (path, (hash, bytes)) in dependencies {
        if fs::metadata(&path)?.len() != bytes || media::file_hash(&path)? != hash {
            return Err(error(
                "MEDIA_CHANGED",
                "Caption source changed before publication",
            ));
        }
    }
    control.check()?;
    media::publish(&temp, &output)?;
    Ok(
        json!({"output":output,"sha256":hash,"bytes":bytes.len(),"compiled":false,
        "mode":"generated","frames":frames,"windows":windows.len(),"start":start,"duration":duration,
        "covers":duration,"cues":{"sampled_windows":sampled,"no_sampled_frame":skipped},
        "project_id":project.id,"expected_revision":project.revision,"track_id":track_id,
        "operations":{"file":output,"select":"operations"},
        "next":"Apply operations from the output document with session.apply at expected_revision; the saved clips render their scenes directly."}),
    )
}

fn scratch(output: &Path) -> Result<Scratch> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
        .as_nanos();
    let path = output
        .parent()
        .expect("validated parent")
        .join(format!(".cutbolt-captions-{}-{nonce}", std::process::id()));
    fs::create_dir(&path)?;
    Ok(Scratch(path))
}
