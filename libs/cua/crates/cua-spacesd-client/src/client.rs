//! [`SpacesdClient`]: connection, transport auto-detection and raw clients.

use crate::{
    endpoint::Endpoint,
    error::{Error, Result},
    transport::{
        self, BearerProvider, ChannelConfig, GatewayAuth, GrpcChannel, Keepalive, Transport,
        TransportPreference,
    },
};
use cua_proto::env::v1::{
    self as pb, accessibility_service_client::AccessibilityServiceClient,
    computer_service_client::ComputerServiceClient, driver_service_client::DriverServiceClient,
    filesystem_service_client::FilesystemServiceClient,
    host_spaces_service_client::HostSpacesServiceClient,
    presence_service_client::PresenceServiceClient, process_service_client::ProcessServiceClient,
    stream_service_client::StreamServiceClient, system_service_client::SystemServiceClient,
    teleport_service_client::TeleportServiceClient, tunnel_service_client::TunnelServiceClient,
    volume_service_client::VolumeServiceClient, windows_service_client::WindowsServiceClient,
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::sync::RwLock;

/// Largest message the client accepts (screenshots, file chunks).
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Default chunk size for bulk transfer when the server does not advertise
/// one (1 MiB).
pub const DEFAULT_CHUNK_BYTES: usize = 1024 * 1024;

/// Exponential backoff for reconnects and idempotent retries.
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    /// Attempts including the first (1 disables retries).
    pub max_attempts: u32,
    /// First delay.
    pub initial_backoff: Duration,
    /// Delay cap.
    pub max_backoff: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 8,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(5),
        }
    }
}

impl RetryPolicy {
    /// Delay before attempt `attempt` (1-based retry count), with jitter.
    pub fn backoff(&self, attempt: u32) -> Duration {
        let exp = self
            .initial_backoff
            .saturating_mul(1u32 << attempt.saturating_sub(1).min(16));
        let capped = exp.min(self.max_backoff);
        let jitter = rand::random::<f64>() * 0.2 + 0.9;
        capped.mul_f64(jitter)
    }

    /// Runs `op` until it succeeds, fails with a non-retryable error, or the
    /// attempts are exhausted.
    pub async fn run<T, F, Fut>(&self, mut op: F) -> Result<T>
    where
        F: FnMut(u32) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut attempt = 0;
        loop {
            match op(attempt).await {
                Ok(v) => return Ok(v),
                Err(e) if e.is_retryable() && attempt + 1 < self.max_attempts => {
                    attempt += 1;
                    let delay = match &e {
                        Error::RateLimited {
                            retry_after: Some(d),
                            ..
                        } => *d,
                        _ => self.backoff(attempt),
                    };
                    tracing::debug!(attempt, ?delay, error = %e, "retrying env call");
                    tokio::time::sleep(delay).await;
                }
                Err(e) => return Err(e),
            }
        }
    }
}

/// Options for [`SpacesdClient::connect`].
#[derive(Clone)]
pub struct ConnectOptions {
    /// Endpoint (see [`Endpoint::parse`]).
    pub endpoint: Endpoint,
    /// spacesd access token.
    pub token: Option<String>,
    /// Principal sent in `x-cua-principal-bin`.
    pub principal: Option<pb::Principal>,
    /// Wire protocol preference.
    pub transport: TransportPreference,
    /// Fleet gateway credentials (Fleet endpoints only).
    pub gateway: Option<GatewayAuth>,
    /// TCP/TLS connect timeout.
    pub connect_timeout: Duration,
    /// Timeout of the `GetCapabilities` probe.
    pub probe_timeout: Duration,
    /// Connection keepalive.
    pub keepalive: Keepalive,
    /// Reconnect / retry policy.
    pub retry: RetryPolicy,
    /// Probe with `GetCapabilities` while connecting. Disabling it requires
    /// an explicit transport and skips `SpacesdNotAvailable` detection.
    pub probe: bool,
}

