// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Canonical, platform-neutral contract declarations for cua-driver clients.
//!
//! This crate deliberately contains no transport or platform implementation.
//! The native driver remains the only execution engine; this package owns the
//! typed inputs/results and versioned declarations used by the live runtime
//! and to generate experimental client SDKs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

pub mod compatibility;
pub mod cursor;
mod cursor_tools;
mod desktop;
mod inputs;
mod outputs;
mod session;
mod verification;

pub use cursor::{
    classify_cursor_semantics, CursorAction, CursorDelivery, CursorPlayback, CursorReducedMotion,
    CursorSemantics, CursorTarget, CursorThemeSelection,
};
pub use inputs::{
    CaptureScope, ClickButton, ClickInput, ClipboardReadInput, ClipboardWriteInput, DesktopScope,
    DragInput, EndSessionInput, EscalateSessionInput, EscalationReason, GetAgentCursorStateInput,
    GetCursorPositionInput, GetDesktopStateInput, GetScreenSizeInput, GetSessionStateInput,
    HotkeyInput, InvokeMenuInput, MoveCursorInput, PressKeyInput, ScrollBy, ScrollDirection,
    ScrollInput, SetAgentCursorEnabledInput, SetAgentCursorMotionInput, SetAgentCursorThemeInput,
    SetWindowFrameInput, StartSessionInput, ToolInput, TypeTextInput,
};
pub use outputs::{
    ActionDelivery, ActionDeliveryMode, ActionEffect, ActionEscalation, ActionEscalationReason,
    ActionEscalationTarget, ActionEvidence, ActionEvidenceKind, ActionResult,
    ActionResultValidationError, ActionRoute, ClipboardReadOutput, ClipboardWriteOutput,
    CursorMotionOutput, CursorPointOutput, CursorPositionOutput, CursorThemeOutput,
    CursorVisualOutput, DesktopStateOutput, EffectiveScope, EndSessionOutput,
    GetAgentCursorStateOutput, ScreenSizeOutput, SessionStateOutput, SetAgentCursorEnabledOutput,
    SetAgentCursorMotionOutput, SetAgentCursorThemeOutput, StartSessionOutput, ToolOutput,
};
pub use verification::{
    BoundsExpectation, ElementPredicate, ElementSelector, PredicateOutcome, StatePredicate,
    UnknownReason, VerificationStatus, VerifyStateInput, VerifyStateOutput, WindowPredicate,
    VERIFY_STATE_DEFAULT_TIMEOUT_MS,
};

/// Shape version for the MCP `tools/list` result emitted by cua-driver.
pub const TOOLS_LIST_SCHEMA_VERSION: &str = "1";

/// Version of the additive capability-token vocabulary.
pub const CAPABILITY_VERSION: &str = "1";

/// Shape version for the checked-in generated client contract.
pub const CONTRACT_VERSION: &str = "0.6.0";

/// MCP protocol version used by current cua-driver clients.
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
    pub mcp_protocol_version: String,
    pub transport: String,
    pub tools: Vec<ToolContract>,
}

pub fn manifest() -> ContractManifest {
    let mut tools = session::contracts();
    tools.extend(desktop::contracts());
    tools.extend(cursor_tools::contracts());
    tools.extend(verification::contracts());
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
                (
                    tool.name,
                    ToolIndexEntry {
                        capabilities: tool.capabilities,
                        input_fields,
                        output_validator: tool.output_validator,
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
    if name == "list_windows" {
        return Some(desktop::list_windows_success_output_schema());
    }
    tool_contract(name).and_then(|contract| contract.success_output_schema)
}

/// Validate a successful structured payload against the Rust output type that
/// also generates its SDK schema. Returns `Ok(false)` for non-SDK tools.
pub fn validate_success_output(name: &str, value: Value) -> Result<bool, String> {
    if is_action_result_tool(name) {
        validate_typed_output::<ActionResult>(value)?;
        return Ok(true);
    }
    if name == "list_windows" {
        desktop::validate_list_windows_output(value)?;
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
        assert_eq!(manifest.contract_version, "0.6.0");
        assert!(manifest.experimental);
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
                ["properties"]["exists"]["enum"],
            serde_json::json!([true])
        );
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
        assert!(tool_contract("list_windows").is_none());
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
                        {"z_index": 4, "platform_field": true},
                        {"z_index": null}
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
}
