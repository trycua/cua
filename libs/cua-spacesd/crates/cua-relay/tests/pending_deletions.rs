// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Pending deletions (`GET /v1/machines/{id}/pending-deletions`, `POST
//! .../{space}/ack`): removing a computer with `?spaces=delete` records its
//! Spaces for it to delete when it is next online. Only that computer's
//! token reads and acks them; adding it again keeps them, and a deleted
//! Space's id cannot be registered again until it is acked.

use std::path::Path;
use std::sync::Arc;

use cua_relay::devices::DevicePolicy;
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use reqwest::{Method, StatusCode};
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

fn config(issuer: &FakeIssuer, dir: &Path) -> RelayConfig {
    RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_policy: DevicePolicy {
            grace_secs: 14 * 86_400,
            ..DevicePolicy::default()
        },
        state_file: Some(dir.join("machines.json")),
        ..RelayConfig::default()
    }
}

async fn serve(relay: &Relay) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let relay = relay.clone();
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

struct Env {
    base: String,
    issuer: FakeIssuer,
    dir: tempfile::TempDir,
    ada: String,
    bob: String,
}

impl Env {
    async fn start() -> Env {
        let issuer = FakeIssuer::new(ISSUER);
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::new(config(&issuer, dir.path()));
        let base = serve(&relay).await;
        let ada = issuer.token("ada-sub", Some("ada@example.com"), "cua-relay", 3000);
        let bob = issuer.token("bob-sub", Some("bob@example.com"), "cua-relay", 3000);
        Env {
            base,
            issuer,
            dir,
            ada,
            bob,
        }
    }

    /// A second relay on the same state files (a restart).
    async fn restart(&self) -> String {
        serve(&Relay::new(config(&self.issuer, self.dir.path()))).await
    }

