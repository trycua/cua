//! Shared model and platform contract for Agent View.
//!
//! Agent View is an opt-in surface rather than one process-global "latest screenshot".
//! Frames carry an existing session/workspace identity and an exact native
//! window or browser-tab identity. Platform backends can therefore keep
//! several target cards visible without introducing target claims or leases.
//!
//! Native backends render the shared model on macOS, Windows, and Linux
//! X11/XWayland while keeping platform-specific window-system code isolated.

use std::sync::OnceLock;

use std::collections::HashMap;

mod desktop_layout;
mod session_tabs;

pub use desktop_layout::{
    layout_desktop, layout_desktop_with_shell, png_dimensions, DesktopLayout, LayoutRect,
    ShellStyle, TargetLayout, TargetSize,
};
pub use session_tabs::{layout_session_tabs, session_accent, SessionTab, SessionTabsLayout};

pub const AGENT_VIEW_DEFAULT_ENABLED: bool = false;

/// Canonical `~/.cua-driver/config.json` path matching what the per-platform
/// `set_config` tools write to. Resolves `$HOME` first (Unix/macOS) and falls
/// back to `%USERPROFILE%` (Windows, where `HOME` is usually unset). Returns
/// `None` when neither is set (sandboxed CI).
pub fn default_config_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(|h| {
            std::path::PathBuf::from(h)
                .join(".cua-driver")
                .join("config.json")
        })
}

/// Read a single key from `~/.cua-driver/config.json` as a raw JSON value,
/// returning `None` when the file is missing/malformed or the key is absent.
/// Used by the per-platform `load_driver_config` helpers to rehydrate the
/// in-memory `DriverConfig` at process startup so `set_config` writes survive
/// across stateless `cua-driver call` invocations.
pub fn read_config_value(key: &str) -> Option<serde_json::Value> {
    let path = default_config_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    json.get(key).cloned()
}

/// Merge a single `key`/`value` into `~/.cua-driver/config.json`,
/// preserving any other keys that are already there. Used by the
/// per-platform `set_config` tools to persist `agent_view` /
/// `agent_view_geometry` so the next daemon restart picks them up.
pub fn write_config_key(key: &str, value: serde_json::Value) -> Result<(), String> {
    let path = default_config_path().ok_or_else(|| "$HOME is not set".to_string())?;
    let mut json: serde_json::Value = path
        .exists()
        .then(|| std::fs::read_to_string(&path).ok())
        .flatten()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    json[key] = value;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let body = serde_json::to_string_pretty(&json).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(())
}

/// Read `agent_view` + `agent_view_geometry` from the
/// config file, falling back to defaults when missing or malformed.
/// Surfaced by the per-platform `get_config` tools alongside the
/// in-memory `DriverConfig` fields.
pub fn read_agent_view_keys_from_file() -> (bool, Option<String>) {
    let path = match default_config_path() {
        Some(p) => p,
        None => return (AGENT_VIEW_DEFAULT_ENABLED, None),
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return (AGENT_VIEW_DEFAULT_ENABLED, None),
    };
    let json: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return (AGENT_VIEW_DEFAULT_ENABLED, None),
    };
    let enabled = json
        .get("agent_view")
        .and_then(|v| v.as_bool())
        .unwrap_or(AGENT_VIEW_DEFAULT_ENABLED);
    let geometry = json
        .get("agent_view_geometry")
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned());
    (enabled, geometry)
}

/// Geometry of the PiP window, in screen points (top-left origin).
///
/// Parsed from `--agent-view-geometry WxH+X+Y`. `x` / `y` are
/// optional; when `None` the platform backend picks a sensible
/// "top-right corner with a small inset" default so a user enabling
/// the feature without any geometry flags still sees a window.
#[derive(Debug, Clone, Copy)]
pub struct PipGeometry {
    pub width: u32,
    pub height: u32,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for PipGeometry {
    fn default() -> Self {
        Self {
            width: 640,
            height: 420,
            x: None,
            y: None,
        }
    }
}

impl PipGeometry {
    /// Parse `WxH` or `WxH+X+Y` (matching the common X11 geometry form).
    /// Returns `None` on any parse failure so the caller can fall back
    /// to defaults without panicking.
    pub fn parse(s: &str) -> Option<Self> {
        // Split off the optional `+X+Y` tail first so the leading
        // `WxH` parses cleanly even when no position is provided.
        let (size, pos): (&str, Option<(i32, i32)>) = match s.find('+') {
            Some(i) => {
                let tail = &s[i + 1..];
                let mut parts = tail.split('+');
                let x = parts.next()?.parse().ok()?;
                let y = parts.next()?.parse().ok()?;
                (&s[..i], Some((x, y)))
            }
            None => (s, None),
        };
        let mut wh = size.split('x');
        let w: u32 = wh.next()?.parse().ok()?;
        let h: u32 = wh.next()?.parse().ok()?;
        Some(Self {
            width: w,
            height: h,
            x: pos.map(|p| p.0),
            y: pos.map(|p| p.1),
        })
    }
}

/// Configuration for the PiP window. Built by `main.rs` from CLI
/// flags and handed to `PipBackendFactory::start`.
#[derive(Debug, Clone)]
pub struct PipConfig {
    /// `--agent-view` is on argv. The factory is only consulted
    /// when this is true; the field is kept here so backends that
    /// share a `start()` path can early-return.
    pub enabled: bool,
    pub geometry: PipGeometry,
    /// Native window title shared by all platform backends.
    pub title: String,
}

impl Default for PipConfig {
    fn default() -> Self {
        Self {
            enabled: AGENT_VIEW_DEFAULT_ENABLED,
            geometry: PipGeometry::default(),
            title: "Cua Agent View".to_owned(),
        }
    }
}

impl PipConfig {
    /// Parse the Agent View CLI flags out of `std::env::args()`.
    /// Recognised flags:
    /// ```text
    /// --agent-view
    /// --no-agent-view
    /// --agent-view-geometry  WxH | WxH+X+Y
    /// ```
    /// Unknown flags are ignored so this never conflicts with the
    /// other arg-parser passes (CursorConfig, the subcommand router).
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::parse(&args[1..])
    }

