//! Bounded, editable pixel scenes compiled to the existing lossless timeline profile.
//! Exact time sampling; original integer pixel transforms and optional interpolated 2D mapping.
use crate::{
    Result,
    animation::Curve,
    composite::{self, AlphaMode, BlendMode, RectMask},
    error, media, render,
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

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub image: Identity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matte: Option<Identity>,
    pub hold: Time,
    pub offset: [u32; 2],
    pub anchor: [i32; 2],
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Timing {
    Strict,
    SampleStart,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum End {
    Loop,
    HoldLast,
    Transparent,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Transform {
    pub position: [i32; 2],
    pub crop: [u32; 4],
    pub scale: u32,
    pub quarter_turns: u8,
    pub opacity: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spatial: Option<crate::spatial::Transform>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Animation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_x: Option<Curve>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_y: Option<Curve>,
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
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub id: String,
    pub canvas: [u32; 2],
    pub start: Time,
    pub duration: Time,
    pub frames: Vec<Frame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graphics: Option<crate::graphics::Graphic>,
    pub timing: Timing,
    pub end: End,
    pub transform: Transform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<Animation>,
    #[serde(default, skip_serializing_if = "AlphaMode::is_straight")]
    pub alpha_mode: AlphaMode,
    #[serde(default, skip_serializing_if = "BlendMode::is_normal")]
    pub blend_mode: BlendMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<RectMask>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<crate::effects::Effect>,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Channels {
    DuplicateMono,
    PreserveStereo,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Resampling {
    Linear,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Padding {
    Silence,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Audio {
    pub file: Identity,
    pub start: Time,
    pub channels: Channels,
    pub resampling: Resampling,
    pub padding: Padding,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Color {
    SrgbStraightEncoded,
}
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub schema_version: u32,
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub output_scale: u32,
    pub duration: Time,
    pub background: [u8; 3],
    pub color: Color,
    pub layers: Vec<Layer>,
    pub audio: Option<Audio>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_mix: Option<crate::audio::Mix>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expressions: Option<crate::expressions::Program>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporal: Option<crate::temporal::Exposure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub geometry: Option<crate::geometry::Geometry>,
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
        let frames = self.duration.units(FPS)?;
        if self.schema_version != 1
            || !bounded_id(&self.id)
            || !(1..=512).contains(&self.width)
            || !(1..=512).contains(&self.height)
            || !(1..=8).contains(&self.output_scale)
            || !(1..=250).contains(&frames)
            || self.width as u64 * self.height as u64 * frames * 3 > 256 * 1024 * 1024
            || self.width * self.height * self.output_scale * self.output_scale > 8_000_000
            || self.layers.is_empty()
            || self.layers.len() > 16
        {
            return Err(invalid(
                "Scene v1 requires 1-250 frames at 25 fps, 1-16 layers, canvas up to 512 squared, output scale 1-8, at most 8M output pixels and 256 MiB raw video",
            ));
        }
        let mut ids = HashSet::new();
        let mut references = 0;
        for layer in &self.layers {
            references += layer.frames.len();
            if !bounded_id(&layer.id)
                || !ids.insert(&layer.id)
                || layer.canvas.contains(&0)
                || layer.canvas.iter().any(|n| *n > 512)
                || (layer.frames.is_empty() != layer.graphics.is_some())
                || references > 1024
                || layer.duration.units(FPS)? == 0
                || layer.start.units(FPS)? as u128 + layer.duration.units(FPS)? as u128
                    > frames as u128
            {
                return Err(invalid(
                    "Layer IDs must be unique; canvas 1-512; choose frames or graphics; at most 1024 frame references; active ranges must fit the scene",
                ));
            }
            if layer.graphics.is_some()
                && (!matches!(layer.timing, Timing::Strict)
                    || !matches!(layer.end, End::HoldLast)
                    || !matches!(layer.alpha_mode, AlphaMode::Straight))
            {
                return Err(invalid(
                    "Graphics require strict timing, hold_last and straight alpha",
                ));
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
            for f in &layer.frames {
                f.hold.validate()?;
                if f.hold.num == 0 || f.anchor.iter().any(|n| !(-4096..=4096).contains(n)) {
                    return Err(invalid("Frame holds must be positive and anchors bounded"));
                }
                if matches!(layer.timing, Timing::Strict) {
                    f.hold.units(FPS)?;
                }
                total = total.plus(f.hold)?;
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
    if fs::metadata(&path)?.len() != identity.bytes {
        return Err(error(
            "MEDIA_CHANGED",
            "Source byte count differs from its identity",
        ));
    }
    // Read at most the declared bounded length, even if a concurrent writer grows the file.
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(&path)?
        .take(identity.bytes + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != identity.bytes
        || format!("{:x}", Sha256::digest(&bytes)) != identity.sha256
    {
        return Err(error(
            "MEDIA_CHANGED",
            "Source digest differs from its identity",
        ));
    }
    Ok((path, bytes))
}

pub(crate) fn decode_png(bytes: &[u8]) -> Result<Pixels> {
    let fail = |e: png::DecodingError| error("UNSUPPORTED_IMAGE", e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    let mut reader = decoder.read_info().map_err(fail)?;
    let info = reader.info();
    if info.width == 0
        || info.height == 0
        || info.width > 512
        || info.height > 512
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
            "Expected plain or sRGB 8-bit RGB/RGBA PNG up to 512 squared; APNG, ICC, alternate gamma/chromaticity, HDR and orientation metadata are unsupported",
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
            if pixels > 16_000_000 {
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
        for frame in &layer.frames {
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
                    let image = decode_png(&bytes)?;
                    pixels += image.width as u64 * image.height as u64;
                    if pixels > 16_000_000 {
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
                    if pixels > 16_000_000 {
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
            if frame.offset[0] as u64 + image.width as u64 > layer.canvas[0] as u64
                || frame.offset[1] as u64 + image.height as u64 > layer.canvas[1] as u64
            {
                return Err(invalid(
                    "PNG plus trim offset must fit its declared full source canvas",
                ));
            }
            total = total.plus(frame.hold)?;
        }
        let mut layer_report = json!({"layer_id":layer.id,"source_cycle_duration":total,"timing":layer.timing,"end":layer.end,"selected_frames":selected,"sampled_parameters":sampled,"alpha_mode":layer.alpha_mode,"blend_mode":layer.blend_mode,"mask_inverted":layer.mask.as_ref().map(|m|m.inverted)});
        if let Some(report) = graphic_report {
            layer_report["graphics"] = report;
        }
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
                    let anchor = if layer.graphics.is_some() {
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
    if layer.graphics.is_some() {
        return Ok(Some(0));
    }
    let mut time = time.minus(layer.start)?;
    let total = layer
        .frames
        .iter()
        .try_fold(Time::ZERO, |a, f| a.plus(f.hold))?;
    if time.compare(total)? != std::cmp::Ordering::Less {
        match layer.end {
            End::Transparent => return Ok(None),
            End::HoldLast => return Ok(Some(layer.frames.len() - 1)),
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
    for (i, frame) in layer.frames.iter().enumerate() {
        end = end.plus(frame.hold)?;
        if time.compare(end)? == std::cmp::Ordering::Less {
            return Ok(Some(i));
        }
    }
    Err(invalid("Animation sampling exceeded duration"))
}

fn compose_sample(scene: &Scene, prepared: &Prepared, sample: usize) -> Result<Vec<u8>> {
    if let Some(geometry) = &prepared.geometry {
        return Ok(if let Some(state) = &geometry.states[sample] {
            compose_geometry(scene, prepared, state, sample)
        } else {
            scene
                .background
                .repeat((scene.width * scene.height) as usize)
        });
    }
    let mut rgb = scene
        .background
        .repeat((scene.width * scene.height) as usize);
    for (layer_index, layer) in scene.layers.iter().enumerate() {
        let Some(index) = prepared.selected[layer_index][sample] else {
            continue;
        };
        let (image, offset, anchor) = if layer.graphics.is_some() {
            (&prepared.graphics[&layer.id], [0, 0], [0, 0])
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
                    Some(if let Some(p) = processed {
                        (p, AlphaMode::Straight, coverage)
                    } else {
                        (p, layer.alpha_mode, coverage)
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
        // Iterate destination pixels, so off-canvas scales cannot cause unbounded work.
        for dy in 0..scene.height {
            for dx in 0..scene.width {
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
                let (p, alpha_mode) = if let Some(processed) = &processed {
                    (processed.as_slice(), AlphaMode::Straight)
                } else {
                    (p, layer.alpha_mode)
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

fn compose(scene: &Scene, prepared: &Prepared, frame: u64) -> Result<Vec<u8>> {
    let samples = prepared.samples_per_frame;
    let first = frame as usize * samples;
    if samples == 1 {
        return compose_sample(scene, prepared, first);
    }
    let mut sums = vec![0u32; (scene.width * scene.height * 3) as usize];
    for sample in first..first + samples {
        let rgb = compose_sample(scene, prepared, sample)?;
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
    let mut video = BufWriter::new(File::create_new(scratch.0.join("video.rgb"))?);
    let frames = scene.duration.units(FPS)?;
    for n in 0..frames {
        video.write_all(&compose(scene, &prepared, n)?)?;
    }
    video.flush()?;
    drop(video);
    let mut audio = BufWriter::new(File::create_new(scratch.0.join("audio.pcm"))?);
    for sample in &prepared.pcm {
        audio.write_all(&sample.to_le_bytes())?;
    }
    audio.flush()?;
    drop(audio);
    let temp = scratch.0.join("output.mkv");
    let (ffv1_level, ffv1_slices) = media::ffv1_encoding(
        scene.width * scene.output_scale,
        scene.height * scene.output_scale,
    );
    let args = vec![
        "-hide_banner".into(),
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-n".into(),
        "-f".into(),
        "rawvideo".into(),
        "-pixel_format".into(),
        "rgb24".into(),
        "-video_size".into(),
        format!("{}x{}", scene.width, scene.height),
        "-framerate".into(),
        "25".into(),
        "-i".into(),
        scratch.0.join("video.rgb").to_string_lossy().into_owned(),
        "-f".into(),
        "s16le".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-i".into(),
        scratch.0.join("audio.pcm").to_string_lossy().into_owned(),
        "-vf".into(),
        format!(
            "scale={}:{}:flags=neighbor,setsar=1",
            scene.width * scene.output_scale,
            scene.height * scene.output_scale
        ),
        "-c:v".into(),
        "ffv1".into(),
        "-level".into(),
        ffv1_level.into(),
        "-slices".into(),
        ffv1_slices.into(),
        "-pix_fmt".into(),
        "bgr0".into(),
        "-threads".into(),
        "1".into(),
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
    media::capture(&media::tool("ffmpeg"), &args, Duration::from_secs(180))?;
    let verified = render::inspect_reference(
        &temp,
        scene.width * scene.output_scale,
        scene.height * scene.output_scale,
        &media::Uncontrolled,
    )?;
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
    report["asset"] = json!({"id":scene.id,"path":output,"duration":scene.duration});
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
