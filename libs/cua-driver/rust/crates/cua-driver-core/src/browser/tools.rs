//! The five browser tools: get_browser_state, browser_prepare,
//! browser_navigate, browser_click, browser_type.
//!
//! Schemas, annotations, and exact-or-refused semantics live here; OS
//! identity comes from the [`BrowserPlatform`] adapter; CDP mechanics
//! from [`super::engine`]. All structured outputs share the shape
//! `{"status": "ok" | "refused", ...}`.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef, ToolRegistry};
use crate::tool_args::ArgsExt;

use super::engine::BrowserEngine;
use super::platform::PrepareRequest;
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::types::BindingQuality;

/// Register all five browser tools against one shared engine. Platform
/// crates call this from their `register_all` after constructing the
/// engine with their adapter.
pub fn register_browser_tools(engine: &Arc<BrowserEngine>, registry: &mut ToolRegistry) {
    registry.register(Box::new(GetBrowserStateTool::new(engine.clone())));
    registry.register(Box::new(BrowserPrepareTool::new(engine.clone())));
    registry.register(Box::new(BrowserNavigateTool::new(engine.clone())));
    registry.register(Box::new(BrowserClickTool::new(engine.clone())));
    registry.register(Box::new(BrowserTypeTool::new(engine.clone())));
}

// ── Shared helpers ───────────────────────────────────────────────────────────

/// The public caller session id, falling back to the daemon's internal mirror.
fn session_of(args: &Value) -> String {
    args.opt_str("session")
        .or_else(|| args.opt_str("_session_id"))
        .unwrap_or_else(|| "default".into())
}

/// Target/ref minting requires an explicit (non-default) session so the
/// capability namespace has a real owner whose end event cleans it up.
fn require_explicit_session(args: &Value) -> Result<String, ToolResult> {
    let sid = session_of(args);
    if sid.is_empty() || sid == "default" {
        return Err(ToolResult::error(
            "Browser targets and page refs are session-scoped capabilities — declare an \
             explicit session (start_session) and pass its id on this call.",
        ));
    }
    Ok(sid)
}

fn schema_target_id() -> Value {
    json!({
        "type": "string",
        "description": "Opaque browser target id minted by get_browser_state \
            (session-scoped; never a CDP id)."
    })
}

fn schema_tab_id() -> Value {
    json!({
        "type": "string",
        "description": "Opaque tab id from get_browser_state (session-scoped)."
    })
}

fn schema_ref() -> Value {
    json!({
        "type": "string",
        "description": "Page element ref in the p<snapshot>:<index> namespace from \
            get_browser_state. Refs are invalidated by navigation and by newer \
            snapshots of the same tab."
    })
}

fn schema_session() -> Value {
    json!({
        "type": "string",
        "description": "Stable caller-declared session id. Browser targets, tabs, and refs are scoped to this session."
    })
}

// ── get_browser_state ────────────────────────────────────────────────────────

pub struct GetBrowserStateTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl GetBrowserStateTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "get_browser_state".into(),
            description: "Read-only browser inspection. Mode 1 (bind): pass pid + \
                window_id of a native browser window to classify it, correlate it to \
                a CDP target (exact-or-refuse), and mint a session-scoped target id \
                plus tab ids. Mode 2 (snapshot): pass target_id + tab_id to snapshot \
                the tab's main frame and mint p<snapshot>:<index> element refs for \
                browser_click / browser_type. Never performs setup — a missing \
                endpoint is a structured browser_requires_setup refusal pointing at \
                browser_prepare."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "description": "Native browser process id (bind mode)." },
                    "window_id": { "type": "integer", "description": "Native window id owned by pid (bind mode)." },
                    "target_id": schema_target_id(),
                    "tab_id": schema_tab_id(),
                    "session": schema_session(),
                },
                "additionalProperties": true
            }),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        };
        Self { def, engine }
    }
}

