use crate::{Result, error, time::Time};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    pub id: String,
    pub path: String,
    pub duration: Time,
    #[serde(default, skip_serializing_if = "crate::registry::Metadata::is_empty")]
    pub metadata: crate::registry::Metadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::registry::Identity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<crate::proxy::Binding>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub gap: bool,
    pub source_in: Time,
    pub duration: Time,
}
fn is_false(value: &bool) -> bool {
    !value
}
impl Clip {
    pub(crate) fn advance_source(&mut self, offset: Time) -> Result<()> {
        if !self.gap {
            self.source_in = self.source_in.plus(offset)?;
        }
        Ok(())
    }
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub id: String,
    pub revision: u64,
    pub width: u32,
    pub height: u32,
    pub frame_rate: Time,
    pub assets: Vec<Asset>,
    pub clips: Vec<Clip>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<crate::tracks::Arrangement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<crate::sequences::Sequence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_scale: Option<u32>,
}

#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
pub enum Operation {
    #[serde(rename = "media.paths")]
    Paths {
        asset_id: String,
        path: String,
        #[serde(default)]
        proxy_path: Option<String>,
    },
    #[serde(rename = "transcript.cut")]
    TranscriptCut {
        document: Box<crate::transcript::Document>,
        plan: crate::transcript_cut::Plan,
    },
    #[serde(rename = "multicam.create")]
    MulticamCreate {
        id: String,
        group: crate::multicam::Group,
    },
    #[serde(rename = "multicam.edit")]
    MulticamEdit {
        id: String,
        edit: crate::multicam::Edit,
    },
    #[serde(rename = "sequence.create")]
    SequenceCreate { id: String, duration: Time },
    #[serde(rename = "sequence.edit")]
    SequenceEdit {
        id: String,
        edit: crate::tracks::Edit,
    },
    #[serde(rename = "sequence.remove")]
    SequenceRemove { id: String },
    #[serde(rename = "tracks.edit")]
    Tracks { edit: crate::tracks::Edit },
    #[serde(rename = "media.proxy.attach")]
    ProxyAttach {
        asset_id: String,
        proxy: crate::proxy::Binding,
    },
    #[serde(rename = "media.proxy.detach")]
    ProxyDetach { asset_id: String },
    #[serde(rename = "media.proxy.relink")]
    ProxyRelink { asset_id: String, path: String },
    #[serde(rename = "preview.proxy")]
    PreviewProxy { scale: Option<u32> },
    #[serde(rename = "media.add")]
    AddMedia { asset: Asset },
    #[serde(rename = "media.metadata")]
    Metadata {
        asset_id: String,
        metadata: crate::registry::Metadata,
    },
    #[serde(rename = "media.bind")]
    Bind {
        asset_id: String,
        identity: crate::registry::Identity,
    },
    #[serde(rename = "media.relink")]
    Relink { asset_id: String, path: String },
    #[serde(rename = "clip.append")]
    Append { clip: Clip },
    #[serde(rename = "clip.split")]
    Split {
        clip_id: String,
        new_clip_id: String,
        offset: Time,
    },
    #[serde(rename = "clip.trim")]
    Trim {
        clip_id: String,
        source_in: Time,
        duration: Time,
    },
    #[serde(rename = "clip.remove")]
    Remove { clip_id: String },
    #[serde(rename = "clip.move")]
    Move { clip_id: String, to_index: usize },
    #[serde(rename = "clip.insert")]
    Insert {
        at: Time,
        clip: Clip,
        right_id: Option<String>,
    },
    #[serde(rename = "clip.overwrite")]
    Overwrite {
        at: Time,
        clip: Clip,
        right_id: Option<String>,
    },
    #[serde(rename = "timeline.ripple_delete")]
    RippleDelete {
        start: Time,
        duration: Time,
        right_id: Option<String>,
    },
    #[serde(rename = "clip.slip")]
    Slip { clip_id: String, source_in: Time },
    #[serde(rename = "clip.roll")]
    Roll {
        left_id: String,
        left_duration: Time,
    },
    #[serde(rename = "clip.slide")]
    Slide {
        clip_id: String,
        previous_duration: Time,
    },
}

fn valid_id(id: &str) -> Result<()> {
    if id.trim().is_empty() || id.len() > 128 {
        return Err(error("INVALID_ID", "IDs must contain 1-128 bytes"));
    }
    Ok(())
}