    async fn call_on(
        base: &str,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut r = reqwest::Client::new().request(method, format!("{base}{path}"));
        if let Some(t) = token {
            r = r.bearer_auth(t);
        }
        if let Some(b) = body {
            r = r.json(&b);
        }
        let r = r.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn call(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        Self::call_on(&self.base, method, path, token, body).await
    }

    /// Registers a computer (or, with `host`, a Space on it); returns its
    /// machine token.
    async fn try_register(&self, token: &str, id: &str, host: Option<&str>) -> (StatusCode, Value) {
        self.call(
            Method::POST,
            "/v1/machines",
            Some(token),
            Some(json!({"id": id, "name": format!("{id}-name"), "host": host})),
        )
        .await
    }

    async fn register(&self, token: &str, id: &str, host: Option<&str>) -> String {
        let (s, v) = self.try_register(token, id, host).await;
        assert!(s.is_success(), "register {id}: {s} {v}");
        v["machine_token"].as_str().unwrap().to_owned()
    }

    fn new_id(&self) -> String {
        format!("m{}", uuid::Uuid::new_v4().simple())
    }

    async fn listed(&self, token: &str) -> Vec<String> {
        let (s, v) = self
            .call(Method::GET, "/v1/machines", Some(token), None)
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        v["machines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap().to_owned())
            .collect()
    }

    async fn remove(&self, computer: &str, delete: bool) {
        let q = if delete { "?spaces=delete" } else { "" };
        let (s, v) = self
            .call(
                Method::DELETE,
                &format!("/v1/machines/{computer}{q}"),
                Some(&self.ada),
                None,
            )
            .await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{v}");
    }

    async fn pending(&self, computer: &str, token: Option<&str>) -> (StatusCode, Value) {
        self.call(
            Method::GET,
            &format!("/v1/machines/{computer}/pending-deletions"),
            token,
            None,
        )
        .await
    }

    async fn ack(&self, computer: &str, space: &str, token: &str) -> StatusCode {
        self.call(
            Method::POST,
            &format!("/v1/machines/{computer}/pending-deletions/{space}/ack"),
            Some(token),
            None,
        )
        .await
        .0
    }
}

fn ids(v: &Value) -> Vec<String> {
    let mut out: Vec<String> = v["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_str().unwrap().to_owned())
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn removing_with_delete_records_the_spaces_for_the_computer_alone() {
    let env = Env::start().await;
    let host = env.new_id();
    let host_token = env.register(&env.ada, &host, None).await;
    let kid1 = env.new_id();
    let kid1_token = env.register(&env.ada, &kid1, Some(&host)).await;
    let kid2 = env.new_id();
    env.register(&env.ada, &kid2, Some(&host)).await;
    let other = env.new_id();
    let other_token = env.register(&env.ada, &other, None).await;
    let bobs = env.new_id();
    let bob_token = env.register(&env.bob, &bobs, None).await;

    // Nothing pending yet: the computer reads an empty list.
    let (s, v) = env.pending(&host, Some(&host_token)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["spaces"], json!([]));
    assert!(v.get("expires_at").is_none(), "{v}");

    env.remove(&host, true).await;
    // The Spaces leave the account at once.
    assert_eq!(env.listed(&env.ada).await, vec![other.clone()]);

    // The removed computer's token still reads its own record.
    let (s, v) = env.pending(&host, Some(&host_token)).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let mut want = vec![kid1.clone(), kid2.clone()];
    want.sort();
    assert_eq!(ids(&v), want);
    assert_eq!(v["computer"], host.as_str());
    let names: Vec<&str> = v["spaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&format!("{kid1}-name").as_str()), "{v}");
    let at = v["requested_at"].as_u64().unwrap();
    assert_eq!(v["expires_at"].as_u64().unwrap(), at + 90 * 86_400);

    // No one else does.
    let (s, v) = env.pending(&host, Some(&other_token)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "another computer of hers: {v}");
    let (s, _) = env.pending(&host, Some(&bob_token)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "another account's computer");
    let (s, _) = env.pending(&host, Some(&env.ada)).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "the owner's account token");
    let (s, _) = env.pending(&host, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = env.pending(&host, Some("cmt_not-a-real-token")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = env.pending(&host, Some(&kid1_token)).await;
    assert_eq!(
        s,
        StatusCode::UNAUTHORIZED,
        "a deleted Space's own token is gone"
    );
    assert_eq!(
        env.ack(&host, &kid1, &other_token).await,
        StatusCode::FORBIDDEN
    );
    // ... and the computer's token reads only its own id.
    let (s, _) = env.pending(&other, Some(&host_token)).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // It survives a restart.
    let restarted = env.restart().await;
    let (s, v) = Env::call_on(
        &restarted,
        Method::GET,
        &format!("/v1/machines/{host}/pending-deletions"),
        Some(&host_token),
        None,
    )
    .await;
    assert_eq!((s, ids(&v)), (StatusCode::OK, want.clone()));

    // Acks are idempotent; the last one clears the record.
    assert_eq!(
        env.ack(&host, &kid1, &host_token).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        env.ack(&host, &kid1, &host_token).await,
        StatusCode::NO_CONTENT
    );
    let (_, v) = env.pending(&host, Some(&host_token)).await;
    assert_eq!(ids(&v), vec![kid2.clone()]);
    assert_eq!(
        env.ack(&host, &format!("relay:{kid2}"), &host_token).await,
        StatusCode::NO_CONTENT
    );
    // Cleared: the removed computer's token now means nothing here.
    let (s, _) = env.pending(&host, Some(&host_token)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let raw =
        std::fs::read_to_string(env.dir.path().join("machines.pending-deletions.json")).unwrap();
    assert_eq!(raw.trim(), "{}");
}

#[tokio::test]
async fn removing_without_delete_records_nothing() {
    let env = Env::start().await;
    let host = env.new_id();
    let host_token = env.register(&env.ada, &host, None).await;
    let kid = env.new_id();
    env.register(&env.ada, &kid, Some(&host)).await;
    env.remove(&host, false).await;
    let (s, _) = env.pending(&host, Some(&host_token)).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A detached Space may come back: its id is not refused.
    env.register(&env.ada, &host, None).await;
    assert!(env.listed(&env.ada).await.contains(&kid));
}

#[tokio::test]
async fn adding_the_computer_again_keeps_the_deletions_and_refuses_the_ids() {
    let env = Env::start().await;
    let host = env.new_id();
    let old_token = env.register(&env.ada, &host, None).await;
    let kid = env.new_id();
    env.register(&env.ada, &kid, Some(&host)).await;
    env.remove(&host, true).await;

    // `cua host setup` adds it again with the same id and a new token.
    let new_token = env.register(&env.ada, &host, None).await;
    assert_ne!(new_token, old_token);
    for token in [&new_token, &old_token] {
        let (s, v) = env.pending(&host, Some(token)).await;
        assert_eq!((s, ids(&v)), (StatusCode::OK, vec![kid.clone()]), "{v}");
    }
    assert!(
        !env.listed(&env.ada).await.contains(&kid),
        "deleted means deleted"
    );

    // The deleted Space cannot be registered again meanwhile.
    let (s, v) = env.try_register(&env.ada, &kid, Some(&host)).await;
    assert_eq!(s, StatusCode::GONE, "{v}");
    assert!(v["error"].as_str().unwrap().contains("deleted"), "{v}");

    // Once the computer deleted it, the id is free again.
    assert_eq!(
        env.ack(&host, &kid, &new_token).await,
        StatusCode::NO_CONTENT
    );
    let (s, v) = env.pending(&host, Some(&new_token)).await;
    assert_eq!((s, v["spaces"].clone()), (StatusCode::OK, json!([])));
    env.register(&env.ada, &kid, Some(&host)).await;
}

#[tokio::test]
async fn a_second_removal_adds_to_the_record() {
    let env = Env::start().await;
    let host = env.new_id();
    env.register(&env.ada, &host, None).await;
    let kid1 = env.new_id();
    env.register(&env.ada, &kid1, Some(&host)).await;
    env.remove(&host, true).await;
    let token = env.register(&env.ada, &host, None).await;
    let kid2 = env.new_id();
    env.register(&env.ada, &kid2, Some(&host)).await;
    env.remove(&host, true).await;
    let (_, v) = env.pending(&host, Some(&token)).await;
    let mut want = vec![kid1, kid2];
    want.sort();
    assert_eq!(ids(&v), want);
}
