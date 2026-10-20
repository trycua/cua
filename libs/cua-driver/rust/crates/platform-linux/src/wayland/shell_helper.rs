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
//! - `CaptureWindow(id,pid)` (API 9) / `CaptureWindowPreview(id,pid,max)`
//!   (API 10) — snapshot one window's own actor, so a covering window never
//!   shows; the preview variant downscales on the GPU first.
//!
//! Everything here is **best-effort**: if the extension isn't installed/enabled
//! the calls return `None` / no-op and callers keep the prior behaviour (no
//! screen coords, no Wayland cursor). Reuses one session-bus connection on a
//! dedicated runtime; preview frames do not spawn subprocesses or poll for exit.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
mod transport;
use std::time::Duration;

use crate::x11::WindowInfo;

const DEST: &str = "org.cua.WinRects";
const COMPOSITOR_DEST: &str = "org.gnome.Shell";
const PATH: &str = "/org/cua/WinRects";
const IFACE: &str = "org.cua.WinRects";
const DBUS_DEST: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
const BROWSER_HELPER_API_VERSION: u32 = 4;
const SEMANTIC_CURSOR_API_VERSION: u32 = 8;
/// `CaptureWindow(id, pid)`: per-window actor capture.
pub const WINDOW_CAPTURE_API_VERSION: u32 = 9;
/// `CaptureWindowPreview(id, pid, max_dimension)`: GPU-downscaled capture.
pub const WINDOW_PREVIEW_API_VERSION: u32 = 10;

#[derive(Debug, Clone)]
struct ShellWindow {
    info: WindowInfo,
    focused: bool,
}

pub fn available() -> bool {
    shell_owner(false).is_some()
}

/// Whether GNOME Shell is this session's compositor: the desktop name says
/// so, or `org.gnome.Shell` has an owner on the session bus. The bus check
/// keeps a daemon started without `XDG_CURRENT_DESKTOP` from treating GNOME
/// as a compositor with no Shell at all.
pub fn gnome_shell_session() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| {
        desktops
            .split(':')
            .any(|desktop| desktop.eq_ignore_ascii_case("gnome"))
    }) || SessionShellProbe.name_owner(COMPOSITOR_DEST).is_some()
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
    transport::call(destination, object_path, method, args, timeout)
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
    select_shell_owner(&SessionShellProbe, min_version)
}

/// Session-bus and procfs observations behind [`select_shell_owner`]. The
/// selection rules live in that function so they can be tested without a
/// running Shell; this trait only reads the facts.
trait ShellOwnerProbe {
    type ProcessPin;
    fn name_owner(&self, name: &str) -> Option<String>;
    fn connection_identity(&self, owner: &str) -> Option<(u32, u32)>;
    fn current_uid(&self) -> u32;
    fn pin_process(&self, pid: u32) -> Option<Self::ProcessPin>;
    fn process_still_pinned(&self, pid: u32, pin: &Self::ProcessPin) -> bool;
    fn trusted_shell(&self, pid: u32) -> bool;
    fn helper_version(&self, owner: &str) -> Option<u32>;
}

fn select_shell_owner<P: ShellOwnerProbe>(probe: &P, min_version: Option<u32>) -> Option<String> {
    let (owner, version) = attest_shell_owner(probe, min_version.is_some())?;
    min_version
        .is_none_or(|minimum| version.is_some_and(|version| version >= minimum))
        .then_some(owner)
}

/// The helper is trusted only when its unique owner is also the owner of
/// `org.gnome.Shell`, belongs to this user, and is hosted by an attested Shell
/// process. A second genuine `gnome-shell` of the same user (a nested or
/// development Shell) therefore cannot serve the helper for the session's
/// compositor. Every name and credential check is repeated after the optional
/// version call, so a re-owned name or recycled PID refuses and a returned
/// version belongs to the attested owner.
fn attest_shell_owner<P: ShellOwnerProbe>(
    probe: &P,
    read_version: bool,
) -> Option<(String, Option<u32>)> {
    let unique_owner = |name: &str| {
        probe
            .name_owner(name)
            .filter(|owner| owner.starts_with(':'))
    };
    let owner = unique_owner(DEST)?;
    if unique_owner(COMPOSITOR_DEST)? != owner {
        return None;
    }

    let (pid, uid) = probe.connection_identity(&owner)?;
    if uid != probe.current_uid() {
        return None;
    }
    let pin = probe.pin_process(pid)?;
    if !probe.trusted_shell(pid) {
        return None;
    }

    let version = if read_version {
        Some(probe.helper_version(&owner)?)
    } else {
        None
    };

    if unique_owner(DEST)? != owner
        || unique_owner(COMPOSITOR_DEST)? != owner
        || probe.connection_identity(&owner)? != (pid, uid)
        || !probe.trusted_shell(pid)
        || !probe.process_still_pinned(pid, &pin)
    {
        return None;
    }
    Some((owner, version))
}

