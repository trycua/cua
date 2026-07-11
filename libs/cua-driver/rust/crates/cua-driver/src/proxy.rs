//! Stdio MCP proxy that forwards `tools/list` and `tools/call` through
//! the running `cua-driver-rs serve` daemon over its Unix socket.
//!
//! This is the runtime half of the TCC auto-relaunch path (issue #1525,
//! mirror of Swift PR #1479). When `cua-driver-rs mcp` is invoked from
//! an IDE terminal — Claude Code, Cursor, VS Code, Warp — macOS TCC
//! attributes the process to the calling terminal, not to
//! `CuaDriver.app`. The MCP client side sees a normal stdio server,
//! but every AX probe silently fails because the binary is running
//! against the wrong bundle id.
//!
//! The fix: detect that context (see `crate::bundle`), ensure a daemon
//! is running under `LaunchServices` (which gives it the right TCC
//! attribution), then proxy every MCP request through the daemon's
//! socket. The MCP client never sees the redirection — same JSON-RPC
//! envelope, same tool semantics.
//!
//! Why this lives in `cua-driver` and not `mcp-server`:
//!   `cua_driver_core::server::run` already speaks JSON-RPC over stdio
//!   against an in-process `ToolRegistry`. The proxy speaks the same
//!   protocol on the client side but the server side is the daemon's
//!   line-delimited JSON UDS protocol, owned by `crate::serve`.
//!   Putting the proxy here avoids `mcp-server → cua-driver` reverse
//!   coupling.

use std::sync::Arc;

use cua_driver_core::protocol::{initialize_result, Request, Response};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::serve::{
    is_daemon_listening, send_request, DaemonProfile, DaemonRequest,
    DaemonResponse,
    CODEX_COMPUTER_USE_TOOL_NAMES,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum ControlConnectionState {
    Connecting,
    Ready {
        approval_broker_token: Option<String>,
    },
    Rejected(String),
}

/// Run the MCP stdio proxy. Reads JSON-RPC lines from stdin, forwards
/// the body of each `tools/list` / `tools/call` to the daemon at
/// `socket_path`, and writes the daemon's response back as a proper
/// JSON-RPC envelope.
///
/// Mirrors `cua_driver_core::server::run`'s control flow exactly — same
/// EOF + parse-error + notification handling — only the per-method
/// branches change.
///
/// Fails fast if the daemon isn't reachable, so MCP clients see a
/// clear startup error instead of a "successful" handshake that
/// advertises zero tools and then errors on every call. Matches
/// Swift `makeProxy`'s `fetchProxyToolList` pre-check.
pub async fn run_proxy(
    socket_path: String,
    expected_profile: DaemonProfile,
) -> anyhow::Result<()> {
    if !is_daemon_listening(&socket_path) {
        anyhow::bail!(
            "cua-driver-rs daemon not reachable on {socket_path}. Start it \
             with `open -n -g -a CuaDriver --args serve` and retry."
        );
    }

    // Mint this MCP session's identity once at proxy startup. One proxy process
    // == one MCP session; the daemon outlives it. We stamp this id on every
    // forwarded request so the daemon can OWN and CLEAN UP this session's
    // state (recording, config overrides) and tear it down on disconnect via
    // a `session_end` signal. Dep-free `pid + start-nanos` is sufficient for
    // daemon-local uniqueness over this proxy's lifetime (no `uuid` crate dep
    // for one mint).
    let session_id = mint_session_id();
    debug!(session_id = %session_id, "proxy session minted");

    // Open ONE long-lived "control" connection to the daemon and hold it open
    // for this proxy's entire lifetime (separate from the per-call connections
    // that `send_request` opens and closes per tool call). It sends a single
    // `session_begin` line and then parks reading — it never writes again and
    // never closes until this process dies.
    //
    // This is the reaper: when the proxy exits (graceful stdin EOF) OR is
    // SIGKILLed/crashes, the kernel closes this socket; the daemon's
    // per-connection reader hits EOF and fires `session_end` for `session_id`,
    // tearing down every piece of state this session owns (overlay cursor,
    // config overrides, recording). Liveness is connection-based, so an
    // alive-but-idle session — one issuing zero tool calls — is never reaped:
    // its control connection stays parked open.
    //
    // Detached + fire-and-forget. If the connect races daemon startup and
    // fails, we log and continue — the per-call `send_request` has its own
    // retry/timeout, and a restarted daemon loses session state anyway, so a
    // missing control connection only degrades to no-reaper (the recording
    // idle-TTL still backstops a leaked recording). It must NOT bail the proxy.
    let (control_ready_tx, control_ready_rx) =
        tokio::sync::watch::channel(ControlConnectionState::Connecting);
    {
        let socket = socket_path.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            run_control_connection(
                socket,
                sid,
                expected_profile,
                control_ready_tx,
            )
            .await;
        });
    }

    let _ = wait_for_control_connection(control_ready_rx.clone()).await?;

    // Cache the tool list once at startup. The daemon's registry is
    // static for the lifetime of the daemon, so polling on every
    // `tools/list` would waste a round-trip per call. Swift does the
    // same caching in `fetchProxyToolList`.
    let cached_tools_list =
        fetch_tools_list_from_daemon(&socket_path, &session_id, expected_profile)?;
    let cached_tools_list = Arc::new(cached_tools_list);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();
    let mut client_supports_elicitation = false;

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF — MCP client disconnected (stdin closed).
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ proxy request");

        let response = match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Response::parse_error()
            }
            Ok(req) if req.is_notification() => {
                // Notifications get dropped, same as `server::run`.
                continue;
            }
            Ok(req) => {
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                if req.method == "initialize" {
                    client_supports_elicitation = supports_elicitation(&req);
                }
                if req.method == "tools/call"
                    && expected_profile == DaemonProfile::CodexComputerUseCompat
                {
                    match req.tool_call() {
                        Err(error) => Response::error(
                            id,
                            -32602,
                            format!("Invalid params: {error}"),
                        ),
                        Ok(call) => {
                            forward_tool_call_with_approval(
                                id,
                                call.name,
                                call.args,
                                &socket_path,
                                &session_id,
                                &control_ready_rx,
                                client_supports_elicitation,
                                &mut reader,
                                &mut writer,
                            )
                            .await
                        }
                    }
                } else {
                    handle_proxy_request(
                        req,
                        id,
                        &socket_path,
                        &cached_tools_list,
                        &session_id,
                        &control_ready_rx,
                    )
                    .await
                }
            }
        };

        let serialized = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#
            )
        });
        debug!(raw = %serialized, "← proxy response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    // Reached on a clean stdin EOF (the `n == 0` break above) — the normal
    // "MCP client disconnected" seam. Session teardown is NO LONGER done here:
    // it's fully subsumed by the persistent control connection spawned at
    // startup. On any proxy exit — graceful stdin EOF (this path), an I/O
    // error propagated via `?`, OR a SIGKILL/crash — the kernel closes the
    // control socket, the daemon's reader hits EOF, and it fires
    // `session_end(session_id)` once (idempotent). That single path reliably
    // covers the ungraceful-death case the old best-effort exit hook missed.
    Ok(())
}

