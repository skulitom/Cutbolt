//! Compact results for agents, who read every byte a tool returns. A summary keeps what an agent
//! acts on: `ok`, errors, frames, work, outputs and their identities. The bulk an agent rarely
//! reads becomes one-line summaries or counts: per-frame scene timing runs, echoed inputs, the
//! identities of files it named itself. MCP returns summaries unless a call asks for
//! `detail: "full"`; the CLI returns full results unless asked for a summary, and job stores
//! always keep the full receipt.
use crate::{Result, error};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// How much of a result a call returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    Summary,
    Full,
}

impl Detail {
    /// Remove a request's `detail` argument, which every command accepts.
    pub(crate) fn take(request: &mut Value) -> Result<Option<Self>> {
        match request.as_object_mut().and_then(|o| o.remove("detail")) {
            None => Ok(None),
            Some(Value::String(text)) if text == "summary" => Ok(Some(Self::Summary)),
            Some(Value::String(text)) if text == "full" => Ok(Some(Self::Full)),
            Some(other) => Err(error(
                "INVALID_JSON",
                format!("detail: expected \"summary\" or \"full\", got {other}"),
            )),
        }
    }
}

/// Condense a command's result in place. A result that lost anything says so in `detail`, with
/// the fields `detail: "full"` would return in full.
pub(crate) fn summarize(command: &str, result: &mut Value) {
    let mut shortened = Vec::new();
    condense(command, result, "", &mut shortened);
    if !shortened.is_empty() {
        result["detail"] = json!(format!(
            "summary; detail \"full\" returns {}",
            shortened.join(", ")
        ));
    }
}

