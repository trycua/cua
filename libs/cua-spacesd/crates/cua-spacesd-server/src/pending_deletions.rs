// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Deleting the Spaces this computer's owner deleted while it was offline.
//!
//! Removing a computer from the account with "Also delete its Spaces"
//! (`DELETE /v1/machines/{id}?spaces=delete`) works while the computer is
//! offline: the relay forgets the Spaces at once and keeps a pending
//! deletion for the computer ([`cua_relay::pending_deletions`]). Every time
//! this host dials the relay ([`cua_relay::client::JoinConfig::dialing`]),
//! it reads that list with its machine token, deletes each listed Space
//! through its cua daemon (`DeleteHostSpace`, else `DeleteCloudSpace`: the
//! sandbox, its files and, for a cloud Space, its cloud resources) as this
//! machine's owner, and acks each one.
//!
//! - It deletes only Spaces on the list, by their relay id.
//! - A Space that is not here any more counts as deleted and is acked.
//! - A delete that fails is not acked: the next sweep tries again.
//! - A computer added again (`cua host setup` after a removal) keeps its
//!   pending deletions, and the relay refuses to register those Spaces again
//!   until they are acked, so they never come back (deleted means deleted).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cua_proto::env::v1::host_spaces_service_client::HostSpacesServiceClient;
use cua_proto::env::v1::{DeleteCloudSpaceRequest, DeleteHostSpaceRequest};
use cua_relay::client::{ack_pending_deletion, pending_deletions, PendingError};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tonic::{Code, Request};

use crate::host_spaces::{daemon_channel, read_policy};

/// How long one Space's delete may take (a VM's disk can be large).
const DELETE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Deletes one Space on this machine.
#[tonic::async_trait]
pub trait SpaceDeleter: Send + Sync {
    /// Deletes `space` (a relay machine id). `Ok(true)`: deleted;
    /// `Ok(false)`: not on this machine (nothing to delete); `Err`: not
    /// deleted, try again later.
    async fn delete(&self, space: &str) -> Result<bool, String>;
}

/// Deletes through the host's cua daemon named in the host policy.
pub struct DaemonDeleter {
    policy_file: PathBuf,
}

impl DaemonDeleter {
    /// A deleter reading the host policy at `policy_file` on every call.
    pub fn new(policy_file: impl Into<PathBuf>) -> Self {
        Self {
            policy_file: policy_file.into(),
        }
    }
}

#[tonic::async_trait]
impl SpaceDeleter for DaemonDeleter {
    async fn delete(&self, space: &str) -> Result<bool, String> {
        let policy = read_policy(&self.policy_file).map_err(|s| s.message().to_owned())?;
        let daemon = policy
            .spaces_daemon
            .ok_or("this host's policy names no cua daemon; run `cua host setup` on it again")?;
        let channel = daemon_channel(&daemon)
            .await
            .map_err(|s| s.message().to_owned())?;
        // No caller metadata: the call is this machine's own (its owner's).
        let mut client = HostSpacesServiceClient::new(channel);
        let mut req = Request::new(DeleteHostSpaceRequest {
            space: space.to_owned(),
        });
        req.set_timeout(DELETE_TIMEOUT);
        match client.delete_host_space(req).await {
            Ok(_) => return Ok(true),
            Err(s) if s.code() == Code::NotFound => {}
            Err(s) => return Err(s.message().to_owned()),
        }
        // Not a Space this host provides: maybe one in the owner's cloud
        // that this computer created.
        let mut req = Request::new(DeleteCloudSpaceRequest {
            space: space.to_owned(),
        });
        req.set_timeout(DELETE_TIMEOUT);
        match client.delete_cloud_space(req).await {
            Ok(_) => Ok(true),
            Err(s) if s.code() == Code::NotFound => Ok(false),
            Err(s) => Err(s.message().to_owned()),
        }
    }
}

/// What one sweep did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Deleted here and acked.
    pub deleted: Vec<String>,
    /// Not on this machine (already gone) and acked.
    pub absent: Vec<String>,
    /// Not deleted or not acked, with why: the next sweep tries again.
    pub failed: Vec<(String, String)>,
}

