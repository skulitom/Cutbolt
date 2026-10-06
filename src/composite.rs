//! Original integer compositing and source-canvas rectangular mask sampling.
use crate::{
    At, Result,
    animation::{Curve, Sampler},
    error,
    time::Time,
};
use serde::{Deserialize, Serialize};

/// How PNG RGB relates to alpha: `straight` (default, ordinary PNG) or `premultiplied` (RGB already multiplied by alpha; no channel may exceed alpha).
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AlphaMode {
    #[default]
    Straight,
    Premultiplied,
}
impl AlphaMode {
    pub fn is_straight(&self) -> bool {
        matches!(self, Self::Straight)
    }
}

/// Blend with the backdrop in encoded sRGB: `normal` (default) uses the source, `multiply` uses `s*d`, `screen` uses `1-(1-s)*(1-d)`.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
}
impl BlendMode {
    pub fn is_normal(&self) -> bool {
        matches!(self, Self::Normal)
    }
}

pub(crate) fn validate_alpha(rgba: &[u8], alpha: AlphaMode) -> Result<()> {
    if matches!(alpha, AlphaMode::Premultiplied)
        && rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[..3].iter().any(|c| *c > p[3]))
    {
        return Err(error(
            "INVALID_ALPHA",
            "Premultiplied RGB channels must not exceed alpha; zero alpha requires zero RGB",
        ));
    }
    Ok(())
}

/// The backdrop is opaque. Preserve exact premultiplied values until final rounding.
pub(crate) fn channel(
    dest: u8,
    source: u8,
    alpha: u8,
    opacity: u8,
    mode: BlendMode,
    encoding: AlphaMode,
) -> u8 {
    masked_channel(dest, source, alpha, opacity, mode, encoding, MASK_WEIGHT)
}

pub(crate) const MASK_WEIGHT: u32 = 65536;

/// Coverage scales both associated color and alpha without an intermediate RGBA8 round.
pub(crate) fn masked_channel(
    dest: u8,
    source: u8,
    alpha: u8,
    opacity: u8,
    mode: BlendMode,
    encoding: AlphaMode,
    mask: u32,
) -> u8 {
    let (d, s, a, o) = (dest as u64, source as u64, alpha as u64, opacity as u64);
    let unit = MASK_WEIGHT as u64;
    let coverage = a * o * mask as u64;
    let weighted = match encoding {
        AlphaMode::Straight => s * a * o,
        AlphaMode::Premultiplied => s * 255 * o,
    } * mask as u64;
    // Each arm divides by its own constant, which compiles to a multiplication instead of a
    // 64-bit division by a run-time denominator for every channel.
    const NORMAL: u64 = 65025 * MASK_WEIGHT as u64;
    const PRODUCT: u64 = 16581375 * MASK_WEIGHT as u64;
    (match mode {
        BlendMode::Normal => (weighted + d * (NORMAL - coverage) + NORMAL / 2) / NORMAL,
        BlendMode::Multiply => {
            (d * weighted + 255 * d * (65025 * unit - coverage) + PRODUCT / 2) / PRODUCT
        }
        BlendMode::Screen => (d * PRODUCT + (255 - d) * weighted + PRODUCT / 2) / PRODUCT,
    }) as u8
}

/// Normal blend of an RGBA source row over an opaque RGB row at full mask coverage, with the
/// values of `channel`: with k = alpha x opacity and X = source x k + dest x (65025 - k), the
/// reference rounds (X x 65536 + 65025 x 32768) / (65025 x 65536), which is (X + 32512) / 65025
/// because 65025 is odd. Zero alpha keeps the destination exactly, so no pixel is skipped and the
/// 32-bit loop vectorizes. `white` blends opaque white with the source alpha (a matte pass).
pub(crate) fn normal_row(
    dest: &mut [u8],
    source: &[u8],
    opacity: u8,
    encoding: AlphaMode,
    white: bool,
) {
    let o = u32::from(opacity);
    let premultiplied = matches!(encoding, AlphaMode::Premultiplied) && !white;
    for (d, s) in dest
        .as_chunks_mut::<3>()
        .0
        .iter_mut()
        .zip(source.as_chunks::<4>().0)
    {
        let k = u32::from(s[3]) * o;
        let rest = 65025 - k;
        for c in 0..3 {
            let weighted = if white {
                255 * k
            } else if premultiplied {
                u32::from(s[c]) * 255 * o
            } else {
                u32::from(s[c]) * k
            };
            d[c] = ((weighted + u32::from(d[c]) * rest + 32512) / 65025) as u8;
        }
    }
}

