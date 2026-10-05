//! Refresh across processes: the app, the daemon and the CLI share one
//! credential store. With refresh-token rotation and reuse detection, two of
//! them refreshing at once with the same token revokes the session. The
//! fake issuer here is strict that way: a refresh token works once, and
//! presenting a spent one revokes everything.
//!
//! The children are this test binary re-run (`CUA_TWO_PROCESS_CHILD`), file
//! store in a temp dir, never the OS keychain or ~/.cua.

use cua_auth::{Credentials, Oidc, Session, Store};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Issuer {
    /// Refresh tokens already spent.
    spent: Vec<String>,
    current: u32,
    refreshes: u32,
    reuse_detected: bool,
    /// Answer refreshes with this status instead (transient failures).
    fail_with: Option<u16>,
}

fn serve(st: Arc<Mutex<Issuer>>) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let b = base.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let (st, base) = (st.clone(), b.clone());
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                let body = loop {
                    let n = s.read(&mut tmp).unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_lowercase();
                        let len: usize = head
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break String::from_utf8_lossy(&buf[i + 4..i + 4 + len]).to_string();
                        }
                    }
                };
                let line = String::from_utf8_lossy(&buf)
                    .lines()
                    .next()
                    .unwrap()
                    .to_string();
                let (status, out) = if line.contains("openid-configuration") {
                    (
                        200,
                        serde_json::json!({ "token_endpoint": format!("{base}/token") }),
                    )
                } else {
                    let rt = url::form_urlencoded::parse(body.as_bytes())
                        .find(|(k, _)| k == "refresh_token")
                        .map(|(_, v)| v.into_owned())
                        .unwrap_or_default();
                    let mut g = st.lock().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    if let Some(code) = g.fail_with {
                        (
                            code,
                            serde_json::json!({ "error": "temporarily_unavailable" }),
                        )
                    } else if g.reuse_detected || g.spent.contains(&rt) {
                        g.reuse_detected = true;
                        (
                            400,
                            serde_json::json!({ "error": "invalid_grant", "error_description": "Token reuse detected" }),
                        )
                    } else {
                        g.spent.push(rt);
                        g.current += 1;
                        g.refreshes += 1;
                        let n = g.current;
                        (
                            200,
                            serde_json::json!({ "access_token": format!("at-{n}"),
                            "refresh_token": format!("rt-{n}"), "expires_in": 900,
                            "scope": "openid profile offline_access" }),
                        )
                    }
                };
                let text = out.to_string();
                let _ = write!(
                    s,
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
            });
        }
    });
    base
}

fn old_creds() -> Credentials {
    Credentials {
        access_token: "at-0".into(),
        refresh_token: Some("rt-0".into()),
        expires_at: "2000-01-01T00:00:00Z".into(),
        token_type: "Bearer".into(),
        scope: Some("openid offline_access".into()),
        id_token: None,
    }
}

/// Child role: refresh through the shared store and report.
#[test]
fn child_process() {
    let (Ok(url), Ok(path)) = (
        std::env::var("CUA_TWO_PROCESS_ISSUER"),
        std::env::var("CUA_TWO_PROCESS_STORE"),
    ) else {
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let s = Session::new(Oidc::new(url, "cua-cli"), Store::File(path.into()));
    let r = rt.block_on(s.access_token(true));
    println!(
        "CHILD_RESULT {}",
        r.map_or_else(|e| format!("ERR {e}"), |t| format!("OK {t}"))
    );
}

#[test]
fn concurrent_processes_refresh_once_and_nobody_is_signed_out() {
    let st = Arc::new(Mutex::new(Issuer::default()));
    let url = serve(st.clone());
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("credentials.json");
    Store::File(path.clone()).save(&old_creds()).unwrap();

    let exe = std::env::current_exe().unwrap();
    let kids: Vec<_> = (0..6)
        .map(|_| {
            std::process::Command::new(&exe)
                .args([
                    "child_process",
                    "--exact",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("CUA_TWO_PROCESS_ISSUER", &url)
                .env("CUA_TWO_PROCESS_STORE", &path)
                .stdout(std::process::Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    for k in kids {
        let out = k.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("CHILD_RESULT OK"), "child failed: {text}");
    }
    let g = st.lock().unwrap();
    assert!(!g.reuse_detected, "a spent refresh token was presented");
    assert_eq!(g.refreshes, 1, "exactly one process refreshes");
    let stored = Store::File(path).load().unwrap().unwrap();
    assert_eq!(stored.refresh_token.as_deref(), Some("rt-1"));
}

#[tokio::test]
async fn transient_refresh_failures_keep_the_session() {
    for code in [429u16, 500, 503] {
        let st = Arc::new(Mutex::new(Issuer {
            fail_with: Some(code),
            ..Default::default()
        }));
        let url = serve(st);
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("credentials.json");
        let store = Store::File(path.clone());
        store.save(&old_creds()).unwrap();
        let s = Session::new(Oidc::new(url, "cua-cli"), Store::File(path));
        let e = s.access_token(false).await.unwrap_err();
        assert!(
            !matches!(
                e,
                cua_auth::Error::Unauthenticated(_) | cua_auth::Error::NotLoggedIn
            ),
            "{code}: {e}"
        );
        assert_eq!(
            store.load().unwrap(),
            Some(old_creds()),
            "{code} wiped the session"
        );
    }
}

#[tokio::test]
async fn an_unreadable_store_is_not_signed_out() {
    // A directory where the credentials file should be: reads fail with an
    // I/O error (like a locked keychain), which is not "no session".
    let st = Arc::new(Mutex::new(Issuer::default()));
    let url = serve(st.clone());
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("credentials.json");
    std::fs::create_dir(&path).unwrap();
    let s = Session::new(Oidc::new(url, "cua-cli"), Store::File(path.clone()));
    let e = s.access_token(false).await.unwrap_err();
    assert!(matches!(e, cua_auth::Error::Store(_)), "{e}");
    assert!(path.is_dir());
    assert_eq!(st.lock().unwrap().refreshes, 0);
}

#[tokio::test]
async fn a_dead_grant_still_signs_out() {
    let st = Arc::new(Mutex::new(Issuer::default()));
    st.lock().unwrap().reuse_detected = true;
    let url = serve(st);
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("credentials.json");
    let store = Store::File(path.clone());
    store.save(&old_creds()).unwrap();
    let s = Session::new(Oidc::new(url, "cua-cli"), Store::File(path));
    assert!(matches!(
        s.access_token(false).await,
        Err(cua_auth::Error::Unauthenticated(_))
    ));
    assert!(store.load().unwrap().is_none());
}
