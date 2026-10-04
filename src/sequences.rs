//! Reusable original track definitions, explicit references and bounded dependency validation.
use crate::{
    Result, error,
    model::Project,
    time::Time,
    tracks::{self, Arrangement, Edit, Kind},
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sequence {
    pub id: String,
    pub arrangement: Arrangement,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub multicam: Option<crate::multicam::Group>,
}
pub(crate) fn get<'a>(project: &'a Project, id: &str) -> Result<&'a Sequence> {
    project
        .sequences
        .iter()
        .find(|s| s.id == id)
        .ok_or_else(|| error("MISSING_SEQUENCE", id))
}
fn dependencies(sequence: &Sequence) -> BTreeSet<&str> {
    sequence
        .arrangement
        .tracks
        .iter()
        .flat_map(|t| &t.clips)
        .filter_map(|c| c.sequence_id.as_deref())
        .chain(
            sequence
                .multicam
                .iter()
                .flat_map(|g| &g.angles)
                .map(|a| a.sequence_id.as_str()),
        )
        .collect()
}
pub(crate) fn validate(project: &Project) -> Result<()> {
    let count = |a: &Arrangement| a.tracks.iter().map(|t| t.clips.len()).sum::<usize>();
    if project.sequences.len() > 32
        || project
            .sequences
            .iter()
            .map(|s| count(&s.arrangement))
            .sum::<usize>()
            + project.tracks.as_ref().map_or(0, count)
            > 1000
    {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Sequence catalogs support 32 definitions and 1000 total native clips",
        ));
    }
    let mut ids = BTreeSet::new();
    for s in &project.sequences {
        tracks::id(&s.id)?;
        if !ids.insert(&s.id) {
            return Err(error("DUPLICATE_ID", &s.id));
        }
    }
    for s in &project.sequences {
        s.arrangement.validate(project)?;
        crate::multicam::validate(s, project)?;
    }
    fn depth(
        project: &Project,
        id: &str,
        active: &mut BTreeSet<String>,
        done: &mut BTreeMap<String, usize>,
    ) -> Result<usize> {
        if let Some(n) = done.get(id) {
            return Ok(*n);
        }
        if !active.insert(id.to_owned()) {
            return Err(error("SEQUENCE_CYCLE", id));
        }
        let mut n = 1;
        for child in dependencies(get(project, id)?) {
            n = n.max(1 + depth(project, child, active, done)?);
        }
        active.remove(id);
        if n > 8 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "At most eight nested sequence levels are supported",
            ));
        }
        done.insert(id.to_owned(), n);
        Ok(n)
    }
    let mut done = BTreeMap::new();
    for s in &project.sequences {
        depth(project, &s.id, &mut BTreeSet::new(), &mut done)?;
    }
    Ok(())
}
pub(crate) fn create(project: &mut Project, id: String, duration: Time) -> Result<()> {
    if project.sequences.iter().any(|s| s.id == id) {
        return Err(error("DUPLICATE_ID", id));
    }
    project.sequences.push(Sequence {
        id,
        multicam: None,
        arrangement: Arrangement {
            duration,
            tracks: vec![],
            links: vec![],
        },
    });
    Ok(())
}
// Changing a definition changes all instances. Protect locked references at every ancestor,
// including disabled tracks and definitions that are not currently used by the root timeline.
pub(crate) fn check_dependents(project: &Project, id: &str) -> Result<()> {
    let mut changed = BTreeSet::from([id.to_owned()]);
    loop {
        let mut more = changed.clone();
        for s in &project.sequences {
            if let Some(group) = &s.multicam
                && group
                    .angles
                    .iter()
                    .any(|a| changed.contains(&a.sequence_id))
            {
                if group.locked {
                    return Err(error(
                        "TRACK_LOCKED",
                        "Multicam angle belongs to a locked group",
                    ));
                }
                more.insert(s.id.clone());
            }
            for t in &s.arrangement.tracks {
                if t.clips.iter().any(|c| {
                    c.sequence_id
                        .as_ref()
                        .is_some_and(|id| changed.contains(id))
                }) {
                    tracks::unlocked(t)?;
                    more.insert(s.id.clone());
                }
            }
        }
        if let Some(a) = &project.tracks {
            for t in &a.tracks {
                if t.clips.iter().any(|c| {
                    c.sequence_id
                        .as_ref()
                        .is_some_and(|id| changed.contains(id))
                }) {
                    tracks::unlocked(t)?;
                }
            }
        }
        if changed == more {
            return Ok(());
        }
        changed = more;
    }
}
pub(crate) fn edit(project: &mut Project, id: &str, edit: Edit) -> Result<()> {
    get(project, id)?;
    if get(project, id)?.multicam.is_some() {
        return Err(error(
            "MANAGED_SEQUENCE",
            "Edit retained camera choices with multicam.edit",
        ));
    }
    check_dependents(project, id)?;
    if matches!(edit, Edit::Create { .. } | Edit::Promote { .. }) {
        return Err(error(
            "INVALID_SEQUENCE_EDIT",
            "A child definition already has native tracks",
        ));
    }
    let mut child = project.clone();
    child.clips.clear();
    child.tracks = Some(get(project, id)?.arrangement.clone());
    tracks::edit(&mut child, edit)?;
    project
        .sequences
        .iter_mut()
        .find(|s| s.id == id)
        .expect("existing definition")
        .arrangement = child.tracks.expect("native definition");
    Ok(())
}
pub(crate) fn remove(project: &mut Project, id: &str) -> Result<()> {
    get(project, id)?;
    if project
        .sequences
        .iter()
        .any(|s| dependencies(s).contains(id))
        || project.tracks.as_ref().is_some_and(|a| {
            a.tracks
                .iter()
                .flat_map(|t| &t.clips)
                .any(|c| c.sequence_id.as_deref() == Some(id))
        })
    {
        return Err(error("SEQUENCE_IN_USE", id));
    }
    if get(project, id)?
        .arrangement
        .tracks
        .iter()
        .any(|t| t.locked)
    {
        return Err(error(
            "TRACK_LOCKED",
            "Unlock child tracks before removing the definition",
        ));
    }
    project.sequences.retain(|s| s.id != id);
    Ok(())
}
// Proxy selection follows enabled references of the corresponding stream kind.
pub(crate) fn used_assets(project: &Project) -> Result<BTreeSet<String>> {
    fn visit(
        project: &Project,
        a: &Arrangement,
        kind: Kind,
        seen: &mut BTreeSet<(String, bool)>,
        result: &mut BTreeSet<String>,
    ) -> Result<()> {
        for t in a.tracks.iter().filter(|t| t.enabled && t.kind == kind) {
            for c in &t.clips {
                if let Some(id) = &c.sequence_id {
                    if seen.insert((id.clone(), kind == Kind::Video)) {
                        visit(project, &get(project, id)?.arrangement, kind, seen, result)?;
                    }
                } else {
                    result.insert(c.asset_id.clone());
                }
            }
        }
        Ok(())
    }
    let mut result = project
        .clips
        .iter()
        .filter_map(|c| c.asset_id.clone())
        .collect();
    if let Some(a) = &project.tracks {
        for kind in [Kind::Video, Kind::Audio] {
            visit(project, a, kind, &mut BTreeSet::new(), &mut result)?;
        }
    }
    Ok(result)
}
pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"operations":["sequence.create","sequence.edit","sequence.remove"],"reference":"native_clip_sequence_id_exclusive_with_asset_id","maximum_definitions":32,"maximum_depth":8,"maximum_total_native_clips":1000,"maximum_render_clip_handle_inspections":256,"time_mapping":"unit_rate_exact_source_in_window","video":"opaque_child_output","audio":"child_mix_saturates_to_pcm16_before_parent_mixing","locks":"definition_tracks_and_all_transitive_parent_references","saved_sessions":true,"network":false})
}

pub(crate) fn root_instances(project: &Project, changed: &str) -> BTreeSet<String> {
    let mut affected = BTreeSet::from([changed.to_owned()]);
    loop {
        let mut more = affected.clone();
        for s in &project.sequences {
            if dependencies(s).iter().any(|id| affected.contains(*id)) {
                more.insert(s.id.clone());
            }
        }
        if more == affected {
            break;
        }
        affected = more;
    }
    project
        .tracks
        .iter()
        .flat_map(|a| &a.tracks)
        .flat_map(|t| &t.clips)
        .filter(|c| {
            c.sequence_id
                .as_ref()
                .is_some_and(|id| affected.contains(id))
        })
        .map(|c| c.id.clone())
        .collect()
}
