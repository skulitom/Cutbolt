//! Project-owned metadata and explicit, content-checked local relinking proposals.
use crate::{
    Result, error, media,
    model::{Asset, Operation, Project},
    render,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Path, PathBuf},
};

/// Project-owned asset metadata for organizing and registry.search; at most 4096 UTF-8 bytes in total, no NUL characters. Media files are never tagged.
#[derive(schemars::JsonSchema, Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Metadata {
    /// Display title, at most 1024 bytes; default empty.
    #[serde(default)]
    pub title: String,
    /// Bin path from outermost component, at most 8 nonblank components of up to 128 bytes; default empty.
    #[serde(default)]
    pub bin: Vec<String>,
    /// At most 32 unique case-sensitive tags, each nonblank and up to 128 bytes; default empty.
    #[serde(default)]
    pub tags: Vec<String>,
    /// At most 32 custom name/value pairs; names nonblank and up to 128 bytes, values up to 1024 bytes.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
}
impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.title.is_empty()
            && self.bin.is_empty()
            && self.tags.is_empty()
            && self.fields.is_empty()
    }
    pub(crate) fn validate(&self) -> Result<()> {
        let text = |s: &String| !s.trim().is_empty() && s.len() <= 128 && !s.contains('\0');
        let size = self.title.len()
            + self.bin.iter().map(String::len).sum::<usize>()
            + self.tags.iter().map(String::len).sum::<usize>()
            + self
                .fields
                .iter()
                .map(|(k, v)| k.len() + v.len())
                .sum::<usize>();
        if size > 4096
            || self.title.len() > 1024
            || self.title.contains('\0')
            || self.bin.len() > 8
            || self.tags.len() > 32
            || self.fields.len() > 32
            || !self.bin.iter().all(text)
            || !self.tags.iter().all(text)
            || self.tags.iter().collect::<BTreeSet<_>>().len() != self.tags.len()
            || self
                .fields
                .iter()
                .any(|(k, v)| !text(k) || v.len() > 1024 || v.contains('\0'))
        {
            return Err(error(
                "INVALID_METADATA",
                "Metadata requires <=4096 UTF-8 bytes, bounded nonblank bin/tag/field names and unique tags",
            ));
        }
        Ok(())
    }
}

/// Content identity of a media file, checked before the file is used.
#[derive(
    schemars::JsonSchema, Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord,
)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    /// SHA-256 of the file content as 64 lowercase hex characters.
    pub sha256: String,
    /// File size in bytes; positive.
    pub bytes: u64,
}
impl Identity {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.bytes == 0
            || self.bytes > 9_007_199_254_740_991
            || self.sha256.len() != 64
            || !self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(error(
                "INVALID_IDENTITY",
                "Identity requires lowercase SHA-256 and a positive safe byte count",
            ));
        }
        Ok(())
    }
}

/// Asset filter for registry.search; every supplied filter must match, and results sort by asset ID.
#[derive(schemars::JsonSchema, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Case-insensitive substring of ID, path, title, bin, tag or field name/value; at most 1024 bytes. Empty matches all.
    #[serde(default)]
    pub text: String,
    /// Bin path prefix, at most 8 components; matches that bin and its descendants.
    #[serde(default)]
    pub bin: Vec<String>,
    /// Tags every result must have (exact match), at most 32.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Number of results to skip, 0-1000; default 0. Use the returned `next_offset` to page.
    #[serde(default)]
    pub offset: usize,
    /// Maximum results to return, 1-200; default 50.
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    50
}

pub fn search(project: &Project, query: &Query) -> Result<Value> {
    project.validate()?;
    if query.text.len() > 1024
        || query.bin.len() > 8
        || query.tags.len() > 32
        || query
            .bin
            .iter()
            .chain(&query.tags)
            .any(|s| s.trim().is_empty() || s.len() > 128)
        || query.offset > 1000
        || !(1..=200).contains(&query.limit)
    {
        return Err(error(
            "INVALID_QUERY",
            "Query exceeds text/bin/tag or pagination limits",
        ));
    }
    let needle = query.text.to_lowercase();
    let mut found: Vec<_> = project
        .assets
        .iter()
        .filter(|a| {
            a.metadata.bin.starts_with(&query.bin)
                && query.tags.iter().all(|tag| a.metadata.tags.contains(tag))
                && (needle.is_empty()
                    || std::iter::once(&a.id)
                        .chain(std::iter::once(&a.path))
                        .chain(std::iter::once(&a.metadata.title))
                        .chain(a.metadata.bin.iter())
                        .chain(a.metadata.tags.iter())
                        .chain(a.metadata.fields.keys())
                        .chain(a.metadata.fields.values())
                        .any(|s| s.to_lowercase().contains(&needle)))
        })
        .collect();
    found.sort_by(|a, b| a.id.cmp(&b.id));
    let total = found.len();
    let results: Vec<_> = found
        .into_iter()
        .skip(query.offset)
        .take(query.limit)
        .collect();
    let mut duplicates: BTreeMap<&Identity, Vec<&str>> = BTreeMap::new();
    for a in &project.assets {
        if let Some(identity) = &a.identity {
            duplicates.entry(identity).or_default().push(&a.id);
        }
    }
    let duplicates: Vec<_> = duplicates
        .into_iter()
        .filter_map(|(identity, mut ids)| {
            ids.sort();
            (ids.len() > 1).then(|| json!({"identity":identity,"asset_ids":ids}))
        })
        .collect();
    Ok(
        json!({"total":total,"assets":results,"next_offset":(query.offset + results.len() < total).then_some(query.offset+results.len()),"duplicates":duplicates}),
    )
}

