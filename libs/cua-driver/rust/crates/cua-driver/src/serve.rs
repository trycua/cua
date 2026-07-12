//! Unix-socket daemon server and client for `cua-driver serve`/`stop`/`status`.
//!
//! Protocol: line-delimited JSON over a Unix domain socket.
//!
//! Request shapes:
//!   {"method":"call","name":"<tool>","args":{...}}
//!   {"method":"list"}
//!   {"method":"describe","name":"<tool>"}
//!   {"method":"shutdown"}
//!
//! Response shapes:
//!   {"ok":true,"result":...}
//!   {"ok":false,"error":"...","exit_code":1}
//!
//! The socket file is at:
//!   macOS  — ~/Library/Caches/cua-driver/cua-driver.sock
//!   Linux  — ~/.cache/cua-driver/cua-driver.sock
//!   Windows — \\.\pipe\cua-driver  (TODO: use named pipe; stubs only for now)

use serde::{Deserialize, Serialize};

// ── Recording idle-TTL backstop (#1764) ─────────────────────────────────────────

/// Default TTL of zero `call` activity after which an active recording is
/// auto-stopped. Generous: a single agent turn can be slow. Overridable via the
/// `CUA_DRIVER_RS_RECORDING_IDLE_TTL_SECS` env var (used by the #1764 verify
/// harness to exercise the backstop quickly).
const RECORDING_IDLE_TTL_SECS_DEFAULT: u64 = 300;

/// Wall-clock seconds since the Unix epoch. Same idiom `recording.rs` uses for
/// `now_ms`.
fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Resolve + apply the session identity for a tool call at the daemon boundary.
///
/// A session is a **caller-declared** identity (the public `session` arg), not a
/// property of the MCP connection. We mirror an explicit `session` into the
/// reserved `_session_id` key that every session-aware tool already reads
/// (cursor key, per-session config override, recording owner). When no `session`
/// was declared we fall back to the per-connection minted id for `_session_id`
/// ONLY — that preserves connection-EOF cleanup of recording / config as before.
/// The cursor is deliberately NOT driven by that fallback: `resolve_cursor_key`
/// reads the explicit `session`/`cursor_id` arg only, so a cursor appears
/// exactly when a run declares its session (explicit-required).
///
/// Also refreshes the idle-TTL clock for an explicit session (the minted
/// fallback is reaped by EOF, not TTL). Returns the effective `_session_id` for
/// the resurrection guard.
fn apply_session_identity(
    args: &mut serde_json::Value,
    minted: &Option<String>,
) -> Option<String> {
    let explicit = args
        .as_object()
        .and_then(|o| o.get("session"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned());
    if let Some(obj) = args.as_object_mut() {
        if !obj.contains_key("_session_id") {
            if let Some(id) = explicit.clone().or_else(|| minted.clone()) {
                obj.insert("_session_id".to_owned(), serde_json::Value::String(id));
            }
        }
    }
    if let Some(sess) = &explicit {
        cua_driver_core::session::touch_session(sess);
    }
    args.as_object()
        .and_then(|o| o.get("_session_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_owned())
}

/// Whether `tool_name` manages session lifecycle and so must be EXEMPT from the
/// resurrection guard. `start_session` revives an ended id (the explicit,
/// caller-intended way to reuse one) and `end_session` is idempotent — both
/// would be wrongly rejected if the guard gated them on an already-ended id.
fn is_session_lifecycle_tool(tool_name: &str) -> bool {
    matches!(tool_name, "start_session" | "end_session")
}

/// Resolve the recording idle TTL, honoring the env override.
fn recording_idle_ttl_secs() -> u64 {
    std::env::var("CUA_DRIVER_RS_RECORDING_IDLE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(RECORDING_IDLE_TTL_SECS_DEFAULT)
}

/// Spawn a detached daemon task that auto-stops a recording after the idle TTL.
///
/// Defense-in-depth for #1764: the primary teardown is now the per-session
/// reaper (the proxy's persistent control connection EOF fires `session_end`,
/// covering proxy SIGKILL/crash). This TTL covers the residual case a non-proxy
/// raw client starts a recording and dies without ever sending `session_begin`.
/// It MUST NOT stop a still-active session: it only reaps when BOTH
/// the recording is enabled AND there has been no `call` activity for the full
/// TTL. As long as a session keeps issuing tool calls, `last_activity` is bumped
/// every turn and the idle window never reaches the TTL. `stop()` is idempotent,
/// so racing with the proxy-exit hook or an explicit `stop_recording` is benign.
fn spawn_recording_idle_backstop(
    registry: std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    last_activity: std::sync::Arc<std::sync::atomic::AtomicU64>,
) {
    let ttl = recording_idle_ttl_secs();
    tokio::spawn(async move {
        // 30s tick granularity: reap latency is `ttl` rounded up to the next
        // tick, so a sub-30s TTL override (e.g. in tests) still fires no sooner
        // than ~30s. Fine for the 300s production default.
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            if registry.recording.current_state().enabled {
                let idle = now_unix_secs().saturating_sub(
                    last_activity.load(std::sync::atomic::Ordering::Relaxed),
                );
                if idle >= ttl {
                    tracing::warn!(
                        "recording idle {idle}s ≥ {ttl}s TTL; auto-stopping \
                         (proxy-exit hook likely missed)"
                    );
                    // Unconditional (`None`): the idle backstop is a last-resort
                    // GLOBAL kill of whatever recording is active after global
                    // inactivity — not an owner reclaiming a specific session.
                    // It already targets the live recording by definition, so a
                    // requester id would be redundant and could only make it
                    // wrongly no-op. Session-scoped teardown is the proxy-exit
                    // `session_end` path (#1764 / session-identity work).
                    // stop_owner can SYNCHRONOUSLY finalize the recording's mp4
                    // (SCStream::stop_capture blocks on disk I/O), so run it on a
                    // blocking thread to keep the reactor free (see the EOF reaper).
                    let reg2 = registry.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        reg2.recording.stop_owner(None)
                    })
                    .await;
                }
            }
        }
    });
}

/// Default idle-TTL for a caller-declared session (seconds). A session that
/// isn't touched (no tool call carrying its `session`) for this long is reclaimed
/// by [`spawn_session_idle_sweep`] — its cursor removed, recording stopped,
/// config cleared. Overridable via `CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS`.
const SESSION_IDLE_TTL_SECS_DEFAULT: u64 = 300;

fn session_idle_ttl_secs() -> u64 {
    std::env::var("CUA_DRIVER_RS_SESSION_IDLE_TTL_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(SESSION_IDLE_TTL_SECS_DEFAULT)
}

/// Spawn the detached idle-TTL sweep for caller-declared sessions.
///
/// A session is no longer tied to a connection's lifetime (it's a caller-declared
/// identity that can span connections, transports, and apps), so a run that ends
/// — or crashes — without calling `end_session` is reclaimed here. Each tick ends
/// every session whose last activity is older than the TTL; `session::evict_idle`
/// fans `fire_session_end` out to the cursor/recording/config cleanup hooks. A
/// session that keeps issuing tool calls bumps its activity every turn and never
/// reaches the idle window. Idempotent and cheap.
fn spawn_session_idle_sweep() {
    let ttl = std::time::Duration::from_secs(session_idle_ttl_secs());
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            let ended = cua_driver_core::session::evict_idle(ttl);
            if !ended.is_empty() {
                tracing::info!(count = ended.len(), "idle-TTL reclaimed sessions: {ended:?}");
            }
        }
    });
}