/// Normal blend of an RGBA source row over an opaque RGB row with per-pixel mask `coverage`, with
/// the values of `masked_channel`. Runs at full coverage take `normal_row`, whose values are
/// `channel`'s, which is `masked_channel` at full coverage; zero coverage keeps the destination
/// exactly, as `masked_channel` does; partial coverage calls `masked_channel` itself. `white` is
/// opaque white with the source alpha, as in `normal_row`.
pub(crate) fn masked_normal_row(
    dest: &mut [u8],
    source: &[u8],
    coverage: &[u32],
    opacity: u8,
    encoding: AlphaMode,
    white: bool,
) {
    let class = |value: u32| match value {
        0 => 0,
        MASK_WEIGHT => 2,
        _ => 1,
    };
    let mut start = 0;
    while start < coverage.len() {
        let kind = class(coverage[start]);
        let end = coverage[start..]
            .iter()
            .position(|&value| class(value) != kind)
            .map_or(coverage.len(), |n| start + n);
        match kind {
            0 => {}
            2 => normal_row(
                &mut dest[start * 3..end * 3],
                &source[start * 4..end * 4],
                opacity,
                encoding,
                white,
            ),
            _ => {
                let mode = if white { AlphaMode::Straight } else { encoding };
                for i in start..end {
                    let s = &source[i * 4..i * 4 + 4];
                    for c in 0..3 {
                        dest[i * 3 + c] = masked_channel(
                            dest[i * 3 + c],
                            if white { 255 } else { s[c] },
                            s[3],
                            opacity,
                            BlendMode::Normal,
                            mode,
                            coverage[i],
                        );
                    }
                }
            }
        }
        start = end;
    }
}

/// Rectangle limiting layer opacity, in source-canvas pixels before crop and transform.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectMask {
    /// `[x, y, width, height]`; x/y -32768..32768, size 0..32768; may extend outside the canvas.
    pub rect: [i32; 4],
    /// Keep pixels outside the rectangle instead; default false.
    #[serde(default)]
    pub inverted: bool,
    /// Optional curves overriding rectangle components on the layer-local clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<MaskAnimation>,
    /// Optional soft edge; omit for a hard edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feather: Option<Feather>,
}

/// Feather ramp placement: `inner` fades in inside the edge, `centered` straddles it, `outer` fades out beyond it.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FeatherEdge {
    Inner,
    Centered,
    Outer,
}

/// Linear soft edge for a layer mask, with square corners.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Feather {
    /// Ramp width in source pixels, 1..4096.
    pub radius: u16,
    /// Ramp placement relative to the rectangle edge.
    pub edge: FeatherEdge,
}

/// Keyframe curves overriding mask rectangle components; declare at least one.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaskAnimation {
    /// Curve for the left edge in source pixels, -32768..32768.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<Curve>,
    /// Curve for the top edge in source pixels, -32768..32768.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<Curve>,
    /// Curve for the width in source pixels, 0..32768.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<Curve>,
    /// Curve for the height in source pixels, 0..32768.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Curve>,
}

pub(crate) struct MaskSampler {
    rect: [i32; 4],
    curves: [Option<Sampler>; 4],
}

