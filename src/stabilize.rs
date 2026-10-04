//! Original camera-motion measurement, rigid compensation and explicit crop decisions.
use crate::{
    Result,
    animation::{Curve, Interpolation, Keyframe, Sampler as CurveSampler},
    error, scene, spatial,
    time::Time,
    tracking,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashMap, path::PathBuf};

/// Editable rigid camera correction applied to the source canvas before the layer's authored spatial mapping; produced by `stabilization.inspect`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Compensation {
    /// Fixed rotation and zoom center `[x, y]` in source-canvas millipixels, each within ±32768000.
    pub center_milli: [i32; 2],
    /// Horizontal correction curve in millipixels, ±32768000, on the layer-local clock.
    pub translation_x_milli: Curve,
    /// Vertical correction curve in millipixels, ±32768000, on the layer-local clock.
    pub translation_y_milli: Curve,
    /// Roll correction curve in millidegrees, ±3600000, on the layer-local clock.
    pub rotation_mdeg: Curve,
    /// Constant zoom about `center_milli`, 1000..4000 (1000 = no zoom).
    pub zoom_milli: u32,
    /// Clip `[x, y, width, height]` in compensated source-canvas pixels, before the authored spatial transform; position ±32768, size 1..32768.
    pub viewport: [i32; 4],
}

pub(crate) struct Sampler {
    center_milli: [i32; 2],
    curves: [CurveSampler; 3],
    zoom_milli: u32,
    viewport: [i32; 4],
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Sample {
    pub translation_milli: [i32; 2],
    pub rotation_mdeg: i32,
    pub zoom_milli: u32,
    pub viewport: [i32; 4],
    pub forward: [f64; 6],
    pub inverse: [f64; 6],
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_STABILIZATION", message)
}

impl Compensation {
    pub(crate) fn prepare(&self, duration: Time) -> Result<Sampler> {
        if self
            .center_milli
            .iter()
            .any(|v| !(-32768000..=32768000).contains(v))
            || !(1000..=4000).contains(&self.zoom_milli)
            || self.viewport[..2]
                .iter()
                .any(|v| !(-32768..=32768).contains(v))
            || self.viewport[2..].iter().any(|v| !(1..=32768).contains(v))
        {
            return Err(invalid(
                "Compensation center supports +/-32768 pixels, zoom 1000..4000, viewport position +/-32768 and size 1..32768",
            ));
        }
        Ok(Sampler {
            center_milli: self.center_milli,
            zoom_milli: self.zoom_milli,
            viewport: self.viewport,
            curves: [
                self.translation_x_milli
                    .prepare(duration, -32768000, 32768000)?,
                self.translation_y_milli
                    .prepare(duration, -32768000, 32768000)?,
                self.rotation_mdeg.prepare(duration, -3600000, 3600000)?,
            ],
        })
    }
}

fn rotation(mdeg: i32) -> (f64, f64) {
    match mdeg.rem_euclid(360000) {
        0 => (0.0, 1.0),
        90000 => (1.0, 0.0),
        180000 => (0.0, -1.0),
        270000 => (-1.0, 0.0),
        value => (f64::from(value) / 1000.0).to_radians().sin_cos(),
    }
}

pub(crate) fn matrix(
    center: [i32; 2],
    translation: [i32; 2],
    angle: i32,
    zoom: u32,
) -> ([f64; 6], [f64; 6]) {
    let [cx, cy] = center.map(|v| f64::from(v) / 1000.0);
    let [tx, ty] = translation.map(|v| f64::from(v) / 1000.0);
    let z = f64::from(zoom) / 1000.0;
    let (s, c) = rotation(angle);
    let forward = [
        z * c,
        -z * s,
        cx + z * (tx - c * cx + s * cy),
        z * s,
        z * c,
        cy + z * (ty - s * cx - c * cy),
    ];
    let inverse = [
        c / z,
        s / z,
        cx - c * (cx / z + tx) - s * (cy / z + ty),
        -s / z,
        c / z,
        cy + s * (cx / z + tx) - c * (cy / z + ty),
    ];
    (forward, inverse)
}

impl Sampler {
    pub(crate) fn sample(&self, time: Time) -> Result<Sample> {
        let translation = [self.curves[0].sample(time)?, self.curves[1].sample(time)?];
        let angle = self.curves[2].sample(time)?;
        let (forward, inverse) = matrix(self.center_milli, translation, angle, self.zoom_milli);
        Ok(Sample {
            translation_milli: translation,
            rotation_mdeg: angle,
            zoom_milli: self.zoom_milli,
            viewport: self.viewport,
            forward,
            inverse,
        })
    }
}

pub(crate) fn compose(a: [f64; 6], b: [f64; 6]) -> [f64; 6] {
    [
        a[0] * b[0] + a[1] * b[3],
        a[0] * b[1] + a[1] * b[4],
        a[0] * b[2] + a[1] * b[5] + a[2],
        a[3] * b[0] + a[4] * b[3],
        a[3] * b[1] + a[4] * b[4],
        a[3] * b[2] + a[4] * b[5] + a[5],
    ]
}

/// Camera motion model: `translation` fits shift only; `rigid` fits shift plus roll about the crop center.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Model {
    Translation,
    Rigid,
}