fn condense(command: &str, result: &mut Value, at: &str, shortened: &mut Vec<String>) {
    let Some(object) = result.as_object_mut() else {
        return;
    };
    count(object, "resolved_identities", at, shortened);
    match command {
        "scene.inspect" | "scene.render" => scene(object, at, shortened),
        "graphics.instantiate" | "captions.scene" => {
            if let Some(Value::Object(inspection)) = object.get_mut("inspection") {
                scene(inspection, &format!("{at}inspection."), shortened);
            }
        }
        "export.run" => count(object, "sources", at, shortened),
        // Each cut lists its cells and times; the cell rectangles only place them on the sheet.
        "preview.cuts" => {
            if object.remove("cells").is_some() {
                note(shortened, at, "cells");
            }
        }
        "audio.beats" => {
            if let Some(Value::Object(onsets)) = object.get_mut("onsets") {
                count(onsets, "times", &format!("{at}onsets."), shortened);
            }
        }
        // A job's result is its command's receipt, summarized like that command's own result.
        "job.wait" | "job.status" => {
            let run = object.get("ticket").and_then(|t| t.get("command"));
            if let Some(run) = run.and_then(Value::as_str).map(str::to_owned)
                && let Some(receipt) = object.get_mut("result")
            {
                condense(&run, receipt, &format!("{at}result."), shortened);
            }
            if let Some(Value::Array(jobs)) = object.get_mut("jobs") {
                for job in jobs {
                    let run = job["command"].as_str().unwrap_or_default().to_owned();
                    if let Some(receipt) = job.get_mut("result") {
                        condense(&run, receipt, &format!("{at}jobs[].result."), shortened);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Record a shortened field once, by its path.
fn note(shortened: &mut Vec<String>, at: &str, field: &str) {
    let path = format!("{at}{field}");
    if !shortened.contains(&path) {
        shortened.push(path);
    }
}

/// Replace an array with its length.
fn count(object: &mut Map<String, Value>, field: &str, at: &str, shortened: &mut Vec<String>) {
    if let Some(Value::Array(items)) = object.get(field) {
        let length = items.len();
        object.insert(field.to_owned(), json!(length));
        note(shortened, at, field);
    }
}

/// A scene inspection or render receipt: one line per layer instead of its timing runs, counts
/// instead of source lists and per-sample arrays, and no echoed specifications or tool versions.
fn scene(report: &mut Map<String, Value>, at: &str, shortened: &mut Vec<String>) {
    let frames = report.get("frames").and_then(Value::as_u64);
    if let Some(Value::Array(timing)) = report.remove("timing") {
        let layers = timing.iter().map(|l| json!(layer(l, frames))).collect();
        report.insert("layers".into(), Value::Array(layers));
        note(shortened, at, "timing");
    }
    count(report, "sources", at, shortened);
    // The matte profile and sampling are fixed text; the count is what varies.
    if let Some(Value::Object(matte)) = report.get_mut("frame_matte") {
        let fields = matte.len();
        matte.retain(|field, _| field == "matted_pairs");
        if matte.len() < fields {
            note(shortened, at, "frame_matte");
        }
    }
    for tool in ["ffmpeg", "ffprobe"] {
        if report.remove(tool).is_some() {
            note(shortened, at, tool);
        }
    }
    for (part, echoed, arrays) in [
        ("expressions", "program", &["frame_bindings"][..]),
        (
            "temporal",
            "specification",
            &["sample_times", "active_sample_times"][..],
        ),
        (
            "geometry",
            "specification",
            &["sample_states", "node_world_matrices"][..],
        ),
    ] {
        if let Some(Value::Object(sub)) = report.get_mut(part) {
            sub.remove(echoed);
            for field in arrays {
                if let Some(Value::Array(items)) = sub.get(*field) {
                    let length = items.len();
                    sub.insert((*field).to_owned(), json!(length));
                }
            }
            note(shortened, at, part);
        }
    }
}

/// One layer's timing in a line: when it shows, which source frames it selects, how each sampled
/// parameter changes, and its blend, mask, graphics and tilemap.
fn layer(timing: &Value, frames: Option<u64>) -> String {
    let id = timing["layer_id"].as_str().unwrap_or("?");
    let mut parts = Vec::new();
    // Runs of selected source frames, null while the layer is hidden.
    let (mut position, mut first, mut last, mut shown) = (0u64, None, 0u64, 0u64);
    let mut selected: Vec<&Value> = Vec::new();
    for run in timing["selected_frames"].as_array().into_iter().flatten() {
        let count = run["count"].as_u64().unwrap_or(0);
        if count > 0 && !run["value"].is_null() {
            first.get_or_insert(position);
            last = position + count - 1;
            shown += count;
            if selected.last() != Some(&&run["value"]) {
                selected.push(&run["value"]);
            }
        }
        position += count;
    }
    let unit = if frames.is_some_and(|f| f != position) {
        "samples"
    } else {
        "frames"
    };
    match first {
        None => parts.push("never shown".to_owned()),
        Some(first) => {
            let mut on = format!("{unit} {first}-{last}");
            if shown != last - first + 1 {
                on.push_str(&format!(" ({shown} shown)"));
            }
            parts.push(on);
            let numbers: Vec<u64> = selected.iter().filter_map(|v| v.as_u64()).collect();
            if let (Some(low), Some(high)) = (numbers.iter().min(), numbers.iter().max()) {
                parts.push(match selected.len() {
                    1 => format!("source frame {low}"),
                    runs => format!("source frames {low}-{high} in {runs} runs"),
                });
            }
        }
    }
    // Distinct consecutive values of each sampled parameter, in order of first appearance.
    let mut order: Vec<&str> = Vec::new();
    let mut values: BTreeMap<&str, Vec<&Value>> = BTreeMap::new();
    for run in timing["sampled_parameters"]
        .as_array()
        .into_iter()
        .flatten()
    {
        for (key, value) in run["value"].as_object().into_iter().flatten() {
            let seen = values.entry(key).or_insert_with(|| {
                order.push(key);
                Vec::new()
            });
            if seen.last() != Some(&value) {
                seen.push(value);
            }
        }
    }
    for key in order {
        parts.push(parameter(key, &values[key]));
    }
    if let Some(mode) = timing["blend_mode"].as_str().filter(|m| *m != "normal") {
        parts.push(format!("blend {mode}"));
    }
    if timing["mask_inverted"] == true {
        parts.push("inverted mask".to_owned());
    }
    if let Some(graphics) = timing.get("graphics") {
        parts.push(graphic(graphics));
    }
    if let Some([columns, rows]) = timing["tilemap"]["grid"].as_array().map(Vec::as_slice) {
        parts.push(format!("tilemap {columns}x{rows}"));
    }
    format!("{id}: {}", parts.join(", "))
}

/// A sampled parameter: its value when it holds, else its first and last values when they are
/// short, else how many values it takes.
fn parameter(key: &str, values: &[&Value]) -> String {
    let short = |v: &Value| {
        let text = v.to_string();
        (text.len() <= 24 && !v.is_object()).then_some(text)
    };
    match (
        values,
        values.first().and_then(|v| short(v)),
        values.last().and_then(|v| short(v)),
    ) {
        ([_], Some(value), _) => format!("{key} {value}"),
        ([_], None, _) => format!("{key} fixed"),
        (_, Some(first), Some(last)) => format!("{key} {first}→{last} ({} values)", values.len()),
        _ => format!("{key} animated ({} values)", values.len()),
    }
}

/// A graphics layer's layout: its kind, and for text its lines, glyphs and any clipping.
fn graphic(report: &Value) -> String {
    let mut text = report["kind"].as_str().unwrap_or("graphics").to_owned();
    if let Some(shape) = report["shape"].as_str() {
        text.push_str(&format!(" {shape}"));
    }
    if let Some(lines) = report["line_widths"].as_array() {
        let glyphs = report["glyphs"].as_array().map_or(0, Vec::len);
        text.push_str(&format!(
            " {} line{}, {glyphs} glyphs",
            lines.len(),
            if lines.len() == 1 { "" } else { "s" }
        ));
    }
    if let Some(clipped) = report["clipped_coverage_pixels"]
        .as_u64()
        .filter(|c| *c > 0)
    {
        text.push_str(&format!(", {clipped} clipped pixels"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn runs(values: &[(u64, Value)]) -> Value {
        Value::Array(
            values
                .iter()
                .map(|(count, value)| json!({"count":count,"value":value}))
                .collect(),
        )
    }

    /// A scene.render receipt shaped like the engine's: two layers, sources and tool versions.
    fn receipt() -> Value {
        let mut fading = Vec::new();
        for (n, opacity) in [0, 64, 128, 255].iter().enumerate() {
            fading.push((1, json!({"opacity":opacity,"position":[10, 20 - n as i64]})));
        }
        fading.push((6, json!({"opacity":255,"position":[10,17]})));
        json!({"profile":"pixel-scene-v1","scene_id":"s","frames":12,"frame_rate":{"num":25,"den":1},
        "width":64,"height":36,"output":"renders/s.mkv","sha256":"a".repeat(64),
        "asset":{"id":"s","path":"renders/s.mkv","identity":{"sha256":"a".repeat(64),"bytes":9}},
        "work":{"composited_pixels":100},"ffmpeg":"ffmpeg version 7","ffprobe":"ffprobe version 7",
        "frame_matte":{"profile":"binary-source-matte-v1","matted_pairs":0,"sampling":"held"},
        "sources":[{"path":"a.png"},{"path":"b.png"}],
        "timing":[
            {"layer_id":"sky","blend_mode":"normal","mask_inverted":null,
             "selected_frames":runs(&[(12, json!(0))]),
             "sampled_parameters":runs(&[(12, json!({"opacity":255,"position":[0,0]}))]),
             "tilemap":{"grid":[4,1],"tile_selected_frames":[runs(&[(12, json!(0))])]}},
            {"layer_id":"title","blend_mode":"screen","mask_inverted":true,
             "selected_frames":runs(&[(2, Value::Null), (10, json!(0))]),
             "sampled_parameters":Value::Array(
                 [vec![(2, Value::Null)], fading].concat().iter()
                     .map(|(count, value)| json!({"count":count,"value":value})).collect()),
             "graphics":{"kind":"text","line_widths":[40.5],"glyphs":[{},{},{}],"clipped_coverage_pixels":5}},
            {"layer_id":"walk","blend_mode":"normal","mask_inverted":null,
             "selected_frames":runs(&[(3, json!(0)), (3, json!(1)), (3, Value::Null), (3, json!(2))]),
             "sampled_parameters":runs(&[(12, json!({"opacity":255,"position":[0,0],"spatial":{"matrix":[1,0,0,1]}}))])}
        ]})
    }

    #[test]
    fn scene_receipts_keep_one_line_per_layer() {
        let mut result = receipt();
        summarize("scene.render", &mut result);
        assert_eq!(
            result["layers"],
            json!([
                "sky: frames 0-11, source frame 0, opacity 255, position [0,0], tilemap 4x1",
                "title: frames 2-11, source frame 0, opacity 0→255 (4 values), position [10,20]→[10,17] (4 values), blend screen, inverted mask, text 1 line, 3 glyphs, 5 clipped pixels",
                "walk: frames 0-11 (9 shown), source frames 0-2 in 3 runs, opacity 255, position [0,0], spatial fixed"
            ])
        );
        assert!(result.get("timing").is_none() && result.get("ffmpeg").is_none());
        assert_eq!(result["sources"], 2);
        assert_eq!(result["frame_matte"], json!({"matted_pairs":0}));
        // What an agent acts on stays as it was.
        let original = receipt();
        for field in ["frames", "output", "asset", "work", "sha256", "scene_id"] {
            assert_eq!(result[field], original[field], "{field}");
        }
        assert_eq!(
            result["detail"],
            "summary; detail \"full\" returns timing, sources, frame_matte, ffmpeg, ffprobe"
        );
    }

    #[test]
    fn job_results_are_summarized_by_their_command() {
        let mut single = json!({"ticket":{"job_id":"j","command":"scene.render"},"status":"completed",
            "result":receipt(),"finished":true});
        summarize("job.wait", &mut single);
        assert!(single["result"]["layers"].is_array());
        assert!(single["detail"].as_str().unwrap().contains("result.timing"));
        let mut batch = json!({"finished":true,"jobs":[
            {"job_id":"a","command":"scene.render","status":"completed","result":receipt()},
            {"job_id":"b","command":"scene.render","status":"completed","result":receipt()},
            {"job_id":"c","command":"export.run","status":"failed","error":{"code":"X","message":"m"}}]});
        summarize("job.wait", &mut batch);
        assert_eq!(batch["jobs"][1]["result"]["sources"], 2);
        assert_eq!(batch["jobs"][2]["error"]["code"], "X");
        // Each shortened field is named once, by its path in any job.
        let detail = batch["detail"].as_str().unwrap();
        assert_eq!(
            detail.matches("jobs[].result.timing").count(),
            1,
            "{detail}"
        );
        // A render's job result without a ticket command, or a failed job, is left alone.
        let mut failed =
            json!({"ticket":{"job_id":"j"},"status":"failed","result":null,"error":{"code":"X"}});
        summarize("job.status", &mut failed);
        assert!(failed.get("detail").is_none());
    }

    #[test]
    fn identities_and_lists_become_counts() {
        let mut still =
            json!({"output":"still.png","resolved_identities":[{"path":"a"},{"path":"b"}]});
        summarize("scene.still", &mut still);
        assert_eq!(still["resolved_identities"], 2);
        let mut beats = json!({"tempo_bpm":120,"onsets":{"times":[1,2,3]},"beats":{"times":[1,2]}});
        summarize("audio.beats", &mut beats);
        assert_eq!(beats["onsets"]["times"], 3);
        assert_eq!(beats["beats"]["times"], json!([1, 2]));
        let mut cuts = json!({"cuts":[{"cells":[0,1]}],"cells":[{},{}]});
        summarize("preview.cuts", &mut cuts);
        assert!(cuts.get("cells").is_none() && cuts["cuts"][0]["cells"] == json!([0, 1]));
        // Nothing to shorten: no detail note.
        let mut small = json!({"valid":true});
        summarize("project.validate", &mut small);
        assert_eq!(small, json!({"valid":true}));
    }

    #[test]
    fn detail_is_summary_or_full() {
        let mut request = json!({"command":"x","detail":"full"});
        assert_eq!(Detail::take(&mut request).unwrap(), Some(Detail::Full));
        assert!(request.get("detail").is_none());
        assert_eq!(Detail::take(&mut json!({})).unwrap(), None);
        let wrong = Detail::take(&mut json!({"detail":"brief"})).unwrap_err();
        assert!(wrong.message.starts_with("detail:"), "{}", wrong.message);
    }
}
