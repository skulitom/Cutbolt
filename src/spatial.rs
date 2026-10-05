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
const TOTAL_BITS: u32 = TOTAL.trailing_zeros();
const _: () = assert!(TOTAL == 1 << TOTAL_BITS);
const Q_BITS: u32 = Q.trailing_zeros();
const _: () = assert!(Q == 1 << Q_BITS);

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
    /// Whether the mapping is the same at every time: no curves and no stabilization compensation.
    pub(crate) fn is_constant(&self) -> bool {
        self.animation.is_none() && self.compensation.is_none()
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

/// The source pixels a layer can sample in one frame, as `[x, y, width, height]` inside `limit`
/// (source-canvas pixels), with the most taps the frame can take. The destination is the area
/// `limit` can cover, grown by a pixel and clipped to the scene and viewport, as `draw` bounds it.
/// Mapped back and grown by two pixels, that area holds every tap. `None` when nothing is sampled
/// or the mapping is not finite.
pub(crate) fn sampled_region(
    mapping: &Mapping,
    spec: &Transform,
    limit: [i64; 4],
    dimensions: [u32; 2],
) -> Option<([i64; 4], i64)> {
    let [lx, ly, lw, lh] = limit;
    if lw <= 0 || lh <= 0 {
        return None;
    }
    let bounds = |m: [f64; 6], rect: [f64; 4], margin: f64| {
        let [x0, y0, x1, y1] = rect;
        let corners = [(x0, y0), (x1, y0), (x0, y1), (x1, y1)]
            .map(|(x, y)| (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5]));
        let low = |v: [f64; 4]| v.iter().cloned().fold(f64::INFINITY, f64::min).floor() - margin;
        let high =
            |v: [f64; 4]| v.iter().cloned().fold(f64::NEG_INFINITY, f64::max).ceil() + margin;
        let (xs, ys) = (corners.map(|c| c.0), corners.map(|c| c.1));
        let rect = [low(xs), low(ys), high(xs), high(ys)];
        rect.iter().all(|v| v.is_finite()).then_some(rect)
    };
    let grown = [lx - 1, ly - 1, lx + lw + 1, ly + lh + 1].map(|v| v as f64);
    let [mut x0, mut y0, mut x1, mut y1] = bounds(mapping.source_to_scene, grown, 2.0)?;
    (x0, y0) = (x0.max(0.0), y0.max(0.0));
    (x1, y1) = (x1.min(dimensions[0] as f64), y1.min(dimensions[1] as f64));
    if let Some([vx, vy, vw, vh]) = spec.viewport.map(|v| v.map(f64::from)) {
        (x0, y0, x1, y1) = (x0.max(vx), y0.max(vy), x1.min(vx + vw), y1.min(vy + vh));
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let per_pixel = if matches!(spec.sampling, Sampling::Bilinear) {
        4.0
    } else {
        1.0
    };
    let taps = (x1 - x0) * (y1 - y0) * per_pixel;
    let [sx0, sy0, sx1, sy1] = bounds(mapping.scene_to_source, [x0, y0, x1, y1], 2.0)?;
    let left = sx0.max(lx as f64) as i64;
    let top = sy0.max(ly as f64) as i64;
    let right = sx1.min((lx + lw) as f64) as i64;
    let bottom = sy1.min((ly + lh) as f64) as i64;
    (left < right && top < bottom).then_some(([left, top, right - left, bottom - top], taps as i64))
}

/// Destination pixels a layer must visit before its rows are drawn in parallel bands.
const PARALLEL_PIXELS: i64 = 1 << 16;

/// A layer's stored pixels, read without a call per tap: a `size` image placed at `offset` in the
/// source canvas, with an optional mask over the crop. `white` reads opaque white with the stored
/// alpha (a matte pass). `draw` reads it wherever all of a pixel's taps lie inside both the crop
/// and the image, and calls its `pixel` closure elsewhere; the two must agree.
pub(crate) struct Plain<'a> {
    pub rgba: &'a [u8],
    pub size: [i64; 2],
    pub offset: [i64; 2],
    pub encoding: AlphaMode,
    pub white: bool,
    pub mask: Option<&'a crate::composite::MaskAxes>,
}

