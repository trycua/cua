//! `cua daemon`: the cua SDK runtime hosted out of process.
//!
//! - [`Runtime`] is *the* SDK runtime: one [`cua_sandbox_core::Sandboxes`]
//!   (local, direct, your own cloud), cached sandbox handles and spacesd
//!   connections. `cua-sdk` embeds it in-process
//!   (`Cua::embedded`); the daemon hosts the same value.
//! - [`server`] serves `cua.daemon.v1` on a Unix socket (`~/.cua/cua.sock`,
//!   mode 0600, no token) and/or loopback TCP (bearer token). The loopback
//!   listener also carries
//!   - the **env passthrough** `/v1/sandboxes/<name>/env/<grpc path>`: any
//!     `cua.env.v1` call (native gRPC or gRPC-Web) is proxied to the
//!     sandbox's spacesd with the sandbox credentials attached by the
//!     daemon, including the `/media` WebSocket;
//!   - the **media bridge** `/v1/bridge/media?ticket=<bridge ticket>` for
//!     webviews: a short-lived bridge ticket from `OpenMediaBridge`, so a
//!     browser never sees an env token.
//! - [`client::DaemonClient`] is the typed client `Cua::connect` uses.
//!
//! Errors cross the wire as `google.rpc.Status` with a packed
//! `cua.daemon.v1.DaemonErrorInfo`; both sides use [`Error`], so embedded
//! and daemon topologies fail identically.

use cua_proto::daemon::v1::{DaemonErrorInfo, DaemonErrorReason};
use prost::Message;
use std::path::PathBuf;

#[cfg(feature = "client")]
pub mod client;
#[cfg(feature = "byoc")]
mod cloud_sweep;
mod convert;
pub mod doctor;
/// Extension points for the Keyvault, the Cua Volume and teleport (Cua
/// Spaces).
#[cfg(feature = "spaces")]
pub mod extension;
#[cfg(feature = "test-fixtures")]
pub mod fixtures;
pub mod guard;
pub mod guest_identity;
#[cfg(all(feature = "server", feature = "spaces"))]
mod host_svc;
pub mod identity;
pub mod local;
#[cfg(feature = "spaces")]
pub mod machine_doctor;
pub mod maintenance;
#[cfg(feature = "server")]
mod passthrough;
pub mod pool;
#[cfg(all(feature = "server", feature = "spaces"))]
mod prewarm_pools;
mod runtime;
#[cfg(feature = "server")]
pub mod server;
pub mod share_types;
pub mod shares;
pub use share_types::{ShareAccess, ShareCheck, ShareInfo, ShareTarget};
#[cfg(feature = "spaces")]
pub mod share_hub;
#[cfg(feature = "spaces")]
mod share_link;
#[cfg(feature = "server")]
mod share_svc;
#[cfg(feature = "server")]
mod spaces_svc;
pub mod storage;

pub use convert::{
    build_from_pb, build_to_pb, info_to_pb, probe_from_pb, probe_to_pb, provider_from_pb,
    provider_to_pb, registry_secret_from_pb, registry_secret_to_pb, sidecar_from_pb, sidecar_to_pb,
};
#[cfg(feature = "spaces")]
pub use cua_spaces;
#[cfg(feature = "server")]
pub use passthrough::space_key;
pub use runtime::{
    CreateRequest, ForwardInfo, LIST_CLOUD_TIMEOUT, Listing, PoolInfo, Runtime, RuntimeConfig,
    SandboxRecord, SpacesdAttachment, contrib_provider_names, location_of,
};

/// The released cua SDK version this daemon was built as (reported by
/// `GetInfo` and the discovery file; see build.rs).
pub const VERSION: &str = env!("CUA_DAEMON_VERSION");

/// Type URL of a packed `DaemonErrorInfo`.
pub const ERROR_INFO_TYPE_URL: &str = "type.googleapis.com/cua.daemon.v1.DaemonErrorInfo";

