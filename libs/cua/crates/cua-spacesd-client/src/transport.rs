//! Channel construction: native gRPC (HTTP/2, h2c or TLS) through tonic, or
//! gRPC-Web over HTTP/1.1 through `tonic-web`'s client layer and a hyper
//! HTTP/1 connector. Both are wrapped in one boxed service that adds the
//! endpoint's path prefix and the auth / principal / Fleet headers.

use crate::{
    endpoint::{Endpoint, EndpointKind},
    error::{BoxError, Error, Result},
};
use base64::Engine;
use cua_proto::{env::v1::Principal, metadata};
use http::{HeaderValue, Request, Response, Uri, header::AUTHORIZATION};
use prost::Message;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};
use tonic::body::Body;
use tower::{Service, ServiceExt, util::BoxCloneSyncService};

/// The boxed channel every generated client in this crate runs on.
///
/// Its error is the concrete [`ChannelError`] rather than a bare
/// `Box<dyn Error>`: with a trait-object error, rustc cannot prove the
/// futures of tonic calls `Send` for every lifetime (the "implementation of
/// `From` is not general enough" limitation), which makes env calls
/// impossible to `tokio::spawn` or to export through async FFI.
pub type GrpcChannel = BoxCloneSyncService<Request<Body>, Response<Body>, ChannelError>;

/// Inner channel before prefixing and header injection.
type RawChannel = BoxCloneSyncService<Request<Body>, Response<Body>, BoxError>;

/// Transport-level failure of a [`GrpcChannel`] call.
#[derive(Debug)]
pub struct ChannelError(pub BoxError);

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::error::Error for ChannelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// Header carrying the Fleet claim for gateway correlation (same as
/// `cyclops-sdk`'s `service_request`).
pub const FLEET_CLAIM_HEADER: &str = "x-cua-fleet-claim";

/// Header carrying the spacesd token when `authorization` is taken by
/// the Fleet gateway bearer.
pub const ENV_TOKEN_HEADER: &str = "x-cua-env-authorization";

/// Wire protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransportPreference {
    /// Probe native gRPC first and fall back to gRPC-Web (direct endpoints),
    /// or use gRPC-Web directly (Fleet gateway, relay).
    #[default]
    Auto,
    /// Native gRPC over HTTP/2 only.
    Native,
    /// gRPC-Web over HTTP/1.1 only.
    GrpcWeb,
}

/// The protocol a connected client actually uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Native gRPC (HTTP/2). Client streams are available.
    Native,
    /// gRPC-Web (HTTP/1.1). Client streams use unary fallbacks.
    GrpcWeb,
}

/// Extra request headers a [`BearerProvider`] adds (name, value).
pub type ExtraHeaders = Vec<(String, String)>;

/// Supplies the Fleet gateway bearer (for example `CyclopsClient::access_token`).
pub trait BearerProvider: Send + Sync {
    /// Returns the raw bearer token (without the `Bearer ` prefix).
    fn bearer(
        &self,
        force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, BoxError>> + Send + '_>>;

    /// Headers sent with the bearer on every request (for example a relay
    /// client-device session). None by default.
    fn extra_headers(
        &self,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<ExtraHeaders, BoxError>> + Send + '_>>
    {
        Box::pin(async { Ok(Vec::new()) })
    }
}

/// A static bearer.
#[derive(Clone)]
pub struct StaticBearer(pub String);

impl BearerProvider for StaticBearer {
    fn bearer(
        &self,
        _force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, BoxError>> + Send + '_>> {
        let token = self.0.clone();
        Box::pin(async move { Ok(token) })
    }
}

impl<F, Fut> BearerProvider for F
where
    F: Fn(bool) -> Fut + Send + Sync,
    Fut: Future<Output = std::result::Result<String, BoxError>> + Send + 'static,
{
    fn bearer(
        &self,
        force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<String, BoxError>> + Send + '_>> {
        Box::pin((self)(force_refresh))
    }
}

/// Fleet gateway credentials: the gateway bearer and the claim name.
#[derive(Clone)]
pub struct GatewayAuth {
    /// Bearer source (refreshing).
    pub bearer: Arc<dyn BearerProvider>,
    /// Exact claim name returned by Fleet, sent as `X-Cua-Fleet-Claim`.
    pub claim: Option<String>,
}

impl fmt::Debug for GatewayAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayAuth")
            .field("bearer", &"<provider>")
            .field("claim", &self.claim)
            .finish()
    }
}

