//! Browser state, preparation, navigation, input, and page-owned dialog tools.
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

use super::approval::MCP_HOST_APPROVAL_ARG;
use super::engine::BrowserEngine;
use super::platform::{PrepareAuthorization, PrepareProfile, PrepareRequest, PrepareStrategy};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::BrowserActionKind;
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
    registry.register(Box::new(BrowserDialogTool::new(engine.clone())));
    registry.register(Box::new(BrowserSetInputFilesTool::new(engine.clone())));
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

fn semantic_ref_value(listed: &super::engine::SemanticListedRef) -> Value {
    json!({
        "ref": listed.external,
        "role": listed.node.role,
        "name": listed.node.name,
        "value": listed.node.value,
        "states": listed.node.states,
        "actions": listed.node.actions.iter().map(|action| action.as_str()).collect::<Vec<_>>(),
        "frame": listed.node.frame.kind.as_str(),
        "visibility": listed.node.visibility.as_str(),
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
                plus tab ids. Mode 2 (snapshot): pass target_id + tab_id. The \
                dom_refs_v1 compatibility format returns composed DOM refs. \
                semantic_v2 joins accessibility, DOM, layout, and viewport state; \
                ranks visible content before retained/offscreen state; and returns a \
                semantic outline, typed action refs, content refs, scoped reads, and \
                opaque continuation. Never performs setup — a missing \
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
                    "snapshot_format": {
                        "type": "string",
                        "enum": ["dom_refs_v1", "semantic_v2"],
                        "description": "Versioned snapshot contract. dom_refs_v1 remains the compatibility default."
                    },
                    "scope_ref": {
                        "type": "string",
                        "description": "Current semantic/content ref whose subtree should be observed."
                    },
                    "query": {
                        "type": "string",
                        "description": "Read-only semantic match over role, accessible name, and visible text."
                    },
                    "continuation": {
                        "type": "string",
                        "description": "Opaque continuation minted by an earlier semantic_v2 response."
                    },
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
            let snapshot_format = args
                .opt_str("snapshot_format")
                .unwrap_or_else(|| "dom_refs_v1".into());
            if snapshot_format != "dom_refs_v1" && snapshot_format != "semantic_v2" {
                return ToolResult::error(format!(
                    "snapshot_format must be \"dom_refs_v1\" or \"semantic_v2\", got {snapshot_format:?}"
                ));
            }
            if snapshot_format == "dom_refs_v1"
                && (args.opt_str("scope_ref").is_some()
                    || args.opt_str("query").is_some()
                    || args.opt_str("continuation").is_some())
            {
                return ToolResult::error(
                    "scope_ref, query, and continuation require snapshot_format=\"semantic_v2\"",
                );
            }
            if snapshot_format == "semantic_v2" {
                return match self
                    .engine
                    .snapshot_tab_semantic(
                        &session,
                        &target_id,
                        &tab_id,
                        args.opt_str("scope_ref").as_deref(),
                        args.opt_str("query").as_deref(),
                        args.opt_str("continuation").as_deref(),
                    )
                    .await
                {
                    Ok(outcome) => {
                        let refs = outcome
                            .refs
                            .iter()
                            .map(semantic_ref_value)
                            .collect::<Vec<_>>();
                        let content_refs = outcome
                            .content_refs
                            .iter()
                            .map(semantic_ref_value)
                            .collect::<Vec<_>>();
                        ToolResult::text(format!(
                            "semantic snapshot p{} of {}: {} action ref(s), {} content ref(s)",
                            outcome.snapshot_id,
                            outcome.url,
                            refs.len(),
                            content_refs.len()
                        ))
                        .with_structured(json!({
                            "status": "ok",
                            "mode": "snapshot",
                            "target_id": target_id,
                            "tab_id": tab_id,
                            "snapshot": {
                                "id": format!("p{}", outcome.snapshot_id),
                                "format": "semantic_v2",
                                "complete": outcome.complete,
                                "scope": outcome.scope,
                                "selected_nodes": outcome.selected_nodes,
                                "total_nodes": outcome.total_nodes,
                                "node_budget": super::semantic::DEFAULT_SEMANTIC_NODE_BUDGET,
                                "omitted": {
                                    "css_hidden": outcome.omissions.css_hidden,
                                    "offscreen": outcome.omissions.offscreen,
                                    "page_occluded": outcome.omissions.page_occluded,
                                    "no_layout": outcome.omissions.no_layout,
                                    "unknown": outcome.omissions.unknown,
                                    "budget": outcome.omissions.budget,
                                    "unprovable_frame": outcome.omissions.unprovable_frame,
                                },
                                "continuation": outcome.continuation,
                            },
                            "page": {
                                "url": outcome.url,
                                "title": outcome.title,
                            },
                            "outline": outcome.outline,
                            "refs": refs,
                            "content_refs": content_refs,
                            "oopif": {
                                "status": outcome.oopif.as_str(),
                                "frames": outcome.oopif.frames(),
                            },
                        }))
                    }
                    Err(refusal) => refusal.to_tool_result(),
                };
            }
            return match self
                .engine
                .snapshot_tab(&session, &target_id, &tab_id)
                .await
            {
                Ok(outcome) => {
                    let ref_list: Vec<Value> = outcome
                        .refs
                        .iter()
                        .map(|(ext, entry)| {
                            json!({
                                "ref": ext,
                                "node": entry.node_name,
                                "label": entry.label,
                                "frame": entry.frame.kind.as_str(),
                            })
                        })
                        .collect();
                    ToolResult::text(format!(
                        "snapshot p{} of {}: {} interactive element(s)",
                        outcome.snapshot_id,
                        outcome.url,
                        ref_list.len()
                    ))
                    .with_structured(json!({
                        "status": "ok",
                        "mode": "snapshot",
                        "target_id": target_id,
                        "tab_id": tab_id,
                        "snapshot_id": format!("p{}", outcome.snapshot_id),
                        "url": outcome.url,
                        "refs": ref_list,
                        "truncated": outcome.truncated,
                        "oopif": {
                            "status": outcome.oopif.as_str(),
                            "frames": outcome.oopif.frames(),
                        },
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

        let transport_session = args.opt_str("_transport_session_id");
        match self
            .engine
            .bind_native(&session, transport_session.as_deref(), pid, window_id)
            .await
        {
            Ok((target_id, record)) => {
                let tabs: Vec<Value> = record
                    .tabs
                    .values()
                    .map(|t| {
                        json!({
                            "tab_id": t.tab_id,
                            "title": t.title,
                            "url": t.url,
                            "active": t.active,
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
                pid. Existing endpoints are detected without side effects. Acting setup \
                requires MCP-host approval or a short-lived token from the interactive \
                browser-approve command, allow_launch=true, and a driver-owned isolated \
                profile. It launches a separate browser and never copies, modifies, or \
                terminates the requested user profile. Existing-profile attachment is \
                explicit, requires an exact interactive approval artifact, and never \
                treats ordinary MCP transport approval as profile consent. On proven \
                platforms, that approval also permits one bounded exact-window setup: \
                open the recognized browser product's fixed remote-debugging page, toggle \
                its uniquely matched per-instance checkbox, prove the PID-owned loopback \
                endpoint, and close the temporary tab. Every visible effect is reported; \
                ambiguity is refused."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "pid": { "type": "integer", "description": "Browser process id to prepare." },
                    "window_id": { "type": "integer", "description": "Exact native window approval anchor; required for strategy.kind=existing_profile." },
                    "approval_token": {
                        "type": "string",
                        "description": "Single-use token minted by `cua-driver browser-approve` for direct CLI/raw use. Omit for an MCP-host-approved call."
                    },
                    "allow_launch": {
                        "type": "boolean",
                        "description": "Allow a separate driver-owned isolated Chromium process to be launched (default false)."
                    },
                    "profile": {
                        "type": "object",
                        "properties": {
                            "mode": { "type": "string", "enum": ["isolated_new", "isolated_named"] },
                            "name": { "type": "string", "description": "Required only for isolated_named; 1-64 path-safe ASCII characters." }
                        },
                        "required": ["mode"],
                        "additionalProperties": false
                    },
                    "strategy": {
                        "type": "object",
                        "properties": {
                            "kind": { "type": "string", "enum": ["existing_profile"] }
                        },
                        "required": ["kind"],
                        "additionalProperties": false
                    },
                    "session": schema_session(),
                },
                "required": ["pid"],
                "additionalProperties": true
            }),
            read_only: false,
            destructive: true,
            idempotent: false,
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
        let session = match require_explicit_session(&args) {
            Ok(session) => session,
            Err(error) => return error,
        };
        let profile = match args.get("profile") {
            None | Some(Value::Null) => None,
            Some(value) => match serde_json::from_value::<PrepareProfile>(value.clone()) {
                Ok(profile) => Some(profile),
                Err(error) => {
                    return ToolResult::error(format!("invalid browser profile request: {error}"))
                }
            },
        };
        let strategy = match args.get("strategy") {
            None | Some(Value::Null) => None,
            Some(value) => match serde_json::from_value::<PrepareStrategy>(value.clone()) {
                Ok(strategy) => Some(strategy),
                Err(error) => {
                    return ToolResult::error(format!(
                        "invalid browser preparation strategy: {error}"
                    ))
                }
            },
        };
        let approval_token = args.opt_str("approval_token");
        let authorization = if strategy == Some(PrepareStrategy::ExistingProfile) {
            approval_token.map(PrepareAuthorization::ApprovalArtifact)
        } else if args
            .get(MCP_HOST_APPROVAL_ARG)
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Some(PrepareAuthorization::McpHost)
        } else {
            approval_token.map(PrepareAuthorization::ApprovalArtifact)
        };
        let request = PrepareRequest {
            pid,
            window_id: args.opt_u64("window_id"),
            session,
            transport_session: args.opt_str("_transport_session_id"),
            authorization,
            strategy,
            profile,
            allow_launch: args.opt_bool("allow_launch").unwrap_or(false),
        };
        match self.engine.prepare_browser(request).await {
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
                    "prepared_pid": outcome.prepared_pid,
                    "side_effects": outcome.side_effects,
                    "attachment": outcome.attachment,
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

        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
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
                (Input.dispatchMouseEvent), and refuses where that route cannot \
                preserve standalone-browser background posture. \
                input_route=\"dom_event\" (synthetic \
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
                            It refuses rather than foregrounding a standalone browser. \
                            \"dom_event\": synthetic full-background DOM click, only \
                            when explicitly requested."
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
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };
        if route == "trusted" && validated.record.cdp_window_id.is_some() {
            if let Some(limitation) = self
                .engine
                .platform
                .standalone_trusted_input_background_limitation()
            {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    format!(
                        "{limitation}; use input_route=\"dom_event\" with a ref for a synthetic full-background click"
                    ),
                )
                .to_tool_result();
            }
        }

        // Ref path: re-prove the ref's frame/document identity and get
        // the session (tab or contained OOPIF child) its node lives in.
        let (backend_node_id, frame_kind, cdp_session) = match &ext_ref {
            Some(r) => {
                let entry = match self
                    .engine
                    .store
                    .resolve_ref(&session, &target_id, &tab_id, r)
                {
                    Ok(entry) => entry,
                    Err(refusal) => return refusal.to_tool_result(),
                };
                if entry.semantic && !entry.actions.contains(&BrowserActionKind::Click) {
                    return BrowserRefusal::new(
                        BrowserRefusalCode::BrowserActionUnavailable,
                        format!("semantic ref {r} does not declare the click action"),
                    )
                    .to_tool_result();
                }
                let frame_session = match self
                    .engine
                    .frame_session_for_mutation(
                        &session,
                        &target_id,
                        &tab_id,
                        &validated,
                        &entry.frame,
                    )
                    .await
                {
                    Ok(s) => s,
                    Err(refusal) => return refusal.to_tool_result(),
                };
                (
                    Some(entry.backend_node_id),
                    Some(entry.frame.kind.as_str()),
                    frame_session,
                )
            }
            None => (None, None, validated.cdp_session.clone()),
        };

        let conn = &validated.conn;
        let cdp = cdp_session.as_str();

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
                    "frame": frame_kind,
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

        if let Err(error) = conn
            .call(
                Some(cdp),
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
            )
            .await
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!(
                    "the target tab could not enter CDP focus emulation for trusted input: {error}"
                ),
            )
            .to_tool_result();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;

        let mut delivery_error = None;
        for (event_type, click_count) in [("mousePressed", 1), ("mouseReleased", 1)] {
            if let Err(error) = conn
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
                delivery_error = Some(error);
                break;
            }
        }
        let cleanup_error = conn
            .call(
                Some(cdp),
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": false }),
            )
            .await
            .err();
        if let Some(error) = delivery_error {
            // Trusted input is the contract; we never silently fall back to
            // synthetic events. Focus emulation has already been unwound.
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!(
                    "trusted Input route failed ({error}) — re-run with \
                     input_route=\"dom_event\" to explicitly request a synthetic click"
                ),
            )
            .to_tool_result();
        }
        if let Some(error) = cleanup_error {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!(
                    "trusted click was acknowledged but CDP focus emulation could not be restored ({error}); delivery is unknown and must not be retried automatically"
                ),
            )
            .with_detail(json!({ "delivery": "unknown", "retryable": false }))
            .to_tool_result();
        }
        ToolResult::text(format!("clicked ({x:.0}, {y:.0}) in {tab_id}")).with_structured(json!({
            "status": "ok",
            "route": "trusted",
            "target_id": target_id,
            "tab_id": tab_id,
            "ref": ext_ref,
            "frame": frame_kind,
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

/// Editability check evaluated ON the ref's node (`this`), so focus is
/// verified against the node's own root — Document or ShadowRoot — and
/// behaves the same in the main frame, composed shadow DOM,
/// same-process iframes, and OOPIF child documents.
const EDITABLE_AND_FOCUSED_CHECK: &str = "function() { \
    const root = this.getRootNode(); \
    const active = ('activeElement' in root) ? root.activeElement : null; \
    if (active !== this) return false; \
    if (this.isContentEditable) return true; \
    if (this.tagName === 'TEXTAREA') return !this.disabled && !this.readOnly; \
    if (this.tagName !== 'INPUT') return false; \
    return !this.disabled && !this.readOnly && \
        !['button','checkbox','color','file','hidden','image','radio','range','reset','submit']\
        .includes((this.type || 'text').toLowerCase()); \
}";

const FOCUS_EMULATION_READY_CHECK: &str = "function() { \
    const root = this.getRootNode(); \
    const active = ('activeElement' in root) ? root.activeElement : null; \
    return document.hasFocus() && active === this; \
}";

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

        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(v) => v,
            Err(refusal) => return refusal.to_tool_result(),
        };

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
        if entry.semantic && !entry.actions.contains(&BrowserActionKind::Type) {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("semantic ref {ext_ref} does not declare the type action"),
            )
            .to_tool_result();
        }
        // Re-prove the ref's frame identity; typing routes to the frame's
        // own session (tab, or the contained OOPIF child session).
        let cdp_session = match self
            .engine
            .frame_session_for_mutation(&session, &target_id, &tab_id, &validated, &entry.frame)
            .await
        {
            Ok(s) => s,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let conn = &validated.conn;
        let cdp = cdp_session.as_str();

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
        // Frame- and shadow-aware editability check: evaluated on the
        // ref's own node so it works identically for the main document,
        // shadow roots (getRootNode().activeElement), same-process
        // iframes, and OOPIF child documents.
        let object_id = match conn
            .call(
                Some(cdp),
                "DOM.resolveNode",
                json!({ "backendNodeId": entry.backend_node_id }),
            )
            .await
            .ok()
            .and_then(|resolved| {
                resolved
                    .get("object")
                    .and_then(|o| o.get("objectId"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            }) {
            Some(o) => o,
            None => {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    "the ref's node no longer resolves in the live page",
                )
                .to_tool_result()
            }
        };
        let editable = conn
            .call(
                Some(cdp),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": EDITABLE_AND_FOCUSED_CHECK,
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

        let requested_chars = text.chars().count();
        let (typed, delivered_chars) = if mode == "insert_text" {
            match conn
                .call(Some(cdp), "Input.insertText", json!({ "text": text }))
                .await
            {
                Ok(_) => (Ok(()), requested_chars),
                Err(error) => (Err(error), 0),
            }
        } else {
            if let Err(error) = conn
                .call(
                    Some(cdp),
                    "Emulation.setFocusEmulationEnabled",
                    json!({ "enabled": true }),
                )
                .await
            {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    format!(
                        "the inactive tab could not enter CDP focus emulation for trusted keystrokes: {error}"
                    ),
                )
                .to_tool_result();
            }
            let mut focus_ready = false;
            let mut focus_error = None;
            for _ in 0..20 {
                if let Err(error) = conn
                    .call(
                        Some(cdp),
                        "DOM.focus",
                        json!({ "backendNodeId": entry.backend_node_id }),
                    )
                    .await
                {
                    focus_error = Some(error);
                    break;
                }
                let ready = conn
                    .call(
                        Some(cdp),
                        "Runtime.callFunctionOn",
                        json!({
                            "objectId": object_id,
                            "functionDeclaration": FOCUS_EMULATION_READY_CHECK,
                            "returnByValue": true,
                        }),
                    )
                    .await;
                if matches!(
                    ready,
                    Ok(ref value) if value["result"]["value"].as_bool() == Some(true)
                ) {
                    focus_ready = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            if !focus_ready {
                let _ = conn
                    .call(
                        Some(cdp),
                        "Emulation.setFocusEmulationEnabled",
                        json!({ "enabled": false }),
                    )
                    .await;
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserInputTrustUnavailable,
                    format!(
                        "the exact editable ref did not become focus-ready under CDP emulation{}",
                        focus_error
                            .as_ref()
                            .map(|error| format!(": {error}"))
                            .unwrap_or_default()
                    ),
                )
                .to_tool_result();
            }
            // Chromium acknowledges focus emulation before every renderer's
            // trusted-input path is ready. Edge on Linux can otherwise drop
            // the first one or two characters while still returning success.
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if let Err(error) = conn
                .call(
                    Some(cdp),
                    "DOM.focus",
                    json!({ "backendNodeId": entry.backend_node_id }),
                )
                .await
            {
                let _ = conn
                    .call(
                        Some(cdp),
                        "Emulation.setFocusEmulationEnabled",
                        json!({ "enabled": false }),
                    )
                    .await;
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    format!("the exact editable ref became stale before trusted typing: {error}"),
                )
                .to_tool_result();
            }
            let mut result = Ok(());
            let mut delivered = 0;
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
                        json!({ "type": "keyDown", "key": key }),
                    )
                    .await;
                let character = if down.is_ok() {
                    conn.call(
                        Some(cdp),
                        "Input.dispatchKeyEvent",
                        json!({
                            "type": "char",
                            "key": key,
                            "text": key_text,
                            "unmodifiedText": key_text,
                        }),
                    )
                    .await
                } else {
                    Ok(json!({}))
                };
                let up = conn
                    .call(
                        Some(cdp),
                        "Input.dispatchKeyEvent",
                        json!({ "type": "keyUp", "key": key }),
                    )
                    .await;
                if let Err(e) = down.and(character).and(up) {
                    result = Err(e);
                    break;
                }
                delivered += 1;
                tokio::time::sleep(std::time::Duration::from_millis(15)).await;
            }
            if let Err(error) = conn
                .call(
                    Some(cdp),
                    "Emulation.setFocusEmulationEnabled",
                    json!({ "enabled": false }),
                )
                .await
            {
                result = Err(error);
            }
            (result, delivered)
        };

        match typed {
            Ok(()) => ToolResult::text(format!(
                "typed {} char(s) into {tab_id}",
                requested_chars
            ))
            .with_structured(json!({
                "status": "ok",
                "target_id": target_id,
                "tab_id": tab_id,
                "ref": ext_ref,
                "frame": entry.frame.kind.as_str(),
                "mode": mode,
                "chars": requested_chars,
                "requested_chars": requested_chars,
                "delivered_chars": delivered_chars,
            })),
            Err(e) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputIncomplete,
                format!(
                    "trusted Input typing stopped after {delivered_chars} of {requested_chars} character(s): {e}"
                ),
            )
            .with_detail(json!({
                "requested_chars": requested_chars,
                "delivered_chars": delivered_chars,
                "retryable": false,
            }))
            .to_tool_result(),
        }
    }
}

// ── browser_dialog ──────────────────────────────────────────────────────────

pub struct BrowserDialogTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserDialogTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_dialog".into(),
                description: "Inspect or resolve a page-owned JavaScript alert, confirm, prompt, or beforeunload dialog on one exactly-bound tab. This never handles browser permission UI, extension UI, native dialogs, or file pickers. Inspect returns an opaque dialog_id; accept/dismiss require that exact current id.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target_id": schema_target_id(),
                        "tab_id": schema_tab_id(),
                        "session": schema_session(),
                        "action": { "type": "string", "enum": ["inspect", "accept", "dismiss"] },
                        "dialog_id": { "type": "string", "description": "Opaque current dialog generation returned by action=inspect." },
                        "prompt_text": { "type": "string", "description": "Sensitive response text, valid only when accepting a prompt dialog." }
                    },
                    "required": ["target_id", "tab_id", "action"],
                    "additionalProperties": true
                }),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            },
            engine,
        }
    }
}

