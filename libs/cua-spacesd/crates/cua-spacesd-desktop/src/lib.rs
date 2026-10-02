// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Adapter-owned RCDP provider implementations for `cua-driver`.
//!
//! The wrappers in this crate are the only production types that know both
//! RCDP contracts and CUA native arguments. Native process and window IDs stay
//! in the shared private catalog and are injected only immediately before a
//! CUA tool invocation.

use std::collections::{HashMap, HashSet};
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
mod macos_capture;
#[cfg(target_os = "macos")]
mod macos_cursor_shape;
#[cfg(target_os = "macos")]
mod macos_display;
#[cfg(target_os = "macos")]
mod macos_geometry;
#[cfg(target_os = "macos")]
mod macos_h264;

#[cfg(target_os = "windows")]
mod windows_capture;
#[cfg(target_os = "windows")]
mod windows_display;
#[cfg(target_os = "windows")]
mod windows_h264;
#[cfg(target_os = "windows")]
mod windows_support;

pub mod codec_adapter;
pub mod codec_audio;
pub mod codec_encoder;
mod driver_input;
#[cfg(target_os = "linux")]
mod encoded_capture;
pub mod grpc;
#[cfg(target_os = "linux")]
mod linux_capture;
#[cfg(target_os = "linux")]
mod linux_screencopy;
#[cfg(target_os = "linux")]
pub(crate) mod linux_stream;
#[cfg(target_os = "linux")]
mod linux_wayland;
#[cfg(target_os = "linux")]
pub(crate) mod linux_x11;
mod tool_input;

use cua_driver_core::protocol::{Content, ToolResult};
use cua_driver_core::tool::ToolRegistry;
use cua_media_protocol::{
    AccessibilitySnapshotId, ActionCapability, ActionDeliveryGuarantee, SessionPolicy,
    SurfaceGeometry, TargetEpoch, TargetGrant, TargetHandle, WindowDescriptor,
};
use cua_spacesd_provider_api::{
    enforce_action_policy, AccessibilityProvider, AccessibilitySnapshot, ActionInvocation,
    ActionOutcome, ActionProvider, AppliedWindowGeometry, BackendTargetKey, CaptureConfig,
    CaptureLease, CaptureProvider, CaptureSink, InteractiveInputLease, InteractiveInputProvider,
    PickTargetRequest, PixelFormat, ProviderAppIcon, ProviderError, ProviderErrorCode,
    ProviderFuture, ProviderTarget, ProviderTargetId, TargetProvider, TargetQuery,
    WindowGeometryProvider,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct NativeTarget {
    pid: i64,
    window_id: u64,
}

#[derive(Debug, Clone)]
struct NativeWindow {
    native: NativeTarget,
    application_id: Option<String>,
    app_name: String,
    title: String,
    geometry: SurfaceGeometry,
    visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NativePixelGeometry {
    width_px: u32,
    height_px: u32,
}

pub(crate) const CUA_PROVIDER_SESSION_ID: &str = "rcdp-provider";

/// The cua-driver session a human viewer's media input runs in: one per
/// principal, marked human-origin, so the driver draws no agent cursor for it
/// and the cursor hook never publishes it as an agent (the viewer's client
/// draws and publishes the viewer's own cursor).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn human_input_session(principal_id: &str) -> String {
    format!("rcdp-human:{principal_id}")
}

/// Trusted-adapter arguments for a tool call in the human-input `session`:
/// the private session id and `_input_origin: human` (both reserved, so no
/// public caller can supply them).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn human_input_arguments(
    session: &str,
    mut arguments: serde_json::Map<String, serde_json::Value>,
) -> serde_json::Value {
    arguments.insert("_session_id".into(), session.to_owned().into());
    arguments.insert(
        cua_driver_core::agent_cursor::INPUT_ORIGIN_ARG.into(),
        cua_driver_core::agent_cursor::InputOrigin::Human
            .as_str()
            .into(),
    );
    serde_json::Value::Object(arguments)
}
/// Actions the Linux X11/XTest input backend can inject. Advertised as the
/// target's action capabilities so cua-spacesd accepts these tools over the action
/// channel; each is a background-safe guarantee (see `action_guarantee`).
#[cfg(target_os = "linux")]
const LINUX_INPUT_ACTIONS: &[&str] = &[
    "click",
    "double_click",
    "right_click",
    "scroll",
    "drag",
    "press_key",
    "hotkey",
    "type_text",
];
#[cfg(target_os = "macos")]
const MAX_NATIVE_APP_ICON_BYTES: u64 = 8 * 1024 * 1024;
type NativeGeometryCallback = Arc<dyn Fn(u32, u32) + Send + Sync>;

#[derive(Debug, Clone)]
struct CatalogEntry {
    target: ProviderTarget,
    native: NativeTarget,
    active: bool,
}

#[derive(Default)]
struct TargetCatalog {
    by_native: HashMap<NativeTarget, CatalogEntry>,
    by_handle: HashMap<TargetHandle, NativeTarget>,
    by_provider_id: HashMap<ProviderTargetId, NativeTarget>,
    by_grant: HashMap<TargetGrant, NativeTarget>,
}

impl TargetCatalog {
    fn refresh(&mut self, windows: Vec<NativeWindow>) -> Vec<ProviderTarget> {
        let seen = windows
            .iter()
            .map(|window| window.native)
            .collect::<HashSet<_>>();
        for (native, entry) in &mut self.by_native {
            if !seen.contains(native) {
                entry.active = false;
            }
        }

        let mut targets = Vec::with_capacity(windows.len());
        for window in windows {
            if let Some(entry) = self.by_native.get_mut(&window.native) {
                if !entry.active {
                    entry.target.id.epoch = TargetEpoch(entry.target.id.epoch.0.saturating_add(1));
                    entry.target.descriptor.target_epoch = entry.target.id.epoch;
                }
                entry.active = true;
                entry.target.descriptor.app_name = window.app_name;
                entry.target.descriptor.title = window.title;
                entry.target.descriptor.geometry = window.geometry;
                entry.target.descriptor.visible = window.visible;
                self.by_provider_id
                    .insert(entry.target.id.clone(), window.native);
                targets.push(entry.target.clone());
                continue;
            }

            let handle = TargetHandle(format!("target-{}", uuid::Uuid::new_v4()));
            let grant = TargetGrant(format!("grant-{}", uuid::Uuid::new_v4()));
            let id = ProviderTargetId {
                key: BackendTargetKey::new(format!(
                    "macos:{}:{}",
                    window.native.pid, window.native.window_id
                )),
                epoch: TargetEpoch(1),
            };
            let target = ProviderTarget {
                id: id.clone(),
                descriptor: WindowDescriptor {
                    window: handle.clone(),
                    target_epoch: id.epoch,
                    app_name: window.app_name,
                    title: window.title,
                    geometry: window.geometry,
                    visible: window.visible,
                },
                grant: Some(grant.clone()),
            };
            self.by_handle.insert(handle, window.native);
            self.by_provider_id.insert(id, window.native);
            self.by_grant.insert(grant, window.native);
            self.by_native.insert(
                window.native,
                CatalogEntry {
                    target: target.clone(),
                    native: window.native,
                    active: true,
                },
            );
            targets.push(target);
        }
        targets
    }

    fn resolve(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<ProviderTarget, ProviderError> {
        let native = self.by_handle.get(handle).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target handle is unknown",
            )
        })?;
        let entry = self.by_native.get(native).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target is unavailable",
            )
        })?;
        if !entry.active {
            return Err(ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target is no longer active",
            ));
        }
        if entry.target.id.epoch != epoch {
            return Err(ProviderError::new(
                ProviderErrorCode::StaleTarget,
                format!(
                    "target epoch is stale: current={}, requested={}",
                    entry.target.id.epoch.0, epoch.0
                ),
            ));
        }
        Ok(entry.target.clone())
    }

    fn restore(&self, grant: &TargetGrant) -> Result<ProviderTarget, ProviderError> {
        let native = self.by_grant.get(grant).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target grant is unknown",
            )
        })?;
        let entry = self.by_native.get(native).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target is unavailable",
            )
        })?;
        if !entry.active {
            return Err(ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "granted target is no longer active",
            ));
        }
        Ok(entry.target.clone())
    }

    fn native(&self, target: &ProviderTargetId) -> Result<NativeTarget, ProviderError> {
        let native = self.by_provider_id.get(target).copied().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "provider target is unknown",
            )
        })?;
        let entry = self.by_native.get(&native).ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "target is unavailable",
            )
        })?;
        if !entry.active || entry.target.id.epoch != target.epoch {
            return Err(ProviderError::new(
                ProviderErrorCode::StaleTarget,
                "provider target epoch is stale",
            ));
        }
        Ok(entry.native)
    }
}

/// A background session reaches a Hyprland window only through the
/// cua-hyprland-plugin's isolated seats, which admit qualified clients only.
/// Any other window is refused when the session opens, as X11 refuses a
/// toolkit that drops synthetic events, instead of acknowledging every
/// batch as undelivered.
#[cfg(target_os = "linux")]
fn hyprland_background_admission(native: NativeTarget) -> Result<(), ProviderError> {
    use platform_linux::wayland::hyprland_input;
    if !hyprland_input::enabled() {
        return Ok(());
    }
    let pid = u32::try_from(native.pid).unwrap_or(0);
    hyprland_input::background_admission(pid).map_err(|reason| {
        ProviderError::new(
            ProviderErrorCode::WouldRequireActivation,
            format!(
                "Hyprland background input does not reach this window ({reason}); \
                 open the session with allow_activation"
            ),
        )
    })
}

/// [`tool_input::GestureTools`] over the embedded cua-driver registry, in
/// native pixels. A human viewer's input runs in its own human-origin driver
/// session ([`human_input_session`]): no agent cursor in the guest, and none
/// in presence. An agent's runs in the provider's coordinate session and
/// keeps the agent cursor.
#[cfg(target_os = "linux")]
struct RegistryGestureTools {
    inner: Arc<CuaProviderInner>,
    /// The human-input session, `None` for an agent.
    human_session: Option<String>,
}

#[cfg(target_os = "linux")]
impl tool_input::GestureTools for RegistryGestureTools {
    fn invoke(
        &self,
        tool: &str,
        mut arguments: serde_json::Map<String, serde_json::Value>,
    ) -> Result<(), ProviderError> {
        let inner = self.inner.clone();
        let name = tool.to_owned();
        let human_session = self.human_session.clone();
        let result = grpc::tool_backend::block_on_driver(async move {
            if let Some(session) = human_session {
                inner.ensure_human_session(&session).await?;
                // Gesture points are native pixels of the streamed target.
                return Ok::<_, ProviderError>(
                    inner
                        .registry
                        .invoke_from_trusted_adapter_with_native_window_pixels(
                            &name,
                            human_input_arguments(&session, arguments),
                        )
                        .await,
                );
            }
            arguments.insert(
                "_session_id".into(),
                CUA_PROVIDER_SESSION_ID.to_owned().into(),
            );
            inner.ensure_coordinate_session().await?;
            Ok::<_, ProviderError>(
                inner
                    .registry
                    .invoke_with_native_window_pixels(&name, serde_json::Value::Object(arguments))
                    .await,
            )
        })
        .map_err(|error| ProviderError::new(ProviderErrorCode::Internal, error))??;
        if result.is_error == Some(true) {
            return Err(ProviderError::new(
                ProviderErrorCode::DeliveryFailed,
                format!("{tool}: {}", tool_result_text(&result)),
            ));
        }
        Ok(())
    }
}

