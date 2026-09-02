//! End-to-end tests for the v2 DOM-ref slice against a deterministic
//! mock CDP endpoint: composed shadow DOM, same-process iframes,
//! capability-tested OOPIFs with containment, frame/document identity
//! revalidation, navigation invalidation, and unproven-capability
//! omission/refusal.

use std::io::Cursor;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex as StdMutex,
};
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::{json, Value};

use crate::action_record::ActionExecutionRecord;
use crate::credentials::{
    BrowserSecretTargetConstraint, CredentialBindingSpec, CredentialDeliveryOutcome,
    CredentialField, CredentialProvider, CredentialProviderClass, CredentialProviderError,
    CredentialProviderId, PrivateCredentialField, PrivateCredentialFieldLocator,
    ProviderFillSession, ProviderHealth, ProviderHealthState, ProviderReleaseId,
    ProviderReleaseRequest, ReleasePlan, SafeCredentialLabel, SecretBroker, SecretBrokerBuilder,
    SecretLease, SecretReleaseAuthorization, SecretTargetConstraint, UserPresenceHandle,
};
use crate::protocol::{Content, ToolResult};
use crate::tool::Tool;

use super::engine::BrowserEngine;
use super::mock_cdp::{MockCdpServer, MockEvent, MockHandler, MockReply};
use super::platform::{
    BrowserConsentOutcome, BrowserConsentRequest, BrowserPlatform, ExistingProfileSetupOutcome,
    ExistingProfileSetupRequest, PrepareAction, PrepareOutcome, PrepareRequest,
};
use super::pointer::BrowserPointerTool;
use super::refusal::BrowserRefusal;
use super::tools::{
    browser_protected_resource_scope, BrowserClickTool, BrowserNavigateTool, BrowserPrepareTool,
    BrowserTypeTool, GetBrowserStateTool, TypeSecretTool,
};
use super::types::{
    BrowserClassification, BrowserEngineFamily, BrowserProcessRole, BrowserProduct,
    EndpointOwnershipMethod, EndpointOwnershipProof, EndpointTransport, NativeOwnershipMethod,
    NativeOwnershipProof, NativeWindowInfo, OwnedEndpoint, ProcessFingerprint, Rect,
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
    main_url: String,
    main_loader: String,
    iframe_loader: String,
    oopif_loader: String,
    tab_sessions: u64,
    oopif_sessions: u64,
    fail_key_down_after: Option<usize>,
    completed_key_pairs: usize,
    semantic_large_page: bool,
    semantic_full_dom_fails: bool,
    semantic_full_dom_times_out: bool,
    semantic_truncated_dom: bool,
    screenshot_data: String,
    viewport_css_width: f64,
    viewport_css_height: f64,
    tab_visible: bool,
    password_target_live: bool,
    secret_beforeinput_observed: bool,
    secret_input_observed: bool,
    secret_focus_exact: bool,
    secret_insert_seen: bool,
    secret_insert_error: bool,
    /// Every incoming CDP call: (sessionId, method, params).
    calls: Vec<(Option<String>, String, Value)>,
}

impl Default for FixtureState {
    fn default() -> Self {
        Self {
            oopif_supported: true,
            oopif_present: true,
            emit_rogue_attach: false,
            main_url: "https://fixture.test/".into(),
            main_loader: "L_MAIN_1".into(),
            iframe_loader: "L_IFRAME_1".into(),
            oopif_loader: "L_OOPIF_1".into(),
            tab_sessions: 0,
            oopif_sessions: 0,
            fail_key_down_after: None,
            completed_key_pairs: 0,
            semantic_large_page: false,
            semantic_full_dom_fails: false,
            semantic_full_dom_times_out: false,
            semantic_truncated_dom: false,
            screenshot_data: "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9ZJrAAAAAASUVORK5CYII=".into(),
            viewport_css_width: 800.0,
            viewport_css_height: 600.0,
            tab_visible: true,
            password_target_live: true,
            secret_beforeinput_observed: true,
            secret_input_observed: true,
            secret_focus_exact: true,
            secret_insert_seen: false,
            secret_insert_error: false,
            calls: Vec::new(),
        }
    }
}

fn synthetic_delivery_secret() -> String {
    ["CUA_", "DELIVERY_", "CANARY_", "4J8M2Q"].concat()
}

fn screenshot_png_base64(width: u32, height: u32) -> String {
    let image = RgbaImage::from_pixel(width, height, Rgba([18, 171, 52, 255]));
    let mut encoded = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut encoded, ImageFormat::Png)
        .expect("encode screenshot fixture PNG");
    BASE64.encode(encoded.into_inner())
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

/// Large application document used to prove that hidden retained controls do
/// not consume the semantic snapshot budget ahead of the active view.
fn large_semantic_document() -> Value {
    let mut children = Vec::new();
    for id in 0..320_i64 {
        children.push(json!({
            "nodeType": 1,
            "nodeName": "BUTTON",
            "backendNodeId": 1_000 + id,
            "attributes": [
                "aria-hidden", "true",
                "style", "display:none",
                "aria-label", format!("Retained control {id}"),
            ],
        }));
    }
    children.extend([
        json!({
            "nodeType": 1,
            "nodeName": "H1",
            "backendNodeId": 2_000,
            "children": [{
                "nodeType": 3,
                "nodeName": "#text",
                "nodeValue": "Visible message",
                "backendNodeId": 2_001,
            }],
        }),
        json!({
            "nodeType": 1,
            "nodeName": "P",
            "backendNodeId": 2_002,
            "children": [{
                "nodeType": 3,
                "nodeName": "#text",
                "nodeValue": "Please review the attached fixture report.",
                "backendNodeId": 2_003,
            }],
        }),
        json!({
            "nodeType": 1,
            "nodeName": "TEXTAREA",
            "backendNodeId": 2_010,
            "attributes": ["aria-label", "Reply body"],
        }),
        json!({
            "nodeType": 1,
            "nodeName": "BUTTON",
            "backendNodeId": 2_011,
            "attributes": ["aria-label", "Reply"],
        }),
        json!({
            "nodeType": 1,
            "nodeName": "INPUT",
            "backendNodeId": 2_012,
            "attributes": [
                "aria-label", "Account password",
                "type", "PaSsWoRd",
                "value", "CUA_SECURE_FIELD_CANARY_7Y2Q"
            ],
        }),
        json!({
            "nodeType": 1,
            "nodeName": "INPUT",
            "backendNodeId": 2_013,
            "attributes": [
                "aria-label", "Password hint",
                "type", "text"
            ],
        }),
    ]);
    for id in 0..305_i64 {
        children.push(json!({
            "nodeType": 1,
            "nodeName": "BUTTON",
            "backendNodeId": 3_000 + id,
            "attributes": ["aria-label", format!("Archive item {id}")],
        }));
    }
    json!({
        "root": {
            "nodeType": 9,
            "nodeName": "#document",
            "documentURL": "https://fixture.test/inbox/item-2",
            "frameId": "F_MAIN",
            "children": [{
                "nodeType": 1,
                "nodeName": "HTML",
                "backendNodeId": 999,
                "children": children,
            }]
        }
    })
}

fn truncated_semantic_document() -> Value {
    json!({
        "root": {
            "nodeType": 9,
            "nodeName": "#document",
            "documentURL": "https://fixture.test/inbox/item-2",
            "frameId": "F_MAIN",
            "children": [{
                "nodeType": 1,
                "nodeName": "HTML",
                "backendNodeId": 999,
                "childNodeCount": 629,
                "children": [],
            }]
        }
    })
}

fn large_semantic_ax_tree(frame_id: &str) -> Value {
    if frame_id != "F_MAIN" {
        return json!({"nodes": [{
            "nodeId": format!("root-{frame_id}"),
            "ignored": false,
            "role": {"value": "RootWebArea"},
            "name": {"value": "Inner fixture"},
            "childIds": []
        }]});
    }
    let mut child_ids = vec![
        "heading".to_owned(),
        "body".to_owned(),
        "editor".to_owned(),
        "reply".to_owned(),
        "account-password".to_owned(),
        "password-hint".to_owned(),
    ];
    child_ids.extend((0..305).map(|id| format!("archive-{id}")));
    let mut nodes = vec![
        json!({
            "nodeId": "root-main",
            "ignored": false,
            "role": {"value": "RootWebArea"},
            "name": {"value": "Fixture inbox"},
            "childIds": child_ids
        }),
        json!({
            "nodeId": "heading",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2000,
            "role": {"value": "heading"},
            "name": {"value": "Visible message"},
            "childIds": []
        }),
        json!({
            "nodeId": "body",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2003,
            "role": {"value": "StaticText"},
            "name": {"value": "Please review the attached fixture report."},
            "childIds": []
        }),
        json!({
            "nodeId": "editor",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2010,
            "role": {"value": "textbox"},
            "name": {"value": "Reply body"},
            "value": {"value": ""},
            "properties": [
                {"name": "editable", "value": {"value": "plaintext"}},
                {"name": "focusable", "value": {"value": true}}
            ],
            "childIds": []
        }),
        json!({
            "nodeId": "reply",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2011,
            "role": {"value": "button"},
            "name": {"value": "Reply"},
            "properties": [
                {"name": "disabled", "value": {"value": false}},
                {"name": "focusable", "value": {"value": true}}
            ],
            "childIds": []
        }),
        json!({
            "nodeId": "account-password",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2012,
            "role": {"value": "textbox"},
            "name": {"value": "Account password"},
            "value": {"value": "CUA_SECURE_FIELD_CANARY_7Y2Q"},
            "properties": [
                {"name": "editable", "value": {"value": "plaintext"}},
                {"name": "focusable", "value": {"value": true}}
            ],
            "childIds": []
        }),
        json!({
            "nodeId": "password-hint",
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 2013,
            "role": {"value": "textbox"},
            "name": {"value": "Password hint"},
            "value": {"value": "visible hint"},
            "properties": [
                {"name": "editable", "value": {"value": "plaintext"}},
                {"name": "focusable", "value": {"value": true}}
            ],
            "childIds": []
        }),
    ];
    for id in 0..305_i64 {
        nodes.push(json!({
            "nodeId": format!("archive-{id}"),
            "parentId": "root-main",
            "ignored": false,
            "backendDOMNodeId": 3_000 + id,
            "role": {"value": "button"},
            "name": {"value": format!("Archive item {id}")},
            "childIds": []
        }));
    }
    json!({"nodes": nodes})
}