/// Blends premultiplied tap sums in TOTAL units (tap weight x mask coverage) onto one backdrop
/// pixel, rounding once.
#[inline]
fn blend_total(pixel: &mut [u8], color: [u64; 3], alpha: u64, opacity: u8, blend: BlendMode) {
    // Coverage is at most the mask weight and tap weights sum to WEIGHT, so the sums stay below
    // TOTAL x 255 x 255 < 2^64.
    let remaining = 65025 * TOTAL - alpha as u128 * opacity as u128;
    // round(n / (den * TOTAL)) with TOTAL = 2^48: floor(floor(x / 2^48) / den) equals
    // floor(x / (den * 2^48)), and the shifted value fits in u64, so each channel costs a shift
    // and a 64-bit division by a constant instead of a 128-bit division.
    let round =
        |n: u128, den: u64| (((n + den as u128 * TOTAL / 2) >> TOTAL_BITS) as u64 / den) as u8;
    for i in 0..3 {
        let old = pixel[i] as u128;
        let weighted = color[i] as u128 * opacity as u128;
        pixel[i] = match blend {
            BlendMode::Normal => round(weighted + old * remaining, 65025),
            BlendMode::Multiply => round(old * weighted + 255 * old * remaining, 16581375),
            BlendMode::Screen => round(old * 16581375 * TOTAL + (255 - old) * weighted, 16581375),
        };
    }
}

/// `blend_total`'s normal blend of sums at full mask coverage, given in WEIGHT units (2^16 times
/// smaller). With M = color x opacity + old x (65025 x 2^32 - alpha x opacity), the TOTAL-unit
/// numerator is 2^16 x M, so its rounding (n + 65025 x 2^47) >> 48 is (M + 65025 x 2^31) >> 32.
/// M stays below 2^57, so the blend needs no 128-bit arithmetic.
#[inline]
fn normal_full(pixel: &mut [u8], color: [u64; 3], alpha: u64, opacity: u8) {
    let opacity = opacity as u64;
    let remaining = (65025 << 32) - alpha * opacity;
    for i in 0..3 {
        let m = color[i] * opacity + pixel[i] as u64 * remaining;
        pixel[i] = (((m + (65025 << 31)) >> 32) / 65025) as u8;
    }
}

