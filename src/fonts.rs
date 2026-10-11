//! Read-only installed-font discovery and explicit name-to-content resolution. No font is copied.
use crate::{At, Result, error, scene::Identity};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
const MAX_FILES: usize = 8192;
const MAX_DIRECTORIES: usize = 2048;
const MAX_ENTRIES: usize = 131072;

/// An explicit font file identity, or an installed family/full name resolving to exactly one file.
#[allow(dead_code)] // Schema only: command preparation resolves names before parsing the scene.
#[derive(schemars::JsonSchema)]
#[serde(untagged)]
pub(crate) enum Input {
    /// Bound external font file. Relative files stay inside input_root; absolute files must be installed fonts.
    File(Identity),
    /// Case-insensitive exact family or full name; ambiguous names require an explicit identity from fonts.find.
    Name(String),
}

/// Read-only installed-font listing. Files are never installed, copied or modified.
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Find {
    /// Case-insensitive substring of a family or full name; omit to list all discovered font files.
    #[serde(default)]
    pub family: Option<String>,
    /// Number of matching files to skip, in canonical path order; default zero.
    #[serde(default)]
    pub offset: usize,
    /// Maximum files returned, 1..=128; default 32. Follow next_offset for another page.
    #[serde(default = "default_limit")]
    pub limit: usize,
}
fn default_limit() -> usize {
    32
}

#[derive(Clone, Serialize)]
struct Entry {
    identity: Identity,
    families: Vec<String>,
    full_names: Vec<String>,
    styles: Vec<String>,
    usable: bool,
    reason: Option<String>,
}
struct Catalog {
    roots: Vec<PathBuf>,
    entries: Vec<Entry>,
    skipped: Vec<Value>,
}

/// Only the platform's font folders are searched. Missing folders are normal; unreadable folders
/// fail the scan, since a partial scan cannot establish that a name is unambiguous.
fn roots() -> Result<Vec<PathBuf>> {
    let mut candidates = Vec::new();
    #[cfg(windows)]
    {
        if let Some(windows) = std::env::var_os("WINDIR") {
            candidates.push(PathBuf::from(windows).join("Fonts"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            candidates.push(PathBuf::from(local).join("Microsoft/Windows/Fonts"));
        }
    }
    #[cfg(target_os = "linux")]
    {
        candidates.push(PathBuf::from("/usr/share/fonts"));
        candidates.push(PathBuf::from("/usr/local/share/fonts"));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".local/share/fonts"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        candidates.extend([
            PathBuf::from("/System/Library/Fonts"),
            PathBuf::from("/Library/Fonts"),
        ]);
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join("Library/Fonts"));
        }
    }
    let mut result = BTreeSet::new();
    for folder in candidates {
        if !folder.is_absolute() {
            continue;
        }
        match folder.canonicalize() {
            Ok(path) if path.is_dir() => {
                result.insert(path);
            }
            Ok(_) => {
                return Err(error(
                    "INVALID_PATH",
                    "An OS font folder is not a directory",
                ));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(error(
                    "INVALID_PATH",
                    format!("Font folder {}: {e}", folder.display()),
                ));
            }
        }
    }
    Ok(result.into_iter().collect())
}

fn names(face: &ttf_parser::Face<'_>, ids: &[u16]) -> Vec<String> {
    face.names()
        .into_iter()
        .filter(|n| ids.contains(&n.name_id))
        .filter_map(|n| n.to_string())
        .filter(|s| !s.trim().is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn entry(path: PathBuf, bytes: &[u8]) -> Result<Entry> {
    let face = ttf_parser::Face::parse(bytes, 0)
        .map_err(|e| error("UNSUPPORTED_FONT", format!("Cannot read font names: {e:?}")))?;
    let has = |tag: &[u8; 4]| {
        face.raw_face()
            .table(ttf_parser::Tag::from_bytes(tag))
            .is_some()
    };
    let reason = if bytes.get(..4) != Some(&[0, 1, 0, 0]) || !has(b"glyf") || !has(b"loca") {
        Some("Rendering requires a single TrueType outline font; collections and CFF outlines are unsupported".into())
    } else if [
        b"CFF ", b"CFF2", b"COLR", b"CBDT", b"sbix", b"SVG ", b"morx", b"mort", b"fvar",
    ]
    .iter()
    .any(|tag| has(tag))
    {
        Some("Rendering requires static monochrome TrueType outlines without variable, color or AAT tables".into())
    } else {
        fontdue::Font::from_bytes(
            bytes,
            fontdue::FontSettings {
                load_substitutions: false,
                ..Default::default()
            },
        )
        .err()
        .map(str::to_owned)
    };
    Ok(Entry {
        identity: Identity {
            path,
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        },
        families: names(
            &face,
            &[
                ttf_parser::name_id::FAMILY,
                ttf_parser::name_id::TYPOGRAPHIC_FAMILY,
            ],
        ),
        full_names: names(&face, &[ttf_parser::name_id::FULL_NAME]),
        styles: names(
            &face,
            &[
                ttf_parser::name_id::SUBFAMILY,
                ttf_parser::name_id::TYPOGRAPHIC_SUBFAMILY,
            ],
        ),
        usable: reason.is_none(),
        reason,
    })
}

fn font_extension(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "ttf" | "otf" | "ttc" | "otc"
        )
    })
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    if fs::metadata(path)?.len() > MAX_BYTES {
        return Err(error("LIMIT_EXCEEDED", "Font files must be at most 8 MiB"));
    }
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Font file grew beyond 8 MiB while reading",
        ));
    }
    Ok(bytes)
}

