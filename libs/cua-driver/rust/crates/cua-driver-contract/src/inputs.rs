// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Typed inputs for the published Cua Driver SDK surface.
//!
//! These types are transport-free. The contract generator derives JSON Schema
//! from them, and live Rust handlers deserialize the same types before acting.

use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

pub trait ToolInput: Serialize + DeserializeOwned + JsonSchema {
    const TOOL_NAME: &'static str;

    fn input_schema() -> Value {
        let settings = schemars::generate::SchemaSettings::draft2020_12().with(|settings| {
            settings.meta_schema = None;
            settings.inline_subschemas = true;
        });
        let schema = settings.into_generator().into_root_schema_for::<Self>();
        let mut value = serde_json::to_value(schema).expect("tool input schema serializes");
        normalize_schema(&mut value);
        value
    }
}

fn normalize_schema(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("title");
            if object.get("type").and_then(Value::as_str) == Some("object") {
                object
                    .entry("properties")
                    .or_insert_with(|| Value::Object(serde_json::Map::new()));
                object
                    .entry("required")
                    .or_insert_with(|| Value::Array(Vec::new()));
                object
                    .entry("additionalProperties")
                    .or_insert(Value::Bool(true));
            }
            for child in object.values_mut() {
                normalize_schema(child);
            }
        }
        Value::Array(values) => {
            for child in values {
                normalize_schema(child);
            }
        }
        _ => {}
    }
}

fn string_schema(generator: &mut SchemaGenerator) -> Schema {
    String::json_schema(generator)
}

fn string_list_schema(generator: &mut SchemaGenerator) -> Schema {
    Vec::<String>::json_schema(generator)
}

fn number_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "number" })
}

fn click_button_schema(generator: &mut SchemaGenerator) -> Schema {
    ClickButton::json_schema(generator)
}

fn scroll_by_schema(generator: &mut SchemaGenerator) -> Schema {
    ScrollBy::json_schema(generator)
}

fn positive_integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1 })
}

fn drag_duration_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 0, "maximum": 10000 })
}

fn drag_steps_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": 200 })
}

fn scroll_amount_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": 50 })
}

fn escalation_detail_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "maxLength": 200 })
}

fn capture_scope_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "string",
        "enum": ["auto", "window", "desktop"],
        "default": "auto"
    })
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CaptureScope {
    #[default]
    Auto,
    Window,
    Desktop,
}

impl CaptureScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "window" => Some(Self::Window),
            "desktop" => Some(Self::Desktop),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Window => "window",
            Self::Desktop => "desktop",
        }
    }
}

impl std::fmt::Display for CaptureScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EscalationReason {
    AxTreePixelMismatch,
    BackgroundDeliveryFailed,
    ForegroundIneffective,
    NoWindowTarget,
    Other,
}

impl EscalationReason {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "ax_tree_pixel_mismatch" => Some(Self::AxTreePixelMismatch),
            "background_delivery_failed" => Some(Self::BackgroundDeliveryFailed),
            "foreground_ineffective" => Some(Self::ForegroundIneffective),
            "no_window_target" => Some(Self::NoWindowTarget),
            "other" => Some(Self::Other),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AxTreePixelMismatch => "ax_tree_pixel_mismatch",
            Self::BackgroundDeliveryFailed => "background_delivery_failed",
            Self::ForegroundIneffective => "foreground_ineffective",
            Self::NoWindowTarget => "no_window_target",
            Self::Other => "other",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DesktopScope {
    #[serde(rename = "desktop")]
    Desktop,
}

impl JsonSchema for DesktopScope {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DesktopScope".into()
    }

    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({ "const": "desktop" })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClickButton {
    Left,
    Right,
    Middle,
}

impl ClickButton {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

impl ScrollDirection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScrollBy {
    Line,
    Page,
}

impl ScrollBy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Line => "line",
            Self::Page => "page",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct StartSessionInput {
    /// Stable session id for this run (e.g. "research-run-1").
    pub session: String,
    /// Per-session perception/action modality. auto starts window-only and requires explicit escalation before desktop tools; window and desktop are strict. Immutable for the live session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "capture_scope_schema")]
    pub capture_scope: Option<CaptureScope>,
}

impl ToolInput for StartSessionInput {
    const TOOL_NAME: &'static str = "start_session";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EscalateSessionInput {
    pub session: String,
    pub reason: EscalationReason,
    /// Optional bounded diagnostic detail. Never use secrets or page content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "escalation_detail_schema")]
    pub detail: Option<String>,
}

impl ToolInput for EscalateSessionInput {
    const TOOL_NAME: &'static str = "escalate_session";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct GetSessionStateInput {
    pub session: String,
}

impl ToolInput for GetSessionStateInput {
    const TOOL_NAME: &'static str = "get_session_state";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct EndSessionInput {
    /// The session id to end.
    pub session: String,
}

impl ToolInput for EndSessionInput {
    const TOOL_NAME: &'static str = "end_session";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GetDesktopStateInput {
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    /// Write the PNG here instead of returning base64.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub screenshot_out_file: Option<String>,
}

impl ToolInput for GetDesktopStateInput {
    const TOOL_NAME: &'static str = "get_desktop_state";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GetScreenSizeInput {
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
}

impl ToolInput for GetScreenSizeInput {
    const TOOL_NAME: &'static str = "get_screen_size";
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GetCursorPositionInput {
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
}

impl ToolInput for GetCursorPositionInput {
    const TOOL_NAME: &'static str = "get_cursor_position";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoveCursorInput {
    #[schemars(schema_with = "number_schema")]
    pub x: f64,
    #[schemars(schema_with = "number_schema")]
    pub y: f64,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
}

impl ToolInput for MoveCursorInput {
    const TOOL_NAME: &'static str = "move_cursor";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ClickInput {
    #[schemars(schema_with = "number_schema")]
    pub x: f64,
    #[schemars(schema_with = "number_schema")]
    pub y: f64,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "click_button_schema")]
    pub button: Option<ClickButton>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "positive_integer_schema")]
    pub count: Option<u32>,
}

