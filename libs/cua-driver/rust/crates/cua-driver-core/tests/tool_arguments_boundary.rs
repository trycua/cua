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
        input_schema: json!({"type": "object", "properties": {}}),
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