/// Connection-level keepalive settings.
#[derive(Clone, Copy, Debug)]
pub struct Keepalive {
    /// HTTP/2 PING interval (native) / TCP keepalive (both).
    pub interval: Duration,
    /// How long to wait for a PING ack before the connection is dead.
    pub timeout: Duration,
}

impl Default for Keepalive {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(20),
            timeout: Duration::from_secs(10),
        }
    }
}

/// Everything needed to build a channel.
#[derive(Clone)]
pub(crate) struct ChannelConfig {
    pub endpoint: Endpoint,
    pub token: Option<String>,
    pub principal: Option<Principal>,
    pub gateway: Option<GatewayAuth>,
    pub connect_timeout: Duration,
    pub keepalive: Keepalive,
}

/// Decides the transport order for `pref` and `endpoint`.
pub(crate) fn transport_order(pref: TransportPreference, endpoint: &Endpoint) -> Vec<Transport> {
    match pref {
        TransportPreference::Native => vec![Transport::Native],
        TransportPreference::GrpcWeb => vec![Transport::GrpcWeb],
        TransportPreference::Auto => match endpoint.kind() {
            // The gateway's `/api/svc` proxy is HTTP/1.1 upstream.
            EndpointKind::FleetGateway { .. } => vec![Transport::GrpcWeb],
            EndpointKind::Relay { .. } | EndpointKind::Direct => {
                vec![Transport::Native, Transport::GrpcWeb]
            }
        },
    }
}

/// Install rustls' `ring` provider as the process default, once.
///
/// Dependency graphs that mix TLS stacks (e.g. `oci-client`'s reqwest pulls
/// in `aws-lc-rs` while tonic and hyper-rustls use `ring`) compile both rustls
/// backends, and rustls then refuses to choose a default on its own. Every
/// cua entry point that may build a TLS client calls this first. It is a no-op
/// when the host application already installed a provider.
pub fn ensure_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

pub(crate) fn build_channel(cfg: &ChannelConfig, transport: Transport) -> Result<GrpcChannel> {
    ensure_crypto_provider();
    let inner: RawChannel = match transport {
        Transport::Native => native_channel(cfg)?,
        Transport::GrpcWeb => grpc_web_channel(cfg)?,
    };
    let headers = HeaderInjector::new(cfg)?;
    Ok(BoxCloneSyncService::new(Prefixed {
        inner,
        prefix: cfg.endpoint.path_prefix().to_string(),
        headers: Arc::new(headers),
    }))
}

fn native_channel(cfg: &ChannelConfig) -> Result<RawChannel> {
    use tonic::transport::{Channel, ClientTlsConfig};
    let mut ep = Channel::from_shared(cfg.endpoint.origin())
        .map_err(|e| Error::InvalidEndpoint(e.to_string()))?
        .connect_timeout(cfg.connect_timeout)
        .http2_keep_alive_interval(cfg.keepalive.interval)
        .keep_alive_timeout(cfg.keepalive.timeout)
        .keep_alive_while_idle(true)
        .tcp_keepalive(Some(cfg.keepalive.interval))
        .tcp_nodelay(true)
        .http2_adaptive_window(true);
    if cfg.endpoint.is_tls() {
        ep = ep
            .tls_config(
                ClientTlsConfig::new()
                    .with_webpki_roots()
                    .domain_name(cfg.endpoint.host().to_string()),
            )
            .map_err(|e| Error::Transport(e.to_string()))?;
    }
    // Lazy: tonic reconnects transparently (with backoff) on the next call
    // after a connection drops.
    let channel = ep.connect_lazy();
    Ok(BoxCloneSyncService::new(
        channel.map_err(|e| Box::new(e) as BoxError),
    ))
}