impl RectMask {
    pub(crate) fn prepare(&self, duration: Time) -> Result<MaskSampler> {
        if self
            .feather
            .as_ref()
            .is_some_and(|f| !(1..=4096).contains(&f.radius))
        {
            return Err(error(
                "INVALID_MASK",
                "Mask feather radius must be 1..4096 source pixels",
            ));
        }
        for (i, value) in self.rect.iter().enumerate() {
            let minimum = if i < 2 { -32768 } else { 0 };
            if !(minimum..=32768).contains(value) {
                return Err(error(
                    "INVALID_MASK",
                    format!(
                        "rect[{i}]: {value} is outside {minimum}..32768; mask position must be -32768..32768 and dimensions 0..32768"
                    ),
                ));
            }
        }
        let mut curves = [None, None, None, None];
        if let Some(animation) = &self.animation {
            let entries = [
                &animation.x,
                &animation.y,
                &animation.width,
                &animation.height,
            ];
            if entries.iter().all(|c| c.is_none()) {
                return Err(error(
                    "INVALID_MASK",
                    "Mask animation must declare at least one curve",
                ));
            }
            for (i, entry) in entries.iter().enumerate() {
                curves[i] = entry
                    .as_ref()
                    .map(|c| c.prepare(duration, if i < 2 { -32768 } else { 0 }, 32768))
                    .transpose()
                    .under(|| format!("animation.{}", ["x", "y", "width", "height"][i]))?;
            }
        }
        Ok(MaskSampler {
            rect: self.rect,
            curves,
        })
    }

    pub(crate) fn includes(&self, rect: [i32; 4], x: i64, y: i64) -> bool {
        let [left, top, width, height] = rect.map(i64::from);
        let inside = left <= x && x < left + width && top <= y && y < top + height;
        inside != self.inverted
    }

    pub(crate) fn coverage(&self, rect: [i32; 4], x: i64, y: i64) -> u32 {
        let Some(feather) = &self.feather else {
            return if self.includes(rect, x, y) {
                MASK_WEIGHT
            } else {
                0
            };
        };
        let [left, top, width, height] = rect.map(i64::from);
        // Twice the signed distance from the pixel center to the closest rectangle edge.
        // Negative distances give square (Chebyshev) corners outside the rectangle.
        let distance = (2 * (x - left) + 1)
            .min(2 * (left + width - x) - 1)
            .min(2 * (y - top) + 1)
            .min(2 * (top + height - y) - 1);
        let value = if width == 0 || height == 0 {
            0
        } else {
            ramp(feather, distance)
        };
        if self.inverted {
            MASK_WEIGHT - value
        } else {
            value
        }
    }

    /// The mask's coverage over the source-canvas area `[x, y, width, height]`, by axis.
    pub(crate) fn axes(&self, rect: [i32; 4], area: [i64; 4]) -> MaskAxes {
        let [left, top, width, height] = rect.map(i64::from);
        let empty = width == 0 || height == 0;
        // The value of one axis: whether a position lies in the rectangle's span, or the feather
        // ramp of its doubled distance to the span's nearer edge.
        let axis = |start: i64, size: i64, from: i64, count: i64| {
            (from..from + count)
                .map(|p| match &self.feather {
                    None if start <= p && p < start + size => MASK_WEIGHT,
                    None => 0,
                    Some(_) if empty => 0,
                    Some(feather) => ramp(
                        feather,
                        (2 * (p - start) + 1).min(2 * (start + size - p) - 1),
                    ),
                })
                .collect()
        };
        MaskAxes {
            origin: [area[0], area[1]],
            columns: axis(left, width, area[0], area[2]),
            rows: axis(top, height, area[1], area[3]),
            inverted: self.inverted,
        }
    }
}

/// The feather weight of a doubled signed edge distance; it never decreases as the distance grows.
fn ramp(feather: &Feather, distance: i64) -> u32 {
    let radius = i64::from(feather.radius) * 2;
    let (n, d) = match feather.edge {
        FeatherEdge::Inner => (distance, radius),
        FeatherEdge::Centered => (distance + radius, 2 * radius),
        FeatherEdge::Outer => (distance + radius, radius),
    };
    ((n.clamp(0, d) * i64::from(MASK_WEIGHT) + d / 2) / d) as u32
}

/// A rectangle mask's coverage over an area, separated by axis. A hard mask covers a pixel when
/// both its column and its row lie in the rectangle. A feathered mask ramps the smaller of the
/// horizontal and vertical edge distances, and the ramp never decreases, so the ramp of the
/// smaller distance is the smaller of the two ramps. Either way, `RectMask::coverage` is the
/// smaller of a column value and a row value, inverted when the mask is.
pub(crate) struct MaskAxes {
    origin: [i64; 2],
    columns: Vec<u32>,
    rows: Vec<u32>,
    inverted: bool,
}

