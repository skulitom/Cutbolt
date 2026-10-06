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
pub(crate) const MAX_LAYERS: usize = 64;
/// Longest scene at any of the native rates: 3,000 frames at 25 fps, 7,200 at 60 fps.
pub(crate) const MAX_SECONDS: u64 = 120;
pub(crate) const MAX_FRAMES: u64 = MAX_SECONDS * 25;
/// Frames in the longest scene at the fastest native rate, 60 fps. Shutter sampling and
/// expressions may keep as many per-sample records as such a scene keeps without them.
pub(crate) const MAX_UNSAMPLED_FRAMES: u64 = MAX_SECONDS * 60;
/// Bound on destination pixels the compositor may visit over a whole scene (see `Scene::work`).
/// It keeps the worst case near the former 600-frame, 16-layer, 8-megapixel maximum.
pub(crate) const MAX_COMPOSITED_PIXELS: u64 = 64_000_000_000;
pub(crate) const MAX_REFERENCES: usize = 1024;
pub(crate) const MAX_DECODED_PIXELS: usize = 64_000_000;
pub(crate) const MAX_TEXT_SIZE: u16 = 512;
pub(crate) const MAX_GLYPH: usize = 1024;
pub(crate) const MAX_TILES: usize = 256;
pub(crate) const MAX_TILE_CELLS: usize = 4096;

pub(crate) fn limits() -> Value {
    json!({"canvas_per_axis":[1,MAX_CANVAS],"output_scale":[1,8],"maximum_output_pixels":MAX_OUTPUT_PIXELS,
        "frames":[1,MAX_FRAMES],"frame_rate":{"num":25,"den":1},"frame_rates":"eight_native_rates_default_25","maximum_seconds":MAX_SECONDS,
        "frames_at_60_fps":[1,MAX_SECONDS*60],"layers":[1,MAX_LAYERS],"maximum_frame_references":MAX_REFERENCES,
        "maximum_composited_pixels":MAX_COMPOSITED_PIXELS,"composited_pixels":"per_active_sample_clipped_destination_rectangle_plus_tilemap_canvas_whole_scene_for_spatial_or_3d_twice_when_transparent;unchanging_bottom_layers_once;plus_whole_scene_per_shutter_sample",
        "unchanging_layers":"whole_scene_one_held_image_no_curves_bindings_animated_effects_masks_or_spatial_and_every_layer_below_also_unchanging",
        "maximum_decoded_pixels":MAX_DECODED_PIXELS,"decoded_pixels":"png_sources_mattes_and_visible_bounds_of_graphics","png_per_axis":[1,MAX_CANVAS],"png_maximum_bytes":64*1024*1024,
        "png_total_bytes":256*1024*1024,"text_size":[1,MAX_TEXT_SIZE],"glyph_bitmap_per_axis":MAX_GLYPH,
        "integer_layer_scale":[1,16],"tilemap":{"maximum_tiles":MAX_TILES,"maximum_cells":MAX_TILE_CELLS,"tile_size_per_axis":[1,MAX_CANVAS],
        "empty_cells":"null","assembly":"straight_rgba_copy_into_layer_canvas_before_mask_effects_transform"},
        "encoding":"streamed_rgb24_frames_parallel_composition_deterministic_order"})
}

