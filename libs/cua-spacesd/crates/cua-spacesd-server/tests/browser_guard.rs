// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A token-less loopback server refuses browser requests from other origins
//! and non-loopback host names (preflights included); with a token every
//! origin is allowed, since each request must then authenticate.

use std::time::Duration;

use axum::body::Body;
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use http::{Method, Request, StatusCode};
use tower::ServiceExt as _;

fn router(token: Option<&str>) -> (axum::Router, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, token.map(str::to_owned));
    (ServerBuilder::new(ctx).build().router(), data)
}

async fn send(router: &axum::Router, request: Request<Body>) -> StatusCode {
    router.clone().oneshot(request).await.unwrap().status()
}

fn health(host: &str, origin: Option<&str>) -> Request<Body> {
    let mut b = Request::get(cua_proto::metadata::HEALTH_PATH).header("host", host);
    if let Some(origin) = origin {
        b = b.header("origin", origin);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn tokenless_loopback_refuses_foreign_origins_and_hosts() {
    let (r, _d) = router(None);
    assert!(send(&r, health("127.0.0.1:3211", None)).await.is_success());
    assert!(
        send(&r, health("localhost:3211", Some("http://localhost:5173")))
            .await
            .is_success()
    );
    assert_eq!(
        send(&r, health("127.0.0.1:3211", Some("https://evil.example"))).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&r, health("127.0.0.1:3211", Some("null"))).await,
        StatusCode::FORBIDDEN
    );
    // DNS rebinding: the page's own origin, but a foreign host name.
    assert_eq!(
        send(&r, health("rebound.evil.example:3211", None)).await,
        StatusCode::FORBIDDEN
    );
    // The gRPC-Web preflight is refused too, so a page never gets as far
    // as sending the call.
    let preflight = Request::builder()
        .method(Method::OPTIONS)
        .uri("/cua.env.v1.ProcessService/Start")
        .header("host", "127.0.0.1:3211")
        .header("origin", "https://evil.example")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "content-type,x-grpc-web")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&r, preflight).await, StatusCode::FORBIDDEN);
    let call = Request::post("/cua.env.v1.SystemService/GetCapabilities")
        .header("host", "127.0.0.1:3211")
        .header("origin", "https://evil.example")
        .header("content-type", "application/grpc-web+proto")
        .body(Body::from(vec![0u8, 0, 0, 0, 0]))
        .unwrap();
    assert_eq!(send(&r, call).await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn token_servers_allow_any_origin() {
    let (r, _d) = router(Some("secret-token"));
    assert!(
        send(&r, health("relay.example", Some("https://app.example")))
            .await
            .is_success()
    );
}