struct CuaProviderInner {
    catalog: Mutex<TargetCatalog>,
    registry: Arc<ToolRegistry>,
    next_snapshot: AtomicU32,
    coordinate_session_configured: AtomicBool,
    /// Human-input driver sessions (see [`human_input_session`]) whose
    /// coordinate config is set.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    human_sessions_configured: Mutex<std::collections::HashSet<String>>,
    native_pixel_geometries: Mutex<HashMap<NativeTarget, NativePixelGeometry>>,
    application: Option<ApplicationSelector>,
    /// Last-reported pressed state per presence-cursor key, for press-edge
    /// detection on the desktop overlay.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    cursor_pressed: Mutex<HashMap<String, bool>>,
    /// Sink for the CUA agent cursor (see `CuaProviderConfig::agent_cursor`).
    agent_cursor: Option<tokio::sync::mpsc::UnboundedSender<cua_media_protocol::CursorState>>,
    /// Video encoder for CPU-captured frames (Linux X11).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    encoder: Arc<dyn cua_spacesd_session::media::encoder::FrameEncoderFactory>,
}

impl CuaProviderInner {
    fn refresh(&self, on_screen_only: bool) -> Result<Vec<ProviderTarget>, ProviderError> {
        let windows = native_windows(on_screen_only)?
            .into_iter()
            .filter(|window| match &self.application {
                None => true,
                Some(ApplicationSelector::Id(expected)) => {
                    window.application_id.as_ref() == Some(expected)
                }
                Some(ApplicationSelector::Name(expected)) => &window.app_name == expected,
            })
            .collect();
        Ok(self
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .refresh(windows))
    }

    fn native(&self, target: &ProviderTargetId) -> Result<NativeTarget, ProviderError> {
        self.catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .native(target)
    }

    fn set_native_pixel_geometry(&self, native: NativeTarget, width_px: u32, height_px: u32) {
        self.native_pixel_geometries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                native,
                NativePixelGeometry {
                    width_px,
                    height_px,
                },
            );
    }

    fn native_pixel_geometry(&self, native: NativeTarget) -> Option<NativePixelGeometry> {
        self.native_pixel_geometries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&native)
            .copied()
    }

    async fn ensure_coordinate_session(&self) -> Result<(), ProviderError> {
        if self.coordinate_session_configured.load(Ordering::Acquire) {
            return Ok(());
        }
        let result = self
            .registry
            .invoke(
                "set_config",
                serde_json::json!({
                    "_session_id": CUA_PROVIDER_SESSION_ID,
                    "max_image_dimension": 0,
                }),
            )
            .await;
        if result.is_error == Some(true) {
            return Err(ProviderError::new(
                ProviderErrorCode::Internal,
                tool_result_text(&result),
            ));
        }
        let cursor_result = self
            .registry
            .invoke(
                "set_agent_cursor_enabled",
                serde_json::json!({
                    "enabled": false,
                    "_session_id": CUA_PROVIDER_SESSION_ID,
                }),
            )
            .await;
        if cursor_result.is_error == Some(true) {
            return Err(ProviderError::new(
                ProviderErrorCode::Internal,
                "RCDP could not disable its CUA agent cursor",
            ));
        }
        self.coordinate_session_configured
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Configure a human-input driver session like the provider's own
    /// coordinate session (native pixels), once.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    async fn ensure_human_session(&self, session: &str) -> Result<(), ProviderError> {
        if self
            .human_sessions_configured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(session)
        {
            return Ok(());
        }
        let result = self
            .registry
            .invoke_from_trusted_adapter(
                "set_config",
                human_input_arguments(
                    session,
                    serde_json::Map::from_iter([(
                        "max_image_dimension".to_owned(),
                        serde_json::Value::from(0),
                    )]),
                ),
            )
            .await;
        if result.is_error == Some(true) {
            return Err(ProviderError::new(
                ProviderErrorCode::Internal,
                tool_result_text(&result),
            ));
        }
        self.human_sessions_configured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(session.to_owned());
        Ok(())
    }

    /// Consume a cua-driver cursor hook event (screen-logical coords) and, for
    /// every window this provider is capturing, emit a window-local `CursorState`
    /// to the agent-cursor sink when the cursor is over that window. This is how
    /// rcdp "always knows where the cursor is and what window it is over": the
    /// driver's cursor moves fire the hook (see cua-driver `cursor_hook`), and we
    /// map the screen point into each captured window's frame-pixel space.
    ///
    /// Mapping a screen point into a window needs that window's on-screen
    /// bounds, and the only implementation of that lookup here is
    /// `platform_macos::windows::window_bounds_by_id`. So the agent-cursor
    /// overlay is macOS-only for now; elsewhere (the Hyprland/Wayland capture
    /// path included) this is a no-op and no `CursorState` is emitted. Window
    /// streaming itself is unaffected.
    #[cfg(target_os = "macos")]
    fn on_cursor_event(&self, ev: cua_driver_core::cursor_hook::CursorHookEvent) {
        let Some(sink) = self.agent_cursor.as_ref() else {
            return;
        };
        // A human viewer's relayed input is not an agent cursor.
        if !grpc::agent_cursor_event(&ev) {
            return;
        }
        // A captured window has a native pixel geometry (set when capture starts).
        let captured: Vec<(NativeTarget, NativePixelGeometry)> = {
            let geoms = self
                .native_pixel_geometries
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            geoms.iter().map(|(n, g)| (*n, *g)).collect()
        };
        if captured.is_empty() {
            return;
        }
        let catalog = self
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (native, geom) in captured {
            let Some(bounds) =
                platform_macos::windows::window_bounds_by_id(native.window_id as u32)
            else {
                continue;
            };
            if bounds.width <= 0.0 || bounds.height <= 0.0 {
                continue;
            }
            // Only report the cursor for the window it is actually over.
            if ev.x < bounds.x
                || ev.y < bounds.y
                || ev.x >= bounds.x + bounds.width
                || ev.y >= bounds.y + bounds.height
            {
                continue;
            }
            // Screen-logical -> window-local frame pixels (the space clients draw
            // in): offset by the window origin, scale by capture_px / window_pt.
            let x = (ev.x - bounds.x) * (f64::from(geom.width_px) / bounds.width);
            let y = (ev.y - bounds.y) * (f64::from(geom.height_px) / bounds.height);
            let window = catalog
                .by_native
                .get(&native)
                .map(|entry| entry.target.descriptor.window.clone());
            let (user_id, name, color) = agent_cursor_identity(&ev.cursor_id);
            // The origin desktop has exactly one physical pointer, and this
            // event is the driver moving it, so the system shape read now is
            // the shape under *this* cursor. For a participant who does not
            // own the pointer we would have to hit-test another point's
            // cursor, which no supported platform exposes -- see
            // `host_cursor_shape`.
            let shape = host_cursor_shape();
            let _ = sink.send(cua_media_protocol::CursorState {
                user_id,
                name,
                color,
                window,
                x,
                y,
                visible: true,
                pressed: ev.pressed,
                shape: shape.clone(),
            });
        }
    }

    /// No-op counterpart for platforms without a window-bounds lookup. Keeping
    /// the method present means the hook registration stays platform-independent.
    #[cfg(not(target_os = "macos"))]
    fn on_cursor_event(&self, _ev: cua_driver_core::cursor_hook::CursorHookEvent) {}
}

/// Reserved presence identity for the anonymous CUA agent cursor (mirrors the
/// `"host"` synthetic id convention). Not a real connected user. Used only for
/// the driver's unnamed `"default"` cursor.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AGENT_CURSOR_USER_ID: &str = "cua-agent";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AGENT_CURSOR_NAME: &str = "CUA agent";
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AGENT_CURSOR_COLOR: &str = "#3b82f6";

/// Translate the driver's platform-neutral system cursor shape into the
/// protocol's.
///
/// Two separate vocabularies deliberately: the driver's is about what the OS
/// draws, the protocol's is what a viewer must render, and neither should
/// depend on the other's crate. This is the single seam between them.
///
/// `Unknown` is preserved rather than smoothed into `Default`. It means the
/// host cannot report a shape, and a viewer should hold the last shape it saw
/// instead of snapping to an arrow.
pub fn host_cursor_shape() -> cua_media_protocol::CursorShape {
    use cua_driver_core::cursor_shape::{ResizeAxis as Driver, SystemCursorShape as Shape};
    use cua_media_protocol::{CursorShape as Wire, ResizeAxis as WireAxis};

    let axis = |axis: Driver| match axis {
        Driver::NorthSouth => WireAxis::NorthSouth,
        Driver::EastWest => WireAxis::EastWest,
        Driver::NorthEastSouthWest => WireAxis::NorthEastSouthWest,
        Driver::NorthWestSouthEast => WireAxis::NorthWestSouthEast,
        Driver::All => WireAxis::All,
        Driver::Column => WireAxis::Column,
        Driver::Row => WireAxis::Row,
    };

    let shape = match cua_driver_core::cursor_shape::current_system_cursor_shape() {
        Shape::Default => Wire::Default,
        Shape::Text => Wire::Text,
        Shape::VerticalText => Wire::VerticalText,
        Shape::Pointer => Wire::Pointer,
        Shape::Grab => Wire::Grab,
        Shape::Grabbing => Wire::Grabbing,
        Shape::Crosshair => Wire::Crosshair,
        // The v1 media vocabulary has no separate progress cursor.
        Shape::Wait | Shape::Progress => Wire::Wait,
        Shape::NotAllowed => Wire::NotAllowed,
        Shape::Resize(a) => Wire::Resize { axis: axis(a) },
        Shape::Custom {
            png,
            hotspot_x,
            hotspot_y,
            scale,
        } => Wire::Custom {
            png,
            hotspot_x,
            hotspot_y,
            scale,
        },
        Shape::Unknown => Wire::Unknown,
    };
    // Never forward an unbounded bitmap through the presence channel.
    shape.bounded()
}

/// Map one cua-driver cursor id onto a presence identity.
///
/// cua-driver is already multi-cursor: every driver session owns a distinct
/// `CursorKey`, and `CursorHookEvent::cursor_id` carries it. Collapsing every
/// session into the single `"cua-agent"` id made each agent's move look like
/// the *same* cursor teleporting across the window, and made two concurrent
/// agents indistinguishable. Keying presence by the driver's own cursor id is
/// what makes N simultaneous agents render as N distinct cursors.
///
/// The color is `cursor_overlay::session_fill_hex`, the exact function the
/// driver's own desktop overlay uses to tint that session's cursor, so one
/// agent is the same color on the origin desktop and in the window stream.
/// It is derived from the id rather than agent-supplied, so an agent cannot
/// choose its own identity color.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn agent_cursor_identity(cursor_id: &str) -> (String, String, String) {
    if cursor_id.is_empty() || cursor_id == "default" {
        return (
            AGENT_CURSOR_USER_ID.to_owned(),
            AGENT_CURSOR_NAME.to_owned(),
            AGENT_CURSOR_COLOR.to_owned(),
        );
    }
    (
        format!("{AGENT_CURSOR_USER_ID}:{cursor_id}"),
        format!("{AGENT_CURSOR_NAME} {cursor_id}"),
        cursor_overlay::session_fill_hex(cursor_id),
    )
}

#[cfg(test)]
mod agent_cursor_identity_tests {
    use super::agent_cursor_identity;

    #[test]
    fn anonymous_cursor_keeps_the_reserved_agent_identity() {
        for id in ["", "default"] {
            let (user, name, color) = agent_cursor_identity(id);
            assert_eq!(user, "cua-agent");
            assert_eq!(name, "CUA agent");
            assert_eq!(color, "#3b82f6");
        }
    }

    /// The regression this function exists for: two driver sessions acting at
    /// once must not share a presence id, or one agent's move relocates the
    /// other's rendered cursor.
    #[test]
    fn distinct_sessions_get_distinct_ids_and_colors() {
        let (a_id, a_name, a_color) = agent_cursor_identity("crimson");
        let (b_id, b_name, b_color) = agent_cursor_identity("aqua");
        assert_ne!(a_id, b_id);
        assert_ne!(a_name, b_name);
        assert_ne!(a_color, b_color);
        assert_ne!(a_id, "cua-agent");
        // Stable across calls: a cursor must not change identity as it moves.
        assert_eq!(agent_cursor_identity("crimson").0, a_id);
        assert_eq!(agent_cursor_identity("crimson").2, a_color);
    }

