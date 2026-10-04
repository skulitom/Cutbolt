//! Native placed tracks and transactional linked editing. Times are exact rationals.
use crate::{
    Result, error,
    model::{Clip, Project},
    time::Time,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Video,
    Audio,
}
impl Kind {
    pub(crate) fn clock(self, fps: Time) -> Time {
        if self == Self::Video {
            fps
        } else {
            Time { num: 48000, den: 1 }
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackClip {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub asset_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    pub start: Time,
    pub source_in: Time,
    pub duration: Time,
}
impl TrackClip {
    pub(crate) fn source_duration(&self, project: &Project) -> Result<Time> {
        match (&self.sequence_id, self.asset_id.is_empty()) {
            (Some(id), true) => Ok(crate::sequences::get(project, id)?.arrangement.duration),
            (None, false) => project
                .assets
                .iter()
                .find(|a| a.id == self.asset_id)
                .map(|a| a.duration)
                .ok_or_else(|| error("MISSING_MEDIA", &self.asset_id)),
            _ => Err(error(
                "INVALID_CLIP_SOURCE",
                "A track clip requires exactly one nonempty asset_id or sequence_id",
            )),
        }
    }
    pub(crate) fn end(&self) -> Result<Time> {
        self.start.plus(self.duration)
    }
    pub(crate) fn legacy(&self) -> Clip {
        Clip {
            id: self.id.clone(),
            asset_id: self.sequence_id.is_none().then(|| self.asset_id.clone()),
            gap: false,
            source_in: self.source_in,
            duration: self.duration,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    pub id: String,
    pub kind: Kind,
    pub locked: bool,
    pub enabled: bool,
    pub clips: Vec<TrackClip>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Dissolve,
    DipBlack,
    WipeLeft,
    WipeRight,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    pub id: String,
    pub left_id: String,
    pub right_id: String,
    pub before: Time,
    pub after: Time,
    pub kind: TransitionKind,
}
impl Track {
    fn continuous(&self, start: Time, end: Time, anchor: &TrackClip) -> Result<bool> {
        let mut cursor = start;
        while cursor.compare(end)?.is_lt() {
            let Some(clip) = self.clips.iter().find(|c| {
                c.start.compare(cursor).is_ok_and(|v| v.is_le())
                    && c.end()
                        .and_then(|e| e.compare(cursor))
                        .is_ok_and(|v| v.is_gt())
            }) else {
                return Ok(false);
            };
            if clip.asset_id != anchor.asset_id
                || clip.sequence_id != anchor.sequence_id
                || clip
                    .source_in
                    .plus(anchor.start)?
                    .compare(anchor.source_in.plus(clip.start)?)?
                    .is_ne()
            {
                return Ok(false);
            }
            let last = clip.end()?;
            cursor = if last.compare(end)?.is_lt() {
                last
            } else {
                end
            };
        }
        Ok(true)
    }
    pub(crate) fn endpoints(&self, effect: &Transition) -> Result<(&TrackClip, &TrackClip)> {
        let left = self.clips.iter().find(|c| c.id == effect.left_id);
        let right = self.clips.iter().find(|c| c.id == effect.right_id);
        match (left, right) {
            (Some(a), Some(b)) if a.id != b.id => Ok((a, b)),
            _ => Err(error(
                "INVALID_TRANSITION",
                "Transition requires two different clips on its track",
            )),
        }
    }
    pub(crate) fn interval(&self, effect: &Transition) -> Result<(Time, Time)> {
        let (_, right) = self.endpoints(effect)?;
        Ok((
            right.start.minus(effect.before)?,
            right.start.plus(effect.after)?,
        ))
    }
    pub(crate) fn transition_at(&self, time: Time) -> Result<Option<&Transition>> {
        for effect in &self.transitions {
            let (start, end) = self.interval(effect)?;
            if !time.compare(start)?.is_lt() && time.compare(end)?.is_lt() {
                return Ok(Some(effect));
            }
        }
        Ok(None)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    pub clip_id: String,
    pub start: Time,
    pub source_in: Time,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub id: String,
    pub members: Vec<Anchor>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Arrangement {
    pub duration: Time,
    pub tracks: Vec<Track>,
    pub links: Vec<Link>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    Reject,
    ReplaceClips,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Linked {
    Include,
    RejectPartial,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shift {
    pub backward: bool,
    pub amount: Time,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub clip_id: String,
    pub track_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    Split(crate::track_edit::Split),
    Slip(crate::track_edit::Slip),
    Roll(crate::track_edit::Roll),
    Slide(crate::track_edit::Slide),
    Insert(crate::track_edit::Insert),
    Overwrite(crate::track_edit::Overwrite),
    RippleDelete(crate::track_edit::RippleDelete),
    TransitionSet {
        track_id: String,
        transition: Transition,
    },
    TransitionRemove {
        track_id: String,
        id: String,
    },
    Create {
        duration: Time,
    },
    Promote {
        video_track_id: String,
        audio_track_id: String,
    },
    Add {
        track: Track,
    },
    State {
        track_id: String,
        locked: bool,
        enabled: bool,
    },
    Order {
        track_ids: Vec<String>,
    },
    Duration {
        duration: Time,
    },
    Place {
        track_id: String,
        clip: TrackClip,
        collision: Collision,
    },
    Move {
        clip_ids: Vec<String>,
        shift: Shift,
        targets: Vec<Target>,
        links: Linked,
        collision: Collision,
    },
    Remove {
        clip_ids: Vec<String>,
        links: Linked,
    },
    Link {
        id: String,
        clip_ids: Vec<String>,
    },
    Unlink {
        id: String,
    },
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_TRACKS", message)
}
pub(crate) fn id(value: &str) -> Result<()> {
    if value.trim().is_empty() || value.len() > 128 {
        return Err(error(
            "INVALID_ID",
            "Track, clip, link and transition IDs require 1..128 bytes",
        ));
    }
    Ok(())
}
pub(crate) fn unlocked(track: &Track) -> Result<()> {
    if track.locked {
        return Err(error(
            "TRACK_LOCKED",
            format!("Track {} is locked", track.id),
        ));
    }
    Ok(())
}
fn overlaps(a: &TrackClip, b: &TrackClip) -> Result<bool> {
    Ok(a.start.compare(b.end()?)?.is_lt() && b.start.compare(a.end()?)?.is_lt())
}
impl Arrangement {
    pub(crate) fn validate(&self, project: &Project) -> Result<()> {
        self.duration.units(project.frame_rate)?;
        if self.tracks.len() > 32
            || self.tracks.iter().map(|t| t.clips.len()).sum::<usize>() > 1000
            || self.links.len() > 500
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Tracks support 32 tracks, 1000 clips and 500 links",
            ));
        }
        let mut track_ids = BTreeSet::new();
        let mut transition_ids = BTreeSet::new();
        let mut clips = BTreeMap::new();
        for track in &self.tracks {
            id(&track.id)?;
            if !track_ids.insert(&track.id) {
                return Err(error("DUPLICATE_ID", &track.id));
            }
            for clip in &track.clips {
                id(&clip.id)?;
                if clips.insert(&clip.id, (track, clip)).is_some() {
                    return Err(error("DUPLICATE_ID", &clip.id));
                }
                for t in [clip.start, clip.source_in, clip.duration] {
                    t.units(track.kind.clock(project.frame_rate))?;
                }
                let source_duration = clip.source_duration(project)?;
                if clip.duration.num == 0
                    || clip.end()?.compare(self.duration)?.is_gt()
                    || clip
                        .source_in
                        .plus(clip.duration)?
                        .compare(source_duration)?
                        .is_gt()
                {
                    return Err(error(
                        "INVALID_RANGE",
                        format!("Clip {} exceeds its source or timeline", clip.id),
                    ));
                }
            }
            let mut ordered: Vec<_> = track.clips.iter().collect();
            ordered.sort_by(|a, b| a.start.compare(b.start).expect("validated times"));
            for pair in ordered.windows(2) {
                if overlaps(pair[0], pair[1])? {
                    return Err(error(
                        "CLIP_COLLISION",
                        format!(
                            "Clips {} and {} overlap on {}",
                            pair[0].id, pair[1].id, track.id
                        ),
                    ));
                }
            }
            if track.transitions.len() > 1000 {
                return Err(error("LIMIT_EXCEEDED", "Too many transitions"));
            }
            let mut intervals = Vec::new();
            for effect in &track.transitions {
                id(&effect.id)?;
                if !transition_ids.insert(&effect.id) {
                    return Err(error("DUPLICATE_ID", &effect.id));
                }
                let clock = track.kind.clock(project.frame_rate);
                effect.before.units(clock)?;
                effect.after.units(clock)?;
                let (left, right) = track.endpoints(effect)?;
                if left.end()?.compare(right.start)?.is_ne()
                    || effect.before.plus(effect.after)?.num == 0
                {
                    return Err(error(
                        "INVALID_TRANSITION",
                        "Transition needs adjacent endpoints and a nonempty interval",
                    ));
                }
                let (begin, end) = track.interval(effect).map_err(|_| {
                    error(
                        "INVALID_TRANSITION",
                        "Transition starts before the timeline",
                    )
                })?;
                if !track.continuous(begin, right.start, left)?
                    || !track.continuous(right.start, end, right)?
                {
                    return Err(error(
                        "INVALID_TRANSITION",
                        "Transition interval crosses a gap or discontinuous source mapping",
                    ));
                }
                if track.kind == Kind::Audio
                    && matches!(
                        effect.kind,
                        TransitionKind::WipeLeft | TransitionKind::WipeRight
                    )
                {
                    return Err(error(
                        "INVALID_TRANSITION",
                        "Audio supports dissolve and dip_black",
                    ));
                }
                if effect.before.compare(right.source_in)?.is_gt()
                    || left
                        .source_in
                        .plus(left.duration)?
                        .plus(effect.after)?
                        .compare(left.source_duration(project)?)?
                        .is_gt()
                {
                    return Err(error(
                        "INSUFFICIENT_HANDLES",
                        "Transition exceeds incoming preroll or outgoing postroll",
                    ));
                }
                intervals.push(track.interval(effect)?);
            }
            intervals.sort_by(|a, b| a.0.compare(b.0).expect("validated times"));
            for pair in intervals.windows(2) {
                if pair[0].1.compare(pair[1].0)?.is_gt() {
                    return Err(error(
                        "TRANSITION_COLLISION",
                        "Transitions on a track cannot overlap",
                    ));
                }
            }
        }
        let mut linked = BTreeSet::new();
        let mut links = BTreeSet::new();
        for link in &self.links {
            id(&link.id)?;
            if !links.insert(&link.id) {
                return Err(error("DUPLICATE_ID", &link.id));
            }
            if !(2..=32).contains(&link.members.len()) {
                return Err(invalid("Links require 2..32 members"));
            }
            let mut first: Option<(Time, Time)> = None;
            for anchor in &link.members {
                let (track, clip) = clips
                    .get(&anchor.clip_id)
                    .ok_or_else(|| error("MISSING_CLIP", &anchor.clip_id))?;
                if !linked.insert(&anchor.clip_id) {
                    return Err(invalid("A clip can belong to only one link"));
                }
                anchor.start.units(track.kind.clock(project.frame_rate))?;
                anchor
                    .source_in
                    .units(track.kind.clock(project.frame_rate))?;
                let a = clip.start.plus(anchor.source_in)?;
                let b = clip.source_in.plus(anchor.start)?;
                if let Some((x, y)) = first {
                    if a.plus(y)?.compare(x.plus(b)?)?.is_ne() {
                        return Err(error(
                            "SYNC_CONFLICT",
                            format!("Link {} would lose source synchronization", link.id),
                        ));
                    }
                } else {
                    first = Some((a, b));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn track(&self, name: &str) -> Result<usize> {
        self.tracks
            .iter()
            .position(|t| t.id == name)
            .ok_or_else(|| error("MISSING_TRACK", name))
    }
    pub(crate) fn locate(&self, name: &str) -> Result<(usize, usize)> {
        self.tracks
            .iter()
            .enumerate()
            .find_map(|(t, track)| {
                track
                    .clips
                    .iter()
                    .position(|c| c.id == name)
                    .map(|c| (t, c))
            })
            .ok_or_else(|| error("MISSING_CLIP", name))
    }
    pub(crate) fn selection(&self, names: &[String], policy: Linked) -> Result<BTreeSet<String>> {
        if names.is_empty() || names.len() > 1000 {
            return Err(invalid("Select 1..1000 clips"));
        }
        let mut selected: BTreeSet<_> = names.iter().cloned().collect();
        if selected.len() != names.len() {
            return Err(invalid("Duplicate selected clips"));
        }
        for name in names {
            self.locate(name)?;
        }
        for link in &self.links {
            let count = link
                .members
                .iter()
                .filter(|m| selected.contains(&m.clip_id))
                .count();
            if count > 0 && count < link.members.len() {
                if matches!(policy, Linked::RejectPartial) {
                    return Err(error(
                        "LINKED_SELECTION",
                        format!("Select all of link {} or include linked partners", link.id),
                    ));
                }
                selected.extend(link.members.iter().map(|m| m.clip_id.clone()));
            }
        }
        Ok(selected)
    }
    pub(crate) fn check_locks(&self, selection: &BTreeSet<String>) -> Result<()> {
        for name in selection {
            let (t, _) = self.locate(name)?;
            unlocked(&self.tracks[t])?;
        }
        Ok(())
    }
    fn erase(&mut self, selection: &BTreeSet<String>) {
        for track in &mut self.tracks {
            track.clips.retain(|c| !selection.contains(&c.id));
            track
                .transitions
                .retain(|e| !selection.contains(&e.left_id) && !selection.contains(&e.right_id));
        }
        self.links
            .retain(|link| !link.members.iter().any(|m| selection.contains(&m.clip_id)));
    }
    fn resolve(&mut self, moving: &BTreeSet<String>, policy: Collision) -> Result<()> {
        let mut victims = BTreeSet::new();
        for track in &self.tracks {
            for a in track.clips.iter().filter(|c| moving.contains(&c.id)) {
                for b in track.clips.iter().filter(|c| c.id != a.id) {
                    if overlaps(a, b)? {
                        match (moving.contains(&a.id), moving.contains(&b.id)) {
                            (true, false) => {
                                victims.insert(b.id.clone());
                            }
                            (false, true) => {
                                victims.insert(a.id.clone());
                            }
                            _ => {
                                return Err(error(
                                    "CLIP_COLLISION",
                                    "Selected placements overlap each other",
                                ));
                            }
                        }
                    }
                }
            }
        }
        if victims.is_empty() {
            return Ok(());
        }
        if matches!(policy, Collision::Reject) {
            return Err(error(
                "CLIP_COLLISION",
                format!(
                    "Placement collides with {}",
                    victims.iter().cloned().collect::<Vec<_>>().join(", ")
                ),
            ));
        }
        let victims = self.selection(&victims.into_iter().collect::<Vec<_>>(), Linked::Include)?;
        if victims.iter().any(|v| moving.contains(v)) {
            return Err(error(
                "SYNC_CONFLICT",
                "Replacement would remove a moving linked partner",
            ));
        }
        self.check_locks(&victims)?;
        self.erase(&victims);
        Ok(())
    }
    pub(crate) fn visible(&self, time: Time) -> Result<Option<(&Track, &TrackClip)>> {
        for track in self
            .tracks
            .iter()
            .rev()
            .filter(|t| t.kind == Kind::Video && t.enabled)
        {
            for clip in &track.clips {
                if !time.compare(clip.start)?.is_lt() && time.compare(clip.end()?)?.is_lt() {
                    return Ok(Some((track, clip)));
                }
            }
        }
        Ok(None)
    }
}
fn arrangement(project: &mut Project) -> Result<&mut Arrangement> {
    project
        .tracks
        .as_mut()
        .ok_or_else(|| invalid("Create tracks or explicitly promote the sequential timeline first"))
}

pub(crate) fn edit(project: &mut Project, edit: Edit) -> Result<()> {
    match edit {
        Edit::Split(e) => crate::track_edit::split(arrangement(project)?, e)?,
        Edit::Slip(e) => crate::track_edit::slip(arrangement(project)?, e)?,
        Edit::Roll(e) => crate::track_edit::roll(arrangement(project)?, e)?,
        Edit::Slide(e) => crate::track_edit::slide(arrangement(project)?, e)?,
        Edit::Insert(e) => {
            let fps = project.frame_rate;
            crate::track_edit::insert(arrangement(project)?, fps, e)?;
        }
        Edit::Overwrite(e) => {
            let fps = project.frame_rate;
            crate::track_edit::overwrite(arrangement(project)?, fps, e)?;
        }
        Edit::RippleDelete(e) => {
            let fps = project.frame_rate;
            crate::track_edit::ripple(arrangement(project)?, fps, e)?;
        }
        Edit::TransitionSet {
            track_id,
            transition,
        } => {
            let a = arrangement(project)?;
            let index = a.track(&track_id)?;
            unlocked(&a.tracks[index])?;
            let effects = &mut a.tracks[index].transitions;
            if let Some(old) = effects.iter_mut().find(|e| e.id == transition.id) {
                *old = transition;
            } else {
                effects.push(transition);
            }
        }
        Edit::TransitionRemove { track_id, id } => {
            let a = arrangement(project)?;
            let index = a.track(&track_id)?;
            unlocked(&a.tracks[index])?;
            let effects = &mut a.tracks[index].transitions;
            let old = effects
                .iter()
                .position(|e| e.id == id)
                .ok_or_else(|| error("MISSING_TRANSITION", &id))?;
            effects.remove(old);
        }
        Edit::Create { duration } => {
            if project.tracks.is_some() || !project.clips.is_empty() {
                return Err(invalid("Create requires an empty sequential timeline"));
            }
            project.tracks = Some(Arrangement {
                duration,
                tracks: vec![],
                links: vec![],
            });
        }
        Edit::Promote {
            video_track_id,
            audio_track_id,
        } => {
            if project.tracks.is_some() {
                return Err(invalid("Project already has tracks"));
            }
            let mut a = Arrangement {
                duration: project.duration()?,
                tracks: vec![
                    Track {
                        id: video_track_id,
                        kind: Kind::Video,
                        locked: false,
                        enabled: true,
                        clips: vec![],
                        transitions: vec![],
                    },
                    Track {
                        id: audio_track_id,
                        kind: Kind::Audio,
                        locked: false,
                        enabled: true,
                        clips: vec![],
                        transitions: vec![],
                    },
                ],
                links: vec![],
            };
            let mut start = Time::ZERO;
            let mut ids: BTreeSet<_> = project.clips.iter().map(|c| c.id.clone()).collect();
            let mut suffix = 0;
            for clip in &project.clips {
                if let Some(asset_id) = &clip.asset_id {
                    let mut audio_id = format!("promoted-audio-{suffix}");
                    while ids.contains(&audio_id) {
                        suffix += 1;
                        audio_id = format!("promoted-audio-{suffix}");
                    }
                    ids.insert(audio_id.clone());
                    suffix += 1;
                    let video = TrackClip {
                        id: clip.id.clone(),
                        asset_id: asset_id.clone(),
                        sequence_id: None,
                        start,
                        source_in: clip.source_in,
                        duration: clip.duration,
                    };
                    let audio = TrackClip {
                        id: audio_id,
                        ..video.clone()
                    };
                    a.links.push(Link {
                        id: format!("promoted-link-{}", a.links.len()),
                        members: [&video, &audio]
                            .map(|c| Anchor {
                                clip_id: c.id.clone(),
                                start: c.start,
                                source_in: c.source_in,
                            })
                            .to_vec(),
                    });
                    a.tracks[0].clips.push(video);
                    a.tracks[1].clips.push(audio);
                }
                start = start.plus(clip.duration)?;
            }
            project.clips.clear();
            project.tracks = Some(a);
        }
        Edit::Add { track } => {
            if !track.clips.is_empty() || !track.transitions.is_empty() {
                return Err(invalid(
                    "Add an empty track, then place clips and transitions explicitly",
                ));
            }
            arrangement(project)?.tracks.push(track);
        }
        Edit::State {
            track_id,
            locked,
            enabled,
        } => {
            let a = arrangement(project)?;
            let index = a.track(&track_id)?;
            let track = &mut a.tracks[index];
            if track.locked && track.enabled != enabled {
                return Err(error(
                    "TRACK_LOCKED",
                    "Unlock a track before changing its enabled state",
                ));
            }
            track.locked = locked;
            track.enabled = enabled;
        }
        Edit::Order { track_ids } => {
            let a = arrangement(project)?;
            if track_ids.len() != a.tracks.len()
                || track_ids.iter().collect::<BTreeSet<_>>().len() != track_ids.len()
            {
                return Err(invalid("Supply every track ID once"));
            }
            let mut tracks = Vec::new();
            for (new, id) in track_ids.iter().enumerate() {
                let old = a.track(id)?;
                if old != new {
                    unlocked(&a.tracks[old])?;
                }
                tracks.push(a.tracks[old].clone());
            }
            a.tracks = tracks;
        }
        Edit::Duration { duration } => arrangement(project)?.duration = duration,
        Edit::Place {
            track_id,
            clip,
            collision,
        } => {
            let a = arrangement(project)?;
            let index = a.track(&track_id)?;
            unlocked(&a.tracks[index])?;
            if a.locate(&clip.id).is_ok() {
                return Err(error("DUPLICATE_ID", &clip.id));
            }
            let moving = BTreeSet::from([clip.id.clone()]);
            a.tracks[index].clips.push(clip);
            a.resolve(&moving, collision)?;
        }
        Edit::Move {
            clip_ids,
            shift,
            targets,
            links,
            collision,
        } => {
            shift.amount.validate()?;
            let a = arrangement(project)?;
            let moving = a.selection(&clip_ids, links)?;
            a.check_locks(&moving)?;
            let mut target_map = BTreeMap::new();
            for target in targets {
                if !moving.contains(&target.clip_id)
                    || target_map
                        .insert(target.clip_id.clone(), a.track(&target.track_id)?)
                        .is_some()
                {
                    return Err(invalid(
                        "Targets must uniquely name selected or included clips",
                    ));
                }
            }
            let mut moved = Vec::new();
            for name in &moving {
                let (old, c) = a.locate(name)?;
                let new = *target_map.get(name).unwrap_or(&old);
                unlocked(&a.tracks[new])?;
                if a.tracks[new].kind != a.tracks[old].kind {
                    return Err(invalid(
                        "Cannot move a clip between video and audio track types",
                    ));
                }
                let mut clip = a.tracks[old].clips[c].clone();
                clip.start = if shift.backward {
                    clip.start.minus(shift.amount)?
                } else {
                    clip.start.plus(shift.amount)?
                };
                moved.push((new, clip));
            }
            for track in &mut a.tracks {
                track.clips.retain(|c| !moving.contains(&c.id));
            }
            for (t, clip) in moved {
                a.tracks[t].clips.push(clip);
            }
            let mut effects = Vec::new();
            for t in 0..a.tracks.len() {
                for effect in std::mem::take(&mut a.tracks[t].transitions) {
                    effects.push((t, effect));
                }
            }
            for (old, effect) in effects {
                let left = a.locate(&effect.left_id)?.0;
                let right = a.locate(&effect.right_id)?.0;
                let target = if left == right { left } else { old };
                a.tracks[target].transitions.push(effect);
            }
            a.resolve(&moving, collision)?;
        }
        Edit::Remove { clip_ids, links } => {
            let a = arrangement(project)?;
            let selected = a.selection(&clip_ids, links)?;
            a.check_locks(&selected)?;
            a.erase(&selected);
        }
        Edit::Link { id, clip_ids } => {
            let a = arrangement(project)?;
            let selected = a.selection(&clip_ids, Linked::RejectPartial)?;
            a.check_locks(&selected)?;
            let mut members = Vec::new();
            for name in selected {
                let (t, c) = a.locate(&name)?;
                let c = &a.tracks[t].clips[c];
                members.push(Anchor {
                    clip_id: name,
                    start: c.start,
                    source_in: c.source_in,
                });
            }
            a.links.push(Link { id, members });
        }
        Edit::Unlink { id } => {
            let a = arrangement(project)?;
            let index = a
                .links
                .iter()
                .position(|l| l.id == id)
                .ok_or_else(|| error("MISSING_LINK", id))?;
            a.check_locks(
                &a.links[index]
                    .members
                    .iter()
                    .map(|m| m.clip_id.clone())
                    .collect(),
            )?;
            a.links.remove(index);
        }
    }
    Ok(())
}

/// A transient range snapshot for preview/export; the caller's saved project is untouched.
pub(crate) fn range(project: &Project, start: Time, duration: Time) -> Result<Project> {
    let end = start.plus(duration)?;
    let mut selected = project.clone();
    if let Some(a) = &mut selected.tracks {
        a.duration = duration;
        for track in &mut a.tracks {
            let mut clips = Vec::new();
            for clip in &track.clips {
                let x = if start.compare(clip.start)?.is_gt() {
                    start
                } else {
                    clip.start
                };
                let last = clip.end()?;
                let y = if end.compare(last)?.is_lt() {
                    end
                } else {
                    last
                };
                if x.compare(y)?.is_lt() {
                    let mut c = clip.clone();
                    c.start = x.minus(start)?;
                    c.source_in = c.source_in.plus(x.minus(clip.start)?)?;
                    c.duration = y.minus(x)?;
                    clips.push(c);
                }
            }
            track.clips = clips;
        }
        let retained: BTreeMap<_, _> = a
            .tracks
            .iter()
            .flat_map(|t| t.clips.iter())
            .map(|c| {
                (
                    c.id.clone(),
                    Anchor {
                        clip_id: c.id.clone(),
                        start: c.start,
                        source_in: c.source_in,
                    },
                )
            })
            .collect();
        for link in &mut a.links {
            link.members = link
                .members
                .iter()
                .filter_map(|m| retained.get(&m.clip_id).cloned())
                .collect();
        }
        a.links.retain(|l| l.members.len() >= 2);
    } else {
        selected.clips.clear();
        let mut offset = Time::ZERO;
        for clip in &project.clips {
            let last = offset.plus(clip.duration)?;
            let x = if start.compare(offset)?.is_gt() {
                start
            } else {
                offset
            };
            let y = if end.compare(last)?.is_lt() {
                end
            } else {
                last
            };
            if x.compare(y)?.is_lt() {
                let mut c = clip.clone();
                c.advance_source(x.minus(offset)?)?;
                c.duration = y.minus(x)?;
                selected.clips.push(c);
            }
            offset = last;
        }
    }
    selected.validate()?;
    Ok(selected)
}

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"profile":"native-tracks-v1","operation":"tracks.edit","edits":["transition_set","transition_remove","split","slip","roll","slide","insert","overwrite","ripple_delete","create","promote","add","state","order","duration","place","move","remove","link","unlink"],"maximum_tracks":32,"maximum_model_clips":1000,"maximum_render_clips":64,"render_frame_rate":25,"audio_clock":48000,"video":"highest_enabled_opaque_track","audio":"sum_enabled_stereo_tracks_then_clip_pcm16","gaps":"implicit_black_and_silence_with_explicit_duration","collision":["reject","replace_clips"],"replacement":"whole_colliding_clips_and_linked_partners;not_interval_overwrite","links":["include","reject_partial"],"locks":"source_target_and_replaced_partner_tracks","source_profile":"reference_FFV1_RGB8_PCM16_stereo","supported_saved_sessions":true,"transitions":{"video":["dissolve","dip_black","wipe_left","wipe_right"],"audio":["dissolve","dip_black"],"handles":"explicit_before_cut_and_after_cut","sampling":"frame_or_sample_centers","video_rounding":"nearest_ties_up","audio_rounding":"nearest_ties_away_from_zero_before_track_mixing","overlapping_intervals":false},"ripple":true,"boundary_edits":{"fragment_ids":"explicit_right_clip_and_right_link_maps","link_anchors":"preserved","span_selection":"whole_linked_partner_tracks","end_policy":["keep","resize"],"transition_policy":["reject_affected","remove_affected"],"split_transitions":"preserve_clock_across_continuous_source_fragments"},"network":false})
}
