// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Real machine join through a forward proxy. Environment is confined to a child
//! test process; no production injection switch or global environment mutation.
use std::{process::Command, time::Duration};

use cua_relay::{
    client::{JoinConfig, SessionEnd},
    server::{Relay, RelayConfig},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "machine-secret-canary";
const ID: &str = "proxy-machine";
const CHILD: &str = "CUA_RELAY_PROXY_TEST_CHILD";

#[test]
fn machine_join_uses_http_proxy() {
    if std::env::var_os(CHILD).is_none() {
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args(["--exact", "machine_join_uses_http_proxy", "--nocapture"]);
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
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = proxy.local_addr().unwrap();
        drop(proxy);
        let result = child
            .env(CHILD, addr.to_string())
            .env("HTTP_PROXY", format!("http://{addr}"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let relay = Relay::new(RelayConfig { tokens: vec![TOKEN.into()], ..RelayConfig::default() });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let serving = relay.clone();
        let relay_task = tokio::spawn(async move { serving.serve(listener, std::future::pending()).await });
        let proxy = TcpListener::bind(std::env::var(CHILD).unwrap()).await.unwrap();
        let proxy_task = tokio::spawn(async move {
            let (mut inbound, _) = proxy.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(inbound.read_u8().await.unwrap());
                assert!(head.len() < 4096);
            }
            let head = String::from_utf8(head).unwrap();
            assert!(head.starts_with("CONNECT relay.invalid:80 HTTP/1.1\r\n"), "{head}");
            assert!(!head.contains(TOKEN));
            assert!(!head.to_ascii_lowercase().contains("authorization"));
            let mut upstream = TcpStream::connect(addr).await.unwrap();
            // Every status/header boundary may be fragmented by the network.
            for byte in b"HTTP/1.1 200 Connection established\r\n\r\n" {
                inbound.write_all(&[*byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
            tokio::io::copy_bidirectional(&mut inbound, &mut upstream).await.unwrap();
        });
        let local = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        let local_task = tokio::spawn(async move {
            axum::serve(local, axum::Router::new().route("/health", axum::routing::get(|| async { "through-machine" }))).await.unwrap();
        });
        let config = JoinConfig::new("ws://relay.invalid".into(), TOKEN.into(), ID.into(), local_addr);
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let mut join = tokio::spawn(async move { cua_relay::client::session(&config, &stopped).await });
        let online = async {
            while !relay.machine_ids().contains(&ID.to_string()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::select! {
            ended = &mut join => panic!("join ended before becoming online: {ended:?}"),
            result = tokio::time::timeout(Duration::from_secs(5), online) => result.expect("proxy join never became online"),
        }
        let response = reqwest::Client::builder().no_proxy().build().unwrap()
            .get(format!("http://{addr}/m/{ID}/health")).bearer_auth(TOKEN)
            .send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.text().await.unwrap(), "through-machine");
        stop.cancel();
        assert!(matches!(join.await.unwrap(), SessionEnd::Shutdown));
        // Established-session mux task lifetime is existing client behavior.
        // Partial CONNECT cancellation is covered in transport's unit tests.
        proxy_task.abort();
        local_task.abort();
        relay_task.abort();
    });
}

#[test]
fn proxy_tunnel_keeps_native_tls_verification() {
    if std::env::var_os(CHILD).is_none() {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = proxy.local_addr().unwrap();
        drop(proxy);
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "proxy_tunnel_keeps_native_tls_verification",
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
        let result = child
            .env(CHILD, addr.to_string())
            .env("HTTPS_PROXY", format!("http://{addr}"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let proxy = TcpListener::bind(std::env::var(CHILD).unwrap())
            .await
            .unwrap();
        // This is a public test-only identity, never installed in a trust store.
        let identity = native_tls::Identity::from_pkcs8(
            include_bytes!("fixtures/untrusted-cert.pem"),
            include_bytes!("fixtures/untrusted-key.pem"),
        )
        .unwrap();
        let tls =
            tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap());
        let proxy_task = tokio::spawn(async move {
            let (mut stream, _) = proxy.accept().await.unwrap();
            let mut head = Vec::new();
            while !head.ends_with(b"\r\n\r\n") {
                head.push(stream.read_u8().await.unwrap());
            }
            assert!(String::from_utf8(head)
                .unwrap()
                .starts_with("CONNECT relay.invalid:443 HTTP/1.1\r\n"));
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            assert!(
                tls.accept(stream).await.is_err(),
                "untrusted origin unexpectedly accepted"
            );
        });
        let config = JoinConfig::new(
            "wss://relay.invalid/?origin-secret-canary".into(),
            TOKEN.into(),
            ID.into(),
            "127.0.0.1:1".parse().unwrap(),
        );
        let end = tokio::time::timeout(
            Duration::from_secs(5),
            cua_relay::client::session(&config, &CancellationToken::new()),
        )
        .await
        .unwrap();
        assert!(
            matches!(&end, SessionEnd::Lost(reason) if reason == "origin TLS failed"),
            "{end:?}"
        );
        assert!(!format!("{end:?}").contains("canary"));
        proxy_task.await.unwrap();
    });
}

#[test]
#[allow(clippy::result_large_err)] // tungstenite callback fixes the error response type.
fn reconnect_through_proxy_rereads_token_and_checks_pinned_keys() {
    if std::env::var_os(CHILD).is_none() {
        let proxy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = proxy.local_addr().unwrap();
        drop(proxy);
        let mut child = Command::new(std::env::current_exe().unwrap());
        child.args([
            "--exact",
            "reconnect_through_proxy_rereads_token_and_checks_pinned_keys",
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
        let result = child
            .env(CHILD, addr.to_string())
            .env("HTTP_PROXY", format!("http://{addr}"))
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        use base64::Engine as _;
        use cua_relay::assertion::{RelayKey, JWKS_HEADER, OWNER_HEADER};
        let proxy = TcpListener::bind(std::env::var(CHILD).unwrap())
            .await
            .unwrap();
        let files = tempfile::tempdir().unwrap();
        let token_path = files.path().join("token");
        std::fs::write(&token_path, "first-token").unwrap();
        let mut config = JoinConfig::new(
            "ws://relay.invalid".into(),
            "stale-token".into(),
            ID.into(),
            "127.0.0.1:1".parse().unwrap(),
        );
        config.relay_token_file = Some(token_path.clone());
        config.max_backoff = Duration::from_millis(10);
        let pinned = RelayKey::generate();
        config
            .account
            .pin_jwks_json(&pinned.jwks().to_string())
            .unwrap();
        let account = config.account.clone();
        let header =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pinned.jwks().to_string());
        let stop = CancellationToken::new();
        let stopped = stop.clone();
        let run = tokio::spawn(cua_relay::client::run(config, stopped));
        let test = async {
            for token in ["first-token", "second-token"] {
                let (mut stream, _) = proxy.accept().await.unwrap();
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(stream.read_u8().await.unwrap());
                }
                assert!(!String::from_utf8(head).unwrap().contains("token"));
                stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
                let mut socket = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |request: &http::Request<()>, mut response: http::Response<()>| {
                        assert_eq!(
                            request.headers()["authorization"],
                            format!("Bearer {token}")
                        );
                        assert_eq!(request.headers()[cua_relay::MACHINE_ID_HEADER], ID);
                        response
                            .headers_mut()
                            .insert(JWKS_HEADER, header.parse().unwrap());
                        response
                            .headers_mut()
                            .insert(OWNER_HEADER, "proxy-test-owner".parse().unwrap());
                        Ok(response)
                    },
                )
                .await
                .unwrap();
                std::fs::write(&token_path, "second-token").unwrap();
                // Wait until session processed the key-bearing 101 before closing.
                while account.owner().as_deref() != Some("proxy-test-owner") {
                    tokio::task::yield_now().await;
                }
                socket.close(None).await.unwrap();
            }
        };
        tokio::time::timeout(Duration::from_secs(5), test)
            .await
            .unwrap();
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .unwrap()
            .unwrap();
    });
}
