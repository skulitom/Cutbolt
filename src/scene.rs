//! Bounded, editable pixel scenes compiled to the existing lossless timeline profile.
//! Exact time sampling; original integer pixel transforms and optional interpolated 2D mapping.
use crate::{
    At, Result,
    animation::Curve,
    composite::{self, AlphaMode, BlendMode, RectMask},
    error, media,
    registry::{UPDATE_IDENTITY, changed_digest, changed_size},
    render,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::{BufWriter, Cursor, Write},
    path::{Component, Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const FPS: Time = Time { num: 25, den: 1 };
const RATE: Time = Time { num: 48000, den: 1 };

/// Bounds shared by scene validation, text rasterization and capability discovery.
/// A logical canvas may be composited at native output size (output_scale 1, e.g. 1920x1080).
pub(crate) const MAX_CANVAS: u32 = 4096;
pub(crate) const MAX_OUTPUT_PIXELS: u64 = 8_000_000;
pub(crate) const MAX_LAYERS: usize = 16;
pub(crate) const MAX_FRAMES: u64 = 250;
pub(crate) const MAX_REFERENCES: usize = 1024;
pub(crate) const MAX_DECODED_PIXELS: usize = 64_000_000;
pub(crate) const MAX_TEXT_SIZE: u16 = 512;
pub(crate) const MAX_GLYPH: usize = 1024;
pub(crate) const MAX_TILES: usize = 256;
pub(crate) const MAX_TILE_CELLS: usize = 4096;

pub(crate) fn limits() -> Value {
    json!({"canvas_per_axis":[1,MAX_CANVAS],"output_scale":[1,8],"maximum_output_pixels":MAX_OUTPUT_PIXELS,
        "frames":[1,MAX_FRAMES],"frame_rate":{"num":25,"den":1},"layers":[1,MAX_LAYERS],"maximum_frame_references":MAX_REFERENCES,
        "maximum_decoded_pixels":MAX_DECODED_PIXELS,"png_per_axis":[1,MAX_CANVAS],"png_maximum_bytes":64*1024*1024,
        "png_total_bytes":256*1024*1024,"text_size":[1,MAX_TEXT_SIZE],"glyph_bitmap_per_axis":MAX_GLYPH,
        "integer_layer_scale":[1,16],"tilemap":{"maximum_tiles":MAX_TILES,"maximum_cells":MAX_TILE_CELLS,"tile_size_per_axis":[1,MAX_CANVAS],
        "empty_cells":"null","assembly":"straight_rgba_copy_into_layer_canvas_before_mask_effects_transform"},
        "encoding":"streamed_rgb24_frames_parallel_composition_deterministic_order"})
}

/// Content identity of a source file: a path inside the input root plus its SHA-256 and size.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Relative path of normal components inside `input_root`; no `..`, drive or absolute parts.
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the file contents (64 characters). Omit it, with or without `bytes`, to have the file hashed when the request runs.
    #[serde(default)]
    pub sha256: String,
    /// Exact nonzero file size in bytes; checked against the file before use. Omitted values are read from the file.
    #[serde(default)]
    pub bytes: u64,
}
/// One held source image in a layer or tile animation.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// Identity of an 8-bit RGB or RGBA PNG, plain or sRGB-tagged.
    pub image: Identity,
    /// Optional binary matte PNG of the same size: opaque black clears source pixels, white keeps them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<Identity>,
    /// Positive display length, rational seconds; strict timing needs a whole number of 25 fps frames.
    pub hold: Time,
    /// `[x, y]` pixel position of a trimmed PNG inside the layer canvas or tile; the image must fit.
    pub offset: [u32; 2],
    /// `[x, y]` full-canvas pivot (pixel corners) placed at the transform position; each -4096..4096.
    pub anchor: [i32; 2],
}
/// How frame holds map to the 25 fps scene clock: `strict` requires every hold to be whole frames; `sample_start` shows the frame containing each exact sample time.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    Strict,
    SampleStart,
}
/// What shows after the frame cycle ends: `loop` repeats it, `hold_last` keeps the final frame, `transparent` shows nothing.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Loop,
    HoldLast,
    Transparent,
}
/// Integer layer placement: crop, clockwise quarter turns, nearest-neighbor scale, then the anchor lands at `position`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    /// `[x, y]` scene pixel where the frame anchor lands; each -32768..32768.
    pub position: [i32; 2],
    /// `[x, y, width, height]` source-canvas rectangle to show; nonzero size, must fit the canvas.
    pub crop: [u32; 4],
    /// Integer nearest-neighbor enlargement, 1..16.
    pub scale: u32,
    /// Clockwise 90-degree rotations, 0..3.
    pub quarter_turns: u8,
    /// Layer opacity 0..255, where 255 is opaque.
    pub opacity: u8,
    /// Optional interpolated mapping with subpixel translation, scale, rotation and fitting; omit for the integer path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<crate::spatial::Transform>,
}
/// Keyframe curves overriding the layer's static position and opacity on the layer-local clock; declare at least one.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    /// Curve for the x position in scene pixels, -32768..32768; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_x: Option<Curve>,
    /// Curve for the y position in scene pixels, -32768..32768; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_y: Option<Curve>,
    /// Curve for opacity, 0..255; omit to keep the static value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<Curve>,
}
#[derive(Clone, Debug, Serialize)]
struct Parameters {
    position: [i32; 2],
    opacity: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    mask_rect: Option<[i32; 4]>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    effects: Vec<crate::effects::Sample>,
    #[serde(skip_serializing_if = "Option::is_none")]
    spatial: Option<crate::spatial::Mapping>,
}
/// One reusable tile: an ordinary held-frame animation with its own clock, relative to layer start.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    /// Ordered held frames, at least one; each must fit `tile_size`.
    pub frames: Vec<Frame>,
    /// How this tile's holds map to the 25 fps clock.
    pub timing: Timing,
    /// What the tile shows after its frame cycle ends.
    pub end: End,
}
/// A grid of tile references assembled into the layer's source canvas before masks, effects and
/// transforms. `cells` lists rows top to bottom; each entry indexes `tiles` or is null (transparent).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tilemap {
    /// `[width, height]` of every cell in pixels, 1..4096 each; times the grid must equal the layer canvas.
    pub tile_size: [u32; 2],
    /// Reusable tile animations referenced by `cells`, 1..256.
    pub tiles: Vec<Tile>,
    /// Equal-length rows, top to bottom, of `tiles` indexes or null for transparent; at most 4096 cells.
    pub cells: Vec<Vec<Option<u16>>>,
}
impl Tilemap {
    /// Columns and rows of the cell grid.
    pub(crate) fn grid(&self) -> (u32, u32) {
        (
            self.cells.first().map_or(0, |r| r.len()) as u32,
            self.cells.len() as u32,
        )
    }
    fn validate(&self, layer: &Layer, at: &dyn Fn(&str) -> String) -> Result<()> {
        let (columns, rows) = self.grid();
        if self.tiles.is_empty()
            || self.tiles.len() > MAX_TILES
            || columns == 0
            || rows == 0
            || columns as usize * rows as usize > MAX_TILE_CELLS
            || self.cells.iter().any(|r| r.len() != columns as usize)
            || self.tile_size.iter().any(|n| !(1..=MAX_CANVAS).contains(n))
            || self.tile_size[0] as u64 * columns as u64 != layer.canvas[0] as u64
            || self.tile_size[1] as u64 * rows as u64 != layer.canvas[1] as u64
        {
            return Err(invalid(&format!(
                "{}: a tilemap needs 1-{MAX_TILES} tiles, a rectangular grid of 1-{MAX_TILE_CELLS} cells, tile size 1-{MAX_CANVAS} per axis, and canvas = tile_size x grid",
                at("tilemap")
            )));
        }
        for (r, row) in self.cells.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if cell.is_some_and(|i| i as usize >= self.tiles.len()) {
                    return Err(invalid(&format!(
                        "{}: cell refers to a missing tile",
                        at(&format!("tilemap.cells[{r}][{c}]"))
                    )));
                }
            }
        }
        for (ti, tile) in self.tiles.iter().enumerate() {
            if tile.frames.is_empty() {
                return Err(invalid(&format!(
                    "{}: a tile needs at least one frame",
                    at(&format!("tilemap.tiles[{ti}]"))
                )));
            }
            for (fi, f) in tile.frames.iter().enumerate() {
                let path = || at(&format!("tilemap.tiles[{ti}].frames[{fi}].hold"));
                f.hold.validate().at(path)?;
                if f.hold.num == 0 || f.anchor.iter().any(|n| !(-4096..=4096).contains(n)) {
                    return Err(invalid("Frame holds must be positive and anchors bounded"));
                }
                if matches!(tile.timing, Timing::Strict) {
                    f.hold.units(FPS).at(path)?;
                }
            }
        }
        Ok(())
    }
}
/// One scene layer; later layers composite over earlier ones. Its source is exactly one of `frames`, `graphics` or `tilemap`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Unique nonblank layer ID within the scene, at most 128 bytes.
    pub id: String,
    /// `[width, height]` of the full source canvas in pixels, 1..4096 each; frames, crop and masks use it.
    pub canvas: [u32; 2],
    /// Scene time the layer becomes active, rational seconds on the 25 fps grid.
    pub start: Time,
    /// Nonzero active length, rational seconds on the 25 fps grid; start plus duration must fit the scene.
    pub duration: Time,
    /// Ordered held PNG frames; leave empty when using `graphics` or `tilemap`.
    pub frames: Vec<Frame>,
    /// Text and shapes rasterized into the canvas; needs strict timing, hold_last and straight alpha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics: Option<crate::graphics::Graphic>,
    /// Grid of animated tiles assembled into the canvas; needs strict timing and hold_last, and no geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilemap: Option<Tilemap>,
    /// How frame holds map to the 25 fps scene clock.
    pub timing: Timing,
    /// What shows after the frame cycle ends; the layer duration still limits activation.
    pub end: End,
    /// Crop, rotation, scale, placement and opacity, with optional spatial mapping.
    pub transform: Transform,
    /// Optional position/opacity keyframe curves on the layer-local clock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Animation>,
    /// How PNG RGB relates to alpha; default straight.
    #[serde(default, skip_serializing_if = "AlphaMode::is_straight")]
    pub alpha_mode: AlphaMode,
    /// Blend with the layers below; default normal.
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend_mode: BlendMode,
    /// Optional source-canvas rectangle limiting layer opacity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<RectMask>,
    /// Ordered effects applied to source pixels before compositing; at most 8. Omit or leave empty for none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<crate::effects::Effect>,
}
/// WAV channel mapping to stereo output: `duplicate_mono` copies a mono file to both channels; `preserve_stereo` keeps a stereo file's channels.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channels {
    DuplicateMono,
    PreserveStereo,
}
/// Conversion to 48 kHz; only `linear` interpolation is supported.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resampling {
    Linear,
}
/// Fill before and after the narration; only `silence` is supported.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Padding {
    Silence,
}
/// One narration WAV placed on the scene clock and converted to 48 kHz stereo; it is never trimmed or stretched.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    /// Identity of a classic PCM16 WAV, mono or stereo, at 24, 44.1 or 48 kHz, at most 64 MiB.
    pub file: Identity,
    /// Scene time where narration begins, rational seconds on a 48 kHz sample boundary; audio must end within the scene.
    pub start: Time,
    /// Channel mapping; must match the WAV channel count.
    pub channels: Channels,
    /// Sample-rate conversion method.
    pub resampling: Resampling,
    /// Fill outside the narration.
    pub padding: Padding,
}
/// Scene color interpretation; only `srgb_straight_encoded`: encoded sRGB values with straight alpha, composited in encoded sRGB.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    SrgbStraightEncoded,
}
/// Bounded pixel scene of layers and audio, validated by scene.inspect and compiled by scene.render to a lossless 25 fps asset.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    /// Scene format version; must be 1.
    pub schema_version: u32,
    /// Nonblank scene ID, at most 128 bytes; also the compiled asset ID.
    pub id: String,
    /// Logical canvas width in pixels, 1..4096.
    pub width: u32,
    /// Logical canvas height in pixels, 1..4096.
    pub height: u32,
    /// Integer nearest-neighbor output enlargement, 1..8; the scaled output is at most 8,000,000 pixels.
    pub output_scale: u32,
    /// Scene length, rational seconds; a whole number of 25 fps frames, 1..250.
    pub duration: Time,
    /// Opaque encoded sRGB `[r, g, b]` backdrop below all layers.
    pub background: [u8; 3],
    /// Color interpretation of sources and compositing.
    pub color: Color,
    /// Ordered layers, 1..16, with at most 1024 frame references in total.
    pub layers: Vec<Layer>,
    /// Optional narration WAV; null gives silence unless `audio_mix` is set. Cannot be combined with `audio_mix`.
    pub audio: Option<Audio>,
    /// Optional PCM mix recipe instead of `audio`; its duration must equal the scene's and output must be stereo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_mix: Option<crate::audio::Mix>,
    /// Optional typed property graph whose position/opacity bindings replace sampled layer values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expressions: Option<crate::expressions::Program>,
    /// Optional shutter sampling; omit for one sample at each frame start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<crate::temporal::Exposure>,
    /// Optional 3D plane geometry with cameras, depth and lighting; tilemap layers are rejected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<crate::geometry::Geometry>,
    /// Render over a transparent backdrop instead of `background`, as a straight-alpha FFV1 bgra asset for alpha_over tracks (captions and titles over video). Normal-blend layers only, without geometry; default false.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transparent: bool,
}

