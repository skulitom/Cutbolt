//! Original bounded patch tracking from content-bound local image sequences.
use crate::{
    Result,
    animation::{Curve, Interpolation, Keyframe},
    composite::{AlphaMode, MaskAnimation, RectMask},
    error,
    scene::{self, Scene},
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};

/// Tracking motion model; only `translation` (integer-pixel patch displacement) is supported.
#[derive(Clone, Copy, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    Translation,
}

/// Request for `tracking.inspect`: track an image patch and return a moving mask plus a replacement scene, saving nothing. Work is limited to 64000000 patch-pixel comparisons.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    /// Scene recipe containing the tracked layer; validated before and after analysis.
    pub scene: Scene,
    /// Absolute directory containing the scene's relative image paths.
    pub input_root: PathBuf,
    /// ID of an image-frame layer with 2..=128 active frames at 25 fps; graphics and tilemap layers reject.
    pub layer_id: String,
    /// Motion model.
    pub model: Model,
    /// Initial reference patch `[x, y, width, height]` in source canvas pixels before effects and transforms; width/height 4..=64, inside the canvas.
    pub region: [u32; 4],
    /// Search distance in pixels around the previous accepted position, 1..=32.
    pub search_radius: u32,
    /// Maximum Euclidean move between consecutive frames in pixels, 1..=`search_radius`.
    pub maximum_step: u32,
    /// Maximum Euclidean change between consecutive steps in pixels, 1..=64.
    pub maximum_acceleration: u32,
    /// Minimum normalized correlation with the fixed initial patch, in thousandths, 850..=1000.
    pub minimum_correlation_milli: u16,
    /// Minimum correlation lead over every other candidate, in thousandths, 20..=1000.
    pub minimum_margin_milli: u16,
    /// Cut threshold: maximum mean absolute luminance change over the canvas, in thousandths of 255, 1..=1000.
    pub maximum_frame_change_milli: u16,
    /// Static rectangle mask (no `animation`), optionally feathered; returned with hold x/y curves following the patch.
    pub mask: RectMask,
}

#[derive(Clone, Serialize)]
pub(crate) struct Observation {
    pub time: Time,
    pub layer_time: Time,
    pub source_frame: usize,
    pub region: [i32; 4],
    pub displacement: [i32; 2],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subpixel_displacement_milli: Option<[i32; 2]>,
    pub correlation: f64,
    pub margin: f64,
    pub frame_change: f64,
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_TRACKING", message)
}
fn lost(n: u64, reason: &str) -> crate::Error {
    error(
        "TRACKING_UNRELIABLE",
        format!("Layer frame {n}: {reason}; no trajectory or replacement scene was produced"),
    )
}

fn patch(image: &[u8], stride: usize, rect: [i32; 4]) -> Vec<u8> {
    let [x, y, w, h] = rect.map(|v| v as usize);
    (y..y + h)
        .flat_map(|row| {
            image[row * stride + x..row * stride + x + w]
                .iter()
                .copied()
        })
        .collect()
}

/// Uncentered integer sums retain precision before the one square root/division.
fn energy(values: &[u8]) -> i64 {
    let sum: i64 = values.iter().map(|&v| i64::from(v)).sum();
    values.len() as i64 * values.iter().map(|&v| i64::from(v).pow(2)).sum::<i64>() - sum * sum
}

fn correlation(
    reference: &[u8],
    reference_sum: i64,
    reference_energy: i64,
    image: &[u8],
    stride: usize,
    rect: [i32; 4],
) -> Option<f64> {
    let [x, y, w, h] = rect.map(|v| v as usize);
    let mut sum = 0i64;
    let mut square = 0i64;
    let mut product = 0i64;
    for row in 0..h {
        for col in 0..w {
            let a = i64::from(reference[row * w + col]);
            let b = i64::from(image[(y + row) * stride + x + col]);
            sum += b;
            square += b * b;
            product += a * b;
        }
    }
    let n = reference.len() as i64;
    let en = n * square - sum * sum;
    if en < 25 * n * n {
        return None;
    }
    Some(
        ((n * product - reference_sum * sum) as f64
            / ((en as f64) * (reference_energy as f64)).sqrt())
        .clamp(-1.0, 1.0),
    )
}

pub(crate) fn measure(request: &Inspect) -> Result<Vec<Observation>> {
    measure_impl(request, false)
}

pub(crate) fn measure_precise(request: &Inspect) -> Result<Vec<Observation>> {
    measure_impl(request, true)
}