impl Catalog {
    fn scan() -> Result<Self> {
        let roots = roots()?;
        let mut directories = roots.clone();
        let mut visited = BTreeSet::new();
        let mut files = BTreeSet::new();
        let mut skipped = Vec::new();
        let mut examined = 0;
        while let Some(folder) = directories.pop() {
            if !visited.insert(folder.clone()) {
                continue;
            }
            if visited.len() > MAX_DIRECTORIES {
                return Err(error(
                    "LIMIT_EXCEEDED",
                    "Font discovery is limited to 2048 directories",
                ));
            }
            for child in fs::read_dir(&folder)? {
                let child = child?;
                examined += 1;
                if examined > MAX_ENTRIES {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Font discovery examines at most 131072 directory entries",
                    ));
                }
                let kind = child.file_type()?;
                if kind.is_dir() {
                    let directory = child.path().canonicalize()?;
                    if roots.iter().any(|r| directory.starts_with(r)) {
                        directories.push(directory);
                        if visited.len() + directories.len() > MAX_DIRECTORIES {
                            return Err(error(
                                "LIMIT_EXCEEDED",
                                "Font discovery is limited to 2048 directories",
                            ));
                        }
                    }
                } else if font_extension(&child.path()) {
                    let path = child.path().canonicalize()?;
                    if path.is_file() && roots.iter().any(|r| path.starts_with(r)) {
                        files.insert(path);
                        if files.len() > MAX_FILES {
                            return Err(error(
                                "LIMIT_EXCEEDED",
                                "Font discovery is limited to 8192 files",
                            ));
                        }
                    } else {
                        skipped.push(json!({"path":child.path(),"reason":"Font link is outside the OS font folders or is not a file"}));
                    }
                }
            }
        }
        let mut entries = Vec::new();
        let mut total = 0u64;
        for path in files {
            let candidate = read_bounded(&path).and_then(|bytes| {
                total += bytes.len() as u64;
                if total > 512 * 1024 * 1024 {
                    return Err(error(
                        "LIMIT_EXCEEDED",
                        "Font discovery reads at most 512 MiB per request",
                    ));
                }
                entry(path.clone(), &bytes)
            });
            match candidate {
                Ok(e) => entries.push(e),
                Err(e) if total > 512 * 1024 * 1024 => return Err(e),
                Err(e) => skipped.push(json!({"path":path,"reason":e.message})),
            }
        }
        Ok(Self {
            roots,
            entries,
            skipped,
        })
    }

    fn resolve(&self, name: &str) -> Result<Identity> {
        if name.trim().is_empty() || name.len() > 256 {
            return Err(error(
                "UNSUPPORTED_FONT",
                "A font name requires 1-256 nonblank UTF-8 bytes",
            ));
        }
        // All full-name AND family matches count. A Regular face whose full name equals its
        // family's name must not silently hide the family's Bold and Italic faces.
        let key = name.to_lowercase();
        let matches: Vec<_> = self
            .entries
            .iter()
            .filter(|e| {
                e.families
                    .iter()
                    .chain(&e.full_names)
                    .any(|n| n.to_lowercase() == key)
            })
            .collect();
        if let [entry] = matches.as_slice()
            && entry.usable
        {
            return Ok(entry.identity.clone());
        }
        Err(error("UNSUPPORTED_FONT", format!("Installed font name {name:?} has {} matching files; exactly one usable file is required", matches.len()))
            .with_hint("Use fonts.find with a family filter, then pass one usable entry's explicit identity")
            .with_detail(json!({"name":name,"matches":matches.into_iter().take(32).collect::<Vec<_>>(),"skipped_files":self.skipped.len()})))
    }
}

