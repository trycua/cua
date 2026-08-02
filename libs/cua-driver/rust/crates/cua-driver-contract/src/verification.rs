// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cross-platform deterministic state-verification contract.
//!
//! The caller owns the task-level meaning of success. The driver only checks
//! bounded predicates against one explicitly authorized window.

use crate::{Platform, SchemaMode, ToolAnnotations, ToolContract, ToolInput, ToolOutput};
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Serialize};

const ALL_PLATFORMS: [Platform; 3] = [Platform::Macos, Platform::Windows, Platform::Linux];

pub const VERIFY_STATE_DEFAULT_TIMEOUT_MS: u64 = 5_000;

fn timeout_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "type": "integer",
        "minimum": 0,
        "maximum": 10000,
        "default": VERIFY_STATE_DEFAULT_TIMEOUT_MS
    })
}

fn stable_samples_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1, "maximum": 5, "default": 2 })
}

fn tolerance_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "number", "minimum": 0, "maximum": 100 })
}

fn number_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "number" })
}

fn integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer" })
}

fn positive_integer_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "integer", "minimum": 1 })
}

fn string_schema(generator: &mut SchemaGenerator) -> Schema {
    String::json_schema(generator)
}

fn nonempty_string_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "string", "minLength": 1 })
}

fn true_only_boolean_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "type": "boolean", "enum": [true] })
}

fn nullable_string_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
}

fn nullable_unknown_reason_schema(_: &mut SchemaGenerator) -> Schema {
    json_schema!({
        "anyOf": [
            {
                "type": "string",
                "enum": [
                    "invalid_predicate",
                    "unsupported_predicate",
                    "untrusted_source",
                    "multi_match",
                    "target_missing",
                    "observation_unavailable",
                    "stability_unproven"
                ]
            },
            { "type": "null" }
        ]
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct BoundsExpectation {
    #[schemars(schema_with = "number_schema")]
    pub x: f64,
    #[schemars(schema_with = "number_schema")]
    pub y: f64,
    #[schemars(schema_with = "number_schema")]
    pub width: f64,
    #[schemars(schema_with = "number_schema")]
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "tolerance_schema")]
    pub tolerance_px: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ElementSelector {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "nonempty_string_schema")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "nonempty_string_schema")]
    pub label_contains: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct WindowPredicate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounds: Option<BoundsExpectation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct ElementPredicate {
    pub selector: ElementSelector,
    /// Assert that at least one trusted element matches the selector.
    ///
    /// Element walks are not yet exhaustive on every platform, so absence
    /// cannot be proven. `false` is rejected instead of returning an
    /// indefinitely-unknown predicate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "true_only_boolean_schema")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_equals: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct StatePredicate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowPredicate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element: Option<ElementPredicate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, uniffi::Record)]
#[serde(deny_unknown_fields)]
pub struct VerifyStateInput {
    /// Exact process whose window may be observed.
    #[schemars(schema_with = "positive_integer_schema")]
    pub pid: i64,
    /// Exact native window identifier.
    #[schemars(schema_with = "integer_schema")]
    pub window_id: u64,
    /// One to eight predicates, combined with logical AND.
    #[schemars(length(min = 1, max = 8))]
    pub expect: Vec<StatePredicate>,
    /// Optional session id for capture-scope and authorization continuity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "string_schema")]
    pub session: Option<String>,
    /// Bounded wait. Zero performs one sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "timeout_schema")]
    pub timeout_ms: Option<u64>,
    /// Consecutive satisfied samples required before returning success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(schema_with = "stable_samples_schema")]
    pub stable_samples: Option<u64>,
    /// Return the final window screenshot as image content for a multimodal
    /// caller. The driver does not interpret that image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_screenshot: Option<bool>,
}

impl ToolInput for VerifyStateInput {
    const TOOL_NAME: &'static str = "verify_state";
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Satisfied,
    Unsatisfied,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Enum)]
