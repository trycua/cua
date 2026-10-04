// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every 401 the relay answers logs one `cua_relay::auth` line naming the
//! reason, route and client kind, and never a token, token fragment, email,
//! account id, IP address, machine id or device name.

use std::io::Write;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use cua_relay::devices::{register_message, DevicePolicy};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use ring::signature::KeyPair as _;
use serde_json::json;

const ISSUER: &str = "https://auth.test/realms/cua";
const EMAIL: &str = "ada.lovelace@example.com";
const SUB: &str = "user-7f3a9c0e-sub";
const DEVICE_NAME: &str = "adas-secret-laptop";
const ADMIN_TOKEN: &str = "admin-token-do-not-log-9b1e";

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Everything logged in this test binary.
fn logs() -> &'static Buffer {
    static LOGS: OnceLock<Buffer> = OnceLock::new();
    LOGS.get_or_init(|| {
        let buffer = Buffer::default();
        let writer = buffer.clone();
        tracing_subscriber::fmt()
            .with_ansi(false)
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(move || writer.clone())
            .init();
        buffer
    })
}

fn take_logs() -> String {
    let mut buf = logs().0.lock().unwrap();
    let text = String::from_utf8_lossy(&buf).into_owned();
    buf.clear();
    text
}

/// The one auth line logged since the last call.
fn auth_line() -> String {
    let text = take_logs();
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| l.contains("cua_relay::auth"))
        .collect();
    assert_eq!(lines.len(), 1, "exactly one auth line in:\n{text}");
    lines[0].to_owned()
}

fn assert_fields(line: &str, reason: &str, route: &str, client: &str) {
    assert!(line.contains("401 unauthenticated"), "{line}");
    assert!(line.contains(&format!("reason=\"{reason}\"")), "{line}");
    assert!(line.contains(&format!("route=\"{route}\"")), "{line}");
    assert!(line.contains(&format!("client=\"{client}\"")), "{line}");
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

async fn start(oidc: OidcConfig, issuer: &FakeIssuer) -> String {
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(oidc, issuer.jwks()))),
        admin_token: Some(ADMIN_TOKEN.into()),
        device_policy: DevicePolicy {
            grace_secs: 3600,
            ..DevicePolicy::default()
        },
        ..RelayConfig::default()
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        let _ = relay.serve(listener, std::future::pending()).await;
    });
    base
}

async fn get(url: &str, token: Option<&str>, ua: Option<&str>) -> reqwest::StatusCode {
    let mut r = reqwest::Client::new().get(url);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    if let Some(ua) = ua {
        r = r.header("user-agent", ua);
    }
    r.send().await.unwrap().status()
}

