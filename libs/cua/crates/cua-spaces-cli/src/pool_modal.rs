// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Modal as overflow capacity of your machines (when you connected Modal on
//! the machine that coordinates, see `pool_ext`): a host named `modal` that
//! lives in the coordinator's process. Its job
//! runner is the same as a machine's ([`cua_cluster::NodeJobs`]: ledger,
//! fencing), and each job runs in a fresh Modal sandbox (gVisor) created
//! and deleted through the contrib Modal provider (Modal's own Go SDK in
//! `cua-modal-helper`), reached over the sandbox's TLS tunnel. The spend
//! guard is its job limit: at most N sandboxes at once. Every sandbox also
//! ends itself after an hour (the provider's lifetime backstop), so a
//! coordinator that dies leaves nothing running for long.
//!
//! Placement puts jobs here only when no machine has room, or when a run
//! names it (`cua sb run --on modal`).

use std::path::Path;
#[cfg(feature = "contrib-modal")]
use std::sync::{Arc, OnceLock};

use cua_sdk::CuaError;
use cua_spaces_cluster::coordinator::LinkFactory;
use cua_spaces_cluster::coordinator::store::NodeRow;

/// The name of the Modal capacity among your machines.
pub const NAME: &str = "modal";

/// Its row.
pub fn row(max: u32) -> NodeRow {
    NodeRow {
        name: NAME.into(),
        via: "provider".into(),
        provider: NAME.into(),
        state: "ready".into(),
        os: "linux".into(),
        max_jobs: max,
        added_ms: cua_cluster::model::now_ms(),
        ..Default::default()
    }
}

/// The link factory that answers for the Modal row (once per process: one
/// job runner owns its ledger).
#[cfg(feature = "contrib-modal")]
pub fn factory(dir: &Path, max: u32) -> Result<LinkFactory, CuaError> {
    let env = std::env::var("MODAL_ENVIRONMENT").unwrap_or_default();
    if env.trim().is_empty() {
        return Err(CuaError::InvalidArgument(
            "Modal overflow needs MODAL_ENVIRONMENT: the Modal environment your machines may \
             create sandboxes in (never your default one by accident)"
                .into(),
        ));
    }
    for v in ["MODAL_TOKEN_ID", "MODAL_TOKEN_SECRET"] {
        if std::env::var(v).map_or(true, |t| t.trim().is_empty()) {
            return Err(CuaError::InvalidArgument(format!(
                "Modal overflow needs {v} (a Modal token for that environment)"
            )));
        }
    }
    let dir = dir.join("providers").join(NAME);
    let link: Arc<OnceLock<Arc<dyn cua_cluster::link::NodeLink>>> = Arc::default();
    Ok(Arc::new(move |r: &NodeRow| {
        if r.provider != NAME {
            return None;
        }
        if let Some(l) = link.get() {
            return Some(l.clone());
        }
        match open(&dir, max) {
            Ok(l) => Some(link.get_or_init(|| l).clone()),
            Err(e) => {
                tracing::warn!("the Modal capacity did not open: {e}");
                None
            }
        }
    }))
}

#[cfg(feature = "contrib-modal")]
fn open(dir: &Path, max: u32) -> std::io::Result<Arc<dyn cua_cluster::link::NodeLink>> {
    use cua_cluster::node::{NodeConfig, NodeJobs};
    let mut sandboxes = cua_sandbox_core::Sandboxes::builder().state_dir(dir.join("sandboxes"));
    for p in cua_contrib::providers() {
        sandboxes = sandboxes.provider(p);
    }
    let exec = ModalExecutor {
        sandboxes: sandboxes.build(),
    };
    let cfg = NodeConfig {
        dir: dir.join("jobs"),
        // The limit is the sandbox count; CPU and memory are Modal's.
        millicpus_total: max.saturating_mul(16_000),
        memory_total_mb: u64::from(max) * 64 * 1024,
        jobs_max: max,
        min_available_mb: 0,
        retention: std::time::Duration::from_secs(24 * 3600),
        output_retention: std::time::Duration::from_secs(3600),
        boot_id: format!("{:016x}", rand::random::<u64>()),
    };
    let jobs = NodeJobs::open(cfg, Arc::new(exec))?;
    Ok(Arc::new(cua_cluster::link::LocalLink::new(
        jobs, NAME, "linux",
    )))
}

