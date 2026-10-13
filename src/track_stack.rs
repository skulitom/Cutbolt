//! Ordered, bounded frame streams for native clips and recursively rendered child sequences.
use crate::{
    Result, clip_effects, error,
    media::{Control, Watch},
    scene::{Pixels, Transform},
    track_composite::{Ahead, Effect, PreparedReader},
    tracks::{OverlayTransform, TransitionKind},
};
use serde::Serialize;
mod working;
use std::{
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Input {
    Media {
        arguments: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        direct: Option<crate::direct::Picture>,
    },
    Generated {
        scene_id: String,
        recipe_sha256: String,
        first: u64,
        divisor: u32,
        #[serde(skip)]
        source: Arc<crate::generated::Source>,
    },
    Sequence {
        sequence_id: String,
        plan: Box<Plan>,
    },
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Clip {
    pub clip_id: String,
    pub first: u64,
    pub end: u64,
    pub alpha: bool,
    pub input: Input,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transform: Option<OverlayTransform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effects: Option<clip_effects::Stack>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Mix {
    pub incoming: usize,
    pub kind: TransitionKind,
    pub count: u64,
    /// Effect frame at the beginning of the segment.
    pub offset: u64,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Layer {
    pub track: usize,
    pub outgoing: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mix: Option<Mix>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Segment {
    pub first: u64,
    pub end: u64,
    /// Bottom to top, with provably hidden lower layers removed before opening decoders.
    pub layers: Vec<Layer>,
}
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Plan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working: Option<crate::color::working::Context>,
    /// Final byte output only. Recursive children return working pixels without encoding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_transfer: Option<crate::color::Transfer>,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub clips: Vec<Clip>,
    pub segments: Vec<Segment>,
    #[serde(skip)]
    pub resources: clip_effects::Resources,
    /// Conservative bound for active decoder queues and engine-owned frame buffers.
    pub frame_buffer_bytes: u64,
}
impl Plan {
    pub(crate) fn check_memory(&mut self) -> Result<()> {
        let plane = u64::from(self.width) * u64::from(self.height);
        // RGB backdrop, two transition views, output planes and one temporary RGBA image.
        let canvas = plane * if self.working.is_some() { 48 } else { 16 };
        let mut peak = 0;
        for segment in &self.segments {
            let mut active = 0;
            let mut proxy_canvas = 0;
            for index in segment
                .layers
                .iter()
                .flat_map(|l| std::iter::once(l.outgoing).chain(l.mix.iter().map(|m| m.incoming)))
            {
                let clip = &self.clips[index];
                if let Some(stack) = &clip.effects
                    && stack.properties.is_some()
                    && stack.coordinate_scale > 1
                {
                    proxy_canvas = proxy_canvas.max(
                        plane
                            * u64::from(stack.coordinate_scale).pow(2)
                            * if self.working.is_some() { 16 } else { 7 },
                    );
                }
                active += match &clip.input {
                    Input::Media { .. } => {
                        plane
                            * (if clip.alpha { 16 } else { 12 }
                                + if self.working.is_some() { 8 } else { 0 })
                    }
                    Input::Sequence { plan, .. } => plan.frame_buffer_bytes,
                    Input::Generated { source, .. } => source.frame_buffer_bytes(),
                };
            }
            peak = peak.max(canvas + active + proxy_canvas);
        }
        self.frame_buffer_bytes = peak;
        if peak > 1024 * 1024 * 1024 {
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "Active native layer frame buffers require at most {peak} bytes, above the 1 GiB bound; reduce simultaneous layers or canvas size"
                ),
            ));
        }
        Ok(())
    }
}

enum PreparedInput<'a> {
    Media(Box<PreparedReader>),
    Sequence(Box<Frames<'a>>),
    Generated {
        source: Arc<crate::generated::Source>,
        first: u64,
        divisor: u32,
    },
}
enum ActiveInput<'a> {
    Media(Ahead),
    Sequence(Box<Frames<'a>>),
    Generated {
        source: Arc<crate::generated::Source>,
        first: u64,
        divisor: u32,
    },
}
pub(crate) struct Frames<'a> {
    plan: &'a Plan,
    pending: Vec<Option<PreparedInput<'a>>>,
    active: Vec<Option<ActiveInput<'a>>>,
    effects: Vec<Option<clip_effects::Prepared>>,
    properties: Vec<Option<crate::layer_composite::Sampler<'a>>>,
    program: String,
    timeout: Duration,
    abort: Arc<AtomicBool>,
    cursor: u64,
    segment: usize,
}
impl<'a> Frames<'a> {
    /// Prepare every subprocess on the controlling thread, including recursive descendants.
    pub(crate) fn prepare(
        plan: &'a Plan,
        timeout: Duration,
        control: &dyn Control,
        abort: &Arc<AtomicBool>,
    ) -> Result<Self> {
        let program = control.tool("ffmpeg");
        let pending = plan
            .clips
            .iter()
            .map(|clip| {
                let input = match &clip.input {
                    Input::Media { arguments, direct } => PreparedInput::Media(Box::new(
                        PreparedReader::prepare(arguments, direct.as_ref(), &program, control)?,
                    )),
                    Input::Generated {
                        source,
                        first,
                        divisor,
                        ..
                    } => PreparedInput::Generated {
                        source: source.clone(),
                        first: *first,
                        divisor: *divisor,
                    },
                    Input::Sequence { plan, .. } => PreparedInput::Sequence(Box::new(
                        Self::prepare(plan, timeout, control, abort)?,
                    )),
                };
                Ok(Some(input))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            plan,
            pending,
            active: (0..plan.clips.len()).map(|_| None).collect(),
            effects: plan
                .clips
                .iter()
                .map(|c| c.effects.as_ref().map(|s| s.prepare()).transpose())
                .collect::<Result<_>>()?,
            properties: plan
                .clips
                .iter()
                .map(|c| {
                    c.effects
                        .as_ref()
                        .and_then(|s| s.properties.as_ref().map(|p| (s, p)))
                        .map(|(s, p)| p.recipe().prepare(s.source_duration))
                        .transpose()
                })
                .collect::<Result<_>>()?,
            program,
            timeout,
            abort: abort.clone(),
            cursor: 0,
            segment: 0,
        })
    }
    fn start(&mut self, index: usize, n: u64) -> Result<()> {
        let clip = &self.plan.clips[index];
        if self.active[index].is_none() {
            if n != clip.first {
                return Err(error(
                    "RENDER_VALIDATION_FAILED",
                    "Native layer decoder started at the wrong frame",
                ));
            }
            self.active[index] = Some(
                match self.pending[index].take().ok_or_else(|| {
                    error(
                        "RENDER_VALIDATION_FAILED",
                        "Native layer decoder was already consumed",
                    )
                })? {
                    PreparedInput::Media(prepared) => {
                        let reader = (*prepared).start(
                            &self.program,
                            self.timeout,
                            Watch {
                                abort: Some(self.abort.clone()),
                            },
                        )?;
                        ActiveInput::Media(Ahead::start(
                            reader,
                            self.plan.width as usize
                                * self.plan.height as usize
                                * if clip.alpha { 4 } else { 3 },
                            clip.end - clip.first,
                        ))
                    }
                    PreparedInput::Sequence(frames) => ActiveInput::Sequence(frames),
                    PreparedInput::Generated {
                        source,
                        first,
                        divisor,
                    } => ActiveInput::Generated {
                        source,
                        first,
                        divisor,
                    },
                },
            );
        }
        Ok(())
    }
    fn read(&mut self, index: usize, n: u64) -> Result<Vec<u8>> {
        self.start(index, n)?;
        let clip = &self.plan.clips[index];
        match self.active[index].as_mut().expect("started source") {
            ActiveInput::Media(reader) => reader.next(),
            ActiveInput::Sequence(frames) => frames.next(n - clip.first),
            ActiveInput::Generated {
                source,
                first,
                divisor,
            } => source.frame(*first + n - clip.first, *divisor),
        }
    }
    fn recycle(&mut self, index: usize, n: u64, bytes: Vec<u8>) -> Result<()> {
        if n + 1 == self.plan.clips[index].end {
            match self.active[index].take().expect("active source") {
                ActiveInput::Media(reader) => reader.finish(),
                ActiveInput::Sequence(frames) => frames.finish(),
                ActiveInput::Generated { .. } => Ok(()),
            }
        } else {
            if !bytes.is_empty()
                && let Some(ActiveInput::Media(reader)) = &self.active[index]
            {
                reader.recycle(bytes);
            }
            Ok(())
        }
    }
    fn draw(&mut self, index: usize, n: u64, rgb: &mut [u8]) -> Result<()> {
        let clip = &self.plan.clips[index];
        let planar = self.read(index, n)?;
        let plane = self.plan.width as usize * self.plan.height as usize;
        if planar.len() != plane * if clip.alpha { 4 } else { 3 } {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Native layer source returned the wrong frame size",
            ));
        }
        let rgba = (0..plane)
            .flat_map(|i| {
                [
                    planar[plane * 2 + i],
                    planar[i],
                    planar[plane + i],
                    if clip.alpha {
                        planar[plane * 3 + i]
                    } else {
                        255
                    },
                ]
            })
            .collect();
        let mut image = Pixels {
            width: self.plan.width,
            height: self.plan.height,
            rgba,
        };
        if let (Some(stack), Some(prepared)) = (&clip.effects, &self.effects[index]) {
            prepared.apply_rgba(
                stack,
                &self.plan.resources,
                n - clip.first,
                [image.width, image.height],
                &mut image.rgba,
            )?;
        }
        if let (Some(stack), Some(sampler)) = (&clip.effects, &self.properties[index]) {
            stack
                .properties
                .as_ref()
                .expect("prepared properties")
                .draw(
                    sampler,
                    stack.time(n - clip.first)?,
                    &mut image,
                    rgb,
                    stack.coordinate_scale,
                )?;
        } else {
            let transform = Transform {
                position: [0, 0],
                crop: [0, 0, image.width, image.height],
                scale: 1,
                quarter_turns: 0,
                opacity: 255,
                spatial: None,
            };
            let parameters = crate::layer_composite::Sample {
                position: [0, 0],
                opacity: 255,
                mask_rect: None,
                effects: vec![],
                spatial: None,
            };
            crate::layer_composite::Frame {
                recipe: crate::layer_composite::Recipe {
                    transform: &transform,
                    animation: None,
                    mask: None,
                    effects: &[],
                    alpha_mode: crate::composite::AlphaMode::Straight,
                    blend_mode: crate::composite::BlendMode::Normal,
                },
                parameters: &parameters,
                image: &image,
                offset: [0, 0],
                anchor: [0, 0],
            }
            .draw(rgb, [self.plan.width, self.plan.height], None, false, 1);
        }
        self.recycle(index, n, planar)
    }
    pub(crate) fn next(&mut self, n: u64) -> Result<Vec<u8>> {
        if let Some(context) = self.plan.working {
            let pixels = self.next_working(n)?;
            let plane = pixels.len();
            let mut planar = vec![0; plane * 3];
            for (i, pixel) in pixels.into_iter().enumerate() {
                let rgba = context.encoded8_as(
                    pixel,
                    self.plan.delivery_transfer.unwrap_or(context.transfer),
                    [i as u32 % self.plan.width, i as u32 / self.plan.width],
                    crate::color::working::Dither::Ordered8x8,
                );
                planar[i] = rgba[1];
                planar[plane + i] = rgba[2];
                planar[2 * plane + i] = rgba[0];
            }
            return Ok(planar);
        }
        if n != self.cursor || n >= self.plan.frames {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Native layer frames must be consumed once, in order",
            ));
        }
        while self.plan.segments[self.segment].end <= n {
            self.segment += 1;
        }
        let segment = &self.plan.segments[self.segment];
        // Preserve the opaque full-frame fast path: no RGB shuffle, temporary image or base decoder.
        if segment.layers.len() == 1 && segment.layers[0].mix.is_none() {
            let index = segment.layers[0].outgoing;
            let clip = &self.plan.clips[index];
            if !clip.alpha && clip.effects.is_none() && clip.transform.is_none() {
                let bytes = self.read(index, n)?;
                self.recycle(index, n, Vec::new())?;
                self.cursor += 1;
                return Ok(bytes);
            }
        }
        let plane = self.plan.width as usize * self.plan.height as usize;
        let mut rgb = vec![0; plane * 3];
        let first = segment.first;
        for layer in &segment.layers {
            if let Some(mix) = &layer.mix {
                // Both transition endpoints see the same lower-track backdrop; then their
                // complete views use the existing exact frame-center transition equations.
                let mut incoming = rgb.clone();
                self.draw(layer.outgoing, n, &mut rgb)?;
                self.draw(mix.incoming, n, &mut incoming)?;
                Effect::new(mix.kind, self.plan.width, mix.count, mix.offset + n - first)
                    .apply_packed(&mut rgb, &incoming, self.plan.width as usize);
            } else {
                self.draw(layer.outgoing, n, &mut rgb)?;
            }
        }
        let mut planar = vec![0; plane * 3];
        for (i, pixel) in rgb.as_chunks::<3>().0.iter().enumerate() {
            planar[i] = pixel[1];
            planar[plane + i] = pixel[2];
            planar[plane * 2 + i] = pixel[0];
        }
        self.cursor += 1;
        Ok(planar)
    }
    pub(crate) fn finish(self) -> Result<()> {
        if self.cursor != self.plan.frames
            || self.active.iter().any(Option::is_some)
            || self.pending.iter().any(Option::is_some)
        {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Native layer stream ended before all planned frames",
            ));
        }
        Ok(())
    }
}

