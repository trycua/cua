// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Host join transport only. TLS, WebSocket validation and the whole-connect
//! deadline remain owned by tungstenite and `client::session` respectively.
use std::ffi::OsString;

use bytes::Bytes;
use http::{Method, Request, StatusCode, Uri};
use http_body_util::Empty;
use hyper_util::{client::proxy::matcher::Matcher, rt::TokioIo};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
};
use tokio_tungstenite::{tungstenite, MaybeTlsStream, WebSocketStream};

use crate::client::SessionEnd;

pub(super) trait Io: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Io for T {}
type Stream = Box<dyn Io>;

fn lost(stage: &'static str) -> SessionEnd {
    SessionEnd::Lost(stage.into())
}

/// Never include URLs, headers, response bodies or nested dependency errors in
/// diagnostics. A dependency's Display/Debug is not a secret-redaction contract.
pub(super) async fn connect(
    request: tungstenite::handshake::client::Request,
) -> Result<
    (
        WebSocketStream<MaybeTlsStream<Stream>>,
        tungstenite::handshake::client::Response,
    ),
    SessionEnd,
> {
    let destination = destination(request.uri()).map_err(|e| SessionEnd::Refused(e.into()))?;
    let proxy = route(&destination, |key| std::env::var_os(key))
        .map_err(|e| SessionEnd::Refused(format!("proxy configuration: {e}")))?;
    let stream: Stream = match proxy {
        Some(proxy) => Box::new(proxy_stream(&proxy, &destination).await?),
        None => Box::new(
            dial(&destination)
                .await
                .map_err(|_| lost("direct dial failed"))?,
        ),
    };
    tokio_tungstenite::client_async_tls_with_config(request, stream, None, None)
        .await
        .map_err(|e| match e {
            tungstenite::Error::Http(response) => {
                SessionEnd::Refused(format!("HTTP {}", response.status()))
            }
            tungstenite::Error::Tls(_) => lost("origin TLS failed"),
            tungstenite::Error::Io(_) => lost("origin handshake I/O failed"),
            _ => lost("WebSocket upgrade failed"),
        })
}

fn destination(uri: &Uri) -> Result<Uri, &'static str> {
    let (scheme, port) = match uri.scheme_str() {
        Some("wss") => ("https", 443),
        Some("ws") => ("http", 80),
        _ => return Err("unsupported relay scheme"),
    };
    let host = uri.host().ok_or("missing relay host")?;
    if uri.authority().is_some_and(|a| a.as_str().contains('@')) {
        return Err("relay URL userinfo is unsupported");
    }
    Uri::builder()
        .scheme(scheme)
        .authority(format!("{host}:{}", uri.port_u16().unwrap_or(port)))
        .path_and_query("/")
        .build()
        .map_err(|_| "invalid relay authority")
}