pub(crate) struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
struct Prepared {
    images: HashMap<PathBuf, Pixels>,
    matted: HashMap<(PathBuf, PathBuf), Pixels>,
    graphics: HashMap<String, Pixels>,
    sources: Vec<(PathBuf, Identity)>,
    pcm: Vec<i16>,
    report: Value,
    parameters: Vec<Vec<Option<Parameters>>>,
    selected: Vec<Vec<Option<usize>>>,
    /// For tilemap layers: per tile, the selected frame for every exposure sample.
    tiles: Vec<Option<Vec<Vec<Option<usize>>>>>,
    samples_per_frame: usize,
    geometry: Option<crate::geometry::Prepared>,
}

impl Prepared {
    fn frame_pixels(&self, frame: &Frame) -> &Pixels {
        match &frame.matte {
            Some(matte) => &self.matted[&(frame.image.path.clone(), matte.path.clone())],
            None => &self.images[&frame.image.path],
        }
    }
}

fn invalid(message: &str) -> crate::Error {
    error("INVALID_SCENE", message)
}
fn bounded_id(id: &str) -> bool {
    !id.trim().is_empty() && id.len() <= 128
}

impl Scene {
    pub(crate) fn validate(&self) -> Result<u64> {
        if let Some(mix) = &self.audio_mix
            && (self.audio.is_some()
                || mix.duration.compare(self.duration)? != std::cmp::Ordering::Equal)
        {
            return Err(error(
                "INVALID_AUDIO",
                "Choose audio or audio_mix; mix duration must equal scene duration",
            ));
        }
        if self
            .audio_mix
            .as_ref()
            .and_then(|m| m.routing.as_ref())
            .is_some_and(|r| r.output != crate::pcm_wave::Layout::Stereo)
        {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Scene soundtracks require explicit stereo output routing",
            ));
        }
        let frames = self.duration.units(FPS).at(|| "duration".into())?;
        if self.schema_version != 1
            || !bounded_id(&self.id)
            || !(1..=MAX_CANVAS).contains(&self.width)
            || !(1..=MAX_CANVAS).contains(&self.height)
            || !(1..=8).contains(&self.output_scale)
            || !(1..=MAX_FRAMES).contains(&frames)
            || self.width as u64
                * self.height as u64
                * self.output_scale as u64
                * self.output_scale as u64
                > MAX_OUTPUT_PIXELS
            || self.layers.is_empty()
            || self.layers.len() > MAX_LAYERS
        {
            return Err(invalid(&format!(
                "Scene v1 requires 1-250 frames at 25 fps, 1-16 layers, canvas 1-{MAX_CANVAS} per axis, output scale 1-8 and at most 8M output pixels (got {}x{} x{}, {frames} frames, {} layers)",
                self.width,
                self.height,
                self.output_scale,
                self.layers.len()
            )));
        }
        let mut ids = HashSet::new();
        let mut references = 0;
        for (li, layer) in self.layers.iter().enumerate() {
            let at = |field: &str| format!("layers[{li}] ({}).{field}", layer.id);
            let start_units = layer.start.units(FPS).at(|| at("start"))?;
            let duration_units = layer.duration.units(FPS).at(|| at("duration"))?;
            references += layer.frames.len()
                + layer
                    .tilemap
                    .iter()
                    .flat_map(|m| &m.tiles)
                    .map(|t| t.frames.len())
                    .sum::<usize>();
            let sources = !layer.frames.is_empty() as u8
                + layer.graphics.is_some() as u8
                + layer.tilemap.is_some() as u8;
            if !bounded_id(&layer.id)
                || !ids.insert(&layer.id)
                || layer.canvas.contains(&0)
                || layer.canvas.iter().any(|n| *n > MAX_CANVAS)
                || sources != 1
                || references > MAX_REFERENCES
                || duration_units == 0
                || start_units as u128 + duration_units as u128 > frames as u128
            {
                return Err(invalid(&format!(
                    "{}: layer IDs must be unique; canvas 1-{MAX_CANVAS}; choose exactly one of frames, graphics or tilemap; at most {MAX_REFERENCES} frame references; active ranges must fit the scene",
                    at("layer")
                )));
            }
            if (layer.graphics.is_some() || layer.tilemap.is_some())
                && (!matches!(layer.timing, Timing::Strict) || !matches!(layer.end, End::HoldLast))
            {
                return Err(invalid(&format!(
                    "{}: graphics and tilemap layers use strict timing and hold_last; tiles carry their own clocks",
                    at("timing")
                )));
            }
            if layer.graphics.is_some() && !matches!(layer.alpha_mode, AlphaMode::Straight) {
                return Err(invalid("Graphics require straight alpha"));
            }
            if let Some(map) = &layer.tilemap {
                map.validate(layer, &at)?;
            }
            let t = &layer.transform;
            let [x, y, w, h] = t.crop;
            if w == 0
                || h == 0
                || x as u64 + w as u64 > layer.canvas[0] as u64
                || y as u64 + h as u64 > layer.canvas[1] as u64
                || !(1..=16).contains(&t.scale)
                || t.quarter_turns > 3
                || t.position.iter().any(|n| !(-32768..=32768).contains(n))
            {
                return Err(invalid(
                    "Crop must fit the source canvas; integer scale 1-16; clockwise quarter turns 0-3; bounded position",
                ));
            }
            let mut total = Time::ZERO;
            for (fi, f) in layer.frames.iter().enumerate() {
                f.hold.validate().at(|| at(&format!("frames[{fi}].hold")))?;
                if f.hold.num == 0 || f.anchor.iter().any(|n| !(-4096..=4096).contains(n)) {
                    return Err(invalid("Frame holds must be positive and anchors bounded"));
                }
                if matches!(layer.timing, Timing::Strict) {
                    f.hold
                        .units(FPS)
                        .at(|| at(&format!("frames[{fi}].hold (strict timing)")))?;
                }
                total = total.plus(f.hold)?;
            }
        }
        if self.geometry.is_some() && self.layers.iter().any(|l| l.tilemap.is_some()) {
            return Err(error(
                "UNSUPPORTED_SCENE",
                "Tilemap layers are not supported in 3D geometry scenes",
            ));
        }
        if self.transparent {
            if self.geometry.is_some() {
                return Err(error(
                    "UNSUPPORTED_SCENE",
                    "transparent scenes do not support 3D geometry",
                ));
            }
            if let Some(layer) = self
                .layers
                .iter()
                .find(|l| !matches!(l.blend_mode, composite::BlendMode::Normal))
            {
                return Err(error(
                    "UNSUPPORTED_SCENE",
                    format!(
                        "transparent scenes composite normal-blend layers only; layer {:?} uses another blend mode",
                        layer.id
                    ),
                ));
            }
        }
        Ok(frames)
    }
}