/// Own the proxy's single long-lived control connection. Connects directly to
/// the daemon socket (its OWN async open — `send_request` is sync, blocking,
/// and one-shot, so it cannot be reused here), sends one `session_begin` line
/// carrying `session_id`, then parks in a read loop until the connection
/// closes. It never writes again. The daemon records `session_id` from
/// `session_begin` and fires `session_end` when this connection EOFs — which
/// the kernel triggers on proxy exit AND on kill -9.
///
/// When the daemon closes the connection during a permission-triggered re-exec,
/// reconnect with the same session identity and send `session_begin` again.
/// The old daemon reaps the old connection before it exits; the replacement
/// daemon then owns cleanup for the reconnected session. The task ends only
/// when the proxy runtime drops it, which also closes the current connection
/// and triggers the replacement daemon's EOF cleanup.
async fn run_control_connection(
    socket_path: String,
    session_id: String,
    expected_profile: DaemonProfile,
    readiness: tokio::sync::watch::Sender<ControlConnectionState>,
) {
    let begin = DaemonRequest {
        method: "session_begin".into(),
        name: None,
        args: (expected_profile == DaemonProfile::CodexComputerUseCompat)
            .then(|| serde_json::json!({"approval_broker": true})),
        session_id: Some(session_id.clone()),
    };
    let line = match serde_json::to_string(&begin) {
        Ok(s) => s + "\n",
        Err(e) => {
            warn!("control connection: serialize session_begin failed: {e}");
            return;
        }
    };

    #[cfg(unix)]
    {
        loop {
            let connected = match run_unix_control_connection_once(
                &socket_path,
                &session_id,
                &line,
                expected_profile,
                &readiness,
            )
            .await
            {
                Ok(connected) => connected,
                Err(error) => {
                    let _ = readiness.send(ControlConnectionState::Rejected(error));
                    return;
                }
            };
            debug!(
                session_id = %session_id,
                connected,
                "control connection unavailable; waiting to reconnect"
            );
            tokio::time::sleep(std::time::Duration::from_millis(
                if connected { 50 } else { 250 },
            ))
            .await;
        }
    }

    #[cfg(all(not(unix), target_os = "windows"))]
    {
        loop {
            let connected = match run_windows_control_connection_once(
                &socket_path,
                &session_id,
                &line,
                expected_profile,
                &readiness,
            )
            .await
            {
                Ok(connected) => connected,
                Err(error) => {
                    let _ = readiness.send(ControlConnectionState::Rejected(error));
                    return;
                }
            };
            debug!(
                session_id = %session_id,
                connected,
                "control connection unavailable; waiting to reconnect"
            );
            tokio::time::sleep(std::time::Duration::from_millis(
                if connected { 50 } else { 250 },
            ))
            .await;
        }
    }

    #[cfg(all(not(unix), not(target_os = "windows")))]
    {
        let _ = (line, session_id, socket_path, expected_profile, readiness);
    }
}

async fn wait_for_control_connection(
    mut readiness: tokio::sync::watch::Receiver<ControlConnectionState>,
) -> anyhow::Result<Option<String>> {
    tokio::time::timeout(std::time::Duration::from_secs(10), async move {
        loop {
            match readiness.borrow().clone() {
                ControlConnectionState::Ready {
                    approval_broker_token,
                } => return Ok(approval_broker_token),
                ControlConnectionState::Rejected(error) => anyhow::bail!(error),
                ControlConnectionState::Connecting => {}
            }
            readiness.changed().await.map_err(|_| {
                anyhow::anyhow!("daemon control-connection task stopped unexpectedly")
            })?;
        }
    })
    .await
    .map_err(|_| {
        anyhow::anyhow!(
            "timed out waiting for the daemon session control connection"
        )
    })?
}

fn validate_control_ack(
    line: &str,
    expected_profile: DaemonProfile,
) -> anyhow::Result<Option<String>> {
    let response: DaemonResponse = serde_json::from_str(line)
        .map_err(|error| anyhow::anyhow!("decode session_begin response: {error}"))?;
    if !response.ok {
        anyhow::bail!(
            "daemon rejected session_begin: {}",
            response
                .error
                .unwrap_or_else(|| "unknown daemon error".to_owned())
        );
    }
    let reported_profile = response
        .result
        .as_ref()
        .and_then(|result| result.get("profile"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("session_begin ACK did not report a daemon profile"))?;
    if reported_profile != expected_profile.as_str() {
        anyhow::bail!(
            "daemon profile mismatch: MCP requested `{expected_profile}`, but the daemon \
             reports `{reported_profile}`"
        );
    }
    let approval_broker_token = response
        .result
        .as_ref()
        .and_then(|result| result.get("approval_broker_token"))
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(str::to_owned);
    if expected_profile == DaemonProfile::CodexComputerUseCompat
        && approval_broker_token.is_none()
    {
        anyhow::bail!(
            "session_begin ACK did not include an authenticated app approval broker token"
        );
    }
    Ok(approval_broker_token)
}

#[cfg(unix)]
async fn run_unix_control_connection_once(
    socket_path: &str,
    session_id: &str,
    begin_line: &str,
    expected_profile: DaemonProfile,
    readiness: &tokio::sync::watch::Sender<ControlConnectionState>,
) -> Result<bool, String> {
    use tokio::net::UnixStream;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut stream = loop {
        match UnixStream::connect(socket_path).await {
            Ok(stream) => break stream,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => {
                debug!(session_id, "control connect failed while daemon restarts: {error}");
                return Ok(false);
            }
        }
    };
    if let Err(error) = stream.write_all(begin_line.as_bytes()).await {
        debug!(session_id, "control connection: write session_begin failed: {error}");
        return Ok(true);
    }
    let _ = stream.flush().await;
    let mut reader = BufReader::new(stream);
    let mut buffer = String::new();
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        reader.read_line(&mut buffer),
    )
    .await
    {
        Ok(Ok(bytes)) if bytes > 0 => {}
        Ok(Ok(_)) => {
            debug!(session_id, "control connection closed before session_begin ACK");
            return Ok(true);
        }
        Ok(Err(error)) => {
            debug!(session_id, "control connection ACK read failed: {error}");
            return Ok(true);
        }
        Err(_) => {
            debug!(session_id, "control connection timed out waiting for session_begin ACK");
            return Ok(true);
        }
    }
    let approval_broker_token = validate_control_ack(buffer.trim(), expected_profile)
        .map_err(|error| error.to_string())?;
    let _ = readiness.send(ControlConnectionState::Ready {
        approval_broker_token,
    });
    debug!(session_id, "control connection established (session_begin acknowledged)");

    loop {
        buffer.clear();
        match reader.read_line(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => continue,
        }
    }
    let _ = readiness.send(ControlConnectionState::Connecting);
    Ok(true)
}

