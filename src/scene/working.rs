//! Precise scene frame source. Uses the normal scene contract and dependency checks,
//! but retains straight-linear UNORM16 through layers, 3D planes and shutter samples.
use super::{Frame, Layer, Prepared, Scene};
use crate::{
    Result,
    color::{
        Transfer,
        working::{Context, Pixel, layer::Image},
    },
    composite::AlphaMode,
    effects, error,
    time::Time,
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

const LIMIT: u64 = 1 << 30;
type Key = (PathBuf, Option<PathBuf>, bool);
struct Picture {
    size: [u32; 2],
    pixels: Vec<Pixel>,
}
enum Plane {
    Solid(Pixel),
    Pixels(Vec<Pixel>),
}
impl Plane {
    fn new(pixels: Vec<Pixel>) -> Self {
        if let Some(first) = pixels.first().copied()
            && pixels.iter().all(|p| *p == first)
        {
            Self::Solid(first)
        } else {
            Self::Pixels(pixels)
        }
    }
    fn bytes(&self) -> u64 {
        match self {
            Self::Solid(_) => 8,
            Self::Pixels(p) => p.len() as u64 * 8,
        }
    }
    fn pixels(&self, count: usize) -> Vec<Pixel> {
        match self {
            Self::Solid(p) => vec![*p; count],
            Self::Pixels(p) => p.clone(),
        }
    }
}

/// Prepared original scene sources, ready for generated frames without a movie or byte compositor.
/// Frame values are linear SDR. Encoding/dither and publication belong to the caller.
pub struct Source {
    context: Context,
    scene: Scene,
    prepared: Prepared,
    images: HashMap<Key, Picture>,
    graphics: HashMap<String, (Picture, [u32; 2])>,
    effects: Vec<Vec<effects::Sampler>>,
    base: Option<(usize, Plane)>,
    backdrop: Pixel,
    retained: u64,
    audio_samples: u64,
    silent: bool,
}

fn key(frame: &Frame, alpha: AlphaMode) -> Key {
    (
        frame.image.path.clone(),
        frame.matte.as_ref().map(|m| m.path.clone()),
        alpha.is_straight(),
    )
}
fn decode(p: &super::Pixels, alpha: AlphaMode, context: Context) -> Result<Picture> {
    Ok(Picture {
        size: [p.width, p.height],
        pixels: p
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| context.decode8(*p, Transfer::Srgb, alpha))
            .collect::<Result<_>>()?,
    })
}
fn bounded(bytes: u64, message: &str) -> Result<()> {
    if bytes > LIMIT {
        Err(error("LIMIT_EXCEEDED", message))
    } else {
        Ok(())
    }
}

pub(super) fn sample_opacity(
    layer: &Layer,
    selected: &[Option<usize>],
    times: &[Option<Time>],
    expressions: Option<&[Vec<crate::expressions::Bound>]>,
) -> Result<Vec<u16>> {
    let curve = layer
        .animation
        .as_ref()
        .and_then(|a| a.opacity.as_ref())
        .map(|c| c.prepare(layer.duration, 0, 255))
        .transpose()?;
    selected
        .iter()
        .enumerate()
        .map(|(i, selected)| {
            if selected.is_none() {
                return Ok(0);
            }
            let time = times[i]
                .expect("selected scene sample")
                .minus(layer.start)?;
            if let Some(binding) = expressions.and_then(|all| {
                all[i].iter().find(|b| {
                    b.layer == layer.id && b.property == crate::expressions::Property::Opacity
                })
            }) {
                let value = binding.value.scalar()?;
                // The graph validated 0..255 before rounding. Retain its exact rational value
                // without multiplying through Number's separate expression-result magnitude cap.
                return Ok(((value.num as u128 * 257 + u128::from(value.den) / 2)
                    / u128::from(value.den)) as u16);
            }
            curve
                .as_ref()
                .map_or(Ok(u16::from(layer.transform.opacity) * 257), |c| {
                    c.sample_scaled(time, 257).map(|v| v as u16)
                })
        })
        .collect()
}

