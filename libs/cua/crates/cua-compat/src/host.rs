//! The host router: one listener for every `<port>-<id>.<domain>` name.
//!
//! - The first byte tells TLS (terminated with the wildcard leaf; the name
//!   comes from SNI) from plain HTTP (the name comes from `Host`).
//! - `<port>-<id>` with a guest port: the stream is piped to a forward of
//!   that port (any protocol over it, WebSockets included). When the
//!   sandbox needs a traffic token, the first request's head is checked
//!   first.
//! - `api.<domain>`, `<envd port>-<id>` and anything else: the provider's
//!   HTTP router (h1 or h2), with the sandbox id from the name attached.

use crate::e2b::HostSandbox;
use async_trait::async_trait;
use axum::Router;
use bytes::{Bytes, BytesMut};
use http::StatusCode;
use hyper_util::rt::{TokioExecutor, TokioIo};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

/// Largest request head read to route or authorize a connection.
const MAX_HEAD: usize = 64 << 10;

/// Resolves a sandbox port to a forward address.
#[async_trait]
pub trait PortResolver: Send + Sync + 'static {
    /// The header that must carry the sandbox's traffic token, and the
    /// token, when one is required.
    async fn traffic_token(&self, id: &str) -> Option<(String, String)>;
    /// The status of a request without the token (E2B: 403; Daytona: 401).
    fn unauthorized_status(&self) -> StatusCode {
        StatusCode::FORBIDDEN
    }
    /// Another credential in the request head that also grants access
    /// (Daytona: the owner's API key as `Authorization: Bearer`).
    async fn alternative_credential(&self, _id: &str, _head: &[u8]) -> bool {
        false
    }
    /// [`PortResolver::traffic_token`] for one port (a provider whose
    /// ports differ in who may connect overrides this).
    async fn traffic_token_for(&self, id: &str, _port: u16) -> Option<(String, String)> {
        self.traffic_token(id).await
    }
    /// [`PortResolver::alternative_credential`] for one port.
    async fn alternative_credential_for(&self, id: &str, _port: u16, head: &[u8]) -> bool {
        self.alternative_credential(id, head).await
    }
    /// The request head as it goes to the guest (a provider that adds or
    /// strips headers after authorizing overrides this).
    async fn rewrite_head(&self, _id: &str, _port: u16, head: Bytes) -> Bytes {
        head
    }
    /// Does `port` of sandbox `id` speak HTTP/2 over TLS (so the router
    /// offers `h2` in ALPN for its name)?
    async fn wants_h2(&self, _id: &str, _port: u16) -> bool {
        false
    }
    /// The address that reaches guest `port` of sandbox `id`.
    async fn resolve(&self, id: &str, port: u16) -> Result<String, (StatusCode, String)>;
}

/// Where a host name points.
#[derive(Debug, PartialEq, Eq)]
pub enum Target {
    /// A guest port of a sandbox.
    Port { id: String, port: u16 },
    /// The provider router, for sandbox `id` when the name carries one.
    App(Option<String>),
}

/// Classifies `host` (no port) under `domain` (no port).
pub fn classify(host: &str, domain: &str, envd_port: u16) -> Target {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let Some(label) = host
        .strip_suffix(&format!(".{}", domain.to_ascii_lowercase()))
        .filter(|l| !l.contains('.'))
    else {
        return Target::App(None);
    };
    let Some((port, id)) = label.split_once('-') else {
        return Target::App(None);
    };
    match port.parse::<u16>() {
        Ok(p) if p == envd_port => Target::App(Some(id.to_string())),
        Ok(p) if !id.is_empty() => Target::Port {
            id: id.to_string(),
            port: p,
        },
        _ => Target::App(None),
    }
}

/// The router's shared parts.
#[derive(Clone)]
pub struct HostRouter {
    pub domain: String,
    pub envd_port: u16,
    pub tls: Arc<rustls::ServerConfig>,
    pub app: Router,
    pub resolver: Arc<dyn PortResolver>,
}

