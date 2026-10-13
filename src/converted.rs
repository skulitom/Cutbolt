//! Saved normalization recipes and leased, content-verified timeline sources.
use crate::{
    Result,
    cache::Policy,
    conform, conform_cache, error, media,
    model::{Asset, Decode, Project, Provenance},
    registry::Identity,
    render,
    time::Time,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

const DEFAULT_BYTES: u64 = 50_000_000_000;

pub(crate) fn validate(project: &Project, asset: &Asset) -> Result<()> {
    let Some(Decode::Converted { recipe }) = &asset.decode else {
        return Ok(());
    };
    let identity = asset.identity.as_ref().ok_or_else(|| {
        error(
            "INVALID_IDENTITY",
            "Converted sources need their original identity",
        )
    })?;
    if recipe.source.file.sha256 != identity.sha256
        || recipe.source.file.bytes != identity.bytes
        || recipe.duration != asset.duration
        || recipe.id != asset.id
        || recipe.width != Some(project.width)
        || recipe.height != Some(project.height)
        || recipe.frame_rate.unwrap_or(Time { num: 25, den: 1 }) != project.frame_rate
        || recipe.source_in != Time::ZERO
        || recipe.rate != (Time { num: 1, den: 1 })
        || recipe.reverse
        || recipe.freeze
        || recipe.remap.is_some()
        || recipe.lut.is_some()
        || recipe.schema_version != 1
        || asset.audio_only()
    {
        return Err(error("INVALID_CONFORM", "Saved conversion must bind the whole original at the project canvas and rate, without editorial effects").with_path("asset.decode.recipe"));
    }
    recipe.duration.units(project.frame_rate)?;
    recipe.duration.units(Time::new(48000, 1)?)?;
    Ok(())
}

pub(crate) fn bind(project: &Project, mut asset: Asset, root: &Path) -> Result<Asset> {
    let root = media::input_root(root)?;
    let path = media::project_file(Path::new(&asset.path), &root)?;
    let file = crate::identity::relative(&path, &root)?;
    let identity: Identity =
        serde_json::from_value(json!({"sha256":file["sha256"],"bytes":file["bytes"]}))?;
    if let Some(expected) = &asset.identity {
        crate::registry::check_file(&path, &asset.path, expected, crate::registry::RELINK_ASSET)?;
    }
    asset.identity = Some(identity.clone());
    if asset.direct() {
        return crate::direct::bind(project, asset, &root, false);
    }
    let metadata = media::probe(&path)?;
    let (ready, _) = crate::readiness::inspect(&path, &file, &metadata)?;
    let fits = ready["width"] == project.width
        && ready["height"] == project.height
        && serde_json::from_value::<Time>(ready["frame_rate"].clone())
            .is_ok_and(|v| v == project.frame_rate);
    if ready["direct"] == true && fits && asset.decode.is_none() {
        return crate::direct::bind(project, asset, &root, false);
    }
    if ready["ready"] == true && (fits || ready["audio_only"] == true) && asset.decode.is_none() {
        let duration: Time = serde_json::from_value(ready["duration"].clone())?;
        if duration != asset.duration {
            return Err(duration_error(&asset, duration));
        }
        return Ok(asset);
    }
    let mut recipe = if let Some(Decode::Converted { recipe }) = &asset.decode {
        (**recipe).clone()
    } else {
        let streams = metadata["streams"]
            .as_array()
            .ok_or_else(|| error("UNSUPPORTED_MEDIA", "Source has no streams"))?;
        let video = streams.iter().find(|v|v["codec_type"] == "video").ok_or_else(||error("UNSUPPORTED_MEDIA","Automatic conversion currently requires video; prepare audio-only sources explicitly"))?;
        let audio = streams.iter().find(|v| v["codec_type"] == "audio");
        let known = serde_json::from_value::<Time>(ready["duration"].clone()).ok();
        let mut value = crate::readiness::conform(
            file["path"].as_str().expect("path"),
            video,
            audio,
            Some(project.frame_rate),
            known,
        )
        .map_err(|why| error("UNSUPPORTED_MEDIA", why))?;
        value["source"]["file"] = file.clone();
        value["id"] = json!(asset.id);
        value["width"] = json!(project.width);
        value["height"] = json!(project.height);
        if matches!(
            video["pix_fmt"].as_str(),
            Some("bgra" | "rgba" | "yuva444p10le" | "yuva444p12le" | "yuva444p16le")
        ) {
            value["source"]["alpha"] = json!("straight");
        }
        serde_json::from_value::<conform::Recipe>(value)?
    };
    if recipe.duration != asset.duration {
        return Err(duration_error(&asset, recipe.duration));
    }
    recipe.source.file.path = PathBuf::from(file["path"].as_str().expect("path"));
    asset.decode = Some(Decode::Converted {
        recipe: Box::new(recipe.clone()),
    });
    asset.provenance = Some(Provenance {
        original: identity,
        conversion: None,
    });
    validate(project, &asset)?;
    // Validate the proposed interpretation and every selected source timestamp before saving.
    conform::inspect(&recipe, &root)?;
    Ok(asset)
}
fn duration_error(asset: &Asset, duration: Time) -> crate::Error {
    error(
        "INVALID_MEDIA",
        format!(
            "Asset {:?} duration differs from its complete prepared duration",
            asset.id
        ),
    )
    .with_path("asset.duration")
    .with_detail(json!({"actual_duration":duration}))
}

pub(crate) fn configuration(root: &Path) -> Result<(PathBuf, Policy)> {
    let path = match std::env::var_os("CUTBOLT_CONFORM_CACHE") {
        Some(path) => PathBuf::from(path),
        None => root.join(".cutbolt/cache/conform"),
    };
    if !path.is_absolute() {
        return Err(error(
            "INVALID_CACHE",
            "CUTBOLT_CONFORM_CACHE must be an absolute directory",
        ));
    }
    let max_bytes = match std::env::var("CUTBOLT_CONFORM_CACHE_BYTES") {
        Ok(text) => text.parse::<u64>().map_err(|_| {
            error(
                "INVALID_CACHE",
                "CUTBOLT_CONFORM_CACHE_BYTES must be a positive integer",
            )
        })?,
        Err(std::env::VarError::NotPresent) => DEFAULT_BYTES,
        Err(_) => {
            return Err(error(
                "INVALID_CACHE",
                "Invalid conversion cache byte policy",
            ));
        }
    };
    let policy = Policy {
        max_bytes,
        max_entries: 4096,
    };
    crate::conform_store::validate(&policy)?;
    if max_bytes == 0 {
        return Err(error(
            "INVALID_CACHE",
            "Conversion cache byte policy must be positive",
        ));
    }
    Ok((path, policy))
}

fn used_assets(project: &Project) -> BTreeSet<String> {
    fn walk(
        project: &Project,
        a: &crate::tracks::Arrangement,
        used: &mut BTreeSet<String>,
        seen: &mut BTreeSet<String>,
    ) {
        for track in a.tracks.iter().filter(|t| t.enabled) {
            for clip in &track.clips {
                if let Some(id) = &clip.sequence_id {
                    if seen.insert(id.clone())
                        && let Some(sequence) = project.sequences.iter().find(|s| &s.id == id)
                    {
                        walk(project, &sequence.arrangement, used, seen);
                    }
                } else {
                    used.insert(clip.asset_id.clone());
                }
            }
        }
    }
    let mut used: BTreeSet<_> = project
        .clips
        .iter()
        .filter_map(|c| c.asset_id.clone())
        .collect();
    if let Some(a) = &project.tracks {
        walk(project, a, &mut used, &mut BTreeSet::new());
    }
    used
}
fn recipes(project: &Project, root: &Path) -> Result<Vec<(String, conform::Recipe)>> {
    let used = used_assets(project);
    project
        .assets
        .iter()
        .filter(|a| used.contains(&a.id))
        .filter_map(|asset| {
            let Some(Decode::Converted { recipe }) = &asset.decode else {
                return None;
            };
            Some((|| {
                validate(project, asset)?;
                let path = media::project_file(Path::new(&asset.path), root)?;
                let mut recipe = (**recipe).clone();
                recipe.source.file.path = path
                    .strip_prefix(root)
                    .map_err(|_| error("INVALID_PATH", "Original must remain inside input_root"))?
                    .to_path_buf();
                Ok((asset.id.clone(), recipe))
            })())
        })
        .collect()
}

fn required(project: &Project, root: &Path, id: &str) -> crate::Error {
    // Make the suggested queue usable by a plain CLI caller as well as a workspace.
    // Only fixed engine-state directories are created, without following reparse points.
    let job_root = (|| -> Result<PathBuf> {
        let mut path = root.to_path_buf();
        for name in [".cutbolt", "jobs"] {
            path.push(name);
            match std::fs::create_dir(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(e) => return Err(e.into()),
            }
            let metadata = std::fs::symlink_metadata(&path)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(error(
                    "INVALID_PATH",
                    "Preparation job directory must be a real directory",
                ));
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(error(
                        "INVALID_PATH",
                        "Preparation job directory cannot be a reparse point",
                    ));
                }
            }
        }
        Ok(path)
    })();
    let job_root = match job_root {
        Ok(path) => path,
        Err(failure) => return failure,
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    error("PREPARATION_REQUIRED",format!("Asset {id:?} needs its cached conversion"))
        .with_path(format!("assets.{id}.decode"))
        .with_hint("Queue the preparation request in detail, wait for completion, then retry.")
        .with_detail(json!({"asset_id":id,"request":{"command":"job.start","job_root":job_root,"request_id":format!("prepare-{:x}-{nonce:x}",std::process::id()),
            "run":"media.prepare","arguments":{"project":project,"input_root":root,"output_root":root,"cached":true}}}))
}