/// Legacy crop/shrink/placement, preserving byte-alpha opacity rounding. A keyed opaque source
/// uses associated-color box averaging; an unchanged opaque source keeps floor-of-channel-mean.
#[cfg(test)]
fn legacy_over(rgb: &mut [u8], size: [u32; 2], image: &Pixels, transform: &OverlayTransform) {
    let [cx, cy, cw, ch] = transform.crop.unwrap_or([0, 0, image.width, image.height]);
    let k = transform.divisor as i64;
    let [x, y] = transform.position.map(i64::from);
    let (right, bottom) = (
        (x + i64::from(cw) / k).min(i64::from(size[0])),
        (y + i64::from(ch) / k).min(i64::from(size[1])),
    );
    for dy in y.max(0)..bottom {
        for dx in x.max(0)..right {
            let mut sum = [0u64; 4];
            for row in 0..k {
                for col in 0..k {
                    let sx = i64::from(cx) + (dx - x) * k + col;
                    let sy = i64::from(cy) + (dy - y) * k + row;
                    let p = &image.rgba[((sy * i64::from(image.width) + sx) * 4) as usize..][..4];
                    for c in 0..3 {
                        sum[c] += u64::from(p[c]) * u64::from(p[3]);
                    }
                    sum[3] += u64::from(p[3]);
                }
            }
            let alpha = sum[3] / (k * k) as u64;
            let alpha = (alpha * u64::from(transform.opacity) + 127) / 255;
            if alpha == 0 {
                continue;
            }
            let dest = ((dy * i64::from(size[0]) + dx) * 3) as usize;
            for c in 0..3 {
                let value = sum[c] / sum[3];
                rgb[dest + c] =
                    ((value * alpha + u64::from(rgb[dest + c]) * (255 - alpha) + 127) / 255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{clip_properties, time::Time};
    #[test]
    fn overlay_sugar_preserves_legacy_alpha_and_box_rounding() {
        let size = [24, 16];
        let mut image = Pixels {
            width: size[0],
            height: size[1],
            rgba: (0..24 * 16)
                .flat_map(|i| {
                    [
                        (i * 17 % 256) as u8,
                        (i * 29 % 256) as u8,
                        (i * 37 % 256) as u8,
                        (i * 41 % 256) as u8,
                    ]
                })
                .collect(),
        };
        let duration = Time::new(1, 1).unwrap();
        for divisor in 1..=8 {
            for opacity in [0, 1, 63, 127, 128, 254, 255] {
                let old = OverlayTransform {
                    crop: Some([2, 1, divisor * 2, divisor]),
                    divisor,
                    opacity,
                    position: [-1, 3],
                };
                let mut reference = vec![79; 24 * 16 * 3];
                legacy_over(&mut reference, size, &image, &old);
                let sugar = clip_properties::legacy(&old, size);
                for p in [
                    clip_properties::normalize(&[], Some(&old), size, duration)
                        .unwrap()
                        .unwrap(),
                    clip_properties::normalize(&sugar, None, size, duration)
                        .unwrap()
                        .unwrap(),
                ] {
                    let mut actual = vec![79; reference.len()];
                    let sampler = p.recipe().prepare(duration).unwrap();
                    p.draw(&sampler, Time::ZERO, &mut image, &mut actual, 1)
                        .unwrap();
                    assert_eq!(actual, reference, "divisor {divisor}, opacity {opacity}");
                }
            }
        }
    }
}
