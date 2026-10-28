// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `/share/*` on a host: a real relay, a real spacesd joined to it as an
//! account machine, and a stand-in for the cua daemon's share gateway on a
//! Unix socket. Covers who reaches the gateway, the verified caller it is
//! handed, and everything that must never reach it.

#![cfg(unix)]

mod common;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use common::*;

const ISSUER: &str = "https://auth.test/realms/cua";

/// Answers every request with what it saw; `/share/<id>/ws` echoes
/// WebSocket messages.
async fn gateway(socket: &Path) {
    use axum::extract::ws::{Message, WebSocketUpgrade};
    async fn echo(
        method: axum::http::Method,
        uri: axum::http::Uri,
        headers: axum::http::HeaderMap,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let h: serde_json::Map<String, Value> = headers
            .iter()
            .map(|(k, v)| (k.to_string(), json!(v.to_str().unwrap_or(""))))
            .collect();
        let mut r = axum::response::Response::new(axum::body::Body::from(
            json!({
                "method": method.as_str(),
                "uri": uri.to_string(),
                "headers": h,
                "body": String::from_utf8_lossy(&body),
            })
            .to_string(),
        ));
        r.headers_mut()
            .insert("x-from-gateway", "yes".parse().unwrap());
        r
    }
    let app = axum::Router::new()
        .route(
            "/share/{id}/ws",
            axum::routing::get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(m)) = socket.recv().await {
                        if matches!(m, Message::Text(_)) && socket.send(m).await.is_err() {
                            break;
                        }
                    }
                })
            }),
        )
        .fallback(echo);
    let listener = tokio::net::UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
}

struct Setup {
    target: Target,
    relay_addr: SocketAddr,
    issuer: FakeIssuer,
    machine_id: String,
    policy_file: PathBuf,
    socket: PathBuf,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Drop for Setup {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

fn token(issuer: &FakeIssuer, user: &str) -> String {
    issuer.token(user, Some(&format!("{user}@example.com")), "cua-relay", 600)
}

async fn call(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: &str,
) -> (http::StatusCode, http::HeaderMap, Value) {
    let mut request = http::Request::builder().method(method).uri(url);
    for (k, v) in headers {
        request = request.header(*k, *v);
    }
    let response = http_client()
        .request(request.body(full(body.to_owned())).unwrap())
        .await
        .unwrap();
    let (status, headers) = (response.status(), response.headers().clone());
    let bytes = body_bytes(response, 1 << 20).await;
    let json = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned().into());
    (status, headers, json)
}

/// `policy` is the host policy JSON with `SOCKET` standing for the gateway.
async fn setup(policy: &str) -> Setup {
    let driver = target().await;
    let local_ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    let machine_id = format!("acct{}", uuid::Uuid::new_v4().simple());
    let ada = token(&issuer, "ada");
    let (status, _, reply) = call(
        "POST",
        &format!("http://{relay_addr}/v1/machines"),
        &[
            ("authorization", &format!("Bearer {ada}")),
            ("content-type", "application/json"),
        ],
        &json!({"id": machine_id, "name": "Ada's laptop"}).to_string(),
    )
    .await;
    assert_eq!(status, http::StatusCode::CREATED, "{reply}");
    // Ada shares the machine with Bob (the relay's allowlist).
    let (status, ..) = call(
        "PATCH",
        &format!("http://{relay_addr}/v1/machines/{machine_id}"),
        &[
            ("authorization", &format!("Bearer {ada}")),
            ("content-type", "application/json"),
        ],
        &json!({"allow": ["bob@example.com"]}).to_string(),
    )
    .await;
    assert!(status.is_success(), "{status}");

    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("gateway.sock");
    gateway(&socket).await;
    let token_file = dir.path().join("machine-token");
    std::fs::write(
        &token_file,
        format!("{}\n", reply["machine_token"].as_str().unwrap()),
    )
    .unwrap();
    let policy_file = dir.path().join("host.json");
    std::fs::write(
        &policy_file,
        policy.replace("SOCKET", socket.to_str().unwrap()),
    )
    .unwrap();
    let mut config = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        String::new(),
        machine_id.clone(),
        local,
    );
    config.relay_token_file = Some(token_file);
    config.heartbeat = Duration::from_secs(2);
    local_ctx.auth().set_external(Arc::new(
        cua_spacesd_server::relay_account::RelayAssertionAuth::new(
            machine_id.clone(),
            config.account.clone(),
            Some(policy_file.clone()),
        ),
    ));
    let link = config.account.clone();
    let shutdown = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(config, shutdown.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&machine_id) && !link.keys.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(relay.machine_ids().contains(&machine_id), "machine joined");
    Setup {
        target: driver,
        relay_addr,
        issuer,
        machine_id,
        policy_file,
        socket,
        shutdown,
        _dir: dir,
    }
}

