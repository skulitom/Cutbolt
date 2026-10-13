//! Cached explicit normalization, with leases for subsequent timeline preparation integration.
use crate::{
    Result,
    cache::{self, Policy},
    cache_store::{Temporary, digest, key_ok},
    conform,
    conform_store::{self, Entry, Store},
    error, media,
    registry::Identity,
    render, scene,
    time::Time,
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

struct Dependency {
    path: PathBuf,
    identity: Identity,
}
fn dependency(path: PathBuf) -> Result<Dependency> {
    Ok(Dependency {
        identity: Identity {
            bytes: fs::metadata(&path)?.len(),
            sha256: media::file_hash(&path)?,
        },
        path,
    })
}
fn verify_dependencies(values: &[Dependency]) -> Result<()> {
    for d in values {
        if fs::metadata(&d.path)?.len() != d.identity.bytes
            || media::file_hash(&d.path)? != d.identity.sha256
        {
            return Err(error(
                "MEDIA_CHANGED",
                "A source, LUT or decoder changed before cached conversion publication",
            ));
        }
    }
    Ok(())
}
fn inputs(
    recipe: &conform::Recipe,
    root: &Path,
    cache_root: &Path,
) -> Result<(String, Vec<Dependency>)> {
    if recipe.id.trim().is_empty() || recipe.id.len() > 128 {
        return Err(error(
            "INVALID_CONFORM",
            "Conform asset ID must contain 1..128 nonblank bytes",
        ));
    }
    let source = scene::identity_file(&recipe.source.file, root, 16 * 1024 * 1024 * 1024)?;
    let mut dependencies = vec![dependency(source)?];
    if let Some(lut) = &recipe.lut {
        dependencies.push(dependency(scene::identity_file(
            &lut.file,
            root,
            16 * 1024 * 1024 * 1024,
        )?)?);
    }
    for name in ["ffmpeg", "ffprobe"] {
        dependencies.push(dependency(cache::resolve_tool(&media::tool(name))?)?);
    }
    if dependencies.iter().any(|d| d.path.starts_with(cache_root)) {
        return Err(error(
            "INVALID_CACHE",
            "Inputs and decoder executables must be outside the conform cache root",
        ));
    }
    let mut normalized = serde_json::to_value(recipe)?;
    normalized.as_object_mut().expect("recipe").remove("id");
    normalized["source"]["file"]
        .as_object_mut()
        .expect("source identity")
        .remove("path");
    if let Some(file) = normalized
        .get_mut("lut")
        .and_then(|l| l.get_mut("file"))
        .and_then(Value::as_object_mut)
    {
        file.remove("path");
    }
    let suffix = |path: &Path| {
        path.extension()
            .map(|s| s.to_string_lossy().to_ascii_lowercase())
    };
    let key = digest(&serde_json::to_vec(
        &json!({"schema":1,"recipe":normalized,"producer":cache::producer(),
        "source_extension":suffix(&recipe.source.file.path),"lut_extension":recipe.lut.as_ref().map(|l|suffix(&l.file.path)),
        "dependencies":dependencies.iter().map(|d|&d.identity).collect::<Vec<_>>()}),
    )?);
    Ok((key, dependencies))
}
fn decoded_metadata(recipe: &conform::Recipe, receipt: &Value) -> Result<Value> {
    if recipe.picture()?.is_some() {
        return Ok(receipt["decoded"].clone());
    }
    Ok(
        json!({"pixel_format":null,"video_bytes":0,"video_sha256":null,
        "pcm_bytes":recipe.duration.units(Time::new(48000,1)?)?*4,"pcm_sha256":receipt["pcm_sha256"]}),
    )
}
fn verify_decoded(entry: &Entry, recipe: &conform::Recipe) -> Result<()> {
    let invalid = || {
        error(
            "CACHE_CORRUPT",
            "Cached conversion's decoded content differs from its recorded recipe and digests",
        )
    };
    let decoded = &entry.metadata["decoded"];
    let samples = recipe.duration.units(Time::new(48000, 1)?)?;
    if decoded["pcm_bytes"] != samples * 4 || !decoded["pcm_sha256"].as_str().is_some_and(key_ok) {
        return Err(invalid());
    }
    let timeout = Duration::from_secs((180 + 2 * samples / 48000).min(4 * 3600));
    if let Some((width, height)) = recipe.picture()? {
        let channels = if recipe.source.alpha.is_some() { 4 } else { 3 };
        let format = if channels == 4 { "rgba" } else { "rgb24" };
        let frames = recipe
            .duration
            .units(recipe.frame_rate.unwrap_or(Time::new(25, 1)?))?;
        let bytes = (width as u64)
            .checked_mul(height as u64)
            .and_then(|v| v.checked_mul(channels))
            .and_then(|v| v.checked_mul(frames))
            .ok_or_else(invalid)?;
        if decoded["video_bytes"] != bytes
            || decoded["pixel_format"] != format
            || !decoded["video_sha256"].as_str().is_some_and(key_ok)
            || conform::decoded_hash(&entry.path, Some(format), bytes, timeout)?
                != decoded["video_sha256"].as_str().ok_or_else(invalid)?
        {
            return Err(invalid());
        }
    } else if decoded["video_bytes"] != 0 || !decoded["pixel_format"].is_null() {
        return Err(invalid());
    }
    if conform::decoded_hash(&entry.path, None, samples * 4, timeout)?
        != decoded["pcm_sha256"].as_str().ok_or_else(invalid)?
    {
        return Err(invalid());
    }
    Ok(())
}
struct Lease {
    _store: Store,
    entry: Entry,
    key: String,
    hit: bool,
    dependencies: Vec<Dependency>,
}
fn acquire(
    root: &Path,
    policy: &Policy,
    recipe: &conform::Recipe,
    input_root: &Path,
) -> Result<Lease> {
    conform_store::validate(policy)?;
    if policy.max_bytes == 0 || policy.max_entries == 0 {
        return Err(
            error("LIMIT_EXCEEDED", "Conform caching needs a positive budget").with_path("policy"),
        );
    }
    let root = media::input_root(root)?;
    let (key, dependencies) = inputs(recipe, input_root, &root)?;
    let extension = if recipe.picture()?.is_some() {
        "mkv"
    } else {
        "wav"
    };
    let mut produced = false;
    for _ in 0..4 {
        match Store::open(&root, false, false) {
            Ok(mut store) => {
                let summary = store.inspect()?;
                let fits = summary["payload_bytes"]
                    .as_u64()
                    .is_some_and(|n| n <= policy.max_bytes)
                    && summary["entries"]
                        .as_array()
                        .is_some_and(|e| e.len() <= policy.max_entries as usize);
                if fits {
                    match store.read(&key).and_then(|entry| {
                        entry
                            .map(|entry| verify_decoded(&entry, recipe).map(|_| entry))
                            .transpose()
                    }) {
                        Ok(Some(entry)) => {
                            verify_dependencies(&dependencies)?;
                            return Ok(Lease {
                                _store: store,
                                entry,
                                key,
                                hit: !produced,
                                dependencies,
                            });
                        }
                        Ok(None) => {}
                        Err(e) if e.code == "CACHE_CORRUPT" => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Err(e) if e.code == "CACHE_MISSING" => {}
            Err(e) => return Err(e),
        }
        let mut store = Store::open(&root, true, true)?;
        store.prune(policy)?;
        match store.read(&key).and_then(|entry| {
            entry
                .map(|entry| verify_decoded(&entry, recipe).map(|_| entry))
                .transpose()
        }) {
            Ok(Some(_)) => continue, // Release writer lease, then acquire a reader lease and recheck.
            Ok(None) => {}
            Err(e) if e.code == "CACHE_CORRUPT" => store.discard(&key)?,
            Err(e) => return Err(e),
        }
        let output = store.begin(&key, extension, policy)?;
        let shared = Arc::new(Mutex::new(store));
        let reserved = shared.clone();
        let budget = policy.clone();
        let reserved_key = key.clone();
        let quota: conform::Quota = Arc::new(move |bytes| {
            reserved
                .lock()
                .expect("conform store")
                .reserve(&reserved_key, bytes, &budget)
        });
        let result = (|| -> Result<()> {
            let receipt = conform::run_with_quota(
                recipe,
                input_root,
                output.parent().expect("staging directory"),
                &output,
                Some(policy.max_bytes),
                Some(quota),
            )?;
            let identity = Identity {
                bytes: fs::metadata(&output)?.len(),
                sha256: media::file_hash(&output)?,
            };
            let metadata = json!({"schema":1,"decoded":decoded_metadata(recipe,&receipt)?,"geometry":receipt.get("geometry"),
                "duration":recipe.duration,"working_transfer":recipe.working_transfer,"ffmpeg":receipt["ffmpeg"],"ffprobe":receipt["ffprobe"]});
            verify_dependencies(&dependencies)?;
            shared
                .lock()
                .expect("conform store")
                .finish(&key, extension, &output, &identity, &metadata)?;
            Ok(())
        })();
        if let Err(e) = result {
            shared.lock().expect("conform store").discard(&key)?;
            return Err(
                if e.code == "LIMIT_EXCEEDED" && e.path.as_deref() == Some("max_output_bytes") {
                    e.with_path("policy.max_bytes").with_hint("Raise policy.max_bytes or shorten task.recipe.duration; no delivery output was published.")
                } else {
                    e
                },
            );
        }
        produced = true;
        // Writer lock is released before the next read, preventing a returned exclusive lease.
    }
    Err(error(
        "STORE_BUSY",
        "Conform entry repeatedly evicted between preparation and reader lease",
    ))
}

pub(crate) fn run(
    root: &Path,
    policy: &Policy,
    recipe: &conform::Recipe,
    input_root: &Path,
    output_root: &Path,
    output: &Path,
) -> Result<Value> {
    let extension = if recipe.picture()?.is_some() {
        "mkv"
    } else {
        "wav"
    };
    let destination = render::destination_extension(output, output_root, extension)?;
    let cache_root = media::input_root(root)?;
    if destination.starts_with(&cache_root) {
        return Err(error(
            "INVALID_CACHE",
            "Delivery output must be outside the conform cache root",
        ));
    }
    let lease = acquire(&cache_root, policy, recipe, input_root)?;
    let temp = Temporary::new(destination.parent().expect("destination parent"), extension)?;
    fs::copy(&lease.entry.path, &temp.0)?;
    if fs::metadata(&temp.0)?.len() != lease.entry.identity.bytes
        || media::file_hash(&temp.0)? != lease.entry.identity.sha256
    {
        return Err(error(
            "CACHE_CORRUPT",
            "Cached conversion changed during delivery copy",
        ));
    }
    verify_dependencies(&lease.dependencies)?;
    media::publish(&temp.0, &destination)?;
    let summary = lease._store.inspect()?;
    Ok(
        json!({"result":{"output":destination,"identity":lease.entry.identity,"details":lease.entry.metadata,
        "asset":{"id":recipe.id,"path":destination,"duration":recipe.duration,"identity":lease.entry.identity}},
        "cache":{"kind":"conform","key":lease.key,"hit":lease.hit,"producer":cache::producer(),"policy":policy,
            "payload_bytes":summary["payload_bytes"],"decoded_verified":true,"raw_decode_scratch":"separate_existing_limits"}}),
    )
}
