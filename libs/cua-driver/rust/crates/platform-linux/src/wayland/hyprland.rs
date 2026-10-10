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
    /// Absent on compositors that do not report it; never read as native.
    #[serde(default)]
    xwayland: Option<bool>,
    /// 0 is the focused client; larger is longer ago. Absent on old builds.
    #[serde(rename = "focusHistoryID", default)]
    focus_history: i64,
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
    /// Name of the output this one mirrors, or `"none"`.
    #[serde(rename = "mirrorOf", default)]
    mirror_of: String,
}

fn powered_by_default() -> bool {
    true
}

impl DisplayMonitor {
    /// A mirror repeats another output's content and has no area of its own
    /// in the layout.
    fn in_layout(&self) -> bool {
        !self.disabled && matches!(self.mirror_of.as_str(), "" | "none")
    }

    /// In the layout and not in DPMS standby: the user can actually see it.
    fn powered(&self) -> bool {
        self.in_layout() && self.dpms_status
    }

    /// The output mode divided by its scale, rounded as Hyprland rounds its
    /// logical monitor size. Window geometry and the layout use this size.
    /// Hyprland reports the mode in the panel's native orientation; quarter
    /// turns (wl_output transforms 1 and 3) swap the logical axes, and flipped
    /// transforms (4-7) are refused.
    fn logical_size(&self) -> Result<(u32, u32)> {
        if !self.scale.is_finite() || self.scale <= 0.0 {
            bail!("invalid Hyprland display scale");
        }
        if self.transform > 3 {
            bail!("Hyprland display identity requires unflipped outputs");
        }
        if !valid_dimensions(self.width, self.height) {
            bail!("invalid Hyprland display dimensions");
        }
        let (width, height) = super::logical_output_size((self.width, self.height), self.transform);
        let logical_width = (f64::from(width) / self.scale).round();
        let logical_height = (f64::from(height) / self.scale).round();
        if !logical_width.is_finite()
            || !logical_height.is_finite()
            || logical_width < 1.0
            || logical_height < 1.0
            || logical_width > f64::from(u32::MAX)
            || logical_height > f64::from(u32::MAX)
        {
            bail!("invalid Hyprland logical display dimensions");
        }
        let (logical_width, logical_height) = (logical_width as u32, logical_height as u32);
        if !valid_dimensions(logical_width, logical_height) {
            bail!("invalid Hyprland logical display dimensions");
        }
        Ok((logical_width, logical_height))
    }
}

/// One output's logical rectangle in Hyprland layout coordinates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameOutput {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The desktop action frame: the bounding box of every powered output
/// (enabled and not in DPMS standby), in logical Hyprland layout coordinates.
/// Desktop-scope screenshots, sizes and actions all use this frame, with
/// frame pixel (0, 0) at layout point (`x`, `y`). It is re-derived from the
/// compositor on every call, so monitors turned on or off take effect
/// immediately.
#[derive(Clone, Debug, PartialEq)]
pub struct DesktopFrame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Output scale reported with the frame: the largest scale among its
    /// outputs, so a single output reports its own scale.
    pub scale: f64,
    pub outputs: Vec<FrameOutput>,
}

impl DesktopFrame {
    pub fn to_layout(&self, x: i32, y: i32) -> (i32, i32) {
        (self.x.saturating_add(x), self.y.saturating_add(y))
    }

    pub fn from_layout(&self, x: i32, y: i32) -> (i32, i32) {
        (x.saturating_sub(self.x), y.saturating_sub(self.y))
    }

    /// Whether a layout point lies on one of the frame's outputs. With
    /// several outputs the frame's bounding box can include gaps and outputs
    /// in standby, where the compositor would move the pointer to some other
    /// output edge instead of the point the caller saw.
    pub fn output_contains_layout(&self, x: i32, y: i32) -> bool {
        self.outputs.iter().any(|o| {
            let (x, y) = (i64::from(x), i64::from(y));
            x >= i64::from(o.x)
                && y >= i64::from(o.y)
                && x < i64::from(o.x) + i64::from(o.width)
                && y < i64::from(o.y) + i64::from(o.height)
        })
    }

    /// Move window geometry from layout coordinates into this frame, so it
    /// shares an origin with a screenshot of the frame.
    pub fn rebase_windows(&self, windows: &mut [crate::x11::WindowInfo]) {
        for window in windows {
            (window.x, window.y) = self.from_layout(window.x, window.y);
        }
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
    /// `Some(true)` for XWayland, `Some(false)` for native Wayland, `None`
    /// when the compositor did not report the flag.
    pub xwayland: Option<bool>,
    hidden: bool,
    /// Focus recency (0 = focused); the closest thing to z-order Hyprland
    /// exposes.
    pub focus_order: i64,
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
    let read_only = matches!(
        command,
        "j/monitors" | "j/clients" | "j/activewindow" | "j/cursorpos"
    );
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
        if address == 0 || !seen.insert(address) {
            bail!("invalid or duplicate Hyprland window identity");
        }
        let (Ok(pid), Ok(width), Ok(height)) = (
            u32::try_from(c.pid),
            u32::try_from(c.size[0]),
            u32::try_from(c.size[1]),
        ) else {
            continue;
        };
        if pid == 0 || !valid_dimensions(width, height) {
            continue;
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
            xwayland: c.xwayland,
            hidden: c.hidden,
            focus_order: c.focus_history,
        });
    }
    Ok(windows)
}