/// Register a `session_end` hook that stops a recording the ending session owns.
///
/// The per-platform cursor-remove + config-clear hooks already run on
/// `fire_session_end`; this adds recording teardown to the SAME signal, so
/// `end_session`, the idle-TTL sweep, and the control-connection EOF reaper all
/// stop a session's recording uniformly (matching `end_session`'s contract).
/// `stop_owner(Some(sid))` is a no-op unless `sid` owns the live recording, and
/// runs on a detached thread so finalizing the mp4 never blocks the synchronous
/// `fire_session_end` caller (the sweep task or an async tool invoke). The EOF
/// arm keeps its own inline `spawn_blocking` stop for ordered finalize-then-reply;
/// a second stop here is an idempotent no-op.
fn register_recording_session_end_hook(
    recording: std::sync::Arc<cua_driver_core::recording::RecordingSession>,
) {
    cua_driver_core::session::register_session_end_hook(move |sid| {
        let recording = recording.clone();
        let sid = sid.to_owned();
        std::thread::spawn(move || {
            let _ = recording.stop_owner(Some(&sid));
        });
    });
}

// ── Paths ─────────────────────────────────────────────────────────────────────

/// Returns the platform default socket/pipe path.
pub fn default_socket_path() -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/cua-driver/cua-driver.sock")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/cua-driver/cua-driver.sock")
    }
    #[cfg(target_os = "windows")]
    {
        r"\\.\pipe\cua-driver".to_owned()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/tmp/cua-driver.sock".to_owned()
    }
}

/// On Windows, returns the named-pipe path of the uiAccess-elevated worker
/// (`cua-driver-uia.exe`). The main CLI/MCP binary can prefer this pipe over the
/// regular daemon pipe for the one path that genuinely needs UIAccess integrity:
/// **synthetic input (SendInput / pixel clicks) into AppContainer (UWP) windows**,
/// which UIPI blocks from a Medium-IL process. The element-action path (UIA
/// Invoke / ValuePattern driven by `element_index`) does NOT need the worker — it
/// drives real UWP apps (verified: Calculator num5Button 0→5) as-is from the
/// Medium-IL daemon. See #1602 / the `cua-driver-uia` crate for the worker side.
#[cfg(target_os = "windows")]
pub fn default_uia_pipe_path() -> String {
    r"\\.\pipe\cua-driver-uia".to_owned()
}

/// Returns the platform default PID file path.
pub fn default_pid_file_path() -> String {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/cua-driver/cua-driver.pid")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/cua-driver/cua-driver.pid")
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:/Temp".into());
        format!("{local}/cua-driver/cua-driver.pid")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/tmp/cua-driver.pid".to_owned()
    }
}

// ── Protocol types ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonRequest {
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub args: Option<serde_json::Value>,
    /// MCP-session identity minted once per `cua-driver mcp` proxy process and
    /// stamped on every forwarded request. The daemon uses it to OWN and CLEAN
    /// UP session-scoped state (recording, config overrides). Absent (`None`)
    /// means an anonymous/"global" session — a one-shot `cua-driver call` or a
    /// legacy proxy that predates this field — which keeps today's behavior.
    /// `skip_serializing_if = Option::is_none` makes the absent case
    /// byte-identical on the wire, so old clients and daemons interoperate.
    /// serde defaults a missing field to `None` on deserialize.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

impl DaemonResponse {
    pub fn ok(result: serde_json::Value) -> Self {
        Self { ok: true, result: Some(result), error: None, exit_code: None }
    }
    pub fn err(msg: impl Into<String>, code: i32) -> Self {
        Self { ok: false, result: None, error: Some(msg.into()), exit_code: Some(code) }
    }
}

// ── Client ────────────────────────────────────────────────────────────────────

/// Probe whether a daemon is listening on `socket_path`.
///
/// On Windows this uses `WaitNamedPipeW` with a tiny timeout, which checks
/// whether a pipe instance is available **without consuming one**. The
/// previous design (sending a `list` request) opened a pipe instance,
/// then the immediately-following real `send_request` had to wait for the
/// daemon to spin up its NEXT instance — that race caused real tool calls
/// (especially state-dependent ones like `click` that need the daemon's
/// element_index cache) to fall through to the in-process path with a
/// fresh empty cache. See the conversation around the
/// "Element 3 not in cache" bug for the diagnosis.
///
/// `WaitNamedPipeW(name, 1)`:
///   - Returns TRUE if an instance is currently available.
///   - Returns FALSE if not available within 1 ms.
///   - Does NOT consume the instance — that's reserved for the actual call.
pub fn is_daemon_listening(socket_path: &str) -> bool {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::ffi::OsStrExt;

        // Raw FFI to `WaitNamedPipeW` so this crate doesn't have to pull in
        // the `windows` crate as a direct dependency just for one probe.
        // `kernel32.dll` is implicitly linked on Windows targets.
        #[link(name = "kernel32")]
        extern "system" {
            fn WaitNamedPipeW(lpNamedPipeName: *const u16, nTimeOut: u32) -> i32;
        }

        let wide: Vec<u16> = std::ffi::OsStr::new(socket_path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // NMPWAIT_NOWAIT == 1: "do not wait at all"; returns nonzero only if an
        // instance is immediately available. We don't want to block here —
        // this is a one-shot existence probe.
        unsafe { WaitNamedPipeW(wide.as_ptr(), 1) != 0 }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let req = DaemonRequest { method: "list".into(), name: None, args: None, session_id: None };
        send_request(socket_path, &req)
            .ok()
            .map(|r| r.ok)
            .unwrap_or(false)
    }
}

