//! Real Windows tool implementations (compiled only on Windows).

use async_trait::async_trait;

/// Pin the agent-cursor overlay above `hwnd` in the z-order, normalising to
/// the **root** ancestor so the pin lands on the window that actually appears
/// in the global z-stack.
///
/// For UWP / packaged apps the inner `Windows.UI.Core.CoreWindow` is hosted
/// inside `ApplicationFrameHost.exe` — UIA targets the inner window but only
/// the host appears in the z-order. `GA_ROOT` normalises both inputs to the
/// host so the overlay sits at z+1 of whatever is actually painted on screen.
///
/// No-op when the overlay is disabled or `key` is empty (anonymous, cursor-less
/// run); the command is just dropped by the render thread in that case.
fn pin_overlay_above(key: &str, hwnd: u64) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT};
    let root = unsafe { GetAncestor(HWND(hwnd as *mut _), GA_ROOT) };
    let wid = if !root.0.is_null() {
        root.0 as u64
    } else {
        hwnd
    };
    crate::overlay::send_command(
        key.to_owned(),
        cursor_overlay::OverlayCommand::PinAbove(wid),
    );
}

/// Resolve the screen point a coordinate action should actuate at, scrolling
/// the target element into view first when its cached center lands off-screen.
///
/// The element cache captures each element's center at walk time. On a short
/// display a tall window reflows so some controls sit below the visible area;
/// their cached center then falls outside the window rect and a raw tap would
/// land on the taskbar / another window instead of the control. Before failing,
/// we ask the control to scroll itself visible via UIA
/// `ScrollItemPattern::ScrollIntoView` (the control drives its own
/// ScrollViewer), then re-read its *live* bounding-rect center.
///
/// Returns:
/// - `Ok((cx,cy))` unchanged when already in bounds — fast path, no COM call;
/// - `Ok((nx,ny))` with the post-scroll center when scrolling brought it in;
/// - `Err(message)` when it is still off-screen — preserving the existing clean
///   failure (a clear error beats a misfire onto the taskbar).
///
/// Background-safe: ScrollIntoView is delivered over the accessibility channel
/// (wrapped in the UWP fg-steal bypass) and does not raise or activate the
/// window. MSAA-walked elements are skipped (no ScrollItemPattern) and keep the
/// existing failure.
fn resolve_onscreen_point_with_scroll(
    element_cache: &crate::uia::cache::ElementCache,
    pid: u32,
    hwnd: u64,
    idx: usize,
    cx: i32,
    cy: i32,
    action_verb: &str,
) -> Result<(i32, i32), String> {
    let cached_center_in_window = crate::input::point_in_window_bounds(hwnd, cx, cy);
    // A center can still be clipped by a ScrollViewer or a docked sibling while
    // remaining inside the outer HWND rectangle. UIA's IsOffscreen property is
    // authoritative for that case; without it a foreground tap can land on the
    // visible control covering the stale point (for example a bottom toolbar).
    if let Some(retained) = element_cache.get_element_retained(pid, hwnd, idx) {
        if retained.is_uia() {
            let is_offscreen =
                unsafe { crate::uia::scroll::element_is_offscreen(retained.as_ptr()) };
            if cached_center_in_window && is_offscreen != Some(true) {
                return Ok((cx, cy));
            }
            if let Some((nx, ny)) = unsafe {
                crate::uia::scroll::scroll_into_view_and_recenter(hwnd, retained.as_ptr())
            } {
                if crate::input::point_in_window_bounds(hwnd, nx, ny) {
                    return Ok((nx, ny));
                }
            }
        } else if cached_center_in_window {
            return Ok((cx, cy));
        }
        // `retained` drops here → COM Release.
    } else if cached_center_in_window {
        return Ok((cx, cy));
    }
    Err(format!(
        "Element [{idx}] resolves to ({cx},{cy}) but is not visibly actionable — \
         tried UIA ScrollIntoView but it is still off-screen, so {action_verb} \
         would land on whatever covers that point. Make the window taller or \
         scroll the region into view, then retry."
    ))
}

/// Convert (px, py) — "window-local screenshot pixels, top-left origin
/// of the PNG returned by `get_window_state`" — to screen coordinates.
///
/// The capture path (`crate::capture::screenshot_window`) takes the
/// PrintWindow buffer (sized to `GetWindowRect`) and crops it to
/// `DWMWA_EXTENDED_FRAME_BOUNDS` with a 1-pixel inset on each side
/// (`DWM_CROP_INSET_PX`). So the bitmap's top-left in screen coords is
/// `(dwm_frame.left + 1, dwm_frame.top + 1)` — and CALLER coordinates
/// `(px, py)` are offsets relative to that origin.
///
/// Pre-PR-#1697 the click / right_click / double_click / drag impls
/// called `ClientToScreen(px, py)`, which interprets the offsets as
/// CLIENT-area relative — so the screen click landed ~30 px BELOW the
/// bitmap location the agent intended (the title-bar height got added).
/// This helper aligns the screen mapping with what the screenshot
/// actually shows.
///
/// Fallbacks: if the DWM call fails (very old Windows / shell-extension
/// hook) we degrade to `GetWindowRect.top-left + (px, py)` — matches
/// the capture fallback which also keeps the full PrintWindow bitmap
/// without crop.
fn bitmap_to_screen(hwnd: u64, px: i32, py: i32) -> (i32, i32) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
    use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;

    // Keep this constant in sync with `capture::DWM_CROP_INSET_PX`.
    const DWM_CROP_INSET_PX: i32 = 1;
    let h = HWND(hwnd as *mut _);
    unsafe {
        let mut dwm = RECT::default();
        let hr = DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut dwm as *mut _ as *mut _,
            std::mem::size_of::<RECT>() as u32,
        );
        if hr.is_ok() {
            return (
                dwm.left + DWM_CROP_INSET_PX + px,
                dwm.top + DWM_CROP_INSET_PX + py,
            );
        }
        // Fallback path — capture also keeps the full bitmap when DWM
        // bounds aren't available, so the bitmap origin IS GetWindowRect
        // top-left in that branch.
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        (wr.left + px, wr.top + py)
    }
}

fn screen_to_bitmap(hwnd: u64, sx: i32, sy: i32) -> (i32, i32) {
    let (origin_x, origin_y) = bitmap_to_screen(hwnd, 0, 0);
    (sx - origin_x, sy - origin_y)
}

/// Animate the agent cursor to (sx, sy) in screen coordinates and wait for the
/// glide to finish before returning.  No-op when the overlay is not enabled.
///
/// On the very first call the cursor is at the off-screen initial position
/// (-200, -200).  Animating from there would cause a jarring off-screen fly-in,
/// so we snap to the target with a ClickPulse first and skip the glide wait.
///
/// For all subsequent calls this defers to
/// [`crate::overlay::animate_cursor_to`], which sends `MoveTo` and awaits the
/// render thread's arrival oneshot — that's how we keep the click action
/// from firing before the cursor has visually landed (the old heuristic
/// `tokio::sleep(80..600 ms)` was racing the spring-physics glide).
///
/// `key` is the session's cursor key (see [`resolve_cursor_key`]). An empty key
/// (anonymous, no declared session) is cursor-less: every overlay op
/// short-circuits, so the action runs with no visible cursor.
async fn overlay_glide_to(key: &str, sx: f64, sy: f64) {
    if key.is_empty() {
        return;
    }
    if !crate::overlay::is_enabled(key) {
        return;
    }
    let pos = crate::overlay::current_position(key);
    if pos.0 < 0.0 && pos.1 < 0.0 {
        // Snap to target on first use; no animation to wait for.
        crate::overlay::send_command(
            key.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
        );
        return;
    }
    // Fixed-duration glide → decouple the click from the render thread. When
    // `glide_duration_ms > 0` the path completes in exactly that wall-clock time
    // (see render_state::tick_motion), so instead of `await`-ing the render
    // thread's arrival oneshot — which couples click latency to overlay FPS and
    // degrades under many concurrent cursors — we fire the move and sleep the
    // known duration. The click then lands a deterministic time after dispatch
    // regardless of render load, while the cursor still animates visually on the
    // render thread. Speed-based glides (`== 0`, the default) keep the precise
    // arrival-await since their duration depends on distance.
    let motion = crate::overlay::current_motion(key);
    if motion.glide_duration_ms > 0.0 {
        crate::overlay::send_command(
            key.to_owned(),
            cursor_overlay::OverlayCommand::MoveTo {
                x: sx,
                y: sy,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
            },
        );
        tokio::time::sleep(std::time::Duration::from_millis(
            motion.glide_duration_ms as u64,
        ))
        .await;
        return;
    }
    crate::overlay::animate_cursor_to(key.to_owned(), sx, sy).await;
}
use cua_driver_contract::{
    ClickButton, ClickInput, DragInput, GetCursorPositionInput, GetDesktopStateInput,
    GetScreenSizeInput, HotkeyInput, MoveCursorInput, PressKeyInput, ScrollDirection, ScrollInput,
    TypeTextInput,
};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
    tool_args::{parse_typed_input, parse_typed_projection},
};
use serde_json::{json, Value};
use std::sync::{Arc, RwLock};

use crate::uia::ElementCache;
use cursor_overlay::CursorRegistry;
use windows::core::Interface as _;

/// The cursor key for an anonymous (cursor-less) call. A run opts into a cursor
/// by declaring a `session`; without one, every cursor op short-circuits on
/// this empty key (see `overlay::send_command` / `overlay_glide_to`).
pub(crate) const NO_CURSOR: &str = "";

/// Resolve the cursor key for a tool invocation, or [`NO_CURSOR`] (`""`) for an
/// anonymous call.
///
/// A cursor is tied to a **caller-declared session**, never to the MCP
/// connection. Precedence: an explicit `session` arg, then its legacy alias
/// `cursor_id`. We deliberately do NOT fall back to the connection-injected
/// `_session_id` or to a seeded `"default"` cursor — `""` means "no session
/// declared → no cursor", while the underlying action (click/type/…) still
/// executes. Mirrors `platform_macos::tools::cursor_tools::resolve_cursor_key`
/// so the two platforms key cursors identically.
pub(crate) fn resolve_cursor_key(args: &Value) -> String {
    for key in ["session", "cursor_id"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return v.to_owned();
            }
        }
    }
    NO_CURSOR.to_owned()
}

/// Returns `true` when a click/scroll invocation should take the **window-less
/// screen-absolute** branch: the caller gave numeric `x` AND `y`, gave NO `pid`
/// and NO `window_id`, and explicitly selected `scope="desktop"`.
///
/// Pure + arg-shape agnostic (works for both click and scroll args) so it's
/// unit-testable without Win32. When this returns `false` but `x,y` are present
/// with no pid/window_id, the caller returns a coordinate-scope error.
fn is_windowless_desktop_action(args: &serde_json::Value) -> bool {
    if args.get("scope").and_then(Value::as_str) != Some("desktop") {
        return false;
    }
    let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
    let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
    if has_pid || has_window_id {
        return false;
    }
    let has_num = |k: &str| args.get(k).map(|v| v.is_number()).unwrap_or(false);
    has_num("x") && has_num("y")
}

// ── DriverConfig + ResizeRegistry + ZoomRegistry ─────────────────────────────

#[derive(Clone)]
pub struct DriverConfig {
    pub capture_mode: String,
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            capture_mode: "ax".into(),
            max_image_dimension: 1568,
        }
    }
}

/// Load `DriverConfig` from `~/.cua-driver/config.json`, falling back to
/// defaults for any missing/malformed keys. Called once at `ToolState`
/// construction (i.e. on every fresh `cua-driver call` process) so that a
/// prior `set_config max_image_dimension=...` survives across stateless
/// one-shot invocations — matching the macOS daemon's startup load. See #2008.
/// (`capture_mode` is still loaded/persisted for back-compat but no longer
/// affects behavior — `get_window_state` always returns tree + screenshot.)
pub fn load_driver_config() -> DriverConfig {
    let mut cfg = DriverConfig::default();
    if let Some(v) =
        pip_preview::read_config_value("capture_mode").and_then(|v| v.as_str().map(str::to_owned))
    {
        cfg.capture_mode = v;
    }
    if let Some(v) = pip_preview::read_config_value("max_image_dimension").and_then(|v| v.as_u64())
    {
        if let Ok(v32) = u32::try_from(v) {
            cfg.max_image_dimension = v32;
        }
    }
    cfg
}

pub struct ResizeRegistry {
    ratios: std::sync::Mutex<std::collections::HashMap<u32, f64>>,
}

impl ResizeRegistry {
    pub fn new() -> Self {
        Self {
            ratios: std::sync::Mutex::new(Default::default()),
        }
    }
    pub fn set_ratio(&self, pid: u32, ratio: f64) {
        self.ratios.lock().unwrap().insert(pid, ratio);
    }
    pub fn clear_ratio(&self, pid: u32) {
        self.ratios.lock().unwrap().remove(&pid);
    }
    pub fn ratio(&self, pid: u32) -> Option<f64> {
        self.ratios.lock().unwrap().get(&pid).copied()
    }
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
        (
            self.origin_x + px * self.scale_inv,
            self.origin_y + py * self.scale_inv,
        )
    }
}

pub struct ZoomRegistry {
    inner: std::sync::Mutex<std::collections::HashMap<u32, ZoomContext>>,
}

impl ZoomRegistry {
    pub fn new() -> Self {
        Self {
            inner: std::sync::Mutex::new(Default::default()),
        }
    }
    pub fn set(&self, pid: u32, ctx: ZoomContext) {
        self.inner.lock().unwrap().insert(pid, ctx);
    }
    pub fn get(&self, pid: u32) -> Option<ZoomContext> {
        self.inner.lock().unwrap().get(&pid).copied()
    }
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
            config: Arc::new(RwLock::new(load_driver_config())),
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
            description:
                "List Windows apps — both currently running and installed-but-not-running — \
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
                directly; list_apps is not a prerequisite."
                    .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
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

        let running_count = apps
            .iter()
            .filter(|a| a["running"].as_bool().unwrap_or(false))
            .count();
        let total = apps.len();
        let installed_only = total - running_count;
        let mut lines = vec![format!(
            "✅ Found {total} app(s): {running_count} running, {installed_only} installed-not-running."
        )];
        for app in apps
            .iter()
            .filter(|a| a["running"].as_bool().unwrap_or(false))
        {
            let name = app["name"].as_str().unwrap_or("?");
            let pid = app["pid"].as_u64().unwrap_or(0);
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
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

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
                is_on_screen, minimized. The macOS-specific on_current_space / space_ids fields are \
                omitted on Windows; current_space_id is null.\n\n\
                Inputs: pid (optional pid filter), on_screen_only (bool, default false).".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer","description":"Optional pid filter. When set, only this pid's windows are returned."},
                "on_screen_only":{"type":"boolean","description":"When true, drop windows that aren't currently on-screen. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let filter_pid = args.opt_u64("pid").map(|v| v as u32);
        let on_screen_only = args.bool_or("on_screen_only", false);
        let (mut windows, pid_to_name) = tokio::task::spawn_blocking(move || {
            let wins = crate::win32::list_windows(filter_pid);
            let procs = crate::win32::list_processes();
            let map: std::collections::HashMap<u32, String> =
                procs.into_iter().map(|p| (p.pid, p.name)).collect();
            (wins, map)
        })
        .await
        .unwrap_or_default();
        if on_screen_only {
            windows.retain(|w| w.is_on_screen);
        }

        // Swift surfaces a warning when a pid filter matches nothing.
        let missing_pid_warning = if let Some(fp) = filter_pid {
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
                Some(format!(
                    "⚠️ No windows found for pid {fp}. The pid may be wrong or the app may not \
                     have created a window yet. The current frontmost app appears to be \
                     \"{fg_name}\" (pid {fg_pid})."
                ))
            } else {
                None
            }
        } else {
            None
        };

