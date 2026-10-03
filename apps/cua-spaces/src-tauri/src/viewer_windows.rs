// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Per-Space desktop windows and picture-in-picture pins.
//!
//! `open_space_window` gives each sandbox a normal, fullscreen-capable window
//! (on macOS the green traffic-light / fullscreen turns it into a real OS
//! Space via `NSWindowCollectionBehavior::FullScreenPrimary`).
//! `pin_space_pip` opens a small borderless always-on-top mirror using the
//! same status-window recipe as the notch portal.
//!
//! The viewer page discovers what to show by asking `viewer_config` for the
//! entry stored under its own window label — no URL query plumbing, which
//! keeps `WebviewUrl::App` trivial in both dev and bundled builds.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, State, WebviewUrl, WebviewWindow,
};

use crate::commands::AppState;
use crate::webview_data::IsolateWebview as _;

#[cfg(target_os = "macos")]
use objc2_app_kit::{NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior};

const PIP_WIDTH: f64 = 320.0;
const PIP_HEIGHT: f64 = 200.0;
const PIP_MARGIN: f64 = 24.0;

/// The teleport picker is a single, fixed-label window.
const TELEPORT_PICKER_LABEL: &str = "teleport-picker";
const TELEPORT_PICKER_WIDTH: f64 = 720.0;
const TELEPORT_PICKER_HEIGHT: f64 = 500.0;

/// Everything a viewer page needs to show one Space. The page asks the
/// shell for a media ticket itself (`open_space_stream`), so no endpoint or
/// credential travels in the window configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceWindowRequest {
    /// Space id (`local:<name>`, `cloud:<name>`, ...; also derives the window label).
    pub id: String,
    /// Human Space name (window title, PiP overlay label).
    pub name: String,
    /// Who is driving this Space right now: "agent" or "you". Feeds the PiP
    /// controller badge; purely informational.
    #[serde(default)]
    pub controller: Option<String>,
    /// Guest OS ("linux" | "macos" | "windows"), when known.
    #[serde(default)]
    pub os: Option<String>,
}

/// The single remote window a dedicated `winone-*` stream window mirrors. When
/// present the viewer runs in SINGLE mode: it opens exactly this window's RCDP
/// session, sizes the OS window to the remote inner size, and syncs resizes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetWindow {
    /// The window handle (`WindowRef.id`, matches `RemoteWindow.id`).
    pub id: String,
    pub app_name: String,
    pub title: String,
    /// True when this is an ADDITIONAL stream of a window already open
    /// elsewhere (shift-click, instance >= 2).
    ///
    /// The viewer needs this because the driver grants bidirectional geometry
    /// control to exactly ONE session per window (a second one fails with
    /// FAILED_PRECONDITION), so a replica opens observe-only from the start.
    pub replica: bool,
    /// A ticketed media URL minted by someone else (the MCP's
    /// `stream_space_window` through the control server). When set the
    /// viewer attaches to it instead of opening its own session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_url: Option<String>,
    /// The media session behind `media_url`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ViewerConfig {
    /// "space" (interactive), "pip" (view-only mirror), or "windows" (the
    /// per-window RCDP client hosting one panel per remote window).
    pub view: &'static str,
    pub space: SpaceWindowRequest,
    /// Most recent cached screenshot for this Space (`data:` URL), so the
    /// viewer can paint a blurred background while the live stream connects
    /// (item D). Filled at read time from the shared cache; `None` when nothing
    /// has been cached yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_screenshot: Option<String>,
    /// Any teleport transfer currently overlaying this Space window (item B).
    /// Filled at read time; `None` when no transfer is in flight.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transfer: Option<TransferOverlay>,
    /// The single remote window this stream window is dedicated to (the
    /// `winone-*` per-window path). `None` for the shared multi-window
    /// `win-*` client and for "space"/"pip" views.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_window: Option<TargetWindow>,
}

impl ViewerConfig {
    fn new(view: &'static str, space: SpaceWindowRequest) -> Self {
        Self {
            view,
            space,
            last_screenshot: None,
            transfer: None,
            target_window: None,
        }
    }

    /// A single-window stream config: one dedicated OS window per remote window.
    fn new_single(view: &'static str, space: SpaceWindowRequest, target: TargetWindow) -> Self {
        Self {
            view,
            space,
            last_screenshot: None,
            transfer: None,
            target_window: Some(target),
        }
    }
}

/// The transfer overlay's state for one Space window (item B). Mirrors the
/// front-end `TransferOverlayState`: `phase` is "active" or "error".
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferOverlay {
    pub phase: String,
    pub app_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Bytes uploaded so far, once the CLI starts reporting real upload
    /// progress. Drives the determinate bar and the "{x} MB / {y} MB" label;
    /// `None` during the brief pre-upload/export phase (indeterminate bar).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sent_bytes: Option<u64>,
    /// Total bytes to upload (the fully-buffered bundle size); `None` until the
    /// first `progress` line arrives.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
}

/// label -> viewer configuration for windows created by the shell.
pub struct ViewerConfigs(pub Mutex<HashMap<String, ViewerConfig>>);

impl Default for ViewerConfigs {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

/// Shared, app-wide "last screenshot" cache: Space id -> (`data:` URL, ts ms).
/// Written by `cache_space_screenshot` (fed from the tile poll and the hover
/// preload in the portal window) and read into `ViewerConfig` so viewer windows
/// — a separate webview — can show a recent frame. See `screenshotCache.ts`.
pub struct SpaceScreenshots(pub Mutex<HashMap<String, (String, i64)>>);

impl Default for SpaceScreenshots {
    fn default() -> Self {
        Self(Mutex::new(HashMap::new()))
    }
}

impl SpaceScreenshots {
    fn remember(&self, space_id: &str, data_url: String) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let mut map = self.0.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(space_id.to_string(), (data_url, ts));
    }

    fn latest(&self, space_id: &str) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(space_id)
            .map(|(url, _)| url.clone())
    }
}

/// window label -> in-flight transfer overlay (item B). Shared behind an `Arc`
/// so the streaming `teleport_push` reader can hold a handle while it forwards
/// upload progress from a blocking thread.
pub type TransferMap = Mutex<HashMap<String, TransferOverlay>>;

#[derive(Default)]
pub struct TransferState(pub std::sync::Arc<TransferMap>);

/// Parameters of the most recent teleport push for a Space, stored the moment a
/// push starts so the on-stream overlay's Retry can re-run the exact same push
/// in Rust — the centered picker is gone by then, so it can no longer relay it.
#[derive(Clone, Debug)]
pub struct LastPush {
    pub app_id: String,
    pub scope: String,
    pub include: Vec<String>,
    /// The app's display name (for the overlay heading on a Retry).
    pub app_name: String,
    /// Whether the consent sheet acknowledged sensitive items.
    pub acknowledge_sensitive: bool,
}

/// Space id -> the last push's params. Written by `teleport_push` when a push
/// begins; read by `retry_space_transfer` to re-run it, and cleared by
/// `cancel_space_transfer`.
#[derive(Default)]
pub struct LastPushState(pub Mutex<HashMap<String, LastPush>>);