/// Measurement segment beginning at a declared cut, with its own reference patches; smoothing never crosses segments.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    /// Layer-local start, frame-aligned rational seconds at 25 fps; increasing, the first is zero.
    pub start: Time,
    /// 3..8 stationary background patches `[x, y, width, height]` in source-canvas pixels, sides 4..64, inside the canvas; centers must span a triangle of at least 64 square pixels.
    pub regions: Vec<[u32; 4]>,
}

/// Patch-tracking controls applied to every tracked patch.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tracking {
    /// Search radius in pixels around the previous accepted integer location, 1..32.
    pub search_radius: u32,
    /// Maximum Euclidean displacement between consecutive frames in pixels, 1..`search_radius`.
    pub maximum_step: u32,
    /// Maximum Euclidean change in step between frames in pixels, 1..64.
    pub maximum_acceleration: u32,
    /// Minimum normalized correlation of a match, 850..1000 milli.
    pub minimum_correlation_milli: u16,
    /// Minimum score margin over the next distinct peak, 20..1000 milli.
    pub minimum_margin_milli: u16,
    /// Maximum normalized mean absolute frame-to-frame change, 1..1000 milli.
    pub maximum_frame_change_milli: u16,
}

/// Target camera path within each segment, tagged by `mode`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Smoothing {
    /// Hold each segment's initial pose.
    Lock,
    /// Follow a triangular moving average of the measured path.
    Smooth {
        /// Half-window in frames, 1..32; truncated at segment boundaries.
        radius: u8,
    },
}

/// Border handling for the corrected image, tagged by `mode`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Crop {
    /// Keep zoom at 1000; moving edges may reveal the backdrop.
    Preserve,
    /// Use the smallest constant zoom that keeps the viewport inside every source frame.
    Zoom {
        /// Largest accepted zoom, 1000..4000; if none fits, inspection fails with `STABILIZATION_CROP_LIMIT`.
        maximum_zoom_milli: u32,
    },
}

/// `stabilization.inspect` request: measures camera motion on one image layer and returns a compensated replacement scene; read-only.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Inspect {
    /// Complete scene containing the layer.
    pub scene: scene::Scene,
    /// Absolute directory containing the scene's bound source files.
    pub input_root: PathBuf,
    /// ID of an image layer with 2..128 active frames at 25 fps and no existing compensation.
    pub layer_id: String,
    /// Camera motion model.
    pub model: Model,
    /// 1..16 measurement segments, each covering at least two frames.
    pub segments: Vec<Segment>,
    /// Patch-tracking controls.
    pub tracking: Tracking,
    /// Maximum RMS patch-center fit error per frame in millipixels, 1..2000.
    pub maximum_fit_error_milli: u32,
    /// Maximum measured roll relative to the segment's first frame in millidegrees, 0..15000.
    pub maximum_roll_mdeg: u32,
    /// Target camera path.
    pub smoothing: Smoothing,
    /// Blend from the measured path (0) to the target path (1000), 0..1000.
    pub strength_milli: u16,
    /// Border handling.
    pub crop: Crop,
    /// Sampler written to the returned spatial transform.
    pub sampling: spatial::Sampling,
}

#[derive(Clone, Debug, Serialize)]
struct Pose {
    translation_milli: [i32; 2],
    rotation_mdeg: i32,
}

