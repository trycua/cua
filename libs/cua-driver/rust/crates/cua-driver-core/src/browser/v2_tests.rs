//! End-to-end tests for the v2 DOM-ref slice against a deterministic
//! mock CDP endpoint: composed shadow DOM, same-process iframes,
//! capability-tested OOPIFs with containment, frame/document identity
//! revalidation, navigation invalidation, and unproven-capability
//! omission/refusal.

use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::Tool;

use super::engine::BrowserEngine;
use super::mock_cdp::{MockCdpServer, MockEvent, MockHandler, MockReply};
use super::platform::{BrowserPlatform, PrepareAction, PrepareOutcome, PrepareRequest};
use super::refusal::BrowserRefusal;
use super::tools::{BrowserClickTool, BrowserTypeTool, GetBrowserStateTool};
use super::types::{
    BrowserClassification, BrowserEngineFamily, EndpointOwnershipMethod, EndpointOwnershipProof,
    NativeOwnershipMethod, NativeOwnershipProof, NativeWindowInfo, OwnedEndpoint,
    ProcessFingerprint, Rect,
};

// ── Scripted Chromium fixture ────────────────────────────────────────────────

/// Mutable knobs + a call log, shared between the mock handler and the
/// test body. Tests flip loaders/capabilities mid-run to simulate
/// navigations, frame removal, and capability regression.
#[derive(Debug)]
struct FixtureState {
    oopif_supported: bool,
    oopif_present: bool,
    emit_rogue_attach: bool,
    main_loader: String,
    iframe_loader: String,
    oopif_loader: String,
    tab_sessions: u64,
    oopif_sessions: u64,
    /// Every incoming CDP call: (sessionId, method, params).
    calls: Vec<(Option<String>, String, Value)>,
}

impl Default for FixtureState {
    fn default() -> Self {
        Self {
            oopif_supported: true,
            oopif_present: true,
            emit_rogue_attach: false,
            main_loader: "L_MAIN_1".into(),
            iframe_loader: "L_IFRAME_1".into(),
            oopif_loader: "L_OOPIF_1".into(),
            tab_sessions: 0,
            oopif_sessions: 0,
            calls: Vec::new(),
        }
    }
}

type SharedState = Arc<StdMutex<FixtureState>>;

/// Main-frame document: a plain button, an open shadow root with an
/// input, a user-agent shadow root (must be skipped), a same-process
/// iframe with a button, and an OOPIF placeholder (no contentDocument).
fn main_document() -> Value {
    json!({
        "root": {
            "nodeType": 9,
            "nodeName": "#document",
            "documentURL": "https://fixture.test/",
            "frameId": "F_MAIN",
            "children": [{
                "nodeType": 1,
                "nodeName": "HTML",
                "backendNodeId": 1,
                "children": [
                    {
                        "nodeType": 1,
                        "nodeName": "BUTTON",
                        "backendNodeId": 10,
                        "attributes": ["id", "main-btn"],
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "DIV",
                        "backendNodeId": 11,
                        "shadowRoots": [{
                            "nodeType": 11,
                            "nodeName": "#document-fragment",
                            "shadowRootType": "open",
                            "backendNodeId": 12,
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "INPUT",
                                "backendNodeId": 20,
                                "attributes": ["aria-label", "Shadow Input", "type", "text"],
                            }]
                        }]
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "INPUT",
                        "backendNodeId": 21,
                        "attributes": ["type", "text"],
                        "shadowRoots": [{
                            "nodeType": 11,
                            "nodeName": "#document-fragment",
                            "shadowRootType": "user-agent",
                            "backendNodeId": 23,
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "DIV",
                                "backendNodeId": 22,
                                "attributes": ["role", "button"],
                            }]
                        }]
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "IFRAME",
                        "backendNodeId": 13,
                        "frameId": "F_IFRAME",
                        "contentDocument": {
                            "nodeType": 9,
                            "nodeName": "#document",
                            "frameId": "F_IFRAME",
                            "children": [{
                                "nodeType": 1,
                                "nodeName": "BUTTON",
                                "backendNodeId": 30,
                                "attributes": ["id", "inner-btn"],
                            }]
                        }
                    },
                    {
                        "nodeType": 1,
                        "nodeName": "IFRAME",
                        "backendNodeId": 14,
                        "frameId": "F_OOPIF",
                    }
                ]
            }]
        }
    })
}

