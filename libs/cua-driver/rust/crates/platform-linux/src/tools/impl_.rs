//! Real Linux tool implementations (compiled only on Linux).

use async_trait::async_trait;
use cua_driver_core::{
    protocol::ToolResult,
    tool::{Tool, ToolDef, ToolRegistry},
    tool_args::ArgsExt,
};
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::atspi::ElementCache;
use cursor_overlay::CursorRegistry;

// ── DriverConfig + ResizeRegistry + ZoomRegistry ─────────────────────────────

#[derive(Clone)]
pub struct DriverConfig {
    pub capture_mode: String,
    pub capture_scope: String,
    pub max_image_dimension: u32,
}

impl Default for DriverConfig {
    fn default() -> Self { Self { capture_mode: "som".into(), capture_scope: "window".into(), max_image_dimension: 1568 } }
}

/// Load `DriverConfig` from `~/.cua-driver/config.json`, falling back to
/// defaults for any missing/malformed keys. Called once at `ToolState`
/// construction (i.e. on every fresh `cua-driver call` process) so that a
/// prior `set_config capture_mode=vision` survives across stateless one-shot
/// invocations — matching the macOS daemon's startup load. See #2008.
pub fn load_driver_config() -> DriverConfig {
    let mut cfg = DriverConfig::default();
    if let Some(v) = pip_preview::read_config_value("capture_mode").and_then(|v| v.as_str().map(str::to_owned)) {
        cfg.capture_mode = v;
    }
    if let Some(v) = pip_preview::read_config_value("capture_scope").and_then(|v| v.as_str().map(str::to_owned)) {
        cfg.capture_scope = v;
    }
    if let Some(v) = pip_preview::read_config_value("max_image_dimension").and_then(|v| v.as_u64()) {
        if let Ok(v32) = u32::try_from(v) { cfg.max_image_dimension = v32; }
    }
    cfg
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
    pub mouse_hold: std::sync::Mutex<std::collections::HashMap<String, MouseHoldState>>,
    pub config: Arc<RwLock<DriverConfig>>,
}

#[derive(Clone, Debug)]
pub struct MouseHoldState {
    pub cursor_id: String,
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
            description: "List Linux apps — both currently running and installed-but-not-running — \
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
                state — visibility, geometry, titles — call list_windows instead.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
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