pub(crate) fn identity_bytes(identity: &Identity, root: &Path) -> Result<(PathBuf, Vec<u8>)> {
    if !root.is_absolute()
        || identity.path.as_os_str().is_empty()
        || identity
            .path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || identity.path.to_string_lossy().contains(':')
    {
        return Err(error(
            "INVALID_PATH",
            "Scene media paths must be relative normal components inside an absolute input root",
        ));
    }
    if identity.bytes == 0
        || identity.bytes > 64 * 1024 * 1024
        || identity.sha256.len() != 64
        || !identity
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error(
            "INVALID_IDENTITY",
            "Expected lowercase SHA-256 and 1-64 MiB file size",
        ));
    }
    let path = media::allowed_file(&root.join(&identity.path), root)?;
    let label = identity.path.to_string_lossy();
    let size = fs::metadata(&path)?.len();
    if size != identity.bytes {
        return Err(changed_size(&label, identity.bytes, size, UPDATE_IDENTITY));
    }
    // Read at most the declared bounded length, even if a concurrent writer grows the file.
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(&path)?
        .take(identity.bytes + 1)
        .read_to_end(&mut bytes)?;
    let digest = format!("{:x}", Sha256::digest(&bytes));
    if bytes.len() as u64 != identity.bytes || digest != identity.sha256 {
        return Err(changed_digest(
            &label,
            &identity.sha256,
            &digest,
            UPDATE_IDENTITY,
        ));
    }
    Ok((path, bytes))
}

/// Decode with the original 512-pixel bound used by sequence, tracking and stabilization profiles.
/// Check a relative identity by size and a streamed SHA-256, without loading the file into memory.
/// Same path containment rules as `identity_bytes`; used for large media sources.
pub(crate) fn identity_file(identity: &Identity, root: &Path, max_bytes: u64) -> Result<PathBuf> {
    if !root.is_absolute()
        || identity.path.as_os_str().is_empty()
        || identity
            .path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        || identity.path.to_string_lossy().contains(':')
    {
        return Err(error(
            "INVALID_PATH",
            "Media paths must be relative normal components inside an absolute input root",
        ));
    }
    if identity.bytes == 0
        || identity.bytes > max_bytes
        || identity.sha256.len() != 64
        || !identity
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(error(
            "INVALID_IDENTITY",
            format!("Expected lowercase SHA-256 and a 1..{max_bytes}-byte file size"),
        ));
    }
    let path = media::allowed_file(&root.join(&identity.path), root)?;
    let label = identity.path.to_string_lossy();
    let size = fs::metadata(&path)?.len();
    if size != identity.bytes {
        return Err(changed_size(&label, identity.bytes, size, UPDATE_IDENTITY));
    }
    let digest = media::file_hash(&path)?;
    if digest != identity.sha256 {
        return Err(changed_digest(
            &label,
            &identity.sha256,
            &digest,
            UPDATE_IDENTITY,
        ));
    }
    let size = fs::metadata(&path)?.len();
    if size != identity.bytes {
        return Err(changed_size(&label, identity.bytes, size, UPDATE_IDENTITY));
    }
    Ok(path)
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<Pixels> {
    decode_png_bounded(bytes, 512)
}

