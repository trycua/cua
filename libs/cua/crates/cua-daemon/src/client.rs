//! Typed client for a running `cua daemon`.

use crate::{
    Discovery, Error, Result, SandboxRecord,
    convert::{dur, info_from_pb, probe_to_pb, provider_to_pb},
    identity::version_key,
    runtime::CreateRequest,
};
use cua_proto::daemon::v1::{
    self as pb, daemon_service_client::DaemonServiceClient, pool_service_client::PoolServiceClient,
    runtime_service_client::RuntimeServiceClient, sandbox_service_client::SandboxServiceClient,
    space_service_client::SpaceServiceClient,
};
use cua_sandbox_core::{Probe, ProviderKind};
use http::HeaderValue;
use std::{path::PathBuf, time::Duration};
use tonic::transport::{Channel, Endpoint};

/// Where the daemon is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DaemonAddress {
    /// Unix socket path.
    Socket(PathBuf),
    /// Loopback URL + token.
    Url {
        /// `http://127.0.0.1:<port>`.
        url: String,
        /// Bearer token.
        token: String,
    },
}

impl DaemonAddress {
    /// Parses `address`: an `http(s)://` URL (needs `token`), a
    /// `unix:<path>` or a plain path. `None` uses the discovery file, then
    /// the default socket.
    pub fn resolve(address: Option<&str>, token: Option<String>) -> Result<Self> {
        match address.map(str::trim).filter(|a| !a.is_empty()) {
            Some(a) if a.starts_with("http://") || a.starts_with("https://") => {
                let token = token
                    .or_else(|| {
                        Discovery::read(&crate::default_discovery_path())
                            .filter(|d| d.loopback_url.as_deref() == Some(a))
                            .and_then(|d| d.token)
                    })
                    .ok_or_else(|| {
                        Error::InvalidArgument(format!("a token is required for {a}"))
                    })?;
                Ok(Self::Url {
                    url: a.trim_end_matches('/').to_string(),
                    token,
                })
            }
            Some(a) => Ok(Self::Socket(PathBuf::from(
                a.strip_prefix("unix:").unwrap_or(a),
            ))),
            None => {
                if let Some(d) = Discovery::read(&crate::default_discovery_path()) {
                    if cfg!(unix)
                        && let Some(s) = d.socket_path
                    {
                        return Ok(Self::Socket(PathBuf::from(s)));
                    }
                    if let (Some(url), Some(token)) = (d.loopback_url, d.token) {
                        return Ok(Self::Url { url, token });
                    }
                }
                Ok(Self::Socket(crate::default_socket_path()))
            }
        }
    }
}

/// Adds the loopback bearer token to every daemon call.
#[derive(Clone)]
pub struct Auth(Option<HeaderValue>);

impl tonic::service::Interceptor for Auth {
    fn call(
        &mut self,
        mut req: tonic::Request<()>,
    ) -> std::result::Result<tonic::Request<()>, tonic::Status> {
        if let Some(v) = &self.0 {
            req.metadata_mut().insert(
                "authorization",
                tonic::metadata::MetadataValue::try_from(v.as_bytes())
                    .map_err(|_| tonic::Status::internal("token"))?,
            );
        }
        Ok(req)
    }
}

/// The daemon channel.
pub type Chan = tonic::service::interceptor::InterceptedService<Channel, Auth>;

/// A client of `cua.daemon.v1`. Cheap to clone. Connects lazily.
#[derive(Clone)]
pub struct DaemonClient {
    channel: Chan,
    address: DaemonAddress,
}

impl std::fmt::Debug for DaemonClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let addr = match &self.address {
            DaemonAddress::Socket(p) => p.display().to_string(),
            DaemonAddress::Url { url, .. } => url.clone(),
        };
        f.debug_struct("DaemonClient")
            .field("address", &addr)
            .finish()
    }
}

/// Result of a service request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpResult {
    /// Status.
    pub status: u16,
    /// Headers.
    pub headers: Vec<(String, String)>,
    /// Body.
    pub body: Vec<u8>,
}