pub fn find(request: &Find) -> Result<Value> {
    if !(1..=128).contains(&request.limit) || request.family.as_ref().is_some_and(|s| s.len() > 256)
    {
        return Err(error(
            "INVALID_PARAMETER",
            "fonts.find requires limit 1..=128 and a family filter of at most 256 bytes",
        ));
    }
    let catalog = Catalog::scan()?;
    let key = request.family.as_deref().unwrap_or_default().to_lowercase();
    let matches: Vec<_> = catalog
        .entries
        .iter()
        .filter(|e| {
            key.is_empty()
                || e.families
                    .iter()
                    .chain(&e.full_names)
                    .any(|n| n.to_lowercase().contains(&key))
        })
        .collect();
    let end = request
        .offset
        .saturating_add(request.limit)
        .min(matches.len());
    Ok(
        json!({"roots":catalog.roots,"fonts":matches.iter().skip(request.offset).take(request.limit).collect::<Vec<_>>(),
        "matched":matches.len(),"next_offset":(end < matches.len()).then_some(end),
        "scanned":catalog.entries.len()+catalog.skipped.len(),"skipped_count":catalog.skipped.len(),
        "skipped":catalog.skipped.iter().take(32).collect::<Vec<_>>(),
        "policy":"Fonts are read in place. Output contains rasterized glyphs; font licensing remains the user's responsibility."}),
    )
}

/// Relative fonts retain the scene input-root rule. Absolute fonts must resolve inside an OS font
/// folder on every read, including the final publication check; arbitrary external paths fail.
pub(crate) fn identity_bytes(identity: &Identity, root: &Path) -> Result<(PathBuf, Vec<u8>)> {
    if !root.is_absolute() {
        return Err(error("INVALID_PATH", "Scene input_root must be absolute"));
    }
    if !identity.path.is_absolute() {
        return crate::scene::identity_bytes(identity, root);
    }
    let path = identity.path.canonicalize().map_err(|e| {
        error(
            "INVALID_PATH",
            format!("Installed font {}: {e}", identity.path.display()),
        )
    })?;
    let folder = roots()?
        .into_iter()
        .find(|r| path.starts_with(r))
        .ok_or_else(|| {
            error(
                "INVALID_PATH",
                "Absolute font paths must lie inside an OS font folder",
            )
        })?;
    if !font_extension(&path) {
        return Err(error(
            "UNSUPPORTED_FONT",
            "Installed font paths require a .ttf, .otf, .ttc or .otc extension",
        ));
    }
    let relative = Identity {
        path: path.strip_prefix(&folder).expect("checked prefix").into(),
        ..identity.clone()
    };
    crate::scene::identity_bytes(&relative, &folder)
}