pub(crate) fn decode_png_bounded(bytes: &[u8], max_side: u32) -> Result<Pixels> {
    let fail = |e: png::DecodingError| error("UNSUPPORTED_IMAGE", e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: (max_side as usize * max_side as usize * 4).max(16 * 1024 * 1024),
    });
    let mut reader = decoder.read_info().map_err(fail)?;
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > max_side
        || info.height > max_side
        || info.bit_depth != png::BitDepth::Eight
        || !matches!(info.color_type, png::ColorType::Rgb | png::ColorType::Rgba)
        || info.trns.is_some()
        || info.animation_control.is_some()
        || info.icc_profile.is_some()
        || info.coding_independent_code_points.is_some()
        || info.exif_metadata.is_some()
        || info.mastering_display_color_volume.is_some()
        || info.content_light_level.is_some()
        || (info.srgb.is_none() && (info.gama_chunk.is_some() || info.chrm_chunk.is_some()))
    {
        return Err(error(
            "UNSUPPORTED_IMAGE",
            format!(
                "Expected plain or sRGB 8-bit RGB/RGBA PNG up to {max_side} per axis; APNG, ICC, alternate gamma/chromaticity, HDR and orientation metadata are unsupported"
            ),
        ));
    }
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| invalid("PNG size overflow"))?
    ];
    let out = reader.next_frame(&mut buf).map_err(fail)?;
    reader.finish().map_err(fail)?;
    buf.truncate(out.buffer_size());
    let rgba = if out.color_type == png::ColorType::Rgb {
        buf.as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect()
    } else {
        buf
    };
    Ok(Pixels {
        width: out.width,
        height: out.height,
        rgba,
    })
}

