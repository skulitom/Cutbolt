//! Original bounded layout and shape coverage; fontdue supplies glyph rasterization.
mod unicode;
use crate::{
    Result, error,
    scene::{self, Identity},
};
use fontdue::{Font, FontSettings, Metrics};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};
pub use unicode::{BaseDirection, TextLayout};

/// Horizontal alignment of each line's advance width inside the text box: `left`, `center` or `right`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Align {
    Left,
    Center,
    Right,
}
/// Vertical placement of the lines inside the text box, each line taking `line_height`: `top` puts the first baseline at the box top plus `size`; `middle` centers the lines (rounding up); `bottom` puts the last line's slot at the box bottom, leaving `line_height - size` below its baseline.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VAlign {
    #[default]
    Top,
    Middle,
    Bottom,
}
impl VAlign {
    pub(crate) fn is_top(&self) -> bool {
        *self == Self::Top
    }
    /// First baseline for `lines` lines in `rect`.
    pub(crate) fn first_baseline(
        self,
        rect: [i32; 4],
        size: u16,
        line_height: u16,
        lines: usize,
    ) -> i32 {
        let block = lines as i32 * line_height as i32;
        let offset = match self {
            Self::Top => 0,
            Self::Middle => (rect[3] - block).div_euclid(2),
            Self::Bottom => rect[3] - block,
        };
        rect[1] + offset + size as i32
    }
}
/// Box behind each nonempty text line: the line's advance width by its `line_height` slot (from `baseline - size`), grown by `padding` on every side. Overlapping boxes are drawn once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextBackground {
    /// Straight-alpha RGBA box color as `[r, g, b, a]` bytes.
    pub color: [u8; 4],
    /// Pixels added on every side of each line box, 0..=256.
    pub padding: u16,
}
/// Outline around glyphs, drawn under the fill: the text's alpha dilated by a disc of radius `width`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextOutline {
    /// Straight-alpha RGBA outline color as `[r, g, b, a]` bytes.
    pub color: [u8; 4],
    /// Outline radius in pixels, 1..=8.
    pub width: u8,
}
/// Line breaking: `none` (LF only), `character` (before a scalar or grapheme that would overflow the box) or `word` (Unicode line-break opportunities; requires `layout`).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Wrap {
    None,
    Character,
    Word,
}
/// Text outside the box: `reject` fails with TEXT_OVERFLOW; `clip` discards and counts out-of-box coverage.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Overflow {
    Reject,
    Clip,
}
/// Shape primitive: `rectangle` or `ellipse` (inscribed in the rectangle), hard-edged at pixel centers.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Rectangle,
    Ellipse,
}
/// Inside stroke painted over the fill between the shape and the shape inset by `width`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Stroke {
    /// Stroke width in pixels, 1..=256.
    pub width: u32,
    /// Straight-alpha RGBA color as `[r, g, b, a]` bytes.
    pub color: [u8; 4],
}
/// Generated layer source for scene `graphics`, tagged by `kind`; drawn on the layer canvas with straight alpha.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Graphic {
    /// Text laid out in a box with explicitly supplied external TrueType fonts.
    Text {
        /// Text with LF line breaks; 1..=1024 Unicode scalars and at most 4096 UTF-8 bytes.
        text: String,
        /// Ordered font identities under `input_root`, 1..=4; each scalar or grapheme uses the first font that has it.
        fonts: Vec<Identity>,
        /// Font em size in pixels, 1..=512; the first baseline is `rect` y plus size.
        size: u16,
        /// Straight-alpha RGBA text color as `[r, g, b, a]` bytes.
        color: [u8; 4],
        /// Text box `[x, y, width, height]` in canvas pixels, sizes 1..=4096; must lie wholly inside the layer canvas.
        rect: [i32; 4],
        /// Baseline spacing between lines in pixels, 1..=2048.
        line_height: u16,
        /// Extra pixels between scalars (or shaped clusters) on a line, 0..=128.
        letter_spacing: u16,
        /// Line alignment inside the box.
        align: Align,
        /// Vertical alignment of the lines inside the box; default `top`.
        #[serde(default, skip_serializing_if = "VAlign::is_top")]
        valign: VAlign,
        /// Optional box behind each line, as for captions over video; drawn under the outline and fill.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        background: Option<TextBackground>,
        /// Optional outline around the glyphs, drawn under the fill.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        outline: Option<TextOutline>,
        /// Line breaking mode.
        wrap: Wrap,
        /// Out-of-box policy.
        overflow: Overflow,
        /// Optional Unicode shaping/bidi layout; omit for the left-to-right scalar layout `ltr_scalar_v1`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        layout: Option<TextLayout>,
    },
    /// Filled rectangle or ellipse with an optional inside stroke.
    Shape {
        /// Primitive to draw.
        shape: Shape,
        /// Bounding rectangle `[x, y, width, height]` in canvas pixels; x/y -32768..=32768, sizes 1..=4096; clips to the canvas.
        rect: [i32; 4],
        /// Straight-alpha RGBA fill color as `[r, g, b, a]` bytes.
        fill: [u8; 4],
        /// Optional inside stroke; omit or null for none.
        stroke: Option<Stroke>,
    },
}
struct Loaded {
    identity: Identity,
    path: PathBuf,
    font: Font,
    bytes: Vec<u8>,
    shaping: Option<harfrust::ShaperData>,
}
#[derive(Default)]
pub(crate) struct Cache {
    fonts: HashMap<PathBuf, Loaded>,
    bytes: u64,
}
impl Cache {
    fn load(&mut self, identity: &Identity, root: &Path) -> Result<()> {
        if let Some(previous) = self.fonts.get(&identity.path) {
            if previous.identity.sha256 != identity.sha256
                || previous.identity.bytes != identity.bytes
            {
                return Err(error(
                    "INVALID_IDENTITY",
                    "Conflicting font identities for one path",
                ));
            }
            return Ok(());
        }
        if self.fonts.len() >= 8
            || identity.bytes > 8 * 1024 * 1024
            || self.bytes as u128 + identity.bytes as u128 > 32 * 1024 * 1024
        {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Fonts require at most 8 files, 8 MiB each and 32 MiB total",
            ));
        }
        let (path, bytes) = scene::identity_bytes(identity, root)?;
        // This profile deliberately accepts ordinary TrueType outline fonts only.
        if bytes.get(..4) != Some(&[0, 1, 0, 0]) {
            return Err(error(
                "UNSUPPORTED_FONT",
                "Expected a single TrueType outline font",
            ));
        }
        let font = Font::from_bytes(
            bytes.as_slice(),
            FontSettings {
                load_substitutions: false,
                ..FontSettings::default()
            },
        )
        .map_err(|e| error("UNSUPPORTED_FONT", e.to_string()))?;
        self.bytes += identity.bytes;
        self.fonts.insert(
            identity.path.clone(),
            Loaded {
                identity: identity.clone(),
                path,
                font,
                bytes,
                shaping: None,
            },
        );
        Ok(())
    }
    pub(crate) fn sources(&self) -> Vec<(PathBuf, Identity)> {
        let mut values: Vec<_> = self
            .fonts
            .values()
            .map(|f| (f.path.clone(), f.identity.clone()))
            .collect();
        values.sort_by(|a, b| a.0.cmp(&b.0));
        values
    }
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_GRAPHIC", message)
}
/// Glyph-space y of a rasterized bitmap's top edge. The rasterizer positions the outline with a
/// sub-pixel offset `fract(1 - fract(height) - fract(ymin))` (wrapped into 0..1) below the bitmap top,
/// while reporting `ymin` as a floor. When f32 scaling leaves bounds a hair from an integer, that offset
/// approaches 1 and `ymin + height` sits one row below where the outline was drawn. Recompute the same
/// offset with identical f32 operations and round the true top edge, so placement follows the drawing.
pub(crate) fn bitmap_top(m: &fontdue::Metrics) -> i32 {
    let fract = |v: f32| v - v.trunc();
    let mut offset = fract(1.0 - fract(m.bounds.height) - fract(m.bounds.ymin));
    if offset < 0.0 {
        offset += 1.0;
    }
    if m.height == 0 {
        return m.ymin + m.height as i32;
    }
    (m.bounds.ymin + m.bounds.height + offset).round() as i32
}
fn rect_valid(rect: [i32; 4]) -> bool {
    rect[..2].iter().all(|v| (-32768..=32768).contains(v))
        && rect[2..].iter().all(|v| (1..=4096).contains(v))
}
fn supported(c: char) -> bool {
    c == '\n'
        || matches!(c as u32,0x20..=0x7e|0xa0..=0x24f|0x370..=0x482|0x48a..=0x52f|0x2010..=0x2027|0x2030..=0x205e|0x3000..=0x3029|0x3030..=0x303f|0x3041..=0x3096|0x30a1..=0x30fa|0x3400..=0x9fff|0xac00..=0xd7a3)
}
fn inside(shape: &Shape, rect: [i32; 4], x: i32, y: i32) -> bool {
    let [left, top, w, h] = rect;
    if w <= 0 || h <= 0 || x < left || y < top || x >= left + w || y >= top + h {
        return false;
    }
    if matches!(shape, Shape::Rectangle) {
        return true;
    }
    let (w, h) = (w as i128, h as i128);
    let dx = 2 * (x - left) as i128 + 1 - w;
    let dy = 2 * (y - top) as i128 + 1 - h;
    dx * dx * h * h + dy * dy * w * w <= w * w * h * h
}
fn put(rgba: &mut [u8], width: u32, x: i32, y: i32, color: [u8; 4], coverage: u8) {
    let offset = (y as usize * width as usize + x as usize) * 4;
    let p = &mut rgba[offset..offset + 4];
    let alpha = (color[3] as u32 * coverage as u32 + 127) / 255;
    if alpha == 0 {
        return;
    }
    let a = alpha * 255 + p[3] as u32 * (255 - alpha);
    for c in 0..3 {
        p[c] = ((color[c] as u32 * alpha * 255 + p[c] as u32 * p[3] as u32 * (255 - alpha) + a / 2)
            / a) as u8;
    }
    p[3] = ((a + 127) / 255) as u8;
}
/// Composite a text graphic's background and outline under its already rendered fill. `text` is
/// the fill alone on a transparent canvas, and `spans` holds each line's `[x0, x1, baseline]`.
fn decorate(text: Vec<u8>, canvas: [u32; 2], graphic: &Graphic, spans: &[[i32; 3]]) -> Vec<u8> {
    let Graphic::Text {
        size,
        line_height,
        background,
        outline,
        ..
    } = graphic
    else {
        return text;
    };
    if background.is_none() && outline.is_none() {
        return text;
    }
    let (width, height) = (canvas[0] as i32, canvas[1] as i32);
    let at = |x: i32, y: i32| (y as usize * width as usize + x as usize) * 4;
    let mut out = vec![0; text.len()];
    if let Some(b) = background {
        let pad = b.padding as i32;
        let mut covered = vec![false; (width * height) as usize];
        for &[x0, x1, baseline] in spans.iter().filter(|s| s[1] > s[0]) {
            let top = baseline - *size as i32 - pad;
            let bottom = baseline - *size as i32 + *line_height as i32 + pad;
            for y in top.max(0)..bottom.min(height) {
                for x in (x0 - pad).max(0)..(x1 + pad).min(width) {
                    covered[(y * width + x) as usize] = true;
                }
            }
        }
        for (i, _) in covered.iter().enumerate().filter(|(_, c)| **c) {
            put(
                &mut out,
                canvas[0],
                i as i32 % width,
                i as i32 / width,
                b.color,
                255,
            );
        }
    }
    if let Some(o) = outline {
        let r = o.width as i32;
        let disc: Vec<(i32, i32)> = (-r..=r)
            .flat_map(|dy| (-r..=r).map(move |dx| (dx, dy)))
            .filter(|(dx, dy)| dx * dx + dy * dy <= r * r)
            .collect();
        let alpha = |x: i32, y: i32| text[at(x, y) + 3];
        let (mut left, mut top, mut right, mut bottom) = (width, height, -1, -1);
        for y in 0..height {
            for x in 0..width {
                if alpha(x, y) != 0 {
                    (left, top) = (left.min(x), top.min(y));
                    (right, bottom) = (right.max(x), bottom.max(y));
                }
            }
        }
        for y in (top - r).max(0)..=(bottom + r).min(height - 1) {
            for x in (left - r).max(0)..=(right + r).min(width - 1) {
                let coverage = disc
                    .iter()
                    .map(|(dx, dy)| (x + dx, y + dy))
                    .filter(|&(sx, sy)| sx >= 0 && sy >= 0 && sx < width && sy < height)
                    .map(|(sx, sy)| alpha(sx, sy))
                    .max()
                    .unwrap_or(0);
                if coverage != 0 {
                    put(&mut out, canvas[0], x, y, o.color, coverage);
                }
            }
        }
    }
    for y in 0..height {
        for x in 0..width {
            let p = at(x, y);
            if text[p + 3] != 0 {
                let color = [text[p], text[p + 1], text[p + 2], text[p + 3]];
                put(&mut out, canvas[0], x, y, color, 255);
            }
        }
    }
    out
}
struct Glyph {
    c: char,
    font: usize,
    metrics: Metrics,
    x: f32,
}

