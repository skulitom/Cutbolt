//! Loudness of a timeline range for the whole mix and each enabled audio track, so an agent can
//! set clip gain against a target without rendering a delivery.
use crate::{Result, error, media, model::Project, render, time::Time, tracks::Kind};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Longest measured range: four hours. The audio streams from a scratch WAV, so memory does not
/// grow with length; the scratch file needs about 690 MB per hour.
const MAX_SECONDS: u64 = 4 * 3600;

/// Removes a private scratch directory when measuring ends, successfully or not.
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Meters for `[start, start + duration)` of the timeline (the rest of it when `duration` is
/// omitted): the mix of every enabled audio track and, with `per_track`, each one played alone.
pub fn inspect(
    project: &Project,
    input_root: &Path,
    start: Option<Time>,
    duration: Option<Time>,
    per_track: bool,
    curve: bool,
) -> Result<Value> {
    let start = start.unwrap_or(Time::ZERO);
    let duration = match duration {
        Some(duration) => duration,
        None => project.duration()?.minus(start)?,
    };
    if duration.compare(Time::new(MAX_SECONDS, 1)?)?.is_gt() {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!("Meters measure at most {MAX_SECONDS} s at a time; select a shorter range"),
        ));
    }
    let mix = measure(project, input_root, start, duration, curve)?;
    let mut tracks = Vec::new();
    if per_track && let Some(arrangement) = &project.tracks {
        for (index, track) in arrangement.tracks.iter().enumerate() {
            if !track.enabled || track.kind != Kind::Audio {
                continue;
            }
            let mut solo = project.clone();
            for (other, t) in solo
                .tracks
                .as_mut()
                .expect("tracks")
                .tracks
                .iter_mut()
                .enumerate()
            {
                if other != index && t.kind == Kind::Audio {
                    t.enabled = false;
                }
            }
            tracks.push(
                json!({"track_id":track.id,"meters":measure(&solo, input_root, start, duration, curve)?}),
            );
        }
    }
    Ok(json!({"start":start,"duration":duration,"mix":mix,"tracks":tracks}))
}

/// Render the range's audio exactly as an audio-only export would, then meter the PCM.
pub(crate) fn measure(
    project: &Project,
    input_root: &Path,
    start: Time,
    duration: Time,
    curve: bool,
) -> Result<Value> {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
        .as_nanos();
    let scratch = Scratch(
        std::env::temp_dir().join(format!("cutbolt-meters-{}-{nonce}", std::process::id())),
    );
    fs::create_dir(&scratch.0)?;
    let plan = render::plan_audio_range(
        project,
        input_root,
        &scratch.0,
        &scratch.0.join("mix.wav"),
        start,
        duration,
    )?;
    render::run_plan(
        &plan,
        &plan.output,
        Duration::from_secs(MAX_SECONDS),
        false,
        &media::Uncontrolled,
    )?;
    let (frames, mut meters, over_time) =
        crate::audio_processing::measure_wav(&plan.output, curve)?;
    if frames != plan.samples {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Measured audio does not match the timeline range",
        ));
    }
    for source in &plan.sources {
        if media::file_hash(&source.path)? != source.sha256 {
            return Err(error("MEDIA_CHANGED", "Source changed during measurement"));
        }
    }
    if let Some(over_time) = over_time {
        meters["over_time"] = over_time;
    }
    if let Some(dynamics) = plan.dynamics() {
        meters["dynamics"] = dynamics;
    }
    Ok(meters)
}
