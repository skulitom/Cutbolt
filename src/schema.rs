//! Request schemas sized for agents. Each command's schema carries only the definitions it
//! uses, and listings abbreviate the largest shared types; `schema` returns any of them in full.
use crate::{Result, commands::Request, error};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::OnceLock};

/// Shared types abbreviated in tool listings, by public lookup name and definition name.
pub const DEFERRED: [(&str, &str); 5] = [
    ("project", "Project"),
    ("operation", "Operation"),
    ("scene", "Scene"),
    ("template", "Template"),
    ("audio_routing", "Routing"),
];
const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

fn full() -> &'static Value {
    static FULL: OnceLock<Value> = OnceLock::new();
    FULL.get_or_init(|| {
        serde_json::to_value(schemars::schema_for!(Request)).expect("request schema")
    })
}

fn definitions() -> &'static Map<String, Value> {
    full()["$defs"].as_object().expect("schema definitions")
}

/// Every command in declaration order, with its tagged request variant.
fn variants() -> impl Iterator<Item = (&'static str, &'static Value)> {
    full()["oneOf"]
        .as_array()
        .expect("tagged request variants")
        .iter()
        .map(|v| {
            (
                v["properties"]["command"]["const"]
                    .as_str()
                    .expect("command tag"),
                v,
            )
        })
}

pub fn commands() -> impl Iterator<Item = &'static str> {
    variants().map(|(command, _)| command)
}

/// Argument schema for one command (its request without `command`). With `abbreviate`, the
/// deferred shared types become short stubs naming the lookup that returns them.
pub fn arguments(command: &str, abbreviate: bool) -> Option<Value> {
    let (_, variant) = variants().find(|(c, _)| *c == command)?;
    let mut input = variant.clone();
    let object = input.as_object_mut()?;
    object.remove("description");
    object
        .get_mut("properties")?
        .as_object_mut()?
        .remove("command");
    if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
        required.retain(|p| p != "command");
    }
    if abbreviate {
        stub_deferred(&mut input);
    }
    Some(attach(input, abbreviate))
}

/// Add `$defs` holding only the definitions the schema reaches, then the dialect.
fn attach(mut schema: Value, abbreviate: bool) -> Value {
    let mut used = BTreeMap::new();
    let mut pending = references(&schema);
    while let Some(name) = pending.pop() {
        if used.contains_key(&name) {
            continue;
        }
        let mut definition = definitions()[&name].clone();
        if abbreviate {
            stub_deferred(&mut definition);
        }
        pending.extend(references(&definition));
        used.insert(name, definition);
    }
    let object = schema.as_object_mut().expect("object schema");
    if !used.is_empty() {
        object.insert("$defs".into(), json!(used));
    }
    object.insert("$schema".into(), DIALECT.into());
    schema
}

pub(crate) fn references(schema: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(schema, &mut |node| {
        if let Some(target) = node.get("$ref").and_then(Value::as_str) {
            found.push(
                target
                    .strip_prefix("#/$defs/")
                    .expect("local reference")
                    .to_owned(),
            );
        }
    });
    found
}

fn walk(schema: &Value, visit: &mut impl FnMut(&Map<String, Value>)) {
    match schema {
        Value::Object(object) => {
            visit(object);
            object.values().for_each(|v| walk(v, visit));
        }
        Value::Array(items) => items.iter().for_each(|v| walk(v, visit)),
        _ => {}
    }
}

/// Replace references to deferred types with a stub: their summary, any variant tags, and the
/// lookup that returns the complete definition.
fn stub_deferred(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            let target = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|t| t.strip_prefix("#/$defs/"));
            if let Some((public, name)) = DEFERRED.iter().find(|(_, d)| target == Some(*d)) {
                *schema = stub(public, name, object);
            } else {
                object.values_mut().for_each(stub_deferred);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(stub_deferred),
        _ => {}
    }
}

