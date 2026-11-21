// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A computer whose token the relay does not know (removed, rotated, account
//! deleted) must stop knocking: after one more try it waits long, and a new
//! token in its token file ends the wait.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use cua_relay::client::{run, JoinConfig};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

/// A relay that answers every join with 401 and records the bearer it saw.
async fn reject_all(hits: Arc<AtomicUsize>, tokens: Arc<std::sync::Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                match stream.read_u8().await {
                    Ok(b) => head.push(b),
                    Err(_) => break,
                }
            }
            let text = String::from_utf8_lossy(&head).to_string();
            if let Some(line) = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
            {
                tokens.lock().unwrap().push(line.to_owned());
            }
            hits.fetch_add(1, Ordering::SeqCst);
            let _ = stream
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                )
                .await;
        }
    });
    url
}

fn config(url: String, file: Option<std::path::PathBuf>) -> JoinConfig {
    let mut c = JoinConfig::new(
        url,
        "stale".into(),
        "0123456789abcdef0123456789abcdef".into(),
        "127.0.0.1:1".parse().unwrap(),
    );
    c.max_backoff = Duration::from_millis(50);
    c.min_reauth_backoff = Duration::from_secs(30);
    c.max_reauth_backoff = Duration::from_secs(30);
    c.relay_token_file = file;
    c
}

#[tokio::test]
async fn a_rejected_token_is_retried_once_then_rarely() {
    let hits = Arc::new(AtomicUsize::new(0));
    let url = reject_all(hits.clone(), Arc::default()).await;
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(config(url, None), stop.clone()));
    tokio::time::sleep(Duration::from_secs(3)).await;
    stop.cancel();
    task.await.unwrap();
    // The first try, one quick retry, then a wait of at least 15 s. With the
    // old 50 ms backoff this would be dozens.
    let n = hits.load(Ordering::SeqCst);
    assert!((2..=3).contains(&n), "{n} joins in 3 s");
}

#[tokio::test]
async fn a_new_token_in_the_file_ends_the_wait() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    std::fs::write(&file, "old-token").unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let url = reject_all(hits.clone(), seen.clone()).await;
    let stop = CancellationToken::new();
    let task = tokio::spawn(run(config(url, Some(file.clone())), stop.clone()));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let before = hits.load(Ordering::SeqCst);
    std::fs::write(&file, "new-token").unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    stop.cancel();
    task.await.unwrap();
    assert!(
        hits.load(Ordering::SeqCst) > before,
        "no retry with the new token"
    );
    assert!(seen.lock().unwrap().iter().any(|l| l.contains("new-token")));
}