#[async_trait]
impl Tool for GetBrowserStateTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Snapshot mode: target_id (+ tab_id) — uses existing capabilities.
        if let Some(target_id) = args.opt_str("target_id") {
            let session = match require_explicit_session(&args) {
                Ok(s) => s,
                Err(e) => return e,
            };
            let tab_id = match args.opt_str("tab_id") {
                Some(t) => t,
                None => {
                    return BrowserRefusal::new(
                        BrowserRefusalCode::BrowserTabRequired,
                        "snapshot mode requires a tab_id from a prior bind",
                    )
                    .to_tool_result()
                }
            };
            return match self
                .engine
                .snapshot_tab(&session, &target_id, &tab_id)
                .await
            {
                Ok((snapshot_id, url, refs)) => {
                    let ref_list: Vec<Value> = refs
                        .iter()
                        .map(|(ext, entry)| {
                            json!({
                                "ref": ext,
                                "node": entry.node_name,
                                "label": entry.label,
                            })
                        })
                        .collect();
                    ToolResult::text(format!(
                        "snapshot p{snapshot_id} of {url}: {} interactive element(s)",
                        ref_list.len()
                    ))
                    .with_structured(json!({
                        "status": "ok",
                        "mode": "snapshot",
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "snapshot_id": format!("p{snapshot_id}"),
                        "url": url,
                        "refs": ref_list,
                    }))
                }
                Err(refusal) => refusal.to_tool_result(),
            };
        }

        // Bind mode: pid + window_id.
        let pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let window_id = match args.require_u64("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let session = match require_explicit_session(&args) {
            Ok(s) => s,
            Err(e) => return e,
        };

        match self.engine.bind_native(&session, pid, window_id).await {
            Ok((target_id, record)) => {
                let tabs: Vec<Value> = record
                    .tabs
                    .values()
                    .map(|t| {
                        json!({
                            "tab_id": t.tab_id,
                            "active": t.cdp_target_id == record.cdp_target_id,
                        })
                    })
                    .collect();
                let quality = match record.quality {
                    BindingQuality::Exact => "exact",
                    BindingQuality::Heuristic => "heuristic",
                };
                let binding_route = if record.cdp_window_id.is_some() {
                    "native_cdp_window"
                } else {
                    "embedded_single_page"
                };
                ToolResult::text(format!(
                    "bound target {target_id} ({quality}) with {} tab(s)",
                    tabs.len()
                ))
                .with_structured(json!({
                    "status": "ok",
                    "mode": "bind",
                    "target_id": target_id,
                    "binding_quality": quality,
                    "binding_route": binding_route,
                    "mutation_allowed": record.quality == BindingQuality::Exact,
                    "native_title": record.native_title,
                    "tabs": tabs,
                }))
            }
            Err(refusal) => refusal.to_tool_result(),
        }
    }
}

// ── browser_prepare ──────────────────────────────────────────────────────────

pub struct BrowserPrepareTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserPrepareTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "browser_prepare".into(),
            description: "Explicitly prepare an owned DevTools endpoint for a browser \
                pid. Minimal and never implicit: get_browser_state refuses with \
                browser_requires_setup instead of calling this for you. Pass \
                consent=true to authorize consent-gated setup and allow_restart=true \
                only if the platform may restart the browser."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "description": "Browser process id to prepare." },
                    "consent": {
                        "type": "boolean",
                        "description": "Explicit caller consent for consent-gated setup (default false)."
                    },
                    "allow_restart": {
                        "type": "boolean",
                        "description": "Allow the adapter to restart the browser to enable the endpoint (default false)."
                    },
                    "session": schema_session(),
                },
                "required": ["pid"],
                "additionalProperties": true
            }),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        };
        Self { def, engine }
    }
}

#[async_trait]
impl Tool for BrowserPrepareTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let request = PrepareRequest {
            pid,
            consent_granted: args.opt_bool("consent").unwrap_or(false),
            allow_restart: args.opt_bool("allow_restart").unwrap_or(false),
        };
        match self.engine.platform.prepare_endpoint(request).await {
            Ok(outcome) => {
                let prepared = outcome.endpoint.is_some();
                ToolResult::text(format!(
                    "browser_prepare: {} — {}",
                    if prepared {
                        "endpoint available"
                    } else {
                        "no endpoint"
                    },
                    outcome.message
                ))
                .with_structured(json!({
                    "status": "ok",
                    "prepared": prepared,
                    "action": outcome.action,
                    "message": outcome.message,
                    // The ws_url itself stays internal; expose only proof metadata.
                    "endpoint_ownership": outcome.endpoint.map(|e| e.ownership),
                }))
            }
            Err(refusal) => refusal.to_tool_result(),
        }
    }
}

// ── browser_navigate ─────────────────────────────────────────────────────────

