//! Fleet and Local provisioning against fakes: `cua_fleet::testing::FakeFleet`
//! for the control plane and `cua_spacesd_client::testing::MockServer` as the guest's
//! spacesd (behind an emulated Fleet gateway for claims).

use async_trait::async_trait;
use cua_fleet::testing::FakeFleet;
use cua_sandbox_core::placement::{On, Runtime};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::contract::inputs::FleetRuntime;
use cua_spaces::{Provider, SpaceCreate, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockGateway, MockServer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const IMAGE: &str = "ghcr.io/trycua/linux:24.04-disk";

#[tokio::test]
async fn create_in_the_cloud_binds_connects_through_the_gateway_and_registers() {
    let reg = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let probe = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client())
        .fleet_namespace("cua-e2e-sp")
        .build();
    let pool = probe.fleet_pool_name(FleetRuntime::Kubevirt, IMAGE);
    let srv = MockServer::start(MockAuth {
        token: None,
        gateway: Some(MockGateway {
            prefix: format!("/api/svc/{pool}/sbx-cua-e2e-claim-env"),
            bearer: "fake-fleet-token".into(),
            claim: "cua-e2e-claim".into(),
        }),
        prefix: None,
    })
    .await;
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client_with_base(&srv.url()))
        .fleet_namespace("cua-e2e-sp")
        .build();
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(IMAGE.into()),
            name: Some("cua-e2e-claim".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .expect("waited");
    assert_eq!(info.id, "cloud:cua-e2e-claim");
    assert_eq!(info.provider, Provider::Cloud);
    assert!(
        fake.exists("pool", &pool, &pool),
        "a warm pool per image and runtime"
    );
    // The Image row: the claim's template names the image (a tag, so no
    // digest), recorded with the Space.
    assert_eq!(
        (info.image.as_str(), info.image_digest.as_str()),
        (IMAGE, "")
    );

    // Commands go through the gateway (gRPC-Web + claim header).
    let space = spaces.space(&info.id).await.unwrap();
    assert_eq!(
        space.spacesd().unwrap().transport(),
        cua_spacesd_client::Transport::GrpcWeb
    );
    let out = space
        .bash("echo through-the-gateway", Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(out.stdout, "through-the-gateway\n");
    // The token travelled with the claim, in its Secret (the image reads
    // it from /run/cua/env-token): the template opts in, the claim names
    // the Secret, and the Space holds the same token.
    let cred = spaces.registry().credential(&info.id).unwrap().unwrap();
    let token = cred.token.clone().expect("a cloud Space holds its token");
    assert_eq!(token.len(), 64);
    let t = fake.object("template", &pool, &pool).unwrap();
    assert_eq!(t["spec"]["vmTemplate"]["claimSecrets"], true);
    let claim = fake.object("claim", &pool, "cua-e2e-claim").unwrap();
    assert_eq!(
        claim["spec"]["secretRef"]["name"],
        "cua-claim-cua-e2e-claim"
    );
    let secret = fake
        .object("secret", &pool, "cua-claim-cua-e2e-claim")
        .unwrap();
    let held = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        secret["data"]["env-token"].as_str().unwrap(),
    )
    .unwrap();
    assert_eq!(held, token.as_bytes());

    // `reuse` returns it instead of creating another.
    let again = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            reuse: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(again.reused());
    assert_eq!(again.ready().unwrap().id, info.id);

    // A registry written before the unified refs (a `space://fleet/...` id,
    // no namespace hint) still reaches it: the namespace is looked up by
    // the claim name. A bare name in two locations is ambiguous (typed).
    let legacy = tempfile::tempdir().unwrap();
    std::fs::write(
        legacy.path().join("spaces.json"),
        serde_json::to_vec(&serde_json::json!([
            {"id": format!("space://fleet/{pool}/cua-e2e-claim"), "name": "old"},
            {"id": "space://local/cua-e2e-claim", "name": "twin"},
        ]))
        .unwrap(),
    )
    .unwrap();
    std::fs::write(
        legacy.path().join("spaces-credentials.json"),
        serde_json::to_vec(&serde_json::json!({"cloud:cua-e2e-claim": {"token": cred.token}}))
            .unwrap(),
    )
    .unwrap();
    let old = Spaces::builder()
        .home(legacy.path())
        .fleet(fake.client_with_base(&srv.url()))
        .build();
    let ids: Vec<String> = old.list().unwrap().into_iter().map(|i| i.id).collect();
    assert_eq!(ids, ["cloud:cua-e2e-claim", "local:cua-e2e-claim"]);
    let e = old.resolve("cua-e2e-claim").unwrap_err();
    assert_eq!(e.tag(), "ambiguous_sandbox", "{e}");
    assert!(
        e.to_string()
            .contains("local:cua-e2e-claim, cloud:cua-e2e-claim"),
        "{e}"
    );
    let reached = old.space("cloud:cua-e2e-claim").await.unwrap();
    // Reached from another machine's registry: the image comes from the
    // claim all the same.
    assert_eq!(reached.image(), Some((IMAGE, "")));
    assert_eq!(
        reached
            .bash("echo legacy", Duration::from_secs(10))
            .await
            .unwrap()
            .stdout,
        "legacy\n"
    );

    // Delete deletes the claim (stops metering) and unregisters.
    spaces.delete(&info.id).await.unwrap();
    assert!(!fake.exists("claim", &pool, "cua-e2e-claim"));
    assert!(!fake.exists("secret", &pool, "cua-claim-cua-e2e-claim"));
    assert!(spaces.list().unwrap().is_empty());
}

