//! Cluster jobs on a host, hermetic: the host's `HostSpacesServer` runs
//! each job in a fresh Space of a local runtime whose instances are a mock
//! spacesd, through the real job runner ([`cua_cluster::NodeJobs`]) and
//! executor ([`cua_spaces::host_jobs::SpacesExecutor`]). A job's Space is
//! created with the member's directories mounted, its command runs (its
//! output streams into the host's log, its result file comes back from the
//! mounted work directory) and the Space is deleted; a host that does not
//! provide Spaces runs no jobs; the removed native kind is refused, and a
//! machine Space only on a dedicated host.

use async_trait::async_trait;
use cua_host::service::FakeServiceManager;
use cua_host::{Host, HostProfile, HostSettingsChange, SetupOptions};
use cua_proto::env::v1 as pb;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::Spaces;
use cua_spaces::host_spaces::{HostCaller, HostSpacesServer};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A local runtime whose instances are one mock spacesd on loopback.
struct FakeRuntime {
    port: u16,
    started: Mutex<Vec<LocalStartSpec>>,
    deleted: Mutex<Vec<String>>,
    running: Mutex<BTreeMap<String, bool>>,
    /// Instances their engine says stopped on their own (a killed container).
    exits: Mutex<BTreeMap<String, cua_sandbox_core::StopReason>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.started.lock().unwrap().push(spec.clone());
        // The guest writes its result into its mounted work directory.
        if let Some(work) = spec.mounts.iter().find(|m| m.guest == "/cua/job") {
            std::fs::write(work.host.join("out.json"), b"{\"ok\":true}").unwrap();
        }
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().insert(name.into(), false);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(true) => Ok(InstanceStatus::Running),
            Some(false) => Ok(InstanceStatus::Stopped),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.deleted.lock().unwrap().push(name.into());
        self.running.lock().unwrap().remove(name);
        Ok(())
    }
    async fn stop_reason(
        &self,
        name: &str,
        _runtime_type: &str,
    ) -> Result<Option<cua_sandbox_core::StopReason>, RuntimeError> {
        Ok(self.exits.lock().unwrap().get(name).copied())
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    _env: MockServer,
    rt: Arc<FakeRuntime>,
    server: Arc<HostSpacesServer>,
}

/// A spare machine in direct mode (no relay account), providing Spaces.
async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let mut opts = SetupOptions::direct("127.0.0.1:0".parse().unwrap()).profile(HostProfile::Spare);
    opts.driver_bin = Some(driver);
    Host::new(&home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .setup(opts, &cua_host::NoAccount)
        .await
        .unwrap();
    let env = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: env.addr.port(),
        started: Mutex::default(),
        deleted: Mutex::default(),
        running: Mutex::default(),
        exits: Mutex::default(),
    });
    let spaces = Spaces::builder()
        .home(&home)
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(dir.path().join("state"))
                .build(),
        )
        .build();
    Fixture {
        server: HostSpacesServer::new(spaces),
        _dir: dir,
        home,
        _env: env,
        rt,
    }
}

fn spec(id: &str, argv: &[&str]) -> pb::HostJobSpec {
    pb::HostJobSpec {
        job_id: id.into(),
        attempt: 1,
        image: "ghcr.io/trycua/linux:24.04-slim".into(),
        kind: "container".into(),
        argv: argv.iter().map(|s| s.to_string()).collect(),
        millicpus: 500,
        memory_mb: 256,
        timeout_secs: 30,
        ..Default::default()
    }
}

async fn ended(jobs: &cua_spaces::host_jobs::NodeJobs, id: &str) -> pb::HostJob {
    for _ in 0..400 {
        if let Some(r) = jobs.get(id)
            && r.state().is_terminal()
        {
            return r.to_pb(true);
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} did not end: {:?}", jobs.get(id));
}

/// The guest `uv` a member's Space gets, as if downloaded before (the test
/// stays off the network).
fn seed_uv(home: &std::path::Path) {
    let target = cua_host::uv::uv_target_for("linux", std::env::consts::ARCH).unwrap();
    let dir = home.join("job-tools").join(target);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("uv"), b"#!/bin/sh\n").unwrap();
    std::fs::write(
        dir.join(".uv-version"),
        format!("{} {target}\n", cua_host::uv::UV_VERSION),
    )
    .unwrap();
}