impl HostRouter {
    /// Serves connections from `listener` forever.
    pub async fn serve(self, listener: TcpListener) {
        loop {
            let (sock, _) = match listener.accept().await {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!("compat: host router accept: {e}");
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    continue;
                }
            };
            let me = self.clone();
            tokio::spawn(async move {
                if let Err(e) = me.connection(sock).await {
                    tracing::debug!("compat: host connection: {e}");
                }
            });
        }
    }

    /// A connection to the API listener. The SDKs send everything for one
    /// sandbox to one base URL (`E2B_SANDBOX_URL`) and name the sandbox and
    /// the guest port in headers (`E2b-Sandbox-Id`, `E2b-Sandbox-Port`), so
    /// a request for a guest port other than envd's (the code interpreter's
    /// server on 49999, a user's service) is proxied to that port, request
    /// by request (one connection carries both kinds); the rest is the
    /// provider's API.
    pub async fn api_connection(&self, sock: TcpStream) -> io::Result<()> {
        let _ = sock.set_nodelay(true);
        self.dispatch(sock, Bytes::new(), Target::App(None)).await
    }

    /// Proxies a request that names a guest port in its headers; any other
    /// request goes on to the provider's router.
    async fn port_header_request(
        &self,
        req: http::Request<axum::body::Body>,
        next: axum::middleware::Next,
    ) -> http::Response<axum::body::Body> {
        let h = req.headers();
        let text = |n: &str| h.get(n).and_then(|v| v.to_str().ok()).map(str::to_string);
        let (Some(id), Some(port)) = (
            text("e2b-sandbox-id"),
            text("e2b-sandbox-port").and_then(|p| p.trim().parse::<u16>().ok()),
        ) else {
            return next.run(req).await;
        };
        if self.envd_port == 0 || port == self.envd_port {
            return next.run(req).await;
        }
        let edge = |status: StatusCode, msg: &str, port: Option<u16>| {
            let mut body = serde_json::Map::new();
            body.insert("sandboxId".into(), id.clone().into());
            body.insert("message".into(), msg.into());
            if let Some(p) = port {
                body.insert("port".into(), p.into());
            }
            body.insert("code".into(), status.as_u16().into());
            http::Response::builder()
                .status(status)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::Value::Object(body).to_string(),
                ))
                .unwrap()
        };
        if let Some((name, want)) = self.resolver.traffic_token(&id).await {
            let got = text(&name).unwrap_or_default();
            let mut head = String::from("GET / HTTP/1.1\r\n");
            for (k, v) in req.headers() {
                head.push_str(&format!("{k}: {}\r\n", v.to_str().unwrap_or("")));
            }
            head.push_str("\r\n");
            if !crate::auth::same(&got, &want)
                && !self
                    .resolver
                    .alternative_credential(&id, head.as_bytes())
                    .await
            {
                let message = if got.is_empty() {
                    format!(
                        "Sandbox is secured with traffic access token. Token header '{name}' is missing"
                    )
                } else {
                    format!(
                        "Sandbox is secured with traffic access token. Provided token in header '{name}' is invalid"
                    )
                };
                return edge(self.resolver.unauthorized_status(), &message, None);
            }
        }
        let addr = match self.resolver.resolve(&id, port).await {
            Ok(a) => a,
            Err((status, msg)) => return edge(status, &msg, None),
        };
        let not_open = || {
            edge(
                StatusCode::BAD_GATEWAY,
                "The sandbox is running but port is not open",
                Some(port),
            )
        };
        let Ok(upstream) = TcpStream::connect(&addr).await else {
            return not_open();
        };
        let _ = upstream.set_nodelay(true);
        let Ok((mut send, conn)) =
            hyper::client::conn::http1::handshake(TokioIo::new(upstream)).await
        else {
            return not_open();
        };
        tokio::spawn(async move {
            let _ = conn.await;
        });
        match send.send_request(req).await {
            Ok(resp) => resp.map(axum::body::Body::new),
            Err(_) => not_open(),
        }
    }

    async fn connection(&self, sock: TcpStream) -> io::Result<()> {
        let _ = sock.set_nodelay(true);
        let mut first = [0u8; 1];
        let n = sock.peek(&mut first).await?;
        if n == 0 {
            return Ok(());
        }
        if first[0] == 0x16 {
            // The ClientHello names the host: a port declared as HTTP/2
            // offers `h2` in ALPN, every other name `http/1.1` only (the
            // router pipes bytes to a guest server whose protocol it does
            // not know).
            let start =
                tokio_rustls::LazyConfigAcceptor::new(rustls::server::Acceptor::default(), sock)
                    .await?;
            let sni = start.client_hello().server_name().map(str::to_string);
            let target = sni
                .as_deref()
                .map(|h| classify(h, &self.domain, self.envd_port))
                .unwrap_or(Target::App(None));
            let mut cfg = self.tls.clone();
            if let Target::Port { id, port } = &target
                && self.resolver.wants_h2(id, *port).await
            {
                let mut c = (*self.tls).clone();
                c.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
                cfg = Arc::new(c);
            }
            let tls = start.into_stream(cfg).await?;
            self.dispatch(tls, Bytes::new(), target).await
        } else {
            let mut sock = sock;
            let head = read_head(&mut sock).await?;
            let host = header_value(&head, "host").unwrap_or_default();
            let host = host.rsplit_once(':').map_or(host.as_str(), |(h, p)| {
                if p.chars().all(|c| c.is_ascii_digit()) {
                    h
                } else {
                    host.as_str()
                }
            });
            let target = classify(host, &self.domain, self.envd_port);
            self.dispatch(sock, head, target).await
        }
    }

    async fn dispatch<S>(&self, mut io: S, head: Bytes, target: Target) -> io::Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        match target {
            Target::App(id) => {
                let app = match id {
                    Some(id) => self.app.clone().layer(axum::Extension(HostSandbox(id))),
                    None => self.app.clone(),
                };
                let me = self.clone();
                let app = app.layer(axum::middleware::from_fn(move |req, next| {
                    let me = me.clone();
                    async move { me.port_header_request(req, next).await }
                }));
                let svc = hyper_util::service::TowerToHyperService::new(app);
                hyper_util::server::conn::auto::Builder::new(TokioExecutor::new())
                    .serve_connection_with_upgrades(TokioIo::new(Rewind::new(head, io)), svc)
                    .await
                    .map_err(io::Error::other)
            }
            Target::Port { id, port } => {
                let mut head = head;
                if head.is_empty() {
                    head = read_head(&mut io).await?;
                }
                if let Some((name, want)) = self.resolver.traffic_token_for(&id, port).await {
                    let got = header_value(&head, &name).unwrap_or_default();
                    if !crate::auth::same(&got, &want)
                        && !self
                            .resolver
                            .alternative_credential_for(&id, port, &head)
                            .await
                    {
                        let message = if got.is_empty() {
                            format!(
                                "Sandbox is secured with traffic access token. Token header '{name}' is missing"
                            )
                        } else {
                            format!(
                                "Sandbox is secured with traffic access token. Provided token in header '{name}' is invalid"
                            )
                        };
                        return respond(
                            &mut io,
                            self.resolver.unauthorized_status(),
                            &id,
                            &message,
                            None,
                        )
                        .await;
                    }
                }
                if !head.is_empty() {
                    head = self.resolver.rewrite_head(&id, port, head).await;
                }
                let addr = match self.resolver.resolve(&id, port).await {
                    Ok(a) => a,
                    Err((status, msg)) => {
                        return respond(&mut io, status, &id, &msg, None).await;
                    }
                };
                let upstream = match TcpStream::connect(&addr).await {
                    Ok(s) => s,
                    Err(_) => return port_not_open(&mut io, &id, port).await,
                };
                let _ = upstream.set_nodelay(true);
                // Send the request on, and wait for the first answer bytes
                // while the rest of the client's bytes keep flowing: a
                // forward to a closed guest port accepts and then closes
                // without a byte, which is E2B's "port is not open".
                let (mut cr, mut cw) = tokio::io::split(io);
                let (mut ur, mut uw) = upstream.into_split();
                let up = tokio::spawn(async move {
                    let sent = async {
                        uw.write_all(&head).await?;
                        tokio::io::copy(&mut cr, &mut uw).await?;
                        uw.shutdown().await
                    }
                    .await;
                    let _ = sent;
                });
                let mut first = vec![0u8; 16 << 10];
                let n = ur.read(&mut first).await.unwrap_or(0);
                if n == 0 {
                    up.abort();
                    return port_not_open(&mut cw, &id, port).await;
                }
                cw.write_all(&first[..n]).await?;
                let _ = tokio::io::copy(&mut ur, &mut cw).await;
                let _ = cw.shutdown().await;
                up.abort();
                Ok(())
            }
        }
    }
}