impl ConnectOptions {
    /// Options for `endpoint` with defaults.
    pub fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            token: None,
            principal: None,
            transport: TransportPreference::Auto,
            gateway: None,
            connect_timeout: Duration::from_secs(10),
            probe_timeout: Duration::from_secs(15),
            keepalive: Keepalive::default(),
            retry: RetryPolicy::default(),
            probe: true,
        }
    }

    /// Parses `endpoint` and returns default options.
    pub fn parse(endpoint: &str) -> Result<Self> {
        Ok(Self::new(Endpoint::parse(endpoint)?))
    }

    /// Sets the spacesd token.
    pub fn token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }

    /// Sets the principal.
    pub fn principal(mut self, principal: pb::Principal) -> Self {
        self.principal = Some(principal);
        self
    }

    /// Sets the transport preference.
    pub fn transport(mut self, transport: TransportPreference) -> Self {
        self.transport = transport;
        self
    }

    /// Routes through the Fleet gateway with this bearer and claim.
    pub fn fleet_gateway(mut self, bearer: Arc<dyn BearerProvider>, claim: Option<String>) -> Self {
        self.gateway = Some(GatewayAuth { bearer, claim });
        self
    }

    /// Sets the retry policy.
    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Sets keepalive.
    pub fn keepalive(mut self, keepalive: Keepalive) -> Self {
        self.keepalive = keepalive;
        self
    }

    /// Sets the probe timeout.
    pub fn probe_timeout(mut self, timeout: Duration) -> Self {
        self.probe_timeout = timeout;
        self
    }

    /// Enables or disables the connect-time probe.
    pub fn probe(mut self, probe: bool) -> Self {
        self.probe = probe;
        self
    }

    pub(crate) fn channel_config(&self) -> ChannelConfig {
        ChannelConfig {
            endpoint: self.endpoint.clone(),
            token: self.token.clone(),
            principal: self.principal.clone(),
            gateway: self.gateway.clone(),
            connect_timeout: self.connect_timeout,
            keepalive: self.keepalive,
        }
    }
}

impl std::fmt::Debug for ConnectOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectOptions")
            .field("endpoint", &self.endpoint)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .field("transport", &self.transport)
            .field("gateway", &self.gateway)
            .finish_non_exhaustive()
    }
}

pub(crate) struct Inner {
    pub(crate) channel: GrpcChannel,
    pub(crate) transport: Transport,
    pub(crate) options: ConnectOptions,
    pub(crate) capabilities: RwLock<Option<pb::GetCapabilitiesResponse>>,
    /// Plain-HTTP client for [`SpacesdClient::http`], built on first use.
    pub(crate) http: std::sync::OnceLock<crate::http::HttpClient>,
}

/// Async client for one cua-spacesd. Cheap to clone.
#[derive(Clone)]
pub struct SpacesdClient {
    pub(crate) inner: Arc<Inner>,
}

impl std::fmt::Debug for SpacesdClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpacesdClient")
            .field("endpoint", &self.inner.options.endpoint)
            .field("transport", &self.inner.transport)
            .finish()
    }
}

