//! Native placed tracks and transactional linked editing. Times are exact rationals.
use crate::{
    At, Result, error,
    model::{Clip, Project},
    time::Time,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Track media kind: `video` uses the project frame clock; `audio` (stereo) uses the 48 kHz sample clock.
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
/// Clip placed at an explicit time on a native track, reading either a media asset or a child sequence. Times align to the track clock.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackClip {
    /// Clip ID unique across all tracks of this arrangement, 1-128 bytes.
    pub id: String,
    /// Asset added with media.add; omit when `sequence_id` is set. Exactly one of the two is required.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub asset_id: String,
    /// Child sequence created by sequence.create or multicam.create; omit for media clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence_id: Option<String>,
    /// Timeline position in rational seconds; the clip must end by the arrangement duration.
    pub start: Time,
    /// Source (or child sequence) position played at `start`, in rational seconds.
    pub source_in: Time,
    /// Positive length in rational seconds; `source_in + duration` must fit the source.
    pub duration: Time,
    /// Audio-track clips only: linear gain, 1000 = unity, 0..4000; default 1000.
    #[serde(default = "unity", skip_serializing_if = "is_unity")]
    pub gain_milli: u32,
    /// Audio-track clips only: linear fade-in from the clip start, on the 48 kHz grid, at most 60 s; default zero.
    #[serde(default = "zero", skip_serializing_if = "is_zero")]
    pub fade_in: Time,
    /// Audio-track clips only: linear fade-out ending at the clip end, on the 48 kHz grid, at most 60 s; `fade_in + fade_out` must fit `duration`. Default zero.
    #[serde(default = "zero", skip_serializing_if = "is_zero")]
    pub fade_out: Time,
    /// Audio-track clips only: gain automation in linear milli-units 0..4000, replacing `gain_milli`. Key times are positions on the clip's source clock (`source_in` plays key time `source_in`), so cuts and trims keep the gain aligned with the audio; 1..2000 keys, any interpolation. Fades still apply on top.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain_curve: Option<crate::animation::Curve>,
    /// Set only in range snapshots: this part's offset into the original clip and the original
    /// length, in 48 kHz samples, so a trimmed clip keeps the original clip's fade envelope.
    #[serde(skip)]
    pub(crate) envelope: Option<(u64, u64)>,
    /// `alpha_over` track clips only: crop, shrink, fade and place the clip's frame, as for picture-in-picture; omit to cover the whole frame unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<OverlayTransform>,
}
/// Picture-in-picture placement of an `alpha_over` clip, applied in order: `crop` the source, shrink it by `divisor`, multiply its alpha by `opacity`, then put its top-left corner at `position`. Parts outside the canvas are clipped.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayTransform {
    /// Source region `[x, y, width, height]` in source pixels, inside the frame, with sizes that are multiples of `divisor`; omit for the whole frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crop: Option<[u32; 4]>,
    /// Integer shrink factor 1-8: each output pixel is the floor of its `divisor` x `divisor` block's mean, per channel. Opaque sources only; default 1.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub divisor: u32,
    /// Alpha multiplier 0-255 (255 keeps the source alpha): `alpha x opacity / 255`, rounded to nearest. Default 255.
    #[serde(default = "opaque", skip_serializing_if = "is_opaque")]
    pub opacity: u8,
    /// Canvas position `[x, y]` of the result's top-left corner, -32768..=32768; parts outside the canvas are clipped. Default `[0, 0]`.
    #[serde(default, skip_serializing_if = "is_origin")]
    pub position: [i32; 2],
}
fn one() -> u32 {
    1
}
fn is_one(value: &u32) -> bool {
    *value == 1
}
fn opaque() -> u8 {
    255
}
fn is_opaque(value: &u8) -> bool {
    *value == 255
}
fn is_origin(value: &[i32; 2]) -> bool {
    *value == [0, 0]
}
impl OverlayTransform {
    fn check(&self, width: u32, height: u32) -> Result<()> {
        let [x, y, w, h] = self.crop.unwrap_or([0, 0, width, height]);
        if !(1..=8).contains(&self.divisor) {
            return Err(invalid("transform divisor must be 1-8"));
        }
        if w == 0
            || h == 0
            || x as u64 + w as u64 > width as u64
            || y as u64 + h as u64 > height as u64
            || w % self.divisor != 0
            || h % self.divisor != 0
        {
            return Err(invalid(format!(
                "transform crop [{x}, {y}, {w}, {h}] must lie inside the {width}x{height} frame with sizes that are multiples of divisor {}",
                self.divisor
            )));
        }
        if self.position.iter().any(|p| p.unsigned_abs() > 32768) {
            return Err(invalid("transform position must be within -32768..=32768"));
        }
        Ok(())
    }
}
const UNITY: u32 = 1000;
/// Keys one clip's gain curve may hold: ducking a long music bed needs four per pause.
pub(crate) const MAX_GAIN_KEYS: usize = 2000;
/// Largest clip level in linear milli-units (+12 dB).
pub(crate) const MAX_GAIN_MILLI: u32 = 4000;
/// The 48 kHz audio clock.
const SAMPLES: Time = Time { num: 48000, den: 1 };
/// Longest fade: 60 s keeps the per-sample gain arithmetic exact in doubles.
const MAX_FADE_SAMPLES: u64 = 60 * 48000;
fn unity() -> u32 {
    UNITY
}
fn is_unity(gain: &u32) -> bool {
    *gain == UNITY
}
fn zero() -> Time {
    Time::ZERO
}
fn is_zero(time: &Time) -> bool {
    time.num == 0
}
impl Default for TrackClip {
    /// An empty placement at zero without audio adjustments; callers set the placement fields.
    fn default() -> Self {
        Self {
            id: String::new(),
            asset_id: String::new(),
            sequence_id: None,
            start: Time::ZERO,
            source_in: Time::ZERO,
            duration: Time::ZERO,
            gain_milli: UNITY,
            fade_in: Time::ZERO,
            fade_out: Time::ZERO,
            gain_curve: None,
            envelope: None,
            transform: None,
        }
    }
}
/// A clip's level envelope on the 48 kHz clock: gain plus linear fades, all in samples.
pub(crate) struct Envelope {
    pub gain_milli: u32,
    pub fade_in: u64,
    pub fade_out: u64,
    /// This clip's offset into the clip the fades belong to.
    pub offset: u64,
    /// Length of the clip the fades belong to.
    pub samples: u64,
}
impl TrackClip {
    /// Whether the clip changes its audio level: a gain other than unity, or a fade.
    pub(crate) fn adjusts_audio(&self) -> bool {
        self.gain_milli != UNITY
            || self.fade_in.num != 0
            || self.fade_out.num != 0
            || self.gain_curve.is_some()
    }
    pub(crate) fn envelope(&self) -> Result<Envelope> {
        let (offset, samples) = match self.envelope {
            Some(part) => part,
            None => (0, self.duration.units(SAMPLES)?),
        };
        Ok(Envelope {
            gain_milli: self.gain_milli,
            fade_in: self.fade_in.units(SAMPLES).at(|| "fade_in".into())?,
            fade_out: self.fade_out.units(SAMPLES).at(|| "fade_out".into())?,
            offset,
            samples,
        })
    }
    fn check_audio(&self, kind: Kind, source_duration: Time) -> Result<()> {
        if !self.adjusts_audio() {
            return Ok(());
        }
        if kind != Kind::Audio {
            return Err(invalid(
                "gain_milli, gain_curve, fade_in and fade_out apply only to audio-track clips",
            ));
        }
        if let Some(curve) = &self.gain_curve {
            curve.prepare_keys(source_duration, 0, MAX_GAIN_MILLI as i32, MAX_GAIN_KEYS)?;
        }
        if self.gain_milli > MAX_GAIN_MILLI {
            return Err(invalid("gain_milli must be 0..4000, with 1000 for unity"));
        }
        let e = self.envelope()?;
        if e.fade_in.max(e.fade_out) > MAX_FADE_SAMPLES {
            return Err(error("INVALID_RANGE", "Fades last at most 60 s"));
        }
        if e.fade_in + e.fade_out > e.samples {
            return Err(error(
                "INVALID_RANGE",
                format!(
                    "fade_in {} s and fade_out {} s together exceed the clip's {} s",
                    self.fade_in,
                    self.fade_out,
                    Time::new(e.samples, 48000)?
                ),
            ));
        }
        Ok(())
    }
    pub(crate) fn source_duration(&self, project: &Project) -> Result<Time> {
        match (&self.sequence_id, self.asset_id.is_empty()) {
            (Some(id), true) => Ok(crate::sequences::get(project, id)?.arrangement.duration),
            (None, false) => project.asset(&self.asset_id).map(|a| a.duration),
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
/// Native video or audio track. The highest enabled opaque video track with a clip supplies the frame; enabled audio tracks are summed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// Track ID unique within this arrangement, 1-128 bytes.
    pub id: String,
    /// Media kind; it sets the clock for clip and transition times.
    pub kind: Kind,
    /// True protects the track's clips, transitions, order position and enabled state from edits.
    pub locked: bool,
    /// False excludes the track from playback and rendering.
    pub enabled: bool,
    /// Placed clips; they may touch but not overlap. Array order does not affect playback.
    pub clips: Vec<TrackClip>,
    /// Transitions between adjacent clips on this track; omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<Transition>,
    /// Video only: `alpha_over` composites this track's straight-alpha clips over the result below.
    #[serde(default, skip_serializing_if = "Composite::is_opaque")]
    pub composite: Composite,
    /// Audio tracks of the project timeline only: dynamics on this track's summed clips before they join the mix; omitted when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dynamics: Option<crate::dynamics::Dynamics>,
}
/// How a video track combines with lower tracks.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Composite {
    /// The clip supplies the entire frame (original behaviour).
    #[default]
    Opaque,
    /// Straight-alpha "over" in encoded RGB: round((s*a + d*(255-a)) / 255), ties up.
    AlphaOver,
}
impl Composite {
    pub fn is_opaque(&self) -> bool {
        *self == Composite::Opaque
    }
}
/// Transition style: `dissolve` crossfades; `dip_black` fades out to black or silence and back in; `wipe_left`/`wipe_right` reveal the incoming clip from that edge. Audio supports only `dissolve` and `dip_black`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionKind {
    Dissolve,
    DipBlack,
    WipeLeft,
    WipeRight,
}
/// Effect across the cut between two exactly adjacent clips on one track, covering `[cut - before, cut + after)`. Both sources need real handles; none are synthesized.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Transition ID unique within this arrangement, 1-128 bytes.
    pub id: String,
    /// Outgoing clip on this track; its end is the cut.
    pub left_id: String,
    /// Incoming clip on this track, starting exactly where `left_id` ends.
    pub right_id: String,
    /// Length before the cut, aligned to the track clock; the incoming clip needs this much source before its `source_in`.
    pub before: Time,
    /// Length after the cut, aligned to the track clock; the outgoing clip needs this much source after its end. `before + after` must be positive.
    pub after: Time,
    /// Transition style.
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
/// Link member: a clip and its `start` and `source_in` recorded when the link was made.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// Linked clip in this arrangement.
    pub clip_id: String,
    /// Clip start recorded at link time, in rational seconds.
    pub start: Time,
    /// Clip source_in recorded at link time, in rational seconds.
    pub source_in: Time,
}
/// Synchronization group created by the `link` edit. Every member must keep the same change in `start - source_in` relative to its anchor, or edits fail with SYNC_CONFLICT.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Link ID unique within this arrangement, 1-128 bytes.
    pub id: String,
    /// 2-32 members; a clip belongs to at most one link.
    pub members: Vec<Anchor>,
}
/// Native placed-track timeline, used for the project's `tracks` and for each child sequence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Arrangement {
    /// Explicit timeline end in rational seconds, aligned to the project frame rate; includes leading and trailing gaps.
    pub duration: Time,
    /// Tracks in bottom-to-top order, at most 32.
    pub tracks: Vec<Track>,
    /// Synchronization links between clips, at most 500.
    pub links: Vec<Link>,
    /// Project timeline only: dynamics on the sum of the enabled audio tracks, before final PCM16 rounding; omitted when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<crate::dynamics::Dynamics>,
}
/// Overlap policy on destination tracks: `reject` fails on any overlap with an existing clip; `replace_clips` removes each whole colliding clip together with its linked partners.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    Reject,
    ReplaceClips,
}
/// Linked-partner policy: `include` adds the linked partners of affected clips (whole partner tracks for interval edits); `reject_partial` fails unless the caller selects them.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Linked {
    Include,
    RejectPartial,
}
/// Time offset given as a direction and a nonnegative magnitude.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Shift {
    /// True moves earlier (subtracts `amount`); false moves later.
    pub backward: bool,
    /// Offset size in rational seconds; zero is allowed. Resulting times must stay aligned to each track's clock.
    pub amount: Time,
}
/// Destination track for one clip in a `move` edit.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Target {
    /// Selected or link-included clip to retarget.
    pub clip_id: String,
    /// Unlocked destination track of the same kind.
    pub track_id: String,
}
/// Native track edit, tagged by `op`, used by tracks.edit and sequence.edit. Locked tracks reject changes, and the result must stay valid.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Edit {
    /// Split every selected clip at one absolute timeline time; left parts keep their IDs and right parts need new ones.
    Split(crate::track_edit::Split),
    /// Shift the selected clips' source ranges; timeline starts and durations stay.
    Slip(crate::track_edit::Slip),
    /// Move the cut after each selected clip; the exactly adjacent next clip's start, source_in and duration compensate.
    Roll(crate::track_edit::Roll),
    /// Move each selected middle clip in time; its previous neighbor lengthens or shortens and its next neighbor compensates.
    Slide(crate::track_edit::Slide),
    /// Open space on selected tracks, splitting crossing clips and shifting later content right, then place new clips in it.
    Insert(crate::track_edit::Insert),
    /// Clear an interval on selected tracks and place new clips in it; content outside keeps its timeline position.
    Overwrite(crate::track_edit::Overwrite),
    /// Remove an interval on selected tracks and shift later content on them left.
    RippleDelete(crate::track_edit::RippleDelete),
    /// Add a transition to a track, or replace the one with the same ID.
    TransitionSet {
        /// Unlocked track holding both endpoint clips.
        track_id: String,
        /// Complete transition definition.
        transition: Transition,
    },
    /// Remove a transition; its underlying cut remains.
    TransitionRemove {
        /// Unlocked track holding the transition.
        track_id: String,
        /// ID of the transition to remove.
        id: String,
    },
    /// Create an empty arrangement; requires a project without tracks or sequential clips.
    Create {
        /// Explicit timeline end in rational seconds, aligned to the project frame rate.
        duration: Time,
    },
    /// Convert the sequential timeline into one video and one audio track of linked clip pairs with identical playback; gaps become empty space.
    Promote {
        /// ID for the new video track; its clips keep the original clip IDs.
        video_track_id: String,
        /// ID for the new audio track; its clips get `promoted-audio-N` IDs.
        audio_track_id: String,
    },
    /// Add an empty track above all existing tracks.
    Add {
        /// Track to add; its `clips` and `transitions` must be empty.
        track: Track,
    },
    /// Set a track's lock and enabled state.
    State {
        /// Track to change.
        track_id: String,
        /// New lock state.
        locked: bool,
        /// New enabled state; a currently locked track must be unlocked first to change it.
        enabled: bool,
    },
    /// Reorder tracks; locked tracks cannot change position.
    Order {
        /// Every track ID exactly once, in new bottom-to-top order.
        track_ids: Vec<String>,
    },
    /// Change the explicit timeline end without truncating clips; an end before any clip's end fails.
    Duration {
        /// New end in rational seconds, aligned to the project frame rate.
        duration: Time,
    },
    /// Place one new clip on a track.
    Place {
        /// Unlocked track that receives the clip.
        track_id: String,
        /// New clip whose ID is unused in this arrangement.
        clip: TrackClip,
        /// Policy for overlaps with existing clips.
        collision: Collision,
    },
    /// Shift a clip selection by one common offset, optionally retargeting clips to other tracks of the same kind.
    Move {
        /// Clips to move, 1-1000 unique IDs.
        clip_ids: Vec<String>,
        /// Common timeline offset for every moved clip.
        shift: Shift,
        /// Per-clip destination tracks; clips without an entry stay on their track. May be empty.
        targets: Vec<Target>,
        /// Linked-partner policy.
        links: Linked,
        /// Policy for overlaps at the destinations.
        collision: Collision,
    },
    /// Remove a clip selection, leaving empty space; transitions and links of removed clips are removed too.
    Remove {
        /// Clips to remove, 1-1000 unique IDs.
        clip_ids: Vec<String>,
        /// Linked-partner policy.
        links: Linked,
    },
    /// Link clips, recording their current start and source_in as sync anchors.
    Link {
        /// New link ID unique within this arrangement, 1-128 bytes.
        id: String,
        /// 2-32 clips that are not already linked.
        clip_ids: Vec<String>,
    },
    /// Remove a link so its clips can be edited independently; the clips stay in place.
    Unlink {
        /// ID of the link to remove.
        id: String,
    },
    /// Set the gain and linear fades of audio-track clips; omitted fields keep their values. Fades follow clip edges through trims; splits and interval edits reject cuts inside a fade.
    ClipAudio {
        /// Audio-track clips to change, 1-1000 unique IDs.
        clip_ids: Vec<String>,
        /// Linear gain, 1000 = unity, 0..4000.
        #[serde(default)]
        gain_milli: Option<u32>,
        /// Fade-in from each clip's start, on the 48 kHz grid, at most 60 s; zero removes it.
        #[serde(default)]
        fade_in: Option<Time>,
        /// Fade-out ending at each clip's end, on the 48 kHz grid, at most 60 s; zero removes it.
        #[serde(default)]
        fade_out: Option<Time>,
        /// Gain automation on each clip's source clock, replacing `gain_milli`; see the clip field.
        #[serde(default)]
        gain_curve: Option<crate::animation::Curve>,
        /// Remove the clips' gain automation; default false. Cannot be combined with `gain_curve`.
        #[serde(default)]
        clear_gain_curve: bool,
    },
    /// Set or clear the picture-in-picture transform of `alpha_over` track clips.
    ClipTransform {
        /// Clips on `alpha_over` tracks, 1-1000 unique IDs.
        clip_ids: Vec<String>,
        /// New transform for every listed clip; null restores the full, unchanged frame.
        transform: Option<OverlayTransform>,
    },
    /// Set or clear the dynamics (a limiter) of an unlocked audio track, or of the master mix when `track_id` is omitted. Project timeline only.
    AudioDynamics {
        /// Audio track to change; omit for the master mix.
        #[serde(default)]
        track_id: Option<String>,
        /// New dynamics; null or omitted removes them.
        #[serde(default)]
        dynamics: Option<crate::dynamics::Dynamics>,
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
        self.duration
            .units(project.frame_rate)
            .at(|| "tracks.duration".into())?;
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
                for (field, t) in [
                    ("start", clip.start),
                    ("source_in", clip.source_in),
                    ("duration", clip.duration),
                ] {
                    t.units(track.kind.clock(project.frame_rate))
                        .at(|| format!("track {:?} clip {:?} {field}", track.id, clip.id))?;
                }
                let source_duration = clip
                    .source_duration(project)
                    .at(|| format!("track {:?} clip {:?}", track.id, clip.id))?;
                let source_end = clip.source_in.plus(clip.duration)?;
                let range = |message: String| Err(error("INVALID_RANGE", message));
                if clip.duration.num == 0 {
                    return range(format!("clip {:?} duration must be positive", clip.id));
                }
                if clip.end()?.compare(self.duration)?.is_gt() {
                    return range(format!(
                        "clip {:?} ends at {} s, after the timeline end {} s",
                        clip.id,
                        clip.end()?,
                        self.duration
                    ));
                }
                if let Some(transform) = &clip.transform {
                    if track.composite.is_opaque() {
                        return Err(invalid(format!(
                            "clip {:?} has a transform, which only alpha_over track clips accept",
                            clip.id
                        )));
                    }
                    transform
                        .check(project.width, project.height)
                        .at(|| format!("track {:?} clip {:?}", track.id, clip.id))?;
                }
                clip.check_audio(track.kind, source_duration)
                    .at(|| format!("track {:?} clip {:?}", track.id, clip.id))?;
                if source_end.compare(source_duration)?.is_gt() {
                    let source = match &clip.sequence_id {
                        Some(id) => format!("sequence {id:?}"),
                        None => format!("asset {:?}", clip.asset_id),
                    };
                    return range(format!(
                        "clip {:?} needs source {} s to {source_end} s but {source} lasts {source_duration} s",
                        clip.id, clip.source_in
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
            if !track.composite.is_opaque()
                && (track.kind != Kind::Video
                    || !track.transitions.is_empty()
                    || track.clips.iter().any(|c| c.sequence_id.is_some()))
            {
                return Err(error(
                    "UNSUPPORTED_TIMELINE",
                    format!(
                        "Track {:?}: alpha_over applies to video tracks of asset clips without transitions",
                        track.id
                    ),
                ));
            }
            if track.transitions.len() > 1000 {
                return Err(error("LIMIT_EXCEEDED", "Too many transitions"));
            }
            if let Some(dynamics) = &track.dynamics {
                if track.kind != Kind::Audio {
                    return Err(error(
                        "UNSUPPORTED_TIMELINE",
                        format!("Track {:?}: dynamics apply to audio tracks", track.id),
                    ));
                }
                dynamics
                    .limiter
                    .validate()
                    .at(|| format!("track {:?} dynamics", track.id))?;
            }
            let mut intervals = Vec::new();
            for effect in &track.transitions {
                id(&effect.id)?;
                if !transition_ids.insert(&effect.id) {
                    return Err(error("DUPLICATE_ID", &effect.id));
                }
                let clock = track.kind.clock(project.frame_rate);
                effect
                    .before
                    .units(clock)
                    .at(|| format!("transition {:?} before", effect.id))?;
                effect
                    .after
                    .units(clock)
                    .at(|| format!("transition {:?} after", effect.id))?;
                let (left, right) = track.endpoints(effect)?;
                if left.fade_out.num != 0 || right.fade_in.num != 0 {
                    return Err(error(
                        "INVALID_TRANSITION",
                        format!(
                            "Transition {:?} already shapes the cut; remove the fade_out of {:?} and the fade_in of {:?}",
                            effect.id, left.id, right.id
                        ),
                    ));
                }
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
        if let Some(master) = &self.master {
            master.limiter.validate().at(|| "master dynamics".into())?;
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
                    .ok_or_else(|| {
                        crate::missing(
                            "MISSING_CLIP",
                            "clip",
                            &anchor.clip_id,
                            clips.keys().map(|id| id.as_str()),
                        )
                    })
                    .at(|| format!("link {:?}", link.id))?;
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
            .ok_or_else(|| self.missing_track(name))
    }
    /// MISSING_TRACK naming `name` and the arrangement's track IDs.
    pub(crate) fn missing_track(&self, name: &str) -> crate::Error {
        crate::missing(
            "MISSING_TRACK",
            "track",
            name,
            self.tracks.iter().map(|t| t.id.as_str()),
        )
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
            .ok_or_else(|| {
                crate::missing(
                    "MISSING_CLIP",
                    "clip",
                    name,
                    self.tracks
                        .iter()
                        .flat_map(|t| &t.clips)
                        .map(|c| c.id.as_str()),
                )
            })
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
            .filter(|t| t.kind == Kind::Video && t.enabled && t.composite.is_opaque())
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
                .ok_or_else(|| {
                    crate::missing(
                        "MISSING_TRANSITION",
                        "transition",
                        &id,
                        effects.iter().map(|e| e.id.as_str()),
                    )
                })
                .at(|| format!("track {track_id:?}"))?;
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
                master: None,
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
                        composite: Default::default(),
                        dynamics: None,
                    },
                    Track {
                        id: audio_track_id,
                        kind: Kind::Audio,
                        locked: false,
                        enabled: true,
                        clips: vec![],
                        transitions: vec![],
                        composite: Default::default(),
                        dynamics: None,
                    },
                ],
                links: vec![],
                master: None,
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
                        ..Default::default()
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
            let index = a.links.iter().position(|l| l.id == id).ok_or_else(|| {
                crate::missing(
                    "MISSING_LINK",
                    "link",
                    &id,
                    a.links.iter().map(|l| l.id.as_str()),
                )
            })?;
            a.check_locks(
                &a.links[index]
                    .members
                    .iter()
                    .map(|m| m.clip_id.clone())
                    .collect(),
            )?;
            a.links.remove(index);
        }
        Edit::ClipAudio {
            clip_ids,
            gain_milli,
            fade_in,
            fade_out,
            gain_curve,
            clear_gain_curve,
        } => {
            if clear_gain_curve && gain_curve.is_some() {
                return Err(invalid("Give gain_curve or clear_gain_curve, not both"));
            }
            let a = arrangement(project)?;
            a.selection(&clip_ids, Linked::Include)?;
            for name in &clip_ids {
                let (t, c) = a.locate(name)?;
                unlocked(&a.tracks[t])?;
                if a.tracks[t].kind != Kind::Audio {
                    return Err(invalid(format!(
                        "clip {name:?} is on video track {:?}; gain and fades apply to audio-track clips",
                        a.tracks[t].id
                    )));
                }
                let clip = &mut a.tracks[t].clips[c];
                clip.gain_milli = gain_milli.unwrap_or(clip.gain_milli);
                clip.fade_in = fade_in.unwrap_or(clip.fade_in);
                clip.fade_out = fade_out.unwrap_or(clip.fade_out);
                if clear_gain_curve {
                    clip.gain_curve = None;
                } else if gain_curve.is_some() {
                    clip.gain_curve = gain_curve.clone();
                }
            }
        }
        Edit::ClipTransform {
            clip_ids,
            transform,
        } => {
            let a = arrangement(project)?;
            a.selection(&clip_ids, Linked::Include)?;
            for name in &clip_ids {
                let (t, c) = a.locate(name)?;
                unlocked(&a.tracks[t])?;
                a.tracks[t].clips[c].transform = transform.clone();
            }
        }
        Edit::AudioDynamics { track_id, dynamics } => {
            let a = arrangement(project)?;
            match track_id {
                None => a.master = dynamics,
                Some(id) => {
                    let index = a.track(&id)?;
                    let track = &mut a.tracks[index];
                    unlocked(track)?;
                    if track.kind != Kind::Audio {
                        return Err(invalid(format!(
                            "track {id:?} is a video track; dynamics apply to audio tracks"
                        )));
                    }
                    track.dynamics = dynamics;
                }
            }
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
                    if track.kind == Kind::Audio && clip.adjusts_audio() {
                        let e = clip.envelope()?;
                        c.envelope =
                            Some((e.offset + x.minus(clip.start)?.units(SAMPLES)?, e.samples));
                    }
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
    serde_json::json!({"profile":"native-tracks-v1","operation":"tracks.edit","edits":["transition_set","transition_remove","split","slip","roll","slide","insert","overwrite","ripple_delete","create","promote","add","state","order","duration","place","move","remove","link","unlink","clip_audio","clip_transform","audio_dynamics"],"maximum_tracks":32,"maximum_model_clips":1000,"maximum_render_clips":64,"render_chunking":"windows_over_64_clips_render_as_exactly_joined_chunks","render_frame_rates":"native_project_rate","audio_clock":48000,"video":"highest_enabled_opaque_track","audio":"sum_enabled_stereo_tracks_then_clip_pcm16","clip_audio":{"gain_milli":[0,4000],"gain_curve":{"clock":"clip_source_time","maximum_keys":2000,"interpolation":["hold","linear","ease_in","ease_out","ease_in_out"],"rounding":"curve_value_nearest_then_one_sample_rounding"},"fades":"linear","maximum_fade_seconds":60,"overlapping_fades":false,"rounding":"nearest_ties_away_from_zero_per_clip_before_track_mixing","cuts_inside_fades":"rejected","trims":"fades_follow_clip_edges"},"dynamics":crate::dynamics::capabilities(),"overlay_transform":{"order":["crop","divisor","opacity","position"],"divisor":[1,8],"shrink":"floor_of_block_mean_per_channel_opaque_sources","opacity":"alpha_x_opacity_over_255_nearest","outside_canvas":"clipped"},"gaps":"implicit_black_and_silence_with_explicit_duration","collision":["reject","replace_clips"],"replacement":"whole_colliding_clips_and_linked_partners;not_interval_overwrite","links":["include","reject_partial"],"locks":"source_target_and_replaced_partner_tracks","source_profile":"reference_FFV1_RGB8_PCM16_stereo","supported_saved_sessions":true,"transitions":{"video":["dissolve","dip_black","wipe_left","wipe_right"],"audio":["dissolve","dip_black"],"handles":"explicit_before_cut_and_after_cut","sampling":"frame_or_sample_centers","video_rounding":"nearest_ties_up","audio_rounding":"nearest_ties_away_from_zero_before_track_mixing","overlapping_intervals":false},"ripple":true,"boundary_edits":{"fragment_ids":"explicit_right_clip_and_right_link_maps","link_anchors":"preserved","span_selection":"whole_linked_partner_tracks","end_policy":["keep","resize"],"transition_policy":["reject_affected","remove_affected"],"split_transitions":"preserve_clock_across_continuous_source_fragments"},"network":false})
}
