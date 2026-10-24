//! A local VM Space that boots again in place (a resize of a running VM, a
//! sandbox-level restart or resume) may come back on new host ports. The
//! daemon's Space must then be reached at the new run: `cua_spaces::Spaces`
//! caches its own connection per Space (what the Space env passthrough, and
//! so an SDK's `Space.spacesd()`, goes through), and a resize left it
//! dialing the old run, so every call failed with a transport error until
//! the daemon restarted (`Sandbox.resize` "returns OK but the guest is
//! unreachable").
//!
//! Fakes only: a loopback mock cua-spacesd per run behind a local runtime
//! whose guest port 3211 maps to whichever mock is current.

use async_trait::async_trait;
use cua_daemon::{Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    Resize, RuntimeError,
};
use cua_spaces::SpaceCreate;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

const IMAGE: &str = "ghcr.io/trycua/linux:24.04";

/// A local runtime whose guest ports map to loopback listeners; a run
/// (start, resume, resize) gets whatever `ports` holds when it starts.
#[derive(Default)]
struct PortRuntime {
    ports: Mutex<BTreeMap<u16, u16>>,
    running: Mutex<BTreeSet<String>>,
    resized: Mutex<Vec<Resize>>,
}

impl PortRuntime {
    fn point_at(&self, spacesd: &MockServer) {
        *self.ports.lock().unwrap() = [(3211, spacesd.addr.port())].into();
    }

    fn run(&self, name: &str) -> LocalInstance {
        self.running.lock().unwrap().insert(name.into());
        LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ports: self.ports.lock().unwrap().clone(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl LocalRuntime for PortRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        Ok(self.run(&spec.name))
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(self.run(name))
    }
    /// As QEMU's: the running VM stops and boots again from its disk.
    async fn resize(&self, name: &str, shape: &Resize) -> Result<LocalInstance, RuntimeError> {
        self.resized.lock().unwrap().push(shape.clone());
        self.running.lock().unwrap().remove(name);
        Ok(self.run(name))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().contains(name) {
            true => Ok(InstanceStatus::Running),
            false => Ok(InstanceStatus::Stopped),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn endpoints(&self, _: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: self.ports.lock().unwrap().clone(),
            ..Default::default()
        })
    }
}

struct Fixture {
    daemon: DaemonRuntime,
    rt: Arc<PortRuntime>,
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

async fn create(d: &DaemonRuntime, name: &str) {
    d.spaces()
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some(IMAGE.into()),
            spacesd: Some(true),
            name: Some(name.into()),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
}

/// The port of the connection the daemon's Space `local:<name>` holds:
/// what its env passthrough (an SDK's `Space.spacesd()`) dials.
async fn space_port(d: &DaemonRuntime, name: &str) -> u16 {
    d.spaces()
        .space(&format!("local:{name}"))
        .await
        .unwrap()
        .spacesd()
        .unwrap()
        .endpoint()
        .port()
}

#[tokio::test]
async fn a_resized_space_is_reached_at_its_new_run() {
    let f = fixture();
    let (old, new) = (
        MockServer::start(MockAuth::default()).await,
        MockServer::start(MockAuth::default()).await,
    );
    f.rt.point_at(&old);
    create(&f.daemon, "vm").await;
    // Connected before the resize, as a caller holding the Space is.
    assert_eq!(space_port(&f.daemon, "vm").await, old.addr.port());

    f.rt.point_at(&new);
    let shape = Resize {
        cpus: Some(3),
        memory_mb: Some(3072),
        ..Default::default()
    };
    f.daemon.resize("vm", &shape).await.unwrap();
    assert_eq!(f.rt.resized.lock().unwrap().as_slice(), [shape]);
    assert_eq!(
        space_port(&f.daemon, "vm").await,
        new.addr.port(),
        "the Space is reached through its new run, not the one the resize stopped"
    );
}

#[tokio::test]
async fn a_space_restarted_at_the_sandbox_level_is_reached_at_its_new_run() {
    let f = fixture();
    let (old, new) = (
        MockServer::start(MockAuth::default()).await,
        MockServer::start(MockAuth::default()).await,
    );
    f.rt.point_at(&old);
    create(&f.daemon, "mv").await;
    assert_eq!(space_port(&f.daemon, "mv").await, old.addr.port());

    // A microVM picks new host ports on every boot.
    f.rt.point_at(&new);
    f.daemon.restart("mv").await.unwrap();
    assert_eq!(space_port(&f.daemon, "mv").await, new.addr.port());
}
