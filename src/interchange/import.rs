use super::*;

const ITEM: &[&str] = &[
    "OTIO_SCHEMA",
    "metadata",
    "name",
    "source_range",
    "effects",
    "markers",
    "enabled",
    "color",
];
fn item_fields(value: &Value, extra: &[&str], path: &str, a: &mut Analysis) -> Result<()> {
    let mut fields_allowed = ITEM.to_vec();
    fields_allowed.extend(extra);
    fields(value, &fields_allowed, path, a)
}
fn children<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>> {
    let values = array(&value["children"], path)?;
    if values.len() > 3000 {
        return Err(invalid(path, "Too many composition children"));
    }
    Ok(values)
}
fn named(a: &mut Analysis, value: &Value, path: &str, id: &str) {
    a.names
        .push(json!({"path":path,"source_name":value["name"],"native_id":id}));
}

pub(super) fn prepare(request: &Import, document: &Value, a: &mut Analysis) -> Result<Project> {
    let mut project = Project {
        schema_version: 1,
        id: request.id.clone(),
        revision: 0,
        width: request.width,
        height: request.height,
        frame_rate: request.frame_rate,
        assets: vec![],
        clips: vec![],
        tracks: None,
        sequences: vec![],
        preview_scale: None,
        transfer: None,
    };
    project.validate()?;
    schema(document, &["Timeline.1"], "$")?;
    fields(
        document,
        &[
            "OTIO_SCHEMA",
            "metadata",
            "name",
            "global_start_time",
            "tracks",
        ],
        "$",
        a,
    )?;
    named(a, document, "$", &project.id);
    if !document["global_start_time"].is_null() {
        let start = rational(&document["global_start_time"], "$/global_start_time")?;
        if start.num != 0 {
            a.loss(
                "$/global_start_time",
                "display_origin",
                "Global display/timecode origin is omitted; native timeline starts at zero",
                false,
            );
        }
    }
    let stack = &document["tracks"];
    schema(stack, &["Stack.1"], "$/tracks")?;
    item_fields(stack, &["children"], "$/tracks", a)?;
    if !stack["source_range"].is_null() || !boolean(&stack["enabled"], "$/tracks/enabled")? {
        return Err(invalid(
            "$/tracks",
            "A trimmed or disabled root stack is not supported",
        ));
    }
    let inputs = children(stack, "$/tracks")?;
    if inputs.len() > 32 {
        return Err(invalid("$/tracks", "At most 32 tracks are supported"));
    }
    let mut bindings = BTreeMap::new();
    let mut asset_ids = BTreeMap::new();
    for binding in &request.bindings {
        if binding.target_url.is_empty()
            || binding.target_url.len() > 4096
            || binding.target_url.contains('\0')
            || bindings
                .insert(binding.target_url.as_str(), &binding.asset)
                .is_some()
        {
            return Err(error(
                "INVALID_BINDING",
                "Provide unique nonempty reference strings of at most 4096 bytes",
            ));
        }
        if let Some(previous) = asset_ids.insert(&binding.asset.id, &binding.asset)
            && previous != &binding.asset
        {
            return Err(error(
                "INVALID_BINDING",
                "One asset ID cannot refer to different bindings",
            ));
        }
    }
    let mut used = BTreeMap::new();
    let mut tracks = Vec::new();
    let mut duration = Time::ZERO;
    let mut total_clips = 0usize;
    for (ti, value) in inputs.iter().enumerate() {
        let path = format!("$/tracks/children/{ti}");
        schema(value, &["Track.1"], &path)?;
        item_fields(value, &["kind", "children"], &path, a)?;
        if !value["source_range"].is_null() {
            return Err(invalid(
                &path,
                "Track trimming requires an explicit flattened source range",
            ));
        }
        let kind = match text(&value["kind"], &path)? {
            "Video" => tracks::Kind::Video,
            "Audio" => tracks::Kind::Audio,
            _ => return Err(invalid(&path, "Unknown track kind")),
        };
        let mut track = tracks::Track {
            id: format!("track-{ti}"),
            kind,
            locked: false,
            enabled: boolean(&value["enabled"], &path)?,
            clips: vec![],
            transitions: vec![],
            composite: Default::default(),
        };
        named(a, value, &path, &track.id);
        let values = children(value, &path)?;
        let mut ids = BTreeMap::<usize, String>::new();
        let mut cursor = Time::ZERO;
        for (ci, child) in values.iter().enumerate() {
            let child_path = format!("{path}/children/{ci}");
            match text(&child["OTIO_SCHEMA"], &child_path)? {
                "Transition.1" => {}
                "Gap.1" => {
                    item_fields(child, &[], &child_path, a)?;
                    let (_, length) = range(&child["source_range"], &child_path)?;
                    cursor = cursor.plus(length)?;
                }
                "Clip.1" | "Clip.2" => {
                    total_clips += 1;
                    if total_clips > 1000 {
                        return Err(invalid(&child_path, "At most 1000 clips are supported"));
                    }
                    let version = child["OTIO_SCHEMA"].as_str().expect("matched schema");
                    item_fields(
                        child,
                        if version == "Clip.1" {
                            &["media_reference"]
                        } else {
                            &["media_references", "active_media_reference_key"]
                        },
                        &child_path,
                        a,
                    )?;
                    let reference = if version == "Clip.1" {
                        &child["media_reference"]
                    } else {
                        let key = text(&child["active_media_reference_key"], &child_path)?;
                        let references = object(&child["media_references"], &child_path)?;
                        if references.len() > 1 {
                            a.loss(
                                &format!("{child_path}/media_references"),
                                "inactive_references",
                                "Inactive media alternatives are omitted",
                                false,
                            );
                        }
                        references.get(key).ok_or_else(|| {
                            invalid(&child_path, "Active media reference does not exist")
                        })?
                    };
                    let reference_path = format!("{child_path}/reference");
                    schema(reference, &["ExternalReference.1"], &reference_path)?;
                    fields(
                        reference,
                        &[
                            "OTIO_SCHEMA",
                            "metadata",
                            "name",
                            "available_range",
                            "available_image_bounds",
                            "target_url",
                        ],
                        &reference_path,
                        a,
                    )?;
                    let url = text(&reference["target_url"], &reference_path)?;
                    let asset = bindings.get(url).ok_or_else(|| {
                        invalid(
                            &reference_path,
                            &format!("No explicit local binding for {url}"),
                        )
                    })?;
                    let (origin, available) =
                        range(&reference["available_range"], &reference_path)?;
                    if available.compare(asset.duration)?.is_ne() {
                        return Err(invalid(
                            &reference_path,
                            "Available duration must match the bound native asset",
                        ));
                    }
                    let (start, length) = if child["source_range"].is_null() {
                        (origin, available)
                    } else {
                        range(&child["source_range"], &child_path)?
                    };
                    let source_in = difference(start, origin, &child_path)?;
                    if length.num == 0 || source_in.plus(length)?.compare(available)?.is_gt() {
                        return Err(invalid(
                            &child_path,
                            "Clip range must be nonempty and within available media",
                        ));
                    }
                    let id = format!("clip-{ti}-{ci}");
                    named(a, child, &child_path, &id);
                    if boolean(&child["enabled"], &child_path)? {
                        ids.insert(ci, id.clone());
                        track.clips.push(tracks::TrackClip {
                            id,
                            asset_id: asset.id.clone(),
                            sequence_id: None,
                            start: cursor,
                            source_in,
                            duration: length,
                        });
                        used.insert(asset.id.clone(), (*asset).clone());
                    } else {
                        a.loss(&format!("{child_path}/enabled"),"disabled_clip","Disabled clip becomes an empty interval; its editable definition is omitted",false);
                    }
                    cursor = cursor.plus(length)?;
                }
                _ => {
                    return Err(invalid(
                        &child_path,
                        "Nested compositions and unknown child schemas are unsupported",
                    ));
                }
            }
        }
        for (ci, child) in values
            .iter()
            .enumerate()
            .filter(|(_, v)| v["OTIO_SCHEMA"] == "Transition.1")
        {
            let child_path = format!("{path}/children/{ci}");
            fields(
                child,
                &[
                    "OTIO_SCHEMA",
                    "metadata",
                    "name",
                    "in_offset",
                    "out_offset",
                    "transition_type",
                    "enabled",
                ],
                &child_path,
                a,
            )?;
            let before = positive(rational(&child["in_offset"], &child_path)?, &child_path)?;
            let after = positive(rational(&child["out_offset"], &child_path)?, &child_path)?;
            let transition_type = text(&child["transition_type"], &child_path)?;
            if transition_type != "SMPTE_Dissolve" || !boolean(&child["enabled"], &child_path)? {
                a.loss(
                    &child_path,
                    "transition_omitted",
                    format!("Unsupported or disabled transition {transition_type} becomes a cut"),
                    false,
                );
                continue;
            }
            let left = ci.checked_sub(1).and_then(|i| ids.get(&i));
            let right = ids.get(&(ci + 1));
            let (Some(left), Some(right)) = (left, right) else {
                return Err(invalid(
                    &child_path,
                    "Dissolves require two adjacent enabled clips; gap/end fades are unsupported",
                ));
            };
            let left_clip = track
                .clips
                .iter()
                .find(|c| &c.id == left)
                .expect("enabled left");
            let right_clip = track
                .clips
                .iter()
                .find(|c| &c.id == right)
                .expect("enabled right");
            if before.compare(left_clip.duration)?.is_gt()
                || after.compare(right_clip.duration)?.is_gt()
            {
                return Err(invalid(
                    &child_path,
                    "Transition interval must fit its two adjacent clips",
                ));
            }
            let id = format!("transition-{ti}-{ci}");
            named(a, child, &child_path, &id);
            track.transitions.push(tracks::Transition {
                id,
                left_id: left.clone(),
                right_id: right.clone(),
                before,
                after,
                kind: tracks::TransitionKind::Dissolve,
            });
        }
        if cursor.compare(duration)?.is_gt() {
            duration = cursor;
        }
        tracks.push(track);
    }
    project.assets = used.into_values().collect();
    project.tracks = Some(tracks::Arrangement {
        duration,
        tracks,
        links: vec![],
    });
    // Native validation independently enforces clocks, handles, collisions and bounds.
    if let Err(e) = project.validate() {
        return Err(invalid(
            "$",
            &format!("Native timeline rejects {}: {}", e.code, e.message),
        ));
    }
    Ok(project)
}