/// Tap colors are encoded RGB with their declared alpha representation. Accumulate
/// premultiplied channels without intermediate RGBA quantization; round only at the backdrop.
/// With `threads` above one, a large layer draws in parallel row bands: each pixel reads only its
/// own taps and backdrop value, so the bands give the same values.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    rgb: &mut [u8],
    dimensions: [u32; 2],
    mapping: &Mapping,
    spec: &Transform,
    crop: [u32; 4],
    opacity: u8,
    blend: BlendMode,
    threads: usize,
    plain: Option<Plain<'_>>,
    pixel: impl Fn(i64, i64) -> Option<([u8; 4], AlphaMode, u32)> + Sync,
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
            lo.clamp(0.0, size as f64) as i64,
            hi.clamp(0.0, size as f64) as i64,
        )
    };
    let ((mut x0, mut x1), (mut y0, mut y1)) = if finite {
        (
            bound(corners.map(|p| p.0), dimensions[0]),
            bound(corners.map(|p| p.1), dimensions[1]),
        )
    } else {
        ((0, dimensions[0] as i64), (0, dimensions[1] as i64))
    };
    // Pixels outside the viewport are skipped.
    if let Some([vx, vy, vw, vh]) = spec.viewport.map(|v| v.map(i64::from)) {
        (x0, x1) = (x0.max(vx), x1.min(vx + vw));
        (y0, y1) = (y0.max(vy), y1.min(vy + vh));
    }
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    // `a * px + b * py + c` adds the same products in the same order when the column and row
    // products are taken once, so every mapped coordinate is unchanged.
    let columns = (x0..x1)
        .map(|x| {
            let px = x as f64 + 0.5;
            (px, a * px, d * px)
        })
        .collect::<Vec<_>>();
    let q = |value: f64| (value * Q as f64).round() as i64;
    // An axis-aligned mapping has zero cross terms, and zero times any pixel center is the same
    // signed zero. A column's source x (or y) is then the same in every row, and a row's in
    // every column: it is computed once, with the same operations and value.
    let py0 = y0 as f64 + 0.5;
    let column_x = (b == 0.0).then(|| {
        columns
            .iter()
            .map(|&(_, apx, _)| q(apx + b * py0 + c))
            .collect::<Vec<_>>()
    });
    let column_y = (e == 0.0).then(|| {
        columns
            .iter()
            .map(|&(_, _, dpx)| q(dpx + e * py0 + f))
            .collect::<Vec<_>>()
    });
    let clamp = matches!(spec.edge, Edge::Clamp);
    let bilinear = matches!(spec.sampling, Sampling::Bilinear);
    // Taps inside both the crop and the image can be read directly; clamping leaves them as
    // they are.
    let inner = plain.as_ref().map(|p| {
        let [ox, oy] = p.offset;
        (
            [cx.max(ox), (cx + cw).min(ox + p.size[0])],
            [cy.max(oy), (cy + ch).min(oy + p.size[1])],
        )
    });
    // The columns of a row whose samples can land near the crop. Along a row the source point is
    // (a x px + bx, d x px + by): each axis bounds px to an interval, solved for the crop grown by
    // one source pixel (taps reach half a pixel outside it) and widened by two destination
    // pixels, so the skipped pixels get no taps and stay unchanged. Rounding errors are far
    // smaller than these margins.
    let span = |bx: f64, by: f64| -> (usize, usize) {
        let mut range = (f64::NEG_INFINITY, f64::INFINITY);
        for (k, m, lo, hi) in [
            (a, bx, (cx - 1) as f64, (cx + cw + 1) as f64),
            (d, by, (cy - 1) as f64, (cy + ch + 1) as f64),
        ] {
            let (from, to) = if k == 0.0 {
                if m < lo - 1.0 || m > hi + 1.0 {
                    (f64::INFINITY, f64::NEG_INFINITY)
                } else {
                    (f64::NEG_INFINITY, f64::INFINITY)
                }
            } else if k > 0.0 {
                ((lo - m) / k, (hi - m) / k)
            } else {
                ((hi - m) / k, (lo - m) / k)
            };
            range = (range.0.max(from), range.1.min(to));
        }
        if range.0.is_nan() || range.1.is_nan() {
            return (0, columns.len());
        }
        // px is x + 0.5; keep columns from floor(from) - 2 through ceil(to) + 2.
        let first = (range.0.floor() - 2.0).max(x0 as f64).min(x1 as f64) as i64 - x0;
        let last = (range.1.ceil() + 3.0).max(x0 as f64).min(x1 as f64) as i64 - x0;
        (first as usize, last.max(first) as usize)
    };
    let draw_row = |row: &mut [u8], y: i64| {
        let py = y as f64 + 0.5;
        let (bpy, epy) = (b * py, e * py);
        let row_x = (a == 0.0).then(|| q(columns[0].1 + bpy + c));
        let row_y = (d == 0.0).then(|| q(columns[0].2 + epy + f));
        let (start, end) = span(bpy + c, epy + f);
        for (i, (x, &(px, apx, dpx))) in (x0..x1).zip(&columns).enumerate().take(end).skip(start) {
            if let (Some(m), Some(sample)) = (&mapping.scene_to_compensated, &mapping.compensation)
            {
                let u = ((m[0] * px + m[1] * py + m[2]) * Q as f64).round() as i64;
                let v = ((m[3] * px + m[4] * py + m[5]) * Q as f64).round() as i64;
                let [vx, vy, vw, vh] = sample.viewport.map(i64::from);
                if u < vx * Q || u >= (vx + vw) * Q || v < vy * Q || v >= (vy + vh) * Q {
                    continue;
                }
            }
            let qx = match (&column_x, row_x) {
                (Some(column), _) => column[i],
                (None, Some(row)) => row,
                _ => q(apx + bpy + c),
            };
            let qy = match (&column_y, row_y) {
                (Some(column), _) => column[i],
                (None, Some(row)) => row,
                _ => q(dpx + epy + f),
            };
            if clamp && (qx < cx * Q || qx >= (cx + cw) * Q || qy < cy * Q || qy >= (cy + ch) * Q) {
                continue;
            }
            // The taps: the containing pixel, or the four pixel centers around the sample. Q is
            // 2^16, so the shifts and masks are the Euclidean quotients and remainders.
            let (lo, top, u, v) = if bilinear {
                let xx = qx - Q / 2;
                let yy = qy - Q / 2;
                (xx >> Q_BITS, yy >> Q_BITS, xx & (Q - 1), yy & (Q - 1))
            } else {
                (qx >> Q_BITS, qy >> Q_BITS, 0, 0)
            };
            let dest = &mut row[x as usize * 3..x as usize * 3 + 3];
            if let (Some(plain), Some(([ix0, ix1], [iy0, iy1]))) = (&plain, inner) {
                let reach = bilinear as i64;
                if lo >= ix0 && lo + reach < ix1 && top >= iy0 && top + reach < iy1 {
                    let [ox, oy] = plain.offset;
                    let stride = plain.size[0];
                    let straight = plain.encoding.is_straight() || plain.white;
                    let at = |sx: i64, sy: i64| (((sy - oy) * stride + sx - ox) * 4) as usize;
                    // A tap's alpha and premultiplied color.
                    let premultiply = |p: &[u8]| {
                        let alpha = p[3] as u64;
                        let unit = if straight { alpha } else { 255 };
                        let value = |i: usize| if plain.white { 255 } else { p[i] as u64 };
                        (alpha, [0, 1, 2].map(|i| value(i) * unit))
                    };
                    let taps = if bilinear {
                        [
                            (lo, top, (Q - u) * (Q - v)),
                            (lo + 1, top, u * (Q - v)),
                            (lo, top + 1, (Q - u) * v),
                            (lo + 1, top + 1, u * v),
                        ]
                    } else {
                        [(lo, top, Q * Q), (0, 0, 0), (0, 0, 0), (0, 0, 0)]
                    };
                    let taps = &taps[..if bilinear { 4 } else { 1 }];
                    let o = opacity as u64;
                    match plain.mask {
                        None if blend.is_normal() && !bilinear => {
                            // One tap of weight 2^32: M is 2^32 x X with
                            // X = color x opacity + old x (65025 - alpha x opacity), and
                            // (M + 65025 x 2^31) >> 32 is X + 32512, as in `normal_row`.
                            // X stays below 2^26.
                            let (alpha, color) = premultiply(&plain.rgba[at(lo, top)..][..4]);
                            let o = o as u32;
                            let rest = 65025 - alpha as u32 * o;
                            for i in 0..3 {
                                let x = color[i] as u32 * o + dest[i] as u32 * rest;
                                dest[i] = ((x + 32512) / 65025) as u8;
                            }
                        }
                        None if blend.is_normal() => {
                            // Bilinear weights factor as (Q - u or u) x (Q - v or v), so the
                            // weighted sums interpolate each row, then the two rows: exactly the
                            // same integers.
                            let first = at(lo, top);
                            let second = first + stride as usize * 4;
                            let row0: &[u8; 8] = plain.rgba[first..first + 8].try_into().unwrap();
                            let row1: &[u8; 8] = plain.rgba[second..second + 8].try_into().unwrap();
                            let (wx0, wx1) = ((Q - u) as u64, u as u64);
                            let (wy0, wy1) = ((Q - v) as u64, v as u64);
                            let mix = |p00: u64, p01: u64, p10: u64, p11: u64| {
                                wy0 * (wx0 * p00 + wx1 * p01) + wy1 * (wx0 * p10 + wx1 * p11)
                            };
                            if row0[3] & row0[7] & row1[3] & row1[7] == 255 {
                                // Opaque taps: color is 255 x P, with P the weighted encoded
                                // values, and alpha is 255 x 2^32, so M is 255 x N with
                                // N = P x opacity + old x 2^32 x (255 - opacity). Then
                                // ((M + 65025 x 2^31) >> 32) / 65025 is ((N + 255 x 2^31) >> 32) / 255.
                                let keep = (255 - o) << 32;
                                for i in 0..3 {
                                    let p = if plain.white {
                                        255 << 32
                                    } else {
                                        mix(
                                            row0[i] as u64,
                                            row0[4 + i] as u64,
                                            row1[i] as u64,
                                            row1[4 + i] as u64,
                                        )
                                    };
                                    let n = p * o + dest[i] as u64 * keep;
                                    dest[i] = (((n + (255 << 31)) >> 32) / 255) as u8;
                                }
                            } else {
                                let taps = [
                                    premultiply(&row0[..4]),
                                    premultiply(&row0[4..]),
                                    premultiply(&row1[..4]),
                                    premultiply(&row1[4..]),
                                ];
                                let alpha = mix(taps[0].0, taps[1].0, taps[2].0, taps[3].0);
                                let color = [0, 1, 2].map(|i| {
                                    mix(taps[0].1[i], taps[1].1[i], taps[2].1[i], taps[3].1[i])
                                });
                                if alpha == 0 && color == [0; 3] {
                                    continue;
                                }
                                normal_full(dest, color, alpha, opacity);
                            }
                        }
                        mask => {
                            // Tap weight x coverage, in TOTAL units.
                            let mut sums = ([0u64; 3], 0u64);
                            for &(sx, sy, weight) in taps {
                                let coverage = mask
                                    .map_or(crate::composite::MASK_WEIGHT, |m| m.coverage(sx, sy));
                                let weight = weight as u64 * coverage as u64;
                                let (alpha, color) = premultiply(&plain.rgba[at(sx, sy)..][..4]);
                                sums.1 += weight * alpha;
                                for (sum, value) in sums.0.iter_mut().zip(color) {
                                    *sum += weight * value;
                                }
                            }
                            if sums.1 == 0 && sums.0 == [0; 3] {
                                continue;
                            }
                            blend_total(dest, sums.0, sums.1, opacity, blend);
                        }
                    }
                    continue;
                }
            }
            let mut color = [0u64; 3];
            let mut alpha = 0u64;
            let mut tap = |mut sx: i64, mut sy: i64, weight: u64| {
                if weight == 0 {
                    return;
                }
                if clamp {
                    sx = sx.clamp(cx, cx + cw - 1);
                    sy = sy.clamp(cy, cy + ch - 1);
                }
                if sx < cx || sx >= cx + cw || sy < cy || sy >= cy + ch {
                    return;
                }
                let Some((p, encoding, coverage)) = pixel(sx, sy) else {
                    return;
                };
                debug_assert!(coverage <= crate::composite::MASK_WEIGHT);
                let weight = weight * coverage as u64;
                let unit = if encoding.is_straight() {
                    p[3] as u64
                } else {
                    255
                };
                alpha += weight * p[3] as u64;
                for i in 0..3 {
                    color[i] += weight * p[i] as u64 * unit;
                }
            };
            if bilinear {
                tap(lo, top, ((Q - u) * (Q - v)) as u64);
                tap(lo + 1, top, (u * (Q - v)) as u64);
                tap(lo, top + 1, ((Q - u) * v) as u64);
                tap(lo + 1, top + 1, (u * v) as u64);
            } else {
                tap(lo, top, WEIGHT as u64);
            }
            if alpha == 0 && color == [0; 3] {
                // No tap contributed, and each blend keeps the backdrop exactly.
                continue;
            }
            blend_total(dest, color, alpha, opacity, blend);
        }
    };
    let stride = dimensions[0] as usize * 3;
    let rows = &mut rgb[y0 as usize * stride..y1 as usize * stride];
    let bands = if (x1 - x0) * (y1 - y0) >= PARALLEL_PIXELS {
        threads.clamp(1, 64).min((y1 - y0) as usize)
    } else {
        1
    };
    if bands <= 1 {
        for (i, row) in rows.chunks_exact_mut(stride).enumerate() {
            draw_row(row, y0 + i as i64);
        }
        return;
    }
    let per_band = ((y1 - y0) as usize).div_ceil(bands);
    std::thread::scope(|scope| {
        for (band, chunk) in rows.chunks_mut(per_band * stride).enumerate() {
            let draw_row = &draw_row;
            scope.spawn(move || {
                for (i, row) in chunk.chunks_exact_mut(stride).enumerate() {
                    draw_row(row, y0 + (band * per_band + i) as i64);
                }
            });
        }
    });
}
pub fn capabilities() -> Value {
    json!({"profile":"spatial-transform-v1","field":"transform.spatial","compensation":"optional_editable_source_canvas_rigid_before_authored_mapping","sampling":["nearest","bilinear"],"edges":["transparent","clamp"],"fit":["contain","cover","stretch"],"fit_reference":"optional_window_size_or_legacy_crop_dimensions","fit_reference_clipping":false,"pixel_aspect_range":["1/16","16"],"property_units":{"translation":"millipixels","scale":"milli_multiplier","rotation":"millidegrees_clockwise"},"coordinate_grid":"source_pixel_corner_1/65536_nearest_ties_away","alpha":"premultiplied_filtering_without_intermediate_quantization","geometry":"f64_inverse_mapping_exact_quadrants","property_time":"existing_exact_rational_curves","animated":["translate_x_milli","translate_y_milli","scale_x_milli","scale_y_milli","rotation_mdeg"],"antialiasing":"bilinear_only_no_area_minification_or_motion_blur"})
}

