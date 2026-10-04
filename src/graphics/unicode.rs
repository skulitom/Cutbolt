//! Original bounded layout orchestration around external Unicode/font algorithms.
use super::{Align, Cache, Graphic, Overflow, Wrap, decorate, put};
use crate::{Result, error};
use fontdue::{Font, FontSettings};
use harfrust::{
    BufferFlags, Direction, FontRef, Language, ShapeOptions, Shaper, ShaperData, Tag, UnicodeBuffer,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::Path,
};
use unicode_bidi::{BidiInfo, Level};
use unicode_script::{Script, UnicodeScript};
use unicode_segmentation::UnicodeSegmentation;

const MAX_WORK: usize = 1_048_576;
const MAX_GLYPHS: usize = 8192;
const MAX_DRAW_PIXELS: usize = 32_000_000;

/// Paragraph base direction: `auto` (from the first strong character), `ltr` or `rtl`; embedded runs keep their own direction.
#[derive(Clone, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BaseDirection {
    #[default]
    Auto,
    Ltr,
    Rtl,
}
fn und() -> String {
    "und".into()
}
/// Optional text layout profile, tagged by `profile`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "profile", rename_all = "snake_case", deny_unknown_fields)]
pub enum TextLayout {
    /// Unicode layout: grapheme font fallback, OpenType shaping, bidi and word wrapping.
    UnicodeV1 {
        /// Paragraph base direction; default `auto`.
        #[serde(default)]
        direction: BaseDirection,
        /// ASCII language tag for shaping, such as `sr`; hyphen-separated 1..=8 character subtags, at most 63 characters. Default `und`.
        #[serde(default = "und")]
        language: String,
    },
}
fn invalid_font(message: &str) -> crate::Error {
    error("UNSUPPORTED_FONT", message)
}
fn ignored(c: char) -> bool {
    matches!(c, '\u{034f}'|'\u{061c}'|'\u{200b}'..='\u{200f}'|'\u{202a}'..='\u{202e}'|'\u{2060}'|'\u{2066}'..='\u{2069}')
}
fn supported(c: char) -> bool {
    c == '\n'
        || (!c.is_control()
            && !matches!(c,'\u{00ad}'|'\u{2028}'|'\u{2029}'|'\u{fe00}'..='\u{fe0f}'|'\u{feff}'|'\u{e0100}'..='\u{e01ef}'))
}
fn strong(script: Script) -> bool {
    !matches!(script, Script::Common | Script::Inherited | Script::Unknown)
}
fn language(value: &str) -> Result<Language> {
    if value.is_empty()
        || value.len() > 63
        || value.split('-').enumerate().any(|(i, s)| {
            s.is_empty()
                || s.len() > 8
                || !s.bytes().all(|b| {
                    if i == 0 {
                        b.is_ascii_alphabetic()
                    } else {
                        b.is_ascii_alphanumeric()
                    }
                })
        })
    {
        return Err(error(
            "INVALID_GRAPHIC",
            "Language requires bounded ASCII language-tag subtags",
        ));
    }
    value
        .parse()
        .map_err(|_| error("INVALID_GRAPHIC", "Invalid shaping language"))
}
fn charge(work: &mut usize, amount: usize) -> Result<()> {
    *work = work
        .checked_add(amount)
        .ok_or_else(|| error("LIMIT_EXCEEDED", "Text work overflow"))?;
    if *work > MAX_WORK {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Text shaping exceeds one million input scalars",
        ));
    }
    Ok(())
}
fn pixel(value_128: i64) -> i32 {
    (value_128.signum() * ((value_128.abs() + 64) / 128)) as i32
}
#[derive(Clone)]
struct Piece {
    bytes: Range<usize>,
    script: Script,
    font: usize,
}
struct Glyph {
    id: u16,
    font: usize,
    cluster: usize,
    script: Script,
    rtl: bool,
    x_64: i64,
    y_64: i32,
    advance_64: i32,
}
struct Line {
    bytes: Range<usize>,
    width_64: i64,
    glyphs: Vec<Glyph>,
    rtl: bool,
}
struct Paragraph<'a, 'b> {
    text: &'a str,
    pieces: Vec<Piece>,
    bidi: BidiInfo<'a>,
    shapers: &'b [Shaper<'b>],
    language: Language,
    size: i32,
    spacing_64: i64,
    default_rtl: bool,
}
impl Paragraph<'_, '_> {
    fn line(&self, bytes: Range<usize>, work: &mut usize) -> Result<Line> {
        charge(work, self.text[bytes.clone()].chars().count())?;
        let rtl = self
            .bidi
            .paragraphs
            .first()
            .map_or(self.default_rtl, |p| p.level.is_rtl());
        let mut result = Line {
            bytes: bytes.clone(),
            width_64: 0,
            glyphs: Vec::new(),
            rtl,
        };
        if bytes.is_empty() {
            return Ok(result);
        }
        let (levels, runs) = self
            .bidi
            .visual_runs(&self.bidi.paragraphs[0], bytes.clone());
        let mut previous_cluster = None;
        for run in runs {
            let rtl = levels[run.start].is_rtl();
            let mut spans: Vec<Piece> = Vec::new();
            for piece in &self.pieces {
                let intersect = piece.bytes.start.max(run.start)..piece.bytes.end.min(run.end);
                if intersect.is_empty() {
                    continue;
                }
                if let Some(last) = spans.last_mut().filter(|p| {
                    p.bytes.end == intersect.start
                        && p.script == piece.script
                        && p.font == piece.font
                }) {
                    last.bytes.end = intersect.end;
                } else {
                    spans.push(Piece {
                        bytes: intersect,
                        script: piece.script,
                        font: piece.font,
                    });
                }
            }
            if rtl {
                spans.reverse();
            }
            for span in spans {
                let mut buffer = UnicodeBuffer::new();
                for (offset, c) in self.text[span.bytes.clone()].char_indices() {
                    buffer.add(c, (span.bytes.start + offset) as u32);
                }
                buffer.set_script(span.script.short_name().parse().expect("known ISO script"));
                buffer.set_direction(if rtl {
                    Direction::RightToLeft
                } else {
                    Direction::LeftToRight
                });
                buffer.set_language(self.language.clone());
                // Keep joining context across font changes, but never across the selected line.
                buffer.set_pre_context(&self.text[bytes.start..span.bytes.start]);
                buffer.set_post_context(&self.text[span.bytes.end..bytes.end]);
                let mut flags = BufferFlags::REMOVE_DEFAULT_IGNORABLES;
                if span.bytes.start == bytes.start {
                    flags |= BufferFlags::BEGINNING_OF_TEXT;
                }
                if span.bytes.end == bytes.end {
                    flags |= BufferFlags::END_OF_TEXT;
                }
                buffer.set_flags(flags);
                let shaped = self.shapers[span.font]
                    .shape(buffer, ShapeOptions::default().scale(Some(self.size * 64)));
                if result.glyphs.len() + shaped.len() > MAX_GLYPHS {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Text line exceeds 8192 shaped glyphs",
                    ));
                }
                for (info, position) in shaped.glyph_infos().iter().zip(shaped.glyph_positions()) {
                    if info.glyph_id == 0 {
                        return Err(error(
                            "MISSING_GLYPH",
                            "The selected font cannot shape a complete text cluster",
                        ));
                    }
                    if info.glyph_id > u16::MAX as u32
                        || position.y_advance != 0
                        || position.x_advance.unsigned_abs() > crate::scene::MAX_GLYPH as u32 * 64
                        || position.x_offset.unsigned_abs() > 32768 * 64
                        || position.y_offset.unsigned_abs() > 32768 * 64
                    {
                        return Err(invalid_font(
                            "Shaped glyph metrics exceed the horizontal layout bounds",
                        ));
                    }
                    if previous_cluster.is_some_and(|p| p != info.cluster) {
                        result.width_64 += self.spacing_64;
                    }
                    previous_cluster = Some(info.cluster);
                    let cluster = info.cluster as usize;
                    if !span.bytes.contains(&cluster) || !self.text.is_char_boundary(cluster) {
                        return Err(invalid_font("Invalid shaped source cluster"));
                    }
                    result.glyphs.push(Glyph {
                        id: info.glyph_id as u16,
                        font: span.font,
                        cluster,
                        script: span.script,
                        rtl,
                        x_64: result.width_64 + position.x_offset as i64,
                        y_64: position.y_offset,
                        advance_64: position.x_advance,
                    });
                    result.width_64 += position.x_advance as i64;
                }
            }
        }
        if result.width_64 < 0 {
            return Err(invalid_font("Shaped line has a negative advance"));
        }
        Ok(result)
    }
    fn lines(&self, wrap: &Wrap, width_64: i64, work: &mut usize) -> Result<Vec<Line>> {
        if self.text.is_empty() || matches!(wrap, Wrap::None) {
            return Ok(vec![self.line(0..self.text.len(), work)?]);
        }
        let ends: Vec<_> = self.pieces.iter().map(|p| p.bytes.end).collect();
        let breaks: HashSet<_> = unicode_linebreak::linebreaks(self.text)
            .map(|(end, _)| end)
            .collect();
        let mut lines = Vec::new();
        let mut cursor = 0;
        while cursor < ends.len() {
            let start = if cursor == 0 { 0 } else { ends[cursor - 1] };
            let mut previous: Option<Line> = None;
            let mut last_word = None;
            let mut i = cursor;
            while i < ends.len() {
                let current = self.line(start..ends[i], work)?;
                if current.width_64 > width_64 && previous.is_some() {
                    let previous = previous.take().expect("nonempty previous line");
                    let end = if matches!(wrap, Wrap::Word) {
                        last_word.unwrap_or(previous.bytes.end)
                    } else {
                        previous.bytes.end
                    };
                    lines.push(if previous.bytes.end == end {
                        previous
                    } else {
                        self.line(start..end, work)?
                    });
                    cursor = ends.binary_search(&end).expect("grapheme line boundary") + 1;
                    break;
                }
                if breaks.contains(&ends[i]) {
                    last_word = Some(ends[i]);
                }
                previous = Some(current);
                i += 1;
            }
            if i == ends.len() {
                lines.push(previous.expect("last nonempty line"));
                cursor = ends.len();
            }
        }
        Ok(lines)
    }
}