    /// The color must be the same one the driver's own desktop overlay paints
    /// for that session, so both ends agree on who is who.
    #[test]
    fn color_matches_the_driver_desktop_overlay_fill() {
        assert_eq!(
            agent_cursor_identity("crimson").2,
            cursor_overlay::session_fill_hex("crimson")
        );
    }
}

#[derive(Clone)]
pub struct CuaTargetProvider(Arc<CuaProviderInner>);

#[derive(Clone)]
pub struct CuaCaptureProvider(Arc<CuaProviderInner>);

#[derive(Clone)]
pub struct CuaActionProvider(Arc<CuaProviderInner>);

#[derive(Clone)]
pub struct CuaInteractiveInputProvider(Arc<CuaProviderInner>);

#[derive(Clone)]
pub struct CuaAccessibilityProvider(Arc<CuaProviderInner>);

#[derive(Clone)]
pub struct CuaWindowGeometryProvider(Arc<CuaProviderInner>);

/// Whole-display capture targets.
#[derive(Clone)]
pub struct CuaDisplayProvider(#[allow(dead_code)] Arc<CuaProviderInner>);

const DISPLAY_KEY_PREFIX: &str = "display:";

/// Whether the desktop is a Hyprland (Wayland) session, which the X11
/// backends cannot see into (see [`linux_wayland`]).
#[cfg(target_os = "linux")]
pub(crate) fn wayland_session() -> bool {
    linux_wayland::active()
}

/// The Hyprland window manager (focus and geometry through `hyprctl`).
#[cfg(target_os = "linux")]
pub(crate) fn wayland_window_manager() -> Arc<dyn grpc::tool_backend::WindowManager> {
    Arc::new(linux_wayland::HyprlandWindowManager)
}

/// The Wayland session's text clipboard (wl-clipboard).
#[cfg(target_os = "linux")]
pub(crate) fn wayland_clipboard() -> Arc<dyn grpc::tool_backend::TextClipboard> {
    Arc::new(linux_wayland::WlClipboard)
}

/// The display id behind a display capture target, if it is one.
#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub(crate) fn display_key(target: &ProviderTargetId) -> Option<String> {
    target
        .key
        .provider_value()
        .strip_prefix(DISPLAY_KEY_PREFIX)
        .map(str::to_owned)
}

fn default_encoder() -> Arc<dyn cua_spacesd_session::media::encoder::FrameEncoderFactory> {
    Arc::new(codec_encoder::CodecEncoderFactory::default())
}

impl cua_spacesd_provider_api::DisplayProvider for CuaDisplayProvider {
    fn displays(&self) -> Result<Vec<cua_spacesd_provider_api::ProviderDisplay>, ProviderError> {
        #[cfg(target_os = "linux")]
        {
            if linux_wayland::active() {
                return Ok(linux_wayland::displays());
            }
            let (conn, root) = linux_x11::connect()?;
            Ok(linux_x11::displays(&conn, root))
        }
        #[cfg(target_os = "macos")]
        {
            Ok(macos_display::displays())
        }
        #[cfg(target_os = "windows")]
        {
            Ok(windows_display::displays())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        Err(ProviderError::new(
            ProviderErrorCode::Unsupported,
            "desktop (display) targets are implemented on Linux (X11 and Hyprland), macOS and Windows only; stream windows instead",
        ))
    }

    fn display_target(&self, display_id: &str) -> Result<ProviderTarget, ProviderError> {
        let displays = self.displays()?;
        let display = displays
            .iter()
            .find(|display| {
                display.id == display_id || (display_id == "primary" && display.primary)
            })
            .ok_or_else(|| {
                ProviderError::new(ProviderErrorCode::TargetUnavailable, "unknown display")
            })?;
        let id = ProviderTargetId {
            key: BackendTargetKey::new(format!("{DISPLAY_KEY_PREFIX}{}", display.id)),
            epoch: TargetEpoch(1),
        };
        Ok(ProviderTarget {
            id,
            descriptor: WindowDescriptor {
                window: TargetHandle(format!("display-{}", display.id)),
                target_epoch: TargetEpoch(1),
                app_name: "Desktop".into(),
                title: display.name.clone(),
                geometry: SurfaceGeometry {
                    width_px: display.native_width_px,
                    height_px: display.native_height_px,
                    scale_factor: display.scale_factor,
                },
                visible: true,
            },
            grant: None,
        })
    }
}

/// Renders every connected remote user as a colored overlay cursor on the
/// host desktop. Native window identifiers stay inside the adapter: callers
/// address windows by opaque `TargetHandle` and window-local frame pixels.
#[derive(Clone)]
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub struct CuaPresenceOverlay(Arc<CuaProviderInner>);

/// Launches configured applications through the CUA `launch_app` tool, which
/// guarantees the launched window does not take foreground focus.
#[derive(Clone)]
pub struct CuaAppLauncher(Arc<CuaProviderInner>);

pub struct CuaProviderBundle {
    pub targets: Arc<CuaTargetProvider>,
    pub captures: Arc<CuaCaptureProvider>,
    pub actions: Arc<CuaActionProvider>,
    pub inputs: Arc<CuaInteractiveInputProvider>,
    pub accessibility: Arc<CuaAccessibilityProvider>,
    pub geometry: Arc<CuaWindowGeometryProvider>,
    pub presence: Arc<CuaPresenceOverlay>,
    pub displays: Arc<CuaDisplayProvider>,
    pub launcher: Arc<CuaAppLauncher>,
    /// The embedded cua-driver tool registry, so an embedder (cua-spacesd) can serve
    /// cua-driver's own MCP over it — driving THIS process's cua-driver, whose
    /// cursor moves fire the cursor hook.
    pub registry: Arc<cua_driver_core::tool::ToolRegistry>,
}

/// Stable, server-side application selection. The `Id` value maps to a
/// bundle identifier on macOS, an application identity/AUMID on Windows, and
/// a desktop application ID on Linux. It is never accepted as a window target
/// or serialized inside a `WindowDescriptor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApplicationSelector {
    Id(String),
    Name(String),
}

#[derive(Clone, Default)]
pub struct CuaProviderConfig {
    pub application: Option<ApplicationSelector>,
    /// Sink for the CUA agent's cursor: each window action emits a `CursorState`
    /// (window-local frame pixels) so the daemon can broadcast it to viewers as
    /// a `RemoteCursor`, letting clients render the agent cursor as an overlay
    /// without streaming the driver's on-screen cursor into the video.
    pub agent_cursor: Option<tokio::sync::mpsc::UnboundedSender<cua_media_protocol::CursorState>>,
    /// Encoder for CPU-captured frames. `None` selects the built-in default
    /// (`cua-media-codec`: probed fastest-first with runtime fallback).
    pub encoder: Option<Arc<dyn cua_spacesd_session::media::encoder::FrameEncoderFactory>>,
}

impl std::fmt::Debug for CuaProviderConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CuaProviderConfig")
            .field("application", &self.application)
            .field("agent_cursor", &self.agent_cursor.is_some())
            .field(
                "encoder",
                &self.encoder.as_ref().map(|encoder| encoder.name()),
            )
            .finish()
    }
}

impl CuaProviderBundle {
    /// Native (pid, window id) behind a catalog target. Crate-internal: the
    /// desktop backend joins catalog handles with native window facts.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub(crate) fn native_of(&self, target: &ProviderTargetId) -> Option<(i64, u64)> {
        self.targets
            .0
            .native(target)
            .ok()
            .map(|native| (native.pid, native.window_id))
    }

    pub fn new() -> Result<Self, ProviderError> {
        Self::with_config(CuaProviderConfig::default())
    }

    pub fn with_config(config: CuaProviderConfig) -> Result<Self, ProviderError> {
        if matches!(
            config.application.as_ref(),
            Some(ApplicationSelector::Id(value) | ApplicationSelector::Name(value)) if value.is_empty()
        ) {
            return Err(ProviderError::new(
                ProviderErrorCode::Internal,
                "application selector must not be empty",
            ));
        }
        // Build under the runtime scope the registry's own invocations run in
        // (the process authorization's legacy context), as cua-driver-sdk
        // does: element caches remember the scope they were created in, and
        // a mismatch refuses every element token they hand out.
        let scope = cua_driver_core::session_authorization::configured_registry()
            .and_then(|registry| registry.legacy_context())
            .map(|context| context.runtime_scope_key());
        let registry = match scope {
            Ok(scope) => cua_driver_core::tool::with_runtime_scope(scope, platform_registry)?,
            Err(error) => {
                tracing::warn!(%error, "no driver authorization context; element tokens use the legacy scope");
                platform_registry()?
            }
        };
        let registry = Arc::new(registry);
        registry.init_self_weak();
        let inner = Arc::new(CuaProviderInner {
            catalog: Mutex::new(TargetCatalog::default()),
            registry,
            next_snapshot: AtomicU32::new(1),
            coordinate_session_configured: AtomicBool::new(false),
            human_sessions_configured: Mutex::new(std::collections::HashSet::new()),
            native_pixel_geometries: Mutex::new(HashMap::new()),
            application: config.application,
            cursor_pressed: Mutex::new(HashMap::new()),
            agent_cursor: config.agent_cursor,
            encoder: config.encoder.unwrap_or_else(default_encoder),
        });
        // When an agent-cursor sink is wired, register the process-wide cua-driver
        // cursor hook so every cursor move made by this process's cua-driver is
        // mapped to window-local coords and forwarded to viewers.
        if inner.agent_cursor.is_some() {
            let hook_inner = inner.clone();
            cua_driver_core::cursor_hook::set_cursor_hook_fn(move |ev| {
                hook_inner.on_cursor_event(ev)
            });
        }
        let registry = inner.registry.clone();
        Ok(Self {
            targets: Arc::new(CuaTargetProvider(inner.clone())),
            captures: Arc::new(CuaCaptureProvider(inner.clone())),
            actions: Arc::new(CuaActionProvider(inner.clone())),
            inputs: Arc::new(CuaInteractiveInputProvider(inner.clone())),
            accessibility: Arc::new(CuaAccessibilityProvider(inner.clone())),
            geometry: Arc::new(CuaWindowGeometryProvider(inner.clone())),
            presence: Arc::new(CuaPresenceOverlay(inner.clone())),
            displays: Arc::new(CuaDisplayProvider(inner.clone())),
            launcher: Arc::new(CuaAppLauncher(inner)),
            registry,
        })
    }
}

impl InteractiveInputProvider for CuaInteractiveInputProvider {
    /// Opens a cua-driver interactive input session for the target. The
    /// spacesd only resolves the target and maps the session policy; the
    /// native session (and all input injection) belongs to cua-driver.
    ///
    /// Without an owner the input is a viewer's: media input is streamed by
    /// a person watching the target, while agents act through one-shot
    /// actions.
    fn open(
        &self,
        target: &ProviderTargetId,
        policy: SessionPolicy,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        self.open_as(
            target,
            policy,
            &cua_spacesd_provider_api::InputOwner {
                id: "anonymous".into(),
                name: "Anonymous".into(),
                agent: false,
            },
        )
    }

