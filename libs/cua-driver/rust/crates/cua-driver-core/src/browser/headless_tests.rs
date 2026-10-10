//! Driver-owned headless route: prepare → bind by opaque anchor → typed DOM
//! work, against the mock CDP endpoint. The platform below fails every
//! native-window, compositor, focus, and visual call, so these tests prove the
//! route never needs a display. No real process is spawned or signalled.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::Tool;

use super::engine::BrowserEngine;
use super::mock_cdp::{MockCdpServer, MockHandler};
use super::platform::{
    BrowserPlatform, BrowserVisualAction, IsolatedBrowserProcess, PrepareOutcome, PrepareRequest,
};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::tools::{
    BrowserClickTool, BrowserNavigateTool, BrowserPrepareTool, BrowserTypeTool, GetBrowserStateTool,
};
use super::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProcessRole, BrowserProduct,
    EndpointOwnershipMethod, EndpointOwnershipProof, EndpointTransport, NativeWindowInfo,
    OwnedEndpoint, ProcessFingerprint,
};
use super::v2_tests::{fixture_handler, ref_of, structured, FixtureState, SharedState};

/// Pids above Linux's maximum pid_max (2^22), so they can never be real.
static NEXT_FAKE_PID: AtomicU32 = AtomicU32::new(5_000_000);

/// A launched browser that only records what cleanup does to it.
struct FakeBrowser {
    pid: u32,
    signals: Arc<StdMutex<Vec<String>>>,
    /// The leader already exited and was reaped by `try_wait`.
    exited: bool,
}

impl IsolatedBrowserProcess for FakeBrowser {
    fn id(&self) -> u32 {
        self.pid
    }
    fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        use std::os::unix::process::ExitStatusExt;
        Ok(self.exited.then(|| ExitStatus::from_raw(0)))
    }
    fn kill(&mut self) -> std::io::Result<()> {
        self.signals
            .lock()
            .unwrap()
            .push(format!("kill:{}", self.pid));
        Ok(())
    }
    fn wait(&mut self) -> std::io::Result<ExitStatus> {
        use std::os::unix::process::ExitStatusExt;
        Ok(ExitStatus::from_raw(0))
    }
    fn kill_process_group(&mut self) {
        self.signals
            .lock()
            .unwrap()
            .push(format!("group:{}", self.pid));
    }
}

struct HeadlessPlatform {
    port: u16,
    ws_url: String,
    /// Replaces the live endpoint URL to simulate an endpoint change.
    endpoint_override: StdMutex<Option<String>>,
    start_time: AtomicU64,
    native_calls: AtomicUsize,
    spawned_args: StdMutex<Vec<Vec<String>>>,
    signals: Arc<StdMutex<Vec<String>>>,
}

impl HeadlessPlatform {
    fn native_call(&self) -> BrowserRefusal {
        self.native_calls.fetch_add(1, Ordering::SeqCst);
        BrowserRefusal::new(
            BrowserRefusalCode::BrowserRouteUnavailable,
            "headless fixture has no display",
        )
    }
}

#[async_trait]
impl BrowserPlatform for HeadlessPlatform {
    fn isolated_browser_executable(&self) -> Result<String, BrowserRefusal> {
        Ok("/fixture/chromium".into())
    }

    fn spawn_isolated_browser(
        &self,
        command: Command,
        profile: &Path,
    ) -> Result<Box<dyn IsolatedBrowserProcess>, BrowserRefusal> {
        self.spawned_args.lock().unwrap().push(
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        );
        std::fs::write(
            profile.join("DevToolsActivePort"),
            format!("{}\n/devtools/browser/mock", self.port),
        )
        .unwrap();
        Ok(Box::new(FakeBrowser {
            pid: NEXT_FAKE_PID.fetch_add(1, Ordering::SeqCst),
            signals: self.signals.clone(),
            exited: false,
        }))
    }

    async fn visualize_browser_action(&self, _action: BrowserVisualAction) {
        self.native_call();
    }

