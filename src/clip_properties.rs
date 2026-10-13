//! Saved native clip properties, normalized once into the shared scene layer contract.
use crate::{
    At, Result,
    animation::Curve,
    composite::{AlphaMode, BlendMode, RectMask},
    effects, error, layer_composite, scene,
    time::Time,
    tracks::OverlayTransform,
};
use serde::{Deserialize, Serialize};

/// Source sampling for a clip transform. `scene` uses the shared integer/spatial mapping.
/// `box_floor` preserves legacy crop/box-shrink placement and averages associated color.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "filter", rename_all = "snake_case", deny_unknown_fields)]
pub enum Sampling {
    /// Shared scene integer or filtered spatial mapping.
    #[default]
    Scene,
    /// Associated-color block mean with floor rounding, before opacity and blend.
    BoxFloor {
        /// Box width/height, 1..8; crop size must be divisible. No rotation, scale or spatial mapping.
        divisor: u32,
    },
}
/// Layer coverage normally rounds only at the final blend. `byte_alpha` first rounds
/// alpha times opacity to a byte, preserving legacy overlay recipes exactly.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OpacityRounding {
    #[default]
    Coverage,
    ByteAlpha,
}

/// Shared scene transform plus source-clock animation and a source-canvas anchor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TransformProperty {
    /// Scene integer transform and optional interpolated spatial mapping, in original canvas pixels.
    pub value: scene::Transform,
    /// Optional position/opacity curves on the clip's source clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<scene::Animation>,
    /// Source-canvas pivot placed at value.position; default [0,0], each -32768..32768.
    #[serde(default)]
    pub anchor: [i32; 2],
    /// Scene sampling by default; explicit box_floor is the exact legacy overlay shrink.
    #[serde(default)]
    pub sampling: Sampling,
}
/// Clip opacity, evaluated on the source clock. Declaring this requires transform opacity
/// to remain 255 with no transform opacity curve, so the property has one owner.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OpacityProperty {
    /// Static opacity, 0..255.
    pub value: u8,
    /// Optional 0..255 curve replacing value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub curve: Option<Curve>,
    /// Coverage rounds at final blend by default; byte_alpha preserves legacy overlay rounding.
    #[serde(default)]
    pub rounding: OpacityRounding,
}
/// Saved pixel effects retain their list order. Structural properties have one declaration
/// each and fixed source-mask, transform, opacity and blend phases, independent of list position.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipEffect {
    /// Shared primary grade; preserves alpha.
    Grade(effects::Grade),
    /// Shared selective grade; preserves alpha.
    SelectiveGrade(effects::SelectiveGrade),
    /// Shared source-clock key; retains coverage over lower tracks.
    ChromaKey(crate::keying::ChromaKey),
    /// Identity-bound encoded RGB table at this point in the ordered pixel chain.
    Lut(crate::lut::Transform),
    /// One source-canvas transform, with optional source-clock animation.
    Transform(Box<TransformProperty>),
    /// One source-clock opacity property.
    Opacity(OpacityProperty),
    /// One source-canvas rectangle mask, including feather and source-clock animation.
    Mask {
        /// Source-canvas rectangle coverage and optional source-clock animation.
        mask: RectMask,
    },
    /// One encoded RGB blend mode over lower active tracks.
    BlendMode {
        /// Encoded RGB operation over lower active tracks.
        mode: BlendMode,
    },
}
impl ClipEffect {
    pub(crate) fn pixel(&self) -> Option<effects::Effect> {
        Some(match self {
            Self::Grade(v) => effects::Effect::Grade(v.clone()),
            Self::SelectiveGrade(v) => effects::Effect::SelectiveGrade(v.clone()),
            Self::ChromaKey(v) => effects::Effect::ChromaKey(v.clone()),
            Self::Lut(v) => effects::Effect::Lut(v.clone()),
            _ => return None,
        })
    }
    pub(crate) fn structural(&self) -> bool {
        self.pixel_kind().is_none()
    }
    fn pixel_kind(&self) -> Option<()> {
        matches!(
            self,
            Self::Grade(_) | Self::SelectiveGrade(_) | Self::ChromaKey(_) | Self::Lut(_)
        )
        .then_some(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Properties {
    pub transform: scene::Transform,
    pub animation: Option<scene::Animation>,
    pub anchor: [i32; 2],
    pub sampling: Sampling,
    pub rounding: OpacityRounding,
    pub mask: Option<RectMask>,
    pub blend: BlendMode,
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_EFFECT", message)
}
fn identity(canvas: [u32; 2]) -> scene::Transform {
    scene::Transform {
        position: [0, 0],
        crop: [0, 0, canvas[0], canvas[1]],
        scale: 1,
        quarter_turns: 0,
        opacity: 255,
        spatial: None,
    }
}
/// Normalize legacy placement to explicit transform/opacity properties. This same recipe
/// is used for old OverlayTransform and its new saved-effect equivalent.
pub(crate) fn legacy(value: &OverlayTransform, canvas: [u32; 2]) -> [ClipEffect; 2] {
    let crop = value.crop.unwrap_or([0, 0, canvas[0], canvas[1]]);
    [
        ClipEffect::Transform(Box::new(TransformProperty {
            value: scene::Transform {
                position: value.position,
                crop,
                ..identity(canvas)
            },
            animation: None,
            anchor: [crop[0] as i32, crop[1] as i32],
            sampling: Sampling::BoxFloor {
                divisor: value.divisor,
            },
        })),
        ClipEffect::Opacity(OpacityProperty {
            value: value.opacity,
            curve: None,
            rounding: OpacityRounding::ByteAlpha,
        }),
    ]
}
pub(crate) fn normalize(
    effects: &[ClipEffect],
    overlay: Option<&OverlayTransform>,
    canvas: [u32; 2],
    duration: Time,
) -> Result<Option<Properties>> {
    if effects.len() > 8 {
        return Err(invalid(
            "Use at most eight clip effects and structural properties",
        ));
    }
    let mut properties = Properties {
        transform: identity(canvas),
        animation: None,
        anchor: [0, 0],
        sampling: Sampling::Scene,
        rounding: OpacityRounding::Coverage,
        mask: None,
        blend: BlendMode::Normal,
    };
    let mut found = [false; 4];
    let mut opacity = None;
    let sugar = overlay.map(|o| legacy(o, canvas));
    for effect in effects.iter().chain(sugar.iter().flatten()) {
        let slot = match effect {
            ClipEffect::Transform(_) => 0,
            ClipEffect::Opacity(_) => 1,
            ClipEffect::Mask { .. } => 2,
            ClipEffect::BlendMode { .. } => 3,
            _ => continue,
        };
        if std::mem::replace(&mut found[slot], true) {
            return Err(invalid(
                "Each structural property may appear once; legacy overlay transform already owns transform and opacity",
            ));
        }
        match effect {
            ClipEffect::Transform(t) => {
                properties.transform = t.value.clone();
                properties.animation = t.animation.clone();
                properties.anchor = t.anchor;
                properties.sampling = t.sampling.clone();
            }
            ClipEffect::Opacity(v) => opacity = Some(v),
            ClipEffect::Mask { mask } => properties.mask = Some(mask.clone()),
            ClipEffect::BlendMode { mode } => properties.blend = *mode,
            _ => unreachable!(),
        }
    }
    if let Some(value) = opacity {
        if properties.transform.opacity != 255
            || properties
                .animation
                .as_ref()
                .is_some_and(|a| a.opacity.is_some())
        {
            return Err(invalid(
                "An opacity property requires transform opacity 255 and no transform opacity curve",
            ));
        }
        properties.transform.opacity = value.value;
        properties.rounding = value.rounding;
        if let Some(curve) = &value.curve {
            properties
                .animation
                .get_or_insert(scene::Animation {
                    position_x: None,
                    position_y: None,
                    opacity: None,
                })
                .opacity = Some(curve.clone());
        }
    }
    let t = &properties.transform;
    let [x, y, w, h] = t.crop;
    if w == 0
        || h == 0
        || u64::from(x) + u64::from(w) > u64::from(canvas[0])
        || u64::from(y) + u64::from(h) > u64::from(canvas[1])
        || !(1..=16).contains(&t.scale)
        || t.quarter_turns > 3
        || t.position
            .iter()
            .chain(&properties.anchor)
            .any(|v| !(-32768..=32768).contains(v))
    {
        return Err(invalid(
            "Clip transform crop must fit the original source canvas; scale 1..16, quarter turns 0..3, position/anchor -32768..32768",
        ));
    }
    if let Sampling::BoxFloor { divisor } = properties.sampling {
        let d = divisor;
        if !(1..=8).contains(&divisor)
            || !w.is_multiple_of(d)
            || !h.is_multiple_of(d)
            || t.scale != 1
            || t.quarter_turns != 0
            || t.spatial.is_some()
            || properties.anchor != [x as i32, y as i32]
        {
            return Err(invalid(
                "box_floor requires divisor 1..8, divisible crop size, scale 1, no rotation/spatial mapping, and an anchor at the crop origin",
            ));
        }
    }
    properties
        .recipe()
        .prepare(duration)
        .under(|| "clip properties".into())?;
    Ok(found.contains(&true).then_some(properties))
}
impl Properties {
    /// Proxy pixels represent original-canvas blocks. Expand only this frame, draw with
    /// unchanged source coordinates, then retain output block centers (ties upward).
    pub(crate) fn draw(
        &self,
        sampler: &layer_composite::Sampler<'_>,
        time: Time,
        image: &mut scene::Pixels,
        rgb: &mut [u8],
        scale: u32,
    ) -> Result<()> {
        let parameters =
            sampler.sample(time, self.anchor, layer_composite::Overrides::default())?;
        if scale == 1 {
            self.draw_original(image, rgb, parameters);
            return Ok(());
        }
        let (width, height) = (image.width * scale, image.height * scale);
        let plane = width as usize * height as usize;
        let mut full = scene::Pixels {
            width,
            height,
            rgba: vec![0; plane * 4],
        };
        let mut backdrop = vec![0; plane * 3];
        for y in 0..height {
            for x in 0..width {
                let src = ((y / scale) * image.width + x / scale) as usize;
                let dst = (y * width + x) as usize;
                full.rgba[dst * 4..dst * 4 + 4].copy_from_slice(&image.rgba[src * 4..src * 4 + 4]);
                backdrop[dst * 3..dst * 3 + 3].copy_from_slice(&rgb[src * 3..src * 3 + 3]);
            }
        }
        self.draw_original(&mut full, &mut backdrop, parameters);
        for y in 0..image.height {
            for x in 0..image.width {
                let src = ((y * scale + scale / 2) * width + x * scale + scale / 2) as usize;
                let dst = (y * image.width + x) as usize;
                rgb[dst * 3..dst * 3 + 3].copy_from_slice(&backdrop[src * 3..src * 3 + 3]);
            }
        }
        Ok(())
    }
    fn draw_original(
        &self,
        image: &mut scene::Pixels,
        rgb: &mut [u8],
        mut parameters: layer_composite::Sample,
    ) {
        if let Sampling::BoxFloor { divisor } = self.sampling {
            self.draw_box(image, rgb, &parameters, divisor);
            return;
        }
        if self.rounding == OpacityRounding::ByteAlpha {
            for p in image.rgba.as_chunks_mut::<4>().0 {
                p[3] = ((u32::from(p[3]) * u32::from(parameters.opacity) + 127) / 255) as u8;
            }
            parameters.opacity = 255;
        }
        layer_composite::Frame {
            recipe: self.recipe(),
            parameters: &parameters,
            image,
            offset: [0, 0],
            anchor: self.anchor,
        }
        .draw(rgb, [image.width, image.height], None, false, 1);
    }
    /// Mask in source space, associated-color box average, then declared opacity rounding.
    fn draw_box(
        &self,
        image: &scene::Pixels,
        rgb: &mut [u8],
        sample: &layer_composite::Sample,
        divisor: u32,
    ) {
        let [cx, cy, cw, ch] = self.transform.crop;
        let k = i64::from(divisor);
        let [x, y] = sample.position.map(i64::from);
        let unit = u64::from(crate::composite::MASK_WEIGHT);
        for dy in y.max(0)..(y + i64::from(ch) / k).min(i64::from(image.height)) {
            for dx in x.max(0)..(x + i64::from(cw) / k).min(i64::from(image.width)) {
                let mut sum = [0u64; 4];
                for row in 0..k {
                    for col in 0..k {
                        let sx = i64::from(cx) + (dx - x) * k + col;
                        let sy = i64::from(cy) + (dy - y) * k + row;
                        let p =
                            &image.rgba[((sy * i64::from(image.width) + sx) * 4) as usize..][..4];
                        let mask = self
                            .mask
                            .as_ref()
                            .zip(sample.mask_rect)
                            .map_or(unit, |(mask, rect)| u64::from(mask.coverage(rect, sx, sy)));
                        let a = u64::from(p[3]) * mask;
                        for c in 0..3 {
                            sum[c] += u64::from(p[c]) * a;
                        }
                        sum[3] += a;
                    }
                }
                let mut alpha = (sum[3] / ((k * k) as u64 * unit)) as u8;
                let mut opacity = sample.opacity;
                if self.rounding == OpacityRounding::ByteAlpha {
                    alpha = ((u32::from(alpha) * u32::from(opacity) + 127) / 255) as u8;
                    opacity = 255;
                }
                if alpha == 0 {
                    continue;
                }
                let dst = ((dy * i64::from(image.width) + dx) * 3) as usize;
                for c in 0..3 {
                    rgb[dst + c] = crate::composite::channel(
                        rgb[dst + c],
                        (sum[c] / sum[3]) as u8,
                        alpha,
                        opacity,
                        self.blend,
                        AlphaMode::Straight,
                    );
                }
            }
        }
    }
    pub(crate) fn recipe(&self) -> layer_composite::Recipe<'_> {
        layer_composite::Recipe {
            transform: &self.transform,
            animation: self.animation.as_ref(),
            mask: self.mask.as_ref(),
            effects: &[],
            alpha_mode: AlphaMode::Straight,
            blend_mode: self.blend,
        }
    }
    pub(crate) fn opaque(&self, canvas: [u32; 2]) -> bool {
        self.animation.is_none()
            && self.mask.is_none()
            && self.blend.is_normal()
            && self.transform.spatial.is_none()
            && self.transform.opacity == 255
            && self.transform.scale == 1
            && self.transform.quarter_turns == 0
            && self.transform.position == [0, 0]
            && self.anchor == [0, 0]
            && self.transform.crop == [0, 0, canvas[0], canvas[1]]
            && matches!(
                self.sampling,
                Sampling::Scene | Sampling::BoxFloor { divisor: 1 }
            )
    }
}