/// Read the PID stored in `pid_file_path`, if any.
pub fn read_pid_file(pid_file_path: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

/// Send a request to the daemon and return the response.
/// Uses a 3-second connect timeout (by polling) and a 10-second read timeout.
#[cfg(unix)]
pub fn send_request(socket_path: &str, req: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    use std::io::Read;
    use std::os::unix::net::UnixStream;
    use std::time::{Duration, Instant};

    let mut stream = UnixStream::connect(socket_path)
        .map_err(|e| anyhow::anyhow!("connect to {socket_path}: {e}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    // Per-read timeout acts as a liveness poll, NOT a hard cap on the whole
    // response: an AX-heavy `get_window_state` (slow tree walk) or a multi-MB
    // SOM screenshot can legitimately take longer than one window to produce.
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;

    let mut w = stream.try_clone()?;
    let line = serde_json::to_string(req)? + "\n";
    // EAGAIN-aware write: a daemon momentarily too busy to read our request
    // (backpressure under concurrent slow tools) makes the 5s SO_SNDTIMEO write
    // time out. Treat that like the read loop below does (#1997) — keep retrying
    // until an overall deadline — instead of failing with a fatal "Resource
    // temporarily unavailable (os error 35)" transport error. Mirrors the 120s
    // read budget.
    let write_deadline = Instant::now() + Duration::from_secs(120);
    cua_driver_core::socket_io::write_all_with_retry(&mut w, line.as_bytes(), write_deadline)?;

    // Read the single newline-terminated response line. A blocking UnixStream
    // with SO_RCVTIMEO returns `WouldBlock`/`TimedOut` (EAGAIN, os error 35)
    // when the timeout elapses with no bytes ready — that is NOT a transport
    // failure, just "the daemon is still working". Previously this surfaced as
    // a fatal `daemon transport error … Resource temporarily unavailable`
    // (#1864). Loop and keep waiting (re-arming the per-read poll) until we
    // have a full line, the daemon closes the connection, or an overall
    // deadline generous enough for the slowest AX walk / largest response.
    let overall_deadline = Instant::now() + Duration::from_secs(120);
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut chunk = [0u8; 64 * 1024];
    let resp_line = loop {
        if let Some(nl) = buf.iter().position(|&b| b == b'\n') {
            break String::from_utf8_lossy(&buf[..nl]).into_owned();
        }
        match stream.read(&mut chunk) {
            Ok(0) => {
                // EOF. Use whatever we buffered (some daemons close right after
                // a final unterminated line); otherwise the daemon hung up.
                if buf.is_empty() {
                    anyhow::bail!("daemon closed connection without response");
                }
                break String::from_utf8_lossy(&buf).into_owned();
            }
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= overall_deadline {
                    anyhow::bail!(
                        "timed out after 120s waiting for daemon response \
                         (received {} bytes so far)",
                        buf.len()
                    );
                }
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    };
    let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
    Ok(resp)
}

#[cfg(not(unix))]
pub fn send_request(socket_path: &str, req: &DaemonRequest) -> anyhow::Result<DaemonResponse> {
    #[cfg(target_os = "windows")]
    {
        use std::io::{BufRead, BufReader, Write};
        use std::time::Duration;

        // Retry opening the pipe — server may still be starting.
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let pipe = loop {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(socket_path)
            {
                Ok(f) => break f,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) => anyhow::bail!("connect to named pipe {socket_path}: {e}"),
            }
        };

        let mut writer = pipe.try_clone()?;
        let line = serde_json::to_string(req)? + "\n";
        writer.write_all(line.as_bytes())?;
        writer.flush()?;

        // Server writes one JSON line then flushes; pipe stays open until we drop it.
        let reader = BufReader::new(pipe);
        let resp_line = reader
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("daemon closed connection without response"))??;
        let resp: DaemonResponse = serde_json::from_str(&resp_line)?;
        Ok(resp)
    }
    #[cfg(not(target_os = "windows"))]
    {
        anyhow::bail!("daemon client not supported on this platform (socket path: {socket_path})");
    }
}

// ── Server ────────────────────────────────────────────────────────────────────

/// Run the daemon server. Binds `socket_path`, writes `pid_file_path`,
/// accepts connections, and serves requests until `{"method":"shutdown"}`.
///
/// This is `async` and must be called from a tokio runtime.
#[cfg(unix)]
pub async fn run_serve(
    registry: std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixListener;

    // Create parent directory.
    if let Some(dir) = std::path::Path::new(socket_path).parent() {
        std::fs::create_dir_all(dir)?;
    }

    // Remove stale socket file (from a crashed previous daemon).
    let _ = std::fs::remove_file(socket_path);

    let listener = UnixListener::bind(socket_path)
        .map_err(|e| anyhow::anyhow!("bind {socket_path}: {e}"))?;

    eprintln!("Cua Driver daemon listening on {socket_path}");

    // Write PID file.
    if let Some(pid_path) = pid_file_path {
        if let Some(dir) = std::path::Path::new(pid_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(pid_path, std::process::id().to_string());
    }

    // Shutdown channel.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    // Idle-TTL recording backstop (#1764), now demoted to SECONDARY cover. The
    // PRIMARY teardown is the per-session reaper: the proxy holds a persistent
    // control connection (session_begin) whose EOF — on graceful exit AND on
    // kill -9 — fires session_end, stopping that session's recording reliably.
    // This TTL still uniquely covers (a) a non-proxy raw client that starts a
    // recording and dies without ever sending session_begin and (b) a
    // daemon-internal wedge. We track the last `call` activity timestamp; a
    // background task auto-stops the recording after `RECORDING_IDLE_TTL_SECS`
    // of zero tool activity. Keyed on call activity (NOT connection liveness),
    // so an actively-used session is never reaped.
    let last_activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_unix_secs()));
    spawn_recording_idle_backstop(registry.clone(), last_activity.clone());
    spawn_session_idle_sweep();
    if let Some(port) = crate::mcp_http::configured_port() {
        crate::mcp_http::spawn(registry.clone(), port);
    }
    register_recording_session_end_hook(registry.recording.clone());

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let reg = registry.clone();
                let shutdown_tx2 = shutdown_tx.clone();
                let last_activity = last_activity.clone();

                tokio::spawn(async move {
                    let (reader, mut writer) = stream.into_split();
                    let mut lines = BufReader::new(reader).lines();

                    // Set ONLY by a `session_begin` on this connection — i.e.
                    // the proxy's persistent control connection. Per-call
                    // connections (call/list/describe) leave this None, so
                    // their immediate EOF triggers NO teardown. When a control
                    // connection EOFs (graceful proxy exit OR kill -9, both
                    // kernel-guaranteed), the post-loop block reaps the session.
                    let mut control_session_id: Option<String> = None;

                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: DaemonRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                let resp = DaemonResponse::err(
                                    format!("JSON parse error: {e}"), 65
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                continue;
                            }
                        };

                        match req.method.as_str() {
                            "shutdown" => {
                                let resp = DaemonResponse::ok(serde_json::json!({"shutdown": true}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                let mut guard = shutdown_tx2.lock().await;
                                if let Some(tx) = guard.take() {
                                    let _ = tx.send(());
                                }
                                return;
                            }
                            "list" => {
                                // Include full ToolDef (input_schema + annotation
                                // hints + capabilities) so MCP proxy callers can
                                // build a complete `tools/list` response from
                                // one daemon round-trip. Older clients that only
                                // read name/description still work — the extra
                                // fields are ignored.
                                //
                                // `capabilities` is sourced from the centralised
                                // `cua_driver_core::tool::default_capabilities_for`
                                // name → tokens map so the daemon and in-process
                                // paths emit identical capability arrays.
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| {
                                        let caps = cua_driver_core::tool::default_capabilities_for(name);
                                        serde_json::json!({
                                            "name": name,
                                            "description": def.description,
                                            "input_schema": def.input_schema,
                                            "read_only": def.read_only,
                                            "destructive": def.destructive,
                                            "idempotent": def.idempotent,
                                            "open_world": def.open_world,
                                            "capabilities": caps,
                                        })
                                    })
                                    .collect();
                                // Mirror `tools_list` envelope: include
                                // capability_version + schema_version so MCP
                                // proxy callers can pass them through verbatim
                                // (one daemon round-trip, complete response).
                                let resp = DaemonResponse::ok(serde_json::json!({
                                    "tools": tools,
                                    "capability_version": cua_driver_core::tool::CAPABILITY_VERSION,
                                    "schema_version": "1",
                                }));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "describe" => {
                                let name = req.name.as_deref().unwrap_or("");
                                match reg.get_def(name) {
                                    Some(def) => {
                                        let resp = DaemonResponse::ok(serde_json::json!({
                                            "name": def.name,
                                            "description": def.description,
                                            "input_schema": def.input_schema
                                        }));
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                    None => {
                                        let resp = DaemonResponse::err(
                                            format!("Unknown tool: {name}"), 64
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                    }
                                }
                            }
                            "call" => {
                                // Recording idle-TTL liveness: any serviced tool
                                // call counts as activity (#1764).
                                last_activity.store(
                                    now_unix_secs(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                let raw_name = req.name.as_deref().unwrap_or("").to_owned();
                                // Deprecated alias: `type_text_chars` → `type_text`.
                                // Mirrors Swift's `ToolRegistry.call` aliasing.
                                let tool_name = if raw_name == "type_text_chars" {
                                    eprintln!("[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                                    "type_text".to_owned()
                                } else { raw_name.clone() };
                                let mut args = req.args.unwrap_or(serde_json::Value::Object(
                                    serde_json::Map::new()
                                ));
                                // Apply the caller-declared session identity
                                // (explicit `session` → `_session_id`; minted id
                                // as the recording/config fallback only). See
                                // `apply_session_identity` for the full rationale
                                // and the cursor's explicit-required contract.
                                let effective_session =
                                    apply_session_identity(&mut args, &req.session_id);
                                // Resurrection guard: a call whose effective
                                // session has already ended (end_session / idle
                                // TTL / control-connection EOF) must NOT run — it
                                // would re-create session-owned state (cursor,
                                // config override, recording) the reaper already
                                // passed. Reject it LOUDLY (isError) so a stray
                                // late action is visibly a failure, not a phantom
                                // success a caller silently trusts. The session-
                                // lifecycle tools are EXEMPT: `start_session`
                                // revives an ended id (explicit, intentional reuse)
                                // and `end_session` stays idempotent. Live and
                                // anonymous calls pass through unchanged.
                                if let Some(sid) = &effective_session {
                                    if !is_session_lifecycle_tool(&tool_name)
                                        && cua_driver_core::session::is_session_ended(sid)
                                    {
                                        let resp = DaemonResponse::err(
                                            format!(
                                                "session '{sid}' has ended; tool call '{tool_name}' was \
                                                 rejected. Call start_session with this id to revive it \
                                                 before issuing further actions, or use a new session id."
                                            ),
                                            1,
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                        continue;
                                    }
                                }
                                if reg.get_def(&tool_name).is_none() {
                                    let resp = DaemonResponse::err(
                                        format!("Unknown tool: {tool_name}"), 64
                                    );
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let result = reg.invoke(&tool_name, args).await;
                                let is_err = result.is_error.unwrap_or(false);
                                let content: Vec<serde_json::Value> = result.content.iter().map(|c| {
                                    match c {
                                        cua_driver_core::protocol::Content::Text { text, .. } =>
                                            serde_json::json!({"type":"text","text":text}),
                                        cua_driver_core::protocol::Content::Image { data, mime_type, .. } =>
                                            serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                                    }
                                }).collect();
                                let mut result_obj = serde_json::json!({
                                    "content": content,
                                    "isError": is_err
                                });
                                if let Some(sc) = result.structured_content {
                                    result_obj["structuredContent"] = sc;
                                }
                                // Preserve tool-level `isError` and structured
                                // content inside a successful daemon transport.
                                let resp = DaemonResponse::ok(result_obj);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_begin" => {
                                // The proxy's persistent control connection.
                                // Record the session_id so this connection's EOF
                                // (graceful proxy exit OR kill -9) reaps the
                                // session in the post-loop block below. This is
                                // the ONLY place a connection is marked control;
                                // per-call connections never send this. ACK ok.
                                if let Some(sid) = req.session_id.as_deref() {
                                    control_session_id = Some(sid.to_owned());
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({"session_begin": true})
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_end" => {
                                // Legacy back-compat: a new-daemon/old-proxy
                                // pairing still sends this explicitly on graceful
                                // stdin EOF. It arrives on a per-call connection
                                // (control_session_id stays None) so it does NOT
                                // double-fire against the EOF reaper, and
                                // fire_session_end is idempotent regardless. New
                                // proxies never send it — the control-connection
                                // EOF is the single teardown path. Always ACK ok.
                                if let Some(sid) = req.session_id.as_deref() {
                                    // stop_owner can SYNCHRONOUSLY finalize the
                                    // recording's mp4 — run it off the reactor on
                                    // a blocking thread (see the EOF reaper).
                                    // fire_session_end stays inline (non-blocking).
                                    // Mark the session ended FIRST so an in-flight
                                    // start_recording sees ended=true and bails — the
                                    // mark-before-reap invariant the cursor/config hooks
                                    // already satisfy (their reap runs inside
                                    // fire_session_end, after the mark). stop_owner does
                                    // not consult is_session_ended, so reaping after the
                                    // mark is safe.
                                    cua_driver_core::session::fire_session_end(sid);
                                    let reg2 = reg.clone();
                                    let sid_for_stop = sid.to_owned();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        reg2.recording.stop_owner(Some(&sid_for_stop))
                                    })
                                    .await;
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({"session_end": true})
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            other => {
                                let resp = DaemonResponse::err(
                                    format!("Unknown method: {other}"), 65
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                        }
                    }

                    // Reader EOF (`while let` exited): the kernel closes the
                    // socket on graceful proxy exit AND on kill -9. If this was
                    // the proxy's persistent control connection, reap the
                    // session now — the reliable ungraceful-death teardown that
                    // the old graceful-only proxy-exit hook could not provide.
                    // Per-call connections leave control_session_id None, so
                    // their (immediate) EOF is a no-op here. fire_session_end is
                    // idempotent, so racing a legacy explicit session_end is
                    // benign.
                    if let Some(sid) = control_session_id {
                        // stop_owner can SYNCHRONOUSLY finalize the recording's
                        // mp4 — on macOS it hits SCStream::stop_capture(), which
                        // blocks on disk I/O (video_sckit.rs). Run it on a
                        // blocking thread so it does not stall a runtime worker.
                        // fire_session_end stays inline: its hooks (overlay
                        // Remove, config-override clear) are non-blocking.
                        // Mark the session ended FIRST so an in-flight start_recording
                        // sees ended=true and bails (mark-before-reap; the cursor/config
                        // hooks already reap inside fire_session_end after the mark).
                        // stop_owner ignores is_session_ended, so reaping after is safe.
                        cua_driver_core::session::fire_session_end(&sid);
                        let reg2 = reg.clone();
                        let sid_for_stop = sid.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            reg2.recording.stop_owner(Some(&sid_for_stop))
                        })
                        .await;
                    }
                });
            }
            _ = &mut shutdown_rx => {
                eprintln!("Cua Driver daemon shutting down.");
                break;
            }
        }
    }

    // Clean up.
    let _ = std::fs::remove_file(socket_path);
    if let Some(pid_path) = pid_file_path {
        let _ = std::fs::remove_file(pid_path);
    }

    Ok(())
}

/// On Windows, optionally spawn the sibling uiAccess'd worker
/// (`cua-driver-uia.exe`) via ShellExecute if it lives next to the main binary
/// AND we're at Medium IL AND the binary is opt-in via env var.
///
/// History: the uia worker was the original answer to "send synthetic input
/// (SendInput / pixel clicks) into UWP / AppContainer windows from a Medium-IL
/// daemon" — UIPI blocks that cross-integrity input, so the worker carries
/// `uiAccess="true"` in its manifest and was meant to be Authenticode-signed
/// (EV cert per #1602) so Windows AIS would elevate it to UIAccess integrity at
/// launch.
///
/// IMPORTANT (verified): the worker is NOT required to automate real UWP apps in
/// general. The element-action path — UIA Invoke / ValuePattern driven by
/// `element_index` — drives AppContainer apps as-is from the Medium-IL daemon
/// (Calculator num5Button 0→5, no worker). Only the pixel / SendInput path needs
/// the worker, and only against AppContainer (UWP) targets.
///
/// With #1630 the canonical answer for that input path became "register the
/// autostart task at RunLevel=Highest so the main daemon is already at High IL",
/// which obviates the worker entirely for the vast majority of users.
///
/// Current behavior:
///
/// 1. If the main daemon is already at High IL (the RunLevel=Highest path),
///    skip the worker — it's redundant and, more importantly, attempting to
///    ShellExecute an unsigned uiAccess'd PE pops a Windows error dialog
///    ("A referral was returned from the server" = AIS refusing to elevate
///    an unsigned uiAccess binary). That dialog blocks the daemon's startup
///    and confuses users.
///
/// 2. If the main daemon is at Medium IL (older installs without the
///    Highest task), AND `CUA_DRIVER_RS_SPAWN_UIA_WORKER=1` is set (opt-in),
///    AND a uiAccess'd worker is installed, spawn it. This path is kept for
///    the future EV-cert flow where the worker IS properly signed.
///
/// 3. Otherwise: skip silently. The main daemon still serves requests, and
///    element_index UWP automation (UIA Invoke / ValuePattern) works without the
///    worker. Only pixel / SendInput into AppContainer (UWP) windows needs the
///    elevated path — re-run with the Highest autostart task or (when shipped)
///    the signed uia worker. See #1602.
#[cfg(target_os = "windows")]
fn maybe_spawn_uia_worker() {
    // Skip when at High IL — main daemon already has the privileges the
    // worker was supposed to provide.
    if is_self_at_high_il() {
        tracing::debug!("uia spawn skipped: main daemon already at High IL");
        return;
    }

    // Opt-in for the future EV-cert flow. Default-off until the worker is
    // actually signed and tested.
    if !crate::bundle::is_env_truthy("CUA_DRIVER_RS_SPAWN_UIA_WORKER") {
        tracing::debug!(
            "uia spawn skipped: CUA_DRIVER_RS_SPAWN_UIA_WORKER not set (opt-in only \
             until the worker is EV-signed; see #1602)"
        );
        return;
    }

    let current = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            tracing::debug!("uia spawn skipped: current_exe failed: {e}");
            return;
        }
    };
    let uia = match current.parent() {
        Some(dir) => dir.join("cua-driver-uia.exe"),
        None => return,
    };
    if !uia.exists() {
        tracing::debug!("uia spawn skipped: {} not present", uia.display());
        return;
    }
    let uia_str = uia.display().to_string();
    let cmd = format!(
        "(New-Object -ComObject Shell.Application).ShellExecute('{uia_str}','','','',0)"
    );
    match std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &cmd])
        .spawn()
    {
        Ok(_child) => {
            eprintln!("cua-driver: spawned uiAccess worker via {}", uia.display());
        }
        Err(e) => {
            tracing::warn!("uia spawn failed: {e}");
        }
    }
}

/// Returns true when the current process is at High IL (admin token). Checked
/// via a one-shot PowerShell call to `WindowsPrincipal.IsInRole(Administrator)`
/// — the standard managed equivalent of OpenProcessToken + GetTokenInformation.
///
/// Done via PowerShell instead of the windows-crate Win32 API because cua-driver
/// doesn't depend on the `windows` crate directly (only platform-windows does),
/// and `serve.rs` runs only once at daemon start so the ~50ms PowerShell-spawn
/// cost is acceptable.
#[cfg(target_os = "windows")]
fn is_self_at_high_il() -> bool {
    let out = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "([System.Security.Principal.WindowsPrincipal][System.Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([System.Security.Principal.WindowsBuiltInRole]::Administrator)",
        ])
        .output();
    match out {
        Ok(o) => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.trim().eq_ignore_ascii_case("True")
        }
        Err(_) => false,
    }
}

