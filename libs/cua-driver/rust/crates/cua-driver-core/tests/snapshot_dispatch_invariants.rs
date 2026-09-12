use cua_driver_core::authorization::PermissionMode;
use cua_driver_core::element_cache::{
    register_runtime_cache, retire_runtime_scope, ElementCacheCore, SnapshotPayload,
};
use cua_driver_core::element_token::{token_for, ResolvedElement};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::session_authorization::{
    EffectiveAuthorizationContext, SessionAuthorizationRegistry, SessionModeCeiling,
};
use cua_driver_core::tool::{with_runtime_scope, Tool, ToolDef, ToolRegistry};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

const PID: i32 = 732_347;
const WINDOW: u64 = 17;
const WAIT: Duration = Duration::from_secs(5);

struct Payload(Vec<u64>);

impl SnapshotPayload for Payload {
    type Element = u64;
    fn len(&self) -> usize {
        self.0.len()
    }
    fn retain(&self, index: usize) -> Option<u64> {
        self.0.get(index).copied()
    }
}

struct ProbeState {
    cache: Arc<ElementCacheCore<Payload>>,
    capture_started: Notify,
    finish_capture: Notify,
    observed: Mutex<Vec<u64>>,
}

struct ReleaseCapture(Arc<ProbeState>);

impl Drop for ReleaseCapture {
    fn drop(&mut self) {
        self.0.finish_capture.notify_one();
    }
}

struct ProbeTool {
    def: ToolDef,
    state: Arc<ProbeState>,
}

#[async_trait::async_trait]
impl Tool for ProbeTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if self.def.name == "get_window_state" {
            let revision = args["revision"].as_u64().unwrap();
            let prepared = Payload(vec![revision]);
            if args["pause_capture"] == true {
                self.state.capture_started.notify_one();
                self.state.finish_capture.notified().await;
            }
            let snapshot = self.state.cache.publish(PID, WINDOW, prepared);
            return ToolResult::text("test capture complete").with_structured(json!({
                "pid": PID,
                "window_id": WINDOW,
                "snapshot_id": format!("s{snapshot:08x}"),
                "elements": [{
                    "element_index": 0,
                    "element_token": token_for(snapshot, 0),
                    "role": "button",
                    "depth": 0
                }]
            }));
        }
        let resolved = match self.state.cache.resolve_element_args(
            PID,
            args["element_index"].as_u64().map(|index| index as usize),
            args["element_token"].as_str(),
            args["snapshot_id"].as_str(),
            Some(WINDOW),
            "click",
        ) {
            Ok(resolved) => resolved,
            Err(refusal) => return refusal,
        };
        let ResolvedElement::Element {
            window_id: Some(_),
            element: revision,
            ..
        } = resolved
        else {
            return ToolResult::error("test requires a snapshot-bound target");
        };
        self.state.observed.lock().unwrap().push(revision);
        ToolResult::error("test probe stops before native input")
    }
}

fn context() -> Arc<EffectiveAuthorizationContext> {
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

struct Fixture {
    registry: Arc<ToolRegistry>,
    context: Arc<EffectiveAuthorizationContext>,
    state: Arc<ProbeState>,
}

impl Fixture {
    fn new() -> Self {
        let context = context();
        let cache = with_runtime_scope(context.runtime_scope_key(), || {
            let cache = Arc::new(ElementCacheCore::new());
            register_runtime_cache(&cache);
            cache
        });
        let state = Arc::new(ProbeState {
            cache,
            capture_started: Notify::new(),
            finish_capture: Notify::new(),
            observed: Mutex::new(Vec::new()),
        });
        let mut registry = ToolRegistry::new();
        for (name, read_only) in [("get_window_state", true), ("click", false)] {
            registry.register(Box::new(ProbeTool {
                state: state.clone(),
                def: ToolDef {
                    name: name.into(),
                    description: "test-only paused capture and target lookup".into(),
                    input_schema: json!({"type": "object"}),
                    read_only,
                    destructive: false,
                    idempotent: false,
                    open_world: false,
                },
            }));
        }
        Self {
            registry: Arc::new(registry),
            context,
            state,
        }
    }

    async fn read(&self, revision: u64) -> ToolResult {
        let result = self
            .registry
            .invoke_with_context(
                "get_window_state",
                json!({"pid": PID, "window_id": WINDOW, "revision": revision}),
                self.context.clone(),
            )
            .await;
        assert_ne!(
            result.is_error,
            Some(true),
            "registry refused fixture read: {result:?}"
        );
        result
    }

    async fn click(&self, token: &str) -> ToolResult {
        self.registry
            .invoke_with_context(
                "click",
                json!({"pid": PID, "window_id": WINDOW, "element_token": token}),
                self.context.clone(),
            )
            .await
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        retire_runtime_scope(&self.context.runtime_scope_key());
    }
}

fn token(result: &ToolResult) -> String {
    result.structured_content.as_ref().unwrap()["elements"][0]["element_token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn dispatch_never_pairs_old_identity_with_pending_capture_payload() {
    let fixture = Fixture::new();
    let original = token(&fixture.read(1).await);
    let release = ReleaseCapture(fixture.state.clone());
    let registry = fixture.registry.clone();
    let context = fixture.context.clone();
    let reader = tokio::spawn(async move {
        registry
            .invoke_with_context(
                "get_window_state",
                json!({"pid": PID, "window_id": WINDOW, "revision": 2, "pause_capture": true}),
                context,
            )
            .await
    });
    tokio::time::timeout(WAIT, fixture.state.capture_started.notified())
        .await
        .unwrap();
    let action = tokio::time::timeout(WAIT, fixture.click(&original)).await;
    let observed = fixture.state.observed.lock().unwrap().clone();
    drop(release);
    let replacement = tokio::time::timeout(WAIT, reader).await.unwrap().unwrap();
    assert_ne!(replacement.is_error, Some(true));
    let action = action.expect("registry action waited on the entire paused capture");
    assert!(observed.is_empty() || observed == vec![1],
        "real registry admitted identity from snapshot 1 against payload {observed:?}; response: {action:?}");
}

#[tokio::test]
async fn dispatch_stale_refusal_and_fresh_recovery_reach_the_expected_lookup() {
    let fixture = Fixture::new();
    let original = token(&fixture.read(1).await);
    let replacement = token(&fixture.read(2).await);
    let stale = fixture.click(&original).await;
    assert_eq!(
        stale.structured_content.as_ref().unwrap()["refusal"]["code"],
        "stale_element_token"
    );
    assert!(fixture.state.observed.lock().unwrap().is_empty());
    fixture.click(&replacement).await;
    assert_eq!(*fixture.state.observed.lock().unwrap(), vec![2]);
}

#[tokio::test]
async fn dispatch_generation_refusal_precedes_native_payload_lookup() {
    let first = Fixture::new();
    let second = Fixture::new();
    let first_token = token(&first.read(1).await);
    let second_token = token(&second.read(2).await);
    let refused = second.click(&first_token).await;
    assert_eq!(
        refused.structured_content.as_ref().unwrap()["refusal"]["code"],
        "generation_mismatch"
    );
    assert!(second.state.observed.lock().unwrap().is_empty());
    first.click(&first_token).await;
    second.click(&second_token).await;
    assert_eq!(*first.state.observed.lock().unwrap(), vec![1]);
    assert_eq!(*second.state.observed.lock().unwrap(), vec![2]);
}