        // z_index: list_windows merges EnumWindows first (which the Win32
        // window manager returns in canonical top-to-bottom z-order), then
        // appends any UIA-only HWNDs UIA found that EnumWindows missed
        // (UIA-only entries land at the bottom of the z-stack — they have
        // no canonical Win32 ordering). Higher list index = farther from
        // front. Swift convention: higher z_index = closer to front.
        // Invert via `(len - 1 - i)` so the front-most window gets the
        // largest z.
        let n = windows.len();
        let records: Vec<serde_json::Value> = windows
            .iter()
            .enumerate()
            .map(|(i, w)| {
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
                    "is_on_screen": w.is_on_screen,
                    "minimized":    w.minimized,
                })
            })
            .collect();

        let total = records.len();
        let pid_count = windows
            .iter()
            .map(|w| w.pid)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let on_screen = records
            .iter()
            .filter(|r| r["is_on_screen"].as_bool().unwrap_or(false))
            .count();
        // Swift text format 1:1 — append the SPI-unavailable note since
        // Windows has no equivalent of macOS Spaces.
        let header = format!(
            "✅ Found {total} window(s) across {pid_count} app(s); {on_screen} on-screen. \
             (SkyLight Space SPIs unavailable — on_current_space / space_ids omitted.)"
        );
        let mut lines = vec![header];
        if let Some(warning) = missing_pid_warning {
            lines.push(warning);
        }
        for r in &records {
            let app = r["app_name"].as_str().unwrap_or("?");
            let pid = r["pid"].as_u64().unwrap_or(0);
            let tt = r["title"].as_str().unwrap_or("");
            let wid = r["window_id"].as_u64().unwrap_or(0);
            let title_disp = if tt.is_empty() {
                "(no title)".to_owned()
            } else {
                format!("\"{tt}\"")
            };
            let tag = if r["is_on_screen"].as_bool().unwrap_or(false) {
                ""
            } else {
                " [off-screen]"
            };
            lines.push(format!(
                "- {app} (pid {pid}) {title_disp} [window_id: {wid}]{tag}"
            ));
        }

        // Legacy alias: keep the old flat fields under each record for any
        // pre-existing Rust callers, but they read from the same source.
        let legacy_windows: Vec<serde_json::Value> = windows
            .iter()
            .map(|w| {
                json!({
                    "window_id": w.hwnd, "pid": w.pid, "title": w.title,
                    "x": w.x, "y": w.y, "width": w.width, "height": w.height,
                    "is_on_screen": w.is_on_screen, "minimized": w.minimized,
                })
            })
            .collect();
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
            description: "Walk a running app's UIA tree and return BOTH a structured \
                `elements` array (preferred) AND a Markdown rendering of the same tree \
                (back-compat). Every actionable element is tagged with [element_index N] \
                in the markdown and as `element_index` in the structured array — pass \
                those indices to `click`, `type_text`, `scroll`, etc.\n\n\
                INVARIANT: call `get_window_state` once per turn per (pid, window_id) before \
                any element-indexed action against that window. The index map is replaced by \
                the next snapshot of the same (pid, window_id).\n\n\
                PREFERRED CONSUMERS read `structuredContent.elements` (one entry per \
                indexed row with `element_index`, `role`, `label`, `frame: {x,y,w,h}`, \
                `parent_index`, `depth`). The markdown `tree_markdown` stays available \
                and unchanged in shape for existing text-parsing callers — but new \
                fields will only be added to the structured side.\n\n\
                The UIA tree walked is the window's tree (HWND-scoped); the screenshot and \
                window bounds reported come from the same `window_id`. This is the source of \
                truth for which window the caller intends to reason about — the driver never \
                picks a window implicitly.\n\n\
                `window_id` MUST belong to `pid`; the call returns `isError: true` otherwise. \
                The driver does not auto-fall-back to a different window.\n\n\
                Set `query` to a case-insensitive substring to filter `tree_markdown` down to \
                matching lines plus their ancestor chain. element_index values and \
                `element_count` are unchanged — the filter only trims the rendered Markdown.\n\n\
                Always returns BOTH the element tree AND a screenshot — ground on both \
                and cross-check (the tree lies on some surfaces). Choose the modality at \
                ACTION time: an element ax action (element_index/element_token → \
                accessibility rung) or an element px action (x,y → pixel rung off this \
                screenshot). capture_mode is deprecated and ignored.\n\n\
                Uses `IUIAutomationCacheRequest` to batch-fetch all element properties in a \
                single COM call (Chrome's ~5000-element tree returns in ~2-3s instead of \
                timing out at 4s with per-property RPCs).\n\n\
                Optional `max_elements` / `max_depth` bound the UIA walk to mitigate \
                context-window blow-up on Electron / large web apps that produce 10k+ \
                element trees. When applied, BOTH the markdown and the structured \
                elements are truncated identically. Omit both for current default behaviour \
                (≤5 000 elements, depth ≤25).\n\n\
                Windows requires no special permissions.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Process ID from `list_apps`."},
                "window_id":{"type":"integer","description":"HWND of the target window. Must belong to `pid`. Enumerate via `list_windows` or read from `launch_app`'s `windows` array."},
                "capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),
                "include_screenshot":{"type":"boolean","description":"Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return tree only (the cheap path for re-indexing before an element ax action)."},
                "screenshot_out_file":{"type":"string","description":"When set, write the PNG to this file path instead of embedding base64 in the response. The structured output will contain `screenshot_file_path` instead."},
                "query":{"type":"string","description":"Optional case-insensitive substring. When set, `tree_markdown` only contains lines that match plus their ancestor chain; element indices and `element_count` are unchanged."},
                "max_elements":{"type":"integer","minimum":1,"description":"Cap on the total number of UIA nodes walked. Truncates depth-first; markdown and structured elements truncate together. Omit for the default (5 000). Lower for Electron / large web apps that produce 10k+ element trees."},
                "max_depth":{"type":"integer","minimum":1,"description":"Cap on the UIA-tree walk depth. Nodes whose rendered indent would exceed this are omitted. Omit for the default (25). Lower for deep menu / Electron trees."}
            },"additionalProperties":false}),
            // Swift annotation: idempotent: false (each call is a fresh snapshot).
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Swift error wording 1:1.
        let pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(v) => v as u32,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let hwnd =
            match args.get("window_id").and_then(|v| v.as_u64()) {
                Some(v) => v,
                None => return ToolResult::error(
                    "Missing required integer field window_id. Use `list_windows` to enumerate \
                 the target app's windows, or read `launch_app`'s `windows` array.",
                ),
            };
        // Validate window belongs to pid — Swift's hard error.
        let windows_for_pid =
            tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                .await
                .unwrap_or_default();
        if !windows_for_pid.iter().any(|w| w.hwnd == hwnd) {
            // Check if the window exists under a different pid.
            let all = tokio::task::spawn_blocking(|| crate::win32::list_windows(None))
                .await
                .unwrap_or_default();
            if let Some(w) = all.iter().find(|w| w.hwnd == hwnd) {
                return ToolResult::error(format!(
                    "window_id {hwnd} belongs to pid {}, not pid {pid}. Call \
                     `list_windows({{\"pid\": {pid}}})` to get this pid's own windows.",
                    w.pid
                ));
            }
            return ToolResult::error(format!(
                "No window with window_id {hwnd} exists. Call `list_windows({{\"pid\": \
                 {pid}}})` for candidates."
            ));
        }
        let max_dim = {
            let cfg = self.state.config.read().unwrap();
            cfg.max_image_dimension
        };
        use cua_driver_core::tool_args::ArgsExt;
        // `capture_mode` is DEPRECATED and ignored — get_window_state always
        // returns BOTH the UIA tree and a screenshot now, so the agent grounds on
        // both and cross-checks (the UIA tree lies often enough that a grounding
        // screenshot should always be present). The modality is chosen at action
        // time: an element ax action (element_index) or an element px action (x,y).
        // We don't read the arg; it stays in the schema only so old callers don't
        // trip additionalProperties:false.
        let query = args.opt_str("query");
        let screenshot_out_file = args.opt_str("screenshot_out_file");
        // Optional caps — when omitted, fall back to the walker's built-in
        // defaults (#22865). minimum:1 enforced in the schema, but defend
        // against 0 here too.
        let max_elements = args
            .get("max_elements")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(crate::uia::DEFAULT_MAX_TOTAL_ELEMENTS);
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize)
            .unwrap_or(crate::uia::DEFAULT_MAX_DEPTH);

        // Always walk the UIA tree. The screenshot is returned by DEFAULT — the
        // grounding frame the agent cross-checks the (sometimes-lying) tree
        // against — but `include_screenshot:false` opts out of the grab entirely
        // (tree only: the cheap path for re-indexing before an element ax action).
        // `screenshot_out_file` still forces a capture to disk regardless. With
        // `screenshot_out_file` the bytes go to disk and the path is surfaced
        // instead of embedding base64; otherwise the base64 PNG is embedded.
        let include_screenshot = args.get("include_screenshot").and_then(|v| v.as_bool());
        let do_tree = true;
        let do_shot = include_screenshot != Some(false) || screenshot_out_file.is_some();

        let state = self.state.clone();
        let q = query.clone();
        let out_file = screenshot_out_file.clone();
        let blocking = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let tree_result = if do_tree {
                Some(crate::uia::walk_tree_bounded(
                    hwnd,
                    q.as_deref(),
                    max_elements,
                    max_depth,
                ))
            } else {
                None
            };
            // Capture screenshot AND any error message so the response can
            // surface *why* there's no image (the iconic-window guard from
            // #1973 / PR #1974 is the load-bearing case: minimized windows
            // legitimately can't be captured, and the caller needs to know
            // to call `bring_to_front` instead of retrying).
            // The previous `Err(_) => None` silently dropped the error and
            // upstream agents saw an empty response with no signal.
            let (screenshot, screenshot_err) = if do_shot {
                match crate::capture::screenshot_window_bytes(hwnd) {
                    Ok(raw) => {
                        let orig_w = crate::capture::png_dimensions_pub(&raw)
                            .map(|(w, _)| w)
                            .unwrap_or(0);
                        let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        let original_w = if w < orig_w { Some(orig_w) } else { None };
                        // `screenshot_out_file` set (any mode) → write to disk and
                        // surface the path, never embed bytes. Otherwise (vision,
                        // no out_file) → embed base64.
                        if let Some(ref path) = out_file {
                            std::fs::write(path, &png)?;
                            (Some((None, Some(path.clone()), w, h, original_w)), None)
                        } else {
                            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                            (Some((Some(B64.encode(&png)), None, w, h, original_w)), None)
                        }
                    }
                    Err(e) => (None, Some(format!("{e}"))),
                }
            } else {
                (None, None)
            };
            Ok((tree_result, screenshot, screenshot_err))
        });
        // Timeout: Chrome's UIA provider can block indefinitely on property reads.
        let result: Result<anyhow::Result<_>, _> =
            match tokio::time::timeout(std::time::Duration::from_secs(4), blocking).await {
                Ok(join_result) => join_result.map_err(|e| anyhow::anyhow!("task panic: {e}")),
                Err(_elapsed) => {
                    // Surface the target's window class + an actionable hint
                    // instead of just "UIA provider unresponsive". The class
                    // points the caller at the right workaround (e.g. SALFRAME
                    // → screenshot + pixel coords + delivery_mode:"foreground"; UWP
                    // class → re-call with a depth-limited scan and act by pixel
                    // off the screenshot if the tree stays unusable).
                    let class = crate::input::delivery::read_class_name(hwnd);
                    Err(anyhow::anyhow!(
                        "get_window_state timed out after 4s (UIA provider unresponsive on \
                     hwnd 0x{hwnd:x}, class '{class}'). Fallback options: \
                     (a) re-call this tool with a depth-limited scan \
                     (`max_elements` / `max_depth`) — if the tree stays unusable, act \
                     by pixel `click(x, y)` off the screenshot in the response; \
                     (b) if the target is a transient VCL / message-box dialog, send \
                     `press_key` with `delivery_mode:\"foreground\"` (SendInput) to fire the \
                     default accelerator (Esc / Enter / Y / N) without needing the tree."
                    ))
                }
            };
        let result = result.and_then(|r| r);

        match result {
            Ok((tree_opt, screenshot_opt, screenshot_err)) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": hwnd, "pid": pid });

                if let Some(tr) = tree_opt {
                    let count = tr
                        .nodes
                        .iter()
                        .filter(|n| n.element_index.is_some())
                        .count();
                    let header = format!("window_id={hwnd} pid={pid} elements={count}\n\n");
                    content.push(cua_driver_core::protocol::Content::text(
                        header + &tr.tree_markdown,
                    ));
                    // Route the cache to the matching dispatch path: any
                    // node whose msaa_role is Some came from the MSAA
                    // walker, so the entire snapshot must Drop via
                    // IAccessible and click must dispatch through MSAA.
                    let is_msaa = tr.nodes.iter().any(|n| n.msaa_role.is_some());
                    if is_msaa {
                        state.element_cache.update_msaa(pid, hwnd, &tr.nodes);
                    } else {
                        state.element_cache.update(pid, hwnd, &tr.nodes);
                    }
                    structured["element_count"] = json!(count);
                    structured["tree_markdown"] = json!(tr.tree_markdown);

                    // Surface 6: register a snapshot in the global token
                    // registry. Windows uses u64 HWND but the registry
                    // stores u32 — truncate (HWND fits in 32-bit on
                    // every supported edition; the upper 32 bits are
                    // zero in user-space).
                    let snapshot_id = cua_driver_core::element_token::global().register_snapshot(
                        pid as i32,
                        hwnd as u32,
                        count,
                    );

                    // Structured `elements` array — preferred consumption
                    // path. Shape matches the cross-platform spec:
                    // `{element_index, element_token, role, label, depth,
                    // parent_index?, frame?: {x,y,w,h}}`. Frame is
                    // included when UIA reported a usable BoundingRectangle.
                    let elements: Vec<serde_json::Value> = tr
                        .nodes
                        .iter()
                        .filter_map(|n| {
                            let idx = n.element_index?;
                            // `label`: name → value → automation_id → help_text.
                            let label = n.name.clone()
                                .or_else(|| n.value.clone())
                                .or_else(|| n.automation_id.clone())
                                .or_else(|| n.help_text.clone());
                            let mut entry = json!({
                                "element_index": idx,
                                // Surface 6: opaque token paired to the
                                // integer index. See cua-driver-core's
                                // `element_token` module for the format
                                // and validity contract.
                                "element_token": cua_driver_core::element_token::token_for(snapshot_id, idx),
                                "role": n.control_type,
                                "depth": n.depth,
                            });
                            if let Some(label) = label {
                                entry["label"] = json!(label);
                            }
                            // Surface the element's value separately from `label`
                            // (which collapses name→value→automation_id→help): a
                            // control with both a name AND text (a ValuePattern
                            // edit holding typed content) would otherwise hide the
                            // text from a caller reading the structured side. See
                            // the macOS get_window_state builder for the rationale.
                            if let Some(value) = n.value.clone().filter(|v| !v.is_empty()) {
                                entry["value"] = json!(value);
                            }
                            if let Some(parent) = n.parent_element_index {
                                entry["parent_index"] = json!(parent);
                            }
                            if let Some((l, t, r, b)) = n.rect {
                                entry["frame"] = json!({
                                    "x": l,
                                    "y": t,
                                    "w": (r - l).max(0),
                                    "h": (b - t).max(0),
                                });
                            }
                            Some(entry)
                        })
                        .collect();
                    structured["elements"] = json!(elements);
                    // Surface 6: snapshot id mirror for debug correlation.
                    structured["snapshot_id"] =
                        json!(cua_driver_core::element_token::token_for(snapshot_id, 0)
                            .trim_end_matches(":0")
                            .to_string());
                    structured["_note"] = json!(
                        "Prefer `elements` — `tree_markdown` will continue to work \
                         but new fields will only be added to the structured side. \
                         Issue #22865: use `max_elements` / `max_depth` to bound the \
                         UIA walk on apps with very large trees."
                    );
                    // Best-effort-background ladder: a UIA walk that ran but found
                    // zero actionable elements is NOT a clean snapshot — the window
                    // may be a non-UIA surface (canvas/WebGL/custom-drawn) or its
                    // tree wasn't ready (Chromium/Electron need an enable + settle).
                    // Mark it degraded so callers don't read `elements: []` as "this
                    // window has no controls", and point at the next rung. An empty
                    // UIA tree means element_index has nothing to bind to, so the
                    // deliberate move is an element px action — read the screenshot
                    // already in this response and click by pixel (x,y).
                    if count == 0 {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(
                            "ax_tree_empty: the UIA walk returned no actionable elements. \
                             The window may be a non-UIA surface (canvas/WebGL/custom-drawn) \
                             or its accessibility tree was not ready (Chromium/Electron \
                             require a UIA-enable + settle). Do not treat element data as \
                             authoritative — re-snapshot if the app just launched, otherwise \
                             switch to the visual path."
                        );
                        structured["escalation"] = json!({
                            "recommended": "px",
                            "reason": "non-AX surface — act by pixel (x,y) off the \
                                       screenshot in this response (an element px action)."
                        });
                    }
                }

                if let Some((b64_opt, file_path, w, h, orig_w)) = screenshot_opt {
                    if let Some(ow) = orig_w {
                        if w > 0 {
                            state.resize_registry.set_ratio(pid, ow as f64 / w as f64);
                        }
                    } else {
                        state.resize_registry.clear_ratio(pid);
                    }
                    // base64 is embedded only when no out_file was given (vision
                    // path). With `screenshot_out_file` the bytes went to disk and
                    // we surface the path instead — never both.
                    if let Some(b64) = b64_opt {
                        content.push(cua_driver_core::protocol::Content::image_png(b64));
                    }
                    structured["screenshot_width"] = json!(w);
                    structured["screenshot_height"] = json!(h);
                    // Surface 7: mirror the MCP image part's `mimeType` onto
                    // the structured payload so consumers don't have to sniff
                    // magic bytes off the base64 to know the format.
                    structured["screenshot_mime_type"] = json!("image/png");
                    if let Some(fp) = file_path {
                        structured["screenshot_file_path"] = json!(fp);
                    }
                } else if let Some(err) = screenshot_err {
                    // Capture failed (most commonly because the target is
                    // minimized — see #1973). Surface the reason in BOTH the
                    // human-readable content stream (so the model sees it
                    // alongside the UIA tree it does get) AND structuredContent
                    // (so MCP clients with structured-only parsing can detect
                    // and act on it). Without this the caller saw an empty
                    // response with no clue why and burned turns retrying.
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "screenshot unavailable: {err}"
                    )));
                    structured["screenshot_error"] = json!(err);
                }

                ToolResult {
                    content,
                    is_error: None,
                    structured_content: Some(structured),
                }
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
    if trimmed.is_empty() {
        return (String::new(), String::new());
    }
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
    let candidate = if file.is_empty() {
        target
    } else {
        file.as_str()
    };
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
            extra_args.insert(0, format!("--disable-features={OCCLUSION_FEATURE}"));
        }
    }

    // Boolean toggles — insert at front only when not already present.
    for flag in &[
        "--disable-backgrounding-occluded-windows",
        "--disable-renderer-backgrounding",
    ] {
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
async fn restore_foreground_polling_best_effort(prior_foreground_addr: usize, spawned_pid: u32) {
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

fn contains_remote_debugging_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("--remote-debugging-port") || lower.contains("--remote-debugging-pipe")
}

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
                The macOS-specific `webkit_inspector_port`, \
                `creates_new_application_instance`, and `additional_arguments` fields are \
                accepted; `additional_arguments` is honored (forwarded as ShellExecuteEx \
                parameters or as the AUMID activation arguments string). \
                The remaining macOS-only fields currently no-op on Windows.".into(),
            input_schema: json!({"type":"object","properties":{
                "bundle_id":{"type":"string","description":"App User Model ID (AUMID) for a packaged app — pattern `{PackageFamilyName}!{ApplicationId}`, e.g. `Microsoft.WindowsNotepad_8wekyb3d8bbwe!App`. Falls back to a `name` alias if no `!` is present. Either bundle_id, name, aumid, path, or launch_path must be provided."},
                "aumid":{"type":"string","description":"Explicit AUMID for a packaged app. Cleaner alternative to overloading `bundle_id`. Takes precedence over `bundle_id` / `name` when present."},
                "name":{"type":"string","description":"App display name. Tried against the `shell:AppsFolder` index first for packaged-app lookup; on a miss, passed to ShellExecuteEx's PATH search."},
                "path":{"type":"string","description":"Full path to executable. Windows-only; takes precedence over name/bundle_id/aumid (but not over `launch_path`)."},
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps`. Highest precedence — when set, this exact string is handed to ShellExecuteEx unchanged. For Windows desktop apps it's the full `.exe` commandline (path + arguments preserved from the source shortcut); for UWP apps it's `shell:appsFolder\\{PackageFamilyName}!{AppId}`. For precise UWP pid capture, prefer `aumid` over `launch_path`."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open in the default browser via ShellExecuteEx (no activation)."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process (or activation arguments for packaged apps)."},
                "webkit_inspector_port":{"type":"integer","description":"Accepted for cross-platform parity; no-op on Windows."},
                "creates_new_application_instance":{"type":"boolean","description":"Accepted for parity; no-op on Windows (ShellExecuteEx always creates a new process)."},
                "start_minimized":{"type":"boolean","description":"When true, launch the app's window minimized to the taskbar instead of restored-but-not-activated. Use this when the agent wants to drive the app entirely in the background — the user's previously-frontmost window (e.g. terminal) stays visually on top. Desktop launches hold the foreground lock through startup and use SW_SHOWMINNOACTIVE; packaged-app activation remains broker-controlled and receives a best-effort SW_SHOWMINNOACTIVE post-pass. UIA / background dispatch still work on a minimized window; only `screenshot` and `delivery_mode:\"foreground\"` need it restored."},
                "workspace_id":{"type":"string","description":"Optional live native_space workspace. The first resolved top-level windows are moved to it and verified before success is returned."},
                "session":{"type":"string","description":"Optional session id; inherits its workspace binding when workspace_id is omitted."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let workspace_id = match cua_driver_core::workspace::resolve_workspace_id(&args) {
            Ok(value) => value,
            Err(error) => {
                return ToolResult::error(error.to_string()).with_structured(json!({
                    "code": error.code(),
                }))
            }
        };
        if let Some(workspace_id) = workspace_id.as_deref() {
            let manager = cua_driver_core::workspace::default_manager()
                .expect("workspace id resolution requires a manager");
            let record = match manager.get(workspace_id) {
                Ok(record) => record,
                Err(error) => {
                    return ToolResult::error(error.to_string()).with_structured(json!({
                        "code": error.code(),
                        "workspace_id": workspace_id,
                        "app_launched": false,
                    }))
                }
            };
            if !record.capabilities.launch {
                return ToolResult::error(
                    "Windows native_space cannot safely route a third-party launch with the documented IVirtualDesktopManager API",
                )
                .with_structured(json!({
                    "code": "workspace_operation_unsupported",
                    "workspace_id": workspace_id,
                    "app_launched": false,
                }));
            }
        }
        let launch_path_opt = args
            .get("launch_path")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let path_opt = args.get("path").and_then(|v| v.as_str()).map(str::to_owned);
        let name_opt = args.get("name").and_then(|v| v.as_str()).map(str::to_owned);
        let bundle_id_opt = args
            .get("bundle_id")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let aumid_opt = args
            .get("aumid")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let urls: Vec<String> = args
            .get("urls")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let start_minimized = args
            .get("start_minimized")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        // `nShow` for the ShellExecuteEx-based launches. SW_SHOWNOACTIVATE
        // keeps the new window from grabbing focus but still renders it on
        // top of the previous frontmost window. SW_SHOWMINNOACTIVE both
        // skips activation AND drops the window to the taskbar, which is
        // what an agent doing background work wants — the user's terminal
        // stays visually on top. UIA / PostMessage / set_value all keep
        // working against a minimized window; only screenshot and
        // delivery_mode:"foreground" pay a cost.
        let n_show: i32 = if start_minimized {
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWMINNOACTIVE.0
        } else {
            windows::Win32::UI::WindowsAndMessaging::SW_SHOWNOACTIVATE.0
        };
        // Snapshot the set of currently-running pids BEFORE we launch. The
        // detached start_minimized polling task uses this to detect "any
        // process that came into existence as a result of our launch", so
        // it can minimize their windows too — covers the launcher-stub
        // case (LibreOffice swriter.exe → soffice.bin, GIMP gimp-3.exe →
        // gimp-3.2.exe, etc.) where the launched pid exits and a child
        // process with a different name owns the actual window. Captured
        // here (before the launch) so the new soffice.bin pid won't be
        // in the snapshot.
        let pre_launch_pids: std::sync::Arc<std::collections::HashSet<u32>> =
            if start_minimized || workspace_id.is_some() {
                let pids: std::collections::HashSet<u32> = crate::win32::list_processes()
                    .into_iter()
                    .map(|p| p.pid)
                    .collect();
                std::sync::Arc::new(pids)
            } else {
                std::sync::Arc::new(std::collections::HashSet::new())
            };

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
        let foreground_before_addr: usize =
            unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize };

        // Pure-function decision on whether to fire the post-launch
        // restore. Shape mirrors the resolved params — see
        // `should_restore_foreground_after_launch` for the rule.
        let launch_shape = LaunchTargetShape {
            has_aumid: aumid_opt.is_some(),
            has_bundle_id: bundle_id_opt.is_some(),
            has_name: name_opt.is_some(),
            has_path: path_opt.is_some(),
            has_launch_path: launch_path_opt.is_some(),
            has_urls: !urls.is_empty(),
        };
        let restore_foreground = should_restore_foreground_after_launch(launch_shape);
        let mut extra_args: Vec<String> = args
            .get("additional_arguments")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        if args.get("cdp_debugging_port").is_some() {
            return ToolResult::error(
                "cdp_debugging_port moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }
        if launch_path_opt
            .as_deref()
            .into_iter()
            .chain(path_opt.as_deref())
            .chain(name_opt.as_deref())
            .chain(extra_args.iter().map(String::as_str))
            .any(contains_remote_debugging_flag)
        {
            return ToolResult::error(
                "Chromium remote-debugging flags moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }

        // Resolve target — launch_path > path > aumid > name > bundle_id
        // (alias of name on Windows). launch_path is highest precedence for
        // round-trip with list_apps; the value is handed to ShellExecuteEx
        // unchanged (UWP routing below is bypassed when launch_path is set).
        let target = launch_path_opt
            .clone()
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
        } else if let Some(b) = bundle_id_opt
            .clone()
            .filter(|s| crate::launch_uwp::is_aumid(s))
        {
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

        // Strict no-activation is available only for the desktop launch path.
        // UWP activation is broker-controlled and retains its existing
        // restore-after-activation behavior.
        let mut foreground_lock = if start_minimized && aumid_for_uwp.is_none() {
            Some(crate::input::ForegroundLockGuard::acquire())
        } else {
            None
        };
        if foreground_lock
            .as_ref()
            .is_some_and(|guard| !guard.acquired())
        {
            return ToolResult::error(
                "Background minimized launch is unavailable because Windows did not grant the \
                 foreground lock required to prevent the new process from activating. No process \
                 was started. Launch without start_minimized only when a foreground change is \
                 acceptable.",
            )
            .with_structured(json!({
                "code": "background_unavailable",
                "delivery_mode": "background",
                "event_kind": "app_launch",
            }));
        }

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
                Ok(Err(e)) => {
                    return ToolResult::error(format!(
                        "Failed to activate packaged app {aumid:?}: {e}"
                    ))
                }
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        } else {
            // Legacy ShellExecuteExW path — unchanged behavior for plain
            // Win32 apps and for callers passing an explicit `path` /
            // `launch_path`.
            let urls_clone = urls.clone();
            let target_for_shell = target_file_opt.clone();
            let extra_for_shell = extra_joined.clone();
            let n_show_for_shell = n_show;
            let direct_minimized_exe = start_minimized
                && target_for_shell.as_deref().is_some_and(|target| {
                    std::path::Path::new(target).is_file()
                        && std::path::Path::new(target)
                            .extension()
                            .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                });
            // Bound the shell launch with a timeout. An unregistered protocol
            // or file association makes `ShellExecuteExW` block on a modal shell
            // dialog ("you'll need a new app to open this …") on the *session*
            // desktop — which a Session-0/headless daemon can neither see nor
            // dismiss — wedging the launch (and that request) indefinitely.
            // `SEE_MASK_FLAG_NO_UI` (below) suppresses the no-association error
            // UI so that case fails fast with an error code; the timeout is the
            // backstop for any *other* blocking broker dialog (SmartScreen, an
            // elevation/consent surface) so a bad target can't hang the daemon.
            let launch = tokio::task::spawn_blocking(move || -> anyhow::Result<u32> {
                use windows::core::{PCWSTR, PWSTR};
                use windows::Win32::Foundation::CloseHandle;
                use windows::Win32::System::Threading::{
                    CreateProcessW, GetProcessId, PROCESS_CREATION_FLAGS, PROCESS_INFORMATION,
                    STARTF_USESHOWWINDOW, STARTUPINFOW,
                };
                use windows::Win32::UI::Shell::{
                    ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS,
                    SHELLEXECUTEINFOW,
                };

                fn to_wide(s: &str) -> Vec<u16> {
                    s.encode_utf16().chain(std::iter::once(0)).collect()
                }
                let op_w = to_wide("open");
                let file_w = to_wide(if let Some(t) = target_for_shell.as_deref() {
                    t
                } else {
                    urls_clone.first().map(|s| s.as_str()).unwrap_or("")
                });
                let args_w = to_wide(&extra_for_shell);

                let pid = if direct_minimized_exe {
                    let target = target_for_shell
                        .as_deref()
                        .expect("checked executable path");
                    let mut command_line = to_wide(&if extra_for_shell.is_empty() {
                        format!(r#""{target}""#)
                    } else {
                        format!(r#""{target}" {extra_for_shell}"#)
                    });
                    let startup = STARTUPINFOW {
                        cb: std::mem::size_of::<STARTUPINFOW>() as u32,
                        dwFlags: STARTF_USESHOWWINDOW,
                        wShowWindow: n_show_for_shell as u16,
                        ..Default::default()
                    };
                    let mut process = PROCESS_INFORMATION::default();
                    unsafe {
                        CreateProcessW(
                            PCWSTR(file_w.as_ptr()),
                            PWSTR(command_line.as_mut_ptr()),
                            None,
                            None,
                            false,
                            PROCESS_CREATION_FLAGS(0),
                            None,
                            PCWSTR::null(),
                            &startup,
                            &mut process,
                        )?;
                        let _ = CloseHandle(process.hThread);
                        let _ = CloseHandle(process.hProcess);
                    }
                    process.dwProcessId
                } else {
                    let mut info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
                        lpVerb: PCWSTR(op_w.as_ptr()),
                        lpFile: PCWSTR(file_w.as_ptr()),
                        lpParameters: if extra_for_shell.is_empty() {
                            PCWSTR::null()
                        } else {
                            PCWSTR(args_w.as_ptr())
                        },
                        nShow: n_show_for_shell,
                        ..Default::default()
                    };
                    unsafe {
                        ShellExecuteExW(&mut info)?;
                    }
                    if !info.hProcess.is_invalid() {
                        let p = unsafe { GetProcessId(info.hProcess) };
                        unsafe {
                            let _ = CloseHandle(info.hProcess);
                        }
                        p
                    } else {
                        0
                    }
                };

                // Open any additional URLs in the default browser (no focus
                // steal, no pid capture for these — Swift's NSWorkspace flow
                // behaves the same way for secondary URLs).
                for url in &urls_clone[urls_clone.len().min(1)..] {
                    let file = to_wide(url);
                    let mut url_info = SHELLEXECUTEINFOW {
                        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
                        fMask: SEE_MASK_FLAG_NO_UI,
                        lpVerb: PCWSTR(op_w.as_ptr()),
                        lpFile: PCWSTR(file.as_ptr()),
                        nShow: n_show_for_shell,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = ShellExecuteExW(&mut url_info);
                    }
                }

                Ok(pid)
            });
            // 15s is generous: ShellExecuteExW returns once activation starts
            // (process created), not when the window appears, so a healthy
            // launch resolves in well under a second. On timeout we abandon the
            // blocking task (a spawn_blocking thread can't be cancelled — it
            // unblocks if/when the modal is dismissed) and return an error so
            // the daemon stays responsive instead of wedging on the request.
            let result = tokio::time::timeout(std::time::Duration::from_secs(15), launch).await;

            match result {
                Ok(Ok(Ok(p))) => p,
                Ok(Ok(Err(e))) => return ToolResult::error(format!("Failed to launch: {e}")),
                Ok(Err(e)) => return ToolResult::error(format!("Task error: {e}")),
                Err(_elapsed) => {
                    return ToolResult::error(format!(
                        "Launch of {:?} timed out after 15s — the target likely has no \
                     registered handler and a blocking shell dialog appeared on the \
                     session desktop; aborted to keep the daemon responsive.",
                        target_file_opt
                            .as_deref()
                            .or_else(|| urls.first().map(|s| s.as_str()))
                            .unwrap_or("")
                    ))
                }
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
            let n_show_for_urls = n_show;
            let _ = tokio::task::spawn_blocking(move || {
                use windows::core::PCWSTR;
                use windows::Win32::UI::Shell::{ShellExecuteExW, SHELLEXECUTEINFOW};
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
                        nShow: n_show_for_urls,
                        ..Default::default()
                    };
                    unsafe {
                        let _ = ShellExecuteExW(&mut url_info);
                    }
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

        // UWP / packaged-app host-window resolution. `launch_uwp` returns the
        // real packaged-app pid (e.g. CalculatorApp.exe), but a UWP app's
        // top-level window — the HWND a caller must actually drive — is owned
        // by `ApplicationFrameHost.exe`, not by that process. The app process
        // only owns a `CoreWindow` reparented as a child of the AFH frame, so
        // `list_windows(Some(app_pid))` is empty and the generic retry loop
        // below would either spin out to a stub-fallback miss or return no
        // windows at all (the original "launch_app of a UWP app returns empty
        // windows[] / a pid you can't drive" bug). Map app_pid → its AFH host
        // frame and report `(frame_hwnd, afh_pid)` so the returned handles
        // resolve in `get_window_state` and element actions land. Retry: the
        // frame can lag the activation by a few hundred ms.
        if aumid_for_uwp.is_some() {
            for _ in 0..10 {
                let host =
                    tokio::task::spawn_blocking(move || crate::win32::resolve_uwp_host_window(pid))
                        .await
                        .unwrap_or(None);
                if let Some(w) = host {
                    windows_json = vec![json!({
                        "window_id": w.hwnd, "title": w.title,
                        "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
                        "layer": 0,
                        "z_index": 0,
                        "is_on_screen": true,
                    })];
                    // Report the AFH pid: that's the pid that owns the frame
                    // HWND, so `get_window_state(resolved_pid, window_id)`
                    // validates and `list_windows(Some(resolved_pid))` lists it.
                    resolved_pid = w.pid;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            }
        }

        for _ in 0..5 {
            if !windows_json.is_empty() {
                break;
            }
            let wins = tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                .await
                .unwrap_or_default();
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
                if !is_slow_launcher || total_attempts > 60 {
                    break;
                }
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

        if let Some(workspace_id) = workspace_id.as_deref() {
            if pre_launch_pids.contains(&pid) {
                return ToolResult::error(format!(
                    "App reused pre-existing pid {pid}; refusing to move its existing windows into workspace '{workspace_id}'"
                ))
                .with_structured(json!({
                    "code": "workspace_existing_instance",
                    "workspace_id": workspace_id,
                    "pid": pid,
                    "app_launched": false,
                }));
            }
            if windows_json.is_empty() {
                return ToolResult::error(format!(
                    "App launched as pid {pid}, but no top-level window appeared to move into workspace '{workspace_id}'"
                ))
                .with_structured(json!({
                    "code": "workspace_window_not_found",
                    "workspace_id": workspace_id,
                    "pid": pid,
                    "app_launched": true,
                }));
            }
            let manager = cua_driver_core::workspace::default_manager()
                .expect("workspace id resolution requires a manager");
            for window in &windows_json {
                let Some(window_id) = window.get("window_id").and_then(Value::as_i64) else {
                    continue;
                };
                if let Err(error) = manager
                    .move_window(
                        workspace_id,
                        cua_driver_core::workspace::WindowTarget {
                            window_id,
                            pid: Some(i64::from(pid)),
                        },
                    )
                    .await
                {
                    return ToolResult::error(format!(
                        "App launched as pid {pid}, but window {window_id} could not be moved to workspace '{workspace_id}': {error}"
                    ))
                    .with_structured(json!({
                        "code": "workspace_move_failed",
                        "workspace_id": workspace_id,
                        "pid": pid,
                        "window_id": window_id,
                        "app_launched": true,
                    }));
                }
            }
            if let Err(error) = manager.note_launch(workspace_id, pid) {
                return ToolResult::error(format!(
                    "App launched as pid {pid}, but workspace ownership could not be recorded: {error}"
                ))
                .with_structured(json!({
                    "code": error.code(),
                    "workspace_id": workspace_id,
                    "pid": pid,
                    "app_launched": true,
                }));
            }
        }

        // start_minimized post-pass: drop every resolved window to the
        // taskbar. The desktop ShellExecuteEx path already minimizes via
        // nShow=SW_SHOWMINNOACTIVE, but three cases still need this
        // fallback:
        //   1. AUMID / UWP activations come up un-minimized —
        //      ActivateApplication has no nShow.
        //   2. Apps that ignore the nShow hint and decide their own geometry
        //      (notable offenders: LibreOffice, some Electron apps).
        //   3. Launcher-stub flows (LO swriter.exe → soffice.bin, GIMP, etc.)
        //      where the launched pid exits before the real window appears,
        //      so `windows_json` is empty at this point.
        //
        // To cover (3) we spawn a background polling task that keeps
        // looking for windows under the launched pid family for up to 5 s
        // and minimizes anything that materializes. The task runs detached
        // so the launch_app response isn't held up by it.
        //
        // SW_SHOWMINNOACTIVE preserves the foreground while minimizing. Using
        // SW_MINIMIZE here would itself activate the next z-order window and
        // force a visible restore-after-steal cycle.
        if start_minimized {
            // First, minimize anything already resolved (covers the common
            // single-process path where windows_json was populated).
            let immediate_hwnds: Vec<u64> = windows_json
                .iter()
                .filter_map(|w| w["window_id"].as_u64())
                .collect();
            // Capture pid + basename for the post-launch polling task to
            // pick up launcher-stub child processes.
            let stub_basename = target_file_opt
                .as_deref()
                .and_then(|t| t.rsplit(|c: char| c == '\\' || c == '/').next())
                .unwrap_or("")
                .to_owned();
            // parent_pid is the launched pid family root — used below to
            // constrain the post-launch minimize sweep to descendants /
            // name-relatives of the launched app, so we don't accidentally
            // minimize a user's unrelated app that started during the 5 s
            // poll window.
            let parent_pid = pid;
            let immediate_hwnds_for_poll = immediate_hwnds.clone();
            let _ = tokio::task::spawn_blocking(move || {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWMINNOACTIVE};
                for h in immediate_hwnds {
                    unsafe {
                        let _ = ShowWindow(HWND(h as *mut _), SW_SHOWMINNOACTIVE);
                    }
                }
            })
            .await;
            // Poll launcher-stub late-window cases before returning. The
            // start_minimized contract is observable at response time; a
            // detached task allowed callers to see a transient restored
            // window immediately after a successful launch.
            // Strategy: every 200 ms for 5 s, find pids that
            //   (a) weren't in the pre-launch snapshot, AND
            //   (b) are part of the launched app's family — either a
            //       descendant of the launched root pid, OR a process
            //       sharing the launched binary's name prefix (covers
            //       renamed-binary chains like swriter.exe → soffice.bin
            //       where the child's name doesn't match but it's a
            //       direct descendant).
            // Minimize THEIR visible windows only. Without the family
            // constraint, anything the user opens during the 5 s window
            // (Chrome, terminal, IDE) would also get minimized — that's
            // the regression CodeRabbit flagged.
            //
            // Loop ends early once we've minimized the first set of
            // windows AND they remain minimized for three ticks — the typical
            // app has its main window up within ~2 s of launch.
            let pre_pids = pre_launch_pids.clone();
            let basename_for_poll = stub_basename.clone();
            let launch_foreground_lock = foreground_lock.take();
            (async move {
                use std::collections::HashSet;
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{
                    IsIconic, ShowWindow, SW_SHOWMINNOACTIVE,
                };
                let _foreground_lock = launch_foreground_lock;
                let mut minimized: HashSet<u64> = immediate_hwnds_for_poll.into_iter().collect();
                let mut idle_ticks_after_any_hit: u8 = 0;
                let mut hit_count_total = minimized.len();
                for _ in 0..25 {
                    let pre_pids_clone = pre_pids.clone();
                    let basename_clone = basename_for_poll.clone();
                    // Build the family pid set fresh each tick — both
                    // descendant graph and name-relatives can grow as
                    // launcher-stub chains spawn deeper children.
                    let family_new_pids: Vec<u32> = tokio::task::spawn_blocking(move || {
                        let related: std::collections::HashSet<u32> =
                            crate::win32::related_processes(parent_pid, &basename_clone)
                                .into_iter()
                                .chain(std::iter::once(parent_pid))
                                .collect();
                        crate::win32::list_processes()
                            .into_iter()
                            .filter(|p| {
                                !pre_pids_clone.contains(&p.pid) && related.contains(&p.pid)
                            })
                            .map(|p| p.pid)
                            .collect()
                    })
                    .await
                    .unwrap_or_default();
                    let mut tick_hits: usize = 0;
                    for cpid in family_new_pids {
                        let wins = tokio::task::spawn_blocking(move || {
                            crate::win32::list_windows(Some(cpid))
                        })
                        .await
                        .unwrap_or_default();
                        for w in wins {
                            let is_new = minimized.insert(w.hwnd);
                            if is_new {
                                hit_count_total += 1;
                            }
                            let hwnd_iso = w.hwnd as usize;
                            let restored = tokio::task::spawn_blocking(move || unsafe {
                                let hwnd = HWND(hwnd_iso as *mut _);
                                if IsIconic(hwnd).as_bool() {
                                    false
                                } else {
                                    let _ = ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
                                    true
                                }
                            })
                            .await
                            .unwrap_or(false);
                            if is_new || restored {
                                tick_hits += 1;
                            }
                        }
                    }
                    if tick_hits == 0 {
                        idle_ticks_after_any_hit += 1;
                        if hit_count_total > 0 && idle_ticks_after_any_hit >= 3 {
                            // Stable: known windows remained minimized and no
                            // new window appeared for 600 ms.
                            break;
                        }
                    } else {
                        idle_ticks_after_any_hit = 0;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            })
            .await;
        }

        // Match Swift text format 1:1.
        let mut summary = format!("✅ Launched {display} (pid {pid}) in background.");
        if !windows_json.is_empty() {
            summary.push_str("\n\nWindows:");
            for w in &windows_json {
                let title = w["title"].as_str().unwrap_or("");
                let title_disp = if title.is_empty() {
                    "(no title)".to_owned()
                } else {
                    format!("\"{title}\"")
                };
                let wid = w["window_id"].as_u64().unwrap_or(0);
                summary.push_str(&format!("\n- {title_disp} [window_id: {wid}]"));
            }
            summary.push_str(&format!(
                "\n→ Call get_window_state(pid: {pid}, window_id) to inspect."
            ));
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
            "workspace_id": workspace_id,
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
            // extras (`button`, no `modifier`/`action`/`debug_image_out`) apply here.
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
                returned by `get_window_state`). On `delivery_mode:\"background\"` (the \
                default), cua-driver FIRST does a UIA hit-test at the resolved \
                screen position: if the deepest invokable element under that point exposes \
                `InvokePattern`, it's invoked through the accessibility channel — same \
                background-safe path as the `element_index` mode, no foreground swap, no \
                visible flash. **This makes pixel clicks on UWP / WinUI3 / Win11 packaged \
                apps work flash-free out of the box** — agents should default to \
                `delivery_mode:\"background\"` even on XAML hosts whose CoreInput dispatcher \
                drops raw PostMessage. The PostMessage(WM_LBUTTONDOWN/UP) fallback only \
                runs when the UIA hit-test misses (canvas / video / WebGL / custom-drawn \
                surfaces with no UIA peer), and on those targets `delivery_mode:\"background\"` \
                returns a structured `background_unavailable` error if PostMessage is \
                known to drop too — at which point you switch to `delivery_mode:\"foreground\"`. \
                **Always try the default `background` click first and let that error be \
                your signal — do NOT pass `foreground` preemptively because the target \
                'looks like' GTK/Chromium/etc. The driver decides when background is \
                impossible; a guessed foreground click needlessly steals the user's focus.** \
                `count: 2` posts two down/up pairs for a double-click. Pixel clicks need \
                a visible on-screen window to anchor the coordinate conversion (errors \
                with `pid X has no on-screen window` otherwise).\n\n\
                Exactly one of `element_index` or (`x` AND `y`) must be provided. \
                `pid` is required for window scope and omitted for desktop scope. `window_id` is required when \
                `element_index` is used (scopes the cache lookup). After a `zoom` call, \
                pass `from_zoom=true` to auto-translate zoom-image coords back to \
                full-window space.\n\n\
                Windows-only convenience: `button: \"left\"|\"right\"|\"middle\"` switches \
                the mouse button (Swift exposes right-click as a separate `right_click` \
                tool). The Swift-only `action` / `modifier` / `debug_image_out` schema \
                fields aren't supported yet.".into(),
            input_schema: json!({
                "type":"object","properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID for window scope. Omit with scope=desktop for screen-absolute coordinates from get_desktop_state."},
                    "window_id":{"type":"integer","description":"HWND for the window whose get_window_state produced the element_index. Required when element_index is used. Optional when element_token is supplied (the token carries it)."},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"X in window-local screenshot pixels — same space as the PNG get_window_state returns. Must be provided together with y."},
                    "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                    "button": cua_driver_core::tool_schema::button_schema(),
                    "count":{"type":"integer","minimum":1,"maximum":3,"description":"Click count — 1 (single), 2 (double), 3 (triple). Default 1."},
                    "modifier": cua_driver_core::tool_schema::modifier_schema(),
                    "from_zoom":{"type":"boolean","description":"When true, x and y are pixel coordinates in the last `zoom` image for this pid. The driver maps them back to window coords."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use crate::uia::cache::SnapshotKind;
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);

        // ── Window-less screen-absolute branch (capture_scope="desktop") ──────
        // When the caller gives x,y with NO pid/window_id, treat x,y as TRUE
        // SCREEN pixels. The core registry has already enforced the session's
        // effective capture scope; this local check validates the action form.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if !has_pid && !has_window_id && has_xy {
            if !is_windowless_desktop_action(&args) {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id requires scope=\"desktop\"; \
                     use get_desktop_state to pick coordinates, or pass a pid/window_id.",
                )
                .with_structured(json!({
                    "code": "desktop_coordinate_scope_required",
                    "suggestion": "pass scope=desktop",
                }));
            }
            let input = match parse_typed_projection::<ClickInput>("click", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let button = input
                .button
                .unwrap_or(ClickButton::Left)
                .as_str()
                .to_owned();
            let count = input.count.unwrap_or(1) as usize;
            if count == 0 {
                return ToolResult::error("click.count must be at least 1.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let modifiers: Vec<String> = args.str_array("modifier");
            let sx = input.x as i32;
            let sy = input.y as i32;

            // Resolve the application window before moving the agent cursor.
            // WindowFromPoint can return a transparent layered overlay, and the
            // cursor overlay is about to occupy this exact screen point.
            let root = unsafe {
                use windows::Win32::Foundation::POINT;
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetAncestor, WindowFromPoint, GA_ROOT,
                };
                let target = WindowFromPoint(POINT { x: sx, y: sy });
                if target.0.is_null() {
                    return ToolResult::error(format!("No window under screen point ({sx},{sy})."));
                }
                let root = GetAncestor(target, GA_ROOT);
                if root.0.is_null() {
                    target
                } else {
                    root
                }
            };
            let hwnd_u = root.0 as u64;

            // Animate the agent cursor to the screen point, then click.
            overlay_glide_to(&cursor_key, sx as f64, sy as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx as f64,
                    y: sy as f64,
                },
            );

            // Click the HWND that owned the pixel before the driver overlay
            // moved there. The active SendInput path performs the foreground
            // swap and UIPI checks needed for Chromium and retained-mode apps.
            let send_result = tokio::task::spawn_blocking(move || -> anyhow::Result<u64> {
                let mod_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                crate::input::send_click_synthesized_active_mods(
                    hwnd_u, sx, sy, count, &button, &mod_refs,
                )?;
                Ok(hwnd_u)
            })
            .await;
            return match send_result {
                Ok(Ok(hwnd_u)) => {
                    let click_word = match count {
                        2 => "double-click",
                        3 => "triple-click",
                        _ => "click",
                    };
                    ToolResult::text(format!(
                        "✅ Sent {click_word} via SendInput at screen ({sx},{sy}) on HWND 0x{hwnd_u:x} (desktop scope)."
                    ))
                    // SendInput dispatched but is not driver-verifiable (no read-back).
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" }))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Surface 6: element_token / element_index precedence resolution.
        // Windows uses u64 HWND but the token registry stores u32; truncate
        // through the same path get_window_state used when registering.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        // Surface 5: explicit rejection of unknown buttons so a typo doesn't fall
        // through to the SendInput `_ => MOUSEEVENTF_LEFTDOWN/UP` default and
        // silently emit a left-click. Empty / missing keeps back-compat.
        let button_raw = args.str_or("button", "left").to_lowercase();
        if !matches!(button_raw.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_raw}\" — expected one of left, right, middle."
            ));
        }
        let button = if button_raw.is_empty() {
            "left".to_string()
        } else {
            button_raw
        };
        let count = args.u64_or("count", 1) as usize;
        // macOS click `modifier` surface: held keys (cmd/shift/option/ctrl). Only
        // the SendInput rung (delivery_mode:"foreground" pixel tap, and the
        // window-less desktop-scope click above) can hold them; the background
        // UIA-Invoke / PostMessage rungs have no keyboard state to carry them, so
        // a modifier passed to a background click is necessarily ignored there.
        let modifiers: Vec<String> = args.str_array("modifier");
        let delivery = DeliveryMode::from_args(&args);
        // For every non-foreground click, mark the target window
        // non-activatable (WS_EX_NOACTIVATE) for the duration so a target that
        // self-activates in its UIA-Invoke / click handler (WPF
        // `UIElement.Focus()`, XAML, Tauri/WebView2) CANNOT steal the user's
        // foreground — the window still receives the click. Held for the whole
        // invoke; a no-op for delivery_mode:"foreground" (which wants the swap) and
        // when no window_id was given.
        let _noact = match (delivery != DeliveryMode::Foreground, hwnd_opt) {
            (true, Some(h)) => Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(h as *mut _),
            )),
            _ => None,
        };
        // Optional `action` arg picks among the actions exposed in the
        // accessibility tree. Today this only changes behavior for MSAA
        // BUTTONDROPDOWN: `"expand"` clicks the right-edge (dropdown arrow
        // half) instead of the center (press half). Defaults to first
        // action in the element's `actions=[...]` list, which preserves
        // existing semantics for UIA elements.
        let action_req = args
            .get("action")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        if let Some(idx) = elem_idx {
            // ── MSAA dispatch (SAL/VCL targets) ────────────────────────────
            // For MSAA elements, route by role:
            //   - `action:"expand"` on BUTTONDROPDOWN → SendInput at the
            //     cached rect's right-edge (the dropdown arrow half).
            //     Opens the picker (e.g. LO Writer Font Color → SALTMPSUBFRAME).
            //   - default action / `"invoke"` → SendInput at center
            //     (matches `accDoDefaultAction` semantics for VCL controls
            //     and works through delivery_mode:"foreground"). We DO NOT call
            //     `accDoDefaultAction` here because LO's MSAA impl applies
            //     the change asynchronously and returns S_OK either way,
            //     so the visible behavior is the same as a center click.
            if let Some((SnapshotKind::Msaa, role)) = self
                .state
                .element_cache
                .get_element_kind_and_role(pid, hwnd, idx)
            {
                const ROLE_BUTTONDROPDOWN: i32 = 0x38;
                const ROLE_BUTTONMENU: i32 = 0x39;
                const ROLE_BUTTONDROPDOWNGRID: i32 = 0x3A;
                const ROLE_SPLITBUTTON: i32 = 0x3E;
                let want_expand = action_req.as_deref() == Some("expand");
                let is_dropdown_role = matches!(
                    role,
                    Some(
                        ROLE_BUTTONDROPDOWN
                            | ROLE_BUTTONMENU
                            | ROLE_BUTTONDROPDOWNGRID
                            | ROLE_SPLITBUTTON
                    )
                );
                let (tx, ty) = if want_expand && is_dropdown_role {
                    match self.state.element_cache.get_element_rect(pid, hwnd, idx) {
                        Some((_l, t, r, b)) => {
                            // Right-edge of the SplitButton — the dropdown
                            // arrow half. -4 puts the click safely inside
                            // the arrow region for the typical ~12-16 px
                            // arrow width VCL uses.
                            (r - 4, (t + b) / 2)
                        }
                        None => {
                            return ToolResult::error(format!(
                                "MSAA element [{idx}] has no cached rect — \
                                 cannot dispatch action:\"expand\". Re-run \
                                 get_window_state to refresh the cache."
                            ));
                        }
                    }
                } else if want_expand && !is_dropdown_role {
                    return ToolResult::error(format!(
                        "action:\"expand\" requested for MSAA element [{idx}] \
                         but its role ({role:?}) is not a dropdown button. \
                         Use action:\"invoke\" or omit the action arg."
                    ));
                } else {
                    match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                        Some(v) => v,
                        None => {
                            return ToolResult::error(format!(
                                "MSAA element [{idx}] not in cache for hwnd={hwnd}. \
                             Call get_window_state first."
                            ))
                        }
                    }
                };
                pin_overlay_above(&cursor_key, hwnd);
                overlay_glide_to(&cursor_key, tx as f64, ty as f64).await;
                crate::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::ClickPulse {
                        x: tx as f64,
                        y: ty as f64,
                    },
                );
                let btn_fg = button.clone();
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, tx, ty, count, &btn_fg)
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                let half = if want_expand { "dropdown" } else { "press" };
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Performed SendInput click on MSAA [{idx}] {half} half at ({tx},{ty})."
                    ))
                    .with_structured(
                        json!({ "path": "msaa", "verified": false, "effect": "unverifiable" }),
                    ),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }

            // UIA path: get cached center (no COM call needed — captured at walk time).
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => {
                    return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                }
            };
            // NB: the off-screen guard is applied per-delivery-path below (the
            // foreground SendInput tap and the background coordinate injection),
            // NOT here — a background UIA Invoke fires the element's handler
            // regardless of whether the element is scrolled into view, so a broad
            // guard would wrongly block legitimate actions like opening a modal
            // from an off-screen button.
            // Step 2: pin overlay to target window, then animate to screen coords.
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, cx as f64, cy as f64).await;
            // Step 3: click pulse + actual click.
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
            let btn = button.clone();

            // An explicit accessibility action is a semantic request, not a
            // pixel gesture hint. Route expand through ExpandCollapsePattern
            // even when foreground delivery was allowed; this reliably opens
            // WPF/WinUI menus and tree nodes whose visual click target is
            // transient or scroll-adjusted.
            if action_req.as_deref() == Some("expand") {
                let state = self.state.clone();
                let expand = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                    use windows::core::Interface;
                    use windows::Win32::UI::Accessibility::{
                        IUIAutomationElement, IUIAutomationExpandCollapsePattern,
                        UIA_ExpandCollapsePatternId,
                    };

                    let retained = state
                        .element_cache
                        .get_element_retained(pid, hwnd, idx)
                        .ok_or_else(|| {
                            anyhow::anyhow!("element [{idx}] is not in the UIA cache")
                        })?;
                    if !retained.is_uia() {
                        anyhow::bail!("element [{idx}] is not a UIA element");
                    }
                    let element =
                        unsafe { IUIAutomationElement::from_raw(retained.as_ptr() as *mut _) };
                    let pattern = unsafe {
                        element
                            .GetCurrentPattern(UIA_ExpandCollapsePatternId)
                            .and_then(|value| value.cast::<IUIAutomationExpandCollapsePattern>())
                    }
                    .map_err(|error| {
                        anyhow::anyhow!("ExpandCollapsePattern unavailable: {error}")
                    })?;
                    let result =
                        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                            pattern.Expand()
                        });
                    std::mem::forget(element);
                    result.map_err(|error| anyhow::anyhow!("ExpandCollapse.Expand failed: {error}"))
                })
                .await;
                return match expand {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Expanded UIA element [{idx}] via ExpandCollapsePattern."
                    ))
                    .with_structured(json!({ "path": "uia_expand_collapse", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }

            // delivery_mode:"foreground" — skip UIA Invoke and use SendInput at the
            // cached element center. The caller explicitly chose foreground
            // delivery; UIA Invoke would be background-safe (which they
            // rejected) and might fail on elements that don't support
            // InvokePattern anyway.
            if delivery == DeliveryMode::Foreground {
                // Foreground delivery is a real SendInput tap at (cx,cy); if the
                // element is off-screen, scroll it into view and re-resolve so the
                // tap doesn't land on the taskbar, else keep the clean failure.
                let (cx, cy) = match resolve_onscreen_point_with_scroll(
                    &self.state.element_cache,
                    pid,
                    hwnd,
                    idx,
                    cx,
                    cy,
                    "a foreground click",
                ) {
                    Ok(p) => p,
                    Err(msg) => return ToolResult::error(msg),
                };
                let btn_fg = btn.clone();
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, cx, cy, count, &btn_fg)
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Performed SendInput click on [{idx}] at screen ({cx},{cy}) (delivery_mode:foreground)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // delivery_mode:"background" on WinUI3 for double / right / middle: single
            // left falls through to the UIA Invoke path below (already drives
            // WinUI3). Double-left lands via a double UIA Invoke; right/middle
            // can't both land and hold the contract → structured error.
            if delivery == DeliveryMode::Background
                && (count > 1 || btn == "right" || btn == "middle")
            {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, Some(idx), count, &btn).await
                {
                    return r;
                }
            }
            // Chromium's UIA Invoke raises a fully occluded renderer, while a
            // targeted PostMessage left click reaches the renderer without
            // changing foreground, z-order, or the real cursor. Keep AX for
            // target resolution and use the posted-message transport only for
            // the empirically verified single-left-click shape.
            if delivery == DeliveryMode::Background
                && btn == "left"
                && count == 1
                && crate::input::is_chromium_target_window(hwnd)
            {
                let posted = tokio::task::spawn_blocking(move || {
                    crate::input::post_click_screen(hwnd, cx, cy, count, &btn)
                })
                .await;
                return match posted {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Posted click on Chromium element [{idx}] at screen ({cx},{cy}) \
                         (background, no foreground swap)."
                    ))
                    .with_structured(json!({
                        "path": "post_message",
                        "verified": false,
                        "effect": "unverifiable"
                    })),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
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
                // Direct Chromium UIA Invoke can return S_OK without firing a
                // DOM event while occluded. Try the honest coordinate actuator
                // first: it lands while visible and reports occlusion without
                // raising the window when hidden.
                if delivery == DeliveryMode::Background
                    && crate::input::is_chromium_target_window(hwnd)
                {
                    let (cx, cy) = resolve_onscreen_point_with_scroll(
                        &state_clone.element_cache, pid, hwnd, idx, cx, cy, "clicking",
                    )
                    .map_err(|message| anyhow::anyhow!(message))?;
                    return crate::input::inject_click_screen(hwnd, cx, cy, count, &btn)
                        .map(|()| format!(
                            "✅ Injected click on [{idx}] (screen ({cx},{cy}), background, no foreground swap)."
                        ))
                        .map_err(|error| anyhow::anyhow!(
                            "__CUA_BG_UNAVAILABLE_CLICK__{error}"
                        ));
                }
                if use_uia_invoke {
                    // Retain the element out of the cache (AddRef under the
                    // cache lock) so it can't be freed by a concurrent
                    // get_window_state on the same (pid, hwnd) while this click
                    // is mid-flight (use-after-free → daemon crash). The guard
                    // lives to the end of this `if let` block, past every UIA
                    // pattern dispatch below; its Release fires when it drops.
                    if let Some(element_guard) = state_clone.element_cache.get_element_retained(pid, hwnd, idx) {
                        let ptr = element_guard.as_ptr();
                        use windows::Win32::UI::Accessibility::{
                            IUIAutomationElement, IUIAutomationInvokePattern,
                            IUIAutomationTogglePattern, IUIAutomationSelectionItemPattern,
                            IUIAutomationExpandCollapsePattern,
                            UIA_InvokePatternId, UIA_TogglePatternId,
                            UIA_SelectionItemPatternId, UIA_ExpandCollapsePatternId,
                        };
                        use windows::core::Interface;
                        let elem: IUIAutomationElement =
                            unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };

                        // Try patterns in order of click-semantics specificity:
                        //   1. Invoke      - buttons / hyperlinks (canonical)
                        //   2. Toggle      - checkboxes (no Invoke)
                        //   3. SelectionItem - radio buttons, listbox items
                        //   4. ExpandCollapse - combo boxes, tree nodes
                        // Each call is wrapped in the UWP foreground-steal bypass.
                        //
                        // Why this order: PR #1699's WinUI3 control parity tests
                        // surfaced these pattern-dispatch gaps. Without these
                        // fallthroughs, cua-driver couldn't drive WinUI3
                        // CheckBox / RadioButton / ComboBox because PostMessage
                        // doesn't reach their handlers (CoreInput dispatcher).
                        let invoke_result = unsafe { elem.GetCurrentPattern(UIA_InvokePatternId) };
                        if let Ok(pattern) = invoke_result {
                            if let Ok(inv) = pattern.cast::<IUIAutomationInvokePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { inv.Invoke() });
                                match outcome {
                                    Ok(()) => {
                                        std::mem::forget(elem);
                                        return Ok(format!(
                                            "✅ Performed UIA Invoke on [{idx}] (screen ({cx},{cy}))."
                                        ));
                                    }
                                    Err(e) => tracing::debug!(target: "click",
                                        "UIA Invoke on [{idx}]: {e}, trying Toggle"),
                                }
                            }
                        }
                        let toggle_result = unsafe { elem.GetCurrentPattern(UIA_TogglePatternId) };
                        if let Ok(pattern) = toggle_result {
                            if let Ok(tg) = pattern.cast::<IUIAutomationTogglePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { tg.Toggle() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(format!(
                                        "✅ Performed UIA Toggle on [{idx}] (screen ({cx},{cy}))."
                                    ));
                                }
                            }
                        }
                        let sel_result = unsafe { elem.GetCurrentPattern(UIA_SelectionItemPatternId) };
                        if let Ok(pattern) = sel_result {
                            if let Ok(si) = pattern.cast::<IUIAutomationSelectionItemPattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { si.Select() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(format!(
                                        "✅ Performed UIA SelectionItem.Select on [{idx}] (screen ({cx},{cy}))."
                                    ));
                                }
                            }
                        }
                        let exp_result = unsafe { elem.GetCurrentPattern(UIA_ExpandCollapsePatternId) };
                        if let Ok(pattern) = exp_result {
                            if let Ok(ec) = pattern.cast::<IUIAutomationExpandCollapsePattern>() {
                                let outcome = crate::uia::fg_bypass::run_with_uwp_bypass(
                                    hwnd as isize, || unsafe { ec.Expand() });
                                if outcome.is_ok() {
                                    std::mem::forget(elem);
                                    return Ok(format!(
                                        "✅ Performed UIA ExpandCollapse.Expand on [{idx}] (screen ({cx},{cy}))."
                                    ));
                                }
                            }
                        }
                        std::mem::forget(elem);
                    }
                }
                if delivery == DeliveryMode::Background
                    && crate::input::delivery::would_be_silently_dropped(
                        hwnd,
                        EventKind::MouseClick,
                    )
                {
                    let (cx, cy) = resolve_onscreen_point_with_scroll(
                        &state_clone.element_cache,
                        pid,
                        hwnd,
                        idx,
                        cx,
                        cy,
                        "clicking",
                    )
                    .map_err(|message| anyhow::anyhow!(message))?;
                    return crate::input::inject_click_screen(hwnd, cx, cy, count, &btn)
                        .map(|()| {
                            format!(
                                "✅ Injected click on [{idx}] (screen ({cx},{cy}), background, no foreground swap)."
                            )
                        })
                        .map_err(|error| {
                            anyhow::anyhow!("__CUA_BG_UNAVAILABLE_CLICK__{error}")
                        });
                }
                crate::input::post_click_screen(hwnd, cx, cy, count, &btn)?;
                let action_name = match btn.as_str() {
                    "right"  => "ShowMenu",
                    _        => "PostMessage click",
                };
                Ok(format!("✅ Performed {action_name} on [{idx}] (screen ({cx},{cy}))."))
            }).await;
            match result {
                // UIA Invoke/Toggle/SelectionItem (or the PostMessage/injection
                // fallback) dispatched, but none of these is driver-verifiable —
                // UIA Invoke has no read-back. `effect: unverifiable`; the caller
                // confirms via screenshot. (The would_be_silently_dropped surfaces
                // are diverted to `background_unavailable` below before reaching
                // here, so a success here always means a real dispatch.)
                Ok(Ok(msg)) => ToolResult::text(msg).with_structured(
                    json!({ "path": "ax", "verified": false, "effect": "unverifiable" }),
                ),
                Ok(Err(e)) if e.to_string().contains("__CUA_BG_UNAVAILABLE_CLICK__") => {
                    let cause = e.to_string().replace("__CUA_BG_UNAVAILABLE_CLICK__", "");
                    crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        cause,
                    )
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(px, py);
                        px = wx;
                        py = wy;
                    }
                    None => {
                        return ToolResult::error(format!(
                            "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                        ))
                    }
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            // px/py are bitmap pixels — see `bitmap_to_screen` for the
            // mapping (DWM-frame top-left + 1-px inset, NOT ClientToScreen).
            let (sx_i, sy_i) = bitmap_to_screen(hwnd, px as i32, py as i32);
            let (sx, sy) = (sx_i as f64, sy_i as f64);
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx, sy).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
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
            // delivery_mode:"foreground" short-circuit — caller explicitly chose
            // foreground delivery. Skip the UIA hit-test (UIA Invoke is
            // background-safe and would deliver the click without the
            // foreground swap the caller asked for).
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let mods_owned = modifiers.clone();
                let send_result = tokio::task::spawn_blocking(move || {
                    let mod_refs: Vec<&str> = mods_owned.iter().map(String::as_str).collect();
                    crate::input::send_click_synthesized_active_mods(
                        hwnd, sx as i32, sy as i32, count, &btn, &mod_refs,
                    )
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => {
                        let click_word = match count {
                            2 => "double-click",
                            3 => "triple-click",
                            _ => "click",
                        };
                        ToolResult::text(format!(
                            "✅ Sent {click_word} via SendInput to pid {pid} at ({sx},{sy}) (delivery_mode:foreground)."
                        ))
                        .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" }))
                    }
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }

            // delivery_mode:"background" on WinUI3 for double / right / middle (pixel
            // path, no element_index → no cached element to UIA-Invoke): these
            // can't both land and hold the contract on WinUI3 (posted input is
            // ignored, the pen injector steals foreground) → structured error.
            if delivery == DeliveryMode::Background
                && (count > 1 || btn == "right" || btn == "middle")
            {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, None, count, &btn).await
                {
                    return r;
                }
            }
            // Match the AX-addressed Chromium route above: the point remains
            // PX-resolved, but transport uses the background-safe posted
            // message path proven against the fully occluded fixture.
            if delivery == DeliveryMode::Background
                && btn == "left"
                && count == 1
                && crate::input::is_chromium_target_window(hwnd)
            {
                let posted = tokio::task::spawn_blocking(move || {
                    crate::input::post_click_screen(hwnd, sx_i, sy_i, count, &btn)
                })
                .await;
                return match posted {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Posted click to Chromium pid {pid} at ({sx},{sy}) \
                         (background, no foreground swap)."
                    ))
                    .with_structured(json!({
                        "path": "post_message",
                        "verified": false,
                        "effect": "unverifiable"
                    })),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
            // As above, bypass Chromium's false-positive UIA Invoke and use
            // the coordinate actuator before attempting any accessibility hit
            // test. The actuator itself distinguishes visible delivery from a
            // fully occluded structured refusal.
            if delivery == DeliveryMode::Background && crate::input::is_chromium_target_window(hwnd)
            {
                let btn2 = btn.clone();
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, sx as i32, sy as i32, count, &btn2)
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected click to pid {pid} at ({sx},{sy}) (background, no foreground swap)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        error.to_string(),
                    ),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
            let use_uia = (btn == "left" || btn == "middle") && count == 1;
            if use_uia {
                let invoked = tokio::task::spawn_blocking(move || {
                    crate::uia::windows_enum::try_invoke_in_window_at_point(
                        hwnd as isize,
                        sx as i32,
                        sy as i32,
                    )
                })
                .await
                .unwrap_or(false);
                if invoked {
                    return ToolResult::text(format!(
                        "✅ Performed UIA Invoke at ({sx},{sy}) for pid {pid}."
                    ))
                    .with_structured(
                        json!({ "path": "ax", "verified": false, "effect": "unverifiable" }),
                    );
                }
            }

            // UIA did not land. Known dropped surfaces other than direct
            // Chromium (handled above) get one targeted injection attempt.
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let btn2 = btn.clone();
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, sx as i32, sy as i32, count, &btn2)
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected click to pid {pid} at ({sx},{sy}) (background, no foreground swap)."
                    ))
                    .with_structured(json!({ "path": "pixel", "verified": false, "effect": "unverifiable" })),
                    Ok(Err(error)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        error.to_string(),
                    ),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }

            // delivery_mode:"background", non-dropped + non-UIA click — e.g. a
            // right-click or multi-click on a plain Win32 window. Chromium is
            // never reached here: its mouse events are flagged silently-dropped
            // above and handled by the injection path, so the only targets that
            // fall through are ones PostMessage delivers to without a foreground
            // swap. (Foreground was handled at the top; the legacy `auto`
            // Chromium-SendInput heuristic was removed in the macOS-alignment
            // pass — escalating to delivery_mode:"foreground" is now the explicit
            // path for any target PostMessage can't reach.)

            // bitmap pixels -> screen (DWM-frame origin + inset). Use
            // post_click_screen so we don't double-ClientToScreen.
            let result = tokio::task::spawn_blocking(move || {
                crate::input::post_click_screen(hwnd, sx_i, sy_i, count, &btn)
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Match Swift text format: "✅ Posted click/double-click/triple-click to pid X."
                    let click_word = match count {
                        2 => "double-click",
                        3 => "triple-click",
                        _ => "click",
                    };
                    ToolResult::text(format!("✅ Posted {click_word} to pid {pid}."))
                        .with_structured(
                            json!({ "path": "pixel", "verified": false, "effect": "unverifiable" }),
                        )
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

/// px-focus for the keyboard family (type_text / press_key / hotkey): pixel-click
/// at (x,y) to establish real renderer focus before a keystroke — the *element px
/// action* form of a keyboard tool. Reuses ClickTool's exact coordinate
/// translation + delivery_mode (UIA hit-test → PostMessage fallback), so it lands
/// on the same pixel a px-click would. `Ok(())` on success; `Err(ToolResult)`
/// short-circuits the caller. Mirrors macOS `tools::focus_by_pixel`.
#[allow(clippy::too_many_arguments)]
async fn focus_by_pixel(
    state: &Arc<ToolState>,
    pid: u32,
    window_id: Option<u64>,
    x: f64,
    y: f64,
    foreground: bool,
    session: Option<String>,
    session_id: Option<String>,
    from_zoom: bool,
) -> Result<(), ToolResult> {
    let mut click_args = json!({
        "pid": pid, "x": x, "y": y,
        "delivery_mode": if foreground { "foreground" } else { "background" },
    });
    if let Some(wid) = window_id {
        click_args["window_id"] = json!(wid);
    }
    if let Some(s) = session {
        click_args["session"] = json!(s);
    }
    if let Some(s) = session_id {
        click_args["_session_id"] = json!(s);
    }
    if from_zoom {
        click_args["from_zoom"] = json!(true);
    }
    let focus = ClickTool {
        state: state.clone(),
    }
    .invoke(click_args)
    .await;
    if focus.is_error == Some(true) {
        // Preserve the click tool's structured background refusal (for example
        // background_occluded / background_uipi_blocked). Re-wrapping it as a
        // text-only error made keyboard-family PX calls lose the actionable
        // capability result produced by the actual actuator.
        return Err(focus);
    }
    // Brief settle so the renderer registers focus before the keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    Ok(())
}

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
                "type":"object","required":["text"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to type into the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "text":{"type":"string","description":"Text to insert at the focused element's cursor."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Required when element_index is used. Optional when element_token is supplied (the token carries it)."},
                    "element_index":{"type":"integer","description":"Element index from the last get_window_state for the same (pid, window_id). When supplied, type_text (1) writes via UIA ValuePattern.SetValue on that element (works for WPF/WinForms/UWP/XAML without focus steal) and (2) verifies by reading that element's value back by handle — focus-independent, so the structured `verify` reaches `confirmed`/`unchanged` even when the target isn't foreground. Strongly preferred over typing into 'whatever is focused' (no element_index), which falls back to PostMessage WM_CHAR + a foreground-only focused-element read-back. Requires window_id."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "x":{"type":"number","description":"Window-local screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_index) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the UIA/WM_CHAR path can't reach. Read straight off the get_window_state PNG, same convention as click."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y of the field (see x)."},
                    "delay_ms":{"type":"integer","minimum":0,"maximum":200,"description":"Milliseconds between characters. Default 30."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<TypeTextInput>("type_text", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let text =
                cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&input.text)
                    .into_owned();
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let text_len = text.chars().count();
            return match tokio::task::spawn_blocking(move || {
                crate::input::send_text_synthesized(hwnd, &text)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) into the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "characters": text_len,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop type task failed: {error}")),
            };
        }
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags before delivery —
        // catches the case where an LLM hallucinated its own tool-
        // invocation tags into the text param (see text_sanitize docs).
        // Returns Cow::Borrowed on the no-match fast path so the common
        // case is allocation-free.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "type_text",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index as u64)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        let delivery = DeliveryMode::from_args(&args);

        // ── px form: focus by pixel-click, then type into the focused element ──
        // Pass x,y (no element_index) for an *element px action*: pixel-click the
        // field to give the Chromium/Electron renderer the real keyboard focus the
        // UIA/WM_CHAR path can't establish on its own, then fall through to the
        // focused-window WM_CHAR path below (which lands once focused). Reuses
        // ClickTool's exact coordinate translation + delivery_mode. Mirrors macOS.
        {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if elem_idx.is_some() {
                    return ToolResult::error(
                        "Pass either element_index (ax) or x,y (px) to type_text, not both.",
                    );
                }
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    hwnd_opt,
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                )
                .await
                {
                    return e;
                }
                // elem_idx stays None → the type path below writes to the now-
                // focused element via the WM_CHAR (key_events) rung.
            }
        }

        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.",
            );
        }
        // Same no-raise guard as click: a XAML/WPF ValuePattern.SetValue handler
        // calls UIElement.Focus()→SetForegroundWindow; WS_EX_NOACTIVATE on the
        // target makes that a no-op while the value still gets set. Safe because
        // type_text never uses the SendInput foreground-swap path. No-op for
        // delivery_mode:"foreground" and when no window_id was given.
        let _noact = match (delivery != DeliveryMode::Foreground, hwnd_opt) {
            (true, Some(h)) => Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(h as *mut _),
            )),
            _ => None,
        };
        let _delay_ms = args.u64_or("delay_ms", 30);
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        let text_len = text.chars().count();

        // Refuse known background drops before the final WM_CHAR path. WPF is
        // conditional: indexed text still has a working UIA ValuePattern route,
        // while unindexed text would be posted to the top-level and disappear.
        if delivery == DeliveryMode::Background
            && (crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::TextInput)
                || (elem_idx.is_none() && crate::input::delivery::is_wpf_target_window(hwnd)))
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::TextInput,
            );
        }
        // delivery_mode:"foreground" — explicit SendInput Unicode with a brief
        // foreground swap (send_text_synthesized), symmetric with press_key's
        // foreground path. This is the rung that lands on VCL/LibreOffice
        // document grids and other targets where PostMessage WM_CHAR is
        // silently dropped. Honest by construction: if the foreground swap is
        // rejected (daemon not at UIAccess integrity), it returns an error
        // rather than a false success.
        if delivery == DeliveryMode::Foreground {
            // An indexed foreground type targets that element, not whichever
            // child happened to retain focus in the top-level window. UIA
            // SetFocus is not sufficient for Chromium renderer controls, so
            // establish real system focus with the same foreground coordinate
            // actuator used by an indexed click before sending Unicode input.
            if let Some(idx) = elem_idx {
                let (cx, cy) =
                    match self
                        .state
                        .element_cache
                        .get_element_center(pid, hwnd, idx as usize)
                    {
                        Some(center) => center,
                        None => {
                            return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                        }
                    };
                let (cx, cy) = match resolve_onscreen_point_with_scroll(
                    &self.state.element_cache,
                    pid,
                    hwnd,
                    idx as usize,
                    cx,
                    cy,
                    "foreground typing",
                ) {
                    Ok(point) => point,
                    Err(message) => return ToolResult::error(message),
                };
                let focus_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, cx, cy, 1, "left")
                })
                .await;
                match focus_result {
                    Ok(Ok(())) => {
                        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                    }
                    Ok(Err(error)) => return ToolResult::error(error.to_string()),
                    Err(error) => {
                        return ToolResult::error(format!(
                            "foreground element-focus task failed: {error}"
                        ))
                    }
                }
            }
            let text_fg = text.clone();
            let r = tokio::task::spawn_blocking(move || {
                crate::input::send_text_synthesized(hwnd, &text_fg)
            })
            .await;
            return match r {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Typed {text_len} char(s) on pid {raw_pid} via SendInput (delivery_mode:foreground)."
                ))
                .with_structured(serde_json::json!({
                    "path": "key_events",
                    "characters": text_len,
                    // Foreground SendInput delivered, but with no read-back the
                    // driver can't confirm the effect → unverifiable. No escalation:
                    // foreground IS the escalated rung (mirrors macOS PATH_KEY_EVENTS_FG).
                    "effect": "unverifiable",
                })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Pin the agent-cursor overlay above the target window so the synthetic
        // cursor stays sandwiched at z+1 of the type target for the full
        // duration of the keystrokes (both XAML/UIA and PostMessage paths).
        pin_overlay_above(&cursor_key, hwnd);

        // Glide the agent cursor onto the field being typed into, so the viewer
        // can see *where* the agent is typing — same visual feedback as a click.
        // Only when an element_index is supplied (we have its cached center);
        // the focused-element path has no resolvable position to point at.
        if let Some(idx) = elem_idx {
            if let Some((cx, cy)) =
                self.state
                    .element_cache
                    .get_element_center(pid, hwnd, idx as usize)
            {
                overlay_glide_to(&cursor_key, cx as f64, cy as f64).await;
                crate::overlay::send_command(
                    cursor_key.clone(),
                    cursor_overlay::OverlayCommand::ClickPulse {
                        x: cx as f64,
                        y: cy as f64,
                    },
                );
            }
        }

        // 0. Terminal short-circuit: Windows Terminal, mintty, ConsoleWindowClass,
        //    GVim / NeoVim — all consume keys through console / VT pipelines that
        //    PostMessage(WM_CHAR) doesn't reach, and the `set_value` UIA path
        //    silently no-ops on most of these. The only background way in was a
        //    cloaked focus grab (SendInput Unicode) — which the macOS-aligned
        //    contract forbids in background. Surface background_unavailable; the
        //    agent escalates to delivery_mode:"foreground".
        if crate::terminal::is_terminal_hwnd(hwnd) {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::TextInput,
            );
        }

        // CUA-543 routing: PostMessage WM_CHAR doesn't reach modern
        // XAML / WinUI3 hosts. When the target is one of those AND the
        // caller has supplied an element_index, route through UIA
        // `ValuePattern.SetValue` — same code path the `set_value` tool
        // uses, which we've verified works on modern Notepad / WinUI3.
        // Legacy Win32 stays on the PostMessage path so the no-focus-
        // steal property is preserved.
        // ── Automatic routing — the caller need not know the framework. ──
        // 1. With an element_index, try UIA ValuePattern.SetValue first: it works
        //    for WPF / WinForms / UWP / XAML and many web inputs, sets the value
        //    through the accessibility channel (no keystrokes), and the `_noact`
        //    guard blocks any self-foreground — so it never raises. Auto-falls-
        //    back to the WM_CHAR path below if the element has no ValuePattern
        //    (most legacy Win32 EDITs consume WM_CHAR fine without focus steal).
        if let Some(idx) = elem_idx {
            let idx = idx as usize;
            let state = self.state.clone();
            let text_for_uia = text.clone();
            let set_ok = tokio::task::spawn_blocking(move || -> bool {
                // Retain the element under the cache lock so a concurrent
                // get_window_state snapshot-replace on the same (pid, hwnd)
                // can't Release it to zero while this SetValue is in flight.
                // The guard is held for the whole closure.
                let Some(element_guard) = state.element_cache.get_element_retained(pid, hwnd, idx)
                else {
                    return false;
                };
                let ptr = element_guard.as_ptr();
                use windows::core::{Interface, BSTR};
                use windows::Win32::UI::Accessibility::{
                    IUIAutomationElement, IUIAutomationValuePattern, UIA_ValuePatternId,
                };
                let elem: IUIAutomationElement =
                    unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
                let ok = (|| -> anyhow::Result<()> {
                    let pattern = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId) }?;
                    let vp: IUIAutomationValuePattern = pattern.cast()?;
                    // Shield the SetValue against host self-foreground. A
                    // Chromium/Electron (or XAML) ValuePattern.SetValue handler
                    // calls SetForegroundWindow(self), which WS_EX_NOACTIVATE
                    // does NOT stop; the EnableWindow shield does (disabled
                    // top-level can't be foregrounded) while the a11y-channel
                    // SetValue still lands. Same fix as the UIA Invoke path.
                    crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                        // ValuePattern has no caret/selection insertion API. Preserve
                        // the current document and append as the deterministic
                        // background fallback; replacing the whole value violates
                        // type_text's insertion contract for editors.
                        let current = vp
                            .CurrentValue()
                            .map(|value| value.to_string())
                            .unwrap_or_default();
                        let inserted = format!("{current}{text_for_uia}");
                        vp.SetValue(&BSTR::from(inserted.as_str()))
                    })?;
                    Ok(())
                })()
                .is_ok();
                std::mem::forget(elem);
                ok
            })
            .await
            .unwrap_or(false);
            if set_ok {
                // Read the value back by element handle — focus-independent
                // (works regardless of which window is foreground), proving the
                // a11y write actually took rather than trusting SetValue's
                // return alone.
                let state_rb = self.state.clone();
                let text_rb = text.clone();
                let verify = tokio::task::spawn_blocking(move || {
                    match read_cached_element_value(&state_rb, pid, hwnd, idx) {
                        Some(v) if v.contains(text_rb.as_str()) => "confirmed",
                        Some(_) => "unchanged",
                        None => "unreadable",
                    }
                })
                .await
                .unwrap_or("unreadable");
                // SURFACE-AWARE VERIFICATION (mirrors macOS type_text). On
                // Electron/Chromium web inputs the UIA layer accepts a
                // `ValuePattern.SetValue` and echoes it straight back through
                // `ValuePattern.CurrentValue` while the renderer/DOM never observes
                // it — so a read-back "confirm" is a shim echo, not ground truth
                // (the Windows analogue of the Slack-search AX false-confirm).
                // Refuse to claim verified there: the screenshot is the only truth
                // and the next rung is the element px action. Probe ONLY when it
                // would otherwise confirm via the UIA value path, so native UIA
                // types (WPF / WinForms / UWP / XAML) pay nothing. The signal is
                // the window class — Chromium and every Electron host expose
                // `Chrome_WidgetWin_*` (see `is_chromium_target_window`).
                let ax_echo_surface =
                    verify == "confirmed" && crate::input::is_chromium_target_window(hwnd);
                let verified = verify == "confirmed" && !ax_echo_surface;
                let (mark, note) = if verified {
                    ("✅ Wrote", String::new())
                } else if ax_echo_surface {
                    (
                        "📨 Sent (unverified)",
                        " — Electron/web surface: the UIA layer accepts and echoes the \
                      write but the renderer may not have observed it, so the driver \
                      cannot confirm via UIA. Verify via the screenshot; if it didn't \
                      land, re-type with the px form (pass x,y to pixel-focus the \
                      field)."
                            .to_string(),
                    )
                } else {
                    (
                        "📨 Sent (unverified)",
                        " — driver could not confirm; verify via screenshot.".to_string(),
                    )
                };
                return ToolResult::text(format!(
                    "{mark} {text_len} char(s) on pid {raw_pid} via UIA ValuePattern \
                     (element_index=[{idx}]; verify: {verify}).{note}"
                ))
                .with_structured({
                    // `effect` mirrors the UIA read-back: a TRUSTED positive
                    // read-back is "confirmed"; an unchanged/unreadable value, or an
                    // Electron UIA echo we refuse to trust, is "unverifiable".
                    let mut s = serde_json::json!({
                        "path": "ax",
                        "characters": text_len,
                        "verified": verified,
                        "verify": verify,
                        "effect": if verified { "confirmed" } else { "unverifiable" },
                    });
                    if ax_echo_surface {
                        // Electron UIA echo → the element px action is the next rung,
                        // not foreground (it's a renderer-focus problem, not an
                        // activation one).
                        s["escalation"] = serde_json::json!({
                            "recommended": "px",
                            "reason": "Electron/web surface — the UIA write was echoed \
                                       but the renderer may not have observed it. \
                                       Confirm via the screenshot; if it didn't land, \
                                       re-type with the element px action (pass x,y to \
                                       pixel-focus the field, then type)."
                        });
                    } else if !verified {
                        // The field IS in the UIA tree (it's a focus/accept problem,
                        // not a missing element), so escalation points at foreground.
                        s["escalation"] = serde_json::json!({
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — re-call \
                                       with delivery_mode:\"foreground\" if a screenshot \
                                       shows the text didn't land."
                        });
                    }
                    s
                });
            }
            // ValuePattern unavailable → fall through to the WM_CHAR path.
        }

        // 2. No element_index on a WPF target: WM_CHAR is dropped and there's no
        //    element to SetValue. The only background path was a cloaked focus
        //    grab (SendInput), which the macOS-aligned contract forbids in
        //    background. Prefer supplying an element_index (path 1, UIA SetValue —
        //    never raises); otherwise surface background_unavailable so the agent
        //    escalates to delivery_mode:"foreground".
        if elem_idx.is_none() && crate::input::delivery::is_wpf_target_window(hwnd) {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::TextInput,
            );
        }

        // 3. Legacy Win32 / GDI / Chromium-IME — PostMessage WM_CHAR, no focus steal.
        //
        // Read-back verification (mirrors macOS type_text): WM_CHAR is silently
        // dropped by some focused targets (notably a VCL/LibreOffice Calc grid
        // not in cell-edit mode), and PostMessage reports success regardless. So
        // snapshot the focused element's UIA value before and after; if it did
        // not change we can't confirm the text landed and report "Sent
        // (unverified)" with the foreground-escalation hint, instead of a blind
        // success. If the focused element exposes no ValuePattern we likewise
        // can't confirm (conservative: under-claim rather than false-succeed).
        let text_for_post = text.clone();
        let verify_pid = pid;
        let verify_idx = elem_idx.map(|i| i as usize);
        let state_rb = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Prefer a focus-independent read of the *specific* cached element
            // when we have its index; only fall back to the (flaky, focus-
            // dependent) system focused element when typing into "whatever is
            // focused" with no element_index.
            let read = |idx: Option<usize>| match idx {
                Some(i) => read_cached_element_value(&state_rb, verify_pid, hwnd, i),
                None => read_focused_value_uia(verify_pid),
            };
            let before = read(verify_idx);
            let post_res = crate::input::post_type_text(hwnd, &text_for_post);
            std::thread::sleep(std::time::Duration::from_millis(40));
            let after = read(verify_idx);
            (post_res, before, after)
        })
        .await;
        match result {
            Ok((Ok(()), before, after)) => {
                // Three-way verdict. Crucially, "couldn't read the field" is NOT
                // the same as "the text didn't land": on Windows, UIA's
                // GetFocusedElement is system-wide (unlike macOS's per-app
                // AXFocusedUIElement), so when the target isn't the foreground
                // app we cannot read it back even though a PostMessage WM_CHAR
                // may have landed fine. Treating unreadable as failure would
                // spuriously push agents to foreground and break the
                // background-first contract. So only DOWNGRADE on positive
                // evidence (read OK both times, value unchanged).
                match (&before, &after) {
                    (Some(b), Some(a)) if a != b => ToolResult::text(format!(
                        "✅ Typed {text_len} char(s) on pid {raw_pid} via PostMessage \
                         ({_delay_ms}ms delay; verified via UIA read-back)."
                    ))
                    .with_structured(serde_json::json!({
                        "path": "key_events", "characters": text_len,
                        "verified": true, "verify": "confirmed",
                        "effect": "confirmed",
                    })),
                    (Some(_), Some(_)) => ToolResult::text(format!(
                        "📨 Sent {text_len} char(s) to pid {raw_pid} via PostMessage, but \
                         the focused field's value did not change — the text was likely \
                         dropped (e.g. a VCL/LibreOffice grid not in cell-edit mode). \
                         Retry with delivery_mode:\"foreground\"."
                    ))
                    .with_structured(serde_json::json!({
                        "path": "key_events", "characters": text_len,
                        "verified": false, "verify": "unchanged",
                        "effect": "unverifiable",
                        "escalation": {
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — re-call \
                                       with delivery_mode:\"foreground\" if a screenshot \
                                       shows the text didn't land."
                        },
                    })),
                    _ => ToolResult::text(format!(
                        "📨 Sent {text_len} char(s) to pid {raw_pid} via PostMessage \
                         ({_delay_ms}ms delay; not verified — could not read \
                         the focused field back, e.g. the target isn't foreground or \
                         exposes no ValuePattern). If it didn't land, retry with \
                         delivery_mode:\"foreground\"."
                    ))
                    .with_structured(serde_json::json!({
                        "path": "key_events", "characters": text_len,
                        "verified": false, "verify": "unreadable",
                        "effect": "unverifiable",
                        "escalation": {
                            "recommended": "foreground",
                            "reason": "background insert could not be confirmed — re-call \
                                       with delivery_mode:\"foreground\" if a screenshot \
                                       shows the text didn't land."
                        },
                    })),
                }
            }
            Ok((Err(e), _, _)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// Read a **specific cached element's** text value by its `element_index`,
/// for type_text read-back verification. Tries `ValuePattern.CurrentValue`
/// then falls back to `TextPattern` document text. Unlike
/// [`read_focused_value_uia`] this is **focus-independent**: it reads the exact
/// element the caller typed into (by handle, from the per-(pid,window) cache),
/// so it works whether or not the target is the foreground window. Returns
/// `None` if the index isn't cached or the element exposes no readable text
/// pattern. Mirrors the cache-retain + `mem::forget` discipline of the
/// ValuePattern.SetValue path so the cached COM ref isn't released.
fn read_cached_element_value(state: &ToolState, pid: u32, hwnd: u64, idx: usize) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationValuePattern,
        UIA_TextPatternId, UIA_ValuePatternId,
    };
    let guard = state.element_cache.get_element_retained(pid, hwnd, idx)?;
    let ptr = guard.as_ptr();
    let elem: IUIAutomationElement = unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
    let val = unsafe {
        elem.GetCurrentPattern(UIA_ValuePatternId)
            .ok()
            .and_then(|p| p.cast::<IUIAutomationValuePattern>().ok())
            .and_then(|vp| vp.CurrentValue().ok())
            .map(|b| b.to_string())
            .or_else(|| {
                elem.GetCurrentPattern(UIA_TextPatternId)
                    .ok()
                    .and_then(|p| p.cast::<IUIAutomationTextPattern>().ok())
                    .and_then(|tp| tp.DocumentRange().ok())
                    .and_then(|r| r.GetText(-1).ok())
                    .map(|b| b.to_string())
            })
    };
    // The cache guard owns the ref; don't let `elem`'s Drop release it.
    std::mem::forget(elem);
    val
}

