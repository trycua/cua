// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `HostSpacesService` on a host (`cua-spacesd join` with a host policy):
//! Spaces this machine provides to its owner's enrolled devices, through the
//! relay. A host in direct mode (`cua-spacesd serve --direct-host-policy`)
//! serves it on its direct listener to the env token holder, by default
//! only to loopback, Tailscale and private LAN peers
//! ([`crate::peer::is_private_address`]), since that listener is plaintext.
//!
//! The driver authenticates the caller (a relay-verified editor, or the
//! local token holder), checks that the host provides Spaces, and forwards
//! the call to the host's cua daemon over its Unix socket with the verified
//! caller in [`CALLER_METADATA`]. The daemon creates and deletes the Spaces
//! with its own runtimes (Lume, Docker), attaches each to the relay, keeps
//! the capacity limits and writes the audit; the driver never runs a
//! sandbox itself and never exposes this machine's own desktop or files
//! through this service.
//!
//! When the daemon's socket does not answer, the driver starts it with the
//! `cua` CLI named in the policy (`cua daemon start`), once per call, and
//! waits a bounded time for it.
//!
//! `HostJobsService` (cluster jobs: each runs once in a fresh Space the
//! daemon creates and deletes) is forwarded the same way, to the same
//! callers, under the same policy.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::host_jobs_service_client::HostJobsServiceClient;
use cua_proto::env::v1::host_jobs_service_server::{HostJobsService, HostJobsServiceServer};
use cua_proto::env::v1::host_snapshots_service_client::HostSnapshotsServiceClient;
use cua_proto::env::v1::host_snapshots_service_server::{
    HostSnapshotsService, HostSnapshotsServiceServer,
};
use cua_proto::env::v1::host_spaces_service_client::HostSpacesServiceClient;
use cua_proto::env::v1::host_spaces_service_server::{HostSpacesService, HostSpacesServiceServer};
use cua_proto::env::v1::*;
use tonic::{Code, Request, Response, Status};

use crate::auth::caller;
use crate::context::ServerContext;
use crate::error::status;
use crate::provider::{feature, ServiceProvider};
use crate::relay_account::{HostPolicy, SpacesDaemon};

/// Metadata the driver sets on a forwarded call: the verified caller as
/// JSON (`{"account","email","name","role","via"}`). Any value a client
/// sent is replaced.
pub const CALLER_METADATA: &str = "x-cua-host-caller";

/// The feature `GetCapabilities` reports while this host provides Spaces.
pub const HOST_SPACES_FEATURE: &str = "host_spaces";

/// The feature `GetCapabilities` reports while this host runs cluster jobs
/// (`HostJobsService`): whenever it provides Spaces.
pub const HOST_JOBS_FEATURE: &str = "host_jobs";

/// How long a forwarded create may run: a macOS VM's first create
/// downloads a 20+ GB image before it boots, which on a slow connection
/// takes well over the 20 min this once was (the daemon's own budgets
/// bound each phase; this only catches a daemon that lost the call).
const CREATE_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// How long the driver waits for a daemon it started (the daemon socket
/// is a Unix socket; other systems do not start one).
#[cfg(unix)]
const DAEMON_START_WAIT: Duration = Duration::from_secs(20);

/// Serves `HostSpacesService` from the host policy at `policy_file`.
pub struct HostSpacesProvider {
    policy_file: PathBuf,
}

impl HostSpacesProvider {
    /// A provider reading the host policy at `policy_file` on every call.
    pub fn new(policy_file: impl Into<PathBuf>) -> Self {
        let policy_file = policy_file.into();
        if let Some(dir) = policy_file.parent() {
            let _ = ROUTED_HOST_DIR.set(dir.to_path_buf());
        }
        Self { policy_file }
    }
}

/// The host directory (next to the policy) whose `provided-spaces.json`
/// lists the routed Spaces this host forwards on loopback.
static ROUTED_HOST_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Whether `port` is the loopback forward of a routed Space this host
/// provides that `account` may use (it created it, or owns this machine).
pub(crate) fn routed_port_allowed(port: u16, account: &str, owner: bool) -> bool {
    let Some(dir) = ROUTED_HOST_DIR.get() else {
        return false;
    };
    routed_port_allowed_in(dir, port, account, owner)
}