impl Project {
    pub fn new(id: String, width: u32, height: u32, frame_rate: Time) -> Result<Self> {
        let project = Self {
            schema_version: 1,
            id,
            revision: 0,
            width,
            height,
            frame_rate,
            assets: vec![],
            clips: vec![],
            tracks: None,
            sequences: vec![],
            preview_scale: None,
        };
        project.validate()?;
        Ok(project)
    }
    pub fn duration(&self) -> Result<Time> {
        if let Some(tracks) = &self.tracks {
            return Ok(tracks.duration);
        }
        self.clips
            .iter()
            .try_fold(Time::ZERO, |sum, c| sum.plus(c.duration))
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(error(
                "UNSUPPORTED_SCHEMA",
                "Only schema version 1 is supported",
            ));
        }
        valid_id(&self.id)?;
        self.frame_rate.validate()?;
        if let Some(scale) = self.preview_scale {
            crate::proxy::dimensions(self, scale)?;
        }
        if self.frame_rate.num == 0
            || self.width == 0
            || self.height == 0
            || self.width > 8192
            || self.height > 8192
        {
            return Err(error(
                "INVALID_SEQUENCE",
                "Invalid frame rate or dimensions (maximum 8192)",
            ));
        }
        if self.revision >= 9_007_199_254_740_991
            || self.clips.len() > 1000
            || self.assets.len() > 1000
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Project exceeds supported revision or 1000-item limits",
            ));
        }
        let mut ids = HashSet::new();
        let mut assets = HashMap::new();
        for asset in &self.assets {
            valid_id(&asset.id)?;
            asset.duration.validate()?;
            if !ids.insert(&asset.id) {
                return Err(error("DUPLICATE_ID", &asset.id));
            }
            if asset.path.is_empty() || asset.duration.num == 0 {
                return Err(error(
                    "INVALID_MEDIA",
                    "Media requires a path and positive duration",
                ));
            }
            asset.metadata.validate()?;
            if let Some(identity) = &asset.identity {
                identity.validate()?;
            }
            if let Some(proxy) = &asset.proxy {
                proxy.validate(self, asset)?;
            }
            assets.insert(&asset.id, asset);
        }
        crate::sequences::validate(self)?;
        ids.clear();
        if let Some(tracks) = &self.tracks {
            if !self.clips.is_empty() {
                return Err(error(
                    "INVALID_TRACKS",
                    "Sequential clips and tracks cannot coexist",
                ));
            }
            tracks.validate(self)?;
        }
        for clip in &self.clips {
            valid_id(&clip.id)?;
            if !ids.insert(&clip.id) {
                return Err(error("DUPLICATE_ID", &clip.id));
            }
            clip.source_in.units(self.frame_rate)?;
            clip.duration.units(self.frame_rate)?;
            if clip.duration.num == 0 {
                return Err(error("INVALID_RANGE", "Clip duration must be positive"));
            }
            if clip.gap {
                if clip.asset_id.is_some() || clip.source_in.num != 0 {
                    return Err(error(
                        "INVALID_GAP",
                        "Explicit gaps require no asset and source_in zero",
                    ));
                }
                continue;
            }
            let id = clip.asset_id.as_ref().ok_or_else(|| {
                error(
                    "MISSING_MEDIA",
                    "Media clips require asset_id; gaps require gap=true",
                )
            })?;
            let asset = assets.get(id).ok_or_else(|| error("MISSING_MEDIA", id))?;
            if clip
                .source_in
                .plus(clip.duration)?
                .compare(asset.duration)?
                == Ordering::Greater
            {
                return Err(error(
                    "INVALID_RANGE",
                    format!("Clip {} exceeds source or is empty", clip.id),
                ));
            }
        }
        self.duration()?;
        Ok(())
    }
    /// Pure snapshot transform: caller's project remains unchanged on success or failure.
    pub fn apply(&self, expected_revision: u64, operations: Vec<Operation>) -> Result<Self> {
        self.validate()?;
        if expected_revision != self.revision {
            return Err(error(
                "REVISION_CONFLICT",
                "Expected revision does not match supplied snapshot",
            ));
        }
        if operations.is_empty() || operations.len() > 1000 {
            return Err(error("INVALID_BATCH", "Batch requires 1-1000 operations"));
        }
        let mut next = self.clone();
        for op in operations {
            if next.tracks.is_some()
                && matches!(
                    &op,
                    Operation::Append { .. }
                        | Operation::Split { .. }
                        | Operation::Trim { .. }
                        | Operation::Remove { .. }
                        | Operation::Move { .. }
                        | Operation::Insert { .. }
                        | Operation::Overwrite { .. }
                        | Operation::RippleDelete { .. }
                        | Operation::Slip { .. }
                        | Operation::Roll { .. }
                        | Operation::Slide { .. }
                )
            {
                return Err(error(
                    "UNSUPPORTED_TIMELINE",
                    "Use tracks.edit for a native track timeline",
                ));
            }
            match op {
                Operation::Paths {
                    asset_id,
                    path,
                    proxy_path,
                } => {
                    crate::media::project_path(std::path::Path::new(&path))?;
                    let asset = next.asset_mut(&asset_id)?;
                    if asset.identity.is_none() {
                        return Err(error(
                            "IDENTITY_REQUIRED",
                            "Bind an identity before changing project media paths",
                        ));
                    }
                    if let Some(path) = proxy_path {
                        crate::media::project_path(std::path::Path::new(&path))?;
                        asset
                            .proxy
                            .as_mut()
                            .ok_or_else(|| error("PROXY_MISSING", "Asset has no proxy binding"))?
                            .path = path;
                    }
                    asset.path = path;
                }
                Operation::TranscriptCut { document, plan } => {
                    crate::transcript_cut::apply(&mut next, &document, &plan)?
                }
                Operation::MulticamCreate { id, group } => {
                    crate::multicam::create(&mut next, id, group)?
                }
                Operation::MulticamEdit { id, edit } => {
                    crate::multicam::edit(&mut next, &id, edit)?
                }
                Operation::SequenceCreate { id, duration } => {
                    crate::sequences::create(&mut next, id, duration)?
                }
                Operation::SequenceEdit { id, edit } => {
                    crate::sequences::edit(&mut next, &id, edit)?
                }
                Operation::SequenceRemove { id } => crate::sequences::remove(&mut next, &id)?,
                Operation::Tracks { edit } => crate::tracks::edit(&mut next, edit)?,
                Operation::ProxyAttach { asset_id, proxy } => {
                    next.asset_mut(&asset_id)?.proxy = Some(proxy);
                }
                Operation::ProxyDetach { asset_id } => {
                    next.asset_mut(&asset_id)?.proxy = None;
                }
                Operation::ProxyRelink { asset_id, path } => {
                    if !std::path::Path::new(&path).is_absolute() {
                        return Err(error("INVALID_PATH", "Proxy relink path must be absolute"));
                    }
                    next.asset_mut(&asset_id)?
                        .proxy
                        .as_mut()
                        .ok_or_else(|| error("PROXY_MISSING", "Asset has no proxy binding"))?
                        .path = path;
                }
                Operation::PreviewProxy { scale } => next.preview_scale = scale,
                Operation::AddMedia { asset } => next.assets.push(asset),
                Operation::Insert { at, clip, right_id } => {
                    let mut left = next.slice(Time::ZERO, at)?;
                    let mut right = next.slice(at, next.duration()?)?;
                    next.rename_shared_fragment(&left, &mut right, right_id)?;
                    left.push(clip);
                    left.extend(right);
                    next.clips = left;
                }
                Operation::Overwrite { at, clip, right_id } => {
                    let mut left = next.slice(Time::ZERO, at)?;
                    let end = at.plus(clip.duration)?;
                    let mut right = if end.compare(next.duration()?)? == Ordering::Less {
                        next.slice(end, next.duration()?)?
                    } else {
                        Vec::new()
                    };
                    next.rename_shared_fragment(&left, &mut right, right_id)?;
                    left.push(clip);
                    left.extend(right);
                    next.clips = left;
                }
                Operation::RippleDelete {
                    start,
                    duration,
                    right_id,
                } => {
                    if duration.num == 0 {
                        return Err(error(
                            "INVALID_RANGE",
                            "Ripple deletion must have positive duration",
                        ));
                    }
                    let mut left = next.slice(Time::ZERO, start)?;
                    let mut right = next.slice(start.plus(duration)?, next.duration()?)?;
                    next.rename_shared_fragment(&left, &mut right, right_id)?;
                    left.extend(right);
                    next.clips = left;
                }
                Operation::Slip { clip_id, source_in } => {
                    let i = next.index(&clip_id)?;
                    if next.clips[i].gap {
                        return Err(error(
                            "INVALID_EDIT",
                            "A gap has no source interval to slip",
                        ));
                    }
                    next.clips[i].source_in = source_in;
                }
                Operation::Roll {
                    left_id,
                    left_duration,
                } => {
                    let i = next.index(&left_id)?;
                    if i + 1 >= next.clips.len() {
                        return Err(error("INVALID_EDIT", "Roll needs a following clip"));
                    }
                    let old = next.clips[i].duration;
                    next.shift_start(i + 1, old, left_duration)?;
                    next.clips[i].duration = left_duration;
                }
                Operation::Slide {
                    clip_id,
                    previous_duration,
                } => {
                    let i = next.index(&clip_id)?;
                    if i == 0 || i + 1 >= next.clips.len() {
                        return Err(error("INVALID_EDIT", "Slide needs both neighboring clips"));
                    }
                    let old = next.clips[i - 1].duration;
                    next.shift_start(i + 1, old, previous_duration)?;
                    next.clips[i - 1].duration = previous_duration;
                }
                Operation::Metadata { asset_id, metadata } => {
                    next.asset_mut(&asset_id)?.metadata = metadata
                }
                Operation::Bind { asset_id, identity } => {
                    identity.validate()?;
                    let asset = next.asset_mut(&asset_id)?;
                    if asset
                        .identity
                        .as_ref()
                        .is_some_and(|previous| previous != &identity)
                    {
                        return Err(error(
                            "IDENTITY_MISMATCH",
                            "A bound asset identity cannot be replaced; register a new asset",
                        ));
                    }
                    asset.identity = Some(identity);
                }
                Operation::Relink { asset_id, path } => {
                    if !std::path::Path::new(&path).is_absolute() {
                        return Err(error("INVALID_PATH", "Relink path must be absolute"));
                    }
                    let asset = next.asset_mut(&asset_id)?;
                    if asset.identity.is_none() {
                        return Err(error(
                            "IDENTITY_REQUIRED",
                            "Bind an identity before relinking",
                        ));
                    }
                    asset.path = path;
                }
                Operation::Append { clip } => next.clips.push(clip),
                Operation::Split {
                    clip_id,
                    new_clip_id,
                    offset,
                } => {
                    let i = next.index(&clip_id)?;
                    offset.units(next.frame_rate)?;
                    let clip = &next.clips[i];
                    if offset.num == 0 || offset.compare(clip.duration)? != Ordering::Less {
                        return Err(error(
                            "INVALID_RANGE",
                            "Split offset must be strictly inside the clip",
                        ));
                    }
                    let mut right = clip.clone();
                    right.id = new_clip_id;
                    right.advance_source(offset)?;
                    right.duration = clip.duration.minus(offset)?;
                    next.clips[i].duration = offset;
                    next.clips.insert(i + 1, right);
                }
                Operation::Trim {
                    clip_id,
                    source_in,
                    duration,
                } => {
                    let i = next.index(&clip_id)?;
                    next.clips[i].source_in = source_in;
                    next.clips[i].duration = duration;
                }
                Operation::Remove { clip_id } => {
                    let i = next.index(&clip_id)?;
                    next.clips.remove(i);
                }
                Operation::Move { clip_id, to_index } => {
                    let i = next.index(&clip_id)?;
                    if to_index >= next.clips.len() {
                        return Err(error(
                            "INVALID_INDEX",
                            "Move index must be within the sequence",
                        ));
                    }
                    let clip = next.clips.remove(i);
                    next.clips.insert(to_index, clip);
                }
            }
            next.validate()?;
        }
        next.revision += 1;
        next.validate()?;
        Ok(next)
    }
    fn index(&self, id: &str) -> Result<usize> {
        self.clips
            .iter()
            .position(|c| c.id == id)
            .ok_or_else(|| error("MISSING_CLIP", id))
    }
    fn asset_mut(&mut self, id: &str) -> Result<&mut Asset> {
        self.assets
            .iter_mut()
            .find(|a| a.id == id)
            .ok_or_else(|| error("MISSING_MEDIA", id))
    }
    /// Half-open timeline slice; source intervals are clipped, never rounded or merged.
    fn slice(&self, start: Time, end: Time) -> Result<Vec<Clip>> {
        start.units(self.frame_rate)?;
        end.units(self.frame_rate)?;
        if start.compare(end)? == Ordering::Greater
            || end.compare(self.duration()?)? == Ordering::Greater
        {
            return Err(error(
                "INVALID_RANGE",
                "Edit range must lie inside the timeline",
            ));
        }
        let mut position = Time::ZERO;
        let mut clips = Vec::new();
        for clip in &self.clips {
            let next = position.plus(clip.duration)?;
            let left = if start.compare(position)? == Ordering::Greater {
                start
            } else {
                position
            };
            let right = if end.compare(next)? == Ordering::Less {
                end
            } else {
                next
            };
            if left.compare(right)? == Ordering::Less {
                let mut part = clip.clone();
                part.advance_source(left.minus(position)?)?;
                part.duration = right.minus(left)?;
                clips.push(part);
            }
            position = next;
        }
        Ok(clips)
    }
    fn rename_shared_fragment(
        &self,
        left: &[Clip],
        right: &mut [Clip],
        right_id: Option<String>,
    ) -> Result<()> {
        let shared = left
            .last()
            .zip(right.first())
            .is_some_and(|(a, b)| a.id == b.id);
        match (shared, right_id) {
            (true, Some(id)) => {
                valid_id(&id)?;
                right[0].id = id;
                Ok(())
            }
            (true, None) => Err(error(
                "SPLIT_ID_REQUIRED",
                "Supply right_id when an original clip survives on both sides",
            )),
            (false, Some(_)) => Err(error(
                "UNUSED_SPLIT_ID",
                "right_id is only valid when an original clip survives on both sides",
            )),
            (false, None) => Ok(()),
        }
    }
    fn shift_start(&mut self, i: usize, old: Time, new: Time) -> Result<()> {
        new.units(self.frame_rate)?;
        if new.compare(old)? != Ordering::Less {
            let delta = new.minus(old)?;
            self.clips[i].advance_source(delta)?;
            self.clips[i].duration = self.clips[i].duration.minus(delta)?;
        } else {
            let delta = old.minus(new)?;
            if !self.clips[i].gap {
                self.clips[i].source_in = self.clips[i].source_in.minus(delta)?;
            }
            self.clips[i].duration = self.clips[i].duration.plus(delta)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(n: u64) -> Time {
        Time::new(n, 1).unwrap()
    }
    fn sample() -> Project {
        let mut p = Project::new("test".into(), 320, 180, t(25)).unwrap();
        p.assets.push(Asset {
            metadata: Default::default(),
            identity: None,
            proxy: None,
            id: "a".into(),
            path: "external.mkv".into(),
            duration: t(12),
        });
        p.clips.push(Clip {
            id: "c".into(),
            asset_id: Some("a".into()),
            gap: false,
            source_in: t(1),
            duration: t(10),
        });
        p
    }
    #[test]
    fn split_preserves_source_range_and_total_duration() {
        let p = sample();
        let q = p
            .apply(
                0,
                vec![Operation::Split {
                    clip_id: "c".into(),
                    new_clip_id: "d".into(),
                    offset: t(4),
                }],
            )
            .unwrap();
        assert_eq!(q.clips[1].source_in, t(5));
        assert_eq!(q.clips[1].duration, t(6));
        assert_eq!(q.duration().unwrap(), p.duration().unwrap());
        assert_eq!(p.clips.len(), 1);
    }
    #[test]
    fn failed_batch_does_not_mutate_original() {
        let p = sample();
        let before = serde_json::to_string(&p).unwrap();
        assert!(
            p.apply(
                0,
                vec![
                    Operation::Remove {
                        clip_id: "c".into()
                    },
                    Operation::Remove {
                        clip_id: "missing".into()
                    }
                ]
            )
            .is_err()
        );
        assert_eq!(before, serde_json::to_string(&p).unwrap());
    }
    #[test]
    fn rejects_conflicts_unknown_fields_and_duplicate_ids() {
        assert_eq!(
            sample().apply(7, vec![]).unwrap_err().code,
            "REVISION_CONFLICT"
        );
        assert!(
            serde_json::from_str::<Operation>(
                r#"{"op":"clip.remove","clip_id":"c","surprise":true}"#
            )
            .is_err()
        );
        let mut p = sample();
        p.clips.push(p.clips[0].clone());
        assert_eq!(p.validate().unwrap_err().code, "DUPLICATE_ID");
    }
    #[test]
    fn trim_move_remove_and_serialization_work() {
        let p = sample()
            .apply(
                0,
                vec![
                    Operation::Split {
                        clip_id: "c".into(),
                        new_clip_id: "d".into(),
                        offset: t(4),
                    },
                    Operation::Trim {
                        clip_id: "c".into(),
                        source_in: t(2),
                        duration: t(2),
                    },
                    Operation::Move {
                        clip_id: "d".into(),
                        to_index: 0,
                    },
                    Operation::Remove {
                        clip_id: "c".into(),
                    },
                ],
            )
            .unwrap();
        assert_eq!(p.clips[0].id, "d");
        assert_eq!(p.duration().unwrap(), t(6));
        let restored: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        restored.validate().unwrap();
        assert_eq!(restored.duration().unwrap(), t(6));
    }
}