fn semantic_layout_snapshot(backends: &[i64], bounds: &[[f64; 4]]) -> Value {
    let styles = (0..backends.len())
        .map(|_| json!([0, 1, 2, 3, 3]))
        .collect::<Vec<_>>();
    let node_indices = (0..backends.len()).collect::<Vec<_>>();
    let paint_orders = (1..=backends.len()).collect::<Vec<_>>();
    json!({
        "strings": ["block", "visible", "1", "auto", "pointer"],
        "documents": [{
            "nodes": {"backendNodeId": backends},
            "layout": {
                "nodeIndex": node_indices,
                "bounds": bounds,
                "styles": styles,
                "paintOrders": paint_orders
            }
        }]
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
        let mut recorded_params = call.params.clone();
        if call.method == "Input.insertText"
            && call.params["text"].as_str() == Some(synthetic_delivery_secret().as_str())
        {
            st.secret_insert_seen = true;
            recorded_params["text"] = Value::String("[secret omitted]".to_owned());
        }
        st.calls.push((
            call.session_id.clone(),
            call.method.clone(),
            recorded_params,
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
                        "url": st.main_url.clone(),
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
            "DOM.getDocument" if is_tab => {
                let depth = call.params["depth"].as_i64().unwrap_or(-1);
                if st.semantic_full_dom_times_out && (depth == -1 || depth > 8) {
                    MockReply::err(-32000, "CDP DOM.getDocument timed out after 20s")
                } else if st.semantic_full_dom_fails && (depth == -1 || depth > 8) {
                    MockReply::err(-32000, "Object reference chain is too long")
                } else if st.semantic_truncated_dom && depth == 8 {
                    MockReply::ok(truncated_semantic_document())
                } else {
                    MockReply::ok(if st.semantic_large_page {
                        large_semantic_document()
                    } else {
                        main_document()
                    })
                }
            }
            "DOM.describeNode" if is_tab && call.params["backendNodeId"] == 999 => {
                MockReply::ok(json!({
                    "node": large_semantic_document()["root"]["children"][0].clone()
                }))
            }
            "DOM.getDocument" if is_oopif => MockReply::ok(oopif_document()),
            "Accessibility.getFullAXTree" if is_tab => {
                let frame_id = call.params["frameId"].as_str().unwrap_or("F_MAIN");
                if st.semantic_large_page {
                    MockReply::ok(large_semantic_ax_tree(frame_id))
                } else {
                    MockReply::ok(json!({"nodes": []}))
                }
            }
            "Accessibility.getFullAXTree" if is_oopif => MockReply::ok(json!({"nodes": [
                {"nodeId": "oopif-root", "ignored": false,
                 "role": {"value": "RootWebArea"}, "childIds": ["oopif-input"]},
                {"nodeId": "oopif-input", "parentId": "oopif-root", "ignored": false,
                 "backendDOMNodeId": 100, "role": {"value": "textbox"},
                 "name": {"value": "Embedded input"},
                 "properties": [{"name": "editable", "value": {"value": "plaintext"}}],
                 "childIds": []}
            ]})),
            "DOMSnapshot.captureSnapshot" if is_tab => {
                if st.semantic_large_page {
                    let mut backends = vec![999, 2000, 2003, 2010, 2011, 2012, 2013];
                    let mut bounds = vec![
                        [0.0, 0.0, 800.0, 600.0],
                        [20.0, 20.0, 500.0, 40.0],
                        [20.0, 80.0, 600.0, 80.0],
                        [20.0, 180.0, 600.0, 120.0],
                        [20.0, 320.0, 100.0, 36.0],
                        [160.0, 320.0, 200.0, 36.0],
                        [380.0, 320.0, 200.0, 36.0],
                    ];
                    for id in 0..305_i64 {
                        backends.push(3_000 + id);
                        bounds.push([20.0, 2_000.0 + id as f64 * 40.0, 160.0, 30.0]);
                    }
                    MockReply::ok(semantic_layout_snapshot(&backends, &bounds))
                } else {
                    MockReply::ok(semantic_layout_snapshot(&[], &[]))
                }
            }
            "DOMSnapshot.captureSnapshot" if is_oopif => MockReply::ok(semantic_layout_snapshot(
                &[90, 100],
                &[[0.0, 0.0, 300.0, 100.0], [10.0, 10.0, 120.0, 30.0]],
            )),
            "Page.getLayoutMetrics" => MockReply::ok(json!({
                "cssVisualViewport": {
                    "pageX": 0.0,
                    "pageY": 0.0,
                    "clientWidth": st.viewport_css_width,
                    "clientHeight": st.viewport_css_height
                }
            })),
            "Runtime.evaluate"
                if call.params["expression"] == "document.visibilityState === 'visible'" =>
            {
                MockReply::ok(json!({
                    "result": {
                        "type": "boolean",
                        "value": st.tab_visible
                    }
                }))
            }
            "Page.captureScreenshot" if is_tab => {
                MockReply::ok(json!({"data": st.screenshot_data.clone()}))
            }
            "Page.createIsolatedWorld" => {
                let context = match call.params["frameId"].as_str() {
                    Some("F_MAIN") => 101,
                    Some("F_IFRAME") => 102,
                    Some("F_OOPIF") => 103,
                    _ => return MockReply::err(-32000, "Unknown frame"),
                };
                MockReply::ok(json!({ "executionContextId": context }))
            }
            "Page.navigate" if is_tab => MockReply::ok(json!({
                "frameId": "F_MAIN",
                "loaderId": "L_MAIN_NAVIGATED",
            })),
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
            "Input.dispatchKeyEvent" => {
                let event_type = call.params["type"].as_str().unwrap_or_default();
                if event_type == "keyDown" && st.fail_key_down_after == Some(st.completed_key_pairs)
                {
                    MockReply::err(-32000, "fixture key delivery failure")
                } else {
                    if event_type == "keyUp" {
                        st.completed_key_pairs += 1;
                    }
                    MockReply::ok(json!({}))
                }
            }
            "Input.insertText"
                if call.params["text"].as_str() == Some(synthetic_delivery_secret().as_str()) =>
            {
                if st.secret_insert_error {
                    MockReply::err(-32000, "fixture secret insertion failure")
                } else {
                    MockReply::ok(json!({}))
                }
            }
            "DOM.focus"
            | "Emulation.setFocusEmulationEnabled"
            | "Input.dispatchMouseEvent"
            | "Input.insertText" => MockReply::ok(json!({})),
            "DOM.resolveNode"
                if matches!(
                    call.params["objectGroup"].as_str(),
                    Some("__cua_secret_target_v1" | "__cua_secret_delivery_v1")
                ) =>
            {
                let backend = call.params["backendNodeId"].as_i64().unwrap_or_default();
                let expected_context = match backend {
                    30 => 102,
                    100 => 103,
                    _ => 101,
                };
                if call.params["executionContextId"].as_i64() != Some(expected_context) {
                    MockReply::err(-32000, "Node belongs to a different document")
                } else {
                    MockReply::ok(json!({
                        "object": { "objectId": format!("secret-obj-{backend}") }
                    }))
                }
            }
            "DOM.resolveNode" => MockReply::ok(json!({
                "object": { "objectId": format!("obj-{}", call.params["backendNodeId"]) }
            })),
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| {
                        function.contains("cua_secret_delivery_observer_install_v1")
                    }) =>
            {
                MockReply::ok(json!({ "result": { "value": st.password_target_live } }))
            }
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| {
                        function.contains("cua_secret_delivery_focus_ready_v1")
                    }) =>
            {
                MockReply::ok(json!({
                    "result": {
                        "value": st.password_target_live && st.secret_focus_exact
                    }
                }))
            }
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| function.contains("cua_secret_delivery_select_v1")) =>
            {
                MockReply::ok(json!({
                    "result": {
                        "value": st.password_target_live && st.secret_focus_exact
                    }
                }))
            }
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| {
                        function.contains("cua_secret_delivery_observer_verify_v1")
                    }) =>
            {
                MockReply::ok(json!({
                    "result": {
                        "value": st.password_target_live
                            && st.secret_focus_exact
                            && st.secret_beforeinput_observed
                            && st.secret_input_observed
                    }
                }))
            }
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| {
                        function.contains("cua_secret_delivery_observer_cleanup_v1")
                    }) =>
            {
                MockReply::ok(json!({ "result": { "value": true } }))
            }
            "Runtime.callFunctionOn"
                if call.params["functionDeclaration"]
                    .as_str()
                    .is_some_and(|function| function.contains("this.type === 'password'")) =>
            {
                MockReply::ok(json!({ "result": { "value": st.password_target_live } }))
            }
            "Runtime.callFunctionOn" => MockReply::ok(json!({ "result": { "value": true } })),
            "Runtime.releaseObject" => MockReply::ok(json!({})),
            other => MockReply::method_not_found(other),
        }
    })
}

