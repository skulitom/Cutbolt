//! Listing-only summaries for the default catalog. The parser and full lookups are unchanged.
use serde_json::{Value, json};

pub(crate) fn listing(schema: &mut Value) {
    schema
        .as_object_mut()
        .expect("tool schema")
        .remove("$schema");
    if let Some(time) = schema.pointer_mut("/$defs/Time") {
        *time = json!({"description":"Exact seconds: 2.5, \"5/2\", or {num,den}.","type":["number","string","object"]});
    }
    descriptions(schema);
}

fn clause(text: &str) -> String {
    let first = text
        .trim()
        .split('\n')
        .next()
        .unwrap_or_default()
        .split("; ")
        .next()
        .unwrap_or_default()
        .split(". ")
        .next()
        .unwrap_or_default()
        .trim();
    if first.chars().count() <= 120 {
        first.to_owned()
    } else {
        format!(
            "{}...",
            first.chars().take(117).collect::<String>().trim_end()
        )
    }
}

fn descriptions(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            let required = object.get("required").cloned().unwrap_or_else(|| json!([]));
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                for (name, property) in properties {
                    if !required
                        .as_array()
                        .is_some_and(|names| names.iter().any(|v| v == name))
                        && let Some(Value::String(text)) = property.get_mut("description")
                    {
                        *text = clause(text);
                    }
                }
            }
            for (key, value) in object {
                // These contain literal values, not schemas (even when they look like schemas).
                if !matches!(key.as_str(), "default" | "enum" | "const" | "examples") {
                    descriptions(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(descriptions),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(before: &Value, after: &Value) {
        if let Some(properties) = before.get("properties").and_then(Value::as_object) {
            for (name, property) in properties {
                let required = before["required"]
                    .as_array()
                    .is_some_and(|r| r.iter().any(|v| v == name));
                let now = &after["properties"][name];
                if required {
                    assert_eq!(
                        property.get("description"),
                        now.get("description"),
                        "required {name}"
                    );
                } else if let Some(text) = now["description"].as_str() {
                    assert!(text.chars().count() <= 120, "optional {name}: {text}");
                }
            }
        }
        match before {
            Value::Object(object) => {
                for (key, value) in object {
                    if !matches!(
                        key.as_str(),
                        "default" | "enum" | "const" | "examples" | "Time"
                    ) {
                        check(value, &after[key]);
                    }
                }
            }
            Value::Array(items) => {
                for (i, value) in items.iter().enumerate() {
                    check(value, &after[i]);
                }
            }
            _ => {}
        }
    }

    #[test]
    fn optional_descriptions_are_bounded_and_required_descriptions_survive() {
        for tool in crate::mcp::tools(None) {
            let mut schema = tool["inputSchema"].clone();
            listing(&mut schema);
            assert!(schema.get("$schema").is_none());
            check(&tool["inputSchema"], &schema);
            if let Some(time) = schema.pointer("/$defs/Time") {
                assert!(time.to_string().len() < 120);
                assert_eq!(time["type"], json!(["number", "string", "object"]));
            }
        }
    }

    #[test]
    fn literal_values_and_unicode_are_preserved() {
        let literal = json!({"properties":{"x":{"description":"long; literal description"}}});
        let mut schema = json!({"type":"object", "default":literal, "enum":[literal],
            "properties":{"x":{"description":"α".repeat(150)}, "y":{"description":"First clause; full explanation."}}});
        listing(&mut schema);
        assert_eq!(schema["default"], literal);
        assert_eq!(schema["enum"][0], literal);
        assert_eq!(
            schema["properties"]["x"]["description"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            120
        );
        assert_eq!(schema["properties"]["y"]["description"], "First clause");
    }
}