    fn existing_profile_consent_focus_guard(
        &self,
        _pid: i64,
        _window_id: u64,
    ) -> Option<Box<dyn Send>> {
        self.native_call();
        None
    }

    async fn classify_browser(&self, _pid: i64) -> Result<BrowserClassification, BrowserRefusal> {
        Ok(BrowserClassification {
            is_browser: true,
            engine: BrowserEngineFamily::Chromium,
            product_kind: BrowserProduct::Chromium,
            product: None,
            channel: None,
            process_role: BrowserProcessRole::StandaloneConsumer,
            supports_cdp: true,
        })
    }

    async fn native_window(
        &self,
        _pid: i64,
        _window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal> {
        Err(self.native_call())
    }

    async fn is_only_exact_native_window(
        &self,
        _pid: i64,
        _window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal> {
        Err(self.native_call())
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        Ok(Some(OwnedEndpoint {
            ws_url: self
                .endpoint_override
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| self.ws_url.clone()),
            http_port: Some(self.port),
            transport: EndpointTransport::LegacyJsonVersion,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: Some("headless fixture socket owner".into()),
            },
        }))
    }

    async fn process_fingerprint(&self, pid: i64) -> Result<ProcessFingerprint, BrowserRefusal> {
        Ok(ProcessFingerprint {
            pid,
            start_time: Some(self.start_time.load(Ordering::SeqCst)),
            executable: Some("/fixture/chromium".into()),
        })
    }

    async fn prepare_endpoint(
        &self,
        _request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        Err(self.native_call())
    }
}

/// What the page "remembers" so a fresh snapshot can read typed and
/// clicked state back as DOM attributes.
#[derive(Default)]
struct Page {
    typed: String,
    clicked: Vec<String>,
}

fn readback_handler(state: SharedState, page: Arc<StdMutex<Page>>) -> MockHandler {
    fn patch(node: &mut Value, page: &Page) {
        let backend = node["backendNodeId"].as_i64();
        if let Some(attrs) = node.get_mut("attributes").and_then(Value::as_array_mut) {
            if backend == Some(20) && !page.typed.is_empty() {
                attrs.extend([json!("value"), json!(page.typed)]);
            }
            if backend == Some(10) && page.clicked.iter().any(|id| id == "obj-10") {
                attrs.extend([json!("aria-label"), json!("clicked")]);
            }
        }
        for key in ["root", "children", "shadowRoots", "contentDocument"] {
            match node.get_mut(key) {
                Some(Value::Array(children)) => children.iter_mut().for_each(|c| patch(c, page)),
                Some(child @ Value::Object(_)) => patch(child, page),
                _ => {}
            }
        }
    }
    let inner = fixture_handler(state);
    Arc::new(move |call| {
        let mut reply = inner(call);
        let mut page = page.lock().unwrap();
        match call.method.as_str() {
            "Input.insertText" => page
                .typed
                .push_str(call.params["text"].as_str().unwrap_or_default()),
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"] == "function() { this.click(); }" =>
            {
                page.clicked
                    .push(call.params["objectId"].as_str().unwrap_or_default().into());
            }
            "DOM.getDocument" => {
                if let Ok(document) = &mut reply.result {
                    patch(document, &page);
                }
            }
            _ => {}
        }
        reply
    })
}

struct Harness {
    engine: Arc<BrowserEngine>,
    platform: Arc<HeadlessPlatform>,
    state: SharedState,
    _server: MockCdpServer,
    _root: tempfile::TempDir,
}

async fn harness() -> Harness {
    let root = tempfile::tempdir().unwrap();
    super::prepare::TEST_PROFILE_ROOT.with(|slot| *slot.borrow_mut() = Some(root.path().into()));
    let state: SharedState = Arc::new(StdMutex::new(FixtureState::default()));
    let server = MockCdpServer::start(readback_handler(state.clone(), Default::default())).await;
    let ws_url = server.ws_url();
    let port = ws_url
        .trim_start_matches("ws://127.0.0.1:")
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let platform = Arc::new(HeadlessPlatform {
        port,
        ws_url,
        endpoint_override: StdMutex::new(None),
        start_time: AtomicU64::new(1),
        native_calls: AtomicUsize::new(0),
        spawned_args: StdMutex::new(Vec::new()),
        signals: Default::default(),
    });
    Harness {
        engine: BrowserEngine::new(platform.clone()),
        platform,
        state,
        _server: server,
        _root: root,
    }
}

