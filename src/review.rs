//! Seeing an edit and its footage: cut-review sheets of a timeline, frame sheets of any source
//! file, and shot boundaries, so an agent can check cuts and log footage from still images.
use crate::{Result, error, media, model::Project, preview, render, scene, time::Time};
use serde_json::{Value, json};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

/// Cuts on one review sheet: two cells each, four cells per row.
const MAX_CUTS: usize = 16;
/// Frames on one source sheet.
const MAX_CELLS: usize = 64;

/// One change of the visible picture at `time`.
struct Cut {
    time: Time,
    before: Value,
    after: Value,
    continuous: bool,
}

/// The visible clip at `time` as `{track_id, clip_id, asset_id, source_time}`, or null for black.
fn shown(project: &Project, time: Time) -> Result<Value> {
    let Some(a) = &project.tracks else {
        let mut offset = Time::ZERO;
        for clip in &project.clips {
            let end = offset.plus(clip.duration)?;
            if !time.compare(offset)?.is_lt() && time.compare(end)?.is_lt() {
                return Ok(if clip.gap {
                    Value::Null
                } else {
                    json!({"clip_id":clip.id,"asset_id":clip.asset_id,"source_time":clip.source_in.plus(time.minus(offset)?)?})
                });
            }
            offset = end;
        }
        return Ok(Value::Null);
    };
    Ok(match a.visible(time)? {
        Some((track, clip)) => json!({"track_id":track.id,"clip_id":clip.id,
            "asset_id":if clip.asset_id.is_empty() { Value::Null } else { json!(clip.asset_id) },
            "sequence_id":clip.sequence_id,"source_time":clip.source_in.plus(time.minus(clip.start)?)?}),
        None => Value::Null,
    })
}

/// Every time after zero where the visible clip changes, in order.
fn cuts(project: &Project) -> Result<Vec<Cut>> {
    let frame = Time::new(project.frame_rate.den, project.frame_rate.num)?;
    let end = project.duration()?;
    let mut times = Vec::new();
    match &project.tracks {
        Some(a) => {
            for track in a.tracks.iter().filter(|t| {
                t.kind == crate::tracks::Kind::Video && t.enabled && t.composite.is_opaque()
            }) {
                for clip in &track.clips {
                    times.push(clip.start);
                    times.push(clip.end()?);
                }
            }
        }
        None => {
            let mut offset = Time::ZERO;
            for clip in &project.clips {
                offset = offset.plus(clip.duration)?;
                times.push(offset);
            }
        }
    }
    times.sort_by(|a, b| a.compare(*b).expect("valid times"));
    times.dedup_by(|a, b| a.compare(*b).is_ok_and(|o| o.is_eq()));
    let mut result = Vec::new();
    for time in times {
        if !time.compare(Time::ZERO)?.is_gt() || !time.compare(end)?.is_lt() {
            continue;
        }
        let (before, after) = (shown(project, time.minus(frame)?)?, shown(project, time)?);
        if before["clip_id"] == after["clip_id"] && before["track_id"] == after["track_id"] {
            continue;
        }
        // A split of one source with contiguous timing is an edit point but not a visible cut.
        let continuous = !before.is_null()
            && before["asset_id"] == after["asset_id"]
            && !after["asset_id"].is_null()
            && serde_json::from_value::<Time>(before["source_time"].clone())?
                .plus(frame)?
                .compare(serde_json::from_value::<Time>(
                    after["source_time"].clone(),
                )?)?
                .is_eq();
        result.push(Cut {
            time,
            before,
            after,
            continuous,
        });
    }
    Ok(result)
}

