//! Original subject-directed crop decisions with exact bounded source-pixel paths.
use crate::{
    Result,
    animation::{Curve, Interpolation, Keyframe},
    error, scene, spatial,
    time::Time,
    tracking,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

const FPS: Time = Time { num: 25, den: 1 };

/// Keyframe of an authored subject box.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BoxKey {
    /// Segment-local time, rational seconds; keys must lie within the segment.
    pub time: Time,
    /// Subject box `[x, y, width, height]` in source-canvas pixels; positive size, inside the canvas.
    pub rect: [i32; 4],
    /// Interpolation from this key to the next.
    pub interpolation: Interpolation,
}

/// How a segment's subject boxes are obtained, tagged by `mode`.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Selection {
    /// Authored boxes interpolated over the segment.
    Manual {
        /// 1..128 keys, including one at segment time zero.
        keys: Vec<BoxKey>,
    },
    /// Translate a fixed focus box by a tracked patch's measured integer displacement; needs at least two frames.
    Track {
        /// Subject box `[x, y, width, height]` at the segment start, in source-canvas pixels; with padding it must fit the canvas.
        focus: [i32; 4],
        /// Reference patch `[x, y, width, height]` inside `focus`, sides 4..64 pixels.
        region: [u32; 4],
        /// Patch-tracking controls.
        controls: crate::stabilize::Tracking,
    },
}

/// Subject segment covering frames from `start` to the next segment start or scene end.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// Scene time, frame-aligned rational seconds at 25 fps, before the scene end; unique, one must be zero.
    pub start: Time,
    /// Subject label, 1..64 ASCII letters, digits, `_` or `-`.
    pub subject_id: String,
    /// Reset crop smoothing and movement limits at `start`; the segment at time zero must set true.
    pub cut: bool,
    /// How the subject boxes are obtained.
    pub selection: Selection,
}

/// `reframe.inspect` request: plans a constant-size crop that follows subjects and returns a replacement scene; read-only.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    /// Scene with exactly one full-duration, untransformed image layer starting at zero; 1..128 frames at 25 fps.
    pub scene: scene::Scene,
    /// Absolute directory containing the scene's bound source files.
    pub input_root: PathBuf,
    /// Crop window `[width, height]` in source pixels; fits the canvas and has exactly the `output_size` aspect ratio.
    pub window: [u32; 2],
    /// Logical output `[width, height]` in pixels, each 1..512.
    pub output_size: [u32; 2],
    /// Margin added on every side of each subject box in source pixels, 0..4096.
    pub padding: u32,
    /// Maximum crop-origin movement per frame `[x, y]` in source pixels, each 1..4096.
    pub maximum_step: [u32; 2],
    /// Triangular smoothing half-window in frames, 0..16; 0 disables smoothing.
    pub smoothing_radius: u8,
    /// Sampler written to the returned spatial fit.
    pub sampling: spatial::Sampling,
    /// 1..16 subject segments; input order does not matter.
    pub segments: Vec<Segment>,
}

#[derive(Clone, Serialize)]
struct Confidence {
    correlation: f64,
    margin: f64,
    frame_change: f64,
}

#[derive(Serialize)]
struct Decision {
    time: Time,
    segment: usize,
    segment_time: Time,
    subject_id: String,
    cut: bool,
    source_frame: usize,
    selection: &'static str,
    focus: [i32; 4],
    padded_focus: [i32; 4],
    desired_origin_half_pixels: [i64; 2],
    smoothed_origin: [i32; 2],
    feasible_origin_intervals: [[i32; 2]; 2],
    crop: [i32; 4],
    step: [i32; 2],
    constraint_adjustment: [i32; 2],
    confidence: Option<Confidence>,
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_REFRAME", message)
}

fn rect(rect: [i32; 4], canvas: [u32; 2], padding: u32) -> Result<[i32; 4]> {
    let [x, y, w, h] = rect.map(i64::from);
    let p = i64::from(padding);
    if w <= 0
        || h <= 0
        || x < p
        || y < p
        || x + w + p > i64::from(canvas[0])
        || y + h + p > i64::from(canvas[1])
    {
        return Err(invalid(
            "Every focus box, including padding, must lie inside the source canvas",
        ));
    }
    Ok([
        (x - p) as i32,
        (y - p) as i32,
        (w + 2 * p) as i32,
        (h + 2 * p) as i32,
    ])
}

