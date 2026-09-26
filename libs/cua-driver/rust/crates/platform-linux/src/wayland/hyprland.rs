//! Read-only Hyprland identity and geometry, adapted from #3052.
//!
//! Native IDs are full compositor addresses, never title matches or truncated
//! protocol object IDs. IPC and capture must belong to the same compositor.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
const QUERY_TOTAL_TIMEOUT: Duration = Duration::from_secs(3);
const QUERY_RETRY_BACKOFF: Duration = Duration::from_millis(50);
const QUERY_MAX_ATTEMPTS: usize = 2;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOGICAL_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct Workspace {
    id: i64,
}

#[derive(Clone, Debug, Deserialize)]
struct Client {
    address: String,
    mapped: bool,
    hidden: bool,
    pid: i64,
    title: String,
    class: String,
    at: [i32; 2],
    size: [i32; 2],
    workspace: Workspace,
}

#[derive(Deserialize)]
struct Monitor {
    #[serde(rename = "activeWorkspace")]
    active_workspace: Workspace,
    #[serde(rename = "specialWorkspace")]
    special_workspace: Workspace,
    /// A monitor in DPMS standby still owns its workspaces, but nothing on it
    /// is visible to the user.
    #[serde(rename = "dpmsStatus", default = "powered_by_default")]
    dpms_status: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct DisplayMonitor {
    #[serde(default)]
    name: String,
    width: u32,
    height: u32,
    scale: f64,
    x: i32,
    y: i32,
    transform: u32,
    #[serde(default)]
    disabled: bool,
    #[serde(rename = "dpmsStatus", default = "powered_by_default")]
    dpms_status: bool,
}

fn powered_by_default() -> bool {
    true
}

impl DisplayMonitor {
    /// Enabled and not in DPMS standby: the user can actually see it.
    fn powered(&self) -> bool {
        !self.disabled && self.dpms_status
    }
}

/// One powered output inside the desktop frame, in Hyprland layout coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameOutput {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The desktop action frame: the bounding box of every powered output
/// (enabled and not in DPMS standby), in Hyprland layout coordinates.
/// Desktop-scope screenshots, sizes and actions all use this frame, with
/// frame pixel (0, 0) at layout point (`x`, `y`). It is re-derived from the
/// compositor on every call, so monitors turned on or off take effect
/// immediately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopFrame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub outputs: Vec<FrameOutput>,
}

impl DesktopFrame {
    pub fn to_layout(&self, x: i32, y: i32) -> (i32, i32) {
        (self.x.saturating_add(x), self.y.saturating_add(y))
    }

    pub fn from_layout(&self, x: i32, y: i32) -> (i32, i32) {
        (x.saturating_sub(self.x), y.saturating_sub(self.y))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub address: u64,
    pub pid: u32,
    pub title: String,
    pub app_id: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub workspace: i64,
    pub visible: bool,
    hidden: bool,
}

pub fn is_session() -> bool {
    !super::is_inject_mode()
        && std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some_and(|s| !s.is_empty())
}

fn ipc_path() -> Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?;
    let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE")
        .context("missing HYPRLAND_INSTANCE_SIGNATURE")?;
    if signature.is_empty()
        || !signature
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        bail!("invalid Hyprland instance signature");
    }
    let runtime = PathBuf::from(runtime);
    if !runtime.is_absolute() {
        bail!("XDG_RUNTIME_DIR must be absolute");
    }
    Ok(runtime.join("hypr").join(signature).join(".socket.sock"))
}

pub(super) fn peer_pid(fd: RawFd) -> Result<libc::pid_t> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0
        || len as usize != std::mem::size_of::<libc::ucred>()
        || cred.pid <= 0
        || cred.uid != unsafe { libc::geteuid() }
    {
        bail!("could not verify same-user compositor peer");
    }
    Ok(cred.pid)
}

fn ipc_connection() -> Result<UnixStream> {
    ipc_connection_with_timeout(QUERY_TIMEOUT)
}

fn ipc_connection_with_timeout(timeout: Duration) -> Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(ipc_path()?)?, timeout)?;
    Ok(socket.into())
}