/// A job runs in a fresh Space that is deleted afterwards, with its
/// directories mounted; its output streams into the host's log, its exit
/// code and result file come back, and a re-delivery runs nothing.
#[tokio::test]
async fn a_job_runs_once_in_a_fresh_space_that_is_deleted() {
    let f = fixture().await;
    seed_uv(&f.home);
    let jobs = f.server.jobs_for(&HostCaller::local()).await.unwrap();
    let mut with_result = spec("c1-1", &["echo", "hello"]);
    with_result.result_path = "out.json".into();
    with_result.run_id = "train".into();
    let acks = jobs
        .run_batch(
            vec![with_result.clone(), spec("c1-2", &["fail", "3"])],
            "test",
        )
        .await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    let ok = ended(&jobs, "c1-1").await;
    assert_eq!((ok.state.as_str(), ok.exit_code), ("succeeded", Some(0)));
    assert_eq!(ok.stdout, b"hello\n");
    assert_eq!(ok.result, b"{\"ok\":true}");
    assert!(ok.space.starts_with("local:cj-"), "{}", ok.space);
    // The live output log is the host's, read from any offset.
    let (chunk, size) = jobs.output("c1-1", 0).unwrap();
    assert_eq!((chunk.as_slice(), size), (&b"hello\n"[..], 6));
    // The member's directories were mounted: its work directory (bundle,
    // control files), its run's data, the caches and uv (read-only).
    let started = f.rt.started.lock().unwrap().clone();
    let first = started
        .iter()
        .find(|s| {
            s.mounts
                .get(1)
                .is_some_and(|m| m.host.display().to_string().contains("/job-data/train-"))
        })
        .expect("c1-1's Space had its run's data directory mounted");
    let mounts: Vec<(&str, bool)> = first
        .mounts
        .iter()
        .map(|m| (m.guest.as_str(), m.read_only))
        .collect();
    assert_eq!(
        mounts,
        [
            ("/cua/job", false),
            ("/cua/data", false),
            ("/cua/cache", false),
            ("/cua/tools", true)
        ]
    );
    // A Space with mounts never comes from a warm pool.
    assert_eq!(first.warm, Some(false));
    let bad = ended(&jobs, "c1-2").await;
    assert_eq!((bad.state.as_str(), bad.exit_code), ("failed", Some(3)));
    // Each ran in its own Space, which is gone now.
    let started: Vec<String> =
        f.rt.started
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.name.clone())
            .collect();
    assert_eq!(started.len(), 2, "{started:?}");
    let deleted = f.rt.deleted.lock().unwrap().clone();
    for name in &started {
        assert!(deleted.contains(name), "{name} not deleted: {deleted:?}");
    }
    let again = jobs.run_batch(vec![with_result], "test").await;
    assert_eq!(again[0].outcome, "duplicate");
    assert_eq!(f.rt.started.lock().unwrap().len(), 2);
    // The ledger is on disk under the host directory.
    assert!(f.home.join("host/jobs/ledger.jsonl").is_file());
}