#[async_trait]
impl Tool for BrowserDialogTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, action) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("action"),
        ) {
            (Ok(target), Ok(tab), Ok(action)) => (target, tab, action),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => return error,
        };
        if !matches!(action.as_str(), "inspect" | "accept" | "dismiss") {
            return ToolResult::error("action must be inspect, accept, or dismiss");
        }
        let session = match require_explicit_session(&args) {
            Ok(session) => session,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(validated) => validated,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let conn = &validated.conn;
        let cdp_session = validated.cdp_session.as_str();
        if let Err(error) = conn.call(Some(cdp_session), "Page.enable", json!({})).await {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("the exact tab does not expose JavaScript dialog events: {error}"),
            )
            .to_tool_result();
        }
        tokio::task::yield_now().await;

        let Some(dialog) = conn.dialog_state(cdp_session) else {
            return if action == "inspect" {
                ToolResult::text(format!(
                    "no page-owned JavaScript dialog is open in {tab_id}"
                ))
                .with_structured(json!({
                    "status": "ok", "target_id": target_id, "tab_id": tab_id,
                    "present": false
                }))
            } else {
                BrowserRefusal::new(
                    BrowserRefusalCode::BrowserActionUnavailable,
                    "no current page-owned JavaScript dialog exists on the exact tab",
                )
                .to_tool_result()
            };
        };
        let dialog_id = format!("dialog-{}", dialog.generation);
        if action == "inspect" {
            return ToolResult::text(format!("{} dialog is open in {tab_id}", dialog.kind))
                .with_structured(json!({
                    "status": "ok", "target_id": target_id, "tab_id": tab_id,
                    "present": true, "dialog_id": dialog_id, "kind": dialog.kind
                }));
        }

        let supplied_id = match args.require_str("dialog_id") {
            Ok(id) => id,
            Err(error) => return error,
        };
        if supplied_id != dialog_id {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the dialog capability is stale or belongs to another dialog",
            )
            .to_tool_result();
        }
        let prompt_text = args.opt_str("prompt_text");
        if prompt_text.is_some() && (action != "accept" || dialog.kind != "prompt") {
            return ToolResult::error("prompt_text is valid only when accepting a prompt dialog");
        }
        let mut params = json!({ "accept": action == "accept" });
        if let Some(text) = prompt_text {
            params["promptText"] = Value::String(text);
        }
        match conn
            .call(Some(cdp_session), "Page.handleJavaScriptDialog", params)
            .await
        {
            Ok(_) => {
                conn.clear_dialog_state(cdp_session, dialog.generation);
                ToolResult::text(format!("{action}ed {} dialog in {tab_id}", dialog.kind))
                    .with_structured(json!({
                        "status": "ok", "target_id": target_id, "tab_id": tab_id,
                        "dialog_id": dialog_id, "kind": dialog.kind, "action": action
                    }))
            }
            Err(error) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("the exact JavaScript dialog could not be resolved: {error}"),
            )
            .to_tool_result(),
        }
    }
}