/// A sheet of the last frame before and the first frame after each cut from `start`, 16 cuts at a
/// time, with the table that maps cells to cuts and the `next` start for the following page.
pub fn cut_sheet(
    project: &Project,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    start: Option<Time>,
    limit: Option<usize>,
    tile_width: Option<u32>,
) -> Result<Value> {
    project.validate()?;
    let start = start.unwrap_or(Time::ZERO);
    let limit = limit.unwrap_or(MAX_CUTS);
    if !(1..=MAX_CUTS).contains(&limit) {
        return Err(error("INVALID_ARGUMENT", "limit must be 1-16 cuts"));
    }
    let all = cuts(project)?;
    let total = all.len();
    let mut chosen = Vec::new();
    for cut in all {
        if !cut.time.compare(start)?.is_lt() {
            chosen.push(cut);
        }
    }
    let next = chosen.get(limit).map(|c| c.time);
    chosen.truncate(limit);
    if chosen.is_empty() {
        return Ok(json!({"cuts":[],"total_cuts":total,"next":null,"output":null}));
    }
    let tile_width = tile_width.unwrap_or(192);
    if !(16..=480).contains(&tile_width) {
        return Err(error(
            "INVALID_ARGUMENT",
            "tile_width must be 16-480 pixels",
        ));
    }
    let frame = Time::new(project.frame_rate.den, project.frame_rate.num)?;
    let mut times = Vec::new();
    for cut in &chosen {
        times.push(cut.time.minus(frame)?);
        times.push(cut.time);
    }
    let spec = preview::Sheet {
        times,
        columns: 4,
        tile_width,
        tile_height: (tile_width as u64 * project.height as u64 / project.width as u64).max(1)
            as u32,
        gap: 6,
        background: [24, 24, 24],
    };
    let mut receipt = preview::sheet(project, &spec, input_root, output_root, output)?;
    receipt["cuts"] = chosen
        .iter()
        .enumerate()
        .map(|(i, c)| {
            json!({"time":c.time,"cells":[2*i,2*i+1],"before":c.before,"after":c.after,"continuous":c.continuous})
        })
        .collect();
    receipt["total_cuts"] = json!(total);
    receipt["next"] = json!(next);
    receipt["layout"] = json!("two cuts per row: before, after, before, after");
    Ok(receipt)
}

/// The probed video stream of `path`, its displayed size and its exact duration.
pub(crate) fn video(path: &Path) -> Result<(Value, u32, u32, Time)> {
    let metadata = media::probe(path)?;
    let stream = metadata["streams"]
        .as_array()
        .and_then(|s| s.iter().find(|s| s["codec_type"] == "video"))
        .cloned()
        .ok_or_else(|| error("UNSUPPORTED_MEDIA", "The file has no video stream"))?;
    let size = |key: &str| {
        stream[key]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .filter(|v| *v > 0)
    };
    let (Some(width), Some(height)) = (size("width"), size("height")) else {
        return Err(error("UNSUPPORTED_MEDIA", "The video stream has no size"));
    };
    let exact = || -> Option<Time> {
        let ticks = stream["duration_ts"].as_u64()?;
        let (num, den) = stream["time_base"].as_str()?.split_once('/')?;
        Time::new(ticks * num.parse::<u64>().ok()?, den.parse().ok()?).ok()
    };
    let duration = match exact() {
        Some(duration) => duration,
        None => serde_json::from_value::<Time>(
            stream["duration"]
                .as_str()
                .or(metadata["format"]["duration"].as_str())
                .map(|d| json!(d))
                .ok_or_else(|| error("UNSUPPORTED_MEDIA", "The file has no duration"))?,
        )?,
    };
    Ok((metadata, width, height, duration))
}

