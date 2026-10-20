//! The niri IPC transport: socket attestation, requests and the serde subset of
//! niri's replies.

use super::*;

pub(super) const QUERY_TIMEOUT: Duration = Duration::from_secs(1);
pub(super) const QUERY_TOTAL_TIMEOUT: Duration = Duration::from_secs(3);
pub(super) const QUERY_RETRY_BACKOFF: Duration = Duration::from_millis(50);
const QUERY_MAX_ATTEMPTS: usize = 2;
const MAX_REPLY_BYTES: usize = 4 * 1024 * 1024;
const MAX_LOGICAL_PIXELS: u64 = 64 * 1024 * 1024;

// ── Session ──────────────────────────────────────────────────────────────────

/// The compositor process recognised as niri for this driver process.
pub(super) static RECOGNIZED: OnceLock<libc::pid_t> = OnceLock::new();
/// Why the latest probe before recognition could not tell whether
/// `NIRI_SOCKET` is this session's niri; routes refuse with it meanwhile.
static UNKNOWN: Mutex<Option<String>> = Mutex::new(None);
/// Whether the adapter supports the recognised niri's release: `None` until
/// niri answers a `Version` request, and every route refuses meanwhile.
static RELEASE: Mutex<Option<Result<(), String>>> = Mutex::new(None);
/// The oldest niri release the adapter supports: 25.08 reports the window
/// layout that geometry needs, and serves several requests per connection.
const MIN_RELEASE: (u32, u32) = (25, 8);

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Attestation {
    /// `NIRI_SOCKET` belongs to another session: not niri for this process.
    Unrelated(String),
    Attested(libc::pid_t),
    /// niri was recognised, but this connection cannot be proven to reach it.
    Refused(String),
}

/// Session identity is tri-state. Before recognition, IPC and Wayland peers
/// that differ (an inherited socket of another desktop) mean an unrelated
/// session. After recognition, any loss, mismatch or new compositor process
/// refuses rather than falling through to generic discovery.
fn attest(
    pinned: Option<libc::pid_t>,
    ipc_peer: Result<libc::pid_t>,
    wayland_peer: Result<libc::pid_t>,
) -> Attestation {
    let (ipc_peer, wayland_peer) = match (ipc_peer, wayland_peer) {
        (Ok(ipc), Ok(wayland)) => (ipc, wayland),
        (ipc, wayland) => {
            let reason = format!(
                "niri IPC or WAYLAND_DISPLAY peer is unavailable (IPC: {}, Wayland: {})",
                ipc.map_or_else(|e| format!("{e:#}"), |pid| format!("pid {pid}")),
                wayland.map_or_else(|e| format!("{e:#}"), |pid| format!("pid {pid}")),
            );
            return match pinned {
                None => Attestation::Unrelated(reason),
                Some(_) => Attestation::Refused(reason),
            };
        }
    };
    if ipc_peer != wayland_peer {
        let reason = format!(
            "NIRI_SOCKET is served by pid {ipc_peer} but WAYLAND_DISPLAY by pid {wayland_peer}"
        );
        return match pinned {
            None => Attestation::Unrelated(reason),
            Some(_) => Attestation::Refused(reason),
        };
    }
    match pinned {
        Some(pinned) if pinned != ipc_peer => Attestation::Refused(format!(
            "niri was recognised as pid {pinned}, but the compositor is now pid {ipc_peer}"
        )),
        _ => Attestation::Attested(ipc_peer),
    }
}

fn socket_path() -> Result<PathBuf> {
    let path = PathBuf::from(std::env::var_os("NIRI_SOCKET").context("missing NIRI_SOCKET")?);
    if !path.is_absolute() {
        bail!("NIRI_SOCKET must be an absolute path");
    }
    Ok(path)
}

fn connect(timeout: Duration) -> Result<UnixStream> {
    let socket = socket2::Socket::new(socket2::Domain::UNIX, socket2::Type::STREAM, None)?;
    socket.connect_timeout(&socket2::SockAddr::unix(socket_path()?)?, timeout)?;
    Ok(socket.into())
}

