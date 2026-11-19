// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Drift guard: every way a real sign-in (`cua-auth`, against a loopback
//! issuer, with a file store in a temporary directory, never the keychain)
//! fails reaches the shells as a typed reason with words, never silently
//! and never as "Unknown".
//!
//! The shells see a sign-in error as the SDK's `CuaError` case and its
//! words; [`sdk_case`] is the SDK's mapping (`cua-sdk/src/native/auth.rs`,
//! `impl From<cua_auth::Error> for CuaError`), exhaustive so a new
//! `cua_auth::Error` case does not build here until it has a reason.

use cua_auth::{Credentials, Flow, Method, Oidc, Session, Store};
use cua_spaces_app_core::settings::SignInPhase;
use cua_spaces_app_core::sign_in::{self, SignInFailure};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The SDK's case for a sign-in error, as the shells receive it.
fn sdk_case(e: &cua_auth::Error) -> &'static str {
    use cua_auth::Error as E;
    match e {
        E::NotLoggedIn | E::Unauthenticated(_) => "Unauthenticated",
        E::Http(_) => "Http",
        E::Timeout(_) => "Timeout",
        E::Unsupported(_) => "Unsupported",
        E::InvalidArgument(_) => "InvalidArgument",
        E::Store(_) => "Internal",
    }
}

/// The reason a shell shows for `e`, and checks it is shown with words.
fn shown(e: &cua_auth::Error) -> SignInFailure {
    let phase = sign_in::failed_from_error(sdk_case(e), &e.to_string());
    let SignInPhase::Failed {
        reason,
        message,
        detail,
    } = &phase
    else {
        panic!("{phase:?}")
    };
    assert!(message.starts_with("Sign-in didn't finish."), "{message}");
    assert_eq!(detail.as_deref(), Some(e.to_string().trim()));
    let footer = sign_in::footer(&sign_in::SignInFooterInput {
        sign_in: phase.clone(),
        can_sign_in: true,
        ..Default::default()
    });
    assert_eq!(footer.action_label.as_deref(), Some("Try again"));
    assert!(footer.toast.is_some(), "a failure always toasts");
    *reason
}

/// What the loopback issuer answers.
#[derive(Clone)]
struct Script {
    discovery_status: u16,
    /// The token endpoint discovery names (none: this issuer's).
    token_endpoint: Option<String>,
    /// Browser + PKCE accepted (the preflight answers 200).
    pkce_ok: bool,
    device_start: (u16, Value),
    /// The device grant's answers in order (the last one repeats).
    device_polls: Vec<(u16, Value)>,
    device_expires_in: u64,
    /// The authorization code exchange.
    exchange: (u16, Value),
}

impl Default for Script {
    fn default() -> Self {
        Script {
            discovery_status: 200,
            token_endpoint: None,
            pkce_ok: false,
            device_start: (200, Value::Null),
            device_polls: vec![(400, json!({"error": "authorization_pending"}))],
            device_expires_in: 2,
            exchange: (200, tokens()),
        }
    }
}