async fn port_not_open<S: AsyncWrite + Unpin>(io: &mut S, id: &str, port: u16) -> io::Result<()> {
    respond(
        io,
        StatusCode::BAD_GATEWAY,
        id,
        "The sandbox is running but port is not open",
        Some(port),
    )
    .await
}

/// E2B's edge error body: `{"sandboxId", "message", ["port",] "code"}`.
async fn respond<S: AsyncWrite + Unpin>(
    io: &mut S,
    status: StatusCode,
    id: &str,
    msg: &str,
    port: Option<u16>,
) -> io::Result<()> {
    let mut body = serde_json::Map::new();
    body.insert("sandboxId".into(), id.into());
    body.insert("message".into(), msg.into());
    if let Some(p) = port {
        body.insert("port".into(), p.into());
    }
    body.insert("code".into(), status.as_u16().into());
    let body = serde_json::Value::Object(body).to_string();
    let resp = format!(
        "HTTP/1.1 {} {}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        body.len(),
        body
    );
    io.write_all(resp.as_bytes()).await?;
    io.shutdown().await
}

/// Reads up to the end of the first request head (`\r\n\r\n`).
async fn read_head<S: AsyncRead + Unpin>(io: &mut S) -> io::Result<Bytes> {
    let mut buf = BytesMut::with_capacity(2048);
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(buf.freeze());
        }
        if buf.len() > MAX_HEAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request head too large",
            ));
        }
        let n = io.read_buf(&mut buf).await?;
        if n == 0 {
            return Ok(buf.freeze());
        }
    }
}

