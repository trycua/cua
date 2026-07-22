//! Stdio MCP proxy that forwards `tools/list` and `tools/call` through
//! the running `cua-driver-rs serve` daemon over its Unix socket.
//!
//! This is the only MCP execution path. The client side sees a normal stdio
//! server, while the daemon remains the single owner of tool state, policy,
//! and platform permission identity.
//!
//! On macOS the CLI can ensure a daemon is running under `LaunchServices`
//! (which gives it the right TCC attribution). Embedded hosts and other
//! platforms start the daemon explicitly. The MCP client never sees that
//! boundary — it receives the standard JSON-RPC envelope.
//!
//! Why this lives in `cua-driver` and not `mcp-server`:
//!   `cua_driver_core::server` defines the shared JSON-RPC protocol. The
//!   proxy speaks that protocol on the client side, while the server side is the daemon's
//!   line-delimited JSON UDS protocol, owned by `crate::serve`.
//!   Putting the proxy here avoids `mcp-server → cua-driver` reverse
//!   coupling.

use std::sync::Arc;

use cua_driver_core::policy::{authorize_tool_call, validate_configured_policy};
use cua_driver_core::protocol::{initialize_result, Request, Response};
use cua_driver_core::server::{
    observe_proxy_session_started, observe_proxy_tool_completed, tool_observation_timer,
    StdioExecutionPath,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::serve::{is_daemon_listening, send_request, DaemonRequest, ToolObservationOrigin};

/// Run the MCP stdio proxy. Reads JSON-RPC lines from stdin, forwards
/// the body of each `tools/list` / `tools/call` to the daemon at
/// `socket_path`, and writes the daemon's response back as a proper
/// JSON-RPC envelope.
///
/// Implements the core protocol's EOF, parse-error, notification, and
/// response rules while forwarding method dispatch to the daemon.
///
/// Fails fast if the daemon isn't reachable, so MCP clients see a
/// clear startup error instead of a "successful" handshake that
/// advertises zero tools and then errors on every call. Matches
/// Swift `makeProxy`'s `fetchProxyToolList` pre-check.
pub async fn run_proxy(socket_path: String) -> anyhow::Result<()> {
    validate_configured_policy()?;
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
    // The daemon must acknowledge `session_begin` before the proxy accepts tool
    // calls. Besides lifecycle cleanup, that registered control channel is the
    // trust boundary used by destructive `browser_prepare` calls.
    let (control_ready_tx, control_ready_rx) = tokio::sync::oneshot::channel();
    {
        let socket = socket_path.clone();
        let sid = session_id.clone();
        tokio::spawn(async move {
            run_control_connection(socket, sid, control_ready_tx).await;
        });
    }
    tokio::time::timeout(std::time::Duration::from_secs(4), control_ready_rx)
        .await
        .map_err(|_| anyhow::anyhow!("daemon did not acknowledge the MCP control session"))?
        .map_err(|_| anyhow::anyhow!("daemon control session closed before acknowledgement"))?;

    // Cache the tool list once at startup. The daemon's registry is
    // static for the lifetime of the daemon, so polling on every
    // `tools/list` would waste a round-trip per call. Swift does the
    // same caching in `fetchProxyToolList`.
    let (cached_tools_list, daemon_observes_tool_calls) =
        fetch_tools_list_from_daemon(&socket_path, &session_id)?;
    let cached_tools_list = Arc::new(cached_tools_list);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();
    let mut session_observed = false;

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
                // Notifications are intentionally dropped by the stdio adapter.
                continue;
            }
            Ok(req) => {
                let initialize_metadata = (!session_observed)
                    .then(|| req.initialize_metadata())
                    .flatten();
                let session_context = (!daemon_observes_tool_calls)
                    .then(|| {
                        req.tool_call().ok().and_then(|call| {
                            let known_tool = proxy_knows_tool(&cached_tools_list, &call.name);
                            cua_driver_core::session::begin_tool_call(
                                &call.name,
                                &call.args,
                                known_tool,
                                cua_driver_core::session::SessionTransport::McpStdio,
                                cua_driver_core::session::SessionClientKind::Mcp,
                            )
                        })
                    })
                    .flatten();
                let tool_timer = (!daemon_observes_tool_calls)
                    .then(|| {
                        tool_observation_timer(
                            &req,
                            |name| proxy_knows_tool(&cached_tools_list, name),
                            StdioExecutionPath::DaemonProxy,
                        )
                    })
                    .flatten();
                let id = req.id.clone().unwrap_or(serde_json::Value::Null);
                let response = handle_proxy_request(
                    req,
                    id,
                    &socket_path,
                    &cached_tools_list,
                    &session_id,
                    daemon_observes_tool_calls,
                )
                .await;
                if let Some(metadata) = initialize_metadata {
                    observe_proxy_session_started(metadata);
                    session_observed = true;
                }
                if let Some(timer) = tool_timer {
                    let outcome = timer.finish(&response);
                    if let Some(context) = session_context {
                        context.complete(&outcome);
                    }
                    observe_proxy_tool_completed(outcome);
                }
                response
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

fn proxy_knows_tool(cached_tools_list: &serde_json::Value, name: &str) -> bool {
    if name == "type_text_chars" {
        return true;
    }
    cached_tools_list
        .get("tools")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|tools| {
            tools
                .iter()
                .any(|tool| tool.get("name").and_then(serde_json::Value::as_str) == Some(name))
        })
}

/// Own the proxy's single long-lived control connection. Connects directly to
/// the daemon socket (its OWN async open — `send_request` is sync, blocking,
/// and one-shot, so it cannot be reused here), sends one `session_begin` line
/// carrying `session_id`, then parks in a read loop until the connection
/// closes. It never writes again. The daemon records `session_id` from
/// `session_begin` and fires `session_end` when this connection EOFs — which
/// the kernel triggers on proxy exit AND on kill -9.
///
/// On any read result/error (daemon-side close, broken pipe), the loop exits
/// and the task ends; the proxy keeps running on its per-call connections. A
/// connect failure (racing daemon startup) is logged and swallowed — it must
/// not bail the proxy.
async fn run_control_connection(
    socket_path: String,
    session_id: String,
    control_ready: tokio::sync::oneshot::Sender<()>,
) {
    let begin = DaemonRequest {
        method: "session_begin".into(),
        name: None,
        args: None,
        session_id: Some(session_id.clone()),
        observation_origin: None,
        client_kind: None,
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
        use tokio::net::UnixStream;
        // Retry the connect briefly — the daemon may still be spinning up
        // (mirrors the windows pipe-open retry below). The is_daemon_listening
        // precheck makes the window tiny, but keep both paths symmetric.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut stream = loop {
            match UnixStream::connect(&socket_path).await {
                Ok(s) => break s,
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => {
                    debug!(session_id = %session_id, "control connect failed (daemon starting?): {e}");
                    return;
                }
            }
        };
        if let Err(e) = stream.write_all(line.as_bytes()).await {
            debug!("control connection: write session_begin failed: {e}");
            return;
        }
        let _ = stream.flush().await;
        debug!(session_id = %session_id, "control connection established (session_begin sent)");

        // Park: read until the daemon closes (it ACKs session_begin then keeps
        // the conn open; we drain anything and only return on EOF/error). The
        // proxy never writes here again — the connection lives until process
        // death, when the kernel closes it and the daemon reaps the session.
        let mut reader = BufReader::new(stream);
        let mut buf = String::new();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let _ = control_ready.send(());
            }
        }
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => break, // daemon closed or error — task done.
                Ok(_) => continue,       // ACK / stray line — ignore, keep parked.
            }
        }
        debug!(session_id = %session_id, "control connection closed");
    }

    #[cfg(all(not(unix), target_os = "windows"))]
    {
        use tokio::net::windows::named_pipe::ClientOptions;
        // Retry the pipe open briefly — the daemon may still be spinning up its
        // next instance (mirrors send_request's open-retry).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let client = loop {
            match ClientOptions::new().open(&socket_path) {
                Ok(c) => break Some(c),
                Err(_) if std::time::Instant::now() < deadline => {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                Err(e) => {
                    debug!(session_id = %session_id, "control pipe open failed (daemon starting?): {e}");
                    break None;
                }
            }
        };
        let mut client = match client {
            Some(c) => c,
            None => return,
        };
        if let Err(e) = client.write_all(line.as_bytes()).await {
            debug!("control connection: write session_begin failed: {e}");
            return;
        }
        let _ = client.flush().await;
        debug!(session_id = %session_id, "control connection established (session_begin sent)");

        let mut reader = BufReader::new(client);
        let mut buf = String::new();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let _ = control_ready.send(());
            }
        }
        loop {
            buf.clear();
            match reader.read_line(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(_) => continue,
            }
        }
        debug!(session_id = %session_id, "control connection closed");
    }

    #[cfg(all(not(unix), not(target_os = "windows")))]
    {
        let _ = (line, session_id, socket_path, control_ready);
    }
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

