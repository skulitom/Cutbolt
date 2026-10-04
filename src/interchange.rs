//! Bounded original editorial mapping against the public OTIO data contract.
use crate::{
    Result, error, media,
    model::{Asset, Project},
    render, scene,
    time::Time,
    tracks,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
mod export;
mod import;

const LIMIT: u64 = 4 * 1024 * 1024;
const MAX_INTEGER: f64 = 9_007_199_254_740_991.0;

/// Maps one media reference in the interchange document to an identity-bound local asset.
#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// An exact logical reference from the interchange document; never fetched.
    pub target_url: String,
    /// Explicit local asset, including a bound content identity.
    pub asset: Asset,
}
/// Request for interchange.import: map a bounded OTIO document to a proposed native project.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Import {
    /// `.otio` file identity `{path, bytes, sha256}` relative to `input_root`; at most 4 MiB.
    pub source: scene::Identity,
    /// Existing absolute directory containing the source document.
    pub input_root: PathBuf,
    /// Existing absolute directory that every bound asset and proxy path must resolve inside.
    pub media_root: PathBuf,
    /// ID of the proposed project.
    pub id: String,
    /// Project canvas width in pixels.
    pub width: u32,
    /// Project canvas height in pixels.
    pub height: u32,
    /// Timeline frame rate in frames per second as a rational `{num, den}`.
    pub frame_rate: Time,
    /// Up to 1000 media bindings, one per distinct document reference; unused ones are ignored.
    pub bindings: Vec<Binding>,
    /// Unique IDs of reported nonblocking losses to accept; default none.
    #[serde(default)]
    pub acknowledged_losses: Vec<String>,
}
/// Request for interchange.export: write a project as a new OTIO file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Export {
    /// Project snapshot to export.
    pub project: Project,
    /// Absolute directory containing every registered asset; each asset identity is checked.
    pub input_root: PathBuf,
    /// Existing absolute directory that must contain `output`.
    pub output_root: PathBuf,
    /// Absolute path of the new `.otio` file; existing files are never replaced.
    pub output: PathBuf,
    /// Unique IDs of reported nonblocking losses to accept; default none.
    #[serde(default)]
    pub acknowledged_losses: Vec<String>,
}

#[derive(Debug, Serialize)]
struct Loss {
    id: String,
    path: String,
    code: &'static str,
    message: String,
    blocking: bool,
}
#[derive(Default)]
struct Analysis {
    losses: Vec<Loss>,
    names: Vec<Value>,
}
impl Analysis {
    fn loss(&mut self, path: &str, code: &'static str, message: impl Into<String>, blocking: bool) {
        let id = format!("{path}:{code}");
        if !self.losses.iter().any(|v| v.id == id) {
            self.losses.push(Loss {
                id,
                path: path.into(),
                code,
                message: message.into(),
                blocking,
            });
        }
    }
    fn permitted(&self, acknowledged: &[String]) -> Result<bool> {
        let unique: BTreeSet<_> = acknowledged.iter().collect();
        if unique.len() != acknowledged.len() || acknowledged.len() > 4096 {
            return Err(error(
                "INVALID_LOSS_ACKNOWLEDGEMENT",
                "Loss acknowledgements must be unique and bounded",
            ));
        }
        for id in acknowledged {
            if !self.losses.iter().any(|v| &v.id == id && !v.blocking) {
                return Err(error(
                    "INVALID_LOSS_ACKNOWLEDGEMENT",
                    format!("Unknown or blocking loss: {id}"),
                ));
            }
        }
        Ok(self
            .losses
            .iter()
            .all(|v| !v.blocking && unique.contains(&v.id)))
    }
    fn report(&self, ready: bool) -> Value {
        json!({"profile":"otio-editorial-v1","ready":ready,"losses":self.losses,"names":self.names,
            "required_acknowledgements":self.losses.iter().filter(|v|!v.blocking).map(|v|&v.id).collect::<Vec<_>>(),
            "fetches_media":false,"updates_session":false})
    }
    fn unsupported(&mut self, failure: crate::Error) {
        let (path, message) = failure
            .message
            .split_once(": ")
            .unwrap_or(("$", &failure.message));
        self.loss(path, "unsupported_structure", message, true);
    }
}