/// The attestation, the IPC connection, and whether the answer is lasting:
/// a socket nobody serves, or two different live peers. A timeout is not.
pub(super) fn probe(
    pinned: Option<libc::pid_t>,
    timeout: Duration,
) -> (Attestation, Option<UnixStream>, bool) {
    let ipc = connect(timeout);
    let ipc_peer = ipc
        .as_ref()
        .map_err(|e| anyhow!("{e:#}"))
        .and_then(|ipc| peer_pid(ipc.as_raw_fd()));
    // Without an IPC peer there is nothing to compare the Wayland peer with.
    let wayland_peer = match &ipc_peer {
        Ok(_) => wayland_connection_with_timeout(timeout)
            .and_then(|wayland| peer_pid(wayland.backend().poll_fd().as_raw_fd())),
        Err(_) => Err(anyhow!("not checked")),
    };
    let lasting = match &ipc {
        Err(error) => error.downcast_ref::<io::Error>().is_some_and(|error| {
            matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            )
        }),
        Ok(_) => ipc_peer.is_ok() && wayland_peer.is_ok(),
    };
    (attest(pinned, ipc_peer, wayland_peer), ipc.ok(), lasting)
}

/// True when this process drives a niri session: `NIRI_SOCKET` and
/// `WAYLAND_DISPLAY` were attested to be the same niri process, now or
/// earlier in this process. Recognition is remembered, so a later socket
/// loss refuses instead of turning this off. Also true while a probe cannot
/// tell (see [`Probed::Unknown`]), so niri's routes refuse.
pub fn is_session() -> bool {
    match session_check(
        !super::super::is_inject_mode() && super::super::is_wayland(),
        RECOGNIZED.get().is_some(),
        std::env::var_os("NIRI_SOCKET").is_some_and(|socket| !socket.is_empty()),
    ) {
        SessionCheck::NotNiri => return false,
        SessionCheck::Recognized => return true,
        SessionCheck::Probe => {}
    }
    // An unrelated socket stays unrelated for the same environment: a stale
    // NIRI_SOCKET on another desktop must not cost a probe on every call.
    static UNRELATED: Mutex<Option<(OsString, OsString)>> = Mutex::new(None);
    let environment = (
        std::env::var_os("NIRI_SOCKET").unwrap_or_default(),
        std::env::var_os("WAYLAND_DISPLAY").unwrap_or_default(),
    );
    if UNRELATED
        .lock()
        .is_ok_and(|unrelated| unrelated.as_ref() == Some(&environment))
    {
        return false;
    }
    let (attestation, connection, lasting) = probe(None, QUERY_TIMEOUT);
    match probed(attestation, lasting) {
        Probed::Niri(pid) => {
            // A proven niri stays niri even when it is too old for the
            // adapter or its version is not read yet: routes refuse with the
            // reason instead of falling back to generic Wayland discovery.
            if let Err(error) = release_check(&RELEASE, || {
                connection
                    .context("niri IPC connection unavailable")
                    .and_then(release_of)
            }) {
                tracing::warn!("{error:#}");
            }
            let pinned = *RECOGNIZED.get_or_init(|| pid);
            tracing::info!(pid = pinned, "niri session recognised");
            true
        }
        Probed::Unknown(reason) => {
            tracing::warn!("niri session probe failed, refusing until one passes: {reason}");
            *UNKNOWN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(reason);
            true
        }
        Probed::NotNiri(reason) => {
            static LOGGED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if !LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                tracing::warn!("NIRI_SOCKET is not this Wayland session's niri: {reason}");
            }
            if let Ok(mut unrelated) = UNRELATED.lock() {
                *unrelated = Some(environment);
            }
            false
        }
    }
}

#[derive(Debug, PartialEq)]
enum Probed {
    Niri(libc::pid_t),
    /// A failure that may pass, such as a timeout or a failed peer read:
    /// the session may be niri. Its routes refuse until a later probe
    /// recognises niri, instead of the generic Wayland routes the adapter
    /// replaces running on a niri session.
    Unknown(String),
    /// Another session's socket, or none listening: lasting, so remembered.
    NotNiri(String),
}

/// What a probe before recognition decides; `lasting` as [`probe`] reports.
fn probed(attestation: Attestation, lasting: bool) -> Probed {
    match attestation {
        Attestation::Attested(pid) => Probed::Niri(pid),
        Attestation::Unrelated(reason) | Attestation::Refused(reason) if lasting => {
            Probed::NotNiri(reason)
        }
        Attestation::Unrelated(reason) | Attestation::Refused(reason) => Probed::Unknown(reason),
    }
}

/// The refusal while niri is not recognised, with the reason a probe could
/// not tell when there is one.
fn not_recognised(unknown: Option<&str>) -> anyhow::Error {
    match unknown {
        Some(reason) => anyhow!(
            "niri session has not been recognised: the session probe failed ({reason}); try again once niri answers"
        ),
        None => anyhow!("niri session has not been recognised"),
    }
}

