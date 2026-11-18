// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A browser sign-in nobody finishes (the tab was closed, the browser never
//! opened) ends with "the browser didn't confirm", not a wait without end.
//! Its own test binary: it sets the process-wide wait
//! (`CUA_OIDC_LOGIN_TIMEOUT_SECS`).

use cua_auth::{Flow, Method, Oidc, Session, Store};
use cua_spaces_app_core::sign_in::{self, SignInFailure};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// An issuer that accepts browser sign-in and nothing else.
async fn issuer() -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    let b = base.clone();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            let base = b.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = s.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let body = if head.starts_with("GET /.well-known/openid-configuration") {
                    format!(
                        r#"{{"authorization_endpoint":"{base}/auth","token_endpoint":"{base}/token"}}"#
                    )
                } else {
                    "{}".into()
                };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    base
}

#[tokio::test]
async fn a_browser_sign_in_nobody_finishes_is_not_confirmed() {
    // SAFETY: the only test in this binary; nothing else reads the env.
    unsafe { std::env::set_var("CUA_OIDC_LOGIN_TIMEOUT_SECS", "1") };
    let url = issuer().await;
    let d = tempfile::tempdir().unwrap();
    let s = Session::new(
        Oidc::new(&url, "cua-cli"),
        Store::File(d.path().join("credentials.json")),
    );
    let p = s.begin_login(Flow::Browser).await.unwrap();
    assert_eq!(p.method, Method::Browser);
    let e = tokio::time::timeout(Duration::from_secs(20), p.complete())
        .await
        .unwrap()
        .unwrap_err();
    assert!(matches!(e, cua_auth::Error::Timeout(_)), "{e}");
    assert_eq!(
        sign_in::failure_of("Timeout", &e.to_string()),
        SignInFailure::NotConfirmed
    );
}