    pub fn parse(args: &[String]) -> Self {
        let mut cfg = PipConfig::default();
        let (enabled, geometry) = Self::parse_overrides(args);
        if let Some(enabled) = enabled {
            cfg.enabled = enabled;
        }
        if let Some(geometry) = geometry {
            cfg.geometry = geometry;
        }
        cfg
    }

    fn parse_overrides(args: &[String]) -> (Option<bool>, Option<PipGeometry>) {
        let mut enabled = None;
        let mut geometry = None;
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--agent-view" => enabled = Some(true),
                "--no-agent-view" => enabled = Some(false),
                "--agent-view-geometry" => {
                    if let Some(geom) = args.get(i + 1).and_then(|s| PipGeometry::parse(s)) {
                        geometry = Some(geom);
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        (enabled, geometry)
    }

    /// Resolve the config from (in order of precedence, low → high):
    ///
    ///   defaults  →  `~/.cua-driver/config.json` keys
    ///                  (`agent_view` bool, `agent_view_geometry` string)
    ///              →  CLI flags
    ///
    /// Lets users persist `--agent-view` across daemon restarts by
    /// editing `~/.cua-driver/config.json` once, instead of re-running
    /// `claude mcp add` with the flag baked into the args list.
    /// Malformed or missing file falls back to the next layer silently.
    pub fn from_args_and_file(config_path: &std::path::Path) -> Self {
        let args: Vec<String> = std::env::args().skip(1).collect();
        Self::from_file_and_args(config_path, &args)
    }

    fn from_file_and_args(config_path: &std::path::Path, args: &[String]) -> Self {
        let mut cfg = PipConfig::default();
        if let Ok(text) = std::fs::read_to_string(config_path) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(b) = json.get("agent_view").and_then(|v| v.as_bool()) {
                    cfg.enabled = b;
                }
                if let Some(s) = json.get("agent_view_geometry").and_then(|v| v.as_str()) {
                    if let Some(g) = PipGeometry::parse(s) {
                        cfg.geometry = g;
                    }
                }
            }
        }
        // CLI args override anything in the file.
        let (enabled, geometry) = PipConfig::parse_overrides(args);
        if let Some(enabled) = enabled {
            cfg.enabled = enabled;
        }
        if let Some(geometry) = geometry {
            cfg.geometry = geometry;
        }
        cfg
    }
}

/// The exact surface represented by an Agent View card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipTargetKind {
    NativeWindow,
    BrowserTab,
}

impl PipTargetKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NativeWindow => "native_window",
            Self::BrowserTab => "browser_tab",
        }
    }
}

/// Stable target metadata used to group frames in the Agent View.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipTarget {
    /// Runtime-unique session key. This is grouping metadata, not an ownership
    /// or authorization claim.
    pub workspace_id: String,
    /// Short public session label shown to the user.
    pub workspace_label: String,
    /// Stable exact-target key within the workspace.
    pub target_id: String,
    /// Process-local identity used to de-duplicate equivalent bindings. This
    /// may be more stable than the public target reference (for example a CDP
    /// page target across repeated browser-window binds).
    pub identity_key: String,
    pub target_kind: PipTargetKind,
    pub target_label: String,
    /// Native window containing this target, when proven. Browser tabs use it
    /// to avoid a duplicate native Chrome/Edge window card.
    pub native_container: Option<PipNativeContainer>,
}

impl PipTarget {
    pub fn view_id(&self) -> String {
        view_id(&self.workspace_id, &self.identity_key)
    }
}

