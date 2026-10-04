//! Explicit local reuse of identity-bound probes, proxy media and previews.
pub use crate::cache_store::Policy;
use crate::cache_store::{Store, Temporary, digest};
use crate::{
    Result, error, media,
    model::{Operation, Project},
    preview, proxy,
    registry::Identity,
    render,
    time::Time,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};
include!(concat!(env!("OUT_DIR"), "/engine_sources.rs"));

/// Cacheable task, tagged by `type`; file tasks copy the cached result to a new unused `output` inside `output_root`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Task {
    /// Probe metadata and content identity of one source file.
    Probe {
        /// Absolute path of a file inside `input_root`.
        path: PathBuf,
        /// Existing absolute directory containing `path`.
        input_root: PathBuf,
    },
    /// Lossless proxy for one bound asset, like proxy.generate; returns operations to attach it.
    Proxy {
        /// Project containing the asset.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Must equal `project.revision`.
        expected_revision: u64,
        /// ID of an asset with a bound identity.
        asset_id: String,
        /// Size divisor 2, 4 or 8; project dimensions must divide exactly (25 fps only).
        scale: u32,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Existing absolute directory that must contain `output`.
        output_root: PathBuf,
        /// Unused absolute `.mkv` path inside `output_root`.
        output: PathBuf,
    },
    /// One timeline frame as PNG, like preview.frame.
    Frame {
        /// Project to preview.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Frame time in rational seconds; a frame boundary before the timeline end.
        time: Time,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Existing absolute directory that must contain `output`.
        output_root: PathBuf,
        /// Unused absolute `.png` path inside `output_root`.
        output: PathBuf,
    },
    /// Timeline interval as a lossless MKV, like preview.range.
    Interval {
        /// Project to preview.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Interval start in rational seconds on a frame boundary.
        start: Time,
        /// Positive interval length in rational seconds, ending inside the timeline.
        duration: Time,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Existing absolute directory that must contain `output`.
        output_root: PathBuf,
        /// Unused absolute `.mkv` path inside `output_root`.
        output: PathBuf,
    },
    /// Contact sheet PNG, like preview.sheet.
    Sheet {
        /// Project to sample.
        #[schemars(with = "crate::reference::ProjectInput")]
        project: Project,
        /// Sheet times and layout.
        spec: preview::Sheet,
        /// Existing absolute directory that project media paths resolve against.
        input_root: PathBuf,
        /// Existing absolute directory that must contain `output`.
        output_root: PathBuf,
        /// Unused absolute `.png` path inside `output_root`.
        output: PathBuf,
    },
}
/// cache.run request: performs or reuses one task through an identity-keyed local cache.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Existing absolute directory holding the cache database; keep it apart from media.
    pub cache_root: PathBuf,
    /// Cache budget, applied before the task runs.
    pub policy: Policy,
    /// Task to perform or reuse.
    pub task: Task,
}