fn valid_dimensions(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_LOGICAL_PIXELS
}

/// Content-free logical geometry of the desktop frame (see [`DesktopFrame`])
/// plus its output scale. A single output's frame is its mode divided by its
/// scale, rounded as Hyprland rounds its logical monitor size. Desktop
/// capture, desktop action admission, and the virtual-pointer extent all use
/// this frame, matching the logical window geometry and window captures. The
/// common policy adapter uses it for display-scoped observation. Never
/// substitute a screenshot, XWayland root, or guessed primary monitor here.
pub fn screen_size() -> Result<(u32, u32, f64)> {
    screen_size_from_monitors(query("j/monitors")?)
}

fn screen_size_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<(u32, u32, f64)> {
    let frame = desktop_frame_from_monitors(monitors)?;
    Ok((frame.width, frame.height, frame.scale))
}

/// The desktop frame and every monitor in the layout with its power state
/// and scale, both from one compositor snapshot so the size and the monitor
/// list always describe the same layout. Sizes and positions are
/// desktop-frame (logical) pixels; positions are `None` for monitors outside
/// the frame (standby). Mirrors are omitted: they repeat another output.
pub fn screen_report() -> Result<(DesktopFrame, serde_json::Value)> {
    screen_report_from_monitors(query("j/monitors")?)
}

fn screen_report_from_monitors(
    monitors: Vec<DisplayMonitor>,
) -> Result<(DesktopFrame, serde_json::Value)> {
    let frame = desktop_frame_from_monitors(monitors.clone())?;
    let report = monitor_report_for(&monitors, &frame);
    Ok((frame, report))
}

fn monitor_report_for(monitors: &[DisplayMonitor], frame: &DesktopFrame) -> serde_json::Value {
    serde_json::Value::Array(
        monitors
            .iter()
            .filter(|m| m.in_layout())
            .map(|m| {
                let size = m.logical_size().ok();
                let (fx, fy) = frame.from_layout(m.x, m.y);
                let in_frame = frame.outputs.iter().any(|o| o.name == m.name);
                serde_json::json!({
                    "name": m.name,
                    "width": size.map(|(width, _)| width),
                    "height": size.map(|(_, height)| height),
                    "scale": m.scale,
                    "powered": m.powered(),
                    "frame_x": in_frame.then_some(fx),
                    "frame_y": in_frame.then_some(fy),
                })
            })
            .collect(),
    )
}

/// One compositor snapshot of everything a desktop-scope action needs: the
/// desktop frame (screenshot coordinates) and the virtual-pointer layout.
/// `pointer` is `None` when the layout is a single output the pointer layout
/// cannot qualify; the virtual pointer then keeps that output's own mode.
#[derive(Clone, Debug)]
pub struct DesktopSnapshot {
    pub frame: DesktopFrame,
    pub pointer: Option<(i32, i32, u32, u32)>,
}

pub fn desktop_snapshot() -> Result<DesktopSnapshot> {
    desktop_snapshot_from_monitors(query("j/monitors")?)
}

fn desktop_snapshot_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<DesktopSnapshot> {
    Ok(DesktopSnapshot {
        frame: desktop_frame_from_monitors(monitors.clone())?,
        pointer: pointer_space_from_monitors(monitors)?,
    })
}

/// Current desktop frame spanning every powered output.
pub fn desktop_frame() -> Result<DesktopFrame> {
    desktop_frame_from_monitors(query("j/monitors")?)
}

fn desktop_frame_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<DesktopFrame> {
    let in_layout: Vec<DisplayMonitor> = monitors
        .into_iter()
        .filter(DisplayMonitor::in_layout)
        .collect();
    // An output in DPMS standby shows nothing, so the frame leaves it out
    // while another output is on. With every output in standby the display is
    // only idle (hypridle turned it off): keep the whole layout, as a single
    // output always did, so observation and the input that wakes it still work.
    let powered: Vec<DisplayMonitor> = if in_layout.iter().any(|m| m.dpms_status) {
        in_layout.into_iter().filter(|m| m.dpms_status).collect()
    } else {
        in_layout
    };
    if powered.is_empty() {
        bail!("Hyprland has no enabled output: every monitor is disabled or mirrors another");
    }
    let mut outputs = Vec::with_capacity(powered.len());
    for monitor in &powered {
        // A lone output may be rotated: its logical rectangle swaps axes and
        // the single-output capture rotates into it. Composing several outputs
        // assumes unrotated ones, so a rotated output among others refuses.
        if monitor.transform != 0 && powered.len() > 1 {
            bail!("Hyprland multi-monitor desktops require unrotated outputs");
        }
        outputs.push(frame_output(monitor)?);
    }
    let (x, y, width, height) = bounding_box(&outputs)?;
    let scale = powered.iter().map(|m| m.scale).fold(f64::MIN, f64::max);
    Ok(DesktopFrame {
        x,
        y,
        width,
        height,
        scale,
        outputs,
    })
}

fn frame_output(monitor: &DisplayMonitor) -> Result<FrameOutput> {
    let (width, height) = monitor.logical_size()?;
    Ok(FrameOutput {
        name: monitor.name.clone(),
        x: monitor.x,
        y: monitor.y,
        width,
        height,
    })
}

