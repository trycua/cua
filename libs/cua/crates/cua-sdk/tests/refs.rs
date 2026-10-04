#![allow(deprecated)] // `apply_pool` sets up the fake pools
//! Unified sandbox refs through the SDK, embedded and through the daemon:
//! qualified ids (`local:`, `cloud:`, `direct:`) that round-trip, a typed
//! `AmbiguousSandbox` for a bare name in two locations, narrowing, and the
//! legacy spellings. In-memory fakes only (a fake local runtime, FakeFleet,
//! a loopback MockServer spacesd); nothing touches host apps or `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig, fixtures,
    server::{self, ServerConfig},
};
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use cua_sdk::{
    Cua, CuaError, SandboxCreateOptions, ambiguous_sandbox_candidates, parse_sandbox_ref,
    qualify_sandbox_ref,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

const POOL: &str = "cua-e2e-refs";

/// A local runtime that only records instances.
#[derive(Default)]
struct Instances(Mutex<HashMap<String, InstanceStatus>>);

fn instance(name: &str) -> LocalInstance {
    LocalInstance {
        name: name.into(),
        backend: "fake".into(),
        status: InstanceStatus::Running,
        endpoints: LocalEndpoints {
            host: "127.0.0.1".into(),
            ..Default::default()
        },
    }
}

#[async_trait]
impl LocalRuntime for Instances {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        self.0
            .lock()
            .unwrap()
            .insert(spec.name.clone(), InstanceStatus::Running);
        Ok(instance(&spec.name))
    }
    async fn stop(&self, name: &str) -> RuntimeResult<()> {
        self.0
            .lock()
            .unwrap()
            .insert(name.into(), InstanceStatus::Stopped);
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(instance(name))
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(n, s)| LocalSummary {
                name: n.clone(),
                backend: "fake".into(),
                status: s.clone(),
            })
            .collect())
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        self.0
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }
    async fn endpoints(&self, _name: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(instance("x").endpoints)
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.0
            .lock()
            .unwrap()
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| RuntimeError::NotFound(name.into()))
    }
}

fn local(name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        name: Some(name.into()),
        ..SandboxCreateOptions::new("local", "container:img")
    }
}

fn cloud(name: &str) -> SandboxCreateOptions {
    SandboxCreateOptions {
        name: Some(name.into()),
        pool: Some(POOL.into()),
        ..SandboxCreateOptions::new("cloud", "ghcr.io/trycua/cua-desktop-linux:test")
    }
}