fn prepare(scene: &Scene, root: &Path) -> Result<Prepared> {
    let frames = scene.validate()?;
    let exposure = crate::temporal::Plan::new(scene, frames)?;
    let geometry = crate::geometry::prepare(scene, &exposure.times)?;
    let expression_samples = if scene.expressions.is_some() {
        let graph = crate::expressions::Prepared::new(scene)?;
        graph.check_render_work(exposure.times.len())?;
        Some(
            exposure
                .times
                .iter()
                .map(|time| match time {
                    Some(time) => graph.sample(*time).map(|s| s.bindings),
                    None => Ok(Vec::new()),
                })
                .collect::<Result<Vec<_>>>()?,
        )
    } else {
        None
    };
    media::input_root(root)?;
    let mut images = HashMap::<PathBuf, Pixels>::new();
    let mut matted = HashMap::new();
    let mut graphics = HashMap::new();
    let mut font_cache = crate::graphics::Cache::default();
    let mut sources = Vec::<(PathBuf, Identity)>::new();
    let mut pixels = 0u64;
    let mut source_bytes = 0u128;
    let mut timing = Vec::new();
    let mut parameters = Vec::new();
    let mut selections = Vec::new();
    let mut tile_selections = Vec::new();
    let mut alpha_checked = HashSet::new();
    for layer in &scene.layers {
        let selected = exposure
            .times
            .iter()
            .map(|time| {
                time.map(|time| select_at(layer, time))
                    .transpose()
                    .map(Option::flatten)
            })
            .collect::<Result<Vec<_>>>()?;
        let sampled = sample_parameters(
            layer,
            &selected,
            &exposure.times,
            expression_samples.as_deref(),
        )?;
        selections.push(selected.clone());
        let mut total = Time::ZERO;
        let graphic_report = if let Some(graphic) = &layer.graphics {
            pixels += layer.canvas[0] as u64 * layer.canvas[1] as u64;
            if pixels > MAX_DECODED_PIXELS as u64 {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Decoded scene image budget exceeded",
                ));
            }
            let (rgba, report) =
                crate::graphics::rasterize(graphic, layer.canvas, root, &mut font_cache)?;
            graphics.insert(
                layer.id.clone(),
                Pixels {
                    width: layer.canvas[0],
                    height: layer.canvas[1],
                    rgba,
                },
            );
            total = layer.duration;
            Some(report)
        } else {
            None
        };
        if layer.tilemap.is_some() {
            // The assembled tile canvas exists once per composited sample.
            pixels += layer.canvas[0] as u64 * layer.canvas[1] as u64;
            total = layer.duration;
        }
        let tile_size = layer.tilemap.as_ref().map(|m| m.tile_size);
        let frame_list = layer
            .frames
            .iter()
            .map(|f| (f, layer.canvas, true))
            .chain(
                layer
                    .tilemap
                    .iter()
                    .flat_map(|m| &m.tiles)
                    .flat_map(|t| &t.frames)
                    .map(|f| (f, tile_size.expect("tilemap"), false)),
            )
            .collect::<Vec<_>>();
        for (frame, bound, layer_frame) in frame_list {
            for identity in std::iter::once(&frame.image).chain(frame.matte.iter()) {
                if let Some((_, previous)) = sources
                    .iter()
                    .find(|(_, previous)| previous.path == identity.path)
                {
                    if previous.sha256 != identity.sha256 || previous.bytes != identity.bytes {
                        return Err(error(
                            "INVALID_IDENTITY",
                            "One scene path cannot have conflicting identities",
                        ));
                    }
                } else {
                    source_bytes += identity.bytes as u128;
                    if source_bytes > 256 * 1024 * 1024 {
                        return Err(error(
                            "LIMIT_EXCEEDED",
                            "Scene PNG source byte budget exceeds 256 MiB",
                        ));
                    }
                    let (path, bytes) = identity_bytes(identity, root)?;
                    let image = decode_png_bounded(&bytes, MAX_CANVAS)?;
                    pixels += image.width as u64 * image.height as u64;
                    if pixels > MAX_DECODED_PIXELS as u64 {
                        return Err(error(
                            "LIMIT_EXCEEDED",
                            "Decoded scene image budget exceeded",
                        ));
                    }
                    images.insert(identity.path.clone(), image);
                    sources.push((path, identity.clone()));
                }
            }
            if let Some(matte) = &frame.matte {
                let key = (frame.image.path.clone(), matte.path.clone());
                if let std::collections::hash_map::Entry::Vacant(entry) = matted.entry(key) {
                    let image = &images[&frame.image.path];
                    let mask = &images[&matte.path];
                    if image.width != mask.width
                        || image.height != mask.height
                        || mask.rgba.as_chunks::<4>().0.iter().any(|p| {
                            p[3] != 255 || p[0] != p[1] || p[0] != p[2] || ![0, 255].contains(&p[0])
                        })
                    {
                        return Err(error(
                            "INVALID_MATTE",
                            "Frame matte must be an opaque binary RGB PNG with the same dimensions as its source image",
                        ));
                    }
                    pixels += image.width as u64 * image.height as u64;
                    if pixels > MAX_DECODED_PIXELS as u64 {
                        return Err(error(
                            "LIMIT_EXCEEDED",
                            "Decoded and matted scene image budget exceeded",
                        ));
                    }
                    let mut rgba = image.rgba.clone();
                    for (p, m) in rgba
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(mask.rgba.as_chunks::<4>().0.iter())
                    {
                        if m[0] == 0 {
                            p.fill(0);
                        }
                    }
                    entry.insert(Pixels {
                        width: image.width,
                        height: image.height,
                        rgba,
                    });
                }
            }
            let image = &images[&frame.image.path];
            if matches!(layer.alpha_mode, AlphaMode::Premultiplied)
                && alpha_checked.insert(frame.image.path.clone())
            {
                composite::validate_alpha(&image.rgba, layer.alpha_mode)?;
            }
            if frame.offset[0] as u64 + image.width as u64 > bound[0] as u64
                || frame.offset[1] as u64 + image.height as u64 > bound[1] as u64
            {
                return Err(invalid(&format!(
                    "Layer {:?}: PNG {} plus trim offset must fit its declared {}",
                    layer.id,
                    frame.image.path.display(),
                    if layer_frame {
                        "full source canvas"
                    } else {
                        "tile size"
                    }
                )));
            }
            if layer_frame {
                total = total.plus(frame.hold)?;
            }
        }
        let tile_selection = layer
            .tilemap
            .as_ref()
            .map(|map| {
                map.tiles
                    .iter()
                    .map(|tile| {
                        exposure
                            .times
                            .iter()
                            .zip(&selected)
                            .map(|(time, active)| match (time, active) {
                                (Some(time), Some(_)) => {
                                    select_cycle(&tile.frames, &tile.end, time.minus(layer.start)?)
                                }
                                _ => Ok(None),
                            })
                            .collect::<Result<Vec<_>>>()
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let mut layer_report = json!({"layer_id":layer.id,"source_cycle_duration":total,"timing":layer.timing,"end":layer.end,"selected_frames":runs(&selected),"sampled_parameters":runs(&sampled),"alpha_mode":layer.alpha_mode,"blend_mode":layer.blend_mode,"mask_inverted":layer.mask.as_ref().map(|m|m.inverted)});
        if let Some(report) = graphic_report {
            layer_report["graphics"] = report;
        }
        if let (Some(map), Some(chosen)) = (&layer.tilemap, &tile_selection) {
            let (columns, rows) = map.grid();
            layer_report["tilemap"] = json!({"grid":[columns,rows],"tile_size":map.tile_size,"tiles":map.tiles.len(),
                "occupied_cells":map.cells.iter().flatten().filter(|c|c.is_some()).count(),"tile_selected_frames":chosen});
        }
        tile_selections.push(tile_selection);
        if !layer.effects.is_empty() {
            layer_report["effects"] = json!(layer.effects);
            layer_report["effect_processing"] =
                json!("linear_srgb_f64_to_straight_srgb_u8_before_compositing");
        }
        if let Some(feather) = layer.mask.as_ref().and_then(|m| m.feather.as_ref()) {
            layer_report["mask_feather"] = json!(feather);
        }
        if let Some(spatial) = &layer.transform.spatial {
            layer_report["spatial"] = json!(spatial);
            layer_report["spatial_processing"] = crate::spatial::capabilities();
        }
        timing.push(layer_report);
        parameters.push(sampled);
    }
    sources.extend(font_cache.sources());
    let mut pcm = vec![0i16; frames as usize * 1920 * 2];
    let mut audio_report = json!({"silence":true,"output_samples":frames*1920});
    if let Some(audio) = &scene.audio {
        let (path, bytes) = identity_bytes(&audio.file, root)?;
        // Restrict the layout contract to classic mono/stereo PCM, where channel order
        // is unambiguous. Hound also accepts extensible masks that this profile cannot express.
        let mut chunk = 12usize;
        let mut classic_pcm = false;
        while let Some(header) = bytes.get(chunk..chunk.saturating_add(8)) {
            let size =
                u32::from_le_bytes(header[4..8].try_into().expect("four-byte size")) as usize;
            if &header[..4] == b"fmt " {
                classic_pcm = size >= 16 && bytes.get(chunk + 8..chunk + 10) == Some(&[1, 0]);
                break;
            }
            chunk = chunk
                .saturating_add(8)
                .saturating_add(size)
                .saturating_add(size % 2);
        }
        if !classic_pcm {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Only classic PCM WAV is supported; extensible channel masks are not inferred",
            ));
        }
        let mut reader = hound::WavReader::new(Cursor::new(bytes))
            .map_err(|e| error("UNSUPPORTED_AUDIO", e.to_string()))?;
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int
            || spec.bits_per_sample != 16
            || ![24000, 44100, 48000].contains(&spec.sample_rate)
            || !matches!(
                (&audio.channels, spec.channels),
                (Channels::DuplicateMono, 1) | (Channels::PreserveStereo, 2)
            )
        {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Expected PCM16 WAV at 24/44.1/48 kHz, with explicit duplicate_mono or preserve_stereo mapping",
            ));
        }
        let source = reader
            .samples::<i16>()
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| error("UNSUPPORTED_AUDIO", e.to_string()))?;
        if source.is_empty() || !source.len().is_multiple_of(spec.channels as usize) {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Empty or incomplete WAV samples",
            ));
        }
        let count = source.len() as u64 / spec.channels as u64;
        let converted = (count * 48000 + spec.sample_rate as u64 / 2) / spec.sample_rate as u64;
        let start = audio.start.units(RATE)?;
        if start as u128 + converted as u128 > frames as u128 * 1920 {
            return Err(error(
                "AUDIO_OVERFLOW",
                "Narration exceeds the scene; extend the scene explicitly, never truncate or stretch",
            ));
        }
        for n in 0..converted {
            let q = n * spec.sample_rate as u64;
            let a = q / 48000;
            let rem = (q % 48000) as i64;
            let b = (a + 1).min(count - 1);
            for ch in 0..2 {
                let c = if spec.channels == 1 { 0 } else { ch };
                let x = source[(a * spec.channels as u64 + c) as usize] as i64;
                let y = source[(b * spec.channels as u64 + c) as usize] as i64;
                let weighted = x * (48000 - rem) + y * rem;
                // Round nearest with exact half ties away from zero; endpoint extends last sample.
                pcm[((start + n) * 2 + ch) as usize] =
                    ((weighted.abs() + 24000) / 48000 * weighted.signum()) as i16;
            }
        }
        audio_report = json!({"source_samples":count,"source_rate":spec.sample_rate,"source_channels":spec.channels,"converted_samples":converted,"start_sample":start,"output_samples":frames*1920,"resampling":"linear-nearest-ties-away-v1","channel_mapping":audio.channels,"padding":"silence","source_duration":Time::new(count,spec.sample_rate as u64)?,"converted_duration":Time::new(converted,48000)?});
        sources.push((path, audio.file.clone()));
    }
    if let Some(mix) = &scene.audio_mix {
        let prepared = crate::audio::prepare(mix, root)?;
        if prepared.layout != crate::pcm_wave::Layout::Stereo {
            return Err(error(
                "UNSUPPORTED_AUDIO",
                "Scene soundtracks require explicit stereo output routing",
            ));
        }
        pcm = prepared.pcm;
        audio_report = prepared.report;
        sources.extend(prepared.sources);
    }
    let mut report = json!({"profile":"pixel-scene-v1","scene_id":scene.id,"frames":frames,"samples":frames*1920,"width":scene.width*scene.output_scale,"height":scene.height*scene.output_scale,"color":"srgb-opaque-composited-in-encoded-srgb","timing":timing,"audio":audio_report,"sources":sources.iter().map(|(path,i)|json!({"path":path,"identity":i})).collect::<Vec<_>>()});
    report["frame_matte"] = json!({"profile":"binary-source-matte-v1","matted_pairs":matted.len(),"sampling":"same held frame as source; applied before effects and spatial filtering"});
    if let Some(samples) = expression_samples {
        report["expressions"] = json!({"profile":"typed-property-graph-v1","program":scene.expressions,"frame_bindings":samples});
    }
    if let Some(temporal) = exposure.report {
        report["temporal"] = temporal;
    }
    if let Some(geometry) = &geometry {
        report["geometry"] = geometry.report.clone();
    }
    Ok(Prepared {
        images,
        matted,
        graphics,
        sources,
        pcm,
        report,
        parameters,
        selected: selections,
        tiles: tile_selections,
        samples_per_frame: exposure.samples,
        geometry,
    })
}