#[cfg(all(not(unix), target_os = "windows"))]
async fn run_windows_control_connection_once(
    socket_path: &str,
    session_id: &str,
    begin_line: &str,
    expected_profile: DaemonProfile,
    readiness: &tokio::sync::watch::Sender<ControlConnectionState>,
) -> Result<bool, String> {
    use tokio::net::windows::named_pipe::ClientOptions;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut client = loop {
        match ClientOptions::new().open(socket_path) {
            Ok(client) => break client,
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => {
                debug!(session_id, "control pipe open failed while daemon restarts: {error}");
                return Ok(false);
            }
        }
    };
    if let Err(error) = client.write_all(begin_line.as_bytes()).await {
        debug!(session_id, "control connection: write session_begin failed: {error}");
        return Ok(true);
    }
    let _ = client.flush().await;
    let mut reader = BufReader::new(client);
    let mut buffer = String::new();
    match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        reader.read_line(&mut buffer),
    )
    .await
    {
        Ok(Ok(bytes)) if bytes > 0 => {}
        Ok(Ok(_)) => {
            debug!(session_id, "control connection closed before session_begin ACK");
            return Ok(true);
        }
        Ok(Err(error)) => {
            debug!(session_id, "control connection ACK read failed: {error}");
            return Ok(true);
        }
        Err(_) => {
            debug!(session_id, "control connection timed out waiting for session_begin ACK");
            return Ok(true);
        }
    }
    let approval_broker_token = validate_control_ack(buffer.trim(), expected_profile)
        .map_err(|error| error.to_string())?;
    let _ = readiness.send(ControlConnectionState::Ready {
        approval_broker_token,
    });
    debug!(session_id, "control connection established (session_begin acknowledged)");

    loop {
        buffer.clear();
        match reader.read_line(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(_) => continue,
        }
    }
    let _ = readiness.send(ControlConnectionState::Connecting);
    Ok(true)
}

/// Mint a session id unique among the live proxies sharing one daemon, for the
/// lifetime of this proxy process. `pid + process-start nanos` is dep-free and
/// sufficient: two proxies can't share a pid concurrently, and the nanos guard
/// disambiguates pid reuse across the daemon's lifetime. We deliberately avoid
/// the `uuid` crate — a single v4 mint isn't worth a new dependency.
fn mint_session_id() -> String {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("mcp-{pid}-{nanos}")
}

fn validate_daemon_profile_and_roster(
    result: &serde_json::Value,
    expected_profile: DaemonProfile,
    socket_path: &str,
) -> anyhow::Result<()> {
    let reported_profile = result
        .get("profile")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "daemon on {socket_path} did not report a tool profile. Stop that daemon and \
                 restart it with the current cua-driver binary before retrying."
            )
        })?;
    if reported_profile != expected_profile.as_str() {
        anyhow::bail!(
            "daemon profile mismatch on {socket_path}: MCP requested `{expected_profile}`, \
             but the daemon reports `{reported_profile}`. Stop it and restart the matching \
             `cua-driver serve{}` profile.",
            if expected_profile == DaemonProfile::CodexComputerUseCompat {
                " --codex-computer-use-compat"
            } else {
                ""
            }
        );
    }

    if expected_profile == DaemonProfile::CodexComputerUseCompat {
        let tools = result
            .get("tools")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("daemon list response missing `tools` array"))?;
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(serde_json::Value::as_str))
            .collect();
        if names.as_slice() != CODEX_COMPUTER_USE_TOOL_NAMES {
            anyhow::bail!(
                "daemon on {socket_path} reports the Codex Computer Use profile, but its tool \
                 roster is invalid: expected {:?}, got {:?}. Restart it with the current \
                 cua-driver binary.",
                CODEX_COMPUTER_USE_TOOL_NAMES,
                names
            );
        }
    }
    Ok(())
}

