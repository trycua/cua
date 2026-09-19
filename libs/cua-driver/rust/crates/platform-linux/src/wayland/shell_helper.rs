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
//! - `SetCursorState(...)` / `SetCursorColor(...)` — keep the compositor cursor
//!   aligned with the shared semantic theme and active session identity.
//!
//! Everything here is **best-effort**: if the extension isn't installed/enabled
//! the calls return `None` / no-op and callers keep the prior behaviour (no
//! screen coords, no Wayland cursor). Uses a short-lived `gdbus` subprocess.
//! Visual updates are serialized on a dedicated thread so helper discovery and
//! subprocess waits never run on the invoking async runtime thread. Geometry,
//! capture and focus calls retain their existing synchronous behavior.

use std::collections::VecDeque;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::process::Command;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::x11::WindowInfo;

const DEST: &str = "org.cua.WinRects";
const PATH: &str = "/org/cua/WinRects";
const IFACE: &str = "org.cua.WinRects";
const DBUS_DEST: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
const BROWSER_HELPER_API_VERSION: u32 = 4;
const SEMANTIC_CURSOR_API_VERSION: u32 = 8;

#[derive(Debug, Clone)]
struct ShellWindow {
    info: WindowInfo,
    focused: bool,
}

pub fn available() -> bool {
    shell_owner(false).is_some()
}

pub fn semantic_cursor_available() -> bool {
    shell_owner_with_min_version(Some(SEMANTIC_CURSOR_API_VERSION)).is_some()
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
    shell_owner_with_min_version(require_browser_api.then_some(BROWSER_HELPER_API_VERSION))
}

