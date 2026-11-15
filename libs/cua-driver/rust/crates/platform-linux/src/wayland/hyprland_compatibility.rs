// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Which targets background input on Hyprland's agent seats can reach.
//!
//! Background input arrives on the plugin's own seats (`Cua-Agent`,
//! `Cua-Agent-2`), separate `wl_seat` globals beside the user's. A native
//! Wayland client that binds every advertised seat gets it the same way, with
//! no toolkit-specific path; GTK 3, GTK 4 and Qt 6 do. So admission checks the
//! live target, not a list of packages:
//!
//! - the compositor must report the exact `(pid, window address)` as a mapped
//!   native Wayland client. XWayland windows refuse: X11 has one core pointer
//!   and keyboard, so a second seat cannot reach them separately;
//! - the process must be alive, and the input must not be one its toolkit is
//!   known to drop ([`QUIRKS`]).
//!
//! The plugin repeats the live checks at dispatch and adds the one only the
//! compositor can see: whether the client bound the lane seat's pointer or
//! keyboard. A client that binds only its first `wl_seat` (Chromium's Ozone
//! backend, and so Electron) is refused there with `client_not_bound`, before
//! any event is sent.
//!
//! This is a compatibility check, not a security boundary against same-user
//! code.

use std::io::{BufRead, BufReader};

/// A toolkit that binds the agent seats but drops some of their input.
struct Quirk {
    /// A shared library the client has mapped, naming its toolkit.
    library: &'static str,
    /// Whether the quirk is about keyboard input (else pointer input).
    keyboard: bool,
    /// Refusal code reported to the caller.
    reason: &'static str,
}

/// Refused although the client binds the agent seats. Add an entry only for a
/// reproduced failure, and say what fails.
const QUIRKS: &[Quirk] = &[
    // Qt 6 sends a key to its focus window, which is only ever a window the
    // compositor activated for the user's seat: a background Qt window drops
    // agent-seat keys (Qt 6.12, Qt Quick). Its pointer input works.
    Quirk {
        library: "libQt6Gui.so",
        keyboard: true,
        reason: "client_drops_background_keys",
    },
];

/// Whether background input on the agent seats may target `address`, a
/// window of `pid`; `keyboard` when the actions include keys. `Err` is the
/// refusal code.
pub(super) fn admit(pid: u32, address: u64, keyboard: bool) -> Result<(), &'static str> {
    let window = super::hyprland::window_for_address(address).ok_or("target_unavailable")?;
    admit_target(pid, window.pid, window.xwayland, keyboard, QUIRKS)
}

/// `window_pid` and `xwayland` are what the compositor reports for the window.
fn admit_target(
    pid: u32,
    window_pid: u32,
    xwayland: Option<bool>,
    keyboard: bool,
    quirks: &[Quirk],
) -> Result<(), &'static str> {
    if window_pid != pid {
        return Err("target_unavailable");
    }
    match xwayland {
        Some(false) => {}
        Some(true) => return Err("client_xwayland"),
        None => return Err("client_unattested"),
    }
    let maps =
        std::fs::File::open(format!("/proc/{pid}/maps")).map_err(|_| "target_unavailable")?;
    let applicable: Vec<&Quirk> = quirks
        .iter()
        .filter(|quirk| quirk.keyboard == keyboard)
        .collect();
    if applicable.is_empty() {
        return Ok(());
    }
    quirk_in(BufReader::new(maps), &applicable).map_or(Ok(()), |quirk| Err(quirk.reason))
}

/// The first quirk whose library appears in a `/proc/<pid>/maps` listing.
fn quirk_in<'a>(maps: impl BufRead, quirks: &[&'a Quirk]) -> Option<&'a Quirk> {
    maps.lines().map_while(Result::ok).find_map(|line| {
        let file = line.rsplit('/').next()?;
        quirks
            .iter()
            .copied()
            .find(|quirk| file.starts_with(quirk.library))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const QT_KEYS: Quirk = Quirk {
        library: "libQt6Gui.so",
        keyboard: true,
        reason: "client_drops_background_keys",
    };

    #[test]
    fn any_native_wayland_client_is_admitted() {
        // This test process stands in for an arbitrary unlisted application.
        let pid = std::process::id();
        for keyboard in [false, true] {
            assert_eq!(
                admit_target(pid, pid, Some(false), keyboard, QUIRKS),
                Ok(())
            );
        }
    }

    #[test]
    fn xwayland_and_unattested_windows_refuse() {
        let pid = std::process::id();
        assert_eq!(
            admit_target(pid, pid, Some(true), false, QUIRKS),
            Err("client_xwayland")
        );
        assert_eq!(
            admit_target(pid, pid, None, false, QUIRKS),
            Err("client_unattested")
        );
    }

    #[test]
    fn window_of_another_process_or_a_dead_one_refuses() {
        let pid = std::process::id();
        assert_eq!(
            admit_target(pid, pid + 1, Some(false), false, QUIRKS),
            Err("target_unavailable")
        );
        assert_eq!(
            admit_target(u32::MAX, u32::MAX, Some(false), false, QUIRKS),
            Err("target_unavailable")
        );
    }

    #[test]
    fn qt6_quirk_refuses_keys_only() {
        let maps = "7f00-7f10 r-xp 00000000 fd:01 42 /usr/lib/libQt6Gui.so.6.12.0\n\
                    7f10-7f20 r-xp 00000000 fd:01 43 /usr/lib/libc.so.6\n";
        assert_eq!(
            quirk_in(maps.as_bytes(), &[&QT_KEYS]).map(|quirk| quirk.reason),
            Some("client_drops_background_keys")
        );
        let gtk = "7f00-7f10 r-xp 00000000 fd:01 44 /usr/lib/libgtk-3.so.0.2400.52\n\
                   7f10-7f20 rw-p 00000000 00:00 0 [heap]\n";
        assert!(quirk_in(gtk.as_bytes(), &[&QT_KEYS]).is_none());
        // The live check reads this process's own maps: no Qt here.
        let pid = std::process::id();
        assert_eq!(
            admit_target(pid, pid, Some(false), true, &[QT_KEYS]),
            Ok(())
        );
    }

    #[test]
    fn a_quirk_matching_this_process_refuses_only_its_input_kind() {
        let pid = std::process::id();
        let maps = std::fs::read_to_string(format!("/proc/{pid}/maps")).unwrap();
        let library: &'static str = Box::leak(
            maps.lines()
                .find_map(|line| line.rsplit('/').next().filter(|f| f.contains(".so")))
                .expect("a mapped shared library")
                .split(".so")
                .next()
                .map(|stem| format!("{stem}.so"))
                .unwrap()
                .into_boxed_str(),
        );
        let quirk = Quirk {
            library,
            keyboard: true,
            reason: "client_quirk_test",
        };
        assert_eq!(
            admit_target(pid, pid, Some(false), true, std::slice::from_ref(&quirk)),
            Err("client_quirk_test")
        );
        assert_eq!(
            admit_target(pid, pid, Some(false), false, std::slice::from_ref(&quirk)),
            Ok(())
        );
    }
}
