//! Frame and range previews for sequential timelines and placed tracks, including gaps.
use crate::{At, Result, error, media, model::Project, render, scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{path::Path, time::Duration};
const FPS: Time = Time { num: 25, den: 1 };
pub(crate) fn validate(project: &Project) -> Result<u64> {
    project.validate()?;
    if let Some(a) = &project.tracks {
        // Previews read only the window they show; long ranges render as chunks.
        if project.frame_rate.compare(FPS)?.is_ne() {
            return Err(error(
                "UNSUPPORTED_TIMELINE",
                "Track previews require 25 fps",
            ));
        }
        return a.duration.units(FPS);
    }
    crate::render::clock::rate(project.frame_rate)?;
    if project.clips.is_empty() {
        return Err(error(
            "UNSUPPORTED_TIMELINE",
            "Previews require a supported native frame rate and at least one sequential item",
        ));
    }
    for clip in &project.clips {
        clip.source_in.units(project.frame_rate)?;
        if clip.duration.units(project.frame_rate)? > 180_000 && clip.gap {
            return Err(error(
                "LIMIT_EXCEEDED",
                "A rendered gap supports at most 180000 frames",
            ));
        }
    }
    project.duration()?.units(project.frame_rate)
}
/// Message suffix naming the start of the last previewable frame of a `total`-frame timeline.
fn last_frame(project: &Project, total: u64) -> Result<String> {
    if total == 0 {
        return Ok("; the timeline is empty".into());
    }
    let frame = Time::new(project.frame_rate.den, project.frame_rate.num)?;
    Ok(format!(
        "; the last frame starts at {} s",
        Time::new(total - 1, 1)?.times(frame)?
    ))
}
pub(crate) fn read_frame(
    project: &Project,
    input_root: &Path,
    time: Time,
) -> Result<(Vec<u8>, Value)> {
    media::input_root(input_root)?;
    let preview_scale = project.preview_scale;
    let mapped = crate::proxy::preview_project(project, input_root)?;
    let project = &mapped;
    let total = validate(project)?;
    let selected = time.units(project.frame_rate).at(|| "time".into())?;
    if selected >= total {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "Preview time {time} s must be before the timeline end {} s{}",
                project.duration()?,
                last_frame(project, total)?
            ),
        ));
    }
    if project.width as u64 * project.height as u64 > 8_000_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Frame preview supports at most 8M pixels",
        ));
    }
    if let Some(a) = &project.tracks {
        if a.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .any(|c| c.sequence_id.is_some())
        {
            let (pixels, sources) = crate::track_render::read_frame(project, input_root, time)?;
            let visible = a.visible(time)?;
            let transition_id = visible
                .map(|(t, _)| t.transition_at(time))
                .transpose()?
                .flatten()
                .map(|e| &e.id);
            return Ok((
                pixels,
                json!({"project_revision":project.revision,"time":time,"timeline_frame":selected,"track_id":visible.map(|(t,_)| &t.id),"sequence_id":visible.and_then(|(_,c)| c.sequence_id.as_ref()),"transition_id":transition_id,"source":null,"sources":sources,"width":project.width,"height":project.height,"source_quality":if preview_scale.is_some(){"proxy"}else{"original"},"preview_scale":preview_scale}),
            ));
        }
        // Alpha overlays need the composited graph rather than the single-clip fast path.
        let mut overlays = Vec::new();
        for track in a
            .tracks
            .iter()
            .filter(|t| t.enabled && !t.composite.is_opaque())
        {
            for clip in &track.clips {
                if !time.compare(clip.start)?.is_lt() && time.compare(clip.end()?)?.is_lt() {
                    overlays.push(&track.id);
                }
            }
        }
        if !overlays.is_empty() {
            let (pixels, sources) = crate::track_render::read_frame(project, input_root, time)?;
            let visible = a.visible(time)?;
            return Ok((
                pixels,
                json!({"project_revision":project.revision,"time":time,"timeline_frame":selected,"track_id":visible.map(|(t,_)| &t.id),"overlay_track_ids":overlays,"transition_id":visible.map(|(t,_)| t.transition_at(time)).transpose()?.flatten().map(|e| &e.id),"source":null,"sources":sources,"width":project.width,"height":project.height,"source_quality":if preview_scale.is_some(){"proxy"}else{"original"},"preview_scale":preview_scale}),
            ));
        }
        if let Some((track, _)) = a.visible(time)?
            && let Some(effect) = track.transition_at(time)?
        {
            let (pixels, sources) = crate::track_render::read_frame(project, input_root, time)?;
            return Ok((
                pixels,
                json!({"project_revision":project.revision,"time":time,"timeline_frame":selected,"track_id":track.id,"transition_id":effect.id,"source":null,"sources":sources,"width":project.width,"height":project.height,"source_quality":if preview_scale.is_some(){"proxy"}else{"original"},"preview_scale":preview_scale}),
            ));
        }
        let mut single = project.clone();
        single.tracks = None;
        single.clips.clear();
        let mut track_id = None;
        if let Some((track, clip)) = a.visible(time)? {
            let mut c = clip.legacy();
            c.source_in = c.source_in.plus(time.minus(clip.start)?)?;
            c.duration = Time::new(1, 25)?;
            single.clips.push(c);
            track_id = Some(track.id.clone());
        } else {
            single.clips.push(crate::model::Clip {
                id: "track-gap".into(),
                asset_id: None,
                gap: true,
                source_in: Time::ZERO,
                duration: Time::new(1, 25)?,
            });
        }
        let (pixels, mut receipt) = read_frame(&single, input_root, Time::ZERO)?;
        receipt["time"] = json!(time);
        receipt["timeline_frame"] = json!(selected);
        receipt["track_id"] = json!(track_id);
        receipt["source_quality"] = json!(if preview_scale.is_some() {
            "proxy"
        } else {
            "original"
        });
        receipt["preview_scale"] = json!(preview_scale);
        return Ok((pixels, receipt));
    }
    let mut offset = 0;
    let clip = project
        .clips
        .iter()
        .find(|clip| {
            let end = offset
                + clip
                    .duration
                    .units(project.frame_rate)
                    .expect("validated time");
            if selected < end {
                true
            } else {
                offset = end;
                false
            }
        })
        .expect("validated range");
    if clip.gap {
        let pixels = vec![0; project.width as usize * project.height as usize * 3];
        let receipt = json!({"project_revision":project.revision,"time":time,"timeline_frame":selected,"clip_id":clip.id,"gap":true,"source":null,"source_frame":null,"width":project.width,"height":project.height,"source_quality":if preview_scale.is_some(){"proxy"}else{"original"},"preview_scale":preview_scale});
        return Ok((pixels, receipt));
    }
    let asset = project
        .assets
        .iter()
        .find(|a| Some(&a.id) == clip.asset_id.as_ref())
        .expect("validated asset");
    let path = media::project_file(Path::new(&asset.path), input_root)?;
    let source = render::inspect_reference_at(
        &path,
        project.width,
        project.height,
        project.frame_rate,
        &media::Uncontrolled,
    )?;
    crate::registry::verify_source(asset, &source)?;
    let source_end = clip.source_in.plus(clip.duration)?;
    if source_end.units(project.frame_rate)? > source.frames {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "clip {:?} needs source {} s to {source_end} s but asset {:?} decodes to {} s ({} frames)",
                clip.id,
                clip.source_in,
                asset.id,
                Time::new(source.frames, 1)?
                    .times(Time::new(project.frame_rate.den, project.frame_rate.num)?)?,
                source.frames
            ),
        ));
    }
    let source_frame = clip.source_in.units(project.frame_rate)? + selected - offset;
    let args = [
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-protocol_whitelist".into(),
        "file,pipe".into(),
        "-i".into(),
        path.to_string_lossy().into_owned(),
        "-vf".into(),
        format!("select=eq(n\\,{source_frame})"),
        "-frames:v".into(),
        "1".into(),
        "-an".into(),
        "-pix_fmt".into(),
        "rgb24".into(),
        "-f".into(),
        "rawvideo".into(),
        "pipe:1".into(),
    ];
    let pixels = media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(120))?;
    if pixels.len() != project.width as usize * project.height as usize * 3 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Preview frame size differs",
        ));
    }
    if media::file_hash(&path)? != source.sha256 {
        return Err(error("MEDIA_CHANGED", "Source changed during preview"));
    }
    let receipt = json!({"project_revision":project.revision,"time":time,"timeline_frame":selected,"clip_id":clip.id,"source_frame":source_frame,"source":source,"width":project.width,"height":project.height,"source_quality":if preview_scale.is_some() {"proxy"} else {"original"},"preview_scale":preview_scale});
    Ok((pixels, receipt))
}
pub fn frame(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    time: Time,
) -> Result<Value> {
    let (pixels, mut receipt) = read_frame(project, input_root, time)?;
    let output = render::destination_extension(output, output_root, "png")?;
    let scratch = scene::Scratch::new(output.parent().expect("validated parent"))?;
    let temp = scratch.0.join("frame.png");
    scene::write_png(
        &temp,
        receipt["width"].as_u64().expect("width") as u32,
        receipt["height"].as_u64().expect("height") as u32,
        &pixels,
    )?;
    receipt["output"] = json!(output);
    receipt["sha256"] = json!(media::file_hash(&temp)?);
    if let Some(path) = receipt["source"]["path"].as_str()
        && media::file_hash(Path::new(path))?
            != receipt["source"]["sha256"]
                .as_str()
                .expect("source identity")
    {
        return Err(error("MEDIA_CHANGED", "Source changed during preview"));
    }
    if let Some(sources) = receipt["sources"].as_array() {
        for source in sources {
            if media::file_hash(Path::new(source["path"].as_str().expect("source path")))?
                != source["sha256"].as_str().expect("source hash")
            {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Source changed during transition preview publication",
                ));
            }
        }
    }
    media::publish(&temp, &output)?;
    Ok(receipt)
}
pub fn range(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Time,
    duration: Time,
) -> Result<Value> {
    media::input_root(input_root)?;
    let preview_scale = project.preview_scale;
    let mapped = crate::proxy::preview_project(project, input_root)?;
    let project = &mapped;
    let total = validate(project)?;
    let begin = start.units(project.frame_rate).at(|| "start".into())?;
    let count = duration
        .units(project.frame_rate)
        .at(|| "duration".into())?;
    if count == 0 || begin as u128 + count as u128 > total as u128 {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "Preview range {start} s to {} s must be nonempty and inside the timeline, 0 s to {} s",
                start.plus(duration)?,
                project.duration()?
            ),
        ));
    }
    let mut receipt = render::run_range(project, input_root, output_root, output, start, duration)?;
    receipt["range_start"] = json!(start);
    receipt["range_duration"] = json!(duration);
    receipt["source_quality"] = json!(if preview_scale.is_some() {
        "proxy"
    } else {
        "original"
    });
    receipt["preview_scale"] = json!(preview_scale);
    receipt["width"] = json!(project.width);
    receipt["height"] = json!(project.height);
    Ok(receipt)
}

