// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The standalone viewer server: many sandboxes behind one loopback port.
//!
//! - `GET /` lists the sandboxes (needs the server key: open the URL the
//!   server prints once, it sets an HttpOnly cookie).
//! - `GET /s/<id>/open` mints a viewer ticket for the sandbox and redirects
//!   to `/s/<id>/viewer/#ticket=...` (the fragment never reaches a server).
//! - `/s/<id>/viewer/...` is the same embedded page cua-spacesd serves, so
//!   its base is `/s/<id>/`.
//! - Everything else under `/s/<id>/` (gRPC-Web, `/media`, `/files`) is a
//!   transparent pipe to the sandbox's spacesd: bodies stream both ways,
//!   WebSocket upgrades are spliced, and only the transport headers of the
//!   upstream (for example a Fleet gateway bearer) are added. The viewer
//!   ticket in the request authorizes each call at the spacesd.
//!
//! Only `Host: 127.0.0.1|localhost|[::1]` is served (DNS-rebinding guard).

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use http::{HeaderMap, HeaderName, HeaderValue, StatusCode, Uri, header};
use hyper_util::client::legacy::Client;
use hyper_util::rt::{TokioExecutor, TokioIo};

/// Where one sandbox's spacesd is, and the transport headers to add.
#[derive(Debug, Clone)]
pub struct Upstream {
    /// Base URL of the spacesd origin (a trailing `/` is added).
    pub base: url::Url,
    /// Headers every proxied request gets (never the env token: the
    /// browser's viewer ticket authorizes).
    pub headers: HeaderMap,
}

/// A sandbox the index lists.
#[derive(Debug, Clone)]
pub struct SandboxEntry {
    /// Id used in `/s/<id>/` (for example `local:dev`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-line detail (provider, image, state).
    pub detail: String,
}

/// What the server needs from its host (the cua SDK, a gateway, a list).
#[async_trait::async_trait]
pub trait Sandboxes: Send + Sync + 'static {
    /// Sandboxes to list on the index page.
    async fn list(&self) -> Result<Vec<SandboxEntry>, String>;
    /// The spacesd of `id`.
    async fn upstream(&self, id: &str) -> Result<Upstream, String>;
    /// A fresh viewer fragment for `id`: `ticket=...` plus hints
    /// (`files=...`), exactly what `CreateViewerTicketResponse.viewer_path`
    /// carries after `#`.
    async fn viewer_fragment(&self, id: &str) -> Result<String, String>;
}

struct Shared {
    sandboxes: Arc<dyn Sandboxes>,
    key: String,
    client: Client<
        hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
        Body,
    >,
}

/// Name of the cookie that holds the server key.
const COOKIE: &str = "cua_viewer_key";

/// A new random server key.
pub fn random_key() -> String {
    use base64::Engine as _;
    use rand::RngCore as _;
    let mut bytes = [0u8; 24];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The standalone router. `key` guards the index and ticket minting.
pub fn router(sandboxes: Arc<dyn Sandboxes>, key: String) -> Router {
    // An explicit provider: with more than one rustls backend compiled in
    // (workspace feature unification), rustls has no process default and
    // the implicit builder panics.
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
        .expect("ring supports the default TLS versions")
        .https_or_http()
        .enable_http1()
        .build();
    let client = Client::builder(TokioExecutor::new()).build(https);
    let shared = Arc::new(Shared {
        sandboxes,
        key,
        client,
    });
    Router::new()
        .route("/", get(index))
        .route("/s/{id}/open", get(open))
        .route(
            "/s/{id}/viewer",
            get(|| async { Redirect::permanent("viewer/") }),
        )
        .route(
            "/s/{id}/viewer/",
            get(|| async { crate::serve("index.html") }),
        )
        .route(
            "/s/{id}/viewer/{*path}",
            get(|Path((_, path)): Path<(String, String)>| async move { crate::serve(&path) }),
        )
        .route("/s/{id}/{*rest}", any(proxy))
        .layer(axum::middleware::from_fn(loopback_host_only))
        .with_state(shared)
}

async fn loopback_host_only(request: Request, next: axum::middleware::Next) -> Response {
    let host = request
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let name = host.rsplit_once(':').map_or(host, |(h, _)| h);
    if matches!(name, "127.0.0.1" | "localhost" | "[::1]") {
        next.run(request).await
    } else {
        (
            StatusCode::MISDIRECTED_REQUEST,
            "this viewer server answers only on loopback",
        )
            .into_response()
    }
}

fn has_key(shared: &Shared, headers: &HeaderMap, uri: &Uri) -> (bool, bool) {
    let from_query = uri
        .query()
        .and_then(|q| q.split('&').find_map(|p| p.strip_prefix("key=")))
        .is_some_and(|k| constant_eq(k, &shared.key));
    let from_cookie = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|c| {
            c.trim()
                .strip_prefix(&format!("{COOKIE}="))
                .map(str::to_owned)
        })
        .any(|k| constant_eq(&k, &shared.key));
    (from_query, from_cookie)
}