impl Source {
    pub fn new(scene: &Scene, root: &Path) -> Result<Self> {
        Self::new_in(scene, root, Context::LINEAR16)
    }
    /// Prepare a non-default working representation. Default8/encoded keeps the established
    /// FrameSource; this source never silently substitutes its equations for that default.
    pub fn new_in(scene: &Scene, root: &Path, context: Context) -> Result<Self> {
        if context.space.is_legacy() {
            return Err(error(
                "INVALID_COLOR",
                "Use the existing scene source for default 8-bit encoded rendering",
            ));
        }
        let mut prepared = super::prepare_with_precision(scene, root, true)?;
        for layer in prepared
            .working_opacity
            .as_mut()
            .expect("precise preparation")
        {
            for opacity in layer {
                *opacity = context.quantum(*opacity);
            }
        }
        let audio_samples = prepared.pcm.len() as u64 / 2;
        let silent = prepared.pcm.iter().all(|v| *v == 0);
        if silent {
            prepared.pcm = Vec::new();
        }
        let raw_bytes = prepared
            .images
            .values()
            .chain(prepared.matted.values())
            .map(|p| p.rgba.len() as u64)
            .sum::<u64>()
            + prepared
                .graphics
                .values()
                .map(|(p, _)| p.rgba.len() as u64)
                .sum::<u64>();
        let original_report_bytes = serde_json::to_vec(&prepared.report)?.len() as u64;
        let metadata = (serde_json::to_vec(&prepared.parameters)?.len()
            + serde_json::to_vec(&prepared.selected)?.len()
            + serde_json::to_vec(&prepared.tiles)?.len()
            + serde_json::to_vec(scene)?.len()) as u64
            * 8
            + original_report_bytes * 8;
        let table_bytes = prepared
            .effect_resources
            .report()
            .and_then(|v| v["allocated_table_bytes"].as_u64())
            .unwrap_or(0);
        let opacity_bytes = prepared
            .working_opacity
            .as_ref()
            .expect("precise preparation")
            .iter()
            .map(|v| v.len() as u64 * 2)
            .sum::<u64>();
        let mut retained = metadata + table_bytes + opacity_bytes + prepared.pcm.len() as u64 * 2;
        bounded(
            retained + raw_bytes,
            "Precise scene preparation exceeds the 1 GiB retained-payload bound",
        )?;
        let mut images = HashMap::new();
        for layer in &scene.layers {
            for frame in layer.frames.iter().chain(
                layer
                    .tilemap
                    .iter()
                    .flat_map(|m| &m.tiles)
                    .flat_map(|t| &t.frames),
            ) {
                let key = key(frame, layer.alpha_mode);
                if let std::collections::hash_map::Entry::Vacant(entry) = images.entry(key) {
                    let picture = prepared.frame_pixels(frame);
                    retained += u64::from(picture.width) * u64::from(picture.height) * 8;
                    bounded(
                        retained + raw_bytes,
                        "Precise scene source conversion exceeds the 1 GiB preparation-payload bound",
                    )?;
                    entry.insert(decode(picture, layer.alpha_mode, context)?);
                }
            }
        }
        let mut graphics = HashMap::new();
        for (id, (picture, offset)) in &prepared.graphics {
            retained += u64::from(picture.width) * u64::from(picture.height) * 8;
            bounded(
                retained + raw_bytes,
                "Precise scene graphics exceed the 1 GiB preparation-payload bound",
            )?;
            graphics.insert(
                id.clone(),
                (decode(picture, AlphaMode::Straight, context)?, *offset),
            );
        }
        // These byte buffers held original inputs, not intermediate effects. Every interpretation
        // is now retained in the wide caches; binary mattes were exact zero/keep operations.
        prepared.images.clear();
        prepared.matted.clear();
        prepared.graphics.clear();
        let effects = scene
            .layers
            .iter()
            .map(|l| effects::prepare(&l.effects, l.duration))
            .collect::<Result<_>>()?;
        let backdrop = if scene.transparent {
            Pixel::TRANSPARENT
        } else {
            context.decode8(
                [
                    scene.background[0],
                    scene.background[1],
                    scene.background[2],
                    255,
                ],
                Transfer::Srgb,
                AlphaMode::Straight,
            )?
        };
        let mut source = Self {
            context,
            scene: scene.clone(),
            prepared,
            images,
            graphics,
            effects,
            base: None,
            backdrop,
            retained,
            audio_samples,
            silent,
        };
        bounded(
            source.frame_buffer_bytes(),
            "Precise scene frame buffers exceed 1 GiB",
        )?;
        let count = scene.static_layers()?;
        if count > 0
            && let Some(first) = source.prepared.times.iter().position(Option::is_some)
        {
            let pixels = vec![backdrop; (scene.width * scene.height) as usize];
            let base = Plane::new(source.layers(first, 0..count, pixels)?);
            source.retained += base.bytes();
            bounded(
                source.retained,
                "Precise scene base exceeds the 1 GiB retained-payload bound",
            )?;
            source.base = Some((count, base));
        }
        if source.base.as_ref().map_or(0, |b| b.0) < count {
            return Err(error(
                "LIMIT_EXCEEDED",
                "Precise scene must cache its approved static layers",
            ));
        }
        source.prepared.report["profile"] = serde_json::json!("linear-unorm16-scene-source");
        source.prepared.report["color"] = serde_json::json!("straight-linear-srgb-unorm16");
        source.prepared.report["working_context"] = serde_json::json!(context);
        if context.space != Context::LINEAR16.space {
            source.prepared.report["profile"] = serde_json::json!("working-scene-source");
            source.prepared.report["color"] = serde_json::json!({"depth":context.space.bits,"light":context.space.light,"transfer":context.transfer,"alpha":"linear_coverage"});
        }
        source.prepared.report["working_processing"] = serde_json::json!({"opacity":"exact source curves and expression bindings to declared working depth","temporal":"associated mean in declared working light","delivery":"caller encodes and dithers once","retained_payload_bytes":source.retained,"frame_buffer_bytes":source.frame_buffer_bytes(),"cached_layers":count});
        if let Some(temporal) = source.prepared.report.get_mut("temporal") {
            temporal["integration"] = serde_json::json!(
                "associated_mean_in_declared_working_light_and_depth_after_complete_compositing"
            );
        }
        for (i, layer) in source.prepared.report["timing"]
            .as_array_mut()
            .expect("scene timing")
            .iter_mut()
            .enumerate()
        {
            layer["working_opacity"] = super::runs(
                &source
                    .prepared
                    .working_opacity
                    .as_ref()
                    .expect("precise preparation")[i],
            );
            layer["sampled_parameters_note"] = serde_json::json!(
                "geometry and legacy byte controls; working_opacity supplies actual coverage in a 16-bit carrier"
            );
            if layer.get("effect_processing").is_some() {
                layer["effect_processing"] = serde_json::json!(
                    "linear_srgb_f64_to_declared_working_light_and_depth_once_per_effect_stack"
                );
            }
        }
        let report_bytes = serde_json::to_vec(&source.prepared.report)?.len() as u64;
        // Include the added wide-opacity runs and profile notes, with room for this count's digits.
        source.retained += report_bytes.saturating_sub(original_report_bytes) * 8 + 128;
        bounded(
            source.retained,
            "Precise scene report exceeds the 1 GiB retained-payload bound",
        )?;
        source.prepared.report["working_processing"]["retained_payload_bytes"] =
            serde_json::json!(source.retained);
        Ok(source)
    }