/// [`routed_port_allowed`] for the host directory `dir`.
pub(crate) fn routed_port_allowed_in(dir: &Path, port: u16, account: &str, owner: bool) -> bool {
    /// The fields of a `provided-spaces.json` row this check reads (the
    /// host's cua writes the file: `cua_host::provided::ProvidedSpace`).
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Provided {
        #[serde(default)]
        routed: bool,
        #[serde(default)]
        direct_port: u16,
        #[serde(default)]
        created_by_account: String,
    }
    let Ok(raw) = std::fs::read(dir.join("provided-spaces.json")) else {
        return false;
    };
    let Ok(all) = serde_json::from_slice::<Vec<Provided>>(&raw) else {
        return false;
    };
    all.iter().any(|s| {
        s.routed
            && s.direct_port == port
            && port != 0
            && (owner || (!account.is_empty() && s.created_by_account == account))
    })
}

#[cfg(test)]
mod routed_tests {
    use super::*;

    #[test]
    fn only_a_routed_spaces_own_port_is_forwarded() {
        let dir = tempfile::tempdir().unwrap();
        let rec = |port: u16, routed: bool, acct: &str| cua_host::provided::ProvidedSpace {
            local_space: format!("local:s{port}"),
            name: format!("s{port}"),
            image: "linux".into(),
            created_by: "ada".into(),
            created_by_account: acct.into(),
            direct_port: port,
            routed,
            ..Default::default()
        };
        cua_host::provided::save_provided(
            dir.path(),
            &[rec(40001, true, "ada"), rec(40002, false, "ada")],
        )
        .unwrap();
        let ok = |p, a: &str, o| routed_port_allowed_in(dir.path(), p, a, o);
        assert!(ok(40001, "ada", false));
        assert!(!ok(40001, "bob", false), "another account's Space");
        assert!(ok(40001, "bob", true), "the machine's owner");
        assert!(!ok(40002, "ada", false), "a direct Space is not routed");
        assert!(!ok(22, "ada", true), "never another port");
        assert!(!ok(40001, "", false));
    }
}

fn read_policy(path: &Path) -> Result<HostPolicy, Status> {
    let raw = std::fs::read(path).map_err(|e| {
        status(
            Code::FailedPrecondition,
            ErrorReason::NotInitialized,
            format!("host policy {}: {e}", path.display()),
        )
    })?;
    serde_json::from_slice(&raw).map_err(|e| {
        status(
            Code::FailedPrecondition,
            ErrorReason::Internal,
            format!("host policy {}: {e}", path.display()),
        )
    })
}

impl ServiceProvider for HostSpacesProvider {
    fn capabilities(&self) -> Vec<Feature> {
        let providing = read_policy(&self.policy_file)
            .map(|p| p.provide_spaces && p.spaces_daemon.is_some())
            .unwrap_or(false);
        let limitation = if providing {
            ""
        } else {
            "this computer does not provide Spaces (on it: `cua host config --provide-spaces on`)"
        };
        vec![
            feature(HOST_SPACES_FEATURE, providing, limitation),
            feature(HOST_JOBS_FEATURE, providing, limitation),
        ]
    }

    fn register(
        &self,
        routes: tonic::service::Routes,
        _ctx: &ServerContext,
    ) -> tonic::service::Routes {
        let host = HostSpacesImpl {
            policy_file: Arc::new(self.policy_file.clone()),
        };
        routes
            .add_service(HostJobsServiceServer::new(HostJobsImpl(host.clone())))
            .add_service(
                HostSnapshotsServiceServer::new(HostSnapshotsImpl(host.clone()))
                    .max_decoding_message_size(64 * 1024 * 1024)
                    .max_encoding_message_size(64 * 1024 * 1024),
            )
            .add_service(HostSpacesServiceServer::new(host))
    }