/// Platform adapter pointing at the fixture endpoint: pid 1 is a
/// Chromium browser owning native window 7 at (0,0,800,600).
struct FixturePlatform {
    ws_url: String,
    trusted_input_limited: bool,
    managed_endpoint_visible: bool,
    process_role: BrowserProcessRole,
    managed_discovery_invoked: Arc<AtomicBool>,
    existing_endpoint_visible: Arc<AtomicBool>,
    setup_invoked: Arc<AtomicBool>,
    setup_aborted: Arc<AtomicBool>,
    stall_consent: bool,
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
            product_kind: BrowserProduct::GoogleChrome,
            product: Some("MockChrome".into()),
            channel: Some("stable".into()),
            // The mock endpoint is an explicit in-process harness, not a
            // personal standalone browser profile.
            process_role: self.process_role,
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
        self.managed_discovery_invoked.store(true, Ordering::SeqCst);
        if !self.managed_endpoint_visible {
            return Ok(None);
        }
        self.discover_existing_profile_endpoint(pid).await
    }

    async fn discover_existing_profile_endpoint(
        &self,
        pid: i64,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        if !self.existing_endpoint_visible.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(OwnedEndpoint {
            ws_url: self.ws_url.clone(),
            http_port: None,
            transport: EndpointTransport::LegacyJsonVersion,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: None,
            },
        }))
    }

    async fn reprove_existing_profile_endpoint(
        &self,
        pid: i64,
        expected_ws_url: &str,
    ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
        if expected_ws_url != self.ws_url {
            return Ok(None);
        }
        Ok(Some(OwnedEndpoint {
            ws_url: self.ws_url.clone(),
            http_port: None,
            transport: EndpointTransport::LegacyJsonVersion,
            ownership: EndpointOwnershipProof {
                method: EndpointOwnershipMethod::ListeningSocketPid,
                owner_pid: pid,
                listener_pid: None,
                detail: Some("fixture exact approved endpoint".to_owned()),
            },
        }))
    }

    async fn setup_existing_profile_endpoint(
        &self,
        _request: ExistingProfileSetupRequest,
    ) -> Result<ExistingProfileSetupOutcome, BrowserRefusal> {
        self.setup_invoked.store(true, Ordering::SeqCst);
        Ok(ExistingProfileSetupOutcome {
            opened_setup_page: true,
            closed_setup_page: false,
            enabled_remote_debugging: true,
            used_bounded_pixel_fallback: false,
            focused_setup_address_field: true,
            foregrounded_window: false,
            injected_global_input: false,
            endpoint: Some(OwnedEndpoint {
                ws_url: self.ws_url.clone(),
                http_port: None,
                transport: EndpointTransport::DevToolsActivePort,
                ownership: EndpointOwnershipProof {
                    method: EndpointOwnershipMethod::ListeningSocketPid,
                    owner_pid: 1,
                    listener_pid: None,
                    detail: Some("fixture exact setup transition".to_owned()),
                },
            }),
        })
    }

    async fn commit_existing_profile_setup(
        &self,
        _request: ExistingProfileSetupRequest,
    ) -> Result<bool, BrowserRefusal> {
        Ok(true)
    }

    async fn abort_existing_profile_setup(
        &self,
        _request: ExistingProfileSetupRequest,
        error: BrowserRefusal,
    ) -> BrowserRefusal {
        self.setup_aborted.store(true, Ordering::SeqCst);
        error
    }

    async fn handle_existing_profile_consent(
        &self,
        _request: BrowserConsentRequest,
    ) -> Result<BrowserConsentOutcome, BrowserRefusal> {
        if self.stall_consent {
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            return Ok(BrowserConsentOutcome::NotPresent);
        }
        Err(BrowserRefusal::new(
            super::refusal::BrowserRefusalCode::BrowserRouteUnavailable,
            "fixture consent prompt is unavailable",
        ))
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
            attachment: None,
        })
    }
}

// ── Scaffolding ──────────────────────────────────────────────────────────────

struct Fixture {
    state: SharedState,
    // Kept alive for the test's duration; dropping it kills the endpoint.
    _server: MockCdpServer,
    engine: Arc<BrowserEngine>,
    setup_invoked: Arc<AtomicBool>,
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
    let setup_invoked = Arc::new(AtomicBool::new(false));
    let engine = BrowserEngine::new(Arc::new(FixturePlatform {
        ws_url: server.ws_url(),
        trusted_input_limited,
        managed_endpoint_visible: true,
        process_role: BrowserProcessRole::EmbeddedApplication,
        managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
        existing_endpoint_visible: Arc::new(AtomicBool::new(true)),
        setup_invoked: setup_invoked.clone(),
        setup_aborted: Arc::new(AtomicBool::new(false)),
        stall_consent: false,
    }));
    Fixture {
        state,
        _server: server,
        engine,
        setup_invoked,
    }
}

async fn fixture() -> Fixture {
    fixture_with(|_| {}).await
}

struct FixtureProtectedProvider {
    consent_seen: AtomicBool,
}

#[async_trait]
impl crate::consent::ProtectedConsentProvider for FixtureProtectedProvider {
    fn provider_id(&self) -> &'static str {
        "test.browser-protected-provider"
    }

    async fn request_consent(
        &self,
        request: &crate::consent::ConsentRequest,
    ) -> Result<crate::consent::ProviderDecision, String> {
        self.consent_seen.store(true, Ordering::SeqCst);
        Ok(crate::consent::ProviderDecision {
            action: crate::consent::ConsentAction::Accept,
            request_digest: request.request_digest.clone(),
        })
    }
}

struct DiscoveryCredentialProvider {
    id: CredentialProviderId,
    releases: AtomicUsize,
    cancellations: AtomicUsize,
    state: SharedState,
    plan: StdMutex<DiscoveryReleasePlan>,
    post_release_mutation: StdMutex<Option<DiscoveryPostReleaseMutation>>,
}

#[derive(Debug, Clone, Copy)]
enum DiscoveryReleasePlan {
    RuntimeDelivers,
    ProviderFills,
    NeedsUserPresence,
}

#[derive(Debug, Clone, Copy)]
enum DiscoveryPostReleaseMutation {
    NavigateMainFrame,
    ChangePasswordType,
    ChangeOrigin,
}

impl DiscoveryCredentialProvider {
    fn new(state: SharedState) -> Arc<Self> {
        Arc::new(Self {
            id: CredentialProviderId::new("fixture-credential-provider").unwrap(),
            releases: AtomicUsize::new(0),
            cancellations: AtomicUsize::new(0),
            state,
            plan: StdMutex::new(DiscoveryReleasePlan::RuntimeDelivers),
            post_release_mutation: StdMutex::new(None),
        })
    }

    fn set_plan(&self, plan: DiscoveryReleasePlan) {
        *self.plan.lock().unwrap() = plan;
    }

    fn mutate_after_release(&self, mutation: DiscoveryPostReleaseMutation) {
        *self.post_release_mutation.lock().unwrap() = Some(mutation);
    }
}

#[async_trait]
impl CredentialProvider for DiscoveryCredentialProvider {
    fn id(&self) -> &CredentialProviderId {
        &self.id
    }

    fn class(&self) -> CredentialProviderClass {
        CredentialProviderClass::Fake
    }

    async fn release(
        &self,
        _request: &ProviderReleaseRequest,
    ) -> Result<ReleasePlan, CredentialProviderError> {
        self.releases.fetch_add(1, Ordering::SeqCst);
        if let Some(mutation) = self.post_release_mutation.lock().unwrap().take() {
            let mut state = self.state.lock().unwrap();
            match mutation {
                DiscoveryPostReleaseMutation::NavigateMainFrame => {
                    state.main_loader = "L_MAIN_AFTER_PROVIDER".to_owned();
                }
                DiscoveryPostReleaseMutation::ChangePasswordType => {
                    state.password_target_live = false;
                }
                DiscoveryPostReleaseMutation::ChangeOrigin => {
                    state.main_url = "https://other.fixture.test/login".to_owned();
                }
            }
        }
        match *self.plan.lock().unwrap() {
            DiscoveryReleasePlan::RuntimeDelivers => Ok(ReleasePlan::RuntimeDelivers(
                SecretLease::from_utf8_bytes(synthetic_delivery_secret().into_bytes()).unwrap(),
            )),
            DiscoveryReleasePlan::ProviderFills => Ok(ReleasePlan::ProviderFills(
                ProviderFillSession::from_opaque_bytes(vec![1]).unwrap(),
            )),
            DiscoveryReleasePlan::NeedsUserPresence => Ok(ReleasePlan::NeedsUserPresence(
                UserPresenceHandle::from_opaque_bytes(vec![1]).unwrap(),
            )),
        }
    }

    async fn cancel(&self, _release: ProviderReleaseId) -> Result<(), CredentialProviderError> {
        self.cancellations.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth {
            state: ProviderHealthState::Available,
        }
    }
}

struct CredentialDiscoveryFixture {
    state: SharedState,
    _server: MockCdpServer,
    engine: Arc<BrowserEngine>,
    broker: Arc<SecretBroker>,
    provider: Arc<DiscoveryCredentialProvider>,
    context: Arc<crate::session_authorization::EffectiveAuthorizationContext>,
    session: String,
    target_id: String,
    tab_id: String,
    password_ref: String,
    ordinary_ref: String,
}