pub struct BrowserNavigateTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserNavigateTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "browser_navigate".into(),
            description: "Navigate one tab of an exactly-bound browser target to a \
                new URL (http/https/about only). Refused for heuristic bindings. \
                Navigation invalidates all p<snapshot>:<index> refs for the tab."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target_id": schema_target_id(),
                    "tab_id": schema_tab_id(),
                    "url": { "type": "string", "description": "Destination URL (http:, https:, or about:)." },
                    "session": schema_session(),
                },
                "required": ["target_id", "tab_id", "url"],
                "additionalProperties": true
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: true,
        };
        Self { def, engine }
    }
}

#[async_trait]
impl Tool for BrowserNavigateTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, url) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("url"),
        ) {
            (Ok(t), Ok(tab), Ok(u)) => (t, tab, u),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        let session = match require_explicit_session(&args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let lower = url.to_ascii_lowercase();
        if !(lower.starts_with("http://")
            || lower.starts_with("https://")
            || lower.starts_with("about:"))
        {
            return ToolResult::error(format!(
                "browser_navigate only accepts http/https/about URLs, got: {url}"
            ));
        }

        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };

        match validated
            .conn
            .call(
                Some(&validated.cdp_session),
                "Page.navigate",
                json!({ "url": url }),
            )
            .await
        {
            Ok(result) => {
                if let Some(err_text) = result.get("errorText").and_then(Value::as_str) {
                    return ToolResult::error(format!("navigation failed: {err_text}"));
                }
                // Refs die with the old document.
                self.engine
                    .store
                    .invalidate_tab_snapshots(&session, &target_id, &tab_id);
                ToolResult::text(format!("navigated {tab_id} to {url}")).with_structured(json!({
                    "status": "ok",
                    "target_id": target_id,
                    "tab_id": tab_id,
                    "url": url,
                    "refs_invalidated": true,
                }))
            }
            Err(e) => ToolResult::error(format!("Page.navigate failed: {e}")),
        }
    }
}

// ── browser_click ────────────────────────────────────────────────────────────

pub struct BrowserClickTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserClickTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "browser_click".into(),
            description: "Click a page element (by ref) or viewport coordinates in an \
                exactly-bound tab. Default route is trusted hardware-like input \
                (Input.dispatchMouseEvent); input_route=\"dom_event\" (synthetic \
                el.click(), ref required) is used only when explicitly requested. \
                Refused for heuristic bindings."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target_id": schema_target_id(),
                    "tab_id": schema_tab_id(),
                    "session": schema_session(),
                    "ref": schema_ref(),
                    "x": { "type": "number", "description": "Viewport x (CSS px) — alternative to ref." },
                    "y": { "type": "number", "description": "Viewport y (CSS px) — alternative to ref." },
                    "input_route": {
                        "type": "string",
                        "enum": ["trusted", "dom_event"],
                        "description": "\"trusted\" (default): Input.dispatchMouseEvent. \
                            \"dom_event\": synthetic DOM click, only when explicitly requested."
                    },
                },
                "required": ["target_id", "tab_id"],
                "additionalProperties": true
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        };
        Self { def, engine }
    }
}

