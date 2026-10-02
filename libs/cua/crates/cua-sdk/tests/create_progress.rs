//! `Spaces.create_with_progress`: a local Space create reports what it is
//! doing (the runtime's pull and boot, the wait for cua-spacesd, the
//! connect) in order and ends with `ready`, embedded and through the daemon
//! (`CreateSpaceStream`). A failed create still reports the progress it made
//! and returns the error. Everything runs against in-process fakes: a fake
//! local runtime whose instance is a MockServer spacesd. Nothing starts a
//! container or VM, and nothing touches `~/.cua`.

use async_trait::async_trait;
use cua_daemon::{
    Runtime, RuntimeConfig,
    fixtures::{self, SpacesdFixture},
    server::{self, DaemonHandle, ServerConfig},
};
use cua_sandbox_core::progress::{Phase, Progress, report};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, RuntimeResult,
};
use cua_sdk::{Cua, SpaceCreateListener, SpaceCreateOptions, SpaceCreateProgress};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// A runtime whose one instance is the fixture's spacesd; it reports a pull
/// and a boot the way `cua_vmm`'s backends do. `fail` makes `start` fail
/// after the pull.
struct Fake {
    port: u16,
    fail: bool,
    /// `start` reports a pull, then never returns (a download a cancel
    /// cuts off).
    hang: bool,
    running: Mutex<HashMap<String, bool>>,
    gpus: Arc<Mutex<Vec<Option<String>>>>,
}

#[async_trait]
impl LocalRuntime for Fake {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> RuntimeResult<LocalInstance> {
        report(Progress::phase(Phase::Pulling).detail(&spec.image));
        self.gpus.lock().unwrap().push(spec.gpu.clone());
        report(
            Progress::phase(Phase::Pulling)
                .fraction(0.25)
                .detail(&spec.image)
                .bytes(cua_sandbox_core::progress::Transfer {
                    done: 250,
                    total: 1000,
                    per_second: Some(100.0),
                }),
        );
        if self.hang {
            self.running.lock().unwrap().insert(spec.name.clone(), true);
            std::future::pending::<()>().await;
        }
        report(
            Progress::phase(Phase::Pulling)
                .fraction(1.0)
                .detail(&spec.image),
        );
        if self.fail {
            return Err(RuntimeError::Other("the engine ran out of disk".into()));
        }
        report(Progress::phase(Phase::Booting));
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, _: &str) -> RuntimeResult<()> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> RuntimeResult<LocalInstance> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn list(&self) -> RuntimeResult<Vec<LocalSummary>> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> RuntimeResult<InstanceStatus> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            _ => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn endpoints(&self, _: &str) -> RuntimeResult<LocalEndpoints> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
    async fn delete(&self, name: &str) -> RuntimeResult<()> {
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
}

#[derive(Default)]
struct Heard(Mutex<Vec<SpaceCreateProgress>>);

impl SpaceCreateListener for Heard {
    fn on_progress(&self, progress: SpaceCreateProgress) {
        self.0.lock().unwrap().push(progress);
    }
}

impl Heard {
    fn phases(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|p| p.phase.clone())
            .collect()
    }
}

struct World {
    _env: SpacesdFixture,
    _dirs: tempfile::TempDir,
    _daemon: Option<DaemonHandle>,
    cua: Arc<Cua>,
    gpus: Arc<Mutex<Vec<Option<String>>>>,
}

async fn world(daemon: bool, fail: bool) -> World {
    world_with(daemon, fail, false).await
}

async fn world_with(daemon: bool, fail: bool, hang: bool) -> World {
    let gpus: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let env = fixtures::start_env(None, None).await;
    let port: u16 = env.url.rsplit(':').next().unwrap().parse().unwrap();
    let dirs = tempfile::tempdir().unwrap();
    let runtime = Runtime::new(RuntimeConfig {
        state_dir: Some(dirs.path().join("sandboxes")),
        // Never the real ~/.cua registry.
        spaces_home: Some(dirs.path().join("cua")),
        local: Some(Arc::new(Fake {
            port,
            fail,
            hang,
            running: Mutex::default(),
            gpus: gpus.clone(),
        })),
        env_probe_timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    })
    .unwrap();
    if !daemon {
        return World {
            _env: env,
            _dirs: dirs,
            _daemon: None,
            cua: Cua::from_runtime(runtime),
            gpus,
        };
    }
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
    let cua = Cua::connect(h.loopback_url.clone(), Some(h.token.clone())).unwrap();
    World {
        _env: env,
        _dirs: dirs,
        _daemon: Some(h),
        cua,
        gpus,
    }
}

fn opts(name: &str) -> SpaceCreateOptions {
    SpaceCreateOptions {
        image: Some("ghcr.io/trycua/linux:24.04".into()),
        on: Some("local".into()),
        kind: Some("container".into()),
        name: Some(name.into()),
        // Ignored by `create_with_progress`, which always waits.
        wait: Some(false),
        ..Default::default()
    }
}

