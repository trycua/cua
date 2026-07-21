// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Small JSON-Schema builders matching cua-driver's existing schema dialect.

use serde_json::{json, Map, Value};

pub fn string(description: Option<&str>) -> Value {
    let mut value = json!({ "type": "string" });
    if let Some(description) = description {
        value["description"] = Value::String(description.into());
    }
    value
}

pub fn string_enum(values: &[&str], description: Option<&str>) -> Value {
    let mut value = json!({ "type": "string", "enum": values });
    if let Some(description) = description {
        value["description"] = Value::String(description.into());
    }
    value
}

pub fn nullable(schema: Value) -> Value {
    json!({ "anyOf": [schema, { "type": "null" }] })
}

pub fn object(
    required: &[&str],
    properties: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let properties: Map<String, Value> = properties
        .into_iter()
        .map(|(name, schema)| (name.to_owned(), schema))
        .collect();
    json!({
        "type": "object",
        "required": required,
        "properties": properties,
        "additionalProperties": true
    })
}

pub fn closed_object(
    required: &[&str],
    properties: impl IntoIterator<Item = (&'static str, Value)>,
) -> Value {
    let mut value = object(required, properties);
    value["additionalProperties"] = Value::Bool(false);
    value
}

pub fn session_state_output(extra: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut required = vec![
        Value::String("session".into()),
        Value::String("capture_scope".into()),
        Value::String("effective_scope".into()),
        Value::String("desktop_unlocked".into()),
        Value::String("escalation_reason".into()),
        Value::String("escalation_detail".into()),
    ];
    let mut properties = Map::from_iter([
        ("session".into(), string(None)),
        (
            "capture_scope".into(),
            string_enum(&["auto", "window", "desktop"], None),
        ),
        (
            "effective_scope".into(),
            string_enum(&["window", "desktop"], None),
        ),
        ("desktop_unlocked".into(), json!({ "type": "boolean" })),
        (
            "escalation_reason".into(),
            nullable(string_enum(
                &[
                    "ax_tree_pixel_mismatch",
                    "background_delivery_failed",
                    "foreground_ineffective",
                    "no_window_target",
                    "other",
                ],
                None,
            )),
        ),
        ("escalation_detail".into(), nullable(string(None))),
    ]);
    for (name, value) in extra {
        required.push(Value::String(name.into()));
        properties.insert(name.into(), value);
    }
    json!({
        "type": "object",
        "required": required,
        "properties": properties,
        "additionalProperties": true
    })
}