            let mut consumed: std::collections::HashSet<usize> =
                std::collections::HashSet::new();
            let mut out = Vec::new();
            for p in &procs {
                let key_source = if !p.cmdline.is_empty() { &p.cmdline } else { &p.name };
                let basename = exec_basename(key_source);
                if basename.is_empty() { continue }
                let candidates = by_exe.get(&basename).map(|v| v.as_slice()).unwrap_or(&[]);
                let merged = disambiguate_installed_match(candidates, &installed, key_source);
                if let Some(idx) = merged { consumed.insert(idx); }
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
                        if !p.name.is_empty() { p.name.clone() } else { basename.clone() },
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
                if consumed.contains(&i) { continue }
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
    if candidates.is_empty() { return None; }
    if candidates.len() == 1 { return Some(candidates[0]); }

    // Look for an exact path match against the cmdline's first token.
    let proc_path = proc_key_source.split_whitespace().next().unwrap_or("");
    if !proc_path.is_empty() {
        if let Some(&idx) = candidates.iter().find(|&&i| installed[i].launch_path == proc_path) {
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
        if tok == "env" { continue }
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
            description: "List top-level X11 windows via _NET_CLIENT_LIST.".into(),
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
        let windows = tokio::task::spawn_blocking(move || crate::wayland::list_windows_dispatch(filter_pid)).await.unwrap_or_default();
        let mut lines = vec![format!("Found {} windows:", windows.len())];
        for w in &windows {
            lines.push(format!("  [xid={}] pid={:?} \"{}\" {}x{}+{}+{}",
                w.xid, w.pid, w.title, w.width, w.height, w.x, w.y));
        }
        let structured = json!({ "windows": windows.iter().map(window_record_json).collect::<Vec<_>>() });
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
/// The Linux `WindowInfo` struct (see `crate::x11::WindowInfo`) exposes
/// neither an app name nor a visibility flag, so `app_name` is an empty
/// string and `is_on_screen` defaults to `true` — the same best-effort
/// default the Windows backend uses.
fn window_record_json(w: &crate::x11::WindowInfo) -> Value {
    json!({
        "window_id": w.xid,
        "pid": w.pid,
        "app_name": "",
        "title": w.title,
        // Canonical cross-platform geometry (macOS/Windows parity).
        "bounds": { "x": w.x, "y": w.y, "width": w.width, "height": w.height },
        "is_on_screen": true,
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
            title: "Example".to_owned(),
            x: 10,
            y: 20,
            width: 300,
            height: 400,
        };
        let rec = window_record_json(&w);

        // Canonical cross-platform shape: nested `bounds` object.
        let bounds = rec.get("bounds").expect("record must carry a `bounds` object");
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
        assert_eq!(rec["app_name"], json!(""));
        assert_eq!(rec["is_on_screen"], json!(true));
        assert_eq!(rec["window_id"], json!(42));
        assert_eq!(rec["title"], json!("Example"));
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
                per indexed row with `element_index`, `role`, `label`, \
                `frame: {x,y,w,h}` when AT-SPI reports usable bounds, \
                `parent_index`, `depth`). The markdown `tree_markdown` stays \
                available and unchanged in shape for existing text-parsing \
                callers — but new fields will only be added to the structured \
                side.\n\n\
                Also captures a screenshot.\n\n\
                Optional `max_elements` / `max_depth` bound the AT-SPI walk to \
                mitigate context-window blow-up on Electron / large web apps \
                that produce 10k+ element trees (#22865). When applied, BOTH \
                the markdown and the structured elements are truncated \
                identically. Omit both for current default behaviour.".into(),
            input_schema: json!({"type":"object","required":["pid","window_id"],"properties":{
                "pid":{"type":"integer"},
                "window_id":{"type":"integer","description":"X11 XID from list_windows."},
                "capture_mode":{"type":"string","enum":["som","vision","ax"],
                    "description":"som=tree+screenshot (default), vision=screenshot only, ax=tree only."},
                "query":{"type":"string"},
                "max_elements":{"type":"integer","minimum":1,"description":"Cap on total AT-SPI nodes walked. Omit for the default (5 000). Lower for huge web/Electron trees (#22865)."},
                "max_depth":{"type":"integer","minimum":1,"description":"Cap on the AT-SPI tree walk depth. Omit for the default (uncapped). Lower for deeply nested apps (#22865)."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let xid = match args.require_u64("window_id") { Ok(v) => v, Err(e) => return e };
        let (default_mode, max_dim) = {
            let cfg = self.state.config.read().unwrap();
            (cfg.capture_mode.clone(), cfg.max_image_dimension)
        };
        let capture_mode = args.str_or("capture_mode", &default_mode);
        let query = args.opt_str("query");
        // Optional caps — when omitted, the AT-SPI walker uses its built-in
        // defaults (#22865).
        let max_elements = args.get("max_elements").and_then(|v| v.as_u64()).map(|v| v.max(1) as usize);
        let max_depth = args.get("max_depth").and_then(|v| v.as_u64()).map(|v| v.max(1) as usize);

        // "ax" = tree only; "vision" = screenshot only; "som" (default) = both.
        let do_tree = capture_mode != "vision";
        let do_shot = capture_mode != "ax";
        let state = self.state.clone();

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            let tree_result = if do_tree {
                Some(crate::atspi::walk_tree_bounded(pid, xid, query.as_deref(), max_elements, max_depth))
            } else {
                None
            };
            // Best-effort per-element screen bounds (AT-SPI Component.GetExtents).
            // Tolerant: an empty/missing map never fails the call.
            let bounds = if do_tree {
                crate::atspi::get_all_element_bounds(pid, xid).unwrap_or_default()
            } else {
                Vec::new()
            };
            let screenshot = if do_shot {
                match crate::wayland::screenshot_dispatch(xid) {
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
            Ok((tree_result, screenshot, bounds))
        }).await;

        match result {
            Ok(Ok((tree_opt, shot_opt, bounds))) => {
                let mut content = Vec::new();
                let mut structured = json!({ "window_id": xid, "pid": pid });

                if let Some(tr) = tree_opt {
                    let count = tr.nodes.iter().filter(|n| n.element_index.is_some()).count();
                    let header = format!("window_id={xid} pid={pid} elements={count}\n\n");
                    content.push(cua_driver_core::protocol::Content::text(header + &tr.tree_markdown));
                    state.element_cache.update(pid, xid, &tr.nodes);
                    structured["element_count"] = json!(count);
                    structured["tree_markdown"] = json!(tr.tree_markdown);

                    // Surface 6: register a snapshot in the global token
                    // registry so each actionable element can be addressed
                    // by an opaque per-snapshot `element_token` alongside
                    // its existing integer `element_index`. The integer
                    // surface stays unchanged — the token is additive.
                    let snapshot_id = cua_driver_core::element_token::global()
                        .register_snapshot(pid as i32, xid as u32, count);

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
                            let label = n.name.clone()
                                .or_else(|| n.value.clone())
                                .or_else(|| n.description.clone());
                            let mut entry = json!({
                                "element_index": idx,
                                // Surface 6: opaque token paired to the
                                // integer index. See cua-driver-core's
                                // `element_token` module for the format
                                // and validity contract.
                                "element_token": cua_driver_core::element_token::token_for(snapshot_id, idx),
                                "role": n.role,
                                "depth": n.depth,
                            });
                            if let Some(label) = label {
                                entry["label"] = json!(label);
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
                    structured["elements"] = json!(elements);
                    // Surface 6: snapshot id mirror for debug correlation.
                    structured["snapshot_id"] = json!(
                        cua_driver_core::element_token::token_for(snapshot_id, 0)
                            .trim_end_matches(":0")
                            .to_string()
                    );
                    structured["_note"] = json!(
                        "Prefer `elements` — `tree_markdown` will continue to work \
                         but new fields will only be added to the structured side. \
                         Issue #22865: use `max_elements` / `max_depth` to bound the \
                         AT-SPI walk on apps with very large trees."
                    );
                    // Best-effort-background ladder parity with macOS/Windows: an
                    // AT-SPI walk that ran but found zero actionable elements is
                    // NOT a clean "this window has no controls" — far more often
                    // the bridge wasn't ready (toolkit-accessibility off, or the
                    // daemon isn't on the desktop session bus so the registry is
                    // empty), or it's a non-AX surface (canvas/WebGL). Mark it
                    // degraded so callers don't read `elements: []` as authoritative.
                    if count == 0 {
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
                    }
                }

                if let Some((b64, w, h, orig_w)) = shot_opt {
                    if let Some(ow) = orig_w {
                        if w > 0 { state.resize_registry.set_ratio(pid, ow as f64 / w as f64); }
                    } else {
                        state.resize_registry.clear_ratio(pid);
                    }
                    content.push(cua_driver_core::protocol::Content::image_png(b64));
                    structured["screenshot_width"] = json!(w);
                    structured["screenshot_height"] = json!(h);
                    // Surface 7: mirror the MCP image part's `mimeType` onto
                    // the structured payload so consumers don't have to sniff
                    // magic bytes off the base64 to know the format.
                    structured["screenshot_mime_type"] = json!("image/png");
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
            description: "Launch a Linux app in the background. Provide launch_path (preferred — \
                round-trip the value from list_apps), name (app name, launched via xdg-open or \
                direct exec), bundle_id (ignored on Linux), or urls (list of URLs to open). \
                Resolution precedence: launch_path > name > bundle_id.".into(),
            input_schema: json!({"type":"object","properties":{
                "launch_path":{"type":"string","description":"Round-trip the `launch_path` returned by `list_apps` — the Exec= command from the .desktop file with XDG field codes already stripped. Highest precedence on Linux; spawned directly via the system shell."},
                "name":{"type":"string","description":"App name or command to launch."},
                "bundle_id":{"type":"string","description":"Ignored on Linux (macOS/Windows concept)."},
                "urls":{"type":"array","items":{"type":"string"},"description":"URLs to open via xdg-open."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let launch_path_opt = args.opt_str("launch_path");
        let name_opt = args.opt_str("name");
        let urls: Vec<String> = args.str_array("urls");

        if launch_path_opt.is_none() && name_opt.is_none() && urls.is_empty() {
            return ToolResult::error("Provide at least one of: launch_path, name, or urls.");
        }

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            // Open URLs via xdg-open.
            if !urls.is_empty() {
                for url in &urls {
                    std::process::Command::new("xdg-open").arg(url).spawn()?;
                }
                return Ok(format!("Opened {} URL(s) via xdg-open.", urls.len()));
            }
            // launch_path > name. Both go through the same direct-exec path
            // (so XDG `Exec=` commands round-trip), but launch_path is the
            // canonical form preferred by list_apps callers.
            let command = launch_path_opt.as_deref().or(name_opt.as_deref());
            if let Some(cmd) = command {
                let mut parts = cmd.split_whitespace();
                let prog = parts.next().unwrap_or(cmd);
                let rest: Vec<&str> = parts.collect();
                match std::process::Command::new(prog).args(&rest).spawn() {
                    Ok(child) => return Ok(format!("Launched '{}' with pid {}.", cmd, child.id())),
                    Err(_) => {
                        // Fall back to xdg-open for .desktop app names.
                        std::process::Command::new("xdg-open").arg(cmd).spawn()?;
                        return Ok(format!("Opened '{}' via xdg-open.", cmd));
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
    let reply = conn.translate_coordinates(xid as u32, root, 0, 0)?.reply()?;
    Ok((reply.dst_x as f64 + x, reply.dst_y as f64 + y))
}

fn parse_mouse_button(name: &str) -> u8 {
    match name {
        "right" => 3,
        "middle" => 2,
        _ => 1,
    }
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
    let trans = conn.translate_coordinates(xid as u32, root, 0, 0)?.reply()?;
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
                Err(e) => tracing::warn!("MPX click fell back to XSendEvent: {e}"),
            }
        }
    }
    crate::input::send_click(xid, lx, ly, count, button)
}

fn mouse_button_name(button: u8) -> &'static str {
    match button {
        3 => "right",
        2 => "middle",
        _ => "left",
    }
}

fn resolve_cursor_key(args: &Value) -> String {
    for key in ["session", "cursor_id"] {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return v.to_owned();
            }
        }
    }
    "default".to_owned()
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

fn held_target_mismatch(args: &Value, cursor_id: &str, hold: &MouseHoldState) -> Option<ToolResult> {
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

async fn overlay_glide_to(sx: f64, sy: f64) {
    overlay_glide_to_for("default", sx, sy).await;
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
    if !crate::overlay::is_enabled_for(cursor_id) {
        return;
    }
    // Wayland (Mutter/KDE, no layer-shell): glide the agent cursor via the
    // WinRects shell extension. It eases to the target itself, so send the
    // destination once here rather than the interpolated stream the X11 render
    // loop uses (which is invisible on those compositors anyway). No-op if the
    // extension isn't installed.
    if crate::wayland::is_wayland() {
        crate::wayland::shell_helper::move_cursor(sx as i32, sy as i32);
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
    status.lines()
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
    let mut parent_to_children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
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
        let parent_pid = status.lines()
            .find(|l| l.starts_with("PPid:"))
            .and_then(|l| l[5..].trim().parse::<u32>().ok());
        if let Some(parent_pid) = parent_pid {
            parent_to_children.entry(parent_pid).or_default().push(child_pid);
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
                silently degrading to left.".into(),
            input_schema: json!({
                "type":"object","required":["pid"],"properties":{
                    "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
                    "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer"},
                    "x":{"type":"number"},
                    "y":{"type":"number"},
                    "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state. REQUIRES `pid` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op. Pass `window_id` too when known to scope the cache lookup."},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Default: \"left\" (legacy back-compat). X11: routed via ButtonPress/Release with the matching evdev code. Native Wayland: only left-button is supported via the virtual-pointer protocol; right/middle return an error."},
                    "count":{"type":"integer"},
                    "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);

        // ── Window-less screen-absolute branch (capture_scope="desktop") ──────
        // x,y with NO pid and NO window_id → TRUE SCREEN pixels. Foreground,
        // vision-driven desktop-scope click (the Linux peer of the Windows
        // WindowFromPoint / macOS global-HID path). Gate on the effective scope:
        // under "window" return the same `desktop_scope_disabled` contract.
        let has_pid = args.get("pid").map(|v| !v.is_null()).unwrap_or(false);
        let has_window_id = args.get("window_id").map(|v| !v.is_null()).unwrap_or(false);
        let has_xy = args.get("x").map(|v| v.is_number()).unwrap_or(false)
            && args.get("y").map(|v| v.is_number()).unwrap_or(false);
        if has_xy && !has_pid && !has_window_id {
            // Linux config is global-only (no per-session override layer).
            let scope = self.state.config.read().unwrap().capture_scope.clone();
            if scope != "desktop" {
                return ToolResult::error(
                    "click: x,y given with no pid/window_id, but capture_scope is \
                     \"window\". Screen-absolute clicks require desktop scope. Call \
                     set_config with capture_scope=desktop (and use get_desktop_state \
                     to read true screen pixels) first."
                        .to_string(),
                )
                .with_structured(json!({
                    "code": "desktop_scope_disabled",
                    "capture_scope": scope,
                    "suggestion": "set_config capture_scope=desktop",
                }));
            }
            let button_raw = args.str_or("button", "left").to_lowercase();
            if !matches!(button_raw.as_str(), "" | "left" | "right" | "middle") {
                return ToolResult::error(format!(
                    "click: unknown button \"{button_raw}\" — expected one of left, right, middle."
                ));
            }
            let button = parse_mouse_button(if button_raw.is_empty() { "left" } else { &button_raw });
            let sx = args.f64_or("x", 0.0) as i32;
            let sy = args.f64_or("y", 0.0) as i32;
            let n = args.u64_or("count", 1) as usize;
            // Glide the agent-cursor overlay to the click point first (the macOS
            // / Windows desktop paths already do this). Without it the overlay
            // sits idle elsewhere while only the real pointer warps, so a viewer
            // sees the cursor "click somewhere else."
            overlay_glide_to_for(&cursor_id, sx as f64, sy as f64).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::input::send_click_xtest_desktop(sx, sy, button, n)
            })
            .await;
            return match r {
                Ok(Ok(())) => ToolResult::text(format!(
                    "✅ Sent screen-absolute click at ({sx},{sy}) (desktop scope)."
                )),
                Ok(Err(e)) => ToolResult::error(format!("desktop-scope click failed: {e}")),
                Err(e) => ToolResult::error(format!("task error: {e}")),
            };
        }

        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let count = args.u64_or("count", 1) as usize;
        // Surface 5: reject unknown buttons so a typo can't silently fall through
        // to a left-click. Empty string keeps back-compat with old clients.
        let button_str_raw = args.str_or("button", "left").to_lowercase();
        if !matches!(button_str_raw.as_str(), "" | "left" | "right" | "middle") {
            return ToolResult::error(format!(
                "click: unknown button \"{button_str_raw}\" — expected one of left, right, middle."
            ));
        }
        let button_str = if button_str_raw.is_empty() { "left" } else { button_str_raw.as_str() };
        let button = parse_mouse_button(button_str);

        // Surface 6: element_token / element_index precedence resolution.
        // We resolve before the legacy `opt_u64("element_index")` branch
        // so a token-only call (no integer arg) still takes the element path.
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg     = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg.map(|v| v as u32),
            "click",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let elem_idx_resolved: Option<usize> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } =>
                Some(*element_index),
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } =>
                window_id.map(|v| v as u64),
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
                    .or_else(|| crate::x11::list_windows(Some(pid)).into_iter().next().map(|w| w.xid))
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
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
            );

            // Now perform the actual click: AT-SPI doAction(0) first (background-
            // safe, no focus steal), else XSendEvent at window-local coords.
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                if crate::atspi::perform_action(pid, idx).is_ok() {
                    return Ok(());
                }
                let (xid2, lx, ly) = resolve_element_local_coords(pid, idx, xid_hint)?;
                crate::input::send_click(xid2, lx as i32, ly as i32, count, button)?;
                Ok(())
            })
            .await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Clicked element [{idx}] (pid {pid}).")),
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI element click failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
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
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(
                    format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")
                ),
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

        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        // delivery_mode: background (default) = no-focus-steal injection;
        // foreground = activate the target window (EWMH) first, then inject,
        // then restore prior active. Mirrors macOS/Windows.
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<&'static str> {
            if crate::wayland::is_wayland() {
                if crate::wayland::is_inject_mode() {
                    crate::wayland::inject_click(xid, x, y, count as u32, button)?;
                    return Ok("wayland_libei");
                }
                // Native Wayland: focus+raise the target toplevel
                // (foreign-toplevel `activate`), then drive `count` virtual-pointer
                // button events. Wayland injection routes to the compositor focus.
                crate::wayland::click(xid, xi, yi, count as u32, button)?;
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
                if !fg && button == 1 && count == 1 {
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
                        crate::input::send_click_xtest_desktop(
                            sx.round() as i32,
                            sy.round() as i32,
                            button,
                            count,
                        )?;
                        return Ok("x11_xtest_fg");
                    }
                }
                x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, xi, yi, button, count)?;
                Ok(if fg { "x11_pixel_fg" } else { "x11_pixel" })
            };
            if delivery.is_foreground() {
                crate::input::with_x11_foreground(xid, 80, || inject(true))
            } else {
                inject(false)
            }
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            // A click is never driver-verifiable (no read-back) — verified:false,
            // the caller confirms via screenshot. path reports the rung taken.
            Ok(Ok(path)) => ToolResult::text(format!(
                "✅ Clicked at ({x:.1}, {y:.1}) × {count} (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "path": path, "verified": false })),
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
                    "element_index":{"type":"integer","description":"Element index from get_window_state (accepted for cross-platform parity). REQUIRES `pid` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op."},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") { Ok(v) => v, Err(e) => return e };
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
            args.opt_u64("window_id").map(|v| v as u32),
            "type_text",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let (resolved_elem_idx, resolved_window_id) = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, window_id, .. } =>
                (Some(*element_index), window_id.map(|v| v as u64)),
            cua_driver_core::element_token::ResolvedElement::None => (None, None),
        };
        let xid_opt = resolved_window_id.or_else(|| args.opt_u64("window_id"));

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

        // EIS nested compositor: focus-FREE per-surface typing into window_id
        // (the target need not be focused). Routed over the inject control socket.
        if crate::wayland::is_inject_mode() {
            let text_len = text.chars().count();
            let text_w = text.clone();
            let result =
                tokio::task::spawn_blocking(move || crate::wayland::inject_type_text(xid, &text_w)).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (focus-free via EIS compositor)."
                ))
                .with_structured(json!({ "path": "key_events", "characters": text_len, "verified": false })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Native Wayland: keys go to the *focused* surface (no pid/window
        // targeting in the protocol). Type via the virtual-keyboard tool; pair
        // with a prior `click`/`activate` to focus the intended window.
        if crate::wayland::is_wayland() {
            let text_len = text.chars().count();
            let text_w = text.clone();
            let result =
                tokio::task::spawn_blocking(move || crate::wayland::type_text(&text_w)).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (via Wayland virtual-keyboard)."
                ))
                .with_structured(json!({ "path": "key_events", "characters": text_len, "verified": false })),
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
        let wm_class_is_terminal = tokio::task::spawn_blocking(move || {
            crate::terminal::is_terminal_window(xid)
        }).await.unwrap_or(false);
        if pid_is_terminal || wm_class_is_terminal {
            let text_len = text.chars().count();
            let text_t = text.clone();
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                // pty-master injection is preferred — it skips the X event
                // queue entirely. Falls through to XTest if the terminal
                // isn't reachable that way (descendant pty unresolvable).
                if inject_terminal_input(pid, xid, &text_t)? {
                    return Ok(());
                }
                crate::input::send_type_text_xtest(&text_t)
            }).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) (terminal emulator: pty/XTest key events)."
                ))
                .with_structured(json!({ "path": "key_events", "characters": text_len, "verified": false })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        // Pulse the agent cursor onto the field being typed into (when an
        // element_index OR element_token is supplied — token resolution
        // already ran above so `resolved_elem_idx` covers both).
        if let Some(idx) = resolved_elem_idx {
            crate::overlay::send_command(cursor_overlay::OverlayCommand::PinAbove(xid));
            if let Ok(Ok((sx, sy))) =
                tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
            {
                overlay_glide_to(sx, sy).await;
                crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse {
                    x: sx,
                    y: sy,
                });
            }
        }
        let text_len = text.chars().count();

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
        }).await.ok().flatten();
        if focus_kind == Some(false) {
            let text_f = text.clone();
            let result = tokio::task::spawn_blocking(move || {
                if inject_terminal_input(pid, xid, &text_f)? {
                    return Ok(());
                }
                // XTest (real input to the focused window), NOT XSendEvent: GTK/Qt
                // drop synthetic key events, so a spreadsheet cell / canvas would
                // stay empty. The click that gave this widget focus already put it
                // under the X input focus, so XTest-to-focus lands correctly.
                crate::input::send_type_text_xtest(&text_f)
            }).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Typed {text_len} character(s) into the focused widget."
                ))
                .with_structured(json!({ "path": "key_events", "characters": text_len, "verified": false })),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Try AT-SPI EditableText first (focus-free, works for Qt6/GTK4).
        let text_clone = text.clone();
        let atspi_result = tokio::task::spawn_blocking(move || {
            crate::atspi::type_into_editable(pid, &text_clone)
        }).await;

        match atspi_result {
            Ok(Ok(())) => {
                // AT-SPI succeeded — focus-free typing worked (Qt6, GTK4, etc.)!
                return ToolResult::text(format!("Typed {text_len} character(s) (via AT-SPI)."))
                    .with_structured(json!({ "path": "ax", "characters": text_len, "verified": true }));
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
        }).await;

        match qt5_result {
            Ok(Ok(())) => {
                return ToolResult::text(format!("Typed {text_len} character(s) (via AT-SPI with focus workaround)."))
                    .with_structured(json!({ "path": "ax", "characters": text_len, "verified": true }));
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
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<&'static str> {
            // Terminals: write to the pty master (focus-free, below the toolkit).
            if inject_terminal_input(pid, xid, &text)? {
                return Ok("key_events");
            }
            if delivery.is_foreground() {
                // Activate target → XTest real keystrokes → restore prior active.
                return crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_type_text_xtest(&text)?;
                    Ok("key_events_fg")
                });
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
            crate::input::send_type_text(xid, &text)?;
            Ok("key_events")
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            // Read-back verdict: the AT-SPI EditableText.insertText path ("ax") is
            // the driver-verifiable rung on Linux — the a11y layer confirms the
            // insert into the widget model (truthful on GTK/Qt). The keystroke /
            // XSendEvent / XTest rungs aren't read-back-confirmed (verified:false;
            // caller confirms via screenshot).
            Ok(Ok(path)) => ToolResult::text(format!(
                "Typed {text_len} character(s) (via X11, delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "path": path, "characters": text_len, "verified": path == "ax" })),
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
                    "modifiers":{"type":"array","items":{"type":"string"}},
                    "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state. Resolves window_id when window_id is omitted."},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let key = match args.require_str("key") { Ok(v) => v, Err(e) => return e };
        let mods: Vec<String> = args.str_array("modifiers");

        // Surface 6: resolve element_token / element_index for the window-id
        // hint. press_key targets a window via XSendEvent, so we only need
        // the resolved window_id — element_index itself is not used to
        // address an AX node here (no focus-grab path).
        let element_token_arg = args.opt_str("element_token");
        let window_id_arg     = args.opt_u64("window_id");
        let element_index_arg = args.opt_u64("element_index").map(|v| v as usize);
        let resolved = match cua_driver_core::element_token::resolve_element_args(
            pid as i32,
            element_index_arg,
            element_token_arg.as_deref(),
            window_id_arg.map(|v| v as u32),
            "press_key",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let xid_opt = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } =>
                window_id.map(|v| v as u64).or(window_id_arg),
            cua_driver_core::element_token::ResolvedElement::None => window_id_arg,
        };
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
        // EIS nested compositor: focus-free named-key into window_id.
        if crate::wayland::is_inject_mode() {
            let key_w = key.clone();
            let result =
                tokio::task::spawn_blocking(move || crate::wayland::inject_press_key(xid, &key_w)).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!("Pressed key '{key}' (focus-free via EIS compositor).")),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        // Native Wayland: send the key to the focused surface via virtual-keyboard.
        if crate::wayland::is_wayland() {
            let key_w = key.clone();
            let result = tokio::task::spawn_blocking(move || crate::wayland::press_key(&key_w)).await;
            return match result {
                Ok(Ok(())) => ToolResult::text(format!(
                    "Pressed key '{key}' (via Wayland virtual-keyboard)."
                )),
                Ok(Err(e)) => ToolResult::error(e.to_string()),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }

        let key_for_task = key.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if mods.is_empty() && key_for_task.eq_ignore_ascii_case("enter") {
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
            if delivery.is_foreground() {
                return crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_key_xtest(&key_for_task, &m)
                });
            }
            crate::input::send_key(xid, &key_for_task, &m)
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Pressed key '{key}' (delivery_mode={mode_label})."))
                .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
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
                        "description":"Modifier(s) + one non-modifier key, e.g. [\"ctrl\",\"c\"]."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let xid_opt = args.opt_u64("window_id");

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
        } else if let Some(k) = args.opt_str("key") {
            let mods: Vec<String> = args.str_array("modifiers");
            (k, mods)
        } else {
            return ToolResult::error("Provide 'keys' array (e.g. [\"ctrl\",\"c\"]) or 'key'+'modifiers' parameters.");
        };

        let key_display = format!("{}+{}", mods.join("+"), key);
        let key_for_wayland = key.clone();
        let mods_for_wayland = mods.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_wayland() {
                // Native Wayland: route the modifier combo through wtype's
                // -M/-k/-m sequence — the closest equivalent to the X11
                // state-mask path. window_id is irrelevant once focused.
                let mut combo: Vec<String> = mods_for_wayland.clone();
                combo.push(key_for_wayland.clone());
                return crate::wayland::hotkey(&combo);
            }
            let m: Vec<&str> = mods.iter().map(String::as_str).collect();
            // foreground: activate the target first, then inject the accelerator
            // as REAL key events via XTest. Synthetic XSendEvent keys
            // (`send_key`) are dropped by GTK/Qt/Chromium/Firefox, so the
            // foreground rung must use XTest, which reaches the focused window.
            if delivery.is_foreground() {
                return crate::input::with_x11_foreground(xid, 80, || {
                    crate::input::send_key_xtest(&key, &m)
                });
            }
            crate::input::send_key(xid, &key, &m)
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Pressed {key_display} on pid {pid} (delivery_mode={mode_label})."))
                .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
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
                "type":"object","required":["pid","value"],"properties":{
                    "pid":{"type":"integer"},
                    "window_id":{"type":"integer","description":"Required when element_index is used; optional when element_token is supplied (the token carries it)."},
                    "element_index":{"type":"integer","description":"Element index from get_window_state. Must be supplied unless element_token is provided. REQUIRES `pid` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op. Pass `window_id` too when known to scope the cache lookup."},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "value":{"type":"string"}
                },"additionalProperties":false
            }),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let value = match args.require_str("value") { Ok(v) => v, Err(e) => return e };
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
        let idx = match resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } => element_index,
            cua_driver_core::element_token::ResolvedElement::None =>
                return ToolResult::error(
                    "set_value requires element_index or element_token to address the target element."
                ),
        };
        let value_for_task = value.clone();
        // Pulse the agent cursor onto the target element before writing, so a
        // value write gets the same visual feedback as a click — the viewer can
        // see *where* the agent is acting. No-op when the element bounds can't
        // be resolved or the overlay is disabled.
        if let Ok(Ok((sx, sy))) =
            tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
        {
            let window_id = args.u64_or("window_id", 0);
            if window_id != 0 {
                crate::overlay::send_command(cursor_overlay::OverlayCommand::PinAbove(window_id));
            }
            overlay_glide_to(sx, sy).await;
            crate::overlay::send_command(cursor_overlay::OverlayCommand::ClickPulse {
                x: sx,
                y: sy,
            });
        }
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
                    "element_index":{"type":"integer"},
                    "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                    "delivery_mode": crate::input::delivery::delivery_mode_schema()
                },"additionalProperties":false
            }),
            read_only: false, destructive: false, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let direction = match args.require_str("direction") { Ok(v) => v, Err(e) => return e };
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
            args.opt_u64("window_id").map(|v| v as u32),
            "scroll",
        ) {
            Ok(r) => r,
            Err(e) => return e,
        };
        let xid_opt: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } =>
                window_id.map(|v| v as u64).or_else(|| args.opt_u64("window_id")),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };

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
        let direction_for_wayland = direction.clone();
        let amount_u32 = amount as u32;
        let cursor_id_for_task = cursor_id.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_wayland() {
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
                if let Ok((cx, cy)) = window_screen_center(xid) {
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
                        Err(e) => tracing::warn!("MPX scroll fell back to XSendEvent: {e}"),
                    }
                }
            }
            crate::input::send_click(xid, 0, 0, amount, button)
            };
            if delivery.is_foreground() {
                crate::input::with_x11_foreground(xid, 80, x11_scroll)
            } else {
                x11_scroll()
            }
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "Scrolled {direction} {amount} ticks (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
            Ok(Err(e)) => ToolResult::error(e.to_string()),
            Err(e) => ToolResult::error(format!("Task error: {e}")),
        }
    }
}