async fn credential_discovery_fixture() -> CredentialDiscoveryFixture {
    let mut initial = FixtureState {
        semantic_large_page: true,
        main_url: "https://fixture.test/login".to_owned(),
        ..FixtureState::default()
    };
    initial.password_target_live = true;
    let state = Arc::new(StdMutex::new(initial));
    let server = MockCdpServer::start(fixture_handler(state.clone())).await;

    let ceiling = crate::session_authorization::SessionModeCeiling::for_trusted_sessions(
        [crate::authorization::PermissionMode::Unrestricted],
        true,
        Duration::from_secs(60),
        Duration::from_secs(30),
    )
    .unwrap();
    let context = crate::session_authorization::SessionAuthorizationRegistry::with_ceiling(ceiling)
        .compatibility_context(crate::authorization::PermissionMode::Unrestricted, None)
        .unwrap();
    let provider = DiscoveryCredentialProvider::new(state.clone());
    let mut builder = SecretBrokerBuilder::new(context.runtime_scope_key()).unwrap();
    let session = builder.runtime_session_key("credential-discovery").unwrap();
    builder.add_provider(provider.clone()).unwrap();
    builder
        .add_binding(CredentialBindingSpec {
            session: session.clone(),
            provider: provider.id().clone(),
            private_fields: vec![PrivateCredentialField {
                field: CredentialField::Password,
                locator: PrivateCredentialFieldLocator::new(
                    "synthetic://fixture-vault/login/password",
                )
                .unwrap(),
            }],
            safe_label: Some(SafeCredentialLabel::new("Synthetic login").unwrap()),
            allowed_targets: vec![SecretTargetConstraint::Browser(
                BrowserSecretTargetConstraint::exact_password_origin("https://fixture.test")
                    .unwrap(),
            )],
            expires_at: None,
            max_releases: None,
            authorization: SecretReleaseAuthorization::trusted_scope("fixture-login").unwrap(),
        })
        .unwrap();
    let broker = builder.build().unwrap();
    let engine = BrowserEngine::new_with_runtime_services_and_secret_broker(
        Arc::new(FixturePlatform {
            ws_url: server.ws_url(),
            trusted_input_limited: false,
            managed_endpoint_visible: false,
            process_role: BrowserProcessRole::StandaloneConsumer,
            managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
            existing_endpoint_visible: Arc::new(AtomicBool::new(true)),
            setup_invoked: Arc::new(AtomicBool::new(false)),
            setup_aborted: Arc::new(AtomicBool::new(false)),
            stall_consent: false,
        }),
        Arc::new(crate::consent::ApprovalBroker::new(Some(Arc::new(
            FixtureProtectedProvider {
                consent_seen: AtomicBool::new(false),
            },
        )))),
        Arc::new(crate::consent::ProtectedResourceOwnershipStore::default()),
        Some(broker.clone()),
    );
    let transport = format!("credential-transport-{}", context.runtime_scope_key());
    let prepared = BrowserPrepareTool::new(engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": session,
            "_transport_session_id": transport,
            "strategy": { "kind": "existing_profile" }
        }))
        .await;
    assert_eq!(
        structured(&prepared)["status"],
        "ok",
        "credential fixture prepare failed: {}",
        structured(&prepared)
    );
    let bound = GetBrowserStateTool::new(engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": session,
            "_transport_session_id": transport,
        }))
        .await;
    let bound = structured(&bound);
    assert_eq!(
        bound["status"], "ok",
        "credential fixture bind failed: {bound}"
    );
    let target_id = bound["target_id"].as_str().unwrap().to_owned();
    let tab_id = bound["tabs"][0]["tab_id"].as_str().unwrap().to_owned();
    let snapshot = GetBrowserStateTool::new(engine.clone())
        .invoke(json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "session": session,
            "snapshot_format": "semantic_v2",
        }))
        .await;
    let snapshot = structured(&snapshot);
    assert_eq!(
        snapshot["status"], "ok",
        "credential fixture snapshot failed: {snapshot}"
    );
    let password_ref = snapshot["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["secure_field"] == "password")
        .and_then(|entry| entry["ref"].as_str())
        .expect("semantic password ref")
        .to_owned();
    let ordinary_ref = snapshot["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Password hint")
        .and_then(|entry| entry["ref"].as_str())
        .expect("ordinary text ref")
        .to_owned();

    CredentialDiscoveryFixture {
        state,
        _server: server,
        engine,
        broker,
        provider,
        context,
        session,
        target_id,
        tab_id,
        password_ref,
        ordinary_ref,
    }
}

#[tokio::test]
async fn credential_discovery_mints_fresh_handles_only_after_exact_target_proof() {
    let fixture = credential_discovery_fixture().await;
    let first = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(
        first[0].label.as_ref().map(SafeCredentialLabel::as_str),
        Some("Synthetic login")
    );
    assert_eq!(first[0].fields, vec![CredentialField::Password]);
    assert_eq!(first[0].provider_class, Some(CredentialProviderClass::Fake));

    let second = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_ne!(first[0].handle.as_str(), second[0].handle.as_str());
    assert_eq!(fixture.broker.active_handle_count(), 1);
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.provider.cancellations.load(Ordering::SeqCst), 0);

    let proof_calls_before = fixture
        .state
        .lock()
        .unwrap()
        .calls
        .iter()
        .filter(|(_, method, _)| method == "Page.createIsolatedWorld")
        .count();
    let ordinary = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.ordinary_ref,
        )
        .await
        .unwrap();
    assert!(ordinary.is_empty());
    assert_eq!(fixture.broker.active_handle_count(), 1);
    let calls = fixture.state.lock().unwrap().calls.clone();
    assert_eq!(
        calls
            .iter()
            .filter(|(_, method, _)| method == "Page.createIsolatedWorld")
            .count(),
        proof_calls_before,
        "ordinary fields must be rejected before secure-node instrumentation"
    );
    assert!(calls.iter().any(|(_, method, params)| {
        method == "DOM.resolveNode"
            && params["objectGroup"].as_str() == Some("__cua_secret_target_v1")
            && params["executionContextId"].as_i64() == Some(101)
    }));
    assert!(
        calls
            .iter()
            .all(|(_, method, _)| method != "Input.insertText"),
        "discovery must never deliver input"
    );
}

#[tokio::test]
async fn credential_discovery_rejects_tab_and_document_substitution_before_minting() {
    let fixture = credential_discovery_fixture().await;
    let wrong_tab = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            "tab-substituted",
            &fixture.password_ref,
        )
        .await
        .unwrap_err();
    assert_eq!(
        wrong_tab.code,
        super::refusal::BrowserRefusalCode::BrowserTabNotFound
    );
    assert_eq!(fixture.broker.active_handle_count(), 0);

    fixture.state.lock().unwrap().main_loader = "L_MAIN_NAVIGATED".to_owned();
    let navigated = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap_err();
    assert_eq!(
        navigated.code,
        super::refusal::BrowserRefusalCode::BrowserRefStale
    );
    assert_eq!(fixture.broker.active_handle_count(), 0);
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn credential_discovery_rejects_nested_frames_before_minting() {
    let fixture = credential_discovery_fixture().await;
    let (snapshot_id, index) = super::store::parse_ref(&fixture.password_ref).unwrap();
    fixture
        .engine
        .store
        .update_target(&fixture.session, &fixture.target_id, |target| {
            let entry = target
                .tabs
                .get_mut(&fixture.tab_id)
                .and_then(|tab| tab.snapshots.get_mut(&snapshot_id))
                .and_then(|snapshot| snapshot.refs.get_mut(&index))
                .unwrap();
            entry.frame.kind = super::store::FrameKind::Iframe;
            entry.frame.identity = Some(super::store::FrameIdentity {
                frame_id: "F_IFRAME".to_owned(),
                loader_id: "L_IFRAME_1".to_owned(),
            });
        });

    let nested = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap_err();
    assert_eq!(
        nested.code,
        super::refusal::BrowserRefusalCode::BrowserOriginOutsideScope
    );
    assert_eq!(fixture.broker.active_handle_count(), 0);
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn credential_discovery_reproves_current_password_semantics_before_minting() {
    let fixture = credential_discovery_fixture().await;
    fixture.state.lock().unwrap().password_target_live = false;
    let changed = fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap_err();
    assert_eq!(
        changed.code,
        super::refusal::BrowserRefusalCode::BrowserRefStale
    );
    assert_eq!(fixture.broker.active_handle_count(), 0);
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
}

async fn discover_fixture_credential(
    fixture: &CredentialDiscoveryFixture,
) -> crate::credentials::CredentialHandle {
    fixture
        .engine
        .discover_credentials_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
        )
        .await
        .unwrap()
        .remove(0)
        .handle
}

#[tokio::test]
async fn credential_delivery_inserts_once_with_boolean_only_exact_node_evidence() {
    let fixture = credential_discovery_fixture().await;
    let handle = discover_fixture_credential(&fixture).await;
    let outcome = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap();
    assert_eq!(outcome, CredentialDeliveryOutcome::Delivered);
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.broker.active_handle_count(), 0);

    let calls = fixture.state.lock().unwrap().calls.clone();
    let inserts = calls
        .iter()
        .filter(|(_, method, _)| method == "Input.insertText")
        .collect::<Vec<_>>();
    assert_eq!(inserts.len(), 1);
    assert_eq!(inserts[0].2["text"], "[secret omitted]");
    assert!(calls.iter().any(|(_, method, params)| {
        method == "Runtime.callFunctionOn"
            && params["functionDeclaration"]
                .as_str()
                .is_some_and(|function| function.contains("cua_secret_delivery_observer_verify_v1"))
    }));
    for (_, method, params) in &calls {
        if method != "Runtime.callFunctionOn" {
            continue;
        }
        let declaration = params["functionDeclaration"].as_str().unwrap_or_default();
        if !declaration.contains("cua_secret_delivery_") {
            continue;
        }
        for forbidden in [
            "event.data",
            "this.value",
            "selectionStart",
            "selectionEnd",
            "textContent",
        ] {
            assert!(
                !declaration.contains(forbidden),
                "secret delivery inspected forbidden content via {forbidden}"
            );
        }
    }
    assert!(
        calls
            .iter()
            .all(|(_, method, _)| method != "Input.dispatchKeyEvent"),
        "credential delivery must never fall back to generic key events"
    );

    let canary = synthetic_delivery_secret();
    let retained = serde_json::to_string(&calls).unwrap();
    assert!(
        !retained.contains(&canary),
        "CDP fixture retained the secret"
    );
    assert!(!format!("{handle:?} {outcome:?}").contains(&canary));

    let replay = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        replay.code,
        super::refusal::BrowserRefusalCode::SecretHandleConsumed
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(_, method, _)| method == "Input.insertText")
            .count(),
        1,
        "a consumed handle must never retry insertion"
    );
}

#[tokio::test]
async fn credential_delivery_attests_the_exact_tab_for_browser_input_authorization() {
    let fixture = credential_discovery_fixture().await;
    let tool = TypeSecretTool::new(fixture.engine.clone());
    let scope = tool
        .protected_resource_scope(
            "browser_bound_input",
            &json!({
                "session": fixture.session,
                "target_id": fixture.target_id,
                "tab_id": fixture.tab_id,
                "ref": fixture.password_ref,
                "handle": "ch-0123456789abcdef0123456789abcdef",
                "field": "password",
            }),
        )
        .await;
    let scope = scope.unwrap().expect("exact browser input scope");
    assert_eq!(scope["kind"], "authenticated_browser_tab");
    assert_eq!(scope["action_class"], "page_input");
    assert_eq!(scope["live_origin"], "https://fixture.test");
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn credential_delivery_rejects_target_drift_before_provider_work() {
    let fixture = credential_discovery_fixture().await;
    let handle = discover_fixture_credential(&fixture).await;
    fixture.state.lock().unwrap().main_loader = "L_MAIN_BEFORE_PROVIDER".to_owned();

    let error = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        super::refusal::BrowserRefusalCode::BrowserRefStale
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.broker.active_handle_count(), 1);
    assert!(!fixture.state.lock().unwrap().secret_insert_seen);
}

