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

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

pub use cua_driver_core::daemon::{
    is_daemon_listening, send_request, socket_path_for_namespace, DaemonRequest, DaemonResponse,
    ToolObservationOrigin,
};

static ACTIVE_PROXY_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn active_proxy_sessions() -> &'static Mutex<HashSet<String>> {
    ACTIVE_PROXY_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn is_active_proxy_session(session: Option<&str>) -> bool {
    session.is_some_and(|session| active_proxy_sessions().lock().unwrap().contains(session))
}

fn inject_browser_approvals(tool_name: &str, args: &mut serde_json::Value, session: Option<&str>) {
    if tool_name == "browser_prepare" && is_active_proxy_session(session) {
        if let Some(arguments) = args.as_object_mut() {
            arguments.insert(
                cua_driver_core::browser::approval::MCP_HOST_APPROVAL_ARG.to_owned(),
                serde_json::Value::Bool(true),
            );
        }
    }
    if tool_name == "browser_download" && is_active_proxy_session(session) {
        if let Some(arguments) = args.as_object_mut() {
            arguments.insert(
                cua_driver_core::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG.to_owned(),
                serde_json::Value::Bool(true),
            );
        }
    }
}

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
fn apply_session_identity(args: &mut serde_json::Value, minted: &Option<String>) -> Option<String> {
    let explicit = args
        .as_object()
        .and_then(|o| o.get("session"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned());
    if let Some(obj) = args.as_object_mut() {
        obj.remove("_session_id");
        obj.remove("_transport_session_id");
        if let Some(id) = explicit.clone().or_else(|| minted.clone()) {
            obj.insert("_session_id".to_owned(), serde_json::Value::String(id));
        }
        if let Some(id) = minted.clone() {
            obj.insert(
                "_transport_session_id".to_owned(),
                serde_json::Value::String(id),
            );
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
                let idle = now_unix_secs()
                    .saturating_sub(last_activity.load(std::sync::atomic::Ordering::Relaxed));
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
                    let _ =
                        tokio::task::spawn_blocking(move || reg2.recording.stop_owner(None)).await;
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
                tracing::info!(
                    count = ended.len(),
                    "idle-TTL reclaimed sessions: {ended:?}"
                );
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
    socket_path_for_namespace(crate::bundle::state_namespace())
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
    if crate::bundle::is_local_installation() {
        r"\\.\pipe\cua-driver-local-uia".to_owned()
    } else {
        r"\\.\pipe\cua-driver-uia".to_owned()
    }
}

/// Returns the platform default PID file path.
pub fn default_pid_file_path() -> String {
    let namespace = crate::bundle::state_namespace();
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/Library/Caches/{namespace}/{namespace}.pid")
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        format!("{home}/.cache/{namespace}/{namespace}.pid")
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:/Temp".into());
        format!("{local}/{namespace}/{namespace}.pid")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "/tmp/cua-driver.pid".to_owned()
    }
}

// ── Protocol types ────────────────────────────────────────────────────────────

fn daemon_observation_transport(req: &DaemonRequest) -> Option<crate::telemetry::Transport> {
    match req.observation_origin {
        Some(ToolObservationOrigin::McpProxy) => Some(crate::telemetry::Transport::McpStdio),
        Some(ToolObservationOrigin::Direct) => Some(crate::telemetry::Transport::Daemon),
        // Legacy callers did not declare ownership. Leaving them unobserved
        // preserves the legacy proxy as the single emitter during rollout;
        // current direct callers explicitly select `Direct` above.
        None => None,
    }
}

fn observe_daemon_result(
    observation: Option<(
        cua_driver_core::server::ToolObservationTimer,
        crate::telemetry::Transport,
    )>,
    session_context: Option<cua_driver_core::session::SessionToolContext>,
    result: serde_json::Value,
) -> serde_json::Value {
    let Some((timer, transport)) = observation else {
        return result;
    };
    let response = cua_driver_core::protocol::Response::ok(serde_json::Value::Null, result);
    let outcome = timer.finish(&response);
    if let Some(context) = session_context {
        context.complete(&outcome);
    }
    crate::telemetry::capture_tool_completed(outcome, transport);
    match response.body {
        cua_driver_core::protocol::ResponseBody::Result { result } => result,
        cua_driver_core::protocol::ResponseBody::Error { .. } => {
            unreachable!("constructed ok response")
        }
    }
}

fn observe_daemon_error(
    observation: Option<(
        cua_driver_core::server::ToolObservationTimer,
        crate::telemetry::Transport,
    )>,
    exit_code: i32,
) {
    let Some((timer, transport)) = observation else {
        return;
    };
    let response = cua_driver_core::protocol::Response::ok(
        serde_json::Value::Null,
        serde_json::json!({
            "content": [],
            "isError": true,
            "structuredContent": { "exit_code": exit_code },
        }),
    );
    crate::telemetry::capture_tool_completed(timer.finish(&response), transport);
}

async fn invoke_daemon_tool(
    registry: &std::sync::Arc<cua_driver_core::tool::ToolRegistry>,
    req: DaemonRequest,
) -> DaemonResponse {
    let observation_transport = daemon_observation_transport(&req);
    let raw_name = req.name.as_deref().unwrap_or("").to_owned();
    let tool_name = if raw_name == "type_text_chars" {
        eprintln!(
            "[cua-driver-rs] deprecated tool name 'type_text_chars' — use 'type_text' instead."
        );
        "type_text".to_owned()
    } else {
        raw_name
    };
    let known_tool = registry.get_def(&tool_name).is_some();
    let mut args = req
        .args
        .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
    let effective_session = apply_session_identity(&mut args, &req.session_id);
    let operation = cua_driver_core::server::tool_operation(&tool_name, Some(&args));
    let observation = observation_transport.map(|transport| {
        (
            cua_driver_core::server::ToolObservationTimer::start_with_operation(
                tool_name.clone(),
                operation,
                known_tool,
                true,
                cua_driver_core::server::StdioExecutionPath::DirectDaemon,
            ),
            transport,
        )
    });

    if let Some(sid) = &effective_session {
        if !is_session_lifecycle_tool(&tool_name) && cua_driver_core::session::is_session_ended(sid)
        {
            observe_daemon_error(observation, 1);
            return DaemonResponse::err(
                format!(
                    "session '{sid}' has ended; tool call '{tool_name}' was rejected. \
                     Call start_session with this id to revive it before issuing further \
                     actions, or use a new session id."
                ),
                1,
            );
        }
    }

    // Policy enforcement — defense-in-depth for direct daemon socket connections.
    // Evaluate before registry lookup so a deny-by-default policy does not leak
    // whether an unapproved name happens to be registered. This also preserves
    // the MCP policy contract now that every call passes through the daemon.
    if let Err(error) = cua_driver_core::authorization::authorize_tool_call(&tool_name, &args) {
        observe_daemon_error(observation, 1);
        return DaemonResponse::err(error.to_string(), 1);
    }

    if !known_tool {
        observe_daemon_error(observation, 64);
        return DaemonResponse::err(format!("Unknown tool: {tool_name}"), 64);
    }

    inject_browser_approvals(&tool_name, &mut args, req.session_id.as_deref());

    let session_context = observation_transport.and_then(|transport| {
        let transport = match transport {
            crate::telemetry::Transport::McpStdio => {
                cua_driver_core::session::SessionTransport::McpStdio
            }
            crate::telemetry::Transport::McpHttp => {
                cua_driver_core::session::SessionTransport::McpHttp
            }
            crate::telemetry::Transport::Cli => cua_driver_core::session::SessionTransport::Cli,
            crate::telemetry::Transport::Daemon => {
                cua_driver_core::session::SessionTransport::Daemon
            }
        };
        cua_driver_core::session::begin_tool_call(&tool_name, &args, true, transport)
    });

    let result = registry.invoke(&tool_name, args).await;
    let is_error = result.is_error.unwrap_or(false);
    let content: Vec<serde_json::Value> = result
        .content
        .iter()
        .map(|item| match item {
            cua_driver_core::protocol::Content::Text { text, .. } => {
                serde_json::json!({"type":"text","text":text})
            }
            cua_driver_core::protocol::Content::Image {
                data, mime_type, ..
            } => serde_json::json!({"type":"image","data":data,"mimeType":mime_type}),
        })
        .collect();
    let mut result_value = serde_json::json!({
        "content": content,
        "isError": is_error,
    });
    if let Some(structured) = result.structured_content {
        result_value["structuredContent"] = structured;
    }
    DaemonResponse::ok(observe_daemon_result(
        observation,
        session_context,
        result_value,
    ))
}

/// Read the PID stored in `pid_file_path`, if any.
pub fn read_pid_file(pid_file_path: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file_path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
}

// ── Server ────────────────────────────────────────────────────────────────────

#[cfg(unix)]
#[derive(Clone, Copy)]
struct SocketIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn socket_identity(socket_path: &str) -> anyhow::Result<SocketIdentity> {
    use std::os::unix::fs::MetadataExt as _;
    let metadata = std::fs::symlink_metadata(socket_path)?;
    Ok(SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(unix)]
fn remove_owned_socket(socket_path: &str, identity: SocketIdentity) {
    let Ok(current) = socket_identity(socket_path) else {
        return;
    };
    if current.device == identity.device && current.inode == identity.inode {
        let _ = std::fs::remove_file(socket_path);
    }
}

#[cfg(unix)]
fn secure_embedded_socket(socket_path: &str, embedded: bool) -> anyhow::Result<()> {
    if !embedded {
        return Ok(());
    }
    use std::os::unix::fs::PermissionsExt as _;
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(socket_path, permissions)
        .map_err(|e| anyhow::anyhow!("secure embedded daemon socket {socket_path}: {e}"))
}

#[cfg(unix)]
fn prepare_embedded_socket_path(socket_path: &str, embedded: bool) -> anyhow::Result<()> {
    if !embedded {
        // Preserve the legacy standalone stale-socket recovery behavior.
        let _ = std::fs::remove_file(socket_path);
        return Ok(());
    }
    match std::fs::symlink_metadata(socket_path) {
        Ok(_) => anyhow::bail!(
            "embedded daemon endpoint already exists at {socket_path}; the owning host must prove and remove its stale socket before spawn"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn daemon_metadata_response() -> DaemonResponse {
    DaemonResponse::ok(
        serde_json::to_value(cua_driver_core::daemon::current_daemon_metadata())
            .expect("daemon metadata is serializable"),
    )
}

async fn wait_for_parent_stdin_eof() {
    use tokio::io::AsyncReadExt as _;

    let mut stdin = tokio::io::stdin();
    let mut buffer = [0_u8; 64];
    loop {
        match stdin.read(&mut buffer).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

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

    cua_driver_core::authorization::validate_startup_authorization()?;

    // Create parent directory.
    if let Some(dir) = std::path::Path::new(socket_path).parent() {
        std::fs::create_dir_all(dir)?;
    }

    let embedded = cua_driver_core::embedded_mode();
    // Embedded endpoints are host-owned. The daemon must never unlink an
    // arbitrary pre-existing path between the host's safety check and bind.
    prepare_embedded_socket_path(socket_path, embedded)?;

    let listener =
        UnixListener::bind(socket_path).map_err(|e| anyhow::anyhow!("bind {socket_path}: {e}"))?;
    secure_embedded_socket(socket_path, embedded)?;
    let bound_socket = socket_identity(socket_path)?;

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
    let parent_liveness = async {
        if cua_driver_core::parent_liveness_stdin_enabled() {
            wait_for_parent_stdin_eof().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(parent_liveness);

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
                            "metadata" => {
                                let resp = daemon_metadata_response();
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
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
                                // name + concrete-schema resolver so daemon
                                // responses match the core MCP capability contract.
                                let tools: Vec<serde_json::Value> = reg.iter_defs()
                                    .map(|(name, def)| {
                                        let caps = cua_driver_core::tool::advertised_capabilities_for(
                                            name,
                                            &def.input_schema,
                                        );
                                        let risk = cua_driver_core::authorization::risk_metadata_json(name);
                                        serde_json::json!({
                                            "name": name,
                                            "description": def.description,
                                            "input_schema": def.input_schema,
                                            "read_only": def.read_only,
                                            "destructive": def.destructive,
                                            "idempotent": def.idempotent,
                                            "open_world": def.open_world,
                                            "capabilities": caps,
                                            "risk": risk,
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
                                    "schema_version": cua_driver_core::tool::TOOLS_LIST_SCHEMA_VERSION,
                                    "tool_observation_owner": "daemon",
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
                            "authorization_status" => {
                                let resp = DaemonResponse::ok(
                                    cua_driver_core::authorization::status_json()
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "revoke_authorization" => {
                                let args = req.args.as_ref().unwrap_or(&serde_json::Value::Null);
                                let all = args.get("all").and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                let session = args.get("session")
                                    .and_then(serde_json::Value::as_str)
                                    .filter(|value| !value.is_empty());
                                let result = if all && session.is_none() {
                                    let count = cua_driver_core::session::revoke_all_sessions();
                                    Ok(serde_json::json!({"revoked": count, "scope": "all"}))
                                } else if !all {
                                    session.map_or_else(
                                        || Err("revoke_authorization requires session or all=true"),
                                        |session| {
                                            cua_driver_core::session::end_session(session);
                                            Ok(serde_json::json!({"revoked": 1, "scope": "session"}))
                                        },
                                    )
                                } else {
                                    Err("revoke_authorization accepts exactly one of session or all=true")
                                };
                                let resp = match result {
                                    Ok(value) => DaemonResponse::ok(value),
                                    Err(error) => DaemonResponse::err(error, 64),
                                };
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "call" => {
                                // Recording idle-TTL liveness: any serviced tool
                                // call counts as activity (#1764).
                                last_activity.store(
                                    now_unix_secs(),
                                    std::sync::atomic::Ordering::Relaxed,
                                );
                                let resp = invoke_daemon_tool(&reg, req).await;
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
                                    active_proxy_sessions()
                                        .lock()
                                        .unwrap()
                                        .insert(sid.to_owned());
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
                        active_proxy_sessions().lock().unwrap().remove(&sid);
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
            _ = &mut parent_liveness => {
                eprintln!("Cua Driver embedded host closed its lifetime pipe; shutting down.");
                break;
            }
        }
    }

    // Do not unlink a replacement socket created after this listener was bound.
    remove_owned_socket(socket_path, bound_socket);
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
        Some(dir) => dir.join(crate::bundle::uia_executable_name()),
        None => return,
    };
    if !uia.exists() {
        tracing::debug!("uia spawn skipped: {} not present", uia.display());
        return;
    }
    let uia_str = uia.display().to_string();
    let cmd =
        format!("(New-Object -ComObject Shell.Application).ShellExecute('{uia_str}','','','',0)");
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

#[cfg(target_os = "windows")]
unsafe fn security_attrs_from_sddl(
    sddl: &str,
) -> Option<(SecurityAttributesRaw, *mut std::ffi::c_void)> {
    #[link(name = "advapi32")]
    extern "system" {
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            string_security_descriptor: *const u16,
            string_sd_revision: u32,
            security_descriptor: *mut *mut std::ffi::c_void,
            security_descriptor_size: *mut u32,
        ) -> i32;
    }
    let sddl: Vec<u16> = format!("{sddl}\0").encode_utf16().collect();
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

/// Return the current process token's user SID as an SDDL string. Embedded
/// named pipes use this identity instead of the standalone daemon's historical
/// Everyone DACL.
#[cfg(target_os = "windows")]
unsafe fn current_user_sid_string() -> Option<String> {
    #[repr(C)]
    struct SidAndAttributes {
        sid: *mut std::ffi::c_void,
        attributes: u32,
    }
    #[repr(C)]
    struct TokenUserRaw {
        user: SidAndAttributes,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            process: *mut std::ffi::c_void,
            desired_access: u32,
            token: *mut *mut std::ffi::c_void,
        ) -> i32;
        fn GetTokenInformation(
            token: *mut std::ffi::c_void,
            information_class: u32,
            information: *mut std::ffi::c_void,
            information_length: u32,
            return_length: *mut u32,
        ) -> i32;
        fn ConvertSidToStringSidW(sid: *mut std::ffi::c_void, string_sid: *mut *mut u16) -> i32;
    }

    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER_CLASS: u32 = 1;
    let mut token = std::ptr::null_mut();
    if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 || token.is_null() {
        return None;
    }
    let mut required = 0_u32;
    let _ = GetTokenInformation(
        token,
        TOKEN_USER_CLASS,
        std::ptr::null_mut(),
        0,
        &mut required,
    );
    if required == 0 {
        let _ = CloseHandle(token);
        return None;
    }
    let mut buffer = vec![0_u8; required as usize];
    let ok = GetTokenInformation(
        token,
        TOKEN_USER_CLASS,
        buffer.as_mut_ptr().cast(),
        required,
        &mut required,
    );
    let _ = CloseHandle(token);
    if ok == 0 {
        return None;
    }
    // GetTokenInformation writes into a byte buffer whose alignment is not
    // guaranteed to match TOKEN_USER. Copy the small header out rather than
    // creating a potentially unaligned reference into the buffer.
    let token_user = std::ptr::read_unaligned(buffer.as_ptr().cast::<TokenUserRaw>());
    let mut string_sid = std::ptr::null_mut();
    if ConvertSidToStringSidW(token_user.user.sid, &mut string_sid) == 0 || string_sid.is_null() {
        return None;
    }
    let length = (0..)
        .find(|&index| *string_sid.add(index) == 0)
        .unwrap_or(0);
    let sid = String::from_utf16_lossy(std::slice::from_raw_parts(string_sid, length));
    let _ = LocalFree(string_sid.cast());
    (!sid.is_empty()).then_some(sid)
}

/// Build the named-pipe ACL for one daemon mode.
///
/// Standalone preserves the existing cross-integrity Everyone descriptor used
/// by elevated installs. Embedded mode is private to the current user while
/// retaining the low mandatory label needed when host and daemon integrity
/// levels differ.
#[cfg(target_os = "windows")]
unsafe fn build_pipe_security_attrs(
    embedded: bool,
) -> Option<(SecurityAttributesRaw, *mut std::ffi::c_void)> {
    if embedded {
        let sid = current_user_sid_string()?;
        security_attrs_from_sddl(&format!("D:P(A;;GA;;;{sid})S:(ML;;NW;;;LW)"))
    } else {
        security_attrs_from_sddl("D:(A;OICI;GA;;;WD)S:(ML;;NW;;;LW)")
    }
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

    cua_driver_core::authorization::validate_startup_authorization()?;

    eprintln!("Cua Driver daemon listening on {socket_path}");

    // Build the mode-specific descriptor once and reuse it for every pipe
    // instance. Embedded mode fails closed if its current-user-only ACL cannot
    // be created; a default or Everyone ACL would violate the private-endpoint
    // contract promised by the host SDK.
    let embedded = cua_driver_core::embedded_mode();
    let security_attrs = unsafe { build_pipe_security_attrs(embedded) };
    if security_attrs.is_none() {
        if embedded {
            anyhow::bail!("failed to build current-user security descriptor for embedded pipe");
        }
        eprintln!(
            "cua-driver: failed to build cross-IL SECURITY_ATTRIBUTES; pipe will be \
             High-IL exclusive. CLI and MCP clients from Medium-IL processes \
             will be unable to reach the daemon."
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
    let parent_liveness = async {
        if cua_driver_core::parent_liveness_stdin_enabled() {
            wait_for_parent_stdin_eof().await;
        } else {
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(parent_liveness);

    // Idle-TTL recording backstop (#1764). See the unix branch above for the
    // full rationale; the leak (record_video via ffmpeg) is platform-independent.
    let last_activity = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(now_unix_secs()));
    spawn_recording_idle_backstop(registry.clone(), last_activity.clone());
    spawn_session_idle_sweep();
    if let Some(port) = crate::mcp_http::configured_port() {
        crate::mcp_http::spawn(registry.clone(), port);
    }
    register_recording_session_end_hook(registry.recording.clone());

    let mut first_pipe = true;
    loop {
        // Standalone daemons use the cross-IL descriptor when available;
        // embedded daemons use the current-user descriptor and reserve the
        // pipe name with their first instance.
        let first_pipe_instance = embedded && first_pipe;
        let server = if sec_attrs_ptr.is_null() {
            ServerOptions::new()
                .first_pipe_instance(first_pipe_instance)
                .create(socket_path)
                .map_err(|e| anyhow::anyhow!("create named pipe {socket_path}: {e}"))?
        } else {
            unsafe {
                ServerOptions::new()
                    .first_pipe_instance(first_pipe_instance)
                    .create_with_security_attributes_raw(socket_path, sec_attrs_ptr)
                    .map_err(|e| anyhow::anyhow!("create named pipe {socket_path}: {e}"))?
            }
        };
        first_pipe = false;

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
                            "metadata" => {
                                let resp = daemon_metadata_response();
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
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
                                        let caps = cua_driver_core::tool::advertised_capabilities_for(
                                            name,
                                            &def.input_schema,
                                        );
                                        let risk = cua_driver_core::authorization::risk_metadata_json(name);
                                        serde_json::json!({
                                            "name": name,
                                            "description": def.description,
                                            "input_schema": def.input_schema,
                                            "read_only": def.read_only,
                                            "destructive": def.destructive,
                                            "idempotent": def.idempotent,
                                            "open_world": def.open_world,
                                            "capabilities": caps,
                                            "risk": risk,
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
                                    "schema_version": cua_driver_core::tool::TOOLS_LIST_SCHEMA_VERSION,
                                    "tool_observation_owner": "daemon",
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
                            "authorization_status" => {
                                let resp = DaemonResponse::ok(
                                    cua_driver_core::authorization::status_json()
                                );
                                let _ = writer.write_all(
                                    (serde_json::to_string(&resp).unwrap() + "\n").as_bytes()
                                ).await;
                            }
                            "revoke_authorization" => {
                                let args = req.args.as_ref().unwrap_or(&serde_json::Value::Null);
                                let all = args.get("all").and_then(serde_json::Value::as_bool)
                                    .unwrap_or(false);
                                let session = args.get("session")
                                    .and_then(serde_json::Value::as_str)
                                    .filter(|value| !value.is_empty());
                                let result = if all && session.is_none() {
                                    let count = cua_driver_core::session::revoke_all_sessions();
                                    Ok(serde_json::json!({"revoked": count, "scope": "all"}))
                                } else if !all {
                                    session.map_or_else(
                                        || Err("revoke_authorization requires session or all=true"),
                                        |session| {
                                            cua_driver_core::session::end_session(session);
                                            Ok(serde_json::json!({"revoked": 1, "scope": "session"}))
                                        },
                                    )
                                } else {
                                    Err("revoke_authorization accepts exactly one of session or all=true")
                                };
                                let resp = match result {
                                    Ok(value) => DaemonResponse::ok(value),
                                    Err(error) => DaemonResponse::err(error, 64),
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
                                let resp = invoke_daemon_tool(&reg, req).await;
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
                                    active_proxy_sessions()
                                        .lock()
                                        .unwrap()
                                        .insert(sid.to_owned());
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
                        active_proxy_sessions().lock().unwrap().remove(&sid);
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
            _ = &mut parent_liveness => {
                eprintln!("Cua Driver embedded host closed its lifetime pipe; shutting down.");
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
        let pid_hint = pid_file_path
            .as_deref()
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
        if matches!(platform_windows::diagnostics::current_session_id(), Some(0),) {
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

    if let Err(e) = rt.block_on(run_serve(registry, &socket_path, pid_file_path.as_deref())) {
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

    let req = DaemonRequest {
        method: "shutdown".into(),
        name: None,
        args: None,
        session_id: None,
        observation_origin: None,
    };
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
        let request = DaemonRequest {
            method: "authorization_status".to_owned(),
            name: None,
            args: None,
            session_id: None,
            observation_origin: Some(ToolObservationOrigin::Direct),
        };
        if let Ok(response) = send_request(socket_path, &request) {
            if let Some(status) = response.result {
                println!(
                    "  permission mode: {} ({})",
                    status["permission_mode"].as_str().unwrap_or("invalid"),
                    status["permission_mode_source"]
                        .as_str()
                        .unwrap_or("unknown source")
                );
                println!(
                    "  user policy: configured={}, active={}, valid={}",
                    status["user_policy_configured"].as_bool().unwrap_or(false),
                    status["user_policy_active"].as_bool().unwrap_or(false),
                    status["user_policy_valid"].as_bool().unwrap_or(false),
                );
                if let Some(hash) = status["user_policy_sha256"].as_str() {
                    println!("  user policy sha256: {hash}");
                }
                println!(
                    "  managed policy: configured={}, active={}, valid={}",
                    status["managed_policy_configured"]
                        .as_bool()
                        .unwrap_or(false),
                    status["managed_policy_active"].as_bool().unwrap_or(false),
                    status["managed_policy_valid"].as_bool().unwrap_or(false),
                );
                if let Some(hash) = status["managed_policy_sha256"].as_str() {
                    println!("  managed policy sha256: {hash}");
                }
                println!(
                    "  protected consent collector: {}",
                    status["protected_consent_collector"]
                        .as_str()
                        .unwrap_or("unavailable")
                );
                println!(
                    "  session policy: configured={}, approved_at_startup={}, valid={}",
                    status["session_policy_configured"]
                        .as_bool()
                        .unwrap_or(false),
                    status["session_policy_approved_at_startup"]
                        .as_bool()
                        .unwrap_or(false),
                    status["session_policy_valid"].as_bool().unwrap_or(false),
                );
                if let Some(manifest) = status["session_policy"].as_object() {
                    println!(
                        "  session policy sha256: {}",
                        manifest
                            .get("sha256")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("unknown")
                    );
                }
            }
        }
    } else {
        eprintln!("Cua Driver daemon is not running");
        std::process::exit(1);
    }
}

/// Revoke one authorization/session scope or every live scope. This local
/// control is intentionally deny-only and never accepts a grant token.
pub fn run_revoke_cmd(socket_path: &str, session: Option<&str>, all: bool) {
    let request = DaemonRequest {
        method: "revoke_authorization".to_owned(),
        name: None,
        args: Some(if all {
            serde_json::json!({"all": true})
        } else {
            serde_json::json!({"session": session})
        }),
        session_id: None,
        observation_origin: Some(ToolObservationOrigin::Direct),
    };
    match send_request(socket_path, &request) {
        Ok(response) if response.ok => {
            let result = response.result.unwrap_or_default();
            println!(
                "Revoked {} authorization scope(s).",
                result["revoked"].as_u64().unwrap_or(0)
            );
        }
        Ok(response) => {
            eprintln!(
                "{}",
                response
                    .error
                    .unwrap_or_else(|| "authorization revocation failed".to_owned())
            );
            std::process::exit(response.exit_code.unwrap_or(1));
        }
        Err(error) => {
            eprintln!("authorization revocation failed: {error}");
            std::process::exit(1);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(all(test, unix))]
mod socket_tests {
    use super::{remove_owned_socket, secure_embedded_socket, socket_identity};
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn only_embedded_sockets_are_forced_private() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("driver.sock");
        let _listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o770)).unwrap();

        secure_embedded_socket(socket.to_str().unwrap(), false).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o770
        );

        secure_embedded_socket(socket.to_str().unwrap(), true).unwrap();
        assert_eq!(
            std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn cleanup_preserves_a_replacement_socket() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("driver.sock");
        let first = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let first_identity = socket_identity(socket.to_str().unwrap()).unwrap();
        std::fs::remove_file(&socket).unwrap();
        let _replacement = std::os::unix::net::UnixListener::bind(&socket).unwrap();

        remove_owned_socket(socket.to_str().unwrap(), first_identity);

        assert!(socket.exists());
        drop(first);
    }
}

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
            observation_origin: None,
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
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket1, &call_req(Some(&s1))))
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
            observation_origin: None,
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
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket3, &call_req(Some(&s3))))
                .await
                .unwrap()
                .expect("ended call response");
        assert!(
            !resp.ok,
            "ended-session call must be rejected loudly (not ok)"
        );
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
            // Session policy is caller-declared. The transport session id is
            // cleanup/ownership metadata and must not grant policy authority.
            args: Some(serde_json::json!({ "session": s3b.clone() })),
            session_id: Some(s3b),
            observation_origin: None,
        };
        let resp = tokio::task::spawn_blocking(move || send_request(&socket3b, &start))
            .await
            .unwrap()
            .expect("start_session response");
        assert!(
            resp.ok,
            "start_session must run even for an ended id (lifecycle-exempt)"
        );

        // 3c. A call on the revived id now RUNS again.
        let socket3c = socket.clone();
        let s3c = sid.to_owned();
        let resp =
            tokio::task::spawn_blocking(move || send_request(&socket3c, &call_req(Some(&s3c))))
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
            observation_origin: None,
        };
        let _ = tokio::task::spawn_blocking(move || send_request(&socket5, &shutdown)).await;
        let _ = server.await;
        let _ = std::fs::remove_file(&socket);
    }
}

#[cfg(test)]
mod telemetry_routing_tests {
    use super::*;

    fn request(origin: Option<ToolObservationOrigin>) -> DaemonRequest {
        DaemonRequest {
            method: "call".into(),
            name: Some("browser_click".into()),
            args: Some(serde_json::json!({})),
            session_id: Some("bounded-session".into()),
            observation_origin: origin,
        }
    }

    #[test]
    fn observation_origin_selects_exactly_one_transport() {
        assert_eq!(
            daemon_observation_transport(&request(Some(ToolObservationOrigin::McpProxy))),
            Some(crate::telemetry::Transport::McpStdio)
        );
        assert_eq!(
            daemon_observation_transport(&request(Some(ToolObservationOrigin::Direct))),
            Some(crate::telemetry::Transport::Daemon)
        );
        assert_eq!(daemon_observation_transport(&request(None)), None);
    }

    #[test]
    fn observation_origin_is_additive_and_backward_compatible() {
        let legacy: DaemonRequest = serde_json::from_value(serde_json::json!({
            "method": "call",
            "name": "click",
            "args": {},
            "session_id": "bounded-session"
        }))
        .unwrap();
        assert_eq!(legacy.observation_origin, None);

        let current = serde_json::to_value(request(Some(ToolObservationOrigin::McpProxy))).unwrap();
        assert_eq!(current["observation_origin"], "mcp_proxy");
    }
}

#[cfg(test)]
mod session_boundary_tests {
    use super::{active_proxy_sessions, apply_session_identity, inject_browser_approvals};
    use cua_driver_core::browser::approval::MCP_HOST_APPROVAL_ARG;
    use cua_driver_core::browser::download::MCP_HOST_DOWNLOAD_APPROVAL_ARG;
    use serde_json::json;

    #[test]
    fn explicit_session_becomes_session_id_and_is_returned() {
        let mut args = json!({ "x": 1, "session": "research-1" });
        let eff = apply_session_identity(&mut args, &None);
        assert_eq!(eff.as_deref(), Some("research-1"));
        assert_eq!(args["_session_id"], "research-1");
        assert!(args.get("_transport_session_id").is_none());
    }

    #[test]
    fn public_and_transport_sessions_remain_independent() {
        let mut args = json!({ "session": "capability-session" });
        let eff = apply_session_identity(&mut args, &Some("proxy-session".to_owned()));
        assert_eq!(eff.as_deref(), Some("capability-session"));
        assert_eq!(args["_session_id"], "capability-session");
        assert_eq!(args["_transport_session_id"], "proxy-session");
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
        assert_eq!(args["_transport_session_id"], "mcp-123");
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
    fn caller_set_session_id_is_replaced_by_minted() {
        let mut args = json!({ "_session_id": "caller-set" });
        let eff = apply_session_identity(&mut args, &Some("mcp-999".to_owned()));
        assert_eq!(args["_session_id"], "mcp-999");
        assert_eq!(args["_transport_session_id"], "mcp-999");
        assert_eq!(eff.as_deref(), Some("mcp-999"));
    }

    #[test]
    fn browser_prepare_approval_requires_a_live_proxy_session() {
        let session = "approval-boundary-test";
        let mut raw_args = json!({"pid": 42});
        inject_browser_approvals("browser_prepare", &mut raw_args, Some(session));
        assert!(raw_args.get(MCP_HOST_APPROVAL_ARG).is_none());

        active_proxy_sessions()
            .lock()
            .unwrap()
            .insert(session.to_owned());
        let mut proxy_args = json!({"pid": 42});
        inject_browser_approvals("browser_prepare", &mut proxy_args, Some(session));
        active_proxy_sessions().lock().unwrap().remove(session);
        assert_eq!(proxy_args[MCP_HOST_APPROVAL_ARG], true);

        let mut other_tool = json!({"pid": 42});
        active_proxy_sessions()
            .lock()
            .unwrap()
            .insert(session.to_owned());
        inject_browser_approvals("get_browser_state", &mut other_tool, Some(session));
        active_proxy_sessions().lock().unwrap().remove(session);
        assert!(other_tool.get(MCP_HOST_APPROVAL_ARG).is_none());

        let mut raw_download = json!({"destination_root": "/private/path"});
        inject_browser_approvals("browser_download", &mut raw_download, Some(session));
        assert!(raw_download.get(MCP_HOST_DOWNLOAD_APPROVAL_ARG).is_none());

        active_proxy_sessions()
            .lock()
            .unwrap()
            .insert(session.to_owned());
        let mut proxy_download = json!({"destination_root": "/private/path"});
        inject_browser_approvals("browser_download", &mut proxy_download, Some(session));
        active_proxy_sessions().lock().unwrap().remove(session);
        assert_eq!(proxy_download[MCP_HOST_DOWNLOAD_APPROVAL_ARG], true);
    }
}