/// Decimal seconds for an FFmpeg seek.
fn seconds(time: Time) -> String {
    let micros = u128::from(time.num) * 1_000_000 / u128::from(time.den);
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// One frame of `path` at `time`, scaled to `width` x `height` RGB, or None past the end.
pub(crate) fn grab(path: &Path, time: Time, width: u32, height: u32) -> Result<Option<Vec<u8>>> {
    let arguments: Vec<String> = [
        "-v",
        "error",
        "-nostdin",
        "-ss",
        &seconds(time),
        "-i",
        &path.to_string_lossy(),
        "-map",
        "0:v:0",
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={width}:{height}:flags=area,format=rgb24"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "-",
    ]
    .map(str::to_owned)
    .to_vec();
    let bytes = media::capture(&media::tool("ffmpeg"), &arguments, Duration::from_secs(120))?;
    Ok((bytes.len() == (width * height * 3) as usize).then_some(bytes))
}

/// Write `cells` frames of `tile` x `tile_height` in rows of `columns` to a new PNG at `output`.
fn write_sheet(
    output: &Path,
    frames: &[Option<Vec<u8>>],
    columns: u32,
    tile: (u32, u32),
) -> Result<(u32, u32, Vec<Value>)> {
    const GAP: u32 = 6;
    let rows = (frames.len() as u32).div_ceil(columns);
    let width = columns * tile.0 + (columns - 1) * GAP;
    let height = rows * tile.1 + (rows - 1) * GAP;
    let mut pixels = [24u8, 24, 24].repeat((width * height) as usize);
    let mut cells = Vec::new();
    for (index, frame) in frames.iter().enumerate() {
        let x = (index as u32 % columns) * (tile.0 + GAP);
        let y = (index as u32 / columns) * (tile.1 + GAP);
        if let Some(frame) = frame {
            for row in 0..tile.1 {
                let from = (row * tile.0 * 3) as usize;
                let to = (((y + row) * width + x) * 3) as usize;
                pixels[to..to + (tile.0 * 3) as usize]
                    .copy_from_slice(&frame[from..from + (tile.0 * 3) as usize]);
            }
        }
        cells.push(json!({"index":index,"x":x,"y":y,"present":frame.is_some()}));
    }
    let scratch = scene::Scratch::new(output.parent().expect("output parent"))?;
    let temp = scratch.0.join("sheet.png");
    scene::write_png(&temp, width, height, &pixels)?;
    media::publish(&temp, output)?;
    Ok((width, height, cells))
}

/// Times of `count` frames spread evenly through `duration`, at the middle of equal parts.
pub(crate) fn spread(duration: Time, count: u64) -> Result<Vec<Time>> {
    (0..count)
        .map(|i| duration.times(Time::new(2 * i + 1, 2 * count)?))
        .collect()
}

/// A sheet of frames from any decodable source file: at `times`, or `count` evenly spread frames.
#[allow(clippy::too_many_arguments)]
pub fn source_sheet(
    path: &Path,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
    times: Option<Vec<Time>>,
    count: Option<u32>,
    columns: Option<u32>,
    tile_width: Option<u32>,
) -> Result<Value> {
    let path = media::allowed_file(path, input_root)?;
    let output = render::destination_extension(output, output_root, "png")?;
    let (_, width, height, duration) = video(&path)?;
    let times = match (times, count) {
        (Some(_), Some(_)) => {
            return Err(error("INVALID_ARGUMENT", "Give times or count, not both"));
        }
        (Some(times), None) => times,
        (None, count) => spread(duration, u64::from(count.unwrap_or(16)))?,
    };
    let columns = columns.unwrap_or(4);
    let tile_width = tile_width.unwrap_or(192);
    if times.is_empty() || times.len() > MAX_CELLS || !(1..=8).contains(&columns) {
        return Err(error(
            "INVALID_ARGUMENT",
            "A source sheet holds 1-64 frames in 1-8 columns",
        ));
    }
    if !(16..=480).contains(&tile_width) {
        return Err(error(
            "INVALID_ARGUMENT",
            "tile_width must be 16-480 pixels",
        ));
    }
    let tile = (
        tile_width,
        ((tile_width as u64 * height as u64 / width as u64).max(1)) as u32,
    );
    let mut frames = Vec::new();
    for time in &times {
        frames.push(grab(&path, *time, tile.0, tile.1)?);
    }
    let (sheet_width, sheet_height, mut cells) = write_sheet(&output, &frames, columns, tile)?;
    for (cell, time) in cells.iter_mut().zip(&times) {
        cell["time"] = json!(time);
    }
    Ok(
        json!({"output":output,"width":sheet_width,"height":sheet_height,"tile":[tile.0,tile.1],
        "source":crate::identity::relative(&path, input_root)?,"source_size":[width,height],"duration":duration,"cells":cells}),
    )
}

/// Presentation times of every video frame, from packet timestamps without decoding.
pub(crate) fn frame_times(path: &Path) -> Result<Vec<Time>> {
    let arguments: Vec<String> = [
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time",
        "-of",
        "csv=p=0",
        &path.to_string_lossy(),
    ]
    .map(str::to_owned)
    .to_vec();
    let text = media::capture(
        &media::tool("ffprobe"),
        &arguments,
        Duration::from_secs(600),
    )?;
    let mut times = Vec::new();
    for line in String::from_utf8_lossy(&text).lines() {
        let line = line.trim().trim_end_matches(',');
        if !line.is_empty() && line != "N/A" {
            times.push(serde_json::from_value::<Time>(json!(line))?);
        }
    }
    times.sort_by(|a, b| a.compare(*b).expect("valid times"));
    Ok(times)
}

/// Shot boundaries of a source: frames are shrunk to 64x36 gray and compared with the previous
/// one by mean absolute difference (0-255). A cut is a frame whose difference reaches `threshold`
/// and is at least twice the difference of each of its two neighbors on either side, so motion
/// and single-frame flashes are not cuts. Gradual transitions are not detected.
#[allow(clippy::too_many_arguments)]
pub fn shots(
    path: &Path,
    input_root: &Path,
    threshold: Option<u8>,
    minimum_frames: Option<u32>,
    output_root: Option<&Path>,
    output: Option<&Path>,
    tile_width: Option<u32>,
) -> Result<Value> {
    const W: usize = 64;
    const H: usize = 36;
    let path = media::allowed_file(path, input_root)?;
    let (_, width, height, duration) = video(&path)?;
    let threshold = threshold.unwrap_or(20).max(1);
    let minimum = minimum_frames.unwrap_or(6).max(1) as usize;
    let mut child = Command::new(media::tool("ffmpeg"))
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-vf",
            &format!("scale={W}:{H}:flags=area,format=gray"),
            "-f",
            "rawvideo",
            "-pix_fmt",
            "gray",
            "-",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| error("TOOL_FAILED", format!("Could not start ffmpeg: {e}")))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut previous = vec![0u8; W * H];
    let mut current = vec![0u8; W * H];
    let mut differences = Vec::new();
    let mut count = 0usize;
    loop {
        match stdout.read_exact(&mut current) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
        if count > 0 {
            let sum: u64 = current
                .iter()
                .zip(&previous)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum();
            differences.push(sum as f64 / (W * H) as f64);
        } else {
            differences.push(0.0);
        }
        std::mem::swap(&mut previous, &mut current);
        count += 1;
    }
    let status = child.wait()?;
    if !status.success() || count == 0 {
        return Err(error("TOOL_FAILED", "Could not decode the video stream"));
    }
    let mut times = frame_times(&path)?;
    if times.len() != count {
        // Fall back to the nominal rate when packets do not map one-to-one onto decoded frames.
        let step = duration.times(Time::new(1, count as u64)?)?;
        times = (0..count as u64)
            .map(|n| step.times(Time::new(n, 1)?))
            .collect::<Result<_>>()?;
    }
    let origin = times[0];
    let neighbor = |n: usize| differences.get(n).copied().unwrap_or(0.0);
    let mut starts = vec![0usize];
    for (n, &d) in differences.iter().enumerate().skip(1) {
        let around = [n.wrapping_sub(2), n - 1, n + 1, n + 2]
            .into_iter()
            .filter(|&m| m >= 1 && m < count)
            .map(neighbor)
            .fold(0.0, f64::max);
        if d >= f64::from(threshold)
            && d >= 2.0 * around
            && n - starts.last().copied().unwrap_or(0) >= minimum
        {
            starts.push(n);
        }
    }
    let mut shots = Vec::new();
    for (i, &first) in starts.iter().enumerate() {
        let last = starts.get(i + 1).copied().unwrap_or(count);
        let start = times[first].minus(origin)?;
        let end = match times.get(last) {
            Some(t) => t.minus(origin)?,
            None => duration,
        };
        shots.push(json!({"index":i,"start":start,"end":end,"frames":last-first,
            "cut_score":if first == 0 { Value::Null } else { json!((differences[first]*10.0).round()/10.0) }}));
    }
    let mut result = json!({"source":crate::identity::relative(&path, input_root)?,"frames":count,"duration":duration,
        "threshold":threshold,"minimum_frames":minimum,"shots":shots,"metric":"mean_absolute_gray_difference_64x36"});
    if let Some(output) = output {
        let output_root =
            output_root.ok_or_else(|| error("INVALID_ARGUMENT", "output needs output_root"))?;
        let middles: Vec<Time> = result["shots"]
            .as_array()
            .expect("shots")
            .iter()
            .take(MAX_CELLS)
            .map(|s| {
                let start = serde_json::from_value::<Time>(s["start"].clone())?;
                let end = serde_json::from_value::<Time>(s["end"].clone())?;
                start.plus(end.minus(start)?.times(Time::new(1, 2)?)?)
            })
            .collect::<Result<_>>()?;
        let sheet = source_sheet(
            &path,
            input_root,
            output_root,
            output,
            Some(middles),
            None,
            None,
            tile_width,
        )?;
        result["sheet"] = json!({"output":sheet["output"],"width":sheet["width"],"height":sheet["height"],
            "cells":"one per shot, at its middle, in shot order","shots_shown":sheet["cells"].as_array().map(Vec::len)});
        result["output"] = sheet["output"].clone();
    }
    let _ = (width, height);
    Ok(result)
}