/// A source file inside the input root, pinned by its SHA-256 and size.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// Path relative to `input_root`; no `..`, drive or absolute parts.
    pub path: PathBuf,
    /// Lowercase hex SHA-256 of the contents. When omitted, the file is hashed as the request runs.
    #[serde(default)]
    pub sha256: String,
    /// Nonzero size in bytes, checked before use; read from the file when omitted.
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
    /// Positive display length, rational seconds; strict timing needs a whole number of scene frames.
    pub hold: Time,
    /// `[x, y]` pixel position of a trimmed PNG inside the layer canvas or tile; the image must fit.
    pub offset: [u32; 2],
    /// `[x, y]` full-canvas pivot (pixel corners) placed at the transform position; each -4096..4096.
    pub anchor: [i32; 2],
}
/// How frame holds map to the scene's frame clock: `strict` requires every hold to be whole frames; `sample_start` shows the frame containing each exact sample time.
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
    /// Boxed: a mapping is large, and long scenes hold one record per layer and frame.
    #[serde(skip_serializing_if = "Option::is_none")]
    spatial: Option<Box<crate::spatial::Mapping>>,
}
/// One reusable tile: an ordinary held-frame animation with its own clock, relative to layer start.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Tile {
    /// Ordered held frames, at least one; each must fit `tile_size`.
    pub frames: Vec<Frame>,
    /// How this tile's holds map to the scene's frame clock.
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
    fn validate(&self, layer: &Layer, rate: Time, at: &dyn Fn(&str) -> String) -> Result<()> {
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
                    f.hold.units(rate).at(path)?;
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
    /// Scene time the layer becomes active, rational seconds on the scene's frame grid.
    pub start: Time,
    /// Nonzero active length, rational seconds on the scene's frame grid; start plus duration must fit the scene.
    pub duration: Time,
    /// Ordered held PNG frames; leave empty when using `graphics` or `tilemap`.
    pub frames: Vec<Frame>,
    /// Text and shapes rasterized into the canvas; needs strict timing, hold_last and straight alpha.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics: Option<crate::graphics::Graphic>,
    /// Grid of animated tiles assembled into the canvas; needs strict timing and hold_last, and no geometry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tilemap: Option<Tilemap>,
    /// How frame holds map to the scene's frame clock.
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
impl Layer {
    /// Whether the recipe alone guarantees the same source pixels and sampled values at every
    /// exposure sample inside the scene: active for the whole scene, one held image (or one per
    /// tile) that never ends, and no position/opacity curves, expression bindings, animated mask
    /// or effect values, or time-varying spatial mapping. `static_base` treats such layers as
    /// unchanging, so the work estimate may count them once.
    fn unchanging(&self, scene: &Scene) -> Result<bool> {
        let held = |frames: &[Frame], end: &End| -> Result<bool> {
            Ok(match frames {
                [frame] => {
                    !matches!(end, End::Transparent) || !frame.hold.compare(self.duration)?.is_lt()
                }
                _ => false,
            })
        };
        let source = if self.graphics.is_some() {
            true
        } else if let Some(map) = &self.tilemap {
            let mut all = true;
            for tile in &map.tiles {
                all &= held(&tile.frames, &tile.end)?;
            }
            all
        } else {
            held(&self.frames, &self.end)?
        };
        Ok(source
            && self.start.num == 0
            && self.duration.compare(scene.duration)?.is_eq()
            && self.animation.is_none()
            && self.mask.as_ref().is_none_or(|m| m.animation.is_none())
            && self.effects.iter().all(crate::effects::Effect::is_constant)
            && self
                .transform
                .spatial
                .as_ref()
                .is_none_or(crate::spatial::Transform::is_constant)
            && !scene
                .expressions
                .as_ref()
                .is_some_and(|p| p.bindings.iter().any(|b| b.layer == self.id)))
    }
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
/// Bounded pixel scene of layers and audio, validated by scene.inspect and compiled by scene.render to a lossless asset at its frame_rate.
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
    /// Scene length, rational seconds; a whole number of frames at `frame_rate` and of 48 kHz samples, at most 120 seconds (3,000 frames at 25 fps, 7,200 at 60 fps).
    pub duration: Time,
    /// Output frame rate, one of the eight native rates; default 25. Match the timeline's rate so the compiled asset can be placed on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame_rate: Option<Time>,
    /// Opaque encoded sRGB `[r, g, b]` backdrop below all layers.
    pub background: [u8; 3],
    /// Color interpretation of sources and compositing.
    pub color: Color,
    /// Ordered layers, 1..64, with at most 1024 frame references in total.
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
    /// Rasterized graphics by layer ID, with the image's offset inside the layer canvas.
    graphics: HashMap<String, (Pixels, [u32; 2])>,
    sources: Vec<(PathBuf, Identity)>,
    pcm: Vec<i16>,
    report: Value,
    parameters: Vec<Vec<Option<Parameters>>>,
    selected: Vec<Vec<Option<usize>>>,
    /// For tilemap layers: per tile, the selected frame for every exposure sample.
    tiles: Vec<Option<Vec<Vec<Option<usize>>>>>,
    /// Exposure sample times, frame-major; None outside the scene, where samples show the backdrop.
    times: Vec<Option<Time>>,
    samples_per_frame: usize,
    geometry: Option<crate::geometry::Prepared>,
    /// Composite of the unchanging bottom layers, made once per render (see `static_base`).
    base: Option<Base>,
    /// Source images with constant effects already applied, by layer and frame index (see
    /// `processed_sources`).
    processed: HashMap<(usize, usize), Pixels>,
    /// Threads one frame may use to draw a large spatial layer in row bands: 1 inside the frame
    /// pool, more when a single frame is composed.
    threads: usize,
}

/// The first `layers` layers composited over the backdrop: color, and for transparent scenes the
/// matte pass.
struct Base {
    layers: usize,
    color: Vec<u8>,
    matte: Option<Vec<u8>>,
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
        let rate = self.clock()?;
        let frames = self.duration.units(rate).at(|| "duration".into())?;
        self.duration.units(RATE).at(|| "duration".into())?;
        if self.schema_version != 1
            || !bounded_id(&self.id)
            || !(1..=MAX_CANVAS).contains(&self.width)
            || !(1..=MAX_CANVAS).contains(&self.height)
            || !(1..=8).contains(&self.output_scale)
            || !(1..=max_frames(rate)).contains(&frames)
            || self.width as u64
                * self.height as u64
                * self.output_scale as u64
                * self.output_scale as u64
                > MAX_OUTPUT_PIXELS
            || self.layers.is_empty()
            || self.layers.len() > MAX_LAYERS
        {
            return Err(invalid(&format!(
                "Scene v1 requires 1 frame to {MAX_SECONDS} seconds of frames at its frame_rate ({} at {rate} fps), 1-{MAX_LAYERS} layers, canvas 1-{MAX_CANVAS} per axis, output scale 1-8 and at most 8M output pixels (got {}x{} x{}, {frames} frames, {} layers)",
                max_frames(rate),
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
            let start_units = layer.start.units(rate).at(|| at("start"))?;
            let duration_units = layer.duration.units(rate).at(|| at("duration"))?;
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
                map.validate(layer, rate, &at)?;
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
                        .units(rate)
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
        let (work, heaviest) = self.work(rate)?;
        if work > MAX_COMPOSITED_PIXELS {
            let (index, share) = heaviest;
            let accumulation = match self.accumulation(rate)? {
                0 => String::new(),
                pixels => format!(
                    "; averaging the shutter samples adds {pixels}, one whole-scene pass per sample"
                ),
            };
            return Err(error(
                "LIMIT_EXCEEDED",
                format!(
                    "Scene compositing work is {work} layer pixels over all frames, above the {MAX_COMPOSITED_PIXELS} budget; the largest share is layers[{index}] ({}) with {share}{accumulation}. The bottom {} layer(s) never change and count once; a layer counts once only when it and every layer below it last the whole scene with one held image and no curves, bindings, animated effects or masks. Shorten the scene or the layers' active ranges, crop or shrink large layers, move unchanging layers to the bottom, use fewer shutter samples, or split the scene",
                    self.layers[index].id,
                    self.static_layers()?
                ),
            ));
        }
        Ok(frames)
    }

    /// Bottom layers that `static_base` will certainly composite once per render, judged from
    /// the recipe before any media is read (see `Layer::unchanging`). 3D scenes have none.
    pub(crate) fn static_layers(&self) -> Result<usize> {
        if self.geometry.is_some() {
            return Ok(0);
        }
        let mut count = 0;
        for layer in &self.layers {
            if !layer.unchanging(self)? {
                break;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Upper bound on destination pixels the compositor visits over the whole scene, before any
    /// media is read, and the layer contributing most. Each active sample of a layer costs its
    /// transformed crop clipped to the scene (the whole scene for spatial or 3D layers, whose
    /// footprint depends on sampled values) plus the canvas a tilemap assembles; a transparent
    /// scene composes a color and a matte pass. The unchanging bottom layers (`static_layers`)
    /// are composited once per render and count once. With shutter sampling every layer counts
    /// at every sample, and averaging adds one whole-scene pass per sample (`accumulation`).
    /// Call after the layer checks of `validate`.
    pub(crate) fn work(&self, rate: Time) -> Result<(u64, (usize, u64))> {
        let scene = self.width as u64 * self.height as u64;
        let samples = self.temporal.as_ref().map_or(1, |t| t.samples.clamp(1, 32)) as u64;
        let passes = 1 + self.transparent as u64;
        let once = self.static_layers()?;
        let mut total = 0u64;
        let mut heaviest = (0, 0);
        for (index, layer) in self.layers.iter().enumerate() {
            let t = &layer.transform;
            let destination = if self.geometry.is_some() || t.spatial.is_some() {
                scene
            } else {
                let (w, h) = if t.quarter_turns % 2 == 0 {
                    (t.crop[2], t.crop[3])
                } else {
                    (t.crop[3], t.crop[2])
                };
                (w as u64 * t.scale as u64).min(self.width as u64)
                    * (h as u64 * t.scale as u64).min(self.height as u64)
            };
            let assembly = if layer.tilemap.is_some() {
                layer.canvas[0] as u64 * layer.canvas[1] as u64
            } else {
                0
            };
            let composited = if index < once {
                1
            } else {
                layer.duration.units(rate)?.saturating_mul(samples)
            };
            let share = composited
                .saturating_mul(passes)
                .saturating_mul(destination + assembly);
            if share > heaviest.1 {
                heaviest = (index, share);
            }
            total = total.saturating_add(share);
        }
        Ok((total.saturating_add(self.accumulation(rate)?), heaviest))
    }

    /// Pixels a shutter-sampled render visits to start and average its samples: the whole scene
    /// once per sample and pass, about as costly as one full-scene layer. Zero without sampling.
    fn accumulation(&self, rate: Time) -> Result<u64> {
        let samples = self.temporal.as_ref().map_or(1, |t| t.samples.clamp(1, 32)) as u64;
        if samples == 1 {
            return Ok(0);
        }
        Ok(self
            .duration
            .units(rate)?
            .saturating_mul(samples * (1 + self.transparent as u64))
            .saturating_mul(self.width as u64 * self.height as u64))
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
    for (li, layer) in scene.layers.iter().enumerate() {
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
        )
        .under(|| format!("layers[{li}] ({})", layer.id))?;
        selections.push(selected.clone());
        let mut total = Time::ZERO;
        let graphic_report = if let Some(graphic) = &layer.graphics {
            // The whole canvas exists while it is rasterized; only its visible part is kept.
            if pixels + layer.canvas[0] as u64 * layer.canvas[1] as u64 > MAX_DECODED_PIXELS as u64
            {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    format!(
                        "Decoded scene image budget exceeded at layer {:?}: {MAX_DECODED_PIXELS} pixels hold the PNG sources, mattes and visible parts of graphics",
                        layer.id
                    ),
                ));
            }
            let (rgba, report) =
                crate::graphics::rasterize(graphic, layer.canvas, root, &mut font_cache)?;
            let canvas = Pixels {
                width: layer.canvas[0],
                height: layer.canvas[1],
                rgba,
            };
            // Trimming relies on the integer path skipping pixels outside the image exactly as it
            // composites fully transparent ones; effects, spatial taps and 3D keep the canvas.
            let (image, offset) = if layer.effects.is_empty()
                && layer.transform.spatial.is_none()
                && scene.geometry.is_none()
            {
                trim(canvas)
            } else {
                (canvas, [0, 0])
            };
            pixels += image.width as u64 * image.height as u64;
            graphics.insert(layer.id.clone(), (image, offset));
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
                "occupied_cells":map.cells.iter().flatten().filter(|c|c.is_some()).count(),
                "tile_selected_frames":chosen.iter().map(|tile|runs(tile)).collect::<Vec<_>>()});
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
    let samples = scene.duration.units(RATE)?;
    let mut pcm = vec![0i16; samples as usize * 2];
    let mut audio_report = json!({"silence":true,"output_samples":samples});
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
        if start as u128 + converted as u128 > samples as u128 {
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
    let mut report = json!({"profile":"pixel-scene-v1","scene_id":scene.id,"frame_rate":scene.clock()?,"frames":frames,"samples":samples,"width":scene.width*scene.output_scale,"height":scene.height*scene.output_scale,"color":"srgb-opaque-composited-in-encoded-srgb","timing":timing,"audio":audio_report,"sources":sources.iter().map(|(path,i)|json!({"path":path,"identity":i})).collect::<Vec<_>>()});
    report["frame_matte"] = json!({"profile":"binary-source-matte-v1","matted_pairs":matted.len(),"sampling":"same held frame as source; applied before effects and spatial filtering"});
    let (work, _) = scene.work(scene.clock()?)?;
    report["work"] = json!({"composited_pixels":work,"maximum_composited_pixels":MAX_COMPOSITED_PIXELS,
        "static_layers":scene.static_layers()?,"decoded_pixels":pixels,"maximum_decoded_pixels":MAX_DECODED_PIXELS});
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
        times: exposure.times,
        samples_per_frame: exposure.samples,
        geometry,
        base: None,
        processed: HashMap::new(),
        threads: 1,
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
        .transpose()
        .under(|| "transform.spatial".into())?;
    let effects = crate::effects::prepare(&layer.effects, layer.duration)?;
    let mask = layer
        .mask
        .as_ref()
        .map(|m| m.prepare(layer.duration))
        .transpose()
        .under(|| "mask".into())?;
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
            .transpose()
            .under(|| "animation.position_x".into())?;
        y = animation
            .position_y
            .as_ref()
            .map(|c| c.prepare(layer.duration, -32768, 32768))
            .transpose()
            .under(|| "animation.position_y".into())?;
        opacity = animation
            .opacity
            .as_ref()
            .map(|c| c.prepare(layer.duration, 0, 255))
            .transpose()
            .under(|| "animation.opacity".into())?;
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
                spatial: mapped.map(Box::new),
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

pub(crate) fn select_frame(layer: &Layer, n: u64, rate: Time) -> Result<Option<usize>> {
    select_at(layer, Time::new(n * rate.den, rate.num)?)
}

/// Frames in the longest scene at `rate`: 3,000 at 25 fps, 7,200 at 60 fps.
pub(crate) fn max_frames(rate: Time) -> u64 {
    MAX_SECONDS * rate.num / rate.den
}

impl Scene {
    /// The validated output frame rate.
    pub(crate) fn clock(&self) -> Result<Time> {
        render::clock::rate(self.frame_rate.unwrap_or(FPS))
    }
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
/// Compose one sample into `rgb`, reusing its allocation. A transparent scene composes over
/// black; with `matte`, every source pixel becomes white with its own alpha, so the result
/// accumulates 255 x alpha with the same weights and rounding as the color pass.
fn compose_sample(
    scene: &Scene,
    prepared: &Prepared,
    sample: usize,
    matte: bool,
    mut rgb: Vec<u8>,
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
    let pixels = (scene.width * scene.height) as usize;
    // Outside the scene no layer is selected, so the sample is the bare backdrop.
    let inside = prepared.times[sample].is_some();
    let base = prepared.base.as_ref().filter(|_| inside);
    // Shutter sampling hands back the previous sample's buffer; a single sample starts afresh.
    let reuse = rgb.len() == pixels * 3;
    match base {
        Some(base) => {
            let start = if matte {
                base.matte
                    .as_ref()
                    .expect("transparent scenes keep a matte base")
            } else {
                &base.color
            };
            if reuse {
                rgb.copy_from_slice(start);
            } else {
                rgb = start.clone();
            }
        }
        None if reuse => fill(&mut rgb, backdrop),
        None => rgb = backdrop.repeat(pixels),
    }
    if !inside {
        return Ok(rgb);
    }
    let first = base.map_or(0, |b| b.layers);
    compose_layers(
        scene,
        prepared,
        sample,
        matte,
        first..scene.layers.len(),
        rgb,
    )
}

/// Fill packed RGB with one color, doubling the copied span as `[T]::repeat` does.
fn fill(rgb: &mut [u8], color: [u8; 3]) {
    if rgb.len() < 3 {
        return;
    }
    rgb[..3].copy_from_slice(&color);
    let mut filled = 3;
    while filled < rgb.len() {
        let span = filled.min(rgb.len() - filled);
        rgb.copy_within(..span, filled);
        filled += span;
    }
}

#[cfg(test)]
thread_local! {
    static REFERENCE_PATHS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
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

/// Composites `layers` of one exposure sample over `rgb`, in order.
fn compose_layers(
    scene: &Scene,
    prepared: &Prepared,
    sample: usize,
    matte: bool,
    layers: std::ops::Range<usize>,
    mut rgb: Vec<u8>,
) -> Result<Vec<u8>> {
    for layer_index in layers {
        let layer = &scene.layers[layer_index];
        let Some(index) = prepared.selected[layer_index][sample] else {
            continue;
        };
        let assembled;
        let (image, offset, anchor) = if layer.graphics.is_some() {
            let (image, offset) = &prepared.graphics[&layer.id];
            (image, *offset, [0, 0])
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
        let (image, processor) = match prepared.processed.get(&(layer_index, index)) {
            Some(processed) => (processed, None),
            None => (
                image,
                crate::effects::Processor::new(&layer.effects, &parameters.effects),
            ),
        };
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
            };
            let size = [scene.width, scene.height];
            let (opacity, blend) = (parameters.opacity, layer.blend_mode);
            #[cfg(test)]
            if reference_paths() {
                crate::spatial::reference(
                    &mut rgb,
                    size,
                    mapping,
                    spec,
                    t.crop,
                    opacity,
                    blend,
                    |sx, sy| {
                        let coverage = layer
                            .mask
                            .as_ref()
                            .zip(parameters.mask_rect)
                            .map_or(composite::MASK_WEIGHT, |(mask, rect)| {
                                mask.coverage(rect, sx, sy)
                            });
                        tap(sx, sy, coverage)
                    },
                );
                continue;
            }
            // Taps stay inside the crop, so the mask's axes cover the crop.
            let mask = layer
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
                .filter(|_| layer.alpha_mode.is_straight())
                .and_then(|processor| {
                    let (region, taps) =
                        crate::spatial::sampled_region(mapping, spec, limit, size)?;
                    (region[2] * region[3] <= taps).then(|| {
                        let pixels =
                            process_region(image, offset, region, processor, prepared.threads);
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
                    encoding: layer.alpha_mode,
                    white: matte,
                    mask: mask.as_ref(),
                }),
            };
            crate::spatial::draw(
                &mut rgb,
                size,
                mapping,
                spec,
                t.crop,
                opacity,
                blend,
                prepared.threads,
                plain,
                |sx, sy| {
                    let coverage = mask
                        .as_ref()
                        .map_or(composite::MASK_WEIGHT, |m| m.coverage(sx, sy));
                    tap(sx, sy, coverage)
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
        let (span_x, span_y) = if t.quarter_turns.is_multiple_of(2) {
            (cw as i64 * scale, ch as i64 * scale)
        } else {
            (ch as i64 * scale, cw as i64 * scale)
        };
        let x0 = left.clamp(0, scene.width as i64) as u32;
        let x1 = (left + span_x).clamp(0, scene.width as i64) as u32;
        let y0 = top.clamp(0, scene.height as i64) as u32;
        let y1 = (top + span_y).clamp(0, scene.height as i64) as u32;
        if parameters.opacity == 0 {
            // Zero coverage leaves every destination value unchanged in each blend mode.
            continue;
        }
        // Scaled source offsets of each destination column and row, divided once per layer and
        // row rather than per pixel. -1 marks a position before the layer, as no source pixel.
        if (processor.is_none() || layer.alpha_mode.is_straight())
            && layer.blend_mode.is_normal()
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
            let mask = layer
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
                let dest = (dy * scene.width as usize + (rx0 + left) as usize) * 3;
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
                            layer.alpha_mode,
                            matte,
                        );
                    }
                    _ => composite::normal_row(
                        dest,
                        row,
                        parameters.opacity,
                        layer.alpha_mode,
                        matte,
                    ),
                }
            }
            continue;
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

/// The bounding box of a straight-RGBA canvas's nonzero alpha, as an image and its offset in the
/// canvas; a fully transparent canvas becomes an empty image. A fully transparent straight pixel
/// leaves the destination unchanged in every blend mode, so drawing the trimmed image at its
/// offset on the integer path composites exactly as the whole canvas does.
fn trim(canvas: Pixels) -> (Pixels, [u32; 2]) {
    let (width, height) = (canvas.width as usize, canvas.height as usize);
    let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
    for (y, row) in canvas.rgba.chunks_exact(width * 4).enumerate() {
        for (x, pixel) in row.as_chunks::<4>().0.iter().enumerate() {
            if pixel[3] != 0 {
                (left, right) = (left.min(x), right.max(x));
                (top, bottom) = (top.min(y), y);
            }
        }
    }
    if left > right {
        let empty = Pixels {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        return (empty, [0, 0]);
    }
    if (left, top, right, bottom) == (0, 0, width - 1, height - 1) {
        return (canvas, [0, 0]);
    }
    let row_bytes = (right + 1 - left) * 4;
    let mut rgba = Vec::with_capacity(row_bytes * (bottom + 1 - top));
    for y in top..=bottom {
        let start = (y * width + left) * 4;
        rgba.extend_from_slice(&canvas.rgba[start..start + row_bytes]);
    }
    (
        Pixels {
            width: (right + 1 - left) as u32,
            height: (bottom + 1 - top) as u32,
            rgba,
        },
        [left as u32, top as u32],
    )
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
            let (image, offset) = &prepared.graphics[&layer.id];
            (image, *offset)
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
/// The bottom layers that look the same at every exposure sample inside the scene (same frame,
/// tile frames and sampled parameters), composited once. Compositing is sequential, rounding after
/// each layer, and each layer reads only its own inputs and the destination, so every frame that
/// continues from this base gets exactly the values of compositing all layers. Samples outside the
/// scene show the bare backdrop (see `compose_sample`). 3D scenes composite per sample.
fn static_base(scene: &Scene, prepared: &Prepared) -> Result<Option<Base>> {
    if prepared.geometry.is_some() {
        return Ok(None);
    }
    let inside = (0..prepared.times.len())
        .filter(|&s| prepared.times[s].is_some())
        .collect::<Vec<_>>();
    let Some(&first) = inside.first() else {
        return Ok(None);
    };
    let unchanging = |index: usize| -> Result<bool> {
        let selected = &prepared.selected[index];
        if inside.iter().any(|&s| selected[s] != selected[first]) {
            return Ok(false);
        }
        if let Some(tiles) = &prepared.tiles[index]
            && tiles
                .iter()
                .any(|t| inside.iter().any(|&s| t[s] != t[first]))
        {
            return Ok(false);
        }
        let parameters = &prepared.parameters[index];
        let reference = serde_json::to_vec(&parameters[first])?;
        for &s in &inside[1..] {
            if serde_json::to_vec(&parameters[s])? != reference {
                return Ok(false);
            }
        }
        Ok(true)
    };
    let mut layers = 0;
    while layers < scene.layers.len() && unchanging(layers)? {
        layers += 1;
    }
    if layers == 0 {
        return Ok(None);
    }
    let backdrop = if scene.transparent {
        [0, 0, 0]
    } else {
        scene.background
    };
    let blank = backdrop.repeat((scene.width * scene.height) as usize);
    let color = compose_layers(scene, prepared, first, false, 0..layers, blank.clone())?;
    let matte = if scene.transparent {
        Some(compose_layers(
            scene,
            prepared,
            first,
            true,
            0..layers,
            blank,
        )?)
    } else {
        None
    };
    Ok(Some(Base {
        layers,
        color,
        matte,
    }))
}

/// The work estimate counts `Scene::static_layers` once, so the render must composite at least
/// those layers once into its base; anything else would exceed the approved work.
fn approved_base(scene: &Scene, prepared: &Prepared) -> Result<()> {
    let counted = scene.static_layers()?;
    let cached = prepared.base.as_ref().map_or(0, |b| b.layers);
    if cached < counted {
        return Err(error(
            "LIMIT_EXCEEDED",
            format!(
                "The work estimate counted {counted} unchanging bottom layers once, but only {cached} composite once; the scene was not rendered"
            ),
        ));
    }
    Ok(())
}

/// Source images of layers above the base whose effects sample the same values at every exposure
/// sample, with the chain applied once per render instead of once per frame. Effects read only a
/// source pixel and its source-canvas position, so compositing the processed image without
/// effects gives the same values. Straight layers only: a pixel the chain leaves unchanged keeps
/// its straight encoding. Processed images total at most `MAX_DECODED_PIXELS`; layers beyond that
/// apply their effects per frame.
fn processed_sources(
    scene: &Scene,
    prepared: &Prepared,
) -> Result<HashMap<(usize, usize), Pixels>> {
    let mut processed = HashMap::new();
    if prepared.geometry.is_some() {
        return Ok(processed);
    }
    let first_layer = prepared.base.as_ref().map_or(0, |b| b.layers);
    let mut budget = MAX_DECODED_PIXELS;
    for (layer_index, layer) in scene.layers.iter().enumerate().skip(first_layer) {
        if layer.effects.is_empty() || !layer.alpha_mode.is_straight() || layer.tilemap.is_some() {
            continue;
        }
        let mut sampled = prepared.parameters[layer_index].iter().flatten();
        let Some(first) = sampled.next() else {
            continue;
        };
        let effects = serde_json::to_vec(&first.effects)?;
        let mut constant = true;
        for parameters in sampled {
            if serde_json::to_vec(&parameters.effects)? != effects {
                constant = false;
                break;
            }
        }
        if !constant {
            continue;
        }
        let Some(processor) = crate::effects::Processor::new(&layer.effects, &first.effects) else {
            continue;
        };
        let indices = prepared.selected[layer_index]
            .iter()
            .flatten()
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        for index in indices {
            let (image, offset) = if layer.graphics.is_some() {
                let (image, offset) = &prepared.graphics[&layer.id];
                (image, *offset)
            } else {
                let frame = &layer.frames[index];
                (prepared.frame_pixels(frame), frame.offset)
            };
            let Some(left) = budget.checked_sub(image.rgba.len() / 4) else {
                return Ok(processed);
            };
            budget = left;
            processed.insert(
                (layer_index, index),
                process_image(image, offset, &processor),
            );
        }
    }
    Ok(processed)
}

/// `image` (straight RGBA placed at `offset` in its source canvas) through an effect chain, in
/// parallel row bands.
fn process_image(
    image: &Pixels,
    offset: [u32; 2],
    processor: &crate::effects::Processor,
) -> Pixels {
    let workers = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 64);
    let [x, y] = offset.map(i64::from);
    let region = [x, y, i64::from(image.width), i64::from(image.height)];
    process_region(image, offset, region, processor, workers)
}

/// The `region` (`[x, y, width, height]` in the source canvas, inside the image) of `image`
/// (straight RGBA placed at `offset`) through an effect chain, in up to `threads` row bands.
fn process_region(
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

/// Composes frames 0..`frames` on `workers` threads and writes them strictly in order. Workers
/// take frames in order and never run more than `window` frames ahead of the writer, which bounds
/// memory; a slow frame no longer holds the other threads idle as a fixed batch did.
fn write_ordered(
    frames: u64,
    workers: usize,
    window: u64,
    compose: &(dyn Fn(u64) -> Result<Vec<u8>> + Sync),
    write: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    use std::sync::{
        Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    };
    let next = AtomicU64::new(0);
    // Frames written so far, and whether the writer has stopped.
    let progress = (Mutex::new((0u64, false)), Condvar::new());
    std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel::<(u64, Result<Vec<u8>>)>();
        for _ in 0..workers {
            let sender = sender.clone();
            let (next, progress) = (&next, &progress);
            scope.spawn(move || {
                loop {
                    let n = next.fetch_add(1, Ordering::Relaxed);
                    if n >= frames {
                        return;
                    }
                    {
                        let (lock, ready) = progress;
                        let mut state = lock.lock().expect("progress lock");
                        while !state.1 && n >= state.0 + window {
                            state = ready.wait(state).expect("progress lock");
                        }
                        if state.1 {
                            return;
                        }
                    }
                    if sender.send((n, compose(n))).is_err() {
                        return;
                    }
                }
            });
        }
        drop(sender);
        let outcome = (|| {
            let mut pending = std::collections::BTreeMap::new();
            let mut written = 0u64;
            for (n, frame) in receiver.iter() {
                pending.insert(n, frame?);
                while let Some(frame) = pending.remove(&written) {
                    write(&frame)?;
                    written += 1;
                    progress.0.lock().expect("progress lock").0 = written;
                    progress.1.notify_all();
                }
            }
            Ok(())
        })();
        // Release waiting workers and refuse further frames, whatever the outcome.
        progress.0.lock().expect("progress lock").1 = true;
        progress.1.notify_all();
        drop(receiver);
        outcome
    })
}

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
        return compose_sample(scene, prepared, first, matte, Vec::new());
    }
    // At most 32 samples of 255 sum to 8,160, so 16-bit sums are exact; one buffer serves every
    // sample of the frame and then holds the result.
    let mut sums = vec![0u16; (scene.width * scene.height * 3) as usize];
    let mut rgb = Vec::new();
    for sample in first..first + samples {
        rgb = compose_sample(scene, prepared, sample, matte, rgb)?;
        for (sum, value) in sums.iter_mut().zip(&rgb) {
            *sum += u16::from(*value);
        }
    }
    // The equal-weight mean of each possible sum, rounded to nearest with exact half ties upward.
    let count = samples as u32;
    let means = (0..=255 * count)
        .map(|sum| ((sum + count / 2) / count) as u8)
        .collect::<Vec<_>>();
    for (value, sum) in rgb.iter_mut().zip(&sums) {
        *value = means[usize::from(*sum)];
    }
    Ok(rgb)
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
            "sheet.png",
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
    let mut prepared = prepare(scene, root)?;
    let machine = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 64);
    prepared.threads = machine;
    prepared.base = static_base(scene, &prepared)?;
    approved_base(scene, &prepared)?;
    prepared.processed = processed_sources(scene, &prepared)?;
    let scratch = Scratch::new(output.parent().expect("validated parent"))?;
    let rate = scene.clock()?;
    let frames = scene.duration.units(rate)?;
    let samples = scene.duration.units(RATE)?;
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
    let mut args: Vec<String> = vec![
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
        format!("{}/{}", rate.num, rate.den),
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
    ];
    if rate != FPS {
        // Exact frame timestamps at fractional and high rates, as the renderers use.
        args.extend([
            "-r".into(),
            format!("{}/{}", rate.num, rate.den),
            "-fps_mode".into(),
            "cfr".into(),
            "-enc_time_base:v".into(),
            format!("{}/{}", rate.den, rate.num),
        ]);
    }
    args.push(temp.to_string_lossy().into_owned());
    let ffmpeg = media::version("ffmpeg")?;
    let ffprobe = media::version("ffprobe")?;
    // Allow composition time in addition to the original fixed encoding allowance.
    let megapixel_frames = frames * out_w as u64 * out_h as u64 / 1_000_000;
    let timeout = Duration::from_secs((180 + 2 * megapixel_frames).min(3600));
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    // With shutter sampling each worker also holds 16-bit sums and one sample (9 bytes a pixel);
    // together they stay within about 512 MiB.
    let scratch = if prepared.samples_per_frame > 1 {
        9 * pixels
    } else {
        0
    };
    let workers = machine.min(((512u64 << 20) / scratch.max(1)).max(1) as usize);
    // Processors the frame pool leaves idle (a short scene, or few workers within that memory)
    // draw large spatial layers' rows in bands instead.
    prepared.threads = (machine as u64 / frames.min(workers as u64)).max(1) as usize;
    // At least one frame per worker may wait for the writer, up to two within about 256 MiB.
    let frame_bytes = pixels * 4;
    let window = ((256u64 << 20) / frame_bytes).clamp(workers as u64, 2 * workers as u64);
    let shared = &prepared;
    media::feed_stdin(&media::tool("ffmpeg"), &args, timeout, |stdin| {
        // Composition is a pure function of the prepared scene, so output is identical for any
        // worker count; frames are written strictly in order.
        write_ordered(
            frames,
            workers,
            window,
            &|n| compose(scene, shared, n),
            &mut |rgb| Ok(stdin.write_all(rgb)?),
        )
    })?;
    let (out_w, out_h) = (
        scene.width * scene.output_scale,
        scene.height * scene.output_scale,
    );
    let verified = if scene.transparent {
        let (verified, alpha) =
            render::inspect_overlay(&temp, out_w, out_h, rate, &media::Uncontrolled)?;
        if !alpha {
            return Err(error(
                "RENDER_VALIDATION_FAILED",
                "Transparent scene output lacks its alpha plane",
            ));
        }
        verified
    } else {
        render::inspect_reference_at(&temp, out_w, out_h, rate, &media::Uncontrolled)?
    };
    if verified.frames != frames || verified.samples != samples {
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

/// One frame of a scene as a PNG at its output size, without compiling a movie: RGB, or straight
/// RGBA for a transparent scene. The frame is the one shown at `time`, as in the compiled asset.
pub fn still(
    scene: &Scene,
    root: &Path,
    output_root: &Path,
    output: &Path,
    time: Time,
) -> Result<Value> {
    let output = render::destination_extension(output, output_root, "png")?;
    let mut prepared = prepare(scene, root)?;
    prepared.threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 64);
    // As in a render, the unchanging bottom layers composite once rather than once per sample.
    prepared.base = static_base(scene, &prepared)?;
    approved_base(scene, &prepared)?;
    let rate = scene.clock()?;
    let frames = scene.duration.units(rate)?;
    // The frame on screen at `time`: the last frame start at or before it.
    let n = (time.num as u128 * rate.num as u128 / (time.den as u128 * rate.den as u128)) as u64;
    if n >= frames {
        return Err(error(
            "INVALID_RANGE",
            format!(
                "time {time} s is not inside the scene's {} s",
                scene.duration
            ),
        ));
    }
    let pixels = compose(scene, &prepared, n)?;
    let channels = if scene.transparent { 4 } else { 3 };
    let scale = scene.output_scale as usize;
    let (width, height) = (scene.width as usize, scene.height as usize);
    let mut enlarged = Vec::with_capacity(pixels.len() * scale * scale);
    for row in pixels.chunks(width * channels) {
        let mut line = Vec::with_capacity(row.len() * scale);
        for pixel in row.chunks(channels) {
            for _ in 0..scale {
                line.extend_from_slice(pixel);
            }
        }
        for _ in 0..scale {
            enlarged.extend_from_slice(&line);
        }
    }
    let scratch = Scratch::new(output.parent().expect("validated parent"))?;
    let temp = scratch.0.join("frame.png");
    let (out_w, out_h) = ((width * scale) as u32, (height * scale) as u32);
    write_png_channels(&temp, out_w, out_h, &enlarged, scene.transparent)?;
    for (_, identity) in &prepared.sources {
        identity_bytes(identity, root)?;
    }
    media::publish(&temp, &output)?;
    Ok(
        json!({"output":output,"scene_id":scene.id,"frame":n,"time":Time::new(n * rate.den, rate.num)?,
        "frame_rate":rate,"width":out_w,"height":out_h,"transparent":scene.transparent}),
    )
}

pub(crate) fn write_png(path: &Path, width: u32, height: u32, bytes: &[u8]) -> Result<()> {
    write_png_channels(path, width, height, bytes, false)
}

fn write_png_channels(
    path: &Path,
    width: u32,
    height: u32,
    bytes: &[u8],
    alpha: bool,
) -> Result<()> {
    let mut encoder = png::Encoder::new(BufWriter::new(File::create_new(path)?), width, height);
    encoder.set_color(if alpha {
        png::ColorType::Rgba
    } else {
        png::ColorType::Rgb
    });
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

    /// The title card with only its panel, a shape that rasterizes without files.
    fn panel_scene() -> Scene {
        let template: Value =
            serde_json::from_str(include_str!("../examples/title-card-template.json")).unwrap();
        let mut scene: Scene = serde_json::from_value(template["scene"].clone()).unwrap();
        scene.layers.truncate(1);
        scene
    }

    fn frames(n: u64, rate: Time) -> Time {
        Time::new(n * rate.den, rate.num).unwrap()
    }

    #[test]
    fn cached_bottom_layers_and_processed_sources_composite_identically() {
        let mut scene = panel_scene();
        let panel = scene.layers[0].clone();
        let effect =
            |value: Value| -> crate::effects::Effect { serde_json::from_value(value).unwrap() };
        let grade = effect(
            json!({"kind":"grade","exposure_milli":400,"contrast_milli":1200,"white_balance_milli":[1100,1000,900]}),
        );
        // A hue turn mixes channels, so its constant chain is processed per pixel.
        let turn = effect(
            json!({"kind":"grade","exposure_milli":-200,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000],"hue_shift_mdeg":70000,"saturation_milli":1400}),
        );
        let key = json!({"kind":"chroma_key","key_rgb":[28,56,89],"inner_milli":60,"outer_milli":300,"strength_milli":700,"unmix_milli":500,"spill":{"channel":"blue","strength_milli":500}});
        let mut fading_key = key.clone();
        fading_key["strength_curve"] = json!({"keys":[{"time":{"num":0,"den":1},"value":0,"interpolation":"linear"},{"time":{"num":3,"den":1},"value":1000,"interpolation":"hold"}]});
        let curve = |property: &str, to: i32| -> Option<Animation> {
            Some(serde_json::from_value(json!({property:{"keys":[{"time":{"num":0,"den":1},"value":0,"interpolation":"linear"},{"time":{"num":3,"den":1},"value":to,"interpolation":"hold"}]}})).unwrap())
        };
        let layer =
            |id: &str, effects: Vec<crate::effects::Effect>, animation: Option<Animation>| Layer {
                id: id.into(),
                effects,
                animation,
                ..panel.clone()
            };
        scene.layers = vec![
            layer("still", vec![], None),
            layer("graded", vec![grade.clone()], None),
            layer("moving-graded", vec![grade], curve("position_x", 40)),
            layer("moving-keyed", vec![effect(key)], curve("position_y", -30)),
            layer("fading-key", vec![effect(fading_key)], None),
            layer("fading", vec![], curve("opacity", 255)),
            layer("moving-turned", vec![turn], curve("position_x", -25)),
        ];
        for transparent in [false, true] {
            scene.transparent = transparent;
            let frames = scene.validate().unwrap();
            let root = std::env::temp_dir();
            let plain = prepare(&scene, &root).unwrap();
            let mut cached = prepare(&scene, &root).unwrap();
            cached.base = static_base(&scene, &cached).unwrap();
            cached.processed = processed_sources(&scene, &cached).unwrap();
            // The two unchanging layers form the base; the moving layers' constant chains are
            // applied once; the animated key strength and the fade stay per frame.
            assert_eq!(cached.base.as_ref().unwrap().layers, 2);
            let mut processed = cached.processed.keys().map(|k| k.0).collect::<Vec<_>>();
            processed.sort();
            assert_eq!(processed, [2, 3, 6]);
            for n in 0..frames {
                assert_eq!(
                    compose(&scene, &cached, n).unwrap(),
                    compose(&scene, &plain, n).unwrap(),
                    "frame {n}, transparent {transparent}"
                );
            }
        }
    }

    #[test]
    fn faster_masked_and_spatial_paths_composite_like_the_per_pixel_ones() {
        let mut scene = panel_scene();
        let length = frames(8, FPS);
        scene.duration = length;
        let panel = scene.layers[0].clone();
        let curve = |from: i32, to: i32| json!({"keys":[{"time":{"num":0,"den":1},"value":from,"interpolation":"linear"},{"time":{"num":8,"den":25},"value":to,"interpolation":"hold"}]});
        let layer = |id: &str, extra: Value| -> Layer {
            let mut value = serde_json::to_value(Layer {
                id: id.into(),
                duration: length,
                ..panel.clone()
            })
            .unwrap();
            for (key, field) in extra.as_object().unwrap() {
                if key == "spatial" || key == "position" || key == "opacity" {
                    value["transform"][key] = field.clone();
                } else {
                    value[key] = field.clone();
                }
            }
            serde_json::from_value(value).unwrap()
        };
        // Rotations off the quadrants map every pixel; axis-aligned ones map columns and rows.
        let spatial = |sampling: &str, edge: &str, rotation: i32, animation: Value| {
            json!({"translate_milli":[1250,-700],"scale_milli":[1100,900],"rotation_mdeg":rotation,"flip":[false,true],
                "pixel_aspect":{"num":16,"den":15},"sampling":sampling,"edge":edge,"animation":animation})
        };
        let mask = |rect: [i32; 4], inverted: bool, feather: Value, animation: Value| {
            let mut mask = json!({"rect":rect,"inverted":inverted,"animation":animation});
            if !feather.is_null() {
                mask["feather"] = feather;
            }
            mask
        };
        let key = json!({"kind":"chroma_key","key_rgb":[40,200,90],"inner_milli":120,"outer_milli":320,"strength_milli":800,
            "unmix_milli":400,"spill":{"channel":"green","strength_milli":500},"strength_curve":curve(300, 1000),
            "mask":{"rect":[30,20,90,50],"feather":6}});
        let selective = json!({"kind":"selective_grade","grade":{"exposure_milli":500,"contrast_milli":1000,"white_balance_milli":[1100,1000,900]},
            "mix_milli":800,"mix_curve":curve(100, 900),"qualifier":{"saturation":{"low":200,"high":1000,"feather":100},"inverted":false}});
        let grade = json!({"kind":"grade","exposure_milli":300,"contrast_milli":1100,"white_balance_milli":[1050,1000,950],
            "animation":{"exposure_milli":curve(-500, 800)}});
        // Hue turns and saturation need per-pixel processing on every path.
        let turned = json!({"kind":"grade","exposure_milli":200,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000],
            "hue_shift_mdeg":-60000,"animation":{"hue_shift_mdeg":curve(-90000, 200000),"saturation_milli":curve(0, 2500)}});
        let selective_turn = json!({"kind":"selective_grade","grade":{"exposure_milli":0,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000],
            "hue_shift_mdeg":55000,"saturation_milli":1300},"mix_milli":900,"qualifier":{"hue":{"center":350000,"inner":20000,"outer":60000},"inverted":false}});
        let centered = json!({"radius":12,"edge":"centered"});
        let layers = vec![
            // Unchanging, so it joins the static base.
            layer(
                "static-masked-row",
                json!({"mask":mask([12, 8, 120, 70], false, json!({"radius":20,"edge":"outer"}), Value::Null)}),
            ),
            layer(
                "masked-row",
                json!({"mask":mask([20, 10, 100, 60], false, centered.clone(), json!({"x":curve(-10, 70)})),
                    "animation":{"opacity":curve(255, 140)}}),
            ),
            layer(
                "masked-row-graded",
                json!({"mask":mask([-20, 30, 300, 25], true, json!({"radius":5,"edge":"outer"}), json!({"height":curve(0, 40)})),
                    "effects":[grade.clone()], "position":[-37, 21]}),
            ),
            layer(
                "masked-row-selective",
                json!({"mask":mask([50, 0, 40, 90], true, Value::Null, json!({"width":curve(40, 0)})),
                    "effects":[selective], "position":[25, -12], "opacity":200}),
            ),
            layer(
                "spatial-plain",
                json!({"spatial":spatial("bilinear", "transparent", 12000, json!({"rotation_mdeg":curve(-20000, 40000),"scale_x_milli":curve(800, 1500)}))}),
            ),
            layer(
                "spatial-masked",
                json!({"spatial":spatial("nearest", "clamp", 0, json!({"translate_x_milli":curve(-9000, 9000)})),
                    "mask":mask([30, 15, 90, 50], false, json!({"radius":9,"edge":"inner"}), json!({"y":curve(0, 30)}))}),
            ),
            layer(
                "spatial-keyed",
                json!({"spatial":spatial("bilinear", "clamp", 12000, json!({"scale_y_milli":curve(1000, 2500)})),
                    "effects":[key, grade], "mask":mask([0, 0, 160, 90], true, json!({"radius":30,"edge":"centered"}), Value::Null),
                    "animation":{"opacity":curve(90, 255)}}),
            ),
            // A constant grade, applied once to the source in a render.
            layer(
                "spatial-graded",
                json!({"spatial":spatial("bilinear", "transparent", 0, json!({"translate_y_milli":curve(-3000, 5000)})),
                    "effects":[{"kind":"grade","exposure_milli":-400,"contrast_milli":900,"white_balance_milli":[1000,1000,1200]}]}),
            ),
            layer(
                "masked-row-turned",
                json!({"mask":mask([10, 5, 110, 70], false, centered.clone(), json!({"width":curve(110, 60)})),
                    "effects":[turned.clone()], "position":[13, -9]}),
            ),
            layer(
                "spatial-turned",
                json!({"spatial":spatial("bilinear", "clamp", 12000, json!({"rotation_mdeg":curve(0, 30000)})),
                    "effects":[turned]}),
            ),
            // A constant selective turn, applied once to the source in a render.
            layer(
                "spatial-selective-turn",
                json!({"spatial":spatial("nearest", "transparent", 0, json!({"translate_x_milli":curve(4000, -6000)})),
                    "effects":[selective_turn]}),
            ),
        ];
        let root = std::env::temp_dir();
        // Noise with every alpha class, trimmed to a 150 x 80 image at (4, 6) on each canvas.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut noise = || {
            let rgba = (0..150 * 80)
                .flat_map(|_| {
                    let mut next = || {
                        state ^= state << 13;
                        state ^= state >> 7;
                        state ^= state << 17;
                        state
                    };
                    let alpha = match next() % 4 {
                        0 => 0,
                        1 => 255,
                        _ => next() as u8,
                    };
                    let (r, g, b) = (next() as u8, next() as u8, next() as u8);
                    [r, g, b, alpha]
                })
                .collect();
            Pixels {
                width: 150,
                height: 80,
                rgba,
            }
        };
        for transparent in [false, true] {
            scene.transparent = transparent;
            scene.layers = layers.clone();
            let frames = scene.validate().unwrap();
            let mut prepared = prepare(&scene, &root).unwrap();
            for layer in &scene.layers {
                prepared
                    .graphics
                    .insert(layer.id.clone(), (noise(), [4, 6]));
            }
            let reference = |prepared: &Prepared, n: u64| {
                REFERENCE_PATHS.with(|r| r.set(true));
                let frame = compose(&scene, prepared, n).unwrap();
                REFERENCE_PATHS.with(|r| r.set(false));
                frame
            };
            for n in 0..frames {
                let expected = reference(&prepared, n);
                for threads in [1, 3] {
                    prepared.threads = threads;
                    assert!(
                        compose(&scene, &prepared, n).unwrap() == expected,
                        "frame {n}, transparent {transparent}, threads {threads}"
                    );
                }
            }
            // With the static base and once-processed sources as a render prepares them.
            prepared.base = static_base(&scene, &prepared).unwrap();
            prepared.processed = processed_sources(&scene, &prepared).unwrap();
            assert_eq!(prepared.base.as_ref().unwrap().layers, 1);
            let mut processed = prepared.processed.keys().map(|k| k.0).collect::<Vec<_>>();
            processed.sort();
            assert_eq!(processed, [7, 10]);
            for n in 0..frames {
                let expected = reference(&prepared, n);
                assert!(
                    compose(&scene, &prepared, n).unwrap() == expected,
                    "frame {n}, transparent {transparent}, cached"
                );
            }
        }
    }

    #[test]
    fn ordered_frames_arrive_in_order_and_stop_on_failure() {
        let mut written = Vec::new();
        write_ordered(
            200,
            8,
            8,
            &|n| {
                // Uneven work makes later frames finish first.
                std::thread::sleep(std::time::Duration::from_micros((n % 7) * 300));
                Ok(n.to_le_bytes().to_vec())
            },
            &mut |frame| {
                written.push(u64::from_le_bytes(frame.try_into().unwrap()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written, (0..200).collect::<Vec<_>>());
        let failed = write_ordered(
            200,
            8,
            8,
            &|n| {
                if n == 50 {
                    Err(error("TEST", "frame 50"))
                } else {
                    Ok(vec![0])
                }
            },
            &mut |_| Ok(()),
        )
        .unwrap_err();
        assert_eq!(failed.code, "TEST");
        let mut count = 0;
        let stopped = write_ordered(200, 8, 8, &|_| Ok(vec![0]), &mut |_| {
            count += 1;
            if count == 20 {
                Err(error("WRITE", "pipe closed"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!((stopped.code, count), ("WRITE", 20));
    }

    #[test]
    fn scenes_last_two_minutes_at_their_rate() {
        for (rate, longest) in [(FPS, 3000), (Time { num: 60, den: 1 }, 7200)] {
            assert_eq!(max_frames(rate), longest);
            let mut scene = panel_scene();
            scene.frame_rate = Some(rate);
            scene.duration = frames(longest, rate);
            assert_eq!(scene.validate().unwrap(), longest);
            scene.duration = frames(longest + 1, rate);
            let error = scene.validate().unwrap_err();
            assert_eq!(error.code, "INVALID_SCENE");
            assert!(error.message.contains(&format!(
                "1 frame to 120 seconds of frames at its frame_rate ({longest} at {} fps)",
                rate.num
            )));
        }
    }

    #[test]
    fn scenes_hold_sixty_four_layers() {
        let mut scene = panel_scene();
        let panel = scene.layers[0].clone();
        scene.layers = (0..64)
            .map(|i| Layer {
                id: format!("panel{i}"),
                ..panel.clone()
            })
            .collect();
        scene.validate().unwrap();
        scene.layers.push(Layer {
            id: "one-too-many".into(),
            ..panel
        });
        let error = scene.validate().unwrap_err();
        assert_eq!(error.code, "INVALID_SCENE");
        assert!(error.message.contains("1-64 layers") && error.message.contains("65 layers"));
    }

    /// An opacity curve from `from` to 255 over the panel scene's three seconds.
    fn fade(from: i32) -> Option<Animation> {
        Some(serde_json::from_value(json!({"opacity":{"keys":[{"time":{"num":0,"den":1},"value":from,"interpolation":"linear"},{"time":{"num":3,"den":1},"value":255,"interpolation":"hold"}]}})).unwrap())
    }

    #[test]
    fn work_counts_clipped_destinations_assembly_and_passes() {
        let mut scene = panel_scene();
        // An animated layer composites in every frame.
        scene.layers[0].animation = fade(0);
        // 160 x 90 shown for 75 frames.
        assert_eq!(
            scene.work(FPS).unwrap(),
            (75 * 160 * 90, (0, 75 * 160 * 90))
        );
        // A 40 x 30 crop turned a quarter and enlarged 4x covers 120 x 160, clipped to 120 x 90.
        let t = &mut scene.layers[0].transform;
        (t.crop, t.scale, t.quarter_turns) = ([0, 0, 40, 30], 4, 1);
        assert_eq!(scene.work(FPS).unwrap().0, 75 * 120 * 90);
        // Transparent scenes compose a color and a matte pass.
        scene.transparent = true;
        assert_eq!(scene.work(FPS).unwrap().0, 2 * 75 * 120 * 90);
        // A tilemap also assembles its whole canvas for every active sample.
        scene.transparent = false;
        let mut tiled = scene.layers[0].clone();
        tiled.id = "tiles".into();
        tiled.duration = frames(10, FPS);
        tiled.tilemap = Some(serde_json::from_value(json!({"tile_size":[80,45],"tiles":[{"frames":[],"timing":"strict","end":"hold_last"}],"cells":[[0,0],[0,0]]})).unwrap());
        scene.layers.push(tiled);
        assert_eq!(
            scene.work(FPS).unwrap(),
            (
                75 * 120 * 90 + 10 * (120 * 90 + 160 * 90),
                (0, 75 * 120 * 90)
            )
        );
    }

    #[test]
    fn unchanging_bottom_layers_count_once_and_composite_once() {
        let mut scene = panel_scene();
        let panel = scene.layers[0].clone();
        let named = |id: &str| Layer {
            id: id.into(),
            ..panel.clone()
        };
        let moving = Layer {
            animation: fade(40),
            ..named("moving")
        };
        scene.layers = vec![named("a"), named("b"), moving, named("above")];
        let one = 160 * 90;
        assert_eq!(scene.static_layers().unwrap(), 2);
        assert_eq!(scene.work(FPS).unwrap().0, 2 * one + 2 * 75 * one);
        // Whatever can change keeps the bottom layer, and so every layer above it, per frame.
        let effect = |value: Value| -> Vec<crate::effects::Effect> {
            vec![serde_json::from_value(value).unwrap()]
        };
        let curve = json!({"keys":[{"time":{"num":0,"den":1},"value":0,"interpolation":"linear"},{"time":{"num":1,"den":1},"value":20,"interpolation":"hold"}]});
        type Change<'a> = Box<dyn Fn(&mut Layer) + 'a>;
        let changes: Vec<(&str, Change)> = vec![
            (
                "late start",
                Box::new(|l| (l.start, l.duration) = (frames(1, FPS), frames(74, FPS))),
            ),
            ("early end", Box::new(|l| l.duration = frames(74, FPS))),
            ("flat curve", Box::new(|l| l.animation = fade(255))),
            (
                "moving mask",
                Box::new(|l| {
                    l.mask = Some(
                        serde_json::from_value(json!({"rect":[0,0,9,9],"animation":{"x":curve}}))
                            .unwrap(),
                    )
                }),
            ),
            (
                "graded over time",
                Box::new(|l| {
                    l.effects = effect(
                        json!({"kind":"grade","exposure_milli":0,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000],"animation":{"exposure_milli":curve}}),
                    )
                }),
            ),
            (
                "fading key",
                Box::new(|l| {
                    l.effects = effect(
                        json!({"kind":"chroma_key","key_rgb":[0,255,0],"inner_milli":60,"outer_milli":300,"strength_milli":700,"strength_curve":curve}),
                    )
                }),
            ),
            (
                "spatial curve",
                Box::new(|l| {
                    l.transform.spatial = Some(serde_json::from_value(json!({"translate_milli":[0,0],"scale_milli":[1000,1000],"rotation_mdeg":0,"flip":[false,false],"pixel_aspect":{"num":1,"den":1},"sampling":"nearest","edge":"transparent","animation":{"rotation_mdeg":curve}})).unwrap())
                }),
            ),
        ];
        let root = std::env::temp_dir();
        let cached_layers = |scene: &Scene| {
            let prepared = prepare(scene, &root).unwrap();
            static_base(scene, &prepared)
                .unwrap()
                .map_or(0, |b| b.layers)
        };
        for (name, change) in &changes {
            let mut changed = scene.clone();
            change(&mut changed.layers[0]);
            assert_eq!(changed.static_layers().unwrap(), 0, "{name}");
            // The render may find more unchanging layers than the recipe proves, never fewer.
            assert!(
                cached_layers(&changed) >= changed.static_layers().unwrap(),
                "{name}"
            );
        }
        // Constant effects, masks and mappings stay unchanging.
        let mut constant = scene.clone();
        let l = &mut constant.layers[1];
        l.effects = effect(
            json!({"kind":"grade","exposure_milli":300,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000]}),
        );
        l.mask = Some(serde_json::from_value(json!({"rect":[2,2,90,40],"inverted":true})).unwrap());
        assert_eq!(constant.static_layers().unwrap(), 2);
        assert_eq!(cached_layers(&constant), 2);
        // A binding replaces the layer's values even when the graph is constant.
        let mut bound = scene.clone();
        bound.expressions = Some(serde_json::from_value(json!({"schema_version":1,"seed":0,
            "nodes":[{"id":"full","kind":"scalar","expression":{"op":"literal","value":{"type":"scalar","value":{"num":255,"den":1}}}}],
            "bindings":[{"layer":"b","property":"opacity","node":"full"}]})).unwrap());
        assert_eq!(bound.static_layers().unwrap(), 1);
        assert!(cached_layers(&bound) >= 1);
        // One held image that never ends is unchanging; a second image or an ending is not.
        let image = Frame {
            image: Identity {
                path: "a.png".into(),
                sha256: String::new(),
                bytes: 0,
            },
            matte: None,
            hold: frames(1, FPS),
            offset: [0, 0],
            anchor: [0, 0],
        };
        let mut held = scene.clone();
        held.layers[0].graphics = None;
        held.layers[0].frames = vec![image.clone()];
        assert_eq!(held.static_layers().unwrap(), 2);
        held.layers[0].end = End::Transparent;
        assert_eq!(held.static_layers().unwrap(), 0);
        held.layers[0].frames[0].hold = held.duration;
        assert_eq!(held.static_layers().unwrap(), 2);
        held.layers[0].frames.push(image);
        assert_eq!(held.static_layers().unwrap(), 0);
        // A centered shutter samples outside the scene at both ends; those samples show the
        // backdrop, so the base still holds and every frame is unchanged.
        scene.temporal = Some(serde_json::from_value(json!({"shutter_angle":{"num":360,"den":1},"phase":{"num":-180,"den":1},"samples":4,"integration":"encoded_rgb"})).unwrap());
        // Every sample of a changing layer counts, and averaging adds a whole-scene pass each.
        assert_eq!(
            scene.work(FPS).unwrap().0,
            2 * one + 2 * 75 * 4 * one + 75 * 4 * one
        );
        for transparent in [false, true] {
            scene.transparent = transparent;
            let plain = prepare(&scene, &root).unwrap();
            let mut cached = prepare(&scene, &root).unwrap();
            cached.base = static_base(&scene, &cached).unwrap();
            approved_base(&scene, &cached).unwrap();
            assert_eq!(cached.base.as_ref().unwrap().layers, 2);
            for n in 0..75 {
                assert_eq!(
                    compose(&scene, &cached, n).unwrap(),
                    compose(&scene, &plain, n).unwrap(),
                    "frame {n}, transparent {transparent}"
                );
            }
        }
    }

    #[test]
    fn work_over_the_budget_is_rejected_before_media_is_read() {
        // Eight 8-megapixel layers for 1,000 frames is exactly the budget.
        let mut scene = panel_scene();
        (scene.width, scene.height, scene.output_scale) = (4000, 2000, 1);
        scene.duration = frames(1001, FPS);
        let panel = &mut scene.layers[0];
        (panel.canvas, panel.duration) = ([4000, 2000], frames(1000, FPS));
        panel.transform.crop = [0, 0, 4000, 2000];
        let panel = panel.clone();
        scene.layers = (0..8)
            .map(|i| Layer {
                id: format!("wash{i}"),
                ..panel.clone()
            })
            .collect();
        assert_eq!(scene.work(FPS).unwrap().0, MAX_COMPOSITED_PIXELS);
        scene.validate().unwrap();
        scene.layers[5].duration = frames(1001, FPS);
        let error = scene.validate().unwrap_err();
        assert_eq!(error.code, "LIMIT_EXCEEDED");
        assert!(
            error.message.contains("64008000000 layer pixels")
                && error.message.contains("layers[5] (wash5) with 8008000000"),
            "{}",
            error.message
        );
        // Inspection reports the same estimate; nothing is read for a rejected scene.
        let missing = std::env::temp_dir().join("cutbolt-no-such-root");
        assert_eq!(
            inspect(&scene, &missing).unwrap_err().code,
            "LIMIT_EXCEEDED"
        );
    }

    #[test]
    fn trimmed_graphics_composite_exactly_like_whole_canvases() {
        let root = std::env::temp_dir();
        let transform = |position: [i32; 2], crop, scale, quarter_turns, opacity| Transform {
            position,
            crop,
            scale,
            quarter_turns,
            opacity,
            spatial: None,
        };
        let base = panel_scene();
        let panel = base.layers[0].clone();
        let mut empty = panel.clone();
        empty.id = "empty".into();
        empty.graphics = Some(serde_json::from_value(json!({"kind":"shape","shape":"rectangle","rect":[0,0,160,90],"fill":[9,9,9,0],"stroke":null})).unwrap());
        let mut turned = panel.clone();
        turned.id = "turned".into();
        turned.transform = transform([150, -20], [4, 10, 150, 70], 2, 1, 200);
        turned.blend_mode = composite::BlendMode::Multiply;
        turned.mask =
            Some(serde_json::from_value(json!({"rect":[30,20,90,40],"inverted":true})).unwrap());
        let mut moved = panel.clone();
        moved.id = "moved".into();
        moved.transform = transform([-30, 25], [0, 0, 160, 90], 1, 2, 255);
        moved.blend_mode = composite::BlendMode::Screen;
        let mut opaque = base.clone();
        opaque.layers = vec![panel.clone(), empty, turned, moved];
        let mut transparent = base.clone();
        transparent.transparent = true;
        transparent.layers = vec![panel.clone()];
        transparent.layers[0].transform = transform([20, 7], [2, 3, 150, 80], 1, 3, 180);
        for scene in [opaque, transparent] {
            let mut prepared = prepare(&scene, &root).unwrap();
            assert_ne!(prepared.graphics["panel"].1, [0, 0]);
            let trimmed = compose(&scene, &prepared, 0).unwrap();
            let mut retained = 0;
            for layer in &scene.layers {
                let (image, offset) = &prepared.graphics[&layer.id];
                retained += image.width * image.height;
                assert!(image.width * image.height < 160 * 90 && offset[0] + image.width <= 160);
                let (rgba, _) = crate::graphics::rasterize(
                    layer.graphics.as_ref().unwrap(),
                    layer.canvas,
                    &root,
                    &mut Default::default(),
                )
                .unwrap();
                let whole = Pixels {
                    width: 160,
                    height: 90,
                    rgba,
                };
                prepared.graphics.insert(layer.id.clone(), (whole, [0, 0]));
            }
            assert_eq!(prepared.report["work"]["decoded_pixels"], retained);
            assert_eq!(trimmed, compose(&scene, &prepared, 0).unwrap());
        }
    }

    #[test]
    fn per_frame_records_stay_small() {
        // 64 layers x 7,200 frames hold one record each.
        assert!(std::mem::size_of::<Option<Parameters>>() <= 64);
    }

    #[test]
    fn tile_selections_are_run_length_encoded() {
        assert_eq!(
            runs(&[Some(0), Some(0), Some(1), None, None]),
            json!([{"count":2,"value":0},{"count":1,"value":1},{"count":2,"value":null}])
        );
    }

    #[test]
    fn curve_rejections_name_the_layer_curve_key_value_and_bound() {
        // A 6.72 s title, as in the demos that met these rejections.
        let mut scene = panel_scene();
        let length = Time::new(168, 25).unwrap();
        scene.duration = length;
        scene.layers[0].duration = length;
        scene.layers[0].id = "title".into();
        let key = |num: u64, den: u64, value: i32| json!({"time":{"num":num,"den":den},"value":value,"interpolation":"linear"});
        let rejected = |scene: &Scene| {
            let error = prepare(scene, &std::env::temp_dir()).err().unwrap();
            assert_eq!(error.code, "INVALID_ANIMATION", "{}", error.message);
            error.message
        };
        let animated = |animation: Value| -> Option<Animation> {
            Some(serde_json::from_value(animation).unwrap())
        };
        // A key past the layer's end.
        scene.layers[0].animation =
            animated(json!({"opacity":{"keys":[key(0, 1, 0), key(34, 5, 255)]}}));
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).animation.opacity.keys[1].time: 34/5 s (6.8 s) is past the end of the 168/25 s (6.72 s) duration; key times must lie within 0..duration"
        );
        // Keys stretched past the end name the first one beyond it.
        scene.layers[0].animation =
            animated(json!({"opacity":{"keys":[key(0, 1, 0), key(4, 1, 255)]},
            "position_y":{"keys":[key(0, 1, 0), key(17, 5, 40), key(42, 5, 80), key(9, 1, 0)]}}));
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).animation.position_y.keys[2].time: 42/5 s (8.4 s) is past the end of the 168/25 s (6.72 s) duration; key times must lie within 0..duration"
        );
        // Values, denominators and equal times name their key too.
        scene.layers[0].animation =
            animated(json!({"opacity":{"keys":[key(0, 1, 0), key(1, 1, 256)]}}));
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).animation.opacity.keys[1].value: 256 is outside the property's range 0..255"
        );
        scene.layers[0].animation =
            animated(json!({"opacity":{"keys":[key(1, 2, 0), key(0, 1, 9), key(2, 4, 255)]}}));
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).animation.opacity.keys[2].time: 1/2 s (0.5 s) equals keys[0].time; equal-time keys are ambiguous, including equivalent rational times"
        );
        scene.layers[0].animation =
            animated(json!({"opacity":{"keys":[key(0, 1, 0), key(1, 1_000_001, 255)]}}));
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).animation.opacity.keys[1].time: 1/1000001 s (0.000001 s) reduces to denominator 1000001, above the 1000000 limit; use a coarser fraction"
        );
        // A hue shift beyond one full turn, inside an effect.
        scene.layers[0].animation = None;
        scene.layers[0].effects = vec![
            serde_json::from_value(json!({"kind":"grade","exposure_milli":0,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000]})).unwrap(),
            serde_json::from_value(json!({"kind":"grade","exposure_milli":0,"contrast_milli":1000,"white_balance_milli":[1000,1000,1000],
                "animation":{"hue_shift_mdeg":{"keys":[key(0, 1, 0), key(6, 1, 400_000)]}}})).unwrap(),
        ];
        assert_eq!(
            rejected(&scene),
            "layers[0] (title).effects[1].animation.hue_shift_mdeg.keys[1].value: 400000 is outside the property's range -360000..360000"
        );
        // A geometry rotation beyond one full turn.
        scene.layers[0].effects.clear();
        scene.geometry = Some(serde_json::from_value(json!({
            "nodes":[{"id":"card","transform":{"position_milli":{"value":[0,0,0]},
                "rotation_mdeg":{"value":[0,0,0],"animation":{"z":{"keys":[key(0, 1, 0), key(6, 1, 720_000)]}}},
                "scale_milli":{"value":[1000,1000,1000]}},
                "plane":{"layer":"title","size_milli":[1600,900],"material":"unlit","double_sided":false}}],
            "camera":{"position_milli":{"value":[0,0,4000]},"target_milli":{"value":[0,0,0]},"up_milli":{"value":[0,1000,0]},
                "projection":{"kind":"perspective","vertical_fov_mdeg":{"value":40000}},"near_milli":100,"far_milli":100000},
            "lights":[],"shadows":"none"})).unwrap());
        assert_eq!(
            rejected(&scene),
            "geometry.nodes[0] (card).transform.rotation_mdeg.animation.z.keys[1].value: 720000 is outside the property's range -360000..360000"
        );
    }
}
