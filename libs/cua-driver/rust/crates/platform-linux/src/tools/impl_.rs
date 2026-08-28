//! Real Linux tool implementations (compiled only on Linux).

use async_trait::async_trait;
use cua_driver_contract::{
    ClickButton, ClickInput, DragInput, GetCursorPositionInput, GetDesktopStateInput,
    GetScreenSizeInput, HotkeyInput, InvokeMenuInput, MoveCursorInput, PressKeyInput, ScrollInput,
    TypeTextInput,
};
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
    tool_args::{parse_typed_input, parse_typed_projection, ArgsExt},
    window_target::{PidOnlyWindowTargetGuard, WindowTargetCandidate, WindowTargetCandidates},
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::atspi::ElementCache;
use cursor_overlay::CursorRegistry;

fn window_target_candidates_for_pid(
    windows: impl IntoIterator<Item = crate::x11::WindowInfo>,
    pid: u32,
) -> Vec<WindowTargetCandidate> {
    windows
        .into_iter()
        .filter(|window| window.pid == Some(pid))
        .map(|window| WindowTargetCandidate {
            window_id: window.xid,
            title: window.title,
            app_name: Some(window.app_name),
            is_on_screen: window.is_on_screen,
        })
        .collect()
}

fn pid_window_target_candidates(pid: i64) -> Vec<WindowTargetCandidate> {
    let Ok(pid) = u32::try_from(pid) else {
        return Vec::new();
    };
    window_target_candidates_for_pid(crate::wayland::list_windows_dispatch(Some(pid)), pid)
}

fn pid_window_guarded<T: Tool + 'static>(
    tool: T,
    candidates: &WindowTargetCandidates,
) -> Box<dyn Tool> {
    Box::new(PidOnlyWindowTargetGuard::new(
        Box::new(tool),
        candidates.clone(),
    ))
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
/// prior `set_config capture_mode=vision` survives across stateless one-shot
/// invocations — matching the macOS daemon's startup load. See #2008.
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
    pub mouse_hold: std::sync::Mutex<std::collections::HashMap<String, MouseHoldState>>,
    pub config: Arc<RwLock<DriverConfig>>,
}

#[derive(Clone, Debug)]
pub struct MouseHoldState {
    pub pid: u32,
    pub xid: u64,
    pub button: u8,
    pub x: f64,
    pub y: f64,
}