fn sample_parameters(
    layer: &Layer,
    selected: &[Option<usize>],
    times: &[Option<Time>],
    expressions: Option<&[Vec<crate::expressions::Bound>]>,
) -> Result<Vec<Option<Parameters>>> {
    let spatial = layer
        .transform
        .spatial
        .as_ref()
        .map(|s| s.prepare(layer.duration))
        .transpose()?;
    let effects = crate::effects::prepare(&layer.effects, layer.duration)?;
    let mask = layer
        .mask
        .as_ref()
        .map(|m| m.prepare(layer.duration))
        .transpose()?;
    let (mut x, mut y, mut opacity) = (None, None, None);
    if let Some(animation) = &layer.animation {
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
            .map(|c| c.prepare(layer.duration, -32768, 32768))
            .transpose()?;
        y = animation
            .position_y
            .as_ref()
            .map(|c| c.prepare(layer.duration, -32768, 32768))
            .transpose()?;
        opacity = animation
            .opacity
            .as_ref()
            .map(|c| c.prepare(layer.duration, 0, 255))
            .transpose()?;
    }
    selected
        .iter()
        .enumerate()
        .map(|(n, frame)| {
            if frame.is_none() {
                return Ok(None);
            }
            let time = times[n]
                .expect("selected sample is within the scene")
                .minus(layer.start)?;
            let mut position = [
                x.as_ref()
                    .map(|c| c.sample(time))
                    .transpose()?
                    .unwrap_or(layer.transform.position[0]),
                y.as_ref()
                    .map(|c| c.sample(time))
                    .transpose()?
                    .unwrap_or(layer.transform.position[1]),
            ];
            let mut sampled_opacity = opacity
                .as_ref()
                .map(|c| c.sample(time))
                .transpose()?
                .map(|v| v as u8)
                .unwrap_or(layer.transform.opacity);
            if let Some(samples) = expressions {
                for binding in &samples[n] {
                    if binding.layer == layer.id {
                        match binding.property {
                            crate::expressions::Property::Position => {
                                position = [binding.rounded[0], binding.rounded[1]]
                            }
                            crate::expressions::Property::Opacity => {
                                sampled_opacity = binding.rounded[0] as u8
                            }
                        }
                    }
                }
            }
            let mapped = spatial
                .as_ref()
                .map(|sampler| {
                    let anchor = if layer.graphics.is_some() || layer.tilemap.is_some() {
                        [0, 0]
                    } else {
                        layer.frames[frame.expect("selected")].anchor
                    };
                    sampler.mapping(
                        layer.transform.spatial.as_ref().expect("prepared"),
                        &layer.transform,
                        anchor,
                        position,
                        time,
                    )
                })
                .transpose()?;
            Ok(Some(Parameters {
                spatial: mapped,
                effects: effects
                    .iter()
                    .map(|e| e.sample(time))
                    .collect::<Result<_>>()?,
                mask_rect: mask.as_ref().map(|m| m.sample(time)).transpose()?,
                position,
                opacity: sampled_opacity,
            }))
        })
        .collect()
}

pub(crate) fn select_frame(layer: &Layer, n: u64) -> Result<Option<usize>> {
    select_at(layer, Time::new(n, 25)?)
}

fn select_at(layer: &Layer, time: Time) -> Result<Option<usize>> {
    if time.compare(layer.start)?.is_lt()
        || !time.compare(layer.start.plus(layer.duration)?)?.is_lt()
    {
        return Ok(None);
    }
    if layer.graphics.is_some() || layer.tilemap.is_some() {
        return Ok(Some(0));
    }
    select_cycle(&layer.frames, &layer.end, time.minus(layer.start)?)
}

/// Select a held frame at a time relative to the start of its animation.
fn select_cycle(frames: &[Frame], end: &End, mut time: Time) -> Result<Option<usize>> {
    let total = frames.iter().try_fold(Time::ZERO, |a, f| a.plus(f.hold))?;
    if time.compare(total)? != std::cmp::Ordering::Less {
        match end {
            End::Transparent => return Ok(None),
            End::HoldLast => return Ok(Some(frames.len() - 1)),
            End::Loop => {
                let cycles =
                    (time.num as u128 * total.den as u128) / (time.den as u128 * total.num as u128);
                // Remainder directly in rational arithmetic; no per-hold rounding or accumulated drift.
                let n = time.num as u128 * total.den as u128
                    - cycles * total.num as u128 * time.den as u128;
                let d = time.den as u128 * total.den as u128;
                let gcd = |mut a: u128, mut b: u128| {
                    while b != 0 {
                        (a, b) = (b, a % b);
                    }
                    a
                };
                let g = gcd(n, d);
                time = Time::new(
                    u64::try_from(n / g).map_err(|_| invalid("Time overflow"))?,
                    u64::try_from(d / g).map_err(|_| invalid("Time overflow"))?,
                )?;
            }
        }
    }
    let mut end = Time::ZERO;
    for (i, frame) in frames.iter().enumerate() {
        end = end.plus(frame.hold)?;
        if time.compare(end)? == std::cmp::Ordering::Less {
            return Ok(Some(i));
        }
    }
    Err(invalid("Animation sampling exceeded duration"))
}