/// Bound the socket connection as well as protocol dispatch. Inherited
/// WAYLAND_SOCKET descriptors are deliberately unsupported here: this adapter
/// opens independent, attested connections for each observation.
pub(super) fn wayland_connection() -> Result<wayland_client::Connection> {
    wayland_connection_with_timeout(QUERY_TIMEOUT)
}

fn wayland_connection_with_timeout(timeout: Duration) -> Result<wayland_client::Connection> {
    if std::env::var_os("WAYLAND_SOCKET").is_some() {
        bail!("Hyprland observation requires a named WAYLAND_DISPLAY socket");
    }
    let display =
        PathBuf::from(std::env::var_os("WAYLAND_DISPLAY").context("missing WAYLAND_DISPLAY")?);
    let path = if display.is_absolute() {
        display
    } else {
        if display.components().count() != 1 {
            bail!("invalid WAYLAND_DISPLAY socket name");
        }
        let runtime =
            PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("missing XDG_RUNTIME_DIR")?);
        if !runtime.is_absolute() {
            bail!("XDG_RUNTIME_DIR must be absolute");
        }
        runtime.join(display)
    };
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(path)?, timeout)?;
    wayland_client::Connection::from_socket(socket.into()).context("Wayland connection failed")
}

/// Bind the Wayland connection to the IPC compositor before accepting pixels.
pub(super) fn verify_capture_peer(connection: &wayland_client::Connection) -> Result<()> {
    let ipc = ipc_connection()?;
    if peer_pid(ipc.as_raw_fd())? != peer_pid(connection.backend().poll_fd().as_raw_fd())? {
        bail!("Hyprland IPC and WAYLAND_DISPLAY name different compositor processes");
    }
    Ok(())
}

fn query<T: serde::de::DeserializeOwned>(command: &str) -> Result<T> {
    let mut expected_peer = None;
    query_with(
        command,
        QUERY_TIMEOUT,
        QUERY_TOTAL_TIMEOUT,
        QUERY_RETRY_BACKOFF,
        |deadline| {
            let ipc = ipc_connection_with_timeout(query_time_remaining(deadline)?)?;
            // A stale inherited instance signature must not supply geometry for
            // a different nested desktop. Re-attest both sockets on every try.
            let wayland = wayland_connection_with_timeout(query_time_remaining(deadline)?)?;
            let peer = peer_pid(ipc.as_raw_fd())?;
            if peer != peer_pid(wayland.backend().poll_fd().as_raw_fd())? {
                bail!("Hyprland IPC and WAYLAND_DISPLAY name different compositor processes");
            }
            if expected_peer.is_some_and(|expected| expected != peer) {
                bail!("Hyprland compositor changed during observation retry");
            }
            expected_peer = Some(peer);
            Ok(ipc)
        },
    )
}

fn query_time_remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "Hyprland IPC query timed out").into());
    }
    Ok(remaining)
}

fn is_query_timeout(error: &anyhow::Error) -> bool {
    error.downcast_ref::<io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        )
    })
}

/// Retry only a timed-out observation, never an action or an identity failure.
/// Each attempt discards all previous bytes and opens newly attested sockets.
fn query_with<T: serde::de::DeserializeOwned>(
    command: &str,
    per_attempt: Duration,
    total: Duration,
    backoff: Duration,
    mut connect: impl FnMut(Instant) -> Result<UnixStream>,
) -> Result<T> {
    let deadline = Instant::now() + total;
    // Keep this a closed list: a generic "j/" prefix also admits JSON-formatted
    // dispatch commands. JSON output does not imply a read-only operation.
    let read_only = matches!(command, "j/monitors" | "j/clients" | "j/activewindow");
    for attempt in 1..=QUERY_MAX_ATTEMPTS {
        query_time_remaining(deadline)?;
        let attempt_deadline = deadline.min(Instant::now() + per_attempt);
        // Connection and peer-attestation errors are permanent. Only read-only
        // request/reply timeouts below are eligible for a new query.
        let mut ipc = connect(attempt_deadline)?;
        let reply = query_time_remaining(attempt_deadline)
            .and_then(|remaining| read_reply(&mut ipc, command.as_bytes(), remaining));
        match reply {
            Ok(bytes) => {
                return serde_json::from_slice(&bytes).context("invalid Hyprland IPC JSON")
            }
            Err(error) => {
                if !read_only
                    || !is_query_timeout(&error)
                    || attempt == QUERY_MAX_ATTEMPTS
                    || deadline.saturating_duration_since(Instant::now()) <= backoff
                {
                    return Err(error).with_context(|| {
                        format!("Hyprland IPC observation failed after {attempt} attempt(s)")
                    });
                }
                drop(ipc);
                tracing::warn!(
                    command,
                    attempt,
                    "Hyprland IPC observation timed out; retrying on a fresh connection"
                );
                std::thread::sleep(backoff);
            }
        }
    }
    unreachable!("the final query attempt always returns")
}

