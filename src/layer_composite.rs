//! Bounded per-sample layer evaluation and compositing, shared by scene and timeline renderers.
//! Callers supply the owner-clock time and one decoded image; preparation stores only curve keys.
use crate::{
    At, Result,
    composite::{self, AlphaMode, BlendMode, RectMask},
    effects, error,
    scene::{Animation, Pixels, Transform},
    time::Time,
};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Sample {
    pub position: [i32; 2],
    pub opacity: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask_rect: Option<[i32; 4]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<crate::effects::Sample>,
    /// Boxed: a mapping is large, and long scenes hold one record per layer and frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spatial: Option<Box<crate::spatial::Mapping>>,
}
/// Borrowed layer properties; frame selection and source decoding belong to the caller.
#[derive(Clone, Copy)]
pub(crate) struct Recipe<'a> {
    pub transform: &'a Transform,
    pub animation: Option<&'a Animation>,
    pub mask: Option<&'a RectMask>,
    pub effects: &'a [effects::Effect],
    pub alpha_mode: AlphaMode,
    pub blend_mode: BlendMode,
}

/// Expression bindings override sampled properties before computing the spatial mapping.
#[derive(Default)]
pub(crate) struct Overrides {
    pub position: Option<[i32; 2]>,
    pub opacity: Option<u8>,
}

pub(crate) struct Sampler<'a> {
    transform: &'a Transform,
    spatial: Option<crate::spatial::Sampler>,
    effects: Vec<effects::Sampler>,
    mask: Option<composite::MaskSampler>,
    x: Option<crate::animation::Sampler>,
    y: Option<crate::animation::Sampler>,
    opacity: Option<crate::animation::Sampler>,
}

impl<'a> Recipe<'a> {
    /// Validate and prepare once, using the source duration for clips or layer duration for scenes.
    pub(crate) fn prepare(&self, duration: Time) -> Result<Sampler<'a>> {
        let spatial = self
            .transform
            .spatial
            .as_ref()
            .map(|s| s.prepare(duration))
            .transpose()
            .under(|| "transform.spatial".into())?;
        let effects = crate::effects::prepare(self.effects, duration)?;
        let mask = self
            .mask
            .as_ref()
            .map(|m| m.prepare(duration))
            .transpose()
            .under(|| "mask".into())?;
        let (mut x, mut y, mut opacity) = (None, None, None);
        if let Some(animation) = self.animation {
            if animation.position_x.is_none()
                && animation.position_y.is_none()
                && animation.opacity.is_none()
            {
                return Err(error(
                    "INVALID_ANIMATION",
                    "Animation must declare at least one property curve",
                ));
            }
            x = animation
                .position_x
                .as_ref()
                .map(|c| c.prepare(duration, -32768, 32768))
                .transpose()
                .under(|| "animation.position_x".into())?;
            y = animation
                .position_y
                .as_ref()
                .map(|c| c.prepare(duration, -32768, 32768))
                .transpose()
                .under(|| "animation.position_y".into())?;
            opacity = animation
                .opacity
                .as_ref()
                .map(|c| c.prepare(duration, 0, 255))
                .transpose()
                .under(|| "animation.opacity".into())?;
        }
        Ok(Sampler {
            transform: self.transform,
            spatial,
            effects,
            mask,
            x,
            y,
            opacity,
        })
    }
}

