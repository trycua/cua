//! Session D-Bus address auto-discovery for out-of-session daemons.
//!
//! AT-SPI is reached entirely over the **session** bus (see [`crate::a11y`] and
//! [`crate::atspi`]): the accessibility-bus launcher, every toolkit's AT-SPI
//! bridge, and `EditableText.insertText` all connect via
//! `zbus::Connection::session()`, which resolves the bus from
//! `DBUS_SESSION_BUS_ADDRESS`.
//!
//! When the daemon is started *inside* the desktop session that variable is
//! already exported. But when it is started **outside** it — a container
//! entrypoint, a headless box, `runuser`/`su` into the desktop user, or a
//! systemd *system* unit — `DBUS_SESSION_BUS_ADDRESS` is unset. Then
//! `Connection::session()` has nothing to connect to, the AT-SPI registry walk
//! comes back empty, and `get_window_state` reports every window as having no
//! elements. This is the single most common reason AT-SPI "silently does
//! nothing" on Linux, and it is exactly the failure seen driving cua-driver in
//! the XFCE-in-a-container test rig (the VNC session runs an ad-hoc bus whose
//! address only that session's processes know).
//!
//! [`ensure_session_bus_discovered`] recovers the address the same way
//! [`crate::xauth`] recovers the X11 auth cookie: it prefers the standard
//! systemd user-bus socket at `/run/user/<uid>/bus`, and otherwise reads
//! `DBUS_SESSION_BUS_ADDRESS` out of a running desktop-session process's
//! `/proc/<pid>/environ`, exporting it before the first session-bus connect.
//! It is a no-op when the variable is already set and runs at most once.

use std::sync::OnceLock;

static DONE: OnceLock<()> = OnceLock::new();

/// `comm` names (15-char-truncated by the kernel) of long-lived desktop-session
/// leaders that carry `DBUS_SESSION_BUS_ADDRESS` in their environment. Matched
/// by prefix in both directions so the truncation (`gnome-session-binary` →
/// `gnome-session-b`, `cinnamon-session` → `cinnamon-sessio`) still hits.
const SESSION_PROCESS_COMMS: &[&str] = &[
    "xfce4-session",
    "xfsettingsd",
    "xfwm4",
    "gnome-session-binary",
    "gnome-shell",
    "mate-session",
    "lxqt-session",
    "cinnamon-session",
    "plasma_session",
    "ksmserver",
    "plasmashell",
    "sway",
    "labwc",
    "dbus-daemon",
];

/// Discover and export `DBUS_SESSION_BUS_ADDRESS` when it is unset, so AT-SPI
/// works even when the daemon was started outside the desktop session
/// (container, headless, `runuser`, systemd system unit). Idempotent (runs once
/// per process) and safe to call eagerly at daemon startup.
pub fn ensure_session_bus_discovered() {
    DONE.get_or_init(|| {
        if std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some() {
            return;
        }
        if let Some(address) = discover_session_bus_address() {
            // SAFETY: set early at startup, before any session-bus connect /
            // worker thread reads the environment.
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
            tracing::info!(
                target: "cua-driver",
                "DBUS_SESSION_BUS_ADDRESS was unset; adopted the desktop session bus at {address} \
                 so AT-SPI works without a session-attached launch"
            );
        } else {
            tracing::debug!(
                target: "cua-driver",
                "DBUS_SESSION_BUS_ADDRESS unset and no session bus found \
                 (no /run/user/<uid>/bus socket, no desktop-session process exposing it); \
                 AT-SPI trees may stay empty — start the daemon inside the desktop session"
            );
        }
    });
}

/// Prefer the standard systemd user-bus socket; fall back to reading the
/// address from a running desktop-session process's environment.
fn discover_session_bus_address() -> Option<String> {
    let uid = current_uid();
    let standard_socket = format!("/run/user/{uid}/bus");
    if std::path::Path::new(&standard_socket).exists() {
        return Some(format!("unix:path={standard_socket}"));
    }
    discover_from_session_process_environ()
}

/// The daemon's real uid, read from `/proc/self` (no libc dependency). Falls
/// back to 0 if `/proc` is unreadable — the standard-socket probe then just
/// misses and we go to the environ scan.
fn current_uid() -> u32 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|m| m.uid())
        .unwrap_or(0)
}