    /// Opens for `owner`. A human's input never draws cua-driver's agent
    /// cursor or reaches presence as an agent: on Hyprland its tool calls
    /// run in a human-origin driver session; the X11 and macOS interactive
    /// sessions inject without the overlay or the cursor hook.
    fn open_as(
        &self,
        target: &ProviderTargetId,
        policy: SessionPolicy,
        owner: &cua_spacesd_provider_api::InputOwner,
    ) -> Result<Option<Arc<dyn InteractiveInputLease>>, ProviderError> {
        #[cfg(not(target_os = "linux"))]
        let _ = owner;
        let Some(delivery_mode) = driver_input::interactive_mode(policy) else {
            // Still resolve the target so a stale handle reports as such.
            if display_key(target).is_none() {
                self.0.native(target)?;
            }
            return Ok(None);
        };

        #[cfg(target_os = "linux")]
        {
            use platform_linux::input::interactive::{
                InteractiveInputConfig, InteractiveInputSession,
            };
            if linux_wayland::active() {
                // cua-driver has no stateful Hyprland session: the lease folds
                // each batch into gestures for the driver's tools. A display
                // target is never in the window catalog; resolve it as a
                // display.
                let inner = self.0.clone();
                let lease = tool_input::open(
                    target,
                    delivery_mode,
                    |target| {
                        let native = inner.native(target)?;
                        if delivery_mode == cua_driver_core::interactive_input::InteractiveDeliveryMode::Background {
                            hyprland_background_admission(native)?;
                        }
                        let descriptor = inner
                            .catalog
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .by_native
                            .get(&native)
                            .map(|entry| entry.target.descriptor.geometry.clone());
                        let geometry = inner.clone();
                        Ok(tool_input::ToolInputTarget::Window {
                            pid: native.pid,
                            window_id: native.window_id,
                            extent: Box::new(move || {
                                geometry
                                    .native_pixel_geometry(native)
                                    .map(|pixels| (pixels.width_px, pixels.height_px))
                                    .or_else(|| {
                                        descriptor
                                            .as_ref()
                                            .map(|geometry| (geometry.width_px, geometry.height_px))
                                    })
                            }),
                        })
                    },
                    |display_id| {
                        linux_wayland::displays()
                            .into_iter()
                            .find(|display| display.id == display_id)
                            .ok_or_else(|| {
                                ProviderError::new(
                                    ProviderErrorCode::TargetUnavailable,
                                    "display is gone",
                                )
                            })
                    },
                    Box::new(linux_wayland::focused_window),
                    Arc::new(RegistryGestureTools {
                        inner: self.0.clone(),
                        human_session: (!owner.agent).then(|| human_input_session(&owner.id)),
                    }),
                )?;
                tracing::info!(
                    target: "cua_spacesd_client::host_input",
                    target = %target.key.provider_value(),
                    ?delivery_mode,
                    "opened cua-driver tool input session (Hyprland)"
                );
                return Ok(Some(Arc::new(lease)));
            }
            let config = if let Some(display_id) = display_key(target) {
                let (conn, root) = linux_x11::connect()?;
                let display = linux_x11::displays(&conn, root)
                    .into_iter()
                    .find(|display| display.id == display_id)
                    .ok_or_else(|| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "display is gone")
                    })?;
                let (x, y, width, height) = display.bounds;
                InteractiveInputConfig {
                    window: None,
                    region: Some((x as i32, y as i32, width as u32, height as u32)),
                    delivery_mode,
                }
            } else {
                let native = self.0.native(target)?;
                InteractiveInputConfig {
                    window: Some(u32::try_from(native.window_id).map_err(|_| {
                        ProviderError::new(
                            ProviderErrorCode::TargetUnavailable,
                            "native window identifier is outside the X11 range",
                        )
                    })?),
                    region: None,
                    delivery_mode,
                }
            };
            let session =
                InteractiveInputSession::open(config).map_err(driver_input::interactive_error)?;
            Ok(Some(Arc::new(driver_input::LinuxLease(session))))
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(display_id) = display_key(target) {
                // A whole-display stream (every viewer's desktop view): the
                // session posts through the HID stream at the display's
                // global points, so a click lands on the Dock, the menu bar
                // or whatever window is under it. A display has no window to
                // address background input to: `background_only` is refused
                // as would-require-activation, the same as on Linux.
                let display = macos_display::displays()
                    .into_iter()
                    .find(|display| display.id == display_id)
                    .ok_or_else(|| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "display is gone")
                    })?;
                let (x, y, width, height) = display.bounds;
                let session = platform_macos::input::InteractiveInputSession::open(
                    platform_macos::input::InteractiveInputConfig {
                        delivery_mode,
                        ..platform_macos::input::InteractiveInputConfig::display(
                            x, y, width, height,
                        )
                    },
                )
                .map_err(driver_input::interactive_error)?;
                tracing::info!(
                    target: "cua_spacesd_client::host_input",
                    display = %display_id,
                    ?delivery_mode,
                    "opened cua-driver interactive input session on a display"
                );
                return Ok(Some(Arc::new(driver_input::MacosLease(session))));
            }
            let native = self.0.native(target)?;
            let pid = i32::try_from(native.pid).map_err(|_| {
                ProviderError::new(
                    ProviderErrorCode::TargetUnavailable,
                    "native process identifier is outside the macOS range",
                )
            })?;
            let window_id = u32::try_from(native.window_id).map_err(|_| {
                ProviderError::new(
                    ProviderErrorCode::TargetUnavailable,
                    "native window identifier is outside the macOS range",
                )
            })?;
            let session = platform_macos::input::InteractiveInputSession::open(
                platform_macos::input::InteractiveInputConfig {
                    pid,
                    window_id,
                    region: None,
                    delivery_mode,
                    queue_capacity: 32,
                },
            )
            .map_err(driver_input::interactive_error)?;
            tracing::info!(
                target: "cua_spacesd_client::host_input",
                pid,
                window_id,
                ?delivery_mode,
                queue_capacity = 32,
                "opened cua-driver interactive input session"
            );
            Ok(Some(Arc::new(driver_input::MacosLease(session))))
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // cua-driver has no Windows interactive session yet; one-shot
            // actions go through its tools (desktop scope for a display).
            let _ = delivery_mode;
            if display_key(target).is_none() {
                self.0.native(target)?;
            }
            Ok(None)
        }
    }
}

impl WindowGeometryProvider for CuaWindowGeometryProvider {
    fn supports(&self, target: &ProviderTargetId) -> Result<bool, ProviderError> {
        self.0.native(target)?;
        Ok(cfg!(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux"
        )))
    }

    fn resize<'a>(
        &'a self,
        target: &'a ProviderTargetId,
        width_points: u32,
        height_points: u32,
    ) -> ProviderFuture<'a, Result<AppliedWindowGeometry, ProviderError>> {
        Box::pin(async move {
            let native = self.0.native(target)?;
            resize_native_window(native, width_points, height_points).await
        })
    }
}

/// Publish the origin-desktop overlay's per-platform status exactly once.
///
/// The cross-platform contract in AGENTS.md requires that a platform which
/// cannot meet the contract *publish that limitation* rather than substitute
/// misleading behaviour. Before this, `update_cursor` ended in a bare
/// `let _ = (..)` on every non-Windows platform: presence cursors were
/// accepted, acknowledged, and silently dropped, so an operator had no way to
/// tell a working overlay from an absent one.
fn announce_desktop_overlay_support() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        #[cfg(target_os = "windows")]
        tracing::info!(
            platform = "windows",
            "origin-desktop cursor overlay active (Cua.AgentCursorOverlay)"
        );
        #[cfg(target_os = "macos")]
        tracing::info!(
            platform = "macos",
            shape_reporting = cua_driver_core::cursor_shape::cursor_shape_supported(),
            "origin-desktop cursor overlay active (AppKit overlay window on the \
             daemon's main thread)"
        );
        #[cfg(target_os = "linux")]
        tracing::warn!(
            platform = "linux",
            "origin-desktop cursor overlay UNAVAILABLE: remote and agent cursors \
             are rendered in the per-window streams only. cursor-overlay has X11 \
             and partial Wayland backends, but this provider has no \
             window-local-to-screen mapping outside macOS/Windows, so a presence \
             point cannot be placed on the desktop."
        );
    });
}

impl CuaPresenceOverlay {
    /// Start the desktop overlay renderer. Safe to call more than once.
    ///
    /// On platforms without a desktop overlay this publishes that fact once
    /// (see [`announce_desktop_overlay_support`]) instead of appearing to
    /// succeed.
    pub fn activate(&self) {
        announce_desktop_overlay_support();
        #[cfg(target_os = "windows")]
        windows_support::activate_overlay();
    }

    /// Whether this build renders cursors on the *origin* desktop (the machine
    /// whose windows are being streamed out), as opposed to only inside the
    /// per-window streams. Lets callers and tests assert the real capability
    /// rather than infer it from a call that cannot fail.
    pub fn renders_on_origin_desktop(&self) -> bool {
        // macOS renders since cua-spacesd hands its main thread to the AppKit
        // overlay loop; Windows renders from the overlay's own STA thread.
        // Linux has no window-local-to-screen mapping in this provider yet.
        cfg!(any(target_os = "windows", target_os = "macos"))
    }

    /// Show or move one user's desktop cursor. Coordinates are window-local
    /// frame pixels for the given opaque target.
    pub fn update_cursor(
        &self,
        user_key: &str,
        window: Option<&TargetHandle>,
        x: f64,
        y: f64,
        visible: bool,
        pressed: bool,
    ) {
        #[cfg(target_os = "windows")]
        {
            let native = window.and_then(|handle| {
                let catalog = self
                    .0
                    .catalog
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                catalog.by_handle.get(handle).copied()
            });
            let screen = native
                .filter(|_| visible)
                .and_then(|native| windows_support::frame_point_to_screen(native.window_id, x, y));
            match screen {
                Some((screen_x, screen_y)) => {
                    windows_support::overlay_set_visible(user_key, true);
                    if let Some(native) = native {
                        windows_support::overlay_pin_above(user_key, native.window_id);
                    }
                    windows_support::overlay_move(user_key, screen_x, screen_y);
                    let was_pressed = {
                        let mut pressed_states = self
                            .0
                            .cursor_pressed
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        pressed_states.insert(user_key.to_owned(), pressed) == Some(true)
                    };
                    if pressed != was_pressed {
                        windows_support::overlay_set_pressed(user_key, pressed, screen_x, screen_y);
                    }
                }
                None => windows_support::overlay_set_visible(user_key, false),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (user_key, window, x, y, visible, pressed);
        }
    }

    /// Current screen position of one user's desktop overlay cursor, when
    /// the overlay renders it. Test and diagnostic seam: overlay windows are
    /// excluded from ordinary screen captures, so verification reads the
    /// render state instead of pixels.
    pub fn cursor_screen_position(&self, user_key: &str) -> Option<(f64, f64)> {
        #[cfg(target_os = "windows")]
        {
            if platform_windows::overlay::is_enabled(user_key) {
                return Some(platform_windows::overlay::current_position(user_key));
            }
            None
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = user_key;
            None
        }
    }

    /// The host's physical cursor as an opaque window handle plus
    /// window-local frame coordinates, when the pointer is over a window the
    /// catalog knows. Lets clients render the host user's cursor without the
    /// daemon revealing native identifiers.
    pub fn host_cursor(&self) -> Option<(TargetHandle, f64, f64, bool)> {
        #[cfg(target_os = "windows")]
        {
            let (pid, hwnd, x, y, pressed) = windows_support::host_cursor()?;
            let native = NativeTarget {
                pid: i64::from(pid),
                window_id: hwnd,
            };
            let catalog = self
                .0
                .catalog
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let entry = catalog
                .by_native
                .get(&native)
                .filter(|entry| entry.active)?;
            Some((entry.target.descriptor.window.clone(), x, y, pressed))
        }
        #[cfg(not(target_os = "windows"))]
        None
    }

    /// Remove one user's desktop cursor entirely (connection closed).
    pub fn remove_cursor(&self, user_key: &str) {
        #[cfg(target_os = "windows")]
        {
            self.0
                .cursor_pressed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(user_key);
            windows_support::overlay_remove(user_key);
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = user_key;
        }
    }
}

impl CuaAppLauncher {
    /// Launch an executable in the background and return the launched
    /// window's opaque descriptor when one materialized in time.
    pub async fn launch(
        &self,
        path: &str,
        args: &[String],
    ) -> Result<Option<cua_media_protocol::WindowDescriptor>, ProviderError> {
        // The native launch joins arguments with spaces into one
        // ShellExecuteEx parameter string, so values containing whitespace
        // need explicit quoting to survive as single arguments.
        let quoted: Vec<String> = args
            .iter()
            .map(|arg| {
                if arg.contains(char::is_whitespace) && !arg.starts_with('"') {
                    format!("\"{arg}\"")
                } else {
                    arg.clone()
                }
            })
            .collect();
        let result = self
            .0
            .registry
            .invoke(
                "launch_app",
                serde_json::json!({ "path": path, "additional_arguments": quoted }),
            )
            .await;
        if result.is_error == Some(true) {
            return Err(ProviderError::new(
                ProviderErrorCode::DeliveryFailed,
                tool_result_text(&result),
            ));
        }
        let structured = result
            .structured_content
            .clone()
            .or_else(|| tool_result_json(&result));
        let launched_hwnd = structured
            .as_ref()
            .and_then(|value| value.get("windows"))
            .and_then(|windows| windows.get(0))
            .and_then(|window| window.get("window_id"))
            .and_then(serde_json::Value::as_u64);
        let Some(launched_hwnd) = launched_hwnd else {
            return Ok(None);
        };
        // Refresh the catalog so the launched window gets an opaque handle,
        // then return that descriptor without leaking the native id.
        let targets = self.0.refresh(false)?;
        let catalog = self
            .0
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        Ok(targets
            .into_iter()
            .find(|target| {
                catalog
                    .by_provider_id
                    .get(&target.id)
                    .is_some_and(|native| native.window_id == launched_hwnd)
            })
            .map(|target| target.descriptor))
    }
}

impl TargetProvider for CuaTargetProvider {
    fn enumerate(&self, query: &TargetQuery) -> Result<Vec<ProviderTarget>, ProviderError> {
        self.0.refresh(query.on_screen_only)
    }