/// Run-length form of a per-frame report array, `[{"count": n, "value": v}, ...]` in frame order,
/// so held or static values cost one entry instead of one per frame.
fn runs<T: Serialize>(values: &[T]) -> Value {
    let mut out: Vec<Value> = Vec::new();
    for value in values {
        let value = json!(value);
        match out.last_mut() {
            Some(run) if run["value"] == value => {
                run["count"] = json!(run["count"].as_u64().unwrap_or(0) + 1);
            }
            _ => out.push(json!({"count":1,"value":value})),
        }
    }
    Value::Array(out)
}
/// Compose one sample. A transparent scene composes over black; with `matte`, every source pixel
/// becomes white with its own alpha, so the result accumulates 255 x alpha with the same weights
/// and rounding as the color pass.
fn compose_sample(
    scene: &Scene,
    prepared: &Prepared,
    sample: usize,
    matte: bool,
) -> Result<Vec<u8>> {
    let backdrop = if scene.transparent {
        [0, 0, 0]
    } else {
        scene.background
    };
    if let Some(geometry) = &prepared.geometry {
        return Ok(if let Some(state) = &geometry.states[sample] {
            compose_geometry(scene, prepared, state, sample)
        } else {
            scene
                .background
                .repeat((scene.width * scene.height) as usize)
        });
    }
    let mut rgb = backdrop.repeat((scene.width * scene.height) as usize);
    for (layer_index, layer) in scene.layers.iter().enumerate() {
        let Some(index) = prepared.selected[layer_index][sample] else {
            continue;
        };
        let assembled;
        let (image, offset, anchor) = if layer.graphics.is_some() {
            (&prepared.graphics[&layer.id], [0, 0], [0, 0])
        } else if let Some(map) = &layer.tilemap {
            assembled = assemble_tiles(map, layer, prepared, layer_index, sample);
            (&assembled, [0, 0], [0, 0])
        } else {
            let frame = &layer.frames[index];
            (prepared.frame_pixels(frame), frame.offset, frame.anchor)
        };
        let t = &layer.transform;
        let parameters = prepared.parameters[layer_index][sample]
            .as_ref()
            .expect("selected layer has parameters");
        let processor = crate::effects::Processor::new(&layer.effects, &parameters.effects);
        if let (Some(spec), Some(mapping)) = (&t.spatial, &parameters.spatial) {
            crate::spatial::draw(
                &mut rgb,
                [scene.width, scene.height],
                mapping,
                spec,
                t.crop,
                parameters.opacity,
                layer.blend_mode,
                |sx, sy| {
                    let coverage = layer
                        .mask
                        .as_ref()
                        .zip(parameters.mask_rect)
                        .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                            mask.coverage(rect, sx, sy)
                        });
                    if coverage == 0 {
                        return None;
                    }
                    let ix = sx - offset[0] as i64;
                    let iy = sy - offset[1] as i64;
                    if ix < 0 || iy < 0 || ix >= image.width as i64 || iy >= image.height as i64 {
                        return None;
                    }
                    let p: [u8; 4] = image.rgba[((iy * image.width as i64 + ix) * 4) as usize..]
                        [..4]
                        .try_into()
                        .expect("RGBA");
                    let processed = processor
                        .as_ref()
                        .and_then(|processor| processor.pixel(&p, layer.alpha_mode, [sx, sy]));
                    Some(match processed {
                        _ if matte => (
                            [255, 255, 255, processed.unwrap_or(p)[3]],
                            AlphaMode::Straight,
                            coverage,
                        ),
                        Some(p) => (p, AlphaMode::Straight, coverage),
                        None => (p, layer.alpha_mode, coverage),
                    })
                },
            );
            continue;
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
        let (span_x, span_y) = if t.quarter_turns % 2 == 0 {
            (cw as i64 * scale, ch as i64 * scale)
        } else {
            (ch as i64 * scale, cw as i64 * scale)
        };
        let x0 = left.clamp(0, scene.width as i64) as u32;
        let x1 = (left + span_x).clamp(0, scene.width as i64) as u32;
        let y0 = top.clamp(0, scene.height as i64) as u32;
        let y1 = (top + span_y).clamp(0, scene.height as i64) as u32;
        for dy in y0..y1 {
            for dx in x0..x1 {
                let rx = dx as i64 - left;
                let ry = dy as i64 - top;
                if rx < 0 || ry < 0 {
                    continue;
                }
                let (rx, ry) = (rx / scale, ry / scale);
                let (sx, sy) = match t.quarter_turns {
                    0 => (rx, ry),
                    1 => (ry, ch as i64 - 1 - rx),
                    2 => (cw as i64 - 1 - rx, ch as i64 - 1 - ry),
                    _ => (cw as i64 - 1 - ry, rx),
                };
                if sx < 0 || sy < 0 || sx >= cw as i64 || sy >= ch as i64 {
                    continue;
                }
                let coverage = layer
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
                        layer.alpha_mode,
                        [sx + offset[0] as i64, sy + offset[1] as i64],
                    )
                });
                let white;
                let (p, alpha_mode) = if let Some(processed) = &processed {
                    (processed.as_slice(), AlphaMode::Straight)
                } else {
                    (p, layer.alpha_mode)
                };
                let (p, alpha_mode) = if matte {
                    white = [255, 255, 255, p[3]];
                    (white.as_slice(), AlphaMode::Straight)
                } else {
                    (p, alpha_mode)
                };
                let dest = ((dy * scene.width + dx) * 3) as usize;
                for c in 0..3 {
                    rgb[dest + c] = if coverage == composite::MASK_WEIGHT {
                        composite::channel(
                            rgb[dest + c],
                            p[c],
                            p[3],
                            parameters.opacity,
                            layer.blend_mode,
                            alpha_mode,
                        )
                    } else {
                        composite::masked_channel(
                            rgb[dest + c],
                            p[c],
                            p[3],
                            parameters.opacity,
                            layer.blend_mode,
                            alpha_mode,
                            coverage,
                        )
                    };
                }
            }
        }
    }
    Ok(rgb)
}

/// Copy each occupied cell's selected tile frame (straight RGBA, trim offset applied) into a
/// transparent canvas. Cells never overlap, so no blending happens during assembly.
fn assemble_tiles(
    map: &Tilemap,
    layer: &Layer,
    prepared: &Prepared,
    layer_index: usize,
    sample: usize,
) -> Pixels {
    let [width, height] = layer.canvas;
    let [tw, th] = map.tile_size;
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    let chosen = prepared.tiles[layer_index]
        .as_ref()
        .expect("tilemap selections are prepared");
    for (r, row) in map.cells.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            let Some(tile) = cell.map(usize::from) else {
                continue;
            };
            let Some(index) = chosen[tile][sample] else {
                continue;
            };
            let frame = &map.tiles[tile].frames[index];
            let image = prepared.frame_pixels(frame);
            let x0 = c * tw as usize + frame.offset[0] as usize;
            let y0 = r * th as usize + frame.offset[1] as usize;
            let row_bytes = image.width as usize * 4;
            for y in 0..image.height as usize {
                let src = y * row_bytes;
                let dst = ((y0 + y) * width as usize + x0) * 4;
                rgba[dst..dst + row_bytes].copy_from_slice(&image.rgba[src..src + row_bytes]);
            }
        }
    }
    Pixels {
        width,
        height,
        rgba,
    }
}

fn compose_geometry(
    scene: &Scene,
    prepared: &Prepared,
    state: &crate::geometry::State,
    sample: usize,
) -> Vec<u8> {
    let processors = scene
        .layers
        .iter()
        .enumerate()
        .map(|(i, layer)| {
            prepared.parameters[i][sample]
                .as_ref()
                .and_then(|p| crate::effects::Processor::new(&layer.effects, &p.effects))
        })
        .collect::<Vec<_>>();
    crate::geometry::compose(scene, state, |i, sx, sy| {
        let layer = &scene.layers[i];
        let index = prepared.selected[i][sample]?;
        let parameters = prepared.parameters[i][sample].as_ref()?;
        let [x, y, width, height] = layer.transform.crop;
        if sx < x || sy < y || sx >= x + width || sy >= y + height || parameters.opacity == 0 {
            return None;
        }
        let coverage = layer
            .mask
            .as_ref()
            .zip(parameters.mask_rect)
            .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                mask.coverage(rect, sx as i64, sy as i64)
            });
        if coverage == 0 {
            return None;
        }
        let (image, offset) = if layer.graphics.is_some() {
            (&prepared.graphics[&layer.id], [0, 0])
        } else {
            let frame = &layer.frames[index];
            (prepared.frame_pixels(frame), frame.offset)
        };
        let ix = sx as i64 - offset[0] as i64;
        let iy = sy as i64 - offset[1] as i64;
        if ix < 0 || iy < 0 || ix >= image.width as i64 || iy >= image.height as i64 {
            return None;
        }
        let rgba: [u8; 4] = image.rgba[((iy * image.width as i64 + ix) * 4) as usize..][..4]
            .try_into()
            .expect("RGBA");
        let processed = processors[i]
            .as_ref()
            .and_then(|p| p.pixel(&rgba, layer.alpha_mode, [sx as i64, sy as i64]));
        let (rgba, alpha_mode) = if let Some(p) = processed {
            (p, AlphaMode::Straight)
        } else {
            (rgba, layer.alpha_mode)
        };
        if rgba[3] == 0 {
            return None;
        }
        Some(crate::geometry::Texel {
            rgba,
            alpha_mode,
            opacity: parameters.opacity,
            coverage,
            blend: layer.blend_mode,
        })
    })
}

/// One output frame: RGB, or straight RGBA for a transparent scene.
fn compose(scene: &Scene, prepared: &Prepared, frame: u64) -> Result<Vec<u8>> {
    if !scene.transparent {
        return compose_plane(scene, prepared, frame, false);
    }
    // Over black the color pass holds premultiplied color; the matte pass holds 255 x alpha.
    let color = compose_plane(scene, prepared, frame, false)?;
    let matte = compose_plane(scene, prepared, frame, true)?;
    Ok(straighten(&color, &matte))
}

