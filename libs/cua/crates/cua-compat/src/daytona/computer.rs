//! Daytona computer use (`/computeruse/*`) on Cua desktops.
//!
//! Daytona's computer use drives an X11 desktop (Xvfb, xfce4, x11vnc and
//! noVNC) inside a Linux sandbox. Cua Spaces have a desktop on every OS
//! (Linux, macOS, Windows), reached through cua-spacesd's computer service,
//! so the same SDK calls work on all of them: screenshots, pointer and
//! keyboard input, displays, windows, accessibility trees and screen
//! recordings. The desktop belongs to the Space (it cannot be stopped apart
//! from it), so `start` makes sure it is up and `stop` reports it running.
//!
//! Shapes: daytona_toolbox_api_client 0.225.0 (`computer_use_api.py` and its
//! models), SDK behaviour in `daytona/_sync/computer_use.py` and
//! `ComputerUse.ts`.

use super::toolbox::{error, no_content, sandbox};
use super::{Daytona, daemon_error, ok_json};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use base64::Engine;
use cua_spacesd_client::{KeySpec, SpacesdClient, pb};
use http::{HeaderMap, Response, StatusCode, header};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

/// The processes Daytona's X11 stack consists of, which on a Linux Space are
/// all "the desktop session" of Cua.
pub const X11_NAMES: [&str; 4] = ["xvfb", "xfce4", "x11vnc", "novnc"];
/// The one process name that exists on every OS.
pub const DESKTOP: &str = "desktop";

#[derive(Default)]
pub struct Registry {
    recs: Mutex<HashMap<String, Vec<Rec>>>,
}

struct Rec {
    id: String,
    file_name: String,
    path: PathBuf,
    started_ms: u64,
    ended_ms: Option<u64>,
    frames: u64,
    running: Option<cua_spacesd_client::ScreenRecording>,
}

impl Registry {
    pub async fn drop_sandbox(&self, id: &str) {
        let recs = self.recs.lock().await.remove(id).unwrap_or_default();
        for r in recs {
            if let Some(run) = r.running {
                let _ = run.stop().await;
            }
            let _ = std::fs::remove_file(&r.path);
        }
    }
}

pub fn routes(d: Arc<Daytona>) -> Router {
    Router::new()
        .route("/{id}/computeruse/start", post(start))
        .route("/{id}/computeruse/stop", post(stop))
        .route("/{id}/computeruse/status", get(status))
        .route("/{id}/computeruse/process-status", get(status))
        .route(
            "/{id}/computeruse/process/{name}/status",
            get(process_status),
        )
        .route(
            "/{id}/computeruse/process/{name}/restart",
            post(process_restart),
        )
        .route("/{id}/computeruse/process/{name}/logs", get(process_logs))
        .route(
            "/{id}/computeruse/process/{name}/errors",
            get(process_errors),
        )
        .route("/{id}/computeruse/mouse/position", get(mouse_position))
        .route("/{id}/computeruse/mouse/move", post(mouse_move))
        .route("/{id}/computeruse/mouse/click", post(mouse_click))
        .route("/{id}/computeruse/mouse/drag", post(mouse_drag))
        .route("/{id}/computeruse/mouse/scroll", post(mouse_scroll))
        .route("/{id}/computeruse/mouse/down", post(mouse_down))
        .route("/{id}/computeruse/mouse/up", post(mouse_up))
        .route("/{id}/computeruse/keyboard/type", post(kb_type))
        .route("/{id}/computeruse/keyboard/key", post(kb_key))
        .route("/{id}/computeruse/keyboard/hotkey", post(kb_hotkey))
        .route("/{id}/computeruse/keyboard/down", post(kb_down))
        .route("/{id}/computeruse/keyboard/up", post(kb_up))
        .route("/{id}/computeruse/screenshot", get(shot_full))
        .route("/{id}/computeruse/screenshot/region", get(shot_region))
        .route(
            "/{id}/computeruse/screenshot/compressed",
            get(shot_compressed),
        )
        .route(
            "/{id}/computeruse/screenshot/region/compressed",
            get(shot_region_compressed),
        )
        .route("/{id}/computeruse/display/info", get(display_info))
        .route("/{id}/computeruse/display/windows", get(windows))
        .route("/{id}/computeruse/recordings", get(rec_list))
        .route("/{id}/computeruse/recordings/start", post(rec_start))
        .route("/{id}/computeruse/recordings/stop", post(rec_stop))
        .route(
            "/{id}/computeruse/recordings/{rid}",
            get(rec_get).delete(rec_delete),
        )
        .route(
            "/{id}/computeruse/recordings/{rid}/download",
            get(rec_download),
        )
        .route("/{id}/computeruse/a11y/tree", get(a11y_tree))
        .route("/{id}/computeruse/a11y/find", post(a11y_find))
        .route("/{id}/computeruse/a11y/node/focus", post(a11y_focus))
        .route("/{id}/computeruse/a11y/node/invoke", post(a11y_invoke))
        .route("/{id}/computeruse/a11y/node/value", post(a11y_value))
        .with_state(d)
}

// ── pure translations ────────────────────────────────────────────────────

/// A Daytona mouse button name.
pub fn button_of(b: Option<&str>) -> Result<pb::MouseButton, String> {
    match b.unwrap_or("left").to_ascii_lowercase().as_str() {
        "left" | "" => Ok(pb::MouseButton::Left),
        "right" => Ok(pb::MouseButton::Right),
        "middle" => Ok(pb::MouseButton::Middle),
        "back" => Ok(pb::MouseButton::Back),
        "forward" => Ok(pb::MouseButton::Forward),
        o => Err(format!("unknown mouse button {o:?}")),
    }
}

