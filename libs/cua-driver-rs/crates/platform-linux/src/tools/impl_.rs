//! Real Linux tool implementations (compiled only on Linux).

use async_trait::async_trait;
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef, ToolRegistry}};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use crate::atspi::ElementCache;
use cursor_overlay::CursorRegistry;

// ── DriverConfig + ResizeRegistry + ZoomRegistry ─────────────────────────────

#[derive(Clone)]
pub struct DriverConfig {
    pub capture_mode: String,
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self { Self { capture_mode: "som".into(), max_image_dimension: 0 } }
}

pub struct ResizeRegistry {
    ratios: std::sync::Mutex<std::collections::HashMap<u32, f64>>,
}

impl ResizeRegistry {
    pub fn new() -> Self { Self { ratios: std::sync::Mutex::new(Default::default()) } }
    pub fn set_ratio(&self, pid: u32, ratio: f64) { self.ratios.lock().unwrap().insert(pid, ratio); }
    pub fn clear_ratio(&self, pid: u32) { self.ratios.lock().unwrap().remove(&pid); }
    pub fn ratio(&self, pid: u32) -> Option<f64> { self.ratios.lock().unwrap().get(&pid).copied() }
}

/// Per-process zoom context — stores padded crop origin and resize scale from
/// the most recent `zoom` call so `click(from_zoom=true)` can translate
/// zoom-image pixel coordinates back to full-window coordinates.
#[derive(Clone, Copy, Debug)]
pub struct ZoomContext {
    pub origin_x: f64,
    pub origin_y: f64,
    /// Inverse resize scale: `cw / out_w` (1.0 = no downscale).
    pub scale_inv: f64,
}

impl ZoomContext {
    pub fn zoom_to_window(&self, px: f64, py: f64) -> (f64, f64) {
        (self.origin_x + px * self.scale_inv, self.origin_y + py * self.scale_inv)
    }
}

pub struct ZoomRegistry {
    inner: std::sync::Mutex<std::collections::HashMap<u32, ZoomContext>>,
}

impl ZoomRegistry {
    pub fn new() -> Self { Self { inner: std::sync::Mutex::new(Default::default()) } }
    pub fn set(&self, pid: u32, ctx: ZoomContext) { self.inner.lock().unwrap().insert(pid, ctx); }
    pub fn get(&self, pid: u32) -> Option<ZoomContext> { self.inner.lock().unwrap().get(&pid).copied() }
}

pub struct ToolState {
    pub element_cache: Arc<ElementCache>,
    pub cursor_registry: Arc<CursorRegistry>,
    pub resize_registry: Arc<ResizeRegistry>,
    pub zoom_registry: Arc<ZoomRegistry>,
    pub config: Arc<RwLock<DriverConfig>>,
}

impl ToolState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            element_cache: Arc::new(ElementCache::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            resize_registry: Arc::new(ResizeRegistry::new()),
            zoom_registry: Arc::new(ZoomRegistry::new()),
            config: Arc::new(RwLock::new(DriverConfig::default())),
        })
    }
}

// ── list_apps ────────────────────────────────────────────────────────────────