async fn reports_in_order(daemon: bool) {
    let w = world(daemon, false).await;
    let heard = Arc::new(Heard::default());
    let created = w
        .cua
        .spaces()
        .create_with_progress(
            SpaceCreateOptions {
                gpu: Some("auto".into()),
                ..opts("cua-e2e-progress")
            },
            heard.clone(),
        )
        .await
        .unwrap();
    let space = created.space.expect("waited until ready");
    assert_eq!(space.id, "local:cua-e2e-progress");
    let phases = heard.phases();
    let order = [
        "preparing",
        "pulling",
        "booting",
        "waiting_for_services",
        "connecting",
        "ready",
    ];
    let mut dedup = phases.clone();
    dedup.dedup();
    assert_eq!(dedup, order, "{phases:?}");
    let pulls: Vec<Option<f64>> = heard
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.phase == "pulling")
        .map(|p| p.fraction)
        .collect();
    assert_eq!(pulls, [None, Some(0.25), Some(1.0)]);
    // Byte counts and the Space's id ride along, embedded and through the
    // daemon's stream.
    let with_bytes: Vec<(Option<u64>, Option<u64>, Option<f64>)> = heard
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|p| p.bytes_done.is_some())
        .map(|p| (p.bytes_done, p.bytes_total, p.bytes_per_second))
        .collect();
    assert_eq!(with_bytes, [(Some(250), Some(1000), Some(100.0))]);
    assert!(
        heard
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|p| p.phase != "ready")
            .all(|p| p.space == "local:cua-e2e-progress"),
        "{:?}",
        heard.0.lock().unwrap()
    );
    // The GPU option reaches the runtime.
    assert_eq!(
        w.gpus.lock().unwrap().as_slice(),
        [Some("auto".to_string())]
    );
    assert!(
        heard
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|p| p.detail == "ghcr.io/trycua/linux:24.04")
    );
    assert_eq!(w.cua.spaces().list().await.unwrap().len(), 1);
}

async fn failure_reports_then_errors(daemon: bool) {
    let w = world(daemon, true).await;
    let heard = Arc::new(Heard::default());
    let err = w
        .cua
        .spaces()
        .create_with_progress(opts("cua-e2e-progress-fail"), heard.clone())
        .await
        .expect_err("the runtime failed");
    assert!(err.to_string().contains("out of disk"), "{err}");
    let phases = heard.phases();
    assert!(phases.contains(&"pulling".to_string()), "{phases:?}");
    assert!(!phases.contains(&"ready".to_string()), "{phases:?}");
    assert!(w.cua.spaces().list().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_reports_progress_in_order() {
    tokio::time::timeout(Duration::from_secs(60), reports_in_order(false))
        .await
        .expect("timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_streams_progress_in_order() {
    tokio::time::timeout(Duration::from_secs(60), reports_in_order(true))
        .await
        .expect("timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_failure_reports_then_errors() {
    tokio::time::timeout(Duration::from_secs(60), failure_reports_then_errors(false))
        .await
        .expect("timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_failure_reports_then_errors() {
    tokio::time::timeout(Duration::from_secs(60), failure_reports_then_errors(true))
        .await
        .expect("timed out");
}

/// `Spaces.cancel_create` stops a create by its `create_id`, embedded and
/// through the daemon: the create fails with `Cancelled`, the instance is
/// gone, and a second cancel finds nothing to do.
async fn cancel_stops_the_create(daemon: bool) {
    let w = world_with(daemon, false, true).await;
    let heard = Arc::new(Heard::default());
    let spaces = w.cua.spaces();
    let create = {
        let spaces = spaces.clone();
        let heard = heard.clone();
        tokio::spawn(async move {
            spaces
                .create_with_progress(
                    SpaceCreateOptions {
                        create_id: Some("app:1".into()),
                        ..opts("cua-e2e-cancel")
                    },
                    heard,
                )
                .await
        })
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !heard
        .0
        .lock()
        .unwrap()
        .iter()
        .any(|p| p.bytes_done.is_some())
    {
        assert!(tokio::time::Instant::now() < deadline, "no pull reported");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let o = spaces.cancel_create("app:1".into()).await.unwrap();
    assert_eq!(
        (o.state.as_str(), o.id.as_str()),
        ("cancelled", "local:cua-e2e-cancel")
    );
    match create.await.unwrap() {
        Err(cua_sdk::CuaError::Cancelled(m)) => assert!(m.contains("Cancelled"), "{m}"),
        other => panic!("{other:?}"),
    }
    let again = spaces
        .cancel_create("local:cua-e2e-cancel".into())
        .await
        .unwrap();
    assert_eq!(again.state, "not_creating");
    assert!(spaces.list().await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_cancel_stops_the_create() {
    tokio::time::timeout(Duration::from_secs(60), cancel_stops_the_create(false))
        .await
        .expect("timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn daemon_cancel_stops_the_create() {
    tokio::time::timeout(Duration::from_secs(60), cancel_stops_the_create(true))
        .await
        .expect("timed out");
}