/// A local app pre-selected for teleport (the AX window-drag / `.app`-drop
/// shortcut). When present the picker skips its window grid and opens straight
/// on the consent checklist for this app.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeleportApp {
    pub id: String,
    pub name: String,
}

/// What the teleport picker needs: the target Space (so the footer
/// reads "Teleport to {name}" and the push resolves the bound sandbox from
/// `space_id`), plus an optional pre-selected app for the drag shortcut.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeleportPickerRequest {
    pub space_id: String,
    pub space_name: String,
    #[serde(default)]
    pub app: Option<TeleportApp>,
    /// The preselected app's SDK catalog entry (core JSON), from a dropped
    /// bundle or a dragged window.
    #[serde(default)]
    pub entry: Option<serde_json::Value>,
    /// Files or folders dropped with the app.
    #[serde(default)]
    pub files: Vec<String>,
    /// Go straight to the consent screen (captures and demos only; set by
    /// `CUA_SPACES_START_VIEW=teleport`).
    #[serde(default)]
    pub auto_review: bool,
}

/// The single picker window's current target; read back by the page via
/// `teleport_picker_config`.
#[derive(Default)]
pub struct TeleportPickerState(pub Mutex<Option<TeleportPickerRequest>>);

/// One Space in the popped-out list window's roster.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacesListEntry {
    pub id: String,
    pub name: String,
    pub detail: String,
    pub status: String,
    /// Operating system ("macos" | "windows" | "linux"), so the list can show
    /// what each Space runs. Absent when a provider does not report one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

/// What the popped-out list window shows: the portal's current Spaces and which
/// one is selected (its windows lead the "Windows" section).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpacesListRequest {
    pub spaces: Vec<SpacesListEntry>,
    #[serde(default)]
    pub selected_id: Option<String>,
}

/// The main window's last requested selection; read back by the page via
/// `spaces_list_config`.
#[derive(Default)]
pub struct SpacesListState(pub Mutex<Option<SpacesListRequest>>);

/// The main window: an ordinary, opaque, decorated desktop window declared in
/// `tauri.conf.json` (label `main`). The notch's list button and the Dock
/// bring it forward.
pub const MAIN_LABEL: &str = "main";

/// Read the main window's last requested selection.
#[tauri::command]
pub fn spaces_list_config(state: State<'_, SpacesListState>) -> Result<SpacesListRequest, String> {
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "no Spaces list target is set".to_string())
}

/// Show the main window, selecting `request.selected_id` when given.
pub fn show_main_window(app: &AppHandle, selected: Option<&str>) -> Result<(), String> {
    let window = app
        .get_webview_window(MAIN_LABEL)
        .ok_or_else(|| "the main window is missing from tauri.conf.json".to_string())?;
    if let Some(id) = selected {
        let _ = Emitter::emit(&window, "main:select", serde_json::json!({ "spaceId": id }));
    }
    let _ = window.show();
    let _ = window.unminimize();
    window.set_focus().map_err(|e| e.to_string())
}

/// Open the main window (the notch's list button). Kept under its old name so
/// the portal's bridge is unchanged.
#[tauri::command]
pub fn open_spaces_list(
    app: AppHandle,
    state: State<'_, SpacesListState>,
    request: SpacesListRequest,
) -> Result<(), String> {
    let selected = request.selected_id.clone();
    if let Ok(mut current) = state.0.lock() {
        *current = Some(request);
    }
    show_main_window(&app, selected.as_deref())
}

/// Open the main window on its New Space sheet.
#[tauri::command]
pub fn open_new_space(app: AppHandle) -> Result<(), String> {
    show_main_window(&app, None)?;
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = Emitter::emit(&window, "main:new-space", ());
    }
    Ok(())
}

/// Open the main window on its Settings page.
#[tauri::command]
pub fn open_main_settings(app: AppHandle) -> Result<(), String> {
    show_main_window(&app, None)?;
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = Emitter::emit(&window, "main:settings", ());
    }
    Ok(())
}

/// Open the main window on its Volume page (Cua Volume's conflicts).
pub fn open_main_volume(app: AppHandle) -> Result<(), String> {
    show_main_window(&app, None)?;
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        let _ = Emitter::emit(&window, "main:volume", ());
    }
    Ok(())
}

/// Hide the main window (it is only ever hidden, never destroyed, so the
/// Dock and the notch can bring it back).
#[tauri::command]
pub fn close_spaces_list(app: AppHandle, state: State<'_, SpacesListState>) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(MAIN_LABEL) {
        window.hide().map_err(|e| e.to_string())?;
    }
    if let Ok(mut current) = state.0.lock() {
        *current = None;
    }
    Ok(())
}

/// Window labels must stay within Tauri's allowed alphabet.
pub(crate) fn sanitize_label(value: &str) -> String {
    let mut label: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if label.is_empty() {
        label.push('x');
    }
    label
}

pub(crate) fn space_window_label(space_id: &str) -> String {
    format!("space-{}", sanitize_label(space_id))
}

pub(crate) fn pip_window_label(space_id: &str) -> String {
    format!("pip-{}", sanitize_label(space_id))
}

pub(crate) fn window_stream_label(space_id: &str) -> String {
    format!("win-{}", sanitize_label(space_id))
}

/// Label for a DEDICATED per-remote-window stream window: one OS window per
/// remote window, distinct from the shared multi-window `win-*` client.
///
/// `instance` 1 is the primary window and keeps the historical, un-suffixed
/// label (so existing callers, the MCP control path and any persisted config
/// keep addressing the same window). Instances above 1 are the shift-click
/// REPLICAS and get a `-sN` suffix, which is what lets a second OS window exist
/// for the same remote target — each webview then opens its own rcdp session.
pub(crate) fn winone_window_label_at(space_id: &str, window_id: &str, instance: u8) -> String {
    let base = format!(
        "winone-{}-{}",
        sanitize_label(space_id),
        sanitize_label(window_id)
    );
    if instance <= 1 {
        base
    } else {
        format!("{base}-s{instance}")
    }
}

/// The primary (instance 1) label. Test-only: production always goes through
/// `winone_window_label_at`, which picks the slot.
#[cfg(test)]
fn winone_window_label(space_id: &str, window_id: &str) -> String {
    winone_window_label_at(space_id, window_id, 1)
}

/// How many concurrent stream windows one remote target may have.
///
/// Two, deliberately. Each window holds a full rcdp session — its own
/// WebSocket, decoder and framebuffer — so this is a real per-window resource
/// cost, and the affordance exists to test multi-participant presence, for
/// which two participants is the whole case. A shift-click once both exist
/// focuses the replica rather than opening a third.
pub(crate) const WINONE_MAX_INSTANCES: u8 = 2;

/// Pick the instance slot a shift-click should open: the lowest free slot in
/// `1..=WINONE_MAX_INSTANCES`, or `None` when every slot is taken (the caller
/// then focuses the last one instead of opening an unbounded pile of windows).
pub(crate) fn next_free_winone_instance(taken: &[u8]) -> Option<u8> {
    (1..=WINONE_MAX_INSTANCES).find(|slot| !taken.contains(slot))
}

/// Smallest inner size (logical points) a stream window may be resized to, so a
/// remote-driven or user-driven shrink can never collapse the window.
const STREAM_MIN_WIDTH: f64 = 240.0;
const STREAM_MIN_HEIGHT: f64 = 160.0;

