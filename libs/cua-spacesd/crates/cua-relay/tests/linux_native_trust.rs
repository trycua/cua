//! Linux OpenSSL trust fixture: no trust-store writes or external connections.
//! Ignored elsewhere because native TLS uses different platform trust APIs.
use cua_relay::{
    assertion::OWNER_HEADER,
    client::{JoinConfig, SessionEnd},
};
use std::{process::Command, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

const MODE: &str = "CUA_TEST_LINUX_TRUST_MODE";
const ADDRESS: &str = "CUA_TEST_LINUX_TRUST_ADDRESS";

#[test]
#[cfg_attr(not(target_os = "linux"), ignore = "requires Linux native TLS/OpenSSL")]
#[allow(clippy::result_large_err)] // tungstenite fixes callback error type.
fn linux_native_trust_and_hostname() {
    if std::env::var_os(MODE).is_none() {
        let files = tempfile::tempdir().unwrap();
        let trusted = files.path().join("trusted.pem");
        let empty = files.path().join("empty.pem");
        let roots = files.path().join("empty-roots");
        std::fs::write(&trusted, include_bytes!("fixtures/untrusted-cert.pem")).unwrap();
        std::fs::write(&empty, "").unwrap();
        std::fs::create_dir(&roots).unwrap();
        for mode in [
            "trusted-direct",
            "trusted-proxy",
            "wrong-host-proxy",
            "untrusted-proxy",
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            drop(listener);
            let mut child = Command::new(std::env::current_exe().unwrap());
            child.args(["--exact", "linux_native_trust_and_hostname", "--nocapture"]);
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
            child
                .env(MODE, mode)
                .env(ADDRESS, address.to_string())
                .env(
                    "SSL_CERT_FILE",
                    if mode == "untrusted-proxy" {
                        &empty
                    } else {
                        &trusted
                    },
                )
                .env("SSL_CERT_DIR", &roots);
            if mode != "trusted-direct" {
                child.env("HTTPS_PROXY", format!("http://{address}"));
            }
            let output = child.output().unwrap();
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
        let mode = std::env::var(MODE).unwrap();
        let address = std::env::var(ADDRESS).unwrap();
        let direct = mode == "trusted-direct";
        let success = direct || mode == "trusted-proxy";
        let host = if mode == "wrong-host-proxy" {
            "wrong.invalid"
        } else {
            "localhost"
        };
        let listener = TcpListener::bind(&address).await.unwrap();
        let port = if direct {
            listener.local_addr().unwrap().port()
        } else {
            443
        };
        let identity = native_tls::Identity::from_pkcs8(
            include_bytes!("fixtures/untrusted-cert.pem"),
            include_bytes!("fixtures/untrusted-key.pem"),
        )
        .unwrap();
        let acceptor =
            tokio_native_tls::TlsAcceptor::from(native_tls::TlsAcceptor::new(identity).unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            if !direct {
                let mut head = Vec::new();
                while !head.ends_with(b"\r\n\r\n") {
                    head.push(socket.read_u8().await.unwrap());
                }
                let head = String::from_utf8(head).unwrap();
                assert!(head.starts_with(&format!("CONNECT {host}:443 HTTP/1.1\r\n")));
                assert!(!head.to_ascii_lowercase().contains("authorization"));
                socket.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
            }
            match acceptor.accept(socket).await {
                Ok(tls) => {
                    assert!(success, "negative TLS case accepted");
                    let mut ws = tokio_tungstenite::accept_hdr_async(
                        tls,
                        |request: &http::Request<()>, mut response: http::Response<()>| {
                            assert_eq!(
                                request.headers()["authorization"],
                                "Bearer linux-fixture-canary"
                            );
                            response
                                .headers_mut()
                                .insert(OWNER_HEADER, "linux-trust-fixture".parse().unwrap());
                            Ok(response)
                        },
                    )
                    .await
                    .unwrap();
                    ws.close(None).await.unwrap();
                }
                Err(_) => assert!(!success, "positive TLS case rejected"),
            }
        });
        let config = JoinConfig::new(
            format!("wss://{host}:{port}/"),
            "linux-fixture-canary".into(),
            "linux-trust-machine".into(),
            "127.0.0.1:1".parse().unwrap(),
        );
        let end = tokio::time::timeout(
            Duration::from_secs(5),
            cua_relay::client::session(&config, &CancellationToken::new()),
        )
        .await
        .unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("TLS fixture server did not finish")
            .unwrap();
        if success {
            assert_eq!(
                config.account.owner().as_deref(),
                Some("linux-trust-fixture"),
                "{end:?}"
            );
        } else {
            assert!(matches!(end, SessionEnd::Lost(reason) if reason == "origin TLS failed"));
            assert!(config.account.owner().is_none());
        }
    });
}
