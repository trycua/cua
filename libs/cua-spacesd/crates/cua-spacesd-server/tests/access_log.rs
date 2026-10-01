// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Every authorized remote access (token, relay identity, viewer ticket,
//! /mcp) lands in the hash-chained access log; refused calls and a
//! token-less loopback server do not.

use std::time::Duration;

use axum::body::Body;
use cua_spacesd_server::access_log::{self, AccessEntry};
use cua_spacesd_server::auth::{ExternalAuthenticator, ExternalGrant};
use cua_spacesd_server::{ServerBuilder, ServerConfig, ServerContext};
use http::Request;
use tower::ServiceExt as _;

fn server(token: Option<&str>, log: bool) -> (axum::Router, ServerContext, tempfile::TempDir) {
    let data = tempfile::tempdir().unwrap();
    let config = ServerConfig {
        listen: "127.0.0.1:0".parse().unwrap(),
        data_dir: data.path().to_path_buf(),
        shutdown_grace: Duration::from_secs(1),
        access_log: log,
        ..ServerConfig::default()
    };
    let ctx = ServerContext::new(config, token.map(str::to_owned));
    (ServerBuilder::new(ctx.clone()).build().router(), ctx, data)
}

fn grpc(path: &str, bearer: Option<&str>) -> Request<Body> {
    let mut b = Request::post(path)
        .header("host", "127.0.0.1:3211")
        .header("content-type", "application/grpc-web+proto");
    if let Some(t) = bearer {
        b = b.header("authorization", format!("Bearer {t}"));
    }
    b.body(Body::from(vec![0u8, 0, 0, 0, 0])).unwrap()
}

fn entries(ctx: &ServerContext) -> Vec<AccessEntry> {
    let path = ctx.config().access_log_path();
    access_log::verify(&path).expect("chain verifies");
    std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

const CAPS: &str = "/cua.env.v1.SystemService/GetCapabilities";

#[tokio::test]
async fn token_calls_are_recorded_and_refusals_are_not() {
    let (r, ctx, _d) = server(Some("s3cret"), true);
    r.clone().oneshot(grpc(CAPS, Some("wrong"))).await.unwrap();
    assert!(entries(&ctx).is_empty(), "a refused call is not an access");
    r.clone().oneshot(grpc(CAPS, Some("s3cret"))).await.unwrap();
    // Coalesced: the same caller and service again within a minute.
    r.clone().oneshot(grpc(CAPS, Some("s3cret"))).await.unwrap();
    let e = entries(&ctx);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].body.via, "token");
    assert_eq!(e[0].body.who, "token");
    assert_eq!(e[0].body.what, "SystemService");
    let text = std::fs::read_to_string(ctx.config().access_log_path()).unwrap();
    assert!(!text.contains("s3cret"), "never the credential");

    // /mcp records through `Auth::record_headers` (the handler calls it
    // after its bearer check); a client-sent principal is only a claim.
    let mut headers = http::HeaderMap::new();
    headers.insert(
        "authorization",
        http::HeaderValue::from_static("Bearer s3cret"),
    );
    let claimed = cua_proto::env::v1::Principal {
        id: "u1".into(),
        display_name: "Mallory".into(),
        color: String::new(),
        kind: 1,
    };
    headers.insert(
        cua_proto::metadata::PRINCIPAL_BIN,
        http::HeaderValue::from_str(&cua_spacesd_server::auth::encode_principal(&claimed)).unwrap(),
    );
    ctx.auth().record_headers(&headers, "MCP");
    let e = entries(&ctx);
    let mcp = e
        .iter()
        .find(|e| e.body.what == "MCP")
        .expect("MCP recorded");
    assert_eq!(mcp.body.who, "token (claims Mallory)");
}

struct Relay;
impl ExternalAuthenticator for Relay {
    fn authenticate(&self, headers: &http::HeaderMap) -> Option<Result<ExternalGrant, String>> {
        let v = headers.get("x-test-relay")?;
        Some(Ok(ExternalGrant {
            principal: cua_proto::env::v1::Principal {
                id: "acct-123".into(),
                display_name: "ada@example.test".into(),
                color: String::new(),
                kind: 1,
            },
            view_only: v == "viewer",
            host_only: false,
            account: None,
        }))
    }
}

#[tokio::test]
async fn relay_identities_are_recorded_by_who_they_are() {
    let (r, ctx, _d) = server(Some("s3cret"), true);
    ctx.auth().set_external(std::sync::Arc::new(Relay));
    let mut req = grpc("/cua.env.v1.ProcessService/List", None);
    req.headers_mut()
        .insert("x-test-relay", http::HeaderValue::from_static("1"));
    r.clone().oneshot(req).await.unwrap();
    let e = entries(&ctx);
    assert_eq!(e.len(), 1, "{e:?}");
    assert_eq!(e[0].body.via, "relay");
    assert_eq!(e[0].body.who, "ada@example.test (acct-123)");
    assert_eq!(e[0].body.what, "ProcessService");
}

