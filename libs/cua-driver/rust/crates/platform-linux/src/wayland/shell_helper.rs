//! Client for the bundled **cua WinRects** GNOME Shell extension
//! (`org.cua.WinRects`, see `wayland-helper/winrects@cua/`).
//!
//! On GNOME Mutter (and other non-wlroots compositors) a normal client cannot
//! query a window's on-screen origin (`org.gnome.Shell.Introspect.GetWindows`
//! is privacy-denied; Wayland exposes no global coordinates) nor position an
//! overlay surface at a screen coordinate (no `zwlr_layer_shell_v1`). Both are
//! solvable only from *inside* the compositor — which is what the extension is
//! for. It runs in the shell's privileged context and exposes:
//!
//! - `GetRects() -> json` — every window's `meta_window.get_frame_rect()` (screen
//!   geometry), so `screen_xy = window_origin + AT-SPI CoordType::Window xy`
//!   (the GNOME analogue of the X11 `_GTK_FRAME_EXTENTS` reconstruction).
//! - `MoveCursor(x,y)` / `ClickPulse(x,y)` / `HideCursor()` — draw the agent
//!   cursor as a Clutter actor on the compositor stage.
//!
//! Everything here is **best-effort**: if the extension isn't installed/enabled
//! the calls return `None` / no-op and callers keep the prior behaviour (no
//! screen coords, no Wayland cursor). Uses a short-lived `gdbus` subprocess so
//! there's no zbus blocking-feature or async-context coupling — the calls are
//! infrequent (once per `get_window_state`, a few per click).

use std::process::Command;
use std::time::Duration;

const DEST: &str = "org.cua.WinRects";
const PATH: &str = "/org/cua/WinRects";
const IFACE: &str = "org.cua.WinRects";

fn gdbus_call(method: &str, args: &[String]) -> Option<String> {
    let mut cmd = Command::new("gdbus");
    cmd.arg("call")
        .arg("--session")
        .arg("--dest")
        .arg(DEST)
        .arg("--object-path")
        .arg(PATH)
        .arg("--method")
        .arg(format!("{IFACE}.{method}"));
    for a in args {
        cmd.arg(a);
    }
    // gdbus is local IPC; cap it so a wedged shell can't stall the caller.
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let out = wait_timeout(child, Duration::from_millis(800))?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `Child::wait` with a deadline (no extra crates). Kills + reaps on timeout.
fn wait_timeout(mut child: std::process::Child, dur: Duration) -> Option<std::process::Output> {
    let deadline = std::time::Instant::now() + dur;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => return None,
        }
    }
}

/// Screen origin (x, y) of the window backing `pid`, from the extension's
/// `GetRects`. `None` when the extension is unavailable or no window matches.
pub fn window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    let raw = gdbus_call("GetRects", &[])?;
    // gdbus prints a GVariant tuple like `('[{"pid":..,"x":..}]',)`. Pull the
    // JSON array out robustly (first '[' .. last ']') rather than parsing the
    // GVariant wrapper, so an apostrophe in a window title can't break it.
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    let json = &raw[start..=end];
    let arr: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    for w in &arr {
        if w.get("pid").and_then(|p| p.as_u64()) == Some(pid as u64) {
            let x = w.get("x")?.as_i64()? as i32;
            let y = w.get("y")?.as_i64()? as i32;
            return Some((x, y));
        }
    }
    None
}

/// Glide the agent cursor to screen `(x, y)`.
pub fn move_cursor(x: i32, y: i32) {
    let _ = gdbus_call("MoveCursor", &[x.to_string(), y.to_string()]);
}

/// Snap + pulse the agent cursor at screen `(x, y)` (a click indicator).
pub fn click_pulse(x: i32, y: i32) {
    let _ = gdbus_call("ClickPulse", &[x.to_string(), y.to_string()]);
}

/// Hide the agent cursor.
pub fn hide_cursor() {
    let _ = gdbus_call("HideCursor", &[]);
}