impl ToolState {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            element_cache: Arc::new(ElementCache::new()),
            cursor_registry: Arc::new(CursorRegistry::new()),
            resize_registry: Arc::new(ResizeRegistry::new()),
            zoom_registry: Arc::new(ZoomRegistry::new()),
            mouse_hold: std::sync::Mutex::new(Default::default()),
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
                "List Linux apps — both currently running and installed-but-not-running — \
                with per-app state flags:\n\n\
                - running: is a process for this app live? (pid is 0 when false)\n\
                - active: reserved (Linux X11/Wayland focus model differs from frontmost-app); \
                always false.\n\
                - kind: `\"desktop\"` for XDG `.desktop` launcher entries.\n\
                - launch_path: the launcher command from `Exec=` (field codes stripped). \
                Pass to `launch_app(launch_path=...)`.\n\
                - bundle_id: the XDG \"desktop file id\" — the `.desktop` file's path \
                relative to its XDG `applications/` root with the `.desktop` suffix \
                stripped and path separators replaced with `-` \
                (e.g. `kde4/konqbrowser.desktop` → `kde4-konqbrowser`).\n\
                - last_used: RFC3339 mtime of the `.desktop` file, when readable.\n\n\
                Running apps come from `/proc`. Installed apps come from XDG Desktop Entry \
                files in $XDG_DATA_HOME/applications and each $XDG_DATA_DIRS entry's \
                applications/ subdir. Entries with `NoDisplay=true` or `Hidden=true` are \
                filtered. A `.desktop` file whose launcher matches a running process \
                (by basename) is merged into a single entry with `running: true`.\n\n\
                Use this for \"is X installed?\" as well as \"is X running?\". For per-window \
                state — visibility, geometry, titles — call list_windows instead."
                    .into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }

    async fn invoke(&self, _args: Value) -> ToolResult {
        let apps = tokio::task::spawn_blocking(|| -> Vec<serde_json::Value> {
            let procs = crate::proc_fs::list_processes();
            let installed = crate::installed_apps::list_installed_apps();

            // Match running processes to installed apps by executable
            // basename (Exec=firefox %u → "firefox"; cmdline /usr/bin/firefox
            // → "firefox"). Many distros register multiple .desktop entries
            // sharing the same basename (e.g. several `firefox` profiles, a
            // `code` and a `code-insiders` both shelling `code`), so the
            // bucket is `Vec<usize>` — we pick a winner per running process
            // via `disambiguate_installed_match` instead of overwriting.
            let mut by_exe: std::collections::HashMap<String, Vec<usize>> =
                std::collections::HashMap::new();
            for (i, app) in installed.iter().enumerate() {
                let basename = exec_basename(&app.launch_path);
                if !basename.is_empty() {
                    by_exe.entry(basename).or_default().push(i);
                }
            }

            let mut consumed: std::collections::HashSet<usize> = std::collections::HashSet::new();
            let mut out = Vec::new();
            for p in &procs {
                let key_source = if !p.cmdline.is_empty() {
                    &p.cmdline
                } else {
                    &p.name
                };
                let basename = exec_basename(key_source);
                if basename.is_empty() {
                    continue;
                }
                let candidates = by_exe.get(&basename).map(|v| v.as_slice()).unwrap_or(&[]);
                let merged = disambiguate_installed_match(candidates, &installed, key_source);
                if let Some(idx) = merged {
                    consumed.insert(idx);
                }
                let (name, bundle_id, launch_path, kind, last_used) = match merged {
                    Some(idx) => {
                        let a = &installed[idx];
                        (
                            a.name.clone(),
                            Some(a.bundle_id.clone()),
                            Some(a.launch_path.clone()),
                            Some("desktop".to_owned()),
                            a.last_used.clone(),
                        )
                    }
                    None => (
                        if !p.name.is_empty() {
                            p.name.clone()
                        } else {
                            basename.clone()
                        },
                        None,
                        None,
                        None,
                        None,
                    ),
                };
                out.push(json!({
                    "pid":         p.pid,
                    "bundle_id":   bundle_id,
                    "name":        name,
                    "running":     true,
                    "active":      false,
                    "kind":        kind,
                    "launch_path": launch_path,
                    "last_used":   last_used,
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            for (i, app) in installed.iter().enumerate() {
                if consumed.contains(&i) {
                    continue;
                }
                out.push(json!({
                    "pid":         0,
                    "bundle_id":   app.bundle_id.clone(),
                    "name":        app.name.clone(),
                    "running":     false,
                    "active":      false,
                    "kind":        "desktop",
                    "launch_path": app.launch_path.clone(),
                    "last_used":   app.last_used.clone(),
                    "windows":     Vec::<serde_json::Value>::new(),
                }));
            }
            out
        })
        .await
        .unwrap_or_default();

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
        // Unified `apps` array + legacy `processes` alias for older callers.
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

/// Pick which of several `.desktop`-derived installed apps best matches a
/// running process whose basename collided. Precedence:
///   1. exact full-launch-path equality with the process's cmdline token
///      (so `/usr/bin/firefox` beats `/snap/firefox/current/firefox`)
///   2. most recently modified launcher (`last_used` desc, RFC3339 string
///      ordering is lexicographic-safe for our format)
///   3. first candidate (deterministic by source order)
fn disambiguate_installed_match(
    candidates: &[usize],
    installed: &[crate::installed_apps::InstalledApp],
    proc_key_source: &str,
) -> Option<usize> {
    if candidates.is_empty() {
        return None;
    }
    if candidates.len() == 1 {
        return Some(candidates[0]);
    }

    // Look for an exact path match against the cmdline's first token.
    let proc_path = proc_key_source.split_whitespace().next().unwrap_or("");
    if !proc_path.is_empty() {
        if let Some(&idx) = candidates
            .iter()
            .find(|&&i| installed[i].launch_path == proc_path)
        {
            return Some(idx);
        }
    }

    // Fall back to the candidate with the most recent `last_used`.
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

/// Return the lowercase basename of an executable token, stripping any
/// leading shell-wrapper words and quoting. `env FOO=1 /usr/bin/firefox %U`
/// → `firefox`. `firefox %u` → `firefox`.
fn exec_basename(s: &str) -> String {
    // Take the first whitespace-separated token that looks like a binary,
    // skipping `env`-style prefixes and `K=V` assignments.
    for tok in s.split_whitespace() {
        if tok == "env" {
            continue;
        }
        if tok.contains('=') && !tok.starts_with('/') && !tok.starts_with('-') {
            // `FOO=bar` env-var assignment — skip.
            continue;
        }
        let cleaned = tok.trim_matches(|c| c == '"' || c == '\'');
        let basename = std::path::Path::new(cleaned)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(cleaned);
        return basename.to_ascii_lowercase();
    }
    String::new()
}

// ── list_windows ─────────────────────────────────────────────────────────────

pub struct ListWindowsTool;
static LIST_WINDOWS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for ListWindowsTool {
    fn def(&self) -> &ToolDef {
        LIST_WINDOWS_DEF.get_or_init(|| ToolDef {
            name: "list_windows".into(),
            description: "List top-level windows. Each record includes z_index (integer or null; \
                higher values are closer to the front; null means stacking order is unavailable \
                and callers must not infer one). To select a frontmost candidate, take the maximum \
                integer z_index; if every value is null, use an explicit fallback instead of \
                relying on array order.".into(),
            input_schema: json!({"type":"object","properties":{
                "pid":{"type":"integer"},
                "on_screen_only":{"type":"boolean","description":"When true, filter to visible windows only. Default false."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let filter_pid = args.opt_u64("pid").map(|v| v as u32);
        let on_screen_only = args.bool_or("on_screen_only", false);
        let mut windows =
            tokio::task::spawn_blocking(move || crate::wayland::list_windows_dispatch(filter_pid))
                .await
                .unwrap_or_default();
        // Exited applications can remain visible in AT-SPI/X11 as zombies;
        // never return those stale targets to callers.
        windows.retain(|window| window.pid.map_or(true, crate::proc_fs::is_process_live));
        if on_screen_only {
            windows.retain(|window| window.is_on_screen);
        }
        if crate::wayland::is_wayland() {
            crate::wayland::remember_observed_window_origins(&windows);
        }
        let mut lines = vec![format!("Found {} windows:", windows.len())];
        for w in &windows {
            lines.push(format!(
                "  [xid={}] pid={:?} \"{}\" {}x{}+{}+{}",
                w.xid, w.pid, w.title, w.width, w.height, w.x, w.y
            ));
        }
        let structured =
            json!({ "windows": windows.iter().map(window_record_json).collect::<Vec<_>>() });
        ToolResult::text(lines.join("\n")).with_structured(structured)
    }
}

/// Build the structured `list_windows` record for one Linux window.
///
/// Emits the canonical cross-platform shape — geometry nested under a
/// `bounds: {x,y,width,height}` object plus `app_name` / `is_on_screen`,
/// matching the macOS and Windows backends (#2017) — while KEEPING the
/// historical flat `x/y/width/height` fields inline as a legacy alias so
/// existing Linux callers don't break. Fully additive; no field removed,
/// no schema_version bump.
///
fn window_record_json(w: &crate::x11::WindowInfo) -> Value {
    json!({
        "window_id": w.xid,
        "pid": w.pid,
        "app_name": w.app_name,
        "title": w.title,
        // Canonical cross-platform geometry (macOS/Windows parity).
        "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
        "is_on_screen": w.is_on_screen,
        "z_index": w.z_index,
        // Legacy alias: flat fields kept inline for pre-existing callers.
        "x": w.x, "y": w.y,
        "width": w.width, "height": w.height,
    })
}

#[cfg(test)]
mod list_windows_tests {
    use super::*;

    #[test]
    fn record_has_bounds_and_flat_legacy_fields() {
        let w = crate::x11::WindowInfo {
            xid: 42,
            pid: Some(1234),
            app_name: "example-app".to_owned(),
            title: "Example".to_owned(),
            is_on_screen: true,
            z_index: Some(3),
            x: 10,
            y: 20,
            width: 300,
            height: 400,
        };
        let rec = window_record_json(&w);

        // Canonical cross-platform shape: nested `bounds` object.
        let bounds = rec
            .get("bounds")
            .expect("record must carry a `bounds` object");
        assert_eq!(bounds["x"], json!(10));
        assert_eq!(bounds["y"], json!(20));
        assert_eq!(bounds["width"], json!(300));
        assert_eq!(bounds["height"], json!(400));

        // Legacy alias: flat fields must still be present.
        assert_eq!(rec["x"], json!(10));
        assert_eq!(rec["y"], json!(20));
        assert_eq!(rec["width"], json!(300));
        assert_eq!(rec["height"], json!(400));

        // Cross-platform companions.
        assert_eq!(rec["app_name"], json!("example-app"));
        assert_eq!(rec["is_on_screen"], json!(true));
        assert_eq!(rec["z_index"], json!(3));
        assert_eq!(rec["window_id"], json!(42));
        assert_eq!(rec["title"], json!("Example"));
    }

    #[test]
    fn unavailable_wayland_order_serializes_as_null() {
        let w = crate::x11::WindowInfo {
            xid: 43,
            pid: Some(1234),
            app_name: "native-wayland-app".to_owned(),
            title: "Example".to_owned(),
            is_on_screen: true,
            z_index: None,
            x: 0,
            y: 0,
            width: 300,
            height: 400,
        };

        assert_eq!(window_record_json(&w)["z_index"], Value::Null);
    }

    #[test]
    fn chromium_launch_detection_uses_executable_basename() {
        assert!(chromium_family_program("/usr/bin/google-chrome-stable"));
        assert!(chromium_family_program("CuaTestHarness.Electron"));
        assert!(chromium_family_program("chromium-browser"));
        assert!(!chromium_family_program("/usr/bin/gnome-text-editor"));
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
            description: "Walk a running app's AT-SPI tree and return BOTH a \
                structured `elements` array (preferred) AND a Markdown rendering of \
                the same tree (back-compat). Every actionable element is tagged \
                with [element_index N] in the markdown and as `element_index` in \
                the structured array.\n\n\
                PREFERRED CONSUMERS read `structuredContent.elements` (one entry \
                per indexed row with `element_index`, `role`, `label`, `value`, \
                `enabled`, `selected`, \
                `frame: {x,y,w,h}` when AT-SPI reports usable bounds, \
                `parent_index`, `depth`). The markdown `tree_markdown` stays \
                available and unchanged in shape for existing text-parsing \
                callers — but new fields will only be added to the structured \
                side. Set `query` to project BOTH representations to matching \
                rows plus their ancestor chain while preserving original indices. \
                `total_element_count` reports the complete snapshot and \
                `returned_element_count` reports the projection.\n\n\
                Always returns BOTH the element tree AND a screenshot — ground on \
                both and cross-check (the tree lies on some surfaces). Choose the \
                modality at ACTION time: an element ax action \
                (element_index/element_token → accessibility rung) or an element px \
                action (x,y → pixel rung off this screenshot). capture_mode is \
                deprecated and ignored. On Wayland, where output capture cannot prove \
                the requested surface's identity, the truthful tree is returned without \
                a screenshot and `screenshot_error.code` is \
                `surface_identity_unproven`.\n\n\
                Optional `max_elements` / `max_depth` bound the AT-SPI walk to \
                mitigate context-window blow-up on Electron / large web apps \
                that produce 10k+ element trees. When applied, BOTH \
                the markdown and the structured elements are truncated \
                identically. Omit both for current default behaviour.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "pid":{"type":"integer"},
                "window_id":{"type":"integer","description":"Native window identifier from list_windows."},
                "capture_mode": cua_driver_core::capture_mode::capture_mode_schema(),
                "include_screenshot":{"type":"boolean",
                    "description":"Default true — returns a grounding screenshot alongside the tree. Set false to skip the grab and return tree only (the cheap path for re-indexing before an element ax action)."},
                "screenshot_out_file":{"type":"string",
                    "description":"When set, write the PNG to this file path (~ expanded) instead of embedding base64 in the response. The structured output carries screenshot_file_path instead."},
                "query":{"type":"string","description":"Optional case-insensitive substring. Projects both tree_markdown and structured elements to matches plus ancestors while preserving original indices. Compare total_element_count with returned_element_count."},
                "max_elements":{"type":"integer","minimum":1,"description":"Cap on total AT-SPI nodes walked. Omit for the default (5 000). Lower for huge web/Electron trees."},
                "max_depth":{"type":"integer","minimum":1,"description":"Cap on the AT-SPI tree walk depth. Omit for the default (uncapped). Lower for deeply nested apps."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let xid = match args.require_u64("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let max_dim = {
            let cfg = self.state.config.read().unwrap();
            cfg.max_image_dimension
        };
        // `capture_mode` is DEPRECATED and ignored — get_window_state always
        // returns BOTH the AT-SPI tree and a screenshot now, so the agent grounds
        // on both and cross-checks (the tree lies often enough that a grounding
        // screenshot should always be present). The modality is chosen at action
        // time: an element ax action (element_index) or element px action (x,y).
        // We don't even read the arg; it stays in the schema only so old callers
        // don't trip additionalProperties:false.
        let query = args.opt_str("query");
        // include_screenshot (default true) — the perf opt-out. The tree+screenshot
        // pair is the default; `include_screenshot:false` skips the grab and returns
        // tree only (the cheap re-index path before an element ax action). A
        // screenshot_out_file still forces a capture (to disk), regardless.
        let include_screenshot = args.get("include_screenshot").and_then(|v| v.as_bool());
        // screenshot_out_file: when set, write the PNG to disk and surface the
        // path instead of embedding base64 in the response. `~` expands.
        let screenshot_out_file = args.opt_str("screenshot_out_file").map(|s| {
            if let Some(rest) = s.strip_prefix("~/") {
                let home = std::env::var("HOME").unwrap_or_default();
                format!("{home}/{rest}")
            } else {
                s
            }
        });
        // Optional caps — when omitted, the AT-SPI walker uses its built-in
        // defaults (#22865).
        let max_elements = args
            .get("max_elements")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize);
        let max_depth = args
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .map(|v| v.max(1) as usize);

        let process_is_live = crate::proc_fs::is_process_live(pid);
        let window_matches = if crate::wayland::is_wayland() {
            crate::wayland::list_windows_dispatch(Some(pid))
                .iter()
                .any(|window| window.xid == xid && window.pid == Some(pid))
                || crate::wayland::window_was_listed_for_pid(pid, xid)
        } else {
            crate::x11::window_belongs_to_pid(xid, pid)
        };
        if !process_is_live || !window_matches {
            return ToolResult::error(format!(
                "Window target pid {pid}, window_id {xid} is stale or no longer running; refresh list_windows."
            ));
        }

        // Always walk the AT-SPI tree; capture the screenshot by default. The
        // tree+screenshot pair is the default so the agent grounds on both and
        // cross-checks the (sometimes-lying) tree against the frame. An explicit
        // `include_screenshot:false` skips the grab; an unproven Wayland surface
        // returns the tree with a typed screenshot error instead of unrelated pixels.
        let should_capture = include_screenshot != Some(false) || screenshot_out_file.is_some();
        let observation_only = args
            .get("_observation_only")
            .and_then(|value| value.as_bool())
            == Some(true);
        let state = self.state.clone();
        let query_for_walk = query.clone();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let tree_result = Some(crate::atspi::walk_tree_bounded(
                pid,
                xid,
                query_for_walk.as_deref(),
                max_elements,
                max_depth,
            ));
            // Bounds and element indices come from the same captured AT-SPI
            // traversal. Joining two live walks by ordinal mis-associated
            // Chromium controls when its lazy subtree changed between walks.
            let bounds = tree_result
                .as_ref()
                .map(|tree| tree.bounds.clone())
                .unwrap_or_default();
            // Capture and DELIVER the screenshot alongside the tree by default — the
            // grounding frame the agent cross-checks the tree against. With
            // screenshot_out_file set, write to disk and surface the path instead
            // of embedding base64; otherwise embed base64. Skipped only when
            // include_screenshot:false and no disk path was requested.
            // Tuple: (Option<b64>, Option<file_path>, w, h, Option<original_w>).
            let mut screenshot_error = None;
            let screenshot = if should_capture {
                match crate::wayland::screenshot_dispatch(xid) {
                    Ok(raw) => {
                        let orig_w = crate::capture::png_dimensions_pub(&raw)
                            .map(|(w, _)| w)
                            .unwrap_or(0);
                        let png = crate::capture::resize_png_if_needed(&raw, max_dim)?;
                        let (w, h) = crate::capture::png_dimensions_pub(&png)?;
                        let original_w = if w < orig_w { Some(orig_w) } else { None };
                        if let Some(ref path) = screenshot_out_file {
                            std::fs::write(path, &png)?;
                            Some((None, Some(path.clone()), w, h, original_w))
                        } else {
                            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
                            Some((Some(B64.encode(&png)), None, w, h, original_w))
                        }
                    }
                    Err(error) if crate::wayland::is_surface_identity_unproven(&error) => {
                        screenshot_error = Some(error.to_string());
                        None
                    }
                    Err(error) => {
                        return Err(anyhow::anyhow!(
                            "window screenshot failed for window {xid}: {error}"
                        ));
                    }
                }
            } else {
                None
            };
            Ok((tree_result, screenshot, bounds, screenshot_error))
        })
        .await;

        match result {
            Ok(Ok((tree_opt, shot_opt, bounds, screenshot_error))) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": xid, "pid": pid });

                if let Some(tr) = tree_opt {
                    let source_trusted = tr.trusted;
                    let source_degraded_reason = tr.degraded_reason.clone();
                    let count = tr
                        .nodes
                        .iter()
                        .filter(|n| n.element_index.is_some())
                        .count();
                    let header = format!("window_id={xid} pid={pid} elements={count}\n\n");
                    content.push(cua_driver_core::protocol::Content::text(
                        header + &tr.tree_markdown,
                    ));
                    if !observation_only {
                        state.element_cache.update(pid, xid, &tr.nodes);
                    }
                    structured["element_count"] = json!(count);
                    // AT-SPI's current bounded walker does not surface an
                    // exhaustive-walk proof. Keep negative existence unknown.
                    structured["elements_complete"] = json!(false);
                    structured["tree_markdown"] = json!(tr.tree_markdown);

                    // Surface 6: register a snapshot in the global token
                    // registry so each actionable element can be addressed
                    // by an opaque per-snapshot `element_token` alongside
                    // its existing integer `element_index`. The integer
                    // surface stays unchanged — the token is additive.
                    let snapshot_id = (!observation_only).then(|| {
                        cua_driver_core::element_token::global()
                            .register_snapshot(pid as i32, xid as u32, count)
                    });

                    // Structured `elements` array: one entry per actionable node.
                    // Shape: `{element_index, element_token, role, label,
                    // depth, parent_index?, frame?: {x,y,w,h}}`. Frame is
                    // included whenever AT-SPI Component.GetExtents(Screen)
                    // reported usable bounds; omitted otherwise (some
                    // toolkits leave bounds unset on hidden / virtual
                    // elements).
                    use std::collections::HashMap;
                    let bounds_by_idx: HashMap<usize, (i32, i32, u32, u32)> = bounds
                        .into_iter()
                        .map(|(i, x, y, w, h)| (i, (x, y, w, h)))
                        .collect();
                    let elements: Vec<serde_json::Value> = tr
                        .nodes
                        .iter()
                        .filter_map(|n| {
                            let idx = n.element_index?;
                            // `label` mirrors what a human reading the markdown row
                            // would call this element: name first, then value,
                            // then description.
                            let label = n
                                .name
                                .clone()
                                .or_else(|| n.value.clone())
                                .or_else(|| n.description.clone());
                            let mut entry = json!({
                                "element_index": idx,
                                "role": n.role,
                                "depth": n.depth,
                            });
                            if let Some(snapshot_id) = snapshot_id {
                                entry["element_token"] = json!(
                                    cua_driver_core::element_token::token_for(snapshot_id, idx)
                                );
                            }
                            if n.in_web_content {
                                entry["in_web_content"] = json!(true);
                            }
                            if let Some(label) = label {
                                entry["label"] = json!(label);
                            }
                            // Surface the element's value separately from `label`
                            // (which collapses name→value→description): a field
                            // with both a name AND typed text would otherwise hide
                            // the text from a caller reading the structured side,
                            // leaving it only in tree_markdown. See the macOS
                            // get_window_state builder for the rationale.
                            if let Some(value) = n.value.clone().filter(|v| !v.is_empty()) {
                                entry["value"] = json!(value);
                            }
                            if let Some(enabled) = n.enabled {
                                entry["enabled"] = json!(enabled);
                            }
                            if let Some(selected) = n.selected {
                                entry["selected"] = json!(selected);
                            }
                            if let Some(parent) = n.parent_element_index {
                                entry["parent_index"] = json!(parent);
                            }
                            if let Some((x, y, w, h)) = bounds_by_idx.get(&idx).copied() {
                                entry["frame"] = json!({ "x": x, "y": y, "w": w, "h": h });
                            }
                            Some(entry)
                        })
                        .collect();
                    let elements = cua_driver_core::element_query::project_elements_for_query(
                        elements,
                        query.as_deref(),
                        &tr.tree_markdown,
                    );
                    structured["total_element_count"] = json!(count);
                    structured["returned_element_count"] = json!(elements.len());
                    structured["elements"] = json!(elements);
                    // Surface 6: snapshot id mirror for debug correlation.
                    if let Some(snapshot_id) = snapshot_id {
                        structured["snapshot_id"] =
                            json!(cua_driver_core::element_token::token_for(snapshot_id, 0)
                                .trim_end_matches(":0")
                                .to_string());
                    }
                    structured["_note"] = json!(
                        "Prefer `elements` — `tree_markdown` will continue to work \
                         but new fields will only be added to the structured side. \
                         Use `max_elements` / `max_depth` to bound the \
                         AT-SPI walk on apps with very large trees."
                    );
                    // Best-effort-background ladder parity with macOS/Windows: an
                    // AT-SPI walk that ran but found zero actionable elements is
                    // NOT a clean "this window has no controls" — far more often
                    // the bridge wasn't ready (toolkit-accessibility off, or the
                    // daemon isn't on the desktop session bus so the registry is
                    // empty), or it's a non-AX surface (canvas/WebGL). Mark it
                    // degraded so callers don't read `elements: []` as authoritative.
                    if !source_trusted {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(source_degraded_reason
                            .unwrap_or_else(|| {
                                "x11_property_fallback_partial: AT-SPI was unavailable and \
                                 Cua Driver only recovered window metadata. Treat it as \
                                 discovery evidence; it cannot prove checked state."
                                    .to_owned()
                            }));
                    } else if count == 0 {
                        structured["degraded"] = json!(true);
                        structured["degraded_reason"] = json!(
                            "atspi_tree_empty: the AT-SPI walk returned no actionable \
                             elements. Common causes: the a11y bridge is off (enable \
                             `gsettings set org.gnome.desktop.interface \
                             toolkit-accessibility true`), the daemon is not on the \
                             desktop session bus (DBUS_SESSION_BUS_ADDRESS unreachable — \
                             run `cua-driver doctor`), or the window is a non-AX surface \
                             (canvas/WebGL/custom-drawn). Do not treat element data as \
                             authoritative — verify via the screenshot, and re-snapshot \
                             after enabling a11y or if the app just launched."
                        );
                        // Point the agent at the next rung explicitly: an empty
                        // tree means element_index has nothing to bind to. The
                        // recommendation is session-dependent (X11 → px,
                        // Wayland → foreground) — see non_ax_escalation.
                        structured["escalation"] = non_ax_escalation();
                    }
                }

                if let Some((b64_opt, file_path, w, h, orig_w)) = shot_opt {
                    if !observation_only {
                        if let Some(ow) = orig_w {
                            if w > 0 {
                                state.resize_registry.set_ratio(pid, ow as f64 / w as f64);
                            }
                        } else {
                            state.resize_registry.clear_ratio(pid);
                        }
                    }
                    // ax mode + screenshot_out_file writes the PNG to disk and
                    // returns b64=None — never embed the image bytes in that case.
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
                }
                if let Some(reason) = screenshot_error {
                    structured["screenshot_frame_valid"] = json!(false);
                    structured["screenshot_error"] = surface_identity_unproven_error(xid, reason);
                }

                ToolResult {
                    content,
                    is_error: None,
                    structured_content: Some(structured),
                    action_record: None,
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Capture error: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn surface_identity_unproven_error(xid: u64, reason: String) -> Value {
    json!({
        "code": "surface_identity_unproven",
        "window_id": xid,
        "reason": reason,
        "suggestion": "capture the full output explicitly, or retry on a compositor backend that supports identified per-window capture"
    })
}

// ── launch_app ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod get_window_state_capture_tests {
    use super::*;

    #[test]
    fn surface_identity_failure_is_a_typed_screenshot_error() {
        let error = surface_identity_unproven_error(
            0x2962,
            "surface_identity_unproven: fixture".to_owned(),
        );

        assert_eq!(error["code"], "surface_identity_unproven");
        assert_eq!(error["window_id"], 0x2962);
        assert!(error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("surface_identity_unproven")));
    }
}

pub struct LaunchAppTool;
static LAUNCH_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn contains_remote_debugging_flag(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("--remote-debugging-port") || lower.contains("--remote-debugging-pipe")
}

/// Spawn a launcher command line (an executable plus arguments, e.g. an XDG
/// `Exec=` value with field codes stripped) in the background and return the
/// child pid.
fn spawn_launch_command(cmd: &str, additional_arguments: &[String]) -> std::io::Result<u32> {
    let mut parts = cmd.split_whitespace();
    let prog = parts.next().unwrap_or(cmd);
    let mut rest: Vec<String> = parts.map(str::to_owned).collect();
    rest.extend(additional_arguments.iter().cloned());
    let mut launch = std::process::Command::new(prog);
    launch
        .args(&rest)
        // Enable accessibility for this child without toggling GNOME's global
        // ScreenReaderEnabled setting (which can launch Orca). Native
        // toolkits ignore these when they do not need them.
        .env("ACCESSIBILITY_ENABLED", "1")
        .env("NO_AT_BRIDGE", "0");
    if chromium_family_program(prog)
        && !rest
            .iter()
            .any(|arg| arg == "--force-renderer-accessibility")
    {
        launch.arg("--force-renderer-accessibility");
    }
    let child = launch.spawn()?;
    let pid = child.id();
    reap_in_background(child);
    Ok(pid)
}

/// Collect a launched child's exit status in the background.
///
/// `std::process::Child` does not reap on drop and the driver never waits on
/// an app it launches, so every launched app used to linger as a zombie for
/// the daemon's lifetime: it held a pid slot, and — because the pid stays
/// present — made "does this pid exist" liveness checks report a terminated
/// app as still running.
///
/// Reaping is scoped to one thread per child rather than setting
/// `SIGCHLD` to `SIG_IGN`, which is process-global and would break every
/// caller that needs an exit status, including the `xdg-open` check below
/// and any `Command::status()`/`output()` elsewhere in the daemon. The
/// thread blocks in `wait` until the app exits, so it costs nothing while
/// the app runs and never delays the launch itself.
fn reap_in_background(mut child: std::process::Child) {
    let pid = child.id();
    let reaper = std::thread::Builder::new()
        .name(format!("cua-reap-{pid}"))
        .stack_size(64 * 1024)
        .spawn(move || {
            if let Err(e) = child.wait() {
                tracing::debug!(pid, "launched app could not be reaped: {e}");
            }
        });
    if let Err(e) = reaper {
        tracing::debug!(pid, "could not start reaper thread for launched app: {e}");
    }
}

/// Match a launch_app `name` that failed direct exec against installed XDG
/// .desktop applications: exact display name, desktop-file id, or `Exec=`
/// basename first, then a display-name substring when it is unambiguous.
fn match_installed_app<'a>(
    apps: &'a [crate::installed_apps::InstalledApp],
    query: &str,
) -> Option<&'a crate::installed_apps::InstalledApp> {
    let q = query.to_ascii_lowercase();
    if q.is_empty() {
        return None;
    }
    apps.iter()
        .find(|a| {
            a.name.to_ascii_lowercase() == q
                || a.bundle_id.to_ascii_lowercase() == q
                || exec_basename(&a.launch_path) == q
        })
        .or_else(|| {
            let mut matches = apps
                .iter()
                .filter(|a| a.name.to_ascii_lowercase().contains(&q));
            match (matches.next(), matches.next()) {
                (Some(only), None) => Some(only),
                _ => None,
            }
        })
}

/// Watch a just-spawned `xdg-open` long enough to catch a fast failure.
/// xdg-open's generic fallback can `exec` the target app and stay alive for
/// its whole lifetime, so a child still running after the grace period counts
/// as success; a quick non-zero exit (2 = file not found, 3 = no handler
/// tool, 4 = action failed) is the only reliable failure signal.
fn xdg_open_failure(mut child: std::process::Child) -> Option<String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        // A branch that observes an exit status has already reaped the child;
        // one that gives up on it still owes that, so hand it to the reaper
        // instead of dropping it and leaving a zombie behind.
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return None,
            Ok(Some(status)) => return Some(format!("xdg-open failed ({status})")),
            Ok(None) if std::time::Instant::now() >= deadline => {
                reap_in_background(child);
                return None;
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(e) => {
                reap_in_background(child);
                return Some(format!("could not observe xdg-open: {e}"));
            }
        }
    }
}

#[cfg(test)]
mod launch_app_tests {
    use super::*;
    use crate::installed_apps::InstalledApp;

    fn app(name: &str, bundle_id: &str, launch_path: &str) -> InstalledApp {
        InstalledApp {
            name: name.to_owned(),
            bundle_id: bundle_id.to_owned(),
            launch_path: launch_path.to_owned(),
            last_used: None,
        }
    }

    fn fixture() -> Vec<InstalledApp> {
        vec![
            app("Galculator", "galculator", "galculator"),
            app(
                "Google Chrome",
                "google-chrome",
                "/usr/bin/google-chrome-stable",
            ),
            app("File Manager", "thunar", "thunar"),
            app(
                "File Manager Settings",
                "thunar-settings",
                "thunar-settings",
            ),
        ]
    }

    /// Process state letter from `/proc/<pid>/stat`, or `None` once the entry
    /// is gone. `comm` may itself contain spaces and parentheses, so the state
    /// is read after the final `)` rather than by splitting from the left.
    fn proc_state(pid: u32) -> Option<char> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(')')?.1;
        after_comm.split_whitespace().next()?.chars().next()
    }

    #[test]
    fn launched_children_are_reaped_instead_of_lingering_as_zombies() {
        // A launched app that exits must not stay in the process table. The
        // driver never waits on what it launches, so without an explicit
        // reaper every launch leaked a pid slot for the daemon's lifetime and
        // left "does this pid exist" liveness checks reporting a terminated
        // app as still running.
        // `/bin/sh` rather than a richer coreutils binary: it is the one
        // executable POSIX and the Nix build sandbox both guarantee, and the
        // sandbox has no `/bin/true`.
        let pid = spawn_launch_command("/bin/sh -c exit", &[]).expect("/bin/sh should spawn");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match proc_state(pid) {
                None => break,                    // reaped, entry gone
                Some(state) if state != 'Z' => {} // still running or exiting
                Some(_) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "pid {pid} was still a zombie after the reaper deadline"
                    );
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} was never reaped"
            );
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }

    #[test]
    fn matches_exact_display_name_case_insensitively() {
        let apps = fixture();
        let hit = match_installed_app(&apps, "google chrome").expect("should match");
        assert_eq!(hit.bundle_id, "google-chrome");
    }

    #[test]
    fn matches_desktop_file_id_and_exec_basename() {
        let apps = fixture();
        assert_eq!(
            match_installed_app(&apps, "galculator").unwrap().name,
            "Galculator"
        );
        assert_eq!(
            match_installed_app(&apps, "google-chrome-stable")
                .unwrap()
                .name,
            "Google Chrome"
        );
    }

    #[test]
    fn matches_unambiguous_display_name_substring() {
        let apps = fixture();
        assert_eq!(
            match_installed_app(&apps, "chrome").unwrap().name,
            "Google Chrome"
        );
    }

    #[test]
    fn refuses_ambiguous_substring_and_unknown_names() {
        let apps = fixture();
        // "file manager" is a substring of two entries — refuse to guess.
        // ("File Manager" itself still resolves via the exact-name rung.)
        assert!(match_installed_app(&apps, "file man").is_none());
        assert_eq!(
            match_installed_app(&apps, "file manager")
                .unwrap()
                .bundle_id,
            "thunar"
        );
        assert!(match_installed_app(&apps, "gnome-calculator").is_none());
        assert!(match_installed_app(&apps, "").is_none());
    }
}

#[async_trait]
impl Tool for LaunchAppTool {
    fn def(&self) -> &ToolDef {
        LAUNCH_DEF.get_or_init(|| ToolDef {
            name: "launch_app".into(),
            description: "Launch a Linux app in the background. Provide launch_path (preferred — \
                round-trip the value from list_apps), name (tried as a direct command, then \
                matched against installed .desktop applications, then handed to xdg-open if it \
                is a URL or existing file path), bundle_id (ignored on Linux), or urls (list of \
                URLs to open). Resolution precedence: launch_path > name > bundle_id. Errors \
                when the name resolves to nothing launchable.".into(),
            input_schema: json!({"type":"object","properties":{
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps` — the Exec= command from the .desktop file with XDG field codes already stripped. Highest precedence on Linux; spawned directly via the system shell."},
                "name":{"type":"string","description":"App name or command to launch. Tried as a direct command first, then matched against installed .desktop applications (exact display name, desktop-file id, or Exec basename; else an unambiguous display-name substring)."},
                "bundle_id":{"type":"string","description":"Ignored on Linux (macOS/Windows concept)."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open via xdg-open."},
                "additional_arguments":{"type":"array","items":{"type":"string"},"description":"Extra command-line arguments passed to the launched process."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let launch_path_opt = args.opt_str("launch_path");
        let name_opt = args.opt_str("name");
        let urls: Vec<String> = args.str_array("urls");
        let additional_arguments: Vec<String> = args.str_array("additional_arguments");
        if args.get("cdp_debugging_port").is_some() {
            return ToolResult::error(
                "cdp_debugging_port moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }
        if launch_path_opt
            .as_deref()
            .into_iter()
            .chain(name_opt.as_deref())
            .chain(additional_arguments.iter().map(String::as_str))
            .any(contains_remote_debugging_flag)
        {
            return ToolResult::error(
                "Chromium remote-debugging flags moved to browser_prepare so DevTools is never enabled on an unproven user profile",
            );
        }

        if launch_path_opt.is_none() && name_opt.is_none() && urls.is_empty() {
            return ToolResult::error("Provide at least one of: launch_path, name, or urls.");
        }

        let result = tokio::task::spawn_blocking(
            move || -> anyhow::Result<(String, Option<u32>, String)> {
                // Open URLs via xdg-open.
                if !urls.is_empty() {
                    let mut children = Vec::new();
                    for url in &urls {
                        children.push((
                            url.clone(),
                            std::process::Command::new("xdg-open").arg(url).spawn()?,
                        ));
                    }
                    for (url, child) in children {
                        if let Some(reason) = xdg_open_failure(child) {
                            anyhow::bail!("could not open '{url}': {reason}");
                        }
                    }
                    return Ok((
                        format!("Opened {} URL(s) via xdg-open.", urls.len()),
                        None,
                        "xdg-open".to_owned(),
                    ));
                }
                // launch_path > name. Both go through the same direct-exec path
                // (so XDG `Exec=` commands round-trip), but launch_path is the
                // canonical form preferred by list_apps callers.
                let command = launch_path_opt.as_deref().or(name_opt.as_deref());
                if let Some(cmd) = command {
                    match spawn_launch_command(cmd, &additional_arguments) {
                        Ok(pid) => {
                            return Ok((
                                format!("✅ Launched {cmd} (pid {pid}) in background."),
                                Some(pid),
                                cmd.to_owned(),
                            ));
                        }
                        Err(_) => {
                            // Not an executable on PATH. Resolve the name against
                            // installed XDG .desktop applications — the same
                            // source list_apps reads — and run the match's Exec=.
                            let installed = crate::installed_apps::list_installed_apps();
                            if let Some(app) = match_installed_app(&installed, cmd) {
                                let pid =
                                    spawn_launch_command(&app.launch_path, &additional_arguments)
                                        .map_err(|e| {
                                        anyhow::anyhow!(
                                            "'{cmd}' matched installed app '{}' but its launcher \
                                         `{}` failed to start: {e}",
                                            app.name,
                                            app.launch_path
                                        )
                                    })?;
                                return Ok((
                                    format!(
                                        "✅ Launched {} (`{}`, pid {pid}) in background — \
                                         resolved '{cmd}' via its desktop entry.",
                                        app.name, app.launch_path
                                    ),
                                    Some(pid),
                                    app.name.clone(),
                                ));
                            }
                            // xdg-open handles URLs and file paths, not app
                            // names — only fall through for something it can
                            // plausibly open, and surface its fast non-zero
                            // exit instead of reporting a launch that never
                            // happened.
                            if !cmd.contains("://") && !std::path::Path::new(cmd).exists() {
                                anyhow::bail!(
                                    "'{cmd}' is not an executable on PATH and matches no \
                                     installed .desktop application. Call list_apps and \
                                     round-trip its launch_path."
                                );
                            }
                            let child = std::process::Command::new("xdg-open").arg(cmd).spawn()?;
                            if let Some(reason) = xdg_open_failure(child) {
                                anyhow::bail!("could not open '{cmd}': {reason}");
                            }
                            // xdg-open may spawn a helper and exit, so do not
                            // claim its pid is the app pid.
                            return Ok((
                                format!("Opened '{cmd}' via xdg-open."),
                                None,
                                cmd.to_owned(),
                            ));
                        }
                    }
                }
                unreachable!()
            },
        )
        .await;

        match result {
            Ok(Ok((message, pid_opt, name))) => {
                if let Some(pid) = pid_opt {
                    let windows = tokio::task::spawn_blocking(move || {
                        let deadline =
                            std::time::Instant::now() + std::time::Duration::from_secs(3);
                        loop {
                            let windows = crate::wayland::list_windows_dispatch(Some(pid));
                            if !windows.is_empty() || std::time::Instant::now() >= deadline {
                                return windows.iter().map(window_record_json).collect::<Vec<_>>();
                            }
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    })
                    .await
                    .unwrap_or_default();
                    ToolResult::text(message).with_structured(json!({
                        "pid": pid,
                        "bundle_id": Value::Null,
                        "name": name,
                        "running": true,
                        "active": false,
                        "windows": windows,
                    }))
                } else {
                    ToolResult::text(message).with_structured(json!({
                        "pid": Value::Null,
                        "bundle_id": Value::Null,
                        "name": name,
                        "running": Value::Null,
                        "active": false,
                        "windows": [],
                    }))
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Failed to launch: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

fn chromium_family_program(program: &str) -> bool {
    let basename = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    ["chrome", "chromium", "electron", "brave", "edge"]
        .iter()
        .any(|needle| basename.contains(needle))
}

// ── shared helpers ────────────────────────────────────────────────────────────

/// Resolve an AT-SPI element's center in window-local coordinates.
///
/// Returns `(xid, window_local_x, window_local_y)`.
/// Looks up element bounds via native AT-SPI, finds the owning window
/// (via `xid_hint` or the first window for `pid`), then converts screen-absolute
/// → window-local coords via compositor metadata on Wayland or
/// XTranslateCoordinates on X11.
fn resolve_element_local_coords(
    pid: u32,
    idx: usize,
    xid_hint: Option<u64>,
) -> anyhow::Result<(u64, f64, f64)> {
    let (bx, by, bw, bh) = crate::atspi::get_element_bounds(pid, idx)?;
    let screen_cx = bx as f64 + bw as f64 / 2.0;
    let screen_cy = by as f64 + bh as f64 / 2.0;

    let xid = if let Some(x) = xid_hint {
        x
    } else if crate::wayland::wayland_input_enabled() {
        crate::wayland::list_windows_dispatch(Some(pid))
            .into_iter()
            .next()
            .map(|window| window.xid)
            .ok_or_else(|| anyhow::anyhow!("No Wayland windows for pid {pid}"))?
    } else {
        crate::x11::list_windows(Some(pid))
            .into_iter()
            .next()
            .map(|w| w.xid)
            .ok_or_else(|| anyhow::anyhow!("No windows for pid {pid}"))?
    };

    if crate::wayland::wayland_input_enabled() {
        let (window_x, window_y, window_width, window_height) =
            crate::wayland::window_geometry(xid)
                .ok_or_else(|| anyhow::anyhow!("No Wayland geometry for window {xid}"))?;
        if window_width == 0 || window_height == 0 {
            anyhow::bail!("Wayland window {xid} has no usable geometry");
        }
        return Ok((
            xid,
            screen_cx - window_x as f64,
            screen_cy - window_y as f64,
        ));
    }

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    let local_x = screen_cx - reply.dst_x as f64;
    let local_y = screen_cy - reply.dst_y as f64;
    Ok((xid, local_x, local_y))
}

fn element_screen_center(pid: u32, idx: usize) -> anyhow::Result<(f64, f64)> {
    let (bx, by, bw, bh) = crate::atspi::get_element_bounds(pid, idx)?;
    Ok((bx as f64 + bw as f64 / 2.0, by as f64 + bh as f64 / 2.0))
}

fn window_local_to_screen(xid: u64, x: f64, y: f64) -> anyhow::Result<(f64, f64)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let reply = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    Ok((reply.dst_x as f64 + x, reply.dst_y as f64 + y))
}

fn parse_mouse_button(name: &str) -> u8 {
    match name {
        "right" => 3,
        "middle" => 2,
        _ => 1,
    }
}

/// Escalation hint for a non-AX / suspected-no-op surface — the cross-platform
/// `escalation` field mirrored from macOS. The recommended next rung depends on
/// the session type: X11 can pixel-target a specific window in the background,
/// so the deliberate move is an element px action — click by pixel (x,y) off the
/// screenshot already in this response (`px`). Native Wayland CANNOT
/// background-target an unfocused window (libei injects to the compositor's input
/// focus — see `input::delivery`), and the pixel path itself routes through the
/// same coordinate-free AT-SPI actuation that just no-op'd, so the move there is
/// to bring the window to the foreground first (`foreground`).
fn non_ax_escalation() -> Value {
    if crate::wayland::is_wayland() {
        json!({
            "recommended": "foreground",
            "reason": "non-AX surface on Wayland: a specific unfocused window can't be \
                       pixel-targeted in the background (libei injects to the compositor's \
                       focus). bring_to_front, then click by pixel off the screenshot \
                       with delivery_mode:\"foreground\"."
        })
    } else {
        json!({
            "recommended": "px",
            "reason": "non-AX surface — act by pixel (x,y) off the screenshot \
                       in this response (an element px action)."
        })
    }
}

/// Structured payload for a `type_text` response. Keeps the legacy
/// `path`/`characters`/`verified` fields for back-compat and adds the cross-tool
/// `effect` tri-state. Linux's AT-SPI `insertText` return value acknowledges
/// the method call but does not read the widget value back, so it and every
/// keystroke / XSendEvent / XTest / Wayland rung are `"unverifiable"` (the
/// caller confirms through a separate observation) and
/// carries a `foreground` escalation, because the field IS in the AT-SPI tree —
/// it's a delivery/focus problem, not a missing element. The foreground rung
/// itself (`key_events_fg`) is already the last resort, so it emits no
/// escalation. Mirrors the macOS `type_text` contract.
fn type_text_structured(path: &str, characters: usize, verified: bool) -> Value {
    let mut s = json!({
        "path": path,
        "characters": characters,
        "verified": verified,
        "effect": if verified { "confirmed" } else { "unverifiable" },
    });
    if !verified && path != "key_events_fg" {
        s["escalation"] = json!({
            "recommended": "foreground",
            "reason": "background insert could not be confirmed — re-call with \
                       delivery_mode:\"foreground\" if a screenshot shows the text \
                       didn't land."
        });
    }
    s
}

/// Structured payload for the Electron/Chromium AX-echo case: the AT-SPI
/// `insertText` rung returned success on a Chromium embedder, but on those
/// surfaces the a11y bridge can accept and echo the write while the renderer
/// never observes it — so a path=="ax" "confirm" is a shim echo, not ground
/// truth. Mirrors the macOS `type_text` `ax_echo_surface` branch: report
/// effect:"unverifiable" + escalation:{recommended:"px"} (a renderer-focus
/// problem — pixel-focus the field, not foreground-activate it).
fn type_text_structured_electron(text_len: usize) -> Value {
    json!({
        "path": "ax",
        "characters": text_len,
        "verified": false,
        "effect": "unverifiable",
        "escalation": {
            "recommended": "px",
            "reason": "Electron/web surface — the AX write was echoed but the \
                       renderer may not have observed it. Confirm via the screenshot; \
                       if it didn't land, re-type with the element px action (pass x,y \
                       to pixel-focus the field, then type)."
        }
    })
}

/// Build the success `ToolResult` for an AT-SPI insert. The EditableText
/// method's boolean is a delivery acknowledgement, not a fresh value readback,
/// so native widgets remain `unverifiable`. Chromium embedders additionally
/// recommend the pixel rung because their accessibility bridge can acknowledge
/// a write the renderer never observes.
fn type_text_ax_result(pid: u32, text_len: usize, route: &str) -> ToolResult {
    if is_chromium_embedder(pid) {
        return ToolResult::text(format!(
            "📨 Sent (unverified) {text_len} character(s) ({route}). — Electron/web \
             surface: the AX layer accepts and echoes the write but the renderer may \
             not have observed it, so the driver cannot confirm via AX. Verify via the \
             screenshot; if it didn't land, re-type with the px form (pass x,y to \
             pixel-focus the field)."
        ))
        .with_structured(type_text_structured_electron(text_len));
    }
    ToolResult::text(format!("Typed {text_len} character(s) ({route})."))
        .with_structured(type_text_structured("ax", text_len, false))
}

/// True when `pid` is a Chromium-based embedder — a Chrome/Chromium browser or
/// any Electron/CEF app. On these surfaces an AT-SPI `EditableText.insertText`
/// can succeed at the bridge while the Chromium *renderer* never observes it,
/// so the AT-SPI "ax" rung must not be trusted as a confirmed insert (see
/// [`type_text_ax_result`]).
///
/// This is the Linux analogue of macOS `ElectronJs::is_electron` (which checks
/// for a bundled Electron Framework). Linux has no bundle, so the signal is
/// Chromium's multiprocess fingerprint: the embedder forks sandboxed helpers
/// whose argv carries `--type=renderer` / `--type=zygote` / `--type=gpu-process`.
/// Native GTK/Qt apps never spawn such helpers, so this is conservative (very
/// low false-positive). The single-process fallback also matches the embedder's
/// own argv. Reads `/proc`; cheap and only invoked on the rare AT-SPI confirm.
fn is_chromium_embedder(pid: u32) -> bool {
    fn argv_is_chromium_helper(p: u32) -> bool {
        match fs::read(format!("/proc/{p}/cmdline")) {
            Ok(raw) => String::from_utf8_lossy(&raw).split('\0').any(|arg| {
                arg == "--type=renderer" || arg == "--type=zygote" || arg == "--type=gpu-process"
            }),
            Err(_) => false,
        }
    }
    // Single-process / the embedder itself carrying a Chromium switch.
    if argv_is_chromium_helper(pid) {
        return true;
    }
    // Build PPid → children across /proc, then BFS the descendants of `pid`
    // looking for a Chromium helper. Same /proc-walk shape as
    // `terminal_descendant_ttys`.
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return false,
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let child: u32 = match name.to_string_lossy().parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let status = match fs::read_to_string(format!("/proc/{child}/status")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        if let Some(ppid) = status
            .lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l[5..].trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(child);
        }
    }
    let mut queue = std::collections::VecDeque::from([pid]);
    let mut seen = std::collections::HashSet::new();
    while let Some(cur) = queue.pop_front() {
        if !seen.insert(cur) {
            continue;
        }
        if let Some(kids) = children.get(&cur) {
            for &kid in kids {
                if argv_is_chromium_helper(kid) {
                    return true;
                }
                queue.push_back(kid);
            }
        }
    }
    false
}

fn is_webkitgtk_embedder(pid: u32) -> bool {
    fn argv_is_webkit_helper(pid: u32) -> bool {
        fs::read(format!("/proc/{pid}/cmdline"))
            .map(|raw| {
                let cmdline = String::from_utf8_lossy(&raw);
                cmdline.contains("WebKitWebProcess") || cmdline.contains("WebKitNetworkProcess")
            })
            .unwrap_or(false)
    }

    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let Ok(child) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(status) = fs::read_to_string(format!("/proc/{child}/status")) else {
            continue;
        };
        if let Some(ppid) = status
            .lines()
            .find(|line| line.starts_with("PPid:"))
            .and_then(|line| line[5..].trim().parse::<u32>().ok())
        {
            children.entry(ppid).or_default().push(child);
        }
    }
    let mut queue = std::collections::VecDeque::from([pid]);
    let mut seen = std::collections::HashSet::new();
    while let Some(current) = queue.pop_front() {
        if !seen.insert(current) {
            continue;
        }
        if argv_is_webkit_helper(current) {
            return true;
        }
        if let Some(descendants) = children.get(&current) {
            queue.extend(descendants.iter().copied());
        }
    }
    false
}

fn maps_indicate_gtk(maps: &str) -> bool {
    maps.contains("libgtk-3.so") || maps.contains("libgtk-4.so")
}

fn is_gtk_process(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/maps"))
        .map(|maps| maps_indicate_gtk(&maps))
        .unwrap_or(false)
}

fn unavailable_webkit_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground()
        && is_webkitgtk_embedder(pid)
        && !crate::wayland::is_inject_mode()
        && !crate::input::real_pointer_input_available())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::WebKitSyntheticInput,
        )
    })
}

fn unavailable_webkit_keyboard_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground() && is_webkitgtk_embedder(pid) && !crate::wayland::is_inject_mode())
        .then(|| {
            crate::input::delivery::background_unavailable_error(
                crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
            )
        })
}

fn unavailable_gtk_keyboard_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground() && is_gtk_process(pid) && !crate::wayland::is_inject_mode()).then(
        || {
            crate::input::delivery::background_unavailable_error(
                crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
            )
        },
    )
}

fn unavailable_gtk_pointer_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    (!delivery.is_foreground()
        && is_gtk_process(pid)
        && !crate::wayland::is_inject_mode()
        && !crate::input::real_pointer_input_available())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
        )
    })
}

fn unavailable_wayland_focused_input_background(
    delivery: crate::input::delivery::DeliveryMode,
    focus_free_inject_supported: bool,
) -> Option<ToolResult> {
    (crate::wayland::wayland_input_enabled()
        && !(focus_free_inject_supported && crate::wayland::is_inject_mode())
        && !delivery.is_foreground())
    .then(|| {
        crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
        )
    })
}

/// Chromium's X11 renderer drops synthetic input sent to an occluded,
/// unfocused toplevel. Returning success here would be a silent loss, so all
/// input tools expose the same typed refusal and leave foreground activation
/// as the explicit escalation.
fn unavailable_chromium_background(
    pid: u32,
    delivery: crate::input::delivery::DeliveryMode,
) -> Option<ToolResult> {
    if chromium_background_must_refuse(
        delivery.is_foreground(),
        crate::wayland::is_inject_mode(),
        is_chromium_embedder(pid),
    ) {
        Some(crate::input::delivery::background_unavailable_error(
            crate::input::delivery::BackgroundUnavailable::ChromiumInput,
        ))
    } else {
        None
    }
}

fn chromium_background_must_refuse(
    foreground: bool,
    focus_free_inject_mode: bool,
    chromium: bool,
) -> bool {
    chromium && !foreground && !focus_free_inject_mode
}

/// Screen-absolute center of a window (top-left from translate_coordinates plus
/// half its geometry). Used to position the no-focus-steal scroll over the
/// window's content. Blocking — call inside spawn_blocking.
fn window_screen_center(xid: u64) -> anyhow::Result<(i32, i32)> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    let geom = conn.get_geometry(xid as u32)?.reply()?;
    let trans = conn
        .translate_coordinates(xid as u32, root, 0, 0)?
        .reply()?;
    Ok((
        trans.dst_x as i32 + geom.width as i32 / 2,
        trans.dst_y as i32 + geom.height as i32 / 2,
    ))
}

/// X11 no-focus-steal pixel click with graceful fallback. On a real Xorg host
/// the MPX uinput pointer + XI2 shield grab lands a *true* button event on
/// XInput2 toolkits (GTK3/4) that silently drop synthetic `XSendEvent` pointers
/// — so right / middle / double clicks actually register. On Xvfb / Xtigervnc /
/// unsupported servers (`real_pointer_input_available()` returns false) or if
/// the MPX attempt fails, it falls back to the legacy `XSendEvent` path so
/// headless tests and core-only toolkits keep working. `lx`,`ly` are
/// window-local; screen-absolute coords for the warp are derived here. Blocking
/// — call inside spawn_blocking.
fn x11_pixel_click_no_focus_steal(
    cursor_id: &str,
    xid: u64,
    lx: i32,
    ly: i32,
    button: u8,
    count: usize,
) -> anyhow::Result<()> {
    if crate::input::real_pointer_input_available() {
        if let Ok((sx, sy)) = window_local_to_screen(xid, lx as f64, ly as f64) {
            match crate::input::send_virtual_pointer_click(
                cursor_id,
                &crate::input::VirtualPointerClick {
                    target_window: xid,
                    x: sx.round() as i32,
                    y: sy.round() as i32,
                    button,
                    count,
                },
            ) {
                Ok(()) => return Ok(()),
                Err(error) if crate::input::is_uinput_unavailable(&error) => return Err(error),
                Err(e) => tracing::warn!("MPX click fell back to XSendEvent: {e}"),
            }
        }
    }
    crate::input::send_click(xid, lx, ly, count, button)
}

fn linux_input_error(error: anyhow::Error) -> ToolResult {
    if crate::input::is_uinput_unavailable(&error) {
        ToolResult::error(error.to_string()).with_structured(json!({
            "code": crate::input::UINPUT_UNAVAILABLE_CODE,
        }))
    } else {
        ToolResult::error(error.to_string())
    }
}

#[cfg(test)]
#[test]
fn uinput_failure_has_a_stable_structured_code() {
    let result = linux_input_error(crate::input::uinput_unavailable("synthetic failure"));
    assert_eq!(result.is_error, Some(true));
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str),
        Some(crate::input::UINPUT_UNAVAILABLE_CODE)
    );
}

fn x11_pixel_click_no_focus_steal_modifiers(
    xid: u64,
    lx: i32,
    ly: i32,
    button: u8,
    count: usize,
    modifiers: &[&str],
) -> anyhow::Result<()> {
    crate::input::send_click_with_modifiers(xid, lx, ly, count, button, modifiers)
}

fn mouse_button_name(button: u8) -> &'static str {
    match button {
        3 => "right",
        2 => "middle",
        _ => "left",
    }
}

fn resolve_cursor_key(args: &Value) -> String {
    for key in ["session", "_session_id", "cursor_id"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return v.to_owned();
            }
        }
    }
    "default".to_owned()
}

/// Return the cursor key only for a lifecycle-owned session. Cursor positioning
/// for keyboard actions deliberately does not opt anonymous calls or the
/// legacy cursor_id-only path into session semantics. The proxy-minted
/// `_session_id` is trusted lifecycle state and must behave like a public named
/// session for cursor ownership.
fn named_session_cursor_key(args: &Value) -> Option<String> {
    ["session", "_session_id"].into_iter().find_map(|key| {
        args.get(key)
            .and_then(Value::as_str)
            .filter(|session| !session.is_empty())
            .map(str::to_owned)
    })
}

fn finite_cursor_point(point: Option<(f64, f64)>) -> Option<(f64, f64)> {
    point.filter(|(x, y)| x.is_finite() && y.is_finite())
}

fn choose_keyboard_cursor_target(
    explicit: Option<(f64, f64)>,
    remembered: Option<(f64, f64)>,
    window_center: Option<(f64, f64)>,
    current_pointer: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    finite_cursor_point(explicit)
        .or_else(|| finite_cursor_point(remembered))
        .or_else(|| finite_cursor_point(window_center))
        .or_else(|| finite_cursor_point(current_pointer))
}

fn mouse_hold_json(cursor_id: &str, hold: Option<&MouseHoldState>) -> Value {
    match hold {
        Some(hold) => json!({
            "cursor_id": cursor_id,
            "held": true,
            "pid": hold.pid,
            "window_id": hold.xid,
            "button": mouse_button_name(hold.button),
            "x": hold.x,
            "y": hold.y,
        }),
        None => json!({
            "cursor_id": cursor_id,
            "held": false,
            "pid": Value::Null,
            "window_id": Value::Null,
            "button": Value::Null,
            "x": Value::Null,
            "y": Value::Null,
        }),
    }
}

fn held_target_mismatch(
    args: &Value,
    cursor_id: &str,
    hold: &MouseHoldState,
) -> Option<ToolResult> {
    match args.opt_u32("pid") {
        Ok(Some(pid)) if pid != hold.pid => {
            return Some(
                ToolResult::error(format!(
                    "Cursor '{cursor_id}' is holding a button for pid {}, not pid {pid}.",
                    hold.pid
                ))
                .with_structured(mouse_hold_json(cursor_id, Some(hold))),
            );
        }
        Err(err) => return Some(err.with_structured(mouse_hold_json(cursor_id, Some(hold)))),
        _ => {}
    }

    match args.opt_u64("window_id") {
        Some(xid) if xid != hold.xid => Some(
            ToolResult::error(format!(
                "Cursor '{cursor_id}' is holding a button for window_id {}, not {xid}.",
                hold.xid
            ))
            .with_structured(mouse_hold_json(cursor_id, Some(hold))),
        ),
        _ => None,
    }
}

fn overlay_snap_to_for(cursor_id: &str, sx: f64, sy: f64, heading: Option<f64>) {
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::SnapTo {
            x: sx,
            y: sy,
            heading_radians: heading,
        },
    );
}

fn overlay_move_to_for(cursor_id: &str, sx: f64, sy: f64, heading: Option<f64>) {
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::MoveTo {
            x: sx,
            y: sy,
            end_heading_radians: heading.unwrap_or(std::f64::consts::FRAC_PI_4),
        },
    );
}

async fn overlay_glide_to_for(cursor_id: &str, sx: f64, sy: f64) {
    // Input always revives its agent cursor. Hiding it is useful while idle,
    // but a hidden cursor must not make pointer or keyboard control invisible.
    crate::overlay::send_command_for(
        cursor_id.to_owned(),
        cursor_overlay::OverlayCommand::SetEnabled(true),
    );
    // Every Wayland backend owns its animation loop. Send one destination and
    // let the Linux overlay layer select the semantic helper, layer-shell, or
    // older helper fallback without an interpolated stream fighting it.
    if crate::wayland::is_wayland() {
        overlay_move_to_for(cursor_id, sx, sy, None);
        return;
    }
    let pos = crate::overlay::current_position_for(cursor_id);
    if pos.0 < 0.0 && pos.1 < 0.0 {
        crate::overlay::send_command_for(
            cursor_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
        );
        return;
    }
    crate::overlay::animate_cursor_to_for(cursor_id.to_owned(), sx, sy).await;
}

/// Keep the logical cursor position in sync with every visibly targeted
/// pointer action. Overlay delivery is intentionally best-effort: registry
/// state is still updated when no renderer is running or its queue is closed.
async fn reveal_pointer_action_for(
    state: &ToolState,
    cursor_id: &str,
    sx: f64,
    sy: f64,
    click_pulse: bool,
) {
    if !sx.is_finite() || !sy.is_finite() {
        return;
    }
    state.cursor_registry.set_enabled(cursor_id, true);
    state.cursor_registry.update_position(cursor_id, sx, sy);
    overlay_glide_to_for(cursor_id, sx, sy).await;
    if click_pulse {
        crate::overlay::send_command_for(
            cursor_id.to_owned(),
            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
        );
    }
}

fn keyboard_window_center(xid: u64) -> Option<(f64, f64)> {
    if xid == 0 {
        return None;
    }
    if crate::wayland::is_wayland() {
        return crate::wayland::window_geometry(xid).and_then(|(x, y, width, height)| {
            (width > 0 && height > 0).then_some((
                f64::from(x) + f64::from(width) / 2.0,
                f64::from(y) + f64::from(height) / 2.0,
            ))
        });
    }
    window_screen_center(xid)
        .ok()
        .map(|(x, y)| (f64::from(x), f64::from(y)))
}

fn current_pointer_position() -> Option<(f64, f64)> {
    if crate::wayland::is_wayland() {
        return crate::wayland::last_synth_cursor_pos().map(|(x, y)| (f64::from(x), f64::from(y)));
    }

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::ConnectionExt as _;
    use x11rb::rust_connection::RustConnection;

    let (connection, screen_num) = RustConnection::connect(None).ok()?;
    let root = connection.setup().roots[screen_num].root;
    let reply = connection.query_pointer(root).ok()?.reply().ok()?;
    Some((f64::from(reply.root_x), f64::from(reply.root_y)))
}

fn explicit_keyboard_cursor_target(
    pid: u32,
    xid: u64,
    element_index: Option<usize>,
    pixel_target: Option<(f64, f64)>,
) -> Option<(f64, f64)> {
    if let Some(element_index) = element_index {
        let (sx, sy) = element_screen_center(pid, element_index).ok()?;
        return Some((sx, sy));
    }

    let (x, y) = pixel_target?;
    if crate::wayland::wayland_input_enabled() {
        return crate::wayland::window_geometry(xid)
            .map(|(wx, wy, _, _)| (f64::from(wx) + x.round(), f64::from(wy) + y.round()));
    }
    window_local_to_screen(xid, x, y).ok()
}

/// Position and reveal a named session's cursor before keyboard/value input.
/// `preserve_legacy_element_visual` retains the existing element-only feedback
/// for type_text/set_value anonymous calls without adding session-style fallback
/// placement to them. Geometry and overlay failures are observational and never
/// affect the tool's actual input result.
async fn position_named_session_keyboard_cursor(
    state: &ToolState,
    args: &Value,
    pid: u32,
    xid: u64,
    element_index: Option<usize>,
    pixel_target: Option<(f64, f64)>,
    preserve_legacy_element_visual: bool,
) {
    let named_cursor_id = named_session_cursor_key(args);
    let cursor_id = match named_cursor_id {
        Some(ref cursor_id) => cursor_id.clone(),
        None if preserve_legacy_element_visual && element_index.is_some() => {
            resolve_cursor_key(args)
        }
        None => return,
    };

    let remembered = named_cursor_id.as_ref().and_then(|_| {
        state
            .cursor_registry
            .get(&cursor_id)
            .and_then(|cursor| cursor.x.zip(cursor.y))
    });
    let explicit = tokio::task::spawn_blocking(move || {
        explicit_keyboard_cursor_target(pid, xid, element_index, pixel_target)
    })
    .await
    .ok()
    .flatten();

    let fallback = if named_cursor_id.is_some()
        && explicit.is_none()
        && finite_cursor_point(remembered).is_none()
    {
        tokio::task::spawn_blocking(move || {
            let center = keyboard_window_center(xid);
            let pointer = center.is_none().then(current_pointer_position).flatten();
            (center, pointer)
        })
        .await
        .unwrap_or((None, None))
    } else {
        (None, None)
    };

    let Some((sx, sy)) =
        choose_keyboard_cursor_target(explicit, remembered, fallback.0, fallback.1)
    else {
        return;
    };
    if xid != 0 {
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
    }
    reveal_pointer_action_for(state, &cursor_id, sx, sy, false).await;
}

async fn track_overlay_drag_for(
    cursor_id: String,
    from: (f64, f64),
    to: (f64, f64),
    duration_ms: u64,
    steps: usize,
) {
    crate::overlay::send_command_for(
        cursor_id.clone(),
        cursor_overlay::OverlayCommand::SetEnabled(true),
    );
    crate::overlay::send_command_for(
        cursor_id.clone(),
        cursor_overlay::OverlayCommand::SetPressed(true),
    );
    let steps = steps.max(1);
    let step_delay = std::time::Duration::from_millis(duration_ms / steps as u64);
    for index in 0..=steps {
        let t = index as f64 / steps as f64;
        let x = from.0 + (to.0 - from.0) * t;
        let y = from.1 + (to.1 - from.1) * t;
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::track_pointer_command(x, y),
        );
        if index < steps && !step_delay.is_zero() {
            tokio::time::sleep(step_delay).await;
        }
    }
    crate::overlay::send_command_for(cursor_id, cursor_overlay::OverlayCommand::SetPressed(false));
}

fn process_name(pid: u32) -> Option<String> {
    let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let first = String::from_utf8_lossy(&cmdline)
        .split('\0')
        .next()
        .unwrap_or("")
        .trim()
        .to_owned();
    if !first.is_empty() {
        return std::path::Path::new(&first)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .or(Some(first));
    }

    let status = fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    status
        .lines()
        .find(|l| l.starts_with("Name:"))
        .map(|l| l[5..].trim().to_owned())
}

fn is_terminal_process(pid: u32) -> bool {
    // Canonical list lives in `crate::terminal::TERMINAL_PROCESS_NAMES`
    // — keeps the additive contract centralised. Adding a new terminal
    // here means appending one string in `crate::terminal`.
    match process_name(pid).as_deref() {
        Some(name) => crate::terminal::is_terminal_process_name(name),
        None => false,
    }
}

fn terminal_descendant_ttys(pid: u32) -> Vec<PathBuf> {
    let mut parent_to_children: std::collections::HashMap<u32, Vec<u32>> =
        std::collections::HashMap::new();
    let proc_dir = std::path::Path::new("/proc");
    let entries = match fs::read_dir(proc_dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };

    for entry in entries.flatten() {
        let pid_str = entry.file_name();
        let pid_str = pid_str.to_string_lossy();
        let child_pid: u32 = match pid_str.parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        let status = match fs::read_to_string(proc_dir.join(&*pid_str).join("status")) {
            Ok(status) => status,
            Err(_) => continue,
        };
        let parent_pid = status
            .lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l[5..].trim().parse::<u32>().ok());
        if let Some(parent_pid) = parent_pid {
            parent_to_children
                .entry(parent_pid)
                .or_default()
                .push(child_pid);
        }
    }

    let mut descendants = Vec::new();
    let mut queue = std::collections::VecDeque::from([pid]);
    while let Some(current) = queue.pop_front() {
        if let Some(children) = parent_to_children.get(&current) {
            for &child in children {
                descendants.push(child);
                queue.push_back(child);
            }
        }
    }
    descendants.sort_unstable();

    let mut ttys = Vec::new();
    for child in descendants {
        let tty = match fs::read_link(format!("/proc/{child}/fd/0")) {
            Ok(path) => path,
            Err(_) => continue,
        };
        if tty.starts_with("/dev/pts/") {
            ttys.push(tty);
        }
    }
    ttys
}

fn terminal_tty_for_window(pid: u32, xid: u64) -> Option<PathBuf> {
    if !is_terminal_process(pid) {
        return None;
    }
    let mut windows = crate::x11::list_windows(Some(pid));
    windows.sort_by_key(|w| w.xid);
    let window_index = windows.iter().position(|w| w.xid == xid)?;
    let ttys = terminal_descendant_ttys(pid);
    ttys.get(window_index).cloned()
}

/// Type into a terminal window without touching X focus. Resolves the window's
/// pty, then borrows the emulator's master fd and writes to it (see
/// `crate::tty`). Returns `Ok(false)` when the target isn't a terminal we can
/// reach this way so the caller falls back to the generic XSendEvent path.
fn inject_terminal_input(pid: u32, xid: u64, text: &str) -> anyhow::Result<bool> {
    let Some(tty) = terminal_tty_for_window(pid, xid) else {
        return Ok(false);
    };
    // tty is `/dev/pts/<N>`; the emulator (pid) holds the master for the same N.
    let Some(ptn) = tty
        .file_name()
        .and_then(|s| s.to_str())
        .and_then(|s| s.parse::<u32>().ok())
    else {
        return Ok(false);
    };
    crate::tty::inject_via_master(pid, ptn, text)
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
            description: "Click against a target pid. **Prefer `element_index` over pixel \
                coordinates** — element_index works on backgrounded / hidden windows, surfaces \
                a stable handle, and tells you what you're clicking via the cached AT-SPI \
                element's role + label. Reach for `x, y` only when the target is a canvas / \
                custom-drawn surface that doesn't appear in the AT-SPI tree.\n\n\
                Provide either (window_id + x/y) or (pid + element_index). Routes via \
                XSendEvent (no focus steal). element_index cache is scoped per (pid, \
                window_id) and is replaced by the next get_window_state of the same window — \
                re-snapshot every turn before clicking.\n\n\
                After a zoom call, pass from_zoom=true to auto-translate zoom-image coords \
                back to full-window space.\n\n\
                button: \"left\" (default), \"right\", or \"middle\". Defaults to left so the \
                field is fully back-compat. X11: routes through XSendEvent ButtonPress/Release \
                with the matching button code. Native Wayland: only left-button is supported \
                via the virtual-pointer protocol — right/middle return an error rather than \
                silently degrading to left. `modifier` holds ctrl/shift/alt/super for the \
                click on X11. Native Wayland refuses modified pointer clicks until its input \
                protocol can carry keyboard modifier state.".into(),
            input_schema: json!({
                // `pid` is conditionally required (validated in code: needed for
                // window/element clicks, omitted for windowless scope="desktop"),
                // so it is NOT pinned in `required` — matches the click→[] canon
                // in cua_driver_core::tool_schema.
                "type":"object","required":[],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "x":{"type":"number"},
                    "y":{"type":"number"},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    // Shape matches the shared button_schema() canon (string +
                    // [left,right,middle]); kept inline to carry the Linux/Wayland
                    // back-compat prose the click button-schema test asserts on.
                    "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Default: \"left\" (legacy back-compat). X11: routed via ButtonPress/Release with the matching evdev code. Native Wayland: only left-button is supported via the virtual-pointer protocol; right/middle return an error."},
                    "count":{"type":"integer"},
                    "modifier": cua_driver_core::tool_schema::modifier_schema(),
                    "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let modifiers: Vec<String> = args.str_array("modifier");

        // ── Window-less screen-absolute branch (desktop target) ───────────────
        // x,y with NO pid and NO window_id → TRUE SCREEN pixels. Foreground,
        // desktop-scope click (the Linux peer of the Windows WindowFromPoint /
        // macOS global-HID path). The core registry has already enforced the
        // session policy; this branch validates the explicit action form.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if has_xy && !has_pid && !has_window_id {
            if args.get("scope").and_then(Value::as_str) != Some("desktop") {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id requires scope=\"desktop\"; \
                     use get_desktop_state to read true screen pixels first."
                        .to_string(),
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
            let button = parse_mouse_button(input.button.unwrap_or(ClickButton::Left).as_str());
            let sx = input.x as i32;
            let sy = input.y as i32;
            let n = input.count.unwrap_or(1) as usize;
            if n == 0 {
                return ToolResult::error("click.count must be at least 1.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            // Glide the agent-cursor overlay to the click point first (the macOS
            // / Windows desktop paths already do this). Without it the overlay
            // sits idle elsewhere while only the real pointer warps, so a viewer
            // sees the cursor "click somewhere else."
            reveal_pointer_action_for(&self.state, &cursor_id, f64::from(sx), f64::from(sy), true)
                .await;
            let r = tokio::task::spawn_blocking(move || {
                if crate::wayland::wayland_input_enabled() {
                    if !modifiers.is_empty() {
                        anyhow::bail!(
                            "modified desktop clicks are unavailable on native Wayland: \
                             the virtual-pointer route cannot carry keyboard modifier state"
                        );
                    }
                    crate::wayland::click_desktop(sx, sy, n as u32, button)
                } else {
                    let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    crate::input::send_click_xtest_desktop_with_modifiers(
                        sx,
                        sy,
                        button,
                        n,
                        &modifier_refs,
                    )
                }
            })
            .await;
            return match r {
                // Screen-absolute click — never driver-verifiable (no
                // read-back); the caller confirms via screenshot.
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent screen-absolute click at ({sx},{sy}) (desktop scope)."
                ))
                .with_structured(
                    json!({ "path": if crate::wayland::wayland_input_enabled() { "wayland_desktop" } else { "xtest_desktop" }, "verified": false, "effect": "unverifiable" }),
                ),
                Ok(Err(e)) => ToolResult::error(format!("desktop-scope click failed: {e}")),
                Err(e) => ToolResult::error(format!("task error: {e}")),
            };
        }

        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let count = args.u64_or("count", 1) as usize;
        // Surface 5: reject unknown buttons so a typo can't silently fall through
        // to a left-click. Empty string keeps back-compat with old clients.
        let button_str_raw = args.str_or("button", "left").to_lowercase();
        if !matches!(button_str_raw.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_str_raw}\" — expected one of left, right, middle."
            ));
        }
        let button_str = if button_str_raw.is_empty() {
            "left"
        } else {
            button_str_raw.as_str()
        };
        let button = parse_mouse_button(button_str);

        // Surface 6: element_token / element_index precedence resolution.
        // We resolve before the legacy `opt_u64("element_index")` branch
        // so a token-only call (no integer arg) still takes the element path.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg.map(|v| v as u32),
            "click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx_resolved: Option<usize> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                window_id_arg.or_else(|| window_id.map(|v| v as u64))
            }
            cua_driver_core::element_token::ResolvedElement::None => window_id_arg,
        };

        if let Some(idx) = elem_idx_resolved {
            let xid_hint = window_id_resolved;
            // Resolve the element's screen center + its window FIRST, so the
            // agent cursor glides to the target *before* the click fires —
            // matching the coordinate path below and the macOS/Windows backends.
            // Previously perform_action ran inside this spawn_blocking, so the
            // app updated before the cursor visibly arrived.
            let (xid, sx, sy) = tokio::task::spawn_blocking(move || -> (u64, f64, f64) {
                let (cx, cy) = element_screen_center(pid, idx).unwrap_or((0.0, 0.0));
                let xid = xid_hint
                    .or_else(|| {
                        crate::x11::list_windows(Some(pid))
                            .into_iter()
                            .next()
                            .map(|w| w.xid)
                    })
                    .unwrap_or(0);
                (xid, cx, cy)
            })
            .await
            .unwrap_or((0, 0.0, 0.0));
            if xid != 0 {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(xid),
                );
            }
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;

            // Chromium can execute a genuine AT-SPI action without focus. Try
            // that route before applying its background synthetic-input gate.
            if modifiers.is_empty() {
                let ax_result =
                    tokio::task::spawn_blocking(move || crate::atspi::perform_action(pid, idx))
                        .await;
                if let Ok(Ok((_action, suspected_noop))) = ax_result {
                    let mut structured = json!({
                        "path": "ax",
                        "verified": false,
                        "effect": if suspected_noop { "suspected_noop" } else { "unverifiable" },
                    });
                    if suspected_noop {
                        structured["escalation"] = non_ax_escalation();
                    }
                    return ToolResult::text(format!("Clicked element [{idx}] (pid {pid})."))
                        .with_structured(structured);
                }
            }
            if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
                return refusal;
            }

            // The AX route was unavailable. Fall back to a target-addressed
            // X11 event for toolkits that accept it.
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let (xid2, lx, ly) = resolve_element_local_coords(pid, idx, xid_hint)?;
                let modifier_refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                if crate::wayland::wayland_input_enabled() && !modifier_refs.is_empty() {
                    anyhow::bail!(
                        "modified element clicks are unavailable on native Wayland: \
                         the pointer route cannot carry keyboard modifier state"
                    );
                }
                // An explicit X11 foreground request needs the real XTest path
                // even for a plain click. Some native widgets (notably GTK
                // selectable rows) expose bounds but no AT-SPI Action and
                // ignore a targeted XSendEvent; limiting XTest to modified
                // clicks made those rows addressable but not selectable.
                if delivery.is_foreground() && !crate::wayland::wayland_input_enabled() {
                    crate::input::with_x11_foreground(xid2, 80, || {
                        crate::input::send_click_xtest_desktop_with_modifiers(
                            sx.round() as i32,
                            sy.round() as i32,
                            button,
                            count,
                            &modifier_refs,
                        )
                    })
                } else {
                    crate::input::send_click_with_modifiers(
                        xid2,
                        lx as i32,
                        ly as i32,
                        count,
                        button,
                        &modifier_refs,
                    )
                }
            })
            .await;
            return match result {
                // An element click is never driver-verifiable (no read-back) —
                // verified:false; the caller confirms via screenshot. `effect` is
                // the richer signal: a passive/role-mismatched AT-SPI actuation is
                // a likely no-op (→ cross to vision/pixel), otherwise the dispatch
                // was fine but unconfirmable.
                Ok(Ok(())) => {
                    let structured = json!({
                        "path": "x11_pixel",
                        "verified": false,
                        "effect": "unverifiable",
                    });
                    ToolResult::text(format!("Clicked element [{idx}] (pid {pid})."))
                        .with_structured(structured)
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI element click failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }

        // Coordinate-based path.
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        // Resolve the screen point the cursor glides to. Tool coordinates are
        // always window-local screenshot pixels; native Wayland translates
        // through compositor/AT-SPI geometry while X11 uses XTranslateCoordinates.
        let wayland_output_point = if crate::wayland::wayland_input_enabled() {
            Some(crate::wayland::window_local_to_output(
                xid,
                x.round() as i32,
                y.round() as i32,
            ))
        } else {
            None
        };
        let glide_target = if let Some((sx, sy)) = wayland_output_point {
            Some((sx as f64, sy as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y))
                .await
                .ok()
                .and_then(|r| r.ok())
        };
        if let Some((sx, sy)) = glide_target {
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
        }

        let (xi, yi) = (x as i32, y as i32);
        let (output_x, output_y) = wayland_output_point.unwrap_or((xi, yi));
        let cursor_id_for_task = cursor_id.clone();
        let modifiers_for_task = modifiers.clone();
        // delivery_mode: background (default) = no-focus-steal injection;
        // foreground = activate the target window (EWMH) first, then inject,
        // then restore prior active. Mirrors macOS/Windows.
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<&'static str> {
            if crate::wayland::wayland_input_enabled() {
                if !modifiers_for_task.is_empty() {
                    anyhow::bail!(
                        "modified coordinate clicks are unavailable on native Wayland: \
                         the pointer route cannot carry keyboard modifier state"
                    );
                }
                // Vision/pixel click on native Wayland. Mutter drops synthetic
                // virtual-pointer events (the `wayland::click` warp doesn't land),
                // so for a plain left single click resolve the screen pixel to the
                // covering accessible element and fire its action by
                // `element_index` — the coordinate-free path already verified
                // working. (x,y) are screen coords here, matching the frames in
                // `get_window_state`. Miss → fall through to the injection paths.
                if !delivery.is_foreground() && button == 1 && count == 1 {
                    if let Ok(Some(_)) =
                        crate::atspi::perform_action_at_screen_point(pid, xid, output_x, output_y)
                    {
                        return Ok("wayland_atspi");
                    }
                }
                if crate::wayland::is_inject_mode() {
                    crate::wayland::inject_click(pid, xid, x, y, count as u32, button)?;
                    return Ok("wayland_cua_compositor");
                }
                if !delivery.is_foreground() {
                    return Ok("background_unavailable");
                }
                // Native Wayland: focus+raise the target toplevel
                // (foreign-toplevel `activate`), then drive `count` virtual-pointer
                // button events. Wayland injection routes to the compositor focus.
                crate::wayland::with_target_foreground(pid, xid, || {
                    crate::wayland::click_focused(output_x, output_y, count as u32, button)
                })?;
                return Ok("wayland_activate");
            }
            // X11 injection. Tiered no-focus-steal delivery (background):
            //   1. Plain left single-click → AT-SPI doAction at that point.
            //   2. Right / middle / double click → real MPX uinput pointer + XI2
            //      shield grab (real Xorg only; skipped on Xvfb/Wayland).
            //   3. Fallback → synthetic XSendEvent.
            // Foreground skips the AT-SPI shortcut and does a real activated pixel
            // click (the agent's escalation when background didn't land).
            let inject = |fg: bool| -> anyhow::Result<&'static str> {
                if !fg && button == 1 && count == 1 && modifiers_for_task.is_empty() {
                    if let Ok(Some(_)) = crate::atspi::perform_action_at_point(pid, xi, yi) {
                        return Ok("x11_atspi");
                    }
                }
                if fg {
                    // Foreground: the window is already activated. Deliver a REAL
                    // XTest warp+button click. Synthetic XSendEvent button events
                    // are dropped by GTK/Qt/Chromium/Firefox, and the MPX uinput
                    // path needs /dev/uinput, which headless X servers
                    // (Xvfb/Xtigervnc) lack — so neither focuses the clicked
                    // widget. XTest is accepted as real input and gives the
                    // widget keyboard focus, so a following type lands.
                    if let Ok((sx, sy)) = window_local_to_screen(xid, xi as f64, yi as f64) {
                        let modifier_refs: Vec<&str> =
                            modifiers_for_task.iter().map(String::as_str).collect();
                        crate::input::send_click_xtest_desktop_with_modifiers(
                            sx.round() as i32,
                            sy.round() as i32,
                            button,
                            count,
                            &modifier_refs,
                        )?;
                        return Ok("x11_xtest_fg");
                    }
                }
                if modifiers_for_task.is_empty() {
                    x11_pixel_click_no_focus_steal(
                        &cursor_id_for_task,
                        xid,
                        xi,
                        yi,
                        button,
                        count,
                    )?;
                } else {
                    let modifier_refs: Vec<&str> =
                        modifiers_for_task.iter().map(String::as_str).collect();
                    x11_pixel_click_no_focus_steal_modifiers(
                        xid,
                        xi,
                        yi,
                        button,
                        count,
                        &modifier_refs,
                    )?;
                }
                Ok(if fg { "x11_pixel_fg" } else { "x11_pixel" })
            };
            if delivery.is_foreground() {
                crate::input::with_x11_foreground(xid, 80, || inject(true))
            } else {
                inject(false)
            }
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok("background_unavailable")) => {
                crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                )
            }
            // A pixel/coordinate click is never driver-verifiable (no read-back) —
            // verified:false, effect:"unverifiable"; the caller confirms via
            // screenshot. path reports the rung taken.
            Ok(Ok(path)) => ToolResult::text(format!(
                "✅ Clicked at ({x:.1}, {y:.1}) × {count} (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "path": path, "verified": false, "effect": "unverifiable" })),
            Ok(Err(e)) => linux_input_error(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── px-focus helper (keyboard family) ───────────────────────────────────────

/// px-focus for the keyboard family (type_text / press_key / hotkey): pixel-click
/// at (x,y) to establish real renderer focus before a keystroke — the *element px
/// action* form of a keyboard tool. Reuses ClickTool's exact coordinate
/// translation + delivery_mode so it lands on the same pixel a px-click would.
/// `Ok(())` on success; `Err(ToolResult)` short-circuits the caller.
///
/// Mirrors macOS `tools::focus_by_pixel`. Linux differences: `pid` is `u32`,
/// `window_id` is `u64` (the X11 XID / window id), and there is no `_session_id`
/// field — the Linux ClickTool resolves the agent cursor from `session` /
/// `cursor_id` only (`resolve_cursor_key`).
async fn focus_by_pixel(
    state: &Arc<ToolState>,
    pid: u32,
    window_id: Option<u64>,
    x: f64,
    y: f64,
    foreground: bool,
    session: Option<String>,
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
    if from_zoom {
        click_args["from_zoom"] = json!(true);
    }
    let focus = ClickTool {
        state: state.clone(),
    }
    .invoke(click_args)
    .await;
    if focus.is_error == Some(true) {
        return Err(ToolResult::error(format!(
            "focus pixel-click at ({x:.0},{y:.0}) failed."
        )));
    }
    // Brief settle so the renderer registers focus before the keystrokes.
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    Ok(())
}

/// Establish widget-local focus inside a nested-compositor target without
/// changing the compositor's focused toplevel. AX uses Component.GrabFocus;
/// PX sends a private per-surface left click at the requested local point.
async fn focus_nested_inject_target(
    pid: u32,
    window_id: u64,
    element_index: Option<usize>,
    pixel: Option<(f64, f64)>,
) -> Result<(), ToolResult> {
    if let Some(index) = element_index {
        return match tokio::task::spawn_blocking(move || crate::atspi::focus_element(pid, index))
            .await
        {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(ToolResult::error(format!(
                "AT-SPI Component.GrabFocus returned false for element {index}"
            ))),
            Ok(Err(error)) => Err(ToolResult::error(error.to_string())),
            Err(error) => Err(ToolResult::error(format!("Task error: {error}"))),
        };
    }
    if let Some((x, y)) = pixel {
        return match tokio::task::spawn_blocking(move || {
            crate::wayland::inject_click(pid, window_id, x, y, 1, 1)
        })
        .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => Err(ToolResult::error(error.to_string())),
            Err(error) => Err(ToolResult::error(format!("Task error: {error}"))),
        };
    }
    Ok(())
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
            description: "Type text to a window via XSendEvent (KeyPress/KeyRelease). No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["text"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "text":{"type":"string"},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    "x":{"type":"number","description":"Screenshot-pixel X of the field to type into — the element px action form. Pass x,y (no element_index) and the tool pixel-clicks there to establish real renderer focus, then types. Use for Chromium/Electron inputs the AX path can't reach. Read straight off the get_window_state PNG, same convention as click."},
                    "y":{"type":"number","description":"Screenshot-pixel Y of the field (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop") && args.get("pid").is_none() {
            let input = match parse_typed_projection::<TypeTextInput>("type_text", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let text =
                cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&input.text)
                    .into_owned();
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result = tokio::task::spawn_blocking(move || {
                if wayland {
                    crate::wayland::type_text_focused(&text)
                } else {
                    crate::input::send_type_text_xtest(&text)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text("Typed text into the focused desktop application.")
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Strip trailing agent-protocol closing tags — see
        // cua_driver_core::text_sanitize docs for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        // Surface 6: resolve element_token / element_index for the
        // optional pre-typing focus glide below. The token also carries
        // the window_id when supplied so the caller can omit window_id.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "type_text",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (resolved_elem_idx, resolved_window_id) = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element {
                element_index,
                window_id,
                ..
            } => (Some(*element_index), window_id.map(|v| v as u64)),
            cua_driver_core::element_token::ResolvedElement::None => (None, None),
        };
        let xid_opt = args.opt_u64("window_id").or(resolved_window_id);

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if resolved_elem_idx.is_none() {
            if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
                return refusal;
            }
        }

        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() != py.is_some() {
            return ToolResult::error("Pass both x and y to type_text, or neither.");
        }
        if px.is_some() && resolved_elem_idx.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to type_text, not both.",
            );
        }

        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            resolved_elem_idx,
            px.zip(py),
            true,
        )
        .await;

        let text_len = text.chars().count();
        // Native toolkit editables have a stronger focus-free route than raw
        // compositor keyboard injection. Keep Chromium/WebKit on real key events
        // because their accessibility bridges may echo a write that never reaches
        // renderer-owned state.
        if crate::wayland::is_inject_mode()
            && resolved_elem_idx.is_some()
            && !is_chromium_embedder(pid)
            && !is_webkitgtk_embedder(pid)
        {
            let idx = resolved_elem_idx.expect("checked above");
            let text_at = text.clone();
            let targeted = tokio::task::spawn_blocking(move || {
                crate::atspi::type_into_editable_at(pid, idx, &text_at)
            })
            .await;
            if let Ok(Ok(())) = targeted {
                return type_text_ax_result(pid, text_len, "via targeted AT-SPI");
            }
        }
        // The private nested compositor can target the owning Wayland client
        // directly. Establish widget-local focus first, without changing the
        // compositor's focused toplevel, so keys reach the addressed control.
        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, resolved_elem_idx, px.zip(py)).await
            {
                return error;
            }
            let text_w = text.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_type_text(pid, xid, &text_w)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (focus-free via cua-compositor)."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // ── px form: focus by pixel-click, then type into the now-focused element ──
        // Pass x,y (no element_index/token) for an *element px action*: pixel-click
        // the field to give the renderer the real keyboard focus the AT-SPI path
        // can't, then fall through to the focused-element type path below (it
        // escalates AT-SPI → key events and lands once focused). Reuses ClickTool's
        // exact coordinate translation + delivery_mode.
        if let (Some(cx), Some(cy)) = (px, py) {
            if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
                return refusal;
            }
            let from_zoom = args.bool_or("from_zoom", false);
            if let Err(e) = focus_by_pixel(
                &self.state,
                pid,
                Some(xid),
                cx,
                cy,
                delivery.is_foreground(),
                args.opt_str("session"),
                from_zoom,
            )
            .await
            {
                return e;
            }
            // resolved_elem_idx stays None → the type path below writes to the now-
            // focused element via the background key / AT-SPI rung.
        }

        if resolved_elem_idx.is_some() {
            if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
                return refusal;
            }
        }

        // Renderer EditableText writes can update the accessible value without
        // emitting the DOM input event. For an explicit foreground request on
        // native Wayland, focus the named field and send real keyboard input so
        // Chromium/WebKit observe the same event sequence as a user.
        if delivery.is_foreground()
            && crate::wayland::wayland_input_enabled()
            && (is_chromium_embedder(pid) || is_webkitgtk_embedder(pid))
        {
            if let Some(idx) = resolved_elem_idx {
                let text_w = text.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::wayland::with_target_foreground(pid, xid, || {
                        if !crate::atspi::focus_element(pid, idx)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {idx}"
                            );
                        }
                        crate::wayland::type_text_focused(&text_w)
                    })
                })
                .await;
                return match result {
                    Ok(Ok(())) => ToolResult::text(format!(
                        "Typed {text_len} character(s) (via Wayland virtual-keyboard)."
                    ))
                    .with_structured(type_text_structured(
                        "key_events",
                        text_len,
                        false,
                    )),
                    Ok(Err(error)) => ToolResult::error(error.to_string()),
                    Err(error) => ToolResult::error(format!("Task error: {error}")),
                };
            }
        }

        // AX addressing names one exact editable. Try this focus-free route
        // before native Wayland keyboard injection, which can only target the
        // compositor's globally focused surface.
        if let Some(idx) = resolved_elem_idx {
            let text_at = text.clone();
            let targeted = tokio::task::spawn_blocking(move || {
                crate::atspi::type_into_editable_at(pid, idx, &text_at)
            })
            .await;
            match targeted {
                Ok(Ok(())) => {
                    return type_text_ax_result(pid, text_len, "via targeted AT-SPI");
                }
                Ok(Err(_)) | Err(_)
                    if !delivery.is_foreground() && crate::wayland::wayland_input_enabled() =>
                {
                    return crate::input::delivery::background_unavailable_error(
                        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                    );
                }
                _ => {}
            }
        }

        // Native Wayland: keys go to the *focused* surface (no pid/window
        // targeting in the protocol). Type via the virtual-keyboard tool; pair
        // with a prior `click`/`activate` to focus the intended window.
        if crate::wayland::wayland_input_enabled() {
            if !delivery.is_foreground() {
                return crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                );
            }
            let text_w = text.clone();
            let idx = resolved_elem_idx;
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::with_target_foreground(pid, xid, || {
                    if let Some(idx) = idx {
                        if !crate::atspi::focus_element(pid, idx)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {idx}"
                            );
                        }
                    }
                    crate::wayland::type_text_focused(&text_w)
                })
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (via Wayland virtual-keyboard)."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Terminal short-circuit: when the target window's WM_CLASS marks it
        // as a terminal emulator (Ghostty / Alacritty / kitty / …), skip the
        // AT-SPI path entirely. Terminals expose an editable text area that
        // AT-SPI `insertText` would aim at, but the write never reaches the
        // pty, so the user sees "type acknowledged but nothing appeared".
        // Try pty-master injection first (most reliable), then XTest key
        // synthesis. Either way the structured response reports
        // `path: "key_events"` so callers can verify the route taken.
        let pid_is_terminal = is_terminal_process(pid);
        let wm_class_is_terminal =
            tokio::task::spawn_blocking(move || crate::terminal::is_terminal_window(xid))
                .await
                .unwrap_or(false);
        if pid_is_terminal || wm_class_is_terminal {
            let text_len = text.chars().count();
            let text_t = text.clone();
            let foreground = delivery.is_foreground();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<&'static str> {
                // pty-master injection is preferred — it skips the X event
                // queue entirely. Falls through to XTest if the terminal
                // isn't reachable that way (descendant pty unresolvable).
                if inject_terminal_input(pid, xid, &text_t)? {
                    return Ok("pty");
                }
                if foreground {
                    crate::input::with_x11_foreground(xid, 80, || {
                        crate::input::send_type_text_xtest(&text_t)
                    })?;
                    Ok("key_events_fg")
                } else {
                    Ok("background_unavailable")
                }
            })
            .await;
            return match result {
                Ok(Ok("background_unavailable")) => {
                    crate::input::delivery::background_unavailable_error(
                        crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                    )
                }
                Ok(Ok(path)) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (terminal emulator: pty/XTest key events)."
                ))
                .with_structured(type_text_structured(path, text_len, false)),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        // Foreground means the caller explicitly permits activation. Chromium
        // and WebKitGTK can acknowledge an accessibility write without
        // producing the renderer input event, so web embedders use real XTest
        // key events. Native toolkits keep their verifiable AT-SPI path below.
        if delivery.is_foreground() && (is_chromium_embedder(pid) || is_webkitgtk_embedder(pid)) {
            let text_f = text.clone();
            let idx = resolved_elem_idx;
            let result = tokio::task::spawn_blocking(move || {
                crate::input::with_x11_foreground(xid, 80, || {
                    if let Some(idx) = idx {
                        if !crate::atspi::focus_element(pid, idx)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {idx}"
                            );
                        }
                    }
                    crate::input::send_type_text_xtest(&text_f)
                })
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (via X11, delivery_mode=foreground)."
                ))
                .with_structured(type_text_structured(
                    "key_events_fg",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Prefer the focused widget — the element the user just clicked. If a
        // NON-editable input holds keyboard focus (a spreadsheet cell, a
        // terminal, a canvas), the focus-free AT-SPI editable search below would
        // grab the wrong field (e.g. gnumeric's name box instead of the selected
        // cell, or skip a terminal entirely), so synth-type into the focused
        // widget instead: terminals via pty injection, everything else via
        // XSendEvent to the focused window. A focused *editable* (Some(true)) or
        // nothing focused (None) falls through to the existing AT-SPI-first flow.
        let focus_kind = tokio::task::spawn_blocking(move || {
            crate::atspi::focused_is_editable(pid).ok().flatten()
        })
        .await
        .ok()
        .flatten();
        if focus_kind == Some(false) {
            if !delivery.is_foreground() {
                return crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                );
            }
            let text_f = text.clone();
            let result = tokio::task::spawn_blocking(move || {
                if inject_terminal_input(pid, xid, &text_f)? {
                    return Ok(());
                }
                // XTest (real input to the focused window), NOT XSendEvent: GTK/Qt
                // drop synthetic key events, so a spreadsheet cell / canvas would
                // stay empty. The click that gave this widget focus already put it
                // under the X input focus, so XTest-to-focus lands correctly.
                crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_type_text_xtest(&text_f)
                })
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) into the focused widget."
                ))
                .with_structured(type_text_structured(
                    "key_events",
                    text_len,
                    false,
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Try AT-SPI EditableText first (focus-free, works for Qt6/GTK4).
        let text_clone = text.clone();
        let atspi_result =
            tokio::task::spawn_blocking(move || crate::atspi::type_into_editable(pid, &text_clone))
                .await;

        match atspi_result {
            Ok(Ok(())) => {
                // AT-SPI succeeded — focus-free typing worked (Qt6, GTK4, etc.)!
                // Electron/Chromium can echo this write without the renderer
                // observing it, so the confirm is suppressed there (mirrors macOS).
                return type_text_ax_result(pid, text_len, "via AT-SPI");
            }
            _ => {
                // AT-SPI failed (no editable exposed). Qt5 doesn't expose widgets
                // when unfocused, so try the synthetic-focus workaround.
            }
        }

        // Qt5 workaround: send synthetic FocusIn to make Qt5's AT-SPI bridge
        // expose the widget tree, type via AT-SPI, then send FocusOut.
        // This doesn't change the X11 active window, so the test's focus check passes.
        let text_clone2 = text.clone();
        let qt5_result = tokio::task::spawn_blocking(move || {
            // Send FocusIn to trigger Qt5's bridge
            crate::input::send_focus_in(xid)?;
            std::thread::sleep(std::time::Duration::from_millis(100));

            // Try AT-SPI again now that widgets should be exposed
            let result = crate::atspi::type_into_editable(pid, &text_clone2);

            // Restore state with FocusOut
            crate::input::send_focus_out(xid)?;

            result
        })
        .await;

        match qt5_result {
            Ok(Ok(())) => {
                return type_text_ax_result(pid, text_len, "via AT-SPI with focus workaround");
            }
            _ => {
                // AT-SPI still didn't work. Fall back to X11 XSendEvent.
            }
        }

        // Track which path the final fallback chain took, so the
        // structured response stays honest. The closure can't borrow
        // a local mutably across `spawn_blocking`, so funnel the
        // decision through the success type instead.
        // delivery_mode: background (default) = focus-free AT-SPI / XSendEvent;
        // foreground = activate the window (EWMH), then synthesize REAL key
        // events to it via XTest — the escalation when background didn't land
        // (e.g. a GTK dialog whose widget ignores synthetic XSendEvent keys).
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<&'static str> {
            // Terminals: write to the pty master (focus-free, below the toolkit).
            if inject_terminal_input(pid, xid, &text)? {
                return Ok("key_events");
            }
            // GUI apps: X11 only routes keystrokes to the *focused* toplevel's
            // focused widget, so background XSendEvent typing doesn't land. Fill
            // the editable field via AT-SPI instead — focus-free and toolkit-
            // agnostic. Fall back to Tk send or XSendEvent when no a11y field is exposed.
            if crate::atspi::insert_text(pid, &text).unwrap_or(false) {
                return Ok("ax");
            }
            // Tk apps: use Tk's `send` command (no AT-SPI bridge, so AT-SPI above
            // returned false). This is the Tk-specific override, like CDP for Chromium.
            if crate::input::inject_tk_send(&text).unwrap_or(false) {
                return Ok("key_events");
            }
            if delivery.is_foreground() {
                crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_type_text_xtest(&text)
                })?;
                Ok("key_events_fg")
            } else {
                Ok("background_unavailable")
            }
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            // AT-SPI's boolean acknowledges the EditableText call; it is not a
            // fresh value readback. Keep the result unverifiable and apply the
            // stricter Chromium escalation where appropriate.
            Ok(Ok("ax")) => type_text_ax_result(
                pid,
                text_len,
                &format!("via X11, delivery_mode={mode_label}"),
            ),
            Ok(Ok("background_unavailable")) => {
                crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                )
            }
            Ok(Ok(path)) => ToolResult::text(format!(
                "Typed {text_len} character(s) (via X11, delivery_mode={mode_label})."
            ))
            .with_structured(type_text_structured(path, text_len, false)),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── press_key ─────────────────────────────────────────────────────────────────

pub struct PressKeyTool {
    state: Arc<ToolState>,
}
static PRESS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// Turn a modified `press_key` request into the chord an underlying keyboard
/// path expects when it takes a single key and no separate modifier list.
///
/// Returns `None` when there is nothing to promote, so the caller keeps the
/// plain single-key route. Dropping the modifiers instead is not a silent
/// no-op: the bare key still reaches the app and is typed as a character.
fn press_key_chord(mods: &[String], key: &str) -> Option<Vec<String>> {
    if mods.is_empty() {
        return None;
    }
    let mut chord = mods.to_vec();
    chord.push(key.to_owned());
    Some(chord)
}

#[cfg(test)]
mod press_key_tests {
    use super::press_key_chord;

    #[test]
    fn unmodified_press_stays_on_the_single_key_route() {
        assert_eq!(press_key_chord(&[], "return"), None);
    }

    #[test]
    fn modifiers_are_promoted_to_a_chord_in_order() {
        assert_eq!(
            press_key_chord(&["ctrl".to_owned()], "s"),
            Some(vec!["ctrl".to_owned(), "s".to_owned()])
        );
        assert_eq!(
            press_key_chord(&["ctrl".to_owned(), "shift".to_owned()], "t"),
            Some(vec!["ctrl".to_owned(), "shift".to_owned(), "t".to_owned()])
        );
    }

    #[test]
    fn the_requested_key_is_last_so_partition_modifiers_can_find_it() {
        // `wayland::hotkey` splits the array with `partition_modifiers`, which
        // takes the last non-modifier as the key. Appending keeps that true.
        let chord = press_key_chord(&["ctrl".to_owned()], "s").expect("chord");
        assert_eq!(chord.last().map(String::as_str), Some("s"));
    }
}

#[async_trait]
impl Tool for PressKeyTool {
    fn def(&self) -> &ToolDef {
        PRESS_DEF.get_or_init(|| ToolDef {
            name: "press_key".into(),
            description: "Press a key via XSendEvent to a window. No focus steal.".into(),
            input_schema: json!({
                "type":"object","required":["key"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "key":{"type":"string"},
                    "modifiers":{"type":"array","items":{"type":"string"}},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    "x":{"type":"number","description":"Screenshot-pixel X — the element px action form: pixel-click there to focus, then send the key. Use when the key must go to a Chromium/Electron surface the AX path can't focus. Pass with y, no element_index."},
                    "y":{"type":"number","description":"Screenshot-pixel Y (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop") && args.get("pid").is_none() {
            let input = match parse_typed_projection::<PressKeyInput>("press_key", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let key = input.key;
            let display = key.clone();
            let modifiers = input.modifiers.unwrap_or_default();
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result = tokio::task::spawn_blocking(move || {
                if wayland && modifiers.is_empty() {
                    crate::wayland::press_key_focused(&key)
                } else if wayland {
                    let mut keys = modifiers;
                    keys.push(key);
                    crate::wayland::hotkey_focused(&keys)
                } else {
                    let refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    crate::input::send_key_xtest(&key, &refs)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed desktop key '{display}'."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = args.u64_or("pid", 0) as u32;
        let key = match args.require_str("key") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let mods: Vec<String> = args.str_array("modifiers");

        // Surface 6: resolve the element token/index into both its owning
        // window and exact child. Foreground delivery establishes child focus
        // inside the verified top-level activation transaction; background
        // XSendEvent retains the historical direct-window path.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            element_index_arg,
            element_token_arg.as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg.map(|v| v as u32),
            "press_key",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let resolved_element_index = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let xid_opt = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                window_id_arg.or_else(|| window_id.map(|v| v as u64))
            }
            cua_driver_core::element_token::ResolvedElement::None => window_id_arg,
        };
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // ── px form: pixel-click to focus, then the key goes to the focused element ──
        // Reuses click's translation + delivery_mode; after it, deliver via the plain
        // background path (the focus-click already handled fronting when fg). Pass x,y
        // (no element_index) for Chromium/Electron surfaces the AX path can't focus.
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }

        if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }

        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() != py.is_some() {
            return ToolResult::error("Pass both x and y to press_key, or neither.");
        }
        if px.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to press_key, not both.",
            );
        }

        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            resolved_element_index,
            px.zip(py),
            false,
        )
        .await;

        // Nested cua-compositor addresses the owning Wayland client directly.
        // Preserve legacy modifiers by promoting the request to a chord.
        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, resolved_element_index, px.zip(py)).await
            {
                return error;
            }
            let result = match press_key_chord(&mods, &key) {
                None => {
                    let key_w = key.clone();
                    tokio::task::spawn_blocking(move || {
                        crate::wayland::inject_press_key(pid, xid, &key_w)
                    })
                    .await
                }
                Some(chord) => {
                    tokio::task::spawn_blocking(move || {
                        crate::wayland::inject_hotkey(pid, xid, &chord)
                    })
                    .await
                }
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed key '{key}' (focus-free via cua-compositor)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let px_target = {
            if let (Some(cx), Some(cy)) = (px, py) {
                let from_zoom = args.bool_or("from_zoom", false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    Some(xid),
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    from_zoom,
                )
                .await
                {
                    return e;
                }
                Some((cx.round() as i32, cy.round() as i32))
            } else {
                None
            }
        };

        // Native Wayland: send the key to the focused surface via virtual-keyboard.
        // `wayland::press_key` carries no modifier list, so a modified request has
        // to become a chord here — otherwise the modifiers are dropped and the
        // bare key is typed as a character (Ctrl+S inserts a literal "s").
        if crate::wayland::wayland_input_enabled() {
            let key_w = key.clone();
            let chord = press_key_chord(&mods, &key);
            let idx = resolved_element_index;
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::with_target_foreground(pid, xid, || {
                    if let Some(idx) = idx {
                        if !crate::atspi::focus_element(pid, idx)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {idx}"
                            );
                        }
                    }
                    match chord {
                        Some(keys) => crate::wayland::hotkey_focused(&keys),
                        None => crate::wayland::press_key_focused(&key_w),
                    }
                })
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed key '{key}' (via Wayland virtual-keyboard)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let key_for_task = key.clone();
        // Foreground delivery is one atomic activate-and-XTest transaction.
        // A preceding PX click establishes internal widget focus, but that
        // click restores the prior top-level before returning.
        let deliver_fg = delivery.is_foreground();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if resolved_element_index.is_none()
                && mods.is_empty()
                && key_for_task.eq_ignore_ascii_case("enter")
            {
                if inject_terminal_input(pid, xid, "\n")? {
                    return Ok(());
                }
            }
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            // foreground: activate the window first, then inject a REAL key via
            // XTest. Synthetic XSendEvent keys (`send_key`) are dropped by
            // GTK/Qt/Chromium/Firefox, so the foreground rung must use XTest —
            // it delivers to the now-focused window. background = direct
            // XSendEvent (no focus steal) for apps that accept it.
            if deliver_fg {
                return crate::input::with_x11_foreground(xid, 80, || {
                    if let Some(element_index) = resolved_element_index {
                        if !crate::atspi::focus_element(pid, element_index)? {
                            anyhow::bail!(
                                "AT-SPI Component.GrabFocus returned false for element {element_index}"
                            );
                        }
                    }
                    crate::input::send_key_xtest(&key_for_task, &m)
                });
            }
            if let Some(element_index) = resolved_element_index {
                if !crate::atspi::focus_element(pid, element_index)? {
                    anyhow::bail!(
                        "AT-SPI Component.GrabFocus returned false for element {element_index}"
                    );
                }
            }
            if let Some((x, y)) = px_target {
                crate::input::send_key_at(xid, x, y, &key_for_task, &m)
            } else {
                crate::input::send_key(xid, &key_for_task, &m)
            }
        })
        .await;
        let mode_label = if deliver_fg {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => {
                ToolResult::text(format!("Pressed key '{key}' (delivery_mode={mode_label})."))
                    .with_structured(json!({ "verified": false, "delivery_mode": mode_label }))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── hotkey ────────────────────────────────────────────────────────────────────

fn is_modifier(k: &str) -> bool {
    matches!(
        k.to_lowercase().as_str(),
        "ctrl"
            | "control"
            | "shift"
            | "alt"
            | "super"
            | "meta"
            | "cmd"
            | "command"
            | "win"
            | "windows"
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
            description: "Press a combination of keys simultaneously, e.g. [\"ctrl\",\"c\"] for Copy. \
                Sent via XSendEvent directly to the target pid; target does NOT need to be frontmost.".into(),
            input_schema: json!({
                "type":"object","required":["keys"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "keys":{"type":"array","items":{"type":"string"},"minItems":2,
                        "description":"Modifier(s) + one non-modifier key, e.g. [\"ctrl\",\"c\"]."},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    "x":{"type":"number","description":"Screenshot-pixel X — the element px action form: pixel-click there to focus, then send the combo (so e.g. Ctrl+V pastes into that field). Pass with y. Use for Chromium/Electron surfaces the background combo can't reach."},
                    "y":{"type":"number","description":"Screenshot-pixel Y (see x)."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.opt_str("scope").as_deref() == Some("desktop") && args.get("pid").is_none() {
            let input = match parse_typed_projection::<HotkeyInput>("hotkey", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let keys = input.keys;
            if keys.len() < 2 {
                return ToolResult::error("hotkey.keys must contain at least two keys.")
                    .with_structured(json!({ "code": "invalid_arguments" }));
            }
            let modifiers: Vec<String> = keys
                .iter()
                .filter(|key| is_modifier(key))
                .cloned()
                .collect();
            let Some(key) = keys.iter().rev().find(|key| !is_modifier(key)).cloned() else {
                return ToolResult::error("keys must include at least one non-modifier key.");
            };
            let display = keys.join("+");
            position_named_session_keyboard_cursor(&self.state, &args, 0, 0, None, None, false)
                .await;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_focused" } else { "xtest" };
            let result = tokio::task::spawn_blocking(move || {
                if wayland {
                    crate::wayland::hotkey_focused(&keys)
                } else {
                    let refs: Vec<&str> = modifiers.iter().map(String::as_str).collect();
                    crate::input::send_key_xtest(&key, &refs)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed desktop hotkey {display}."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = args.u64_or("pid", 0) as u32;
        let window_id_arg = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|value| value as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            element_index_arg,
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            window_id_arg.map(|value| value as u32),
            "hotkey",
        ) {
            Ok(resolved) => resolved,
            Err(error) => return error,
        };
        let resolved_element_index = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let xid_opt = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => {
                window_id_arg.or_else(|| window_id.map(|value| value as u64))
            }
            cua_driver_core::element_token::ResolvedElement::None => window_id_arg,
        };

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        // Parse keys array (preferred) or fall back to legacy key+modifiers.
        let (key, mods) = if let Some(arr) = args.get("keys").and_then(|v| v.as_array()) {
            let keys: Vec<String> = arr
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect();
            let modifiers: Vec<String> = keys.iter().filter(|k| is_modifier(k)).cloned().collect();
            let non_mods: Vec<String> = keys.iter().filter(|k| !is_modifier(k)).cloned().collect();
            if non_mods.is_empty() {
                return ToolResult::error("keys must include at least one non-modifier key.");
            }
            (non_mods.last().unwrap().clone(), modifiers)
        } else if let Some(k) = args.opt_str("key") {
            let mods: Vec<String> = args.str_array("modifiers");
            (k, mods)
        } else {
            return ToolResult::error(
                "Provide 'keys' array (e.g. [\"ctrl\",\"c\"]) or 'key'+'modifiers' parameters.",
            );
        };

        let key_display = format!("{}+{}", mods.join("+"), key);
        let key_for_wayland = key.clone();
        let mods_for_wayland = mods.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);

        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_webkit_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_keyboard_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }

        let px = args.get("x").and_then(|value| value.as_f64());
        let py = args.get("y").and_then(|value| value.as_f64());
        if px.is_some() != py.is_some() {
            return ToolResult::error("Pass both x and y to hotkey, or neither.");
        }
        if px.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to hotkey, not both.",
            );
        }

        position_named_session_keyboard_cursor(
            &self.state,
            &args,
            pid,
            xid,
            resolved_element_index,
            px.zip(py),
            false,
        )
        .await;

        if crate::wayland::is_inject_mode() {
            if let Err(error) =
                focus_nested_inject_target(pid, xid, resolved_element_index, px.zip(py)).await
            {
                return error;
            }
            let mut chord = mods.clone();
            chord.push(key.clone());
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_hotkey(pid, xid, &chord)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed hotkey '{key_display}' (focus-free via cua-compositor)."
                )),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        // Chromium renderer shortcuts need DOM focus established by a real
        // pointer event. Component.GrabFocus alone can report success while
        // the renderer still drops the virtual-keyboard chord.
        if delivery.is_foreground()
            && crate::wayland::wayland_input_enabled()
            && is_chromium_embedder(pid)
        {
            if let Some(element_index) = resolved_element_index {
                let coordinates = tokio::task::spawn_blocking(move || {
                    resolve_element_local_coords(pid, element_index, Some(xid))
                })
                .await;
                let (_, x, y) = match coordinates {
                    Ok(Ok(value)) => value,
                    Ok(Err(error)) => return ToolResult::error(error.to_string()),
                    Err(error) => return ToolResult::error(format!("Task error: {error}")),
                };
                if let Err(error) = focus_by_pixel(
                    &self.state,
                    pid,
                    Some(xid),
                    x,
                    y,
                    true,
                    args.opt_str("session"),
                    false,
                )
                .await
                {
                    return error;
                }
            }
        }

        if let Some(element_index) = resolved_element_index {
            let focused = tokio::task::spawn_blocking(move || {
                crate::atspi::focus_element(pid, element_index)
            })
            .await;
            match focused {
                Ok(Ok(true)) => {}
                Ok(Ok(false)) => {
                    return ToolResult::error(format!(
                        "AT-SPI Component.GrabFocus returned false for element {element_index}"
                    ))
                }
                Ok(Err(error)) => return ToolResult::error(error.to_string()),
                Err(error) => return ToolResult::error(format!("Task error: {error}")),
            }
        }

        // ── px form: pixel-click to focus, then the combo acts on the focused field ──
        // (e.g. Ctrl+V into a Chromium input). Reuses click's translation +
        // delivery_mode; after it, deliver the combo via the plain background path
        // (the focus-click already fronted when fg).
        let px_target = {
            if let (Some(cx), Some(cy)) = (px, py) {
                let from_zoom = args.bool_or("from_zoom", false);
                if let Err(e) = focus_by_pixel(
                    &self.state,
                    pid,
                    Some(xid),
                    cx,
                    cy,
                    delivery.is_foreground(),
                    args.opt_str("session"),
                    from_zoom,
                )
                .await
                {
                    return e;
                }
                Some((cx.round() as i32, cy.round() as i32))
            } else {
                None
            }
        };
        let deliver_fg = delivery.is_foreground();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::wayland_input_enabled() {
                // Native Wayland: route the modifier combo through wtype's
                // -M/-k/-m sequence — the closest equivalent to the X11
                // state-mask path. window_id is irrelevant once focused.
                let mut combo: Vec<String> = mods_for_wayland.clone();
                combo.push(key_for_wayland.clone());
                return crate::wayland::hotkey(xid, &combo);
            }
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            // foreground: activate the target first, then inject the accelerator
            // as REAL key events via XTest. Synthetic XSendEvent keys
            // (`send_key`) are dropped by GTK/Qt/Chromium/Firefox, so the
            // foreground rung must use XTest, which reaches the focused window.
            if deliver_fg {
                return crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_key_xtest(&key, &m)
                });
            }
            if let Some((x, y)) = px_target {
                crate::input::send_key_at(xid, x, y, &key, &m)
            } else {
                crate::input::send_key(xid, &key, &m)
            }
        })
        .await;
        let mode_label = if deliver_fg {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Pressed {key_display} on pid {pid} (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── set_value ─────────────────────────────────────────────────────────────────

pub struct SetValueTool {
    state: Arc<ToolState>,
}
static SV_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetValueTool {
    fn def(&self) -> &ToolDef {
        SV_DEF.get_or_init(|| ToolDef {
            name: "set_value".into(),
            description: "Set value of an AT-SPI element via SetValue action.".into(),
            input_schema: json!({
                "type":"object","required":["pid","value"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer","description":"Required when element_index is used; optional when element_token is supplied (the token carries it)."},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    "value":{"type":"string"}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let value = match args.require_str("value") {
            Ok(v) => v,
            Err(e) => return e,
        };
        // Surface 6: element_token / element_index precedence resolution.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "set_value",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (idx, resolved_window_id) = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element {
                element_index,
                window_id,
                ..
            } => (*element_index, window_id.map(u64::from)),
            cua_driver_core::element_token::ResolvedElement::None => return ToolResult::error(
                "set_value requires element_index or element_token to address the target element.",
            ),
        };
        let value_for_task = value.clone();
        let xid = args
            .opt_u64("window_id")
            .or(resolved_window_id)
            .unwrap_or(0);
        position_named_session_keyboard_cursor(&self.state, &args, pid, xid, Some(idx), None, true)
            .await;
        let result =
            tokio::task::spawn_blocking(move || crate::atspi::set_value(pid, idx, &value_for_task))
                .await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Set value of element [{idx}] to '{value}'.")),
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
            description: "Scroll the target pid's focused region via XSendEvent Button4/5. \
                direction required; by defaults to line, amount defaults to 3.".into(),
            input_schema: json!({
                // `pid` is conditionally required (validated in code), so only
                // `direction` is pinned — matches the scroll→["direction"] canon
                // in cua_driver_core::tool_schema.
                "type":"object","required":["direction"],"properties":{
                    "session": cua_driver_core::tool_schema::session_schema(),
                    "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                    "pid":{"type":"integer"},
                    "direction":{"type":"string","enum":["up","down","left","right"]},
                    "by":{"type":"string","enum":["line","page"]},
                    "amount":{"type":"integer","minimum":1,"maximum":50},
                    "window_id":{"type":"integer"},
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
                    "element_token": cua_driver_core::tool_schema::element_token_schema(),
                    "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                    "x":{"type":"number","description":"Window-local screenshot-pixel X of the scroll target. Pass with y and without element_index."},
                    "y":{"type":"number","description":"Window-local screenshot-pixel Y of the scroll target. Pass with x and without element_index."},
                    "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<ScrollInput>("scroll", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let direction = input.direction.as_str().to_owned();
            let x = input.x.round() as i32;
            let y = input.y.round() as i32;
            let amount = input.amount.unwrap_or(3).clamp(1, 50) as usize;
            let display = direction.clone();
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_desktop" } else { "xtest" };
            if named_session_cursor_key(&args).is_some() {
                reveal_pointer_action_for(
                    &self.state,
                    &cursor_id,
                    f64::from(x),
                    f64::from(y),
                    false,
                )
                .await;
            }
            let result = tokio::task::spawn_blocking(move || {
                if wayland {
                    crate::wayland::scroll_desktop(x, y, &direction, amount as u32)
                } else {
                    crate::input::send_scroll_xtest_desktop(x, y, &direction, amount)
                }
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Scrolled desktop {display} × {amount}."))
                    .with_structured(
                        json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                    ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let direction = match args.require_str("direction") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let amount = args.u64_or("amount", 3).clamp(1, 50) as usize;
        // Surface 6: resolve element_token / element_index. The Linux
        // scroll implementation today doesn't actually pre-focus the
        // element (X11 scroll buttons go to the window root), but the
        // token still needs to be accepted + validated so a stale
        // token surfaces an error instead of silently no-op'ing.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "scroll",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let xid_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => window_id
                .map(|v| v as u64)
                .or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };

        // Resolve XID: use window_id if given, else first window for pid.
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                            "No windows found for pid {pid}. Provide window_id."
                        ))
                    }
                }
            }
        };

        let pixel_target = match (
            args.get("x").and_then(|value| value.as_f64()),
            args.get("y").and_then(|value| value.as_f64()),
        ) {
            (Some(x), Some(y)) => {
                // Pixel targets use the latest screenshot's coordinate frame.
                // Apply the same buffer-to-window ratio as click/drag before
                // positioning either the agent cursor or the input device.
                let ratio = self.state.resize_registry.ratio(pid).unwrap_or(1.0);
                Some((x * ratio, y * ratio))
            }
            (None, None) => None,
            _ => return ToolResult::error("Pass both x and y to pixel-target scroll."),
        };
        let resolved_element_index = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        if pixel_target.is_some() && resolved_element_index.is_some() {
            return ToolResult::error(
                "Pass either element_index (ax) or x,y (px) to scroll, not both.",
            );
        }

        if named_session_cursor_key(&args).is_some() {
            let visual_target = tokio::task::spawn_blocking(move || {
                explicit_keyboard_cursor_target(pid, xid, resolved_element_index, pixel_target)
                    .or_else(|| keyboard_window_center(xid))
            })
            .await
            .ok()
            .flatten();
            if let Some((sx, sy)) = visual_target {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::PinAbove(xid),
                );
                reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, false).await;
            }
        }

        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } =
            &resolved
        {
            let idx = *element_index;
            // WebKitGTK acknowledges the AT-SPI scroll action without moving
            // the DOM scroller. On native Wayland, use a real compositor wheel
            // event at the resolved element instead of reporting a silent
            // success. Chromium's AT-SPI scroll path remains effective.
            if !(crate::wayland::wayland_input_enabled() && is_webkitgtk_embedder(pid)) {
                let direction_for_ax = direction.clone();
                let ax_result = tokio::task::spawn_blocking(move || {
                    crate::atspi::scroll_element(pid, idx, &direction_for_ax, amount)
                })
                .await;
                if matches!(ax_result, Ok(Ok(()))) {
                    let mode = if delivery.is_foreground() {
                        "foreground"
                    } else {
                        "background"
                    };
                    return ToolResult::text(format!(
                        "Scrolled {direction} {amount} ticks via AT-SPI (delivery_mode:{mode})."
                    ))
                    .with_structured(json!({
                        "path": "atspi",
                        "verified": false,
                        "delivery_mode": mode
                    }));
                }
            }
        }

        if crate::wayland::is_inject_mode() {
            let Some((x, y)) = pixel_target else {
                return crate::input::delivery::background_unavailable_error(
                    crate::input::delivery::BackgroundUnavailable::FocusedInputOnly,
                );
            };
            let direction_for_inject = direction.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::inject_scroll(pid, xid, x, y, &direction_for_inject, amount as u32)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks (focus-free via cua-compositor)."
                ))
                .with_structured(json!({
                    "verified": false,
                    "delivery_mode": if delivery.is_foreground() { "foreground" } else { "background" },
                    "route": "cua_compositor_inject"
                })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        if crate::wayland::wayland_input_enabled() {
            if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, false) {
                return refusal;
            }
            let direction_for_wayland = direction.clone();
            let local_point = match (pixel_target, &resolved) {
                (Some(point), _) => Some(point),
                (
                    None,
                    cua_driver_core::element_token::ResolvedElement::Element {
                        element_index, ..
                    },
                ) => {
                    let idx = *element_index;
                    match tokio::task::spawn_blocking(move || {
                        resolve_element_local_coords(pid, idx, Some(xid))
                    })
                    .await
                    {
                        Ok(Ok((_resolved_xid, x, y))) => Some((x, y)),
                        Ok(Err(error)) => {
                            return ToolResult::error(format!(
                                "Could not resolve scroll element [{idx}] coordinates: {error}"
                            ))
                        }
                        Err(error) => {
                            return ToolResult::error(format!(
                                "Scroll element coordinate task failed: {error}"
                            ))
                        }
                    }
                }
                (None, cua_driver_core::element_token::ResolvedElement::None) => None,
            };
            let output_point = local_point.map(|(x, y)| {
                crate::wayland::window_local_to_output(xid, x.round() as i32, y.round() as i32)
            });
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::scroll_at(xid, output_point, &direction_for_wayland, amount as u32)
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Scrolled {direction} {amount} ticks (delivery_mode=foreground)."
                ))
                .with_structured(json!({ "verified": false, "delivery_mode": "foreground" })),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }

        // An element-addressed scroll must land over the element. The old
        // fallback used (0, 0) in the window, which can report success while
        // Chromium/GTK routes the wheel to the toplevel instead of the
        // requested scroll region.
        let element_point = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                let idx = *element_index;
                match tokio::task::spawn_blocking(move || {
                    resolve_element_local_coords(pid, idx, Some(xid))
                })
                .await
                {
                    Ok(Ok((_resolved_xid, local_x, local_y))) => {
                        let screen = window_local_to_screen(xid, local_x, local_y).ok();
                        Some(((local_x, local_y), screen))
                    }
                    Ok(Err(e)) => {
                        return ToolResult::error(format!(
                            "Could not resolve scroll element [{idx}] coordinates: {e}"
                        ))
                    }
                    Err(e) => {
                        return ToolResult::error(format!(
                            "Scroll element coordinate task failed: {e}"
                        ))
                    }
                }
            }
            cua_driver_core::element_token::ResolvedElement::None => pixel_target.map(|local| {
                let screen = window_local_to_screen(xid, local.0, local.1).ok();
                (local, screen)
            }),
        };

        // X11 scroll buttons: 4=up, 5=down, 6=left, 7=right
        // Note: "page" scroll is still per-click on X11; send more ticks for page.
        let button: u8 = match direction.as_str() {
            "up" => 4,
            "left" => 6,
            "right" => 7,
            _ => 5,
        };
        let cursor_id_for_task = cursor_id.clone();
        let direction_for_wayland = direction.clone();
        let amount_u32 = amount as u32;
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::wayland_input_enabled() {
                return crate::wayland::scroll(xid, &direction_for_wayland, amount_u32);
            }
            // foreground: activate the window, then scroll, then restore — for
            // surfaces that only route wheel events to the active window.
            let x11_scroll = || -> anyhow::Result<()> {
                // X11: synthetic Button4-7 XSendEvents are dropped by XInput2
                // toolkits (GTK never scrolls). On a real Xorg host, drive a real
                // wheel detent through the MPX uinput pointer over the window's
                // center — libinput turns it into the XI2 smooth-scroll GTK reads —
                // without stealing focus. Falls back to the legacy XSendEvent
                // Button4-7 path on Xvfb / unsupported servers.
                // Button → axis/sign: 4=up(+v) 5=down(-v) 6=left(-h) 7=right(+h).
                if crate::input::real_pointer_input_available() {
                    let pointer_point = element_point
                        .and_then(|(_, screen)| screen)
                        .map(|(x, y)| (x as i32, y as i32))
                        .or_else(|| window_screen_center(xid).ok());
                    if let Some((cx, cy)) = pointer_point {
                        let horizontal = matches!(button, 6 | 7);
                        let ticks = match button {
                            4 | 7 => amount as i32,
                            _ => -(amount as i32), // 5 (down) and 6 (left)
                        };
                        match crate::input::send_virtual_pointer_scroll(
                            &cursor_id_for_task,
                            &crate::input::VirtualPointerScroll {
                                target_window: xid,
                                x: cx,
                                y: cy,
                                horizontal,
                                ticks,
                            },
                        ) {
                            Ok(()) => return Ok(()),
                            Err(error) if crate::input::is_uinput_unavailable(&error) => {
                                return Err(error)
                            }
                            Err(e) => tracing::warn!("MPX scroll fell back to XSendEvent: {e}"),
                        }
                    }
                }
                let (local_x, local_y) =
                    element_point.map(|(local, _)| local).unwrap_or((0.0, 0.0));
                crate::input::send_click(xid, local_x as i32, local_y as i32, amount, button)
            };
            if delivery.is_foreground() {
                let point = element_point
                    .and_then(|(_, screen)| screen)
                    .or_else(|| {
                        window_screen_center(xid)
                            .ok()
                            .map(|(x, y)| (x as f64, y as f64))
                    })
                    .ok_or_else(|| anyhow::anyhow!("could not resolve foreground scroll point"))?;
                crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_click_xtest_desktop(
                        point.0 as i32,
                        point.1 as i32,
                        button,
                        amount,
                    )
                })
            } else {
                x11_scroll()
            }
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Scrolled {direction} {amount} ticks (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => linux_input_error(e),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// `ScreenshotTool` and `ScreenshotCompatTool` removed in PR #1692 — see the
// matching note in platform-windows/src/tools/impl_.rs. `get_window_state` is
// the canonical screenshot path (it always returns a screenshot now); the
// underlying capture machinery (XGetImage / `import` shell-out / etc.)
// stays reachable through GetWindowStateTool.

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
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }
        // Surface 6: element_token / element_index precedence.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "double_click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx_resolved = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => args
                .opt_u64("window_id")
                .or_else(|| window_id.map(|v| v as u64)),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if let Some(idx) = elem_idx_resolved {
            let xid_hint = window_id_resolved;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            })
            .await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
                    {
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(xid),
                        );
                        reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
                    }
                    let lxi = lx as i32;
                    let lyi = ly as i32;
                    let wayland_point = crate::wayland::wayland_input_enabled()
                        .then(|| crate::wayland::window_local_to_output(xid, lxi, lyi));
                    let cursor_id_for_task = cursor_id.clone();
                    let click_result = tokio::task::spawn_blocking(move || {
                        if crate::wayland::is_inject_mode() {
                            return crate::wayland::inject_click(pid, xid, lx, ly, 2, 1);
                        }
                        if crate::wayland::wayland_input_enabled() {
                            let (output_x, output_y) = wayland_point.unwrap_or((lxi, lyi));
                            return crate::wayland::click(xid, output_x, output_y, 2, 1);
                        }
                        if delivery.is_foreground() {
                            return crate::input::with_x11_foreground(xid, 80, || {
                                let (sx, sy) = window_local_to_screen(xid, lxi as f64, lyi as f64)?;
                                crate::input::send_click_xtest_desktop(
                                    sx.round() as i32,
                                    sy.round() as i32,
                                    1,
                                    2,
                                )
                            });
                        }
                        x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, lxi, lyi, 1, 2)
                    })
                    .await;
                    match click_result {
                        Ok(Ok(())) => {
                            ToolResult::text(format!("✅ Double-clicked element [{idx}]."))
                        }
                        Ok(Err(e)) => linux_input_error(e),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match window_id_resolved {
            Some(v) => v,
            None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        let wayland_output_point = if crate::wayland::wayland_input_enabled() {
            Some(crate::wayland::window_local_to_output(
                xid,
                x.round() as i32,
                y.round() as i32,
            ))
        } else {
            None
        };
        let glide_target = if let Some((sx, sy)) = wayland_output_point {
            Some((sx as f64, sy as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y))
                .await
                .ok()
                .and_then(|r| r.ok())
        };
        if let Some((sx, sy)) = glide_target {
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
        }
        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_inject_mode() {
                return crate::wayland::inject_click(pid, xid, x, y, 2, 1);
            }
            if crate::wayland::wayland_input_enabled() {
                let (output_x, output_y) = wayland_output_point.unwrap_or((xi, yi));
                return crate::wayland::click(xid, output_x, output_y, 2, 1);
            }
            if delivery.is_foreground() {
                return crate::input::with_x11_foreground(xid, 80, || {
                    // Real XTest double-click at the screen point — synthetic
                    // XSendEvent button events are dropped by GTK/Qt and the MPX
                    // uinput path needs /dev/uinput (absent on Xvfb/Xtigervnc), so
                    // neither lands. Mirrors the single-click foreground path.
                    if let Ok((sx, sy)) = window_local_to_screen(xid, xi as f64, yi as f64) {
                        crate::input::send_click_xtest_desktop(
                            sx.round() as i32,
                            sy.round() as i32,
                            1,
                            2,
                        )?;
                        return Ok(());
                    }
                    x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 1, 2)
                });
            }
            x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 1, 2)
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Double-clicked at ({x:.1}, {y:.1}) (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => linux_input_error(e),
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
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index": cua_driver_core::tool_schema::element_index_schema(),
                "element_token": cua_driver_core::tool_schema::element_token_schema(),
                "snapshot_id": cua_driver_core::tool_schema::snapshot_id_schema(),
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }
        // Surface 6: element_token / element_index precedence.
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            args.opt_u64("element_index").map(|v| v as usize),
            args.opt_str("element_token").as_deref(),
            args.opt_str("snapshot_id").as_deref(),
            args.opt_u64("window_id").map(|v| v as u32),
            "right_click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx_resolved = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => {
                Some(*element_index)
            }
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } => args
                .opt_u64("window_id")
                .or_else(|| window_id.map(|v| v as u64)),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if let Some(idx) = elem_idx_resolved {
            let xid_hint = window_id_resolved;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            })
            .await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
                    {
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(xid),
                        );
                        reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
                    }
                    let lxi = lx as i32;
                    let lyi = ly as i32;
                    let wayland_point = crate::wayland::wayland_input_enabled()
                        .then(|| crate::wayland::window_local_to_output(xid, lxi, lyi));
                    let cursor_id_for_task = cursor_id.clone();
                    let click_result = tokio::task::spawn_blocking(move || {
                        if crate::wayland::is_inject_mode() {
                            return crate::wayland::inject_click(pid, xid, lx, ly, 1, 3);
                        }
                        if crate::wayland::wayland_input_enabled() {
                            let (output_x, output_y) = wayland_point.unwrap_or((lxi, lyi));
                            return crate::wayland::click(xid, output_x, output_y, 1, 3);
                        }
                        if delivery.is_foreground() {
                            return crate::input::with_x11_foreground(xid, 80, || {
                                let (sx, sy) = window_local_to_screen(xid, lxi as f64, lyi as f64)?;
                                crate::input::send_click_xtest_desktop(
                                    sx.round() as i32,
                                    sy.round() as i32,
                                    3,
                                    1,
                                )
                            });
                        }
                        x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, lxi, lyi, 3, 1)
                    })
                    .await;
                    match click_result {
                        Ok(Ok(())) => {
                            ToolResult::text(format!("✅ Right-clicked element [{idx}]."))
                        }
                        Ok(Err(e)) => linux_input_error(e),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match window_id_resolved {
            Some(v) => v,
            None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        if from_zoom {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        let wayland_output_point = if crate::wayland::wayland_input_enabled() {
            Some(crate::wayland::window_local_to_output(
                xid,
                x.round() as i32,
                y.round() as i32,
            ))
        } else {
            None
        };
        let glide_target = if let Some((sx, sy)) = wayland_output_point {
            Some((sx as f64, sy as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y))
                .await
                .ok()
                .and_then(|r| r.ok())
        };
        if let Some((sx, sy)) = glide_target {
            reveal_pointer_action_for(&self.state, &cursor_id, sx, sy, true).await;
        }
        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_inject_mode() {
                return crate::wayland::inject_click(pid, xid, x, y, 1, 3);
            }
            if crate::wayland::wayland_input_enabled() {
                let (output_x, output_y) = wayland_output_point.unwrap_or((xi, yi));
                return crate::wayland::click(xid, output_x, output_y, 1, 3);
            }
            if delivery.is_foreground() {
                return crate::input::with_x11_foreground(xid, 80, || {
                    // Real XTest right-click at the screen point (synthetic
                    // XSendEvent is dropped by GTK/Qt; MPX needs /dev/uinput).
                    // Mirrors the single-click foreground path.
                    if let Ok((sx, sy)) = window_local_to_screen(xid, xi as f64, yi as f64) {
                        crate::input::send_click_xtest_desktop(
                            sx.round() as i32,
                            sy.round() as i32,
                            3,
                            1,
                        )?;
                        return Ok(());
                    }
                    x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 3, 1)
                });
            }
            x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, 3, 1)
        })
        .await;
        let mode_label = if delivery.is_foreground() {
            "foreground"
        } else {
            "background"
        };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Right-clicked at ({x:.1}, {y:.1}) (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => linux_input_error(e),
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
            input_schema: json!({"type":"object","required":["from_x","from_y","to_x","to_y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema(),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer","description":"Target window XID. Required."},
                "from_x":{"type":"number"},
                "from_y":{"type":"number"},
                "to_x":{"type":"number"},
                "to_y":{"type":"number"},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Total drag duration. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Intermediate MotionNotify events. Default: 20."},
                "modifier": cua_driver_core::tool_schema::modifier_schema(),
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean"},
                "scope":{"type":"string","enum":["window","desktop"],"default":"window"},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if args.opt_str("scope").as_deref() == Some("desktop")
            && args.get("pid").is_none()
            && args.get("window_id").is_none()
        {
            let input = match parse_typed_projection::<DragInput>("drag", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (from_x, from_y, to_x, to_y) = (input.from_x, input.from_y, input.to_x, input.to_y);
            let button = parse_mouse_button(input.button.unwrap_or(ClickButton::Left).as_str());
            let duration_ms = input.duration_ms.unwrap_or(500).min(10_000);
            let steps = input.steps.unwrap_or(20).clamp(1, 200) as usize;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland { "wayland_desktop" } else { "xtest" };
            let result = tokio::task::spawn_blocking(move || {
                if wayland {
                    crate::wayland::drag_desktop(
                        from_x.round() as i32,
                        from_y.round() as i32,
                        to_x.round() as i32,
                        to_y.round() as i32,
                        steps as u32,
                        duration_ms,
                        button,
                    )
                } else {
                    crate::input::send_drag_xtest_desktop(
                        from_x.round() as i32,
                        from_y.round() as i32,
                        to_x.round() as i32,
                        to_y.round() as i32,
                        button,
                        duration_ms,
                        steps,
                    )
                }
            });
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                (from_x, from_y),
                (to_x, to_y),
                duration_ms,
                steps,
            );
            let (result, ()) = tokio::join!(result, visual_drag);
            if matches!(&result, Ok(Ok(()))) {
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, to_x, to_y);
            }
            return match result {
                Ok(Ok(())) => ToolResult::text("Dragged on the desktop.").with_structured(
                    json!({"scope":"desktop","path":path,"effect":"unverifiable"}),
                ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => return ToolResult::error("window_id is required on Linux."),
        };
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        if let Some(refusal) = unavailable_chromium_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_webkit_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_gtk_pointer_background(pid, delivery) {
            return refusal;
        }
        if let Some(refusal) = unavailable_wayland_focused_input_background(delivery, true) {
            return refusal;
        }

        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key)
                .or_else(|| args.opt_i64(key).map(|i| i as f64))
        };
        let mut from_x = match coerce("from_x") {
            Some(v) => v,
            None => return ToolResult::error("Missing: from_x"),
        };
        let mut from_y = match coerce("from_y") {
            Some(v) => v,
            None => return ToolResult::error("Missing: from_y"),
        };
        let mut to_x = match coerce("to_x") {
            Some(v) => v,
            None => return ToolResult::error("Missing: to_x"),
        };
        let mut to_y = match coerce("to_y") {
            Some(v) => v,
            None => return ToolResult::error("Missing: to_y"),
        };

        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = args.u64_or("steps", 20) as usize;
        let button_str = args.str_or("button", "left");
        let button = parse_mouse_button(button_str.as_str());
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

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        let wayland_points = crate::wayland::wayland_input_enabled().then(|| {
            (
                crate::wayland::window_local_to_output(
                    xid,
                    from_x.round() as i32,
                    from_y.round() as i32,
                ),
                crate::wayland::window_local_to_output(
                    xid,
                    to_x.round() as i32,
                    to_y.round() as i32,
                ),
            )
        });
        let screen_from = if let Some((from, _)) = wayland_points {
            Some((from.0 as f64, from.1 as f64))
        } else {
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y))
                .await
                .ok()
                .and_then(|result| result.ok())
        };
        if let Some((sx_from, sy_from)) = screen_from {
            overlay_glide_to_for(&cursor_id, sx_from, sy_from).await;
            self.state
                .cursor_registry
                .update_position(&cursor_id, sx_from, sy_from);
            overlay_snap_to_for(&cursor_id, sx_from, sy_from, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx_from,
                    y: sy_from,
                },
            );
        }
        // Native Wayland: emit press + interpolated motion + release as one
        // virtual-pointer (wlroots) or libei (GNOME/KDE) sequence, output-relative
        // coords. Returns early so we don't fall into the X11 XSendEvent loop below.
        if crate::wayland::wayland_input_enabled() {
            let steps_u32 = steps as u32;
            let drag_result = if crate::wayland::is_inject_mode() {
                tokio::task::spawn_blocking(move || {
                    crate::wayland::inject_drag(
                        pid,
                        xid,
                        (from_x, from_y),
                        (to_x, to_y),
                        steps,
                        button as u32,
                    )
                })
            } else {
                let ((fxi, fyi), (txi, tyi)) = wayland_points.unwrap_or((
                    (from_x.round() as i32, from_y.round() as i32),
                    (to_x.round() as i32, to_y.round() as i32),
                ));
                tokio::task::spawn_blocking(move || {
                    crate::wayland::drag(xid, fxi, fyi, txi, tyi, steps_u32, duration_ms, button)
                })
            };
            let ((from_output_x, from_output_y), (to_output_x, to_output_y)) = wayland_points
                .unwrap_or((
                    (from_x.round() as i32, from_y.round() as i32),
                    (to_x.round() as i32, to_y.round() as i32),
                ));
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                (f64::from(from_output_x), f64::from(from_output_y)),
                (f64::from(to_output_x), f64::from(to_output_y)),
                duration_ms,
                steps,
            );
            let (drag_result, ()) = tokio::join!(drag_result, visual_drag);
            if matches!(&drag_result, Ok(Ok(()))) {
                self.state.cursor_registry.update_position(
                    &cursor_id,
                    f64::from(to_output_x),
                    f64::from(to_output_y),
                );
            }
            return match drag_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Posted drag ({button_str}) to pid {pid} \
                     from ({from_x:.0}, {from_y:.0}) → ({to_x:.0}, {to_y:.0}) \
                     in {duration_ms}ms / {steps} steps."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        if delivery.is_foreground() {
            let screen_points = tokio::task::spawn_blocking(move || {
                Ok::<_, anyhow::Error>((
                    window_local_to_screen(xid, from_x, from_y)?,
                    window_local_to_screen(xid, to_x, to_y)?,
                ))
            })
            .await;
            let ((screen_from_x, screen_from_y), (screen_to_x, screen_to_y)) = match screen_points {
                Ok(Ok(points)) => points,
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            };
            let drag_result = tokio::task::spawn_blocking(move || {
                crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_drag_xtest_desktop(
                        screen_from_x.round() as i32,
                        screen_from_y.round() as i32,
                        screen_to_x.round() as i32,
                        screen_to_y.round() as i32,
                        button,
                        duration_ms,
                        steps,
                    )
                })
            });
            let visual_drag = track_overlay_drag_for(
                cursor_id.clone(),
                (screen_from_x, screen_from_y),
                (screen_to_x, screen_to_y),
                duration_ms,
                steps,
            );
            let (drag_result, ()) = tokio::join!(drag_result, visual_drag);
            if matches!(&drag_result, Ok(Ok(()))) {
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, screen_to_x, screen_to_y);
            }
            return match drag_result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Dragged ({button_str}) to pid {pid} from ({from_x:.0}, {from_y:.0}) \
                     to ({to_x:.0}, {to_y:.0}) in {duration_ms}ms / {steps} steps \
                     (delivery_mode=foreground)."
                ))
                .with_structured(json!({
                    "path": "x11_xtest_fg",
                    "verified": false,
                    "delivery_mode": "foreground"
                })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetPressed(true),
        );
        let press_result = tokio::task::spawn_blocking(move || {
            crate::input::send_button_down(
                xid,
                from_x.round() as i32,
                from_y.round() as i32,
                button,
            )
        })
        .await;
        let mut result: anyhow::Result<()> = match press_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(anyhow::anyhow!("Task error: {e}")),
        };

        if result.is_ok() {
            let step_delay_ms = if steps > 1 {
                duration_ms / steps as u64
            } else {
                duration_ms
            };
            for i in 1..=steps {
                let t = i as f64 / steps.max(1) as f64;
                let ix = from_x + (to_x - from_x) * t;
                let iy = from_y + (to_y - from_y) * t;
                let motion_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_motion(
                        xid,
                        ix.round() as i32,
                        iy.round() as i32,
                        Some(button),
                    )
                })
                .await;
                match motion_result {
                    Ok(Ok(())) => {
                        if let Ok(Ok((sx, sy))) =
                            tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy))
                                .await
                        {
                            self.state
                                .cursor_registry
                                .update_position(&cursor_id, sx, sy);
                            crate::overlay::send_command_for(
                                cursor_id.clone(),
                                cursor_overlay::track_pointer_command(sx, sy),
                            );
                        }
                        if step_delay_ms > 0 {
                            tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms))
                                .await;
                        }
                    }
                    Ok(Err(e)) => {
                        result = Err(e);
                        break;
                    }
                    Err(e) => {
                        result = Err(anyhow::anyhow!("Task error: {e}"));
                        break;
                    }
                }
            }
        }

        let release_result = tokio::task::spawn_blocking(move || {
            crate::input::send_button_up(xid, to_x.round() as i32, to_y.round() as i32, button)
        })
        .await;
        if result.is_ok() {
            result = match release_result {
                Ok(Ok(())) => Ok(()),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(anyhow::anyhow!("Task error: {e}")),
            };
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetPressed(false),
        );

        if result.is_ok() {
            if let Ok(Ok((sx_to, sy_to))) =
                tokio::task::spawn_blocking(move || window_local_to_screen(xid, to_x, to_y)).await
            {
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::track_pointer_command(sx_to, sy_to),
                );
                self.state
                    .cursor_registry
                    .update_position(&cursor_id, sx_to, sy_to);
            }
        }

        match result {
            Ok(()) => ToolResult::text(format!(
                "✅ Posted drag ({button_str}) to pid {pid} \
                 from ({from_x:.0}, {from_y:.0}) → ({to_x:.0}, {to_y:.0}) \
                 in {duration_ms}ms / {steps} steps."
            )),
            Err(e) => ToolResult::error(e.to_string()),
        }
    }
}

// ── mouse_button_down / mouse_drag / mouse_button_up ────────────────────────

pub struct MouseButtonDownTool {
    state: Arc<ToolState>,
}
static MDOWN_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseButtonDownTool {
    fn def(&self) -> &ToolDef {
        MDOWN_DEF.get_or_init(|| ToolDef {
            name: "mouse_button_down".into(),
            description: "Press and hold a mouse button at (x,y) via background X11 delivery. \
                Does not release the button; pair with mouse_drag / mouse_button_up. \
                Returns the current held-button state.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id","x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "button": cua_driver_core::tool_schema::button_schema(),
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if let Some(held) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        {
            return ToolResult::error(format!(
                "Cursor '{cursor_id}' already has a held mouse button. Call mouse_button_up first."
            ))
            .with_structured(mouse_hold_json(&cursor_id, Some(&held)));
        }

        let pid = match args.require_u32("pid") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let xid = match args.opt_u64("window_id") {
            Some(v) => v,
            None => return ToolResult::error("window_id is required on Linux."),
        };
        let button_name = args.str_or("button", "left");
        let button = parse_mouse_button(button_name.as_str());
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
        if args.bool_or("from_zoom", false) {
            match self.state.zoom_registry.get(pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {pid}. Call zoom first."
                    ))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
        {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );
        }

        let xi = x as i32;
        let yi = y as i32;
        // Native Wayland: route through the persistent virtual-pointer module
        // so the held button survives across tool calls; the X11 path keeps
        // the existing input::send_button_down behaviour.
        let result = if crate::wayland::is_wayland() {
            let cid = cursor_id.clone();
            tokio::task::spawn_blocking(move || {
                crate::wayland::persistent_vptr::press(&cid, xid, xi, yi, button)
            })
            .await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_down(xid, xi, yi, button))
                .await
        };
        match result {
            Ok(Ok(())) => {
                let hold = MouseHoldState {
                    pid,
                    xid,
                    button,
                    x,
                    y,
                };
                self.state
                    .mouse_hold
                    .lock()
                    .unwrap()
                    .insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(&cursor_id, sx, sy, None);
                    crate::overlay::send_command_for(
                        cursor_id.clone(),
                        cursor_overlay::OverlayCommand::SetPressed(true),
                    );
                }
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' held {} button down at ({x:.1}, {y:.1}).",
                    mouse_button_name(button),
                ))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()).with_structured(mouse_hold_json(
                &cursor_id,
                self.state.mouse_hold.lock().unwrap().get(&cursor_id),
            )),
            Err(e) => {
                ToolResult::error(format!("Task error: {e}")).with_structured(mouse_hold_json(
                    &cursor_id,
                    self.state.mouse_hold.lock().unwrap().get(&cursor_id),
                ))
            }
        }
    }
}

pub struct MouseDragTool {
    state: Arc<ToolState>,
}
static MDRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseDragTool {
    fn def(&self) -> &ToolDef {
        MDRAG_DEF.get_or_init(|| ToolDef {
            name: "mouse_drag".into(),
            description: "Move a previously-held mouse button to a new point via background X11 delivery. \
                Requires an active mouse_button_down state; does not release the button. \
                Returns the updated held-button state.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Total drag duration. Default: 500."},
                "steps":{"type":"integer","minimum":1,"maximum":200,"description":"Intermediate MotionNotify events. Default: 20."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let Some(mut hold) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        else {
            return ToolResult::error(format!(
                "No mouse button is currently held for cursor '{cursor_id}'. Call mouse_button_down first."
            ))
            .with_structured(mouse_hold_json(&cursor_id, None));
        };
        if let Some(err) = held_target_mismatch(&args, &cursor_id, &hold) {
            return err;
        }

        let mut to_x = args.f64_or("x", 0.0);
        let mut to_y = args.f64_or("y", 0.0);
        if args.bool_or("from_zoom", false) {
            match self.state.zoom_registry.get(hold.pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(to_x, to_y);
                    to_x = wx;
                    to_y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {}. Call zoom first.",
                        hold.pid
                    ))
                    .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(hold.pid) {
            to_x *= ratio;
            to_y *= ratio;
        }

        let xid = hold.xid;

        let from_x = hold.x;
        let from_y = hold.y;
        let duration_ms = args.u64_or("duration_ms", 500);
        let steps = args.u64_or("steps", 20).max(1) as usize;
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y)).await
        {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            self.state
                .cursor_registry
                .update_position(&cursor_id, sx, sy);
            overlay_snap_to_for(&cursor_id, sx, sy, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );
        }

        let button = hold.button;
        let step_delay_ms = if steps > 1 {
            duration_ms / steps as u64
        } else {
            duration_ms
        };
        let mut result: anyhow::Result<()> = Ok(());
        let mut prev_x = from_x;
        let mut prev_y = from_y;
        let is_wl = crate::wayland::is_wayland();
        for i in 1..=steps {
            let t = i as f64 / steps as f64;
            let ix = from_x + (to_x - from_x) * t;
            let iy = from_y + (to_y - from_y) * t;
            // Native Wayland: route motion through the persistent virtual-
            // pointer so the held button stays held across the entire drag;
            // X11 keeps the existing input::send_motion path.
            let cid_inner = cursor_id.clone();
            let move_result = if is_wl {
                tokio::task::spawn_blocking(move || {
                    crate::wayland::persistent_vptr::move_to(
                        &cid_inner,
                        ix.round() as i32,
                        iy.round() as i32,
                    )
                })
                .await
            } else {
                tokio::task::spawn_blocking(move || {
                    crate::input::send_motion(
                        xid,
                        ix.round() as i32,
                        iy.round() as i32,
                        Some(button),
                    )
                })
                .await
            };
            match move_result {
                Ok(Ok(())) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy))
                            .await
                    {
                        let heading = if (ix - prev_x).abs() > f64::EPSILON
                            || (iy - prev_y).abs() > f64::EPSILON
                        {
                            Some((iy - prev_y).atan2(ix - prev_x))
                        } else {
                            None
                        };
                        self.state
                            .cursor_registry
                            .update_position(&cursor_id, sx, sy);
                        overlay_move_to_for(&cursor_id, sx, sy, heading);
                    }
                    prev_x = ix;
                    prev_y = iy;
                    if step_delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(step_delay_ms)).await;
                    }
                }
                Ok(Err(e)) => {
                    result = Err(e);
                    break;
                }
                Err(e) => {
                    result = Err(anyhow::anyhow!("Task error: {e}"));
                    break;
                }
            }
        }

        match result {
            Ok(()) => {
                hold.x = to_x;
                hold.y = to_y;
                self.state
                    .mouse_hold
                    .lock()
                    .unwrap()
                    .insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, to_x, to_y))
                        .await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(
                        &cursor_id,
                        sx,
                        sy,
                        Some((to_y - from_y).atan2(to_x - from_x)),
                    );
                    crate::overlay::send_command_for(
                        cursor_id.clone(),
                        cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
                    );
                }
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' dragged held {} button to ({to_x:.1}, {to_y:.1}).",
                    mouse_button_name(hold.button),
                ))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
            }
            Err(e) => ToolResult::error(e.to_string())
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
        }
    }
}

pub struct MouseButtonUpTool {
    state: Arc<ToolState>,
}
static MUP_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for MouseButtonUpTool {
    fn def(&self) -> &ToolDef {
        MUP_DEF.get_or_init(|| ToolDef {
            name: "mouse_button_up".into(),
            description: "Release a previously-held mouse button via background X11 delivery. \
                If x/y are omitted, releases at the last held position. Returns the current held-button state.".into(),
            input_schema: json!({"type":"object","properties":{
                "session": cua_driver_core::tool_schema::session_schema_with(
                    "When both are present, session takes precedence over cursor_id."
                ),
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let Some(hold) = self
            .state
            .mouse_hold
            .lock()
            .unwrap()
            .get(&cursor_id)
            .cloned()
        else {
            return ToolResult::error(format!(
                "No mouse button is currently held for cursor '{cursor_id}'."
            ))
            .with_structured(mouse_hold_json(&cursor_id, None));
        };
        if let Some(err) = held_target_mismatch(&args, &cursor_id, &hold) {
            return err;
        }

        let xid = hold.xid;

        let mut x = args.opt_f64("x").unwrap_or(hold.x);
        let mut y = args.opt_f64("y").unwrap_or(hold.y);
        if args.bool_or("from_zoom", false) {
            match self.state.zoom_registry.get(hold.pid) {
                Some(ctx) => {
                    let (wx, wy) = ctx.zoom_to_window(x, y);
                    x = wx;
                    y = wy;
                }
                None => {
                    return ToolResult::error(format!(
                        "from_zoom=true but no zoom context for pid {}. Call zoom first.",
                        hold.pid
                    ))
                    .with_structured(mouse_hold_json(&cursor_id, Some(&hold)))
                }
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(hold.pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
        {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
        }

        let button = hold.button;
        let xi = x as i32;
        let yi = y as i32;
        // Native Wayland: release through the persistent virtual-pointer so
        // the same vptr device that emitted the press also emits the release
        // (single logical drag rather than a click pair).
        let result = if crate::wayland::is_wayland() {
            let cid = cursor_id.clone();
            tokio::task::spawn_blocking(move || {
                crate::wayland::persistent_vptr::release(&cid, button)
            })
            .await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_up(xid, xi, yi, button))
                .await
        };
        match result {
            Ok(Ok(())) => {
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state
                        .cursor_registry
                        .update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(&cursor_id, sx, sy, None);
                }
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::SetPressed(false),
                );
                self.state.mouse_hold.lock().unwrap().remove(&cursor_id);
                let cleared = mouse_hold_json(&cursor_id, None);
                ToolResult::text(format!(
                    "✅ Cursor '{cursor_id}' released held {} button at ({x:.1}, {y:.1}).",
                    mouse_button_name(button),
                ))
                .with_structured(cleared)
            }
            Ok(Err(e)) => ToolResult::error(e.to_string())
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
            Err(e) => ToolResult::error(format!("Task error: {e}"))
                .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
        }
    }
}

pub struct ParallelMouseDragTool {
    state: Arc<ToolState>,
}
static PMDRAG_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

/// cua-compositor path for parallel_mouse_drag: build window-local drag paths
/// and run them as concurrent multi-cursor injections over the control socket.
/// Coordinates stay window-local (the compositor maps them per app_id), so no
/// X11 geometry/MPX is needed — the X11 path's hard blocker on Wayland.
async fn parallel_drag_inject(args: &Value) -> ToolResult {
    let Some(items) = args.get("drags").and_then(|v| v.as_array()) else {
        return ToolResult::error("drags[] is required.");
    };
    if items.len() < 2 {
        return ToolResult::error("parallel_mouse_drag requires at least two drag items.");
    }
    let mut drags = Vec::with_capacity(items.len());
    for (i, item) in items.iter().enumerate() {
        let Some(xid) = item.get("window_id").and_then(|v| v.as_u64()) else {
            return ToolResult::error("each drag item requires window_id.");
        };
        let local: Vec<(f64, f64)> = if let Some(pts) = item.get("path").and_then(|v| v.as_array())
        {
            pts.iter()
                .filter_map(|p| {
                    let a = p.as_array()?;
                    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                })
                .collect()
        } else {
            let g = |k: &str| item.get(k).and_then(|v| v.as_f64());
            match (g("from_x"), g("from_y"), g("to_x"), g("to_y")) {
                (Some(fx), Some(fy), Some(tx), Some(ty)) => vec![(fx, fy), (tx, ty)],
                _ => {
                    return ToolResult::error(
                        "each drag item requires path[] or from_x/from_y/to_x/to_y.",
                    )
                }
            }
        };
        if local.len() < 2 {
            return ToolResult::error("drag path needs at least 2 points.");
        }
        let steps = item
            .get("steps")
            .and_then(|v| v.as_u64())
            .unwrap_or(60)
            .clamp(1, 300) as usize;
        let x_button = parse_mouse_button(
            item.get("button")
                .and_then(|v| v.as_str())
                .unwrap_or("left"),
        ) as u32;
        let app = match tokio::task::spawn_blocking(move || {
            crate::wayland::inject_target_for_window(xid)
        })
        .await
        {
            Ok(Ok(target)) => target,
            Ok(Err(error)) => return ToolResult::error(error.to_string()),
            Err(error) => return ToolResult::error(format!("Task error: {error}")),
        };
        drags.push(crate::wayland::InjectDrag {
            app_id: app,
            idx: i,
            x_button,
            path: local,
            steps,
        });
    }
    let n = drags.len();
    match tokio::task::spawn_blocking(move || crate::wayland::inject_parallel_drags(&drags)).await {
        Ok(Ok(())) => ToolResult::text(format!(
            "Ran {n} concurrent drags (multi-cursor via cua-compositor)."
        )),
        Ok(Err(e)) => ToolResult::error(e.to_string()),
        Err(e) => ToolResult::error(format!("Task error: {e}")),
    }
}

#[async_trait]
impl Tool for ParallelMouseDragTool {
    fn def(&self) -> &ToolDef {
        PMDRAG_DEF.get_or_init(|| ToolDef {
            name: "parallel_mouse_drag".into(),
            description: "Run multiple mouse drag gestures concurrently via Linux MPX/XI2 virtual master pointers. \
                Each drag item runs on its own session-scoped master pointer (true same-window concurrent draws on X11). \
                Each item presses once, glides continuously through its whole path, and releases once — one smooth held \
                drag, not a chain of clicks. A path is given either as a straight segment (from_x/from_y → to_x/to_y) or \
                as a function `fn` = y(x) sampled over [x_from, x_to] in window-local pixels (e.g. fn:\"x\" is a diagonal, \
                fn:\"300+120*sin(x/40)\" a sine wave). Functions support + - * / ^, sin/cos/tan, sqrt, abs, exp, ln, pi, e.".into(),
            input_schema: json!({"type":"object","required":["drags"],"properties":{
                "drags":{"type":"array","minItems":2,"items":{"type":"object","required":["session","window_id"],"properties":{
                    "session":{"type":"string","description":"Session/cursor id; also keys the virtual master pointer."},
                    "window_id":{"type":"integer"},
                    "path":{"type":"array","items":{"type":"array","items":{"type":"number"}},"description":"Explicit window-local waypoints [[x,y],...] (>=2); pressed once, glided through, released once. Takes precedence over fn/from-to."},
                    "fn":{"type":"string","description":"Expression y(x) in window-local pixels; sampled over [x_from,x_to]. Mutually exclusive with from_x/to_x."},
                    "x_from":{"type":"number","description":"Domain start (window-local x) when `fn` is used."},
                    "x_to":{"type":"number","description":"Domain end (window-local x) when `fn` is used."},
                    "samples":{"type":"integer","minimum":2,"maximum":400,"description":"Waypoints sampled along `fn`. Default: 80."},
                    "from_x":{"type":"number"},
                    "from_y":{"type":"number"},
                    "to_x":{"type":"number"},
                    "to_y":{"type":"number"},
                    "button": cua_driver_core::tool_schema::button_schema(),
                    "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Default: 1500 for fn paths, 500 for straight."},
                    "steps":{"type":"integer","minimum":1,"maximum":300,"description":"Motion sub-steps along the whole path. Default: scaled to path length."}
                },"additionalProperties":false}}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Nested cua-compositor: run the drags as concurrent multi-cursor
        // injections (window-local, no X11 MPX/geometry needed).
        if crate::wayland::is_inject_mode() {
            return parallel_drag_inject(&args).await;
        }
        // Native Wayland without the inject socket: MPX/XI2 + uinput master
        // pointers don't exist on Wayland. Surface a typed error instead of
        // silently calling the X11 path that's guaranteed to fail.
        if crate::wayland::is_wayland() {
            return ToolResult::error(
                "parallel_mouse_drag requires the cua-compositor inject socket on Wayland \
                 (set CUA_INJECT_SOCKET to the cua-compositor control socket), \
                 or run the target under X11.",
            );
        }
        match tokio::task::spawn_blocking(crate::input::check_parallel_pointer_support).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return ToolResult::error(e.to_string()),
            Err(e) => return ToolResult::error(format!("Task error: {e}")),
        }

        let Some(items) = args.get("drags").and_then(|v| v.as_array()) else {
            return ToolResult::error("drags[] is required.");
        };
        if items.len() < 2 {
            return ToolResult::error("parallel_mouse_drag requires at least two drag items.");
        }

        let mut drags = Vec::with_capacity(items.len());
        for item in items {
            let Some(session) = item.get("session").and_then(|v| v.as_str()) else {
                return ToolResult::error("each drag item requires session.");
            };
            let Some(xid) = item.get("window_id").and_then(|v| v.as_u64()) else {
                return ToolResult::error("each drag item requires window_id.");
            };

            // Build the window-local waypoint path from one of: an explicit
            // `path` of [x,y] points, a function `fn` (y = f(x) sampled over
            // [x_from, x_to]), or a straight from→to segment.
            let is_fn = item.get("fn").and_then(|v| v.as_str()).is_some();
            let local: Vec<(f64, f64)> = if let Some(pts) =
                item.get("path").and_then(|v| v.as_array())
            {
                let mut out = Vec::with_capacity(pts.len());
                for p in pts {
                    let a = p.as_array();
                    let (Some(px), Some(py)) = (
                        a.and_then(|a| a.first()).and_then(|v| v.as_f64()),
                        a.and_then(|a| a.get(1)).and_then(|v| v.as_f64()),
                    ) else {
                        return ToolResult::error("each `path` entry must be [x, y].");
                    };
                    out.push((px, py));
                }
                if out.len() < 2 {
                    return ToolResult::error("`path` needs at least 2 points.");
                }
                out
            } else if let Some(expr_str) = item.get("fn").and_then(|v| v.as_str()) {
                let Some(x_from) = item.get("x_from").and_then(|v| v.as_f64()) else {
                    return ToolResult::error("`fn` requires x_from.");
                };
                let Some(x_to) = item.get("x_to").and_then(|v| v.as_f64()) else {
                    return ToolResult::error("`fn` requires x_to.");
                };
                let samples = item
                    .get("samples")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(80)
                    .clamp(2, 400);
                match crate::input::sample_function(expr_str, x_from, x_to, samples) {
                    Ok(pts) => pts,
                    Err(e) => return ToolResult::error(e.to_string()),
                }
            } else {
                let coerce = |k: &str| item.get(k).and_then(|v| v.as_f64());
                match (coerce("from_x"), coerce("from_y"), coerce("to_x"), coerce("to_y")) {
                    (Some(fx), Some(fy), Some(tx), Some(ty)) => vec![(fx, fy), (tx, ty)],
                    _ => return ToolResult::error("each drag item requires either `fn`+x_from+x_to, or from_x/from_y/to_x/to_y."),
                }
            };

            let button = parse_mouse_button(
                item.get("button")
                    .and_then(|v| v.as_str())
                    .unwrap_or("left"),
            );
            let duration_ms = item
                .get("duration_ms")
                .and_then(|v| v.as_u64())
                .unwrap_or(if is_fn { 1500 } else { 500 });

            // One translate gives the window origin; the path is a pure offset.
            let origin =
                match tokio::task::spawn_blocking(move || window_local_to_screen(xid, 0.0, 0.0))
                    .await
                {
                    Ok(Ok(o)) => o,
                    Ok(Err(e)) => return ToolResult::error(e.to_string()),
                    Err(e) => return ToolResult::error(format!("Task error: {e}")),
                };
            let path: Vec<(i32, i32)> = local
                .iter()
                .map(|(lx, ly)| {
                    (
                        (origin.0 + lx).round() as i32,
                        (origin.1 + ly).round() as i32,
                    )
                })
                .collect();

            // Default sub-step count scaled to path length (smooth glide),
            // overridable via `steps`.
            let total_len: f64 = path
                .windows(2)
                .map(|w| {
                    (((w[1].0 - w[0].0) as f64).powi(2) + ((w[1].1 - w[0].1) as f64).powi(2)).sqrt()
                })
                .sum();
            let steps = item
                .get("steps")
                .and_then(|v| v.as_u64())
                .map(|s| (s as usize).clamp(1, 300))
                .unwrap_or_else(|| ((total_len / 3.0).round() as usize).clamp(24, 300));

            let start = path[0];
            self.state
                .cursor_registry
                .update_position(session, start.0 as f64, start.1 as f64);
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::PinAbove(xid),
            );
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::SnapTo {
                    x: start.0 as f64,
                    y: start.1 as f64,
                    heading_radians: None,
                },
            );
            crate::overlay::send_command_for(
                session.to_owned(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );

            drags.push((
                session.to_owned(),
                crate::input::VirtualPointerDrag {
                    target_window: xid,
                    button,
                    path,
                    duration_ms,
                    steps,
                },
            ));
        }

        let drags_for_task = drags.clone();
        let result = tokio::task::spawn_blocking(move || {
            crate::input::send_parallel_virtual_pointer_drags(&drags_for_task)
        })
        .await;
        match result {
            Ok(Ok(())) => {
                for (session, drag) in &drags {
                    let n = drag.path.len();
                    let end = drag.path[n - 1];
                    let prev = drag.path[n.saturating_sub(2)];
                    self.state
                        .cursor_registry
                        .update_position(session, end.0 as f64, end.1 as f64);
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SnapTo {
                            x: end.0 as f64,
                            y: end.1 as f64,
                            heading_radians: Some(
                                ((end.1 - prev.1) as f64).atan2((end.0 - prev.0) as f64),
                            ),
                        },
                    );
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::ClickPulse {
                            x: end.0 as f64,
                            y: end.1 as f64,
                        },
                    );
                }
                ToolResult::text(format!(
                    "✅ Ran {} MPX drag gesture(s) concurrently.",
                    drags.len()
                ))
                .with_structured(json!({"count": drags.len()}))
            }
            Ok(Err(e)) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                }
                linux_input_error(e)
            }
            Err(e) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(
                        session.to_owned(),
                        cursor_overlay::OverlayCommand::SetPressed(false),
                    );
                }
                ToolResult::error(format!("Task error: {e}"))
            }
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
                Requires no TCC permissions."
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
        if let Err(result) = parse_typed_input::<GetScreenSizeInput>("get_screen_size", args) {
            return result;
        }
        let result = tokio::task::spawn_blocking(|| {
            // X11 reports pixel dimensions; scale factor on X11 is not
            // well-defined per-monitor, so report 1.0 (matches DPI-unaware
            // assumption).  Wayland/HiDPI X11 callers should query
            // `xrandr --query` for true scale.
            let (w, h) = x11_screen_size()?;
            Ok::<(u32, u32, f64), anyhow::Error>((w, h, 1.0))
        })
        .await;
        match result {
            // Matches Swift text format 1:1.
            Ok(Ok((w, h, scale))) => {
                ToolResult::text(format!("✅ Main display: {w}x{h} points @ {scale}x"))
                    .with_structured(json!({ "width": w, "height": h, "scale_factor": scale }))
            }
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

/// Read the true X11 root-window size in pixels: (width, height).
/// Shared by `get_screen_size` and `get_desktop_state`.
fn x11_screen_size() -> anyhow::Result<(u32, u32)> {
    use x11rb::connection::Connection;
    use x11rb::rust_connection::RustConnection;
    let (conn, screen_num) = RustConnection::connect(None)
        .map_err(|e| anyhow::anyhow!("{e}{}", crate::no_display_hint()))?;
    let setup = conn.setup();
    let screen = &setup.roots[screen_num];
    let w = screen.width_in_pixels as u32;
    let h = screen.height_in_pixels as u32;
    // WSLg / headless XWayland quirk: the X server connects but the
    // root screen advertises a 0-px geometry until a real output is
    // attached. Returning {width:0,height:0} here would propagate a
    // success with zero dimensions to the client, which then either
    // divides by zero when scaling or feeds the value into `int(...)`
    // after the missing key collapses to None. Fail loudly with an
    // actionable, typed error instead (never emit a 0/null where the
    // client expects a usable int). See issue #2005.
    if w == 0 || h == 0 {
        anyhow::bail!(
            "X11 connected but reports a 0x0 root screen — no usable \
             display geometry.{}",
            crate::no_display_hint()
        );
    }
    Ok((w, h))
}

/// Put the desktop image in the exact coordinate frame consumed by desktop
/// actions. Native Wayland capture buffers may use backing pixels while the
/// compositor's pointer protocol and reported screen geometry use logical
/// pixels. Returning the backing image alongside logical dimensions violates
/// the screenshot-to-action contract and makes every vision-grounded action
/// miss by the output scale.
fn normalize_desktop_capture_for_action_frame(
    png: Vec<u8>,
    action_width: u32,
    action_height: u32,
) -> anyhow::Result<(Vec<u8>, u32, u32, f64)> {
    if action_width == 0 || action_height == 0 {
        anyhow::bail!("desktop action frame is empty: {action_width}x{action_height}");
    }

    let (capture_width, capture_height) = crate::capture::png_dimensions_pub(&png)?;
    let scale_x = f64::from(capture_width) / f64::from(action_width);
    let scale_y = f64::from(capture_height) / f64::from(action_height);
    if (scale_x - scale_y).abs() > 0.01 {
        anyhow::bail!(
            "desktop capture {capture_width}x{capture_height} cannot be mapped uniformly to \
             action frame {action_width}x{action_height} (scale {scale_x:.4}x{scale_y:.4})"
        );
    }

    if capture_width == action_width && capture_height == action_height {
        return Ok((png, action_width, action_height, 1.0));
    }

    let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?;
    let resized = image.resize_exact(
        action_width,
        action_height,
        image::imageops::FilterType::Lanczos3,
    );
    let mut encoded = std::io::Cursor::new(Vec::new());
    resized.write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok((encoded.into_inner(), action_width, action_height, scale_x))
}

// ── get_desktop_state ─────────────────────────────────────────────────────────

pub struct GetDesktopStateTool;
static GDS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        GDS_DEF.get_or_init(|| ToolDef {
            name: "get_desktop_state".into(),
            description: "Capture the full display in the desktop action coordinate frame. \
                Use the returned PNG directly as the coordinate source for actions whose target is \
                {kind:\"desktop\",display_id:\"primary\"}. No AT-SPI walk.".into(),
            input_schema: json!({"type":"object","properties":{
                "session":{"type":"string","description":"For multi-call work, prefer a short public session label and repeat it on every call that accepts it. Omit it to use the authenticated transport's implicit lifecycle session."},
                "screenshot_out_file":{"type":"string","description":"Write PNG here instead of base64."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let input = match parse_typed_input::<GetDesktopStateInput>("get_desktop_state", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let out_file = input.screenshot_out_file;

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Capture the full display at native size first. When the
            // compositor consumes logical input coordinates, normalize the
            // image below so screenshot pixels still land exactly.
            let native_png = crate::capture::screenshot_display_bytes()?;
            let (native_w, native_h) = crate::capture::png_dimensions_pub(&native_png)?;
            // True screen size. On a pure-Wayland session (native backend
            // opted in, no X11 DISPLAY) the capture above came from the
            // wlroots `zwlr_screencopy` cascade, whose full-display buffer is
            // the whole output at native (physical) pixels — so the PNG
            // dimensions ARE the true screen size. Querying the X11 root
            // window here would fail with "$DISPLAY variable not set" and
            // abort the tool even though the screenshot already succeeded.
            // Only fall back to the X11 root-window geometry off Wayland, so
            // the X11 / XWayland path is unchanged. See #2017 / Sway testing.
            let (screen_w, screen_h) = if crate::wayland::is_wayland() {
                (native_w, native_h)
            } else {
                x11_screen_size()?
            };
            let (png, shot_w, shot_h, scale_factor) =
                normalize_desktop_capture_for_action_frame(native_png, screen_w, screen_h)?;
            // Optional: write PNG to disk instead of returning base64.
            let written = if let Some(path) = out_file.as_deref() {
                std::fs::write(path, &png)?;
                Some(path.to_string())
            } else {
                None
            };
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            let b64 = if written.is_some() {
                None
            } else {
                Some(B64.encode(&png))
            };
            Ok((
                b64,
                shot_w,
                shot_h,
                screen_w,
                screen_h,
                scale_factor,
                written,
            ))
        })
        .await;

        match result {
            Ok(Ok((b64_opt, shot_w, shot_h, screen_w, screen_h, scale_factor, written))) => {
                let mut content = Vec::new();
                let mut structured = json!({
                    "platform": "linux",
                    "display": "primary",
                    "screenshot_width": shot_w,
                    "screenshot_height": shot_h,
                    "screen_width": screen_w,
                    "screen_height": screen_h,
                    "scale_factor": scale_factor,
                    "screenshot_mime_type": "image/png",
                });
                if let Some(b64) = b64_opt {
                    content.push(cua_driver_core::protocol::Content::image_png(b64));
                }
                if let Some(path) = written {
                    structured["screenshot_file_path"] = json!(path);
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "✅ Desktop screenshot {shot_w}x{shot_h} written to {path} (screen {screen_w}x{screen_h})"
                    )));
                } else {
                    content.push(cua_driver_core::protocol::Content::text(format!(
                        "✅ Desktop screenshot {shot_w}x{shot_h} (screen {screen_w}x{screen_h})"
                    )));
                }
                ToolResult {
                    content,
                    is_error: None,
                    structured_content: Some(structured),
                    action_record: None,
                }
            }
            Ok(Err(e)) => ToolResult::error(format!("Capture error: {e}")),
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
        if let Err(result) =
            parse_typed_input::<GetCursorPositionInput>("get_cursor_position", args)
        {
            return result;
        }
        // Native Wayland: there's no protocol for clients to query the real
        // global cursor position. Fall back to the synthetic registry that
        // records every `motion_absolute` this process emits.
        if crate::wayland::is_wayland() {
            return match crate::wayland::last_synth_cursor_pos() {
                Some((x, y)) => ToolResult::text(
                    format!("✅ Cursor at ({x}, {y}) (synthetic — last move_cursor in this process)")
                ).with_structured(json!({
                    "x": x, "y": y, "source": "synthetic"
                })),
                None => ToolResult::text(
                    "Cursor position unknown on Wayland — no move_cursor has been issued in this process yet.".to_string()
                ).with_structured(json!({ "source": "synthetic", "available": false })),
            };
        }
        let result = tokio::task::spawn_blocking(|| {
            use x11rb::connection::Connection;
            use x11rb::protocol::xproto::ConnectionExt as _;
            use x11rb::rust_connection::RustConnection;
            let (conn, screen_num) = RustConnection::connect(None)?;
            let root = conn.setup().roots[screen_num].root;
            let reply = conn.query_pointer(root)?.reply()?;
            Ok::<(i32, i32), anyhow::Error>((reply.root_x as i32, reply.root_y as i32))
        })
        .await;
        match result {
            // Text format matches Swift `GetCursorPositionTool` 1:1.
            Ok(Ok((x, y))) => ToolResult::text(format!("✅ Cursor at ({x}, {y})"))
                .with_structured(json!({ "x": x, "y": y, "source": "x11" })),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CursorControlScope {
    Agent,
    Desktop,
}

fn cursor_control_scope(args: &Value) -> CursorControlScope {
    match args.get("scope").and_then(Value::as_str) {
        Some("desktop") => CursorControlScope::Desktop,
        _ => CursorControlScope::Agent,
    }
}

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description: "Move the synthetic agent cursor without changing the user's pointer. Only an explicit scope=desktop request moves the real OS pointer in get_desktop_state coordinates.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "x":{"type":"number"},"y":{"type":"number"},"session": cua_driver_core::tool_schema::session_schema(),"cursor_id":{"type":"string"},"scope":{"type":"string","enum":["window","desktop"],"default":"window"}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if cursor_control_scope(&args) == CursorControlScope::Desktop {
            let input = match parse_typed_projection::<MoveCursorInput>("move_cursor", &args) {
                Ok(input) => input,
                Err(result) => return result,
            };
            let (x, y) = (input.x, input.y);
            let xi = x.round() as i32;
            let yi = y.round() as i32;
            let wayland = crate::wayland::wayland_input_enabled();
            let path = if wayland {
                "wayland_desktop"
            } else {
                "xtest_desktop"
            };
            let result = if wayland {
                tokio::task::spawn_blocking(move || {
                    crate::wayland::move_cursor_absolute(None, xi, yi)
                })
                .await
            } else {
                tokio::task::spawn_blocking(move || crate::input::send_move_xtest_desktop(xi, yi))
                    .await
            };
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Moved the real desktop pointer to ({xi}, {yi})."
                ))
                .with_structured(
                    json!({"scope":"desktop","path":path,"x":xi,"y":yi,"effect":"unverifiable"}),
                ),
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        let cursor_id = resolve_cursor_key(&args);
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so the
        // overlay arrow settles to the natural macOS-style pose.
        // Use the acknowledged animation path so a first-ever move seeds and
        // displays the session cursor just as reliably as a coordinate click.
        reveal_pointer_action_for(&self.state, &cursor_id, x, y, false).await;
        ToolResult::text(format!(
            "Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1}); the user pointer was unchanged."
        ))
    }
}

// ── set_agent_cursor_enabled ──────────────────────────────────────────────────

pub struct SetAgentCursorEnabledV2Tool {
    state: Arc<ToolState>,
}

static CURSOR_ENABLED_V2_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetAgentCursorEnabledV2Tool {
    fn def(&self) -> &ToolDef {
        CURSOR_ENABLED_V2_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_enabled"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let enabled = match args.get("enabled").and_then(Value::as_bool) {
            Some(value) => value,
            None => return ToolResult::error("Missing required boolean field `enabled`."),
        };
        let session = resolve_cursor_key(&args);
        self.state.cursor_registry.set_enabled(&session, enabled);
        crate::overlay::send_command_for(
            session.clone(),
            cursor_overlay::OverlayCommand::SetEnabled(enabled),
        );
        ToolResult::text(format!(
            "Agent cursor for session '{session}' {}.",
            if enabled { "enabled" } else { "disabled" }
        ))
        .with_structured(json!({"session":session,"enabled":enabled}))
    }
}

pub struct SetAgentCursorMotionV2Tool;

static CURSOR_MOTION_V2_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn cursor_number(value: Option<&Value>) -> Option<f64> {
    value.and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_i64().map(|integer| integer as f64))
    })
}

#[async_trait]
impl Tool for SetAgentCursorMotionV2Tool {
    fn def(&self) -> &ToolDef {
        CURSOR_MOTION_V2_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_motion"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let session = resolve_cursor_key(&args);
        let current = crate::overlay::current_motion_for(&session);
        let motion = current.with_overrides(
            cursor_number(args.get("start_handle")),
            cursor_number(args.get("end_handle")),
            cursor_number(args.get("arc_size")),
            cursor_number(args.get("arc_flow")),
            cursor_number(args.get("spring")),
            cursor_number(args.get("glide_duration_ms")),
            cursor_number(args.get("dwell_after_click_ms")),
            cursor_number(args.get("idle_hide_ms")),
            None,
            cursor_number(args.get("turn_radius")),
        );
        crate::overlay::send_command_for(
            session.clone(),
            cursor_overlay::OverlayCommand::SetMotion(motion.clone()),
        );
        ToolResult::text(format!(
            "Agent cursor motion updated for session '{session}'."
        ))
        .with_structured(json!({"session":session,"motion":{
            "start_handle":motion.start_handle,
            "end_handle":motion.end_handle,
            "arc_size":motion.arc_size,
            "arc_flow":motion.arc_flow,
            "spring":motion.spring,
            "glide_duration_ms":motion.glide_duration_ms,
            "dwell_after_click_ms":motion.dwell_after_click_ms,
            "idle_hide_ms":motion.idle_hide_ms,
            "turn_radius":motion.turn_radius
        }}))
    }
}

pub struct SetAgentCursorThemeTool {
    state: Arc<ToolState>,
}

static CURSOR_THEME_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn cursor_reduced_motion(value: Option<&str>) -> cursor_overlay::ReducedMotion {
    match value {
        Some("on") => cursor_overlay::ReducedMotion::On,
        Some("off") => cursor_overlay::ReducedMotion::Off,
        _ => cursor_overlay::ReducedMotion::Auto,
    }
}

#[async_trait]
impl Tool for SetAgentCursorThemeTool {
    fn def(&self) -> &ToolDef {
        CURSOR_THEME_DEF.get_or_init(|| canonical_cursor_def("set_agent_cursor_theme"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let Some(theme_id) = args.get("theme_id").and_then(Value::as_str) else {
            return ToolResult::error("Missing required string field `theme_id`.");
        };
        let session = resolve_cursor_key(&args);
        let resolved_theme = match cursor_overlay::resolve_theme_selection(theme_id) {
            Ok(theme) => theme,
            Err(error) => {
                return ToolResult::error(format!(
                    "Cursor theme '{theme_id}' cannot be selected: {error}"
                ));
            }
        };
        let (version, profile) = resolved_theme
            .as_deref()
            .map(|theme| (theme.version.as_str(), theme.profile.as_str()))
            .unwrap_or((
                cursor_overlay::DEFAULT_THEME_VERSION,
                cursor_overlay::THEME_PROFILE,
            ));
        let reduced_motion =
            cursor_reduced_motion(args.get("reduced_motion").and_then(Value::as_str));
        self.state
            .cursor_registry
            .update_config(&session, |config| {
                config.theme_id = theme_id.to_owned();
                config.reduced_motion = reduced_motion;
            });
        crate::overlay::send_command_for(
            session.clone(),
            cursor_overlay::OverlayCommand::SetTheme {
                theme_id: theme_id.to_owned(),
                reduced_motion,
            },
        );
        ToolResult::text(format!(
            "Agent cursor theme for session '{session}' set to '{theme_id}'."
        ))
        .with_structured(json!({"session":session,"theme":{
            "id":theme_id,
            "version":version,
            "profile":profile,
            "reduced_motion":reduced_motion,
            "fallback":null
        }}))
    }
}

pub struct GetAgentCursorStateV2Tool;

static CURSOR_STATE_V2_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetAgentCursorStateV2Tool {
    fn def(&self) -> &ToolDef {
        CURSOR_STATE_V2_DEF.get_or_init(|| canonical_cursor_def("get_agent_cursor_state"))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let session = resolve_cursor_key(&args);
        let enabled = crate::overlay::is_enabled_for(&session);
        let motion = crate::overlay::current_motion_for(&session);
        let (theme_id, version, profile, fallback, visual) =
            crate::overlay::current_theme_state_for(&session).unwrap_or_else(|| {
                (
                    cursor_overlay::DEFAULT_THEME_ID.into(),
                    cursor_overlay::DEFAULT_THEME_VERSION.into(),
                    cursor_overlay::THEME_PROFILE.into(),
                    None,
                    cursor_overlay::CursorVisualState::default(),
                )
            });
        let modifiers: Vec<&str> = [
            visual.delivery.map(|value| value.as_str()),
            visual.target.map(|value| value.as_str()),
        ]
        .into_iter()
        .flatten()
        .collect();
        ToolResult::text(format!("Agent cursor state for session '{session}'.")).with_structured(
            json!({
                "session":session,
                "enabled":enabled,
                "position":null,
                "theme":{
                    "id":theme_id,
                    "version":version,
                    "profile":profile,
                    "reduced_motion":visual.reduced_motion,
                    "fallback":fallback
                },
                "visual_state":{
                    "requested_action":visual.requested_action,
                    "resolved_action":visual.resolved_action,
                    "modifiers":modifiers,
                    "phase":visual.phase(),
                    "frame":visual.frame(),
                    "preempted_count":visual.preempted_count
                },
                "motion":{
                    "start_handle":motion.start_handle,
                    "end_handle":motion.end_handle,
                    "arc_size":motion.arc_size,
                    "arc_flow":motion.arc_flow,
                    "spring":motion.spring,
                    "glide_duration_ms":motion.glide_duration_ms,
                    "dwell_after_click_ms":motion.dwell_after_click_ms,
                    "idle_hide_ms":motion.idle_hide_ms,
                    "turn_radius":motion.turn_radius
                }
            }),
        )
    }
}

fn canonical_cursor_def(name: &str) -> ToolDef {
    let contract = cua_driver_contract::tool_contract(name)
        .unwrap_or_else(|| panic!("missing canonical cursor contract for {name}"));
    ToolDef::from_contract(&contract)
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
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        // Check X11 connectivity (required for window enumeration and input injection).
        let x11_ok = tokio::task::spawn_blocking(|| {
            x11rb::rust_connection::RustConnection::connect(None).is_ok()
        })
        .await
        .unwrap_or(false);

        // Check AT-SPI: not merely "is there a session bus?" but "does
        // org.a11y.Bus actually answer on it?" — the previous env-var-or-
        // /run/user heuristic false-passed exactly the headless/container case
        // (/run/user exists, but no a11y bus → empty trees). Probe for real.
        let dbus_address = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
        let atspi_ok = tokio::task::spawn_blocking(crate::health_report::probe_a11y_bus)
            .await
            .unwrap_or(false);

        let wayland_display = std::env::var("WAYLAND_DISPLAY").ok();
        let atspi_status = if atspi_ok {
            match &dbus_address {
                Some(a) => format!("✅ org.a11y.Bus reachable (DBUS_SESSION_BUS_ADDRESS={a})"),
                None => "✅ org.a11y.Bus reachable".to_string(),
            }
        } else if dbus_address.is_none() {
            "❌ no session bus (DBUS_SESSION_BUS_ADDRESS unset and none auto-discovered) — \
             AT-SPI trees will be empty; start the daemon inside the desktop session"
                .to_string()
        } else {
            "❌ session bus present but org.a11y.Bus has no owner — enable accessibility \
             (gsettings set org.gnome.desktop.interface toolkit-accessibility true) / \
             start at-spi-bus-launcher"
                .to_string()
        };
        let status_text = format!(
            "X11 display: {}\nWayland: {}\nAT-SPI (D-Bus): {}\nXSendEvent injection: {}",
            if x11_ok { "✅ connected" } else { "❌ DISPLAY not set or X11 unavailable" },
            match &wayland_display {
                Some(s) if crate::wayland::wayland_enabled() =>
                    format!("✅ native Wayland session (WAYLAND_DISPLAY={s}) — experimental backend ENABLED"),
                Some(s) => format!(
                    "⚠️  native Wayland session (WAYLAND_DISPLAY={s}) — experimental backend OFF; \
                     set {}=1 to enable it",
                    crate::wayland::ENABLE_WAYLAND_ENV
                ),
                None => "❌ not a Wayland session".to_string(),
            },
            atspi_status,
            if x11_ok { "✅ available" } else { "❌ requires X11" }
        );
        ToolResult::text(status_text)
            .with_structured(json!({ "x11": x11_ok, "wayland": wayland_display.is_some(), "wayland_enabled": crate::wayland::wayland_enabled(), "atspi": atspi_ok, "dbus_session_bus_address": dbus_address, "xsend_event": x11_ok }))
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
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        let (agent_view_enabled, agent_view_geometry) =
            pip_preview::read_agent_view_keys_from_file();
        ToolResult::text("cua-driver-rs configuration").with_structured(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "source_sha": option_env!("CUA_DRIVER_SOURCE_SHA"),
            "platform": "linux",
            "capture_mode": cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_view": agent_view_enabled,
            "agent_view_geometry": agent_view_geometry
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
            description: "Update cua-driver-rs configuration. capture_mode / \
                max_image_dimension take effect immediately.\n\n\
                Two input shapes (both accepted, matching Windows/Swift):\n\
                - **{key, value}** (preferred): `{\"key\": \"max_image_dimension\", \"value\": 800}` \
                  — single leaf write.\n\
                - **Legacy per-field**: `{\"capture_mode\": \"som\", \"max_image_dimension\": 0}`.\n\n\
                The agent_view keys persist to ~/.cua-driver/config.json and apply on next \
                daemon restart (Agent View is initialised once at startup).".into(),
            input_schema: json!({"type":"object","properties":{
                "key":{"type":"string","description":"Name of a single config field to write ({key, value} shape). Pair with `value`."},
                "value":{"description":"New value for `key`. JSON type depends on the key."},
                "capture_mode":{"type":"string","enum":["ax","vision"],"description":"Legacy per-field shape. Default capture mode for get_window_state. (\"som\"/\"screenshot\" still decode as deprecated aliases.)"},
                "max_image_dimension":{"type":"integer","description":"Legacy per-field shape. Max dimension for screenshot resizing (0 = no limit)."},
                "agent_view":{"type":"boolean","description":"Enable the multi-target Agent View (applies next restart)."},
                "agent_view_geometry":{"type":"string","description":"Agent View size + optional position in `WxH` or `WxH+X+Y` form."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        if args.get("capture_scope").is_some()
            || args.get("key").and_then(Value::as_str) == Some("capture_scope")
        {
            return ToolResult::error(
                "config key 'capture_scope' is retired; select a window or desktop target on each action",
            )
            .with_structured(json!({
                "code": "config_key_retired",
                "key": "capture_scope",
                "replacement": "action.target",
            }));
        }
        let mut cfg = self.state.config.write().unwrap();
        let mut parts = Vec::new();
        // {key, value} shape (what the Swift/macOS and Windows callers send).
        // Linux previously read only the legacy per-field keys below, so a
        // `{"key":"max_image_dimension","value":800}` write was silently
        // dropped (issue #1923). Dispatch on `key` to the same fields.
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
                        parts.push(format!("capture_mode={s}"));
                    }
                    None => return ToolResult::error(format!("`capture_mode` must be a string, got {val}.")),
                },
                "max_image_dimension" => match val.as_u64() {
                    Some(n) => {
                        cfg.max_image_dimension = n as u32;
                        if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(n)) {
                            tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
                        }
                        parts.push(format!("max_image_dimension={n}"));
                    }
                    None => return ToolResult::error(format!("`max_image_dimension` must be an integer, got {val}.")),
                },
                "agent_view" => match val.as_bool() {
                    Some(b) => {
                        if let Err(e) = pip_preview::write_config_key("agent_view", Value::Bool(b)) {
                            return ToolResult::error(format!("failed to persist agent_view: {e}"));
                        }
                        parts.push(format!("agent_view={b} (next restart)"));
                    }
                    None => return ToolResult::error(format!("`agent_view` must be a boolean, got {val}.")),
                },
                "agent_view_geometry" => match val.as_str() {
                    Some(s) => {
                        if pip_preview::PipGeometry::parse(s).is_none() {
                            return ToolResult::error(format!(
                                "agent_view_geometry `{s}` is not a valid WxH or WxH+X+Y string"
                            ));
                        }
                        if let Err(e) = pip_preview::write_config_key("agent_view_geometry", Value::String(s.to_owned())) {
                            return ToolResult::error(format!("failed to persist agent_view_geometry: {e}"));
                        }
                        parts.push(format!("agent_view_geometry={s} (next restart)"));
                    }
                    None => return ToolResult::error(format!("`agent_view_geometry` must be a string, got {val}.")),
                },
                other => return ToolResult::error(format!(
                    "Unknown config key `{other}`. Known: capture_mode, max_image_dimension, agent_view, agent_view_geometry."
                )),
            }
        }
        // Legacy per-field shape.
        if let Some(mode) = args.opt_str("capture_mode") {
            if let Err(e) =
                pip_preview::write_config_key("capture_mode", Value::String(mode.clone()))
            {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
            parts.push(format!("capture_mode={mode}"));
            cfg.capture_mode = mode;
        }
        if let Some(dim) = args.opt_u64("max_image_dimension") {
            cfg.max_image_dimension = dim as u32;
            if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(dim)) {
                tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
            }
            parts.push(format!("max_image_dimension={dim}"));
        }
        if let Some(enabled) = args.get("agent_view").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("agent_view", Value::Bool(enabled)) {
                return ToolResult::error(format!("failed to persist agent_view: {e}"));
            }
            parts.push(format!("agent_view={enabled} (next restart)"));
        }
        if let Some(geom) = args.opt_str("agent_view_geometry") {
            if pip_preview::PipGeometry::parse(&geom).is_none() {
                return ToolResult::error(format!(
                    "agent_view_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) =
                pip_preview::write_config_key("agent_view_geometry", Value::String(geom.clone()))
            {
                return ToolResult::error(format!("failed to persist agent_view_geometry: {e}"));
            }
            parts.push(format!("agent_view_geometry={geom} (next restart)"));
        }
        let msg = if parts.is_empty() {
            "Config unchanged (no known parameters).".to_owned()
        } else {
            format!("Config updated: {}", parts.join(", "))
        };
        let (agent_view_enabled, agent_view_geometry) =
            pip_preview::read_agent_view_keys_from_file();
        ToolResult::text(msg).with_structured(json!({
            "capture_mode": cfg.capture_mode,
            "max_image_dimension": cfg.max_image_dimension,
            "agent_view": agent_view_enabled,
            "agent_view_geometry": agent_view_geometry
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
                crate::proc_fs::list_processes(),
                crate::x11::list_windows(None),
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
            let cmd = if p.cmdline.is_empty() {
                p.name.clone()
            } else {
                p.cmdline.clone()
            };
            lines.push(format!("- {} (pid {})", cmd, p.pid));
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
                    "- pid={:?} {} [window_id: {}] {}x{}+{}+{}",
                    w.pid, title, w.xid, w.width, w.height, w.x, w.y
                ));
            }
            lines.push(
                "→ Call get_window_state(pid, window_id) to inspect a window's UI.".to_owned(),
            );
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
        use cua_driver_core::tool_args::ArgsExt;
        let xid = match args.require_u64("window_id") {
            Ok(v) => v,
            Err(e) => return e,
        };
        let pid = args.opt_u64("pid").map(|v| v as u32);
        let x1 = match args.opt_f64("x1") {
            Some(v) => v,
            None => return ToolResult::error("Missing x1"),
        };
        let y1 = match args.opt_f64("y1") {
            Some(v) => v,
            None => return ToolResult::error("Missing y1"),
        };
        let x2 = match args.opt_f64("x2") {
            Some(v) => v,
            None => return ToolResult::error("Missing x2"),
        };
        let y2 = match args.opt_f64("y2") {
            Some(v) => v,
            None => return ToolResult::error("Missing y2"),
        };
        if x2 <= x1 || y2 <= y1 {
            return ToolResult::error("x2 must be > x1 and y2 must be > y1");
        }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Route through the Wayland-aware window capture dispatcher so
            // pure-Wayland sessions surface a typed "per-window capture not
            // supported yet" error instead of accidentally calling the
            // X11-only path with a foreign-toplevel id.
            let png = crate::wayland::screenshot_window_dispatch(xid)?;
            cursor_overlay::capture_utils::crop_png_to_jpeg(&png, x1, y1, x2, y2, 500)
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
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!(
                            "Zoom ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG."
                        )),
                    ],
                    is_error: None,
                    // Surface 7: `mime_type` mirrors the MCP image part's `mimeType`
                    // onto the structured payload (additive — `format` stays).
                    structured_content: Some(json!({
                        "width": w, "height": h, "format": "jpeg",
                        "mime_type": "image/jpeg"
                    })),
                    action_record: None,
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
                    "element_index": cua_driver_core::tool_schema::element_index_schema(),
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
        // Same trailing-protocol-tag scrub as TypeTextTool — see
        // cua_driver_core::text_sanitize for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let delay_ms = args.u64_or("delay_ms", 30);
        let xid_opt = args.opt_u64("window_id");
        let xid = match xid_opt {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
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
            if crate::wayland::wayland_input_enabled() {
                // Per-char `wtype` loop with the requested delay — mirrors the
                // X11 XSendEvent per-char path. Sleeping here is fine because
                // we're inside spawn_blocking.
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    let s = ch.encode_utf8(&mut buf);
                    crate::wayland::type_text(xid, s)?;
                    if delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    }
                }
                return Ok(());
            }
            crate::input::send_type_text_with_delay(xid, &text, delay_ms)
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

pub struct KillAppTool;
static KILL_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for KillAppTool {
    fn def(&self) -> &ToolDef {
        KILL_DEF.get_or_init(|| ToolDef {
            name: "kill_app".into(),
            description: "Force-terminate a process by pid (kill -9 equivalent on Linux). \
                Use as escalation when the cooperative close path failed to make the process \
                exit. Unsaved state is lost — prefer the cooperative path first."
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

    async fn protected_resource_scope(
        &self,
        adapter_id: &str,
        args: &Value,
    ) -> Result<Option<Value>, String> {
        if adapter_id != "process_control" {
            return Ok(None);
        }
        use cua_driver_core::browser::platform::BrowserPlatform;
        let pid = args
            .get("pid")
            .and_then(Value::as_i64)
            .filter(|pid| *pid > 0)
            .ok_or_else(|| "kill_app requires a positive integer pid".to_owned())?;
        let fingerprint = crate::browser_platform::LinuxBrowserPlatform
            .process_fingerprint(pid)
            .await
            .map_err(|error| error.message)?;
        Ok(Some(json!({
            "kind": "process_instance",
            "fingerprint": fingerprint,
        })))
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        if let Some(expected) = args.get("_protected_process_fingerprint") {
            let current = match self
                .protected_resource_scope("process_control", &args)
                .await
            {
                Ok(Some(scope)) => scope["fingerprint"].clone(),
                Ok(None) => Value::Null,
                Err(message) => return kill_app_stale_process_refusal(message),
            };
            if current != *expected {
                return kill_app_stale_process_refusal(
                    "the process identity changed at the termination boundary".to_owned(),
                );
            }
        }
        let pid_i = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) if p > 0 && p <= i32::MAX as i64 => p as i32,
            Some(_) => {
                return ToolResult::error("kill_app: `pid` must be a positive integer".to_string())
            }
            None => {
                return ToolResult::error(
                    "kill_app: missing required integer field `pid`".to_string(),
                )
            }
        };
        // SAFETY: libc::kill is a thin syscall wrapper, no thread-safety concerns.
        let rc = unsafe { libc::kill(pid_i, libc::SIGKILL) };
        if rc == 0 {
            ToolResult::text(format!("✅ Sent SIGKILL to pid {pid_i}."))
        } else {
            let err = std::io::Error::last_os_error();
            ToolResult::error(format!(
                "kill_app: kill(pid={pid_i}, SIGKILL) failed: {err}. \
                 The process may not exist, or the daemon lacks permission to signal it."
            ))
        }
    }
}

fn kill_app_stale_process_refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": {
            "code": "protected_resource_scope_stale",
            "message": message,
        }
    }))
}

// ── invoke_menu (Linux) ──────────────────────────────────────────────────────

pub struct InvokeMenuTool;

static INVOKE_MENU_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

fn normalized_menu_path(path: Vec<String>) -> Result<Vec<String>, String> {
    if path.is_empty() || path.len() > 16 {
        return Err("invoke_menu: path must contain between 1 and 16 segments".into());
    }
    path.into_iter()
        .enumerate()
        .map(|(index, segment)| {
            let segment = segment.trim();
            if segment.is_empty() {
                Err(format!("invoke_menu: path segment {index} is empty"))
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect()
}

fn menu_refusal(message: String) -> ToolResult {
    ToolResult::error(message.clone()).with_structured(json!({
        "status": "refused",
        "refusal": { "code": "menu_path_unavailable", "message": message }
    }))
}

#[async_trait]
impl Tool for InvokeMenuTool {
    fn def(&self) -> &ToolDef {
        INVOKE_MENU_DEF.get_or_init(|| {
            let contract =
                cua_driver_contract::tool_contract("invoke_menu").expect("invoke_menu contract");
            ToolDef {
                name: contract.name,
                description: contract.description,
                input_schema: contract.input_schema,
                read_only: contract.annotations.read_only,
                destructive: contract.annotations.destructive,
                idempotent: contract.annotations.idempotent,
                open_world: contract.annotations.open_world,
            }
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::action_record::{
            ActionEffect, ActionEvidence, ActionExecutionRecord, ActionTransport, ActualDelivery,
            EvidenceKind, RequestedDelivery,
        };

        let input: InvokeMenuInput = match parse_typed_input("invoke_menu", args) {
            Ok(input) => input,
            Err(result) => return result,
        };
        let path = match normalized_menu_path(input.path) {
            Ok(path) => path,
            Err(error) => return menu_refusal(error),
        };
        let pid = input.pid;
        let window_id = input.window_id;
        if !crate::wayland::list_windows_dispatch(Some(pid))
            .iter()
            .any(|window| window.xid == window_id && window.pid == Some(pid))
        {
            return menu_refusal("invoke_menu: window_id does not belong to pid".into());
        }

        let activation = if crate::wayland::is_wayland() {
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::activate_window_for_input_target(window_id, Some(pid))
            })
            .await;
            match result {
                Ok(Ok(())) => None,
                Ok(Err(error)) => {
                    return menu_refusal(format!(
                        "invoke_menu: target window could not be activated: {error}"
                    ))
                }
                Err(error) => {
                    return menu_refusal(format!("invoke_menu: activation task failed: {error}"))
                }
            }
        } else {
            match tokio::task::spawn_blocking(move || {
                crate::input::x11_activate_window_persistent(window_id)
            })
            .await
            {
                Ok(Ok(prior)) => Some(prior),
                Ok(Err(error)) => {
                    return menu_refusal(format!(
                        "invoke_menu: target window could not be activated: {error}"
                    ))
                }
                Err(error) => {
                    return menu_refusal(format!("invoke_menu: activation task failed: {error}"))
                }
            }
        };

        let outcome = tokio::task::spawn_blocking(move || {
            let result = crate::atspi::native::invoke_menu_path(pid, &path);
            if let Some(Some(prior_window)) = activation {
                let _ = crate::input::x11_activate_window_persistent(prior_window);
            }
            result
        })
        .await;

        match outcome {
            Ok(Ok(())) => ToolResult::text(
                "Resolved the live native menu path and dispatched its final accessibility action; verify the command's semantic effect from fresh state.",
            )
            .with_action_record(
                ActionExecutionRecord::builder(
                    ActionEffect::Unverifiable,
                    ActionTransport::LinuxAtSpiAction,
                    RequestedDelivery::Foreground,
                )
                .actual_delivery(ActualDelivery::Foreground)
                .evidence(ActionEvidence {
                    kind: EvidenceKind::NativeApiResult,
                    detail: "Every menu hop resolved uniquely and AT-SPI accepted the final action"
                        .into(),
                })
                .build()
                .expect("invoke_menu record is valid"),
            ),
            Ok(Err(error)) => menu_refusal(format!("invoke_menu: {error}")),
            Err(error) => menu_refusal(format!("invoke_menu: blocking task failed: {error}")),
        }
    }
}

// ── set_window_frame (Linux) ─────────────────────────────────────────────────

pub struct SetWindowFrameTool;

static SET_WINDOW_FRAME_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for SetWindowFrameTool {
    fn def(&self) -> &ToolDef {
        SET_WINDOW_FRAME_DEF.get_or_init(|| {
            let contract = cua_driver_contract::tool_contract("set_window_frame")
                .expect("set_window_frame contract");
            ToolDef {
                name: contract.name,
                description: contract.description,
                input_schema: contract.input_schema,
                read_only: contract.annotations.read_only,
                destructive: contract.annotations.destructive,
                idempotent: contract.annotations.idempotent,
                open_world: contract.annotations.open_world,
            }
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_contract::SetWindowFrameInput;
        use cua_driver_core::action_record::{
            effect_from_value_readback, ActionEvidence, ActionExecutionRecord, ActionTransport,
            ActualDelivery, EvidenceKind, RequestedDelivery,
        };

        let input: SetWindowFrameInput =
            match cua_driver_core::tool_args::parse_typed_input("set_window_frame", args) {
                Ok(input) => input,
                Err(result) => return result,
            };
        if crate::wayland::is_wayland() {
            return ToolResult::error(
                "set_window_frame: this Wayland compositor exposes no protocol that permits a client to set another top-level window's geometry",
            );
        }
        let values = [input.x, input.y, input.width, input.height];
        if values.iter().any(|value| !value.is_finite())
            || input.width <= 0.0
            || input.height <= 0.0
        {
            return ToolResult::error(
                "set_window_frame: x/y must be finite and width/height must be finite positive numbers",
            );
        }
        let to_i32 = |name: &str, value: f64| -> Result<i32, String> {
            let rounded = value.round();
            if rounded < i32::MIN as f64 || rounded > i32::MAX as f64 {
                Err(format!("{name} is out of range for an X11 coordinate"))
            } else {
                Ok(rounded as i32)
            }
        };
        let to_u32 = |name: &str, value: f64| -> Result<u32, String> {
            let rounded = value.round();
            if rounded < 1.0 || rounded > u32::MAX as f64 {
                Err(format!("{name} is out of range for an X11 size"))
            } else {
                Ok(rounded as u32)
            }
        };
        let x = match to_i32("x", input.x) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let y = match to_i32("y", input.y) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let width = match to_u32("width", input.width) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let height = match to_u32("height", input.height) {
            Ok(value) => value,
            Err(error) => return ToolResult::error(format!("set_window_frame: {error}")),
        };
        let window_id = input.window_id;
        let pid = input.pid;
        let outcome = tokio::task::spawn_blocking(move || {
            let before = crate::x11::list_windows(Some(pid))
                .into_iter()
                .find(|window| window.xid == window_id)
                .map(|window| (window.x, window.y, window.width, window.height));
            let (observed, confirmed, mutation_error) =
                crate::x11::set_window_frame(window_id, pid, x, y, width, height)
                    .map_err(|error| error.to_string())?;
            let observed_frame =
                observed.map(|window| (window.x, window.y, window.width, window.height));
            let changed = observed_frame.is_some_and(|observed| before != Some(observed));
            Ok::<_, String>((observed_frame, confirmed, changed, mutation_error))
        })
        .await;
        let (observed, confirmed, changed, mutation_error) = match outcome {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => return ToolResult::error(format!("set_window_frame: {error}")),
            Err(error) => {
                return ToolResult::error(format!(
                    "set_window_frame: blocking task failed: {error}"
                ));
            }
        };
        let effect = effect_from_value_readback(confirmed, changed, observed.is_some());
        let requested = (x, y, width, height);
        let mut record = ActionExecutionRecord::builder(
            effect,
            ActionTransport::LinuxX11ConfigureWindow,
            RequestedDelivery::NotApplicable,
        )
        .actual_delivery(ActualDelivery::NotApplicable)
        .detail(format!(
            "requested={requested:?} observed={observed:?} mutation_error={mutation_error:?}"
        ));
        if observed.is_some() {
            record = record.evidence(ActionEvidence {
                kind: EvidenceKind::ValueReadback,
                detail: if confirmed {
                    "X11 geometry readback matched the requested frame".into()
                } else {
                    "X11 geometry readback did not match the requested frame".into()
                },
            });
        }
        ToolResult::text(if confirmed {
            "Set and verified the requested window frame."
        } else if observed.is_none() {
            "The native window mutation was attempted, but its resulting frame could not be read back."
        } else {
            "The window frame did not settle at the requested geometry."
        })
        .with_action_record(record.build().expect("set_window_frame record is valid"))
    }
}

// ── bring_to_front (Linux) ───────────────────────────────────────────────────

pub struct BringToFrontTool;

static BTF_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        BTF_DEF.get_or_init(|| ToolDef {
            name: "bring_to_front".into(),
            description:
                "Persistently activate a window so subsequent input lands on it. \
                 This deliberately breaks the no-foreground contract and is not part \
                 of the normal input ladder. For an ordinary `background_unavailable` \
                 response, retry only the refused action with \
                 `delivery_mode:\"foreground\"`; the input tool performs its own \
                 activate, act, and restore sequence. Use `bring_to_front` only for a \
                 focus-proxy surface that must remain foreground across multiple calls, \
                 such as a remote desktop session, or when repeated action-scoped \
                 activation prevents the remote surface from accepting input. \
                 X11: EWMH _NET_ACTIVE_WINDOW activation (the `wmctrl -a` equivalent, \
                 with proper timestamp handling to beat focus-stealing prevention). \
                 Wayland: activates through a target-addressable compositor adapter \
                 (wlroots foreign-toplevel or the GNOME Shell helper) and refuses \
                 when the compositor offers no safe adapter. Matches the macOS / \
                 Windows bring_to_front rung."
                .into(),
            input_schema: serde_json::json!({
                "type":"object","required":["pid"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer","description":"X11 window id (xid) to activate. If omitted, the first window of `pid` is used."}
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        if crate::wayland::is_wayland() {
            let window_id = match args.opt_u64("window_id") {
                Some(window_id) => window_id,
                None => match crate::wayland::list_windows_dispatch(Some(pid)).first() {
                    Some(window) => window.xid,
                    None => {
                        return ToolResult::error(format!(
                            "bring_to_front: no window_id given and no Wayland windows found for pid {pid}."
                        ))
                    }
                },
            };
            let result = tokio::task::spawn_blocking(move || {
                crate::wayland::activate_window_for_input_target(window_id, Some(pid))
            })
            .await;
            return match result {
                Ok(Ok(())) => {
                    ToolResult::text(format!("Brought Wayland window {window_id} to front."))
                        .with_structured(serde_json::json!({
                            "window_id": window_id,
                            "platform": "linux",
                            "session": "wayland",
                        }))
                }
                Ok(Err(error)) => ToolResult::error(error.to_string()),
                Err(error) => ToolResult::error(format!("Task error: {error}")),
            };
        }

        // X11: resolve the target xid (window_id, else first window for pid).
        let xid = match args.opt_u64("window_id") {
            Some(x) => x,
            None => {
                let windows =
                    tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                        .await
                        .unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => {
                        return ToolResult::error(format!(
                        "bring_to_front: no window_id given and no windows found for pid {pid}."
                    ))
                    }
                }
            }
        };
        let r =
            tokio::task::spawn_blocking(move || crate::input::x11_activate_window_persistent(xid))
                .await;
        match r {
            Ok(Ok(prior)) => ToolResult::text(format!(
                "✅ Brought window {xid} to front (X11 _NET_ACTIVE_WINDOW)."
            ))
            .with_structured(serde_json::json!({
                "window_id": xid,
                "prior_active": prior,
                "platform": "linux",
            })),
            Ok(Err(e)) => ToolResult::error(format!("bring_to_front failed: {e}")),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// ── registry ─────────────────────────────────────────────────────────────────

pub fn build_registry(compat: bool) -> ToolRegistry {
    build_registry_with_provider(compat, None)
}

pub fn build_registry_with_provider(
    compat: bool,
    provider: Option<std::sync::Arc<dyn cua_driver_core::consent::ProtectedConsentProvider>>,
) -> ToolRegistry {
    let state = ToolState::new();
    let cursor_outcome_reader = {
        let cursor_registry = state.cursor_registry.clone();
        cua_driver_core::session::register_scoped_cursor_outcome_reader(std::sync::Arc::new(
            move |session_id| {
                let state = cursor_registry.get(session_id);
                let motion_customized = state.is_some()
                    && crate::overlay::current_motion_for(session_id)
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
                        crate::overlay::is_visible_for_session(session_id),
                        Some(state.config.theme_id.as_str()),
                        motion_customized,
                        active_cursor_count,
                    ),
                    None => cua_driver_core::session::bounded_cursor_outcome(
                        false,
                        false,
                        false,
                        None,
                        false,
                        active_cursor_count,
                    ),
                }
            },
        ))
    };
    let session_end_hook = {
        let cursor_registry = state.cursor_registry.clone();
        let state_for_session_end = state.clone();
        cua_driver_core::session::register_scoped_session_end_hook(move |session_id| {
            cursor_registry.remove(session_id);
            crate::overlay::remove_cursor(session_id.to_owned());
            state_for_session_end
                .mouse_hold
                .lock()
                .unwrap()
                .remove(session_id);
            crate::input::forget_master_pointer(session_id);
        })
    };
    let session_revive_hook =
        cua_driver_core::session::register_scoped_session_revive_hook(move |session_id| {
            crate::overlay::revive_cursor(session_id.to_owned());
        });
    let mut r = ToolRegistry::new_with_protected_consent_provider(provider);
    r.retain_cursor_outcome_reader(cursor_outcome_reader);
    r.retain_session_end_hook(session_end_hook);
    r.retain_session_revive_hook(session_revive_hook);
    if let Some(runtime_scope) = cua_driver_core::tool::current_dispatch_runtime_scope() {
        let prefix = format!("__cua_runtime_{runtime_scope}:");
        let cursor_registry = state.cursor_registry.clone();
        r.retain_runtime_cleanup(move || {
            for cursor in cursor_registry
                .all_states()
                .into_iter()
                .filter(|cursor| cursor.config.cursor_id.starts_with(&prefix))
            {
                cursor_registry.remove(&cursor.config.cursor_id);
                crate::overlay::remove_cursor(cursor.config.cursor_id);
            }
        });
    }
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool {
        state: state.clone(),
    }));
    r.register(Box::new(
        cua_driver_core::expectation::VerifyStateTool::new(std::sync::Arc::new(
            cua_driver_core::expectation::ToolObservationProvider::new(
                std::sync::Arc::new(ListWindowsTool),
                std::sync::Arc::new(GetWindowStateTool {
                    state: state.clone(),
                }),
            ),
        )),
    ));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    let pid_window_candidates: WindowTargetCandidates = Arc::new(pid_window_target_candidates);
    r.register(pid_window_guarded(BringToFrontTool, &pid_window_candidates));
    r.register(Box::new(SetWindowFrameTool));
    r.register(Box::new(InvokeMenuTool));
    r.register(pid_window_guarded(
        ClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        DoubleClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        RightClickTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        DragTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseButtonDownTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseDragTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        MouseButtonUpTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(Box::new(ParallelMouseDragTool {
        state: state.clone(),
    }));
    r.register(pid_window_guarded(
        TypeTextTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        PressKeyTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        HotkeyTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        SetValueTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    r.register(pid_window_guarded(
        ScrollTool {
            state: state.clone(),
        },
        &pid_window_candidates,
    ));
    cua_driver_core::clipboard::register_clipboard_tools(
        &mut r,
        Arc::new(crate::clipboard::LinuxClipboard::new()),
    );
    // `screenshot` removed - see the matching comment in
    // platform-windows/src/tools/impl_.rs::build_registry. Canonical
    // screenshot path is `get_window_state` (it always returns a screenshot now).
    let _ = compat;
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetDesktopStateTool));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorEnabledV2Tool {
        state: state.clone(),
    }));
    r.register(Box::new(SetAgentCursorMotionV2Tool));
    r.register(Box::new(GetAgentCursorStateV2Tool));
    r.register(Box::new(SetAgentCursorThemeTool {
        state: state.clone(),
    }));
    r.register(Box::new(CheckPermissionsTool));
    // `health_report` — single-call cross-platform driver diagnostics.
    // Stable schema_version="1" contract for downstream consumers. Linux skips
    // tcc_* and bundle_identity with "not applicable on Linux".
    r.register(Box::new(
        cua_driver_core::health_report::HealthReportTool::new(std::sync::Arc::new(
            crate::health_report::LinuxHealthProvider,
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
    // `type_text_chars` is a deprecated invoke-time alias for `type_text`.
    // Keep it out of tools/list, matching the macOS and Windows registries.
    let _: &TypeTextCharsTool = &TypeTextCharsTool;
    // Cross-platform `page` tool definition lives in mcp-server; Linux plugs
    // in its AT-SPI + CDP backend here.
    r.register(Box::new(cua_driver_core::page::PageTool::new(Arc::new(
        super::page::LinuxPageBackend::new(),
    ))));
    let browser_engine = cua_driver_core::browser::BrowserEngine::new_with_runtime_services(
        Arc::new(crate::browser_platform::LinuxBrowserPlatform),
        r.approval_broker(),
        r.protected_resource_ownership(),
    );
    cua_driver_core::browser::register_browser_tools(&browser_engine, &mut r);
    r.register_recording_tools();
    r.register_session_tools();
    r
}

#[cfg(test)]
mod click_button_schema_tests {
    use super::{chromium_background_must_refuse, maps_indicate_gtk, ClickTool};
    use cua_driver_core::tool::Tool;

    /// Surface 5: schema must advertise the three canonical button values and
    /// describe the back-compat default. Linux already routed button=middle/right
    /// pre-Surface-5; this freezes the schema shape so the contract can't drift.
    #[test]
    fn schema_advertises_button_enum_and_description() {
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
        let desc = button
            .get("description")
            .and_then(|v| v.as_str())
            .expect("button.description present");
        let lc = desc.to_ascii_lowercase();
        assert!(lc.contains("left"), "description should mention default");
        assert!(
            lc.contains("wayland"),
            "description should call out wayland fallback"
        );
    }

    #[test]
    fn chromium_background_requires_focus_free_inject_mode() {
        assert!(chromium_background_must_refuse(false, false, true));
        assert!(!chromium_background_must_refuse(false, true, true));
        assert!(!chromium_background_must_refuse(true, false, true));
        assert!(!chromium_background_must_refuse(false, false, false));
    }

    #[test]
    fn gtk_process_maps_are_detected_without_matching_unrelated_libraries() {
        assert!(maps_indicate_gtk(
            "7f00-7f01 r-xp /usr/lib/x86_64-linux-gnu/libgtk-3.so.0.2404.32"
        ));
        assert!(maps_indicate_gtk(
            "7f00-7f01 r-xp /nix/store/hash-gtk4/lib/libgtk-4.so.1"
        ));
        assert!(!maps_indicate_gtk(
            "7f00-7f01 r-xp /usr/lib/x86_64-linux-gnu/libgdk_pixbuf-2.0.so"
        ));
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
            "/usr/bin/chrome --remote-debugging-port 9222"
        ));
        assert!(!contains_remote_debugging_flag(
            "--user-data-dir=/tmp/profile"
        ));
    }
}

#[cfg(test)]
mod pid_window_target_tests {
    use super::*;
    use cua_driver_core::window_target::{resolve_pid_window_target, PidWindowTargetResolution};

    fn window(xid: u64, pid: u32) -> crate::x11::WindowInfo {
        crate::x11::WindowInfo {
            xid,
            pid: Some(pid),
            app_name: "editor".into(),
            title: format!("Document {xid}"),
            is_on_screen: true,
            z_index: Some(1),
            x: 0,
            y: 0,
            width: 640,
            height: 480,
        }
    }

    #[test]
    fn same_pid_sibling_windows_are_ambiguous() {
        let candidates =
            window_target_candidates_for_pid([window(7, 42), window(8, 42), window(9, 99)], 42);
        assert!(matches!(
            resolve_pid_window_target(candidates),
            PidWindowTargetResolution::Ambiguous(windows)
                if windows.iter().map(|window| window.window_id).collect::<Vec<_>>() == [7, 8]
        ));
    }
}

#[cfg(test)]
mod session_cursor_target_tests {
    use super::{
        choose_keyboard_cursor_target, cursor_control_scope, named_session_cursor_key,
        reveal_pointer_action_for, CursorControlScope, ToolState,
    };
    use serde_json::json;

    #[test]
    fn lifecycle_owned_sessions_opt_into_keyboard_cursor_positioning() {
        assert_eq!(
            named_session_cursor_key(&json!({"session": "editing-run"})).as_deref(),
            Some("editing-run")
        );
        assert_eq!(
            named_session_cursor_key(&json!({"cursor_id": "legacy"})),
            None
        );
        assert_eq!(
            named_session_cursor_key(&json!({"_session_id": "implicit"})).as_deref(),
            Some("implicit")
        );
        assert_eq!(named_session_cursor_key(&json!({})), None);
    }

    #[test]
    fn keyboard_cursor_uses_explicit_then_remembered_then_safe_seed() {
        let explicit = Some((10.0, 20.0));
        let remembered = Some((30.0, 40.0));
        let window_center = Some((50.0, 60.0));
        let current_pointer = Some((70.0, 80.0));

        assert_eq!(
            choose_keyboard_cursor_target(explicit, remembered, window_center, current_pointer),
            explicit
        );
        assert_eq!(
            choose_keyboard_cursor_target(None, remembered, window_center, current_pointer),
            remembered
        );
        assert_eq!(
            choose_keyboard_cursor_target(None, None, window_center, current_pointer),
            window_center
        );
        assert_eq!(
            choose_keyboard_cursor_target(None, None, None, current_pointer),
            current_pointer
        );
    }

    #[test]
    fn invalid_coordinates_do_not_poison_session_position_reuse() {
        assert_eq!(
            choose_keyboard_cursor_target(Some((f64::NAN, 1.0)), Some((12.0, 34.0)), None, None,),
            Some((12.0, 34.0))
        );
    }

    #[test]
    fn real_pointer_control_requires_explicit_desktop_scope() {
        assert_eq!(cursor_control_scope(&json!({})), CursorControlScope::Agent);
        assert_eq!(
            cursor_control_scope(&json!({"scope": "window"})),
            CursorControlScope::Agent
        );
        assert_eq!(
            cursor_control_scope(&json!({"scope": "desktop"})),
            CursorControlScope::Desktop
        );
    }

    #[tokio::test]
    async fn pointer_position_survives_an_unavailable_overlay() {
        let state = ToolState::new();
        reveal_pointer_action_for(&state, "no-renderer", 123.0, 456.0, true).await;

        let cursor = state
            .cursor_registry
            .get("no-renderer")
            .expect("pointer action records its position independently of rendering");
        assert!(cursor.config.enabled, "input must revive its agent cursor");
        assert_eq!(cursor.x.zip(cursor.y), Some((123.0, 456.0)));
    }
}

#[cfg(test)]
mod desktop_capture_frame_tests {
    use super::normalize_desktop_capture_for_action_frame;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let rgba = vec![0x7f; (width * height * 4) as usize];
        cua_driver_core::image_utils::encode_rgba_to_png(&rgba, width, height)
            .expect("encode fixture")
    }

    #[test]
    fn native_wayland_capture_is_resized_to_the_reported_action_frame() {
        let (normalized, width, height, scale) =
            normalize_desktop_capture_for_action_frame(png(3200, 2000), 1600, 1000)
                .expect("normalize 2x capture");

        assert_eq!((width, height), (1600, 1000));
        assert_eq!(
            cua_driver_core::image_utils::png_dimensions(&normalized).unwrap(),
            (1600, 1000)
        );
        assert_eq!(scale, 2.0);
    }

    #[test]
    fn nonuniform_capture_mapping_fails_instead_of_distorting_coordinates() {
        let error = normalize_desktop_capture_for_action_frame(png(3200, 2000), 1600, 1200)
            .expect_err("nonuniform mapping must fail closed");
        assert!(error.to_string().contains("cannot be mapped uniformly"));
    }
}