    pub fn report(&self) -> &serde_json::Value {
        &self.prepared.report
    }
    pub fn context(&self) -> Context {
        self.context
    }
    pub fn retained_bytes(&self) -> u64 {
        self.retained
    }
    /// Includes the exposure accumulator, overlapping sample canvases, largest tile canvas
    /// and enlarged/proxy output. Prepared source payload has a separate reported charge.
    pub fn frame_buffer_bytes(&self) -> u64 {
        let s = &self.scene;
        let tile = s
            .layers
            .iter()
            .filter(|l| l.tilemap.is_some())
            .map(|l| u64::from(l.canvas[0]) * u64::from(l.canvas[1]) * 8)
            .max()
            .unwrap_or(0);
        u64::from(s.width) * u64::from(s.height) * (48 + 8 * u64::from(s.output_scale).pow(2))
            + tile
    }
    pub fn sources(&self) -> Vec<crate::render::Source> {
        self.prepared
            .sources
            .iter()
            .map(|(path, i)| crate::render::Source {
                path: path.clone(),
                sha256: i.sha256.clone(),
                frames: 0,
                samples: 0,
                conversion: None,
            })
            .collect()
    }
    /// Call before publishing output; frame sampling itself reads only the pinned prepared data.
    pub fn verify_sources(&self, root: &Path) -> Result<()> {
        self.prepared.verify_sources(root)
    }
    pub fn audio_samples(&self) -> u64 {
        self.audio_samples
    }
    pub fn pcm(&self, first: u64, samples: u64) -> Result<Vec<i16>> {
        let end = first
            .checked_add(samples)
            .filter(|n| *n <= self.audio_samples)
            .ok_or_else(|| error("INVALID_RANGE", "Precise scene audio exceeds its source"))?;
        Ok(if self.silent {
            vec![0; samples as usize * 2]
        } else {
            self.prepared.pcm[first as usize * 2..end as usize * 2].to_vec()
        })
    }