/// Largest fraction of the monitor's work area a stream window should occupy.
/// A remote window reports its inner size in guest device pixels, so a Retina
/// guest window (e.g. Chrome at ~1.3–2x) would otherwise open far larger than
/// the host screen. The canvas CSS-scales to whatever the window ends up, so
/// shrinking to fit loses no detail.
const STREAM_MAX_SCREEN_FRACTION: f64 = 0.8;

/// Clamp a requested stream-window inner size to the minimum. Pure so the
/// clamp is unit-testable without a live window.
pub(crate) fn clamp_stream_size(width: f64, height: f64) -> (f64, f64) {
    (width.max(STREAM_MIN_WIDTH), height.max(STREAM_MIN_HEIGHT))
}

/// Clamp to the minimum, then, when `max_w`/`max_h` are positive, scale the size
/// DOWN proportionally (preserving aspect ratio) so it fits within them. Pure so
/// the fit is unit-testable without a live window.
pub(crate) fn fit_stream_size(width: f64, height: f64, max_w: f64, max_h: f64) -> (f64, f64) {
    let (mut w, mut h) = clamp_stream_size(width, height);
    if max_w > 0.0 && max_h > 0.0 && (w > max_w || h > max_h) {
        let scale = (max_w / w).min(max_h / h);
        w = (w * scale).max(STREAM_MIN_WIDTH);
        h = (h * scale).max(STREAM_MIN_HEIGHT);
    }
    (w, h)
}

fn store_config(state: &ViewerConfigs, label: &str, config: ViewerConfig) {
    state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(label.to_string(), config);
}

#[tauri::command]
pub fn viewer_config(
    window: WebviewWindow,
    state: State<'_, ViewerConfigs>,
    screenshots: State<'_, SpaceScreenshots>,
    transfers: State<'_, TransferState>,
) -> Result<ViewerConfig, String> {
    let label = window.label().to_string();
    let mut config = state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .get(&label)
        .cloned()
        .ok_or_else(|| format!("no viewer configuration for window {label:?}"))?;
    // Enrich with the shared caches at read time so the viewer paints a blurred
    // recent frame (D) and knows about an in-flight transfer (B) on mount.
    config.last_screenshot = screenshots.latest(&config.space.id);
    config.transfer = transfers
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .get(&label)
        .cloned();
    Ok(config)
}

/// On macOS the Space window MAY enter native full screen (green button) as
/// its own OS Space; it never does so on its own.
#[cfg(target_os = "macos")]
fn configure_space_ns_window(window: &WebviewWindow) -> Result<(), String> {
    let pointer = window.ns_window().map_err(|error| error.to_string())?;
    if pointer.is_null() {
        return Err("space NSWindow handle is null".to_string());
    }
    // SAFETY: same contract as `configure_notch_window` in lib.rs — Tauri owns
    // the NSWindow, we borrow it on the main thread without retaining it.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    native.setCollectionBehavior(
        NSWindowCollectionBehavior::FullScreenPrimary | NSWindowCollectionBehavior::Managed,
    );
    Ok(())
}

/// PiP windows reuse the notch portal's status-window recipe so they float
/// over every Space, but stay draggable by their background.
#[cfg(target_os = "macos")]
fn configure_pip_ns_window(window: &WebviewWindow) -> Result<(), String> {
    let pointer = window.ns_window().map_err(|error| error.to_string())?;
    if pointer.is_null() {
        return Err("pip NSWindow handle is null".to_string());
    }
    // SAFETY: see above.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    native.setLevel(NSStatusWindowLevel);
    native.setCollectionBehavior(crate::notch_collection_behavior());
    // Not movable-by-background: that makes the whole webview a drag region and
    // blocks edge/corner resizing (only one corner works). The PiP is dragged by
    // its name bar (`data-tauri-drag-region`) instead, leaving all edges resizable.
    native.setHidesOnDeactivate(false);
    native.setCanHide(false);
    Ok(())
}

/// A dedicated `winone-*` stream window must be a normal, movable, standalone
/// window — NOT bound to the Space's native-fullscreen macOS Space the way that
/// Space's own view is. `CanJoinAllSpaces` makes it appear on the active Space
/// (including while a fullscreen Space is frontmost) and `Managed` keeps it as
/// an ordinary window that survives the user leaving fullscreen. It stays a
/// top-level window (Tauri builds it with no parent), decorated and resizable.
///
/// Known limitation: while a native-fullscreen Space view is frontmost, macOS
/// still shows a newly-opened window within that Space; the user can move it out
/// after leaving fullscreen but not before. Fully separating it up front is the
/// deferred "leave fullscreen first" redesign, intentionally NOT done here.
#[cfg(target_os = "macos")]
fn configure_winone_ns_window(window: &WebviewWindow) -> Result<(), String> {
    let pointer = window.ns_window().map_err(|error| error.to_string())?;
    if pointer.is_null() {
        return Err("winone NSWindow handle is null".to_string());
    }
    // SAFETY: same contract as `configure_space_ns_window` — Tauri owns the
    // NSWindow; we borrow it on the main thread without retaining it.
    let native = unsafe { &*pointer.cast::<NSWindow>() };
    native.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces | NSWindowCollectionBehavior::Managed,
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn on_main_thread_configure(
    window: &WebviewWindow,
    configure: fn(&WebviewWindow) -> Result<(), String>,
) {
    let clone = window.clone();
    let _ = window.run_on_main_thread(move || {
        if let Err(reason) = configure(&clone) {
            eprintln!("[cua-spaces] window configuration failed: {reason}");
        }
    });
}

/// Read the current target for the teleport picker window.
#[tauri::command]
pub fn teleport_picker_config(
    state: State<'_, TeleportPickerState>,
) -> Result<TeleportPickerRequest, String> {
    state
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .clone()
        .ok_or_else(|| "no teleport picker target set".to_string())
}

/// Center `window` on the monitor that currently hosts the portal (the active
/// display the user is working on), falling back to the window's own current
/// monitor and then the primary.
fn center_picker(app: &AppHandle, window: &WebviewWindow) {
    let monitor = app
        .get_webview_window("portal")
        .and_then(|portal| portal.current_monitor().ok().flatten())
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let pos = monitor.position();
        let size = monitor.size();
        let mon_x = pos.x as f64 / scale;
        let mon_y = pos.y as f64 / scale;
        let mon_w = size.width as f64 / scale;
        let mon_h = size.height as f64 / scale;
        let x = mon_x + (mon_w - TELEPORT_PICKER_WIDTH).max(0.0) / 2.0;
        let y = mon_y + (mon_h - TELEPORT_PICKER_HEIGHT).max(0.0) / 2.0;
        let _ = window.set_position(LogicalPosition::new(x, y));
    }
}