fn unique(label: &str) -> String {
    format!("{label}-{}", uuid::Uuid::new_v4())
}

fn refusal_code(result: &ToolResult) -> String {
    structured(result)["refusal"]["code"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a refusal: {}", structured(result)))
        .to_owned()
}

async fn prepare(h: &Harness, extra: Value) -> ToolResult {
    let mut args = json!({
        "headless": true,
        "allow_launch": true,
        "profile": {"mode": "isolated_new"},
    });
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    BrowserPrepareTool::new(h.engine.clone()).invoke(args).await
}

async fn prepare_ok(h: &Harness, session: &str) -> (String, PathBuf, String) {
    let result = prepare(h, json!({"session": session})).await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["attachment"]["kind"], "driver_owned_headless", "{s}");
    let anchor = s["attachment"]["headless_target"]
        .as_str()
        .unwrap()
        .to_owned();
    let args = h
        .platform
        .spawned_args
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let profile = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--user-data-dir="))
        .map(PathBuf::from)
        .unwrap();
    (anchor, profile, s["prepared_pid"].to_string())
}

async fn bind_headless(h: &Harness, session: &str, anchor: &str) -> ToolResult {
    GetBrowserStateTool::new(h.engine.clone())
        .invoke(json!({"headless_target": anchor, "session": session}))
        .await
}

async fn navigate(h: &Harness, session: &str, target: &str, tab: &str) -> ToolResult {
    BrowserNavigateTool::new(h.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "session": session,
            "url": "https://fixture.test/form",
        }))
        .await
}

async fn snapshot(h: &Harness, session: &str, target: &str, tab: &str, format: &str) -> Value {
    let result = GetBrowserStateTool::new(h.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "session": session,
            "snapshot_format": format,
        }))
        .await;
    let s = structured(&result).clone();
    assert_eq!(s["status"], "ok", "{s}");
    s
}

async fn bound(h: &Harness, session: &str) -> (String, String, String, PathBuf, String) {
    let (anchor, profile, pid) = prepare_ok(h, session).await;
    let result = bind_headless(h, session, &anchor).await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    let target = s["target_id"].as_str().unwrap().to_owned();
    let tab = s["tabs"][0]["tab_id"].as_str().unwrap().to_owned();
    (anchor, target, tab, profile, pid)
}