/// Scan `/proc` for a desktop-session leader and return the
/// `DBUS_SESSION_BUS_ADDRESS` from its environment, if any. Only processes the
/// daemon's uid owns are readable; that is exactly the desktop user's session,
/// which is what we want.
fn discover_from_session_process_environ() -> Option<String> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let pid_dir = entry.path();
        let comm = std::fs::read_to_string(pid_dir.join("comm")).unwrap_or_default();
        let comm = comm.trim();
        if comm.is_empty() || !is_session_process(comm) {
            continue;
        }
        // `environ` is NUL-separated KEY=VALUE; unreadable for other-uid procs.
        let environ = match std::fs::read(pid_dir.join("environ")) {
            Ok(e) => e,
            Err(_) => continue,
        };
        if let Some(address) = dbus_address_from_environ(&environ) {
            return Some(address);
        }
    }
    None
}

/// Extract a non-empty `DBUS_SESSION_BUS_ADDRESS` from a NUL-separated
/// `/proc/<pid>/environ` blob. Split out so the parse is unit-tested without a
/// live process.
fn dbus_address_from_environ(environ: &[u8]) -> Option<String> {
    for kv in environ.split(|&b| b == 0) {
        if let Some(value) = kv.strip_prefix(b"DBUS_SESSION_BUS_ADDRESS=") {
            if let Ok(address) = std::str::from_utf8(value) {
                if !address.is_empty() {
                    return Some(address.to_owned());
                }
            }
        }
    }
    None
}

/// True when `comm` looks like a desktop-session leader, tolerating the
/// kernel's 15-char `comm` truncation in either direction. Empty `comm` never
/// matches (every string "starts with" the empty string, so it must be
/// excluded explicitly).
fn is_session_process(comm: &str) -> bool {
    if comm.is_empty() {
        return false;
    }
    SESSION_PROCESS_COMMS
        .iter()
        .any(|known| known.starts_with(comm) || comm.starts_with(known))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_session_leaders_with_comm_truncation() {
        // Full names.
        assert!(is_session_process("xfce4-session"));
        assert!(is_session_process("sway"));
        // Kernel-truncated comm (15 chars).
        assert!(is_session_process("gnome-session-b")); // gnome-session-binary
        assert!(is_session_process("cinnamon-sessio")); // cinnamon-session
                                                        // Non-session processes are ignored.
        assert!(!is_session_process("bash"));
        assert!(!is_session_process("cua-driver"));
        assert!(!is_session_process(""));
    }

    #[test]
    fn standard_socket_address_is_unix_path() {
        // Pure formatting check — the discovery itself is environment-dependent.
        let uid = current_uid();
        let socket = format!("/run/user/{uid}/bus");
        assert!(format!("unix:path={socket}").starts_with("unix:path=/run/user/"));
    }

    #[test]
    fn parses_dbus_address_from_environ_blob() {
        // Realistic NUL-separated /proc/<pid>/environ.
        let environ = b"PATH=/usr/bin\0DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus\0HOME=/home/cua\0";
        assert_eq!(
            dbus_address_from_environ(environ).as_deref(),
            Some("unix:path=/run/user/1000/bus")
        );
        // Not anchored to the first entry, and tolerates a trailing NUL.
        let mid = b"A=1\0DBUS_SESSION_BUS_ADDRESS=unix:abstract=/tmp/dbus-xyz\0";
        assert_eq!(
            dbus_address_from_environ(mid).as_deref(),
            Some("unix:abstract=/tmp/dbus-xyz")
        );
    }

    #[test]
    fn dbus_address_absent_or_empty_is_none() {
        assert_eq!(dbus_address_from_environ(b"PATH=/usr/bin\0HOME=/x\0"), None);
        // Present but empty value must not match (avoids exporting "").
        assert_eq!(
            dbus_address_from_environ(b"DBUS_SESSION_BUS_ADDRESS=\0"),
            None
        );
        assert_eq!(dbus_address_from_environ(b""), None);
        // A key that merely contains the name as a substring must not match.
        assert_eq!(
            dbus_address_from_environ(b"OLD_DBUS_SESSION_BUS_ADDRESS=x\0"),
            None
        );
    }
}
