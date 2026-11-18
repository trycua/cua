// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The relay pool client against a stand-in relay: a relay that restarted
//! forgot this device's session, and the client signs a new one and asks
//! again (found by the chaos runs: every pool call failed until the cached
//! session ran out).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_trait::async_trait;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use cua_spaces_cluster::pool::relay::{PoolAuth, RelayPool};
use serde_json::json;

struct Auth {
    session: AtomicU32,
    resets: AtomicU32,
}

#[async_trait]
impl PoolAuth for Auth {
    async fn headers(&self) -> Result<Vec<(String, String)>, String> {
        Ok(vec![(
            "x-cua-device-session".into(),
            format!("s{}", self.session.load(Ordering::SeqCst)),
        )])
    }

    async fn reset(&self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
        self.session.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_forgotten_device_session_is_signed_again() {
    // The relay restarted: it knows only session s1.
    let app = Router::new().route(
        "/v1/pool",
        get(|h: HeaderMap| async move {
            if h.get("x-cua-device-session").and_then(|v| v.to_str().ok()) == Some("s1") {
                (StatusCode::OK, Json(json!({"epoch": 7, "account": "ada"})))
            } else {
                (
                    StatusCode::FORBIDDEN,
                    Json(json!({"code": "device_not_enrolled", "error": "do this from an enrolled device"})),
                )
            }
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await });
    let auth = Arc::new(Auth {
        session: AtomicU32::new(0),
        resets: AtomicU32::new(0),
    });
    let pool = RelayPool::new(&base, "ada", auth.clone());
    let v = pool.status().await.unwrap();
    assert_eq!(v["epoch"], 7);
    assert_eq!(auth.resets.load(Ordering::SeqCst), 1);
    // Once is enough: a refusal after a fresh session is reported.
    auth.session.store(5, Ordering::SeqCst);
    assert!(pool.status().await.is_err());
    assert_eq!(auth.resets.load(Ordering::SeqCst), 2);
}

/// A coordinator answers a request with 404 (`cua agent status session:typo`):
/// the answer is sealed, and the relay passes it on. The client must hand it
/// back, not conclude that the relay has no pool API: that sent the daemon
/// into coordinating its pool runs alone until it was restarted, so its
/// sessions were invisible to the other machines (found by the chaos runs).
#[tokio::test]
async fn a_coordinators_not_found_is_an_answer_not_a_relay_without_a_pool() {
    let app = Router::new().route(
        "/v1/pool/rpc",
        post(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"sealed": {"v": 1, "n": "AAAA", "c": "BBBB"}})),
            )
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await });
    let pool = RelayPool::new(
        &base,
        "ada",
        Arc::new(Auth {
            session: AtomicU32::new(1),
            resets: AtomicU32::new(0),
        }),
    );
    let (status, body) = pool
        .rpc("session_get", json!({"sealed": "x"}), json!({}))
        .await
        .expect("the coordinator's 404 is returned, not an error");
    assert_eq!(status, 404);
    assert!(body.get("sealed").is_some());
}

/// A relay that has no pool API answers the route with a plain 404: that is
/// "unsupported", and the daemon coordinates its pool runs itself.
#[tokio::test]
async fn a_relay_without_the_pool_api_is_unsupported() {
    let app = Router::new().route(
        "/v1/pool/rpc",
        post(|| async {
            (
                StatusCode::NOT_FOUND,
                Json(json!({"error": "not found", "code": "not_found"})),
            )
        }),
    );
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await });
    let pool = RelayPool::new(
        &base,
        "ada",
        Arc::new(Auth {
            session: AtomicU32::new(1),
            resets: AtomicU32::new(0),
        }),
    );
    let e = pool
        .rpc("session_get", json!({}), json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(
            e,
            cua_spaces_cluster::pool::relay::PoolError::Unsupported(_)
        ),
        "{e:?}"
    );
}