async fn suite(daemon: bool) {
    let env = fixtures::start_env(None, None).await;
    let fake = FakeFleet::new();
    fake.client()
        .apply_pool(&cua_fleet::PoolSpec::new(
            POOL,
            "ghcr.io/trycua/cua-desktop-linux:test",
        ))
        .await
        .unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
        fleet_client: Some(fake.client()),
        local: Some(Arc::new(Instances::default())),
        env_probe_timeout: Some(Duration::from_secs(2)),
        ..Default::default()
    })
    .unwrap();
    let mut handle = None;
    let cua = if daemon {
        let h = server::start(
            runtime,
            ServerConfig {
                socket_path: None,
                loopback: Some("127.0.0.1:0".parse().unwrap()),
                token: "daemon-token".into(),
                discovery_path: Some(dirs.path().join("daemon.json")),
                bridge_ticket_ttl: Duration::from_secs(30),
            },
        )
        .await
        .unwrap();
        let c = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
        handle = Some(h);
        c
    } else {
        runtime.mark_share_host();
        Cua::from_runtime(runtime)
    };
    let sbx = cua.sandboxes();

    // Qualified ids, the same kind of value everywhere.
    let lb = sbx.create(local("box")).await.unwrap();
    assert_eq!(lb.id(), "local:box");
    assert_eq!(lb.info().location, "local");
    let only = sbx.get("box".into()).await.unwrap();
    assert_eq!(only.id, "local:box", "a unique bare name resolves");

    let cb = sbx.create(cloud("box")).await.unwrap();
    assert_eq!(cb.id(), "cloud:box");
    assert_eq!(cb.info().location, "cloud");
    assert!(
        !cb.id().contains(POOL),
        "pools and namespaces never show in ids"
    );

    // A bare name in two locations is a typed error listing both refs.
    let err = sbx.get("box".into()).await.unwrap_err();
    let CuaError::AmbiguousSandbox(message) = &err else {
        panic!("{err:?}")
    };
    assert_eq!(
        ambiguous_sandbox_candidates(message.clone()),
        vec!["local:box".to_string(), "cloud:box".to_string()]
    );
    assert!(matches!(
        sbx.connect("box".into()).await,
        Err(CuaError::AmbiguousSandbox(_))
    ));

    // Qualified refs and narrowing pick one; ids round-trip.
    for id in [lb.id(), cb.id()] {
        assert_eq!(sbx.get(id.clone()).await.unwrap().id, id);
        assert_eq!(sbx.connect(id.clone()).await.unwrap().id(), id);
    }
    let narrowed = qualify_sandbox_ref("box".into(), Some(false)).unwrap();
    assert_eq!(narrowed, "cloud:box");
    assert_eq!(sbx.get(narrowed).await.unwrap().id, "cloud:box");
    assert_eq!(
        qualify_sandbox_ref("box".into(), Some(true)).unwrap(),
        "local:box"
    );
    assert!(matches!(
        qualify_sandbox_ref("cloud:box".into(), Some(true)),
        Err(CuaError::InvalidArgument(_))
    ));

    // Legacy spellings still resolve; output uses the new form.
    for legacy in [
        format!("fleet:{POOL}:box"),
        format!("space://fleet/{POOL}/box"),
    ] {
        assert_eq!(sbx.get(legacy).await.unwrap().id, "cloud:box");
    }
    assert_eq!(
        sbx.get("space://local/box".into()).await.unwrap().id,
        "local:box"
    );
    let p = parse_sandbox_ref(format!("fleet:{POOL}:box")).unwrap();
    assert_eq!(
        (p.location.as_deref(), p.name.as_str(), p.id.as_str()),
        (Some("cloud"), "box", "cloud:box")
    );

    // Listings carry both, by id.
    let ids: Vec<String> = sbx
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(ids.contains(&"local:box".to_string()), "{ids:?}");
    assert!(ids.contains(&"cloud:box".to_string()), "{ids:?}");

    // A cloud claim this machine did not create is reachable by its ref.
    fake.client()
        .claim(
            &fake.client().get_pool(POOL).await.unwrap().pool,
            cua_fleet::ClaimOptions {
                name: Some("elsewhere".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        sbx.connect("elsewhere".into()).await.unwrap().id(),
        "cloud:elsewhere"
    );

    // Cloud names are unique across the account's pools.
    fake.client()
        .apply_pool(&cua_fleet::PoolSpec::new(
            "cua-e2e-refs-2",
            "ghcr.io/trycua/cua-desktop-linux:test",
        ))
        .await
        .unwrap();
    let mut other_pool = cloud("box");
    other_pool.pool = Some("cua-e2e-refs-2".into());
    let Err(err) = sbx.create(other_pool).await else {
        panic!("a second cloud:box was created")
    };
    assert!(
        matches!(&err, CuaError::InvalidArgument(m) if m.contains("cloud:box")),
        "{err:?}"
    );

    // A direct machine's id is its address.
    let url = env.url.clone();
    let authority = url.trim_start_matches("http://").to_string();
    let d = sbx
        .connect_url(url.clone(), None, Some("dev".into()))
        .await
        .unwrap();
    assert_eq!(d.id(), format!("direct:{authority}"));
    assert_eq!(d.name(), "dev");
    assert_eq!(
        sbx.get("dev".into()).await.unwrap().id,
        d.id(),
        "a remembered connection is found by its name"
    );
    assert_eq!(
        sbx.get(format!("url:{authority}")).await.unwrap().id,
        d.id()
    );

    // Delete by ref: the other location's sandbox of the same name stays.
    sbx.delete("local:box".into()).await.unwrap();
    assert_eq!(sbx.get("box".into()).await.unwrap().id, "cloud:box");
    sbx.delete("cloud:box".into()).await.unwrap();
    sbx.delete("cloud:elsewhere".into()).await.unwrap();
    sbx.delete(d.id()).await.unwrap();
    assert!(matches!(
        sbx.get("box".into()).await,
        Err(CuaError::NotFound(_))
    ));
    drop(handle);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded() {
    tokio::time::timeout(Duration::from_secs(120), suite(false))
        .await
        .expect("bounded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon() {
    tokio::time::timeout(Duration::from_secs(120), suite(true))
        .await
        .expect("bounded");
}
