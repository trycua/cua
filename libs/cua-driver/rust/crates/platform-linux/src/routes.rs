// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Ordered route lists per capability, chosen once from the classified
//! [`Desktop`] and rebuilt by [`crate::desktop::reclassify`]. A dispatcher
//! tries each route in order: the first `Done` or `Refused` decides, and
//! `NotApplicable` passes to the next. X11 and XWayland fallbacks are
//! entries in the list like any other route.
//!
//! Selection reads only the desktop. A check that needs a live probe, such
//! as niri's IPC attestation or a KWin window the registry has seen, stays
//! inside the route body and answers `NotApplicable` when it fails.

use std::sync::{Arc, PoisonError, RwLock};

use crate::desktop::{Compositor, Desktop};

/// What one route made of a request.
#[derive(Debug)]
pub enum Route<T> {
    Done(T),
    Refused(anyhow::Error),
    NotApplicable,
}

/// Window enumeration and geometry routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowsRoute {
    /// Hyprland IPC: PIDs and full native identity.
    Hyprland,
    /// niri IPC, once niri attests this session.
    Niri,
    /// The verified GNOME Shell helper.
    GnomeHelper,
    /// KWin's window list, where KWin access is allowed.
    KwinCapture,
    /// The trusted KWin helper.
    KwinHelper,
    /// wlr/ext foreign-toplevel, then the AT-SPI registry.
    NativeProtocols,
    /// X11 `_NET_CLIENT_LIST`.
    X11,
    /// X11 for XWayland clients, merged with AT-SPI for native toplevels.
    XWaylandWithAtspi,
}

/// Window activation routes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusRoute {
    /// KWin's own activation, for a window KWin listed.
    KwinActivation,
    /// Refuses focus-bound input for a window KWin listed.
    KwinInputRefusal,
    /// Hyprland: attests an already-active target, refuses otherwise.
    Hyprland,
    /// niri IPC focus with read-back, once niri attests this session.
    Niri,
    /// The nested cua-compositor's control socket.
    Injected,
    /// wlr foreign-toplevel `activate` with read-back.
    ForeignToplevel,
    /// The GNOME Shell helper.
    GnomeHelper,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxRoutes {
    pub windows: Vec<WindowsRoute>,
    /// Routes that may set another window's frame. Empty on native Wayland,
    /// where no compositor protocol permits it.
    pub frame: Vec<WindowsRoute>,
    /// Activation before focus-bound input.
    pub focus: Vec<FocusRoute>,
    /// Activation for its own sake (`bring_to_front`, menus), where no
    /// global input follows.
    pub bring_forward: Vec<FocusRoute>,
}

impl LinuxRoutes {
    pub fn select(desktop: Desktop) -> LinuxRoutes {
        use FocusRoute as F;
        use WindowsRoute as W;
        let windows = match desktop {
            Desktop::X11 | Desktop::XWayland(_) => vec![W::X11],
            // Hyprland's route lists or refuses, so nothing follows it.
            Desktop::Wayland(Compositor::Hyprland) => vec![W::Hyprland],
            Desktop::Wayland(compositor) => {
                let mut routes = match compositor {
                    Compositor::Injected => vec![],
                    _ => vec![W::Niri],
                };
                routes.extend([
                    W::GnomeHelper,
                    W::KwinCapture,
                    W::KwinHelper,
                    W::NativeProtocols,
                    W::XWaylandWithAtspi,
                ]);
                routes
            }
        };
        let frame = match desktop {
            Desktop::X11 | Desktop::XWayland(_) => vec![W::X11],
            Desktop::Wayland(_) => vec![],
        };
        // The control socket is named by `CUA_INJECT_SOCKET`, which only
        // classifies as `Injected` beside a `WAYLAND_DISPLAY`; on X11 the
        // route reads the variable itself. Hyprland's and the injected
        // route activate or refuse, so nothing follows them.
        let focus = match desktop {
            Desktop::Wayland(Compositor::Hyprland) => vec![F::KwinInputRefusal, F::Hyprland],
            Desktop::Wayland(Compositor::Injected) | Desktop::XWayland(Compositor::Injected) => {
                vec![F::KwinInputRefusal, F::Injected]
            }
            Desktop::Wayland(Compositor::Niri | Compositor::Other) => vec![
                F::KwinInputRefusal,
                F::Niri,
                F::ForeignToplevel,
                F::GnomeHelper,
            ],
            Desktop::XWayland(_) => {
                vec![F::KwinInputRefusal, F::ForeignToplevel, F::GnomeHelper]
            }
            Desktop::X11 => vec![
                F::KwinInputRefusal,
                F::Injected,
                F::ForeignToplevel,
                F::GnomeHelper,
            ],
        };
        let bring_forward = std::iter::once(F::KwinActivation)
            .chain(focus.iter().copied())
            .collect();
        LinuxRoutes {
            windows,
            frame,
            focus,
            bring_forward,
        }
    }
}

