//! Experimental read-only, runtime-session-scoped supervision receipts.
use super::ToolState;
use async_trait::async_trait;
use cua_driver_core::{
    owned_supervision::ReceiptId,
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
};
use serde_json::Value;
use std::{sync::Arc, time::Duration};
struct ReceiptTool {
    state: Arc<ToolState>,
    def: ToolDef,
    fence: bool,
    release: bool,
}
pub fn register(registry: &mut ToolRegistry, state: Arc<ToolState>) {
    for (name, fence, release) in [
        ("get_action_supervision", false, false),
        ("fence_action_supervision", true, false),
        ("release_action_supervision", false, true),
    ] {
        registry.register(Box::new(ReceiptTool { state: state.clone(), fence, release, def: ToolDef {
            name: name.into(), description: "Read an owned native observation receipt for this explicit runtime session. Finished observation is not application commitment. Timeout retains the observer; never replay input merely because observation is pending.".into(),
            input_schema: serde_json::json!({"type":"object","required":["session","receipt_id"],"properties":{"session":{"type":"string"},"receipt_id":{"type":"string"},"timeout_ms":{"type":"integer","minimum":0,"maximum":60000}},"additionalProperties":false}),
            read_only:!release,destructive:false,idempotent:!release,open_world:false,
        }}));
    }
}
#[async_trait]
impl Tool for ReceiptTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if args
            .get("_public_session_label")
            .and_then(Value::as_str)
            .is_none()
        {
            return ToolResult::error("session_required: explicit runtime session required");
        }
        let Some(scope) = args.get("_session_id").and_then(Value::as_str) else {
            return ToolResult::error("session_required");
        };
        let id: ReceiptId = match serde_json::from_value(args["receipt_id"].clone()) {
            Ok(id) => id,
            Err(_) => return ToolResult::error("receipt_unavailable"),
        };
        if self.release {
            return match self.state.supervision.release(scope, &id) {
                Ok(()) => ToolResult::text("Terminal observation receipt released.")
                    .with_structured(serde_json::json!({"receipt_id":id,"released":true})),
                Err(e) => ToolResult::error(format!(
                    "supervision receipt: {e:?}; pending observers cannot be released."
                )),
            };
        }
        if self.fence
            && args
                .get("timeout_ms")
                .is_some_and(|value| value.as_u64().is_none_or(|ms| ms > 60000))
        {
            return ToolResult::error("invalid_timeout: expected integer 0..60000");
        }
        let outcome = if self.fence {
            self.state
                .supervision
                .fence(
                    scope,
                    &id,
                    Duration::from_millis(args["timeout_ms"].as_u64().unwrap_or(1000)),
                )
                .await
        } else {
            self.state.supervision.read(scope, &id)
        };
        match outcome {
            Ok(status) => ToolResult::text("Observation status only; independently verify application commitment.").with_structured(serde_json::json!({"receipt_id":id,"supervision":status,"activation_after_dispatch":self.state.supervision.activation_observed(scope,&id).ok().flatten(),"application_commit":"unverified"})),
            Err(e) => ToolResult::error(format!("supervision receipt: {e:?}; input must not be replayed without fresh observation.")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_driver_core::owned_supervision::{Observation, ReceiptState};
    fn tool(state: Arc<ToolState>, fence: bool, release: bool) -> ReceiptTool {
        ReceiptTool {
            state,
            fence,
            release,
            def: ToolDef {
                name: "receipt-test".into(),
                description: "test".into(),
                input_schema: serde_json::json!({}),
                read_only: !release,
                destructive: false,
                idempotent: !release,
                open_world: false,
            },
        }
    }
    fn args(id: &ReceiptId, scope: &str) -> Value {
        serde_json::json!({"_session_id":scope,"_public_session_label":"explicit","receipt_id":id,"timeout_ms":0})
    }
    fn refusal(result: ToolResult, message: &str) {
        assert_eq!(result.is_error, Some(true));
        assert_eq!(
            serde_json::to_value(result).unwrap()["content"][0]["text"],
            message
        );
    }
    #[tokio::test]
    async fn timeout_and_foreign_scope_cannot_cancel_or_release_observer() {
        let state = Arc::new(ToolState::default());
        let (send, receive) = tokio::sync::oneshot::channel();
        let id = state
            .supervision
            .reserve("owner")
            .unwrap()
            .supervise(async move {
                receive.await.unwrap();
                Observation {
                    polled: true,
                    foreground_changed: false,
                    new_window_count: 0,
                }
            });
        refusal(
            tool(state.clone(), true, false)
                .invoke(args(&id, "owner"))
                .await,
            "supervision receipt: Timeout; input must not be replayed without fresh observation.",
        );
        refusal(tool(state.clone(),false,false).invoke(args(&id,"other")).await,"supervision receipt: Unavailable; input must not be replayed without fresh observation.");
        refusal(
            tool(state.clone(), false, true)
                .invoke(args(&id, "owner"))
                .await,
            "supervision receipt: Pending; pending observers cannot be released.",
        );
        assert_eq!(
            state.supervision.read("owner", &id).unwrap(),
            ReceiptState::Pending
        );
        send.send(()).unwrap();
        state
            .supervision
            .fence("owner", &id, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(
            tool(state.clone(), false, true)
                .invoke(args(&id, "owner"))
                .await
                .structured_content
                .unwrap()["released"],
            true
        );
        refusal(tool(state,false,false).invoke(args(&id,"owner")).await,"supervision receipt: Unavailable; input must not be replayed without fresh observation.");
    }
    #[tokio::test]
    async fn missing_explicit_session_and_invalid_timeout_refuse() {
        let state = Arc::new(ToolState::default());
        let id = state
            .supervision
            .reserve("owner")
            .unwrap()
            .supervise(async {
                Observation {
                    polled: true,
                    foreground_changed: false,
                    new_window_count: 0,
                }
            });
        let mut a = args(&id, "owner");
        a.as_object_mut().unwrap().remove("_public_session_label");
        refusal(
            tool(state.clone(), false, false).invoke(a).await,
            "session_required: explicit runtime session required",
        );
        for timeout in [
            serde_json::json!(-1),
            serde_json::json!(60001),
            serde_json::json!("1"),
        ] {
            let mut a = args(&id, "owner");
            a["timeout_ms"] = timeout;
            refusal(
                tool(state.clone(), true, false).invoke(a).await,
                "invalid_timeout: expected integer 0..60000",
            );
        }
    }
}
