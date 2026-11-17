// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Regression coverage for the canonical dispatch boundary's handling of
//! malformed `arguments` values.
//!
//! The MCP contract types `arguments` as an object and `Request::tool_call`
//! substitutes `{}` when it is absent, but a client can still send any JSON
//! value. Found by the `registry_invoke` fuzz target: a non-object value for a
//! session-selecting tool reached an index assignment inside
//! `ToolRegistry::invoke_authorized` and panicked the dispatcher.

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

struct Stub(ToolDef);

#[async_trait::async_trait]
impl Tool for Stub {
    fn def(&self) -> &ToolDef {
        &self.0
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::text("stub")
    }
}

fn unrestricted_context() -> Arc<EffectiveAuthorizationContext> {
    let ceiling = SessionModeCeiling::for_trusted_sessions(
        [PermissionMode::Unrestricted],
        true,
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap();
    SessionAuthorizationRegistry::with_ceiling(ceiling)
        .compatibility_context(PermissionMode::Unrestricted, None)
        .unwrap()
}

fn registry_with(name: &str) -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(Stub(ToolDef {
        name: name.to_owned(),
        description: "stub".to_owned(),
        input_schema: if name == "get_window_state" {
            json!({
                "type": "object",
                "properties": {
                    "pid": {"type": "integer"},
                    "window_id": {"type": "integer"},
                    "max_dimension": {"type": "integer", "minimum": 1},
                    "max_image_dimension": {"type": "integer", "minimum": 0}
                },
                "additionalProperties": false
            })
        } else {
            json!({"type": "object", "properties": {}})
        },
        read_only: true,
        destructive: false,
        idempotent: true,
        open_world: false,
    })));
    let registry = Arc::new(registry);
    registry.init_self_weak();
    registry
}

#[tokio::test]
async fn window_state_rejects_invalid_legacy_screenshot_caps_before_dispatch() {
    let registry = registry_with("get_window_state");
    for invalid in [
        json!(0),
        json!(-1),
        json!(1.5),
        json!(true),
        json!("bad"),
        json!(u64::from(u32::MAX) + 1),
    ] {
        for canonical_override in [None, Some(0)] {
            let mut args = json!({"pid": 42, "window_id": 7, "max_dimension": invalid});
            if let Some(canonical_override) = canonical_override {
                args["max_image_dimension"] = json!(canonical_override);
            }
            let result = registry
                .invoke_with_context("get_window_state", args.clone(), unrestricted_context())
                .await;
            assert_eq!(result.is_error, Some(true), "accepted {args}");
            let refusal = result.structured_content.expect("structured refusal");
            assert_eq!(refusal["code"], "invalid_arguments", "{args}");
            assert_eq!(refusal["tool"], "get_window_state");
            assert!(refusal["detail"]
                .as_str()
                .unwrap()
                .contains("max_dimension"));
        }
    }
}

#[tokio::test]
async fn window_state_accepts_valid_caps_and_native_resolution_override() {
    let registry = registry_with("get_window_state");
    for cap in [
        None,
        Some(json!(null)),
        Some(json!(1)),
        Some(json!(1024)),
        Some(json!(u32::MAX)),
    ] {
        for canonical_override in [None, Some(0), Some(2048)] {
            let mut args = json!({"pid": 42, "window_id": 7});
            if let Some(cap) = &cap {
                args["max_dimension"] = cap.clone();
            }
            if let Some(canonical_override) = canonical_override {
                args["max_image_dimension"] = json!(canonical_override);
            }
            let result = registry
                .invoke_with_context("get_window_state", args.clone(), unrestricted_context())
                .await;
            assert_eq!(
                serde_json::to_value(result).unwrap(),
                serde_json::to_value(ToolResult::text("stub")).unwrap(),
                "valid arguments did not reach the backend: {args}"
            );
        }
    }
}

#[tokio::test]
async fn non_object_arguments_for_a_session_selecting_tool_are_rejected_not_panicked() {
    let registry = registry_with("get_session");
    for arguments in [
        json!(true),
        json!(null),
        json!(7),
        json!("s"),
        json!([1, 2]),
    ] {
        let result = registry
            .invoke_with_context("get_session", arguments.clone(), unrestricted_context())
            .await;
        assert_eq!(
            result.is_error,
            Some(true),
            "arguments {arguments} were accepted"
        );
        let structured = result.structured_content.expect("structured refusal");
        assert_eq!(
            structured["code"], "invalid_arguments",
            "arguments {arguments}"
        );
        assert_eq!(structured["tool"], "get_session");
    }
}

#[tokio::test]
async fn non_object_arguments_for_an_ordinary_tool_are_rejected_before_dispatch() {
    let registry = registry_with("get_screen_size");
    let result = registry
        .invoke_with_context("get_screen_size", json!("nope"), unrestricted_context())
        .await;
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result.structured_content.unwrap()["code"],
        "invalid_arguments"
    );
}

#[tokio::test]
async fn unknown_tool_still_wins_over_malformed_arguments() {
    let registry = registry_with("get_screen_size");
    let result = registry
        .invoke_with_context("no_such_tool", json!(true), unrestricted_context())
        .await;
    assert_eq!(result.is_error, Some(true));
    let text = serde_json::to_string(&result.content).unwrap();
    assert!(text.contains("Unknown tool"), "{text}");
}
