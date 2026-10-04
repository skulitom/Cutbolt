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

/// The complete schema of a command's arguments or of a deferred shared type.
pub fn lookup(name: &str) -> Result<Value> {
    if let Some((public, definition)) = DEFERRED.iter().find(|(p, _)| *p == name) {
        let schema = attach(json!({"$ref":format!("#/$defs/{definition}")}), false);
        return Ok(json!({"name":public,"kind":"type","schema":schema}));
    }
    let command = name
        .strip_prefix("cutbolt_")
        .map_or(name.to_owned(), |t| t.replace('_', "."));
    let Some(schema) = arguments(&command, false) else {
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
    Ok(
        json!({"name":command,"kind":"command","description":crate::mcp::description(&command),
        "mcp_tool":tool,"schema":schema}),
    )
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
    fn lookups_return_complete_schemas() {
        for (public, definition) in DEFERRED {
            let found = lookup(public).unwrap();
            assert_eq!(found["schema"]["$ref"], format!("#/$defs/{definition}"));
            assert!(found["schema"]["$defs"][definition].is_object());
        }
        for command in commands() {
            let found = lookup(command).unwrap();
            assert_eq!(found["kind"], "command");
            for target in references(&found["schema"]) {
                assert!(
                    found["schema"]["$defs"][&target].is_object(),
                    "{command} lacks {target}"
                );
            }
        }
        let by_tool = lookup("cutbolt_scene_inspect").unwrap();
        assert_eq!(by_tool["name"], "scene.inspect");
        assert_eq!(by_tool["mcp_tool"], "cutbolt_scene_inspect");
        assert!(lookup("scene.render").unwrap()["mcp_tool"].is_null());
        assert_eq!(lookup("scenes").unwrap_err().code, "UNKNOWN_SCHEMA");
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
}