/// The recognised niri process, or the refusal while there is none.
pub(super) fn recognised() -> Result<libc::pid_t> {
    RECOGNIZED.get().copied().ok_or_else(|| {
        not_recognised(
            UNKNOWN
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_deref(),
        )
    })
}

#[derive(Debug, PartialEq)]
enum SessionCheck {
    NotNiri,
    /// Recognised earlier: stay niri even if the socket is gone, so routes
    /// refuse instead of falling through to generic Wayland discovery.
    Recognized,
    Probe,
}

fn session_check(adapter_on: bool, recognized: bool, socket_set: bool) -> SessionCheck {
    if !adapter_on {
        SessionCheck::NotNiri
    } else if recognized {
        SessionCheck::Recognized
    } else if socket_set {
        SessionCheck::Probe
    } else {
        SessionCheck::NotNiri
    }
}

/// The `Version` reply on a newly attested connection, which carries only
/// this request: niri before 25.05 serves one request per connection.
fn release_of(connection: UnixStream) -> Result<String> {
    let mut connection = Some(connection);
    Ipc {
        connect: move |_| {
            connection
                .take()
                .context("niri IPC connection already used")
        },
        per_attempt: QUERY_TIMEOUT,
        total: QUERY_TOTAL_TIMEOUT,
        backoff: QUERY_RETRY_BACKOFF,
        lines: None,
    }
    .query(Request::Version)
}

/// Whether the adapter supports niri `version`, such as `26.04 (8ed0da4)`.
/// Only the leading digits of the year and month count: packagers may add
/// a suffix, such as `26.04-1 (e35c630)`.
fn release_supported(version: &str) -> Result<(), String> {
    let release = version
        .split_once(" (")
        .map_or(version, |(release, _)| release);
    let leading = |part: Option<&str>| {
        let part = part?;
        let digits = part
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(part.len());
        part[..digits].parse::<u32>().ok()
    };
    let mut parts = release.split('.');
    match (leading(parts.next()), leading(parts.next())) {
        (Some(year), Some(month)) => {
            if (year, month) >= MIN_RELEASE {
                Ok(())
            } else {
                Err(format!(
                    "niri {release} is not supported: the niri adapter needs niri {}.{:02} or later",
                    MIN_RELEASE.0, MIN_RELEASE.1
                ))
            }
        }
        _ => Err(format!(
            "niri reports version {version:?}, which the niri adapter cannot read; it needs niri {}.{:02} or later",
            MIN_RELEASE.0, MIN_RELEASE.1
        )),
    }
}

/// Decide whether the adapter supports niri's release, reading its version
/// through `read` while undecided. A failed read leaves it undecided.
fn release_check(
    release: &Mutex<Option<Result<(), String>>>,
    read: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let mut release = release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let verdict = match release.as_ref() {
        Some(verdict) => verdict.clone(),
        None => {
            let version = read().context(
                "niri did not report its version, so the niri adapter cannot yet check that it supports it",
            )?;
            let verdict = release_supported(&version);
            if let Err(reason) = &verdict {
                tracing::warn!("{reason}");
            }
            release.insert(verdict).clone()
        }
    };
    verdict.map_err(|reason| anyhow!(reason))
}

/// The recognised niri process, when the adapter supports it. Until niri's
/// release is known, each call asks a newly attested connection.
pub(super) fn pinned() -> Result<libc::pid_t> {
    let pinned = recognised()?;
    release_check(&RELEASE, || release_of(attest_to(pinned, QUERY_TIMEOUT)?))?;
    Ok(pinned)
}

/// A connection to the recognised niri process, re-attested each time.
pub(super) fn attested_connection(deadline: Instant) -> Result<UnixStream> {
    attest_to(pinned()?, time_remaining(deadline)?)
}

fn attest_to(pinned: libc::pid_t, timeout: Duration) -> Result<UnixStream> {
    match probe(Some(pinned), timeout) {
        (Attestation::Attested(_), Some(ipc), _) => Ok(ipc),
        (Attestation::Unrelated(reason) | Attestation::Refused(reason), ..) => bail!("{reason}"),
        (Attestation::Attested(_), None, _) => bail!("niri IPC connection unavailable"),
    }
}

