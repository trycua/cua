// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The routes this session's desktop takes, per capability. [`LinuxRoutes`]
//! is chosen once from [`desktop`](crate::desktop::desktop) and rebuilt
//! whenever the desktop is reclassified. Each capability is an ordered list
//! of routes; a call tries them in order until one is [`Outcome::Done`] or
//! [`Outcome::Refused`]. Checks that need the running session (niri's
//! attestation, a KWin window id, the GNOME helper) stay in the route body,
//! which reports [`Outcome::NotApplicable`] when they fail.

use std::sync::{Arc, PoisonError, RwLock};

use crate::desktop::{Compositor, Desktop};

/// What one route made of a call.
pub(crate) enum Outcome<T> {
    Done(T),
    /// The route owns this call and refuses it: no later route runs.
    Refused(anyhow::Error),
    /// The route does not serve this call: the next route runs.
    NotApplicable,
}

impl<T> From<anyhow::Result<T>> for Outcome<T> {
    fn from(result: anyhow::Result<T>) -> Self {
        match result {
            Ok(value) => Outcome::Done(value),
            Err(error) => Outcome::Refused(error),
        }
    }
}

/// Run `routes` in order through `run` until one is done or refuses.
pub(crate) fn first_route<R: Copy, T>(
    capability: &str,
    routes: &[R],
    mut run: impl FnMut(R) -> Outcome<T>,
) -> anyhow::Result<T> {
    for &route in routes {
        match run(route) {
            Outcome::Done(value) => return Ok(value),
            Outcome::Refused(error) => return Err(error),
            Outcome::NotApplicable => {}
        }
    }
    anyhow::bail!("no Linux {capability} route applies to this desktop")
}

/// Screen and window capture.
pub(crate) trait LinuxCapture {
    /// The whole desktop as PNG bytes.
    fn capture_display(&self) -> anyhow::Result<Vec<u8>>;
    /// One window as PNG bytes. On Wayland, `pid` lets a compositor tie the
    /// capture to the process that owns `xid`.
    fn capture_window(&self, xid: u64, pid: Option<u32>) -> anyhow::Result<Vec<u8>>;
}

/// A display capture route. The bodies are in `wayland/mod.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisplayRoute {
    /// niri's attested wlroots screencopy. Applies once niri is attested;
    /// then terminal.
    Niri,
    /// Hyprland's per-output composite, when it has more than one output.
    HyprlandOutputs,
    /// The opt-in GNOME compositor helper. Terminal when the helper is
    /// available: GNOME's portal can crash Shell on a 0x0 cursor sprite.
    GnomeHelper,
    /// KWin's ScreenShot2, only while `cua-driver kwin-capture` is on. A
    /// failure passes on.
    KWinWorkspace,
    /// zwlr_screencopy_manager_v1, then `grim`.
    WlrScreencopy,
    /// ext-image-copy-capture-v1.
    ExtImageCopy,
    /// xdg-desktop-portal Screenshot. A refusal is terminal; an unreachable
    /// portal passes on.
    Portal,
    /// The X11 root window: ImageMagick `import`, then the composed
    /// top-level windows.
    X11Root,
}

/// A window capture route. The bodies are in `wayland/mod.rs`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowRoute {
    /// KWin's per-window capture, for a window KWin listed.
    KWin,
    /// Hyprland's toplevel export. Terminal.
    Hyprland,
    /// A crop of niri's display capture. Applies once niri is attested;
    /// then terminal.
    Niri,
    /// The nested cua-compositor's capture. Terminal.
    CuaCompositor,
    /// A GNOME helper (API 9+) snapshot of the window's own actor. Terminal
    /// unless the helper has no per-window capture.
    GnomeHelper,
    /// The display capture cropped to compositor-attested window geometry.
    /// Terminal: it fails closed when the geometry is unproven.
    OutputCrop,
    /// The X11 per-window cascade: MIT-SHM, XGetImage, then `import`.
    X11,
}

/// The display capture routes for `desktop`, in the order they are tried.
pub(crate) fn display_routes(desktop: Desktop) -> Vec<DisplayRoute> {
    use DisplayRoute::*;
    let Desktop::Wayland(compositor) = desktop else {
        return vec![X11Root];
    };
    let mut routes = Vec::new();
    // niri is attested over IPC, which may name a niri started from another
    // compositor's session, so any compositor but the injected one tries it.
    if compositor != Compositor::Injected {
        routes.push(Niri);
    }
    if compositor == Compositor::Hyprland {
        routes.push(HyprlandOutputs);
    }
    routes.extend([
        GnomeHelper,
        KWinWorkspace,
        WlrScreencopy,
        ExtImageCopy,
        Portal,
        X11Root,
    ]);
    routes
}

/// The window capture routes for `desktop`, in the order they are tried.
pub(crate) fn window_routes(desktop: Desktop) -> Vec<WindowRoute> {
    use WindowRoute::*;
    match desktop {
        Desktop::X11 | Desktop::XWayland(_) => vec![X11],
        Desktop::Wayland(Compositor::Hyprland) => vec![KWin, Hyprland],
        Desktop::Wayland(Compositor::Injected) => vec![KWin, CuaCompositor],
        Desktop::Wayland(Compositor::Niri | Compositor::Other) => {
            vec![KWin, Niri, GnomeHelper, OutputCrop]
        }
    }
}

