//! HTTP to sandbox services: whole requests (readiness probes, small calls)
//! and a protocol-transparent streaming pipe ([`open`]) that forwards any
//! method, every end-to-end header and both bodies without buffering, so
//! SSE responses, long-lived streams and large binary bodies pass through
//! unchanged.

use crate::{Error, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, combinators::UnsyncBoxBody};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use std::time::Duration;

/// A whole HTTP response.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Headers (name, value).
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Vec<u8>,
}

impl HttpResponse {
    /// Body as lossy UTF-8.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// Error type of streamed bodies.
pub type BodyError = Box<dyn std::error::Error + Send + Sync>;
/// A streamed request body.
pub type RequestBody = UnsyncBoxBody<Bytes, BodyError>;
/// A streamed response (the body arrives as the server sends it).
pub type StreamingResponse = http::Response<hyper::body::Incoming>;

/// Where a service is reachable from this process: a base URL (requests go
/// to `<url><path>`) and the headers every request needs (the Fleet
/// gateway's bearer and claim; nothing for loopback or direct URLs).
#[derive(Clone, PartialEq, Eq)]
pub struct ServiceEndpoint {
    /// Base URL, no trailing slash.
    pub url: String,
    /// Headers to send with every request.
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for ServiceEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceEndpoint")
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| k).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ServiceEndpoint {
    /// `url + path` (path gets a leading `/`).
    pub fn url_for(&self, path: &str) -> String {
        if path.is_empty() {
            return self.url.clone();
        }
        let path = if path.starts_with('/') || path.starts_with('?') {
            path.to_string()
        } else {
            format!("/{path}")
        };
        format!("{}{}", self.url.trim_end_matches('/'), path)
    }
}

/// Hop-by-hop headers (RFC 9110 §7.6.1) a proxy never forwards, plus
/// `host` and `content-length`, which the client sets for the new request.
pub fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-connection"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "host"
            | "content-length"
    )
}

type Https = hyper_rustls::HttpsConnector<HttpConnector>;

#[derive(Clone)]
pub(crate) struct HttpClient {
    client: Client<Https, RequestBody>,
}

impl HttpClient {
    /// A client (connections pool per client; never shared across Tokio
    /// runtimes, whose pooled connections die with them).
    pub(crate) fn new() -> Self {
        cua_spacesd_client::transport::ensure_crypto_provider();
        let mut http = HttpConnector::new();
        http.enforce_http(false);
        http.set_connect_timeout(Some(Duration::from_secs(10)));
        http.set_nodelay(true);
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1()
            .wrap_connector(http);
        Self {
            client: Client::builder(TokioExecutor::new()).build(https),
        }
    }

    /// Sends a request and returns as soon as the response head arrives;
    /// the body streams. `connect_timeout` bounds only the wait for the head.
    pub(crate) async fn open(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: RequestBody,
        head_timeout: Option<Duration>,
    ) -> Result<StreamingResponse> {
        let mut req = http::Request::builder().method(method).uri(url);
        // Headers named by `Connection` are hop-by-hop too.
        let listed: Vec<String> = headers
            .iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("connection"))
            .flat_map(|(_, v)| v.split(',').map(|t| t.trim().to_ascii_lowercase()))
            .collect();
        for (k, v) in headers {
            if is_hop_by_hop(k) || listed.contains(&k.to_ascii_lowercase()) {
                continue;
            }
            req = req.header(k, v);
        }
        let req = req
            .body(body)
            .map_err(|e| Error::InvalidArgument(e.to_string()))?;
        let fut = async {
            self.client
                .request(req)
                .await
                .map_err(|e| Error::Http(format!("{method} {url}: {e}")))
        };
        match head_timeout {
            Some(t) => tokio::time::timeout(t, fut)
                .await
                .map_err(|_| Error::Timeout(format!("{method} {url}")))?,
            None => fut.await,
        }
    }

    /// A whole request and response within `timeout`.
    pub(crate) async fn request(
        &self,
        method: &str,
        url: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<HttpResponse> {
        let fut = async {
            let resp = self
                .open(method, url, headers, full(body.unwrap_or_default()), None)
                .await?;
            collect(resp).await
        };
        tokio::time::timeout(timeout, fut)
            .await
            .map_err(|_| Error::Timeout(format!("{method} {url}")))?
    }
}

/// Installs the process TLS provider (ring) once, for hyper and reqwest.
pub(crate) fn ensure_tls() {
    cua_spacesd_client::transport::ensure_crypto_provider();
}

/// A request body from bytes.
pub fn full(body: impl Into<Bytes>) -> RequestBody {
    Full::new(body.into())
        .map_err(|never| match never {})
        .boxed_unsync()
}

/// Reads a streamed response whole.
pub async fn collect(resp: StreamingResponse) -> Result<HttpResponse> {
    let status = resp.status().as_u16();
    let headers = resp
        .headers()
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or_default().to_string()))
        .collect();
    let body = resp
        .into_body()
        .collect()
        .await
        .map_err(|e| Error::Http(e.to_string()))?
        .to_bytes()
        .to_vec();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

/// Opens a streaming request to `endpoint` + `path`: the endpoint's own
/// headers, then the caller's (hop-by-hop headers dropped; a caller header
/// the endpoint sets, such as the Fleet bearer, is refused). Returns
/// once the response head arrives (bounded by `head_timeout`), with the
/// body still streaming.
pub async fn open(
    endpoint: &ServiceEndpoint,
    method: &str,
    path: &str,
    headers: &[(String, String)],
    body: RequestBody,
    head_timeout: Option<Duration>,
) -> Result<StreamingResponse> {
    let owned: Vec<String> = endpoint
        .headers
        .iter()
        .map(|(k, _)| k.to_ascii_lowercase())
        .collect();
    if let Some((k, _)) = headers
        .iter()
        .find(|(k, _)| owned.contains(&k.to_ascii_lowercase()))
    {
        return Err(Error::InvalidArgument(format!(
            "header {k:?} is set by the service route (for example the Fleet gateway)"
        )));
    }
    let mut all = endpoint.headers.clone();
    all.extend(headers.iter().cloned());
    HttpClient::new()
        .open(method, &endpoint.url_for(path), &all, body, head_timeout)
        .await
}