pub(crate) fn sources(
    project: &Project,
    root: &Path,
    produce: bool,
    control: &dyn media::Control,
) -> Result<BTreeMap<String, render::Source>> {
    let root = media::input_root(root)?;
    let recipes = recipes(project, &root)?;
    if recipes.is_empty() {
        return Ok(BTreeMap::new());
    }
    let (cache, policy) = configuration(&root)?;
    if !cache.try_exists()? {
        if !produce {
            return Err(required(project, &root, &recipes[0].0));
        }
        std::fs::create_dir_all(&cache)?;
    }
    if produce {
        control.phase("preparing sources")?;
        control.total(recipes.len() as u64);
    }
    let requests: Vec<_> = recipes.iter().map(|(_, r)| r.clone()).collect();
    let leases =
        conform_cache::acquire_many(&cache, &policy, &requests, &root, produce, Some(control))
            .map_err(|e| {
                if e.code == "CACHE_MISSING" {
                    let index = e
                        .detail
                        .as_ref()
                        .and_then(|d| d["recipe_index"].as_u64())
                        .unwrap_or(0) as usize;
                    required(project, &root, &recipes[index.min(recipes.len() - 1)].0)
                } else {
                    e
                }
            })?;
    let mut result = BTreeMap::new();
    for ((id, recipe), lease) in recipes.into_iter().zip(leases) {
        let source = render::Source {
            path: lease.entry.path.clone(),
            sha256: lease.entry.identity.sha256.clone(),
            frames: recipe.duration.units(project.frame_rate)?,
            samples: recipe.duration.units(Time::new(48000, 1)?)?,
            conversion: Some(lease),
        };
        result.insert(id, source);
    }
    if produce {
        control.frames(result.len() as u64)?;
    }
    Ok(result)
}
pub(crate) fn prepare(project: &Project, root: &Path) -> Result<Value> {
    project.validate()?;
    let sources = sources(project, root, true, &media::Uncontrolled)?;
    for source in sources.values() {
        source.verify(&media::Uncontrolled)?;
    }
    let assets = project
        .assets
        .iter()
        .filter_map(|asset| {
            sources.get(&asset.id).map(|source| {
                let mut asset = asset.clone();
                asset.provenance = Some(Provenance {
                    original: asset.identity.clone().expect("validated"),
                    conversion: Some(
                        source
                            .conversion
                            .as_ref()
                            .expect("prepared conversion")
                            .entry
                            .identity
                            .clone(),
                    ),
                });
                asset
            })
        })
        .collect::<Vec<_>>();
    Ok(
        json!({"cached":true,"prepared":assets,"sources":sources,"project_revision":project.revision}),
    )
}