/// Resolve only known font slots, before parsing or saving a queued command. Other strings and
/// identity fields (images, audio, user metadata) are untouched. Saved jobs contain pinned files.
pub(crate) fn prepare(request: &mut Value) -> Result<Vec<Value>> {
    let mut resolver = Resolver {
        catalog: None,
        completed: Vec::new(),
    };
    match request["command"].as_str().unwrap_or_default() {
        "scene.inspect" | "scene.still" | "scene.render" => {
            resolver.scene(&mut request["scene"], "scene")?
        }
        "captions.scene" | "captions.render" => {
            if let Some(scene) = request.get_mut("scene") {
                resolver.scene(scene, "scene")?;
            }
            if let Some(layouts) = request["layouts"].as_object_mut() {
                for (key, layout) in layouts {
                    if let Some(fonts) = layout.get_mut("fonts") {
                        resolver.fonts(fonts, &format!("layouts.{key}.fonts"))?;
                    }
                }
            }
        }
        "graphics.instantiate" => {
            if let Some(template) = request.get_mut("template") {
                if let Some(scene) = template.get_mut("scene") {
                    resolver.scene(scene, "template.scene")?;
                }
                if let Some(parameters) = template["parameters"].as_array_mut() {
                    for (i, parameter) in parameters.iter_mut().enumerate() {
                        if parameter["definition"]["type"] == "fonts"
                            && let Some(fonts) = parameter["definition"].get_mut("default")
                        {
                            resolver.fonts(
                                fonts,
                                &format!("template.parameters[{i}].definition.default"),
                            )?;
                        }
                    }
                }
            }
            if let Some(values) = request.get_mut("values").and_then(Value::as_object_mut) {
                for (key, value) in values {
                    if value["type"] == "fonts"
                        && let Some(fonts) = value.get_mut("value")
                    {
                        resolver.fonts(fonts, &format!("values.{key}.value"))?;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(resolver.completed)
}
struct Resolver {
    catalog: Option<Catalog>,
    completed: Vec<Value>,
}
impl Resolver {
    fn scene(&mut self, scene: &mut Value, path: &str) -> Result<()> {
        if let Some(layers) = scene["layers"].as_array_mut() {
            for (i, layer) in layers.iter_mut().enumerate() {
                if layer["graphics"]["kind"] == "text"
                    && let Some(fonts) = layer["graphics"].get_mut("fonts")
                {
                    self.fonts(fonts, &format!("{path}.layers[{i}].graphics.fonts"))?;
                }
            }
        }
        Ok(())
    }
    fn fonts(&mut self, fonts: &mut Value, path: &str) -> Result<()> {
        if let Some(items) = fonts.as_array_mut() {
            for (i, font) in items.iter_mut().enumerate() {
                self.font(font).at(|| format!("{path}[{i}]"))?;
            }
        }
        Ok(())
    }
    fn font(&mut self, font: &mut Value) -> Result<()> {
        if let Some(name) = font.as_str() {
            if self.catalog.is_none() {
                self.catalog = Some(Catalog::scan()?);
            }
            let identity = self.catalog.as_ref().unwrap().resolve(name)?;
            let mut report = serde_json::to_value(&identity)?;
            report["font_name"] = json!(name);
            self.completed.push(report);
            *font = serde_json::to_value(identity)?;
        } else if let Some(path) = font["path"].as_str().map(PathBuf::from)
            && path.is_absolute()
            && (font.get("bytes").is_none() || font.get("sha256").is_none())
        {
            // Leave non-installed and missing paths to the ordinary identity completion pass,
            // preserving its existing input-root checks and error codes.
            let Ok(canonical) = path.canonicalize() else {
                return Ok(());
            };
            if !roots()?.iter().any(|r| canonical.starts_with(r)) {
                return Ok(());
            }
            let bytes = read_bounded(&canonical)?;
            let identity = json!({"path":path,"bytes":bytes.len(),"sha256":format!("{:x}", Sha256::digest(&bytes))});
            for key in ["bytes", "sha256"] {
                if font.get(key).is_none() {
                    font[key] = identity[key].clone();
                }
            }
            self.completed.push(font.clone());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn font(path: &str, family: &str, full: &str, usable: bool) -> Entry {
        Entry {
            identity: Identity {
                path: path.into(),
                bytes: 1,
                sha256: "0".repeat(64),
            },
            families: vec![family.into()],
            full_names: vec![full.into()],
            styles: vec![],
            usable,
            reason: None,
        }
    }
    #[test]
    fn family_ambiguity_is_not_hidden_by_regular_full_name() {
        let catalog = Catalog {
            roots: vec![],
            skipped: vec![],
            entries: vec![
                font("regular.ttf", "Example", "Example", true),
                font("bold.ttf", "Example", "Example Bold", true),
                font("single.ttf", "Single", "Single Regular", true),
                font("variable.ttf", "Variable", "Variable", false),
            ],
        };
        assert_eq!(
            catalog.resolve("EXAMPLE").unwrap_err().code,
            "UNSUPPORTED_FONT"
        );
        assert_eq!(
            catalog.resolve("Example Bold").unwrap().path,
            Path::new("bold.ttf")
        );
        assert_eq!(
            catalog.resolve("single").unwrap().path,
            Path::new("single.ttf")
        );
        assert!(catalog.resolve("Variable").is_err());
        assert!(catalog.resolve("missing").is_err());
    }
    #[test]
    fn prepare_visits_only_documented_font_slots() {
        let mut value =
            json!({"command":"session.create","project":{"metadata":{"fonts":["no such font"]}}});
        let original = value.clone();
        assert!(prepare(&mut value).unwrap().is_empty());
        assert_eq!(value, original);
        let mut value = json!({"command":"scene.inspect","scene":{"layers":[{"graphics":{"kind":"shape","fonts":["no such font"]}}]}});
        assert!(prepare(&mut value).unwrap().is_empty());
    }
}