fn oopif_document() -> Value {
    json!({
        "root": {
            "nodeType": 9,
            "nodeName": "#document",
            "documentURL": "https://ads.example/frame",
            "frameId": "F_OOPIF",
            "children": [{
                "nodeType": 1,
                "nodeName": "HTML",
                "backendNodeId": 90,
                "children": [{
                    "nodeType": 1,
                    "nodeName": "INPUT",
                    "backendNodeId": 100,
                    "attributes": ["id", "ad-input", "type", "text"],
                }]
            }]
        }
    })
}

fn fixture_handler(state: SharedState) -> MockHandler {
    Arc::new(move |call| {
        let mut st = state.lock().unwrap();
        st.calls.push((
            call.session_id.clone(),
            call.method.clone(),
            call.params.clone(),
        ));
        let sess = call.session_id.clone().unwrap_or_default();
        let is_tab = sess.starts_with("tab-sess-");
        let is_oopif = sess.starts_with("oopif-sess-");

        match call.method.as_str() {
            "Target.getTargets" => MockReply::ok(json!({
                "targetInfos": [{
                    "targetId": "T1",
                    "type": "page",
                    "title": "Fixture",
                    "url": "https://fixture.test/",
                    "attached": false,
                }]
            })),
            "Browser.getWindowForTarget" => MockReply::ok(json!({ "windowId": 11 })),
            "Browser.getWindowBounds" => MockReply::ok(json!({
                "bounds": { "left": 0.0, "top": 0.0, "width": 800.0, "height": 600.0 }
            })),
            "Target.attachToTarget" => {
                st.tab_sessions += 1;
                MockReply::ok(json!({ "sessionId": format!("tab-sess-{}", st.tab_sessions) }))
            }
            "Page.getFrameTree" if is_tab => MockReply::ok(json!({
                "frameTree": {
                    "frame": {
                        "id": "F_MAIN",
                        "loaderId": st.main_loader.clone(),
                        "url": "https://fixture.test/",
                    },
                    "childFrames": [{
                        "frame": {
                            "id": "F_IFRAME",
                            "parentId": "F_MAIN",
                            "loaderId": st.iframe_loader.clone(),
                            "url": "https://fixture.test/inner",
                        }
                    }]
                }
            })),
            "Page.getFrameTree" if is_oopif => MockReply::ok(json!({
                "frameTree": {
                    "frame": {
                        "id": "F_OOPIF",
                        "loaderId": st.oopif_loader.clone(),
                        "url": "https://ads.example/frame",
                    }
                }
            })),
            "DOM.getDocument" if is_tab => MockReply::ok(main_document()),
            "DOM.getDocument" if is_oopif => MockReply::ok(oopif_document()),
            "Target.setAutoAttach" if is_tab => {
                if call.params["autoAttach"].as_bool() == Some(false) {
                    return MockReply::ok(json!({}));
                }
                if !st.oopif_supported {
                    return MockReply::method_not_found("Target.setAutoAttach");
                }
                let mut events = Vec::new();
                if st.oopif_present {
                    st.oopif_sessions += 1;
                    events.push(MockEvent {
                        method: "Target.attachedToTarget".into(),
                        session_id: Some(sess.clone()),
                        params: json!({
                            "sessionId": format!("oopif-sess-{}", st.oopif_sessions),
                            "targetInfo": {
                                "targetId": "T_OOPIF",
                                "type": "iframe",
                                "url": "https://ads.example/frame",
                                "attached": true,
                            },
                            "waitingForDebugger": false,
                        }),
                    });
                }
                if st.emit_rogue_attach {
                    // A child announced on a session we never proved.
                    events.push(MockEvent {
                        method: "Target.attachedToTarget".into(),
                        session_id: Some("unproven-sess".into()),
                        params: json!({
                            "sessionId": "rogue-sess",
                            "targetInfo": {
                                "targetId": "T_ROGUE",
                                "type": "iframe",
                                "url": "https://evil.example/",
                                "attached": true,
                            },
                            "waitingForDebugger": false,
                        }),
                    });
                    // A popup page target beneath the tab — wrong type.
                    events.push(MockEvent {
                        method: "Target.attachedToTarget".into(),
                        session_id: Some(sess.clone()),
                        params: json!({
                            "sessionId": "popup-sess",
                            "targetInfo": {
                                "targetId": "T_POPUP",
                                "type": "page",
                                "url": "https://fixture.test/popup",
                                "attached": true,
                            },
                            "waitingForDebugger": false,
                        }),
                    });
                }
                MockReply::ok(json!({})).with_events(events)
            }
            "DOM.scrollIntoViewIfNeeded" => MockReply::ok(json!({})),
            "DOM.getBoxModel" => {
                let backend = call.params["backendNodeId"].as_i64().unwrap_or(0);
                let known = if is_oopif {
                    backend == 100
                } else {
                    [10, 20, 21, 30].contains(&backend)
                };
                if known {
                    let (x, y) = ((backend * 10) as f64, (backend * 10) as f64);
                    MockReply::ok(json!({
                        "model": {
                            "content": [x, y, x + 20.0, y, x + 20.0, y + 10.0, x, y + 10.0]
                        }
                    }))
                } else {
                    MockReply::err(-32000, "No node with given id")
                }
            }
            "DOM.focus"
            | "Input.dispatchMouseEvent"
            | "Input.insertText"
            | "Input.dispatchKeyEvent" => MockReply::ok(json!({})),
            "DOM.resolveNode" => MockReply::ok(json!({
                "object": { "objectId": format!("obj-{}", call.params["backendNodeId"]) }
            })),
            "Runtime.callFunctionOn" => MockReply::ok(json!({ "result": { "value": true } })),
            other => MockReply::method_not_found(other),
        }
    })
}

