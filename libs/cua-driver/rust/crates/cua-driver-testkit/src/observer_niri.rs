//! Independent niri observations over niri's IPC socket. This module never
//! calls the driver's niri adapter, capture, geometry, or input code.
//!
//! niri's IPC reports no stacking order and no position for tiled windows, so
//! occlusion is classified only where niri's own rendering rule decides it:
//! floating windows are not drawn while a tiled window in a fullscreen column
//! holds focus on the same workspace (`Workspace::is_floating_visible`). That
//! rule needs a fullscreen column, and IPC cannot tell one from a maximized
//! column of the same size: a client's `xdg_toplevel.set_maximized` or the
//! maximize-to-edges action fills the output while floating windows stay
//! drawn. So the rule certifies occlusion only behind the foreground sentinel,
//! by exact id, while the sentinel itself reports fullscreen from niri's last
//! configure (`ForegroundSentinel` registers it). The rule also needs a static
//! view and focus that does not follow the mouse, which the canonical niri
//! lane's config provides (animations and focus-follows-mouse off). Every
//! other overlap is reported as ambiguous instead of guessed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{DesktopJournal, DesktopSnapshot, FocusEvent, ObserverError, TargetWindow, TargetZ};

const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
const REPLY_LIMIT: u64 = 1024 * 1024;
/// Logical pixels of slack when comparing a fullscreen tile with its output.
const FULLSCREEN_TOLERANCE: f64 = 1.0;
/// How long a snapshot waits for the sentinel's next fullscreen report. The
/// sentinel writes one on each tick of its 100 ms timer, the same timer as
/// its heartbeat, and a snapshot needs just one tick after the window list
/// read. That timer can stall for hundreds of milliseconds on a loaded
/// runner, so the wait is the harness's liveness bound for it; a punctual
/// tick still returns within about 100 ms.
const SENTINEL_REPORT_TIMEOUT: Duration = crate::sentinel::SENTINEL_TICK_LIVENESS;

