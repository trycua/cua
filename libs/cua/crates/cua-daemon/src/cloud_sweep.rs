// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The automatic cloud sweep: every `CUA_CLOUD_SWEEP_SECS` seconds
//! (default 900; `0` turns it off) the daemon runs one pass of
//! [`CloudManager::auto_sweep`] (the logic lives in `cua-byoc`): the
//! deletions `cua cloud sweep --delete` makes without `--all`, in every
//! connected cloud. The first pass is a minute after start.

use std::sync::Arc;
use std::time::Duration;

use cua_byoc::autosweep;
use cua_sandbox_core::byoc::CloudManager;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::runtime::Runtime;

/// Schedules the sweep for `runtime`'s clouds (nothing when this build has
/// none, or the environment turns it off).
pub fn spawn(runtime: &Runtime, shutdown: watch::Receiver<bool>) -> Option<JoinHandle<()>> {
    let clouds = runtime.sandboxes().clouds().ok()?.clone();
    spawn_with(
        clouds,
        autosweep::interval_from_env(),
        autosweep::FIRST_RUN_DELAY,
        shutdown,
    )
}

/// [`spawn`] with an explicit `interval` (`None`: off) and first delay.
pub fn spawn_with(
    clouds: Arc<dyn CloudManager>,
    interval: Option<Duration>,
    first: Duration,
    mut shutdown: watch::Receiver<bool>,
) -> Option<JoinHandle<()>> {
    let every = interval?;
    Some(tokio::spawn(async move {
        let mut wait = first;
        loop {
            tokio::select! {
                _ = shutdown.changed() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            match autosweep::run_once(clouds.as_ref()).await {
                Ok((_, Some(line))) => tracing::info!("{line}"),
                Ok((_, None)) => {}
                Err(e) => tracing::warn!("cloud auto-sweep failed: {e}"),
            }
            wait = every;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cua_sandbox_core::byoc::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    #[derive(Default)]
    struct Counting(AtomicUsize);

    #[async_trait::async_trait]
    impl CloudManager for Counting {
        async fn status(&self, _: Option<&str>) -> cua_sandbox_core::Result<CloudStatusReport> {
            unreachable!()
        }
        async fn test(&self, _: &CloudTarget) -> cua_sandbox_core::Result<CloudTestReport> {
            unreachable!()
        }
        async fn connect(
            &self,
            _: &CloudTarget,
            _: bool,
            _: Option<u32>,
        ) -> cua_sandbox_core::Result<CloudConnected> {
            unreachable!()
        }
        async fn disconnect(
            &self,
            _: &str,
            _: bool,
        ) -> cua_sandbox_core::Result<CloudDisconnected> {
            unreachable!()
        }
        async fn sweep(
            &self,
            _: Option<&str>,
            _: bool,
            _: bool,
        ) -> cua_sandbox_core::Result<CloudSweepReport> {
            unreachable!()
        }
        async fn auto_sweep(&self) -> cua_sandbox_core::Result<CloudSweepReport> {
            self.0.fetch_add(1, SeqCst);
            Ok(CloudSweepReport::default())
        }
        async fn audit(&self, _: usize) -> cua_sandbox_core::Result<Vec<CloudAuditEntry>> {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn it_sweeps_on_a_schedule_until_shutdown_and_not_at_all_when_off() {
        let (tx, rx) = watch::channel(false);
        let m = Arc::new(Counting::default());
        assert!(spawn_with(m.clone(), None, Duration::ZERO, rx.clone()).is_none());
        let task = spawn_with(
            m.clone(),
            Some(Duration::from_millis(10)),
            Duration::ZERO,
            rx,
        )
        .unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while m.0.load(SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("three passes");
        tx.send(true).unwrap();
        task.await.unwrap();
        let after = m.0.load(SeqCst);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(m.0.load(SeqCst), after, "stopped at shutdown");
    }
}
