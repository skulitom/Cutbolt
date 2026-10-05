//! A whole caption document rendered as one transparent overlay asset for an `alpha_over` track.
//! A document can run for hours, longer than any scene, so the overlay is compiled as a series of
//! caption windows, each an ordinary caption scene, and the windows are joined losslessly. Windows
//! stay at ten seconds and fifteen cues, well inside the scene limits, so each one's work is small.
use crate::{
    Result,
    captions::{self, Document, Layout},
    error, media,
    model::Project,
    render,
    scene::{self, Scene},
    time::Time,
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Caption layers per window: the scene limit less the blank layer every window carries.
const CUES_PER_WINDOW: usize = 15;

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
    let output = render::destination(request.output, request.output_root)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
        .as_nanos();
    let scratch = Scratch(
        output
            .parent()
            .expect("validated parent")
            .join(format!(".cutbolt-captions-{}-{nonce}", std::process::id())),
    );
    fs::create_dir(&scratch.0)?;
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