/// Connects without blocking past `timeout`: a wedged compositor stops
/// accepting, and a full backlog would block a plain connect forever.
fn connect(path: &Path, timeout: Duration) -> std::io::Result<UnixStream> {
    use std::os::unix::ffi::OsStrExt as _;

    let bytes = path.as_os_str().as_bytes();
    // SAFETY: an all-zero sockaddr_un is a valid empty address.
    let mut address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if bytes.len() >= address.sun_path.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "socket path is too long",
        ));
    }
    address.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (slot, byte) in address.sun_path.iter_mut().zip(bytes) {
        *slot = *byte as libc::c_char;
    }
    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: plain socket creation; ownership moves into UnixStream below.
        let fd = unsafe {
            libc::socket(
                libc::AF_UNIX,
                libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: fd is a fresh socket owned only here.
        let stream = unsafe { UnixStream::from_raw_fd(fd) };
        // SAFETY: address is a valid sockaddr_un of the given length.
        let connected = unsafe {
            libc::connect(
                fd,
                (&address as *const libc::sockaddr_un).cast(),
                std::mem::size_of::<libc::sockaddr_un>() as libc::socklen_t,
            )
        };
        if connected == 0 {
            stream.set_nonblocking(false)?;
            return Ok(stream);
        }
        let error = std::io::Error::last_os_error();
        // A Unix stream connect either completes or reports a full backlog.
        if error.raw_os_error() != Some(libc::EAGAIN) || Instant::now() >= deadline {
            return Err(error);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn peer_pid(stream: &UnixStream) -> std::io::Result<i32> {
    // SAFETY: an all-zero ucred is valid output storage.
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the pointer and length describe `credentials`.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(credentials.pid)
}

fn socket_peer(path: &Path) -> std::io::Result<i32> {
    peer_pid(&connect(path, QUERY_TIMEOUT)?)
}

fn niri_socket() -> Option<PathBuf> {
    std::env::var_os("NIRI_SOCKET")
        .filter(|socket| !socket.is_empty())
        .map(PathBuf::from)
}

fn wayland_socket() -> Option<PathBuf> {
    let display = PathBuf::from(std::env::var_os("WAYLAND_DISPLAY")?);
    if display.is_absolute() {
        return Some(display);
    }
    Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(display))
}

/// The niri pid this process recognised first. A later niri, for example a
/// restarted compositor, is a different session and is refused.
static ATTESTED: OnceLock<i32> = OnceLock::new();

/// The pids serving `NIRI_SOCKET` and `WAYLAND_DISPLAY`.
fn peers() -> Result<(i32, i32), String> {
    let niri = niri_socket().ok_or("NIRI_SOCKET is unset")?;
    let wayland = wayland_socket().ok_or("WAYLAND_DISPLAY is unset")?;
    let peer = |path: &Path| {
        socket_peer(path).map_err(|error| format!("{} is unreachable: {error}", path.display()))
    };
    Ok((peer(&niri)?, peer(&wayland)?))
}

/// Before niri is recognised, `Ok(None)` leaves the session to the other
/// observers: `NIRI_SOCKET` is missing, stale or served by an unrelated
/// compositor, as when a nested session inherits it from a niri desktop.
/// `Ok(Some(pid))` recognises the niri serving both sockets. Once niri was
/// recognised, losing either socket or a different peer is an error.
fn attestation(
    recognised: Option<i32>,
    peers: Result<(i32, i32), String>,
) -> Result<Option<i32>, ObserverError> {
    match (recognised, peers) {
        (None, Ok((niri, wayland))) if niri == wayland => Ok(Some(niri)),
        (None, _) => Ok(None),
        (Some(pid), Ok((niri, wayland))) if niri == pid && wayland == pid => Ok(Some(pid)),
        (Some(pid), Ok((niri, wayland))) => Err(ObserverError::new(format!(
            "niri {pid} was recognised, but NIRI_SOCKET is now served by {niri} and \
             WAYLAND_DISPLAY by {wayland}"
        ))),
        (Some(pid), Err(error)) => Err(ObserverError::new(format!(
            "niri {pid} was recognised, but {error}"
        ))),
    }
}

/// Whether `NIRI_SOCKET` belongs to the compositor serving `WAYLAND_DISPLAY`;
/// see [`attestation`].
pub(super) fn attest() -> Result<bool, ObserverError> {
    let Some(pid) = attestation(ATTESTED.get().copied(), peers())? else {
        return Ok(false);
    };
    let attested = *ATTESTED.get_or_init(|| pid);
    if attested != pid {
        return Err(ObserverError::new(format!(
            "niri pid changed from {attested} to {pid}; refusing a new compositor"
        )));
    }
    Ok(true)
}

/// Attests both sockets again. Each snapshot, journal and focus operation
/// does this once at its start; its connections then check only niri's peer,
/// so polling does not open a Wayland client per request.
fn ensure_attested() -> Result<(), ObserverError> {
    if attest()? {
        Ok(())
    } else {
        Err(ObserverError::new(
            "NIRI_SOCKET is not served by the WAYLAND_DISPLAY compositor",
        ))
    }
}

/// A connection to the attested niri, with read and write deadlines. A peer
/// other than the attested niri is refused.
fn attested_connection(purpose: &str) -> Result<UnixStream, ObserverError> {
    let socket =
        niri_socket().ok_or_else(|| ObserverError::new("niri observer requires NIRI_SOCKET"))?;
    let error = |operation: &str, error: std::io::Error| {
        ObserverError::new(format!("niri IPC {operation} {purpose}: {error}"))
    };
    let stream = connect(&socket, QUERY_TIMEOUT).map_err(|e| error("connect", e))?;
    let peer = peer_pid(&stream).map_err(|e| error("read peer of", e))?;
    if Some(&peer) != ATTESTED.get() {
        return Err(ObserverError::new(format!(
            "niri IPC peer {peer} is not the attested niri {:?}",
            ATTESTED.get()
        )));
    }
    stream
        .set_read_timeout(Some(QUERY_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(QUERY_TIMEOUT)))
        .map_err(|e| error("configure", e))?;
    Ok(stream)
}

/// Sends one request on a fresh connection and returns the `Ok` payload.
fn request(request: &serde_json::Value) -> Result<serde_json::Value, ObserverError> {
    let error = |operation: &str, error: std::io::Error| {
        ObserverError::new(format!("niri IPC {operation} {request}: {error}"))
    };
    let mut stream = attested_connection(&request.to_string())?;
    writeln!(stream, "{request}").map_err(|e| error("write", e))?;
    let mut line = String::new();
    BufReader::new(stream.take(REPLY_LIMIT))
        .read_line(&mut line)
        .map_err(|e| error("read", e))?;
    parse_reply(&line)
}

fn parse_reply(line: &str) -> Result<serde_json::Value, ObserverError> {
    let reply: serde_json::Value = serde_json::from_str(line)
        .map_err(|error| ObserverError::new(format!("invalid niri IPC reply: {error}")))?;
    match reply {
        serde_json::Value::Object(mut reply) if reply.len() == 1 => {
            if let Some(payload) = reply.remove("Ok") {
                Ok(payload)
            } else if let Some(message) = reply.remove("Err") {
                Err(ObserverError::new(format!("niri IPC refused: {message}")))
            } else {
                Err(ObserverError::new("niri IPC reply is neither Ok nor Err"))
            }
        }
        _ => Err(ObserverError::new("niri IPC reply is neither Ok nor Err")),
    }
}

/// Reads one response kind, e.g. `{"Ok":{"Windows":[...]}}` for `"Windows"`.
fn query<T: DeserializeOwned>(name: &str) -> Result<T, ObserverError> {
    let mut payload = request(&serde_json::Value::String(name.to_owned()))?;
    let value = payload
        .get_mut(name)
        .map(serde_json::Value::take)
        .ok_or_else(|| ObserverError::new(format!("niri answered {name} with another response")))?;
    serde_json::from_value(value)
        .map_err(|error| ObserverError::new(format!("invalid niri {name} JSON: {error}")))
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Layout {
    tile_size: (f64, f64),
    tile_pos_in_workspace_view: Option<(f64, f64)>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
struct Window {
    id: u64,
    pid: Option<i64>,
    workspace_id: Option<u64>,
    is_focused: bool,
    is_floating: bool,
    layout: Layout,
}

#[derive(Clone, Debug, Deserialize)]
struct Workspace {
    id: u64,
    output: Option<String>,
    is_active: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct Logical {
    width: u32,
    height: u32,
}

#[derive(Clone, Debug, Deserialize)]
struct Output {
    logical: Option<Logical>,
}

#[derive(Clone, Debug, Deserialize)]
struct Overview {
    is_open: bool,
}

struct Desktop {
    windows: Vec<Window>,
    workspaces: Vec<Workspace>,
    outputs: HashMap<String, Output>,
    overview_open: bool,
    sentinel: Option<Sentinel>,
    /// When the window list was first read; the sentinel's fullscreen report
    /// must be at least this recent.
    started: Duration,
}

/// The foreground sentinel's window and its fullscreen state file, which the
/// sentinel replaces on every tick and setup never resets.
#[derive(Clone, Debug)]
struct Sentinel {
    window: TargetWindow,
    state: PathBuf,
}

static SENTINEL: Mutex<Option<Sentinel>> = Mutex::new(None);

/// Registers the foreground sentinel whose fullscreen reports may certify
/// occlusion, or clears it with `None`.
pub(super) fn register_sentinel(sentinel: Option<(TargetWindow, PathBuf)>) {
    *SENTINEL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) =
        sentinel.map(|(window, state)| Sentinel { window, state });
}

/// Milliseconds since the epoch, the sentinel's report clock.
fn report_clock() -> Duration {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    Duration::from_millis(now.as_millis() as u64)
}

fn registered_sentinel() -> Option<Sentinel> {
    SENTINEL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// The fullscreen state the sentinel wrote to `state` at or after `since`,
/// waiting up to `timeout` for that report. An older report says nothing
/// about the window list read since.
fn fullscreen_since(state: &Path, since: Duration, timeout: Duration) -> Option<bool> {
    let deadline = Instant::now() + timeout;
    loop {
        let report = std::fs::read_to_string(state)
            .ok()
            .and_then(|state| fullscreen_report(&state));
        if let Some((_, fullscreen)) = report.filter(|(at, _)| *at >= since) {
            return Some(fullscreen);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `{"at_ms":…,"fullscreen":…}` as the sentinel writes it.
fn fullscreen_report(state: &str) -> Option<(Duration, bool)> {
    let state: serde_json::Value = serde_json::from_str(state).ok()?;
    Some((
        Duration::from_millis(state["at_ms"].as_u64()?),
        state["fullscreen"].as_bool()?,
    ))
}

/// One desktop state from four requests. Each request is its own connection,
/// so the windows are read again last and the read is retried when they
/// changed in between, for example across a workspace switch.
fn desktop() -> Result<Desktop, ObserverError> {
    ensure_attested()?;
    for _ in 0..3 {
        let started = report_clock();
        let windows: Vec<Window> = query("Windows")?;
        let workspaces = query("Workspaces")?;
        let outputs = query("Outputs")?;
        let overview_open = query::<Overview>("OverviewState")?.is_open;
        if query::<Vec<Window>>("Windows")? == windows {
            return Ok(Desktop {
                windows,
                workspaces,
                outputs,
                overview_open,
                sentinel: registered_sentinel(),
                started,
            });
        }
    }
    Err(ObserverError::new(
        "niri windows kept changing while the desktop was read",
    ))
}

fn focused(windows: &[Window]) -> Result<Option<&Window>, ObserverError> {
    let mut focused = windows.iter().filter(|window| window.is_focused);
    let first = focused.next();
    if focused.next().is_some() {
        return Err(ObserverError::new(
            "niri reports more than one focused window",
        ));
    }
    Ok(first)
}

/// The target by exact niri id, or by its pid's only window when no id is known.
fn select(windows: &[Window], target: TargetWindow) -> Result<Option<&Window>, ObserverError> {
    let pid = Some(i64::from(target.pid));
    if target.native_id != 0 {
        let window = windows.iter().find(|window| window.id == target.native_id);
        if window.is_some_and(|window| window.pid != pid) {
            return Err(ObserverError::new(
                "niri target window id belongs to a different pid",
            ));
        }
        return Ok(window);
    }
    let mut matches = windows.iter().filter(|window| window.pid == pid);
    let first = matches.next();
    if matches.next().is_some() {
        return Err(ObserverError::new(
            "niri target pid has multiple windows; native_id required",
        ));
    }
    Ok(first)
}

fn covers_output(window: &Window, output: &Logical) -> bool {
    let (width, height) = window.layout.tile_size;
    width + FULLSCREEN_TOLERANCE >= f64::from(output.width)
        && height + FULLSCREEN_TOLERANCE >= f64::from(output.height)
}

fn floating_overlap(a: &Window, b: &Window) -> Option<bool> {
    let (ax, ay) = a.layout.tile_pos_in_workspace_view?;
    let (bx, by) = b.layout.tile_pos_in_workspace_view?;
    let (aw, ah) = a.layout.tile_size;
    let (bw, bh) = b.layout.tile_size;
    Some(ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah)
}

/// Whether the focused output-sized `tile` hides floating windows: only when
/// it is the sentinel and the sentinel reports fullscreen after the window
/// list was read. Only this case waits for the report.
fn sentinel_occludes(desktop: &Desktop, tile: &Window) -> Result<TargetZ, ObserverError> {
    let Some(sentinel) = desktop.sentinel.as_ref().filter(|sentinel| {
        sentinel.window.native_id == tile.id && tile.pid == Some(i64::from(sentinel.window.pid))
    }) else {
        return Err(ObserverError::new(
            "niri focused output-sized tile is not the foreground sentinel; \
             fullscreen and occlusion are unproven",
        ));
    };
    match fullscreen_since(&sentinel.state, desktop.started, SENTINEL_REPORT_TIMEOUT) {
        Some(true) => Ok(TargetZ::BackgroundOccluded),
        Some(false) => Err(ObserverError::new(
            "niri sentinel reports it is not fullscreen; a maximized tile does not hide \
             floating windows",
        )),
        None => Err(ObserverError::new(format!(
            "niri sentinel wrote no fullscreen report since the window list was read, \
             after waiting {SENTINEL_REPORT_TIMEOUT:?}: its timer is stalled"
        ))),
    }
}

fn classify(desktop: &Desktop, target: TargetWindow) -> Result<TargetZ, ObserverError> {
    let Some(target) = select(&desktop.windows, target)? else {
        return Ok(TargetZ::NotFound);
    };
    if target.is_focused {
        return Ok(TargetZ::Foreground);
    }
    let workspace = target.workspace_id.and_then(|id| {
        desktop
            .workspaces
            .iter()
            .find(|workspace| workspace.id == id)
    });
    let Some(workspace) = workspace.filter(|workspace| workspace.is_active) else {
        // niri has no minimize; a window off the active workspace is not shown.
        return Ok(TargetZ::Minimized);
    };
    if desktop.overview_open {
        return Err(ObserverError::new(
            "niri overview is open; z-order is ambiguous",
        ));
    }
    let output = workspace
        .output
        .as_ref()
        .and_then(|name| desktop.outputs.get(name))
        .and_then(|output| output.logical.as_ref())
        .ok_or_else(|| ObserverError::new("niri target workspace has no logical output"))?;
    if !target.is_floating {
        return Err(ObserverError::new(
            "niri IPC reports no position for tiled windows; z-order is ambiguous",
        ));
    }
    let focused = focused(&desktop.windows)?;
    if let Some(tile) = focused.filter(|focused| {
        !focused.is_floating
            && focused.workspace_id == Some(workspace.id)
            && covers_output(focused, output)
    }) {
        return sentinel_occludes(desktop, tile);
    }
    for other in &desktop.windows {
        if other.id == target.id || !other.is_floating || other.workspace_id != target.workspace_id
        {
            continue;
        }
        // IPC has no floating stacking order, so any overlap is ambiguous.
        if floating_overlap(target, other) != Some(false) {
            return Err(ObserverError::new(
                "niri overlapping floating windows have ambiguous z-order",
            ));
        }
    }
    Ok(TargetZ::BackgroundVisible)
}

pub(super) fn probe() -> Result<(), ObserverError> {
    let version: serde_json::Value = query("Version")?;
    if !version.is_string() {
        return Err(ObserverError::new(
            "niri answered Version without a version",
        ));
    }
    desktop().map(|_| ())
}

pub(super) fn snapshot(target: TargetWindow) -> Result<DesktopSnapshot, ObserverError> {
    let desktop = desktop()?;
    let focus = focused(&desktop.windows)?.map(|window| window.id);
    Ok(DesktopSnapshot {
        foreground: focus,
        input_focus: focus,
        target_z: classify(&desktop, target)?,
        cursor_pos: None,
    })
}

pub(super) fn focus_identity() -> Result<Option<u64>, ObserverError> {
    ensure_attested()?;
    Ok(focused(&query::<Vec<Window>>("Windows")?)?.map(|window| window.id))
}

#[derive(Deserialize)]
struct WindowFocus {
    id: u64,
    is_focused: bool,
}

/// Which windows niri reports focused, updated from its event stream the way
/// niri-ipc's `EventStreamState` applies window events.
#[derive(Default)]
struct FocusState {
    windows: Option<HashMap<u64, bool>>,
}

impl FocusState {
    /// Applies one event line. Events before the first `WindowsChanged` and
    /// events about other state are ignored.
    fn apply(&mut self, line: &[u8]) -> Result<(), ObserverError> {
        let event: serde_json::Value = serde_json::from_slice(line)
            .map_err(|error| ObserverError::new(format!("invalid niri event: {error}")))?;
        let invalid =
            |error: serde_json::Error| ObserverError::new(format!("invalid niri event: {error}"));
        if let Some(change) = event.get("WindowsChanged") {
            let windows: Vec<WindowFocus> =
                serde_json::from_value(change["windows"].clone()).map_err(invalid)?;
            self.windows = Some(
                windows
                    .into_iter()
                    .map(|window| (window.id, window.is_focused))
                    .collect(),
            );
            return Ok(());
        }
        let Some(windows) = self.windows.as_mut() else {
            return Ok(());
        };
        if let Some(change) = event.get("WindowOpenedOrChanged") {
            let window: WindowFocus =
                serde_json::from_value(change["window"].clone()).map_err(invalid)?;
            if window.is_focused {
                windows.values_mut().for_each(|focused| *focused = false);
            }
            windows.insert(window.id, window.is_focused);
        } else if let Some(change) = event.get("WindowClosed") {
            let id: u64 = serde_json::from_value(change["id"].clone()).map_err(invalid)?;
            windows.remove(&id);
        } else if let Some(change) = event.get("WindowFocusChanged") {
            let id: Option<u64> = serde_json::from_value(change["id"].clone()).map_err(invalid)?;
            for (window, focused) in windows.iter_mut() {
                *focused = Some(*window) == id;
            }
        }
        Ok(())
    }

    /// The focused window, once the window list is known.
    fn focus(&self) -> Result<Option<Option<u64>>, ObserverError> {
        let Some(windows) = &self.windows else {
            return Ok(None);
        };
        let mut focused = windows.iter().filter(|(_, focused)| **focused);
        let first = focused.next().map(|(id, _)| *id);
        if focused.next().is_some() {
            return Err(ObserverError::new(
                "niri reports more than one focused window",
            ));
        }
        Ok(Some(first))
    }
}

const JOURNAL_EVENT_LIMIT: usize = 10_000;

/// Records every focus change niri reports through its event stream, so a
/// focus that leaves and returns between two polls is still seen.
pub(super) struct Journal {
    stop: Arc<AtomicBool>,
    sampler: Option<JoinHandle<Result<DesktopJournal, ObserverError>>>,
}

impl Journal {
    pub(super) fn start() -> Result<Self, ObserverError> {
        ensure_attested()?;
        let mut stream = attested_connection("EventStream")?;
        writeln!(stream, "\"EventStream\"")
            .map_err(|error| ObserverError::new(format!("niri EventStream request: {error}")))?;
        Self::with_stream(stream)
    }

    /// Ready barrier: the subscription is acknowledged and niri has sent its
    /// current window list before the observed action may start.
    fn with_stream(stream: UnixStream) -> Result<Self, ObserverError> {
        let mut reader = BufReader::new(stream);
        let mut line = Vec::new();
        let deadline = Instant::now() + QUERY_TIMEOUT;
        let read_line = |reader: &mut BufReader<UnixStream>, line: &mut Vec<u8>| {
            line.clear();
            match reader.read_until(b'\n', line) {
                Ok(0) => Err(ObserverError::new("niri event stream closed")),
                Ok(_) => Ok(()),
                Err(error) => Err(ObserverError::new(format!("niri event stream: {error}"))),
            }
        };
        read_line(&mut reader, &mut line)?;
        let reply = parse_reply(&String::from_utf8_lossy(&line))?;
        if reply != serde_json::json!("Handled") {
            return Err(ObserverError::new(format!(
                "niri answered EventStream with {reply}"
            )));
        }
        let mut state = FocusState::default();
        let mut focus = loop {
            if Instant::now() >= deadline {
                return Err(ObserverError::new("niri event stream sent no window list"));
            }
            read_line(&mut reader, &mut line)?;
            state.apply(&line)?;
            if let Some(focus) = state.focus()? {
                break focus;
            }
        };
        reader
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(25)))
            .map_err(|error| ObserverError::new(format!("niri event stream: {error}")))?;
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let sampler = std::thread::spawn(move || {
            let mut journal = DesktopJournal::default();
            let mut line = Vec::new();
            // After a stop, keep reading until the socket is quiet: niri may
            // still be writing events it queued during the action.
            let mut quiet_deadline = None;
            loop {
                if quiet_deadline.is_none() && thread_stop.load(Ordering::Acquire) {
                    quiet_deadline = Some(Instant::now() + QUERY_TIMEOUT);
                }
                // A timed-out read keeps its partial line for the next read.
                match reader.read_until(b'\n', &mut line) {
                    Ok(0) => return Err(ObserverError::new("niri event stream closed")),
                    Ok(_) if line.ends_with(b"\n") => {
                        state.apply(&line)?;
                        if let Some(current) = state.focus()? {
                            if current != focus {
                                if journal.focus_events.len() >= JOURNAL_EVENT_LIMIT {
                                    return Err(ObserverError::new(
                                        "niri focus journal exceeded event limit",
                                    ));
                                }
                                journal.focus_events.push(FocusEvent {
                                    from: focus,
                                    to: current,
                                });
                                focus = current;
                            }
                        }
                        line.clear();
                    }
                    Ok(_) => {}
                    Err(error)
                        if matches!(
                            error.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        if quiet_deadline.is_some() && line.is_empty() {
                            return Ok(journal);
                        }
                    }
                    Err(error) => {
                        return Err(ObserverError::new(format!("niri event stream: {error}")))
                    }
                }
                if quiet_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(ObserverError::new(format!(
                        "niri event stream did not go quiet within {QUERY_TIMEOUT:?}"
                    )));
                }
            }
        });
        Ok(Self {
            stop,
            sampler: Some(sampler),
        })
    }

    pub(super) fn drain(mut self) -> Result<DesktopJournal, ObserverError> {
        self.stop.store(true, Ordering::Release);
        self.sampler
            .take()
            .expect("active journal")
            .join()
            .map_err(|_| ObserverError::new("niri focus journal panicked"))?
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(sampler) = self.sampler.take() {
            let _ = sampler.join();
        }
    }
}

/// Whether focus left `from` for `to`, both resolved by exact identity. This
/// does not use z-order: the tiled sentinel has no IPC position, so its own
/// z-order is ambiguous once a floating window holds focus.
fn focus_transferred(
    windows: &[Window],
    from: TargetWindow,
    to: TargetWindow,
) -> Result<bool, ObserverError> {
    let resolve = |target| {
        select(windows, target)?
            .map(|window| window.id)
            .ok_or_else(|| ObserverError::new("niri focus-transfer window not found"))
    };
    let (from, to) = (resolve(from)?, resolve(to)?);
    if from == to {
        return Err(ObserverError::new(
            "niri focus transfer requires two distinct windows",
        ));
    }
    Ok(focused(windows)?.map(|window| window.id) == Some(to))
}

pub(super) fn wait_for_focus_transfer(
    from: TargetWindow,
    to: TargetWindow,
    timeout: Duration,
) -> Result<(), ObserverError> {
    ensure_attested()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let windows: Vec<Window> = query("Windows")?;
        if focus_transferred(&windows, from, to)? {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(ObserverError::new(format!(
                "niri focus did not move from window {} to window {} within {timeout:?}; focused: {:?}",
                from.native_id,
                to.native_id,
                focused(&windows)?.map(|window| window.id)
            )));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Focuses the target through niri's own action, for test setup only.
pub(super) fn focus(target: TargetWindow) -> Result<(), ObserverError> {
    ensure_attested()?;
    let id = select(&query::<Vec<Window>>("Windows")?, target)?
        .ok_or_else(|| ObserverError::new("niri target window not found"))?
        .id;
    let reply = request(&serde_json::json!({ "Action": { "FocusWindow": { "id": id } } }))?;
    if reply == serde_json::json!("Handled") {
        Ok(())
    } else {
        Err(ObserverError::new(format!(
            "niri answered FocusWindow with {reply}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn window(id: u64, pid: i64, floating: bool, focused: bool) -> serde_json::Value {
        json!({
            "id": id, "title": format!("window {id}"), "app_id": "fixture", "pid": pid,
            "workspace_id": 1, "is_focused": focused, "is_floating": floating,
            "is_urgent": false, "focus_timestamp": null,
            "layout": {
                "pos_in_scrolling_layout": if floating { json!(null) } else { json!([1, 1]) },
                "tile_size": [940.0, 780.0], "window_size": [940, 780],
                "tile_pos_in_workspace_view": if floating { json!([0.0, 0.0]) } else { json!(null) },
                "window_offset_in_tile": [0.0, 0.0],
            },
        })
    }

    fn fullscreen(mut window: serde_json::Value) -> serde_json::Value {
        window["layout"]["tile_size"] = json!([1920.0, 1080.0]);
        window["layout"]["window_size"] = json!([1920, 1080]);
        window
    }

    fn desktop(windows: Vec<serde_json::Value>) -> Desktop {
        Desktop {
            windows: serde_json::from_value(json!(windows)).unwrap(),
            workspaces: serde_json::from_value(json!([
                {"id": 1, "idx": 1, "name": null, "output": "winit", "is_urgent": false,
                 "is_active": true, "is_focused": true, "active_window_id": 10},
                {"id": 2, "idx": 2, "name": null, "output": "winit", "is_urgent": false,
                 "is_active": false, "is_focused": false, "active_window_id": null},
            ]))
            .unwrap(),
            outputs: serde_json::from_value(json!({
                "winit": {"name": "winit", "make": "Smithay", "model": "Winit",
                    "serial": null, "physical_size": null, "modes": [], "current_mode": null,
                    "is_custom_mode": true, "vrr_supported": false, "vrr_enabled": false,
                    "logical": {"x": 0, "y": 0, "width": 1920, "height": 1080, "scale": 1.0,
                        "transform": "Flipped180"}}
            }))
            .unwrap(),
            overview_open: false,
            sentinel: Some(Sentinel {
                window: target(10, 100),
                state: PathBuf::from("/nonexistent/sentinel-state.json"),
            }),
            started: Duration::ZERO,
        }
    }

    /// Points the desktop's sentinel at a state file holding `report`.
    fn report(desktop: &mut Desktop, report: &str) -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("sentinel-state.json");
        std::fs::write(&state, report).unwrap();
        desktop.sentinel.as_mut().unwrap().state = state;
        directory
    }

    fn target(native_id: u64, pid: u32) -> TargetWindow {
        TargetWindow { pid, native_id }
    }

    #[test]
    fn floating_target_behind_focused_fullscreen_sentinel_is_occluded() {
        let mut desktop = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        let _state = report(&mut desktop, r#"{"at_ms":1,"fullscreen":true}"#);
        assert_eq!(
            classify(&desktop, target(20, 200)).unwrap(),
            TargetZ::BackgroundOccluded
        );
        assert_eq!(
            classify(&desktop, target(10, 100)).unwrap(),
            TargetZ::Foreground
        );
    }

    #[test]
    fn client_maximized_sentinel_does_not_certify_occlusion() {
        // A client maximize fills the output like fullscreen, but floating
        // windows stay drawn above it. Only the sentinel's report differs.
        let mut desktop = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        let _state = report(&mut desktop, r#"{"at_ms":1,"fullscreen":false}"#);
        assert!(classify(&desktop, target(20, 200))
            .unwrap_err()
            .to_string()
            .contains("not fullscreen"));
        let _state = report(&mut desktop, r#"{"at_ms":1}"#);
        assert!(classify(&desktop, target(20, 200))
            .unwrap_err()
            .to_string()
            .contains("no fullscreen report"));
    }

    #[test]
    fn output_sized_tile_other_than_the_sentinel_is_ambiguous() {
        let mut desktop = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        for sentinel in [None, Some(target(11, 100)), Some(target(10, 101))] {
            desktop.sentinel = sentinel.map(|window| Sentinel {
                window,
                state: PathBuf::from("/nonexistent/sentinel-state.json"),
            });
            assert!(classify(&desktop, target(20, 200))
                .unwrap_err()
                .to_string()
                .contains("not the foreground sentinel"));
        }
    }

    #[test]
    fn sentinel_fullscreen_report_survives_a_journal_reset_before_the_posture_snapshot() {
        // ForegroundSentinel waits for a heartbeat, resets its event journal
        // and immediately checks the background posture.
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("sentinel-events.jsonl");
        let state = directory.path().join("sentinel-state.json");
        let since = report_clock();
        let now = since.as_millis();
        std::fs::write(
            &journal,
            format!("{{\"kind\":\"heartbeat\",\"at_ms\":{now}}}\n"),
        )
        .unwrap();
        std::fs::write(&state, format!(r#"{{"at_ms":{now},"fullscreen":true}}"#)).unwrap();
        register_sentinel(Some((target(10, 100), state)));
        std::fs::write(&journal, "").unwrap();
        let mut posture = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        posture.sentinel = registered_sentinel();
        posture.started = since;
        register_sentinel(None);
        assert_eq!(
            classify(&posture, target(20, 200)).unwrap(),
            TargetZ::BackgroundOccluded
        );
    }

    #[test]
    fn focused_sentinel_snapshot_does_not_wait_for_its_fullscreen_report() {
        // Most snapshots never consult the report, for example while waiting
        // for the sentinel itself to hold focus. Here no report ever arrives.
        let desktop = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        let started = Instant::now();
        assert_eq!(
            classify(&desktop, target(10, 100)).unwrap(),
            TargetZ::Foreground
        );
        assert!(started.elapsed() < SENTINEL_REPORT_TIMEOUT);
    }

    #[test]
    fn sentinel_fullscreen_report_must_follow_the_window_list_read() {
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("sentinel-state.json");
        let since = Duration::from_millis(10_000);
        let timeout = Duration::from_millis(30);
        // A report from before the read may predate an unfullscreen.
        std::fs::write(&state, r#"{"at_ms":9999,"fullscreen":true}"#).unwrap();
        assert_eq!(fullscreen_since(&state, since, timeout), None);
        std::fs::write(&state, r#"{"at_ms":10000,"fullscreen":false}"#).unwrap();
        assert_eq!(fullscreen_since(&state, since, timeout), Some(false));
        std::fs::write(&state, r#"{"at_ms":10000}"#).unwrap();
        assert_eq!(fullscreen_since(&state, since, timeout), None);

        // The snapshot waits for the next report.
        std::fs::write(&state, r#"{"at_ms":9999,"fullscreen":false}"#).unwrap();
        let writer = {
            let state = state.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(20));
                std::fs::write(state, r#"{"at_ms":10100,"fullscreen":true}"#).unwrap();
            })
        };
        assert_eq!(
            fullscreen_since(&state, since, SENTINEL_REPORT_TIMEOUT),
            Some(true)
        );
        writer.join().unwrap();
    }

    #[test]
    fn a_sentinel_tick_that_runs_late_still_reports() {
        // On a loaded runner the sentinel's timer skipped three ticks, which
        // its heartbeat liveness bound allows.
        let directory = tempfile::tempdir().unwrap();
        let state = directory.path().join("sentinel-state.json");
        let since = Duration::from_millis(10_000);
        std::fs::write(&state, r#"{"at_ms":9999,"fullscreen":true}"#).unwrap();
        let writer = {
            let state = state.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(400));
                std::fs::write(state, r#"{"at_ms":10400,"fullscreen":true}"#).unwrap();
            })
        };
        assert_eq!(
            fullscreen_since(&state, since, SENTINEL_REPORT_TIMEOUT),
            Some(true)
        );
        writer.join().unwrap();
    }

    #[test]
    fn floating_target_above_a_non_fullscreen_tile_is_visible() {
        let desktop = desktop(vec![
            window(10, 100, false, true),
            window(20, 200, true, false),
        ]);
        assert_eq!(
            classify(&desktop, target(20, 200)).unwrap(),
            TargetZ::BackgroundVisible
        );
    }

    #[test]
    fn floating_overlap_tiled_target_and_open_overview_are_ambiguous() {
        let overlapping = desktop(vec![
            window(10, 100, true, true),
            window(20, 200, true, false),
        ]);
        assert!(classify(&overlapping, target(20, 200))
            .unwrap_err()
            .to_string()
            .contains("overlapping floating windows"));

        let tiled = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, false, false),
        ]);
        assert!(classify(&tiled, target(20, 200))
            .unwrap_err()
            .to_string()
            .contains("no position for tiled windows"));

        let mut overview = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        overview.overview_open = true;
        assert!(classify(&overview, target(20, 200))
            .unwrap_err()
            .to_string()
            .contains("overview is open"));
    }

    #[test]
    fn window_on_an_inactive_workspace_is_not_shown() {
        let mut hidden = window(20, 200, true, false);
        hidden["workspace_id"] = json!(2);
        let desktop = desktop(vec![fullscreen(window(10, 100, false, true)), hidden]);
        assert_eq!(
            classify(&desktop, target(20, 200)).unwrap(),
            TargetZ::Minimized
        );
    }

    #[test]
    fn identity_is_the_exact_niri_id_and_never_a_pid_sibling() {
        let desktop = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
            window(21, 200, true, false),
        ]);
        assert_eq!(
            classify(&desktop, target(30, 200)).unwrap(),
            TargetZ::NotFound
        );
        assert!(classify(&desktop, target(20, 300))
            .unwrap_err()
            .to_string()
            .contains("different pid"));
        assert!(classify(&desktop, target(0, 200))
            .unwrap_err()
            .to_string()
            .contains("native_id required"));
    }

    #[test]
    fn focus_canary_sees_the_floating_target_take_focus_from_the_sentinel() {
        let sentinel = target(10, 100);
        let harness = target(20, 200);
        let before = desktop(vec![
            fullscreen(window(10, 100, false, true)),
            window(20, 200, true, false),
        ]);
        assert!(!focus_transferred(&before.windows, sentinel, harness).unwrap());

        let after = desktop(vec![
            fullscreen(window(10, 100, false, false)),
            window(20, 200, true, true),
        ]);
        assert!(focus_transferred(&after.windows, sentinel, harness).unwrap());
        // The unfocused tiled sentinel has no IPC position, so z-order cannot
        // decide this canary.
        assert!(classify(&after, sentinel).is_err());
        assert!(focus_transferred(&after.windows, sentinel, sentinel).is_err());
    }

    fn windows_changed(focused_id: u64) -> String {
        json!({"WindowsChanged": {"windows": [
            fullscreen(window(10, 100, false, focused_id == 10)),
            window(20, 200, true, focused_id == 20),
        ]}})
        .to_string()
    }

    #[test]
    fn journal_records_a_focus_that_leaves_and_returns() {
        let (stream, mut niri) = UnixStream::pair().unwrap();
        writeln!(niri, r#"{{"Ok":"Handled"}}"#).unwrap();
        writeln!(niri, r#"{{"WorkspacesChanged":{{"workspaces":[]}}}}"#).unwrap();
        writeln!(niri, "{}", windows_changed(10)).unwrap();
        let journal = Journal::with_stream(stream).unwrap();
        // The second focus event arrives in two writes to exercise a read
        // that times out mid-line.
        writeln!(niri, r#"{{"WindowFocusChanged":{{"id":20}}}}"#).unwrap();
        write!(niri, r#"{{"WindowFocusChanged":"#).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        writeln!(niri, r#"{{"id":10}}}}"#).unwrap();
        let journal = journal.drain().unwrap();
        assert_eq!(
            journal.focus_events,
            vec![
                FocusEvent {
                    from: Some(10),
                    to: Some(20)
                },
                FocusEvent {
                    from: Some(20),
                    to: Some(10)
                },
            ]
        );
    }

    #[test]
    fn journal_records_focus_taken_by_an_opened_or_changed_window() {
        // niri reports focus on a window it opens, or whose title or
        // workspace changes in the same refresh, only inside
        // WindowOpenedOrChanged; no WindowFocusChanged follows.
        let (stream, mut niri) = UnixStream::pair().unwrap();
        writeln!(niri, r#"{{"Ok":"Handled"}}"#).unwrap();
        writeln!(niri, "{}", windows_changed(10)).unwrap();
        let journal = Journal::with_stream(stream).unwrap();
        let mut dialog = window(30, 200, true, true);
        dialog["title"] = json!("dialog");
        writeln!(
            niri,
            "{}",
            json!({"WindowOpenedOrChanged": {"window": dialog}})
        )
        .unwrap();
        writeln!(niri, r#"{{"WindowClosed":{{"id":30}}}}"#).unwrap();
        writeln!(niri, r#"{{"WindowFocusChanged":{{"id":10}}}}"#).unwrap();
        let mut renamed = window(20, 200, true, true);
        renamed["title"] = json!("renamed");
        writeln!(
            niri,
            "{}",
            json!({"WindowOpenedOrChanged": {"window": renamed}})
        )
        .unwrap();
        writeln!(niri, r#"{{"WindowFocusChanged":{{"id":10}}}}"#).unwrap();
        let events = journal.drain().unwrap().focus_events;
        let transitions = events
            .iter()
            .map(|event| (event.from, event.to))
            .collect::<Vec<_>>();
        assert_eq!(
            transitions,
            vec![
                (Some(10), Some(30)),
                (Some(30), None),
                (None, Some(10)),
                (Some(10), Some(20)),
                (Some(20), Some(10)),
            ]
        );
    }

    #[test]
    fn journal_waits_for_the_window_list_and_reports_a_closed_stream() {
        let (stream, mut niri) = UnixStream::pair().unwrap();
        writeln!(niri, r#"{{"Ok":"Handled"}}"#).unwrap();
        drop(niri);
        assert!(Journal::with_stream(stream)
            .err()
            .unwrap()
            .to_string()
            .contains("closed"));

        let (stream, mut niri) = UnixStream::pair().unwrap();
        writeln!(niri, r#"{{"Ok":"Handled"}}"#).unwrap();
        writeln!(niri, "{}", windows_changed(10)).unwrap();
        let journal = Journal::with_stream(stream).unwrap();
        drop(niri);
        std::thread::sleep(Duration::from_millis(60));
        assert!(journal.drain().unwrap_err().to_string().contains("closed"));
    }

    #[test]
    fn socket_peer_is_the_listening_process() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("niri.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(socket_peer(&path).unwrap() as u32, std::process::id());
        assert!(socket_peer(&directory.path().join("missing.sock")).is_err());
    }

    #[test]
    fn attestation_falls_through_until_niri_is_recognised_then_fails_closed() {
        let lost = || Err("NIRI_SOCKET is unreachable".to_owned());
        assert_eq!(attestation(None, Ok((7, 7))).unwrap(), Some(7));
        assert_eq!(attestation(None, Ok((7, 8))).unwrap(), None);
        assert_eq!(attestation(None, lost()).unwrap(), None);
        assert_eq!(attestation(Some(7), Ok((7, 7))).unwrap(), Some(7));
        assert!(attestation(Some(7), lost())
            .unwrap_err()
            .to_string()
            .contains("unreachable"));
        for peers in [(8, 8), (7, 8), (8, 7)] {
            assert!(attestation(Some(7), Ok(peers))
                .unwrap_err()
                .to_string()
                .contains("now served by"));
        }
    }

    #[test]
    fn replies_unwrap_ok_and_preserve_err() {
        assert_eq!(
            parse_reply(r#"{"Ok":{"Version":"26.04"}}"#).unwrap(),
            json!({"Version": "26.04"})
        );
        assert!(parse_reply(r#"{"Err":"no such window"}"#)
            .unwrap_err()
            .to_string()
            .contains("no such window"));
        assert!(parse_reply("").is_err());
    }
}
