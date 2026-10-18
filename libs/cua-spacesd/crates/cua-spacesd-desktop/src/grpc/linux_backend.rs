// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Linux X11 implementation of [`DesktopBackend`].
//!
//! Window handles come from the shared `CuaProviderBundle` catalog, so a
//! handle returned by `WindowsService` streams through `StreamService` and
//! drives `AccessibilityService` without translation. Accessibility uses
//! cua-driver `platform-linux` AT-SPI; all pointer and keyboard input is
//! delivered by cua-driver's targeted input (`platform_linux::input::targeted`),
//! which owns the auto/background/foreground semantics; clipboard owns the X11
//! CLIPBOARD selection through clipboard-rs.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use cua_spacesd_provider_api::{
    ProviderDisplay, ProviderError, ProviderErrorCode, TargetProvider, TargetQuery,
};
use cua_spacesd_session::media::MediaProviders;

use super::backend::*;
use cua_driver_core::interactive_input::{
    DeliveryReport, DeliveryUsed as DriverDeliveryUsed, KeyOp, PointerOp, TargetedButton,
    TargetedDelivery,
};
use platform_linux::input::targeted::{self, KeyRequest, PointerRequest};

use crate::driver_input;
use crate::linux_stream::{self, StreamSource};
use crate::linux_x11::{self, X11WindowKind, X11WindowState};
use crate::CuaProviderBundle;

pub struct LinuxBackend {
    bundle: CuaProviderBundle,
    clipboard: Mutex<Option<clipboard_rs::ClipboardContext>>,
    a11y_generation: Mutex<HashMap<u32, u64>>,
    next_generation: AtomicU64,
    audio: Option<Arc<dyn cua_spacesd_session::media::audio::AudioProvider>>,
}

fn unsupported(message: &str) -> ProviderError {
    ProviderError::new(ProviderErrorCode::Unsupported, message)
}

fn failed(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(ProviderErrorCode::DeliveryFailed, error.to_string())
}

impl LinuxBackend {
    pub fn new() -> Result<Self, ProviderError> {
        Self::with_audio(crate::codec_audio::CodecAudioProvider::new().map(|audio| {
            Arc::new(audio) as Arc<dyn cua_spacesd_session::media::audio::AudioProvider>
        }))
    }

    /// Use an explicit audio provider (tests inject a synthetic source).
    pub fn with_audio(
        audio: Option<Arc<dyn cua_spacesd_session::media::audio::AudioProvider>>,
    ) -> Result<Self, ProviderError> {
        let bundle = CuaProviderBundle::new()?;
        Ok(Self {
            bundle,
            clipboard: Mutex::new(None),
            a11y_generation: Mutex::new(HashMap::new()),
            next_generation: AtomicU64::new(1),
            audio,
        })
    }

    /// Native (pid, xid) of a window record.
    fn native(&self, window: &WindowRecord) -> Result<(u32, u32), ProviderError> {
        let target = self.bundle.targets.resolve(&window.handle, window.epoch)?;
        let (pid, window_id) = self.bundle.native_of(&target.id).ok_or_else(|| {
            ProviderError::new(ProviderErrorCode::TargetUnavailable, "window is gone")
        })?;
        Ok((pid as u32, window_id as u32))
    }

    fn with_clipboard<T>(
        &self,
        work: impl FnOnce(&clipboard_rs::ClipboardContext) -> Result<T, String>,
    ) -> Result<T, ProviderError> {
        let mut guard = self.clipboard.lock().unwrap();
        if guard.is_none() {
            *guard = Some(clipboard_rs::ClipboardContext::new().map_err(|error| {
                ProviderError::new(
                    ProviderErrorCode::Unsupported,
                    format!("clipboard unavailable: {error}"),
                )
            })?);
        }
        work(guard.as_ref().expect("clipboard context")).map_err(failed)
    }
}

fn driver_delivery(requested: DeliveryRequest) -> TargetedDelivery {
    match requested {
        DeliveryRequest::Auto => TargetedDelivery::Auto,
        DeliveryRequest::Background => TargetedDelivery::Background,
        DeliveryRequest::Foreground => TargetedDelivery::Foreground,
    }
}

fn result(report: DeliveryReport) -> DeliveryResult {
    DeliveryResult {
        delivery: match report.delivery {
            DriverDeliveryUsed::Background => DeliveryUsed::Background,
            DriverDeliveryUsed::Foreground => DeliveryUsed::Foreground,
        },
        focus_changed: report.focus_changed,
        pointer_moved: report.pointer_moved,
        detail: match report.note {
            Some(note) => format!("{note}; {}", report.detail),
            None => report.detail,
        },
    }
}

