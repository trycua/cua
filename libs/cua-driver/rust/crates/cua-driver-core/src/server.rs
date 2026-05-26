//! Async MCP stdio server loop.

use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, error, warn};

use crate::protocol::{initialize_result, Request, Response};
use crate::tool::ToolRegistry;

/// Run the MCP server, reading JSON-RPC lines from stdin and writing
/// responses to stdout. Exits when stdin reaches EOF or a fatal I/O
/// error occurs.
pub async fn run(registry: Arc<ToolRegistry>) -> anyhow::Result<()> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut writer = tokio::io::BufWriter::new(stdout);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            // EOF
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        debug!(raw = trimmed, "→ request");

        let Some(response) = (match serde_json::from_str::<Request>(trimmed) {
            Err(e) => {
                error!("JSON parse error: {e}");
                Some(Response::parse_error())
            }
            Ok(req) => handle_request(req, &registry).await,
        }) else {
            // Notifications are silently dropped.
            continue;
        };

        let serialized = serde_json::to_string(&response).unwrap_or_else(|e| {
            format!(
                r#"{{"jsonrpc":"2.0","id":null,"error":{{"code":-32603,"message":"serialize error: {e}"}}}}"#
            )
        });
        debug!(raw = %serialized, "← response");

        writer.write_all(serialized.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
    }

    Ok(())
}

/// Handle one parsed MCP JSON-RPC request against a registry.
///
/// Returns `None` for notifications, matching the stdio server behavior.
/// Embedders can use this to expose cua-driver over an in-process transport
/// without going through stdin/stdout or launching the standalone driver.
pub async fn handle_request(req: Request, registry: &Arc<ToolRegistry>) -> Option<Response> {
    if req.is_notification() {
        return None;
    }
    let id = req.id.clone().unwrap_or(serde_json::Value::Null);
    Some(dispatch_request(req, id, registry).await)
}

async fn dispatch_request(
    req: Request,
    id: serde_json::Value,
    registry: &Arc<ToolRegistry>,
) -> Response {
    match req.method.as_str() {
        "initialize" => Response::ok(id, initialize_result()),

        "tools/list" => Response::ok(id, registry.tools_list()),

        "tools/call" => match req.tool_call() {
            Err(e) => Response::error(id, -32602, format!("Invalid params: {e}")),
            Ok(call) => {
                let result = registry.invoke(&call.name, call.args).await;
                match serde_json::to_value(result) {
                    Ok(v) => Response::ok(id, v),
                    Err(e) => Response::error(id, -32603, format!("Serialize error: {e}")),
                }
            }
        },

        other => {
            warn!(method = other, "unknown method");
            Response::method_not_found(id, other)
        }
    }
}
