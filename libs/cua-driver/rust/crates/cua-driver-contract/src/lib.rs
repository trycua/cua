// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Canonical, platform-neutral contract declarations for cua-driver clients.
//!
//! This crate deliberately contains no transport or platform implementation.
//! The native driver remains the only execution engine; this package owns the
//! typed inputs/results and versioned declarations used by the live runtime
//! and to generate experimental client SDKs.

use schemars::{generate::SchemaSettings, transform::RecursiveTransform, Schema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub mod compatibility;
mod config_value;
pub mod cursor;
mod cursor_tools;
mod desktop;
mod inputs;
mod outputs;
mod session;
mod verification;
mod visual;
mod windows;
pub use config_value::coerce_set_config_value;
pub use inputs::json_schema_validation_form;
pub use windows::*;

pub(crate) fn schema_settings() -> SchemaSettings {
    SchemaSettings::draft2020_12()
        .with(|settings| {
            settings.meta_schema = None;
            settings.inline_subschemas = true;
        })
        .with_transform(RecursiveTransform(drop_schemars_numeric_format))
}

fn drop_schemars_numeric_format(schema: &mut Schema) {
    if matches!(
        schema.get("format").and_then(Value::as_str),
        Some("uint32" | "uint64" | "double")
    ) {
        schema.remove("format");
    }
}

pub use cursor::{
    classify_cursor_semantics, CursorAction, CursorDelivery, CursorMotionEffects,
    CursorMotionEffectsOutput, CursorMotionStyle, CursorMotionTiming, CursorPlayback,
    CursorReducedMotion, CursorSemantics, CursorTarget, CursorThemeSelection,
};
pub use inputs::{
    action_target_schema, ActionTarget, CaptureScope, ClickButton, ClickInput, ClickPosition,
    ClipboardReadInput, ClipboardWriteInput, CursorMotionSelection, DesktopScope, DragInput,
    EndSessionInput, EscalateSessionInput, EscalationReason, GetAgentCursorStateInput,
    GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput, GetSessionInput,
    GetSessionStateInput, HotkeyInput, InputDeliveryMode, InvokeMenuInput, LegacyClickInput,
    ListSessionsInput, MoveCursorInput, PressKeyInput, ScrollBy, ScrollDirection, ScrollInput,
    SetAgentCursorEnabledInput, SetAgentCursorMotionInput, SetAgentCursorThemeInput,
    SetWindowFrameInput, StartSessionInput, ToolInput, TypeTextInput,
    MULTI_CALL_SESSION_DESCRIPTION,
};
pub use outputs::{
    advertised_output_schema, conforming_error_envelope, is_refusal_envelope,
    refusal_envelope_schema, ActionDelivery, ActionDeliveryMode, ActionEffect, ActionError,
    ActionEscalation, ActionEscalationReason, ActionEscalationTarget, ActionEvidence,
    ActionEvidenceKind, ActionResult, ActionResultValidationError, ActionRoute,
    AgentOverlayCapture, AgentOverlayCaptureStatus, ClipboardReadOutput, ClipboardWriteOutput,
    CursorMotionOutput, CursorPointOutput, CursorPositionOutput, CursorThemeOutput,
    CursorVisualOutput, DesktopStateOutput, EffectiveScope, EndSessionOutput,
    GetAgentCursorStateOutput, ListSessionsOutput, ScreenSizeOutput, SessionClientKindOutput,
    SessionLifecycleState, SessionOutput, SessionStateOutput, SessionTransportOutput,
    SetAgentCursorEnabledOutput, SetAgentCursorMotionOutput, SetAgentCursorThemeOutput,
    StartSessionOutput, ToolOutput, TOOL_INVOCATION_FAILED_CODE,
};
pub use verification::{
    BoundsExpectation, ElementPredicate, ElementSelector, PredicateOutcome, StatePredicate,
    UnknownReason, VerificationStatus, VerifyStateInput, VerifyStateOutput, WindowPredicate,
    VERIFY_STATE_DEFAULT_TIMEOUT_MS,
};
pub use visual::{
    ParseVisualRegionsInput, ParseVisualRegionsOptions, ParseVisualRegionsOutput,
    VisualActionCoordinateSpace, VisualCaptureProvenance, VisualCaptureSource,
    VisualContractValidationError, VisualParseError, VisualParseErrorCode, VisualParseTiming,
    VisualParseWarning, VisualParserMetadata, VisualRegion, VisualRegionBounds, VisualRegionKind,
    VisualScreenshotReference, VISUAL_REGIONS_SCHEMA,
};

/// Shape version for the MCP `tools/list` result emitted by cua-driver.
pub const TOOLS_LIST_SCHEMA_VERSION: &str = "1";

/// Version of the additive capability-token vocabulary.
pub const CAPABILITY_VERSION: &str = "1";

/// Shape version for the checked-in generated client contract.
pub const CONTRACT_VERSION: &str = "0.8.0";

/// Legacy version negotiated by `initialize.params.protocolVersion` and served
/// by the loopback HTTP compatibility endpoint. Modern stdio discovery and
/// per-request negotiation are defined by the endpoint implementation.
pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";

/// Tools whose successful result is the shared closed [`ActionResult`].
///
/// Keep this vocabulary in the contract crate so MCP schema advertising,
/// runtime validation, SDK normalization, and the execution seam cannot drift.
pub const ACTION_RESULT_TOOLS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "scroll",
    "drag",
    "mouse_drag",
    "parallel_mouse_drag",
    "move_cursor",
    "mouse_button_down",
    "mouse_button_up",
    "type_text",
    "type_text_chars",
    "press_key",
    "hotkey",
    "set_value",
    "set_window_frame",
    "invoke_menu",
    "browser_click",
    "browser_pointer",
    "browser_type",
];

