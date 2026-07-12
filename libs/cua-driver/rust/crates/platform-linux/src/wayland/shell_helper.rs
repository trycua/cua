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

use crate::x11::WindowInfo;

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

/// Enumerate GNOME Shell toplevels when the compositor helper is available.
///
/// AT-SPI remains the source of accessibility elements, but it is a poor
/// source of truth for desktop window discovery: one unresponsive application
/// can exhaust the bounded registry walk and hide every healthy toplevel. The
/// shell already owns the authoritative stacking list, geometry, visibility,
/// title, and PID, so use that metadata directly for `list_windows`.
pub fn list_windows(filter_pid: Option<u32>) -> Option<Vec<WindowInfo>> {
    let raw = gdbus_call("GetRects", &[])?;
    parse_windows(&raw, filter_pid)
}

/// Ask GNOME Shell to focus and raise one stable-sequence window.
///
/// Returns `false` when the helper is absent, the id is unknown, or Shell did
/// not confirm focus. Callers must not inject global libei input unless this
/// returns true: portal input is focus-bound and otherwise targets whichever
/// application the user happened to be using.
pub fn activate_window(window_id: u64) -> bool {
    let Ok(window_id) = u32::try_from(window_id) else {
        return false;
    };
    let accepted = gdbus_call("Activate", &[window_id.to_string()])
        .is_some_and(|output| output.trim_start().starts_with("(true,"));
    if !accepted {
        return false;
    }
    std::thread::sleep(Duration::from_millis(60));
    window_is_focused(window_id)
}

fn window_is_focused(window_id: u32) -> bool {
    let Some(raw) = gdbus_call("GetRects", &[]) else {
        return false;
    };
    let (Some(start), Some(end)) = (raw.find('['), raw.rfind(']')) else {
        return false;
    };
    serde_json::from_str::<Vec<serde_json::Value>>(&raw[start..=end])
        .ok()
        .and_then(|windows| {
            windows.into_iter().find(|window| {
                window.get("id").and_then(serde_json::Value::as_u64) == Some(window_id as u64)
            })
        })
        .and_then(|window| window.get("focused").and_then(serde_json::Value::as_bool))
        .unwrap_or(false)
}

fn parse_windows(raw: &str, filter_pid: Option<u32>) -> Option<Vec<WindowInfo>> {
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    let windows: Vec<serde_json::Value> = serde_json::from_str(&raw[start..=end]).ok()?;

    Some(
        windows
            .into_iter()
            .filter_map(|window| {
                let pid = u32::try_from(window.get("pid")?.as_u64()?).ok()?;
                if filter_pid.is_some_and(|wanted| wanted != pid) {
                    return None;
                }
                let id = window.get("id")?.as_u64()?.max(1);
                let x = i32::try_from(window.get("x")?.as_i64()?).ok()?;
                let y = i32::try_from(window.get("y")?.as_i64()?).ok()?;
                let width = u32::try_from(window.get("w")?.as_u64()?).ok()?;
                let height = u32::try_from(window.get("h")?.as_u64()?).ok()?;
                let visible = window
                    .get("visible")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(width > 0 && height > 0);
                let minimized = window
                    .get("minimized")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let title = window
                    .get("title")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let z_index = window
                    .get("stacking")
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| usize::try_from(value).ok());

                Some(WindowInfo {
                    xid: id,
                    pid: Some(pid),
                    app_name: title.clone(),
                    title,
                    is_on_screen: visible && !minimized && width > 0 && height > 0,
                    z_index,
                    x,
                    y,
                    width,
                    height,
                })
            })
            .collect(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_filters_shell_windows() {
        let raw = r#"('[{"id":46,"pid":6079,"title":"Sentinel's window","x":66,"y":32,"w":958,"h":736,"focused":true,"minimized":false,"visible":true,"stacking":2},{"id":47,"pid":6080,"title":"Hidden","x":0,"y":0,"w":100,"h":100,"minimized":true,"visible":false,"stacking":1}]',)"#;
        let windows = parse_windows(raw, Some(6079)).expect("valid helper response");
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].xid, 46);
        assert_eq!(windows[0].pid, Some(6079));
        assert_eq!(windows[0].title, "Sentinel's window");
        assert_eq!((windows[0].x, windows[0].y), (66, 32));
        assert_eq!((windows[0].width, windows[0].height), (958, 736));
        assert!(windows[0].is_on_screen);
        assert_eq!(windows[0].z_index, Some(2));
    }

    #[test]
    fn marks_minimized_shell_windows_off_screen() {
        let raw = r#"('[{"id":47,"pid":6080,"title":"Hidden","x":0,"y":0,"w":100,"h":100,"minimized":true,"visible":false,"stacking":1}]',)"#;
        let windows = parse_windows(raw, None).expect("valid helper response");
        assert_eq!(windows.len(), 1);
        assert!(!windows[0].is_on_screen);
    }
}