fn read_reply(stream: &mut UnixStream, command: &[u8], timeout: Duration) -> Result<Vec<u8>> {
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(timeout))?;
    stream.write_all(command)?;
    let mut reply = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        let remaining = query_time_remaining(deadline)?;
        stream.set_read_timeout(Some(remaining))?;
        let count = stream
            .read(&mut chunk)
            .context("Hyprland IPC reply unavailable")?;
        if count == 0 {
            break;
        }
        if reply.len() + count > MAX_REPLY_BYTES {
            bail!("Hyprland IPC reply exceeds size limit");
        }
        reply.extend_from_slice(&chunk[..count]);
    }
    Ok(reply)
}

fn windows_from_clients(clients: Vec<Client>, active: &HashSet<i64>) -> Result<Vec<Window>> {
    let mut seen = HashSet::new();
    let mut windows = Vec::new();
    for c in clients.into_iter().filter(|c| c.mapped) {
        let address = u64::from_str_radix(c.address.strip_prefix("0x").unwrap_or(&c.address), 16)?;
        let pid = u32::try_from(c.pid)?;
        let width = u32::try_from(c.size[0])?;
        let height = u32::try_from(c.size[1])?;
        if address == 0 || pid == 0 || !valid_dimensions(width, height) || !seen.insert(address) {
            bail!("invalid or duplicate Hyprland window identity/geometry");
        }
        windows.push(Window {
            address,
            pid,
            title: c.title,
            app_id: c.class,
            x: c.at[0],
            y: c.at[1],
            width,
            height,
            workspace: c.workspace.id,
            visible: !c.hidden && active.contains(&c.workspace.id),
            hidden: c.hidden,
        });
    }
    Ok(windows)
}

fn valid_dimensions(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_LOGICAL_PIXELS
}

/// Content-free geometry of the desktop frame (see [`DesktopFrame`]).
/// The common policy adapter uses this for display-scoped observation. Never
/// substitute a screenshot, XWayland root, or guessed primary monitor here.
pub fn screen_size() -> Result<(u32, u32, f64)> {
    screen_size_from_monitors(query("j/monitors")?)
}

fn screen_size_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<(u32, u32, f64)> {
    let frame = desktop_frame_from_monitors(monitors)?;
    Ok((frame.width, frame.height, 1.0))
}

/// Every enabled monitor with its power state, for agents that need to know
/// which displays are actually on. Positions are relative to the desktop
/// frame when powered; `None` for monitors outside it (standby).
pub fn monitor_report() -> Result<serde_json::Value> {
    let monitors: Vec<DisplayMonitor> = query("j/monitors")?;
    let frame = desktop_frame_from_monitors(monitors.clone())?;
    Ok(serde_json::Value::Array(
        monitors
            .iter()
            .filter(|m| !m.disabled)
            .map(|m| {
                let (fx, fy) = frame.from_layout(m.x, m.y);
                serde_json::json!({
                    "name": m.name,
                    "width": m.width,
                    "height": m.height,
                    "powered": m.powered(),
                    "frame_x": m.powered().then_some(fx),
                    "frame_y": m.powered().then_some(fy),
                })
            })
            .collect(),
    ))
}