#[tokio::test]
async fn each_401_logs_its_reason_and_no_secret() {
    logs();
    let issuer = FakeIssuer::new(ISSUER);
    let base = start(OidcConfig::new(ISSUER), &issuer).await;
    let machines = format!("{base}/v1/machines");
    let mut secrets: Vec<String> = vec![
        EMAIL.into(),
        SUB.into(),
        DEVICE_NAME.into(),
        ADMIN_TOKEN.into(),
        "127.0.0.1".into(),
    ];
    let check = |token: &str, line: &str, secrets: &mut Vec<String>| {
        secrets.push(token.to_owned());
        for part in token.split('.') {
            secrets.push(part.to_owned());
        }
        for s in secrets.iter().filter(|s| s.len() >= 8) {
            assert!(!line.contains(s.as_str()), "{s:?} leaked into {line}");
            // No 12-char fragment of a token either.
            if !s.contains('@') && s.len() >= 12 {
                for start in (0..=s.len() - 12).step_by(4) {
                    if let Some(frag) = s.get(start..start + 12) {
                        assert!(!line.contains(frag), "fragment {frag:?} leaked: {line}");
                    }
                }
            }
        }
    };

    // Missing token.
    assert_eq!(get(&machines, None, Some("cua-cli/0.9.0")).await, 401);
    assert_fields(&auth_line(), "missing_token", "machines", "cli");

    // Wrong audience: the token's public audience is logged.
    let token = issuer.token(SUB, Some(EMAIL), "account", 300);
    assert_eq!(get(&machines, Some(&token), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "wrong_audience", "machines", "sdk");
    assert!(
        line.contains(&format!("token_issuer=\"{ISSUER}\"")),
        "{line}"
    );
    assert!(line.contains("token_audience=\"account\""), "{line}");
    assert!(!line.contains("clock_skew_secs"), "{line}");
    check(&token, &line, &mut secrets);

    // An audience that is not a public name is not logged.
    let token = issuer.token(SUB, Some(EMAIL), "private-client-7731", 300);
    assert_eq!(get(&machines, Some(&token), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "wrong_audience", "machines", "sdk");
    assert!(line.contains("token_audience=\"other\""), "{line}");
    check(&token, &line, &mut secrets);

    // Expired (beyond the 30 s leeway), with the clock skew.
    let token = issuer.token(SUB, Some(EMAIL), "cua-relay", -600);
    let ua = "Cua%20Spaces/5 CFNetwork/1568 Darwin/25.5.0";
    assert_eq!(get(&machines, Some(&token), Some(ua)).await, 401);
    let line = auth_line();
    assert_fields(&line, "expired", "machines", "app");
    let skew: i64 = line
        .split("clock_skew_secs=")
        .nth(1)
        .and_then(|s| s.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no skew in {line}"));
    assert!((599..=610).contains(&skew), "{skew}");
    check(&token, &line, &mut secrets);

    // Bad signature: one token's signature on another token's claims.
    let a = issuer.token(SUB, Some(EMAIL), "cua-relay", 300);
    let b = issuer.token("someone-else", None, "cua-relay", 300);
    let forged = format!(
        "{}.{}",
        a.rsplit_once('.').unwrap().0,
        b.rsplit_once('.').unwrap().1
    );
    assert_eq!(get(&machines, Some(&forged), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "bad_signature", "machines", "sdk");
    check(&forged, &line, &mut secrets);

    // Signed by a key the issuer does not publish.
    let stranger = FakeIssuer::new(ISSUER);
    let token = stranger.token(SUB, Some(EMAIL), "cua-relay", 300);
    assert_eq!(get(&machines, Some(&token), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "unknown_signing_key", "machines", "sdk");
    check(&token, &line, &mut secrets);

    // Malformed.
    assert_eq!(get(&machines, Some("not-a-jwt-secretish"), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "malformed", "machines", "sdk");
    check("not-a-jwt-secretish", &line, &mut secrets);

    // Unknown (revoked) machine token.
    let token = "cmt_0123456789abcdef0123456789abcdef";
    assert_eq!(get(&machines, Some(token), None).await, 401);
    let line = auth_line();
    assert_fields(&line, "unknown_machine_token", "machines", "machine");
    check(token, &line, &mut secrets);

    // Device enrollment: missing token, then a stale proof with its skew.
    let devices = format!("{base}/v1/devices");
    assert_eq!(get(&devices, None, None).await, 401);
    assert_fields(&auth_line(), "missing_token", "devices", "sdk");
    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let device_id = cua_relay::devices::device_id(pair.public_key().as_ref());
    secrets.push(device_id.clone());
    let ts = cua_relay::assertion::now_secs() - 900;
    let sig = b64().encode(
        pair.sign(&rng, register_message(&device_id, ts).as_bytes())
            .unwrap()
            .as_ref(),
    );
    let token = issuer.token(SUB, Some(EMAIL), "cua-relay", 300);
    let status = reqwest::Client::new()
        .post(&devices)
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": DEVICE_NAME,
            "ts": ts,
            "sig": sig,
        }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(status, 401);
    let line = auth_line();
    assert_fields(&line, "device_proof_stale", "devices", "sdk");
    assert!(
        line.contains("clock_skew_secs=-90") || line.contains("clock_skew_secs=-89"),
        "{line}"
    );
    check(&token, &line, &mut secrets);
    check(&sig, &line, &mut secrets);

    // Admin.
    assert_eq!(
        get(
            &format!("{base}/relay/v1/machines"),
            Some("wrong-admin"),
            None
        )
        .await,
        401
    );
    assert_fields(&auth_line(), "invalid_admin_token", "admin", "sdk");

    // Proxy: an account machine without a token, and a static-token
    // machine without any spacesd credential.
    let token = issuer.token(SUB, Some(EMAIL), "cua-relay", 300);
    let machine_id = format!("m{}", uuid::Uuid::new_v4().simple());
    secrets.push(machine_id.clone());
    let status = reqwest::Client::new()
        .post(&machines)
        .bearer_auth(&token)
        .json(&json!({"id": machine_id, "name": DEVICE_NAME}))
        .send()
        .await
        .unwrap()
        .status();
    assert!(status.is_success(), "{status}");
    take_logs();
    let ua = "Mozilla/5.0 (Macintosh)";
    assert_eq!(
        get(&format!("{base}/m/{machine_id}/v1/x"), None, Some(ua)).await,
        401
    );
    let line = auth_line();
    assert_fields(&line, "missing_token", "proxy", "browser");
    check(&token, &line, &mut secrets);
    assert_eq!(
        get(&format!("{base}/m/notregistered01/v1/x"), None, None).await,
        401
    );
    assert_fields(&auth_line(), "spacesd_credential_missing", "proxy", "sdk");

    // Machine join with an unknown relay token.
    let mut request =
        tokio_tungstenite::tungstenite::client::IntoClientRequest::into_client_request(format!(
            "{}{}",
            base.replace("http://", "ws://"),
            cua_relay::CONNECT_PATH
        ))
        .unwrap();
    let h = request.headers_mut();
    h.insert(cua_relay::MACHINE_ID_HEADER, machine_id.parse().unwrap());
    h.insert(cua_relay::VERSION_HEADER, "0.2.2".parse().unwrap());
    h.insert(
        "authorization",
        "Bearer static-relay-token-guess".parse().unwrap(),
    );
    let err = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");
    let line = auth_line();
    assert_fields(&line, "invalid_relay_token", "connect", "spacesd");
    check("static-relay-token-guess", &line, &mut secrets);

    wrong_issuer_is_named_only_when_public().await;
}

/// Wrong issuer: named only when public. (Called from the test above: the
/// log buffer is shared, so the cases run in sequence.)
async fn wrong_issuer_is_named_only_when_public() {
    // Configured for another issuer: a cua.ai issuer is logged as-is.
    let issuer = FakeIssuer::new("https://auth.cua.ai/realms/other");
    let base = start(OidcConfig::new(ISSUER), &issuer).await;
    let token = issuer.token(SUB, Some(EMAIL), "cua-relay", 300);
    assert_eq!(
        get(&format!("{base}/v1/machines"), Some(&token), None).await,
        401
    );
    let text = take_logs();
    let line = text
        .lines()
        .find(|l| l.contains("reason=\"wrong_issuer\""))
        .unwrap_or_else(|| panic!("no wrong_issuer line in:\n{text}"));
    assert!(
        line.contains("token_issuer=\"https://auth.cua.ai/realms/other\""),
        "{line}"
    );
    assert!(!line.contains(EMAIL) && !line.contains(SUB), "{line}");

    // A private issuer URL is not.
    let private = FakeIssuer::new("https://idp.internal.example/tenant-4471");
    let base = start(OidcConfig::new(ISSUER), &private).await;
    let token = private.token(SUB, None, "cua-relay", 300);
    assert_eq!(
        get(&format!("{base}/v1/machines"), Some(&token), None).await,
        401
    );
    let text = take_logs();
    let line = text
        .lines()
        .find(|l| l.contains("reason=\"wrong_issuer\""))
        .unwrap_or_else(|| panic!("no wrong_issuer line in:\n{text}"));
    assert!(line.contains("token_issuer=\"other\""), "{line}");
    assert!(!line.contains("tenant-4471"), "{line}");
}