/// Read the currently-focused UIA element's `ValuePattern` value **iff it
/// belongs to `expect_pid`**, for type_text read-back verification.
/// Self-contained COM (STA) init/teardown so it can be called from a
/// `spawn_blocking` worker. Returns `None` if UIA is unreachable, nothing is
/// focused, the focused element is owned by a different process (e.g. the
/// target is backgrounded and system focus is on the user's foreground app —
/// we must NOT read that element's value), or the focused element exposes no
/// `ValuePattern` (most rich-text/document surfaces). Callers treat `None` as
/// "could not confirm" rather than failure. Mirrors the focused-element read in
/// the `debug_window_info` tool.
fn read_focused_value_uia(expect_pid: u32) -> Option<String> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationValuePattern, UIA_ValuePatternId,
    };

    let coinit_ok = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok() };
    let value = (|| -> Option<String> {
        let uia: IUIAutomation =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }.ok()?;
        let focused = unsafe { uia.GetFocusedElement() }.ok()?;
        // Scope to the target: system focus may be on a different app when the
        // target is backgrounded; reading that element would give a bogus verdict.
        if unsafe { focused.CurrentProcessId() }.ok()? as u32 != expect_pid {
            return None;
        }
        let pattern = unsafe { focused.GetCurrentPattern(UIA_ValuePatternId) }.ok()?;
        let vp: IUIAutomationValuePattern = pattern.cast().ok()?;
        Some(unsafe { vp.CurrentValue() }.ok()?.to_string())
    })();
    if coinit_ok {
        unsafe {
            CoUninitialize();
        }
    }
    value
}

