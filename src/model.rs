use crate::{At, Result, error, time::Time};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

/// External source media declared in a project with media.add; files are referenced, never copied.
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Asset {
    /// Unique asset ID within the project, 1-128 bytes; clips refer to it as `asset_id`.
    pub id: String,
    /// Source file path: absolute, or relative normal components resolved under `input_root`.
    pub path: String,
    /// Complete source duration in rational seconds; positive, and clip ranges must fit inside it.
    pub duration: Time,
    /// Searchable title, bin, tags and custom fields; omitted when empty.
    #[serde(default, skip_serializing_if = "crate::registry::Metadata::is_empty")]
    pub metadata: crate::registry::Metadata,
    /// Bound content identity set by media.bind; required for relinking and proxies. Omitted when unbound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<crate::registry::Identity>,
    /// Preview proxy attached with media.proxy.attach; omitted when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<crate::proxy::Binding>,
}

/// Item of the sequential timeline: a source interval of an asset, or an explicit gap of black and silence.
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Clip {
    /// Unique clip ID among the sequential clips, 1-128 bytes.
    pub id: String,
    /// ID of an asset added with media.add; omit for a gap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<String>,
    /// True for an explicit gap with no `asset_id` and zero `source_in`; default false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub gap: bool,
    /// Source start in rational seconds, aligned to the project frame rate; zero for gaps.
    pub source_in: Time,
    /// Positive frame-aligned length in rational seconds; `source_in + duration` must fit the asset.
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

/// Versioned project snapshot: canvas, frame rate, media and either a sequential clip list or native tracks. Returned by project.create and session.get; change it only through operations.
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// Snapshot schema version; must be 1.
    pub schema_version: u32,
    /// Project ID, 1-128 bytes.
    pub id: String,
    /// Revision number, incremented once per applied batch; pass it as `expected_revision`.
    pub revision: u64,
    /// Frame width in pixels, 1-8192.
    pub width: u32,
    /// Frame height in pixels, 1-8192.
    pub height: u32,
    /// Frames per second as a positive rational such as 25/1 or 30000/1001; video times align to it.
    pub frame_rate: Time,
    /// Declared source media, at most 1000; add with media.add.
    pub assets: Vec<Asset>,
    /// Sequential timeline in playback order, at most 1000 items; must be empty when `tracks` is set.
    pub clips: Vec<Clip>,
    /// Native placed-track timeline, created by tracks.edit `create` or `promote`; omitted for sequential projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tracks: Option<crate::tracks::Arrangement>,
    /// Reusable child sequence definitions, at most 32; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sequences: Vec<crate::sequences::Sequence>,
    /// Proxy preview divisor 2, 4 or 8 set by preview.proxy; omitted for full-quality previews.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_scale: Option<u32>,
}