/// Straight RGBA from premultiplied color over black and a 255 x alpha matte, rounding to nearest.
fn straighten(color: &[u8], matte: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(color.len() / 3 * 4);
    for (c, m) in color
        .as_chunks::<3>()
        .0
        .iter()
        .zip(matte.as_chunks::<3>().0)
    {
        let alpha = m[0];
        for &channel in c {
            rgba.push(match alpha {
                0 => 0,
                a => ((u32::from(channel) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8,
            });
        }
        rgba.push(alpha);
    }
    rgba
}

fn compose_plane(scene: &Scene, prepared: &Prepared, frame: u64, matte: bool) -> Result<Vec<u8>> {
    let samples = prepared.samples_per_frame;
    let first = frame as usize * samples;
    if samples == 1 {
        return compose_sample(scene, prepared, first, matte);
    }
    let mut sums = vec![0u32; (scene.width * scene.height * 3) as usize];
    for sample in first..first + samples {
        let rgb = compose_sample(scene, prepared, sample, matte)?;
        for (sum, value) in sums.iter_mut().zip(rgb) {
            *sum += value as u32;
        }
    }
    Ok(sums
        .into_iter()
        .map(|sum| ((sum + samples as u32 / 2) / samples as u32) as u8)
        .collect())
}

/// Owned scratch directory. Cleanup only the known files we created, never recursive traversal.
pub(crate) struct Scratch(pub PathBuf);
impl Scratch {
    pub(crate) fn new(parent: &Path) -> Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error("CLOCK_ERROR", "Clock before epoch"))?
            .as_nanos();
        let path = parent.join(format!(".cutbolt-scene-{}-{nonce}", std::process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        for name in [
            "video.rgb",
            "audio.pcm",
            "output.mkv",
            "output.wav",
            "frame.png",
            "source.rgb",
            "source.pcm",
            "captions.txt",
        ] {
            let _ = fs::remove_file(self.0.join(name));
        }
        let _ = fs::remove_dir(&self.0);
    }
}

pub fn inspect(scene: &Scene, root: &Path) -> Result<Value> {
    Ok(prepare(scene, root)?.report)
}

pub fn run(scene: &Scene, root: &Path, output_root: &Path, output: &Path) -> Result<Value> {
    let output = render::destination(output, output_root)?;
    let prepared = prepare(scene, root)?;
    let scratch = Scratch::new(output.parent().expect("validated parent"))?;
    let frames = scene.duration.units(FPS)?;
    let mut audio = BufWriter::new(File::create_new(scratch.0.join("audio.pcm"))?);
    for sample in &prepared.pcm {
        audio.write_all(&sample.to_le_bytes())?;
    }
    audio.flush()?;
    drop(audio);
    let temp = scratch.0.join("output.mkv");
    let (out_w, out_h) = (
        scene.width * scene.output_scale,
        scene.height * scene.output_scale,
    );
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(out_w, out_h);
    // Integer enlargement stays nearest-neighbour; native-size scenes skip the scaler entirely.
    let filter = if scene.output_scale == 1 {
        "setsar=1".to_string()
    } else {
        format!("scale={out_w}:{out_h}:flags=neighbor,setsar=1")
    };
    let args = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-n".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        if scene.transparent { "rgba" } else { "rgb24" }.into(),
        "-video_size".into(),
        format!("{}x{}", scene.width, scene.height),
        "-framerate".into(),
        "25".into(),
        "-i".into(),
        "pipe:0".into(),
        "-f".into(),
        "s16le".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-i".into(),
        scratch.0.join("audio.pcm").to_string_lossy().into_owned(),
        "-vf".into(),
        filter,
        "-c:v".into(),
        "ffv1".into(),
        "-level".into(),
        ffv1_level.into(),
        "-slices".into(),
        ffv1_slices.into(),
        "-pix_fmt".into(),
        if scene.transparent { "bgra" } else { "bgr0" }.into(),
        "-threads".into(),
        ffv1_slices.into(),
        "-c:a".into(),
        "pcm_s16le".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-f".into(),
        "matroska".into(),
        temp.to_string_lossy().into_owned(),
    ];
    let ffmpeg = media::version("ffmpeg")?;
    let ffprobe = media::version("ffprobe")?;
    // Allow composition time in addition to the original fixed encoding allowance.
    let megapixel_frames = frames * out_w as u64 * out_h as u64 / 1_000_000;
    let timeout = Duration::from_secs((180 + 2 * megapixel_frames).min(3600));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16) as u64;
    let shared = &prepared;
    media::feed_stdin(&media::tool("ffmpeg"), &args, timeout, |stdin| {
        // Compose frames in parallel batches; write them strictly in order. Composition is a pure
        // function of the prepared scene, so output is identical for any worker count.
        let mut next = 0u64;
        while next < frames {
            let batch = workers.min(frames - next);
            let composed = std::thread::scope(|scope| {
                let handles = (next..next + batch)
                    .map(|n| scope.spawn(move || compose(scene, shared, n)))
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("composition thread"))
                    .collect::<Result<Vec<_>>>()
            })?;
            for rgb in composed {
                stdin.write_all(&rgb)?;
            }
            next += batch;
        }
        Ok(())
    })?;
    let (out_w, out_h) = (
        scene.width * scene.output_scale,
        scene.height * scene.output_scale,
    );
    let verified = if scene.transparent {
        let (verified, alpha) = render::inspect_overlay(&temp, out_w, out_h, &media::Uncontrolled)?;
        if !alpha {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Transparent scene output lacks its alpha plane",
            ));
        }
        verified
    } else {
        render::inspect_reference(&temp, out_w, out_h, &media::Uncontrolled)?
    };
    if verified.frames != frames || verified.samples != frames * 1920 {
        return Err(error(
            "RENDER_VALIDATION_FAILED",
            "Scene output counts differ",
        ));
    }
    for (_, identity) in &prepared.sources {
        identity_bytes(identity, root)?;
    }
    let mut report = prepared.report;
    report["output"] = json!(output);
    report["sha256"] = json!(verified.sha256);
    report["ffmpeg"] = json!(ffmpeg);
    report["ffprobe"] = json!(ffprobe);
    report["scene_sha256"] = json!(format!("{:x}", Sha256::digest(serde_json::to_vec(scene)?)));
    report["asset"] = json!({"id":scene.id,"path":output,"duration":scene.duration,
        "identity":{"sha256":verified.sha256,"bytes":fs::metadata(&temp)?.len()}});
    media::publish(&temp, &output)?;
    Ok(report)
}

pub(crate) fn write_png(path: &Path, width: u32, height: u32, bytes: &[u8]) -> Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create_new(path)?), width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    // Reference projects do not yet declare color management; do not invent a color tag.
    let mut writer = encoder
        .write_header()
        .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    writer
        .write_image_data(bytes)
        .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    writer
        .finish()
        .map_err(|e| error("ENCODE_FAILED", e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_frames_straighten_premultiplied_color() {
        // Opaque, half-covered white, half-covered dark red, and untouched pixels.
        let color = [10, 200, 30, 128, 128, 128, 50, 0, 0, 0, 0, 0];
        let matte = [255, 255, 255, 128, 128, 128, 128, 128, 128, 0, 0, 0];
        assert_eq!(
            straighten(&color, &matte),
            [
                10, 200, 30, 255, 255, 255, 255, 128, 100, 0, 0, 128, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn transparent_scenes_reject_what_over_cannot_express() {
        let template: Value =
            serde_json::from_str(include_str!("../examples/title-card-template.json")).unwrap();
        let mut scene: Scene = serde_json::from_value(template["scene"].clone()).unwrap();
        scene.validate().unwrap();
        // Opaque scenes keep their serialization, so existing scene fingerprints are unchanged.
        assert!(
            serde_json::to_value(&scene)
                .unwrap()
                .get("transparent")
                .is_none()
        );
        scene.transparent = true;
        scene.validate().unwrap();
        assert_eq!(serde_json::to_value(&scene).unwrap()["transparent"], true);
        scene.layers[0].blend_mode = composite::BlendMode::Multiply;
        let error = scene.validate().unwrap_err();
        assert_eq!(error.code, "UNSUPPORTED_SCENE");
        assert!(error.message.contains("normal-blend"));
    }
}