static ROUTES: RwLock<Option<Arc<LinuxRoutes>>> = RwLock::new(None);

/// This session's routes, selected on first use.
pub fn routes() -> Arc<LinuxRoutes> {
    let known = ROUTES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    known.unwrap_or_else(|| reselect(crate::desktop::desktop()))
}

/// Select the routes again for a newly classified desktop.
pub(crate) fn reselect(desktop: Desktop) -> Arc<LinuxRoutes> {
    let routes = Arc::new(LinuxRoutes::select(desktop));
    *ROUTES.write().unwrap_or_else(PoisonError::into_inner) = Some(routes.clone());
    routes
}

/// The frame one `set_window_frame` call asks for.
#[derive(Clone, Copy, Debug)]
pub struct FrameRequest {
    pub window_id: u64,
    pub pid: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// What a frame route observed: the frame read back, whether it matched the
/// request, whether it changed, and the window manager's refusal, if any.
pub type FrameOutcome = (Option<(i32, i32, u32, u32)>, bool, bool, Option<String>);

#[cfg(target_os = "linux")]
pub trait LinuxWindows {
    fn list(&self, filter_pid: Option<u32>) -> Route<Vec<crate::x11::WindowInfo>>;
    fn set_frame(&self, request: FrameRequest) -> Route<FrameOutcome>;
}

#[cfg(target_os = "linux")]
impl LinuxWindows for WindowsRoute {
    fn list(&self, filter_pid: Option<u32>) -> Route<Vec<crate::x11::WindowInfo>> {
        use crate::wayland as w;
        match self {
            WindowsRoute::Hyprland => w::hyprland_windows(filter_pid),
            WindowsRoute::Niri => w::niri_windows(filter_pid),
            WindowsRoute::GnomeHelper => w::gnome_helper_windows(filter_pid),
            WindowsRoute::KwinCapture => w::kwin_capture_windows(filter_pid),
            WindowsRoute::KwinHelper => w::kwin_helper_windows(filter_pid),
            WindowsRoute::NativeProtocols => w::native_protocol_windows(filter_pid),
            WindowsRoute::X11 => Route::Done(crate::x11::list_windows(filter_pid)),
            WindowsRoute::XWaylandWithAtspi => w::xwayland_with_atspi_windows(filter_pid),
        }
    }

