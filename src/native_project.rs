//! Original content-bound transfer contract. The installed source application
//! reads its own native format; this module validates the explicit transfer.
use crate::{Result, error, interchange, media, registry, scene, time::Time};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::PathBuf};

const PROTOCOL: &str = "cutbolt-native-transfer-v1";
const PROFILE: &str = "native-flat-timeline-v1";

/// Request for native.import: validate a content-bound native-project transfer and propose a native snapshot.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Import {
    /// The actual native project, never opened for writing by this engine.
    pub source: scene::Identity,
    /// A transfer produced by the source application's public project interface.
    pub transfer: scene::Identity,
    /// An explicitly reviewed, content-bound adapter/build acceptance manifest.
    pub acceptance: scene::Identity,
    /// Existing absolute directory containing the three distinct `source`, `transfer` and `acceptance` files.
    pub input_root: PathBuf,
    /// Existing absolute directory that every bound asset and proxy path must resolve inside.
    pub media_root: PathBuf,
    /// ID of the proposed project.
    pub id: String,
    /// Up to 1000 media bindings, as for interchange.import.
    pub bindings: Vec<interchange::Binding>,
    /// Unique IDs of nonblocking `host:` issues and editorial losses to accept; default none.
    #[serde(default)]
    pub acknowledged_losses: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Acceptance {
    schema_version: u32,
    profile: String,
    adapter_id: String,
    adapter_sha256: String,
    builds: Vec<Build>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Build {
    version: String,
    application_sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Producer {
    adapter_id: String,
    adapter_sha256: String,
    version: String,
    application_sha256: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    id: String,
    path: String,
    code: String,
    message: String,
    blocking: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Transfer {
    protocol: String,
    profile: String,
    producer: Producer,
    source: registry::Identity,
    selected_sequence: String,
    sequence_count: u32,
    complete: bool,
    width: u32,
    height: u32,
    frame_rate: Time,
    issues: Vec<Issue>,
    document: Value,
}

fn sha(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn label(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}
fn invalid(message: &str) -> crate::Error {
    error("INVALID_NATIVE_TRANSFER", message)
}

pub fn import(request: &Import) -> Result<Value> {
    if request.transfer.bytes > 4 * 1024 * 1024 || request.acceptance.bytes > 64 * 1024 {
        return Err(error(
            "LIMIT_EXCEEDED",
            "Transfer/acceptance maximums are 4 MiB/64 KiB",
        ));
    }
    let (source_path, source_bytes) = scene::identity_bytes(&request.source, &request.input_root)?;
    drop(source_bytes);
    let (transfer_path, bytes) = scene::identity_bytes(&request.transfer, &request.input_root)?;
    let transfer: Transfer = serde_json::from_slice(&bytes)?;
    let (acceptance_path, bytes) = scene::identity_bytes(&request.acceptance, &request.input_root)?;
    let acceptance: Acceptance = serde_json::from_slice(&bytes)?;
    if source_path == transfer_path
        || source_path == acceptance_path
        || transfer_path == acceptance_path
    {
        return Err(invalid(
            "Native source, transfer and acceptance must be distinct files",
        ));
    }
    if acceptance.schema_version != 1
        || acceptance.profile != PROFILE
        || transfer.protocol != PROTOCOL
        || transfer.profile != PROFILE
        || !label(&acceptance.adapter_id, 128)
        || !sha(&acceptance.adapter_sha256)
        || acceptance.builds.is_empty()
        || acceptance.builds.len() > 32
    {
        return Err(invalid(
            "Unsupported or invalid transfer/acceptance contract",
        ));
    }
    let mut versions = BTreeSet::new();
    for build in &acceptance.builds {
        if !label(&build.version, 128)
            || !sha(&build.application_sha256)
            || !versions.insert(&build.version)
        {
            return Err(invalid(
                "Acceptance requires unique exact builds and application identities",
            ));
        }
    }
    if transfer.producer.adapter_id != acceptance.adapter_id
        || transfer.producer.adapter_sha256 != acceptance.adapter_sha256
    {
        return Err(error(
            "NATIVE_ADAPTER_MISMATCH",
            "Transfer adapter is outside the accepted identity",
        ));
    }
    if !acceptance.builds.iter().any(|b| {
        b.version == transfer.producer.version
            && b.application_sha256 == transfer.producer.application_sha256
    }) {
        return Err(error(
            "UNSUPPORTED_NATIVE_BUILD",
            "Exact source-application build is not in this acceptance matrix",
        ));
    }
    transfer.source.validate()?;
    if transfer.source.bytes != request.source.bytes
        || transfer.source.sha256 != request.source.sha256
    {
        return Err(error(
            "NATIVE_SOURCE_MISMATCH",
            "Transfer belongs to a different native project identity",
        ));
    }
    if !transfer.complete
        || !label(&transfer.selected_sequence, 512)
        || transfer.sequence_count == 0
        || transfer.sequence_count > 1000
        || transfer.issues.len() > 1024
    {
        return Err(invalid(
            "Transfer must declare a complete bounded sequence selection and issue inventory",
        ));
    }
    let mut issue_ids = BTreeSet::new();
    for issue in &transfer.issues {
        if !issue.id.starts_with("host:")
            || !label(&issue.id, 512)
            || !issue_ids.insert(&issue.id)
            || !label(&issue.path, 1024)
            || !label(&issue.code, 128)
            || !label(&issue.message, 2048)
        {
            return Err(invalid(
                "Host issues require unique namespaced IDs and bounded explicit details",
            ));
        }
    }
    if transfer.sequence_count > 1
        && !transfer
            .issues
            .iter()
            .any(|i| i.code == "unselected_sequences")
    {
        return Err(invalid(
            "Multiple sequences require an explicit unselected-sequence diagnostic",
        ));
    }
    let acknowledgements: BTreeSet<_> = request.acknowledged_losses.iter().collect();
    if acknowledgements.len() != request.acknowledged_losses.len() || acknowledgements.len() > 4096
    {
        return Err(error(
            "INVALID_LOSS_ACKNOWLEDGEMENT",
            "Acknowledgements must be unique and bounded",
        ));
    }
    for id in &request.acknowledged_losses {
        if id.starts_with("host:") && !transfer.issues.iter().any(|i| i.id == *id && !i.blocking) {
            return Err(error(
                "INVALID_LOSS_ACKNOWLEDGEMENT",
                "Unknown or blocking host issue",
            ));
        }
    }
    let request_document = interchange::Import {
        source: request.transfer.clone(),
        input_root: request.input_root.clone(),
        media_root: request.media_root.clone(),
        id: request.id.clone(),
        width: transfer.width,
        height: transfer.height,
        frame_rate: transfer.frame_rate,
        bindings: request.bindings.clone(),
        acknowledged_losses: request
            .acknowledged_losses
            .iter()
            .filter(|s| !s.starts_with("host:"))
            .cloned()
            .collect(),
    };
    let mut report = interchange::import_document(&request_document, &transfer.document)?;
    let host_ready = transfer
        .issues
        .iter()
        .all(|i| !i.blocking && acknowledgements.contains(&i.id));
    if !host_ready {
        report["ready"] = json!(false);
        report["project"] = Value::Null;
    }
    let required = report["required_acknowledgements"]
        .as_array_mut()
        .expect("interchange acknowledgements");
    required.extend(
        transfer
            .issues
            .iter()
            .filter(|i| !i.blocking)
            .map(|i| json!(i.id)),
    );
    report["host_issues"] = json!(transfer.issues);
    report["profile"] = json!(PROFILE);
    report["producer"] = json!(transfer.producer);
    report["selected_sequence"] = json!(transfer.selected_sequence);
    report["source"] = json!(request.source);
    report["transfer"] = json!(request.transfer);
    report["acceptance"] = json!(request.acceptance);
    report["writes_files"] = json!(false);
    report["executes_adapter"] = json!(false);
    for (path, expected) in [
        (&source_path, &request.source),
        (&transfer_path, &request.transfer),
        (&acceptance_path, &request.acceptance),
    ] {
        if path.metadata()?.len() != expected.bytes || media::file_hash(path)? != expected.sha256 {
            return Err(error(
                "MEDIA_CHANGED",
                "Native source, transfer or acceptance changed during import",
            ));
        }
    }
    Ok(report)
}

pub fn capabilities() -> Value {
    json!({"protocol":PROTOCOL,"profile":PROFILE,"source_maximum_bytes":67108864,"transfer_maximum_bytes":4194304,
        "acceptance_maximum_bytes":65536,"maximum_exact_builds":32,"maximum_host_issues":1024,
        "native_reader":"external_source_application_public_interface","native_parser":false,"executes_adapter":false,
        "provenance":"caller_reviewed_hash_bound_transfer;not_a_cryptographic_attestation","output":"proposed_snapshot_only",
        "media":"explicit_identity_bound_local_bindings","losses":"explicit_host_and_editorial_diagnostics;blocking_issues_cannot_be_acknowledged"})
}