#[derive(Serialize)]
struct Measurement {
    time: Time,
    layer_time: Time,
    source_frame: usize,
    segment: usize,
    measured: Pose,
    target: Pose,
    compensation: Pose,
    fit_error_pixels: f64,
    minimum_correlation: f64,
    minimum_margin: f64,
}

fn rounded(n: i64, d: i64) -> i32 {
    ((n.abs() + d / 2) / d) as i32 * if n < 0 { -1 } else { 1 }
}

fn fit(
    regions: &[[u32; 4]],
    displacements: &[[i32; 2]],
    center: [i32; 2],
    model: Model,
) -> (Pose, f64) {
    let points: Vec<_> = regions
        .iter()
        .map(|r| {
            [
                f64::from(r[0]) + f64::from(r[2]) / 2.0,
                f64::from(r[1]) + f64::from(r[3]) / 2.0,
            ]
        })
        .collect();
    let targets: Vec<_> = points
        .iter()
        .zip(displacements)
        .map(|(p, d)| {
            [
                p[0] + f64::from(d[0]) / 1000.0,
                p[1] + f64::from(d[1]) / 1000.0,
            ]
        })
        .collect();
    let mean = |items: &Vec<[f64; 2]>| {
        [
            items.iter().map(|p| p[0]).sum::<f64>() / items.len() as f64,
            items.iter().map(|p| p[1]).sum::<f64>() / items.len() as f64,
        ]
    };
    let a = mean(&points);
    let b = mean(&targets);
    let (mut dot, mut cross) = (0.0, 0.0);
    for (p, q) in points.iter().zip(&targets) {
        dot += (p[0] - a[0]) * (q[0] - b[0]) + (p[1] - a[1]) * (q[1] - b[1]);
        cross += (p[0] - a[0]) * (q[1] - b[1]) - (p[1] - a[1]) * (q[0] - b[0]);
    }
    let angle = match model {
        Model::Translation => 0,
        Model::Rigid => (cross.atan2(dot).to_degrees() * 1000.0).round() as i32,
    };
    let (sn, cs) = rotation(angle);
    let [cx, cy] = center.map(|v| f64::from(v) / 1000.0);
    let shift = [
        b[0] - cx - cs * (a[0] - cx) + sn * (a[1] - cy),
        b[1] - cy - sn * (a[0] - cx) - cs * (a[1] - cy),
    ];
    let pose = Pose {
        translation_milli: shift.map(|v| (v * 1000.0).round() as i32),
        rotation_mdeg: angle,
    };
    let (m, _) = matrix(center, pose.translation_milli, angle, 1000);
    let error = (points
        .iter()
        .zip(&targets)
        .map(|(p, q)| {
            (m[0] * p[0] + m[1] * p[1] + m[2] - q[0]).powi(2)
                + (m[3] * p[0] + m[4] * p[1] + m[5] - q[1]).powi(2)
        })
        .sum::<f64>()
        / points.len() as f64)
        .sqrt();
    (pose, error)
}

fn distributed(regions: &[[u32; 4]]) -> bool {
    let points: Vec<_> = regions
        .iter()
        .map(|r| {
            [
                2 * i64::from(r[0]) + i64::from(r[2]),
                2 * i64::from(r[1]) + i64::from(r[3]),
            ]
        })
        .collect();
    let mut area = 0;
    for a in &points {
        for b in &points {
            for c in &points {
                area =
                    area.max(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs());
            }
        }
    }
    // Area is in doubled coordinates: require a triangle of at least 64 source-pixel squares.
    area >= 512
}

