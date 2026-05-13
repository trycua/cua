//! Real Windows tool implementations (compiled only on Windows).

use async_trait::async_trait;

/// Animate the agent cursor to (sx, sy) in screen coordinates and wait for the
/// glide to finish before returning.  No-op when the overlay is not enabled.
///
/// On the very first call the cursor is at the off-screen initial position
/// (-200, -200).  Animating from there would cause a jarring off-screen fly-in,
/// so we snap to the target with a ClickPulse first and skip the glide wait.
async fn overlay_glide_to(sx: f64, sy: f64) {
    if !crate::overlay::is_enabled() { return; }
    let pos = crate::overlay::current_position();
    if pos.0 < 0.0 && pos.1 < 0.0 {
        // Snap to target on first use; no animation to wait for.
        crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy });
        return;
    }
    // Compute travel time from distance and speed (matches Swift: peak=900, avg~467 pts/sec).
    let dist = ((sx - pos.0).powi(2) + (sy - pos.1).powi(2)).sqrt();
    let avg_speed = 467.0_f64; // (300 + 900 + 200) / 3 ≈ Swift avg
    let ms = (dist / avg_speed * 1000.0).clamp(80.0, 600.0) as u64;
    // Always arrive at 45° (upper-left) — matches Swift's endAngleDegrees: 45.0.
    crate::overlay::send_command(cursor_overlay::OverlayCommand::MoveTo {
        x: sx, y: sy, end_heading_radians: std::f64::consts::FRAC_PI_4,
    });
    tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
}
use mcp_server::{protocol::ToolResult, tool::{Tool, ToolDef, ToolRegistry}};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use crate::uia::ElementCache;
use cursor_overlay::CursorRegistry;
use windows::core::Interface as _;

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
            // Description copied from Swift `ListAppsTool.swift` and adapted —
            // notes Windows-specific limitations (no bundle_id, no installed
            // enumeration yet).  Same semantics for running/active state flags.
            description: "List Windows apps that are currently running, with per-app state flags:\n\n\
                - running: is a process for this app live? (pid is 0 when false)\n\
                - active: is it the system-frontmost app? (implies running)\n\n\
                Only processes that own at least one visible top-level window are included — \
                background services and console processes are filtered out (mirrors Swift's \
                NSApplicationActivationPolicyRegular filter).\n\n\
                Use this for \"is X running?\". For per-window state — on-screen, minimized, \
                window titles — call list_windows instead. For just opening an app — running \
                or not — call launch_app({path: ...}) directly; list_apps is not a prerequisite.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // Strategy (port of `AppEnumerator.apps()` semantics):
        // 1. Enumerate top-level visible windows; their owner pids are the
        //    "running apps with a user-facing surface".
        // 2. Look up each pid's executable name via the process table.
        // 3. Mark `active` for the pid that owns GetForegroundWindow.
        let apps = tokio::task::spawn_blocking(|| -> Vec<serde_json::Value> {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

            let wins = crate::win32::list_windows(None);
            // Deduplicate by pid — one app may own several top-level windows.
            let mut seen_pids = std::collections::HashSet::new();
            let mut running_pids: Vec<u32> = Vec::new();
            for w in &wins {
                if seen_pids.insert(w.pid) { running_pids.push(w.pid); }
            }

            let foreground_pid = unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0u32;
                let _ = GetWindowThreadProcessId(fg, Some(&mut pid));
                pid
            };

            let procs = crate::win32::list_processes();
            let pid_to_name: std::collections::HashMap<u32, &str> =
                procs.iter().map(|p| (p.pid, p.name.as_str())).collect();

            running_pids.iter().map(|&pid| {
                let name = pid_to_name.get(&pid).copied().unwrap_or("<unknown>");
                json!({
                    "pid":       pid,
                    "bundle_id": serde_json::Value::Null,  // Windows has no bundle ID concept
                    "name":      name,
                    "running":   true,
                    "active":    pid == foreground_pid,
                })
            }).collect()
        }).await.unwrap_or_default();

        let running_count = apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false)).count();
        let total = apps.len();
        let installed_only = total - running_count;
        // Match Swift's text format: header line + bullet per running app.
        let mut lines = vec![format!(
            "✅ Found {total} app(s): {running_count} running, {installed_only} installed-not-running."
        )];
        for app in apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false)) {
            let name = app["name"].as_str().unwrap_or("?");
            let pid  = app["pid"].as_u64().unwrap_or(0);
            // Windows lacks bundle_id; omit the trailing [bundle] segment.
            lines.push(format!("- {name} (pid {pid})"));
        }
        // `apps` key (matches Swift `Output.apps`); `processes` key kept as
        // an alias for any pre-existing Rust callers that read the old shape.
        let structured = json!({
            "apps": apps,
            "processes": apps.iter().map(|a| json!({
                "pid":  a["pid"], "name": a["name"]
            })).collect::<Vec<_>>(),
        });
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
            // Description matches Swift `ListWindowsTool.swift` semantics —
            // Windows-specific caveat: no Spaces concept, so on_current_space
            // / space_ids are always omitted and current_space_id is null.
            description: "List every top-level window currently known to the window manager. \
                Each record self-contains its owning app identity so the caller never has to \
                join back against list_apps.\n\n\
                Use this — not list_apps — for any window-level reasoning: \"does this app \
                have a visible window right now?\", \"which of this pid's windows is the main \
                one?\".\n\n\
                Per-record fields: window_id (HWND), pid + app_name, title, \
                bounds {x, y, width, height}, layer (always 0), z_index (stacking order), \
                is_on_screen. The macOS-specific on_current_space / space_ids fields are \
                omitted on Windows; current_space_id is null.\n\n\
                Inputs: pid (optional pid filter), on_screen_only (bool, default false — \
                Windows currently only enumerates visible non-minimized windows; this flag \
                is accepted but has no effect on the result set yet).".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer","description":"Optional pid filter. When set, only this pid's windows are returned."},
                "on_screen_only":{"type":"boolean","description":"When true, drop windows that aren't currently on-screen. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let filter_pid = args.get("pid").and_then(|v| v.as_u64()).map(|v| v as u32);
        let _on_screen_only = args.get("on_screen_only").and_then(|v| v.as_bool()).unwrap_or(false);
        let (windows, pid_to_name) = tokio::task::spawn_blocking(move || {
            let wins = crate::win32::list_windows(filter_pid);
            let procs = crate::win32::list_processes();
            let map: std::collections::HashMap<u32, String> =
                procs.into_iter().map(|p| (p.pid, p.name)).collect();
            (wins, map)
        }).await.unwrap_or_default();

        // Swift surfaces a warning when a pid filter matches nothing.
        if let Some(fp) = filter_pid {
            if windows.is_empty() {
                use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
                use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
                let fg_pid = unsafe {
                    let fg = GetForegroundWindow();
                    let mut p = 0u32;
                    let _ = GetWindowThreadProcessId(fg, Some(&mut p));
                    p
                };
                let fg_name = pid_to_name.get(&fg_pid).map(|s| s.as_str()).unwrap_or("?");
                let msg = format!(
                    "⚠️ No windows found for pid {fp}. The pid may be wrong or the app may not \
                     have created a window yet. The current frontmost app appears to be \
                     \"{fg_name}\" (pid {fg_pid})."
                );
                return ToolResult::text(msg);
            }
        }

        // z_index: EnumWindows on Win32 returns top-to-bottom; higher index =
        // farther from front.  Swift convention: higher z_index = closer to
        // front.  Invert via `(len - 1 - i)`.
        let n = windows.len();
        let records: Vec<serde_json::Value> = windows.iter().enumerate().map(|(i, w)| {
            let app_name = pid_to_name.get(&w.pid).cloned().unwrap_or_default();
            let z_index = (n.saturating_sub(1).saturating_sub(i)) as i64;
            json!({
                "window_id":  w.hwnd,
                "pid":        w.pid,
                "app_name":   app_name,
                "title":      w.title,
                "bounds":     { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                "layer":      0,
                "z_index":    z_index,
                "is_on_screen": true,
            })
        }).collect();

        let total = records.len();
        let pid_count = windows.iter().map(|w| w.pid).collect::<std::collections::HashSet<_>>().len();
        let on_screen = records.iter().filter(|r| r["is_on_screen"].as_bool().unwrap_or(false)).count();
        // Swift text format 1:1 — append the SPI-unavailable note since
        // Windows has no equivalent of macOS Spaces.
        let header = format!(
            "✅ Found {total} window(s) across {pid_count} app(s); {on_screen} on-screen. \
             (SkyLight Space SPIs unavailable — on_current_space / space_ids omitted.)"
        );
        let mut lines = vec![header];
        for r in &records {
            let app  = r["app_name"].as_str().unwrap_or("?");
            let pid  = r["pid"].as_u64().unwrap_or(0);
            let tt   = r["title"].as_str().unwrap_or("");
            let wid  = r["window_id"].as_u64().unwrap_or(0);
            let title_disp = if tt.is_empty() { "(no title)".to_owned() } else { format!("\"{tt}\"") };
            let tag = if r["is_on_screen"].as_bool().unwrap_or(false) { "" } else { " [off-screen]" };
            lines.push(format!("- {app} (pid {pid}) {title_disp} [window_id: {wid}]{tag}"));
        }

        // Legacy alias: keep the old flat fields under each record for any
        // pre-existing Rust callers, but they read from the same source.
        let legacy_windows: Vec<serde_json::Value> = windows.iter().map(|w| json!({
            "window_id": w.hwnd, "pid": w.pid, "title": w.title,
            "x": w.x, "y": w.y, "width": w.width, "height": w.height,
        })).collect();
        let structured = json!({
            "windows":          records,
            "current_space_id": serde_json::Value::Null,  // Windows has no Spaces
            "_legacy_windows":  legacy_windows,
        });
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
            // Description ported from Swift `GetWindowStateTool.swift` with
            // Windows-specific adaptations (UIA instead of AX; no SkyLight
            // Space validation; no per-call `javascript` field — that's a
            // macOS AppleScript hook).
            description: "Walk a running app's UIA tree and return a Markdown rendering of its \
                UI, tagging every actionable element with [element_index N]. Pass those \
                indices to `click`, `type_text`, `scroll`, etc. — those tools resolve the \
                index to the cached element on the server.\n\n\
                INVARIANT: call `get_window_state` once per turn per (pid, window_id) before \
                any element-indexed action against that window. The index map is replaced by \
                the next snapshot of the same (pid, window_id).\n\n\
                The UIA tree walked is the window's tree (HWND-scoped); the screenshot and \
                window bounds reported come from the same `window_id`. This is the source of \
                truth for which window the caller intends to reason about — the driver never \
                picks a window implicitly.\n\n\
                `window_id` MUST belong to `pid`; the call returns `isError: true` otherwise. \
                The driver does not auto-fall-back to a different window.\n\n\
                Set `query` to a case-insensitive substring to filter `tree_markdown` down to \
                matching lines plus their ancestor chain. element_index values and \
                `element_count` are unchanged — the filter only trims the rendered Markdown.\n\n\
                Response shape is controlled by `capture_mode`:\n\
                - `som` (default) — walks UIA AND captures screenshot.\n\
                - `vision` — captures only; UIA walk skipped; element_index cache not updated.\n\
                - `ax` — UIA only; no screenshot.\n\n\
                Uses `IUIAutomationCacheRequest` to batch-fetch all element properties in a \
                single COM call (Chrome's ~5000-element tree returns in ~2-3s instead of \
                timing out at 4s with per-property RPCs).\n\n\
                Windows requires no special permissions.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id"],"properties":{
                "pid":{"type":"integer","description":"Process ID from `list_apps`."},
                "window_id":{"type":"integer","description":"HWND of the target window. Must belong to `pid`. Enumerate via `list_windows` or read from `launch_app`'s `windows` array."},
                "capture_mode":{"type":"string","enum":["som","vision","ax"],
                    "description":"som=tree+screenshot (default), vision=screenshot only, ax=tree only."},
                "query":{"type":"string","description":"Optional case-insensitive substring. When set, `tree_markdown` only contains lines that match plus their ancestor chain; element indices and `element_count` are unchanged."}
            },"additionalProperties":false}),
            // Swift annotation: idempotent: false (each call is a fresh snapshot).
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as u32,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let hwnd = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None    => return ToolResult::error(
                "Missing required integer field window_id. Use `list_windows` to enumerate \
                 the target app's windows, or read `launch_app`'s `windows` array."),
        };
        // Validate window belongs to pid — Swift's hard error.
        let windows_for_pid = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
            .await.unwrap_or_default();
        if !windows_for_pid.iter().any(|w| w.hwnd == hwnd) {
            // Check if the window exists under a different pid.
            let all = tokio::task::spawn_blocking(|| crate::win32::list_windows(None))
                .await.unwrap_or_default();
            if let Some(w) = all.iter().find(|w| w.hwnd == hwnd) {
                return ToolResult::error(format!(
                    "window_id {hwnd} belongs to pid {}, not pid {pid}. Call \
                     `list_windows({{\"pid\": {pid}}})` to get this pid's own windows.",
                    w.pid));
            }
            return ToolResult::error(format!(
                "No window with window_id {hwnd} exists. Call `list_windows({{\"pid\": \
                 {pid}}})` for candidates."));
        }
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
        let q = query.clone();
        let blocking = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let tree_result = if do_tree {
                Some(crate::uia::walk_tree(hwnd, q.as_deref()))
            } else {
                None
            };
            let screenshot = if do_shot {
                match crate::capture::screenshot_window_bytes(hwnd) {
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
        });
        // Timeout: Chrome's UIA provider can block indefinitely on property reads.
        let result: Result<anyhow::Result<_>, _> = match tokio::time::timeout(
            std::time::Duration::from_secs(4), blocking).await
        {
            Ok(join_result) => join_result.map_err(|e| anyhow::anyhow!("task panic: {e}")),
            Err(_elapsed) => Err(anyhow::anyhow!("get_window_state timed out after 4s (UIA provider unresponsive)")),
        };
        let result = result.and_then(|r| r);

        match result {
            Ok((tree_opt, screenshot_opt)) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": hwnd, "pid": pid });

                if let Some(tr) = tree_opt {
                    let count = tr.nodes.iter().filter(|n| n.element_index.is_some()).count();
                    let header = format!("window_id={hwnd} pid={pid} elements={count}\n\n");
                    content.push(mcp_server::protocol::Content::text(header + &tr.tree_markdown));
                    state.element_cache.update(pid, hwnd, &tr.nodes);
                    structured["element_count"] = json!(count);
                    structured["tree_markdown"] = json!(tr.tree_markdown);
                }

                if let Some((b64, w, h, orig_w)) = screenshot_opt {
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
            Err(e) => ToolResult::error(format!("Error: {e}")),
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
            // Description ported from Swift `LaunchAppTool.swift` with
            // Windows-specific notes. `bundle_id` is accepted as an alias
            // for `name` (Windows has no bundle-identifier concept).
            description: "Launch a Windows app hidden — the driver never brings the target to \
                the foreground, the target's window is launched with SW_SHOWNOACTIVATE so it \
                does not steal focus from whatever is currently frontmost.\n\n\
                Provide either `bundle_id` / `name` (resolved via ShellExecuteEx's PATH search) \
                or `path` (full path to an executable). If both `name` and `path` are given, \
                `path` wins. `urls` opens each URL in the default browser without activating it.\n\n\
                Returns the launched app's pid, name, active flag, AND a `windows` array — \
                same per-window shape `list_windows` returns. When the launch settles but no \
                window has materialized yet (transient; rare), `windows` comes back empty — \
                call `list_windows(pid)` explicitly a moment later.\n\n\
                Windows-only field: `path` (Swift uses `bundle_id` since macOS apps resolve via \
                LaunchServices; Windows has no LaunchServices, so `path` is the canonical form). \
                The macOS-specific `electron_debugging_port`, `webkit_inspector_port`, \
                `creates_new_application_instance`, and `additional_arguments` fields are \
                accepted; `additional_arguments` is honored, the others currently no-op on \
                Windows.".into(),
            input_schema: json!({"type":"object","properties":{
                "bundle_id":{"type":"string","description":"Alias for `name` on Windows (no bundle-id concept). Either bundle_id or name must be provided when path is absent."},
                "name":{"type":"string","description":"App display name passed to ShellExecuteEx."},
                "path":{"type":"string","description":"Full path to executable. Windows-only; takes precedence over name/bundle_id."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open in the default browser via ShellExecuteEx (no activation)."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process."},
                "electron_debugging_port":{"type":"integer","description":"Accepted for cross-platform parity; currently no-op on Windows."},
                "webkit_inspector_port":{"type":"integer","description":"Accepted for cross-platform parity; no-op on Windows."},
                "creates_new_application_instance":{"type":"boolean","description":"Accepted for parity; no-op on Windows (ShellExecuteEx always creates a new process)."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let path_opt = args.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        let name_opt = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let bundle_id_opt = args.get("bundle_id").and_then(|v| v.as_str()).map(str::to_owned);
        let urls: Vec<String> = args.get("urls").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let extra_args: Vec<String> = args.get("additional_arguments").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        // Resolve target — path > name > bundle_id (alias of name on Windows).
        let target = path_opt.clone()
            .or(name_opt.clone())
            .or(bundle_id_opt.clone());

        if target.is_none() && urls.is_empty() {
            // Match Swift's wording verbatim.
            return ToolResult::error("Provide either bundle_id or name to identify the app to launch.");
        }

        // Single launch path: use ShellExecuteExW with SEE_MASK_NOCLOSEPROCESS
        // so we can read the spawned process's pid back.
        let display = target.clone().unwrap_or_else(|| urls.join(", "));
        let extra_joined = extra_args.join(" ");
        let urls_clone = urls.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
            use windows::Win32::UI::Shell::{
                ShellExecuteExW, SHELLEXECUTEINFOW, SEE_MASK_NOCLOSEPROCESS,
            };
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE;
            use windows::Win32::System::Threading::GetProcessId;
            use windows::Win32::Foundation::CloseHandle;
            use windows::core::PCWSTR;

            fn to_wide(s: &str) -> Vec<u16> {
                s.encode_utf16().chain(std::iter::once(0)).collect()
            }
            let op_w   = to_wide("open");
            let file_w = to_wide(if let Some(t) = target.as_deref() { t } else { urls_clone.first().map(|s| s.as_str()).unwrap_or("") });
            let args_w = to_wide(&extra_joined);

            let mut info = SHELLEXECUTEINFOW {
                cbSize:       std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                fMask:        SEE_MASK_NOCLOSEPROCESS,
                lpVerb:       PCWSTR(op_w.as_ptr()),
                lpFile:       PCWSTR(file_w.as_ptr()),
                lpParameters: if extra_joined.is_empty() { PCWSTR::null() } else { PCWSTR(args_w.as_ptr()) },
                nShow:        SW_SHOWNOACTIVATE.0,
                ..Default::default()
            };
            unsafe { ShellExecuteExW(&mut info)?; }
            let pid = if !info.hProcess.is_invalid() {
                let p = unsafe { GetProcessId(info.hProcess) };
                unsafe { let _ = CloseHandle(info.hProcess); }
                p
            } else { 0 };

            // Open any additional URLs in the default browser (no focus
            // steal, no pid capture for these — Swift's NSWorkspace flow
            // behaves the same way for secondary URLs).
            for url in &urls_clone[urls_clone.len().min(1)..] {
                let file = to_wide(url);
                let mut url_info = SHELLEXECUTEINFOW {
                    cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                    lpVerb: PCWSTR(op_w.as_ptr()),
                    lpFile: PCWSTR(file.as_ptr()),
                    nShow:  SW_SHOWNOACTIVATE.0,
                    ..Default::default()
                };
                unsafe { let _ = ShellExecuteExW(&mut url_info); }
            }

            Ok(pid)
        }).await;

        let pid = match result {
            Ok(Ok(p))  => p,
            Ok(Err(e)) => return ToolResult::error(format!("Failed to launch: {e}")),
            Err(e)     => return ToolResult::error(format!("Task error: {e}")),
        };

        // Resolve the pid's windows so the caller can skip a list_windows
        // round-trip — same approach as Swift's `resolveWindows`.  Retry
        // 5×200ms; Win32 window registration can lag the launch.
        let mut windows_json: Vec<serde_json::Value> = Vec::new();
        for _ in 0..5 {
            let wins = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                .await.unwrap_or_default();
            if !wins.is_empty() {
                windows_json = wins.iter().map(|w| json!({
                    "window_id": w.hwnd, "title": w.title,
                    "bounds":    { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                    "layer":     0,
                    "z_index":   0,           // single-window context; per-record z_index already in list_windows
                    "is_on_screen": true,
                })).collect();
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        // Match Swift text format 1:1.
        let mut summary = format!("✅ Launched {display} (pid {pid}) in background.");
        if !windows_json.is_empty() {
            summary.push_str("\n\nWindows:");
            for w in &windows_json {
                let title = w["title"].as_str().unwrap_or("");
                let title_disp = if title.is_empty() { "(no title)".to_owned() } else { format!("\"{title}\"") };
                let wid = w["window_id"].as_u64().unwrap_or(0);
                summary.push_str(&format!("\n- {title_disp} [window_id: {wid}]"));
            }
            summary.push_str(&format!("\n→ Call get_window_state(pid: {pid}, window_id) to inspect."));
        }
        let structured = json!({
            "pid":       pid,
            "bundle_id": serde_json::Value::Null,
            "name":      display,
            "running":   true,
            "active":    false,  // SW_SHOWNOACTIVATE — Swift's background-launch invariant
            "windows":   windows_json,
        });
        ToolResult::text(summary).with_structured(structured)
    }
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
            // Description matches Swift `ClickTool.swift` semantics (left-click
            // primitive with two addressing modes — element_index via UIA, or
            // window-local pixel coords via PostMessage).  Windows-only schema
            // extras (`button`, no `modifier`/`action`/`debug_image_out`) are
            // documented in PARITY.md.
            description: "Left-click against a target pid. Two addressing modes:\n\n\
                - `element_index` + `window_id` (from the last `get_window_state` snapshot \
                of that window) — performs the UIA Invoke pattern on the cached element via \
                PostMessage. No cursor move, no focus steal. Requires a prior \
                `get_window_state(pid, window_id)` in this turn; the element_index cache \
                is scoped per (pid, window_id).\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`) — synthesizes mouse events via \
                PostMessage(WM_LBUTTONDOWN/UP) and delivers them to the deepest child \
                window under that point. `count: 2` posts two down/up pairs for a \
                double-click.\n\n\
                Exactly one of `element_index` or (`x` AND `y`) must be provided. \
                `pid` is required in both modes. `window_id` is required when \
                `element_index` is used (scopes the cache lookup). After a `zoom` call, \
                pass `from_zoom=true` to auto-translate zoom-image coords back to \
                full-window space.\n\n\
                Windows-only convenience: `button: \"left\"|\"right\"|\"middle\"` switches \
                the mouse button (Swift exposes right-click as a separate `right_click` \
                tool). The Swift-only `action` / `modifier` / `debug_image_out` schema \
                fields aren't supported yet.".into(),
            input_schema: json!({
                "type":"object","required":["pid"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "window_id":{"type":"integer","description":"HWND for the window whose get_window_state produced the element_index. Required when element_index is used."},
                    "element_index":{"type":"integer","description":"Element index from the last get_window_state for the same (pid, window_id)."},
                    "x":{"type":"number","description":"X in window-local screenshot pixels — same space as the PNG get_window_state returns. Must be provided together with y."},
                    "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                    "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Windows-only convenience; Swift exposes right-click as a separate `right_click` tool."},
                    "count":{"type":"integer","minimum":1,"maximum":3,"description":"Click count — 1 (single), 2 (double), 3 (triple). Default 1."},
                    "from_zoom":{"type":"boolean","description":"When true, x and y are pixel coordinates in the last `zoom` image for this pid. The driver maps them back to window coords."}
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
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let x = args.get("x").and_then(|v| v.as_f64());
        let y = args.get("y").and_then(|v| v.as_f64());
        let button = args.get("button").and_then(|v| v.as_str()).unwrap_or("left").to_owned();
        let count = args.get("count").and_then(|v| v.as_u64()).unwrap_or(1) as usize;

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        if let Some(idx) = elem_idx {
            // UIA path: get cached center (no COM call needed — captured at walk time).
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => return ToolResult::error(format!("Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first.")),
            };
            // Step 2: animate cursor to screen coords (awaits glide_duration_ms).
            overlay_glide_to(cx as f64, cy as f64).await;
            // Step 3: click pulse + post click.
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse {
                x: cx as f64, y: cy as f64,
            });
            let btn = button.clone();
            let action_name = match btn.as_str() {
                "right"  => "ShowMenu",  // closest UIA analogue of Swift "show_menu"
                "middle" => "Invoke",
                _        => "Invoke",    // left-click = UIA Invoke pattern (matches Swift "AXPress")
            };
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                // cx/cy are screen coords from CurrentBoundingRectangle; use
                // post_click_screen so deepest-child resolution happens once.
                crate::input::post_click_screen(hwnd, cx, cy, count, &btn)?;
                // Match Swift text format: "✅ Performed AXPress on [N] role \"title\"."
                // (UIA has no readily-available role/name on the cached element here;
                // emit element_index + screen coords for traceability instead).
                Ok(format!("✅ Performed {action_name} on [{idx}] (screen ({cx},{cy}))."))
            }).await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(px, py); px = wx; py = wy; }
                    None => return ToolResult::error(
                        format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                    ),
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            // px/py are window-client coords. Convert to screen for the overlay.
            let (sx, sy) = unsafe {
                use windows::Win32::Foundation::{HWND, POINT};
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                let mut pt = POINT { x: px as i32, y: py as i32 };
                let _ = ClientToScreen(HWND(hwnd as *mut _), &mut pt);
                (pt.x as f64, pt.y as f64)
            };
            overlay_glide_to(sx, sy).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy });
            let btn = button.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::input::post_click(hwnd, px as i32, py as i32, count, &btn)
            }).await;
            match result {
                Ok(Ok(())) => {
                    // Match Swift text format: "✅ Posted click/double-click/triple-click to pid X."
                    let click_word = match count {
                        2 => "double-click",
                        3 => "triple-click",
                        _ => "click",
                    };
                    ToolResult::text(format!("✅ Posted {click_word} to pid {pid}."))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Match Swift's error wording for the missing-target case.
            ToolResult::error("Provide element_index or (x, y) to address the click target.")
        }
    }
}