/// Every failure of the SDK runtime, in both topologies.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// Bad input.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// No such sandbox, forward or bridge.
    #[error("not found: {0}")]
    NotFound(String),
    /// A bare sandbox name matches sandboxes in more than one location.
    #[error("ambiguous sandbox name: {message}")]
    AmbiguousSandbox {
        /// The message (it names the candidates too).
        message: String,
        /// The qualified refs the name matches (`local:box`, `cloud:box`).
        candidates: Vec<String>,
    },
    /// A location, kind or runtime that does not exist, or a combination
    /// that does not; `valid` lists the accepted values for `axis`.
    #[error("invalid placement: {message}")]
    InvalidPlacement {
        /// The message (it lists the valid values too).
        message: String,
        /// `on`, `kind`, `runtime` or `image`.
        axis: String,
        /// The accepted values.
        valid: Vec<String>,
    },
    /// Provider not configured (for example a missing provider credential).
    #[error("provider not configured: {0}")]
    ProviderNotConfigured(String),
    /// The provider or this build cannot do this.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// No cua-spacesd answered.
    #[error("cua-spacesd is not available: {0}")]
    SpacesdNotAvailable(String),
    /// A deadline elapsed.
    #[error("timed out: {0}")]
    Timeout(String),
    /// Your own cloud account refused or failed a call (the message names
    /// the cloud's error).
    #[error("your cloud: {0}")]
    Cloud(String),
    /// Local runtime failure.
    #[error("local runtime: {0}")]
    Runtime(String),
    /// spacesd failure.
    #[error("env: {0}")]
    Env(String),
    /// HTTP failure talking to a sandbox service.
    #[error("http: {0}")]
    Http(String),
    /// Missing or wrong token.
    #[error("unauthenticated: {0}")]
    Unauthenticated(String),
    /// Could not reach the daemon.
    #[error("daemon transport: {0}")]
    Transport(String),
    /// No daemon listens at the address (a refused or missing socket, or a
    /// discovery file left by a daemon that exited). The message is
    /// [`DAEMON_NOT_RUNNING`]. Client-side only: never sent on the wire.
    #[error("{0}")]
    DaemonNotRunning(String),
    /// The Space's spacesd lacks a feature the call needs.
    #[error("capability missing: {0}")]
    CapabilityMissing(String),
    /// A host-side prerequisite is not available.
    #[error("host capability missing: {0}")]
    HostCapabilityMissing(String),
    /// The teleport consent gate refused.
    #[error("teleport refused: {0}")]
    TeleportRefused(String),
    /// Not enough free disk space for a pull, build or VM create.
    #[error("insufficient disk: {0}")]
    InsufficientDisk(String),
    /// This host has no room for another Space now (VM memory or CPU
    /// limit, or the machine is low on memory).
    #[error("host capacity: {0}")]
    HostCapacity(String),
    /// The create was cancelled; what it made is gone.
    #[error("cancelled: {0}")]
    Cancelled(String),
    /// Bug or I/O failure.
    #[error("internal: {0}")]
    Internal(String),
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl Error {
    /// The wire reason.
    pub fn reason(&self) -> DaemonErrorReason {
        use DaemonErrorReason as R;
        match self {
            Error::InvalidArgument(_) => R::InvalidArgument,
            Error::NotFound(_) => R::NotFound,
            Error::AmbiguousSandbox { .. } => R::AmbiguousSandbox,
            Error::InvalidPlacement { .. } => R::InvalidPlacement,
            Error::ProviderNotConfigured(_) => R::ProviderNotConfigured,
            Error::Unsupported(_) => R::Unsupported,
            Error::SpacesdNotAvailable(_) => R::SpacesdNotAvailable,
            Error::Timeout(_) => R::Timeout,
            Error::Cloud(_) => R::Cloud,
            Error::Runtime(_) => R::Runtime,
            Error::Env(_) => R::Env,
            Error::Http(_) => R::Http,
            Error::Unauthenticated(_) => R::Unauthenticated,
            Error::CapabilityMissing(_) => R::CapabilityMissing,
            Error::HostCapabilityMissing(_) => R::HostCapabilityMissing,
            Error::TeleportRefused(_) => R::TeleportRefused,
            Error::InsufficientDisk(_) => R::InsufficientDisk,
            Error::HostCapacity(_) => R::HostCapacity,
            Error::Cancelled(_) => R::Cancelled,
            Error::Transport(_) | Error::DaemonNotRunning(_) | Error::Internal(_) => R::Internal,
        }
    }

    /// The message without the variant prefix.
    pub fn message(&self) -> &str {
        match self {
            Error::InvalidArgument(m)
            | Error::NotFound(m)
            | Error::ProviderNotConfigured(m)
            | Error::Unsupported(m)
            | Error::SpacesdNotAvailable(m)
            | Error::Timeout(m)
            | Error::Cloud(m)
            | Error::Runtime(m)
            | Error::Env(m)
            | Error::Http(m)
            | Error::Unauthenticated(m)
            | Error::Transport(m)
            | Error::DaemonNotRunning(m)
            | Error::CapabilityMissing(m)
            | Error::HostCapabilityMissing(m)
            | Error::TeleportRefused(m)
            | Error::InsufficientDisk(m)
            | Error::HostCapacity(m)
            | Error::Cancelled(m)
            | Error::Internal(m)
            | Error::AmbiguousSandbox { message: m, .. }
            | Error::InvalidPlacement { message: m, .. } => m,
        }
    }

    /// The qualified candidates of [`Error::AmbiguousSandbox`] (empty for
    /// every other error).
    pub fn candidates(&self) -> &[String] {
        match self {
            Error::AmbiguousSandbox { candidates, .. } => candidates,
            _ => &[],
        }
    }

    fn from_reason(
        reason: DaemonErrorReason,
        m: String,
        metadata: &std::collections::HashMap<String, String>,
    ) -> Self {
        use DaemonErrorReason as R;
        match reason {
            R::AmbiguousSandbox => Error::AmbiguousSandbox {
                message: m,
                candidates: metadata
                    .get("candidates")
                    .map(|c| {
                        c.split(',')
                            .map(str::trim)
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            R::InvalidPlacement => Error::InvalidPlacement {
                message: m,
                axis: metadata.get("axis").cloned().unwrap_or_default(),
                valid: metadata
                    .get("valid")
                    .map(|c| {
                        c.split(',')
                            .map(str::trim)
                            .filter(|c| !c.is_empty())
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
            },
            R::InvalidArgument => Error::InvalidArgument(m),
            R::NotFound => Error::NotFound(m),
            R::ProviderNotConfigured => Error::ProviderNotConfigured(m),
            R::Unsupported => Error::Unsupported(m),
            R::SpacesdNotAvailable => Error::SpacesdNotAvailable(m),
            R::Timeout => Error::Timeout(m),
            R::Cloud => Error::Cloud(m),
            R::Runtime => Error::Runtime(m),
            R::Env => Error::Env(m),
            R::Http => Error::Http(m),
            R::Unauthenticated => Error::Unauthenticated(m),
            R::CapabilityMissing => Error::CapabilityMissing(m),
            R::HostCapabilityMissing => Error::HostCapabilityMissing(m),
            R::TeleportRefused => Error::TeleportRefused(m),
            R::InsufficientDisk => Error::InsufficientDisk(m),
            R::HostCapacity => Error::HostCapacity(m),
            R::Cancelled => Error::Cancelled(m),
            R::Internal | R::Unspecified => Error::Internal(m),
        }
    }

    /// Encodes as a `tonic::Status` with a packed `DaemonErrorInfo`.
    pub fn to_status(&self) -> tonic::Status {
        use tonic::Code;
        let code = match self {
            Error::InvalidArgument(_) | Error::InvalidPlacement { .. } => Code::InvalidArgument,
            Error::NotFound(_) => Code::NotFound,
            Error::AmbiguousSandbox { .. } => Code::FailedPrecondition,
            Error::ProviderNotConfigured(_) => Code::FailedPrecondition,
            Error::Unsupported(_) => Code::Unimplemented,
            Error::SpacesdNotAvailable(_) => Code::Unavailable,
            Error::Timeout(_) => Code::DeadlineExceeded,
            Error::Unauthenticated(_) => Code::Unauthenticated,
            Error::CapabilityMissing(_) | Error::HostCapabilityMissing(_) => {
                Code::FailedPrecondition
            }
            Error::TeleportRefused(_) => Code::PermissionDenied,
            Error::InsufficientDisk(_) | Error::HostCapacity(_) => Code::ResourceExhausted,
            Error::Cancelled(_) => Code::Cancelled,
            Error::Cloud(_) | Error::Runtime(_) | Error::Env(_) | Error::Http(_) => Code::Aborted,
            Error::Transport(_) | Error::DaemonNotRunning(_) | Error::Internal(_) => Code::Internal,
        };
        let mut metadata = std::collections::HashMap::new();
        if let Error::AmbiguousSandbox { candidates, .. } = self {
            metadata.insert("candidates".to_string(), candidates.join(","));
        }
        if let Error::InvalidPlacement { axis, valid, .. } = self {
            metadata.insert("axis".to_string(), axis.clone());
            metadata.insert("valid".to_string(), valid.join(","));
        }
        let info = DaemonErrorInfo {
            reason: self.reason() as i32,
            message: self.message().to_string(),
            metadata,
        };
        let rpc = cua_spacesd_client::error::RpcStatus {
            code: code as i32,
            message: self.message().to_string(),
            details: vec![pbjson_types::Any {
                type_url: ERROR_INFO_TYPE_URL.into(),
                value: info.encode_to_vec().into(),
            }],
        };
        tonic::Status::with_details(code, self.message().to_string(), rpc.encode_to_vec().into())
    }

    /// Decodes a status produced by [`Error::to_status`]; statuses without
    /// details map by code.
    pub fn from_status(status: &tonic::Status) -> Self {
        if let Ok(rpc) = cua_spacesd_client::error::RpcStatus::decode(status.details())
            && let Some(any) = rpc
                .details
                .iter()
                .find(|a| a.type_url.ends_with("cua.daemon.v1.DaemonErrorInfo"))
            && let Ok(info) = DaemonErrorInfo::decode(any.value.as_ref())
        {
            let reason = DaemonErrorReason::try_from(info.reason).unwrap_or_default();
            return Self::from_reason(reason, info.message, &info.metadata);
        }
        let m = status.message().to_string();
        match status.code() {
            tonic::Code::Unauthenticated => Error::Unauthenticated(m),
            tonic::Code::NotFound => Error::NotFound(m),
            tonic::Code::InvalidArgument => Error::InvalidArgument(m),
            tonic::Code::Unimplemented => Error::Unsupported(m),
            tonic::Code::DeadlineExceeded => Error::Timeout(m),
            tonic::Code::Unavailable => Error::Transport(m),
            tonic::Code::Cancelled => Error::Cancelled(m),
            _ => Error::Internal(m),
        }
    }
}

impl From<tonic::Status> for Error {
    fn from(s: tonic::Status) -> Self {
        Error::from_status(&s)
    }
}

impl From<cua_sandbox_core::Error> for Error {
    fn from(e: cua_sandbox_core::Error) -> Self {
        use cua_sandbox_core::Error as E;
        let m = e.to_string();
        match e {
            E::InvalidArgument(a) => Error::InvalidArgument(a),
            E::CloudEnded => Error::Unsupported(m),
            E::InvalidPlacement(p) => Error::from(p),
            E::NotFound(_) => Error::NotFound(m),
            E::AmbiguousSandbox {
                candidates,
                message,
                ..
            } => Error::AmbiguousSandbox {
                message,
                candidates,
            },
            E::ProviderNotConfigured(p) => Error::ProviderNotConfigured(match p {
                cua_sandbox_core::ProviderKind::Local => {
                    "local runtimes are not available in this build yet (cua-vmm)".into()
                }
                cua_sandbox_core::ProviderKind::Direct
                | cua_sandbox_core::ProviderKind::Contrib => m,
            }),
            E::ContribNotConfigured(c) => Error::ProviderNotConfigured(c),
            E::Unsupported { .. } => Error::Unsupported(m),
            E::SpacesdNotAvailable { .. } => Error::SpacesdNotAvailable(m),
            E::Timeout(t) => Error::Timeout(t),
            E::Http(h) => Error::Http(h),
            E::UnsupportedImage(u) => Error::Unsupported(u),
            E::Runtime(cua_sandbox_core::RuntimeError::UnsupportedImage(u)) => {
                Error::Unsupported(u)
            }
            E::Runtime(cua_sandbox_core::RuntimeError::InvalidPlacement(p)) => Error::from(p),
            E::Runtime(cua_sandbox_core::RuntimeError::InsufficientDisk(d)) => {
                Error::InsufficientDisk(d)
            }
            E::Runtime(cua_sandbox_core::RuntimeError::HostCapacity(m)) => Error::HostCapacity(m),
            E::Runtime(r @ cua_sandbox_core::RuntimeError::Unsupported { .. }) => {
                Error::Unsupported(r.to_string())
            }
            E::Runtime(r) => Error::Runtime(r.to_string()),
            E::Env(env) => Error::from(env),
            E::Mcp(_) => Error::Http(m),
            E::Cloud(c) => Error::Cloud(c),
            E::Io(_) | E::Json(_) => Error::Internal(m),
            E::Cancelled(c) => Error::Cancelled(c),
        }
    }
}

impl From<cua_sandbox_core::placement::PlacementError> for Error {
    fn from(p: cua_sandbox_core::placement::PlacementError) -> Self {
        Error::InvalidPlacement {
            message: p.message,
            axis: p.axis.as_str().to_string(),
            valid: p.valid,
        }
    }
}

impl From<cua_spacesd_client::Error> for Error {
    fn from(e: cua_spacesd_client::Error) -> Self {
        match e {
            cua_spacesd_client::Error::SpacesdNotAvailable { .. } => {
                Error::SpacesdNotAvailable(e.to_string())
            }
            cua_spacesd_client::Error::Unauthenticated(_) => Error::Unauthenticated(e.to_string()),
            cua_spacesd_client::Error::InvalidEndpoint(m) => Error::InvalidArgument(m),
            cua_spacesd_client::Error::Timeout(_)
            | cua_spacesd_client::Error::DesktopNotReady(_) => Error::Timeout(e.to_string()),
            cua_spacesd_client::Error::Transport(_) => Error::Transport(e.to_string()),
            other => Error::Env(other.to_string()),
        }
    }
}

#[cfg(feature = "spaces")]
impl From<cua_spaces::Error> for Error {
    fn from(e: cua_spaces::Error) -> Self {
        use cua_spaces::Error as E;
        let m = e.to_string();
        match e {
            E::InvalidArgument(a) => Error::InvalidArgument(a),
            E::NotFound(_) => Error::NotFound(m),
            E::SpacesdNotAvailable { .. } => Error::SpacesdNotAvailable(m),
            E::CapabilityMissing { .. } | E::HostTooOld(_) => Error::CapabilityMissing(m),
            E::HostCapabilityMissing { .. } => Error::HostCapabilityMissing(m),
            E::WrongProvider { .. } | E::Unsupported(_) => Error::Unsupported(m),
            // Both are a consent gate saying no; the message names which (a
            // teleport's own words: this error says "teleport refused:" itself).
            E::TeleportRefused(why) => Error::TeleportRefused(why),
            E::LoginRefused(_) => Error::TeleportRefused(m),
            E::Timeout(_) => Error::Timeout(m),
            E::Cancelled(c) => Error::Cancelled(c),
            E::Env(env) => Error::from(env),
            E::Sandbox(s) => Error::from(s),
            E::Transfer(_) | E::Agent(_) | E::Stream(_) => Error::Env(m),
            // A build step failed: the message has its command, exit code
            // and last output.
            E::Build { .. } => Error::Runtime(m),
            E::Io(_) | E::Json(_) => Error::Internal(m),
            E::Mcp(_) => Error::Http(m),
            E::Relay(r) => match r {
                cua_host::Error::Unauthenticated(_) | cua_host::Error::PermissionDenied(_) => {
                    Error::Unauthenticated(m)
                }
                cua_host::Error::NotFound(_) => Error::NotFound(m),
                cua_host::Error::InvalidArgument(_) => Error::InvalidArgument(m),
                _ => Error::Http(m),
            },
            // A host at capacity: the create cannot run there now.
            E::LimitExceeded(_) => Error::Unsupported(m),
            E::AmbiguousHost { choices, .. } => Error::AmbiguousSandbox {
                message: m,
                candidates: choices,
            },
            E::Extension { kind, .. } => match kind {
                "not_found" => Error::NotFound(m),
                "forbidden" | "not_confirmed" => Error::Unauthenticated(m),
                "volume_backend" | "drive_backend" => Error::Env(m),
                _ => Error::InvalidArgument(m),
            },
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Internal(e.to_string())
    }
}

/// `~/.cua` (or `$CUA_HOME`).
pub fn cua_home() -> PathBuf {
    cua_home::cua_home()
}

/// Default daemon socket, `~/.cua/cua.sock`.
pub fn default_socket_path() -> PathBuf {
    cua_home().join("cua.sock")
}

/// Default discovery file, `~/.cua/daemon.json` (mode 0600): pid, socket,
/// loopback URL and token of the running daemon.
pub fn default_discovery_path() -> PathBuf {
    cua_home().join("daemon.json")
}

/// What every caller shows when no daemon listens: [`Error::DaemonNotRunning`].
pub const DAEMON_NOT_RUNNING: &str =
    "The Cua daemon isn't running. Start Cua, or run `cua daemon start`.";

/// What a caller shows when the daemon went away during its call (it
/// exited, crashed or was replaced): [`Error::DaemonNotRunning`].
pub const DAEMON_LOST: &str = "The Cua daemon stopped during this call (it exited, crashed or was \
     replaced). Cua Spaces starts it again; otherwise run `cua daemon start`, then try again.";

/// `~/.cua/daemon.starting`: the pid of a daemon that is starting (from its
/// spawn until its discovery file is written), so a client waits for it
/// instead of running a runtime of its own meanwhile.
pub fn default_starting_path() -> PathBuf {
    cua_home().join("daemon.starting")
}

/// The pid [`default_starting_path`] names when that process is alive
/// (never this process); a marker of a process that exited is removed.
pub fn starting_pid() -> Option<u32> {
    let path = default_starting_path();
    let pid: u32 = std::fs::read_to_string(&path).ok()?.trim().parse().ok()?;
    if pid == std::process::id() {
        return None;
    }
    if pid_alive(pid) {
        Some(pid)
    } else {
        let _ = std::fs::remove_file(&path);
        None
    }
}

/// Marks a daemon (`pid`) as starting until dropped
/// ([`default_starting_path`]).
pub struct StartingMarker {
    path: PathBuf,
    pid: u32,
}

impl StartingMarker {
    /// Writes the marker for `pid`.
    pub fn write(pid: u32) -> Self {
        let path = default_starting_path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = write_private(&path, pid.to_string().as_bytes());
        Self { path, pid }
    }
}

impl Drop for StartingMarker {
    fn drop(&mut self) {
        // Only its own: a daemon started meanwhile keeps its marker.
        if std::fs::read_to_string(&self.path).is_ok_and(|s| s.trim() == self.pid.to_string()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Contents of the discovery file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Discovery {
    /// Daemon pid.
    pub pid: u32,
    /// Unix socket, when listening on one.
    pub socket_path: Option<String>,
    /// Loopback URL, when listening on one.
    pub loopback_url: Option<String>,
    /// Loopback bearer token.
    pub token: Option<String>,
    /// Daemon version.
    pub version: String,
}

impl Discovery {
    /// Reads the discovery file.
    pub fn read(path: &std::path::Path) -> Option<Self> {
        serde_json::from_slice(&std::fs::read(path).ok()?).ok()
    }

    /// Writes the discovery file with owner-only permissions.
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        // The file holds the loopback bearer token: create it owner-only
        // from the start (no chmod window, no reuse of a planted temp file).
        write_private(path, &serde_json::to_vec_pretty(self)?)
    }

    /// Whether a daemon accepts connections at the recorded address (the
    /// socket, else the loopback port). A connect probe, no RPC.
    pub fn listening(&self) -> bool {
        #[cfg(unix)]
        if let Some(s) = &self.socket_path {
            return socket_listening(std::path::Path::new(s));
        }
        self.loopback_url.as_deref().is_some_and(loopback_listening)
    }

    /// Whether the recorded daemon provably exited: its pid is not alive
    /// and nothing listens at its address. A live pid (even a reused one)
    /// or a listener is never stale.
    pub fn is_stale(&self) -> bool {
        self.pid != std::process::id() && !pid_alive(self.pid) && !self.listening()
    }
}

/// Whether `pid` is a live process.
pub fn pid_alive(pid: u32) -> bool {
    pid != 0 && cua_vmm::host::pid_alive(pid)
}

/// Whether `command` (a process's command line) runs a cua daemon:
/// `<dir>/cua daemon start ...` (or the Cua Spaces `cua-spaces-cli`).
#[cfg(unix)]
fn is_daemon_command(command: &str) -> bool {
    let Some(at) = command.find(" daemon start") else {
        return false;
    };
    // The executable path may hold spaces ("Cua Spaces.app"): its last
    // component is the program.
    std::path::Path::new(command[..at].trim())
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| matches!(n, "cua" | "cua-spaces-cli"))
}

/// Whether the live process `pid` is a cua daemon (by its command line),
/// so a pid read earlier was not reused by another program.
#[cfg(unix)]
pub fn is_daemon_process(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && is_daemon_command(&String::from_utf8_lossy(&o.stdout)))
}

/// Ends the cua daemon process `pid` that did not exit when asked: SIGTERM,
/// then SIGKILL, waiting up to `grace` after each. A daemon that closed
/// its listeners on `Shutdown` but never exited kept holding the Keyvault
/// socket after an app update (trycua/cua#4950). Never signals this
/// process or one that is not a cua daemon. Returns whether `pid` is gone.
/// Blocking; call it off the async runtime.
#[cfg(unix)]
pub fn terminate_daemon(pid: u32, grace: std::time::Duration) -> bool {
    if pid == 0 || pid == std::process::id() || pid > i32::MAX as u32 {
        return false;
    }
    if !pid_alive(pid) {
        return true;
    }
    if !is_daemon_process(pid) {
        return false;
    }
    for signal in [libc::SIGTERM, libc::SIGKILL] {
        // SAFETY: plain kill(2) on a positive pid checked above.
        unsafe { libc::kill(pid as libc::pid_t, signal) };
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline {
            if !pid_alive(pid) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    !pid_alive(pid)
}

/// The pid of the process listening on the Unix socket at `path`
/// (`LOCAL_PEERPID` on macOS, `SO_PEERCRED` on Linux); `None` when nothing
/// listens or the OS does not say.
#[cfg(unix)]
pub fn socket_listener_pid(path: &std::path::Path) -> Option<u32> {
    use std::os::fd::AsRawFd;
    let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    let fd = stream.as_raw_fd();
    #[cfg(target_vendor = "apple")]
    {
        let mut pid: libc::pid_t = 0;
        let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
        // SAFETY: getsockopt into a properly sized pid_t on a live socket.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_LOCAL,
                libc::LOCAL_PEERPID,
                (&mut pid as *mut libc::pid_t).cast(),
                &mut len,
            )
        };
        (rc == 0 && pid > 0).then_some(pid as u32)
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: getsockopt into a properly sized ucred on a live socket.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut cred as *mut libc::ucred).cast(),
                &mut len,
            )
        };
        (rc == 0 && cred.pid > 0).then_some(cred.pid as u32)
    }
    #[cfg(not(any(target_vendor = "apple", target_os = "linux", target_os = "android")))]
    {
        let _ = fd;
        None
    }
}

/// Whether a server accepts connections on the Unix socket at `path`
/// (a missing file or a refused connect is not listening).
#[cfg(unix)]
pub fn socket_listening(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Whether something accepts TCP connections at a loopback URL
/// (`http://127.0.0.1:<port>`). Non-loopback URLs are never probed.
pub fn loopback_listening(url: &str) -> bool {
    let Some(addr) = url::Url::parse(url).ok().and_then(|u| {
        let host: std::net::IpAddr = u.host_str()?.trim_matches(['[', ']']).parse().ok()?;
        let port = u.port_or_known_default()?;
        host.is_loopback()
            .then_some(std::net::SocketAddr::new(host, port))
    }) else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500)).is_ok()
}

/// The discovery file at `path` when its daemon accepts connections.
/// A discovery file whose daemon provably exited ([`Discovery::is_stale`])
/// is removed with its socket; one whose pid is still alive is left alone
/// (a daemon that is starting, or a reused pid) and reported as not
/// running.
pub fn live_discovery(path: &std::path::Path) -> Option<Discovery> {
    let d = Discovery::read(path)?;
    if d.listening() {
        return Some(d);
    }
    remove_stale(path, &d);
    None
}

/// Removes the discovery file at `path` and the socket it records when
/// [`Discovery::is_stale`] holds for `d`, and only when `path` still holds
/// `d` (a daemon that started meanwhile keeps its files). Returns whether
/// it removed the discovery file.
pub fn remove_stale(path: &std::path::Path, d: &Discovery) -> bool {
    if !d.is_stale() {
        return false;
    }
    #[cfg(unix)]
    if let Some(s) = &d.socket_path {
        let s = std::path::Path::new(s);
        // Only a dead socket is removed: never a regular file, a directory
        // or a symlink the discovery file happens to name.
        if is_socket_file(s) && !socket_listening(s) {
            let _ = std::fs::remove_file(s);
        }
    }
    if Discovery::read(path).as_ref() == Some(d) {
        return std::fs::remove_file(path).is_ok();
    }
    false
}

/// Whether `path` itself (not a symlink target) is a Unix socket.
#[cfg(unix)]
pub fn is_socket_file(path: &std::path::Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// Writes `data` to `path` readable by the owner only (0600 on Unix) from
/// the moment it exists, atomically. For files holding tokens.
pub fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    cua_home::write_private(path, data)
}

/// Sets mode 0600 on unix; no-op elsewhere.
pub fn restrict_permissions(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// A random URL-safe token of 256 bits.
pub fn random_token() -> String {
    use base64::Engine;
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn only_a_cua_daemon_command_line_counts() {
        assert!(is_daemon_command(
            "/Applications/Cua Spaces.app/Contents/Resources/native/cua daemon start --foreground --loopback 127.0.0.1:0\n"
        ));
        assert!(is_daemon_command(
            "/usr/local/bin/cua-spaces-cli daemon start --foreground"
        ));
        assert!(!is_daemon_command("/usr/bin/vim daemon start"));
        assert!(!is_daemon_command(
            "/Applications/Cua Spaces.app/Contents/MacOS/cua daemon stop"
        ));
        assert!(!is_daemon_command(""));
    }

    /// A process that looks like a daemon and ignores SIGTERM, as the
    /// daemon that never exited after `Shutdown` did (#4950); reaped by a
    /// thread so a killed one does not linger as a zombie.
    #[cfg(unix)]
    fn stubborn_daemon(dir: &std::path::Path) -> (u32, std::sync::mpsc::Receiver<()>) {
        use std::os::unix::fs::PermissionsExt;
        let exe = dir.join("cua");
        std::fs::write(&exe, "#!/bin/sh\ntrap '' TERM\nwhile :; do sleep 1; done\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut child = std::process::Command::new(&exe)
            .args(["daemon", "start", "--foreground"])
            .spawn()
            .unwrap();
        let pid = child.id();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = child.wait();
            let _ = tx.send(());
        });
        // Until the shell has set its trap.
        std::thread::sleep(std::time::Duration::from_millis(300));
        (pid, rx)
    }

    #[cfg(unix)]
    #[test]
    fn a_daemon_that_ignores_sigterm_is_killed() {
        let dir = tempfile::tempdir().unwrap();
        let (pid, exited) = stubborn_daemon(dir.path());
        assert!(is_daemon_process(pid));
        assert!(terminate_daemon(pid, std::time::Duration::from_secs(2)));
        exited
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the stubborn daemon exited");
    }

    #[cfg(unix)]
    #[test]
    fn terminate_never_signals_another_program() {
        let mut other = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        assert!(!is_daemon_process(other.id()));
        assert!(!terminate_daemon(
            other.id(),
            std::time::Duration::from_millis(100)
        ));
        assert!(other.try_wait().unwrap().is_none(), "left running");
        assert!(!terminate_daemon(
            std::process::id(),
            std::time::Duration::from_millis(100)
        ));
        other.kill().unwrap();
        other.wait().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_listener_of_a_socket_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.sock");
        assert_eq!(socket_listener_pid(&path), None);
        let _listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(socket_listener_pid(&path), Some(std::process::id()));
    }

    #[cfg(feature = "spaces")]
    #[test]
    fn a_teleport_refusal_says_teleport_refused_once() {
        let err = Error::from(cua_spaces::Error::TeleportRefused(
            "the approver declined".into(),
        ));
        assert_eq!(err, Error::TeleportRefused("the approver declined".into()));
        assert_eq!(err.to_string(), "teleport refused: the approver declined");
        let back = Error::from_status(&err.to_status());
        assert_eq!(back.to_string(), "teleport refused: the approver declined");
    }

    #[test]
    fn error_roundtrips_through_status() {
        for e in [
            Error::NotFound("sandbox x".into()),
            Error::SpacesdNotAvailable("no driver".into()),
            Error::Unsupported("local".into()),
            Error::ProviderNotConfigured("local".into()),
            Error::Timeout("t".into()),
            Error::InsufficientDisk("needs 3 GiB".into()),
            Error::HostCapacity("this machine has no room".into()),
            Error::AmbiguousSandbox {
                message: "\"box\" names 2 sandboxes; use one of: local:box, cloud:box".into(),
                candidates: vec!["local:box".into(), "cloud:box".into()],
            },
        ] {
            assert_eq!(Error::from_status(&e.to_status()), e);
        }
    }

    #[cfg(feature = "spaces")]
    #[test]
    fn a_full_machine_is_resource_exhausted_not_not_found() {
        let err = Error::from(cua_spaces::Error::Sandbox(
            cua_sandbox_core::Error::Runtime(cua_sandbox_core::RuntimeError::HostCapacity(
                "this machine has no room for a 2-CPU, 4 GiB Space now. fix: delete a Space".into(),
            )),
        ));
        assert!(matches!(err, Error::HostCapacity(_)), "{err:?}");
        let status = err.to_status();
        assert_eq!(status.code(), tonic::Code::ResourceExhausted);
        assert!(matches!(
            Error::from_status(&status),
            Error::HostCapacity(m) if m.contains("no room")
        ));
    }

    #[test]
    fn plain_status_maps_by_code() {
        assert!(matches!(
            Error::from_status(&tonic::Status::unauthenticated("no")),
            Error::Unauthenticated(_)
        ));
    }

    #[test]
    fn tokens_are_random_and_long() {
        let (a, b) = (random_token(), random_token());
        assert_ne!(a, b);
        assert!(a.len() >= 43);
    }
}
