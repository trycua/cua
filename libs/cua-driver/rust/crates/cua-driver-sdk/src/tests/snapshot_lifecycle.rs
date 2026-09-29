use super::{CuaDriver, DriverHostOptions};
use cua_driver_core::element_cache::{register_runtime_cache, ElementCacheCore, SnapshotPayload};
use cua_driver_core::element_token::{token_for, ResolvedElement, STALE_TOKEN_ERROR};
use cua_driver_core::protocol::ToolResult;
use cua_driver_core::tool::{
    current_dispatch_runtime_scope, with_runtime_scope, Tool, ToolDef, ToolRegistry,
};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;

const WAIT: Duration = Duration::from_secs(5);

struct ProbePayload;
impl SnapshotPayload for ProbePayload {
    type Element = usize;
    fn len(&self) -> usize {
        1
    }
    fn retain(&self, index: usize) -> Option<usize> {
        (index == 0).then_some(0)
    }
}

fn resolve<S: SnapshotPayload>(
    cache: &ElementCacheCore<S>,
    pid: i32,
    token: &str,
) -> Result<(u64, usize), String> {
    cache
        .resolve_element_args(pid, None, Some(token), None, None, "click")
        .map(|result| match result {
            ResolvedElement::Element {
                window_id: Some(window),
                element_index,
                ..
            } => (window, element_index),
            _ => panic!("expected element"),
        })
        .map_err(|error| {
            error.structured_content.unwrap()["refusal"]["message"]
                .as_str()
                .unwrap()
                .to_owned()
        })
}

struct CaptureProbe {
    started: Notify,
    invocation_dropped: Notify,
    native_finished: Notify,
    release: Mutex<Option<mpsc::Receiver<()>>>,
    published: Mutex<Option<(String, String)>>,
    cache: Mutex<Option<Arc<ElementCacheCore<ProbePayload>>>>,
}

struct ReleaseNative(Option<mpsc::Sender<()>>);

impl Drop for ReleaseNative {
    fn drop(&mut self) {
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

struct InvocationLifetime(Arc<CaptureProbe>);

impl Drop for InvocationLifetime {
    fn drop(&mut self) {
        self.0.invocation_dropped.notify_one();
    }
}

thread_local! {
    static NEXT_PROBE: RefCell<Option<Arc<CaptureProbe>>> = const { RefCell::new(None) };
}

struct CaptureTool {
    def: ToolDef,
    probe: Arc<CaptureProbe>,
    cache: Arc<ElementCacheCore<ProbePayload>>,
}

#[async_trait::async_trait]
impl Tool for CaptureTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let _lifetime = InvocationLifetime(self.probe.clone());
        let native = self.probe.clone();
        let receiver = native.release.lock().unwrap().take().unwrap();
        tokio::task::spawn_blocking(move || {
            native.started.notify_one();
            let released = receiver.recv_timeout(WAIT);
            native.native_finished.notify_one();
            released.expect("test did not release native capture");
        })
        .await
        .unwrap();
        let scope = current_dispatch_runtime_scope().expect("SDK supplied runtime scope");
        let snapshot = self.cache.publish(731_347, 17, ProbePayload);
        let token = token_for(snapshot, 0);
        *self.probe.published.lock().unwrap() = Some((scope, token.clone()));
        ToolResult::text("capture complete").with_structured(json!({"token": token}))
    }
}

fn register_capture_probe(registry: &mut ToolRegistry) {
    let probe = NEXT_PROBE.with(|probe| probe.borrow_mut().take().unwrap());
    let cache = Arc::new(ElementCacheCore::new());
    register_runtime_cache(&cache);
    *probe.cache.lock().unwrap() = Some(cache.clone());
    registry.register(Box::new(CaptureTool {
        def: ToolDef {
            name: "health_report".into(),
            description: "test-only capture completion and lifecycle probe".into(),
            input_schema: json!({"type": "object"}),
            read_only: true,
            destructive: false,
            idempotent: false,
            open_world: false,
        },
        probe,
        cache,
    }));
}

fn driver_with_tools(register: fn(&mut ToolRegistry)) -> Arc<CuaDriver> {
    CuaDriver::try_create_for_host(DriverHostOptions {
        cursor: cursor_overlay::CursorConfig {
            enabled: false,
            ..Default::default()
        },
        host_owns_permission_ux: false,
        host_bundle_id: None,
        claude_code_compatibility: false,
        prepare_desktop_environment: false,
        register_host_tools: Some(register),
        authorization_host: None,
        activity_observer: None,
    })
    .unwrap()
}

