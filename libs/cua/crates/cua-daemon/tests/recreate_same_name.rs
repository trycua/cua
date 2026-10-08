//! A local Space deleted and created again under the same name must get a
//! fresh connection from the daemon runtime. The runtime caches a sandbox
//! handle and its cua-spacesd connection per `local:<name>`; the Spaces
//! delete paths (the `SpaceService` RPCs, the `delete_space` MCP tool, a
//! persistent agent's cleanup) go through `cua_spaces::Spaces::delete`, which
//! deletes by name in the sandbox manager and never saw those caches, so the
//! new Space (new port, new token) was reached through the old dead
//! connection until the daemon restarted ("Live video didn't start here").
//!
//! Fakes only: a loopback mock cua-spacesd per incarnation behind a local
//! runtime whose guest port 3211 maps to whichever mock is current.

use async_trait::async_trait;
use cua_daemon::{Error, Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::placement::On;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::SpaceCreate;
use cua_spaces::mcp::McpServer;
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};

const IMAGE: &str = "ghcr.io/trycua/cua-desktop-linux:docker-latest";

/// A local runtime whose guest ports map to loopback listeners; the
/// listener a new instance gets is whatever `ports` holds when it starts.
#[derive(Default)]
struct PortRuntime {
    ports: Mutex<BTreeMap<u16, u16>>,
    running: Mutex<BTreeSet<String>>,
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

/// Creates local Space `name` the way the daemon does (`Spaces::create`).
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

/// The port of the cua-spacesd connection the daemon hands out for
/// `local:<name>` (what an `OpenMediaBridge` or an env passthrough uses).
async fn env_port(d: &DaemonRuntime, name: &str) -> Result<u16, Error> {
    // `OpenMediaBridge` asks for the sandbox's record first.
    let id = format!("local:{name}");
    let _ = d.record(&id).await;
    Ok(d.env(&id, None).await?.client.endpoint().port())
}

#[tokio::test]
async fn a_space_deleted_and_created_again_gets_a_fresh_connection() {
    let f = fixture();
    let (first, second) = (
        MockServer::start(MockAuth::default()).await,
        MockServer::start(MockAuth::default()).await,
    );
    assert_ne!(first.addr.port(), second.addr.port());

    f.rt.point_at(&first);
    create(&f.daemon, "same").await;
    assert_eq!(
        env_port(&f.daemon, "same").await.unwrap(),
        first.addr.port()
    );

    // The Spaces delete path, as `cua spaces delete` and the app use it.
    f.daemon.spaces().delete("local:same").await.unwrap();
    f.rt.point_at(&second);
    create(&f.daemon, "same").await;
    assert_eq!(
        env_port(&f.daemon, "same").await.unwrap(),
        second.addr.port(),
        "the new Space is reached through its own cua-spacesd, not the deleted one's"
    );
}

#[tokio::test]
async fn the_delete_space_tool_drops_the_connection_too() {
    let f = fixture();
    let (first, second) = (
        MockServer::start(MockAuth::default()).await,
        MockServer::start(MockAuth::default()).await,
    );

    f.rt.point_at(&first);
    create(&f.daemon, "tool").await;
    assert_eq!(
        env_port(&f.daemon, "tool").await.unwrap(),
        first.addr.port()
    );

    // `cua mcp` / the agents' `delete_space` run `Spaces::delete` in the
    // daemon without touching the SpaceService.
    let out = McpServer::new(f.daemon.spaces().clone())
        .call("delete_space", serde_json::json!({"space": "local:tool"}))
        .await;
    assert!(!out.is_error, "{out:?}");

    f.rt.point_at(&second);
    create(&f.daemon, "tool").await;
    assert_eq!(
        env_port(&f.daemon, "tool").await.unwrap(),
        second.addr.port()
    );
}

#[tokio::test]
async fn a_sandbox_deleted_behind_the_daemon_is_not_reused() {
    let f = fixture();
    let (first, second) = (
        MockServer::start(MockAuth::default()).await,
        MockServer::start(MockAuth::default()).await,
    );

    f.rt.point_at(&first);
    create(&f.daemon, "away").await;
    assert_eq!(
        env_port(&f.daemon, "away").await.unwrap(),
        first.addr.port()
    );

    // Another process (the CLI without a daemon, a second runtime) deletes
    // the sandbox: this runtime's caches get no call at all.
    let other = cua_sandbox_core::Sandboxes::builder()
        .local(f.rt.clone())
        .state_dir(f.daemon.sandboxes().state().dir())
        .build();
    other.delete("away").await.unwrap();
    f.daemon.spaces().remove("local:away").await.unwrap();
    assert!(
        env_port(&f.daemon, "away").await.is_err(),
        "a deleted sandbox is not reachable through its old connection"
    );
    assert!(
        f.daemon
            .list(None)
            .await
            .unwrap()
            .iter()
            .all(|r| r.name != "away"),
        "and is not listed as running"
    );

    f.rt.point_at(&second);
    create(&f.daemon, "away").await;
    assert_eq!(
        env_port(&f.daemon, "away").await.unwrap(),
        second.addr.port()
    );
}

#[tokio::test]
async fn a_connection_is_kept_while_the_sandbox_stays_the_same() {
    let f = fixture();
    let only = MockServer::start(MockAuth::default()).await;
    f.rt.point_at(&only);
    create(&f.daemon, "kept").await;
    for _ in 0..3 {
        assert_eq!(env_port(&f.daemon, "kept").await.unwrap(), only.addr.port());
    }
    // A power change rewrites the state file's status, not the sandbox.
    f.daemon
        .sandboxes()
        .state()
        .set_status("kept", "running")
        .unwrap();
    assert_eq!(env_port(&f.daemon, "kept").await.unwrap(), only.addr.port());
    assert!(
        f.daemon
            .list(None)
            .await
            .unwrap()
            .iter()
            .any(|r| r.name == "kept")
    );
}

#[tokio::test]
async fn another_sandboxes_state_file_does_not_drop_a_live_connection() {
    let f = fixture();
    let only = MockServer::start(MockAuth::default()).await;
    f.rt.point_at(&only);
    create(&f.daemon, "shared").await;
    assert_eq!(
        env_port(&f.daemon, "shared").await.unwrap(),
        only.addr.port()
    );
    // A local and a cloud sandbox of one name share one state file: the
    // cloud claim's overwrites the local one's, but the local sandbox is
    // still running and still the same.
    f.daemon
        .sandboxes()
        .state()
        .save_fleet_claim("shared", "some-pool")
        .unwrap();
    assert_eq!(
        env_port(&f.daemon, "shared").await.unwrap(),
        only.addr.port()
    );
}
