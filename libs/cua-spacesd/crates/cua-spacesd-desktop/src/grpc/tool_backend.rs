// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! [`DesktopBackend`] over the cua-driver tool registry and the rcdp media
//! providers (macOS: ScreenCaptureKit/VideoToolbox, Windows: WGC/Media
//! Foundation or OpenH264).
//!
//! Windows come from the shared `CuaProviderBundle` catalog joined with the
//! driver's `list_windows` facts, so a handle from `WindowsService` streams
//! through `StreamService` unchanged. Input, window management and
//! accessibility go through the same driver tools an MCP client would call
//! (`click`, `type_text`, `get_window_state`, …). Screenshots take one BGRA
//! frame from the capture provider.
//!
//! The two seams ([`DriverTools`] and [`NativeCatalog`]) let tests drive the
//! backend with fakes; nothing here touches the host by itself.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use cua_media_protocol::{TargetEpoch, TargetHandle};
use cua_spacesd_provider_api::{
    CaptureConfig, CaptureEvent, CaptureSink, OwnedFrame, PixelFormat, ProviderDisplay,
    ProviderError, ProviderErrorCode, ProviderTargetId,
};
use cua_spacesd_session::media::MediaProviders;
use serde_json::{json, Value};

use super::backend::*;

/// Blocking access to the driver tools (called from `spawn_blocking`).
pub trait DriverTools: Send + Sync + 'static {
    /// Structured result, or the tool's error text.
    fn invoke(&self, name: &str, arguments: Value) -> Result<Value, String>;
    fn has(&self, name: &str) -> bool;
}

/// One catalogued window: the opaque handle and its native identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeWindow {
    pub handle: TargetHandle,
    pub epoch: TargetEpoch,
    pub id: ProviderTargetId,
    pub pid: i64,
    pub window_id: u64,
}

/// The window catalog shared with the media providers.
pub trait NativeCatalog: Send + Sync + 'static {
    fn windows(&self) -> Result<Vec<NativeWindow>, ProviderError>;
}

/// [`DriverTools`] over the in-process cua-driver registry.
pub struct RegistryTools(pub Arc<cua_driver_core::tool::ToolRegistry>);

impl DriverTools for RegistryTools {
    fn invoke(&self, name: &str, mut arguments: Value) -> Result<Value, String> {
        if let Value::Object(map) = &mut arguments {
            map.entry("_session_id")
                .or_insert_with(|| crate::CUA_PROVIDER_SESSION_ID.into());
        }
        let registry = self.0.clone();
        let name = name.to_owned();
        // Pointer arguments are window pixels this backend computed from the
        // window's bounds and the display scale, not from a driver
        // screenshot, so they go through the in-process native-pixel entry.
        let result = block_on_driver(async move {
            registry
                .invoke_with_native_window_pixels(&name, arguments)
                .await
        })?;
        if result.is_error == Some(true) {
            return Err(crate::tool_result_text(&result));
        }
        Ok(result
            .structured_content
            .clone()
            .or_else(|| crate::tool_result_json(&result))
            .unwrap_or(Value::Null))
    }

    fn has(&self, name: &str) -> bool {
        self.0.tool_names().any(|tool| tool == name)
    }
}

/// Run a driver call to completion from synchronous code, whatever runtime
/// (if any) the caller is on.
pub(crate) fn block_on_driver<F>(call: F) -> Result<F::Output, String>
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    let fresh = |call| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .map(|runtime| runtime.block_on(call))
    };
    // The gRPC services call the backend from async handlers: block in
    // place on a multi-thread runtime (a plain `block_on` on a worker
    // panics), else run the call on its own thread and runtime.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            Ok(tokio::task::block_in_place(|| handle.block_on(call)))
        }
        Ok(_) => std::thread::scope(|scope| {
            scope
                .spawn(|| fresh(call))
                .join()
                .map_err(|_| "driver tool thread panicked".to_string())?
        }),
        Err(_) => fresh(call),
    }
}

/// [`NativeCatalog`] over the provider bundle's target catalog.
pub struct BundleCatalog(pub Arc<crate::CuaProviderBundle>);

impl NativeCatalog for BundleCatalog {
    fn windows(&self) -> Result<Vec<NativeWindow>, ProviderError> {
        use cua_spacesd_provider_api::{TargetProvider as _, TargetQuery};
        let targets = self.0.targets.enumerate(&TargetQuery {
            on_screen_only: false,
        })?;
        Ok(targets
            .into_iter()
            .filter_map(|target| {
                let (pid, window_id) = self.0.native_of(&target.id)?;
                Some(NativeWindow {
                    handle: target.descriptor.window.clone(),
                    epoch: target.id.epoch,
                    id: target.id,
                    pid,
                    window_id,
                })
            })
            .collect())
    }
}

/// A window's current snapshot generation and the driver's `element_token`
/// for each element index it returned.
type A11yGeneration = (u64, HashMap<u64, String>);

pub struct ToolBackend {
    tools: Arc<dyn DriverTools>,
    catalog: Arc<dyn NativeCatalog>,
    media: MediaProviders,
    registry: Option<Arc<cua_driver_core::tool::ToolRegistry>>,
    platform: &'static str,
    /// Per window: the current snapshot generation and the driver's
    /// `element_token` per element (the only element target it accepts).
    a11y_generation: Mutex<HashMap<(i64, u64), A11yGeneration>>,
    next_generation: AtomicU64,
    /// A text clipboard the platform provides next to the driver tools
    /// (Hyprland: wl-clipboard). None: clipboard calls are unsupported.
    clipboard: Option<Arc<dyn TextClipboard>>,
    /// Platform window-manager operations the driver tools do not offer
    /// (Hyprland: focused window, move/resize through its IPC).
    window_manager: Option<Arc<dyn WindowManager>>,
}

/// Window-manager facts and geometry for platforms where the driver has no
/// such tools (a Wayland compositor's own IPC). Never input.
pub trait WindowManager: Send + Sync {
    /// The native window id of the focused window.
    fn focused_window_id(&self) -> Option<u64>;
    /// Run a window action through the compositor; None when it has none
    /// for `action` (the driver tools are tried next).
    fn window_action(&self, window_id: u64, action: &WindowAction) -> Option<Result<(), String>>;
    /// Move and resize a window (logical points).
    fn set_bounds(
        &self,
        window_id: u64,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), String>;
}