// ── press_key ─────────────────────────────────────────────────────────────────

pub struct PressKeyTool {
    state: Arc<ToolState>,
}
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
                `element_index` focuses the cached UIA element before sending the key; the \
                top-level window remains backgrounded when delivery_mode is background.".into(),
            input_schema: json!({
                "type":"object","required":["key"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to send the key to the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "key":{"type":"string","description":"Key name (return, tab, escape, up, down, left, right, space, delete, home, end, pageup, pagedown, f1-f12, letter, digit)."},
                    "modifiers":{"type":"array","items":{"type":"string"},"description":"Optional modifier names held while the key is pressed (ctrl/shift/alt/win)."},
                    "element_index":{"type":"integer","description":"Optional element_index from the last get_window_state; focuses that UIA element before the key is delivered."},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "x":{"type":"number","description":"Window-local screenshot-pixel X — the element px action form: pixel-click there to focus, then send the key. Use when the key must go to a Chromium/Electron surface the UIA path can't focus. Pass with y, no element_index."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y (see x)."},
                    "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used; otherwise auto-resolves the pid's first visible window."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<PressKeyInput>("press_key", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let key = input.key;
            let mods = input.modifiers.unwrap_or_default();
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let key_display = key.clone();
            return match tokio::task::spawn_blocking(move || {
                let modifiers: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::send_key_synthesized(hwnd, &key, &modifiers)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Sent {key_display} to the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "key": key_display,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop key task failed: {error}")),
            };
        }
        let key = match args.require_str("key") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mods: Vec<String> = args.str_array("modifiers");
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "press_key",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index as u64)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        let delivery = DeliveryMode::from_args(&args);
        // Swift requires window_id when element_index is supplied — ports the
        // same validation.
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.",
            );
        }

        // px form: pixel-click to focus, then the key goes to the focused element.
        // Reuses click's translation + delivery_mode; after it, deliver via the
        // plain background post path (the focus-click already handled fronting if
        // foreground), so skip the background_unavailable check + foreground swap
        // below. Mirrors macOS press_key.
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                if elem_idx.is_some() {
                    return ToolResult::error(
                        "Pass either element_index (ax) or x,y (px) to press_key, not both.",
                    );
                }
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    hwnd_opt,
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                )
                .await
                {
                    return e;
                }
                true
            } else {
                false
            }
        };

        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Classify known background drops before touching UIA focus. Focusing
        // first made an honest Chromium refusal transiently activate the target.
        let event_kind = if mods.is_empty() {
            EventKind::Keystroke
        } else {
            EventKind::KeyCombo
        };
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, event_kind)
        {
            return crate::input::delivery::background_unavailable_error(hwnd, event_kind);
        }

        // W1: an element-addressed key needs the control's actual focus
        // target, not merely its owning top-level HWND. Embedded WebView hosts
        // can activate their frame from UIA SetFocus even under
        // WS_EX_NOACTIVATE. Their proven-safe pixel route establishes renderer
        // focus with a posted click, so reuse that route at the AX element's
        // cached center.
        let background_webview_focus = elem_idx.is_some()
            && delivery == DeliveryMode::Background
            && crate::input::has_chromium_descendant(hwnd);
        let mut noact = if elem_idx.is_some() && delivery == DeliveryMode::Background {
            Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(hwnd as *mut _),
            ))
        } else {
            None
        };
        if let Some(idx) = elem_idx.filter(|_| background_webview_focus) {
            let Some((cx, cy)) =
                self.state
                    .element_cache
                    .get_element_center(pid, hwnd, idx as usize)
            else {
                return ToolResult::error(format!(
                    "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                ));
            };
            let (cx, cy) = match resolve_onscreen_point_with_scroll(
                &self.state.element_cache,
                pid,
                hwnd,
                idx as usize,
                cx,
                cy,
                "focusing for key delivery",
            ) {
                Ok(point) => point,
                Err(message) => return ToolResult::error(message),
            };
            let (mut px, mut py) = screen_to_bitmap(hwnd, cx, cy);
            if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px = (px as f64 / ratio).round() as i32;
                py = (py as f64 / ratio).round() as i32;
            }
            // Release the outer guard before the shared pixel helper. ClickTool
            // owns a guard around the click itself, then releases it before its
            // renderer settle period. This is the exact route already proven by
            // the PX background cell, including targeted injection fallback.
            drop(noact.take());
            if let Err(error) = focus_by_pixel(
                &self.state,
                pid,
                Some(hwnd),
                px as f64,
                py as f64,
                false,
                args.opt_str("session"),
                args.opt_str("_session_id"),
                false,
            )
            .await
            {
                return error;
            }
        } else if let Some(idx) = elem_idx {
            let state = self.state.clone();
            let focused = tokio::task::spawn_blocking(move || {
                crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || {
                    state.element_cache.focus_element(pid, hwnd, idx as usize)
                })
            })
            .await;
            match focused {
                Ok(Ok(())) => {
                    if delivery == DeliveryMode::Background {
                        let _ = crate::input::wait_for_focused_descendant(
                            hwnd,
                            std::time::Duration::from_millis(500),
                        );
                    }
                }
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("UIA focus task failed: {e}")),
            }
        }
        let key_display = key.clone();
        // Foreground: send_key_synthesized takes the SetForegroundWindow path.
        // Skipped when px-focus already fronted/clicked the target — the key then
        // goes via the plain background post path below.
        if !px_focus && delivery == DeliveryMode::Foreground {
            let send_result = tokio::task::spawn_blocking(move || {
                let m: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::send_key_synthesized(hwnd, &key, &m)
            })
            .await;
            return match send_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent {key_display} via SendInput on pid {raw_pid} (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            crate::input::post_key(hwnd, &key, &m)
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "📨 Sent {key_display} to pid {raw_pid} via PostMessage (not verified). \
                 If it didn't land, retry with delivery_mode:\"foreground\"."
            ))
            .with_structured(serde_json::json!({
                "path": "key_events",
                "key": key_display,
                "verified": false,
                "verify": "unreadable",
                "effect": "unverifiable",
                "escalation": {
                    "recommended": "foreground",
                    "reason": "background key delivery could not be confirmed — re-call \
                               with delivery_mode:\"foreground\" if a screenshot shows the key didn't land."
                },
            })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── hotkey ────────────────────────────────────────────────────────────────────

fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "ctrl" | "control" | "shift" | "alt" | "win" | "windows" | "cmd" | "command"
    )
}