    fn picture(&self, frame: &Frame, layer: &Layer) -> &Picture {
        &self.images[&key(frame, layer.alpha_mode)]
    }
    fn assemble(&self, layer_index: usize, sample: usize) -> Picture {
        let layer = &self.scene.layers[layer_index];
        let map = layer.tilemap.as_ref().expect("tilemap");
        let [width, height] = layer.canvas;
        let [tw, th] = map.tile_size;
        let mut pixels = vec![Pixel::TRANSPARENT; (width * height) as usize];
        let chosen = self.prepared.tiles[layer_index]
            .as_ref()
            .expect("tile selections");
        for (r, row) in map.cells.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                let Some(tile) = cell.map(usize::from) else {
                    continue;
                };
                let Some(index) = chosen[tile][sample] else {
                    continue;
                };
                let frame = &map.tiles[tile].frames[index];
                let image = self.picture(frame, layer);
                let (x0, y0) = (
                    c * tw as usize + frame.offset[0] as usize,
                    r * th as usize + frame.offset[1] as usize,
                );
                for y in 0..image.size[1] as usize {
                    let src = y * image.size[0] as usize;
                    let dst = (y0 + y) * width as usize + x0;
                    pixels[dst..dst + image.size[0] as usize]
                        .copy_from_slice(&image.pixels[src..src + image.size[0] as usize]);
                }
            }
        }
        Picture {
            size: [width, height],
            pixels,
        }
    }
    fn effects(&self, index: usize, sample: usize) -> Result<effects::working::Frame<'_>> {
        let layer = &self.scene.layers[index];
        let time = self.prepared.times[sample]
            .expect("selected sample")
            .minus(layer.start)?;
        effects::working::frame(
            &layer.effects,
            &self.effects[index],
            &self.prepared.effect_resources,
            time,
        )
    }
    fn layers(
        &self,
        sample: usize,
        layers: std::ops::Range<usize>,
        mut pixels: Vec<Pixel>,
    ) -> Result<Vec<Pixel>> {
        for i in layers {
            let layer = &self.scene.layers[i];
            let Some(index) = self.prepared.selected[i][sample] else {
                continue;
            };
            let assembled;
            let (image, offset, anchor) = if layer.graphics.is_some() {
                let (image, offset) = &self.graphics[&layer.id];
                (image, *offset, [0, 0])
            } else if layer.tilemap.is_some() {
                assembled = self.assemble(i, sample);
                (&assembled, [0, 0], [0, 0])
            } else {
                let frame = &layer.frames[index];
                (self.picture(frame, layer), frame.offset, frame.anchor)
            };
            let parameters = self.prepared.parameters[i][sample]
                .as_ref()
                .expect("selected parameters");
            let opacity = self
                .prepared
                .working_opacity
                .as_ref()
                .expect("wide opacity")[i][sample];
            let processor = self.effects(i, sample)?;
            crate::color::working::layer::draw_in(
                self.context,
                layer.recipe(),
                parameters,
                opacity,
                &Image {
                    size: image.size,
                    offset,
                    pixels: &image.pixels,
                },
                anchor,
                [self.scene.width, self.scene.height],
                &mut pixels,
                Some(&processor),
            )?;
        }
        Ok(pixels)
    }
    fn geometry(&self, state: &crate::geometry::State, sample: usize) -> Result<Vec<Pixel>> {
        let processors = self
            .scene
            .layers
            .iter()
            .enumerate()
            .map(|(i, _)| {
                self.prepared.selected[i][sample]
                    .map(|_| self.effects(i, sample))
                    .transpose()
            })
            .collect::<Result<Vec<_>>>()?;
        crate::geometry::compose_working(
            &self.scene,
            state,
            self.backdrop,
            self.context,
            |i, sx, sy| {
                let layer = &self.scene.layers[i];
                let Some(index) = self.prepared.selected[i][sample] else {
                    return Ok(None);
                };
                let p = self.prepared.parameters[i][sample]
                    .as_ref()
                    .expect("selected parameters");
                let opacity = self
                    .prepared
                    .working_opacity
                    .as_ref()
                    .expect("wide opacity")[i][sample];
                let [x, y, w, h] = layer.transform.crop;
                if sx < x || sy < y || sx >= x + w || sy >= y + h || opacity == 0 {
                    return Ok(None);
                }
                let coverage = layer
                    .mask
                    .as_ref()
                    .zip(p.mask_rect)
                    .map_or(crate::composite::MASK_WEIGHT, |(m, r)| {
                        m.coverage(r, i64::from(sx), i64::from(sy))
                    });
                if coverage == 0 {
                    return Ok(None);
                }
                let (image, offset) = if layer.graphics.is_some() {
                    let (image, offset) = &self.graphics[&layer.id];
                    (image, *offset)
                } else {
                    let frame = &layer.frames[index];
                    (self.picture(frame, layer), frame.offset)
                };
                let (ix, iy) = (
                    i64::from(sx) - i64::from(offset[0]),
                    i64::from(sy) - i64::from(offset[1]),
                );
                if ix < 0
                    || iy < 0
                    || ix >= i64::from(image.size[0])
                    || iy >= i64::from(image.size[1])
                {
                    return Ok(None);
                }
                let pixel = processors[i]
                    .as_ref()
                    .expect("selected processor")
                    .pixel_in(
                        self.context,
                        image.pixels[(iy * i64::from(image.size[0]) + ix) as usize],
                        [i64::from(sx), i64::from(sy)],
                    )?;
                Ok((pixel.0[3] > 0).then_some(crate::geometry::WorkingTexel {
                    pixel,
                    opacity,
                    coverage,
                    blend: layer.blend_mode,
                }))
            },
        )
    }
    fn sample(&self, sample: usize) -> Result<Vec<Pixel>> {
        let count = (self.scene.width * self.scene.height) as usize;
        if self.prepared.times[sample].is_none() {
            return Ok(vec![self.backdrop; count]);
        }
        if let Some(geometry) = &self.prepared.geometry {
            return match &geometry.states[sample] {
                Some(state) => self.geometry(state, sample),
                None => Ok(vec![self.backdrop; count]),
            };
        }
        let (first, pixels) = self.base.as_ref().map_or_else(
            || (0, vec![self.backdrop; count]),
            |(first, base)| (*first, base.pixels(count)),
        );
        self.layers(sample, first..self.scene.layers.len(), pixels)
    }
    /// Logical scene frame, followed by exact enlargement or associated box reduction for proxies.
    pub fn frame(&self, n: u64, divisor: u32) -> Result<Vec<Pixel>> {
        if n >= self.scene.duration.units(self.scene.clock()?)? || !matches!(divisor, 1 | 2 | 4 | 8)
        {
            return Err(error(
                "INVALID_RANGE",
                "Precise scene frame or proxy divisor is outside its bounds",
            ));
        }
        let s = &self.scene;
        let count = self.prepared.samples_per_frame;
        let first = n as usize * count;
        let mut pixels = self.sample(first)?;
        if count > 1 {
            let mut totals = vec![[0u64; 4]; pixels.len()];
            for sample in first..first + count {
                if sample != first {
                    pixels = self.sample(sample)?;
                }
                for (sum, p) in totals.iter_mut().zip(&pixels) {
                    accumulate(sum, *p);
                }
            }
            for (p, sum) in pixels.iter_mut().zip(totals) {
                *p = self.context.quantize(mean(sum, count as u64));
            }
        }
        let (width, height) = (
            s.width * s.output_scale / divisor,
            s.height * s.output_scale / divisor,
        );
        if width == 0 || height == 0 {
            return Err(error("INVALID_RANGE", "Precise proxy has no output pixels"));
        }
        if s.output_scale == 1 && divisor == 1 {
            return Ok(pixels);
        }
        let mut output = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                if divisor == 1 {
                    output.push(
                        pixels[((y / s.output_scale) * s.width + x / s.output_scale) as usize],
                    );
                    continue;
                }
                let mut sum = [0u64; 4];
                for dy in 0..divisor {
                    for dx in 0..divisor {
                        let (sx, sy) = (
                            (x * divisor + dx) / s.output_scale,
                            (y * divisor + dy) / s.output_scale,
                        );
                        accumulate(&mut sum, pixels[(sy * s.width + sx) as usize]);
                    }
                }
                output.push(self.context.quantize(mean(sum, u64::from(divisor).pow(2))));
            }
        }
        Ok(output)
    }
}
fn accumulate(sum: &mut [u64; 4], pixel: Pixel) {
    let a = u64::from(pixel.0[3]);
    for (sum, c) in sum[..3].iter_mut().zip(pixel.0) {
        *sum += u64::from(c) * a;
    }
    sum[3] += a;
}
fn mean(sum: [u64; 4], count: u64) -> Pixel {
    if sum[3] == 0 {
        return Pixel::TRANSPARENT;
    }
    let mut result = [0; 4];
    for c in 0..3 {
        result[c] = ((sum[c] + sum[3] / 2) / sum[3]) as u16;
    }
    result[3] = ((sum[3] + count / 2) / count) as u16;
    Pixel(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn prepared_frames_pin_images_tables_and_audio_but_publication_rehashes_all() {
        let root = std::env::temp_dir().join(format!(
            "cutbolt-working-scene-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let mut encoder =
            png::Encoder::new(fs::File::create(root.join("image.png")).unwrap(), 2, 2);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&[101, 213, 57, 171].repeat(4))
            .unwrap();
        fs::write(
            root.join("look.cube"),
            "LUT_1D_SIZE 2\n0.2 0.9 0.1\n0.8 0.3 0.7\n",
        )
        .unwrap();
        let mut audio = hound::WavWriter::create(
            root.join("sound.wav"),
            hound::WavSpec {
                channels: 2,
                sample_rate: 48000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        audio.write_sample(73i16).unwrap();
        audio.write_sample(-37i16).unwrap();
        audio.finalize().unwrap();
        let identity = |name: &str| json!({"path":name,"bytes":fs::metadata(root.join(name)).unwrap().len(),"sha256":crate::media::file_hash(&root.join(name)).unwrap()});
        let scene:Scene=serde_json::from_value(json!({"schema_version":1,"id":"pinned-working-scene","width":4,"height":4,"output_scale":1,"duration":{"num":1,"den":25},"background":[7,11,19],"color":"srgb_straight_encoded",
            "layers":[{"id":"picture","canvas":[2,2],"start":{"num":0,"den":1},"duration":{"num":1,"den":25},"frames":[{"image":identity("image.png"),"hold":{"num":1,"den":25},"offset":[0,0],"anchor":[0,0]}],"timing":"strict","end":"hold_last","transform":{"position":[1,1],"crop":[0,0,2,2],"scale":1,"quarter_turns":0,"opacity":171},"effects":[{"kind":"lut","file":identity("look.cube"),"interpolation":"linear"}]}],
            "audio":{"file":identity("sound.wav"),"start":{"num":0,"den":1},"channels":"preserve_stereo","resampling":"linear","padding":"silence"}})).unwrap();
        let source = Source::new(&scene, &root).unwrap();
        assert_eq!(source.sources().len(), 3);
        assert_eq!(source.pcm(0, 2).unwrap(), [73, -37, 0, 0]);
        let pixels = source.frame(0, 1).unwrap();
        assert!(pixels.iter().any(|p| *p != source.backdrop));
        for name in ["image.png", "look.cube", "sound.wav"] {
            let path = root.join(name);
            let saved = fs::read(&path).unwrap();
            let mut changed = saved.clone();
            let n = changed.len() - 1;
            changed[n] ^= 1;
            fs::write(&path, changed).unwrap();
            assert_eq!(source.frame(0, 1).unwrap(), pixels);
            assert_eq!(source.pcm(0, 2).unwrap(), [73, -37, 0, 0]);
            assert_eq!(
                source.verify_sources(&root).unwrap_err().code,
                "MEDIA_CHANGED"
            );
            fs::write(&path, saved).unwrap();
            source.verify_sources(&root).unwrap();
        }
        assert_eq!(source.frame(1, 1).unwrap_err().code, "INVALID_RANGE");
        assert_eq!(source.frame(0, 8).unwrap_err().code, "INVALID_RANGE");
        for name in ["image.png", "look.cube", "sound.wav"] {
            fs::remove_file(root.join(name)).unwrap();
        }
        fs::remove_dir(root).unwrap();
    }
}