/// Plain-text clipboard access for [`ToolBackend`].
pub trait TextClipboard: Send + Sync {
    /// The current text, or None when the clipboard holds no text.
    fn get_text(&self) -> Result<Option<String>, String>;
    fn set_text(&self, text: &str) -> Result<(), String>;
}

fn unsupported(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCode::Unsupported, message)
}

fn failed(message: impl Into<String>) -> ProviderError {
    ProviderError::new(ProviderErrorCode::DeliveryFailed, message)
}

fn gone() -> ProviderError {
    ProviderError::new(ProviderErrorCode::TargetUnavailable, "window is gone")
}

fn number(value: &Value, key: &str) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(0.0)
}

/// BGRA sink that keeps the first frame.
struct FirstFrame(Mutex<Option<mpsc::SyncSender<OwnedFrame>>>);

impl CaptureSink for FirstFrame {
    fn on_event(&self, event: CaptureEvent) {
        if let CaptureEvent::Frame(frame) = event {
            if frame.format == PixelFormat::Bgra8 {
                if let Some(sender) = self.0.lock().unwrap().take() {
                    let _ = sender.try_send(frame);
                }
            }
        }
    }
}

/// Whether the keyboard target was looked up as the focused window.
fn focused_fallback(window: &Option<&WindowRecord>, focused: &Option<WindowRecord>) -> bool {
    window.is_some() && focused.is_some()
}

/// Tightly packed copy of a (possibly strided) BGRA frame.
fn packed_bgra(frame: &OwnedFrame) -> Vec<u8> {
    let row = frame.width_px as usize * 4;
    let stride = frame
        .bytes_per_row
        .map(|stride| stride as usize)
        .unwrap_or(row)
        .max(row);
    if stride == row {
        return frame.bytes[..(row * frame.height_px as usize).min(frame.bytes.len())].to_vec();
    }
    let mut packed = Vec::with_capacity(row * frame.height_px as usize);
    for line in frame.bytes.chunks(stride).take(frame.height_px as usize) {
        packed.extend_from_slice(&line[..row.min(line.len())]);
    }
    packed
}

impl ToolBackend {
    pub fn new(
        tools: Arc<dyn DriverTools>,
        catalog: Arc<dyn NativeCatalog>,
        media: MediaProviders,
        registry: Option<Arc<cua_driver_core::tool::ToolRegistry>>,
        platform: &'static str,
    ) -> Self {
        Self {
            tools,
            catalog,
            media,
            registry,
            platform,
            a11y_generation: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
            clipboard: None,
            window_manager: None,
        }
    }

    /// The native id of the focused window: the window manager's answer, or
    /// on Windows the foreground HWND (the driver's `window_id` is the HWND,
    /// and its `list_windows` reports no focus). None when neither knows,
    /// and the driver's per-window `focused` fact is used instead.
    fn foreground_window_id(&self) -> Option<u64> {
        if let Some(wm) = &self.window_manager {
            return wm.focused_window_id();
        }
        #[cfg(target_os = "windows")]
        if self.platform == "Windows" {
            // SAFETY: GetForegroundWindow takes no arguments and only reads
            // the calling desktop's foreground window.
            let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
            return (!hwnd.0.is_null()).then_some(hwnd.0 as usize as u64);
        }
        None
    }

    /// Use `window_manager` for the focused window and window geometry.
    pub fn with_window_manager(mut self, window_manager: Arc<dyn WindowManager>) -> Self {
        self.window_manager = Some(window_manager);
        self
    }

    /// Serve text clipboard calls through `clipboard`.
    pub fn with_text_clipboard(mut self, clipboard: Arc<dyn TextClipboard>) -> Self {
        self.clipboard = Some(clipboard);
        self
    }

    /// The host backend: the provider bundle plus the codec audio provider.
    pub fn for_bundle(
        bundle: crate::CuaProviderBundle,
        audio: Option<Arc<dyn cua_spacesd_session::media::audio::AudioProvider>>,
        platform: &'static str,
    ) -> Self {
        let media = MediaProviders {
            targets: bundle.targets.clone(),
            displays: bundle.displays.clone(),
            captures: bundle.captures.clone(),
            actions: bundle.actions.clone(),
            accessibility: bundle.accessibility.clone(),
            geometry: bundle.geometry.clone(),
            inputs: bundle.inputs.clone(),
            audio,
        };
        let registry = bundle.registry.clone();
        Self::new(
            Arc::new(RegistryTools(registry.clone())),
            Arc::new(BundleCatalog(Arc::new(bundle))),
            media,
            Some(registry),
            platform,
        )
    }

    fn native(&self, window: &WindowRecord) -> Result<NativeWindow, ProviderError> {
        self.catalog
            .windows()?
            .into_iter()
            .find(|native| native.handle == window.handle)
            .ok_or_else(gone)
            .and_then(|native| {
                if native.epoch == window.epoch {
                    Ok(native)
                } else {
                    Err(ProviderError::new(
                        ProviderErrorCode::StaleTarget,
                        "the window was replaced",
                    ))
                }
            })
    }

    fn invoke(&self, name: &str, arguments: Value) -> Result<Value, ProviderError> {
        if !self.tools.has(name) {
            return Err(unsupported(format!(
                "the {} driver has no {name} tool",
                self.platform
            )));
        }
        self.tools.invoke(name, arguments).map_err(failed)
    }

    fn scale_at(&self, point: (f64, f64)) -> f64 {
        let displays = self.displays().unwrap_or_default();
        displays
            .iter()
            .find(|display| {
                let (x, y, w, h) = display.bounds;
                point.0 >= x && point.1 >= y && point.0 < x + w && point.1 < y + h
            })
            .or_else(|| displays.iter().find(|display| display.primary))
            .map(|display| display.scale_factor)
            .filter(|scale| *scale > 0.0)
            .unwrap_or(1.0)
    }