/// Build a SECURITY_ATTRIBUTES that lets any Medium-IL (or higher) process
/// open the named pipe, even though the daemon itself is running at
/// High IL (via the autostart Scheduled Task's `RunLevel=Highest`, per
/// #1630). Without this, the default UIPI rule "no write-up across IL
/// boundaries" makes the High-IL daemon's pipe unreachable from a normal
/// Medium-IL user shell — every `cua-driver <tool>` call from the CLI
/// silently falls through to in-process execution with a fresh, empty
/// `ToolState`, which breaks the element_index cache invariant
/// (`get_window_state` → `click(element_index)` stops working because
/// the two calls land in different ToolState instances).
///
/// SDDL: `D:(A;OICI;GA;;;WD)S:(ML;;NW;;;LW)`
///   - DACL grants `GENERIC_ALL` to `WD` (Everyone).
///   - SACL sets the mandatory label to LOW with `NW` (NoWriteUp), so
///     processes at Low-IL and above can write the pipe — i.e. no
///     IL-based write restriction in practice.
///
/// Returns the SECURITY_ATTRIBUTES struct AND the raw security-descriptor
/// pointer (which the caller must keep alive for the lifetime of the
/// pipe-server, then free via `LocalFree`).
#[cfg(target_os = "windows")]
unsafe fn build_open_pipe_security_attrs()
    -> Option<(SecurityAttributesRaw, *mut std::ffi::c_void)>
{
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut std::ffi::c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }
    let sddl: Vec<u16> = "D:(A;OICI;GA;;;WD)S:(ML;;NW;;;LW)\0"
        .encode_utf16()
        .collect();
    let mut sd_ptr: *mut std::ffi::c_void = std::ptr::null_mut();
    let mut sd_size: u32 = 0;
    let ok = ConvertStringSecurityDescriptorToSecurityDescriptorW(
        sddl.as_ptr(),
        1, // SDDL_REVISION_1
        &mut sd_ptr,
        &mut sd_size,
    );
    if ok == 0 || sd_ptr.is_null() {
        return None;
    }
    let attrs = SecurityAttributesRaw {
        n_length: std::mem::size_of::<SecurityAttributesRaw>() as u32,
        lp_security_descriptor: sd_ptr,
        b_inherit_handle: 0,
    };
    Some((attrs, sd_ptr))
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct SecurityAttributesRaw {
    n_length: u32,
    lp_security_descriptor: *mut std::ffi::c_void,
    b_inherit_handle: i32,
}