/// One edit in an atomic timeline.apply or session.apply batch, tagged by `op`; each must leave a valid project. The clip.* operations and timeline.ripple_delete edit the sequential `clips` list and are rejected once native tracks exist.
#[derive(schemars::JsonSchema, Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", deny_unknown_fields)]
#[schemars(rename = "Operation")]
pub enum Operation {
    /// Change an identity-bound asset's original and optional proxy paths, as proposed by project.portable.
    #[serde(rename = "media.paths")]
    Paths {
        /// Asset to update; it must have a bound identity.
        asset_id: String,
        /// New source path: absolute, or relative normal components resolved under `input_root`.
        path: String,
        /// New proxy path in the same form; requires an attached proxy. Omit to keep the current one.
        #[serde(default)]
        proxy_path: Option<String>,
    },
    /// Apply a transcript.plan result, ripple-deleting the planned word ranges. Fails if the project or document changed since planning.
    #[serde(rename = "transcript.cut")]
    TranscriptCut {
        /// Transcript document the plan was made from, as returned by transcript.plan.
        document: Box<crate::transcript::Document>,
        /// Fingerprint-bound cut plan returned by transcript.plan.
        plan: crate::transcript_cut::Plan,
    },
    /// Create a camera group as a managed child sequence that can be placed by `sequence_id`.
    #[serde(rename = "multicam.create")]
    MulticamCreate {
        /// New sequence ID, unique among the project's sequences, 1-128 bytes.
        id: String,
        /// Camera angles, cuts and audio policy.
        group: crate::multicam::Group,
    },
    /// Change one camera group's cuts, angles, audio policy, duration or lock state.
    #[serde(rename = "multicam.edit")]
    MulticamEdit {
        /// ID of a sequence created with multicam.create.
        id: String,
        /// Camera-group edit to apply.
        edit: crate::multicam::Edit,
    },
    /// Add an empty reusable child sequence with native tracks.
    #[serde(rename = "sequence.create")]
    SequenceCreate {
        /// New sequence ID, unique among the project's sequences, 1-128 bytes.
        id: String,
        /// Child timeline length in rational seconds, aligned to the project frame rate.
        duration: Time,
    },
    /// Apply one native track edit inside a child sequence; every instance changes. Camera groups use multicam.edit.
    #[serde(rename = "sequence.edit")]
    SequenceEdit {
        /// ID of the child sequence to edit.
        id: String,
        /// Track edit, as in tracks.edit; `create` and `promote` are rejected.
        edit: crate::tracks::Edit,
    },
    /// Remove a child sequence that nothing references and whose tracks are unlocked.
    #[serde(rename = "sequence.remove")]
    SequenceRemove {
        /// ID of the child sequence to remove.
        id: String,
    },
    /// Edit the project's native placed-track timeline.
    #[serde(rename = "tracks.edit")]
    Tracks {
        /// Track edit to apply, tagged by its own `op`.
        edit: crate::tracks::Edit,
    },
    /// Attach or replace an asset's preview proxy, usually one returned by proxy.generate.
    #[serde(rename = "media.proxy.attach")]
    ProxyAttach {
        /// Asset that receives the proxy; it must have a bound identity.
        asset_id: String,
        /// Proxy binding; its `source_identity` must equal the asset's identity.
        proxy: crate::proxy::Binding,
    },
    /// Remove an asset's proxy binding; no files are deleted.
    #[serde(rename = "media.proxy.detach")]
    ProxyDetach {
        /// Asset whose proxy binding is removed.
        asset_id: String,
    },
    /// Replace only an attached proxy's path, usually as proposed by proxy.relink.
    #[serde(rename = "media.proxy.relink")]
    ProxyRelink {
        /// Asset whose proxy moves; it must have a proxy binding.
        asset_id: String,
        /// New absolute proxy file path.
        path: String,
    },
    /// Select the proxy scale for frame and range previews; final renders always use full quality.
    #[serde(rename = "preview.proxy")]
    PreviewProxy {
        /// Dimension divisor 2, 4 or 8; null or omitted selects full-quality sources.
        scale: Option<u32>,
    },
    /// Declare an external source; rendering verifies its actual contents.
    #[serde(rename = "media.add")]
    AddMedia {
        /// Asset to add, with an ID not already used by another asset.
        asset: Asset,
    },
    /// Replace one asset's complete metadata.
    #[serde(rename = "media.metadata")]
    Metadata {
        /// Asset to update.
        asset_id: String,
        /// New metadata; omitted parts become empty.
        metadata: crate::registry::Metadata,
    },
    /// Bind an asset's content identity, usually as proposed by registry.bind.
    #[serde(rename = "media.bind")]
    Bind {
        /// Asset to bind.
        asset_id: String,
        /// Content identity; an existing different identity cannot be replaced.
        identity: crate::registry::Identity,
    },
    /// Change an identity-bound asset's source path, usually as proposed by registry.relink.
    #[serde(rename = "media.relink")]
    Relink {
        /// Asset to relink; it must have a bound identity.
        asset_id: String,
        /// New absolute source path.
        path: String,
    },
    /// Append a media clip or gap to the end of the sequential timeline.
    #[serde(rename = "clip.append")]
    Append {
        /// Clip to append, with a new unique ID.
        clip: Clip,
    },
    /// Split a clip in two at an offset from its start; the left part keeps `clip_id`.
    #[serde(rename = "clip.split")]
    Split {
        /// Clip to split.
        clip_id: String,
        /// New unique ID for the right part.
        new_clip_id: String,
        /// Split point from the clip's start, frame-aligned and strictly inside the clip.
        offset: Time,
    },
    /// Replace a clip's source interval; later clips shift by the duration change.
    #[serde(rename = "clip.trim")]
    Trim {
        /// Clip to trim.
        clip_id: String,
        /// New source start in rational seconds, frame-aligned; zero for gaps.
        source_in: Time,
        /// New positive frame-aligned duration; `source_in + duration` must fit the asset.
        duration: Time,
    },
    /// Remove a clip and close the gap; later clips shift earlier.
    #[serde(rename = "clip.remove")]
    Remove {
        /// Clip to remove.
        clip_id: String,
    },
    /// Move a clip to another position in the sequential order.
    #[serde(rename = "clip.move")]
    Move {
        /// Clip to move.
        clip_id: String,
        /// Zero-based index in the resulting list; must be less than the clip count.
        to_index: usize,
    },
    /// Insert a clip at a timeline time, splitting any clip there and shifting later content right.
    #[serde(rename = "clip.insert")]
    Insert {
        /// Timeline time in rational seconds, frame-aligned and at most the current end.
        at: Time,
        /// New clip with a unique ID.
        clip: Clip,
        /// New ID for the right fragment when a clip is split at `at`; omit otherwise.
        right_id: Option<String>,
    },
    /// Replace the interval from `at` for the clip's duration; later content keeps its position and the end may extend.
    #[serde(rename = "clip.overwrite")]
    Overwrite {
        /// Timeline start in rational seconds, frame-aligned and at most the current end.
        at: Time,
        /// New clip with a unique ID; its duration sets the replaced length.
        clip: Clip,
        /// New ID for the right fragment when one clip survives on both sides; omit otherwise.
        right_id: Option<String>,
    },
    /// Remove a timeline interval and close it; later content shifts earlier.
    #[serde(rename = "timeline.ripple_delete")]
    RippleDelete {
        /// Interval start in rational seconds, frame-aligned.
        start: Time,
        /// Positive frame-aligned length; the interval must lie inside the timeline.
        duration: Time,
        /// New ID for the right fragment when one clip survives on both sides; omit otherwise.
        right_id: Option<String>,
    },
    /// Change a media clip's source start while keeping its placement and duration; gaps cannot slip.
    #[serde(rename = "clip.slip")]
    Slip {
        /// Media clip to slip.
        clip_id: String,
        /// New source start in rational seconds, frame-aligned; the range must fit the asset.
        source_in: Time,
    },
    /// Move the cut between a clip and the next one; their combined duration stays the same.
    #[serde(rename = "clip.roll")]
    Roll {
        /// Clip before the cut; it needs a following clip.
        left_id: String,
        /// New duration of the left clip; the next clip's source_in and duration compensate.
        left_duration: Time,
    },
    /// Move a middle clip by trimming its neighbors; its content, its duration and the total duration stay.
    #[serde(rename = "clip.slide")]
    Slide {
        /// Middle clip to move; it needs clips on both sides.
        clip_id: String,
        /// New duration of the previous clip; the next clip's source_in and duration compensate.
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
            clip.source_in
                .units(self.frame_rate)
                .at(|| format!("clip {:?} source_in", clip.id))?;
            clip.duration
                .units(self.frame_rate)
                .at(|| format!("clip {:?} duration", clip.id))?;
            if clip.duration.num == 0 {
                return Err(error(
                    "INVALID_RANGE",
                    format!("clip {:?} duration must be positive", clip.id),
                ));
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
            let asset = assets
                .get(id)
                .ok_or_else(|| self.missing_asset(id))
                .at(|| format!("clip {:?}", clip.id))?;
            let end = clip.source_in.plus(clip.duration)?;
            if end.compare(asset.duration)? == Ordering::Greater {
                return Err(error(
                    "INVALID_RANGE",
                    format!(
                        "clip {:?} needs source {} s to {end} s but asset {:?} lasts {} s",
                        clip.id, clip.source_in, asset.id, asset.duration
                    ),
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
                            format!(
                                "Split offset {offset} s must be strictly inside clip {:?}, which lasts {} s",
                                clip.id, clip.duration
                            ),
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
        self.clips.iter().position(|c| c.id == id).ok_or_else(|| {
            crate::missing(
                "MISSING_CLIP",
                "clip",
                id,
                self.clips.iter().map(|c| &*c.id),
            )
        })
    }
    /// MISSING_MEDIA naming `id` and the project's asset IDs.
    pub(crate) fn missing_asset(&self, id: &str) -> crate::Error {
        crate::missing(
            "MISSING_MEDIA",
            "asset",
            id,
            self.assets.iter().map(|a| &*a.id),
        )
    }
    pub(crate) fn asset(&self, id: &str) -> Result<&Asset> {
        self.assets
            .iter()
            .find(|a| a.id == id)
            .ok_or_else(|| self.missing_asset(id))
    }
    fn asset_mut(&mut self, id: &str) -> Result<&mut Asset> {
        let i = self
            .assets
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| self.missing_asset(id))?;
        Ok(&mut self.assets[i])
    }
    /// Half-open timeline slice; source intervals are clipped, never rounded or merged.
    fn slice(&self, start: Time, end: Time) -> Result<Vec<Clip>> {
        start.units(self.frame_rate)?;
        end.units(self.frame_rate)?;
        let total = self.duration()?;
        if start.compare(end)? == Ordering::Greater || end.compare(total)? == Ordering::Greater {
            return Err(error(
                "INVALID_RANGE",
                format!(
                    "Edit range {start} s to {end} s must lie inside the timeline, 0 s to {total} s"
                ),
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
    fn errors_name_missing_ids_and_exact_ranges() {
        let p = sample();
        let missing = p
            .apply(
                0,
                vec![Operation::Remove {
                    clip_id: "nope".into(),
                }],
            )
            .unwrap_err();
        assert_eq!(
            (missing.code, missing.message.as_str()),
            (
                "MISSING_CLIP",
                r#"Unknown clip "nope"; available clip IDs: "c""#
            )
        );
        let range = p
            .apply(
                0,
                vec![Operation::Trim {
                    clip_id: "c".into(),
                    source_in: t(8),
                    duration: t(5),
                }],
            )
            .unwrap_err();
        assert_eq!(
            (range.code, range.message.as_str()),
            (
                "INVALID_RANGE",
                r#"clip "c" needs source 8 s to 13 s but asset "a" lasts 12 s"#
            )
        );
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
