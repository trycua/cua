// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which route a Wayland keyboard call took, so a tool's reply names the
//! route that really sent the keys instead of assuming one.

use std::cell::Cell;

/// A route Wayland keyboard input can go out on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardRoute {
    /// niri's own keymap, over its IPC.
    Niri,
    /// The wlroots virtual keyboard (`zwp_virtual_keyboard_v1`), directly or
    /// through `wtype`.
    VirtualKeyboard,
    /// libei, through the RemoteDesktop portal or KWin's EIS.
    Libei,
    /// The nested cua-compositor's control socket.
    Compositor,
}

impl KeyboardRoute {
    /// The route as a reply names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Niri => "niri's keyboard",
            Self::VirtualKeyboard => "Wayland virtual-keyboard",
            Self::Libei => "libei",
            Self::Compositor => "cua-compositor",
        }
    }
}

std::thread_local! {
    static LAST: Cell<Option<KeyboardRoute>> = const { Cell::new(None) };
}

/// Record that keyboard input on this thread just went out on `route`.
pub(crate) fn note(route: KeyboardRoute) {
    LAST.with(|last| last.set(Some(route)));
}

/// Run `body` and return the last route its keyboard input went out on, or
/// `None` when it sent no keyboard input. Must run on the thread that sends
/// the input.
pub fn tracked<T>(
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<(T, Option<KeyboardRoute>)> {
    LAST.with(|last| last.set(None));
    let value = body()?;
    Ok((value, LAST.with(Cell::take)))
}

/// The reply suffix for keys sent on `route`, e.g. `" (via libei)"`.
pub fn via(route: Option<KeyboardRoute>) -> String {
    route.map_or_else(String::new, |route| format!(" (via {})", route.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_names_the_last_route_the_body_used() {
        let (value, route) = tracked(|| {
            note(KeyboardRoute::VirtualKeyboard);
            note(KeyboardRoute::Libei);
            Ok(5)
        })
        .unwrap();
        assert_eq!((value, route), (5, Some(KeyboardRoute::Libei)));
        assert_eq!(via(route), " (via libei)");
    }

    #[test]
    fn a_route_from_an_earlier_call_is_not_reported() {
        note(KeyboardRoute::Niri);
        let ((), route) = tracked(|| Ok(())).unwrap();
        assert_eq!(route, None);
        assert_eq!(via(route), "");
    }
}
