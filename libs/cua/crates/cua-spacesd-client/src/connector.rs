// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Environment routing below the existing RPC/HTTP TLS connectors. A selected
//! proxy is mandatory: a configuration or CONNECT failure never becomes direct IO.

use bytes::Bytes;
use http::{Method, Request, StatusCode, Uri};
use http_body_util::Empty;
use hyper::{
    rt::{Read, ReadBufCursor, Write},
    upgrade::Upgraded,
};
use hyper_util::{
    client::{
        legacy::connect::{Connected, Connection, HttpConnector},
        proxy::matcher::Matcher,
    },
    rt::TokioIo,
};
use std::{
    ffi::OsString,
    future::Future,
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::net::TcpStream;
use tower::Service;

#[derive(Clone)]
pub(crate) struct ProxyConnector {
    tcp: HttpConnector,
    timeout: Duration,
}

impl ProxyConnector {
    pub(crate) fn new(timeout: Duration, keepalive: Option<Duration>) -> Self {
        let mut tcp = HttpConnector::new();
        tcp.enforce_http(false);
        tcp.set_connect_timeout(Some(timeout));
        tcp.set_keepalive(keepalive);
        tcp.set_nodelay(true);
        Self { tcp, timeout }
    }
}

impl Service<Uri> for ProxyConnector {
    type Response = RoutedIo;
    type Error = io::Error;
    type Future = Pin<Box<dyn Future<Output = io::Result<RoutedIo>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, origin: Uri) -> Self::Future {
        let route = selected_proxy(&origin, |key| std::env::var_os(key));
        let mut tcp = self.tcp.clone();
        let timeout = self.timeout;
        Box::pin(async move {
            let proxy = route.map_err(io::Error::other)?;
            let connect = async {
                match proxy {
                    None => tcp
                        .call(origin)
                        .await
                        .map(RoutedIo::Direct)
                        .map_err(io::Error::other),
                    Some(proxy) => {
                        let socket = tcp
                            .call(proxy)
                            .await
                            .map_err(|_| io::Error::other("proxy TCP connection failed"))?;
                        connect_tunnel(socket, &origin).await.map(RoutedIo::Tunnel)
                    }
                }
            };
            tokio::time::timeout(timeout, connect)
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connection timed out"))?
        })
    }
}

// Keep the library's NO_PROXY grammar, while refusing configuration that its
// permissive proxy parser would otherwise discard. Uppercase presence wins;
// an empty scheme-specific setting permits ALL_PROXY, not its lowercase twin.
fn selected_proxy(
    origin: &Uri,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<Option<Uri>, &'static str> {
    let setting = |upper, lower| env(upper).or_else(|| env(lower));
    let scheme = if origin.scheme_str() == Some("https") {
        setting("HTTPS_PROXY", "https_proxy")
    } else {
        setting("HTTP_PROXY", "http_proxy")
    };
    let selected = scheme
        .filter(|v| !v.is_empty())
        .or_else(|| setting("ALL_PROXY", "all_proxy").filter(|v| !v.is_empty()));
    let Some(selected) = selected else {
        return Ok(None);
    };
    let exclusions = setting("NO_PROXY", "no_proxy").unwrap_or_default();
    let exclusions = exclusions.to_str().ok_or("NO_PROXY is not Unicode")?;
    // A non-routable placeholder asks only whether NO_PROXY excludes this URI.
    if Matcher::builder()
        .all("http://proxy.invalid")
        .no(exclusions)
        .build()
        .intercept(origin)
        .is_none()
    {
        return Ok(None);
    }
    if env("REQUEST_METHOD").is_some() {
        return Err("proxy use in a CGI environment is unsupported");
    }
    let selected = selected.to_str().ok_or("proxy URL is not Unicode")?;
    validate_proxy(selected).map(Some)
}