/// Platform adapter pointing at the fixture endpoint: pid 1 is a
/// Chromium browser owning native window 7 at (0,0,800,600).
struct FixturePlatform {
    ws_url: String,
    trusted_input_limited: bool,
}

#[async_trait]
impl BrowserPlatform for FixturePlatform {
    fn standalone_trusted_input_background_limitation(&self) -> Option<&'static str> {
        self.trusted_input_limited
            .then_some("fixture trusted input raises the standalone window")
    }

    async fn classify_browser(&self, _pid: i64) -> Result<BrowserClassification, BrowserRefusal> {
        Ok(BrowserClassification {
            is_browser: true,
            engine: BrowserEngineFamily::Chromium,
            product: Some("MockChrome".into()),
            channel: Some("stable".into()),
            supports_cdp: true,
        })
    }

    async fn native_window(
        &self,
        pid: i64,
        window_id: u64,
    ) -> Result<NativeWindowInfo, BrowserRefusal> {
        Ok(NativeWindowInfo {
            pid,
            window_id,
            title: "Fixture - Chrome".into(),
            bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
            geometry_exact: true,
            ownership: NativeOwnershipProof {
                method: NativeOwnershipMethod::WindowServerOwner,
                owner_pid: pid,
                detail: None,
            },
        })
    }

    async fn is_only_exact_native_window(
        &self,
        _pid: i64,
        _window_id: u64,
    ) -> Result<Option<bool>, BrowserRefusal> {
        Ok(Some(true))
    }

    async fn discover_owned_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        Ok(Some(OwnedEndpoint {
            ws_url: self.ws_url.clone(),
            http_port: None,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                detail: None,
            },
        }))
    }

    async fn process_fingerprint(&self, pid: i64) -> Result<ProcessFingerprint, BrowserRefusal> {
        Ok(ProcessFingerprint {
            pid,
            start_time: Some(1),
            executable: None,
        })
    }

    async fn prepare_endpoint(
        &self,
        _request: PrepareRequest,
    ) -> Result<PrepareOutcome, BrowserRefusal> {
        Ok(PrepareOutcome {
            action: PrepareAction::NoOp,
            endpoint: None,
            message: "fixture: nothing to do".into(),
            prepared_pid: None,
            side_effects: Default::default(),
        })
    }
}

