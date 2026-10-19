//! Unified sandbox refs through the SDK, embedded and through the daemon:
//! qualified ids (`local:`, `direct:`) that round-trip, a typed
//! `AmbiguousSandbox` for a bare name in two locations, narrowing, the
//! legacy spellings, and `cloud:` refs, which parse but answer "Cua Cloud has
//! ended". In-memory fakes only (a fake local runtime, a loopback MockServer
//! spacesd); nothing touches host apps or `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig, fixtures,
    server::{self, ServerConfig},
};
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

async fn suite(daemon: bool) {
    let env = fixtures::start_env(None, None).await;
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        spaces_home: Some(dirs.path().join("cua")),
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

    // A direct machine's id is its address.
    let url = env.url.clone();
    let authority = url.trim_start_matches("http://").to_string();
    let db = sbx
        .connect_url(url.clone(), None, Some("box".into()))
        .await
        .unwrap();
    assert_eq!(db.id(), format!("direct:{authority}"));
    assert_eq!(db.name(), "box");

    // A bare name in two locations is a typed error listing both refs.
    let err = sbx.get("box".into()).await.unwrap_err();
    let CuaError::AmbiguousSandbox(message) = &err else {
        panic!("{err:?}")
    };
    let mut candidates = ambiguous_sandbox_candidates(message.clone());
    candidates.sort();
    assert_eq!(
        candidates,
        vec![format!("direct:{authority}"), "local:box".to_string()]
    );
    assert!(matches!(
        sbx.connect("box".into()).await,
        Err(CuaError::AmbiguousSandbox(_))
    ));

    // Qualified refs and narrowing pick one; ids round-trip.
    for id in [lb.id(), db.id()] {
        assert_eq!(sbx.get(id.clone()).await.unwrap().id, id);
        assert_eq!(sbx.connect(id.clone()).await.unwrap().id(), id);
    }
    assert_eq!(
        qualify_sandbox_ref("box".into(), Some(true)).unwrap(),
        "local:box"
    );
    assert!(matches!(
        qualify_sandbox_ref("cloud:box".into(), Some(true)),
        Err(CuaError::InvalidArgument(_))
    ));

    // Legacy spellings still resolve; output uses the new form.
    assert_eq!(
        sbx.get("space://local/box".into()).await.unwrap().id,
        "local:box"
    );
    let p = parse_sandbox_ref(format!("fleet:{POOL}:box")).unwrap();
    assert_eq!(
        (p.location.as_deref(), p.name.as_str(), p.id.as_str()),
        (Some("cloud"), "box", "cloud:box")
    );

    // Cua Cloud has ended: a cloud ref parses, then fails with the one
    // message wherever it is used.
    for cloud in ["cloud:box", &format!("fleet:{POOL}:box")] {
        let e = sbx.get(cloud.into()).await.unwrap_err();
        assert!(
            e.to_string().contains("Cua Cloud has ended"),
            "{cloud}: {e}"
        );
        let e = sbx.connect(cloud.into()).await.err().expect("fails");
        assert!(
            e.to_string().contains("Cua Cloud has ended"),
            "{cloud}: {e}"
        );
        let e = sbx.delete(cloud.into()).await.unwrap_err();
        assert!(
            e.to_string().contains("Cua Cloud has ended"),
            "{cloud}: {e}"
        );
    }

    // Listings carry the local and direct ones, by id, and no cloud one.
    let ids: Vec<String> = sbx
        .list(None)
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(ids.contains(&"local:box".to_string()), "{ids:?}");
    assert!(ids.contains(&db.id()), "{ids:?}");
    assert!(ids.iter().all(|i| !i.starts_with("cloud:")), "{ids:?}");

    // A remembered connection is found by its name once it is unambiguous.
    sbx.delete("local:box".into()).await.unwrap();
    assert_eq!(
        sbx.get("box".into()).await.unwrap().id,
        db.id(),
        "a remembered connection is found by its name"
    );
    assert_eq!(
        sbx.get(format!("url:{authority}")).await.unwrap().id,
        db.id()
    );
    sbx.delete(db.id()).await.unwrap();
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