#[tokio::test]
async fn a_runtime_image_mismatch_is_refused_before_any_request() {
    let reg = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    // What the image is comes from its manifest (a fixture here).
    cua_fleet::testing::set_image_variant(IMAGE, cua_fleet::ImageVariant::ContainerDisk);
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client())
        .build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(IMAGE.into()),
            runtime: Runtime::Gvisor,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument");
    assert!(e.to_string().contains("containerDisk"), "{e}");
    assert!(
        fake.requests().is_empty(),
        "nothing claimed, nothing pulled"
    );
}

#[tokio::test]
async fn fleet_tools_without_credentials_name_the_missing_host_capability() {
    let reg = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(reg.path()).build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "host_capability_missing");
    assert!(e.to_string().contains("CUA_CLIENT_ID"));
}

/// A local runtime whose one "instance" is a MockServer on loopback.
struct FakeRuntime {
    port: u16,
    started: Mutex<Vec<LocalStartSpec>>,
    deleted: Mutex<Vec<String>>,
    running: Mutex<BTreeMap<String, bool>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        // What a real runtime reports while it pulls (see cua_vmm::container).
        use cua_sandbox_core::progress::{Phase, Progress, report};
        report(
            Progress::phase(Phase::Pulling)
                .fraction(0.5)
                .detail(&spec.image),
        );
        report(Progress::phase(Phase::Booting));
        self.started.lock().unwrap().push(spec.clone());
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
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn create_locally_starts_with_a_fresh_token_and_delete_deletes() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        started: Mutex::new(vec![]),
        deleted: Mutex::new(vec![]),
        running: Mutex::new(BTreeMap::new()),
    });
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("cua-e2e-local-1".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert_eq!(info.id, "local:cua-e2e-local-1");
    let spec = rt.started.lock().unwrap()[0].clone();
    let token = spec
        .env
        .get("CUA_ENV_TOKEN")
        .cloned()
        .expect("token passed to the guest");
    assert_eq!(token.len(), 32);
    assert!(spec.ports.contains(&3211));
    assert_eq!(
        spaces
            .registry()
            .credential(&info.id)
            .unwrap()
            .unwrap()
            .token,
        Some(token)
    );

    // Reconnects from the registry through the runtime's endpoints.
    let fresh = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    let space = fresh.space(&info.id).await.unwrap();
    assert_eq!(
        space
            .bash("echo local", Duration::from_secs(5))
            .await
            .unwrap()
            .stdout,
        "local\n"
    );

    fresh.delete(&info.id).await.unwrap();
    assert_eq!(rt.deleted.lock().unwrap().as_slice(), ["cua-e2e-local-1"]);
    assert!(fresh.list().unwrap().is_empty());
}

