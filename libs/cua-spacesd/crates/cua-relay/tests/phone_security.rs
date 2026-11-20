// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A phone is a smaller kind of device: it reaches a machine through a short
//! list of calls, cannot enroll a computer, is served only by a relay that has
//! phone support on, and is cut off the moment its device is revoked. Each
//! test asserts the exact refusal.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use cua_push::{MockProvider, PushHub, Registry};
use cua_relay::devices::{register_message, session_message, DevicePolicy};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::push_api::PushConfig;
use cua_relay::server::{Relay, RelayConfig};
use reqwest::StatusCode;
use ring::signature::KeyPair as _;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const ISSUER: &str = "https://auth.test/realms/cua";
const THREAD: &str = "cua.env.v1.HostThreadService/CallThreadTool";
const PROCESS: &str = "cua.env.v1.ProcessService/Run";
const POINTER: &str = "cua.env.v1.ComputerService/Pointer";

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

struct Device {
    pair: ring::signature::EcdsaKeyPair,
    id: String,
    /// `cua-spaces-mobile` for a phone, `cua-cli` for a computer.
    client: &'static str,
    platform: &'static str,
}

impl Device {
    fn new(client: &'static str, platform: &'static str) -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let id = cua_relay::devices::device_id(pair.public_key().as_ref());
        Self {
            pair,
            id,
            client,
            platform,
        }
    }

    fn sign(&self, message: &str) -> String {
        let rng = ring::rand::SystemRandom::new();
        b64().encode(self.pair.sign(&rng, message.as_bytes()).unwrap().as_ref())
    }
}

struct Env {
    base: String,
    issuer: FakeIssuer,
    http: reqwest::Client,
    stop: CancellationToken,
    relay: Relay,
}

async fn start(phone_support: bool) -> Env {
    start_phones(phone_support, true).await
}

/// `phones`: `CUA_RELAY_PHONES` (on unless turned off) with phone support on.
async fn start_phones(phone_support: bool, phones: bool) -> Env {
    start_full(phone_support, phones, Duration::ZERO).await
}

/// [`start_phones`] with the reconnect grace after a relay start (a request for a
/// known computer that is not connected waits that long for it).
async fn start_full(phone_support: bool, phones: bool, grace: Duration) -> Env {
    let issuer = FakeIssuer::new(ISSUER);
    let hub = Arc::new(PushHub::new(
        Registry::in_memory(),
        Arc::new(MockProvider::new()),
    ));
    let mut oidc = OidcConfig::new(ISSUER);
    oidc.authorized_parties = vec!["cua-cli".into(), "cua-spaces-mobile".into()];
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(oidc, issuer.jwks()))),
        device_policy: DevicePolicy {
            grace_secs: 0,
            ..DevicePolicy::default()
        },
        push: phone_support.then(|| PushConfig::new(hub).with_phones(phones)),
        reconnect_grace: grace,
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    Env {
        base,
        issuer,
        http: reqwest::Client::new(),
        stop: CancellationToken::new(),
        relay,
    }
}

impl Env {
    fn token(&self, user: &str, client: &str) -> String {
        self.issuer.token_for_client(
            user,
            Some(&format!("{user}@example.com")),
            "cua-relay",
            600,
            client,
        )
    }

    /// Registers `d` (a fresh sign-in enrolls it) and opens a session.
    async fn enroll(&self, user: &str, d: &Device) -> String {
        self.issuer
            .set_auth_time(user, cua_relay::assertion::now_secs() as i64);
        let ts = cua_relay::assertion::now_secs();
        let r = self
            .http
            .post(format!("{}/v1/devices", self.base))
            .bearer_auth(self.token(user, d.client))
            .json(&json!({
                "public_key": b64().encode(d.pair.public_key().as_ref()),
                "name": "device", "platform": d.platform, "ts": ts,
                "sig": d.sign(&register_message(&d.id, ts)), "bootstrap": true,
            }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "register: {}", r.status());
        self.session(user, d).await.expect("session")
    }

    async fn session(&self, user: &str, d: &Device) -> Result<String, (StatusCode, Value)> {
        let ts = cua_relay::assertion::now_secs();
        let r = self
            .http
            .post(format!("{}/v1/devices/session", self.base))
            .bearer_auth(self.token(user, d.client))
            .json(&json!({"device_id": d.id, "ts": ts,
                          "sig": d.sign(&session_message(&d.id, ts))}))
            .send()
            .await
            .unwrap();
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(v["session"].as_str().unwrap().to_owned())
        } else {
            Err((status, v))
        }
    }

    /// A machine of `owner` whose "daemon" answers every call with the relay
    /// assertion it was handed. `reached` counts the calls that arrived.
    async fn machine(
        &self,
        owner: &str,
        owner_device: &Device,
        owner_session: &str,
    ) -> (String, Arc<AtomicUsize>) {
        self.machine_with(owner, owner_device, owner_session, true)
            .await
    }

