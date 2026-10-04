//! Original bounded 2D mapping with exact property clocks and fixed-weight image sampling.
use crate::{
    Result,
    animation::{Curve, Sampler as CurveSampler},
    composite::{AlphaMode, BlendMode},
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const Q: i64 = 65536;
const WEIGHT: u128 = (Q as u128) * (Q as u128);
const TOTAL: u128 = WEIGHT * crate::composite::MASK_WEIGHT as u128;

/// Source sampling filter: `nearest` (containing source pixel) or `bilinear` (four neighboring pixel centers).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    Nearest,
    Bilinear,
}
/// Crop-edge policy: `transparent` (taps outside the crop are transparent) or `clamp` (samples must map inside the crop; taps clamp to its edge pixels).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Edge {
    Transparent,
    Clamp,
}
/// Aspect fitting: `contain` (smaller ratio, uniform), `cover` (larger ratio, uniform) or `stretch` (independent x/y ratios).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Contain,
    Cover,
    Stretch,
}
/// Scales the source so its reference size fits a target size, before spatial scale and rotation; does not center or crop.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Fit {
    /// Target `[width, height]` in logical scene pixels, each 1..=32768.
    pub size: [u32; 2],
    /// How the two axis ratios combine.
    pub mode: FitMode,
    /// Optional fitting reference `[width, height]`, each 1..=32768; defaults to the source crop size. Source taps stay limited to the crop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_size: Option<[u32; 2]>,
}
/// Keyframe curves on the layer-local clock that override static spatial values; at least one curve is required.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    /// Horizontal translation curve in millipixels, -32768000..=32768000; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_x_milli: Option<Curve>,
    /// Vertical translation curve in millipixels, -32768000..=32768000; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translate_y_milli: Option<Curve>,
    /// Horizontal scale curve in thousandths (1000 = 1.0), 1..=16000; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_x_milli: Option<Curve>,
    /// Vertical scale curve in thousandths (1000 = 1.0), 1..=16000; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scale_y_milli: Option<Curve>,
    /// Clockwise rotation curve in millidegrees, -3600000..=3600000; interpolates signed values, not the shortest arc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_mdeg: Option<Curve>,
}
/// Optional layer `transform.spatial`: subpixel translation, scale, rotation, mirroring, pixel aspect and fitting with filtered sampling.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// Offset added to the sampled layer position, `[x, y]` in millipixels, each -32768000..=32768000.
    pub translate_milli: [i32; 2],
    /// Source axis scale `[x, y]` in thousandths (1000 = 1.0), each 1..=16000.
    pub scale_milli: [i32; 2],
    /// Clockwise rotation in millidegrees added to `quarter_turns`, -3600000..=3600000.
    pub rotation_mdeg: i32,
    /// Mirror the source horizontally/vertically, as `[x, y]`, before rotation.
    pub flip: [bool; 2],
    /// Declared source pixel width/height `{num, den}`, 1/16..=16, reduced denominator at most 1000000.
    pub pixel_aspect: Time,
    /// Sampling filter.
    pub sampling: Sampling,
    /// Behavior at the source crop edge.
    pub edge: Edge,
    /// Optional aspect fitting applied before `scale_milli`; omit for none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,
    /// Optional destination clip `[x, y, width, height]` in logical scene pixels; x/y -32768..=32768, sizes 1..=32768.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub viewport: Option<[i32; 4]>,
    /// Optional curves animating translation, scale and rotation; all other fields are static.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Animation>,
    /// Optional camera-stabilization correction returned by `stabilization.inspect`, composed before this mapping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensation: Option<crate::stabilize::Compensation>,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_SPATIAL", message)
}
pub(crate) struct Sampler {
    values: [i32; 5],
    curves: Vec<Option<CurveSampler>>,
    compensation: Option<crate::stabilize::Sampler>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Mapping {
    pub translate_milli: [i32; 2],
    pub scale_milli: [i32; 2],
    pub rotation_mdeg: i32,
    pub source_to_scene: [f64; 6],
    pub scene_to_source: [f64; 6],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compensation: Option<crate::stabilize::Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_to_compensated: Option<[f64; 6]>,
}
impl Transform {
    pub(crate) fn identity() -> Self {
        Self {
            translate_milli: [0, 0],
            scale_milli: [1000, 1000],
            rotation_mdeg: 0,
            flip: [false, false],
            pixel_aspect: Time { num: 1, den: 1 },
            sampling: Sampling::Nearest,
            edge: Edge::Transparent,
            fit: None,
            viewport: None,
            animation: None,
            compensation: None,
        }
    }
    pub(crate) fn prepare(&self, duration: Time) -> Result<Sampler> {
        self.pixel_aspect.validate()?;
        if self.pixel_aspect.compare(Time::new(1, 16)?)?.is_lt()
            || self.pixel_aspect.compare(Time::new(16, 1)?)?.is_gt()
            || Time::new(self.pixel_aspect.num, self.pixel_aspect.den)?.den > 1_000_000
        {
            return Err(invalid(
                "Pixel aspect must be 1/16..16 with reduced denominator <=1000000",
            ));
        }
        if self.fit.as_ref().is_some_and(|f| {
            f.size.iter().any(|v| !(1..=32768).contains(v))
                || f.window_size
                    .is_some_and(|s| s.iter().any(|v| !(1..=32768).contains(v)))
        }) || self.viewport.is_some_and(|[x, y, w, h]| {
            !(-32768..=32768).contains(&x)
                || !(-32768..=32768).contains(&y)
                || !(1..=32768).contains(&w)
                || !(1..=32768).contains(&h)
        }) {
            return Err(invalid(
                "Fit dimensions and viewport sizes must be 1..32768; viewport positions must be -32768..32768",
            ));
        }
        let values = [
            self.translate_milli[0],
            self.translate_milli[1],
            self.scale_milli[0],
            self.scale_milli[1],
            self.rotation_mdeg,
        ];
        let ranges = [
            (-32_768_000, 32_768_000),
            (-32_768_000, 32_768_000),
            (1, 16000),
            (1, 16000),
            (-3_600_000, 3_600_000),
        ];
        if values
            .iter()
            .zip(ranges)
            .any(|(v, (a, b))| !(a..=b).contains(v))
        {
            return Err(invalid(
                "Translation supports +/-32768 pixels, scale 0.001..16 and rotation +/-3600 degrees",
            ));
        }
        let keys = self
            .animation
            .as_ref()
            .map(|a| {
                [
                    a.translate_x_milli.as_ref(),
                    a.translate_y_milli.as_ref(),
                    a.scale_x_milli.as_ref(),
                    a.scale_y_milli.as_ref(),
                    a.rotation_mdeg.as_ref(),
                ]
            })
            .unwrap_or([None; 5]);
        if self.animation.is_some() && keys.iter().all(|k| k.is_none()) {
            return Err(invalid("Spatial animation must declare at least one curve"));
        }
        let curves = keys
            .iter()
            .zip(ranges)
            .map(|(c, (a, b))| c.map(|c| c.prepare(duration, a, b)).transpose())
            .collect::<Result<_>>()?;
        Ok(Sampler {
            values,
            curves,
            compensation: self
                .compensation
                .as_ref()
                .map(|c| c.prepare(duration))
                .transpose()?,
        })
    }
}
impl Sampler {
    pub(crate) fn mapping(
        &self,
        spec: &Transform,
        legacy: &crate::scene::Transform,
        anchor: [i32; 2],
        position: [i32; 2],
        time: Time,
    ) -> Result<Mapping> {
        let mut values = self.values;
        for (v, c) in values.iter_mut().zip(&self.curves) {
            if let Some(c) = c {
                *v = c.sample(time)?;
            }
        }
        let [tx, ty, sx, sy, rotation] = values;
        let mut scale = [
            legacy.scale as f64 * spec.pixel_aspect.num as f64 / spec.pixel_aspect.den as f64,
            legacy.scale as f64,
        ];
        if let Some(fit) = &spec.fit {
            let window = fit.window_size.unwrap_or([legacy.crop[2], legacy.crop[3]]);
            let x = fit.size[0] as f64 / (window[0] as f64 * scale[0]);
            let y = fit.size[1] as f64 / (window[1] as f64 * scale[1]);
            let factors = match fit.mode {
                FitMode::Contain => [x.min(y); 2],
                FitMode::Cover => [x.max(y); 2],
                FitMode::Stretch => [x, y],
            };
            scale[0] *= factors[0];
            scale[1] *= factors[1];
        }
        for (i, value) in [sx, sy].iter().enumerate() {
            scale[i] *= *value as f64 / 1000.0 * if spec.flip[i] { -1.0 } else { 1.0 };
        }
        let angle = (rotation + legacy.quarter_turns as i32 * 90000).rem_euclid(360000);
        // Preserve exact quadrant landmarks rather than inheriting sine/cosine roundoff.
        let (sin, cos) = match angle {
            0 => (0.0, 1.0),
            90000 => (1.0, 0.0),
            180000 => (0.0, -1.0),
            270000 => (-1.0, 0.0),
            _ => (angle as f64 / 1000.0).to_radians().sin_cos(),
        };
        let dest = [
            position[0] as f64 + tx as f64 / 1000.0,
            position[1] as f64 + ty as f64 / 1000.0,
        ];
        let (ax, ay) = (anchor[0] as f64, anchor[1] as f64);
        let [a, b, c, d] = [
            cos * scale[0],
            -sin * scale[1],
            sin * scale[0],
            cos * scale[1],
        ];
        let [ia, ib, ic, id] = [
            cos / scale[0],
            sin / scale[0],
            -sin / scale[1],
            cos / scale[1],
        ];
        let mut mapping = Mapping {
            translate_milli: [tx, ty],
            scale_milli: [sx, sy],
            rotation_mdeg: rotation,
            source_to_scene: [
                a,
                b,
                dest[0] - a * ax - b * ay,
                c,
                d,
                dest[1] - c * ax - d * ay,
            ],
            scene_to_source: [
                ia,
                ib,
                ax - ia * dest[0] - ib * dest[1],
                ic,
                id,
                ay - ic * dest[0] - id * dest[1],
            ],
            compensation: None,
            scene_to_compensated: None,
        };
        if let Some(compensation) = &self.compensation {
            let sample = compensation.sample(time)?;
            mapping.scene_to_compensated = Some(mapping.scene_to_source);
            mapping.source_to_scene =
                crate::stabilize::compose(mapping.source_to_scene, sample.forward);
            mapping.scene_to_source =
                crate::stabilize::compose(sample.inverse, mapping.scene_to_source);
            mapping.compensation = Some(sample);
        }
        Ok(mapping)
    }
}

/// Tap colors are encoded RGB with their declared alpha representation. Accumulate
/// premultiplied channels without intermediate RGBA quantization; round only at the backdrop.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    rgb: &mut [u8],
    dimensions: [u32; 2],
    mapping: &Mapping,
    spec: &Transform,
    crop: [u32; 4],
    opacity: u8,
    blend: BlendMode,
    mut pixel: impl FnMut(i64, i64) -> Option<([u8; 4], AlphaMode, u32)>,
) {
    let [a, b, c, d, e, f] = mapping.scene_to_source;
    let [cx, cy, cw, ch] = crop.map(i64::from);
    // Visit only destination pixels whose centers can map near the crop: forward-map the crop
    // grown by one source pixel (bilinear taps reach half a pixel outside) and add a two-pixel
    // margin for rounding. Unvisited pixels would receive no taps and stay exactly unchanged.
    let [fa, fb, fc, fd, fe, ff] = mapping.source_to_scene;
    let corners = [
        ((cx - 1) as f64, (cy - 1) as f64),
        ((cx + cw + 1) as f64, (cy - 1) as f64),
        ((cx - 1) as f64, (cy + ch + 1) as f64),
        ((cx + cw + 1) as f64, (cy + ch + 1) as f64),
    ]
    .map(|(sx, sy)| (fa * sx + fb * sy + fc, fd * sx + fe * sy + ff));
    let finite = corners.iter().all(|(x, y)| x.is_finite() && y.is_finite());
    let bound = |values: [f64; 4], size: u32| {
        let lo = values.iter().cloned().fold(f64::INFINITY, f64::min).floor() - 2.0;
        let hi = values
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil()
            + 2.0;
        (
            lo.clamp(0.0, size as f64) as u32,
            hi.clamp(0.0, size as f64) as u32,
        )
    };
    let ((x0, x1), (y0, y1)) = if finite {
        (
            bound(corners.map(|p| p.0), dimensions[0]),
            bound(corners.map(|p| p.1), dimensions[1]),
        )
    } else {
        ((0, dimensions[0]), (0, dimensions[1]))
    };
    for y in y0..y1 {
        for x in x0..x1 {
            if let Some([vx, vy, vw, vh]) = spec.viewport
                && ((x as i64) < vx as i64
                    || (y as i64) < vy as i64
                    || x as i64 >= vx as i64 + vw as i64
                    || y as i64 >= vy as i64 + vh as i64)
            {
                continue;
            }
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            if let (Some(m), Some(sample)) = (&mapping.scene_to_compensated, &mapping.compensation)
            {
                let u = ((m[0] * px + m[1] * py + m[2]) * Q as f64).round() as i64;
                let v = ((m[3] * px + m[4] * py + m[5]) * Q as f64).round() as i64;
                let [vx, vy, vw, vh] = sample.viewport.map(i64::from);
                if u < vx * Q || u >= (vx + vw) * Q || v < vy * Q || v >= (vy + vh) * Q {
                    continue;
                }
            }
            let qx = ((a * px + b * py + c) * Q as f64).round() as i64;
            let qy = ((d * px + e * py + f) * Q as f64).round() as i64;
            if matches!(spec.edge, Edge::Clamp)
                && (qx < cx * Q || qx >= (cx + cw) * Q || qy < cy * Q || qy >= (cy + ch) * Q)
            {
                continue;
            }
            let taps = match spec.sampling {
                Sampling::Nearest => [
                    (qx.div_euclid(Q), qy.div_euclid(Q), WEIGHT as u64),
                    (0, 0, 0),
                    (0, 0, 0),
                    (0, 0, 0),
                ],
                Sampling::Bilinear => {
                    let xx = qx - Q / 2;
                    let yy = qy - Q / 2;
                    let lo = xx.div_euclid(Q);
                    let top = yy.div_euclid(Q);
                    let u = xx.rem_euclid(Q);
                    let v = yy.rem_euclid(Q);
                    [
                        (lo, top, ((Q - u) * (Q - v)) as u64),
                        (lo + 1, top, (u * (Q - v)) as u64),
                        (lo, top + 1, ((Q - u) * v) as u64),
                        (lo + 1, top + 1, (u * v) as u64),
                    ]
                }
            };
            let mut color = [0u128; 3];
            let mut alpha = 0u128;
            for (mut sx, mut sy, weight) in taps {
                if weight == 0 {
                    continue;
                }
                if matches!(spec.edge, Edge::Clamp) {
                    sx = sx.clamp(cx, cx + cw - 1);
                    sy = sy.clamp(cy, cy + ch - 1);
                }
                if sx < cx || sx >= cx + cw || sy < cy || sy >= cy + ch {
                    continue;
                }
                let Some((p, encoding, coverage)) = pixel(sx, sy) else {
                    continue;
                };
                let weight = weight as u128 * coverage as u128;
                alpha += weight * p[3] as u128;
                for i in 0..3 {
                    color[i] += weight
                        * p[i] as u128
                        * if matches!(encoding, AlphaMode::Straight) {
                            p[3] as u128
                        } else {
                            255
                        };
                }
            }
            let remaining = 65025 * TOTAL - alpha * opacity as u128;
            let dest = ((y * dimensions[0] + x) * 3) as usize;
            for i in 0..3 {
                let old = rgb[dest + i] as u128;
                let weighted = color[i] * opacity as u128;
                let (n, den) = match blend {
                    BlendMode::Normal => (weighted + old * remaining, 65025 * TOTAL),
                    BlendMode::Multiply => {
                        (old * weighted + 255 * old * remaining, 16581375 * TOTAL)
                    }
                    BlendMode::Screen => (
                        old * 16581375 * TOTAL + (255 - old) * weighted,
                        16581375 * TOTAL,
                    ),
                };
                rgb[dest + i] = ((n + den / 2) / den) as u8;
            }
        }
    }
}
pub fn capabilities() -> Value {
    json!({"profile":"spatial-transform-v1","field":"transform.spatial","compensation":"optional_editable_source_canvas_rigid_before_authored_mapping","sampling":["nearest","bilinear"],"edges":["transparent","clamp"],"fit":["contain","cover","stretch"],"fit_reference":"optional_window_size_or_legacy_crop_dimensions","fit_reference_clipping":false,"pixel_aspect_range":["1/16","16"],"property_units":{"translation":"millipixels","scale":"milli_multiplier","rotation":"millidegrees_clockwise"},"coordinate_grid":"source_pixel_corner_1/65536_nearest_ties_away","alpha":"premultiplied_filtering_without_intermediate_quantization","geometry":"f64_inverse_mapping_exact_quadrants","property_time":"existing_exact_rational_curves","animated":["translate_x_milli","translate_y_milli","scale_x_milli","scale_y_milli","rotation_mdeg"],"antialiasing":"bilinear_only_no_area_minification_or_motion_blur"})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitting_window_keeps_source_crop_and_pixel_aspect_explicit() {
        let duration = Time::new(1, 1).unwrap();
        let legacy = crate::scene::Transform {
            position: [0, 0],
            crop: [10, 20, 200, 100],
            scale: 3,
            quarter_turns: 0,
            opacity: 255,
            spatial: None,
        };
        let mut spec = Transform::identity();
        spec.pixel_aspect = Time::new(2, 1).unwrap();
        for (mode, expected) in [
            (FitMode::Contain, [3.0, 0.0, -30.0, 0.0, 1.5, -30.0]),
            (FitMode::Cover, [6.0, 0.0, -60.0, 0.0, 3.0, -60.0]),
            (FitMode::Stretch, [3.0, 0.0, -30.0, 0.0, 3.0, -60.0]),
        ] {
            spec.fit = Some(Fit {
                size: [120, 60],
                mode,
                window_size: Some([40, 20]),
            });
            let map = spec
                .prepare(duration)
                .unwrap()
                .mapping(&spec, &legacy, [10, 20], [0, 0], Time::ZERO)
                .unwrap();
            assert_eq!(map.source_to_scene, expected);
        }
        spec.fit = Some(Fit {
            size: [120, 60],
            mode: FitMode::Contain,
            window_size: None,
        });
        let map = spec
            .prepare(duration)
            .unwrap()
            .mapping(&spec, &legacy, [10, 20], [0, 0], Time::ZERO)
            .unwrap();
        for (actual, expected) in map
            .source_to_scene
            .into_iter()
            .zip([0.6, 0.0, -6.0, 0.0, 0.3, -6.0])
        {
            assert!((actual - expected).abs() < 1e-12);
        }
        for invalid in [[0, 20], [20, 0], [32769, 1], [1, u32::MAX]] {
            spec.fit.as_mut().unwrap().window_size = Some(invalid);
            assert!(spec.prepare(duration).is_err());
        }
    }
}