// `ScreenshotTool` and `ScreenshotCompatTool` removed in PR #1692 — see the
// matching note in platform-windows/src/tools/impl_.rs. `get_window_state`
// with `capture_mode:"vision"` is the canonical screenshot path; the
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state. REQUIRES `pid` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op. Pass `window_id` too when known to scope the cache lookup."},
                "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        // Surface 6: element_token / element_index precedence.
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
        let elem_idx_resolved = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } =>
                Some(*element_index),
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } =>
                window_id.map(|v| v as u64),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if let Some(idx) = elem_idx_resolved {
            let xid_hint = window_id_resolved;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            }).await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
                    {
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(xid),
                        );
                        overlay_glide_to_for(&cursor_id, sx, sy).await;
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
                        );
                    }
                    let lxi = lx as i32;
                    let lyi = ly as i32;
                    let cursor_id_for_task = cursor_id.clone();
                    let click_result = tokio::task::spawn_blocking(move || {
                        if crate::wayland::is_wayland() {
                            return crate::wayland::click(xid, lxi, lyi, 2, 1);
                        }
                        x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, lxi, lyi, 1, 2)
                    }).await;
                    match click_result {
                        Ok(Ok(())) => ToolResult::text(format!("✅ Double-clicked element [{idx}].")),
                        Ok(Err(e)) => ToolResult::error(e.to_string()),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match window_id_resolved {
            Some(v) => v, None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
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
        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_wayland() {
                return crate::wayland::click(xid, xi, yi, 2, 1);
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
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Double-clicked at ({x:.1}, {y:.1}) (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "element_index":{"type":"integer","description":"AT-SPI element index from get_window_state. REQUIRES `pid` to be passed alongside it — element_index alone (no pid) fails fast with \"Missing required integer field: pid\"; it is not a silent no-op. Pass `window_id` too when known to scope the cache lookup."},
                "element_token":{"type":"string","description":"Opaque per-snapshot element handle from `structuredContent.elements[].element_token`. Takes precedence over element_index when both supplied. Returns an explicit \"stale\" error if the snapshot has been superseded."},
                "modifier":{"type":"array","items":{"type":"string"},"description":"Modifier keys to hold."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."},
                "delivery_mode": crate::input::delivery::delivery_mode_schema()
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        // Surface 6: element_token / element_index precedence.
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
        let elem_idx_resolved = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { element_index, .. } =>
                Some(*element_index),
            cua_driver_core::element_token::ResolvedElement::None => None,
        };
        let window_id_resolved: Option<u64> = match &resolved {
            cua_driver_core::element_token::ResolvedElement::Element { window_id, .. } =>
                window_id.map(|v| v as u64),
            cua_driver_core::element_token::ResolvedElement::None => args.opt_u64("window_id"),
        };
        if let Some(idx) = elem_idx_resolved {
            let xid_hint = window_id_resolved;
            let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, f64, f64)> {
                resolve_element_local_coords(pid, idx, xid_hint)
            }).await;
            return match result {
                Ok(Ok((xid, lx, ly))) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || element_screen_center(pid, idx)).await
                    {
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::PinAbove(xid),
                        );
                        overlay_glide_to_for(&cursor_id, sx, sy).await;
                        crate::overlay::send_command_for(
                            cursor_id.clone(),
                            cursor_overlay::OverlayCommand::ClickPulse { x: sx, y: sy },
                        );
                    }
                    let lxi = lx as i32;
                    let lyi = ly as i32;
                    let cursor_id_for_task = cursor_id.clone();
                    let click_result = tokio::task::spawn_blocking(move || {
                        if crate::wayland::is_wayland() {
                            return crate::wayland::click(xid, lxi, lyi, 1, 3);
                        }
                        x11_pixel_click_no_focus_steal(&cursor_id_for_task, xid, lxi, lyi, 3, 1)
                    }).await;
                    match click_result {
                        Ok(Ok(())) => ToolResult::text(format!("✅ Right-clicked element [{idx}].")),
                        Ok(Err(e)) => ToolResult::error(e.to_string()),
                        Err(e) => ToolResult::error(format!("Task error: {e}")),
                    }
                }
                Ok(Err(e)) => ToolResult::error(format!("AT-SPI bounds failed: {e}")),
                Err(e) => ToolResult::error(format!("Task error: {e}")),
            };
        }
        let xid = match window_id_resolved {
            Some(v) => v, None => return ToolResult::error("Provide either element_index or window_id + x/y."),
        };
        let from_zoom = args.bool_or("from_zoom", false);
        let mut x = args.f64_or("x", 0.0);
        let mut y = args.f64_or("y", 0.0);
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
        let (xi, yi) = (x as i32, y as i32);
        let cursor_id_for_task = cursor_id.clone();
        let delivery = crate::input::delivery::DeliveryMode::from_args(&args);
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            if crate::wayland::is_wayland() {
                return crate::wayland::click(xid, xi, yi, 1, 3);
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
        }).await;
        let mode_label = if delivery.is_foreground() { "foreground" } else { "background" };
        match result {
            Ok(Ok(())) => ToolResult::text(format!(
                "✅ Right-clicked at ({x:.1}, {y:.1}) (delivery_mode={mode_label})."
            ))
            .with_structured(json!({ "verified": false, "delivery_mode": mode_label })),
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
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
        let cursor_id = resolve_cursor_key(&args);
        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
        let xid = match args.opt_u64("window_id") {
            Some(v) => v, None => return ToolResult::error("window_id is required on Linux."),
        };

        let coerce = |key: &str| -> Option<f64> {
            args.opt_f64(key).or_else(|| args.opt_i64(key).map(|i| i as f64))
        };
        let mut from_x = match coerce("from_x") { Some(v) => v, None => return ToolResult::error("Missing: from_x") };
        let mut from_y = match coerce("from_y") { Some(v) => v, None => return ToolResult::error("Missing: from_y") };
        let mut to_x   = match coerce("to_x")   { Some(v) => v, None => return ToolResult::error("Missing: to_x") };
        let mut to_y   = match coerce("to_y")   { Some(v) => v, None => return ToolResult::error("Missing: to_y") };

        let duration_ms = args.u64_or("duration_ms", 500);
        let steps       = args.u64_or("steps", 20) as usize;
        let button_str  = args.str_or("button", "left");
        let button = parse_mouse_button(button_str.as_str());
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

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx_from, sy_from))) =
            tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y)).await
        {
            overlay_glide_to_for(&cursor_id, sx_from, sy_from).await;
            self.state.cursor_registry.update_position(&cursor_id, sx_from, sy_from);
            overlay_snap_to_for(&cursor_id, sx_from, sy_from, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::ClickPulse { x: sx_from, y: sy_from },
            );
        }
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetPressed(true),
        );

        // Native Wayland: emit press + interpolated motion + release as one
        // virtual-pointer sequence (output-relative coords). Returns early so
        // we don't fall into the X11 XSendEvent loop below.
        if crate::wayland::is_wayland() {
            let (fxi, fyi) = (from_x.round() as i32, from_y.round() as i32);
            let (txi, tyi) = (to_x.round() as i32, to_y.round() as i32);
            let steps_u32 = steps as u32;
            let drag_result = tokio::task::spawn_blocking(move || {
                crate::wayland::drag(xid, fxi, fyi, txi, tyi, steps_u32, button)
            }).await;
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(false),
            );
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

        let press_result = tokio::task::spawn_blocking(move || {
            crate::input::send_button_down(xid, from_x.round() as i32, from_y.round() as i32, button)
        }).await;
        let mut result: anyhow::Result<()> = match press_result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(anyhow::anyhow!("Task error: {e}")),
        };

        if result.is_ok() {
            let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };
            let mut prev_x = from_x;
            let mut prev_y = from_y;
            for i in 1..=steps {
                let t = i as f64 / steps.max(1) as f64;
                let ix = from_x + (to_x - from_x) * t;
                let iy = from_y + (to_y - from_y) * t;
                let motion_result = tokio::task::spawn_blocking(move || {
                    crate::input::send_motion(xid, ix.round() as i32, iy.round() as i32, Some(button))
                }).await;
                match motion_result {
                    Ok(Ok(())) => {
                        if let Ok(Ok((sx, sy))) =
                            tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy)).await
                        {
                        let heading = if (ix - prev_x).abs() > f64::EPSILON
                            || (iy - prev_y).abs() > f64::EPSILON
                        {
                            Some((iy - prev_y).atan2(ix - prev_x))
                        } else {
                            None
                        };
                        self.state.cursor_registry.update_position(&cursor_id, sx, sy);
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
        }

        let release_result = tokio::task::spawn_blocking(move || {
            crate::input::send_button_up(xid, to_x.round() as i32, to_y.round() as i32, button)
        }).await;
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
                self.state.cursor_registry.update_position(&cursor_id, sx_to, sy_to);
                overlay_snap_to_for(
                    &cursor_id,
                    sx_to,
                    sy_to,
                    Some((to_y - from_y).atan2(to_x - from_x)),
                );
                crate::overlay::send_command_for(
                    cursor_id.clone(),
                    cursor_overlay::OverlayCommand::ClickPulse { x: sx_to, y: sy_to },
                );
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
                "cursor_id":{"type":"string","description":"Optional multi-cursor instance id. Default: 'default'."},
                "pid":{"type":"integer"},
                "window_id":{"type":"integer"},
                "x":{"type":"number"},
                "y":{"type":"number"},
                "button":{"type":"string","enum":["left","right","middle"],"description":"Mouse button. Default: left."},
                "from_zoom":{"type":"boolean","description":"Set true after a zoom call to auto-translate zoom-image pixel coordinates back to full-window space."}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        if let Some(held) = self.state.mouse_hold.lock().unwrap().get(&cursor_id).cloned() {
            return ToolResult::error(format!(
                "Cursor '{cursor_id}' already has a held mouse button. Call mouse_button_up first."
            ))
            .with_structured(mouse_hold_json(&cursor_id, Some(&held)));
        }

        let pid = match args.require_u32("pid") { Ok(v) => v, Err(e) => return e };
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
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(format!("from_zoom=true but no zoom context for pid {pid}. Call zoom first.")),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) = tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await {
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
            tokio::task::spawn_blocking(move || crate::wayland::persistent_vptr::press(&cid, xid, xi, yi, button)).await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_down(xid, xi, yi, button)).await
        };
        match result {
            Ok(Ok(())) => {
                let hold = MouseHoldState { cursor_id: cursor_id.clone(), pid, xid, button, x, y };
                self.state.mouse_hold.lock().unwrap().insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state.cursor_registry.update_position(&cursor_id, sx, sy);
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
            Ok(Err(e)) => ToolResult::error(e.to_string())
                .with_structured(mouse_hold_json(&cursor_id, self.state.mouse_hold.lock().unwrap().get(&cursor_id))),
            Err(e) => ToolResult::error(format!("Task error: {e}"))
                .with_structured(mouse_hold_json(&cursor_id, self.state.mouse_hold.lock().unwrap().get(&cursor_id))),
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
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
        let Some(mut hold) = self.state.mouse_hold.lock().unwrap().get(&cursor_id).cloned() else {
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
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(to_x, to_y); to_x = wx; to_y = wy; }
                None => return ToolResult::error(format!("from_zoom=true but no zoom context for pid {}. Call zoom first.", hold.pid))
                    .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
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
        if let Ok(Ok((sx, sy))) = tokio::task::spawn_blocking(move || window_local_to_screen(xid, from_x, from_y)).await {
            overlay_glide_to_for(&cursor_id, sx, sy).await;
            self.state.cursor_registry.update_position(&cursor_id, sx, sy);
            overlay_snap_to_for(&cursor_id, sx, sy, None);
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetPressed(true),
            );
        }

        let button = hold.button;
        let step_delay_ms = if steps > 1 { duration_ms / steps as u64 } else { duration_ms };
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
                    crate::wayland::persistent_vptr::move_to(&cid_inner, ix.round() as i32, iy.round() as i32)
                }).await
            } else {
                tokio::task::spawn_blocking(move || {
                    crate::input::send_motion(xid, ix.round() as i32, iy.round() as i32, Some(button))
                }).await
            };
            match move_result {
                Ok(Ok(())) => {
                    if let Ok(Ok((sx, sy))) =
                        tokio::task::spawn_blocking(move || window_local_to_screen(xid, ix, iy)).await
                    {
                        let heading = if (ix - prev_x).abs() > f64::EPSILON || (iy - prev_y).abs() > f64::EPSILON {
                            Some((iy - prev_y).atan2(ix - prev_x))
                        } else {
                            None
                        };
                        self.state.cursor_registry.update_position(&cursor_id, sx, sy);
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
                self.state.mouse_hold.lock().unwrap().insert(cursor_id.clone(), hold.clone());
                if let Ok(Ok((sx, sy))) = tokio::task::spawn_blocking(move || window_local_to_screen(xid, to_x, to_y)).await {
                    self.state.cursor_registry.update_position(&cursor_id, sx, sy);
                    overlay_snap_to_for(&cursor_id, sx, sy, Some((to_y - from_y).atan2(to_x - from_x)));
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
                "session":{"type":"string","description":"Optional multi-cursor session id; takes precedence over cursor_id."},
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
        let Some(mut hold) = self.state.mouse_hold.lock().unwrap().get(&cursor_id).cloned() else {
            return ToolResult::error(format!("No mouse button is currently held for cursor '{cursor_id}'."))
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
                Some(ctx) => { let (wx, wy) = ctx.zoom_to_window(x, y); x = wx; y = wy; }
                None => return ToolResult::error(format!("from_zoom=true but no zoom context for pid {}. Call zoom first.", hold.pid))
                    .with_structured(mouse_hold_json(&cursor_id, Some(&hold))),
            }
        } else if let Some(ratio) = self.state.resize_registry.ratio(hold.pid) {
            x *= ratio;
            y *= ratio;
        }

        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::PinAbove(xid),
        );
        if let Ok(Ok((sx, sy))) = tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await {
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
            tokio::task::spawn_blocking(move || crate::wayland::persistent_vptr::release(&cid, button)).await
        } else {
            tokio::task::spawn_blocking(move || crate::input::send_button_up(xid, xi, yi, button)).await
        };
        match result {
            Ok(Ok(())) => {
                hold.x = x;
                hold.y = y;
                if let Ok(Ok((sx, sy))) =
                    tokio::task::spawn_blocking(move || window_local_to_screen(xid, x, y)).await
                {
                    self.state.cursor_registry.update_position(&cursor_id, sx, sy);
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

/// EIS-compositor path for parallel_mouse_drag: build window-local drag paths
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
        let local: Vec<(f64, f64)> = if let Some(pts) = item.get("path").and_then(|v| v.as_array()) {
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
                _ => return ToolResult::error("each drag item requires path[] or from_x/from_y/to_x/to_y."),
            }
        };
        if local.len() < 2 {
            return ToolResult::error("drag path needs at least 2 points.");
        }
        let steps = item.get("steps").and_then(|v| v.as_u64()).unwrap_or(60).clamp(1, 300) as usize;
        let x_button = parse_mouse_button(item.get("button").and_then(|v| v.as_str()).unwrap_or("left")) as u32;
        let app = match tokio::task::spawn_blocking(move || crate::wayland::app_id_for_window(xid)).await {
            Ok(Some(a)) => a,
            _ => return ToolResult::error(format!("no Wayland app_id for window {xid}")),
        };
        drags.push(crate::wayland::InjectDrag { app_id: app, idx: i, x_button, path: local, steps });
    }
    let n = drags.len();
    match tokio::task::spawn_blocking(move || crate::wayland::inject_parallel_drags(&drags)).await {
        Ok(Ok(())) => ToolResult::text(format!("Ran {n} concurrent drags (multi-cursor via EIS compositor).")),
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
                    "button":{"type":"string","enum":["left","right","middle"],"description":"Default: left."},
                    "duration_ms":{"type":"integer","minimum":0,"maximum":10000,"description":"Default: 1500 for fn paths, 500 for straight."},
                    "steps":{"type":"integer","minimum":1,"maximum":300,"description":"Motion sub-steps along the whole path. Default: scaled to path length."}
                },"additionalProperties":false}}
            },"additionalProperties":false}),
            read_only: false, destructive: true, idempotent: false, open_world: true,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // EIS nested compositor: run the drags as concurrent multi-cursor
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
                 or run the target under X11."
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
            let local: Vec<(f64, f64)> = if let Some(pts) = item.get("path").and_then(|v| v.as_array()) {
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
                let samples = item.get("samples").and_then(|v| v.as_u64()).unwrap_or(80).clamp(2, 400);
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

            let button = parse_mouse_button(item.get("button").and_then(|v| v.as_str()).unwrap_or("left"));
            let duration_ms = item.get("duration_ms").and_then(|v| v.as_u64())
                .unwrap_or(if is_fn { 1500 } else { 500 });

            // One translate gives the window origin; the path is a pure offset.
            let origin = match tokio::task::spawn_blocking(move || window_local_to_screen(xid, 0.0, 0.0)).await {
                Ok(Ok(o)) => o,
                Ok(Err(e)) => return ToolResult::error(e.to_string()),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            };
            let path: Vec<(i32, i32)> = local.iter()
                .map(|(lx, ly)| ((origin.0 + lx).round() as i32, (origin.1 + ly).round() as i32))
                .collect();

            // Default sub-step count scaled to path length (smooth glide),
            // overridable via `steps`.
            let total_len: f64 = path.windows(2)
                .map(|w| (((w[1].0 - w[0].0) as f64).powi(2) + ((w[1].1 - w[0].1) as f64).powi(2)).sqrt())
                .sum();
            let steps = item.get("steps").and_then(|v| v.as_u64())
                .map(|s| (s as usize).clamp(1, 300))
                .unwrap_or_else(|| ((total_len / 3.0).round() as usize).clamp(24, 300));

            let start = path[0];
            self.state.cursor_registry.update_position(session, start.0 as f64, start.1 as f64);
            crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::PinAbove(xid));
            crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SnapTo {
                x: start.0 as f64,
                y: start.1 as f64,
                heading_radians: None,
            });
            crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SetPressed(true));

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
        let result = tokio::task::spawn_blocking(move || crate::input::send_parallel_virtual_pointer_drags(&drags_for_task)).await;
        match result {
            Ok(Ok(())) => {
                for (session, drag) in &drags {
                    let n = drag.path.len();
                    let end = drag.path[n - 1];
                    let prev = drag.path[n.saturating_sub(2)];
                    self.state.cursor_registry.update_position(session, end.0 as f64, end.1 as f64);
                    crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SnapTo {
                        x: end.0 as f64,
                        y: end.1 as f64,
                        heading_radians: Some(((end.1 - prev.1) as f64).atan2((end.0 - prev.0) as f64)),
                    });
                    crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SetPressed(false));
                    crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::ClickPulse {
                        x: end.0 as f64,
                        y: end.1 as f64,
                    });
                }
                ToolResult::text(format!("✅ Ran {} MPX drag gesture(s) concurrently.", drags.len()))
                    .with_structured(json!({"count": drags.len()}))
            }
            Ok(Err(e)) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SetPressed(false));
                }
                ToolResult::error(e.to_string())
            }
            Err(e) => {
                for (session, _) in &drags {
                    crate::overlay::send_command_for(session.to_owned(), cursor_overlay::OverlayCommand::SetPressed(false));
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
                Requires no TCC permissions.".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let result = tokio::task::spawn_blocking(|| {
            // X11 reports pixel dimensions; scale factor on X11 is not
            // well-defined per-monitor, so report 1.0 (matches DPI-unaware
            // assumption).  Wayland/HiDPI X11 callers should query
            // `xrandr --query` for true scale.
            let (w, h) = x11_screen_size()?;
            Ok::<(u32, u32, f64), anyhow::Error>((w, h, 1.0))
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

// ── get_desktop_state ─────────────────────────────────────────────────────────

pub struct GetDesktopStateTool;
static GDS_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for GetDesktopStateTool {
    fn def(&self) -> &ToolDef {
        GDS_DEF.get_or_init(|| ToolDef {
            name: "get_desktop_state".into(),
            description: "Full-display vision screenshot in true screen pixels (no downscale), \
                for capture_scope=\"desktop\" GUI loops. Captures the entire display (root \
                window) as native-size PNG so screen-absolute pixel coordinates land exactly. \
                No AT-SPI walk.".into(),
            input_schema: json!({"type":"object","properties":{
                "session":{"type":"string","description":"Optional session id."},
                "screenshot_out_file":{"type":"string","description":"Write PNG here instead of base64."}
            },"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: false, open_world: false,
        })
    }

    async fn invoke(&self, args: Value) -> ToolResult {
        // Gate on the global capture_scope: a full-display capture is a
        // desktop-scope operation, available only when capture_scope="desktop"
        // (same gate as window-less screen-absolute click/scroll). Read the
        // persisted value the same way load_config does.
        let scope = pip_preview::read_config_value("capture_scope")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "window".to_owned());
        if scope != "desktop" {
            return ToolResult::error(format!(
                "get_desktop_state requires capture_scope=\"desktop\" (current scope is \
                 \"{scope}\"). Full-display capture is a desktop-scope operation; call \
                 set_config with capture_scope=desktop first (it also enables window-less \
                 screen-absolute click/scroll). For a single window, use \
                 get_window_state(pid, window_id) instead."
            ))
            .with_structured(serde_json::json!({
                "code": "desktop_scope_disabled",
                "capture_scope": scope,
                "suggestion": "set_config capture_scope=desktop",
            }));
        }

        let out_file = args.opt_str("screenshot_out_file");

        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            // Vision-only: capture the FULL DISPLAY at native size. No downscale
            // so screen-absolute pixels land exactly.
            let png = crate::capture::screenshot_display_bytes()?;
            let (shot_w, shot_h) = crate::capture::png_dimensions_pub(&png)?;
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
                (shot_w, shot_h)
            } else {
                x11_screen_size()?
            };
            // Optional: write PNG to disk instead of returning base64.
            let written = if let Some(path) = out_file.as_deref() {
                std::fs::write(path, &png)?;
                Some(path.to_string())
            } else {
                None
            };
            use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
            let b64 = if written.is_some() { None } else { Some(B64.encode(&png)) };
            Ok((b64, shot_w, shot_h, screen_w, screen_h, written))
        }).await;

        match result {
            Ok(Ok((b64_opt, shot_w, shot_h, screen_w, screen_h, written))) => {
                let mut content = Vec::new();
                let mut structured = json!({
                    "platform": "linux",
                    "screenshot_width": shot_w,
                    "screenshot_height": shot_h,
                    "screen_width": screen_w,
                    "screen_height": screen_h,
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
                ToolResult { content, is_error: None, structured_content: Some(structured) }
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
            description: "Return the current mouse cursor position in screen points (origin top-left).".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
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
        }).await;
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

#[async_trait]
impl Tool for MoveCursorTool {
    fn def(&self) -> &ToolDef {
        MCURSOR_DEF.get_or_init(|| ToolDef {
            name: "move_cursor".into(),
            description: "Move the agent cursor overlay to (x, y). Does NOT move the real mouse cursor.".into(),
            input_schema: json!({"type":"object","required":["x","y"],"properties":{
                "x":{"type":"number"},"y":{"type":"number"},"session":{"type":"string"},"cursor_id":{"type":"string"}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let x = args.f64_or("x", 0.0);
        let y = args.f64_or("y", 0.0);
        let window_id = args.get("window_id").and_then(|v| v.as_u64());
        let cursor_id = resolve_cursor_key(&args);
        self.state.cursor_registry.update_position(&cursor_id, x, y);
        // End pointing upper-left (45°) — matches Swift's
        // `AgentCursor.animateAndWait(endAngleDegrees: 45)` convention so the
        // overlay arrow settles to the natural macOS-style pose.
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::MoveTo {
                x,
                y,
                end_heading_radians: std::f64::consts::FRAC_PI_4,
            },
        );
        // Native Wayland: also warp the real cursor via zwlr_virtual_pointer.
        // Off-thread because the wayland-client roundtrip is blocking. Best-effort
        // — overlay update + registry write already succeeded; surface a warning
        // only if the warp itself failed.
        let real_warp_note = if crate::wayland::is_wayland() {
            let xi = x.round() as i32;
            let yi = y.round() as i32;
            match tokio::task::spawn_blocking(move || crate::wayland::move_cursor_absolute(window_id, xi, yi)).await {
                Ok(Ok(())) => " (real cursor warped via virtual-pointer)",
                Ok(Err(_)) | Err(_) => " (overlay updated; real-cursor warp failed)",
            }
        } else {
            ""
        };
        ToolResult::text(format!("Agent cursor '{cursor_id}' moved to ({x:.1}, {y:.1}).{real_warp_note}"))
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
                "enabled":{"type":"boolean"},"session":{"type":"string"},"cursor_id":{"type":"string"}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let enabled = match args.require_bool("enabled") { Ok(v) => v, Err(e) => return e };
        let cursor_id = resolve_cursor_key(&args);
        self.state.cursor_registry.set_enabled(&cursor_id, enabled);
        crate::overlay::send_command_for(
            cursor_id.clone(),
            cursor_overlay::OverlayCommand::SetEnabled(enabled),
        );
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
            description: format!("Configure the visual appearance of an agent cursor instance.\n\n\
                - cursor_id: instance name (default='default')\n\
                - cursor_icon: built-in ({}) or a path to a PNG/JPEG/SVG/ICO file; '' reverts to the default cursor\n\
                - cursor_color: hex color e.g. '#00FFFF' or CSS name\n\
                - cursor_label: short text shown near the cursor\n\
                - cursor_size: dot radius in points (default=16)\n\
                - cursor_opacity: 0.0–1.0 (default=0.85)",
                cursor_overlay::BuiltinShape::names_help()),
            input_schema: json!({
                "type":"object","properties":{
                    "session":{"type":"string"},
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
        let cursor_id = resolve_cursor_key(&args);
        // Resolve `cursor_icon` (built-in name or image path — same vocabulary as
        // the CLI flags) to a shape override and dispatch it, so the overlay
        // actually changes instead of just recording the string.
        let mut shape_cmd: Option<cursor_overlay::OverlayCommand> = None;
        if let Some(icon) = args.opt_str("cursor_icon") {
            let icon_owned = icon.clone();
            match tokio::task::spawn_blocking(move || {
                cursor_overlay::resolve_cursor_icon(&icon_owned)
            }).await {
                Ok(Ok(resolution)) => shape_cmd = Some(cursor_overlay::OverlayCommand::from_cursor_icon(resolution)),
                Ok(Err(e)) => return ToolResult::error(format!("Invalid cursor_icon: {e}")),
                Err(e) => return ToolResult::error(format!("Task error: {e}")),
            }
        }
        self.state.cursor_registry.update_config(&cursor_id, |cfg| {
            if let Some(v) = args.opt_str("cursor_icon") { cfg.cursor_icon = Some(v); }
            if let Some(v) = args.opt_str("cursor_color") { cfg.cursor_color = Some(v); }
            if let Some(v) = args.opt_str("cursor_label") { cfg.cursor_label = Some(v); }
            if let Some(v) = args.opt_f64("cursor_size") { cfg.cursor_size = Some(v); }
            if let Some(v) = args.opt_f64("cursor_opacity") { cfg.cursor_opacity = Some(v.clamp(0.0, 1.0)); }
        });
        if let Some(cmd) = shape_cmd {
            crate::overlay::send_command_for(cursor_id.clone(), cmd);
        }
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
            input_schema: json!({"type":"object","properties":{"session":{"type":"string"},"cursor_id":{"type":"string"}},"additionalProperties":false}),
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        let cursor_id = resolve_cursor_key(&args);
        let states = if args.get("session").is_some() || args.get("cursor_id").is_some() {
            vec![self.state.cursor_registry.get_or_create(&cursor_id)]
        } else {
            self.state.cursor_registry.all_states()
        };
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
                   icon instead of the default silhouette. Empty string reverts to the \
                   default cursor.\n\
                 All parameters are optional; omit any you do not want to change."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session": {
                        "type": "string",
                        "description": "Optional multi-cursor session id; takes precedence over cursor_id."
                    },
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
        use cua_driver_core::tool_args::ArgsExt;
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
            crate::overlay::send_command_for(cursor_id.clone(), cmd);
        }
        let gradient_provided = args.get("gradient_colors").is_some();
        let bloom_provided = args.get("bloom_color").is_some();
        if gradient_provided || bloom_provided {
            crate::overlay::send_command_for(
                cursor_id.clone(),
                cursor_overlay::OverlayCommand::SetGradient {
                    gradient_colors,
                    bloom_color: bloom_color.flatten(),
                },
            );
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
            .map(|s| if s.is_empty() { "(reverted to default)".to_owned() } else { s.to_owned() })
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
            read_only: true, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, _args: Value) -> ToolResult {
        let cfg = self.state.config.read().unwrap();
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        ToolResult::text("cua-driver-rs configuration")
            .with_structured(json!({
                "version": env!("CARGO_PKG_VERSION"),
                "platform": "linux",
                "capture_mode": cfg.capture_mode,
                "capture_scope": cfg.capture_scope,
                "max_image_dimension": cfg.max_image_dimension,
                "experimental_pip": pip_enabled,
                "experimental_pip_geometry": pip_geometry
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
                The experimental_pip keys persist to ~/.cua-driver/config.json and apply on next \
                daemon restart (the PiP backend is initialised once at startup; \
                Linux ships only the trait stub today — see issue #1729).".into(),
            input_schema: json!({"type":"object","properties":{
                "key":{"type":"string","description":"Name of a single config field to write ({key, value} shape). Pair with `value`."},
                "value":{"description":"New value for `key`. JSON type depends on the key."},
                "capture_mode":{"type":"string","enum":["som","vision","ax"],"description":"Legacy per-field shape. Default capture mode for get_window_state."},
                "capture_scope":{"type":"string","enum":["window","desktop"],"description":"Capture scope: single window or whole desktop. Default window."},
                "max_image_dimension":{"type":"integer","description":"Legacy per-field shape. Max dimension for screenshot resizing (0 = no limit)."},
                "experimental_pip":{"type":"boolean","description":"Enable the experimental PiP preview window (applies next restart; Linux backend stubbed)."},
                "experimental_pip_geometry":{"type":"string","description":"PiP window size + optional position in `WxH` or `WxH+X+Y` form."}
            },"additionalProperties":false}),
            read_only: false, destructive: false, idempotent: true, open_world: false,
        })
    }
    async fn invoke(&self, args: Value) -> ToolResult {
        use cua_driver_core::tool_args::ArgsExt;
        let mut cfg = self.state.config.write().unwrap();
        let mut parts = Vec::new();
        // {key, value} shape (what the Swift/macOS and Windows callers send).
        // Linux previously read only the legacy per-field keys below, so a
        // `{"key":"max_image_dimension","value":800}` write was silently
        // dropped (issue #1923). Dispatch on `key` to the same fields.
        if let (Some(key), Some(val)) = (
            args.get("key").and_then(|v| v.as_str()),
            args.get("value"),
        ) {
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
                "capture_scope" => match val.as_str() {
                    Some(s @ ("window" | "desktop")) => {
                        cfg.capture_scope = s.to_owned();
                        if let Err(e) = pip_preview::write_config_key("capture_scope", Value::String(s.to_owned())) {
                            tracing::warn!("set_config: failed to persist capture_scope: {e}");
                        }
                        parts.push(format!("capture_scope={s}"));
                    }
                    Some(other) => return ToolResult::error(format!("`capture_scope` must be \"window\" or \"desktop\", got \"{other}\".")),
                    None => return ToolResult::error(format!("`capture_scope` must be a string, got {val}.")),
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
                "experimental_pip" => match val.as_bool() {
                    Some(b) => {
                        if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(b)) {
                            return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
                        }
                        parts.push(format!("experimental_pip={b} (next restart)"));
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
                        parts.push(format!("experimental_pip_geometry={s} (next restart)"));
                    }
                    None => return ToolResult::error(format!("`experimental_pip_geometry` must be a string, got {val}.")),
                },
                other => return ToolResult::error(format!(
                    "Unknown config key `{other}`. Known: capture_mode, capture_scope, max_image_dimension, experimental_pip, experimental_pip_geometry."
                )),
            }
        }
        // Legacy per-field shape.
        if let Some(mode) = args.opt_str("capture_mode") {
            if let Err(e) = pip_preview::write_config_key("capture_mode", Value::String(mode.clone())) {
                tracing::warn!("set_config: failed to persist capture_mode: {e}");
            }
            parts.push(format!("capture_mode={mode}"));
            cfg.capture_mode = mode;
        }
        if let Some(scope) = args.opt_str("capture_scope") {
            if scope != "window" && scope != "desktop" {
                return ToolResult::error(format!("`capture_scope` must be \"window\" or \"desktop\", got \"{scope}\"."));
            }
            if let Err(e) = pip_preview::write_config_key("capture_scope", Value::String(scope.clone())) {
                tracing::warn!("set_config: failed to persist capture_scope: {e}");
            }
            parts.push(format!("capture_scope={scope}"));
            cfg.capture_scope = scope;
        }
        if let Some(dim) = args.opt_u64("max_image_dimension") {
            cfg.max_image_dimension = dim as u32;
            if let Err(e) = pip_preview::write_config_key("max_image_dimension", Value::from(dim)) {
                tracing::warn!("set_config: failed to persist max_image_dimension: {e}");
            }
            parts.push(format!("max_image_dimension={dim}"));
        }
        if let Some(enabled) = args.get("experimental_pip").and_then(|v| v.as_bool()) {
            if let Err(e) = pip_preview::write_config_key("experimental_pip", Value::Bool(enabled)) {
                return ToolResult::error(format!("failed to persist experimental_pip: {e}"));
            }
            parts.push(format!("experimental_pip={enabled} (next restart)"));
        }
        if let Some(geom) = args.opt_str("experimental_pip_geometry") {
            if pip_preview::PipGeometry::parse(&geom).is_none() {
                return ToolResult::error(format!(
                    "experimental_pip_geometry `{geom}` is not a valid WxH or WxH+X+Y string"
                ));
            }
            if let Err(e) = pip_preview::write_config_key("experimental_pip_geometry", Value::String(geom.clone())) {
                return ToolResult::error(format!("failed to persist experimental_pip_geometry: {e}"));
            }
            parts.push(format!("experimental_pip_geometry={geom} (next restart)"));
        }
        let msg = if parts.is_empty() {
            "Config unchanged (no known parameters).".to_owned()
        } else {
            format!("Config updated: {}", parts.join(", "))
        };
        let (pip_enabled, pip_geometry) = pip_preview::read_pip_keys_from_file();
        ToolResult::text(msg)
            .with_structured(json!({
                "capture_mode": cfg.capture_mode,
                "capture_scope": cfg.capture_scope,
                "max_image_dimension": cfg.max_image_dimension,
                "experimental_pip": pip_enabled,
                "experimental_pip_geometry": pip_geometry
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
        use cua_driver_core::tool_args::ArgsExt;
        let xid = match args.require_u64("window_id") { Ok(v) => v, Err(e) => return e };
        let pid = args.opt_u64("pid").map(|v| v as u32);
        let x1 = match args.opt_f64("x1") { Some(v) => v, None => return ToolResult::error("Missing x1") };
        let y1 = match args.opt_f64("y1") { Some(v) => v, None => return ToolResult::error("Missing y1") };
        let x2 = match args.opt_f64("x2") { Some(v) => v, None => return ToolResult::error("Missing x2") };
        let y2 = match args.opt_f64("y2") { Some(v) => v, None => return ToolResult::error("Missing y2") };
        if x2 <= x1 || y2 <= y1 { return ToolResult::error("x2 must be > x1 and y2 must be > y1"); }

        let state = self.state.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Route through the Wayland-aware window capture dispatcher so
            // pure-Wayland sessions surface a typed "per-window capture not
            // supported yet" error instead of accidentally calling the
            // X11-only path with a foreign-toplevel id.
            let png = crate::wayland::screenshot_window_dispatch(xid)?;
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
                use cua_driver_core::protocol::Content;
                ToolResult {
                    content: vec![
                        Content::image_jpeg(b64),
                        Content::text(format!("Zoom ({x1:.0},{y1:.0})–({x2:.0},{y2:.0}) → {w}×{h} px JPEG.")),
                    ],
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
        use cua_driver_core::tool_args::ArgsExt;
        let pid = args.u64_or("pid", 0) as u32;
        let text_raw = match args.require_str("text") { Ok(v) => v, Err(e) => return e };
        // Same trailing-protocol-tag scrub as TypeTextTool — see
        // cua_driver_core::text_sanitize for rationale.
        let text = cua_driver_core::text_sanitize::strip_trailing_agent_protocol_tags(&text_raw)
            .into_owned();
        let delay_ms = args.u64_or("delay_ms", 30);
        let xid_opt = args.opt_u64("window_id");
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
            if crate::wayland::is_wayland() {
                // Per-char `wtype` loop with the requested delay — mirrors the
                // X11 XSendEvent per-char path. Sleeping here is fine because
                // we're inside spawn_blocking.
                let mut buf = [0u8; 4];
                for ch in text.chars() {
                    let s = ch.encode_utf8(&mut buf);
                    crate::wayland::type_text(s)?;
                    if delay_ms > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                    }
                }
                return Ok(());
            }
            crate::input::send_type_text_with_delay(xid, &text, delay_ms)
        }).await;
        match result {
            Ok(Ok(())) => ToolResult::text(format!("Typed {text_len} character(s) with {delay_ms}ms delay.")),
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
                exit. Unsaved state is lost — prefer the cooperative path first.".into(),
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
        let pid_i = match args.get("pid").and_then(|v| v.as_i64()) {
            Some(p) if p > 0 && p <= i32::MAX as i64 => p as i32,
            Some(_) => return ToolResult::error("kill_app: `pid` must be a positive integer".to_string()),
            None => return ToolResult::error("kill_app: missing required integer field `pid`".to_string()),
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

// ── bring_to_front (Linux stub) ──────────────────────────────────────────────

pub struct BringToFrontTool;

static BTF_DEF: std::sync::OnceLock<ToolDef> = std::sync::OnceLock::new();

#[async_trait]
impl Tool for BringToFrontTool {
    fn def(&self) -> &ToolDef {
        BTF_DEF.get_or_init(|| ToolDef {
            name: "bring_to_front".into(),
            description:
                "Persistently activate a window so subsequent input lands on it. \
                 X11: EWMH _NET_ACTIVE_WINDOW activation (the `wmctrl -a` equivalent, \
                 proper timestamp handling to beat focus-stealing prevention) — call \
                 it before `delivery_mode:\"foreground\"` input to avoid a per-call \
                 flash, or to escalate when background injection didn't land. \
                 Wayland: a standalone activate is NOT exposed — the compositor's \
                 security model bundles activation into the virtual-pointer/click \
                 path, so use `delivery_mode:\"foreground\"` on the input call \
                 itself; this reports that constraint on Wayland rather than \
                 faking it. Matches the macOS / Windows bring_to_front rung."
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
        // Wayland: no standalone external activate (compositor security model
        // bundles it into the vptr/click path). Report honestly; the agent
        // escalates via delivery_mode:"foreground" on the input call instead.
        if crate::wayland::is_wayland() {
            return ToolResult::error(
                "bring_to_front: Wayland has no standalone window-activation API for \
                 external clients — the compositor bundles activation into the \
                 virtual-pointer/click path. Use delivery_mode:\"foreground\" on the \
                 click/type_text call itself (it activates the target as part of the \
                 injection)."
                    .to_string(),
            )
            .with_structured(serde_json::json!({
                "code": "bring_to_front_wayland_bundled",
                "platform": "linux",
                "session": "wayland",
                "suggestion":
                    "On Wayland, pass delivery_mode:\"foreground\" to click / type_text — \
                     activation is performed as part of the injection.",
            }));
        }

        // X11: resolve the target xid (window_id, else first window for pid).
        let xid = match args.opt_u64("window_id") {
            Some(x) => x,
            None => {
                let pid = args.u64_or("pid", 0) as u32;
                let windows = tokio::task::spawn_blocking(move || crate::x11::list_windows(Some(pid)))
                    .await.unwrap_or_default();
                match windows.first() {
                    Some(w) => w.xid,
                    None => return ToolResult::error(format!(
                        "bring_to_front: no window_id given and no windows found for pid {pid}."
                    )),
                }
            }
        };
        let r = tokio::task::spawn_blocking(move || crate::input::x11_activate_window_persistent(xid)).await;
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
    let state = ToolState::new();
    {
        let cursor_registry = state.cursor_registry.clone();
        let state_for_session_end = state.clone();
        cua_driver_core::session::register_session_end_hook(move |session_id| {
            cursor_registry.remove(session_id);
            crate::overlay::remove_cursor(session_id.to_owned());
            state_for_session_end.mouse_hold.lock().unwrap().remove(session_id);
            crate::input::forget_master_pointer(session_id);
        });
    }
    let mut r = ToolRegistry::new();
    r.register(Box::new(ListAppsTool));
    r.register(Box::new(ListWindowsTool));
    r.register(Box::new(GetWindowStateTool { state: state.clone() }));
    r.register(Box::new(LaunchAppTool));
    r.register(Box::new(KillAppTool));
    r.register(Box::new(BringToFrontTool));
    r.register(Box::new(ClickTool { state: state.clone() }));
    r.register(Box::new(DoubleClickTool { state: state.clone() }));
    r.register(Box::new(RightClickTool { state: state.clone() }));
    r.register(Box::new(DragTool { state: state.clone() }));
    r.register(Box::new(MouseButtonDownTool { state: state.clone() }));
    r.register(Box::new(MouseDragTool { state: state.clone() }));
    r.register(Box::new(MouseButtonUpTool { state: state.clone() }));
    r.register(Box::new(ParallelMouseDragTool { state: state.clone() }));
    r.register(Box::new(TypeTextTool));
    r.register(Box::new(PressKeyTool));
    r.register(Box::new(HotkeyTool));
    r.register(Box::new(SetValueTool));
    r.register(Box::new(ScrollTool));
    // `screenshot` removed - see the matching comment in
    // platform-windows/src/tools/impl_.rs::build_registry. Canonical
    // screenshot path is `get_window_state` with `capture_mode:"vision"`.
    let _ = compat;
    r.register(Box::new(GetScreenSizeTool));
    r.register(Box::new(GetDesktopStateTool));
    r.register(Box::new(GetCursorPositionTool));
    r.register(Box::new(MoveCursorTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorEnabledTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorMotionTool { state: state.clone() }));
    r.register(Box::new(GetAgentCursorStateTool { state: state.clone() }));
    r.register(Box::new(SetAgentCursorStyleTool { state: state.clone() }));
    r.register(Box::new(CheckPermissionsTool));
    // `health_report` — single-call cross-platform driver diagnostics.
    // Stable schema_version="1" contract for downstream consumers
    // (Hermes Agent, NousResearch/hermes-agent#47065). Linux skips
    // tcc_* and bundle_identity with "not applicable on Linux".
    r.register(Box::new(cua_driver_core::health_report::HealthReportTool::new(
        std::sync::Arc::new(crate::health_report::LinuxHealthProvider),
    )));
    r.register(Box::new(GetConfigTool { state: state.clone() }));
    r.register(Box::new(SetConfigTool { state: state.clone() }));
    r.register(Box::new(GetAccessibilityTreeTool));
    r.register(Box::new(ZoomTool { state: state.clone() }));
    r.register(Box::new(TypeTextCharsTool));
    // Cross-platform `page` tool definition lives in mcp-server; Linux plugs
    // in its AT-SPI + CDP backend here.
    r.register(Box::new(cua_driver_core::page::PageTool::new(
        Arc::new(super::page::LinuxPageBackend::new()),
    )));
    r.register_recording_tools();
    r.register_session_tools();
    r
}

#[cfg(test)]
mod click_button_schema_tests {
    use super::ClickTool;
    use cua_driver_core::tool::Tool;

    /// Surface 5: schema must advertise the three canonical button values and
    /// describe the back-compat default. Linux already routed button=middle/right
    /// pre-Surface-5; this freezes the schema shape so the contract can't drift.
    #[test]
    fn schema_advertises_button_enum_and_description() {
        let tool = ClickTool { state: super::ToolState::new() };
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
        assert!(lc.contains("wayland"), "description should call out wayland fallback");
    }
}

#[cfg(test)]
mod driver_config_tests {
    use super::DriverConfig;

    #[test]
    fn capture_scope_defaults_to_window() {
        assert_eq!(DriverConfig::default().capture_scope, "window");
    }
}
