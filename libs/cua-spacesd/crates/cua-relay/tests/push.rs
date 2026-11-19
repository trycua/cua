// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Push notifications for the phone app: a phone registers, a daemon of the
//! same account reports an event, and only that account's phones get the
//! alert (the mock provider records it). Off unless the relay is started
//! with push on.

use std::sync::Arc;

use cua_push::{MockProvider, PushHub, Registry};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::push_api::PushConfig;
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";

struct Env {
    base: String,
    issuer: FakeIssuer,
    mock: Arc<MockProvider>,
    http: reqwest::Client,
    enrollment: bool,
    /// The enrolled computer standing in for each account's daemon: its
    /// device session.
    daemons: std::sync::Mutex<std::collections::HashMap<String, String>>,
    /// A Space (computer) registered for each account the events are about.
    spaces: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

async fn start(push: bool, enrollment: bool) -> Env {
    let issuer = FakeIssuer::new(ISSUER);
    let mock = Arc::new(MockProvider::new());
    let hub = Arc::new(PushHub::new(Registry::in_memory(), mock.clone()));
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        device_enrollment: enrollment,
        device_policy: cua_relay::devices::DevicePolicy {
            grace_secs: 0,
            ..Default::default()
        },
        push: push.then(|| PushConfig::new(hub)),
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    Env {
        base,
        issuer,
        mock,
        http: reqwest::Client::new(),
        enrollment,
        daemons: Default::default(),
        spaces: Default::default(),
    }
}

impl Env {
    fn token(&self, user: &str) -> String {
        self.issuer
            .token(user, Some(&format!("{user}@example.com")), "cua-relay", 600)
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        user: &str,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token(user));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn register(&self, user: &str, token: &str) -> (StatusCode, Value) {
        self.register_as(user, &format!("dev_{user:0>8}"), token, json!({}))
            .await
    }

    async fn register_as(
        &self,
        user: &str,
        device: &str,
        token: &str,
        extra: Value,
    ) -> (StatusCode, Value) {
        let mut body = json!({"platform": "ios", "provider": "apns", "token": token,
                              "app_id": "ai.cua.spaces.mobile", "env": "sandbox", "locale": "en"});
        for (k, v) in extra.as_object().cloned().unwrap_or_default() {
            body[k] = v;
        }
        self.send(
            reqwest::Method::PUT,
            &format!("/v1/push/registrations/{device}"),
            user,
            Some(body),
        )
        .await
    }

