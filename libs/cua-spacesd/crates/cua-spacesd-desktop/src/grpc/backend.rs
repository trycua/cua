// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The platform seam behind the desktop gRPC services.
//!
//! Services translate protobuf requests into these plain types (screen
//! points in global logical points, opaque window handles) and back. A
//! backend never sees protobuf, and tests drive the services with a fake.

use cua_media_protocol::{TargetEpoch, TargetHandle};
use cua_spacesd_provider_api::{ProviderDisplay, ProviderError};
use cua_spacesd_session::media::MediaProviders;

/// Largest clipboard image exchanged (`ClipboardContent.image_png`).
pub const MAX_CLIPBOARD_IMAGE_BYTES: usize = 16 * 1024 * 1024;

/// Clipboard flavors a backend exchanges.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct ClipboardData {
    /// Plain text.
    pub text: Option<String>,
    /// Absolute file paths.
    pub files: Vec<String>,
    /// A PNG image.
    pub image_png: Option<Vec<u8>>,
}

/// Requested input delivery (mirrors `cua.env.v1.Delivery`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryRequest {
    Auto,
    Background,
    Foreground,
}

/// Delivery actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryUsed {
    Background,
    Foreground,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeliveryResult {
    pub delivery: DeliveryUsed,
    pub focus_changed: bool,
    pub pointer_moved: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Standard,
    Dialog,
    Panel,
    Menu,
    Tooltip,
    System,
    Phantom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStateKind {
    Normal,
    Minimized,
    Maximized,
    Fullscreen,
    Hidden,
}

/// One window as the services report it.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowRecord {
    pub handle: TargetHandle,
    pub epoch: TargetEpoch,
    pub title: String,
    pub app_name: String,
    pub app_id: String,
    pub pid: u32,
    /// Global logical points: x, y, width, height.
    pub bounds: (f64, f64, f64, f64),
    pub display_id: String,
    pub state: WindowStateKind,
    pub kind: WindowKind,
    pub focused: bool,
    pub on_screen: bool,
    pub z_order: u32,
}