/// Bounding box of logical output rectangles, in layout coordinates.
fn bounding_box(outputs: &[FrameOutput]) -> Result<(i32, i32, u32, u32)> {
    let min_x = outputs
        .iter()
        .map(|o| i64::from(o.x))
        .min()
        .context("no outputs")?;
    let min_y = outputs
        .iter()
        .map(|o| i64::from(o.y))
        .min()
        .context("no outputs")?;
    let max_x = outputs
        .iter()
        .map(|o| i64::from(o.x) + i64::from(o.width))
        .max()
        .context("no outputs")?;
    let max_y = outputs
        .iter()
        .map(|o| i64::from(o.y) + i64::from(o.height))
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
/// every enabled, unmirrored output's logical rectangle; DPMS standby does
/// not change the layout. Returns `(x, y, width, height)` of that box in
/// layout coordinates. A lone output may be rotated (its logical rectangle
/// swaps axes); a rotated output among others is refused.
fn pointer_layout_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<(i32, i32, u32, u32)> {
    let in_layout: Vec<&DisplayMonitor> = monitors.iter().filter(|m| m.in_layout()).collect();
    let mut outputs = Vec::new();
    for monitor in &in_layout {
        if monitor.transform != 0 && in_layout.len() > 1 {
            bail!(
                "Hyprland pointer layout requires unrotated outputs ({} has transform {}{})",
                monitor.name,
                monitor.transform,
                if monitor.dpms_status {
                    ""
                } else {
                    ", in standby"
                }
            );
        }
        outputs.push(frame_output(monitor)?);
    }
    bounding_box(&outputs)
}

/// Virtual-pointer layout, or `None` for a single output it cannot qualify
/// (a rotated one), whose own mode is then the whole layout. With several
/// outputs in the layout no single output's mode is the layout, so an
/// unqualified output (even one in standby) is an error, not a fallback.
pub fn pointer_space() -> Result<Option<(i32, i32, u32, u32)>> {
    pointer_space_from_monitors(query("j/monitors")?)
}

fn pointer_space_from_monitors(
    monitors: Vec<DisplayMonitor>,
) -> Result<Option<(i32, i32, u32, u32)>> {
    let in_layout = monitors.iter().filter(|m| m.in_layout()).count();
    match pointer_layout_from_monitors(monitors) {
        Ok(layout) => Ok(Some(layout)),
        Err(_) if in_layout <= 1 => Ok(None),
        // Keep the cause in the message itself: tools report only the outer
        // error, and the agent needs to know which output blocks the action.
        Err(error) => Err(anyhow::anyhow!(
            "{error}; the virtual pointer spans every monitor in the layout, including ones in standby, so it cannot be sized"
        )),
    }
}

/// Desktop capture for layouts the generic capture cannot represent. With a
/// single output this returns `None` and the caller keeps its capture
/// cascade, whose buffer is that output. With several outputs the generic
/// capture would copy only the first one, so each powered output is copied
/// with `grim -o`, scaled to its logical size, and placed at its layout
/// offset. Areas no powered output covers stay black. The image is exactly
/// the desktop frame.
///
/// The generic path is kept only once the layout is known to be a single
/// output: if the monitor query fails, the capture fails too, rather than
/// copying one output of what may be a multi-monitor desktop.
pub fn composite_desktop_capture() -> Option<Result<Vec<u8>>> {
    let monitors: Vec<DisplayMonitor> = match query("j/monitors") {
        Ok(monitors) => monitors,
        Err(error) => return Some(Err(error)),
    };
    // One output: use the generic capture. It needs no desktop frame, so
    // pixel-only callers such as trajectory recording keep working on a
    // rotated output, which desktop actions refuse.
    if monitors.len() <= 1 {
        return None;
    }
    Some(desktop_frame_from_monitors(monitors).and_then(|frame| compose_desktop_capture(&frame)))
}

/// The desktop frame and whether capturing it needs per-output composition
/// (more than one monitor in the layout), read from one compositor snapshot.
pub struct DesktopCapturePlan {
    pub frame: DesktopFrame,
    pub composite: bool,
}

pub fn desktop_capture_plan() -> Result<DesktopCapturePlan> {
    desktop_capture_plan_from_monitors(query("j/monitors")?)
}

fn desktop_capture_plan_from_monitors(monitors: Vec<DisplayMonitor>) -> Result<DesktopCapturePlan> {
    let composite = monitors.len() > 1;
    Ok(DesktopCapturePlan {
        frame: desktop_frame_from_monitors(monitors)?,
        composite,
    })
}

/// Compose `frame` from per-output captures (see [`composite_desktop_capture`]).
pub fn compose_desktop_capture(frame: &DesktopFrame) -> Result<Vec<u8>> {
    let canvas = compose_frame(frame, |output| {
        let png = capture_output_png(&output.name)?;
        Ok(image::load_from_memory_with_format(&png, image::ImageFormat::Png)?.to_rgba8())
    })?;
    let mut encoded = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(canvas).write_to(&mut encoded, image::ImageFormat::Png)?;
    Ok(encoded.into_inner())
}

fn capture_output_png(name: &str) -> Result<Vec<u8>> {
    // The PNG is decoded right away, so skip compression.
    let out = std::process::Command::new("grim")
        .args(["-t", "png", "-l", "0", "-o", name, "-"])
        .output()
        .context("grim is required for multi-monitor Hyprland desktop capture")?;
    if !out.status.success() || out.stdout.is_empty() {
        bail!(
            "grim could not capture output {name}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(out.stdout)
}

/// Place each output's capture at its logical rectangle inside the frame. A
/// scaled output's capture is resized to its logical size, which must be a
/// uniform scaling of the captured image.
fn compose_frame(
    frame: &DesktopFrame,
    mut capture: impl FnMut(&FrameOutput) -> Result<image::RgbaImage>,
) -> Result<image::RgbaImage> {
    let mut canvas = image::RgbaImage::new(frame.width, frame.height);
    for output in &frame.outputs {
        let mut image = capture(output)?;
        if (image.width(), image.height()) != (output.width, output.height) {
            let scale_x = f64::from(image.width()) / f64::from(output.width);
            let scale_y = f64::from(image.height()) / f64::from(output.height);
            if (scale_x - scale_y).abs() > 0.01 {
                bail!(
                    "output {} captured at {}x{}, which does not scale uniformly to its {}x{} \
                     logical size",
                    output.name,
                    image.width(),
                    image.height(),
                    output.width,
                    output.height
                );
            }
            image = image::imageops::resize(
                &image,
                output.width,
                output.height,
                image::imageops::FilterType::Lanczos3,
            );
        }
        let (left, top) = frame.from_layout(output.x, output.y);
        image::imageops::replace(&mut canvas, &image, i64::from(left), i64::from(top));
    }
    Ok(canvas)
}

/// wl_output transform of the single active output. Full-display screencopy
/// frames arrive in the panel's native orientation; callers use this to turn
/// them into the logical desktop frame.
pub fn single_output_transform() -> Result<u32> {
    let monitors: Vec<DisplayMonitor> = query("j/monitors")?;
    let [monitor] = monitors.as_slice() else {
        bail!("Hyprland display identity requires exactly one active output");
    };
    Ok(monitor.transform)
}

pub fn list_windows() -> Result<Vec<Window>> {
    let monitors: Vec<Monitor> = query("j/monitors")?;
    // Windows on an output in DPMS standby are not visible while another
    // output is on. With every output in standby the display is only idle,
    // so its workspaces stay on screen.
    let any_powered = monitors.iter().any(|m| m.dpms_status);
    let active = monitors
        .into_iter()
        .filter(|m| m.dpms_status || !any_powered)
        .flat_map(|m| [m.active_workspace.id, m.special_workspace.id])
        .filter(|id| *id != 0)
        .collect();
    windows_from_clients(query("j/clients")?, &active)
}

#[derive(Deserialize)]
struct CursorPos {
    x: f64,
    y: f64,
}

/// The pointer in Hyprland's global layout coordinates.
pub fn cursor_position() -> Result<(f64, f64)> {
    let pos: CursorPos = query("j/cursorpos")?;
    Ok((pos.x, pos.y))
}

/// Move the pointer (`dispatch movecursor`), no button state. For the
/// presence shape probe only.
pub fn move_cursor(x: f64, y: f64) -> Result<()> {
    let mut ipc = ipc_connection()?;
    let command = format!(
        "dispatch movecursor {} {}",
        x.round() as i64,
        y.round() as i64
    );
    let reply = read_reply(&mut ipc, command.as_bytes(), QUERY_TIMEOUT)?;
    if reply.trim_ascii() != b"ok" {
        bail!(
            "Hyprland movecursor refused: {}",
            String::from_utf8_lossy(&reply)
        );
    }
    Ok(())
}

/// The active window's address, or `None` when no window has focus.
pub fn active_window_address() -> Result<Option<u64>> {
    let active: serde_json::Value = query("j/activewindow")?;
    let Some(address) = active.get("address").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    let address = u64::from_str_radix(address.strip_prefix("0x").unwrap_or(address), 16)
        .context("invalid Hyprland active window address")?;
    Ok((address != 0).then_some(address))
}

/// The only output's active (non-special) workspace, or `None` with several
/// outputs, where one workspace id cannot describe what the person sees.
pub fn single_output_workspace() -> Result<Option<i64>> {
    let monitors: Vec<Monitor> = query("j/monitors")?;
    Ok(match monitors.as_slice() {
        [monitor] if monitor.active_workspace.id != 0 => Some(monitor.active_workspace.id),
        _ => None,
    })
}

/// Show one workspace (`dispatch workspace <id>`). Only for handing the
/// person's empty workspace back after an exact-window browser setup or
/// consent prompt moved them to the browser's.
pub fn restore_workspace(id: i64) -> Result<()> {
    let mut ipc = ipc_connection()?;
    let command = format!("dispatch workspace {id}");
    let reply = read_reply(&mut ipc, command.as_bytes(), QUERY_TIMEOUT)?;
    if reply.trim_ascii() != b"ok" {
        bail!(
            "Hyprland workspace switch refused: {}",
            String::from_utf8_lossy(&reply)
        );
    }
    Ok(())
}

/// Focus one exact window (`dispatch focuswindow address:0x…`). Only for
/// handing focus back to the window that held it before an exact-window
/// browser setup transaction.
pub fn restore_focus_to_window(address: u64) -> Result<()> {
    let mut ipc = ipc_connection()?;
    let command = format!("dispatch focuswindow address:0x{address:x}");
    let reply = read_reply(&mut ipc, command.as_bytes(), QUERY_TIMEOUT)?;
    if reply.trim_ascii() != b"ok" {
        bail!(
            "Hyprland focuswindow refused: {}",
            String::from_utf8_lossy(&reply)
        );
    }
    Ok(())
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

/// True only when the compositor positively identifies the exact
/// `(address, pid)` client as native Wayland. A zero address, missing or
/// mismatched window, or an absent/XWayland flag all refuse.
pub fn native_client_attested(address: u64, pid: u32) -> bool {
    list_windows().is_ok_and(|windows| native_attestation(&windows, address, pid))
}

fn native_attestation(windows: &[Window], address: u64, pid: u32) -> bool {
    address != 0
        && windows
            .iter()
            .find(|w| w.address == address)
            .is_some_and(|w| w.pid == pid && w.xwayland == Some(false))
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
            xwayland: Some(false),
            hidden: false,
            focus_order: 0,
        }
    }

    #[test]
    fn native_attestation_requires_exact_address_pid_and_native_flag() {
        let native = window(0x10, 42);
        let mut xwayland = window(0x20, 42);
        xwayland.xwayland = Some(true);
        let mut absent = window(0x30, 42);
        absent.xwayland = None;
        let windows = [native, xwayland, absent];
        assert!(native_attestation(&windows, 0x10, 42));
        assert!(!native_attestation(&windows, 0x20, 42));
        assert!(!native_attestation(&windows, 0x30, 42));
        assert!(!native_attestation(&windows, 0x10, 43));
        assert!(!native_attestation(&windows, 0x40, 42));
        assert!(!native_attestation(&windows, 0, 42));
    }

    #[test]
    fn client_xwayland_flag_is_tri_state() {
        let active = HashSet::from([1]);
        let with = |flag: Option<bool>| {
            let mut c = client("0x10", 42, [800, 600]);
            c.xwayland = flag;
            windows_from_clients(vec![c], &active)
                .unwrap()
                .remove(0)
                .xwayland
        };
        assert_eq!(with(Some(true)), Some(true));
        assert_eq!(with(Some(false)), Some(false));
        assert_eq!(with(None), None);
        // `client()` omits the field entirely: absent must deserialize to None.
        assert_eq!(client("0x10", 42, [800, 600]).xwayland, None);
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
        assert!(accessibility_target(std::slice::from_ref(&a), a.address, a.pid).is_some());
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

    fn client(address: &str, pid: i64, size: [i32; 2]) -> Client {
        serde_json::from_value(serde_json::json!({
            "address": address,
            "mapped": true,
            "hidden": false,
            "pid": pid,
            "title": "Fixture",
            "class": "fixture",
            "at": [0, 0],
            "size": size,
            "workspace": {"id": 1},
        }))
        .unwrap()
    }

    #[test]
    fn unrepresentable_client_does_not_hide_valid_siblings() {
        let active = HashSet::from([1]);
        let windows = windows_from_clients(
            vec![
                client("0x10", 42, [800, 600]),
                client("0x20", 43, [6, -3]),
                client("0x30", -1, [800, 600]),
                client("0x40", 44, [0, 600]),
                client("0x50", 0, [800, 600]),
                client("0x60", 45, [i32::MAX, i32::MAX]),
                client("0x70", 46, [640, 480]),
            ],
            &active,
        )
        .unwrap();

        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].address, 0x10);
        assert_eq!(windows[1].address, 0x70);
    }

    #[test]
    fn null_and_duplicate_client_addresses_still_fail() {
        let active = HashSet::from([1]);
        let null_address =
            windows_from_clients(vec![client("0x0", 42, [800, 600])], &active).unwrap_err();
        assert_eq!(
            null_address.to_string(),
            "invalid or duplicate Hyprland window identity"
        );

        let duplicate_address = windows_from_clients(
            vec![
                client("0x10", 42, [800, 600]),
                client("0x10", 43, [800, 600]),
            ],
            &active,
        )
        .unwrap_err();
        assert_eq!(
            duplicate_address.to_string(),
            "invalid or duplicate Hyprland window identity"
        );

        let duplicate_unrepresentable = windows_from_clients(
            vec![client("0x10", 42, [800, 600]), client("0x10", 43, [6, -3])],
            &active,
        )
        .unwrap_err();
        assert_eq!(
            duplicate_unrepresentable.to_string(),
            "invalid or duplicate Hyprland window identity"
        );

        let unrepresentable_duplicate = windows_from_clients(
            vec![client("0x20", 43, [6, -3]), client("0x20", 44, [800, 600])],
            &active,
        )
        .unwrap_err();
        assert_eq!(
            unrepresentable_duplicate.to_string(),
            "invalid or duplicate Hyprland window identity"
        );
    }

    fn display_monitor() -> DisplayMonitor {
        serde_json::from_value(serde_json::json!({
            "width": 1920, "height": 1080, "scale": 1.0,
            "x": 0, "y": 0, "transform": 0,
        }))
        .unwrap()
    }

    fn scaled_monitor(width: u32, height: u32, scale: f64) -> DisplayMonitor {
        let mut monitor = display_monitor();
        (monitor.width, monitor.height, monitor.scale) = (width, height, scale);
        monitor
    }

    // (output mode, scale, logical frame). 1.6666666 is the #4219 laptop.
    const SCALED_OUTPUTS: [((u32, u32), f64, (u32, u32)); 5] = [
        ((1920, 1080), 1.0, (1920, 1080)),
        ((2560, 1600), 1.25, (2048, 1280)),
        ((2880, 1800), 1.5, (1920, 1200)),
        ((2160, 1350), 1.6666666, (1296, 810)),
        ((3840, 2160), 2.0, (1920, 1080)),
    ];

    #[test]
    fn display_identity_publishes_the_logical_frame_and_scale() {
        for ((width, height), scale, logical) in SCALED_OUTPUTS {
            assert_eq!(
                screen_size_from_monitors(vec![scaled_monitor(width, height, scale)]).unwrap(),
                (logical.0, logical.1, scale),
                "{width}x{height} @ {scale}"
            );
        }
    }

    /// Hyprland maps `motion_absolute(x, y, x_extent, y_extent)` onto its
    /// logical layout as `x / x_extent`. A desktop screenshot pixel must put
    /// the pointer on the physical pixel the agent saw in the native capture.
    #[test]
    fn scaled_desktop_frame_and_virtual_pointer_extent_agree() {
        for ((mode_w, mode_h), scale, _) in SCALED_OUTPUTS {
            let (frame_w, frame_h, _) =
                screen_size_from_monitors(vec![scaled_monitor(mode_w, mode_h, scale)]).unwrap();
            let layout =
                pointer_layout_from_monitors(vec![scaled_monitor(mode_w, mode_h, scale)]).unwrap();
            let (origin_x, origin_y, extent_w, extent_h) =
                super::super::select_virtual_pointer_space((mode_w, mode_h), Some(layout));
            let extent = (extent_w, extent_h);
            assert_eq!((origin_x, origin_y), (0, 0), "{mode_w}x{mode_h} @ {scale}");
            assert_eq!(extent, (frame_w, frame_h), "{mode_w}x{mode_h} @ {scale}");

            let landed = |(x, y): (u32, u32), (extent_w, extent_h): (u32, u32)| {
                (
                    f64::from(x) / f64::from(extent_w) * f64::from(frame_w) * scale,
                    f64::from(y) / f64::from(extent_h) * f64::from(frame_h) * scale,
                )
            };
            for point in [
                (0, 0),
                (frame_w / 3, frame_h / 5),
                (frame_w / 2, frame_h / 2),
                (frame_w - 1, frame_h - 1),
            ] {
                // Desktop capture resizes the native buffer to the frame.
                let seen = (
                    f64::from(point.0) * f64::from(mode_w) / f64::from(frame_w),
                    f64::from(point.1) * f64::from(mode_h) / f64::from(frame_h),
                );
                let (x, y) = landed(point, extent);
                assert!(
                    (x - seen.0).abs() < 0.5 && (y - seen.1).abs() < 0.5,
                    "{mode_w}x{mode_h} @ {scale}: {point:?} landed at ({x}, {y}), saw {seen:?}"
                );
            }

            // The physical wl_output mode is the wrong extent on scaled
            // outputs: the pointer lands at 1 / scale of the target.
            if scale != 1.0 {
                let center = (frame_w / 2, frame_h / 2);
                let seen_x = f64::from(center.0) * f64::from(mode_w) / f64::from(frame_w);
                let (x, _) = landed(center, (mode_w, mode_h));
                assert!((x - seen_x / scale).abs() < 1.0 && (x - seen_x).abs() > 1.0);
            }
        }
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
        // Every output in standby: the display is idle, and the frame keeps it.
        let frame = desktop_frame_from_monitors(vec![off]).unwrap();
        assert_eq!(
            (frame.x, frame.width, frame.outputs.len()),
            (-1920, 1920, 1)
        );
        let mut disabled = monitor_at("A", 0, 0);
        disabled.disabled = true;
        assert!(desktop_frame_from_monitors(vec![disabled]).is_err());
    }

    /// #4161: a laptop panel with a second monitor stacked above it, so the
    /// top output sits at a negative layout offset.
    fn stacked_outputs() -> Vec<DisplayMonitor> {
        vec![monitor_at("eDP-1", 0, 0), monitor_at("HDMI-A-1", 0, -1080)]
    }

    #[test]
    fn desktop_frame_spans_outputs_stacked_at_a_negative_offset() {
        let frame = desktop_frame_from_monitors(stacked_outputs()).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height, frame.scale),
            (0, -1080, 1920, 2160, 1.0)
        );
        assert_eq!(frame.from_layout(0, -1080), (0, 0));
        assert_eq!(frame.from_layout(0, 0), (0, 1080));
        assert_eq!(frame.to_layout(960, 540), (960, -540));
        assert_eq!(
            pointer_layout_from_monitors(stacked_outputs()).unwrap(),
            (0, -1080, 1920, 2160)
        );
    }

    #[test]
    fn desktop_frame_mixes_output_scales_in_logical_pixels() {
        let mut laptop = scaled_monitor(2160, 1350, 1.6666666);
        laptop.name = "eDP-1".into();
        let mut external = monitor_at("DP-1", 1296, 0);
        (external.width, external.height) = (2560, 1440);
        let frame = desktop_frame_from_monitors(vec![laptop.clone(), external.clone()]).unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (0, 0, 3856, 1440)
        );
        assert_eq!(frame.scale, 1.6666666);
        assert_eq!(
            frame
                .outputs
                .iter()
                .map(|o| (o.name.as_str(), o.x, o.width, o.height))
                .collect::<Vec<_>>(),
            [("eDP-1", 0, 1296, 810), ("DP-1", 1296, 2560, 1440)]
        );
        assert_eq!(
            pointer_layout_from_monitors(vec![laptop, external]).unwrap(),
            (0, 0, 3856, 1440)
        );
    }

    #[test]
    fn mirrored_outputs_have_no_area_in_the_frame_or_pointer_layout() {
        let mut mirror = monitor_at("HDMI-A-1", 1920, 0);
        mirror.mirror_of = "eDP-1".into();
        let monitors = vec![monitor_at("eDP-1", 0, 0), mirror];
        let frame = desktop_frame_from_monitors(monitors.clone()).unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.outputs.len()),
            (1920, 1080, 1)
        );
        assert_eq!(
            pointer_layout_from_monitors(monitors).unwrap(),
            (0, 0, 1920, 1080)
        );
    }

    #[test]
    fn screen_report_describes_the_same_snapshot_as_its_frame() {
        let mut off = monitor_at("C", 1920, 0);
        off.dpms_status = false;
        let (frame, report) = screen_report_from_monitors(vec![
            monitor_at("A", -1920, 0),
            monitor_at("B", 0, 0),
            off,
        ])
        .unwrap();
        assert_eq!((frame.x, frame.width, frame.height), (-1920, 3840, 1080));
        let report = report.as_array().unwrap();
        assert_eq!(report.len(), 3);
        assert_eq!(
            (report[0]["frame_x"].as_i64(), report[1]["frame_x"].as_i64()),
            (Some(0), Some(1920))
        );
        assert_eq!(
            (
                report[2]["powered"].as_bool(),
                report[2]["frame_x"].as_i64()
            ),
            (Some(false), None)
        );
        assert!(screen_report_from_monitors(vec![]).is_err());
    }

    #[test]
    fn window_geometry_is_rebased_onto_the_frame_origin() {
        let frame =
            desktop_frame_from_monitors(vec![monitor_at("A", -1920, 0), monitor_at("B", 0, 0)])
                .unwrap();
        let mut windows = vec![crate::x11::WindowInfo {
            xid: 1,
            pid: Some(42),
            app_name: "fixture".into(),
            title: "Fixture".into(),
            is_on_screen: true,
            z_index: None,
            x: -1820,
            y: 36,
            width: 800,
            height: 600,
        }];
        frame.rebase_windows(&mut windows);
        assert_eq!((windows[0].x, windows[0].y), (100, 36));
    }

    #[test]
    fn capture_plan_composes_every_layout_with_more_than_one_monitor() {
        let plan = desktop_capture_plan_from_monitors(vec![monitor_at("A", 0, 0)]).unwrap();
        assert!(!plan.composite);
        // A second monitor, even in standby, means the generic capture (which
        // copies one output) cannot be trusted to be the frame.
        let mut standby = monitor_at("B", 1920, 0);
        standby.dpms_status = false;
        let plan =
            desktop_capture_plan_from_monitors(vec![monitor_at("A", 0, 0), standby]).unwrap();
        assert!(plan.composite);
        assert_eq!((plan.frame.width, plan.frame.outputs.len()), (1920, 1));
    }

    #[test]
    fn pointer_space_falls_back_only_for_a_single_unqualified_output() {
        let mut rotated = monitor_at("B", 0, 0);
        rotated.transform = 1;
        // One rotated output: its logical rectangle (axes swapped) is the layout.
        assert_eq!(
            pointer_space_from_monitors(vec![rotated.clone()]).unwrap(),
            Some((0, 0, 1080, 1920))
        );
        // One flipped output stays unqualified: its own mode is the whole layout.
        let mut flipped = monitor_at("B", 0, 0);
        flipped.transform = 5;
        assert_eq!(pointer_space_from_monitors(vec![flipped]).unwrap(), None);
        // A rotated output in standby beside a powered one: no fallback is right.
        rotated.dpms_status = false;
        let monitors = vec![monitor_at("A", -1920, 0), rotated];
        assert!(desktop_frame_from_monitors(monitors.clone()).is_ok());
        let error = pointer_space_from_monitors(monitors.clone())
            .unwrap_err()
            .to_string();
        assert!(error.contains("B has transform 1, in standby"), "{error}");
        assert!(desktop_snapshot_from_monitors(monitors).is_err());
        // A qualified layout is used as is.
        assert_eq!(
            pointer_space_from_monitors(vec![monitor_at("A", -1920, 0), monitor_at("B", 0, 0)])
                .unwrap(),
            Some((-1920, 0, 3840, 1080))
        );
    }

    #[test]
    fn frame_points_off_every_output_are_detected() {
        // L-shaped layout: a gap at the bottom right of the bounding box.
        let frame =
            desktop_frame_from_monitors(vec![monitor_at("A", 0, 0), monitor_at("B", 1920, -1080)])
                .unwrap();
        assert_eq!(
            (frame.x, frame.y, frame.width, frame.height),
            (0, -1080, 3840, 2160)
        );
        assert!(frame.output_contains_layout(10, 10));
        assert!(frame.output_contains_layout(1920, -1080));
        assert!(frame.output_contains_layout(3839, -1));
        assert!(!frame.output_contains_layout(1920, 0));
        assert!(!frame.output_contains_layout(10, -10));
        assert!(!frame.output_contains_layout(3840, -1));
    }

    #[test]
    fn desktop_frame_allows_rotation_only_for_a_lone_output() {
        let mut rotated = monitor_at("A", 0, 0);
        rotated.transform = 1;
        let frame = desktop_frame_from_monitors(vec![rotated.clone()]).unwrap();
        assert_eq!((frame.width, frame.height), (1080, 1920));
        assert!(desktop_frame_from_monitors(vec![rotated, monitor_at("B", 1080, 0)]).is_err());
        let mut flipped = monitor_at("A", 0, 0);
        flipped.transform = 4;
        assert!(desktop_frame_from_monitors(vec![flipped]).is_err());
    }

    #[test]
    fn pointer_layout_keeps_outputs_in_standby() {
        let mut standby = monitor_at("HDMI-A-1", 0, -1080);
        standby.dpms_status = false;
        let monitors = vec![monitor_at("eDP-1", 0, 0), standby];
        let frame = desktop_frame_from_monitors(monitors.clone()).unwrap();
        assert_eq!((frame.y, frame.height), (0, 1080));
        assert_eq!(
            pointer_layout_from_monitors(monitors).unwrap(),
            (0, -1080, 1920, 2160)
        );
    }

    /// A desktop-frame point must land on the same layout point after the
    /// pointer maps `abs / extent` across its layout box.
    #[test]
    fn frame_points_reach_the_pointer_layout_at_a_negative_offset() {
        let frame = desktop_frame_from_monitors(stacked_outputs()).unwrap();
        let layout = pointer_layout_from_monitors(stacked_outputs()).unwrap();
        let (origin_x, origin_y, extent_w, extent_h) =
            super::super::select_virtual_pointer_space((1920, 1080), Some(layout));
        for point in [(0, 0), (10, 80), (960, 1079), (960, 1080), (1919, 2159)] {
            let (layout_x, layout_y) = frame.to_layout(point.0, point.1);
            let (abs_x, abs_y) = super::super::pointer_abs(
                origin_x, origin_y, extent_w, extent_h, layout_x, layout_y,
            );
            let landed = (
                layout.0
                    + (f64::from(abs_x) / f64::from(extent_w) * f64::from(layout.2)).round() as i32,
                layout.1
                    + (f64::from(abs_y) / f64::from(extent_h) * f64::from(layout.3)).round() as i32,
            );
            assert_eq!(landed, (layout_x, layout_y), "frame point {point:?}");
        }
    }

    #[test]
    fn display_identity_reports_the_logical_frame_of_rotated_outputs() {
        for (transform, expected) in [(1, (1080, 1920)), (2, (1920, 1080)), (3, (1080, 1920))] {
            let mut monitor = display_monitor();
            monitor.transform = transform;
            let (width, height, _) = screen_size_from_monitors(vec![monitor]).unwrap();
            assert_eq!((width, height), expected, "transform {transform}");
        }
    }

    #[test]
    fn composed_frame_places_each_output_at_its_logical_rectangle() {
        let red = image::Rgba([255, 0, 0, 255]);
        let blue = image::Rgba([0, 0, 255, 255]);
        let mut panel = monitor_at("panel", 0, 0);
        (panel.width, panel.height) = (2, 2);
        // A scale-2 output stacked above the panel: its physical 4x4 capture
        // covers a 2x2 logical rectangle and must not spill onto the panel.
        let mut above = scaled_monitor(4, 4, 2.0);
        (above.name, above.y) = ("above".into(), -2);
        let frame = desktop_frame_from_monitors(vec![panel, above]).unwrap();
        let canvas = compose_frame(&frame, |output| {
            Ok(match output.name.as_str() {
                "above" => image::RgbaImage::from_pixel(4, 4, red),
                _ => image::RgbaImage::from_pixel(2, 2, blue),
            })
        })
        .unwrap();
        assert_eq!(canvas.dimensions(), (2, 4));
        for x in 0..2 {
            assert_eq!(*canvas.get_pixel(x, 0), red);
            assert_eq!(*canvas.get_pixel(x, 1), red);
            assert_eq!(*canvas.get_pixel(x, 2), blue);
            assert_eq!(*canvas.get_pixel(x, 3), blue);
        }

        let error = compose_frame(&frame, |_| Ok(image::RgbaImage::new(4, 2))).unwrap_err();
        assert!(
            error.to_string().contains("does not scale uniformly"),
            "{error}"
        );
    }

    #[test]
    fn display_identity_rejects_missing_and_unsupported_frames() {
        assert!(screen_size_from_monitors(vec![]).is_err());
        for scale in [0.0, f64::NAN, f64::INFINITY] {
            let mut monitor = display_monitor();
            monitor.scale = scale;
            assert!(screen_size_from_monitors(vec![monitor]).is_err());
        }
        for transform in [4, 7] {
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