fn precise_correlation(
    reference: &[u8],
    image: &[u8],
    stride: usize,
    rect: [i32; 4],
    shift: [i32; 2],
) -> Option<f64> {
    let [x, y, w, h] = rect;
    let x = f64::from(x) + f64::from(shift[0]) / 1000.0;
    let y = f64::from(y) + f64::from(shift[1]) / 1000.0;
    let height = image.len() / stride;
    if x < 0.0 || y < 0.0 || x + f64::from(w) > stride as f64 || y + f64::from(h) > height as f64 {
        return None;
    }
    let (left, top) = (x.floor() as usize, y.floor() as usize);
    let (u, v) = (x.fract(), y.fract());
    let (mut sum, mut square, mut product) = (0.0, 0.0, 0.0);
    let mean = reference.iter().map(|&v| f64::from(v)).sum::<f64>() / reference.len() as f64;
    let energy = reference
        .iter()
        .map(|&v| (f64::from(v) - mean).powi(2))
        .sum::<f64>();
    for yy in 0..h as usize {
        for xx in 0..w as usize {
            let (x, y) = (left + xx, top + yy);
            let (right, bottom) = ((x + 1).min(stride - 1), (y + 1).min(height - 1));
            let value = f64::from(image[y * stride + x]) * (1.0 - u) * (1.0 - v)
                + f64::from(image[y * stride + right]) * u * (1.0 - v)
                + f64::from(image[bottom * stride + x]) * (1.0 - u) * v
                + f64::from(image[bottom * stride + right]) * u * v;
            sum += value;
            square += value * value;
            product += (f64::from(reference[yy * w as usize + xx]) - mean) * value;
        }
    }
    let variance = square - sum * sum / reference.len() as f64;
    if variance < 25.0 * reference.len() as f64 {
        return None;
    }
    Some((product / (energy * variance).sqrt()).clamp(-1.0, 1.0))
}