    fn http_routes(&self) -> Option<axum::Router> {
        // The checkpoint files this host exported, to the holders of a read
        // grant the host's daemon issued (it signs them with a key in its
        // cua home, which the route verifies), from the peers a host call
        // is taken from.
        let policy_file = self.policy_file.clone();
        let home: cua_snapshot_sync::serve::HomeFn = Arc::new(move || {
            let p = read_policy(&policy_file).ok()?;
            if !p.provide_spaces {
                return None;
            }
            let d = p.spaces_daemon?;
            Some(if d.cua_home.is_empty() {
                std::path::PathBuf::from(std::env::var_os("HOME")?).join(".cua")
            } else {
                std::path::PathBuf::from(d.cua_home)
            })
        });
        let policy_file = self.policy_file.clone();
        let allow: cua_snapshot_sync::serve::AllowFn = Arc::new(move |req| {
            let policy = read_policy(&policy_file).map_err(|s| s.message().to_string())?;
            let peer = req.extensions().get::<crate::peer::PeerAddr>().map(|p| p.0);
            match peer_refusal(&policy, peer) {
                Some(refused) => Err(refused.message().to_string()),
                None => Ok(()),
            }
        });
        Some(cua_snapshot_sync::serve::router(home, allow))
    }

    fn services(&self) -> Vec<&'static str> {
        vec![
            "cua.env.v1.HostSpacesService",
            "cua.env.v1.HostJobsService",
            "cua.env.v1.HostSnapshotsService",
        ]
    }

    fn http_paths(&self) -> Vec<&'static str> {
        vec![cua_proto::metadata::SNAPSHOT_BLOBS_PATH]
    }
}

#[derive(Clone)]
struct HostSpacesImpl {
    policy_file: Arc<PathBuf>,
}

/// `HostJobsService`, forwarded like `HostSpacesService`.
struct HostJobsImpl(HostSpacesImpl);

/// How long a forwarded job call may take besides its own wait (a batch
/// of job acceptances is written to the host's ledger first).
const JOBS_CALL: Duration = Duration::from_secs(60);

impl HostJobsImpl {
    async fn client<T>(
        &self,
        req: &Request<T>,
    ) -> Result<(HostJobsServiceClient<tonic::transport::Channel>, String), Status> {
        let policy = read_policy(&self.0.policy_file)?;
        if let Some(refusal) = peer_refusal(&policy, crate::peer::peer_of(req)) {
            return Err(refusal);
        }
        let who = caller_json(req)?;
        if !policy.provide_spaces {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::FeatureUnsupported,
                "this machine does not provide Spaces, so it runs no jobs (on it: `cua host config --provide-spaces on`)",
            ));
        }
        let daemon = policy.spaces_daemon.ok_or_else(|| {
            status(
                Code::FailedPrecondition,
                ErrorReason::NotInitialized,
                "this host's policy names no cua daemon; run `cua host setup` on it again",
            )
        })?;
        let channel = daemon_channel(&daemon).await?;
        Ok((
            HostJobsServiceClient::new(channel)
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024),
            who,
        ))
    }
}

#[tonic::async_trait]
impl HostJobsService for HostJobsImpl {
    async fn run_host_jobs(
        &self,
        request: Request<RunHostJobsRequest>,
    ) -> Result<Response<RunHostJobsResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .run_host_jobs(with_caller(request.into_inner(), &who, Some(JOBS_CALL))?)
            .await
    }

    async fn get_host_job(
        &self,
        request: Request<GetHostJobRequest>,
    ) -> Result<Response<GetHostJobResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .get_host_job(with_caller(request.into_inner(), &who, Some(JOBS_CALL))?)
            .await
    }

    async fn list_host_jobs(
        &self,
        request: Request<ListHostJobsRequest>,
    ) -> Result<Response<ListHostJobsResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        let r = request.into_inner();
        let wait = Duration::from_millis(u64::from(r.wait_ms.min(30_000)));
        client
            .list_host_jobs(with_caller(r, &who, Some(JOBS_CALL + wait))?)
            .await
    }

    async fn cancel_host_job(
        &self,
        request: Request<CancelHostJobRequest>,
    ) -> Result<Response<CancelHostJobResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .cancel_host_job(with_caller(request.into_inner(), &who, Some(JOBS_CALL))?)
            .await
    }
}

// Keep the JSON readable as ASCII gRPC metadata without losing Unicode names.
struct AsciiJsonFormatter;

