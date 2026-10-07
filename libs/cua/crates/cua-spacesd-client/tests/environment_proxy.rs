// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Public-client routing contract. Each case has an isolated process environment.
use cua_spacesd_client::{
    ConnectOptions, Error, HttpCall, SpacesdClient, TransportPreference,
    testing::{MockAuth, MockServer},
};
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinSet,
};

const MODE: &str = "CUA_TEST_CONTROLLER_PROXY_MODE";
const ADDRESS: &str = "CUA_TEST_CONTROLLER_PROXY_ADDRESS";
const TOKEN: &str = "controller-fixture-canary";

#[test]
fn relay_rpc_uses_environment_proxy() {
    if std::env::var_os(MODE).is_none() {
        let mut failures = Vec::new();
        for mode in [
            "native",
            "grpc-web",
            "auto",
            "http",
            "refused",
            "config",
            "timeout",
            "cancel",
            "direct",
            "bypass",
            "tls-native",
            "tls-web",
            "tls-http",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "relay_rpc_uses_environment_proxy", "--nocapture"]);
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
            if mode != "direct" {
                child.env(
                    if mode.starts_with("tls-") {
                        "HTTPS_PROXY"
                    } else {
                        "HTTP_PROXY"
                    },
                    format!("http://{address}"),
                );
                if mode.starts_with("tls-") {
                    child.env("HTTP_PROXY", "http://invalid-http-proxy.invalid:9");
                }
            }
            if mode == "config" {
                child.env("HTTP_PROXY", "http://user:proxy-secret@proxy.invalid");
            }
            if mode == "bypass" {
                child.env("NO_PROXY", "127.0.0.0/8");
            }
            let output = child
                .env(MODE, mode)
                .env(ADDRESS, address.to_string())
                .output()
                .unwrap();
            if !output.status.success() {
                failures.push(format!(
                    "{mode}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(run_case());
}

async fn head(socket: &mut TcpStream) -> String {
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        assert!(head.len() < 4096);
        head.push(socket.read_u8().await.unwrap());
    }
    String::from_utf8(head).unwrap()
}

async fn run_case() {
    let mode = std::env::var(MODE).unwrap();
    let mock = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        prefix: Some("/m/proxy-machine".into()),
        ..Default::default()
    })
    .await;
    let proxy = TcpListener::bind(std::env::var(ADDRESS).unwrap())
        .await
        .unwrap();
    let upstream = mock.addr;
    let attempts = Arc::new(AtomicUsize::new(0));
    let accepted = Arc::new(tokio::sync::Notify::new());
    let closed = Arc::new(tokio::sync::Notify::new());
    let tls_rejected = Arc::new(tokio::sync::Notify::new());
    let proxy_task = {
        let mode = mode.clone();
        let attempts = attempts.clone();
        let accepted = accepted.clone();
        let closed = closed.clone();
        let tls_rejected = tls_rejected.clone();
        tokio::spawn(async move {
            let mut connections = JoinSet::new();
            loop {
                tokio::select! {
                    socket = proxy.accept() => {
                        let (mut socket, _) = socket.unwrap();
                        let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                        let mode = mode.clone(); let accepted = accepted.clone(); let closed = closed.clone(); let tls_rejected = tls_rejected.clone();
                        connections.spawn(async move {
                            let connect = head(&mut socket).await;
                            let target = if mode == "refused" { upstream.to_string() } else if mode.starts_with("tls-") { "relay.invalid:443".into() } else { "relay.invalid:80".into() };
                            assert_eq!(connect, format!("CONNECT {target} HTTP/1.1\r\nhost: {target}\r\n\r\n"));
                            assert!(!connect.contains(TOKEN));
                            if mode == "refused" || (mode == "auto" && attempt == 0) {
                                socket.write_all(b"HTTP/1.1 403 private-reason\r\nContent-Length: 999\r\n\r\n").await.unwrap();
                                return;
                            }
                            if mode == "timeout" || mode == "cancel" {
                                accepted.notify_one();
                                let mut bytes = Vec::new();
                                tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut bytes)).await.unwrap().unwrap();
                                assert!(bytes.is_empty());
                                closed.notify_one();
                                return;
                            }
                            socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
                            if mode.starts_with("tls-") {
                                let identity = rcgen::generate_simple_self_signed(vec!["relay.invalid".into()]).unwrap();
                                let key = rustls::pki_types::PrivatePkcs8KeyDer::from(identity.signing_key.serialize_der());
                                let config = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                                    .with_safe_default_protocol_versions().unwrap().with_no_client_auth()
                                    .with_single_cert(vec![identity.cert.der().clone()], key.into()).unwrap();
                                let hello = tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), socket).await.unwrap();
                                assert_eq!(hello.client_hello().server_name(), Some("relay.invalid"));
                                let alpn: Vec<_> = hello.client_hello().alpn().into_iter().flatten().collect();
                                if mode == "tls-native" { assert_eq!(alpn, vec![&b"h2"[..]]); }
                                else { assert!(alpn.is_empty()); } // Existing HTTP/1-only builder omits ALPN.
                                let error = hello.into_stream(Arc::new(config)).await.expect_err("untrusted origin must be rejected");
                                assert!(error.to_string().contains("UnknownCA"), "{error:?}");
                                tls_rejected.notify_one();
                            } else if mode == "http" {
                                let request = head(&mut socket).await;
                                assert!(request.starts_with("GET /m/proxy-machine/mcp?fixture=1 HTTP/1.1\r\n"));
                                assert!(request.contains(&format!("authorization: Bearer {TOKEN}\r\n")));
                                socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}").await.unwrap();
                            } else {
                                let mut origin = TcpStream::connect(upstream).await.unwrap();
                                let _ = tokio::io::copy_bidirectional(&mut socket, &mut origin).await;
                            }
                        });
                    },
                    completed = connections.join_next(), if !connections.is_empty() => { completed.unwrap().unwrap(); }
                }
            }
        })
    };
    let endpoint = if matches!(mode.as_str(), "direct" | "bypass" | "refused" | "config") {
        format!("http://{upstream}/m/proxy-machine")
    } else if mode.starts_with("tls-") {
        "https://relay.invalid/m/proxy-machine".into()
    } else {
        "http://relay.invalid/m/proxy-machine".into()
    };
    let preference = match mode.as_str() {
        "grpc-web" | "tls-web" => TransportPreference::GrpcWeb,
        "auto" | "refused" | "config" => TransportPreference::Auto,
        _ => TransportPreference::Native,
    };
    let mut options = ConnectOptions::parse(&endpoint)
        .unwrap()
        .token(TOKEN)
        .transport(preference)
        .probe(!matches!(mode.as_str(), "http" | "tls-http"))
        .probe_timeout(Duration::from_secs(2));
    options.connect_timeout = if mode == "timeout" {
        Duration::from_millis(100)
    } else {
        Duration::from_secs(2)
    };
    if mode == "cancel" {
        let task = tokio::spawn(SpacesdClient::connect(options));
        tokio::time::timeout(Duration::from_secs(2), accepted.notified())
            .await
            .unwrap();
        task.abort();
        let _ = task.await;
        // Tonic owns its connection worker independently of the cancelled RPC.
        // Its outstanding CONNECT must still end at the configured two-second budget.
        tokio::time::timeout(Duration::from_secs(3), closed.notified())
            .await
            .expect("cancelled RPC must not leave an unbounded connection attempt");
    } else {
        let result = SpacesdClient::connect(options).await;
        if matches!(
            mode.as_str(),
            "refused" | "config" | "timeout" | "tls-native" | "tls-web"
        ) {
            assert!(
                matches!(&result, Err(Error::SpacesdNotAvailable { .. })),
                "{result:?}"
            );
            let diagnostic = format!("{result:?}");
            for secret in [TOKEN, "proxy-secret", "private-reason"] {
                assert!(!diagnostic.contains(secret));
            }
            if mode == "timeout" {
                tokio::time::timeout(Duration::from_secs(1), closed.notified())
                    .await
                    .unwrap();
            }
            assert_eq!(mock.state.observed.grpc_requests.load(Ordering::SeqCst), 0);
            assert_eq!(
                mock.state.observed.grpc_web_requests.load(Ordering::SeqCst),
                0
            );
        } else {
            let client = result.expect("client must use its selected route");
            if mode == "tls-http" {
                let error = client
                    .http(HttpCall {
                        method: "GET".into(),
                        path: "/mcp".into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap_err();
                assert!(matches!(error, Error::Transport(_)), "{error:?}");
            } else if mode == "http" {
                let reply = client
                    .http(HttpCall {
                        method: "GET".into(),
                        path: "/mcp?fixture=1".into(),
                        ..Default::default()
                    })
                    .await
                    .unwrap();
                assert_eq!(reply.body, b"{}");
            } else {
                assert_eq!(client.capabilities().await.unwrap().protocol_version, 1);
                assert_eq!(
                    client
                        .run("echo proxy-ok")
                        .await
                        .unwrap()
                        .stdout_str()
                        .trim(),
                    "proxy-ok"
                );
            }
            if mode == "auto" {
                assert_eq!(mock.state.observed.grpc_requests.load(Ordering::SeqCst), 0);
                assert!(mock.state.observed.grpc_web_requests.load(Ordering::SeqCst) > 0);
                assert_eq!(attempts.load(Ordering::SeqCst), 2);
            }
            drop(client);
        }
    }
    if matches!(mode.as_str(), "direct" | "bypass" | "config") {
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
    } else {
        assert!(attempts.load(Ordering::SeqCst) > 0);
    }
    if mode.starts_with("tls-") {
        tokio::time::timeout(Duration::from_secs(2), tls_rejected.notified())
            .await
            .unwrap();
    }
    drop(mock);
    assert!(!proxy_task.is_finished(), "proxy fixture failed");
    proxy_task.abort();
    let _ = proxy_task.await;
}
