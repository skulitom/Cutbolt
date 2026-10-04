//! Original grouped boundary edits. The caller validates and commits the entire candidate.
use crate::{
    Result, error,
    time::Time,
    tracks::{self, Arrangement, Link, Linked, Shift, TrackClip},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
/// Explicit ID for the right-hand survivor when a clip or link is split.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewId {
    /// Existing clip or link ID; its left survivor keeps this ID.
    pub id: String,
    /// New ID for the right survivor, 1-128 bytes and unused in this arrangement.
    pub new_id: String,
}
/// Split edit: cut every selected clip at one absolute timeline time.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Split {
    /// Clips to split, 1-1000 unique IDs.
    pub clip_ids: Vec<String>,
    /// Absolute timeline time strictly inside every selected and included clip.
    pub at: Time,
    /// Linked-partner policy.
    pub links: Linked,
    /// One mapping per split clip, including link-included partners; missing or surplus mappings fail.
    pub right_clip_ids: Vec<NewId>,
    /// One mapping per link whose members are split; the right-side group gets the new ID.
    pub right_link_ids: Vec<NewId>,
}
/// Slip edit: shift selected clips' source ranges without moving them.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Slip {
    /// Clips to slip, 1-1000 unique IDs.
    pub clip_ids: Vec<String>,
    /// Offset applied to each `source_in`; forward shows later source content.
    pub shift: Shift,
    /// Linked-partner policy.
    pub links: Linked,
}
/// Roll edit: move the cuts after the selected clips.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Roll {
    /// Clips before the cuts, 1-1000 unique IDs; each needs an exactly adjacent next clip.
    pub left_ids: Vec<String>,
    /// Cut movement; forward lengthens the left clips and shortens their next neighbors.
    pub shift: Shift,
    /// Linked-partner policy; `include` also adds linked neighbors.
    pub links: Linked,
}
/// Slide edit: move middle clips between their neighbors.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Slide {
    /// Middle clips to move, 1-1000 unique IDs; each needs exactly adjacent previous and next clips.
    pub clip_ids: Vec<String>,
    /// Timeline offset; forward lengthens the previous neighbors and shortens the next ones.
    pub shift: Shift,
    /// Linked-partner policy; `include` also adds linked neighbors.
    pub links: Linked,
}
/// Arrangement end after an interval edit: `keep` leaves the explicit end unchanged; `resize` adds an insert's duration, extends to an overwrite's end if later, or subtracts a ripple deletion's duration.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EndPolicy {
    Keep,
    Resize,
}
/// Handling of transitions an interval edit touches: `reject_affected` fails; `remove_affected` removes each such transition entirely first.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransitionPolicy {
    RejectAffected,
    RemoveAffected,
}
/// New clip placed inside an insert or overwrite interval; it is not linked automatically.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    /// Selected or link-expanded track that receives the clip.
    pub track_id: String,
    /// New clip with an unused ID, lying entirely inside the edited interval.
    pub clip: TrackClip,
}
/// Insert edit: open space on selected tracks and optionally fill it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Insert {
    /// Tracks to edit, 1-32 unique IDs; `links` may add more.
    pub track_ids: Vec<String>,
    /// Insertion time in rational seconds; not after the arrangement end.
    pub at: Time,
    /// Positive length of the opened space.
    pub duration: Time,
    /// New clips placed inside the opened space; may be empty.
    pub clips: Vec<Placement>,
    /// Linked-partner policy.
    pub links: Linked,
    /// New IDs for right parts of clips that straddle `at`.
    pub right_clip_ids: Vec<NewId>,
    /// New IDs for right-side groups of links whose members are split.
    pub right_link_ids: Vec<NewId>,
    /// Arrangement end policy.
    pub end_policy: EndPolicy,
    /// Policy for transitions the insertion point falls strictly inside.
    pub transitions: TransitionPolicy,
}
/// Overwrite edit: replace an interval on selected tracks.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Overwrite {
    /// Tracks to edit, 1-32 unique IDs; `links` may add more.
    pub track_ids: Vec<String>,
    /// Interval start in rational seconds; not after the arrangement end.
    pub at: Time,
    /// Positive length of the replaced interval.
    pub duration: Time,
    /// Replacement clips inside the interval; may be empty, leaving empty space.
    pub clips: Vec<Placement>,
    /// Linked-partner policy.
    pub links: Linked,
    /// New IDs for right parts of clips that survive on both sides of the interval.
    pub right_clip_ids: Vec<NewId>,
    /// New IDs for right-side groups of links whose members survive on both sides.
    pub right_link_ids: Vec<NewId>,
    /// Arrangement end policy.
    pub end_policy: EndPolicy,
    /// Policy for transitions overlapping the interval.
    pub transitions: TransitionPolicy,
}
/// Ripple-delete edit: remove an interval on selected tracks and close it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RippleDelete {
    /// Tracks to edit, 1-32 unique IDs; `links` may add more.
    pub track_ids: Vec<String>,
    /// Interval start in rational seconds.
    pub start: Time,
    /// Positive length; the interval must end within the arrangement.
    pub duration: Time,
    /// Linked-partner policy.
    pub links: Linked,
    /// New IDs for right parts of clips that survive on both sides of the interval.
    pub right_clip_ids: Vec<NewId>,
    /// New IDs for right-side groups of links whose members survive on both sides.
    pub right_link_ids: Vec<NewId>,
    /// Arrangement end policy.
    pub end_policy: EndPolicy,
    /// Policy for transitions overlapping the interval.
    pub transitions: TransitionPolicy,
}
fn invalid(message: impl Into<String>) -> crate::Error {
    error("INVALID_TRACK_EDIT", message)
}
fn shifted(time: Time, shift: &Shift, inverse: bool) -> Result<Time> {
    shift.amount.validate()?;
    if shift.backward != inverse {
        time.minus(shift.amount)
    } else {
        time.plus(shift.amount)
    }
}
fn clip<'a>(a: &'a Arrangement, id: &str) -> Result<&'a TrackClip> {
    let (t, c) = a.locate(id)?;
    Ok(&a.tracks[t].clips[c])
}
fn clip_mut<'a>(a: &'a mut Arrangement, id: &str) -> Result<&'a mut TrackClip> {
    let (t, c) = a.locate(id)?;
    Ok(&mut a.tracks[t].clips[c])
}
fn names(set: &BTreeSet<String>) -> Vec<String> {
    set.iter().cloned().collect()
}
fn fresh_map(values: Vec<NewId>, existing: BTreeSet<String>) -> Result<BTreeMap<String, String>> {
    let mut used = existing;
    let mut result = BTreeMap::new();
    for value in values {
        tracks::id(&value.new_id)?;
        if !used.insert(value.new_id.clone()) {
            return Err(error("DUPLICATE_ID", value.new_id));
        }
        if result.insert(value.id, value.new_id).is_some() {
            return Err(invalid("Duplicate fragment mapping"));
        }
    }
    Ok(result)
}
fn old_clip_ids(a: &Arrangement) -> BTreeSet<String> {
    a.tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
        .collect()
}
fn new_clip_map(a: &Arrangement, values: Vec<NewId>) -> Result<BTreeMap<String, String>> {
    fresh_map(values, old_clip_ids(a))
}
fn new_link_map(a: &Arrangement, values: Vec<NewId>) -> Result<BTreeMap<String, String>> {
    fresh_map(values, a.links.iter().map(|l| l.id.clone()).collect())
}
// A transformation lists the zero, one or two chronological survivors of each old clip.
// Preserve original anchors so validation can detect unequal content shifts; never rebase
// a group merely to make a synchronization failure disappear.
fn rebuild_links(
    a: &mut Arrangement,
    parts: &BTreeMap<String, Vec<TrackClip>>,
    mut ids: BTreeMap<String, String>,
) -> Result<()> {
    let mut links = Vec::new();
    for link in &a.links {
        let counts: Vec<_> = link
            .members
            .iter()
            .map(|m| parts.get(&m.clip_id).map_or(1, Vec::len))
            .collect();
        let count = counts[0];
        if counts.iter().any(|n| *n != count) {
            return Err(error(
                "SYNC_CONFLICT",
                format!(
                    "Link {} would retain unequal fragment counts; unlink explicitly first",
                    link.id
                ),
            ));
        }
        for n in 0..count {
            let id = if n == 0 {
                link.id.clone()
            } else {
                ids.remove(&link.id).ok_or_else(|| {
                    error(
                        "SPLIT_ID_REQUIRED",
                        format!("Supply a right-link ID for {}", link.id),
                    )
                })?
            };
            let mut members = link.members.clone();
            for m in &mut members {
                if let Some(p) = parts.get(&m.clip_id) {
                    m.clip_id = p[n].id.clone();
                }
            }
            links.push(Link { id, members });
        }
    }
    if !ids.is_empty() {
        return Err(error("UNUSED_SPLIT_ID", "Unused right-link IDs"));
    }
    a.links = links;
    Ok(())
}
/// A part cut from `old` keeps the fades at the edges it shares with `old`. A cut inside a fade
/// would change what plays, so it is rejected.
fn keep_fades(old: &TrackClip, part: &mut TrackClip) -> Result<()> {
    if old.fade_in.num == 0 && old.fade_out.num == 0 {
        return Ok(());
    }
    let head = part.source_in.minus(old.source_in)?;
    let tail = old
        .source_in
        .plus(old.duration)?
        .minus(part.source_in.plus(part.duration)?)?;
    let fade_out_from = old.duration.minus(old.fade_out)?;
    for (cut, present) in [
        (head, head.num != 0),
        (old.duration.minus(tail)?, tail.num != 0),
    ] {
        if present && (cut.compare(old.fade_in)?.is_lt() || cut.compare(fade_out_from)?.is_gt()) {
            return Err(error(
                "INVALID_RANGE",
                format!(
                    "A cut {} s into clip {:?} falls inside its fades (fade_in {} s, fade_out {} s); shorten them with clip_audio first",
                    cut, old.id, old.fade_in, old.fade_out
                ),
            ));
        }
    }
    if head.num != 0 {
        part.fade_in = Time::ZERO;
    }
    if tail.num != 0 {
        part.fade_out = Time::ZERO;
    }
    Ok(())
}
fn apply_parts(a: &mut Arrangement, parts: &mut BTreeMap<String, Vec<TrackClip>>) -> Result<()> {
    for track in &a.tracks {
        for old in &track.clips {
            for part in parts.get_mut(&old.id).into_iter().flatten() {
                keep_fades(old, part)?;
            }
        }
    }
    for track in &mut a.tracks {
        track.clips = track
            .clips
            .iter()
            .flat_map(|c| parts.get(&c.id).cloned().unwrap_or_else(|| vec![c.clone()]))
            .collect();
        track.transitions.retain_mut(|e| {
            if let Some(p) = parts.get(&e.left_id) {
                let Some(last) = p.last() else {
                    return false;
                };
                e.left_id = last.id.clone();
            }
            if let Some(p) = parts.get(&e.right_id) {
                let Some(first) = p.first() else {
                    return false;
                };
                e.right_id = first.id.clone();
            }
            true
        });
    }
    Ok(())
}
pub(crate) fn split(a: &mut Arrangement, e: Split) -> Result<()> {
    let selected = a.selection(&e.clip_ids, e.links)?;
    a.check_locks(&selected)?;
    let mut ids = new_clip_map(a, e.right_clip_ids)?;
    let link_ids = new_link_map(a, e.right_link_ids)?;
    let mut parts = BTreeMap::new();
    for id in selected {
        let old = clip(a, &id)?;
        if !e.at.compare(old.start)?.is_gt() || !e.at.compare(old.end()?)?.is_lt() {
            return Err(error(
                "INVALID_RANGE",
                format!(
                    "Split time {} s must be strictly inside every selected clip; clip {id:?} spans {} s to {} s",
                    e.at,
                    old.start,
                    old.end()?
                ),
            ));
        }
        let left_duration = e.at.minus(old.start)?;
        let mut left = old.clone();
        left.duration = left_duration;
        let mut right = old.clone();
        right.id = ids.remove(&id).ok_or_else(|| {
            error(
                "SPLIT_ID_REQUIRED",
                format!("Supply a right-clip ID for {id}"),
            )
        })?;
        right.start = e.at;
        right.source_in = right.source_in.plus(left_duration)?;
        right.duration = old.end()?.minus(e.at)?;
        parts.insert(id, vec![left, right]);
    }
    if !ids.is_empty() {
        return Err(error("UNUSED_SPLIT_ID", "Unused right-clip IDs"));
    }
    rebuild_links(a, &parts, link_ids)?;
    apply_parts(a, &mut parts)?;
    Ok(())
}
pub(crate) fn slip(a: &mut Arrangement, e: Slip) -> Result<()> {
    let selected = a.selection(&e.clip_ids, e.links)?;
    a.check_locks(&selected)?;
    for id in selected {
        let c = clip_mut(a, &id)?;
        c.source_in = shifted(c.source_in, &e.shift, false)?;
    }
    Ok(())
}
fn neighbor(a: &Arrangement, id: &str, next: bool) -> Result<String> {
    let (t, c) = a.locate(id)?;
    let clip = &a.tracks[t].clips[c];
    for candidate in &a.tracks[t].clips {
        if candidate.id != id
            && if next {
                candidate.start.compare(clip.end()?)?.is_eq()
            } else {
                candidate.end()?.compare(clip.start)?.is_eq()
            }
        {
            return Ok(candidate.id.clone());
        }
    }
    Err(error(
        "MISSING_NEIGHBOR",
        format!(
            "Clip {id} needs an exactly adjacent {}",
            if next { "next clip" } else { "previous clip" }
        ),
    ))
}
fn neighbors(a: &Arrangement, set: &BTreeSet<String>, next: bool) -> Result<BTreeSet<String>> {
    set.iter().map(|id| neighbor(a, id, next)).collect()
}
fn expand(a: &Arrangement, set: BTreeSet<String>, links: Linked) -> Result<BTreeSet<String>> {
    a.selection(&names(&set), links)
}
fn disjoint(sets: &[&BTreeSet<String>]) -> Result<()> {
    let mut used = BTreeSet::new();
    for set in sets {
        for id in *set {
            if !used.insert(id) {
                return Err(invalid("A clip cannot occupy two boundary-edit roles"));
            }
        }
    }
    Ok(())
}
pub(crate) fn roll(a: &mut Arrangement, e: Roll) -> Result<()> {
    let mut left = a.selection(&e.left_ids, e.links)?;
    let right = loop {
        let right = expand(a, neighbors(a, &left, true)?, e.links)?;
        let more = expand(a, neighbors(a, &right, false)?, e.links)?;
        if more == left {
            break right;
        }
        left.extend(more);
    };
    disjoint(&[&left, &right])?;
    a.check_locks(&left)?;
    a.check_locks(&right)?;
    for id in left {
        let c = clip_mut(a, &id)?;
        c.duration = shifted(c.duration, &e.shift, false)?;
    }
    for id in right {
        let c = clip_mut(a, &id)?;
        c.start = shifted(c.start, &e.shift, false)?;
        c.source_in = shifted(c.source_in, &e.shift, false)?;
        c.duration = shifted(c.duration, &e.shift, true)?;
    }
    Ok(())
}
pub(crate) fn slide(a: &mut Arrangement, e: Slide) -> Result<()> {
    let mut middle = a.selection(&e.clip_ids, e.links)?;
    let (previous, next) = loop {
        let previous = expand(a, neighbors(a, &middle, false)?, e.links)?;
        let next = expand(a, neighbors(a, &middle, true)?, e.links)?;
        let mut more = neighbors(a, &previous, true)?;
        more.extend(neighbors(a, &next, false)?);
        more = expand(a, more, e.links)?;
        if more == middle {
            break (previous, next);
        }
        middle.extend(more);
    };
    disjoint(&[&previous, &middle, &next])?;
    a.check_locks(&previous)?;
    a.check_locks(&middle)?;
    a.check_locks(&next)?;
    for id in previous {
        let c = clip_mut(a, &id)?;
        c.duration = shifted(c.duration, &e.shift, false)?;
    }
    for id in middle {
        let c = clip_mut(a, &id)?;
        c.start = shifted(c.start, &e.shift, false)?;
    }
    for id in next {
        let c = clip_mut(a, &id)?;
        c.start = shifted(c.start, &e.shift, false)?;
        c.source_in = shifted(c.source_in, &e.shift, false)?;
        c.duration = shifted(c.duration, &e.shift, true)?;
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Mode {
    Insert,
    Overwrite,
    Ripple,
}
struct Span {
    mode: Mode,
    tracks: Vec<String>,
    start: Time,
    duration: Time,
    clips: Vec<Placement>,
    links: Linked,
    right_clips: Vec<NewId>,
    right_links: Vec<NewId>,
    end_policy: EndPolicy,
    transitions: TransitionPolicy,
}
fn affected(clip: &TrackClip, start: Time, end: Time, mode: Mode) -> Result<bool> {
    Ok(clip.end()?.compare(start)?.is_gt()
        && (!matches!(mode, Mode::Overwrite) || clip.start.compare(end)?.is_lt()))
}
fn selected_tracks(a: &Arrangement, e: &Span, end: Time) -> Result<BTreeSet<usize>> {
    if e.tracks.is_empty() || e.tracks.len() > 32 {
        return Err(invalid("Select 1..32 tracks"));
    }
    let mut selected = BTreeSet::new();
    for id in &e.tracks {
        if !selected.insert(a.track(id)?) {
            return Err(invalid("Duplicate selected tracks"));
        }
    }
    loop {
        let mut more = selected.clone();
        for link in &a.links {
            let mut touches = false;
            let mut members = Vec::new();
            for member in &link.members {
                let (t, c) = a.locate(&member.clip_id)?;
                members.push(t);
                touches |=
                    selected.contains(&t) && affected(&a.tracks[t].clips[c], e.start, end, e.mode)?;
            }
            if touches {
                for t in members {
                    if !selected.contains(&t) && matches!(e.links, Linked::RejectPartial) {
                        return Err(error(
                            "LINKED_SELECTION",
                            format!("Link {} requires another target track", link.id),
                        ));
                    }
                    more.insert(t);
                }
            }
        }
        if more == selected {
            break;
        }
        selected = more;
    }
    for t in &selected {
        tracks::unlocked(&a.tracks[*t])?;
    }
    Ok(selected)
}

/// Reuse the interval editor's linked-track selection when planning explicit fragment IDs.
pub(crate) fn ripple_fragments(
    a: &Arrangement,
    targets: &[String],
    start: Time,
    duration: Time,
    links: Linked,
) -> Result<(Vec<String>, Vec<String>)> {
    let end = start.plus(duration)?;
    let span = Span {
        mode: Mode::Ripple,
        tracks: targets.to_vec(),
        start,
        duration,
        clips: vec![],
        links,
        right_clips: vec![],
        right_links: vec![],
        end_policy: EndPolicy::Keep,
        transitions: TransitionPolicy::RejectAffected,
    };
    let selected = selected_tracks(a, &span, end)?;
    let mut clips = Vec::new();
    for index in selected {
        for clip in &a.tracks[index].clips {
            if clip.start.compare(start)?.is_lt() && clip.end()?.compare(end)?.is_gt() {
                clips.push(clip.id.clone());
            }
        }
    }
    let links = a
        .links
        .iter()
        .filter(|link| {
            link.members
                .iter()
                .any(|member| clips.contains(&member.clip_id))
        })
        .map(|link| link.id.clone())
        .collect();
    Ok((clips, links))
}
fn span(a: &mut Arrangement, fps: Time, e: Span) -> Result<()> {
    e.duration.validate()?;
    e.start.validate()?;
    let end = e.start.plus(e.duration)?;
    if e.duration.num == 0
        || e.start.compare(a.duration)?.is_gt()
        || (matches!(e.mode, Mode::Ripple) && end.compare(a.duration)?.is_gt())
    {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "Interval {} s to {end} s must be nonempty and start inside the timeline, 0 s to {} s; ripple must also end inside",
                e.start, a.duration
            ),
        ));
    }
    let selected = selected_tracks(a, &e, end)?;
    for t in &selected {
        let clock = a.tracks[*t].kind.clock(fps);
        e.start.units(clock)?;
        e.duration.units(clock)?;
    }
    if matches!(e.end_policy, EndPolicy::Resize) {
        a.duration = match e.mode {
            Mode::Insert => a.duration.plus(e.duration)?,
            Mode::Ripple => a.duration.minus(e.duration)?,
            Mode::Overwrite => {
                if end.compare(a.duration)?.is_gt() {
                    end
                } else {
                    a.duration
                }
            }
        };
    }
    let mut ids = new_clip_map(a, e.right_clips)?;
    let link_ids = new_link_map(a, e.right_links)?;
    let mut used = old_clip_ids(a);
    used.extend(ids.values().cloned());
    for placement in &e.clips {
        let t = a.track(&placement.track_id)?;
        if !selected.contains(&t)
            || placement.clip.start.compare(e.start)?.is_lt()
            || placement.clip.end()?.compare(end)?.is_gt()
        {
            return Err(invalid(
                "New placements must lie inside the inserted/replaced interval on a selected track",
            ));
        }
        tracks::id(&placement.clip.id)?;
        if !used.insert(placement.clip.id.clone()) {
            return Err(error("DUPLICATE_ID", &placement.clip.id));
        }
    }
    for t in &selected {
        let mut remove = BTreeSet::new();
        for transition in &a.tracks[*t].transitions {
            let (x, y) = a.tracks[*t].interval(transition)?;
            let intersects = if matches!(e.mode, Mode::Insert) {
                e.start.compare(x)?.is_gt() && e.start.compare(y)?.is_lt()
            } else {
                e.start.compare(y)?.is_lt() && end.compare(x)?.is_gt()
            };
            if intersects {
                if matches!(e.transitions, TransitionPolicy::RejectAffected) {
                    return Err(error(
                        "TRANSITION_CONFLICT",
                        format!("Interval intersects transition {}", transition.id),
                    ));
                }
                remove.insert(transition.id.clone());
            }
        }
        a.tracks[*t]
            .transitions
            .retain(|fx| !remove.contains(&fx.id));
    }
    let mut parts = BTreeMap::new();
    for t in selected {
        for old in &a.tracks[t].clips {
            if !affected(old, e.start, end, e.mode)? {
                continue;
            }
            let mut result = Vec::new();
            if matches!(e.mode, Mode::Insert) {
                if old.start.compare(e.start)?.is_lt() {
                    let mut left = old.clone();
                    left.duration = e.start.minus(old.start)?;
                    result.push(left);
                    let mut right = old.clone();
                    right.source_in = right.source_in.plus(e.start.minus(old.start)?)?;
                    right.start = e.start.plus(e.duration)?;
                    right.duration = old.end()?.minus(e.start)?;
                    result.push(right);
                } else {
                    let mut c = old.clone();
                    c.start = c.start.plus(e.duration)?;
                    result.push(c);
                }
            } else {
                if old.start.compare(e.start)?.is_lt() {
                    let mut left = old.clone();
                    left.duration = e.start.minus(old.start)?;
                    result.push(left);
                }
                if old.end()?.compare(end)?.is_gt() {
                    let mut right = old.clone();
                    let begin = if old.start.compare(end)?.is_gt() {
                        old.start
                    } else {
                        end
                    };
                    right.source_in = right.source_in.plus(begin.minus(old.start)?)?;
                    right.duration = old.end()?.minus(begin)?;
                    right.start = if matches!(e.mode, Mode::Ripple) {
                        begin.minus(e.duration)?
                    } else {
                        begin
                    };
                    result.push(right);
                }
            }
            if result.len() == 2 {
                result[1].id = ids.remove(&old.id).ok_or_else(|| {
                    error(
                        "SPLIT_ID_REQUIRED",
                        format!("Supply a right-clip ID for {}", old.id),
                    )
                })?;
            }
            parts.insert(old.id.clone(), result);
        }
    }
    if !ids.is_empty() {
        return Err(error("UNUSED_SPLIT_ID", "Unused right-clip IDs"));
    }
    rebuild_links(a, &parts, link_ids)?;
    apply_parts(a, &mut parts)?;
    for placement in e.clips {
        let t = a.track(&placement.track_id)?;
        a.tracks[t].clips.push(placement.clip);
    }
    Ok(())
}
pub(crate) fn insert(a: &mut Arrangement, fps: Time, e: Insert) -> Result<()> {
    span(
        a,
        fps,
        Span {
            mode: Mode::Insert,
            tracks: e.track_ids,
            start: e.at,
            duration: e.duration,
            clips: e.clips,
            links: e.links,
            right_clips: e.right_clip_ids,
            right_links: e.right_link_ids,
            end_policy: e.end_policy,
            transitions: e.transitions,
        },
    )
}
pub(crate) fn overwrite(a: &mut Arrangement, fps: Time, e: Overwrite) -> Result<()> {
    span(
        a,
        fps,
        Span {
            mode: Mode::Overwrite,
            tracks: e.track_ids,
            start: e.at,
            duration: e.duration,
            clips: e.clips,
            links: e.links,
            right_clips: e.right_clip_ids,
            right_links: e.right_link_ids,
            end_policy: e.end_policy,
            transitions: e.transitions,
        },
    )
}
pub(crate) fn ripple(a: &mut Arrangement, fps: Time, e: RippleDelete) -> Result<()> {
    span(
        a,
        fps,
        Span {
            mode: Mode::Ripple,
            tracks: e.track_ids,
            start: e.start,
            duration: e.duration,
            clips: vec![],
            links: e.links,
            right_clips: e.right_clip_ids,
            right_links: e.right_link_ids,
            end_policy: e.end_policy,
            transitions: e.transitions,
        },
    )
}
