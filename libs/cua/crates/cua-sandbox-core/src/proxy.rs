//! A small loopback HTTP reverse proxy.
//!
//! It backs two things that must look the same local and in the cloud:
//!
//! - [`crate::Tunnel::forward`] on a cloud sandbox without cua-spacesd:
//!   a loopback listener whose requests go to the port's service through the
//!   Fleet gateway with the Fleet bearer and claim header attached, so any
//!   HTTP client (and WebSocket upgrades) reach the guest port with no
//!   credentials of its own;
//! - local public URLs (the cua daemon's share proxy): a per-URL token in the
//!   path, checked (with its expiry) on every request before anything is
//!   forwarded to the sandbox's loopback port.
//!
//! Bodies stream in both directions (server-sent events and chunked MCP
//! responses pass through), and `Connection: upgrade` requests are spliced
//! end to end once the upstream answers `101`.

use crate::{Error, Result};
use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header};
use http_body_util::{BodyExt, Full, combinators::BoxBody};
use hyper::body::Incoming;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::{TokioExecutor, TokioIo},
};
use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc, time::Duration};
use tokio::net::TcpListener;

/// Where one proxied request goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyRoute {
    /// Upstream URL for this request, path and query included
    /// (`https://fleet/api/svc/ns/sb-mcp/mcp?x=1`).
    pub url: String,
    /// Headers to set on the upstream request (credentials), replacing any
    /// the client sent under the same names.
    pub headers: Vec<(String, String)>,
}

/// The answer of a [`ProxyRouter`] for a request path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RouteDecision {
    /// Forward.
    Forward(ProxyRoute),
    /// Refuse with this status and message (401 for a bad token, 404 for an
    /// unknown path, 410 for an expired URL).
    Refuse(u16, String),
}

/// Boxed future of a [`ProxyRouter`].
pub type RouteFuture = Pin<Box<dyn Future<Output = RouteDecision> + Send>>;

/// Maps an incoming `path?query` to an upstream.
pub trait ProxyRouter: Send + Sync + 'static {
    /// Decides where `path_and_query` goes.
    fn route(&self, path_and_query: &str) -> RouteFuture;
}

impl<F> ProxyRouter for F
where
    F: Fn(&str) -> RouteFuture + Send + Sync + 'static,
{
    fn route(&self, path_and_query: &str) -> RouteFuture {
        self(path_and_query)
    }
}

type ProxyBody = BoxBody<Bytes, hyper::Error>;
type Https = hyper_rustls::HttpsConnector<HttpConnector>;

/// A running proxy. Dropping it (or [`HttpProxy::close`]) stops the
/// listener; connections in flight finish on their own.
#[derive(Debug)]
pub struct HttpProxy {
    local_addr: SocketAddr,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl HttpProxy {
    /// Binds `bind` (use a loopback address) and serves `router`.
    pub async fn start(bind: SocketAddr, router: Arc<dyn ProxyRouter>) -> Result<Self> {
        let listener = TcpListener::bind(bind).await?;
        let local_addr = listener.local_addr()?;
        let client = client();
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (router, client) = (router.clone(), client.clone());
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(move |req: Request<Incoming>| {
                        let (router, client) = (router.clone(), client.clone());
                        async move {
                            Ok::<_, std::convert::Infallible>(handle(req, &*router, &client).await)
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), svc)
                        .with_upgrades()
                        .await;
                });
            }
        });
        Ok(Self {
            local_addr,
            task: Some(task),
        })
    }

    /// The loopback address accepting connections.
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// `http://<local_addr>`.
    pub fn url(&self) -> String {
        format!("http://{}", self.local_addr)
    }

    /// Stops accepting connections.
    pub fn close(mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

impl Drop for HttpProxy {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

fn client() -> Client<Https, ProxyBody> {
    cua_spacesd_client::transport::ensure_crypto_provider();
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(Duration::from_secs(10)));
    // HTTP/1.1 only: upgrades (WebSocket) are an HTTP/1.1 mechanism.
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_webpki_roots()
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);
    Client::builder(TokioExecutor::new()).build(https)
}

fn is_upgrade(headers: &HeaderMap) -> bool {
    headers.contains_key(header::UPGRADE)
        && headers
            .get_all(header::CONNECTION)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .any(|t| t.trim().eq_ignore_ascii_case("upgrade"))
}

