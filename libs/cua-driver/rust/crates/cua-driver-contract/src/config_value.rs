// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Key-aware parsing for the `set_config` `{key, value}` shape.
//!
//! `value` is advertised with a single `string` type so strict
//! function-calling validators (Vertex AI / Gemini) accept the tool list.
//! Clients that follow the schema therefore send `"800"` or `"true"`; this
//! turns those strings into the JSON type the key expects before the platform
//! handler reads them (#4798).

use serde_json::Value;

/// `set_config` keys whose value is a non-negative integer.
const INTEGER_KEYS: &[&str] = &["max_image_dimension"];

/// `set_config` keys whose value is a boolean, besides the
/// `cursor.motion.effects.<name>` family.
const BOOLEAN_KEYS: &[&str] = &["experimental_pip"];

fn is_boolean_key(key: &str) -> bool {
    BOOLEAN_KEYS.contains(&key) || key.starts_with("cursor.motion.effects.")
}

/// Parse a string `value` into the type `key` expects.
///
/// Integer keys accept decimal digits (surrounding whitespace ignored);
/// boolean keys accept `true` / `false`. Anything else, including values that
/// are already typed and strings that do not parse, is returned unchanged so
/// the platform handler's own type error still names the key.
pub fn coerce_set_config_value(key: &str, value: &Value) -> Value {
    let Some(text) = value.as_str() else {
        return value.clone();
    };
    let text = text.trim();
    if INTEGER_KEYS.contains(&key) {
        if let Ok(number) = text.parse::<u64>() {
            return Value::from(number);
        }
    } else if is_boolean_key(key) {
        match text {
            "true" => return Value::Bool(true),
            "false" => return Value::Bool(false),
            _ => {}
        }
    }
    value.clone()
}
