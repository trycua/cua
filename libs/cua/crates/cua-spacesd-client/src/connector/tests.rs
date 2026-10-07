// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

#[test]
fn environment_selection_and_explicit_bypass() {
    let route = |url: &str, values: &[(&str, &str)]| {
        selected_proxy(&url.parse().unwrap(), |key| {
            values
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        })
        .map(|v| v.map(|v| v.to_string()))
    };
    let env = [
        ("HTTPS_PROXY", "http://secure:81"),
        ("https_proxy", "http://ignored:82"),
        ("HTTP_PROXY", "http://plain:83"),
        ("ALL_PROXY", "fallback:84"),
    ];
    assert_eq!(
        route("https://relay.invalid/", &env),
        Ok(Some("http://secure:81/".into()))
    );
    assert_eq!(
        route("http://relay.invalid/", &env),
        Ok(Some("http://plain:83/".into()))
    );
    assert_eq!(
        route(
            "https://relay.invalid/",
            &[
                ("HTTPS_PROXY", ""),
                ("https_proxy", "ignored:80"),
                ("ALL_PROXY", "fallback:84")
            ]
        ),
        Ok(Some("http://fallback:84/".into()))
    );
    assert_eq!(
        route("https://relay.invalid/", &[("https_proxy", "proxy:80")]),
        Ok(Some("http://proxy/".into()))
    );
    assert_eq!(route("https://relay.invalid/", &[]), Ok(None));
    assert_eq!(
        route("https://relay.invalid/", &[("HTTP_PROXY", "proxy:80")]),
        Ok(None)
    );
    for (origin, no) in [
        ("https://relay.invalid/", "relay.invalid"),
        ("https://sub.relay.invalid/", ".relay.invalid"),
        ("http://127.0.0.1/", "127.0.0.0/8"),
        ("https://[::1]/", "::1"),
        ("https://relay.invalid/", "*"),
    ] {
        assert_eq!(
            route(
                origin,
                &[
                    ("ALL_PROXY", "http://secret@proxy/#fragment"),
                    ("NO_PROXY", no)
                ]
            ),
            Ok(None)
        );
    }
    assert_eq!(
        route(
            "https://relay.invalid/",
            &[
                ("ALL_PROXY", "proxy:80"),
                ("NO_PROXY", ""),
                ("no_proxy", "*")
            ]
        ),
        Ok(Some("http://proxy/".into()))
    );
    assert_eq!(
        route("http://localhost/", &[("HTTP_PROXY", "proxy:80")]),
        Ok(Some("http://proxy/".into()))
    );
    assert_eq!(
        route(
            "https://relay.invalid/",
            &[("HTTPS_PROXY", "proxy:80"), ("REQUEST_METHOD", "GET")]
        ),
        Err("proxy use in a CGI environment is unsupported")
    );
}

#[test]
fn proxy_configuration_is_strict_and_redacted() {
    for (value, expected) in [
        (
            "http://proxy/#secret",
            "proxy URL fragments are unsupported",
        ),
        ("http://proxy#secret", "proxy URL fragments are unsupported"),
        (
            "http://user:secret@proxy",
            "proxy authentication is unsupported",
        ),
        ("http://@proxy", "proxy authentication is unsupported"),
        ("https://proxy", "only HTTP proxies are supported"),
        ("socks5://proxy", "only HTTP proxies are supported"),
        ("http://proxy/path", "invalid proxy authority or path"),
        ("http://proxy/?secret", "invalid proxy authority or path"),
        ("http://proxy/../", "invalid proxy authority or path"),
        ("http://proxy\\secret", "invalid proxy authority or path"),
        ("http://proxy ", "invalid proxy authority or path"),
        ("http://proxy:65536", "invalid proxy URL"),
        ("http://proxy:0", "invalid proxy authority or port"),
        ("http://proxy:", "invalid proxy authority or port"),
        ("http://", "invalid proxy URL"),
    ] {
        assert_eq!(validate_proxy(value), Err(expected), "case {value}");
    }
    for (value, expected) in [
        ("proxy:8080", "http://proxy:8080/"),
        ("http://[::1]:3128/", "http://[::1]:3128/"),
    ] {
        assert_eq!(validate_proxy(value).unwrap().to_string(), expected);
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let result = selected_proxy(&"https://relay.invalid".parse().unwrap(), |key| {
            (key == "HTTPS_PROXY").then(|| OsString::from_vec(vec![0xff]))
        });
        assert_eq!(result, Err("proxy URL is not Unicode"));
    }
}

async fn fixture(response: &'static [u8]) -> (TokioIo<TcpStream>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut head = Vec::new();
        while !head.ends_with(b"\r\n\r\n") {
            head.push(socket.read_u8().await.unwrap());
            assert!(head.len() < 4096);
        }
        assert_eq!(
            String::from_utf8(head).unwrap(),
            "CONNECT [::1]:443 HTTP/1.1\r\nhost: [::1]:443\r\n\r\n"
        );
        // Force a status-line split rather than relying on TCP packet boundaries.
        socket.write_all(&response[..1]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        socket.write_all(&response[1..]).await.unwrap();
        let mut end = Vec::new();
        socket.read_to_end(&mut end).await.unwrap();
        assert!(end.is_empty());
    });
    (TokioIo::new(socket), server)
}

#[tokio::test]
async fn fragmented_connect_preserves_read_ahead_and_closes() {
    let (socket, server) = fixture(b"HTTP/1.1 200 OK\r\n\r\ntunneled-prefix").await;
    let stream = connect_tunnel(
        socket,
        &"https://[::1]/secret?token=secret".parse().unwrap(),
    )
    .await
    .unwrap();
    let mut stream = TokioIo::new(stream);
    let mut prefix = [0; 15];
    stream.read_exact(&mut prefix).await.unwrap();
    assert_eq!(&prefix, b"tunneled-prefix");
    drop(stream);
    tokio::time::timeout(Duration::from_secs(2), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn refusal_does_not_wait_for_body_and_redacts_response() {
    for (response, expected) in [
        (&b"HTTP/1.1 407 secret-reason\r\nProxy-Authenticate: secret-challenge\r\nContent-Length: 999\r\n\r\n"[..], "proxy authentication required: HTTP 407"),
        (&b"HTTP/1.1 403 secret-reason\r\nContent-Length: 999\r\n\r\n"[..], "proxy CONNECT refused: HTTP 403"),
    ] {
        let (socket, server) = fixture(response).await;
        let result = tokio::time::timeout(Duration::from_secs(2), connect_tunnel(socket, &"https://[::1]/".parse().unwrap())).await.unwrap();
        assert_eq!(result.err().unwrap().to_string(), expected);
        tokio::time::timeout(Duration::from_secs(2), server).await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn dropping_connect_future_closes_the_owned_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let socket = TcpStream::connect(listener.local_addr().unwrap())
        .await
        .unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    let task = tokio::spawn(async move {
        connect_tunnel(
            TokioIo::new(socket),
            &"https://relay.invalid/".parse().unwrap(),
        )
        .await
    });
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(peer.read_u8().await.unwrap());
    }
    task.abort();
    let _ = task.await;
    let mut remaining = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut remaining))
        .await
        .unwrap()
        .unwrap();
    assert!(remaining.is_empty());
}