/// A Wayland client socket proven to reach the recognised niri process, for
/// a keyboard helper that must not rediscover the display on its own.
pub(in crate::wayland) fn attested_wayland_socket() -> Result<UnixStream> {
    let pinned = pinned()?;
    let socket = super::super::compositor_peer::wayland_socket_with_timeout(QUERY_TIMEOUT)?;
    let peer = peer_pid(socket.as_raw_fd())?;
    if peer != pinned {
        bail!("WAYLAND_DISPLAY is served by pid {peer}, not the recognised niri pid {pinned}");
    }
    Ok(socket)
}

/// IPC to the recognised niri process. One value serves one action: its
/// requests share one attested connection.
pub(super) type AttestedIpc = Ipc<fn(Instant) -> Result<UnixStream>>;

pub(super) fn ipc() -> AttestedIpc {
    Ipc {
        connect: attested_connection,
        per_attempt: QUERY_TIMEOUT,
        total: QUERY_TOTAL_TIMEOUT,
        backoff: QUERY_RETRY_BACKOFF,
        lines: None,
    }
}

// ── IPC transport ────────────────────────────────────────────────────────────

fn time_remaining(deadline: Instant) -> Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, "niri IPC timed out").into());
    }
    Ok(remaining)
}

fn is_timeout(error: &anyhow::Error) -> bool {
    error.downcast_ref::<io::Error>().is_some_and(|error| {
        matches!(
            error.kind(),
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
        )
    })
}

/// niri replies one JSON line per request; the event stream sends one line
/// per event.
pub(super) struct Lines {
    pub(super) stream: UnixStream,
    buffer: Vec<u8>,
    /// Bytes of `buffer` already known to hold no line end.
    scanned: usize,
}

impl Lines {
    pub(super) fn new(stream: UnixStream) -> Self {
        Self {
            stream,
            buffer: Vec::new(),
            scanned: 0,
        }
    }

    pub(super) fn send(&mut self, line: &str, deadline: Instant) -> Result<()> {
        self.stream
            .set_write_timeout(Some(time_remaining(deadline)?))?;
        self.stream.write_all(line.as_bytes())?;
        self.stream.write_all(b"\n")?;
        Ok(())
    }

    /// The next complete line, or `None` once `deadline` passes first.
    pub(super) fn next(&mut self, deadline: Instant) -> Result<Option<Vec<u8>>> {
        let mut chunk = [0u8; 8192];
        loop {
            if let Some(end) = self.buffer[self.scanned..]
                .iter()
                .position(|byte| *byte == b'\n')
            {
                let mut line: Vec<u8> = self.buffer.drain(..=self.scanned + end).collect();
                line.pop();
                self.scanned = 0;
                return Ok(Some(line));
            }
            self.scanned = self.buffer.len();
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }
            self.stream.set_read_timeout(Some(remaining))?;
            match self.stream.read(&mut chunk) {
                Ok(0) => bail!("niri closed the IPC connection"),
                Ok(count) => {
                    if self.buffer.len() + count > MAX_REPLY_BYTES {
                        bail!("niri IPC reply exceeds size limit");
                    }
                    self.buffer.extend_from_slice(&chunk[..count]);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(error) => return Err(error).context("niri IPC reply unavailable"),
            }
        }
    }

    /// One request and its reply line.
    fn exchange(&mut self, request: &str, deadline: Instant) -> Result<serde_json::Value> {
        self.send(request, deadline)?;
        self.reply(deadline)
    }

    /// The reply line to the request sent last.
    fn reply(&mut self, deadline: Instant) -> Result<serde_json::Value> {
        let line = self.next(deadline)?.ok_or_else(|| {
            anyhow::Error::from(io::Error::new(
                io::ErrorKind::TimedOut,
                "niri IPC reply timed out",
            ))
        })?;
        serde_json::from_slice(&line).context("invalid niri IPC JSON")
    }
}

/// The successful payload of a reply, or niri's own error text.
pub(super) fn reply_ok(request: &str, reply: serde_json::Value) -> Result<serde_json::Value> {
    let serde_json::Value::Object(mut reply) = reply else {
        bail!("unexpected niri reply to {request}");
    };
    if let Some(error) = reply.remove("Err") {
        bail!(
            "niri refused {request}: {}",
            error
                .as_str()
                .map_or_else(|| error.to_string(), str::to_owned)
        );
    }
    reply
        .remove("Ok")
        .with_context(|| format!("unexpected niri reply to {request}"))
}

/// The niri IPC requests this adapter reads. Only these are retried.
#[derive(Clone, Copy, Debug)]
pub(super) enum Request {
    Version,
    Windows,
    Workspaces,
    Outputs,
    FocusedWindow,
    OverviewState,
    KeyboardLayouts,
}