#[cfg(target_os = "windows")]
pub async fn run_serve(
    registry: std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::windows::named_pipe::ServerOptions;

    eprintln!("Cua Driver daemon listening on {socket_path}");

    // Build the cross-IL security descriptor once, reuse on every pipe
    // instance. If this fails we fall back to the default-ACL `create`
    // (which is High-IL exclusive when the daemon is at High IL — i.e.
    // the bug we're trying to fix). Log the failure so it's diagnosable.
    let security_attrs = unsafe { build_open_pipe_security_attrs() };
    if security_attrs.is_none() {
        eprintln!(
            "cua-driver: failed to build cross-IL SECURITY_ATTRIBUTES; pipe will be \
             High-IL exclusive. CLI calls from Medium-IL shells will fall through \
             to in-process and break state-dependent tool sequences."
        );
    }
    // Hold the SD pointer alive for the lifetime of run_serve. We never
    // free it — the daemon process exit reclaims it.
    let sec_attrs_ptr: *mut std::ffi::c_void = match &security_attrs {
        Some((attrs, _sd_ptr)) => attrs as *const _ as *mut _,
        None => std::ptr::null_mut(),
    };

    // Spawn the sibling uiAccess'd worker if it's installed. Best-effort —
    // the main daemon still serves requests even if the worker fails to start.
    maybe_spawn_uia_worker();

    // Write PID file.
    if let Some(pid_path) = pid_file_path {
        if let Some(dir) = std::path::Path::new(pid_path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(pid_path, std::process::id().to_string());
    }

    let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let shutdown_tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx)));

    // Idle-TTL recording backstop (#1764). See the unix branch above for the
    // full rationale; the leak (record_video via ffmpeg) is platform-independent.
    let last_activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_unix_secs()));
    spawn_recording_idle_backstop(registry.clone(), last_activity.clone());
    spawn_session_idle_sweep();
    if let Some(port) = crate::mcp_http::configured_port() {
        crate::mcp_http::spawn(registry.clone(), port);
    }
    register_recording_session_end_hook(registry.recording.clone());

    loop {
        // Create a new pipe server instance to accept the next client.
        // Use create_with_security_attributes_raw so Medium-IL clients can
        // open the pipe even though we're at High IL (see comment on
        // build_open_pipe_security_attrs). Fall back to default ACL if
        // SD construction failed at startup.
        let server = if sec_attrs_ptr.is_null() {
            ServerOptions::new()
                .first_pipe_instance(false)
                .create(socket_path)
                .map_err(|e| anyhow::anyhow!("create named pipe {socket_path}: {e}"))?
        } else {
            unsafe {
                ServerOptions::new()
                    .first_pipe_instance(false)
                    .create_with_security_attributes_raw(socket_path, sec_attrs_ptr)
                    .map_err(|e| anyhow::anyhow!("create named pipe {socket_path}: {e}"))?
            }
        };

        tokio::select! {
            result = server.connect() => {
                result.map_err(|e| anyhow::anyhow!("named pipe connect: {e}"))?;

                let reg = registry.clone();
                let shutdown_tx2 = shutdown_tx.clone();
                let last_activity = last_activity.clone();

                tokio::spawn(async move {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();

                    // See the unix branch: set only by `session_begin` on the
                    // proxy's persistent control connection; drives the post-loop
                    // EOF reaper. Named-pipe peer death surfaces as
                    // ERROR_BROKEN_PIPE on the next read, ending the while-let
                    // loop equally reliably.
                    let mut control_session_id: Option<String> = None;

                    while let Ok(Some(line)) = lines.next_line().await {
                        let req: DaemonRequest = match serde_json::from_str(&line) {
                            Ok(r) => r,
                            Err(e) => {
                                let resp = DaemonResponse::err(format!("JSON parse error: {e}"), 65);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                continue;
                            }
                        };

                        match req.method.as_str() {
                            "shutdown" => {
                                let resp = DaemonResponse::ok(serde_json::json!({"shutdown": true}));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                                let mut guard = shutdown_tx2.lock().await;
                                if let Some(tx) = guard.take() { let _ = tx.send(()); }
                                return;
                            }
                            "list" => {
                                // Include full ToolDef so MCP proxy callers can
                                // build a complete `tools/list` response from
                                // one daemon round-trip. See the unix branch
                                // above for rationale (capabilities map, etc.).
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| {
                                        let caps = cua_driver_core::tool::default_capabilities_for(name);
                                        serde_json::json!({
                                            "name": name,
                                            "description": def.description,
                                            "input_schema": def.input_schema,
                                            "read_only": def.read_only,
                                            "destructive": def.destructive,
                                            "idempotent": def.idempotent,
                                            "open_world": def.open_world,
                                            "capabilities": caps,
                                        })
                                    })
                                    .collect();
                                // Mirror `tools_list` envelope: include
                                // capability_version + schema_version so MCP
                                // proxy callers can pass them through verbatim
                                // (one daemon round-trip, complete response).
                                let resp = DaemonResponse::ok(serde_json::json!({
                                    "tools": tools,
                                    "capability_version": cua_driver_core::tool::CAPABILITY_VERSION,
                                    "schema_version": "1",
                                }));
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "describe" => {
                                let name = req.name.as_deref().unwrap_or("");
                                let resp = match reg.get_def(name) {
                                    Some(def) => DaemonResponse::ok(serde_json::json!({
                                        "name": def.name, "description": def.description,
                                        "input_schema": def.input_schema
                                    })),
                                    None => DaemonResponse::err(format!("Unknown tool: {name}"), 64),
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "call" => {
                                // Recording idle-TTL liveness (#1764).
                                last_activity.store(
                                    now_unix_secs(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                let raw_name = req.name.as_deref().unwrap_or("").to_owned();
                                let tool_name = if raw_name == "type_text_chars" {
                                    eprintln!("[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead.");
                                    "type_text".to_owned()
                                } else { raw_name.clone() };
                                let mut args = req.args.unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                                // Apply the caller-declared session identity
                                // (see the unix branch + apply_session_identity).
                                let effective_session =
                                    apply_session_identity(&mut args, &req.session_id);
                                // Resurrection guard on the effective session
                                // (see the unix branch for the full rationale):
                                // reject a stray late action on a dead id LOUDLY,
                                // but exempt the lifecycle tools so start_session
                                // can revive and end_session stays idempotent.
                                if let Some(sid) = &effective_session {
                                    if !is_session_lifecycle_tool(&tool_name)
                                        && cua_driver_core::session::is_session_ended(sid)
                                    {
                                        let resp = DaemonResponse::err(
                                            format!(
                                                "session '{sid}' has ended; tool call '{tool_name}' was \
                                                 rejected. Call start_session with this id to revive it \
                                                 before issuing further actions, or use a new session id."
                                            ),
                                            1,
                                        );
                                        let _ = writer.write_all(
                                            (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                        ).await;
                                        continue;
                                    }
                                }
                                if reg.get_def(&tool_name).is_none() {
                                    let resp = DaemonResponse::err(format!("Unknown tool: {tool_name}"), 64);
                                    let _ = writer.write_all(
                                        (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                    ).await;
                                    continue;
                                }
                                let result = reg.invoke(&tool_name, args).await;
                                let is_err = result.is_error.unwrap_or(false);
                                let content: Vec<serde_json::Value> = result.content.iter().map(|c| match c {
                                    cua_driver_core::protocol::Content::Text { text, .. } =>
                                        serde_json::json!({"type":"text","text":text}),
                                    cua_driver_core::protocol::Content::Image { data, mime_type, .. } =>
                                        serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
                                }).collect();
                                let mut result_obj = serde_json::json!({"content": content, "isError": is_err});
                                if let Some(sc) = result.structured_content {
                                    result_obj["structuredContent"] = sc;
                                }
                                // Preserve tool-level `isError` and structured
                                // content inside a successful daemon transport.
                                let resp = DaemonResponse::ok(result_obj);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_begin" => {
                                // The proxy's persistent control connection (see
                                // the unix branch). Record the session_id so this
                                // pipe instance's EOF / broken-pipe reaps the
                                // session in the post-loop block below. ACK ok.
                                if let Some(sid) = req.session_id.as_deref() {
                                    control_session_id = Some(sid.to_owned());
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({"session_begin": true})
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "session_end" => {
                                // Legacy back-compat (see the unix branch). Per-call
                                // connection; control_session_id stays None so no
                                // EOF double-fire. fire_session_end is idempotent.
                                if let Some(sid) = req.session_id.as_deref() {
                                    // stop_owner can SYNCHRONOUSLY finalize the
                                    // recording's mp4 — run it off the reactor on
                                    // a blocking thread (see the EOF reaper).
                                    // fire_session_end stays inline (non-blocking).
                                    // Mark the session ended FIRST so an in-flight
                                    // start_recording sees ended=true and bails — the
                                    // mark-before-reap invariant the cursor/config hooks
                                    // already satisfy (their reap runs inside
                                    // fire_session_end, after the mark). stop_owner does
                                    // not consult is_session_ended, so reaping after the
                                    // mark is safe.
                                    cua_driver_core::session::fire_session_end(sid);
                                    let reg2 = reg.clone();
                                    let sid_for_stop = sid.to_owned();
                                    let _ = tokio::task::spawn_blocking(move || {
                                        reg2.recording.stop_owner(Some(&sid_for_stop))
                                    })
                                    .await;
                                }
                                let resp = DaemonResponse::ok(
                                    serde_json::json!({"session_end": true})
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            other => {
                                let resp = DaemonResponse::err(format!("Unknown method: {other}"), 65);
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                        }
                    }

                    // Reader EOF / ERROR_BROKEN_PIPE: named-pipe peer death on
                    // graceful proxy exit AND kill -9. Reap the session if this
                    // was the proxy's control connection (see the unix branch for
                    // the full rationale). Per-call connections leave
                    // control_session_id None.
                    if let Some(sid) = control_session_id {
                        // Run stop_owner off the reactor (see the unix branch):
                        // recording finalize can be a synchronous blocking call.
                        // fire_session_end stays inline (non-blocking hooks).
                        // Mark the session ended FIRST so an in-flight start_recording
                        // sees ended=true and bails (mark-before-reap; the cursor/config
                        // hooks already reap inside fire_session_end after the mark).
                        // stop_owner ignores is_session_ended, so reaping after is safe.
                        cua_driver_core::session::fire_session_end(&sid);
                        let reg2 = reg.clone();
                        let sid_for_stop = sid.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            reg2.recording.stop_owner(Some(&sid_for_stop))
                        })
                        .await;
                    }
                });
            }
            _ = &mut shutdown_rx => {
                eprintln!("Cua Driver daemon shutting down.");
                break;
            }
        }
    }

    if let Some(pid_path) = pid_file_path {
        let _ = std::fs::remove_file(pid_path);
    }
    Ok(())
}

#[cfg(not(any(unix, target_os = "windows")))]
pub async fn run_serve(
    _registry: std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    _socket_path: &str,
    _pid_file_path: Option<&str>,
) -> anyhow::Result<()> {
    anyhow::bail!("cua-driver serve is not supported on this platform");
}

// ── CLI helpers ───────────────────────────────────────────────────────────────

/// `cua-driver serve` implementation.
pub fn run_serve_cmd(
    registry: std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    socket_path: &str,
    pid_file_path: Option<&str>,
) {
    let socket_path = socket_path.to_owned();
    let pid_file_path = pid_file_path.map(str::to_owned);

    // Fail fast if another daemon is already running.
    if is_daemon_listening(&socket_path) {
        let pid_hint = pid_file_path.as_deref()
            .and_then(|p| read_pid_file(p))
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default();
        eprintln!(
            "Cua Driver daemon is already running on {socket_path}{pid_hint}. \
             Run `cua-driver stop` first."
        );
        std::process::exit(1);
    }

    // Session-0 warning banner (Windows). The daemon runs fine in services
    // / SSH contexts for tools that don't touch the desktop, but every
    // window-driving tool (click, type_text, screenshot, get_window_state,
    // list_windows, launch_app for UWP) will fail or return empty when
    // invoked from this daemon. Surfacing this at startup saves users
    // hours of debugging tools that are working as designed.
    #[cfg(target_os = "windows")]
    {
        if matches!(
            platform_windows::diagnostics::current_session_id(),
            Some(0),
        ) {
            eprintln!(
                "WARNING: cua-driver serve is starting in Session 0 (services). \
                 Window-driving tools — click, type_text, screenshot, \
                 get_window_state, list_windows, and UWP launches — need an \
                 attached interactive desktop and will fail or return empty \
                 here. Re-run from an interactive logon (RDP, console, or a \
                 scheduled task in the user's session) for the GUI tools to \
                 function. Non-GUI tools (list_apps for Win32, get_config, \
                 doctor, etc.) work normally."
            );
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    if let Err(e) = rt.block_on(run_serve(
        registry,
        &socket_path,
        pid_file_path.as_deref(),
    )) {
        eprintln!("cua-driver serve error: {e}");
        std::process::exit(1);
    }
}

/// `cua-driver stop` implementation.
pub fn run_stop_cmd(socket_path: &str) {
    if !is_daemon_listening(socket_path) {
        eprintln!("Cua Driver daemon is not running");
        std::process::exit(1);
    }

    let req = DaemonRequest { method: "shutdown".into(), name: None, args: None, session_id: None };
    match send_request(socket_path, &req) {
        Ok(_) => {
            // Poll until daemon stops responding (up to 2 seconds).
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                // On Windows the pipe path isn't a filesystem file; probe liveness instead.
                let gone = if std::path::Path::new(socket_path).exists() {
                    false
                } else {
                    !is_daemon_listening(socket_path)
                };
                if gone {
                    // Swift's `stop` exits silently on success (no stdout
                    // line) — match that for byte-for-byte parity.
                    return;
                }
                if std::time::Instant::now() >= deadline {
                    eprintln!("Cua Driver daemon did not release socket within 2s");
                    std::process::exit(1);
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
        Err(e) => {
            eprintln!("stop: {e}");
            std::process::exit(1);
        }
    }
}

/// `cua-driver status` implementation.
pub fn run_status_cmd(socket_path: &str, pid_file_path: &str) {
    if is_daemon_listening(socket_path) {
        println!("Cua Driver daemon is running");
        println!("  socket: {socket_path}");
        if let Some(pid) = read_pid_file(pid_file_path) {
            println!("  pid: {pid}");
        } else {
            println!("  pid: unknown (no pid file)");
        }
    } else {
        eprintln!("Cua Driver daemon is not running");
        std::process::exit(1);
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(all(test, unix))]
mod gate_tests {
    //! Ended-session resurrection guard wired into the `call` dispatch.
    //!
    //! Closes PR #1779's gap: `is_session_ended()` was dead code, so an
    //! in-flight per-call request landing AFTER `session_end` fired would
    //! re-create session-owned metadata (cursor registry / config override)
    //! the reaper already passed. The gate REJECTS a `call` carrying an ended
    //! session id LOUDLY (isError) — a silent benign-ok was a trap that looked
    //! like success while doing nothing. The session-lifecycle tools are exempt:
    //! a `start_session` re-declare REVIVES the id so its subsequent actions run
    //! again. Live and anonymous calls always pass through.

    use super::{run_serve, send_request, DaemonRequest};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;
    use cua_driver_core::protocol::ToolResult;
    use cua_driver_core::tool::{Tool, ToolDef, ToolRegistry};
    use serde_json::Value;

    static PROBE_INVOCATIONS: AtomicUsize = AtomicUsize::new(0);

    struct ProbeTool {
        def: ToolDef,
    }

    impl ProbeTool {
        fn new() -> Self {
            Self {
                def: ToolDef {
                    name: "probe".into(),
                    description: "test probe; bumps a shared invocation counter".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    read_only: false,
                    destructive: false,
                    idempotent: true,
                    open_world: false,
                },
            }
        }
    }

    #[async_trait]
    impl Tool for ProbeTool {
        fn def(&self) -> &ToolDef {
            &self.def
        }
        async fn invoke(&self, _args: Value) -> ToolResult {
            PROBE_INVOCATIONS.fetch_add(1, Ordering::SeqCst);
            ToolResult::text("probe ran")
        }
    }

    fn call_req(sid: Option<&str>) -> DaemonRequest {
        DaemonRequest {
            method: "call".into(),
            name: Some("probe".into()),
            args: Some(serde_json::json!({})),
            session_id: sid.map(|s| s.to_owned()),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ended_session_call_is_gated_live_and_anon_pass() {
        PROBE_INVOCATIONS.store(0, Ordering::SeqCst);

        let mut reg = ToolRegistry::new();
        reg.register(Box::new(ProbeTool::new()));
        // The real start_session tool so we can prove an explicit re-declare
        // REVIVES an ended id end-to-end through the daemon boundary.
        reg.register(Box::new(cua_driver_core::session_tools::StartSessionTool));
        let registry = Arc::new(reg);

        // Unique temp socket — never the default socket / CuaDriver.app daemon.
        let socket = format!(
            "/tmp/cua-driver-gate-test-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let socket_for_server = socket.clone();
        let reg_for_server = registry.clone();
        let server = tokio::spawn(async move {
            let _ = run_serve(reg_for_server, &socket_for_server, None).await;
        });

        // Wait for the daemon to bind.
        for _ in 0..100 {
            if std::path::Path::new(&socket).exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        let sid = "gate-test-session-A1B2C3";

        // 1. LIVE session call → tool runs.
        let socket1 = socket.clone();
        let s1 = sid.to_owned();
        let resp = tokio::task::spawn_blocking(move || send_request(&socket1, &call_req(Some(&s1))))
            .await
            .unwrap()
            .expect("live call response");
        assert!(resp.ok, "live-session call should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            1,
            "live-session call must invoke the tool"
        );

        // 2. End the session (mirrors the control-conn EOF reaper path).
        let socket2 = socket.clone();
        let end = DaemonRequest {
            method: "session_end".into(),
            name: None,
            args: None,
            session_id: Some(sid.to_owned()),
        };
        let resp = tokio::task::spawn_blocking(move || send_request(&socket2, &end))
            .await
            .unwrap()
            .expect("session_end response");
        assert!(resp.ok, "session_end should ack ok");

        // 3. ENDED session call carrying the same sid → REJECTED LOUDLY. The
        //    tool must NOT run, and the caller must see a failure (not a phantom
        //    success that silently does nothing).
        let socket3 = socket.clone();
        let s3 = sid.to_owned();
        let resp = tokio::task::spawn_blocking(move || send_request(&socket3, &call_req(Some(&s3))))
            .await
            .unwrap()
            .expect("ended call response");
        assert!(!resp.ok, "ended-session call must be rejected loudly (not ok)");
        assert!(
            resp.error.as_deref().unwrap_or("").contains("has ended"),
            "rejection must explain the session ended; got {:?}",
            resp.error
        );
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            1,
            "rejected ended-session call must not invoke the tool — resurrection closed"
        );

        // 3b. Explicit re-declare via start_session REVIVES the id.
        let socket3b = socket.clone();
        let s3b = sid.to_owned();
        let start = DaemonRequest {
            method: "call".into(),
            name: Some("start_session".into()),
            args: Some(serde_json::json!({})),
            session_id: Some(s3b),
        };
        let resp = tokio::task::spawn_blocking(move || send_request(&socket3b, &start))
            .await
            .unwrap()
            .expect("start_session response");
        assert!(resp.ok, "start_session must run even for an ended id (lifecycle-exempt)");

        // 3c. A call on the revived id now RUNS again.
        let socket3c = socket.clone();
        let s3c = sid.to_owned();
        let resp = tokio::task::spawn_blocking(move || send_request(&socket3c, &call_req(Some(&s3c))))
            .await
            .unwrap()
            .expect("revived call response");
        assert!(resp.ok, "call on a revived session should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            2,
            "revived-session call must invoke the tool again"
        );

        // 4. Anonymous call (no session id) still passes — no false positive.
        let socket4 = socket.clone();
        let resp = tokio::task::spawn_blocking(move || send_request(&socket4, &call_req(None)))
            .await
            .unwrap()
            .expect("anon call response");
        assert!(resp.ok, "anonymous call should succeed");
        assert_eq!(
            PROBE_INVOCATIONS.load(Ordering::SeqCst),
            3,
            "anonymous (no session id) call must still invoke the tool"
        );

        // Tear down the daemon.
        let socket5 = socket.clone();
        let shutdown = DaemonRequest {
            method: "shutdown".into(),
            name: None,
            args: None,
            session_id: None,
        };
        let _ = tokio::task::spawn_blocking(move || send_request(&socket5, &shutdown)).await;
        let _ = server.await;
        let _ = std::fs::remove_file(&socket);
    }
}

#[cfg(test)]
mod session_boundary_tests {
    use super::apply_session_identity;
    use serde_json::json;

    #[test]
    fn explicit_session_becomes_session_id_and_is_returned() {
        let mut args = json!({ "x": 1, "session": "research-1" });
        let eff = apply_session_identity(&mut args, &None);
        assert_eq!(eff.as_deref(), Some("research-1"));
        assert_eq!(args["_session_id"], "research-1");
    }

    #[test]
    fn no_session_falls_back_to_minted_for_session_id_only() {
        // The minted per-connection id drives `_session_id` (recording / config
        // lifecycle) but there is NO explicit `session`, so the cursor resolver
        // — which reads `session`/`cursor_id`, not `_session_id` — sees nothing.
        let mut args = json!({ "x": 1 });
        let eff = apply_session_identity(&mut args, &Some("mcp-123".to_owned()));
        assert_eq!(args["_session_id"], "mcp-123");
        assert_eq!(eff.as_deref(), Some("mcp-123"));
        assert!(args.get("session").is_none());
    }

    #[test]
    fn anonymous_when_no_session_and_no_minted() {
        let mut args = json!({ "x": 1 });
        let eff = apply_session_identity(&mut args, &None);
        assert!(eff.is_none());
        assert!(args.get("_session_id").is_none());
    }

    #[test]
    fn caller_set_session_id_is_not_overwritten_by_minted() {
        let mut args = json!({ "_session_id": "caller-set" });
        let eff = apply_session_identity(&mut args, &Some("mcp-999".to_owned()));
        assert_eq!(args["_session_id"], "caller-set");
        assert_eq!(eff.as_deref(), Some("caller-set"));
    }
}