/// Only the winning environment values participate. The matcher owns the
/// NO_PROXY grammar; the explicit guard prevents its permissive parser from
/// silently turning malformed/unsupported proxy configuration into direct I/O.
fn route(uri: &Uri, env: impl Fn(&str) -> Option<OsString>) -> Result<Option<Uri>, &'static str> {
    let value = |upper, lower| env(upper).or_else(|| env(lower));
    let specific = match uri.scheme_str() {
        Some("https") => value("HTTPS_PROXY", "https_proxy"),
        _ => value("HTTP_PROXY", "http_proxy"),
    };
    let Some(raw) = specific
        .filter(|v| !v.is_empty())
        .or_else(|| value("ALL_PROXY", "all_proxy").filter(|v| !v.is_empty()))
    else {
        return Ok(None);
    };
    let no = value("NO_PROXY", "no_proxy").unwrap_or_default();
    let no = no.to_str().ok_or("NO_PROXY is not Unicode")?;
    // Matcher keeps its NoProxy type private. This is solely a bypass query;
    // the placeholder is never returned as a route or used for network I/O.
    let bypass = Matcher::builder()
        .all("http://proxy.invalid")
        .no(no)
        .build();
    if bypass.intercept(uri).is_none() {
        return Ok(None);
    }
    if env("REQUEST_METHOD").is_some() {
        return Err("proxy use in a CGI environment is unsupported");
    }
    let raw = raw.to_str().ok_or("proxy URL is not Unicode")?;
    if raw.contains('#') {
        return Err("proxy URL fragments are unsupported");
    }
    let parsed: Uri = raw.parse().map_err(|_| "invalid proxy URL")?;
    if parsed.scheme_str().is_some_and(|s| s != "http") {
        return Err("only HTTP proxies are supported");
    }
    let authority = parsed.authority().ok_or("invalid proxy authority")?;
    if authority.as_str().contains('@') {
        return Err("proxy authentication is unsupported");
    }
    if authority.as_str() != authority.host() {
        let port = authority
            .as_str()
            .strip_prefix(authority.host())
            .and_then(|p| p.strip_prefix(':'))
            .and_then(|p| p.parse::<u16>().ok());
        if port.is_none_or(|p| p == 0) {
            return Err("invalid proxy port");
        }
    }
    if parsed
        .path_and_query()
        .is_some_and(|p| p.as_str() != "/" && !p.as_str().is_empty())
    {
        return Err("proxy URL paths and queries are unsupported");
    }
    let proxy = Matcher::builder()
        .all(raw)
        .build()
        .intercept(uri)
        .ok_or("invalid proxy URL")?;
    if proxy.uri().host().is_none() || proxy.uri().port().is_some_and(|p| p.as_u16() == 0) {
        return Err("invalid proxy authority");
    }
    Ok(Some(proxy.uri().clone()))
}

async fn dial(uri: &Uri) -> std::io::Result<TcpStream> {
    // URI IPv6 hosts include brackets, while lookup_host's host tuple does not.
    let host = uri
        .host()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']');
    TcpStream::connect((host, uri.port_u16().unwrap_or(80))).await
}

async fn proxy_stream(
    proxy: &Uri,
    destination: &Uri,
) -> Result<TokioIo<hyper::upgrade::Upgraded>, SessionEnd> {
    let socket = dial(proxy).await.map_err(|_| lost("proxy dial failed"))?;
    let (mut sender, connection) = hyper::client::conn::http1::Builder::new()
        .max_headers(64)
        .max_buf_size(16 * 1024)
        .handshake(TokioIo::new(socket))
        .await
        .map_err(|_| lost("proxy HTTP handshake failed"))?;
    let authority = destination
        .authority()
        .expect("validated destination")
        .as_str();
    let request = Request::builder()
        .method(Method::CONNECT)
        .uri(authority)
        .header(http::header::HOST, authority)
        .body(Empty::<Bytes>::new())
        .map_err(|_| lost("invalid CONNECT request"))?;
    let exchange = async move {
        let response = sender
            .send_request(request)
            .await
            .map_err(|_| lost("proxy CONNECT failed"))?;
        if response.status() == StatusCode::PROXY_AUTHENTICATION_REQUIRED {
            return Err(lost("proxy authentication required: HTTP 407"));
        }
        if response.status() != StatusCode::OK {
            // Hyper 1.11 does not upgrade CONNECT 204. Support the deployed 200
            // contract explicitly, without waiting on a refused response body.
            return Err(SessionEnd::Lost(format!(
                "proxy CONNECT refused: HTTP {}",
                response.status()
            )));
        }
        hyper::upgrade::on(response)
            .await
            .map_err(|_| lost("proxy CONNECT upgrade failed"))
    };
    // No detached task: failure, timeout or cancellation drops both futures and
    // their I/O. Upgraded retains Hyper's read-ahead bytes on the successful path.
    let driver = async {
        connection
            .with_upgrades()
            .await
            .map_err(|_| lost("proxy CONNECT I/O failed"))
    };
    let (stream, ()) = tokio::try_join!(exchange, driver)?;
    Ok(TokioIo::new(stream))
}

#[cfg(test)]
mod tests;