/// One-shot daemon `list` over the UDS, reshaped into a MCP
/// `tools/list` result. The daemon now returns the full ToolDef
/// (`name`, `description`, `input_schema`, annotation hints) per
/// commit 3's `serve.rs` change.
fn fetch_tools_list_from_daemon(
    socket_path: &str,
    session_id: &str,
    expected_profile: DaemonProfile,
) -> anyhow::Result<serde_json::Value> {
    let req = DaemonRequest {
        method: "list".into(),
        name: None,
        args: None,
        session_id: Some(session_id.to_owned()),
    };
    let resp = send_request(socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(
            "daemon refused tool list on {socket_path}: {}",
            resp.error.unwrap_or_else(|| "(no error message)".into())
        );
    }
    let result = resp.result.ok_or_else(|| {
        anyhow::anyhow!("daemon list response missing `result` field")
    })?;
    validate_daemon_profile_and_roster(&result, expected_profile, socket_path)?;
    let tools_array = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!("daemon list response missing `tools` array")
        })?;

    // Reshape the daemon's `{name, description, input_schema, read_only,
    // ..., capabilities}` envelope into MCP's `{name, description,
    // inputSchema, annotations: {...}, capabilities}` shape. Same
    // translation `ToolDef::to_list_entry` does for the in-process
    // path so MCP clients see identical tools/list output either way.
    //
    // `capabilities` is passed through verbatim when the daemon
    // provides it; older daemons that don't emit the field fall back
    // to a name-keyed lookup so the proxy still surfaces capability
    // metadata without an extra round-trip.
    let mcp_tools: Vec<serde_json::Value> = tools_array
        .iter()
        .map(|t| {
            let name = t.get("name").cloned().unwrap_or(serde_json::Value::Null);
            let description = t
                .get("description")
                .cloned()
                .unwrap_or(serde_json::Value::String(String::new()));
            let input_schema = t.get("input_schema").cloned().unwrap_or_else(
                || serde_json::json!({"type": "object", "properties": {}}),
            );
            let read_only = t.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false);
            let destructive =
                t.get("destructive").and_then(|v| v.as_bool()).unwrap_or(false);
            let idempotent =
                t.get("idempotent").and_then(|v| v.as_bool()).unwrap_or(false);
            let open_world =
                t.get("open_world").and_then(|v| v.as_bool()).unwrap_or(false);
            let capabilities = t.get("capabilities")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_else(|| {
                    // Fallback: derive from the centralised map by
                    // name. Keeps the proxy compatible with daemon
                    // builds that pre-date the capabilities field.
                    name.as_str()
                        .map(cua_driver_core::tool::default_capabilities_for)
                        .unwrap_or_default()
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect()
                });
            serde_json::json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
                "annotations": {
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": idempotent,
                    "openWorldHint": open_world,
                },
                "capabilities": capabilities,
            })
        })
        .collect();

    // `capability_version` and `schema_version` are passed through
    // when the daemon emits them; older daemons fall back to the
    // proxy's compiled-in `CAPABILITY_VERSION` so MCP clients always
    // see the envelope keys.
    let capability_version = result
        .get("capability_version")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String(
            cua_driver_core::tool::CAPABILITY_VERSION.to_owned()
        ));
    let schema_version = result
        .get("schema_version")
        .cloned()
        .unwrap_or_else(|| serde_json::Value::String("1".to_owned()));

    Ok(serde_json::json!({
        "tools": mcp_tools,
        "capability_version": capability_version,
        "schema_version": schema_version,
    }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AppApprovalChallenge {
    challenge: String,
    app_identifier: String,
    display_name: String,
    allow_persistent_approval: bool,
}

impl AppApprovalChallenge {
    fn from_daemon_response(response: &DaemonResponse) -> Option<Self> {
        let structured = response
            .result
            .as_ref()?
            .get("structuredContent")?;
        if structured.get("code")?.as_str()? != "app_approval_required" {
            return None;
        }
        let approval = structured.get("approval")?;
        Some(Self {
            challenge: approval.get("challenge")?.as_str()?.to_owned(),
            app_identifier: approval.get("app")?.as_str()?.to_owned(),
            display_name: approval.get("displayName")?.as_str()?.to_owned(),
            allow_persistent_approval: approval
                .get("allowPersistentApproval")
                .and_then(serde_json::Value::as_bool)
                .or_else(|| {
                    approval
                        .get("allow_persistent_approval")
                        .and_then(serde_json::Value::as_bool)
                })
                .unwrap_or(false),
        })
    }

    fn elicitation_request(&self, request_id: &str) -> serde_json::Value {
        let persist = if self.allow_persistent_approval {
            serde_json::json!(["session", "always"])
        } else {
            serde_json::json!(["session"])
        };
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "elicitation/create",
            "params": {
                "message": format!(
                    "Allow Computer Use to use \"{}\"?",
                    self.display_name
                ),
                "requestedSchema": {
                    "type": "object",
                    "properties": {},
                },
                "_meta": {
                    "codex_approval_kind": "mcp_tool_call",
                    "connector_id": "computer-use",
                    "connector_name": "Computer Use",
                    "persist": persist,
                    "riskLevel": "low",
                    "tool_params": {"app": self.app_identifier},
                    "tool_params_display": [{
                        "name": "app",
                        "display_name": "App",
                        "value": self.display_name,
                    }],
                },
            },
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ElicitationAction {
    Accept,
    Decline,
    Cancel,
}

impl ElicitationAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Decline => "decline",
            Self::Cancel => "cancel",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ElicitationDecision {
    action: ElicitationAction,
    persistence: Option<String>,
}

fn supports_elicitation(request: &Request) -> bool {
    request
        .params
        .as_ref()
        .and_then(|params| params.get("capabilities"))
        .and_then(|capabilities| capabilities.get("elicitation"))
        .is_some_and(serde_json::Value::is_object)
}

fn parse_elicitation_decision(
    response: &serde_json::Value,
    expected_id: &str,
) -> Result<ElicitationDecision, String> {
    if response.get("id").and_then(serde_json::Value::as_str) != Some(expected_id) {
        return Err("elicitation response id did not match the pending request".to_owned());
    }
    if let Some(error) = response.get("error") {
        return Err(format!("MCP client rejected app approval elicitation: {error}"));
    }
    let result = response
        .get("result")
        .ok_or_else(|| "elicitation response did not include a result".to_owned())?;
    let action = match result.get("action").and_then(serde_json::Value::as_str) {
        Some("accept") => ElicitationAction::Accept,
        Some("decline") => ElicitationAction::Decline,
        Some("cancel") => ElicitationAction::Cancel,
        Some(other) => return Err(format!("unsupported elicitation action '{other}'")),
        None => return Err("elicitation response did not include an action".to_owned()),
    };
    let persistence = result
        .get("_meta")
        .and_then(|meta| meta.get("persist"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(ElicitationDecision {
        action,
        persistence,
    })
}

async fn write_json_value<W: AsyncWrite + Unpin>(
    writer: &mut W,
    value: &serde_json::Value,
) -> anyhow::Result<()> {
    let serialized = serde_json::to_vec(value)?;
    writer.write_all(&serialized).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    Ok(())
}

async fn await_elicitation_decision<R, W>(
    reader: &mut R,
    writer: &mut W,
    request_id: &str,
) -> Result<ElicitationDecision, String>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("read elicitation response: {error}"))?;
        if bytes == 0 {
            return Err("MCP client disconnected during app approval elicitation".to_owned());
        }
        let value: serde_json::Value = serde_json::from_str(line.trim())
            .map_err(|error| format!("decode elicitation response: {error}"))?;
        if value.get("id").and_then(serde_json::Value::as_str) == Some(request_id) {
            return parse_elicitation_decision(&value, request_id);
        }

        // MCP permits notifications while a server request is pending. Ignore
        // those, but fail an interleaved client request explicitly so the
        // caller does not wait forever for a response that cannot run yet.
        if value.get("method").is_some() && value.get("id").is_some() {
            let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
            let response = Response::error(
                id,
                -32000,
                "another MCP request cannot run while app approval is pending",
            );
            let response = serde_json::to_value(response)
                .map_err(|error| format!("encode pending-request response: {error}"))?;
            write_json_value(writer, &response)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
}

async fn send_daemon_request_async(
    socket_path: &str,
    request: DaemonRequest,
    context: &str,
) -> Result<DaemonResponse, String> {
    let socket = socket_path.to_owned();
    let blocking = tokio::task::spawn_blocking(move || send_request(&socket, &request)).await;
    match blocking {
        Err(error) => Err(format!("internal join error {context}: {error}")),
        Ok(Err(error)) => Err(format!("daemon transport error {context}: {error}")),
        Ok(Ok(response)) => Ok(response),
    }
}

async fn call_daemon_tool(
    socket_path: &str,
    session_id: &str,
    name: &str,
    args: serde_json::Value,
) -> Result<DaemonResponse, String> {
    send_daemon_request_async(
        socket_path,
        DaemonRequest {
            method: "call".to_owned(),
            name: Some(name.to_owned()),
            args: Some(args),
            session_id: Some(session_id.to_owned()),
        },
        &format!("forwarding `{name}`"),
    )
    .await
}

fn app_approval_error_response(
    id: serde_json::Value,
    code: &str,
    message: impl Into<String>,
) -> Response {
    let message = message.into();
    Response::ok(
        id,
        serde_json::json!({
            "content": [{"type": "text", "text": message}],
            "isError": true,
            "structuredContent": {"code": code, "message": message},
        }),
    )
}

async fn forward_tool_call_with_approval<R, W>(
    id: serde_json::Value,
    name: String,
    args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
    control_ready: &tokio::sync::watch::Receiver<ControlConnectionState>,
    client_supports_elicitation: bool,
    reader: &mut R,
    writer: &mut W,
) -> Response
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let broker_token = match wait_for_control_connection(control_ready.clone()).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            return Response::error(
                id,
                -32603,
                "daemon did not provide an authenticated app approval broker token",
            )
        }
        Err(error) => {
            return Response::error(
                id,
                -32603,
                format!(
                    "daemon session control connection unavailable before forwarding `{name}`: {error}"
                ),
            )
        }
    };

    let first = match call_daemon_tool(socket_path, session_id, &name, args.clone()).await {
        Ok(response) => response,
        Err(error) => return Response::error(id, -32603, error),
    };
    let Some(challenge) = AppApprovalChallenge::from_daemon_response(&first) else {
        return daemon_response_to_mcp(id, &name, first);
    };

    if !client_supports_elicitation {
        return app_approval_error_response(
            id,
            "elicitation_not_supported",
            format!(
                "Computer Use requires app approval for '{}', but this MCP client did not negotiate the elicitation capability.",
                challenge.display_name
            ),
        );
    }

    let elicitation_id = format!("cua-app-approval-{}", uuid::Uuid::new_v4());
    if let Err(error) = write_json_value(writer, &challenge.elicitation_request(&elicitation_id)).await {
        return Response::error(
            id,
            -32603,
            format!("write app approval elicitation: {error}"),
        );
    }
    let decision = match await_elicitation_decision(reader, writer, &elicitation_id).await {
        Ok(decision) => decision,
        Err(error) => {
            return app_approval_error_response(id, "app_approval_unavailable", error)
        }
    };

    let resolution = match send_daemon_request_async(
        socket_path,
        DaemonRequest {
            method: "app_approval_resolve".to_owned(),
            name: None,
            args: Some(serde_json::json!({
                "challenge": challenge.challenge,
                "broker_token": broker_token,
                "action": decision.action.as_str(),
                "persist": decision.persistence.as_deref().unwrap_or("session"),
            })),
            session_id: Some(session_id.to_owned()),
        },
        "resolving app approval",
    )
    .await
    {
        Ok(response) => response,
        Err(error) => return Response::error(id, -32603, error),
    };
    if !resolution.ok {
        return app_approval_error_response(
            id,
            "app_approval_unavailable",
            resolution
                .error
                .unwrap_or_else(|| "Computer Use could not resolve app approval.".to_owned()),
        );
    }
    let resolution = resolution.result.unwrap_or_default();
    match resolution
        .get("resolution")
        .and_then(serde_json::Value::as_str)
    {
        Some("approved") => {
            let retry = match call_daemon_tool(socket_path, session_id, &name, args).await {
                Ok(response) => response,
                Err(error) => return Response::error(id, -32603, error),
            };
            if AppApprovalChallenge::from_daemon_response(&retry).is_some() {
                return app_approval_error_response(
                    id,
                    "app_approval_unavailable",
                    "Computer Use approval was accepted, but the daemon requested approval again.",
                );
            }
            daemon_response_to_mcp(id, &name, retry)
        }
        Some("declined") => app_approval_error_response(
            id,
            "app_approval_denied",
            resolution
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Computer Use approval was declined."),
        ),
        Some("canceled") => app_approval_error_response(
            id,
            "app_approval_canceled",
            resolution
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Computer Use approval was canceled."),
        ),
        _ => app_approval_error_response(
            id,
            "app_approval_unavailable",
            "Computer Use daemon returned an invalid app approval resolution.",
        ),
    }
}

/// JSON-RPC method dispatcher for the proxy. Mirrors
/// `cua_driver_core::server::handle_request`:
///   - `initialize`     → static `initialize_result()` (same envelope
///                        the in-process path returns; the daemon's
///                        identity is hidden from the MCP client).
///   - `tools/list`     → return the cached daemon tool list.
///   - `tools/call`     → forward to the daemon and reshape the
///                        response into MCP's `CallTool.Result`.
///   - other            → method-not-found, same as in-process.
async fn handle_proxy_request(
    req: Request,
    id: serde_json::Value,
    socket_path: &str,
    cached_tools_list: &Arc<serde_json::Value>,
    session_id: &str,
    control_ready: &tokio::sync::watch::Receiver<ControlConnectionState>,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, (**cached_tools_list).clone()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                forward_tool_call(
                    id,
                    call.name,
                    call.args,
                    socket_path,
                    session_id,
                    control_ready,
                )
                .await
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}

/// Forward a single MCP `tools/call` to the daemon as a `call`
/// request, then translate the `DaemonResponse` back into an MCP
/// `CallTool.Result` envelope.
///
/// Error mapping:
///   - Tool ran and reported failure (`!resp.ok`, including unknown
///     tool / bad params) → JSON-RPC success with `result.isError =
///     true`. Mirrors the in-process `cua_driver_core::server` path so
///     MCP clients see identical envelopes either way.
///   - Transport failure (UDS unreachable, decode error, blocking
///     task panic) → JSON-RPC error (`-32603`), because the MCP
///     client really does need to distinguish "tool said no" from
///     "I couldn't reach the tool at all."
async fn forward_tool_call(
    id: serde_json::Value,
    name: String,
    args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
    control_ready: &tokio::sync::watch::Receiver<ControlConnectionState>,
) -> Response {
    if let Err(error) = wait_for_control_connection(control_ready.clone()).await {
        return Response::error(
            id,
            -32603,
            format!(
                "daemon session control connection unavailable before forwarding `{name}`: {error}"
            ),
        );
    }
    let response = match call_daemon_tool(socket_path, session_id, &name, args).await {
        Ok(response) => response,
        Err(error) => return Response::error(id, -32603, error),
    };
    daemon_response_to_mcp(id, &name, response)
}

fn daemon_response_to_mcp(
    id: serde_json::Value,
    _name: &str,
    response: DaemonResponse,
) -> Response {
    if !response.ok {
        // MCP separates two failure modes:
        //   - JSON-RPC errors → `Response::error(...)`, used for
        //     transport / protocol failures (unknown method, bad
        //     params shape, server crash).
        //   - Tool-level errors → `Response::ok(...)` carrying a
        //     `CallTool.Result` with `isError: true` and the error
        //     message in `content[]`. The tool ran, returned a
        //     well-formed result that says "I failed."
        //
        // A non-`ok` daemon response means the tool call reached the
        // daemon and the daemon decided the tool returned an error
        // (or rejected the call). That's tool-level, not transport-
        // level, so the in-process `cua_driver_core::server` would surface
        // it as `Response::ok` with `isError: true`. Mirror that
        // shape here so MCP clients see identical envelopes either
        // way — CodeRabbit #2.
        if let Some(result) = response
            .result
            .filter(|result| result.get("isError").and_then(|value| value.as_bool()) == Some(true))
        {
            return Response::ok(id, result);
        }
        let msg = response
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = response.exit_code.unwrap_or(1);
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true,
            "structuredContent": { "exit_code": exit_code }
        });
        return Response::ok(id, result);
    }

    let result = response.result.unwrap_or_else(|| {
        serde_json::json!({
            "content": [],
            "isError": false
        })
    });
    Response::ok(id, result)
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// Unit-test only the JSON shape of the proxy's tool-error envelope.
// The full proxy loop is exercised by the macOS integration test
// (the CUA_DRIVER_RS_MCP_FORCE_PROXY harness); these tests just lock
// in the per-branch reshape so a
// regression to `Response::error` for tool-level failures would fail
// fast in CI on every platform.

#[cfg(test)]
mod tests {
    use super::*;

    fn approval_challenge_response(allow_persistent: bool) -> DaemonResponse {
        DaemonResponse::tool_error(
            serde_json::json!({
                "content": [{"type":"text","text":"approval required"}],
                "isError": true,
                "structuredContent": {
                    "code": "app_approval_required",
                    "message": "approval required",
                    "approval": {
                        "challenge": "challenge-1",
                        "app": "com.apple.calculator",
                        "displayName": "Calculator",
                        "allowPersistentApproval": allow_persistent,
                    }
                }
            }),
            "approval required",
            1,
        )
    }

    #[test]
    fn initialize_capability_negotiation_is_explicit() {
        let supported: Request = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"capabilities": {"elicitation": {}}},
        }))
        .unwrap();
        assert!(supports_elicitation(&supported));

        let unsupported: Request = serde_json::from_value(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"capabilities": {}},
        }))
        .unwrap();
        assert!(!supports_elicitation(&unsupported));
    }

    #[test]
    fn approval_challenge_builds_codex_computer_use_elicitation_metadata() {
        let challenge = AppApprovalChallenge::from_daemon_response(
            &approval_challenge_response(true),
        )
        .unwrap();
        let request = challenge.elicitation_request("approval-request-1");
        assert_eq!(request["method"], "elicitation/create");
        assert_eq!(
            request["params"]["message"],
            "Allow Computer Use to use \"Calculator\"?"
        );
        assert_eq!(
            request["params"]["requestedSchema"],
            serde_json::json!({"type":"object","properties":{}})
        );
        let meta = &request["params"]["_meta"];
        assert_eq!(meta["codex_approval_kind"], "mcp_tool_call");
        assert_eq!(meta["connector_id"], "computer-use");
        assert_eq!(meta["connector_name"], "Computer Use");
        assert_eq!(meta["persist"], serde_json::json!(["session", "always"]));
        assert_eq!(meta["riskLevel"], "low");
        assert_eq!(meta["tool_params"]["app"], "com.apple.calculator");
        assert_eq!(meta["tool_params_display"][0]["value"], "Calculator");

        let session_only = AppApprovalChallenge::from_daemon_response(
            &approval_challenge_response(false),
        )
        .unwrap()
        .elicitation_request("approval-request-2");
        assert_eq!(
            session_only["params"]["_meta"]["persist"],
            serde_json::json!(["session"])
        );
    }

    #[test]
    fn elicitation_decisions_distinguish_accept_decline_cancel_and_persistence() {
        for (action, expected) in [
            ("accept", ElicitationAction::Accept),
            ("decline", ElicitationAction::Decline),
            ("cancel", ElicitationAction::Cancel),
        ] {
            let decision = parse_elicitation_decision(
                &serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": "approval-1",
                    "result": {"action": action},
                }),
                "approval-1",
            )
            .unwrap();
            assert_eq!(decision.action, expected);
            assert_eq!(decision.persistence, None);
        }

        let persistent = parse_elicitation_decision(
            &serde_json::json!({
                "jsonrpc": "2.0",
                "id": "approval-1",
                "result": {"action": "accept", "_meta": {"persist": "always"}},
            }),
            "approval-1",
        )
        .unwrap();
        assert_eq!(persistent.action, ElicitationAction::Accept);
        assert_eq!(persistent.persistence.as_deref(), Some("always"));
    }

    #[tokio::test]
    async fn nested_elicitation_response_is_read_as_a_response_not_a_request() {
        let (proxy_stream, mut client_stream) = tokio::io::duplex(2048);
        let (proxy_read, mut proxy_write) = tokio::io::split(proxy_stream);
        let mut proxy_read = BufReader::new(proxy_read);
        tokio::spawn(async move {
            client_stream
                .write_all(
                    b"{\"jsonrpc\":\"2.0\",\"id\":\"approval-7\",\"result\":{\"action\":\"accept\",\"_meta\":{\"persist\":\"always\"}}}\n",
                )
                .await
                .unwrap();
        });
        let decision = await_elicitation_decision(
            &mut proxy_read,
            &mut proxy_write,
            "approval-7",
        )
        .await
        .unwrap();
        assert_eq!(decision.action, ElicitationAction::Accept);
        assert_eq!(decision.persistence.as_deref(), Some("always"));
    }

    #[test]
    fn unsupported_client_error_is_a_tool_failure() {
        let response = app_approval_error_response(
            serde_json::json!(9),
            "elicitation_not_supported",
            "client did not negotiate elicitation",
        );
        let response = serde_json::to_value(response).unwrap();
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["structuredContent"]["code"],
            "elicitation_not_supported"
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsupported_client_does_not_elicit_resolve_or_retry() {
        use tokio::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("approval.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&mut stream);
                reader.read_line(&mut line).await.unwrap();
            }
            let request: DaemonRequest = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(request.method, "call");
            assert_eq!(request.name.as_deref(), Some("get_app_state"));
            let response = approval_challenge_response(true);
            stream
                .write_all(
                    format!("{}\n", serde_json::to_string(&response).unwrap()).as_bytes(),
                )
                .await
                .unwrap();
            stream.flush().await.unwrap();

            assert!(
                tokio::time::timeout(
                    std::time::Duration::from_millis(250),
                    listener.accept(),
                )
                .await
                .is_err(),
                "unsupported client must not resolve or retry the challenge"
            );
        });

        let (_control_tx, control_rx) = tokio::sync::watch::channel(
            ControlConnectionState::Ready {
                approval_broker_token: Some("daemon-broker-token".to_owned()),
            },
        );
        let (proxy_stream, _client_stream) = tokio::io::duplex(1024);
        let (proxy_read, mut proxy_write) = tokio::io::split(proxy_stream);
        let mut proxy_read = BufReader::new(proxy_read);
        let response = forward_tool_call_with_approval(
            serde_json::json!(4),
            "get_app_state".to_owned(),
            serde_json::json!({"app":"Calculator"}),
            socket.to_str().unwrap(),
            "session-a",
            &control_rx,
            false,
            &mut proxy_read,
            &mut proxy_write,
        )
        .await;
        let response = serde_json::to_value(response).unwrap();
        assert_eq!(
            response["result"]["structuredContent"]["code"],
            "elicitation_not_supported"
        );
        server.await.unwrap();
    }

    /// Reconstruct the `!resp.ok` branch in isolation so we can assert
    /// on the serialized shape without spinning up a real daemon /
    /// tokio runtime. Keep this in sync with `forward_tool_call`.
    fn build_tool_error_response(
        id: serde_json::Value,
        resp: DaemonResponse,
    ) -> Response {
        let msg = resp.error.unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = resp.exit_code.unwrap_or(1);
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true,
            "structuredContent": { "exit_code": exit_code }
        });
        Response::ok(id, result)
    }

    #[test]
    fn daemon_tool_failure_wraps_as_jsonrpc_success_with_iserror_true() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: Some("missing required field `pid`".into()),
            exit_code: Some(64),
        };
        let resp = build_tool_error_response(serde_json::json!(7), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");

        // Top-level JSON-RPC envelope: success (`result`), not error.
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], serde_json::json!(7));
        assert!(value.get("error").is_none(),
            "tool-level failure must NOT surface as JSON-RPC error: got {value}");
        assert!(value.get("result").is_some(),
            "tool-level failure must carry a `result` payload: got {value}");

        // CallTool.Result inside `result`: isError + content text.
        let result = &value["result"];
        assert_eq!(result["isError"], serde_json::json!(true));
        assert_eq!(result["content"][0]["type"], "text");
        assert_eq!(result["content"][0]["text"], "missing required field `pid`");
        assert_eq!(result["structuredContent"]["exit_code"], 64);
    }

    #[test]
    fn daemon_failure_with_no_error_message_uses_fallback_text() {
        let daemon_resp = DaemonResponse {
            ok: false,
            result: None,
            error: None,
            exit_code: None,
        };
        let resp = build_tool_error_response(serde_json::json!("abc"), daemon_resp);
        let value = serde_json::to_value(&resp).expect("serialize");
        assert_eq!(value["result"]["isError"], serde_json::json!(true));
        assert_eq!(value["result"]["content"][0]["text"], "daemon reported failure");
        assert_eq!(value["result"]["structuredContent"]["exit_code"], 1);
    }

    #[test]
    fn daemon_tool_failure_preserves_original_structured_error() {
        let original = serde_json::json!({
            "content": [{"type":"text","text":"Call get_app_state first"}],
            "isError": true,
            "structuredContent": {
                "code": "no_active_app_session",
                "message": "Call get_app_state first"
            }
        });
        let daemon_resp = DaemonResponse::tool_error(original.clone(), "fallback", 1);
        let result = daemon_resp
            .result
            .filter(|result| {
                result.get("isError").and_then(|value| value.as_bool()) == Some(true)
            })
            .expect("structured tool error");
        assert_eq!(result, original);
    }

    fn daemon_list_result(profile: &str, names: &[&str]) -> serde_json::Value {
        serde_json::json!({
            "profile": profile,
            "tools": names.iter().map(|name| serde_json::json!({"name": name})).collect::<Vec<_>>()
        })
    }

    #[test]
    fn daemon_profile_mismatch_fails_closed() {
        let native = daemon_list_result("native", &["click"]);
        let error = validate_daemon_profile_and_roster(
            &native,
            DaemonProfile::CodexComputerUseCompat,
            "/tmp/explicit.sock",
        )
        .expect_err("native daemon must not satisfy compat proxy");
        assert!(error.to_string().contains("profile mismatch"));
        assert!(error.to_string().contains("/tmp/explicit.sock"));

        let compat = daemon_list_result(
            "codex-computer-use-compat",
            &CODEX_COMPUTER_USE_TOOL_NAMES,
        );
        let error = validate_daemon_profile_and_roster(
            &compat,
            DaemonProfile::Native,
            "/tmp/explicit.sock",
        )
        .expect_err("compat daemon must not satisfy native proxy");
        assert!(error.to_string().contains("profile mismatch"));
    }

    #[test]
    fn compat_profile_requires_exact_ordered_ten_tool_roster() {
        let exact = daemon_list_result(
            "codex-computer-use-compat",
            &CODEX_COMPUTER_USE_TOOL_NAMES,
        );
        validate_daemon_profile_and_roster(
            &exact,
            DaemonProfile::CodexComputerUseCompat,
            "/tmp/compat.sock",
        )
        .expect("exact compat roster");

        let mut reordered = CODEX_COMPUTER_USE_TOOL_NAMES;
        reordered.swap(0, 1);
        let wrong = daemon_list_result("codex-computer-use-compat", &reordered);
        let error = validate_daemon_profile_and_roster(
            &wrong,
            DaemonProfile::CodexComputerUseCompat,
            "/tmp/compat.sock",
        )
        .expect_err("reordered roster must fail closed");
        assert!(error.to_string().contains("roster is invalid"));
    }

    #[test]
    fn reconnect_ack_must_preserve_the_requested_profile() {
        let native = DaemonResponse::ok(serde_json::json!({
            "session_begin": true,
            "profile": DaemonProfile::Native,
        }));
        let native = serde_json::to_string(&native).unwrap();
        validate_control_ack(&native, DaemonProfile::Native).expect("matching profile");
        let error = validate_control_ack(
            &native,
            DaemonProfile::CodexComputerUseCompat,
        )
        .expect_err("a replacement daemon with the wrong profile must fail closed");
        assert!(error.to_string().contains("profile mismatch"));

        let missing = DaemonResponse::ok(serde_json::json!({"session_begin": true}));
        let missing = serde_json::to_string(&missing).unwrap();
        assert!(validate_control_ack(&missing, DaemonProfile::Native)
            .unwrap_err()
            .to_string()
            .contains("did not report"));

        let compat_without_broker = DaemonResponse::ok(serde_json::json!({
            "session_begin": true,
            "profile": DaemonProfile::CodexComputerUseCompat,
        }));
        assert!(validate_control_ack(
            &serde_json::to_string(&compat_without_broker).unwrap(),
            DaemonProfile::CodexComputerUseCompat,
        )
        .unwrap_err()
        .to_string()
        .contains("approval broker token"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn control_connection_reconnects_after_daemon_rebind() {
        use tokio::net::{UnixListener, UnixStream};

        async fn accept_session_begin(
            listener: &UnixListener,
            expected_session: &str,
        ) -> UnixStream {
            let (mut stream, _) = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                listener.accept(),
            )
            .await
            .expect("control connection timeout")
            .expect("accept control connection");
            let mut line = String::new();
            {
                let mut reader = BufReader::new(&mut stream);
                tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    reader.read_line(&mut line),
                )
                .await
                .expect("session_begin timeout")
                .expect("read session_begin");
            }
            let request: DaemonRequest =
                serde_json::from_str(line.trim()).expect("decode session_begin");
            assert_eq!(request.method, "session_begin");
            assert_eq!(request.session_id.as_deref(), Some(expected_session));
            let ack = DaemonResponse::ok(serde_json::json!({
                "session_begin": true,
                "profile": DaemonProfile::Native,
            }));
            stream
                .write_all(
                    format!("{}\n", serde_json::to_string(&ack).unwrap()).as_bytes(),
                )
                .await
                .expect("write session_begin ACK");
            stream.flush().await.expect("flush session_begin ACK");
            stream
        }

        let root = tempfile::tempdir().expect("socket tempdir");
        let socket = root.path().join("daemon.sock");
        let first_listener = UnixListener::bind(&socket).expect("bind first daemon");
        let session_id = "proxy-reexec-test-session".to_owned();
        let (ready_tx, mut ready_rx) =
            tokio::sync::watch::channel(ControlConnectionState::Connecting);
        let task = tokio::spawn(run_control_connection(
            socket.to_string_lossy().into_owned(),
            session_id.clone(),
            DaemonProfile::Native,
            ready_tx,
        ));

        let first_stream = accept_session_begin(&first_listener, &session_id).await;
        wait_for_control_connection(ready_rx.clone())
            .await
            .expect("first control connection ready");
        drop(first_listener);
        drop(first_stream);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while matches!(
                &*ready_rx.borrow(),
                ControlConnectionState::Ready { .. }
            ) {
                ready_rx.changed().await.expect("readiness sender alive");
            }
        })
        .await
        .expect("first control connection closes");
        std::fs::remove_file(&socket).expect("remove first daemon socket");

        let second_listener = UnixListener::bind(&socket).expect("bind replacement daemon");
        let second_stream = accept_session_begin(&second_listener, &session_id).await;
        wait_for_control_connection(ready_rx.clone())
            .await
            .expect("replacement control connection ready");

        task.abort();
        let _ = task.await;
        drop(second_stream);
    }
}
