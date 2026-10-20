//! An agent that sends action after action as single calls is told, at the
//! dispatch seam, to chain them with `run_steps`. Steps inside `run_steps` are
//! never nudged.

use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::batch_nudge::NUDGE;
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;

/// A stand-in action tool whose text-only result the registry turns into an
/// `unverifiable` action result, as a click is in practice.
struct Stub(ToolDef);

#[async_trait::async_trait]
impl Tool for Stub {
    fn def(&self) -> &ToolDef {
        &self.0
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        ToolResult::text("Posted")
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
    for name in ["click", "press_key", "get_window_state"] {
        registry.register(Box::new(Stub(ToolDef {
            name: name.to_owned(),
            description: "stub".to_owned(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pid": {"type": "integer"},
                    "window_id": {"type": "integer"},
                    "session": {"type": "string"},
                    "key": {"type": "string"},
                },
            }),
            read_only: name == "get_window_state",
            destructive: false,
            idempotent: false,
            open_world: false,
        })));
    }
    let registry = Arc::new(registry);
    registry.init_self_weak();
    registry
}

async fn call(registry: &ToolRegistry, tool: &str, args: Value) -> ToolResult {
    registry
        .invoke_with_context(tool, args, unrestricted_context())
        .await
}

fn hint(result: &ToolResult) -> Option<String> {
    result
        .structured_content
        .as_ref()
        .and_then(|value| value.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

#[tokio::test]
async fn the_third_direct_action_carries_the_nudge_after_its_own_hint() {
    let registry = registry();
    let args = || json!({"pid": 42, "window_id": 7, "session": "nudge-direct"});
    let first = call(&registry, "click", args()).await;
    let second = call(
        &registry,
        "press_key",
        json!({"pid": 42, "key": "tab", "session": "nudge-direct"}),
    )
    .await;
    // A read between actions neither counts nor breaks the streak.
    call(&registry, "get_window_state", args()).await;
    let third = call(&registry, "click", args()).await;

    let own = hint(&first).expect("an unverifiable click carries its recovery hint");
    assert!(!own.contains(NUDGE));
    assert!(!hint(&second).unwrap_or_default().contains(NUDGE));
    let third = hint(&third).expect("hint");
    assert!(third.ends_with(NUDGE), "{third}");
    assert!(third.starts_with(&own), "{third}");

    // The nudge is advice only: it adds no call to make.
    let fourth = call(&registry, "click", args()).await;
    assert!(!hint(&fourth).unwrap_or_default().contains(NUDGE));
}

#[tokio::test]
async fn steps_inside_run_steps_are_not_nudged_and_do_not_count() {
    let registry = registry();
    let batch = call(
        &registry,
        "run_steps",
        json!({"session": "nudge-batch", "steps": [
            {"tool": "click", "args": {"pid": 42, "window_id": 7}},
            {"tool": "click", "args": {"pid": 42, "window_id": 7}},
            {"tool": "click", "args": {"pid": 42, "window_id": 7}},
            {"tool": "click", "args": {"pid": 42, "window_id": 7}},
        ]}),
    )
    .await;
    let rendered = serde_json::to_string(&batch.structured_content).unwrap();
    assert!(!rendered.contains(NUDGE), "{rendered}");

    // Four batched steps did not make a streak: the next direct call is the first.
    let direct = call(
        &registry,
        "click",
        json!({"pid": 42, "window_id": 7, "session": "nudge-batch"}),
    )
    .await;
    assert!(!hint(&direct).unwrap_or_default().contains(NUDGE));
}