#[derive(Clone, Debug, Serialize)]
struct Source {
    id: String,
    path: PathBuf,
    identity: Identity,
}
fn source(id: String, path: &Path, root: &Path, expected: Option<&Identity>) -> Result<Source> {
    let path = media::allowed_file(path, root)?;
    let identity = Identity {
        bytes: fs::metadata(&path)?.len(),
        sha256: media::file_hash(&path)?,
    };
    if expected.is_some_and(|e| *e != identity) {
        return Err(error(
            "MEDIA_CHANGED",
            "Cache source differs from its declared identity",
        ));
    }
    Ok(Source { id, path, identity })
}
fn verify_sources(sources: &[Source]) -> Result<()> {
    for s in sources {
        if fs::metadata(&s.path)?.len() != s.identity.bytes
            || media::file_hash(&s.path)? != s.identity.sha256
        {
            return Err(error(
                "MEDIA_CHANGED",
                "A cache dependency changed before publication",
            ));
        }
    }
    Ok(())
}
fn project_sources(p: &Project, root: &Path, proxy_only: Option<&str>) -> Result<Vec<Source>> {
    p.validate()?;
    let used = crate::sequences::used_assets(p)?;
    let mut output = Vec::new();
    for asset in &p.assets {
        if proxy_only.is_some_and(|id| id != asset.id)
            || (proxy_only.is_none() && !used.contains(&asset.id))
        {
            continue;
        }
        let mut path = Path::new(&asset.path);
        let mut expected = asset.identity.as_ref();
        if proxy_only.is_none()
            && let Some(scale) = p.preview_scale
            && !(p.tracks.is_some()
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("wav")))
        {
            let binding = asset
                .proxy
                .as_ref()
                .ok_or_else(|| error("PROXY_MISSING", "Selected preview proxy is missing"))?;
            if binding.scale != scale {
                return Err(error("INVALID_PROXY", "Selected proxy scale differs"));
            }
            path = Path::new(&binding.path);
            expected = Some(&binding.identity);
        }
        let resolved = media::project_file(path, root)?;
        output.push(source(asset.id.clone(), &resolved, root, expected)?);
    }
    output.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(output)
}
fn normalized_project(project: &Project) -> Result<Value> {
    let mut value = serde_json::to_value(project)?;
    for asset in value["assets"].as_array_mut().expect("project assets") {
        asset.as_object_mut().expect("asset").remove("path");
        if let Some(proxy) = asset.get_mut("proxy").and_then(Value::as_object_mut) {
            proxy.remove("path");
        }
    }
    Ok(value)
}
fn producer() -> String {
    let mut h = Sha256::new();
    h.update(std::env::consts::OS);
    h.update(std::env::consts::ARCH);
    h.update(BUILD_CONFIGURATION);
    for (name, bytes) in ENGINE_SOURCES {
        h.update((name.len() as u64).to_le_bytes());
        h.update(name.as_bytes());
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    }
    format!("{:x}", h.finalize())
}
fn tool(name: &str) -> Result<Source> {
    let selected = media::tool(name);
    let p = Path::new(&selected);
    let mut candidates = Vec::new();
    if p.is_absolute() || p.components().count() > 1 {
        candidates.push(p.to_path_buf());
    } else {
        let mut dirs = Vec::new();
        #[cfg(windows)]
        {
            if let Some(parent) = std::env::current_exe()?.parent() {
                dirs.push(parent.to_path_buf());
            }
            dirs.push(std::env::current_dir()?);
            if let Some(windows) = std::env::var_os("SystemRoot") {
                dirs.push(PathBuf::from(&windows).join("System32"));
                dirs.push(PathBuf::from(windows));
            }
        }
        dirs.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        for dir in dirs {
            #[cfg(windows)]
            if p.extension().is_none() {
                candidates.push(dir.join(format!("{selected}.exe")));
            }
            candidates.push(dir.join(&selected));
        }
    }
    let path = candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| error("TOOL_UNAVAILABLE", format!("Cannot resolve {name}")))?
        .canonicalize()?;
    Ok(Source {
        id: name.into(),
        identity: Identity {
            bytes: fs::metadata(&path)?.len(),
            sha256: media::file_hash(&path)?,
        },
        path,
    })
}
pub fn inspect(root: &Path) -> Result<Value> {
    Store::open(root, false)?.inspect()
}
pub fn prune(root: &Path, policy: &Policy) -> Result<Value> {
    policy.validate()?;
    Store::open(root, false)?.prune(policy)
}