// ── Scaffolding ──────────────────────────────────────────────────────────────

struct Fixture {
    state: SharedState,
    // Kept alive for the test's duration; dropping it kills the endpoint.
    _server: MockCdpServer,
    engine: Arc<BrowserEngine>,
}

async fn fixture_with(configure: impl FnOnce(&mut FixtureState)) -> Fixture {
    fixture_with_platform(configure, false).await
}

async fn fixture_with_platform(
    configure: impl FnOnce(&mut FixtureState),
    trusted_input_limited: bool,
) -> Fixture {
    let mut initial = FixtureState::default();
    configure(&mut initial);
    let state = Arc::new(StdMutex::new(initial));
    let server = MockCdpServer::start(fixture_handler(state.clone())).await;
    let engine = BrowserEngine::new(Arc::new(FixturePlatform {
        ws_url: server.ws_url(),
        trusted_input_limited,
    }));
    Fixture {
        state,
        _server: server,
        engine,
    }
}

async fn fixture() -> Fixture {
    fixture_with(|_| {}).await
}

fn structured(result: &ToolResult) -> &Value {
    result
        .structured_content
        .as_ref()
        .expect("structured content")
}

const SESSION: &str = "run-v2";

async fn bind(f: &Fixture) -> (String, String) {
    let tool = GetBrowserStateTool::new(f.engine.clone());
    let result = tool
        .invoke(json!({ "pid": 1, "window_id": 7, "session": SESSION }))
        .await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "bind must succeed: {s}");
    assert_eq!(s["binding_quality"], "exact");
    let target_id = s["target_id"].as_str().unwrap().to_owned();
    let tab_id = s["tabs"][0]["tab_id"].as_str().unwrap().to_owned();
    (target_id, tab_id)
}

async fn snapshot(f: &Fixture, target_id: &str, tab_id: &str) -> Value {
    let tool = GetBrowserStateTool::new(f.engine.clone());
    let result = tool
        .invoke(json!({ "target_id": target_id, "tab_id": tab_id, "session": SESSION }))
        .await;
    structured(&result).clone()
}

/// The `ref` string of the first snapshot entry in the given frame kind
/// with the given backing label fragment (or any, if empty).
fn ref_of(snapshot: &Value, frame: &str, label_fragment: &str) -> String {
    snapshot["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["frame"] == frame
                && (label_fragment.is_empty()
                    || r["label"].as_str().unwrap_or("").contains(label_fragment))
        })
        .unwrap_or_else(|| panic!("no {frame} ref with label ~{label_fragment:?}: {snapshot}"))
        ["ref"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn recorded_calls(f: &Fixture, method: &str) -> Vec<(Option<String>, Value)> {
    f.state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(_, m, _)| m == method)
        .map(|(s, _, p)| (s.clone(), p.clone()))
        .collect()
}

// ── Snapshot composition ─────────────────────────────────────────────────────