impl Sampler<'_> {
    /// Sample coverage before the byte round, retaining the exact eased source clock.
    pub(crate) fn opacity_precise(&self, time: Time, override_value: Option<u16>) -> Result<u16> {
        time.validate()?;
        if let Some(value) = override_value {
            return Ok(value);
        }
        self.opacity
            .as_ref()
            .map_or(Ok(u16::from(self.transform.opacity) * 257), |curve| {
                curve.sample_scaled(time, 257).map(|value| value as u16)
            })
    }
    /// Evaluate one exact owner-clock time, without allocating a duration-sized sample table.
    pub(crate) fn sample(
        &self,
        time: Time,
        anchor: [i32; 2],
        overrides: Overrides,
    ) -> Result<Sample> {
        let position = [
            self.x
                .as_ref()
                .map(|c| c.sample(time))
                .transpose()?
                .unwrap_or(self.transform.position[0]),
            self.y
                .as_ref()
                .map(|c| c.sample(time))
                .transpose()?
                .unwrap_or(self.transform.position[1]),
        ];
        let opacity = self
            .opacity
            .as_ref()
            .map(|c| c.sample(time))
            .transpose()?
            .map(|v| v as u8)
            .unwrap_or(self.transform.opacity);
        let position = overrides.position.unwrap_or(position);
        let opacity = overrides.opacity.unwrap_or(opacity);
        let spatial = self
            .spatial
            .as_ref()
            .map(|sampler| {
                sampler.mapping(
                    self.transform.spatial.as_ref().expect("prepared"),
                    self.transform,
                    anchor,
                    position,
                    time,
                )
            })
            .transpose()?
            .map(Box::new);
        Ok(Sample {
            position,
            opacity,
            spatial,
            effects: self
                .effects
                .iter()
                .map(|e| e.sample(time))
                .collect::<Result<_>>()?,
            mask_rect: self.mask.as_ref().map(|m| m.sample(time)).transpose()?,
        })
    }
}

