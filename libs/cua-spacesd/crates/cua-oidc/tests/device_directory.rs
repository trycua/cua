// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The HTTP device directory against a stand-in for the relay's
//! `GET /internal/v1/devices/{id}` (the relay's own tests cover the route).

use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use cua_oidc::devices::{DeviceDirectory, HttpDeviceDirectory};
use serde_json::json;

const TOKEN: &str = "relay-internal-token-0123456789abcdef";
const KNOWN: &str = "dev_0123456789abcdef01234567";

async fn relay(Path(id): Path<String>, headers: HeaderMap) -> axum::response::Response {
    if headers.get("authorization").and_then(|v| v.to_str().ok())
        != Some(&format!("Bearer {TOKEN}"))
    {
        return (StatusCode::UNAUTHORIZED, "invalid internal token").into_response();
    }
    if id == KNOWN {
        return axum::Json(json!({
            "id": KNOWN, "account": "ada", "state": "enrolled", "enrolled": true, "revoked": false,
            "public_jwk": {"kty": "EC", "crv": "P-256", "x": "AA", "y": "AA"},
        }))
        .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({"error": "device_not_found", "detail": "no such device"})),
    )
        .into_response()
}

async fn serve(router: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    base
}

#[tokio::test]
async fn reads_devices_from_the_relay() {
    let base = serve(axum::Router::new().route("/internal/v1/devices/{id}", get(relay))).await;
    let dir = HttpDeviceDirectory::new(&base, TOKEN);
    let d = dir.device(KNOWN).await.unwrap().unwrap();
    assert_eq!(d.account, "ada");
    assert!(d.enrolled && !d.revoked);
    assert_eq!(
        dir.device("dev_ffffffffffffffffffffffff").await.unwrap(),
        None
    );
    // Malformed ids never reach the relay.
    assert_eq!(dir.device("../../v1/machines").await.unwrap(), None);
    // A wrong token is an outage, not "no such device".
    let wrong = HttpDeviceDirectory::new(&base, "wrong");
    assert!(wrong.device(KNOWN).await.is_err());
}

#[tokio::test]
async fn a_relay_without_the_route_is_an_outage() {
    // The relay answers a plain 404 when its internal token is unset.
    let base = serve(axum::Router::new()).await;
    let dir = HttpDeviceDirectory::new(&base, TOKEN);
    assert!(dir.device(KNOWN).await.is_err());
}