#[tokio::test]
async fn headless_prepare_binds_by_anchor_and_does_typed_dom_work_without_native_calls() {
    let h = harness().await;
    let session = unique("headless-flow");
    let (anchor, profile, _) = prepare_ok(&h, &session).await;
    assert!(anchor.starts_with("hl-"));
    assert!(profile.is_dir());
    let args = h.platform.spawned_args.lock().unwrap()[0].clone();
    for flag in ["--headless=new", "--remote-debugging-port=0"] {
        assert!(
            args.iter().any(|arg| arg == flag),
            "missing {flag}: {args:?}"
        );
    }

    let bind = bind_headless(&h, &session, &anchor).await;
    let s = structured(&bind);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["binding_route"], "driver_owned_headless");
    assert_eq!(s["binding_quality"], "exact");
    assert_eq!(s["mutation_allowed"], true);
    assert_eq!(s["endpoint_access_class"], "driver_owned");
    assert!(s["native_title"].is_null());
    let target = s["target_id"].as_str().unwrap().to_owned();
    let tab = s["tabs"][0]["tab_id"].as_str().unwrap().to_owned();

    snapshot(&h, &session, &target, &tab, "semantic_v2").await;
    let nav = navigate(&h, &session, &target, &tab).await;
    assert_eq!(structured(&nav)["status"], "ok", "{}", structured(&nav));

    let snap = snapshot(&h, &session, &target, &tab, "dom_refs_v1").await;
    let typed = BrowserTypeTool::new(h.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "session": session,
            "ref": ref_of(&snap, "main", "Shadow Input"), "text": "hello headless",
        }))
        .await;
    assert_eq!(structured(&typed)["status"], "ok", "{}", structured(&typed));
    let clicked = BrowserClickTool::new(h.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "session": session,
            "ref": ref_of(&snap, "main", "main-btn"), "input_route": "dom_event",
        }))
        .await;
    assert_eq!(
        structured(&clicked)["status"],
        "ok",
        "{}",
        structured(&clicked)
    );

    let fresh = snapshot(&h, &session, &target, &tab, "dom_refs_v1").await;
    ref_of(&fresh, "main", "value=hello headless");
    ref_of(&fresh, "main", "aria-label=clicked");
    assert_eq!(h.platform.native_calls.load(Ordering::SeqCst), 0);
    let calls = h.state.lock().unwrap().calls.clone();
    assert!(!calls
        .iter()
        .any(|(_, method, _)| method == "Page.bringToFront" || method == "Target.activateTarget"));
    crate::session::fire_session_end(&session);
}