fn rounded(n: i64, d: i64) -> i32 {
    ((n.abs() + d / 2) / d) as i32 * if n < 0 { -1 } else { 1 }
}

fn smooth(targets: &[i64], radius: usize) -> Vec<i32> {
    (0..targets.len())
        .map(|n| {
            let mut total = 0;
            let mut weights = 0;
            for (j, target) in targets
                .iter()
                .enumerate()
                .take((n + radius + 1).min(targets.len()))
                .skip(n.saturating_sub(radius))
            {
                let weight = (radius + 1 - j.abs_diff(n)) as i64;
                total += target * weight;
                weights += weight;
            }
            rounded(total, 2 * weights)
        })
        .collect()
}

/// Backwards feasibility followed by closest current target with a viable suffix.
/// Rectangular constraints make each axis independent. This is not global energy minimization.
fn path(intervals: &[[i32; 2]], targets: &[i32], maximum: i32) -> Result<Vec<i32>> {
    let mut reachable = intervals.to_vec();
    for n in (0..reachable.len() - 1).rev() {
        reachable[n][0] = reachable[n][0].max(reachable[n + 1][0] - maximum);
        reachable[n][1] = reachable[n][1].min(reachable[n + 1][1] + maximum);
    }
    if reachable.iter().any(|r| r[0] > r[1]) {
        return Err(error(
            "REFRAME_INFEASIBLE",
            "No crop path retains every selected box under the declared pan limit; revise boxes, window, limit or explicit cuts",
        ));
    }
    let mut result: Vec<i32> = Vec::new();
    for (&[mut a, mut b], target) in reachable.iter().zip(targets) {
        if let Some(previous) = result.last() {
            a = a.max(previous - maximum);
            b = b.min(previous + maximum);
        }
        if a > b {
            return Err(error(
                "REFRAME_INFEASIBLE",
                "Crop path lost its validated feasible interval",
            ));
        }
        result.push((*target).clamp(a, b));
    }
    Ok(result)
}

fn manual(keys: &[BoxKey], duration: Time, canvas: [u32; 2], count: u64) -> Result<Vec<[i32; 4]>> {
    if keys.is_empty() || keys.len() > 128 || !keys.iter().any(|k| k.time.num == 0) {
        return Err(invalid(
            "Manual selections need 1..128 focus keys including time zero",
        ));
    }
    for key in keys {
        rect(key.rect, canvas, 0)?;
    }
    let curves: Vec<_> = (0..4)
        .map(|axis| {
            Curve {
                keys: keys
                    .iter()
                    .map(|key| Keyframe {
                        time: key.time,
                        value: key.rect[axis],
                        interpolation: key.interpolation,
                    })
                    .collect(),
                retime: None,
            }
            .prepare(
                duration,
                if axis < 2 { 0 } else { 1 },
                canvas[axis % 2] as i32,
            )
        })
        .collect::<Result<_>>()?;
    (0..count)
        .map(|n| {
            let t = Time::new(n, 25)?;
            Ok([
                curves[0].sample(t)?,
                curves[1].sample(t)?,
                curves[2].sample(t)?,
                curves[3].sample(t)?,
            ])
        })
        .collect()
}