#[async_trait]
impl Tool for BrowserClickTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id) = match (args.require_str("target_id"), args.require_str("tab_id"))
        {
            (Ok(t), Ok(tab)) => (t, tab),
            (Err(e), _) | (_, Err(e)) => return e,
        };
        let session = match require_explicit_session(&args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let route = args
            .opt_str("input_route")
            .unwrap_or_else(|| "trusted".into());
        if route != "trusted" && route != "dom_event" {
            return ToolResult::error(format!(
                "input_route must be \"trusted\" or \"dom_event\", got {route:?}"
            ));
        }
        let ext_ref = args.opt_str("ref");
        let coords = match (args.opt_f64("x"), args.opt_f64("y")) {
            (Some(x), Some(y)) => Some((x, y)),
            (None, None) => None,
            _ => return ToolResult::error("pass both x and y, or neither"),
        };
        if ext_ref.is_none() && coords.is_none() {
            return ToolResult::error("browser_click needs a ref or x/y coordinates");
        }
        if route == "dom_event" && ext_ref.is_none() {
            return ToolResult::error("input_route=dom_event requires a ref");
        }

        // Resolve the ref BEFORE revalidation? No — revalidate first so a
        // stale binding refuses before we touch the page at all.
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };

        let backend_node_id = match &ext_ref {
            Some(r) => {
                match self
                    .engine
                    .store
                    .resolve_ref(&session, &target_id, &tab_id, r)
                {
                    Ok(entry) => Some(entry.backend_node_id),
                    Err(refusal) => return refusal.to_tool_result(),
                }
            }
            None => None,
        };

        let conn = &validated.conn;
        let cdp = validated.cdp_session.as_str();

        // dom_event route: explicit opt-in only.
        if route == "dom_event" {
            let backend = backend_node_id.expect("checked above");
            let resolved = match conn
                .call(
                    Some(cdp),
                    "DOM.resolveNode",
                    json!({ "backendNodeId": backend }),
                )
                .await
            {
                Ok(v) => v,
                Err(_) => {
                    return BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRefStale,
                        "the ref's node no longer resolves in the live page",
                    )
                    .to_tool_result()
                }
            };
            let object_id = match resolved
                .get("object")
                .and_then(|o| o.get("objectId"))
                .and_then(Value::as_str)
            {
                Some(o) => o.to_owned(),
                None => {
                    return BrowserRefusal::new(
                        BrowserRefusalCode::BrowserRefStale,
                        "the ref's node has no live object in the page",
                    )
                    .to_tool_result()
                }
            };
            return match conn
                .call(
                    Some(cdp),
                    "Runtime.callFunctionOn",
                    json!({
                        "objectId": object_id,
                        "functionDeclaration": "function() { this.click(); }",
                    }),
                )
                .await
            {
                Ok(_) => ToolResult::text(format!(
                    "dispatched DOM click on {} in {tab_id}",
                    ext_ref.as_deref().unwrap_or("?")
                ))
                .with_structured(json!({
                    "status": "ok",
                    "route": "dom_event",
                    "target_id": target_id,
                    "tab_id": tab_id,
                    "ref": ext_ref,
                })),
                Err(e) => ToolResult::error(format!("DOM click failed: {e}")),
            };
        }

        // Trusted route: resolve a click point, then Input.dispatchMouseEvent.
        let (x, y) = match (backend_node_id, coords) {
            (Some(backend), _) => {
                // Best effort scroll-into-view; ignore failure (older Chromium).
                let _ = conn
                    .call(
                        Some(cdp),
                        "DOM.scrollIntoViewIfNeeded",
                        json!({ "backendNodeId": backend }),
                    )
                    .await;
                let box_model = match conn
                    .call(
                        Some(cdp),
                        "DOM.getBoxModel",
                        json!({ "backendNodeId": backend }),
                    )
                    .await
                {
                    Ok(v) => v,
                    Err(_) => {
                        return BrowserRefusal::new(
                            BrowserRefusalCode::BrowserRefStale,
                            "the ref's node has no layout box — it left the DOM or is hidden",
                        )
                        .to_tool_result()
                    }
                };
                match quad_center(&box_model) {
                    Some(pt) => pt,
                    None => {
                        return BrowserRefusal::new(
                            BrowserRefusalCode::BrowserRefStale,
                            "the ref's node returned an unusable layout box",
                        )
                        .to_tool_result()
                    }
                }
            }
            (None, Some(pt)) => pt,
            (None, None) => unreachable!("validated above"),
        };

        for (event_type, click_count) in [("mousePressed", 1), ("mouseReleased", 1)] {
            if let Err(e) = conn
                .call(
                    Some(cdp),
                    "Input.dispatchMouseEvent",
                    json!({
                        "type": event_type,
                        "x": x,
                        "y": y,
                        "button": "left",
                        "clickCount": click_count,
                    }),
                )
                .await
            {
                // Trusted input is the contract; we never silently fall
                // back to synthetic events.
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    format!(
                        "trusted Input route failed ({e}) — re-run with \
                         input_route=\"dom_event\" to explicitly request a synthetic click"
                    ),
                )
                .to_tool_result();
            }
        }
        ToolResult::text(format!("clicked ({x:.0}, {y:.0}) in {tab_id}")).with_structured(json!({
            "status": "ok",
            "route": "trusted",
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": ext_ref,
            "x": x,
            "y": y,
        }))
    }
}

/// Center of the content quad from a `DOM.getBoxModel` result.
fn quad_center(box_model: &Value) -> Option<(f64, f64)> {
    let quad = box_model.get("model")?.get("content")?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let nums: Vec<f64> = quad.iter().filter_map(Value::as_f64).collect();
    if nums.len() < 8 {
        return None;
    }
    let xs = [nums[0], nums[2], nums[4], nums[6]];
    let ys = [nums[1], nums[3], nums[5], nums[7]];
    Some((xs.iter().sum::<f64>() / 4.0, ys.iter().sum::<f64>() / 4.0))
}

