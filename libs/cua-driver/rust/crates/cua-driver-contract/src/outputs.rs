// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use crate::{CaptureScope, EscalationReason, Platform};
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
        output_schema_with_additional_properties::<Self>(true)
    }
}

fn output_schema_with_additional_properties<T: JsonSchema>(additional_properties: bool) -> Value {
    let mut settings = SchemaSettings::draft2020_12();
    settings.inline_subschemas = true;
    settings.meta_schema = None;
    let mut schema = serde_json::to_value(settings.into_generator().into_root_schema_for::<T>())
        .expect("JSON Schema serializes");
    strip_schema_titles(&mut schema);
    if let Some(object) = schema.as_object_mut() {
        object.insert(
            "additionalProperties".into(),
            Value::Bool(additional_properties),
        );
        object
            .entry("properties")
            .or_insert_with(|| Value::Object(Map::new()));
    }
    schema
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct CursorMotionOutput {
    pub start_handle: f64,
    pub end_handle: f64,
    pub arc_size: f64,
    pub arc_flow: f64,
    pub spring: f64,
    pub glide_duration_ms: f64,
    pub dwell_after_click_ms: f64,
    pub idle_hide_ms: f64,
    pub turn_radius: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct CursorThemeOutput {
    pub id: String,
    pub version: String,
    pub profile: String,
    pub reduced_motion: crate::CursorReducedMotion,
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub fallback: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct CursorVisualOutput {
    pub requested_action: crate::CursorAction,
    pub resolved_action: crate::CursorAction,
    pub modifiers: Vec<String>,
    pub phase: String,
    pub frame: u64,
    pub preempted_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct CursorPointOutput {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct SetAgentCursorEnabledOutput {
    pub session: String,
    pub enabled: bool,
}

impl ToolOutput for SetAgentCursorEnabledOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct SetAgentCursorMotionOutput {
    pub session: String,
    pub motion: CursorMotionOutput,
}

impl ToolOutput for SetAgentCursorMotionOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct SetAgentCursorThemeOutput {
    pub session: String,
    pub theme: CursorThemeOutput,
}

impl ToolOutput for SetAgentCursorThemeOutput {}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
pub struct GetAgentCursorStateOutput {
    pub session: String,
    pub enabled: bool,
    #[schemars(required)]
    pub position: Option<CursorPointOutput>,
    pub theme: CursorThemeOutput,
    pub visual_state: CursorVisualOutput,
    pub motion: CursorMotionOutput,
}

impl ToolOutput for GetAgentCursorStateOutput {}

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardReadOutput {
    pub supported: bool,
    pub types: Vec<String>,
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub text: Option<String>,
    pub privacy_sensitive: bool,
    pub content_redacted_from_telemetry: bool,
}

impl ToolOutput for ClipboardReadOutput {
    fn validate(&self) -> Result<(), String> {
        if !self.supported {
            return Err("successful clipboard reads must be supported".into());
        }
        if !self.privacy_sensitive || !self.content_redacted_from_telemetry {
            return Err("clipboard privacy metadata must remain enabled".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct ClipboardWriteOutput {
    pub supported: bool,
    pub written_type: String,
    pub types: Vec<String>,
    pub privacy_sensitive: bool,
    pub content_redacted_from_telemetry: bool,
}

impl ToolOutput for ClipboardWriteOutput {
    fn validate(&self) -> Result<(), String> {
        if !self.supported {
            return Err("successful clipboard writes must be supported".into());
        }
        if !matches!(self.written_type.as_str(), "text" | "image" | "file_url") {
            return Err("written_type must be text, image, or file_url".into());
        }
        if !self.privacy_sensitive || !self.content_redacted_from_telemetry {
            return Err("clipboard privacy metadata must remain enabled".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionEffect {
    Confirmed,
    Partial,
    Unverifiable,
    SuspectedNoop,
    Refused,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionRoute {
    Accessibility,
    SyntheticEvents,
    GlobalInput,
    SystemApi,
    Dom,
    TrustedInput,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionDeliveryMode {
    Background,
    Foreground,
    NotApplicable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ActionDelivery {
    pub mode: ActionDeliveryMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivered_count: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionEvidenceKind {
    ValueReadback,
    WindowChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ActionEvidence {
    pub kind: ActionEvidenceKind,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionEscalationTarget {
    Pixel,
    Foreground,
    Page,
    Session,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum ActionEscalationReason {
    RouteUnavailable,
    DeliveryFailed,
    EffectUnconfirmed,
    SuspectedNoop,
    PermissionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ActionEscalation {
    pub target: ActionEscalationTarget,
    pub reason: ActionEscalationReason,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ActionResult {
    pub effect: ActionEffect,
    pub route: ActionRoute,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<ActionDelivery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Vec<ActionEvidence>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub escalation: Option<ActionEscalation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionResultValidationError {
    ConfirmedRequiresEvidence,
    PartialRequiresDeliveredCount,
    RefusedCannotHaveDelivery,
    RefusedCannotHaveEvidence,
}

impl std::fmt::Display for ActionResultValidationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::ConfirmedRequiresEvidence => "confirmed effect requires evidence",
            Self::PartialRequiresDeliveredCount => "partial effect requires delivered_count",
            Self::RefusedCannotHaveDelivery => "refused effect cannot include delivery",
            Self::RefusedCannotHaveEvidence => "refused effect cannot include evidence",
        })
    }
}

impl std::error::Error for ActionResultValidationError {}

impl ActionResult {
    pub fn validate_invariants(&self) -> Result<(), ActionResultValidationError> {
        match self.effect {
            ActionEffect::Confirmed
                if self
                    .evidence
                    .as_ref()
                    .is_none_or(|evidence| evidence.is_empty()) =>
            {
                Err(ActionResultValidationError::ConfirmedRequiresEvidence)
            }
            ActionEffect::Partial
                if self
                    .delivery
                    .as_ref()
                    .and_then(|delivery| delivery.delivered_count)
                    .is_none() =>
            {
                Err(ActionResultValidationError::PartialRequiresDeliveredCount)
            }
            ActionEffect::Refused if self.delivery.is_some() => {
                Err(ActionResultValidationError::RefusedCannotHaveDelivery)
            }
            ActionEffect::Refused if self.evidence.is_some() => {
                Err(ActionResultValidationError::RefusedCannotHaveEvidence)
            }
            _ => Ok(()),
        }
    }
}

impl ToolOutput for ActionResult {
    fn validate(&self) -> Result<(), String> {
        self.validate_invariants()
            .map_err(|error| error.to_string())
    }

    fn output_schema() -> Value {
        output_schema_with_additional_properties::<Self>(false)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn object_variant(schema: &Value) -> &Value {
        if schema.get("properties").is_some() {
            return schema;
        }
        schema["anyOf"]
            .as_array()
            .expect("nullable anyOf")
            .iter()
            .find(|variant| variant.get("properties").is_some())
            .expect("object variant")
    }

    fn confirmed_result() -> ActionResult {
        ActionResult {
            effect: ActionEffect::Confirmed,
            route: ActionRoute::Accessibility,
            delivery: Some(ActionDelivery {
                mode: ActionDeliveryMode::Background,
                delivered_count: None,
            }),
            evidence: Some(vec![ActionEvidence {
                kind: ActionEvidenceKind::ValueReadback,
            }]),
            escalation: None,
        }
    }

    #[test]
    fn action_result_schema_is_exact_and_closed() {
        let schema = ActionResult::output_schema();
        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["required"], json!(["effect", "route"]));

        let properties = schema["properties"].as_object().expect("properties");
        assert_eq!(
            properties.keys().map(String::as_str).collect::<Vec<_>>(),
            ["delivery", "effect", "escalation", "evidence", "route"]
        );
        assert_eq!(
            properties["effect"]["enum"],
            json!([
                "confirmed",
                "partial",
                "unverifiable",
                "suspected_noop",
                "refused"
            ])
        );
        assert_eq!(
            properties["route"]["enum"],
            json!([
                "accessibility",
                "synthetic_events",
                "global_input",
                "system_api",
                "dom",
                "trusted_input"
            ])
        );

        let delivery = object_variant(&properties["delivery"]);
        assert_eq!(delivery["additionalProperties"], false);
        assert_eq!(delivery["required"], json!(["mode"]));
        assert_eq!(
            delivery["properties"]["mode"]["enum"],
            json!(["background", "foreground", "not_applicable", "unknown"])
        );

        let evidence_schema = &properties["evidence"];
        let evidence_array = if evidence_schema.get("items").is_some() {
            evidence_schema
        } else {
            evidence_schema["anyOf"]
                .as_array()
                .expect("nullable anyOf")
                .iter()
                .find(|variant| variant.get("items").is_some())
                .expect("array variant")
        };
        let evidence = &evidence_array["items"];
        assert_eq!(evidence["additionalProperties"], false);
        assert_eq!(evidence["required"], json!(["kind"]));
        assert_eq!(
            evidence["properties"]["kind"]["enum"],
            json!(["value_readback", "window_change"])
        );

        let escalation = object_variant(&properties["escalation"]);
        assert_eq!(escalation["additionalProperties"], false);
        assert_eq!(escalation["required"], json!(["target", "reason"]));
        assert_eq!(
            escalation["properties"]["target"]["enum"],
            json!(["pixel", "foreground", "page", "session"])
        );
        assert_eq!(
            escalation["properties"]["reason"]["enum"],
            json!([
                "route_unavailable",
                "delivery_failed",
                "effect_unconfirmed",
                "suspected_noop",
                "permission_required"
            ])
        );
    }

    #[test]
    fn action_result_round_trips_without_legacy_or_request_fields() {
        let result = confirmed_result();
        let value = serde_json::to_value(&result).expect("serialize");
        assert_eq!(
            value,
            json!({
                "effect": "confirmed",
                "route": "accessibility",
                "delivery": {"mode": "background"},
                "evidence": [{"kind": "value_readback"}]
            })
        );
        assert_eq!(
            serde_json::from_value::<ActionResult>(value).expect("deserialize"),
            result
        );

        for legacy in [
            ("scope", json!("desktop")),
            ("target", json!("button")),
            ("x", json!(10)),
            ("y", json!(20)),
            ("path", json!("cgevent")),
            ("transport", json!("windows_send_input")),
            ("verified", json!(true)),
            ("extensions", json!({})),
        ] {
            let mut value = serde_json::to_value(&result).expect("serialize");
            value
                .as_object_mut()
                .expect("object")
                .insert(legacy.0.into(), legacy.1);
            assert!(
                serde_json::from_value::<ActionResult>(value).is_err(),
                "accepted legacy field {}",
                legacy.0
            );
        }

        let mut delivery_extension = serde_json::to_value(&result).expect("serialize");
        delivery_extension["delivery"]["requested"] = json!("background");
        assert!(serde_json::from_value::<ActionResult>(delivery_extension).is_err());

        let mut evidence_extension = serde_json::to_value(&result).expect("serialize");
        evidence_extension["evidence"][0]["detail"] = json!("private readback");
        assert!(serde_json::from_value::<ActionResult>(evidence_extension).is_err());

        let escalation_extension = json!({
            "effect": "unverifiable",
            "route": "synthetic_events",
            "escalation": {
                "target": "foreground",
                "reason": "delivery_failed",
                "requires": ["window_id"]
            }
        });
        assert!(serde_json::from_value::<ActionResult>(escalation_extension).is_err());
    }

    #[test]
    fn action_result_enforces_effect_invariants() {
        let mut result = confirmed_result();
        result.evidence = None;
        assert_eq!(
            result.validate_invariants(),
            Err(ActionResultValidationError::ConfirmedRequiresEvidence)
        );
        assert_eq!(
            ToolOutput::validate(&result),
            Err("confirmed effect requires evidence".into())
        );

        result.effect = ActionEffect::Partial;
        result.delivery = Some(ActionDelivery {
            mode: ActionDeliveryMode::Foreground,
            delivered_count: None,
        });
        assert_eq!(
            result.validate_invariants(),
            Err(ActionResultValidationError::PartialRequiresDeliveredCount)
        );
        result.delivery.as_mut().expect("delivery").delivered_count = Some(1);
        assert_eq!(result.validate_invariants(), Ok(()));

        result.effect = ActionEffect::Refused;
        assert_eq!(
            result.validate_invariants(),
            Err(ActionResultValidationError::RefusedCannotHaveDelivery)
        );
        result.delivery = None;
        result.evidence = Some(vec![ActionEvidence {
            kind: ActionEvidenceKind::WindowChange,
        }]);
        assert_eq!(
            result.validate_invariants(),
            Err(ActionResultValidationError::RefusedCannotHaveEvidence)
        );
        result.evidence = None;
        assert_eq!(result.validate_invariants(), Ok(()));
    }
}