impl serde_json::ser::Formatter for AsciiJsonFormatter {
    fn write_string_fragment<W: ?Sized + std::io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> std::io::Result<()> {
        for ch in fragment.chars() {
            if ch.is_ascii() {
                writer.write_all(&[ch as u8])?;
            } else {
                for unit in ch.encode_utf16(&mut [0; 2]) {
                    write!(writer, "\\u{unit:04x}")?;
                }
            }
        }
        Ok(())
    }
}

/// The verified caller as the daemon receives it.
fn caller_json<T>(req: &Request<T>) -> Result<String, Status> {
    let who = caller(req);
    if who.viewer.is_some() {
        return Err(status(
            Code::PermissionDenied,
            ErrorReason::PermissionDenied,
            "a view-only share cannot create or delete Spaces on this host",
        ));
    }
    let value = match (&who.account, who.asserted, who.token_verified) {
        (Some(a), true, _) => serde_json::json!({
            "account": a.account, "email": a.email, "name": a.name,
            "role": a.role,
            // Who vouched: the relay (an assertion, also over the mesh) or
            // the host's tailnet (`--tailnet`).
            "via": if who.principal.as_ref().is_some_and(|p| p.id.starts_with("tailnet:")) {
                "tailnet"
            } else {
                "relay"
            },
        }),
        (None, false, true) => serde_json::json!({
            "account": "local", "role": "owner", "via": "token",
        }),
        _ => {
            return Err(status(
                Code::Unauthenticated,
                ErrorReason::Unauthenticated,
                "HostSpacesService needs a relay-verified account or this computer's token",
            ))
        }
    };
    let mut bytes = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(&mut bytes, AsciiJsonFormatter);
    serde::Serialize::serialize(&value, &mut serializer)
        .map_err(|_| Status::internal("caller metadata JSON"))?;
    String::from_utf8(bytes).map_err(|_| Status::internal("caller metadata UTF-8"))
}

