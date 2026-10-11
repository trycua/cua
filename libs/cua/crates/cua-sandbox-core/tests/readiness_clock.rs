//! The readiness budget of a create is counted from when the image is
//! present: a pull longer than the whole budget does not fail a guest that
//! then comes up in time, and a guest that does not come up in time still
//! fails. The runtime is a fake that reports its phases and says when its
//! guest port listens; nothing is pulled or booted.

use async_trait::async_trait;
use cua_sandbox_core::progress::{Phase, Progress, report};
use cua_sandbox_core::{
    CreateOptions, Error, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime,
    LocalStartSpec, LocalSummary, Probe, ProviderKind, RuntimeError, RuntimeResult, Sandboxes,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const PORT: u16 = 8080;

/// "Pulls" for `pull`, then its guest port listens `boot` after the start
/// returns.
struct SlowPull {
    pull: Duration,
    boot: Duration,
    listening_at: Mutex<HashMap<String, Instant>>,
}

impl SlowPull {
    fn new(pull: Duration, boot: Duration) -> Arc<Self> {
        Arc::new(Self {
            pull,
            boot,
            listening_at: Mutex::default(),
        })
    }

    fn instance(name: &str) -> LocalInstance {
        LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ports: [(PORT, PORT)].into(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl LocalRuntime for SlowPull {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        report(Progress::phase(Phase::Pulling));
        tokio::time::sleep(self.pull).await;
        report(Progress::phase(Phase::Creating));
        report(Progress::phase(Phase::Booting));
        self.listening_at
            .lock()
            .unwrap()
            .insert(spec.name.clone(), Instant::now() + self.boot);
        Ok(Self::instance(&spec.name))
    }
    async fn guest_tcp_listening(&self, name: &str, port: u16) -> RuntimeResult<Option<bool>> {
        let at = self.listening_at.lock().unwrap().get(name).copied();
        Ok(Some(
            port == PORT && at.is_some_and(|at| Instant::now() >= at),
        ))
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(Self::instance(name))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        if self.listening_at.lock().unwrap().contains_key(name) {
            Ok(InstanceStatus::Running)
        } else {
            Err(RuntimeError::NotFound(name.into()))
        }
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.listening_at.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(Self::instance("").endpoints)
    }
}

fn options(budget: Duration) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Local, "docker.io/library/python:3.12-slim")
        .name("readiness-clock")
        .wait_for(Probe::Tcp(PORT));
    o.ports = vec![PORT];
    o.ready_timeout = budget;
    o.ready_timeout_given = true;
    o
}

#[tokio::test]
async fn a_pull_longer_than_the_budget_does_not_count_against_it() {
    let dir = tempfile::tempdir().unwrap();
    // The pull takes twice the budget; the guest then listens well within it.
    let rt = SlowPull::new(Duration::from_millis(1600), Duration::from_millis(300));
    let sbx = Sandboxes::builder().local(rt).state_dir(dir.path()).build();
    let sandbox = sbx
        .create(options(Duration::from_millis(800)))
        .await
        .expect("the guest came up within the budget once the image was present");
    assert_eq!(sandbox.name(), "readiness-clock");
}

#[tokio::test]
async fn a_guest_that_does_not_come_up_in_the_budget_still_fails() {
    let dir = tempfile::tempdir().unwrap();
    let rt = SlowPull::new(Duration::from_millis(50), Duration::from_secs(30));
    let sbx = Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir.path())
        .build();
    let started = Instant::now();
    let err = sbx
        .create(options(Duration::from_millis(600)))
        .await
        .expect_err("the port never listened within the budget");
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "bounded by the budget"
    );
}
