//! Stdio MCP proxy that forwards `tools/list` and `tools/call` through
//! the running `cua-driver-rs serve` daemon over its Unix socket.
//!
//! This is the runtime half of the TCC auto-relaunch path (issue #1525,
//! mirror of Swift PR #1479). When `cua-driver-rs mcp` is invoked from
//! an IDE terminal — Claude Code, Cursor, VS Code, Warp — macOS TCC
//! attributes the process to the calling terminal, not to
//! `CuaDriverRs.app`. The MCP client side sees a normal stdio server,
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
//!   `mcp_server::server::run` already speaks JSON-RPC over stdio
//!   against an in-process `ToolRegistry`. The proxy speaks the same
//!   protocol on the client side but the server side is the daemon's
//!   line-delimited JSON UDS protocol, owned by `crate::serve`.
//!   Putting the proxy here avoids `mcp-server → cua-driver` reverse
//!   coupling.

use std::sync::Arc;

use mcp_server::protocol::{initialize_result, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::serve::{is_daemon_listening, send_request, DaemonRequest};

/// Run the MCP stdio proxy. Reads JSON-RPC lines from stdin, forwards
/// the body of each `tools/list` / `tools/call` to the daemon at
/// `socket_path`, and writes the daemon's response back as a proper
/// JSON-RPC envelope.
///
/// Mirrors `mcp_server::server::run`'s control flow exactly — same
/// EOF + parse-error + notification handling — only the per-method
/// branches change.
///
/// Fails fast if the daemon isn't reachable, so MCP clients see a
/// clear startup error instead of a "successful" handshake that
/// advertises zero tools and then errors on every call. Matches
/// Swift `makeProxy`'s `fetchProxyToolList` pre-check.
pub async fn run_proxy(socket_path: String) -> anyhow::Result<()> {
    if !is_daemon_listening(&socket_path) {
        anyhow::bail!(
            "cua-driver-rs daemon not reachable on {socket_path}. Start it \
             with `open -n -g -a CuaDriverRs --args serve` and retry."
        );
    }

    // Cache the tool list once at startup. The daemon's registry is
    // static for the lifetime of the daemon, so polling on every
    // `tools/list` would waste a round-trip per call. Swift does the
    // same caching in `fetchProxyToolList`.
    let cached_tools_list = fetch_tools_list_from_daemon(&socket_path)?;
    let cached_tools_list = Arc::new(cached_tools_list);

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF
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
                handle_proxy_request(req, id, &socket_path, &cached_tools_list).await
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

    Ok(())
}

/// One-shot daemon `list` over the UDS, reshaped into a MCP
/// `tools/list` result. The daemon now returns the full ToolDef
/// (`name`, `description`, `input_schema`, annotation hints) per
/// commit 3's `serve.rs` change.
fn fetch_tools_list_from_daemon(socket_path: &str) -> anyhow::Result<serde_json::Value> {
    let req = DaemonRequest { method: "list".into(), name: None, args: None };
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
    let tools_array = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow::anyhow!("daemon list response missing `tools` array")
        })?;

    // Reshape the daemon's `{name, description, input_schema, read_only,
    // ...}` envelope into MCP's `{name, description, inputSchema,
    // annotations: {...}}` shape. Same translation
    // `ToolDef::to_list_entry` does for the in-process path so MCP
    // clients see identical tools/list output either way.
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
            serde_json::json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
                "annotations": {
                    "readOnlyHint": read_only,
                    "destructiveHint": destructive,
                    "idempotentHint": idempotent,
                    "openWorldHint": open_world,
                }
            })
        })
        .collect();

    Ok(serde_json::json!({ "tools": mcp_tools }))
}

/// JSON-RPC method dispatcher for the proxy. Mirrors
/// `mcp_server::server::handle_request`:
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
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, (**cached_tools_list).clone()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => forward_tool_call(id, call.name, call.args, socket_path).await,
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
///     true`. Mirrors the in-process `mcp_server::server` path so
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
) -> Response {
    let req = DaemonRequest {
        method: "call".into(),
        name: Some(name.clone()),
        args: Some(args),
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
        // level, so the in-process `mcp_server::server` would surface
        // it as `Response::ok` with `isError: true`. Mirror that
        // shape here so MCP clients see identical envelopes either
        // way — CodeRabbit #2.
        let msg = resp.error.unwrap_or_else(|| "daemon reported failure".into());
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
// (CUA_DRIVER_RS_MCP_FORCE_PROXY harness in PARITY.md §"Manual smoke
// test"); these tests just lock in the per-branch reshape so a
// regression to `Response::error` for tool-level failures would fail
// fast in CI on every platform.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::serve::DaemonResponse;

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
}