/// Every capability's routes for one desktop.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LinuxRoutes {
    display_capture: Vec<DisplayRoute>,
    window_capture: Vec<WindowRoute>,
}

impl LinuxRoutes {
    pub(crate) fn new(desktop: Desktop) -> Self {
        Self {
            display_capture: display_routes(desktop),
            window_capture: window_routes(desktop),
        }
    }
}

#[cfg(target_os = "linux")]
impl LinuxCapture for LinuxRoutes {
    fn capture_display(&self) -> anyhow::Result<Vec<u8>> {
        first_route("display capture", &self.display_capture, DisplayRoute::run)
    }

    fn capture_window(&self, xid: u64, pid: Option<u32>) -> anyhow::Result<Vec<u8>> {
        first_route("window capture", &self.window_capture, |route| {
            route.run(xid, pid)
        })
    }
}

static ROUTES: RwLock<Option<Arc<LinuxRoutes>>> = RwLock::new(None);

/// This session's routes, chosen on first use.
pub(crate) fn routes() -> Arc<LinuxRoutes> {
    let known = ROUTES
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    known.unwrap_or_else(|| rebuild(crate::desktop::desktop()))
}

/// Choose the routes again for a reclassified `desktop`.
pub(crate) fn rebuild(desktop: Desktop) -> Arc<LinuxRoutes> {
    let routes = Arc::new(LinuxRoutes::new(desktop));
    *ROUTES.write().unwrap_or_else(PoisonError::into_inner) = Some(routes.clone());
    routes
}

#[cfg(test)]
mod tests {
    use super::*;

    const COMPOSITORS: [Compositor; 4] = [
        Compositor::Injected,
        Compositor::Hyprland,
        Compositor::Niri,
        Compositor::Other,
    ];

    #[test]
    fn x11_and_xwayland_capture_through_x11_only() {
        let mut desktops = vec![Desktop::X11];
        desktops.extend(COMPOSITORS.map(Desktop::XWayland));
        for desktop in desktops {
            assert_eq!(
                display_routes(desktop),
                [DisplayRoute::X11Root],
                "{desktop:?}"
            );
            assert_eq!(window_routes(desktop), [WindowRoute::X11], "{desktop:?}");
        }
    }

    #[test]
    fn wayland_display_capture_ends_in_the_portal_then_x11() {
        use DisplayRoute::*;
        let tail = [
            GnomeHelper,
            KWinWorkspace,
            WlrScreencopy,
            ExtImageCopy,
            Portal,
            X11Root,
        ];
        let with = |head: &[DisplayRoute]| [head, &tail[..]].concat();
        assert_eq!(display_routes(Desktop::Wayland(Compositor::Injected)), tail);
        assert_eq!(
            display_routes(Desktop::Wayland(Compositor::Hyprland)),
            with(&[Niri, HyprlandOutputs])
        );
        assert_eq!(
            display_routes(Desktop::Wayland(Compositor::Niri)),
            with(&[Niri])
        );
        assert_eq!(
            display_routes(Desktop::Wayland(Compositor::Other)),
            with(&[Niri])
        );
    }

    #[test]
    fn wayland_window_capture_ends_in_a_terminal_compositor_route() {
        use WindowRoute::*;
        assert_eq!(
            window_routes(Desktop::Wayland(Compositor::Injected)),
            [KWin, CuaCompositor]
        );
        assert_eq!(
            window_routes(Desktop::Wayland(Compositor::Hyprland)),
            [KWin, Hyprland]
        );
        for compositor in [Compositor::Niri, Compositor::Other] {
            assert_eq!(
                window_routes(Desktop::Wayland(compositor)),
                [KWin, Niri, GnomeHelper, OutputCrop],
                "{compositor:?}"
            );
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reclassifying_the_desktop_rebuilds_its_routes() {
        // The override holds the environment lock, so no other test
        // reclassifies in between, and reclassifies again when dropped.
        let _x11 = crate::test_env::x11_display(":9999999");
        rebuild(Desktop::Wayland(Compositor::Other));
        crate::desktop::reclassify();
        assert_eq!(*routes(), LinuxRoutes::new(Desktop::X11));
    }

    #[test]
    fn the_first_route_that_is_done_or_refuses_ends_the_call() {
        let mut tried = Vec::new();
        let mut run = |route: u8| {
            tried.push(route);
            match route {
                0 => Outcome::NotApplicable,
                1 => Outcome::Done("done"),
                _ => Outcome::Refused(anyhow::anyhow!("refused")),
            }
        };
        assert_eq!(first_route("test", &[0, 1, 2], &mut run).unwrap(), "done");
        assert_eq!(
            first_route("test", &[0, 2, 1], &mut run)
                .unwrap_err()
                .to_string(),
            "refused"
        );
        assert_eq!(
            first_route("test", &[0, 0], &mut run)
                .unwrap_err()
                .to_string(),
            "no Linux test route applies to this desktop"
        );
        assert_eq!(tried, [0, 1, 0, 2, 0, 0]);
    }
}