#[tokio::test]
async fn credential_delivery_consumes_without_inserting_when_target_drifts_after_provider_work() {
    for mutation in [
        DiscoveryPostReleaseMutation::NavigateMainFrame,
        DiscoveryPostReleaseMutation::ChangePasswordType,
        DiscoveryPostReleaseMutation::ChangeOrigin,
    ] {
        let fixture = credential_discovery_fixture().await;
        let handle = discover_fixture_credential(&fixture).await;
        fixture.provider.mutate_after_release(mutation);

        let error = fixture
            .engine
            .deliver_credential_for_ref_with_context(
                fixture.context.as_ref(),
                &fixture.session,
                &fixture.target_id,
                &fixture.tab_id,
                &fixture.password_ref,
                &handle,
                CredentialField::Password,
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.code,
            super::refusal::BrowserRefusalCode::SecretDeliveryMisdirected,
            "post-provider mutation {mutation:?} was not collapsed to a fixed outcome"
        );
        assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.broker.active_handle_count(), 0);
        assert!(!fixture.state.lock().unwrap().secret_insert_seen);
    }
}

#[tokio::test]
async fn credential_delivery_maps_non_runtime_plans_without_provider_metadata() {
    for (plan, expected) in [
        (
            DiscoveryReleasePlan::ProviderFills,
            CredentialDeliveryOutcome::ProviderFillUnavailable,
        ),
        (
            DiscoveryReleasePlan::NeedsUserPresence,
            CredentialDeliveryOutcome::UserPresenceRequired,
        ),
    ] {
        let fixture = credential_discovery_fixture().await;
        let handle = discover_fixture_credential(&fixture).await;
        fixture.provider.set_plan(plan);
        let outcome = fixture
            .engine
            .deliver_credential_for_ref_with_context(
                fixture.context.as_ref(),
                &fixture.session,
                &fixture.target_id,
                &fixture.tab_id,
                &fixture.password_ref,
                &handle,
                CredentialField::Password,
            )
            .await
            .unwrap();
        assert_eq!(outcome, expected);
        assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
        assert_eq!(fixture.broker.active_handle_count(), 0);
        assert!(!fixture.state.lock().unwrap().secret_insert_seen);
    }
}

#[tokio::test]
async fn credential_delivery_missing_events_is_misdirected_and_never_retried() {
    let fixture = credential_discovery_fixture().await;
    let handle = discover_fixture_credential(&fixture).await;
    fixture.state.lock().unwrap().secret_input_observed = false;
    let error = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        super::refusal::BrowserRefusalCode::SecretDeliveryMisdirected
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.broker.active_handle_count(), 0);
    assert!(fixture.state.lock().unwrap().secret_insert_seen);

    let replay = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        replay.code,
        super::refusal::BrowserRefusalCode::SecretHandleConsumed
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn credential_delivery_unavailable_focus_oracle_is_unverified_without_insertion() {
    let fixture = credential_discovery_fixture().await;
    let handle = discover_fixture_credential(&fixture).await;
    fixture.state.lock().unwrap().secret_focus_exact = false;
    let error = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        super::refusal::BrowserRefusalCode::SecretDeliveryUnverified
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.broker.active_handle_count(), 0);
    assert!(!fixture.state.lock().unwrap().secret_insert_seen);
}

#[tokio::test]
async fn credential_delivery_insert_error_is_unverified_and_scrubbed() {
    let fixture = credential_discovery_fixture().await;
    let handle = discover_fixture_credential(&fixture).await;
    fixture.state.lock().unwrap().secret_insert_error = true;
    let error = fixture
        .engine
        .deliver_credential_for_ref_with_context(
            fixture.context.as_ref(),
            &fixture.session,
            &fixture.target_id,
            &fixture.tab_id,
            &fixture.password_ref,
            &handle,
            CredentialField::Password,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        super::refusal::BrowserRefusalCode::SecretDeliveryUnverified
    );
    assert_eq!(fixture.provider.releases.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.broker.active_handle_count(), 0);
    let state = fixture.state.lock().unwrap();
    assert!(state.secret_insert_seen);
    let retained = serde_json::to_string(&state.calls).unwrap();
    assert!(!retained.contains(&synthetic_delivery_secret()));
    assert!(!format!("{error:?}").contains(&synthetic_delivery_secret()));
}

async fn protected_existing_profile_fixture() -> (Fixture, Arc<FixtureProtectedProvider>) {
    let state = Arc::new(StdMutex::new(FixtureState::default()));
    let server = MockCdpServer::start(fixture_handler(state.clone())).await;
    let setup_invoked = Arc::new(AtomicBool::new(false));
    let provider = Arc::new(FixtureProtectedProvider {
        consent_seen: AtomicBool::new(false),
    });
    let engine = BrowserEngine::new_with_protected_consent_provider(
        Arc::new(FixturePlatform {
            ws_url: server.ws_url(),
            trusted_input_limited: false,
            managed_endpoint_visible: false,
            process_role: BrowserProcessRole::StandaloneConsumer,
            managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
            existing_endpoint_visible: Arc::new(AtomicBool::new(true)),
            setup_invoked: setup_invoked.clone(),
            setup_aborted: Arc::new(AtomicBool::new(false)),
            stall_consent: false,
        }),
        Some(provider.clone()),
    );
    (
        Fixture {
            state,
            _server: server,
            engine,
            setup_invoked,
        },
        provider,
    )
}

async fn existing_profile_setup_fixture() -> (Fixture, Arc<AtomicBool>) {
    let state = Arc::new(StdMutex::new(FixtureState::default()));
    let server = MockCdpServer::start(fixture_handler(state.clone())).await;
    let setup_invoked = Arc::new(AtomicBool::new(false));
    let engine = BrowserEngine::new_with_protected_consent_provider(
        Arc::new(FixturePlatform {
            ws_url: server.ws_url(),
            trusted_input_limited: false,
            managed_endpoint_visible: false,
            process_role: BrowserProcessRole::StandaloneConsumer,
            managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
            existing_endpoint_visible: Arc::new(AtomicBool::new(false)),
            setup_invoked: setup_invoked.clone(),
            setup_aborted: Arc::new(AtomicBool::new(false)),
            stall_consent: false,
        }),
        Some(Arc::new(FixtureProtectedProvider {
            consent_seen: AtomicBool::new(false),
        })),
    );
    (
        Fixture {
            state,
            _server: server,
            engine,
            setup_invoked: setup_invoked.clone(),
        },
        setup_invoked,
    )
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

#[tokio::test]
async fn standalone_consumer_bind_without_grant_refuses_before_endpoint_discovery() {
    let state = Arc::new(StdMutex::new(FixtureState::default()));
    let server = MockCdpServer::start(fixture_handler(state)).await;
    let managed_discovery_invoked = Arc::new(AtomicBool::new(false));
    let setup_invoked = Arc::new(AtomicBool::new(false));
    let engine = BrowserEngine::new(Arc::new(FixturePlatform {
        ws_url: server.ws_url(),
        trusted_input_limited: false,
        managed_endpoint_visible: true,
        process_role: BrowserProcessRole::StandaloneConsumer,
        managed_discovery_invoked: managed_discovery_invoked.clone(),
        existing_endpoint_visible: Arc::new(AtomicBool::new(true)),
        setup_invoked: setup_invoked.clone(),
        setup_aborted: Arc::new(AtomicBool::new(false)),
        stall_consent: false,
    }));

    let result = GetBrowserStateTool::new(engine)
        .invoke(json!({ "pid": 1, "window_id": 7, "session": SESSION }))
        .await;
    let refusal = structured(&result);
    assert_eq!(refusal["status"], "refused");
    assert_eq!(refusal["refusal"]["code"], "browser_consent_required");
    assert_eq!(
        refusal["refusal"]["detail"]["reason"],
        "consumer_profile_endpoint_requires_grant"
    );
    assert_eq!(
        refusal["refusal"]["detail"]["next_action"],
        "browser_prepare"
    );
    assert!(
        !managed_discovery_invoked.load(Ordering::SeqCst),
        "read-only bind must not inspect a consent-gated endpoint"
    );
    assert!(
        !setup_invoked.load(Ordering::SeqCst),
        "read-only bind must never invoke browser setup UI"
    );
}

#[tokio::test]
async fn approved_existing_profile_attach_claims_then_binds_one_generation() {
    // Match Chrome's per-instance toggle: the endpoint is discoverable only
    // through the approved existing-profile route, never as driver-managed.
    let (f, provider) = protected_existing_profile_fixture().await;
    let prepare = BrowserPrepareTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "_transport_session_id": "transport-v2-attach",
            "strategy": { "kind": "existing_profile" }
        }))
        .await;
    let prepared = structured(&prepare);
    assert_eq!(prepared["status"], "ok", "{prepared}");
    assert_eq!(prepared["action"], "attached_existing_profile");
    assert_eq!(prepared["attachment"]["kind"], "existing_profile");
    assert_eq!(prepared["attachment"]["capabilities_invalidated"], true);
    assert_eq!(prepared["side_effects"]["displayed_consent_prompt"], false);
    assert!(provider.consent_seen.load(Ordering::SeqCst));
    assert!(!f.setup_invoked.load(Ordering::SeqCst));

    let state = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "_transport_session_id": "transport-v2-attach"
        }))
        .await;
    assert_eq!(structured(&state)["status"], "ok", "{}", structured(&state));
    crate::session::fire_session_end("transport-v2-attach");
}