fn grpc_web_channel(cfg: &ChannelConfig) -> Result<RawChannel> {
    use hyper_util::{
        client::legacy::{Client, connect::HttpConnector},
        rt::TokioExecutor,
    };
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    http.set_connect_timeout(Some(cfg.connect_timeout));
    http.set_keepalive(Some(cfg.keepalive.interval));
    http.set_nodelay(true);
    // Name the crypto provider explicitly: other crates in a dependency graph
    // (e.g. reqwest via oci-client) may enable a second rustls backend, and
    // rustls then refuses to pick a process default on its own.
    let https = hyper_rustls::HttpsConnectorBuilder::new()
        .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
        .map_err(|e| Error::Transport(e.to_string()))?
        .https_or_http()
        .enable_http1()
        .wrap_connector(http);
    let client: Client<_, tonic_web::GrpcWebCall<Body>> = Client::builder(TokioExecutor::new())
        .http09_responses(false)
        .pool_idle_timeout(Duration::from_secs(60))
        .build(https);
    let origin = cfg.endpoint.origin();
    let origin: Uri = origin
        .parse()
        .map_err(|e: http::uri::InvalidUri| Error::InvalidEndpoint(e.to_string()))?;
    // One gRPC-Web frame per body chunk before tonic-web decodes it: see
    // [`GrpcWebFrames`].
    let client = tower::ServiceBuilder::new()
        .map_response(|res: Response<hyper::body::Incoming>| res.map(GrpcWebFrames::new))
        .service(client);
    let svc = tower::ServiceBuilder::new()
        .layer(tonic_web::GrpcWebClientLayer::new())
        .service(client)
        .map_request(move |mut req: Request<_>| {
            let pq = req
                .uri()
                .path_and_query()
                .cloned()
                .unwrap_or_else(|| http::uri::PathAndQuery::from_static("/"));
            let mut parts = origin.clone().into_parts();
            parts.path_and_query = Some(pq);
            if let Ok(uri) = Uri::from_parts(parts) {
                *req.uri_mut() = uri;
            }
            req
        })
        .map_response(
            |res: Response<tonic_web::GrpcWebCall<GrpcWebFrames<hyper::body::Incoming>>>| {
                res.map(Body::new)
            },
        )
        .map_err(|e| Box::new(e) as BoxError);
    Ok(BoxCloneSyncService::new(svc))
}

/// Re-chunks a binary gRPC-Web response body so every chunk holds exactly one
/// frame (5-byte header + payload).
///
/// tonic-web 0.14's client decoder loses the trailers when a data frame and
/// the trailers frame arrive in the same body chunk: it returns the data,
/// parks the trailers, and at end of stream reports "missing grpc-status
/// trailer". Direct connections to cua-spacesd never hit it (hyper writes
/// each frame as its own chunk), but a buffering proxy does: the Fleet
/// gateway (nginx) coalesces a unary response into one chunk.
///
/// Frames larger than [`GrpcWebFrames::MAX_FRAME`] (never produced by the
/// driver: messages are capped at 8 MiB) are passed through unbuffered.
pub struct GrpcWebFrames<B> {
    inner: B,
    buf: bytes::BytesMut,
    trailers: Option<http::HeaderMap>,
    done: bool,
}

impl<B> GrpcWebFrames<B> {
    const HEADER: usize = 5;
    const MAX_FRAME: usize = 64 << 20;

    fn new(inner: B) -> Self {
        Self {
            inner,
            buf: bytes::BytesMut::new(),
            trailers: None,
            done: false,
        }
    }

