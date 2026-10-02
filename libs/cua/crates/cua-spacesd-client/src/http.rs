//! Raw HTTP to the spacesd's port, with the client's own credentials.
//!
//! cua-spacesd serves a few plain HTTP routes next to gRPC on the same
//! port, most importantly streamable-HTTP MCP at `/mcp` (the cua-driver tool
//! registry, including the typed-envelope extension). [`SpacesdClient::http`]
//! sends one request there through the same endpoint, path prefix and
//! authentication the gRPC channel uses: the env token as `authorization`
//! (or, through the Fleet gateway, the Fleet bearer plus `X-Cua-Fleet-Claim`
//! with the env token moved to `x-cua-env-authorization`). Callers cannot
//! set or override those headers.

use std::time::Duration;

use bytes::Bytes;
use http::{HeaderName, HeaderValue, Method, Request, header};
use http_body_util::{BodyExt, Full, Limited};
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};

use crate::{
    SpacesdClient,
    error::{Error, Result},
    transport::{ENV_TOKEN_HEADER, FLEET_CLAIM_HEADER, HeaderInjector, ensure_crypto_provider},
};

/// Default cap on a response body.
pub const DEFAULT_HTTP_RESPONSE_LIMIT: usize = 16 * 1024 * 1024;
/// Largest request body [`SpacesdClient::http`] sends.
pub const MAX_HTTP_REQUEST_BYTES: usize = 16 * 1024 * 1024;
/// Timeout when the caller sets none.
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Headers the client owns; a caller-supplied copy is rejected.
const RESERVED: &[&str] = &[
    "authorization",
    "host",
    "content-length",
    "transfer-encoding",
    "connection",
    ENV_TOKEN_HEADER,
    FLEET_CLAIM_HEADER,
    cua_proto::metadata::PRINCIPAL_BIN,
];

pub(crate) type HttpClient = Client<hyper_rustls::HttpsConnector<HttpConnector>, Full<Bytes>>;

/// One HTTP request to the spacesd.
#[derive(Clone, Debug, Default)]
pub struct HttpCall {
    /// Method, for example `POST`.
    pub method: String,
    /// Absolute path (with an optional query) under the endpoint, for example
    /// `/mcp`. Never a URL.
    pub path: String,
    /// Extra headers, in order; duplicates are kept.
    pub headers: Vec<(String, String)>,
    /// Request body.
    pub body: Vec<u8>,
    /// Whole-request timeout; [`DEFAULT_HTTP_TIMEOUT`] when unset.
    pub timeout: Option<Duration>,
    /// Response body cap; [`DEFAULT_HTTP_RESPONSE_LIMIT`] when unset.
    pub max_response_bytes: Option<usize>,
}

/// The spacesd's answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpReply {
    /// Status code.
    pub status: u16,
    /// Response headers, in order; duplicates are kept.
    pub headers: Vec<(String, String)>,
    /// Response body.
    pub body: Vec<u8>,
}

pub(crate) fn build_client(connect_timeout: Duration) -> Result<HttpClient> {
    ensure_crypto_provider();
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(connect_timeout));
    http.set_nodelay(true);
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
        .map_err(|e| Error::Transport(e.to_string()))?
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);
    Ok(Client::builder(TokioExecutor::new())
        .pool_idle_timeout(Duration::from_secs(60))
        .build(https))
}

fn validate(call: &HttpCall) -> Result<(Method, Vec<(HeaderName, HeaderValue)>)> {
    let method = Method::from_bytes(call.method.as_bytes())
        .map_err(|_| Error::Protocol("invalid HTTP method".into()))?;
    if !call.path.starts_with('/') || call.path.starts_with("//") || call.path.contains("://") {
        return Err(Error::Protocol(
            "HTTP path must be an absolute path under the endpoint".into(),
        ));
    }
    if call.body.len() > MAX_HTTP_REQUEST_BYTES {
        return Err(Error::Protocol("HTTP request body is too large".into()));
    }
    let mut headers = Vec::with_capacity(call.headers.len());
    for (name, value) in &call.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Error::Protocol("invalid HTTP header name".into()))?;
        if RESERVED.contains(&name.as_str()) {
            return Err(Error::Protocol(format!(
                "header {name} is set by the client and cannot be overridden"
            )));
        }
        let value = HeaderValue::from_str(value)
            .map_err(|_| Error::Protocol("invalid HTTP header value".into()))?;
        headers.push((name, value));
    }
    Ok((method, headers))
}

