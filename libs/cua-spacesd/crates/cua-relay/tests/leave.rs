// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `POST /v1/machines/{id}/leave`: a person a machine is shared with takes
//! themself off it. Their own allowlist or viewer entry goes (and with it
//! their access, at once), a team machine is hidden from their listing,
//! the owner's audit log says who left, and only people it is shared with
//! can leave: not the owner, not a stranger, not a machine token.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_relay::devices::{register_message, session_message};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";

async fn start(issuer: &FakeIssuer, device_enrollment: bool) -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (relay, base)
}

async fn send(req: reqwest::RequestBuilder) -> (StatusCode, Value) {
    let r = req.send().await.unwrap();
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

const SESSION: &str = "x-cua-device-session";

struct Device {
    token: String,
    session: String,
}

async fn enroll(base: &str, issuer: &FakeIssuer, sub: &str, email: &str) -> Device {
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let now = cua_relay::assertion::now_secs();
    issuer.set_auth_time(sub, now as i64);
    let token = issuer.token(sub, Some(email), "cua-relay", 600);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: &str| b64.encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let http = reqwest::Client::new();
    let (s, _) = send(
        http.post(format!("{base}/v1/devices"))
            .bearer_auth(&token)
            .json(&json!({
                "public_key": b64.encode(pair.public_key().as_ref()),
                "name": format!("{sub} laptop"),
                "ts": now,
                "sig": sign(&register_message(&id, now)),
                "bootstrap": true,
            })),
    )
    .await;
    assert!(s.is_success(), "register {s}");
    let (s, v) = send(
        http.post(format!("{base}/v1/devices/session"))
            .bearer_auth(&token)
            .json(&json!({"device_id": id, "ts": now, "sig": sign(&session_message(&id, now))})),
    )
    .await;
    assert!(s.is_success(), "session {s}");
    Device {
        token,
        session: v["session"].as_str().unwrap().to_owned(),
    }
}

impl Device {
    fn req(&self, method: reqwest::Method, url: String) -> reqwest::RequestBuilder {
        reqwest::Client::new()
            .request(method, url)
            .bearer_auth(&self.token)
            .header(SESSION, &self.session)
    }
}

fn listed(list: &Value, id: &str) -> bool {
    list["machines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["id"] == id)
}

#[tokio::test]
async fn an_editor_and_a_viewer_leave_and_lose_access_at_once() {
    let issuer = FakeIssuer::new(ISSUER);
    let (relay, base) = start(&issuer, true).await;
    let http = reqwest::Client::new();
    let ada = enroll(&base, &issuer, "ada", "ada@example.com").await;
    let bob = enroll(&base, &issuer, "bob", "bob@example.com").await;
    let cat = enroll(&base, &issuer, "cat", "cat@example.com").await;
    let eve = enroll(&base, &issuer, "eve", "eve@example.com").await;
    let (post, get, patch) = (
        reqwest::Method::POST,
        reqwest::Method::GET,
        reqwest::Method::PATCH,
    );
    let id = "space-leave00000001";
    let (s, reg) = send(
        ada.req(post.clone(), format!("{base}/v1/machines"))
            .json(&json!({"id": id, "name": "qa-shared"})),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{reg}");
    let machine_token = reg["machine_token"].as_str().unwrap().to_owned();
    // A local server stands in for the machine so streams can be opened.
    let app = axum::Router::new().fallback(|| async { "ok" });
    let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_addr = local.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(local, app).await;
    });
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        machine_token.clone(),
        id.to_owned(),
        local_addr,
    );
    join.heartbeat = Duration::from_secs(2);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id.to_owned()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    // Bob edits (shared by email, other case), Cat watches.
    let (s, v) = send(
        ada.req(patch.clone(), format!("{base}/v1/machines/{id}"))
            .json(&json!({"allow": ["Bob@Example.com"], "viewers": ["cat"]})),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let probe = |d: &Device| {
        let req = d.req(get.clone(), format!("{base}/m/{id}/probe"));
        async move { req.send().await.unwrap().status() }
    };
    assert_eq!(probe(&bob).await, StatusCode::OK);
    assert_eq!(probe(&cat).await, StatusCode::OK);

    let leave = |d: &Device| send(d.req(post.clone(), format!("{base}/v1/machines/{id}/leave")));
    // The owner deletes instead; a stranger does not see it; a machine
    // token cannot leave for anyone.
    let (s, v) = leave(&ada).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");
    assert!(v["error"].as_str().unwrap().contains("delete it instead"));
    let (s, _) = leave(&eve).await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, v) = send(
        http.post(format!("{base}/v1/machines/{id}/leave"))
            .bearer_auth(&machine_token),
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{v}");

    // Bob leaves: his entry goes, he loses access and the listing.
    let (s, v) = leave(&bob).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["removed"], json!(["bob@example.com"]));
    assert!(v.get("hidden_team").is_none());
    assert_eq!(probe(&bob).await, StatusCode::FORBIDDEN);
    let (_, list) = send(bob.req(get.clone(), format!("{base}/v1/machines"))).await;
    assert!(!listed(&list, id), "{list}");
    // Cat (a viewer) leaves too.
    let (s, v) = leave(&cat).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["removed"], json!(["cat"]));
    assert_eq!(probe(&cat).await, StatusCode::FORBIDDEN);
    // Gone already: a second leave finds nothing.
    let (s, _) = leave(&bob).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // The owner's lists are empty and their audit log says who left.
    let (_, mine) = send(ada.req(get.clone(), format!("{base}/v1/machines/{id}"))).await;
    assert_eq!(mine["allow"], json!([]));
    assert!(mine.get("viewers").is_none(), "{mine}");
    assert_eq!(probe(&ada).await, StatusCode::OK);
    let (s, audit) = send(ada.req(get.clone(), format!("{base}/v1/audit"))).await;
    assert_eq!(s, StatusCode::OK, "{audit}");
    let left: Vec<&Value> = audit["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["kind"] == "share_left")
        .collect();
    let subjects: Vec<&str> = left.iter().filter_map(|e| e["subject"].as_str()).collect();
    assert!(subjects.contains(&"bob@example.com"), "{audit}");
    assert!(subjects.contains(&"cat@example.com"), "{audit}");
    assert!(left.iter().all(|e| e["machine"] == id));
    // Bob's own log says he left.
    let (_, theirs) = send(bob.req(get.clone(), format!("{base}/v1/audit"))).await;
    assert!(
        theirs["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["kind"] == "machine_left" && e["machine"] == id),
        "{theirs}"
    );
    stop.cancel();
}

/// Team machines: a member who leaves no longer sees it in their listing
/// (the team keeps it, other members still see it), and the owner's log
/// says so.
#[cfg(feature = "hosted")]
mod team {
    use super::*;

    #[tokio::test]
    async fn a_member_who_leaves_a_team_machine_no_longer_lists_it() {
        let issuer = FakeIssuer::new(ISSUER);
        let (_relay, base) = start(&issuer, true).await;
        let ada = enroll(&base, &issuer, "ada", "ada@aigis.test").await;
        let bob = enroll(&base, &issuer, "bob", "bob@aigis.test").await;
        let cat = enroll(&base, &issuer, "cat", "cat@aigis.test").await;
        let id = "space-teamleave0001";
        let post = reqwest::Method::POST;
        let get = reqwest::Method::GET;
        let (s, v) = send(
            ada.req(post.clone(), format!("{base}/v1/machines"))
                .json(&json!({"id": id, "name": "qa-team"})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{v}");
        let (s, team) = send(
            ada.req(post.clone(), format!("{base}/v1/teams"))
                .json(&json!({"name": "QA"})),
        )
        .await;
        assert_eq!(s, StatusCode::CREATED, "{team}");
        let team = team["id"].as_str().unwrap().to_owned();
        let (s, v) = send(
            ada.req(post.clone(), format!("{base}/v1/teams/{team}/machines"))
                .json(&json!({"machine": id, "os": "linux"})),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        for who in ["bob@aigis.test", "cat@aigis.test"] {
            let (s, v) = send(
                ada.req(post.clone(), format!("{base}/v1/teams/{team}/invites"))
                    .json(&json!({"email": who})),
            )
            .await;
            assert_eq!(s, StatusCode::CREATED, "{v}");
        }
        // Listing accepts the email invites.
        for d in [&bob, &cat] {
            let (_, list) = send(d.req(get.clone(), format!("{base}/v1/machines"))).await;
            assert!(listed(&list, id), "{list}");
        }
        let (s, v) = send(bob.req(post.clone(), format!("{base}/v1/machines/{id}/leave"))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert_eq!(v["hidden_team"], team.as_str());
        assert_eq!(v["removed"], json!([]));
        let (_, list) = send(bob.req(get.clone(), format!("{base}/v1/machines"))).await;
        assert!(!listed(&list, id), "{list}");
        // Cat still has it; the team still has it.
        let (_, list) = send(cat.req(get.clone(), format!("{base}/v1/machines"))).await;
        assert!(listed(&list, id), "{list}");
        let (_, t) = send(ada.req(get.clone(), format!("{base}/v1/teams/{team}"))).await;
        assert_eq!(t["machines"][0]["id"], id, "{t}");
        let (_, audit) = send(ada.req(get.clone(), format!("{base}/v1/audit"))).await;
        assert!(
            audit["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["kind"] == "share_left" && e["subject"] == "bob@aigis.test"),
            "{audit}"
        );
    }
}