#[serde(rename_all = "snake_case")]
pub enum UnknownReason {
    InvalidPredicate,
    UnsupportedPredicate,
    UntrustedSource,
    MultiMatch,
    TargetMissing,
    ObservationUnavailable,
    StabilityUnproven,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct PredicateOutcome {
    pub index: u64,
    pub status: VerificationStatus,
    #[schemars(required, schema_with = "nullable_unknown_reason_schema")]
    pub unknown_reason: Option<UnknownReason>,
    /// Normalized, bounded JSON projection of the matched state.
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub observed_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, uniffi::Record)]
pub struct VerifyStateOutput {
    pub status: VerificationStatus,
    pub stable: bool,
    pub elapsed_ms: u64,
    pub samples: u64,
    pub predicates: Vec<PredicateOutcome>,
}

impl ToolOutput for VerifyStateOutput {}

pub fn contracts() -> Vec<ToolContract> {
    vec![ToolContract {
        name: VerifyStateInput::TOOL_NAME.into(),
        description: "Deterministically verify bounded predicates against one exact window. \
            The driver evaluates structured window/accessibility state and may return the final \
            screenshot as uninterpreted visual evidence for a multimodal caller. Predicate \
            results are satisfied, unsatisfied, or unknown; unknown never implies success. \
            Accessibility projections are conservative: absence remains unknown unless the \
            observed search domain is proven exhaustive."
            .into(),
        platforms: ALL_PLATFORMS.to_vec(),
        aliases: Vec::new(),
        capabilities: vec![
            "state.verify".into(),
            "state.verify.window".into(),
            "state.verify.element".into(),
            "state.observe.visual_evidence".into(),
        ],
        annotations: ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        schema_mode: SchemaMode::PortableSubset,
        cursor_semantics: None,
        input_schema: VerifyStateInput::input_schema(),
        success_output_schema: Some(VerifyStateOutput::output_schema()),
        output_validator: crate::validate_typed_output::<VerifyStateOutput>,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn schema_bounds_the_polling_contract() {
        let schema = VerifyStateInput::input_schema();
        assert_eq!(schema["properties"]["expect"]["minItems"], 1);
        assert_eq!(schema["properties"]["expect"]["maxItems"], 8);
        assert_eq!(
            schema["properties"]["timeout_ms"]["default"],
            VERIFY_STATE_DEFAULT_TIMEOUT_MS
        );
        assert_eq!(schema["properties"]["timeout_ms"]["maximum"], 10_000);
        assert_eq!(schema["properties"]["stable_samples"]["maximum"], 5);
        let selector = &schema["properties"]["expect"]["items"]["properties"]["element"]
            ["properties"]["selector"];
        assert_eq!(selector["properties"]["role"]["minLength"], 1);
        assert_eq!(selector["properties"]["label_contains"]["minLength"], 1);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn predicate_shape_is_flat_and_language_binding_friendly() {
        let schema = VerifyStateInput::input_schema();
        let item = &schema["properties"]["expect"]["items"];
        assert!(item["properties"]["window"].is_object());
        assert!(item["properties"]["element"].is_object());
        assert!(item.get("oneOf").is_none());
    }

    #[test]
    fn output_requires_tri_state_and_bounded_evidence_projection() {
        let output = VerifyStateOutput {
            status: VerificationStatus::Unknown,
            stable: false,
            elapsed_ms: 10,
            samples: 1,
            predicates: vec![PredicateOutcome {
                index: 0,
                status: VerificationStatus::Unknown,
                unknown_reason: Some(UnknownReason::UntrustedSource),
                observed_json: Some(json!({"role":"web_area"}).to_string()),
            }],
        };
        assert!(output.validate().is_ok());
        assert_eq!(
            serde_json::to_value(output).unwrap()["status"],
            json!("unknown")
        );
        let schema = VerifyStateOutput::output_schema();
        let predicate = &schema["properties"]["predicates"]["items"];
        for field in ["unknown_reason", "observed_json"] {
            assert!(
                predicate["properties"][field]["anyOf"]
                    .as_array()
                    .is_some_and(|variants| variants
                        .iter()
                        .any(|variant| variant["type"] == "null")),
                "{field} must be required and nullable"
            );
        }
        let satisfied = VerifyStateOutput {
            status: VerificationStatus::Satisfied,
            stable: true,
            elapsed_ms: 1,
            samples: 2,
            predicates: vec![PredicateOutcome {
                index: 0,
                status: VerificationStatus::Satisfied,
                unknown_reason: None,
                observed_json: None,
            }],
        };
        assert!(
            satisfied.validate().is_ok(),
            "published output schema must accept runtime null optionals"
        );
    }
}
