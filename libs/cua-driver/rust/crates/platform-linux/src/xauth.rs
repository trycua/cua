//! Xwayland `XAUTHORITY` auto-discovery for SSH-driven Wayland sessions.
//!
//! When cua-driver is driven over SSH against a Wayland+Xwayland desktop,
//! `DISPLAY` is typically inherited (e.g. `:0`) but `XAUTHORITY` is not.
//! Xwayland authenticates X11 clients with a per-session MIT cookie passed to
//! it via its own `-auth <file>` argument, which an X11 client cannot guess —
//! so every X11 connect fails with "Authorization required, but no
//! authorization protocol specified" (empty `list_windows`, failing
//! `get_screen_size` / `get_cursor_position`, overlay connect errors). See
//! issue #1926.
//!
//! [`ensure_xauthority_discovered`] recovers the cookie by reading the running
//! Xwayland (or Xorg) process's `-auth` argument out of `/proc` and exporting
//! it as `XAUTHORITY` before the first X11 connect. It is a no-op when
//! `XAUTHORITY` is already set or there is no `DISPLAY`, and runs at most once.

use std::sync::OnceLock;

static DONE: OnceLock<()> = OnceLock::new();

/// Discover and export `XAUTHORITY` from the running X server's `-auth`
/// argument when it is unset on a session that has a `DISPLAY`. Idempotent
/// (runs once per process) and safe to call eagerly at daemon startup.
pub fn ensure_xauthority_discovered() {
    DONE.get_or_init(|| {
        // Already have an explicit cookie, or no X11 DISPLAY to authenticate
        // against — nothing to do.
        if std::env::var_os("XAUTHORITY").is_some() {
            return;
        }
        if std::env::var_os("DISPLAY").is_none() {
            return;
        }
        if let Some(auth) = discover_xauth_from_proc() {
            // SAFETY: set early at startup, before any X11 connect / worker
            // thread reads the environment.
            std::env::set_var("XAUTHORITY", &auth);
            tracing::info!(
                target: "cua-driver",
                "XAUTHORITY was unset; adopted the running X server's auth cookie at {auth} (#1926)"
            );
        } else {
            tracing::debug!(
                target: "cua-driver",
                "XAUTHORITY unset and no X server -auth cookie found in /proc; \
                 X11 connects may fail with 'Authorization required' (#1926)"
            );
        }
    });
}

/// Scan `/proc` for an X server process (`Xwayland`, `Xorg`, or `X`) and return
/// the existing file named by its `-auth <file>` argument, if any.
fn discover_xauth_from_proc() -> Option<String> {
    let proc_dir = std::fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let pid_dir = entry.path();
        // `comm` is the (possibly truncated) executable name + trailing newline.
        let comm = std::fs::read_to_string(pid_dir.join("comm")).unwrap_or_default();
        match comm.trim() {
            "Xwayland" | "Xorg" | "X" => {}
            _ => continue,
        }
        // `cmdline` is NUL-separated argv. Find `-auth` and take the next arg.
        let cmdline = match std::fs::read(pid_dir.join("cmdline")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
        for pair in args.windows(2) {
            if pair[0] == b"-auth" {
                if let Ok(path) = std::str::from_utf8(pair[1]) {
                    if !path.is_empty() && std::path::Path::new(path).exists() {
                        return Some(path.to_owned());
                    }
                }
            }
        }
    }
    None
}