/// Current desktop frame spanning every powered output.
pub fn desktop_frame() -> Result<DesktopFrame> {
    desktop_frame_from_monitors(query("j/monitors")?)
}

fn desktop_frame_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<DesktopFrame> {
    let powered: Vec<DisplayMonitor> = monitors
        .into_iter()
        .filter(DisplayMonitor::powered)
        .collect();
    if powered.is_empty() {
        bail!("Hyprland has no powered output: every monitor is disabled or in DPMS standby");
    }
    for monitor in &powered {
        if monitor.scale != 1.0 || monitor.transform != 0 {
            bail!("Hyprland display identity requires unscaled, unrotated outputs");
        }
        if !valid_dimensions(monitor.width, monitor.height) {
            bail!("invalid Hyprland display dimensions");
        }
    }
    let (x, y, width, height) = bounding_box(&powered)?;
    Ok(DesktopFrame {
        x,
        y,
        width,
        height,
        outputs: powered
            .into_iter()
            .map(|m| FrameOutput {
                name: m.name,
                x: m.x,
                y: m.y,
                width: m.width,
                height: m.height,
            })
            .collect(),
    })
}

fn bounding_box(monitors: &[DisplayMonitor]) -> Result<(i32, i32, u32, u32)> {
    let min_x = monitors
        .iter()
        .map(|m| i64::from(m.x))
        .min()
        .context("no outputs")?;
    let min_y = monitors
        .iter()
        .map(|m| i64::from(m.y))
        .min()
        .context("no outputs")?;
    let max_x = monitors
        .iter()
        .map(|m| i64::from(m.x) + i64::from(m.width))
        .max()
        .context("no outputs")?;
    let max_y = monitors
        .iter()
        .map(|m| i64::from(m.y) + i64::from(m.height))
        .max()
        .context("no outputs")?;
    let width = u32::try_from(max_x - min_x)?;
    let height = u32::try_from(max_y - min_y)?;
    if !valid_dimensions(width, height) {
        bail!("invalid Hyprland display dimensions");
    }
    Ok((i32::try_from(min_x)?, i32::try_from(min_y)?, width, height))
}

/// Hyprland maps absolute virtual-pointer motion across the bounding box of
/// every enabled output; DPMS standby does not change the layout. Returns
/// `(x, y, width, height)` of that box in layout coordinates.
pub fn pointer_layout() -> Result<(i32, i32, u32, u32)> {
    let enabled: Vec<DisplayMonitor> = query::<Vec<DisplayMonitor>>("j/monitors")?
        .into_iter()
        .filter(|m| !m.disabled)
        .collect();
    bounding_box(&enabled)
}

/// Real pointer position in layout coordinates.
pub fn cursor_position() -> Result<(i32, i32)> {
    #[derive(Deserialize)]
    struct CursorPos {
        x: f64,
        y: f64,
    }
    let pos: CursorPos = query("j/cursorpos")?;
    Ok((pos.x.round() as i32, pos.y.round() as i32))
}