/// A Daytona key name (xdotool style: `Return`, `ctrl`, `BackSpace`,
/// `Page_Down`, `super`, `a`) as a Cua key.
pub fn key_input(name: &str) -> Result<pb::KeyInput, String> {
    let lower = name.trim().to_ascii_lowercase().replace(' ', "_");
    let alias = match lower.as_str() {
        "page_down" | "pagedown" | "next" | "pgdn" => "page_down",
        "page_up" | "pageup" | "prior" | "pgup" => "page_up",
        "super" | "super_l" | "win" | "windows" | "cmd" | "command" | "meta" => "meta",
        "ctrl" | "control" | "control_l" | "ctrl_l" => "control",
        "alt" | "alt_l" | "option" | "opt" => "alt",
        "shift_l" => "shift",
        "return" | "kp_enter" | "enter" => "enter",
        "escape" | "esc" => "escape",
        "backspace" | "back_space" => "backspace",
        "delete" | "del" => "delete",
        "up" => "arrow_up",
        "down" => "arrow_down",
        "left" => "arrow_left",
        "right" => "arrow_right",
        "caps_lock" | "capslock" => "caps_lock",
        other => other,
    };
    KeySpec::parse(alias)
        .map(|k| k.to_pb())
        .map_err(|_| format!("unknown key {name:?}"))
}

pub fn modifier_keys(names: &[String]) -> Result<Vec<i32>, String> {
    names
        .iter()
        .map(|m| {
            let k = key_input(m)?;
            match k.key {
                Some(pb::key_input::Key::Named(n))
                    if (pb::Key::Shift as i32..=pb::Key::CapsLock as i32).contains(&n) =>
                {
                    Ok(n)
                }
                _ => Err(format!("{m:?} is not a modifier")),
            }
        })
        .collect()
}

/// `"ctrl+shift+t"` (or `"ctrl shift t"`) as keys.
pub fn hotkey_keys(spec: &str) -> Result<Vec<pb::KeyInput>, String> {
    let parts: Vec<&str> = spec
        .split(|c: char| c == '+' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("keys should not be empty".into());
    }
    parts.iter().map(|p| key_input(p)).collect()
}

/// Wheel direction and amount as (delta x, delta y) in lines: down and right
/// are positive (the CSS convention `deltaY > 0` = scroll down).
pub fn scroll_delta(direction: &str, amount: i64) -> Result<(f64, f64), String> {
    let n = amount.max(1) as f64;
    match direction.to_ascii_lowercase().as_str() {
        "down" => Ok((0.0, n)),
        "up" => Ok((0.0, -n)),
        "right" => Ok((n, 0.0)),
        "left" => Ok((-n, 0.0)),
        o => Err(format!(
            "unknown scroll direction {o:?} (up, down, left or right)"
        )),
    }
}

fn int(v: &Value, k: &str) -> Option<i64> {
    v.get(k)
        .and_then(|x| x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)))
}

fn point(x: i64, y: i64) -> Option<pb::Point> {
    Some(pb::Point {
        x: x as f64,
        y: y as f64,
    })
}

/// A stable numeric id for a window/display id string.
pub fn numeric_id(s: &str) -> i64 {
    s.parse::<i64>()
        .ok()
        .filter(|n| *n >= 0)
        .unwrap_or_else(|| {
            s.bytes().fold(1469598103934665603u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(1099511628211)
            }) as i64
                & 0x7fff_ffff
        })
}

// ── plumbing ─────────────────────────────────────────────────────────────

type Sb = (crate::state::Record, SpacesdClient);

async fn open(d: &Daytona, h: &HeaderMap, id: &str) -> Result<Sb, Response<Body>> {
    let (r, c, _) = sandbox(d, h, id).await?;
    Ok((r, c))
}

fn fail(e: cua_spacesd_client::Error) -> Response<Body> {
    match e {
        cua_spacesd_client::Error::FeatureUnsupported { .. } => daemon_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "DESKTOP_UNAVAILABLE",
            format!("this Space cannot do that: {e}"),
        ),
        e => error(StatusCode::BAD_GATEWAY, e.to_string()),
    }
}

async fn body(b: Body) -> Result<Value, Response<Body>> {
    super::toolbox::body_json(b).await
}

async fn primary(c: &SpacesdClient) -> Result<pb::Display, Response<Body>> {
    let ds = c.displays().await.map_err(fail)?;
    ds.iter()
        .find(|d| d.primary)
        .or_else(|| ds.first())
        .cloned()
        .ok_or_else(|| {
            daemon_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DISPLAY_NOT_FOUND",
                "the Space has no display",
            )
        })
}

fn logical_edge(d: &pb::Display) -> f64 {
    d.bounds.as_ref().map_or(0.0, |b| b.width.max(b.height))
}

fn engine() -> base64::engine::general_purpose::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

// ── desktop processes ────────────────────────────────────────────────────

async fn is_linux(c: &SpacesdClient) -> bool {
    c.capabilities()
        .await
        .map(|k| {
            k.os.as_ref()
                .is_none_or(|o| o.family == pb::OsFamily::Linux as i32 || o.family == 0)
        })
        .unwrap_or(true)
}

async fn names(c: &SpacesdClient) -> Vec<&'static str> {
    if is_linux(c).await {
        X11_NAMES.to_vec()
    } else {
        vec![DESKTOP]
    }
}

async fn ready(c: &SpacesdClient) -> bool {
    c.desktop_readiness()
        .await
        .map(|r| r.ready)
        .unwrap_or(false)
}

fn proc_status(running: bool) -> Value {
    json!({"running": running, "pid": 0, "priority": 0, "autoRestart": true})
}

async fn all_status(c: &SpacesdClient) -> serde_json::Map<String, Value> {
    let up = ready(c).await;
    let mut m = serde_json::Map::new();
    for n in names(c).await {
        m.insert(n.to_string(), proc_status(up));
    }
    m
}