#[tokio::test]
async fn approved_existing_profile_tools_stay_within_the_reviewed_cdp_surface() {
    const TRANSPORT: &str = "transport-v2-method-policy";
    let (f, _) = protected_existing_profile_fixture().await;
    let prepare = BrowserPrepareTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "_transport_session_id": TRANSPORT,
            "strategy": { "kind": "existing_profile" }
        }))
        .await;
    assert_eq!(
        structured(&prepare)["status"],
        "ok",
        "{}",
        structured(&prepare)
    );

    let state = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "_transport_session_id": TRANSPORT
        }))
        .await;
    let bound = structured(&state);
    assert_eq!(bound["status"], "ok", "{bound}");
    let target = bound["target_id"].as_str().unwrap().to_owned();
    let tab = bound["tabs"][0]["tab_id"].as_str().unwrap().to_owned();

    let snap = snapshot(&f, &target, &tab).await;
    let button = ref_of(&snap, "main", "main-btn");
    let input = ref_of(&snap, "main", "Shadow Input");
    let clicked = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "ref": button,
            "input_route": "dom_event",
            "session": SESSION
        }))
        .await;
    assert_eq!(
        structured(&clicked)["status"],
        "ok",
        "{}",
        structured(&clicked)
    );
    let typed = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "ref": input,
            "text": "policy-check",
            "session": SESSION
        }))
        .await;
    assert_eq!(structured(&typed)["status"], "ok", "{}", structured(&typed));
    let navigated = BrowserNavigateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "url": "https://fixture.test/policy-check",
            "session": SESSION
        }))
        .await;
    assert_eq!(
        structured(&navigated)["status"],
        "ok",
        "{}",
        structured(&navigated)
    );

    let forbidden = [
        "Runtime.enable",
        "Target.setDiscoverTargets",
        "Page.addScriptToEvaluateOnNewDocument",
        "Network.enable",
        "Fetch.enable",
        "Emulation.setUserAgentOverride",
        "Emulation.setDeviceMetricsOverride",
        "Emulation.setTimezoneOverride",
    ];
    let calls = f.state.lock().unwrap().calls.clone();
    for method in forbidden {
        assert!(
            calls.iter().all(|(_, observed, _)| observed != method),
            "existing-profile operation emitted forbidden CDP method {method}: {calls:?}"
        );
    }

    crate::session::fire_session_end(TRANSPORT);
}

#[tokio::test]
async fn protected_provider_accepts_exact_attach_and_session_end_revokes_the_grant() {
    const PROTECTED_SESSION: &str = "protected-provider-v2";
    const PROTECTED_TRANSPORT: &str = "protected-transport-v2";
    let (f, provider) = protected_existing_profile_fixture().await;
    let prepare = BrowserPrepareTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": PROTECTED_SESSION,
            "_transport_session_id": PROTECTED_TRANSPORT,
            "strategy": { "kind": "existing_profile" }
        }))
        .await;
    assert_eq!(
        structured(&prepare)["status"],
        "ok",
        "{}",
        structured(&prepare)
    );
    assert!(provider.consent_seen.load(Ordering::SeqCst));

    crate::session::fire_session_end(PROTECTED_TRANSPORT);
    tokio::task::yield_now().await;

    let state = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": PROTECTED_SESSION,
            "_transport_session_id": PROTECTED_TRANSPORT
        }))
        .await;
    assert_eq!(structured(&state)["status"], "refused");
}

#[tokio::test]
async fn approved_existing_profile_setup_reports_exact_side_effects() {
    let (f, setup_invoked) = existing_profile_setup_fixture().await;
    let prepare = BrowserPrepareTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "strategy": { "kind": "existing_profile" }
        }))
        .await;
    let prepared = structured(&prepare);
    assert_eq!(prepared["status"], "ok", "{prepared}");
    assert!(setup_invoked.load(Ordering::SeqCst));
    assert_eq!(prepared["side_effects"]["opened_setup_page"], true);
    assert_eq!(prepared["side_effects"]["closed_setup_page"], true);
    assert_eq!(prepared["side_effects"]["enabled_remote_debugging"], true);
    assert_eq!(prepared["side_effects"]["changed_preferences"], true);
    assert_eq!(
        prepared["side_effects"]["focused_setup_address_field"],
        true
    );

    let state = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION
        }))
        .await;
    assert_eq!(structured(&state)["status"], "ok", "{}", structured(&state));
}

#[tokio::test]
async fn refused_consent_cancels_stalled_claim_before_revoking_grant() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stalled_server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    });
    let engine = BrowserEngine::new_with_protected_consent_provider(
        Arc::new(FixturePlatform {
            ws_url: format!("ws://{address}/devtools/browser"),
            trusted_input_limited: false,
            managed_endpoint_visible: false,
            process_role: BrowserProcessRole::StandaloneConsumer,
            managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
            existing_endpoint_visible: Arc::new(AtomicBool::new(false)),
            setup_invoked: Arc::new(AtomicBool::new(false)),
            setup_aborted: Arc::new(AtomicBool::new(false)),
            stall_consent: false,
        }),
        Some(Arc::new(FixtureProtectedProvider {
            consent_seen: AtomicBool::new(false),
        })),
    );
    let prepared = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        BrowserPrepareTool::new(engine).invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "strategy": { "kind": "existing_profile" }
        })),
    )
    .await
    .expect("a refused consent route must not deadlock grant revocation");
    assert_eq!(structured(&prepared)["status"], "refused");
    assert_eq!(
        structured(&prepared)["refusal"]["code"],
        "browser_route_unavailable"
    );
    stalled_server.abort();
}

#[tokio::test]
async fn cancelled_prepare_aborts_the_exact_pending_setup() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stalled_server = tokio::spawn(async move {
        let (_stream, _) = listener.accept().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    });
    let setup_aborted = Arc::new(AtomicBool::new(false));
    let engine = BrowserEngine::new_with_protected_consent_provider(
        Arc::new(FixturePlatform {
            ws_url: format!("ws://{address}/devtools/browser"),
            trusted_input_limited: false,
            managed_endpoint_visible: false,
            process_role: BrowserProcessRole::StandaloneConsumer,
            managed_discovery_invoked: Arc::new(AtomicBool::new(false)),
            existing_endpoint_visible: Arc::new(AtomicBool::new(false)),
            setup_invoked: Arc::new(AtomicBool::new(false)),
            setup_aborted: setup_aborted.clone(),
            stall_consent: true,
        }),
        Some(Arc::new(FixtureProtectedProvider {
            consent_seen: AtomicBool::new(false),
        })),
    );
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(750),
        BrowserPrepareTool::new(engine).invoke(json!({
            "pid": 1,
            "window_id": 7,
            "session": SESSION,
            "strategy": { "kind": "existing_profile" }
        })),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "fixture must cancel during consent handling"
    );
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while !setup_aborted.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("dropping prepare must schedule exact pending-setup rollback");
    stalled_server.abort();
}

async fn snapshot(f: &Fixture, target_id: &str, tab_id: &str) -> Value {
    let tool = GetBrowserStateTool::new(f.engine.clone());
    let result = tool
        .invoke(json!({ "target_id": target_id, "tab_id": tab_id, "session": SESSION }))
        .await;
    structured(&result).clone()
}

async fn semantic_snapshot(f: &Fixture, target_id: &str, tab_id: &str) -> Value {
    let tool = GetBrowserStateTool::new(f.engine.clone());
    let result = tool
        .invoke(json!({
            "target_id": target_id,
            "tab_id": tab_id,
            "session": SESSION,
            "snapshot_format": "semantic_v2"
        }))
        .await;
    structured(&result).clone()
}

async fn semantic_snapshot_with(f: &Fixture, target_id: &str, tab_id: &str, extra: Value) -> Value {
    let mut args = json!({
        "target_id": target_id,
        "tab_id": tab_id,
        "session": SESSION,
        "snapshot_format": "semantic_v2"
    });
    args.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    let result = GetBrowserStateTool::new(f.engine.clone())
        .invoke(args)
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
async fn semantic_snapshot_keeps_visible_content_after_hidden_node_pressure() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot(&f, &target, &tab).await;

    assert_eq!(snap["status"], "ok", "{snap}");
    assert_eq!(snap["snapshot"]["format"], "semantic_v2", "{snap}");
    assert!(
        snap["outline"]
            .as_str()
            .is_some_and(|outline| outline.contains("Visible message")
                && outline.contains("Please review the attached fixture report.")),
        "visible semantic content was omitted: {snap}"
    );
    let refs = snap["refs"].as_array().expect("semantic refs");
    assert!(
        refs.iter().any(|entry| entry["name"] == "Reply"
            && entry["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|action| action == "click"))),
        "visible Reply action was omitted: {snap}"
    );
    assert!(
        refs.iter().any(|entry| entry["name"] == "Reply body"
            && entry["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|action| action == "type"))),
        "visible editor was omitted: {snap}"
    );
    assert!(
        refs.iter().all(|entry| !entry["name"]
            .as_str()
            .unwrap_or("")
            .starts_with("Retained control")),
        "CSS-hidden retained controls leaked into refs: {snap}"
    );
    assert_eq!(snap["snapshot"]["omitted"]["css_hidden"], 320);
}

#[tokio::test]
async fn semantic_snapshot_marks_only_exact_password_inputs_and_omits_readback() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot_with(&f, &target, &tab, json!({"query": "password"})).await;

    assert_eq!(snap["status"], "ok", "{snap}");
    let refs = snap["refs"].as_array().expect("semantic refs");
    let password = refs
        .iter()
        .find(|entry| entry["name"] == "Account password")
        .unwrap_or_else(|| panic!("password ref missing: {snap}"));
    assert_eq!(password["secure_field"], "password", "{snap}");
    assert!(
        password.get("value").is_none(),
        "secure field exposed a value projection: {snap}"
    );

    let aria_only = refs
        .iter()
        .find(|entry| entry["name"] == "Password hint")
        .unwrap_or_else(|| panic!("ARIA-only control missing: {snap}"));
    assert!(
        aria_only.get("secure_field").is_none(),
        "ARIA-only password naming became secret-eligible: {snap}"
    );
    assert_eq!(aria_only["value"], "visible hint", "{snap}");
    assert!(
        !snap.to_string().contains("CUA_SECURE_FIELD_CANARY_7Y2Q"),
        "password value leaked through semantic output: {snap}"
    );
}

#[tokio::test]
async fn semantic_snapshot_can_capture_an_inactive_tab_without_activation_calls() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let result = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "snapshot_format": "semantic_v2",
            "include_screenshot": true
        }))
        .await;
    let snapshot = structured(&result);
    assert_eq!(snapshot["status"], "ok", "{snapshot}");
    assert_eq!(snapshot["screenshot"]["mime_type"], "image/png");
    assert_eq!(snapshot["screenshot"]["width"], 1);
    assert_eq!(snapshot["screenshot"]["height"], 1);
    assert_eq!(snapshot["screenshot"]["source"], "cdp_tab");
    assert_eq!(snapshot["screenshot_width"], 1);
    assert_eq!(snapshot["screenshot_height"], 1);
    assert_eq!(snapshot["screenshot_mime_type"], "image/png");
    assert_eq!(
        snapshot["screenshot"]["coordinate_space"],
        "viewport_css_px"
    );
    assert_eq!(snapshot["screenshot"]["viewport_css_width"], 800.0);
    assert_eq!(snapshot["screenshot"]["viewport_css_height"], 600.0);
    assert_eq!(snapshot["screenshot"]["pixel_to_css_scale_x"], 800.0);
    assert_eq!(snapshot["screenshot"]["pixel_to_css_scale_y"], 600.0);
    assert!(result.content.iter().any(|content| matches!(
        content,
        Content::Image { mime_type, .. } if mime_type == "image/png"
    )));

    let state = f.state.lock().unwrap();
    assert!(state.calls.iter().any(|(_, method, params)| {
        method == "Page.captureScreenshot"
            && params["format"] == "png"
            && params["fromSurface"] == true
            && params["captureBeyondViewport"] == false
            && params["clip"]["x"] == 0.0
            && params["clip"]["y"] == 0.0
            && params["clip"]["width"] == 800.0
            && params["clip"]["height"] == 600.0
            && params["clip"]["scale"] == 1.0
    }));
    assert!(state.calls.iter().all(|(_, method, _)| {
        method != "Target.activateTarget" && method != "Page.bringToFront"
    }));
}