impl SpacesdClient {
    /// Sends one HTTP request to the spacesd (for example streamable-HTTP
    /// MCP at `/mcp`) with this client's endpoint and credentials.
    ///
    /// Not retried: the caller decides whether a request is idempotent. A
    /// response over the size limit fails without returning its contents.
    pub async fn http(&self, call: HttpCall) -> Result<HttpReply> {
        let (method, headers) = validate(&call)?;
        let options = &self.inner.options;
        let url = options.endpoint.http_url(&call.path);
        let mut request = Request::builder()
            .method(method)
            .uri(&url)
            .body(Full::new(Bytes::from(call.body)))
            .map_err(|e| Error::Protocol(format!("invalid HTTP request: {e}")))?;
        for (name, value) in headers {
            request.headers_mut().append(name, value);
        }
        let injector = HeaderInjector::new(&options.channel_config())?;
        injector
            .apply(&mut request)
            .await
            .map_err(|e| Error::Transport(e.to_string()))?;
        let client = self.http_client()?;
        let limit = call
            .max_response_bytes
            .unwrap_or(DEFAULT_HTTP_RESPONSE_LIMIT);
        let timeout = call.timeout.unwrap_or(DEFAULT_HTTP_TIMEOUT);
        let exchange = async {
            let response = client
                .request(request)
                .await
                .map_err(|e| Error::Transport(e.to_string()))?;
            if let Some(length) = response
                .headers()
                .get(header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<usize>().ok())
                && length > limit
            {
                return Err(Error::Protocol(
                    "HTTP response exceeds the configured size limit".into(),
                ));
            }
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(k, v)| Some((k.as_str().to_string(), v.to_str().ok()?.to_string())))
                .collect();
            let body = Limited::new(response.into_body(), limit)
                .collect()
                .await
                .map_err(|_| {
                    Error::Protocol("HTTP response exceeds the configured size limit".into())
                })?
                .to_bytes()
                .to_vec();
            Ok(HttpReply {
                status,
                headers,
                body,
            })
        };
        tokio::time::timeout(timeout, exchange)
            .await
            .map_err(|_| Error::Timeout(timeout))?
    }

    fn http_client(&self) -> Result<HttpClient> {
        if let Some(client) = self.inner.http.get() {
            return Ok(client.clone());
        }
        let client = build_client(self.inner.options.connect_timeout)?;
        Ok(self.inner.http.get_or_init(|| client).clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(path: &str, headers: &[(&str, &str)]) -> HttpCall {
        HttpCall {
            method: "POST".into(),
            path: path.into(),
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn rejects_urls_and_reserved_headers() {
        assert!(validate(&call("/mcp", &[("accept", "application/json")])).is_ok());
        for path in ["mcp", "//evil/mcp", "http://evil/mcp", "/x?u=https://e"] {
            assert!(validate(&call(path, &[])).is_err(), "{path}");
        }
        for name in [
            "Authorization",
            "x-cua-env-authorization",
            "X-Cua-Fleet-Claim",
            "x-cua-principal-bin",
            "host",
        ] {
            assert!(validate(&call("/mcp", &[(name, "x")])).is_err(), "{name}");
        }
    }

    #[test]
    fn keeps_duplicate_headers_in_order() {
        let (_, headers) = validate(&call("/mcp", &[("x-test", "a"), ("x-test", "b")])).unwrap();
        let values: Vec<_> = headers.iter().map(|(_, v)| v.to_str().unwrap()).collect();
        assert_eq!(values, ["a", "b"]);
    }
}