pub(crate) fn rasterize(
    graphic: &Graphic,
    canvas: [u32; 2],
    root: &Path,
    cache: &mut Cache,
) -> Result<(Vec<u8>, Value)> {
    let [width, height] = canvas;
    let mut rgba = vec![0; width as usize * height as usize * 4];
    match graphic {
        Graphic::Shape {
            shape,
            rect,
            fill,
            stroke,
        } => {
            if !rect_valid(*rect)
                || stroke
                    .as_ref()
                    .is_some_and(|s| s.width == 0 || s.width > 256)
            {
                return Err(invalid(
                    "Shape requires bounded positive rectangle and stroke width 1-256",
                ));
            }
            for y in 0..height as i32 {
                for x in 0..width as i32 {
                    if inside(shape, *rect, x, y) {
                        put(&mut rgba, width, x, y, *fill, 255);
                        if let Some(stroke) = stroke {
                            let w = stroke.width as i32;
                            let inner =
                                [rect[0] + w, rect[1] + w, rect[2] - 2 * w, rect[3] - 2 * w];
                            if !inside(shape, inner, x, y) {
                                put(&mut rgba, width, x, y, stroke.color, 255);
                            }
                        }
                    }
                }
            }
            Ok((
                rgba,
                json!({"kind":"shape","shape":shape,"rect":rect,"coverage":"pixel_center_hard_edge","stroke":"inside_over_fill"}),
            ))
        }
        Graphic::Text {
            text,
            fonts,
            size,
            color,
            rect,
            line_height,
            letter_spacing,
            align,
            valign,
            background,
            outline,
            wrap,
            overflow,
            layout,
        } => {
            if background.as_ref().is_some_and(|b| b.padding > 256)
                || outline
                    .as_ref()
                    .is_some_and(|o| !(1..=8).contains(&o.width))
            {
                return Err(error(
                    "INVALID_GRAPHIC",
                    "Text background padding must be 0..=256 and outline width 1..=8",
                ));
            }
            if text.is_empty()
                || text.len() > 4096
                || text.chars().count() > 1024
                || fonts.is_empty()
                || fonts.len() > 4
                || !(1..=crate::scene::MAX_TEXT_SIZE).contains(size)
                || !(1..=2048).contains(line_height)
                || *letter_spacing > 128
                || !rect_valid(*rect)
                || rect[0] < 0
                || rect[1] < 0
                || rect[0] as u64 + rect[2] as u64 > width as u64
                || rect[1] as u64 + rect[3] as u64 > height as u64
            {
                return Err(invalid(
                    "Text requires 1-1024 scalars/4096 bytes, 1-4 fonts, size 1-512, line height 1-2048, spacing 0-128 and a box inside the canvas",
                ));
            }
            if layout.is_some() {
                return unicode::rasterize(graphic, canvas, root, cache, rgba);
            }
            if matches!(wrap, Wrap::Word) {
                return Err(error(
                    "UNSUPPORTED_TEXT",
                    "Word wrapping requires the Unicode layout profile",
                ));
            }
            if !text.chars().all(supported) {
                return Err(error(
                    "UNSUPPORTED_TEXT",
                    "This layout supports declared left-to-right scalar ranges and LF only; shaping/bidi/combining controls are unsupported",
                ));
            }
            for font in fonts {
                cache.load(font, root)?;
            }
            let mut lines: Vec<Vec<Glyph>> = vec![vec![]];
            let mut widths = vec![0.0f32];
            for c in text.chars() {
                if c == '\n' {
                    lines.push(vec![]);
                    widths.push(0.0);
                    continue;
                }
                let index = fonts
                    .iter()
                    .position(|f| cache.fonts[&f.path].font.has_glyph(c))
                    .ok_or_else(|| {
                        error(
                            "MISSING_GLYPH",
                            format!("No supplied font contains U+{:04X}", c as u32),
                        )
                    })?;
                let metrics = cache.fonts[&fonts[index].path]
                    .font
                    .metrics(c, *size as f32);
                if metrics.width > crate::scene::MAX_GLYPH
                    || metrics.height > crate::scene::MAX_GLYPH
                    || !metrics.advance_width.is_finite()
                    || !(0.0..=crate::scene::MAX_GLYPH as f32).contains(&metrics.advance_width)
                    || metrics.xmin.unsigned_abs() > 32768
                    || metrics.ymin.unsigned_abs() > 32768
                {
                    return Err(error(
                        "UNSUPPORTED_FONT",
                        "Glyph metrics exceed the bounded canvas contract",
                    ));
                }
                let mut line = lines.len() - 1;
                let spacing = if lines[line].is_empty() {
                    0.0
                } else {
                    *letter_spacing as f32
                };
                if matches!(wrap, Wrap::Character)
                    && !lines[line].is_empty()
                    && widths[line] + spacing + metrics.advance_width > rect[2] as f32
                {
                    lines.push(vec![]);
                    widths.push(0.0);
                    line += 1;
                }
                let x = widths[line]
                    + if lines[line].is_empty() {
                        0.0
                    } else {
                        *letter_spacing as f32
                    };
                widths[line] = x + metrics.advance_width;
                lines[line].push(Glyph {
                    c,
                    font: index,
                    metrics,
                    x,
                });
            }
            let mut report = vec![];
            let mut spans = Vec::new();
            let mut clipped = 0usize;
            let mut cache_glyphs = HashMap::new();
            let mut glyph_pixels = 0usize;
            for (n, line) in lines.iter().enumerate() {
                let offset = match align {
                    Align::Left => 0.0,
                    Align::Center => (rect[2] as f32 - widths[n]) / 2.0,
                    Align::Right => rect[2] as f32 - widths[n],
                };
                let baseline = valign.first_baseline(*rect, *size, *line_height, lines.len())
                    + n as i32 * *line_height as i32;
                if matches!(overflow, Overflow::Reject)
                    && (widths[n] > rect[2] as f32
                        || baseline > rect[1] + rect[3]
                        || baseline - (*size as i32) < rect[1])
                {
                    return Err(error("TEXT_OVERFLOW", "Text line exceeds its declared box"));
                }
                spans.push([
                    rect[0] + offset.round() as i32,
                    rect[0] + (offset + widths[n]).round() as i32,
                    baseline,
                ]);
                for glyph in line {
                    let m = &glyph.metrics;
                    let x = rect[0] + (glyph.x + offset).round() as i32 + m.xmin;
                    let y = baseline - bitmap_top(m);
                    if let std::collections::hash_map::Entry::Vacant(entry) =
                        cache_glyphs.entry((glyph.font, glyph.c))
                    {
                        glyph_pixels += m.width * m.height;
                        if glyph_pixels > crate::scene::MAX_DECODED_PIXELS {
                            return Err(error(
                                "LIMIT_EXCEEDED",
                                "Glyph coverage cache exceeds 64M pixels",
                            ));
                        }
                        entry.insert(
                            cache.fonts[&fonts[glyph.font].path]
                                .font
                                .rasterize(glyph.c, *size as f32)
                                .1,
                        );
                    }
                    let bitmap = &cache_glyphs[&(glyph.font, glyph.c)];
                    for by in 0..m.height {
                        for bx in 0..m.width {
                            let coverage = bitmap[by * m.width + bx];
                            if coverage == 0 {
                                continue;
                            }
                            let (px, py) = (x + bx as i32, y + by as i32);
                            if px < rect[0]
                                || py < rect[1]
                                || px >= rect[0] + rect[2]
                                || py >= rect[1] + rect[3]
                            {
                                clipped += 1;
                                if matches!(overflow, Overflow::Reject) {
                                    return Err(error(
                                        "TEXT_OVERFLOW",
                                        "Glyph coverage exceeds its declared box",
                                    ));
                                }
                                continue;
                            }
                            put(&mut rgba, width, px, py, *color, coverage);
                        }
                    }
                    report.push(json!({"scalar":glyph.c.to_string(),"font_index":glyph.font,"line":n,"bitmap_rect":[x,y,m.width as i32,m.height as i32],"baseline":baseline,"advance":m.advance_width}));
                }
            }
            let rgba = decorate(rgba, canvas, graphic, &spans);
            Ok((
                rgba,
                json!({"kind":"text","layout":"ltr_scalar_v1","rasterizer":"fontdue-0.9.4-scalar","glyphs":report,"line_widths":widths,"clipped_coverage_pixels":clipped,"box":rect}),
            ))
        }
    }
}
pub fn capabilities() -> Value {
    json!({"primitives":["text","rectangle","ellipse"],"fonts":"identity_bound_user_supplied_truetype","layout":"ltr_scalar_v1","layouts":["ltr_scalar_v1","unicode_v1"],"font_fallback":"ordered_explicit_files_missing_glyph_errors","complex_shaping":true,"unicode":unicode::capabilities(),"vertical_alignment":["top","middle","bottom"],"text_background":"padded_line_boxes_drawn_once","text_outline":{"shape":"disc_dilated_alpha","width":[1,8]},"animation":"scene_position_opacity_and_masks","alpha":"straight","maximum_fonts":8,"maximum_font_bytes":8388608,"maximum_total_font_bytes":33554432})
}

#[cfg(test)]
mod tests {
    use super::VAlign;

    #[test]
    fn vertical_alignment_places_line_slots() {
        // Two 40-pixel line slots of 32-pixel text in a box 100 pixels high starting at y=10.
        let rect = [0, 10, 200, 100];
        assert_eq!(VAlign::Top.first_baseline(rect, 32, 40, 2), 42);
        assert_eq!(VAlign::Middle.first_baseline(rect, 32, 40, 2), 52);
        // The last baseline sits line_height - size above the box bottom.
        assert_eq!(VAlign::Bottom.first_baseline(rect, 32, 40, 2) + 40, 110 - 8);
        assert_eq!(VAlign::Middle.first_baseline([0, 0, 10, 41], 32, 40, 1), 32);
    }
}