/// Open (or re-target) the teleport picker for one Space. This is the
/// click path (per-tile Teleport button, `.app` drop) and the AX window-drag
/// shortcut (when `request.app` is set the picker jumps straight to consent).
#[tauri::command]
pub fn open_teleport_picker(
    app: AppHandle,
    state: State<'_, TeleportPickerState>,
    request: TeleportPickerRequest,
) -> Result<(), String> {
    // Record the target before the page loads / re-reads it.
    if let Ok(mut current) = state.0.lock() {
        *current = Some(request.clone());
    }

    // A picker is already open — re-target it in place and bring it forward.
    if let Some(existing) = app.get_webview_window(TELEPORT_PICKER_LABEL) {
        let _ = tauri::Emitter::emit(&existing, "teleport-picker:retarget", &request);
        let _ = existing.show();
        let _ = existing.unminimize();
        center_picker(&app, &existing);
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    let window = tauri::WebviewWindowBuilder::new(
        &app,
        TELEPORT_PICKER_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title(format!("Teleport to {}", request.space_name))
    .inner_size(TELEPORT_PICKER_WIDTH, TELEPORT_PICKER_HEIGHT)
    .min_inner_size(720.0, 520.0)
    // A standard, opaque window with the system title bar: consent screens are
    // ordinary windows, never a transparent overlay.
    .decorations(true)
    .transparent(false)
    .resizable(true)
    .isolated(&app)
    .build()
    .map_err(|e| e.to_string())?;

    center_picker(&app, &window);
    let _ = window.show();
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Close the teleport picker window and forget its target.
#[tauri::command]
pub fn close_teleport_picker(
    app: AppHandle,
    state: State<'_, TeleportPickerState>,
) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(TELEPORT_PICKER_LABEL) {
        window.close().map_err(|e| e.to_string())?;
    }
    if let Ok(mut current) = state.0.lock() {
        *current = None;
    }
    Ok(())
}

/// Open (or focus) the full desktop window for one Space.
#[tauri::command]
pub fn open_space_window(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space: SpaceWindowRequest,
) -> Result<(), String> {
    open_space_window_impl(&app, &state, space)
}

/// Command-free core so the loopback control server (MCP-driven) can also open a
/// Space viewer, not just the front-end.
pub fn open_space_window_impl(
    app: &AppHandle,
    configs: &ViewerConfigs,
    space: SpaceWindowRequest,
) -> Result<(), String> {
    let label = space_window_label(&space.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    store_config(configs, &label, ViewerConfig::new("space", space.clone()));
    build_space_window(app, &label, &space)?;
    Ok(())
}

/// Create and configure a Space desktop window (shared by `open_space_window`
/// and `begin_space_transfer`). A normal, resizable, decorated window: it
/// never enters native fullscreen on its own (that switched the user's macOS
/// Space when an agent opened a viewer); the green button still offers it.
/// The viewer scales the remote display to fit and re-aspects the window once
/// the stream reports its size.
fn build_space_window(
    app: &AppHandle,
    label: &str,
    space: &SpaceWindowRequest,
) -> Result<WebviewWindow, String> {
    let window = tauri::WebviewWindowBuilder::new(app, label, WebviewUrl::App("index.html".into()))
        .title(format!("{} - Cua Space", space.name))
        .inner_size(1280.0, 800.0)
        .min_inner_size(480.0, 320.0)
        .resizable(true)
        .center()
        .isolated(app)
        .build()
        .map_err(|e| e.to_string())?;

    #[cfg(target_os = "macos")]
    on_main_thread_configure(&window, configure_space_ns_window);

    let _ = window.show();
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(window)
}

/// Toggle per-window streaming for a Space.
///
/// Enabling opens one Tauri window (`win-<id>`) hosting the multi-window
/// client: the page reads its target from `viewer_config`, lists the
/// Space's windows (`list_remote_windows`) and opens one wire-v2 media
/// session per window (`open_space_stream`), each rendered as its own
/// framed panel. Disabling closes the window and forgets its config.
#[tauri::command]
pub fn set_window_stream(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space: SpaceWindowRequest,
    enabled: bool,
) -> Result<(), String> {
    let label = window_stream_label(&space.id);

    if !enabled {
        if let Some(window) = app.get_webview_window(&label) {
            window.close().map_err(|e| e.to_string())?;
        }
        if let Ok(mut configs) = state.0.lock() {
            configs.remove(&label);
        }
        return Ok(());
    }

    open_window_stream(&app, &state, &space)
}

/// Open (or focus) the `win-<id>` WindowStream window for a Space. Shared by
/// `set_window_stream` (kept for programmatic toggling) and
/// `stream_remote_windows` (the picker's "This Mac" tab).
fn open_window_stream(
    app: &AppHandle,
    state: &ViewerConfigs,
    space: &SpaceWindowRequest,
) -> Result<(), String> {
    let label = window_stream_label(&space.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    store_config(state, &label, ViewerConfig::new("windows", space.clone()));
    let window =
        tauri::WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title(format!("{} — Windows", space.name))
            .inner_size(1200.0, 800.0)
            .resizable(true)
            .isolated(app)
            .build()
            .map_err(|e| e.to_string())?;

    let _ = window.show();
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Stream ONE of the Space's REMOTE windows onto this Mac — the picker's "This
/// Mac" tab. The viewer mints its own media ticket for that window.
///
/// Unlike the shared multi-window `win-*` client, this opens a DEDICATED OS
/// window (`winone-<space>-<window>`) for the single picked remote window: the
/// OS titlebar/resize IS the frame. The window is sized to the remote inner
/// size once geometry is known (a placeholder `inner_size` until then), and the
/// single-mode viewer syncs resizes back to the guest.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn stream_remote_windows(
    app: AppHandle,
    viewers: State<'_, ViewerConfigs>,
    space_id: String,
    space_name: String,
    window_id: String,
    app_name: String,
    title: String,
    // `replica` — shift-click: open an ADDITIONAL stream window for this same
    // target rather than focusing the existing one, so two participants
    // can be driven side by side. Absent/false keeps the historical behaviour.
    replica: Option<bool>,
) -> Result<(), String> {
    let space = SpaceWindowRequest {
        id: space_id,
        name: space_name,
        controller: Some("you".into()),
        os: None,
    };
    open_winone_impl(
        &app,
        &viewers,
        space,
        &window_id,
        &app_name,
        &title,
        replica.unwrap_or(false),
        None,
    )
    .await
}

/// Command-free core that opens (or focuses) a dedicated `winone-*` window
/// streaming one remote window. Shared by the front-end command and the
/// loopback control server (`media` = a ticketed URL + session id the MCP
/// already minted; `None` lets the viewer open its own session).
#[allow(clippy::too_many_arguments)]
pub async fn open_winone_impl(
    app: &AppHandle,
    viewers: &ViewerConfigs,
    space: SpaceWindowRequest,
    window_id: &str,
    app_name: &str,
    title: &str,
    replica: bool,
    media: Option<(String, String)>,
) -> Result<(), String> {
    // Which instance slot this call opens.
    //
    // Plain activation ALWAYS means instance 1: click a row twice and you focus
    // the one window you already have, exactly as before. A shift-click
    // (`replica`) instead asks for an ADDITIONAL window, so it takes the lowest
    // free slot — the second one in practice. Two windows on one target are two
    // separate webviews, so each runs its own `RcdpClient`, its own `join` and
    // its own `open_session`: two distinct rcdp participants, which is the
    // point of the affordance (drive one, watch the other's cursor overlay).
    let instance = if replica {
        let taken: Vec<u8> = (1..=WINONE_MAX_INSTANCES)
            .filter(|slot| {
                app.get_webview_window(&winone_window_label_at(&space.id, window_id, *slot))
                    .is_some()
            })
            .collect();
        match next_free_winone_instance(&taken) {
            Some(slot) => slot,
            // Capped: focus the last replica instead of opening a third.
            None => WINONE_MAX_INSTANCES,
        }
    } else {
        1
    };

    let label = winone_window_label_at(&space.id, window_id, instance);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.unminimize();
        existing.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

    // If a Space desktop view is currently in native fullscreen (its own macOS
    // Space), step back to the desktop Space first (⌃←) so this stream window
    // opens on the normal desktop instead of trapped inside the fullscreen
    // Space. The fullscreen Space stays alive in Mission Control. See
    // `space_switch`.
    #[cfg(target_os = "macos")]
    if app.webview_windows().iter().any(|(other, window)| {
        other.starts_with("space-") && window.is_fullscreen().unwrap_or(false)
    }) {
        crate::space_switch::switch_to_user_desktop_space();
        tokio::time::sleep(std::time::Duration::from_millis(450)).await;
    }

    let target = TargetWindow {
        id: window_id.to_string(),
        app_name: app_name.to_string(),
        title: title.to_string(),
        // Only the primary asks for geometry control; see TargetWindow.
        replica: instance > 1,
        media_url: media.as_ref().map(|(url, _)| url.clone()),
        media_session_id: media.map(|(_, id)| id),
    };
    store_config(
        viewers,
        &label,
        ViewerConfig::new_single("windows", space, target),
    );

    // Standard decorations: the OS titlebar/resize is the window's frame. The
    // inner size is a placeholder the single-mode viewer resizes once the remote
    // geometry is known.
    // Two windows on the same target must be tellable apart while the user waves
    // the mouse in one and watches the other, so a replica names its instance.
    // The viewer appends its live rcdp session id once the session opens (see
    // WindowStream's `single` title), which is the on-screen proof that the two
    // windows really are two participants.
    let window =
        tauri::WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title(if instance > 1 {
                format!("{app_name} — {title} (stream {instance})")
            } else {
                format!("{app_name} — {title}")
            })
            .inner_size(960.0, 600.0)
            .resizable(true)
            .isolated(app)
            .build()
            .map_err(|e| e.to_string())?;

    // Make it a normal, movable, standalone window rather than one trapped in
    // the Space's fullscreen macOS Space (see `configure_winone_ns_window`).
    #[cfg(target_os = "macos")]
    on_main_thread_configure(&window, configure_winone_ns_window);

    let _ = window.show();
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
    window.set_focus().map_err(|e| e.to_string())?;
    Ok(())
}

/// Resize the CURRENT stream window's inner area to `width`×`height` LOGICAL
/// points (clamped to a sane minimum). Called by the single-mode viewer to size
/// the OS window to the remote window's inner size and to refit it when the
/// remote window's geometry changes.
#[tauri::command]
pub fn resize_stream_window(window: WebviewWindow, width: f64, height: f64) -> Result<(), String> {
    // Cap to a fraction of the current monitor's work area so a large remote
    // window doesn't open bigger than the host screen.
    let (max_w, max_h) = window
        .current_monitor()
        .ok()
        .flatten()
        .map(|monitor| {
            let scale = monitor.scale_factor();
            let size = monitor.size();
            (
                size.width as f64 / scale * STREAM_MAX_SCREEN_FRACTION,
                size.height as f64 / scale * STREAM_MAX_SCREEN_FRACTION,
            )
        })
        .unwrap_or((0.0, 0.0));
    let (w, h) = fit_stream_size(width, height, max_w, max_h);
    window
        .set_size(LogicalSize::new(w, h))
        .map_err(|e| e.to_string())
}

/// Pin a small view-only mirror of one Space in the bottom-right corner.
#[tauri::command]
pub fn pin_space_pip(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space: SpaceWindowRequest,
) -> Result<(), String> {
    pin_space_pip_impl(&app, &state, space)
}

/// Command-free core so the loopback control server (MCP-driven) can pin a Space
/// as picture-in-picture, not just the front-end.
pub fn pin_space_pip_impl(
    app: &AppHandle,
    configs: &ViewerConfigs,
    space: SpaceWindowRequest,
) -> Result<(), String> {
    let label = pip_window_label(&space.id);
    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        return Ok(());
    }

    store_config(configs, &label, ViewerConfig::new("pip", space.clone()));
    let window =
        tauri::WebviewWindowBuilder::new(app, &label, WebviewUrl::App("index.html".into()))
            .title(space.name.clone())
            .inner_size(PIP_WIDTH, PIP_HEIGHT)
            .decorations(true)
            .resizable(true)
            .always_on_top(true)
            .visible_on_all_workspaces(true)
            .skip_taskbar(true)
            .shadow(true)
            .isolated(app)
            .build()
            .map_err(|e| e.to_string())?;

    // Bottom-right corner of the window's monitor (fall back to primary).
    if let Ok(Some(monitor)) = window
        .current_monitor()
        .or_else(|_| window.primary_monitor())
    {
        let scale = monitor.scale_factor();
        let position = monitor.position();
        let size = monitor.size();
        let x = position.x as f64 / scale + size.width as f64 / scale - PIP_WIDTH - PIP_MARGIN;
        let y =
            position.y as f64 / scale + size.height as f64 / scale - PIP_HEIGHT - PIP_MARGIN * 3.0;
        let _ = window.set_size(LogicalSize::new(PIP_WIDTH, PIP_HEIGHT));
        let _ = window.set_position(LogicalPosition::new(x, y));
    }

    #[cfg(target_os = "macos")]
    on_main_thread_configure(&window, configure_pip_ns_window);

    let _ = window.show();
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
    Ok(())
}

/// Match the PiP mirror window to the remote desktop's aspect ratio (from the
/// framebuffer `width`/`height`) and lock it natively so macOS constrains
/// resizing to that aspect from every corner — smooth, and no letterboxing. Sized
/// once here; the native `contentAspectRatio` governs subsequent user resizes.
#[tauri::command]
pub fn set_pip_aspect(
    app: AppHandle,
    space_id: String,
    width: f64,
    height: f64,
) -> Result<(), String> {
    if width <= 0.0 || height <= 0.0 {
        return Ok(());
    }
    let label = pip_window_label(&space_id);
    let Some(window) = app.get_webview_window(&label) else {
        return Ok(());
    };
    #[cfg(target_os = "macos")]
    {
        let target = window.clone();
        let _ = window.run_on_main_thread(move || {
            if let Ok(pointer) = target.ns_window() {
                if !pointer.is_null() {
                    // SAFETY: same contract as `configure_pip_ns_window` — borrowed
                    // on the main thread, not retained.
                    let native = unsafe { &*pointer.cast::<NSWindow>() };
                    // Lock the *content* aspect (excludes the title bar) so every
                    // resize corner keeps the stream from letterboxing, then snap
                    // the content to that aspect and re-anchor bottom-right.
                    native.setContentAspectRatio(objc2_foundation::NSSize { width, height });
                    let content_w = PIP_WIDTH;
                    let content_h = (content_w * height / width).round();
                    native.setContentSize(objc2_foundation::NSSize {
                        width: content_w,
                        height: content_h,
                    });
                    if let Some(screen) = native.screen() {
                        let vf = screen.visibleFrame();
                        let frame = native.frame();
                        // AppKit origin is bottom-left; y grows upward.
                        let x = vf.origin.x + vf.size.width - frame.size.width - PIP_MARGIN;
                        let y = vf.origin.y + PIP_MARGIN;
                        native.setFrameOrigin(objc2_foundation::NSPoint { x, y });
                    }
                }
            }
        });
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Best-effort outer resize on other platforms (approximate: ignores the
        // decoration inset).
        let aspect = width / height;
        if let Ok(size) = window.inner_size() {
            let scale = window.scale_factor().unwrap_or(1.0);
            let w = (size.width as f64 / scale).round();
            let _ = window.set_size(LogicalSize::new(w, (w / aspect).round()));
        }
    }
    Ok(())
}

/// Close the PiP mirror for one Space, if present.
#[tauri::command]
pub fn unpin_space_pip(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space_id: String,
) -> Result<(), String> {
    unpin_space_pip_impl(&app, &state, &space_id)
}

pub fn unpin_space_pip_impl(
    app: &AppHandle,
    configs: &ViewerConfigs,
    space_id: &str,
) -> Result<(), String> {
    let label = pip_window_label(space_id);
    if let Some(window) = app.get_webview_window(&label) {
        window.close().map_err(|e| e.to_string())?;
    }
    if let Ok(mut map) = configs.0.lock() {
        map.remove(&label);
    }
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
    Ok(())
}

// --- Stream panels (picture in picture) -------------------------------------

/// Heard by the webview when a Stream panel (a Space's picture-in-picture
/// mirror or one of its windows' streams) opens or closes; it then re-reads
/// [`stream_panels`] and hands the core a `synced` event.
pub const STREAM_PANELS_CHANGED: &str = "stream-panels:changed";

/// Whether `label` is a Stream panel's window.
pub(crate) fn is_stream_panel_label(label: &str) -> bool {
    label.starts_with("pip-") || label.starts_with("winone-")
}

/// The Stream section rows (the core's row ids) whose panel is open for
/// `space_id`: the Desktop row for the Space's mirror, a window's handle for
/// its stream windows (primary or replica). `exists` says whether a label's
/// window is still up; configs of closed windows do not count.
pub(crate) fn open_panel_rows(
    configs: &HashMap<String, ViewerConfig>,
    space_id: &str,
    exists: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut labels: Vec<&String> = configs.keys().collect();
    labels.sort();
    let mut rows: Vec<String> = Vec::new();
    for label in labels {
        let config = &configs[label];
        if config.space.id != space_id || !exists(label) {
            continue;
        }
        let row = if *label == pip_window_label(space_id) {
            cua_spaces_app_core::spaces::stream::DESKTOP_ROW_ID.to_string()
        } else if let Some(target) = config
            .target_window
            .as_ref()
            .filter(|_| label.starts_with("winone-"))
        {
            target.id.clone()
        } else {
            continue;
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    rows
}

/// The rows of `space_id`'s Stream section whose panel is open now.
#[tauri::command]
pub fn stream_panels(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space_id: String,
) -> Vec<String> {
    let configs = state
        .0
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    open_panel_rows(&configs, &space_id, |label| {
        app.get_webview_window(label).is_some()
    })
}

/// Closes every stream window of one of the Space's windows (its primary
/// and any replica): the row's `pip.exit` button.
#[tauri::command]
pub fn close_stream_window(
    app: AppHandle,
    state: State<'_, ViewerConfigs>,
    space_id: String,
    window_id: String,
) -> Result<(), String> {
    for instance in 1..=WINONE_MAX_INSTANCES {
        let label = winone_window_label_at(&space_id, &window_id, instance);
        if let Some(window) = app.get_webview_window(&label) {
            window.close().map_err(|e| e.to_string())?;
        }
        if let Ok(mut map) = state.0.lock() {
            map.remove(&label);
        }
    }
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
    Ok(())
}

/// Every viewer window label of `space_id`: its desktop window, its mirror,
/// its window client and its window streams (primary and replicas).
pub(crate) fn space_window_labels(
    configs: &HashMap<String, ViewerConfig>,
    space_id: &str,
) -> Vec<String> {
    let mut labels: Vec<String> = configs
        .iter()
        .filter(|(_, c)| c.space.id == space_id)
        .map(|(label, _)| label.clone())
        .collect();
    for label in [
        space_window_label(space_id),
        pip_window_label(space_id),
        window_stream_label(space_id),
    ] {
        if !labels.contains(&label) {
            labels.push(label);
        }
    }
    labels.sort();
    labels
}

/// Closes every viewer window of a Space: its delete started, so nothing
/// streams from it any more.
pub(crate) fn close_space_windows(app: &AppHandle, space_id: &str) {
    let Some(state) = app.try_state::<ViewerConfigs>() else {
        return;
    };
    let labels = state
        .0
        .lock()
        .map(|map| space_window_labels(&map, space_id))
        .unwrap_or_default();
    for label in &labels {
        if let Some(window) = app.get_webview_window(label) {
            let _ = window.close();
        }
    }
    if let Ok(mut map) = state.0.lock() {
        for label in &labels {
            map.remove(label);
        }
    }
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
}

/// A Stream panel's window closed (from its own title bar or by us):
/// forget its config and tell the webview.
pub(crate) fn stream_panel_closed(app: &AppHandle, label: &str) {
    if !is_stream_panel_label(label) {
        return;
    }
    if let Some(state) = app.try_state::<ViewerConfigs>() {
        if let Ok(mut map) = state.0.lock() {
            map.remove(label);
        }
    }
    let _ = app.emit(STREAM_PANELS_CHANGED, ());
}

// --- Shared screenshot cache (items C/D) -------------------------------------

/// Record the latest screenshot for a Space into the shared cache. Called from
/// the portal window's tile poll and hover preload; read back by viewer windows
/// through `viewer_config`.
#[tauri::command]
pub fn cache_space_screenshot(
    screenshots: State<'_, SpaceScreenshots>,
    space_id: String,
    data_url: String,
) {
    if space_id.is_empty() || data_url.is_empty() {
        return;
    }
    screenshots.remember(&space_id, data_url);
}

// --- Teleport transfer overlay (item B) --------------------------------------

/// What `begin_space_transfer` needs: the Space id/name and the app being
/// teleported.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginTransferRequest {
    pub space_id: String,
    pub space_name: String,
    pub app_name: String,
}

/// Open (or focus) Space X's desktop window and mark a teleport transfer as in
/// flight over it (item B). Called by the picker the instant a push begins, so
/// the user lands on the live Space immediately with the transfer overlay up.
#[tauri::command]
pub async fn begin_space_transfer(
    app: AppHandle,
    viewers: State<'_, ViewerConfigs>,
    transfers: State<'_, TransferState>,
    request: BeginTransferRequest,
) -> Result<(), String> {
    let label = space_window_label(&request.space_id);
    let overlay = TransferOverlay {
        phase: "active".to_string(),
        app_name: request.app_name.clone(),
        message: None,
        sent_bytes: None,
        total_bytes: None,
    };
    // Record the overlay first so a freshly-created window reads it from
    // `viewer_config` on mount even if it misses the event below.
    if let Ok(mut map) = transfers.0.lock() {
        map.insert(label.clone(), overlay.clone());
    }
    // Light up the notch's transfer indicator (indeterminate until progress).
    emit_notch_transfer(&app, true, None, None);

    let signal = TransferSignalPayload {
        status: "start",
        app_name: Some(request.app_name.clone()),
        message: None,
        sent_bytes: None,
        total_bytes: None,
    };

    if let Some(existing) = app.get_webview_window(&label) {
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
        let _ = existing.emit("space-transfer", &signal);
        return Ok(());
    }

    let space = SpaceWindowRequest {
        id: request.space_id.clone(),
        name: request.space_name.clone(),
        controller: None,
        os: None,
    };
    store_config(&viewers, &label, ViewerConfig::new("space", space.clone()));
    // The confirmed teleport opens the live stream fullscreen (its own macOS
    // Space), which also keeps the notch UI from overlapping it.
    build_space_window(&app, &label, &space)?;
    // The window reads the active overlay from viewer_config on mount; the event
    // is a belt-and-braces nudge for an already-loaded page.
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.emit("space-transfer", &signal);
    }
    Ok(())
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferSignalPayload {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    app_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sent_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    total_bytes: Option<u64>,
}

/// Aggregate transfer signal for the notch: an active transfer shows a progress
/// indicator in the ambient portal's left-of-notch slot (like the hotspot /
/// provisioning indicators), so an in-flight upload is visible without the Space
/// window focused. Emitted to all windows; only the portal listens.
#[derive(serde::Serialize, Clone)]
struct NotchTransferPayload {
    active: bool,
    sent_bytes: Option<u64>,
    total_bytes: Option<u64>,
}

pub(crate) fn emit_notch_transfer(
    app: &AppHandle,
    active: bool,
    sent_bytes: Option<u64>,
    total_bytes: Option<u64>,
) {
    let _ = tauri::Emitter::emit(
        app,
        "notch:transfer",
        NotchTransferPayload {
            active,
            sent_bytes,
            total_bytes,
        },
    );
}

/// Write a terminal transfer signal into the shared state map and emit a
/// matching `space-transfer` event to the Space window. `status` is "done"
/// (clears the overlay, back to the live stream) or "error" (marks it failed
/// with `message`, showing Retry / Cancel). Shared by `update_space_transfer`,
/// `cancel_space_transfer`, and `teleport_push` — which now owns the terminal
/// signal itself, since the centered picker closes the moment the push starts.
pub(crate) fn emit_transfer_terminal(
    app: &AppHandle,
    transfers: &TransferMap,
    space_id: &str,
    status: &str,
    message: Option<String>,
) {
    let label = space_window_label(space_id);
    match status {
        "error" => {
            if let Ok(mut map) = transfers.lock() {
                if let Some(overlay) = map.get_mut(&label) {
                    overlay.phase = "error".to_string();
                    overlay.message = message.clone();
                }
            }
        }
        // "done" (and any clear) removes the overlay entirely.
        _ => {
            if let Ok(mut map) = transfers.lock() {
                map.remove(&label);
            }
        }
    }
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.emit(
            "space-transfer",
            &TransferSignalPayload {
                status: if status == "error" { "error" } else { "done" },
                app_name: None,
                message,
                sent_bytes: None,
                total_bytes: None,
            },
        );
    }
    // The transfer is over: clear the notch indicator.
    emit_notch_transfer(app, false, None, None);
}

/// Forward byte progress of an in-flight transfer to its Space window's
/// overlay and the notch indicator.
pub(crate) fn emit_transfer_progress(
    app: &AppHandle,
    transfers: &TransferMap,
    space_id: &str,
    sent_bytes: u64,
    total_bytes: u64,
) {
    let label = space_window_label(space_id);
    if let Ok(mut map) = transfers.lock() {
        if let Some(overlay) = map.get_mut(&label) {
            overlay.sent_bytes = Some(sent_bytes);
            overlay.total_bytes = Some(total_bytes);
        }
    }
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.emit(
            "space-transfer",
            &TransferSignalPayload {
                status: "progress",
                app_name: None,
                message: None,
                sent_bytes: Some(sent_bytes),
                total_bytes: Some(total_bytes),
            },
        );
    }
    emit_notch_transfer(app, true, Some(sent_bytes), Some(total_bytes));
}

/// Reset a Space's transfer overlay to the active phase and emit a "start"
/// signal (clearing any prior error and stale byte counters). Used by
/// `retry_space_transfer` before it re-runs the push. Preserves the overlay's
/// existing app name when `app_name` is empty.
pub(crate) fn emit_transfer_start(
    app: &AppHandle,
    transfers: &TransferMap,
    space_id: &str,
    app_name: &str,
) {
    let label = space_window_label(space_id);
    let name = {
        let mut map = transfers.lock().unwrap_or_else(|p| p.into_inner());
        let existing = map.get(&label).map(|overlay| overlay.app_name.clone());
        let name = if app_name.is_empty() {
            existing.unwrap_or_default()
        } else {
            app_name.to_string()
        };
        map.insert(
            label.clone(),
            TransferOverlay {
                phase: "active".to_string(),
                app_name: name.clone(),
                message: None,
                sent_bytes: None,
                total_bytes: None,
            },
        );
        name
    };
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.emit(
            "space-transfer",
            &TransferSignalPayload {
                status: "start",
                app_name: Some(name),
                message: None,
                sent_bytes: None,
                total_bytes: None,
            },
        );
    }
}

/// Update an in-flight transfer's overlay: `status` is "done" (clears it) or
/// "error" (shows the failure with Retry / Cancel). Retained as a command for
/// completeness, though `teleport_push` now emits the terminal signal itself.
#[tauri::command]
pub fn update_space_transfer(
    app: AppHandle,
    transfers: State<'_, TransferState>,
    space_id: String,
    status: String,
    message: Option<String>,
) -> Result<(), String> {
    match status.as_str() {
        "done" | "error" => {
            emit_transfer_terminal(&app, &transfers.0, &space_id, &status, message);
            Ok(())
        }
        other => Err(format!("unknown transfer status {other:?}")),
    }
}

/// Re-run a Space's teleport push (the on-stream overlay's Retry). Reads the
/// params stored when the push began and re-runs it entirely in Rust — the
/// centered picker is closed, so it can no longer drive this. When no params are
/// stored (nothing to retry), surfaces an error on the overlay instead.
#[tauri::command]
pub async fn retry_space_transfer(
    app: AppHandle,
    core: State<'_, AppState>,
    transfers: State<'_, TransferState>,
    last: State<'_, LastPushState>,
    space_id: String,
) -> Result<(), String> {
    let params = last
        .0
        .lock()
        .ok()
        .and_then(|map| map.get(&space_id).cloned());
    let Some(params) = params else {
        emit_transfer_terminal(
            &app,
            &transfers.0,
            &space_id,
            "error",
            Some(
                "This teleport can no longer be retried — start it again from the picker."
                    .to_string(),
            ),
        );
        return Ok(());
    };

    // Back to the active overlay (clears the error + stale bytes), then re-run
    // the push in Rust; it forwards progress and emits the terminal signal.
    emit_transfer_start(&app, &transfers.0, &space_id, &params.app_name);
    let _ =
        crate::commands::run_push(app, core.0.clone(), transfers.0.clone(), params, space_id).await;
    Ok(())
}

/// Cancel a Space's in-flight (or failed) teleport transfer: forget its stored
/// retry params and dismiss the overlay back to the live stream. The rcdp push
/// subprocess, if still running, is left to finish on its own; the overlay just
/// stops tracking it.
#[tauri::command]
pub fn cancel_space_transfer(
    app: AppHandle,
    transfers: State<'_, TransferState>,
    last: State<'_, LastPushState>,
    space_id: String,
) -> Result<(), String> {
    if let Ok(mut map) = last.0.lock() {
        map.remove(&space_id);
    }
    emit_transfer_terminal(&app, &transfers.0, &space_id, "done", None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_panels_are_the_mirror_and_window_streams_of_that_space() {
        let space = |id: &str| SpaceWindowRequest {
            id: id.into(),
            name: id.into(),
            controller: None,
            os: None,
        };
        let target = |id: &str, replica: bool| TargetWindow {
            id: id.into(),
            app_name: "Firefox".into(),
            title: "Docs".into(),
            replica,
            media_url: None,
            media_session_id: None,
        };
        let mut configs = HashMap::new();
        configs.insert(
            pip_window_label("local:a"),
            ViewerConfig::new("pip", space("local:a")),
        );
        configs.insert(
            winone_window_label_at("local:a", "w-1", 1),
            ViewerConfig::new_single("windows", space("local:a"), target("w-1", false)),
        );
        configs.insert(
            winone_window_label_at("local:a", "w-1", 2),
            ViewerConfig::new_single("windows", space("local:a"), target("w-1", true)),
        );
        configs.insert(
            winone_window_label_at("local:a", "w-2", 1),
            ViewerConfig::new_single("windows", space("local:a"), target("w-2", false)),
        );
        // Another Space's panel, and the Space's own interactive window.
        configs.insert(
            winone_window_label_at("local:b", "w-9", 1),
            ViewerConfig::new_single("windows", space("local:b"), target("w-9", false)),
        );
        configs.insert(
            space_window_label("local:a"),
            ViewerConfig::new("space", space("local:a")),
        );
        let all = open_panel_rows(&configs, "local:a", |_| true);
        assert_eq!(all, ["desktop", "w-1", "w-2"]);
        // A delete closes all of the Space's windows, and only its own.
        let labels = space_window_labels(&configs, "local:a");
        assert_eq!(labels.len(), 6, "{labels:?}");
        assert!(labels.contains(&window_stream_label("local:a")));
        assert!(!labels
            .iter()
            .any(|l| l.contains("local-b") || l.contains("local_b")));
        // A closed window's leftover config does not count.
        let closed = winone_window_label_at("local:a", "w-2", 1);
        let some = open_panel_rows(&configs, "local:a", |label| label != closed);
        assert_eq!(some, ["desktop", "w-1"]);
        assert!(is_stream_panel_label(&pip_window_label("x")));
        assert!(!is_stream_panel_label(&space_window_label("x")));
    }

    #[test]
    fn labels_are_sanitized_and_prefixed() {
        assert_eq!(
            space_window_label("fleet:ns/claim-1"),
            "space-fleet-ns-claim-1"
        );
        assert_eq!(pip_window_label("abc_DEF-1"), "pip-abc_DEF-1");
        assert_eq!(
            window_stream_label("fleet:ns/claim-1"),
            "win-fleet-ns-claim-1"
        );
        assert_eq!(sanitize_label(""), "x");
    }

    #[test]
    fn winone_labels_sanitize_both_segments() {
        // Both the space id and the (rcdp handle) window id are sanitized, so a
        // handle like "0x1f/2" can never break Tauri's label alphabet.
        assert_eq!(
            winone_window_label("fleet:ns/claim-1", "0x1f/2"),
            "winone-fleet-ns-claim-1-0x1f-2"
        );
        // An empty window id still yields a valid, unique-per-space label.
        assert_eq!(winone_window_label("space", ""), "winone-space-x");
    }

    /// Instance 1 MUST keep the historical label: the MCP control path, the
    /// picker and any stored viewer config all address the primary window by it,
    /// so suffixing it would silently orphan them.
    #[test]
    fn primary_winone_label_is_unsuffixed_and_replicas_are_distinct() {
        assert_eq!(
            winone_window_label_at("fleet:ns/claim-1", "0x1f/2", 1),
            winone_window_label("fleet:ns/claim-1", "0x1f/2"),
        );
        let primary = winone_window_label_at("space", "0x1f", 1);
        let replica = winone_window_label_at("space", "0x1f", 2);
        assert_eq!(replica, "winone-space-0x1f-s2");
        // Distinct labels are what let both windows exist at once; equal labels
        // would make the second call focus the first window instead.
        assert_ne!(primary, replica);
    }

    /// Shift-click takes the lowest free slot, and stops at the cap rather than
    /// opening an unbounded pile of rcdp sessions.
    #[test]
    fn replica_slots_fill_lowest_first_and_stop_at_the_cap() {
        assert_eq!(WINONE_MAX_INSTANCES, 2);
        // Nothing open yet: a shift-click is just the first window.
        assert_eq!(next_free_winone_instance(&[]), Some(1));
        // The usual case: one window streaming, shift-click adds the second.
        assert_eq!(next_free_winone_instance(&[1]), Some(2));
        // The first window was closed but the replica is still up — reuse slot 1
        // rather than inventing a third label.
        assert_eq!(next_free_winone_instance(&[2]), Some(1));
        // Capped: both slots taken, so no new window (the caller focuses).
        assert_eq!(next_free_winone_instance(&[1, 2]), None);
    }

    #[test]
    fn stream_size_is_clamped_to_minimum() {
        // Above the floor is passed through unchanged.
        assert_eq!(clamp_stream_size(960.0, 600.0), (960.0, 600.0));
        // Below the floor on either axis clamps to the minimum.
        assert_eq!(
            clamp_stream_size(10.0, 10.0),
            (STREAM_MIN_WIDTH, STREAM_MIN_HEIGHT)
        );
        assert_eq!(clamp_stream_size(1000.0, 5.0), (1000.0, STREAM_MIN_HEIGHT));
        assert_eq!(clamp_stream_size(5.0, 1000.0), (STREAM_MIN_WIDTH, 1000.0));
    }

    #[test]
    fn stream_size_fits_within_a_maximum() {
        // Fits already -> unchanged (min still applies).
        assert_eq!(fit_stream_size(960.0, 600.0, 1440.0, 900.0), (960.0, 600.0));
        // No maximum given -> just the minimum clamp.
        assert_eq!(fit_stream_size(3000.0, 2000.0, 0.0, 0.0), (3000.0, 2000.0));
        // Too wide/tall -> scaled DOWN preserving aspect ratio (2:1 stays 2:1).
        let (w, h) = fit_stream_size(2560.0, 1280.0, 1152.0, 900.0);
        assert!((w - 1152.0).abs() < 0.5, "w={w}");
        assert!((h - 576.0).abs() < 0.5, "h={h}");
        assert!((w / h - 2.0).abs() < 0.01, "aspect preserved");
    }
}