    /// The device session of a computer enrolled for `user` (its daemon).
    async fn daemon_session(&self, user: &str) -> Option<String> {
        use base64::Engine as _;
        use ring::signature::KeyPair as _;
        if !self.enrollment {
            return None;
        }
        if let Some(s) = self.daemons.lock().unwrap().get(user) {
            return Some(s.clone());
        }
        let now = cua_relay::assertion::now_secs();
        self.issuer.set_auth_time(user, now as i64);
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let id = cua_relay::devices::device_id(pair.public_key().as_ref());
        let sign = |m: String| b64.encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
        let r = self
            .http
            .post(format!("{}/v1/devices", self.base))
            .bearer_auth(self.token(user))
            .json(
                &json!({"public_key": b64.encode(pair.public_key().as_ref()), "name": "daemon",
                          "platform": "macos", "ts": now,
                          "sig": sign(cua_relay::devices::register_message(&id, now)),
                          "bootstrap": true}),
            )
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success());
        let v: Value = self
            .http
            .post(format!("{}/v1/devices/session", self.base))
            .bearer_auth(self.token(user))
            .json(&json!({"device_id": id, "ts": now,
                          "sig": sign(cua_relay::devices::session_message(&id, now))}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let session = v["session"].as_str().unwrap().to_owned();
        self.daemons
            .lock()
            .unwrap()
            .insert(user.to_owned(), session.clone());
        Some(session)
    }

    /// A Space of `user`'s own on the relay (`relay:<id>`).
    async fn own_space(&self, user: &str) -> String {
        if let Some(s) = self.spaces.lock().unwrap().get(user) {
            return s.clone();
        }
        let id = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let mut req = self
            .http
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(self.token(user))
            .json(&json!({"id": id, "name": "a space"}));
        if let Some(session) = self.daemon_session(user).await {
            req = req.header("x-cua-device-session", session);
        }
        let r = req.send().await.unwrap();
        assert!(r.status().is_success(), "register a space: {}", r.status());
        let space = format!("relay:{id}");
        self.spaces
            .lock()
            .unwrap()
            .insert(user.to_owned(), space.clone());
        space
    }

    async fn event(&self, user: &str, id: &str, kind: &str) -> (StatusCode, Value) {
        let space = self.own_space(user).await;
        let mut req = self
            .http
            .post(format!("{}/v1/push/events", self.base))
            .bearer_auth(self.token(user))
            .json(
                &json!({"id": id, "kind": kind, "space": space, "entry": 4, "agent": "Claude Code",
                        "scope_choices": ["once", "space"]}),
            );
        if let Some(session) = self.daemon_session(user).await {
            req = req.header("x-cua-device-session", session);
        }
        let r = req.send().await.unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

fn tok(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

#[tokio::test]
async fn push_is_off_until_the_relay_turns_it_on() {
    let env = start(false, false).await;
    let (status, body) = env
        .send(reqwest::Method::GET, "/v1/push", "ada", None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(body["error"].as_str().unwrap().contains("not set up"));
    assert_eq!(env.register("ada", &tok(1)).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_phone_registers_and_its_account_gets_the_alert_and_nobody_else() {
    let env = start(true, false).await;
    let (status, reg) = env.register("ada", &tok(1)).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{reg}");
    env.register("bob", &tok(2)).await;
    let (_, status_body) = env
        .send(reqwest::Method::GET, "/v1/push", "ada", None)
        .await;
    assert!(
        !status_body.to_string().contains(&tok(1)),
        "the token is never shown back"
    );
    assert_eq!(status_body["phones"][0]["id"], "dev_00000ada");

    let (status, d) = env.event("ada", "n1", "approval_needed").await;
    assert_eq!(status, StatusCode::OK, "{d}");
    assert_eq!(d["sent"], 1);
    let sent = env.mock.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0.token, tok(1), "only Ada's phone");
    assert!(sent[0].0.sandbox, "env sandbox reaches the provider");
    assert_eq!(sent[0].1.title, "Claude Code needs you");
    let apns = sent[0].1.apns_json();
    assert_eq!(apns["space"], env.own_space("ada").await.as_str());
    assert_eq!(apns["entry"], 4);
    assert_eq!(apns["aps"]["category"], "CUA_APPROVAL");
    assert_eq!(apns["scope_choices"], json!(["once", "space"]));

    // Reported twice, sent once.
    let (_, again) = env.event("ada", "n1", "approval_needed").await;
    assert_eq!(again["skipped"], "duplicate");
    assert_eq!(env.mock.sent().len(), 1);
}

#[tokio::test]
async fn a_phone_takes_only_the_alerts_it_chose() {
    let env = start(true, false).await;
    env.register_as(
        "ada",
        "dev_phone001",
        &tok(1),
        json!({"categories": ["done"]}),
    )
    .await;
    env.register_as(
        "ada",
        "dev_phone002",
        &tok(2),
        json!({"categories": ["needs_you"]}),
    )
    .await;
    let (_, d) = env.event("ada", "a", "approval_needed").await;
    assert_eq!(d["sent"], 1);
    let (_, d) = env.event("ada", "b", "task_finished").await;
    assert_eq!(d["sent"], 1);
    let sent = env.mock.sent();
    assert_eq!(
        (sent[0].0.id.as_str(), sent[1].0.id.as_str()),
        ("dev_phone002", "dev_phone001")
    );
    // A bad category or provider is refused.
    let (status, _) = env
        .register_as(
            "ada",
            "dev_phone003",
            &tok(3),
            json!({"categories": ["everything"]}),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = env
        .register_as("ada", "dev_phone003", &tok(3), json!({"provider": "fcm"}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = env
        .register_as("ada", "dev_phone003", &tok(3), json!({"env": "beta"}))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn preferences_are_per_account_and_unregistering_stops_alerts() {
    let env = start(true, false).await;
    env.register("ada", &tok(1)).await;

    let (status, prefs) = env
        .send(
            reqwest::Method::PUT,
            "/v1/push/preferences",
            "ada",
            Some(json!({"task_finished": false})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(prefs["task_finished"], false);
    assert_eq!(prefs["approval_needed"], true);
    let (_, bobs) = env
        .send(reqwest::Method::GET, "/v1/push/preferences", "bob", None)
        .await;
    assert_eq!(bobs["task_finished"], true);

    let (_, d) = env.event("ada", "n1", "task_finished").await;
    assert_eq!(d["skipped"], "preferences_off");
    let (_, d) = env.event("ada", "n2", "task_failed").await;
    assert_eq!(d["sent"], 1);

    // Another account cannot remove Ada's phone; Ada can; twice is not found.
    let path = "/v1/push/registrations/dev_00000ada";
    assert_eq!(
        env.send(reqwest::Method::DELETE, path, "bob", None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        env.send(reqwest::Method::DELETE, path, "ada", None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        env.send(reqwest::Method::DELETE, path, "ada", None).await.0,
        StatusCode::NOT_FOUND
    );
    let (_, d) = env.event("ada", "n3", "task_failed").await;
    assert_eq!(d["skipped"], "no_phones");
    let (_, status) = env
        .send(reqwest::Method::GET, "/v1/push", "ada", None)
        .await;
    assert_eq!(status["phones"].as_array().unwrap().len(), 0);
    assert_eq!(status["provider"], "mock");
    assert_eq!(status["configured"], true);
}

#[tokio::test]
async fn bad_requests_a_machine_token_and_floods_are_refused() {
    let env = start(true, false).await;
    // No token at all.
    let r = env
        .http
        .get(format!("{}/v1/push", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    // A bad token value and a bad device id.
    let (status, _) = env.register("ada", "short").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = env.register_as("ada", "x", &tok(1), json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // A bad event.
    let (status, _) = env
        .send(
            reqwest::Method::POST,
            "/v1/push/events",
            "ada",
            Some(json!({"id": "", "kind": "task_failed"})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = env
        .send(
            reqwest::Method::POST,
            "/v1/push/events",
            "ada",
            Some(json!({"id": "x", "kind": "nope"})),
        )
        .await;
    assert!(status.is_client_error());
    // A body that is too large.
    let (status, _) = env
        .register_as(
            "ada",
            "dev_big00001",
            &tok(1),
            json!({"name": "x".repeat(20_000)}),
        )
        .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    // A flood of events is limited per account.
    let mut limited = false;
    for i in 0..(cua_relay::push_api::EVENTS_PER_WINDOW + 5) {
        let (status, _) = env.event("ada", &format!("e{i}"), "task_failed").await;
        if status == StatusCode::TOO_MANY_REQUESTS {
            limited = true;
            break;
        }
    }
    assert!(limited);
    // Another account is not affected by Ada's flood.
    assert_eq!(
        env.event("bob", "b1", "task_failed").await.0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_phone_and_a_daemon_must_both_be_enrolled_devices() {
    // Enrollment on, no grace: an unenrolled client cannot register a phone,
    // and an account token alone cannot report an event: it comes from an
    // enrolled computer of the account.
    let env = start(true, true).await;
    let (status, body) = env.register("ada", &tok(1)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert_eq!(body["code"], "device_forbidden");
    let r = env
        .http
        .post(format!("{}/v1/push/events", env.base))
        .bearer_auth(env.token("ada"))
        .json(&json!({"id": "n0", "kind": "task_failed"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.text().await.unwrap().contains("device_forbidden"));
    // From an enrolled computer it is accepted, about a Space of the account;
    // a Space of someone else's, or one nobody registered, is refused.
    let mine = env.own_space("ada").await;
    let theirs = env.own_space("bob").await;
    for space in [theirs, "relay:space-0000000000000000".to_string()] {
        let r = env
            .http
            .post(format!("{}/v1/push/events", env.base))
            .bearer_auth(env.token("ada"))
            .header(
                "x-cua-device-session",
                env.daemon_session("ada").await.unwrap(),
            )
            .json(&json!({"id": "n9", "kind": "approval_needed", "space": space}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{space}");
        assert!(r
            .text()
            .await
            .unwrap()
            .contains("this Space is not one of your account's"));
    }
    // No other spelling gets around the check: another case, another prefix, no
    // prefix, a short or odd id are all refused as not a Space of the relay.
    let theirs = env.own_space("bob").await;
    for space in [
        theirs.to_uppercase(),
        theirs.replacen("relay:", "Relay:", 1),
        theirs.replacen("relay:", "local:", 1),
        theirs.replacen("relay:", "", 1),
        "relay:short".to_string(),
        "relay:UPPER-CASE-ID-0000".to_string(),
        "relay:has space in it".to_string(),
    ] {
        let r = env
            .http
            .post(format!("{}/v1/push/events", env.base))
            .bearer_auth(env.token("ada"))
            .header(
                "x-cua-device-session",
                env.daemon_session("ada").await.unwrap(),
            )
            .json(&json!({"id": "n10", "kind": "approval_needed", "space": space}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{space}");
        assert!(r
            .text()
            .await
            .unwrap()
            .contains("space: relay:<id> of one of your account's Spaces"));
    }
    let _ = mine;
    let (status, d) = env.event("ada", "n1", "task_failed").await;
    assert_eq!(status, StatusCode::OK, "{d}");
    assert_eq!(d["skipped"], "no_phones");
}

#[tokio::test]
async fn without_a_device_session_registrations_are_few_and_cannot_replace_a_devices() {
    // Enrollment off: no session binds the id. Only a few registrations are
    // held, and every refusal is the same answer.
    let env = start(true, false).await;
    for n in 0..cua_push::registry::MAX_UNBOUND {
        let (status, _) = env
            .register_as("ada", &format!("dev_phone{n:03}"), &tok(n as u8), json!({}))
            .await;
        assert_eq!(status, StatusCode::NO_CONTENT);
    }
    let (s1, b1) = env
        .register_as("ada", "dev_phone999", &tok(99), json!({}))
        .await;
    assert_eq!(s1, StatusCode::CONFLICT, "{b1}");
    // Another account is unaffected by Ada's registrations.
    assert_eq!(
        env.register_as("bob", "dev_phone999", &tok(99), json!({}))
            .await
            .0,
        StatusCode::NO_CONTENT
    );
}

/// An enrolled phone registers only itself: the path's device id must be the
/// one its device session is of.
#[tokio::test]
async fn an_enrolled_phone_registers_only_itself() {
    use base64::Engine as _;
    use ring::signature::KeyPair as _;
    let env = start(true, true).await;
    let now = cua_relay::assertion::now_secs();
    env.issuer.set_auth_time("ada", now as i64);
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let sign = |m: String| b64.encode(pair.sign(&rng, m.as_bytes()).unwrap().as_ref());
    let call = |method: reqwest::Method, path: String, session: Option<String>, body: Value| {
        let env = &env;
        async move {
            let mut r = env
                .http
                .request(method, format!("{}{path}", env.base))
                .bearer_auth(env.token("ada"))
                .json(&body);
            if let Some(s) = session {
                r = r.header("x-cua-device-session", s);
            }
            let r = r.send().await.unwrap();
            let status = r.status();
            (status, r.json::<Value>().await.unwrap_or(Value::Null))
        }
    };
    let (status, _) = call(
        reqwest::Method::POST,
        "/v1/devices".into(),
        None,
        json!({"public_key": b64.encode(pair.public_key().as_ref()), "name": "p", "ts": now,
               "sig": sign(cua_relay::devices::register_message(&id, now)), "bootstrap": true}),
    )
    .await;
    assert!(status.is_success());
    let (status, session) = call(
        reqwest::Method::POST,
        "/v1/devices/session".into(),
        None,
        json!({"device_id": id, "ts": now, "sig": sign(cua_relay::devices::session_message(&id, now))}),
    )
    .await;
    assert!(status.is_success(), "{session}");
    let session = session["session"].as_str().unwrap().to_string();
    let body = json!({"platform": "ios", "token": tok(7)});
    // Itself: registered. Another device id: refused.
    let (status, _) = call(
        reqwest::Method::PUT,
        format!("/v1/push/registrations/{id}"),
        Some(session.clone()),
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, b) = call(
        reqwest::Method::PUT,
        "/v1/push/registrations/dev_someone_else".into(),
        Some(session),
        body,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(b["code"], "other_device");
}
