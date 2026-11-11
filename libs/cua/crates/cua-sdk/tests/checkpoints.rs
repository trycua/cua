//! The checkpoint and fork API of the SDK (the surface every binding is
//! generated from): `Sandboxes.checkpoint(.., pin)`, `pin_checkpoint`,
//! `prune_checkpoints`, `CheckpointSummary.pinned/.shared_memory`,
//! `forkable` create options, `Space.checkpoint` / `Space.fork` /
//! `Spaces.fork`, embedded and through a daemon over loopback. Fakes only
//! (temp homes, a fake local runtime, a mock cua-spacesd; no host
//! processes).

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig, fixtures,
    server::{self, ServerConfig},
};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalState,
    LocalSummary, RuntimeError, RuntimeResult, SandboxState,
};
use cua_sdk::{
    CheckpointSummary, Cua, CuaError, SandboxCreateOptions, Sandboxes, Space, SpaceCreateOptions,
    Spaces,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The API's shape, checked by the compiler: what every binding exposes.
#[allow(dead_code)]
async fn api(sbx: &Sandboxes, spaces: &Spaces, space: &Space) -> cua_sdk::Result<()> {
    let c: CheckpointSummary = sbx.checkpoint("bake".into(), "world".into(), true).await?;
    let _: (bool, bool) = (c.pinned, c.shared_memory);
    let _: CheckpointSummary = sbx.pin_checkpoint("world".into(), false).await?;
    let _: Vec<String> = sbx.prune_checkpoints().await?;
    let _: CheckpointSummary = space.checkpoint("world".into(), false).await?;
    let _: Arc<Space> = space.fork("copy".into()).await?;
    let _: Arc<Space> = spaces.fork("world".into(), "copy".into()).await?;
    Ok(())
}

#[test]
fn create_options_take_forkable() {
    assert_eq!(SandboxCreateOptions::default().forkable, None);
    assert_eq!(SpaceCreateOptions::default().forkable, None);
    let o = SandboxCreateOptions {
        forkable: Some(true),
        ..SandboxCreateOptions::new("local", "linux")
    };
    assert_eq!(o.forkable, Some(true));
}

/// Local instances by name; checkpoints share memory; a fork starts with
/// cua-spacesd at `env_port`.
struct Fake {
    env_port: u16,
    instances: Mutex<HashMap<String, InstanceStatus>>,
}

impl Fake {
    fn instance(&self, name: &str) -> LocalInstance {
        LocalInstance {
            name: name.into(),
            backend: "qemu".into(),
            status: InstanceStatus::Running,
            endpoints: LocalEndpoints {
                host: "127.0.0.1".into(),
                ports: [(3211, self.env_port)].into(),
                ..Default::default()
            },
        }
    }
}

#[async_trait]
impl LocalRuntime for Fake {
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
        self.instances
            .lock()
            .unwrap()
            .insert(name.into(), InstanceStatus::Running);
        Ok(self.instance(name))
    }

    async fn fork(&self, _source: &str, new_name: &str) -> RuntimeResult<()> {
        self.instances
            .lock()
            .unwrap()
            .insert(new_name.into(), InstanceStatus::Stopped);
        Ok(())
    }

    async fn checkpoint_shared(&self, _name: &str, _checkpoint: &str) -> RuntimeResult<bool> {
        Ok(true)
    }

    async fn delete_checkpoint(&self, _runtime_type: &str, _checkpoint: &str) -> RuntimeResult<()> {
        Ok(())
    }

    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .instances
            .lock()
            .unwrap()
            .iter()
            .map(|(n, s)| LocalSummary {
                name: n.clone(),
                backend: "qemu".into(),
                status: s.clone(),
            })
            .collect())
    }

    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.instances
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.instances
            .lock()
            .unwrap()
            .remove(name)
            .map(drop)
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }

    async fn endpoints(&self, name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(self.instance(name).endpoints)
    }
}

struct World {
    _env: fixtures::SpacesdFixture,
    _home: tempfile::TempDir,
    _daemon: Option<server::DaemonHandle>,
}