    /// Tool arguments that address `point` (global logical points): window
    /// screenshot pixels for a window target, desktop pixels otherwise.
    fn address(
        &self,
        window: Option<(&WindowRecord, &NativeWindow)>,
        point: (f64, f64),
    ) -> serde_json::Map<String, Value> {
        let scale = self.scale_at(point);
        let mut arguments = serde_json::Map::new();
        match window {
            Some((record, native)) => {
                arguments.insert("pid".into(), native.pid.into());
                arguments.insert("window_id".into(), native.window_id.into());
                arguments.insert("x".into(), ((point.0 - record.bounds.0) * scale).into());
                arguments.insert("y".into(), ((point.1 - record.bounds.1) * scale).into());
            }
            None => {
                arguments.insert("scope".into(), "desktop".into());
                arguments.insert("x".into(), (point.0 * scale).into());
                arguments.insert("y".into(), (point.1 * scale).into());
            }
        }
        arguments
    }

    fn grab(&self, target: &ProviderTargetId) -> Result<OwnedFrame, ProviderError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        let lease = self.media.captures.start(
            target,
            &CaptureConfig {
                max_fps: 30,
                max_dimension: 0,
                target_bitrate_kbps: None,
                accepted_formats: vec![PixelFormat::Bgra8],
            },
            Arc::new(FirstFrame(Mutex::new(Some(sender)))),
        )?;
        lease.request_keyframe();
        // Generous: the Hyprland grabber shells out to hyprctl and grim, which
        // can take seconds on a loaded or emulated guest.
        let frame = receiver.recv_timeout(Duration::from_secs(15));
        lease.stop();
        frame.map_err(|_| {
            ProviderError::new(ProviderErrorCode::DeliveryFailed, "no frame within 15 s")
        })
    }
}

fn delivery_mode(delivery: DeliveryRequest) -> Option<&'static str> {
    match delivery {
        DeliveryRequest::Auto => None,
        DeliveryRequest::Background => Some("background"),
        DeliveryRequest::Foreground => Some("foreground"),
    }
}