fn validate_proxy(value: &str) -> Result<Uri, &'static str> {
    // Validate the raw spelling before URL normalization can remove fragments,
    // credentials, dot paths or whitespace. Never include the value in errors.
    if value.contains('#') {
        return Err("proxy URL fragments are unsupported");
    }
    if value.contains('@') {
        return Err("proxy authentication is unsupported");
    }
    let authority = match value.split_once("://") {
        Some(("http", rest)) => rest,
        Some(_) => return Err("only HTTP proxies are supported"),
        None => value,
    };
    let authority = authority.strip_suffix('/').unwrap_or(authority);
    if authority.contains(['/', '?', '\\']) || authority.chars().any(char::is_whitespace) {
        return Err("invalid proxy authority or path");
    }
    let normalized = format!("http://{authority}/");
    let parsed = url::Url::parse(&normalized).map_err(|_| "invalid proxy URL")?;
    if parsed.host_str().is_none() || parsed.port() == Some(0) || authority.ends_with(':') {
        return Err("invalid proxy authority or port");
    }
    parsed.as_str().parse().map_err(|_| "invalid proxy URL")
}

async fn connect_tunnel(socket: TokioIo<TcpStream>, origin: &Uri) -> io::Result<Upgraded> {
    let host = origin
        .host()
        .ok_or_else(|| io::Error::other("missing origin host"))?;
    let port = origin
        .port_u16()
        .unwrap_or(if origin.scheme_str() == Some("https") {
            443
        } else {
            80
        });
    let target = format!("{host}:{port}");
    let request = Request::builder()
        .method(Method::CONNECT)
        .uri(&target)
        .header(http::header::HOST, &target)
        .body(Empty::<Bytes>::new())
        .map_err(|_| io::Error::other("invalid CONNECT destination"))?;
    let (mut requests, connection) = hyper::client::conn::http1::Builder::new()
        .max_headers(64)
        .max_buf_size(16 * 1024)
        .handshake(socket)
        .await
        .map_err(|_| io::Error::other("proxy HTTP handshake failed"))?;
    let response = async move {
        let response = requests
            .send_request(request)
            .await
            .map_err(|_| io::Error::other("proxy CONNECT exchange failed"))?;
        match response.status() {
            StatusCode::OK => hyper::upgrade::on(response)
                .await
                .map_err(|_| io::Error::other("proxy CONNECT upgrade failed")),
            StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
                Err(io::Error::other("proxy authentication required: HTTP 407"))
            }
            status => Err(io::Error::other(format!(
                "proxy CONNECT refused: HTTP {}",
                status.as_u16()
            ))),
        }
    };
    // Both futures belong to this call. Failure, deadline or cancellation drops
    // the socket; successful upgrade retains bytes read beyond the HTTP headers.
    let drive = async {
        connection
            .with_upgrades()
            .await
            .map_err(|_| io::Error::other("proxy CONNECT I/O failed"))
    };
    let (upgraded, ()) = tokio::try_join!(response, drive)?;
    Ok(upgraded)
}

pub(crate) enum RoutedIo {
    Direct(TokioIo<TcpStream>),
    Tunnel(Upgraded),
}

impl Connection for RoutedIo {
    fn connected(&self) -> Connected {
        match self {
            Self::Direct(io) => io.connected(),
            // CONNECT carries origin-form requests, not forward-proxy requests.
            Self::Tunnel(_) => Connected::new(),
        }
    }
}
impl Read for RoutedIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(io) => Pin::new(io).poll_read(cx, buf),
            Self::Tunnel(io) => Pin::new(io).poll_read(cx, buf),
        }
    }
}
impl Write for RoutedIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Direct(io) => Pin::new(io).poll_write(cx, buf),
            Self::Tunnel(io) => Pin::new(io).poll_write(cx, buf),
        }
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(io) => Pin::new(io).poll_flush(cx),
            Self::Tunnel(io) => Pin::new(io).poll_flush(cx),
        }
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Direct(io) => Pin::new(io).poll_shutdown(cx),
            Self::Tunnel(io) => Pin::new(io).poll_shutdown(cx),
        }
    }
    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Direct(io) => io.is_write_vectored(),
            Self::Tunnel(io) => io.is_write_vectored(),
        }
    }
    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Direct(io) => Pin::new(io).poll_write_vectored(cx, bufs),
            Self::Tunnel(io) => Pin::new(io).poll_write_vectored(cx, bufs),
        }
    }
}

#[cfg(test)]
mod tests;
