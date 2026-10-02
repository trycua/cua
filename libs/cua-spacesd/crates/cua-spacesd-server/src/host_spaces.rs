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

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

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

/// How long a forwarded create may run (a macOS VM's first boot).
const CREATE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
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
        Self {
            policy_file: policy_file.into(),
        }
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
        vec![feature(
            HOST_SPACES_FEATURE,
            providing,
            if providing {
                ""
            } else {
                "this machine does not provide Spaces (on it: `cua host config --provide-spaces on`)"
            },
        )]
    }

    fn register(
        &self,
        routes: tonic::service::Routes,
        _ctx: &ServerContext,
    ) -> tonic::service::Routes {
        routes.add_service(HostSpacesServiceServer::new(HostSpacesImpl {
            policy_file: Arc::new(self.policy_file.clone()),
        }))
    }

    fn http_routes(&self) -> Option<axum::Router> {
        None
    }

    fn services(&self) -> Vec<&'static str> {
        vec!["cua.env.v1.HostSpacesService"]
    }
}

struct HostSpacesImpl {
    policy_file: Arc<PathBuf>,
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
            "role": a.role, "via": "relay",
        }),
        (None, false, true) => serde_json::json!({
            "account": "local", "role": "owner", "via": "token",
        }),
        _ => {
            return Err(status(
                Code::Unauthenticated,
                ErrorReason::Unauthenticated,
                "HostSpacesService needs a relay-verified account or this machine's token",
            ))
        }
    };
    Ok(value.to_string())
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
        std::fs::write(
            &path,
            r#"{"owner":"ada","provide_spaces":true,"spaces_daemon":{"socket":"/x/cua.sock"}}"#,
        )
        .unwrap();
        assert!(p.capabilities()[0].supported);
        std::fs::write(&path, r#"{"owner":"ada","provide_spaces":false}"#).unwrap();
        assert!(!p.capabilities()[0].supported);
    }
}