#[tokio::test]
async fn viewer_tickets_are_recorded() {
    use cua_spacesd_server::auth::{TicketScope, ViewerGrant};
    let (r, ctx, _d) = server(Some("s3cret"), true);
    let grant = ViewerGrant {
        principal_id: "viewer:bob".into(),
        display_name: "Bob".into(),
        ..ViewerGrant::default()
    };
    let (ticket, _) = ctx.auth().mint_ticket(
        TicketScope::Viewer,
        &serde_json::to_string(&grant).unwrap(),
        "",
        Duration::from_secs(60),
    );
    let method = cua_proto::metadata::VIEWER_GRPC_METHODS[0];
    let req = Request::post(method)
        .header("host", "127.0.0.1:3211")
        .header("content-type", "application/grpc-web+proto")
        .header(
            cua_proto::metadata::ENV_AUTHORIZATION,
            format!("Bearer {ticket}"),
        )
        .body(Body::from(vec![0u8, 0, 0, 0, 0]))
        .unwrap();
    r.clone().oneshot(req).await.unwrap();
    let e = entries(&ctx);
    assert!(
        e.iter()
            .any(|e| e.body.via == "viewer" && e.body.who == "Bob (viewer:bob)"),
        "viewer access recorded: {e:?}"
    );
}

#[tokio::test]
async fn off_by_default_and_silent_without_a_token() {
    let (r, ctx, _d) = server(Some("s3cret"), false);
    r.clone().oneshot(grpc(CAPS, Some("s3cret"))).await.unwrap();
    assert!(!ctx.config().access_log_path().exists());

    let (r, ctx, _d) = server(None, true);
    r.clone().oneshot(grpc(CAPS, None)).await.unwrap();
    assert!(entries(&ctx).is_empty());
}

/// A view-only share (a relay account the Space was shared with to watch)
/// reaches presence and the view-only stream, and nothing else: every other
/// call is refused with `PermissionDenied`, named, and recorded.
#[tokio::test]
async fn view_only_shares_are_refused_everything_but_watching() {
    let (r, ctx, _d) = server(Some("s3cret"), true);
    ctx.auth().set_external(std::sync::Arc::new(Relay));
    let as_viewer = |path: &str| {
        let mut req = grpc(path, None);
        req.headers_mut()
            .insert("x-test-relay", http::HeaderValue::from_static("viewer"));
        req
    };
    let refused = r
        .clone()
        .oneshot(as_viewer("/cua.env.v1.ProcessService/Start"))
        .await
        .unwrap();
    let h = refused.headers();
    assert_eq!(h.get("grpc-status").unwrap(), "7", "PermissionDenied");
    let message = percent_decode(h.get("grpc-message").unwrap().to_str().unwrap());
    assert_eq!(
        message,
        "view-only share: ada@example.test (acct-123) cannot call /cua.env.v1.ProcessService/Start"
    );
    for path in [
        "/cua.env.v1.DesktopService/Click",
        "/cua.env.v1.ComputerService/SetClipboard",
        "/cua.env.v1.FilesystemService/ReadFile",
        "/cua.env.v1.SystemService/CreateViewerTicket",
    ] {
        let resp = r.clone().oneshot(as_viewer(path)).await.unwrap();
        assert_eq!(resp.headers().get("grpc-status").unwrap(), "7", "{path}");
    }
    // Watching is allowed (the call reaches the service).
    let ok = r
        .clone()
        .oneshot(as_viewer("/cua.env.v1.SystemService/GetCapabilities"))
        .await
        .unwrap();
    assert_ne!(
        ok.headers()
            .get("grpc-status")
            .map(|v| v.to_str().unwrap().to_owned()),
        Some("7".into())
    );
    // No plain HTTP route (`/mcp`) accepts it.
    let mut headers = http::HeaderMap::new();
    headers.insert("x-test-relay", http::HeaderValue::from_static("viewer"));
    assert!(!ctx.check_bearer(&headers));
    headers.insert("x-test-relay", http::HeaderValue::from_static("editor"));
    assert!(ctx.check_bearer(&headers));
    // The refusal is in the access log, by who and what.
    let e = entries(&ctx);
    assert!(
        e.iter().any(|e| e.body.who == "ada@example.test (acct-123)"
            && e.body.what == "refused ProcessService (view-only share)"),
        "{e:?}"
    );
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap()
}