    /// The next complete frame in `buf`, or (at end of stream / for an
    /// oversized frame) whatever is buffered.
    fn next_chunk(&mut self) -> Option<bytes::Bytes> {
        if self.buf.len() >= Self::HEADER {
            let len =
                u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
            let total = Self::HEADER + len;
            if self.buf.len() >= total {
                return Some(self.buf.split_to(total).freeze());
            }
            if total > Self::MAX_FRAME {
                return Some(self.buf.split().freeze());
            }
        }
        if self.done && !self.buf.is_empty() {
            return Some(self.buf.split().freeze());
        }
        None
    }
}

impl<B> http_body::Body for GrpcWebFrames<B>
where
    B: http_body::Body<Data = bytes::Bytes> + Unpin,
{
    type Data = bytes::Bytes;
    type Error = B::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<std::result::Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = &mut *self;
        loop {
            if let Some(chunk) = this.next_chunk() {
                return Poll::Ready(Some(Ok(http_body::Frame::data(chunk))));
            }
            if this.done {
                return Poll::Ready(
                    this.trailers
                        .take()
                        .map(|t| Ok(http_body::Frame::trailers(t))),
                );
            }
            match std::task::ready!(Pin::new(&mut this.inner).poll_frame(cx)) {
                Some(Ok(frame)) => match frame.into_data() {
                    Ok(data) => this.buf.extend_from_slice(&data),
                    Err(frame) => {
                        if let Ok(t) = frame.into_trailers() {
                            this.trailers = Some(t);
                        }
                    }
                },
                Some(Err(e)) => return Poll::Ready(Some(Err(e))),
                None => this.done = true,
            }
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done && self.buf.is_empty() && self.trailers.is_none()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::default()
    }
}

pub(crate) struct HeaderInjector {
    authorization: Option<HeaderValue>,
    env_token_header: Option<HeaderValue>,
    principal: Option<HeaderValue>,
    gateway: Option<GatewayAuth>,
    claim: Option<HeaderValue>,
}

impl HeaderInjector {
    pub(crate) fn new(cfg: &ChannelConfig) -> Result<Self> {
        let bearer = |t: &str| {
            HeaderValue::from_str(&format!("Bearer {t}"))
                .map(|mut v| {
                    v.set_sensitive(true);
                    v
                })
                .map_err(|_| Error::InvalidEndpoint("token contains invalid characters".into()))
        };
        let token = cfg.token.as_deref().filter(|t| !t.is_empty());
        let (authorization, env_token_header) = match (&cfg.gateway, token) {
            (None, Some(t)) => (Some(bearer(t)?), None),
            (Some(_), Some(t)) => (None, Some(bearer(t)?)),
            (_, None) => (None, None),
        };
        let principal = cfg
            .principal
            .as_ref()
            .map(|p| {
                let encoded =
                    base64::engine::general_purpose::STANDARD_NO_PAD.encode(p.encode_to_vec());
                HeaderValue::from_str(&encoded)
            })
            .transpose()
            .map_err(|_| Error::InvalidEndpoint("principal".into()))?;
        let claim = cfg
            .gateway
            .as_ref()
            .and_then(|g| g.claim.as_deref())
            .filter(|c| valid_claim(c))
            .map(HeaderValue::from_str)
            .transpose()
            .map_err(|_| Error::InvalidEndpoint("claim".into()))?;
        Ok(Self {
            authorization,
            env_token_header,
            principal,
            gateway: cfg.gateway.clone(),
            claim,
        })
    }

    pub(crate) async fn apply<B>(&self, req: &mut Request<B>) -> std::result::Result<(), BoxError> {
        let headers = req.headers_mut();
        if let Some(gateway) = &self.gateway {
            let token = gateway.bearer.bearer(false).await?;
            let mut value = HeaderValue::from_str(&format!("Bearer {token}"))?;
            value.set_sensitive(true);
            headers.insert(AUTHORIZATION, value);
            if let Some(claim) = &self.claim {
                headers.insert(FLEET_CLAIM_HEADER, claim.clone());
            }
            if let Some(env) = &self.env_token_header {
                headers.insert(ENV_TOKEN_HEADER, env.clone());
            }
            for (name, value) in gateway.bearer.extra_headers().await? {
                let name = http::header::HeaderName::from_bytes(name.as_bytes())?;
                let mut value = HeaderValue::from_str(&value)?;
                value.set_sensitive(true);
                headers.insert(name, value);
            }
        } else if let Some(auth) = &self.authorization {
            headers.insert(AUTHORIZATION, auth.clone());
        }
        if let Some(p) = &self.principal {
            headers.insert(metadata::PRINCIPAL_BIN, p.clone());
        }
        Ok(())
    }
}

