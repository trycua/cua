// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Log lines name an account by a short keyed tag, never its id or email
//! (CUA-2027).

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine as _;
use cua_relay::devices::{register_message, DevicePolicy};
use cua_relay::oidc::testing::FakeIssuer;
use cua_relay::oidc::{OidcConfig, OidcValidator};
use cua_relay::server::{Relay, RelayConfig};
use ring::signature::KeyPair as _;
use serde_json::json;

const ISSUER: &str = "https://auth.test/realms/cua";
const EMAIL: &str = "grace.hopper@example.com";
const SUB: &str = "user-5c2d8e11-sub-do-not-log";

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

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
}

#[tokio::test]
async fn account_flows_log_a_tag_and_never_the_account_id_or_email() {
    let buffer = logs();
    let state = tempfile::tempdir().unwrap();
    let issuer = FakeIssuer::new(ISSUER);
    let relay = Relay::new(RelayConfig {
        oidc: Some(Arc::new(OidcValidator::with_jwks(
            OidcConfig::new(ISSUER),
            issuer.jwks(),
        ))),
        state_file: Some(state.path().join("machines.json")),
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
    let token = issuer.token(SUB, Some(EMAIL), "cua-relay", 300);
    let http = reqwest::Client::new();

    let machine = uuid::Uuid::new_v4().simple().to_string();
    let r = http
        .post(format!("{base}/v1/machines"))
        .bearer_auth(&token)
        .json(&json!({"id": machine, "name": "grace's laptop"}))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    let rng = ring::rand::SystemRandom::new();
    let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
    let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
    let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
    let device_id = cua_relay::devices::device_id(pair.public_key().as_ref());
    let ts = cua_relay::assertion::now_secs();
    let sig = b64().encode(
        pair.sign(&rng, register_message(&device_id, ts).as_bytes())
            .unwrap()
            .as_ref(),
    );
    let r = http
        .post(format!("{base}/v1/devices"))
        .bearer_auth(&token)
        .json(&json!({
            "public_key": b64().encode(pair.public_key().as_ref()),
            "name": "laptop",
            "ts": ts,
            "sig": sig,
        }))
        .send()
        .await
        .unwrap();
    assert!(r.status().is_success(), "{}", r.status());

    let text = String::from_utf8_lossy(&buffer.0.lock().unwrap()).into_owned();
    let tag = cua_relay::ident::tag(SUB);
    assert!(text.contains("machine registered"), "{text}");
    assert!(text.contains("device registered"), "{text}");
    assert!(
        text.lines()
            .filter(|l| l.contains("registered"))
            .all(|l| l.contains(&format!("account={tag}"))),
        "every account line carries the tag {tag}:\n{text}"
    );
    for secret in [SUB, EMAIL, "5c2d8e11"] {
        assert!(!text.contains(secret), "{secret:?} leaked:\n{text}");
    }
    // The key sits next to the state file, 0600.
    let key = state.path().join("machines.json.analytics-key");
    assert_eq!(std::fs::read(&key).unwrap().len(), 32);
}

/// Source guard: a log macro in the relay never takes an account id, user
/// id or email as a field value without going through `ident::tag`.
#[test]
fn log_macros_do_not_take_raw_account_ids_or_emails() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(&src).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (line, body) in log_macros(&text) {
            let code = strip_strings(&body);
            let risky = ["sub", "user", "account", "email", "owner", "approver"]
                .iter()
                .any(|f| has_word(&code, f));
            if risky && !body.contains("ident::tag") && !body.contains("= %tag") {
                bad.push(format!("{}:{line}: {}", path.display(), one_line(&body)));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "raw identifiers in log macros:\n{}",
        bad.join("\n")
    );
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `(line, text)` of every `info!/warn!/error!/debug!/trace!(...)`.
fn log_macros(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for level in ["info", "warn", "error", "debug", "trace"] {
        let needle = format!("{level}!(");
        let mut from = 0;
        while let Some(at) = text[from..].find(&needle) {
            let start = from + at;
            let mut depth = 0i32;
            let mut end = start + needle.len() - 1;
            let mut in_str = false;
            for (i, c) in text[end..].char_indices() {
                match c {
                    '"' if !text[..end + i].ends_with('\\') => in_str = !in_str,
                    '(' if !in_str => depth += 1,
                    ')' if !in_str => {
                        depth -= 1;
                        if depth == 0 {
                            end += i + 1;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let line = text[..start].matches('\n').count() + 1;
            out.push((line, text[start..end.min(text.len())].to_owned()));
            from = start + needle.len();
        }
    }
    out
}

fn strip_strings(s: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    let mut prev = ' ';
    for c in s.chars() {
        if c == '"' && prev != '\\' {
            in_str = !in_str;
        } else if !in_str {
            out.push(c);
        }
        prev = c;
    }
    out
}

fn has_word(code: &str, word: &str) -> bool {
    code.match_indices(word).any(|(i, _)| {
        let before = code[..i].chars().next_back();
        let after = code[i + word.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        edge(before) && edge(after)
    })
}