async fn start(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Err(e) = c
        .wait_desktop_ready(std::time::Duration::from_secs(60))
        .await
    {
        return daemon_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "DESKTOP_UNAVAILABLE",
            format!("the desktop is not up: {e}"),
        );
    }
    ok_json(
        json!({"message": "Computer use processes started successfully", "status": all_status(&c).await}),
    )
}

async fn stop(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    // The desktop is the Space's own: it keeps running (the status says so).
    ok_json(json!({
        "message": "The desktop belongs to the Space and keeps running; stop the sandbox to end it",
        "status": all_status(&c).await,
    }))
}

async fn status(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    ok_json(json!({"status": if ready(&c).await { "active" } else { "inactive" }}))
}

async fn known(c: &SpacesdClient, name: &str) -> Result<(), Response<Body>> {
    if names(c).await.contains(&name) || name == DESKTOP {
        Ok(())
    } else {
        Err(daemon_error(
            StatusCode::NOT_FOUND,
            "PROCESS_NOT_FOUND",
            format!("process {name} not found"),
        ))
    }
}

async fn process_status(
    State(d): State<Arc<Daytona>>,
    Path((id, name)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Err(e) = known(&c, &name).await {
        return e;
    }
    ok_json(json!({"processName": name, "running": ready(&c).await}))
}

async fn process_restart(
    State(d): State<Arc<Daytona>>,
    Path((id, name)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Err(e) = known(&c, &name).await {
        return e;
    }
    match c
        .wait_desktop_ready(std::time::Duration::from_secs(30))
        .await
    {
        Ok(_) => ok_json(
            json!({"message": format!("{name} is running (the desktop session is the Space's own)"), "processName": name}),
        ),
        Err(e) => daemon_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "DESKTOP_UNAVAILABLE",
            format!("the desktop is not up: {e}"),
        ),
    }
}

async fn process_logs(
    State(d): State<Arc<Daytona>>,
    Path((id, name)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Err(e) = known(&c, &name).await {
        return e;
    }
    let detail = c
        .desktop_readiness()
        .await
        .map(|r| r.detail)
        .unwrap_or_default();
    ok_json(json!({"processName": name, "logs": detail}))
}

async fn process_errors(
    State(d): State<Arc<Daytona>>,
    Path((id, name)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    if let Err(e) = known(&c, &name).await {
        return e;
    }
    let r = c.desktop_readiness().await.ok();
    let errs = match r {
        Some(r) if !r.ready => r.detail,
        _ => String::new(),
    };
    ok_json(json!({"processName": name, "errors": errs}))
}

// ── mouse ────────────────────────────────────────────────────────────────

async fn ptr(
    c: &SpacesdClient,
    a: pb::pointer_request::Action,
) -> Result<pb::PointerResponse, Response<Body>> {
    c.pointer(None, a).await.map_err(fail)
}

fn pos_json(p: Option<pb::Point>) -> Value {
    let p = p.unwrap_or_default();
    json!({"x": p.x.round() as i64, "y": p.y.round() as i64})
}

async fn mouse_position(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    match c.cursor_position().await {
        Ok((x, y)) => ok_json(json!({"x": x.round() as i64, "y": y.round() as i64})),
        Err(e) => fail(e),
    }
}

async fn mouse_move(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(x), Some(y)) = (int(&b, "x"), int(&b, "y")) else {
        return error(StatusCode::BAD_REQUEST, "x and y are required");
    };
    match ptr(
        &c,
        pb::pointer_request::Action::Move(pb::PointerMove {
            position: point(x, y),
            duration: None,
        }),
    )
    .await
    {
        Ok(r) => ok_json(pos_json(r.cursor_position.or(point(x, y)))),
        Err(e) => e,
    }
}

async fn mouse_click(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(x), Some(y)) = (int(&b, "x"), int(&b, "y")) else {
        return error(StatusCode::BAD_REQUEST, "x and y are required");
    };
    let button = match button_of(b.get("button").and_then(Value::as_str)) {
        Ok(b) => b,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let count = if b.get("double").and_then(Value::as_bool) == Some(true) {
        2
    } else {
        int(&b, "clicks").filter(|n| *n >= 1).unwrap_or(1) as u32
    };
    let mods: Vec<String> = b
        .get("modifiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let modifiers = match modifier_keys(&mods) {
        Ok(m) => m,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let click = pb::PointerClick {
        position: point(x, y),
        button: button as i32,
        count,
        modifiers,
    };
    match ptr(&c, pb::pointer_request::Action::Click(click)).await {
        Ok(_) => ok_json(json!({"x": x, "y": y})),
        Err(e) => e,
    }
}

async fn mouse_drag(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(sx), Some(sy), Some(ex), Some(ey)) = (
        int(&b, "startX"),
        int(&b, "startY"),
        int(&b, "endX"),
        int(&b, "endY"),
    ) else {
        return error(
            StatusCode::BAD_REQUEST,
            "startX, startY, endX and endY are required",
        );
    };
    let button = match button_of(b.get("button").and_then(Value::as_str)) {
        Ok(b) => b,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let mods: Vec<String> = b
        .get("modifiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let modifiers = match modifier_keys(&mods) {
        Ok(m) => m,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let drag = pb::PointerDrag {
        from: point(sx, sy),
        to: point(ex, ey),
        path: vec![],
        button: button as i32,
        duration: None,
        modifiers,
    };
    match ptr(&c, pb::pointer_request::Action::Drag(drag)).await {
        Ok(_) => ok_json(json!({"x": ex, "y": ey})),
        Err(e) => e,
    }
}

async fn mouse_scroll(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let (Some(x), Some(y)) = (int(&b, "x"), int(&b, "y")) else {
        return error(StatusCode::BAD_REQUEST, "x and y are required");
    };
    let (dx, dy) = match scroll_delta(
        b.get("direction").and_then(Value::as_str).unwrap_or("down"),
        int(&b, "amount").unwrap_or(1),
    ) {
        Ok(v) => v,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let s = pb::PointerScroll {
        position: point(x, y),
        delta_x: dx,
        delta_y: dy,
        unit: pb::ScrollUnit::Line as i32,
    };
    match ptr(&c, pb::pointer_request::Action::Scroll(s)).await {
        Ok(_) => ok_json(json!({"success": true})),
        Err(e) => e,
    }
}

async fn mouse_button(
    d: Arc<Daytona>,
    id: String,
    h: HeaderMap,
    b: Body,
    down: bool,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let button = match button_of(b.get("button").and_then(Value::as_str)) {
        Ok(b) => b,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    // x and y are optional: without them the button changes at the cursor.
    let position = match (int(&b, "x"), int(&b, "y")) {
        (Some(x), Some(y)) => point(x, y),
        _ => match c.cursor_position().await {
            Ok((x, y)) => Some(pb::Point { x, y }),
            Err(e) => return fail(e),
        },
    };
    let a = if down {
        pb::pointer_request::Action::Down(pb::PointerDown {
            position,
            button: button as i32,
        })
    } else {
        pb::pointer_request::Action::Up(pb::PointerUp {
            position,
            button: button as i32,
        })
    };
    match ptr(&c, a).await {
        Ok(_) => ok_json(pos_json(position)),
        Err(e) => e,
    }
}

async fn mouse_down(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    mouse_button(d, id, h, b, true).await
}

async fn mouse_up(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    mouse_button(d, id, h, b, false).await
}

// ── keyboard ─────────────────────────────────────────────────────────────

async fn kb(c: &SpacesdClient, a: pb::keyboard_request::Action) -> Result<(), Response<Body>> {
    c.keyboard(None, a).await.map(|_| ()).map_err(fail)
}

async fn kb_type(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(text) = b.get("text").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "text is required");
    };
    let delay = int(&b, "delay")
        .filter(|d| *d > 0)
        .map(|ms| pbjson_types::Duration {
            seconds: ms / 1000,
            nanos: ((ms % 1000) * 1_000_000) as i32,
        });
    let a = pb::keyboard_request::Action::Type(pb::KeyboardType {
        text: text.into(),
        mode: pb::TextEntryMode::Keystrokes as i32,
        delay,
    });
    match kb(&c, a).await {
        Ok(()) => ok_json(json!({})),
        Err(e) => e,
    }
}

async fn kb_key(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(key) = b.get("key").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "key is required");
    };
    let key = match key_input(key) {
        Ok(k) => k,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let mods: Vec<String> = b
        .get("modifiers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let modifiers = match modifier_keys(&mods) {
        Ok(m) => m,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let a = pb::keyboard_request::Action::Press(pb::KeyboardPress {
        key: Some(key),
        modifiers,
        repeat: 1,
    });
    match kb(&c, a).await {
        Ok(()) => ok_json(json!({})),
        Err(e) => e,
    }
}

async fn kb_hotkey(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let keys = match hotkey_keys(b.get("keys").and_then(Value::as_str).unwrap_or("")) {
        Ok(k) => k,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    match kb(
        &c,
        pb::keyboard_request::Action::Hotkey(pb::KeyboardHotkey { keys }),
    )
    .await
    {
        Ok(()) => ok_json(json!({})),
        Err(e) => e,
    }
}

async fn kb_updown(
    d: Arc<Daytona>,
    id: String,
    h: HeaderMap,
    b: Body,
    down: bool,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(key) = b.get("key").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "key is required");
    };
    let key = match key_input(key) {
        Ok(k) => Some(k),
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    let a = if down {
        pb::keyboard_request::Action::Down(pb::KeyboardDown { key })
    } else {
        pb::keyboard_request::Action::Up(pb::KeyboardUp { key })
    };
    match kb(&c, a).await {
        Ok(()) => ok_json(json!({})),
        Err(e) => e,
    }
}

async fn kb_down(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    kb_updown(d, id, h, b, true).await
}

async fn kb_up(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    kb_updown(d, id, h, b, false).await
}

// ── screenshots ──────────────────────────────────────────────────────────

#[derive(serde::Deserialize, Default)]
struct ShotQuery {
    x: Option<f64>,
    y: Option<f64>,
    width: Option<f64>,
    height: Option<f64>,
    #[serde(rename = "showCursor")]
    show_cursor: Option<bool>,
    format: Option<String>,
    quality: Option<u32>,
    scale: Option<f64>,
}

fn format_of(f: Option<&str>) -> Result<pb::ImageFormat, String> {
    match f.map(str::to_ascii_lowercase).as_deref() {
        None | Some("") | Some("png") => Ok(pb::ImageFormat::Png),
        Some("jpeg") | Some("jpg") => Ok(pb::ImageFormat::Jpeg),
        Some("webp") => Ok(pb::ImageFormat::Webp),
        Some(o) => Err(format!("unsupported format {o:?} (png, jpeg or webp)")),
    }
}

async fn shot(
    d: Arc<Daytona>,
    id: String,
    h: HeaderMap,
    q: ShotQuery,
    region: bool,
    compressed: bool,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let format = match format_of(if compressed {
        q.format.as_deref()
    } else {
        None
    }) {
        Ok(f) => f,
        Err(m) => return error(StatusCode::BAD_REQUEST, m),
    };
    if let Some(s) = q.scale
        && !(s > 0.0 && s <= 1.0)
    {
        return error(StatusCode::BAD_REQUEST, "scale must be in (0, 1]");
    }
    let region = if region {
        let (Some(x), Some(y), Some(w), Some(hh)) = (q.x, q.y, q.width, q.height) else {
            return error(
                StatusCode::BAD_REQUEST,
                "x, y, width and height are required",
            );
        };
        if w <= 0.0 || hh <= 0.0 {
            return error(StatusCode::BAD_REQUEST, "width and height must be positive");
        }
        Some(pb::Rect {
            x,
            y,
            width: w,
            height: hh,
        })
    } else {
        None
    };
    let disp = match primary(&c).await {
        Ok(d) => d,
        Err(e) => return e,
    };
    // Daytona coordinates are the screenshot's pixels: ask for the image at
    // the display's logical size (a 2x display would otherwise return 2x).
    let edge = match &region {
        Some(r) => r.width.max(r.height),
        None => logical_edge(&disp),
    };
    let scale = q.scale.unwrap_or(1.0);
    let max_dimension = if (disp.scale_factor > 1.0 || scale < 1.0) && edge > 0.0 {
        (edge * scale).round().max(1.0) as u32
    } else {
        0
    };
    let req = pb::ScreenshotRequest {
        source: Some(pb::screenshot_request::Source::DisplayId(disp.id.clone())),
        region,
        format: format as i32,
        quality: q.quality.unwrap_or(0).min(100),
        max_dimension,
        include_cursor: q.show_cursor.unwrap_or(false),
    };
    let resp = match c.computer().screenshot(req).await {
        Ok(r) => r.into_inner(),
        Err(s) => return fail(s.into()),
    };
    let cursor = c.cursor_position().await.ok();
    ok_json(json!({
        "screenshot": engine().encode(&resp.image),
        "cursorPosition": cursor.map(|(x, y)| json!({"x": x.round() as i64, "y": y.round() as i64})),
        "sizeBytes": resp.image.len(),
    }))
}

async fn shot_full(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<ShotQuery>,
) -> Response<Body> {
    shot(d, id, h, q, false, false).await
}

async fn shot_region(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<ShotQuery>,
) -> Response<Body> {
    shot(d, id, h, q, true, false).await
}

async fn shot_compressed(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<ShotQuery>,
) -> Response<Body> {
    shot(d, id, h, q, false, true).await
}

async fn shot_region_compressed(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<ShotQuery>,
) -> Response<Body> {
    shot(d, id, h, q, true, true).await
}

// ── displays and windows ─────────────────────────────────────────────────

async fn display_info(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let ds = match c.displays().await {
        Ok(d) => d,
        Err(e) => return fail(e),
    };
    let out: Vec<Value> = ds
        .iter()
        .enumerate()
        .map(|(i, dd)| {
            let b = dd.bounds.unwrap_or_default();
            json!({
                "id": dd.id.parse::<i64>().unwrap_or(i as i64),
                "x": b.x.round() as i64, "y": b.y.round() as i64,
                "width": b.width.round() as i64, "height": b.height.round() as i64,
                "isActive": dd.primary,
            })
        })
        .collect();
    ok_json(json!({"displays": out}))
}

async fn windows(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let ws = match c
        .windows()
        .list_windows(pb::ListWindowsRequest {
            filter: Some(pb::WindowFilter {
                on_screen_only: true,
                ..Default::default()
            }),
        })
        .await
    {
        Ok(r) => r.into_inner().windows,
        Err(s) => return fail(s.into()),
    };
    let out: Vec<Value> = ws
        .iter()
        .map(|w| {
            let b = w.bounds.unwrap_or_default();
            json!({
                "id": numeric_id(w.r#ref.as_ref().map_or("", |r| r.id.as_str())),
                "title": w.title,
                "x": b.x.round() as i64, "y": b.y.round() as i64,
                "width": b.width.round() as i64, "height": b.height.round() as i64,
                "isActive": w.focused,
            })
        })
        .collect();
    ok_json(json!({"windows": out, "count": out.len()}))
}

// ── recordings ───────────────────────────────────────────────────────────

fn rfc3339(ms: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

fn rec_json(r: &Rec) -> Value {
    let bytes = std::fs::metadata(&r.path).map(|m| m.len()).ok();
    let end = r.ended_ms.unwrap_or_else(crate::state::now_ms);
    json!({
        "id": r.id,
        "fileName": r.file_name,
        "filePath": format!("/home/daytona/.daytona/recordings/{}", r.file_name),
        "status": if r.running.is_some() { "recording" } else { "completed" },
        "startTime": rfc3339(r.started_ms),
        "endTime": r.ended_ms.map(rfc3339),
        "durationSeconds": (end.saturating_sub(r.started_ms)) as f64 / 1000.0,
        "sizeBytes": bytes,
    })
}

async fn rec_start(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (r, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let label = b
        .get("label")
        .and_then(Value::as_str)
        .unwrap_or("recording");
    let safe: String = label
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let rid = super::api_uuid();
    let file_name = format!("{safe}-{}.mp4", &rid[..8]);
    let dir = d
        .cfg
        .dir
        .clone()
        .unwrap_or_else(std::env::temp_dir)
        .join("recordings")
        .join(&r.id);
    let path = dir.join(&file_name);
    let run = match c
        .record_screen(cua_spacesd_client::RecordOptions::new(&path))
        .await
    {
        Ok(run) => run,
        Err(e) => {
            let m = e.to_string();
            let code = if m.contains("ffmpeg") {
                "RECORDING_FFMPEG_NOT_FOUND"
            } else {
                "RECORDING_FAILED"
            };
            return daemon_error(StatusCode::SERVICE_UNAVAILABLE, code, m);
        }
    };
    let rec = Rec {
        id: rid,
        file_name,
        path,
        started_ms: crate::state::now_ms(),
        ended_ms: None,
        frames: 0,
        running: Some(run),
    };
    let out = rec_json(&rec);
    d.computer
        .recs
        .lock()
        .await
        .entry(r.id.clone())
        .or_default()
        .push(rec);
    super::toolbox::json_status(StatusCode::CREATED, out)
}

async fn rec_stop(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (r, _) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let Some(rid) = b.get("id").and_then(Value::as_str) else {
        return error(StatusCode::BAD_REQUEST, "id is required");
    };
    let mut m = d.computer.recs.lock().await;
    let Some(rec) = m
        .get_mut(&r.id)
        .and_then(|v| v.iter_mut().find(|x| x.id == rid))
    else {
        return super::toolbox::error(StatusCode::NOT_FOUND, format!("recording {rid} not found"));
    };
    let Some(run) = rec.running.take() else {
        return error(
            StatusCode::BAD_REQUEST,
            format!("recording {rid} is not running"),
        );
    };
    match run.stop().await {
        Ok(done) => {
            rec.frames = done.frames;
            rec.ended_ms = Some(crate::state::now_ms());
            ok_json(rec_json(rec))
        }
        Err(e) => daemon_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "RECORDING_FAILED",
            e.to_string(),
        ),
    }
}

async fn rec_list(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> Response<Body> {
    let (r, _) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let m = d.computer.recs.lock().await;
    let list: Vec<Value> = m
        .get(&r.id)
        .map(|v| v.iter().map(rec_json).collect())
        .unwrap_or_default();
    ok_json(json!({"recordings": list}))
}

async fn rec_get(
    State(d): State<Arc<Daytona>>,
    Path((id, rid)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (r, _) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let m = d.computer.recs.lock().await;
    match m.get(&r.id).and_then(|v| v.iter().find(|x| x.id == rid)) {
        Some(rec) => ok_json(rec_json(rec)),
        None => super::toolbox::error(StatusCode::NOT_FOUND, format!("recording {rid} not found")),
    }
}

async fn rec_delete(
    State(d): State<Arc<Daytona>>,
    Path((id, rid)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (r, _) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let mut m = d.computer.recs.lock().await;
    let list = m.entry(r.id.clone()).or_default();
    let Some(i) = list.iter().position(|x| x.id == rid) else {
        return super::toolbox::error(StatusCode::NOT_FOUND, format!("recording {rid} not found"));
    };
    if list[i].running.is_some() {
        return daemon_error(
            StatusCode::CONFLICT,
            "RECORDING_STILL_ACTIVE",
            format!("recording {rid} is still running: stop it first"),
        );
    }
    let rec = list.remove(i);
    let _ = std::fs::remove_file(&rec.path);
    no_content()
}

async fn rec_download(
    State(d): State<Arc<Daytona>>,
    Path((id, rid)): Path<(String, String)>,
    h: HeaderMap,
) -> Response<Body> {
    let (r, _) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let path = {
        let m = d.computer.recs.lock().await;
        match m.get(&r.id).and_then(|v| v.iter().find(|x| x.id == rid)) {
            Some(rec) if rec.running.is_some() => {
                return daemon_error(
                    StatusCode::CONFLICT,
                    "RECORDING_STILL_ACTIVE",
                    format!("recording {rid} is still running: stop it first"),
                );
            }
            Some(rec) => rec.path.clone(),
            None => {
                return super::toolbox::error(
                    StatusCode::NOT_FOUND,
                    format!("recording {rid} not found"),
                );
            }
        }
    };
    match tokio::fs::read(&path).await {
        Ok(data) => Response::builder()
            .header(header::CONTENT_TYPE, "video/mp4")
            .header(header::CONTENT_LENGTH, data.len())
            .body(Body::from(data))
            .expect("response"),
        Err(e) => super::toolbox::error(StatusCode::NOT_FOUND, format!("recording file: {e}")),
    }
}

// ── accessibility ────────────────────────────────────────────────────────

fn a11y_unavailable(e: &cua_spacesd_client::Error) -> Response<Body> {
    daemon_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "A11Y_UNAVAILABLE",
        format!("the accessibility tree is not available in this Space: {e}"),
    )
}

fn node_json(n: &pb::AccessibilityNode, snapshot: &str) -> Value {
    let b = n.bounds.unwrap_or_default();
    json!({
        "id": format!("{snapshot}:{}", n.element_id),
        "role": n.role,
        "name": n.name,
        "description": n.description,
        "states": n.states,
        "actions": n.actions.iter().filter_map(|a| pb::AccessibilityAction::try_from(*a).ok()).map(|a| a.as_str_name().trim_start_matches("ACCESSIBILITY_ACTION_").to_ascii_lowercase()).collect::<Vec<_>>(),
        "bounds": {"x": b.x.round() as i64, "y": b.y.round() as i64, "width": b.width.round() as i64, "height": b.height.round() as i64},
    })
}

/// Flat nodes (with parent ids and depths) as a nested tree under `root`.
pub fn nest(nodes: &[pb::AccessibilityNode], snapshot: &str) -> Vec<Value> {
    let mut by_parent: HashMap<&str, Vec<&pb::AccessibilityNode>> = HashMap::new();
    for n in nodes {
        by_parent.entry(n.parent_id.as_str()).or_default().push(n);
    }
    fn build(
        n: &pb::AccessibilityNode,
        by_parent: &HashMap<&str, Vec<&pb::AccessibilityNode>>,
        snapshot: &str,
        depth: usize,
    ) -> Value {
        let mut v = node_json(n, snapshot);
        if depth < 256
            && let Some(ch) = by_parent.get(n.element_id.as_str())
        {
            v["children"] = Value::Array(
                ch.iter()
                    .map(|c| build(c, by_parent, snapshot, depth + 1))
                    .collect(),
            );
        }
        v
    }
    let ids: std::collections::HashSet<&str> =
        nodes.iter().map(|n| n.element_id.as_str()).collect();
    nodes
        .iter()
        .filter(|n| n.parent_id.is_empty() || !ids.contains(n.parent_id.as_str()))
        .map(|n| build(n, &by_parent, snapshot, 0))
        .collect()
}

#[derive(serde::Deserialize, Default)]
struct TreeQuery {
    scope: Option<String>,
    pid: Option<u32>,
    #[serde(rename = "maxDepth")]
    max_depth: Option<u32>,
}

/// The windows an accessibility call covers: the focused one (`scope`
/// `focused`, the default), all on-screen windows (`all`), or those of `pid`.
async fn scope_windows(
    c: &SpacesdClient,
    scope: Option<&str>,
    pid: Option<u32>,
) -> Result<Vec<pb::WindowInfo>, Response<Body>> {
    let filter = pb::WindowFilter {
        on_screen_only: true,
        pid: pid.unwrap_or(0),
        ..Default::default()
    };
    let ws = c
        .windows()
        .list_windows(pb::ListWindowsRequest {
            filter: Some(filter),
        })
        .await
        .map_err(|s| fail(s.into()))?
        .into_inner()
        .windows;
    Ok(match (scope.unwrap_or("focused"), pid) {
        ("all", _) | (_, Some(_)) => ws,
        _ => ws.into_iter().filter(|w| w.focused).take(1).collect(),
    })
}

async fn a11y_tree(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    Query(q): Query<TreeQuery>,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let ws = match scope_windows(&c, q.scope.as_deref(), q.pid).await {
        Ok(w) => w,
        Err(e) => return e,
    };
    let mut roots = Vec::new();
    let mut truncated = false;
    for w in &ws {
        let t = match c
            .accessibility()
            .get_tree(pb::GetTreeRequest {
                window: w.r#ref.clone(),
                max_depth: q.max_depth.unwrap_or(0),
                include_hidden: false,
                max_nodes: 5000,
            })
            .await
        {
            Ok(t) => t.into_inner(),
            Err(s) => return a11y_unavailable(&s.into()),
        };
        truncated |= t.truncated;
        for mut n in nest(&t.nodes, &t.snapshot_id) {
            if n["name"].as_str().unwrap_or("").is_empty() {
                n["name"] = json!(w.title);
            }
            roots.push(n);
        }
    }
    let root = match roots.len() {
        1 => roots.remove(0),
        _ => json!({"role": "desktop", "name": "", "children": roots}),
    };
    ok_json(json!({"root": root, "truncated": truncated}))
}

async fn a11y_find(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let limit = int(&b, "limit").filter(|l| *l > 0).unwrap_or(100) as usize;
    let name = b.get("name").and_then(Value::as_str).unwrap_or("");
    let mode = b
        .get("nameMatch")
        .and_then(Value::as_str)
        .unwrap_or("exact");
    let regex = (!name.is_empty() && mode == "regex").then(|| regex::Regex::new(name));
    if let Some(Err(e)) = &regex {
        return error(
            StatusCode::BAD_REQUEST,
            format!("invalid name pattern: {e}"),
        );
    }
    let states: Vec<String> = b
        .get("states")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let ws = match scope_windows(
        &c,
        b.get("scope").and_then(Value::as_str),
        int(&b, "pid").map(|p| p as u32),
    )
    .await
    {
        Ok(w) => w,
        Err(e) => return e,
    };
    let mut matches = Vec::new();
    let mut truncated = false;
    for w in &ws {
        let query = pb::AccessibilityQuery {
            role: b
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: if mode == "exact" {
                name.to_string()
            } else {
                String::new()
            },
            name_contains: if mode == "substring" {
                name.to_string()
            } else {
                String::new()
            },
            value_contains: String::new(),
            states: states.clone(),
        };
        let r = match c
            .accessibility()
            .find(pb::FindRequest {
                window: w.r#ref.clone(),
                query: Some(query),
                max_results: if regex.is_some() { 0 } else { limit as u32 },
            })
            .await
        {
            Ok(r) => r.into_inner(),
            Err(s) => return a11y_unavailable(&s.into()),
        };
        for n in &r.nodes {
            if let Some(Ok(re)) = &regex
                && !re.is_match(&n.name)
            {
                continue;
            }
            if matches.len() >= limit {
                truncated = true;
                break;
            }
            matches.push(node_json(n, &r.snapshot_id));
        }
    }
    ok_json(json!({"matches": matches, "truncated": truncated}))
}

/// `snapshot:element` back into an element ref.
pub fn element_ref(id: &str) -> Option<pb::ElementRef> {
    let (s, e) = id.split_once(':')?;
    (!s.is_empty() && !e.is_empty()).then(|| pb::ElementRef {
        snapshot_id: s.to_string(),
        element_id: e.to_string(),
    })
}

async fn act(
    c: &SpacesdClient,
    id: &str,
    action: pb::AccessibilityAction,
    value: &str,
) -> Response<Body> {
    let Some(element) = element_ref(id) else {
        return super::toolbox::error(
            StatusCode::NOT_FOUND,
            format!("accessibility node {id} not found (ids come from a tree or a find)"),
        );
    };
    match c
        .accessibility()
        .act(pb::ActRequest {
            element: Some(element),
            action: action as i32,
            value: value.to_string(),
            custom_action: String::new(),
            delivery: pb::Delivery::Auto as i32,
        })
        .await
    {
        Ok(_) => ok_json(json!({})),
        Err(s)
            if s.code() == tonic::Code::NotFound || s.code() == tonic::Code::FailedPrecondition =>
        {
            super::toolbox::error(
                StatusCode::NOT_FOUND,
                format!("accessibility node {id} not found: {}", s.message()),
            )
        }
        Err(s) => a11y_unavailable(&s.into()),
    }
}

async fn a11y_focus(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    act(
        &c,
        b.get("id").and_then(Value::as_str).unwrap_or(""),
        pb::AccessibilityAction::Focus,
        "",
    )
    .await
}

async fn a11y_invoke(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    let action = match b
        .get("action")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        None | Some("") | Some("press") | Some("click") | Some("default") => {
            pb::AccessibilityAction::Press
        }
        Some("focus") => pb::AccessibilityAction::Focus,
        Some("increment") => pb::AccessibilityAction::Increment,
        Some("decrement") => pb::AccessibilityAction::Decrement,
        Some("show_menu") | Some("showmenu") | Some("menu") => pb::AccessibilityAction::ShowMenu,
        Some("expand") => pb::AccessibilityAction::Expand,
        Some("collapse") => pb::AccessibilityAction::Collapse,
        Some("select") => pb::AccessibilityAction::Select,
        Some("scroll_into_view") => pb::AccessibilityAction::ScrollIntoView,
        Some(o) => return error(StatusCode::BAD_REQUEST, format!("unknown action {o:?}")),
    };
    act(
        &c,
        b.get("id").and_then(Value::as_str).unwrap_or(""),
        action,
        "",
    )
    .await
}

async fn a11y_value(
    State(d): State<Arc<Daytona>>,
    Path(id): Path<String>,
    h: HeaderMap,
    b: Body,
) -> Response<Body> {
    let (_, c) = match open(&d, &h, &id).await {
        Ok(x) => x,
        Err(e) => return e,
    };
    let b = match body(b).await {
        Ok(b) => b,
        Err(e) => return e,
    };
    act(
        &c,
        b.get("id").and_then(Value::as_str).unwrap_or(""),
        pb::AccessibilityAction::SetValue,
        b.get("value").and_then(Value::as_str).unwrap_or(""),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn keys_speak_xdotool() {
        for (n, want) in [
            ("Return", pb::Key::Enter),
            ("ctrl", pb::Key::Control),
            ("super", pb::Key::Meta),
            ("BackSpace", pb::Key::Backspace),
            ("Page_Down", pb::Key::PageDown),
            ("Up", pb::Key::ArrowUp),
            ("F5", pb::Key::F5),
            ("Escape", pb::Key::Escape),
        ] {
            let k = key_input(n).unwrap_or_else(|e| panic!("{n}: {e}"));
            assert_eq!(k.key, Some(pb::key_input::Key::Named(want as i32)), "{n}");
        }
        assert_eq!(
            key_input("a").unwrap().key,
            Some(pb::key_input::Key::Named(pb::Key::A as i32))
        );
        assert_eq!(
            key_input("é").unwrap().key,
            Some(pb::key_input::Key::Character("é".into()))
        );
        assert!(key_input("nonsense-key").is_err());
    }

    #[test]
    fn hotkeys_and_modifiers_and_buttons() {
        assert_eq!(hotkey_keys("ctrl+shift+t").unwrap().len(), 3);
        assert_eq!(hotkey_keys("alt tab").unwrap().len(), 2);
        assert!(hotkey_keys("  ").is_err());
        assert_eq!(
            modifier_keys(&["ctrl".into(), "shift".into()]).unwrap(),
            vec![pb::Key::Control as i32, pb::Key::Shift as i32]
        );
        assert!(modifier_keys(&["a".into()]).is_err());
        assert_eq!(button_of(None).unwrap(), pb::MouseButton::Left);
        assert_eq!(button_of(Some("Right")).unwrap(), pb::MouseButton::Right);
        assert!(button_of(Some("laser")).is_err());
    }

    #[test]
    fn scrolling_is_signed_by_direction() {
        assert_eq!(scroll_delta("down", 3).unwrap(), (0.0, 3.0));
        assert_eq!(scroll_delta("UP", 2).unwrap(), (0.0, -2.0));
        assert_eq!(scroll_delta("left", 0).unwrap(), (-1.0, 0.0));
        assert!(scroll_delta("sideways", 1).is_err());
    }

    #[test]
    fn trees_nest_and_ids_round_trip() {
        let n = |id: &str, parent: &str, name: &str| pb::AccessibilityNode {
            element_id: id.into(),
            parent_id: parent.into(),
            name: name.into(),
            role: "button".into(),
            ..Default::default()
        };
        let tree = nest(
            &[
                n("1", "", "root"),
                n("2", "1", "a"),
                n("3", "2", "b"),
                n("4", "1", "c"),
            ],
            "snap",
        );
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0]["children"].as_array().unwrap().len(), 2);
        assert_eq!(tree[0]["children"][0]["children"][0]["id"], "snap:3");
        let r = element_ref("snap:3").unwrap();
        assert_eq!(
            (r.snapshot_id.as_str(), r.element_id.as_str()),
            ("snap", "3")
        );
        assert!(element_ref("nocolon").is_none());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn key_names_never_panic(s in "\\PC{0,20}") {
            let _ = key_input(&s);
            let _ = hotkey_keys(&s);
        }

        #[test]
        fn numeric_ids_are_stable(s in "[a-z0-9:_-]{1,30}") {
            prop_assert_eq!(numeric_id(&s), numeric_id(&s));
            prop_assert!(numeric_id(&s) >= 0);
        }

        #[test]
        fn nest_keeps_every_node(parents in proptest::collection::vec(0usize..8, 1..20)) {
            let nodes: Vec<pb::AccessibilityNode> = parents.iter().enumerate().map(|(i, p)| pb::AccessibilityNode {
                element_id: i.to_string(),
                parent_id: if i == 0 || *p >= i { String::new() } else { p.to_string() },
                ..Default::default()
            }).collect();
            fn count(v: &Value) -> usize { 1 + v["children"].as_array().map_or(0, |c| c.iter().map(count).sum()) }
            let total: usize = nest(&nodes, "s").iter().map(count).sum();
            prop_assert_eq!(total, nodes.len());
        }
    }
}
