//! Saved structural properties in explicit working storage. Legacy byte-alpha remains an
//! authored quantization policy; ordinary opacity never takes that path implicitly.
use super::{OpacityRounding, Properties, Sampling};
use crate::{
    Result,
    color::working::{Context, Depth, Pixel, layer::Image},
    layer_composite,
    time::Time,
};

fn byte_alpha(alpha: u16, opacity: u16) -> u16 {
    let numerator = u64::from(alpha) * u64::from(opacity) * 255;
    let denominator = 65535u64.pow(2);
    (((numerator + denominator / 2) / denominator) * 257) as u16
}
impl Properties {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw_working(
        &self,
        sampler: &layer_composite::Sampler<'_>,
        time: Time,
        image: &mut [Pixel],
        destination: &mut [Pixel],
        dimensions: [u32; 2],
        scale: u32,
        context: Context,
    ) -> Result<()> {
        let parameters =
            sampler.sample(time, self.anchor, layer_composite::Overrides::default())?;
        let opacity = context.quantum(sampler.opacity_precise(time, None)?);
        if scale == 1 {
            return self.draw_working_original(
                image,
                destination,
                dimensions,
                &parameters,
                opacity,
                context,
            );
        }
        let [width, height] = dimensions.map(|v| v * scale);
        let mut full = vec![Pixel::TRANSPARENT; (width * height) as usize];
        let mut backdrop = vec![Pixel::TRANSPARENT; full.len()];
        for y in 0..height {
            for x in 0..width {
                let source = ((y / scale) * dimensions[0] + x / scale) as usize;
                let target = (y * width + x) as usize;
                full[target] = image[source];
                backdrop[target] = destination[source];
            }
        }
        self.draw_working_original(
            &mut full,
            &mut backdrop,
            [width, height],
            &parameters,
            opacity,
            context,
        )?;
        for y in 0..dimensions[1] {
            for x in 0..dimensions[0] {
                destination[(y * dimensions[0] + x) as usize] =
                    backdrop[((y * scale + scale / 2) * width + x * scale + scale / 2) as usize];
            }
        }
        Ok(())
    }
    fn draw_working_original(
        &self,
        image: &mut [Pixel],
        destination: &mut [Pixel],
        dimensions: [u32; 2],
        parameters: &layer_composite::Sample,
        mut opacity: u16,
        context: Context,
    ) -> Result<()> {
        if let Sampling::BoxFloor { divisor } = self.sampling {
            return self.draw_working_box(
                image,
                destination,
                dimensions,
                parameters,
                opacity,
                divisor,
                context,
            );
        }
        if self.rounding == OpacityRounding::ByteAlpha {
            for pixel in image.iter_mut() {
                pixel.0[3] = byte_alpha(pixel.0[3], opacity);
            }
            opacity = u16::MAX;
        }
        crate::color::working::layer::draw_in(
            context,
            self.recipe(),
            parameters,
            opacity,
            &Image {
                size: dimensions,
                offset: [0, 0],
                pixels: image,
            },
            self.anchor,
            dimensions,
            destination,
            None,
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn draw_working_box(
        &self,
        image: &[Pixel],
        destination: &mut [Pixel],
        dimensions: [u32; 2],
        parameters: &layer_composite::Sample,
        opacity: u16,
        divisor: u32,
        context: Context,
    ) -> Result<()> {
        let [cx, cy, cw, ch] = self.transform.crop;
        let k = i64::from(divisor);
        let [x, y] = parameters.position.map(i64::from);
        let unit = u64::from(crate::composite::MASK_WEIGHT);
        let quantum = match context.space.bits {
            Depth::Eight => 257,
            Depth::Sixteen => 1,
        };
        for dy in y.max(0)..(y + i64::from(ch) / k).min(i64::from(dimensions[1])) {
            for dx in x.max(0)..(x + i64::from(cw) / k).min(i64::from(dimensions[0])) {
                let mut sum = [0u64; 4];
                for row in 0..k {
                    for col in 0..k {
                        let sx = i64::from(cx) + (dx - x) * k + col;
                        let sy = i64::from(cy) + (dy - y) * k + row;
                        let pixel = image[(sy * i64::from(dimensions[0]) + sx) as usize];
                        let mask = self
                            .mask
                            .as_ref()
                            .zip(parameters.mask_rect)
                            .map_or(unit, |(m, r)| u64::from(m.coverage(r, sx, sy)));
                        let alpha = u64::from(pixel.0[3]) * mask;
                        for (sum, c) in sum[..3].iter_mut().zip(pixel.0) {
                            *sum += u64::from(c) * alpha;
                        }
                        sum[3] += alpha;
                    }
                }
                if sum[3] == 0 {
                    continue;
                }
                let mut pixel = Pixel([0; 4]);
                for c in 0..3 {
                    pixel.0[c] = (sum[c] / (sum[3] * quantum) * quantum) as u16;
                }
                pixel.0[3] = (sum[3] / ((k * k) as u64 * unit * quantum) * quantum) as u16;
                let opacity = if self.rounding == OpacityRounding::ByteAlpha {
                    pixel.0[3] = byte_alpha(pixel.0[3], opacity);
                    u16::MAX
                } else {
                    opacity
                };
                let index = (dy * i64::from(dimensions[0]) + dx) as usize;
                destination[index] = context.over(
                    pixel,
                    destination[index],
                    opacity,
                    crate::composite::MASK_WEIGHT,
                    self.blend,
                )?;
            }
        }
        Ok(())
    }
}