// ── browser_type ─────────────────────────────────────────────────────────────

pub struct BrowserTypeTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserTypeTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        let def = ToolDef {
            name: "browser_type".into(),
            description: "Type text into an exactly-bound tab via the Input domain. \
                mode=\"insert_text\" (default) uses Input.insertText; \
                mode=\"keystrokes\" dispatches per-character key events. Pass a ref \
                to an editable element from the latest snapshot. A ref is required; \
                heuristic bindings are refused."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "target_id": schema_target_id(),
                    "tab_id": schema_tab_id(),
                    "session": schema_session(),
                    "text": { "type": "string", "description": "Text to type." },
                    "ref": schema_ref(),
                    "mode": {
                        "type": "string",
                        "enum": ["insert_text", "keystrokes"],
                        "description": "insert_text (default): bulk Input.insertText. \
                            keystrokes: per-character Input.dispatchKeyEvent."
                    },
                },
                "required": ["target_id", "tab_id", "ref", "text"],
                "additionalProperties": true
            }),
            read_only: false,
            destructive: false,
            idempotent: false,
            open_world: false,
        };
        Self { def, engine }
    }
}

#[async_trait]
impl Tool for BrowserTypeTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, text) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("text"),
        ) {
            (Ok(t), Ok(tab), Ok(x)) => (t, tab, x),
            (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => return e,
        };
        let session = match require_explicit_session(&args) {
            Ok(s) => s,
            Err(e) => return e,
        };
        let mode = args.opt_str("mode").unwrap_or_else(|| "insert_text".into());
        if mode != "insert_text" && mode != "keystrokes" {
            return ToolResult::error(format!(
                "mode must be \"insert_text\" or \"keystrokes\", got {mode:?}"
            ));
        }

        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let conn = &validated.conn;
        let cdp = validated.cdp_session.as_str();

        let ext_ref = match args.require_str("ref") {
            Ok(value) => value,
            Err(error) => return error,
        };
        let entry = match self
            .engine
            .store
            .resolve_ref(&session, &target_id, &tab_id, &ext_ref)
        {
            Ok(e) => e,
            Err(refusal) => return refusal.to_tool_result(),
        };
        if let Err(_e) = conn
            .call(
                Some(cdp),
                "DOM.focus",
                json!({ "backendNodeId": entry.backend_node_id }),
            )
            .await
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserRefStale,
                "the ref's node can no longer be focused — re-snapshot the tab",
            )
            .to_tool_result();
        }
        let editable = conn
            .call(
                Some(cdp),
                "Runtime.evaluate",
                json!({
                    "expression": "(() => { const e = document.activeElement; if (!e) return false; if (e.isContentEditable) return true; if (e.tagName === 'TEXTAREA') return !e.disabled && !e.readOnly; if (e.tagName !== 'INPUT') return false; return !e.disabled && !e.readOnly && !['button','checkbox','color','file','hidden','image','radio','range','reset','submit'].includes((e.type || 'text').toLowerCase()); })()",
                    "returnByValue": true,
                }),
            )
            .await;
        if !matches!(
            editable,
            Ok(ref value) if value["result"]["value"].as_bool() == Some(true)
        ) {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                "the requested ref is not a focused editable element",
            )
            .to_tool_result();
        }

        let typed = if mode == "insert_text" {
            conn.call(Some(cdp), "Input.insertText", json!({ "text": text }))
                .await
                .map(|_| ())
        } else {
            let mut result = Ok(());
            for ch in text.chars() {
                let (key, key_text) = if ch == '\n' {
                    ("Enter".to_string(), "\r".to_string())
                } else {
                    (ch.to_string(), ch.to_string())
                };
                let down = conn
                    .call(
                        Some(cdp),
                        "Input.dispatchKeyEvent",
                        json!({ "type": "keyDown", "key": key, "text": key_text }),
                    )
                    .await;
                let up = conn
                    .call(
                        Some(cdp),
                        "Input.dispatchKeyEvent",
                        json!({ "type": "keyUp", "key": key }),
                    )
                    .await;
                if let Err(e) = down.and(up) {
                    result = Err(e);
                    break;
                }
            }
            result
        };

        match typed {
            Ok(()) => ToolResult::text(format!(
                "typed {} char(s) into {tab_id}",
                text.chars().count()
            ))
            .with_structured(json!({
                "status": "ok",
                "target_id": target_id,
                "tab_id": tab_id,
                "ref": ext_ref,
                "mode": mode,
                "chars": text.chars().count(),
            })),
            Err(e) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("trusted Input typing route failed: {e}"),
            )
            .to_tool_result(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::platform::{BrowserPlatform, PrepareOutcome, PrepareRequest};
    use crate::browser::types::{
        BrowserClassification, BrowserEngineFamily, NativeWindowInfo, OwnedEndpoint,
        ProcessFingerprint,
    };

    /// Minimal adapter: pid 1 is a CDP-capable browser with no endpoint
    /// (setup required); pid 2 is not a browser; pid 3 is Safari-like
    /// (browser, no CDP). Prepare requires consent.
    struct MockPlatform;

    #[async_trait]
    impl BrowserPlatform for MockPlatform {
        async fn classify_browser(
            &self,
            pid: i64,
        ) -> Result<BrowserClassification, BrowserRefusal> {
            Ok(match pid {
                1 => BrowserClassification {
                    is_browser: true,
                    engine: BrowserEngineFamily::Chromium,
                    product: Some("MockChrome".into()),
                    channel: Some("stable".into()),
                    supports_cdp: true,
                },
                3 => BrowserClassification {
                    is_browser: true,
                    engine: BrowserEngineFamily::Webkit,
                    product: Some("MockSafari".into()),
                    channel: None,
                    supports_cdp: false,
                },
                _ => BrowserClassification {
                    is_browser: false,
                    engine: BrowserEngineFamily::Unknown,
                    product: None,
                    channel: None,
                    supports_cdp: false,
                },
            })
        }

        async fn native_window(
            &self,
            pid: i64,
            window_id: u64,
        ) -> Result<NativeWindowInfo, BrowserRefusal> {
            use crate::browser::types::{NativeOwnershipMethod, NativeOwnershipProof, Rect};
            Ok(NativeWindowInfo {
                pid,
                window_id,
                title: "Mock - Chrome".into(),
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
            _pid: i64,
        ) -> Result<Option<OwnedEndpoint>, BrowserRefusal> {
            Ok(None)
        }

        async fn process_fingerprint(
            &self,
            pid: i64,
        ) -> Result<ProcessFingerprint, BrowserRefusal> {
            Ok(ProcessFingerprint {
                pid,
                start_time: Some(1),
                executable: None,
            })
        }

        async fn prepare_endpoint(
            &self,
            request: PrepareRequest,
        ) -> Result<PrepareOutcome, BrowserRefusal> {
            if !request.consent_granted {
                return Err(BrowserRefusal::new(
                    BrowserRefusalCode::BrowserConsentRequired,
                    "endpoint setup requires explicit consent",
                ));
            }
            Ok(PrepareOutcome {
                action: crate::browser::platform::PrepareAction::NoOp,
                endpoint: None,
                message: "mock: nothing to do".into(),
            })
        }
    }

    fn engine() -> Arc<BrowserEngine> {
        BrowserEngine::new(Arc::new(MockPlatform))
    }

    fn structured(result: &ToolResult) -> &Value {
        result
            .structured_content
            .as_ref()
            .expect("structured content")
    }

    #[test]
    fn tool_annotations_and_schemas() {
        let e = engine();
        let state = GetBrowserStateTool::new(e.clone());
        assert!(
            state.def().read_only,
            "get_browser_state must be strictly read-only"
        );
        assert!(state.def().idempotent);

        for (def, name) in [
            (
                BrowserPrepareTool::new(e.clone()).def().clone(),
                "browser_prepare",
            ),
            (
                BrowserNavigateTool::new(e.clone()).def().clone(),
                "browser_navigate",
            ),
            (
                BrowserClickTool::new(e.clone()).def().clone(),
                "browser_click",
            ),
            (
                BrowserTypeTool::new(e.clone()).def().clone(),
                "browser_type",
            ),
        ] {
            assert_eq!(def.name, name);
            assert!(!def.read_only, "{name} is a mutation");
            assert!(def.input_schema["properties"].is_object(), "{name} schema");
        }
        // Navigate reaches the open web; the input tools do not.
        assert!(BrowserNavigateTool::new(e.clone()).def().open_world);
        assert!(!BrowserClickTool::new(e).def().open_world);
    }

    #[test]
    fn registration_registers_all_five() {
        let e = engine();
        let mut registry = ToolRegistry::new();
        register_browser_tools(&e, &mut registry);
        let names: Vec<&str> = registry.tool_names().collect();
        assert_eq!(
            names,
            vec![
                "get_browser_state",
                "browser_prepare",
                "browser_navigate",
                "browser_click",
                "browser_type"
            ]
        );
    }

    #[tokio::test]
    async fn bind_requires_an_explicit_session() {
        let e = engine();
        let tool = GetBrowserStateTool::new(e);
        for args in [
            json!({ "pid": 1, "window_id": 7 }),
            json!({ "pid": 1, "window_id": 7, "_session_id": "default" }),
        ] {
            let result = tool.invoke(args).await;
            assert_eq!(result.is_error, Some(true));
        }
    }

    #[tokio::test]
    async fn public_session_field_is_the_primary_capability_namespace() {
        let tool = GetBrowserStateTool::new(engine());
        let result = tool
            .invoke(json!({ "pid": 2, "window_id": 7, "session": "public-run" }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_route_unavailable"
        );
        assert_ne!(
            result.is_error,
            Some(true),
            "public session must pass capability-namespace validation"
        );
    }

    #[tokio::test]
    async fn non_browser_pid_refuses_route_unavailable() {
        let tool = GetBrowserStateTool::new(engine());
        let result = tool
            .invoke(json!({ "pid": 2, "window_id": 7, "_session_id": "run-1" }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_route_unavailable"
        );
    }

    #[tokio::test]
    async fn cdp_less_browser_refuses_route_unavailable() {
        let tool = GetBrowserStateTool::new(engine());
        let result = tool
            .invoke(json!({ "pid": 3, "window_id": 7, "_session_id": "run-1" }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_route_unavailable"
        );
    }

    #[tokio::test]
    async fn missing_endpoint_refuses_requires_setup_without_preparing() {
        let tool = GetBrowserStateTool::new(engine());
        let result = tool
            .invoke(json!({ "pid": 1, "window_id": 7, "_session_id": "run-1" }))
            .await;
        let s = structured(&result);
        assert_eq!(s["status"], "refused");
        assert_eq!(s["refusal"]["code"], "browser_requires_setup");
    }

    #[tokio::test]
    async fn prepare_without_consent_refuses_consent_required() {
        let tool = BrowserPrepareTool::new(engine());
        let result = tool
            .invoke(json!({ "pid": 1, "_session_id": "run-1" }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_consent_required"
        );

        let ok = tool
            .invoke(json!({ "pid": 1, "consent": true, "_session_id": "run-1" }))
            .await;
        let s = structured(&ok);
        assert_eq!(s["status"], "ok");
        assert_eq!(s["prepared"], false);
    }

    #[tokio::test]
    async fn mutations_on_unknown_targets_refuse_binding_stale() {
        let e = engine();
        for result in [
            BrowserNavigateTool::new(e.clone())
                .invoke(json!({
                    "target_id": "bt999", "tab_id": "tab1",
                    "url": "https://example.test", "_session_id": "run-1"
                }))
                .await,
            BrowserClickTool::new(e.clone())
                .invoke(json!({
                    "target_id": "bt999", "tab_id": "tab1", "ref": "p1:0",
                    "_session_id": "run-1"
                }))
                .await,
            BrowserTypeTool::new(e)
                .invoke(json!({
                    "target_id": "bt999", "tab_id": "tab1", "text": "hi",
                    "_session_id": "run-1"
                }))
                .await,
        ] {
            assert_eq!(
                structured(&result)["refusal"]["code"],
                "browser_binding_stale",
                "unknown capability must refuse, not error"
            );
        }
    }

    #[tokio::test]
    async fn mutations_reject_bad_url_and_bad_route_before_binding() {
        let e = engine();
        let nav = BrowserNavigateTool::new(e.clone())
            .invoke(json!({
                "target_id": "bt1", "tab_id": "tab1", "url": "file:///etc/passwd",
                "_session_id": "run-1"
            }))
            .await;
        assert_eq!(nav.is_error, Some(true));

        let click = BrowserClickTool::new(e)
            .invoke(json!({
                "target_id": "bt1", "tab_id": "tab1", "ref": "p1:0",
                "input_route": "sneaky", "_session_id": "run-1"
            }))
            .await;
        assert_eq!(click.is_error, Some(true));
    }

    #[tokio::test]
    async fn heuristic_bindings_refuse_mutation() {
        use crate::browser::types::{BindingQuality, Rect};
        use std::collections::HashMap;

        let e = engine();
        // Seed a heuristic binding directly into the store.
        let mut tabs = HashMap::new();
        let tab_id = e.store.mint_tab_id();
        tabs.insert(
            tab_id.clone(),
            crate::browser::store::TabRecord {
                tab_id: tab_id.clone(),
                cdp_target_id: "CDPX".into(),
                snapshots: HashMap::new(),
            },
        );
        let target_id = e.store.mint_target(
            "run-1",
            crate::browser::store::TargetRecord {
                target_id: String::new(),
                pid: 1,
                window_id: 7,
                ws_url: "ws://127.0.0.1:9222/devtools/browser/x".into(),
                endpoint_owner_pid: 1,
                fingerprint: ProcessFingerprint {
                    pid: 1,
                    start_time: Some(1),
                    executable: None,
                },
                native_title: "Mock - Chrome".into(),
                native_bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
                cdp_target_id: "CDPX".into(),
                cdp_window_id: Some(5),
                quality: BindingQuality::Heuristic,
                tabs,
            },
        );

        let result = BrowserTypeTool::new(e)
            .invoke(json!({
                "target_id": target_id, "tab_id": tab_id, "text": "hi",
                "_session_id": "run-1"
            }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_wrong_target_refused",
            "heuristic bindings are read-only"
        );
    }

    #[tokio::test]
    async fn stale_fingerprint_refuses_before_any_cdp_traffic() {
        use crate::browser::types::{BindingQuality, Rect};
        use std::collections::HashMap;

        let e = engine();
        let mut tabs = HashMap::new();
        let tab_id = e.store.mint_tab_id();
        tabs.insert(
            tab_id.clone(),
            crate::browser::store::TabRecord {
                tab_id: tab_id.clone(),
                cdp_target_id: "CDPX".into(),
                snapshots: HashMap::new(),
            },
        );
        // Bound fingerprint has start_time 999; MockPlatform now reports 1.
        let target_id = e.store.mint_target(
            "run-1",
            crate::browser::store::TargetRecord {
                target_id: String::new(),
                pid: 1,
                window_id: 7,
                ws_url: "ws://127.0.0.1:9222/devtools/browser/x".into(),
                endpoint_owner_pid: 1,
                fingerprint: ProcessFingerprint {
                    pid: 1,
                    start_time: Some(999),
                    executable: None,
                },
                native_title: "Mock - Chrome".into(),
                native_bounds: Rect::new(0.0, 0.0, 800.0, 600.0),
                cdp_target_id: "CDPX".into(),
                cdp_window_id: Some(5),
                quality: BindingQuality::Exact,
                tabs,
            },
        );

        let result = BrowserNavigateTool::new(e)
            .invoke(json!({
                "target_id": target_id, "tab_id": tab_id,
                "url": "https://example.test", "_session_id": "run-1"
            }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_binding_stale"
        );
    }

    #[tokio::test]
    async fn session_end_hook_drops_the_capability_namespace() {
        use crate::browser::types::{BindingQuality, Rect};
        use std::collections::HashMap;

        let e = engine();
        let sid = "browser-store-cleanup-session-771";
        e.store.mint_target(
            sid,
            crate::browser::store::TargetRecord {
                target_id: String::new(),
                pid: 1,
                window_id: 7,
                ws_url: "ws://127.0.0.1:9222/devtools/browser/x".into(),
                endpoint_owner_pid: 1,
                fingerprint: ProcessFingerprint {
                    pid: 1,
                    start_time: Some(1),
                    executable: None,
                },
                native_title: "T".into(),
                native_bounds: Rect::new(0.0, 0.0, 1.0, 1.0),
                cdp_target_id: "C".into(),
                cdp_window_id: Some(1),
                quality: BindingQuality::Exact,
                tabs: HashMap::new(),
            },
        );
        assert_eq!(e.store.target_count(sid), 1);
        crate::session::fire_session_end(sid);
        assert_eq!(
            e.store.target_count(sid),
            0,
            "session end must clean the store"
        );
    }
}