/// A header's value in a raw request head (case-insensitive name).
pub fn header_value(head: &[u8], name: &str) -> Option<String> {
    let text = String::from_utf8_lossy(head);
    text.split("\r\n").skip(1).find_map(|line| {
        let (k, v) = line.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

/// A stream that first replays bytes already read from it.
pub struct Rewind<S> {
    prefix: Bytes,
    inner: S,
}

impl<S> Rewind<S> {
    pub fn new(prefix: Bytes, inner: S) -> Self {
        Self { prefix, inner }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Rewind<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = self.prefix.len().min(buf.remaining());
            let chunk = self.prefix.split_to(n);
            buf.put_slice(&chunk);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Rewind<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_names() {
        let d = "e2b.localhost";
        assert_eq!(
            classify("8080-abc123.e2b.localhost", d, 49983),
            Target::Port {
                id: "abc123".into(),
                port: 8080
            }
        );
        assert_eq!(
            classify("49983-abc123.E2B.localhost.", d, 49983),
            Target::App(Some("abc123".into()))
        );
        assert_eq!(classify("api.e2b.localhost", d, 49983), Target::App(None));
        assert_eq!(
            classify("8080-x.other.localhost", d, 49983),
            Target::App(None)
        );
        assert_eq!(
            classify("a.8080-x.e2b.localhost", d, 49983),
            Target::App(None)
        );
    }

    #[test]
    fn reads_header_values() {
        let head = b"GET / HTTP/1.1\r\nHost: 8080-x.e2b.localhost:49443\r\nE2B-Traffic-Access-Token: t\r\n\r\n";
        assert_eq!(
            header_value(head, "host").as_deref(),
            Some("8080-x.e2b.localhost:49443")
        );
        assert_eq!(
            header_value(head, "e2b-traffic-access-token").as_deref(),
            Some("t")
        );
    }
}
