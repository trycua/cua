// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Worst-case buffering per client stream, to size
//! `CUA_RELAY_MAX_GLOBAL_STREAMS` against the pod's memory: one machine
//! whose local service answers every request with an endless body, and
//! `count` clients that open a request through the relay and never read.
//! Watch the relay's `/metrics` (heap and resident bytes) while it holds.
//!
//! ```text
//! cargo run --release --example slow_streams -- http://127.0.0.1:18080 reg 200
//! ```

use std::time::Duration;

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "http://127.0.0.1:18080".into());
    let token = args.next().unwrap_or_else(|| "reg".into());
    let count: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(200);
    let hold: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    // The machine's local service: an endless response to every request.
    let backend = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local = backend.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = backend.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf).await;
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ntransfer-encoding: chunked\r\n\r\n")
                    .await;
                let chunk = vec![b'x'; 64 * 1024];
                let head = format!("{:x}\r\n", chunk.len());
                loop {
                    if s.write_all(head.as_bytes()).await.is_err()
                        || s.write_all(&chunk).await.is_err()
                        || s.write_all(b"\r\n").await.is_err()
                    {
                        return;
                    }
                }
            });
        }
    });

    let id = format!("slowstreams{}", "0".repeat(21));
    let join = cua_relay::client::JoinConfig::new(
        url.replace("http://", "ws://"),
        token,
        id.clone(),
        local,
    );
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Clients that send a request and never read the answer.
    let host = url.trim_start_matches("http://").to_owned();
    let mut held = Vec::new();
    for _ in 0..count {
        if let Ok(mut s) = tokio::net::TcpStream::connect(&host).await {
            let req = format!("GET /m/{id}/ HTTP/1.1\r\nhost: {host}\r\n\r\n");
            let _ = s.write_all(req.as_bytes()).await;
            held.push(s);
        }
    }
    println!("SLOW holding {} streams for {hold}s", held.len());
    tokio::time::sleep(Duration::from_secs(hold)).await;
    drop(held);
    stop.cancel();
}
