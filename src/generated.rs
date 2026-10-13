//! Saved, bounded scene sources; their pixels join the native timeline stream directly.
use crate::{Result, error, media, model::Project, scene::Scene, time::Time};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Mutex},
};

/// An inline scene, or a workspace document reference expanded before the edit. The saved
/// snapshot contains the resolved recipe and pinned inputs; changing its authoring JSON later
/// does not silently change a saved revision. Scene sound is available on audio tracks.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct SceneRef(pub Box<Scene>);
impl PartialEq for SceneRef {
    fn eq(&self, other: &Self) -> bool {
        // Equality follows the saved representation, including exact clocks and identities.
        serde_json::to_value(self).expect("serializable scene")
            == serde_json::to_value(other).expect("serializable scene")
    }
}
impl Eq for SceneRef {}

pub fn capabilities() -> serde_json::Value {
    serde_json::json!({"source":"native_track_clip.scene","schema_version":2,
        "document":"workspace_file_expanded_and_pinned_before_save","video":"per_frame_scene_composition_no_intermediate_movie",
        "audio":"scene_pcm_on_audio_tracks","clock":"clip_source_time","dimensions":"match_original_project_canvas",
        "scene_limits":crate::scene::limits(),"shared_prepared_payload_bytes":1024*1024*1024u64,
        "previews":"frame_range_and_proxy","nested_sequences":true,"dependencies":"full_hash_before_publication_and_cache_hits"})
}
impl SceneRef {
    pub(crate) fn validate(&self, project: &Project) -> Result<Time> {
        self.0.validate()?;
        let scale = project.effect_coordinate_scale.unwrap_or(1);
        if self.0.width * self.0.output_scale != project.width * scale
            || self.0.height * self.0.output_scale != project.height * scale
            || self.0.clock()? != project.frame_rate
        {
            return Err(error(
                "INVALID_SCENE",
                "Generated scene output size and frame rate must match the original project canvas and clock",
            ));
        }
        pinned(&serde_json::to_value(self)?)?;
        Ok(self.0.duration)
    }
    pub(crate) fn key(&self) -> Result<String> {
        Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(self)?)))
    }
}
fn pinned(value: &serde_json::Value) -> Result<()> {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            if map.contains_key("path")
                && map.contains_key("sha256")
                && map.contains_key("bytes")
                && (map["bytes"].as_u64().is_none_or(|n| n == 0)
                    || map["sha256"].as_str().is_none_or(|s| {
                        s.len() != 64
                            || !s
                                .bytes()
                                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                    }))
            {
                return Err(error(
                    "INVALID_SCENE",
                    "Saved generated scenes require pinned file sizes and SHA-256 identities; resolve the scene in its input workspace before placing it",
                ));
            }
            for item in map.values() {
                pinned(item)?;
            }
        }
        Value::Array(items) => {
            for item in items {
                pinned(item)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Content dependencies for cache hits, including scenes in unused saved definitions. This
/// verifies files without rendering frames or allocating the scene's decoded image buffers.
pub(crate) fn dependencies(
    source: &SceneRef,
    root: &Path,
) -> Result<Vec<(crate::render::Source, u64)>> {
    fn visit(
        value: &serde_json::Value,
        root: &Path,
        font: bool,
        out: &mut Vec<(crate::render::Source, u64)>,
    ) -> Result<()> {
        use serde_json::Value;
        match value {
            Value::Object(map)
                if map.contains_key("path")
                    && map.contains_key("sha256")
                    && map.contains_key("bytes") =>
            {
                let identity: crate::scene::Identity = serde_json::from_value(value.clone())?;
                let path = if font {
                    crate::fonts::identity_bytes(&identity, root)?.0
                } else {
                    crate::scene::identity_file(&identity, root, 1024 * 1024 * 1024)?
                };
                out.push((
                    crate::render::Source {
                        path,
                        sha256: identity.sha256,
                        frames: 0,
                        samples: 0,
                        conversion: None,
                    },
                    identity.bytes,
                ));
            }
            Value::Object(map) => {
                for (key, item) in map {
                    visit(item, root, font || key == "fonts", out)?;
                }
            }
            Value::Array(items) => {
                for item in items {
                    visit(item, root, font, out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    let mut result = Vec::new();
    visit(&serde_json::to_value(source)?, root, false, &mut result)?;
    Ok(result)
}

/// Resolve only native-clip scene slots. Workspace document expansion already happened, so
/// saved revisions retain the resolved recipe rather than rereading authoring JSON on render.
pub(crate) fn prepare(
    request: &mut serde_json::Value,
    workspace: Option<&crate::workspace::Workspace>,
) -> Result<Vec<serde_json::Value>> {
    fn visit(
        value: &mut serde_json::Value,
        root: Option<&Path>,
        completed: &mut Vec<serde_json::Value>,
    ) -> Result<()> {
        use serde_json::{Value, json};
        match value {
            Value::Array(items) => {
                for item in items {
                    visit(item, root, completed)?;
                }
            }
            Value::Object(map) => {
                if map.contains_key("source_in")
                    && map.contains_key("duration")
                    && let Some(scene) = map.get_mut("scene")
                {
                    let mut request = json!({"command":"scene.inspect","scene":scene});
                    if let Some(root) = root {
                        request["input_root"] = json!(root);
                    }
                    completed.extend(crate::fonts::prepare(&mut request)?);
                    completed.extend(crate::identity::complete(&mut request)?);
                    *scene = request["scene"].take();
                }
                for key in [
                    "project",
                    "tracks",
                    "sequences",
                    "arrangement",
                    "operations",
                    "edits",
                    "edit",
                    "clip",
                    "clips",
                ] {
                    if let Some(item) = map.get_mut(key) {
                        visit(item, root, completed)?;
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
    let root = request["input_root"]
        .as_str()
        .map(std::path::PathBuf::from)
        .or_else(|| workspace.map(|w| w.root().to_path_buf()));
    let mut completed = Vec::new();
    visit(request, root.as_deref(), &mut completed)?;
    Ok(completed)
}

#[derive(Default)]
struct Inner {
    scenes: BTreeMap<String, Arc<Source>>,
    bytes: u64,
}
/// Typed generated data: parents must not encode a working child before blending it.
pub(crate) enum Source {
    Legacy(crate::scene::FrameSource),
    Working(crate::scene::working::Source),
}
impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedSource")
            .field("retained_bytes", &self.retained_bytes())
            .finish_non_exhaustive()
    }
}
impl Source {
    fn new(
        scene: &Scene,
        root: &Path,
        context: Option<crate::color::working::Context>,
    ) -> Result<Self> {
        match context {
            None => crate::scene::FrameSource::new(scene, root).map(Self::Legacy),
            Some(c) => crate::scene::working::Source::new_in(scene, root, c).map(Self::Working),
        }
    }
    pub(crate) fn retained_bytes(&self) -> u64 {
        match self {
            Self::Legacy(s) => s.retained_bytes(),
            Self::Working(s) => s.retained_bytes(),
        }
    }
    pub(crate) fn frame_buffer_bytes(&self) -> u64 {
        match self {
            Self::Legacy(s) => s.frame_buffer_bytes(),
            Self::Working(s) => s.frame_buffer_bytes(),
        }
    }
    pub(crate) fn sources(&self) -> Vec<crate::render::Source> {
        match self {
            Self::Legacy(s) => s.sources(),
            Self::Working(s) => s.sources(),
        }
    }
    pub(crate) fn write_audio(
        &self,
        first: u64,
        count: u64,
        control: &dyn media::Control,
        wave: &mut crate::pcm_stream::Writer<std::fs::File>,
    ) -> Result<()> {
        match self {
            Self::Legacy(s) => s.write_audio(first, count, control, wave),
            Self::Working(s) => {
                let end = first
                    .checked_add(count)
                    .filter(|n| *n <= s.audio_samples())
                    .ok_or_else(|| {
                        error("INVALID_RANGE", "Generated scene audio exceeds its source")
                    })?;
                let mut at = first;
                while at < end {
                    control.check()?;
                    let count = (end - at).min(4096);
                    let pcm = s.pcm(at, count)?;
                    let bytes = pcm.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<_>>();
                    wave.push(&bytes)?;
                    at += count;
                }
                Ok(())
            }
        }
    }
    pub(crate) fn frame(&self, n: u64, divisor: u32) -> Result<Vec<u8>> {
        match self {
            Self::Legacy(s) => s.frame(n, divisor),
            Self::Working(_) => Err(error(
                "INVALID_COLOR",
                "Working scene pixels cannot pass through a byte child source",
            )),
        }
    }
    pub(crate) fn working_frame(
        &self,
        n: u64,
        divisor: u32,
        context: crate::color::working::Context,
    ) -> Result<Vec<crate::color::working::Pixel>> {
        match self {
            Self::Working(s) if s.context() == context => s.frame(n, divisor),
            _ => Err(error(
                "INVALID_COLOR",
                "Generated source and parent working contexts differ",
            )),
        }
    }
}
/// One pool per complete render, including child sequences, audio branches and chunks.
#[derive(Clone, Default)]
pub(crate) struct Pool(Arc<Mutex<Inner>>);
impl Pool {
    pub(crate) fn report(&self) -> Result<Option<serde_json::Value>> {
        let pool = self
            .0
            .lock()
            .map_err(|_| error("RENDER_VALIDATION_FAILED", "Generated scene pool failed"))?;
        Ok((!pool.scenes.is_empty()).then(||serde_json::json!({"scope":"whole_render_shared",
            "scene_count":pool.scenes.len(),"retained_payload_bytes":pool.bytes,"maximum_payload_bytes":1024*1024*1024u64})))
    }
    pub(crate) fn get(
        &self,
        source: &SceneRef,
        root: &Path,
        control: &dyn media::Control,
        context: Option<crate::color::working::Context>,
    ) -> Result<Arc<Source>> {
        control.check()?;
        let key = format!("{}:{}", source.key()?, serde_json::to_string(&context)?);
        let mut pool = self
            .0
            .lock()
            .map_err(|_| error("RENDER_VALIDATION_FAILED", "Generated scene pool failed"))?;
        if let Some(prepared) = pool.scenes.get(&key) {
            return Ok(prepared.clone());
        }
        let prepared = Source::new(&source.0, root, context)?;
        let bytes = prepared.retained_bytes();
        let total = pool.bytes.checked_add(bytes).filter(|&n| n <= 1024 * 1024 * 1024)
            .ok_or_else(|| error("LIMIT_EXCEEDED", "Prepared generated-scene payload exceeds the 1 GiB whole-render bound; shorten scenes or reuse recipes"))?;
        control.check()?;
        let prepared = Arc::new(prepared);
        pool.bytes = total;
        pool.scenes.insert(key, prepared.clone());
        Ok(prepared)
    }
}