fn measure_impl(request: &Inspect, precise: bool) -> Result<Vec<Observation>> {
    let [x, y, w, h] = request.region;
    if !(4..=64).contains(&w)
        || !(4..=64).contains(&h)
        || !(1..=32).contains(&request.search_radius)
        || request.maximum_step == 0
        || request.maximum_step > request.search_radius
        || !(1..=64).contains(&request.maximum_acceleration)
        || !(850..=1000).contains(&request.minimum_correlation_milli)
        || !(20..=1000).contains(&request.minimum_margin_milli)
        || !(1..=1000).contains(&request.maximum_frame_change_milli)
        || request.mask.animation.is_some()
    {
        return Err(invalid(
            "Patch dimensions 4..64, radius 1..32, step 1..radius, acceleration 1..64, correlation 850..1000, margin 20..1000, frame change 1..1000 and a static mask are required",
        ));
    }
    // Reuse complete scene validation, identity checks, frame semantics and alpha validation.
    scene::inspect(&request.scene, &request.input_root)?;
    let layer = request
        .scene
        .layers
        .iter()
        .find(|l| l.id == request.layer_id)
        .ok_or_else(|| invalid("Tracked layer does not exist"))?;
    let count = layer.duration.units(Time::new(25, 1)?)?;
    let start = layer.start.units(Time::new(25, 1)?)?;
    if layer.graphics.is_some()
        || layer.tilemap.is_some()
        || !(2..=128).contains(&count)
        || u64::from(x) + u64::from(w) > u64::from(layer.canvas[0])
        || u64::from(y) + u64::from(h) > u64::from(layer.canvas[1])
    {
        return Err(invalid(
            "Tracking requires 2..128 active image frames and an initial patch inside the source canvas",
        ));
    }
    request.mask.prepare(layer.duration)?;
    if count
        * u64::from(2 * request.search_radius + 1).pow(2)
        * if precise { 26 } else { 1 }
        * u64::from(w * h)
        > 64_000_000
    {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Tracking is bounded to 64000000 patch-pixel comparisons; reduce duration, patch or search radius",
        ));
    }
    let mut images = HashMap::new();
    for frame in &layer.frames {
        if !images.contains_key(&frame.image.path) {
            let (_, bytes) = scene::identity_bytes(&frame.image, &request.input_root)?;
            images.insert(frame.image.path.clone(), scene::decode_png(&bytes)?);
        }
    }
    let initial = [x as i32, y as i32, w as i32, h as i32];
    let mut previous = initial;
    let mut previous_step = [0i64; 2];
    let mut previous_milli = [initial[0] * 1000, initial[1] * 1000];
    let mut previous_image: Option<Vec<u8>> = None;
    let mut reference = Vec::new();
    let mut reference_sum = 0;
    let mut reference_energy = 0;
    let mut observations = Vec::new();
    let stride = layer.canvas[0] as usize;
    for n in 0..count {
        let source_frame = scene::select_frame(layer, start + n)?
            .ok_or_else(|| lost(n, "source is transparent beyond its last frame"))?;
        let frame = &layer.frames[source_frame];
        let image = &images[&frame.image.path];
        let mut canvas = vec![0u8; (layer.canvas[0] * layer.canvas[1]) as usize];
        for row in 0..image.height as usize {
            for col in 0..image.width as usize {
                let p = &image.rgba[(row * image.width as usize + col) * 4..][..4];
                let luma = u32::from(p[0]) * 54 + u32::from(p[1]) * 183 + u32::from(p[2]) * 19;
                let value = match layer.alpha_mode {
                    AlphaMode::Straight => (luma * u32::from(p[3]) + 32640) / 65280,
                    AlphaMode::Premultiplied => (luma + 128) / 256,
                };
                canvas
                    [(row + frame.offset[1] as usize) * stride + col + frame.offset[0] as usize] =
                    value as u8;
            }
        }
        if precise {
            // A symmetric binomial low-pass reduces phase-dependent sampling artifacts.
            // This affects measurement only; render pixels remain the original source bytes.
            let original = canvas.clone();
            let height = layer.canvas[1] as usize;
            for yy in 0..height {
                for xx in 0..stride {
                    let mut value = 0u32;
                    for (dy, wy) in [(-1, 1), (0, 2), (1, 1)] {
                        for (dx, wx) in [(-1, 1), (0, 2), (1, 1)] {
                            let x = (xx as i64 + dx).clamp(0, stride as i64 - 1) as usize;
                            let y = (yy as i64 + dy).clamp(0, height as i64 - 1) as usize;
                            value += u32::from(original[y * stride + x]) * wx * wy;
                        }
                    }
                    canvas[yy * stride + xx] = ((value + 8) / 16) as u8;
                }
            }
        }
        let frame_change = previous_image.as_ref().map_or(0.0, |p| {
            p.iter()
                .zip(&canvas)
                .map(|(a, b)| u64::from(a.abs_diff(*b)))
                .sum::<u64>() as f64
                / (canvas.len() as f64 * 255.0)
        });
        if frame_change * 1000.0 > f64::from(request.maximum_frame_change_milli) {
            return Err(lost(
                n,
                "whole-canvas change exceeds the declared cut threshold",
            ));
        }
        if n == 0 {
            reference = patch(&canvas, stride, initial);
            reference_sum = reference.iter().map(|&v| i64::from(v)).sum();
            reference_energy = energy(&reference);
            if reference_energy < 25 * (reference.len() as i64).pow(2) {
                return Err(lost(
                    n,
                    "reference patch has less than five encoded levels of luminance deviation",
                ));
            }
        }
        let radius = request.search_radius as i32;
        let mut best = (-2.0f64, previous);
        let mut second = -2.0f64;
        let mut candidates = Vec::new();
        for yy in (previous[1] - radius).max(0)
            ..=(previous[1] + radius).min(layer.canvas[1] as i32 - h as i32)
        {
            for xx in (previous[0] - radius).max(0)
                ..=(previous[0] + radius).min(layer.canvas[0] as i32 - w as i32)
            {
                let rect = [xx, yy, w as i32, h as i32];
                if let Some(score) = correlation(
                    &reference,
                    reference_sum,
                    reference_energy,
                    &canvas,
                    stride,
                    rect,
                ) {
                    if precise {
                        candidates.push((score, rect));
                    }
                    if score > best.0 {
                        second = best.0;
                        best = (score, rect);
                    } else if score > second {
                        second = score;
                    }
                }
            }
        }
        let mut location = [best.1[0] * 1000, best.1[1] * 1000];
        if precise {
            candidates.sort_by(|a, b| b.0.total_cmp(&a.0));
            let mut peaks: Vec<[i32; 4]> = Vec::new();
            let mut refined = Vec::new();
            for (original, rect) in candidates {
                if peaks
                    .iter()
                    .any(|p| (p[0] - rect[0]).abs() <= 1 && (p[1] - rect[1]).abs() <= 1)
                {
                    continue;
                }
                peaks.push(rect);
                let mut score = original;
                let mut at = [rect[0] * 1000, rect[1] * 1000];
                for dy in [-500, -250, 0, 250, 500] {
                    for dx in [-500, -250, 0, 250, 500] {
                        if let Some(value) =
                            precise_correlation(&reference, &canvas, stride, rect, [dx, dy])
                            && value > score + 1e-12
                        {
                            score = value;
                            at = [rect[0] * 1000 + dx, rect[1] * 1000 + dy];
                        }
                    }
                }
                refined.push((score, rect, at));
            }
            refined.sort_by(|a, b| b.0.total_cmp(&a.0));
            if let Some(&(score, rect, at)) = refined.first() {
                best = (score, rect);
                location = at;
            }
            second = refined.get(1).map_or(-1.0, |p| p.0);
        }
        let margin = best.0 - second.max(-1.0);
        if best.0 * 1000.0 + 1e-9 < f64::from(request.minimum_correlation_milli)
            || margin * 1000.0 + 1e-9 < f64::from(request.minimum_margin_milli)
        {
            return Err(lost(
                n,
                &format!(
                    "patch correlation {:.6} or uniqueness margin {:.6} is insufficient",
                    best.0, margin
                ),
            ));
        }
        // The seed region itself must be the unique first match.
        if n == 0 && best.1 != initial {
            return Err(lost(n, "initial patch is ambiguous"));
        }
        let step = [
            i64::from(location[0] - previous_milli[0]),
            i64::from(location[1] - previous_milli[1]),
        ];
        if step.iter().map(|v| v * v).sum::<i64>() > (i64::from(request.maximum_step) * 1000).pow(2)
            || n > 1
                && step
                    .iter()
                    .zip(previous_step)
                    .map(|(a, b)| (a - b).pow(2))
                    .sum::<i64>()
                    > (i64::from(request.maximum_acceleration) * 1000).pow(2)
        {
            return Err(lost(
                n,
                "measured motion exceeds the step or acceleration bound",
            ));
        }
        let displacement = [best.1[0] - initial[0], best.1[1] - initial[1]];
        let layer_time = Time::new(n, 25)?;
        observations.push(Observation {
            time: Time::new(start + n, 25)?,
            layer_time,
            source_frame,
            region: best.1,
            displacement,
            subpixel_displacement_milli: precise.then_some([
                location[0] - initial[0] * 1000,
                location[1] - initial[1] * 1000,
            ]),
            correlation: best.0,
            margin,
            frame_change,
        });
        previous = best.1;
        previous_step = step;
        previous_milli = location;
        previous_image = Some(canvas);
    }
    scene::inspect(&request.scene, &request.input_root)?;
    Ok(observations)
}