impl MaskAxes {
    /// The value shared by source-canvas row `y`, before inversion.
    pub(crate) fn row(&self, y: i64) -> u32 {
        self.rows[(y - self.origin[1]) as usize]
    }

    /// Coverage at source-canvas column `x` of a row with value `row`.
    pub(crate) fn at(&self, x: i64, row: u32) -> u32 {
        let value = self.columns[(x - self.origin[0]) as usize].min(row);
        if self.inverted {
            MASK_WEIGHT - value
        } else {
            value
        }
    }

    /// `RectMask::coverage` at source-canvas pixel `(x, y)` inside the area.
    pub(crate) fn coverage(&self, x: i64, y: i64) -> u32 {
        self.at(x, self.row(y))
    }
}

impl MaskSampler {
    pub(crate) fn sample(&self, time: Time) -> Result<[i32; 4]> {
        let mut rect = self.rect;
        for (i, curve) in self.curves.iter().enumerate() {
            if let Some(curve) = curve {
                rect[i] = curve.sample(time)?;
            }
        }
        Ok(rect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_and_opaque_blend_boundaries() {
        for mode in [BlendMode::Normal, BlendMode::Multiply, BlendMode::Screen] {
            assert_eq!(channel(83, 255, 0, 255, mode, AlphaMode::Straight), 83);
            assert_eq!(channel(83, 0, 0, 255, mode, AlphaMode::Premultiplied), 83);
            assert_eq!(channel(83, 255, 255, 0, mode, AlphaMode::Straight), 83);
        }
        assert_eq!(
            channel(83, 0, 255, 255, BlendMode::Multiply, AlphaMode::Straight),
            0
        );
        assert_eq!(
            channel(83, 255, 255, 255, BlendMode::Multiply, AlphaMode::Straight),
            83
        );
        assert_eq!(
            channel(83, 0, 255, 255, BlendMode::Screen, AlphaMode::Straight),
            83
        );
        assert_eq!(
            channel(83, 255, 255, 255, BlendMode::Screen, AlphaMode::Straight),
            255
        );
    }

    #[test]
    fn premultiplied_low_alpha_preserves_signal_and_rejects_invalid_input() {
        assert_eq!(
            channel(0, 1, 1, 255, BlendMode::Normal, AlphaMode::Premultiplied),
            1
        );
        assert!(validate_alpha(&[1, 0, 0, 0], AlphaMode::Premultiplied).is_err());
        assert!(validate_alpha(&[50, 0, 0, 49], AlphaMode::Premultiplied).is_err());
        assert!(validate_alpha(&[200, 30, 0, 0], AlphaMode::Straight).is_ok());
    }

    #[test]
    fn normal_rows_match_the_reference_channel() {
        let alphas: Vec<u8> = (0..=255).collect();
        for opacity in [0u8, 1, 2, 77, 128, 200, 254, 255] {
            for dest in 0..=255u8 {
                for source in [0u8, 1, 2, 63, 127, 128, 200, 254, 255] {
                    for (encoding, white) in [
                        (AlphaMode::Straight, false),
                        (AlphaMode::Premultiplied, false),
                        (AlphaMode::Straight, true),
                    ] {
                        let pixels: Vec<u8> = alphas
                            .iter()
                            .flat_map(|&a| {
                                // Premultiplied channels never exceed alpha.
                                let s = if matches!(encoding, AlphaMode::Premultiplied) {
                                    source.min(a)
                                } else {
                                    source
                                };
                                [s, s / 2, 255 - s, a]
                            })
                            .collect();
                        let mut row = vec![dest; alphas.len() * 3];
                        normal_row(&mut row, &pixels, opacity, encoding, white);
                        for (d, p) in row.chunks(3).zip(pixels.chunks(4)) {
                            for c in 0..3 {
                                let s = if white { 255 } else { p[c] };
                                let mode = if white { AlphaMode::Straight } else { encoding };
                                assert_eq!(
                                    d[c],
                                    channel(dest, s, p[3], opacity, BlendMode::Normal, mode),
                                    "dest {dest} source {s} alpha {} opacity {opacity}",
                                    p[3]
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn mask_axes_match_per_pixel_coverage() {
        let feathers = [
            None,
            Some((1, FeatherEdge::Inner)),
            Some((1, FeatherEdge::Outer)),
            Some((2, FeatherEdge::Centered)),
            Some((3, FeatherEdge::Inner)),
            Some((5, FeatherEdge::Outer)),
            Some((7, FeatherEdge::Centered)),
            Some((4096, FeatherEdge::Centered)),
        ];
        let rects = [
            [3, 4, 10, 6],
            [-5, 2, 8, 30],
            [0, 0, 0, 7],
            [6, 6, 9, 0],
            [12, -3, 1, 1],
            [-40, -40, 100, 100],
            [30, 30, 5, 5],
        ];
        for feather in feathers {
            for rect in rects {
                for inverted in [false, true] {
                    let mask = RectMask {
                        rect,
                        inverted,
                        animation: None,
                        feather: feather.map(|(radius, edge)| Feather { radius, edge }),
                    };
                    let area = [-6, -4, 28, 25];
                    let axes = mask.axes(rect, area);
                    for y in area[1]..area[1] + area[3] {
                        for x in area[0]..area[0] + area[2] {
                            assert_eq!(
                                axes.coverage(x, y),
                                mask.coverage(rect, x, y),
                                "rect {rect:?} feather {feather:?} inverted {inverted} at {x},{y}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn masked_rows_match_the_reference_channel() {
        // Runs of zero, partial and full coverage in every order, including single pixels.
        let pattern = [
            0,
            1,
            MASK_WEIGHT,
            MASK_WEIGHT,
            32768,
            0,
            0,
            MASK_WEIGHT - 1,
            MASK_WEIGHT,
            65,
            12345,
            0,
            MASK_WEIGHT,
        ];
        for opacity in [0u8, 1, 77, 200, 254, 255] {
            for (encoding, white) in [
                (AlphaMode::Straight, false),
                (AlphaMode::Premultiplied, false),
                (AlphaMode::Straight, true),
            ] {
                for shift in 0..pattern.len() {
                    let coverage: Vec<u32> = (0..256)
                        .map(|i| pattern[(i + shift) % pattern.len()])
                        .collect();
                    let pixels: Vec<u8> = (0..=255u8)
                        .flat_map(|a| {
                            let s = a.wrapping_mul(37).wrapping_add(shift as u8);
                            // Premultiplied channels never exceed alpha.
                            if matches!(encoding, AlphaMode::Premultiplied) {
                                let s = s.min(a);
                                [s, s / 3, a - s, a]
                            } else {
                                [s, s / 3, 255 - s, a]
                            }
                        })
                        .collect();
                    let dest: Vec<u8> = (0..256 * 3).map(|i| (i * 89 + shift * 7) as u8).collect();
                    let mut row = dest.clone();
                    masked_normal_row(&mut row, &pixels, &coverage, opacity, encoding, white);
                    for i in 0..256 {
                        let p = &pixels[i * 4..i * 4 + 4];
                        for c in 0..3 {
                            let mode = if white { AlphaMode::Straight } else { encoding };
                            let s = if white { 255 } else { p[c] };
                            assert_eq!(
                                row[i * 3 + c],
                                masked_channel(
                                    dest[i * 3 + c],
                                    s,
                                    p[3],
                                    opacity,
                                    BlendMode::Normal,
                                    mode,
                                    coverage[i]
                                ),
                                "pixel {i} channel {c} coverage {} opacity {opacity}",
                                coverage[i]
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn half_open_masks_including_empty_and_inverse() {
        let mask = RectMask {
            rect: [-2, 1, 3, 2],
            inverted: false,
            animation: None,
            feather: None,
        };
        assert!(mask.includes(mask.rect, -2, 1));
        assert!(mask.includes(mask.rect, 0, 2));
        assert!(!mask.includes(mask.rect, 1, 2));
        assert!(!mask.includes(mask.rect, 0, 3));
        assert!(!mask.includes([0, 0, 0, 5], 0, 0));
        let inverse = RectMask {
            inverted: true,
            ..mask
        };
        assert!(inverse.includes([0, 0, 0, 5], 0, 0));
    }
}
