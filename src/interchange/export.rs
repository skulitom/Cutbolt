use super::*;

fn item(schema: &str, name: &str) -> Value {
    json!({"OTIO_SCHEMA":schema,"name":name,"metadata":{},"effects":[],"markers":[],"enabled":true,"color":null,"source_range":null})
}
fn gap(length: Time) -> Value {
    let mut value = item("Gap.1", "");
    value["source_range"] = tr(Time::ZERO, length);
    value
}

pub(super) fn prepare(project: &Project, input_root: &Path) -> Result<(Value, Analysis)> {
    project.validate()?;
    let root = media::input_root(input_root)?;
    let mut a = Analysis::default();
    let mut prepared = project.clone();
    if prepared.tracks.is_none() {
        tracks::edit(
            &mut prepared,
            tracks::Edit::Promote {
                video_track_id: "picture".into(),
                audio_track_id: "sound".into(),
            },
        )?;
    }
    let arrangement = prepared.tracks.as_ref().expect("promoted tracks");
    if arrangement.tracks.len() > 32
        || arrangement
            .tracks
            .iter()
            .map(|t| t.clips.len())
            .sum::<usize>()
            > 1000
        || project.assets.len() > 1000
    {
        a.loss(
            "$",
            "profile_bounds",
            "This profile allows 32 tracks, 1000 clips and 1000 assets",
            true,
        );
    }
    if !project.sequences.is_empty() {
        a.loss(
            "$/sequences",
            "sequence_definitions",
            "Reusable sequence definitions cannot be represented in this flat profile",
            true,
        );
    }
    if !arrangement.links.is_empty() {
        a.loss(
            "$/tracks/links",
            "editing_links",
            "Linked-edit groups are omitted; explicit video/audio timing is retained",
            false,
        );
    }
    if project.preview_scale.is_some() {
        a.loss(
            "$/preview_scale",
            "preview_selection",
            "Proxy-preview selection is omitted; full-quality media is referenced",
            false,
        );
    }
    let mut references = BTreeMap::new();
    for asset in &project.assets {
        let path = checked_asset(asset, &root)?;
        if !arrangement
            .tracks
            .iter()
            .any(|track| track.clips.iter().any(|clip| clip.asset_id == asset.id))
        {
            a.loss(
                &format!("$/assets/{}", asset.id),
                "unused_asset",
                "Unused registry asset is omitted from the editorial document",
                false,
            );
        }
        let url = path
            .strip_prefix(&root)
            .map_err(|_| {
                error(
                    "PATH_OUTSIDE_ROOT",
                    "Asset is outside the export media root",
                )
            })?
            .to_str()
            .ok_or_else(|| error("INVALID_PATH", "Interchange paths require Unicode"))?
            .replace('\\', "/");
        if asset.proxy.is_some() {
            a.loss(
                &format!("$/assets/{}", asset.id),
                "proxy_binding",
                "Proxy attachment is omitted from interchange",
                false,
            );
        }
        if !asset.metadata.is_empty() {
            a.loss(
                &format!("$/assets/{}/metadata", asset.id),
                "asset_metadata",
                "Native registry metadata is omitted from interchange",
                false,
            );
        }
        references.insert(asset.id.clone(),json!({"OTIO_SCHEMA":"ExternalReference.1","name":asset.id,"metadata":{},"target_url":url,
            "available_range":tr(Time::ZERO,asset.duration),"available_image_bounds":null}));
    }
    let mut output_tracks = Vec::new();
    for (ti, track) in arrangement.tracks.iter().enumerate() {
        let path = format!("$/tracks/{ti}");
        if track.locked {
            a.loss(
                &format!("{path}/locked"),
                "track_lock",
                "Track editing lock is omitted from interchange",
                false,
            );
        }
        let mut value = item("Track.1", &track.id);
        value["kind"] = json!(if track.kind == tracks::Kind::Video {
            "Video"
        } else {
            "Audio"
        });
        value["enabled"] = json!(track.enabled);
        let mut clips = track.clips.iter().collect::<Vec<_>>();
        clips.sort_by(|x, y| x.start.compare(y.start).expect("validated times"));
        let mut transition_anchors = BTreeSet::new();
        for effect in &track.transitions {
            if !transition_anchors.insert(&effect.right_id) {
                a.loss(
                    &format!("{path}/transitions/{}", effect.id),
                    "multiple_boundary_effects",
                    "Multiple effects at one cut cannot map to one editorial transition",
                    true,
                );
            }
        }
        let mut children = Vec::new();
        let mut cursor = Time::ZERO;
        for (ci, clip) in clips.iter().enumerate() {
            if clip.sequence_id.is_some() {
                a.loss(
                    &format!("{path}/clips/{ci}"),
                    "nested_sequence",
                    "Nested sequence instances require a separate interchange profile",
                    true,
                );
                continue;
            }
            if clip.start.compare(cursor)?.is_gt() {
                children.push(gap(clip.start.minus(cursor)?));
            }
            if let Some(effect) = track.transitions.iter().find(|t| t.right_id == clip.id) {
                let previous = ci.checked_sub(1).map(|i| clips[i]);
                if previous.is_none_or(|p| {
                    p.id != effect.left_id
                        || effect.before.compare(p.duration).is_ok_and(|v| v.is_gt())
                }) || effect.after.compare(clip.duration)?.is_gt()
                {
                    a.loss(
                        &format!("{path}/transitions/{}", effect.id),
                        "split_transition_clock",
                        "Transition spans split clips and cannot map to two adjacent OTIO items",
                        true,
                    );
                } else if effect.kind != tracks::TransitionKind::Dissolve {
                    a.loss(
                        &format!("{path}/transitions/{}", effect.id),
                        "transition_omitted",
                        "Only generic cross-dissolves are represented; this effect becomes a cut",
                        false,
                    );
                } else {
                    children.push(json!({"OTIO_SCHEMA":"Transition.1","name":effect.id,"metadata":{},"in_offset":rt(effect.before),"out_offset":rt(effect.after),"transition_type":"SMPTE_Dissolve"}));
                }
            }
            let mut child = item("Clip.2", &clip.id);
            child["source_range"] = tr(clip.source_in, clip.duration);
            child["media_references"] = json!({"DEFAULT_MEDIA":references.get(&clip.asset_id).ok_or_else(||error("MISSING_MEDIA",&clip.asset_id))?});
            child["active_media_reference_key"] = json!("DEFAULT_MEDIA");
            children.push(child);
            cursor = clip.start.plus(clip.duration)?;
        }
        if cursor.compare(arrangement.duration)?.is_lt() {
            children.push(gap(arrangement.duration.minus(cursor)?));
        }
        value["children"] = json!(children);
        output_tracks.push(value);
    }
    if output_tracks.is_empty() && arrangement.duration.num > 0 {
        let mut track = item("Track.1", "empty-timeline");
        track["kind"] = json!("Video");
        track["children"] = json!([gap(arrangement.duration)]);
        output_tracks.push(track);
    }
    let mut stack = item("Stack.1", "tracks");
    stack["children"] = json!(output_tracks);
    let document = json!({"OTIO_SCHEMA":"Timeline.1","name":project.id,"metadata":{},"global_start_time":null,"tracks":stack});
    Ok((document, a))
}