#[tokio::test]
async fn semantic_snapshot_does_not_capture_unless_requested() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snapshot = semantic_snapshot(&f, &target, &tab).await;
    assert_eq!(snapshot["status"], "ok", "{snapshot}");
    assert_eq!(snapshot["screenshot"], Value::Null);
    assert_eq!(snapshot["screenshot_width"], Value::Null);
    assert_eq!(snapshot["screenshot_height"], Value::Null);
    assert_eq!(snapshot["screenshot_mime_type"], Value::Null);
    assert!(recorded_calls(&f, "Page.captureScreenshot").is_empty());
}

#[tokio::test]
async fn requested_tab_screenshot_maps_non_unit_png_pixels_to_viewport_css() {
    let f = fixture_with(|state| {
        state.viewport_css_width = 2.0;
        state.viewport_css_height = 1.0;
        state.screenshot_data = screenshot_png_base64(4, 2);
    })
    .await;
    let (target, tab) = bind(&f).await;
    let result = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "snapshot_format": "dom_refs_v1",
            "include_screenshot": true
        }))
        .await;
    let snapshot = structured(&result);
    assert_eq!(snapshot["status"], "ok", "{snapshot}");
    assert_eq!(snapshot["screenshot_width"], 4);
    assert_eq!(snapshot["screenshot_height"], 2);
    assert_eq!(snapshot["screenshot_mime_type"], "image/png");
    assert_eq!(snapshot["screenshot"]["viewport_css_width"], 2.0);
    assert_eq!(snapshot["screenshot"]["viewport_css_height"], 1.0);
    assert_eq!(snapshot["screenshot"]["pixel_to_css_scale_x"], 0.5);
    assert_eq!(snapshot["screenshot"]["pixel_to_css_scale_y"], 0.5);
    let capture = recorded_calls(&f, "Page.captureScreenshot");
    assert_eq!(capture.len(), 1, "{capture:?}");
    assert_eq!(capture[0].1["clip"]["width"], 2.0);
    assert_eq!(capture[0].1["clip"]["height"], 1.0);
    assert_eq!(capture[0].1["clip"]["scale"], 1.0);
}

#[tokio::test]
async fn requested_tab_screenshot_refuses_invalid_viewport_metrics_before_capture() {
    let f = fixture_with(|state| state.viewport_css_width = 0.0).await;
    let (target, tab) = bind(&f).await;
    let result = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "snapshot_format": "dom_refs_v1",
            "include_screenshot": true
        }))
        .await;
    let refusal = structured(&result);
    assert_eq!(refusal["status"], "refused", "{refusal}");
    assert_eq!(refusal["refusal"]["code"], "browser_route_unavailable");
    assert!(recorded_calls(&f, "Page.captureScreenshot").is_empty());
}

#[tokio::test]
async fn requested_tab_screenshot_refuses_malformed_image_data() {
    let f = fixture_with(|state| state.screenshot_data = "not-base64".into()).await;
    let (target, tab) = bind(&f).await;
    let result = GetBrowserStateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "snapshot_format": "semantic_v2",
            "include_screenshot": true
        }))
        .await;
    let refusal = structured(&result);
    assert_eq!(refusal["status"], "refused", "{refusal}");
    assert_eq!(refusal["refusal"]["code"], "browser_route_unavailable");
    assert!(result
        .content
        .iter()
        .all(|content| !matches!(content, Content::Image { .. })));
}

#[tokio::test]
async fn semantic_continuation_is_opaque_single_use_and_reaches_offscreen_content() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let first = semantic_snapshot(&f, &target, &tab).await;
    let token = first["snapshot"]["continuation"]
        .as_str()
        .expect("large fixture continuation")
        .to_owned();
    assert!(token.starts_with("bc-"), "{first}");

    let continued = semantic_snapshot_with(&f, &target, &tab, json!({"continuation": token})).await;
    assert_eq!(continued["status"], "ok", "{continued}");
    assert_eq!(continued["snapshot"]["scope"], "continuation");
    assert!(
        continued["refs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| { entry["name"] == "Archive item 304" }),
        "last offscreen action was not reachable: {continued}"
    );

    let reused = semantic_snapshot_with(&f, &target, &tab, json!({"continuation": token})).await;
    assert_eq!(reused["status"], "refused", "{reused}");
    assert_eq!(reused["refusal"]["code"], "browser_ref_stale");
}

#[tokio::test]
async fn newer_semantic_snapshot_invalidates_prior_continuations() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let first = semantic_snapshot(&f, &target, &tab).await;
    let token = first["snapshot"]["continuation"]
        .as_str()
        .expect("large fixture continuation")
        .to_owned();
    let newer = semantic_snapshot(&f, &target, &tab).await;
    assert_eq!(newer["status"], "ok", "{newer}");

    let stale = semantic_snapshot_with(&f, &target, &tab, json!({"continuation": token})).await;
    assert_eq!(stale["status"], "refused", "{stale}");
    assert_eq!(stale["refusal"]["code"], "browser_ref_stale");
}

#[tokio::test]
async fn main_frame_navigation_invalidates_semantic_continuations() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let first = semantic_snapshot(&f, &target, &tab).await;
    let token = first["snapshot"]["continuation"]
        .as_str()
        .expect("large fixture continuation")
        .to_owned();

    f.state.lock().unwrap().main_loader = "L_MAIN_2".into();

    let stale = semantic_snapshot_with(&f, &target, &tab, json!({"continuation": token})).await;
    assert_eq!(stale["status"], "refused", "{stale}");
    assert_eq!(stale["refusal"]["code"], "browser_ref_stale");
}

#[tokio::test]
async fn semantic_query_and_content_scope_are_read_only_and_precise() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let queried =
        semantic_snapshot_with(&f, &target, &tab, json!({"query": "Archive item 304"})).await;
    assert_eq!(queried["snapshot"]["scope"], "query", "{queried}");
    assert_eq!(queried["refs"].as_array().unwrap().len(), 1, "{queried}");
    assert_eq!(queried["refs"][0]["name"], "Archive item 304");

    let natural =
        semantic_snapshot_with(&f, &target, &tab, json!({"query": "reply archive 304"})).await;
    assert_eq!(natural["snapshot"]["scope"], "query", "{natural}");
    assert_eq!(natural["refs"][0]["name"], "Archive item 304", "{natural}");
    assert!(
        natural["refs"]
            .as_array()
            .is_some_and(|refs| refs.iter().any(|entry| entry["name"] == "Reply")),
        "{natural}"
    );

    let fresh = semantic_snapshot(&f, &target, &tab).await;
    let heading_ref = fresh["content_refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Visible message")
        .and_then(|entry| entry["ref"].as_str())
        .expect("heading content ref")
        .to_owned();
    let scoped = semantic_snapshot_with(&f, &target, &tab, json!({"scope_ref": heading_ref})).await;
    assert_eq!(scoped["snapshot"]["scope"], "subtree", "{scoped}");
    assert_eq!(scoped["refs"].as_array().unwrap().len(), 0, "{scoped}");
    assert_eq!(
        scoped["content_refs"].as_array().unwrap().len(),
        1,
        "{scoped}"
    );
    assert_eq!(scoped["content_refs"][0]["name"], "Visible message");
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
}

#[tokio::test]
async fn navigation_targets_an_inactive_tab_without_activating_it() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let result = BrowserNavigateTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "url": "https://fixture.test/background-navigation",
            "session": SESSION,
        }))
        .await;

    let structured = result
        .structured_content
        .as_ref()
        .expect("navigation structured content");
    assert_eq!(structured["status"], "ok", "{structured}");
    let navigate = recorded_calls(&f, "Page.navigate");
    assert_eq!(navigate.len(), 1, "{navigate:?}");
    assert_eq!(
        navigate[0].1["url"],
        "https://fixture.test/background-navigation"
    );
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
}

#[tokio::test]
async fn browser_visual_feedback_probes_live_tab_visibility_without_activating_it() {
    let f = fixture_with(|state| state.tab_visible = false).await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let main_ref = ref_of(&snap, "main", "main-btn");

    let result = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "ref": main_ref,
            "input_route": "dom_event",
            "session": SESSION,
        }))
        .await;

    assert_eq!(
        structured(&result)["status"],
        "ok",
        "{}",
        structured(&result)
    );
    let visibility = recorded_calls(&f, "Runtime.evaluate");
    assert_eq!(visibility.len(), 1, "{visibility:?}");
    assert_eq!(
        visibility[0].1["expression"],
        "document.visibilityState === 'visible'"
    );
    assert_eq!(visibility[0].1["returnByValue"], true);
    assert_eq!(visibility[0].1["awaitPromise"], false);
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
}