struct SessionShellProbe;

impl ShellOwnerProbe for SessionShellProbe {
    type ProcessPin = std::fs::File;

    fn name_owner(&self, name: &str) -> Option<String> {
        let raw = gdbus_call_to(
            DBUS_DEST,
            DBUS_PATH,
            &format!("{DBUS_IFACE}.GetNameOwner"),
            &[name.to_owned()],
            Duration::from_millis(800),
        )?;
        parse_quoted_string(&raw)
    }

    fn connection_identity(&self, owner: &str) -> Option<(u32, u32)> {
        let scalar = |method: &str| {
            let raw = gdbus_call_to(
                DBUS_DEST,
                DBUS_PATH,
                &format!("{DBUS_IFACE}.{method}"),
                &[owner.to_owned()],
                Duration::from_millis(800),
            )?;
            parse_first_u32(&raw)
        };
        Some((
            scalar("GetConnectionUnixProcessID")?,
            scalar("GetConnectionUnixUser")?,
        ))
    }

    fn current_uid(&self) -> u32 {
        current_uid()
    }

    /// An open `/proc/PID` directory: if the process exits and its PID is
    /// reused, the directory reached by that path is a different inode.
    fn pin_process(&self, pid: u32) -> Option<std::fs::File> {
        let process_directory = std::fs::File::open(format!("/proc/{pid}")).ok()?;
        process_directory
            .metadata()
            .ok()?
            .is_dir()
            .then_some(process_directory)
    }

    fn process_still_pinned(&self, pid: u32, pin: &std::fs::File) -> bool {
        let (Ok(pinned), Ok(current)) = (pin.metadata(), std::fs::metadata(format!("/proc/{pid}")))
        else {
            return false;
        };
        current.dev() == pinned.dev() && current.ino() == pinned.ino()
    }

    fn trusted_shell(&self, pid: u32) -> bool {
        is_trusted_gnome_shell(pid)
    }