fn border_free(
    center: [i32; 2],
    pose: &Pose,
    zoom: u32,
    viewport: [i32; 4],
    source: [f64; 4],
    guard: f64,
) -> bool {
    let (_, m) = matrix(center, pose.translation_milli, pose.rotation_mdeg, zoom);
    let [x, y, w, h] = viewport.map(f64::from);
    [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
        .into_iter()
        .all(|(x, y)| {
            let u = m[0] * x + m[1] * y + m[2];
            let v = m[3] * x + m[4] * y + m[5];
            u >= source[0] + guard - 1e-9
                && v >= source[1] + guard - 1e-9
                && u <= source[2] - guard + 1e-9
                && v <= source[3] - guard + 1e-9
        })
}

pub fn inspect(request: &Inspect) -> Result<Value> {
    scene::inspect(&request.scene, &request.input_root)?;
    let layer = request
        .scene
        .layers
        .iter()
        .find(|l| l.id == request.layer_id)
        .ok_or_else(|| invalid("Layer does not exist"))?;
    let fps = Time::new(25, 1)?;
    let start = layer.start.units(fps)?;
    let count = layer.duration.units(fps)?;
    if layer.graphics.is_some()
        || layer.tilemap.is_some()
        || !(2..=128).contains(&count)
        || request.segments.is_empty()
        || request.segments.len() > 16
        || !(1..=2000).contains(&request.maximum_fit_error_milli)
        || request.maximum_roll_mdeg > 15000
        || request.strength_milli > 1000
        || matches!(request.smoothing,Smoothing::Smooth{radius} if !(1..=32).contains(&radius))
        || matches!(request.crop,Crop::Zoom{maximum_zoom_milli} if !(1000..=4000).contains(&maximum_zoom_milli))
        || layer
            .transform
            .spatial
            .as_ref()
            .is_some_and(|s| s.compensation.is_some())
    {
        return Err(invalid(
            "Need 2..128 image frames, 1..16 segments, fit tolerance 1..2000 millipixels, roll <=15000 millidegrees, strength <=1000, smoothing radius 1..32, zoom 1000..4000 and an uncompensated input",
        ));
    }
    let mut boundaries = Vec::new();
    let mut work = 0u64;
    for segment in &request.segments {
        let at = segment.start.units(fps)?;
        if at >= count
            || boundaries.last().is_some_and(|last| at <= *last)
            || !(3..=8).contains(&segment.regions.len())
            || segment.regions.iter().any(|r| {
                !(4..=64).contains(&r[2])
                    || !(4..=64).contains(&r[3])
                    || u64::from(r[0]) + u64::from(r[2]) > u64::from(layer.canvas[0])
                    || u64::from(r[1]) + u64::from(r[3]) > u64::from(layer.canvas[1])
            })
            || !distributed(&segment.regions)
        {
            return Err(invalid(
                "Segments need increasing frame-aligned starts and 3..8 distributed patches inside the canvas, with dimensions 4..64 and a center triangle area >=64 pixels squared",
            ));
        }
        boundaries.push(at);
    }
    if boundaries[0] != 0 {
        return Err(invalid("First segment must begin at layer time zero"));
    }
    boundaries.push(count);
    if !(1..=32).contains(&request.tracking.search_radius) {
        return Err(invalid("Search radius must be 1..32"));
    }
    for (segment, range) in request.segments.iter().zip(boundaries.windows(2)) {
        if range[1] - range[0] < 2 {
            return Err(invalid(
                "Each segment must contain at least two output frames",
            ));
        }
        work += (range[1] - range[0])
            * (u64::from(2 * request.tracking.search_radius + 1).pow(2) * 26)
            * segment
                .regions
                .iter()
                .map(|r| u64::from(r[2] * r[3]))
                .sum::<u64>();
    }
    if work > 128_000_000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Stabilization supports at most 128000000 patch-pixel comparisons",
        ));
    }
    let [cx, cy, cw, ch] = layer.transform.crop;
    let center = [(cx * 1000 + cw * 500) as i32, (cy * 1000 + ch * 500) as i32];
    let viewport = [cx as i32, cy as i32, cw as i32, ch as i32];
    let mut measurements = Vec::new();
    let mut source_rects = Vec::new();
    let mut dimensions = HashMap::new();
    for (segment_index, (segment, range)) in request
        .segments
        .iter()
        .zip(boundaries.windows(2))
        .enumerate()
    {
        let length = range[1] - range[0];
        let mut window = request.scene.clone();
        let mut target = layer.clone();
        let mut selected = Vec::new();
        target.frames.clear();
        for n in range[0]..range[1] {
            let index = scene::select_frame(layer, start + n, Time { num: 25, den: 1 })?
                .ok_or_else(|| {
                    error(
                        "STABILIZATION_UNRELIABLE",
                        "A tracked source frame is transparent",
                    )
                })?;
            selected.push(index);
            let mut frame = layer.frames[index].clone();
            frame.hold = Time::new(1, 25)?;
            let size = if let Some(size) = dimensions.get(&frame.image.path) {
                *size
            } else {
                let (_, bytes) = scene::identity_bytes(&frame.image, &request.input_root)?;
                let image = scene::decode_png(&bytes)?;
                let size = [image.width, image.height];
                dimensions.insert(frame.image.path.clone(), size);
                size
            };
            source_rects.push([
                f64::from(cx.max(frame.offset[0])),
                f64::from(cy.max(frame.offset[1])),
                f64::from((cx + cw).min(frame.offset[0] + size[0])),
                f64::from((cy + ch).min(frame.offset[1] + size[1])),
            ]);
            target.frames.push(frame);
        }
        target.start = Time::ZERO;
        target.duration = Time::new(length, 25)?;
        target.timing = scene::Timing::Strict;
        target.end = scene::End::HoldLast;
        target.transform = scene::Transform {
            position: [0, 0],
            crop: [0, 0, layer.canvas[0], layer.canvas[1]],
            scale: 1,
            quarter_turns: 0,
            opacity: 255,
            spatial: None,
        };
        target.animation = None;
        target.mask = None;
        target.effects.clear();
        window.layers = vec![target];
        window.duration = Time::new(length, 25)?;
        window.audio = None;
        window.audio_mix = None;
        let mut tracks = Vec::new();
        let controls = &request.tracking;
        for region in &segment.regions {
            let tracked = tracking::measure_precise(&tracking::Inspect {
                scene: window.clone(),
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
            tracks.push(tracked);
        }
        let mut poses = Vec::new();
        let mut errors = Vec::new();
        for n in 0..length as usize {
            let displacements: Vec<_> = tracks
                .iter()
                .map(|t| {
                    t[n].subpixel_displacement_milli
                        .expect("precise measurement")
                })
                .collect();
            let (pose, error) = fit(&segment.regions, &displacements, center, request.model);
            if error * 1000.0 > f64::from(request.maximum_fit_error_milli) + 1e-9
                || pose.rotation_mdeg.unsigned_abs() > request.maximum_roll_mdeg
            {
                return Err(crate::error(
                    "STABILIZATION_UNRELIABLE",
                    format!(
                        "Layer frame {}: rigid fit error {:.6} pixels or roll {} millidegrees exceeds the declared limit",
                        range[0] + n as u64,
                        error,
                        pose.rotation_mdeg
                    ),
                ));
            }
            poses.push(pose);
            errors.push(error);
        }
        for (n, pose) in poses.iter().enumerate() {
            let fields = |p: &Pose| {
                [
                    p.translation_milli[0],
                    p.translation_milli[1],
                    p.rotation_mdeg,
                ]
            };
            let mut smoothed = [0; 3];
            if let Smoothing::Smooth { radius } = request.smoothing {
                let radius = radius as usize;
                let mut sums = [0i64; 3];
                let mut total = 0;
                for (j, p) in poses
                    .iter()
                    .enumerate()
                    .take((n + radius + 1).min(poses.len()))
                    .skip(n.saturating_sub(radius))
                {
                    let weight = (radius + 1 - n.abs_diff(j)) as i64;
                    total += weight;
                    for (sum, v) in sums.iter_mut().zip(fields(p)) {
                        *sum += weight * i64::from(v);
                    }
                }
                smoothed = sums.map(|sum| rounded(sum, total));
            }
            let mut target = fields(pose);
            for (v, smooth) in target.iter_mut().zip(smoothed) {
                *v = rounded(
                    i64::from(*v) * (1000 - i64::from(request.strength_milli))
                        + i64::from(smooth) * i64::from(request.strength_milli),
                    1000,
                );
            }
            let angle = target[2] - pose.rotation_mdeg;
            let (sn, cs) = rotation(angle);
            let [tx, ty] = pose.translation_milli.map(f64::from);
            let compensation = Pose {
                translation_milli: [
                    (f64::from(target[0]) - cs * tx + sn * ty).round() as i32,
                    (f64::from(target[1]) - sn * tx - cs * ty).round() as i32,
                ],
                rotation_mdeg: angle,
            };
            measurements.push(Measurement {
                time: Time::new(start + range[0] + n as u64, 25)?,
                layer_time: Time::new(range[0] + n as u64, 25)?,
                source_frame: selected[n],
                segment: segment_index,
                measured: pose.clone(),
                target: Pose {
                    translation_milli: [target[0], target[1]],
                    rotation_mdeg: target[2],
                },
                compensation,
                fit_error_pixels: errors[n],
                minimum_correlation: tracks.iter().map(|t| t[n].correlation).fold(1.0, f64::min),
                minimum_margin: tracks.iter().map(|t| t[n].margin).fold(2.0, f64::min),
            });
        }
    }
    let guard = if matches!(request.sampling, spatial::Sampling::Bilinear) {
        0.5
    } else {
        0.0
    };
    let covers = |zoom| {
        measurements
            .iter()
            .zip(&source_rects)
            .all(|(m, source)| border_free(center, &m.compensation, zoom, viewport, *source, guard))
    };
    let minimum_zoom = if covers(4000) {
        let (mut low, mut high) = (1000, 4000);
        while low < high {
            let middle = (low + high) / 2;
            if covers(middle) {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        Some(low)
    } else {
        None
    };
    let zoom = match request.crop {
        Crop::Preserve => 1000,
        Crop::Zoom { maximum_zoom_milli } => minimum_zoom
            .filter(|v| *v <= maximum_zoom_milli)
            .ok_or_else(|| {
                error(
                    "STABILIZATION_CROP_LIMIT",
                    "No border-free crop fits the declared maximum zoom",
                )
            })?,
    };
    let curve = |axis: usize| Curve {
        keys: measurements
            .iter()
            .map(|m| Keyframe {
                time: m.layer_time,
                value: if axis < 2 {
                    m.compensation.translation_milli[axis]
                } else {
                    m.compensation.rotation_mdeg
                },
                interpolation: Interpolation::Hold,
            })
            .collect(),
        retime: None,
    };
    let compensation = Compensation {
        center_milli: center,
        translation_x_milli: curve(0),
        translation_y_milli: curve(1),
        rotation_mdeg: curve(2),
        zoom_milli: zoom,
        viewport,
    };
    let mut scene = request.scene.clone();
    let output = scene
        .layers
        .iter_mut()
        .find(|l| l.id == layer.id)
        .expect("validated layer");
    let spec = output
        .transform
        .spatial
        .get_or_insert_with(spatial::Transform::identity);
    spec.compensation = Some(compensation.clone());
    spec.sampling = request.sampling;
    spec.edge = spatial::Edge::Transparent;
    scene::inspect(&scene, &request.input_root)?;
    Ok(
        json!({"profile":"camera-stabilization-v1","analysis":{"model":request.model,"segments":request.segments,"tracking":request.tracking,"maximum_fit_error_milli":request.maximum_fit_error_milli,"maximum_roll_mdeg":request.maximum_roll_mdeg,"smoothing":request.smoothing,"strength_milli":request.strength_milli,"sampling":request.sampling},"layer_id":layer.id,"measurements":measurements,"compensation":compensation,"scene":scene,"crop":{"zoom_milli":zoom,"minimum_border_free_zoom_milli":minimum_zoom,"border_free_source_footprint":covers(zoom),"retained_area_fraction":1_000_000.0/(f64::from(zoom)*f64::from(zoom)),"bilinear_guard_pixels":guard,"alpha_is_preserved_not_filled":true},"sources":layer.frames.iter().map(|f|&f.image).collect::<Vec<_>>(),"applied":false}),
    )
}

pub fn capabilities() -> Value {
    json!({"profile":"camera-stabilization-v1","models":["translation","rigid"],"measurement_grid_milli":250,"measurement_filter":"symmetric_binomial_3x3","confidence":"refined_peak_margin_between_distinct_integer_basins","maximum_patch_pixel_comparisons":128000000,"maximum_per_patch_pixel_comparisons":64000000,"maximum_frames":128,"segments":[1,16],"patches_per_segment":[3,8],"maximum_roll_mdeg":15000,"smoothing":["lock","triangular_window"],"crop":["preserve","constant_zoom"],"maximum_zoom_milli":4000,"correction":"editable_source_canvas_rigid_before_authored_mapping","automatic_cut_recovery":false,"rolling_shutter_correction":false,"external_model":false})
}
