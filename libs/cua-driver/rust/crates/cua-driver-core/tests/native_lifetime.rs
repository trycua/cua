use cua_driver_core::{
    action_record::{ActionTransport, RequestedDelivery},
    protocol::ToolResult,
    tool::{retain_native_resource, scope_native_dispatch, spawn_native},
};
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn cancelled_action_keeps_platform_mutation_lease_until_worker_cleanup() {
    let lane = Arc::new(tokio::sync::Semaphore::new(1));
    let (release, blocked) = std::sync::mpsc::channel();
    let (observe, worker) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(scope_native_dispatch(None, {
        let lane = lane.clone();
        async move {
            let guard = retain_native_resource(lane.acquire_owned().await.unwrap());
            let native = spawn_native(move || {
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            drop(guard);
            observe.send(native).unwrap();
            std::future::pending::<()>().await;
        }
    }));
    let native = worker.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(lane.available_permits(), 0);
    release.send(()).unwrap();
    native.await.unwrap();
    assert_eq!(lane.available_permits(), 1);
}

#[test]
fn uncertain_native_attempt_preserves_unknown_delivery_without_retry_advice() {
    let error = ToolResult::native_action_error(
        "native write did not acknowledge delivery",
        ActionTransport::MacosAxValue,
    );
    let result = ToolResult::from_native_error(error, RequestedDelivery::Background);
    assert_eq!(result.is_error, Some(true));
    let public = result.structured_content.unwrap();
    assert_eq!(public["effect"], "unverifiable");
    assert_eq!(public["delivery"]["mode"], "unknown");
    assert!(public.get("escalation").is_none());
    assert!(result.action_record.is_some());
}

#[test]
fn pre_dispatch_failure_does_not_invent_native_delivery() {
    let result = ToolResult::from_native_error(
        anyhow::anyhow!("target description changed before dispatch"),
        RequestedDelivery::Background,
    );
    assert_eq!(result.is_error, Some(true));
    assert!(result.action_record.is_none());
    assert!(result.structured_content.is_none());
}

#[test]
fn public_target_schema_requires_identity_not_observation_position() {
    let index = cua_driver_core::tool_schema::element_index_schema();
    let snapshot = cua_driver_core::tool_schema::snapshot_id_schema();
    assert!(index["description"].as_str().unwrap().contains("refused"));
    assert!(snapshot["description"]
        .as_str()
        .unwrap()
        .contains("does not authorize"));
    let request = serde_json::json!({
        "target": {"kind":"window", "pid":7, "window_id":42},
        "delivery_mode":"background", "element_index":3, "snapshot_id":"observation"
    });
    assert!(serde_json::from_value::<cua_driver_contract::ClickInput>(request).is_err());
}

struct EndingAction {
    definition: cua_driver_core::tool::ToolDef,
    acknowledged: bool,
}

#[async_trait::async_trait]
impl cua_driver_core::tool::Tool for EndingAction {
    fn def(&self) -> &cua_driver_core::tool::ToolDef {
        &self.definition
    }
    async fn invoke(&self, args: serde_json::Value) -> ToolResult {
        use cua_driver_core::action_record::*;
        cua_driver_core::session::end_session(args["session"].as_str().unwrap());
        let result = ToolResult::text("native boundary returned")
            .with_structured(serde_json::json!({"path":"ax", "verified":false}));
        if self.acknowledged {
            result.with_action_record(
                ActionExecutionRecord::builder(
                    ActionEffect::Unverifiable,
                    ActionTransport::MacosAxAction,
                    RequestedDelivery::Background,
                )
                .actual_delivery(ActualDelivery::Background)
                .evidence(ActionEvidence {
                    kind: EvidenceKind::NativeApiResult,
                    detail: "AXPress".into(),
                })
                .build()
                .unwrap(),
            )
        } else {
            result
        }
    }
}

#[tokio::test]
async fn session_cancellation_preserves_acknowledgement_but_not_legacy_delivery_inference() {
    use cua_driver_core::{
        action_record::ActualDelivery,
        authorization::PermissionMode,
        session_authorization::{SessionAuthorizationRegistry, SessionModeCeiling},
        tool::{ToolDef, ToolRegistry},
    };
    for acknowledged in [false, true] {
        let ceiling = SessionModeCeiling::for_trusted_sessions(
            [PermissionMode::Standard],
            false,
            Duration::from_secs(60),
            Duration::from_secs(30),
        )
        .unwrap();
        let context = SessionAuthorizationRegistry::with_ceiling(ceiling)
            .compatibility_context(PermissionMode::Standard, None)
            .unwrap();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(EndingAction {
            definition: ToolDef {
                name: "click".into(),
                description: "native outcome boundary".into(),
                input_schema: serde_json::json!({"type":"object"}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            acknowledged,
        }));
        let result = registry
            .invoke_with_context(
                "click",
                serde_json::json!({
                    "session":format!("native-outcome-{acknowledged}"), "pid":8675433,
                    "window_id":44, "x":1, "y":1,
                }),
                context,
            )
            .await;
        assert_eq!(result.is_error, Some(true));
        let record = result.action_record.unwrap();
        assert_eq!(
            record.actual_delivery,
            Some(if acknowledged {
                ActualDelivery::Background
            } else {
                ActualDelivery::Unknown
            })
        );
        assert_eq!(!record.evidence.is_empty(), acknowledged);
    }
}
