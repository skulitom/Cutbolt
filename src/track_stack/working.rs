//! Working frames cross recursive sequence/generated boundaries without delivery conversion.
use super::{ActiveInput, Frames};
use crate::{
    Result,
    color::working::{Context, Pixel},
    composite::{AlphaMode, BlendMode, MASK_WEIGHT},
    error,
    tracks::TransitionKind,
};

impl Frames<'_> {
    fn read_working(
        &mut self,
        index: usize,
        n: u64,
        context: Context,
    ) -> Result<(Vec<Pixel>, Vec<u8>)> {
        self.start(index, n)?;
        let clip = &self.plan.clips[index];
        let (pixels, bytes) = match self.active[index].as_mut().expect("started source") {
            ActiveInput::Media(reader) => {
                let bytes = reader.next()?;
                let plane = (self.plan.width * self.plan.height) as usize;
                if bytes.len() != plane * if clip.alpha { 4 } else { 3 } {
                    return Err(error(
                        "RENDER_VALIDATION_FAILED",
                        "Working source returned the wrong frame size",
                    ));
                }
                let pixels = (0..plane)
                    .map(|i| {
                        context.decode8(
                            [
                                bytes[2 * plane + i],
                                bytes[i],
                                bytes[plane + i],
                                if clip.alpha {
                                    bytes[3 * plane + i]
                                } else {
                                    255
                                },
                            ],
                            context.transfer,
                            AlphaMode::Straight,
                        )
                    })
                    .collect::<Result<_>>()?;
                (pixels, bytes)
            }
            ActiveInput::Sequence(frames) => {
                if frames.plan.working != Some(context) {
                    return Err(error(
                        "INVALID_COLOR",
                        "Nested sequence and parent working contexts differ",
                    ));
                }
                (frames.next_working(n - clip.first)?, Vec::new())
            }
            ActiveInput::Generated {
                source,
                first,
                divisor,
            } => (
                source.working_frame(*first + n - clip.first, *divisor, context)?,
                Vec::new(),
            ),
        };
        if pixels.len() != (self.plan.width * self.plan.height) as usize {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Working child returned the wrong frame size",
            ));
        }
        Ok((pixels, bytes))
    }
    fn draw_working(
        &mut self,
        index: usize,
        n: u64,
        destination: &mut [Pixel],
        context: Context,
    ) -> Result<()> {
        let (mut pixels, bytes) = self.read_working(index, n, context)?;
        let clip = &self.plan.clips[index];
        let dimensions = [self.plan.width, self.plan.height];
        if let (Some(stack), Some(prepared)) = (&clip.effects, &self.effects[index]) {
            prepared.apply_working(
                stack,
                &self.plan.resources,
                n - clip.first,
                dimensions,
                &mut pixels,
                context,
            )?;
        }
        if let (Some(stack), Some(sampler)) = (&clip.effects, &self.properties[index]) {
            stack
                .properties
                .as_ref()
                .expect("prepared properties")
                .draw_working(
                    sampler,
                    stack.time(n - clip.first)?,
                    &mut pixels,
                    destination,
                    dimensions,
                    stack.coordinate_scale,
                    context,
                )?;
        } else {
            for (dest, source) in destination.iter_mut().zip(pixels) {
                *dest = context.over(source, *dest, u16::MAX, MASK_WEIGHT, BlendMode::Normal)?;
            }
        }
        self.recycle(index, n, bytes)
    }
    pub(super) fn next_working(&mut self, n: u64) -> Result<Vec<Pixel>> {
        let context = self.plan.working.ok_or_else(|| {
            error(
                "INVALID_COLOR",
                "A byte sequence cannot supply working pixels",
            )
        })?;
        if n != self.cursor || n >= self.plan.frames {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Working frames must be consumed once, in order",
            ));
        }
        while self.plan.segments[self.segment].end <= n {
            self.segment += 1;
        }
        let segment = &self.plan.segments[self.segment];
        if segment.layers.len() == 1 && segment.layers[0].mix.is_none() {
            let index = segment.layers[0].outgoing;
            let clip = &self.plan.clips[index];
            if !clip.alpha && clip.effects.is_none() && clip.transform.is_none() {
                let (pixels, bytes) = self.read_working(index, n, context)?;
                self.recycle(index, n, bytes)?;
                self.cursor += 1;
                return Ok(pixels);
            }
        }
        let mut pixels = vec![Pixel::BLACK; (self.plan.width * self.plan.height) as usize];
        let first = segment.first;
        for layer in &segment.layers {
            if let Some(mix) = &layer.mix {
                let mut incoming = pixels.clone();
                self.draw_working(layer.outgoing, n, &mut pixels, context)?;
                self.draw_working(mix.incoming, n, &mut incoming, context)?;
                transition(
                    context,
                    mix.kind,
                    mix.count,
                    mix.offset + n - first,
                    self.plan.width,
                    &mut pixels,
                    &incoming,
                );
            } else {
                self.draw_working(layer.outgoing, n, &mut pixels, context)?;
            }
        }
        self.cursor += 1;
        Ok(pixels)
    }
}
/// Frame-center weights remain exact through the final store. Complete endpoint views are
/// opaque, so their component means are also their associated means. Wipes select whole pixels.
fn transition(
    context: Context,
    kind: TransitionKind,
    count: u64,
    index: u64,
    width: u32,
    left: &mut [Pixel],
    right: &[Pixel],
) {
    let n = u128::from(count);
    let k = 2 * u128::from(index) + 1;
    let d = 2 * n;
    for (i, (a, b)) in left.iter_mut().zip(right).enumerate() {
        let x = (i % width as usize) as u128;
        let weights = match kind {
            TransitionKind::Dissolve => Some((d - k, k)),
            TransitionKind::DipBlack => Some((d.saturating_sub(2 * k), (2 * k).saturating_sub(d))),
            TransitionKind::WipeLeft => {
                if (2 * x + 1) * n <= u128::from(width) * k {
                    *a = *b;
                }
                None
            }
            TransitionKind::WipeRight => {
                if (2 * (u128::from(width) - x) - 1) * n <= u128::from(width) * k {
                    *a = *b;
                }
                None
            }
        };
        if let Some((l, r)) = weights {
            for c in 0..3 {
                a.0[c] = context
                    .quantum(((u128::from(a.0[c]) * l + u128::from(b.0[c]) * r + n) / d) as u16);
            }
            a.0[3] = u16::MAX;
        }
    }
}
