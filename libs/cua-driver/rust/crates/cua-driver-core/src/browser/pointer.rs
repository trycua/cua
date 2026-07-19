//! Extended browser pointer actions.
//!
//! This module deliberately shares the exact-or-refused posture of the
//! original browser click tool. Every mutation is serialized by the real CDP
//! tab, then revalidates the native window, endpoint, tab, and (for refs) frame
//! identity before dispatch. It never activates a target or brings a page to
//! the foreground.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::protocol::ToolResult;
use crate::tool::{Tool, ToolDef};
use crate::tool_args::ArgsExt;

use super::cdp_ws::CdpConnection;
use super::engine::{BrowserEngine, ValidatedTab};
use super::refusal::{BrowserRefusal, BrowserRefusalCode};
use super::store::{BrowserActionKind, FrameKind, FrameRef};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointerAction {
    Hover,
    RightClick,
    DoubleClick,
    Scroll,
    Drag,
}

impl PointerAction {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "hover" => Ok(Self::Hover),
            "right_click" => Ok(Self::RightClick),
            "double_click" => Ok(Self::DoubleClick),
            "scroll" => Ok(Self::Scroll),
            "drag" => Ok(Self::Drag),
            _ => Err(format!(
                "action must be hover, right_click, double_click, scroll, or drag, got {value:?}"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Hover => "hover",
            Self::RightClick => "right_click",
            Self::DoubleClick => "double_click",
            Self::Scroll => "scroll",
            Self::Drag => "drag",
        }
    }
}