/// Same validation `cyclops-sdk` applies before sending `X-Cua-Fleet-Claim`.
fn valid_claim(claim: &str) -> bool {
    !claim.is_empty()
        && claim.len() <= 128
        && claim
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._~-".contains(&b))
}

#[derive(Clone)]
struct Prefixed {
    inner: RawChannel,
    prefix: String,
    headers: Arc<HeaderInjector>,
}

impl Service<Request<Body>> for Prefixed {
    type Response = Response<Body>;
    type Error = ChannelError;
    type Future =
        Pin<Box<dyn Future<Output = std::result::Result<Response<Body>, ChannelError>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<std::result::Result<(), ChannelError>> {
        self.inner.poll_ready(cx).map_err(ChannelError)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        if !self.prefix.is_empty() {
            let mut parts = req.uri().clone().into_parts();
            let pq = parts
                .path_and_query
                .as_ref()
                .map(|pq| pq.as_str().to_string())
                .unwrap_or_else(|| "/".into());
            if let Ok(pq) = format!("{}{}", self.prefix, pq).parse() {
                parts.path_and_query = Some(pq);
                if let Ok(uri) = Uri::from_parts(parts) {
                    *req.uri_mut() = uri;
                }
            }
        }
        let clone = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, clone);
        let headers = Arc::clone(&self.headers);
        Box::pin(async move {
            headers.apply(&mut req).await.map_err(ChannelError)?;
            inner.oneshot(req).await.map_err(ChannelError)
        })
    }
}

#[cfg(test)]
mod extra_header_tests {
    use super::*;

    struct WithSession;

    impl BearerProvider for WithSession {
        fn bearer(
            &self,
            _force_refresh: bool,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<String, BoxError>> + Send + '_>>
        {
            Box::pin(async { Ok("account".to_string()) })
        }

        fn extra_headers(
            &self,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<ExtraHeaders, BoxError>> + Send + '_>>
        {
            Box::pin(async {
                Ok(vec![(
                    "x-cua-device-session".to_string(),
                    "cds_1".to_string(),
                )])
            })
        }
    }

    fn config(bearer: Arc<dyn BearerProvider>) -> ChannelConfig {
        ChannelConfig {
            endpoint: Endpoint::parse("https://relay.example/m/abcd1234").unwrap(),
            token: Some("env".into()),
            principal: None,
            gateway: Some(GatewayAuth {
                bearer,
                claim: None,
            }),
            connect_timeout: Duration::from_secs(1),
            keepalive: Keepalive::default(),
        }
    }

    /// A gateway bearer's extra headers (a relay device session) ride on
    /// every request next to the bearer; plain bearers add none.
    #[tokio::test]
    async fn gateway_extra_headers_are_sent_with_the_bearer() {
        let injector = HeaderInjector::new(&config(Arc::new(WithSession))).unwrap();
        let mut req = Request::new(());
        injector.apply(&mut req).await.unwrap();
        assert_eq!(req.headers()[AUTHORIZATION], "Bearer account");
        assert_eq!(req.headers()["x-cua-device-session"], "cds_1");
        assert!(req.headers()["x-cua-device-session"].is_sensitive());

        let plain = HeaderInjector::new(&config(Arc::new(StaticBearer("a".into())))).unwrap();
        let mut req = Request::new(());
        plain.apply(&mut req).await.unwrap();
        assert!(!req.headers().contains_key("x-cua-device-session"));
    }
}