fn capture_driver() -> (Arc<CuaDriver>, Arc<CaptureProbe>, ReleaseNative) {
    let (sender, receiver) = mpsc::channel();
    let probe = Arc::new(CaptureProbe {
        started: Notify::new(),
        invocation_dropped: Notify::new(),
        native_finished: Notify::new(),
        release: Mutex::new(Some(receiver)),
        published: Mutex::new(None),
        cache: Mutex::new(None),
    });
    NEXT_PROBE.with(|next| *next.borrow_mut() = Some(probe.clone()));
    (
        driver_with_tools(register_capture_probe),
        probe,
        ReleaseNative(Some(sender)),
    )
}

async fn wait_for_closed_admission(driver: &CuaDriver) {
    tokio::time::timeout(WAIT, async {
        while driver.is_available() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("shutdown did not close admission");
}

#[tokio::test]
async fn sdk_shutdown_drains_snapshot_publication_and_retires_the_result() {
    let serial = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
    let (driver, probe, release) = capture_driver();
    let caller = driver.clone();
    let action =
        tokio::spawn(async move { caller.call_tool("health_report".into(), "{}".into()).await });
    tokio::time::timeout(WAIT, probe.started.notified())
        .await
        .unwrap();
    let closer = driver.clone();
    let shutdown = tokio::spawn(async move { closer.shutdown().await });
    wait_for_closed_admission(&driver).await;
    let returned_before_capture = shutdown.is_finished();
    let refused = driver.call_tool("health_report".into(), "{}".into()).await;
    drop(release);
    let result = tokio::time::timeout(WAIT, action)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    tokio::time::timeout(WAIT, shutdown)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (scope, token) = probe.published.lock().unwrap().clone().unwrap();
    let resolution = with_runtime_scope(scope, || {
        resolve(
            probe.cache.lock().unwrap().as_ref().unwrap(),
            731_347,
            &token,
        )
    });
    drop(driver);
    drop(serial);
    assert!(
        !returned_before_capture,
        "shutdown returned during an admitted capture"
    );
    assert!(
        refused.is_err(),
        "closed runtime admitted another publisher"
    );
    assert!(!result.is_error);
    assert_eq!(resolution, Err(STALE_TOKEN_ERROR.to_owned()));
}

#[tokio::test]
async fn sdk_cancelled_capture_does_not_publish_after_shutdown() {
    let serial = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
    let (driver, probe, release) = capture_driver();
    let caller = driver.clone();
    let action =
        tokio::spawn(async move { caller.call_tool("health_report".into(), "{}".into()).await });
    let started = tokio::time::timeout(WAIT, probe.started.notified()).await;
    if started.is_err() && action.is_finished() {
        panic!(
            "capture returned before native work started: {:?}",
            action.await
        );
    }
    started.unwrap();
    action.abort();
    assert!(action.await.unwrap_err().is_cancelled());
    tokio::time::timeout(WAIT, probe.invocation_dropped.notified())
        .await
        .unwrap();
    let closer = driver.clone();
    let mut shutdown = tokio::spawn(async move { closer.shutdown().await });
    wait_for_closed_admission(&driver).await;
    let early = tokio::time::timeout(Duration::from_millis(250), &mut shutdown).await;
    drop(release);
    tokio::time::timeout(WAIT, probe.native_finished.notified())
        .await
        .unwrap();
    match early {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => tokio::time::timeout(WAIT, shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    }
    let published = probe.published.lock().unwrap().clone();
    drop(driver);
    drop(serial);
    assert!(
        published.is_none(),
        "cancelled capture published after its invocation ended"
    );
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use crate::DriverBackend;
    use core_foundation::base::{CFGetRetainCount, CFRetain, CFTypeRef, TCFType};
    use core_foundation::string::CFString;
    use cua_driver_core::element_cache::current_runtime_cache;
    use platform_macos::ax::cache::{CachedSnapshot, ElementCache};

    thread_local! {
        static NEXT_STATE: RefCell<Option<(usize, u32, Option<String>)>> = const { RefCell::new(None) };
    }

    struct NativeStateTool {
        def: ToolDef,
        cache: Arc<ElementCache>,
        token: String,
    }

    #[async_trait::async_trait]
    impl Tool for NativeStateTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }

        async fn invoke(&self, _args: Value) -> ToolResult {
            ToolResult::text(
                resolve(&self.cache, 731_348, &self.token)
                    .is_ok()
                    .to_string(),
            )
        }
    }

    fn register_native_state(registry: &mut ToolRegistry) {
        let cache =
            current_runtime_cache::<CachedSnapshot>().expect("built-in native cache registered");
        let token = NEXT_STATE.with(|next| {
            let mut next = next.borrow_mut();
            let (ptr, windows, oldest) = next.as_mut().unwrap();
            for window in 0..*windows {
                unsafe { CFRetain(*ptr as CFTypeRef) };
                let id = cache.publish(
                    731_348,
                    u64::from(window),
                    CachedSnapshot {
                        elements: vec![*ptr],
                    },
                );
                if window == 0 {
                    *oldest = Some(token_for(id, 0));
                }
            }
            oldest.clone().unwrap()
        });
        registry.register(Box::new(NativeStateTool {
            cache,
            token,
            def: ToolDef {
                name: "health_report".into(),
                description: "test-only owner of real macOS ToolState".into(),
                input_schema: json!({"type": "object"}),
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        }));
    }

    fn native_driver(ptr: usize, windows: u32) -> (Arc<CuaDriver>, String, String) {
        NEXT_STATE.with(|next| *next.borrow_mut() = Some((ptr, windows, None)));
        let driver = driver_with_tools(register_native_state);
        let DriverBackend::Embedded(runtime) = &driver.backend else {
            panic!("expected actual embedded SDK runtime");
        };
        let scope = runtime.runtime_scope_key().to_owned();
        let oldest = NEXT_STATE.with(|next| next.borrow_mut().take().unwrap().2.unwrap());
        (driver, scope, oldest)
    }

    fn resolve_native(token: &str) -> Result<(u64, usize), String> {
        let cache = current_runtime_cache::<CachedSnapshot>()
            .ok_or_else(|| STALE_TOKEN_ERROR.to_owned())?;
        resolve(&cache, 731_348, token)
    }

    #[tokio::test]
    async fn sdk_shutdown_releases_native_snapshot_while_closed_handle_is_retained() {
        let serial = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let value = CFString::new("sdk-snapshot-shutdown-native-retain-accounting");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let (driver, scope, token) = native_driver(ptr, 1);
        assert_eq!(unsafe { CFGetRetainCount(ptr as CFTypeRef) }, base + 1);
        driver.shutdown().await.unwrap();
        let retained_after_shutdown = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let retired = with_runtime_scope(scope, || resolve_native(&token));
        drop(driver);
        let retained_after_destroy = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        drop(serial);
        assert_eq!(retired, Err(STALE_TOKEN_ERROR.to_owned()));
        assert_eq!(
            retained_after_destroy, base,
            "destroy must balance the native retain"
        );
        assert_eq!(
            retained_after_shutdown, base,
            "closed SDK handle still owns an unadmitted native snapshot"
        );
    }

    #[tokio::test]
    async fn sdk_destroying_one_runtime_preserves_other_native_snapshot() {
        let serial = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let first = CFString::new("sdk-first-runtime-native-snapshot-isolation");
        let second = CFString::new("sdk-second-runtime-native-snapshot-isolation");
        let first_ptr = first.as_concrete_TypeRef() as usize;
        let second_ptr = second.as_concrete_TypeRef() as usize;
        let first_base = unsafe { CFGetRetainCount(first_ptr as CFTypeRef) };
        let second_base = unsafe { CFGetRetainCount(second_ptr as CFTypeRef) };
        let (first_driver, first_scope, first_token) = native_driver(first_ptr, 1);
        let (second_driver, second_scope, second_token) = native_driver(second_ptr, 1);
        assert_ne!(first_scope, second_scope);
        first_driver.shutdown().await.unwrap();
        drop(first_driver);
        let first_count = unsafe { CFGetRetainCount(first_ptr as CFTypeRef) };
        let second_count = unsafe { CFGetRetainCount(second_ptr as CFTypeRef) };
        let own = with_runtime_scope(second_scope.clone(), || resolve_native(&second_token));
        let foreign = with_runtime_scope(second_scope, || resolve_native(&first_token));
        second_driver.shutdown().await.unwrap();
        drop(second_driver);
        let second_after_destroy = unsafe { CFGetRetainCount(second_ptr as CFTypeRef) };
        drop(serial);
        assert_eq!(first_count, first_base);
        assert_eq!(second_count, second_base + 1);
        assert_eq!(own, Ok((0, 0)));
        assert!(foreign.is_err());
        assert_eq!(second_after_destroy, second_base);
    }

    #[tokio::test]
    async fn sdk_token_eviction_releases_the_corresponding_native_snapshot() {
        let serial = crate::runtime::TEST_RUNTIME_LOCK.lock().unwrap();
        let value = CFString::new("sdk-snapshot-eviction-native-retain-accounting");
        let ptr = value.as_concrete_TypeRef() as usize;
        let base = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let cap = cua_driver_core::element_token::LRU_CAP_PER_PID;
        let (driver, scope, token) = native_driver(ptr, cap as u32 + 1);
        let retained_after_eviction = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        let retired = with_runtime_scope(scope, || resolve_native(&token));
        driver.shutdown().await.unwrap();
        drop(driver);
        let retained_after_destroy = unsafe { CFGetRetainCount(ptr as CFTypeRef) };
        drop(serial);
        assert_eq!(retired, Err(STALE_TOKEN_ERROR.to_owned()));
        assert_eq!(retained_after_destroy, base);
        assert_eq!(
            retained_after_eviction,
            base + cap as isize,
            "token eviction did not retire native cache ownership"
        );
    }
}