fn ref_declares_pointer_action(actions: &[BrowserActionKind], action: PointerAction) -> bool {
    match action {
        PointerAction::Scroll => actions
            .iter()
            .any(|kind| matches!(kind, BrowserActionKind::Scroll | BrowserActionKind::Pointer)),
        _ => actions.contains(&BrowserActionKind::Pointer),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputRoute {
    Trusted,
    DomEvent,
}

impl InputRoute {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "trusted" => Ok(Self::Trusted),
            "dom_event" => Ok(Self::DomEvent),
            _ => Err(format!(
                "input_route must be \"trusted\" or \"dom_event\", got {value:?}"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Trusted => "trusted",
            Self::DomEvent => "dom_event",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Location {
    Ref(String),
    Coordinates(f64, f64),
}

#[derive(Debug, Clone, PartialEq)]
struct PointerRequest {
    action: PointerAction,
    route: InputRoute,
    origin: Location,
    destination: Option<Location>,
    delta_x: f64,
    delta_y: f64,
}

fn explicit_session(args: &Value) -> Result<String, ToolResult> {
    let session = args
        .opt_str("session")
        .or_else(|| args.opt_str("_session_id"))
        .unwrap_or_else(|| "default".into());
    if session.is_empty() || session == "default" {
        return Err(ToolResult::error(
            "Browser targets and page refs are session-scoped capabilities - declare an explicit session (start_session) and pass its id on this call.",
        ));
    }
    Ok(session)
}

fn finite_pair(args: &Value, x_name: &str, y_name: &str) -> Result<Option<(f64, f64)>, String> {
    match (args.opt_f64(x_name), args.opt_f64(y_name)) {
        (None, None) => Ok(None),
        (Some(x), Some(y)) if x.is_finite() && y.is_finite() => Ok(Some((x, y))),
        (Some(_), Some(_)) => Err(format!("{x_name} and {y_name} must be finite numbers")),
        _ => Err(format!("pass both {x_name} and {y_name}, or neither")),
    }
}

fn one_location(
    args: &Value,
    ref_name: &str,
    x_name: &str,
    y_name: &str,
) -> Result<Option<Location>, String> {
    let reference = args.opt_str(ref_name);
    let coords = finite_pair(args, x_name, y_name)?;
    match (reference, coords) {
        (Some(_), Some(_)) => Err(format!(
            "pass either {ref_name} or {x_name}/{y_name}, not both"
        )),
        (Some(reference), None) => Ok(Some(Location::Ref(reference))),
        (None, Some((x, y))) => Ok(Some(Location::Coordinates(x, y))),
        (None, None) => Ok(None),
    }
}

fn parse_request(args: &Value) -> Result<PointerRequest, String> {
    let action = PointerAction::parse(
        args.get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required string field: action".to_owned())?,
    )?;
    let route = InputRoute::parse(
        args.get("input_route")
            .and_then(Value::as_str)
            .unwrap_or("trusted"),
    )?;
    let origin = one_location(args, "ref", "x", "y")?
        .ok_or_else(|| "browser_pointer needs a ref or x/y coordinates".to_owned())?;
    let destination = one_location(args, "destination_ref", "to_x", "to_y")?;

    if route == InputRoute::DomEvent && !matches!(origin, Location::Ref(_)) {
        return Err("input_route=dom_event requires a ref".into());
    }
    if action == PointerAction::Drag && destination.is_none() {
        return Err("action=drag requires destination_ref or to_x/to_y".into());
    }
    if action != PointerAction::Drag && destination.is_some() {
        return Err("destination_ref and to_x/to_y are valid only for action=drag".into());
    }

    let delta_x = args.opt_f64("delta_x").unwrap_or(0.0);
    let delta_y = args.opt_f64("delta_y").unwrap_or(0.0);
    if !delta_x.is_finite() || !delta_y.is_finite() {
        return Err("delta_x and delta_y must be finite numbers".into());
    }
    if action == PointerAction::Scroll && delta_x == 0.0 && delta_y == 0.0 {
        return Err("action=scroll requires a non-zero delta_x or delta_y".into());
    }
    if action != PointerAction::Scroll
        && (args.get("delta_x").is_some() || args.get("delta_y").is_some())
    {
        return Err("delta_x and delta_y are valid only for action=scroll".into());
    }

    Ok(PointerRequest {
        action,
        route,
        origin,
        destination,
        delta_x,
        delta_y,
    })
}

struct ResolvedRef {
    external: String,
    backend_node_id: i64,
    frame: FrameRef,
    cdp_session: String,
}

fn same_exact_frame(left: &FrameRef, right: &FrameRef) -> bool {
    left.kind == right.kind
        && left.oopif_target_id == right.oopif_target_id
        && left.identity == right.identity
}

fn stale(message: impl Into<String>) -> ToolResult {
    BrowserRefusal::new(BrowserRefusalCode::BrowserRefStale, message).to_tool_result()
}

async fn resolve_object(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<String, ToolResult> {
    let resolved = conn
        .call(
            Some(cdp_session),
            "DOM.resolveNode",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await
        .map_err(|_| stale("the ref's node no longer resolves in the live page"))?;
    resolved
        .get("object")
        .and_then(|object| object.get("objectId"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| stale("the ref's node has no live object in the page"))
}

async fn point_for_ref(
    conn: &CdpConnection,
    cdp_session: &str,
    backend_node_id: i64,
) -> Result<(f64, f64), ToolResult> {
    let _ = conn
        .call(
            Some(cdp_session),
            "DOM.scrollIntoViewIfNeeded",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await;
    let model = conn
        .call(
            Some(cdp_session),
            "DOM.getBoxModel",
            json!({ "backendNodeId": backend_node_id }),
        )
        .await
        .map_err(|_| stale("the ref's node has no live layout box"))?;
    quad_center(&model).ok_or_else(|| stale("the ref's node returned an unusable layout box"))
}

fn quad_center(box_model: &Value) -> Option<(f64, f64)> {
    let quad = box_model.get("model")?.get("content")?.as_array()?;
    if quad.len() < 8 {
        return None;
    }
    let values = quad
        .iter()
        .take(8)
        .map(Value::as_f64)
        .collect::<Option<Vec<_>>>()?;
    Some((
        (values[0] + values[2] + values[4] + values[6]) / 4.0,
        (values[1] + values[3] + values[5] + values[7]) / 4.0,
    ))
}

pub struct BrowserPointerTool {
    def: ToolDef,
    engine: Arc<BrowserEngine>,
}

impl BrowserPointerTool {
    pub fn new(engine: Arc<BrowserEngine>) -> Self {
        Self {
            def: ToolDef {
                name: "browser_pointer".into(),
                description: "Perform hover, right-click, double-click, scroll, or drag in an exactly-bound browser tab. Semantic refs must declare pointer for hover, right-click, double-click, and drag; scroll accepts a scroll or pointer capability. The trusted route uses CDP Input events and refuses if standalone background posture cannot be preserved. The explicit dom_event route requires a page ref and synthesizes full-background DOM events. Never activates or brings a tab to the foreground.".into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "target_id": { "type": "string", "description": "Opaque target id minted by get_browser_state." },
                        "tab_id": { "type": "string", "description": "Opaque tab id minted by get_browser_state." },
                        "session": { "type": "string", "description": "Explicit caller session owning the browser capabilities." },
                        "action": { "type": "string", "enum": ["hover", "right_click", "double_click", "scroll", "drag"] },
                        "input_route": { "type": "string", "enum": ["trusted", "dom_event"], "default": "trusted" },
                        "ref": { "type": "string", "description": "Origin page ref. Alternative to x/y." },
                        "x": { "type": "number", "description": "Origin viewport x in CSS pixels." },
                        "y": { "type": "number", "description": "Origin viewport y in CSS pixels." },
                        "destination_ref": { "type": "string", "description": "Drag destination page ref in the exact same frame." },
                        "to_x": { "type": "number", "description": "Drag destination viewport x in CSS pixels." },
                        "to_y": { "type": "number", "description": "Drag destination viewport y in CSS pixels." },
                        "delta_x": { "type": "number", "description": "Horizontal scroll delta in CSS pixels." },
                        "delta_y": { "type": "number", "description": "Vertical scroll delta in CSS pixels." }
                    },
                    "required": ["target_id", "tab_id", "session", "action"],
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

    async fn resolve_ref(
        &self,
        session: &str,
        target_id: &str,
        tab_id: &str,
        validated: &ValidatedTab,
        external: &str,
        action: PointerAction,
    ) -> Result<ResolvedRef, ToolResult> {
        let entry = self
            .engine
            .store
            .resolve_ref(session, target_id, tab_id, external)
            .map_err(|refusal| refusal.to_tool_result())?;
        let declared = ref_declares_pointer_action(&entry.actions, action);
        if entry.semantic && !declared {
            let required = if action == PointerAction::Scroll {
                "scroll or pointer"
            } else {
                "pointer"
            };
            return Err(BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!("semantic ref {external} does not declare the {required} action"),
            )
            .to_tool_result());
        }
        let cdp_session = self
            .engine
            .frame_session_for_mutation(session, target_id, tab_id, validated, &entry.frame)
            .await
            .map_err(|refusal| refusal.to_tool_result())?;
        Ok(ResolvedRef {
            external: external.to_owned(),
            backend_node_id: entry.backend_node_id,
            frame: entry.frame,
            cdp_session,
        })
    }

    fn trusted_background_refusal(&self, validated: &ValidatedTab) -> Option<ToolResult> {
        if validated.record.cdp_window_id.is_some() {
            if let Some(limitation) = self
                .engine
                .platform
                .standalone_trusted_input_background_limitation()
            {
                return Some(
                    BrowserRefusal::new(
                        BrowserRefusalCode::BrowserInputTrustUnavailable,
                        format!(
                            "{limitation}; use input_route=\"dom_event\" with refs to explicitly request synthetic full-background pointer delivery"
                        ),
                    )
                    .to_tool_result(),
                );
            }
        }
        None
    }

    async fn dom_event(
        &self,
        request: &PointerRequest,
        validated: &ValidatedTab,
        origin: &ResolvedRef,
        destination: Option<&ResolvedRef>,
    ) -> ToolResult {
        let conn = &validated.conn;
        let object_id =
            match resolve_object(conn, &origin.cdp_session, origin.backend_node_id).await {
                Ok(id) => id,
                Err(result) => return result,
            };

        let (function, arguments) = match request.action {
            PointerAction::Hover => (
                "function() { const o={bubbles:true,composed:true,view:this.ownerDocument.defaultView}; this.dispatchEvent(new PointerEvent('pointerover',o)); this.dispatchEvent(new PointerEvent('pointerenter',{...o,bubbles:false})); this.dispatchEvent(new MouseEvent('mouseover',o)); this.dispatchEvent(new MouseEvent('mouseenter',{...o,bubbles:false})); return true; }",
                json!([]),
            ),
            PointerAction::RightClick => (
                "function() { const o={bubbles:true,cancelable:true,composed:true,button:2,buttons:2,view:this.ownerDocument.defaultView}; this.dispatchEvent(new PointerEvent('pointerdown',o)); this.dispatchEvent(new MouseEvent('mousedown',o)); this.dispatchEvent(new MouseEvent('mouseup',{...o,buttons:0})); this.dispatchEvent(new PointerEvent('pointerup',{...o,buttons:0})); this.dispatchEvent(new MouseEvent('contextmenu',{...o,buttons:0})); return true; }",
                json!([]),
            ),
            PointerAction::DoubleClick => (
                "function() { const o={bubbles:true,cancelable:true,composed:true,button:0,view:this.ownerDocument.defaultView}; for (let detail=1; detail<=2; detail++) { this.dispatchEvent(new PointerEvent('pointerdown',{...o,buttons:1,detail})); this.dispatchEvent(new MouseEvent('mousedown',{...o,buttons:1,detail})); this.dispatchEvent(new MouseEvent('mouseup',{...o,buttons:0,detail})); this.dispatchEvent(new PointerEvent('pointerup',{...o,buttons:0,detail})); this.dispatchEvent(new MouseEvent('click',{...o,buttons:0,detail})); } this.dispatchEvent(new MouseEvent('dblclick',{...o,buttons:0,detail:2})); return true; }",
                json!([]),
            ),
            PointerAction::Scroll => (
                "function(dx,dy) { const o={deltaX:dx,deltaY:dy,bubbles:true,cancelable:true,composed:true,view:this.ownerDocument.defaultView}; this.dispatchEvent(new WheelEvent('wheel',o)); let n=this; while (n && n !== this.ownerDocument.documentElement) { const s=this.ownerDocument.defaultView.getComputedStyle(n); if (/(auto|scroll)/.test(s.overflow+s.overflowX+s.overflowY)) break; n=n.parentElement; } const target=n || this.ownerDocument.scrollingElement || this.ownerDocument.documentElement; const beforeX=target.scrollLeft; const beforeY=target.scrollTop; target.scrollBy(dx,dy); const changed=target.scrollLeft !== beforeX || target.scrollTop !== beforeY; if (changed) target.dispatchEvent(new Event('scroll')); return changed; }",
                json!([{ "value": request.delta_x }, { "value": request.delta_y }]),
            ),
            PointerAction::Drag => {
                let destination_argument = if let Some(destination) = destination {
                    let object_id = match resolve_object(
                        conn,
                        &destination.cdp_session,
                        destination.backend_node_id,
                    )
                    .await
                    {
                        Ok(id) => id,
                        Err(result) => return result,
                    };
                    json!([{ "objectId": object_id }, { "value": Value::Null }, { "value": Value::Null }])
                } else if let Some(Location::Coordinates(x, y)) = &request.destination {
                    if origin.frame.kind != FrameKind::Main {
                        return BrowserRefusal::new(
                            BrowserRefusalCode::BrowserWrongTargetRefused,
                            "coordinate drag destinations are only provably in the same frame for a main-frame origin; use destination_ref for iframe or OOPIF drag",
                        )
                        .to_tool_result();
                    }
                    json!([{ "value": Value::Null }, { "value": x }, { "value": y }])
                } else {
                    unreachable!("drag destination validated")
                };
                (
                    "function(destination,x,y) { const doc=this.ownerDocument; const dest=destination || doc.elementFromPoint(x,y); if (!dest || dest.ownerDocument !== doc) return false; let data=null; try { data=new DataTransfer(); } catch (_) {} const common={bubbles:true,cancelable:true,composed:true}; this.dispatchEvent(new PointerEvent('pointerdown',{...common,button:0,buttons:1})); this.dispatchEvent(new MouseEvent('mousedown',{...common,button:0,buttons:1})); this.dispatchEvent(new DragEvent('dragstart',{...common,dataTransfer:data})); dest.dispatchEvent(new DragEvent('dragenter',{...common,dataTransfer:data})); dest.dispatchEvent(new DragEvent('dragover',{...common,dataTransfer:data})); dest.dispatchEvent(new DragEvent('drop',{...common,dataTransfer:data})); this.dispatchEvent(new DragEvent('dragend',{...common,dataTransfer:data})); dest.dispatchEvent(new MouseEvent('mouseup',{...common,button:0,buttons:0})); dest.dispatchEvent(new PointerEvent('pointerup',{...common,button:0,buttons:0})); return true; }",
                    destination_argument,
                )
            }
        };

        match conn
            .call(
                Some(&origin.cdp_session),
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": function,
                    "arguments": arguments,
                    "returnByValue": true,
                }),
            )
            .await
        {
            Ok(value)
                if value.get("exceptionDetails").is_none()
                    && (!matches!(request.action, PointerAction::Scroll | PointerAction::Drag)
                        || value.pointer("/result/value").and_then(Value::as_bool)
                            != Some(false)) =>
            {
                ToolResult::text(format!(
                    "dispatched synthetic {} in {}",
                    request.action.as_str(),
                    validated.tab.tab_id
                ))
                .with_structured(self.success_json(
                    request,
                    validated,
                    origin,
                    destination,
                    None,
                    None,
                ))
            }
            Ok(value) if value.get("exceptionDetails").is_some() => BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                format!(
                    "synthetic {} raised a page-side exception and delivery was not proven",
                    request.action.as_str()
                ),
            )
            .to_tool_result(),
            Ok(_) if request.action == PointerAction::Drag => BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "the drag destination did not resolve in the origin ref's exact document",
            )
            .to_tool_result(),
            Ok(_) => BrowserRefusal::new(
                BrowserRefusalCode::BrowserActionUnavailable,
                "the synthetic scroll target did not move, so delivery was not proven",
            )
            .to_tool_result(),
            Err(error) => ToolResult::error(format!(
                "synthetic {} failed: {error}",
                request.action.as_str()
            )),
        }
    }

    fn success_json(
        &self,
        request: &PointerRequest,
        validated: &ValidatedTab,
        origin: &ResolvedRef,
        destination: Option<&ResolvedRef>,
        origin_point: Option<(f64, f64)>,
        destination_point: Option<(f64, f64)>,
    ) -> Value {
        let external_ref = (!origin.external.is_empty()).then_some(origin.external.as_str());
        json!({
            "status": "ok",
            "action": request.action.as_str(),
            "route": request.route.as_str(),
            "target_id": validated.record.target_id,
            "tab_id": validated.tab.tab_id,
            "ref": external_ref,
            "frame": origin.frame.kind.as_str(),
            "destination_ref": destination.map(|value| value.external.as_str()),
            "x": origin_point.map(|point| point.0),
            "y": origin_point.map(|point| point.1),
            "to_x": destination_point.map(|point| point.0),
            "to_y": destination_point.map(|point| point.1),
            "delta_x": (request.action == PointerAction::Scroll).then_some(request.delta_x),
            "delta_y": (request.action == PointerAction::Scroll).then_some(request.delta_y),
        })
    }

    async fn trusted(
        &self,
        request: &PointerRequest,
        validated: &ValidatedTab,
        origin_ref: Option<&ResolvedRef>,
        destination_ref: Option<&ResolvedRef>,
    ) -> ToolResult {
        let conn = &validated.conn;
        let cdp_session = origin_ref
            .map(|reference| reference.cdp_session.as_str())
            .unwrap_or(validated.cdp_session.as_str());
        let origin = match (&request.origin, origin_ref) {
            (Location::Coordinates(x, y), None) => (*x, *y),
            (Location::Ref(_), Some(reference)) => {
                match point_for_ref(conn, &reference.cdp_session, reference.backend_node_id).await {
                    Ok(point) => point,
                    Err(result) => return result,
                }
            }
            _ => unreachable!("origin resolution matches request"),
        };
        let destination = match (&request.destination, destination_ref) {
            (Some(Location::Coordinates(x, y)), None) => Some((*x, *y)),
            (Some(Location::Ref(_)), Some(reference)) => {
                match point_for_ref(conn, &reference.cdp_session, reference.backend_node_id).await {
                    Ok(point) => Some(point),
                    Err(result) => return result,
                }
            }
            (None, None) => None,
            _ => unreachable!("destination resolution matches request"),
        };

        if let Err(error) = conn
            .call(
                Some(cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": true }),
            )
            .await
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!("the target tab could not enter CDP focus emulation: {error}"),
            )
            .to_tool_result();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;

        let delivery = self
            .dispatch_trusted(conn, cdp_session, request, origin, destination)
            .await;
        let cleanup = conn
            .call(
                Some(cdp_session),
                "Emulation.setFocusEmulationEnabled",
                json!({ "enabled": false }),
            )
            .await;
        if let Err(error) = delivery {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!(
                    "trusted {} failed ({error}); no synthetic fallback was attempted",
                    request.action.as_str()
                ),
            )
            .to_tool_result();
        }
        if let Err(error) = cleanup {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserInputTrustUnavailable,
                format!(
                    "trusted {} was acknowledged but focus emulation could not be restored ({error}); delivery is unknown and must not be retried automatically",
                    request.action.as_str()
                ),
            )
            .with_detail(json!({ "delivery": "unknown", "retryable": false }))
            .to_tool_result();
        }

        let synthetic_origin = ResolvedRef {
            external: match &request.origin {
                Location::Ref(reference) => reference.clone(),
                Location::Coordinates(_, _) => String::new(),
            },
            backend_node_id: origin_ref.map_or(0, |reference| reference.backend_node_id),
            frame: origin_ref
                .map(|reference| reference.frame.clone())
                .unwrap_or_else(FrameRef::main_unproven),
            cdp_session: cdp_session.to_owned(),
        };
        ToolResult::text(format!(
            "dispatched trusted {} in {}",
            request.action.as_str(),
            validated.tab.tab_id
        ))
        .with_structured(self.success_json(
            request,
            validated,
            &synthetic_origin,
            destination_ref,
            Some(origin),
            destination,
        ))
    }

    async fn dispatch_trusted(
        &self,
        conn: &CdpConnection,
        cdp_session: &str,
        request: &PointerRequest,
        origin: (f64, f64),
        destination: Option<(f64, f64)>,
    ) -> anyhow::Result<()> {
        let call = |params: Value| conn.call(Some(cdp_session), "Input.dispatchMouseEvent", params);
        match request.action {
            PointerAction::Hover => {
                call(
                    json!({ "type": "mouseMoved", "x": origin.0, "y": origin.1, "button": "none" }),
                )
                .await?;
            }
            PointerAction::RightClick => {
                for kind in ["mousePressed", "mouseReleased"] {
                    call(json!({ "type": kind, "x": origin.0, "y": origin.1, "button": "right", "clickCount": 1 })).await?;
                }
            }
            PointerAction::DoubleClick => {
                for click_count in [1, 2] {
                    for kind in ["mousePressed", "mouseReleased"] {
                        call(json!({ "type": kind, "x": origin.0, "y": origin.1, "button": "left", "clickCount": click_count })).await?;
                    }
                }
            }
            PointerAction::Scroll => {
                call(json!({ "type": "mouseWheel", "x": origin.0, "y": origin.1, "deltaX": request.delta_x, "deltaY": request.delta_y })).await?;
            }
            PointerAction::Drag => {
                let destination = destination.expect("drag destination validated");
                call(
                    json!({ "type": "mouseMoved", "x": origin.0, "y": origin.1, "button": "none" }),
                )
                .await?;
                call(json!({ "type": "mousePressed", "x": origin.0, "y": origin.1, "button": "left", "buttons": 1, "clickCount": 1 })).await?;
                for step in 1..=8 {
                    let progress = f64::from(step) / 8.0;
                    let x = origin.0 + (destination.0 - origin.0) * progress;
                    let y = origin.1 + (destination.1 - origin.1) * progress;
                    call(json!({ "type": "mouseMoved", "x": x, "y": y, "button": "left", "buttons": 1 })).await?;
                }
                call(json!({ "type": "mouseReleased", "x": destination.0, "y": destination.1, "button": "left", "buttons": 0, "clickCount": 1 })).await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for BrowserPointerTool {
    fn def(&self) -> &ToolDef {
        &self.def
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let target_id = match args.require_str("target_id") {
            Ok(value) => value,
            Err(error) => return error,
        };
        let tab_id = match args.require_str("tab_id") {
            Ok(value) => value,
            Err(error) => return error,
        };
        let session = match explicit_session(&args) {
            Ok(value) => value,
            Err(error) => return error,
        };
        let request = match parse_request(&args) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(error),
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
            Ok(value) => value,
            Err(refusal) => return refusal.to_tool_result(),
        };
        if request.route == InputRoute::Trusted {
            if let Some(refusal) = self.trusted_background_refusal(&validated) {
                return refusal;
            }
        }

        let origin_ref = match &request.origin {
            Location::Ref(external) => match self
                .resolve_ref(
                    &session,
                    &target_id,
                    &tab_id,
                    &validated,
                    external,
                    request.action,
                )
                .await
            {
                Ok(reference) => Some(reference),
                Err(result) => return result,
            },
            Location::Coordinates(_, _) => None,
        };
        let destination_ref = match &request.destination {
            Some(Location::Ref(external)) => match self
                .resolve_ref(
                    &session,
                    &target_id,
                    &tab_id,
                    &validated,
                    external,
                    request.action,
                )
                .await
            {
                Ok(reference) => Some(reference),
                Err(result) => return result,
            },
            _ => None,
        };

        if let (Some(origin), Some(destination)) = (&origin_ref, &destination_ref) {
            if !same_exact_frame(&origin.frame, &destination.frame)
                || origin.cdp_session != destination.cdp_session
            {
                return BrowserRefusal::new(
                    BrowserRefusalCode::BrowserWrongTargetRefused,
                    "drag origin and destination refs must belong to the exact same live frame",
                )
                .to_tool_result();
            }
        }
        if request.action == PointerAction::Drag
            && origin_ref.is_none()
            && destination_ref.is_some()
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "a coordinate drag origin cannot prove it shares a frame with destination_ref; use two refs or two coordinate pairs",
            )
            .to_tool_result();
        }
        if request.action == PointerAction::Drag
            && matches!(&request.destination, Some(Location::Coordinates(_, _)))
            && origin_ref
                .as_ref()
                .is_some_and(|reference| reference.frame.kind != FrameKind::Main)
        {
            return BrowserRefusal::new(
                BrowserRefusalCode::BrowserWrongTargetRefused,
                "a viewport-coordinate drag destination is only in the same provable frame as a main-frame ref; use destination_ref for iframe or OOPIF drag",
            )
            .to_tool_result();
        }

        match request.route {
            InputRoute::DomEvent => {
                self.dom_event(
                    &request,
                    &validated,
                    origin_ref.as_ref().expect("dom route requires ref"),
                    destination_ref.as_ref(),
                )
                .await
            }
            InputRoute::Trusted => {
                self.trusted(
                    &request,
                    &validated,
                    origin_ref.as_ref(),
                    destination_ref.as_ref(),
                )
                .await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_action_and_coordinate_origin() {
        for action in ["hover", "right_click", "double_click"] {
            let parsed = parse_request(&json!({
                "action": action,
                "x": 12.5,
                "y": 8,
            }))
            .unwrap();
            assert_eq!(parsed.origin, Location::Coordinates(12.5, 8.0));
            assert_eq!(parsed.route, InputRoute::Trusted);
        }
    }

    #[test]
    fn dom_event_requires_a_ref() {
        let error = parse_request(&json!({
            "action": "hover",
            "input_route": "dom_event",
            "x": 1,
            "y": 2,
        }))
        .unwrap_err();
        assert!(error.contains("requires a ref"), "{error}");
    }

    #[test]
    fn drag_requires_exactly_one_destination_shape() {
        assert!(parse_request(&json!({ "action": "drag", "ref": "p1:0" }))
            .unwrap_err()
            .contains("requires destination"));
        assert!(parse_request(&json!({
            "action": "drag",
            "ref": "p1:0",
            "destination_ref": "p1:1",
            "to_x": 3,
            "to_y": 4,
        }))
        .unwrap_err()
        .contains("not both"));
    }

    #[test]
    fn scroll_requires_a_nonzero_finite_delta() {
        assert!(parse_request(&json!({ "action": "scroll", "ref": "p1:0" }))
            .unwrap_err()
            .contains("non-zero"));
        let parsed = parse_request(&json!({
            "action": "scroll",
            "ref": "p1:0",
            "delta_y": 240,
        }))
        .unwrap();
        assert_eq!(parsed.delta_y, 240.0);
    }

    #[test]
    fn scroll_capability_does_not_authorize_other_pointer_actions() {
        let scroll_only = [BrowserActionKind::Scroll];
        assert!(ref_declares_pointer_action(
            &scroll_only,
            PointerAction::Scroll
        ));
        for action in [
            PointerAction::Hover,
            PointerAction::RightClick,
            PointerAction::DoubleClick,
            PointerAction::Drag,
        ] {
            assert!(!ref_declares_pointer_action(&scroll_only, action));
        }

        let pointer = [BrowserActionKind::Pointer];
        assert!(ref_declares_pointer_action(&pointer, PointerAction::Scroll));
        assert!(ref_declares_pointer_action(
            &pointer,
            PointerAction::RightClick
        ));
    }

    #[test]
    fn quad_center_rejects_malformed_models() {
        assert_eq!(
            quad_center(&json!({ "model": { "content": [0, 0, 10, 0, 10, 20, 0, 20] } })),
            Some((5.0, 10.0))
        );
        assert_eq!(
            quad_center(&json!({ "model": { "content": [0, 0] } })),
            None
        );
    }

    #[test]
    fn exact_frame_comparison_includes_document_identity() {
        let first = FrameRef::main_unproven();
        let second = FrameRef::main_unproven();
        assert!(same_exact_frame(&first, &second));

        let navigated = FrameRef {
            kind: FrameKind::Main,
            oopif_target_id: None,
            identity: Some(super::super::store::FrameIdentity {
                frame_id: "main".into(),
                loader_id: "new-loader".into(),
            }),
        };
        assert!(!same_exact_frame(&first, &navigated));
    }
}