/// Each job in a fresh Modal sandbox, through the sandbox layer's contrib
/// Modal provider (Modal sandboxes are sandboxes, not Spaces this machine
/// keeps): create (gVisor, the job's CPU and memory), run the command
/// through the sandbox's cua-spacesd over its TLS tunnel, delete.
#[cfg(feature = "contrib-modal")]
struct ModalExecutor {
    sandboxes: cua_sandbox_core::Sandboxes,
}

#[cfg(feature = "contrib-modal")]
#[async_trait::async_trait]
impl cua_cluster::node::JobExecutor for ModalExecutor {
    async fn run(
        &self,
        job: &cua_cluster::JobSpec,
        space: &str,
        cancel: cua_cluster::CancellationToken,
    ) -> cua_cluster::node::ExecOutcome {
        use cua_cluster::node::{ExecEnd, ExecOutcome};
        use cua_sandbox_core::{CreateOptions, ProviderKind};
        let mut out = ExecOutcome::ended(ExecEnd::Exited(0));
        let image = if job.image.trim().is_empty() {
            default_image()
        } else {
            job.image.clone()
        };
        let mut opts = CreateOptions::new(ProviderKind::Contrib, image);
        opts.contrib = Some(NAME.into());
        opts.cpus = job.millicpus.div_ceil(1000).max(1);
        opts.memory_mb = job.memory_mb;
        opts.ready_timeout = std::time::Duration::from_secs(300);
        let opts = opts.name(space);
        let t0 = std::time::Instant::now();
        let created = tokio::select! {
            r = self.sandboxes.create(opts) => r,
            _ = cancel.cancelled() => {
                let _ = self.sandboxes.delete(space).await;
                out.end = ExecEnd::Canceled;
                return out;
            }
        };
        out.create_ms = t0.elapsed().as_millis() as u32;
        let sb = match created {
            Ok(sb) => sb,
            Err(e) => {
                let _ = self.sandboxes.delete(space).await;
                out.end = ExecEnd::Error(format!("the Modal sandbox did not start: {e}"));
                return out;
            }
        };
        let t1 = std::time::Instant::now();
        match sb.spacesd().await {
            Ok(client) => {
                out.end = cua_spaces::host_jobs::run_command(&client, job, &cancel, &mut out).await;
                if !job.result_path.is_empty() && matches!(out.end, ExecEnd::Exited(_)) {
                    cua_spaces::host_jobs::read_result(&client, job, &mut out).await;
                }
            }
            Err(e) => {
                out.end = ExecEnd::Error(format!("the Modal sandbox's cua-spacesd: {e}"));
            }
        }
        out.run_ms = t1.elapsed().as_millis() as u32;
        let t2 = std::time::Instant::now();
        if let Err(e) = self.sandboxes.delete(space).await {
            tracing::warn!(sandbox = %space, "a Modal sandbox was not deleted (it ends itself within the hour): {e}");
        }
        out.delete_ms = t2.elapsed().as_millis() as u32;
        out
    }

    async fn remove_space(&self, space: &str) {
        let _ = self.sandboxes.delete(space).await;
    }

    fn offers(&self) -> cua_cluster::node::Offers {
        cua_cluster::node::Offers {
            kinds: vec!["container".into()],
            runtimes: vec!["gvisor".into()],
        }
    }
}

/// The canonical Linux image.
#[cfg(feature = "contrib-modal")]
fn default_image() -> String {
    "ghcr.io/trycua/linux:24.04-slim".into()
}

/// This build has no Modal provider.
#[cfg(not(feature = "contrib-modal"))]
pub fn factory(_dir: &Path, _max: u32) -> Result<LinkFactory, CuaError> {
    Err(CuaError::Unsupported(
        "this cua is built without the Modal provider (feature contrib-modal)".into(),
    ))
}
