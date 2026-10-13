//! Frame compositing in the precise working representation. Scene and native-track
//! integration share these layer properties; this does not select a project render mode.
use super::Pixel;
use crate::{
    Result, clip_properties, composite, effects, error, layer_composite,
    scene::{Animation, Transform},
    time::Time,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Existing layer semantics applied to linear working pixels. Images and destination
/// buffers are supplied by the caller; no decoding, file publication or frame table is owned here.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Description {
    pub canvas: [u32; 2],
    pub transform: Transform,
    #[serde(default)]
    pub animation: Option<Animation>,
    #[serde(default)]
    pub mask: Option<composite::RectMask>,
    #[serde(default)]
    pub blend_mode: composite::BlendMode,
    #[serde(default)]
    pub effects: Vec<effects::Effect>,
}

/// A borrowed, already decoded straight-linear image positioned in the source canvas.
pub struct Image<'a> {
    pub size: [u32; 2],
    pub offset: [u32; 2],
    pub pixels: &'a [Pixel],
}

/// A validated layer and identity-bound pixel-effect resources.
pub struct Layer {
    description: Description,
    duration: Time,
    effects: effects::working::Effects,
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_EFFECT", message)
}

fn pixel_count([width, height]: [u32; 2]) -> Result<u64> {
    if width == 0 || height == 0 || width > 32768 || height > 32768 {
        return Err(invalid("Working image dimensions must be in 1..32768"));
    }
    Ok(u64::from(width) * u64::from(height))
}

impl Description {
    fn recipe(&self) -> layer_composite::Recipe<'_> {
        layer_composite::Recipe {
            transform: &self.transform,
            animation: self.animation.as_ref(),
            mask: self.mask.as_ref(),
            effects: &[],
            alpha_mode: composite::AlphaMode::Straight,
            blend_mode: self.blend_mode,
        }
    }
}

impl Layer {
    /// Validate source-canvas properties with the same contract as saved clip properties.
    pub fn new(description: Description, duration: Time, root: &Path) -> Result<Self> {
        let count = pixel_count(description.canvas)?;
        if count * 8 > 1 << 30 {
            return Err(invalid("Working canvas exceeds the 1 GiB buffer limit"));
        }
        let mut properties = vec![clip_properties::ClipEffect::Transform(Box::new(
            clip_properties::TransformProperty {
                value: description.transform.clone(),
                animation: description.animation.clone(),
                anchor: [0, 0],
                sampling: clip_properties::Sampling::Scene,
            },
        ))];
        if let Some(mask) = &description.mask {
            properties.push(clip_properties::ClipEffect::Mask { mask: mask.clone() });
        }
        clip_properties::normalize(&properties, None, description.canvas, duration)?;
        let effects = effects::working::Effects::new(description.effects.clone(), duration, root)?;
        Ok(Self {
            description,
            duration,
            effects,
        })
    }

    /// Prepare property curves once for repeated frame draws. Borrowed resources keep LUTs shared.
    pub fn prepare(&self) -> Result<Prepared<'_>> {
        Ok(Prepared {
            layer: self,
            sampler: self.description.recipe().prepare(self.duration)?,
        })
    }

    pub fn identities(&self) -> impl Iterator<Item = &crate::scene::Identity> {
        self.effects.identities()
    }
}

pub struct Prepared<'a> {
    layer: &'a Layer,
    sampler: layer_composite::Sampler<'a>,
}

impl Prepared<'_> {
    /// Draw one exact source-clock sample onto a possibly transparent working canvas.
    /// Source masks and bilinear weights act on associated linear color. Opacity is sampled
    /// directly as UNORM16, and the filtered blend is rounded only when the destination is stored.
    pub fn draw(
        &self,
        time: Time,
        image: Image<'_>,
        anchor: [i32; 2],
        dimensions: [u32; 2],
        destination: &mut [Pixel],
    ) -> Result<()> {
        let description = &self.layer.description;
        let source_count = pixel_count(image.size)?;
        let dest_count = pixel_count(dimensions)?;
        if source_count != image.pixels.len() as u64 || dest_count != destination.len() as u64 {
            return Err(invalid(
                "Working image buffer length differs from its dimensions",
            ));
        }
        if (source_count + dest_count) * 8 > 1 << 30 {
            return Err(invalid(
                "Working source and destination exceed the 1 GiB buffer limit",
            ));
        }
        if (0..2).any(|i| {
            u64::from(image.offset[i]) + u64::from(image.size[i]) > u64::from(description.canvas[i])
        }) || anchor.iter().any(|v| !(-32768..=32768).contains(v))
        {
            return Err(invalid(
                "Working image and anchor must fit their source-canvas bounds",
            ));
        }
        let sample = self
            .sampler
            .sample(time, anchor, layer_composite::Overrides::default())?;
        let opacity = self.sampler.opacity_precise(time, None)?;
        let processor = self.layer.effects.frame(time)?;
        draw(
            description.recipe(),
            &sample,
            opacity,
            &image,
            anchor,
            dimensions,
            destination,
            Some(&processor),
        )
    }
}