fn invalid(path: &str, message: &str) -> crate::Error {
    error("UNSUPPORTED_INTERCHANGE", format!("{path}: {message}"))
}
fn object<'a>(value: &'a Value, path: &str) -> Result<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| invalid(path, "Expected an object"))
}
fn array<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| invalid(path, "Expected an array"))
}
fn text<'a>(value: &'a Value, path: &str) -> Result<&'a str> {
    value.as_str().ok_or_else(|| invalid(path, "Expected text"))
}
fn boolean(value: &Value, path: &str) -> Result<bool> {
    if value.is_null() {
        Ok(true)
    } else {
        value
            .as_bool()
            .ok_or_else(|| invalid(path, "Expected a boolean"))
    }
}
fn empty(value: &Value) -> bool {
    value.is_null()
        || value.as_array().is_some_and(Vec::is_empty)
        || value.as_object().is_some_and(serde_json::Map::is_empty)
}
fn schema(value: &Value, expected: &[&str], path: &str) -> Result<()> {
    if !expected.contains(&text(&value["OTIO_SCHEMA"], path)?) {
        return Err(invalid(path, "Unsupported object schema/version"));
    }
    Ok(())
}
fn fields(value: &Value, allowed: &[&str], path: &str, a: &mut Analysis) -> Result<()> {
    for (key, value) in object(value, path)? {
        if !allowed.contains(&key.as_str()) {
            // Unknown fields may affect interpretation, so acknowledgement alone
            // cannot authorize a guessed conversion.
            a.loss(
                &format!("{path}/{key}"),
                "unknown_field",
                "Unknown field has no supported interpretation",
                true,
            );
        } else if matches!(
            key.as_str(),
            "metadata" | "effects" | "markers" | "color" | "available_image_bounds"
        ) && !empty(value)
        {
            a.loss(
                &format!("{path}/{key}"),
                "omitted_property",
                format!("{key} is omitted from the native editing snapshot"),
                false,
            );
        }
    }
    Ok(())
}

// Accept integer-valued floating spellings such as 25.0, but never round a
// fractional clock or value. Every accepted integer is exactly representable.
fn integer(value: &Value, path: &str) -> Result<i64> {
    let value = value
        .as_f64()
        .ok_or_else(|| invalid(path, "Expected a finite integer clock value"))?;
    if !value.is_finite() || value.fract() != 0.0 || value.abs() > MAX_INTEGER {
        return Err(invalid(
            path,
            "This profile requires exact safe integer values and rates",
        ));
    }
    Ok(value as i64)
}
#[derive(Clone, Copy)]
struct Signed {
    num: i64,
    den: u64,
}
fn rational(value: &Value, path: &str) -> Result<Signed> {
    schema(value, &["RationalTime.1"], path)?;
    if object(value, path)?
        .keys()
        .any(|k| !["OTIO_SCHEMA", "value", "rate"].contains(&k.as_str()))
    {
        return Err(invalid(path, "Unknown rational-time field"));
    }
    let num = integer(&value["value"], path)?;
    let den = integer(&value["rate"], path)?;
    if den <= 0 {
        return Err(invalid(path, "Time rate must be positive"));
    }
    Ok(Signed {
        num,
        den: den as u64,
    })
}
fn positive(value: Signed, path: &str) -> Result<Time> {
    if value.num < 0 {
        return Err(invalid(path, "Duration or offset cannot be negative"));
    }
    Time::new(value.num as u64, value.den)
}
fn difference(a: Signed, b: Signed, path: &str) -> Result<Time> {
    let mut n = a.num as i128 * b.den as i128 - b.num as i128 * a.den as i128;
    let mut d = a.den as i128 * b.den as i128;
    if n < 0 {
        return Err(invalid(path, "Source range starts before available media"));
    }
    let (mut x, mut y) = (n, d);
    while y != 0 {
        let t = x % y;
        x = y;
        y = t;
    }
    n /= x;
    d /= x;
    if n > MAX_INTEGER as i128 || d > MAX_INTEGER as i128 {
        return Err(invalid(
            path,
            "Exact time exceeds the native integer bounds",
        ));
    }
    Time::new(n as u64, d as u64)
}
fn range(value: &Value, path: &str) -> Result<(Signed, Time)> {
    schema(value, &["TimeRange.1"], path)?;
    if object(value, path)?
        .keys()
        .any(|k| !["OTIO_SCHEMA", "start_time", "duration"].contains(&k.as_str()))
    {
        return Err(invalid(path, "Unknown time-range field"));
    }
    Ok((
        rational(&value["start_time"], path)?,
        positive(rational(&value["duration"], path)?, path)?,
    ))
}
fn rt(time: Time) -> Value {
    json!({"OTIO_SCHEMA":"RationalTime.1","value":time.num,"rate":time.den})
}
fn tr(start: Time, duration: Time) -> Value {
    json!({"OTIO_SCHEMA":"TimeRange.1","start_time":rt(start),"duration":rt(duration)})
}

fn checked_asset(asset: &Asset, root: &Path) -> Result<PathBuf> {
    let identity = asset.identity.as_ref().ok_or_else(|| {
        error(
            "IDENTITY_REQUIRED",
            "Interchange media bindings require a content identity",
        )
    })?;
    identity.validate()?;
    let path = local_path(&asset.path, root)?;
    if path.metadata()?.len() != identity.bytes || media::file_hash(&path)? != identity.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            format!("Bound media changed: {}", asset.id),
        ));
    }
    Ok(path)
}