pub struct ListAppsTool;
static LIST_APPS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListAppsTool {
    fn def(&self) -> &ToolDef {
        LIST_APPS_DEF.get_or_init(|| ToolDef {
            name: "list_apps".into(),
            description: "List running processes via /proc filesystem.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let procs = tokio::task::spawn_blocking(|| crate::proc_fs::list_processes()).await.unwrap_or_default();
        let mut lines = vec![format!("Found {} processes:", procs.len())];
        for p in &procs {
            let cmd = if p.cmdline.is_empty() { p.name.clone() } else { p.cmdline.clone() };
            lines.push(format!("  {} (pid {})", cmd, p.pid));
        }
        let structured = json!({ "processes": procs.iter().map(|p| json!({
            "pid": p.pid, "name": p.name, "cmdline": p.cmdline
        })).collect::<Vec<_>>() });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

// ── list_windows ─────────────────────────────────────────────────────────────

pub struct ListWindowsTool;
static LIST_WINDOWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef {
        LIST_WINDOWS_DEF.get_or_init(|| ToolDef {
            name: "list_windows".into(),
            description: "List top-level X11 windows via _NET_CLIENT_LIST.".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer"},
                "on_screen_only":{"type":"boolean","description":"When true, filter to visible windows only. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let filter_pid = args.get("pid").and_then(|v| v.as_u64()).map(|v| v as u32);
        let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(filter_pid)).await.unwrap_or_default();
        let mut lines = vec![format!("Found {} windows:", windows.len())];
        for w in &windows {
            lines.push(format!("  [xid={}] pid={:?} \"{}\" {}x{}+{}+{}",
                w.xid, w.pid, w.title, w.width, w.height, w.x, w.y));
        }
        let structured = json!({ "windows": windows.iter().map(|w| json!({
            "window_id": w.xid,
            "pid": w.pid,
            "title": w.title,
            "x": w.x, "y": w.y,
            "width": w.width, "height": w.height,
        })).collect::<Vec<_>>() });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

// ── get_window_state ─────────────────────────────────────────────────────────

pub struct GetWindowStateTool {
    state: Arc<ToolState>,
}

static GWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetWindowStateTool {
    fn def(&self) -> &ToolDef {
        GWS_DEF.get_or_init(|| ToolDef {
            name: "get_window_state".into(),
            description: "Walk a running app's AT-SPI tree and return a Markdown rendering of its UI. Also captures a screenshot.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id"],"properties":{
                "pid":{"type":"integer"},
                "window_id":{"type":"integer","description":"X11 XID from list_windows."},
                "capture_mode":{"type":"string","enum":["som","vision","ax"],
                    "description":"som=tree+screenshot (default), vision=screenshot only, ax=tree only."},
                "query":{"type":"string"}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required parameter: window_id"),
        };
        let (default_mode, max_dim) = {
            let cfg = self.state.config.read().unwrap();
            (cfg.capture_mode.clone(), cfg.max_image_dimension)
        };
        let capture_mode = args.get("capture_mode").and_then(|v| v.as_str())
            .unwrap_or(&default_mode).to_owned();
        let query = args.get("query").and_then(|v| v.as_str()).map(str::to_owned);

        // "ax" = tree only; "vision" = screenshot only; "som" (default) = both.
        let do_tree = capture_mode != "vision";
        let do_shot = capture_mode != "ax";
        let state = self.state.clone();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let tree_result = if do_tree {
                Some(crate::atspi::walk_tree(pid, xid, query.as_deref()))
            } else {
                None
            };
            let screenshot = if do_shot {
                match crate::capture::screenshot_window_bytes(xid) {
                    Ok(raw) => {
                        let orig_w = crate::capture::png_dimensions_pub(&raw).map(|(w, _)| w).unwrap_or(0);
                        let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                        let original_w = if w < orig_w { Some(orig_w) } else { None };
                        Some((B64.encode(&png), w, h, original_w))
                    }
                    Err(_) => None,
                }
            } else {
                None
            };
            Ok((tree_result, screenshot))
        }).await;

        match result {
            Ok(Ok((tree_opt, shot_opt))) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": xid, "pid": pid });

                if let Some(tr) = tree_opt {
                    let count = tr.nodes.iter().filter(|n| n.element_index.is_some()).count();
                    let header = format!("window_id={xid} pid={pid} elements={count}\n\n");
                    content.push(mcp_server::protocol::Content::text(header + &tr.tree_markdown));
                    state.element_cache.update(pid, xid, &tr.nodes);
                    structured["element_count"] = json!(count);
                    structured["tree_markdown"] = json!(tr.tree_markdown);
                }

                if let Some((b64, w, h, orig_w)) = shot_opt {
                    if let Some(ow) = orig_w {
                        if w > 0 { state.resize_registry.set_ratio(pid, ow as f64 / w as f64); }
                    } else {
                        state.resize_registry.clear_ratio(pid);
                    }
                    content.push(mcp_server::protocol::Content::image_png(b64));
                    structured["screenshot_width"] = json!(w);
                    structured["screenshot_height"] = json!(h);
                }

                ToolResult { content, is_error: None, structured_content: Some(structured) }
            }
            Ok(Err(e)) => ToolResult::error(format!("Capture error: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── launch_app ───────────────────────────────────────────────────────────────

pub struct LaunchAppTool;
static LAUNCH_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        LAUNCH_DEF.get_or_init(|| ToolDef {
            name: "launch_app".into(),
            description: "Launch a Linux app in the background. Provide name (app name, launched via \
                xdg-open or direct exec), bundle_id (ignored on Linux), or urls (list of URLs to open).".into(),
            input_schema: json!({"type":"object","properties":{
                "name":{"type":"string","description":"App name or command to launch."},
                "bundle_id":{"type":"string","description":"Ignored on Linux (macOS/Windows concept)."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open via xdg-open."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let name_opt = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let urls: Vec<String> = args.get("urls").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        if name_opt.is_none() && urls.is_empty() {
            return ToolResult::error("Provide at least one of: name or urls.");
        }

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            // Open URLs via xdg-open.
            if !urls.is_empty() {
                for url in &urls {
                    std::process::Command::new("xdg-open").arg(url).spawn()?;
                }
                return Ok(format!("Opened {} URL(s) via xdg-open.", urls.len()));
            }
            // Launch by name — try direct exec, then xdg-open.
            if let Some(name) = name_opt {
                let mut parts = name.split_whitespace();
                let prog = parts.next().unwrap_or(&name);
                let rest: Vec<&str> = parts.collect();
                match std::process::Command::new(prog).args(&rest).spawn() {
                    Ok(child) => return Ok(format!("Launched '{}' with pid {}.", name, child.id())),
                    Err(_) => {
                        // Fall back to xdg-open for .desktop app names.
                        std::process::Command::new("xdg-open").arg(&name).spawn()?;
                        return Ok(format!("Opened '{}' via xdg-open.", name));
                    }
                }
            }
            unreachable!()
        }).await;

        match result {
            Ok(Ok(msg)) => ToolResult::text(msg),
            Ok(Err(e)) => ToolResult::error(format!("Failed to launch: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── shared helpers ────────────────────────────────────────────────────────────

/// Resolve an AT-SPI element's center in window-local X11 coordinates.
///
/// Returns `(xid, window_local_x, window_local_y)`.
/// Looks up element bounds via pyatspi subprocess, finds the owning window
/// (via `xid_hint` or the first window for `pid`), then converts screen-absolute
/// → window-local coords via X11 translate_coordinates.
fn resolve_element_local_coords(pid: u32, idx: usize, xid_hint: Option<u64>)
    -> anyhow::Result<(u64, f64, f64)>
{
    let (bx, by, bw, bh) = crate::atspi::get_element_bounds(pid, idx)?;
    let screen_cx = bx as f64 + bw as f64 / 2.0;
    let screen_cy = by as f64 + bh as f64 / 2.0;

    let xid = if let Some(x) = xid_hint {
        x
    } else {
        crate::x11::list_windows(Some(pid))
            .into_iter().next().map(|w| w.xid)
            .ok_or_else(|| anyhow::anyhow!("No windows for pid {pid}"))?
    };

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn.translate_coordinates(xid as u32, root, 0, 0)?.reply()?;
    let local_x = screen_cx - reply.dst_x as f64;
    let local_y = screen_cy - reply.dst_y as f64;
    Ok((xid, local_x, local_y))
}

// ── click ─────────────────────────────────────────────────────────────────────

pub struct ClickTool {
    state: Arc<ToolState>,
}
static CLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ClickTool {
    fn def(&self) -> &ToolDef {
        CLICK_DEF.get_or_init(|| ToolDef {
            name: "click".into(),
            description: "Click at (x,y) coordinates or an element_index (AT-SPI) in a window via \
                XSendEvent. No focus steal. Provide either (window_id + x/y) or (pid + element_index). \
                After a zoom call, pass from_zoom=true to auto-translate zoom-image coords back to \
                full-window space.".into(),
            input_schema: json!({
                "type":"object","required":["pid"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "x":{"type":"number"},
                    "y":{"type":"number"},
                    "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state."},
                    "button":{"type":"string","enum":["left","right","middle"]},
                    "count":{"type":"integer"},
                    "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
        let button: u8 = match args.get("button").and_then(|v| v.as_str()).unwrap_or("left") {
            "right" => 3,
            "middle" => 2,
            _ => 1,
        };

        if let Some(idx) = args.get("element_index").and_then(|v| v.as_u64()) {
            let idx = idx as usize;
            let xid_hint = args.get("window_id").and_then(|v| v.as_u64());
            // For element_index: try AT-SPI perform_action first (background-safe).
            // Always get bounds to send the overlay ClickPulse at the element center.
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(f64, f64)> {
                // Get element screen-absolute center for the overlay pulse.
                let screen_cx;
                let screen_cy;
                if let Ok((bx, by, bw, bh)) = crate::atspi::get_element_bounds(pid, idx) {
                    screen_cx = bx as f64 + bw as f64 / 2.0;
                    screen_cy = by as f64 + bh as f64 / 2.0;
                } else {
                    screen_cx = 0.0;
                    screen_cy = 0.0;
                }

                // Primary: AT-SPI doAction(0) — typically "click", no focus steal.
                if crate::atspi::perform_action(pid, idx).is_ok() {
                    return Ok((screen_cx, screen_cy));
                }

                // Fallback: XSendEvent at window-local coords.
                let (xid, lx, ly) = resolve_element_local_coords(pid, idx, xid_hint)?;
                crate::input::send_click(xid, lx as i32, ly as i32, count, button)?;
                Ok((screen_cx, screen_cy))
            }).await;
            return match result {
                Ok(Ok((x, y))) => {
                    crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x, y });
                    ToolResult::text(format!("Clicked element [{idx}] (pid {pid})."))
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI element click failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Coordinate-based path.
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mut y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(
                    format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                ),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x, y });
        // Pin overlay just above the target window for z-order sandwich.
        crate::overlay::send_command(cursor_overlay::OverlayCommand::PinAbove(xid));

        let (xi, yi) = (x as i32, y as i32);
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_click(xid, xi, yi, count, button)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Clicked at ({x:.1}, {y:.1}) × {count}.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── type_text ─────────────────────────────────────────────────────────────────

pub struct TypeTextTool;
static TYPE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        TYPE_DEF.get_or_init(|| ToolDef {
            name: "type_text".into(),
            description: "Type text to a window via XSendEvent (KeyPress/KeyRelease). No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["pid","text"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "text":{"type":"string"},
                    "element_index":{"type":"integer","description":"Element index from get_window_state (accepted for cross-platform parity)."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(),
            None => return ToolResult::error("Missing required parameter: text"),
        };
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_type_text(xid, &text)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Typed {text_len} character(s).")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── press_key ─────────────────────────────────────────────────────────────────

pub struct PressKeyTool;
static PRESS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef {
        PRESS_DEF.get_or_init(|| ToolDef {
            name: "press_key".into(),
            description: "Press a key via XSendEvent to a window. No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["pid","key"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "key":{"type":"string"},
                    "modifiers":{"type":"array","items":{"type":"string"}}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k.to_owned(),
            None => return ToolResult::error("Missing required parameter: key"),
        };
        let mods: Vec<String> = args.get("modifiers")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let key_for_task = key.clone();
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            crate::input::send_key(xid, &key_for_task, &m)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Pressed key '{key}'.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── hotkey ────────────────────────────────────────────────────────────────────

fn is_modifier(k: &str) -> bool {
    matches!(k.to_lowercase().as_str(),
        "ctrl" | "control" | "shift" | "alt" | "super" | "meta" | "cmd" | "command" | "win" | "windows")
}

pub struct HotkeyTool;
static HOTKEY_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        HOTKEY_DEF.get_or_init(|| ToolDef {
            name: "hotkey".into(),
            description: "Press a combination of keys simultaneously, e.g. [\"ctrl\",\"c\"] for Copy. \
                Sent via XSendEvent directly to the target pid; target does NOT need to be frontmost.".into(),
            input_schema: json!({
                "type":"object","required":["pid","keys"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) + one non-modifier key, e.g. [\"ctrl\",\"c\"]."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        // Parse keys array (preferred) or fall back to legacy key+modifiers.
        let (key, mods) = if let Some(arr) = args.get("keys").and_then(|v| v.as_array()) {
            let keys: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            let modifiers: Vec<String> = keys.iter().filter(|k| is_modifier(k)).cloned().collect();
            let non_mods: Vec<String> = keys.iter().filter(|k| !is_modifier(k)).cloned().collect();
            if non_mods.is_empty() {
                return ToolResult::error("keys must include at least one non-modifier key.");
            }
            (non_mods.last().unwrap().clone(), modifiers)
        } else if let Some(k) = args.get("key").and_then(|v| v.as_str()) {
            let mods: Vec<String> = args.get("modifiers").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            (k.to_owned(), mods)
        } else {
            return ToolResult::error("Provide 'keys' array (e.g. [\"ctrl\",\"c\"]) or 'key'+'modifiers' parameters.");
        };

        let key_display = format!("{}+{}", mods.join("+"), key);
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            crate::input::send_key(xid, &key, &m)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Pressed {key_display} on pid {pid}.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── set_value ─────────────────────────────────────────────────────────────────

pub struct SetValueTool;
static SV_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        SV_DEF.get_or_init(|| ToolDef {
            name: "set_value".into(),
            description: "Set value of an AT-SPI element via SetValue action.".into(),
            input_schema: json!({
                "type":"object","required":["pid","window_id","element_index","value"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "element_index":{"type":"integer"},
                    "value":{"type":"string"}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let idx = match args.get("element_index").and_then(|v| v.as_u64()) {
            Some(v) => v as usize,
            None => return ToolResult::error("Missing required parameter: element_index"),
        };
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required parameter: value"),
        };
        let value_for_task = value.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::atspi::set_value(pid, idx, &value_for_task)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Set value of element [{idx}] to '{value}'.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── scroll ────────────────────────────────────────────────────────────────────

pub struct ScrollTool;
static SCROLL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScrollTool {
    fn def(&self) -> &ToolDef {
        SCROLL_DEF.get_or_init(|| ToolDef {
            name: "scroll".into(),
            description: "Scroll the target pid's focused region via XSendEvent Button4/5. \
                direction required; by defaults to line, amount defaults to 3.".into(),
            input_schema: json!({
                "type":"object","required":["pid","direction"],"properties":{
                    "pid":{"type":"integer"},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "by":{"type":"string","enum":["line","page"]},
                    "amount":{"type":"integer","minimum":1,"maximum":50},
                    "window_id":{"type":"integer"},
                    "element_index":{"type":"integer"}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some(d) => d.to_owned(),
            None => return ToolResult::error("Missing required parameter: direction"),
        };
        let amount = args.get("amount").and_then(|v| v.as_u64())
            .unwrap_or(3).clamp(1, 50) as usize;
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        // X11 scroll buttons: 4=up, 5=down, 6=left, 7=right
        // Note: "page" scroll is still per-click on X11; send more ticks for page.
        let button: u8 = match direction.as_str() {
            "up" => 4, "left" => 6, "right" => 7, _ => 5,
        };
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_click(xid, 0, 0, amount, button)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Scrolled {direction} {amount} ticks.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── screenshot ────────────────────────────────────────────────────────────────

pub struct ScreenshotTool {
    state: Arc<ToolState>,
}
static SS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScreenshotTool {
    fn def(&self) -> &ToolDef {
        SS_DEF.get_or_init(|| ToolDef {
            name: "screenshot".into(),
            description: "Capture a screenshot via XGetImage or `import` (ImageMagick). \
                Without window_id captures the full primary display. Supports png and jpeg formats.".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "window_id":{"type":"integer"},
                    "format":{"type":"string","enum":["png","jpeg"]},
                    "quality":{"type":"integer","minimum":1,"maximum":95}
                },"additionalProperties":false
            }),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());
        let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("png").to_owned();
        let quality = args.get("quality").and_then(|v| v.as_u64()).unwrap_or(85) as u8;
        let is_jpeg = format == "jpeg";
        let max_dim = self.state.config.read().unwrap().max_image_dimension;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, u32, u32)> {
            let raw = match xid_opt {
                Some(xid) => crate::capture::screenshot_window_bytes(xid)?,
                None      => crate::capture::screenshot_display_bytes()?,
            };
            let png_bytes = crate::capture::resize_png_if_needed(&raw, max_dim)?;
            if is_jpeg {
                let jpeg = crate::capture::png_bytes_to_jpeg(&png_bytes, quality)?;
                let (w, h) = crate::capture::png_dimensions_pub(&png_bytes)?;
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                Ok((B64.encode(&jpeg), w, h))
            } else {
                let (w, h) = crate::capture::png_dimensions_pub(&png_bytes)?;
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                Ok((B64.encode(&png_bytes), w, h))
            }
        }).await;

        match result {
            Ok(Ok((b64, w, h))) => {
                let label = if is_jpeg { "JPEG" } else { "PNG" };
                let scope = if xid_opt.is_some() { "window" } else { "display" };
                let mut tr = ToolResult::text(format!("Screenshot ({scope}): {w}×{h} {label}."));
                let img = if is_jpeg {
                    mcp_server::protocol::Content::image_jpeg(b64)
                } else {
                    mcp_server::protocol::Content::image_png(b64)
                };
                tr.content.push(img);
                tr.structured_content = Some(json!({ "width": w, "height": h, "format": label }));
                tr
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── double_click ──────────────────────────────────────────────────────────────

pub struct DoubleClickTool {
    state: Arc<ToolState>,
}
static DCLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DoubleClickTool {
    fn def(&self) -> &ToolDef {
        DCLICK_DEF.get_or_init(|| ToolDef {
            name: "double_click".into(),
            description: "Double-click at (x,y) or an element_index (AT-SPI bounds) via XSendEvent. \
                No focus steal. Provide either (window_id + x/y) or (pid + element_index). \
                After a zoom call, pass from_zoom=true to auto-translate zoom-image coords.".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        if let Some(idx) = args.get("element_index").and_then(|v| v.as_u64()) {
            let idx = idx as usize;
            let xid_hint = args.get("window_id").and_then(|v| v.as_u64());
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            }).await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: lx, y: ly });
                    match tokio::task::spawn_blocking(move || crate::input::send_click(xid, lx as i32, ly as i32, 2, 1)).await {
                        Ok(Ok(())) => ToolResult::text(format!("✅ Double-clicked element [{idx}].")),
                        Ok(Err(e)) => ToolResult::error(e.to_string()),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v, None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mut y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(
                    format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                ),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }
        crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x, y });
        let (xi, yi) = (x as i32, y as i32);
        let result = tokio::task::spawn_blocking(move || crate::input::send_click(xid, xi, yi, 2, 1)).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Double-clicked at ({x:.1}, {y:.1}).")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── right_click ───────────────────────────────────────────────────────────────

pub struct RightClickTool {
    state: Arc<ToolState>,
}
static RCLICK_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for RightClickTool {
    fn def(&self) -> &ToolDef {
        RCLICK_DEF.get_or_init(|| ToolDef {
            name: "right_click".into(),
            description: "Right-click at (x,y) or an element_index (AT-SPI bounds) via XSendEvent. \
                No focus steal. Provide either (window_id + x/y) or (pid + element_index). \
                After a zoom call, pass from_zoom=true to auto-translate zoom-image coords.".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state."},
                "modifier":{"type":"array","items":{"type":"string"},"description":"Modifier keys to hold."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required parameter: pid"),
        };
        if let Some(idx) = args.get("element_index").and_then(|v| v.as_u64()) {
            let idx = idx as usize;
            let xid_hint = args.get("window_id").and_then(|v| v.as_u64());
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            }).await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: lx, y: ly });
                    match tokio::task::spawn_blocking(move || crate::input::send_click(xid, lx as i32, ly as i32, 1, 3)).await {
                        Ok(Ok(())) => ToolResult::text(format!("✅ Right-clicked element [{idx}].")),
                        Ok(Err(e)) => ToolResult::error(e.to_string()),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v, None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let mut y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(
                    format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                ),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }
        crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x, y });
        let (xi, yi) = (x as i32, y as i32);
        let result = tokio::task::spawn_blocking(move || crate::input::send_click(xid, xi, yi, 1, 3)).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Right-clicked at ({x:.1}, {y:.1}).")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── drag ─────────────────────────────────────────────────────────────────────

pub struct DragTool {
    state: Arc<ToolState>,
}
static DRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DragTool {
    fn def(&self) -> &ToolDef {
        DRAG_DEF.get_or_init(|| ToolDef {
            name: "drag".into(),
            description: "Press-drag-release gesture from (from_x, from_y) to (to_x, to_y) in \
                          window-local screenshot pixels via XSendEvent (ButtonPress + MotionNotify × steps + ButtonRelease). \
                          duration_ms (default 500), steps (default 20). No focus steal.".into(),
            input_schema: json!({"type":"object","required":["pid","from_x","from_y","to_x","to_y"],"properties":{
                "pid":{"type":"integer"},
                "window_id":{"type":"integer","description":"Target window XID. Required."},
                "from_x":{"type":"number"},
                "from_y":{"type":"number"},
                "to_x":{"type":"number"},
                "to_y":{"type":"number"},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Total drag duration. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Intermediate MotionNotify events. Default: 20."},
                "modifier":{"type":"array","items":{"type":"string"}},
                "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Default: left."},
                "from_zoom":{"type":"boolean"}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(v) => v as u32, None => return ToolResult::error("Missing required parameter: pid"),
        };
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v, None => return ToolResult::error("window_id is required on Linux."),
        };

        let coerce = |key: &str| -> Option<f64> {
            args.get(key).and_then(|v| v.as_f64())
                .or_else(|| args.get(key).and_then(|v| v.as_i64()).map(|i| i as f64))
        };
        let mut from_x = match coerce("from_x") { Some(v) => v, None => return ToolResult::error("Missing: from_x") };
        let mut from_y = match coerce("from_y") { Some(v) => v, None => return ToolResult::error("Missing: from_y") };
        let mut to_x   = match coerce("to_x")   { Some(v) => v, None => return ToolResult::error("Missing: to_x") };
        let mut to_y   = match coerce("to_y")   { Some(v) => v, None => return ToolResult::error("Missing: to_y") };

        let duration_ms = args.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(500);
        let steps       = args.get("steps").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let button_str  = args.get("button").and_then(|v| v.as_str()).unwrap_or("left");
        let button: u8  = match button_str { "right" => 3, "middle" => 2, _ => 1 };
        let from_zoom   = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);

        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(from_x, from_y);
                    let (wx2, wy2) = ctx.zoom_to_window(to_x, to_y);
                    from_x = wx; from_y = wy; to_x = wx2; to_y = wy2;
                }
                None => return ToolResult::error(format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            from_x *= ratio; from_y *= ratio;
            to_x   *= ratio; to_y   *= ratio;
        }

        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_drag(
                xid,
                from_x as i32, from_y as i32,
                to_x   as i32, to_y   as i32,
                duration_ms, steps, button,
            )
        }).await;

        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Posted drag ({button_str}) to pid {pid} \
                 from ({from_x:.0}, {from_y:.0}) → ({to_x:.0}, {to_y:.0}) \
                 in {duration_ms}ms / {steps} steps."
            )),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── get_screen_size ───────────────────────────────────────────────────────────

pub struct GetScreenSizeTool;
static GSS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef {
        GSS_DEF.get_or_init(|| ToolDef {
            name: "get_screen_size".into(),
            description: "Return the logical size of the main display in points plus its backing \
                scale factor. Agents click in points; Retina displays have scale_factor 2.0. \
                Requires no TCC permissions.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let result = tokio::task::spawn_blocking(|| {
            use x11rb::connection::Connection;
            use x11rb::rust_connection::RustConnection;
            let (conn, screen_num) = RustConnection::connect(None)?;
            let setup = conn.setup();
            let screen = &setup.roots[screen_num];
            // X11 reports pixel dimensions; scale factor on X11 is not
            // well-defined per-monitor, so report 1.0 (matches DPI-unaware
            // assumption).  Wayland/HiDPI X11 callers should query
            // `xrandr --query` for true scale.
            Ok::<(u32, u32, f64), anyhow::Error>((
                screen.width_in_pixels as u32,
                screen.height_in_pixels as u32,
                1.0,
            ))
        }).await;
        match result {
            // Matches Swift text format 1:1.
            Ok(Ok((w, h, scale))) => ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
                .with_structured(json!({ "width": w, "height": h, "scale_factor": scale })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── get_cursor_position ───────────────────────────────────────────────────────

pub struct GetCursorPositionTool;
static GCP_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetCursorPositionTool {
    fn def(&self) -> &ToolDef {
        GCP_DEF.get_or_init(|| ToolDef {
            name: "get_cursor_position".into(),
            description: "Return the current mouse cursor position in screen points (origin top-left).".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let result = tokio::task::spawn_blocking(|| {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::ConnectionExt as _;
            use x11rb::rust_connection::RustConnection;
            let (conn, screen_num) = RustConnection::connect(None)?;
            let root = conn.setup().roots[screen_num].root;
            let reply = conn.query_pointer(root)?.reply()?;
            Ok::<(i32, i32), anyhow::Error>((reply.root_x as i32, reply.root_y as i32))
        }).await;
        match result {
            // Text format matches Swift `GetCursorPositionTool` 1:1.
            Ok(Ok((x, y))) => ToolResult::text(format!("✅ Cursor at ({x}, {y})"))
                .with_structured(json!({ "x": x, "y": y })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── move_cursor ───────────────────────────────────────────────────────────────

pub struct MoveCursorTool {
    state: Arc<ToolState>,
}

static MCURSOR_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description: "Move the agent cursor overlay to (x, y). Does NOT move the real mouse cursor.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "x":{"type":"number"},"y":{"type":"number"},"cursor_id":{"type":"string"}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let x = args.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let y = args.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default");
        self.state.cursor_registry.update_position(cursor_id, x, y);
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so the
        // overlay arrow settles to the natural macOS-style pose.
        crate::overlay::send_command(cursor_overlay::OverlayCommand::MoveTo {
            x, y, end_heading_radians: std::f64::consts::FRAC_PI_4,
        });
        ToolResult::text(format!("Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1})."))
    }
}

// ── set_agent_cursor_enabled ──────────────────────────────────────────────────

pub struct SetAgentCursorEnabledTool {
    state: Arc<ToolState>,
}

static SCE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetAgentCursorEnabledTool {
    fn def(&self) -> &ToolDef {
        SCE_DEF.get_or_init(|| ToolDef {
            name: "set_agent_cursor_enabled".into(),
            description: "Show or hide the agent cursor overlay.".into(),
            input_schema: json!({"type":"object","required":["enabled"],"properties":{
                "enabled":{"type":"boolean"},"cursor_id":{"type":"string"}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let enabled = match args.get("enabled").and_then(|v| v.as_bool()) {
            Some(v) => v, None => return ToolResult::error("Missing required parameter: enabled"),
        };
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default");
        self.state.cursor_registry.set_enabled(cursor_id, enabled);
        crate::overlay::send_command(cursor_overlay::OverlayCommand::SetEnabled(enabled));
        ToolResult::text(format!("Agent cursor '{cursor_id}' {}.", if enabled { "enabled" } else { "disabled" }))
    }
}

// ── set_agent_cursor_motion ───────────────────────────────────────────────────

pub struct SetAgentCursorMotionTool {
    state: Arc<ToolState>,
}

static CURSOR_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetAgentCursorMotionTool {
    fn def(&self) -> &ToolDef {
        CURSOR_DEF.get_or_init(|| ToolDef {
            name: "set_agent_cursor_motion".into(),
            description: "Configure the visual appearance of an agent cursor instance.\n\n\
                - cursor_id: instance name (default='default')\n\
                - cursor_icon: built-in ('arrow','crosshair','hand','dot') or PNG/SVG file path\n\
                - cursor_color: hex color e.g. '#00FFFF' or CSS name\n\
                - cursor_label: short text shown near the cursor\n\
                - cursor_size: dot radius in points (default=16)\n\
                - cursor_opacity: 0.0–1.0 (default=0.85)".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "cursor_id":{"type":"string"},
                    "cursor_icon":{"type":"string"},
                    "cursor_color":{"type":"string"},
                    "cursor_label":{"type":"string"},
                    "cursor_size":{"type":"number"},
                    "cursor_opacity":{"type":"number"}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default").to_owned();
        self.state.cursor_registry.update_config(&cursor_id, |cfg| {
            if let Some(v) = args.get("cursor_icon").and_then(|v| v.as_str()) { cfg.cursor_icon = Some(v.to_owned()); }
            if let Some(v) = args.get("cursor_color").and_then(|v| v.as_str()) { cfg.cursor_color = Some(v.to_owned()); }
            if let Some(v) = args.get("cursor_label").and_then(|v| v.as_str()) { cfg.cursor_label = Some(v.to_owned()); }
            if let Some(v) = args.get("cursor_size").and_then(|v| v.as_f64()) { cfg.cursor_size = Some(v); }
            if let Some(v) = args.get("cursor_opacity").and_then(|v| v.as_f64()) { cfg.cursor_opacity = Some(v.clamp(0.0, 1.0)); }
        });
        ToolResult::text(format!("Cursor '{cursor_id}' config updated.")).with_structured(args)
    }
}

// ── get_agent_cursor_state ────────────────────────────────────────────────────

pub struct GetAgentCursorStateTool {
    state: Arc<ToolState>,
}

static GCSTATE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetAgentCursorStateTool {
    fn def(&self) -> &ToolDef {
        GCSTATE_DEF.get_or_init(|| ToolDef {
            name: "get_agent_cursor_state".into(),
            description: "Return the current state of all agent cursor instances.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let states = self.state.cursor_registry.all_states();
        let json = serde_json::to_value(&states).unwrap_or_default();
        ToolResult::text(format!("{} cursor instance(s).", states.len()))
            .with_structured(json!({ "cursors": json }))
    }
}

// ── set_agent_cursor_style ────────────────────────────────────────────────────

pub struct SetAgentCursorStyleTool {
    state: Arc<ToolState>,
}

static STYLE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetAgentCursorStyleTool {
    fn def(&self) -> &ToolDef {
        STYLE_DEF.get_or_init(|| ToolDef {
            name: "set_agent_cursor_style".into(),
            description:
                "Update the visual style of the agent cursor overlay.\n\n\
                 - gradient_colors: array of CSS hex strings (e.g. [\"#FF0000\",\"#0000FF\"]) \
                   used as the arrow fill gradient from tip to tail. Empty array reverts to \
                   the default palette colours.\n\
                 - bloom_color: hex string for the radial halo/bloom behind the cursor \
                   (e.g. \"#00FFFF\"). Empty string reverts to the default.\n\
                 - image_path: path to a PNG, JPEG, SVG, or ICO file to use as the cursor \
                   icon instead of the default gradient arrow. Empty string reverts to the \
                   procedural arrow.\n\
                 All parameters are optional; omit any you do not want to change."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "cursor_id": {
                        "type": "string",
                        "description": "Cursor instance. Default: 'default'."
                    },
                    "gradient_colors": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "CSS hex gradient stops tip→tail. [] = revert to default."
                    },
                    "bloom_color": {
                        "type": "string",
                        "description": "Hex bloom/halo colour (e.g. '#00FFFF'). '' = revert to default."
                    },
                    "image_path": {
                        "type": "string",
                        "description": "Path to PNG/JPEG/SVG/ICO cursor image. '' = revert to arrow."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default").to_owned();

        // image_path
        let image_path = args.get("image_path").and_then(|v| v.as_str());
        let shape_cmd: Option<cursor_overlay::OverlayCommand> = if let Some(path) = image_path {
            if path.is_empty() {
                Some(cursor_overlay::OverlayCommand::SetShape(None))
            } else {
                let path_owned = path.to_owned();
                match tokio::task::spawn_blocking(move || {
                    cursor_overlay::CursorShape::load(&path_owned)
                }).await {
                    Ok(Ok(shape)) => {
                        let path_for_cfg = path.to_owned();
                        self.state.cursor_registry.update_config(&cursor_id, |cfg| {
                            cfg.cursor_icon = Some(path_for_cfg);
                        });
                        Some(cursor_overlay::OverlayCommand::SetShape(Some(shape)))
                    }
                    Ok(Err(e)) => return ToolResult::error(format!("Failed to load image_path: {e}")),
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                }
            }
        } else {
            None
        };

        // gradient_colors
        let gradient_colors: Vec<[u8; 4]> = if let Some(arr) = args.get("gradient_colors").and_then(|v| v.as_array()) {
            let mut out = vec![];
            for v in arr {
                if let Some(hex) = v.as_str() {
                    match parse_hex_color(hex) {
                        Some(c) => out.push(c),
                        None => return ToolResult::error(format!("Invalid hex color: {hex}")),
                    }
                }
            }
            out
        } else {
            vec![]
        };

        // bloom_color
        let bloom_color: Option<Option<[u8; 4]>> = if let Some(hex) = args.get("bloom_color").and_then(|v| v.as_str()) {
            if hex.is_empty() {
                Some(None)
            } else {
                match parse_hex_color(hex) {
                    Some(c) => Some(Some(c)),
                    None => return ToolResult::error(format!("Invalid bloom_color: {hex}")),
                }
            }
        } else {
            None
        };

        // Dispatch to overlay
        if let Some(cmd) = shape_cmd {
            crate::overlay::send_command(cmd);
        }
        let gradient_provided = args.get("gradient_colors").is_some();
        let bloom_provided = args.get("bloom_color").is_some();
        if gradient_provided || bloom_provided {
            crate::overlay::send_command(cursor_overlay::OverlayCommand::SetGradient {
                gradient_colors,
                bloom_color: bloom_color.flatten(),
            });
        }

        let grad_str = args.get("gradient_colors")
            .and_then(|v| v.as_array())
            .map(|arr| {
                let strs: Vec<String> = arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                format!("[{}]", strs.join(", "))
            })
            .unwrap_or_else(|| "(unchanged)".into());
        let bloom_str = args.get("bloom_color")
            .and_then(|v| v.as_str())
            .map(|s| if s.is_empty() { "(reverted)".to_owned() } else { s.to_owned() })
            .unwrap_or_else(|| "(unchanged)".into());
        let img_str = image_path
            .map(|s| if s.is_empty() { "(reverted to arrow)".to_owned() } else { s.to_owned() })
            .unwrap_or_else(|| "(unchanged)".into());

        ToolResult::text(format!(
            "cursor style: gradient_colors={grad_str} bloom_color={bloom_str} image_path={img_str}"
        ))
    }
}

fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let s = hex.trim_start_matches('#');
    match s.len() {
        6 => {
            let r = u8::from_str_radix(&s[0..2], 16).ok()?;
            let g = u8::from_str_radix(&s[2..4], 16).ok()?;
            let b = u8::from_str_radix(&s[4..6], 16).ok()?;
            Some([r, g, b, 255])
        }
        3 => {
            let r = u8::from_str_radix(&s[0..1].repeat(2), 16).ok()?;
            let g = u8::from_str_radix(&s[1..2].repeat(2), 16).ok()?;
            let b = u8::from_str_radix(&s[2..3].repeat(2), 16).ok()?;
            Some([r, g, b, 255])
        }
        _ => None,
    }
}

// ── check_permissions ─────────────────────────────────────────────────────────

pub struct CheckPermissionsTool;
static PERMS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for CheckPermissionsTool {
    fn def(&self) -> &ToolDef {
        PERMS_DEF.get_or_init(|| ToolDef {
            name: "check_permissions".into(),
            description: "Check required permissions for cua-driver-rs on Linux.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        // Check X11 connectivity (required for window enumeration and input injection).
        let x11_ok = tokio::task::spawn_blocking(|| {
            x11rb::rust_connection::RustConnection::connect(None).is_ok()
        }).await.unwrap_or(false);

        // Check AT-SPI (required for accessibility tree).
        let atspi_ok = std::env::var("DBUS_SESSION_BUS_ADDRESS").is_ok()
            || std::path::Path::new("/run/user").exists();

        let status_text = format!(
            "X11 display: {}\nAT-SPI (D-Bus): {}\nXSendEvent injection: {}",
            if x11_ok { "✅ connected" } else { "❌ DISPLAY not set or X11 unavailable" },
            if atspi_ok { "✅ D-Bus session available" } else { "⚠️  D-Bus session not detected" },
            if x11_ok { "✅ available" } else { "❌ requires X11" }
        );
        ToolResult::text(status_text)
            .with_structured(json!({ "x11": x11_ok, "atspi": atspi_ok, "xsend_event": x11_ok }))
    }
}

// ── get_config ────────────────────────────────────────────────────────────────

pub struct GetConfigTool {
    state: Arc<ToolState>,
}
static GCFG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetConfigTool {
    fn def(&self) -> &ToolDef {
        GCFG_DEF.get_or_init(|| ToolDef {
            name: "get_config".into(),
            description: "Return current cua-driver-rs configuration.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        ToolResult::text("cua-driver-rs configuration")
            .with_structured(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "linux",
                "capture_mode": cfg.capture_mode,
                "max_image_dimension": cfg.max_image_dimension
            }))
    }
}

// ── set_config ────────────────────────────────────────────────────────────────

pub struct SetConfigTool {
    state: Arc<ToolState>,
}
static SCFG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetConfigTool {
    fn def(&self) -> &ToolDef {
        SCFG_DEF.get_or_init(|| ToolDef {
            name: "set_config".into(),
            description: "Update cua-driver-rs configuration. Changes take effect immediately.".into(),
            input_schema: json!({"type":"object","properties":{
                "capture_mode":{"type":"string","enum":["som","vision","ax"],"description":"Default capture mode for get_window_state."},
                "max_image_dimension":{"type":"integer","description":"Max dimension for screenshot resizing (0 = no limit)."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let mut cfg = self.state.config.write().unwrap();
        let mut parts = Vec::new();
        if let Some(mode) = args.get("capture_mode").and_then(|v| v.as_str()) {
            cfg.capture_mode = mode.to_owned();
            parts.push(format!("capture_mode={mode}"));
        }
        if let Some(dim) = args.get("max_image_dimension").and_then(|v| v.as_u64()) {
            cfg.max_image_dimension = dim as u32;
            parts.push(format!("max_image_dimension={dim}"));
        }
        let msg = if parts.is_empty() {
            "Config unchanged (no known parameters).".to_owned()
        } else {
            format!("Config updated: {}", parts.join(", "))
        };
        ToolResult::text(msg)
            .with_structured(json!({
                "capture_mode": cfg.capture_mode,
                "max_image_dimension": cfg.max_image_dimension
            }))
    }
}

// ── get_accessibility_tree ────────────────────────────────────────────────────

pub struct GetAccessibilityTreeTool;

static GAX_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetAccessibilityTreeTool {
    fn def(&self) -> &ToolDef {
        GAX_DEF.get_or_init(|| ToolDef {
            name: "get_accessibility_tree".into(),
            description: "Return a lightweight snapshot of the desktop: running processes and \
                on-screen visible X11 windows with their bounds and owner pid.\n\n\
                For the full AT-SPI subtree of a single window (with interactive element indices \
                you can click by), use get_window_state instead — this is a fast discovery read.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let (procs, windows) = tokio::task::spawn_blocking(|| {
            (crate::proc_fs::list_processes(), crate::x11::list_windows(None))
        }).await.unwrap_or_default();

        let mut lines = vec![format!(
            "{} running process(es), {} visible window(s)",
            procs.len(), windows.len()
        )];
        for p in &procs {
            let cmd = if p.cmdline.is_empty() { p.name.clone() } else { p.cmdline.clone() };
            lines.push(format!("- {} (pid {})", cmd, p.pid));
        }
        if !windows.is_empty() {
            lines.push(String::new());
            lines.push("Windows:".to_owned());
            for w in &windows {
                let title = if w.title.is_empty() { "(no title)".to_owned() }
                    else { format!("\"{}\"", w.title) };
                lines.push(format!(
                    "- pid={:?} {} [window_id: {}] {}x{}+{}+{}",
                    w.pid, title, w.xid, w.width, w.height, w.x, w.y
                ));
            }
            lines.push("→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned());
        }

        let structured = json!({
            "processes": procs.iter().map(|p| json!({"pid":p.pid,"name":p.name})).collect::<Vec<_>>(),
            "windows": windows.iter().map(|w| json!({
                "window_id": w.xid, "pid": w.pid, "title": w.title,
                "x": w.x, "y": w.y, "width": w.width, "height": w.height
            })).collect::<Vec<_>>()
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

// ── zoom ──────────────────────────────────────────────────────────────────────

pub struct ZoomTool {
    state: Arc<ToolState>,
}
static ZOOM_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ZoomTool {
    fn def(&self) -> &ToolDef {
        ZOOM_DEF.get_or_init(|| ToolDef {
            name: "zoom".into(),
            description: "Capture a cropped JPEG of a window region (x1,y1)–(x2,y2) in \
                screenshot pixels, with 20% padding. Output is at most 500 px wide.\n\n\
                After a zoom, pass from_zoom=true to click/type_text to auto-translate \
                coordinates back to full-window space.".into(),
            input_schema: json!({
                "type":"object","required":["window_id","x1","y1","x2","y2"],"properties":{
                    "window_id":{"type":"integer"},
                    "pid":{"type":"integer","description":"Target pid — required for from_zoom click/type translation."},
                    "x1":{"type":"number"},"y1":{"type":"number"},
                    "x2":{"type":"number"},"y2":{"type":"number"}
                },"additionalProperties":false
            }),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let xid = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v, None => return ToolResult::error("Missing required parameter: window_id"),
        };
        let pid = args.get("pid").and_then(|v| v.as_u64()).map(|v| v as u32);
        let x1 = match args.get("x1").and_then(|v| v.as_f64()) { Some(v) => v, None => return ToolResult::error("Missing x1") };
        let y1 = match args.get("y1").and_then(|v| v.as_f64()) { Some(v) => v, None => return ToolResult::error("Missing y1") };
        let x2 = match args.get("x2").and_then(|v| v.as_f64()) { Some(v) => v, None => return ToolResult::error("Missing x2") };
        let y2 = match args.get("y2").and_then(|v| v.as_f64()) { Some(v) => v, None => return ToolResult::error("Missing y2") };
        if x2 <= x1 || y2 <= y1 { return ToolResult::error("x2 must be > x1 and y2 must be > y1"); }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            let png = crate::capture::screenshot_window_bytes(xid)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png, x1, y1, x2, y2, 500)
        }).await;

        match result {
            Ok(Ok(crop)) => {
                if let Some(p) = pid {
                    state.zoom_registry.set(p, ZoomContext {
                        origin_x: crop.origin_x,
                        origin_y: crop.origin_y,
                        scale_inv: crop.scale_inv,
                    });
                }
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                let b64 = B64.encode(&crop.jpeg_bytes);
                let (w, h) = (crop.out_w, crop.out_h);
                use mcp_server::protocol::Content;
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!("Zoom ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG.")),
                    ],
                    is_error: None,
                    structured_content: Some(json!({ "width": w, "height": h, "format": "jpeg" })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Zoom failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── type_text_chars ───────────────────────────────────────────────────────────

pub struct TypeTextCharsTool;
static TYPE_CHARS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextCharsTool {
    fn def(&self) -> &ToolDef {
        TYPE_CHARS_DEF.get_or_init(|| ToolDef {
            name: "type_text_chars".into(),
            description: "Type text character-by-character with a configurable inter-character \
                delay (default 30 ms). Useful for apps that miss keystrokes. \
                Otherwise identical to type_text (XSendEvent, no focus steal).".into(),
            input_schema: json!({
                "type":"object","required":["pid","text"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "text":{"type":"string"},
                    "delay_ms":{"type":"integer","description":"Milliseconds between chars (default 30)."},
                    "element_index":{"type":"integer"},
                    "type_chars_only":{"type":"boolean","description":"Skip element focus, type directly. Default false."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid = args.get("pid").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(), None => return ToolResult::error("Missing required parameter: text"),
        };
        let delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(30);
        let xid_opt = args.get("window_id").and_then(|v| v.as_u64());
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_type_text_with_delay(xid, &text, delay_ms)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Typed {text_len} character(s) with {delay_ms}ms delay.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── registry ─────────────────────────────────────────────────────────────────

pub fn build_registry() -> ToolRegistry {
    let state = ToolState::new();
    let mut r = ToolRegistry::new();
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool { state: state.clone() }));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(ClickTool { state: state.clone() }));
    r.register(Box::new(DoubleClickTool { state: state.clone() }));
    r.register(Box::new(RightClickTool { state: state.clone() }));
    r.register(Box::new(DragTool { state: state.clone() }));
    r.register(Box::new(TypeTextTool));
    r.register(Box::new(PressKeyTool));
    r.register(Box::new(HotkeyTool));
    r.register(Box::new(SetValueTool));
    r.register(Box::new(ScrollTool));
    r.register(Box::new(ScreenshotTool { state: state.clone() }));
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorEnabledTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorMotionTool { state: state.clone() }));
    r.register(Box::new(GetAgentCursorStateTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorStyleTool { state: state.clone() }));
    r.register(Box::new(CheckPermissionsTool));
    r.register(Box::new(GetConfigTool { state: state.clone() }));
    r.register(Box::new(SetConfigTool { state: state.clone() }));
    r.register(Box::new(GetAccessibilityTreeTool));
    r.register(Box::new(ZoomTool { state: state.clone() }));
    r.register(Box::new(TypeTextCharsTool));
    r.register_recording_tools();
    r
}