fn constant_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn encode_segment(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

async fn index(State(shared): State<Arc<Shared>>, headers: HeaderMap, uri: Uri) -> Response {
    let (query, cookie) = has_key(&shared, &headers, &uri);
    if query {
        // Trade the key in the URL for a cookie, and drop it from the bar.
        let mut response = Redirect::to("/").into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&format!(
                "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/",
                shared.key
            ))
            .unwrap(),
        );
        return response;
    }
    if !cookie {
        return (
            StatusCode::UNAUTHORIZED,
            "open the URL `cua viewer` printed (it carries the key)",
        )
            .into_response();
    }
    let rows = match shared.sandboxes.list().await {
        Ok(list) if list.is_empty() => {
            "<p class=muted>No sandboxes. Create one with <code>cua sb create linux</code>.</p>"
                .to_owned()
        }
        Ok(list) => list
            .iter()
            .map(|s| {
                format!(
                    "<a class=row href=\"/s/{}/open\"><b>{}</b><span>{}</span></a>",
                    encode_segment(&s.id),
                    html_escape(&s.name),
                    html_escape(&s.detail)
                )
            })
            .collect(),
        Err(e) => format!(
            "<p class=muted>Could not list sandboxes: {}</p>",
            html_escape(&e)
        ),
    };
    let mut response = Html(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content=\"width=device-width\"><title>Cua viewer</title>\
         <style>body{{background:#0b0d10;color:#e8eaed;font:14px -apple-system,system-ui,sans-serif;max-width:640px;margin:40px auto;padding:0 16px}}\
         .row{{display:flex;justify-content:space-between;gap:16px;padding:12px 14px;margin:8px 0;border:1px solid #2a2e35;border-radius:10px;color:inherit;text-decoration:none}}\
         .row:hover{{background:#16191e}}.row span,.muted{{color:#9aa0a6}}code{{color:#cfe0ff}}</style>\
         <h1>Sandboxes</h1>{rows}"
    ))
    .into_response();
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("default-src 'none'; style-src 'unsafe-inline'"),
    );
    response
}

async fn open(
    State(shared): State<Arc<Shared>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    uri: Uri,
) -> Response {
    let (query, cookie) = has_key(&shared, &headers, &uri);
    if !query && !cookie {
        return (StatusCode::UNAUTHORIZED, "missing viewer server key").into_response();
    }
    match shared.sandboxes.viewer_fragment(&id).await {
        Ok(fragment) => {
            let mut response =
                Redirect::to(&format!("/s/{}/viewer/#{fragment}", encode_segment(&id)))
                    .into_response();
            response
                .headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            response
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("could not open a viewer for {id}: {e}"),
        )
            .into_response(),
    }
}

/// Hop-by-hop and host-bound headers never forwarded.
const DROP: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "cookie",
    "origin",
    "referer",
];