pub fn run(request: &Request) -> Result<Value> {
    let start = Instant::now();
    request.policy.validate()?;
    let (kind, recipe, sources, destination) = match &request.task {
        Task::Probe { path, input_root } => (
            "probe",
            json!({}),
            vec![source("probe".into(), path, input_root, None)?],
            None,
        ),
        Task::Proxy {
            project,
            expected_revision,
            asset_id,
            scale,
            input_root,
            output_root,
            output,
        } => {
            project.validate()?;
            proxy::dimensions(project, *scale)?;
            if project.revision != *expected_revision {
                return Err(error("REVISION_CONFLICT", "Expected revision differs"));
            }
            let asset = project
                .assets
                .iter()
                .find(|a| a.id == *asset_id)
                .ok_or_else(|| error("MISSING_MEDIA", asset_id))?;
            if asset.identity.is_none() {
                return Err(error(
                    "IDENTITY_REQUIRED",
                    "Cached proxies require a bound original",
                ));
            }
            (
                "proxy",
                json!({"project":normalized_project(project)?,"asset_id":asset_id,"scale":scale}),
                project_sources(project, input_root, Some(asset_id))?,
                Some(render::destination_extension(output, output_root, "mkv")?),
            )
        }
        Task::Frame {
            project,
            time,
            input_root,
            output_root,
            output,
        } => {
            let total = preview::validate(project)?;
            if time.units(project.frame_rate)? >= total {
                return Err(error(
                    "INVALID_RANGE",
                    "Preview must be before timeline end",
                ));
            }
            (
                "frame",
                json!({"project":normalized_project(project)?,"time":time}),
                project_sources(project, input_root, None)?,
                Some(render::destination_extension(output, output_root, "png")?),
            )
        }
        Task::Interval {
            project,
            start,
            duration,
            input_root,
            output_root,
            output,
        } => {
            let total = preview::validate(project)?;
            let begin = start.units(project.frame_rate)?;
            let count = duration.units(project.frame_rate)?;
            if count == 0 || begin as u128 + count as u128 > total as u128 {
                return Err(error(
                    "INVALID_RANGE",
                    "Preview interval is outside timeline",
                ));
            }
            (
                "interval",
                json!({"project":normalized_project(project)?,"start":start,"duration":duration}),
                project_sources(project, input_root, None)?,
                Some(render::destination_extension(output, output_root, "mkv")?),
            )
        }
        Task::Sheet {
            project,
            spec,
            input_root,
            output_root,
            output,
        } => {
            spec.validate(project)?;
            (
                "sheet",
                json!({"project":normalized_project(project)?,"spec":spec}),
                project_sources(project, input_root, None)?,
                Some(render::destination_extension(output, output_root, "png")?),
            )
        }
    };
    let mut tools = vec![tool("ffprobe")?];
    if kind != "probe" {
        tools.push(tool("ffmpeg")?);
    }
    let identities = |values: &[Source]| {
        values
            .iter()
            .map(|s| json!({"id":s.id,"identity":s.identity}))
            .collect::<Vec<_>>()
    };
    let producer = producer();
    let key = digest(&serde_json::to_vec(
        &json!({"schema_version":1,"producer":producer,"kind":kind,"recipe":recipe,
        "sources":identities(&sources),"tools":identities(&tools)}),
    )?);
    let database = media::input_root(&request.cache_root)?.join(crate::cache_store::FILE);
    if database.try_exists()?
        && sources
            .iter()
            .any(|s| database.canonicalize().is_ok_and(|p| s.path == p))
    {
        return Err(error(
            "INVALID_CACHE",
            "The cache database cannot also be input media",
        ));
    }
    let mut store = Store::open(&request.cache_root, true)?;
    let pruning = store.prune(&request.policy)?;
    // Temporary reserves its exact filename, then yields it for a no-overwrite
    // native producer or cache read. The guard only removes this owned file.
    let extension = if kind == "probe" {
        "json"
    } else if matches!(kind, "frame" | "sheet") {
        "png"
    } else {
        "mkv"
    };
    let temp = Temporary::new(&store.root, extension)?;
    fs::remove_file(&temp.0)?;
    let hit = store.read(&key, kind, &temp.0)?;
    let was_hit = hit.is_some();
    let (identity, metadata, stored) = if let Some(hit) = hit {
        (
            hit.identity,
            hit.metadata,
            json!({"stored":true,"evicted":[]}),
        )
    } else {
        let metadata = match &request.task {
            Task::Probe { .. } => {
                let value = media::probe(&sources[0].path)?;
                let mut file = File::create_new(&temp.0)?;
                file.write_all(&serde_json::to_vec(&value)?)?;
                file.sync_all()?;
                json!({})
            }
            Task::Proxy {
                project,
                expected_revision,
                asset_id,
                scale,
                input_root,
                ..
            } => {
                let value = proxy::generate(&proxy::Generate {
                    project: project.clone(),
                    expected_revision: *expected_revision,
                    asset_id: asset_id.clone(),
                    scale: *scale,
                    input_root: input_root.clone(),
                    output_root: store.root.clone(),
                    output: temp.0.clone(),
                })?;
                json!({"frames":value["proxy"]["frames"]})
            }
            Task::Frame {
                project,
                time,
                input_root,
                ..
            } => {
                let value = preview::frame(project, input_root, &store.root, &temp.0, *time)?;
                json!({"width":value["width"],"height":value["height"],"timeline_frame":value["timeline_frame"]})
            }
            Task::Interval {
                project,
                start,
                duration,
                input_root,
                ..
            } => {
                let value =
                    preview::range(project, input_root, &store.root, &temp.0, *start, *duration)?;
                json!({"width":value["width"],"height":value["height"],"frames":duration.units(project.frame_rate)?,"samples":duration.units(Time{num:48000,den:1})?})
            }
            Task::Sheet {
                project,
                spec,
                input_root,
                ..
            } => {
                let mut value = preview::sheet(project, spec, input_root, &store.root, &temp.0)?;
                value
                    .as_object_mut()
                    .expect("sheet receipt")
                    .remove("output");
                value
                    .as_object_mut()
                    .expect("sheet receipt")
                    .remove("sha256");
                value
            }
        };
        verify_sources(&sources)?;
        verify_sources(&tools)?;
        let identity = Identity {
            bytes: fs::metadata(&temp.0)?.len(),
            sha256: media::file_hash(&temp.0)?,
        };
        let stored = store.put(&key, kind, &temp.0, &identity, &metadata, &request.policy)?;
        (identity, metadata, stored)
    };
    verify_sources(&sources)?;
    verify_sources(&tools)?;
    // Validate any proposed project mutation before a destination can appear.
    let proxy_result = if let Task::Proxy {
        project,
        expected_revision,
        asset_id,
        scale,
        ..
    } = &request.task
    {
        let source = &sources[0];
        let frames = metadata["frames"]
            .as_u64()
            .ok_or_else(|| error("CACHE_CORRUPT", "Proxy frame count missing"))?;
        let binding = proxy::Binding {
            path: destination
                .as_ref()
                .expect("proxy output")
                .to_string_lossy()
                .into_owned(),
            identity: identity.clone(),
            source_identity: source.identity.clone(),
            scale: *scale,
            frames,
        };
        let operations = vec![Operation::ProxyAttach {
            asset_id: asset_id.clone(),
            proxy: binding.clone(),
        }];
        let updated = project.apply(*expected_revision, operations.clone())?;
        Some((binding, operations, updated))
    } else {
        None
    };
    let mut result = if kind == "probe" {
        let mut value: Value = serde_json::from_reader(File::open(&temp.0)?)?;
        if let Some(format) = value.get_mut("format").and_then(Value::as_object_mut) {
            format.insert("filename".into(), json!(sources[0].path));
        }
        json!({"path":sources[0].path,"metadata":value,"identity":sources[0].identity})
    } else {
        let output = destination.as_ref().expect("file task output");
        // Copy to an owned file beside the destination: publication remains on
        // its filesystem even when cache and delivery roots are on different drives.
        let publish = Temporary::new(output.parent().expect("output parent"), extension)?;
        fs::copy(&temp.0, &publish.0)?;
        if media::file_hash(&publish.0)? != identity.sha256 {
            return Err(error("CACHE_CORRUPT", "Cache delivery copy changed"));
        }
        verify_sources(&sources)?;
        verify_sources(&tools)?;
        media::publish(&publish.0, output)?;
        json!({"output":output,"identity":identity,"details":metadata})
    };
    if let Some((binding, operations, updated)) = proxy_result {
        result["proxy"] = json!(binding);
        result["project"] = json!(updated);
        result["operations"] = json!(operations);
    }
    let actual: BTreeMap<_, _> = sources
        .iter()
        .map(|s| (s.id.clone(), json!({"path":s.path,"identity":s.identity})))
        .collect();
    Ok(
        json!({"result":result,"cache":{"key":key,"producer":producer,"hit":was_hit,"store":stored,"budget_evictions":pruning["evicted"],
        "verified_sources":actual,"elapsed_micros":start.elapsed().as_micros()}}),
    )
}