/// Copies end-to-end headers (the same rule as [`crate::http::open`]:
/// hop-by-hop headers and those a `Connection` header names are dropped);
/// keeps `connection: upgrade` + `upgrade` for an upgrade.
fn forward_headers(from: &HeaderMap, upgrade: bool) -> HeaderMap {
    let listed: Vec<String> = from
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| t != "upgrade")
        .collect();
    let mut out = HeaderMap::new();
    for (k, v) in from {
        let name = k.as_str();
        if name == "upgrade" {
            if upgrade {
                out.append(k.clone(), v.clone());
            }
            continue;
        }
        if crate::http::is_hop_by_hop(name) && name != "content-length"
            || listed.iter().any(|l| l == name)
        {
            continue;
        }
        out.append(k.clone(), v.clone());
    }
    if upgrade {
        out.insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
    }
    out
}

fn plain(status: u16, msg: String) -> Response<ProxyBody> {
    let mut r = Response::new(
        Full::new(Bytes::from(msg))
            .map_err(|never| match never {})
            .boxed(),
    );
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

async fn handle(
    mut req: Request<Incoming>,
    router: &dyn ProxyRouter,
    client: &Client<Https, ProxyBody>,
) -> Response<ProxyBody> {
    let pq = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_else(|| "/".into());
    let route = match router.route(&pq).await {
        RouteDecision::Forward(r) => r,
        RouteDecision::Refuse(status, msg) => return plain(status, msg),
    };
    let upgrade = is_upgrade(req.headers());
    let mut headers = forward_headers(req.headers(), upgrade);
    for (k, v) in &route.headers {
        match (
            HeaderName::from_bytes(k.as_bytes()),
            HeaderValue::from_str(v),
        ) {
            (Ok(k), Ok(v)) => {
                headers.insert(k, v);
            }
            _ => return plain(500, format!("bad proxy header {k}")),
        }
    }
    let client_upgrade = upgrade.then(|| hyper::upgrade::on(&mut req));
    let (parts, body) = req.into_parts();
    let mut up = Request::builder().method(parts.method).uri(&route.url);
    if let Some(h) = up.headers_mut() {
        *h = headers;
    }
    let up = match up.body(body.boxed()) {
        Ok(r) => r,
        Err(e) => return plain(502, format!("bad upstream request: {e}")),
    };
    let mut resp = match client.request(up).await {
        Ok(r) => r,
        Err(e) => return plain(502, format!("upstream unreachable: {e}")),
    };
    if resp.status() == StatusCode::SWITCHING_PROTOCOLS
        && let Some(client_upgrade) = client_upgrade
    {
        let upstream_upgrade = hyper::upgrade::on(&mut resp);
        tokio::spawn(async move {
            if let (Ok(a), Ok(b)) = (client_upgrade.await, upstream_upgrade.await) {
                let (mut a, mut b) = (TokioIo::new(a), TokioIo::new(b));
                let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
            }
        });
        let (parts, _) = resp.into_parts();
        return Response::from_parts(
            parts,
            http_body_util::Empty::new()
                .map_err(|never| match never {})
                .boxed(),
        );
    }
    let (mut parts, body) = resp.into_parts();
    parts.headers = forward_headers(&parts.headers, false);
    Response::from_parts(parts, body.boxed())
}

/// Joins an upstream base URL and a request `path?query`.
pub fn join_url(base: &str, path_and_query: &str) -> String {
    let base = base.trim_end_matches('/');
    if path_and_query.is_empty() || path_and_query == "/" {
        format!("{base}/")
    } else if path_and_query.starts_with('/') {
        format!("{base}{path_and_query}")
    } else {
        format!("{base}/{path_and_query}")
    }
}

/// A proxy to one upstream base URL with fixed extra headers (no routing).
pub async fn forward_to(base: String, headers: Vec<(String, String)>) -> Result<HttpProxy> {
    let router = move |pq: &str| -> RouteFuture {
        let r = RouteDecision::Forward(ProxyRoute {
            url: join_url(&base, pq),
            headers: headers.clone(),
        });
        Box::pin(async move { r })
    };
    HttpProxy::start(SocketAddr::from(([127, 0, 0, 1], 0)), Arc::new(router)).await
}

/// Validates that `url` is a plain-HTTP loopback URL (what a local public
/// URL may point at).
pub fn require_loopback_http(url: &str) -> Result<()> {
    let u = url::Url::parse(url).map_err(|e| Error::InvalidArgument(format!("{url}: {e}")))?;
    let loopback = match u.host() {
        Some(url::Host::Ipv4(a)) => a.is_loopback(),
        Some(url::Host::Ipv6(a)) => a.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    if u.scheme() != "http" || !loopback {
        return Err(Error::InvalidArgument(format!(
            "a local public URL must point at a loopback http:// address, not {url}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Answers each request with its method, path and `x-test`/`authorization`
    /// headers; `/ws` upgrades and echoes bytes.
    async fn upstream() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let svc = hyper::service::service_fn(|mut req: Request<Incoming>| async move {
                        if req.uri().path() == "/ws" {
                            let on = hyper::upgrade::on(&mut req);
                            tokio::spawn(async move {
                                if let Ok(u) = on.await {
                                    let mut io = TokioIo::new(u);
                                    let mut buf = [0u8; 64];
                                    // Bounded: one echo.
                                    if let Ok(n) = io.read(&mut buf).await {
                                        let _ = io.write_all(&buf[..n]).await;
                                    }
                                }
                            });
                            let mut r = Response::new(Full::new(Bytes::new()));
                            *r.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                            r.headers_mut()
                                .insert(header::UPGRADE, HeaderValue::from_static("echo"));
                            r.headers_mut()
                                .insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
                            return Ok::<_, std::convert::Infallible>(r);
                        }
                        let h = |n: &str| {
                            req.headers()
                                .get(n)
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or("-")
                                .to_string()
                        };
                        let line = format!(
                            "{} {} x-test={} auth={}",
                            req.method(),
                            req.uri(),
                            h("x-test"),
                            h("authorization")
                        );
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        let text = format!("{line} body={}", String::from_utf8_lossy(&body));
                        Ok(Response::new(Full::new(Bytes::from(text))))
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(s), svc)
                        .with_upgrades()
                        .await;
                });
            }
        });
        addr
    }

    async fn raw(addr: SocketAddr, req: &str) -> String {
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = Vec::new();
        let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
        String::from_utf8_lossy(&out).into_owned()
    }

    #[tokio::test]
    async fn forwards_path_body_and_headers_and_injects_credentials() {
        let up = upstream().await;
        let p = forward_to(
            format!("http://{up}/base"),
            vec![("authorization".into(), "Bearer fleet".into())],
        )
        .await
        .unwrap();
        let r = raw(
            p.local_addr(),
            "POST /mcp?x=1 HTTP/1.1\r\nhost: a\r\nx-test: yes\r\nauthorization: Bearer mine\r\n\
             content-length: 5\r\nconnection: close\r\n\r\nhello",
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 200"), "{r}");
        assert!(
            r.contains("POST /base/mcp?x=1 x-test=yes auth=Bearer fleet body=hello"),
            "{r}"
        );
    }

    #[tokio::test]
    async fn upgrades_are_spliced() {
        let up = upstream().await;
        let p = forward_to(format!("http://{up}"), vec![]).await.unwrap();
        let mut s = tokio::net::TcpStream::connect(p.local_addr())
            .await
            .unwrap();
        s.write_all(b"GET /ws HTTP/1.1\r\nhost: a\r\nconnection: upgrade\r\nupgrade: echo\r\n\r\n")
            .await
            .unwrap();
        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        let head = String::from_utf8_lossy(&buf[..n]).to_string();
        assert!(head.starts_with("HTTP/1.1 101"), "{head}");
        s.write_all(b"ping").await.unwrap();
        let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(&buf[..n], b"ping");
    }

    #[tokio::test]
    async fn refusals_and_unreachable_upstreams() {
        let router = |pq: &str| -> RouteFuture {
            let d = if pq.starts_with("/ok") {
                RouteDecision::Forward(ProxyRoute {
                    url: "http://127.0.0.1:1/".into(),
                    headers: vec![],
                })
            } else {
                RouteDecision::Refuse(401, "bad token".into())
            };
            Box::pin(async move { d })
        };
        let p = HttpProxy::start(SocketAddr::from(([127, 0, 0, 1], 0)), Arc::new(router))
            .await
            .unwrap();
        let r = raw(
            p.local_addr(),
            "GET /nope HTTP/1.1\r\nhost: a\r\nconnection: close\r\n\r\n",
        )
        .await;
        assert!(
            r.starts_with("HTTP/1.1 401") && r.ends_with("bad token"),
            "{r}"
        );
        let r = raw(
            p.local_addr(),
            "GET /ok HTTP/1.1\r\nhost: a\r\nconnection: close\r\n\r\n",
        )
        .await;
        assert!(r.starts_with("HTTP/1.1 502"), "{r}");
    }

    #[test]
    fn urls() {
        assert_eq!(join_url("http://h/a/", "/b?c"), "http://h/a/b?c");
        assert_eq!(join_url("http://h/a", "/"), "http://h/a/");
        assert!(require_loopback_http("http://127.0.0.1:5/").is_ok());
        assert!(require_loopback_http("http://localhost:5/").is_ok());
        assert!(require_loopback_http("https://127.0.0.1:5/").is_err());
        assert!(require_loopback_http("http://10.0.0.1:5/").is_err());
    }
}
