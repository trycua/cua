//! Batch work on all of your machines, the MIT parts.
//!
//! Every machine signed in to your account that provides Spaces (Macs,
//! Linux boxes, cloud VMs in your own account), reached through the relay
//! or its mesh, or directly (Tailscale, LAN, VPN), runs jobs. Whichever of
//! your machines coordinates them (the source-available
//! `cua-spaces-cluster` crate: no one sets one up) keeps a durable job queue
//! and places jobs on them. This crate is what machines need (MIT):
//!
//! - [`node`]: the host half. [`node::NodeJobs`] runs each job in a fresh
//!   Space through a [`node::JobExecutor`], with a durable
//!   [ledger](ledger) keyed by job id (idempotent delivery) and attempt
//!   fencing. The host's daemon serves it as `HostJobsService`. Every job
//!   member runs in a Space: a fresh one per attempt (a macOS VM, a
//!   container) or a dedicated host's own ([`machine`]).
//! - [`jobdir`]: a member's directories on its host (its work directory with
//!   the bundle, the run's data directory, the caches), which its Space
//!   sees through mounts, and its live output log.
//! - [`member`]: running a member's command through its Space's
//!   cua-spacesd, its output going to the log as it arrives.
//! - [`peers`]: how a gang member reaches its peers (the Cua mesh, or
//!   direct on a dedicated network).
//! - [`placement`]: in-memory scoring (spread or pack) over what each host
//!   reports ([`cua_proto::env::v1::HostNodeStatus`]).
//! - [`link`]: how a coordinator reaches a host ([`link::NodeLink`]): one
//!   connection per host, direct or relayed (and failing over between the
//!   two); another transport (a mesh) is another implementation.
//!
//! Semantics: a job's command runs at least once (a lost host's jobs run
//! again elsewhere); its completion is recorded exactly once (the first
//! terminal result of its current attempt); a submit is idempotent per key.

pub mod batch;
pub mod elastic;
pub mod gang_mesh;
pub mod jobdir;
pub mod ledger;
pub mod link;
pub mod machine;
pub mod member;
pub mod model;
pub mod node;
pub mod peers;
pub mod placement;
pub mod sysinfo;

pub use model::{HostJobState, JobSpec};
pub use node::{ExecEnd, ExecOutcome, External, JobExecutor, NodeConfig, NodeJobs, Offers};
pub use placement::Policy;
/// How a job's run is canceled ([`JobExecutor::run`]).
pub use tokio_util::sync::CancellationToken;
