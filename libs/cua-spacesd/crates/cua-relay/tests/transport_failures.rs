//! Exercise the actual session route and cancellation owner, with reachable
//! origin listeners that detect any unintended direct fallback.
use cua_relay::client::JoinConfig;
use std::{process::Command, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

const CHILD: &str = "CUA_TEST_TRANSPORT_FAILURE";
const ADDRESS: &str = "CUA_TEST_PROXY_ADDRESS";

#[test]
fn session_failure_and_cancellation_contract() {
    if std::env::var_os(CHILD).is_none() {
        for mode in [
            "proxy407",
            "proxy403",
            "proxy-eof",
            "proxy-dial",
            "bad-config",
            "ws-invalid",
            "ws403",
            "ws-eof",
            "cancel-connect",
            "cancel-tls",
            "cancel-ws",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args([
                "--exact",
                "session_failure_and_cancellation_contract",
                "--nocapture",
            ]);
            for key in [
                "HTTP_PROXY",
                "http_proxy",
                "HTTPS_PROXY",
                "https_proxy",
                "ALL_PROXY",
                "all_proxy",
                "NO_PROXY",
                "no_proxy",
                "REQUEST_METHOD",
            ] {
                child.env_remove(key);
            }
            let proxy = if mode == "bad-config" {
                "http://secret-user:secret-password@proxy.invalid".to_owned()
            } else {
                format!("http://{address}")
            };
            let output = child
                .env(CHILD, mode)
                .env(ADDRESS, address.to_string())
                .env("HTTP_PROXY", &proxy)
                .env("HTTPS_PROXY", &proxy)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mode = std::env::var(CHILD).unwrap();
        let origin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = origin.local_addr().unwrap();
        let proxy = TcpListener::bind(std::env::var(ADDRESS).unwrap()).await.unwrap();
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let (ready, waiting) = tokio::sync::oneshot::channel();
        let scenario = mode.clone();
        let mut server = Some(tokio::spawn(async move {
            if matches!(scenario.as_str(), "proxy-dial" | "bad-config") {
                drop(proxy);
                ready.send(()).unwrap();
                return;
            }
            // Signal readiness only after the listener exists.
            let (mut socket, _) = proxy.accept().await.unwrap();
            let connect = read_head(&mut socket).await;
            assert!(connect.starts_with(&format!("CONNECT {address} HTTP/1.1\r\n")));
            assert!(!connect.to_ascii_lowercase().contains("authorization"));
            match scenario.as_str() {
                "proxy407" => socket.write_all(b"HTTP/1.1 407 secret-reason\r\nProxy-Authenticate: secret-challenge\r\nContent-Length: 10000\r\n\r\nsecret-body").await.unwrap(),
                "proxy403" => socket.write_all(b"HTTP/1.1 403 secret-reason\r\nContent-Length: 10000\r\n\r\nsecret-body").await.unwrap(),
                "proxy-eof" => { socket.write_all(b"HTTP/1.1 20").await.unwrap(); return; }
                "cancel-connect" => socket.write_all(b"HTTP/1.1 200").await.unwrap(),
                _ => {
                    socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
                    if scenario == "cancel-tls" {
                        let mut hello = [0; 4096];
                        assert!(socket.read(&mut hello).await.unwrap() > 0);
                        assert_eq!(hello[0], 22, "expected TLS handshake record");
                    } else {
                        let ws = read_head(&mut socket).await;
                        assert!(ws.contains("Bearer origin-token-canary"));
                        match scenario.as_str() {
                            "ws-invalid" => socket.write_all(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: secret-invalid-accept\r\n\r\n").await.unwrap(),
                            "ws403" => socket.write_all(b"HTTP/1.1 403 secret-reason\r\nContent-Length: 11\r\n\r\nsecret-body").await.unwrap(),
                            "ws-eof" => return,
                            "cancel-ws" => {},
                            _ => unreachable!(),
                        }
                    }
                }
            }
            if scenario.starts_with("cancel-") { ready.send(()).unwrap(); }
            // Rejection/cancellation must release the socket owned by the dial.
            let mut rest = Vec::new();
            let result = tokio::time::timeout(Duration::from_secs(2), socket.read_to_end(&mut rest)).await.expect("handshake socket survived session completion");
            assert!(result.is_ok() || result.unwrap_err().kind() == std::io::ErrorKind::ConnectionReset);
        }));
        if matches!(mode.as_str(), "proxy-dial" | "bad-config") { server.take().unwrap().await.unwrap(); }
        let scheme = if mode == "cancel-tls" { "wss" } else { "ws" };
        let config = JoinConfig::new(format!("{scheme}://{address}/?url-secret-canary"), "origin-token-canary".into(), "failure-machine".into(), "127.0.0.1:1".parse().unwrap());
        let session = tokio::spawn(async move { cua_relay::client::session(&config, &stopped).await });
        if mode.starts_with("cancel-") {
            tokio::time::timeout(Duration::from_secs(2), waiting).await.unwrap().unwrap();
            stop.cancel();
        }
        let end = tokio::select! {
            result = tokio::time::timeout(Duration::from_secs(3), session) => result.expect("session did not finish").unwrap(),
            _ = origin.accept() => panic!("selected proxy route opened a direct origin connection"),
        };
        let expected = match mode.as_str() {
            "proxy407" => "Lost(\"proxy authentication required: HTTP 407\")",
            "proxy403" => "Lost(\"proxy CONNECT refused: HTTP 403 Forbidden\")",
            "proxy-eof" => "Lost(\"proxy CONNECT failed\")",
            "proxy-dial" => "Lost(\"proxy dial failed\")",
            "bad-config" => "Refused(\"proxy configuration: proxy authentication is unsupported\")",
            "ws-invalid" | "ws-eof" => "Lost(\"WebSocket upgrade failed\")",
            "ws403" => "Refused(\"HTTP 403 Forbidden\")",
            _ => "Shutdown",
        };
        assert_eq!(format!("{end:?}"), expected);
        assert!(!format!("{end:?}").contains("secret"));
        assert!(!format!("{end:?}").contains("canary"));
        assert!(tokio::time::timeout(Duration::from_millis(50), origin.accept()).await.is_err(), "selected proxy route fell back to a directly reachable origin");
        if let Some(server) = server { server.await.unwrap(); }
    });
}

async fn read_head(socket: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    while !bytes.ends_with(b"\r\n\r\n") {
        bytes.push(socket.read_u8().await.unwrap());
        assert!(bytes.len() < 8192);
    }
    String::from_utf8(bytes).unwrap()
}