    fn helper_version(&self, owner: &str) -> Option<u32> {
        let raw = gdbus_call_to(
            owner,
            PATH,
            &format!("{IFACE}.GetVersion"),
            &[],
            Duration::from_millis(800),
        )?;
        parse_first_u32(&raw)
    }
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

/// The outcome of a per-window capture through the GNOME helper.
pub enum WindowCapture {
    /// No verified helper, or one older than API 9: use another route.
    Unsupported,
    /// The helper can capture windows but refused this one (minimized,
    /// closed, wrong owner, locked screen). Do not fall back to a stage crop.
    Unavailable,
    Png(Vec<u8>),
}

/// The verified helper owner and the API version that same owner reported.
fn attested_owner_with_version() -> Option<(String, u32)> {
    let (owner, version) = attest_shell_owner(&SessionShellProbe, true)?;
    Some((owner, version?))
}

/// Capture one window's compositor actor without activating it. Covered
/// windows keep their own content, so no stage crop is involved.
pub fn screenshot_window(id: u64, pid: Option<u32>) -> WindowCapture {
    let Some((owner, version)) = attested_owner_with_version() else {
        return WindowCapture::Unsupported;
    };
    if version < WINDOW_CAPTURE_API_VERSION {
        return WindowCapture::Unsupported;
    }
    let Ok(id) = u32::try_from(id) else {
        return WindowCapture::Unavailable;
    };
    gdbus_call_to(
        &owner,
        PATH,
        &format!("{IFACE}.CaptureWindow"),
        &[id.to_string(), pid.unwrap_or(0).to_string()],
        Duration::from_secs(5),
    )
    .and_then(|raw| decode_capture(&raw))
    .map_or(WindowCapture::Unavailable, WindowCapture::Png)
}

/// Preview-sized pixels of one window, downscaled in the compositor, plus the
/// window's full-size dimensions. `None` when the helper is older than API 10
/// or refuses; callers then take the full-size capture path.
pub fn screenshot_window_preview(
    id: u64,
    pid: u32,
    max_dimension: u32,
) -> Option<(Vec<u8>, u32, u32)> {
    let id = u32::try_from(id).ok()?;
    let owner = shell_owner_with_min_version(Some(WINDOW_PREVIEW_API_VERSION))?;
    let raw = gdbus_call_to(
        &owner,
        PATH,
        &format!("{IFACE}.CaptureWindowPreview"),
        &[id.to_string(), pid.to_string(), max_dimension.to_string()],
        Duration::from_secs(5),
    )?;
    let png = strip_preview_timestamp(decode_capture(&raw)?)?;
    let (width, height) = parse_preview_dimensions(&raw)?;
    Some((png, width, height))
}

/// The `uint32 W, uint32 H` after the PNG in a `CaptureWindowPreview` reply.
fn parse_preview_dimensions(raw: &str) -> Option<(u32, u32)> {
    let tail = &raw[raw.rfind('\'')? + 1..];
    let mut values = tail.split("uint32").skip(1).map(|part| {
        part.trim_matches(|c: char| !c.is_ascii_digit())
            .parse::<u32>()
    });
    let width = values.next()?.ok()?;
    let height = values.next()?.ok()?;
    (values.next().is_none() && width > 0 && height > 0).then_some((width, height))
}

/// GNOME embeds a wall-clock `Creation Time` text chunk even when the pixels
/// are identical. Remove only that chunk, so a host can drop unchanged
/// preview frames by comparing bytes, without decoding pixels.
fn strip_preview_timestamp(mut png: Vec<u8>) -> Option<Vec<u8>> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") {
        return None;
    }
    let mut offset = 8usize;
    loop {
        let header = png.get(offset..offset.checked_add(8)?)?;
        let size = u32::from_be_bytes(header[..4].try_into().ok()?) as usize;
        let end = offset.checked_add(12)?.checked_add(size)?;
        let chunk = png.get(offset..end)?;
        if &chunk[4..8] == b"tEXt" && chunk[8..8 + size].starts_with(b"Creation Time\0") {
            png.drain(offset..end);
        } else if &chunk[4..8] == b"IEND" {
            return (size == 0 && end == png.len()).then_some(png);
        } else {
            offset = end;
        }
    }
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

/// Screen origin of the exact compositor frame `window_id` owned by `pid`.
/// A zero `window_id` is a PID-only query and needs the PID to own exactly
/// one window: another window's origin would place every element wrongly.
///
/// `screenshot_window_dispatch` crops the Shell stage to this same frame
/// rectangle, and GTK's AT-SPI `CoordType::Window` coordinates are relative to
/// that frame. Using Mutter's larger surface-buffer rectangle here shifts
/// elements up and left by the client-side shadow extents whenever the window
/// is floating, so pixel actions derived from the returned screenshot miss
/// their target.
pub fn window_origin(pid: u32, window_id: u64) -> Option<(i32, i32)> {
    let raw = gdbus_call("GetRects", &[])?;
    parse_window_origin(&raw, pid, window_id)
}

fn parse_window_origin(raw: &str, pid: u32, window_id: u64) -> Option<(i32, i32)> {
    let window = select_window(
        parse_windows(raw, Some(pid))?,
        (window_id != 0).then_some(window_id),
    )?;
    Some((window.x, window.y))
}

/// The one window matching `window_id`, or the only window when no ID is
/// given. A duplicated ID is not authoritative.
fn select_window(windows: Vec<WindowInfo>, window_id: Option<u64>) -> Option<WindowInfo> {
    let mut matches = windows
        .into_iter()
        .filter(|window| window_id.is_none_or(|wanted| window.xid == wanted));
    let window = matches.next()?;
    matches.next().is_none().then_some(window)
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
    exact_shell_window(&trusted_shell_windows(Some(pid))?, pid, window_id)
        .map(|window| window.info.clone())
}

