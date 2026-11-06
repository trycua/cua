// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `CUA_RELAY_DEVICE_REQUIRE_APPROVAL=phones`: strict device approval for
//! phone-class devices only. The class comes from the enrolling token's
//! authorized party (`azp`) against `CUA_RELAY_PHONE_CLIENTS`, or from a
//! reported phone platform; every other device keeps the default policy.
//! The matrix: {mode} x {client} x {account already has a device} x {MFA}.

use std::sync::Arc;

use base64::Engine as _;
use cua_push::{MockProvider, PushHub, Registry};
use cua_relay::devices::{register_message, session_message, DevicePolicy};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::push_api::PushConfig;
use cua_relay::server::{Relay, RelayConfig};
use ring::signature::KeyPair as _;
use serde_json::{json, Value};

const ISSUER: &str = "https://auth.test/realms/cua";
const PHONE: &str = "cua-spaces-mobile";
/// Clients that are not phone clients: the CLI, the desktop app, the account
/// app (per-browser devices) and an unattended device-code install.
const COMPUTER_CLIENTS: [&str; 4] = [
    "cua-cli",
    "cua-desktop",
    "cua-account-app",
    "cua-device-code",
];

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

struct Key {
    pair: ring::signature::EcdsaKeyPair,
    id: String,
}

impl Key {
    fn new() -> Self {
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let id = cua_relay::devices::device_id(pair.public_key().as_ref());
        Self { pair, id }
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
}

async fn start(require_approval: bool, phones_only: bool) -> Env {
    let issuer = FakeIssuer::new(ISSUER);
    let hub = Arc::new(PushHub::new(
        Registry::in_memory(),
        Arc::new(MockProvider::new()),
    ));
    let mut oidc = OidcConfig::new(ISSUER);
    oidc.authorized_parties = std::iter::once(PHONE)
        .chain(COMPUTER_CLIENTS)
        .map(String::from)
        .collect();
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(oidc, issuer.jwks()))),
        device_policy: DevicePolicy {
            grace_secs: 0,
            require_approval,
            approval_phones_only: phones_only,
            ..DevicePolicy::default()
        },
        push: Some(PushConfig::new(hub).with_phone_clients(vec![PHONE.into()])),
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
        http: reqwest::Client::new(),
    }
}

