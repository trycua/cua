//! `SpacesdClient::http`: raw HTTP to the spacesd port with the client's own
//! credentials, against a minimal loopback HTTP/1.1 server.

use std::time::Duration;

use cua_spacesd_client::{ConnectOptions, Error, HttpCall, SpacesdClient};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Serves exactly one request, returns its head, and answers `response`.
async fn one_shot(response: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // Bounded: the head plus a small body fits in 64 KiB.
        for _ in 0..16 {
            let n = socket.read(&mut chunk).await.unwrap();
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(head_end) = text.find("\r\n\r\n") {
                let length = text[..head_end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if buf.len() >= head_end + 4 + length {
                    break;
                }
            }
            assert!(
                buf.len() < 64 * 1024,
                "request too large for the test server"
            );
        }
        socket.write_all(&response).await.unwrap();
        socket.shutdown().await.ok();
        String::from_utf8_lossy(&buf).to_string()
    });
    (format!("http://{addr}"), task)
}

async fn client(url: &str, token: Option<&str>) -> SpacesdClient {
    let mut options = ConnectOptions::parse(url).unwrap().probe(false);
    if let Some(token) = token {
        options = options.token(token);
    }
    SpacesdClient::connect(options).await.unwrap()
}

#[tokio::test]
async fn sends_body_headers_and_the_env_token() {
    let (url, server) = one_shot(
        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nmcp-session-id: a\r\nmcp-session-id: b\r\ncontent-length: 2\r\n\r\n{}".to_vec(),
    )
    .await;
    let env = client(&url, Some("secret-token")).await;
    let reply = env
        .http(HttpCall {
            method: "POST".into(),
            path: "/mcp".into(),
            headers: vec![
                ("content-type".into(), "application/json".into()),
                ("x-test".into(), "1".into()),
                ("x-test".into(), "2".into()),
            ],
            body: br#"{"jsonrpc":"2.0"}"#.to_vec(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, b"{}");
    let sessions: Vec<_> = reply
        .headers
        .iter()
        .filter(|(k, _)| k == "mcp-session-id")
        .map(|(_, v)| v.as_str())
        .collect();
    assert_eq!(sessions, ["a", "b"]);
    let request = server.await.unwrap();
    let lower = request.to_ascii_lowercase();
    assert!(request.starts_with("POST /mcp HTTP/1.1\r\n"), "{request}");
    assert!(lower.contains("authorization: bearer secret-token\r\n"));
    assert!(lower.contains("x-test: 1\r\n") && lower.contains("x-test: 2\r\n"));
    assert!(request.ends_with(r#"{"jsonrpc":"2.0"}"#));
}

#[tokio::test]
async fn caller_cannot_override_credentials() {
    let env = client("http://127.0.0.1:9", Some("t")).await;
    let err = env
        .http(HttpCall {
            method: "POST".into(),
            path: "/mcp".into(),
            headers: vec![("authorization".into(), "Bearer other".into())],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Protocol(_)), "{err:?}");
}

#[tokio::test]
async fn oversized_response_fails_without_contents() {
    let body = "s".repeat(64);
    let (url, server) = one_shot(
        format!(
            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
        .into_bytes(),
    )
    .await;
    let env = client(&url, None).await;
    let err = env
        .http(HttpCall {
            method: "GET".into(),
            path: "/status".into(),
            max_response_bytes: Some(8),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("size limit"), "{err}");
    assert!(!err.to_string().contains(&body));
    let request = server.await.unwrap();
    assert!(!request.to_ascii_lowercase().contains("authorization"));
}

#[tokio::test]
async fn times_out() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    // Accept and hold the connection without answering.
    let hold = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
        drop(socket);
    });
    let env = client(&url, None).await;
    let err = env
        .http(HttpCall {
            method: "GET".into(),
            path: "/mcp".into(),
            timeout: Some(Duration::from_millis(200)),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Timeout(_)), "{err:?}");
    hold.abort();
}
