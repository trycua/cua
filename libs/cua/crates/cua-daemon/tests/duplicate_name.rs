//! Creating a sandbox under a name that is taken is refused up front with
//! `AlreadyExists`, and nothing about the existing sandbox changes. It used to
//! start the old sandbox again, probe it with the new sandbox's token and report
//! the answer as "unauthenticated: missing or invalid bearer token".
//!
//! Fakes only: a loopback mock cua-spacesd behind a local runtime.

use async_trait::async_trait;
use cua_daemon::{CreateRequest, Error, Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

/// A local runtime whose guest ports map to loopback listeners; the
/// listener a new instance gets is whatever `ports` holds when it starts.
#[derive(Default)]
struct PortRuntime {
    ports: Mutex<BTreeMap<u16, u16>>,
    running: Mutex<BTreeSet<String>>,
    starts: std::sync::atomic::AtomicUsize,
}

impl PortRuntime {
    fn point_at(&self, spacesd: &MockServer) {
        *self.ports.lock().unwrap() = [(3211, spacesd.addr.port())].into();
    }

    fn endpoints(&self) -> LocalEndpoints {
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: self.ports.lock().unwrap().clone(),
            ..Default::default()
        }
    }
}

#[async_trait]
impl LocalRuntime for PortRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.running.lock().unwrap().insert(spec.name.clone());
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(name.into());
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(),
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().contains(name) {
            true => Ok(InstanceStatus::Running),
            false => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(self.endpoints())
    }
}

struct Fixture {
    daemon: DaemonRuntime,
    rt: Arc<PortRuntime>,
    // Keep the temp dirs alive.
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(PortRuntime::default());
    let daemon = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.path().join("sandboxes")),
        spaces_home: Some(dir.path().to_path_buf()),
        local: Some(rt.clone()),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    Fixture {
        daemon,
        rt,
        _dir: dir,
    }
}

fn request(name: &str) -> CreateRequest {
    CreateRequest {
        location: Some("local".into()),
        name: Some(name.into()),
        image: IMAGE.into(),
        ready_timeout: Some(Duration::from_secs(30)),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_taken_name_is_refused_before_anything_starts() {
    let f = fixture();
    let spacesd = MockServer::start(MockAuth::default()).await;
    f.rt.point_at(&spacesd);

    f.daemon.create(request("dup")).await.unwrap();
    let starts = f.rt.starts.load(std::sync::atomic::Ordering::SeqCst);

    let err = f.daemon.create(request("dup")).await.unwrap_err();
    assert_eq!(
        err,
        Error::AlreadyExists("a sandbox named 'dup' already exists".into())
    );
    // The variant survives the wire (the CLI talks to a daemon).
    assert_eq!(Error::from_status(&err.to_status()), err);
    assert_eq!(
        f.rt.starts.load(std::sync::atomic::Ordering::SeqCst),
        starts,
        "the second create started nothing"
    );
    assert!(
        f.rt.running.lock().unwrap().contains("dup"),
        "the first is untouched"
    );

    // Another name is fine, and so is the same name after a delete.
    f.daemon.create(request("other")).await.unwrap();
    f.daemon.delete("local:dup").await.unwrap();
    f.daemon.create(request("dup")).await.unwrap();
}