/// Reads this machine's pending deletions, deletes each listed Space with
/// `deleter` and acks it. Fails only when the list cannot be read.
pub async fn sweep(
    relay_url: &str,
    machine_id: &str,
    token: &str,
    deleter: &dyn SpaceDeleter,
) -> Result<SweepReport, PendingError> {
    let mut report = SweepReport::default();
    for space in pending_deletions(relay_url, machine_id, token).await? {
        // Only a plain relay id names a Space here; anything else is never
        // passed on (it could name something that is not on the list).
        if !cua_relay::valid_machine_id(&space.id) {
            report
                .failed
                .push((space.id.clone(), "not a Space id".into()));
            continue;
        }
        let here = match deleter.delete(&space.id).await {
            Ok(here) => here,
            Err(e) => {
                tracing::warn!(space = %space.id, name = %space.name, error = %e, "pending Space deletion failed; retrying later");
                report.failed.push((space.id, e));
                continue;
            }
        };
        if let Err(e) = ack_pending_deletion(relay_url, machine_id, token, &space.id).await {
            report.failed.push((space.id, format!("ack: {e}")));
            continue;
        }
        tracing::info!(space = %space.id, name = %space.name, here, "deleted a Space its owner deleted while this computer was removed");
        if here {
            report.deleted.push(space.id);
        } else {
            report.absent.push(space.id);
        }
    }
    Ok(report)
}

/// How the background sweeper runs.
pub struct PendingDeletionsConfig {
    /// The relay (`ws(s)://` or `http(s)://`).
    pub relay_url: String,
    /// This machine's id.
    pub machine_id: String,
    /// The machine token.
    pub relay_token: String,
    /// Re-read before every sweep when readable and non-empty (setup
    /// rotates it under a running service).
    pub relay_token_file: Option<PathBuf>,
    /// Notified on every relay dial ([`cua_relay::client::JoinConfig::dialing`]).
    pub dialing: Arc<tokio::sync::Notify>,
    /// Least time between two sweeps.
    pub min_interval: Duration,
    /// After a sweep with failures: try again this soon.
    pub retry_interval: Duration,
    /// After a clean sweep: look again this often even without a dial.
    pub idle_interval: Duration,
    /// After the relay did not know the token (nothing pending for a
    /// removed computer): ask again this late, unless the token changes.
    pub unknown_interval: Duration,
}

impl PendingDeletionsConfig {
    /// Defaults for everything but the relay, id, token and dial signal.
    pub fn new(
        relay_url: String,
        machine_id: String,
        relay_token: String,
        dialing: Arc<tokio::sync::Notify>,
    ) -> Self {
        Self {
            relay_url,
            machine_id,
            relay_token,
            relay_token_file: None,
            dialing,
            min_interval: Duration::from_secs(60),
            retry_interval: Duration::from_secs(5 * 60),
            idle_interval: Duration::from_secs(6 * 60 * 60),
            unknown_interval: Duration::from_secs(60 * 60),
        }
    }

    fn token(&self) -> String {
        self.relay_token_file
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| self.relay_token.clone())
    }
}

/// Sweeps at start and on each relay dial (at most once per
/// `min_interval`), until `shutdown`.
pub async fn run(
    config: PendingDeletionsConfig,
    deleter: Arc<dyn SpaceDeleter>,
    shutdown: CancellationToken,
) {
    let mut due = Instant::now();
    let mut not_before = Instant::now();
    // The token the relay last did not know: a new one is asked at once.
    let mut unknown: Option<String> = None;
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep_until(due) => {}
            _ = config.dialing.notified() => {
                if unknown.as_deref().is_some_and(|t| t != config.token()) {
                    unknown = None;
                    not_before = Instant::now();
                }
                if unknown.is_none() {
                    due = due.min(not_before.max(Instant::now()));
                }
                continue;
            }
        }
        let token = config.token();
        let started = Instant::now();
        not_before = started + config.min_interval;
        due = match sweep(&config.relay_url, &config.machine_id, &token, &*deleter).await {
            Ok(r) => {
                unknown = None;
                if !r.deleted.is_empty() || !r.absent.is_empty() || !r.failed.is_empty() {
                    tracing::info!(
                        deleted = r.deleted.len(),
                        absent = r.absent.len(),
                        failed = r.failed.len(),
                        "pending Space deletions swept"
                    );
                }
                if r.failed.is_empty() {
                    started + config.idle_interval
                } else {
                    started + config.retry_interval.max(config.min_interval)
                }
            }
            Err(PendingError::Unknown(e)) => {
                tracing::debug!(error = %e, "no pending Space deletions for this token");
                unknown = Some(token);
                not_before = started + config.unknown_interval;
                not_before
            }
            Err(PendingError::Failed(e)) => {
                tracing::info!(error = %e, "pending Space deletions not read; retrying later");
                started + config.retry_interval.max(config.min_interval)
            }
        };
    }
}
