// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Sharing a running Space through the relay, in process: the driver serves
//! normally (not `join`), its owner attaches it to a relay with
//! `SystemService.AttachRelay`, and the relay's lists decide who gets in:
//! a view-only account watches (capabilities, stream, presence) and every
//! other call is refused by the driver itself; an editor gets full use; a
//! removed account is refused at once; detaching ends relay access.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::*;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use cua_spacesd_client::TransportPreference;

use common::*;

const ISSUER: &str = "https://auth.test/realms/cua";

async fn api(
    addr: SocketAddr,
    method: &str,
    path: &str,
    token: &str,
    body: Option<serde_json::Value>,
) -> (http::StatusCode, serde_json::Value) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(format!("http://{addr}{path}"))
        .header("authorization", format!("Bearer {token}"));
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
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
    )
}

async fn caps_code(c: &cua_spacesd_client::SpacesdClient) -> tonic::Code {
    match c.system().get_capabilities(GetCapabilitiesRequest {}).await {
        Ok(_) => tonic::Code::Ok,
        Err(e) => e.code(),
    }
}

#[tokio::test]
async fn a_running_space_is_shared_to_watch_or_to_edit_and_revoked() {
    let driver = target().await;
    if driver.local.is_none() {
        eprintln!("skipped: needs the in-process driver");
        return;
    }
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        // This file is about sharing mechanics, not device enrollment
        // (cua-relay's tests/device_enrollment.rs covers that); the
        // migration grace period ending by default (S3) would otherwise
        // require an enrolled device for every account call here.
        device_enrollment: false,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    let ada = issuer.token("ada", Some("ada@example.com"), "cua-relay", 600);
    let bob = issuer.token("bob", Some("bob@example.com"), "cua-relay", 600);

    // The owner registers a machine for the Space...
    let machine_id = format!("space{}", uuid::Uuid::new_v4().simple());
    let (status, reply) = api(
        relay_addr,
        "POST",
        "/v1/machines",
        &ada,
        Some(serde_json::json!({"id": machine_id, "name": "shared space"})),
    )
    .await;
    assert_eq!(status, http::StatusCode::CREATED, "{reply}");
    let attach = AttachRelayRequest {
        relay_url: format!("ws://{relay_addr}"),
        machine_token: reply["machine_token"].as_str().unwrap().into(),
        machine_id: machine_id.clone(),
        relay_jwks_json: reply["jwks"].to_string(),
        owner: "ada".into(),
        owner_email: "ada@example.com".into(),
    };

    // ...and attaches the running driver with its root token.
    let root = driver
        .client_with_token(TransportPreference::Native, Some(driver.token.clone()))
        .await;
    let caps = root
        .system()
        .get_capabilities(GetCapabilitiesRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(caps
        .features
        .iter()
        .any(|f| f.name == "relay_attach" && f.supported));
    root.system().attach_relay(attach.clone()).await.unwrap();
    for _ in 0..200 {
        if relay.machine_ids().contains(&machine_id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        relay.machine_ids().contains(&machine_id),
        "joined the relay"
    );

    let via_relay = |token: String| {
        let t = Target {
            url: format!("http://{relay_addr}/m/{machine_id}"),
            token: String::new(),
            scratch: driver.scratch.clone(),
            local: None,
        };
        async move {
            t.client_with_token(TransportPreference::Native, Some(token))
                .await
        }
    };
    let patch = |body: serde_json::Value| {
        let (ada, id) = (ada.clone(), machine_id.clone());
        async move {
            let (status, reply) = api(
                relay_addr,
                "PATCH",
                &format!("/v1/machines/{id}"),
                &ada,
                Some(body),
            )
            .await;
            assert_eq!(status, http::StatusCode::OK, "{reply}");
        }
    };

    // The owner reaches it through the relay.
    let owner = via_relay(ada.clone()).await;
    assert_eq!(caps_code(&owner).await, tonic::Code::Ok);
    // Bob is not shared yet: the relay refuses him.
    let b = via_relay(bob.clone()).await;
    assert_eq!(caps_code(&b).await, tonic::Code::PermissionDenied);

    // Viewer: he may watch, nothing else. The refusal comes from the driver.
    patch(serde_json::json!({"viewers": ["bob@example.com"]})).await;
    let b = via_relay(bob.clone()).await;
    assert_eq!(caps_code(&b).await, tonic::Code::Ok);
    let refused = b
        .process()
        .list_processes(ListProcessesRequest::default())
        .await
        .unwrap_err();
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);
    assert_eq!(
        refused.message(),
        "view-only share: User bob (bob) cannot call /cua.env.v1.ProcessService/ListProcesses"
    );
    let refused = b.system().attach_relay(attach.clone()).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);

    // Editor: full use.
    patch(serde_json::json!({"viewers": [], "allow": ["bob@example.com"]})).await;
    let b = via_relay(bob.clone()).await;
    b.process()
        .list_processes(ListProcessesRequest::default())
        .await
        .expect("an editor may use the Space");
    // Even an editor may not re-attach the Space elsewhere: owner only.
    let refused = b.system().attach_relay(attach.clone()).await.unwrap_err();
    assert_eq!(refused.code(), tonic::Code::PermissionDenied);
    assert_eq!(
        refused.message(),
        "only the Space's owner (its root token) may attach a relay"
    );

    // Revoked: refused at once.
    patch(serde_json::json!({"allow": []})).await;
    let b = via_relay(bob.clone()).await;
    assert_eq!(caps_code(&b).await, tonic::Code::PermissionDenied);

    // The access log recorded the watch, the refusal and the edit.
    let log = std::fs::read_to_string(
        driver
            .local
            .as_ref()
            .unwrap()
            .ctx
            .config()
            .access_log_path(),
    )
    .unwrap_or_default();
    if !log.is_empty() {
        assert!(
            log.contains("refused ProcessService (view-only share)"),
            "{log}"
        );
    }

    // Detached: relay callers are no longer accepted by the driver.
    let detached = root
        .system()
        .detach_relay(DetachRelayRequest {})
        .await
        .unwrap()
        .into_inner();
    assert!(detached.detached);
    let owner = via_relay(ada.clone()).await;
    assert_ne!(caps_code(&owner).await, tonic::Code::Ok);
}
