//! Original recoverable camera selections projected into native reusable sequences.
use crate::{
    Result, error,
    model::Project,
    sequences,
    time::Time,
    tracks::{self, Arrangement, Kind, Track, TrackClip},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
const FPS: Time = Time { num: 25, den: 1 };
const AUDIO: Time = Time { num: 48000, den: 1 };

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Angle {
    pub id: String,
    pub sequence_id: String,
    pub start: Time,
    pub duration: Time,
    pub source_in: Time,
    pub audio_in: Time,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cut {
    pub id: String,
    pub at: Time,
    pub angle_id: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum AudioPolicy {
    Mute,
    FollowVideo,
    Fixed { angle_id: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Group {
    pub duration: Time,
    pub locked: bool,
    pub angles: Vec<Angle>,
    pub cuts: Vec<Cut>,
    pub audio: AudioPolicy,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    CutSet { cut: Cut },
    CutRemove { id: String },
    AngleSet { angle: Angle },
    AngleRemove { id: String },
    Audio { policy: AudioPolicy },
    Duration { duration: Time },
    State { locked: bool },
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_MULTICAM", message)
}
fn find<'a>(g: &'a Group, id: &str) -> Result<&'a Angle> {
    g.angles
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| error("MISSING_ANGLE", id))
}
fn interval(angle: &Angle, at: Time, duration: Time, kind: Kind, id: String) -> Result<TrackClip> {
    if at.compare(angle.start)?.is_lt()
        || at
            .plus(duration)?
            .compare(angle.start.plus(angle.duration)?)?
            .is_gt()
    {
        return Err(error(
            "ANGLE_COVERAGE",
            format!(
                "Angle {} does not cover the complete selected interval",
                angle.id
            ),
        ));
    }
    Ok(TrackClip {
        id,
        asset_id: String::new(),
        sequence_id: Some(angle.sequence_id.clone()),
        start: at,
        source_in: if kind == Kind::Video {
            angle.source_in
        } else {
            angle.audio_in
        }
        .plus(at.minus(angle.start)?)?,
        duration,
    })
}
pub(crate) fn project(group: &Group, project: &Project) -> Result<Arrangement> {
    if project.frame_rate.compare(FPS)?.is_ne() {
        return Err(error(
            "UNSUPPORTED_MULTICAM",
            "Multicam v1 uses the 25 fps reference clock",
        ));
    }
    group.duration.units(FPS)?;
    if group.duration.num == 0
        || !(2..=16).contains(&group.angles.len())
        || !(1..=128).contains(&group.cuts.len())
    {
        return Err(invalid(
            "Multicam requires a positive duration, 2..16 angles and 1..128 cuts",
        ));
    }
    let mut ids = BTreeSet::new();
    for a in &group.angles {
        tracks::id(&a.id)?;
        if !ids.insert(&a.id) {
            return Err(error("DUPLICATE_ID", &a.id));
        }
        for t in [a.start, a.duration, a.source_in] {
            t.units(FPS)?;
        }
        a.audio_in.units(AUDIO)?;
        let source = sequences::get(project, &a.sequence_id)?;
        if a.duration.num == 0
            || a.start.compare(group.duration)?.is_ge()
            || a.source_in
                .plus(a.duration)?
                .compare(source.arrangement.duration)?
                .is_gt()
            || a.audio_in
                .plus(a.duration)?
                .compare(source.arrangement.duration)?
                .is_gt()
        {
            return Err(error(
                "INVALID_RANGE",
                format!("Angle {} has an invalid source or coverage interval", a.id),
            ));
        }
    }
    ids.clear();
    for c in &group.cuts {
        tracks::id(&c.id)?;
        if c.id.len() > 120 {
            return Err(invalid("Cut IDs support at most 120 UTF-8 bytes"));
        }
        if !ids.insert(&c.id) {
            return Err(error("DUPLICATE_ID", &c.id));
        }
        c.at.units(FPS)?;
        find(group, &c.angle_id)?;
        if c.at.compare(group.duration)?.is_ge() {
            return Err(invalid("A cut must be before the group end"));
        }
    }
    let mut cuts: Vec<_> = group.cuts.iter().collect();
    cuts.sort_by(|a, b| a.at.compare(b.at).expect("validated"));
    if cuts[0].at.num != 0
        || cuts
            .windows(2)
            .any(|p| p[0].at.compare(p[1].at).expect("validated").is_eq())
    {
        return Err(invalid(
            "Cuts require one selection at zero and unique times",
        ));
    }
    let make_track = |id: &str, kind| Track {
        id: id.into(),
        kind,
        enabled: true,
        locked: group.locked,
        clips: vec![],
        transitions: vec![],
    };
    let mut video = make_track("multicam-video", Kind::Video);
    let mut audio = make_track("multicam-audio", Kind::Audio);
    for (i, c) in cuts.iter().enumerate() {
        let end = cuts.get(i + 1).map_or(group.duration, |c| c.at);
        let duration = end.minus(c.at)?;
        let a = find(group, &c.angle_id)?;
        video.clips.push(interval(
            a,
            c.at,
            duration,
            Kind::Video,
            format!("video-{}", c.id),
        )?);
        if matches!(group.audio, AudioPolicy::FollowVideo) {
            audio.clips.push(interval(
                a,
                c.at,
                duration,
                Kind::Audio,
                format!("audio-{}", c.id),
            )?);
        }
    }
    if let AudioPolicy::Fixed { angle_id } = &group.audio {
        audio.clips.push(interval(
            find(group, angle_id)?,
            Time::ZERO,
            group.duration,
            Kind::Audio,
            "audio-master".into(),
        )?);
    }
    let result = Arrangement {
        duration: group.duration,
        tracks: vec![video, audio],
        links: vec![],
    };
    result.validate(project)?;
    Ok(result)
}
pub(crate) fn validate(sequence: &sequences::Sequence, project_: &Project) -> Result<()> {
    if let Some(group) = &sequence.multicam
        && project(group, project_)? != sequence.arrangement
    {
        return Err(error(
            "MULTICAM_PROJECTION_MISMATCH",
            "Native tracks must match the saved camera selections; use multicam.edit",
        ));
    }
    Ok(())
}
pub(crate) fn create(project_: &mut Project, id: String, group: Group) -> Result<()> {
    if project_.sequences.iter().any(|s| s.id == id) {
        return Err(error("DUPLICATE_ID", id));
    }
    let arrangement = project(&group, project_)?;
    project_.sequences.push(sequences::Sequence {
        id,
        arrangement,
        multicam: Some(group),
    });
    Ok(())
}
pub(crate) fn edit(project_: &mut Project, id: &str, edit: Edit) -> Result<()> {
    sequences::check_dependents(project_, id)?;
    let mut group = sequences::get(project_, id)?
        .multicam
        .clone()
        .ok_or_else(|| invalid("Sequence is not a multicam group"))?;
    if group.locked && !matches!(edit, Edit::State { .. }) {
        return Err(error(
            "TRACK_LOCKED",
            "Unlock the multicam group before editing",
        ));
    }
    match edit {
        Edit::CutSet { cut } => {
            if let Some(old) = group.cuts.iter_mut().find(|c| c.id == cut.id) {
                *old = cut;
            } else {
                group.cuts.push(cut);
            }
        }
        Edit::CutRemove { id } => {
            let index = group
                .cuts
                .iter()
                .position(|c| c.id == id)
                .ok_or_else(|| error("MISSING_CUT", id))?;
            group.cuts.remove(index);
        }
        Edit::AngleSet { angle } => {
            if let Some(old) = group.angles.iter_mut().find(|a| a.id == angle.id) {
                *old = angle;
            } else {
                group.angles.push(angle);
            }
        }
        Edit::AngleRemove { id } => {
            find(&group, &id)?;
            if group.cuts.iter().any(|c| c.angle_id == id)
                || matches!(&group.audio, AudioPolicy::Fixed { angle_id } if *angle_id == id)
            {
                return Err(error("ANGLE_IN_USE", id));
            }
            group.angles.retain(|a| a.id != id);
        }
        Edit::Audio { policy } => group.audio = policy,
        Edit::Duration { duration } => group.duration = duration,
        Edit::State { locked } => group.locked = locked,
    }
    let arrangement = project(&group, project_)?;
    let sequence = project_
        .sequences
        .iter_mut()
        .find(|s| s.id == id)
        .expect("existing group");
    sequence.arrangement = arrangement;
    sequence.multicam = Some(group);
    Ok(())
}
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"operations":["multicam.create","multicam.edit"],"maximum_angles":16,"minimum_angles":2,"maximum_cuts":128,"clock":25,"sync":"explicit_video_and_sample_audio_source_windows","audio":["mute","follow_video","fixed"],"storage":"editable_sequence_with_validated_native_projection","alternate_angles":"retained","alignment":"sync.inspect_with_declared_audio_windows_or_timecode","drift_correction":"explicit_media.conform_constant_rate","network":false})
}