pub(super) fn rasterize(
    graphic: &Graphic,
    canvas: [u32; 2],
    root: &Path,
    cache: &mut Cache,
    mut rgba: Vec<u8>,
) -> Result<(Vec<u8>, Value)> {
    let Graphic::Text {
        text,
        fonts,
        size,
        color,
        rect,
        line_height,
        letter_spacing,
        align,
        valign,
        background: _,
        outline: _,
        wrap,
        overflow,
        layout:
            Some(TextLayout::UnicodeV1 {
                direction,
                language: tag,
            }),
    } = graphic
    else {
        unreachable!("validated Unicode text")
    };
    if !text.chars().all(supported) {
        return Err(error(
            "UNSUPPORTED_TEXT",
            "Unicode layout accepts LF paragraphs; controls, soft hyphens, alternate paragraph separators and variation selectors are unsupported",
        ));
    }
    let language = language(tag)?;
    for identity in fonts {
        cache.load(identity, root)?;
        let loaded = cache.fonts.get_mut(&identity.path).expect("loaded font");
        if loaded.shaping.is_none() {
            let font =
                FontRef::new(&loaded.bytes).map_err(|_| invalid_font("Invalid TrueType tables"))?;
            for tag in [
                b"CFF ", b"CFF2", b"COLR", b"CBDT", b"sbix", b"SVG ", b"morx", b"mort", b"fvar",
            ] {
                if font.table_data(Tag::new(tag)).is_some() {
                    return Err(invalid_font(
                        "Unicode layout requires static monochrome TrueType/GSUB outlines",
                    ));
                }
            }
            loaded.shaping = Some(ShaperData::new(&font));
            loaded.font = Font::from_bytes(
                loaded.bytes.as_slice(),
                FontSettings {
                    load_substitutions: true,
                    ..FontSettings::default()
                },
            )
            .map_err(invalid_font)?;
        }
    }
    let loaded: Vec<_> = fonts.iter().map(|f| &cache.fonts[&f.path]).collect();
    let faces: Vec<_> = loaded
        .iter()
        .map(|f| FontRef::new(&f.bytes).map_err(|_| invalid_font("Invalid cached font")))
        .collect::<Result<_>>()?;
    let shapers: Vec<_> = loaded
        .iter()
        .zip(&faces)
        .map(|(f, face)| {
            f.shaping
                .as_ref()
                .expect("enabled shaping")
                .shaper(face)
                .build()
        })
        .collect();
    let base = match direction {
        BaseDirection::Auto => None,
        BaseDirection::Ltr => Some(Level::ltr()),
        BaseDirection::Rtl => Some(Level::rtl()),
    };
    let mut lines = Vec::new();
    let mut byte_base = 0;
    let mut work = 0;
    for paragraph in text.split('\n') {
        let mut pieces = Vec::new();
        for (offset, grapheme) in paragraph.grapheme_indices(true) {
            let script = grapheme
                .chars()
                .map(|c| c.script())
                .find(|s| strong(*s))
                .unwrap_or(Script::Common);
            let mut chosen = None;
            for (index, shaper) in shapers.iter().enumerate() {
                charge(&mut work, grapheme.chars().count())?;
                let mut buffer = UnicodeBuffer::new();
                buffer.push_str(grapheme);
                if strong(script) {
                    buffer.set_script(script.short_name().parse().expect("known ISO script"));
                }
                buffer.set_language(language.clone());
                buffer.set_pre_context(&paragraph[..offset]);
                buffer.set_post_context(&paragraph[offset + grapheme.len()..]);
                buffer.guess_segment_properties();
                buffer.set_flags(BufferFlags::REMOVE_DEFAULT_IGNORABLES);
                let result = shaper.shape(buffer, ShapeOptions::default());
                if result.len() > MAX_GLYPHS {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Grapheme expands beyond 8192 glyphs",
                    ));
                }
                if (!result.is_empty() || grapheme.chars().all(ignored))
                    && result.glyph_infos().iter().all(|g| g.glyph_id != 0)
                {
                    chosen = Some(index);
                    break;
                }
            }
            let font = chosen.ok_or_else(|| {
                error(
                    "MISSING_GLYPH",
                    format!(
                        "No supplied font can shape the complete grapheme at UTF-8 byte {}",
                        byte_base + offset
                    ),
                )
            })?;
            pieces.push(Piece {
                bytes: offset..offset + grapheme.len(),
                script,
                font,
            });
        }
        for i in 0..pieces.len() {
            if !strong(pieces[i].script) {
                pieces[i].script = pieces[..i]
                    .iter()
                    .rev()
                    .find(|p| strong(p.script))
                    .or_else(|| pieces[i + 1..].iter().find(|p| strong(p.script)))
                    .map(|p| p.script)
                    .unwrap_or(Script::Common);
            }
        }
        let paragraph = Paragraph {
            text: paragraph,
            pieces,
            bidi: BidiInfo::new(paragraph, base),
            shapers: &shapers,
            language: language.clone(),
            size: *size as i32,
            spacing_64: *letter_spacing as i64 * 64,
            default_rtl: base.is_some_and(|level| level.is_rtl()),
        };
        let mut part = paragraph.lines(wrap, rect[2] as i64 * 64, &mut work)?;
        for line in &mut part {
            line.bytes.start += byte_base;
            line.bytes.end += byte_base;
            for glyph in &mut line.glyphs {
                glyph.cluster += byte_base;
            }
        }
        lines.extend(part);
        byte_base += paragraph.text.len() + 1;
    }
    if lines.iter().map(|l| l.glyphs.len()).sum::<usize>() > MAX_GLYPHS {
        return Err(error("LIMIT_EXCEEDED", "Text exceeds 8192 shaped glyphs"));
    }
    let mut glyph_cache = HashMap::new();
    let mut cached_pixels = 0usize;
    let mut drawn_pixels = 0usize;
    let mut clipped = 0usize;
    let mut glyph_report = Vec::new();
    let mut line_report = Vec::new();
    let mut spans = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let delta = rect[2] as i64 * 64 - line.width_64;
        let offset_128 = match align {
            Align::Left => 0,
            Align::Center => delta,
            Align::Right => delta * 2,
        };
        let baseline = valign.first_baseline(*rect, *size, *line_height, lines.len())
            + index as i32 * *line_height as i32;
        if matches!(overflow, Overflow::Reject)
            && (delta < 0 || baseline > rect[1] + rect[3] || baseline - (*size as i32) < rect[1])
        {
            return Err(error("TEXT_OVERFLOW", "Text line exceeds its declared box"));
        }
        spans.push([
            rect[0] + pixel(offset_128),
            rect[0] + pixel(offset_128 + 2 * line.width_64),
            baseline,
        ]);
        line_report.push(json!({"utf8_bytes":[line.bytes.start,line.bytes.end],"width_64":line.width_64,"baseline":baseline,"base_direction":if line.rtl{"rtl"}else{"ltr"}}));
        for glyph in &line.glyphs {
            let key = (glyph.font, glyph.id);
            let font = &loaded[glyph.font].font;
            if glyph.id >= font.glyph_count() {
                return Err(invalid_font("Shaped glyph index exceeds the font"));
            }
            if let std::collections::hash_map::Entry::Vacant(entry) = glyph_cache.entry(key) {
                let metrics = font.metrics_indexed(glyph.id, *size as f32);
                if metrics.width > crate::scene::MAX_GLYPH
                    || metrics.height > crate::scene::MAX_GLYPH
                    || !metrics.advance_width.is_finite()
                    || metrics.advance_width.abs() > crate::scene::MAX_GLYPH as f32
                    || metrics.xmin.unsigned_abs() > 32768
                    || metrics.ymin.unsigned_abs() > 32768
                {
                    return Err(invalid_font(
                        "Indexed glyph metrics exceed the canvas contract",
                    ));
                }
                if metrics.advance_width == 0.0 && metrics.width == 0 && glyph.advance_64 != 0 {
                    return Err(invalid_font(
                        "Substitution glyph has no loaded raster metrics",
                    ));
                }
                cached_pixels += metrics.width * metrics.height;
                if cached_pixels > crate::scene::MAX_DECODED_PIXELS {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Glyph coverage cache exceeds 64M pixels",
                    ));
                }
                entry.insert((metrics, font.rasterize_indexed(glyph.id, *size as f32).1));
            }
            let (metrics, bitmap) = &glyph_cache[&key];
            drawn_pixels += metrics.width * metrics.height;
            if drawn_pixels > MAX_DRAW_PIXELS {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Text raster work exceeds 32M pixels",
                ));
            }
            let origin_128 = rect[0] as i64 * 128 + offset_128 + glyph.x_64 * 2;
            let x = rect[0] + pixel(offset_128 + glyph.x_64 * 2) + metrics.xmin;
            let y = baseline - pixel(glyph.y_64 as i64 * 2) - super::bitmap_top(metrics);
            for by in 0..metrics.height {
                for bx in 0..metrics.width {
                    let coverage = bitmap[by * metrics.width + bx];
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
                    put(&mut rgba, canvas[0], px, py, *color, coverage);
                }
            }
            glyph_report.push(json!({"glyph_id":glyph.id,"font_index":glyph.font,"cluster_utf8":glyph.cluster,"script":glyph.script.short_name(),"direction":if glyph.rtl{"rtl"}else{"ltr"},"line":index,"origin_x_128":origin_128,"offset_y_64":glyph.y_64,"advance_64":glyph.advance_64,"bitmap_rect":[x,y,metrics.width as i32,metrics.height as i32],"baseline":baseline}));
        }
    }
    let widths: Vec<_> = lines.iter().map(|l| l.width_64 as f64 / 64.0).collect();
    let rgba = decorate(rgba, canvas, graphic, &spans);
    Ok((
        rgba,
        json!({"kind":"text","layout":"unicode_v1","shaper":"harfrust-0.13.3","rasterizer":"fontdue-0.9.4-scalar-indexed","direction":direction,"language":tag,"glyphs":glyph_report,"lines":line_report,"line_widths":widths,"clipped_coverage_pixels":clipped,"box":rect,"shaped_input_scalars":work,"cached_coverage_pixels":cached_pixels,"raster_work_pixels":drawn_pixels}),
    ))
}
pub(super) fn capabilities() -> Value {
    json!({"profile":"unicode_v1","shaper":"harfrust-0.13.3","bidi":"unicode-bidi-0.3.18","graphemes":"unicode-segmentation-1.13.3","scripts":"unicode-script-0.5.8","line_breaks":"unicode-linebreak-0.1.5","base_directions":["auto","ltr","rtl"],"wrap":["none","character","word"],"fallback":"first_explicit_font_shaping_complete_grapheme","position_units_per_pixel":64,"maximum_shaped_input_scalars":MAX_WORK,"maximum_glyphs":MAX_GLYPHS,"maximum_raster_work_pixels":MAX_DRAW_PIXELS})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn signed_origin_rounding_and_work_limits() {
        assert_eq!(
            (-192..=192).map(pixel).collect::<Vec<_>>(),
            (-192i64..=192)
                .map(|n| (n as f64 / 128.0).round() as i32)
                .collect::<Vec<_>>()
        );
        let mut work = MAX_WORK - 1;
        charge(&mut work, 1).unwrap();
        assert_eq!(charge(&mut work, 1).unwrap_err().code, "LIMIT_EXCEEDED");
        for tag in ["und", "sr", "zh-Hant-TW", "en-GB"] {
            language(tag).unwrap();
        }
        for tag in ["", "en--GB", "en_ GB", "123", "abcdefghi"] {
            assert!(language(tag).is_err());
        }
        assert!(supported('\u{10400}'));
        assert!(supported('\u{2067}'));
        assert!(!supported('\r'));
        assert!(!supported('\u{2028}'));
        assert!(!supported('\u{fe0f}'));
    }
}