// ── browser_set_input_files ─────────────────────────────────────────────────

pub struct BrowserSetInputFilesTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserSetInputFilesTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_set_input_files".into(),
                description: "Assign one or more explicit absolute local files to an exact live <input type=file> ref through CDP. This bypasses native file pickers, rejects symlinks and non-regular files, and never returns local paths.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target_id": schema_target_id(),
                        "tab_id": schema_tab_id(),
                        "session": schema_session(),
                        "ref": schema_ref(),
                        "files": {
                            "type": "array", "minItems": 1, "maxItems": 32,
                            "items": { "type": "string", "description": "Absolute path to one local regular file." }
                        }
                    },
                    "required": ["target_id", "tab_id", "ref", "files"],
                    "additionalProperties": true
                }),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true,
            },
            engine,
        }
    }
}

fn validated_upload_paths(args: &Value) -> Result<Vec<String>, ToolResult> {
    let Some(files) = args.get("files").and_then(Value::as_array) else {
        return Err(ToolResult::error(
            "files must be a non-empty array of absolute paths",
        ));
    };
    if files.is_empty() || files.len() > 32 {
        return Err(ToolResult::error(
            "files must contain between 1 and 32 paths",
        ));
    }
    files
        .iter()
        .map(|value| {
            let raw = value
                .as_str()
                .ok_or_else(|| ToolResult::error("every files entry must be a string"))?;
            let path = std::path::Path::new(raw);
            if !path.is_absolute() {
                return Err(ToolResult::error("every upload path must be absolute"));
            }
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|_| ToolResult::error("an approved upload path does not exist"))?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ToolResult::error(
                    "upload paths must name regular files directly, not links or directories",
                ));
            }
            std::fs::canonicalize(path)
                .map(|path| path.to_string_lossy().into_owned())
                .map_err(|_| ToolResult::error("an upload path could not be canonicalized"))
        })
        .collect()
}