/// Shared draw entry for prepared scene and timeline layers. Inputs have been validated
/// by their owner, and effects can borrow that owner's shared resource pool.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw(
    recipe: layer_composite::Recipe<'_>,
    sample: &layer_composite::Sample,
    opacity: u16,
    image: &Image<'_>,
    anchor: [i32; 2],
    dimensions: [u32; 2],
    destination: &mut [Pixel],
    processor: Option<&effects::working::Frame<'_>>,
) -> Result<()> {
    if opacity == 0 {
        return Ok(());
    }
    let [cx, cy, cw, ch] = recipe.transform.crop.map(i64::from);
    let fetch = |sx: i64, sy: i64| -> Result<Option<(Pixel, u32)>> {
        if sx < cx || sy < cy || sx >= cx + cw || sy >= cy + ch {
            return Ok(None);
        }
        let coverage = recipe
            .mask
            .zip(sample.mask_rect)
            .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                mask.coverage(rect, sx, sy)
            });
        if coverage == 0 {
            return Ok(None);
        }
        let (x, y) = (
            sx - i64::from(image.offset[0]),
            sy - i64::from(image.offset[1]),
        );
        if x < 0 || y < 0 || x >= i64::from(image.size[0]) || y >= i64::from(image.size[1]) {
            return Ok(None);
        }
        let pixel = image.pixels[(y * i64::from(image.size[0]) + x) as usize];
        let pixel = processor.map_or(Ok(pixel), |p| p.pixel(pixel, [sx, sy]))?;
        Ok(Some((pixel, coverage)))
    };
    if let (Some(spec), Some(mapping)) = (&recipe.transform.spatial, &sample.spatial) {
        let [x0, y0, x1, y1] =
            crate::spatial::destination_bounds(mapping, spec, recipe.transform.crop, dimensions);
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(taps) =
                    crate::spatial::working_taps(mapping, spec, recipe.transform.crop, [x, y])
                else {
                    continue;
                };
                let mut total = Total::default();
                for (sx, sy, weight) in taps {
                    if weight == 0 {
                        continue;
                    }
                    if let Some((pixel, coverage)) = fetch(sx, sy)? {
                        total.add(pixel, coverage, weight);
                    }
                }
                let index = (y * i64::from(dimensions[0]) + x) as usize;
                destination[index] = total.over(destination[index], opacity, recipe.blend_mode)?;
            }
        }
        return Ok(());
    }
    let scale = i64::from(recipe.transform.scale);
    let (ax, ay) = (i64::from(anchor[0]) - cx, i64::from(anchor[1]) - cy);
    let turns = recipe.transform.quarter_turns;
    let (ax, ay) = match turns {
        0 => (ax, ay),
        1 => (ch - ay, ax),
        2 => (cw - ax, ch - ay),
        _ => (ay, cw - ax),
    };
    let left = i64::from(sample.position[0]) - ax * scale;
    let top = i64::from(sample.position[1]) - ay * scale;
    let (width, height) = if turns.is_multiple_of(2) {
        (cw * scale, ch * scale)
    } else {
        (ch * scale, cw * scale)
    };
    for y in top.max(0)..(top + height).min(i64::from(dimensions[1])) {
        for x in left.max(0)..(left + width).min(i64::from(dimensions[0])) {
            let (rx, ry) = ((x - left) / scale, (y - top) / scale);
            let (sx, sy) = match turns {
                0 => (rx, ry),
                1 => (ry, ch - 1 - rx),
                2 => (cw - 1 - rx, ch - 1 - ry),
                _ => (cw - 1 - ry, rx),
            };
            if let Some((pixel, coverage)) = fetch(sx + cx, sy + cy)? {
                let index = (y * i64::from(dimensions[0]) + x) as usize;
                destination[index] =
                    pixel.over(destination[index], opacity, coverage, recipe.blend_mode)?;
            }
        }
    }
    Ok(())
}

/// Exact associated tap products. Four bilinear weights sum to 2^32. Products fit
/// u128 (color at most 2^80); float conversion happens only for the final blend.
#[derive(Default)]
struct Total {
    color: [u128; 3],
    alpha: u128,
}
impl Total {
    fn add(&mut self, pixel: Pixel, mask: u32, weight: u64) {
        let alpha = u128::from(pixel.0[3]) * u128::from(mask) * u128::from(weight);
        self.alpha += alpha;
        for (sum, value) in self.color.iter_mut().zip(pixel.0) {
            *sum += u128::from(value) * alpha;
        }
    }
    fn over(&self, backdrop: Pixel, opacity: u16, mode: composite::BlendMode) -> Result<Pixel> {
        if self.alpha == 0 || opacity == 0 {
            return Ok(backdrop);
        }
        let source = self.color.map(|v| v as f64 / self.alpha as f64 / 65535.0);
        let coverage =
            self.alpha as f64 / ((1u64 << 48) as f64 * 65535.0) * f64::from(opacity) / 65535.0;
        let dest = backdrop.linear();
        let alpha = coverage + dest[3] * (1.0 - coverage);
        let mut out = [0.0; 4];
        for c in 0..3 {
            let blend = match mode {
                composite::BlendMode::Normal => source[c],
                composite::BlendMode::Multiply => source[c] * dest[c],
                composite::BlendMode::Screen => source[c] + dest[c] - source[c] * dest[c],
            };
            out[c] = ((1.0 - coverage) * dest[3] * dest[c]
                + coverage * (1.0 - dest[3]) * source[c]
                + coverage * dest[3] * blend)
                / alpha;
        }
        out[3] = alpha;
        Pixel::from_linear(out)
    }
}