fn local_path(path: &str, root: &Path) -> Result<PathBuf> {
    let source = Path::new(path);
    media::allowed_file(
        &if source.is_absolute() {
            source.into()
        } else {
            root.join(source)
        },
        root,
    )
}

pub fn import(request: &Import) -> Result<Value> {
    if request.source.bytes > LIMIT || request.bindings.len() > 1000 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Interchange accepts at most 4 MiB and 1000 media bindings",
        ));
    }
    media::input_root(&request.media_root)?;
    let (source, bytes) = scene::identity_bytes(&request.source, &request.input_root)?;
    if source.extension().and_then(|s| s.to_str()) != Some("otio") {
        return Err(error(
            "UNSUPPORTED_INTERCHANGE",
            "Interchange source must have .otio extension",
        ));
    }
    let document: Value = serde_json::from_slice(&bytes)?;
    let mut result = import_document(request, &document)?;
    if media::file_hash(&source)? != request.source.sha256 {
        return Err(error(
            "MEDIA_CHANGED",
            "Interchange source changed during inspection",
        ));
    }
    result["source"] = json!(request.source);
    result["writes_files"] = json!(false);
    Ok(result)
}

/// Shared semantic mapping for a bounded document already read by an original
/// native-project transfer adapter. File identity validation belongs to its caller.
pub(crate) fn import_document(request: &Import, document: &Value) -> Result<Value> {
    if request.bindings.len() > 1000 {
        return Err(error("LIMIT_EXCEEDED", "At most 1000 media bindings"));
    }
    media::input_root(&request.media_root)?;
    let mut a = Analysis::default();
    let project = match import::prepare(request, document, &mut a) {
        Ok(v) => Some(v),
        Err(e) if e.code == "UNSUPPORTED_INTERCHANGE" => {
            a.unsupported(e);
            None
        }
        Err(e) => return Err(e),
    };
    let ready = a.permitted(&request.acknowledged_losses)? && project.is_some();
    let mut result = a.report(ready);
    if ready {
        let mut project = project.expect("prepared project");
        for asset in &mut project.assets {
            asset.path = checked_asset(asset, &request.media_root)?
                .to_string_lossy()
                .into_owned();
            if let Some(proxy) = &mut asset.proxy {
                let path = local_path(&proxy.path, &request.media_root)?;
                if path.metadata()?.len() != proxy.identity.bytes
                    || media::file_hash(&path)? != proxy.identity.sha256
                {
                    return Err(error(
                        "MEDIA_CHANGED",
                        format!("Bound proxy changed: {}", asset.id),
                    ));
                }
                proxy.path = path.to_string_lossy().into_owned();
            }
        }
        result["project"] = json!(project);
    } else {
        result["project"] = Value::Null;
    }
    Ok(result)
}

pub fn inspect_export(project: &Project, input_root: &Path) -> Result<Value> {
    let (document, a) = export::prepare(project, input_root)?;
    let mut result = a.report(a.losses.is_empty());
    result["document"] = if a.losses.iter().any(|v| v.blocking) {
        Value::Null
    } else {
        document
    };
    result["writes_files"] = json!(false);
    Ok(result)
}

pub fn export(request: &Export) -> Result<Value> {
    let output = render::destination_extension(&request.output, &request.output_root, "otio")?;
    let (document, a) = export::prepare(&request.project, &request.input_root)?;
    if !a.permitted(&request.acknowledged_losses)? {
        let mut report = a.report(false);
        report["output"] = Value::Null;
        return Ok(report);
    }
    let bytes = serde_json::to_vec_pretty(&document)?;
    if bytes.len() as u64 + 1 > LIMIT {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Serialized interchange exceeds 4 MiB",
        ));
    }
    // A uniquely created sibling allows atomic no-overwrite publication.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| error("CLOCK_ERROR", "Clock is before epoch"))?
        .as_nanos();
    let temp = output.with_file_name(format!(
        ".cutbolt-interchange-{}-{stamp}.tmp",
        std::process::id()
    ));
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp)?;
    let outcome = (|| {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);
        for asset in &request.project.assets {
            checked_asset(asset, &request.input_root)?;
        }
        let sha256 = media::file_hash(&temp)?;
        media::publish(&temp, &output)?;
        let mut result = a.report(true);
        result["output"] = json!(output);
        result["sha256"] = json!(sha256);
        result["bytes"] = json!(bytes.len() + 1);
        Ok(result)
    })();
    let _ = fs::remove_file(&temp);
    outcome
}

pub fn capabilities() -> Value {
    json!({"profile":"otio-editorial-v1","format":"otio_json","document_maximum_bytes":LIMIT,"maximum_bindings":1000,
        "time":"safe_integer_value_and_positive_integer_rate;exact_rationals","media":"explicit_identity_bound_local_assets;no_URL_fetch",
        "losses":"path_specific_acknowledgements;blocking_unknown_structure","imports":"new_proposed_snapshot","exports":"no_overwrite","network":false})
}
