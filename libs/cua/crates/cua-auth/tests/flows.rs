//! Login, refresh and logout against a fake OIDC issuer, with a file store
//! in a temporary directory (never the OS keychain).

use cua_auth::{Credentials, Flow, Method, Oidc, Session, Store};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Default)]
struct State {
    pkce_ok: bool,
    challenge: Option<String>,
    redirect: Option<String>,
    refreshes: u32,
    revoked: bool,
    refresh_dead: bool,
    /// Before refusing a refresh, write these credentials here: another
    /// process that refreshed (and rotated the refresh token) first.
    rotate_first: Option<(std::path::PathBuf, Credentials)>,
}

fn jwt(claims: Value) -> String {
    use base64::Engine;
    let e = |v: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v);
    format!(
        "{}.{}.sig",
        e(br#"{"alg":"none"}"#),
        e(claims.to_string().as_bytes())
    )
}

fn form(body: &str, k: &str) -> Option<String> {
    url::form_urlencoded::parse(body.as_bytes())
        .find(|(a, _)| a == k)
        .map(|(_, v)| v.into_owned())
}

/// A loopback issuer (one request per connection, bounded reads).
async fn issuer(state: Arc<Mutex<State>>) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let b = base.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            let (st, base) = (state.clone(), b.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    if buf.len() > 1 << 16 {
                        return;
                    }
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
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
                let target = first.next().unwrap_or_default().to_string();
                let path = target.split('?').next().unwrap_or_default().to_string();
                let token = |n: u32| {
                    json!({"access_token": jwt(json!({"preferred_username": "tester", "email": "t@example.com", "n": n})),
                        "refresh_token": format!("rt-{n}"), "expires_in": 3600})
                };
                let (status, v) = {
                    let mut g = st.lock().unwrap();
                    match (method.as_str(), path.as_str()) {
                        ("GET", "/.well-known/openid-configuration") => (
                            200,
                            json!({
                                "authorization_endpoint": format!("{base}/auth"),
                                "token_endpoint": format!("{base}/token"),
                                "device_authorization_endpoint": format!("{base}/device"),
                                "revocation_endpoint": format!("{base}/revoke"),
                            }),
                        ),
                        ("GET", "/auth") => {
                            let u = url::Url::parse(&format!("http://x{target}")).unwrap();
                            let q: std::collections::HashMap<String, String> =
                                u.query_pairs().into_owned().collect();
                            g.challenge = q.get("code_challenge").cloned();
                            g.redirect = q.get("redirect_uri").cloned();
                            if g.pkce_ok {
                                (200, json!({}))
                            } else {
                                (400, json!({"error": "Invalid parameter: redirect_uri"}))
                            }
                        }
                        ("POST", "/device") => (
                            200,
                            json!({"device_code": "dc", "user_code": "ABCD-1234",
                            "verification_uri": "https://login.test/device", "expires_in": 60, "interval": 1}),
                        ),
                        ("POST", "/token") => match form(&body, "grant_type").as_deref() {
                            Some("authorization_code") => {
                                let v = form(&body, "code_verifier").unwrap_or_default();
                                if Some(cua_auth::pkce_challenge(&v)) != g.challenge
                                    || form(&body, "redirect_uri") != g.redirect
                                    || form(&body, "code").as_deref() != Some("code-1")
                                {
                                    (400, json!({"error": "invalid_grant"}))
                                } else {
                                    (200, token(0))
                                }
                            }
                            Some("urn:ietf:params:oauth:grant-type:device_code") => (200, token(0)),
                            Some("refresh_token") => {
                                if let Some((path, c)) = g.rotate_first.take() {
                                    Store::File(path).save(&c).unwrap();
                                    (
                                        400,
                                        json!({"error": "invalid_grant", "error_description": "Token is not active"}),
                                    )
                                } else if g.refresh_dead {
                                    (
                                        400,
                                        json!({"error": "invalid_grant", "error_description": "Token is not active"}),
                                    )
                                } else {
                                    g.refreshes += 1;
                                    (200, token(g.refreshes))
                                }
                            }
                            _ => (400, json!({"error": "unsupported_grant_type"})),
                        },
                        ("POST", "/revoke") => {
                            g.revoked = true;
                            (200, json!({}))
                        }
                        _ => (404, json!({})),
                    }
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

#[tokio::test]
async fn browser_login_with_pkce_then_refresh_and_logout() {
    let st = Arc::new(Mutex::new(State {
        pkce_ok: true,
        ..Default::default()
    }));
    let url = issuer(st.clone()).await;
    let d = tempfile::tempdir().unwrap();
    let s = session(&url, d.path());
    assert_eq!(
        s.access_token(false).await.unwrap_err(),
        cua_auth::Error::NotLoggedIn
    );

    let p = s.begin_login(Flow::Auto).await.unwrap();
    assert_eq!(p.method, Method::Browser);
    assert!(p.user_code.is_none() && p.note.is_none());
    let u = url::Url::parse(&p.url).unwrap();
    let q: std::collections::HashMap<String, String> = u.query_pairs().into_owned().collect();
    let cb = format!("{}?code=code-1&state={}", q["redirect_uri"], q["state"]);
    let browser = tokio::spawn(async move { reqwest::get(cb).await.unwrap().status().as_u16() });
    let creds = tokio::time::timeout(std::time::Duration::from_secs(20), p.complete())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(browser.await.unwrap(), 200);
    let id = s.install(creds).await.unwrap();
    assert_eq!(id.email.as_deref(), Some("t@example.com"));
    assert!(s.access_token(false).await.unwrap().contains('.'));
    assert_eq!(st.lock().unwrap().refreshes, 0);

    // Forced refresh rotates the refresh token and persists it.
    s.access_token(true).await.unwrap();
    assert_eq!(st.lock().unwrap().refreshes, 1);
    let stored = Store::File(d.path().join("credentials.json"))
        .load()
        .unwrap()
        .unwrap();
    assert_eq!(stored.refresh_token.as_deref(), Some("rt-1"));

    // Another process (same store) sees the rotated token without refreshing.
    let other = session(&url, d.path());
    assert_eq!(
        other.access_token(false).await.unwrap(),
        stored.access_token
    );
    assert_eq!(st.lock().unwrap().refreshes, 1);

    // Login wrote the non-secret session marker (the account, no tokens).
    let marker = std::fs::read_to_string(d.path().join(cua_auth::SESSION_MARKER)).unwrap();
    assert!(marker.contains("t@example.com") && !marker.contains(&stored.access_token));

    assert_eq!(s.logout().await.unwrap(), (true, true));
    assert!(st.lock().unwrap().revoked);
    assert!(
        !d.path().join(cua_auth::SESSION_MARKER).exists(),
        "logout removes the marker"
    );
    assert_eq!(
        other.access_token(false).await.unwrap_err(),
        cua_auth::Error::NotLoggedIn
    );
}

#[tokio::test]
async fn auto_falls_back_to_device_code_and_browser_only_is_an_error() {
    let st = Arc::new(Mutex::new(State::default()));
    let url = issuer(st.clone()).await;
    let d = tempfile::tempdir().unwrap();
    let s = session(&url, d.path());
    let p = s.begin_login(Flow::Auto).await.unwrap();
    assert_eq!(p.method, Method::Device);
    assert_eq!(p.user_code.as_deref(), Some("ABCD-1234"));
    assert!(
        p.note.as_deref().unwrap().contains("not enabled"),
        "{:?}",
        p.note
    );
    let c = p.complete().await.unwrap();
    s.install(c).await.unwrap();
    assert!(s.identity().unwrap().username.is_some());

    let e = s.begin_login(Flow::Browser).await.err().unwrap();
    assert!(matches!(e, cua_auth::Error::Unsupported(_)), "{e}");
    let p = s.begin_login(Flow::Device).await.unwrap();
    assert!(p.note.is_none());
}

#[tokio::test]
async fn a_dead_refresh_token_clears_the_session() {
    let st = Arc::new(Mutex::new(State {
        refresh_dead: true,
        ..Default::default()
    }));
    let url = issuer(st.clone()).await;
    let d = tempfile::tempdir().unwrap();
    let store = Store::File(d.path().join("credentials.json"));
    store
        .save(&Credentials {
            access_token: "old".into(),
            refresh_token: Some("rt".into()),
            expires_at: "2000-01-01T00:00:00Z".into(),
            token_type: "Bearer".into(),
            scope: None,
            id_token: None,
        })
        .unwrap();
    let s = session(&url, d.path());
    let e = s.access_token(false).await.unwrap_err();
    assert!(
        matches!(e, cua_auth::Error::Unauthenticated(ref m) if m.contains("not active")),
        "{e}"
    );
    assert!(store.load().unwrap().is_none());
}

#[tokio::test]
async fn a_refresh_lost_to_another_process_keeps_its_rotated_session() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("credentials.json");
    let fresh = Credentials {
        access_token: "rotated-by-another-process".into(),
        refresh_token: Some("rt-new".into()),
        expires_at: "2999-01-01T00:00:00Z".into(),
        token_type: "Bearer".into(),
        scope: None,
        id_token: None,
    };
    let st = Arc::new(Mutex::new(State {
        rotate_first: Some((path.clone(), fresh.clone())),
        ..Default::default()
    }));
    let url = issuer(st.clone()).await;
    let store = Store::File(path.clone());
    store
        .save(&Credentials {
            access_token: "old".into(),
            refresh_token: Some("rt-old".into()),
            expires_at: "2000-01-01T00:00:00Z".into(),
            token_type: "Bearer".into(),
            scope: None,
            id_token: None,
        })
        .unwrap();
    let s = session(&url, d.path());
    assert_eq!(
        s.access_token(false).await.unwrap(),
        "rotated-by-another-process"
    );
    // The other process's session survives.
    assert_eq!(store.load().unwrap(), Some(fresh));
}