pub fn is_action_result_tool(name: &str) -> bool {
    ACTION_RESULT_TOOLS.contains(&name)
}

#[derive(
    Debug,
    Clone,
    Copy,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    uniffi::Enum,
)]
#[serde(rename_all = "snake_case")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

uniffi::setup_scaffolding!("cua_driver_contract");

/// Whether a client contract is also the canonical live MCP schema, or a
/// deliberately narrower cross-platform subset accepted by every backend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SchemaMode {
    CanonicalRuntime,
    PortableSubset,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolAnnotations {
    pub read_only: bool,
    pub destructive: bool,
    pub idempotent: bool,
    pub open_world: bool,
}

pub type OutputValidator = fn(Value) -> Result<(), String>;

fn default_output_validator() -> OutputValidator {
    |_| Ok(())
}

pub(crate) fn validate_typed_output<T: ToolOutput>(value: Value) -> Result<(), String> {
    let output = serde_json::from_value::<T>(value).map_err(|error| error.to_string())?;
    output.validate()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContract {
    pub name: String,
    pub description: String,
    pub platforms: Vec<Platform>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
    pub capabilities: Vec<String>,
    pub annotations: ToolAnnotations,
    pub schema_mode: SchemaMode,
    /// Best-effort agent-cursor cue. The live classifier may refine target
    /// and delivery from resolved invocation arguments.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor_semantics: Option<CursorSemantics>,
    pub input_schema: Value,
    /// Schema for successful `structuredContent`. The live MCP surface
    /// advertises this as `outputSchema`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub success_output_schema: Option<Value>,
    /// Stable tool-specific structured error payload, when the tool exposes
    /// one in addition to the shared transport refusal envelope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_output_schema: Option<Value>,
    /// Runtime-only validator bound to the same Rust output type that produced
    /// `success_output_schema`; omitted from the generated manifest.
    #[serde(skip, default = "default_output_validator")]
    pub(crate) output_validator: OutputValidator,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContractManifest {
    pub generated_notice: String,
    pub experimental: bool,
    pub contract_version: String,
    pub tools_list_schema_version: String,
    pub capability_version: String,
    /// Legacy initialize/HTTP compatibility version. This field retains its
    /// historical name so generated SDK manifests remain backward compatible.
    pub mcp_protocol_version: String,
    pub transport: String,
    pub tools: Vec<ToolContract>,
}

pub fn manifest() -> ContractManifest {
    let mut tools = session::contracts();
    tools.extend(desktop::contracts());
    tools.extend(cursor_tools::contracts());
    tools.extend(verification::contracts());
    tools.extend(visual::contracts());
    tools.sort_by(|left, right| left.name.cmp(&right.name));
    ContractManifest {
        generated_notice: "Generated by cua-contract-gen; do not edit by hand.".into(),
        experimental: true,
        contract_version: CONTRACT_VERSION.into(),
        tools_list_schema_version: TOOLS_LIST_SCHEMA_VERSION.into(),
        capability_version: CAPABILITY_VERSION.into(),
        mcp_protocol_version: MCP_PROTOCOL_VERSION.into(),
        transport: "mcp_stdio".into(),
        tools,
    }
}

pub fn tool_contract(name: &str) -> Option<ToolContract> {
    manifest().tools.into_iter().find(|tool| tool.name == name)
}

#[derive(Debug)]
struct ToolIndexEntry {
    capabilities: Vec<String>,
    input_fields: BTreeSet<String>,
    output_validator: OutputValidator,
    advertises_output_schema: bool,
}

fn tool_index() -> &'static BTreeMap<String, ToolIndexEntry> {
    static INDEX: OnceLock<BTreeMap<String, ToolIndexEntry>> = OnceLock::new();
    INDEX.get_or_init(|| {
        manifest()
            .tools
            .into_iter()
            .map(|tool| {
                let input_fields = tool
                    .input_schema
                    .get("properties")
                    .and_then(Value::as_object)
                    .into_iter()
                    .flatten()
                    .map(|(name, _)| name.clone())
                    .collect();
                let advertises_output_schema = tool.success_output_schema.is_some();
                (
                    tool.name,
                    ToolIndexEntry {
                        capabilities: tool.capabilities,
                        input_fields,
                        output_validator: tool.output_validator,
                        advertises_output_schema,
                    },
                )
            })
            .collect()
    })
}

/// Return capability tokens for one published tool without rebuilding schemas
/// on every live `tools/list` entry.
pub fn tool_capabilities(name: &str) -> Option<Vec<String>> {
    tool_index()
        .get(name)
        .map(|entry| entry.capabilities.clone())
}

/// Names owned by a published portable input projection.
pub fn tool_input_fields(name: &str) -> Option<&'static BTreeSet<String>> {
    tool_index().get(name).map(|entry| &entry.input_fields)
}

