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
//!   known to drop ([`QUIRKS`]), unless the user listed its executable in
//!   `$XDG_CONFIG_HOME/cua-driver/hyprland-apps.txt` ([`USER_APPS`]).
//!
//! The plugin repeats the live checks at dispatch and adds the one only the
//! compositor can see: whether the client bound the lane seat's pointer or
//! keyboard. A client that binds only its first `wl_seat` (Chromium's Ozone
//! backend, and so Electron) is refused there with `client_not_bound`, before
//! any event is sent.
//!
//! This is a compatibility check, not a security boundary against same-user
//! code.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

/// The user's list of executables they have qualified themselves, under the
/// config dir. One absolute path per line; blank lines and `#` comments are
/// skipped. A listed app bypasses [`QUIRKS`] only: the live-target checks here
/// and the plugin's dispatch checks still apply.
const USER_APPS: &str = "cua-driver/hyprland-apps.txt";

/// Who admitted a target that passed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Admission {
    /// No known quirk applies.
    Driver,
    /// A quirk applies, and the user's list admitted the executable anyway.
    User,
}

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
pub(super) fn admit(pid: u32, address: u64, keyboard: bool) -> Result<Admission, &'static str> {
    let window = super::hyprland::window_for_address(address).ok_or("target_unavailable")?;
    let user_apps = dirs::config_dir().map(|dir| dir.join(USER_APPS));
    admit_target(
        pid,
        window.pid,
        window.xwayland,
        keyboard,
        QUIRKS,
        user_apps.as_deref(),
    )
}

/// `window_pid` and `xwayland` are what the compositor reports for the window.
/// `user_apps` is the path of the user's list, read only when a quirk applies.
fn admit_target(
    pid: u32,
    window_pid: u32,
    xwayland: Option<bool>,
    keyboard: bool,
    quirks: &[Quirk],
    user_apps: Option<&Path>,
) -> Result<Admission, &'static str> {
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
        return Ok(Admission::Driver);
    }
    let Some(quirk) = quirk_in(BufReader::new(maps), &applicable) else {
        return Ok(Admission::Driver);
    };
    let listed = std::fs::read_link(format!("/proc/{pid}/exe")).is_ok_and(|executable| {
        user_apps
            .map(user_listed)
            .unwrap_or_default()
            .contains(&executable)
    });
    if listed {
        Ok(Admission::User)
    } else {
        Err(quirk.reason)
    }
}

/// The canonical executables named in the user's list. A missing or
/// unreadable list, and entries that are relative or do not resolve, name
/// nothing.
fn user_listed(path: &Path) -> Vec<PathBuf> {
    let mut text = String::new();
    let read =
        std::fs::File::open(path).and_then(|file| file.take(64 * 1024).read_to_string(&mut text));
    if read.is_err() {
        return Vec::new();
    }
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(Path::new)
        .filter(|entry| entry.is_absolute())
        .filter_map(|entry| std::fs::canonicalize(entry).ok())
        .collect()
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
                admit_target(pid, pid, Some(false), keyboard, QUIRKS, None),
                Ok(Admission::Driver)
            );
        }
    }

    #[test]
    fn xwayland_and_unattested_windows_refuse() {
        let pid = std::process::id();
        assert_eq!(
            admit_target(pid, pid, Some(true), false, QUIRKS, None),
            Err("client_xwayland")
        );
        assert_eq!(
            admit_target(pid, pid, None, false, QUIRKS, None),
            Err("client_unattested")
        );
    }

    #[test]
    fn window_of_another_process_or_a_dead_one_refuses() {
        let pid = std::process::id();
        assert_eq!(
            admit_target(pid, pid + 1, Some(false), false, QUIRKS, None),
            Err("target_unavailable")
        );
        assert_eq!(
            admit_target(u32::MAX, u32::MAX, Some(false), false, QUIRKS, None),
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
            admit_target(pid, pid, Some(false), true, &[QT_KEYS], None),
            Ok(Admission::Driver)
        );
    }

    /// A keyboard quirk naming a library this test process has mapped.
    fn quirk_matching_this_process() -> Quirk {
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
        Quirk {
            library,
            keyboard: true,
            reason: "client_quirk_test",
        }
    }

    #[test]
    fn a_quirk_matching_this_process_refuses_only_its_input_kind() {
        let pid = std::process::id();
        let quirk = quirk_matching_this_process();
        assert_eq!(
            admit_target(
                pid,
                pid,
                Some(false),
                true,
                std::slice::from_ref(&quirk),
                None
            ),
            Err("client_quirk_test")
        );
        assert_eq!(
            admit_target(
                pid,
                pid,
                Some(false),
                false,
                std::slice::from_ref(&quirk),
                None
            ),
            Ok(Admission::Driver)
        );
    }

    fn user_list(content: &str) -> tempfile::NamedTempFile {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut file, content.as_bytes()).unwrap();
        file
    }

    #[test]
    fn a_user_listed_executable_is_admitted_past_its_quirk() {
        let pid = std::process::id();
        let quirk = quirk_matching_this_process();
        let exe = std::env::current_exe().unwrap();
        let list = user_list(&format!("# qualified by me\n\n  {}  \n", exe.display()));
        assert_eq!(
            admit_target(
                pid,
                pid,
                Some(false),
                true,
                std::slice::from_ref(&quirk),
                Some(list.path())
            ),
            Ok(Admission::User)
        );
        // An app with no applicable quirk stays the driver's admission.
        assert_eq!(
            admit_target(
                pid,
                pid,
                Some(false),
                false,
                std::slice::from_ref(&quirk),
                Some(list.path())
            ),
            Ok(Admission::Driver)
        );
    }

    #[test]
    fn the_user_list_does_not_lift_live_target_refusals() {
        let pid = std::process::id();
        let exe = std::env::current_exe().unwrap();
        let list = user_list(&format!("{}\n", exe.display()));
        assert_eq!(
            admit_target(pid, pid, Some(true), true, QUIRKS, Some(list.path())),
            Err("client_xwayland")
        );
        assert_eq!(
            admit_target(pid, pid, None, true, QUIRKS, Some(list.path())),
            Err("client_unattested")
        );
        assert_eq!(
            admit_target(pid, pid + 1, Some(false), true, QUIRKS, Some(list.path())),
            Err("target_unavailable")
        );
    }

    #[test]
    fn unlisted_relative_or_missing_entries_keep_the_quirk() {
        let pid = std::process::id();
        let quirk = quirk_matching_this_process();
        let exe = std::env::current_exe().unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap();
        let other = user_list("/usr/bin/true\n/nonexistent/app\n");
        let relative = user_list(&format!("{name}\n"));
        let commented = user_list(&format!("# {}\n", exe.display()));
        let missing = other.path().with_extension("absent");
        for list in [other.path(), relative.path(), commented.path(), &missing] {
            assert_eq!(
                admit_target(
                    pid,
                    pid,
                    Some(false),
                    true,
                    std::slice::from_ref(&quirk),
                    Some(list)
                ),
                Err("client_quirk_test"),
                "{}",
                list.display()
            );
        }
    }

    #[test]
    fn a_listed_symlink_names_the_executable_it_resolves_to() {
        let pid = std::process::id();
        let quirk = quirk_matching_this_process();
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("app");
        std::os::unix::fs::symlink(std::env::current_exe().unwrap(), &link).unwrap();
        let list = user_list(&format!("{}\n", link.display()));
        assert_eq!(
            admit_target(
                pid,
                pid,
                Some(false),
                true,
                std::slice::from_ref(&quirk),
                Some(list.path())
            ),
            Ok(Admission::User)
        );
    }
}