/// `audio.desktop`'s limitation. Unsupported always names the reason (the
/// capability contract); supported carries the gVisor latency note.
fn audio_desktop_limitation(audio: bool, gvisor: bool, no_audio: &str) -> String {
    if !audio {
        no_audio.into()
    } else if gvisor {
        "under gVisor (runsc) the guest audio server's playback path adds ~15-25 ms over runc, so audio from apps that do not compensate for output latency reaches the capture 40-55 ms after their matching video (budget elsewhere is ±40 ms); capture timestamps themselves are accurate".into()
    } else {
        String::new()
    }
}

/// Whether the driver runs under gVisor (runsc): current releases name
/// themselves in the kernel version; older ones (still what Fleet's runsc
/// reports) have the synthetic release exactly "4.4.0". Same rule as
/// `cua_spacesd_server::services::system::is_gvisor`.
fn under_gvisor() -> bool {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let release = release.trim();
    release == "4.4.0"
        || release.to_ascii_lowercase().contains("gvisor")
        || std::fs::read_to_string("/proc/version")
            .is_ok_and(|version| version.to_ascii_lowercase().contains("gvisor"))
}

fn normalized_role(native: &str) -> String {
    let lower = native.to_lowercase();
    match lower.as_str() {
        "push button" | "button" | "toggle button" => "button".into(),
        "text" | "entry" | "password text" | "editbar" => "text_field".into(),
        "frame" | "window" | "dialog" => "window".into(),
        "check box" => "checkbox".into(),
        "radio button" => "radio_button".into(),
        "menu item" | "check menu item" | "radio menu item" => "menu_item".into(),
        "combo box" => "combo_box".into(),
        "label" | "static" => "static_text".into(),
        "list item" => "list_item".into(),
        "page tab" => "tab".into(),
        other => other.replace(' ', "_"),
    }
}

impl DesktopBackend for LinuxBackend {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError> {
        let (conn, root) = linux_x11::connect()?;
        Ok(linux_x11::displays(&conn, root))
    }

    fn desktop_ready(&self) -> Result<(), String> {
        let (conn, root) = linux_x11::connect().map_err(|error| error.message)?;
        if linux_x11::displays(&conn, root).is_empty() {
            return Err("the X server reports no display yet".into());
        }
        if !linux_x11::has_xtest(&conn) {
            return Err("the X server has no XTEST extension (cua-driver input needs it)".into());
        }
        // Before the window manager runs, new windows open unfocused and
        // undecorated, and typed keys reach no window.
        if !linux_x11::window_manager_running(&conn, root) {
            return Err(
                "no window manager is running yet (the desktop session is starting)".into(),
            );
        }
        Ok(())
    }

