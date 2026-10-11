//! Vertex AI / Gemini function-declaration `Schema` subset (#4798).
//!
//! The field list is the Vertex AI `Schema` object
//! (https://cloud.google.com/vertex-ai/docs/reference/rest/v1/Schema).
//! Keywords outside that object, type arrays, and untyped nodes are rejected
//! by those clients for the whole `tools/list`.

use serde_json::Value;

const VERTEX_SCHEMA_FIELDS: &[&str] = &[
    "type",
    "format",
    "title",
    "description",
    "default",
    "items",
    "minItems",
    "maxItems",
    "enum",
    "properties",
    "propertyOrdering",
    "required",
    "minProperties",
    "maxProperties",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "pattern",
    "example",
    "anyOf",
    "additionalProperties",
    "$defs",
];

const VERTEX_SCHEMA_TYPES: &[&str] = &[
    "string", "number", "integer", "boolean", "array", "object", "null",
];

/// Every node of `schema` that falls outside the Vertex AI `Schema` object.
/// Walks schema positions only, so property names and `default` / `example` /
/// `enum` payloads are not mistaken for keywords.
pub fn input_schema_violations(schema: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(schema, "$", &mut out);
    out
}

fn walk(schema: &Value, path: &str, out: &mut Vec<String>) {
    let Some(node) = schema.as_object() else {
        out.push(format!("{path}: schema must be an object, got {schema}"));
        return;
    };
    for key in node.keys() {
        if !VERTEX_SCHEMA_FIELDS.contains(&key.as_str()) {
            out.push(format!(
                "{path}: `{key}` is not a field of the Vertex AI Schema object"
            ));
        }
    }
    match node.get("type") {
        Some(Value::String(name)) if VERTEX_SCHEMA_TYPES.contains(&name.as_str()) => {}
        Some(other) => out.push(format!(
            "{path}: type must be one of {VERTEX_SCHEMA_TYPES:?} as a single string, got {other}"
        )),
        None => out.push(format!("{path}: schema node has no type")),
    }
    if let Some(values) = node.get("enum") {
        match values.as_array() {
            Some(values) if values.iter().all(Value::is_string) => {}
            _ => out.push(format!(
                "{path}: enum must be a list of strings, got {values}"
            )),
        }
    }
    for keyword in ["properties", "$defs"] {
        if let Some(children) = node.get(keyword).and_then(Value::as_object) {
            for (name, child) in children {
                walk(child, &format!("{path}.{keyword}.{name}"), out);
            }
        }
    }
    if let Some(items) = node.get("items") {
        walk(items, &format!("{path}.items"), out);
    }
    if let Some(additional) = node.get("additionalProperties") {
        if !additional.is_boolean() {
            walk(additional, &format!("{path}.additionalProperties"), out);
        }
    }
    if let Some(variants) = node.get("anyOf").and_then(Value::as_array) {
        for (index, variant) in variants.iter().enumerate() {
            walk(variant, &format!("{path}.anyOf[{index}]"), out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn lint(schema: Value) -> Vec<String> {
        input_schema_violations(&schema)
    }

    #[test]
    fn vertex_lint_follows_the_documented_schema_fields() {
        let accepted = lint(json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["value"],
            "properties": {
                "value": {
                    "type": "string",
                    "anyOf": [{"type": "string", "minLength": 1}, {"type": "string", "enum": ["x"]}]
                },
                "limit": {"type": "integer", "minimum": 0, "maximum": 9},
                "tags": {"type": "array", "minItems": 1, "maxItems": 2, "items": {"type": "string"}},
                "const": {"type": "string", "default": {"oneOf": "payload, not a keyword"}}
            }
        }));
        assert!(accepted.is_empty(), "{accepted:#?}");

        let rejected = lint(json!({
            "type": "object",
            "properties": {
                "a": {"oneOf": [{"type": "string"}]},
                "b": {"type": "string", "const": "x"},
                "c": {"type": ["number", "null"]},
                "d": {"type": "array", "uniqueItems": true, "items": {"enum": ["x"]}},
                "e": {"type": "boolean", "enum": [true]},
                "f": {"anyOf": [{"type": "string"}, {"type": "integer"}]}
            }
        }));
        for expected in [
            "$.properties.a: `oneOf` is not a field",
            "$.properties.a: schema node has no type",
            "$.properties.b: `const` is not a field",
            "$.properties.c: type must be one of",
            "$.properties.d: `uniqueItems` is not a field",
            "$.properties.d.items: schema node has no type",
            "$.properties.e: enum must be a list of strings",
            "$.properties.f: schema node has no type",
        ] {
            assert!(
                rejected
                    .iter()
                    .any(|violation| violation.starts_with(expected)),
                "missing `{expected}` in {rejected:#?}"
            );
        }
    }

    #[test]
    fn published_input_schemas_are_vertex_gemini_compatible() {
        let mut violations = Vec::new();
        for contract in cua_driver_contract::manifest().tools {
            for violation in input_schema_violations(&contract.input_schema) {
                violations.push(format!("{} {violation}", contract.name));
            }
        }
        assert!(
            violations.is_empty(),
            "input schema nodes outside the Vertex AI Schema object (#4798):\n{}",
            violations.join("\n")
        );
    }
}
