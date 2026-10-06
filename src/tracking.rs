//! Original bounded patch tracking from content-bound local image sequences.
use crate::{
    At, Result,
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
    /// Layer frame at 25 fps, as failures name it.
    #[serde(skip)]
    pub layer_frame: u64,
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
/// A failure at layer frame `frame`, keeping the observations accepted before it in the error's
/// `detail`, so a caller can shorten the layer to the frames that tracked.
fn lost(frame: u64, reason: &str, accepted: &[Observation]) -> crate::Error {
    let outcome = match accepted {
        [] => "no frame was tracked, and no mask or replacement scene was produced".to_owned(),
        [only] => format!(
            "frame {} tracked before it, but no mask or replacement scene was produced",
            only.layer_frame
        ),
        [first, .., last] => format!(
            "frames {}-{} tracked before it, but no mask or replacement scene was produced",
            first.layer_frame, last.layer_frame
        ),
    };
    crate::Error {
        detail: Some(json!({"failed_layer_frame":frame,"observations":accepted})),
        ..error(
            "TRACKING_UNRELIABLE",
            format!("Layer frame {frame}: {reason}; {outcome}"),
        )
    }
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

/// Track a window cut from a longer layer, whose first frame is `first_frame` of that layer;
/// failures name the longer layer's frames. Observations of the window are not reported.
pub(crate) fn measure(request: &Inspect, first_frame: u64) -> Result<Vec<Observation>> {
    measure_impl(request, false, first_frame).map_err(|e| crate::Error { detail: None, ..e })
}

/// `measure` with the sub-pixel refinement stabilization uses.
pub(crate) fn measure_precise(request: &Inspect, first_frame: u64) -> Result<Vec<Observation>> {
    measure_impl(request, true, first_frame).map_err(|e| crate::Error { detail: None, ..e })
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

fn measure_impl(request: &Inspect, precise: bool, first_frame: u64) -> Result<Vec<Observation>> {
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
    request
        .mask
        .prepare(layer.duration)
        .under(|| "mask".into())?;
    let candidates = u64::from(2 * request.search_radius + 1).pow(2);
    let work = count * candidates * if precise { 26 } else { 1 } * u64::from(w * h);
    if work > 64_000_000 {
        let refinement = if precise {
            " x 26 comparisons per candidate (one whole-pixel and 25 sub-pixel)"
        } else {
            ""
        };
        return Err(error(
            "LIMIT_EXCEEDED",
            format!(
                "Tracking needs {work} patch-pixel comparisons, above the 64000000 limit: {count} frames x {candidates} candidates ((2 x search radius {} + 1)^2){refinement} x {} patch pixels ({w}x{h}); reduce duration, patch or search radius",
                request.search_radius,
                w * h
            ),
        ));
    }
    let mut images = HashMap::new();
    for frame in &layer.frames {
        if !images.contains_key(&frame.image.path) {
            let (_, bytes) = scene::identity_bytes(&frame.image, &request.input_root)?;
            // The scene's own PNG bound; `scene::inspect` above has already held the layer's images
            // to the decoded-pixel budget.
            images.insert(
                frame.image.path.clone(),
                scene::decode_png_bounded(&bytes, scene::MAX_CANVAS)?,
            );
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
        let frame_number = first_frame + n;
        let source_frame = scene::select_frame(layer, start + n, Time { num: 25, den: 1 })?
            .ok_or_else(|| {
                lost(
                    frame_number,
                    "source is transparent beyond its last frame",
                    &observations,
                )
            })?;
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
                frame_number,
                &format!(
                    "whole-canvas change {:.3} thousandths exceeds the declared cut threshold, maximum_frame_change_milli {}",
                    frame_change * 1000.0,
                    request.maximum_frame_change_milli
                ),
                &observations,
            ));
        }
        if n == 0 {
            reference = patch(&canvas, stride, initial);
            reference_sum = reference.iter().map(|&v| i64::from(v)).sum();
            reference_energy = energy(&reference);
            if reference_energy < 25 * (reference.len() as i64).pow(2) {
                // The energy is n^2 times the variance.
                let deviation = (reference_energy as f64).sqrt() / reference.len() as f64;
                return Err(lost(
                    frame_number,
                    &format!(
                        "reference patch {initial:?} has {deviation:.2} encoded levels of luminance deviation, less than the five required; choose a more textured region"
                    ),
                    &observations,
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
        if best.0 < -1.0 {
            return Err(lost(
                frame_number,
                &format!(
                    "no candidate within search radius {radius} of {:?} has five encoded levels of luminance deviation",
                    [previous[0], previous[1]]
                ),
                &observations,
            ));
        }
        if best.0 * 1000.0 + 1e-9 < f64::from(request.minimum_correlation_milli) {
            return Err(lost(
                frame_number,
                &format!(
                    "best match correlation {:.6} at {:?} is below minimum_correlation_milli {} (uniqueness margin {margin:.6})",
                    best.0,
                    [best.1[0], best.1[1]],
                    request.minimum_correlation_milli
                ),
                &observations,
            ));
        }
        if margin * 1000.0 + 1e-9 < f64::from(request.minimum_margin_milli) {
            return Err(lost(
                frame_number,
                &format!(
                    "uniqueness margin {margin:.6} is below minimum_margin_milli {}: the best match at {:?} (correlation {:.6}) barely leads the runner-up ({:.6})",
                    request.minimum_margin_milli,
                    [best.1[0], best.1[1]],
                    best.0,
                    second.max(-1.0)
                ),
                &observations,
            ));
        }
        // The seed region itself must be the unique first match.
        if n == 0 && best.1 != initial {
            return Err(lost(
                frame_number,
                &format!(
                    "initial patch {initial:?} is ambiguous: its best match in the first frame is at {:?}",
                    [best.1[0], best.1[1]]
                ),
                &observations,
            ));
        }
        let step = [
            i64::from(location[0] - previous_milli[0]),
            i64::from(location[1] - previous_milli[1]),
        ];
        let length = |v: [i64; 2]| ((v[0] * v[0] + v[1] * v[1]) as f64).sqrt() / 1000.0;
        if step.iter().map(|v| v * v).sum::<i64>() > (i64::from(request.maximum_step) * 1000).pow(2)
        {
            return Err(lost(
                frame_number,
                &format!(
                    "measured step of {:.3} pixels exceeds maximum_step {}",
                    length(step),
                    request.maximum_step
                ),
                &observations,
            ));
        }
        let change = [step[0] - previous_step[0], step[1] - previous_step[1]];
        if n > 1
            && change.iter().map(|v| v * v).sum::<i64>()
                > (i64::from(request.maximum_acceleration) * 1000).pow(2)
        {
            return Err(lost(
                frame_number,
                &format!(
                    "measured step changed by {:.3} pixels from the previous step, exceeding maximum_acceleration {}",
                    length(change),
                    request.maximum_acceleration
                ),
                &observations,
            ));
        }
        let displacement = [best.1[0] - initial[0], best.1[1] - initial[1]];
        let layer_time = Time::new(n, 25)?;
        observations.push(Observation {
            time: Time::new(start + n, 25)?,
            layer_time,
            layer_frame: frame_number,
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
    let observations = measure_impl(request, false, 0)?;
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

/// Test footage: a 25 fps scene whose one layer `shot` shows `frames`, gray PNGs of
/// `width` x `height` luma bytes written into `root` with their identities.
#[cfg(test)]
pub(crate) fn footage(
    root: &std::path::Path,
    width: u32,
    height: u32,
    frames: &[Vec<u8>],
) -> Scene {
    use sha2::{Digest, Sha256};
    let images: Vec<Value> = frames
        .iter()
        .enumerate()
        .map(|(n, luma)| {
            let name = format!("frame{n}.png");
            let rgb: Vec<u8> = luma.iter().flat_map(|&v| [v, v, v]).collect();
            scene::write_png(&root.join(&name), width, height, &rgb).unwrap();
            let bytes = std::fs::read(root.join(&name)).unwrap();
            json!({"image":{"path":name,"sha256":format!("{:x}", Sha256::digest(&bytes)),"bytes":bytes.len()},
                "hold":{"num":1,"den":25},"offset":[0,0],"anchor":[0,0]})
        })
        .collect();
    let duration = json!({"num":frames.len(),"den":25});
    serde_json::from_value(json!({"schema_version":1,"id":"footage","width":width,"height":height,
        "duration":duration,"output_scale":1,"background":[0,0,0],"color":"srgb_straight_encoded","audio":null,"layers":[{"id":"shot","canvas":[width,height],
        "start":{"num":0,"den":1},"duration":duration,"frames":images,"timing":"strict","end":"hold_last",
        "transform":{"position":[0,0],"crop":[0,0,width,height],"scale":1,"quarter_turns":0,"opacity":255}}]}))
    .unwrap()
}

/// Test texture: deterministic noise over the integer plane, so shifted frames keep one match.
#[cfg(test)]
pub(crate) fn texture(width: u32, height: u32, shift: [i64; 2]) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..i64::from(height) {
        for x in 0..i64::from(width) {
            let (u, v) = ((x - shift[0]) as u64, (y - shift[1]) as u64);
            let h = u.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ v.wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
            pixels.push(((h ^ (h >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9) >> 56) as u8);
        }
    }
    pixels
}

/// A scratch directory removed on drop.
#[cfg(test)]
pub(crate) struct Scratch(pub PathBuf);
#[cfg(test)]
impl Scratch {
    pub(crate) fn new(name: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "cutbolt-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
#[cfg(test)]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn capabilities() -> Value {
    json!({"profile":"patch-tracking-v1","models":["translation"],"input":"content_bound_scene_image_layer","space":"source_canvas_before_effects_and_transform","maximum_frames":128,"patch_dimensions":[4,64],"maximum_search_radius":32,"maximum_patch_pixel_comparisons":64000000,"matching":"fixed_initial_template_normalized_alpha_associated_luminance","minimum_luminance_deviation":5,"confidence":"peak_correlation_and_all_other_candidate_margin","failure":"reject_without_mask_or_scene_reporting_accepted_observations","output":"editable_hold_keyframe_mask_and_explicit_replacement_scene","subpixel_motion":false,"scale_rotation_tracking":false,"automatic_recovery":false})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(scene: Scene, root: &std::path::Path, region: [u32; 4], radius: u32) -> Inspect {
        serde_json::from_value(json!({"scene":scene,"input_root":root,"layer_id":"shot","model":"translation",
            "region":region,"search_radius":radius,"maximum_step":2.min(radius),"maximum_acceleration":4,
            "minimum_correlation_milli":900,"minimum_margin_milli":50,"maximum_frame_change_milli":1000,
            "mask":{"rect":[0,0,8,8],"inverted":false}}))
        .unwrap()
    }

    #[test]
    fn failures_name_the_frame_measurement_and_bound_and_keep_tracked_frames() {
        let scratch = Scratch::new("tracking-messages");
        let root = &scratch.0;
        // The patch moves one pixel, then three: the second step exceeds maximum_step 2.
        let frames = [[0, 0], [1, 0], [4, 0]].map(|shift| texture(48, 32, shift));
        let scene = footage(root, 48, 32, &frames);
        let error = inspect(&request(scene, root, [16, 12, 12, 8], 4)).unwrap_err();
        assert_eq!(error.code, "TRACKING_UNRELIABLE");
        assert_eq!(
            error.message,
            "Layer frame 2: measured step of 3.000 pixels exceeds maximum_step 2; frames 0-1 tracked before it, but no mask or replacement scene was produced"
        );
        let detail = error.detail.unwrap();
        assert_eq!(detail["failed_layer_frame"], 2);
        let observations = detail["observations"].as_array().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[1]["displacement"], json!([1, 0]));
        assert_eq!(observations[1]["layer_time"], json!({"num":1,"den":25}));

        // A flat reference patch reports its measured deviation against the required five levels.
        let flat = Scratch::new("tracking-flat");
        let scene = footage(&flat.0, 48, 32, &[vec![128; 48 * 32], vec![128; 48 * 32]]);
        let error = inspect(&request(scene, &flat.0, [16, 12, 12, 8], 4)).unwrap_err();
        assert_eq!(
            error.message,
            "Layer frame 0: reference patch [16, 12, 12, 8] has 0.00 encoded levels of luminance deviation, less than the five required; choose a more textured region; no frame was tracked, and no mask or replacement scene was produced"
        );
        assert_eq!(error.detail.unwrap()["observations"], json!([]));
    }

    #[test]
    fn excessive_work_states_the_count_and_its_terms() {
        let scratch = Scratch::new("tracking-limit");
        let frames = vec![texture(64, 64, [0, 0]); 4];
        let scene = footage(&scratch.0, 64, 64, &frames);
        let error = inspect(&request(scene, &scratch.0, [0, 0, 64, 64], 32)).unwrap_err();
        assert_eq!(error.code, "LIMIT_EXCEEDED");
        assert_eq!(
            error.message,
            "Tracking needs 69222400 patch-pixel comparisons, above the 64000000 limit: 4 frames x 4225 candidates ((2 x search radius 32 + 1)^2) x 4096 patch pixels (64x64); reduce duration, patch or search radius"
        );
    }
}