pub(super) struct Ipc<C> {
    pub(super) connect: C,
    pub(super) per_attempt: Duration,
    pub(super) total: Duration,
    pub(super) backoff: Duration,
    /// The connection earlier requests used. A connected socket keeps its
    /// peer, so it stays attested; niri answers one connection's requests
    /// in order. A failed exchange drops it, so a late reply is never read
    /// as the answer to a later request.
    pub(super) lines: Option<Lines>,
}

impl<C: FnMut(Instant) -> Result<UnixStream>> Ipc<C> {
    fn connection(&mut self, deadline: Instant) -> Result<Lines> {
        match self.lines.take() {
            Some(lines) => Ok(lines),
            None => Ok(Lines::new((self.connect)(deadline)?)),
        }
    }

    /// A read-only request. Only a timed-out request/reply is retried, on a
    /// newly attested connection; connection and attestation errors are final.
    pub(super) fn query<T: DeserializeOwned>(&mut self, request: Request) -> Result<T> {
        let deadline = Instant::now() + self.total;
        let name = format!("{request:?}");
        let name = name.as_str();
        for attempt in 1..=QUERY_MAX_ATTEMPTS {
            time_remaining(deadline)?;
            let attempt_deadline = deadline.min(Instant::now() + self.per_attempt);
            let mut lines = self.connection(attempt_deadline)?;
            match lines.exchange(&format!("\"{name}\""), attempt_deadline) {
                Ok(reply) => {
                    self.lines = Some(lines);
                    let mut payload = reply_ok(name, reply)?;
                    let value = payload
                        .get_mut(name)
                        .map(serde_json::Value::take)
                        .with_context(|| format!("niri reply to {name} has no {name}"))?;
                    return serde_json::from_value(value)
                        .with_context(|| format!("invalid niri {name} reply"));
                }
                Err(error) => {
                    if !is_timeout(&error)
                        || attempt == QUERY_MAX_ATTEMPTS
                        || deadline.saturating_duration_since(Instant::now()) <= self.backoff
                    {
                        return Err(error).with_context(|| {
                            format!("niri IPC {name} failed after {attempt} attempt(s)")
                        });
                    }
                    drop(lines);
                    tracing::warn!(name, attempt, "niri IPC request timed out; retrying");
                    std::thread::sleep(self.backoff);
                }
            }
        }
        unreachable!("the final query attempt always returns")
    }

    /// One compositor action. Never retried: after a timeout niri may have
    /// applied it. Like input, it is not sent once the tool call bound to
    /// this thread ran out of time (see [`crate::input::action`]): focus
    /// must not move after the caller stopped waiting.
    pub(super) fn action(&mut self, action: serde_json::Value) -> Result<()> {
        self.send_action(action, true)
    }

    /// An action that gives back focus the call took. It is cleanup, like
    /// a button release, so it goes out also once the call ran out of time.
    pub(super) fn restore_action(&mut self, action: serde_json::Value) -> Result<()> {
        self.send_action(action, false)
    }

    fn send_action(&mut self, action: serde_json::Value, gated: bool) -> Result<()> {
        let deadline = Instant::now() + self.per_attempt;
        let request = serde_json::json!({ "Action": action }).to_string();
        let mut lines = self.connection(deadline)?;
        if gated {
            crate::input::action::admit(|| lines.send(&request, deadline))??;
        } else {
            lines.send(&request, deadline)?;
        }
        let reply = lines
            .reply(deadline)
            .context("niri action outcome is unknown")?;
        self.lines = Some(lines);
        match reply_ok(&request, reply)? {
            serde_json::Value::String(handled) if handled == "Handled" => Ok(()),
            other => bail!("unexpected niri reply to {request}: {other}"),
        }
    }

    pub(super) fn focused_window(&mut self) -> Result<Option<IpcWindow>> {
        self.query(Request::FocusedWindow)
    }

    pub(super) fn scene(&mut self) -> Result<Scene> {
        let windows = self.query(Request::Windows)?;
        let workspaces = self.query(Request::Workspaces)?;
        let outputs = self.query(Request::Outputs)?;
        let overview: Overview = self.query(Request::OverviewState)?;
        Ok(Scene {
            windows,
            workspaces,
            outputs,
            overview_open: overview.is_open,
        })
    }
}