    fn pick(&self, _request: &PickTargetRequest) -> Result<ProviderTarget, ProviderError> {
        // Linux has no interactive system picker, so "pick" resolves to the
        // most prominent visible window (largest on-screen area). This lets a
        // client open a session without first enumerating and choosing a
        // handle, mirroring the `--pick` client flow.
        #[cfg(target_os = "linux")]
        #[allow(clippy::needless_return)] // a cfg-gated early return, not a tail
        {
            let targets = self.0.refresh(true)?;
            return targets
                .into_iter()
                .filter(|target| target.descriptor.visible)
                .max_by_key(|target| {
                    u64::from(target.descriptor.geometry.width_px)
                        * u64::from(target.descriptor.geometry.height_px)
                })
                .ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorCode::TargetUnavailable,
                        "no visible window is available to pick",
                    )
                });
        }
        #[cfg(not(target_os = "linux"))]
        Err(ProviderError::new(
            ProviderErrorCode::Unsupported,
            "the CUA macOS adapter does not yet expose a system target picker; enumerate targets instead",
        ))
    }

    fn restore(&self, grant: &TargetGrant) -> Result<ProviderTarget, ProviderError> {
        self.0.refresh(false)?;
        self.0
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .restore(grant)
    }

    fn resolve(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<ProviderTarget, ProviderError> {
        self.0.refresh(false)?;
        self.0
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resolve(handle, epoch)
    }

    fn app_icon(
        &self,
        handle: &TargetHandle,
        epoch: TargetEpoch,
    ) -> Result<Option<ProviderAppIcon>, ProviderError> {
        self.0.refresh(false)?;
        let target = self
            .0
            .catalog
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .resolve(handle, epoch)?;
        let native = self.0.native(&target.id)?;
        #[cfg(target_os = "macos")]
        return macos_app_icon(native);
        #[cfg(not(target_os = "macos"))]
        {
            let _ = native;
            Ok(None)
        }
    }
}

impl CaptureProvider for CuaCaptureProvider {
    fn formats(&self, target: &ProviderTargetId) -> Result<Vec<PixelFormat>, ProviderError> {
        #[cfg(target_os = "windows")]
        if display_key(target).is_some() {
            return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        }
        #[cfg(target_os = "linux")]
        if display_key(target).is_some() {
            // X11: the XShm grabber; Hyprland: screencopy of the monitor region.
            // Both encode H.264 through cua-media-codec.
            return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        }
        #[cfg(target_os = "macos")]
        if display_key(target).is_some() {
            return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        }
        self.0.native(target)?;
        #[cfg(target_os = "macos")]
        return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        #[cfg(target_os = "windows")]
        return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        // Linux encodes H.264 through cua-media-codec (X11 in its capture loop,
        // the Hyprland/Wayland grabber behind an encoding pipeline) and keeps
        // packed BGRA as the fallback.
        #[cfg(target_os = "linux")]
        return Ok(vec![PixelFormat::H264AnnexB, PixelFormat::Bgra8]);
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        Err(ProviderError::new(
            ProviderErrorCode::Unsupported,
            "capture is not implemented on this operating system",
        ))
    }

    fn start(
        &self,
        target: &ProviderTargetId,
        config: &CaptureConfig,
        sink: Arc<dyn CaptureSink>,
    ) -> Result<Arc<dyn CaptureLease>, ProviderError> {
        #[cfg(target_os = "linux")]
        if linux_wayland::active() {
            if let Some(display_id) = display_key(target) {
                let display = linux_wayland::displays()
                    .into_iter()
                    .find(|display| display.id == display_id)
                    .ok_or_else(|| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "display is gone")
                    })?;
                let (x, y, width, height) = display.bounds;
                let bounds = (x as i32, y as i32, width as u32, height as u32);
                if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                    return encoded_capture::start_encoded(
                        self.0.encoder.clone(),
                        config,
                        sink,
                        move |config, sink| linux_wayland::start_region(bounds, config, sink),
                    );
                }
                return linux_wayland::start_region(bounds, config, sink);
            }
        }
        #[cfg(target_os = "linux")]
        if !linux_wayland::active() {
            let encoder = config
                .accepted_formats
                .contains(&PixelFormat::H264AnnexB)
                .then(|| self.0.encoder.clone());
            if let Some(display_id) = display_key(target) {
                let (conn, root) = linux_x11::connect()?;
                let display = linux_x11::displays(&conn, root)
                    .into_iter()
                    .find(|display| display.id == display_id)
                    .ok_or_else(|| {
                        ProviderError::new(ProviderErrorCode::TargetUnavailable, "display is gone")
                    })?;
                let (x, y, width, height) = display.bounds;
                return linux_stream::start(
                    linux_stream::StreamSource::Display {
                        x: x as i16,
                        y: y as i16,
                        width: width as u16,
                        height: height as u16,
                    },
                    config,
                    sink,
                    encoder,
                );
            }
            let native = self.0.native(target)?;
            let window = u32::try_from(native.window_id).map_err(|_| {
                ProviderError::new(
                    ProviderErrorCode::TargetUnavailable,
                    "window id out of range",
                )
            })?;
            if let Ok((conn, _)) = linux_x11::connect() {
                if let Some(geometry) =
                    x11rb::protocol::xproto::ConnectionExt::get_geometry(&conn, window)
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                {
                    self.0.set_native_pixel_geometry(
                        native,
                        u32::from(geometry.width),
                        u32::from(geometry.height),
                    );
                }
            }
            return linux_stream::start(
                linux_stream::StreamSource::Window(window),
                config,
                sink,
                encoder,
            );
        }
        #[cfg(target_os = "macos")]
        if let Some(display_id) = display_key(target) {
            let display_id = macos_display::parse_id(&display_id).ok_or_else(|| {
                ProviderError::new(ProviderErrorCode::TargetUnavailable, "unknown display")
            })?;
            let source = macos_capture::CaptureSource::Display(display_id);
            // Display targets have no window geometry to record.
            let native_geometry = Arc::new(|_: u32, _: u32| {}) as NativeGeometryCallback;
            if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                let encoder = macos_h264::MacosH264Encoder::start(
                    config.max_fps,
                    config.target_bitrate_kbps,
                    sink,
                )?;
                let capture =
                    macos_capture::start_h264(source, config, encoder.clone(), native_geometry)?;
                return Ok(Arc::new(MacosEncodedCaptureLease { capture, encoder }));
            }
            if !config.accepted_formats.contains(&PixelFormat::Bgra8) {
                return Err(ProviderError::new(
                    ProviderErrorCode::Unsupported,
                    "the CUA adapter cannot provide any requested capture format",
                ));
            }
            return macos_capture::start(source, config, sink, native_geometry);
        }
        #[cfg(target_os = "windows")]
        if let Some(display_id) = display_key(target) {
            // Whole display: the GDI loop, through the same H.264 encoder as
            // window streams when the client takes H.264.
            if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                let encoder = windows_h264::WindowsH264Encoder::start(
                    config.max_fps,
                    config.target_bitrate_kbps,
                    sink,
                )?;
                let capture = windows_display::start(&display_id, config.max_fps, encoder.clone())?;
                return Ok(Arc::new(WindowsEncodedCaptureLease { capture, encoder }));
            }
            if !config.accepted_formats.contains(&PixelFormat::Bgra8) {
                return Err(ProviderError::new(
                    ProviderErrorCode::Unsupported,
                    "the CUA adapter cannot provide any requested capture format",
                ));
            }
            return windows_display::start(&display_id, config.max_fps, sink);
        }
        let native = self.0.native(target)?;
        let native_geometry = {
            let inner = self.0.clone();
            Arc::new(move |width_px, height_px| {
                inner.set_native_pixel_geometry(native, width_px, height_px);
            }) as NativeGeometryCallback
        };
        #[cfg(target_os = "macos")]
        {
            if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                let encoder = macos_h264::MacosH264Encoder::start(
                    config.max_fps,
                    config.target_bitrate_kbps,
                    sink,
                )?;
                let capture = macos_capture::start_h264(
                    macos_capture::CaptureSource::Window(native),
                    config,
                    encoder.clone(),
                    native_geometry,
                )?;
                return Ok(Arc::new(MacosEncodedCaptureLease { capture, encoder }));
            }
        }
        #[cfg(target_os = "windows")]
        {
            if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                let encoder = windows_h264::WindowsH264Encoder::start(
                    config.max_fps,
                    config.target_bitrate_kbps,
                    sink,
                )?;
                let capture =
                    start_platform_capture(native, config, encoder.clone(), native_geometry)?;
                return Ok(Arc::new(WindowsEncodedCaptureLease { capture, encoder }));
            }
        }
        #[cfg(target_os = "linux")]
        {
            if config.accepted_formats.contains(&PixelFormat::H264AnnexB) {
                return encoded_capture::start_encoded(
                    self.0.encoder.clone(),
                    config,
                    sink,
                    move |config, sink| {
                        start_platform_capture(native, config, sink, native_geometry)
                    },
                );
            }
        }
        if !config.accepted_formats.contains(&PixelFormat::Bgra8) {
            return Err(ProviderError::new(
                ProviderErrorCode::Unsupported,
                "the CUA adapter cannot provide any requested capture format",
            ));
        }
        start_platform_capture(native, config, sink, native_geometry)
    }
}

#[cfg(target_os = "macos")]
struct MacosEncodedCaptureLease {
    capture: Arc<dyn CaptureLease>,
    encoder: Arc<macos_h264::MacosH264Encoder>,
}

#[cfg(target_os = "macos")]
impl CaptureLease for MacosEncodedCaptureLease {
    fn request_keyframe(&self) {
        self.encoder.request_keyframe();
    }

    fn stop(&self) {
        self.capture.stop();
        self.encoder.stop();
    }
}

#[cfg(target_os = "windows")]
struct WindowsEncodedCaptureLease {
    capture: Arc<dyn CaptureLease>,
    encoder: Arc<windows_h264::WindowsH264Encoder>,
}

#[cfg(target_os = "windows")]
impl CaptureLease for WindowsEncodedCaptureLease {
    fn request_keyframe(&self) {
        self.encoder.request_keyframe();
    }