fn tokens() -> Value {
    use base64::Engine;
    let e = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    let claims = json!({"preferred_username": "ellie", "email": "ellie@example.com"});
    json!({
        "access_token": format!("{}.{}.sig", e(br#"{"alg":"none"}"#), e(claims.to_string().as_bytes())),
        "refresh_token": "rt", "expires_in": 3600
    })
}

/// A loopback issuer that follows `script`; returns its URL.
async fn issuer(script: Script) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let polls = Arc::new(Mutex::new(0usize));
    let b = base.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let (script, base, polls) = (script.clone(), b.clone(), polls.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    if buf.len() > 1 << 16 {
                        return;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let len: usize = head
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse().unwrap_or(0))
                    })
                    .unwrap_or(0)
                    .min(1 << 16);
                while buf.len() < head_end + len {
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let body = String::from_utf8_lossy(&buf[head_end..]).to_string();
                let mut first = head.lines().next().unwrap_or_default().split_whitespace();
                let method = first.next().unwrap_or_default().to_string();
                let path = first
                    .next()
                    .unwrap_or_default()
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let grant = url::form_urlencoded::parse(body.as_bytes())
                    .find(|(k, _)| k == "grant_type")
                    .map(|(_, v)| v.into_owned());
                let (status, v) = match (method.as_str(), path.as_str()) {
                    ("GET", "/.well-known/openid-configuration") => (
                        script.discovery_status,
                        json!({
                            "authorization_endpoint": format!("{base}/auth"),
                            "token_endpoint": script.token_endpoint.clone().unwrap_or(format!("{base}/token")),
                            "device_authorization_endpoint": format!("{base}/device"),
                        }),
                    ),
                    ("GET", "/auth") if script.pkce_ok => (200, json!({})),
                    ("GET", "/auth") => (400, json!({"error": "Invalid parameter: redirect_uri"})),
                    ("POST", "/device") if script.device_start.0 == 200 => (
                        200,
                        json!({"device_code": "dc", "user_code": "4F2-91K",
                            "verification_uri": "https://login.test/device",
                            "expires_in": script.device_expires_in, "interval": 1}),
                    ),
                    ("POST", "/device") => script.device_start.clone(),
                    ("POST", "/token") if grant.as_deref() == Some("authorization_code") => {
                        script.exchange.clone()
                    }
                    ("POST", "/token") => {
                        let mut n = polls.lock().unwrap();
                        let i = (*n).min(script.device_polls.len() - 1);
                        *n += 1;
                        script.device_polls[i].clone()
                    }
                    _ => (404, json!({})),
                };
                let body = v.to_string();
                let resp = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    base
}

fn session(url: &str, dir: &std::path::Path) -> Session {
    Session::new(
        Oidc::new(url, "cua-cli"),
        Store::File(dir.join("credentials.json")),
    )
}

/// A port nothing listens on (bound, then closed).
async fn closed_port() -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

/// Runs a device-code sign-in to its end.
async fn device(script: Script) -> Result<Credentials, cua_auth::Error> {
    let url = issuer(script).await;
    let d = tempfile::tempdir().unwrap();
    let p = session(&url, d.path()).begin_login(Flow::Device).await?;
    assert_eq!(p.method, Method::Device);
    tokio::time::timeout(Duration::from_secs(30), p.complete())
        .await
        .expect("the device flow ends by itself")
}

/// Starts a browser sign-in and answers its redirect with `query`.
async fn browser(script: Script, query: impl Fn(&str) -> String) -> cua_auth::Error {
    let url = issuer(Script {
        pkce_ok: true,
        ..script
    })
    .await;
    let d = tempfile::tempdir().unwrap();
    let p = session(&url, d.path())
        .begin_login(Flow::Browser)
        .await
        .unwrap();
    let u = url::Url::parse(&p.url).unwrap();
    let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
    let redirect = url::Url::parse(&q["redirect_uri"]).unwrap();
    let done = tokio::spawn(tokio::time::timeout(Duration::from_secs(20), p.complete()));
    let mut s =
        tokio::net::TcpStream::connect((redirect.host_str().unwrap(), redirect.port().unwrap()))
            .await
            .unwrap();
    let req = format!(
        "GET /callback?{} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n",
        query(&q["state"])
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut page = String::new();
    let _ = s.read_to_string(&mut page).await;
    done.await.unwrap().unwrap().unwrap_err()
}

#[tokio::test]
async fn an_unreachable_sign_in_service_is_offline() {
    let port = closed_port().await;
    let d = tempfile::tempdir().unwrap();
    let e = session(&format!("http://127.0.0.1:{port}"), d.path())
        .begin_login(Flow::Auto)
        .await
        .err()
        .unwrap();
    assert!(e.to_string().starts_with(cua_auth::UNREACHABLE), "{e}");
    assert_eq!(shown(&e), SignInFailure::Offline);
}

#[tokio::test]
async fn a_broken_sign_in_service_is_its_problem() {
    let url = issuer(Script {
        discovery_status: 502,
        ..Default::default()
    })
    .await;
    let d = tempfile::tempdir().unwrap();
    let e = session(&url, d.path())
        .begin_login(Flow::Auto)
        .await
        .err()
        .unwrap();
    assert_eq!(shown(&e), SignInFailure::ServiceUnavailable, "{e}");

    // The device code is refused at the start (a client the provider
    // does not allow): the service's problem, not the person's code.
    let e = device(Script {
        device_start: (400, json!({"error": "unauthorized_client"})),
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert_eq!(shown(&e), SignInFailure::ServiceUnavailable, "{e}");
}

#[tokio::test]
async fn a_code_never_confirmed_is_not_confirmed() {
    // Pending until the code expires.
    let e = device(Script::default()).await.unwrap_err();
    assert!(matches!(e, cua_auth::Error::Timeout(_)), "{e}");
    assert_eq!(shown(&e), SignInFailure::NotConfirmed);
    // The provider says the code expired.
    let e = device(Script {
        device_polls: vec![(400, json!({"error": "expired_token"}))],
        device_expires_in: 60,
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert!(matches!(e, cua_auth::Error::Timeout(_)), "{e}");
    assert_eq!(shown(&e), SignInFailure::NotConfirmed);
}

#[tokio::test]
async fn declining_in_the_browser_is_declined() {
    let e = device(Script {
        device_polls: vec![(400, json!({"error": "access_denied"}))],
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert!(e.to_string().starts_with(cua_auth::LOGIN_DECLINED), "{e}");
    assert_eq!(shown(&e), SignInFailure::Declined);

    let e = browser(Script::default(), |state| {
        format!("error=access_denied&error_description=User+denied&state={state}")
    })
    .await;
    assert!(e.to_string().starts_with(cua_auth::LOGIN_DECLINED), "{e}");
    assert_eq!(shown(&e), SignInFailure::Declined);
}

#[tokio::test]
async fn a_refused_code_is_rejected_and_a_broken_exchange_is_the_service() {
    let e = browser(
        Script {
            exchange: (
                400,
                json!({"error": "invalid_grant", "error_description": "Code not valid"}),
            ),
            ..Default::default()
        },
        |state| format!("code=c&state={state}"),
    )
    .await;
    assert_eq!(shown(&e), SignInFailure::CodeRejected, "{e}");

    let e = browser(
        Script {
            exchange: (500, json!({"error": "server_error"})),
            ..Default::default()
        },
        |state| format!("code=c&state={state}"),
    )
    .await;
    assert_eq!(shown(&e), SignInFailure::ServiceUnavailable, "{e}");
}

#[tokio::test]
async fn a_network_drop_while_waiting_is_retried_then_offline() {
    // The provider stumbles once, then the person confirms: signed in.
    let c = device(Script {
        device_polls: vec![
            (503, json!({"error": "temporarily_unavailable"})),
            (200, tokens()),
        ],
        device_expires_in: 30,
        ..Default::default()
    })
    .await
    .unwrap();
    assert!(c.access_token.contains('.'));

    // The token endpoint stays unreachable until the code expires: that,
    // not "expired", is what the person sees.
    let port = closed_port().await;
    let e = device(Script {
        token_endpoint: Some(format!("http://127.0.0.1:{port}/token")),
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert!(e.to_string().starts_with(cua_auth::UNREACHABLE), "{e}");
    assert_eq!(shown(&e), SignInFailure::Offline);

    // The provider keeps failing until the code expires.
    let e = device(Script {
        device_polls: vec![(503, json!({"error": "temporarily_unavailable"}))],
        ..Default::default()
    })
    .await
    .unwrap_err();
    assert_eq!(shown(&e), SignInFailure::ServiceUnavailable, "{e}");
}

#[tokio::test]
async fn a_session_that_cannot_be_saved_says_so() {
    let url = issuer(Script {
        device_polls: vec![(200, tokens())],
        ..Default::default()
    })
    .await;
    let d = tempfile::tempdir().unwrap();
    // The store's directory is a file: every write fails, as a locked
    // keychain's would.
    let blocker = d.path().join("not-a-directory");
    std::fs::write(&blocker, b"x").unwrap();
    let s = Session::new(
        Oidc::new(&url, "cua-cli"),
        Store::File(blocker.join("credentials.json")),
    );
    let p = s.begin_login(Flow::Device).await.unwrap();
    let c = p.complete().await.unwrap();
    let e = s.install(c).await.unwrap_err();
    assert!(matches!(e, cua_auth::Error::Store(_)), "{e}");
    assert_eq!(shown(&e), SignInFailure::CouldNotSave);
}

#[test]
fn every_sdk_case_a_sign_in_can_fail_with_has_a_reason() {
    // The cases `sdk_case` can give, with representative words.
    for (case, words) in [
        ("Unauthenticated", "Code not valid"),
        ("Http", "OIDC discovery failed (HTTP 500)"),
        (
            "Timeout",
            "device authorization expired before it was completed",
        ),
        ("Unsupported", "browser sign-in is unavailable: no endpoint"),
        ("Internal", "credential store: denied"),
    ] {
        assert_ne!(
            sign_in::failure_of(case, words),
            SignInFailure::Unknown,
            "{case}: {words}"
        );
    }
}
