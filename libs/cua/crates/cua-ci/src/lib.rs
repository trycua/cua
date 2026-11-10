//! Cua CI: GitHub Actions jobs on Cua Spaces.
//!
//! A pool ([`config::PoolConfig`]) keeps a number of ephemeral GitHub
//! Actions runners registered for an organization or a repository. Each
//! runner lives in a Space of its own: the pool creates the Space from an
//! image (a copy-on-write clone of a macOS VM on Lume, a container or a VM
//! on Linux), starts the actions runner in it with a just-in-time (JIT)
//! configuration, lets it take exactly one job, then deletes the Space. No
//! Space is ever reused, and the GitHub App key that registers runners
//! never enters one.
//!
//! - [`github`]: the GitHub REST client (GitHub App JWT and installation
//!   token, or a plain token), JIT configs, runners, runner groups and the
//!   `actions/runner` releases.
//! - [`pool`]: the reconciler. It keeps `warm` idle runners up to
//!   `max_runners`, notices finished, crashed and timed-out runners, and
//!   removes orphans both ways (Spaces with no runner, runners with no
//!   Space). Spaces and GitHub are behind the [`spaces::Spaces`] and
//!   [`github::RunnerApi`] seams.
//! - [`guest`]: what runs in the Space: install the runner from the host's
//!   tarball cache when the image lacks it, the launch wrapper, status and
//!   log copy-out.
//! - [`egress`]: the default egress policy (private, carrier-grade NAT,
//!   link-local and unique-local ranges denied) and the guest's resolver.
//! - [`events`]: the JSONL event log and the status summary.
//! - [`secret`]: the credential store (the macOS login keychain, or a
//!   0600 file elsewhere).
//! - [`perf`]: the performance core: the warm pool, forks of a golden
//!   image, read-only dependency caches and a repo mirror, the phase
//!   events the bench reads, and the hosted job runner built on them.
//! - [`service`]: the launchd agent or systemd user unit that runs
//!   `cua ci serve`.
//!
//! - [`service_api`], [`hosted`], [`job_runner`]: the hosted Cua CI service.
//!   Wire types and validation of the contract, the HTTP client, the
//!   agent that registers this computer and claims jobs, and the runner
//!   that gives each job a fresh Space.
//! - [`workflow_pr`]: `cua ci connect --open-pr`.
//!
//! The `cua ci` command (`cua-cli`) is a thin layer over this crate.

// Tests build their configs by field, which reads clearer than struct update.
#![cfg_attr(test, allow(clippy::field_reassign_with_default))]

pub mod cache;
pub mod config;
pub mod egress;
pub mod events;
pub mod github;
pub mod guest;
pub mod hosted;
pub mod job_runner;
pub mod overview;
pub mod paths;
pub mod perf;
pub mod pool;
pub mod secret;
pub mod service;
pub mod service_api;
pub mod spaces;
pub mod targets;
pub mod workflow_pr;

#[cfg(any(test, feature = "testing"))]
pub mod testing;

pub use config::{EgressMode, GuestOs, PoolConfig, Scope};
pub use events::{Event, EventLog};
pub use github::{Credentials, GitHubClient, RunnerApi};
pub use paths::CiPaths;
pub use perf::PerfConfig;
pub use perf::runtime::{JitSource, JobClaim, PerfRuntime, Released};
pub use pool::{Capacity, ClaimError, Claimed, Clock, Pool, PoolDeps, PoolStatus, SystemClock};
pub use secret::{Secret, SecretStore};
pub use spaces::{SpaceRow, SpaceSpec, Spaces, SpacesError};

/// Errors of this crate. Messages never carry a secret: tokens, the App
/// key and JIT configs are [`Secret`]s, which print as `[redacted]`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A pool configuration that cannot work (the message says which
    /// field and why).
    #[error("{0}")]
    Config(String),
    /// GitHub refused or failed a call.
    #[error("GitHub: {0}")]
    GitHub(String),
    /// A Spaces call failed.
    #[error("Spaces: {0}")]
    Spaces(#[from] SpacesError),
    /// The credential store failed.
    #[error("credential store: {0}")]
    Secret(String),
    /// Something in the guest failed (install, launch).
    #[error("in the Space: {0}")]
    Guest(String),
    /// A file under the runner home.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// The hosted Cua CI service refused or failed a call.
    #[error("{0}")]
    Service(#[from] service_api::ApiError),
}

/// Result alias.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Milliseconds since the Unix epoch.
pub fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