/// One start of the host's daemon at a time (concurrent creates wait for
/// the same start).
#[cfg(unix)]
static STARTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn daemon_channel(daemon: &SpacesDaemon) -> Result<tonic::transport::Channel, Status> {
    #[cfg(unix)]
    {
        let socket = PathBuf::from(&daemon.socket);
        let connect = |socket: PathBuf| async move {
            tonic::transport::Endpoint::from_static("http://cua-daemon.invalid")
                .connect_timeout(Duration::from_secs(5))
                .connect_with_connector(tower::service_fn(move |_: http::Uri| {
                    let socket = socket.clone();
                    async move {
                        let s = tokio::net::UnixStream::connect(socket).await?;
                        Ok::<_, std::io::Error>(hyper_util::rt::TokioIo::new(s))
                    }
                }))
                .await
        };
        if let Ok(ch) = connect(socket.clone()).await {
            return Ok(ch);
        }
        let _starting = STARTING.lock().await;
        if let Ok(ch) = connect(socket.clone()).await {
            return Ok(ch);
        }
        let Some(bin) = daemon.cua_bin.as_deref().filter(|b| !b.is_empty()) else {
            return Err(status(
                Code::Unavailable,
                ErrorReason::TargetUnavailable,
                "the cua daemon on this host is not running (on it: `cua daemon start`)",
            ));
        };
        tracing::info!(%bin, socket = %socket.display(), "starting the host's cua daemon");
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(["daemon", "start"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .env("CUA_DAEMON_STARTED_BY", "host");
        if !daemon.cua_home.is_empty() {
            cmd.env("CUA_HOME", &daemon.cua_home);
        }
        let _ = tokio::time::timeout(DAEMON_START_WAIT, cmd.status()).await;
        let deadline = tokio::time::Instant::now() + DAEMON_START_WAIT;
        loop {
            if let Ok(ch) = connect(socket.clone()).await {
                return Ok(ch);
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(status(
                    Code::Unavailable,
                    ErrorReason::TargetUnavailable,
                    format!(
                        "the cua daemon on this host did not start (`{bin} daemon start`); \
                         run it on the host and retry"
                    ),
                ));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = daemon;
        Err(status(
            Code::Unimplemented,
            ErrorReason::FeatureUnsupported,
            "providing Spaces from a Windows host is not supported yet",
        ))
    }
}

impl HostSpacesImpl {
    /// Checks the caller and the policy, then connects to the daemon.
    async fn forward_to<T>(
        &self,
        req: &Request<T>,
    ) -> Result<(HostSpacesServiceClient<tonic::transport::Channel>, String), Status> {
        self.forward(req, true).await
    }

    /// [`Self::forward_to`]; `provided` requires that this host provides
    /// Spaces (a Space in the owner's own cloud that this machine created
    /// does not).
    async fn forward<T>(
        &self,
        req: &Request<T>,
        provided: bool,
    ) -> Result<(HostSpacesServiceClient<tonic::transport::Channel>, String), Status> {
        let policy = read_policy(&self.policy_file)?;
        if let Some(refusal) = peer_refusal(&policy, crate::peer::peer_of(req)) {
            return Err(refusal);
        }
        let who = caller_json(req)?;
        if provided && !policy.provide_spaces {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::FeatureUnsupported,
                "this computer does not provide Spaces (on it: `cua host config --provide-spaces on`)",
            ));
        }
        let daemon = policy.spaces_daemon.ok_or_else(|| {
            status(
                Code::FailedPrecondition,
                ErrorReason::NotInitialized,
                "this host's policy names no cua daemon; run `cua host setup` on it again",
            )
        })?;
        let channel = daemon_channel(&daemon).await?;
        Ok((HostSpacesServiceClient::new(channel), who))
    }
}

/// A direct host takes host calls only from loopback, Tailscale and private
/// LAN peers unless its policy allows any address. A relay host, and a
/// request that did not come over TCP, are not limited here.
fn peer_refusal(policy: &HostPolicy, peer: Option<SocketAddr>) -> Option<Status> {
    let direct = policy.direct.as_ref()?;
    if direct.allow_any_address {
        return None;
    }
    let peer = peer?;
    if crate::peer::is_private_address(peer.ip()) {
        return None;
    }
    Some(status(
        Code::PermissionDenied,
        ErrorReason::PermissionDenied,
        format!(
            "this host takes host calls on its plaintext direct listener only from loopback, \
             Tailscale and private LAN addresses, not {}; connect over Tailscale or the LAN (on \
             the host, `cua host setup --direct ... --allow-any-address` accepts any address)",
            peer.ip()
        ),
    ))
}

/// Starts the host's cua daemon when a host in direct mode starts, so the
/// ports it forwards to the Spaces it provides are open again after a
/// restart (the daemon reopens them when asked for its Spaces). Best
/// effort; nothing for a relay host or one that does not provide Spaces.
pub async fn wake_daemon(policy_file: PathBuf) {
    let Ok(policy) = read_policy(&policy_file) else {
        return;
    };
    if policy.direct.is_none() || !policy.provide_spaces {
        return;
    }
    let Some(daemon) = policy.spaces_daemon else {
        return;
    };
    let channel = match daemon_channel(&daemon).await {
        Ok(channel) => channel,
        Err(error) => {
            tracing::warn!(error = %error.message(), "the host's cua daemon did not start");
            return;
        }
    };
    let who = serde_json::json!({"account": "local", "role": "owner", "via": "token"}).to_string();
    let Ok(request) = with_caller(GetHostSpacesRequest {}, &who, Some(Duration::from_secs(60)))
    else {
        return;
    };
    if let Err(error) = HostSpacesServiceClient::new(channel)
        .get_host_spaces(request)
        .await
    {
        tracing::warn!(error = %error.message(), "the host's cua daemon did not list its Spaces");
    }
}

fn with_caller<T>(message: T, who: &str, timeout: Option<Duration>) -> Result<Request<T>, Status> {
    let mut out = Request::new(message);
    out.metadata_mut().insert(
        CALLER_METADATA,
        who.parse()
            .map_err(|_| Status::internal("caller metadata"))?,
    );
    if let Some(t) = timeout {
        out.set_timeout(t);
    }
    Ok(out)
}

/// `HostSnapshotsService`, forwarded like `HostSpacesService`.
struct HostSnapshotsImpl(HostSpacesImpl);

/// How long a forwarded snapshot copy may take (gigabytes over the LAN, or
/// from the fallback).
const SNAPSHOT_COPY: Duration = Duration::from_secs(2 * 60 * 60);

impl HostSnapshotsImpl {
    async fn client<T>(
        &self,
        req: &Request<T>,
    ) -> Result<
        (
            HostSnapshotsServiceClient<tonic::transport::Channel>,
            String,
        ),
        Status,
    > {
        let policy = read_policy(&self.0.policy_file)?;
        if let Some(refusal) = peer_refusal(&policy, crate::peer::peer_of(req)) {
            return Err(refusal);
        }
        let who = caller_json(req)?;
        if !policy.provide_spaces {
            return Err(status(
                Code::FailedPrecondition,
                ErrorReason::FeatureUnsupported,
                "this machine does not provide Spaces (on it: `cua host config --provide-spaces on`)",
            ));
        }
        let daemon = policy.spaces_daemon.ok_or_else(|| {
            status(
                Code::FailedPrecondition,
                ErrorReason::NotInitialized,
                "this host's policy names no cua daemon; run `cua host setup` on it again",
            )
        })?;
        let channel = daemon_channel(&daemon).await?;
        Ok((
            HostSnapshotsServiceClient::new(channel)
                .max_decoding_message_size(64 * 1024 * 1024)
                .max_encoding_message_size(64 * 1024 * 1024),
            who,
        ))
    }
}

type Forwarded<T> = std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl HostSnapshotsService for HostSnapshotsImpl {
    async fn list_host_snapshots(
        &self,
        request: Request<ListHostSnapshotsRequest>,
    ) -> Result<Response<ListHostSnapshotsResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .list_host_snapshots(with_caller(
                request.into_inner(),
                &who,
                Some(Duration::from_secs(60)),
            )?)
            .await
    }

    async fn export_host_snapshot(
        &self,
        request: Request<ExportHostSnapshotRequest>,
    ) -> Result<Response<ExportHostSnapshotResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .export_host_snapshot(with_caller(
                request.into_inner(),
                &who,
                // Hashing a checkpoint the first time reads all of it.
                Some(Duration::from_secs(30 * 60)),
            )?)
            .await
    }

    async fn create_host_snapshot(
        &self,
        request: Request<CreateHostSnapshotRequest>,
    ) -> Result<Response<CreateHostSnapshotResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .create_host_snapshot(with_caller(
                request.into_inner(),
                &who,
                // Saving a VM's memory takes a while on a busy host.
                Some(Duration::from_secs(10 * 60)),
            )?)
            .await
    }

    async fn delete_host_snapshot(
        &self,
        request: Request<DeleteHostSnapshotRequest>,
    ) -> Result<Response<DeleteHostSnapshotResponse>, Status> {
        let (mut client, who) = self.client(&request).await?;
        client
            .delete_host_snapshot(with_caller(
                request.into_inner(),
                &who,
                Some(Duration::from_secs(5 * 60)),
            )?)
            .await
    }

    type PullHostSnapshotStream = Forwarded<PullHostSnapshotResponse>;

    async fn pull_host_snapshot(
        &self,
        request: Request<PullHostSnapshotRequest>,
    ) -> Result<Response<Self::PullHostSnapshotStream>, Status> {
        let (mut client, who) = self.client(&request).await?;
        let s = client
            .pull_host_snapshot(with_caller(
                request.into_inner(),
                &who,
                Some(SNAPSHOT_COPY),
            )?)
            .await?
            .into_inner();
        Ok(Response::new(Box::pin(s)))
    }

    type PublishHostSnapshotStream = Forwarded<PublishHostSnapshotResponse>;

    async fn publish_host_snapshot(
        &self,
        request: Request<PublishHostSnapshotRequest>,
    ) -> Result<Response<Self::PublishHostSnapshotStream>, Status> {
        let (mut client, who) = self.client(&request).await?;
        let s = client
            .publish_host_snapshot(with_caller(
                request.into_inner(),
                &who,
                Some(SNAPSHOT_COPY),
            )?)
            .await?
            .into_inner();
        Ok(Response::new(Box::pin(s)))
    }
}