/// `cua spaces create linux` (and `macos:26`, `macos:26-slim`) on this
/// machine: the alias names the canonical image, as it does in the cloud.
/// The local runtime only knows registry references, so before this it
/// tried to pull `linux` from Docker Hub ("pull access denied for linux").
#[tokio::test]
async fn a_local_create_resolves_an_image_alias_to_its_canonical_image() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        started: Mutex::new(vec![]),
        deleted: Mutex::new(vec![]),
        running: Mutex::new(BTreeMap::new()),
    });
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    for (n, alias) in ["linux", "macos:26-slim"].into_iter().enumerate() {
        spaces
            .create(SpaceCreate {
                on: Some(On::Local),
                image: Some(alias.into()),
                name: Some(format!("alias-{n}")),
                timeout: Some(Duration::from_secs(20)),
                ..Default::default()
            })
            .await
            .unwrap()
            .ready()
            .unwrap();
        let spec = rt.started.lock().unwrap()[n].clone();
        let want = cua_image::canonical::alias(alias).unwrap();
        assert!(want.starts_with("ghcr.io/trycua/"), "{want}");
        assert_eq!(spec.image, want, "{alias}");
    }
    // A registry reference is passed through as given.
    spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("literal".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        rt.started.lock().unwrap()[2].image,
        "cua-e2e-local/linux:docker-local-arm64"
    );
}

