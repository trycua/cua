//! `cua daemon`: the cua SDK runtime hosted out of process.
//!
//! The runtime, the typed client and everything the embedded SDK uses live in
//! `cua-daemon-core`; this crate adds the daemon server and re-exports the
//! core, so `cua_daemon::Runtime`, `cua_daemon::client` and the rest keep
//! their paths. `cua-sdk` depends on the core alone, so it does not wait for
//! the server to compile.
//!
//! - [`Runtime`] is *the* SDK runtime: one `Sandboxes` (local, direct, your
//!   own cloud), cached sandbox handles and spacesd connections. `cua-sdk`
//!   embeds it in-process (`Cua::embedded`); the daemon hosts the same value.
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
//! - `client::DaemonClient` is the typed client `Cua::connect` uses.
//!
//! Errors cross the wire as `google.rpc.Status` with a packed
//! `cua.daemon.v1.DaemonErrorInfo`; both sides use [`Error`], so embedded
//! and daemon topologies fail identically.

pub use cua_daemon_core::*;

/// Child Spaces: a Space makes and manages Spaces of its own.
#[cfg(all(feature = "server", feature = "spaces"))]
pub mod children;
#[cfg(feature = "test-fixtures")]
pub mod fixtures;
#[cfg(all(feature = "server", feature = "spaces"))]
mod host_svc;
#[cfg(feature = "server")]
mod passthrough;
#[cfg(all(feature = "server", feature = "spaces"))]
mod prewarm_pools;
#[cfg(feature = "server")]
pub mod server;
#[cfg(feature = "server")]
mod share_svc;
#[cfg(feature = "server")]
mod spaces_svc;

#[cfg(feature = "server")]
pub use passthrough::space_key;