/// Capture the desktop frame: each powered output is copied with `grim -o`
/// and placed at its layout offset. Areas no powered output covers stay
/// black. The image is exactly `frame.width` x `frame.height`.
pub fn capture_desktop_frame_png() -> Result<Vec<u8>> {
    let frame = desktop_frame()?;
    let capture = |name: &str| -> Result<Vec<u8>> {
        let out = std::process::Command::new("grim")
            .args(["-t", "png", "-o", name, "-"])
            .output()
            .context("grim is required for Hyprland desktop capture")?;
        if !out.status.success() || out.stdout.is_empty() {
            bail!(
                "grim could not capture output {name}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(out.stdout)
    };
    if let [only] = frame.outputs.as_slice() {
        return capture(&only.name);
    }
    let mut canvas = image::RgbaImage::new(frame.width, frame.height);
    for output in &frame.outputs {
        let png = capture(&output.name)?;
        let image = image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.to_rgba8();
        if image.width() != output.width || image.height() != output.height {
            bail!(
                "output {} captured at {}x{}, expected {}x{}",
                output.name,
                image.width(),
                image.height(),
                output.width,
                output.height
            );
        }
        let (left, top) = frame.from_layout(output.x, output.y);
        image::imageops::replace(&mut canvas, &image, i64::from(left), i64::from(top));
    }
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(canvas).write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

pub fn list_windows() -> Result<Vec<Window>> {
    let monitors: Vec<Monitor> = query("j/monitors")?;
    let active = monitors
        .into_iter()
        .filter(|m| m.dpms_status)
        .flat_map(|m| [m.active_workspace.id, m.special_workspace.id])
        .filter(|id| *id != 0)
        .collect();
    windows_from_clients(query("j/clients")?, &active)
}

pub fn window_for_address(address: u64) -> Option<Window> {
    list_windows()
        .ok()?
        .into_iter()
        .find(|w| w.address == address)
}

/// AT-SPI has no native Hyprland handle. Correlate only when the title is
/// unique among this PID's mapped compositor clients, as well as AX roots.
pub fn accessibility_window(address: u64, pid: u32) -> Option<Window> {
    accessibility_target(&list_windows().ok()?, address, pid)
}

fn accessibility_target(windows: &[Window], address: u64, pid: u32) -> Option<Window> {
    let target = windows
        .iter()
        .find(|w| w.address == address && w.pid == pid)?;
    (!target.title.is_empty()
        && windows
            .iter()
            .filter(|w| w.pid == pid && w.title == target.title)
            .count()
            == 1)
        .then(|| target.clone())
}

/// The legacy PID-only bounds caller has no window identity: allow only a
/// single mapped client. Explicit IDs never fall back to this function.
pub fn window_for_pid(pid: u32) -> Option<Window> {
    let mut owned = list_windows().ok()?.into_iter().filter(|w| w.pid == pid);
    let first = owned.next()?;
    owned.next().is_none().then_some(first)
}

pub fn target_is_active(address: u64, pid: Option<u32>) -> Result<bool> {
    let target = window_for_address(address).context("Hyprland target no longer exists")?;
    if pid.is_some_and(|pid| pid != target.pid) {
        bail!("Hyprland target belongs to a different process");
    }
    let active: serde_json::Value = query("j/activewindow")?;
    Ok(
        active.get("address").and_then(|v| v.as_str()) == Some(format!("0x{address:x}").as_str())
            && active.get("pid").and_then(|v| v.as_u64()) == Some(u64::from(target.pid)),
    )
}

fn capture_target(windows: &[Window], address: u64, pid: Option<u32>) -> Result<Window> {
    let target = windows
        .iter()
        .find(|w| w.address == address)
        .context("requested Hyprland window no longer exists; refresh list_windows")?;
    if pid.is_some_and(|pid| target.pid != pid) || target.hidden {
        bail!("requested Hyprland target ownership/visibility is unproven");
    }
    // The v1 wire request takes only the low word. Refuse collisions across
    // ALL mapped clients, including other processes and hidden windows.
    if windows
        .iter()
        .filter(|w| w.address as u32 == address as u32)
        .count()
        != 1
    {
        bail!("Hyprland toplevel export handle is ambiguous");
    }
    Ok(target.clone())
}

pub fn capture(address: u64, pid: Option<u32>) -> Result<Vec<u8>> {
    let before = capture_target(&list_windows()?, address, pid)?;
    let bytes = super::hyprland_capture::capture_toplevel_png(address)?;
    let after = capture_target(&list_windows()?, address, pid)?;
    if before != after {
        bail!("Hyprland target changed during capture; refresh the snapshot");
    }
    // Keep the established window-local logical coordinate contract. Physical
    // toplevel buffers can use a fractional render scale; do not make callers
    // infer it from a monitor mode or apply an output-origin offset.
    let image = image::load_from_memory(&bytes)?;
    if image.width() == before.width && image.height() == before.height {
        return Ok(bytes);
    }
    let image = image.resize_exact(
        before.width,
        before.height,
        image::imageops::FilterType::Triangle,
    );
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(address: u64, pid: u32) -> Window {
        Window {
            address,
            pid,
            title: "Fixture".into(),
            app_id: "fixture".into(),
            x: 967,
            y: 38,
            width: 800,
            height: 600,
            workspace: 1,
            visible: true,
            hidden: false,
        }
    }

    #[test]
    fn exact_identity_never_falls_back_to_sibling_or_other_pid() {
        let windows = [window(0x10, 42), window(0x20, 42)];
        assert_eq!(
            capture_target(&windows, 0x10, Some(42)).unwrap().address,
            0x10
        );
        assert!(capture_target(&windows, 0x30, Some(42)).is_err());
        assert!(capture_target(&windows, 0x10, Some(43)).is_err());
    }

    #[test]
    fn accessibility_correlation_rejects_duplicate_compositor_titles() {
        let a = window(0x10, 42);
        let b = window(0x20, 42);
        assert!(accessibility_target(&[a.clone()], a.address, a.pid).is_some());
        assert!(accessibility_target(&[a.clone(), b], a.address, a.pid).is_none());
    }

    #[test]
    fn truncated_handle_collision_refuses_even_across_pids() {
        assert!(capture_target(
            &[window(0x100000010, 42), window(0x200000010, 43)],
            0x100000010,
            Some(42)
        )
        .is_err());
    }

    #[test]
    fn off_workspace_target_is_capturable_but_hidden_target_refuses() {
        let mut target = window(0x10, 42);
        target.visible = false;
        assert!(capture_target(&[target.clone()], 0x10, Some(42)).is_ok());
        target.hidden = true;
        assert!(capture_target(&[target], 0x10, Some(42)).is_err());
    }

    #[test]
    fn stalled_ipc_reply_is_bounded() {
        let (mut client, _server) = UnixStream::pair().unwrap();
        let start = Instant::now();
        assert!(read_reply(&mut client, b"j/clients", Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    fn replying_ipc(bytes: Vec<u8>, delay: Duration) -> UnixStream {
        let (client, mut server) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            server
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let mut request = [0; 128];
            assert!(server.read(&mut request).unwrap() > 0);
            std::thread::sleep(delay);
            // An oversized reply may be rejected before the writer finishes.
            let _ = server.write_all(&bytes);
        });
        client
    }

    #[test]
    fn timed_out_observation_uses_fresh_connection_and_discards_partial_bytes() {
        let mut attempts = 0;
        let mut stalled = Vec::new();
        let value: serde_json::Value = query_with(
            "j/clients",
            Duration::from_millis(30),
            Duration::from_secs(1),
            Duration::from_millis(1),
            |_| {
                attempts += 1;
                if attempts == 1 {
                    let (client, mut server) = UnixStream::pair()?;
                    server.write_all(b"{\"stale\":")?;
                    stalled.push(server);
                    Ok(client)
                } else {
                    Ok(replying_ipc(b"[]".to_vec(), Duration::ZERO))
                }
            },
        )
        .unwrap();
        assert_eq!(value, serde_json::json!([]));
        assert_eq!(attempts, 2);
    }

    #[test]
    fn healthy_slow_observation_is_not_retried() {
        let mut attempts = 0;
        let value: serde_json::Value = query_with(
            "j/monitors",
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_millis(1),
            |_| {
                attempts += 1;
                Ok(replying_ipc(b"[]".to_vec(), Duration::from_millis(30)))
            },
        )
        .unwrap();
        assert_eq!(value, serde_json::json!([]));
        assert_eq!(attempts, 1);
    }

    #[test]
    fn observation_connection_and_attestation_errors_are_not_retried() {
        for timeout in [false, true] {
            let mut attempts = 0;
            let error = query_with::<serde_json::Value>(
                "j/clients",
                QUERY_TIMEOUT,
                QUERY_TOTAL_TIMEOUT,
                QUERY_RETRY_BACKOFF,
                |_| {
                    attempts += 1;
                    if timeout {
                        return Err(io::Error::from(io::ErrorKind::TimedOut).into());
                    }
                    bail!("could not verify same-user compositor peer");
                },
            )
            .unwrap_err();
            assert_eq!(attempts, 1);
            assert!(timeout || error.to_string().contains("same-user compositor peer"));
        }
    }

    #[test]
    fn observation_retry_must_pass_fresh_attestation() {
        let mut attempts = 0;
        let mut stalled = Vec::new();
        let error = query_with::<serde_json::Value>(
            "j/activewindow",
            Duration::from_millis(30),
            Duration::from_secs(1),
            Duration::from_millis(1),
            |_| {
                attempts += 1;
                if attempts == 2 {
                    bail!("Hyprland compositor changed during observation retry");
                }
                let (client, server) = UnixStream::pair()?;
                stalled.push(server);
                Ok(client)
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 2);
        assert!(error.to_string().contains("compositor changed"));
    }

    #[test]
    fn malformed_truncated_and_oversized_observations_are_not_retried() {
        for bytes in [
            b"invalid".to_vec(),
            b"{\"pid\"".to_vec(),
            vec![b' '; MAX_REPLY_BYTES + 1],
        ] {
            let mut attempts = 0;
            assert!(query_with::<serde_json::Value>(
                "j/clients",
                QUERY_TIMEOUT,
                QUERY_TOTAL_TIMEOUT,
                QUERY_RETRY_BACKOFF,
                |_| {
                    attempts += 1;
                    Ok(replying_ipc(bytes.clone(), Duration::ZERO))
                },
            )
            .is_err());
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn observation_total_deadline_includes_connect_and_read() {
        let mut attempts = 0;
        let mut stalled = Vec::new();
        let start = Instant::now();
        let error = query_with::<serde_json::Value>(
            "j/clients",
            Duration::from_millis(80),
            Duration::from_millis(110),
            Duration::from_millis(1),
            |deadline| {
                attempts += 1;
                assert!(
                    deadline.saturating_duration_since(Instant::now()) <= Duration::from_millis(80)
                );
                std::thread::sleep(Duration::from_millis(10));
                let (client, server) = UnixStream::pair()?;
                stalled.push(server);
                Ok(client)
            },
        )
        .unwrap_err();
        assert!(is_query_timeout(&error));
        assert!(attempts <= QUERY_MAX_ATTEMPTS);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn observation_attempt_cap_applies_with_a_generous_total_budget() {
        let mut attempts = 0;
        let mut stalled = Vec::new();
        let error = query_with::<serde_json::Value>(
            "j/clients",
            Duration::from_millis(30),
            Duration::from_secs(10),
            Duration::from_millis(1),
            |_| {
                attempts += 1;
                let (client, server) = UnixStream::pair()?;
                stalled.push(server);
                Ok(client)
            },
        )
        .unwrap_err();
        assert!(is_query_timeout(&error));
        assert_eq!(attempts, 2);
    }

    #[test]
    fn observation_never_writes_after_connect_consumes_total_deadline() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut client = Some(client);
        let mut attempts = 0;
        let error = query_with::<serde_json::Value>(
            "j/clients",
            Duration::from_millis(10),
            Duration::from_millis(10),
            Duration::from_millis(1),
            |_| {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(30));
                Ok(client.take().unwrap())
            },
        )
        .unwrap_err();
        assert!(is_query_timeout(&error));
        assert_eq!(attempts, 1);
        // The client was dropped without sending a request: EOF, not bytes.
        assert_eq!(server.read(&mut [0; 64]).unwrap(), 0);
    }

    #[test]
    fn input_and_unknown_json_commands_are_never_retried() {
        for command in ["dispatch nop", "j/dispatch nop", "j/unknown"] {
            let mut attempts = 0;
            let mut stalled = Vec::new();
            assert!(query_with::<serde_json::Value>(
                command,
                Duration::from_millis(30),
                Duration::from_secs(1),
                Duration::from_millis(1),
                |_| {
                    attempts += 1;
                    let (client, server) = UnixStream::pair()?;
                    stalled.push(server);
                    Ok(client)
                },
            )
            .is_err());
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn observation_timeout_classifier_is_closed() {
        for (kind, expected) in [
            (io::ErrorKind::WouldBlock, true),
            (io::ErrorKind::TimedOut, true),
            (io::ErrorKind::ConnectionRefused, false),
            (io::ErrorKind::ConnectionReset, false),
            (io::ErrorKind::Interrupted, false),
            (io::ErrorKind::UnexpectedEof, false),
            (io::ErrorKind::PermissionDenied, false),
            (io::ErrorKind::InvalidData, false),
        ] {
            let error = anyhow::Error::from(io::Error::from(kind)).context("query");
            assert_eq!(is_query_timeout(&error), expected);
        }
        assert!(!is_query_timeout(&anyhow::anyhow!("identity unproven")));
        assert!(is_query_timeout(
            &query_time_remaining(Instant::now()).unwrap_err()
        ));
    }

    #[test]
    fn logical_resize_is_bounded_before_allocation() {
        assert!(valid_dimensions(3840, 2160));
        assert!(!valid_dimensions(0, 1));
        assert!(!valid_dimensions(u32::MAX, u32::MAX));
    }

    fn display_monitor() -> DisplayMonitor {
        serde_json::from_value(serde_json::json!({
            "width": 1920, "height": 1080, "scale": 1.0,
            "x": 0, "y": 0, "transform": 0,
        }))
        .unwrap()
    }

    #[test]
    fn display_identity_accepts_qualified_native_geometry() {
        assert_eq!(
            screen_size_from_monitors(vec![display_monitor()]).unwrap(),
            (1920, 1080, 1.0)
        );
    }

    fn monitor_at(name: &str, x: i32, y: i32) -> DisplayMonitor {
        let mut monitor = display_monitor();
        (monitor.name, monitor.x, monitor.y) = (name.to_string(), x, y);
        monitor
    }

    #[test]
    fn desktop_frame_spans_every_powered_output() {
        let frame =
            desktop_frame_from_monitors(vec![monitor_at("A", -1920, 0), monitor_at("B", 0, 0)])
                .unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (-1920, 0, 3840, 1080)
        );
        assert_eq!(frame.to_layout(1920, 10), (0, 10));
        assert_eq!(frame.from_layout(-1920, 0), (0, 0));
        assert_eq!(
            screen_size_from_monitors(vec![monitor_at("A", 0, 0), monitor_at("B", 1920, 0)])
                .unwrap(),
            (3840, 1080, 1.0)
        );
    }

    #[test]
    fn desktop_frame_ignores_outputs_in_standby_or_disabled() {
        let mut off = monitor_at("A", -1920, 0);
        off.dpms_status = false;
        let frame = desktop_frame_from_monitors(vec![off.clone(), monitor_at("B", 0, 0)]).unwrap();
        assert_eq!((frame.x, frame.width, frame.outputs.len()), (0, 1920, 1));
        // Only the left monitor on: the frame follows it to its layout offset.
        let mut right_off = monitor_at("B", 0, 0);
        right_off.disabled = true;
        let frame =
            desktop_frame_from_monitors(vec![monitor_at("A", -1920, 0), right_off]).unwrap();
        assert_eq!((frame.x, frame.width), (-1920, 1920));
        assert!(desktop_frame_from_monitors(vec![off]).is_err());
    }

    #[test]
    fn display_identity_rejects_missing_and_unsupported_frames() {
        assert!(screen_size_from_monitors(vec![]).is_err());
        for scale in [0.0, 1.25, 2.0, f64::NAN, f64::INFINITY] {
            let mut monitor = display_monitor();
            monitor.scale = scale;
            assert!(screen_size_from_monitors(vec![monitor]).is_err());
        }
        for transform in [1, 7] {
            let mut monitor = display_monitor();
            monitor.transform = transform;
            assert!(screen_size_from_monitors(vec![monitor]).is_err());
        }
    }

    #[test]
    fn display_identity_rejects_empty_oversized_and_missing_geometry() {
        for (width, height) in [(0, 1080), (1920, 0), (u32::MAX, u32::MAX)] {
            let mut monitor = display_monitor();
            (monitor.width, monitor.height) = (width, height);
            assert!(screen_size_from_monitors(vec![monitor]).is_err());
        }
        let value = serde_json::json!({
            "width": 1920, "height": 1080, "scale": 1.0, "x": 0, "y": 0,
        });
        assert!(serde_json::from_value::<DisplayMonitor>(value).is_err());
    }
}