impl ToolInput for ClickInput {
    const TOOL_NAME: &'static str = "click";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DragInput {
    #[schemars(schema_with = "number_schema")]
    pub from_x: f64,
    #[schemars(schema_with = "number_schema")]
    pub from_y: f64,
    #[schemars(schema_with = "number_schema")]
    pub to_x: f64,
    #[schemars(schema_with = "number_schema")]
    pub to_y: f64,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "drag_duration_schema")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "drag_steps_schema")]
    pub steps: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "click_button_schema")]
    pub button: Option<ClickButton>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_list_schema")]
    pub modifier: Option<Vec<String>>,
}

impl ToolInput for DragInput {
    const TOOL_NAME: &'static str = "drag";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScrollInput {
    #[schemars(schema_with = "number_schema")]
    pub x: f64,
    #[schemars(schema_with = "number_schema")]
    pub y: f64,
    pub direction: ScrollDirection,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "scroll_by_schema")]
    pub by: Option<ScrollBy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "scroll_amount_schema")]
    pub amount: Option<u64>,
}

impl ToolInput for ScrollInput {
    const TOOL_NAME: &'static str = "scroll";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TypeTextInput {
    pub text: String,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
}

impl ToolInput for TypeTextInput {
    const TOOL_NAME: &'static str = "type_text";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PressKeyInput {
    pub key: String,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_list_schema")]
    pub modifiers: Option<Vec<String>>,
}

impl ToolInput for PressKeyInput {
    const TOOL_NAME: &'static str = "press_key";
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HotkeyInput {
    #[schemars(length(min = 2))]
    pub keys: Vec<String>,
    pub scope: DesktopScope,
    /// Optional session id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
}

impl ToolInput for HotkeyInput {
    const TOOL_NAME: &'static str = "hotkey";
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn generated_click_schema_matches_driver_dialect() {
        let schema = ClickInput::input_schema();
        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["x", "y", "scope"]));
        assert_eq!(schema["properties"]["scope"], json!({ "const": "desktop" }));
        assert_eq!(
            schema["properties"]["button"],
            json!({ "type": "string", "enum": ["left", "right", "middle"] })
        );
        assert_eq!(
            schema["properties"]["count"],
            json!({ "type": "integer", "minimum": 1 })
        );
    }

    #[test]
    fn generated_session_schema_preserves_default_and_open_input() {
        let schema = StartSessionInput::input_schema();
        assert_eq!(schema["additionalProperties"], true);
        assert_eq!(schema["required"], json!(["session"]));
        assert_eq!(schema["properties"]["capture_scope"]["default"], "auto");
        assert_eq!(
            schema["properties"]["capture_scope"]["enum"],
            json!(["auto", "window", "desktop"])
        );
    }

    #[test]
    fn serde_and_schema_reject_unknown_fields() {
        let error = serde_json::from_value::<ClickInput>(json!({
            "x": 1,
            "y": 2,
            "scope": "desktop",
            "pid": 3
        }))
        .expect_err("portable input must reject runtime-only fields");
        assert!(error.to_string().contains("unknown field `pid`"));
    }
}