fn delivery_result(delivery: DeliveryRequest, has_window: bool, detail: &Value) -> DeliveryResult {
    let used = match delivery {
        DeliveryRequest::Foreground => DeliveryUsed::Foreground,
        DeliveryRequest::Background => DeliveryUsed::Background,
        DeliveryRequest::Auto if has_window => DeliveryUsed::Background,
        DeliveryRequest::Auto => DeliveryUsed::Foreground,
    };
    DeliveryResult {
        delivery: used,
        focus_changed: used == DeliveryUsed::Foreground,
        pointer_moved: !has_window,
        detail: detail
            .get("summary")
            .or_else(|| detail.get("message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
    }
}

impl DesktopBackend for ToolBackend {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError> {
        self.media.displays.displays()
    }

    fn windows(&self) -> Result<Vec<WindowRecord>, ProviderError> {
        let catalog = self.catalog.windows()?;
        let listed = self.invoke("list_windows", json!({}))?;
        let facts: HashMap<u64, &Value> = listed
            .get("windows")
            .and_then(Value::as_array)
            .map(|windows| {
                windows
                    .iter()
                    .filter_map(|window| Some((window.get("window_id")?.as_u64()?, window)))
                    .collect()
            })
            .unwrap_or_default();
        let displays = self.displays().unwrap_or_default();
        let active = self.foreground_window_id();
        let mut records = Vec::new();
        for native in catalog {
            let Some(fact) = facts.get(&native.window_id) else {
                continue;
            };
            let bounds = fact.get("bounds").cloned().unwrap_or(Value::Null);
            let bounds = (
                number(&bounds, "x"),
                number(&bounds, "y"),
                number(&bounds, "width"),
                number(&bounds, "height"),
            );
            let center = (bounds.0 + bounds.2 / 2.0, bounds.1 + bounds.3 / 2.0);
            let display_id = displays
                .iter()
                .find(|display| {
                    let (x, y, w, h) = display.bounds;
                    center.0 >= x && center.1 >= y && center.0 < x + w && center.1 < y + h
                })
                .or(displays.first())
                .map(|display| display.id.clone())
                .unwrap_or_default();
            let on_screen = fact
                .get("is_on_screen")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let minimized = fact
                .get("minimized")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let layer = fact.get("layer").and_then(Value::as_i64).unwrap_or(0);
            let text = |key: &str| {
                fact.get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            records.push(WindowRecord {
                handle: native.handle.clone(),
                epoch: native.epoch,
                title: text("title"),
                app_name: text("app_name"),
                app_id: text("bundle_id"),
                pid: u32::try_from(native.pid).unwrap_or(0),
                bounds,
                display_id,
                state: if minimized {
                    WindowStateKind::Minimized
                } else if on_screen {
                    WindowStateKind::Normal
                } else {
                    WindowStateKind::Hidden
                },
                kind: if layer != 0 {
                    WindowKind::System
                } else if bounds.2 < 2.0 || bounds.3 < 2.0 {
                    WindowKind::Phantom
                } else {
                    WindowKind::Standard
                },
                focused: match active {
                    Some(id) => id == native.window_id,
                    None => fact
                        .get("focused")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                },
                on_screen,
                z_order: fact
                    .get("z_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::from(u32::MAX)) as u32,
            });
        }
        records.sort_by_key(|record| record.z_order);
        Ok(records)
    }

    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, ProviderError> {
        let displays = self.displays()?;
        let display = displays
            .iter()
            .find(|display| {
                display.id == display_id || (display_id == "primary" && display.primary)
            })
            .ok_or_else(|| {
                ProviderError::new(ProviderErrorCode::TargetUnavailable, "unknown display")
            })?;
        let target = self.media.displays.display_target(&display.id)?;
        let frame = self.grab(&target.id)?;
        Ok(CapturedImage {
            bgra: packed_bgra(&frame),
            width: frame.width_px,
            height: frame.height_px,
            logical_bounds: display.bounds,
            display_id: display.id.clone(),
        })
    }

    fn capture_window(&self, window: &WindowRecord) -> Result<CapturedImage, ProviderError> {
        let native = self.native(window)?;
        let frame = self.grab(&native.id)?;
        Ok(CapturedImage {
            bgra: packed_bgra(&frame),
            width: frame.width_px,
            height: frame.height_px,
            logical_bounds: window.bounds,
            display_id: window.display_id.clone(),
        })
    }

    fn pointer(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        point: (f64, f64),
        action: &PointerAction,
    ) -> Result<DeliveryResult, ProviderError> {
        let native = window.map(|window| self.native(window)).transpose()?;
        let pair = window.zip(native.as_ref());
        let mut arguments = self.address(pair, point);
        if let Some(mode) = delivery_mode(delivery) {
            if native.is_some() {
                arguments.insert("delivery_mode".into(), mode.into());
            }
        }
        let tool = match action {
            PointerAction::Click { button, count, modifiers } => {
                arguments.insert("button".into(), button.as_str().into());
                arguments.insert("count".into(), (*count).max(1).into());
                if !modifiers.is_empty() {
                    arguments.insert("modifier".into(), json!(modifiers));
                }
                "click"
            }
            PointerAction::Move => {
                arguments.remove("pid");
                arguments.remove("window_id");
                arguments.remove("delivery_mode");
                let scale = self.scale_at(point);
                arguments.insert("scope".into(), "desktop".into());
                arguments.insert("x".into(), (point.0 * scale).into());
                arguments.insert("y".into(), (point.1 * scale).into());
                "move_cursor"
            }
            PointerAction::Drag { path, button, modifiers } => {
                let end = *path.last().ok_or_else(|| unsupported("a drag needs at least one further point"))?;
                let to = self.address(pair, end);
                let from_x = arguments.remove("x").unwrap_or(Value::Null);
                let from_y = arguments.remove("y").unwrap_or(Value::Null);
                arguments.insert("from_x".into(), from_x);
                arguments.insert("from_y".into(), from_y);
                arguments.insert("to_x".into(), to["x"].clone());
                arguments.insert("to_y".into(), to["y"].clone());
                arguments.insert("button".into(), button.as_str().into());
                if !modifiers.is_empty() {
                    arguments.insert("modifier".into(), json!(modifiers));
                }
                "drag"
            }
            PointerAction::Scroll { dx, dy } => {
                let (direction, amount) = if dy.abs() >= dx.abs() {
                    (if *dy >= 0.0 { "down" } else { "up" }, dy.abs())
                } else {
                    (if *dx >= 0.0 { "right" } else { "left" }, dx.abs())
                };
                arguments.insert("direction".into(), direction.into());
                arguments.insert("amount".into(), (amount.round().clamp(1.0, 50.0) as u64).into());
                "scroll"
            }
            PointerAction::Down { .. } | PointerAction::Up { .. } => {
                return Err(unsupported(format!(
                    "separate button down/up is not available through the {} driver tools; use click or drag",
                    self.platform
                )))
            }
        };
        let detail = self.invoke(tool, Value::Object(arguments))?;
        Ok(delivery_result(delivery, native.is_some(), &detail))
    }

    fn keyboard(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        action: &KeyAction,
    ) -> Result<DeliveryResult, ProviderError> {
        // Linux driver keyboard tools need a target window; without one the
        // keys go to the focused window, as on the other platforms.
        let mut focused = None;
        let window = match window {
            None if self.platform.starts_with("Linux") => {
                let active = self
                    .window_manager
                    .as_ref()
                    .and_then(|wm| wm.focused_window_id());
                let records = self.windows()?;
                let natives = self.catalog.windows()?;
                focused = records.into_iter().find(|w| {
                    w.focused
                        || active.is_some_and(|id| {
                            natives
                                .iter()
                                .any(|n| n.handle == w.handle && n.window_id == id)
                        })
                });
                focused.as_ref()
            }
            window => window,
        };
        // Keys for the window that already has focus go the foreground way;
        // the driver never falls back from a refused background route itself.
        let delivery = match delivery {
            DeliveryRequest::Auto if focused_fallback(&window, &focused) => {
                DeliveryRequest::Foreground
            }
            other => other,
        };
        let native = window.map(|window| self.native(window)).transpose()?;
        let mut arguments = serde_json::Map::new();
        if let Some(native) = &native {
            arguments.insert("pid".into(), native.pid.into());
            arguments.insert("window_id".into(), native.window_id.into());
            if let Some(mode) = delivery_mode(delivery) {
                arguments.insert("delivery_mode".into(), mode.into());
            }
        } else {
            // No window could be resolved as focused (Linux: the window
            // manager did not answer; Windows and macOS: the driver
            // otherwise requires a pid, and a window-less call failed with
            // "Missing required integer field: pid"): the driver's desktop
            // scope types into whatever has keyboard focus, instead of a
            // window call with no window.
            arguments.insert("scope".into(), "desktop".into());
        }
        let (tool, repeat) = match action {
            KeyAction::Type { text, .. } => {
                arguments.insert("text".into(), text.as_str().into());
                ("type_text", 1)
            }
            KeyAction::Press { key, modifiers, repeat } => {
                arguments.insert("key".into(), key.as_str().into());
                if !modifiers.is_empty() {
                    arguments.insert("modifiers".into(), json!(modifiers));
                }
                ("press_key", (*repeat).clamp(1, 100))
            }
            KeyAction::Hotkey { keys } => {
                arguments.insert("keys".into(), json!(keys));
                ("hotkey", 1)
            }
            KeyAction::Down { .. } | KeyAction::Up { .. } => {
                return Err(unsupported(format!(
                    "separate key down/up is not available through the {} driver tools; use press or hotkey",
                    self.platform
                )))
            }
        };
        let mut detail = Value::Null;
        for _ in 0..repeat {
            detail = self.invoke(tool, Value::Object(arguments.clone()))?;
        }
        let mut result = delivery_result(delivery, native.is_some(), &detail);
        result.pointer_moved = false;
        Ok(result)
    }

    fn cursor_position(&self) -> Result<(f64, f64), ProviderError> {
        let position = self.invoke("get_cursor_position", json!({}))?;
        let scale = self.scale_at((0.0, 0.0));
        Ok((
            number(&position, "x") / scale,
            number(&position, "y") / scale,
        ))
    }

    fn clipboard_get(&self) -> Result<ClipboardData, ProviderError> {
        let Some(clipboard) = &self.clipboard else {
            return Err(unsupported(format!(
                "the {} backend does not expose the clipboard yet",
                self.platform
            )));
        };
        Ok(ClipboardData {
            text: clipboard.get_text().map_err(failed)?,
            ..ClipboardData::default()
        })
    }

    fn clipboard_set(&self, data: ClipboardData) -> Result<(), ProviderError> {
        let Some(clipboard) = &self.clipboard else {
            return Err(unsupported(format!(
                "the {} backend does not expose the clipboard yet",
                self.platform
            )));
        };
        if !data.files.is_empty() || data.image_png.is_some() {
            return Err(unsupported(format!(
                "the {} backend exposes text clipboard only",
                self.platform
            )));
        }
        clipboard
            .set_text(data.text.as_deref().unwrap_or_default())
            .map_err(failed)
    }

    fn window_action(
        &self,
        window: &WindowRecord,
        action: WindowAction,
    ) -> Result<(), ProviderError> {
        let native = self.native(window)?;
        if let Some(wm) = &self.window_manager {
            if let Some(result) = wm.window_action(native.window_id, &action) {
                return result.map_err(failed);
            }
        }
        match action {
            WindowAction::Activate | WindowAction::Restore => self
                .invoke(
                    "bring_to_front",
                    json!({ "pid": native.pid, "window_id": native.window_id }),
                )
                .map(|_| ()),
            WindowAction::Close { force: true } => self
                .invoke("kill_app", json!({ "pid": native.pid }))
                .map(|_| ()),
            other => Err(unsupported(format!(
                "{other:?} is not available through the {} driver tools",
                self.platform
            ))),
        }
    }

    fn set_window_bounds(
        &self,
        window: &WindowRecord,
        position: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
    ) -> Result<(), ProviderError> {
        let native = self.native(window)?;
        let (x, y) = position.unwrap_or((window.bounds.0, window.bounds.1));
        let (width, height) = size.unwrap_or((window.bounds.2, window.bounds.3));
        if let Some(wm) = &self.window_manager {
            return wm
                .set_bounds(native.window_id, x, y, width.max(1.0), height.max(1.0))
                .map_err(failed);
        }
        self.invoke(
            "set_window_frame",
            json!({
                "pid": native.pid,
                "window_id": native.window_id,
                "x": x,
                "y": y,
                "width": width.max(1.0),
                "height": height.max(1.0),
            }),
        )
        .map(|_| ())
    }

    fn launch(&self, request: &LaunchRequest) -> Result<u32, ProviderError> {
        let mut arguments = json!({ "additional_arguments": request.args });
        match &request.app {
            AppSpecData::AppId(id) => arguments["bundle_id"] = id.as_str().into(),
            AppSpecData::Name(name) => arguments["name"] = name.as_str().into(),
            // The Linux driver takes the command as `launch_path`.
            AppSpecData::Executable(path) if self.platform.starts_with("Linux") => {
                arguments["launch_path"] = path.as_str().into()
            }
            AppSpecData::Executable(path) => arguments["path"] = path.as_str().into(),
        }
        let launched = self.invoke("launch_app", arguments)?;
        Ok(launched.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32)
    }

    fn open(
        &self,
        target: &OpenTarget,
        with: Option<&AppSpecData>,
        background: bool,
    ) -> Result<u32, ProviderError> {
        let argument = match target {
            OpenTarget::Url(url) => url.clone(),
            OpenTarget::Path(path) => path.clone(),
        };
        let Some(app) = with else {
            return Err(unsupported(format!(
                "open without an application is not available through the {} driver tools; pass the app to open it with",
                self.platform
            )));
        };
        self.launch(&LaunchRequest {
            app: app.clone(),
            args: vec![argument],
            env: Vec::new(),
            cwd: None,
            background,
        })
    }

    fn a11y_tree(
        &self,
        window: &WindowRecord,
        max_depth: u32,
        max_nodes: u32,
    ) -> Result<A11yTree, ProviderError> {
        let native = self.native(window)?;
        let mut arguments = json!({
            "pid": native.pid,
            "window_id": native.window_id,
            "include_screenshot": false,
        });
        if max_nodes > 0 {
            arguments["max_elements"] = max_nodes.into();
        }
        if max_depth > 0 {
            arguments["max_depth"] = max_depth.into();
        }
        let state = self.invoke("get_window_state", arguments)?;
        let elements = state
            .get("elements")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let tokens = elements
            .iter()
            .filter_map(|element| {
                Some((
                    element.get("element_index")?.as_u64()?,
                    element.get("element_token")?.as_str()?.to_owned(),
                ))
            })
            .collect();
        self.a11y_generation
            .lock()
            .unwrap()
            .insert((native.pid, native.window_id), (generation, tokens));
        let nodes: Vec<A11yNode> = elements
            .iter()
            .enumerate()
            .map(|(position, element)| {
                let text = |key: &str| {
                    element
                        .get(key)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                let native_role = text("role");
                let mut states = Vec::new();
                if element.get("enabled").and_then(Value::as_bool) == Some(true) {
                    states.push("enabled".into());
                }
                if element.get("selected").and_then(Value::as_bool) == Some(true) {
                    states.push("selected".into());
                }
                A11yNode {
                    element_id: element
                        .get("element_index")
                        .and_then(Value::as_u64)
                        .map(|index| index.to_string())
                        .unwrap_or_else(|| format!("n{position}")),
                    parent_id: element
                        .get("parent_index")
                        .and_then(Value::as_u64)
                        .map(|index| index.to_string())
                        .unwrap_or_default(),
                    depth: element.get("depth").and_then(Value::as_u64).unwrap_or(0) as u32,
                    role: normalized_role(&native_role),
                    native_role,
                    name: text("label"),
                    value: text("value"),
                    description: text("value_description"),
                    bounds: element.get("frame").map(|frame| {
                        (
                            number(frame, "x"),
                            number(frame, "y"),
                            number(frame, "w"),
                            number(frame, "h"),
                        )
                    }),
                    states,
                    actions: Vec::new(),
                }
            })
            .collect();
        let truncated = max_nodes > 0 && nodes.len() >= max_nodes as usize;
        Ok(A11yTree {
            snapshot: generation,
            nodes,
            truncated,
        })
    }

    fn a11y_act(
        &self,
        window: &WindowRecord,
        snapshot: u64,
        element_id: &str,
        action: A11yActionKind,
        value: &str,
    ) -> Result<(), ProviderError> {
        let native = self.native(window)?;
        let current = self
            .a11y_generation
            .lock()
            .unwrap()
            .get(&(native.pid, native.window_id))
            .cloned();
        let tokens = match current {
            Some((generation, tokens)) if generation == snapshot => tokens,
            _ => {
                return Err(ProviderError::new(
                    ProviderErrorCode::StaleTarget,
                    "accessibility snapshot is no longer current",
                ))
            }
        };
        let token = element_id
            .parse::<u64>()
            .ok()
            .and_then(|index| tokens.get(&index))
            .ok_or_else(|| unsupported("this element has no actionable element_token"))?;
        let base = json!({ "pid": native.pid, "element_token": token });
        let with = |extra: Value| {
            let mut arguments = base.clone();
            if let (Value::Object(target), Value::Object(extra)) = (&mut arguments, extra) {
                target.extend(extra);
            }
            arguments
        };
        match action {
            A11yActionKind::Press
            | A11yActionKind::Select
            | A11yActionKind::Expand
            | A11yActionKind::Collapse => self.invoke("click", base.clone()).map(|_| ()),
            A11yActionKind::ShowMenu => self
                .invoke("click", with(json!({ "action": "show_menu" })))
                .map(|_| ()),
            A11yActionKind::SetValue => self
                .invoke("set_value", with(json!({ "value": value })))
                .map(|_| ()),
            A11yActionKind::ScrollIntoView => self
                .invoke("scroll", with(json!({ "direction": "down", "amount": 1 })))
                .map(|_| ()),
            _ => Err(unsupported(format!(
                "this accessibility action is not available through the {} driver tools",
                self.platform
            ))),
        }
    }

    fn media_providers(&self) -> MediaProviders {
        self.media.clone()
    }

    fn tool_registry(&self) -> Option<Arc<cua_driver_core::tool::ToolRegistry>> {
        self.registry.clone()
    }

    fn features(&self) -> Vec<FeatureStatus> {
        let tool = |name: &'static str, tools: &[&str], why: &str| {
            if tools.iter().all(|tool| self.tools.has(tool)) {
                FeatureStatus::yes(name)
            } else {
                FeatureStatus::no(name, why)
            }
        };
        let displays = self
            .displays()
            .map(|displays| !displays.is_empty())
            .unwrap_or(false);
        let audio = self.media.audio.is_some();
        let no_audio = "no audio capture backend is available on this host";
        let encoders = cua_media_codec::probe();
        let hardware = encoders.iter().any(|encoder| {
            encoder.status == cua_media_codec::ProbeStatus::Available && encoder.hardware
        });
        vec![
            if displays {
                FeatureStatus::yes("desktop_stream")
            } else {
                FeatureStatus::no(
                    "desktop_stream",
                    "no display is visible to the capture provider",
                )
            },
            FeatureStatus::yes("window_stream"),
            FeatureStatus::yes("h264_sw"),
            if hardware {
                FeatureStatus::yes("h264_hw")
            } else {
                FeatureStatus::no("h264_hw", "no hardware H.264 encoder was found")
            },
            FeatureStatus {
                limitation:
                    "pointer/key down-up pairs are not available; use click, drag, press or hotkey"
                        .into(),
                ..tool(
                    "background_input",
                    &["click", "type_text"],
                    "the driver has no background input tools",
                )
            },
            tool(
                "a11y",
                &["get_window_state"],
                "the driver has no accessibility tool",
            ),
            tool(
                "windows",
                &["list_windows"],
                "the driver cannot list windows",
            ),
            tool(
                "launch_app",
                &["launch_app"],
                "the driver cannot launch apps",
            ),
            if self.clipboard.is_some() {
                FeatureStatus::yes("clipboard.text")
            } else {
                FeatureStatus::no(
                    "clipboard.text",
                    format!(
                        "the {} backend does not expose the clipboard yet",
                        self.platform
                    ),
                )
            },
            FeatureStatus::no(
                "clipboard.files",
                format!(
                    "the {} backend does not expose the clipboard yet",
                    self.platform
                ),
            ),
            FeatureStatus::no(
                "clipboard.image",
                format!(
                    "the {} backend does not expose the clipboard yet",
                    self.platform
                ),
            ),
            if audio {
                FeatureStatus::yes("audio.desktop")
            } else {
                FeatureStatus::no("audio.desktop", no_audio)
            },
            FeatureStatus::no(
                "audio.per_app",
                "per-app audio capture is implemented for PulseAudio/PipeWire only",
            ),
            FeatureStatus::no(
                "audio.uplink",
                "audio uplink playback is implemented for PulseAudio/PipeWire only",
            ),
            if audio {
                FeatureStatus::yes("audio.opus")
            } else {
                FeatureStatus::no("audio.opus", no_audio)
            },
        ]
    }
}

fn normalized_role(native: &str) -> String {
    let trimmed = native.strip_prefix("AX").unwrap_or(native);
    match trimmed.to_ascii_lowercase().as_str() {
        "button" | "pushbutton" | "push button" => "button".into(),
        "textfield" | "textarea" | "edit" | "text field" => "text_field".into(),
        "window" | "dialog" | "sheet" => "window".into(),
        "checkbox" | "check box" => "checkbox".into(),
        "radiobutton" | "radio button" => "radio_button".into(),
        "menuitem" | "menu item" => "menu_item".into(),
        "combobox" | "popupbutton" | "combo box" => "combo_box".into(),
        "statictext" | "text" => "static_text".into(),
        "row" | "listitem" | "list item" => "list_item".into(),
        "tab" | "tabitem" | "tab item" => "tab".into(),
        "link" | "hyperlink" => "link".into(),
        other => other.replace(' ', "_"),
    }
}

#[cfg(test)]
mod registry_tools_tests {
    use super::{DriverTools, RegistryTools};
    use std::sync::Arc;

    fn tools() -> RegistryTools {
        RegistryTools(Arc::new(cua_driver_core::tool::ToolRegistry::new()))
    }

    // The gRPC services (window streams, SetWindowBounds, GetTree) call the
    // synchronous tool bridge from runtime workers. A plain `block_on` there
    // panics with "Cannot start a runtime from within a runtime", which took
    // down the macOS image's window streams.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn invoke_inside_a_multi_thread_runtime() {
        assert!(tools()
            .invoke("no_such_tool", serde_json::json!({}))
            .is_err());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn invoke_inside_a_current_thread_runtime() {
        assert!(tools()
            .invoke("no_such_tool", serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn invoke_outside_a_runtime() {
        assert!(tools()
            .invoke("no_such_tool", serde_json::json!({}))
            .is_err());
    }

    /// Records the arguments the driver's dispatch hands the tool.
    struct Probe(Arc<std::sync::Mutex<Option<serde_json::Value>>>);

    #[async_trait::async_trait]
    impl cua_driver_core::tool::Tool for Probe {
        fn def(&self) -> &cua_driver_core::tool::ToolDef {
            static DEF: std::sync::OnceLock<cua_driver_core::tool::ToolDef> =
                std::sync::OnceLock::new();
            DEF.get_or_init(|| cua_driver_core::tool::ToolDef {
                name: "click".into(),
                description: "probe".into(),
                input_schema: serde_json::json!({ "type": "object" }),
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: false,
            })
        }

        async fn invoke(&self, args: serde_json::Value) -> cua_driver_core::protocol::ToolResult {
            self.0.lock().unwrap().replace(args);
            cua_driver_core::protocol::ToolResult::text("ok")
        }
    }

    // Window pointer input carries pixels this backend computed from window
    // bounds, with no driver screenshot read; the driver must take them as
    // native window pixels instead of refusing for a missing screenshot.
    #[test]
    fn window_pixels_reach_the_driver_as_native_pixels() {
        let received = Arc::new(std::sync::Mutex::new(None));
        let mut registry = cua_driver_core::tool::ToolRegistry::new();
        registry.register(Box::new(Probe(received.clone())));
        RegistryTools(Arc::new(registry))
            .invoke(
                "click",
                serde_json::json!({ "pid": 7, "window_id": 11, "x": 20.0, "y": 30.0 }),
            )
            .unwrap();
        let received = received.lock().unwrap().clone().expect("click ran");
        assert_eq!(
            received[cua_driver_core::snapshot_store::NATIVE_WINDOW_PIXELS_ARG],
            serde_json::json!(true)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Records tool calls and answers from canned results.
    #[derive(Default)]
    pub(crate) struct FakeTools {
        pub calls: Mutex<Vec<(String, Value)>>,
    }

    impl DriverTools for FakeTools {
        fn invoke(&self, name: &str, arguments: Value) -> Result<Value, String> {
            self.calls
                .lock()
                .unwrap()
                .push((name.to_owned(), arguments.clone()));
            Ok(match name {
                "list_windows" => json!({ "windows": [
                    { "window_id": 11, "pid": 7, "app_name": "Fixture", "title": "Main", "bounds": { "x": 100, "y": 50, "width": 400, "height": 300 }, "layer": 0, "z_index": 0, "is_on_screen": true },
                    { "window_id": 12, "pid": 7, "app_name": "Fixture", "title": "Menu bar", "bounds": { "x": 0, "y": 0, "width": 1280, "height": 24 }, "layer": 25, "z_index": 1, "is_on_screen": true },
                    { "window_id": 99, "pid": 8, "app_name": "Uncatalogued", "title": "x", "bounds": { "x": 0, "y": 0, "width": 10, "height": 10 }, "layer": 0, "z_index": 2, "is_on_screen": true }
                ]}),
                "get_window_state" => json!({ "snapshot_id": "snap-1", "elements": [
                    { "element_index": 0, "element_token": "s1:0", "role": "AXButton", "label": "OK", "depth": 1, "enabled": true, "frame": { "x": 110, "y": 60, "w": 50, "h": 20 } },
                    { "element_index": 1, "element_token": "s1:1", "role": "AXTextField", "label": "Name", "value": "abc", "depth": 1, "parent_index": 0 },
                    { "role": "AXStaticText", "label": "Observed only", "depth": 1 }
                ]}),
                "get_cursor_position" => json!({ "x": 200, "y": 100 }),
                _ => json!({ "summary": format!("{name} ok") }),
            })
        }

        fn has(&self, name: &str) -> bool {
            name != "set_window_frame"
        }
    }

    struct FakeCatalog;

    impl NativeCatalog for FakeCatalog {
        fn windows(&self) -> Result<Vec<NativeWindow>, ProviderError> {
            Ok([(11u64, "w11"), (12, "w12")]
                .into_iter()
                .map(|(window_id, handle)| NativeWindow {
                    handle: TargetHandle(handle.into()),
                    epoch: TargetEpoch(1),
                    id: ProviderTargetId {
                        key: cua_spacesd_provider_api::BackendTargetKey::new(format!(
                            "fake:{handle}"
                        )),
                        epoch: TargetEpoch(1),
                    },
                    pid: 7,
                    window_id,
                })
                .collect())
        }
    }

    fn backend() -> (ToolBackend, Arc<FakeTools>) {
        let tools = Arc::new(FakeTools::default());
        let media = super::super::tests::fake_media_providers(2.0);
        (
            ToolBackend::new(tools.clone(), Arc::new(FakeCatalog), media, None, "test"),
            tools,
        )
    }

    fn last_call(tools: &FakeTools) -> (String, Value) {
        tools.calls.lock().unwrap().last().cloned().unwrap()
    }

    #[test]
    fn windows_join_the_catalog_with_driver_facts() {
        let (backend, _) = backend();
        let windows = backend.windows().unwrap();
        assert_eq!(windows.len(), 2, "uncatalogued windows are not reported");
        assert_eq!(windows[0].handle.0, "w11");
        assert_eq!(windows[0].bounds, (100.0, 50.0, 400.0, 300.0));
        assert_eq!(windows[0].kind, WindowKind::Standard);
        assert_eq!(windows[1].kind, WindowKind::System);
    }

    #[test]
    fn pointer_input_addresses_window_screenshot_pixels_and_desktop_pixels() {
        let (backend, tools) = backend();
        let window = backend.windows().unwrap().remove(0);
        backend
            .pointer(
                Some(&window),
                DeliveryRequest::Background,
                (150.0, 80.0),
                &PointerAction::Click {
                    button: "left".into(),
                    count: 2,
                    modifiers: vec!["shift".into()],
                },
            )
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "click");
        // (150-100, 80-50) logical points at scale 2.
        assert_eq!(arguments["x"], json!(100.0));
        assert_eq!(arguments["y"], json!(60.0));
        assert_eq!(arguments["window_id"], json!(11));
        assert_eq!(arguments["count"], json!(2));
        assert_eq!(arguments["modifier"], json!(["shift"]));
        assert_eq!(arguments["delivery_mode"], json!("background"));

        let result = backend
            .pointer(
                None,
                DeliveryRequest::Auto,
                (10.0, 20.0),
                &PointerAction::Scroll { dx: 0.0, dy: -3.0 },
            )
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "scroll");
        assert_eq!(arguments["scope"], json!("desktop"));
        assert_eq!(arguments["direction"], json!("up"));
        assert_eq!(arguments["amount"], json!(3));
        assert_eq!(result.delivery, DeliveryUsed::Foreground);

        backend
            .pointer(
                Some(&window),
                DeliveryRequest::Auto,
                (110.0, 60.0),
                &PointerAction::Drag {
                    path: vec![(120.0, 70.0), (130.0, 90.0)],
                    button: "left".into(),
                    modifiers: vec![],
                },
            )
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "drag");
        assert_eq!(
            (arguments["from_x"].clone(), arguments["to_y"].clone()),
            (json!(20.0), json!(80.0))
        );

        let error = backend
            .pointer(
                None,
                DeliveryRequest::Auto,
                (0.0, 0.0),
                &PointerAction::Down {
                    button: "left".into(),
                },
            )
            .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Unsupported);
    }

    #[test]
    fn keyboard_maps_to_driver_tools_and_repeats_presses() {
        let (backend, tools) = backend();
        let window = backend.windows().unwrap().remove(0);
        backend
            .keyboard(
                Some(&window),
                DeliveryRequest::Auto,
                &KeyAction::Type {
                    text: "hi".into(),
                    insert: false,
                },
            )
            .unwrap();
        assert_eq!(last_call(&tools).0, "type_text");
        let before = tools.calls.lock().unwrap().len();
        backend
            .keyboard(
                None,
                DeliveryRequest::Auto,
                &KeyAction::Press {
                    key: "tab".into(),
                    modifiers: vec!["shift".into()],
                    repeat: 3,
                },
            )
            .unwrap();
        let calls = tools.calls.lock().unwrap();
        assert_eq!(calls.len() - before, 3);
        assert_eq!(calls.last().unwrap().1["modifiers"], json!(["shift"]));
    }

    /// Linux keys without a target go to the focused window. When none can be
    /// resolved (the window manager did not answer), the call must use the
    /// driver's desktop scope, never a window call with no window ("No windows
    /// found for pid 0").
    #[test]
    fn linux_keyboard_without_a_focused_window_uses_desktop_scope() {
        let tools = Arc::new(FakeTools::default());
        let media = super::super::tests::fake_media_providers(2.0);
        let backend = ToolBackend::new(tools.clone(), Arc::new(FakeCatalog), media, None, "Linux");
        backend
            .keyboard(
                None,
                DeliveryRequest::Auto,
                &KeyAction::Type {
                    text: "doctor".into(),
                    insert: false,
                },
            )
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "type_text");
        assert_eq!(arguments["scope"], json!("desktop"));
        assert!(arguments.get("pid").is_none(), "{arguments}");
    }

    /// macOS keys and chords without a target (`cua do hotkey cmd+w`) go to
    /// the frontmost app through the desktop scope; the window-scoped tool
    /// refuses a call without a pid.
    #[test]
    fn macos_keyboard_without_a_window_uses_desktop_scope() {
        let tools = Arc::new(FakeTools::default());
        let media = super::super::tests::fake_media_providers(2.0);
        let backend = ToolBackend::new(tools.clone(), Arc::new(FakeCatalog), media, None, "macOS");
        backend
            .keyboard(
                None,
                DeliveryRequest::Auto,
                &KeyAction::Hotkey {
                    keys: vec!["cmd".into(), "w".into()],
                },
            )
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "hotkey");
        assert_eq!(arguments["scope"], json!("desktop"));
        assert!(arguments.get("pid").is_none(), "{arguments}");
    }

    #[test]
    fn accessibility_tree_and_act_use_snapshots() {
        let (backend, tools) = backend();
        let window = backend.windows().unwrap().remove(0);
        let tree = backend.a11y_tree(&window, 0, 0).unwrap();
        assert_eq!(tree.nodes.len(), 3);
        assert_eq!(tree.nodes[0].role, "button");
        assert_eq!(tree.nodes[1].role, "text_field");
        assert_eq!(tree.nodes[1].parent_id, "0");
        assert_eq!(tree.nodes[0].bounds, Some((110.0, 60.0, 50.0, 20.0)));
        backend
            .a11y_act(&window, tree.snapshot, "1", A11yActionKind::SetValue, "xyz")
            .unwrap();
        let (name, arguments) = last_call(&tools);
        assert_eq!(name, "set_value");
        // element_token is the driver's only element target; the token
        // carries the window, so no element_index, snapshot_id or window_id.
        assert_eq!(
            arguments,
            json!({ "pid": 7, "element_token": "s1:1", "value": "xyz" })
        );
        // A row the driver returned without a token is observation-only.
        let observed = backend
            .a11y_act(&window, tree.snapshot, "n2", A11yActionKind::Press, "")
            .unwrap_err();
        assert_eq!(observed.code, ProviderErrorCode::Unsupported);
        let stale = backend
            .a11y_act(&window, tree.snapshot + 100, "0", A11yActionKind::Press, "")
            .unwrap_err();
        assert_eq!(stale.code, ProviderErrorCode::StaleTarget);
    }

    #[test]
    fn screenshots_take_one_bgra_frame_and_missing_tools_are_unsupported() {
        let (backend, _) = backend();
        let image = backend.capture_display("primary").unwrap();
        assert_eq!((image.width, image.height), (64, 48));
        assert_eq!(image.bgra.len(), 64 * 48 * 4);
        let window = backend.windows().unwrap().remove(0);
        let error = backend
            .set_window_bounds(&window, None, Some((10.0, 10.0)))
            .unwrap_err();
        assert_eq!(error.code, ProviderErrorCode::Unsupported);
        assert_eq!(backend.cursor_position().unwrap(), (100.0, 50.0));
        assert!(backend
            .features()
            .iter()
            .any(|feature| feature.name == "a11y" && feature.supported));
    }

    #[test]
    fn strided_frames_are_packed() {
        let frame = OwnedFrame {
            bytes: Arc::from(vec![1u8; 2 * 12]),
            format: PixelFormat::Bgra8,
            width_px: 2,
            height_px: 2,
            bytes_per_row: Some(12),
            capture_timestamp_us: 0,
            encode_duration_us: None,
            codec_epoch: 1,
            keyframe: true,
        };
        assert_eq!(packed_bgra(&frame).len(), 16);
    }
}