    /// [`Env::machine`], optionally one that does not say it serves phones (a
    /// Cua Spaces from before phones).
    async fn machine_with(
        &self,
        owner: &str,
        owner_device: &Device,
        owner_session: &str,
        phone_support: bool,
    ) -> (String, Arc<AtomicUsize>) {
        let reached = Arc::new(AtomicUsize::new(0));
        let count = reached.clone();
        let app = axum::Router::new().fallback(axum::routing::any(
            move |h: axum::http::HeaderMap, body: axum::body::Bytes| {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    if &body[..] == b"slow" {
                        // A stream that goes on for a while: a picture of a screen.
                        let ticks = futures_util::stream::unfold(0u32, |n| async move {
                            if n >= 40 {
                                return None;
                            }
                            tokio::time::sleep(Duration::from_millis(250)).await;
                            Some((
                                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(b"x")),
                                n + 1,
                            ))
                        });
                        return axum::response::IntoResponse::into_response(
                            axum::body::Body::from_stream(ticks),
                        );
                    }
                    axum::response::IntoResponse::into_response(axum::Json(json!({"assertion": h
                            .get("x-cua-relay-assertion")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or(""),
                            "body": b64().encode(&body)})))
                }
            },
        ));
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(local, app).await;
        });
        let id = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let reg: Value = self
            .http
            .post(format!("{}/v1/machines", self.base))
            .bearer_auth(self.token(owner, owner_device.client))
            .header("x-cua-device-session", owner_session)
            .json(&json!({"id": id, "name": "studio"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let mut join = cua_relay::client::JoinConfig::new(
            self.base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        join.phone_support = phone_support;
        tokio::spawn(cua_relay::client::run(join, self.stop.clone()));
        for _ in 0..200 {
            if self.relay.machine_ids().contains(&id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(self.relay.machine_ids().contains(&id), "machine joined");
        (id, reached)
    }

    async fn call(
        &self,
        d: &Device,
        session: &str,
        user: &str,
        method: reqwest::Method,
        path: &str,
    ) -> (StatusCode, Value) {
        let r = self
            .http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(self.token(user, d.client))
            .header("x-cua-device-session", session)
            // A caller's own assertion header must never reach the machine.
            .header("x-cua-relay-assertion", "forged")
            .send()
            .await
            .unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

impl Env {
    /// A gRPC-Web call with a binary body.
    async fn grpc(
        &self,
        d: &Device,
        session: &str,
        user: &str,
        path: &str,
        payload: &[u8],
    ) -> (StatusCode, Value) {
        let mut framed = vec![0u8];
        framed.extend((payload.len() as u32).to_be_bytes());
        framed.extend(payload);
        let r = self
            .http
            .post(format!("{}{path}", self.base))
            .bearer_auth(self.token(user, d.client))
            .header("x-cua-device-session", session)
            .header("content-type", "application/grpc-web+proto")
            .body(framed)
            .send()
            .await
            .unwrap();
        let status = r.status();
        let text = r.text().await.unwrap_or_default();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }
}

/// The claims of an assertion the machine received (the relay signs it; the
/// machine verifies it, here we only read it).
fn claims(assertion: &str) -> Value {
    let body = assertion.split('.').nth(1).expect("a JWT");
    serde_json::from_slice(&b64().decode(body).unwrap()).unwrap()
}

#[tokio::test]
async fn a_phone_reaches_a_machine_through_its_short_list_and_is_marked_as_a_phone() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (id, reached) = env.machine("ada", &computer, &cs).await;

    // The phone's thread call arrives, vouched for as a phone, with its device.
    let (status, body) = env
        .call(
            &phone,
            &ps,
            "ada",
            reqwest::Method::POST,
            &format!("/m/{id}/{THREAD}"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let assertion = body["assertion"].as_str().unwrap();
    assert_ne!(
        assertion, "forged",
        "the caller's own assertion is replaced"
    );
    let c = claims(assertion);
    assert_eq!(c["dcls"], "mobile");
    assert_eq!(c["dev"], phone.id);
    assert_eq!(c["acct"], "ada");

    // The desktop calls are forwarded too; the machine decides about them
    // (the role and the shared desktop are decided there).
    let (status, body) = env
        .call(
            &phone,
            &ps,
            "ada",
            reqwest::Method::POST,
            &format!("/m/{id}/{POINTER}"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = reached.load(Ordering::SeqCst);

    // Everything that runs code, reads files or opens a tunnel is refused at
    // the relay, with the same words, and never reaches the machine.
    for (method, path) in [
        (reqwest::Method::POST, format!("/m/{id}/{PROCESS}")),
        (
            reqwest::Method::POST,
            format!("/m/{id}/cua.env.v1.FilesystemService/WriteFile"),
        ),
        (
            reqwest::Method::POST,
            format!("/m/{id}/cua.env.v1.TunnelService/Forward"),
        ),
        (
            reqwest::Method::POST,
            format!("/m/{id}/cua.env.v1.ComputerService/SetClipboard"),
        ),
        (
            reqwest::Method::GET,
            format!("/m/{id}/files?path=/etc/passwd"),
        ),
        (reqwest::Method::GET, format!("/m/{id}/tunnel")),
        (reqwest::Method::GET, format!("/m/{id}/{THREAD}")),
        (
            reqwest::Method::POST,
            format!("/m/{id}/{THREAD}/../{PROCESS}"),
        ),
    ] {
        let (status, body) = env.call(&phone, &ps, "ada", method.clone(), &path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}");
        assert!(
            body.to_string().contains("a phone cannot make this call"),
            "{method} {path}: {body}"
        );
    }
    assert_eq!(
        reached.load(Ordering::SeqCst),
        before,
        "none of them arrived"
    );

    // The computer is not limited, and carries no phone mark.
    let (status, body) = env
        .call(
            &computer,
            &cs,
            "ada",
            reqwest::Method::POST,
            &format!("/m/{id}/{PROCESS}"),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let c = claims(body["assertion"].as_str().unwrap());
    // Its device is named (the host attributes and binds tickets by it), but
    // it is not a phone.
    assert!(c.get("dcls").is_none(), "{c}");
    assert_eq!(c["dev"], computer.id);
}

#[tokio::test]
async fn a_phone_cannot_make_a_computer_a_full_device_and_cannot_stop_being_a_phone() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;

    // A second computer is registered without a fresh sign-in: pending.
    let second = Device::new("cua-cli", "linux");
    env.issuer.set_auth_time("ada", 0);
    let ts = cua_relay::assertion::now_secs();
    let r = env
        .http
        .post(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .json(&json!({
            "public_key": b64().encode(second.pair.public_key().as_ref()), "name": "second",
            "platform": "linux", "ts": ts,
            "sig": second.sign(&register_message(&second.id, ts)), "bootstrap": false,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());

    // The phone tries to approve it.
    let r = env
        .http
        .post(format!("{}/v1/devices/approve", env.base))
        .bearer_auth(env.token("ada", "cua-spaces-mobile"))
        .header("x-cua-device-session", &ps)
        .json(&json!({"device_id": second.id}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let body = r.text().await.unwrap();
    assert!(
        body.contains("a phone cannot approve a computer; approve it from a computer"),
        "{body}"
    );

    // Registering the phone's key again as a Mac does not make it a computer.
    let again = Device {
        platform: "macos",
        ..Device::new("cua-cli", "macos")
    };
    let ts = cua_relay::assertion::now_secs();
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    let r = env
        .http
        .post(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .json(&json!({
            "public_key": b64().encode(phone.pair.public_key().as_ref()), "name": "p",
            "platform": "macos", "ts": ts,
            "sig": phone.sign(&register_message(&phone.id, ts)), "bootstrap": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let _ = again;
    let list: Value = env
        .http
        .get(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mine = |id: &str| {
        list["devices"]
            .as_array()
            .or(list.as_array())
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(mine(&phone.id)["phone"], true, "still a phone");
    assert_eq!(mine(&computer.id)["phone"], false);
}

#[tokio::test]
async fn a_relay_without_phone_support_does_not_serve_a_phone() {
    let env = start(false).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    // Registering works (the device is recorded); using it does not.
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    let ts = cua_relay::assertion::now_secs();
    let r = env
        .http
        .post(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-spaces-mobile"))
        .json(&json!({
            "public_key": b64().encode(phone.pair.public_key().as_ref()), "name": "p",
            "platform": "ios", "ts": ts,
            "sig": phone.sign(&register_message(&phone.id, ts)), "bootstrap": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let ps = env.session("ada", &phone).await.expect("a session opens");
    let (id, reached) = env.machine("ada", &computer, &cs).await;
    let (status, body) = env
        .call(
            &phone,
            &ps,
            "ada",
            reqwest::Method::POST,
            &format!("/m/{id}/{THREAD}"),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        body.to_string()
            .contains("this relay does not serve phones"),
        "{body}"
    );
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn revoking_a_phone_ends_its_session_at_once() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (id, _) = env.machine("ada", &computer, &cs).await;
    let path = format!("/m/{id}/{THREAD}");
    let (status, _) = env
        .call(&phone, &ps, "ada", reqwest::Method::POST, &path)
        .await;
    assert_eq!(status, StatusCode::OK);
    let r = env
        .http
        .delete(format!("{}/v1/devices/{}", env.base, phone.id))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    let (status, body) = env
        .call(&phone, &ps, "ada", reqwest::Method::POST, &path)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        env.session("ada", &phone).await.is_err(),
        "no new session either"
    );
}

#[tokio::test]
async fn only_the_configured_clients_get_through_and_the_answer_does_not_say_which_exist() {
    let env = start(true).await;
    let r = env
        .http
        .get(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "some-other-app"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    let body = r.text().await.unwrap();
    assert!(
        body.contains("account token: this client may not use this relay"),
        "{body}"
    );
    // A token that names no client at all is refused the same way.
    let r = env
        .http
        .get(format!("{}/v1/devices", env.base))
        .bearer_auth(env.issuer.token("ada", None, "cua-relay", 600))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_phone_cannot_tell_a_machine_that_is_not_its_own_from_one_that_does_not_exist() {
    let env = start(true).await;
    let ada_pc = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    env.enroll("ada", &ada_pc).await;
    let ps = env.enroll("ada", &phone).await;
    let bob_pc = Device::new("cua-cli", "macos");
    let bs = env.enroll("bob", &bob_pc).await;
    let (bobs, reached) = env.machine("bob", &bob_pc, &bs).await;
    let path = |id: &str| format!("/m/{id}/{THREAD}");
    let (s1, b1) = env
        .call(&phone, &ps, "ada", reqwest::Method::POST, &path(&bobs))
        .await;
    let (s2, b2) = env
        .call(
            &phone,
            &ps,
            "ada",
            reqwest::Method::POST,
            &path("space-0123456789abcdef"),
        )
        .await;
    assert!(s1.is_client_error() && s2.is_client_error(), "{s1} {s2}");
    // Not shared with Ada, and not there at all: the same words. (The
    // machine id is the only part that differs, and it is the caller's own.)
    let normalize = |b: &Value, id: &str| b.to_string().replace(id, "<id>");
    assert_eq!(s1, s2);
    assert_eq!(
        normalize(&b1, &bobs),
        normalize(&b2, "space-0123456789abcdef")
    );
    assert_eq!(
        reached.load(Ordering::SeqCst),
        0,
        "nothing reached Bob's machine"
    );
}

/// A browser's ambient credentials (cookies) and any `Origin` mean nothing on
/// a machine route: a page on another site cannot ride a signed-in browser into
/// a machine, because the relay reads bearer tokens and tickets only.
#[tokio::test]
async fn cookies_and_a_foreign_origin_open_nothing() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let cs = env.enroll("ada", &computer).await;
    let (id, reached) = env.machine("ada", &computer, &cs).await;
    for path in [
        format!("/m/{id}/media"),
        format!("/m/{id}/{THREAD}"),
        format!("/m/{id}/tunnel"),
    ] {
        let r = env
            .http
            .get(format!("{}{path}", env.base))
            .header("Origin", "https://evil.example")
            .header("Cookie", "session=stolen; cua_share=stolen")
            .header("Upgrade", "websocket")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                base64::engine::general_purpose::STANDARD.encode([7u8; 16]),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
    }
    assert_eq!(reached.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_unknown_id_is_answered_like_a_machine_you_do_not_have_and_only_with_phone_support() {
    for support in [true, false] {
        let env = start(support).await;
        let bob_pc = Device::new("cua-cli", "macos");
        let bs = env.enroll("bob", &bob_pc).await;
        let (bobs, _) = env.machine("bob", &bob_pc, &bs).await;
        let ada_pc = Device::new("cua-cli", "macos");
        let sess = env.enroll("ada", &ada_pc).await;
        let unknown = "space-0123456789abcdef";
        let ask = |id: String, token: Option<String>| {
            let env = &env;
            let sess = sess.clone();
            async move {
                let mut r = env.http.post(format!("{}/m/{id}/{THREAD}", env.base));
                if let Some(t) = token {
                    r = r.bearer_auth(t).header("x-cua-device-session", sess);
                }
                let r = r.send().await.unwrap();
                let status = r.status();
                (
                    status,
                    r.text().await.unwrap_or_default().replace(&id, "<id>"),
                )
            }
        };
        let ada = env.token("ada", "cua-cli");
        let (known_signed_in, known_text) = ask(bobs.clone(), Some(ada.clone())).await;
        let (unknown_signed_in, unknown_text) = ask(unknown.into(), Some(ada)).await;
        let (known_anon, known_anon_text) = ask(bobs.clone(), None).await;
        let (unknown_anon, unknown_anon_text) = ask(unknown.into(), None).await;
        if support {
            assert_eq!(
                (known_signed_in, &known_text),
                (unknown_signed_in, &unknown_text)
            );
            assert_eq!(known_signed_in, StatusCode::FORBIDDEN);
            assert_eq!(
                (known_anon, &known_anon_text),
                (unknown_anon, &unknown_anon_text)
            );
            assert_eq!(known_anon, StatusCode::UNAUTHORIZED);
        } else {
            // Without phone support the relay answers as it always did.
            assert_eq!(unknown_signed_in, StatusCode::BAD_GATEWAY, "{unknown_text}");
        }
    }
}

#[tokio::test]
async fn enrollment_calls_are_rate_limited_per_account_with_phone_support_and_only_then() {
    for support in [true, false] {
        let env = start(support).await;
        let mut refused = None;
        for n in 0..(cua_relay::viewer::DEVICE_CALLS_PER_WINDOW + 5) {
            let d = Device::new("cua-cli", "macos");
            let ts = cua_relay::assertion::now_secs();
            let r = env
                .http
                .post(format!("{}/v1/devices", env.base))
                .bearer_auth(env.token("mallory", "cua-cli"))
                .json(&json!({
                    "public_key": b64().encode(d.pair.public_key().as_ref()), "name": "x",
                    "platform": "linux", "ts": ts,
                    "sig": d.sign(&register_message(&d.id, ts)), "bootstrap": false,
                }))
                .send()
                .await
                .unwrap();
            if r.status() == StatusCode::TOO_MANY_REQUESTS {
                refused = Some((n, r.text().await.unwrap()));
                break;
            }
        }
        if support {
            let (n, body) = refused.expect("the account was stopped");
            assert_eq!(n, cua_relay::viewer::DEVICE_CALLS_PER_WINDOW);
            assert!(
                body.contains("too many requests from this account; try again in a minute"),
                "{body}"
            );
            // Another account is unaffected.
            let other = env
                .http
                .get(format!("{}/v1/devices", env.base))
                .bearer_auth(env.token("ada", "cua-cli"))
                .send()
                .await
                .unwrap();
            assert_ne!(other.status(), StatusCode::TOO_MANY_REQUESTS);
        } else {
            assert!(refused.is_none(), "unchanged without phone support");
        }
    }
}

/// A flood on a stale sign-in (a thief's) is stopped without using up the owner's allowance: the owner's
/// fresh sign-in is still served.
#[tokio::test]
async fn a_thiefs_flood_on_an_old_sign_in_does_not_use_up_the_owners_allowance() {
    let env = start(true).await;
    env.issuer.set_auth_time("victim", 0);
    let mut stopped = false;
    for _ in 0..(cua_relay::viewer::DEVICE_CALLS_PER_WINDOW + 5) {
        let r = env
            .http
            .post(format!("{}/v1/devices/session", env.base))
            .bearer_auth(env.token("victim", "cua-cli"))
            .json(&json!({"device_id": "dev_000000000000000000000000", "ts": 1, "sig": "AAAA"}))
            .send()
            .await
            .unwrap();
        if r.status() == StatusCode::TOO_MANY_REQUESTS {
            stopped = true;
            break;
        }
    }
    assert!(stopped, "the flood on the old sign-in was stopped");
    env.issuer
        .set_auth_time("victim", cua_relay::assertion::now_secs() as i64);
    let r = env
        .http
        .post(format!("{}/v1/devices/session", env.base))
        .bearer_auth(env.token("victim", "cua-cli"))
        .json(&json!({"device_id": "dev_000000000000000000000000", "ts": 1, "sig": "AAAA"}))
        .send()
        .await
        .unwrap();
    assert_ne!(
        r.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "the owner's fresh sign-in is served"
    );
}

/// The account's kill switch for phones: one call revokes every phone, ends
/// their sessions, and leaves the computer that asked (and a computer that
/// was not asked about) alone.
#[tokio::test]
async fn revoke_all_phones_ends_every_phone_and_only_phones() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let other = Device::new("cua-cli", "linux");
    let p1 = Device::new("cua-spaces-mobile", "ios");
    let p2 = Device::new("cua-spaces-mobile", "android");
    let cs = env.enroll("ada", &computer).await;
    let _os = env.enroll("ada", &other).await;
    let s1 = env.enroll("ada", &p1).await;
    let s2 = env.enroll("ada", &p2).await;
    let (id, _) = env.machine("ada", &computer, &cs).await;
    let path = format!("/m/{id}/{THREAD}");
    for (p, s) in [(&p1, &s1), (&p2, &s2)] {
        let (status, _) = env.call(p, s, "ada", reqwest::Method::POST, &path).await;
        assert_eq!(status, StatusCode::OK);
    }
    let r = env
        .http
        .post(format!("{}/v1/devices/revoke-phones", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.unwrap();
    let mut revoked: Vec<String> = body["revoked"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    revoked.sort();
    let mut expected = vec![p1.id.clone(), p2.id.clone()];
    expected.sort();
    assert_eq!(revoked, expected);
    for (p, s) in [(&p1, &s1), (&p2, &s2)] {
        let (status, body) = env.call(p, s, "ada", reqwest::Method::POST, &path).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    }
    // The computers are untouched.
    assert!(env.session("ada", &computer).await.is_ok());
    assert!(env.session("ada", &other).await.is_ok());
    // A phone alone cannot do it without a fresh sign-in, and nothing is revoked.
    let q = Device::new("cua-spaces-mobile", "ios");
    let q2 = Device::new("cua-spaces-mobile", "android");
    let qs = env.enroll("ada", &q).await;
    let _q2s = env.enroll("ada", &q2).await;
    env.issuer.set_auth_time("ada", 0);
    let r = env
        .http
        .post(format!("{}/v1/devices/revoke-phones", env.base))
        .bearer_auth(env.token("ada", "cua-spaces-mobile"))
        .header("x-cua-device-session", &qs)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    assert!(
        env.session("ada", &q2).await.is_ok(),
        "the other phone was not revoked by a phone without a fresh sign-in"
    );
}

/// What an enrolled device is (its name and platform, so its class) is fixed
/// once it is enrolled: a stale token cannot relabel it as a computer.
#[tokio::test]
async fn an_enrolled_devices_platform_and_name_cannot_be_rewritten_by_registering_again() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let cs = env.enroll("ada", &computer).await;
    let phone = Device::new("cua-spaces-mobile", "ios");
    env.enroll("ada", &phone).await;
    let ts = cua_relay::assertion::now_secs();
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    let r = env
        .http
        .post(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .json(&json!({
            "public_key": b64().encode(phone.pair.public_key().as_ref()),
            "name": "Definitely a Mac", "platform": "macos", "ts": ts,
            "sig": phone.sign(&register_message(&phone.id, ts)), "bootstrap": true,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let list: Value = env
        .http
        .get(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let devices = list["devices"].as_array().or(list.as_array()).unwrap();
    let p = devices.iter().find(|d| d["id"] == phone.id).unwrap();
    assert_eq!(p["platform"], "ios");
    assert_eq!(p["name"], "device");
    assert_eq!(p["phone"], true);
}

fn ticket_of(device: &str, principal: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let payload = json!({"s": "viewer", "r": "{}", "p": format!("viewer:{principal}"),
                         "e": now + 120_000, "n": "abc", "d": device});
    format!(
        "v1.{}.c2ln",
        b64().encode(serde_json::to_vec(&payload).unwrap())
    )
}

/// A viewer ticket dies with the device whose call minted it, not only with
/// the account: a revoked phone's tickets stop working at once.
#[tokio::test]
async fn a_ticket_minted_by_a_phone_stops_when_that_phone_is_revoked() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    env.enroll("ada", &phone).await;
    let (id, reached) = env.machine("ada", &computer, &cs).await;
    let ticket = ticket_of(&phone.id, "ada");
    let call = || async {
        let r = env
            .http
            .post(format!(
                "{}/m/{id}/cua.env.v1.SystemService/GetCapabilities",
                env.base
            ))
            .header("x-cua-env-authorization", format!("Bearer {ticket}"))
            .send()
            .await
            .unwrap();
        r.status()
    };
    assert_eq!(call().await, StatusCode::OK, "the phone's ticket works");
    let before = reached.load(Ordering::SeqCst);
    let r = env
        .http
        .delete(format!("{}/v1/devices/{}", env.base, phone.id))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_ne!(
        call().await,
        StatusCode::OK,
        "the revoked phone's ticket is refused"
    );
    assert_eq!(
        reached.load(Ordering::SeqCst),
        before,
        "and never forwarded"
    );
}

/// A stolen, unlocked phone cannot enroll more devices on its own: approving
/// from a phone needs a sign-in made just now, and revoking another device
/// does too.
#[tokio::test]
async fn approving_or_revoking_from_a_phone_needs_a_sign_in_made_just_now() {
    let env = start(true).await;
    let phone = Device::new("cua-spaces-mobile", "ios");
    let ps = env.enroll("ada", &phone).await;
    let second_phone = Device::new("cua-spaces-mobile", "ios");
    // Registered with an old sign-in: pending, waiting for an approval.
    env.issuer.set_auth_time("ada", 0);
    let ts = cua_relay::assertion::now_secs();
    let r = env
        .http
        .post(format!("{}/v1/devices", env.base))
        .bearer_auth(env.token("ada", "cua-spaces-mobile"))
        .json(&json!({
            "public_key": b64().encode(second_phone.pair.public_key().as_ref()),
            "name": "second", "platform": "ios", "ts": ts,
            "sig": second_phone.sign(&register_message(&second_phone.id, ts)), "bootstrap": false,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    let approve = |step_up: bool| {
        let env = &env;
        let ps = ps.clone();
        let id = second_phone.id.clone();
        async move {
            env.issuer.set_auth_time(
                "ada",
                if step_up {
                    cua_relay::assertion::now_secs() as i64
                } else {
                    cua_relay::assertion::now_secs() as i64 - 3600
                },
            );
            let r = env
                .http
                .post(format!("{}/v1/devices/approve", env.base))
                .bearer_auth(env.token("ada", "cua-spaces-mobile"))
                .header("x-cua-device-session", ps)
                .json(&json!({"device_id": id}))
                .send()
                .await
                .unwrap();
            (r.status(), r.text().await.unwrap())
        }
    };
    let (status, body) = approve(false).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(
        body.contains(
            "approving a phone or computer from a phone needs you to sign in again just now"
        ),
        "{body}"
    );
    let (status, body) = approve(true).await;
    assert!(status.is_success(), "{status} {body}");
    // Revoking another device: the same.
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64 - 3600);
    let r = env
        .http
        .delete(format!("{}/v1/devices/{}", env.base, second_phone.id))
        .bearer_auth(env.token("ada", "cua-spaces-mobile"))
        .header("x-cua-device-session", &ps)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert!(r.text().await.unwrap().contains(
        "removing another phone or computer from a phone needs you to sign in again just now"
    ));
}

/// Junk registrations made with a stolen token cannot pile up and lock the
/// owner out: few devices wait for approval at once, and the owner's fresh
/// sign-in still enrolls their own.
#[tokio::test]
async fn a_pile_of_pending_devices_cannot_lock_the_owner_out() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    env.enroll("ada", &computer).await;
    env.issuer.set_auth_time("ada", 0);
    let mut refused = None;
    for n in 0..(cua_relay::devices::MAX_WAITING_PER_ACCOUNT + 2) {
        let d = Device::new("cua-cli", "linux");
        let ts = cua_relay::assertion::now_secs();
        let r = env
            .http
            .post(format!("{}/v1/devices", env.base))
            .bearer_auth(env.token("ada", "cua-cli"))
            .json(&json!({
                "public_key": b64().encode(d.pair.public_key().as_ref()), "name": "junk",
                "platform": "linux", "ts": ts,
                "sig": d.sign(&register_message(&d.id, ts)), "bootstrap": false,
            }))
            .send()
            .await
            .unwrap();
        if r.status() == StatusCode::FORBIDDEN {
            refused = Some((n, r.text().await.unwrap()));
            break;
        }
    }
    let (n, body) = refused.expect("the pile was stopped");
    assert_eq!(n, cua_relay::devices::MAX_WAITING_PER_ACCOUNT);
    assert!(
        body.contains("phones and computers are already waiting for approval"),
        "{body}"
    );
    // The owner signs in and enrolls their own phone all the same.
    let mine = Device::new("cua-spaces-mobile", "ios");
    env.enroll("ada", &mine).await;
}

fn apns_token(n: u8) -> String {
    format!("{n:02x}").repeat(32)
}

impl Env {
    async fn register_push(
        &self,
        user: &str,
        d: &Device,
        session: &str,
        token: &str,
    ) -> reqwest::Response {
        self.http
            .put(format!("{}/v1/push/registrations/{}", self.base, d.id))
            .bearer_auth(self.token(user, d.client))
            .header("x-cua-device-session", session)
            .json(
                &json!({"platform": "ios", "token": token, "app_id": "ai.cua.spaces.mobile",
                          "env": "sandbox"}),
            )
            .send()
            .await
            .unwrap()
    }

    async fn push_phones(&self, user: &str, d: &Device, session: &str) -> Vec<String> {
        let v: Value = self
            .http
            .get(format!("{}/v1/push", self.base))
            .bearer_auth(self.token(user, d.client))
            .header("x-cua-device-session", session)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        v["phones"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// Revoking a phone removes its push registration: a lost phone stops getting
/// alerts, approval prompts included.
#[tokio::test]
async fn revoking_a_phone_removes_its_push_registration() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let cs = env.enroll("ada", &computer).await;
    let lost = Device::new("cua-spaces-mobile", "ios");
    let ls = env.enroll("ada", &lost).await;
    let other = Device::new("cua-spaces-mobile", "ios");
    let os = env.enroll("ada", &other).await;
    assert_eq!(
        env.register_push("ada", &lost, &ls, &apns_token(1))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        env.register_push("ada", &other, &os, &apns_token(2))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(env.push_phones("ada", &other, &os).await.len(), 2);
    let r = env
        .http
        .delete(format!("{}/v1/devices/{}", env.base, lost.id))
        .bearer_auth(env.token("ada", "cua-cli"))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success());
    assert_eq!(
        env.push_phones("ada", &other, &os).await,
        vec![other.id.clone()]
    );
}

/// A push token another account's enrolled phone still holds is not taken
/// over, and the answer says so.
#[tokio::test]
async fn another_accounts_push_token_is_not_taken_over() {
    let env = start(true).await;
    let ada_phone = Device::new("cua-spaces-mobile", "ios");
    let as_ = env.enroll("ada", &ada_phone).await;
    let bob_phone = Device::new("cua-spaces-mobile", "ios");
    let bs = env.enroll("bob", &bob_phone).await;
    assert_eq!(
        env.register_push("ada", &ada_phone, &as_, &apns_token(7))
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let r = env
        .register_push("bob", &bob_phone, &bs, &apns_token(7))
        .await;
    assert_eq!(r.status(), StatusCode::CONFLICT);
    assert!(r
        .text()
        .await
        .unwrap()
        .contains("this phone is registered to another account; sign out of it there first"));
    assert_eq!(
        env.push_phones("ada", &ada_phone, &as_).await,
        vec![ada_phone.id.clone()]
    );
}

/// A pool member states what it can host; the relay keeps what an enrollment
/// states (checked), shows it with the device, and fixes it once enrolled.
#[tokio::test]
async fn an_enrollment_states_what_it_can_host_and_that_is_kept_checked_and_fixed() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let cs = env.enroll("ada", &computer).await;
    let phone = Device::new("cua-spaces-mobile", "ios");
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    let register = |caps: Value, d: &Device| {
        let env = &env;
        let ts = cua_relay::assertion::now_secs();
        let body = json!({
            "public_key": b64().encode(d.pair.public_key().as_ref()), "name": "Ada's iPhone",
            "platform": "ios", "ts": ts, "sig": d.sign(&register_message(&d.id, ts)),
            "bootstrap": true, "capabilities": caps,
        });
        async move {
            let r = env
                .http
                .post(format!("{}/v1/devices", env.base))
                .bearer_auth(env.token("ada", "cua-spaces-mobile"))
                .json(&body)
                .send()
                .await
                .unwrap();
            (r.status(), r.text().await.unwrap())
        }
    };
    // Malformed: a long list, a name with a space, a missing version.
    for bad in [
        json!({"v": 1, "images": (0..40).map(|n| format!("i{n}")).collect::<Vec<_>>()}),
        json!({"v": 1, "images": ["has space"]}),
        json!({"v": 0}),
        json!({"v": 1, "kinds": ["a-very-long-kind-word-indeed"]}),
    ] {
        let (status, body) = register(bad.clone(), &phone).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}: {body}");
        assert!(body.contains("invalid_capabilities"), "{body}");
    }
    // A phone that states nothing it can host.
    let (status, _) = register(json!({"v": 1, "can_host_spaces": false}), &phone).await;
    assert!(status.is_success());
    let list = |id: String| {
        let env = &env;
        let cs = cs.clone();
        async move {
            let v: Value = env
                .http
                .get(format!("{}/v1/devices", env.base))
                .bearer_auth(env.token("ada", "cua-cli"))
                .header("x-cua-device-session", cs)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            v["devices"]
                .as_array()
                .or(v.as_array())
                .unwrap()
                .iter()
                .find(|d| d["id"] == id)
                .cloned()
                .unwrap()
        }
    };
    let d = list(phone.id.clone()).await;
    assert_eq!(d["capabilities"]["can_host_spaces"], false);
    assert_eq!(d["capabilities"]["v"], 1);
    // Enrolled: registering again cannot rewrite it.
    let (status, _) = register(
        json!({"v": 1, "can_host_spaces": true, "images": ["*"], "desktop": true}),
        &phone,
    )
    .await;
    assert!(status.is_success());
    assert_eq!(
        list(phone.id.clone()).await["capabilities"]["can_host_spaces"],
        false
    );
}

/// With phone support on, a client that never finishes its headers is dropped
/// after the header timeout, and headers past the bound are refused before they
/// cost memory. Without it the relay serves as it always did.
#[tokio::test]
async fn with_phone_support_a_slow_or_oversized_header_costs_the_relay_nothing_for_long() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let env = start(true).await;
    let addr = env.base.trim_start_matches("http://").to_string();

    // Headers that never finish: closed by the relay within the timeout (and a little).
    let mut slow = tokio::net::TcpStream::connect(&addr).await.unwrap();
    slow.write_all(b"GET /healthz HTTP/1.1\r\nHost: x\r\nX-Slow: ")
        .await
        .unwrap();
    let mut buf = [0u8; 256];
    let closed = tokio::time::timeout(
        cua_relay::server::HEADER_READ_TIMEOUT + Duration::from_secs(5),
        slow.read(&mut buf),
    )
    .await
    .expect("the relay dropped the slow client in time");
    // Either a plain close or a 408 answer, never held open.
    match closed {
        Ok(0) | Err(_) => {}
        Ok(n) => assert!(
            String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.1 408"),
            "{:?}",
            &buf[..n]
        ),
    }

    // A 70 KB header is refused (431 or a closed connection), not read in full.
    let mut big = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let head = format!(
        "GET /healthz HTTP/1.1\r\nHost: x\r\nX-Big: {}\r\n\r\n",
        "a".repeat(70 * 1024)
    );
    let _ = big.write_all(head.as_bytes()).await;
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), big.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out);
    assert!(
        !text.starts_with("HTTP/1.1 200") && !text.starts_with("HTTP/1.1 204"),
        "{text}"
    );
    assert!(
        out.is_empty() || text.starts_with("HTTP/1.1 431") || text.starts_with("HTTP/1.1 400"),
        "{text}"
    );

    // An ordinary request still works on the same relay.
    let ok = env
        .http
        .get(format!("{}/healthz", env.base))
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);
}

/// A computer that does not say it serves phones (Cua Spaces from before phones)
/// takes a phone for a computer and does what it is asked: it is sent nothing of
/// a phone's, and told why. The same computer still answers its owner's computers.
#[tokio::test]
async fn a_computer_from_before_phones_is_sent_nothing_of_a_phones() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (old, reached_old) = env.machine_with("ada", &computer, &cs, false).await;
    let (new, reached_new) = env.machine("ada", &computer, &cs).await;
    let thread = format!("/m/{old}/{THREAD}");
    let (status, body) = env
        .call(&phone, &ps, "ada", reqwest::Method::POST, &thread)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body.to_string()
            .contains(cua_relay::server::PHONE_OLD_COMPUTER)
            || body
                .as_str()
                .is_some_and(|b| b.contains(cua_relay::server::PHONE_OLD_COMPUTER)),
        "{body}"
    );
    assert_eq!(reached_old.load(Ordering::SeqCst), 0, "nothing reached it");
    // The old computer still answers a computer of the account.
    let (status, _) = env
        .call(&computer, &cs, "ada", reqwest::Method::POST, &thread)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reached_old.load(Ordering::SeqCst), 1);
    // A computer that says it serves phones answers the phone.
    let (status, _) = env
        .call(
            &phone,
            &ps,
            "ada",
            reqwest::Method::POST,
            &format!("/m/{new}/{THREAD}"),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reached_new.load(Ordering::SeqCst), 1);
}

/// What a phone asks of `CreateViewerTicket` is not what the computer is told:
/// view only, five minutes at most, no clipboard, no files, no microphone, no
/// principal. A computer's own request is forwarded as it is.
#[tokio::test]
async fn a_phones_ticket_request_is_rewritten_before_the_computer_sees_it() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (id, _) = env.machine("ada", &computer, &cs).await;
    let path = format!("/m/{id}/cua.env.v1.SystemService/CreateViewerTicket");
    // 24 h, policy 3, clipboard, files_root, microphone: the S9 request.
    let mut asked = vec![
        0x0a, 0x04, 0x08, 0x80, 0xa3, 0x05, 0x10, 0x03, 0x18, 0x01, 0x22, 0x0b,
    ];
    asked.extend(b"/Users/ada/");
    asked.extend([0x28, 0x01]);
    let (status, body) = env.grpc(&phone, &ps, "ada", &path, &asked).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let got = b64().decode(body["body"].as_str().unwrap()).unwrap();
    assert_eq!(
        got,
        [
            &[0u8, 0, 0, 0, 7][..],
            &[0x0a, 0x03, 0x08, 0xac, 0x02, 0x10, 0x01]
        ]
        .concat(),
        "300 s, view only, nothing else, framed"
    );
    let (status, body) = env.grpc(&computer, &cs, "ada", &path, &asked).await;
    assert_eq!(status, StatusCode::OK);
    let got = b64().decode(body["body"].as_str().unwrap()).unwrap();
    assert_eq!(&got[5..], &asked[..], "a computer's request is not touched");
}

/// The pool is how the account's computers coordinate and key each other. A
/// phone is not one: it cannot take the lease, read the keys or register one.
#[tokio::test]
async fn a_phone_cannot_take_part_in_the_pool() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    for (method, path, body) in [
        ("POST", "/v1/pool/lease", json!({})),
        // A body the pool cannot read gets the phone's answer, not a different one.
        ("POST", "/v1/pool/member", json!({"nothing": true})),
        ("GET", "/v1/pool/keys", Value::Null),
    ] {
        let send = |d: &Device, s: &str| {
            let r = env
                .http
                .request(method.parse().unwrap(), format!("{}{path}", env.base))
                .bearer_auth(env.token("ada", d.client))
                .header("x-cua-device-session", s.to_owned());
            if body.is_null() {
                r
            } else {
                r.json(&body)
            }
        };
        let r = send(&phone, &ps).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::FORBIDDEN, "{method} {path}");
        let v: Value = r.json().await.unwrap();
        assert_eq!(v["code"], "mobile_denied", "{v}");
        assert_eq!(v["error"], cua_relay::pool::PHONE_NOT_THE_POOL);
        let r = send(&computer, &cs).send().await.unwrap();
        assert_ne!(
            r.status(),
            StatusCode::FORBIDDEN,
            "a computer takes part: {method} {path}"
        );
    }
}

/// A stolen phone with a refreshed (not recent) sign-in must not reshare the
/// owner's computer, stop or remove it, or mint a pairing code that adds one; it
/// can on a sign-in made just now, as can a computer at any time.
#[tokio::test]
async fn a_phone_changes_the_accounts_computers_only_on_a_recent_sign_in() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (id, _) = env.machine("ada", &computer, &cs).await;
    let calls = |method: &'static str, path: String, body: Value| {
        let env = &env;
        let (phone, ps) = (&phone, &ps);
        async move {
            let r = env
                .http
                .request(method.parse().unwrap(), format!("{}{path}", env.base))
                .bearer_auth(env.token("ada", phone.client))
                .header("x-cua-device-session", ps.as_str())
                .json(&body)
                .send()
                .await
                .unwrap();
            let status = r.status();
            let text = r.text().await.unwrap_or_default();
            (
                status,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)),
            )
        }
    };
    let attempts = |id: &str| {
        vec![
            (
                "PATCH",
                format!("/v1/machines/{id}"),
                json!({"allow": ["thief@example.com"]}),
            ),
            ("POST", format!("/v1/machines/{id}/stop-sharing"), json!({})),
            (
                "POST",
                format!("/v1/machines/{id}/start-sharing"),
                json!({}),
            ),
            ("POST", format!("/v1/machines/{id}/confirm"), json!({})),
            ("POST", format!("/v1/machines/{id}/leave"), json!({})),
            ("DELETE", format!("/v1/machines/{id}"), json!({})),
        ]
    };
    // The phone's sign-in is an hour old (a refreshed token).
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64 - 3600);
    for (method, path, body) in attempts(&id) {
        let (status, v) = calls(method, path.clone(), body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {v}");
        assert_eq!(v["code"], "recent_sign_in_needed", "{method} {path}: {v}");
        assert_eq!(
            v["error"],
            cua_relay::device_api::PHONE_NEEDS_SIGN_IN,
            "{method} {path}"
        );
    }
    // Nothing changed: the computer is still its owner's alone and still there.
    let r = env
        .http
        .get(format!("{}/v1/machines/{id}", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["allow"], json!([]));
    assert_eq!(v["sharing"], true);
    // A computer with the same old sign-in is not held to it.
    let r = env
        .http
        .patch(format!("{}/v1/machines/{id}", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .json(&json!({"allow": ["friend@example.com"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // Just signed in, the phone may reshare (and so on). A pairing code is another
    // matter: a phone adds no computer, on any sign-in.
    env.issuer
        .set_auth_time("ada", cua_relay::assertion::now_secs() as i64);
    let (status, v) = calls(
        "PATCH",
        format!("/v1/machines/{id}"),
        json!({"allow": ["friend2@example.com"]}),
    )
    .await;
    assert!(status.is_success(), "{status} {v}");
    for age in [3600i64, 0] {
        env.issuer
            .set_auth_time("ada", cua_relay::assertion::now_secs() as i64 - age);
        let (status, v) = calls("POST", "/v1/pairing-codes".to_owned(), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{v}");
        assert_eq!(v["error"], cua_relay::pairing::PHONE_NO_PAIRING);
    }
    // A computer mints one.
    let r = env
        .http
        .post(format!("{}/v1/pairing-codes", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
}

/// A token with no enrolled device learns the names of its computers, not which
/// Space belongs to which computer or what a computer reports about itself.
#[tokio::test]
async fn a_token_without_a_device_lists_names_only() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let cs = env.enroll("ada", &computer).await;
    let (host, _) = env.machine("ada", &computer, &cs).await;
    let child = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let r = env
        .http
        .post(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .json(&json!({"id": child, "name": "work", "host": host, "meta": {"cua.host.os": "macos"}}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());
    // With the device: the host and the meta are there.
    let full: Value = env
        .http
        .get(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = |v: &Value, id: &str| {
        v["machines"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(row(&full, &child)["host"], host);
    assert_eq!(row(&full, &child)["meta"]["cua.host.os"], "macos");
    // Without it: names and ids.
    let locked: Value = env
        .http
        .get(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let r = row(&locked, &child);
    assert_eq!(r["name"], "work");
    assert!(r["host"].is_null(), "{r}");
    assert!(r["meta"].as_object().is_none_or(|m| m.is_empty()), "{r}");
}

/// `CUA_RELAY_PHONES=off` is the switch for "no phone, now" that keeps the rest
/// of phone support (and its hardening) on: every phone is refused at the device
/// gate, a computer of the account is untouched, and a phone comes back when it
/// is turned on again.
#[tokio::test]
async fn the_phones_switch_refuses_every_phone_and_nothing_else() {
    for phones in [true, false] {
        let env = start_phones(true, phones).await;
        let computer = Device::new("cua-cli", "macos");
        let phone = Device::new("cua-spaces-mobile", "ios");
        let cs = env.enroll("ada", &computer).await;
        let ps = env.enroll("ada", &phone).await;
        let (id, _) = env.machine("ada", &computer, &cs).await;
        let path = format!("/m/{id}/{THREAD}");
        let (status, body) = env
            .call(&phone, &ps, "ada", reqwest::Method::POST, &path)
            .await;
        if phones {
            assert_eq!(status, StatusCode::OK, "{body}");
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert!(
                body.to_string()
                    .contains(cua_relay::device_api::PHONE_SUPPORT_OFF),
                "{body}"
            );
        }
        let (status, _) = env
            .call(&computer, &cs, "ada", reqwest::Method::POST, &path)
            .await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a computer is untouched (phones {phones})"
        );
    }
}

/// Right after a relay starts, a request for a computer that is known but not connected
/// waits for it. If the computer that connects meanwhile is an old one (no phone
/// support), a phone's already-authorized request must not be forwarded to it.
#[tokio::test]
async fn a_phones_request_that_waits_is_not_forwarded_to_an_old_computer_that_reconnects() {
    for phone_support in [false, true] {
        let env = start_full(true, true, Duration::from_secs(10)).await;
        let computer = Device::new("cua-cli", "macos");
        let phone = Device::new("cua-spaces-mobile", "ios");
        let cs = env.enroll("ada", &computer).await;
        let ps = env.enroll("ada", &phone).await;
        // Registered, not connected: this relay "just started" and knows it.
        let id = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
        let reg: Value = env
            .http
            .post(format!("{}/v1/machines", env.base))
            .bearer_auth(env.token("ada", computer.client))
            .header("x-cua-device-session", &cs)
            .json(&json!({"id": id, "name": "late"}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let reached = Arc::new(AtomicUsize::new(0));
        let count = reached.clone();
        let app = axum::Router::new().fallback(axum::routing::any(move || {
            let count = count.clone();
            async move {
                count.fetch_add(1, Ordering::SeqCst);
                axum::Json(json!({"ok": true}))
            }
        }));
        let local = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let local_addr = local.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(local, app).await;
        });
        // The phone asks now; the computer connects half a second later.
        let ask = {
            let (http, base) = (env.http.clone(), env.base.clone());
            let token = env.token("ada", phone.client);
            let ps = ps.clone();
            let path = format!("/m/{id}/{THREAD}");
            tokio::spawn(async move {
                let r = http
                    .post(format!("{base}{path}"))
                    .bearer_auth(token)
                    .header("x-cua-device-session", ps)
                    .send()
                    .await
                    .unwrap();
                let status = r.status();
                (status, r.text().await.unwrap_or_default())
            })
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut join = cua_relay::client::JoinConfig::new(
            env.base.replace("http://", "ws://"),
            reg["machine_token"].as_str().unwrap().to_owned(),
            id.clone(),
            local_addr,
        );
        join.heartbeat = Duration::from_secs(2);
        join.phone_support = phone_support;
        tokio::spawn(cua_relay::client::run(join, env.stop.clone()));
        let (status, body) = tokio::time::timeout(Duration::from_secs(20), ask)
            .await
            .expect("answered")
            .unwrap();
        if phone_support {
            assert_eq!(status, StatusCode::OK, "{body}");
            assert_eq!(reached.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert!(
                body.contains(cua_relay::server::PHONE_OLD_COMPUTER),
                "{body}"
            );
            assert_eq!(
                reached.load(Ordering::SeqCst),
                0,
                "nothing reached the old computer"
            );
        }
    }
}

/// Removing every phone ends the phones' running streams and leaves the account's
/// computers' streams alone.
#[tokio::test]
async fn revoking_phones_cuts_phone_streams_and_not_computer_streams() {
    let env = start(true).await;
    let computer = Device::new("cua-cli", "macos");
    let phone = Device::new("cua-spaces-mobile", "ios");
    let cs = env.enroll("ada", &computer).await;
    let ps = env.enroll("ada", &phone).await;
    let (id, _) = env.machine("ada", &computer, &cs).await;
    let path = format!("{}/m/{id}/{THREAD}", env.base);
    let open = |d: &Device, s: &str| {
        env.http
            .post(&path)
            .bearer_auth(env.token("ada", d.client))
            .header("x-cua-device-session", s.to_owned())
            .body("slow")
            .send()
    };
    let mut phone_stream = open(&phone, &ps).await.unwrap();
    let mut computer_stream = open(&computer, &cs).await.unwrap();
    assert_eq!(phone_stream.status(), StatusCode::OK);
    assert!(phone_stream.chunk().await.unwrap().is_some());
    assert!(computer_stream.chunk().await.unwrap().is_some());
    let r = env
        .http
        .post(format!("{}/v1/devices/revoke-phones", env.base))
        .bearer_auth(env.token("ada", computer.client))
        .header("x-cua-device-session", &cs)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    // The phone's stream ends within a moment (an error or the end).
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        while let Ok(Some(_)) = phone_stream.chunk().await {}
    })
    .await;
    assert!(
        ended.is_ok(),
        "the phone's stream went on after it was removed"
    );
    // The computer's stream still delivers.
    for _ in 0..3 {
        let chunk = tokio::time::timeout(Duration::from_secs(2), computer_stream.chunk())
            .await
            .expect("the computer's stream is still alive")
            .unwrap();
        assert!(chunk.is_some(), "the computer's stream was cut");
    }
}

/// A new computer added with the account token alone is refused (S5) with a
/// stable code next to the reason, so the apps can ask the person to confirm
/// it is them instead of showing a bare failure.
#[tokio::test]
async fn a_new_computer_from_a_token_alone_is_refused_with_a_code() {
    let env = start(true).await;
    let id = format!("space-{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
    let r = env
        .http
        .post(format!("{}/v1/machines", env.base))
        .bearer_auth(env.token("ada", "cua-cli"))
        .json(&json!({"id": id, "name": "new mac"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let body: Value = r.json().await.unwrap();
    assert_eq!(
        body["code"].as_str(),
        Some(cua_relay::api::NEW_COMPUTER_NEEDS_PROOF),
        "{body}"
    );
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("pairing code")),
        "{body}"
    );
}
