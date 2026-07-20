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

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::time::Duration;

use crate::x11::WindowInfo;

const DEST: &str = "org.cua.WinRects";
const PATH: &str = "/org/cua/WinRects";
const IFACE: &str = "org.cua.WinRects";
const DBUS_DEST: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
const BROWSER_HELPER_API_VERSION: u32 = 4;

#[derive(Debug, Clone)]
struct ShellWindow {
    info: WindowInfo,
    focused: bool,
}

pub fn available() -> bool {
    shell_owner(false).is_some()
}

fn gdbus_call(method: &str, args: &[String]) -> Option<String> {
    let owner = shell_owner(false)?;
    gdbus_call_to(
        &owner,
        PATH,
        &format!("{IFACE}.{method}"),
        args,
        Duration::from_millis(800),
    )
}

fn gdbus_call_with_timeout(method: &str, args: &[String], timeout: Duration) -> Option<String> {
    let owner = shell_owner(false)?;
    gdbus_call_to(&owner, PATH, &format!("{IFACE}.{method}"), args, timeout)
}

fn trusted_gdbus_call(method: &str, args: &[String]) -> Option<String> {
    let owner = shell_owner(true)?;
    gdbus_call_to(
        &owner,
        PATH,
        &format!("{IFACE}.{method}"),
        args,
        Duration::from_millis(800),
    )
}

fn gdbus_call_to(
    destination: &str,
    object_path: &str,
    method: &str,
    args: &[String],
    timeout: Duration,
) -> Option<String> {
    let mut cmd = Command::new("gdbus");
    cmd.arg("call")
        .arg("--session")
        .arg("--dest")
        .arg(destination)
        .arg("--object-path")
        .arg(object_path)
        .arg("--method")
        .arg(method);
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
    let out = wait_timeout(child, timeout)?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Resolve the helper's immutable unique bus name and prove that it is hosted
/// by this user's system-installed GNOME Shell process. Browser-sensitive
/// callers additionally require the current helper API. Calling the unique
/// name closes the race where another process replaces the well-known name
/// after ownership is checked.
fn shell_owner(require_browser_api: bool) -> Option<String> {
    let owner_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetNameOwner"),
        &[DEST.to_owned()],
        Duration::from_millis(800),
    )?;
    let owner = parse_quoted_string(&owner_raw)?;
    if !owner.starts_with(':') {
        return None;
    }

    let pid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixProcessID"),
        &[owner.clone()],
        Duration::from_millis(800),
    )?;
    let uid_raw = gdbus_call_to(
        DBUS_DEST,
        DBUS_PATH,
        &format!("{DBUS_IFACE}.GetConnectionUnixUser"),
        &[owner.clone()],
        Duration::from_millis(800),
    )?;
    let pid = parse_first_u32(&pid_raw)?;
    let uid = parse_first_u32(&uid_raw)?;
    if uid != current_uid() || !is_trusted_gnome_shell(pid) {
        return None;
    }

    if require_browser_api {
        let version_raw = gdbus_call_to(
            &owner,
            PATH,
            &format!("{IFACE}.GetVersion"),
            &[],
            Duration::from_millis(800),
        )?;
        if parse_first_u32(&version_raw)? < BROWSER_HELPER_API_VERSION {
            return None;
        }
    }
    Some(owner)
}

fn parse_quoted_string(raw: &str) -> Option<String> {
    let start = raw.find('\'')? + 1;
    let end = raw[start..].find('\'')? + start;
    (end > start).then(|| raw[start..end].to_owned())
}

fn parse_first_u32(raw: &str) -> Option<u32> {
    // `gdbus call` renders typed scalars as `(uint32 6079,)`. Searching the
    // whole string would incorrectly return the `32` in the type annotation.
    let payload = raw.split_once("uint32").map_or(raw, |(_, payload)| payload);
    payload
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| !part.is_empty())?
        .parse()
        .ok()
}

