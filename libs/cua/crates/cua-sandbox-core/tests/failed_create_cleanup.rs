//! A failed local create deletes the instance it made (and its record)
//! unless `keep_on_failure` is set, whichever step failed: the runtime's own
//! start (its readiness wait), a readiness probe that never passes, a guest
//! that exits before it is ready, or a create that is cancelled. The runtime
//! is a fake that counts what it holds and every delete; nothing is booted.

use async_trait::async_trait;
use cua_sandbox_core::{
    CreateOptions, Error, InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime,
    LocalStartSpec, LocalSummary, Probe, ProviderKind, RuntimeError, RuntimeResult, Sandboxes,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PORT: u16 = 8080;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// `start` leaves the instance behind and returns an error, as a
    /// runtime whose own readiness wait timed out does.
    StartFails,
    /// `start` succeeds; the guest port never listens.
    NeverListens,
    /// `start` succeeds; the instance exits before the probe passes.
    Exits,
    /// `start` leaves the instance and then never returns.
    Hangs,
}

struct Fake {
    mode: Mode,
    live: Mutex<HashMap<String, InstanceStatus>>,
    deletes: Mutex<Vec<String>>,
    /// `delete` fails (the instance stays).
    delete_fails: bool,
}

impl Fake {
    fn new(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            live: Mutex::default(),
            deletes: Mutex::default(),
            delete_fails: false,
        })
    }

    fn failing_delete(mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            live: Mutex::default(),
            deletes: Mutex::default(),
            delete_fails: true,
        })
    }

    fn held(&self) -> usize {
        self.live.lock().unwrap().len()
    }

    fn delete_calls(&self) -> usize {
        self.deletes.lock().unwrap().len()
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
impl LocalRuntime for Fake {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        let status = match self.mode {
            Mode::Exits => InstanceStatus::Stopped,
            _ => InstanceStatus::Running,
        };
        self.live.lock().unwrap().insert(spec.name.clone(), status);
        match self.mode {
            Mode::StartFails => Err(RuntimeError::Other(format!(
                "[local] sandbox '{}' was not ready after 1s: probe Tcp {{ port: 9999 }} never passed",
                spec.name
            ))),
            Mode::Hangs => {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                Ok(Self::instance(&spec.name))
            }
            _ => Ok(Self::instance(&spec.name)),
        }
    }
    async fn guest_tcp_listening(&self, _: &str, _: u16) -> RuntimeResult<Option<bool>> {
        Ok(Some(false))
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
        self.live
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.deletes.lock().unwrap().push(name.into());
        if self.delete_fails {
            return Err(RuntimeError::Other("docker daemon went away".into()));
        }
        self.live.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(Self::instance("").endpoints)
    }
}

fn options(name: &str, keep: bool) -> CreateOptions {
    let mut o = CreateOptions::new(ProviderKind::Local, "docker.io/library/python:3.12-slim")
        .name(name)
        .wait_for(Probe::Tcp(9999));
    o.ports = vec![PORT];
    o.ready_timeout = Duration::from_millis(300);
    o.ready_timeout_given = true;
    o.keep_on_failure = keep;
    o
}

fn manager(rt: &Arc<Fake>, dir: &std::path::Path) -> Sandboxes {
    Sandboxes::builder()
        .local(rt.clone())
        .state_dir(dir)
        .build()
}

#[tokio::test]
async fn a_start_that_fails_its_readiness_wait_deletes_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::new(Mode::StartFails);
    let sbx = manager(&rt, dir.path());
    let err = sbx.create(options("t2", false)).await.err().unwrap();
    assert!(err.to_string().contains("was not ready"), "{err}");
    assert_eq!(rt.held(), 0, "the container was left running");
    assert_eq!(rt.delete_calls(), 1, "cleanup runs exactly once");
    assert!(
        sbx.get("t2").await.is_err(),
        "no record is left for `sb list`"
    );
}

#[tokio::test]
async fn a_readiness_timeout_deletes_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::new(Mode::NeverListens);
    let sbx = manager(&rt, dir.path());
    let err = sbx.create(options("t3", false)).await.err().unwrap();
    assert!(matches!(err, Error::Timeout(_)), "{err}");
    assert_eq!(rt.held(), 0);
    assert_eq!(rt.delete_calls(), 1, "cleanup runs exactly once");
    assert!(sbx.get("t3").await.is_err());
}

#[tokio::test]
async fn a_guest_that_exits_before_ready_deletes_what_it_made() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::new(Mode::Exits);
    let sbx = manager(&rt, dir.path());
    let err = sbx.create(options("t4", false)).await.err().unwrap();
    assert!(err.to_string().contains("exited"), "{err}");
    assert_eq!(rt.held(), 0);
    assert_eq!(rt.delete_calls(), 1, "cleanup runs exactly once");
}

#[tokio::test]
async fn keep_on_failure_keeps_a_failed_start_and_a_failed_readiness() {
    for mode in [Mode::StartFails, Mode::NeverListens] {
        let dir = tempfile::tempdir().unwrap();
        let rt = Fake::new(mode);
        let sbx = manager(&rt, dir.path());
        assert!(sbx.create(options("kept", true)).await.is_err());
        assert_eq!(rt.held(), 1, "the failed create is kept for debugging");
        assert_eq!(rt.delete_calls(), 0);
    }
}

#[tokio::test]
async fn a_failed_delete_does_not_mask_the_create_error() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::failing_delete(Mode::StartFails);
    let sbx = manager(&rt, dir.path());
    let err = sbx.create(options("t5", false)).await.err().unwrap();
    assert!(err.to_string().contains("was not ready"), "{err}");
    assert!(
        !err.to_string().contains("docker daemon went away"),
        "{err}"
    );
    assert_eq!(rt.delete_calls(), 1);
}

#[tokio::test]
async fn a_failed_create_never_deletes_a_name_that_was_already_in_use() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::new(Mode::StartFails);
    rt.live
        .lock()
        .unwrap()
        .insert("mine".into(), InstanceStatus::Running);
    let sbx = manager(&rt, dir.path());
    assert!(sbx.create(options("mine", false)).await.is_err());
    assert_eq!(
        rt.delete_calls(),
        0,
        "the earlier sandbox is not this create's"
    );
    assert_eq!(rt.held(), 1);
}

#[tokio::test]
async fn an_interrupted_create_deletes_what_it_made_once() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Fake::new(Mode::Hangs);
    let sbx = manager(&rt, dir.path());
    let cancel = cua_sandbox_core::CancellationToken::new();
    let stop = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        stop.cancel();
    });
    let err = sbx
        .create_cancellable(options("t6", false), cancel)
        .await
        .err()
        .unwrap();
    assert!(matches!(err, Error::Cancelled(_)), "{err}");
    assert_eq!(rt.held(), 0);
    assert_eq!(rt.delete_calls(), 1, "cleanup runs exactly once");
}

#[tokio::test]
async fn no_container_is_left_after_many_failed_creates() {
    let dir = tempfile::tempdir().unwrap();
    for mode in [Mode::StartFails, Mode::NeverListens, Mode::Exits] {
        let rt = Fake::new(mode);
        let sbx = manager(&rt, dir.path());
        for i in 0..8 {
            let _ = sbx.create(options(&format!("chaos-{i}"), false)).await;
        }
        assert_eq!(rt.held(), 0, "a failed create left a container behind");
        assert_eq!(rt.delete_calls(), 8, "one cleanup per failed create");
    }
}