fn stub(public: &str, name: &str, reference: &Map<String, Value>) -> Value {
    let definition = &definitions()[name];
    let summary = reference
        .get("description")
        .or_else(|| definition.get("description"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut stub = json!({"type":"object","description":format!(
        "{summary} Abbreviated here; cutbolt_schema {{\"name\":\"{public}\"}} returns the complete schema."
    ).trim_start().to_owned()});
    if let Some((tag, values)) = discriminator(definition) {
        stub["required"] = json!([tag]);
        stub["properties"] = json!({tag:{"type":"string","enum":values,
            "description":"Variant tag; the complete schema lists each variant's fields."}});
    }
    stub
}

/// The property every object variant fixes to a constant, with those constants in order.
fn discriminator(definition: &Value) -> Option<(String, Vec<Value>)> {
    let branches = definition.get("oneOf")?.as_array()?;
    let first = branches.first()?.get("properties")?.as_object()?;
    first.iter().find_map(|(tag, _)| {
        let values = branches
            .iter()
            .map(|b| b["properties"][tag].get("const").cloned())
            .collect::<Option<Vec<_>>>()?;
        Some((tag.clone(), values))
    })
}

/// Lookups larger than this come back as an outline unless the whole schema is requested.
const OUTLINE_BYTES: usize = 16 * 1024;

/// The schema of a command's arguments or of a deferred shared type. Command schemas keep the
/// deferred types abbreviated, since each has its own lookup. `select` narrows the result to one
/// tagged variant or one definition; a large schema otherwise comes back as an outline.
pub fn lookup(name: &str, select: Option<&str>, full: bool) -> Result<Value> {
    let (mut found, schema) = if let Some((public, definition)) =
        DEFERRED.iter().find(|(p, _)| *p == name)
    {
        let schema = attach(json!({"$ref":format!("#/$defs/{definition}")}), false);
        (json!({"name":public,"kind":"type"}), schema)
    } else {
        let command = name
            .strip_prefix("cutbolt_")
            .map_or(name.to_owned(), |t| t.replace('_', "."));
        let Some(schema) = arguments(&command, true) else {
            let types = DEFERRED.map(|(p, _)| p).join(", ");
            return Err(error(
                "UNKNOWN_SCHEMA",
                format!(
                    "{name:?} is neither a command nor a shared type. Use a command from capabilities.commands (for example scene.render), its MCP tool name, or one of: {types}"
                ),
            ));
        };
        let tool =
            crate::mcp::exposed(&command).then(|| format!("cutbolt_{}", command.replace('.', "_")));
        let found = json!({"name":command,"kind":"command",
                "description":crate::mcp::description(&command),"mcp_tool":tool});
        (found, schema)
    };
    let schema = match select {
        Some(select) => {
            found["select"] = json!(select);
            narrow(&schema, select)?
        }
        None => schema,
    };
    let size = schema.to_string().len();
    if full || size <= OUTLINE_BYTES {
        found["schema"] = schema;
    } else {
        found["outline"] = outline(&schema);
        found["note"] = json!(format!(
            "The complete schema is {size} bytes. Pass select with a variant or definition name for one part with everything it references, or full: true for all of it."
        ));
    }
    Ok(found)
}

/// The root object of a looked-up schema: a referenced definition, or the schema itself.
fn root(schema: &Value) -> &Value {
    match schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|t| t.strip_prefix("#/$defs/"))
    {
        Some(name) => &schema["$defs"][name],
        None => schema,
    }
}

/// One tagged variant of the root, or one definition, with every definition it reaches.
fn narrow(schema: &Value, select: &str) -> Result<Value> {
    let root = root(schema);
    if let Some((tag, _)) = discriminator(root) {
        let branch = root["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|b| b["properties"][&tag]["const"] == select);
        if let Some(branch) = branch {
            return Ok(attach(branch.clone(), false));
        }
    }
    let defs = schema["$defs"].as_object();
    let name = defs.and_then(|d| {
        d.keys()
            .find(|k| *k == select)
            .or_else(|| d.keys().find(|k| k.eq_ignore_ascii_case(select)))
    });
    if let Some(name) = name {
        return Ok(attach(json!({"$ref":format!("#/$defs/{name}")}), false));
    }
    let mut choices: Vec<String> = discriminator(root)
        .map(|(_, tags)| {
            tags.iter()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    choices.extend(defs.into_iter().flat_map(|d| d.keys().cloned()));
    Err(error(
        "UNKNOWN_SCHEMA",
        format!(
            "select {select:?} is not a variant or definition here; choose one of: {}",
            choices.join(", ")
        ),
    ))
}

/// A compact map of a large schema: the root's variants or properties, and every definition it
/// reaches, each with its description.
fn outline(schema: &Value) -> Value {
    let root = root(schema);
    let described = |v: &Value| v["description"].as_str().unwrap_or_default().to_owned();
    let mut map = json!({"description":described(root)});
    if let Some((tag, _)) = discriminator(root) {
        map["variants"] = root["oneOf"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|b| {
                let fields: Vec<&String> = b["properties"]
                    .as_object()
                    .into_iter()
                    .flat_map(|p| p.keys())
                    .filter(|k| **k != tag)
                    .collect();
                json!({"select":b["properties"][&tag]["const"],"description":described(b),"fields":fields})
            })
            .collect();
    } else if let Some(properties) = root["properties"].as_object() {
        let summary: Map<String, Value> = properties
            .iter()
            .map(|(name, p)| {
                (
                    name.clone(),
                    json!({"description":described(p),"types":references(p)}),
                )
            })
            .collect();
        map["properties"] = Value::Object(summary);
    }
    let own = schema["$ref"]
        .as_str()
        .and_then(|t| t.strip_prefix("#/$defs/"));
    let definitions: Map<String, Value> = schema["$defs"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| Some(name.as_str()) != own)
        .map(|(name, d)| (name.clone(), json!(described(d))))
        .collect();
    map["definitions"] = Value::Object(definitions);
    map
}

fn dereference(schema: &'static Value) -> &'static Value {
    match schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|t| t.strip_prefix("#/$defs/"))
    {
        Some(name) => &definitions()[name],
        None => schema,
    }
}

/// Property names accepted by one request object: the request itself (`context` empty) or a
/// nested object such as render.start's `render`, choosing a tagged variant by its tag.
pub(crate) fn context_properties(command: &str, context: &str, object: &Value) -> Vec<String> {
    let Some((_, mut schema)) = variants().find(|(c, _)| *c == command) else {
        return Vec::new();
    };
    if !context.is_empty() {
        let Some(property) = schema["properties"].get(context) else {
            return Vec::new();
        };
        schema = dereference(property);
        if let Some(branches) = schema.get("oneOf").and_then(Value::as_array) {
            let tagged = |branch: &&Value| {
                branch["properties"].as_object().is_some_and(|p| {
                    p.iter()
                        .any(|(k, v)| v.get("const").is_some_and(|c| object.get(k) == Some(c)))
                })
            };
            let Some(branch) = branches.iter().find(tagged) else {
                return Vec::new();
            };
            schema = branch;
        }
    }
    schema["properties"]
        .as_object()
        .map(|p| p.keys().cloned().collect())
        .unwrap_or_default()
}

/// With a workspace, the roots it supplies become optional and say where they default.
pub fn relax_roots(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            if let Some(Value::String(text)) = object.get_mut("description") {
                *text = workspace_wording(text);
            }
            for (field, relative) in crate::workspace::DEFAULTS {
                let Some(property) = object.get_mut("properties").and_then(|p| p.get_mut(field))
                else {
                    continue;
                };
                let place = match relative {
                    "" => "the workspace".to_owned(),
                    inside => format!("{inside} in the workspace"),
                };
                let text = property["description"].as_str().unwrap_or_default();
                property["description"] =
                    json!(format!("{text} Optional: defaults to {place}.").trim_start());
                if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                    required.retain(|r| r != field);
                }
            }
            object.values_mut().for_each(relax_roots);
        }
        Value::Array(items) => items.iter_mut().for_each(relax_roots),
        _ => {}
    }
}

/// Phrases that require absolute paths without a workspace; with one, relative paths also work.
const ABSOLUTE_PHRASES: [(&str, &str); 13] = [
    ("Existing absolute media root", "Existing media root"),
    (
        "Existing absolute local directory",
        "Existing local directory",
    ),
    (
        "existing absolute local directory",
        "existing local directory",
    ),
    ("Existing absolute directory", "Existing directory"),
    ("Absolute local cache directory", "Local cache directory"),
    ("Absolute directory", "Directory"),
    ("Absolute path of", "Path of"),
    ("Unused absolute ", "Unused "),
    ("New absolute ", "New "),
    ("explicit absolute input_root", "explicit input_root"),
    ("absolute input/output roots", "input/output roots"),
    ("absolute path,", "path,"),
    ("Absolute candidate", "Candidate"),
];

/// Reword a description for a workspace server, where paths may be relative to the workspace.
pub fn workspace_wording(text: &str) -> String {
    ABSOLUTE_PHRASES
        .iter()
        .fold(text.to_owned(), |text, (absolute, either)| {
            text.replace(absolute, either)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Paths of schema nodes an agent would see without a description.
    fn undocumented() -> Vec<String> {
        fn check(schema: &Value, path: &str, missing: &mut Vec<String>) {
            match schema {
                Value::Object(object) => {
                    if let Some(properties) = object.get("properties").and_then(Value::as_object) {
                        for (name, property) in properties {
                            // Tag constants name themselves; every other property needs a description.
                            if property.get("const").is_none() && !documented(property) {
                                missing.push(format!("{path}.{name}"));
                            }
                        }
                    }
                    for key in ["oneOf", "anyOf"] {
                        for (i, branch) in object
                            .get(key)
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .enumerate()
                        {
                            // Object variants (tagged operations) each need their own description.
                            if branch.get("properties").is_some() && !documented(branch) {
                                missing.push(format!("{path}.{key}[{i}]"));
                            }
                        }
                    }
                    for (key, value) in object {
                        check(value, &format!("{path}.{key}"), missing);
                    }
                }
                Value::Array(items) => {
                    for (i, item) in items.iter().enumerate() {
                        check(item, &format!("{path}[{i}]"), missing);
                    }
                }
                _ => {}
            }
        }
        let mut missing = Vec::new();
        for (name, definition) in definitions() {
            if !documented(definition) {
                missing.push(format!("$defs.{name}"));
            }
            check(definition, &format!("$defs.{name}"), &mut missing);
        }
        for command in commands() {
            if crate::mcp::description(command) == crate::mcp::UNDESCRIBED {
                missing.push(format!("command {command}"));
            }
            let mut arguments = arguments(command, false).unwrap();
            arguments.as_object_mut().unwrap().remove("$defs");
            check(&arguments, &format!("command {command}"), &mut missing);
        }
        missing
    }

    fn documented(schema: &Value) -> bool {
        schema
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|d| !d.trim().is_empty())
    }

    #[test]
    fn every_schema_field_is_documented() {
        let missing = undocumented();
        for path in &missing {
            eprintln!("undocumented: {path}");
        }
        assert!(
            missing.is_empty(),
            "{} schema nodes lack a description",
            missing.len()
        );
    }

    #[test]
    fn lookups_return_outlines_selections_and_complete_schemas() {
        for (public, definition) in DEFERRED {
            let whole = lookup(public, None, true).unwrap();
            assert_eq!(whole["schema"]["$ref"], format!("#/$defs/{definition}"));
            assert!(whole["schema"]["$defs"][definition].is_object());
            let default = lookup(public, None, false).unwrap();
            assert!(default["schema"].is_object() || default["outline"]["definitions"].is_object());
        }
        for command in commands() {
            let found = lookup(command, None, true).unwrap();
            assert_eq!(found["kind"], "command");
            for target in references(&found["schema"]) {
                assert!(
                    found["schema"]["$defs"][&target].is_object(),
                    "{command} lacks {target}"
                );
            }
        }
        // Large types outline their parts; select returns one part with what it references.
        let operations = lookup("operation", None, false).unwrap();
        let variants = operations["outline"]["variants"].as_array().unwrap();
        assert!(
            variants
                .iter()
                .any(|v| v["select"] == "clip.append" && v["fields"] == json!(["clip"]))
        );
        let append = lookup("operation", Some("clip.append"), false).unwrap();
        assert!(append["schema"]["$defs"]["Clip"].is_object());
        assert!(append["schema"].to_string().len() < 4096);
        let layer = lookup("scene", Some("layer"), true).unwrap();
        assert_eq!(layer["schema"]["$ref"], "#/$defs/Layer");
        // A large selection is outlined too, so its parts can be selected in turn.
        let outlined = lookup("scene", Some("Layer"), false).unwrap();
        assert!(outlined["outline"]["properties"]["transform"].is_object());
        // Command lookups name shared types instead of repeating them.
        let render = lookup("scene.render", None, false).unwrap();
        assert!(render["schema"]["$defs"].get("Scene").is_none());
        assert!(render["schema"].to_string().len() < 4096);
        let by_tool = lookup("cutbolt_scene_inspect", None, false).unwrap();
        assert_eq!(by_tool["name"], "scene.inspect");
        assert_eq!(by_tool["mcp_tool"], "cutbolt_scene_inspect");
        assert!(lookup("scene.render", None, false).unwrap()["mcp_tool"].is_null());
        assert_eq!(
            lookup("scenes", None, false).unwrap_err().code,
            "UNKNOWN_SCHEMA"
        );
        let unknown = lookup("operation", Some("clip.add"), false).unwrap_err();
        assert_eq!(unknown.code, "UNKNOWN_SCHEMA");
    }

    #[test]
    fn abbreviated_operations_still_list_their_tags() {
        let apply = arguments("session.apply", true).unwrap();
        let operation = &apply["properties"]["operations"]["items"];
        assert_eq!(operation["required"], json!(["op"]));
        let tags = operation["properties"]["op"]["enum"].as_array().unwrap();
        assert!(tags.contains(&json!("clip.append")) && tags.contains(&json!("tracks.edit")));
        assert!(
            apply
                .get("$defs")
                .is_none_or(|d| d.get("Operation").is_none())
        );
    }

    fn accepts_reference(property: &Value) -> bool {
        property["$ref"] == "#/$defs/ProjectInput"
    }

    /// The workspace and reference resolution visit only the request, render.start's `render` and
    /// cache.run's `task`; a root or project input anywhere else would silently be skipped.
    #[test]
    fn roots_and_project_inputs_live_where_they_are_resolved() {
        assert_eq!(
            arguments("render.start", false).unwrap()["properties"]["render"]["$ref"],
            "#/$defs/RenderRequest"
        );
        assert_eq!(
            arguments("cache.run", false).unwrap()["properties"]["task"]["$ref"],
            "#/$defs/Task"
        );
        let roots: Vec<&str> = crate::workspace::DEFAULTS.iter().map(|(f, _)| *f).collect();
        for (name, definition) in definitions() {
            if ["RenderRequest", "Task", "SavedProject"].contains(&name.as_str()) {
                continue;
            }
            walk(definition, &mut |node| {
                for (field, property) in node
                    .get("properties")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                {
                    assert!(
                        !roots.contains(&field.as_str()) && !accepts_reference(property),
                        "{name}.{field} is outside the resolved request contexts"
                    );
                }
            });
        }
        for command in commands().filter(|c| *c != "session.create") {
            let schema = arguments(command, false).unwrap();
            if let Some(project) = schema["properties"].get("project") {
                assert!(
                    accepts_reference(project),
                    "{command}.project takes only snapshots"
                );
            }
        }
        for definition in ["RenderRequest", "Task"] {
            walk(&definitions()[definition], &mut |node| {
                if let Some(project) = node.get("properties").and_then(|p| p.get("project")) {
                    assert!(
                        accepts_reference(project),
                        "{definition}.project takes only snapshots"
                    );
                }
            });
        }
    }

    /// identity::complete treats any object of only path/sha256/bytes keys as a file identity.
    #[test]
    fn only_file_identities_look_like_file_identities() {
        let shape = ["path", "sha256", "bytes"];
        for (name, definition) in definitions() {
            walk(definition, &mut |node| {
                let Some(properties) = node.get("properties").and_then(Value::as_object) else {
                    return;
                };
                let looks = properties.contains_key("path")
                    && properties.keys().all(|k| shape.contains(&k.as_str()));
                assert!(
                    !looks || properties.len() == 3 && name == "Identity",
                    "{name} has an identity-like shape and would be hashed"
                );
            });
        }
    }

    #[test]
    fn workspace_schemas_make_supplied_roots_optional() {
        let mut render = arguments("render.start", true).unwrap();
        relax_roots(&mut render);
        assert_eq!(render["required"], json!(["request_id", "render"]));
        let nested = &render["$defs"]["RenderRequest"];
        assert!(
            !nested["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "input_root")
        );
        assert!(
            nested["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "output")
        );
        let described = nested["properties"]["input_root"]["description"]
            .as_str()
            .unwrap();
        assert!(described.ends_with("Optional: defaults to the workspace."));
        let task = json!({"type":"frame"});
        assert!(context_properties("cache.run", "task", &task).contains(&"project".to_owned()));
        assert!(
            context_properties("render.start", "", &json!({})).contains(&"job_root".to_owned())
        );
    }
}