fn current_uid() -> u32 {
    std::fs::metadata("/proc/self")
        .map(|meta| meta.uid())
        .unwrap_or(u32::MAX)
}

fn is_trusted_gnome_shell(pid: u32) -> bool {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok();
    if comm.as_deref().map(str::trim) != Some("gnome-shell") {
        return false;
    }
    let executable = std::fs::read_link(format!("/proc/{pid}/exe")).ok();
    let metadata = executable
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok());
    executable
        .as_ref()
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
        == Some("gnome-shell")
        && metadata
            .as_ref()
            .is_some_and(|meta| meta.uid() == 0 && meta.permissions().mode() & 0o022 == 0)
}

/// Capture the GNOME stage through the compositor helper.
///
/// Mutter does not expose wlroots screencopy protocols, and its one-shot
/// Screenshot portal may reject an unregistered command-line process. The
/// opt-in helper already runs inside Shell for geometry and activation, so it
/// can use Shell's screenshot API without confusing a stable Wayland window id
/// for an X11 drawable.
pub fn screenshot_display() -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

    let raw = gdbus_call_with_timeout("Capture", &[], Duration::from_secs(5))?;
    let start = raw.find('\'')? + 1;
    let end = raw.rfind('\'')?;
    if end <= start {
        return None;
    }
    B64.decode(&raw[start..end]).ok()
}

/// `Child::wait` with a deadline (no extra crates). Kills + reaps on timeout.
fn wait_timeout(mut child: std::process::Child, dur: Duration) -> Option<std::process::Output> {
    use std::io::Read;

    // Drain stdout while the child is running. Capture() returns a base64 PNG
    // that readily exceeds a pipe's ~64 KiB capacity; waiting for exit before
    // reading deadlocks the child on a full pipe and turns a healthy Shell
    // response into a false timeout.
    let stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut stdout = stdout;
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).ok()?;
        Some(bytes)
    });
    let deadline = std::time::Instant::now() + dur;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let status = child.wait().ok()?;
                    let _ = reader.join();
                    if !status.success() {
                        return None;
                    }
                    return None;
                }
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return None;
            }
        }
    };
    let stdout = reader.join().ok().flatten()?;
    Some(std::process::Output {
        status,
        stdout,
        stderr: Vec::new(),
    })
}

/// Screen origin of the Wayland surface buffer backing `pid`.
///
/// GTK's AT-SPI `CoordType::Window` includes client-side shadow extents, while
/// Mutter's frame rectangle excludes them. The buffer origin preserves those
/// extents so accessibility frames line up with pixels. Older helpers omit the
/// buffer fields and fall back to the frame origin.
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
            let x = w
                .get("buffer_x")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| w.get("x").and_then(serde_json::Value::as_i64))?
                as i32;
            let y = w
                .get("buffer_y")
                .and_then(serde_json::Value::as_i64)
                .or_else(|| w.get("y").and_then(serde_json::Value::as_i64))?
                as i32;
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

/// Return one compositor-attested GNOME window only when the current helper
/// API is hosted by the verified Shell owner.
pub fn trusted_window_for_id(pid: u32, window_id: u64) -> Option<WindowInfo> {
    trusted_shell_windows(Some(pid))?
        .into_iter()
        .find(|window| window.info.xid == window_id)
        .map(|window| window.info)
}

/// Enumerate exact browser-window ids for one process through the verified
/// GNOME Shell helper. `None` means no trusted helper is available; an empty
/// vector means the trusted helper found no owned windows.
pub fn trusted_window_ids_for_pid(pid: u32) -> Option<Vec<u64>> {
    Some(
        trusted_shell_windows(Some(pid))?
            .into_iter()
            .map(|window| window.info.xid)
            .collect(),
    )
}