pub struct HotkeyTool {
    state: Arc<ToolState>,
}
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
                "type":"object","required":["keys"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id to send the hotkey to the current foreground application."},
                    "pid":{"type":"integer","description":"Target process ID."},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) and one non-modifier key, e.g. [\"ctrl\", \"c\"]."},
                    "x":{"type":"number","description":"Window-local screenshot-pixel X — the element px action form: pixel-click there to focus, then send the combo (so e.g. Ctrl+V pastes into that field). Pass with y. Use for Chromium/Electron surfaces the background combo can't reach."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y (see x)."},
                    "window_id":{"type":"integer","description":"Explicit HWND when the pid owns multiple windows."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<HotkeyInput>("hotkey", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            if input.keys.len() < 2 {
                return ToolResult::error("hotkey.keys must contain at least two keys.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let mods: Vec<String> = input
                .keys
                .iter()
                .filter(|key| is_modifier(key))
                .cloned()
                .collect();
            let Some(key) = input
                .keys
                .iter()
                .rev()
                .find(|key| !is_modifier(key))
                .cloned()
            else {
                return ToolResult::error("keys must include at least one non-modifier key.");
            };
            let full_keys = input.keys;
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            let key_display = full_keys.join("+");
            return match tokio::task::spawn_blocking(move || {
                let modifiers: Vec<&str> = mods.iter().map(String::as_str).collect();
                crate::input::send_key_synthesized(hwnd, &key, &modifiers)
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Sent {key_display} to the foreground application (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput", "keys": full_keys,
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop hotkey task failed: {error}")),
            };
        }
        // Parse keys array (Swift's only path).  Legacy key+modifiers shape
        // still accepted as a Rust-side convenience.
        let (key, mods, full_keys) = if let Some(arr) = args.get("keys") {
            let raw = match arr.as_array() {
                Some(r) => r,
                None => return ToolResult::error("Missing required array field keys."),
            };
            let keys: Vec<String> = raw
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
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
            let m: Vec<String> = args
                .get("modifiers")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let mut full = m.clone();
            full.push(k.to_owned());
            (k.to_owned(), m, full)
        } else {
            return ToolResult::error("Missing required array field keys.");
        };
        let hwnd_opt = args.opt_u64("window_id");
        let raw_pid = match args.require_i64("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = raw_pid as u32;
        let delivery = DeliveryMode::from_args(&args);

        // Resolve HWND: use window_id if given, else pick first window for pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let pid2 = pid;
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid2)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Match Swift's text format exactly: `"✅ Pressed K1+K2+... on pid X."`
        // where K1+K2+... is `keys.joined(separator: "+")` of the FULL keys
        // array as the caller supplied it (modifiers + non-modifier in order).
        let key_display = full_keys.join("+");

        // px form: pixel-click to focus, then the combo acts on the focused field
        // (e.g. Ctrl+V into a Chromium input). A background pixel focus does not
        // make Chromium's modifier PostMessage path reliable, so it still goes
        // through the capability guard below; foreground remains explicit.
        let px_focus = {
            let px = args.get("x").and_then(|v| v.as_f64());
            let py = args.get("y").and_then(|v| v.as_f64());
            if let (Some(cx), Some(cy)) = (px, py) {
                let from_zoom = args
                    .get("from_zoom")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    hwnd_opt,
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    args.opt_str("_session_id"),
                    from_zoom,
                )
                .await
                {
                    return e;
                }
                true
            } else {
                false
            }
        };

        if !px_focus && crate::input::is_xaml_host_hwnd(hwnd) {
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
            let result =
                match tokio::time::timeout(std::time::Duration::from_secs(4), blocking).await {
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
        // delivery_mode:"background" — refuse if PostMessage of the key combo
        // would be silently dropped on this target (Chromium with modifiers).
        let event_kind = if has_modifiers {
            EventKind::KeyCombo
        } else {
            EventKind::Keystroke
        };
        if delivery == DeliveryMode::Background
            && (px_focus || crate::input::delivery::would_be_silently_dropped(hwnd, event_kind))
        {
            // macOS-aligned: a `background` hotkey never fronts. This combo would
            // be silently dropped (VCL/classic-Win32 TranslateAccelerator, or
            // Chromium key-combos) and only a foreground/focus grab lands it —
            // which background must not do. Surface background_unavailable; the
            // agent escalates to delivery_mode:"foreground".
            return crate::input::delivery::background_unavailable_error(hwnd, event_kind);
        }
        // delivery_mode:"foreground" — explicit SendInput swap (the path that
        // unblocks TranslateAccelerator-style apps). A Background call that
        // reaches here means the key combo is NOT silently dropped on this
        // target (the drop-check above returned early otherwise), so it stays
        // on PostMessage and the no-foreground contract holds.
        // Foreground is an explicit request for system-queue delivery. This is
        // still required after a PX focus click: PostMessage does not update
        // global modifier state, so Chromium never observes Ctrl+Shift+7 as a
        // chord even though the renderer control is focused.
        let use_send_input = delivery == DeliveryMode::Foreground;
        let result = tokio::task::spawn_blocking(move || {
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            if use_send_input {
                crate::input::send_key_synthesized(hwnd, &key, &m)
            } else {
                crate::input::post_key(hwnd, &key, &m)
            }
        })
        .await;
        let path = if use_send_input {
            "SendInput"
        } else {
            "PostMessage"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Pressed {key_display} on pid {raw_pid} via {path} \
                 (Win32 target)."
            )),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
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
                "type":"object","required":["pid","value"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID."},
                    "window_id":{"type":"integer","description":"HWND of the window. Required when element_index is used; optional when element_token is supplied (the token carries it)."},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "value":{"type":"string","description":"New value. UIA will coerce to the element's native type."}
                },"additionalProperties":false
            }),
            // Swift: idempotent: true (setting same value twice is idempotent).
            read_only: false, destructive: true, idempotent: true, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_key = resolve_cursor_key(&args);
        let raw_pid = args.get("pid").and_then(|v| v.as_i64());
        if raw_pid.is_none() {
            return ToolResult::error(
                "Missing required integer fields pid, window_id, and element_index.",
            );
        }
        let pid = raw_pid.unwrap() as u32;
        let value = match args.get("value").and_then(|v| v.as_str()) {
            Some(v) => v.to_owned(),
            None => return ToolResult::error("Missing required string field value."),
        };
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "set_value",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (hwnd, idx) = match resolved {
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: Some(wid),
                element_index,
                ..
            } => (wid as u64, element_index),
            cua_driver_core::element_token::ResolvedElement::Element {
                window_id: None, ..
            } => {
                return ToolResult::error(
                    "set_value requires window_id when element_index is used \
                 (omit only when supplying element_token, which carries it).",
                )
            }
            cua_driver_core::element_token::ResolvedElement::None => {
                return ToolResult::error(
                    "Missing required integer fields pid, window_id, and element_index.",
                )
            }
        };

        // No-raise guard: a WPF/XAML automation peer calls UIElement.Focus() →
        // SetForegroundWindow during ValuePattern.SetValue. WS_EX_NOACTIVATE on
        // the target makes that a no-op while the value is still set, so a
        // background SetValue can't steal the user's foreground. Held across the
        // whole write. (No-op for delivery_mode:"foreground".)
        let _noact = if crate::input::delivery::DeliveryMode::from_args(&args)
            != crate::input::delivery::DeliveryMode::Foreground
        {
            Some(crate::input::NoActivateGuard::arm(
                windows::Win32::Foundation::HWND(hwnd as *mut _),
            ))
        } else {
            None
        };

        // Glide the agent cursor onto the target element before writing its
        // value, so a value write gets the same visual feedback as a click —
        // the viewer can see *where* the agent is acting. No-op when the
        // overlay is disabled or the element has no cached center.
        if let Some((cx, cy)) = self.state.element_cache.get_element_center(pid, hwnd, idx) {
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, cx as f64, cy as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
        }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            // Retain the element under the cache lock so a concurrent
            // get_window_state snapshot-replace on the same (pid, hwnd) can't
            // Release it to zero while this Value/RangeValue SetValue is in
            // flight. The guard is held for the whole closure.
            let element_guard = state
                .element_cache
                .get_element_retained(pid, hwnd, idx)
                .ok_or_else(|| anyhow::anyhow!("Element {idx} not in cache."))?;
            let ptr = element_guard.as_ptr();
            use windows::core::{Interface, BSTR};
            use windows::Win32::UI::Accessibility::{
                IUIAutomationElement, IUIAutomationValuePattern, UIA_ValuePatternId,
            };
            let elem: IUIAutomationElement =
                unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
            // Try ValuePattern first (text inputs, editable combos, etc).
            // The SetValue is shielded by the EnableWindow bypass: a
            // Chromium/Electron (or XAML) SetValue handler self-foregrounds via
            // SetForegroundWindow, which WS_EX_NOACTIVATE alone does not stop;
            // disabling the host for the duration does, while the a11y-channel
            // write still lands. (No-op shield for non-XAML/non-Chromium hosts.)
            if let Ok(pattern) = unsafe { elem.GetCurrentPattern(UIA_ValuePatternId) } {
                if let Ok(vp) = pattern.cast::<IUIAutomationValuePattern>() {
                    let set =
                        crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                            vp.SetValue(&BSTR::from(value.as_str()))
                        });
                    if set.is_ok() {
                        std::mem::forget(elem);
                        return Ok("ValuePattern".to_string());
                    }
                }
            }
            // Fall through to RangeValuePattern for Sliders / ProgressBars /
            // numeric ranges. RangeValue.SetValue takes a double, so coerce
            // the string. Documented gap-closer (PR #1699 harness exposed).
            use windows::Win32::UI::Accessibility::{
                IUIAutomationRangeValuePattern, UIA_RangeValuePatternId,
            };
            if let Ok(pattern) = unsafe { elem.GetCurrentPattern(UIA_RangeValuePatternId) } {
                if let Ok(rv) = pattern.cast::<IUIAutomationRangeValuePattern>() {
                    let parsed: f64 = value.parse().map_err(|_| {
                        anyhow::anyhow!(
                            "set_value: target element exposes RangeValuePattern (Slider / \
                         ProgressBar / numeric range). `value` must be a parseable f64; \
                         got {value:?}."
                        )
                    })?;
                    crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                        rv.SetValue(parsed)
                    })?;
                    std::mem::forget(elem);
                    return Ok("RangeValuePattern".to_string());
                }
            }
            std::mem::forget(elem);
            anyhow::bail!(
                "set_value: element [{idx}] does not implement ValuePattern or \
                 RangeValuePattern. For controls with TogglePattern (CheckBox) or \
                 SelectionItemPattern (RadioButton / ComboBoxItem), use the `click` \
                 tool instead."
            );
        })
        .await;
        match result {
            Ok(Ok(pattern_name)) => {
                ToolResult::text(format!("✅ Set AXValue on [{idx}] (UIA {pattern_name})."))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── scroll ────────────────────────────────────────────────────────────────────

pub struct ScrollTool {
    state: Arc<ToolState>,
}
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
                "type":"object","required":["direction"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer","description":"Target process ID for window scope. Omit with scope=desktop for screen-absolute coordinates from get_desktop_state."},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "by":{"type":"string","enum":["line","page"],"description":"Scroll granularity. Default: line."},
                    "amount":{"type":"integer","minimum":1,"maximum":50,
                        "description":"Number of scroll ticks. Default 3."},
                    "x":{"type":"number","description":"With pid/window_id: window-local screenshot X used to target a nested scroll surface in foreground mode. Without pid/window_id: screen-absolute X for desktop scope. Must be paired with y."},
                    "y":{"type":"number","description":"With pid/window_id: window-local screenshot Y used to target a nested scroll surface in foreground mode. Without pid/window_id: screen-absolute Y for desktop scope. Must be paired with x."},
                    "window_id":{"type":"integer","description":"HWND of the target window. Required when element_index is used; otherwise auto-resolves the pid's first visible window."},
                    "element_index":{"type":"integer","description":"Optional element_index. Accepted for parity; currently no-op on Windows."},
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        // ── Window-less screen-absolute branch (capture_scope="desktop") ──────
        // No pid/window_id + numeric x,y + desktop scope → synthesize a wheel
        // event at the screen point via SendInput. The wheel routes to whatever
        // window is under (x,y). up/down map to a vertical wheel (sign), and
        // left/right to a horizontal wheel; `amount` is the tick count.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if !has_pid && !has_window_id && has_xy {
            if !is_windowless_desktop_action(&args) {
                return ToolResult::error(
                    "scroll: x,y given with no pid/window_id requires scope=\"desktop\"; \
                     use get_desktop_state to pick coordinates, or pass a pid/window_id.",
                )
                .with_structured(serde_json::json!({
                    "code": "desktop_coordinate_scope_required",
                    "suggestion": "pass scope=desktop",
                }));
            }
            let input = match parse_typed_projection::<ScrollInput>("scroll", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let sx = input.x as i32;
            let sy = input.y as i32;
            let direction = input.direction;
            let amount = input.amount.unwrap_or(3) as u32;
            // Direction → (horizontal?, sign). Positive ticks = up / right.
            let (horizontal, sign) = match direction {
                ScrollDirection::Up => (false, 1),
                ScrollDirection::Down => (false, -1),
                ScrollDirection::Right => (true, 1),
                ScrollDirection::Left => (true, -1),
            };
            let ticks = sign * amount as i32;
            let dir_disp = direction.as_str();
            let result = tokio::task::spawn_blocking(move || {
                crate::input::send_wheel_synthesized(sx, sy, ticks, horizontal)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Scrolled {dir_disp} via SendInput wheel ({amount} tick(s)) at screen ({sx},{sy}) (desktop scope)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e)     => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Window-scoped arguments retain the richer platform contract.
        let direction = match args.get("direction").and_then(|v| v.as_str()) {
            Some(d) => d.to_owned(),
            None => return ToolResult::error("Missing required string field direction."),
        };
        let amount = args.u64_or("amount", 3).clamp(1, 50) as u32;

        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let delivery = DeliveryMode::from_args(&args);
        let by = args.str_or("by", "line");
        let direction_display = direction.clone();
        let by_display = by.clone();
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "scroll",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index as u64)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.",
            );
        }

        // Resolve HWND: use window_id if given, else first window for pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // WebView2/Tauri hosts can expose the indexed scroll container through
        // UIA while their top-level HWND ignores WM_VSCROLL. Prefer the
        // accessibility channel for an indexed target; the message path below
        // remains the fallback for native Win32 scrollbars.
        if delivery == DeliveryMode::Background && crate::input::is_chromium_target_window(hwnd) {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }
        if let Some(idx) = elem_idx {
            let prev_fg_addr = if delivery == DeliveryMode::Background {
                Some(unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as isize
                })
            } else {
                None
            };
            let _noact = if delivery == DeliveryMode::Background {
                Some(crate::input::NoActivateGuard::arm(
                    windows::Win32::Foundation::HWND(hwnd as *mut _),
                ))
            } else {
                None
            };
            let state = self.state.clone();
            let direction_for_uia = direction.clone();
            let uia_result = tokio::task::spawn_blocking(move || {
                let retained = state
                    .element_cache
                    .get_element_retained(pid, hwnd, idx as usize)
                    .ok_or_else(|| anyhow::anyhow!("element [{idx}] is not in the UIA cache"))?;
                if !retained.is_uia() {
                    anyhow::bail!("element [{idx}] is not a UIA scroll element");
                }
                unsafe {
                    crate::uia::scroll::scroll_element(
                        retained.as_ptr(),
                        &direction_for_uia,
                        amount,
                    )
                }
            })
            .await;
            if matches!(uia_result, Ok(Ok(()))) {
                if delivery == DeliveryMode::Background {
                    // Keep WS_EX_NOACTIVATE armed through any WebView handler
                    // queued by the UIA operation.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    if let Some(previous_addr) = prev_fg_addr {
                        use windows::Win32::Foundation::HWND;
                        use windows::Win32::UI::WindowsAndMessaging::{
                            GetAncestor, GetForegroundWindow, GA_ROOT,
                        };
                        let previous = HWND(previous_addr as *mut _);
                        let current = unsafe { GetForegroundWindow() };
                        let current_root = unsafe { GetAncestor(current, GA_ROOT) };
                        let target_root = unsafe { GetAncestor(HWND(hwnd as *mut _), GA_ROOT) };
                        if !previous.0.is_null()
                            && current != previous
                            && !target_root.0.is_null()
                            && current_root == target_root
                        {
                            unsafe {
                                crate::input::force_foreground_attached(previous);
                                std::thread::sleep(std::time::Duration::from_millis(12));
                                crate::input::force_foreground_attached(previous);
                            }
                        }
                    }
                }
                return ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks via UIA (delivery_mode:background)."
                ))
                .with_structured(serde_json::json!({
                    "path": "uia",
                    "verified": false,
                    "delivery_mode": "background"
                }));
            }
        }

        if delivery == DeliveryMode::Background
            && args.get("x").is_some_and(serde_json::Value::is_number)
            && args.get("y").is_some_and(serde_json::Value::is_number)
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }

        // delivery_mode:"background" — WM_VSCROLL/HSCROLL is silently dropped by
        // Chromium and may be by GTK. Surface the standard structured
        // background_unavailable error: its remediation (bring_to_front +
        // delivery_mode:"foreground") is now a real recovery because scroll's
        // foreground path is implemented below (SendInput wheel reaches the
        // Chromium/GTK targets PostMessage can't).
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseScroll)
        {
            return crate::input::delivery::background_unavailable_error(
                hwnd,
                EventKind::MouseScroll,
            );
        }
        // delivery_mode:"foreground" — SendInput MOUSEEVENTF_WHEEL at the target
        // window's screen center (send_wheel_synthesized). The wheel routes to
        // the window under the cursor — queue-origin input Chromium/GTK accept —
        // so it lands where PostMessage WM_*SCROLL is dropped. Mirrors the
        // desktop-scope wheel branch above; `by` (line/page) folds into the tick
        // count (page ≈ 3 detents).
        if delivery == DeliveryMode::Foreground {
            let (horizontal, sign) = match direction.as_str() {
                "up" => (false, 1),
                "down" => (false, -1),
                "right" => (true, 1),
                "left" => (true, -1),
                other => {
                    return ToolResult::error(format!(
                        "scroll: unknown direction \"{other}\" — expected up, down, left, right."
                    ))
                }
            };
            let per: i32 = if by == "page" { 3 } else { 1 };
            let ticks = sign * (amount as i32) * per;
            // A supplied PX target is window-local in the get_window_state
            // bitmap. Route the wheel there so nested web scrollers receive it;
            // otherwise retain the whole-window center fallback.
            let px = args.get("x").and_then(|value| value.as_f64());
            let py = args.get("y").and_then(|value| value.as_f64());
            if px.is_some() != py.is_some() {
                return ToolResult::error("scroll requires x and y together.");
            }
            let center = if let (Some(x), Some(y)) = (px, py) {
                Some(bitmap_to_screen(hwnd, x as i32, y as i32))
            } else {
                tokio::task::spawn_blocking(move || {
                    crate::win32::list_windows(Some(pid))
                        .into_iter()
                        .find(|w| w.hwnd == hwnd)
                        .map(|w| (w.x + w.width / 2, w.y + w.height / 2))
                })
                .await
                .ok()
                .flatten()
            };
            let (cx, cy) = match center {
                Some(c) => c,
                None => {
                    return ToolResult::error(format!(
                        "scroll: could not resolve on-screen bounds for window {hwnd:#x} \
                     (pid {pid}) to target the wheel. The window may be minimized or \
                     off-screen — call bring_to_front first."
                    ))
                }
            };
            let dir_disp = direction.clone();
            let tick_disp = ticks.abs();
            let result = tokio::task::spawn_blocking(move || {
                crate::input::send_wheel_synthesized(cx, cy, ticks, horizontal)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Scrolled {dir_disp} via SendInput wheel ({tick_disp} tick(s)) at \
                     screen ({cx},{cy}) (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                PostMessageW, SB_LINEDOWN, SB_LINELEFT, SB_LINERIGHT, SB_LINEUP, SB_PAGEDOWN,
                SB_PAGELEFT, SB_PAGERIGHT, SB_PAGEUP, WM_HSCROLL, WM_VSCROLL,
            };

            let hwnd_win = HWND(hwnd as *mut _);
            let use_page = by == "page";
            let (msg, code) = match (direction.as_str(), use_page) {
                ("up", false) => (WM_VSCROLL, SB_LINEUP),
                ("up", true) => (WM_VSCROLL, SB_PAGEUP),
                ("down", false) => (WM_VSCROLL, SB_LINEDOWN),
                ("down", true) => (WM_VSCROLL, SB_PAGEDOWN),
                ("left", false) => (WM_HSCROLL, SB_LINELEFT),
                ("left", true) => (WM_HSCROLL, SB_PAGELEFT),
                ("right", false) => (WM_HSCROLL, SB_LINERIGHT),
                ("right", true) => (WM_HSCROLL, SB_PAGERIGHT),
                _ => (WM_VSCROLL, SB_LINEDOWN),
            };
            for _ in 0..amount {
                unsafe {
                    PostMessageW(hwnd_win, msg, WPARAM(code.0 as usize), LPARAM(0))?;
                }
            }
            Ok(())
        })
        .await;

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format shape 1:1 (Swift uses key names
                // like "pagedown"; Windows uses Win32 SB_* constants — show
                // the actual mechanism for traceability).
                let mech_name = match (direction_display.as_str(), by_display.as_str()) {
                    ("up", "page") => "SB_PAGEUP",
                    ("down", "page") => "SB_PAGEDOWN",
                    ("left", "page") => "SB_PAGELEFT",
                    ("right", "page") => "SB_PAGERIGHT",
                    ("up", _) => "SB_LINEUP",
                    ("down", _) => "SB_LINEDOWN",
                    ("left", _) => "SB_LINELEFT",
                    ("right", _) => "SB_LINERIGHT",
                    _ => "SB_LINEDOWN",
                };
                ToolResult::text(format!(
                    "✅ Scrolled pid {raw_pid} {direction_display} via {amount}× {mech_name} message(s)."
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// `ScreenshotTool` and `ScreenshotCompatTool` were removed in PR #1692 —
// `get_window_state` (which now always returns a screenshot) is the single
// canonical screenshot path. The underlying capture functions
// (`crate::capture::screenshot_window_bytes_with_occlusion`,
// `screenshot_display_bytes`, etc.) and the WGC backend
// (`crate::wgc::screenshot_window_via_wgc`) are still in use by
// `GetWindowStateTool` — that's where the actual screenshot machinery
// lives now.

/// Chromium/Electron windows silently drop `PostMessage` mouse events — their
/// input thread only honors SendInput-origin events (#1623). `ClickTool`
/// auto-routes Chromium targets through SendInput, but `DoubleClickTool` /
/// `RightClickTool` did not, so element/pixel gestures on Electron apps
/// (Obsidian, VS Code, Slack, …) reached `post_click_screen` and no-op'd
/// silently. This mirrors that short-circuit for those tools (#1984): when
/// `hwnd` is a Chromium window, deliver `count` clicks of `button` at screen
/// `(sx, sy)` via SendInput with async foreground restore and return
/// `Some(result)`. Returns `None` for non-Chromium targets so the caller
/// proceeds to its normal PostMessage path.
async fn chromium_click_short_circuit(
    hwnd: u64,
    sx: i32,
    sy: i32,
    count: usize,
    button: &str,
    pid: u32,
    gesture: &str,
) -> Option<ToolResult> {
    let is_chromium =
        tokio::task::spawn_blocking(move || crate::input::is_chromium_target_window(hwnd))
            .await
            .unwrap_or(false);
    if !is_chromium {
        return None;
    }
    // Capture the pre-click foreground so the poller can restore it even if
    // Chromium re-activates itself from a renderer-side handler (same pattern
    // and rationale as the ClickTool Chromium branch).
    let prev_fg_addr =
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize };
    let button_owned = button.to_string();
    let send_result = tokio::task::spawn_blocking(move || {
        crate::input::send_click_synthesized(hwnd, sx, sy, count, &button_owned)
    })
    .await;
    tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
    Some(match send_result {
        Ok(Ok(())) => ToolResult::text(format!(
            "✅ Sent {gesture} via SendInput to pid {pid} at screen ({sx},{sy}) (Chromium target)."
        )),
        Ok(Err(e)) => ToolResult::error(e.to_string()),
        Err(e) => ToolResult::error(format!("Task error: {e}")),
    })
}

/// Fire UIA `InvokePattern.Invoke()` `count` times (min 2) in rapid succession
/// on a cached WinUI3 element. Two rapid Invokes land within the system
/// double-click time, so a Button's Click handler folds them into a
/// double-click (the harness records `last_action=double_click`).
///
/// This is the **only** background actuator that lands on a WinUI3 element: the
/// content island ignores posted `WM_*BUTTON` messages (measured: a posted
/// secondary click never fires `RightTapped`), and the synthetic pen/touch
/// injector click-activates the frame (8/8 foreground steals). UIA delivery
/// goes through the accessibility channel, not the input queue.
///
/// Contract: a XAML `Invoke` handler calls `SetFocus()` → `SetForegroundWindow(self)`,
/// which the `EnableWindow` UWP bypass does NOT gate (measured: a bare double
/// Invoke stole foreground). We therefore hold the frame **non-activatable**
/// (`WS_EX_NOACTIVATE`, via `NoActivateGuard`) for the whole burst, which is
/// what makes Windows refuse the self-activation — the same guard ClickTool
/// already relies on for single-click. Runs blocking COM, so call inside
/// `spawn_blocking`. Returns `Some(Ok)` on success, `Some(Err)` if Invoke
/// failed, `None` if the element isn't cached or has no InvokePattern.
fn winui3_uia_multi_invoke(
    state: &Arc<ToolState>,
    pid: u32,
    hwnd: u64,
    idx: usize,
    count: usize,
) -> Option<anyhow::Result<()>> {
    let guard = state.element_cache.get_element_retained(pid, hwnd, idx)?;
    let ptr = guard.as_ptr();
    use windows::core::Interface;
    use windows::Win32::UI::Accessibility::{
        IUIAutomationElement, IUIAutomationInvokePattern, UIA_InvokePatternId,
    };
    // Hold the frame non-activatable so the XAML Invoke handler's
    // SetFocus()→SetForegroundWindow(self) is refused for the whole burst.
    let _noact =
        crate::input::NoActivateGuard::arm(windows::Win32::Foundation::HWND(hwnd as *mut _));
    let elem: IUIAutomationElement = unsafe { IUIAutomationElement::from_raw(ptr as *mut _) };
    let outcome: Option<anyhow::Result<()>> = (|| {
        let pattern = unsafe { elem.GetCurrentPattern(UIA_InvokePatternId) }.ok()?;
        let inv = pattern.cast::<IUIAutomationInvokePattern>().ok()?;
        let n = count.max(2);
        let mut res = Ok(());
        for i in 0..n {
            let r = crate::uia::fg_bypass::run_with_uwp_bypass(hwnd as isize, || unsafe {
                inv.Invoke()
            });
            if let Err(e) = r {
                res = Err(anyhow::anyhow!("UIA Invoke #{i}: {e}"));
                break;
            }
            if i + 1 < n {
                std::thread::sleep(std::time::Duration::from_millis(40));
            }
        }
        Some(res)
    })();
    // The cache owns this element's ref (AddRef'd under the cache lock by
    // get_element_retained); `from_raw` took ownership without AddRef, so forget
    // it to avoid double-releasing the cache's ref.
    std::mem::forget(elem);
    outcome
}

/// WinUI3 background gesture resolution, shared by click / double_click /
/// right_click / drag. For a WinUI3 (`WinUIDesktopWin32WindowClass`) target:
///
///  - **double-click (left)** on a cached InvokePattern element → a double UIA
///    Invoke (lands + holds the no-foreground contract; see
///    [`winui3_uia_multi_invoke`]).
///  - **everything else** — right/middle click, double-click without an
///    invokable cached element, any drag — cannot both land AND hold the
///    contract on WinUI3: UIA has no right-click/drag analogue, the content
///    island ignores posted `WM_*BUTTON`, and the pointer injector
///    click-activates the frame. So we return the structured
///    `background_unavailable` error (the honest result — the caller can retry
///    with `delivery_mode:"foreground"` or `bring_to_front`) instead of a silent
///    no-op that reports false success.
///
/// Returns `None` for non-WinUI3 targets (caller falls through to its normal
/// routing).
async fn winui3_background_gesture(
    state: &Arc<ToolState>,
    pid: u32,
    hwnd: u64,
    idx: Option<usize>,
    count: usize,
    button: &str,
) -> Option<ToolResult> {
    let is_w =
        tokio::task::spawn_blocking(move || crate::input::delivery::is_winui3_target_window(hwnd))
            .await
            .unwrap_or(false);
    if !is_w {
        return None;
    }
    if count >= 2 && button == "left" {
        if let Some(idx) = idx {
            let st = state.clone();
            let uia = tokio::task::spawn_blocking(move || {
                winui3_uia_multi_invoke(&st, pid, hwnd, idx, count)
            })
            .await
            .ok()
            .flatten();
            if let Some(Ok(())) = uia {
                return Some(ToolResult::text(format!(
                    "✅ Double-invoked WinUI3 element [{idx}] x{count} via UIA \
                     (pid {pid}, background, no foreground swap)."
                )));
            }
        }
    }
    Some(crate::input::delivery::background_unavailable_error(
        hwnd,
        crate::input::delivery::EventKind::MouseClick,
    ))
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
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used. Optional when element_token is supplied (the token carries it)."},
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "x":{"type":"number","description":"X in window-local screenshot pixels. Must be provided together with y."},
                "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "from_zoom":{"type":"boolean","description":"After a zoom call, pass true to translate zoom-image coords back to window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        use cua_driver_core::tool_args::ArgsExt;
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "double_click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        let delivery = DeliveryMode::from_args(&args);
        let cursor_key = resolve_cursor_key(&args);
        // Swift validates "both x and y or neither" and "no element_index without window_id".
        let has_xy = x.is_some() && y.is_some();
        let partial_xy = x.is_some() != y.is_some();
        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if elem_idx.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if elem_idx.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_index or (x, y) to address the double-click target.",
            );
        }
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.",
            );
        }

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        if let Some(idx) = elem_idx {
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => {
                    return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                }
            };
            let (cx, cy) = match resolve_onscreen_point_with_scroll(
                &self.state.element_cache,
                pid,
                hwnd,
                idx,
                cx,
                cy,
                "double-clicking",
            ) {
                Ok(p) => p,
                Err(msg) => return ToolResult::error(msg),
            };
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, cx as f64, cy as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
            // delivery_mode:"background" (default) on WinUI3: the pen/touch injector
            // click-activates the content island (8/8 foreground steals) and
            // posted WM_*BUTTON to the frame/island no-ops (measured). A double
            // UIA Invoke on the cached element is the only contract-holding way
            // to land the double-click — it fires the XAML Click handler twice
            // → last_action=double_click, held non-activatable so no foreground
            // swap. If the element has no InvokePattern, a background double-
            // click isn't expressible → structured error.
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, Some(idx), 2, "left").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): Chromium/Electron & GTK targets
            // silently drop posted clicks (#1984) — inject via the coordinate
            // actuator (system input queue, NO foreground swap), exactly like
            // ClickTool, instead of refusing. Only error if injection can't
            // express this click (e.g. right/middle on such a target).
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, cx, cy, 2, "left")
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected double-click to [{idx}] at screen ({cx},{cy}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // delivery_mode:"foreground" — route through SendInput at the cached coords.
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, cx, cy, 2, "left")
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent double-click via SendInput on [{idx}] at screen ({cx},{cy}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, cx, cy, 2, "left", pid, "double-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                crate::input::post_click_screen(hwnd, cx, cy, 2, "left")?;
                // Swift text format 1:1: `"✅ Posted double-click to [N] role \"title\" at screen-point (X, Y)."`.
                // UIA role/title placeholder pending element-cache enrichment.
                Ok(format!(
                    "✅ Posted double-click to [{idx}] at screen-point ({cx}, {cy})."
                ))
            })
            .await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(px, py);
                        px = wx;
                        py = wy;
                    }
                    None => {
                        return ToolResult::error(format!(
                            "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                        ))
                    }
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            // bitmap pixels -> screen via DWM-frame origin (see
            // `bitmap_to_screen` doc for why ClientToScreen is wrong).
            let (sx_i, sy_i) = bitmap_to_screen(hwnd, px as i32, py as i32);
            let (sx, sy) = (sx_i as f64, sy_i as f64);
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx, sy).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
            // delivery_mode:"background" on WinUI3 (pixel path, no element_index): a
            // background double-click on WinUI3 only lands via UIA Invoke on a
            // cached element, which the pixel path doesn't have → structured
            // error (caller retries with delivery_mode:foreground or uses element_index).
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, None, 2, "left").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): inject via the coordinate actuator
            // (no foreground swap) for targets that drop posted clicks (#1984).
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, sx_i, sy_i, 2, "left")
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected double-click to pid {pid} at screen ({sx_i},{sy_i}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // delivery_mode:"foreground" — SendInput at screen coords with FG swap.
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, sx_i, sy_i, 2, "left")
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent double-click via SendInput to pid {pid} at screen ({sx_i},{sy_i}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            let (xi, yi) = (px as i32, py as i32);
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, sx_i, sy_i, 2, "left", pid, "double-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking(move || {
                crate::input::post_click_screen(hwnd, sx_i, sy_i, 2, "left")
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Match Swift's pixel-path text 1:1.
                    ToolResult::text(format!(
                        "✅ Posted double-click to pid {pid} at window-pixel ({xi}, {yi}) → screen-point ({sx_i}, {sy_i})."))
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
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"HWND for the target window. Required when element_index is used. Optional when element_token is supplied (the token carries it)."},
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "x":{"type":"number","description":"X in window-local screenshot pixels. Must be provided together with y."},
                "y":{"type":"number","description":"Y in window-local screenshot pixels. Must be provided together with x."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "from_zoom":{"type":"boolean","description":"After a zoom call, pass true to translate zoom-image coords back to window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        use cua_driver_core::tool_args::ArgsExt;
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "right_click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let hwnd_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        let x = args.opt_f64("x");
        let y = args.opt_f64("y");
        let delivery = DeliveryMode::from_args(&args);
        let cursor_key = resolve_cursor_key(&args);
        // Port Swift's full validation set.
        let has_xy = x.is_some() && y.is_some();
        let partial_xy = x.is_some() != y.is_some();
        if partial_xy {
            return ToolResult::error("Provide both x and y together, not just one.");
        }
        if elem_idx.is_some() && has_xy {
            return ToolResult::error("Provide either element_index or (x, y), not both.");
        }
        if elem_idx.is_none() && !has_xy {
            return ToolResult::error(
                "Provide element_index or (x, y) to address the right-click target.",
            );
        }
        if elem_idx.is_some() && hwnd_opt.is_none() {
            return ToolResult::error(
                "window_id is required when element_index is used — the element_index cache \
                 is scoped per (pid, window_id). Pass the same window_id you used in \
                 `get_window_state`.",
            );
        }

        // Resolve HWND: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        if let Some(idx) = elem_idx {
            let (cx, cy) = match self.state.element_cache.get_element_center(pid, hwnd, idx) {
                Some(v) => v,
                None => {
                    return ToolResult::error(format!(
                        "Element {idx} not in cache for hwnd={hwnd}. Call get_window_state first."
                    ))
                }
            };
            let (cx, cy) = match resolve_onscreen_point_with_scroll(
                &self.state.element_cache,
                pid,
                hwnd,
                idx,
                cx,
                cy,
                "right-clicking",
            ) {
                Ok(p) => p,
                Err(msg) => return ToolResult::error(msg),
            };
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, cx as f64, cy as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: cx as f64,
                    y: cy as f64,
                },
            );
            // delivery_mode:"background" on WinUI3: a right-click can't both land and
            // hold the contract — UIA has no right-click pattern, the content
            // island ignores posted WM_RBUTTON (measured: RightTapped never
            // fired), and the pen-barrel injector click-activates the frame.
            // Return the structured error (retry with delivery_mode:foreground).
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, Some(idx), 1, "right").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): try coordinate injection (no
            // foreground swap) for drop-prone targets (#1984); a right-click
            // injection that the actuator can't express falls back to the error.
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, cx, cy, 1, "right")
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected right-click to [{idx}] at screen ({cx},{cy}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, cx, cy, 1, "right")
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent right-click via SendInput on [{idx}] at screen ({cx},{cy}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, cx, cy, 1, "right", pid, "right-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                crate::input::post_click_screen(hwnd, cx, cy, 1, "right")?;
                // Match Swift's element-path text 1:1
                // (`"✅ Shown menu for [N] role \"title\"."`).  UIA role/title
                // placeholder pending element-cache enrichment.
                Ok(format!("✅ Shown menu for [{idx}] (screen ({cx}, {cy}))."))
            })
            .await;
            match result {
                Ok(Ok(msg)) => ToolResult::text(msg),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            }
        } else if let (Some(mut px), Some(mut py)) = (x, y) {
            let from_zoom = args
                .get("from_zoom")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if from_zoom {
                match self.state.zoom_registry.get(pid) {
                    Some(ctx) => {
                        let (wx, wy) = ctx.zoom_to_window(px, py);
                        px = wx;
                        py = wy;
                    }
                    None => {
                        return ToolResult::error(format!(
                            "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                        ))
                    }
                }
            } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
                px *= ratio;
                py *= ratio;
            }
            // bitmap pixels -> screen via DWM-frame origin (see
            // `bitmap_to_screen` doc).
            let (sx_i, sy_i) = bitmap_to_screen(hwnd, px as i32, py as i32);
            let (sx, sy) = (sx_i as f64, sy_i as f64);
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx, sy).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
            // delivery_mode:"background" on WinUI3: right-click can't land while
            // holding the contract (no UIA right-click, posted WM_RBUTTON
            // ignored, pen-barrel injector steals foreground) → structured error.
            if delivery == DeliveryMode::Background {
                if let Some(r) =
                    winui3_background_gesture(&self.state, pid, hwnd, None, 1, "right").await
                {
                    return r;
                }
            }
            // delivery_mode:"background" (default): try coordinate injection (no
            // foreground swap) for drop-prone targets (#1984).
            if delivery == DeliveryMode::Background
                && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseClick)
            {
                let inj = tokio::task::spawn_blocking(move || {
                    crate::input::inject_click_screen(hwnd, sx_i, sy_i, 1, "right")
                })
                .await;
                return match inj {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Injected right-click to pid {pid} at screen ({sx_i},{sy_i}) (background, no foreground swap)."
                    )),
                    Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                        hwnd,
                        EventKind::MouseClick,
                        e.to_string(),
                    ),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            if delivery == DeliveryMode::Foreground {
                let prev_fg_addr = unsafe {
                    windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
                };
                let send_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_click_synthesized(hwnd, sx_i, sy_i, 1, "right")
                })
                .await;
                tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
                return match send_result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "✅ Sent right-click via SendInput to pid {pid} at screen ({sx_i},{sy_i}) (delivery_mode:foreground)."
                    )),
                    Ok(Err(e)) => ToolResult::error(e.to_string()),
                    Err(e) => ToolResult::error(format!("Task error: {e}")),
                };
            }
            let (xi, yi) = (px as i32, py as i32);
            // Chromium/Electron silently drops PostMessage clicks (#1984) — route via SendInput.
            if let Some(r) =
                chromium_click_short_circuit(hwnd, sx_i, sy_i, 1, "right", pid, "right-click").await
            {
                return r;
            }
            let result = tokio::task::spawn_blocking(move || {
                crate::input::post_click_screen(hwnd, sx_i, sy_i, 1, "right")
            })
            .await;
            match result {
                Ok(Ok(())) => {
                    // Swift pixel-path text 1:1.
                    ToolResult::text(format!(
                        "✅ Posted right-click to pid {pid} at window-pixel ({xi}, {yi}) → screen-point ({sx_i}, {sy_i})."))
                }
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
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
            input_schema: json!({"type":"object","required":["from_x","from_y","to_x","to_y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "scope":{"type":"string","enum":["window","desktop"],"description":"Use desktop with no pid/window_id for screen-absolute coordinates."},
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Target window handle (HWND). Optional — driver picks frontmost window of pid when omitted."},
                "from_x":{"type":"number","description":"Drag-start X in window-local screenshot pixels."},
                "from_y":{"type":"number","description":"Drag-start Y in window-local screenshot pixels."},
                "to_x":{"type":"number","description":"Drag-end X in window-local screenshot pixels."},
                "to_y":{"type":"number","description":"Drag-end Y in window-local screenshot pixels."},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Wall-clock duration of drag path. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Number of intermediate WM_MOUSEMOVE events. Default: 20."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean","description":"When true, coordinates are in the last zoom image for this pid."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use crate::input::delivery::{DeliveryMode, EventKind};
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<DragInput>("drag", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let from_x = input.from_x as i32;
            let from_y = input.from_y as i32;
            let to_x = input.to_x as i32;
            let to_y = input.to_y as i32;
            let duration_ms = input.duration_ms.unwrap_or(500);
            let steps = input.steps.unwrap_or(20) as usize;
            let button = input.button.unwrap_or(ClickButton::Left).as_str();
            let hwnd = match crate::input::mouse::foreground_window() {
                Ok(hwnd) => hwnd,
                Err(error) => return ToolResult::error(error.to_string()),
            };
            return match tokio::task::spawn_blocking(move || {
                crate::input::mouse::send_drag_synthesized(
                    hwnd,
                    from_x,
                    from_y,
                    to_x,
                    to_y,
                    duration_ms,
                    steps,
                    &button,
                )
            })
            .await
            {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Dragged from ({from_x}, {from_y}) to ({to_x}, {to_y}) (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SendInput",
                    "from": {"x": from_x, "y": from_y}, "to": {"x": to_x, "y": to_y},
                    "effect": "unverifiable"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("desktop drag task failed: {error}")),
            };
        }
        // Swift error wording 1:1.
        let raw_pid = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) => p,
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = raw_pid as u32;
        let delivery = DeliveryMode::from_args(&args);

        let cursor_key = resolve_cursor_key(&args);
        // Accepts numeric JSON as either float or integer — coerce both to f64.
        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key)
                .or_else(|| args.opt_i64(key).map(|i| i as f64))
        };
        let from_x_opt = coerce("from_x");
        let from_y_opt = coerce("from_y");
        let to_x_opt = coerce("to_x");
        let to_y_opt = coerce("to_y");
        let (mut from_x, mut from_y, mut to_x, mut to_y) =
            match (from_x_opt, from_y_opt, to_x_opt, to_y_opt) {
                (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
                _ => {
                    return ToolResult::error(
                        "from_x, from_y, to_x, and to_y are all required (window-local pixels).",
                    )
                }
            };

        let hwnd_opt = args.opt_u64("window_id");
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = args.u64_or("steps", 20) as usize;
        let button = args.str_or("button", "left");
        let from_zoom = args.bool_or("from_zoom", false);

        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(from_x, from_y);
                    let (wx2, wy2) = ctx.zoom_to_window(to_x, to_y);
                    from_x = wx;
                    from_y = wy;
                    to_x = wx2;
                    to_y = wy2;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            from_x *= ratio;
            from_y *= ratio;
            to_x *= ratio;
            to_y *= ratio;
        }

        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Compute screen-coord endpoints. Same correction as the click
        // tools — bitmap pixels are anchored to the DWM-frame top-left,
        // not the client area top-left (see `bitmap_to_screen` doc).
        let (sx_from, sy_from) = bitmap_to_screen(hwnd, from_x as i32, from_y as i32);
        let (sx_to, sy_to) = bitmap_to_screen(hwnd, to_x as i32, to_y as i32);

        // delivery_mode:"background" on WinUI3: a pointer drag can't both land and
        // hold the contract — the content island only consumes real
        // system-queue pointer input (which the pen/touch injector delivers but
        // at the cost of click-activating the frame: 8/8 foreground steals) and
        // ignores posted WM_* drag messages; UIA has no drag analogue. Return
        // the structured error so the caller retries with delivery_mode:foreground
        // (a WinUI3 Slider can also be set with the set_value tool, which uses
        // UIA RangeValuePattern and holds the contract).
        if delivery == DeliveryMode::Background {
            if let Some(r) =
                winui3_background_gesture(&self.state, pid, hwnd, None, 1, &button).await
            {
                return r;
            }
        }

        // delivery_mode:"background" — if a PostMessage drag would silently drop
        // (Chromium/WPF/GTK canvas content reads mouse from the system input
        // queue, not the per-window queue), fall back to coordinate-routed
        // synthetic-pen drag injection instead of refusing. No foreground swap,
        // no cursor move; the target is held non-activatable + cloaked for the
        // stroke (mirrors the click pen path).
        if delivery == DeliveryMode::Background
            && crate::input::delivery::would_be_silently_dropped(hwnd, EventKind::MouseMove)
        {
            let target = hwnd;
            let btn = button.clone();
            pin_overlay_above(&cursor_key, hwnd);
            overlay_glide_to(&cursor_key, sx_from as f64, sy_from as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx_from as f64,
                    y: sy_from as f64,
                },
            );
            let inj = tokio::task::spawn_blocking(move || {
                crate::input::inject::inject_drag_screen(
                    target,
                    sx_from,
                    sy_from,
                    sx_to,
                    sy_to,
                    steps.max(8),
                    &btn,
                )
            })
            .await;
            return match inj {
                Ok(Ok(())) => {
                    overlay_glide_to(&cursor_key, sx_to as f64, sy_to as f64).await;
                    crate::overlay::send_command(
                        cursor_key.clone(),
                        cursor_overlay::OverlayCommand::ClickPulse {
                            x: sx_to as f64,
                            y: sy_to as f64,
                        },
                    );
                    ToolResult::text(format!(
                        "✅ Sent drag via synthetic-pen injection on pid {raw_pid} \
                         from screen ({sx_from},{sy_from}) → ({sx_to},{sy_to}) \
                         (delivery_mode:background, PostMessage would have been dropped)."
                    ))
                }
                Ok(Err(e)) => crate::input::delivery::background_unavailable_error_with_cause(
                    hwnd,
                    EventKind::MouseMove,
                    e.to_string(),
                ),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        // delivery_mode:"foreground" — SendInput-based drag. Required for WPF
        // Slider thumbs (and any framework that polls GetKeyState during
        // its drag handler — PostMessage doesn't update per-thread input
        // state, so those targets see a button-up world during the drag
        // and never start tracking). Subject to the same UIAccess
        // foreground-lock constraint as send_click_synthesized.
        if delivery == DeliveryMode::Foreground {
            let btn_fg = button.clone();
            let prev_fg_addr = unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow().0 as usize
            };
            let send_result = tokio::task::spawn_blocking(move || {
                crate::input::mouse::send_drag_synthesized(
                    hwnd,
                    sx_from,
                    sy_from,
                    sx_to,
                    sy_to,
                    duration_ms,
                    steps,
                    &btn_fg,
                )
            })
            .await;
            tokio::spawn(restore_foreground_polling_best_effort(prev_fg_addr, pid));
            let button_suffix = if button == "left" {
                String::new()
            } else {
                format!(" ({} button)", button)
            };
            return match send_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent drag{button_suffix} via SendInput on pid {raw_pid} \
                     from screen ({sx_from},{sy_from}) → ({sx_to},{sy_to}) \
                     in {duration_ms}ms / {steps} steps (delivery_mode:foreground)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Pin the agent-cursor overlay above the drag target so the synthetic
        // cursor stays sandwiched at z+1 of the dragged window for the full
        // path. Drag stays within a single HWND, so one pin at the start is
        // sufficient — the 80 ms z-order tick keeps it asserted thereafter.
        pin_overlay_above(&cursor_key, hwnd);

        // Animate the agent cursor to the drag-start, fire a press pulse,
        // run the actual drag synthesis, then glide to the drag-end and
        // fire a release pulse. Mirrors macOS `tools/drag.rs:191-242`.
        // Cursor doesn't follow the interpolated PostMessage path during
        // the drag itself (the timing coordination would be invasive);
        // pre- and post-glides plus the press/release pulses are enough
        // signal for a user watching the agent operate.
        overlay_glide_to(&cursor_key, sx_from as f64, sy_from as f64).await;
        crate::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::ClickPulse {
                x: sx_from as f64,
                y: sy_from as f64,
            },
        );

        let button_c = button.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Screen-coord, deepest-child variant: routes the gesture to the
            // child control under the start point (e.g. a WinForms Panel),
            // not the top-level frame that would ignore it.
            crate::input::mouse::post_drag_screen(
                hwnd,
                sx_from,
                sy_from,
                sx_to,
                sy_to,
                duration_ms,
                steps,
                &button_c,
            )
        })
        .await;

        // Drag finished — glide the visual cursor to the drag-end and
        // pulse the release. Skipped on error so the cursor doesn't lie
        // about a successful endpoint.
        if matches!(&result, Ok(Ok(()))) {
            overlay_glide_to(&cursor_key, sx_to as f64, sy_to as f64).await;
            crate::overlay::send_command(
                cursor_key.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx_to as f64,
                    y: sy_to as f64,
                },
            );
        }

        match result {
            Ok(Ok(())) => {
                // Match Swift's text format verbatim.
                let button_suffix = if button == "left" {
                    String::new()
                } else {
                    format!(" ({} button)", button)
                };
                ToolResult::text(format!(
                    "✅ Posted drag{button_suffix} to pid {raw_pid} \
                     from window-pixel ({}, {}) → ({}, {}), \
                     screen ({sx_from}, {sy_from}) → ({sx_to}, {sy_to}) \
                     in {duration_ms}ms / {steps} steps.",
                    from_x as i32, from_y as i32, to_x as i32, to_y as i32,
                ))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── get_screen_size ───────────────────────────────────────────────────────────

/// Read the primary display size in PHYSICAL pixels.
///
/// With permonitorv2 DPI awareness (set in cua-driver.manifest),
/// `SM_CXSCREEN` / `SM_CYSCREEN` already return physical pixels — the same
/// coordinate space screenshots and pixel clicks use on Windows. Shared by
/// `get_screen_size` and `get_desktop_state`.
fn physical_screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

pub struct GetScreenSizeTool;
static GSS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetScreenSizeTool {
    fn def(&self) -> &ToolDef {
        GSS_DEF.get_or_init(|| ToolDef {
            name: "get_screen_size".into(),
            description: "Return the size of the main display in physical pixels plus its display \
                scale factor. On Windows, screenshots and pixel clicks use this same physical-pixel \
                coordinate space. Requires no special permissions.".into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema()
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(error) = parse_typed_input::<GetScreenSizeInput>("get_screen_size", args) {
            return error;
        }
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        // With permonitorv2 DPI awareness (set in cua-driver.manifest),
        // SM_CXSCREEN/SM_CYSCREEN return PHYSICAL pixels — the same
        // coordinate space screenshots and pixel clicks use on Windows.
        // Report these as-is, along with the scale factor for reference.
        let (w, h) = physical_screen_size();
        let dpi = unsafe { GetDpiForSystem() };
        let scale = if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 };
        ToolResult::text(format!("✅ Main display: {w}x{h} pixels @ {scale}x"))
            .with_structured(json!({ "width": w, "height": h, "scale_factor": scale }))
    }
}

// ── get_desktop_state ─────────────────────────────────────────────────────────

/// `get_desktop_state` — full-display vision screenshot (Windows).
///
/// Vision-only desktop capture: grabs the ENTIRE primary display at native
/// physical-pixel size (no downscale) so screen-absolute pixel picks land
/// exactly, then reports the true screen size. No UIA walk, no pid/window_id —
/// this is the capture surface for `capture_scope="desktop"` GUI loops where
/// the agent drives `click(x,y)` / `scroll(x,y)` against screen-absolute
/// coordinates.
///
/// Mirrors the `get_window_state` vision branch's ToolResult shape: an
/// `image_png` content part (or a written-out file path), a text summary line,
/// and a `structuredContent` object.
pub struct GetDesktopStateTool;
static GDS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        GDS_DEF.get_or_init(|| ToolDef {
            name: "get_desktop_state".into(),
            description: "Capture a full-display vision screenshot in true screen pixels \
                (no downscale), for capture_scope=\"desktop\" GUI loops where the agent then \
                drives click(x,y)/scroll(x,y) with no pid/window_id. Returns the PNG at native \
                display resolution plus the true screen size so screen-absolute pixel picks \
                land exactly. Vision-only: no UIA tree walk.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session": { "type": "string", "description": "Optional session id." },
                    "screenshot_out_file": { "type": "string", "description": "Write PNG here instead of base64." }
                },
                "additionalProperties": false
            }),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::protocol::Content;
        let input = match parse_typed_input::<GetDesktopStateInput>("get_desktop_state", args) {
            Ok(input) => input,
            Err(error) => return error,
        };
        let screenshot_out_file = input.screenshot_out_file;

        // True screen geometry in physical pixels (same space as the capture).
        let (screen_width, screen_height) = physical_screen_size();

        // Capture the FULL display at native size — no resize. Run the
        // blocking GDI capture off the async runtime.
        let out_file = screenshot_out_file.clone();
        let res = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(Option<String>, Option<String>, u32, u32)> {
                use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
                let png = crate::capture::screenshot_display_bytes()?;
                let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                if let Some(ref path) = out_file {
                    std::fs::write(path, &png)?;
                    Ok((None, Some(path.clone()), w, h))
                } else {
                    Ok((Some(BASE64.encode(&png)), None, w, h))
                }
            },
        )
        .await;

        let (b64_opt, file_path, screenshot_width, screenshot_height) = match res {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolResult::error(format!("Desktop screenshot failed: {e}")),
            Err(e) => return ToolResult::error(format!("Desktop screenshot task error: {e}")),
        };

        let mut content: Vec<Content> = Vec::new();
        if let Some(b64) = b64_opt {
            content.push(Content::image_png(b64));
        }
        let summary = format!(
            "desktop screenshot {screenshot_width}x{screenshot_height} px \
             (screen {screen_width}x{screen_height} px)"
        );
        content.push(Content::text(summary));

        let mut structured = json!({
            "platform": "windows",
            "display": "primary",
            "screenshot_width": screenshot_width,
            "screenshot_height": screenshot_height,
            "screen_width": screen_width,
            "screen_height": screen_height,
            "scale_factor": unsafe {
                let dpi = windows::Win32::UI::HiDpi::GetDpiForSystem();
                if dpi == 0 { 1.0 } else { dpi as f64 / 96.0 }
            },
            "screenshot_mime_type": "image/png",
        });
        if let Some(ref fp) = file_path {
            structured["screenshot_file_path"] = json!(fp);
        }

        ToolResult {
            content,
            is_error: None,
            structured_content: Some(structured),
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
            description:
                "Return the current mouse cursor position in screen points (origin top-left)."
                    .into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema()
            },"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if let Err(error) = parse_typed_input::<GetCursorPositionInput>("get_cursor_position", args)
        {
            return error;
        }
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT { x: 0, y: 0 };
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
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
            description:
                "Move the agent cursor overlay to (x, y). Does NOT move the real mouse cursor."
                    .into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "scope":{"type":"string","enum":["window","desktop"],"description":"desktop moves the real OS pointer; window moves only the agent overlay."},
                "x":{"type":"number"},"y":{"type":"number"},"cursor_id":{"type":"string"}
            },"additionalProperties":false}),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("scope").and_then(Value::as_str) == Some("desktop") {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(error) => return error,
            };
            let x = input.x;
            let y = input.y;
            return match crate::input::mouse::move_cursor_desktop(x as i32, y as i32) {
                Ok(()) => ToolResult::text(format!(
                    "Moved the OS pointer to ({x:.1}, {y:.1}) (scope:desktop)."
                ))
                .with_structured(json!({
                    "scope": "desktop", "path": "SetCursorPos", "x": x, "y": y
                })),
                Err(error) => ToolResult::error(error.to_string()),
            };
        }
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        // Cursor key precedence: caller-declared `session` > legacy `cursor_id`
        // > NO_CURSOR. An anonymous run (no session) has no cursor to move.
        let cursor_key = resolve_cursor_key(&args);
        if !cursor_key.is_empty() {
            self.state
                .cursor_registry
                .update_position(&cursor_key, x, y);
        }
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so
        // the cursor settles to the natural macOS-style pose.
        crate::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
            },
        );
        let shown = if cursor_key.is_empty() {
            "default"
        } else {
            cursor_key.as_str()
        };
        ToolResult::text(format!("Agent cursor '{shown}' moved to ({x:.1}, {y:.1})."))
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
            None => return ToolResult::error("Missing required boolean field `enabled`."),
        };
        let cursor_key = resolve_cursor_key(&args);
        if !cursor_key.is_empty() {
            self.state.cursor_registry.set_enabled(&cursor_key, enabled);
        }
        crate::overlay::send_command(
            cursor_key.clone(),
            cursor_overlay::OverlayCommand::SetEnabled(enabled),
        );
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
            description: format!("Configure the visual appearance and motion curve of an agent cursor instance.\n\n\
                Appearance:\n\
                - cursor_id: instance name (default='default')\n\
                - cursor_icon: built-in ({}) or a path to a PNG/JPEG/SVG/ICO file; '' reverts to the default cursor\n\
                - cursor_color: hex color e.g. '#00FFFF' or CSS name\n\
                - cursor_label: short text shown near the cursor\n\
                - cursor_size: dot radius in points (default=16)\n\
                - cursor_opacity: 0.0–1.0 (default=0.85)\n\n\
                Motion curve (Bezier):\n\
                - arc_size: perpendicular deflection as fraction of path length [0,1]. Default 0.25\n\
                - spring: settle damping [0.3,1.0]; 1.0=no overshoot. Default 0.72\n\
                - glide_duration_ms: fixed flight duration per move [50,5000]; omit for speed-based (the default)\n\
                - dwell_after_click_ms: pause after click ripple [0,5000]. Default 80\n\
                - idle_hide_ms: auto-hide delay [0,60000]; 0=never. Default 20000",
                cursor_overlay::BuiltinShape::names_help()),
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
                    "glide_duration_ms":{"type":"number","minimum":50,"maximum":5000,"description":"Fixed flight duration per move in ms; omit for speed-based timing (the default)."},
                    "dwell_after_click_ms":{"type":"number","minimum":0,"maximum":5000,"description":"Pause after click ripple in ms. Default 80."},
                    "idle_hide_ms":{"type":"number","minimum":0,"maximum":60000,"description":"Auto-hide delay in ms. 0=never. Default 20000."},
                    "turn_radius":{"type":"number","minimum":1,"maximum":1000,"description":"Minimum turning radius of the glide path in points; smaller = tighter curves. Default 80."}
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
        // Cursor key: caller-declared `session` > legacy `cursor_id` > NO_CURSOR.
        let cursor_id = resolve_cursor_key(&args);
        // 0. Resolve `cursor_icon` (built-in name or image path — same vocabulary
        // as the CLI flags) to a shape override and dispatch it below, so the
        // overlay actually changes instead of only recording the string.
        let mut shape_cmd: Option<cursor_overlay::OverlayCommand> = None;
        if let Some(icon) = args.get("cursor_icon").and_then(|v| v.as_str()) {
            let icon_owned = icon.to_owned();
            match tokio::task::spawn_blocking(move || {
                cursor_overlay::resolve_cursor_icon(&icon_owned)
            })
            .await
            {
                Ok(Ok(resolution)) => {
                    shape_cmd = Some(cursor_overlay::OverlayCommand::from_cursor_icon(resolution))
                }
                Ok(Err(e)) => return ToolResult::error(format!("Invalid cursor_icon: {e}")),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        }
        // 1. Per-instance appearance fields (Rust-only).
        self.state.cursor_registry.update_config(&cursor_id, |cfg| {
            if let Some(v) = args.get("cursor_icon").and_then(|v| v.as_str()) {
                cfg.cursor_icon = Some(v.to_owned());
            }
            if let Some(v) = args.get("cursor_color").and_then(|v| v.as_str()) {
                cfg.cursor_color = Some(v.to_owned());
            }
            if let Some(v) = args.get("cursor_label").and_then(|v| v.as_str()) {
                cfg.cursor_label = Some(v.to_owned());
            }
            if let Some(v) = num(args.get("cursor_size")) {
                cfg.cursor_size = Some(v);
            }
            if let Some(v) = num(args.get("cursor_opacity")) {
                cfg.cursor_opacity = Some(v.clamp(0.0, 1.0));
            }
        });
        if let Some(cmd) = shape_cmd {
            crate::overlay::send_command(cursor_id.clone(), cmd);
        }
        // 2. Apply motion knobs to the live render state — was silently
        // dropped before; this is the Swift parity behavior.
        let current = crate::overlay::current_motion(&cursor_id);
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
            num(args.get("turn_radius")),
        );
        crate::overlay::send_command(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetMotion(updated.clone()),
        );
        // Match Swift text format 1:1.
        let summary = format!(
            "cursor motion: startHandle={sh} endHandle={eh} arcSize={asz} arcFlow={af} \
             spring={sp} glideDurationMs={gd} dwellAfterClickMs={dw} idleHideMs={ih} turnRadius={tr}",
            sh = updated.start_handle, eh = updated.end_handle,
            asz = updated.arc_size,   af = updated.arc_flow,
            sp = updated.spring,
            gd = updated.glide_duration_ms as i64,
            dw = updated.dwell_after_click_ms as i64,
            ih = updated.idle_hide_ms as i64,
            tr = updated.turn_radius as i64,
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
            "turn_radius":          updated.turn_radius,
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
                milliseconds to match the setter's units. Pure read-only — no side effects."
                .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        // Report THIS session's cursor (caller-declared `session` > `cursor_id`
        // > "default"), mirroring macOS get_agent_cursor_state scoping.
        let cursor_key = resolve_cursor_key(&args);
        let enabled = crate::overlay::is_enabled(&cursor_key);
        let motion = crate::overlay::current_motion(&cursor_key);
        // Swift text format 1:1: single-line camelCase key=value pairs.
        let summary = format!(
            "cursor: enabled={enabled} startHandle={sh} endHandle={eh} arcSize={asz} \
             arcFlow={af} spring={sp} glideDurationMs={gd} dwellAfterClickMs={dw} idleHideMs={ih} turnRadius={tr}",
            sh = motion.start_handle, eh = motion.end_handle,
            asz = motion.arc_size,   af = motion.arc_flow,
            sp = motion.spring,
            gd = motion.glide_duration_ms as i64,
            dw = motion.dwell_after_click_ms as i64,
            ih = motion.idle_hide_ms as i64,
            tr = motion.turn_radius as i64,
        );
        // Rust-only structured payload: the same fields + the multi-cursor
        // instance map. Cursor instances are a Rust-only extension.
        let cursors =
            serde_json::to_value(self.state.cursor_registry.all_states()).unwrap_or_default();
        ToolResult::text(format!("✅ {summary}")).with_structured(json!({
            "enabled":              enabled,
            "start_handle":         motion.start_handle,
            "end_handle":           motion.end_handle,
            "arc_size":             motion.arc_size,
            "arc_flow":             motion.arc_flow,
            "spring":               motion.spring,
            "glide_duration_ms":    motion.glide_duration_ms,
            "dwell_after_click_ms": motion.dwell_after_click_ms,
            "idle_hide_ms":         motion.idle_hide_ms,
            "turn_radius":          motion.turn_radius,
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
                   icon instead of the default silhouette. Empty string reverts to the \
                   default cursor.\n\
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
                        "description": "Path to PNG/JPEG/SVG/ICO cursor image. '' = revert to the default cursor."
                    }
                },
                "additionalProperties": false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Cursor key: caller-declared `session` > legacy `cursor_id` > NO_CURSOR.
        let cursor_id = resolve_cursor_key(&args);

        // image_path
        let image_path = args.get("image_path").and_then(|v| v.as_str());
        let shape_cmd: Option<cursor_overlay::OverlayCommand> = if let Some(path) = image_path {
            if path.is_empty() {
                Some(cursor_overlay::OverlayCommand::SetShape(None))
            } else {
                let path_owned = path.to_owned();
                match tokio::task::spawn_blocking(move || {
                    cursor_overlay::CursorShape::load(&path_owned)
                })
                .await
                {
                    Ok(Ok(shape)) => {
                        let path_owned2 = path.to_owned();
                        self.state.cursor_registry.update_config(&cursor_id, |c| {
                            c.cursor_icon = Some(path_owned2);
                        });
                        Some(cursor_overlay::OverlayCommand::SetShape(Some(shape)))
                    }
                    Ok(Err(e)) => {
                        return ToolResult::error(format!("Failed to load image_path: {e}"))
                    }
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                }
            }
        } else {
            None
        };

        // gradient_colors
        let gradient_colors: Vec<[u8; 4]> =
            if let Some(arr) = args.get("gradient_colors").and_then(|v| v.as_array()) {
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
        let bloom_color: Option<Option<[u8; 4]>> =
            if let Some(hex) = args.get("bloom_color").and_then(|v| v.as_str()) {
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
            crate::overlay::send_command(cursor_id.clone(), cmd);
        }
        let gradient_provided = args.get("gradient_colors").is_some();
        let bloom_provided = args.get("bloom_color").is_some();
        if gradient_provided || bloom_provided {
            crate::overlay::send_command(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetGradient {
                    gradient_colors,
                    bloom_color: bloom_color.flatten(),
                },
            );
        }

        // Swift `SetAgentCursorStyleTool` text format: only include fields
        // whose post-write value is `Some` (i.e. not reverted to default).
        // Falls back to "✅ cursor style: reverted to default" when every
        // field is empty.
        let mut parts: Vec<String> = Vec::new();
        if let Some(arr) = args.get("gradient_colors").and_then(|v| v.as_array()) {
            let hexes: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            if !hexes.is_empty() {
                parts.push(format!("gradient_colors=[{}]", hexes.join(",")));
            }
        }
        if let Some(s) = args.get("bloom_color").and_then(|v| v.as_str()) {
            if !s.is_empty() {
                parts.push(format!("bloom_color={s}"));
            }
        }
        if let Some(s) = image_path {
            if !s.is_empty() {
                parts.push(format!("image_path={s}"));
            }
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
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
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
                if is_elevated {
                    "elevated"
                } else {
                    "non-elevated"
                }
            ),
            None => format!(
                "Process integrity level: {il_name} (token query failed)\n\
                 UIA accessibility: available (no special permission needed)\n\
                 PostMessage injection: available"
            ),
        };
        ToolResult::text(status_text).with_structured(json!({
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
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
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
            // mirrored on Windows yet (separate cursor-overlay config path).
            description: "Report the current persistent driver config.\n\n\
                Pure read-only. Returns defaults when the underlying state is unset — \
                same fallback the daemon uses at startup. Sibling to `set_config`.\n\n\
                Current schema (Windows):\n\n  \
                {\n    \"schema_version\": 1,\n    \"version\": \"<crate version>\",\n    \
                \"source_sha\": \"<maintainer build commit or null>\",\n    \"platform\": \"windows\",\n    \
                \"capture_mode\": \"ax\" | \"vision\" (DEPRECATED, ignored),\n    \
                \"max_image_dimension\": 0\n  }\n\n\
                `capture_mode` is deprecated and no longer affects behavior — \
                `get_window_state` always returns both the UIA tree and a screenshot.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        // Mirror the macOS agent's parity addition (commit adb9ecca):
        // nested `agent_cursor.enabled` block so Swift-shaped get_config
        // consumers can read the cursor's enabled state from one place.
        // Scope to the CALLING session's cursor (session > cursor_id > default)
        // and read it from the overlay deterministically — `all_states().first()`
        // was a nondeterministic HashMap read across sessions (macOS BUG 3).
        let cursor_enabled = crate::overlay::is_enabled(&resolve_cursor_key(&args));
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "source_sha":          option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
            "experimental_pip":    pip_enabled,
            "experimental_pip_geometry": pip_geometry,
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
                - **Swift-compatible** (preferred): `{\"key\": \"max_image_dimension\", \"value\": 1568}` \
                  — single dotted-path leaf write.\n\
                - **Legacy per-field** (Rust-only): `{\"max_image_dimension\": 0}` \
                  — bulk write of named fields.\n\n\
                Known keys:\n\
                - `capture_mode` (string: `vision` | `ax` | `som`) — DEPRECATED and ignored; \
                  `get_window_state` always returns both the UIA tree and a screenshot. Still \
                  accepted/persisted for back-compat but has no effect.\n\
                - `max_image_dimension` (integer)\n\
                - `experimental_pip` (boolean; persisted to config.json, applies on next daemon restart — Windows backend stubbed today, see issue #1729)\n\
                - `experimental_pip_geometry` (string `WxH` or `WxH+X+Y`; persisted; applies on next daemon restart)\n\n\
                Returns the full updated config in the same shape as `get_config`.".into(),
            input_schema: json!({"type":"object","properties":{
                "key":{"type":"string","description":"Dotted snake_case path to a leaf config field (Swift-compatible shape). Pair with `value`."},
                "value":{"description":"New value for `key`. JSON type depends on the key."},
                "capture_mode":{"type":"string","enum":["ax","vision"],"description":"DEPRECATED and ignored — get_window_state always returns both the UIA tree and a screenshot. Still accepted/persisted for back-compat but has no effect. (\"som\"/\"screenshot\" still decode as deprecated aliases.)"},
                "max_image_dimension":{"type":"integer","description":"Legacy per-field shape."},
                "experimental_pip":{"type":"boolean","description":"Legacy per-field shape. Enables PiP preview (applies next restart)."},
                "experimental_pip_geometry":{"type":"string","description":"Legacy per-field shape. PiP window size + optional position."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        if args.get("capture_scope").is_some()
            || args.get("key").and_then(Value::as_str) == Some("capture_scope")
        {
            return ToolResult::error(
                "config key 'capture_scope' is retired; pass capture_scope=auto|window|desktop to start_session",
            )
            .with_structured(json!({
                "code": "config_key_retired",
                "key": "capture_scope",
                "replacement": "start_session.capture_scope",
            }));
        }
        let mut cfg = self.state.config.write().unwrap();
        let mut applied = false;
        // Swift-compatible {key, value} shape.
        if let (Some(key), Some(val)) =
            (args.get("key").and_then(|v| v.as_str()), args.get("value"))
        {
            match key {
                "capture_mode" => match val.as_str() {
                    Some(s) => {
                        cfg.capture_mode = s.to_owned();
                        if let Err(e) = pip_preview::write_config_key("capture_mode", Value::String(s.to_owned())) {
                            tracing::warn!("set_config: failed to persist capture_mode: {e}");
                        }
                        applied = true;
                    }
                    None    => return ToolResult::error(format!("`capture_mode` must be a string, got {val}.")),
                },
                "max_image_dimension" => match val.as_u64() {
                    Some(n) => {
                        cfg.max_image_dimension = n as u32;
                        if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(n)) {
                            tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
                        }
                        applied = true;
                    }
                    None    => return ToolResult::error(format!("`max_image_dimension` must be an integer, got {val}.")),
                },
                "experimental_pip" => match val.as_bool() {
                    Some(b) => {
                        if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(b)) {
                            return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
                        }
                        applied = true;
                    }
                    None => return ToolResult::error(format!("`experimental_pip` must be a boolean, got {val}.")),
                },
                "experimental_pip_geometry" => match val.as_str() {
                    Some(s) => {
                        if pip_preview::PipGeometry::parse(s).is_none() {
                            return ToolResult::error(format!(
                                "experimental_pip_geometry `{s}` is not a valid WxH or WxH+X+Y string"
                            ));
                        }
                        if let Err(e) = pip_preview::write_config_key("experimental_pip_geometry", Value::String(s.to_owned())) {
                            return ToolResult::error(format!("failed to persist experimental_pip_geometry: {e}"));
                        }
                        applied = true;
                    }
                    None => return ToolResult::error(format!("`experimental_pip_geometry` must be a string, got {val}.")),
                },
                other => return ToolResult::error(format!(
                    "Unknown config key `{other}`. Known: capture_mode, max_image_dimension, experimental_pip, experimental_pip_geometry."
                )),
            }
        }
        // Legacy per-field shape.
        if let Some(mode) = args.get("capture_mode").and_then(|v| v.as_str()) {
            cfg.capture_mode = mode.to_owned();
            if let Err(e) =
                pip_preview::write_config_key("capture_mode", Value::String(mode.to_owned()))
            {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
            applied = true;
        }
        if let Some(dim) = args.get("max_image_dimension").and_then(|v| v.as_u64()) {
            cfg.max_image_dimension = dim as u32;
            if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(dim)) {
                tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
            }
            applied = true;
        }
        if let Some(enabled) = args.get("experimental_pip").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(enabled))
            {
                return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
            }
            applied = true;
        }
        if let Some(geom) = args
            .get("experimental_pip_geometry")
            .and_then(|v| v.as_str())
        {
            if pip_preview::PipGeometry::parse(geom).is_none() {
                return ToolResult::error(format!(
                    "experimental_pip_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) = pip_preview::write_config_key(
                "experimental_pip_geometry",
                Value::String(geom.to_owned()),
            ) {
                return ToolResult::error(format!(
                    "failed to persist experimental_pip_geometry: {e}"
                ));
            }
            applied = true;
        }
        if !applied {
            return ToolResult::error(
                "Missing required string field `key` (or a known legacy per-field).",
            );
        }
        // Emit the same pretty-JSON payload as `get_config` (matches Swift's
        // `set_config` return shape — both tools echo the full config after).
        let cursor_enabled = self
            .state
            .cursor_registry
            .all_states()
            .first()
            .map(|s| s.config.enabled)
            .unwrap_or(true);
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        let payload = json!({
            "schema_version":      1,
            "version":             env!("CARGO_PKG_VERSION"),
            "source_sha":          option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform":            "windows",
            "capture_mode":        cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_cursor":        { "enabled": cursor_enabled },
            "experimental_pip":    pip_enabled,
            "experimental_pip_geometry": pip_geometry,
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
                you can click by), use get_window_state instead — this is a fast discovery read."
                .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let (procs, windows) = tokio::task::spawn_blocking(|| {
            (
                crate::win32::list_processes(),
                crate::win32::list_windows(None),
            )
        })
        .await
        .unwrap_or_default();

        let mut lines = vec![format!(
            "{} running process(es), {} visible window(s)",
            procs.len(),
            windows.len()
        )];
        for p in &procs {
            lines.push(format!("- {} (pid {})", p.name, p.pid));
        }
        if !windows.is_empty() {
            lines.push(String::new());
            lines.push("Windows:".to_owned());
            for w in &windows {
                let title = if w.title.is_empty() {
                    "(no title)".to_owned()
                } else {
                    format!("\"{}\"", w.title)
                };
                lines.push(format!(
                    "- pid={} {} [window_id: {}] {}x{}+{}+{}",
                    w.pid, title, w.hwnd, w.width, w.height, w.x, w.y
                ));
            }
            lines.push(
                "→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned(),
            );
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
                "type":"object","required":["window_id","x1","y1","x2","y2"],"properties":{
                    "pid":{"type":"integer","description":"Target process ID. Validated in code (an explicit \"Missing required integer field pid\" error when absent); kept out of `required` to match the shared cross-platform zoom contract."},
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
            None => return ToolResult::error("Missing required integer field pid."),
        };
        let pid = Some(raw_pid as u32);
        let hwnd = match args.get("window_id").and_then(|v| v.as_u64()) {
            Some(v) => v,
            None => return ToolResult::error("Missing required integer field window_id."),
        };
        use cua_driver_core::tool_args::ArgsExt;
        let coerce = |k: &str| {
            args.opt_f64(k)
                .or_else(|| args.opt_i64(k).map(|i| i as f64))
        };
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

        // Issue #1880: the caller's coordinates are in the (possibly downscaled)
        // screenshot space of the last `get_window_state` call, but the crop below
        // runs on a fresh native-resolution capture. Scale by the stored resize
        // ratio so the crop lands on the intended region; the zoom context then
        // holds native-pixel values, which is what `from_zoom` clicks expect.
        let ratio = self
            .state
            .resize_registry
            .ratio(raw_pid as u32)
            .unwrap_or(1.0);
        let (nx1, ny1, nx2, ny2) = (x1 * ratio, y1 * ratio, x2 * ratio, y2 * ratio);

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            let png = crate::capture::screenshot_window_bytes(hwnd)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png, nx1, ny1, nx2, ny2, 500)
        })
        .await;

        match result {
            Ok(Ok(crop)) => {
                if let Some(p) = pid {
                    state.zoom_registry.set(
                        p,
                        ZoomContext {
                            origin_x: crop.origin_x,
                            origin_y: crop.origin_y,
                            scale_inv: crop.scale_inv,
                        },
                    );
                }
                use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                let b64 = B64.encode(&crop.jpeg_bytes);
                let (w, h) = (crop.out_w, crop.out_h);
                use cua_driver_core::protocol::Content;
                // Match Swift's text format 1:1.
                let summary = "✅ Zoomed region captured at native resolution. \
                    To click a target in this image, use \
                    `click(pid, x, y, from_zoom=true)` where x,y are pixel \
                    coordinates in THIS zoomed image — the driver maps them \
                    back automatically."
                    .to_owned();
                ToolResult {
                    content: vec![Content::image_jpeg(b64), Content::text(summary)],
                    is_error: None,
                    // Surface 7: `mime_type` mirrors the MCP image part's `mimeType`
                    // onto the structured payload (additive — `format` stays).
                    structured_content: Some(json!({
                        "width": w, "height": h, "format": "jpeg",
                        "mime_type": "image/jpeg"
                    })),
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Zoom failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── type_text_chars ───────────────────────────────────────────────────────────

pub struct TypeTextCharsTool {
    _state: Arc<ToolState>,
}
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
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Same trailing-protocol-tag scrub as the main TypeTextTool — see
        // cua_driver_core::text_sanitize for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let delay_ms = args.u64_or("delay_ms", 30);
        let hwnd_opt = args.opt_u64("window_id");
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        let text_len = text.chars().count();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::post_type_text_with_delay(hwnd, &text, delay_ms)
        })
        .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Typed {text_len} character(s) with {delay_ms}ms delay."
            )),
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

// ── bring_to_front ────────────────────────────────────────────────────────

pub struct BringToFrontTool;
static BTF_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        BTF_DEF.get_or_init(|| ToolDef {
            name: "bring_to_front".into(),
            description: "Activate `pid`'s window (or `window_id` if specified) -- bring it to \
                the OS foreground. \n\n\
                **This deliberately breaks the no-foreground contract.** Use only when an \
                agent is about to drive a sequence of operations against a target whose input \
                stack silently drops PostMessage (Chromium DOM content, GTK button widgets) \
                and the agent wants to pay the foreground cost once instead of per call. \n\n\
                Pairs with the `delivery_mode` field on input tools:\n\
                  1. Try `click(..., delivery_mode:\"background\")`. If it returns \
                     `background_unavailable`, the target needs foreground delivery.\n\
                  2. Call `bring_to_front(pid)` so the target is foreground.\n\
                  3. Subsequent input calls with `delivery_mode:\"foreground\"` deliver via \
                     SendInput WITHOUT visible flashing -- the SetForegroundWindow swap \
                     inside SendInput is a no-op since target is already foreground.\n\n\
                Implementation uses the `AttachThreadInput` trick to bypass Windows' \
                foreground-lock when the daemon is not at UIAccess integrity. Returns \
                structured `{previous_fg_hwnd, now_fg_hwnd}` so callers can later restore. \
                Windows only; macOS / Linux return an error pointing at platform-native \
                alternatives.".into(),
            input_schema: json!({"type":"object","required":["pid"],"properties":{
                "pid":{"type":"integer","description":"Target process ID."},
                "window_id":{"type":"integer","description":"Optional HWND. Defaults to pid's first visible top-level window."}
            },"additionalProperties":false}),
            read_only: false,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let hwnd_opt = args.opt_u64("window_id");

        // Resolve hwnd: explicit, or auto from pid.
        let hwnd = match hwnd_opt {
            Some(h) => h,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::win32::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.hwnd,
                    None => {
                        return ToolResult::error(format!(
                            "bring_to_front: no windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Run the foreground swap on a blocking thread. The AttachThreadInput
        // trick mirrors `send_key_synthesized` (input/keyboard.rs:313-345)
        // and is validated by `flash-repro/16-edge-launch-fg.ps1` for the
        // Edge launch focus-steal recovery case.
        let outcome =
            tokio::task::spawn_blocking(move || -> Result<(u64, u64, bool, bool), String> {
                use windows::Win32::Foundation::HWND;
                use windows::Win32::Graphics::Dwm::DwmFlush;
                use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
                    SetForegroundWindow, SetWindowPos, ShowWindowAsync, HWND_NOTOPMOST,
                    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_RESTORE,
                };

                let target = HWND(hwnd as *mut _);
                if !unsafe { IsWindow(target) }.as_bool() {
                    return Err(format!("hwnd 0x{hwnd:x} is not a valid window"));
                }

                let prev_fg = unsafe { GetForegroundWindow() };
                let prev_fg_addr = prev_fg.0 as u64;

                // Iconic windows have no rendered pixels and live at the sentinel
                // (-32000, -32000) position. Restore before changing z-order so
                // bring_to_front is also the advertised recovery path for capture.
                let was_minimized = unsafe { IsIconic(target) }.as_bool();
                if was_minimized {
                    let _ = unsafe { ShowWindowAsync(target, SW_RESTORE) };
                    for _ in 0..20 {
                        if !unsafe { IsIconic(target) }.as_bool() {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(25));
                    }
                    if unsafe { IsIconic(target) }.as_bool() {
                        return Err(format!(
                            "restore request did not complete for minimized hwnd 0x{hwnd:x}"
                        ));
                    }
                }

                // Lock-free z-order raise FIRST: bring the window to the top of the
                // normal band (the HWND_TOPMOST→HWND_NOTOPMOST force-to-front trick)
                // so it's brought to the VISIBLE front even when the foreground-lock
                // denies focus. SWP_NOACTIVATE → no focus steal; SetWindowPos z-order
                // is not gated by the foreground-lock / UIAccess. This is the same
                // technique the delivery_mode:"foreground" pointer path uses.
                let raised = unsafe {
                    let a = SetWindowPos(
                        target,
                        HWND_TOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    )
                    .is_ok();
                    let b = SetWindowPos(
                        target,
                        HWND_NOTOPMOST,
                        0,
                        0,
                        0,
                        0,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
                    )
                    .is_ok();
                    a && b
                };

                // Then best-effort focus/activation (needed for keyboard) via the
                // AttachThreadInput trick — inherits the current FG thread's FG-lock
                // token so SetForegroundWindow can succeed at Medium IL. Still denied
                // on a maxed lock without UIAccess; the z-raise already made the
                // window visible regardless.
                let my_tid = unsafe { GetCurrentThreadId() };
                let mut fg_pid = 0u32;
                let fg_tid = unsafe { GetWindowThreadProcessId(prev_fg, Some(&mut fg_pid)) };
                let attached = fg_tid != 0 && fg_tid != my_tid;
                if attached {
                    let _ = unsafe { AttachThreadInput(my_tid, fg_tid, true) };
                }
                let _ = unsafe { SetForegroundWindow(target) };
                if attached {
                    let _ = unsafe { AttachThreadInput(my_tid, fg_tid, false) };
                }
                let now_fg = unsafe { GetForegroundWindow() };

                // A restored HWND can stop reporting iconic before its compositor
                // surface is painted. Flush DWM before returning, but do not make
                // the restore operation depend on any particular capture backend.
                // Capture has its own WGC fallback for freshly restored surfaces.
                if was_minimized {
                    let _ = unsafe { DwmFlush() };
                }
                Ok((prev_fg_addr, now_fg.0 as u64, raised, was_minimized))
            })
            .await;

        match outcome {
            Ok(Ok((prev, now, raised, restored))) => {
                let focused = now == hwnd;
                let msg = if focused {
                    format!("✅ bring_to_front: pid {pid} hwnd 0x{hwnd:x} is now foreground (was 0x{prev:x}).")
                } else if raised {
                    format!(
                        "✅ bring_to_front: pid {pid} hwnd 0x{hwnd:x} raised to the visible front \
                         (z-order, no focus steal), but the foreground-lock denied keyboard focus — \
                         focus is still 0x{now:x}. A delivery_mode:\"foreground\" pointer click will \
                         land; for keyboard focus run the cua-driver-uia worker."
                    )
                } else {
                    format!(
                        "bring_to_front could neither focus nor raise target 0x{hwnd:x} (current \
                         foreground 0x{now:x})."
                    )
                };
                let r = if focused || raised {
                    ToolResult::text(msg)
                } else {
                    ToolResult::error(msg)
                };
                r.with_structured(json!({
                    "previous_fg_hwnd": format!("0x{prev:x}"),
                    "now_fg_hwnd":      format!("0x{now:x}"),
                    "target_hwnd":      format!("0x{hwnd:x}"),
                    "landed_on_target": focused,
                    "raised":           raised,
                    "restored":         restored,
                }))
            }
            Ok(Err(e)) => ToolResult::error(format!("bring_to_front: {e}")),
            Err(join) => {
                ToolResult::error(format!("bring_to_front: blocking-task join error: {join}"))
            }
        }
    }
}

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
            Some(_) => {
                return ToolResult::error("kill_app: `pid` must be a positive integer".to_string())
            }
            None => {
                return ToolResult::error(
                    "kill_app: missing required integer field `pid`".to_string(),
                )
            }
        };

        // Run the syscalls on a blocking thread — TerminateProcess + the
        // WaitForSingleObject confirmation are both blocking, and we don't
        // want to stall the tokio reactor.
        let outcome = tokio::task::spawn_blocking(move || -> Result<(), String> {
            use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
            use windows::Win32::System::Threading::{
                OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
                PROCESS_TERMINATE,
            };

            // SAFETY: OpenProcess returns a Result<HANDLE> in windows-rs;
            // PROCESS_TERMINATE | PROCESS_SYNCHRONIZE lets us both kill the
            // target AND wait for the OS to fully reap it (so the caller's
            // follow-up list_apps reflects the change immediately).
            let h = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, false, pid_v) }
                .map_err(|e| {
                format!(
                    "OpenProcess(pid={pid_v}, PROCESS_TERMINATE|PROCESS_SYNCHRONIZE) failed: {e}. \
                     The process may not exist, or the daemon lacks the right to terminate it. \
                     Try listing processes first to confirm the pid is correct."
                )
            })?;

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
        })
        .await;

        match outcome {
            Ok(Ok(())) => ToolResult::text(format!("✅ Terminated pid {pid_v}.")),
            Ok(Err(e)) => ToolResult::error(format!("kill_app: {e}")),
            Err(join_err) => {
                ToolResult::error(format!("kill_app: blocking-task join error: {join_err}"))
            }
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
                XAML / UWP / WinUI3 targets; see CUA-543."
                .into(),
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
            _ => {
                return ToolResult::error("debug_window_info: missing or invalid `pid`".to_string())
            }
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
    {
        let cursor_registry = state.cursor_registry.clone();
        let _ = cua_driver_core::session::set_cursor_outcome_reader(std::sync::Arc::new(
            move |session_id| {
                let state = cursor_registry.get(session_id);
                let motion_customized = state.is_some()
                    && crate::overlay::current_motion(session_id)
                        != cursor_overlay::MotionConfig::default();
                let active_cursor_count = cursor_registry
                    .all_states()
                    .iter()
                    .filter(|state| state.config.cursor_id != "default")
                    .count()
                    .max(1);
                match state {
                    Some(state) => cua_driver_core::session::bounded_cursor_outcome(
                        true,
                        state.config.enabled,
                        state.config.cursor_icon.as_deref(),
                        state.config.cursor_color.as_deref(),
                        state.config.cursor_label.as_deref(),
                        motion_customized,
                        active_cursor_count,
                    ),
                    None => cua_driver_core::session::bounded_cursor_outcome(
                        false,
                        false,
                        None,
                        None,
                        None,
                        false,
                        active_cursor_count,
                    ),
                }
            },
        ));
    }
    // Share the element cache with the recording-hook layer so it can
    // resolve element_index → window-local screenshot coords for click.png.
    crate::recording_hooks::set_element_cache(state.element_cache.clone());

    // Drop a session's owned cursor on `session_end` (explicit end_session, the
    // CLI `session end` verb, or the daemon idle-TTL sweep). The session id IS
    // the cursor key (caller-declared `session`), so this prunes the metadata
    // registry AND stops the overlay painting that session's cursor. Both paths
    // guard "default" so the anonymous / one-shot cursor survives. Registering
    // once per process (build_registry runs once in the daemon) is guarded so a
    // repeated build in tests can't accumulate duplicate hooks. Mirrors the
    // macOS `register_all` session_end hook (platform-macos/src/tools/mod.rs).
    {
        static HOOK_ONCE: std::sync::OnceLock<()> = std::sync::OnceLock::new();
        if HOOK_ONCE.set(()).is_ok() {
            let cursor_registry = state.cursor_registry.clone();
            cua_driver_core::session::register_session_end_hook(move |session_id| {
                cursor_registry.remove(session_id);
                crate::overlay::remove_cursor(session_id.to_owned());
            });
        }
    }

    let mut r = ToolRegistry::new();
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool {
        state: state.clone(),
    }));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    r.register(Box::new(BringToFrontTool));
    r.register(Box::new(DebugWindowInfoTool));
    r.register(Box::new(ClickTool {
        state: state.clone(),
    }));
    r.register(Box::new(DoubleClickTool {
        state: state.clone(),
    }));
    r.register(Box::new(RightClickTool {
        state: state.clone(),
    }));
    r.register(Box::new(DragTool {
        state: state.clone(),
    }));
    r.register(Box::new(TypeTextTool {
        state: state.clone(),
    }));
    r.register(Box::new(PressKeyTool {
        state: state.clone(),
    }));
    r.register(Box::new(HotkeyTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetValueTool {
        state: state.clone(),
    }));
    r.register(Box::new(ScrollTool {
        state: state.clone(),
    }));
    // `screenshot` / `ScreenshotCompatTool` removed from the tool surface
    // — `get_window_state` (which now always returns a screenshot) is the
    // single canonical path for getting a window screenshot. Reasons:
    //   1. One entry point means callers always have a window_id context,
    //      which `screenshot` made ambiguous (whole-display fallback
    //      always picked the wrong thing for headless agent workflows).
    //   2. `get_window_state` always returns a PNG alongside the tree
    //      — there's no information `screenshot` exposed that's not in
    //      `get_window_state`.
    //   3. Claude Code's vision pipeline can be retargeted at
    //      `get_window_state` via its tool-anchor config — no need for
    //      a dedicated "screenshot" name.
    // The `ScreenshotTool` and `ScreenshotCompatTool` structs are kept in
    // this file for now as private impl helpers — the `capture::*`
    // functions they wrap are still the actual screenshot machinery.
    // Delete the structs in a follow-up after confirming no callers
    // depend on them via `Tool` trait reflection.
    let _ = compat; // formerly drove the ScreenshotCompatTool branch
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetDesktopStateTool));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorEnabledTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorMotionTool {
        state: state.clone(),
    }));
    r.register(Box::new(GetAgentCursorStateTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorStyleTool {
        state: state.clone(),
    }));
    r.register(Box::new(CheckPermissionsTool));
    // `health_report` — single-call cross-platform driver diagnostics.
    // Stable schema_version="1" contract for downstream consumers. Windows skips
    // tcc_* and bundle_identity with "not applicable on Windows".
    r.register(Box::new(
        cua_driver_core::health_report::HealthReportTool::new(std::sync::Arc::new(
            crate::health_report::WindowsHealthProvider,
        )),
    ));
    r.register(Box::new(GetConfigTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetConfigTool {
        state: state.clone(),
    }));
    r.register(Box::new(GetAccessibilityTreeTool));
    r.register(Box::new(ZoomTool {
        state: state.clone(),
    }));
    // `type_text_chars` is intentionally NOT registered — Swift treats it
    // as a deprecated alias for `type_text` resolved at invoke time in
    // mcp-server's `ToolRegistry::invoke`. Keeping it out of the registry
    // means it doesn't show up in `tools/list` either, matching Swift's
    // ToolRegistry.swift (`type_text_chars` not in `handlers`).
    let _: &TypeTextCharsTool = &TypeTextCharsTool {
        _state: state.clone(),
    }; // touch struct so it stays in this crate for now
       // Cross-platform `page` tool definition lives in mcp-server; Windows plugs
       // in its UIA TextPattern + FindAll backend (CDP for execute_javascript).
    r.register(Box::new(cua_driver_core::page::PageTool::new(
        std::sync::Arc::new(super::page::WindowsPageBackend::new()),
    )));
    let browser_engine = cua_driver_core::browser::BrowserEngine::new(std::sync::Arc::new(
        crate::browser_platform::WindowsBrowserPlatform,
    ));
    cua_driver_core::browser::register_browser_tools(&browser_engine, &mut r);
    r.register_workspace_tools(std::sync::Arc::new(
        crate::workspace::WindowsWorkspaceBackend::new(),
    ));
    r.register_recording_tools();
    r.register_session_tools();
    r
}

