// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Managed TLS trust through the public client and a real HTTP CONNECT proxy.
use base64::Engine;
use cua_spacesd_client::{
    ConnectOptions, HttpCall, SpacesdClient, TransportPreference,
    testing::{MockAuth, MockServer},
};
use std::{path::Path, process::Command, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

const MODE: &str = "CUA_TEST_NATIVE_ROOTS";
const FIXTURE: &str = "CUA_TEST_NATIVE_ROOTS_FIXTURE";
const TOKEN: &str = "native-roots-fixture-canary";

fn pem(der: &[u8]) -> String {
    format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64::engine::general_purpose::STANDARD.encode(der)
    )
}

fn fixture(path: &Path, trust: &str) {
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    ca.key_usages = vec![rcgen::KeyUsagePurpose::KeyCertSign];
    let key = rcgen::KeyPair::generate().unwrap();
    let root = ca.self_signed(&key).unwrap();
    let issuer = rcgen::Issuer::new(ca, key);
    let host = if trust == "wrong-host" {
        "other.invalid"
    } else {
        "relay.invalid"
    };
    let leaf = rcgen::CertificateParams::new(vec![host.into()]).unwrap();
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = leaf.signed_by(&leaf_key, &issuer).unwrap();
    std::fs::write(path.join("leaf.der"), leaf.der()).unwrap();
    std::fs::write(path.join("key.der"), leaf_key.serialize_der()).unwrap();
    let roots = match trust {
        "missing" => return,
        "empty" => String::new(),
        "invalid" => pem(b"not a DER certificate"),
        "bad-pem" => {
            "-----BEGIN CERTIFICATE-----\n!not-base64!\n-----END CERTIFICATE-----\n".into()
        }
        "mixed" => pem(root.der()) + &pem(b"invalid DER"),
        "unrelated" => pem(
            rcgen::generate_simple_self_signed(vec!["unrelated.invalid".into()])
                .unwrap()
                .cert
                .der(),
        ),
        _ => pem(root.der()),
    };
    std::fs::write(path.join("roots.pem"), roots).unwrap();
}

#[test]
fn managed_roots_preserve_verified_tls() {
    if let Ok(mode) = std::env::var(MODE) {
        tokio::runtime::Runtime::new().unwrap().block_on(run(&mode));
        return;
    }
    let mut failures = Vec::new();
    for transport in ["native", "web", "http"] {
        for trust in [
            "trusted",
            "wrong-host",
            "unrelated",
            "empty",
            "missing",
            "bad-pem",
            "invalid",
            "mixed",
        ] {
            let dir = tempfile::tempdir().unwrap();
            fixture(dir.path(), trust);
            let mode = format!("{transport}/{trust}");
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args([
                "--exact",
                "managed_roots_preserve_verified_tls",
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
                "SSL_CERT_DIR",
            ] {
                child.env_remove(key);
            }
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let output = child
                .env(MODE, &mode)
                .env(FIXTURE, dir.path())
                .env("SSL_CERT_FILE", dir.path().join("roots.pem"))
                .env("HTTPS_PROXY", format!("http://{address}"))
                .env("CUA_TEST_NATIVE_ROOTS_PROXY", address.to_string())
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
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

async fn head(socket: &mut (impl AsyncRead + Unpin)) -> String {
    let mut data = Vec::new();
    while !data.ends_with(b"\r\n\r\n") {
        assert!(data.len() < 4096);
        data.push(socket.read_u8().await.unwrap());
    }
    String::from_utf8(data).unwrap()
}

async fn run(mode: &str) {
    let (transport, trust) = mode.split_once('/').unwrap();
    let mock = MockServer::start(MockAuth {
        token: Some(TOKEN.into()),
        prefix: Some("/m/trusted".into()),
        ..Default::default()
    })
    .await;
    let proxy = TcpListener::bind(std::env::var("CUA_TEST_NATIVE_ROOTS_PROXY").unwrap())
        .await
        .unwrap();
    let path = std::path::PathBuf::from(std::env::var_os(FIXTURE).unwrap());
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![std::fs::read(path.join("leaf.der")).unwrap().into()],
        rustls::pki_types::PrivatePkcs8KeyDer::from(std::fs::read(path.join("key.der")).unwrap())
            .into(),
    )
    .unwrap();
    if transport == "native" {
        config.alpn_protocols = vec![b"h2".to_vec()];
    }
    let transport_owned = transport.to_owned();
    let trust_owned = trust.to_owned();
    let upstream = mock.addr;
    let server = tokio::spawn(async move {
        let (mut socket, _) = proxy.accept().await.unwrap();
        assert_eq!(
            head(&mut socket).await,
            "CONNECT relay.invalid:443 HTTP/1.1\r\nhost: relay.invalid:443\r\n\r\n"
        );
        socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        let hello =
            tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), socket)
                .await
                .unwrap();
        assert_eq!(hello.client_hello().server_name(), Some("relay.invalid"));
        let stream = hello.into_stream(Arc::new(config)).await;
        if !matches!(trust_owned.as_str(), "trusted" | "mixed") {
            assert!(
                stream.is_err(),
                "untrusted or wrong-host TLS must fail before application data"
            );
            return;
        }
        let mut stream = stream.expect("trusted matching-host TLS must succeed");
        if transport_owned == "http" {
            let request = head(&mut stream).await;
            assert!(request.starts_with("GET /m/trusted/mcp?fixture=1 HTTP/1.1\r\n"));
            assert!(request.contains(&format!("authorization: Bearer {TOKEN}\r\n")));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
        } else {
            let mut origin = TcpStream::connect(upstream).await.unwrap();
            let _ = tokio::io::copy_bidirectional(&mut stream, &mut origin).await;
        }
    });
    let mut options = ConnectOptions::parse("https://relay.invalid/m/trusted")
        .unwrap()
        .token(TOKEN)
        .transport(if transport == "web" {
            TransportPreference::GrpcWeb
        } else {
            TransportPreference::Native
        })
        .probe(transport != "http")
        .probe_timeout(Duration::from_secs(1));
    options.connect_timeout = Duration::from_secs(2);
    let result = async {
        let client = SpacesdClient::connect(options).await?;
        if transport == "http" {
            let reply = client
                .http(HttpCall {
                    method: "GET".into(),
                    path: "/mcp?fixture=1".into(),
                    ..Default::default()
                })
                .await?;
            assert_eq!(reply.body, b"{}");
        } else {
            assert_eq!(client.capabilities().await?.protocol_version, 1);
        }
        Ok::<_, cua_spacesd_client::Error>(())
    };
    let result = tokio::time::timeout(Duration::from_secs(5), result)
        .await
        .unwrap();
    if matches!(trust, "trusted" | "mixed") {
        result.expect("SSL_CERT_FILE CA must be accepted");
    } else {
        let error = result.expect_err("verification must fail closed");
        assert!(!format!("{error:?}").contains(TOKEN));
        if trust == "invalid" {
            assert!(
                error
                    .to_string()
                    .contains("zero valid certificates found in native root store"),
                "{error:?}"
            );
        }
    }
    if trust == "invalid" {
        server.abort();
        let _ = server.await;
    } else {
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
    }
}