fn shell_owner_with_min_version(min_version: Option<u32>) -> Option<String> {
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

    if let Some(min_version) = min_version {
        let version_raw = gdbus_call_to(
            &owner,
            PATH,
            &format!("{IFACE}.GetVersion"),
            &[],
            Duration::from_millis(800),
        )?;
        if parse_first_u32(&version_raw)? < min_version {
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
    let raw = gdbus_call_with_timeout("Capture", &[], Duration::from_secs(5))?;
    decode_capture(&raw)
}

/// Capture the GNOME stage only when the helper owner and current browser API
/// have passed the same compositor-attestation checks used for mutation.
pub fn trusted_screenshot_display() -> Option<Vec<u8>> {
    let owner = shell_owner(true)?;
    let raw = gdbus_call_to(
        &owner,
        PATH,
        &format!("{IFACE}.Capture"),
        &[],
        Duration::from_secs(5),
    )?;
    decode_capture(&raw)
}

fn decode_capture(raw: &str) -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};

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

/// Screen origin of the compositor frame backing `pid`.
///
/// `screenshot_window_dispatch` crops the Shell stage to this same frame
/// rectangle, and GTK's AT-SPI `CoordType::Window` coordinates are relative to
/// that frame. Using Mutter's larger surface-buffer rectangle here shifts
/// elements up and left by the client-side shadow extents whenever the window
/// is floating, so pixel actions derived from the returned screenshot miss
/// their target.
pub fn window_origin_for_pid(pid: u32) -> Option<(i32, i32)> {
    let raw = gdbus_call("GetRects", &[])?;
    parse_window_origin(&raw, pid)
}

fn parse_window_origin(raw: &str, pid: u32) -> Option<(i32, i32)> {
    // gdbus prints a GVariant tuple like `('[{"pid":..,"x":..}]',)`. Pull the
    // JSON array out robustly (first '[' .. last ']') rather than parsing the
    // GVariant wrapper, so an apostrophe in a window title can't break it.
    let start = raw.find('[')?;
    let end = raw.rfind(']')?;
    let json = &raw[start..=end];
    let arr: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    for w in &arr {
        if w.get("pid").and_then(|p| p.as_u64()) == Some(pid as u64) {
            let x = w.get("x").and_then(serde_json::Value::as_i64)? as i32;
            let y = w.get("y").and_then(serde_json::Value::as_i64)? as i32;
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
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        if window_is_focused(window_id) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        if trusted_shell_windows(None).is_some_and(|windows| {
            windows
                .into_iter()
                .any(|window| window.info.xid == u64::from(window_id) && window.focused)
        }) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
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

// One process-wide dispatcher matches the existing helper's single cursor.
// There is no new per-session or persistent-connection helper protocol here.
const VISUAL_QUEUE_CAPACITY: usize = 64;
static VISUAL_DISPATCHER: OnceLock<Option<VisualDispatcher>> = OnceLock::new();

#[derive(Debug, PartialEq, Eq)]
struct VisualRequest {
    method: &'static str,
    args: Vec<String>,
}

impl VisualRequest {
    fn hide() -> Self {
        Self {
            method: "HideCursor",
            args: Vec::new(),
        }
    }

    fn is_hide(&self) -> bool {
        self.method == "HideCursor"
    }
}

#[derive(Default)]
struct VisualQueue {
    pending: VecDeque<VisualRequest>,
    closed: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum VisualQueueError {
    Full,
    Closed,
}

struct VisualDispatcher {
    shared: Arc<(Mutex<VisualQueue>, Condvar)>,
}

impl VisualDispatcher {
    fn spawn(
        mut dispatch: impl FnMut(VisualRequest) + Send + 'static,
    ) -> std::io::Result<(Self, std::thread::JoinHandle<()>)> {
        let shared = Arc::new((Mutex::new(VisualQueue::default()), Condvar::new()));
        // Closing the worker's guard also closes admission if dispatch panics.
        let worker = Self {
            shared: Arc::clone(&shared),
        };
        let thread = std::thread::Builder::new()
            .name("cua-gnome-cursor".to_owned())
            .spawn(move || {
                loop {
                    let request = {
                        let (lock, ready) = &*worker.shared;
                        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
                        while queue.pending.is_empty() && !queue.closed {
                            queue = ready.wait(queue).unwrap_or_else(|e| e.into_inner());
                        }
                        if queue.closed {
                            break;
                        }
                        queue.pending.pop_front().expect("nonempty visual queue")
                    };
                    // Never hold the admission lock over discovery or helper I/O.
                    dispatch(request);
                }
                // Dropping an owned dispatcher discards queued updates and hides
                // after the in-flight call, never concurrently with that call.
                dispatch(VisualRequest::hide());
            })?;
        Ok((Self { shared }, thread))
    }

    fn enqueue(&self, request: VisualRequest) -> Result<(), VisualQueueError> {
        let (lock, ready) = &*self.shared;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        if queue.closed {
            return Err(VisualQueueError::Closed);
        }
        if request.is_hide() {
            // Reserve one terminal slot so removal cannot be lost to a full
            // queue. Consecutive hides are idempotent. Ordinary commands count
            // ALL pending entries toward capacity, keeping the total <= N + 1.
            if queue.pending.back().is_some_and(VisualRequest::is_hide) {
                return Ok(());
            }
        } else if queue.pending.len() >= VISUAL_QUEUE_CAPACITY {
            return Err(VisualQueueError::Full);
        }
        queue.pending.push_back(request);
        ready.notify_one();
        Ok(())
    }
}

impl Drop for VisualDispatcher {
    fn drop(&mut self) {
        let (lock, ready) = &*self.shared;
        let mut queue = lock.lock().unwrap_or_else(|e| e.into_inner());
        queue.closed = true;
        queue.pending.clear();
        ready.notify_one();
        // Do not join here: callers can be on a current-thread async runtime.
    }
}

fn with_visual_dispatcher<R>(body: impl FnOnce(Option<&VisualDispatcher>) -> R) -> R {
    #[cfg(test)]
    if let Some(dispatcher) = tests::VISUAL_OVERRIDE.with(|slot| slot.borrow().clone()) {
        return body(Some(&dispatcher));
    }
    let dispatcher = VISUAL_DISPATCHER.get_or_init(|| {
        match VisualDispatcher::spawn(|request| {
            if gdbus_call(request.method, &request.args).is_none() {
                tracing::debug!(
                    method = request.method,
                    "GNOME visual helper call unavailable"
                );
            }
        }) {
            // This is process-lived, like the existing overlay dispatcher.
            Ok((dispatcher, _thread)) => Some(dispatcher),
            Err(error) => {
                tracing::warn!(%error, "could not start GNOME visual dispatcher");
                None
            }
        }
    });
    body(dispatcher.as_ref())
}

fn enqueue_visual(method: &'static str, args: Vec<String>) {
    let result = with_visual_dispatcher(|dispatcher| {
        dispatcher
            .ok_or(VisualQueueError::Closed)
            .and_then(|dispatcher| dispatcher.enqueue(VisualRequest { method, args }))
    });
    if let Err(error) = result {
        tracing::warn!(method, ?error, "GNOME visual queue rejected command");
    }
}

/// Glide the agent cursor to screen `(x, y)` (best-effort queue admission).
pub fn move_cursor(x: i32, y: i32) {
    enqueue_visual("MoveCursor", vec![x.to_string(), y.to_string()]);
}

/// Snap + pulse the agent cursor at screen `(x, y)` (a click indicator).
pub fn click_pulse(x: i32, y: i32) {
    enqueue_visual("ClickPulse", vec![x.to_string(), y.to_string()]);
}

/// Set the stable session-specific fill color for the compositor cursor.
pub fn set_cursor_color(fill_color: &str) {
    enqueue_visual("SetCursorColor", vec![fill_color.to_owned()]);
}

/// Update the compositor-owned cursor's semantic action state.
///
/// Callers gate this method on helper v8 so an older helper cannot silently
/// render the retired cursor artwork.
pub fn set_cursor_state(action: &str, delivery: &str, target: &str, active: bool) {
    enqueue_visual(
        "SetCursorState",
        vec![
            action.to_owned(),
            delivery.to_owned(),
            target.to_owned(),
            active.to_string(),
        ],
    );
}

/// Set the renderer-visible public session label for the compositor cursor.
pub fn set_session_label(label: &str) {
    enqueue_visual("SetSessionLabel", vec![label.to_owned()]);
}

/// Queue a hide after all previously accepted updates, including on removal.
/// A saturated queue reserves room for this terminal command. Later updates
/// can show the cursor again; session lifetime remains the caller's concern.
pub fn hide_cursor() {
    enqueue_visual("HideCursor", Vec::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXTENSION_SOURCE: &str =
        include_str!("../../../../../wayland-helper/winrects@cua/extension.js");
    const EXTENSION_METADATA: &str =
        include_str!("../../../../../wayland-helper/winrects@cua/metadata.json");

    // Thread-local injection keeps public-route tests independent and prevents
    // them from ever contacting the process-global helper or a real desktop.
    thread_local! {
        pub(super) static VISUAL_OVERRIDE: std::cell::RefCell<Option<Arc<VisualDispatcher>>> =
            const { std::cell::RefCell::new(None) };
    }

    struct VisualOverride;

    impl VisualOverride {
        fn install(dispatcher: VisualDispatcher) -> Self {
            VISUAL_OVERRIDE.with(|slot| {
                assert!(slot.borrow_mut().replace(Arc::new(dispatcher)).is_none());
            });
            Self
        }
    }

    impl Drop for VisualOverride {
        fn drop(&mut self) {
            VISUAL_OVERRIDE.with(|slot| slot.borrow_mut().take());
        }
    }

    fn visual_request(method: &'static str, args: &[&str]) -> VisualRequest {
        VisualRequest {
            method,
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        }
    }

    #[test]
    fn slow_visual_helper_does_not_stall_current_thread_heartbeat() {
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let mut started_tx = Some(started_tx);
        let (dispatcher, worker) = VisualDispatcher::spawn(move |request| {
            assert!(tokio::runtime::Handle::try_current().is_err());
            if !request.is_hide() {
                started_tx.take().unwrap().send(()).unwrap();
                // Only the heartbeat can release this slow injected helper.
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
        })
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dispatcher = VisualOverride::install(dispatcher);
        runtime.block_on(async {
            move_cursor(10, 20);
            tokio::time::timeout(Duration::from_secs(1), started_rx)
                .await
                .unwrap()
                .unwrap();
            for _ in 0..5 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            release_tx.send(()).unwrap();
            // Closing on the runtime thread must not wait for helper I/O.
            drop(dispatcher);
        });
        worker.join().unwrap();
    }

    #[test]
    fn visual_dispatch_preserves_current_helper_methods_arguments_and_order() {
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let (dispatcher, worker) = VisualDispatcher::spawn(move |request| {
            observed_tx.send(request).unwrap();
        })
        .unwrap();
        let requests = [
            visual_request("SetCursorColor", &["#123456"]),
            visual_request("SetCursorState", &["click", "background", "window", "true"]),
            visual_request("SetSessionLabel", &["session"]),
            visual_request("MoveCursor", &["10", "20"]),
            visual_request("ClickPulse", &["30", "40"]),
            visual_request("SetCursorState", &["click", "", "", "false"]),
            VisualRequest::hide(),
        ];
        let dispatcher = VisualOverride::install(dispatcher);
        set_cursor_color("#123456");
        set_cursor_state("click", "background", "window", true);
        set_session_label("session");
        move_cursor(10, 20);
        click_pulse(30, 40);
        set_cursor_state("click", "", "", false);
        hide_cursor();
        for expected in requests {
            assert_eq!(
                observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                expected
            );
        }
        drop(dispatcher);
        worker.join().unwrap();
        assert_eq!(observed_rx.recv().unwrap(), VisualRequest::hide());
        assert!(observed_rx.recv().is_err(), "worker and sender must exit");
    }

    #[test]
    fn full_visual_queue_reserves_terminal_hide_after_all_accepted_updates() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let mut first = true;
        let (dispatcher, worker) = VisualDispatcher::spawn(move |request| {
            if first {
                first = false;
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            observed_tx.send(request).unwrap();
        })
        .unwrap();
        dispatcher
            .enqueue(visual_request("MoveCursor", &["in-flight"]))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        for index in 0..VISUAL_QUEUE_CAPACITY {
            dispatcher
                .enqueue(visual_request("MoveCursor", &[&index.to_string()]))
                .unwrap();
        }
        assert_eq!(
            dispatcher.enqueue(visual_request("ClickPulse", &["refused"])),
            Err(VisualQueueError::Full)
        );
        // OverlayMsg::Remove and SetEnabled(false) both use this global hide.
        for _ in 0..100 {
            dispatcher.enqueue(VisualRequest::hide()).unwrap();
        }
        assert_eq!(
            dispatcher.shared.0.lock().unwrap().pending.len(),
            VISUAL_QUEUE_CAPACITY + 1
        );
        assert_eq!(
            dispatcher.enqueue(visual_request("MoveCursor", &["also refused"])),
            Err(VisualQueueError::Full)
        );
        release_tx.send(()).unwrap();
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            visual_request("MoveCursor", &["in-flight"])
        );
        for index in 0..VISUAL_QUEUE_CAPACITY {
            assert_eq!(
                observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                visual_request("MoveCursor", &[&index.to_string()])
            );
        }
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            VisualRequest::hide()
        );
        assert!(
            observed_rx.try_recv().is_err(),
            "no stale update after hide"
        );
        // Hiding is not permanent shutdown: a subsequent caller can show again.
        dispatcher
            .enqueue(visual_request("MoveCursor", &["new activity"]))
            .unwrap();
        assert_eq!(
            observed_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            visual_request("MoveCursor", &["new activity"])
        );
        drop(dispatcher);
        worker.join().unwrap();
    }

    #[test]
    fn dropping_visual_dispatcher_discards_pending_updates_then_hides_and_exits() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let (dispatcher, worker) = VisualDispatcher::spawn(move |request| {
            if !request.is_hide() {
                started_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            }
            observed_tx.send(request).unwrap();
        })
        .unwrap();
        dispatcher
            .enqueue(visual_request("MoveCursor", &["in-flight"]))
            .unwrap();
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        dispatcher
            .enqueue(visual_request("ClickPulse", &["stale"]))
            .unwrap();
        drop(dispatcher);
        release_tx.send(()).unwrap();
        worker.join().unwrap();
        assert_eq!(
            observed_rx.into_iter().collect::<Vec<_>>(),
            vec![
                visual_request("MoveCursor", &["in-flight"]),
                VisualRequest::hide()
            ]
        );
    }

    #[test]
    fn failed_visual_worker_closes_admission() {
        let (dispatcher, worker) =
            VisualDispatcher::spawn(|_| panic!("injected helper failure")).unwrap();
        dispatcher
            .enqueue(visual_request("MoveCursor", &["10", "20"]))
            .unwrap();
        assert!(worker.join().is_err());
        assert_eq!(
            dispatcher.enqueue(VisualRequest::hide()),
            Err(VisualQueueError::Closed)
        );
    }

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
    fn accessibility_origin_matches_the_frame_cropped_screenshot() {
        let raw = r#"('[{"id":46,"pid":6079,"title":"Floating GTK","x":14,"y":12,"w":560,"h":736,"buffer_x":0,"buffer_y":0}]',)"#;

        assert_eq!(parse_window_origin(raw, 6079), Some((14, 12)));
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

    #[test]
    fn bundled_helper_v8_uses_host_owned_modifier_badge_chips() {
        assert!(EXTENSION_SOURCE.contains("GetVersion()"));
        assert!(EXTENSION_SOURCE.contains("return 8;"));
        assert!(EXTENSION_SOURCE.contains("SetCursorState"));
        assert!(EXTENSION_SOURCE.contains("SetCursorColor"));
        assert!(EXTENSION_SOURCE.contains("SetSessionLabel"));
        assert!(EXTENSION_SOURCE.contains("const DISPLAY_SIZE = 42;"));
        assert!(EXTENSION_SOURCE.contains("const GLOW_PADDING = 24;"));
        assert!(EXTENSION_SOURCE.contains("function drawCursorGlowShape"));
        assert!(EXTENSION_SOURCE.contains("function glowPath"));
        assert!(EXTENSION_SOURCE.contains("strokePath(cr, width, alpha, fillColor)"));
        assert!(EXTENSION_SOURCE.contains("width + 1.5"));
        assert!(EXTENSION_SOURCE.contains("width - 1"));
        assert!(EXTENSION_SOURCE.contains("createGlowSurface(this._fillColor)"));
        assert!(EXTENSION_SOURCE.contains("cr.translate(-GLOW_PADDING, -GLOW_PADDING);"));
        assert!(EXTENSION_SOURCE.contains("function drawBadgeChip"));
        assert!(EXTENSION_SOURCE.contains("function badgeStyle(fillColor)"));
        assert!(EXTENSION_SOURCE.contains("this._badge.add_child(this._badgeLabel)"));
        assert!(EXTENSION_SOURCE.contains("this._deliveryChip"));
        assert!(EXTENSION_SOURCE.contains("this._targetChip"));
        assert!(EXTENSION_SOURCE.contains("if (labelAlpha > 0.001 || chipAlpha > 0.001)"));
        assert!(EXTENSION_SOURCE.contains("this._badgeLabel.hide()"));
        assert!(!EXTENSION_SOURCE.contains("this._badgeIdentity"));
        assert!(!EXTENSION_SOURCE.contains("this._badgeDot"));
        assert!(!EXTENSION_SOURCE.contains("function drawModifiers"));
        let metadata: serde_json::Value =
            serde_json::from_str(EXTENSION_METADATA).expect("valid bundled helper metadata");
        assert_eq!(metadata["version"], 8);

        for action in [
            "idle", "observe", "click", "drag", "scroll", "text", "key", "navigate", "app",
            "transfer", "record", "system",
        ] {
            assert!(
                EXTENSION_SOURCE.contains(&format!("'{action}'")),
                "missing semantic cursor state {action}"
            );
        }

        assert!(!EXTENSION_SOURCE.contains("const VERTS"));
        assert!(!EXTENSION_SOURCE.contains("setSourceRGBA(0.10, 0.75, 1.00"));
    }
}
