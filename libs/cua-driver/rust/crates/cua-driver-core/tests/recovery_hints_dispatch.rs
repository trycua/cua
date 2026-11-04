//! The dispatch seam gives every refusal and every unconfirmed action a
//! `hint` and, where one exists, a `next_call`, whichever tool produced it.

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A stand-in tool that returns whatever its `script` argument names.
struct Scripted(ToolDef);

#[async_trait::async_trait]
impl Tool for Scripted {
    fn def(&self) -> &ToolDef {
        &self.0
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        match args.get("script").and_then(Value::as_str) {
            Some("stale") => ToolResult::error("element_token is stale")
                .with_structured(json!({"code": "stale_element_token"})),
            Some("producer_hint") => ToolResult::error("refused").with_structured(json!({
                "code": "stale_element_token",
                "hint": "the producer's own words",
                "next_call": {"tool": "list_windows", "arguments": {"pid": 1}},
            })),
            Some("browser_refusal") => ToolResult::text("refused (browser_ref_stale): gone")
                .with_structured(json!({
                    "status": "refused",
                    "refusal": {"code": "browser_ref_stale", "message": "gone"},
                })),
            Some("missing") => ToolResult::error("Missing required parameter: from_x"),
            Some("unverifiable") => ToolResult::text("Posted click").with_structured(json!({
                "path": "cgevent",
                "verified": false,
                "effect": "unverifiable",
            })),
            _ => ToolResult::text("done"),
        }
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

fn registry() -> Arc<ToolRegistry> {
    let mut registry = ToolRegistry::new();
    registry.register_session_tools();
    for name in [
        "click",
        "drag",
        "type_text",
        "get_window_state",
        "get_browser_state",
    ] {
        registry.register(Box::new(Scripted(ToolDef {
            name: name.to_owned(),
            description: "stub".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "script": {"type": "string"},
                    "pid": {"type": "integer"},
                    "window_id": {"type": "integer"},
                    "session": {"type": "string"},
                    "x": {"type": "number"},
                    "y": {"type": "number"},
                },
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        })));
    }
    let registry = Arc::new(registry);
    registry.init_self_weak();
    registry
}

async fn call(tool: &str, args: Value) -> ToolResult {
    registry()
        .invoke_with_context(tool, args, unrestricted_context())
        .await
}

#[tokio::test]
async fn a_refusal_gains_a_hint_and_a_next_call_built_from_the_request() {
    let result = call(
        "click",
        json!({"script": "stale", "pid": 42, "window_id": 7, "session": "run-1"}),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["code"], "stale_element_token");
    assert!(structured["hint"]
        .as_str()
        .unwrap()
        .contains("fresh tokens"));
    assert_eq!(
        structured["next_call"],
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 42, "window_id": 7, "session": "run-1"},
        })
    );
}

#[tokio::test]
async fn a_producers_own_hint_and_next_call_survive_dispatch() {
    let result = call(
        "click",
        json!({"script": "producer_hint", "pid": 42, "window_id": 7}),
    )
    .await;
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["hint"], "the producer's own words");
    assert_eq!(
        structured["next_call"],
        json!({"tool": "list_windows", "arguments": {"pid": 1}})
    );
}

#[tokio::test]
async fn a_browser_style_refusal_that_is_not_an_error_is_annotated_too() {
    // `get_browser_state` refuses without `isError`; agents branch on `status`.
    let result = call("get_browser_state", json!({"script": "browser_refusal"})).await;
    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["refusal"]["code"], "browser_ref_stale");
    assert!(structured["hint"]
        .as_str()
        .unwrap()
        .contains("get_browser_state"));
}

#[tokio::test]
async fn a_refused_action_result_keeps_its_closed_shape_and_gains_a_hint() {
    // An action tool's refusal is published as `effect: refused` with `error.code`.
    let result = call(
        "type_text",
        json!({"script": "browser_refusal", "pid": 42, "window_id": 7}),
    )
    .await;
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["effect"], "refused");
    assert_eq!(structured["error"]["code"], "browser_ref_stale");
    assert!(structured["hint"]
        .as_str()
        .unwrap()
        .contains("get_browser_state"));
    assert_eq!(structured["next_call"]["tool"], "get_browser_state");
    let schema = cua_driver_contract::advertised_tool_output_schema("type_text").expect("schema");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert!(
        validator.is_valid(&structured),
        "schema rejected {structured}"
    );
}

#[tokio::test]
async fn an_unverifiable_action_publishes_a_hint_inside_the_closed_contract() {
    let result = call(
        "click",
        json!({"script": "unverifiable", "pid": 42, "window_id": 7}),
    )
    .await;
    assert_ne!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["effect"], "unverifiable");
    assert!(structured["hint"]
        .as_str()
        .unwrap()
        .starts_with("The click was sent"));
    assert_eq!(
        structured["next_call"],
        json!({
            "tool": "get_window_state",
            "arguments": {"pid": 42, "window_id": 7, "since": "latest"},
        })
    );
    // The advertised output schema is closed; the hint must satisfy it.
    let schema = cua_driver_contract::advertised_tool_output_schema("click").expect("schema");
    let validator = jsonschema::validator_for(&schema).expect("schema compiles");
    assert!(
        validator.is_valid(&structured),
        "schema rejected {structured}"
    );
    cua_driver_contract::validate_success_output("click", structured).expect("typed output");
}

#[tokio::test]
async fn a_plain_missing_argument_error_gains_a_code_and_a_hint() {
    let result = call("drag", json!({"script": "missing"})).await;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured");
    assert_eq!(structured["code"], "invalid_arguments");
    assert!(structured["hint"]
        .as_str()
        .unwrap()
        .contains("required argument"));
}

#[tokio::test]
async fn successes_and_unknown_tools_stay_as_they_were() {
    let result = call("get_window_state", json!({"pid": 1, "window_id": 2})).await;
    assert_ne!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());

    let unknown = call("no_such_tool", json!({})).await;
    assert_eq!(unknown.is_error, Some(true));
    assert!(unknown.structured_content.is_none());
}

#[tokio::test]
async fn reserved_underscore_arguments_never_reach_a_next_call() {
    let result = call(
        "click",
        json!({
            "script": "stale",
            "pid": 42,
            "window_id": 7,
            "_session_id": "forged-internal-id",
        }),
    )
    .await;
    let rendered = result.structured_content.expect("structured").to_string();
    assert!(!rendered.contains("forged-internal-id"));
}

#[tokio::test]
async fn a_failed_batch_step_reports_the_hint_and_next_call() {
    let result = call(
        "run_steps",
        json!({
            "steps": [{
                "tool": "click",
                "args": {"script": "stale", "pid": 42, "window_id": 7},
            }],
        }),
    )
    .await;
    assert_eq!(result.is_error, Some(true));
    let structured = result.structured_content.expect("structured");
    let step = &structured["steps"][0];
    assert_eq!(step["ok"], false);
    assert_eq!(step["code"], "stale_element_token");
    assert!(step["hint"].as_str().unwrap().contains("fresh tokens"));
    assert_eq!(step["next_call"]["tool"], "get_window_state");
    assert_eq!(step["next_call"]["arguments"]["pid"], 42);
}