/// Contact-sheet layout: frames are fitted and centered in tiles, filled row by row; the sheet is at most 8M pixels.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Sheet {
    /// 1 to 64 rational timeline times in cell order; frame boundaries before the end, duplicates allowed.
    pub times: Vec<Time>,
    /// Cells per row, 1 to 8.
    pub columns: u32,
    /// Cell width in pixels, 1 to 1920.
    pub tile_width: u32,
    /// Cell height in pixels, 1 to 1080.
    pub tile_height: u32,
    /// Spacing between cells in pixels, 0 to 32.
    pub gap: u32,
    /// `[r, g, b]` fill (0-255) for gaps, letterboxing and unused cells.
    pub background: [u8; 3],
}
impl Sheet {
    pub(crate) fn validate(&self, project: &Project) -> Result<(u32, u32)> {
        let total = validate(project)?;
        if self.times.is_empty()
            || self.times.len() > 64
            || !(1..=8).contains(&self.columns)
            || !(1..=1920).contains(&self.tile_width)
            || !(1..=1080).contains(&self.tile_height)
            || self.gap > 32
        {
            return Err(error(
                "INVALID_SHEET",
                "Contact sheets require 1..64 times, 1..8 columns, tiles up to 1920x1080 and a gap of at most 32 pixels",
            ));
        }
        for (i, t) in self.times.iter().enumerate() {
            if t.units(project.frame_rate)? >= total {
                return Err(error(
                    "INVALID_RANGE",
                    format!(
                        "times[{i}]: Contact-sheet time {t} s must be before the timeline end {} s{}",
                        project.duration()?,
                        last_frame(project, total)?
                    ),
                ));
            }
        }
        let rows = (self.times.len() as u32).div_ceil(self.columns);
        let width = self.columns * self.tile_width + (self.columns - 1) * self.gap;
        let height = rows * self.tile_height + (rows - 1) * self.gap;
        if width as u64 * height as u64 > 8_000_000 {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Contact sheet supports at most 8M pixels",
            ));
        }
        Ok((width, height))
    }
}
pub fn sheet(
    project: &Project,
    spec: &Sheet,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
) -> Result<Value> {
    let (width, height) = spec.validate(project)?;
    let output = render::destination_extension(output, output_root, "png")?;
    let mut pixels = vec![0; width as usize * height as usize * 3];
    for p in pixels.as_chunks_mut::<3>().0 {
        p.copy_from_slice(&spec.background);
    }
    let mut sources = std::collections::BTreeMap::<String, String>::new();
    let mut cells = Vec::new();
    for (index, time) in spec.times.iter().enumerate() {
        let (frame, receipt) = read_frame(project, input_root, *time)?;
        let sw = receipt["width"].as_u64().expect("frame width");
        let sh = receipt["height"].as_u64().expect("frame height");
        let (dw, dh) = if spec.tile_width as u64 * sh <= spec.tile_height as u64 * sw {
            (
                spec.tile_width as u64,
                (spec.tile_width as u64 * sh / sw).max(1),
            )
        } else {
            (
                (spec.tile_height as u64 * sw / sh).max(1),
                spec.tile_height as u64,
            )
        };
        let x = (index as u32 % spec.columns) * (spec.tile_width + spec.gap)
            + (spec.tile_width - dw as u32) / 2;
        let y = (index as u32 / spec.columns) * (spec.tile_height + spec.gap)
            + (spec.tile_height - dh as u32) / 2;
        for py in 0..dh {
            for px in 0..dw {
                let sx = ((2 * px + 1) * sw / (2 * dw)).min(sw - 1);
                let sy = ((2 * py + 1) * sh / (2 * dh)).min(sh - 1);
                let from = ((sy * sw + sx) * 3) as usize;
                let to = (((y as u64 + py) * width as u64 + x as u64 + px) * 3) as usize;
                pixels[to..to + 3].copy_from_slice(&frame[from..from + 3]);
            }
        }
        let mut observed = Vec::new();
        if receipt["source"].is_object() {
            observed.push(&receipt["source"]);
        }
        if let Some(items) = receipt["sources"].as_array() {
            observed.extend(items);
        }
        for s in observed {
            if let (Some(path), Some(hash)) = (s["path"].as_str(), s["sha256"].as_str())
                && sources
                    .insert(path.into(), hash.into())
                    .is_some_and(|old| old != hash)
            {
                return Err(error(
                    "MEDIA_CHANGED",
                    "Contact-sheet sources changed between cells",
                ));
            }
        }
        cells.push(json!({"index":index,"time":time,"timeline_frame":time.units(project.frame_rate)?,"x":x,"y":y,"width":dw,"height":dh,
            "source_width":sw,"source_height":sh}));
    }
    let scratch = scene::Scratch::new(output.parent().expect("output parent"))?;
    let temp = scratch.0.join("frame.png");
    scene::write_png(&temp, width, height, &pixels)?;
    for (path, hash) in &sources {
        if media::file_hash(Path::new(path))? != *hash {
            return Err(error(
                "MEDIA_CHANGED",
                "Contact-sheet source changed before publication",
            ));
        }
    }
    let hash = media::file_hash(&temp)?;
    media::publish(&temp, &output)?;
    Ok(
        json!({"output":output,"sha256":hash,"width":width,"height":height,"cells":cells,"source_quality":if project.preview_scale.is_some(){"proxy"}else{"original"},
        "preview_scale":project.preview_scale,"project_revision":project.revision}),
    )
}