pub fn inspect(request: &Inspect) -> Result<Value> {
    let observations = measure(request)?;
    let layer = request
        .scene
        .layers
        .iter()
        .find(|l| l.id == request.layer_id)
        .expect("validated layer");
    let mut keys: [Vec<Keyframe>; 2] = [Vec::new(), Vec::new()];
    for observation in &observations {
        for (axis, keys) in keys.iter_mut().enumerate() {
            keys.push(Keyframe {
                time: observation.layer_time,
                value: request.mask.rect[axis] + observation.displacement[axis],
                interpolation: Interpolation::Hold,
            });
        }
    }
    let [x, y] = keys;
    let mut mask = request.mask.clone();
    mask.animation = Some(MaskAnimation {
        x: Some(Curve {
            keys: x,
            retime: None,
        }),
        y: Some(Curve {
            keys: y,
            retime: None,
        }),
        width: None,
        height: None,
    });
    mask.prepare(layer.duration)?;
    let mut scene = request.scene.clone();
    scene
        .layers
        .iter_mut()
        .find(|l| l.id == request.layer_id)
        .expect("validated layer")
        .mask = Some(mask.clone());
    // Recheck every dependency after analysis, including invisible/nonselected sources.
    scene::inspect(&scene, &request.input_root)?;
    Ok(
        json!({"profile":"patch-tracking-v1","model":"translation","space":"source_canvas_before_effects_and_transform","layer_id":layer.id,"observations":observations,"mask":mask,"scene":scene,"sources":layer.frames.iter().map(|f| &f.image).collect::<Vec<_>>(),"applied":false}),
    )
}

pub fn capabilities() -> Value {
    json!({"profile":"patch-tracking-v1","models":["translation"],"input":"content_bound_scene_image_layer","space":"source_canvas_before_effects_and_transform","maximum_frames":128,"patch_dimensions":[4,64],"maximum_search_radius":32,"maximum_patch_pixel_comparisons":64000000,"matching":"fixed_initial_template_normalized_alpha_associated_luminance","minimum_luminance_deviation":5,"confidence":"peak_correlation_and_all_other_candidate_margin","failure":"reject_entire_trajectory_without_output","output":"editable_hold_keyframe_mask_and_explicit_replacement_scene","subpixel_motion":false,"scale_rotation_tracking":false,"automatic_recovery":false})
}
