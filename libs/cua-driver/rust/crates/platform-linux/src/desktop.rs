// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The session's desktop, classified from the environment once. Route
//! choices read [`desktop`] instead of re-reading `WAYLAND_DISPLAY`, the
//! native-Wayland opt-in or a compositor's socket variable.

use std::ffi::OsString;
use std::sync::{PoisonError, RwLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    /// No `WAYLAND_DISPLAY`: the X11 routes.
    X11,
    /// A Wayland session without the native-backend opt-in
    /// ([`ENABLE_WAYLAND_ENV`]): input takes the X11 routes
    /// through XWayland.
    XWayland(Compositor),
    /// A Wayland session with the native backend opted in.
    Wayland(Compositor),
}

/// The compositor a Wayland session names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compositor {
    /// `CUA_INJECT_SOCKET` is set: input goes to the nested cua-compositor
    /// and no host adapter applies.
    Injected,
    Hyprland,
    /// `NIRI_SOCKET` is set. Until [`Desktop::attested`], it may name
    /// another session's niri.
    Niri,
    Other,
}

impl Desktop {
    pub fn compositor(self) -> Option<Compositor> {
        match self {
            Desktop::X11 => None,
            Desktop::XWayland(compositor) | Desktop::Wayland(compositor) => Some(compositor),
        }
    }

    /// This desktop once niri's IPC attestation has run. An attested niri
    /// wins over a Hyprland signature it inherited; a `NIRI_SOCKET` that is
    /// not this session's niri names [`Compositor::Other`]. Probes niri's
    /// socket when one is set, so only choices that differ on niri call it.
    #[cfg(target_os = "linux")]
    pub fn attested(self) -> Desktop {
        match self {
            Desktop::Wayland(compositor)
                if compositor != Compositor::Injected && crate::wayland::niri::is_session() =>
            {
                Desktop::Wayland(Compositor::Niri)
            }
            Desktop::Wayland(Compositor::Niri) => Desktop::Wayland(Compositor::Other),
            desktop => desktop,
        }
    }
}

/// Name of the opt-in env var that unlocks the experimental native-Wayland
/// backend.
pub const ENABLE_WAYLAND_ENV: &str = "CUA_DRIVER_RS_ENABLE_WAYLAND";

static DESKTOP: RwLock<Option<Desktop>> = RwLock::new(None);

/// This session's desktop, classified on first use.
pub fn desktop() -> Desktop {
    let known = *DESKTOP.read().unwrap_or_else(PoisonError::into_inner);
    known.unwrap_or_else(reclassify)
}

/// Classify again after this process rewrote its display variables, as the
/// nested session does once it has started.
pub(crate) fn reclassify() -> Desktop {
    let desktop = classify(|name| std::env::var_os(name));
    *DESKTOP.write().unwrap_or_else(PoisonError::into_inner) = Some(desktop);
    desktop
}

/// Whether `value` of [`ENABLE_WAYLAND_ENV`] opts in: any
/// value other than empty, `0` or `false`.
pub(crate) fn opted_in(value: Option<&str>) -> bool {
    value.is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn classify(var: impl Fn(&str) -> Option<OsString>) -> Desktop {
    if var("WAYLAND_DISPLAY").is_none() {
        return Desktop::X11;
    }
    let text = |name| var(name).and_then(|value| value.into_string().ok());
    let set = |name| var(name).is_some_and(|value| !value.is_empty());
    let compositor = if text("CUA_INJECT_SOCKET").is_some_and(|socket| !socket.is_empty()) {
        Compositor::Injected
    } else if set("HYPRLAND_INSTANCE_SIGNATURE") {
        Compositor::Hyprland
    } else if set("NIRI_SOCKET") {
        Compositor::Niri
    } else {
        Compositor::Other
    };
    if opted_in(text(ENABLE_WAYLAND_ENV).as_deref()) {
        Desktop::Wayland(compositor)
    } else {
        Desktop::XWayland(compositor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn classify_env(vars: &[(&str, &str)]) -> Desktop {
        classify(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    const WAYLAND: [(&str, &str); 2] = [
        ("WAYLAND_DISPLAY", "wayland-0"),
        ("CUA_DRIVER_RS_ENABLE_WAYLAND", "1"),
    ];

    #[test]
    fn without_a_wayland_display_the_desktop_is_x11() {
        assert_eq!(classify_env(&[("DISPLAY", ":0")]), Desktop::X11);
        assert_eq!(
            classify_env(&[
                ("CUA_DRIVER_RS_ENABLE_WAYLAND", "1"),
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
            ]),
            Desktop::X11
        );
    }

    #[test]
    fn only_the_opt_in_makes_a_wayland_display_native_wayland() {
        for off in [None, Some(""), Some("0"), Some(" false ")] {
            let mut vars = vec![("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")];
            vars.extend(off.map(|off| ("CUA_DRIVER_RS_ENABLE_WAYLAND", off)));
            assert_eq!(
                classify_env(&vars),
                Desktop::XWayland(Compositor::Other),
                "{off:?}"
            );
        }
        assert_eq!(classify_env(&WAYLAND), Desktop::Wayland(Compositor::Other));
    }

    #[test]
    fn the_compositor_is_named_by_its_socket_variable_unless_input_is_injected() {
        let with =
            |extra: &[(&'static str, &'static str)]| classify_env(&[&WAYLAND[..], extra].concat());
        assert_eq!(
            with(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc")]),
            Desktop::Wayland(Compositor::Hyprland)
        );
        assert_eq!(
            with(&[("NIRI_SOCKET", "/run/niri.sock")]),
            Desktop::Wayland(Compositor::Niri)
        );
        assert_eq!(
            with(&[("HYPRLAND_INSTANCE_SIGNATURE", ""), ("NIRI_SOCKET", "")]),
            Desktop::Wayland(Compositor::Other)
        );
        assert_eq!(
            with(&[
                ("CUA_INJECT_SOCKET", "/run/cua.sock"),
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
            ]),
            Desktop::Wayland(Compositor::Injected)
        );
        assert_eq!(
            with(&[
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc"),
                ("NIRI_SOCKET", "/run/niri.sock"),
            ]),
            Desktop::Wayland(Compositor::Hyprland)
        );
        assert_eq!(
            classify_env(&[
                ("WAYLAND_DISPLAY", "wayland-0"),
                ("HYPRLAND_INSTANCE_SIGNATURE", "abc")
            ]),
            Desktop::XWayland(Compositor::Hyprland)
        );
    }
}