    fn stop(&self) {
        self.capture.stop();
        self.encoder.stop();
    }
}

impl ActionProvider for CuaActionProvider {
    fn capabilities(
        &self,
        target: &ProviderTargetId,
    ) -> Result<Vec<ActionCapability>, ProviderError> {
        let native = self.0.native(target)?;
        // Linux delivers input directly through the X11/XTest backend rather
        // than a CUA tool registry, so advertise the actions that backend can
        // inject. Without this the session lists no actions and cua-spacesd rejects
        // every click and key as "not advertised for this target".
        #[cfg(target_os = "linux")]
        #[allow(clippy::needless_return)] // a cfg-gated early return, not a tail
        {
            let _ = native;
            return Ok(LINUX_INPUT_ACTIONS
                .iter()
                .map(|action| ActionCapability {
                    action: (*action).to_owned(),
                    guarantee: action_guarantee(action),
                })
                .collect());
        }
        #[cfg(not(target_os = "linux"))]
        Ok(self
            .0
            .registry
            .tool_names()
            .map(|action| ActionCapability {
                action: action.to_owned(),
                guarantee: action_guarantee_for_native(action, native),
            })
            .collect())
    }

    fn perform<'a>(
        &'a self,
        target: &'a ProviderTargetId,
        action: ActionInvocation,
        policy: SessionPolicy,
    ) -> ProviderFuture<'a, Result<ActionOutcome, ProviderError>> {
        Box::pin(async move {
            let native = self.0.native(target)?;

            // Linux X11 input goes through cua-driver's targeted delivery
            // (`platform_linux::input::targeted`), which owns the X11
            // background/foreground semantics. Wayland and the other platforms
            // use the cua-driver tool registry below.
            #[cfg(target_os = "linux")]
            if !linux_wayland::active() {
                return linux_perform_action(
                    native,
                    action,
                    policy,
                    self.0.native_pixel_geometry(native),
                );
            }

            {
                self.0.ensure_coordinate_session().await?;
                let capability = ActionCapability {
                    action: action.action.clone(),
                    guarantee: action_guarantee_for_native(&action.action, native),
                };
                enforce_action_policy(policy, &capability)?;
                let mut arguments = action.arguments.as_object().cloned().ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorCode::DeliveryFailed,
                        "CUA action arguments must be a JSON object",
                    )
                })?;
                apply_action_delivery_mode(&mut arguments, &action.action, policy);
                apply_interactive_action_hints(&mut arguments, &action.action);
                let coordinate_pairs = pixel_coordinate_pairs(&action.action);
                if coordinate_pairs
                    .iter()
                    .any(|(x, y)| arguments.contains_key(*x) || arguments.contains_key(*y))
                {
                    let source = action.coordinate_space.as_ref().ok_or_else(|| {
                        ProviderError::new(
                            ProviderErrorCode::DeliveryFailed,
                            "pixel action is missing its RCDP video coordinate space",
                        )
                    })?;
                    let native_geometry =
                        self.0.native_pixel_geometry(native).ok_or_else(|| {
                            ProviderError::new(
                                ProviderErrorCode::TargetUnavailable,
                                "native pixel geometry is unavailable for this capture",
                            )
                        })?;
                    scale_pixel_arguments(
                        &mut arguments,
                        coordinate_pairs,
                        source,
                        native_geometry,
                    )?;
                }
                persist_foreground_for_input(&self.0.registry, native, &action.action, policy)
                    .await?;
                // The agent cursor is surfaced to viewers by the cua-driver cursor
                // hook (see on_cursor_event / cursor_hook), not synthesized here.
                arguments.insert("pid".into(), native.pid.into());
                arguments.insert("window_id".into(), native.window_id.into());
                arguments.insert(
                    "_session_id".into(),
                    CUA_PROVIDER_SESSION_ID.to_owned().into(),
                );
                // The pixels were just scaled into the capture's native
                // window geometry, not into a driver screenshot this session
                // read, so they take the in-process native-pixel entry.
                let result = self
                    .0
                    .registry
                    .invoke_with_native_window_pixels(
                        &action.action,
                        serde_json::Value::Object(arguments),
                    )
                    .await;
                if result.is_error == Some(true) {
                    return Err(ProviderError::new(
                        ProviderErrorCode::DeliveryFailed,
                        tool_result_text(&result),
                    ));
                }
                Ok(ActionOutcome {
                    delivered: true,
                    detail: result.structured_content,
                })
            }
        })
    }
}

/// Deliver one RCDP action to an X11 window through cua-driver's targeted
/// delivery.
///
/// Reuses the shared, platform-independent pixel scaling to map the streamed
/// video-frame coordinates into the target window's local pixel space, then
/// hands the window id and scaled arguments to the cua-driver input path. Native
/// identifiers are passed out of band (never inside the argument JSON, which
/// cua-spacesd rejects), so the guest's X window is addressed by its id alone.
#[cfg(target_os = "linux")]
fn linux_perform_action(
    native: NativeTarget,
    action: ActionInvocation,
    policy: SessionPolicy,
    native_pixel_geometry: Option<NativePixelGeometry>,
) -> Result<ActionOutcome, ProviderError> {
    let capability = ActionCapability {
        action: action.action.clone(),
        guarantee: action_guarantee(&action.action),
    };
    enforce_action_policy(policy, &capability)?;

    let mut arguments = action.arguments.as_object().cloned().ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            "CUA action arguments must be a JSON object",
        )
    })?;

    let coordinate_pairs = pixel_coordinate_pairs(&action.action);
    if coordinate_pairs
        .iter()
        .any(|(x, y)| arguments.contains_key(*x) || arguments.contains_key(*y))
    {
        let source = action.coordinate_space.as_ref().ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::DeliveryFailed,
                "pixel action is missing its RCDP video coordinate space",
            )
        })?;
        let native_geometry = native_pixel_geometry.ok_or_else(|| {
            ProviderError::new(
                ProviderErrorCode::TargetUnavailable,
                "native pixel geometry is unavailable for this capture",
            )
        })?;
        scale_pixel_arguments(&mut arguments, coordinate_pairs, source, native_geometry)?;
    }

    linux_capture::deliver_action(native.window_id, &action.action, &arguments, policy)
}

impl AccessibilityProvider for CuaAccessibilityProvider {
    fn snapshot<'a>(
        &'a self,
        target: &'a ProviderTargetId,
    ) -> ProviderFuture<'a, Result<AccessibilitySnapshot, ProviderError>> {
        Box::pin(async move {
            let native = self.0.native(target)?;
            self.0.ensure_coordinate_session().await?;
            let result = self
                .0
                .registry
                .invoke(
                    "get_window_state",
                    serde_json::json!({
                        "pid": native.pid,
                        "window_id": native.window_id,
                        "_session_id": CUA_PROVIDER_SESSION_ID,
                    }),
                )
                .await;
            if result.is_error == Some(true) {
                return Err(ProviderError::new(
                    ProviderErrorCode::DeliveryFailed,
                    tool_result_text(&result),
                ));
            }
            let state = result
                .structured_content
                .clone()
                .or_else(|| tool_result_json(&result))
                .ok_or_else(|| {
                    ProviderError::new(
                        ProviderErrorCode::DeliveryFailed,
                        "CUA get_window_state returned no structured state",
                    )
                })?;
            Ok(AccessibilitySnapshot {
                snapshot_id: AccessibilitySnapshotId(
                    self.0.next_snapshot.fetch_add(1, Ordering::Relaxed),
                ),
                state,
            })
        })
    }
}

fn action_guarantee(action: &str) -> ActionDeliveryGuarantee {
    match action {
        "click" | "double_click" | "right_click" | "scroll" | "type_text" | "press_key"
        | "hotkey" | "set_value" => ActionDeliveryGuarantee::Background,
        "drag" if cfg!(target_os = "macos") => ActionDeliveryGuarantee::MayActivate,
        "drag" => ActionDeliveryGuarantee::Background,
        // The Windows launch path never activates the launched window
        // (SW_SHOWNOACTIVATE plus a foreground-restore guard), so launching
        // qualifies as background there. macOS activation is broker-driven.
        "launch_app" if cfg!(target_os = "windows") => ActionDeliveryGuarantee::Background,
        "bring_to_front" | "launch_app" => ActionDeliveryGuarantee::MayActivate,
        _ => ActionDeliveryGuarantee::Unsupported,
    }
}

// Delivery-mode and foreground helpers below drive the CUA registry path
// (macOS, Windows and Linux Wayland).
fn action_guarantee_for_native(action: &str, native: NativeTarget) -> ActionDeliveryGuarantee {
    #[cfg(target_os = "macos")]
    if action == "scroll"
        && i32::try_from(native.pid)
            .ok()
            .is_some_and(platform_macos::browser::ElectronJs::is_electron)
    {
        return ActionDeliveryGuarantee::MayActivate;
    }
    let _ = native;
    action_guarantee(action)
}

fn apply_action_delivery_mode(
    arguments: &mut serde_json::Map<String, serde_json::Value>,
    action: &str,
    policy: SessionPolicy,
) {
    // Delivery mode is a trusted consequence of the accepted session policy,
    // never client-controlled. This prevents a background-only client from
    // smuggling `foreground` through the adapter's opaque argument object.
    arguments.remove("delivery_mode");
    if !action_supports_delivery_mode(action) {
        return;
    }
    let mode = match policy {
        SessionPolicy::BackgroundOnly => "background",
        SessionPolicy::AllowActivation => "foreground",
        SessionPolicy::ViewOnly => return,
    };
    arguments.insert("delivery_mode".into(), mode.into());
}

fn apply_interactive_action_hints(
    arguments: &mut serde_json::Map<String, serde_json::Value>,
    action: &str,
) {
    // RCDP already observes the captured target continuously and reports its
    // lifecycle through the session. Embedded CUA input therefore does not
    // need the agent-oriented one-second post-action window-change poll.
    arguments.remove("_skip_window_change_detection");
    if action_supports_delivery_mode(action) {
        arguments.insert("_skip_window_change_detection".into(), true.into());
    }
}

fn action_supports_delivery_mode(action: &str) -> bool {
    matches!(
        action,
        "click"
            | "double_click"
            | "right_click"
            | "scroll"
            | "type_text"
            | "press_key"
            | "hotkey"
            | "drag"
    )
}

async fn persist_foreground_for_input(
    registry: &ToolRegistry,
    native: NativeTarget,
    action: &str,
    policy: SessionPolicy,
) -> Result<(), ProviderError> {
    if !requires_persistent_foreground(action, policy) {
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    if i32::try_from(native.pid)
        .ok()
        .is_some_and(|pid| platform_macos::apps::frontmost_pid() == Some(pid))
    {
        return Ok(());
    }

    // CUA's per-action foreground delivery is intentionally only a brief
    // front -> act -> restore assist. Native proxy sessions explicitly opt in
    // to persistent foreground ownership so focus-sensitive applications stay
    // armed across a sequence of pointer and keyboard operations.
    let result = registry
        .invoke(
            "bring_to_front",
            serde_json::json!({
                "pid": native.pid,
                "window_id": native.window_id,
            }),
        )
        .await;
    if result.is_error == Some(true) {
        return Err(ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            tool_result_text(&result),
        ));
    }
    Ok(())
}

fn requires_persistent_foreground(action: &str, policy: SessionPolicy) -> bool {
    policy == SessionPolicy::AllowActivation && action_supports_delivery_mode(action)
}

fn pixel_coordinate_pairs(action: &str) -> &'static [(&'static str, &'static str)] {
    match action {
        "click" | "double_click" | "right_click" | "scroll" | "type_text" | "press_key"
        | "hotkey" => &[("x", "y")],
        "drag" => &[("from_x", "from_y"), ("to_x", "to_y")],
        _ => &[],
    }
}