/// A job's Space that is killed while its command runs ends the job at once,
/// with the reason, instead of the host waiting for a Space that is gone
/// (CUA-2967: a durable agent's driver whose container was killed was only
/// noticed by the 120 s stall watch).
#[tokio::test]
async fn a_job_whose_space_is_killed_ends_at_once_with_the_reason() {
    let f = fixture().await;
    seed_uv(&f.home);
    let jobs = f.server.jobs_for(&HostCaller::local()).await.unwrap();
    let mut long = spec("k1-1", &["sleep", "120000"]);
    long.timeout_secs = 300;
    let acks = jobs.run_batch(vec![long], "test").await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    // Wait until its Space runs, then have the engine report it killed
    // (the command's own stream is still open: the mock spacesd stays up).
    let mut name = String::new();
    for _ in 0..400 {
        if let Some(s) = f.rt.started.lock().unwrap().first() {
            name = s.name.clone();
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(!name.is_empty(), "the job's Space never started");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let killed = std::time::Instant::now();
    f.rt.exits.lock().unwrap().insert(
        name.clone(),
        cua_sandbox_core::StopReason {
            exit_code: 137,
            out_of_memory: false,
        },
    );
    let end = tokio::time::timeout(Duration::from_secs(20), ended(&jobs, "k1-1"))
        .await
        .expect("the job did not end after its Space was killed");
    assert!(
        killed.elapsed() < Duration::from_secs(15),
        "took {:?}",
        killed.elapsed()
    );
    assert_eq!(end.state, "error", "{end:?}");
    assert!(
        end.error.contains("Space stopped") && end.error.contains("killed"),
        "{}",
        end.error
    );
    // Its Space is removed like any job's.
    assert!(f.rt.deleted.lock().unwrap().contains(&name));
}

/// What a scheduler reads: `GetHostSpaces` carries the node status.
#[tokio::test]
async fn the_host_reports_its_node_status() {
    let f = fixture().await;
    let r = f.server.get(&HostCaller::local()).await.unwrap();
    let node = r.node.expect("node status");
    assert!(node.millicpus_total > 0 && node.memory_total_mb > 0);
    assert!(!node.boot_id.is_empty() && !node.ledger_id.is_empty());
    assert_eq!(node.epoch, 1);
    assert!(node.problem.is_empty(), "{}", node.problem);
}

/// Spaces off: no jobs, and the status says why; a view-only caller runs
/// none either.
#[tokio::test]
async fn a_host_that_does_not_provide_spaces_runs_no_jobs() {
    let f = fixture().await;
    let viewer = HostCaller {
        account: "eve".into(),
        role: "viewer".into(),
        ..Default::default()
    };
    let e = f.server.jobs_for(&viewer).await.unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .configure(HostSettingsChange {
            provide_spaces: Some(false),
            share_desktop: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    let e = f.server.jobs_for(&HostCaller::local()).await.unwrap_err();
    assert!(e.to_string().contains("does not provide Spaces"), "{e}");
    let node = f
        .server
        .get(&HostCaller::local())
        .await
        .unwrap()
        .node
        .unwrap();
    assert!(
        node.problem.contains("does not provide Spaces"),
        "{}",
        node.problem
    );
}

/// The removed native kind (`kind = "process"`) is refused with a reason,
/// and nothing starts; a machine Space (`kind = "machine"`) is offered only
/// by a dedicated host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_jobs_are_refused_and_machine_spaces_need_a_dedicated_host() {
    let f = fixture().await;
    let jobs = f.server.jobs_for(&HostCaller::local()).await.unwrap();
    let mut native = spec("n-1", &["echo", "hi"]);
    native.kind = "process".into();
    let acks = jobs.run_batch(vec![native], "test").await;
    assert_eq!(acks[0].outcome, "rejected");
    assert!(
        acks[0].reason.contains("every job runs in a Space"),
        "{}",
        acks[0].reason
    );
    let node = f.server.node_status().await.unwrap();
    assert!(!node.dedicated && !node.kinds.iter().any(|k| k == "machine" || k == "process"));
    let mut machine = spec("m-1", &["echo", "hi"]);
    machine.kind = "machine".into();
    let acks = jobs.run_batch(vec![machine], "test").await;
    assert_eq!(acks[0].outcome, "rejected");
    assert!(
        acks[0].reason.contains("not dedicated"),
        "{}",
        acks[0].reason
    );
    assert!(f.rt.started.lock().unwrap().is_empty());
}

/// A dedicated host (`cua host setup --dedicated`) offers its own Space,
/// and picks it for a job that names no kind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dedicated_host_offers_its_own_space() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let mut opts = SetupOptions::direct("127.0.0.1:0".parse().unwrap()).profile(HostProfile::Spare);
    opts.driver_bin = Some(driver);
    opts.dedicated = true;
    Host::new(&home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .setup(opts, &cua_host::NoAccount)
        .await
        .unwrap();
    let spaces = Spaces::builder()
        .home(&home)
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .state_dir(dir.path().join("state"))
                .build(),
        )
        .build();
    let server = HostSpacesServer::new(spaces);
    let node = server.node_status().await.unwrap();
    assert!(node.dedicated, "{node:?}");
    assert!(
        node.kinds.iter().any(|k| k == "machine"),
        "{:?}",
        node.kinds
    );
    let jobs = server.jobs_for(&HostCaller::local()).await.unwrap();
    let mut auto = spec("a-1", &["echo", "hi"]);
    auto.kind = String::new();
    auto.image = String::new();
    let acks = jobs.run_batch(vec![auto], "test").await;
    assert_eq!(acks[0].outcome, "accepted", "{acks:?}");
    assert_eq!(jobs.get("a-1").unwrap().kind, "machine");
}