/// The one Shell window with this PID and stable sequence ID. A same-PID
/// sibling never substitutes, and a duplicated ID is not authoritative.
fn exact_shell_window(windows: &[ShellWindow], pid: u32, window_id: u64) -> Option<&ShellWindow> {
    let mut matches = windows
        .iter()
        .filter(|window| window.info.pid == Some(pid) && window.info.xid == window_id);
    let window = matches.next()?;
    matches.next().is_none().then_some(window)
}

/// Decide how [`with_focused_window`] reaches the exact target: `None` when it
/// already has focus, otherwise the focused window to restore afterwards.
fn plan_focus(before: &[ShellWindow], pid: u32, window_id: u64) -> anyhow::Result<Option<u64>> {
    let target = exact_shell_window(before, pid, window_id)
        .ok_or_else(|| anyhow::anyhow!("no exact GNOME Shell window owns the approved target"))?;
    if shell_sequence_id(window_id).is_none() {
        anyhow::bail!("the approved target is not a GNOME Shell window sequence");
    }
    if target.focused {
        return Ok(None);
    }
    before
        .iter()
        .find(|window| window.focused)
        .map(|window| Some(window.info.xid))
        .ok_or_else(|| anyhow::anyhow!("GNOME Shell did not expose a restorable focused window"))
}

/// Shell stable sequences are u32. Wider values, such as the AT-SPI fallback
/// handles (`pid << 16`), are never passed to `Activate`.
fn shell_sequence_id(window_id: u64) -> Option<u32> {
    u32::try_from(window_id).ok()
}