pub fn inspect(request: &Inspect) -> Result<Value> {
    // Validate all declared dependencies, even frames not selected by the scene clock.
    let original_report = scene::inspect(&request.scene, &request.input_root)?;
    let count = request.scene.duration.units(FPS)?;
    if request.scene.layers.len() != 1
        || !(1..=128).contains(&count)
        || request.segments.is_empty()
        || request.segments.len() > 16
        || request.output_size.iter().any(|v| !(1..=512).contains(v))
        || request.maximum_step.iter().any(|v| !(1..=4096).contains(v))
        || request.padding > 4096
        || request.smoothing_radius > 16
    {
        return Err(invalid(
            "Need one image layer, 1..128 frames, 1..16 segments, output dimensions 1..512, pan steps 1..4096, padding <=4096 and smoothing radius <=16",
        ));
    }
    let layer = &request.scene.layers[0];
    if layer.graphics.is_some()
        || layer.tilemap.is_some()
        || layer.start.units(FPS)? != 0
        || layer.duration.compare(request.scene.duration)?.is_ne()
        || layer.transform.position != [0, 0]
        || layer.transform.scale != 1
        || layer.transform.quarter_turns != 0
        || layer.transform.spatial.is_some()
        || layer.transform.crop != [0, 0, layer.canvas[0], layer.canvas[1]]
        || layer.frames.iter().any(|f| f.anchor != [0, 0])
        || layer
            .animation
            .as_ref()
            .is_some_and(|a| a.position_x.is_some() || a.position_y.is_some())
    {
        return Err(invalid(
            "Reframing requires a full-duration untransformed source image layer with zero anchors; source masks, effects and opacity may remain",
        ));
    }
    if request
        .window
        .iter()
        .zip(layer.canvas)
        .any(|(&w, s)| w == 0 || w > s)
        || u64::from(request.window[0]) * u64::from(request.output_size[1])
            != u64::from(request.window[1]) * u64::from(request.output_size[0])
    {
        return Err(invalid(
            "The declared integer crop window must fit the source and have exactly the output aspect ratio",
        ));
    }
    let mut segments = request
        .segments
        .iter()
        .map(|s| Ok((s.start.units(FPS)?, s)))
        .collect::<Result<Vec<_>>>()?;
    segments.sort_by_key(|s| s.0);
    if segments[0].0 != 0
        || !segments[0].1.cut
        || segments.iter().any(|(n, s)| {
            *n >= count
                || s.subject_id.is_empty()
                || s.subject_id.len() > 64
                || !s
                    .subject_id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
        || segments.windows(2).any(|p| p[0].0 == p[1].0)
    {
        return Err(invalid(
            "Segments need unique aligned starts before the end, first start zero with cut=true, and bounded ASCII subject IDs",
        ));
    }
    let mut boundaries: Vec<_> = segments.iter().map(|s| s.0).collect();
    boundaries.push(count);
    let mut work = 0u64;
    for ((_, segment), span) in segments.iter().zip(boundaries.windows(2)) {
        if let Selection::Track {
            region,
            focus,
            controls,
        } = &segment.selection
        {
            let [x, y, w, h] = *region;
            if !(4..=64).contains(&w)
                || !(4..=64).contains(&h)
                || !(1..=32).contains(&controls.search_radius)
                || span[1] - span[0] < 2
            {
                return Err(invalid(
                    "Tracked segments need at least two frames, a 4..64-pixel patch and radius 1..32",
                ));
            }
            rect(*focus, layer.canvas, request.padding)?;
            if i64::from(x) < i64::from(focus[0])
                || i64::from(y) < i64::from(focus[1])
                || u64::from(x) + u64::from(w) > (i64::from(focus[0]) + i64::from(focus[2])) as u64
                || u64::from(y) + u64::from(h) > (i64::from(focus[1]) + i64::from(focus[3])) as u64
            {
                return Err(invalid(
                    "The reference tracking patch must lie within the selected focus box",
                ));
            }
            work += (span[1] - span[0])
                * u64::from(2 * controls.search_radius + 1).pow(2)
                * u64::from(w * h);
        }
    }
    if work > 64_000_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Reframing analysis is bounded to 64000000 aggregate patch-pixel comparisons",
        ));
    }
    let source_indices = (0..count)
        .map(|n| {
            scene::select_frame(layer, n, Time { num: 25, den: 1 })?.ok_or_else(|| {
                error(
                    "REFRAME_UNAVAILABLE",
                    "A source frame is transparent beyond its selected media",
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut decisions = Vec::new();
    for (segment_index, ((start, segment), span)) in
        segments.iter().zip(boundaries.windows(2)).enumerate()
    {
        let length = span[1] - span[0];
        let duration = Time::new(length, 25)?;
        let (boxes, confidence, provenance) = match &segment.selection {
            Selection::Manual { keys } => (
                manual(keys, duration, layer.canvas, length)?,
                vec![None; length as usize],
                "authored_boxes",
            ),
            Selection::Track {
                focus,
                region,
                controls,
            } => {
                let mut window = request.scene.clone();
                let mut tracked = layer.clone();
                tracked.frames = (span[0]..span[1])
                    .map(|n| {
                        let mut frame = layer.frames[source_indices[n as usize]].clone();
                        frame.hold = Time { num: 1, den: 25 };
                        frame.anchor = [0, 0];
                        frame
                    })
                    .collect();
                tracked.start = Time::ZERO;
                tracked.duration = duration;
                tracked.timing = scene::Timing::Strict;
                tracked.end = scene::End::HoldLast;
                tracked.transform.opacity = 255;
                tracked.animation = None;
                tracked.mask = None;
                tracked.effects.clear();
                window.layers = vec![tracked];
                window.duration = duration;
                window.audio = None;
                window.audio_mix = None;
                let observations = tracking::measure(&tracking::Inspect {
                    scene: window,
                    input_root: request.input_root.clone(),
                    layer_id: layer.id.clone(),
                    model: tracking::Model::Translation,
                    region: *region,
                    search_radius: controls.search_radius,
                    maximum_step: controls.maximum_step,
                    maximum_acceleration: controls.maximum_acceleration,
                    minimum_correlation_milli: controls.minimum_correlation_milli,
                    minimum_margin_milli: controls.minimum_margin_milli,
                    maximum_frame_change_milli: controls.maximum_frame_change_milli,
                    mask: crate::composite::RectMask {
                        rect: [0, 0, 1, 1],
                        inverted: false,
                        animation: None,
                        feather: None,
                    },
                })?;
                let boxes = observations
                    .iter()
                    .map(|o| {
                        [
                            focus[0] + o.displacement[0],
                            focus[1] + o.displacement[1],
                            focus[2],
                            focus[3],
                        ]
                    })
                    .collect();
                let confidence = observations
                    .iter()
                    .map(|o| {
                        Some(Confidence {
                            correlation: o.correlation,
                            margin: o.margin,
                            frame_change: o.frame_change,
                        })
                    })
                    .collect();
                (boxes, confidence, "local_patch_tracking")
            }
        };
        for (n, (focus, confidence)) in boxes.into_iter().zip(confidence).enumerate() {
            let padded = rect(focus, layer.canvas, request.padding)?;
            let mut intervals = [[0; 2]; 2];
            let mut desired = [0; 2];
            for axis in 0..2 {
                let extent = request.window[axis] as i32;
                intervals[axis] = [
                    (padded[axis] + padded[axis + 2] - extent).max(0),
                    (layer.canvas[axis] as i32 - extent).min(padded[axis]),
                ];
                desired[axis] = i64::from(2 * padded[axis] + padded[axis + 2] - extent);
            }
            decisions.push(Decision {
                time: Time::new(start + n as u64, 25)?,
                segment: segment_index,
                segment_time: Time::new(n as u64, 25)?,
                subject_id: segment.subject_id.clone(),
                cut: segment.cut && n == 0,
                source_frame: source_indices[*start as usize + n],
                selection: provenance,
                focus,
                padded_focus: padded,
                desired_origin_half_pixels: desired,
                smoothed_origin: [0, 0],
                feasible_origin_intervals: intervals,
                crop: [0, 0, request.window[0] as i32, request.window[1] as i32],
                step: [0, 0],
                constraint_adjustment: [0, 0],
                confidence,
            });
        }
    }
    let mut cuts: Vec<_> = decisions
        .iter()
        .enumerate()
        .filter_map(|(n, d)| d.cut.then_some(n))
        .collect();
    cuts.push(count as usize);
    for group in cuts.windows(2) {
        let slice = &mut decisions[group[0]..group[1]];
        for axis in 0..2 {
            let targets: Vec<_> = slice
                .iter()
                .map(|d| d.desired_origin_half_pixels[axis])
                .collect();
            let smoothed = smooth(&targets, request.smoothing_radius as usize);
            let intervals: Vec<_> = slice
                .iter()
                .map(|d| d.feasible_origin_intervals[axis])
                .collect();
            let selected = path(&intervals, &smoothed, request.maximum_step[axis] as i32)?;
            for (n, d) in slice.iter_mut().enumerate() {
                d.smoothed_origin[axis] = smoothed[n];
                d.crop[axis] = selected[n];
                d.step[axis] = if n == 0 {
                    0
                } else {
                    selected[n] - selected[n - 1]
                };
                d.constraint_adjustment[axis] = selected[n] - smoothed[n];
            }
        }
    }
    let mut result = request.scene.clone();
    result.width = request.output_size[0];
    result.height = request.output_size[1];
    let output = &mut result.layers[0];
    output.frames = decisions
        .iter()
        .map(|d| {
            let mut frame = layer.frames[d.source_frame].clone();
            frame.hold = Time { num: 1, den: 25 };
            frame.anchor = [d.crop[0], d.crop[1]];
            frame
        })
        .collect();
    output.timing = scene::Timing::Strict;
    output.end = scene::End::HoldLast;
    let mut mapping = spatial::Transform::identity();
    mapping.sampling = request.sampling;
    mapping.fit = Some(spatial::Fit {
        size: request.output_size,
        mode: spatial::FitMode::Cover,
        window_size: Some(request.window),
    });
    output.transform.spatial = Some(mapping);
    // Revalidate the complete original input, including unused frames, after analysis.
    scene::inspect(&request.scene, &request.input_root)?;
    let rendered_report = scene::inspect(&result, &request.input_root)?;
    Ok(
        json!({"profile":"subject-reframe-v1","window":request.window,"output_size":request.output_size,"scale":Time::new(u64::from(request.output_size[0]),u64::from(request.window[0]))?,"padding":request.padding,"sampling":request.sampling,"crop_edge_filtering":"neighbouring_source_taps","source_canvas_edge":"transparent","maximum_step":request.maximum_step,"smoothing_radius":request.smoothing_radius,"analysis_patch_pixel_comparisons":work,"decisions":decisions,"scene":result,"scene_report":rendered_report,"input_report":original_report,"applied":false}),
    )
}

pub fn capabilities() -> Value {
    json!({"profile":"subject-reframe-v1","source_canvas":[1,512],"logical_output":[1,512],"source_layers":1,"maximum_frames":128,"segments":[1,16],"selection":["manual_boxes","optional_local_patch_translation"],"window":"explicit_integer_source_rectangle_constant_size","output_aspect":"exactly_equal_to_window","path":"triangular_targets_backwards_feasibility_forward_projection","maximum_smoothing_radius":16,"maximum_patch_pixel_comparisons":64000000,"frame_clock":"25/1","automatic_subject_recognition":false,"automatic_cut_recovery":false,"output":"editable_source_anchors_and_explicit_fitting_window","runtime_model":false})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn future_constraints_and_exact_smoothing() {
        assert_eq!(
            path(&[[0, 8], [0, 8], [8, 8]], &[0, 0, 8], 2).unwrap(),
            [4, 6, 8]
        );
        assert!(path(&[[0, 0], [8, 8]], &[0, 8], 2).is_err());
        assert_eq!(path(&[[2, 2]], &[7], 1).unwrap(), [2]);
        assert_eq!(smooth(&[-3, -1, 1, 3], 0), [-2, -1, 1, 2]);
        assert_eq!(smooth(&[10, 10, 10], 2), [5, 5, 5]);
    }

    #[test]
    fn crop_path_matches_enumerated_candidate_graphs() {
        let mut seed = 87239014u64;
        let mut draw = |limit: u32| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 32) % u64::from(limit)) as i32
        };
        for _ in 0..10_000 {
            let count = draw(9) as usize + 1;
            let maximum = draw(4) + 1;
            let intervals: Vec<_> = (0..count)
                .map(|_| {
                    let a = draw(9);
                    let b = draw(9);
                    [a.min(b), a.max(b)]
                })
                .collect();
            let targets: Vec<_> = (0..count).map(|_| draw(15) - 3).collect();
            // Enumerate individual positions and permissible edges, without interval algebra.
            let mut suffixes: Vec<Vec<i32>> = vec![Vec::new(); count];
            suffixes[count - 1] = (intervals[count - 1][0]..=intervals[count - 1][1]).collect();
            for n in (0..count - 1).rev() {
                suffixes[n] = (intervals[n][0]..=intervals[n][1])
                    .filter(|p| {
                        suffixes[n + 1]
                            .iter()
                            .any(|next| (*p - next).abs() <= maximum)
                    })
                    .collect();
            }
            match path(&intervals, &targets, maximum) {
                Err(_) => assert!(suffixes[0].is_empty()),
                Ok(selected) => {
                    assert!(!suffixes[0].is_empty());
                    for n in 0..count {
                        let candidates: Vec<_> = suffixes[n]
                            .iter()
                            .copied()
                            .filter(|p| n == 0 || (*p - selected[n - 1]).abs() <= maximum)
                            .collect();
                        assert!(candidates.contains(&selected[n]));
                        assert_eq!(
                            (selected[n] - targets[n]).abs(),
                            candidates
                                .iter()
                                .map(|p| (p - targets[n]).abs())
                                .min()
                                .unwrap()
                        );
                    }
                }
            }
        }
    }
}