#[tonic::async_trait]
impl HostSpacesService for HostSpacesImpl {
    async fn get_host_spaces(
        &self,
        request: Request<GetHostSpacesRequest>,
    ) -> Result<Response<GetHostSpacesResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .get_host_spaces(with_caller(
                request.into_inner(),
                &who,
                Some(Duration::from_secs(60)),
            )?)
            .await
    }

    async fn create_host_space(
        &self,
        request: Request<CreateHostSpaceRequest>,
    ) -> Result<Response<CreateHostSpaceResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .create_host_space(with_caller(
                request.into_inner(),
                &who,
                Some(CREATE_TIMEOUT),
            )?)
            .await
    }

    async fn cancel_host_space(
        &self,
        request: Request<CancelHostSpaceRequest>,
    ) -> Result<Response<CancelHostSpaceResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .cancel_host_space(with_caller(
                request.into_inner(),
                &who,
                // The host waits for its clean-up (a VM delete).
                Some(Duration::from_secs(5 * 60)),
            )?)
            .await
    }

    async fn delete_cloud_space(
        &self,
        request: Request<DeleteCloudSpaceRequest>,
    ) -> Result<Response<DeleteCloudSpaceResponse>, Status> {
        let (mut client, who) = self.forward(&request, false).await?;
        client
            .delete_cloud_space(with_caller(
                request.into_inner(),
                &who,
                // The daemon waits for the cloud to delete the machine.
                Some(Duration::from_secs(10 * 60)),
            )?)
            .await
    }

    async fn diagnose_host(
        &self,
        request: Request<DiagnoseHostRequest>,
    ) -> Result<Response<DiagnoseHostResponse>, Status> {
        // Whether or not this machine provides Spaces: its health is the
        // owner's to see either way (the daemon checks the caller).
        let (mut client, who) = self.forward(&request, false).await?;
        // This machine's own health only: never a hop to another machine.
        let mut message = request.into_inner();
        message.machine.clear();
        client
            .diagnose_host(with_caller(message, &who, Some(Duration::from_secs(30)))?)
            .await
    }

    async fn delete_host_space(
        &self,
        request: Request<DeleteHostSpaceRequest>,
    ) -> Result<Response<DeleteHostSpaceResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .delete_host_space(with_caller(
                request.into_inner(),
                &who,
                Some(Duration::from_secs(10 * 60)),
            )?)
            .await
    }

    async fn set_host_space_power(
        &self,
        request: Request<SetHostSpacePowerRequest>,
    ) -> Result<Response<SetHostSpacePowerResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .set_host_space_power(with_caller(
                request.into_inner(),
                &who,
                // Turning on waits for a VM's boot and its driver.
                Some(Duration::from_secs(5 * 60)),
            )?)
            .await
    }

    async fn set_host_warm(
        &self,
        request: Request<cua_proto::env::v1::SetHostWarmRequest>,
    ) -> Result<Response<cua_proto::env::v1::SetHostWarmResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .set_host_warm(with_caller(
                request.into_inner(),
                &who,
                // An image pull may come first.
                Some(Duration::from_secs(5 * 60)),
            )?)
            .await
    }

    async fn set_host_space_egress(
        &self,
        request: Request<SetHostSpaceEgressRequest>,
    ) -> Result<Response<SetHostSpaceEgressResponse>, Status> {
        let (mut client, who) = self.forward_to(&request).await?;
        client
            .set_host_space_egress(with_caller(
                request.into_inner(),
                &who,
                // A QEMU VM moves into its network namespace (its memory
                // is streamed to a new process).
                Some(Duration::from_secs(5 * 60)),
            )?)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{CallerIdentity, RelayCaller};

    fn req(identity: CallerIdentity) -> Request<()> {
        let mut r = Request::new(());
        r.extensions_mut().insert(identity);
        r
    }

    #[test]
    fn the_caller_is_the_verified_account_or_the_local_token() {
        let relayed = req(CallerIdentity {
            asserted: true,
            token_verified: true,
            account: Some(RelayCaller {
                account: "ada".into(),
                email: Some("ada@example.com".into()),
                name: None,
                role: "owner".into(),
                ..Default::default()
            }),
            ..Default::default()
        });
        let v: serde_json::Value = serde_json::from_str(&caller_json(&relayed).unwrap()).unwrap();
        assert_eq!(
            (v["account"].as_str(), v["via"].as_str()),
            (Some("ada"), Some("relay"))
        );
        let local = req(CallerIdentity {
            token_verified: true,
            ..Default::default()
        });
        let v: serde_json::Value = serde_json::from_str(&caller_json(&local).unwrap()).unwrap();
        assert_eq!(v["account"], "local");
        // No credential at all (an open loopback server) is refused.
        let anonymous = req(CallerIdentity::default());
        assert_eq!(
            caller_json(&anonymous).unwrap_err().code(),
            Code::Unauthenticated
        );
    }

    #[test]
    fn caller_metadata_round_trips_unicode_and_json_escapes() {
        for name in [
            "Ada",
            "Jos\u{e9} P\u{e9}rez",
            "\u{65e5}\u{672c}\u{8a9e}",
            "\u{1f680}",
            "quote\" slash\\ newline\n",
        ] {
            let relayed = req(CallerIdentity {
                asserted: true,
                account: Some(RelayCaller {
                    account: "ada".into(),
                    email: Some("ada@example.com".into()),
                    name: Some(name.into()),
                    role: "owner".into(),
                    ..Default::default()
                }),
                ..Default::default()
            });
            let json = caller_json(&relayed).unwrap();
            let forwarded = with_caller((), &json, Some(Duration::from_secs(60))).unwrap();
            let text = forwarded
                .metadata()
                .get(CALLER_METADATA)
                .unwrap()
                .to_str()
                .unwrap();
            assert!(text.is_ascii());
            let value: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(value["name"], name);
            assert_eq!(value["account"], "ada");
            assert_eq!(value["email"], "ada@example.com");
            assert_eq!(value["role"], "owner");
            assert_eq!(value["via"], "relay");
            if name.is_ascii() {
                assert_eq!(text, value.to_string());
            }
            assert!(forwarded.metadata().contains_key("grpc-timeout"));
        }
    }

    #[test]
    fn a_direct_host_takes_host_calls_only_from_private_peers() {
        let peer = |s: &str| Some(s.parse::<SocketAddr>().unwrap());
        let relay = HostPolicy::default();
        assert!(peer_refusal(&relay, peer("203.0.113.7:4000")).is_none());
        let mut direct = HostPolicy {
            direct: Some(crate::relay_account::DirectHosting {
                listen: "0.0.0.0:3211".into(),
                allow_any_address: false,
            }),
            ..HostPolicy::default()
        };
        for ok in [
            "127.0.0.1:1",
            "100.101.102.103:1",
            "192.168.1.5:1",
            "[fd7a:115c:a1e0::9]:1",
        ] {
            assert!(peer_refusal(&direct, peer(ok)).is_none(), "{ok}");
        }
        // In process (no TCP peer): not limited here.
        assert!(peer_refusal(&direct, None).is_none());
        let refused = peer_refusal(&direct, peer("203.0.113.7:4000")).unwrap();
        assert_eq!(refused.code(), Code::PermissionDenied);
        assert!(
            refused.message().contains("203.0.113.7"),
            "{}",
            refused.message()
        );
        assert!(refused.message().contains("--allow-any-address"));
        direct.direct.as_mut().unwrap().allow_any_address = true;
        assert!(peer_refusal(&direct, peer("203.0.113.7:4000")).is_none());
    }

    #[test]
    fn capabilities_follow_the_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("host.json");
        let p = HostSpacesProvider::new(&path);
        assert!(!p.capabilities()[0].supported, "no policy");
        assert_eq!(p.capabilities()[1].name, HOST_JOBS_FEATURE);
        std::fs::write(
            &path,
            r#"{"owner":"ada","provide_spaces":true,"spaces_daemon":{"socket":"/x/cua.sock"}}"#,
        )
        .unwrap();
        assert!(p.capabilities()[0].supported);
        assert!(p.capabilities()[1].supported, "jobs follow Spaces");
        std::fs::write(&path, r#"{"owner":"ada","provide_spaces":false}"#).unwrap();
        assert!(!p.capabilities()[0].supported);
    }
}