/// One-shot daemon `list` over the UDS, reshaped into a MCP
/// `tools/list` result. The daemon now returns the full ToolDef
/// (`name`, `description`, `input_schema`, annotation hints) per
/// commit 3's `serve.rs` change.
fn fetch_tools_list_from_daemon(
    socket_path: &str,
    session_id: &str,
) -> anyhow::Result<(serde_json::Value, bool)> {
    let req = DaemonRequest {
        method: "list".into(),
        name: None,
        args: None,
        session_id: Some(session_id.to_owned()),
        observation_origin: None,
        client_kind: None,
    };
    let resp = send_request(socket_path, &req)?;
    if !resp.ok {
        anyhow::bail!(
            "daemon refused tool list on {socket_path}: {}",
            resp.error.unwrap_or_else(|| "(no error message)".into())
        );
    }
    let result = resp
        .result
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `result` field"))?;
    let tools_array = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("daemon list response missing `tools` array"))?;

    // Reshape the daemon's `{name, description, input_schema, read_only,
    // ..., capabilities}` envelope into MCP's `{name, description,
    // inputSchema, annotations: {...}, capabilities}` shape. Same
    // translation `ToolDef::to_list_entry` defines for the core protocol.
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
            let input_schema = t
                .get("input_schema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}}));
            let read_only = t
                .get("read_only")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let destructive = t
                .get("destructive")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let idempotent = t
                .get("idempotent")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let open_world = t
                .get("open_world")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let capabilities = t
                .get("capabilities")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_else(|| {
                    // Fallback: derive from the centralised name + schema
                    // resolver. Keeps the proxy compatible with daemon
                    // builds that pre-date the capabilities field.
                    name.as_str()
                        .map(|name| {
                            cua_driver_core::tool::advertised_capabilities_for(name, &input_schema)
                        })
                        .unwrap_or_default()
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect()
                });
            let risk = t.get("risk").cloned().unwrap_or_else(|| {
                name.as_str()
                    .map(cua_driver_core::authorization::risk_metadata_json)
                    .unwrap_or_else(|| {
                        serde_json::json!({
                            "class": "unclassified",
                            "enforcement": "metadata_only",
                            "operation_sensitive": false,
                            "version": cua_driver_core::authorization::RISK_METADATA_VERSION,
                        })
                    })
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
                "risk": risk,
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
        .unwrap_or_else(|| {
            serde_json::Value::String(cua_driver_core::tool::CAPABILITY_VERSION.to_owned())
        });
    let schema_version = result.get("schema_version").cloned().unwrap_or_else(|| {
        serde_json::Value::String(cua_driver_core::tool::TOOLS_LIST_SCHEMA_VERSION.to_owned())
    });

    let daemon_observes_tool_calls = daemon_owns_tool_observation(&result);

    Ok((
        serde_json::json!({
            "tools": mcp_tools,
            "capability_version": capability_version,
            "schema_version": schema_version,
        }),
        daemon_observes_tool_calls,
    ))
}

fn daemon_owns_tool_observation(result: &serde_json::Value) -> bool {
    result
        .get("tool_observation_owner")
        .and_then(serde_json::Value::as_str)
        == Some("daemon")
}

/// JSON-RPC method dispatcher for the proxy. Mirrors
/// `cua_driver_core::server::handle_request`:
/// - `initialize` → static `initialize_result()` (same envelope as the core
///   protocol server; the daemon's identity is hidden from the MCP client).
/// - `tools/list` → return the cached daemon tool list.
/// - `tools/call` → forward to the daemon and reshape the response into MCP's
///   `CallTool.Result`.
/// - other → method-not-found.
async fn handle_proxy_request(
    req: Request,
    id: serde_json::Value,
    socket_path: &str,
    cached_tools_list: &Arc<serde_json::Value>,
    session_id: &str,
    daemon_observes_tool_calls: bool,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, (**cached_tools_list).clone()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                if let Err(error) = authorize_tool_call(&call.name, &call.args) {
                    return Response::error(id, -32603, error.to_string());
                }
                forward_tool_call(
                    id,
                    call.name,
                    call.args,
                    socket_path,
                    session_id,
                    daemon_observes_tool_calls,
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
///     true`. Mirrors the core protocol's tool-error envelope.
///   - Transport failure (UDS unreachable, decode error, blocking
///     task panic) → JSON-RPC error (`-32603`), because the MCP
///     client really does need to distinguish "tool said no" from
///     "I couldn't reach the tool at all."
async fn forward_tool_call(
    id: serde_json::Value,
    name: String,
    mut args: serde_json::Value,
    socket_path: &str,
    session_id: &str,
    daemon_observes_tool_calls: bool,
) -> Response {
    cua_driver_core::tool_args::sanitize_reserved_args(&mut args);
    let req = DaemonRequest {
        method: "call".into(),
        name: Some(name.clone()),
        args: Some(args),
        session_id: Some(session_id.to_owned()),
        observation_origin: daemon_observes_tool_calls.then_some(ToolObservationOrigin::McpProxy),
        client_kind: None,
    };

    // The daemon client is sync, so jump to a blocking thread to keep
    // the tokio reactor responsive while the AX-heavy call (e.g.
    // `screenshot`, `get_window_state`) does its thing on the daemon
    // side.
    let socket = socket_path.to_owned();
    let blocking = tokio::task::spawn_blocking(move || send_request(&socket, &req)).await;

    let resp = match blocking {
        Err(join_err) => {
            return Response::error(
                id,
                -32603,
                format!("internal join error forwarding to daemon: {join_err}"),
            );
        }
        Ok(Err(e)) => {
            return Response::error(
                id,
                -32603,
                format!("daemon transport error forwarding `{name}`: {e}"),
            );
        }
        Ok(Ok(r)) => r,
    };

    if !resp.ok {
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
        // level, so the core protocol surfaces it as `Response::ok` with
        // `isError: true`. Mirror that shape here — CodeRabbit #2.
        let msg = resp
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
        let exit_code = resp.exit_code.unwrap_or(1);
        let result = serde_json::json!({
            "content": [{ "type": "text", "text": msg }],
            "isError": true,
            "structuredContent": { "exit_code": exit_code }
        });
        return Response::ok(id, result);
    }

    let result = resp.result.unwrap_or_else(|| {
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
// (the daemon-backed integration harness); these tests just lock
// in the per-branch reshape so a
// regression to `Response::error` for tool-level failures would fail
// fast in CI on every platform.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::DaemonResponse;

    /// Reconstruct the `!resp.ok` branch in isolation so we can assert
    /// on the serialized shape without spinning up a real daemon /
    /// tokio runtime. Keep this in sync with `forward_tool_call`.
    fn build_tool_error_response(id: serde_json::Value, resp: DaemonResponse) -> Response {
        let msg = resp
            .error
            .unwrap_or_else(|| "daemon reported failure".into());
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
        assert!(
            value.get("error").is_none(),
            "tool-level failure must NOT surface as JSON-RPC error: got {value}"
        );
        assert!(
            value.get("result").is_some(),
            "tool-level failure must carry a `result` payload: got {value}"
        );

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
        assert_eq!(
            value["result"]["content"][0]["text"],
            "daemon reported failure"
        );
        assert_eq!(value["result"]["structuredContent"]["exit_code"], 1);
    }

    #[test]
    fn cached_proxy_tool_allowlist_is_exact() {
        let cached = serde_json::json!({
            "tools": [
                {"name":"click"},
                {"name":"type_text"}
            ]
        });
        assert!(proxy_knows_tool(&cached, "click"));
        assert!(
            proxy_knows_tool(&cached, "type_text_chars"),
            "deprecated alias stays bounded/known"
        );
        assert!(!proxy_knows_tool(&cached, "click/private-user-value"));
        assert!(!proxy_knows_tool(&cached, ""));
    }

    #[test]
    fn observation_ownership_requires_the_daemon_capability() {
        assert!(daemon_owns_tool_observation(&serde_json::json!({
            "tool_observation_owner": "daemon"
        })));
        assert!(!daemon_owns_tool_observation(&serde_json::json!({})));
        assert!(!daemon_owns_tool_observation(&serde_json::json!({
            "tool_observation_owner": "proxy"
        })));
    }
}