/// One source image and its already sampled placement; no scene or timeline frame table required.
pub(crate) struct Frame<'a> {
    pub recipe: Recipe<'a>,
    pub parameters: &'a Sample,
    pub image: &'a Pixels,
    pub offset: [u32; 2],
    pub anchor: [i32; 2],
}
impl Frame<'_> {
    /// Draw onto an opaque packed-RGB backdrop. Alpha, opacity and mask coverage round only at
    /// the final blend, exactly as scene layers do. Legacy overlay byte rounding is separate.
    pub(crate) fn draw(
        &self,
        rgb: &mut [u8],
        dimensions: [u32; 2],
        processor: Option<effects::Processor<'_>>,
        matte: bool,
        threads: usize,
    ) {
        let Self {
            recipe,
            parameters,
            image,
            offset,
            anchor,
        } = *self;
        let t = recipe.transform;
        debug_assert_eq!(
            rgb.len(),
            dimensions[0] as usize * dimensions[1] as usize * 3
        );
        debug_assert_eq!(
            image.rgba.len(),
            image.width as usize * image.height as usize * 4
        );
        if let (Some(spec), Some(mapping)) = (&t.spatial, &parameters.spatial) {
            let fetch = |sx: i64, sy: i64| -> Option<[u8; 4]> {
                let ix = sx - offset[0] as i64;
                let iy = sy - offset[1] as i64;
                if ix < 0 || iy < 0 || ix >= image.width as i64 || iy >= image.height as i64 {
                    return None;
                }
                Some(
                    image.rgba[((iy * image.width as i64 + ix) * 4) as usize..][..4]
                        .try_into()
                        .expect("RGBA"),
                )
            };
            let tap = |sx: i64, sy: i64, coverage: u32| {
                if coverage == 0 {
                    return None;
                }
                let p = fetch(sx, sy)?;
                let processed = processor
                    .as_ref()
                    .and_then(|processor| processor.pixel(&p, recipe.alpha_mode, [sx, sy]));
                Some(match processed {
                    _ if matte => (
                        [255, 255, 255, processed.unwrap_or(p)[3]],
                        AlphaMode::Straight,
                        coverage,
                    ),
                    Some(p) => (p, AlphaMode::Straight, coverage),
                    None => (p, recipe.alpha_mode, coverage),
                })
            };
            let size = [dimensions[0], dimensions[1]];
            let (opacity, blend) = (parameters.opacity, recipe.blend_mode);
            #[cfg(test)]
            if reference_paths() {
                crate::spatial::reference(
                    rgb,
                    size,
                    mapping,
                    spec,
                    t.crop,
                    opacity,
                    blend,
                    |sx, sy| {
                        let coverage = recipe
                            .mask
                            .as_ref()
                            .zip(parameters.mask_rect)
                            .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                                mask.coverage(rect, sx, sy)
                            });
                        tap(sx, sy, coverage)
                    },
                );
                return;
            }
            // Taps stay inside the crop, so the mask's axes cover the crop.
            let mask = recipe
                .mask
                .as_ref()
                .zip(parameters.mask_rect)
                .map(|(mask, rect)| mask.axes(rect, t.crop.map(i64::from)));
            // Per-frame effects on a straight layer run once for each source pixel the frame can
            // sample, not once per tap, unless those pixels outnumber the taps. Effects read only
            // a pixel and its position, so the values are the same; taps outside the region
            // still go through `tap`.
            let [ox, oy] = offset.map(i64::from);
            let [cx, cy, cw, ch] = t.crop.map(i64::from);
            let (x0, y0) = (cx.max(ox), cy.max(oy));
            let limit = [
                x0,
                y0,
                (cx + cw).min(ox + image.width as i64) - x0,
                (cy + ch).min(oy + image.height as i64) - y0,
            ];
            let frame_processed = processor
                .as_ref()
                .filter(|_| recipe.alpha_mode.is_straight())
                .and_then(|processor| {
                    let (region, taps) =
                        crate::spatial::sampled_region(mapping, spec, limit, size)?;
                    (region[2] * region[3] <= taps).then(|| {
                        let pixels = process_region(image, offset, region, processor, threads);
                        (pixels, region)
                    })
                });
            // Otherwise, without per-frame effects, taps inside the image read its stored pixels
            // directly, with the values `tap` gives.
            let plain = match &frame_processed {
                Some((pixels, [x, y, width, height])) => Some(crate::spatial::Plain {
                    rgba: &pixels.rgba,
                    size: [*width, *height],
                    offset: [*x, *y],
                    encoding: AlphaMode::Straight,
                    white: matte,
                    mask: mask.as_ref(),
                }),
                None => processor.is_none().then(|| crate::spatial::Plain {
                    rgba: &image.rgba,
                    size: [image.width as i64, image.height as i64],
                    offset: [ox, oy],
                    encoding: recipe.alpha_mode,
                    white: matte,
                    mask: mask.as_ref(),
                }),
            };
            crate::spatial::draw(
                rgb,
                size,
                mapping,
                spec,
                t.crop,
                opacity,
                blend,
                threads,
                plain,
                |sx, sy| {
                    let coverage = mask
                        .as_ref()
                        .map_or(composite::MASK_WEIGHT, |m| m.coverage(sx, sy));
                    tap(sx, sy, coverage)
                },
            );
            return;
        }
        let [cx, cy, cw, ch] = t.crop;
        let scale = t.scale as i64;
        let ax = anchor[0] as i64 - cx as i64;
        let ay = anchor[1] as i64 - cy as i64;
        let (ax, ay) = match t.quarter_turns {
            0 => (ax, ay),
            1 => (ch as i64 - ay, ax),
            2 => (cw as i64 - ax, ch as i64 - ay),
            _ => (ay, cw as i64 - ax),
        };
        let left = parameters.position[0] as i64 - ax * scale;
        let top = parameters.position[1] as i64 - ay * scale;
        // Iterate only the destination rectangle the transformed crop can cover (clipped to the
        // canvas), so off-canvas scales cannot cause unbounded work and small layers stay cheap.
        // Pixels outside it never reach a source pixel, so the result is unchanged.
        let (span_x, span_y) = if t.quarter_turns.is_multiple_of(2) {
            (cw as i64 * scale, ch as i64 * scale)
        } else {
            (ch as i64 * scale, cw as i64 * scale)
        };
        let x0 = left.clamp(0, dimensions[0] as i64) as u32;
        let x1 = (left + span_x).clamp(0, dimensions[0] as i64) as u32;
        let y0 = top.clamp(0, dimensions[1] as i64) as u32;
        let y1 = (top + span_y).clamp(0, dimensions[1] as i64) as u32;
        if parameters.opacity == 0 {
            // Zero coverage leaves every destination value unchanged in each blend mode.
            return;
        }
        // Scaled source offsets of each destination column and row, divided once per layer and
        // row rather than per pixel. -1 marks a position before the layer, as no source pixel.
        if (processor.is_none() || recipe.alpha_mode.is_straight())
            && recipe.blend_mode.is_normal()
            && scale == 1
            && t.quarter_turns == 0
            && !reference_paths()
        {
            // Unscaled, unrotated normal layers (most sprites, cards and stage layers): each
            // destination row reads one contiguous source run. Effects run once per source pixel
            // of the run, as the general path runs them once per destination pixel; a straight
            // pixel they leave unchanged stays straight, so the row is all straight. A mask's
            // coverage comes from its column and row values; effects also run where it is zero,
            // which changes nothing there.
            let mut processed = Vec::new();
            let mut coverage = Vec::new();
            let (ox, oy) = (offset[0] as i64, offset[1] as i64);
            // Source-canvas columns rx + cx must lie in the crop and in the image placed at offset.
            let rx0 = 0.max(ox - cx as i64).max(x0 as i64 - left);
            let rx1 = (cw as i64)
                .min(image.width as i64 + ox - cx as i64)
                .min(x1 as i64 - left);
            let ry0 = 0.max(oy - cy as i64).max(y0 as i64 - top);
            let ry1 = (ch as i64)
                .min(image.height as i64 + oy - cy as i64)
                .min(y1 as i64 - top);
            let mask = recipe
                .mask
                .as_ref()
                .zip(parameters.mask_rect)
                .filter(|_| rx1 > rx0 && ry1 > ry0)
                .map(|(mask, rect)| {
                    let area = [rx0 + cx as i64, ry0 + cy as i64, rx1 - rx0, ry1 - ry0];
                    mask.axes(rect, area)
                });
            for ry in ry0..ry1.max(ry0) {
                if rx1 <= rx0 {
                    break;
                }
                let row_coverage = mask.as_ref().map(|m| m.row(ry + cy as i64));
                let dy = (ry + top) as usize;
                let iy = (ry + cy as i64 - oy) as usize;
                let ix = (rx0 + cx as i64 - ox) as usize;
                let width = (rx1 - rx0) as usize;
                let dest = (dy * dimensions[0] as usize + (rx0 + left) as usize) * 3;
                let source = (iy * image.width as usize + ix) * 4;
                let run = &image.rgba[source..source + width * 4];
                let row = match &processor {
                    None => run,
                    Some(processor) => {
                        processed.clear();
                        if let Some([r, g, b]) = processor.tables() {
                            // Zero alpha composites nothing, so its table values are harmless.
                            for p in run.as_chunks::<4>().0 {
                                processed.extend_from_slice(&[
                                    r[p[0] as usize],
                                    g[p[1] as usize],
                                    b[p[2] as usize],
                                    p[3],
                                ]);
                            }
                        } else {
                            let y = ry + cy as i64;
                            for (i, p) in run.as_chunks::<4>().0.iter().enumerate() {
                                let position = [rx0 + cx as i64 + i as i64, y];
                                processed.extend_from_slice(
                                    &processor
                                        .pixel(p, AlphaMode::Straight, position)
                                        .unwrap_or(*p),
                                );
                            }
                        }
                        &processed
                    }
                };
                let dest = &mut rgb[dest..dest + width * 3];
                match (&mask, row_coverage) {
                    (Some(mask), Some(row_value)) => {
                        coverage.clear();
                        coverage.extend((rx0..rx1).map(|rx| mask.at(rx + cx as i64, row_value)));
                        composite::masked_normal_row(
                            dest,
                            row,
                            &coverage,
                            parameters.opacity,
                            recipe.alpha_mode,
                            matte,
                        );
                    }
                    _ => composite::normal_row(
                        dest,
                        row,
                        parameters.opacity,
                        recipe.alpha_mode,
                        matte,
                    ),
                }
            }
            return;
        }
        let unscale = |r: i64| if r < 0 { -1 } else { r / scale };
        let columns = (x0..x1)
            .map(|dx| unscale(dx as i64 - left))
            .collect::<Vec<_>>();
        for dy in y0..y1 {
            let ry = unscale(dy as i64 - top);
            for (dx, &rx) in (x0..x1).zip(&columns) {
                let (sx, sy) = match t.quarter_turns {
                    0 => (rx, ry),
                    1 => (ry, ch as i64 - 1 - rx),
                    2 => (cw as i64 - 1 - rx, ch as i64 - 1 - ry),
                    _ => (cw as i64 - 1 - ry, rx),
                };
                if sx < 0 || sy < 0 || sx >= cw as i64 || sy >= ch as i64 {
                    continue;
                }
                let coverage = recipe
                    .mask
                    .as_ref()
                    .zip(parameters.mask_rect)
                    .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                        mask.coverage(rect, sx + cx as i64, sy + cy as i64)
                    });
                if coverage == 0 {
                    continue;
                }
                let sx = sx + cx as i64 - offset[0] as i64;
                let sy = sy + cy as i64 - offset[1] as i64;
                if sx < 0 || sy < 0 || sx >= image.width as i64 || sy >= image.height as i64 {
                    continue;
                }
                let p = &image.rgba[((sy * image.width as i64 + sx) * 4) as usize..][..4];
                let processed = processor.as_ref().and_then(|processor| {
                    processor.pixel(
                        p,
                        recipe.alpha_mode,
                        [sx + offset[0] as i64, sy + offset[1] as i64],
                    )
                });
                let white;
                let (p, alpha_mode) = if let Some(processed) = &processed {
                    (processed.as_slice(), AlphaMode::Straight)
                } else {
                    (p, recipe.alpha_mode)
                };
                if p[3] == 0 {
                    // Zero alpha (premultiplied RGB is then zero too) leaves the backdrop exact.
                    continue;
                }
                let (p, alpha_mode) = if matte {
                    white = [255, 255, 255, p[3]];
                    (white.as_slice(), AlphaMode::Straight)
                } else {
                    (p, alpha_mode)
                };
                let dest = ((dy * dimensions[0] + dx) * 3) as usize;
                for c in 0..3 {
                    rgb[dest + c] = if coverage == composite::MASK_WEIGHT {
                        composite::channel(
                            rgb[dest + c],
                            p[c],
                            p[3],
                            parameters.opacity,
                            recipe.blend_mode,
                            alpha_mode,
                        )
                    } else {
                        composite::masked_channel(
                            rgb[dest + c],
                            p[c],
                            p[3],
                            parameters.opacity,
                            recipe.blend_mode,
                            alpha_mode,
                            coverage,
                        )
                    };
                }
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    pub(crate) static REFERENCE_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a test asked this thread to compose with the former per-pixel paths, to compare the
/// faster ones with.
#[cfg(test)]
fn reference_paths() -> bool {
    REFERENCE_PATHS.with(std::cell::Cell::get)
}
#[cfg(not(test))]
fn reference_paths() -> bool {
    false
}

