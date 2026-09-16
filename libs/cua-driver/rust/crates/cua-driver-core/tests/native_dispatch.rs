use cua_driver_core::tool::{
    current_dispatch_runtime_scope, native_dispatch_allowed, scope_native_dispatch, spawn_native,
    with_runtime_scope,
};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

#[tokio::test(flavor = "current_thread")]
async fn native_workers_inherit_the_runtime_and_live_request() {
    scope_native_dispatch(None, async {
        let worker = with_runtime_scope("native-request-owner".into(), || {
            spawn_native(|| (current_dispatch_runtime_scope(), native_dispatch_allowed()))
        });
        assert_eq!(
            worker.await.unwrap(),
            (Some("native-request-owner".into()), true)
        );
    })
    .await;
}

async fn cancelled_native_request(begin_before_cancel: bool, dedicated: bool) {
    let deliveries = Arc::new(AtomicUsize::new(0));
    let native_deliveries = deliveries.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let (finished, completed) = tokio::sync::oneshot::channel();
    let request = tokio::spawn(scope_native_dispatch(None, async move {
        spawn_native(move || {
            let work = move || {
                let mut began = false;
                if begin_before_cancel && native_dispatch_allowed() {
                    native_deliveries.fetch_add(1, Ordering::SeqCst);
                    began = true;
                }
                entered.send(()).unwrap();
                blocked.recv_timeout(Duration::from_secs(2)).unwrap();
                if !began && native_dispatch_allowed() {
                    native_deliveries.fetch_add(1, Ordering::SeqCst);
                }
                finished.send(()).unwrap();
            };
            if dedicated {
                std::thread::spawn(cua_driver_core::tool::bind_native(work))
                    .join()
                    .unwrap();
            } else {
                work();
            }
        })
        .await
        .unwrap();
    }));
    ready.await.unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(2), completed)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        deliveries.load(Ordering::SeqCst),
        usize::from(begin_before_cancel)
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_prevents_a_later_native_dispatch_start() {
    for dedicated in [false, true] {
        cancelled_native_request(false, dedicated).await;
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_does_not_claim_to_undo_started_delivery_or_skip_cleanup() {
    for dedicated in [false, true] {
        cancelled_native_request(true, dedicated).await;
    }
}

struct NativeTextBoundary {
    def: cua_driver_core::tool::ToolDef,
    calls: Arc<AtomicUsize>,
    captures: Arc<AtomicUsize>,
    pending: std::sync::Mutex<
        Option<(
            std::sync::mpsc::Receiver<()>,
            tokio::sync::oneshot::Sender<tokio::task::JoinHandle<()>>,
        )>,
    >,
}

#[async_trait::async_trait]
impl cua_driver_core::tool::Tool for NativeTextBoundary {
    fn def(&self) -> &cua_driver_core::tool::ToolDef {
        &self.def
    }

    async fn invoke(&self, _: serde_json::Value) -> cua_driver_core::protocol::ToolResult {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let pending = self.pending.lock().unwrap().take();
        if let Some((blocked, observe)) = pending {
            let captures = self.captures.clone();
            let worker = spawn_native(move || {
                blocked.recv_timeout(Duration::from_secs(5)).unwrap();
                if let Some((window, pid)) = cua_driver_core::recording::dispatch_click_target() {
                    cua_driver_core::recording::capture_dispatch_click_target(window, pid, || {
                        captures.fetch_add(1, Ordering::SeqCst);
                        Some((
                            cua_driver_core::image_utils::encode_rgba_to_png(&[255; 16], 2, 2)
                                .unwrap(),
                            1.0,
                            1.0,
                        ))
                    });
                }
            });
            observe.send(worker).unwrap();
            std::future::pending::<()>().await;
        }
        cua_driver_core::protocol::ToolResult::error("native test boundary completed")
    }
}

#[tokio::test(flavor = "current_thread")]
async fn cancelled_registry_request_holds_text_admission_until_native_cleanup() {
    use cua_driver_core::{
        authorization::PermissionMode,
        session_authorization::{SessionAuthorizationRegistry, SessionModeCeiling},
        tool::{ToolDef, ToolRegistry},
    };
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
    let calls = Arc::new(AtomicUsize::new(0));
    let (release, blocked) = std::sync::mpsc::channel();
    let (observe, worker) = tokio::sync::oneshot::channel();
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(NativeTextBoundary {
        def: ToolDef {
            name: "type_text".into(),
            description: "native test boundary".into(),
            input_schema: serde_json::json!({"type":"object"}),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        calls: calls.clone(),
        captures: Arc::new(AtomicUsize::new(0)),
        pending: std::sync::Mutex::new(Some((blocked, observe))),
    }));
    let registry = Arc::new(registry);
    let args = serde_json::json!({"pid": 8675431, "window_id": 44, "text": "x"});
    let request = tokio::spawn({
        let registry = registry.clone();
        let context = context.clone();
        let args = args.clone();
        async move {
            registry
                .invoke_with_context("type_text", args, context)
                .await
        }
    });
    let worker = tokio::time::timeout(Duration::from_secs(2), worker)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    let overlapping = registry
        .invoke_with_context("type_text", args.clone(), context.clone())
        .await;
    assert_eq!(
        overlapping.structured_content.unwrap()["refusal"]["code"],
        "input_busy"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    worker.await.unwrap();
    registry
        .invoke_with_context("type_text", args, context)
        .await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn native_recording_capture_follows_the_live_registry_request() {
    use cua_driver_core::{
        authorization::PermissionMode,
        session_authorization::{SessionAuthorizationRegistry, SessionModeCeiling},
        tool::{ToolDef, ToolRegistry},
    };
    for cancel_before_capture in [false, true] {
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
        let captures = Arc::new(AtomicUsize::new(0));
        let (release, blocked) = std::sync::mpsc::channel();
        let (observe, worker) = tokio::sync::oneshot::channel();
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(NativeTextBoundary {
            def: ToolDef {
                name: "click".into(),
                description: "native test boundary".into(),
                input_schema: serde_json::json!({"type":"object"}),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            calls: Arc::new(AtomicUsize::new(0)),
            captures: captures.clone(),
            pending: std::sync::Mutex::new(Some((blocked, observe))),
        }));
        let root = tempfile::tempdir().unwrap();
        registry
            .recording
            .start(root.path().to_str().unwrap(), false, None)
            .unwrap();
        let request = tokio::spawn(async move {
            registry
                .invoke_with_context(
                    "click",
                    serde_json::json!({"pid":8675432, "window_id":44, "x":1, "y":1}),
                    context,
                )
                .await
        });
        let worker = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        if cancel_before_capture {
            request.abort();
            while !request.is_finished() {
                tokio::task::yield_now().await;
            }
        }
        release.send(()).unwrap();
        worker.await.unwrap();
        assert_eq!(
            captures.load(Ordering::SeqCst),
            usize::from(!cancel_before_capture)
        );
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
    }
}