/// Return the successful `structuredContent` schema advertised by the live
/// MCP tool. Runtime-only tools can define a narrow shared schema here without
/// committing every generated SDK to their broader platform-specific shape.
pub fn tool_success_output_schema(name: &str) -> Option<Value> {
    tool_contract(name).and_then(|contract| contract.success_output_schema)
}

/// The `outputSchema` one tool advertises on `tools/list`, or `None` when it
/// advertises none.
///
/// Action tools answer with the shared `ActionResult` shape; everything else
/// uses its own success schema when it has one. Both are wrapped by
/// [`advertised_output_schema`] so the refusal arm rides along, because MCP
/// holds every `structuredContent` a tool emits — refusals included — to this
/// schema.
pub fn advertised_tool_output_schema(name: &str) -> Option<Value> {
    let success = if is_action_result_tool(name) {
        Some(<ActionResult as ToolOutput>::output_schema())
    } else {
        tool_success_output_schema(name)
    };
    success.map(advertised_output_schema)
}

/// Whether [`advertised_tool_output_schema`] would answer with a schema.
///
/// The `tools/call` boundary asks this once per call, so it reads the cached
/// tool index instead of rebuilding the manifest and its schemas.
/// `advertised_schema_presence_matches_the_cheap_predicate` pins the two
/// against each other across the whole manifest.
pub fn advertises_output_schema(name: &str) -> bool {
    is_action_result_tool(name)
        || name == "list_windows"
        || tool_index()
            .get(name)
            .is_some_and(|entry| entry.advertises_output_schema)
}