/// A daemon killed mid-create leaves a started sandbox that no Space
/// lists. The next daemon's `recover_interrupted_creates` registers it when
/// its cua-spacesd answers and deletes it when it does not; creates of live
/// processes and journals with nothing behind them are handled without
/// touching anything else. A finished create leaves no journal.
#[tokio::test]
async fn an_interrupted_local_create_is_registered_or_deleted_never_hidden() {
    use cua_sandbox_core::{CreateOptions, ProviderKind};
    use cua_spaces::RecoveryOutcome;
    let srv = MockServer::start(MockAuth::default()).await;
    // Nothing listens on the second runtime's port: that sandbox never answers.
    let dead_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let rt = |port| {
        Arc::new(FakeRuntime {
            port,
            started: Mutex::new(vec![]),
            deleted: Mutex::new(vec![]),
            running: Mutex::new(BTreeMap::new()),
        })
    };
    let (good, bad) = (rt(srv.addr.port()), rt(dead_port));
    let reg = tempfile::tempdir().unwrap();
    let spaces_on = |r: &Arc<FakeRuntime>, state: &std::path::Path| {
        Spaces::builder()
            .home(reg.path())
            .sandboxes(
                cua_sandbox_core::Sandboxes::builder()
                    .local(r.clone())
                    .state_dir(state)
                    .build(),
            )
            .build()
    };
    let (s1, s2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let spaces = spaces_on(&good, s1.path());
    let journal = |name: &str, pid: u32| {
        let dir = spaces.home_dir().join("creating");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!("{name}.json")),
            serde_json::json!({"name": name, "token": "t0k", "pid": pid, "spacesd": true, "started": 0})
                .to_string(),
        )
        .unwrap();
    };
    let dead_pid = {
        let mut c = std::process::Command::new("true").spawn().unwrap();
        let pid = c.id();
        c.wait().unwrap();
        pid
    };

    // A finished create leaves no journal.
    spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("done".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap();
    let creating = spaces.home_dir().join("creating");
    assert_eq!(std::fs::read_dir(&creating).unwrap().count(), 0);

    // The daemon died after starting "half" and before registering it.
    spaces
        .sandboxes()
        .create(
            CreateOptions::new(
                ProviderKind::Local,
                "cua-e2e-local/linux:docker-local-arm64",
            )
            .name("half"),
        )
        .await
        .unwrap();
    journal("half", dead_pid);
    // Nothing was started for "early"; "busy" belongs to a live process.
    journal("early", dead_pid);
    // "cut" was started by the runtime but never recorded (the daemon died
    // inside the create).
    good.running.lock().unwrap().insert("cut".into(), true);
    journal("cut", dead_pid);
    let mut live = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    journal("busy", live.id());
    assert!(!spaces.list().unwrap().iter().any(|s| s.id == "local:half"));

    let got = spaces
        .recover_interrupted_creates(Duration::from_secs(10))
        .await;
    let _ = live.kill();
    let _ = live.wait();
    let outcome = |id: &str| got.iter().find(|r| r.id == id).map(|r| r.outcome.clone());
    assert_eq!(
        outcome("local:half"),
        Some(RecoveryOutcome::Registered),
        "{got:?}"
    );
    assert_eq!(outcome("local:early"), Some(RecoveryOutcome::NothingLeft));
    assert!(
        matches!(outcome("local:cut"), Some(RecoveryOutcome::Deleted(_))),
        "{got:?}"
    );
    assert_eq!(
        outcome("local:busy"),
        None,
        "a live process's create is left alone"
    );
    assert!(spaces.list().unwrap().iter().any(|s| s.id == "local:half"));
    let half = spaces.space("local:half").await.unwrap();
    assert_eq!(
        half.bash("echo back", Duration::from_secs(5))
            .await
            .unwrap()
            .stdout,
        "back\n"
    );
    assert!(creating.join("busy.json").exists());
    assert!(!creating.join("half.json").exists() && !creating.join("early.json").exists());
    assert_eq!(good.deleted.lock().unwrap().as_slice(), ["cut"]);

    // A sandbox whose cua-spacesd never answers is deleted, not left running.
    let spaces2 = spaces_on(&bad, s2.path());
    spaces2
        .sandboxes()
        .create(
            CreateOptions::new(
                ProviderKind::Local,
                "cua-e2e-local/linux:docker-local-arm64",
            )
            .name("mute"),
        )
        .await
        .unwrap();
    std::fs::remove_file(creating.join("busy.json")).unwrap();
    journal("mute", dead_pid);
    let got = spaces2
        .recover_interrupted_creates(Duration::from_secs(2))
        .await;
    assert!(
        matches!(got.as_slice(), [r] if r.id == "local:mute" && matches!(r.outcome, RecoveryOutcome::Deleted(_))),
        "{got:?}"
    );
    assert_eq!(bad.deleted.lock().unwrap().as_slice(), ["mute"]);
    assert!(!spaces2.list().unwrap().iter().any(|s| s.id == "local:mute"));
    assert_eq!(std::fs::read_dir(&creating).unwrap().count(), 0);
}