fn view_id(workspace_id: &str, identity_key: &str) -> String {
    // Both identities may contain `:`. Prefixing the workspace byte length
    // prevents cross-session concatenation aliases.
    format!("{}:{workspace_id}{identity_key}", workspace_id.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PipNativeContainer {
    pub pid: i64,
    pub window_id: u64,
}

/// A single exact-target frame pushed into the Agent View after a tool call.
///
/// `png_bytes` are the raw PNG bytes produced by the platform
/// screenshot callback — the same path that powers `screenshot.png`
/// in the recording pipeline, so PiP shows exactly what the recorder
/// sees.
#[derive(Debug, Clone)]
pub struct PipFrame {
    pub target: PipTarget,
    pub png_bytes: Vec<u8>,
    /// One-line summary shown overlayed on the frame, e.g.
    /// `click element_index=2` or `type_text "hello world"`.
    pub action_label: String,
    /// Wall-clock timestamp (ms since Unix epoch) — used by backends
    /// that want to show "last update Xs ago" in the title bar.
    pub timestamp_ms: u64,
    /// Normalized location of the synthetic agent pointer within the target image.
    pub cursor_position: Option<(f64, f64)>,
}

/// Platform-neutral latest-frame model with bounded target retention.
///
/// Backends use this to keep one card per exact target. The oldest-updated
/// card is evicted when the configured limit is reached; ending a workspace
/// removes all of its cards without closing the underlying applications.
pub struct PipViewModel {
    max_targets: usize,
    frames: HashMap<String, PipFrame>,
    workspace_activity_ms: HashMap<String, u64>,
    active_targets: HashMap<String, String>,
    selected_workspace_id: Option<String>,
    selection_pinned: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipWorkspaceSummary {
    pub workspace_id: String,
    pub workspace_label: String,
    pub target_count: usize,
    pub updated_ms: u64,
}

impl PipViewModel {
    pub fn new(max_targets: usize) -> Self {
        Self {
            max_targets: max_targets.max(1),
            frames: HashMap::new(),
            workspace_activity_ms: HashMap::new(),
            active_targets: HashMap::new(),
            selected_workspace_id: None,
            selection_pinned: false,
        }
    }

    pub fn upsert(&mut self, frame: PipFrame) -> Option<String> {
        let mut frame = frame;
        let workspace_id = frame.target.workspace_id.clone();
        self.workspace_activity_ms
            .entry(workspace_id.clone())
            .and_modify(|timestamp| *timestamp = (*timestamp).max(frame.timestamp_ms))
            .or_insert(frame.timestamp_ms);
        if self.selected_workspace_id.is_none() || !self.selection_pinned {
            self.selected_workspace_id = Some(workspace_id.clone());
        }
        if frame.target.target_kind == PipTargetKind::NativeWindow
            && frame.target.native_container.is_some_and(|container| {
                self.frames.values().any(|existing| {
                    existing.target.workspace_id == workspace_id
                        && existing.target.target_kind == PipTargetKind::BrowserTab
                        && existing.target.native_container == Some(container)
                })
            })
        {
            return None;
        }
        if frame.target.target_kind == PipTargetKind::BrowserTab {
            if let Some(container) = frame.target.native_container {
                self.frames.retain(|_, existing| {
                    existing.target.workspace_id != workspace_id
                        || existing.target.target_kind != PipTargetKind::NativeWindow
                        || existing.target.native_container != Some(container)
                });
            }
        }
        let view_id = frame.target.view_id();
        if frame.cursor_position.is_none() {
            frame.cursor_position = self
                .frames
                .get(&view_id)
                .and_then(|previous| previous.cursor_position);
        }
        self.frames.insert(view_id.clone(), frame);
        self.active_targets
            .insert(workspace_id.clone(), view_id.clone());
        if self.frames.len() <= self.max_targets {
            return None;
        }
        let counts = self
            .frames
            .values()
            .fold(HashMap::new(), |mut counts, frame| {
                *counts
                    .entry(frame.target.workspace_id.as_str())
                    .or_insert(0usize) += 1;
                counts
            });
        let largest_workspace = counts
            .iter()
            .max_by(|(left_id, left_count), (right_id, right_count)| {
                left_count
                    .cmp(right_count)
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(id, _)| *id);
        let evicted = self
            .frames
            .iter()
            .filter(|(id, _)| *id != &view_id)
            .min_by_key(|(_, candidate)| {
                (
                    candidate.target.workspace_id.as_str() != largest_workspace.unwrap_or(""),
                    candidate.timestamp_ms,
                )
            })
            .map(|(id, _)| id.clone());
        if let Some(id) = evicted.as_ref() {
            let workspace_id = self
                .frames
                .get(id)
                .map(|frame| frame.target.workspace_id.clone());
            self.frames.remove(id);
            self.clear_active_view(id);
            if let Some(workspace_id) = workspace_id.as_deref() {
                self.reconcile_active_target(workspace_id);
            }
        }
        self.prune_empty_workspace_activity();
        self.reconcile_selection();
        evicted
    }

    pub fn remove_target(&mut self, workspace_id: &str, identity_key: &str) -> bool {
        let direct = view_id(workspace_id, identity_key);
        let view_id = self
            .frames
            .contains_key(&direct)
            .then_some(direct)
            .or_else(|| {
                self.frames
                    .iter()
                    .find(|(_, frame)| {
                        frame.target.workspace_id == workspace_id
                            && frame.target.target_id == identity_key
                    })
                    .map(|(id, _)| id.clone())
            });
        let removed = view_id
            .as_ref()
            .is_some_and(|view_id| self.frames.remove(view_id).is_some());
        if removed {
            if let Some(view_id) = view_id.as_deref() {
                self.clear_active_view(view_id);
            }
            self.reconcile_active_target(workspace_id);
            self.prune_empty_workspace_activity();
            self.reconcile_selection();
        }
        removed
    }

    pub fn remove_workspace(&mut self, workspace_id: &str) -> Vec<String> {
        let removed = self
            .frames
            .iter()
            .filter(|(_, frame)| frame.target.workspace_id == workspace_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in &removed {
            self.frames.remove(id);
        }
        self.workspace_activity_ms.remove(workspace_id);
        self.active_targets.remove(workspace_id);
        if self.selected_workspace_id.as_deref() == Some(workspace_id) {
            self.selection_pinned = false;
        }
        self.reconcile_selection();
        removed
    }

    pub fn ordered_frames(&self) -> Vec<&PipFrame> {
        let mut frames = self.frames.values().collect::<Vec<_>>();
        frames.sort_by(|a, b| {
            a.target
                .workspace_label
                .cmp(&b.target.workspace_label)
                .then_with(|| b.timestamp_ms.cmp(&a.timestamp_ms))
                .then_with(|| a.target.target_id.cmp(&b.target.target_id))
        });
        frames
    }

    pub fn selected_workspace_id(&self) -> Option<&str> {
        self.selected_workspace_id.as_deref()
    }

    pub fn selected_frames(&self) -> Vec<&PipFrame> {
        let Some(selected) = self.selected_workspace_id() else {
            return Vec::new();
        };
        let mut frames = self
            .frames
            .values()
            .filter(|frame| frame.target.workspace_id == selected)
            .collect::<Vec<_>>();
        frames.sort_by(|a, b| {
            b.timestamp_ms
                .cmp(&a.timestamp_ms)
                .then_with(|| a.target.identity_key.cmp(&b.target.identity_key))
        });
        frames
    }

    pub fn active_view_id(&self) -> Option<&str> {
        let workspace_id = self.selected_workspace_id()?;
        self.active_targets.get(workspace_id).map(String::as_str)
    }

    pub fn workspaces(&self) -> Vec<PipWorkspaceSummary> {
        let mut summaries = HashMap::<&str, PipWorkspaceSummary>::new();
        for frame in self.frames.values() {
            let entry = summaries
                .entry(&frame.target.workspace_id)
                .or_insert_with(|| PipWorkspaceSummary {
                    workspace_id: frame.target.workspace_id.clone(),
                    workspace_label: sanitize_label(&frame.target.workspace_label, 48),
                    target_count: 0,
                    updated_ms: self
                        .workspace_activity_ms
                        .get(&frame.target.workspace_id)
                        .copied()
                        .unwrap_or(frame.timestamp_ms),
                });
            entry.target_count += 1;
        }
        let mut summaries = summaries.into_values().collect::<Vec<_>>();
        summaries.sort_by(|a, b| {
            b.updated_ms
                .cmp(&a.updated_ms)
                .then_with(|| a.workspace_label.cmp(&b.workspace_label))
                .then_with(|| a.workspace_id.cmp(&b.workspace_id))
        });
        summaries
    }

    pub fn select_workspace(&mut self, workspace_id: &str) -> bool {
        if !self
            .frames
            .values()
            .any(|frame| frame.target.workspace_id == workspace_id)
        {
            return false;
        }
        self.selected_workspace_id = Some(workspace_id.to_owned());
        self.selection_pinned = true;
        true
    }

    pub fn select_next_workspace(&mut self) -> bool {
        let workspaces = self.workspaces();
        if workspaces.len() <= 1 {
            return false;
        }
        let selected = self.selected_workspace_id();
        let index = workspaces
            .iter()
            .position(|workspace| Some(workspace.workspace_id.as_str()) == selected)
            .unwrap_or(0);
        self.select_workspace(&workspaces[(index + 1) % workspaces.len()].workspace_id)
    }

    pub fn selection_is_pinned(&self) -> bool {
        self.selection_pinned
    }

    fn reconcile_selection(&mut self) {
        let selected_exists = self
            .selected_workspace_id
            .as_deref()
            .is_some_and(|selected| {
                self.frames
                    .values()
                    .any(|frame| frame.target.workspace_id == selected)
            });
        if selected_exists {
            return;
        }
        self.selection_pinned = false;
        self.selected_workspace_id = self
            .workspaces()
            .into_iter()
            .next()
            .map(|workspace| workspace.workspace_id);
    }

    fn prune_empty_workspace_activity(&mut self) {
        self.workspace_activity_ms.retain(|workspace_id, _| {
            self.frames
                .values()
                .any(|frame| frame.target.workspace_id == *workspace_id)
        });
    }

    fn clear_active_view(&mut self, view_id: &str) {
        self.active_targets.retain(|_, active| active != view_id);
    }

    fn reconcile_active_target(&mut self, workspace_id: &str) {
        if self
            .active_targets
            .get(workspace_id)
            .is_some_and(|active| self.frames.contains_key(active))
        {
            return;
        }
        let fallback = self
            .frames
            .iter()
            .filter(|(_, frame)| frame.target.workspace_id == workspace_id)
            .max_by(|(left_id, left), (right_id, right)| {
                left.timestamp_ms
                    .cmp(&right.timestamp_ms)
                    .then_with(|| right_id.cmp(left_id))
            })
            .map(|(id, _)| id.clone());
        if let Some(fallback) = fallback {
            self.active_targets
                .insert(workspace_id.to_owned(), fallback);
        } else {
            self.active_targets.remove(workspace_id);
        }
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

fn sanitize_label(value: &str, max_chars: usize) -> String {
    let mut label = value
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect::<String>();
    if label.trim().is_empty() {
        label = "Agent".to_owned();
    }
    label
}

/// A live PiP window. Owned by `main.rs` for the lifetime of the
/// process; `shutdown()` consumes it and closes the window.
pub trait PipBackend: Send + Sync {
    /// Push a new frame to the window. Non-blocking; the backend is
    /// responsible for dispatching the actual draw to whatever thread
    /// its UI toolkit requires (the macOS impl dispatches to the main
    /// queue via `dispatch_async`).
    fn push_frame(&self, frame: PipFrame);

    /// Remove every card associated with an ended session/workspace. This
    /// changes only Agent View presentation; it does not close applications.
    fn remove_workspace(&self, workspace_id: &str);

    /// Remove one exact target from presentation. This never closes or
    /// otherwise mutates the underlying native window or browser tab.
    fn remove_target(&self, _workspace_id: &str, _identity_key: &str) {}

    /// Make the floating surface ignore or resume native pointer input.
    ///
    /// Implementations must not return until the native window-system state is
    /// applied. Cua Driver uses this around physical desktop actions so an
    /// overlapping always-on-top Agent View cannot intercept the action meant
    /// for an underlying target.
    fn set_input_passthrough(&self, _passthrough: bool) -> anyhow::Result<()> {
        Ok(())
    }

    /// Close the window and release native resources. Called from
    /// `main.rs` on shutdown.
    fn shutdown(self: Box<Self>);
}

/// Spawns a fresh PiP window. Registered once at startup via
/// `set_pip_backend_factory`.
pub trait PipBackendFactory: Send + Sync {
    fn start(&self, cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>>;
}

static PIP_FACTORY: OnceLock<Box<dyn PipBackendFactory>> = OnceLock::new();

/// Register the platform's PiP backend factory. Idempotent — subsequent
/// calls are silently ignored, matching the other startup-callback
/// setters in `cua_driver_core`.
pub fn set_pip_backend_factory(factory: Box<dyn PipBackendFactory>) {
    let _ = PIP_FACTORY.set(factory);
}

/// Start a PiP window using the registered backend. Returns an error
/// when no backend has been registered for this platform — `main.rs`
/// treats that as "PiP unavailable on this OS" and continues without
/// the window.
pub fn start_pip(cfg: &PipConfig) -> anyhow::Result<Box<dyn PipBackend>> {
    let factory = PIP_FACTORY
        .get()
        .ok_or_else(|| anyhow::anyhow!("no PiP backend registered for this platform"))?;
    factory.start(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(workspace: &str, target: &str, timestamp_ms: u64) -> PipFrame {
        PipFrame {
            target: PipTarget {
                workspace_id: workspace.to_owned(),
                workspace_label: workspace.to_owned(),
                target_id: target.to_owned(),
                identity_key: target.to_owned(),
                target_kind: PipTargetKind::NativeWindow,
                target_label: target.to_owned(),
                native_container: None,
            },
            png_bytes: vec![1],
            action_label: "click".to_owned(),
            timestamp_ms,
            cursor_position: None,
        }
    }

    fn browser_frame(
        workspace: &str,
        target: &str,
        identity: &str,
        container: PipNativeContainer,
        timestamp_ms: u64,
    ) -> PipFrame {
        PipFrame {
            target: PipTarget {
                workspace_id: workspace.to_owned(),
                workspace_label: workspace.to_owned(),
                target_id: target.to_owned(),
                identity_key: identity.to_owned(),
                target_kind: PipTargetKind::BrowserTab,
                target_label: target.to_owned(),
                native_container: Some(container),
            },
            png_bytes: vec![1],
            action_label: "get_browser_state".to_owned(),
            timestamp_ms,
            cursor_position: None,
        }
    }

    fn native_frame(
        workspace: &str,
        target: &str,
        container: PipNativeContainer,
        timestamp_ms: u64,
    ) -> PipFrame {
        let mut frame = frame(workspace, target, timestamp_ms);
        frame.target.native_container = Some(container);
        frame
    }

    #[test]
    fn keeps_distinct_targets_in_the_same_workspace() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "window:1:2", 1));
        model.upsert(frame("agent-a", "browser:bt:tab", 2));
        assert_eq!(model.len(), 2);
    }

    #[test]
    fn non_pointer_updates_preserve_the_last_cursor_position() {
        let mut model = PipViewModel::new(4);
        let mut clicked = frame("agent-a", "window:1:2", 1);
        clicked.cursor_position = Some((0.25, 0.75));
        model.upsert(clicked);
        model.upsert(frame("agent-a", "window:1:2", 2));
        assert_eq!(
            model.selected_frames()[0].cursor_position,
            Some((0.25, 0.75))
        );
    }

    #[test]
    fn updating_one_target_does_not_replace_another() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "window:1:2", 1));
        model.upsert(frame("agent-a", "browser:bt:tab", 2));
        model.upsert(frame("agent-a", "window:1:2", 3));
        assert_eq!(model.len(), 2);
        assert_eq!(model.ordered_frames()[0].target.target_id, "window:1:2");
    }

    #[test]
    fn ending_a_workspace_removes_only_its_cards() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "window:1:2", 1));
        model.upsert(frame("agent-b", "window:3:4", 2));
        assert_eq!(model.remove_workspace("agent-a").len(), 1);
        assert_eq!(model.len(), 1);
        assert_eq!(model.ordered_frames()[0].target.workspace_id, "agent-b");
    }

    #[test]
    fn evicts_the_oldest_other_target_at_capacity() {
        let mut model = PipViewModel::new(2);
        model.upsert(frame("agent-a", "old", 1));
        model.upsert(frame("agent-a", "newer", 2));
        assert_eq!(
            model.upsert(frame("agent-b", "newest", 3)),
            Some(view_id("agent-a", "old"))
        );
        assert_eq!(model.len(), 2);
    }

    #[test]
    fn follows_recent_activity_until_the_user_pins_a_workspace() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "a", 1));
        model.upsert(frame("agent-b", "b", 2));
        assert_eq!(model.selected_workspace_id(), Some("agent-b"));

        assert!(model.select_workspace("agent-a"));
        model.upsert(frame("agent-b", "b", 3));
        assert_eq!(model.selected_workspace_id(), Some("agent-a"));
        assert!(model.selection_is_pinned());
        assert_eq!(model.selected_frames().len(), 1);
    }

    #[test]
    fn active_target_is_scoped_to_the_selected_workspace() {
        let mut model = PipViewModel::new(6);
        model.upsert(frame("agent-a", "a-old", 1));
        model.upsert(frame("agent-a", "a-active", 2));
        model.upsert(frame("agent-b", "b-old", 3));
        model.upsert(frame("agent-b", "b-active", 4));

        assert_eq!(
            model.active_view_id(),
            Some(view_id("agent-b", "b-active").as_str())
        );
        assert!(model.select_workspace("agent-a"));
        assert_eq!(
            model.active_view_id(),
            Some(view_id("agent-a", "a-active").as_str())
        );
        assert!(model.select_workspace("agent-b"));
        assert_eq!(
            model.active_view_id(),
            Some(view_id("agent-b", "b-active").as_str())
        );
    }

    #[test]
    fn removing_an_active_target_falls_back_within_its_workspace() {
        let mut model = PipViewModel::new(6);
        model.upsert(frame("agent-a", "older", 1));
        model.upsert(frame("agent-a", "newer", 2));
        model.upsert(frame("agent-b", "other", 3));
        assert!(model.select_workspace("agent-a"));

        assert!(model.remove_target("agent-a", "newer"));

        assert_eq!(
            model.active_view_id(),
            Some(view_id("agent-a", "older").as_str())
        );
        assert_eq!(model.selected_workspace_id(), Some("agent-a"));
    }

    #[test]
    fn evicting_an_active_target_falls_back_within_its_workspace() {
        let mut model = PipViewModel::new(3);
        model.upsert(frame("agent-a", "fallback", 10));
        model.upsert(frame("agent-a", "active", 1));
        model.upsert(frame("agent-b", "other", 20));
        assert!(model.select_workspace("agent-a"));

        assert_eq!(
            model.upsert(frame("agent-c", "new", 30)),
            Some(view_id("agent-a", "active"))
        );
        assert_eq!(
            model.active_view_id(),
            Some(view_id("agent-a", "fallback").as_str())
        );
    }

    #[test]
    fn removing_the_selected_workspace_falls_back_to_the_most_recent() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "a", 1));
        model.upsert(frame("agent-b", "b", 2));
        assert!(model.select_workspace("agent-a"));
        model.remove_workspace("agent-a");
        assert_eq!(model.selected_workspace_id(), Some("agent-b"));
        assert!(!model.selection_is_pinned());
    }

    #[test]
    fn exact_target_removal_uses_the_internal_identity_key() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("agent-a", "window:1:2", 1));
        assert!(model.remove_target("agent-a", "window:1:2"));
        assert!(model.is_empty());
        assert_eq!(model.selected_workspace_id(), None);
    }

    #[test]
    fn equal_public_labels_do_not_merge_private_workspaces() {
        let mut model = PipViewModel::new(4);
        let mut first = frame("private-a", "one", 1);
        first.target.workspace_label = "research".to_owned();
        let mut second = frame("private-b", "two", 2);
        second.target.workspace_label = "research".to_owned();
        model.upsert(first);
        model.upsert(second);

        let workspaces = model.workspaces();
        assert_eq!(workspaces.len(), 2);
        assert!(workspaces
            .iter()
            .all(|workspace| workspace.workspace_label == "research"));
        assert_eq!(model.selected_workspace_id(), Some("private-b"));
    }

    #[test]
    fn stable_browser_identity_updates_across_repeated_bindings() {
        let mut model = PipViewModel::new(4);
        let container = PipNativeContainer {
            pid: 42,
            window_id: 7,
        };
        model.upsert(browser_frame(
            "agent-a",
            "browser:first:tab-a",
            "browser-cdp:page-1",
            container,
            1,
        ));
        model.upsert(browser_frame(
            "agent-a",
            "browser:second:tab-b",
            "browser-cdp:page-1",
            container,
            2,
        ));

        assert_eq!(model.len(), 1);
        assert_eq!(
            model.selected_frames()[0].target.target_id,
            "browser:second:tab-b"
        );
        assert_eq!(model.selected_frames()[0].timestamp_ms, 2);
    }

    #[test]
    fn browser_tabs_replace_and_suppress_their_native_container_card() {
        let mut model = PipViewModel::new(6);
        let container = PipNativeContainer {
            pid: 42,
            window_id: 7,
        };
        model.upsert(native_frame("agent-a", "window:42:7", container, 1));
        model.upsert(browser_frame(
            "agent-a",
            "browser:binding:tab-a",
            "browser-cdp:page-a",
            container,
            2,
        ));
        model.upsert(browser_frame(
            "agent-a",
            "browser:binding:tab-b",
            "browser-cdp:page-b",
            container,
            3,
        ));
        model.upsert(native_frame("agent-a", "window:42:7", container, 4));

        let frames = model.selected_frames();
        assert_eq!(frames.len(), 2);
        assert!(frames
            .iter()
            .all(|frame| frame.target.target_kind == PipTargetKind::BrowserTab));
    }

    #[test]
    fn suppressed_native_container_activity_still_drives_auto_follow() {
        let mut model = PipViewModel::new(6);
        let first_container = PipNativeContainer {
            pid: 42,
            window_id: 7,
        };
        let second_container = PipNativeContainer {
            pid: 84,
            window_id: 9,
        };
        model.upsert(browser_frame(
            "agent-a",
            "browser:first:tab-a",
            "browser-cdp:page-a",
            first_container,
            1,
        ));
        model.upsert(browser_frame(
            "agent-b",
            "browser:second:tab-b",
            "browser-cdp:page-b",
            second_container,
            2,
        ));
        assert_eq!(model.selected_workspace_id(), Some("agent-b"));

        model.upsert(native_frame("agent-a", "window:42:7", first_container, 3));

        assert_eq!(model.selected_workspace_id(), Some("agent-a"));
        assert_eq!(model.workspaces()[0].workspace_id, "agent-a");
        assert_eq!(model.workspaces()[0].updated_ms, 3);
        assert!(model
            .selected_frames()
            .iter()
            .all(|frame| frame.target.target_kind == PipTargetKind::BrowserTab));
    }

    #[test]
    fn private_session_and_target_delimiters_cannot_alias() {
        let mut model = PipViewModel::new(4);
        model.upsert(frame("private:a", "b", 1));
        model.upsert(frame("private", "a:b", 2));

        assert_eq!(model.len(), 2);
        assert!(model.select_workspace("private:a"));
        assert_eq!(model.selected_frames()[0].target.target_id, "b");
        assert!(model.select_workspace("private"));
        assert_eq!(model.selected_frames()[0].target.target_id, "a:b");
    }

    #[test]
    fn eviction_prefers_the_most_populated_workspace() {
        let mut model = PipViewModel::new(3);
        model.upsert(frame("agent-a", "old-a", 1));
        model.upsert(frame("agent-a", "new-a", 2));
        model.upsert(frame("agent-b", "only-b", 3));
        assert_eq!(
            model.upsert(frame("agent-c", "only-c", 4)),
            Some(view_id("agent-a", "old-a"))
        );
        assert!(model
            .ordered_frames()
            .iter()
            .any(|frame| frame.target.workspace_id == "agent-b"));
    }

    #[test]
    fn parses_agent_view_cli_names() {
        let cfg = PipConfig::parse(&[
            "--agent-view".to_owned(),
            "--agent-view-geometry".to_owned(),
            "640x420+24+36".to_owned(),
        ]);
        assert!(cfg.enabled);
        assert_eq!(cfg.geometry.width, 640);
        assert_eq!(cfg.geometry.height, 420);
        assert_eq!(cfg.geometry.x, Some(24));
        assert_eq!(cfg.geometry.y, Some(36));
    }

    #[test]
    fn agent_view_is_disabled_by_default() {
        let cfg = PipConfig::parse(&[]);
        assert!(!cfg.enabled);
        assert!(!PipConfig::default().enabled);
    }

    #[test]
    fn ignores_removed_pip_cli_names() {
        let cfg = PipConfig::parse(&[
            "--experimental-pip".to_owned(),
            "--experimental-pip-geometry".to_owned(),
            "640x420".to_owned(),
            "--pip".to_owned(),
        ]);
        assert!(!cfg.enabled);
        assert_eq!(cfg.geometry.width, PipGeometry::default().width);
        assert_eq!(cfg.geometry.height, PipGeometry::default().height);
    }

    #[test]
    fn reads_agent_view_config_names() {
        let path = std::env::temp_dir().join(format!(
            "cua-agent-view-config-{}-new.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"agent_view":true,"agent_view_geometry":"800x600+12+24"}"#,
        )
        .unwrap();

        let cfg = PipConfig::from_args_and_file(&path);
        assert!(cfg.enabled);
        assert_eq!(cfg.geometry.width, 800);
        assert_eq!(cfg.geometry.height, 600);
        assert_eq!(cfg.geometry.x, Some(12));
        assert_eq!(cfg.geometry.y, Some(24));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn config_file_overrides_defaults_without_cli_flags() {
        let path = std::env::temp_dir().join(format!(
            "cua-agent-view-config-{}-file-precedence.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"agent_view":false,"agent_view_geometry":"800x600+12+24"}"#,
        )
        .unwrap();

        let cfg = PipConfig::from_file_and_args(&path, &[]);
        assert!(!cfg.enabled);
        assert_eq!(cfg.geometry.width, 800);
        assert_eq!(cfg.geometry.height, 600);
        assert_eq!(cfg.geometry.x, Some(12));
        assert_eq!(cfg.geometry.y, Some(24));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn agent_view_cli_overrides_disabled_config_file() {
        let path = std::env::temp_dir().join(format!(
            "cua-agent-view-config-{}-cli-enable.json",
            std::process::id()
        ));
        std::fs::write(&path, r#"{"agent_view":false}"#).unwrap();

        let cfg = PipConfig::from_file_and_args(&path, &["--agent-view".to_owned()]);
        assert!(cfg.enabled);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cli_flags_override_config_file_values() {
        let path = std::env::temp_dir().join(format!(
            "cua-agent-view-config-{}-cli-precedence.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"agent_view":true,"agent_view_geometry":"800x600+12+24"}"#,
        )
        .unwrap();

        let cfg = PipConfig::from_file_and_args(
            &path,
            &[
                "--no-agent-view".to_owned(),
                "--agent-view-geometry".to_owned(),
                "720x480+30+40".to_owned(),
            ],
        );
        assert!(!cfg.enabled);
        assert_eq!(cfg.geometry.width, 720);
        assert_eq!(cfg.geometry.height, 480);
        assert_eq!(cfg.geometry.x, Some(30));
        assert_eq!(cfg.geometry.y, Some(40));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn ignores_removed_pip_config_names() {
        let path = std::env::temp_dir().join(format!(
            "cua-agent-view-config-{}-old.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"experimental_pip":true,"experimental_pip_geometry":"800x600"}"#,
        )
        .unwrap();

        let cfg = PipConfig::from_args_and_file(&path);
        assert!(!cfg.enabled);
        assert_eq!(cfg.geometry.width, PipGeometry::default().width);
        assert_eq!(cfg.geometry.height, PipGeometry::default().height);

        let _ = std::fs::remove_file(path);
    }
}