#[cfg(test)]
mod cursor_key_resolution_tests {
    use super::{resolve_cursor_key, NO_CURSOR};
    use serde_json::json;

    #[test]
    fn anonymous_resolves_to_no_cursor() {
        // No session/cursor_id → NO_CURSOR (""): the action still runs but no
        // cursor is shown. The connection-injected `_session_id` is NOT a cursor
        // source — it stays the recording/config lifecycle key.
        assert_eq!(resolve_cursor_key(&json!({})), NO_CURSOR);
        assert_eq!(resolve_cursor_key(&json!({ "pid": 1 })), NO_CURSOR);
        assert_eq!(
            resolve_cursor_key(&json!({ "_session_id": "mcp-1-2" })),
            NO_CURSOR
        );
    }

    #[test]
    fn explicit_session_owns_a_cursor() {
        assert_eq!(
            resolve_cursor_key(&json!({ "session": "research-run" })),
            "research-run"
        );
    }

    #[test]
    fn cursor_id_is_a_legacy_alias_and_session_wins() {
        assert_eq!(
            resolve_cursor_key(&json!({ "cursor_id": "user-handle" })),
            "user-handle"
        );
        assert_eq!(
            resolve_cursor_key(&json!({ "session": "s1", "cursor_id": "c1" })),
            "s1"
        );
    }