#[async_trait]
impl Tool for BrowserSetInputFilesTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let (target_id, tab_id, ext_ref) = match (
            args.require_str("target_id"),
            args.require_str("tab_id"),
            args.require_str("ref"),
        ) {
            (Ok(target), Ok(tab), Ok(ext_ref)) => (target, tab, ext_ref),
            (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => return error,
        };
        let files = match validated_upload_paths(&args) {
            Ok(files) => files,
            Err(error) => return error,
        };
        let session = match require_explicit_session(&args) {
            Ok(session) => session,
            Err(error) => return error,
        };
        let _mutation = match self
            .engine
            .lock_mutation(&session, &target_id, &tab_id)
            .await
        {
            Ok(guard) => guard,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let validated = match self
            .engine
            .revalidate_for_mutation(&session, &target_id, Some(&tab_id))
            .await
        {
            Ok(validated) => validated,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let entry = match self
            .engine
            .store
            .resolve_ref(&session, &target_id, &tab_id, &ext_ref)
        {
            Ok(entry) => entry,
            Err(refusal) => return refusal.to_tool_result(),
        };
        if entry.semantic && !entry.actions.contains(&BrowserActionKind::Upload) {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the semantic ref is not a file-upload control",
            )
            .to_tool_result();
        }
        let cdp_session = match self
            .engine
            .frame_session_for_mutation(&session, &target_id, &tab_id, &validated, &entry.frame)
            .await
        {
            Ok(session) => session,
            Err(refusal) => return refusal.to_tool_result(),
        };
        let described = match validated
            .conn
            .call(
                Some(&cdp_session),
                "DOM.describeNode",
                json!({ "backendNodeId": entry.backend_node_id }),
            )
            .await
        {
            Ok(node) => node,
            Err(_) => {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserRefStale,
                    "the upload ref no longer resolves in the live page",
                )
                .to_tool_result()
            }
        };
        let node = &described["node"];
        let attributes = node["attributes"].as_array().cloned().unwrap_or_default();
        let is_file_input = node["nodeName"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case("input"))
            && attributes.chunks_exact(2).any(|pair| {
                pair[0]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case("type"))
                    && pair[1]
                        .as_str()
                        .is_some_and(|value| value.eq_ignore_ascii_case("file"))
            });
        if !is_file_input {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the live ref is not an input[type=file] element",
            )
            .to_tool_result();
        }
        match validated
            .conn
            .call(
                Some(&cdp_session),
                "DOM.setFileInputFiles",
                json!({ "backendNodeId": entry.backend_node_id, "files": files }),
            )
            .await
        {
            Ok(_) => ToolResult::text(format!("assigned {} file(s) in {tab_id}", files.len()))
                .with_structured(json!({
                    "status": "ok", "target_id": target_id, "tab_id": tab_id,
                    "ref": ext_ref, "frame": entry.frame.kind.as_str(), "file_count": files.len()
                })),
            Err(error) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("the browser refused the exact file input assignment: {error}"),
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
        BrowserClassification, BrowserEngineFamily, BrowserProduct, NativeWindowInfo,
        OwnedEndpoint, ProcessFingerprint,
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
                    product_kind: BrowserProduct::GoogleChrome,
                    product: Some("MockChrome".into()),
                    channel: Some("stable".into()),
                    supports_cdp: true,
                },
                3 => BrowserClassification {
                    is_browser: true,
                    engine: BrowserEngineFamily::Webkit,
                    product_kind: BrowserProduct::Safari,
                    product: Some("MockSafari".into()),
                    channel: None,
                    supports_cdp: false,
                },
                _ => BrowserClassification {
                    is_browser: false,
                    engine: BrowserEngineFamily::Unknown,
                    product_kind: BrowserProduct::Other,
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
            _request: PrepareRequest,
        ) -> Result<PrepareOutcome, BrowserRefusal> {
            Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserRequiresSetup,
                "mock endpoint needs acting setup",
            ))
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

        let prepare = BrowserPrepareTool::new(e.clone());
        assert!(prepare.def().destructive);
        assert!(!prepare.def().idempotent);
        let prepare_properties = prepare.def().input_schema["properties"]
            .as_object()
            .expect("browser_prepare properties");
        assert!(!prepare_properties.contains_key("consent"));
        assert!(!prepare_properties.contains_key("allow_restart"));
        assert!(!prepare_properties.contains_key(MCP_HOST_APPROVAL_ARG));

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
            (
                BrowserDialogTool::new(e.clone()).def().clone(),
                "browser_dialog",
            ),
            (
                BrowserSetInputFilesTool::new(e.clone()).def().clone(),
                "browser_set_input_files",
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
    fn registration_registers_all_browser_tools() {
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
                "browser_type",
                "browser_dialog",
                "browser_set_input_files"
            ]
        );
    }

    #[test]
    fn upload_paths_are_absolute_regular_files_and_outputs_need_no_path() {
        let path = std::env::temp_dir().join(format!("cua-upload-{}.txt", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"fixture").unwrap();
        let args = json!({ "files": [path.to_string_lossy()] });
        let validated = validated_upload_paths(&args).unwrap();
        assert_eq!(validated.len(), 1);
        assert!(std::path::Path::new(&validated[0]).is_absolute());
        assert!(validated_upload_paths(&json!({ "files": ["relative.txt"] })).is_err());
        std::fs::remove_file(path).unwrap();
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
    async fn prepare_requires_non_forgeable_approval_for_acting_setup() {
        let tool = BrowserPrepareTool::new(engine());
        let result = tool
            .invoke(json!({
                "pid": 1,
                "allow_launch": true,
                "profile": { "mode": "isolated_new" },
                "session": "run-1"
            }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_consent_required"
        );
    }

    #[tokio::test]
    async fn ordinary_mcp_marker_never_approves_an_existing_profile() {
        let tool = BrowserPrepareTool::new(engine());
        let result = tool
            .invoke(json!({
                "pid": 1,
                "window_id": 7,
                "strategy": { "kind": "existing_profile" },
                "session": "existing-profile-run",
                MCP_HOST_APPROVAL_ARG: true
            }))
            .await;
        let structured = structured(&result);
        assert_eq!(structured["refusal"]["code"], "browser_consent_required");
        assert!(structured["refusal"]["detail"]["approval_request_id"]
            .as_str()
            .is_some());
    }

    #[tokio::test]
    async fn existing_strategy_conflicts_fail_before_platform_setup() {
        let tool = BrowserPrepareTool::new(engine());
        let token = crate::browser::approval::mint_existing_profile_approval(
            crate::browser::approval::ExistingProfileApprovalScope {
                pid: 1,
                window_id: 7,
                session: "existing-conflict".to_owned(),
            },
        )
        .unwrap();
        let result = tool
            .invoke(json!({
                "pid": 1,
                "window_id": 7,
                "strategy": { "kind": "existing_profile" },
                "profile": { "mode": "isolated_new" },
                "approval_token": token,
                "session": "existing-conflict"
            }))
            .await;
        assert_eq!(
            structured(&result)["refusal"]["code"],
            "browser_consent_required"
        );
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
                title: "Mock".into(),
                url: "https://example.test".into(),
                active: Some(true),
                generation: 0,
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
                generation: 0,
                grant_transport_session: None,
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
                title: "Mock".into(),
                url: "https://example.test".into(),
                active: Some(true),
                generation: 0,
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
                generation: 0,
                grant_transport_session: None,
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
                generation: 0,
                grant_transport_session: None,
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
