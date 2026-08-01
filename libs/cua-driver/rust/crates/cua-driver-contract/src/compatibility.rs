// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Mechanical compatibility checks between portable SDK schemas and live tools.
//!
//! This module intentionally implements only the JSON Schema dialect used by
//! the current portable desktop contracts. It proves implication in one
//! direction: every value accepted by the portable schema is also accepted by
//! the live schema. Encountering an unknown validation keyword is an error, not
//! an invitation to silently claim parity.

use std::collections::BTreeSet;

use serde_json::Value;

const SUPPORTED_KEYWORDS: &[&str] = &[
    "type",
    "const",
    "enum",
    "minimum",
    "maximum",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "items",
    "required",
    "properties",
    "additionalProperties",
];

const ANNOTATION_KEYWORDS: &[&str] = &["description", "default", "title", "examples"];

/// Return every reason `portable` is not a subset of `live`.
///
/// An empty result proves that any input accepted by `portable` is accepted by
/// `live` for the supported dialect. This is not a general JSON Schema
/// implication engine.
pub fn schema_subset_violations(portable: &Value, live: &Value) -> Vec<String> {
    let mut violations = Vec::new();
    compare_schema("$", portable, live, &mut violations);
    violations
}

fn compare_schema(path: &str, portable: &Value, live: &Value, violations: &mut Vec<String>) {
    let Some(portable_object) = portable.as_object() else {
        violations.push(format!("{path}: portable schema must be an object"));
        return;
    };
    let Some(live_object) = live.as_object() else {
        violations.push(format!("{path}: live schema must be an object"));
        return;
    };

    check_keywords(path, "portable", portable_object.keys(), violations);
    check_keywords(path, "live", live_object.keys(), violations);

    compare_type(path, portable, live, violations);
    compare_allowed_values(path, portable, live, violations);
    compare_lower_bound(path, "minimum", portable, live, violations);
    compare_upper_bound(path, "maximum", portable, live, violations);
    compare_lower_bound(path, "minLength", portable, live, violations);
    compare_upper_bound(path, "maxLength", portable, live, violations);
    compare_lower_bound(path, "minItems", portable, live, violations);
    compare_upper_bound(path, "maxItems", portable, live, violations);

    if let Some(live_items) = live.get("items") {
        match portable.get("items") {
            Some(portable_items) => compare_schema(
                &format!("{path}.items"),
                portable_items,
                live_items,
                violations,
            ),
            None => violations.push(format!(
                "{path}: live constrains array items but portable does not"
            )),
        }
    }

    compare_object(path, portable, live, violations);
}

fn check_keywords<'a>(
    path: &str,
    side: &str,
    keys: impl Iterator<Item = &'a String>,
    violations: &mut Vec<String>,
) {
    for key in keys {
        if !SUPPORTED_KEYWORDS.contains(&key.as_str())
            && !ANNOTATION_KEYWORDS.contains(&key.as_str())
        {
            violations.push(format!("{path}: unsupported {side} schema keyword `{key}`"));
        }
    }
}

fn compare_type(path: &str, portable: &Value, live: &Value, violations: &mut Vec<String>) {
    let Some(live_type) = live.get("type").and_then(Value::as_str) else {
        return;
    };
    let Some(portable_type) = portable.get("type").and_then(Value::as_str) else {
        if let Some(values) = allowed_values(portable) {
            for value in values {
                if !value_matches_type(value, live_type) {
                    violations.push(format!(
                        "{path}: portable value {value} does not satisfy live type `{live_type}`"
                    ));
                }
            }
        } else {
            violations.push(format!(
                "{path}: live requires type `{live_type}` but portable has no type constraint"
            ));
        }
        return;
    };

    let compatible =
        portable_type == live_type || (portable_type == "integer" && live_type == "number");
    if !compatible {
        violations.push(format!(
            "{path}: portable type `{portable_type}` is not a subset of live type `{live_type}`"
        ));
    }
}

fn value_matches_type(value: &Value, expected: &str) -> bool {
    match expected {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "string" => value.is_string(),
        _ => false,
    }
}

fn allowed_values(schema: &Value) -> Option<Vec<&Value>> {
    if let Some(value) = schema.get("const") {
        return Some(vec![value]);
    }
    schema
        .get("enum")
        .and_then(Value::as_array)
        .map(|values| values.iter().collect())
}

fn compare_allowed_values(
    path: &str,
    portable: &Value,
    live: &Value,
    violations: &mut Vec<String>,
) {
    let Some(live_values) = allowed_values(live) else {
        return;
    };
    let Some(portable_values) = allowed_values(portable) else {
        violations.push(format!(
            "{path}: live restricts allowed values but portable does not"
        ));
        return;
    };
    for value in portable_values {
        if !live_values.contains(&value) {
            violations.push(format!(
                "{path}: portable value {value} is not accepted by the live schema"
            ));
        }
    }
}