/// The former per-pixel `draw`, kept verbatim as the reference for the faster one.
#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(crate) fn reference(
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
            // round(n / (den * TOTAL)) with TOTAL = 2^48: floor(floor(x / 2^48) / den) equals
            // floor(x / (den * 2^48)), and the shifted value fits in u64, so each channel costs a
            // shift and a 64-bit division by a constant instead of a 128-bit division.
            let round = |n: u128, den: u64| {
                (((n + den as u128 * TOTAL / 2) >> TOTAL_BITS) as u64 / den) as u8
            };
            for i in 0..3 {
                let old = rgb[dest + i] as u128;
                let weighted = color[i] * opacity as u128;
                rgb[dest + i] = match blend {
                    BlendMode::Normal => round(weighted + old * remaining, 65025),
                    BlendMode::Multiply => round(old * weighted + 255 * old * remaining, 16581375),
                    BlendMode::Screen => {
                        round(old * 16581375 * TOTAL + (255 - old) * weighted, 16581375)
                    }
                };
            }
        }
    }
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

    struct Random(u64);
    impl Random {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
        fn pick<T: Copy>(&mut self, values: &[T]) -> T {
            values[self.below(values.len() as u64) as usize]
        }
    }

    #[test]
    fn faster_drawing_matches_the_per_pixel_reference() {
        let mut random = Random(0x2545_F491_4F6C_DD1D);
        // A trimmed 23 x 17 image at (2, 3) in a 30 x 24 source canvas, with every alpha class.
        let (width, height, offset) = (23i64, 17i64, [2i64, 3]);
        let straight: Vec<u8> = (0..width * height)
            .flat_map(|_| {
                let alpha = match random.below(4) {
                    0 => 0,
                    1 => 255,
                    _ => random.below(256) as u8,
                };
                [0; 3]
                    .map(|_| random.below(256) as u8)
                    .into_iter()
                    .chain([alpha])
            })
            .collect();
        let premultiplied: Vec<u8> = straight
            .chunks(4)
            .flat_map(|p| {
                let a = p[3] as u32;
                [0, 1, 2]
                    .map(|c| ((p[c] as u32 * a + 127) / 255) as u8)
                    .into_iter()
                    .chain([p[3]])
            })
            .collect();
        let opaque: Vec<u8> = straight
            .chunks(4)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect();
        let coverage: Vec<u32> = (0..30 * 24)
            .map(|_| match random.below(4) {
                0 => 0,
                1 | 2 => crate::composite::MASK_WEIGHT,
                _ => random.below(65537) as u32,
            })
            .collect();
        let duration = Time::new(1, 1).unwrap();
        for case in 0..3000 {
            let big = case % 50 == 0;
            let dimensions = if big { [320u32, 240] } else { [48u32, 40] };
            let crop = [
                random.below(4) as u32,
                random.below(4) as u32,
                26 - random.below(6) as u32,
                20 - random.below(6) as u32,
            ];
            let legacy = crate::scene::Transform {
                position: [dimensions[0] as i32 / 2, dimensions[1] as i32 / 2],
                crop,
                scale: if big { 16 } else { 1 + random.below(2) as u32 },
                quarter_turns: random.below(4) as u8,
                opacity: 255,
                spatial: None,
            };
            let mut spec = Transform::identity();
            spec.translate_milli = [0; 2].map(|_| random.below(10001) as i32 - 5000);
            // Big cases cover the whole destination, so their rows draw in parallel bands.
            spec.scale_milli = [0; 2].map(|_| {
                if big {
                    1000 + random.below(1000) as i32
                } else {
                    300 + random.below(3700) as i32
                }
            });
            spec.rotation_mdeg = random.pick(&[0, 90000, -90000, 45000, -30000, 12345, 359999]);
            spec.flip = [random.below(2) == 1, random.below(2) == 1];
            let (num, den) = random.pick(&[(1, 1), (16, 15), (2, 1)]);
            spec.pixel_aspect = Time::new(num, den).unwrap();
            spec.sampling = random.pick(&[Sampling::Nearest, Sampling::Bilinear]);
            spec.edge = random.pick(&[Edge::Transparent, Edge::Clamp]);
            if !big && random.below(4) == 0 {
                spec.viewport = Some([
                    random.below(30) as i32 - 10,
                    random.below(30) as i32 - 10,
                    1 + random.below(60) as i32,
                    1 + random.below(60) as i32,
                ]);
            }
            let anchor = [
                (crop[0] + crop[2] / 2) as i32,
                (crop[1] + crop[3] / 2) as i32,
            ];
            let mut mapping = spec
                .prepare(duration)
                .unwrap()
                .mapping(&spec, &legacy, anchor, legacy.position, Time::ZERO)
                .unwrap();
            if random.below(6) == 0 {
                // A compensated mapping clips to its viewport through the authored inverse.
                mapping.scene_to_compensated = Some(mapping.scene_to_source);
                mapping.compensation = Some(crate::stabilize::Sample {
                    translation_milli: [0, 0],
                    rotation_mdeg: 0,
                    zoom_milli: 1000,
                    viewport: [
                        random.below(10) as i32,
                        random.below(10) as i32,
                        5 + random.below(20) as i32,
                        5 + random.below(20) as i32,
                    ],
                    forward: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                    inverse: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
                });
            }
            let opacity = random.pick(&[0u8, 1, 128, 200, 255]);
            let blend = random.pick(&[BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen]);
            let mode = random.below(3);
            // Opaque sources take the closed-form blends; both encodings read them alike.
            let (straight, premultiplied) = if random.below(3) == 0 {
                (&opaque, &opaque)
            } else {
                (&straight, &premultiplied)
            };
            // No mask, arbitrary per-pixel coverage, or a rectangle mask.
            let masking = random.below(3);
            let feather = random.pick(&[
                None,
                Some(crate::composite::FeatherEdge::Inner),
                Some(crate::composite::FeatherEdge::Centered),
                Some(crate::composite::FeatherEdge::Outer),
            ]);
            let rect = [
                random.below(20) as i32 - 4,
                random.below(16) as i32 - 4,
                random.below(30) as i32,
                random.below(24) as i32,
            ];
            let rect_mask = crate::composite::RectMask {
                rect,
                inverted: random.below(2) == 1,
                animation: None,
                feather: feather.map(|edge| crate::composite::Feather {
                    radius: 1 + random.below(6) as u16,
                    edge,
                }),
            };
            let axes = rect_mask.axes(rect, crop.map(i64::from));
            let direct = masking != 1 && random.below(4) != 0;
            let plain = || {
                direct.then(|| Plain {
                    rgba: if mode == 1 { premultiplied } else { straight },
                    size: [width, height],
                    offset,
                    encoding: if mode == 1 {
                        AlphaMode::Premultiplied
                    } else {
                        AlphaMode::Straight
                    },
                    white: mode == 2,
                    mask: (masking == 2).then_some(&axes),
                })
            };
            let pixel = |sx: i64, sy: i64| {
                let coverage = match masking {
                    0 => crate::composite::MASK_WEIGHT,
                    1 => coverage[(sy * 30 + sx) as usize],
                    _ => axes.coverage(sx, sy),
                };
                if coverage == 0 {
                    return None;
                }
                let (ix, iy) = (sx - offset[0], sy - offset[1]);
                if ix < 0 || iy < 0 || ix >= width || iy >= height {
                    return None;
                }
                let at = ((iy * width + ix) * 4) as usize;
                Some(match mode {
                    0 => (
                        straight[at..at + 4].try_into().unwrap(),
                        AlphaMode::Straight,
                        coverage,
                    ),
                    1 => (
                        premultiplied[at..at + 4].try_into().unwrap(),
                        AlphaMode::Premultiplied,
                        coverage,
                    ),
                    // A matte pass: white with the source alpha.
                    _ => (
                        [255, 255, 255, straight[at + 3]],
                        AlphaMode::Straight,
                        coverage,
                    ),
                })
            };
            let backdrop: Vec<u8> = (0..dimensions[0] * dimensions[1] * 3)
                .map(|_| random.below(256) as u8)
                .collect();
            let mut expected = backdrop.clone();
            reference(
                &mut expected,
                dimensions,
                &mapping,
                &spec,
                crop,
                opacity,
                blend,
                pixel,
            );
            for threads in [1, 3] {
                let mut actual = backdrop.clone();
                draw(
                    &mut actual,
                    dimensions,
                    &mapping,
                    &spec,
                    crop,
                    opacity,
                    blend,
                    threads,
                    plain(),
                    pixel,
                );
                assert!(
                    actual == expected,
                    "case {case} threads {threads}: {spec:?} {mapping:?} {blend:?} opacity {opacity}"
                );
            }
        }
    }
}
