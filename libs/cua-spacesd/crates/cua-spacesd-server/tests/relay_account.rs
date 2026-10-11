// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Relay account mode end to end, in process: a fake OIDC issuer (keys
//! generated in-test) → cua-relay with the machine directory → a machine
//! registered by its owner joining with its machine token → the spacesd
//! accepting relay-signed principal assertions (no env token on the wire).
//! Covers directory listing (owned / shared), allowlist changes, presence,
//! stop / start sharing (open streams cut, new calls refused), the host
//! policy file, forged assertions, token rotation and removal.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::*;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_client::TransportPreference;
use tokio_util::sync::CancellationToken;

use common::*;

const ISSUER: &str = "https://auth.test/realms/cua";

struct Setup {
    target: Target,
    relay: Relay,
    relay_addr: SocketAddr,
    issuer: FakeIssuer,
    machine_id: String,
    machine_token: String,
    token_file: std::path::PathBuf,
    policy_file: std::path::PathBuf,
    shutdown: CancellationToken,
    _dir: tempfile::TempDir,
}

impl Setup {
    fn ada(&self) -> String {
        self.issuer
            .token("ada", Some("ada@example.com"), "cua-relay", 600)
    }

    fn bob(&self) -> String {
        self.issuer
            .token("bob", Some("bob@example.com"), "cua-relay", 600)
    }

    fn url(&self) -> String {
        format!("http://{}/m/{}", self.relay_addr, self.machine_id)
    }

    async fn client(
        &self,
        transport: TransportPreference,
        token: Option<String>,
    ) -> cua_spacesd_client::SpacesdClient {
        let target = Target {
            url: self.url(),
            token: String::new(),
            scratch: self.target.scratch.clone(),
            local: None,
        };
        target.client_with_token(transport, token).await
    }