fn scale_pixel_arguments(
    arguments: &mut serde_json::Map<String, serde_json::Value>,
    coordinate_pairs: &[(&str, &str)],
    source: &SurfaceGeometry,
    native: NativePixelGeometry,
) -> Result<(), ProviderError> {
    if source.width_px == 0 || source.height_px == 0 {
        return Err(ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            "RCDP video coordinate space must be non-zero",
        ));
    }
    for (x, y) in coordinate_pairs {
        scale_coordinate(arguments, x, source.width_px, native.width_px)?;
        scale_coordinate(arguments, y, source.height_px, native.height_px)?;
    }
    Ok(())
}

fn scale_coordinate(
    arguments: &mut serde_json::Map<String, serde_json::Value>,
    field: &str,
    source_extent: u32,
    native_extent: u32,
) -> Result<(), ProviderError> {
    let Some(value) = arguments.get_mut(field) else {
        return Ok(());
    };
    let coordinate = value.as_f64().ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            format!("pixel coordinate {field} must be numeric"),
        )
    })?;
    if !coordinate.is_finite() || coordinate < 0.0 || coordinate >= f64::from(source_extent) {
        return Err(ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            format!(
                "pixel coordinate {field}={coordinate} is outside the RCDP frame extent {source_extent}"
            ),
        ));
    }
    let scaled = coordinate * f64::from(native_extent) / f64::from(source_extent);
    *value = serde_json::Value::Number(serde_json::Number::from_f64(scaled).ok_or_else(|| {
        ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            format!("scaled pixel coordinate {field} is not finite"),
        )
    })?);
    Ok(())
}

pub(crate) fn tool_result_text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|content| match content {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_else(|| "CUA action failed".into())
}

pub(crate) fn tool_result_json(result: &ToolResult) -> Option<serde_json::Value> {
    result.content.iter().find_map(|content| match content {
        Content::Text { text, .. } => serde_json::from_str(text).ok(),
        _ => None,
    })
}

/// Signalled once the platform tool registry — and with it the cursor
/// overlay's command channel — has been constructed.
///
/// A host that must hand its OS main thread to a UI loop has to know when that
/// is safe: `cursor::overlay::run_on_main_thread()` takes the command receiver
/// once, and if it runs first it finds nothing and parks forever without ever
/// rendering. Publishing readiness from the same function that calls
/// `overlay::init` makes the ordering correct by construction, rather than
/// depending on every caller in every daemon mode remembering to signal.
static OVERLAY_READY: (std::sync::Mutex<bool>, std::sync::Condvar) =
    (std::sync::Mutex::new(false), std::sync::Condvar::new());

fn mark_overlay_ready() {
    let (lock, cvar) = &OVERLAY_READY;
    *lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = true;
    cvar.notify_all();
}

/// Block until the platform registry exists. Returns immediately if it already
/// does. No timeout: a timeout here would be a guess, and the two outcomes it
/// papers over (hang vs. race) are both bugs worth seeing.
pub fn wait_for_overlay_ready() {
    let (lock, cvar) = &OVERLAY_READY;
    let mut ready = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while !*ready {
        ready = cvar
            .wait(ready)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
}

/// The cursor-overlay configuration the host daemon registers its driver with.
///
/// Shared by every platform so the overlay's identity, theme and motion are one
/// decision rather than a per-adapter one. `cursor_id` stays at the crate
/// default (`"default"`), which is the anonymous cursor: per-session cursors are
/// created lazily by key as driver sessions act.
pub fn host_cursor_config() -> cursor_overlay::CursorConfig {
    cursor_overlay::CursorConfig {
        enabled: true,
        ..cursor_overlay::CursorConfig::default()
    }
}

/// macOS registers the tool set *with* the cursor overlay.
///
/// `register_tools()` (the previous call) is the no-overlay constructor: it
/// never calls `cursor::overlay::init`, so the overlay command channel was
/// never created and nothing could render on the origin desktop. The cursor
/// hook still fired -- it is pushed from `CursorRegistry::update_position`,
/// independent of the overlay window -- which is why agent cursors appeared in
/// the *window streams* while the Space's own desktop stayed bare.
///
/// The contract of this constructor (see its doc comment in platform-macos) is
/// that `main()` must then call `cursor::overlay::run_on_main_thread()` on the
/// OS main thread. `cua_spacesd::main` does. Initializing the channel without ever
/// draining it is the documented trap: tools report the cursor facility as
/// available while nothing renders.
#[cfg(target_os = "macos")]
fn platform_registry() -> Result<ToolRegistry, ProviderError> {
    let registry =
        platform_macos::register_tools_with_cursor(host_cursor_config(), false, false, None);
    // System cursor shape reporting is independent of the agent-cursor
    // overlay: a remote viewer wants the I-beam even when Cua draws no cursor
    // of its own. It needs only a Window Server session, so it is gated on
    // graphic access rather than on the overlay config.
    if platform_macos::session::has_graphic_access() {
        macos_cursor_shape::install();
    }
    mark_overlay_ready();
    Ok(registry)
}

/// Windows needs no cooperation from `main()`: `register_tools_with_cursor`
/// calls `overlay::run_on_thread()` internally, which spawns its own STA
/// message-loop thread and returns (Win32 requires the window's creating
/// thread to pump it).
#[cfg(target_os = "windows")]
fn platform_registry() -> Result<ToolRegistry, ProviderError> {
    let registry = platform_windows::register_tools_with_cursor(host_cursor_config(), false);
    mark_overlay_ready();
    Ok(registry)
}

// Linux runs the X11 capture backend directly and does not route through a CUA
// tool registry, so an empty registry keeps the shared catalog/epoch machinery
// working while exposing no automation actions (ViewOnly is sufficient for v1).
#[cfg(target_os = "linux")]
fn platform_registry() -> Result<ToolRegistry, ProviderError> {
    // The full cua-driver Linux registry: AT-SPI accessibility, launch_app,
    // background (XSendEvent/AT-SPI) input and the agent-cursor overlay on
    // the origin desktop. Without a display (headless CI) the overlay stays
    // off and the registry still serves non-graphical tools.
    let mut config = host_cursor_config();
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        config.enabled = false;
    }
    let registry = platform_linux::register_tools_with_cursor(config, false);
    mark_overlay_ready();
    Ok(registry)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn platform_registry() -> Result<ToolRegistry, ProviderError> {
    Err(ProviderError::new(
        ProviderErrorCode::Unsupported,
        "the CUA provider currently supports macOS, Windows, and Linux",
    ))
}

#[cfg(target_os = "macos")]
fn native_windows(on_screen_only: bool) -> Result<Vec<NativeWindow>, ProviderError> {
    let windows = if on_screen_only {
        platform_macos::windows::visible_windows()
    } else {
        platform_macos::windows::all_windows()
    };
    Ok(without_own_windows(windows, std::process::id())
        .into_iter()
        .map(|window| NativeWindow {
            native: NativeTarget {
                pid: i64::from(window.pid),
                window_id: u64::from(window.window_id),
            },
            application_id: platform_macos::apps::bundle_id_for_pid(window.pid),
            app_name: window.app_name,
            title: window.title,
            geometry: SurfaceGeometry {
                width_px: window.bounds.width.max(1.0).round() as u32,
                height_px: window.bounds.height.max(1.0).round() as u32,
                scale_factor: 1.0,
            },
            visible: window.is_on_screen,
        })
        .collect())
}

#[cfg(target_os = "windows")]
fn native_windows(on_screen_only: bool) -> Result<Vec<NativeWindow>, ProviderError> {
    Ok(windows_support::enumerate_windows(on_screen_only))
}

#[cfg(target_os = "linux")]
fn native_windows(on_screen_only: bool) -> Result<Vec<NativeWindow>, ProviderError> {
    // A Wayland (Hyprland) session has no usable X11 client list, so the X11
    // backend would enumerate nothing; use the hyprctl-backed one instead.
    if linux_wayland::active() {
        return linux_wayland::enumerate_windows(on_screen_only)
            .map_err(|message| ProviderError::new(ProviderErrorCode::Internal, message));
    }
    // Phantom windows (input-only, degenerate, our own overlay) never become
    // targets. System windows (dock, desktop) are catalogued but filtered by
    // the services unless asked for.
    let Ok((conn, root)) = linux_x11::connect() else {
        return Ok(Vec::new());
    };
    Ok(linux_x11::list_windows(&conn, root)
        .into_iter()
        .filter(|window| {
            window.kind != linux_x11::X11WindowKind::Phantom && (!on_screen_only || window.mapped)
        })
        .map(|window| NativeWindow {
            native: NativeTarget {
                pid: i64::from(window.pid.unwrap_or(0)),
                window_id: u64::from(window.xid),
            },
            application_id: (!window.app_id.is_empty()).then(|| window.app_id.clone()),
            app_name: window.app_name,
            title: window.title,
            geometry: SurfaceGeometry {
                width_px: window.width.max(1),
                height_px: window.height.max(1),
                scale_factor: 1.0,
            },
            visible: window.mapped,
        })
        .collect())
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn native_windows(_on_screen_only: bool) -> Result<Vec<NativeWindow>, ProviderError> {
    Err(ProviderError::new(
        ProviderErrorCode::Unsupported,
        "target enumeration is not implemented on this operating system",
    ))
}

/// This process's own windows (the agent-cursor overlay, which the daemon
/// runs on macOS) are never targets.
#[cfg(target_os = "macos")]
fn without_own_windows(
    mut windows: Vec<platform_macos::windows::WindowInfo>,
    own_pid: u32,
) -> Vec<platform_macos::windows::WindowInfo> {
    windows.retain(|window| u32::try_from(window.pid).ok() != Some(own_pid));
    windows
}

#[cfg(target_os = "macos")]
fn macos_application_bundle(pid: i64) -> Option<PathBuf> {
    let output = Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let executable = String::from_utf8(output.stdout).ok()?;
    Path::new(executable.trim())
        .ancestors()
        .find(|path| path.extension().is_some_and(|extension| extension == "app"))
        .map(Path::to_path_buf)
}

#[cfg(target_os = "macos")]
fn plist_string(info_plist: &Path, key: &str) -> Option<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(info_plist)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[cfg(target_os = "macos")]
fn macos_app_icon(native: NativeTarget) -> Result<Option<ProviderAppIcon>, ProviderError> {
    let Some(bundle) = macos_application_bundle(native.pid) else {
        return Ok(None);
    };
    let resources = bundle.join("Contents/Resources");
    let icon_name = plist_string(&bundle.join("Contents/Info.plist"), "CFBundleIconFile");
    let preferred = icon_name.and_then(|name| {
        let file_name = Path::new(&name).file_name()?.to_owned();
        let path = resources.join(file_name);
        if path.extension().is_some() {
            Some(path)
        } else {
            Some(path.with_extension("icns"))
        }
    });
    let path = preferred.filter(|path| path.is_file()).or_else(|| {
        std::fs::read_dir(&resources)
            .ok()?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        matches!(
                            extension.to_ascii_lowercase().as_str(),
                            "icns" | "png" | "ico"
                        )
                    })
            })
            .max_by_key(|path| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0))
    });
    let Some(path) = path else {
        return Ok(None);
    };
    let metadata = std::fs::metadata(&path).map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::Internal,
            "the application icon could not be inspected",
        )
    })?;
    if metadata.len() == 0 || metadata.len() > MAX_NATIVE_APP_ICON_BYTES {
        return Err(ProviderError::new(
            ProviderErrorCode::Internal,
            "the application icon has an unsupported size",
        ));
    }
    let media_type = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "icns" => "application/x-apple-icns",
        "png" => "image/png",
        "ico" => "image/x-icon",
        _ => return Ok(None),
    };
    let bytes = std::fs::read(path).map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::Internal,
            "the application icon could not be read",
        )
    })?;
    Ok(Some(ProviderAppIcon {
        media_type: media_type.into(),
        bytes: Arc::from(bytes),
    }))
}