#[tokio::test]
async fn semantic_refs_enforce_declared_action_kinds_before_delivery() {
    let f = fixture_with(|st| st.semantic_large_page = true).await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot(&f, &target, &tab).await;
    let content_ref = snap["content_refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Visible message")
        .and_then(|entry| entry["ref"].as_str())
        .unwrap();
    let click = BrowserClickTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "ref": content_ref,
            "input_route": "dom_event"
        }))
        .await;
    assert_eq!(
        structured(&click)["refusal"]["code"],
        "browser_action_unavailable"
    );

    let pointer = BrowserPointerTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "ref": content_ref,
            "action": "hover",
            "input_route": "dom_event"
        }))
        .await;
    assert_eq!(
        structured(&pointer)["refusal"]["code"],
        "browser_action_unavailable"
    );

    let button_ref = snap["refs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == "Reply")
        .and_then(|entry| entry["ref"].as_str())
        .unwrap();
    let typed = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "session": SESSION,
            "ref": button_ref,
            "text": "must not deliver"
        }))
        .await;
    assert_eq!(
        structured(&typed)["refusal"]["code"],
        "browser_action_unavailable"
    );
    assert!(recorded_calls(&f, "Input.insertText").is_empty());
    assert!(recorded_calls(&f, "Runtime.callFunctionOn").is_empty());
}

#[tokio::test]
async fn semantic_snapshot_uses_bounded_dom_fallback_only_for_known_size_failures() {
    let f = fixture_with(|st| {
        st.semantic_large_page = true;
        st.semantic_full_dom_fails = true;
    })
    .await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot_with(&f, &target, &tab, json!({"query": "Reply"})).await;
    assert_eq!(snap["status"], "ok", "{snap}");
    assert_eq!(snap["snapshot"]["complete"], false, "{snap}");
    let document_calls = recorded_calls(&f, "DOM.getDocument");
    assert!(document_calls
        .iter()
        .any(|(_, params)| params["depth"] == -1));
    assert!(document_calls
        .iter()
        .any(|(_, params)| params["depth"] == 256));
    assert!(document_calls
        .iter()
        .any(|(_, params)| params["depth"] == 8));
}

#[tokio::test]
async fn semantic_snapshot_uses_bounded_dom_fallback_for_full_tree_timeout() {
    let f = fixture_with(|st| {
        st.semantic_large_page = true;
        st.semantic_full_dom_times_out = true;
    })
    .await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot_with(&f, &target, &tab, json!({"query": "Reply"})).await;
    assert_eq!(snap["status"], "ok", "{snap}");
    assert_eq!(snap["snapshot"]["complete"], false, "{snap}");
    let document_calls = recorded_calls(&f, "DOM.getDocument");
    assert!(document_calls
        .iter()
        .any(|(_, params)| params["depth"] == -1));
    assert!(document_calls
        .iter()
        .any(|(_, params)| params["depth"] == 8));
    assert!(document_calls.iter().all(|(_, params)| {
        params["depth"] == -1 || params["depth"].as_i64().is_some_and(|depth| depth <= 8)
    }));
}

#[tokio::test]
async fn semantic_snapshot_hydrates_truncated_fallback_branches() {
    let f = fixture_with(|st| {
        st.semantic_large_page = true;
        st.semantic_full_dom_fails = true;
        st.semantic_truncated_dom = true;
    })
    .await;
    let (target, tab) = bind(&f).await;
    let snap = semantic_snapshot_with(&f, &target, &tab, json!({"query": "Reply"})).await;
    assert_eq!(snap["status"], "ok", "{snap}");
    assert_eq!(snap["snapshot"]["complete"], false, "{snap}");
    assert!(snap["refs"]
        .as_array()
        .is_some_and(|refs| refs.iter().any(|entry| entry["name"] == "Reply")));
    assert!(recorded_calls(&f, "DOM.describeNode")
        .iter()
        .any(|(_, params)| params["backendNodeId"] == 999));
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
async fn protected_browser_scope_reproves_live_origin_and_omits_sensitive_url_text() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let args = json!({
        "target_id": target,
        "tab_id": tab,
        "session": SESSION,
        "x": 10,
        "y": 20,
    });

    let first = browser_protected_resource_scope(&f.engine, &args, "browser_click")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first["live_origin"], "https://fixture.test");
    assert!(
        !first.to_string().contains("secret"),
        "the resource must not contain a full URL"
    );
    let observation = browser_protected_resource_scope(&f.engine, &args, "get_browser_state")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(observation["action_class"], "page_observation");
    assert_eq!(first["action_class"], "page_input");

    f.state.lock().unwrap().main_url = "https://bank.example/transfer?secret=one-time-token".into();
    let second = browser_protected_resource_scope(&f.engine, &args, "browser_click")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second["live_origin"], "https://bank.example");
    assert_ne!(
        first, second,
        "cross-origin navigation must rotate the grant scope"
    );
    assert!(
        !second.to_string().contains("one-time-token"),
        "query text must never reach the consent resource"
    );
}

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
    let focus_emulation = recorded_calls(&f, "Emulation.setFocusEmulationEnabled");
    assert_eq!(focus_emulation.len(), 2);
    assert_eq!(focus_emulation[0].1["enabled"], true);
    assert_eq!(focus_emulation[1].1["enabled"], false);
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
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
    assert_eq!(
        structured(&trusted)["refusal"]["detail"]["alternative_route"],
        "dom_event"
    );
    assert_eq!(
        structured(&trusted)["refusal"]["detail"]["trusted_delivery_attempted"],
        false
    );
    assert!(recorded_calls(&f, "Input.dispatchMouseEvent").is_empty());

    let synthetic_args = json!({
        "target_id": target, "tab_id": tab, "ref": main_ref,
        "input_route": "dom_event", "session": SESSION
    });
    let synthetic = BrowserClickTool::new(f.engine.clone())
        .invoke(synthetic_args.clone())
        .await;
    assert_eq!(structured(&synthetic)["status"], "ok");
    assert_eq!(structured(&synthetic)["effect"], "unverifiable");
    assert_eq!(structured(&synthetic)["escalation"]["recommended"], "page");
    assert!(synthetic.content.iter().any(|content| matches!(
        content,
        crate::protocol::Content::Text { text, .. }
            if text.contains("application effect not verified")
                && text.contains("trust-gated controls")
    )));
    let public = ActionExecutionRecord::from_legacy(
        "browser_click",
        &synthetic_args,
        structured(&synthetic),
    )
    .expect("browser click action record")
    .public_result()
    .expect("public browser click result");
    let public = serde_json::to_value(public).expect("serialize public result");
    assert_eq!(public["effect"], "unverifiable", "{public}");
    assert_eq!(public["route"], "dom", "{public}");
    assert_eq!(public["delivery"]["mode"], "background", "{public}");
    assert_eq!(public["escalation"]["target"], "page", "{public}");
    assert_eq!(
        public["escalation"]["reason"], "effect_unconfirmed",
        "{public}"
    );
    assert!(public.get("status").is_none(), "{public}");
    assert!(!recorded_calls(&f, "Runtime.callFunctionOn").is_empty());
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
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
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
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

#[tokio::test]
async fn partial_keystrokes_report_exact_delivered_prefix() {
    let f = fixture_with(|state| state.fail_key_down_after = Some(2)).await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let shadow_ref = ref_of(&snap, "main", "Shadow Input");

    let result = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "ref": shadow_ref,
            "text": "four",
            "mode": "keystrokes",
            "session": SESSION
        }))
        .await;
    let refusal = &structured(&result)["refusal"];
    assert_eq!(refusal["code"], "browser_input_incomplete");
    assert_eq!(refusal["detail"]["requested_chars"], 4);
    assert_eq!(refusal["detail"]["delivered_chars"], 2);
    assert_eq!(refusal["detail"]["retryable"], false);
}

#[tokio::test]
async fn keystrokes_use_char_events_for_text_delivery() {
    let f = fixture().await;
    let (target, tab) = bind(&f).await;
    let snap = snapshot(&f, &target, &tab).await;
    let shadow_ref = ref_of(&snap, "main", "Shadow Input");

    let result = BrowserTypeTool::new(f.engine.clone())
        .invoke(json!({
            "target_id": target,
            "tab_id": tab,
            "ref": shadow_ref,
            "text": "a\n",
            "mode": "keystrokes",
            "session": SESSION
        }))
        .await;
    assert_eq!(structured(&result)["status"], "ok");

    let events = recorded_calls(&f, "Input.dispatchKeyEvent");
    assert_eq!(events.len(), 6);
    let params: Vec<&Value> = events.iter().map(|(_, params)| params).collect();
    assert_eq!(params[0]["type"], "keyDown");
    assert!(params[0].get("text").is_none());
    assert_eq!(params[1]["type"], "char");
    assert_eq!(params[1]["text"], "a");
    assert_eq!(params[1]["unmodifiedText"], "a");
    assert_eq!(params[2]["type"], "keyUp");
    assert!(params[2].get("text").is_none());
    assert_eq!(params[3]["type"], "keyDown");
    assert_eq!(params[3]["key"], "Enter");
    assert_eq!(params[4]["type"], "char");
    assert_eq!(params[4]["key"], "Enter");
    assert_eq!(params[4]["text"], "\r");
    assert_eq!(params[4]["unmodifiedText"], "\r");
    assert_eq!(params[5]["type"], "keyUp");
    let focus_emulation = recorded_calls(&f, "Emulation.setFocusEmulationEnabled");
    assert_eq!(focus_emulation.len(), 2);
    assert_eq!(focus_emulation[0].1["enabled"], true);
    assert_eq!(focus_emulation[1].1["enabled"], false);
    let readiness_checks = recorded_calls(&f, "Runtime.callFunctionOn");
    assert!(readiness_checks.iter().any(|(_, params)| {
        params["functionDeclaration"]
            .as_str()
            .is_some_and(|declaration| {
                declaration.contains("document.hasFocus()")
                    && declaration.contains("active === this")
            })
    }));
    assert!(recorded_calls(&f, "Page.bringToFront").is_empty());
    assert!(recorded_calls(&f, "Target.activateTarget").is_empty());
}
