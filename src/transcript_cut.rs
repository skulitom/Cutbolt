//! Original word-selection planning over the existing native interval editor.
use crate::{
    Result, error,
    model::{Operation, Project},
    time::Time,
    track_edit, tracks,
    transcript::{self, Document, Origin, fingerprint},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

/// Cut boundary clock: `video` (project frame rate) or `audio` (48 kHz samples).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Clock {
    Video,
    Audio,
}
/// Boundary rounding: `strict` rejects word boundaries off the clock; `outward` widens each cut to the surrounding ticks.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Rounding {
    Strict,
    Outward,
}
/// `reject` fails the plan; `allow_reported` proceeds and lists the affected word IDs in the result.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    Reject,
    AllowReported,
}
/// Inclusive selection of consecutive transcript words to cut.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WordRange {
    /// ID of the first selected word.
    pub first_id: String,
    /// ID of the last selected word; not before `first_id` in document order.
    pub last_id: String,
}
/// Word-based cut request for transcript.plan, bound to one native media clip.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// ID of a concrete media clip (not a nested sequence) whose asset matches the transcript source identity and duration.
    pub clip_id: String,
    /// Tracks to ripple-delete on; must include the bound clip's track.
    pub track_ids: Vec<String>,
    /// 1-128 word selections; overlapping or touching selections merge into one cut.
    pub ranges: Vec<WordRange>,
    /// Clock that cut boundaries must align to.
    pub clock: Clock,
    /// How boundaries that are off the clock are handled.
    pub rounding: Rounding,
    /// Policy for selected words whose origin is `estimated`.
    pub estimates: Policy,
    /// Policy for unselected words that rounding would cut.
    pub collateral: Policy,
    /// Linked-clip policy for the ripple delete.
    pub links: tracks::Linked,
    /// Whether the timeline end shrinks with the cut.
    pub end_policy: track_edit::EndPolicy,
    /// Handling of transitions touched by a cut.
    pub transitions: track_edit::TransitionPolicy,
}
/// Content-bound cut plan returned by transcript.plan; apply it unchanged in a transcript.cut operation.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    /// Plan format version; must be 1.
    pub schema_version: u32,
    /// SHA-256 hex of the planned project; any later project change makes the plan stale.
    pub project_fingerprint: String,
    /// SHA-256 hex fingerprint of the transcript document the plan was made from.
    pub document_fingerprint: String,
    /// Cut request the plan recomputes when applied.
    pub spec: Spec,
}
#[derive(Clone, Debug, Serialize)]
struct Interval {
    start: Time,
    end: Time,
    word_ids: Vec<String>,
}
struct Built {
    operations: Vec<Operation>,
    intervals: Vec<Interval>,
    requested: Vec<Value>,
    collateral: Vec<String>,
    estimates: Vec<String>,
    projection: Vec<Value>,
    after: Project,
}
fn snap(time: Time, rate: Time, rounding: Rounding, upper: bool) -> Result<Time> {
    match rounding {
        Rounding::Strict => {
            time.units(rate)?;
            Ok(time)
        }
        Rounding::Outward => {
            time.validate()?;
            rate.validate()?;
            if rate.num == 0 {
                return Err(transcript::invalid("Cut clock must be positive"));
            }
            let n = time.num as u128 * rate.num as u128;
            let d = time.den as u128 * rate.den as u128;
            let ticks = if upper { n.div_ceil(d) } else { n / d };
            let ticks =
                u64::try_from(ticks).map_err(|_| error("TIME_OVERFLOW", "Cut clock overflow"))?;
            Time::new(ticks, 1)?.times(Time::new(rate.den, rate.num)?)
        }
    }
}
fn before(time: Time, intervals: &[Interval]) -> Result<Time> {
    let mut shift = Time::ZERO;
    for interval in intervals {
        if interval.end.compare(time)?.is_le() {
            shift = shift.plus(interval.end.minus(interval.start)?)?;
        }
    }
    time.minus(shift)
}
fn fresh(used: &mut BTreeSet<String>, prefix: &str, counter: &mut usize) -> String {
    loop {
        let id = format!("{prefix}-{:05}", *counter);
        *counter += 1;
        if used.insert(id.clone()) {
            return id;
        }
    }
}
fn build(project: &Project, document: &Document, spec: &Spec) -> Result<Built> {
    project.validate()?;
    document.validate()?;
    if spec.ranges.is_empty() || spec.ranges.len() > 128 {
        return Err(transcript::invalid("Select 1..128 word ranges"));
    }
    let arrangement = project
        .tracks
        .as_ref()
        .ok_or_else(|| error("UNSUPPORTED_TIMELINE", "Text cuts require native tracks"))?;
    let (binding_track, binding_clip) = arrangement.locate(&spec.clip_id)?;
    if !spec
        .track_ids
        .contains(&arrangement.tracks[binding_track].id)
    {
        return Err(transcript::invalid(
            "The bound source clip's track must be explicitly selected",
        ));
    }
    let clip = &arrangement.tracks[binding_track].clips[binding_clip];
    if clip.sequence_id.is_some() {
        return Err(error(
            "UNSUPPORTED_TRANSCRIPT_BINDING",
            "Bind a concrete media clip, not a nested sequence",
        ));
    }
    let asset = project
        .assets
        .iter()
        .find(|a| a.id == clip.asset_id)
        .ok_or_else(|| error("MISSING_MEDIA", &clip.asset_id))?;
    if asset.identity.as_ref() != Some(&document.source.identity)
        || asset.duration.compare(document.source.duration)?.is_ne()
    {
        return Err(error(
            "TRANSCRIPT_SOURCE_MISMATCH",
            "Bound clip does not reference the transcript's exact source media and duration",
        ));
    }
    let source_end = clip.source_in.plus(clip.duration)?;
    let clock = match spec.clock {
        Clock::Video => project.frame_rate,
        Clock::Audio => transcript::RATE,
    };
    let index: BTreeMap<_, _> = document
        .words
        .iter()
        .enumerate()
        .map(|(i, w)| (w.id.as_str(), i))
        .collect();
    let mut intervals = Vec::new();
    let mut requested = Vec::new();
    let mut selected = BTreeSet::new();
    let mut estimates = BTreeSet::new();
    for selection in &spec.ranges {
        let first = *index
            .get(selection.first_id.as_str())
            .ok_or_else(|| error("MISSING_WORD", &selection.first_id))?;
        let last = *index
            .get(selection.last_id.as_str())
            .ok_or_else(|| error("MISSING_WORD", &selection.last_id))?;
        if first > last {
            return Err(transcript::invalid(
                "Word ranges must follow document order",
            ));
        }
        let from = document.words[first].start;
        let to = document.words[last].end;
        if from.compare(clip.source_in)?.is_lt() || to.compare(source_end)?.is_gt() {
            return Err(error(
                "WORD_OUTSIDE_CLIP",
                "Selected source words must be fully inside the bound clip",
            ));
        }
        let mut names = Vec::new();
        for word in &document.words[first..=last] {
            names.push(word.id.clone());
            selected.insert(word.id.clone());
            if word.origin == Origin::Estimated {
                estimates.insert(word.id.clone());
            }
        }
        let raw_start = clip.start.plus(from.minus(clip.source_in)?)?;
        let raw_end = clip.start.plus(to.minus(clip.source_in)?)?;
        let start = snap(raw_start, clock, spec.rounding, false)?;
        let end = snap(raw_end, clock, spec.rounding, true)?;
        if start.compare(clip.start)?.is_lt() || end.compare(clip.end()?)?.is_gt() {
            return Err(error(
                "CUT_OUTSIDE_CLIP",
                "Clock rounding would cross the bound clip's edges",
            ));
        }
        requested.push(json!({"first_id":selection.first_id,"last_id":selection.last_id,"source_start":from,"source_end":to,
            "timeline_start":raw_start,"timeline_end":raw_end,"cut_start":start,"cut_end":end,
            "leading_expansion":raw_start.minus(start)?,"trailing_expansion":end.minus(raw_end)?}));
        intervals.push(Interval {
            start,
            end,
            word_ids: names,
        });
    }
    if !estimates.is_empty() && matches!(spec.estimates, Policy::Reject) {
        return Err(error(
            "ESTIMATED_BOUNDARY",
            "Selected words contain automatic estimates; correct them or explicitly allow reported estimates",
        ));
    }
    intervals.sort_by(|a, b| a.start.compare(b.start).expect("validated times"));
    let mut merged: Vec<Interval> = Vec::new();
    for interval in intervals {
        if let Some(last) = merged.last_mut()
            && interval.start.compare(last.end)?.is_le()
        {
            if interval.end.compare(last.end)?.is_gt() {
                last.end = interval.end;
            }
            last.word_ids.extend(interval.word_ids);
            last.word_ids.sort();
            last.word_ids.dedup();
        } else {
            merged.push(interval);
        }
    }
    let mut collateral = Vec::new();
    let mut projection = Vec::new();
    for word in &document.words {
        if word.end.compare(clip.source_in)?.is_le() || word.start.compare(source_end)?.is_ge() {
            projection.push(json!({"id":word.id,"state":"outside_binding","fragments":[]}));
            continue;
        }
        let bound_start = if word.start.compare(clip.source_in)?.is_lt() {
            clip.source_in
        } else {
            word.start
        };
        let bound_end = if word.end.compare(source_end)?.is_gt() {
            source_end
        } else {
            word.end
        };
        let binding_complete = bound_start == word.start && bound_end == word.end;
        let start = clip.start.plus(bound_start.minus(clip.source_in)?)?;
        let end = clip.start.plus(bound_end.minus(clip.source_in)?)?;
        let mut cursor = start;
        let mut pieces = Vec::new();
        let mut touched = false;
        for cut in &merged {
            if cut.end.compare(start)?.is_le() || cut.start.compare(end)?.is_ge() {
                continue;
            }
            touched = true;
            if cut.start.compare(cursor)?.is_gt() {
                pieces.push((cursor, cut.start));
            }
            if cut.end.compare(cursor)?.is_gt() {
                cursor = if cut.end.compare(end)?.is_lt() {
                    cut.end
                } else {
                    end
                };
            }
        }
        if cursor.compare(end)?.is_lt() {
            pieces.push((cursor, end));
        }
        if touched && !selected.contains(&word.id) {
            collateral.push(word.id.clone());
        }
        let mut fragments = Vec::new();
        for (a, b) in &pieces {
            fragments.push(
                json!({"source_start":clip.source_in.plus(a.minus(clip.start)?)?,
                "source_end":clip.source_in.plus(b.minus(clip.start)?)?,
                "timeline_start":before(*a,&merged)?,"timeline_end":before(*b,&merged)?}),
            );
        }
        projection.push(json!({"id":word.id,"binding_complete":binding_complete,"state":if pieces.is_empty(){"removed"}else if touched{"partially_removed"}else{"retained"},"fragments":fragments}));
    }
    if !collateral.is_empty() && matches!(spec.collateral, Policy::Reject) {
        return Err(error(
            "ADJACENT_WORD_CUT",
            "Clock rounding would cut an unselected word; change boundaries or explicitly allow the reported collateral words",
        ));
    }
    let mut after = project.clone();
    let mut operations = Vec::new();
    let mut used: BTreeSet<String> = arrangement
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().map(|c| c.id.clone()))
        .chain(arrangement.links.iter().map(|l| l.id.clone()))
        .collect();
    let prefix = format!("text-{}", &document.fingerprint()?[..12]);
    let mut counter = 0;
    for interval in merged.iter().rev() {
        let duration = interval.end.minus(interval.start)?;
        let current = after.tracks.as_ref().expect("native arrangement retained");
        let (clips, links) = track_edit::ripple_fragments(
            current,
            &spec.track_ids,
            interval.start,
            duration,
            spec.links,
        )?;
        let mut names = |ids: Vec<String>| {
            ids.into_iter()
                .map(|id| track_edit::NewId {
                    id,
                    new_id: fresh(&mut used, &prefix, &mut counter),
                })
                .collect()
        };
        let edit = tracks::Edit::RippleDelete(track_edit::RippleDelete {
            track_ids: spec.track_ids.clone(),
            start: interval.start,
            duration,
            links: spec.links,
            right_clip_ids: names(clips),
            right_link_ids: names(links),
            end_policy: spec.end_policy,
            transitions: spec.transitions,
        });
        tracks::edit(&mut after, edit.clone())?;
        after.validate()?;
        operations.push(Operation::Tracks { edit });
    }
    Ok(Built {
        operations,
        intervals: merged,
        requested,
        collateral,
        estimates: estimates.into_iter().collect(),
        projection,
        after,
    })
}
fn verify_binding(project: &Project, spec: &Spec, input_root: &Path) -> Result<()> {
    let arrangement = project
        .tracks
        .as_ref()
        .ok_or_else(|| error("UNSUPPORTED_TIMELINE", "Text cuts require native tracks"))?;
    let (track, clip) = arrangement.locate(&spec.clip_id)?;
    let asset_id = &arrangement.tracks[track].clips[clip].asset_id;
    let asset = project
        .assets
        .iter()
        .find(|a| &a.id == asset_id)
        .ok_or_else(|| error("MISSING_MEDIA", asset_id))?;
    let identity = asset.identity.as_ref().ok_or_else(|| {
        error(
            "TRANSCRIPT_SOURCE_MISMATCH",
            "Bound media requires an exact identity",
        )
    })?;
    let path = crate::media::project_file(std::path::Path::new(&asset.path), input_root)?;
    if std::fs::metadata(&path)?.len() != identity.bytes
        || crate::media::file_hash(&path)? != identity.sha256
    {
        return Err(error(
            "MEDIA_CHANGED",
            "Bound clip's local source no longer matches its identity",
        ));
    }
    Ok(())
}
pub fn plan(
    project: &Project,
    document: &Document,
    expected_revision: u64,
    expected_document_fingerprint: &str,
    spec: &Spec,
    input_root: &Path,
) -> Result<Value> {
    document.verify_source(input_root)?;
    if project.revision != expected_revision {
        return Err(error(
            "REVISION_CONFLICT",
            "Expected project revision does not match",
        ));
    }
    if document.fingerprint()? != expected_document_fingerprint {
        return Err(error(
            "TRANSCRIPT_CONFLICT",
            "Expected transcript fingerprint does not match",
        ));
    }
    let built = build(project, document, spec)?;
    verify_binding(project, spec, input_root)?;
    let plan = Plan {
        schema_version: 1,
        project_fingerprint: fingerprint(project)?,
        document_fingerprint: document.fingerprint()?,
        spec: spec.clone(),
    };
    document.verify_source(input_root)?;
    verify_binding(project, spec, input_root)?;
    Ok(
        json!({"plan":plan,"operation":{"op":"transcript.cut","document":document,"plan":plan},
        "native_operations":built.operations,"requested":built.requested,"merged_cuts":built.intervals,
        "collateral_word_ids":built.collateral,"estimated_word_ids":built.estimates,"word_projection":built.projection,
        "result_duration":built.after.tracks.as_ref().expect("native arrangement").duration,"applied":false}),
    )
}
pub(crate) fn apply(project: &mut Project, document: &Document, plan: &Plan) -> Result<()> {
    if plan.schema_version != 1
        || !transcript::hash_ok(&plan.project_fingerprint)
        || !transcript::hash_ok(&plan.document_fingerprint)
    {
        return Err(transcript::invalid(
            "Invalid text-cut plan version or fingerprints",
        ));
    }
    if fingerprint(project)? != plan.project_fingerprint {
        return Err(error(
            "TEXT_PLAN_STALE",
            "Project content or revision changed after text-cut planning",
        ));
    }
    if document.fingerprint()? != plan.document_fingerprint {
        return Err(error(
            "TRANSCRIPT_CONFLICT",
            "Transcript changed after text-cut planning",
        ));
    }
    let built = build(project, document, &plan.spec)?;
    *project = built.after;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(n: u64, d: u64) -> Time {
        Time::new(n, d).unwrap()
    }
    fn fixture() -> (Project, Document, Spec) {
        let mut document = transcript::tests::fixture();
        document.source.duration = t(6, 1);
        document.range_start = Time::ZERO;
        document.range_duration = t(6, 1);
        document.words = (0..4)
            .map(|i| transcript::Word {
                id: format!("w{i}"),
                text: ["red", "square", "blue", "circle"][i].into(),
                start: t(12 + 4 * i as u64, 10),
                end: t(14 + 4 * i as u64, 10),
                origin: Origin::Corrected,
                probability_milli: None,
                alignment: None,
            })
            .collect();
        let mut project = Project::new("text-fixture".into(), 16, 16, t(25, 1)).unwrap();
        project.assets.push(crate::model::Asset {
            id: "voice".into(),
            path: "voice.mkv".into(),
            duration: t(6, 1),
            metadata: Default::default(),
            identity: Some(document.source.identity.clone()),
            proxy: None,
        });
        let clips: Vec<_> = ["v", "a"]
            .iter()
            .enumerate()
            .map(|(i, id)| tracks::TrackClip {
                id: (*id).into(),
                asset_id: "voice".into(),
                sequence_id: None,
                start: t(2, 1),
                source_in: t(48000 + 7 * i as u64, 48000),
                duration: t(3, 1),
            })
            .collect();
        project.tracks = Some(tracks::Arrangement {
            duration: t(6, 1),
            tracks: clips
                .iter()
                .enumerate()
                .map(|(i, clip)| tracks::Track {
                    id: format!("track{i}"),
                    kind: if i == 0 {
                        tracks::Kind::Video
                    } else {
                        tracks::Kind::Audio
                    },
                    locked: false,
                    enabled: true,
                    clips: vec![clip.clone()],
                    transitions: Vec::new(),
                    composite: Default::default(),
                })
                .collect(),
            links: vec![tracks::Link {
                id: "av".into(),
                members: clips
                    .iter()
                    .map(|c| tracks::Anchor {
                        clip_id: c.id.clone(),
                        start: c.start,
                        source_in: c.source_in,
                    })
                    .collect(),
            }],
        });
        let spec = Spec {
            clip_id: "v".into(),
            track_ids: vec!["track0".into()],
            ranges: vec![WordRange {
                first_id: "w1".into(),
                last_id: "w2".into(),
            }],
            clock: Clock::Video,
            rounding: Rounding::Strict,
            estimates: Policy::Reject,
            collateral: Policy::Reject,
            links: tracks::Linked::Include,
            end_policy: track_edit::EndPolicy::Resize,
            transitions: track_edit::TransitionPolicy::RejectAffected,
        };
        project.validate().unwrap();
        document.validate().unwrap();
        (project, document, spec)
    }
    #[test]
    fn cut_maps_source_and_linked_sample_offset_and_keeps_immutable_input() {
        let (project, document, spec) = fixture();
        let original = project.clone();
        let built = build(&project, &document, &spec).unwrap();
        assert_eq!(project, original);
        assert_eq!(built.intervals.len(), 1);
        assert_eq!(built.intervals[0].start, t(13, 5));
        assert_eq!(built.intervals[0].end, t(16, 5));
        let after = built.after.tracks.unwrap();
        assert_eq!(after.duration, t(27, 5));
        for (i, track) in after.tracks.iter().enumerate() {
            assert_eq!(track.clips.len(), 2);
            let left = &track.clips[0];
            let right = &track.clips[1];
            assert_eq!((left.start, left.duration), (t(2, 1), t(3, 5)));
            assert_eq!((right.start, right.duration), (t(13, 5), t(9, 5)));
            assert_eq!(left.source_in, t(48000 + 7 * i as u64, 48000));
            assert_eq!(right.source_in, t(105600 + 7 * i as u64, 48000));
        }
        assert_eq!(after.links.len(), 2);
        assert_eq!(
            built.projection[3]["fragments"][0]["timeline_start"],
            json!(t(14, 5))
        );
    }
    #[test]
    fn overlapping_and_separated_word_ranges_have_exact_union_and_end_policy() {
        let (project, document, mut spec) = fixture();
        spec.ranges = vec![
            WordRange {
                first_id: "w0".into(),
                last_id: "w1".into(),
            },
            WordRange {
                first_id: "w1".into(),
                last_id: "w2".into(),
            },
        ];
        let merged = build(&project, &document, &spec).unwrap();
        assert_eq!(merged.intervals.len(), 1);
        assert_eq!(merged.operations.len(), 1);
        assert_eq!(merged.intervals[0].word_ids, ["w0", "w1", "w2"]);
        spec.ranges = vec![
            WordRange {
                first_id: "w1".into(),
                last_id: "w1".into(),
            },
            WordRange {
                first_id: "w3".into(),
                last_id: "w3".into(),
            },
        ];
        let separate = build(&project, &document, &spec).unwrap();
        assert_eq!(separate.operations.len(), 2);
        assert_eq!(separate.after.tracks.as_ref().unwrap().duration, t(28, 5));
        let clips = &separate.after.tracks.as_ref().unwrap().tracks[0].clips;
        assert_eq!(
            clips
                .iter()
                .map(|c| (c.start, c.source_in, c.duration))
                .collect::<Vec<_>>(),
            vec![
                (t(2, 1), t(1, 1), t(3, 5)),
                (t(13, 5), t(9, 5), t(3, 5)),
                (t(16, 5), t(13, 5), t(7, 5))
            ]
        );
        spec.end_policy = track_edit::EndPolicy::Keep;
        assert_eq!(
            build(&project, &document, &spec)
                .unwrap()
                .after
                .tracks
                .unwrap()
                .duration,
            t(6, 1)
        );
    }
    #[test]
    fn rounding_reports_even_a_partially_bound_neighbor_and_estimates_are_explicit() {
        let (project, mut document, mut spec) = fixture();
        document.words[0].start = t(98, 100);
        document.words[0].end = t(101, 100);
        document.words[1].start = t(203, 200);
        document.words[1].end = t(103, 100);
        spec.ranges = vec![WordRange {
            first_id: "w1".into(),
            last_id: "w1".into(),
        }];
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "UNALIGNED_TIME"
        );
        spec.rounding = Rounding::Outward;
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "ADJACENT_WORD_CUT"
        );
        spec.collateral = Policy::AllowReported;
        let cut = build(&project, &document, &spec).unwrap();
        assert_eq!(cut.collateral, ["w0"]);
        assert_eq!(cut.projection[0]["binding_complete"], false);
        assert_eq!(cut.projection[0]["state"], "removed");
        assert_eq!(cut.requested[0]["leading_expansion"], json!(t(3, 200)));
        document.words[1].origin = Origin::Estimated;
        document.words[1].probability_milli = Some(800);
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "ESTIMATED_BOUNDARY"
        );
        spec.estimates = Policy::AllowReported;
        assert_eq!(build(&project, &document, &spec).unwrap().estimates, ["w1"]);
    }
    #[test]
    fn linked_locks_partial_selection_and_stale_plans_reject_atomically() {
        let (mut project, document, mut spec) = fixture();
        spec.links = tracks::Linked::RejectPartial;
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "LINKED_SELECTION"
        );
        spec.links = tracks::Linked::Include;
        project.tracks.as_mut().unwrap().tracks[1].locked = true;
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "TRACK_LOCKED"
        );
        project.tracks.as_mut().unwrap().tracks[1].locked = false;
        let plan = Plan {
            schema_version: 1,
            project_fingerprint: fingerprint(&project).unwrap(),
            document_fingerprint: document.fingerprint().unwrap(),
            spec,
        };
        let original = project.clone();
        let mut changed = document.clone();
        changed.words[1].text = "changed".into();
        assert_eq!(
            apply(&mut project, &changed, &plan).unwrap_err().code,
            "TRANSCRIPT_CONFLICT"
        );
        assert_eq!(project, original);
        project.revision += 1;
        assert_eq!(
            apply(&mut project, &document, &plan).unwrap_err().code,
            "TEXT_PLAN_STALE"
        );
        let operation = Operation::TranscriptCut {
            document: Box::new(document),
            plan,
        };
        let applied = original.apply(0, vec![operation.clone()]).unwrap();
        assert_eq!(applied.revision, 1);
        assert!(
            original
                .apply(0, vec![operation.clone(), operation])
                .is_err()
        );
        assert_eq!(original.revision, 0);
    }
    #[test]
    fn transition_removal_requires_explicit_policy_for_a_word_cut() {
        let (project, document, mut spec) = fixture();
        let project = project
            .apply(
                0,
                vec![Operation::Tracks {
                    edit: tracks::Edit::Split(track_edit::Split {
                        clip_ids: vec!["v".into()],
                        at: t(3, 1),
                        links: tracks::Linked::Include,
                        right_clip_ids: vec![
                            track_edit::NewId {
                                id: "v".into(),
                                new_id: "v-right".into(),
                            },
                            track_edit::NewId {
                                id: "a".into(),
                                new_id: "a-right".into(),
                            },
                        ],
                        right_link_ids: vec![track_edit::NewId {
                            id: "av".into(),
                            new_id: "av-right".into(),
                        }],
                    }),
                }],
            )
            .unwrap();
        let project = project
            .apply(
                1,
                ["v", "a"]
                    .iter()
                    .enumerate()
                    .map(|(i, id)| Operation::Tracks {
                        edit: tracks::Edit::TransitionSet {
                            track_id: format!("track{i}"),
                            transition: tracks::Transition {
                                id: format!("fx-{id}"),
                                left_id: (*id).into(),
                                right_id: format!("{id}-right"),
                                before: t(1, 5),
                                after: t(1, 5),
                                kind: tracks::TransitionKind::Dissolve,
                            },
                        },
                    })
                    .collect(),
            )
            .unwrap();
        spec.clip_id = "v-right".into();
        spec.ranges = vec![WordRange {
            first_id: "w2".into(),
            last_id: "w2".into(),
        }];
        assert_eq!(
            build(&project, &document, &spec).err().unwrap().code,
            "TRANSITION_CONFLICT"
        );
        spec.transitions = track_edit::TransitionPolicy::RemoveAffected;
        let result = build(&project, &document, &spec).unwrap();
        assert!(
            result
                .after
                .tracks
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .all(|t| t.transitions.is_empty())
        );
        assert!(
            project
                .tracks
                .as_ref()
                .unwrap()
                .tracks
                .iter()
                .all(|t| t.transitions.len() == 1)
        );
        assert_eq!(result.after.duration().unwrap(), t(29, 5));
    }
    #[test]
    fn outward_clock_bounds_are_minimal_for_fractional_rates() {
        for rate in [
            Time::new(25, 1).unwrap(),
            Time::new(30000, 1001).unwrap(),
            transcript::RATE,
        ] {
            let tick = Time::new(rate.den, rate.num).unwrap();
            for n in 0..2000 {
                let t = Time::new(n, 48000).unwrap();
                let lo = snap(t, rate, Rounding::Outward, false).unwrap();
                let hi = snap(t, rate, Rounding::Outward, true).unwrap();
                assert!(lo.compare(t).unwrap().is_le() && hi.compare(t).unwrap().is_ge());
                assert!(t.minus(lo).unwrap().compare(tick).unwrap().is_lt());
                assert!(hi.minus(t).unwrap().compare(tick).unwrap().is_lt());
                lo.units(rate).unwrap();
                hi.units(rate).unwrap();
                assert_eq!(snap(lo, rate, Rounding::Strict, false).unwrap(), lo);
            }
        }
    }
}