/// Fresh focus readback for one exact window ID.
fn exact_window_focused(windows: &[ShellWindow], window_id: u32) -> bool {
    let mut matches = windows
        .iter()
        .filter(|window| window.info.xid == u64::from(window_id));
    matches.next().is_some_and(|window| window.focused) && matches.next().is_none()
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
    let Some(previous) = plan_focus(&before, pid, window_id)? else {
        return body();
    };
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
    let Some(window_id) = shell_sequence_id(window_id) else {
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
    let Some(window_id) = shell_sequence_id(window_id) else {
        return false;
    };
    let accepted = trusted_gdbus_call("Activate", &[window_id.to_string()])
        .is_some_and(|output| output.trim_start().starts_with("(true,"));
    if !accepted {
        return false;
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    loop {
        if trusted_shell_windows(None)
            .is_some_and(|windows| exact_window_focused(&windows, window_id))
        {
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
    parse_shell_windows(&raw, None).is_some_and(|windows| exact_window_focused(&windows, window_id))
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
    let windows: Vec<serde_json::Value> = serde_json::from_str(raw).ok()?;

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

/// Set the stable session-specific fill color for the compositor cursor.
pub fn set_cursor_color(fill_color: &str) {
    let _ = gdbus_call("SetCursorColor", &[fill_color.to_owned()]);
}

/// Update the compositor-owned cursor's semantic action state.
///
/// Callers gate this method on helper v8 so an older helper cannot silently
/// render the retired cursor artwork.
pub fn set_cursor_state(action: &str, delivery: &str, target: &str, active: bool) {
    let _ = gdbus_call(
        "SetCursorState",
        &[
            action.to_owned(),
            delivery.to_owned(),
            target.to_owned(),
            active.to_string(),
        ],
    );
}

/// Set the renderer-visible public session label for the compositor cursor.
pub fn set_session_label(label: &str) {
    let _ = gdbus_call("SetSessionLabel", &[label.to_owned()]);
}

/// Hide the agent cursor.
pub fn hide_cursor() {
    let _ = gdbus_call("HideCursor", &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_timestamp_does_not_change_pixels_or_frame_identity() {
        use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
        let first = B64.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAE3RFWHRDcmVhdGlvbiBUaW1lAGZpcnN0muU4QwAAABl0RVh0U29mdHdhcmUAZ25vbWUtc2NyZWVuc2hvdO8Dvz4AAAAUSURBVHicY5SLqvjPwMDAwMQABQAczgHzr55lzgAAAABJRU5ErkJggg==").unwrap();
        let second = B64.decode("iVBORw0KGgoAAAANSUhEUgAAAAIAAAACCAYAAABytg0kAAAAFHRFWHRDcmVhdGlvbiBUaW1lAHNlY29uZKzNUcIAAAAZdEVYdFNvZnR3YXJlAGdub21lLXNjcmVlbnNob3TvA78+AAAAFElEQVR4nGOUi6r4z8DAwMDEAAUAHM4B86+eZc4AAAAASUVORK5CYII=").unwrap();
        let stable = strip_preview_timestamp(first.clone()).unwrap();
        assert_eq!(stable, strip_preview_timestamp(second).unwrap());
        assert_eq!(
            image::load_from_memory(&stable).unwrap().to_rgba8(),
            image::load_from_memory(&first).unwrap().to_rgba8()
        );
        assert!(stable.windows(8).any(|bytes| bytes == b"Software"));
        assert_eq!(Some(stable.clone()), strip_preview_timestamp(stable));
        assert!(strip_preview_timestamp(first[..first.len() - 1].to_vec()).is_none());
        let mut malformed = first;
        malformed[8..12].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(strip_preview_timestamp(malformed).is_none());
    }

    #[test]
    fn preview_reply_carries_full_size_dimensions() {
        let raw = "('cG5n', uint32 1600, uint32 900)";
        assert_eq!(decode_capture(raw), Some(b"png".to_vec()));
        assert_eq!(parse_preview_dimensions(raw), Some((1600, 900)));
        assert_eq!(
            parse_preview_dimensions("('cG5n', uint32 0, uint32 900)"),
            None
        );
        assert_eq!(parse_preview_dimensions("('cG5n',)"), None);
        assert_eq!(
            parse_preview_dimensions("('cG5n', uint32 1, uint32 2, uint32 3)"),
            None
        );
    }

    const EXTENSION_SOURCE: &str =
        include_str!("../../../../../wayland-helper/winrects@cua/extension.js");
    const EXTENSION_METADATA: &str =
        include_str!("../../../../../wayland-helper/winrects@cua/metadata.json");

    #[test]
    fn rejects_malformed_or_display_formatted_snapshots() {
        for raw in ["not JSON", "{}", "('[{\"pid\":6079,\"x\":14,\"y\":12}]',)"] {
            assert!(parse_shell_windows(raw, None).is_none());
            assert!(parse_window_origin(raw, 6079, 0).is_none());
        }
    }

    #[test]
    fn parses_and_filters_shell_windows() {
        let raw = r#"[{"id":46,"pid":6079,"title":"Sentinel's window","x":66,"y":32,"w":958,"h":736,"focused":true,"minimized":false,"visible":true,"stacking":2},{"id":47,"pid":6080,"title":"Hidden","x":0,"y":0,"w":100,"h":100,"minimized":true,"visible":false,"stacking":1}]"#;
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
        let raw = r#"[{"id":46,"pid":6079,"title":"Floating GTK","x":14,"y":12,"w":560,"h":736,"buffer_x":0,"buffer_y":0}]"#;

        assert_eq!(parse_window_origin(raw, 6079, 46), Some((14, 12)));
        assert_eq!(parse_window_origin(raw, 6079, 0), Some((14, 12)));
    }

    #[test]
    fn marks_minimized_shell_windows_off_screen() {
        let raw = r#"[{"id":47,"pid":6080,"title":"Hidden","x":0,"y":0,"w":100,"h":100,"minimized":true,"visible":false,"stacking":1}]"#;
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

    /// Scripted bus and procfs answers. Each name or connection maps to a
    /// queue whose last answer repeats, so a test can re-own a name between
    /// the first check and the recheck.
    struct FakeShellBus {
        names: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<&'static str>>>,
        identities: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<(u32, u32)>>>,
        uid: u32,
        trusted_pids: Vec<u32>,
        version: Option<u32>,
        pin_holds: bool,
    }

    fn next_answer<K: std::hash::Hash + Eq, V: Clone>(
        answers: &std::cell::RefCell<std::collections::HashMap<K, Vec<V>>>,
        key: &K,
    ) -> Option<V> {
        let mut answers = answers.borrow_mut();
        let queue = answers.get_mut(key)?;
        if queue.len() > 1 {
            Some(queue.remove(0))
        } else {
            queue.first().cloned()
        }
    }

    impl FakeShellBus {
        /// `:1.97` is the session's Shell (PID 5112). `:1.42` is a second
        /// genuine `gnome-shell` of the same user (PID 4242).
        fn genuine() -> Self {
            Self {
                names: std::cell::RefCell::new(
                    [(DEST, vec![":1.97"]), (COMPOSITOR_DEST, vec![":1.97"])].into(),
                ),
                identities: std::cell::RefCell::new(
                    [(":1.97", vec![(5112, 1000)]), (":1.42", vec![(4242, 1000)])].into(),
                ),
                uid: 1000,
                trusted_pids: vec![5112, 4242],
                version: Some(8),
                pin_holds: true,
            }
        }

        fn with_names(self, helper: Vec<&'static str>, shell: Vec<&'static str>) -> Self {
            *self.names.borrow_mut() = [(DEST, helper), (COMPOSITOR_DEST, shell)].into();
            self
        }
    }

    impl ShellOwnerProbe for FakeShellBus {
        type ProcessPin = u32;

        fn name_owner(&self, name: &str) -> Option<String> {
            let key = [DEST, COMPOSITOR_DEST]
                .into_iter()
                .find(|known| *known == name)?;
            next_answer(&self.names, &key).map(str::to_owned)
        }

        fn connection_identity(&self, owner: &str) -> Option<(u32, u32)> {
            let key = [":1.97", ":1.42"]
                .into_iter()
                .find(|known| *known == owner)?;
            next_answer(&self.identities, &key)
        }

        fn current_uid(&self) -> u32 {
            self.uid
        }

        fn pin_process(&self, pid: u32) -> Option<u32> {
            Some(pid)
        }

        fn process_still_pinned(&self, pid: u32, pin: &u32) -> bool {
            self.pin_holds && pid == *pin
        }

        fn trusted_shell(&self, pid: u32) -> bool {
            self.trusted_pids.contains(&pid)
        }

        fn helper_version(&self, _owner: &str) -> Option<u32> {
            self.version
        }
    }

    #[test]
    fn helper_hosted_by_the_session_shell_is_selected() {
        let bus = FakeShellBus::genuine();
        assert_eq!(select_shell_owner(&bus, None).as_deref(), Some(":1.97"));
        assert_eq!(
            select_shell_owner(&bus, Some(SEMANTIC_CURSOR_API_VERSION)).as_deref(),
            Some(":1.97")
        );
        assert_eq!(
            attest_shell_owner(&bus, true),
            Some((":1.97".to_owned(), Some(8)))
        );
    }

    #[test]
    fn winrects_owner_must_equal_the_gnome_shell_owner() {
        // :1.42 passes every executable check but does not own org.gnome.Shell.
        let decoy = FakeShellBus::genuine().with_names(vec![":1.42"], vec![":1.97"]);
        assert!(decoy.trusted_shell(4242));
        assert_eq!(select_shell_owner(&decoy, None), None);
        let no_shell = FakeShellBus::genuine().with_names(vec![":1.97"], vec![]);
        assert_eq!(select_shell_owner(&no_shell, None), None);
        let no_helper = FakeShellBus::genuine().with_names(vec![], vec![":1.97"]);
        assert_eq!(select_shell_owner(&no_helper, None), None);
        let well_known = FakeShellBus::genuine().with_names(vec![DEST], vec![DEST]);
        assert_eq!(select_shell_owner(&well_known, None), None);
    }

    #[test]
    fn owner_recheck_refuses_reowned_names_and_changed_credentials() {
        let helper_moved =
            FakeShellBus::genuine().with_names(vec![":1.97", ":1.42"], vec![":1.97"]);
        assert_eq!(select_shell_owner(&helper_moved, None), None);
        let shell_moved = FakeShellBus::genuine().with_names(vec![":1.97"], vec![":1.97", ":1.42"]);
        assert_eq!(select_shell_owner(&shell_moved, None), None);
        let recycled = FakeShellBus::genuine();
        recycled
            .identities
            .borrow_mut()
            .insert(":1.97", vec![(5112, 1000), (4242, 1000)]);
        assert_eq!(select_shell_owner(&recycled, None), None);
        let lost_pin = FakeShellBus {
            pin_holds: false,
            ..FakeShellBus::genuine()
        };
        assert_eq!(select_shell_owner(&lost_pin, None), None);
        // The version is read between the checks: a helper that moves after
        // answering cannot lend its version to the attested owner.
        let moved_after_version =
            FakeShellBus::genuine().with_names(vec![":1.97", ":1.42"], vec![":1.97"]);
        assert_eq!(attest_shell_owner(&moved_after_version, true), None);
    }

    #[test]
    fn owner_selection_refuses_wrong_uid_untrusted_process_and_old_api() {
        let other_user = FakeShellBus {
            uid: 1001,
            ..FakeShellBus::genuine()
        };
        assert_eq!(select_shell_owner(&other_user, None), None);
        let untrusted = FakeShellBus {
            trusted_pids: vec![4242],
            ..FakeShellBus::genuine()
        };
        assert_eq!(select_shell_owner(&untrusted, None), None);
        let old_api = FakeShellBus {
            version: Some(7),
            ..FakeShellBus::genuine()
        };
        assert_eq!(select_shell_owner(&old_api, None).as_deref(), Some(":1.97"));
        assert_eq!(
            select_shell_owner(&old_api, Some(SEMANTIC_CURSOR_API_VERSION)),
            None
        );
        let silent_api = FakeShellBus {
            version: None,
            ..FakeShellBus::genuine()
        };
        assert_eq!(
            select_shell_owner(&silent_api, Some(BROWSER_HELPER_API_VERSION)),
            None
        );
        assert_eq!(attest_shell_owner(&silent_api, true), None);
    }

    // Two windows of one PID: 363 at x=0 and 438 at x=600, plus another
    // application's focused window 47.
    const SIBLINGS: &str = r#"[{"id":363,"pid":6079,"title":"Main","x":0,"y":0,"w":560,"h":400},{"id":438,"pid":6079,"title":"Dialog","x":600,"y":0,"w":400,"h":300},{"id":47,"pid":6080,"title":"Other","x":1200,"y":0,"w":100,"h":100,"focused":true}]"#;
    const SIBLINGS_REORDERED: &str = r#"[{"id":438,"pid":6079,"title":"Dialog","x":600,"y":0,"w":400,"h":300},{"id":363,"pid":6079,"title":"Main","x":0,"y":0,"w":560,"h":400},{"id":47,"pid":6080,"title":"Other","x":1200,"y":0,"w":100,"h":100,"focused":true}]"#;

    #[test]
    fn exact_window_origin_survives_sibling_stacking_reorder() {
        for raw in [SIBLINGS, SIBLINGS_REORDERED] {
            assert_eq!(parse_window_origin(raw, 6079, 438), Some((600, 0)));
            assert_eq!(parse_window_origin(raw, 6079, 363), Some((0, 0)));
            // A window of another PID, or an unknown ID, has no origin here.
            assert_eq!(parse_window_origin(raw, 6079, 47), None);
            assert_eq!(parse_window_origin(raw, 6079, 999), None);
        }
    }

    #[test]
    fn pid_only_origin_requires_one_owned_window() {
        assert_eq!(parse_window_origin(SIBLINGS, 6079, 0), None);
        assert_eq!(parse_window_origin(SIBLINGS, 6080, 0), Some((1200, 0)));
    }

    #[test]
    fn duplicate_window_identity_is_not_authoritative() {
        let raw = r#"[{"id":438,"pid":6079,"x":0,"y":0,"w":10,"h":10},{"id":438,"pid":6079,"x":600,"y":0,"w":10,"h":10}]"#;
        assert_eq!(parse_window_origin(raw, 6079, 438), None);
        let windows = parse_shell_windows(raw, None).unwrap();
        assert!(exact_shell_window(&windows, 6079, 438).is_none());
    }

    fn shell_window(id: u64, pid: u32, x: i32, focused: bool) -> ShellWindow {
        ShellWindow {
            info: WindowInfo {
                xid: id,
                pid: Some(pid),
                title: format!("window {id}"),
                app_name: String::new(),
                is_on_screen: true,
                z_index: None,
                x,
                y: 0,
                width: 640,
                height: 480,
            },
            focused,
        }
    }

    #[test]
    fn activation_targets_the_exact_window_never_a_same_pid_sibling() {
        let siblings = [
            shell_window(363, 6079, 0, false),
            shell_window(438, 6079, 600, false),
            shell_window(47, 6080, 1200, true),
        ];
        let mut reversed = siblings.clone();
        reversed.reverse();
        for windows in [&siblings[..], &reversed[..]] {
            assert_eq!(exact_shell_window(windows, 6079, 438).unwrap().info.x, 600);
            assert_eq!(exact_shell_window(windows, 6079, 363).unwrap().info.x, 0);
            assert_eq!(plan_focus(windows, 6079, 438).unwrap(), Some(47));
            assert_eq!(plan_focus(windows, 6079, 363).unwrap(), Some(47));
            assert_eq!(plan_focus(windows, 6080, 47).unwrap(), None);
            for (pid, id) in [(6079, 999), (6080, 438), (6079, 6079 << 16), (6079, 0)] {
                assert!(exact_shell_window(windows, pid, id).is_none());
                assert!(plan_focus(windows, pid, id).is_err());
            }
        }
        let duplicate = [
            shell_window(438, 6079, 0, false),
            shell_window(438, 6079, 600, false),
            shell_window(47, 6080, 1200, true),
        ];
        assert!(plan_focus(&duplicate, 6079, 438).is_err());
        let nothing_focused = [shell_window(438, 6079, 600, false)];
        assert!(plan_focus(&nothing_focused, 6079, 438).is_err());
        // An ID wider than a Shell sequence never plans activation, even if a
        // malformed snapshot lists it as already focused.
        let wide = [shell_window(1 << 33, 6079, 0, true)];
        assert!(plan_focus(&wide, 6079, 1 << 33).is_err());
    }

    #[test]
    fn focus_readback_requires_the_exact_window() {
        let sibling_focused = [
            shell_window(363, 6079, 0, true),
            shell_window(438, 6079, 600, false),
        ];
        assert!(!exact_window_focused(&sibling_focused, 438));
        assert!(exact_window_focused(&sibling_focused, 363));
        let target_focused = [
            shell_window(363, 6079, 0, false),
            shell_window(438, 6079, 600, true),
        ];
        assert!(exact_window_focused(&target_focused, 438));
        assert!(!exact_window_focused(&target_focused, 999));
        let duplicate = [
            shell_window(438, 6079, 0, true),
            shell_window(438, 6079, 600, true),
        ];
        assert!(!exact_window_focused(&duplicate, 438));
    }

    #[test]
    fn accessibility_fallback_handles_are_not_shell_sequences() {
        // The AT-SPI window list hands out `pid << 16 | ordinal`.
        let handle = 3_592_163u64 << 16;
        assert_eq!(shell_sequence_id(handle), None);
        assert!(!activate_window(handle));
        assert!(!trusted_activate_window(handle));
        assert_eq!(shell_sequence_id(u64::from(u32::MAX)), Some(u32::MAX));
        assert_eq!(shell_sequence_id(33), Some(33));
    }

    #[test]
    fn preserves_exact_focus_from_shell_snapshot() {
        let raw = r#"[{"id":46,"pid":6079,"title":"Target","x":66,"y":32,"w":958,"h":736,"focused":false,"minimized":false,"visible":true,"stacking":2},{"id":47,"pid":6080,"title":"Sentinel","x":0,"y":0,"w":100,"h":100,"focused":true,"minimized":false,"visible":true,"stacking":3}]"#;
        let windows = parse_shell_windows(raw, None).expect("valid helper response");
        assert_eq!(windows.len(), 2);
        assert!(!windows[0].focused);
        assert!(windows[1].focused);
        assert_eq!(windows[1].info.xid, 47);
    }

    #[test]
    fn bundled_helper_uses_host_owned_modifier_badge_chips() {
        assert!(EXTENSION_SOURCE.contains("GetVersion()"));
        assert!(EXTENSION_SOURCE.contains("return 10;"));
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
        assert_eq!(metadata["version"], 10);

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
