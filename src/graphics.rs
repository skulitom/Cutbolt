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
            wrap,
            overflow,
            layout,
        } => {
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
            let mut clipped = 0usize;
            let mut cache_glyphs = HashMap::new();
            let mut glyph_pixels = 0usize;
            for (n, line) in lines.iter().enumerate() {
                let offset = match align {
                    Align::Left => 0.0,
                    Align::Center => (rect[2] as f32 - widths[n]) / 2.0,
                    Align::Right => rect[2] as f32 - widths[n],
                };
                let baseline = rect[1] + *size as i32 + n as i32 * *line_height as i32;
                if matches!(overflow, Overflow::Reject)
                    && (widths[n] > rect[2] as f32 || baseline > rect[1] + rect[3])
                {
                    return Err(error("TEXT_OVERFLOW", "Text line exceeds its declared box"));
                }
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
            Ok((
                rgba,
                json!({"kind":"text","layout":"ltr_scalar_v1","rasterizer":"fontdue-0.9.4-scalar","glyphs":report,"line_widths":widths,"clipped_coverage_pixels":clipped,"box":rect}),
            ))
        }
    }
}
pub fn capabilities() -> Value {
    json!({"primitives":["text","rectangle","ellipse"],"fonts":"identity_bound_user_supplied_truetype","layout":"ltr_scalar_v1","layouts":["ltr_scalar_v1","unicode_v1"],"font_fallback":"ordered_explicit_files_missing_glyph_errors","complex_shaping":true,"unicode":unicode::capabilities(),"animation":"scene_position_opacity_and_masks","alpha":"straight","maximum_fonts":8,"maximum_font_bytes":8388608,"maximum_total_font_bytes":33554432})
}
