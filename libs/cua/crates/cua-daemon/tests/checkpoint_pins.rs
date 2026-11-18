//! Checkpoint pins and prune through the daemon's gRPC API (server and
//! `DaemonClient` in process): `CheckpointSandbox.pin`, `PinCheckpoint`,
//! `PruneCheckpoints`, `Checkpoint.pinned` / `.shared_memory`, and a pinned
//! delete refused. Fakes only (temp dirs, a fake local runtime, no host
//! processes). `CreateSandboxRequest.forkable` is covered in `convert.rs`.

use async_trait::async_trait;
use cua_daemon::client::{DaemonAddress, DaemonClient};
use cua_daemon::server::{ServerConfig, start};
use cua_daemon::{Error, Runtime as DaemonRuntime, RuntimeConfig};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, RuntimeError, RuntimeResult, SandboxState,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

/// One running QEMU sandbox whose checkpoints share memory; logs pins.
#[derive(Default)]
struct Runtime {
    calls: Mutex<Vec<String>>,
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "qemu".into()
    }

    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::Other(format!("unused: {}", spec.name)))
    }

    async fn stop(&self, _name: &str) -> RuntimeResult<()> {
        Ok(())
    }

    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Err(RuntimeError::NotFound(name.into()))
    }

    async fn checkpoint_shared(&self, name: &str, checkpoint: &str) -> RuntimeResult<bool> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("checkpoint {name} {checkpoint}"));
        Ok(true)
    }

    async fn pin_checkpoint(
        &self,
        runtime_type: &str,
        checkpoint: &str,
        pinned: bool,
    ) -> RuntimeResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("pin {runtime_type} {checkpoint} {pinned}"));
        Ok(())
    }

    async fn delete_checkpoint(&self, runtime_type: &str, checkpoint: &str) -> RuntimeResult<()> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("delete {runtime_type} {checkpoint}"));
        Ok(())
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![LocalSummary {
            name: "bake".into(),
            backend: "qemu".into(),
            status: InstanceStatus::Running,
        }])
    }

    async fn status(&self, _name: &str) -> RuntimeResult<InstanceStatus> {
        Ok(InstanceStatus::Running)
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        Err(RuntimeError::NotFound(name.into()))
    }

    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211, 1)].into(),
            ..Default::default()
        })
    }
}

/// The daemon (kept alive by the returned handle) and a client of it.
async fn daemon(
    dir: &std::path::Path,
    rt: Arc<Runtime>,
) -> (cua_daemon::server::DaemonHandle, DaemonClient) {
    let runtime = DaemonRuntime::new(RuntimeConfig {
        state_dir: Some(dir.join("sandboxes")),
        spaces_home: Some(dir.to_path_buf()),
        local: Some(rt),
        ..Default::default()
    })
    .unwrap();
    runtime
        .sandboxes()
        .state()
        .save(&SandboxState::Local(LocalState {
            name: "bake".into(),
            runtime_type: "qemu".into(),
            host: "127.0.0.1".into(),
            api_port: 1,
            status: "running".into(),
            ..Default::default()
        }))
        .unwrap();
    let handle = start(
        runtime.clone(),
        ServerConfig {
            socket_path: None,
            loopback: Some(([127, 0, 0, 1], 0).into()),
            token: "t0k3n".into(),
            discovery_path: None,
            bridge_ticket_ttl: Duration::from_secs(60),
        },
    )
    .await
    .unwrap();
    let client = DaemonClient::new(DaemonAddress::Url {
        url: handle.loopback_url.clone().unwrap(),
        token: handle.token.clone(),
    })
    .unwrap();
    (handle, client)
}

#[tokio::test]
async fn pins_and_prune_round_trip_through_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let rt = Arc::new(Runtime::default());
    let (_daemon, client) = daemon(dir.path(), rt.clone()).await;

    let world = client.checkpoint_with("bake", "world", true).await.unwrap();
    assert_eq!(world.name, "world");
    assert!(world.pinned);
    assert!(world.shared_memory);
    let scratch = client.checkpoint("bake", "scratch").await.unwrap();
    assert!(!scratch.pinned);
    assert!(scratch.shared_memory);
    let listed = client.checkpoints().await.unwrap();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().find(|c| c.name == "world").unwrap().pinned);

    match client.delete_checkpoint("world").await {
        Err(e @ Error::InvalidArgument(_)) => {
            assert!(e.to_string().contains("checkpoint world is pinned"), "{e}")
        }
        other => panic!("expected a pinned refusal, got {other:?}"),
    }

    // Prune leaves the pinned one.
    assert_eq!(client.prune_checkpoints().await.unwrap(), ["scratch"]);
    let left: Vec<String> = client
        .checkpoints()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(left, ["world"]);

    let unpinned = client.pin_checkpoint("world", false).await.unwrap();
    assert!(!unpinned.pinned);
    assert!(unpinned.shared_memory, "the record keeps what it was");
    assert_eq!(client.prune_checkpoints().await.unwrap(), ["world"]);
    assert!(client.checkpoints().await.unwrap().is_empty());
    assert!(matches!(
        client.pin_checkpoint("world", true).await,
        Err(Error::NotFound(_))
    ));

    let calls = rt.calls.lock().unwrap().clone();
    assert_eq!(
        calls,
        [
            "checkpoint bake world",
            "pin qemu world true",
            "checkpoint bake scratch",
            "delete qemu scratch",
            "pin qemu world false",
            "delete qemu world",
        ]
    );
}
