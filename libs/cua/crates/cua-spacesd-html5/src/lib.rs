// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The cua-spacesd HTML5 viewer.
//!
//! One static web app (`web/`, built into `assets/`) and two ways to serve
//! it, sharing the frontend and the protocol:
//!
//! - **Embedded**: cua-spacesd mounts [`router`] at `/viewer` on its port
//!   (3211). The page reads its viewer ticket
//!   (`SystemService.CreateViewerTicket`) from the URL fragment and talks
//!   gRPC-Web, the `/media` WebSocket and `/files` on the same origin.
//! - **Standalone** (feature `standalone`): one server for many sandboxes.
//!   `/s/<id>/viewer/` serves the same page and `/s/<id>/...` is a
//!   transparent pipe to that sandbox's spacesd, adding only the transport
//!   credentials (a Fleet gateway bearer, say). The viewer ticket still
//!   authorizes every call at the spacesd.
//!
//! The page is public (it carries no secrets); tickets never appear in a
//! request line because they live in the fragment.

use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use http::{HeaderValue, StatusCode, header};

#[cfg(feature = "standalone")]
pub mod standalone;

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

/// Where the viewer is mounted on a spacesd (`cua_proto::metadata::VIEWER_PATH`).
pub const VIEWER_PATH: &str = "/viewer";

/// Content-Security-Policy of the page: same-origin code only, network to
/// the same origin plus WebSockets (the media socket, and the loopback
/// recorder of `cua skills record`).
pub const CSP: &str = "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' blob: data:; media-src 'self' blob:; worker-src 'self' blob:; connect-src 'self' ws: wss: http://127.0.0.1:* http://localhost:*; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// The bytes of an embedded file (`index.html`, `viewer.js`, ...).
pub fn asset(path: &str) -> Option<&'static [u8]> {
    ASSETS
        .binary_search_by(|(p, _)| p.cmp(&path))
        .ok()
        .map(|i| ASSETS[i].1)
}

fn mime(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "json" => "application/json",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// A response for one embedded file, with the security headers.
pub fn serve(path: &str) -> Response {
    let path = if path.is_empty() { "index.html" } else { path };
    let Some(bytes) = asset(path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let mut response = Response::new(Body::from(bytes));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(path)));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(if path == "index.html" {
            "no-cache"
        } else {
            "public, max-age=300"
        }),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        "permissions-policy",
        HeaderValue::from_static("microphone=(self), clipboard-read=(self), clipboard-write=(self), fullscreen=(self), keyboard-map=(self)"),
    );
    response
}

/// `/viewer` (redirects to `/viewer/`), `/viewer/` and `/viewer/<file>`.
/// The redirect is relative, so it survives path-prefixing proxies.
pub fn router<S: Clone + Send + Sync + 'static>() -> Router<S> {
    Router::new()
        .route(
            VIEWER_PATH,
            get(|| async { Redirect::permanent("viewer/") }),
        )
        .route(
            &format!("{VIEWER_PATH}/"),
            get(|| async { serve("index.html") }),
        )
        .route(
            &format!("{VIEWER_PATH}/{{*path}}"),
            get(|Path(path): Path<String>| async move { serve(&path) }),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt as _;

    async fn get(path: &str) -> Response {
        router::<()>()
            .oneshot(http::Request::get(path).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    #[test]
    fn assets_are_sorted_and_complete() {
        assert!(ASSETS.windows(2).all(|w| w[0].0 < w[1].0));
        for file in [
            "index.html",
            "viewer.js",
            "audioWorklet.js",
            "captureWorklet.js",
        ] {
            assert!(asset(file).is_some(), "{file} is embedded");
        }
        let html = std::str::from_utf8(asset("index.html").unwrap()).unwrap();
        assert!(html.contains("./viewer.js"), "relative script path: {html}");
    }

    #[tokio::test]
    async fn serves_the_page_with_security_headers() {
        let r = get("/viewer/").await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(r.headers()[header::CONTENT_SECURITY_POLICY], CSP);
        assert_eq!(r.headers()[header::REFERRER_POLICY], "no-referrer");
        let js = get("/viewer/viewer.js").await;
        assert_eq!(js.status(), StatusCode::OK);
        assert!(
            js.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/javascript")
        );
        assert_eq!(get("/viewer/nope.js").await.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            get("/viewer/../Cargo.toml").await.status(),
            StatusCode::NOT_FOUND
        );
        let redirect = get("/viewer").await;
        assert_eq!(redirect.status(), StatusCode::PERMANENT_REDIRECT);
        assert_eq!(redirect.headers()[header::LOCATION], "viewer/");
    }
}