/// Validate a successful structured payload against the Rust output type that
/// also generates its SDK schema. Returns `Ok(false)` for non-SDK tools.
pub fn validate_success_output(name: &str, value: Value) -> Result<bool, String> {
    if is_action_result_tool(name) {
        validate_typed_output::<ActionResult>(value)?;
        return Ok(true);
    }
    if let Some(entry) = tool_index().get(name) {
        (entry.output_validator)(value)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize)]
    struct NumericFormatFixture;

    impl schemars::JsonSchema for NumericFormatFixture {
        fn schema_name() -> std::borrow::Cow<'static, str> {
            "NumericFormatFixture".into()
        }

        fn json_schema(_: &mut schemars::SchemaGenerator) -> Schema {
            schemars::json_schema!({
                "type": "object",
                "properties": {
                    "count": {"type": "integer", "format": "uint32"},
                    "frame": {"type": "integer", "format": "uint64"},
                    "ratio": {"type": "number", "format": "double"},
                    "created_at": {"type": "string", "format": "date-time"},
                    "format": {"type": "string"},
                    "annotation": {
                        "const": {"format": "uint32"},
                        "default": {"format": "uint64"},
                        "examples": [{"format": "double"}]
                    }
                },
                "required": ["format"]
            })
        }
    }

    impl ToolInput for NumericFormatFixture {
        const TOOL_NAME: &'static str = "numeric_format_fixture";
    }

    impl ToolOutput for NumericFormatFixture {}

    #[test]
    fn input_and_output_schemas_drop_only_schemars_numeric_formats() {
        let schemas = [
            <NumericFormatFixture as ToolInput>::input_schema(),
            <NumericFormatFixture as ToolOutput>::output_schema(),
        ];

        for (index, schema) in schemas.into_iter().enumerate() {
            for property in ["count", "frame", "ratio"] {
                assert!(schema["properties"][property].get("format").is_none());
            }
            assert_eq!(schema["properties"]["created_at"]["format"], "date-time");
            assert_eq!(
                schema["properties"]["format"],
                serde_json::json!({"type": "string"})
            );
            assert_eq!(schema["required"], serde_json::json!(["format"]));
            // Input schemas rewrite `const` into a one-value `enum` for
            // Vertex/Gemini (#4798); the literal payload must survive intact.
            let literal = if index == 0 {
                &schema["properties"]["annotation"]["enum"][0]
            } else {
                &schema["properties"]["annotation"]["const"]
            };
            assert_eq!(literal, &serde_json::json!({"format": "uint32"}));
            assert_eq!(
                schema["properties"]["annotation"]["default"],
                serde_json::json!({"format": "uint64"})
            );
            assert_eq!(
                schema["properties"]["annotation"]["examples"],
                serde_json::json!([{"format": "double"}])
            );
        }
    }

    #[test]
    fn manifest_is_sorted_and_versioned() {
        let manifest = manifest();
        let names: Vec<_> = manifest
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        assert_eq!(names, sorted);
        assert_eq!(manifest.contract_version, "0.8.0");
        assert!(manifest.experimental);
    }

    /// The `tools/call` boundary decides whether a client will validate a
    /// payload from the cheap predicate, so a tool it disagrees with on would
    /// be checked against a schema it never advertises, or skipped while a
    /// client still validates it.
    #[test]
    fn advertised_schema_presence_matches_the_cheap_predicate() {
        let mut names: Vec<String> = manifest().tools.into_iter().map(|tool| tool.name).collect();
        names.extend(ACTION_RESULT_TOOLS.iter().map(|name| (*name).to_owned()));
        names.push("list_windows".to_owned());
        names.push("no_such_tool".to_owned());

        for name in names {
            assert_eq!(
                advertised_tool_output_schema(&name).is_some(),
                advertises_output_schema(&name),
                "`{name}` disagrees on whether it advertises an output schema"
            );
        }
    }

    #[test]
    fn every_action_result_tool_uses_the_closed_runtime_validator() {
        let valid = serde_json::json!({
            "effect": "unverifiable",
            "route": "synthetic_events"
        });
        let legacy = serde_json::json!({
            "path": "ax",
            "verified": true
        });

        for name in ACTION_RESULT_TOOLS {
            assert_eq!(
                validate_success_output(name, valid.clone()),
                Ok(true),
                "{name} must accept the shared ActionResult"
            );
            assert!(
                validate_success_output(name, legacy.clone()).is_err(),
                "{name} must reject a legacy action payload"
            );
        }
    }

    #[test]
    fn verify_state_is_a_portable_read_only_contract() {
        let contract = tool_contract("verify_state").expect("verify_state contract");
        assert_eq!(contract.schema_mode, SchemaMode::PortableSubset);
        assert!(contract.annotations.read_only);
        assert!(!contract.annotations.destructive);
        assert!(contract
            .capabilities
            .iter()
            .any(|capability| capability == "state.verify"));
        assert_eq!(
            contract.input_schema["required"],
            serde_json::json!(["pid", "window_id", "expect"])
        );
        assert_eq!(
            contract.input_schema["properties"]["expect"]["items"]["properties"]["element"]
                ["properties"]["exists"]["type"],
            serde_json::json!("boolean")
        );
        assert!(
            contract.input_schema["properties"]["expect"]["items"]["properties"]["element"]
                ["properties"]["exists"]
                .get("enum")
                .is_none(),
            "verify_state element.exists should not specify enum (must remain plain boolean for Gemini function-calling compatibility)"
        );
    }

    #[test]
    fn portable_action_sessions_explain_named_multi_call_runs() {
        for name in [
            "click",
            "clipboard_read",
            "clipboard_write",
            "drag",
            "get_cursor_position",
            "get_desktop_state",
            "get_screen_size",
            "hotkey",
            "invoke_menu",
            "move_cursor",
            "press_key",
            "scroll",
            "set_window_frame",
            "type_text",
        ] {
            let contract = tool_contract(name).expect("portable action contract");
            let description = contract.input_schema["properties"]["session"]["description"]
                .as_str()
                .expect("portable session description")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            assert_eq!(description, MULTI_CALL_SESSION_DESCRIPTION, "{name}");
        }

        let verify = tool_contract("verify_state").expect("verify_state contract");
        let description = verify.input_schema["properties"]["session"]["description"]
            .as_str()
            .expect("verify_state session description")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(description.starts_with(MULTI_CALL_SESSION_DESCRIPTION));
        assert!(description.contains("never selects capture modality or authorization"));

        assert!(MULTI_CALL_SESSION_DESCRIPTION.contains("prefer a short public session label"));
        assert!(MULTI_CALL_SESSION_DESCRIPTION.contains("repeat it on every call that accepts it"));
        assert!(MULTI_CALL_SESSION_DESCRIPTION.contains("implicit lifecycle session"));
    }

    #[test]
    fn every_contract_has_success_schema_and_platforms() {
        for tool in manifest().tools {
            assert!(!tool.platforms.is_empty(), "{} has no platform", tool.name);
            assert!(
                tool.success_output_schema.is_some(),
                "{} has no success output schema",
                tool.name
            );
        }
    }

    #[test]
    fn delivery_mode_capability_matches_the_typed_input_schema() {
        for tool in manifest().tools {
            assert_eq!(
                tool.input_schema
                    .pointer("/properties/delivery_mode")
                    .is_some(),
                tool.capabilities
                    .iter()
                    .any(|capability| capability == "input.delivery_mode"),
                "{} typed contract delivery_mode schema/capability mismatch",
                tool.name
            );
        }
    }

    #[test]
    fn desktop_contracts_are_explicit_portable_subsets() {
        for name in [
            "click",
            "drag",
            "get_cursor_position",
            "get_desktop_state",
            "get_screen_size",
            "hotkey",
            "invoke_menu",
            "move_cursor",
            "press_key",
            "scroll",
            "set_window_frame",
            "type_text",
        ] {
            assert_eq!(
                tool_contract(name).expect("desktop contract").schema_mode,
                SchemaMode::PortableSubset,
                "{name}"
            );
        }
    }

    #[test]
    fn set_window_frame_is_exact_targeted_and_positive_sized() {
        let contract = tool_contract("set_window_frame").expect("set_window_frame contract");
        assert_eq!(
            contract.input_schema["required"],
            serde_json::json!(["pid", "window_id", "x", "y", "width", "height"])
        );
        assert_eq!(
            contract.input_schema["properties"]["pid"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            contract.input_schema["properties"]["window_id"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            contract.input_schema["properties"]["width"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            contract.input_schema["properties"]["height"]["minimum"],
            serde_json::json!(1)
        );
        assert_eq!(
            contract.cursor_semantics.expect("cursor semantics").action,
            CursorAction::App
        );
    }

    #[test]
    fn invoke_menu_requires_an_exact_nonempty_bounded_path() {
        let contract = tool_contract("invoke_menu").expect("invoke_menu contract");
        assert_eq!(
            contract.input_schema["required"],
            serde_json::json!(["pid", "window_id", "path"])
        );
        let path = &contract.input_schema["properties"]["path"];
        assert_eq!(path["minItems"], serde_json::json!(1));
        assert_eq!(path["maxItems"], serde_json::json!(16));
        assert_eq!(path["items"]["minLength"], serde_json::json!(1));
        assert_eq!(
            contract.cursor_semantics.expect("cursor semantics").action,
            CursorAction::App
        );
        assert_eq!(
            contract.capabilities,
            vec!["menu.path.invoke", "accessibility.menu.native"]
        );
    }

    #[test]
    fn list_windows_defines_nullable_higher_is_frontmost_z_index() {
        assert!(tool_contract("list_windows").is_some());
        let schema = tool_success_output_schema("list_windows").expect("runtime schema");
        let z_index = &schema["properties"]["windows"]["items"]["properties"]["z_index"];
        assert_eq!(z_index["type"], serde_json::json!(["integer", "null"]));
        let description = z_index["description"].as_str().expect("description");
        assert!(description.contains("Higher values are closer to the front"));
        assert!(description.contains("must not infer an order"));

        assert_eq!(
            validate_success_output(
                "list_windows",
                serde_json::json!({
                    "windows": [
                        {"window_id":1,"pid":2,"app_name":"Example","title":"Doc","bounds":{"x":0,"y":0,"width":10,"height":10},"is_on_screen":true,"z_index":null,"platform_field":true}
                    ],
                    "current_space_id": null
                }),
            ),
            Ok(true)
        );
        assert!(validate_success_output(
            "list_windows",
            serde_json::json!({"windows": [{"z_index": "unknown"}]}),
        )
        .is_err());
    }

    #[test]
    fn desktop_action_contracts_share_the_strict_action_result() {
        let expected = ActionResult::output_schema();
        for name in [
            "click",
            "drag",
            "hotkey",
            "move_cursor",
            "press_key",
            "scroll",
            "type_text",
        ] {
            let contract = tool_contract(name).expect("desktop action contract");
            assert_eq!(
                contract.success_output_schema,
                Some(expected.clone()),
                "{name}"
            );
            assert_eq!(
                contract.success_output_schema.as_ref().expect("schema")["additionalProperties"],
                false,
                "{name}"
            );
        }
    }

    #[test]
    fn session_success_schemas_require_every_typed_field() {
        let start = tool_contract("start_session").expect("start_session contract");
        let success_schema = start.success_output_schema.expect("success schema");
        let required = success_schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>();
        assert_eq!(
            required,
            [
                "session",
                "capture_scope",
                "effective_scope",
                "desktop_capture_authorized",
                "desktop_unlocked",
                "escalation_reason",
                "escalation_detail",
                "active",
                "revived",
            ]
        );
    }

    #[test]
    fn typed_success_outputs_accept_extensions_and_reject_wrong_common_fields() {
        assert_eq!(
            validate_success_output(
                "get_screen_size",
                serde_json::json!({
                    "width": 1512,
                    "height": 982,
                    "scale_factor": 2,
                    "backend": "core_graphics"
                }),
            ),
            Ok(true)
        );
        assert!(validate_success_output(
            "get_screen_size",
            serde_json::json!({
                "width": "1512",
                "height": 982,
                "scale_factor": 2
            }),
        )
        .is_err());
        assert!(validate_success_output(
            "end_session",
            serde_json::json!({"session": "run", "active": true}),
        )
        .is_err());
    }

    #[test]
    fn every_published_tool_has_a_live_typed_output_validator() {
        for contract in manifest().tools {
            let invalid = serde_json::json!([]);
            assert!(
                validate_success_output(&contract.name, invalid).is_err(),
                "{} has no typed output validator",
                contract.name
            );
        }
    }

    /// Fields of the Vertex AI `Schema` object, the OpenAPI 3.0 subset used
    /// for function declaration parameters:
    /// https://cloud.google.com/vertex-ai/docs/reference/rest/v1/Schema
    /// (`ref`/`defs` are written `$ref`/`$defs` in JSON Schema form, as the
    /// function-calling guide shows). Any other keyword, such as `oneOf`,
    /// `allOf`, `not`, `const` or `uniqueItems`, is not part of the object.
    const VERTEX_SCHEMA_FIELDS: &[&str] = &[
        "type",
        "format",
        "title",
        "description",
        "nullable",
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
        "$ref",
        "$defs",
    ];

    /// `Schema.type` values (`TYPE_UNSPECIFIED` excluded), in JSON Schema case.
    const VERTEX_SCHEMA_TYPES: &[&str] = &[
        "string", "number", "integer", "boolean", "array", "object", "null",
    ];

    /// Collect every node of a published input schema that falls outside the
    /// Vertex AI / Gemini function-declaration `Schema` object (#4798,
    /// following #4220). Walks schema positions only, so property names and
    /// `default`/`example`/`enum` payloads are never mistaken for keywords.
    fn vertex_input_schema_violations(schema: &Value, path: &str, out: &mut Vec<String>) {
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
            // Vertex answers "schema didn't specify the schema type field"
            // for an untyped node, including an anyOf without a type (#4798).
            None if !node.contains_key("$ref") => {
                out.push(format!("{path}: schema node has no type"))
            }
            None => {}
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
                    vertex_input_schema_violations(child, &format!("{path}.{keyword}.{name}"), out);
                }
            }
        }
        if let Some(items) = node.get("items") {
            vertex_input_schema_violations(items, &format!("{path}.items"), out);
        }
        if let Some(additional) = node.get("additionalProperties") {
            if !additional.is_boolean() {
                vertex_input_schema_violations(
                    additional,
                    &format!("{path}.additionalProperties"),
                    out,
                );
            }
        }
        if let Some(variants) = node.get("anyOf").and_then(Value::as_array) {
            for (index, variant) in variants.iter().enumerate() {
                vertex_input_schema_violations(variant, &format!("{path}.anyOf[{index}]"), out);
            }
        }
    }

    #[test]
    fn vertex_lint_follows_the_documented_schema_fields() {
        let lint = |schema: Value| {
            let mut out = Vec::new();
            vertex_input_schema_violations(&schema, "$", &mut out);
            out
        };
        // Documented fields pass, including a typed anyOf, nullable, bounds,
        // additionalProperties and a property literally named `const`.
        let accepted = lint(serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "required": ["value"],
            "properties": {
                "value": {
                    "type": "string",
                    "anyOf": [{"type": "string", "minLength": 1}, {"type": "string", "enum": ["x"]}]
                },
                "limit": {"type": "integer", "nullable": true, "minimum": 0, "maximum": 9},
                "tags": {"type": "array", "minItems": 1, "maxItems": 2, "items": {"type": "string"}},
                "const": {"type": "string", "default": {"oneOf": "payload, not a keyword"}}
            }
        }));
        assert!(accepted.is_empty(), "{accepted:#?}");

        // Keywords outside the Schema object, untyped nodes, type arrays and
        // non-string enums are flagged.
        let rejected = lint(serde_json::json!({
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
        for contract in manifest().tools {
            vertex_input_schema_violations(
                &contract.input_schema,
                &format!("{}.input_schema", contract.name),
                &mut violations,
            );
        }
        assert!(
            violations.is_empty(),
            "input schema nodes outside the Vertex AI Schema object (#4798):\n{}",
            violations.join("\n")
        );
    }

    #[test]
    fn action_target_advertises_a_flat_object_for_vertex_clients() {
        let target = action_target_schema();
        assert_eq!(target["type"], "object", "{target}");
        assert!(target.get("anyOf").is_none(), "{target}");
        assert!(target.get("oneOf").is_none(), "{target}");
        assert_eq!(
            target["properties"]["kind"]["enum"],
            serde_json::json!(["window", "desktop"])
        );
        assert_eq!(target["properties"]["kind"]["type"], "string");
        for tool in [
            "drag",
            "hotkey",
            "move_cursor",
            "press_key",
            "scroll",
            "type_text",
            "click",
        ] {
            let contract = tool_contract(tool).unwrap_or_else(|| panic!("{tool} contract"));
            let schema = &contract.input_schema["properties"]["target"];
            assert_eq!(schema["type"], "object", "{tool}: {schema}");
            assert!(schema.get("anyOf").is_none(), "{tool}: {schema}");
            assert!(schema.get("oneOf").is_none(), "{tool}: {schema}");
            assert!(
                schema.pointer("/properties/kind/const").is_none(),
                "{tool} kind must not use const: {schema}"
            );
        }
    }

    /// `set_config.value` is advertised as a string so Vertex/Gemini accept
    /// it, so the driver must parse string values for typed keys (#4798).
    #[test]
    fn set_config_value_strings_are_coerced_to_the_key_type() {
        use serde_json::json;
        for (key, value, expected) in [
            ("max_image_dimension", json!("800"), json!(800)),
            ("max_image_dimension", json!(" 0 "), json!(0)),
            ("experimental_pip", json!("true"), json!(true)),
            ("experimental_pip", json!("false"), json!(false)),
            ("cursor.motion.effects.glow", json!("true"), json!(true)),
            ("cursor.motion.effects.trail", json!("false"), json!(false)),
        ] {
            assert_eq!(
                coerce_set_config_value(key, &value),
                expected,
                "{key} = {value}"
            );
        }
    }

    #[test]
    fn set_config_value_coercion_leaves_other_values_alone() {
        use serde_json::json;
        for (key, value) in [
            // Already typed: unchanged.
            ("max_image_dimension", json!(800)),
            ("experimental_pip", json!(true)),
            // Unparseable: unchanged, so the key's own type error still fires.
            ("max_image_dimension", json!("800px")),
            ("max_image_dimension", json!("-1")),
            ("experimental_pip", json!("yes")),
            // String-valued and reset values are not reinterpreted.
            ("capture_mode", json!("vision")),
            ("experimental_pip_geometry", json!("640x480")),
            ("cursor.motion.style", json!("signature_arc")),
            ("cursor.motion.effects.glow", json!("default")),
            ("cursor.motion", Value::Null),
            ("unknown_key", json!("800")),
        ] {
            assert_eq!(
                coerce_set_config_value(key, &value),
                value,
                "{key} = {value}"
            );
        }
    }

    /// Fields that used to advertise `T | null` keep explicit `null` through
    /// the OpenAPI `nullable` flag Vertex/Gemini document, instead of a type
    /// array (#4798). `effects.glow: null` restores the style's default.
    #[test]
    fn nullable_inputs_advertise_openapi_nullable() {
        for (tool, pointer, ty) in [
            (
                "set_agent_cursor_motion",
                "/properties/effects/properties/glow",
                "boolean",
            ),
            ("set_agent_cursor_motion", "/properties/effects", "object"),
            ("set_agent_cursor_motion", "/properties/arc_flow", "number"),
            (
                "start_session",
                "/properties/cursor_motion/properties/effects/properties/trail",
                "boolean",
            ),
            ("start_session", "/properties/cursor_theme", "object"),
            ("list_sessions", "/properties/limit", "integer"),
            ("verify_state", "/properties/include_screenshot", "boolean"),
            (
                "parse_visual_regions",
                "/properties/options/properties/kinds",
                "array",
            ),
            ("drag", "/properties/target", "object"),
        ] {
            let contract = tool_contract(tool).unwrap_or_else(|| panic!("{tool} contract"));
            let field = contract
                .input_schema
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{tool}{pointer} missing"));
            assert_eq!(field["type"], ty, "{tool}{pointer}: {field}");
            assert_eq!(field["nullable"], true, "{tool}{pointer}: {field}");
        }
    }

    /// run_actions checks each step against the advertised schema with a JSON
    /// Schema engine, which ignores OpenAPI `nullable`. The validation form
    /// must turn the flag back into a `null` type so explicit null still
    /// passes the step check (#4798).
    #[test]
    fn run_actions_validation_form_accepts_explicit_null() {
        use serde_json::json;

        let validator_for = |tool: &str| {
            let schema = tool_contract(tool)
                .unwrap_or_else(|| panic!("{tool} contract"))
                .input_schema;
            jsonschema::validator_for(&json_schema_validation_form(&schema))
                .unwrap_or_else(|error| panic!("{tool}: {error}"))
        };

        let motion = validator_for("set_agent_cursor_motion");
        for args in [
            json!({"session": "s", "effects": {"glow": null}}),
            json!({"session": "s", "effects": {"glow": true}}),
            json!({"session": "s", "effects": null}),
            json!({"session": "s", "arc_flow": null}),
        ] {
            assert!(motion.is_valid(&args), "{args}");
        }
        for args in [
            json!({"session": "s", "effects": {"glow": "on"}}),
            json!({"session": "s", "arc_flow": "1"}),
        ] {
            assert!(!motion.is_valid(&args), "{args}");
        }

        let parse = validator_for("parse_visual_regions");
        assert!(parse.is_valid(&json!({"capture_id": "c", "options": {"kinds": null}})));
        assert!(parse.is_valid(&json!({"capture_id": "c", "options": {"kinds": ["text"]}})));
        assert!(!parse.is_valid(&json!({"capture_id": "c", "options": {"kinds": ["logo"]}})));

        // A nullable enum admits null alongside its listed values.
        let fixture = json!({"type": "string", "enum": ["a"], "nullable": true});
        let enum_validator = jsonschema::validator_for(&json_schema_validation_form(&fixture))
            .expect("fixture compiles");
        assert!(enum_validator.is_valid(&json!("a")));
        assert!(enum_validator.is_valid(&Value::Null));
        assert!(!enum_validator.is_valid(&json!("b")));
    }
}