const OPEN: &str = r#"{"owner":"ada","shares":true,"shares_gateway":"SOCKET","share_desktop":false,"allow":["bob@example.com"]}"#;

impl Setup {
    fn relayed(&self, path: &str) -> String {
        format!("http://{}/m/{}{path}", self.relay_addr, self.machine_id)
    }

    fn bearer(&self, user: &str) -> String {
        format!("Bearer {}", token(&self.issuer, user))
    }

    fn direct(&self, path: &str) -> String {
        format!("{}{path}", self.target.url)
    }
}

fn caller(seen: &Value) -> Option<Value> {
    seen["headers"]["x-cua-share-caller"]
        .as_str()
        .map(|c| serde_json::from_str(c).unwrap())
}

#[tokio::test]
async fn private_links_reach_the_gateway_with_the_relay_verified_caller() {
    let s = setup(OPEN).await;
    let (status, headers, seen) = call(
        "POST",
        &s.relayed("/share/0123456789abcdef/api/items?q=1"),
        &[
            ("authorization", &s.bearer("ada")),
            ("content-type", "text/plain"),
            ("x-app", "kept"),
        ],
        "payload",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{seen}");
    assert_eq!(headers["x-from-gateway"], "yes");
    assert_eq!(seen["method"], "POST");
    assert_eq!(seen["uri"], "/share/0123456789abcdef/api/items?q=1");
    assert_eq!(seen["body"], "payload");
    assert_eq!(seen["headers"]["x-app"], "kept");
    let c = caller(&seen).expect("a caller");
    assert_eq!(c["account"], "ada");
    assert_eq!(c["role"], "owner");
    assert_eq!(c["via"], "relay");
    assert_eq!(c["email"], "ada@example.com");
    assert_eq!(c["teams"], json!([]));
    // The app and the gateway never see a credential.
    for h in [
        "authorization",
        "x-cua-relay-assertion",
        "x-cua-env-authorization",
        "x-cua-relay-authorization",
    ] {
        assert!(seen["headers"].get(h).is_none(), "{h} leaked: {seen}");
    }

    // An account the owner shared the machine with is a `shared` caller:
    // the gateway, not the machine, decides what that opens.
    let (status, _, seen) = call(
        "GET",
        &s.relayed("/share/0123456789abcdef/"),
        &[("authorization", &s.bearer("bob"))],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{seen}");
    let c = caller(&seen).unwrap();
    assert_eq!(
        (c["account"].as_str(), c["role"].as_str()),
        (Some("bob"), Some("shared"))
    );

    // Somebody the machine is not shared with never gets that far.
    let (status, ..) = call(
        "GET",
        &s.relayed("/share/0123456789abcdef/"),
        &[("authorization", &s.bearer("eve"))],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    let (status, ..) = call("GET", &s.relayed("/share/0123456789abcdef/"), &[], "").await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn public_links_are_anonymous_and_carry_no_caller() {
    let s = setup(OPEN).await;
    let (status, _, seen) = call(
        "GET",
        &s.relayed("/share/pub/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/report.html"),
        &[("x-cua-share-caller", r#"{"account":"ada","role":"owner"}"#)],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{seen}");
    assert_eq!(
        seen["uri"],
        "/share/pub/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa/report.html"
    );
    assert!(caller(&seen).is_none(), "{seen}");
    // Direct to the driver, with a forged caller and no credential: also none.
    let (status, _, seen) = call(
        "GET",
        &s.direct("/share/pub/bbbb/x"),
        &[("x-cua-share-caller", r#"{"account":"ada","role":"owner"}"#)],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK);
    assert!(caller(&seen).is_none());
}

#[tokio::test]
async fn a_client_cannot_make_up_who_it_is() {
    let s = setup(OPEN).await;
    let forged = r#"{"account":"ada","role":"owner","via":"relay","teams":[]}"#;
    // Not signed in, a forged caller: refused.
    let (status, ..) = call(
        "GET",
        &s.direct("/share/0123456789abcdef/"),
        &[("x-cua-share-caller", forged)],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
    // A wrong token, and a forged assertion header, likewise.
    let (status, ..) = call(
        "GET",
        &s.direct("/share/0123456789abcdef/"),
        &[
            ("authorization", "Bearer nope"),
            ("x-cua-share-caller", forged),
        ],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
    let (status, ..) = call(
        "GET",
        &s.direct("/share/0123456789abcdef/"),
        &[("x-cua-relay-assertion", "a.b.c")],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::FORBIDDEN);
    // With the machine's token the caller is what the machine knows, not
    // what the client wrote.
    let (status, _, seen) = call(
        "GET",
        &s.direct("/share/0123456789abcdef/"),
        &[
            ("authorization", &format!("Bearer {}", s.target.token)),
            (
                "x-cua-share-caller",
                r#"{"account":"eve","role":"owner","via":"relay"}"#,
            ),
        ],
        "",
    )
    .await;
    assert_eq!(status, http::StatusCode::OK, "{seen}");
    let c = caller(&seen).unwrap();
    assert_eq!(
        (c["account"].as_str(), c["via"].as_str(), c["role"].as_str()),
        (Some("token"), Some("token"), Some("owner"))
    );
    assert!(seen["headers"].get("authorization").is_none());
}

#[tokio::test]
async fn shares_are_off_unless_the_host_policy_says_so() {
    let off = setup(r#"{"owner":"ada","share_desktop":false}"#).await;
    for url in [
        off.relayed("/share/0123456789abcdef/"),
        off.relayed("/share/pub/cccc/x"),
        off.direct("/share/pub/cccc/x"),
    ] {
        let (status, ..) = call("GET", &url, &[("authorization", &off.bearer("ada"))], "").await;
        assert_eq!(status, http::StatusCode::NOT_FOUND, "{url}");
    }
    let no_socket = setup(r#"{"owner":"ada","shares":true}"#).await;
    let (status, ..) = call("GET", &no_socket.relayed("/share/pub/cccc/x"), &[], "").await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_policy_change_applies_at_once_and_a_dead_gateway_is_a_502() {
    let s = setup(OPEN).await;
    let url = s.relayed("/share/pub/dddd/x");
    assert_eq!(call("GET", &url, &[], "").await.0, http::StatusCode::OK);
    // The owner stops sharing the machine in the host policy: public links
    // stop with it.
    std::fs::write(
        &s.policy_file,
        OPEN.replace("SOCKET", s.socket.to_str().unwrap())
            .replace(r#""owner":"ada""#, r#""owner":"ada","sharing":false"#),
    )
    .unwrap();
    assert_eq!(
        call("GET", &url, &[], "").await.0,
        http::StatusCode::FORBIDDEN
    );
    assert_eq!(
        call("GET", &s.direct("/share/pub/dddd/x"), &[], "").await.0,
        http::StatusCode::FORBIDDEN
    );
    // Sharing back on, but the daemon is gone.
    std::fs::write(
        &s.policy_file,
        OPEN.replace("SOCKET", s.socket.to_str().unwrap()),
    )
    .unwrap();
    std::fs::remove_file(&s.socket).unwrap();
    assert_eq!(
        call("GET", &url, &[], "").await.0,
        http::StatusCode::BAD_GATEWAY
    );
}

#[tokio::test]
async fn websockets_are_spliced_through() {
    use futures_util::{SinkExt as _, StreamExt as _};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    use tokio_tungstenite::tungstenite::Message;
    let s = setup(OPEN).await;
    let mut request = format!(
        "ws://{}/share/0123456789abcdef/ws",
        s.target.url.trim_start_matches("http://")
    )
    .into_client_request()
    .unwrap();
    request.headers_mut().insert(
        "authorization",
        format!("Bearer {}", s.target.token).parse().unwrap(),
    );
    let (mut ws, response) = tokio_tungstenite::connect_async(request).await.unwrap();
    assert_eq!(response.status(), http::StatusCode::SWITCHING_PROTOCOLS);
    ws.send(Message::Text("hello".into())).await.unwrap();
    let echo = tokio::time::timeout(Duration::from_secs(5), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(echo, Message::Text("hello".into()));
    // Without a credential the upgrade is refused before the gateway.
    let anonymous = format!(
        "ws://{}/share/0123456789abcdef/ws",
        s.target.url.trim_start_matches("http://")
    );
    assert!(tokio_tungstenite::connect_async(anonymous).await.is_err());
}