/// A runtime with one running local QEMU sandbox, `bake`, whose
/// cua-spacesd (and its forks') is a mock; embedded or behind a daemon.
async fn world(daemon: bool) -> (World, Arc<Cua>) {
    let env = fixtures::start_env(None, None).await;
    let port: u16 = env.url.rsplit(':').next().unwrap().parse().unwrap();
    let home = tempfile::tempdir().unwrap();
    let fake = Arc::new(Fake {
        env_port: port,
        instances: Mutex::new([("bake".to_string(), InstanceStatus::Running)].into()),
    });
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().join("cua")),
        local: Some(fake),
        env_probe_timeout: Some(Duration::from_secs(5)),
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
            api_port: port,
            exposed_ports: Some([("3211".to_string(), port)].into()),
            status: "running".into(),
            ..Default::default()
        }))
        .unwrap();
    let (cua, handle) = if daemon {
        let h = server::start(
            runtime,
            ServerConfig {
                socket_path: None,
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(home.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
        (cua, Some(h))
    } else {
        runtime.mark_share_host();
        (Cua::from_runtime(runtime), None)
    };
    (
        World {
            _env: env,
            _home: home,
            _daemon: handle,
        },
        cua,
    )
}

async fn pins_prune_and_forks(daemon: bool) {
    let (_world, cua) = world(daemon).await;
    let sbx = cua.sandboxes();

    let world = sbx
        .checkpoint("bake".into(), "world".into(), true)
        .await
        .unwrap();
    assert!(world.pinned && world.shared_memory, "{world:?}");
    let scratch = sbx
        .checkpoint("bake".into(), "scratch".into(), false)
        .await
        .unwrap();
    assert!(!scratch.pinned && scratch.shared_memory, "{scratch:?}");
    let listed = sbx.checkpoints().await.unwrap();
    let pinned: Vec<_> = listed.iter().map(|c| (c.name.as_str(), c.pinned)).collect();
    assert_eq!(pinned, [("world", true), ("scratch", false)]);
    assert!(listed.iter().all(|c| c.shared_memory));

    match sbx.delete_checkpoint("world".into()).await {
        Err(CuaError::InvalidArgument(m)) => {
            assert!(m.contains("checkpoint world is pinned"), "{m}")
        }
        other => panic!("expected a pinned refusal, got {other:?}"),
    }
    assert_eq!(sbx.prune_checkpoints().await.unwrap(), ["scratch"]);

    // A Space of the sandbox checkpoints and forks; a fork of a checkpoint
    // comes back connected, as `Spaces.space` returns it.
    let spaces = cua.spaces();
    spaces.add("local:bake".into(), None, None).await.unwrap();
    let bake = spaces.space("local:bake".into()).await.unwrap();
    let again = bake.checkpoint("again".into(), false).await.unwrap();
    assert_eq!(again.source, "bake");
    assert!(!again.pinned && again.shared_memory);
    let copy = bake.fork("bake-copy".into()).await.unwrap();
    assert_eq!(copy.id(), "local:bake-copy");
    let agent = spaces.fork("world".into(), "agent-1".into()).await.unwrap();
    assert_eq!(agent.id(), "local:agent-1");
    let from_id = spaces
        .fork("local:bake".into(), "agent-2".into())
        .await
        .unwrap();
    assert_eq!(from_id.id(), "local:agent-2");

    let unpinned = sbx.pin_checkpoint("world".into(), false).await.unwrap();
    assert!(!unpinned.pinned);
    let mut deleted = sbx.prune_checkpoints().await.unwrap();
    deleted.sort();
    assert_eq!(deleted, ["again", "world"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pins_prune_and_forks_embedded() {
    pins_prune_and_forks(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pins_prune_and_forks_through_the_daemon() {
    pins_prune_and_forks(true).await;
}

#[tokio::test]
async fn a_space_that_is_not_local_neither_checkpoints_nor_forks() {
    let home = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(home.path().join("sandboxes")),
        spaces_home: Some(home.path().join("cua")),
        ..Default::default()
    })
    .unwrap();
    let id = "direct:127.0.0.1:9";
    runtime
        .spaces()
        .registry()
        .upsert(
            cua_proto::daemon::v1::Space {
                id: id.into(),
                name: "Generic service".into(),
                ..Default::default()
            },
            cua_spaces::registry::Credential {
                // Constructing the handle does not contact this endpoint.
                service_urls: [("service".into(), "http://127.0.0.1:9/mcp".into())].into(),
                ..Default::default()
            },
        )
        .unwrap();
    let cua = Cua::from_runtime(runtime);
    let space = cua.spaces().space(id.into()).await.unwrap();
    match space.checkpoint("x".into(), false).await {
        Err(CuaError::Unsupported(m)) => {
            assert_eq!(m, "Checkpointing is not supported for direct Spaces")
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
    match space.fork("y".into()).await {
        Err(CuaError::Unsupported(m)) => assert_eq!(
            m,
            "forks are of local sandboxes (direct:127.0.0.1:9 is not one)"
        ),
        other => panic!("expected Unsupported, got {:?}", other.map(|s| s.id())),
    }
    match cua.spaces().fork(id.into(), "y".into()).await {
        Err(CuaError::Unsupported(m)) => assert!(m.starts_with("forks are of local"), "{m}"),
        other => panic!("expected Unsupported, got {:?}", other.map(|s| s.id())),
    }
}