fn root(root: &Path) -> Result<()> {
    if !root.is_absolute() || !root.is_dir() {
        return Err(error(
            "INVALID_PATH",
            "Input root must be an existing absolute directory",
        ));
    }
    Ok(())
}
fn fingerprint(path: &Path) -> Result<Identity> {
    let before = path.metadata()?.len();
    let sha256 = media::file_hash(path)?;
    if before != path.metadata()?.len() {
        return Err(error("MEDIA_CHANGED", "Source size changed while hashing"));
    }
    let identity = Identity {
        sha256,
        bytes: before,
    };
    identity.validate()?;
    Ok(identity)
}

pub fn bind(project: &Project, revision: u64, input_root: &Path, ids: &[String]) -> Result<Value> {
    project.validate()?;
    root(input_root)?;
    if ids.is_empty() || ids.len() > 1000 || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
    {
        return Err(error("INVALID_SELECTION", "Select 1-1000 unique asset IDs"));
    }
    let mut hashes = HashMap::new();
    let mut operations = Vec::new();
    for id in ids {
        let asset = project
            .assets
            .iter()
            .find(|a| &a.id == id)
            .ok_or_else(|| error("MISSING_MEDIA", id))?;
        let path = media::project_file(Path::new(&asset.path), input_root)?;
        if !hashes.contains_key(&path) {
            hashes.insert(path.clone(), fingerprint(&path)?);
        }
        operations.push(Operation::Bind {
            asset_id: id.clone(),
            identity: hashes[&path].clone(),
        });
    }
    let updated = project.apply(revision, operations.clone())?;
    Ok(json!({"project":updated,"operations":operations}))
}

pub fn status(project: &Project, input_root: &Path) -> Result<Value> {
    project.validate()?;
    root(input_root)?;
    let mut entries = Vec::new();
    let mut hashes = HashMap::new();
    for asset in &project.assets {
        let result = (|| {
            let path = media::project_file(Path::new(&asset.path), input_root)?;
            if !hashes.contains_key(&path) {
                hashes.insert(path.clone(), fingerprint(&path)?);
            }
            Ok::<_, crate::Error>(hashes[&path].clone())
        })();
        entries.push(match result {
            Ok(actual) => json!({"asset_id":asset.id,"state":if let Some(expected)=&asset.identity { if expected==&actual {"online"} else {"changed"} } else {"unbound"},"actual_identity":actual}),
            Err(e) => json!({"asset_id":asset.id,"state":if e.code=="IO_ERROR" && !input_root.join(&asset.path).try_exists().unwrap_or(true) {"missing"} else {"unavailable"},"error":e}),
        });
    }
    Ok(json!({"assets":entries}))
}

pub fn relink(
    project: &Project,
    revision: u64,
    input_root: &Path,
    id: &str,
    candidates: &[PathBuf],
) -> Result<Value> {
    project.validate()?;
    root(input_root)?;
    let asset = project
        .assets
        .iter()
        .find(|a| a.id == id)
        .ok_or_else(|| error("MISSING_MEDIA", id))?;
    let expected = asset.identity.as_ref().ok_or_else(|| {
        error(
            "IDENTITY_REQUIRED",
            "Bind source identity before media goes offline",
        )
    })?;
    let path = match_identity(input_root, expected, candidates)?;
    let operations = vec![Operation::Relink {
        asset_id: id.into(),
        path: path.to_string_lossy().into_owned(),
    }];
    let updated = project.apply(revision, operations.clone())?;
    Ok(json!({"project":updated,"operations":operations,"identity":expected}))
}

pub(crate) fn match_identity(
    input_root: &Path,
    expected: &Identity,
    candidates: &[PathBuf],
) -> Result<PathBuf> {
    root(input_root)?;
    expected.validate()?;
    if candidates.is_empty() || candidates.len() > 1000 {
        return Err(error(
            "INVALID_SELECTION",
            "Provide 1-1000 explicit candidate paths",
        ));
    }
    let mut paths = BTreeSet::new();
    let mut matches = Vec::new();
    for candidate in candidates {
        let path = media::allowed_file(candidate, input_root)?;
        if paths.insert(path.clone()) && &fingerprint(&path)? == expected {
            matches.push(path);
        }
    }
    if matches.len() > 1 {
        return Err(error(
            "AMBIGUOUS_MEDIA",
            "Several candidates have the expected content; select one explicitly",
        ));
    }
    matches.pop().ok_or_else(|| {
        error(
            "IDENTITY_MISMATCH",
            "No candidate matches the bound source identity",
        )
    })
}

pub(crate) fn verify_source(asset: &Asset, source: &render::Source) -> Result<()> {
    if let Some(identity) = &asset.identity
        && (identity.sha256 != source.sha256 || identity.bytes != source.path.metadata()?.len())
    {
        return Err(error(
            "IDENTITY_MISMATCH",
            format!("Asset {} differs from its bound source", asset.id),
        ));
    }
    Ok(())
}