macro_rules! raw_client {
    ($(#[$m:meta])* $name:ident, $ty:ident) => {
        $(#[$m])*
        pub fn $name(&self) -> $ty<GrpcChannel> {
            $ty::new(self.inner.channel.clone())
                .max_decoding_message_size(MAX_MESSAGE_BYTES)
                .max_encoding_message_size(MAX_MESSAGE_BYTES)
        }
    };
}

impl SpacesdClient {
    /// Connects to `endpoint` (any accepted form) with default options.
    pub async fn connect_url(endpoint: &str, token: Option<String>) -> Result<Self> {
        let mut opts = ConnectOptions::parse(endpoint)?;
        opts.token = token;
        Self::connect(opts).await
    }

    /// Connects, auto-detecting the transport with a `GetCapabilities` probe
    /// (native gRPC first, then gRPC-Web). Fails with
    /// [`Error::SpacesdNotAvailable`] when no transport answers as an
    /// spacesd.
    pub async fn connect(options: ConnectOptions) -> Result<Self> {
        let order = transport::transport_order(options.transport, &options.endpoint);
        let cfg = options.channel_config();
        if !options.probe {
            let t = order[0];
            let channel = transport::build_channel(&cfg, t)?;
            return Ok(Self::from_parts(channel, t, options, None));
        }
        let mut failures = Vec::new();
        // Whether any transport got a server answer (a non-spacesd server).
        let mut answered = false;
        for t in order {
            let channel = transport::build_channel(&cfg, t)?;
            let mut probe = SystemServiceClient::new(channel.clone())
                .max_decoding_message_size(MAX_MESSAGE_BYTES);
            let result = tokio::time::timeout(
                options.probe_timeout,
                probe.get_capabilities(pb::GetCapabilitiesRequest {}),
            )
            .await;
            match result {
                Ok(Ok(resp)) => {
                    let caps = resp.into_inner();
                    tracing::debug!(transport = ?t, version = %caps.version, "spacesd probe ok");
                    return Ok(Self::from_parts(channel, t, options, Some(caps)));
                }
                // The transport works, the credentials do not: report that
                // instead of falling back.
                Ok(Err(status)) if status.code() == tonic::Code::Unauthenticated => {
                    return Err(Error::from(status));
                }
                Ok(Err(status)) => {
                    if crate::error::transport_cause(&status).is_none() {
                        answered = true;
                    }
                    failures.push(format!(
                        "{}: {}",
                        transport_label(t),
                        crate::error::status_text(&status)
                    ));
                }
                Err(_) => failures.push(format!(
                    "{}: no answer within {:?}",
                    transport_label(t),
                    options.probe_timeout
                )),
            }
        }
        // Nothing answered at all: the usual cause is a spacesd that is
        // still starting (or a port nothing listens on).
        let reason = if answered {
            failures.join("; ")
        } else {
            format!(
                "nothing answered at {} (it may still be starting; retry in a few seconds): {}",
                options.endpoint,
                failures.join("; ")
            )
        };
        Err(Error::SpacesdNotAvailable {
            endpoint: options.endpoint.to_string(),
            reason,
        })
    }

    fn from_parts(
        channel: GrpcChannel,
        transport: Transport,
        options: ConnectOptions,
        caps: Option<pb::GetCapabilitiesResponse>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                channel,
                transport,
                options,
                capabilities: RwLock::new(caps),
                http: std::sync::OnceLock::new(),
            }),
        }
    }

    /// The token this client authenticates with, if any (for in-guest
    /// consumers of the spacesd's own endpoints, such as an agent's MCP
    /// config for `/mcp`).
    pub fn token(&self) -> Option<&str> {
        self.inner.options.token.as_deref()
    }

    /// The endpoint.
    pub fn endpoint(&self) -> &Endpoint {
        &self.inner.options.endpoint
    }

    /// The negotiated transport.
    pub fn transport(&self) -> Transport {
        self.inner.transport
    }

    /// The retry policy.
    pub fn retry_policy(&self) -> RetryPolicy {
        self.inner.options.retry
    }

    /// The underlying channel, for generated clients of other packages.
    pub fn channel(&self) -> GrpcChannel {
        self.inner.channel.clone()
    }

    raw_client!(/// Raw `SystemService` client.
        system, SystemServiceClient);
    raw_client!(/// Raw `ProcessService` client.
        process, ProcessServiceClient);
    raw_client!(/// Raw `FilesystemService` client.
        filesystem, FilesystemServiceClient);
    raw_client!(/// Raw `ComputerService` client.
        computer, ComputerServiceClient);
    raw_client!(/// Raw `WindowsService` client.
        windows, WindowsServiceClient);
    raw_client!(/// Raw `AccessibilityService` client.
        accessibility, AccessibilityServiceClient);
    raw_client!(/// Raw `DriverService` client.
        driver, DriverServiceClient);
    raw_client!(/// Raw `StreamService` client.
        stream, StreamServiceClient);
    raw_client!(/// Raw `PresenceService` client.
        presence, PresenceServiceClient);
    raw_client!(/// Raw `TeleportService` client.
        teleport, TeleportServiceClient);
    raw_client!(/// Raw `TunnelService` client.
        tunnel, TunnelServiceClient);
    raw_client!(/// Raw `VolumeService` client (the guest mount of Cua Volume).
        volume, VolumeServiceClient);
    raw_client!(/// Raw `HostSpacesService` client (a host that provides Spaces).
        host_spaces, HostSpacesServiceClient);

    /// `GetCapabilities`, cached after the first successful call.
    pub async fn capabilities(&self) -> Result<pb::GetCapabilitiesResponse> {
        if let Some(c) = self.inner.capabilities.read().await.as_ref() {
            return Ok(c.clone());
        }
        self.refresh_capabilities().await
    }

    /// `GetCapabilities`, bypassing the cache.
    pub async fn refresh_capabilities(&self) -> Result<pb::GetCapabilitiesResponse> {
        let caps = self
            .retry_policy()
            .run(|_| async {
                Ok(self
                    .system()
                    .get_capabilities(pb::GetCapabilitiesRequest {})
                    .await?
                    .into_inner())
            })
            .await?;
        *self.inner.capabilities.write().await = Some(caps.clone());
        Ok(caps)
    }

    /// This guest's long-term X25519 public key for sealed delivery (S1),
    /// if it reports one (`GetCapabilitiesResponse::machine_seal_public_key`,
    /// empty from a guest image that predates this). `Ok(None)` for an
    /// empty or wrong-length field rather than an error: an older guest is
    /// an expected, common case, not a protocol failure.
    pub async fn machine_seal_public_key(&self) -> Result<Option<[u8; 32]>> {
        let caps = self.capabilities().await?;
        Ok(caps.machine_seal_public_key.as_slice().try_into().ok())
    }

    /// Whether the guest advertises `feature` as supported.
    pub async fn has_feature(&self, feature: &str) -> Result<bool> {
        Ok(self
            .capabilities()
            .await?
            .features
            .iter()
            .any(|f| f.name == feature && f.supported))
    }

    /// `SystemService.Init`.
    pub async fn init(&self, request: pb::InitRequest) -> Result<pb::InitResponse> {
        let resp = self.system().init(request).await?.into_inner();
        *self.inner.capabilities.write().await = None;
        Ok(resp)
    }

    /// `SystemService.Health`.
    pub async fn health(&self) -> Result<pb::HealthResponse> {
        self.retry_policy()
            .run(|_| async {
                Ok(self
                    .system()
                    .health(pb::HealthRequest {})
                    .await?
                    .into_inner())
            })
            .await
    }

    /// Resource usage (`SystemService.Metrics`): memory and the home
    /// filesystem, with whether each is the guest's own limit.
    pub async fn metrics(&self) -> Result<pb::MetricsResponse> {
        self.retry_policy()
            .run(|_| async {
                Ok(self
                    .system()
                    .metrics(pb::MetricsRequest::default())
                    .await?
                    .into_inner())
            })
            .await
    }

    /// Recommended and maximum chunk sizes from the capabilities limits
    /// (`(preferred, max)`; 1 MiB and 4 MiB when the guest does not say).
    pub async fn chunk_limits(&self) -> (usize, usize) {
        let limits = self
            .capabilities()
            .await
            .ok()
            .and_then(|c| c.limits)
            .unwrap_or_default();
        let max = if limits.max_chunk_bytes == 0 {
            4 * 1024 * 1024
        } else {
            limits.max_chunk_bytes as usize
        };
        let preferred = if limits.preferred_chunk_bytes == 0 {
            DEFAULT_CHUNK_BYTES
        } else {
            limits.preferred_chunk_bytes as usize
        };
        (preferred.min(max), max)
    }
}

/// A transport's name in error messages.
fn transport_label(t: Transport) -> &'static str {
    match t {
        Transport::Native => "gRPC",
        Transport::GrpcWeb => "gRPC-Web",
    }
}