    fn windows(&self) -> Result<Vec<WindowRecord>, ProviderError> {
        let targets = self.bundle.targets.enumerate(&TargetQuery {
            on_screen_only: false,
        })?;
        let (conn, root) = linux_x11::connect()?;
        let facts: HashMap<u32, linux_x11::X11Window> = linux_x11::list_windows(&conn, root)
            .into_iter()
            .map(|window| (window.xid, window))
            .collect();
        let displays = linux_x11::displays(&conn, root);
        let mut records = Vec::new();
        for target in targets {
            let Some((_, window_id)) = self.bundle.native_of(&target.id) else {
                continue;
            };
            let Some(fact) = facts.get(&(window_id as u32)) else {
                continue;
            };
            let center = (
                f64::from(fact.x) + f64::from(fact.width) / 2.0,
                f64::from(fact.y) + f64::from(fact.height) / 2.0,
            );
            let display_id = displays
                .iter()
                .find(|display| {
                    let (x, y, w, h) = display.bounds;
                    center.0 >= x && center.1 >= y && center.0 < x + w && center.1 < y + h
                })
                .or(displays.first())
                .map(|display| display.id.clone())
                .unwrap_or_default();
            records.push(WindowRecord {
                handle: target.descriptor.window.clone(),
                epoch: target.id.epoch,
                title: fact.title.clone(),
                app_name: fact.app_name.clone(),
                app_id: fact.app_id.clone(),
                pid: fact.pid.unwrap_or(0),
                bounds: (
                    f64::from(fact.x),
                    f64::from(fact.y),
                    f64::from(fact.width),
                    f64::from(fact.height),
                ),
                display_id,
                state: match fact.state {
                    X11WindowState::Normal => WindowStateKind::Normal,
                    X11WindowState::Minimized => WindowStateKind::Minimized,
                    X11WindowState::Maximized => WindowStateKind::Maximized,
                    X11WindowState::Fullscreen => WindowStateKind::Fullscreen,
                    X11WindowState::Hidden => WindowStateKind::Hidden,
                },
                kind: match fact.kind {
                    X11WindowKind::Standard => WindowKind::Standard,
                    X11WindowKind::Dialog => WindowKind::Dialog,
                    X11WindowKind::Panel => WindowKind::Panel,
                    X11WindowKind::Menu => WindowKind::Menu,
                    X11WindowKind::Tooltip => WindowKind::Tooltip,
                    X11WindowKind::System => WindowKind::System,
                    X11WindowKind::Phantom => WindowKind::Phantom,
                },
                focused: fact.focused,
                on_screen: fact.mapped,
                z_order: fact.z_order,
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
        let (x, y, width, height) = display.bounds;
        let (bgra, w, h) = linux_stream::grab_once(StreamSource::Display {
            x: x as i16,
            y: y as i16,
            width: width as u16,
            height: height as u16,
        })?;
        Ok(CapturedImage {
            bgra,
            width: w,
            height: h,
            logical_bounds: display.bounds,
            display_id: display.id.clone(),
        })
    }

    fn capture_window(&self, window: &WindowRecord) -> Result<CapturedImage, ProviderError> {
        let (_, xid) = self.native(window)?;
        let (bgra, width, height) = linux_stream::grab_once(StreamSource::Window(xid))?;
        Ok(CapturedImage {
            bgra,
            width,
            height,
            logical_bounds: (
                window.bounds.0,
                window.bounds.1,
                f64::from(width),
                f64::from(height),
            ),
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
        let button = |name: &str| TargetedButton::from_name(name);
        let op = match action {
            PointerAction::Click {
                button: name,
                count,
                modifiers,
            } => PointerOp::Click {
                button: button(name),
                count: *count,
                modifiers: modifiers.clone(),
            },
            PointerAction::Move => PointerOp::Move,
            PointerAction::Down { button: name } => PointerOp::Down {
                button: button(name),
            },
            PointerAction::Up { button: name } => PointerOp::Up {
                button: button(name),
            },
            PointerAction::Drag {
                path,
                button: name,
                modifiers,
            } => PointerOp::Drag {
                path: path.iter().map(|(x, y)| (*x as i32, *y as i32)).collect(),
                button: button(name),
                modifiers: modifiers.clone(),
            },
            PointerAction::Scroll { dx, dy } => PointerOp::Scroll {
                dx: dx.round() as i32,
                dy: dy.round() as i32,
            },
        };
        targeted::pointer(&PointerRequest {
            window: native.map(|(_, xid)| xid),
            delivery: driver_delivery(delivery),
            allow_auto_foreground: driver_input::auto_foreground_allowed(),
            x: point.0.round() as i32,
            y: point.1.round() as i32,
            op,
        })
        .map(result)
        .map_err(driver_input::targeted_error)
    }

    fn keyboard(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        action: &KeyAction,
    ) -> Result<DeliveryResult, ProviderError> {
        let native = window.map(|window| self.native(window)).transpose()?;
        if let (KeyAction::Type { text, insert: true }, Some((pid, _))) = (action, native) {
            // Insert mode: one AT-SPI text insertion into the focused editable.
            if platform_linux::atspi::insert_text(pid, text).unwrap_or(false) {
                return Ok(DeliveryResult {
                    delivery: DeliveryUsed::Background,
                    focus_changed: false,
                    pointer_moved: false,
                    detail: "AT-SPI EditableText insertion".into(),
                });
            }
        }
        let op = match action {
            KeyAction::Type { text, .. } => KeyOp::Type(text.clone()),
            KeyAction::Press {
                key,
                modifiers,
                repeat,
            } => KeyOp::Press {
                key: key.clone(),
                modifiers: modifiers.clone(),
                repeat: *repeat,
            },
            KeyAction::Hotkey { keys } => KeyOp::Hotkey(keys.clone()),
            KeyAction::Down { key } => KeyOp::Down(key.clone()),
            KeyAction::Up { key } => KeyOp::Up(key.clone()),
        };
        targeted::keyboard(&KeyRequest {
            window: native.map(|(_, xid)| xid),
            delivery: driver_delivery(delivery),
            allow_auto_foreground: driver_input::auto_foreground_allowed(),
            op,
        })
        .map(result)
        .map_err(driver_input::targeted_error)
    }

    fn cursor_position(&self) -> Result<(f64, f64), ProviderError> {
        let (conn, root) = linux_x11::connect()?;
        let (x, y) = linux_x11::pointer_position(&conn, root)?;
        Ok((f64::from(x), f64::from(y)))
    }

    fn clipboard_get(&self) -> Result<ClipboardData, ProviderError> {
        use clipboard_rs::common::RustImage as _;
        use clipboard_rs::{Clipboard, ContentFormat};
        self.with_clipboard(|context| {
            let text = if context.has(ContentFormat::Text) {
                context.get_text().ok()
            } else {
                None
            };
            let files = if context.has(ContentFormat::Files) {
                context
                    .get_files()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|file| {
                        file.strip_prefix("file://")
                            .map(str::to_owned)
                            .unwrap_or(file)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            let image_png = if context.has(ContentFormat::Image) {
                context
                    .get_image()
                    .ok()
                    .and_then(|image| image.to_png().ok())
                    .map(|png| png.get_bytes().to_vec())
                    .filter(|png| png.len() <= MAX_CLIPBOARD_IMAGE_BYTES)
            } else {
                None
            };
            Ok(ClipboardData {
                text,
                files,
                image_png,
            })
        })
    }

    fn clipboard_set(&self, data: ClipboardData) -> Result<(), ProviderError> {
        use clipboard_rs::common::{RustImage as _, RustImageData};
        use clipboard_rs::{Clipboard, ClipboardContent};
        self.with_clipboard(|context| {
            let mut contents = Vec::new();
            if let Some(text) = data.text {
                contents.push(ClipboardContent::Text(text));
            }
            if !data.files.is_empty() {
                contents.push(ClipboardContent::Files(
                    data.files
                        .iter()
                        .map(|path| format!("file://{path}"))
                        .collect(),
                ));
            }
            if let Some(png) = data.image_png {
                let image = RustImageData::from_bytes(&png)
                    .map_err(|error| format!("clipboard image is not a PNG: {error}"))?;
                contents.push(ClipboardContent::Image(image));
            }
            if contents.is_empty() {
                return context.clear().map_err(|error| error.to_string());
            }
            context.set(contents).map_err(|error| error.to_string())
        })
    }

    fn window_action(
        &self,
        window: &WindowRecord,
        action: WindowAction,
    ) -> Result<(), ProviderError> {
        let (_, xid) = self.native(window)?;
        let (conn, root) = linux_x11::connect()?;
        match action {
            WindowAction::Activate => linux_x11::activate(&conn, root, xid),
            WindowAction::Minimize => linux_x11::minimize(&conn, root, xid),
            WindowAction::Maximize => linux_x11::maximize(&conn, root, xid),
            WindowAction::Restore => {
                linux_x11::restore(&conn, root, xid)?;
                linux_x11::activate(&conn, root, xid)
            }
            WindowAction::Close { force } => linux_x11::close(&conn, root, xid, force),
        }
    }

    fn set_window_bounds(
        &self,
        window: &WindowRecord,
        position: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
    ) -> Result<(), ProviderError> {
        let (_, xid) = self.native(window)?;
        let (conn, root) = linux_x11::connect()?;
        linux_x11::set_bounds(
            &conn,
            root,
            xid,
            position.map(|(x, y)| (x.round() as i32, y.round() as i32)),
            size.map(|(w, h)| (w.round().max(1.0) as u32, h.round().max(1.0) as u32)),
        )
    }

    fn launch(&self, request: &LaunchRequest) -> Result<u32, ProviderError> {
        match &request.app {
            AppSpecData::Executable(executable) => {
                let mut command = Command::new(executable);
                command
                    .args(&request.args)
                    .envs(request.env.iter().cloned())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Some(cwd) = &request.cwd {
                    command.current_dir(cwd);
                } else if let Some(home) = std::env::var_os("HOME") {
                    command.current_dir(home);
                }
                std::os::unix::process::CommandExt::process_group(&mut command, 0);
                let mut child = command.spawn().map_err(|error| {
                    ProviderError::new(
                        ProviderErrorCode::TargetUnavailable,
                        format!("cannot launch {executable}: {error}"),
                    )
                })?;
                let pid = child.id();
                // Reap the child when it exits so it never lingers as a zombie.
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                Ok(pid)
            }
            AppSpecData::AppId(name) | AppSpecData::Name(name) => {
                let registry = self.bundle.registry.clone();
                let arguments = serde_json::json!({
                    "name": name,
                    "additional_arguments": request.args,
                });
                let result = tokio::runtime::Handle::try_current()
                    .ok()
                    .map(|handle| handle.block_on(registry.invoke("launch_app", arguments.clone())))
                    .unwrap_or_else(|| {
                        tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("runtime")
                            .block_on(registry.invoke("launch_app", arguments))
                    });
                if result.is_error == Some(true) {
                    return Err(ProviderError::new(
                        ProviderErrorCode::TargetUnavailable,
                        format!("launch_app failed for {name}"),
                    ));
                }
                Ok(result
                    .structured_content
                    .as_ref()
                    .and_then(|value| value.get("pid"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0) as u32)
            }
        }
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
        let app = match with {
            Some(AppSpecData::Executable(executable)) => {
                AppSpecData::Executable(executable.clone())
            }
            Some(other) => other.clone(),
            None => AppSpecData::Executable("xdg-open".into()),
        };
        self.launch(&LaunchRequest {
            app,
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
        let (pid, xid) = self.native(window)?;
        if pid == 0 {
            return Err(unsupported(
                "the window does not report its process (_NET_WM_PID)",
            ));
        }
        let walked = platform_linux::atspi::walk_tree_bounded(
            pid,
            u64::from(xid),
            None,
            Some(if max_nodes == 0 {
                5_000
            } else {
                max_nodes as usize
            }),
            (max_depth > 0).then_some(max_depth as usize),
        );
        if !walked.trusted {
            return Err(ProviderError::new(
                ProviderErrorCode::Unsupported,
                walked.degraded_reason.unwrap_or_else(|| {
                    "AT-SPI is unavailable for this window (is the accessibility bus running?)"
                        .into()
                }),
            ));
        }
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.a11y_generation.lock().unwrap().insert(pid, generation);
        let bounds: HashMap<usize, (i32, i32, u32, u32)> = walked
            .bounds
            .iter()
            .map(|(index, x, y, w, h)| (*index, (*x, *y, *w, *h)))
            .collect();
        let nodes = walked
            .nodes
            .iter()
            .enumerate()
            .map(|(position, node)| {
                let element_id = node
                    .element_index
                    .map(|index| index.to_string())
                    .unwrap_or_else(|| format!("n{position}"));
                let mut states = Vec::new();
                if node.enabled == Some(true) {
                    states.push("enabled".to_owned());
                }
                if node.checked == Some(true) {
                    states.push("checked".to_owned());
                }
                if node.selected == Some(true) {
                    states.push("selected".to_owned());
                }
                A11yNode {
                    element_id,
                    parent_id: node
                        .parent_element_index
                        .map(|index| index.to_string())
                        .unwrap_or_default(),
                    depth: node.depth as u32,
                    role: normalized_role(&node.role),
                    native_role: node.role.clone(),
                    name: node.name.clone().unwrap_or_default(),
                    value: node.value.clone().unwrap_or_default(),
                    description: node.description.clone().unwrap_or_default(),
                    bounds: node.element_index.and_then(|index| bounds.get(&index)).map(
                        |(x, y, w, h)| (f64::from(*x), f64::from(*y), f64::from(*w), f64::from(*h)),
                    ),
                    states,
                    actions: node.actions.clone(),
                }
            })
            .collect::<Vec<_>>();
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
        let (pid, _) = self.native(window)?;
        if self.a11y_generation.lock().unwrap().get(&pid) != Some(&snapshot) {
            return Err(ProviderError::new(
                ProviderErrorCode::StaleTarget,
                "accessibility snapshot is no longer current",
            ));
        }
        let index: usize = element_id
            .parse()
            .map_err(|_| unsupported("this element has no actionable index"))?;
        match action {
            A11yActionKind::Press
            | A11yActionKind::Select
            | A11yActionKind::Expand
            | A11yActionKind::Collapse => platform_linux::atspi::perform_action(pid, index)
                .map(|_| ())
                .map_err(failed),
            A11yActionKind::Focus => platform_linux::atspi::focus_element(pid, index)
                .map(|_| ())
                .map_err(failed),
            A11yActionKind::SetValue => {
                platform_linux::atspi::set_value(pid, index, value).map_err(failed)
            }
            A11yActionKind::ScrollIntoView => platform_linux::atspi::scroll_element(
                pid,
                index,
                "down",
                0,
                cua_driver_contract::ScrollBy::Line,
            )
            .map(|_| ())
            .map_err(failed),
            _ => Err(unsupported(
                "this accessibility action is not available through AT-SPI here",
            )),
        }
    }

    fn media_providers(&self) -> MediaProviders {
        MediaProviders {
            targets: self.bundle.targets.clone(),
            displays: self.bundle.displays.clone(),
            captures: self.bundle.captures.clone(),
            actions: self.bundle.actions.clone(),
            accessibility: self.bundle.accessibility.clone(),
            geometry: self.bundle.geometry.clone(),
            inputs: self.bundle.inputs.clone(),
            audio: self.audio.clone(),
        }
    }

    fn tool_registry(&self) -> Option<Arc<cua_driver_core::tool::ToolRegistry>> {
        Some(self.bundle.registry.clone())
    }

    fn features(&self) -> Vec<FeatureStatus> {
        let x11 = linux_x11::connect().is_ok();
        let no_display = "no X11 display (DISPLAY is unset or unreachable)";
        let feature = |name: &'static str, ok: bool, why: &str| {
            if ok {
                FeatureStatus::yes(name)
            } else {
                FeatureStatus::no(name, why)
            }
        };
        let a11y_bus = std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some();
        let encoders = cua_media_codec::probe();
        let hardware = encoders.iter().find(|encoder| {
            encoder.status == cua_media_codec::ProbeStatus::Available && encoder.hardware
        });
        let mut h264_hw = match hardware {
            Some(_) => FeatureStatus::yes("h264_hw"),
            None => FeatureStatus::no(
                "h264_hw",
                "no hardware H.264 encoder was found (NVENC, VA-API, QSV, AMF)",
            ),
        };
        if let Some(encoder) = hardware {
            h264_hw.attributes.push((
                "encoder".into(),
                format!("{:?}", encoder.backend).to_lowercase(),
            ));
        }
        let audio = self.audio.is_some();
        let no_audio = "no PulseAudio/PipeWire server is reachable";
        vec![
            feature("desktop_stream", x11, no_display),
            FeatureStatus {
                attributes: vec![("occlusion_free".into(), "composite".into())],
                ..feature("window_stream", x11, no_display)
            },
            feature("h264_sw", true, ""),
            h264_hw,
            FeatureStatus {
                limitation: "background input uses XSendEvent, which GTK3/GTK4 and Chromium ignore: AUTO delivers to those with foreground XTest (CUA_ENV_AUTO_FOREGROUND=0 makes it fail instead) and BACKGROUND to them fails with WOULD_REQUIRE_ACTIVATION; use AccessibilityService.Act for true background".into(),
                ..feature("background_input", x11, no_display)
            },
            feature("a11y", a11y_bus, "AT-SPI needs a D-Bus session bus (start the desktop with dbus-run-session)"),
            feature("windows", x11, no_display),
            feature("launch_app", true, ""),
            feature("clipboard.text", x11, no_display),
            feature("clipboard.files", x11, no_display),
            feature("clipboard.image", x11, no_display),
            FeatureStatus {
                limitation: audio_desktop_limitation(audio, audio && under_gvisor(), no_audio),
                ..feature("audio.desktop", audio, no_audio)
            },
            FeatureStatus {
                limitation: if audio {
                    "per-app capture needs the app to play through PulseAudio/PipeWire; otherwise the desktop mix is sent with desktop_fallback".into()
                } else {
                    no_audio.into()
                },
                ..feature("audio.per_app", audio, no_audio)
            },
            feature("audio.uplink", audio, no_audio),
            feature("audio.opus", audio, no_audio),
        ]
    }
}

#[cfg(test)]
mod audio_capability_tests {
    use super::audio_desktop_limitation;

    #[test]
    fn unsupported_audio_desktop_always_names_its_limitation() {
        // Regression: the gVisor note's struct update used to blank the
        // unsupported reason (relay conformance: "audio.desktop unsupported
        // without limitation").
        for gvisor in [false, true] {
            assert_eq!(
                audio_desktop_limitation(false, gvisor, "no pulse"),
                "no pulse"
            );
        }
        assert!(audio_desktop_limitation(true, false, "no pulse").is_empty());
        assert!(audio_desktop_limitation(true, true, "no pulse").contains("gVisor"));
    }
}
