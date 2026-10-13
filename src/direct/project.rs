//! Saved source bindings and their exact single-frame preview entry point.
use super::Video;
use crate::{
    Result, error,
    media::{self, Control},
    model::{Asset, Decode, Project},
    registry::Identity,
    render::Source,
    source_index,
    time::Time,
};
use serde_json::{Value, json};
use std::path::Path;

fn matches_project(video: &Video, project: &Project, asset: &Asset) -> Result<()> {
    if (video.width, video.height) != (project.width, project.height) {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            format!(
                "Asset {:?} has size {}x{}; direct sources must match project size {}x{}",
                asset.id, video.width, video.height, project.width, project.height
            ),
        )
        .with_path("asset.decode"));
    }
    if video.rate != project.frame_rate {
        return Err(error(
            "UNSUPPORTED_MEDIA",
            format!(
                "Asset {:?} has frame rate {}; direct sources must match project rate {}",
                asset.id, video.rate, project.frame_rate
            ),
        )
        .with_path("asset.decode"));
    }
    let actual = Time::new(video.frames(), 1)?.times(Time::new(video.rate.den, video.rate.num)?)?;
    if asset.duration != actual {
        return Err(error(
            "INVALID_MEDIA",
            format!(
                "Asset {:?} declares duration {} s; its direct index contains {} frames ({} s)",
                asset.id,
                asset.duration,
                video.frames(),
                actual
            ),
        )
        .with_path("asset.duration")
        .with_detail(json!({"actual_duration":actual,"frames":video.frames()})));
    }
    Ok(())
}

fn stale(asset: &Asset, expected: &str, current: &str) -> crate::Error {
    error("DIRECT_INDEX_STALE", format!("Asset {:?} direct index is stale for the current source, tools or engine", asset.id))
        .with_path("asset.decode.index")
        .with_hint("Apply media.index with this asset_id and input_root to verify the original and refresh its index binding.")
        .with_detail(json!({"asset_id":asset.id,"saved_index":expected,"current_index":current}))
}

/// A reused preliminary identity is provisional until the caller's full publication check.
fn loaded(
    asset: &Asset,
    root: &Path,
    control: &dyn Control,
    reuse_identity: bool,
) -> Result<(std::path::PathBuf, source_index::Loaded)> {
    let path = media::project_file(Path::new(&asset.path), root)?;
    let identity = asset.identity.as_ref().ok_or_else(|| {
        error(
            "INVALID_IDENTITY",
            "Direct assets require a bound original identity",
        )
    })?;
    identity.validate()?;
    control.check()?;
    let bytes = path.metadata()?.len();
    if bytes != identity.bytes {
        return Err(crate::registry::changed_size(
            &asset.path,
            identity.bytes,
            bytes,
            crate::registry::RELINK_ASSET,
        ));
    }
    let hash = if reuse_identity {
        media::file_hash_reused_controlled(&path, control)?
    } else {
        media::file_hash_controlled(&path, control)?
    };
    if hash != identity.sha256 {
        return Err(crate::registry::changed_digest(
            &asset.path,
            &identity.sha256,
            &hash,
            crate::registry::RELINK_ASSET,
        ));
    }
    let metadata = media::probe_controlled(&path, control)?;
    let index = source_index::load(&path, identity.clone(), &metadata, control)?;
    Ok((path, index))
}

pub(crate) fn bind(
    project: &Project,
    mut asset: Asset,
    root: &Path,
    refresh: bool,
) -> Result<Asset> {
    let root = media::input_root(root)?;
    let path = media::project_file(Path::new(&asset.path), &root)?;
    if asset.identity.is_none() {
        asset.identity = Some(Identity {
            bytes: path.metadata()?.len(),
            sha256: media::file_hash(&path)?,
        });
    }
    let (path, index) = loaded(&asset, &root, &media::Uncontrolled, false)?;
    if !refresh
        && let Some(Decode::Direct { index: saved }) = &asset.decode
        && saved != &index.key
    {
        return Err(stale(&asset, saved, &index.key));
    }
    let video = Video::from_index(path, index)?;
    matches_project(&video, project, &asset)?;
    video.verify(&media::Uncontrolled, true)?;
    asset.decode = Some(Decode::Direct {
        index: video.loaded.key,
    });
    Ok(asset)
}

/// Check the saved binding on every use, even when the derived cache was just rebuilt.
pub(crate) fn bound_video(
    project: &Project,
    asset: &Asset,
    root: &Path,
    control: &dyn Control,
    reuse_identity: bool,
) -> Result<(Video, Source)> {
    let Some(Decode::Direct { index: saved }) = &asset.decode else {
        return Err(error(
            "INVALID_MEDIA",
            "An indexed direct binding is required",
        ));
    };
    let (path, loaded) = loaded(asset, root, control, reuse_identity)?;
    if saved != &loaded.key {
        return Err(stale(asset, saved, &loaded.key));
    }
    let video = Video::from_index(path, loaded)?;
    matches_project(&video, project, asset)?;
    let source = Source {
        conversion: None,
        path: video.path.clone(),
        sha256: video.loaded.index.source.sha256.clone(),
        frames: video.frames(),
        samples: video.audio_samples(),
    };
    Ok((video, source))
}

/// Identity has been checked by the caller. Do not decode or rebuild during static checking.
pub(crate) fn index_status(asset: &Asset) -> Result<Option<Value>> {
    let Some(Decode::Direct { index }) = &asset.decode else {
        return Ok(None);
    };
    let source = asset.identity.as_ref().expect("validated direct identity");
    let current = source_index::key(source, &media::Uncontrolled)?;
    let status = if *index != current {
        "stale"
    } else if source_index::cached(index, source) {
        "ready"
    } else {
        "missing"
    };
    Ok(Some(
        json!({"status":status,"saved_index":index,"current_index":current}),
    ))
}