fn numeric_keyword<'a>(schema: &'a Value, keyword: &str) -> Option<&'a Value> {
    schema.get(keyword)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn compare_lower_bound(
    path: &str,
    keyword: &str,
    portable: &Value,
    live: &Value,
    violations: &mut Vec<String>,
) {
    let Some(live_value) = numeric_keyword(live, keyword) else {
        return;
    };
    let Some(live_number) = number(live_value) else {
        violations.push(format!("{path}.{keyword}: live bound must be numeric"));
        return;
    };
    let Some(portable_value) = numeric_keyword(portable, keyword) else {
        violations.push(format!(
            "{path}: live defines {keyword}={live_value} but portable does not"
        ));
        return;
    };
    let Some(portable_number) = number(portable_value) else {
        violations.push(format!("{path}.{keyword}: portable bound must be numeric"));
        return;
    };
    if portable_number < live_number {
        violations.push(format!(
            "{path}: portable {keyword}={portable_value} is broader than live {keyword}={live_value}"
        ));
    }
}

fn compare_upper_bound(
    path: &str,
    keyword: &str,
    portable: &Value,
    live: &Value,
    violations: &mut Vec<String>,
) {
    let Some(live_value) = numeric_keyword(live, keyword) else {
        return;
    };
    let Some(live_number) = number(live_value) else {
        violations.push(format!("{path}.{keyword}: live bound must be numeric"));
        return;
    };
    let Some(portable_value) = numeric_keyword(portable, keyword) else {
        violations.push(format!(
            "{path}: live defines {keyword}={live_value} but portable does not"
        ));
        return;
    };
    let Some(portable_number) = number(portable_value) else {
        violations.push(format!("{path}.{keyword}: portable bound must be numeric"));
        return;
    };
    if portable_number > live_number {
        violations.push(format!(
            "{path}: portable {keyword}={portable_value} is broader than live {keyword}={live_value}"
        ));
    }
}

fn string_set(schema: &Value, keyword: &str) -> BTreeSet<String> {
    schema
        .get(keyword)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn compare_object(path: &str, portable: &Value, live: &Value, violations: &mut Vec<String>) {
    let portable_properties = portable.get("properties").and_then(Value::as_object);
    let live_properties = live.get("properties").and_then(Value::as_object);

    if let Some(portable_properties) = portable_properties {
        let Some(live_properties) = live_properties else {
            violations.push(format!(
                "{path}: portable declares properties but live schema does not"
            ));
            return;
        };
        for (name, portable_property) in portable_properties {
            let property_path = format!("{path}.properties.{name}");
            let Some(live_property) = live_properties.get(name) else {
                violations.push(format!("{property_path}: missing from live schema"));
                continue;
            };
            compare_schema(&property_path, portable_property, live_property, violations);
        }
    }

    let portable_required = string_set(portable, "required");
    let live_required = string_set(live, "required");
    for required in live_required.difference(&portable_required) {
        violations.push(format!(
            "{path}: live requires `{required}` but portable clients may omit it"
        ));
    }

    let portable_additional = portable
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let live_additional = live
        .get("additionalProperties")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if portable_additional && !live_additional {
        violations.push(format!(
            "{path}: portable permits additional properties but live rejects them"
        ));
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn narrower_portable_object_is_accepted() {
        let portable = json!({
            "type": "object",
            "required": ["x", "scope"],
            "properties": {
                "x": { "type": "integer", "minimum": 1 },
                "scope": { "const": "desktop" }
            },
            "additionalProperties": false
        });
        let live = json!({
            "type": "object",
            "required": ["x"],
            "properties": {
                "x": { "type": "number", "minimum": 0 },
                "scope": { "type": "string", "enum": ["window", "desktop"] },
                "pid": { "type": "integer" }
            },
            "additionalProperties": false
        });
        assert!(schema_subset_violations(&portable, &live).is_empty());
    }

    #[test]
    fn missing_property_and_live_requirement_fail() {
        let portable = json!({
            "type": "object",
            "properties": { "x": { "type": "number" } },
            "additionalProperties": false
        });
        let live = json!({
            "type": "object",
            "required": ["pid"],
            "properties": { "pid": { "type": "integer" } },
            "additionalProperties": false
        });
        let violations = schema_subset_violations(&portable, &live);
        assert!(violations
            .iter()
            .any(|value| value.contains("properties.x")));
        assert!(violations
            .iter()
            .any(|value| value.contains("requires `pid`")));
    }

    #[test]
    fn broader_enum_and_bounds_fail() {
        let portable = json!({ "type": "integer", "enum": [1, 2], "minimum": 0 });
        let live = json!({ "type": "integer", "enum": [1], "minimum": 1 });
        let violations = schema_subset_violations(&portable, &live);
        assert!(violations
            .iter()
            .any(|value| value.contains("portable value 2")));
        assert!(violations.iter().any(|value| value.contains("minimum")));
    }

    #[test]
    fn unknown_validation_keyword_fails_closed() {
        let portable = json!({ "type": "string", "pattern": "^[a-z]+$" });
        let live = json!({ "type": "string" });
        let violations = schema_subset_violations(&portable, &live);
        assert_eq!(violations.len(), 1);
        assert!(violations[0].contains("unsupported portable schema keyword `pattern`"));
    }

    #[test]
    fn additional_property_domain_must_be_accepted_live() {
        let portable = json!({ "type": "object" });
        let live = json!({ "type": "object", "additionalProperties": false });
        let violations = schema_subset_violations(&portable, &live);
        assert!(violations
            .iter()
            .any(|value| value.contains("portable permits additional properties")));
    }
}