/// A create reports what it is doing, in order, ending with `ready`: the
/// runtime's pull and boot, the wait for cua-spacesd, the connect. With
/// `wait = false` the reports keep coming after `create` returns.
#[tokio::test]
async fn create_reports_progress_in_order_and_ends_ready() {
    use cua_spaces::{CreatePhase, CreateProgress, ProgressSink};
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        started: Mutex::new(vec![]),
        deleted: Mutex::new(vec![]),
        running: Mutex::new(BTreeMap::new()),
    });
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    for wait in [true, false] {
        let seen: Arc<Mutex<Vec<CreateProgress>>> = Arc::default();
        let s = seen.clone();
        let created = spaces
            .create(SpaceCreate {
                on: Some(On::Local),
                image: Some(IMAGE.into()),
                name: Some(format!("cua-e2e-progress-{wait}")),
                wait: Some(wait),
                progress: Some(ProgressSink::new(move |p| {
                    s.lock().unwrap().push(p.clone())
                })),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(
            matches!(created, cua_spaces::SpaceCreated::Ready { .. }),
            wait
        );
        // Bounded wait for the background create's last report.
        for _ in 0..200 {
            if seen.lock().unwrap().last().map(|p| p.phase) == Some(CreatePhase::Ready) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let seen = seen.lock().unwrap().clone();
        let phases: Vec<CreatePhase> = seen.iter().map(|p| p.phase).collect();
        assert_eq!(phases.first(), Some(&CreatePhase::Preparing), "{phases:?}");
        assert_eq!(phases.last(), Some(&CreatePhase::Ready), "{phases:?}");
        for want in [
            CreatePhase::Pulling,
            CreatePhase::Booting,
            CreatePhase::WaitingForServices,
            CreatePhase::Connecting,
        ] {
            assert!(phases.contains(&want), "{want:?} missing from {phases:?}");
        }
        assert!(
            phases.windows(2).all(|w| w[0].rank() <= w[1].rank()),
            "phases never go backwards: {phases:?}"
        );
        let pull = seen
            .iter()
            .find(|p| p.phase == CreatePhase::Pulling)
            .unwrap();
        assert_eq!((pull.fraction, pull.detail.as_str()), (Some(0.5), IMAGE));
    }
}

/// Deleting a local Space whose instance is stopped (or unreachable)
/// deletes it: a delete needs no running sandbox.
#[tokio::test]
async fn a_stopped_local_space_deletes() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(FakeRuntime {
        port: srv.addr.port(),
        started: Mutex::new(vec![]),
        deleted: Mutex::new(vec![]),
        running: Mutex::new(BTreeMap::new()),
    });
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("cua-e2e-local-stopped".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    rt.stop("cua-e2e-local-stopped").await.unwrap();
    spaces.delete(&info.id).await.unwrap();
    assert_eq!(
        rt.deleted.lock().unwrap().as_slice(),
        ["cua-e2e-local-stopped"]
    );
    assert!(spaces.list().unwrap().is_empty());
    // The sandbox's state file is gone too.
    let left: Vec<_> = std::fs::read_dir(state.path())
        .unwrap()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .collect();
    assert!(left.is_empty(), "{left:?}");
}

#[tokio::test]
async fn a_cloud_space_gets_the_size_asked_for_within_the_absolute_ceiling() {
    let reg = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client())
        .fleet_namespace("cua-e2e-size")
        .build();
    let created = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(IMAGE.into()),
            name: Some("sized".into()),
            cpus: Some(4),
            memory_mb: Some(8192),
            wait: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!created.reused());
    // Its own pool (an unsized Space keeps the image's), whose template
    // reserves what was asked for: what Fleet meters and bills.
    let plain = spaces.fleet_pool_name(FleetRuntime::Kubevirt, IMAGE);
    assert!(!fake.exists("pool", &plain, &plain));

    let templates: Vec<serde_json::Value> = fake
        .requests()
        .into_iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/osgymsandboxtemplates"))
        .filter_map(|r| r.body)
        .collect();
    assert_eq!(templates.len(), 1, "{templates:?}");
    let vm = &templates[0]["spec"]["vmTemplate"];
    assert_eq!(
        (vm["cpuCores"].as_u64(), vm["memory"].as_str()),
        (Some(4), Some("8192Mi"))
    );

    // Nonsense never reaches Fleet (sizes above the everyday range do:
    // Fleet decides what the account may run).
    let before = fake.requests().len();
    for (cpus, memory_mb) in [
        (Some(65), None),
        (Some(0), None),
        (None, Some(256)),
        (None, Some(1 << 40)),
    ] {
        let e = spaces
            .create(SpaceCreate {
                on: Some(On::Cloud),
                image: Some(IMAGE.into()),
                cpus,
                memory_mb,
                wait: Some(false),
                ..Default::default()
            })
            .await
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(e.contains("a Cua Cloud sandbox has"), "{e}");
    }
    assert_eq!(fake.requests().len(), before);

    // Above the everyday range: written as asked (an exempt account).
    let big = || SpaceCreate {
        on: Some(On::Cloud),
        image: Some(IMAGE.into()),
        cpus: Some(16),
        memory_mb: Some(64 * 1024),
        wait: Some(false),
        ..Default::default()
    };
    spaces.create(big()).await.unwrap();
    let last = fake
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.method == "POST" && r.path.ends_with("/osgymsandboxtemplates"))
        .and_then(|r| r.body)
        .unwrap();
    assert_eq!(last["spec"]["vmTemplate"]["cpuCores"], 16);
    assert_eq!(last["spec"]["vmTemplate"]["memory"], "65536Mi");

    // An account under Fleet's size cap: Fleet's denial, typed, with its
    // message.
    fake.faults.lock().unwrap().size_cap = Some((8, 32 * 1024));
    let e = spaces
        .create(SpaceCreate {
            cpus: Some(12),
            ..big()
        })
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.tag(), "fleet_admission_denied", "{e}");
    assert!(
        e.to_string()
            .contains(cua_fleet::testing::SIZE_LIMIT_MESSAGE),
        "{e}"
    );
}