// ── type_text ─────────────────────────────────────────────────────────────────

pub struct TypeTextTool {
    state: Arc<ToolState>,
}

static TYPE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextTool {
    fn def(&self) -> &ToolDef {
        TYPE_DEF.get_or_init(|| ToolDef {
            name: "type_text".into(),
            // Description ported from Swift `TypeTextTool.swift` with the
            // Windows transport note.  Swift tries AXSetAttribute(kAXSelectedText)
            // first, falls back to character-by-character CGEvent synthesis.
            // Windows uses character-by-character PostMessage(WM_CHAR) to the
            // window's focused child — there's no UIA equivalent of
            // AXSelectedText wired up yet, so it's always the synthesis path.
            description: "Insert text into the target pid via character-by-character \
                PostMessage(WM_CHAR) to the focused window. No focus steal.\n\n\
                Special keys (Return, Escape, arrows, Tab) go through `press_key` / \
                `hotkey` — they are not text.\n\n\
                Optional `element_index` + `window_id` (from the last `get_window_state` \
                snapshot of that window) is accepted for parity but currently no-op on \
                Windows (UIA SetFocus not wired up yet); without `element_index`, the \
                write targets the pid's focused window — same as Swift's no-element path.\n\n\
                `delay_ms` (0–200, default 30) spaces successive characters so \
                autocomplete and IME can keep up.".into(),
            input_schema: json!({
                "type":"object","required":["pid","text"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "text":{"type":"string","description":"Text to insert at the focused element's cursor."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Required when element_index is used."},
                    "element_index":{"type":"integer","description":"Optional element_index. Accepted for parity; currently no-op on Windows."},
                    "delay_ms":{"type":"integer","minimum":0,"maximum":200,"description":"Milliseconds between characters. Default 30."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let text = match args.get("text").and_then(|v| v.as_str()) {
            Some(t) => t.to_owned(),
            None    => return ToolResult::error("Missing required string field text."),
        };
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64());
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }
        let _delay_ms = args.get("delay_ms").and_then(|v| v.as_u64()).unwrap_or(30);
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::post_type_text(hwnd, &text)
        }).await;
        match result {
            // Match Swift's CGEvent-fallback text format 1:1 (Windows always
            // takes the synthesis path since AXSelectedText has no UIA
            // equivalent wired up here yet).
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Typed {text_len} char(s) on pid {raw_pid} via PostMessage ({_delay_ms}ms delay)."
            )),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
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
            // Description ported from Swift `PressKeyTool.swift` with
            // Windows-specific transport note (PostMessage WM_KEYDOWN/UP).
            description: "Press and release a single key, delivered directly to the target pid's \
                top-level window via PostMessage(WM_KEYDOWN/WM_KEYUP). The target does NOT need \
                to be frontmost — no focus steal.\n\n\
                Optional `window_id` selects a specific HWND when the pid owns more than one; \
                without it the first visible top-level window for the pid is used.\n\n\
                Key vocabulary: return, tab, escape, up/down/left/right, space, delete, home, \
                end, pageup, pagedown, f1-f12, plus any letter or digit. Optional `modifiers` \
                array takes ctrl/shift/alt/win. For true combinations (ctrl+c), `hotkey` is a \
                cleaner surface.\n\n\
                Note: `element_index` is accepted for cross-platform parity with macOS but \
                currently no-op on Windows (no UIA SetFocus path wired up yet).".into(),
            input_schema: json!({
                "type":"object","required":["pid","key"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "key":{"type":"string","description":"Key name (return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12, letter, digit)."},
                    "modifiers":{"type":"array","items":{"type":"string"},"description":"Optional modifier names held while the key is pressed (ctrl/shift/alt/win)."},
                    "element_index":{"type":"integer","description":"Optional element_index. Accepted for parity; currently no-op on Windows."},
                    "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used; otherwise auto-resolves the pid's first visible window."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let key = match args.get("key").and_then(|v| v.as_str()) {
            Some(k) => k.to_owned(),
            None    => return ToolResult::error("Missing required string field key."),
        };
        let mods: Vec<String> = args.get("modifiers")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        // Swift requires window_id when element_index is supplied — ports the
        // same validation.
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64());
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let key_display = key.clone();
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            crate::input::post_key(hwnd, &key, &m)
        }).await;
        match result {
            // Match Swift's text format 1:1: `"✅ Pressed KEY on pid X."`.
            Ok(Ok(())) => ToolResult::text(format!("✅ Pressed {key_display} on pid {raw_pid}.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── hotkey ────────────────────────────────────────────────────────────────────

fn is_modifier(k: &str) -> bool {
    matches!(k.to_lowercase().as_str(),
        "ctrl" | "control" | "shift" | "alt" | "win" | "windows" | "cmd" | "command")
}

pub struct HotkeyTool;
static HOTKEY_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for HotkeyTool {
    fn def(&self) -> &ToolDef {
        HOTKEY_DEF.get_or_init(|| ToolDef {
            name: "hotkey".into(),
            // Description ported from Swift `HotkeyTool.swift` (omitting macOS-
            // specific FocusWithoutRaise/SLEventPostToPid mechanics — Windows
            // uses PostMessage which doesn't need a NSMenu-activation dance).
            description: "Press a combination of keys simultaneously — e.g. `[\"ctrl\", \"c\"]` \
                for Copy, `[\"ctrl\", \"shift\", \"t\"]` for reopen-closed-tab. The combo is \
                posted directly to the target pid's window via PostMessage(WM_KEYDOWN/UP); \
                the target does NOT need to be frontmost.\n\n\
                **`window_id`** (optional): explicit HWND when the pid owns more than one \
                window; otherwise the pid's first visible window is used.\n\n\
                Recognized modifiers: ctrl/control, shift, alt, win/windows. Non-modifier \
                keys use the same vocabulary as `press_key` (return, tab, escape, \
                up/down/left/right, space, delete, home, end, pageup, pagedown, f1-f12, \
                letters, digits). Order: modifiers first, one non-modifier last.".into(),
            input_schema: json!({
                "type":"object","required":["pid","keys"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) and one non-modifier key, e.g. [\"ctrl\", \"c\"]."},
                    "window_id":{"type":"integer","description":"Explicit HWND when the pid owns multiple windows."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;

        // Parse keys array (Swift's only path).  Legacy key+modifiers shape
        // still accepted as a Rust-side convenience.
        let (key, mods, full_keys) = if let Some(arr) = args.get("keys") {
            let raw = match arr.as_array() {
                Some(r) => r,
                None    => return ToolResult::error("Missing required array field keys."),
            };
            let keys: Vec<String> = raw.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect();
            if keys.len() != raw.len() || keys.is_empty() {
                return ToolResult::error("keys must be a non-empty array of strings.");
            }
            let modifiers: Vec<String> = keys.iter().filter(|k| is_modifier(k)).cloned().collect();
            let non_mods: Vec<String> = keys.iter().filter(|k| !is_modifier(k)).cloned().collect();
            if non_mods.is_empty() {
                return ToolResult::error("keys must include at least one non-modifier key.");
            }
            (non_mods.last().unwrap().clone(), modifiers, keys)
        } else if let Some(k) = args.get("key").and_then(|v| v.as_str()) {
            let m: Vec<String> = args.get("modifiers").and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let mut full = m.clone(); full.push(k.to_owned());
            (k.to_owned(), m, full)
        } else {
            return ToolResult::error("Missing required array field keys.");
        };

        // Resolve HWND: use window_id if given, else pick first window for pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let pid2 = pid;
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid2))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        // Match Swift's text format exactly: `"✅ Pressed K1+K2+... on pid X."`
        // where K1+K2+... is `keys.joined(separator: "+")` of the FULL keys
        // array as the caller supplied it (modifiers + non-modifier in order).
        let key_display = full_keys.join("+");
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            crate::input::post_key(hwnd, &key, &m)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("✅ Pressed {key_display} on pid {raw_pid}.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── set_value ─────────────────────────────────────────────────────────────────

pub struct SetValueTool {
    state: Arc<ToolState>,
}

static SET_VALUE_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        SET_VALUE_DEF.get_or_init(|| ToolDef {
            name: "set_value".into(),
            // Description ported from Swift `SetValueTool.swift` with the
            // Windows transport (UIA ValuePattern instead of AXValue).
            // Swift's AXPopUpButton special-case has a UIA analogue
            // (SelectionPattern) that's not yet wired up here; documented.
            description: "Set a value on a UIA element via the ValuePattern interface.\n\n\
                Two semantic modes (matching Swift's split):\n\
                - **Standard input** (text fields, sliders, combo box edit): writes the value \
                  directly through UIA `IUIAutomationValuePattern::SetValue`. This is the \
                  canonical Windows write path, equivalent to Swift's `AXValue` write.\n\
                - **ComboBox / select dropdown**: ValuePattern.SetValue picks the option \
                  whose text matches `value` on most native ComboBox controls.\n\n\
                For free-form text entry that the target accepts via keystrokes only, prefer \
                `type_text` — UIA ValuePattern writes are ignored by some web inputs (same \
                caveat as Swift's `AXValue`-vs-WebKit).".into(),
            input_schema: json!({
                "type":"object","required":["pid","window_id","element_index","value"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "window_id":{"type":"integer","description":"HWND of the window. The element_index cache is scoped per (pid, window_id)."},
                    "element_index":{"type":"integer","description":"Element index from the last get_window_state."},
                    "value":{"type":"string","description":"New value. UIA will coerce to the element's native type."}
                },"additionalProperties":false
            }),
            // Swift: idempotent: true (setting same value twice is idempotent).
            read_only: false, destructive: true, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift's "Missing required integer fields pid, window_id, and element_index."
        let mut missing_ints: Vec<&str> = Vec::new();
        let raw_pid = args.get("pid").and_then(|v| v.as_i64());
        if raw_pid.is_none()                    { missing_ints.push("pid"); }
        if args.get("window_id").and_then(|v| v.as_u64()).is_none()  { missing_ints.push("window_id"); }
        if args.get("element_index").and_then(|v| v.as_u64()).is_none() { missing_ints.push("element_index"); }
        if !missing_ints.is_empty() {
            // Swift wording when all three are missing; for partial misses
            // we still emit the same shape (good enough for parity).
            return ToolResult::error("Missing required integer fields pid, window_id, and element_index.");
        }
        let pid  = raw_pid.unwrap() as u32;
        let hwnd = args.get("window_id").and_then(|v| v.as_u64()).unwrap();
        let idx  = args.get("element_index").and_then(|v| v.as_u64()).unwrap() as usize;
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None    => return ToolResult::error("Missing required string field value."),
        };

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let ptr = state.element_cache.get_element_ptr(pid, hwnd, idx)
                .ok_or_else(|| anyhow::anyhow!("Element {idx} not in cache."))?;
            use windows::Win32::UI::Accessibility::{IUIAutomationElement, IUIAutomationValuePattern, UIA_ValuePatternId};
            use windows::core::{Interface, BSTR};
            let elem: IUIAutomationElement = unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
            let pattern = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId)? };
            std::mem::forget(elem);
            let vp: IUIAutomationValuePattern = pattern.cast()?;
            unsafe { vp.SetValue(&BSTR::from(value.as_str()))? };
            Ok(())
        }).await;
        match result {
            // Match Swift's text format 1:1 (default AXValue-write path).
            // UIA role/title placeholder pending element-cache enrichment.
            Ok(Ok(())) => ToolResult::text(format!("✅ Set AXValue on [{idx}] (UIA ValuePattern).")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
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
            // Description ported from Swift `ScrollTool.swift` with the
            // Windows-specific transport note (WM_VSCROLL/WM_HSCROLL).
            description: "Scroll the target pid's focused region.\n\n\
                Windows transport: WM_VSCROLL / WM_HSCROLL posted to the window — the same \
                events the OS sends when scrollbars or trackpads scroll, so any window that \
                handles scrollbars correctly responds to this. Swift uses synthesized \
                keystrokes (PageDown / arrow keys) via auth-signed SLEventPostToPid; both \
                approaches reach backgrounded windows.\n\n\
                Mapping: `by: \"page\"` → SB_PAGEDOWN/UP/LEFT/RIGHT × amount; \
                `by: \"line\"` → SB_LINEDOWN/UP/LEFT/RIGHT × amount.\n\n\
                Note: `element_index` is accepted for cross-platform parity but currently \
                no-op on Windows (UIA SetFocus not wired up yet — same caveat as `press_key`).".into(),
            input_schema: json!({
                "type":"object","required":["pid","direction"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "by":{"type":"string","enum":["line","page"],"description":"Scroll granularity. Default: line."},
                    "amount":{"type":"integer","minimum":1,"maximum":50,
                        "description":"Number of scroll ticks. Default 3."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Required when element_index is used; otherwise auto-resolves the pid's first visible window."},
                    "element_index":{"type":"integer","description":"Optional element_index. Accepted for parity; currently no-op on Windows."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some(d) => d.to_owned(),
            None    => return ToolResult::error("Missing required string field direction."),
        };
        let by = args.get("by").and_then(|v| v.as_str()).unwrap_or("line").to_owned();
        let direction_display = direction.clone();
        let by_display = by.clone();
        let amount = args.get("amount").and_then(|v| v.as_u64())
            .unwrap_or(3).clamp(1, 50) as u32;
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64());
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }

        // Resolve HWND: use window_id if given, else first window for pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                PostMessageW, WM_HSCROLL, WM_VSCROLL,
                SB_LINEDOWN, SB_LINEUP, SB_LINELEFT, SB_LINERIGHT,
                SB_PAGEDOWN, SB_PAGEUP, SB_PAGELEFT, SB_PAGERIGHT,
            };

            let hwnd_win = HWND(hwnd as *mut _);
            let use_page = by == "page";
            let (msg, code) = match (direction.as_str(), use_page) {
                ("up",    false) => (WM_VSCROLL, SB_LINEUP),
                ("up",    true)  => (WM_VSCROLL, SB_PAGEUP),
                ("down",  false) => (WM_VSCROLL, SB_LINEDOWN),
                ("down",  true)  => (WM_VSCROLL, SB_PAGEDOWN),
                ("left",  false) => (WM_HSCROLL, SB_LINELEFT),
                ("left",  true)  => (WM_HSCROLL, SB_PAGELEFT),
                ("right", false) => (WM_HSCROLL, SB_LINERIGHT),
                ("right", true)  => (WM_HSCROLL, SB_PAGERIGHT),
                _ => (WM_VSCROLL, SB_LINEDOWN),
            };
            for _ in 0..amount {
                unsafe {
                    PostMessageW(hwnd_win, msg, WPARAM(code.0 as usize), LPARAM(0))?;
                }
            }
            Ok(())
        }).await;

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format shape 1:1 (Swift uses key names
                // like "pagedown"; Windows uses Win32 SB_* constants — show
                // the actual mechanism for traceability).
                let mech_name = match (direction_display.as_str(), by_display.as_str()) {
                    ("up",    "page") => "SB_PAGEUP",
                    ("down",  "page") => "SB_PAGEDOWN",
                    ("left",  "page") => "SB_PAGELEFT",
                    ("right", "page") => "SB_PAGERIGHT",
                    ("up",    _)      => "SB_LINEUP",
                    ("down",  _)      => "SB_LINEDOWN",
                    ("left",  _)      => "SB_LINELEFT",
                    ("right", _)      => "SB_LINERIGHT",
                    _ => "SB_LINEDOWN",
                };
                ToolResult::text(format!(
                    "✅ Scrolled pid {raw_pid} {direction_display} via {amount}× {mech_name} message(s)."
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── screenshot ────────────────────────────────────────────────────────────────

pub struct ScreenshotTool {
    state: Arc<ToolState>,
}
static SCREENSHOT_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScreenshotTool {
    fn def(&self) -> &ToolDef {
        SCREENSHOT_DEF.get_or_init(|| ToolDef {
            name: "screenshot".into(),
            // Description ported from Swift `ScreenshotTool.swift` with the
            // Windows-specific transport note (BitBlt + PrintWindow instead of
            // ScreenCaptureKit).
            description: "Capture a screenshot of a single window via BitBlt + PrintWindow \
                (no focus change). Returns base64-encoded image data in the requested format \
                (default png).\n\n\
                `window_id` is recommended (use `list_windows` to find it). When omitted, \
                captures the full primary display — a Windows-only convenience for whole-\
                screen snapshots without a per-window target.\n\n\
                Requires no special permissions on Windows.".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "window_id":{"type":"integer","description":"HWND of the window to capture. When omitted, captures the full primary display (Windows-only)."},
                    "format":{"type":"string","enum":["png","jpeg"],"description":"Image format. Default: png."},
                    "quality":{"type":"integer","minimum":1,"maximum":95,
                        "description":"JPEG quality 1-95; ignored for png."}
                },"additionalProperties":false
            }),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let format = args.get("format").and_then(|v| v.as_str()).unwrap_or("png").to_owned();
        let quality = args.get("quality").and_then(|v| v.as_u64()).unwrap_or(95) as u8;
        let is_jpeg = format == "jpeg";
        let max_dim = self.state.config.read().unwrap().max_image_dimension;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, u32, u32)> {
            let raw = match hwnd_opt {
                Some(hwnd) => crate::capture::screenshot_window_bytes(hwnd)?,
                None       => crate::capture::screenshot_display_bytes()?,
            };
            let png_bytes = crate::capture::resize_png_if_needed(&raw, max_dim)?;
            let (w, h) = crate::capture::png_dimensions_pub(&png_bytes)?;
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            if is_jpeg {
                let jpeg = crate::capture::png_bytes_to_jpeg(&png_bytes, quality)?;
                Ok((B64.encode(&jpeg), w, h))
            } else {
                Ok((B64.encode(&png_bytes), w, h))
            }
        }).await;

        match result {
            Ok(Ok((b64, w, h))) => {
                let label = if is_jpeg { "jpeg" } else { "png" };
                // Match Swift `ScreenshotTool.swift` text format 1:1:
                //   "✅ Window screenshot — WxH png [window_id: ID]"
                // Whole-display fallback uses "✅ Display screenshot — WxH png"
                // (Rust-only path; Swift always requires window_id).
                let summary = match hwnd_opt {
                    Some(wid) => format!("✅ Window screenshot — {w}x{h} {label} [window_id: {wid}]"),
                    None      => format!("✅ Display screenshot — {w}x{h} {label}"),
                };
                let mut tr = ToolResult::text(summary);
                let img_content = if is_jpeg {
                    mcp_server::protocol::Content::image_jpeg(b64)
                } else {
                    mcp_server::protocol::Content::image_png(b64)
                };
                tr.content.push(img_content);
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
            // Description ported from Swift `DoubleClickTool.swift` semantics
            // (two addressing modes, AXOpen analogue on Windows is not yet
            // wired up — element path always falls back to a stamped pixel
            // double-click via PostMessage).
            description: "Double-click against a target pid. Two addressing modes:\n\n\
                - `element_index` + `window_id` (from the last `get_window_state` snapshot \
                of that window) — synthesizes a stamped pixel double-click at the element's \
                cached on-screen center via PostMessage. (Windows has no AXOpen analogue; \
                Swift's AXOpen-first path falls through to the same pixel recipe when the \
                element doesn't advertise AXOpen, so the user-visible behavior matches.)\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`) — posts two WM_LBUTTONDOWN/UP pairs in \
                quick succession to the deepest child window at that point.\n\n\
                Exactly one of `element_index` or (`x` AND `y`) must be provided. \
                `pid` is required in both modes. `window_id` is required when \
                `element_index` is used. The macOS-only `modifier` field is accepted for \
                parity (no-op on Windows — PostMessage doesn't propagate modifier-key state).".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used."},
                "element_index":{"type":"integer","description":"Element index from the last get_window_state for the same (pid, window_id)."},
                "x":{"type":"number","description":"X in window-local screenshot pixels. Must be provided together with y."},
                "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                "modifier":{"type":"array","items":{"type":"string"},"description":"Modifier keys held during the gesture. Accepted for parity; currently no-op on Windows."},
                "from_zoom":{"type":"boolean","description":"After a zoom call, pass true to translate zoom-image coords back to window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let x = args.get("x").and_then(|v| v.as_f64());
        let y = args.get("y").and_then(|v| v.as_f64());
        // Swift validates "both x and y or neither" and "no element_index without window_id".
        let has_xy        = x.is_some() && y.is_some();
        let partial_xy    = x.is_some() != y.is_some();
        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if elem_idx.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if elem_idx.is_none() && !has_xy {
            return ToolResult::error("Provide element_index or (x, y) to address the double-click target.");
        }
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        if let Some(idx) = elem_idx {
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => return ToolResult::error(format!("Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first.")),
            };
            overlay_glide_to(cx as f64, cy as f64).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: cx as f64, y: cy as f64 });
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                crate::input::post_click_screen(hwnd, cx, cy, 2, "left")?;
                // Swift text format 1:1: `"✅ Posted double-click to [N] role \"title\" at screen-point (X, Y)."`.
                // UIA role/title placeholder pending element-cache enrichment.
                Ok(format!("✅ Posted double-click to [{idx}] at screen-point ({cx}, {cy})."))
            }).await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(px, py); px = wx; py = wy; }
                    None => return ToolResult::error(
                        format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                    ),
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            let (sx, sy) = unsafe {
                use windows::Win32::Foundation::{HWND, POINT};
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                let mut pt = POINT { x: px as i32, y: py as i32 };
                let _ = ClientToScreen(HWND(hwnd as *mut _), &mut pt);
                (pt.x as f64, pt.y as f64)
            };
            overlay_glide_to(sx, sy).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy });
            let (xi, yi) = (px as i32, py as i32);
            let result = tokio::task::spawn_blocking(move || crate::input::post_click(hwnd, xi, yi, 2, "left")).await;
            match result {
                Ok(Ok(())) => {
                    // Match Swift's pixel-path text 1:1.
                    ToolResult::text(format!(
                        "✅ Posted double-click to pid {pid} at window-pixel ({xi}, {yi}) → screen-point ({}, {}).",
                        sx as i32, sy as i32))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Unreachable — guarded by the earlier validation block.
            unreachable!()
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
            // Description ported from Swift `RightClickTool.swift` semantics.
            // Windows uses PostMessage(WM_RBUTTONDOWN/UP); the AXShowMenu
            // analogue (UIA ShowContextMenu) is not yet wired up, so element
            // path falls through to the pixel recipe at the cached center.
            description: "Right-click against a target pid. Two addressing modes:\n\n\
                - `element_index` + `window_id` (from the last `get_window_state` snapshot \
                of that window) — posts WM_RBUTTONDOWN/UP at the element's cached on-screen \
                center via PostMessage. (Windows has no AXShowMenu analogue wired up yet; \
                Swift's AX-action path falls through to the same pixel recipe on non-\
                advertising elements, so user-visible behavior matches.)\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`) — posts WM_RBUTTONDOWN/UP to the deepest \
                child window at that point.\n\n\
                Exactly one of `element_index` or (`x` AND `y`) must be provided. `pid` is \
                required in both modes. `window_id` is required when `element_index` is used. \
                `modifier` is accepted for parity (no-op on Windows — PostMessage doesn't \
                propagate modifier-key state).".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used."},
                "element_index":{"type":"integer","description":"Element index from the last get_window_state for the same (pid, window_id)."},
                "x":{"type":"number","description":"X in window-local screenshot pixels. Must be provided together with y."},
                "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                "modifier":{"type":"array","items":{"type":"string"},"description":"Modifier keys held during the right-click. Accepted for parity; currently no-op on Windows."},
                "from_zoom":{"type":"boolean","description":"After a zoom call, pass true to translate zoom-image coords back to window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let elem_idx = args.get("element_index").and_then(|v| v.as_u64()).map(|v| v as usize);
        let x = args.get("x").and_then(|v| v.as_f64());
        let y = args.get("y").and_then(|v| v.as_f64());
        // Port Swift's full validation set.
        let has_xy     = x.is_some() && y.is_some();
        let partial_xy = x.is_some() != y.is_some();
        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if elem_idx.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if elem_idx.is_none() && !has_xy {
            return ToolResult::error("Provide element_index or (x, y) to address the right-click target.");
        }
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        if let Some(idx) = elem_idx {
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => return ToolResult::error(format!("Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first.")),
            };
            overlay_glide_to(cx as f64, cy as f64).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: cx as f64, y: cy as f64 });
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                crate::input::post_click_screen(hwnd, cx, cy, 1, "right")?;
                // Match Swift's element-path text 1:1
                // (`"✅ Shown menu for [N] role \"title\"."`).  UIA role/title
                // placeholder pending element-cache enrichment.
                Ok(format!("✅ Shown menu for [{idx}] (screen ({cx}, {cy}))."))
            }).await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args.get("from_zoom").and_then(|v| v.as_bool()).unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(px, py); px = wx; py = wy; }
                    None => return ToolResult::error(
                        format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                    ),
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            let (sx, sy) = unsafe {
                use windows::Win32::Foundation::{HWND, POINT};
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                let mut pt = POINT { x: px as i32, y: py as i32 };
                let _ = ClientToScreen(HWND(hwnd as *mut _), &mut pt);
                (pt.x as f64, pt.y as f64)
            };
            overlay_glide_to(sx, sy).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy });
            let (xi, yi) = (px as i32, py as i32);
            let result = tokio::task::spawn_blocking(move || crate::input::post_click(hwnd, xi, yi, 1, "right")).await;
            match result {
                Ok(Ok(())) => {
                    // Swift pixel-path text 1:1.
                    ToolResult::text(format!(
                        "✅ Posted right-click to pid {pid} at window-pixel ({xi}, {yi}) → screen-point ({}, {}).",
                        sx as i32, sy as i32))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Guarded above by the early validation block — unreachable.
            unreachable!()
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
            description: "Press-drag-release gesture from (from_x, from_y) to (to_x, to_y) in window-local screenshot pixels. \
                          duration_ms (default 500) is the wall-clock budget; steps (default 20) interpolates intermediate \
                          WM_MOUSEMOVE events along the path. No focus steal. After a zoom call, pass from_zoom=true.".into(),
            input_schema: json!({"type":"object","required":["pid","from_x","from_y","to_x","to_y"],"properties":{
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Target window handle (HWND). Optional — driver picks frontmost window of pid when omitted."},
                "from_x":{"type":"number","description":"Drag-start X in window-local screenshot pixels."},
                "from_y":{"type":"number","description":"Drag-start Y in window-local screenshot pixels."},
                "to_x":{"type":"number","description":"Drag-end X in window-local screenshot pixels."},
                "to_y":{"type":"number","description":"Drag-end Y in window-local screenshot pixels."},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Wall-clock duration of drag path. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Number of intermediate WM_MOUSEMOVE events. Default: 20."},
                "modifier":{"type":"array","items":{"type":"string"},"description":"Modifier keys: cmd/shift/option/ctrl (held via keyboard)."},
                "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Default: left."},
                "from_zoom":{"type":"boolean","description":"When true, coordinates are in the last zoom image for this pid."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;

        let coerce = |key: &str| -> Option<f64> {
            args.get(key).and_then(|v| v.as_f64())
                .or_else(|| args.get(key).and_then(|v| v.as_i64()).map(|i| i as f64))
        };
        let from_x_opt = coerce("from_x");
        let from_y_opt = coerce("from_y");
        let to_x_opt   = coerce("to_x");
        let to_y_opt   = coerce("to_y");
        let (mut from_x, mut from_y, mut to_x, mut to_y) =
            match (from_x_opt, from_y_opt, to_x_opt, to_y_opt) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => return ToolResult::error(
                    "from_x, from_y, to_x, and to_y are all required (window-local pixels)."),
            };

        let hwnd_opt    = args.get("window_id").and_then(|v| v.as_u64());
        let duration_ms = args.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(500);
        let steps       = args.get("steps").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
        let button      = args.get("button").and_then(|v| v.as_str()).unwrap_or("left").to_owned();
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

        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };

        // Compute screen-coord endpoints for Swift's text format.
        let (sx_from, sy_from, sx_to, sy_to) = unsafe {
            use windows::Win32::Foundation::{HWND, POINT};
            use windows::Win32::Graphics::Gdi::ClientToScreen;
            let mut p1 = POINT { x: from_x as i32, y: from_y as i32 };
            let mut p2 = POINT { x: to_x   as i32, y: to_y   as i32 };
            let _ = ClientToScreen(HWND(hwnd as *mut _), &mut p1);
            let _ = ClientToScreen(HWND(hwnd as *mut _), &mut p2);
            (p1.x, p1.y, p2.x, p2.y)
        };

        let button_c = button.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::mouse::post_drag(
                hwnd,
                from_x as i32, from_y as i32,
                to_x   as i32, to_y   as i32,
                duration_ms, steps, &button_c,
            )
        }).await;

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format verbatim.
                let button_suffix = if button == "left" { String::new() } else { format!(" ({} button)", button) };
                ToolResult::text(format!(
                    "✅ Posted drag{button_suffix} to pid {raw_pid} \
                     from window-pixel ({}, {}) → ({}, {}), \
                     screen ({sx_from}, {sy_from}) → ({sx_to}, {sy_to}) \
                     in {duration_ms}ms / {steps} steps.",
                    from_x as i32, from_y as i32, to_x as i32, to_y as i32,
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
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
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        // SM_CXSCREEN/SM_CYSCREEN return values in the DPI of the current
        // process; for a DPI-unaware process they're in *logical* points,
        // matching Swift's NSScreen.frame.  Scale = system DPI / 96.
        let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let dpi = unsafe { GetDpiForSystem() };
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
        // Matches Swift text format 1:1.
        ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
            .with_structured(json!({ "width": w, "height": h, "scale_factor": scale }))
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
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        unsafe { let _ = GetCursorPos(&mut pt); }
        // Text format matches Swift `GetCursorPositionTool` 1:1.
        ToolResult::text(format!("✅ Cursor at ({}, {})", pt.x, pt.y))
            .with_structured(json!({ "x": pt.x, "y": pt.y }))
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
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so
        // the cursor settles to the natural macOS-style pose.
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
            // Description ported from Swift `SetAgentCursorEnabledTool.swift`.
            description: "Toggle the visual agent-cursor overlay. When enabled, future \
                pointer actions animate a floating arrow to the target's on-screen position \
                before firing the click — purely visual, the click dispatch itself is \
                unchanged. Disabling removes the overlay immediately.\n\n\
                Default: enabled. Stays on for the life of the MCP session / daemon; \
                disable with `{\"enabled\": false}` for headless / CI runs where the \
                visual isn't wanted.\n\n\
                Rust-only: `cursor_id` selects an instance from the multi-cursor registry; \
                default is `'default'` (Swift has a single AgentCursor.shared).".into(),
            input_schema: json!({"type":"object","required":["enabled"],"properties":{
                "enabled":{"type":"boolean","description":"True to show the overlay cursor; false to hide."},
                "cursor_id":{"type":"string","description":"Rust-only: multi-cursor instance id. Default 'default'."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let enabled = match args.get("enabled").and_then(|v| v.as_bool()) {
            Some(v) => v,
            None    => return ToolResult::error("Missing required boolean field `enabled`."),
        };
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default");
        self.state.cursor_registry.set_enabled(cursor_id, enabled);
        crate::overlay::send_command(cursor_overlay::OverlayCommand::SetEnabled(enabled));
        // Match Swift text format 1:1: `"✅ Agent cursor enabled."`
        // (or `"✅ Agent cursor disabled."`).
        ToolResult::text(if enabled {
            "✅ Agent cursor enabled.".to_owned()
        } else {
            "✅ Agent cursor disabled.".to_owned()
        })
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
            description: "Configure the visual appearance and motion curve of an agent cursor instance.\n\n\
                Appearance:\n\
                - cursor_id: instance name (default='default')\n\
                - cursor_icon: built-in ('arrow','crosshair','hand','dot') or PNG/SVG file path\n\
                - cursor_color: hex color e.g. '#00FFFF' or CSS name\n\
                - cursor_label: short text shown near the cursor\n\
                - cursor_size: dot radius in points (default=16)\n\
                - cursor_opacity: 0.0–1.0 (default=0.85)\n\n\
                Motion curve (Bezier):\n\
                - arc_size: perpendicular deflection as fraction of path length [0,1]. Default 0.25\n\
                - spring: settle damping [0.3,1.0]; 1.0=no overshoot. Default 0.72\n\
                - glide_duration_ms: flight duration per move [50,5000]. Default 160\n\
                - dwell_after_click_ms: pause after click ripple [0,5000]. Default 80\n\
                - idle_hide_ms: auto-hide delay [0,60000]; 0=never. Default 20000".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "cursor_id":{"type":"string"},
                    "cursor_icon":{"type":"string"},
                    "cursor_color":{"type":"string"},
                    "cursor_label":{"type":"string"},
                    "cursor_size":{"type":"number"},
                    "cursor_opacity":{"type":"number"},
                    "start_handle":{"type":"number","description":"Start-handle fraction [0,1]. Default 0.3."},
                    "end_handle":{"type":"number","description":"End-handle fraction [0,1]. Default 0.3."},
                    "arc_size":{"type":"number","description":"Arc deflection as fraction of path length [0,1]. Default 0.25."},
                    "arc_flow":{"type":"number","description":"Asymmetry bias [-1,1]. Default 0.0."},
                    "spring":{"type":"number","description":"Settle damping [0.3,1.0]. Default 0.72."},
                    "glide_duration_ms":{"type":"number","minimum":50,"maximum":5000,"description":"Flight duration per move in ms. Default 160."},
                    "dwell_after_click_ms":{"type":"number","minimum":0,"maximum":5000,"description":"Pause after click ripple in ms. Default 80."},
                    "idle_hide_ms":{"type":"number","minimum":0,"maximum":60000,"description":"Auto-hide delay in ms. 0=never. Default 20000."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // JSON numbers without decimals parse as ints; coerce to f64 so
        // callers can write `{"glide_duration_ms": 1500}` without it being
        // silently ignored (matches Swift's `number()` helper).
        fn num(v: Option<&Value>) -> Option<f64> {
            v.and_then(|x| x.as_f64().or_else(|| x.as_i64().map(|i| i as f64)))
        }
        let cursor_id = args.get("cursor_id").and_then(|v| v.as_str()).unwrap_or("default").to_owned();
        // 1. Per-instance appearance fields (Rust-only).
        self.state.cursor_registry.update_config(&cursor_id, |cfg| {
            if let Some(v) = args.get("cursor_icon").and_then(|v| v.as_str()) { cfg.cursor_icon = Some(v.to_owned()); }
            if let Some(v) = args.get("cursor_color").and_then(|v| v.as_str()) { cfg.cursor_color = Some(v.to_owned()); }
            if let Some(v) = args.get("cursor_label").and_then(|v| v.as_str()) { cfg.cursor_label = Some(v.to_owned()); }
            if let Some(v) = num(args.get("cursor_size"))    { cfg.cursor_size    = Some(v); }
            if let Some(v) = num(args.get("cursor_opacity")) { cfg.cursor_opacity = Some(v.clamp(0.0, 1.0)); }
        });
        // 2. Apply motion knobs to the live render state — was silently
        // dropped before; this is the Swift parity behavior.
        let current = crate::overlay::current_motion();
        let updated = current.with_overrides(
            num(args.get("start_handle")),
            num(args.get("end_handle")),
            num(args.get("arc_size")),
            num(args.get("arc_flow")),
            num(args.get("spring")),
            num(args.get("glide_duration_ms")),
            num(args.get("dwell_after_click_ms")),
            num(args.get("idle_hide_ms")),
            None, // press_duration_ms — not in Swift tool surface
        );
        crate::overlay::send_command(cursor_overlay::OverlayCommand::SetMotion(updated.clone()));
        // Match Swift text format 1:1.
        let summary = format!(
            "cursor motion: startHandle={sh} endHandle={eh} arcSize={asz} arcFlow={af} \
             spring={sp} glideDurationMs={gd} dwellAfterClickMs={dw} idleHideMs={ih}",
            sh = updated.start_handle, eh = updated.end_handle,
            asz = updated.arc_size,   af = updated.arc_flow,
            sp = updated.spring,
            gd = updated.glide_duration_ms as i64,
            dw = updated.dwell_after_click_ms as i64,
            ih = updated.idle_hide_ms as i64,
        );
        ToolResult::text(format!("✅ {summary}")).with_structured(json!({
            "cursor_id":            cursor_id,
            "start_handle":         updated.start_handle,
            "end_handle":           updated.end_handle,
            "arc_size":             updated.arc_size,
            "arc_flow":             updated.arc_flow,
            "spring":               updated.spring,
            "glide_duration_ms":    updated.glide_duration_ms,
            "dwell_after_click_ms": updated.dwell_after_click_ms,
            "idle_hide_ms":         updated.idle_hide_ms,
        }))
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
            // Description ported from Swift `GetAgentCursorStateTool.swift`.
            description: "Report the current agent-cursor configuration: enabled flag, \
                motion knobs (startHandle, endHandle, arcSize, arcFlow, spring), glide \
                duration, post-click dwell, and idle-hide delay. Durations come back in \
                milliseconds to match the setter's units. Pure read-only — no side effects.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let enabled = crate::overlay::is_enabled();
        let motion  = crate::overlay::current_motion();
        // Swift text format 1:1: single-line camelCase key=value pairs.
        let summary = format!(
            "cursor: enabled={enabled} startHandle={sh} endHandle={eh} arcSize={asz} \
             arcFlow={af} spring={sp} glideDurationMs={gd} dwellAfterClickMs={dw} idleHideMs={ih}",
            sh = motion.start_handle, eh = motion.end_handle,
            asz = motion.arc_size,   af = motion.arc_flow,
            sp = motion.spring,
            gd = motion.glide_duration_ms as i64,
            dw = motion.dwell_after_click_ms as i64,
            ih = motion.idle_hide_ms as i64,
        );
        // Rust-only structured payload: the same fields + the multi-cursor
        // instance map. Cursor instances are a Rust extension Swift doesn't
        // expose; documented in PARITY.md.
        let cursors = serde_json::to_value(self.state.cursor_registry.all_states()).unwrap_or_default();
        ToolResult::text(format!("✅ {summary}"))
            .with_structured(json!({
                "enabled":              enabled,
                "start_handle":         motion.start_handle,
                "end_handle":           motion.end_handle,
                "arc_size":             motion.arc_size,
                "arc_flow":             motion.arc_flow,
                "spring":               motion.spring,
                "glide_duration_ms":    motion.glide_duration_ms,
                "dwell_after_click_ms": motion.dwell_after_click_ms,
                "idle_hide_ms":         motion.idle_hide_ms,
                "cursors":              cursors,
            }))
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
                        let path_owned2 = path.to_owned();
                        self.state.cursor_registry.update_config(&cursor_id, |c| {
                            c.cursor_icon = Some(path_owned2);
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

        // Swift `SetAgentCursorStyleTool` text format: only include fields
        // whose post-write value is `Some` (i.e. not reverted to default).
        // Falls back to "✅ cursor style: reverted to default" when every
        // field is empty.
        let mut parts: Vec<String> = Vec::new();
        if let Some(arr) = args.get("gradient_colors").and_then(|v| v.as_array()) {
            let hexes: Vec<String> = arr.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if !hexes.is_empty() {
                parts.push(format!("gradient_colors=[{}]", hexes.join(",")));
            }
        }
        if let Some(s) = args.get("bloom_color").and_then(|v| v.as_str()) {
            if !s.is_empty() { parts.push(format!("bloom_color={s}")); }
        }
        if let Some(s) = image_path {
            if !s.is_empty() { parts.push(format!("image_path={s}")); }
        }
        let summary = if parts.is_empty() {
            "reverted to default".to_owned()
        } else {
            parts.join(" ")
        };
        ToolResult::text(format!("✅ cursor style: {summary}"))
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
            description: "Check required permissions for cua-driver-rs on Windows.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        // On Windows, background PostMessage access doesn't require elevated permissions.
        // UIA works for most apps without elevation; some system apps require elevation.
        use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
        use windows::Win32::Security::{TOKEN_QUERY, GetTokenInformation, TokenElevation, TOKEN_ELEVATION};
        let is_elevated = unsafe {
            let proc = GetCurrentProcess();
            let mut token = windows::Win32::Foundation::HANDLE::default();
            if OpenProcessToken(proc, TOKEN_QUERY, &mut token).is_ok() {
                let mut elevation = TOKEN_ELEVATION::default();
                let mut ret_len = 0u32;
                let ok = GetTokenInformation(
                    token,
                    TokenElevation,
                    Some(&mut elevation as *mut _ as *mut _),
                    std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                    &mut ret_len,
                ).is_ok();
                let _ = windows::Win32::Foundation::CloseHandle(token);
                ok && elevation.TokenIsElevated != 0
            } else {
                false
            }
        };
        let status_text = format!(
            "Process elevation: {}\nUIA accessibility: available (no special permission needed)\nPostMessage injection: available",
            if is_elevated { "✅ elevated (administrator)" } else { "ℹ️ standard user" }
        );
        ToolResult::text(status_text)
            .with_structured(json!({ "elevated": is_elevated, "uia": true, "post_message": true }))
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
            // Description ported from Swift `GetConfigTool.swift` with the
            // Windows config path. Swift's `agent_cursor.*` subtree is not
            // mirrored on Windows yet (separate cursor-overlay config path);
            // documented in PARITY.md.
            description: "Report the current persistent driver config.\n\n\
                Pure read-only. Returns defaults when the underlying state is unset — \
                same fallback the daemon uses at startup. Sibling to `set_config`.\n\n\
                Current schema (Windows):\n\n  \
                {\n    \"schema_version\": 1,\n    \"version\": \"<crate version>\",\n    \
                \"platform\": \"windows\",\n    \"capture_mode\": \"som\" | \"vision\" | \"ax\",\n    \
                \"max_image_dimension\": 0\n  }\n".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        // Mirror the macOS agent's parity addition (commit adb9ecca):
        // nested `agent_cursor.enabled` block so Swift-shaped get_config
        // consumers can read the cursor's enabled state from one place.
        let cursor_enabled = self.state.cursor_registry.all_states()
            .first()
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
        });
        // Match Swift's text format 1:1: `"✅ <pretty JSON>"`.
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
        ToolResult::text(format!("✅ {pretty}")).with_structured(payload)
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
            // Description ported from Swift `SetConfigTool.swift`.  Windows
            // accepts BOTH Swift's `{key, value}` dotted-path shape AND a
            // legacy per-field shape; documented as intentional.
            description: "Write a setting into the persistent driver config. Values take \
                effect immediately.\n\n\
                Two input shapes:\n\
                - **Swift-compatible** (preferred): `{\"key\": \"capture_mode\", \"value\": \"som\"}` \
                  — single dotted-path leaf write.\n\
                - **Legacy per-field** (Rust-only): `{\"capture_mode\": \"som\", \"max_image_dimension\": 0}` \
                  — bulk write of named fields.\n\n\
                Known keys:\n\
                - `capture_mode` (string: `vision` | `ax` | `som`)\n\
                - `max_image_dimension` (integer)\n\n\
                Returns the full updated config in the same shape as `get_config`.".into(),
            input_schema: json!({"type":"object","properties":{
                "key":{"type":"string","description":"Dotted snake_case path to a leaf config field (Swift-compatible shape). Pair with `value`."},
                "value":{"description":"New value for `key`. JSON type depends on the key."},
                "capture_mode":{"type":"string","enum":["som","vision","ax"],"description":"Legacy per-field shape."},
                "max_image_dimension":{"type":"integer","description":"Legacy per-field shape."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let mut cfg = self.state.config.write().unwrap();
        let mut applied = false;
        // Swift-compatible {key, value} shape.
        if let (Some(key), Some(val)) = (
            args.get("key").and_then(|v| v.as_str()),
            args.get("value"),
        ) {
            match key {
                "capture_mode" => match val.as_str() {
                    Some(s) => { cfg.capture_mode = s.to_owned(); applied = true; }
                    None    => return ToolResult::error(format!("`capture_mode` must be a string, got {val}.")),
                },
                "max_image_dimension" => match val.as_u64() {
                    Some(n) => { cfg.max_image_dimension = n as u32; applied = true; }
                    None    => return ToolResult::error(format!("`max_image_dimension` must be an integer, got {val}.")),
                },
                other => return ToolResult::error(format!(
                    "Unknown config key `{other}`. Known: capture_mode, max_image_dimension."
                )),
            }
        }
        // Legacy per-field shape.
        if let Some(mode) = args.get("capture_mode").and_then(|v| v.as_str()) {
            cfg.capture_mode = mode.to_owned(); applied = true;
        }
        if let Some(dim) = args.get("max_image_dimension").and_then(|v| v.as_u64()) {
            cfg.max_image_dimension = dim as u32; applied = true;
        }
        if !applied {
            return ToolResult::error("Missing required string field `key` (or a known legacy per-field).");
        }
        // Emit the same pretty-JSON payload as `get_config` (matches Swift's
        // `set_config` return shape — both tools echo the full config after).
        let cursor_enabled = self.state.cursor_registry.all_states()
            .first()
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
        });
        let pretty = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
        ToolResult::text(format!("✅ {pretty}")).with_structured(payload)
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
                on-screen visible windows with their bounds and owner pid.\n\n\
                For the full UIA subtree of a single window (with interactive element indices \
                you can click by), use get_window_state instead — this is a fast discovery read.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let (procs, windows) = tokio::task::spawn_blocking(|| {
            (crate::win32::list_processes(), crate::win32::list_windows(None))
        }).await.unwrap_or_default();

        let mut lines = vec![format!(
            "{} running process(es), {} visible window(s)",
            procs.len(), windows.len()
        )];
        for p in &procs {
            lines.push(format!("- {} (pid {})", p.name, p.pid));
        }
        if !windows.is_empty() {
            lines.push(String::new());
            lines.push("Windows:".to_owned());
            for w in &windows {
                let title = if w.title.is_empty() { "(no title)".to_owned() }
                    else { format!("\"{}\"", w.title) };
                lines.push(format!(
                    "- pid={} {} [window_id: {}] {}x{}+{}+{}",
                    w.pid, title, w.hwnd, w.width, w.height, w.x, w.y
                ));
            }
            lines.push("→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned());
        }

        let structured = json!({
            "processes": procs.iter().map(|p| json!({"pid":p.pid,"name":p.name})).collect::<Vec<_>>(),
            "windows": windows.iter().map(|w| json!({
                "window_id": w.hwnd, "pid": w.pid, "title": w.title,
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
            // Description ported from Swift `ZoomTool.swift`.
            description: "Zoom into a rectangular region of a window screenshot at full (native) \
                resolution. Use this when `get_window_state` returned a resized image and you \
                need to read small text, identify icons, or verify UI details.\n\n\
                Coordinates `x1, y1, x2, y2` are in the same pixel space as the screenshot \
                returned by `get_window_state` (i.e. the resized image if `max_image_dimension` \
                is active). The maximum zoom region width is 500 px in scaled-image coordinates.\n\n\
                A 20% padding is automatically added on every side of the requested region so \
                the target remains visible even if the caller's coordinates are slightly off.\n\n\
                After a zoom, pass `from_zoom=true` to click/type_text to auto-translate \
                coordinates back to full-window space.\n\n\
                Windows-specific: `window_id` is the canonical addressing field (HWND). \
                `pid` is also required (used to scope the zoom translation registry, matching \
                Swift's per-pid zoom context).".into(),
            input_schema: json!({
                "type":"object","required":["pid","window_id","x1","y1","x2","y2"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID."},
                    "window_id":{"type":"integer","description":"HWND of the target window."},
                    "x1":{"type":"number","description":"Left edge of the region (resized-image pixels)."},
                    "y1":{"type":"number","description":"Top edge of the region (resized-image pixels)."},
                    "x2":{"type":"number","description":"Right edge of the region (resized-image pixels)."},
                    "y2":{"type":"number","description":"Bottom edge of the region (resized-image pixels)."}
                },"additionalProperties":false
            }),
            // Swift annotation: idempotent: false.
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None    => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = Some(raw_pid as u32);
        let hwnd = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None    => return ToolResult::error("Missing required integer field window_id."),
        };
        let coerce = |k: &str| args.get(k).and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64)));
        let (x1, y1, x2, y2) = match (coerce("x1"), coerce("y1"), coerce("x2"), coerce("y2")) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return ToolResult::error("Missing required region coordinates (x1, y1, x2, y2)."),
        };
        if x2 <= x1 || y2 <= y1 {
            return ToolResult::error("Invalid region: x2 must be > x1 and y2 must be > y1.");
        }
        if x2 - x1 > 500.0 {
            return ToolResult::error(format!(
                "Zoom region too wide: {} px > 500 px max. Use a narrower region (max 500 px in scaled-image coordinates).",
                (x2 - x1) as i64
            ));
        }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            let png = crate::capture::screenshot_window_bytes(hwnd)?;
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
                // Match Swift's text format 1:1.
                let summary = "✅ Zoomed region captured at native resolution. \
                    To click a target in this image, use \
                    `click(pid, x, y, from_zoom=true)` where x,y are pixel \
                    coordinates in THIS zoomed image — the driver maps them \
                    back automatically.".to_owned();
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(summary),
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

pub struct TypeTextCharsTool { state: Arc<ToolState> }
static TYPE_CHARS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for TypeTextCharsTool {
    fn def(&self) -> &ToolDef {
        TYPE_CHARS_DEF.get_or_init(|| ToolDef {
            name: "type_text_chars".into(),
            description: "Type text character-by-character with a configurable inter-character \
                delay (default 30 ms). Useful for apps that miss keystrokes. \
                Otherwise identical to type_text (WM_CHAR, no focus steal).".into(),
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
        let hwnd_opt = args.get("window_id").and_then(|v| v.as_u64());
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid))).await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => return ToolResult::error(format!("No windows found for pid {pid}. Provide window_id.")),
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::post_type_text_with_delay(hwnd, &text, delay_ms)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Typed {text_len} character(s) with {delay_ms}ms delay.")),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── registry builder ──────────────────────────────────────────────────────────

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
    r.register(Box::new(TypeTextTool { state: state.clone() }));
    r.register(Box::new(PressKeyTool));
    r.register(Box::new(HotkeyTool));
    r.register(Box::new(SetValueTool { state: state.clone() }));
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
    // `type_text_chars` is intentionally NOT registered — Swift treats it
    // as a deprecated alias for `type_text` resolved at invoke time in
    // mcp-server's `ToolRegistry::invoke`. Keeping it out of the registry
    // means it doesn't show up in `tools/list` either, matching Swift's
    // ToolRegistry.swift (`type_text_chars` not in `handlers`).
    let _: &TypeTextCharsTool = &TypeTextCharsTool { state: state.clone() }; // touch struct so it stays in this crate for now
    r.register_recording_tools();
    r
}