#[tokio::test]
async fn snapshot_composes_shadow_iframe_and_oopif_refs() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;

    assert_eq!(snap["status"], "ok", "{snap}");
    let frames: Vec<&str> = snap["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["frame"].as_str().unwrap())
        .collect();
    assert_eq!(
        frames,
        vec!["main", "main", "main", "iframe", "oopif"],
        "main button + shadow input + plain input, iframe button, oopif input: {snap}"
    );
    assert_eq!(snap["oopif"]["status"], "attached");
    assert_eq!(snap["oopif"]["frames"], 1);
    assert_eq!(snap["truncated"], false);

    // Composed shadow content keeps its labels; user-agent shadow
    // internals (backend 22, role=button) must not have been minted.
    let labels: Vec<&str> = snap["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["label"].as_str().unwrap_or(""))
        .collect();
    assert!(labels.iter().any(|l| l.contains("Shadow Input")), "{snap}");
    assert!(
        !labels.iter().any(|l| l.contains("role=button")),
        "user-agent shadow content leaked: {snap}"
    );
}

#[tokio::test]
async fn unproven_oopif_capability_is_omitted_not_guessed() {
    let f = fixture_with(|st| st.oopif_supported = false).await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;

    assert_eq!(
        snap["status"], "ok",
        "capability gap is not an error: {snap}"
    );
    assert_eq!(snap["oopif"]["status"], "unsupported");
    assert_eq!(snap["oopif"]["frames"], 0);
    let frames: Vec<&str> = snap["refs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["frame"].as_str().unwrap())
        .collect();
    assert_eq!(
        frames,
        vec!["main", "main", "main", "iframe"],
        "OOPIF content omitted, everything provable kept: {snap}"
    );
}

#[tokio::test]
async fn rogue_attach_announcements_are_contained() {
    let f = fixture_with(|st| st.emit_rogue_attach = true).await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;

    let oopif_refs: Vec<&Value> = snap["refs"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["frame"] == "oopif")
        .collect();
    assert_eq!(
        oopif_refs.len(),
        1,
        "only the child attached beneath the proven tab session mints refs: {snap}"
    );
    assert_eq!(snap["oopif"]["frames"], 1);

    // The rogue/popup sessions must never have been spoken to.
    let touched: Vec<String> = f
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter_map(|(s, _, _)| s.clone())
        .collect();
    assert!(
        !touched
            .iter()
            .any(|s| s == "rogue-sess" || s == "popup-sess"),
        "core issued commands to an unproven session: {touched:?}"
    );
}

// ── Mutation routing + frame identity revalidation ──────────────────────────

#[tokio::test]
async fn click_routes_oopif_refs_through_the_contained_child_session() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let oopif_ref = ref_of(&snap, "oopif", "ad-input");

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": oopif_ref, "session": SESSION
        }))
        .await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["frame"], "oopif");
    // Box-model center of backend 100 in the child session's space.
    assert_eq!(s["x"], 1010.0);
    assert_eq!(s["y"], 1005.0);

    let mouse = recorded_calls(&f, "Input.dispatchMouseEvent");
    assert!(!mouse.is_empty());
    assert!(
        mouse
            .iter()
            .all(|(sess, _)| sess.as_deref().unwrap_or("").starts_with("oopif-sess-")),
        "OOPIF input must dispatch on the child session: {mouse:?}"
    );
}

#[tokio::test]
async fn click_validates_same_process_iframe_loader_and_uses_the_tab_session() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let iframe_ref = ref_of(&snap, "iframe", "inner-btn");

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": iframe_ref, "session": SESSION
        }))
        .await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["frame"], "iframe");

    let mouse = recorded_calls(&f, "Input.dispatchMouseEvent");
    assert!(
        mouse
            .iter()
            .all(|(sess, _)| sess.as_deref().unwrap_or("").starts_with("tab-sess-")),
        "same-process iframe input stays on the tab session: {mouse:?}"
    );
    assert!(
        !recorded_calls(&f, "Page.getFrameTree").is_empty(),
        "frame identity must have been re-proven"
    );
}

#[tokio::test]
async fn trusted_click_refuses_when_standalone_background_posture_is_unavailable() {
    let f = fixture_with_platform(|_| {}, true).await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let main_ref = ref_of(&snap, "main", "main-btn");

    let trusted = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": main_ref,
            "input_route": "trusted", "session": SESSION
        }))
        .await;
    assert_eq!(
        structured(&trusted)["refusal"]["code"],
        "browser_input_trust_unavailable"
    );
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());

    let synthetic = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": main_ref,
            "input_route": "dom_event", "session": SESSION
        }))
        .await;
    assert_eq!(structured(&synthetic)["status"], "ok");
    assert!(!recorded_calls(&f, "Runtime.callFunctionOn").is_empty());
}