    #[test]
    fn empty_strings_fall_through_to_no_cursor() {
        // An empty `session` falls through to `cursor_id`; both empty → NO_CURSOR.
        assert_eq!(
            resolve_cursor_key(&json!({ "session": "", "cursor_id": "c1" })),
            "c1"
        );
        assert_eq!(
            resolve_cursor_key(&json!({ "session": "", "cursor_id": "" })),
            NO_CURSOR
        );
    }

    #[test]
    fn two_parallel_sessions_resolve_distinct_keys() {
        // The regression this whole port fixes: two concurrent runs each declare
        // their own `session`, so they resolve DISTINCT cursor keys and own
        // separate overlay cursors instead of clobbering one shared cursor.
        let a =
            resolve_cursor_key(&json!({ "pid": 10, "element_index": 1, "session": "calc-2plus1" }));
        let b =
            resolve_cursor_key(&json!({ "pid": 20, "element_index": 1, "session": "calc-5plus6" }));
        assert_eq!(a, "calc-2plus1");
        assert_eq!(b, "calc-5plus6");
        assert_ne!(a, b);
    }
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
        let shape = LaunchTargetShape {
            has_urls: true,
            ..empty()
        };
        assert!(!should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn name_only_restores() {
        let shape = LaunchTargetShape {
            has_name: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn path_only_restores() {
        let shape = LaunchTargetShape {
            has_path: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn aumid_only_restores() {
        // Note: AUMID path has its own synchronous restore in launch_uwp.rs,
        // but the decision function still says "yes, this is an
        // app-identifying launch" — the caller (`LaunchAppTool::invoke`)
        // gates the polling spawn on `aumid_for_uwp.is_none()` so we don't
        // double-restore.
        let shape = LaunchTargetShape {
            has_aumid: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn bundle_id_only_restores() {
        let shape = LaunchTargetShape {
            has_bundle_id: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn launch_path_only_restores() {
        let shape = LaunchTargetShape {
            has_launch_path: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn name_with_urls_restores() {
        // App-identifying field present alongside urls — the user wants
        // the named app to open these URLs in the background, NOT for the
        // urls to take over the default browser's foreground. Restore.
        let shape = LaunchTargetShape {
            has_name: true,
            has_urls: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn path_with_urls_restores() {
        let shape = LaunchTargetShape {
            has_path: true,
            has_urls: true,
            ..empty()
        };
        assert!(should_restore_foreground_after_launch(shape));
    }

    #[test]
    fn aumid_with_urls_restores() {
        let shape = LaunchTargetShape {
            has_aumid: true,
            has_urls: true,
            ..empty()
        };
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
            "msedge", "chrome", "brave", "opera", "vivaldi", "chromium", "thorium", "iridium",
            "browser", "arc",
        ] {
            assert!(is_chromium_browser_target(name), "{name} should match");
            assert!(
                is_chromium_browser_target(&format!("{name}.exe")),
                "{name}.exe should match"
            );
            // Case-insensitive.
            assert!(
                is_chromium_browser_target(&name.to_uppercase()),
                "uppercase {name} should match"
            );
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
            assert!(
                !is_chromium_browser_target(&format!("{name}.exe")),
                "{name}.exe should NOT match"
            );
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
        let dfe: Vec<_> = args
            .iter()
            .filter(|a| a.starts_with("--disable-features="))
            .collect();
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
        assert!(args
            .iter()
            .any(|a| a == "--disable-features=CalculateNativeWinOcclusion"));
        assert!(args
            .iter()
            .any(|a| a == "--disable-backgrounding-occluded-windows"));
        assert!(args.iter().any(|a| a == "--disable-renderer-backgrounding"));
    }
}

#[cfg(test)]
mod click_button_schema_tests {
    use super::ClickTool;
    use cua_driver_core::tool::Tool;

    /// Surface 5: schema must keep advertising the three canonical button
    /// values. Windows was already shipping `button` (pre-Surface-5) so this
    /// test is the freeze test — guards against an inadvertent rename/removal.
    #[test]
    fn schema_advertises_button_enum() {
        let tool = ClickTool {
            state: super::ToolState::new(),
        };
        let d = tool.def();
        let props = d.input_schema.get("properties").expect("properties");
        let button = props.get("button").expect("button field present");
        assert_eq!(button.get("type").and_then(|v| v.as_str()), Some("string"));
        let enum_vals: Vec<&str> = button
            .get("enum")
            .and_then(|v| v.as_array())
            .expect("button.enum present")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        for need in ["left", "right", "middle"] {
            assert!(enum_vals.contains(&need), "missing {need} in button.enum");
        }
    }
}

#[cfg(test)]
mod desktop_scope_tests {
    use super::{is_windowless_desktop_action, GetDesktopStateTool};
    use cua_driver_core::tool::Tool;
    use serde_json::json;

    // ── is_windowless_desktop_action ──────────────────────────────────────────

    #[test]
    fn windowless_true_for_xy_under_desktop_scope_click_shape() {
        // Click arg shape: {x, y}.
        assert!(is_windowless_desktop_action(
            &json!({"x": 10, "y": 20, "scope": "desktop"})
        ));
    }

    #[test]
    fn windowless_true_for_xy_under_desktop_scope_scroll_shape() {
        // Scroll arg shape: {direction, x, y}.
        assert!(is_windowless_desktop_action(&json!({
            "direction": "down", "x": 10, "y": 20, "scope": "desktop"
        })));
    }

    #[test]
    fn windowless_false_when_pid_present() {
        assert!(!is_windowless_desktop_action(&json!({
            "x": 10, "y": 20, "pid": 5, "scope": "desktop"
        })));
    }

    #[test]
    fn windowless_false_when_window_id_present() {
        assert!(!is_windowless_desktop_action(&json!({
            "x": 10, "y": 20, "window_id": 99, "scope": "desktop"
        })));
    }

    #[test]
    fn windowless_false_under_window_scope() {
        assert!(!is_windowless_desktop_action(&json!({
            "x": 10, "y": 20, "scope": "window"
        })));
    }

    #[test]
    fn windowless_false_when_xy_missing() {
        assert!(!is_windowless_desktop_action(
            &json!({"x": 10, "scope": "desktop"})
        ));
        assert!(!is_windowless_desktop_action(
            &json!({"y": 20, "scope": "desktop"})
        ));
        assert!(!is_windowless_desktop_action(&json!({"scope": "desktop"})));
        // Non-numeric x/y must not qualify.
        assert!(!is_windowless_desktop_action(&json!({
            "x": "10", "y": "20", "scope": "desktop"
        })));
    }

    // ── get_desktop_state schema ──────────────────────────────────────────────

    #[test]
    fn get_desktop_state_schema_shape() {
        let d = GetDesktopStateTool.def();
        assert!(d.read_only, "get_desktop_state must be read_only");
        let props = d.input_schema["properties"].as_object().unwrap();
        assert!(!props.contains_key("pid"), "must not accept pid");
        assert!(
            !props.contains_key("window_id"),
            "must not accept window_id"
        );
        assert!(props.contains_key("session"));
        assert!(props.contains_key("screenshot_out_file"));
        assert_eq!(d.input_schema["additionalProperties"], json!(false));
    }
}

#[cfg(test)]
mod browser_launch_guard_tests {
    use super::contains_remote_debugging_flag;

    #[test]
    fn rejects_all_chromium_remote_debugging_spellings() {
        assert!(contains_remote_debugging_flag("--remote-debugging-port=0"));
        assert!(contains_remote_debugging_flag("--REMOTE-DEBUGGING-PIPE"));
        assert!(contains_remote_debugging_flag(
            r#"C:\Program Files\Chrome\chrome.exe --remote-debugging-port 9222"#
        ));
        assert!(!contains_remote_debugging_flag(
            r#"--user-data-dir=C:\Temp\profile"#
        ));
    }
}
