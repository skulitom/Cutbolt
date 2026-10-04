//! Identity-bound preview variants; timeline clocks and final media remain unchanged.
use crate::{
    Result, conform, error, media,
    model::{Asset, Operation, Project},
    registry::{self, Identity},
    render, scene,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
const FPS: Time = Time { num: 25, den: 1 };

/// Preview proxy bound to an asset, usually as returned by proxy.generate. Previews use it only when preview.proxy selects its scale.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// Proxy file path: absolute, or relative normal components resolved under `input_root`.
    pub path: String,
    /// Content identity of the proxy file.
    pub identity: Identity,
    /// Identity of the full-quality source; must equal the asset's bound identity.
    pub source_identity: Identity,
    /// Dimension divisor 2, 4 or 8; project width and height must divide exactly.
    pub scale: u32,
    /// Proxy frame count; must equal the asset duration in 25 fps frames.
    pub frames: u64,
}
pub(crate) fn dimensions(project: &Project, scale: u32) -> Result<(u32, u32)> {
    if ![2, 4, 8].contains(&scale)
        || project.width == 0
        || project.height == 0
        || !project.width.is_multiple_of(scale)
        || !project.height.is_multiple_of(scale)
        || project.frame_rate.compare(FPS)? != std::cmp::Ordering::Equal
    {
        return Err(error(
            "INVALID_PROXY",
            "Proxies require 25 fps and an exact dimension divisor of 2, 4 or 8",
        ));
    }
    Ok((project.width / scale, project.height / scale))
}
impl Binding {
    pub(crate) fn validate(&self, project: &Project, asset: &Asset) -> Result<()> {
        dimensions(project, self.scale)?;
        self.identity.validate()?;
        self.source_identity.validate()?;
        if self.path.is_empty() || self.frames == 0 || self.frames != asset.duration.units(FPS)? {
            return Err(error(
                "INVALID_PROXY",
                "Proxy requires a path and the complete source duration",
            ));
        }
        if asset.identity.as_ref() != Some(&self.source_identity) {
            return Err(error(
                "IDENTITY_MISMATCH",
                "Proxy belongs to a different full-quality source identity",
            ));
        }
        Ok(())
    }
}
fn asset<'a>(project: &'a Project, id: &str) -> Result<&'a Asset> {
    project
        .assets
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| error("MISSING_MEDIA", id))
}
/// proxy.generate request: write a reduced-size FFV1/PCM proxy of one asset and propose a media.proxy.attach operation. The project is not saved.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Generate {
    /// Project containing the asset; it must be 25 fps.
    #[schemars(with = "crate::reference::ProjectInput")]
    pub project: Project,
    /// Must equal `project.revision`.
    pub expected_revision: u64,
    /// Asset to proxy; it needs a bound identity and a 25 fps reference source covering its full duration.
    pub asset_id: String,
    /// Dimension divisor 2, 4 or 8; project width and height must divide exactly.
    pub scale: u32,
    /// Existing absolute directory that contains the source.
    pub input_root: PathBuf,
    /// Existing absolute directory that receives the output.
    pub output_root: PathBuf,
    /// New absolute `.mkv` path inside `output_root`; existing files are never overwritten.
    pub output: PathBuf,
}
pub fn generate(request: &Generate) -> Result<Value> {
    let project = &request.project;
    project.validate()?;
    if project.revision != request.expected_revision {
        return Err(error(
            "REVISION_CONFLICT",
            "Expected revision does not match supplied snapshot",
        ));
    }
    // Validate the proposed revision before publishing any generated media.
    let mut next_revision = project.clone();
    next_revision.revision += 1;
    next_revision.validate()?;
    let (width, height) = dimensions(project, request.scale)?;
    let asset = asset(project, &request.asset_id)?;
    let identity = asset.identity.as_ref().ok_or_else(|| {
        error(
            "IDENTITY_REQUIRED",
            "Bind full-quality source identity before proxy generation",
        )
    })?;
    let path = media::project_file(Path::new(&asset.path), &request.input_root)?;
    let source =
        render::inspect_reference(&path, project.width, project.height, &media::Uncontrolled)?;
    registry::verify_source(asset, &source)?;
    if asset.duration.units(FPS)? != source.frames {
        return Err(error(
            "INVALID_PROXY",
            "Asset duration must describe the complete decoded source",
        ));
    }
    let root = request.input_root.canonicalize()?;
    let relative = path
        .strip_prefix(&root)
        .map_err(|_| error("INVALID_PATH", "Source must be inside input root"))?;
    let recipe = conform::Recipe {
        decode: None,
        schema_version: 1,
        id: asset.id.clone(),
        source: conform::Source {
            file: scene::Identity {
                path: relative.to_path_buf(),
                sha256: identity.sha256.clone(),
                bytes: identity.bytes,
            },
            color: Some(conform::Color::EncodedRgb),
            sdr: None,
        },
        source_in: Time::ZERO,
        duration: asset.duration,
        rate: Time { num: 1, den: 1 },
        reverse: false,
        freeze: false,
        width,
        height,
        audio: conform::Audio::Resample,
        remap: None,
        working_transfer: None,
        lut: None,
    };
    let mut result = conform::run(&recipe, &root, &request.output_root, &request.output)?;
    let proxy = Binding {
        path: result["asset"]["path"]
            .as_str()
            .expect("conform asset path")
            .into(),
        identity: serde_json::from_value(result["asset"]["identity"].clone())?,
        source_identity: identity.clone(),
        scale: request.scale,
        frames: source.frames,
    };
    let operations = vec![Operation::ProxyAttach {
        asset_id: asset.id.clone(),
        proxy: proxy.clone(),
    }];
    let updated = project.apply(request.expected_revision, operations.clone())?;
    result["profile"] = json!("proxy-ffv1-pcm-v1");
    result["proxy"] = json!(proxy);
    result["project"] = json!(updated);
    result["operations"] = json!(operations);
    result
        .as_object_mut()
        .expect("conform receipt")
        .remove("asset");
    Ok(result)
}
pub fn relink(
    project: &Project,
    revision: u64,
    input_root: &Path,
    id: &str,
    candidates: &[PathBuf],
) -> Result<Value> {
    project.validate()?;
    let proxy = asset(project, id)?
        .proxy
        .as_ref()
        .ok_or_else(|| error("PROXY_MISSING", "Asset has no proxy binding"))?;
    let path = registry::match_identity(input_root, &proxy.identity, candidates)?;
    let operations = vec![Operation::ProxyRelink {
        asset_id: id.into(),
        path: path.to_string_lossy().into_owned(),
    }];
    let updated = project.apply(revision, operations.clone())?;
    Ok(json!({"project":updated,"operations":operations,"identity":proxy.identity}))
}
pub fn status(project: &Project, root: &Path) -> Result<Value> {
    project.validate()?;
    let mut mapped = project.clone();
    mapped.preview_scale = None;
    mapped.clips.clear();
    mapped.tracks = None;
    mapped.assets.clear();
    let mut missing = vec![];
    for a in &project.assets {
        if let Some(proxy) = &a.proxy {
            let mut a = a.clone();
            a.path = proxy.path.clone();
            a.identity = Some(proxy.identity.clone());
            a.proxy = None;
            mapped.assets.push(a);
        } else {
            missing.push(json!({"asset_id":a.id,"state":"no_proxy"}));
        }
    }
    let mut result = registry::status(&mapped, root)?;
    result["assets"]
        .as_array_mut()
        .expect("registry assets")
        .extend(missing);
    result["preview_scale"] = json!(project.preview_scale);
    Ok(result)
}
pub(crate) fn preview_project(project: &Project, root: &Path) -> Result<Project> {
    project.validate()?;
    let Some(scale) = project.preview_scale else {
        return Ok(project.clone());
    };
    if project.tracks.as_ref().is_some_and(|a| {
        a.tracks
            .iter()
            .any(|t| !t.composite.is_opaque() && t.enabled && !t.clips.is_empty())
    }) {
        return Err(error(
            "UNSUPPORTED_PREVIEW",
            "Proxy previews do not support alpha_over tracks; select full quality with preview.proxy null",
        ));
    }
    let (width, height) = dimensions(project, scale)?;
    let used = crate::sequences::used_assets(project)?;
    let mut mapped = project.clone();
    mapped.width = width;
    mapped.height = height;
    mapped.preview_scale = None;
    for a in &mut mapped.assets {
        if used.contains(&a.id) {
            // Audio-only WAV remains full quality when video previews select proxies.
            if project.tracks.is_some()
                && Path::new(&a.path)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
            {
                if a.proxy.is_some() {
                    return Err(error(
                        "INVALID_PROXY",
                        "Audio-only WAV does not accept a video proxy binding",
                    ));
                }
                let path = media::project_file(Path::new(&a.path), root)?;
                let wave = crate::pcm_stream::inspect(&path, &media::Uncontrolled)?;
                registry::verify_source(
                    a,
                    &render::Source {
                        path,
                        sha256: wave.sha256,
                        frames: 0,
                        samples: wave.frames,
                    },
                )?;
                continue;
            }
            let proxy = a
                .proxy
                .as_ref()
                .ok_or_else(|| error("PROXY_MISSING", format!("Asset {} has no proxy", a.id)))?;
            if proxy.scale != scale {
                return Err(error(
                    "INVALID_PROXY",
                    format!("Asset {} proxy scale differs from preview choice", a.id),
                ));
            }
            let path = media::project_file(Path::new(&proxy.path), root)?;
            let source = render::inspect_reference(&path, width, height, &media::Uncontrolled)?;
            if source.frames != proxy.frames
                || source.sha256 != proxy.identity.sha256
                || path.metadata()?.len() != proxy.identity.bytes
            {
                return Err(error(
                    "IDENTITY_MISMATCH",
                    format!(
                        "Asset {} proxy differs from its bound content or duration",
                        a.id
                    ),
                ));
            }
            a.path = path.to_string_lossy().into_owned();
            a.identity = Some(proxy.identity.clone());
        }
        a.proxy = None;
    }
    mapped.validate()?;
    Ok(mapped)
}
pub fn capabilities() -> Value {
    json!({"profile":"proxy-ffv1-pcm-v1","scales":[2,4,8],"requires_bound_reference_source":true,"timing":"one_proxy_frame_per_full_quality_frame","audio":"unchanged_pcm","preview_switch":"session.apply preview.proxy; null selects originals","final_export":"always_full_quality_sources","generation":"blocking_cli_library_with_media_conform_limits","missing_selected_proxy":"explicit_error"})
}