/// The `region` (`[x, y, width, height]` in the source canvas, inside the image) of `image`
/// (straight RGBA placed at `offset`) through an effect chain, in up to `threads` row bands.
pub(crate) fn process_region(
    image: &Pixels,
    offset: [u32; 2],
    region: [i64; 4],
    processor: &crate::effects::Processor,
    threads: usize,
) -> Pixels {
    let [rx, ry, rw, rh] = region;
    let [ox, oy] = offset.map(i64::from);
    let width = rw as usize;
    let mut rgba = Vec::with_capacity(width * rh as usize * 4);
    for y in ry..ry + rh {
        let start = (((y - oy) * i64::from(image.width) + rx - ox) * 4) as usize;
        rgba.extend_from_slice(&image.rgba[start..start + width * 4]);
    }
    let process = |chunk: &mut [u8], first: usize| {
        for (i, p) in chunk.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let x = (i % width) as i64 + rx;
            let y = (first + i / width) as i64 + ry;
            if let Some(q) = processor.pixel(p, AlphaMode::Straight, [x, y]) {
                *p = q;
            }
        }
    };
    if width > 0 && !rgba.is_empty() {
        let rows = (rh as usize).div_ceil(threads.max(1)).max(1);
        if threads <= 1 {
            process(&mut rgba, 0);
        } else {
            std::thread::scope(|scope| {
                for (band, chunk) in rgba.chunks_mut(rows * width * 4).enumerate() {
                    let process = &process;
                    scope.spawn(move || process(chunk, band * rows));
                }
            });
        }
    }
    Pixels {
        width: rw as u32,
        height: rh as u32,
        rgba,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn streamed_samples_use_source_time_and_apply_bindings_before_mapping() {
        let curve = |first, last| {
            json!({"keys": [
                {"time": {"num": 0, "den": 1}, "value": first, "interpolation": "linear"},
                {"time": {"num": 7200, "den": 1}, "value": last, "interpolation": "hold"}
            ]})
        };
        let transform = Transform {
            position: [0, 0],
            crop: [0, 0, 1, 1],
            scale: 1,
            quarter_turns: 0,
            opacity: 255,
            spatial: Some(crate::spatial::Transform::identity()),
        };
        let animation: Animation = serde_json::from_value(json!({
            "position_x": curve(0, 240), "opacity": curve(0, 240)
        }))
        .unwrap();
        let mask: RectMask = serde_json::from_value(json!({
            "rect": [0, 0, 1, 1], "animation": {"x": curve(0, 240)}
        }))
        .unwrap();
        let recipe = Recipe {
            transform: &transform,
            animation: Some(&animation),
            mask: Some(&mask),
            effects: &[],
            alpha_mode: AlphaMode::Straight,
            blend_mode: BlendMode::Normal,
        };
        // This is a two-hour source, independent of the bounded scene frame table. Sampling
        // a seek, backwards request or split boundary must not depend on previous calls.
        let sampler = recipe.prepare(Time::new(7200, 1).unwrap()).unwrap();
        for (num, den, expected) in [
            (7200, 1, 240),
            (0, 1, 0),
            (3600, 1, 120),
            (90, 2, 2),
            (15, 1, 1),
            (3600, 1, 120),
        ] {
            let sample = sampler
                .sample(Time::new(num, den).unwrap(), [0, 0], Overrides::default())
                .unwrap();
            assert_eq!(sample.position, [expected, 0]);
            assert_eq!(sample.opacity, expected as u8);
            assert_eq!(sample.mask_rect, Some([expected, 0, 1, 1]));
            let mapping = sample.spatial.unwrap();
            assert_eq!(
                mapping.source_to_scene,
                [1.0, 0.0, expected as f64, 0.0, 1.0, 0.0]
            );
        }
        let sample = sampler
            .sample(
                Time::new(3600, 1).unwrap(),
                [2, 3],
                Overrides {
                    position: Some([9, 11]),
                    opacity: Some(17),
                },
            )
            .unwrap();
        assert_eq!(sample.position, [9, 11]);
        assert_eq!(sample.opacity, 17);
        assert_eq!(
            sample.spatial.unwrap().source_to_scene,
            [1.0, 0.0, 7.0, 0.0, 1.0, 8.0]
        );
        assert_eq!(sample.mask_rect, Some([120, 0, 1, 1]));
    }

    #[test]
    fn stream_blend_keeps_scene_coverage_until_final_rounding() {
        let transform = Transform {
            position: [0, 0],
            crop: [0, 0, 1, 1],
            scale: 1,
            quarter_turns: 0,
            opacity: 128,
            spatial: None,
        };
        let recipe = Recipe {
            transform: &transform,
            animation: None,
            mask: None,
            effects: &[],
            alpha_mode: AlphaMode::Straight,
            blend_mode: BlendMode::Normal,
        };
        let parameters = recipe
            .prepare(Time::new(1, 1).unwrap())
            .unwrap()
            .sample(Time::ZERO, [0, 0], Overrides::default())
            .unwrap();
        let image = Pixels {
            width: 1,
            height: 1,
            rgba: vec![128, 128, 128, 1],
        };
        let mut rgb = vec![0; 3];
        Frame {
            recipe,
            parameters: &parameters,
            image: &image,
            offset: [0, 0],
            anchor: [0, 0],
        }
        .draw(&mut rgb, [1, 1], None, false, 1);
        // round(128 * 1 * 128 / 255^2) = 0. Legacy byte-alpha rounding would produce 1.
        assert_eq!(rgb, [0, 0, 0]);
    }
}