impl DaemonClient {
    /// A lazy client for `address` (no I/O until the first call).
    pub fn new(address: DaemonAddress) -> Result<Self> {
        cua_spacesd_client::transport::ensure_crypto_provider();
        let (channel, token) = match &address {
            DaemonAddress::Url { url, token } => {
                let ch = Endpoint::from_shared(url.clone())
                    .map_err(|e| Error::InvalidArgument(e.to_string()))?
                    .connect_timeout(Duration::from_secs(5))
                    .connect_lazy();
                let v = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|_| Error::InvalidArgument("token".into()))?;
                (ch, Some(v))
            }
            #[cfg(unix)]
            DaemonAddress::Socket(path) => {
                let path = path.clone();
                let ch = Endpoint::from_static("http://cua-daemon.invalid")
                    .connect_timeout(Duration::from_secs(5))
                    .connect_with_connector_lazy(tower::service_fn(move |_: http::Uri| {
                        let path = path.clone();
                        async move {
                            let s = tokio::net::UnixStream::connect(path).await?;
                            Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                        }
                    }));
                (ch, None)
            }
            #[cfg(not(unix))]
            DaemonAddress::Socket(p) => {
                return Err(Error::Unsupported(format!(
                    "Unix sockets are not available on this platform ({}); connect by URL",
                    p.display()
                )));
            }
        };
        Ok(Self {
            channel: tonic::service::interceptor::InterceptedService::new(channel, Auth(token)),
            address,
        })
    }

    /// The address.
    pub fn address(&self) -> &DaemonAddress {
        &self.address
    }

    fn sandboxes(&self) -> SandboxServiceClient<Chan> {
        SandboxServiceClient::new(self.channel.clone())
            .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
    }

    fn share_svc(&self) -> pb::share_service_client::ShareServiceClient<Chan> {
        pb::share_service_client::ShareServiceClient::new(self.channel.clone())
    }

    fn pools(&self) -> PoolServiceClient<Chan> {
        PoolServiceClient::new(self.channel.clone())
    }

    fn runtime(&self) -> RuntimeServiceClient<Chan> {
        RuntimeServiceClient::new(self.channel.clone())
            .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
    }

    fn daemon(&self) -> DaemonServiceClient<Chan> {
        DaemonServiceClient::new(self.channel.clone())
            .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
    }

    /// The raw Spaces client. Messages up to
    /// [`cua_spacesd_client::MAX_MESSAGE_BYTES`], as on the Space's own
    /// connection: a full-size screenshot (`get_desktop_state` of a 6K
    /// display) is larger than gRPC's 4 MiB default.
    pub fn spaces(&self) -> SpaceServiceClient<Chan> {
        SpaceServiceClient::new(self.channel.clone())
            .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
            .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
    }

    /// The raw `HostThreadService` client: what the host's cua-spacesd
    /// calls on this daemon's socket for the people the host is shared
    /// with (on the loopback listener, every caller is this machine's
    /// user).
    pub fn host_thread(
        &self,
    ) -> cua_proto::env::v1::host_thread_service_client::HostThreadServiceClient<Chan> {
        cua_proto::env::v1::host_thread_service_client::HostThreadServiceClient::new(
            self.channel.clone(),
        )
        .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
        .max_encoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
    }

    fn map(e: tonic::Status) -> Error {
        status_error(e)
    }

    /// `GetInfo`.
    pub async fn info(&self) -> Result<pb::GetInfoResponse> {
        Ok(self
            .daemon()
            .get_info(pb::GetInfoRequest {})
            .await
            .map_err(Self::map)?
            .into_inner())
    }

    /// `Shutdown`.
    pub async fn shutdown(&self) -> Result<()> {
        self.daemon()
            .shutdown(pb::ShutdownRequest {})
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `OpenMediaBridge`.
    pub async fn open_media_bridge(
        &self,
        name: &str,
        open_media_json: Option<String>,
    ) -> Result<pb::OpenMediaBridgeResponse> {
        Ok(self
            .daemon()
            .open_media_bridge(pb::OpenMediaBridgeRequest {
                name: name.into(),
                open_media_json: open_media_json.unwrap_or_default(),
            })
            .await
            .map_err(Self::map)?
            .into_inner())
    }

    /// `HostSpacesService.DiagnoseHost`: this machine's health as its
    /// daemon checks it, or with `machine` set, another of the owner's
    /// machines' (asked over the relay by this daemon).
    pub async fn diagnose_host(
        &self,
        machine: String,
    ) -> Result<cua_proto::env::v1::DiagnoseHostResponse> {
        use cua_proto::env::v1::host_spaces_service_client::HostSpacesServiceClient;
        Ok(HostSpacesServiceClient::new(self.channel.clone())
            .diagnose_host(cua_proto::env::v1::DiagnoseHostRequest { machine })
            .await
            .map_err(Self::map)?
            .into_inner())
    }

    /// `Doctor`.
    pub async fn doctor(&self) -> Result<pb::DoctorResponse> {
        Ok(self
            .runtime()
            .doctor(pb::DoctorRequest {})
            .await
            .map_err(Self::map)?
            .into_inner())
    }

    /// `PullImage`.
    pub async fn pull_image(&self, reference: &str) -> Result<pb::LocalImage> {
        self.runtime()
            .pull_image(pb::PullImageRequest {
                reference: reference.into(),
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .image
            .ok_or_else(|| Error::Internal("no image returned".into()))
    }

    /// `BuildImage`.
    pub async fn build_image(
        &self,
        spec_json: &str,
        base: &str,
        push: Option<String>,
    ) -> Result<String> {
        Ok(self
            .runtime()
            .build_image(pb::BuildImageRequest {
                spec_json: spec_json.into(),
                base: base.into(),
                push: push.unwrap_or_default(),
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .result)
    }

    /// `PushImage`.
    pub async fn push_image(&self, reference: &str, destination: &str) -> Result<String> {
        Ok(self
            .runtime()
            .push_image(pb::PushImageRequest {
                reference: reference.into(),
                destination: destination.into(),
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .digest)
    }

    /// `Setup`.
    pub async fn setup(
        &self,
        components: Vec<String>,
        dry_run: bool,
    ) -> Result<Vec<pb::SetupStep>> {
        Ok(self
            .runtime()
            .setup(pb::SetupRequest {
                components,
                dry_run,
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .steps)
    }

    fn sandbox_of(s: Option<pb::Sandbox>) -> Result<SandboxRecord> {
        s.as_ref()
            .map(info_from_pb)
            .ok_or_else(|| Error::Internal("daemon returned no sandbox".into()))
    }

    /// `CreateSandbox`.
    pub async fn create(&self, r: CreateRequest) -> Result<SandboxRecord> {
        let req = create_pb(r);
        let asked = req.network.clone();
        let r = self
            .sandboxes()
            .create_sandbox(req)
            .await
            .map_err(Self::map)?
            .into_inner();
        self.applied(&asked, r).await
    }

    /// The sandbox a create answered, once the daemon says it applied the
    /// guest network asked for (`CreateSandboxResponse.network`). A daemon
    /// that predates `network` never reads it and creates the sandbox with
    /// outbound network: that sandbox is deleted and the create refused,
    /// never handed back open.
    async fn applied(&self, asked: &str, r: pb::CreateSandboxResponse) -> Result<SandboxRecord> {
        let rec = Self::sandbox_of(r.sandbox)?;
        let none = cua_sandbox_core::NetworkMode::parse(asked)
            .is_ok_and(|n| n == cua_sandbox_core::NetworkMode::None);
        if !none || r.network == "none" {
            return Ok(rec);
        }
        let _ = self.delete(&rec.name).await;
        let version = self
            .info()
            .await
            .map(|i| format!(" ({})", i.version))
            .unwrap_or_default();
        Err(Error::CapabilityMissing(format!(
            "network none: the cua daemon{version} is too old to enforce it (the sandbox would \
             have outbound network), so the sandbox it made ({}) was deleted; update cua and \
             restart its daemon",
            rec.name
        )))
    }

    /// `CreateSandboxStream`: [`DaemonClient::create`], passing what the
    /// create does (the image download with its bytes, the boot, the
    /// probes) to `progress` as it happens. A daemon that predates the
    /// stream gets a plain `CreateSandbox`, and `progress` hears nothing.
    pub async fn create_reporting(
        &self,
        r: CreateRequest,
        progress: cua_sandbox_core::progress::Sink,
    ) -> Result<SandboxRecord> {
        use pb::create_sandbox_stream_response::Event;
        let req = create_pb(r);
        let asked = req.network.clone();
        let mut stream = match self
            .sandboxes()
            .create_sandbox_stream(pb::CreateSandboxStreamRequest {
                create: Some(req.clone()),
            })
            .await
        {
            Ok(s) => s.into_inner(),
            Err(e) if e.code() == tonic::Code::Unimplemented => {
                let r = self
                    .sandboxes()
                    .create_sandbox(req)
                    .await
                    .map_err(Self::map)?
                    .into_inner();
                return self.applied(&asked, r).await;
            }
            Err(e) => return Err(Self::map(e)),
        };
        while let Some(msg) = stream.message().await.map_err(Self::map)? {
            match msg.event {
                Some(Event::Progress(p)) => {
                    if let Some(p) = progress_from_pb(&p) {
                        progress(&p);
                    }
                }
                Some(Event::Result(r)) => return self.applied(&asked, r).await,
                None => {}
            }
        }
        Err(Error::Internal(
            "the daemon ended the create stream without a result".into(),
        ))
    }
}

/// A streamed progress report, back as the runtime's (`None`: a phase this
/// build does not know).
pub fn progress_from_pb(
    p: &pb::CreateSpaceProgress,
) -> Option<cua_sandbox_core::progress::Progress> {
    use cua_sandbox_core::progress::{Phase, Progress, Transfer};
    let mut out = Progress::phase(Phase::parse(&p.phase)?).detail(p.detail.clone());
    if let Some(f) = p.fraction {
        out = out.fraction(f);
    }
    if let Some(done) = p.bytes_done {
        out = out.bytes(Transfer {
            done,
            total: p.bytes_total.unwrap_or(0),
            per_second: p.bytes_per_second,
        });
    }
    out.target = p.space.clone();
    Some(out)
}

/// A create as its `CreateSandboxRequest`. The deprecated fields still go
/// out for daemons that predate `location` / `kind` / `runtime`.
#[allow(deprecated)]
fn create_pb(r: CreateRequest) -> pb::CreateSandboxRequest {
    crate::convert::create_request_to_pb(r)
}

impl DaemonClient {
    /// `ConnectSandbox`.
    pub async fn connect(&self, name: &str) -> Result<SandboxRecord> {
        Self::sandbox_of(
            self.sandboxes()
                .connect_sandbox(pb::ConnectSandboxRequest { name: name.into() })
                .await
                .map_err(Self::map)?
                .into_inner()
                .sandbox,
        )
    }

    /// `GetSandbox`.
    pub async fn get(&self, name: &str) -> Result<SandboxRecord> {
        Self::sandbox_of(
            self.sandboxes()
                .get_sandbox(pb::GetSandboxRequest { name: name.into() })
                .await
                .map_err(Self::map)?
                .into_inner()
                .sandbox,
        )
    }

    /// `ListSandboxes`: the known sandboxes of `provider` (all when
    /// `None`).
    #[allow(deprecated)]
    pub async fn list_filtered(
        &self,
        provider: Option<ProviderKind>,
    ) -> Result<crate::runtime::Listing> {
        let r = self
            .sandboxes()
            .list_sandboxes(pb::ListSandboxesRequest {
                location: provider
                    .map(|p| crate::runtime::location_of(p).to_string())
                    .unwrap_or_default(),
                provider: provider
                    .map(|p| provider_to_pb(p) as i32)
                    .unwrap_or_default(),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(crate::runtime::Listing {
            sandboxes: r.sandboxes.iter().map(info_from_pb).collect(),
            warnings: r.warnings,
        })
    }

    /// `ListSandboxes` of the known sandboxes only.
    pub async fn list(&self, provider: Option<ProviderKind>) -> Result<Vec<SandboxRecord>> {
        Ok(self.list_filtered(provider).await?.sandboxes)
    }

    /// `DeleteSandbox`.
    /// `CancelCreateSandbox`.
    pub async fn cancel_create(&self, name: &str) -> Result<Option<String>> {
        let r = self
            .sandboxes()
            .cancel_create_sandbox(pb::CancelCreateSandboxRequest { name: name.into() })
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(r.cancelled.then_some(r.message))
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        self.sandboxes()
            .delete_sandbox(pb::DeleteSandboxRequest { name: name.into() })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `CheckpointSandbox`.
    pub async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<pb::Checkpoint> {
        self.checkpoint_with(name, checkpoint, false).await
    }

    /// `CheckpointSandbox`, pinning the new checkpoint when `pin`.
    pub async fn checkpoint_with(
        &self,
        name: &str,
        checkpoint: &str,
        pin: bool,
    ) -> Result<pb::Checkpoint> {
        self.sandboxes()
            .checkpoint_sandbox(pb::CheckpointSandboxRequest {
                name: name.into(),
                checkpoint: checkpoint.into(),
                pin,
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .checkpoint
            .ok_or_else(|| Error::Internal("no checkpoint in the answer".into()))
    }

    /// `ForkSandbox`: `name`, started from a sandbox or a checkpoint.
    pub async fn fork(&self, source: &str, name: &str) -> Result<SandboxRecord> {
        Self::sandbox_of(
            self.sandboxes()
                .fork_sandbox(pb::ForkSandboxRequest {
                    source: source.into(),
                    name: name.into(),
                    labels: Default::default(),
                })
                .await
                .map_err(Self::map)?
                .into_inner()
                .sandbox,
        )
    }

    /// `ForkSandboxBatch`: the batch's events as they happen (the last is
    /// `done`).
    pub async fn fork_batch(
        &self,
        req: pb::ForkSandboxBatchRequest,
    ) -> Result<tonic::Streaming<pb::ForkSandboxBatchResponse>> {
        Ok(
            cua_proto::daemon::v1::fork_batch_service_client::ForkBatchServiceClient::new(
                self.channel.clone(),
            )
            .max_decoding_message_size(cua_spacesd_client::MAX_MESSAGE_BYTES)
            .fork_sandbox_batch(req)
            .await
            .map_err(Self::map)?
            .into_inner(),
        )
    }

    /// `HostSnapshotsService.PublishHostSnapshot` on this machine: uploads
    /// checkpoint `name` to `fallback`. Returns the bytes uploaded.
    pub async fn publish_checkpoint(
        &self,
        name: &str,
        fallback: cua_proto::env::v1::SnapshotFallback,
    ) -> Result<u64> {
        let mut s =
            cua_proto::env::v1::host_snapshots_service_client::HostSnapshotsServiceClient::new(
                self.channel.clone(),
            )
            .publish_host_snapshot(cua_proto::env::v1::PublishHostSnapshotRequest {
                name: name.into(),
                fallback: Some(fallback),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        let mut done = 0;
        while let Some(m) = s.message().await.map_err(Self::map)? {
            if let Some(p) = m.progress {
                done = p.bytes_done;
            }
        }
        Ok(done)
    }

    /// `ListCheckpoints`.
    pub async fn checkpoints(&self) -> Result<Vec<pb::Checkpoint>> {
        Ok(self
            .sandboxes()
            .list_checkpoints(pb::ListCheckpointsRequest {})
            .await
            .map_err(Self::map)?
            .into_inner()
            .checkpoints)
    }

    /// `DeleteCheckpoint`.
    pub async fn delete_checkpoint(&self, name: &str) -> Result<()> {
        self.sandboxes()
            .delete_checkpoint(pb::DeleteCheckpointRequest { name: name.into() })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `PinCheckpoint`: pins or unpins a checkpoint.
    pub async fn pin_checkpoint(&self, name: &str, pinned: bool) -> Result<pb::Checkpoint> {
        self.sandboxes()
            .pin_checkpoint(pb::PinCheckpointRequest {
                name: name.into(),
                pinned,
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .checkpoint
            .ok_or_else(|| Error::Internal("no checkpoint in the answer".into()))
    }

    /// `PruneCheckpoints`: deletes every checkpoint that is not pinned;
    /// returns their names.
    pub async fn prune_checkpoints(&self) -> Result<Vec<String>> {
        Ok(self
            .sandboxes()
            .prune_checkpoints(pb::PruneCheckpointsRequest {})
            .await
            .map_err(Self::map)?
            .into_inner()
            .deleted)
    }

    /// `CreatePool`.
    pub async fn create_pool(
        &self,
        name: Option<&str>,
        spec: CreateRequest,
        min: u32,
        max: u32,
        idle: Option<std::time::Duration>,
    ) -> Result<crate::PoolInfo> {
        let r = self
            .pools()
            .create_pool(pb::CreatePoolRequest {
                name: name.unwrap_or_default().into(),
                spec: Some(crate::convert::create_request_to_pb(spec)),
                min,
                max,
                idle: idle.map(dur),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        r.pool
            .map(crate::convert::pool_from_pb)
            .ok_or_else(|| Error::Internal("the daemon answered without the pool".into()))
    }

    /// `GetPool`.
    pub async fn get_pool(&self, name: &str) -> Result<crate::PoolInfo> {
        self.pools()
            .get_pool(pb::GetPoolRequest { name: name.into() })
            .await
            .map_err(Self::map)?
            .into_inner()
            .pool
            .map(crate::convert::pool_from_pb)
            .ok_or_else(|| Error::Internal("the daemon answered without the pool".into()))
    }

    /// `ListPools`.
    pub async fn list_pools(&self) -> Result<Vec<crate::PoolInfo>> {
        Ok(self
            .pools()
            .list_pools(pb::ListPoolsRequest {})
            .await
            .map_err(Self::map)?
            .into_inner()
            .pools
            .into_iter()
            .map(crate::convert::pool_from_pb)
            .collect())
    }

    /// `DeletePool`.
    pub async fn delete_pool(&self, name: &str) -> Result<bool> {
        Ok(self
            .pools()
            .delete_pool(pb::DeletePoolRequest { name: name.into() })
            .await
            .map_err(Self::map)?
            .into_inner()
            .deleted)
    }

    /// `SuspendSandbox`.
    pub async fn suspend(&self, name: &str) -> Result<()> {
        self.sandboxes()
            .suspend_sandbox(pb::SuspendSandboxRequest { name: name.into() })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `ResumeSandbox`.
    pub async fn resume(&self, name: &str) -> Result<()> {
        self.sandboxes()
            .resume_sandbox(pb::ResumeSandboxRequest { name: name.into() })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `RestartSandbox`.
    pub async fn restart(&self, name: &str) -> Result<()> {
        self.sandboxes()
            .restart_sandbox(pb::RestartSandboxRequest { name: name.into() })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `ResizeSandbox`.
    pub async fn resize(&self, name: &str, shape: &cua_vmm::Resize) -> Result<()> {
        self.sandboxes()
            .resize_sandbox(pb::ResizeSandboxRequest {
                name: name.into(),
                cpus: shape.cpus,
                memory_mb: shape.memory_mb,
                disk_gb: shape.disk_gb,
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `KeepAlive`.
    pub async fn keep_alive(&self, name: &str, d: Duration) -> Result<()> {
        self.sandboxes()
            .keep_alive(pb::KeepAliveRequest {
                name: name.into(),
                duration: Some(dur(d)),
                on_expire: String::new(),
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `KeepAlive` with `on_expire`: a local sandbox's lifetime and what
    /// ends it (`delete` or `suspend`).
    pub async fn set_lifetime(
        &self,
        name: &str,
        d: Duration,
        on_expire: cua_sandbox_core::LocalExpiry,
    ) -> Result<()> {
        self.sandboxes()
            .keep_alive(pb::KeepAliveRequest {
                name: name.into(),
                duration: Some(dur(d)),
                on_expire: on_expire.as_str().into(),
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `WaitReady`.
    pub async fn wait_ready(&self, name: &str, probes: &[Probe], timeout: Duration) -> Result<()> {
        self.sandboxes()
            .wait_ready(pb::WaitReadyRequest {
                name: name.into(),
                probes: probes.iter().map(probe_to_pb).collect(),
                timeout: Some(dur(timeout)),
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `GetServiceEndpoint`: the daemon's streaming passthrough for a
    /// sandbox service (URL and headers).
    pub async fn service_endpoint(
        &self,
        name: &str,
        service: &str,
    ) -> Result<cua_sandbox_core::ServiceEndpoint> {
        let r = self
            .sandboxes()
            .get_service_endpoint(pb::GetServiceEndpointRequest {
                name: name.into(),
                service: service.into(),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(cua_sandbox_core::ServiceEndpoint {
            url: r.url,
            headers: r.headers.into_iter().map(|h| (h.name, h.value)).collect(),
        })
    }

    /// `ServiceRequest`.
    #[allow(clippy::too_many_arguments)]
    pub async fn service_request(
        &self,
        name: &str,
        service: &str,
        method: &str,
        path: &str,
        headers: &[(String, String)],
        body: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<HttpResult> {
        let r = self
            .sandboxes()
            .service_request(pb::ServiceRequestRequest {
                name: name.into(),
                service: service.into(),
                method: method.into(),
                path: path.into(),
                body: body.unwrap_or_default(),
                timeout: Some(dur(timeout)),
                headers: headers
                    .iter()
                    .map(|(name, value)| pb::HttpHeader {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(HttpResult {
            status: r.status as u16,
            headers: r.headers.into_iter().map(|h| (h.name, h.value)).collect(),
            body: r.body.to_vec(),
        })
    }

    /// `ForwardPort`.
    pub async fn forward(&self, name: &str, port: u16) -> Result<crate::ForwardInfo> {
        let r = self
            .sandboxes()
            .forward_port(pb::ForwardPortRequest {
                name: name.into(),
                port: port as u32,
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        let opt = |s: String| (!s.is_empty()).then_some(s);
        Ok(crate::ForwardInfo {
            id: r.forward_id,
            guest_port: r.guest_port as u16,
            local_addr: opt(r.local_addr),
            url: opt(r.url),
        })
    }

    /// `GetServiceUrl`.
    pub async fn service_url(&self, name: &str, service: &str) -> Result<String> {
        Ok(self
            .sandboxes()
            .get_service_url(pb::GetServiceUrlRequest {
                name: name.into(),
                service: service.into(),
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .url)
    }

    /// `CreatePublicUrl` for a sandbox service.
    pub async fn public_url(
        &self,
        name: &str,
        service: &str,
        ttl: Option<Duration>,
        label: Option<String>,
    ) -> Result<crate::shares::PublicUrl> {
        self.create_public_url(pb::CreatePublicUrlRequest {
            name: name.into(),
            service: service.into(),
            ttl: ttl.map(dur),
            label: label.unwrap_or_default(),
            upstream_url: String::new(),
        })
        .await
    }

    /// `CreatePublicUrl` for a loopback upstream (the daemon hosts it).
    pub async fn share(
        &self,
        upstream: &str,
        ttl: Duration,
        name: &str,
        service: &str,
    ) -> Result<crate::shares::PublicUrl> {
        self.create_public_url(pb::CreatePublicUrlRequest {
            name: name.into(),
            service: service.into(),
            ttl: Some(dur(ttl)),
            label: String::new(),
            upstream_url: upstream.into(),
        })
        .await
    }

    async fn create_public_url(
        &self,
        req: pb::CreatePublicUrlRequest,
    ) -> Result<crate::shares::PublicUrl> {
        let r = self
            .sandboxes()
            .create_public_url(req)
            .await
            .map_err(Self::map)?
            .into_inner()
            .public_url
            .ok_or_else(|| Error::Internal("daemon returned no public URL".into()))?;
        Ok(crate::convert::public_url_from_pb(&r))
    }

    /// `RevokePublicUrl`.
    pub async fn revoke_public_url(&self, name: &str, id: &str) -> Result<()> {
        self.sandboxes()
            .revoke_public_url(pb::RevokePublicUrlRequest {
                name: name.into(),
                id: id.into(),
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `CreateShare`: a link to a port, a Space port, a file or a folder.
    pub async fn create_share(
        &self,
        target: &crate::ShareTarget,
        access: crate::ShareAccess,
        ttl: Option<Duration>,
        name: Option<String>,
    ) -> Result<crate::ShareInfo> {
        let r = self
            .share_svc()
            .create_share(pb::CreateShareRequest {
                target: Some(target.to_pb()),
                access: access.to_pb(),
                ttl: ttl.map(dur),
                name: name.unwrap_or_default(),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        r.share
            .as_ref()
            .map(crate::ShareInfo::from_pb)
            .ok_or_else(|| Error::Internal("daemon returned no share".into()))
    }

    /// `ListShares`.
    pub async fn list_shares(&self) -> Result<Vec<crate::ShareInfo>> {
        let r = self
            .share_svc()
            .list_shares(pb::ListSharesRequest {})
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(r.shares.iter().map(crate::ShareInfo::from_pb).collect())
    }

    /// `StopShare` for one share (id or name), or all of them.
    pub async fn stop_share(&self, id: Option<&str>, all: bool) -> Result<Vec<String>> {
        Ok(self
            .share_svc()
            .stop_share(pb::StopShareRequest {
                id: id.unwrap_or_default().into(),
                all,
            })
            .await
            .map_err(Self::map)?
            .into_inner()
            .stopped)
    }

    /// `CheckShare`: opens the link once, as its owner.
    pub async fn check_share(&self, id: &str, path: Option<&str>) -> Result<crate::ShareCheck> {
        let r = self
            .share_svc()
            .check_share(pb::CheckShareRequest {
                id: id.into(),
                path: path.unwrap_or_default().into(),
            })
            .await
            .map_err(Self::map)?
            .into_inner();
        Ok(crate::ShareCheck {
            status: r.status as u16,
            ok: r.ok,
            via: r.via,
            detail: r.detail,
        })
    }

    /// `CloseForward`.
    pub async fn close_forward(&self, id: &str) -> Result<()> {
        self.sandboxes()
            .close_forward(pb::CloseForwardRequest {
                forward_id: id.into(),
            })
            .await
            .map_err(Self::map)?;
        Ok(())
    }

    /// `GetEnvEndpoint`.
    pub async fn env_endpoint(
        &self,
        name: &str,
        probe_timeout: Option<Duration>,
    ) -> Result<pb::GetEnvEndpointResponse> {
        Ok(self
            .sandboxes()
            .get_env_endpoint(pb::GetEnvEndpointRequest {
                name: name.into(),
                probe_timeout: probe_timeout.map(dur),
            })
            .await
            .map_err(Self::map)?
            .into_inner())
    }
}

/// Maps a status of a daemon call. A connect that fails because nothing
/// listens (a refused or missing socket or port) is
/// [`Error::DaemonNotRunning`], never a raw transport error; other
/// failures before any response stay [`Error::Transport`].
pub fn status_error(e: tonic::Status) -> Error {
    if e.code() == tonic::Code::Unavailable && e.details().is_empty() {
        if refused(&e) {
            return Error::DaemonNotRunning(crate::DAEMON_NOT_RUNNING.into());
        }
        if lost(&e) {
            return Error::DaemonNotRunning(crate::DAEMON_LOST.into());
        }
        return Error::Transport(format!("cannot reach the cua daemon: {}", e.message()));
    }
    if lost(&e) {
        return Error::DaemonNotRunning(crate::DAEMON_LOST.into());
    }
    Error::from_status(&e)
}

/// Whether a status is the connection to the daemon breaking mid-call
/// (tonic's "transport error", an h2 or hyper failure, a reset or a closed
/// pipe), not an error the daemon answered with.
fn lost(e: &tonic::Status) -> bool {
    use std::io::ErrorKind as K;
    use tonic::Code;
    if !e.details().is_empty()
        || !matches!(
            e.code(),
            Code::Unknown | Code::Internal | Code::Unavailable | Code::Cancelled
        )
    {
        return false;
    }
    let m = e.message();
    if m == "transport error"
        || m.starts_with("h2 protocol error")
        || m.contains("connection closed")
    {
        return true;
    }
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(err) = src {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                K::ConnectionReset | K::ConnectionAborted | K::BrokenPipe | K::UnexpectedEof
            )
        {
            return true;
        }
        src = err.source();
    }
    false
}

/// The daemon being started ([`crate::starting_pid`]) when none accepts
/// connections yet: waits (bounded by `budget`) until it does. `Ok(None)`
/// when no daemon runs or starts; `Err` when one is still starting after
/// `budget`, so the caller neither runs a runtime of its own meanwhile nor
/// hangs.
pub fn wait_for_starting(budget: Duration) -> Result<Option<DaemonAddress>> {
    if let Some(a) = live_address() {
        return Ok(Some(a));
    }
    let Some(pid) = crate::starting_pid() else {
        return Ok(None);
    };
    let deadline = std::time::Instant::now() + budget;
    loop {
        if let Some(a) = live_address() {
            return Ok(Some(a));
        }
        if crate::starting_pid().is_none() {
            // It finished (or gave up) between two looks.
            return Ok(live_address());
        }
        if std::time::Instant::now() >= deadline {
            return Err(Error::Timeout(format!(
                "the cua daemon (pid {pid}) is still starting after {} s; try again in a moment",
                budget.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Whether a status carries a connect failure because nothing listens.
fn refused(e: &tonic::Status) -> bool {
    use std::io::ErrorKind as K;
    let mut src: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(e);
    while let Some(err) = src {
        if let Some(io) = err.downcast_ref::<std::io::Error>()
            && matches!(
                io.kind(),
                K::ConnectionRefused | K::NotFound | K::AddrNotAvailable
            )
        {
            return true;
        }
        src = err.source();
    }
    false
}

/// The spawn channel of the Space this process runs in, when it has one:
/// `CUA_SPAWN` and `CUA_SPAWN_TOKEN_FILE` (set for processes started after
/// the channel), else the files cua-spacesd keeps
/// ([`cua_proto::metadata::spawn_dirs`]). Through it, `cua` and the SDKs
/// in a Space make and manage the Space's own children; nothing else.
pub fn spawn_channel() -> Option<DaemonAddress> {
    use cua_proto::metadata as m;
    let read = |p: &std::path::Path| {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    if let (Some(url), Some(file)) = (
        std::env::var(m::ENV_SPAWN).ok().filter(|v| !v.is_empty()),
        std::env::var_os(m::ENV_SPAWN_TOKEN_FILE),
    ) && let Some(token) = read(std::path::Path::new(&file))
    {
        return Some(DaemonAddress::Url { url, token });
    }
    #[cfg(unix)]
    // SAFETY: getuid has no preconditions.
    let uid = Some(unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let uid = None;
    let xdg = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    m::spawn_dirs(uid, xdg.as_deref(), &std::env::temp_dir())
        .into_iter()
        .find_map(|dir| {
            let url = read(&dir.join(m::SPAWN_ADDRESS_FILE))?;
            let token = read(&dir.join(m::SPAWN_TOKEN_FILE))?;
            Some(DaemonAddress::Url { url, token })
        })
}

/// The address of the daemon this machine runs when one accepts
/// connections: the discovery file (`~/.cua/daemon.json`), else the
/// default socket. A discovery file left by a daemon that exited is
/// removed with its socket ([`crate::live_discovery`]); `None` means no
/// daemon runs.
pub fn live_address() -> Option<DaemonAddress> {
    if let Some(d) = crate::live_discovery(&crate::default_discovery_path()) {
        #[cfg(unix)]
        if let Some(s) = d.socket_path {
            return Some(DaemonAddress::Socket(PathBuf::from(s)));
        }
        if let (Some(url), Some(token)) = (d.loopback_url, d.token) {
            return Some(DaemonAddress::Url { url, token });
        }
        return None;
    }
    #[cfg(unix)]
    {
        let s = crate::default_socket_path();
        if crate::socket_listening(&s) {
            return Some(DaemonAddress::Socket(s));
        }
    }
    None
}

/// A client of the daemon this machine already runs, after checking it
/// answers; [`Error::DaemonNotRunning`] when none does. No process is
/// started.
pub async fn existing_daemon() -> Result<DaemonClient> {
    let addr =
        live_address().ok_or_else(|| Error::DaemonNotRunning(crate::DAEMON_NOT_RUNNING.into()))?;
    let c = DaemonClient::new(addr)?;
    tokio::time::timeout(Duration::from_secs(5), c.info())
        .await
        .map_err(|_| Error::Transport("the cua daemon did not answer".into()))??;
    Ok(c)
}

/// [`existing_daemon`], but a daemon that is recorded as running and does
/// not answer yet (busy with a burst of creates, answering after 5 s) is
/// waited for up to `patience` instead of being taken for absent: an
/// absent daemon is started, and a CLI lookup that fails there turned a
/// busy daemon into "no cua daemon is running".
pub async fn existing_daemon_patient(patience: Duration) -> Result<DaemonClient> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        match existing_daemon().await {
            Ok(c) => return Ok(c),
            // Nothing recorded as running: the caller starts one.
            Err(e) if live_address().is_none() => return Err(e),
            Err(e) if tokio::time::Instant::now() >= deadline => return Err(e),
            Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
        }
    }
}

/// The `cua` CLI used to start a daemon: `CUA_BIN`, else this executable
/// when it is the CLI, else `cua` on `PATH`.
pub fn cua_binary() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("CUA_BIN").filter(|v| !v.is_empty()) {
        let p = PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    if let Ok(me) = std::env::current_exe()
        && me.file_stem().is_some_and(|s| s == "cua")
    {
        return Some(me);
    }
    let exe = if cfg!(windows) { "cua.exe" } else { "cua" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(exe))
        .find(|p| p.is_file())
}

/// Whether a daemon reporting `running` is older than a client at `ours`.
/// Unparsable versions are never called outdated.
pub fn daemon_is_outdated(running: &str, ours: &str) -> bool {
    matches!((version_key(running), version_key(ours)), (Some(r), Some(o)) if r < o)
}

/// What [`ensure_daemon`] does with a running daemon older than itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutdatedDaemon {
    /// Keep using it and say how to replace it.
    Warn,
    /// Stop it and start this version (`CUA_DAEMON_RESTART_OUTDATED=1`).
    Restart,
}

fn outdated_daemon_policy(value: Option<&str>) -> OutdatedDaemon {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if matches!(v.as_str(), "1" | "true" | "on" | "yes") => OutdatedDaemon::Restart,
        _ => OutdatedDaemon::Warn,
    }
}

/// The running daemon, or one started now with `cua daemon start` (the
/// CLI from [`cua_binary`]). `CUA_DAEMON_AUTOSTART=0` turns starting off.
///
/// A running daemon older than this client (after an upgrade) keeps serving
/// with a one-time warning, since stopping it would end the sandboxes and
/// sessions it hosts for other clients. `CUA_DAEMON_RESTART_OUTDATED=1`
/// stops it and starts this version instead.
pub async fn ensure_daemon() -> Result<DaemonClient> {
    if let Ok(c) = existing_daemon_patient(Duration::from_secs(30)).await {
        let running = c.info().await.map(|i| i.version).unwrap_or_default();
        if !daemon_is_outdated(&running, crate::VERSION) {
            return Ok(c);
        }
        let policy =
            outdated_daemon_policy(std::env::var("CUA_DAEMON_RESTART_OUTDATED").ok().as_deref());
        if policy == OutdatedDaemon::Warn {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| {
                tracing::warn!(running = %running, ours = crate::VERSION, "the cua daemon is outdated");
                eprintln!(
                    "cua: the running cua daemon is {running}, older than this cua {ours}. \
                     Run `cua daemon stop` (the next command starts {ours}), or set \
                     CUA_DAEMON_RESTART_OUTDATED=1 to replace it automatically.",
                    ours = crate::VERSION
                );
            });
            return Ok(c);
        }
        tracing::info!(running = %running, ours = crate::VERSION, "replacing the outdated cua daemon");
        c.shutdown().await?;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while existing_daemon().await.is_ok() {
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(
                    "the outdated cua daemon did not stop within 10 s".into(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    let off = std::env::var("CUA_DAEMON_AUTOSTART")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off" | "no"
            )
        })
        .unwrap_or(false);
    if off {
        return Err(Error::Unsupported(
            "no cua daemon is running and CUA_DAEMON_AUTOSTART=0".into(),
        ));
    }
    let bin = cua_binary().ok_or_else(|| {
        Error::Unsupported(
            "no cua daemon is running and the `cua` CLI was not found (set CUA_BIN or run \
             `cua daemon start`)"
                .into(),
        )
    })?;
    // `cua daemon start` forks the daemon and returns once it is reachable.
    let status = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::process::Command::new(&bin)
            .args(["daemon", "start"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status(),
    )
    .await
    .map_err(|_| Error::Timeout("`cua daemon start` did not return within 20 s".into()))?
    .map_err(|e| Error::Internal(format!("{}: {e}", bin.display())))?;
    if !status.success() {
        return Err(Error::Internal(format!(
            "`{} daemon start` failed ({status})",
            bin.display()
        )));
    }
    existing_daemon().await
}

#[cfg(test)]
mod outdated_daemon_tests {
    use super::{OutdatedDaemon, daemon_is_outdated, outdated_daemon_policy};

    #[test]
    fn older_daemons_are_outdated() {
        assert!(daemon_is_outdated("0.2.0", "0.3.0"));
        assert!(daemon_is_outdated("0.2.9", "0.2.10"));
        assert!(daemon_is_outdated("0.3.0-rc.1", "0.3.0"));
        assert!(daemon_is_outdated("v1.0.0", "1.0.1"));
    }

    #[test]
    fn same_newer_or_unknown_daemons_are_not_outdated() {
        assert!(!daemon_is_outdated("0.3.0", "0.3.0"));
        assert!(!daemon_is_outdated("0.4.0", "0.3.0"));
        assert!(!daemon_is_outdated("0.3.0", "0.3.0-rc.1"));
        assert!(!daemon_is_outdated("", "0.3.0"));
        assert!(!daemon_is_outdated("dev", "0.3.0"));
        assert!(!daemon_is_outdated("0.3", "0.3.1"));
    }

    #[test]
    fn restart_is_opt_in() {
        assert_eq!(outdated_daemon_policy(None), OutdatedDaemon::Warn);
        assert_eq!(outdated_daemon_policy(Some("0")), OutdatedDaemon::Warn);
        assert_eq!(outdated_daemon_policy(Some("")), OutdatedDaemon::Warn);
        assert_eq!(outdated_daemon_policy(Some("1")), OutdatedDaemon::Restart);
        assert_eq!(
            outdated_daemon_policy(Some(" TRUE ")),
            OutdatedDaemon::Restart
        );
    }
}

#[cfg(test)]
mod status_error_tests {
    use super::status_error;
    use crate::Error;
    use tonic::{Code, Status};

    #[test]
    fn a_daemon_lost_mid_call_reads_as_what_to_do() {
        for s in [
            Status::new(Code::Internal, "transport error"),
            Status::new(Code::Unknown, "transport error"),
            Status::new(
                Code::Unknown,
                "h2 protocol error: error reading a body from connection",
            ),
            Status::new(
                Code::Unavailable,
                "connection closed before message completed",
            ),
        ] {
            match status_error(s.clone()) {
                Error::DaemonNotRunning(m) => {
                    assert_eq!(m, crate::DAEMON_LOST, "{s:?}");
                    assert!(m.contains("cua daemon start"));
                }
                other => panic!("{s:?} -> {other:?}"),
            }
        }
    }

    #[test]
    fn errors_the_daemon_answers_with_pass_through() {
        let e = status_error(Status::new(Code::Internal, "disk full"));
        assert!(!matches!(e, Error::DaemonNotRunning(_)), "{e:?}");
        assert!(e.to_string().contains("disk full"), "{e}");
        let e = status_error(Status::new(Code::NotFound, "transport error"));
        assert!(!matches!(e, Error::DaemonNotRunning(_)), "{e:?}");
    }
}