#[cfg(target_os = "macos")]
async fn resize_native_window(
    native: NativeTarget,
    width_points: u32,
    height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    let pid = i32::try_from(native.pid).map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "target pid is invalid",
        )
    })?;
    let window_id = u32::try_from(native.window_id).map_err(|_| {
        ProviderError::new(
            ProviderErrorCode::TargetUnavailable,
            "target window identifier is invalid",
        )
    })?;
    platform_macos::focus_guard::with_focus_suppressed_now(
        Some(pid),
        "rcdp.window_geometry",
        || async move {
            tokio::task::spawn_blocking(move || {
                macos_geometry::resize_window_by_id(
                    pid,
                    window_id,
                    f64::from(width_points),
                    f64::from(height_points),
                )
            })
            .await
        },
    )
    .await
    .map_err(|error| {
        ProviderError::new(
            ProviderErrorCode::DeliveryFailed,
            format!("host window resize task failed: {error}"),
        )
    })?
    .map(|bounds| AppliedWindowGeometry {
        width_points: bounds.width.round().clamp(1.0, f64::from(u32::MAX)) as u32,
        height_points: bounds.height.round().clamp(1.0, f64::from(u32::MAX)) as u32,
    })
    .map_err(|error| ProviderError::new(ProviderErrorCode::DeliveryFailed, error))
}

#[cfg(target_os = "windows")]
async fn resize_native_window(
    native: NativeTarget,
    width_points: u32,
    height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    let (width_points, height_points) =
        windows_support::resize_window(native.window_id, width_points, height_points)
            .map_err(|error| ProviderError::new(ProviderErrorCode::DeliveryFailed, error))?;
    Ok(AppliedWindowGeometry {
        width_points,
        height_points,
    })
}

#[cfg(target_os = "linux")]
async fn resize_native_window(
    native: NativeTarget,
    width_points: u32,
    height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    if linux_wayland::active() {
        linux_wayland::resize(native.window_id, width_points, height_points)
    } else {
        linux_capture::resize(native.window_id, width_points, height_points)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
async fn resize_native_window(
    _native: NativeTarget,
    _width_points: u32,
    _height_points: u32,
) -> Result<AppliedWindowGeometry, ProviderError> {
    Err(ProviderError::new(
        ProviderErrorCode::Unsupported,
        "background-safe host window resizing is not available on this platform",
    ))
}

#[cfg(target_os = "macos")]
fn start_platform_capture(
    native: NativeTarget,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: NativeGeometryCallback,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    macos_capture::start(
        macos_capture::CaptureSource::Window(native),
        config,
        sink,
        native_geometry,
    )
}

#[cfg(target_os = "windows")]
fn start_platform_capture(
    native: NativeTarget,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: NativeGeometryCallback,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    windows_capture::start(native.window_id, config.max_fps, sink, native_geometry)
}

#[cfg(target_os = "linux")]
fn start_platform_capture(
    native: NativeTarget,
    config: &CaptureConfig,
    sink: Arc<dyn CaptureSink>,
    native_geometry: NativeGeometryCallback,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    if linux_wayland::active() {
        linux_wayland::start(native, config, sink, native_geometry)
    } else {
        linux_capture::start(native, config, sink, native_geometry)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn start_platform_capture(
    _native: NativeTarget,
    _config: &CaptureConfig,
    _sink: Arc<dyn CaptureSink>,
    _native_geometry: NativeGeometryCallback,
) -> Result<Arc<dyn CaptureLease>, ProviderError> {
    Err(ProviderError::new(
        ProviderErrorCode::Unsupported,
        "capture is not implemented on this operating system",
    ))
}

#[cfg(all(test, target_os = "macos"))]
mod own_window_tests {
    use platform_macos::windows::{WindowBounds, WindowInfo};

    fn window(window_id: u32, pid: i32) -> WindowInfo {
        WindowInfo {
            window_id,
            pid,
            app_name: format!("app-{pid}"),
            title: String::new(),
            bounds: WindowBounds {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 100.0,
            },
            layer: 0,
            z_index: 0,
            is_on_screen: true,
            current_space_id: None,
            on_current_space: None,
            space_ids: None,
        }
    }

    // Once serve runs the agent-cursor overlay, its window is on screen;
    // it must not be offered as a stream target.
    #[test]
    fn the_daemons_own_windows_are_not_targets() {
        let kept = super::without_own_windows(vec![window(1, 42), window(2, 7), window(3, 42)], 42);
        assert_eq!(
            kept.iter().map(|w| w.window_id).collect::<Vec<_>>(),
            vec![2]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(visible: bool) -> NativeWindow {
        NativeWindow {
            native: NativeTarget {
                pid: 42,
                window_id: 99,
            },
            application_id: Some("com.example.Fixture".into()),
            app_name: "Fixture".into(),
            title: "Window".into(),
            geometry: SurfaceGeometry {
                width_px: 800,
                height_px: 600,
                scale_factor: 2.0,
            },
            visible,
        }
    }

    #[test]
    fn recycled_native_target_bumps_epoch() {
        let mut catalog = TargetCatalog::default();
        let first = catalog.refresh(vec![window(true)]).remove(0);
        catalog.refresh(Vec::new());
        let second = catalog.refresh(vec![window(true)]).remove(0);
        assert_eq!(first.descriptor.window, second.descriptor.window);
        assert_eq!(first.id.epoch, TargetEpoch(1));
        assert_eq!(second.id.epoch, TargetEpoch(2));
        assert!(matches!(
            catalog.resolve(&first.descriptor.window, first.id.epoch),
            Err(ProviderError {
                code: ProviderErrorCode::StaleTarget,
                ..
            })
        ));
    }

    #[test]
    fn application_selector_matches_stable_id_before_catalog_exposure() {
        let selector = ApplicationSelector::Id("com.example.Fixture".into());
        let matching = window(true);
        let mut other = window(true);
        other.native.window_id = 100;
        other.application_id = Some("com.example.Other".into());
        let windows = vec![matching, other]
            .into_iter()
            .filter(|candidate| match &selector {
                ApplicationSelector::Id(expected) => {
                    candidate.application_id.as_ref() == Some(expected)
                }
                ApplicationSelector::Name(expected) => &candidate.app_name == expected,
            })
            .collect::<Vec<_>>();

        let targets = TargetCatalog::default().refresh(windows);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].descriptor.app_name, "Fixture");
    }

    #[test]
    fn provider_identity_debug_redacts_native_values() {
        let mut catalog = TargetCatalog::default();
        let target = catalog.refresh(vec![window(true)]).remove(0);
        let debug = format!("{:?}", target.id);
        assert!(!debug.contains("42"));
        assert!(!debug.contains("99"));
    }

    #[test]
    fn policy_capabilities_distinguish_background_from_activation() {
        assert_eq!(
            action_guarantee("type_text"),
            ActionDeliveryGuarantee::Background
        );
        assert_eq!(
            action_guarantee("bring_to_front"),
            ActionDeliveryGuarantee::MayActivate
        );
        assert_eq!(
            action_guarantee("unknown_action"),
            ActionDeliveryGuarantee::Unsupported
        );
        assert_eq!(
            action_guarantee("drag"),
            if cfg!(target_os = "macos") {
                ActionDeliveryGuarantee::MayActivate
            } else {
                ActionDeliveryGuarantee::Background
            }
        );
    }

    #[test]
    fn session_policy_owns_cua_delivery_mode() {
        let mut background = serde_json::json!({"delivery_mode": "foreground"})
            .as_object()
            .expect("object")
            .clone();
        apply_action_delivery_mode(&mut background, "click", SessionPolicy::BackgroundOnly);
        assert_eq!(background["delivery_mode"], "background");

        let mut foreground = serde_json::json!({"delivery_mode": "background"})
            .as_object()
            .expect("object")
            .clone();
        apply_action_delivery_mode(&mut foreground, "press_key", SessionPolicy::AllowActivation);
        assert_eq!(foreground["delivery_mode"], "foreground");

        let mut unsupported = serde_json::json!({"delivery_mode": "foreground"})
            .as_object()
            .expect("object")
            .clone();
        apply_action_delivery_mode(
            &mut unsupported,
            "set_value",
            SessionPolicy::AllowActivation,
        );
        assert!(!unsupported.contains_key("delivery_mode"));
    }

    #[test]
    fn interactive_input_skips_redundant_cua_post_action_polling() {
        let mut arguments = serde_json::Map::new();
        apply_interactive_action_hints(&mut arguments, "type_text");
        assert_eq!(
            arguments.get("_skip_window_change_detection"),
            Some(&serde_json::Value::Bool(true))
        );

        arguments.insert("_skip_window_change_detection".into(), true.into());
        apply_interactive_action_hints(&mut arguments, "set_value");
        assert!(!arguments.contains_key("_skip_window_change_detection"));
    }

    #[test]
    fn persistent_foreground_is_limited_to_activation_allowed_input() {
        assert!(requires_persistent_foreground(
            "click",
            SessionPolicy::AllowActivation
        ));
        assert!(requires_persistent_foreground(
            "type_text",
            SessionPolicy::AllowActivation
        ));
        assert!(!requires_persistent_foreground(
            "click",
            SessionPolicy::BackgroundOnly
        ));
        assert!(!requires_persistent_foreground(
            "set_value",
            SessionPolicy::AllowActivation
        ));
        assert!(!requires_persistent_foreground(
            "bring_to_front",
            SessionPolicy::AllowActivation
        ));
    }

    #[test]
    fn pixel_arguments_scale_from_video_frame_to_native_capture() {
        let mut arguments = serde_json::json!({"x": 640, "y": 320})
            .as_object()
            .expect("object")
            .clone();
        scale_pixel_arguments(
            &mut arguments,
            pixel_coordinate_pairs("click"),
            &SurfaceGeometry {
                width_px: 1280,
                height_px: 640,
                scale_factor: 2.0 / 3.0,
            },
            NativePixelGeometry {
                width_px: 1920,
                height_px: 960,
            },
        )
        .expect("coordinates scale");

        assert_eq!(arguments["x"].as_f64(), Some(960.0));
        assert_eq!(arguments["y"].as_f64(), Some(480.0));
    }

    #[test]
    fn drag_scales_both_endpoints_and_rejects_out_of_frame_coordinates() {
        let source = SurfaceGeometry {
            width_px: 100,
            height_px: 50,
            scale_factor: 1.0,
        };
        let native = NativePixelGeometry {
            width_px: 200,
            height_px: 150,
        };
        let mut arguments = serde_json::json!({
            "from_x": 10,
            "from_y": 5,
            "to_x": 99,
            "to_y": 49,
        })
        .as_object()
        .expect("object")
        .clone();
        scale_pixel_arguments(
            &mut arguments,
            pixel_coordinate_pairs("drag"),
            &source,
            native,
        )
        .expect("drag coordinates scale");
        assert_eq!(arguments["from_x"].as_f64(), Some(20.0));
        assert_eq!(arguments["from_y"].as_f64(), Some(15.0));
        assert_eq!(arguments["to_x"].as_f64(), Some(198.0));
        assert_eq!(arguments["to_y"].as_f64(), Some(147.0));

        let mut invalid_arguments = serde_json::json!({
            "from_x": 10,
            "from_y": 5,
            "to_x": 100,
            "to_y": 49,
        })
        .as_object()
        .expect("object")
        .clone();
        assert!(matches!(
            scale_pixel_arguments(
                &mut invalid_arguments,
                pixel_coordinate_pairs("drag"),
                &source,
                native,
            ),
            Err(ProviderError {
                code: ProviderErrorCode::DeliveryFailed,
                ..
            })
        ));
    }

    #[test]
    fn adapter_newtypes_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CuaTargetProvider>();
        assert_send_sync::<CuaCaptureProvider>();
        assert_send_sync::<CuaActionProvider>();
        assert_send_sync::<CuaAccessibilityProvider>();
    }
}