/// Packed top-down BGRA capture plus where it came from.
#[derive(Debug, Clone)]
pub struct CapturedImage {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
    /// Area covered, in global logical points.
    pub logical_bounds: (f64, f64, f64, f64),
    pub display_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PointerAction {
    Click {
        button: String,
        count: u32,
        modifiers: Vec<String>,
    },
    Move,
    Down {
        button: String,
    },
    Up {
        button: String,
    },
    /// Further points (screen) after the start point.
    Drag {
        path: Vec<(f64, f64)>,
        button: String,
        modifiers: Vec<String>,
    },
    /// Wheel lines; positive y scrolls content down, positive x right.
    Scroll {
        dx: f64,
        dy: f64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyAction {
    Type {
        text: String,
        insert: bool,
    },
    Press {
        key: String,
        modifiers: Vec<String>,
        repeat: u32,
    },
    Hotkey {
        keys: Vec<String>,
    },
    Down {
        key: String,
    },
    Up {
        key: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    Activate,
    Minimize,
    Maximize,
    Restore,
    Close { force: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppSpecData {
    AppId(String),
    Executable(String),
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaunchRequest {
    pub app: AppSpecData,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<String>,
    pub background: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenTarget {
    Url(String),
    Path(String),
}

/// One accessibility element in pre-order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct A11yNode {
    pub element_id: String,
    pub parent_id: String,
    pub depth: u32,
    pub role: String,
    pub native_role: String,
    pub name: String,
    pub value: String,
    pub description: String,
    pub bounds: Option<(f64, f64, f64, f64)>,
    pub states: Vec<String>,
    pub actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct A11yTree {
    /// Backend snapshot token; the service wraps it in its own snapshot id.
    pub snapshot: u64,
    pub nodes: Vec<A11yNode>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum A11yActionKind {
    Press,
    Focus,
    SetValue,
    Increment,
    Decrement,
    ShowMenu,
    Expand,
    Collapse,
    Select,
    ScrollIntoView,
    Custom,
}

/// A feature for `GetCapabilities` (name, supported, limitation).
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureStatus {
    pub name: &'static str,
    pub supported: bool,
    pub limitation: String,
    pub attributes: Vec<(String, String)>,
}

impl FeatureStatus {
    pub fn yes(name: &'static str) -> Self {
        Self {
            name,
            supported: true,
            limitation: String::new(),
            attributes: Vec::new(),
        }
    }

    pub fn no(name: &'static str, limitation: impl Into<String>) -> Self {
        Self {
            name,
            supported: false,
            limitation: limitation.into(),
            attributes: Vec::new(),
        }
    }
}

/// Platform backend for Computer, Windows and Accessibility, plus the
/// providers the media plane uses.
pub trait DesktopBackend: Send + Sync + 'static {
    fn displays(&self) -> Result<Vec<ProviderDisplay>, ProviderError>;

    /// Every window front to back, including system and phantom windows
    /// (the services filter).
    fn windows(&self) -> Result<Vec<WindowRecord>, ProviderError>;

    fn capture_display(&self, display_id: &str) -> Result<CapturedImage, ProviderError>;

    fn capture_window(&self, window: &WindowRecord) -> Result<CapturedImage, ProviderError>;

    /// Pointer input at a global logical point.
    fn pointer(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        point: (f64, f64),
        action: &PointerAction,
    ) -> Result<DeliveryResult, ProviderError>;

    fn keyboard(
        &self,
        window: Option<&WindowRecord>,
        delivery: DeliveryRequest,
        action: &KeyAction,
    ) -> Result<DeliveryResult, ProviderError>;

    fn cursor_position(&self) -> Result<(f64, f64), ProviderError>;

    fn clipboard_get(&self) -> Result<ClipboardData, ProviderError>;

    /// Replaces every flavor with `data` (all empty clears the clipboard).
    fn clipboard_set(&self, data: ClipboardData) -> Result<(), ProviderError>;

    fn window_action(
        &self,
        window: &WindowRecord,
        action: WindowAction,
    ) -> Result<(), ProviderError>;

    fn set_window_bounds(
        &self,
        window: &WindowRecord,
        position: Option<(f64, f64)>,
        size: Option<(f64, f64)>,
    ) -> Result<(), ProviderError>;

    /// Returns the launched pid (0 when unknown).
    fn launch(&self, request: &LaunchRequest) -> Result<u32, ProviderError>;

    fn open(
        &self,
        target: &OpenTarget,
        with: Option<&AppSpecData>,
        background: bool,
    ) -> Result<u32, ProviderError>;

    fn a11y_tree(
        &self,
        window: &WindowRecord,
        max_depth: u32,
        max_nodes: u32,
    ) -> Result<A11yTree, ProviderError>;

    /// Act on an element of the backend snapshot `snapshot`.
    fn a11y_act(
        &self,
        window: &WindowRecord,
        snapshot: u64,
        element_id: &str,
        action: A11yActionKind,
        value: &str,
    ) -> Result<(), ProviderError>;

    fn media_providers(&self) -> MediaProviders;

    /// The cua-driver tool registry this backend drives, shared with the
    /// server's Driver service and `/mcp`.
    fn tool_registry(&self) -> Option<std::sync::Arc<cua_driver_core::tool::ToolRegistry>> {
        None
    }

    fn features(&self) -> Vec<FeatureStatus>;

    /// Whether the desktop session can take input, for the `desktop`
    /// component of `Health`: `Err` names what is missing. The default
    /// checks that a display is reported; platforms add their own checks
    /// (a window manager, the input path cua-driver uses).
    fn desktop_ready(&self) -> Result<(), String> {
        match self.displays() {
            Ok(displays) if !displays.is_empty() => Ok(()),
            Ok(_) => Err("no display yet".into()),
            Err(error) => Err(format!("display unavailable: {}", error.message)),
        }
    }
}