impl Env {
    /// Registers `key` signed in through `client` with a fresh sign-in and a
    /// verified email, `mfa` or not. Returns the JSON answer.
    async fn register(
        &self,
        user: &str,
        client: &str,
        platform: &str,
        mfa: bool,
        key: &Key,
    ) -> Value {
        self.issuer
            .set_auth_time(user, cua_relay::assertion::now_secs() as i64);
        self.issuer
            .set_amr(user, if mfa { &["pwd", "otp"] } else { &["pwd"] });
        let token = self.issuer.token_for_client(
            user,
            Some(&format!("{user}@example.com")),
            "cua-relay",
            600,
            client,
        );
        let ts = cua_relay::assertion::now_secs();
        let r = self
            .http
            .post(format!("{}/v1/devices", self.base))
            .bearer_auth(token)
            .json(&json!({
                "public_key": b64().encode(key.pair.public_key().as_ref()),
                "name": "device", "platform": platform, "ts": ts,
                "sig": key.sign(&register_message(&key.id, ts)), "bootstrap": true,
            }))
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "register: {}", r.status());
        r.json().await.unwrap()
    }

    /// Whether `key` can open a device session.
    async fn can_open_session(&self, user: &str, client: &str, key: &Key) -> bool {
        let token = self.issuer.token_for_client(
            user,
            Some(&format!("{user}@example.com")),
            "cua-relay",
            600,
            client,
        );
        let ts = cua_relay::assertion::now_secs();
        self.http
            .post(format!("{}/v1/devices/session", self.base))
            .bearer_auth(token)
            .json(&json!({"device_id": key.id, "ts": ts,
                          "sig": key.sign(&session_message(&key.id, ts))}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success()
    }
}

#[tokio::test]
async fn approval_mode_matrix_over_http() {
    let modes = [
        ("false", false, false),
        ("true", true, false),
        ("phones", true, true),
    ];
    let mut n = 0;
    for (mode, require, phones_only) in modes {
        let env = start(require, phones_only).await;
        let clients = std::iter::once((PHONE, "ios", true))
            .chain(COMPUTER_CLIENTS.iter().map(|c| (*c, "linux", false)));
        for (client, platform, phone) in clients {
            for has_device in [false, true] {
                for mfa in [false, true] {
                    n += 1;
                    let user = format!("u{n}");
                    if has_device {
                        let first = Key::new();
                        let r = env.register(&user, "cua-cli", "macos", false, &first).await;
                        assert_eq!(r["device"]["state"], "enrolled", "the first device enrolls");
                    }
                    let key = Key::new();
                    let r = env.register(&user, client, platform, mfa, &key).await;
                    let strict = require && (phone || !phones_only);
                    let expect = !has_device || !strict || mfa;
                    let ctx = format!(
                        "mode={mode} client={client} has_device={has_device} mfa={mfa}: {r}"
                    );
                    assert_eq!(r["device"]["state"] == "enrolled", expect, "{ctx}");
                    assert_eq!(r["code"].is_null(), expect, "{ctx}");
                    assert_eq!(r["device"]["phone"], phone, "{ctx}");
                    assert_eq!(
                        env.can_open_session(&user, client, &key).await,
                        expect,
                        "{ctx}"
                    );
                }
            }
        }
    }
    assert_eq!(n, 3 * 5 * 4);
}

/// The class is decided by the token's client, not by what the device says:
/// a phone client reporting a computer platform is still a phone (it cannot
/// skip approval), and a non-phone client reporting a phone platform becomes
/// a phone (it only makes itself the stricter, smaller class).
#[tokio::test]
async fn the_class_comes_from_the_token_client_not_the_claimed_platform() {
    let env = start(true, true).await;
    let first = Key::new();
    env.register("ada", "cua-cli", "macos", false, &first).await;

    let sneaky_phone = Key::new();
    let r = env
        .register("ada", PHONE, "macos", false, &sneaky_phone)
        .await;
    assert_eq!(r["device"]["state"], "pending", "{r}");
    assert_eq!(r["device"]["phone"], true, "{r}");

    let claims_phone = Key::new();
    let r = env
        .register("ada", "cua-cli", "ios", false, &claims_phone)
        .await;
    assert_eq!(r["device"]["state"], "pending", "{r}");
    assert_eq!(r["device"]["phone"], true, "{r}");

    // A computer client is unaffected on the same account.
    let computer = Key::new();
    let r = env
        .register("ada", "cua-device-code", "linux", false, &computer)
        .await;
    assert_eq!(r["device"]["state"], "enrolled", "{r}");
    assert_eq!(r["device"]["phone"], false, "{r}");
}

fn relay_with(value: &str, extra: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_cua-relay"));
    cmd.arg("--listen")
        .arg("127.0.0.1:0")
        .env_clear()
        .env("CUA_RELAY_TOKENS", "test-only-token")
        .env("CUA_RELAY_DEVICE_REQUIRE_APPROVAL", value);
    // Windows cannot start its network stack without SYSTEMROOT (os error
    // 10106); keep it through the cleared environment.
    for key in ["SYSTEMROOT", "SystemRoot"] {
        if let Some(v) = std::env::var_os(key) {
            cmd.env(key, v);
        }
    }
    for (k, v) in extra {
        cmd.env(k, v);
    }
    cmd.output().expect("run cua-relay")
}

/// The binary refuses `phones` when no phone client can exist (push off, so
/// phone support off) and refuses a value it does not know, instead of
/// silently approving nothing.
#[test]
fn the_binary_refuses_phones_without_phone_clients_and_unknown_values() {
    let out = relay_with("phones", &[]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("CUA_RELAY_DEVICE_REQUIRE_APPROVAL=phones needs"),
        "{err}"
    );

    let out = relay_with("phone", &[]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("use true, false or phones"), "{err}");
}

/// The bare flag and the old boolean values still parse.
#[test]
fn the_flag_keeps_its_boolean_forms() {
    for args in [
        vec!["--device-require-approval", "--help"],
        vec!["--device-require-approval=phones", "--help"],
    ] {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_cua-relay"))
            .args(&args)
            .env_clear()
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
    }
}