#[tokio::test]
async fn main_frame_navigation_invalidates_refs_via_loader_identity() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let main_ref = ref_of(&snap, "main", "main-btn");

    f.state.lock().unwrap().main_loader = "L_MAIN_2".into();

    let click = BrowserClickTool::new(f.engine.clone());
    let result = click
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": main_ref, "session": SESSION
        }))
        .await;
    assert_eq!(
        structured(&result)["refusal"]["code"],
        "browser_ref_stale",
        "loader change means a new document"
    );
    assert!(
        recorded_calls(&f, "Input.dispatchMouseEvent").is_empty(),
        "no input may reach a document that cannot be re-proven"
    );

    // The whole snapshot namespace was invalidated, so a retry refuses
    // at resolution already.
    let retry = click
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": main_ref, "session": SESSION
        }))
        .await;
    assert_eq!(structured(&retry)["refusal"]["code"], "browser_ref_stale");
}

#[tokio::test]
async fn same_process_iframe_navigation_invalidates_its_refs() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let iframe_ref = ref_of(&snap, "iframe", "inner-btn");

    f.state.lock().unwrap().iframe_loader = "L_IFRAME_2".into();

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": iframe_ref, "session": SESSION
        }))
        .await;
    assert_eq!(structured(&result)["refusal"]["code"], "browser_ref_stale");
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());
}

#[tokio::test]
async fn oopif_navigation_invalidates_its_refs() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let oopif_ref = ref_of(&snap, "oopif", "ad-input");

    f.state.lock().unwrap().oopif_loader = "L_OOPIF_2".into();

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": oopif_ref, "session": SESSION
        }))
        .await;
    assert_eq!(structured(&result)["refusal"]["code"], "browser_ref_stale");
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());
}

#[tokio::test]
async fn removed_oopif_frame_is_stale_not_guessed() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let oopif_ref = ref_of(&snap, "oopif", "ad-input");

    f.state.lock().unwrap().oopif_present = false;

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": oopif_ref, "session": SESSION
        }))
        .await;
    assert_eq!(structured(&result)["refusal"]["code"], "browser_ref_stale");
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());
}

#[tokio::test]
async fn oopif_capability_regression_refuses_rather_than_reroutes() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let oopif_ref = ref_of(&snap, "oopif", "ad-input");

    f.state.lock().unwrap().oopif_supported = false;

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": oopif_ref, "session": SESSION
        }))
        .await;
    assert_eq!(
        structured(&result)["refusal"]["code"],
        "browser_route_unavailable",
        "a lost capability is a refusal, never a fallback to the wrong session"
    );
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());
}

// ── Typing routes ────────────────────────────────────────────────────────────

#[tokio::test]
async fn typing_into_composed_shadow_input_uses_the_tab_session() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let shadow_ref = ref_of(&snap, "main", "Shadow Input");

    let result = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": shadow_ref,
            "text": "hi", "session": SESSION
        }))
        .await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["frame"], "main");

    let inserts = recorded_calls(&f, "Input.insertText");
    assert_eq!(inserts.len(), 1);
    assert!(inserts[0].0.as_deref().unwrap().starts_with("tab-sess-"));
    assert_eq!(inserts[0].1["text"], "hi");
    assert!(
        !recorded_calls(&f, "Runtime.callFunctionOn").is_empty(),
        "editability must be checked on the node itself"
    );
}

#[tokio::test]
async fn typing_into_an_oopif_input_routes_to_the_child_session() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let oopif_ref = ref_of(&snap, "oopif", "ad-input");

    let result = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target, "tab_id": tab, "ref": oopif_ref,
            "text": "q", "session": SESSION
        }))
        .await;
    let s = structured(&result);
    assert_eq!(s["status"], "ok", "{s}");
    assert_eq!(s["frame"], "oopif");

    let inserts = recorded_calls(&f, "Input.insertText");
    assert_eq!(inserts.len(), 1);
    assert!(
        inserts[0].0.as_deref().unwrap().starts_with("oopif-sess-"),
        "typing must land in the contained child session: {inserts:?}"
    );
    let focuses = recorded_calls(&f, "DOM.focus");
    assert!(focuses
        .iter()
        .all(|(sess, _)| sess.as_deref().unwrap().starts_with("oopif-sess-")));
}