    async fn api(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (http::StatusCode, serde_json::Value) {
        api(self.relay_addr, method, path, token, body).await
    }
}

async fn api(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> (http::StatusCode, serde_json::Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"));
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let payload = match body {
        Some(body) => {
            request = request.header("content-type", "application/json");
            serde_json::to_vec(&body).unwrap()
        }
        None => Vec::new(),
    };
    let response = http_client()
        .request(request.body(full(payload)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = body_bytes(response, 1 << 20).await;
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned().into())
    };
    (status, json)
}

async fn wait_until(what: &str, mut check: impl FnMut() -> bool) {
    for _ in 0..200 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

async fn setup() -> Setup {
    let driver = target().await;
    let local_ctx = driver.local.as_ref().expect("in process").ctx.clone();
    let local: SocketAddr = driver.url.trim_start_matches("http://").parse().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // This file is about machine/token mechanics (directory listing,
        // sharing, forged assertions, rotation), not device enrollment,
        // which has its own dedicated coverage in cua-relay's
        // tests/device_enrollment.rs. The migration grace period ending
        // by default (S3) would otherwise require an enrolled device for
        // every account call here.
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });

    // The owner registers the machine (what `cua host setup` does).
    let machine_id = format!("acct{}", uuid::Uuid::new_v4().simple());
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let (status, reply) = api(
        relay_addr,
        "POST",
        "/v1/machines",
        Some(&ada),
        Some(serde_json::json!({"id": machine_id, "name": "Ada's desktop"})),
    )
    .await;
    assert_eq!(status, http::StatusCode::CREATED, "{reply}");
    let machine_token = reply["machine_token"].as_str().unwrap().to_owned();
    assert!(machine_token.starts_with("cmt_"));
    assert_eq!(reply["jwks"]["keys"][0]["kty"], "OKP");

    let dir = tempfile::tempdir().unwrap();
    let token_file = dir.path().join("machine-token");
    std::fs::write(&token_file, format!("{machine_token}\n")).unwrap();
    let policy_file = dir.path().join("host.json");
    std::fs::write(&policy_file, r#"{"owner":"ada","allow":[]}"#).unwrap();

    let mut config = cua_relay::client::JoinConfig::new(
        format!("ws://{relay_addr}"),
        String::new(),
        machine_id.clone(),
        local,
    );
    config.relay_token_file = Some(token_file.clone());
    config.heartbeat = Duration::from_secs(2);
    config.max_backoff = Duration::from_millis(500);
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
    let r = relay.clone();
    let id = machine_id.clone();
    wait_until("machine online", || r.machine_ids().contains(&id)).await;
    wait_until("relay keys learned", || !link.keys.is_empty()).await;
    assert_eq!(link.owner().as_deref(), Some("ada"));
    Setup {
        target: driver,
        relay,
        relay_addr,
        issuer,
        machine_id,
        machine_token,
        token_file,
        policy_file,
        shutdown,
        _dir: dir,
    }
}

async fn health_code(c: &cua_spacesd_client::SpacesdClient) -> tonic::Code {
    match c.system().health(HealthRequest {}).await {
        Ok(_) => tonic::Code::Ok,
        Err(e) => e.code(),
    }
}

#[tokio::test]
async fn owner_connects_with_an_account_token_and_the_directory_lists_it() {
    let s = setup().await;
    let (status, list) = s.api("GET", "/v1/machines", Some(&s.ada()), None).await;
    assert_eq!(status, http::StatusCode::OK);
    let machines = list["machines"].as_array().unwrap();
    assert_eq!(machines.len(), 1);
    let m = &machines[0];
    assert_eq!(m["id"], s.machine_id.as_str());
    assert_eq!(m["name"], "Ada's desktop");
    assert_eq!(m["role"], "owner");
    assert_eq!(m["online"], true);
    assert_eq!(m["sharing"], true);
    assert_eq!(m["owner"]["email"], "ada@example.com");
    assert_eq!(m["url"], s.url().as_str());
    let (_, info) = s.api("GET", "/v1/info", None, None).await;
    assert_eq!(info["account_auth"], true);
    let (_, jwks) = s.api("GET", "/.well-known/jwks.json", None, None).await;
    assert_eq!(jwks["keys"][0]["kid"], s.relay.signing_key().kid());

    for transport in TRANSPORTS {
        let c = s.client(transport, Some(s.ada())).await;
        let (code, out) = run_sh(&c, "echo via-account").await;
        assert_eq!(
            (code, out.as_slice()),
            (Some(0), &b"via-account\n"[..]),
            "{transport:?}"
        );
    }

    // The env token is not an account credential: the relay refuses it for
    // an account machine (it never reaches the driver).
    let env = s
        .client(TransportPreference::Native, Some(s.target.token.clone()))
        .await;
    assert_eq!(health_code(&env).await, tonic::Code::Unauthenticated);
    let anonymous = s.client(TransportPreference::GrpcWeb, None).await;
    assert_eq!(health_code(&anonymous).await, tonic::Code::Unauthenticated);
    // A token for another audience or an expired one is refused.
    let wrong_aud = s.issuer.token("ada", None, "account", 600);
    let c = s.client(TransportPreference::Native, Some(wrong_aud)).await;
    assert_eq!(health_code(&c).await, tonic::Code::Unauthenticated);

    // Presence: an open stream shows the owner as connected.
    let c = s.client(TransportPreference::Native, Some(s.ada())).await;
    let mut stream = c
        .process()
        .start_process(StartProcessRequest {
            config: Some(ProcessConfig {
                command: SH.into(),
                args: vec!["-c".into(), "sleep 30".into()],
                ..Default::default()
            }),
            kill_on_disconnect: true,
            ..Default::default()
        })
        .await
        .unwrap()
        .into_inner();
    let _ = tokio::time::timeout(Duration::from_secs(10), stream.message()).await;
    let (_, m) = s
        .api(
            "GET",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&s.ada()),
            None,
        )
        .await;
    let clients = m["clients"].as_array().unwrap();
    assert!(
        clients
            .iter()
            .any(|c| c["id"] == "ada" && c["streams"].as_u64().unwrap() >= 1),
        "{m}"
    );

    // Host-side stop sharing (machine token): the open stream is cut and new
    // calls are refused.
    let (status, m) = s
        .api(
            "POST",
            &format!("/v1/machines/{}/stop-sharing", s.machine_id),
            Some(&s.machine_token),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::OK, "{m}");
    assert_eq!(m["sharing"], false);
    let cut = tokio::time::timeout(Duration::from_secs(10), async {
        for _ in 0..1000 {
            match stream.message().await {
                Ok(Some(_)) => continue,
                _ => return,
            }
        }
        panic!("stream kept producing events after stop-sharing");
    })
    .await;
    assert!(cut.is_ok(), "open stream survived stop-sharing");
    let c = s.client(TransportPreference::Native, Some(s.ada())).await;
    assert_eq!(health_code(&c).await, tonic::Code::PermissionDenied);
    let (status, _) = s
        .api(
            "POST",
            &format!("/v1/machines/{}/start-sharing", s.machine_id),
            Some(&s.ada()),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(health_code(&c).await, tonic::Code::Ok);

    // The host policy file is enforced by the driver itself.
    std::fs::write(&s.policy_file, r#"{"owner":"ada","sharing":false}"#).unwrap();
    assert_eq!(health_code(&c).await, tonic::Code::PermissionDenied);
    std::fs::write(&s.policy_file, r#"{"owner":"ada"}"#).unwrap();
    assert_eq!(health_code(&c).await, tonic::Code::Ok);
    s.shutdown.cancel();
}

#[tokio::test]
async fn sharing_with_another_account_and_forged_assertions() {
    let s = setup().await;
    // Bob sees nothing and cannot connect.
    let (_, list) = s.api("GET", "/v1/machines", Some(&s.bob()), None).await;
    assert!(list["machines"].as_array().unwrap().is_empty());
    let (status, _) = s
        .api(
            "GET",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&s.bob()),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);
    let bob = s.client(TransportPreference::Native, Some(s.bob())).await;
    assert_eq!(health_code(&bob).await, tonic::Code::PermissionDenied);
    // Only the owner edits the machine.
    let (status, _) = s
        .api(
            "PATCH",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&s.bob()),
            Some(serde_json::json!({"allow": ["bob"]})),
        )
        .await;
    assert_eq!(status, http::StatusCode::NOT_FOUND);

    // Ada shares by email: Bob lists it as shared and connects (the driver
    // trusts the relay allowlist by default).
    let (status, m) = s
        .api(
            "PATCH",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&s.ada()),
            Some(serde_json::json!({"allow": ["Bob@Example.com"], "name": "Studio"})),
        )
        .await;
    assert_eq!(status, http::StatusCode::OK, "{m}");
    assert_eq!(m["allow"], serde_json::json!(["bob@example.com"]));
    let (_, list) = s.api("GET", "/v1/machines", Some(&s.bob()), None).await;
    let shared = &list["machines"][0];
    assert_eq!(shared["role"], "shared");
    assert_eq!(shared["name"], "Studio");
    assert_eq!(
        shared["allow"],
        serde_json::json!([]),
        "allowlist is owner-only"
    );
    let (code, out) = run_sh(&bob, "echo shared").await;
    assert_eq!((code, out.as_slice()), (Some(0), &b"shared\n"[..]));
    // A host that does not trust the relay allowlist refuses Bob.
    std::fs::write(
        &s.policy_file,
        r#"{"owner":"ada","trust_relay_allowlist":false}"#,
    )
    .unwrap();
    assert_eq!(health_code(&bob).await, tonic::Code::PermissionDenied);
    std::fs::write(
        &s.policy_file,
        r#"{"owner":"ada","trust_relay_allowlist":false,"allow":["bob@example.com"]}"#,
    )
    .unwrap();
    assert_eq!(health_code(&bob).await, tonic::Code::Ok);

    // A client cannot smuggle its own assertion through the relay...
    let forged =
        cua_relay::assertion::RelayKey::generate().sign(&cua_relay::assertion::AssertionClaims {
            iss: "evil".into(),
            aud: s.machine_id.clone(),
            sub: "ada".into(),
            acct: "ada".into(),
            email: None,
            name: None,
            mid: s.machine_id.clone(),
            role: "owner".into(),
            scope: "env".into(),
            iat: cua_relay::assertion::now_secs(),
            exp: cua_relay::assertion::now_secs() + 60,
            jti: "x".into(),
        });
    let response = http_client()
        .request(
            http::Request::get(format!("{}/cua.env.v1.SystemService/Health", s.url()))
                .header("x-cua-relay-assertion", forged.clone())
                .body(full(Vec::new()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), http::StatusCode::UNAUTHORIZED);
    // ...and the driver refuses one signed by any other key when reached
    // directly.
    let direct = http_client()
        .request(
            http::Request::post(format!("{}/cua.env.v1.SystemService/Health", s.target.url))
                .header("content-type", "application/grpc-web")
                .header("x-cua-relay-assertion", forged)
                .body(full(vec![0, 0, 0, 0, 0]))
                .unwrap(),
        )
        .await
        .unwrap();
    let grpc_status = direct
        .headers()
        .get("grpc-status")
        .map(|v| v.to_str().unwrap().to_owned());
    assert_eq!(
        grpc_status.as_deref(),
        Some("7"),
        "forged assertion accepted directly"
    );

    // Revoking Bob cuts him off.
    s.api(
        "PATCH",
        &format!("/v1/machines/{}", s.machine_id),
        Some(&s.ada()),
        Some(serde_json::json!({"allow": []})),
    )
    .await;
    std::fs::write(&s.policy_file, r#"{"owner":"ada"}"#).unwrap();
    assert_eq!(health_code(&bob).await, tonic::Code::PermissionDenied);
    s.shutdown.cancel();
}

#[tokio::test]
async fn rotation_and_removal() {
    let s = setup().await;
    // Re-registering (setup run again) rotates the token: the running join
    // re-reads the token file and comes back.
    let (status, reply) = s
        .api(
            "POST",
            "/v1/machines",
            Some(&s.ada()),
            Some(serde_json::json!({"id": s.machine_id})),
        )
        .await;
    assert_eq!(status, http::StatusCode::OK);
    let rotated = reply["machine_token"].as_str().unwrap().to_owned();
    assert_ne!(rotated, s.machine_token);
    std::fs::write(&s.token_file, &rotated).unwrap();
    let c = s.client(TransportPreference::Native, Some(s.ada())).await;
    let mut ok = false;
    for _ in 0..100 {
        if health_code(&c).await == tonic::Code::Ok {
            ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(ok, "machine did not rejoin with the rotated token");
    // The old token no longer works for the API.
    let (status, _) = s
        .api(
            "GET",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&s.machine_token),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::UNAUTHORIZED);
    // Someone else cannot claim the id.
    let (status, _) = s
        .api(
            "POST",
            "/v1/machines",
            Some(&s.bob()),
            Some(serde_json::json!({"id": s.machine_id})),
        )
        .await;
    assert_eq!(status, http::StatusCode::CONFLICT);
    // Removal (host side, machine token) drops the machine for good.
    let (status, _) = s
        .api(
            "DELETE",
            &format!("/v1/machines/{}", s.machine_id),
            Some(&rotated),
            None,
        )
        .await;
    assert_eq!(status, http::StatusCode::NO_CONTENT);
    let r = s.relay.clone();
    let id = s.machine_id.clone();
    wait_until("machine dropped", || !r.machine_ids().contains(&id)).await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(
        !s.relay.machine_ids().contains(&s.machine_id),
        "revoked machine rejoined"
    );
    let (_, list) = s.api("GET", "/v1/machines", Some(&s.ada()), None).await;
    assert!(list["machines"].as_array().unwrap().is_empty());
    s.shutdown.cancel();
}