#[tokio::test]
async fn a_cloud_space_out_of_credit_fails_typed_and_leaves_local_alone() {
    let reg = tempfile::tempdir().unwrap();
    let fake = FakeFleet::new();
    fake.faults.lock().unwrap().credit_exhausted = Some("https://run.cua.ai/billing".into());
    let spaces = Spaces::builder()
        .home(reg.path())
        .fleet(fake.client())
        .fleet_namespace("cua-e2e-credit")
        .build();
    let e = spaces
        .create(SpaceCreate {
            on: Some(On::Cloud),
            image: Some(IMAGE.into()),
            wait: Some(false),
            ..Default::default()
        })
        .await
        .map(|_| ())
        .unwrap_err();
    assert_eq!(e.tag(), "cloud_credit_exhausted");
    assert_eq!(
        e.to_string(),
        "You're out of Cua Cloud credit. Add credit at https://run.cua.ai/billing"
    );
}

/// A local runtime whose start makes the instance, then fails readiness
/// (what a Lume VM whose guest this process cannot reach does).
#[derive(Default)]
struct UnreadyRuntime {
    running: Mutex<BTreeMap<String, bool>>,
    deleted: Mutex<Vec<String>>,
}

#[async_trait]
impl LocalRuntime for UnreadyRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        Err(RuntimeError::Other(
            "Local Network access is not available: cannot reach the VM".into(),
        ))
    }
    async fn stop(&self, name: &str) -> Result<(), RuntimeError> {
        self.running.lock().unwrap().insert(name.into(), false);
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        match self.running.lock().unwrap().get(name) {
            Some(_) => Ok(InstanceStatus::Running),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn delete(&self, name: &str) -> Result<(), RuntimeError> {
        self.deleted.lock().unwrap().push(name.into());
        match self.running.lock().unwrap().remove(name) {
            Some(_) => Ok(()),
            None => Err(RuntimeError::NotFound(name.into())),
        }
    }
    async fn endpoints(&self, name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Err(RuntimeError::NotFound(name.into()))
    }
}

/// A Space whose start fails after its instance exists leaves nothing
/// running that no registry lists: the instance of a generated name is
/// deleted, and the error says why. A name the caller chose is never
/// deleted (it may be an instance the caller already had).
#[tokio::test]
async fn a_space_that_fails_to_start_leaves_no_instance() {
    let rt = Arc::new(UnreadyRuntime::default());
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    let create = |name: Option<&str>| SpaceCreate {
        on: Some(On::Local),
        image: Some(IMAGE.into()),
        name: name.map(Into::into),
        timeout: Some(Duration::from_secs(5)),
        ..Default::default()
    };
    let e = spaces.create(create(None)).await.unwrap_err();
    assert!(e.to_string().contains("Local Network access"), "{e}");
    let deleted = rt.deleted.lock().unwrap().clone();
    assert_eq!(deleted.len(), 1, "{deleted:?}");
    assert!(deleted[0].starts_with("space-"), "{deleted:?}");
    assert!(rt.running.lock().unwrap().is_empty());
    assert!(spaces.list().unwrap().is_empty());

    spaces.create(create(Some("mine"))).await.unwrap_err();
    assert_eq!(rt.deleted.lock().unwrap().len(), 1);
    assert!(rt.running.lock().unwrap().contains_key("mine"));
}

/// `cua sb resume` on a local Space returns once its cua-spacesd answers,
/// as create does: a macOS VM reports running when it has an address and
/// its driver starts 20 to 40 s later, so the next call failed with
/// "transport error (Aborted)".
#[tokio::test]
async fn resume_returns_once_the_spaces_driver_answers() {
    let srv = MockServer::start(MockAuth::default()).await;
    let rt = Arc::new(SlowBootRuntime::start(srv.addr).await);
    let reg = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(reg.path())
        .sandboxes(
            cua_sandbox_core::Sandboxes::builder()
                .local(rt.clone())
                .state_dir(state.path())
                .build(),
        )
        .build();
    let info = spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("cua-e2e-local/linux:docker-local-arm64".into()),
            spacesd: Some(true),
            name: Some("slow".into()),
            timeout: Some(Duration::from_secs(20)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    spaces.sandboxes().suspend("slow").await.unwrap();
    let t = std::time::Instant::now();
    spaces.sandboxes().resume("slow").await.unwrap();
    assert!(
        t.elapsed() >= Duration::from_millis(1400),
        "resume returned before cua-spacesd answered ({:?})",
        t.elapsed()
    );
    // And the Space answers at once.
    let space = spaces.space(&info.id).await.unwrap();
    assert_eq!(
        space
            .bash("echo up", Duration::from_secs(5))
            .await
            .unwrap()
            .stdout,
        "up\n"
    );
}

/// A local runtime whose one instance's cua-spacesd sits behind a gate: a
/// proxy on a fixed port that drops connections while the "guest" boots.
struct SlowBootRuntime {
    port: u16,
    up: Arc<std::sync::atomic::AtomicBool>,
}

impl SlowBootRuntime {
    /// A gate on a fresh port in front of `upstream`.
    async fn start(upstream: std::net::SocketAddr) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let up = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let gate = up.clone();
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                if !gate.load(std::sync::atomic::Ordering::SeqCst) {
                    continue; // dropped: the driver is not up yet
                }
                tokio::spawn(async move {
                    if let Ok(mut out) = tokio::net::TcpStream::connect(upstream).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut out).await;
                    }
                });
            }
        });
        Self { port, up }
    }

    fn set_up(&self, up: bool) {
        self.up.store(up, std::sync::atomic::Ordering::SeqCst);
    }
}

#[async_trait]
impl LocalRuntime for SlowBootRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(&spec.name).await?,
        })
    }
    async fn stop(&self, _name: &str) -> Result<(), RuntimeError> {
        self.set_up(false);
        Ok(())
    }
    async fn suspend(&self, name: &str) -> Result<(), RuntimeError> {
        self.stop(name).await
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        // The VM has an address now; its cua-spacesd answers 1.5 s later.
        let up = self.up.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            up.store(true, std::sync::atomic::Ordering::SeqCst);
        });
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
    async fn status(&self, _name: &str) -> Result<InstanceStatus, RuntimeError> {
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, _name: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn endpoints(&self, _name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: [(3211u16, self.port)].into(),
            ..Default::default()
        })
    }
}
