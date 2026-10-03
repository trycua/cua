//! MCP over stdio: newline-delimited JSON-RPC.
//!
//! A message is one line; a chunk boundary can fall anywhere, so input is
//! read line by line (never "the first line of a chunk", the framer bug
//! `FRICTION.md` §1 describes). Lines are bounded so a peer that never sends
//! a newline cannot grow memory without limit. Requests are handled
//! concurrently (a slow `create_space` does not block `ping`); responses are
//! written whole, one per line, and never interleave.

use super::{McpServer, codes, error};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};

/// Longest accepted message line (16 MiB).
pub const MAX_LINE_BYTES: usize = 16 * 1024 * 1024;

/// Serves until `input` reaches EOF. In-flight calls finish before return.
pub async fn serve<R, W>(server: McpServer, input: R, output: W) -> std::io::Result<()>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut output = output;
        while let Some(line) = rx.recv().await {
            output.write_all(line.as_bytes()).await?;
            output.write_all(b"\n").await?;
            output.flush().await?;
        }
        Ok::<_, std::io::Error>(())
    });
    let server = Arc::new(server);
    let mut reader = BufReader::new(input);
    let mut tasks = tokio::task::JoinSet::new();
    let mut line = Vec::new();
    loop {
        line.clear();
        let n = (&mut reader)
            .take(MAX_LINE_BYTES as u64 + 1)
            .read_until(b'\n', &mut line)
            .await?;
        if n == 0 {
            break;
        }
        if line.len() > MAX_LINE_BYTES && !line.ends_with(b"\n") {
            let _ = tx
                .send(error(Value::Null, codes::INVALID_REQUEST, "message too large").to_string());
            // Discard the rest of the oversized line, boundedly.
            let mut sink = Vec::new();
            let _ = (&mut reader)
                .take(MAX_LINE_BYTES as u64)
                .read_until(b'\n', &mut sink)
                .await;
            continue;
        }
        let text = String::from_utf8_lossy(&line);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                let _ = tx.send(error(Value::Null, codes::PARSE_ERROR, &e.to_string()).to_string());
                continue;
            }
        };
        let server = server.clone();
        let tx = tx.clone();
        tasks.spawn(async move {
            if let Some(response) = server.handle(message).await {
                let _ = tx.send(response.to_string());
            }
        });
        // Reap finished tasks so the set stays small.
        while tasks.try_join_next().is_some() {}
    }
    while tasks.join_next().await.is_some() {}
    drop(tx);
    writer.await.map_err(std::io::Error::other)??;
    Ok(())
}

/// Serves on the process's stdin/stdout. Diagnostics must go to stderr.
pub async fn serve_process_stdio(server: McpServer) -> std::io::Result<()> {
    serve(server, tokio::io::stdin(), tokio::io::stdout()).await
}