    fn set_frame(&self, request: FrameRequest) -> Route<FrameOutcome> {
        match self {
            WindowsRoute::X11 => crate::x11::set_window_frame_route(request),
            _ => Route::NotApplicable,
        }
    }
}

#[cfg(target_os = "linux")]
pub trait LinuxFocus {
    fn activate(&self, window_id: u64, target_pid: Option<u32>) -> Route<()>;
}

#[cfg(target_os = "linux")]
impl LinuxFocus for FocusRoute {
    fn activate(&self, window_id: u64, target_pid: Option<u32>) -> Route<()> {
        use crate::wayland as w;
        match self {
            FocusRoute::KwinActivation => w::kwin_activation(window_id, target_pid),
            FocusRoute::KwinInputRefusal => w::kwin_input_refusal(window_id),
            FocusRoute::Hyprland => w::hyprland_activation(window_id, target_pid),
            FocusRoute::Niri => w::niri_activation(window_id, target_pid),
            FocusRoute::Injected => w::inject_activation(window_id, target_pid),
            FocusRoute::ForeignToplevel => w::foreign_toplevel_activation(window_id),
            FocusRoute::GnomeHelper => w::gnome_helper_activation(window_id),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FocusRoute as F;
    use WindowsRoute as W;

    const COMPOSITORS: [Compositor; 4] = [
        Compositor::Injected,
        Compositor::Hyprland,
        Compositor::Niri,
        Compositor::Other,
    ];

    fn every_desktop() -> impl Iterator<Item = Desktop> {
        std::iter::once(Desktop::X11)
            .chain(COMPOSITORS.map(Desktop::XWayland))
            .chain(COMPOSITORS.map(Desktop::Wayland))
    }

    #[test]
    fn x11_and_xwayland_list_and_frame_windows_through_x11_alone() {
        for desktop in std::iter::once(Desktop::X11).chain(COMPOSITORS.map(Desktop::XWayland)) {
            let routes = LinuxRoutes::select(desktop);
            assert_eq!(routes.windows, [W::X11], "{desktop:?}");
            assert_eq!(routes.frame, [W::X11], "{desktop:?}");
        }
    }

    #[test]
    fn native_wayland_lists_compositor_routes_then_falls_back_to_xwayland() {
        assert_eq!(
            LinuxRoutes::select(Desktop::Wayland(Compositor::Hyprland)).windows,
            [W::Hyprland]
        );
        let generic = [
            W::GnomeHelper,
            W::KwinCapture,
            W::KwinHelper,
            W::NativeProtocols,
            W::XWaylandWithAtspi,
        ];
        let table: [(Compositor, &[W]); 3] = [
            (Compositor::Niri, &[W::Niri]),
            (Compositor::Other, &[W::Niri]),
            (Compositor::Injected, &[]),
        ];
        for (compositor, first) in table {
            let routes = LinuxRoutes::select(Desktop::Wayland(compositor));
            assert_eq!(
                routes.windows,
                [first, &generic[..]].concat(),
                "{compositor:?}"
            );
        }
        for compositor in COMPOSITORS {
            assert!(
                LinuxRoutes::select(Desktop::Wayland(compositor))
                    .frame
                    .is_empty(),
                "{compositor:?}"
            );
        }
    }

    #[test]
    fn focus_routes_follow_the_compositor_and_always_refuse_kwin_input_first() {
        let table: [(Desktop, &[F]); 9] = [
            (
                Desktop::X11,
                &[F::Injected, F::ForeignToplevel, F::GnomeHelper],
            ),
            (Desktop::XWayland(Compositor::Injected), &[F::Injected]),
            (
                Desktop::XWayland(Compositor::Hyprland),
                &[F::ForeignToplevel, F::GnomeHelper],
            ),
            (
                Desktop::XWayland(Compositor::Niri),
                &[F::ForeignToplevel, F::GnomeHelper],
            ),
            (
                Desktop::XWayland(Compositor::Other),
                &[F::ForeignToplevel, F::GnomeHelper],
            ),
            (Desktop::Wayland(Compositor::Injected), &[F::Injected]),
            (Desktop::Wayland(Compositor::Hyprland), &[F::Hyprland]),
            (
                Desktop::Wayland(Compositor::Niri),
                &[F::Niri, F::ForeignToplevel, F::GnomeHelper],
            ),
            (
                Desktop::Wayland(Compositor::Other),
                &[F::Niri, F::ForeignToplevel, F::GnomeHelper],
            ),
        ];
        for (desktop, rest) in table {
            let routes = LinuxRoutes::select(desktop);
            assert_eq!(
                routes.focus,
                [&[F::KwinInputRefusal][..], rest].concat(),
                "{desktop:?}"
            );
        }
        assert_eq!(every_desktop().count(), table.len());
    }

    #[test]
    fn bringing_forward_tries_kwin_activation_before_the_input_focus_routes() {
        for desktop in every_desktop() {
            let routes = LinuxRoutes::select(desktop);
            assert_eq!(
                routes.bring_forward,
                [&[F::KwinActivation][..], &routes.focus[..]].concat(),
                "{desktop:?}"
            );
        }
    }
}