#[tokio::test]
async fn headless_prepare_refuses_out_of_scope_requests() {
    let h = harness().await;
    let session = unique("headless-refusals");
    for (extra, code) in [
        (
            json!({"strategy": {"kind": "existing_profile"}, "pid": 1, "window_id": 7, "profile": null, "allow_launch": false}),
            "browser_consent_required",
        ),
        (json!({"allow_launch": false}), "browser_consent_required"),
        (json!({"profile": null}), "browser_consent_required"),
        (
            json!({"profile": {"mode": "isolated_named", "name": "kept"}}),
            "browser_route_unavailable",
        ),
        (json!({"pid": 1}), "browser_consent_required"),
        (json!({"window_id": 7}), "browser_consent_required"),
    ] {
        let mut extra = extra;
        extra["session"] = json!(session);
        let result = prepare(&h, extra.clone()).await;
        assert_eq!(refusal_code(&result), code, "{extra}");
    }
    let bad = prepare(&h, json!({"session": session, "headless": "yes"})).await;
    assert_eq!(bad.is_error, Some(true));
    assert!(h.platform.spawned_args.lock().unwrap().is_empty());
    assert_eq!(h.platform.native_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn headless_anchor_refuses_forged_unknown_and_other_session_use() {
    let h = harness().await;
    let owner = unique("headless-owner");
    let other = unique("headless-other");
    let (anchor, _, _) = prepare_ok(&h, &owner).await;

    let stolen = bind_headless(&h, &other, &anchor).await;
    assert_eq!(refusal_code(&stolen), "browser_binding_stale");
    let forged = bind_headless(&h, &owner, &format!("hl-{}", uuid::Uuid::new_v4())).await;
    assert_eq!(refusal_code(&forged), "browser_binding_stale");
    let other_transport = GetBrowserStateTool::new(h.engine.clone())
        .invoke(json!({
            "headless_target": anchor, "session": owner, "_transport_session_id": "other-transport",
        }))
        .await;
    assert_eq!(refusal_code(&other_transport), "browser_binding_stale");
    let mixed = GetBrowserStateTool::new(h.engine.clone())
        .invoke(json!({"headless_target": anchor, "pid": 1, "window_id": 7, "session": owner}))
        .await;
    assert_eq!(mixed.is_error, Some(true));

    let bind = bind_headless(&h, &owner, &anchor).await;
    let target = structured(&bind)["target_id"].as_str().unwrap().to_owned();
    let tab = structured(&bind)["tabs"][0]["tab_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let cross = navigate(&h, &other, &target, &tab).await;
    assert_eq!(refusal_code(&cross), "browser_binding_stale");
    assert_eq!(h.platform.native_calls.load(Ordering::SeqCst), 0);
    crate::session::fire_session_end(&owner);
}

#[tokio::test]
async fn headless_mutation_refuses_after_identity_endpoint_or_session_drift() {
    let h = harness().await;
    let session = unique("headless-drift");
    let (_, target, tab, profile, pid) = bound(&h, &session).await;

    h.platform.start_time.store(2, Ordering::SeqCst);
    assert_eq!(
        refusal_code(&navigate(&h, &session, &target, &tab).await),
        "browser_binding_stale"
    );
    h.platform.start_time.store(1, Ordering::SeqCst);
    let ok = navigate(&h, &session, &target, &tab).await;
    assert_eq!(structured(&ok)["status"], "ok", "{}", structured(&ok));

    *h.platform.endpoint_override.lock().unwrap() =
        Some("ws://127.0.0.1:1/devtools/browser/replaced".into());
    assert_eq!(
        refusal_code(&navigate(&h, &session, &target, &tab).await),
        "browser_binding_stale"
    );
    *h.platform.endpoint_override.lock().unwrap() = None;

    crate::session::fire_session_end(&session);
    let ended = navigate(&h, &session, &target, &tab).await;
    assert_eq!(refusal_code(&ended), "browser_binding_stale");
    assert!(
        !profile.exists(),
        "session end must remove the isolated_new profile"
    );
    assert_eq!(
        *h.platform.signals.lock().unwrap(),
        [format!("group:{pid}"), format!("kill:{pid}")]
    );
    assert_eq!(h.platform.native_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn headless_record_removal_refuses_mutation_even_with_a_live_target_capability() {
    let h = harness().await;
    let session = unique("headless-record");
    let (_, target, tab, profile, _) = bound(&h, &session).await;
    // Explicit cleanup drops the launch record but not the capability store.
    h.engine.cleanup_prepared_session(&session);
    assert!(!profile.exists());
    assert_eq!(
        refusal_code(&navigate(&h, &session, &target, &tab).await),
        "browser_consent_required"
    );
    crate::session::fire_session_end(&session);
}

#[tokio::test]
async fn headless_cleanup_signals_only_its_own_process_group_and_removes_profile() {
    let h = harness().await;
    let first = unique("headless-cleanup-a");
    let second = unique("headless-cleanup-b");
    let (_, first_profile, first_pid) = prepare_ok(&h, &first).await;
    let (_, second_profile, second_pid) = prepare_ok(&h, &second).await;

    crate::session::fire_session_end(&first);
    assert!(!first_profile.exists());
    assert!(
        second_profile.is_dir(),
        "another session's profile is untouched"
    );
    assert_eq!(
        *h.platform.signals.lock().unwrap(),
        [format!("group:{first_pid}"), format!("kill:{first_pid}")]
    );

    crate::session::fire_session_end(&second);
    assert!(!second_profile.exists());
    assert_eq!(
        h.platform.signals.lock().unwrap()[2..],
        [format!("group:{second_pid}"), format!("kill:{second_pid}")]
    );
}

#[test]
fn group_cleanup_never_signals_a_reaped_leader() {
    // A leader reaped during launch may have had its numeric id reused by an
    // unrelated process group; only a live, unreaped leader is signalled.
    let signals = Arc::new(StdMutex::new(Vec::new()));
    let mut exited = FakeBrowser {
        pid: NEXT_FAKE_PID.fetch_add(1, Ordering::SeqCst),
        signals: signals.clone(),
        exited: true,
    };
    assert!(!super::prepare::signal_group_if_unreaped(&mut exited));
    assert!(signals.lock().unwrap().is_empty());
    let mut running = FakeBrowser {
        pid: NEXT_FAKE_PID.fetch_add(1, Ordering::SeqCst),
        signals: signals.clone(),
        exited: false,
    };
    assert!(super::prepare::signal_group_if_unreaped(&mut running));
    assert_eq!(*signals.lock().unwrap(), [format!("group:{}", running.pid)]);
}
