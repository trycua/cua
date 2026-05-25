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
    fn default() -> Self { Self { capture_mode: "som".into(), max_image_dimension: 1568 } }
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
            description: "List Windows apps — both currently running and installed-but-not-running — \
                with per-app state flags:\n\n\
                - running: is a process for this app live? (pid is 0 when false)\n\
                - active: is it the system-frontmost app? (implies running)\n\
                - kind: `\"desktop\"` for `.exe`-backed apps (resolved from Start-Menu \
                shortcuts) or `\"uwp\"` for packaged store apps.\n\
                - launch_path: what `launch_app` would consume.\n\
                    * desktop: full `.exe` commandline (path + any arguments preserved from \
                      the source `.lnk`; the exe is quoted when it contains whitespace).\n\
                    * uwp: `shell:appsFolder\\{PackageFamilyName}!{AppId}` where `{AppId}` \
                      falls back to `App` when the package manifest does not expose a \
                      specific Application.Id.\n\
                - last_used: RFC3339 mtime of the launcher (`.lnk` for desktop, \
                package install location for UWP), when readable.\n\n\
                Running apps are derived from visible top-level windows + the foreground \
                pid (mirrors NSApplicationActivationPolicyRegular's intent: background \
                services and console processes are excluded). Installed apps are merged \
                from Start-Menu `.lnk` enumeration and the WinRT PackageManager — running \
                processes whose executable matches a Start-Menu target are folded into a \
                single entry with `running: true`.\n\n\
                Use this for \"is X installed?\" as well as \"is X running?\". For per-window \
                state — on-screen, minimized, window titles — call list_windows instead. \
                For just opening an app — running or not — call launch_app({path: ...}) \
                directly; list_apps is not a prerequisite.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        // Strategy:
        // 1. Enumerate top-level visible windows → derive the set of running pids
        //    that have a user-facing surface.
        // 2. Enumerate the process table to map pid → executable name and full
        //    path (used to merge against Start-Menu / UWP launch_paths).
        // 3. Enumerate installed apps from Start-Menu shortcuts + WinRT
        //    PackageManager. Each installed entry has a launch_path.
        // 4. Merge: a running pid whose executable path matches an installed
        //    desktop launch_path becomes a single entry with running=true and
        //    that launch_path. Remaining installed entries are emitted with
        //    running=false, pid=0. Remaining running pids (no installed-app
        //    match) are emitted as running=true with launch_path=null.
        let apps = tokio::task::spawn_blocking(|| -> Vec<serde_json::Value> {
            use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;

            // Per-step timings logged via `tracing::debug!` (silent at default
            // log level — enable with `RUST_LOG=cua_driver=debug` when debugging
            // a slow list_apps invocation).
            let t0 = std::time::Instant::now();
            let wins = crate::win32::list_windows(None);
            tracing::debug!(target: "list_apps", "step 1 list_windows: {} wins ({}ms)", wins.len(), t0.elapsed().as_millis());
            let mut seen_pids = std::collections::HashSet::new();
            let mut running_pids: Vec<u32> = Vec::new();
            for w in &wins {
                if seen_pids.insert(w.pid) { running_pids.push(w.pid); }
            }

            let t1 = std::time::Instant::now();
            let foreground_pid = unsafe {
                let fg = GetForegroundWindow();
                let mut pid = 0u32;
                let _ = GetWindowThreadProcessId(fg, Some(&mut pid));
                pid
            };
            tracing::debug!(target: "list_apps", "step 2 fg-window: pid={} ({}ms)", foreground_pid, t1.elapsed().as_millis());

            let t2 = std::time::Instant::now();
            let procs = crate::win32::list_processes();
            tracing::debug!(target: "list_apps", "step 3 list_processes: {} procs ({}ms)", procs.len(), t2.elapsed().as_millis());
            let pid_to_name: std::collections::HashMap<u32, String> =
                procs.iter().map(|p| (p.pid, p.name.clone())).collect();

            let t3 = std::time::Instant::now();
            let installed = crate::win32::list_installed_apps();
            tracing::debug!(target: "list_apps", "step 4 list_installed_apps: {} installed ({}ms)", installed.len(), t3.elapsed().as_millis());
            // Lowercase-exe-basename → installed-app indices. We match running
            // processes by basename rather than full path because the process
            // table's executable name is `notepad.exe` while the Start-Menu
            // shortcut target is `C:\Windows\System32\notepad.exe` — both
            // sides need to be normalised to compare. Multiple Start-Menu
            // shortcuts often target the same basename (different `chrome.exe`
            // launchers, multiple `code.exe` profiles, …), so we bucket as
            // `Vec<usize>` and let `disambiguate_installed_match` pick a
            // winner per running pid rather than letting the last write win.
            let mut by_exe: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, app) in installed.iter().enumerate() {
                if app.kind == "desktop" {
                    let basename = std::path::Path::new(&app.launch_path)
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_ascii_lowercase();
                    if !basename.is_empty() {
                        by_exe.entry(basename).or_default().push(i);
                    }
                }
            }

            let mut consumed_installed: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut out: Vec<serde_json::Value> = Vec::new();

            for &pid in &running_pids {
                let name = pid_to_name.get(&pid).cloned().unwrap_or_else(|| "<unknown>".into());
                let key = name.to_ascii_lowercase();
                let candidates = by_exe.get(&key).map(|v| v.as_slice()).unwrap_or(&[]);
                let merged = disambiguate_installed_match(candidates, &installed, &name);
                if let Some(idx) = merged { consumed_installed.insert(idx); }
                let (display_name, bundle_id, launch_path, kind, last_used) = match merged {
                    Some(idx) => {
                        let a = &installed[idx];
                        (
                            a.name.clone(),
                            Some(a.bundle_id.clone()),
                            Some(a.launch_path.clone()),
                            Some(a.kind.clone()),
                            a.last_used.clone(),
                        )
                    }
                    None => (name, None, None, Some("desktop".to_owned()), None),
                };
                out.push(json!({
                    "pid":         pid,
                    "bundle_id":   bundle_id,
                    "name":        display_name,
                    "running":     true,
                    "active":      pid == foreground_pid,
                    "kind":        kind,
                    "launch_path": launch_path,
                    "last_used":   last_used,
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            for (i, app) in installed.iter().enumerate() {
                if consumed_installed.contains(&i) { continue }
                out.push(json!({
                    "pid":         0,
                    "bundle_id":   app.bundle_id.clone(),
                    "name":        app.name.clone(),
                    "running":     false,
                    "active":      false,
                    "kind":        app.kind.clone(),
                    "launch_path": app.launch_path.clone(),
                    "last_used":   app.last_used.clone(),
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            out
        }).await.unwrap_or_default();

        let running_count = apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false)).count();
        let total = apps.len();
        let installed_only = total - running_count;
        let mut lines = vec![format!(
            "✅ Found {total} app(s): {running_count} running, {installed_only} installed-not-running."
        )];
        for app in apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false)) {
            let name = app["name"].as_str().unwrap_or("?");
            let pid  = app["pid"].as_u64().unwrap_or(0);
            lines.push(format!("- {name} (pid {pid})"));
        }
        // `apps` is the unified shape; `processes` retained for any pre-existing
        // callers that consumed the old running-only shape.
        let structured = json!({
            "apps": apps,
            "processes": apps.iter().filter(|a| a["running"].as_bool().unwrap_or(false))
                .map(|a| json!({
                    "pid":  a["pid"], "name": a["name"]
                })).collect::<Vec<_>>(),
        });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

/// Pick which of several Start-Menu / UWP-derived installed apps best matches
/// a running pid whose exe basename collided. Precedence:
///   1. exact case-insensitive equality between `proc_exe_basename` and the
///      installed app's launch_path basename (always true here; left for
///      future per-path matches if we ever bucket on something coarser)
///   2. most recently modified launcher (`last_used` desc — RFC3339 strings
///      are lexicographically ordered the same as chronologically)
///   3. first candidate (deterministic by source order)
fn disambiguate_installed_match(
    candidates: &[usize],
    installed: &[crate::win32::InstalledApp],
    _proc_exe_basename: &str,
) -> Option<usize> {
    if candidates.is_empty() { return None; }
    if candidates.len() == 1 { return Some(candidates[0]); }

    candidates
        .iter()
        .copied()
        .max_by(|&a, &b| {
            installed[a]
                .last_used
                .as_deref()
                .unwrap_or("")
                .cmp(installed[b].last_used.as_deref().unwrap_or(""))
        })
        .or_else(|| candidates.first().copied())
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
        use mcp_server::tool_args::ArgsExt;
        let filter_pid = args.opt_u64("pid").map(|v| v as u32);
        let _on_screen_only = args.bool_or("on_screen_only", false);
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

        // z_index: list_windows merges EnumWindows first (which the Win32
        // window manager returns in canonical top-to-bottom z-order), then
        // appends any UIA-only HWNDs UIA found that EnumWindows missed
        // (UIA-only entries land at the bottom of the z-stack — they have
        // no canonical Win32 ordering). Higher list index = farther from
        // front. Swift convention: higher z_index = closer to front.
        // Invert via `(len - 1 - i)` so the front-most window gets the
        // largest z.
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
        use mcp_server::tool_args::ArgsExt;
        let capture_mode = args.str_or("capture_mode", &default_mode);
        let query = args.opt_str("query");

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

/// Split a `launch_path` round-tripped from `list_apps` into a single
/// ShellExecuteEx `lpFile` token plus a trailing arguments string. Handles
/// three input shapes:
///   - bare path (`C:\Windows\notepad.exe`): returns `(path, "")`
///   - quoted-exe + args (`"C:\Path\chrome.exe" --foo --bar`): returns the
///     unquoted path + the rest
///   - AUMID / shell launch token (`shell:appsFolder\..!..`): returns the
///     whole string as the file token with empty args (the shell resolver
///     handles these directly)
///
/// Args are returned trimmed; their internal quoting is preserved as-is so
/// `--profile-directory="Profile 2"` survives.
fn split_launchable_target(s: &str) -> (String, String) {
    let trimmed = s.trim();
    if trimmed.is_empty() { return (String::new(), String::new()); }
    // AUMID / shell launch tokens go through unchanged.
    if trimmed.starts_with("shell:") || trimmed.contains('!') {
        return (trimmed.to_owned(), String::new());
    }
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(close) = rest.find('"') {
            let path = &rest[..close];
            let args = rest[close + 1..].trim_start().to_owned();
            return (path.to_owned(), args);
        }
        // Unterminated quote — fall through and hand the whole thing back.
        return (trimmed.to_owned(), String::new());
    }
    // No quoting: be permissive. If the input contains a space and the
    // first whitespace-separated token names a `.exe`, treat it as
    // `<exe> <args>`. Otherwise hand the string back whole (lets bare paths
    // with embedded spaces still work — most installs put `Program Files`
    // shortcuts through the quoted branch above).
    if let Some(first_space) = trimmed.find(char::is_whitespace) {
        let (first, rest) = trimmed.split_at(first_space);
        if first.to_ascii_lowercase().ends_with(".exe") {
            return (first.to_owned(), rest.trim_start().to_owned());
        }
    }
    (trimmed.to_owned(), String::new())
}

/// Returns `true` when the launch target names a Chromium-based browser
/// — i.e. one whose hidden launch under `SW_SHOWNOACTIVATE` will trigger
/// renderer-occlusion throttling unless we inject the anti-throttling flags
/// below. See #1620.
///
/// Matches the executable basename (case-insensitive, with or without
/// `.exe`). Accepts bare names (the App Paths shortcut, e.g. `"msedge"`),
/// full paths (`"C:\...\msedge.exe"`), and round-tripped launch paths with
/// trailing arguments (`"\"C:\...\chrome.exe\" --foo"` — the first token is
/// matched via `split_launchable_target`).
fn is_chromium_browser_target(target: &str) -> bool {
    let (file, _) = split_launchable_target(target);
    let candidate = if file.is_empty() { target } else { file.as_str() };
    let lower = candidate.to_ascii_lowercase();
    let basename = lower
        .rsplit_once('\\')
        .map(|(_, b)| b)
        .or_else(|| lower.rsplit_once('/').map(|(_, b)| b))
        .unwrap_or(lower.as_str());
    let stem = basename.strip_suffix(".exe").unwrap_or(basename);
    matches!(
        stem,
        "msedge"
            | "chrome"
            | "brave"
            | "opera"
            | "vivaldi"
            | "chromium"
            | "thorium"
            | "iridium"
            | "browser" // Yandex Browser's exe is browser.exe
            | "arc"
    )
}

/// Anti-throttling flags injected on hidden Chromium launches (#1620).
///
/// `launch_app` uses `SW_SHOWNOACTIVATE` so the launched window is
/// non-foreground from birth. Chromium's `CalculateNativeWinOcclusion`
/// feature treats that as occluded and suspends the renderer process for
/// the tab's entire lifetime — UIA tree exposes only browser chrome, no
/// page DOM, and `PrintWindow` returns a blank body.
///
/// `CalculateNativeWinOcclusion` is the root cause; the two
/// `--disable-backgrounding-*` flags backstop the same effect through the
/// process-priority and renderer-throttling layers (Chromium suspends
/// renderers on multiple signals).
const CHROMIUM_ANTI_THROTTLING_FLAGS: &[&str] = &[
    "--disable-features=CalculateNativeWinOcclusion",
    "--disable-backgrounding-occluded-windows",
    "--disable-renderer-backgrounding",
];

/// Prepend the Chromium anti-throttling flags to `extra_args` unless the
/// caller already has each one. Merges into an existing `--disable-features=`
/// list rather than appending a second `--disable-features=` entry (Chromium
/// has subtle merging rules across duplicate flags).
fn inject_chromium_anti_throttling_flags(extra_args: &mut Vec<String>) {
    const OCCLUSION_FEATURE: &str = "CalculateNativeWinOcclusion";

    // Merge `CalculateNativeWinOcclusion` into any existing
    // `--disable-features=` entry, or insert a fresh one at the front.
    let has_occlusion = extra_args.iter().any(|a| {
        a.strip_prefix("--disable-features=")
            .map(|v| v.split(',').any(|f| f.trim() == OCCLUSION_FEATURE))
            .unwrap_or(false)
    });
    if !has_occlusion {
        if let Some(idx) = extra_args
            .iter()
            .position(|a| a.starts_with("--disable-features="))
        {
            extra_args[idx] = format!("{},{OCCLUSION_FEATURE}", extra_args[idx]);
        } else {
            extra_args.insert(
                0,
                format!("--disable-features={OCCLUSION_FEATURE}"),
            );
        }
    }

    // Boolean toggles — insert at front only when not already present.
    for flag in &["--disable-backgrounding-occluded-windows", "--disable-renderer-backgrounding"] {
        if !extra_args.iter().any(|a| a == flag) {
            extra_args.insert(0, (*flag).to_string());
        }
    }
    let _ = CHROMIUM_ANTI_THROTTLING_FLAGS; // referenced for docs alignment
}

// ── launch_app ───────────────────────────────────────────────────────────────

/// Shape of the resolved launch-target params, used to decide whether the
/// best-effort foreground-restore guard should fire.
///
/// The decision is intentionally a pure function of the params (not of any
/// HWND state) so it can be unit-tested. See
/// [`should_restore_foreground_after_launch`].
#[derive(Clone, Copy, Debug)]
struct LaunchTargetShape {
    has_aumid: bool,
    has_bundle_id: bool,
    has_name: bool,
    has_path: bool,
    has_launch_path: bool,
    has_urls: bool,
}

/// True iff the launch is "open an app in the background" (so we want to
/// restore the user's foreground after the spawn activates the new window)
/// rather than "open a URL in the default browser" (where the user
/// explicitly asked for that page to come up).
///
/// Rule, matching the audit spec:
/// - URLs-only call (no app-identifying field set) → SKIP restore. The
///   user asked for `https://…` to be openable in their default browser;
///   a browser instance freshly launched for that URL is the legitimate
///   foreground.
/// - Any app-identifying field present (`aumid` / `bundle_id` / `name` /
///   `path` / `launch_path`) → RESTORE. The intent is hidden-launch +
///   background drive; the no-foreground contract applies.
fn should_restore_foreground_after_launch(shape: LaunchTargetShape) -> bool {
    let has_app_identifier = shape.has_aumid
        || shape.has_bundle_id
        || shape.has_name
        || shape.has_path
        || shape.has_launch_path;
    // URLs-only carve-out: when the caller passed urls AND no
    // app-identifying field, skip the restore.
    if shape.has_urls && !has_app_identifier {
        return false;
    }
    has_app_identifier
}

/// Async polling restore of the prior foreground window, used by
/// `LaunchAppTool` to mirror the macOS `FocusRestoreGuard` for the legacy
/// `ShellExecuteExW` launch path. The UWP/AUMID branch has its own
/// synchronous restoration in `launch_uwp::restore_foreground_best_effort`
/// and is NOT routed through here.
///
/// Approach:
/// 1. Try a short `WaitForInputIdle` on the spawned pid (if the handle
///    is openable) — this gets us past the worst of the new app's
///    window-creation window before we start sampling foreground state.
/// 2. Poll every 100ms for up to ~3s; when `GetForegroundWindow()` is no
///    longer the prior foreground (i.e. the spawned app actually grabbed
///    focus), call `SetForegroundWindow(prior)`. We avoid restoring
///    eagerly so we don't race the new app's own activation and lose.
///
/// `SetForegroundWindow` from non-UIAccess processes is restricted by
/// Windows' foreground-lock; the call may silently fail. We log the
/// failure at trace level and proceed — no error is surfaced, the launch
/// itself already succeeded.
///
/// Note: `prior_foreground_addr` is the raw HWND value as `usize` (rather
/// than `HWND` directly) because `HWND` wraps `*mut c_void` which is
/// `!Send` and would prevent this future from being scheduled on the
/// multi-threaded tokio runtime.
async fn restore_foreground_polling_best_effort(
    prior_foreground_addr: usize,
    spawned_pid: u32,
) {
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{
        OpenProcess, WaitForInputIdle, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowThreadProcessId, IsWindow, SetForegroundWindow,
    };

    if prior_foreground_addr == 0 {
        tracing::trace!(
            target: "launch_app.focus_restore",
            "no prior foreground HWND to restore — skipping"
        );
        return;
    }

    // Without a captured spawned pid we have no ownership signal — any
    // foreground change during the 3 s window might be the user
    // legitimately Alt-Tabbing, and we shouldn't yank focus back. Skip
    // the entire guard. This is the launcher-stub fallback case
    // (rare for fresh launches).
    if spawned_pid == 0 {
        tracing::trace!(
            target: "launch_app.focus_restore",
            "no spawned pid captured — skipping focus-restore guard (no ownership signal)"
        );
        return;
    }

    // Phase 1: best-effort wait for the spawned process to finish initial
    // window creation. WaitForInputIdle returns when the process pumps
    // its first message — that's typically when the main window has
    // registered. Capped at 1s; the polling loop below covers the rest.
    if spawned_pid != 0 {
        // PROCESS_SYNCHRONIZE is required by WaitForInputIdle;
        // PROCESS_QUERY_LIMITED_INFORMATION is a no-op extra that some
        // antivirus filters tolerate better than QUERY_INFORMATION.
        // OpenProcess result is also `!Send` (HANDLE), so do the open +
        // wait + close in one blocking task.
        let _ = tokio::task::spawn_blocking(move || unsafe {
            if let Ok(handle) = OpenProcess(
                PROCESS_SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                false,
                spawned_pid,
            ) {
                if !handle.is_invalid() {
                    let _ = WaitForInputIdle(handle, 1000);
                }
                // Always close the handle, even if `is_invalid()` (cheap;
                // matches the OpenProcess pairing convention used elsewhere
                // in this crate).
                let _ = CloseHandle(handle);
            }
            // Silently no-op if OpenProcess failed (target may have already
            // exited; rare for a fresh launch but not impossible for a
            // launcher-stub).
        })
        .await;
    }

    // Phase 2: poll for "spawned app became foreground", then flip back.
    // Reconstruct HWND only inside synchronous probes so the future state
    // never holds an `!Send` value across an await point.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if std::time::Instant::now() >= deadline {
            tracing::trace!(
                target: "launch_app.focus_restore",
                "spawned pid {} did not steal foreground within deadline — no restore needed",
                spawned_pid
            );
            return;
        }
        // Only restore when the current foreground window is owned by
        // the spawned pid — otherwise the user may have legitimately
        // Alt-Tabbed during the polling window and we'd be yanking
        // focus away from them. Resolves fg_now's owning PID via
        // GetWindowThreadProcessId and compares against spawned_pid.
        let activated_then_restored: Option<bool> = {
            let prior = HWND(prior_foreground_addr as *mut _);
            let fg_now = unsafe { GetForegroundWindow() };
            if fg_now == prior {
                None
            } else {
                let mut fg_pid: u32 = 0;
                let _ = unsafe { GetWindowThreadProcessId(fg_now, Some(&mut fg_pid)) };
                if fg_pid != spawned_pid {
                    // Foreground changed but to something we didn't
                    // spawn (e.g. user Alt-Tabbed). Don't touch it.
                    None
                } else if !unsafe { IsWindow(prior) }.as_bool() {
                    tracing::trace!(
                        target: "launch_app.focus_restore",
                        "prior foreground HWND no longer valid — skipping restore (spawned pid {} owns fg)",
                        spawned_pid
                    );
                    Some(true)
                } else {
                    let ok = unsafe { SetForegroundWindow(prior) }.as_bool();
                    if !ok {
                        tracing::trace!(
                            target: "launch_app.focus_restore",
                            "SetForegroundWindow returned FALSE — likely foreground-lock denial; \
                             prior HWND 0x{:x}, spawned pid {}",
                            prior_foreground_addr,
                            spawned_pid
                        );
                    } else {
                        tracing::debug!(
                            target: "launch_app.focus_restore",
                            "restored prior foreground HWND 0x{:x} after spawned pid {} activated",
                            prior_foreground_addr,
                            spawned_pid
                        );
                    }
                    Some(ok)
                }
            }
        };
        if activated_then_restored.is_some() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub struct LaunchAppTool;
static LAUNCH_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        LAUNCH_DEF.get_or_init(|| ToolDef {
            name: "launch_app".into(),
            // Description ported from Swift `LaunchAppTool.swift` with
            // Windows-specific notes. `bundle_id` is accepted as an alias
            // for `name` on Windows, with one extra behavior: if the
            // string looks like an AUMID (App User Model ID — contains
            // `!`, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`) we
            // route through `IApplicationActivationManager` so packaged
            // (Microsoft Store / UWP / MSIX) apps return the real
            // process pid instead of a stub-redirect pid.
            description: "Launch a Windows app hidden — the driver never brings the target to \
                the foreground, the target's window is launched with SW_SHOWNOACTIVATE so it \
                does not steal focus from whatever is currently frontmost.\n\n\
                Provide either `bundle_id` / `name` / `aumid` (resolved as below) or `path` \
                (full path to an executable). If both `name` and `path` are given, `path` wins. \
                `urls` opens each URL in the default browser without activating it.\n\n\
                Routing order: explicit `aumid` (or a `bundle_id` containing `!`) activates the \
                packaged app via `IApplicationActivationManager::ActivateApplication`, returning \
                the real packaged-process pid — required on Win11 for built-in apps like \
                Notepad, Calculator, and Paint, which now ship as Microsoft Store packages \
                where the legacy .exe in System32 is a ~7 KB stub that exits immediately. A \
                plain `name` first attempts a `shell:AppsFolder` lookup; on a match it goes \
                through the packaged-app path, otherwise it falls back to ShellExecuteEx's \
                PATH search.\n\n\
                Returns the launched app's pid, name, active flag, AND a `windows` array — \
                same per-window shape `list_windows` returns. When the launch settles but no \
                window has materialized yet (transient; rare), `windows` comes back empty — \
                call `list_windows(pid)` explicitly a moment later. `bundle_id` in the \
                response is set to the AUMID actually used for packaged-app launches and \
                `null` for ShellExecuteEx launches.\n\n\
                Windows-only field: `path` (Swift uses `bundle_id` since macOS apps resolve via \
                LaunchServices; Windows has no LaunchServices, so `path` is the canonical form). \
                The macOS-specific `electron_debugging_port`, `webkit_inspector_port`, \
                `creates_new_application_instance`, and `additional_arguments` fields are \
                accepted; `additional_arguments` is honored (forwarded as ShellExecuteEx \
                parameters or as the AUMID activation arguments string), the others currently \
                no-op on Windows.".into(),
            input_schema: json!({"type":"object","properties":{
                "bundle_id":{"type":"string","description":"App User Model ID (AUMID) for a packaged app — pattern `{PackageFamilyName}!{ApplicationId}`, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`. Falls back to a `name` alias if no `!` is present. Either bundle_id, name, aumid, path, or launch_path must be provided."},
                "aumid":{"type":"string","description":"Explicit AUMID for a packaged app. Cleaner alternative to overloading `bundle_id`. Takes precedence over `bundle_id` / `name` when present."},
                "name":{"type":"string","description":"App display name. Tried against the `shell:AppsFolder` index first for packaged-app lookup; on a miss, passed to ShellExecuteEx's PATH search."},
                "path":{"type":"string","description":"Full path to executable. Windows-only; takes precedence over name/bundle_id/aumid (but not over `launch_path`)."},
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps`. Highest precedence — when set, this exact string is handed to ShellExecuteEx unchanged. For Windows desktop apps it's the full `.exe` commandline (path + arguments preserved from the source shortcut); for UWP apps it's `shell:appsFolder\\{PackageFamilyName}!{AppId}`. For precise UWP pid capture, prefer `aumid` over `launch_path`."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open in the default browser via ShellExecuteEx (no activation)."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process (or activation arguments for packaged apps)."},
                "electron_debugging_port":{"type":"integer","description":"Accepted for cross-platform parity; currently no-op on Windows."},
                "webkit_inspector_port":{"type":"integer","description":"Accepted for cross-platform parity; no-op on Windows."},
                "creates_new_application_instance":{"type":"boolean","description":"Accepted for parity; no-op on Windows (ShellExecuteEx always creates a new process)."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let launch_path_opt = args.get("launch_path").and_then(|v| v.as_str()).map(str::to_owned);
        let path_opt = args.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        let name_opt = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let bundle_id_opt = args.get("bundle_id").and_then(|v| v.as_str()).map(str::to_owned);
        let aumid_opt = args.get("aumid").and_then(|v| v.as_str()).map(str::to_owned);
        let urls: Vec<String> = args.get("urls").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        // Capture the foreground window BEFORE any launch path runs so the
        // post-spawn polling restore (below) has a target HWND to flip back
        // to. Mirrors the macOS `FocusRestoreGuard` behavior. Best-effort —
        // see `restore_foreground_polling_best_effort` for the actual
        // restoration semantics.
        //
        // The UWP/AUMID path already restores foreground synchronously
        // (see `launch_uwp::restore_foreground_best_effort`), so this only
        // gates the legacy ShellExecuteExW path. We capture early so the
        // value is consistent regardless of which sub-path runs.
        //
        // Stored as `usize` (not `HWND`) so this `invoke` future stays
        // `Send` — `HWND` wraps `*mut c_void` and would taint the future
        // for the multi-threaded tokio runtime if held across `.await`.
        let foreground_before_addr: usize = unsafe {
            windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
        };

        // Pure-function decision on whether to fire the post-launch
        // restore. Shape mirrors the resolved params — see
        // `should_restore_foreground_after_launch` for the rule.
        let launch_shape = LaunchTargetShape {
            has_aumid:       aumid_opt.is_some(),
            has_bundle_id:   bundle_id_opt.is_some(),
            has_name:        name_opt.is_some(),
            has_path:        path_opt.is_some(),
            has_launch_path: launch_path_opt.is_some(),
            has_urls:        !urls.is_empty(),
        };
        let restore_foreground = should_restore_foreground_after_launch(launch_shape);
        let mut extra_args: Vec<String> = args.get("additional_arguments").and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
            .unwrap_or_default();

        // Resolve target — launch_path > path > aumid > name > bundle_id
        // (alias of name on Windows). launch_path is highest precedence for
        // round-trip with list_apps; the value is handed to ShellExecuteEx
        // unchanged (UWP routing below is bypassed when launch_path is set).
        let target = launch_path_opt.clone()
            .or(path_opt.clone())
            .or(aumid_opt.clone())
            .or(name_opt.clone())
            .or(bundle_id_opt.clone());

        // #1620: auto-inject Chromium anti-throttling flags when the resolved
        // target names a Chromium-based browser (Edge / Chrome / Brave /
        // Vivaldi / Opera / Chromium / Arc / Thorium / Iridium / Yandex).
        // launch_app hides the window via SW_SHOWNOACTIVATE; Chromium's
        // occlusion logic suspends the renderer in that case, leaving the
        // UIA tree empty and PrintWindow capturing a blank body. The three
        // flags below defeat that for the lifetime of the launched process
        // without affecting normal interactive use.
        //
        // Skipped when `aumid` or `bundle_id` route through the UWP path
        // (packaged Edge is the exception; the desktop Edge channel is what
        // ships now and that goes through the ShellExecuteEx path below).
        if launch_path_opt.is_some() || path_opt.is_some() || name_opt.is_some() {
            if let Some(t) = target.as_deref() {
                if is_chromium_browser_target(t) {
                    inject_chromium_anti_throttling_flags(&mut extra_args);
                }
            }
        }

        if target.is_none() && urls.is_empty() {
            // Error message lists every field the resolver actually accepts.
            // The Swift-original "bundle_id or name" message predated the Windows
            // additions (aumid, path, launch_path, urls) and made #1635 look like
            // an aumid-specific bug. The actual cause of #1635 was the upstream
            // PS argv quote-stripping bug fixed in #1637; this message just stops
            // misleading anyone who hits the error for unrelated reasons.
            return ToolResult::error(
                "Provide one of: bundle_id, name, aumid, path, launch_path, or urls to identify the app to launch.",
            );
        }

        // ── Packaged-app (UWP / MSIX) routing decision ──────────────────────
        //
        // Routing precedence — most explicit signal wins:
        //   1. `launch_path` set: skip UWP routing entirely — round-trip
        //      semantics demand the value be handed to ShellExecuteEx
        //      unchanged. Callers wanting precise UWP pid capture should
        //      pass `aumid` instead.
        //   2. `aumid` parameter (any value): packaged path, AUMID = value.
        //   3. `bundle_id` containing `!`: packaged path, AUMID = value
        //      (Win32 PATH lookups never produce a `!` in the executable
        //      name, so this is a safe marker of caller intent).
        //   4. `name` with no `path`: try `shell:AppsFolder` lookup; on a
        //      hit, packaged path with the resolved AUMID. On miss, fall
        //      through to ShellExecuteExW (existing Win32 path).
        //   5. Anything else: existing ShellExecuteExW path.
        //
        // `path` is intentionally excluded from packaged routing — when
        // the caller has a concrete executable in mind they want
        // CreateProcess semantics, not Start Menu activation.
        let aumid_for_uwp: Option<String> = if launch_path_opt.is_some() || path_opt.is_some() {
            None
        } else if let Some(a) = aumid_opt.clone() {
            Some(a)
        } else if let Some(b) = bundle_id_opt.clone().filter(|s| crate::launch_uwp::is_aumid(s)) {
            Some(b)
        } else if let Some(n) = name_opt.clone() {
            // Run the (cached) AppsFolder lookup on a blocking thread to
            // avoid stalling the async runtime on the cold-enumeration
            // path (~200 ms first call, ~µs after).
            tokio::task::spawn_blocking(move || crate::launch_uwp::resolve_aumid_by_name(&n))
                .await
                .unwrap_or(None)
        } else {
            None
        };

        // Single launch path. `display` is the human-readable identifier
        // we report in the success header; for the packaged path it's
        // the AUMID, which is more informative than the display name.
        let display = aumid_for_uwp
            .clone()
            .or_else(|| target.clone())
            .unwrap_or_else(|| urls.join(", "));

        // When the resolved target came from `launch_path` it may carry
        // shortcut arguments (e.g. `"C:\Path\chrome.exe" --profile-directory="Profile 2"`).
        // Split it into a file token + arguments tail so ShellExecuteEx
        // sees them separately — passing the whole string as lpFile would
        // fail because the shell only resolves a single filesystem path
        // there. No-op for AUMID tokens / plain paths without args / UWP
        // `shell:appsFolder\..!..` tokens.
        let (target_file_opt, extra_joined) = {
            let base_extra = extra_args.join(" ");
            if launch_path_opt.is_some() {
                if let Some(t) = target.as_deref() {
                    let (file, args_tail) = split_launchable_target(t);
                    let combined = if args_tail.is_empty() {
                        base_extra
                    } else if base_extra.is_empty() {
                        args_tail
                    } else {
                        format!("{args_tail} {base_extra}")
                    };
                    let file_opt = if file.is_empty() { None } else { Some(file) };
                    (file_opt, combined)
                } else {
                    (None, base_extra)
                }
            } else {
                (target.clone(), base_extra)
            }
        };

        // Branch: AUMID activation if we resolved one; else legacy
        // ShellExecuteExW. Both branches still need to handle `urls`
        // (additional URLs always go through ShellExecuteExW since the
        // Start Menu doesn't activate by URL).
        let pid = if let Some(aumid) = aumid_for_uwp.clone() {
            let aumid_clone = aumid.clone();
            let args_clone = extra_joined.clone();
            let activation = tokio::task::spawn_blocking(move || {
                crate::launch_uwp::launch_uwp(&aumid_clone, &args_clone)
            })
            .await;
            match activation {
                Ok(Ok(p)) => p,
                Ok(Err(e)) => return ToolResult::error(format!(
                    "Failed to activate packaged app {aumid:?}: {e}"
                )),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Legacy ShellExecuteExW path — unchanged behavior for plain
            // Win32 apps and for callers passing an explicit `path` /
            // `launch_path`.
            let urls_clone = urls.clone();
            let target_for_shell = target_file_opt.clone();
            let extra_for_shell = extra_joined.clone();
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
                let file_w = to_wide(if let Some(t) = target_for_shell.as_deref() {
                    t
                } else {
                    urls_clone.first().map(|s| s.as_str()).unwrap_or("")
                });
                let args_w = to_wide(&extra_for_shell);

                let mut info = SHELLEXECUTEINFOW {
                    cbSize:       std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                    fMask:        SEE_MASK_NOCLOSEPROCESS,
                    lpVerb:       PCWSTR(op_w.as_ptr()),
                    lpFile:       PCWSTR(file_w.as_ptr()),
                    lpParameters: if extra_for_shell.is_empty() {
                        PCWSTR::null()
                    } else {
                        PCWSTR(args_w.as_ptr())
                    },
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

            match result {
                Ok(Ok(p))  => p,
                Ok(Err(e)) => return ToolResult::error(format!("Failed to launch: {e}")),
                Err(e)     => return ToolResult::error(format!("Task error: {e}")),
            }
        };

        // Best-effort foreground-restore for the legacy ShellExecuteExW
        // path. The UWP/AUMID branch already restores foreground
        // synchronously inside `launch_uwp::launch_uwp`, so we only spawn
        // the polling task when we went through the ShellExecuteExW
        // branch. URLs-only invocations are excluded via `restore_foreground`
        // (see `should_restore_foreground_after_launch`).
        //
        // `HWND` wraps a raw `*mut c_void` which is `!Send`, so we marshal
        // the foreground handle across the spawn boundary as a `usize` and
        // reconstruct the `HWND` only inside the synchronous probes (see
        // `restore_foreground_polling_best_effort`). Mirrors how
        // `launch_uwp.rs` carries its restore target.
        if aumid_for_uwp.is_none() && restore_foreground {
            let spawned_pid_for_restore = pid;
            let target_foreground_addr = foreground_before_addr;
            tokio::spawn(async move {
                restore_foreground_polling_best_effort(
                    target_foreground_addr,
                    spawned_pid_for_restore,
                )
                .await;
            });
        }

        // When the packaged path was taken we still need to fan out any
        // extra URLs to the default browser (ShellExecuteEx — no pid
        // capture, mirroring Swift's NSWorkspace secondary-URL flow).
        if aumid_for_uwp.is_some() && !urls.is_empty() {
            let urls_clone = urls.clone();
            let _ = tokio::task::spawn_blocking(move || {
                use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
                use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE;
                use windows::core::PCWSTR;
                fn to_wide(s: &str) -> Vec<u16> {
                    s.encode_utf16().chain(std::iter::once(0)).collect()
                }
                let op_w = to_wide("open");
                for url in &urls_clone {
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
            })
            .await;
        }

        // Resolve the pid's windows so the caller can skip a list_windows
        // round-trip — same approach as Swift's `resolveWindows`. Retry
        // 5×200ms; Win32 window registration can lag the launch.
        //
        // Launcher-stub fallback: when the launched binary is a wrapper that
        // re-execs and exits (GIMP's `gimp-3.exe` → `gimp-3.2.exe`; LO's
        // `swriter.exe` → `soffice.bin`), the launched pid never gets a
        // window — list_windows(Some(pid)) stays empty forever. After the
        // primary retry budget we fall back to scanning the launched pid's
        // descendant + name-related processes and pick the first one with a
        // window. The resolved pid is reflected in the response's `pid` field
        // so callers can target it with subsequent calls. See #1615.
        let mut windows_json: Vec<serde_json::Value> = Vec::new();
        let mut resolved_pid: u32 = pid;
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
        if windows_json.is_empty() {
            // Launcher-stub fallback. Compute the exe basename from the
            // launchable target so name-based matching has something to work
            // with (e.g. "gimp-3.exe" → prefix "gimp" matches "gimp-3.2.exe").
            let basename_for_match = target_file_opt
                .as_deref()
                .and_then(|t| t.rsplit(|c: char| c == '\\' || c == '/').next())
                .unwrap_or("")
                .to_owned();
            // Known-slow launchers get an extended retry budget. GIMP 3.x in
            // particular spends 10-20s on its first launch (font cache rebuild,
            // plugin scan, etc.) before the main window registers. We don't
            // want to wait 20s for every launcher — gate on basename prefix
            // matching known-slow apps. Add to this list as encountered.
            let bn_lower = basename_for_match.to_ascii_lowercase();
            let is_slow_launcher = bn_lower.starts_with("gimp")
                || bn_lower.starts_with("blender")          // OpenGL init can stall
                || bn_lower.starts_with("inkscape")         // similar GTK pattern
                || bn_lower.starts_with("krita")
                || bn_lower.starts_with("freecad");
            let max_candidate_attempts: usize = if is_slow_launcher { 30 } else { 3 };

            let basename_clone = basename_for_match.clone();
            let candidates_initial = tokio::task::spawn_blocking(move || {
                crate::win32::related_processes(pid, &basename_clone)
            })
            .await
            .unwrap_or_default();

            // For slow launchers we may also need to RE-SCAN candidates over
            // time, because the wrapper may not have spawned its child yet
            // when we first scanned. Cap total wait at ~12s (slow) / 0.6s (fast).
            let mut tried: std::collections::HashSet<u32> = std::collections::HashSet::new();
            tried.insert(pid); // already tried in the primary loop
            let mut candidate_queue: Vec<u32> = candidates_initial
                .into_iter()
                .filter(|p| tried.insert(*p))
                .collect();
            let mut total_attempts: usize = 0;
            'outer: loop {
                while let Some(candidate_pid) = candidate_queue.pop() {
                    for _ in 0..max_candidate_attempts {
                        total_attempts += 1;
                        let wins = tokio::task::spawn_blocking(move || {
                            crate::win32::list_windows(Some(candidate_pid))
                        })
                        .await
                        .unwrap_or_default();
                        if !wins.is_empty() {
                            windows_json = wins.iter().map(|w| json!({
                                "window_id": w.hwnd, "title": w.title,
                                "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                                "layer": 0,
                                "z_index": 0,
                                "is_on_screen": true,
                            })).collect();
                            resolved_pid = candidate_pid;
                            break 'outer;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
                // For slow launchers, keep re-scanning descendants — the
                // wrapper may not have spawned its child yet. Cap total
                // wait at ~12s (60 × 200ms) for the slow path.
                if !is_slow_launcher || total_attempts > 60 { break; }
                // Give the wrapper a moment to spawn before re-scanning.
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                total_attempts += 3; // count the 500ms wait as 3 attempts
                let basename_rescan = basename_for_match.clone();
                let fresh = tokio::task::spawn_blocking(move || {
                    crate::win32::related_processes(pid, &basename_rescan)
                })
                .await
                .unwrap_or_default();
                let new_ones: Vec<u32> = fresh.into_iter().filter(|p| tried.insert(*p)).collect();
                candidate_queue = new_ones;
                // Continue the outer loop regardless — even with empty
                // new_ones we want another iteration that hits the
                // total_attempts cap. The loop body handles the empty queue
                // by falling through to the re-scan again.
            }
        }
        let pid = resolved_pid;

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
        // `bundle_id` is the AUMID when we went through the packaged-app
        // path (so the caller can round-trip the same value to relaunch),
        // and `null` for plain Win32 launches (which truly have no bundle
        // identifier concept on Windows).
        let bundle_id_response = match aumid_for_uwp.as_ref() {
            Some(a) => serde_json::Value::String(a.clone()),
            None => serde_json::Value::Null,
        };
        let structured = json!({
            "pid":       pid,
            "bundle_id": bundle_id_response,
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
            description: "Left-click against a target pid. **Prefer `element_index` over \
                pixel coordinates** — element_index works on backgrounded / minimized / \
                hidden / off-desktop windows, surfaces a stable handle that survives \
                rebuilds, and tells you what you're clicking via the cached element's \
                role + label. Reach for `x, y` only when the target is a canvas / video / \
                WebGL / custom-drawn surface that doesn't appear in the UIA tree.\n\n\
                Two addressing modes:\n\n\
                - `element_index` + `window_id` (from the last `get_window_state` snapshot \
                of that window) — performs the UIA Invoke pattern on the cached element via \
                PostMessage. No cursor move, no focus steal. Requires a prior \
                `get_window_state(pid, window_id)` in this turn; the element_index cache \
                is scoped per (pid, window_id) and is replaced by the next snapshot of the \
                same window.\n\n\
                - `x`, `y` (window-local screenshot pixels, top-left origin of the PNG \
                returned by `get_window_state`) — synthesizes mouse events via \
                PostMessage(WM_LBUTTONDOWN/UP) and delivers them to the deepest child \
                window under that point. `count: 2` posts two down/up pairs for a \
                double-click. Pixel clicks need a visible on-screen window to anchor the \
                coordinate conversion (errors with `pid X has no on-screen window` otherwise).\n\n\
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
        use mcp_server::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let hwnd_opt = args.opt_u64("window_id");
        let elem_idx = args.opt_u64("element_index").map(|v| v as usize);
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        let button = args.str_or("button", "left");
        let count = args.u64_or("count", 1) as usize;

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
            // Step 3: click pulse + actual click.
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse {
                x: cx as f64, y: cy as f64,
            });
            let btn = button.clone();
            // Try UIA Invoke first (it works for UWP / modern XAML / web
            // content where PostMessage(WM_LBUTTONDOWN) hits the outer
            // HWND but never reaches the inner XAML/composition element).
            // Fall through to PostMessage when:
            //   - element doesn't support InvokePattern (most text inputs,
            //     non-interactive elements)
            //   - right-click (UIA's ExpandCollapse / ShowContextMenu are
            //     different patterns; keep PostMessage for now)
            //   - count > 1 (double-click semantics aren't an Invoke
            //     concept — PostMessage produces the actual WM_LBUTTONDBLCLK)
            let state_clone = self.state.clone();
            let use_uia_invoke = (btn == "left" || btn == "middle") && count == 1;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                if use_uia_invoke {
                    if let Some(ptr) = state_clone.element_cache.get_element_ptr(pid, hwnd, idx) {
                        use windows::Win32::UI::Accessibility::{
                            IUIAutomationElement, IUIAutomationInvokePattern, UIA_InvokePatternId,
                        };
                        use windows::core::Interface;
                        // Reconstruct without consuming the cache's AddRef;
                        // forget after we extract the pattern so Drop doesn't
                        // double-Release the cached pointer.
                        let elem: IUIAutomationElement =
                            unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
                        let invoke_result = unsafe { elem.GetCurrentPattern(UIA_InvokePatternId) };
                        std::mem::forget(elem);
                        if let Ok(pattern) = invoke_result {
                            if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
                                // UWP foreground-steal bypass: wrap Invoke so
                                // XAML hosts don't self-foreground and steal
                                // user focus. See crate::uia::fg_bypass.
                                let invoke_outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize,
                                    || unsafe { inv.Invoke() },
                                );
                                match invoke_outcome {
                                    Ok(()) => {
                                        return Ok(format!(
                                            "✅ Performed UIA Invoke on [{idx}] (screen ({cx},{cy}))."
                                        ));
                                    }
                                    Err(e) => {
                                        tracing::debug!(
                                            target: "click",
                                            "UIA Invoke failed for [{idx}]: {e}; falling back to PostMessage"
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                // PostMessage fallback (legacy Win32 + non-Invokable elements).
                crate::input::post_click_screen(hwnd, cx, cy, count, &btn)?;
                let action_name = match btn.as_str() {
                    "right"  => "ShowMenu",
                    _        => "PostMessage click",
                };
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
            // Vision-mode (x, y) dispatch is **layered**, mirroring the
            // trope-cua reference impl
            // (`src/CuaDriver.Win/Tools/ClickTool.cs::InvokePixelClickAsync`):
            //
            //   1. UIA hit-test in the target HWND's subtree. If the
            //      deepest InvokePattern-bearing element at (sx, sy) is
            //      found, fire `Invoke()`. Works occluded, no focus
            //      steal, and is the **only** path that lands on UWP /
            //      WebView2 / DirectComposition surfaces — those route
            //      input through `Windows.UI.Input`, not WM_LBUTTONDOWN.
            //
            //   2. If UIA returns false (no Invokable element under the
            //      pixel inside `hwnd`, or `hwnd` has no useful UIA
            //      tree at all), fall through to
            //      `PostMessage(WM_LBUTTONDOWN/UP)` against the deepest
            //      child HWND at the screen point. Covers plain Win32
            //      controls without UIA InvokePattern (most edit
            //      fields, custom-drawn widgets, native containers) and
            //      apps that don't expose UIA at all.
            //
            // The combination closes the gap macOS gets for free from
            // `CGEventPostToPSN` (per-pid event routing): UIA covers
            // UWP-style targets without z-order tricks, PostMessage
            // covers plain Win32 without z-order tricks. Neither path
            // moves the OS cursor or activates the receiver.
            //
            // UIA path runs only for single left/middle clicks —
            // right-click (UIA has no by-coord `ShowContextMenu`) and
            // multi-click (`Invoke()` is single-fire by definition)
            // skip directly to PostMessage.
            let use_uia = (btn == "left" || btn == "middle") && count == 1;
            if use_uia {
                let invoked = tokio::task::spawn_blocking(move || {
                    crate::uia::windows_enum::try_invoke_in_window_at_point(
                        hwnd as isize, sx as i32, sy as i32,
                    )
                })
                .await
                .unwrap_or(false);
                if invoked {
                    return ToolResult::text(format!(
                        "✅ Performed UIA Invoke at ({sx},{sy}) for pid {pid}."
                    ));
                }
            }

            // #1623: PostMessage(WM_LBUTTONDOWN) to Chromium frame HWNDs doesn't
            // reach the DOM input pipeline — Chromium's input thread only accepts
            // events with SendInput-queue origin. For Chromium targets, take the
            // SendInput path; for everything else, take the existing PostMessage
            // path (the path was added as a no-focus-steal click delivery, which
            // PostMessage uniquely provides). The Chromium path moves the cursor
            // visibly and briefly steals foreground — there's no Chromium-native
            // alternative that gets DOM events to fire without these tradeoffs.
            //
            // SendInput on Chromium requires the daemon to have UIAccess integrity
            // (otherwise SetForegroundWindow is rejected and the events land on the
            // wrong window). The MCP proxy auto-prefers the cua-driver-uia worker
            // when both pipes are up, so this path runs with UIAccess in the
            // common case. When UIAccess is missing, send_click_synthesized
            // surfaces an actionable error and we fall through to PostMessage —
            // PostMessage won't fire DOM events on Chromium either, but the user
            // gets a meaningful diagnostic instead of silent no-op.
            let chromium = tokio::task::spawn_blocking(move || {
                crate::input::is_chromium_target_window(hwnd)
            })
            .await
            .unwrap_or(false);
            if chromium {
                // Belt-and-suspenders foreground restore. `send_click_synthesized`
                // does its own SetForegroundWindow(prev_fg) ~40ms after the
                // click, BUT Chromium's reaction to the click can include an
                // async re-activation (focus().activate() in the renderer-side
                // event handler, sometimes 100-500 ms later). The launch_app
                // FocusRestoreGuard (PR #1668) already proved the polling
                // pattern works; reuse the same helper to catch the async case.
                //
                // Captured here (async-runtime side) rather than inside the
                // blocking task so we have a known-stable "before" snapshot.
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, sx as i32, sy as i32, count, &btn)
                })
                .await;
                // Kick off the polling restore regardless of click result
                // — even a partially-inserted click might have started an
                // async activation Chromium can't undo.
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                match send_result {
                    Ok(Ok(())) => {
                        let click_word = match count {
                            2 => "double-click",
                            3 => "triple-click",
                            _ => "click",
                        };
                        return ToolResult::text(format!(
                            "✅ Sent {click_word} via SendInput to pid {pid} (Chromium target)."
                        ));
                    }
                    Ok(Err(e)) => {
                        // Bubble the actionable diagnostic ("Run through uia worker") up
                        // to the caller rather than silently falling to PostMessage,
                        // which we know doesn't work for Chromium.
                        return ToolResult::error(e.to_string());
                    }
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                }
            }

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
                **Routing on Windows.** When the target's owning EXE or top-level window \
                class identifies it as a XAML / WinUI3 / UWP host (modern Notepad, \
                Calculator, Photos, Settings, etc.), the tool requires `element_index` \
                + `window_id` and routes through UI Automation's `ValuePattern.SetValue` \
                — same backend as the `set_value` tool. PostMessage WM_CHAR doesn't \
                reach those hosts (their CoreInput dispatcher only consumes events from \
                the system input queue), so the fallback path silently dropped chars. \
                If you call `type_text(pid, text)` on a XAML host without `element_index`, \
                the tool returns an actionable error pointing you at `get_window_state` \
                first. Legacy Win32 apps still use the PostMessage path, preserving the \
                no-focus-steal property.\n\n\
                `delay_ms` (0–200, default 30) spaces successive characters on the \
                PostMessage path so autocomplete and IME can keep up. Ignored on the \
                UIA path (SetValue is atomic).".into(),
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
        use mcp_server::tool_args::ArgsExt;
        let raw_pid = match args.require_i64("pid") { Ok(v) => v, Err(e) => return e };
        let pid = raw_pid as u32;
        let text = match args.require_str("text") { Ok(v) => v, Err(e) => return e };
        let hwnd_opt = args.opt_u64("window_id");
        let elem_idx = args.opt_u64("element_index");
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.");
        }
        let _delay_ms = args.u64_or("delay_ms", 30);
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

        // CUA-543 routing: PostMessage WM_CHAR doesn't reach modern
        // XAML / WinUI3 hosts. When the target is one of those AND the
        // caller has supplied an element_index, route through UIA
        // `ValuePattern.SetValue` — same code path the `set_value` tool
        // uses, which we've verified works on modern Notepad / WinUI3.
        // Legacy Win32 stays on the PostMessage path so the no-focus-
        // steal property is preserved.
        if crate::input::is_xaml_host_hwnd(hwnd) {
            if let Some(idx) = elem_idx {
                let idx = idx as usize;
                let state = self.state.clone();
                let text_for_uia = text.clone();
                let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    let ptr = state.element_cache.get_element_ptr(pid, hwnd, idx)
                        .ok_or_else(|| anyhow::anyhow!(
                            "Element {idx} not in cache — call get_window_state(pid={pid}, window_id={hwnd}) first."
                        ))?;
                    use windows::Win32::UI::Accessibility::{
                        IUIAutomationElement, IUIAutomationValuePattern, UIA_ValuePatternId,
                    };
                    use windows::core::{Interface, BSTR};
                    let elem: IUIAutomationElement =
                        unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
                    let pattern = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId)? };
                    std::mem::forget(elem);
                    let vp: IUIAutomationValuePattern = pattern.cast()?;
                    unsafe { vp.SetValue(&BSTR::from(text_for_uia.as_str()))? };
                    Ok(())
                }).await;
                return match result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Wrote {text_len} char(s) on pid {raw_pid} via UIA ValuePattern \
                         (XAML / UWP target, element_index=[{idx}])."
                    )),
                    Ok(Err(e)) => ToolResult::error(format!("type_text (UIA path): {e}")),
                    Err(e)     => ToolResult::error(format!("Task error: {e}")),
                };
            } else {
                // XAML target without element_index: PostMessage will silently
                // drop chars. Surface a clear error pointing the agent at the
                // right workflow rather than lying with a "✅ Typed" message.
                return ToolResult::error(format!(
                    "type_text on a modern XAML / UWP target (pid {raw_pid}, hwnd {hwnd}) \
                     requires `element_index` — its WM_CHAR pipeline ignores PostMessage \
                     without keyboard focus. Call `get_window_state(pid={raw_pid}, \
                     window_id={hwnd})` to enumerate elements, then re-call \
                     `type_text(pid, window_id, element_index, text)`. Or call \
                     `set_value(pid, window_id, element_index, value)` directly — same \
                     UIA backend."
                ));
            }
        }

        // Legacy Win32 path — PostMessage WM_CHAR, no focus steal.
        let result = tokio::task::spawn_blocking(move || {
            crate::input::post_type_text(hwnd, &text)
        }).await;
        match result {
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
        use mcp_server::tool_args::ArgsExt;
        let raw_pid = match args.require_i64("pid") { Ok(v) => v, Err(e) => return e };
        let pid = raw_pid as u32;
        let key = match args.require_str("key") { Ok(v) => v, Err(e) => return e };
        let mods: Vec<String> = args.str_array("modifiers");
        let hwnd_opt = args.opt_u64("window_id");
        // Swift requires window_id when element_index is supplied — ports the
        // same validation.
        let elem_idx = args.opt_u64("element_index");
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
            // Description ported from Swift `HotkeyTool.swift` with the
            // Windows-specific dispatch (UIA accelerator for XAML; SendInput
            // for legacy Win32 with modifiers per #1614 / #1618; PostMessage
            // for modifier-less keys).
            description: "Press a combination of keys simultaneously — e.g. `[\"ctrl\", \"c\"]` \
                for Copy, `[\"ctrl\", \"shift\", \"t\"]` for reopen-closed-tab. Dispatch is \
                target-aware:\n\n\
                - **Modern XAML / WinUI / UWP targets** route through UI Automation: the driver \
                walks the target's accessibility subtree, finds a descendant whose AcceleratorKey \
                matches the combo, and invokes it. No focus steal, no system-queue input.\n\n\
                - **Legacy Win32 targets with modifiers** (Ctrl+S, Alt+Tab, etc.) route through \
                `SendInput` against the system input queue, with a brief `SetForegroundWindow` \
                swap so the events land on the target. This path is necessary because \
                PostMessage(WM_KEYDOWN, VK_CONTROL) does NOT update the OS-wide modifier state \
                visible to `GetKeyState` / `TranslateAccelerator`, so Win32 apps that bind \
                accelerators via `TranslateAccelerator` (LibreOffice, FAR, classic Notepad, etc.) \
                never see the combo as a real accelerator on the PostMessage-only path. \
                Trade-off: brief foreground swap (mitigated by restoring the previous foreground \
                after the keystrokes flush). Requires the daemon to have UIAccess integrity so \
                `SetForegroundWindow` is permitted — the MCP proxy auto-prefers the \
                `cua-driver-uia.exe` worker pipe when both daemons are running.\n\n\
                - **Legacy Win32 targets without modifiers** (plain `enter`, `tab`, `f5`, etc.) \
                route through `PostMessage(WM_KEYDOWN/UP)` — no focus steal, no need to update \
                modifier state.\n\n\
                The target does NOT need to be frontmost in any branch.\n\n\
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
        use mcp_server::tool_args::ArgsExt;
        let hwnd_opt = args.opt_u64("window_id");
        let raw_pid = match args.require_i64("pid") { Ok(v) => v, Err(e) => return e };
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

        if crate::input::is_xaml_host_hwnd(hwnd) {
            let mut accelerator_keys = mods.clone();
            accelerator_keys.push(key.clone());
            let accelerator_combo = accelerator_keys.join("+");
            let key_display_for_result = key_display.clone();
            // UIA cross-process subtree walks can wedge on a bad provider —
            // bound the call so a hung provider returns an error instead of
            // blocking the daemon indefinitely. 4 s matches the budget the
            // rest of this file uses for similar UIA scans.
            let blocking = tokio::task::spawn_blocking(move || {
                crate::uia::windows_enum::try_invoke_accelerator_in_window(
                    hwnd as isize,
                    &accelerator_combo,
                )
            });
            let result = match tokio::time::timeout(
                std::time::Duration::from_secs(4),
                blocking,
            )
            .await
            {
                Ok(join_result) => join_result,
                Err(_) => {
                    return ToolResult::error(format!(
                        "hotkey timed out after 4s while scanning UIA accelerators on pid \
                         {raw_pid} (hwnd {hwnd}). A UIA provider in this app is likely \
                         unresponsive."
                    ));
                }
            };
            return match result {
                Ok(Ok((true, _))) => ToolResult::text(format!(
                    "✅ Pressed {key_display_for_result} on pid {raw_pid} via UIA \
                     InvokePattern/TogglePattern (XAML / UWP target)."
                )),
                Ok(Ok((false, scanned))) => ToolResult::error(format!(
                    "hotkey on a modern XAML / UWP target (pid {raw_pid}, hwnd {hwnd}) \
                     could not find a UIA AcceleratorKey or `(Ctrl+X)`-style name hint \
                     matching `{key_display_for_result}` (scanned {scanned} element(s)). \
                     PostMessage WM_KEYDOWN/UP is ignored by this target's input pipeline. \
                     Common cause: the action is nested behind a closed menu (e.g. modern \
                     Notepad's Save under the File menu) — its element isn't in the visible \
                     subtree until the menu is opened. Call \
                     `get_window_state(pid={raw_pid}, window_id={hwnd})` to inspect available \
                     UIA actions, then use an exposed accelerator or `click` the matching \
                     element directly."
                )),
                Ok(Err(e)) => ToolResult::error(format!("hotkey (UIA path): {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Non-XAML target. Three routing decisions:
        //   1. PostMessage WM_KEYDOWN/UP — no foreground swap, no focus
        //      theft. Modifier state is encoded in the LPARAM bits, not
        //      in GetKeyState, so apps that read modifier state directly
        //      from the message (Chromium's `Browser::HandleKeyboardEvent`,
        //      Chromium renderer's input handler, every web app) see
        //      Ctrl+X correctly. Apps that consult `GetKeyState` via
        //      `TranslateAccelerator` (LibreOffice, FAR, classic Notepad)
        //      DON'T fire the accelerator — the modifier looks unset to
        //      them.
        //   2. SendInput synthesized hotkey — pushes events onto the
        //      *system input queue*. Updates GetKeyState system-wide, so
        //      TranslateAccelerator-based apps see Ctrl+S as a real
        //      accelerator. Trade-off: brief foreground swap so SendInput
        //      lands on the right HWND. **This is the only path that
        //      violates the no-foreground contract** — we minimise its
        //      use to apps that genuinely require it.
        //
        // Routing:
        //   - No modifiers → PostMessage (no accelerator concern).
        //   - Modifiers + Chromium-family target → PostMessage. Chromium
        //     processes accelerators in `Browser::HandleKeyboardEvent`,
        //     which reads modifier state from the WM_KEYDOWN LPARAM
        //     directly (not via GetKeyState). Every Chromium browser
        //     (Chrome/Edge/Brave/Arc/Vivaldi) AND every Electron app
        //     (Slack/VS Code/Discord/Teams/Notion) detects as Chromium
        //     via `is_chromium_target_window` and gets the no-foreground
        //     path.
        //   - Modifiers + non-Chromium Win32 → SendInput (the
        //     TranslateAccelerator path). The foreground swap stays.
        let has_modifiers = !mods.is_empty();
        let is_chromium = crate::input::is_chromium_target_window(hwnd);
        let use_send_input = has_modifiers && !is_chromium;
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            if use_send_input {
                crate::input::send_key_synthesized(hwnd, &key, &m)
            } else {
                crate::input::post_key(hwnd, &key, &m)
            }
        }).await;
        let path = if use_send_input { "SendInput" } else { "PostMessage" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Pressed {key_display} on pid {raw_pid} via {path} \
                 (Win32 target)."
            )),
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
        use mcp_server::tool_args::ArgsExt;
        let by = args.str_or("by", "line");
        let direction_display = direction.clone();
        let by_display = by.clone();
        let amount = args.u64_or("amount", 3).clamp(1, 50) as u32;
        let hwnd_opt = args.opt_u64("window_id");
        let elem_idx = args.opt_u64("element_index");
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
                (default `jpeg`, quality `85`). The long edge is downscaled to fit the \
                `max_image_dimension` config (default `1568` px — matches Anthropic's \
                multimodal-vision input size, so a click-coord picked off this PNG addresses \
                the same pixels the model reasoned over).\n\n\
                **Prefer `get_window_state` for UI work** — it returns the UIA tree alongside \
                the same screenshot in one call, populates the element_index cache the click / \
                type_text / scroll tools resolve against, and is the only path to backgrounded \
                accessibility actions. `screenshot` is for when you just need pixels (vision \
                grounding, debugging, attaching to a report).\n\n\
                `window_id` is recommended (use `list_windows` to find it). When omitted, \
                captures the full primary display — a Windows-only convenience for whole-\
                screen snapshots without a per-window target.\n\n\
                Requires no special permissions on Windows.".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "window_id":{"type":"integer","description":"HWND of the window to capture. When omitted, captures the full primary display (Windows-only)."},
                    "format":{"type":"string","enum":["png","jpeg"],"description":"Image format. Default: jpeg."},
                    "quality":{"type":"integer","minimum":1,"maximum":95,
                        "description":"JPEG quality 1-95; ignored for png. Default: 85."}
                },"additionalProperties":false
            }),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use mcp_server::tool_args::ArgsExt;
        let hwnd_opt = args.opt_u64("window_id");
        let format = args.str_or("format", "jpeg");
        let quality = args.u64_or("quality", 85) as u8;
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

// ── screenshot (Claude Code computer-use compat) ─────────────────────────────
//
// Drop-in replacement for `ScreenshotTool` selected via the
// `--claude-code-computer-use-compat` flag. Differences:
//   - `pid` AND `window_id` BOTH required (the regular tool makes window_id
//     optional and falls back to whole-display capture).
//   - Validates the target window belongs to that pid and is visible.
//   - Always returns JPEG @ 85% (Claude Code vision flows prefer smaller).
//   - Includes a follow-up text note pointing the caller at pixel-addressed
//     tools so the LLM uses the window's coordinate space.
//
// Mirrors libs/cua-driver/swift/Sources/CuaDriverServer/
// ClaudeCodeComputerUseCompatTools.swift's `screenshot`. Same name as the
// regular tool — `build_registry(compat)` chooses which to register; both
// are never registered together.

pub struct ScreenshotCompatTool {
    state: Arc<ToolState>,
}
static SCREENSHOT_COMPAT_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ScreenshotCompatTool {
    fn def(&self) -> &ToolDef {
        SCREENSHOT_COMPAT_DEF.get_or_init(|| ToolDef {
            name: "screenshot".into(),
            description:
                "Capture a target window and return a JPEG image. Coordinates accepted by \
                 CuaDriver's pixel tools are pixels in this window screenshot's coordinate space.\n\n\
                 This is the compatibility anchor for Claude Code vision flows: CuaDriver remains \
                 window-scoped, and all other tools are the normal CuaDriver tools.".into(),
            input_schema: json!({
                "type": "object",
                "required": ["pid", "window_id"],
                "properties": {
                    "pid":       {"type":"integer","description":"Target process ID from list_windows or launch_app."},
                    "window_id": {"type":"integer","description":"Target HWND from list_windows or launch_app."}
                },
                "additionalProperties": false
            }),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use mcp_server::tool_args::ArgsExt;
        let pid       = match args.require_u32("pid")       { Ok(v) => v, Err(e) => return e };
        let window_id = match args.require_u64("window_id") { Ok(v) => v, Err(e) => return e };

        // Validate: window must exist, belong to pid, and be visible.
        let window = tokio::task::spawn_blocking(move || {
            crate::win32::list_windows(Some(pid))
                .into_iter()
                .find(|w| w.hwnd == window_id && w.width > 1 && w.height > 1)
        }).await.unwrap_or(None);

        let window = match window {
            Some(w) => w,
            None => return ToolResult::error(format!(
                "No visible window {window_id} found for pid {pid}. \
                 Use list_windows to choose an on-screen target window."
            )),
        };

        let max_dim = self.state.config.read().unwrap().max_image_dimension;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(String, u32, u32)> {
            let raw = crate::capture::screenshot_window_bytes(window_id)?;
            let png_bytes = crate::capture::resize_png_if_needed(&raw, max_dim)?;
            let (w, h) = crate::capture::png_dimensions_pub(&png_bytes)?;
            let jpeg = crate::capture::png_bytes_to_jpeg(&png_bytes, 85)?;
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            Ok((B64.encode(&jpeg), w, h))
        }).await;

        match result {
            Ok(Ok((b64, w, h))) => {
                let title = if window.title.is_empty() { "(untitled)".into() } else { window.title };
                let summary = format!(
                    "Captured window screenshot {w}x{h} for {title} \
                     [pid: {pid}, window_id: {window_id}]. \
                     Use CuaDriver pixel tools with this window-local coordinate space."
                );
                ToolResult {
                    content: vec![
                        mcp_server::protocol::Content::image_jpeg(b64),
                        mcp_server::protocol::Content::text(summary),
                    ],
                    is_error: None,
                    structured_content: Some(json!({
                        "pid": pid, "window_id": window_id,
                        "width": w, "height": h, "format": "jpeg"
                    })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Screenshot failed: {e}")),
            Err(e)     => ToolResult::error(format!("Task error: {e}")),
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
        use mcp_server::tool_args::ArgsExt;
        let hwnd_opt = args.opt_u64("window_id");
        let elem_idx = args.opt_u64("element_index").map(|v| v as usize);
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
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
        use mcp_server::tool_args::ArgsExt;
        let hwnd_opt = args.opt_u64("window_id");
        let elem_idx = args.opt_u64("element_index").map(|v| v as usize);
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
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

        use mcp_server::tool_args::ArgsExt;
        // Accepts numeric JSON as either float or integer — coerce both to f64.
        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key).or_else(|| args.opt_i64(key).map(|i| i as f64))
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

        let hwnd_opt    = args.opt_u64("window_id");
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps       = args.u64_or("steps", 20) as usize;
        let button      = args.str_or("button", "left");
        let from_zoom   = args.bool_or("from_zoom", false);

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
        use mcp_server::tool_args::ArgsExt;
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        let cursor_id_owned = args.str_or("cursor_id", "default");
        let cursor_id = cursor_id_owned.as_str();
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
        use mcp_server::tool_args::ArgsExt;
        let cursor_id_owned = args.str_or("cursor_id", "default");
        let cursor_id = cursor_id_owned.as_str();
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
        // `elevated` reports the daemon's actual token integrity level, NOT the
        // legacy `TokenIsElevated` UAC-elevation flag. The two diverge in two
        // important cases that matter to cua-driver:
        //
        //   1. RID-500 built-in Administrator — runs at High IL all the time
        //      because RID 500 has no UAC split token (FilterAdministratorToken=0
        //      default). `TokenIsElevated` returns false (never UAC-prompted)
        //      but the IL is High and admin-level access works fine.
        //
        //   2. Task-Scheduler-spawned processes registered at RunLevel=Highest
        //      and triggered by AtLogon — Task Scheduler hands out the user's
        //      full admin token without going through the UAC prompt path, so
        //      `TokenIsElevated` returns false while IL is High. This is the
        //      common case for cua-driver's autostart task on non-RID-500
        //      admin users — the daemon IS at High IL (cross-AppContainer UIA
        //      works, Calculator-class UWP apps drive cleanly) but the legacy
        //      flag reports it as a "standard user", which confused everyone
        //      tracking #1602 / #1601. See issue #1640.
        //
        // #1646 / dogfood-iteration-3: PR #1647's in-process windows-rs
        // implementation of TokenIntegrityLevel STILL misreported the IL.
        // Verified externally on the cuademo dogfood VM: a daemon at
        // demonstrably High IL (RID 0x3000 by direct PowerShell-C# Win32
        // read against the daemon's pid) was reported by the daemon's own
        // check_permissions as Medium IL (RID 0x2000). Both code paths used
        // identical Win32 APIs (OpenProcess + OpenProcessToken +
        // GetTokenInformation(TokenIntegrityLevel=25)), yet they disagreed.
        // Root cause unclear — possibly a subtle interaction between the
        // windows-rs crate's bindings and the named-pipe server's thread
        // security context, possibly a Vec-buffer alignment issue that
        // doesn't reproduce in standalone testing. RevertToSelf +
        // OpenProcess(GetCurrentProcessId()) didn't help.
        //
        // Pragmatic workaround: shell out to PowerShell and run the same
        // C# Win32 helper that's verified to return the correct IL. Cost
        // is one PowerShell spawn (~100-200ms) per check_permissions call,
        // which is acceptable — check_permissions is a low-frequency,
        // user-initiated diagnostic, not a hot path.
        const HIGH_IL_RID: u32 = 0x3000;

        let il_rid: Option<u32> = read_self_integrity_level_via_powershell();

        let is_elevated = il_rid.map(|r| r >= HIGH_IL_RID).unwrap_or(false);
        let il_name = match il_rid {
            Some(0x0000) => "Untrusted",
            Some(0x1000) => "Low",
            Some(0x2000) => "Medium",
            Some(0x2100) => "Medium+",
            Some(0x3000) => "High",
            Some(0x4000) => "System",
            Some(_) => "Unknown",
            None => "Unavailable",
        };
        // Distinguish the unavailable case from a real measurement — don't
        // render a fake RID 0x0000 or a fake elevation status when the lookup
        // failed. See CodeRabbit review on PR #1641.
        let status_text = match il_rid {
            Some(rid) => format!(
                "Process integrity level: {il_name} (RID 0x{rid:04X}, {})\n\
                 UIA accessibility: available (no special permission needed)\n\
                 PostMessage injection: available",
                if is_elevated { "elevated" } else { "non-elevated" }
            ),
            None => format!(
                "Process integrity level: {il_name} (token query failed)\n\
                 UIA accessibility: available (no special permission needed)\n\
                 PostMessage injection: available"
            ),
        };
        ToolResult::text(status_text)
            .with_structured(json!({
                "elevated": is_elevated,
                "integrity_level": il_name,
                "integrity_level_rid": il_rid,
                "uia": true,
                "post_message": true
            }))
    }
}

/// Read the current process's token integrity level (RID) by shelling out
/// to PowerShell and running a P/Invoke-based C# helper. See the long
/// comment in `CheckPermissionsTool::invoke` for why the in-process
/// windows-rs approach was abandoned (issue #1646 / dogfood iteration 3).
///
/// Returns `Some(rid)` on success (e.g. `0x3000` for High), `None` if
/// PowerShell exits non-zero or stdout isn't a u32. Spawns one short-lived
/// `powershell.exe` process per call; only used from `check_permissions`,
/// which is a low-frequency, user-initiated diagnostic.
fn read_self_integrity_level_via_powershell() -> Option<u32> {
    let pid = std::process::id();
    // Inline the same C# helper we verified externally on the dogfood VM.
    // Heredoc-wrap (`@" ... "@`) avoids shell-escape acrobatics around the
    // C# braces / double-quotes. The `Write-Output` at the end is parsed
    // as a plain integer.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class IL {{
    [DllImport("kernel32.dll")] public static extern IntPtr OpenProcess(int a, bool i, int p);
    [DllImport("kernel32.dll")] public static extern bool CloseHandle(IntPtr h);
    [DllImport("advapi32.dll")] public static extern bool OpenProcessToken(IntPtr p, int a, out IntPtr t);
    [DllImport("advapi32.dll")] public static extern bool GetTokenInformation(IntPtr t, int c, IntPtr i, int l, out int n);
    [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthority(IntPtr s, int i);
    [DllImport("advapi32.dll")] public static extern IntPtr GetSidSubAuthorityCount(IntPtr s);
    public static uint? Rid(int pid) {{
        var h = OpenProcess(0x1000, false, pid); if (h == IntPtr.Zero) return null;
        try {{ IntPtr t; if (!OpenProcessToken(h, 0x8, out t)) return null;
            try {{ int n; GetTokenInformation(t, 25, IntPtr.Zero, 0, out n); if (n == 0) return null;
                var b = Marshal.AllocHGlobal(n); try {{ if (!GetTokenInformation(t, 25, b, n, out n)) return null;
                    var s = Marshal.ReadIntPtr(b); var c = Marshal.ReadByte(GetSidSubAuthorityCount(s));
                    return (uint)Marshal.ReadInt32(GetSidSubAuthority(s, c - 1)); }}
                finally {{ Marshal.FreeHGlobal(b); }} }}
            finally {{ CloseHandle(t); }} }}
        finally {{ CloseHandle(h); }}
    }}
}}
"@
$rid = [IL]::Rid({pid})
if ($null -ne $rid) {{ Write-Output ([int]$rid) }}
"#,
        pid = pid
    );
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.trim().parse::<u32>().ok()
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
        use mcp_server::tool_args::ArgsExt;
        let coerce = |k: &str| args.opt_f64(k).or_else(|| args.opt_i64(k).map(|i| i as f64));
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

pub struct TypeTextCharsTool { _state: Arc<ToolState> }
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
        use mcp_server::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let text = match args.require_str("text") { Ok(v) => v, Err(e) => return e };
        let delay_ms = args.u64_or("delay_ms", 30);
        let hwnd_opt = args.opt_u64("window_id");
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

// ── kill_app ──────────────────────────────────────────────────────────────────
//
// Force-terminate a process by pid. Background:
//   * Win32 / GDI apps respond cleanly to `WM_CLOSE` (Alt+F4 via PostMessage),
//     so `click` on the X button or a hand-rolled hotkey gesture suffices.
//   * UWP / WinUI3 apps (Calculator, Photos, modern Settings, Win11 Notepad)
//     have backgrounding semantics — they accept `WM_CLOSE` but route it
//     into a "minimize to suspended" path instead of exiting. Cooperative
//     close-button automation produces orphan processes that survive every
//     polite eviction attempt. See CUA-541 for the live repro from
//     WINDOWS_CLAUDE_CODE_TEST_PLAN.md Test G (3 leftover CalculatorApp.exe).
//
// `kill_app` is the force-terminate escalation — `taskkill /F` equivalent —
// keyed on pid. Marked `destructive: true` so MCP clients with permission
// gating prompt before invoking.

pub struct KillAppTool;
static KILL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for KillAppTool {
    fn def(&self) -> &ToolDef {
        KILL_DEF.get_or_init(|| ToolDef {
            name: "kill_app".into(),
            description: "Force-terminate a process by pid. Use when the standard close path \
                (Alt+F4 / WM_CLOSE via `click` on the X button) fails to make the process \
                exit — typical for UWP / WinUI3 apps (Calculator, Photos, modern Notepad) \
                that route WM_CLOSE into a suspended-but-resident state. Equivalent to \
                `taskkill /F /PID <pid>`. Unsaved state is lost. Prefer the click-the-X path \
                first; only escalate to `kill_app` when polite close didn't terminate."
                .into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"PID of the process to terminate."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: true,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid_v: u32 = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) if p > 0 && p <= u32::MAX as u64 => p as u32,
            Some(_) => return ToolResult::error("kill_app: `pid` must be a positive integer".to_string()),
            None => return ToolResult::error("kill_app: missing required integer field `pid`".to_string()),
        };

        // Run the syscalls on a blocking thread — TerminateProcess + the
        // WaitForSingleObject confirmation are both blocking, and we don't
        // want to stall the tokio reactor.
        let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
            use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
            use windows::Win32::System::Threading::{
                OpenProcess, TerminateProcess, WaitForSingleObject,
                PROCESS_TERMINATE, PROCESS_SYNCHRONIZE,
            };

            // SAFETY: OpenProcess returns a Result<HANDLE> in windows-rs;
            // PROCESS_TERMINATE | PROCESS_SYNCHRONIZE lets us both kill the
            // target AND wait for the OS to fully reap it (so the caller's
            // follow-up list_apps reflects the change immediately).
            let h = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid_v) }
                .map_err(|e| format!(
                    "OpenProcess(pid={pid_v}, PROCESS_TERMINATE|PROCESS_SYNCHRONIZE) failed: {e}. \
                     The process may not exist, or the daemon lacks the right to terminate it. \
                     Try listing processes first to confirm the pid is correct."
                ))?;

            let term_result = unsafe { TerminateProcess(h, 1) };

            // WaitForSingleObject so the caller can assume the process is
            // really gone on return. 2-second cap — TerminateProcess is
            // synchronous in practice but the OS still needs a moment to
            // reap kernel structures. WAIT_TIMEOUT is treated as a soft
            // success (the kill request landed; the process may take a
            // little longer to disappear from list_apps).
            let wait_status = unsafe { WaitForSingleObject(h, 2000) };
            let _ = unsafe { CloseHandle(h) };

            match (term_result, wait_status) {
                (Ok(()), s) if s == WAIT_OBJECT_0 => Ok(()),
                (Ok(()), _) => Ok(()), // killed, wait timed out — caller can re-check
                (Err(e), _) => Err(format!(
                    "TerminateProcess(pid={pid_v}) failed: {e}. The handle was opened \
                     successfully but the kill itself was rejected; this is unusual for \
                     PROCESS_TERMINATE rights and may indicate a protected-process target \
                     (PPL, e.g. csrss / system services)."
                )),
            }
        }).await;

        match outcome {
            Ok(Ok(())) => ToolResult::text(format!("✅ Terminated pid {pid_v}.")),
            Ok(Err(e)) => ToolResult::error(format!("kill_app: {e}")),
            Err(join_err) => ToolResult::error(format!(
                "kill_app: blocking-task join error: {join_err}"
            )),
        }
    }
}

// ── debug_window_info ─────────────────────────────────────────────────────────
//
// Diagnostic tool: dump everything the daemon (Session 2 when launched via
// `cua-driver autostart kick`) sees about a given pid's top-level windows.
// Includes Win32 class names, owning .exe basename + full path, and — when
// CUIAutomation is available — the currently-focused UIA element with the
// list of patterns it supports.
//
// Built for CUA-543 investigation: the routing between PostMessage-based
// input and UIA-pattern-based input depends on what the OS actually reports
// for modern XAML / UWP / WinUI3 targets. SSH-side PowerShell probes from
// Session 0 see nothing (cross-session HWNDs return empty class names);
// this tool gives the same view from inside the daemon process.

pub struct DebugWindowInfoTool;
static DEBUG_WI_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for DebugWindowInfoTool {
    fn def(&self) -> &ToolDef {
        DEBUG_WI_DEF.get_or_init(|| ToolDef {
            name: "debug_window_info".into(),
            description: "Diagnostic: dump everything cua-driver sees about a pid's \
                top-level windows from the daemon's session perspective. Returns \
                window class names, owning .exe basename + path, and — when \
                CUIAutomation succeeds — the focused UIA element with the list of \
                patterns it supports (ValuePattern, InvokePattern, TextPattern, \
                TogglePattern, etc.). Used to design / debug input routing for \
                XAML / UWP / WinUI3 targets; see CUA-543.".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"PID of the process to inspect."}
            },"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let pid_v: u32 = match args.get("pid").and_then(|v| v.as_u64()) {
            Some(p) if p > 0 && p <= u32::MAX as u64 => p as u32,
            _ => return ToolResult::error("debug_window_info: missing or invalid `pid`".to_string()),
        };

        let outcome = tokio::task::spawn_blocking(move || -> serde_json::Value {
            use std::collections::HashSet;
            use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, BOOL, TRUE};
            use windows::Win32::System::Com::{
                CoCreateInstance, CoInitializeEx, CoUninitialize,
                CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
            };
            use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
            use windows::Win32::System::Threading::{
                GetCurrentProcessId, OpenProcess,
                QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
                PROCESS_QUERY_LIMITED_INFORMATION,
            };
            use windows::Win32::UI::Accessibility::{
                CUIAutomation, IUIAutomation,
                UIA_InvokePatternId, UIA_TextPatternId, UIA_TogglePatternId,
                UIA_ValuePatternId, UIA_LegacyIAccessiblePatternId,
                UIA_SelectionItemPatternId, UIA_ExpandCollapsePatternId,
            };
            use windows::Win32::UI::WindowsAndMessaging::{
                EnumWindows, GetClassNameW, GetWindow, GetWindowTextW,
                GetWindowThreadProcessId, IsWindowVisible, GW_OWNER,
            };

            // Helper: read class name + title
            fn class_name(h: HWND) -> Option<String> {
                let mut buf = [0u16; 256];
                let n = unsafe { GetClassNameW(h, &mut buf) };
                if n <= 0 { None } else { Some(String::from_utf16_lossy(&buf[..n as usize])) }
            }
            fn window_title(h: HWND) -> Option<String> {
                let mut buf = [0u16; 512];
                let n = unsafe { GetWindowTextW(h, &mut buf) };
                if n <= 0 { None } else { Some(String::from_utf16_lossy(&buf[..n as usize])) }
            }

            // ── Process info: session id + exe path
            let mut session_id: u32 = 0;
            let _ = unsafe { ProcessIdToSessionId(pid_v, &mut session_id) };

            let (exe_path, exe_basename) = unsafe {
                match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid_v) {
                    Ok(h) => {
                        let mut buf = [0u16; 1024];
                        let mut len: u32 = buf.len() as u32;
                        let ok = QueryFullProcessImageNameW(
                            h, PROCESS_NAME_FORMAT(0),
                            windows::core::PWSTR(buf.as_mut_ptr()),
                            &mut len,
                        );
                        let _ = CloseHandle(h);
                        if ok.is_ok() && len > 0 {
                            let path = String::from_utf16_lossy(&buf[..len as usize]);
                            let base = path
                                .rsplit(|c: char| c == '\\' || c == '/')
                                .next()
                                .unwrap_or(&path)
                                .to_ascii_lowercase();
                            (Some(path), Some(base))
                        } else {
                            (None, None)
                        }
                    }
                    Err(_) => (None, None),
                }
            };

            // ── Enumerate top-level visible windows owned by this pid
            struct Ctx { target_pid: u32, hwnds: Vec<isize> }
            extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
                unsafe {
                    let ctx = &mut *(lparam.0 as *mut Ctx);
                    let mut p: u32 = 0;
                    GetWindowThreadProcessId(hwnd, Some(&mut p));
                    if p == ctx.target_pid && IsWindowVisible(hwnd).as_bool() {
                        // Only true top-level (no owner) windows.
                        let owner = GetWindow(hwnd, GW_OWNER);
                        if owner.unwrap_or_default().is_invalid() {
                            ctx.hwnds.push(hwnd.0 as isize);
                        }
                    }
                    TRUE
                }
            }
            let mut ctx = Ctx { target_pid: pid_v, hwnds: vec![] };
            let _ = unsafe {
                EnumWindows(Some(enum_cb), LPARAM(&mut ctx as *mut Ctx as isize))
            };

            // ── Init UIA (best-effort; non-fatal if it fails)
            let mut focused_info: Option<serde_json::Value> = None;
            let coinit_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
            if let Ok(uia) = unsafe {
                CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            } {
                if let Ok(focused) = unsafe { uia.GetFocusedElement() } {
                    // Patterns we care about for input routing
                    let pattern_ids = [
                        (UIA_ValuePatternId.0, "Value"),
                        (UIA_InvokePatternId.0, "Invoke"),
                        (UIA_TextPatternId.0, "Text"),
                        (UIA_TogglePatternId.0, "Toggle"),
                        (UIA_ExpandCollapsePatternId.0, "ExpandCollapse"),
                        (UIA_SelectionItemPatternId.0, "SelectionItem"),
                        (UIA_LegacyIAccessiblePatternId.0, "LegacyIAccessible"),
                    ];
                    let mut patterns: Vec<&str> = vec![];
                    for (id, name) in &pattern_ids {
                        let pid_struct = windows::Win32::UI::Accessibility::UIA_PATTERN_ID(*id);
                        if unsafe { focused.GetCurrentPattern(pid_struct) }.is_ok() {
                            patterns.push(name);
                        }
                    }
                    let name = unsafe { focused.CurrentName() }
                        .ok()
                        .map(|b| b.to_string())
                        .unwrap_or_default();
                    let class = unsafe { focused.CurrentClassName() }
                        .ok()
                        .map(|b| b.to_string())
                        .unwrap_or_default();
                    let ctype = unsafe { focused.CurrentControlType() }
                        .map(|t| t.0)
                        .unwrap_or(0);
                    let value_ro = if patterns.contains(&"Value") {
                        unsafe {
                            focused.GetCurrentPattern(UIA_ValuePatternId)
                                .and_then(|p| p.cast::<windows::Win32::UI::Accessibility::IUIAutomationValuePattern>())
                                .and_then(|vp| vp.CurrentIsReadOnly())
                                .map(|b| b.as_bool())
                                .ok()
                        }
                    } else { None };
                    focused_info = Some(serde_json::json!({
                        "name": name,
                        "class_name": class,
                        "control_type_id": ctype,
                        "supported_patterns": patterns,
                        "value_pattern_is_read_only": value_ro,
                    }));
                }
            }
            if coinit_ok {
                unsafe { CoUninitialize(); }
            }

            // ── Build per-window JSON
            let xaml_classes: HashSet<&str> = [
                "ApplicationFrameWindow",
                "WinUIDesktopWin32WindowClass",
                "Windows.UI.Core.CoreWindow",
                "Microsoft.UI.Content.DesktopChildSiteBridge",
            ].iter().copied().collect();
            let xaml_exes: HashSet<&str> = [
                "notepad.exe", "calculatorapp.exe", "calc.exe",
                "applicationframehost.exe", "photos.exe", "systemsettings.exe",
            ].iter().copied().collect();

            let windows_json: Vec<serde_json::Value> = ctx.hwnds.iter()
                .map(|&h| {
                    let hwnd = HWND(h as *mut _);
                    let cls = class_name(hwnd);
                    let title = window_title(hwnd);
                    let xaml_class_match = cls.as_deref()
                        .map(|c| xaml_classes.contains(c))
                        .unwrap_or(false);
                    serde_json::json!({
                        "hwnd": h,
                        "class_name": cls,
                        "title": title,
                        "xaml_class_match": xaml_class_match,
                    })
                })
                .collect();

            let exe_xaml_match = exe_basename.as_deref()
                .map(|e| xaml_exes.contains(e))
                .unwrap_or(false);

            let daemon_pid = unsafe { GetCurrentProcessId() };
            let mut daemon_session: u32 = 0;
            let _ = unsafe { ProcessIdToSessionId(daemon_pid, &mut daemon_session) };

            serde_json::json!({
                "pid": pid_v,
                "session_id": session_id,
                "daemon_pid": daemon_pid,
                "daemon_session_id": daemon_session,
                "exe_path": exe_path,
                "exe_basename": exe_basename,
                "exe_xaml_match": exe_xaml_match,
                "windows": windows_json,
                "focused_element": focused_info,
                "xaml_routing_recommended": exe_xaml_match
                    || windows_json.iter()
                        .any(|w| w.get("xaml_class_match").and_then(|v| v.as_bool()).unwrap_or(false)),
            })
        }).await;

        match outcome {
            Ok(j) => {
                let pretty = serde_json::to_string_pretty(&j).unwrap_or_else(|_| j.to_string());
                ToolResult::text(pretty).with_structured(j)
            }
            Err(e) => ToolResult::error(format!("debug_window_info: join error: {e}")),
        }
    }
}

// ── registry builder ──────────────────────────────────────────────────────────

pub fn build_registry(compat: bool) -> ToolRegistry {
    let state = ToolState::new();
    let mut r = ToolRegistry::new();
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool { state: state.clone() }));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    r.register(Box::new(DebugWindowInfoTool));
    r.register(Box::new(ClickTool { state: state.clone() }));
    r.register(Box::new(DoubleClickTool { state: state.clone() }));
    r.register(Box::new(RightClickTool { state: state.clone() }));
    r.register(Box::new(DragTool { state: state.clone() }));
    r.register(Box::new(TypeTextTool { state: state.clone() }));
    r.register(Box::new(PressKeyTool));
    r.register(Box::new(HotkeyTool));
    r.register(Box::new(SetValueTool { state: state.clone() }));
    r.register(Box::new(ScrollTool));
    if compat {
        r.register(Box::new(ScreenshotCompatTool { state: state.clone() }));
    } else {
        r.register(Box::new(ScreenshotTool { state: state.clone() }));
    }
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
    let _: &TypeTextCharsTool = &TypeTextCharsTool { _state: state.clone() }; // touch struct so it stays in this crate for now
    // Cross-platform `page` tool definition lives in mcp-server; Windows plugs
    // in its UIA TextPattern + FindAll backend (CDP for execute_javascript).
    r.register(Box::new(mcp_server::page::PageTool::new(
        std::sync::Arc::new(super::page::WindowsPageBackend::new()),
    )));
    r.register_recording_tools();
    r
}

#[cfg(test)]
mod launch_focus_restore_decision_tests {
    use super::{should_restore_foreground_after_launch, LaunchTargetShape};

    fn empty() -> LaunchTargetShape {
        LaunchTargetShape {
            has_aumid: false,
            has_bundle_id: false,
            has_name: false,
            has_path: false,
            has_launch_path: false,
            has_urls: false,
        }
    }

    #[test]
    fn urls_only_skips_restore() {
        // `launch_app({urls: ["https://example.com"]})` — user explicitly
        // asked for navigation in the default browser. The freshly-launched
        // browser instance IS the legitimate foreground; restoring would
        // hide the page the user wanted to see.
        let shape = LaunchTargetShape { has_urls: true, ..empty() };
        assert!(!should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn name_only_restores() {
        let shape = LaunchTargetShape { has_name: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn path_only_restores() {
        let shape = LaunchTargetShape { has_path: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn aumid_only_restores() {
        // Note: AUMID path has its own synchronous restore in launch_uwp.rs,
        // but the decision function still says "yes, this is an
        // app-identifying launch" — the caller (`LaunchAppTool::invoke`)
        // gates the polling spawn on `aumid_for_uwp.is_none()` so we don't
        // double-restore.
        let shape = LaunchTargetShape { has_aumid: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn bundle_id_only_restores() {
        let shape = LaunchTargetShape { has_bundle_id: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn launch_path_only_restores() {
        let shape = LaunchTargetShape { has_launch_path: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn name_with_urls_restores() {
        // App-identifying field present alongside urls — the user wants
        // the named app to open these URLs in the background, NOT for the
        // urls to take over the default browser's foreground. Restore.
        let shape = LaunchTargetShape { has_name: true, has_urls: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn path_with_urls_restores() {
        let shape = LaunchTargetShape { has_path: true, has_urls: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn aumid_with_urls_restores() {
        let shape = LaunchTargetShape { has_aumid: true, has_urls: true, ..empty() };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn no_target_does_not_restore() {
        // Empty params (validated as an error before launch dispatch) — no
        // restore needed because nothing was launched.
        assert!(!should_restore_foreground_after_launch(empty()));
    }
}

#[cfg(test)]
mod chromium_flag_injection_tests {
    use super::{inject_chromium_anti_throttling_flags, is_chromium_browser_target};

    #[test]
    fn detects_bare_browser_names() {
        for name in [
            "msedge", "chrome", "brave", "opera", "vivaldi",
            "chromium", "thorium", "iridium", "browser", "arc",
        ] {
            assert!(is_chromium_browser_target(name), "{name} should match");
            assert!(is_chromium_browser_target(&format!("{name}.exe")), "{name}.exe should match");
            // Case-insensitive.
            assert!(is_chromium_browser_target(&name.to_uppercase()), "uppercase {name} should match");
        }
    }

    #[test]
    fn detects_full_paths() {
        assert!(is_chromium_browser_target(
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"
        ));
        assert!(is_chromium_browser_target(
            r"C:\Program Files\Google\Chrome\Application\chrome.exe"
        ));
        // Forward slashes too (some shells write paths that way).
        assert!(is_chromium_browser_target(
            r"C:/Program Files/Google/Chrome/Application/chrome.exe"
        ));
    }

    #[test]
    fn detects_launch_path_with_trailing_args() {
        // Round-tripped launch_path from list_apps with shortcut arguments.
        assert!(is_chromium_browser_target(
            r#""C:\Program Files\Google\Chrome\Application\chrome.exe" --profile-directory="Profile 2""#
        ));
    }

    #[test]
    fn does_not_match_non_chromium_apps() {
        for name in ["firefox", "notepad", "explorer", "code", "soffice"] {
            assert!(!is_chromium_browser_target(name), "{name} should NOT match");
            assert!(!is_chromium_browser_target(&format!("{name}.exe")), "{name}.exe should NOT match");
        }
        // Empty target.
        assert!(!is_chromium_browser_target(""));
    }

    #[test]
    fn injects_three_flags_into_empty_args() {
        let mut args: Vec<String> = vec![];
        inject_chromium_anti_throttling_flags(&mut args);
        assert!(args.contains(&"--disable-features=CalculateNativeWinOcclusion".to_string()));
        assert!(args.contains(&"--disable-backgrounding-occluded-windows".to_string()));
        assert!(args.contains(&"--disable-renderer-backgrounding".to_string()));
        assert_eq!(args.len(), 3);
    }

    #[test]
    fn merges_into_existing_disable_features_list() {
        let mut args = vec!["--disable-features=Foo,Bar".to_string()];
        inject_chromium_anti_throttling_flags(&mut args);
        // Should NOT have two --disable-features= entries.
        let dfe: Vec<_> = args.iter().filter(|a| a.starts_with("--disable-features=")).collect();
        assert_eq!(dfe.len(), 1);
        assert!(dfe[0].contains("CalculateNativeWinOcclusion"));
        assert!(dfe[0].contains("Foo"));
        assert!(dfe[0].contains("Bar"));
    }

    #[test]
    fn idempotent_when_all_flags_already_present() {
        let mut args = vec![
            "--disable-features=CalculateNativeWinOcclusion".to_string(),
            "--disable-backgrounding-occluded-windows".to_string(),
            "--disable-renderer-backgrounding".to_string(),
        ];
        let before = args.clone();
        inject_chromium_anti_throttling_flags(&mut args);
        assert_eq!(args, before, "must not duplicate flags");
    }

    #[test]
    fn preserves_user_url_argument_after_flags() {
        let mut args = vec!["file:///C:/test_page.html".to_string()];
        inject_chromium_anti_throttling_flags(&mut args);
        // URL must still be present.
        assert!(args.iter().any(|a| a == "file:///C:/test_page.html"));
        // All three flags now in args.
        assert!(args.iter().any(|a| a == "--disable-features=CalculateNativeWinOcclusion"));
        assert!(args.iter().any(|a| a == "--disable-backgrounding-occluded-windows"));
        assert!(args.iter().any(|a| a == "--disable-renderer-backgrounding"));
    }
}