/// Briefly activate an exact compositor-owned window, execute one bounded
/// focus-sensitive operation, and restore the prior exact Shell focus.
pub fn with_focused_window<T>(
    pid: u32,
    window_id: u64,
    body: impl FnOnce() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let before = trusted_shell_windows(None)
        .ok_or_else(|| anyhow::anyhow!("the verified GNOME Shell helper API is unavailable"))?;
    let target = before
        .iter()
        .find(|window| window.info.pid == Some(pid) && window.info.xid == window_id)
        .ok_or_else(|| anyhow::anyhow!("no exact GNOME Shell window owns the approved target"))?;
    let previous = before
        .iter()
        .find(|window| window.focused)
        .map(|window| window.info.xid)
        .ok_or_else(|| anyhow::anyhow!("GNOME Shell did not expose a restorable focused window"))?;

    if target.focused {
        return body();
    }
    trusted_activate_window(window_id)
        .then_some(())
        .ok_or_else(|| anyhow::anyhow!("GNOME Shell did not confirm exact target activation"))?;
    let body_result = body();
    let restored = trusted_activate_window(previous);
    match (body_result, restored) {
        (Ok(value), true) => Ok(value),
        (Err(error), true) => Err(error),
        (Ok(_), false) => {
            anyhow::bail!("GNOME Shell did not restore the previously focused window")
        }
        (Err(error), false) => Err(anyhow::anyhow!(
            "{error}; GNOME Shell also failed to restore the previously focused window"
        )),
    }
}

fn trusted_shell_windows(filter_pid: Option<u32>) -> Option<Vec<ShellWindow>> {
    let raw = trusted_gdbus_call("GetRects", &[])?;
    parse_shell_windows(&raw, filter_pid)
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

fn trusted_activate_window(window_id: u64) -> bool {
    let Ok(window_id) = u32::try_from(window_id) else {
        return false;
    };
    let accepted = trusted_gdbus_call("Activate", &[window_id.to_string()])
        .is_some_and(|output| output.trim_start().starts_with("(true,"));
    if !accepted {
        return false;
    }
    std::thread::sleep(Duration::from_millis(60));
    trusted_shell_windows(None).is_some_and(|windows| {
        windows
            .into_iter()
            .any(|window| window.info.xid == u64::from(window_id) && window.focused)
    })
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
    Some(
        parse_shell_windows(raw, filter_pid)?
            .into_iter()
            .map(|window| window.info)
            .collect(),
    )
}

fn parse_shell_windows(raw: &str, filter_pid: Option<u32>) -> Option<Vec<ShellWindow>> {
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

                Some(ShellWindow {
                    info: WindowInfo {
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
                    },
                    focused: window
                        .get("focused")
                        .and_then(serde_json::Value::as_bool)
                        .unwrap_or(false),
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

    #[test]
    fn parses_dbus_owner_and_numeric_identity() {
        assert_eq!(
            parse_quoted_string("(':1.204',)"),
            Some(":1.204".to_owned())
        );
        assert_eq!(parse_first_u32("(uint32 6079,)"), Some(6079));
        assert_eq!(parse_first_u32("(uint32 4,)"), Some(4));
        assert_eq!(parse_first_u32("(6079,)"), Some(6079));
        assert_eq!(parse_quoted_string("(nothing,)"), None);
        assert_eq!(parse_first_u32("(nothing,)"), None);
    }

    #[test]
    fn preserves_exact_focus_from_shell_snapshot() {
        let raw = r#"('[{"id":46,"pid":6079,"title":"Target","x":66,"y":32,"w":958,"h":736,"focused":false,"minimized":false,"visible":true,"stacking":2},{"id":47,"pid":6080,"title":"Sentinel","x":0,"y":0,"w":100,"h":100,"focused":true,"minimized":false,"visible":true,"stacking":3}]',)"#;
        let windows = parse_shell_windows(raw, None).expect("valid helper response");
        assert_eq!(windows.len(), 2);
        assert!(!windows[0].focused);
        assert!(windows[1].focused);
        assert_eq!(windows[1].info.xid, 47);
    }
}
