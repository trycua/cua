// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{CaptureScope, DesktopScope, EscalationReason, Platform};
use schemars::{generate::SchemaSettings, JsonSchema};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Transport-free structured result used by both the live runtime and SDK generation.
pub trait ToolOutput: Serialize + DeserializeOwned + JsonSchema {
    fn validate(&self) -> Result<(), String> {
        Ok(())
    }

    fn output_schema() -> Value {
        let mut settings = SchemaSettings::draft2020_12();
        settings.inline_subschemas = true;
        settings.meta_schema = None;
        let mut schema =
            serde_json::to_value(settings.into_generator().into_root_schema_for::<Self>())
                .expect("JSON Schema serializes");
        strip_schema_titles(&mut schema);
        if let Some(object) = schema.as_object_mut() {
            object.insert("additionalProperties".into(), Value::Bool(true));
            object
                .entry("properties")
                .or_insert_with(|| Value::Object(Map::new()));
        }
        schema
    }
}

fn strip_schema_titles(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("title");
            object.remove("description");
            for child in object.values_mut() {
                strip_schema_titles(child);
            }
        }
        Value::Array(values) => values.iter_mut().for_each(strip_schema_titles),
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveScope {
    Window,
    Desktop,
}

impl EffectiveScope {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Window => "window",
            Self::Desktop => "desktop",
        }
    }
}

/// Successful structured result shared by session state and escalation tools.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct SessionStateOutput {
    pub session: String,
    pub capture_scope: CaptureScope,
    pub effective_scope: EffectiveScope,
    pub desktop_unlocked: bool,
    #[schemars(required, schema_with = "nullable_escalation_reason_schema")]
    pub escalation_reason: Option<EscalationReason>,
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub escalation_detail: Option<String>,
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub workspace_id: Option<String>,
}

impl ToolOutput for SessionStateOutput {}

/// Successful structured result returned by `start_session`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct StartSessionOutput {
    #[serde(flatten)]
    pub state: SessionStateOutput,
    pub active: bool,
    pub revived: bool,
}

impl ToolOutput for StartSessionOutput {}

/// Successful structured result returned by `end_session`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct EndSessionOutput {
    pub session: String,
    #[schemars(schema_with = "inactive_schema")]
    pub active: bool,
}

impl ToolOutput for EndSessionOutput {
    fn validate(&self) -> Result<(), String> {
        if self.active {
            Err("active must be false".into())
        } else {
            Ok(())
        }
    }
}

fn inactive_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "const": false })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DesktopStateOutput {
    #[schemars(schema_with = "platform_schema")]
    pub platform: Platform,
    pub display: String,
    #[schemars(schema_with = "integer_schema")]
    pub screenshot_width: u64,
    #[schemars(schema_with = "integer_schema")]
    pub screenshot_height: u64,
    #[schemars(schema_with = "integer_schema")]
    pub screen_width: u64,
    #[schemars(schema_with = "integer_schema")]
    pub screen_height: u64,
    #[schemars(schema_with = "number_schema")]
    pub scale_factor: f64,
    #[schemars(schema_with = "png_mime_schema")]
    pub screenshot_mime_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub screenshot_file_path: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for DesktopStateOutput {
    fn validate(&self) -> Result<(), String> {
        if self.screenshot_mime_type == "image/png" {
            Ok(())
        } else {
            Err("screenshot_mime_type must be image/png".into())
        }
    }
}

fn png_mime_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "const": "image/png" })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ScreenSizeOutput {
    #[schemars(schema_with = "number_schema")]
    pub width: f64,
    #[schemars(schema_with = "number_schema")]
    pub height: f64,
    #[schemars(schema_with = "number_schema")]
    pub scale_factor: f64,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for ScreenSizeOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CursorPositionOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "boolean_schema")]
    pub available: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub source: Option<String>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for CursorPositionOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct MoveCursorOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "desktop_scope_schema")]
    pub scope: Option<DesktopScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub y: Option<f64>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for MoveCursorOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ClickOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "desktop_scope_schema")]
    pub scope: Option<DesktopScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "number_schema")]
    pub y: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "boolean_schema")]
    pub verified: Option<bool>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for ClickOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct DesktopActionOutput {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "desktop_scope_schema")]
    pub scope: Option<DesktopScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub effect: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "boolean_schema")]
    pub verified: Option<bool>,
    #[serde(flatten)]
    pub extensions: BTreeMap<String, Value>,
}

impl ToolOutput for DesktopActionOutput {}

fn string_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "string" })
}

fn boolean_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "boolean" })
}

fn number_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "number" })
}

fn integer_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "type": "integer" })
}

fn desktop_scope_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "const": "desktop" })
}

fn platform_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "string",
        "enum": ["macos", "linux", "windows"]
    })
}

fn nullable_string_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
}

fn nullable_escalation_reason_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "anyOf": [
            {
                "type": "string",
                "enum": [
                    "ax_tree_pixel_mismatch",
                    "background_delivery_failed",
                    "foreground_ineffective",
                    "no_window_target",
                    "other"
                ]
            },
            { "type": "null" }
        ]
    })
}