async fn proxy(
    State(shared): State<Arc<Shared>>,
    Path((id, rest)): Path<(String, String)>,
    mut request: Request,
) -> Response {
    let upstream = match shared.sandboxes.upstream(&id).await {
        Ok(u) => u,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("sandbox {id}: {e}")).into_response(),
    };
    let mut base = upstream.base.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    let Ok(mut target) = base.join(&rest) else {
        return (StatusCode::BAD_REQUEST, "bad path").into_response();
    };
    target.set_query(request.uri().query());
    let upgrade = request
        .headers()
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut request));

    let (parts, body) = request.into_parts();
    let mut out = http::Request::builder()
        .method(parts.method)
        .uri(target.as_str());
    let headers = out.headers_mut().expect("builder");
    for (name, value) in parts.headers.iter() {
        if DROP.contains(&name.as_str()) {
            continue;
        }
        if name == header::UPGRADE && !upgrade {
            continue;
        }
        headers.append(name.clone(), value.clone());
    }
    if upgrade {
        headers.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
    for (name, value) in upstream.headers.iter() {
        headers.insert(name.clone(), value.clone());
    }
    if let Ok(host) =
        HeaderValue::from_str(&target[url::Position::BeforeHost..url::Position::AfterPort])
    {
        headers.insert(header::HOST, host);
    }
    let Ok(out) = out.body(if upgrade { Body::empty() } else { body }) else {
        return (StatusCode::BAD_REQUEST, "bad request").into_response();
    };
    let mut response = match shared.client.request(out).await {
        Ok(r) => r,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("upstream: {e}")).into_response(),
    };
    if let (Some(client_upgrade), StatusCode::SWITCHING_PROTOCOLS) =
        (client_upgrade, response.status())
    {
        let upstream_upgrade = hyper::upgrade::on(&mut response);
        tokio::spawn(async move {
            let (Ok(a), Ok(b)) = (client_upgrade.await, upstream_upgrade.await) else {
                return;
            };
            let _ = tokio::io::copy_bidirectional(&mut TokioIo::new(a), &mut TokioIo::new(b)).await;
        });
        let (parts, _) = response.into_parts();
        return Response::from_parts(parts, Body::empty());
    }
    let (mut parts, body) = response.into_parts();
    for name in ["connection", "keep-alive", "transfer-encoding"] {
        parts.headers.remove(HeaderName::from_static(name));
    }
    Response::from_parts(parts, Body::new(body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    struct One(url::Url);

    #[async_trait::async_trait]
    impl Sandboxes for One {
        async fn list(&self) -> Result<Vec<SandboxEntry>, String> {
            Ok(vec![SandboxEntry {
                id: "local:dev".into(),
                name: "dev".into(),
                detail: "container".into(),
            }])
        }
        async fn upstream(&self, _: &str) -> Result<Upstream, String> {
            let mut headers = HeaderMap::new();
            headers.insert("x-transport", HeaderValue::from_static("gw"));
            Ok(Upstream {
                base: self.0.clone(),
                headers,
            })
        }
        async fn viewer_fragment(&self, _: &str) -> Result<String, String> {
            Ok("ticket=v1.a.b&files=%2Fhome%2Fcua".into())
        }
    }

    fn req(path: &str) -> http::Request<Body> {
        http::Request::get(path)
            .header(header::HOST, "127.0.0.1:9")
            .body(Body::empty())
            .unwrap()
    }

    #[tokio::test]
    async fn key_cookie_index_open_and_proxy() {
        // An upstream that echoes what it saw.
        let upstream = Router::new().route(
            "/{*p}",
            any(|uri: Uri, headers: HeaderMap| async move {
                format!(
                    "{} transport={} auth={} cookie={}",
                    uri,
                    headers
                        .get("x-transport")
                        .map(|v| v.to_str().unwrap())
                        .unwrap_or("-"),
                    headers
                        .get("x-cua-env-authorization")
                        .map(|v| v.to_str().unwrap())
                        .unwrap_or("-"),
                    headers.contains_key("cookie"),
                )
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
        let base = url::Url::parse(&format!("http://{addr}/prefix/")).unwrap();
        let app = router(Arc::new(One(base)), "k".into());

        assert_eq!(
            app.clone().oneshot(req("/")).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        let set = app.clone().oneshot(req("/?key=k")).await.unwrap();
        assert_eq!(set.status(), StatusCode::SEE_OTHER);
        assert!(
            set.headers()[header::SET_COOKIE]
                .to_str()
                .unwrap()
                .contains("HttpOnly")
        );
        let with_cookie = http::Request::get("/")
            .header(header::HOST, "localhost:9")
            .header(header::COOKIE, "cua_viewer_key=k")
            .body(Body::empty())
            .unwrap();
        let page = app.clone().oneshot(with_cookie).await.unwrap();
        let html = String::from_utf8(
            page.into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert!(html.contains("/s/local%3Adev/open"), "{html}");

        let open = http::Request::get("/s/local:dev/open")
            .header(header::HOST, "127.0.0.1:9")
            .header(header::COOKIE, "cua_viewer_key=k")
            .body(Body::empty())
            .unwrap();
        let open = app.clone().oneshot(open).await.unwrap();
        assert_eq!(
            open.headers()[header::LOCATION],
            "/s/local%3Adev/viewer/#ticket=v1.a.b&files=%2Fhome%2Fcua"
        );

        let page = app
            .clone()
            .oneshot(req("/s/local:dev/viewer/"))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);

        let call = http::Request::post("/s/local:dev/cua.env.v1.SystemService/Health?x=1")
            .header(header::HOST, "127.0.0.1:9")
            .header(header::COOKIE, "cua_viewer_key=k")
            .header("x-cua-env-authorization", "Bearer v1.a.b")
            .body(Body::empty())
            .unwrap();
        let echoed = app.clone().oneshot(call).await.unwrap();
        let text = String::from_utf8(
            echoed
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(
            text,
            "/prefix/cua.env.v1.SystemService/Health?x=1 transport=gw auth=Bearer v1.a.b cookie=false"
        );

        let rebinding = http::Request::get("/s/local:dev/viewer/")
            .header(header::HOST, "evil.example")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(rebinding).await.unwrap().status(),
            StatusCode::MISDIRECTED_REQUEST
        );
    }
}