// ── Replies ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct IpcWindow {
    pub(super) id: u64,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) app_id: Option<String>,
    pub(super) pid: Option<i32>,
    pub(super) workspace_id: Option<u64>,
    pub(super) is_focused: bool,
    /// When the window last had focus; absent on older releases.
    #[serde(default)]
    pub(super) focus_timestamp: Option<IpcTimestamp>,
    /// Added in niri 25.08; absent on older releases.
    #[serde(default)]
    pub(super) layout: Option<IpcLayout>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct IpcTimestamp {
    secs: u64,
    nanos: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct IpcLayout {
    pub(super) pos_in_scrolling_layout: Option<(u64, u64)>,
    pub(super) tile_size: (f64, f64),
    pub(super) window_size: (i32, i32),
    pub(super) tile_pos_in_workspace_view: Option<(f64, f64)>,
    pub(super) window_offset_in_tile: (f64, f64),
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct IpcWorkspace {
    pub(super) id: u64,
    pub(super) output: Option<String>,
    pub(super) is_active: bool,
    pub(super) is_focused: bool,
    pub(super) active_window_id: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct IpcOutput {
    pub(super) name: String,
    pub(super) modes: Vec<IpcMode>,
    pub(super) current_mode: Option<usize>,
    pub(super) logical: Option<IpcLogical>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
pub(super) struct IpcMode {
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub(super) struct IpcLogical {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) scale: f64,
    pub(super) transform: String,
}

#[derive(Deserialize)]
pub(super) struct Overview {
    pub(super) is_open: bool,
}

#[derive(Deserialize)]
pub(super) struct KeyboardLayouts {
    pub(super) names: Vec<String>,
    pub(super) current_idx: u32,
}

/// wl_output transform number of a niri transform name.
pub(super) fn transform_code(name: &str) -> Result<u32> {
    Ok(match name {
        "Normal" => 0,
        "90" => 1,
        "180" => 2,
        "270" => 3,
        "Flipped" => 4,
        "Flipped90" => 5,
        "Flipped180" => 6,
        "Flipped270" => 7,
        other => bail!("unknown niri output transform {other:?}"),
    })
}

pub(super) fn valid_dimensions(width: u32, height: u32) -> bool {
    width > 0 && height > 0 && u64::from(width) * u64::from(height) <= MAX_LOGICAL_PIXELS
}

impl IpcLogical {
    pub(super) fn checked(&self) -> Result<&Self> {
        if !self.scale.is_finite()
            || self.scale <= 0.0
            || !valid_dimensions(self.width, self.height)
        {
            bail!("invalid niri output geometry");
        }
        Ok(self)
    }
}

impl IpcOutput {
    /// The output's size in niri's pointer space. Absolute virtual-pointer
    /// motion uses Smithay's output geometry, which rounds `mode / scale` up,
    /// while IPC truncates it (1280 px at 1.5 is 853 in IPC, 854 here).
    pub(super) fn pointer_rect(&self) -> Result<Option<Rect>> {
        let Some(logical) = &self.logical else {
            return Ok(None);
        };
        let logical = logical.checked()?;
        let mode = self
            .current_mode
            .and_then(|index| self.modes.get(index))
            .with_context(|| format!("niri output {} has no current mode", self.name))?;
        let (width, height) = super::super::logical_output_size(
            (mode.width, mode.height),
            transform_code(&logical.transform)?,
        );
        let width = (f64::from(width) / logical.scale).ceil();
        let height = (f64::from(height) / logical.scale).ceil();
        if !(1.0..=f64::from(u32::MAX)).contains(&width)
            || !(1.0..=f64::from(u32::MAX)).contains(&height)
        {
            bail!("invalid niri output mode");
        }
        Ok(Some(Rect {
            x: logical.x,
            y: logical.y,
            width: width as u32,
            height: height as u32,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use std::sync::Mutex;

    /// A connection whose peer sends `reply` (raw bytes) and then holds it open.
    fn replying(reply: Vec<u8>) -> UnixStream {
        let (client, mut server) = UnixStream::pair().unwrap();
        std::thread::spawn(move || {
            let _ = server.write_all(&reply);
            std::thread::sleep(Duration::from_secs(2));
        });
        client
    }

    // ── Session attestation ─────────────────────────────────────────────────

    #[test]
    fn foreign_socket_before_recognition_is_an_unrelated_session() {
        // A host desktop's inherited NIRI_SOCKET next to a nested compositor.
        assert_eq!(
            attest(None, Ok(10), Ok(20)),
            Attestation::Unrelated(
                "NIRI_SOCKET is served by pid 10 but WAYLAND_DISPLAY by pid 20".into()
            )
        );
        assert!(matches!(
            attest(None, Err(anyhow!("connection refused")), Ok(20)),
            Attestation::Unrelated(_)
        ));
        assert_eq!(attest(None, Ok(20), Ok(20)), Attestation::Attested(20));
    }

    #[test]
    fn a_probe_that_may_pass_leaves_the_session_unknown_and_refusing() {
        let timed_out = "niri IPC or WAYLAND_DISPLAY peer is unavailable (IPC: niri IPC timed out, Wayland: not checked)";
        assert_eq!(
            probed(Attestation::Unrelated(timed_out.into()), false),
            Probed::Unknown(timed_out.into())
        );
        assert_eq!(
            not_recognised(Some(timed_out)).to_string(),
            format!("niri session has not been recognised: the session probe failed ({timed_out}); try again once niri answers")
        );
        // Nothing listening, or another session's niri: not niri.
        assert_eq!(
            probed(Attestation::Unrelated("refused".into()), true),
            Probed::NotNiri("refused".into())
        );
        assert_eq!(probed(Attestation::Attested(20), true), Probed::Niri(20));
    }

    #[test]
    fn a_recognised_niri_stays_niri_after_its_socket_disappears() {
        assert_eq!(session_check(true, true, false), SessionCheck::Recognized);
        assert_eq!(session_check(true, false, false), SessionCheck::NotNiri);
        assert_eq!(session_check(true, false, true), SessionCheck::Probe);
        assert_eq!(session_check(false, true, true), SessionCheck::NotNiri);
    }

    #[test]
    fn a_proven_niri_refuses_until_a_version_read_answers() {
        let release = Mutex::new(None);
        // niri did not answer: still niri, refused, and asked again later.
        assert_eq!(
            format!(
                "{:#}",
                release_check(&release, || bail!("niri IPC request timed out")).unwrap_err()
            ),
            "niri did not report its version, so the niri adapter cannot yet check that it supports it: niri IPC request timed out"
        );
        release_check(&release, || Ok("26.04 (8ed0da4)".into())).unwrap();
        // Answered once: never read again.
        release_check(&release, || panic!("the release was read again")).unwrap();

        let release = Mutex::new(None);
        let old = "niri 25.05 is not supported: the niri adapter needs niri 25.08 or later";
        assert_eq!(
            release_check(&release, || Ok("25.05 (abc)".into()))
                .unwrap_err()
                .to_string(),
            old
        );
        assert_eq!(
            release_check(&release, || panic!("the release was read again"))
                .unwrap_err()
                .to_string(),
            old
        );
    }

    #[test]
    fn niri_older_than_25_08_is_refused_at_recognition() {
        for version in [
            "25.08 (6a1b2c3)",
            "25.11.1 (abc)",
            "26.04 (8ed0da4)",
            "26.04",
            // niri's packaging guide suggests a distribution suffix.
            "26.04-1 (abc)",
            "25.08.1-2",
        ] {
            assert_eq!(release_supported(version), Ok(()), "{version}");
        }
        // 25.05 still reads one request per connection; 25.08 added the
        // window layout every geometry route needs.
        assert_eq!(
            release_supported("25.05.1 (abc)"),
            Err("niri 25.05.1 is not supported: the niri adapter needs niri 25.08 or later".into())
        );
        assert!(release_supported("25.02").is_err());
        for version in ["", "unknown (abc)", "25", "25.x"] {
            assert_eq!(
                release_supported(version),
                Err(format!(
                    "niri reports version {version:?}, which the niri adapter cannot read; it needs niri 25.08 or later"
                )),
            );
        }
    }

    #[test]
    fn recognised_niri_refuses_loss_mismatch_and_a_new_compositor() {
        assert_eq!(attest(Some(20), Ok(20), Ok(20)), Attestation::Attested(20));
        match attest(Some(20), Err(anyhow!("No such file or directory")), Ok(20)) {
            Attestation::Refused(reason) => assert!(reason.contains("No such file or directory")),
            other => panic!("socket loss must refuse, got {other:?}"),
        }
        assert_eq!(
            attest(Some(20), Ok(20), Ok(30)),
            Attestation::Refused(
                "NIRI_SOCKET is served by pid 20 but WAYLAND_DISPLAY by pid 30".into()
            )
        );
        assert_eq!(
            attest(Some(20), Ok(40), Ok(40)),
            Attestation::Refused(
                "niri was recognised as pid 20, but the compositor is now pid 40".into()
            )
        );
    }

    // ── Transport ───────────────────────────────────────────────────────────

    #[test]
    fn stalled_niri_hits_the_deadline_and_retries_a_read_once() {
        let mut attempts = 0;
        let mut held = Vec::new();
        let started = Instant::now();
        let error = ipc_with(
            |_| {
                attempts += 1;
                let (client, server) = UnixStream::pair()?;
                held.push(server);
                Ok(client)
            },
            Duration::from_millis(40),
            Duration::from_millis(500),
        )
        .query::<Value>(Request::Windows)
        .unwrap_err();
        assert!(is_timeout(&error), "{error:#}");
        assert_eq!(attempts, 2);
        // Bounded by the deadlines, not by the peer; loose for loaded CI.
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn one_action_attests_one_connection_for_all_its_requests() {
        let fake = focus_fixture();
        let mut ipc = fake_ipc(&fake);
        ipc.focused_window().unwrap();
        activate_in(&mut ipc, 7, Some(107), Duration::from_millis(200)).unwrap();
        let fake = fake.lock().unwrap();
        assert_eq!(fake.requests.len(), 5, "{:?}", fake.requests);
        assert_eq!(fake.connections, 1);
    }

    #[test]
    fn an_action_is_never_retried_after_an_ambiguous_timeout() {
        let mut attempts = 0;
        let mut held = Vec::new();
        let error = ipc_with(
            |_| {
                attempts += 1;
                let (client, server) = UnixStream::pair()?;
                held.push(server);
                Ok(client)
            },
            Duration::from_millis(40),
            Duration::from_millis(500),
        )
        .action(focus_window_action(7))
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(error.to_string(), "niri action outcome is unknown");
    }

    #[test]
    fn connection_and_attestation_errors_are_not_retried() {
        let mut attempts = 0;
        let error = ipc_with(
            |_| {
                attempts += 1;
                bail!("NIRI_SOCKET is served by pid 10 but WAYLAND_DISPLAY by pid 20")
            },
            QUERY_TIMEOUT,
            QUERY_TOTAL_TIMEOUT,
        )
        .query::<Value>(Request::Windows)
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert_eq!(
            error.to_string(),
            "NIRI_SOCKET is served by pid 10 but WAYLAND_DISPLAY by pid 20"
        );
    }

    #[test]
    fn niri_error_text_is_kept_verbatim() {
        let error = ipc_with(
            |_| Ok(replying(b"{\"Err\":\"error parsing request\"}\n".to_vec())),
            QUERY_TIMEOUT,
            QUERY_TOTAL_TIMEOUT,
        )
        .action(focus_window_action(7))
        .unwrap_err();
        assert!(error.to_string().ends_with(
            "niri refused {\"Action\":{\"FocusWindow\":{\"id\":7}}}: error parsing request"
        ));
    }

    #[test]
    fn oversize_and_malformed_replies_refuse() {
        let reply = |bytes: Vec<u8>| {
            ipc_with(
                move |_| Ok(replying(bytes.clone())),
                QUERY_TIMEOUT,
                QUERY_TOTAL_TIMEOUT,
            )
            .query::<Vec<IpcWindow>>(Request::Windows)
            .unwrap_err()
        };
        let oversize = reply(vec![b' '; MAX_REPLY_BYTES + 1]);
        assert!(format!("{oversize:#}").contains("niri IPC reply exceeds size limit"));
        let id_as_string =
            br#"{"Ok":{"Windows":[{"id":"7","pid":1,"workspace_id":1,"is_focused":false}]}}
"#;
        assert!(
            format!("{:#}", reply(id_as_string.to_vec())).contains("invalid niri Windows reply")
        );
        let other_variant = b"{\"Ok\":{\"Workspaces\":[]}}\n";
        assert!(format!("{:#}", reply(other_variant.to_vec()))
            .contains("niri reply to Windows has no Windows"));
    }

    #[test]
    fn unknown_fields_are_ignored_and_null_pid_has_no_owner() {
        let mut window = floating(7, 1, (100.0, 60.0), (300, 200));
        window["pid"] = Value::Null;
        window["is_new_in_a_later_release"] = json!({"nested": [1, 2]});
        window["layout"]["also_new"] = json!(true);
        let scene = one_output(vec![window]);
        let window = scene.window(7).unwrap();
        assert_eq!(window.pid, None);
        assert!(window.placement.is_ok());
    }
}
