//! The host half of "Spaces on your machines", hermetic: a host set up
//! against the fake relay directory (`cua_host::testing::FakeRelay`), a
//! local runtime whose instances are a mock spacesd, and
//! `HostSpacesServer` (what the host's daemon serves). A create checks the
//! host's settings and the caller, keeps the limits, attaches the new Space
//! to the relay machine the caller registered, and every create, delete and
//! refusal is a line in the host's hash-chained Spaces audit. The driver
//! side (relay callers, the desktop kept private) is tested in
//! cua-spacesd-server (`tests/host_spaces.rs`); the whole path in the
//! opt-in e2e (`tests/e2e/run-host-spaces-e2e.sh`).

use async_trait::async_trait;
use cua_host::provided::{self, ProvidedSpace};
use cua_host::service::FakeServiceManager;
use cua_host::testing::FakeRelay;
use cua_host::{Host, HostProfile, HostSettingsChange, SetupOptions, StaticToken};
use cua_proto::env::v1 as pb;
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError,
};
use cua_spaces::Spaces;
use cua_spaces::host_spaces::{HostCaller, HostSpacesServer, to_status};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// A local runtime whose instances are one mock spacesd on loopback.
struct FakeRuntime {
    port: u16,
    started: Mutex<Vec<LocalStartSpec>>,
    deleted: Mutex<Vec<String>>,
    running: Mutex<BTreeMap<String, bool>>,
    paused: Mutex<std::collections::BTreeSet<String>>,
    /// Starts never finish (a boot a cancel cuts off).
    hang: std::sync::atomic::AtomicBool,
    /// Starts fail with this (a container engine that does not answer).
    fail: Mutex<Option<String>>,
    /// Starts first download an image of this many bytes, slowly,
    /// reporting it as create progress (as Lume's pull does).
    pull_bytes: Mutex<Option<u64>>,
    /// The download stops at this step of 30 with this error (Lume's pull
    /// failed, or Lume died under it).
    pull_stops: Mutex<Option<(u64, String)>>,
    /// The download stops only once this turns true (the caller has seen it
    /// running), so a test waits on what the client saw, not on timing.
    pull_stop_when: Mutex<Option<tokio::sync::watch::Receiver<bool>>>,
    /// Egress policies the runtime enforces, by instance (`set_egress`,
    /// and a start's spec).
    egress: Mutex<BTreeMap<String, cua_vmm::egress::EgressPolicy>>,
    /// Checkpoints taken.
    checkpoints: Mutex<std::collections::BTreeSet<String>>,
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(RuntimeError::Other(e));
        }
        let pull = *self.pull_bytes.lock().unwrap();
        if let Some(total) = pull {
            use cua_sandbox_core::progress::{Phase, Progress, Transfer, report};
            let stops = self.pull_stops.lock().unwrap().clone();
            let stop_when = self.pull_stop_when.lock().unwrap().clone();
            for i in 0..=30u64 {
                if let Some((_, e)) = stops.as_ref().filter(|(at, _)| *at == i) {
                    if let Some(mut seen) = stop_when {
                        // Bounded: a client that never sees the download
                        // fails the test's own assertion, not a hang.
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(60),
                            seen.wait_for(|s| *s),
                        )
                        .await;
                    }
                    return Err(RuntimeError::Other(e.clone()));
                }
                let done = total * i / 30;
                report(
                    Progress::phase(Phase::Pulling)
                        .detail("ghcr.io/trycua/macos:26")
                        .fraction(done as f64 / total as f64)
                        .bytes(Transfer {
                            done,
                            total,
                            per_second: Some(total as f64 / 3.0),
                        }),
                );
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            report(Progress::phase(Phase::Creating));
            report(Progress::phase(Phase::Booting));
            // A boot takes a while (a macOS VM's, a minute or more).
            tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
        }
        self.started.lock().unwrap().push(spec.clone());
        if !spec.egress.is_empty() {
            self.egress
                .lock()
                .unwrap()
                .insert(spec.name.clone(), spec.egress.clone());
        }
        self.running.lock().unwrap().insert(spec.name.clone(), true);
        if self.hang.load(std::sync::atomic::Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
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
    async fn set_egress(
        &self,
        name: &str,
        policy: &cua_vmm::egress::EgressPolicy,
    ) -> Result<cua_vmm::egress::Enforcement, RuntimeError> {
        self.egress
            .lock()
            .unwrap()
            .insert(name.into(), policy.clone());
        Ok(cua_vmm::egress::Enforcement::Host)
    }
    async fn egress(&self, name: &str) -> Result<cua_vmm::egress::EgressPolicy, RuntimeError> {
        Ok(self
            .egress
            .lock()
            .unwrap()
            .get(name)
            .cloned()
            .unwrap_or_default())
    }
    /// Pauses in memory, as a container does.
    async fn suspend(&self, name: &str) -> Result<(), RuntimeError> {
        self.paused.lock().unwrap().insert(name.into());
        Ok(())
    }
    fn power_control(&self, _runtime_type: &str) -> Option<cua_sandbox_core::PowerControl> {
        Some(cua_sandbox_core::PowerControl::Suspend)
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        self.paused.lock().unwrap().remove(name);
        self.running.lock().unwrap().insert(name.into(), true);
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints(name).await?,
        })
    }
    async fn checkpoint(&self, name: &str, checkpoint: &str) -> Result<(), RuntimeError> {
        if !self.running.lock().unwrap().contains_key(name) {
            return Err(RuntimeError::NotFound(name.into()));
        }
        self.checkpoints.lock().unwrap().insert(checkpoint.into());
        Ok(())
    }
    async fn delete_checkpoint(&self, _rt: &str, checkpoint: &str) -> Result<(), RuntimeError> {
        if self.checkpoints.lock().unwrap().remove(checkpoint) {
            Ok(())
        } else {
            Err(RuntimeError::NotFound(checkpoint.into()))
        }
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, name: &str) -> Result<InstanceStatus, RuntimeError> {
        if self.paused.lock().unwrap().contains(name) {
            return Ok(InstanceStatus::Paused);
        }
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

struct Fixture {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    relay: FakeRelay,
    env: MockServer,
    rt: Arc<FakeRuntime>,
    spaces: Spaces,
    server: Arc<HostSpacesServer>,
}

/// A host set up as a spare machine (desktop off, provides Spaces).
async fn fixture(profile: HostProfile) -> Fixture {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let mut opts = SetupOptions::relay(&relay.url).profile(profile);
    if profile == HostProfile::Desktop {
        // A desktop that does not provide Spaces (`--no-provide-spaces`;
        // a new setup provides them by default).
        opts.provide_spaces = Some(false);
    }
    opts.name = Some("Mac mini (spare)".into());
    opts.driver_bin = Some(driver);
    Host::new(&home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .setup(opts, &StaticToken("ada-token".into()))
        .await
        .unwrap();
    runtime_fixture(dir, home, relay).await
}

/// A spare machine set up in direct mode on loopback (no relay account):
/// the Spaces it provides are forwarded on 127.0.0.1.
async fn direct_fixture() -> Fixture {
    let relay = FakeRelay::start().await;
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(".cua");
    let driver = dir.path().join("cua-spacesd");
    std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
    let mut opts = SetupOptions::direct("127.0.0.1:0".parse().unwrap()).profile(HostProfile::Spare);
    opts.name = Some("Mac mini (spare)".into());
    opts.driver_bin = Some(driver);
    Host::new(&home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .setup(opts, &cua_host::NoAccount)
        .await
        .unwrap();
    runtime_fixture(dir, home, relay).await
}

/// The host's runtime (instances are one mock spacesd) and its server.
async fn runtime_fixture(
    dir: tempfile::TempDir,
    home: std::path::PathBuf,
    relay: FakeRelay,
) -> Fixture {
    let env = MockServer::start(MockAuth::default()).await;
    env.state.advertise(&["relay_attach"]);
    let rt = Arc::new(FakeRuntime {
        port: env.addr.port(),
        started: Mutex::default(),
        deleted: Mutex::default(),
        running: Mutex::default(),
        paused: Mutex::default(),
        hang: Default::default(),
        fail: Mutex::default(),
        pull_stops: Mutex::default(),
        pull_stop_when: Mutex::default(),
        pull_bytes: Mutex::default(),
        egress: Mutex::default(),
        checkpoints: Mutex::default(),
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
        server: HostSpacesServer::new(spaces.clone()),
        spaces,
        _dir: dir,
        home,
        relay,
        env,
        rt,
    }
}

fn ada() -> HostCaller {
    HostCaller::from_metadata(Some(
        r#"{"account":"ada","email":"ada@example.com","role":"owner","via":"relay"}"#,
    ))
    .unwrap()
}

fn bob() -> HostCaller {
    HostCaller::from_metadata(Some(
        r#"{"account":"bob","email":"bob@example.com","role":"shared","via":"relay"}"#,
    ))
    .unwrap()
}

impl Fixture {
    fn host_dir(&self) -> std::path::PathBuf {
        self.home.join("host")
    }

    fn audit(&self) -> Vec<(String, String, String)> {
        let r = provided::read_audit(&self.host_dir(), 100);
        assert_eq!(r.error, None);
        r.recent
            .into_iter()
            .rev()
            .map(|e| (e.action, e.who, e.space))
            .collect()
    }

    /// What the client does first: a relay machine for the new Space, as
    /// `token`'s account.
    async fn register(&self, token: &str, id: &str) -> pb::AttachRelayRequest {
        let reg = cua_host::RelayClient::new(&self.relay.url)
            .unwrap()
            .register(
                token,
                &cua_host::relay::RegisterRequest {
                    id: id.into(),
                    name: id.into(),
                    allow: vec![],
                    host: None,
                    meta: Default::default(),
                },
            )
            .await
            .unwrap();
        pb::AttachRelayRequest {
            relay_url: self.relay.url.clone(),
            machine_token: reg.machine_token,
            machine_id: id.into(),
            relay_jwks_json: reg.jwks.to_string(),
            owner: reg.machine.owner.id,
            owner_email: reg.machine.owner.email.unwrap_or_default(),
        }
    }

    async fn create(
        &self,
        who: &HostCaller,
        image: &str,
        attach: pb::AttachRelayRequest,
    ) -> cua_spaces::Result<pb::HostSpace> {
        self.server
            .create(
                who,
                pb::CreateHostSpaceRequest {
                    image: image.into(),
                    attach: Some(attach),
                    ..Default::default()
                },
            )
            .await
    }
}

#[tokio::test]
async fn the_host_creates_attaches_lists_and_deletes_with_an_audit() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("ada-token", "space-ada00001").await;
    let space = f.create(&ada(), "linux", attach.clone()).await.unwrap();
    assert_eq!(space.relay_machine, "space-ada00001");
    assert!(space.local_space.starts_with("local:"), "{space:?}");
    assert!(space.image.contains("linux"), "{}", space.image);
    assert_eq!(space.created_by, "<ada@example.com> (ada)");
    // The runtime started it with a fresh token and the image resolved
    // from its alias.
    let started = f.rt.started.lock().unwrap()[0].clone();
    assert!(started.image.contains("trycua/linux"), "{}", started.image);
    // Its own driver joined the relay machine the caller registered (a
    // loopback relay is named as the guest reaches the host).
    let attached = f.env.state.relay_attached.lock().unwrap().clone().unwrap();
    assert_eq!(attached.machine_id, "space-ada00001");
    assert_eq!(attached.machine_token, attach.machine_token);
    assert!(
        attached.relay_url.contains("host.docker.internal"),
        "{}",
        attached.relay_url
    );

    // Bob (an editor of the host) creates his own and sees only his.
    let bobs = f
        .create(
            &bob(),
            "linux",
            f.register("bob-token", "space-bob00001").await,
        )
        .await
        .unwrap();
    let seen = f.server.get(&bob()).await.unwrap();
    assert_eq!(
        seen.spaces
            .iter()
            .map(|s| s.relay_machine.as_str())
            .collect::<Vec<_>>(),
        ["space-bob00001"]
    );
    assert!(seen.audit.is_empty(), "only the owner reads the audit");
    let owner = f.server.get(&ada()).await.unwrap();
    assert_eq!(owner.name, "Mac mini (spare)");
    let settings = owner.settings.unwrap();
    assert!(!settings.share_desktop && settings.provide_spaces);
    assert_eq!(owner.spaces.len(), 2);
    let spaces_cap = owner
        .capacity
        .iter()
        .find(|c| c.resource == "spaces")
        .unwrap();
    assert_eq!((spaces_cap.used, spaces_cap.limit), (2, 4));

    // Bob cannot delete Ada's; the owner can delete anyone's.
    let e = f
        .server
        .delete(&bob(), &space.relay_machine)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    let msg = f.server.delete(&ada(), &bobs.relay_machine).await.unwrap();
    assert!(msg.contains("Deleted relay:space-bob00001"), "{msg}");
    // Its relay machine went with it (removed with its own machine token).
    assert!(f.relay.machine("space-bob00001").is_none());
    let deleted_name = bobs.local_space.trim_start_matches("local:").to_string();
    assert!(f.rt.deleted.lock().unwrap().contains(&deleted_name));
    f.server
        .delete(&ada(), &format!("relay:{}", space.relay_machine))
        .await
        .unwrap();
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());

    // The host's status (what its apps show) carries the audit.
    let status = Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .status()
        .await
        .unwrap();
    assert!(status.provided_spaces.is_empty());
    let actions: Vec<(String, String, String)> = f.audit();
    assert_eq!(
        actions
            .iter()
            .map(|(a, _, s)| format!("{a} {s}"))
            .collect::<Vec<_>>(),
        [
            "config Mac mini (spare)",
            "create space-ada00001",
            "create space-bob00001",
            "refused space-ada00001",
            "delete space-bob00001",
            "delete space-ada00001",
        ]
    );
    assert_eq!(status.spaces_audit.len(), 6);
    assert_eq!(actions[3].1, "<bob@example.com> (bob)");
}

/// A Space this machine provides is listed once, as its local Space: its
/// relay machine (it joined the relay as its own) is not listed again under
/// the host (CUA-1527). Another device of the account still sees the relay one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_provided_space_is_listed_once_on_the_host_that_provides_it() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("ada-token", "space-ada00001").await;
    let space = f.create(&ada(), "linux", attach).await.unwrap();
    f.spaces.set_relay(Some(cua_spaces::RelayAccount::new(
        &f.relay.url,
        Arc::new(StaticToken("ada-token".into())),
    )));
    let listed = f.spaces.list_all().await.unwrap();
    let ids: Vec<&str> = listed.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&space.local_space.as_str()), "{ids:?}");
    assert!(
        !ids.contains(&"relay:space-ada00001"),
        "the same Space is not listed twice: {ids:?}"
    );

    // Another device of the account has no local copy: it lists the relay one.
    let (_host, other, _dir) = client_fixture(&f).await;
    let theirs = other.list_all().await.unwrap();
    assert!(
        theirs.iter().any(|s| s.id == "relay:space-ada00001"),
        "{:?}",
        theirs.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
}

/// An image whose cua-spacesd predates relay_attach: the create fails
/// saying the image is too old to be reached from other devices and needs
/// republishing, and leaves nothing behind.
#[tokio::test]
async fn an_image_too_old_for_the_relay_says_so_and_is_removed() {
    let f = fixture(HostProfile::Spare).await;
    f.env.state.unadvertise(&["relay_attach"]);
    f.env.state.set_version("0.1.0");
    let attach = f.register("ada-token", "space-ada00001").await;
    let e = f
        .create(&ada(), "linux", attach)
        .await
        .unwrap_err()
        .to_string();
    // It names the version it found and the release to update to (#4480).
    assert!(e.contains("cua-spacesd 0.1.0 is too old"), "{e}");
    assert!(e.contains("cua-spacesd 0.2.2 or newer"), "{e}");
    assert!(
        e.contains("too old to be reached from your other devices"),
        "{e}"
    );
    assert!(e.contains("needs republishing"), "{e}");
    assert!(e.contains("trycua/linux"), "names the image: {e}");
    assert_eq!(
        f.rt.deleted.lock().unwrap().len(),
        1,
        "the Space is deleted"
    );
    assert!(f.server.get(&ada()).await.unwrap().spaces.is_empty());
}

#[tokio::test]
async fn a_host_that_does_not_provide_spaces_refuses_and_says_how() {
    let f = fixture(HostProfile::Desktop).await;
    let e = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-ada00002").await,
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "host_capability_missing", "{e}");
    assert!(e.to_string().contains("--provide-spaces on"), "{e}");
    assert!(f.rt.started.lock().unwrap().is_empty());
    assert_eq!(f.audit().last().unwrap().0, "refused");

    // A viewer (never forwarded by the driver, but refused here too).
    Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .configure(HostSettingsChange {
            provide_spaces: Some(true),
            ..Default::default()
        })
        .await
        .unwrap();
    let viewer = HostCaller::from_metadata(Some(r#"{"account":"eve","role":"viewer"}"#)).unwrap();
    let e = f
        .create(
            &viewer,
            "linux",
            f.register("ada-token", "space-ada00003").await,
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    // Without the relay machine the caller registered there is nothing to
    // attach the Space to.
    let e = f
        .server
        .create(&ada(), pb::CreateHostSpaceRequest::default())
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument", "{e}");
    assert!(f.rt.started.lock().unwrap().is_empty());
}

#[tokio::test]
async fn limits_two_macos_vms_per_mac_and_the_hosts_own() {
    let f = fixture(HostProfile::Spare).await;
    // Two macOS VMs already run on this host.
    let running = |n: u32| ProvidedSpace {
        relay_machine: format!("space-mac0000{n}"),
        local_space: format!("local:mac-{n}"),
        image: "ghcr.io/trycua/macos:26".into(),
        os: "macos".into(),
        created_by_account: "ada".into(),
        ..Default::default()
    };
    provided::save_provided(&f.host_dir(), &[running(1), running(2)]).unwrap();
    // Their sandboxes exist (a provided Space whose sandbox is gone is
    // forgotten when the host reports its status).
    for n in [1, 2] {
        f.rt.running
            .lock()
            .unwrap()
            .insert(format!("mac-{n}"), true);
        f.spaces
            .sandboxes()
            .state()
            .save(&cua_sandbox_core::SandboxState::Local(
                cua_sandbox_core::LocalState {
                    name: format!("mac-{n}"),
                    runtime_type: "fake".into(),
                    status: "running".into(),
                    ..Default::default()
                },
            ))
            .unwrap();
    }
    let e = f
        .create(
            &ada(),
            "macos:26",
            f.register("ada-token", "space-mac00003").await,
        )
        .await
        .unwrap_err();
    if cfg!(target_os = "macos") {
        assert_eq!(e.tag(), "limit_exceeded", "{e}");
        assert!(e.to_string().contains("Apple's macOS license"), "{e}");
        let caps = f.server.get(&ada()).await.unwrap().capacity;
        let mac = caps.iter().find(|c| c.resource == "macos_vms").unwrap();
        assert_eq!((mac.used, mac.limit), (2, 2));
    } else {
        // Only a Mac runs macOS VMs.
        assert_eq!(e.tag(), "host_capability_missing", "{e}");
        assert!(e.to_string().contains("need a Mac host"), "{e}");
    }
    assert!(f.rt.started.lock().unwrap().is_empty(), "nothing started");
    assert_eq!(f.audit().last().unwrap().0, "refused");

    // The host's own limit on every Space.
    Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .configure(HostSettingsChange {
            max_spaces: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    let e = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-lin00001").await,
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "limit_exceeded", "{e}");
    assert!(e.to_string().contains("--max-spaces"), "{e}");
}

/// A provided Space of `memory_mb` and `disk_gb` that exists on the host.
fn provided_of(f: &Fixture, n: u32, memory_mb: u64, disk_gb: u32) -> ProvidedSpace {
    f.rt.running
        .lock()
        .unwrap()
        .insert(format!("limit-{n}"), true);
    f.spaces
        .sandboxes()
        .state()
        .save(&cua_sandbox_core::SandboxState::Local(
            cua_sandbox_core::LocalState {
                name: format!("limit-{n}"),
                runtime_type: "fake".into(),
                status: "running".into(),
                ..Default::default()
            },
        ))
        .unwrap();
    ProvidedSpace {
        relay_machine: format!("space-limit0000{n}"),
        local_space: format!("local:limit-{n}"),
        image: "linux".into(),
        os: "linux".into(),
        created_by_account: "ada".into(),
        memory_mb,
        disk_gb,
        ..Default::default()
    }
}

async fn ask_bob(
    f: &Fixture,
    memory_mb: u64,
    disk_gb: u32,
    attach: pb::AttachRelayRequest,
) -> cua_spaces::Result<pb::HostSpace> {
    f.server
        .create(
            &bob(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                memory_mb,
                disk_gb,
                attach: Some(attach),
                ..Default::default()
            },
        )
        .await
}

async fn configure(f: &Fixture, change: HostSettingsChange) {
    Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .configure(change)
        .await
        .unwrap();
}

/// The owner's limits on the Spaces other machines create here: memory and
/// disk in total. A create past one is refused for any requester with "has
/// no room for another Space"; a limit lowered below what runs stops
/// nothing (it only refuses new ones); the use is reported live.
#[tokio::test]
async fn memory_and_disk_limits_refuse_what_does_not_fit_and_stop_nothing() {
    let f = fixture(HostProfile::Spare).await;
    provided::save_provided(&f.host_dir(), &[provided_of(&f, 1, 6144, 40)]).unwrap();
    configure(
        &f,
        HostSettingsChange {
            max_spaces: Some(0),
            max_memory_gb: Some(8),
            max_disk_gb: Some(100),
            ..Default::default()
        },
    )
    .await;
    // 6 GB are given out of 8: another 4 GB does not fit, whoever asks (an
    // editor of the machine, as an agent or a child Space's overflow would).
    for (who, id) in [(bob(), "space-mem00001"), (ada(), "space-mem00002")] {
        let e = f
            .server
            .create(
                &who,
                pb::CreateHostSpaceRequest {
                    image: "linux".into(),
                    memory_mb: 4096,
                    attach: Some(f.register("ada-token", id).await),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(e.tag(), "limit_exceeded", "{e}");
        let m = e.to_string();
        assert!(m.contains("has no room for another Space"), "{m}");
        assert!(
            m.contains("Mac mini (spare)") && m.contains("--max-memory"),
            "{m}"
        );
    }
    assert!(f.rt.started.lock().unwrap().is_empty(), "nothing started");
    assert_eq!(f.audit().last().unwrap().0, "refused");
    // A smaller one fits exactly (6 + 2 = 8 GB), and the use says so.
    ask_bob(
        &f,
        2048,
        10,
        f.register("ada-token", "space-mem00003").await,
    )
    .await
    .unwrap();
    let caps = f.server.get(&ada()).await.unwrap().capacity;
    let c = |r: &str| {
        caps.iter()
            .find(|c| c.resource == r)
            .map(|c| (c.used, c.limit))
    };
    assert_eq!(c("memory_gb"), Some((8, 8)));
    assert_eq!(c("disk_gb"), Some((50, 100)));
    assert_eq!(c("spaces"), Some((2, 0)));
    // Disk: 50 of 100 GB are given; lowering the limit to 30 stops nothing,
    // shows as over, and refuses the next one.
    configure(
        &f,
        HostSettingsChange {
            max_disk_gb: Some(30),
            max_memory_gb: Some(0),
            ..Default::default()
        },
    )
    .await;
    let caps = f.server.get(&ada()).await.unwrap().capacity;
    assert_eq!(
        caps.iter()
            .find(|c| c.resource == "disk_gb")
            .map(|c| (c.used, c.limit)),
        Some((50, 30))
    );
    let e = ask_bob(&f, 1024, 5, f.register("ada-token", "space-disk0001").await)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "limit_exceeded", "{e}");
    assert!(
        e.to_string().contains("has no room for another Space"),
        "{e}"
    );
    assert!(e.to_string().contains("--max-disk"), "{e}");
    assert!(
        f.rt.deleted.lock().unwrap().is_empty(),
        "nothing was stopped or removed"
    );
    assert_eq!(provided::load_provided(&f.host_dir()).unwrap().len(), 2);
    // No limit (0) gives the room back.
    configure(
        &f,
        HostSettingsChange {
            max_disk_gb: Some(0),
            max_memory_gb: Some(0),
            ..Default::default()
        },
    )
    .await;
    ask_bob(&f, 1024, 5, f.register("ada-token", "space-disk0002").await)
        .await
        .unwrap();
}

/// The owner's Space limit is refused with the same words, and a Space
/// made here by another machine gets the longest running time the owner set.
#[tokio::test]
async fn the_space_limit_says_no_room_and_a_run_time_limit_ends_a_space() {
    let f = fixture(HostProfile::Spare).await;
    provided::save_provided(&f.host_dir(), &[provided_of(&f, 1, 0, 0)]).unwrap();
    configure(
        &f,
        HostSettingsChange {
            max_spaces: Some(1),
            max_run_minutes: Some(5),
            ..Default::default()
        },
    )
    .await;
    let e = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-room00001").await,
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "limit_exceeded", "{e}");
    assert!(
        e.to_string().contains("has no room for another Space"),
        "{e}"
    );
    assert!(e.to_string().contains("--max-spaces"), "{e}");
    configure(
        &f,
        HostSettingsChange {
            max_spaces: Some(3),
            ..Default::default()
        },
    )
    .await;
    let made = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-room00002").await,
        )
        .await
        .unwrap();
    let life = f
        .spaces
        .lifetime(&made.local_space)
        .expect("a running time limit");
    assert_eq!(life.on_expire, "suspend", "turned off, not deleted");
    let left = life
        .expires_at()
        .duration_since(std::time::SystemTime::now())
        .unwrap();
    assert!(
        left > std::time::Duration::from_secs(240) && left <= std::time::Duration::from_secs(300),
        "{left:?}"
    );
    // Without the limit a Space has no lifetime.
    configure(
        &f,
        HostSettingsChange {
            max_run_minutes: Some(0),
            ..Default::default()
        },
    )
    .await;
    let free = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-room00003").await,
        )
        .await
        .unwrap();
    assert!(f.spaces.lifetime(&free.local_space).is_none());
}

/// The owner deleting a provided Space on the host itself takes it off the
/// relay and the host's list too; a host that still provides Spaces is not
/// removed out from under them.
#[tokio::test]
async fn a_local_delete_on_the_host_forgets_the_provided_space() {
    let f = fixture(HostProfile::Spare).await;
    let space = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-ada00009").await,
        )
        .await
        .unwrap();
    let host = Host::new(&f.home).with_service_manager(Arc::new(FakeServiceManager::default()));
    let e = host.remove().await.unwrap_err();
    assert!(e.to_string().contains(&space.local_space), "{e}");
    f.spaces.delete(&space.local_space).await.unwrap();
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    assert!(f.relay.machine("space-ada00009").is_none(), "off the relay");
    assert_eq!(
        f.audit().last().unwrap(),
        &(
            "delete".to_string(),
            "local".to_string(),
            "space-ada00009".to_string()
        )
    );
    host.remove().await.unwrap();
}

/// The caller gives up on a create (`CancelHostSpace`, by the relay machine
/// it registered): the host stops it and removes what it made; one that
/// already finished is deleted. Both are audited.
#[tokio::test]
async fn a_cancelled_host_create_stops_and_removes_what_it_made() {
    let f = fixture(HostProfile::Spare).await;
    f.rt.hang.store(true, std::sync::atomic::Ordering::SeqCst);
    let attach = f.register("ada-token", "space-ada00009").await;
    let server = f.server.clone();
    let create = tokio::spawn(async move {
        server
            .create(
                &ada(),
                pb::CreateHostSpaceRequest {
                    image: "linux".into(),
                    attach: Some(attach),
                    ..Default::default()
                },
            )
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while f.rt.running.lock().unwrap().is_empty() {
        assert!(tokio::time::Instant::now() < deadline, "never started");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let msg = f
        .server
        .cancel(&ada(), "relay:space-ada00009")
        .await
        .unwrap();
    assert!(msg.starts_with("Cancelled"), "{msg}");
    assert_eq!(create.await.unwrap().unwrap_err().tag(), "cancelled");
    assert!(f.rt.running.lock().unwrap().is_empty(), "its VM is gone");
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    // Again: nothing left to cancel.
    let again = f.server.cancel(&ada(), "space-ada00009").await.unwrap();
    assert!(again.contains("No create"), "{again}");

    // One that finished before the cancel arrived is deleted instead.
    f.rt.hang.store(false, std::sync::atomic::Ordering::SeqCst);
    let done = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-ada0000a").await,
        )
        .await
        .unwrap();
    let msg = f.server.cancel(&ada(), &done.relay_machine).await.unwrap();
    assert!(msg.contains("Deleted relay:space-ada0000a"), "{msg}");
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    let actions: Vec<String> = f
        .audit()
        .into_iter()
        .map(|(a, _, s)| format!("{a} {s}"))
        .collect();
    assert!(
        actions.contains(&"cancel space-ada00009".to_string()),
        "{actions:?}"
    );
}

/// F65: a direct Space (no relay machine) whose create finished before the
/// cancel arrived is deleted by `CancelHostSpace` with its create key, as the
/// contract says, not left behind with "already created".
#[tokio::test]
async fn cancelling_a_finished_direct_create_deletes_its_space() {
    let f = direct_fixture().await;
    let local = HostCaller::local();
    let made = f
        .server
        .create(
            &local,
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                direct: true,
                name: "hc-f65".into(),
                create_key: "hc-f65".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(provided::load_provided(&f.host_dir()).unwrap().len(), 1);
    let msg = f.server.cancel(&local, "hc-f65").await.unwrap();
    assert!(!msg.contains("already created"), "{msg}");
    assert!(
        provided::load_provided(&f.host_dir()).unwrap().is_empty(),
        "the Space is deleted: {msg}"
    );
    assert!(
        !f.rt
            .running
            .lock()
            .unwrap()
            .keys()
            .any(|r| made.local_space.ends_with(r.as_str())),
        "its sandbox is gone"
    );
}

/// The owner turns a provided Space off and on (`SetHostSpacePower`): off
/// stops it, freeing the host, even where the runtime could suspend it; on
/// boots it and its driver joins its relay machine again with the machine
/// token the host keeps. Another account cannot touch it; both are audited.
/// A host in direct mode creates a Space for its token holder without the
/// relay: it forwards a port on its direct address to the Space's
/// cua-spacesd and hands back the Space's token; the Space turns off and on
/// with the same port and no relay attach; deleting it closes the port.
/// A relay host refuses a direct create and says how to set one up.
#[tokio::test]
async fn a_host_creates_a_microvm_space_when_asked() {
    let f = direct_fixture().await;
    let local = HostCaller::local();
    f.server
        .create(
            &local,
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                direct: true,
                kind: "microvm".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let started = f.rt.started.lock().unwrap();
    assert_eq!(
        started.last().and_then(|s| s.kind.as_deref()),
        Some("microvm"),
        "the host's runtime is asked for a microVM"
    );
}

#[tokio::test]
async fn a_direct_host_forwards_its_spaces_without_the_relay() {
    let f = direct_fixture().await;
    let local = HostCaller::local();
    let space = f
        .server
        .create(
            &local,
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                direct: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(space.relay_machine.is_empty(), "{space:?}");
    assert!(space.local_space.starts_with("local:"), "{space:?}");
    assert_ne!(space.direct_port, 0);
    assert!(!space.direct_token.is_empty());
    assert!(
        f.env.state.relay_attached.lock().unwrap().is_none(),
        "never attached to the relay"
    );
    // The forwarded port reaches the Space's cua-spacesd.
    let url = format!("http://127.0.0.1:{}", space.direct_port);
    cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&url)
            .unwrap()
            .token(space.direct_token.clone()),
    )
    .await
    .unwrap();
    // Listed with its port; the token only came with the create.
    let got = f.server.get(&local).await.unwrap();
    assert_eq!(got.spaces.len(), 1);
    assert_eq!(got.spaces[0].direct_port, space.direct_port);
    assert!(got.spaces[0].direct_token.is_empty());
    let recorded = provided::load_provided(&f.host_dir()).unwrap();
    assert!(recorded[0].is_direct() && recorded[0].machine_token.is_empty());

    // Off and on: the same port, no relay.
    let off = f
        .server
        .set_power(&local, &space.local_space, false)
        .await
        .unwrap();
    assert_eq!(off.state, "stopped");
    let on = f
        .server
        .set_power(&local, &space.local_space, true)
        .await
        .unwrap();
    assert_eq!(on.state, "running");
    assert!(
        on.message
            .starts_with(&format!("Started {}", space.local_space)),
        "{}",
        on.message
    );
    assert!(f.env.state.relay_attached.lock().unwrap().is_none());
    cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&url)
            .unwrap()
            .token(space.direct_token.clone()),
    )
    .await
    .unwrap();

    // Deleted: the port closes and the audit names the Space.
    let message = f.server.delete(&local, &space.local_space).await.unwrap();
    assert!(message.contains(&space.local_space), "{message}");
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    let mut closed = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", space.direct_port as u16))
            .await
            .is_err()
        {
            closed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(closed, "the forwarded port closed");
    let actions: Vec<String> = f
        .audit()
        .into_iter()
        .map(|(a, _, s)| format!("{a} {s}"))
        .collect();
    assert!(
        actions.contains(&format!("create {}", space.local_space))
            && actions.contains(&format!("delete {}", space.local_space)),
        "{actions:?}"
    );

    // A relay host does not create direct Spaces.
    let relay_host = fixture(HostProfile::Spare).await;
    let e = relay_host
        .server
        .create(
            &ada(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                direct: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "host_capability_missing", "{e}");
    assert!(e.to_string().contains("--direct"), "{e}");
}

/// A relay host creates a routed Space: no relay machine and no direct
/// address, only a forward on its loopback that callers open over the
/// host's channel; it survives a restart and goes with the Space.
#[tokio::test]
async fn a_relay_host_routes_a_space_through_itself() {
    let f = fixture(HostProfile::Spare).await;
    let before = provided_machines(&f).await;
    let space = f
        .server
        .create(
            &ada(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                routed: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(space.relay_machine.is_empty(), "{space:?}");
    assert_ne!(space.direct_port, 0);
    assert!(!space.direct_token.is_empty());
    assert!(
        f.env.state.relay_attached.lock().unwrap().is_none(),
        "never attached to the relay"
    );
    assert_eq!(
        provided_machines(&f).await,
        before,
        "no relay machine registered"
    );
    let recorded = provided::load_provided(&f.host_dir()).unwrap();
    assert!(recorded[0].routed && recorded[0].is_direct());
    // Its forward is on loopback and reaches the Space's cua-spacesd.
    let url = format!("http://127.0.0.1:{}", space.direct_port);
    cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&url)
            .unwrap()
            .token(space.direct_token.clone()),
    )
    .await
    .unwrap();
    // A new daemon opens it again on the same port.
    let port = space.direct_port as u16;
    let Fixture { server, spaces, .. } = f;
    drop(server);
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let server = HostSpacesServer::new(spaces);
    server.restore_direct_forwards().await;
    cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&format!("http://127.0.0.1:{port}"))
            .unwrap()
            .token(space.direct_token.clone()),
    )
    .await
    .unwrap();
}

/// After the host's daemon starts again, the ports of the Spaces it
/// provides directly open again on the same numbers.
/// A Space whose sandbox was removed outside the host service (a container
/// removed with `docker rm`) is no longer listed: whoever placed a durable
/// Space here learns it is gone. A Space that is only stopped stays.
#[tokio::test]
async fn a_provided_space_whose_sandbox_is_gone_is_no_longer_listed() {
    let f = direct_fixture().await;
    let local = HostCaller::local();
    let create = || pb::CreateHostSpaceRequest {
        image: "linux".into(),
        direct: true,
        ..Default::default()
    };
    let kept = f.server.create(&local, create()).await.unwrap();
    let lost = f.server.create(&local, create()).await.unwrap();
    assert_eq!(f.server.get(&local).await.unwrap().spaces.len(), 2);
    // One is stopped (still there), the other removed from under the host.
    let name = |s: &pb::HostSpace| s.local_space.trim_start_matches("local:").to_string();
    f.rt.running.lock().unwrap().insert(name(&kept), false);
    f.rt.running.lock().unwrap().remove(&name(&lost));
    let got = f.server.get(&local).await.unwrap();
    let listed: Vec<_> = got.spaces.iter().map(|s| s.local_space.clone()).collect();
    assert_eq!(listed, vec![kept.local_space.clone()], "{listed:?}");
    let recorded = provided::load_provided(&f.host_dir()).unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(
        f.audit()
            .iter()
            .any(|(action, _, space)| action == "delete" && *space == lost.local_space),
        "{:?}",
        f.audit()
    );
}

#[tokio::test]
async fn a_restarted_direct_host_reopens_its_forwards() {
    let f = direct_fixture().await;
    let space = f
        .server
        .create(
            &HostCaller::local(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                direct: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let port = space.direct_port as u16;
    // A new daemon: the old server (and its forwards) gone.
    let Fixture { server, spaces, .. } = f;
    drop(server);
    let mut gone = false;
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
        {
            gone = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(gone, "the old daemon's forward closed");
    let server = HostSpacesServer::new(spaces);
    server.restore_direct_forwards().await;
    cua_spacesd_client::SpacesdClient::connect(
        cua_spacesd_client::ConnectOptions::parse(&format!("http://127.0.0.1:{port}"))
            .unwrap()
            .token(space.direct_token.clone()),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn the_host_turns_a_provided_space_off_and_on_and_it_rejoins_the_relay() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("ada-token", "space-ada00011").await;
    let space = f.create(&ada(), "linux", attach.clone()).await.unwrap();
    let name = space.local_space.trim_start_matches("local:").to_string();

    let e = f
        .server
        .set_power(&bob(), &space.relay_machine, false)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    assert_eq!(f.rt.running.lock().unwrap().get(&name), Some(&true));

    let off = f
        .server
        .set_power(&ada(), &space.relay_machine, false)
        .await
        .unwrap();
    assert_eq!(
        (off.state.as_str(), off.power.as_str()),
        ("stopped", "stop")
    );
    assert_eq!(
        off.message,
        format!(
            "Stopped relay:space-ada00011 (its disk is kept; its memory is free) ({} on Mac mini (spare))",
            space.local_space
        )
    );
    assert_eq!(f.rt.running.lock().unwrap().get(&name), Some(&false));
    assert!(
        f.rt.paused.lock().unwrap().is_empty(),
        "stopped, not paused"
    );
    assert_eq!(
        f.spaces.power_of(&space.local_space),
        ("suspend".to_string(), "stopped".to_string())
    );

    // Its driver forgot the relay when it stopped.
    *f.env.state.relay_attached.lock().unwrap() = None;
    let on = f
        .server
        .set_power(&ada(), &format!("relay:{}", space.relay_machine), true)
        .await
        .unwrap();
    assert_eq!(on.state, "running");
    assert!(
        on.message.starts_with("Started relay:space-ada00011 ("),
        "{}",
        on.message
    );
    assert_eq!(f.rt.running.lock().unwrap().get(&name), Some(&true));
    let attached = f.env.state.relay_attached.lock().unwrap().clone().unwrap();
    assert_eq!(attached.machine_id, "space-ada00011");
    assert_eq!(attached.machine_token, attach.machine_token);
    assert_eq!(attached.owner, "ada");
    assert_eq!(attached.owner_email, "ada@example.com");
    assert!(
        attached.relay_jwks_json.contains("\"kid\":\"fake\""),
        "{}",
        attached.relay_jwks_json
    );
    assert!(
        attached.relay_url.contains("host.docker.internal"),
        "{}",
        attached.relay_url
    );

    let actions: Vec<String> = f
        .audit()
        .into_iter()
        .map(|(a, _, s)| format!("{a} {s}"))
        .collect();
    assert_eq!(
        actions[actions.len() - 3..],
        [
            "refused space-ada00011",
            "stop space-ada00011",
            "start space-ada00011"
        ]
    );
}

/// On the host itself a local Space suspends and resumes the way its
/// runtime can, `list` says how it was left, and a Space that cannot be
/// turned off says why.
#[tokio::test]
async fn a_local_space_suspends_and_resumes_and_others_refuse() {
    let f = fixture(HostProfile::Spare).await;
    let space = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-ada00012").await,
        )
        .await
        .unwrap();
    let id = space.local_space.clone();
    let name = id.trim_start_matches("local:").to_string();
    let listed = |spaces: &Spaces| {
        let i = spaces
            .list()
            .unwrap()
            .into_iter()
            .find(|i| i.id == id)
            .unwrap();
        (i.power, i.power_state)
    };
    assert_eq!(
        listed(&f.spaces),
        ("suspend".to_string(), "running".to_string())
    );

    let off = f.spaces.stop(&id).await.unwrap();
    assert_eq!(
        (off.space.as_str(), off.state.as_str(), off.power.as_str()),
        (id.as_str(), "suspended", "suspend")
    );
    assert_eq!(
        off.message,
        format!("Suspended {id} (its memory is kept, and still in use).")
    );
    assert!(f.rt.paused.lock().unwrap().contains(&name));
    assert_eq!(
        listed(&f.spaces),
        ("suspend".to_string(), "suspended".to_string())
    );

    let on = f.spaces.start(&id).await.unwrap();
    assert_eq!(on.state, "running");
    assert_eq!(on.message, format!("Resumed {id}."));
    assert!(f.rt.paused.lock().unwrap().is_empty());
    assert_eq!(
        listed(&f.spaces),
        ("suspend".to_string(), "running".to_string())
    );

    // A Space added by address is someone's machine: never.
    f.spaces
        .registry()
        .upsert(
            cua_proto::daemon::v1::Space {
                id: "direct:10.0.0.5:3211".into(),
                name: "studio".into(),
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();
    let e = f.spaces.stop("direct:10.0.0.5:3211").await.unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    assert_eq!(
        e.to_string(),
        "Turning off is not supported for direct (only a Space one of your direct hosts created) Spaces"
    );
    assert_eq!(
        f.spaces.power_of("direct:10.0.0.5:3211"),
        (String::new(), String::new())
    );

    // Suspending on purpose keeps the memory, the same way.
    let off = f.spaces.suspend(&id).await.unwrap();
    assert_eq!(
        (off.state.as_str(), off.power.as_str()),
        ("suspended", "suspend")
    );
    assert!(f.rt.paused.lock().unwrap().contains(&name));
    f.spaces.start(&id).await.unwrap();
    // Only a Space on this computer suspends or resizes.
    let e = f.spaces.suspend("direct:10.0.0.5:3211").await.unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    let e = f
        .spaces
        .resize("direct:10.0.0.5:3211", None, Some(4096), None)
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    // A resize says what to change.
    let e = f.spaces.resize(&id, None, None, None).await.unwrap_err();
    assert!(e.to_string().contains("cpus, memory_mb or disk_gb"), "{e}");
}

/// Your other Mac must not vanish from "Run on" while its service is down
/// (a failed setup, a restart): a machine of yours set up to provide
/// Spaces is listed offline, by name; one set up only to share its desktop
/// is not.
#[tokio::test]
async fn an_offline_machine_set_up_for_spaces_is_listed_by_name() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let dir = tempfile::tempdir().unwrap();
    for (name, profile) in [
        ("Studio", HostProfile::Spare),
        ("Laptop", HostProfile::Desktop),
    ] {
        let home = dir.path().join(name);
        let driver = dir.path().join(format!("{name}-cua-spacesd"));
        std::fs::write(&driver, b"#!/bin/sh\n").unwrap();
        let mut opts = SetupOptions::relay(&relay.url).profile(profile);
        if profile == HostProfile::Desktop {
            // Its desktop only (`--no-provide-spaces`).
            opts.provide_spaces = Some(false);
        }
        opts.name = Some(name.into());
        opts.driver_bin = Some(driver);
        Host::new(&home)
            .with_service_manager(Arc::new(FakeServiceManager::default()))
            .setup(opts, &StaticToken("ada-token".into()))
            .await
            .unwrap();
    }
    let other = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(other.path())
        .relay(cua_spaces::RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .build();
    let hosts = spaces.hosts().await.unwrap();
    let names: Vec<(&str, bool)> = hosts.iter().map(|h| (h.name.as_str(), h.online)).collect();
    assert_eq!(names, [("Studio", false)]);
}

/// The host fixture's `HostSpacesServer` as the host's daemon serves it
/// (every call as ada, the owner: the fake relay does not assert callers).
/// The host's `HostSpacesService`; `old`: as a host that predates
/// `GetHostSpacesResponse.creating` and `CreateHostSpaceRequest.network`
/// answers (it never lists creates, and never reads the network asked for,
/// so it never says which it applied). The third field: a host whose cua
/// predates egress policies (it ignores `CreateHostSpaceRequest.egress` and
/// has no `SetHostSpaceEgress`).
struct HostSvc(Arc<HostSpacesServer>, bool, bool, Option<tonic::Status>);

#[tonic::async_trait]
impl pb::host_spaces_service_server::HostSpacesService for HostSvc {
    async fn get_host_spaces(
        &self,
        req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        let mut r = self.0.get(&ada()).await;
        if self.1
            && let Ok(r) = &mut r
        {
            r.creating.clear();
        }
        // As the daemon answers (an old host does not know the field).
        if req.get_ref().disk
            && !self.1
            && let Ok(r) = &mut r
        {
            self.0.with_disks(&mut r.spaces).await;
        }
        r.map(tonic::Response::new).map_err(|e| to_status(&e))
    }
    async fn create_host_space(
        &self,
        req: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        let mut req = req.into_inner();
        if self.1 {
            // An unknown field: the old host's decoder drops it.
            req.network.clear();
        }
        if self.2 {
            // An older host does not know the field.
            req.egress = None;
        }
        let r = self.0.create(&ada(), req).await;
        r.map(|s| tonic::Response::new(pb::CreateHostSpaceResponse { space: Some(s) }))
            .map_err(|e| to_status(&e))
    }
    async fn set_host_space_egress(
        &self,
        req: tonic::Request<pb::SetHostSpaceEgressRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpaceEgressResponse>, tonic::Status> {
        if self.2 {
            return Err(tonic::Status::unimplemented("SetHostSpaceEgress"));
        }
        let r = req.into_inner();
        let r = self.0.set_egress(&ada(), &r.space, r.egress).await;
        r.map(tonic::Response::new).map_err(|e| to_status(&e))
    }
    async fn delete_host_space(
        &self,
        req: tonic::Request<pb::DeleteHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSpaceResponse>, tonic::Status> {
        let r = self.0.delete(&ada(), &req.into_inner().space).await;
        r.map(|message| tonic::Response::new(pb::DeleteHostSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }
    async fn cancel_host_space(
        &self,
        req: tonic::Request<pb::CancelHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CancelHostSpaceResponse>, tonic::Status> {
        let r = self.0.cancel(&ada(), &req.into_inner().space).await;
        r.map(|message| tonic::Response::new(pb::CancelHostSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }
    async fn set_host_warm(
        &self,
        _: tonic::Request<pb::SetHostWarmRequest>,
    ) -> Result<tonic::Response<pb::SetHostWarmResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("warm pools"))
    }

    async fn set_host_space_power(
        &self,
        req: tonic::Request<pb::SetHostSpacePowerRequest>,
    ) -> Result<tonic::Response<pb::SetHostSpacePowerResponse>, tonic::Status> {
        let r = req.into_inner();
        let r = self.0.set_power(&ada(), &r.space, r.on).await;
        r.map(tonic::Response::new).map_err(|e| to_status(&e))
    }
    async fn delete_cloud_space(
        &self,
        req: tonic::Request<pb::DeleteCloudSpaceRequest>,
    ) -> Result<tonic::Response<pb::DeleteCloudSpaceResponse>, tonic::Status> {
        let r = self.0.delete_cloud(&ada(), &req.into_inner().space).await;
        r.map(|message| tonic::Response::new(pb::DeleteCloudSpaceResponse { message }))
            .map_err(|e| to_status(&e))
    }

    async fn diagnose_host(
        &self,
        _req: tonic::Request<pb::DiagnoseHostRequest>,
    ) -> Result<tonic::Response<pb::DiagnoseHostResponse>, tonic::Status> {
        if let Some(status) = &self.3 {
            return Err(status.clone());
        }
        Ok(tonic::Response::new(pb::DiagnoseHostResponse {
            report: None,
            cua_version: "0.0.0-test".into(),
            os: "macos".into(),
        }))
    }
}

/// The client half: the spare host is online on the fake relay (its driver
/// the mock spacesd, advertising host_spaces; its HostSpacesService the
/// fixture's server, over gRPC-Web), and `spaces` is another of ada's
/// devices, signed in to the relay. Returns the host's machine id.
async fn client_fixture(f: &Fixture) -> (String, Spaces, tempfile::TempDir) {
    client_fixture_of(f, false).await
}

/// [`client_fixture`] with a host that predates listing its creates.
async fn client_fixture_of(f: &Fixture, old_host: bool) -> (String, Spaces, tempfile::TempDir) {
    client_fixture_full(f, old_host, false, None).await
}

/// [`client_fixture_of`], with a host whose cua predates egress policies
/// (`no_egress`).
async fn client_fixture_with(
    f: &Fixture,
    old_host: bool,
    no_egress: bool,
) -> (String, Spaces, tempfile::TempDir) {
    client_fixture_full(f, old_host, no_egress, None).await
}

/// [`client_fixture_of`] with an extension registered on the client.
async fn client_fixture_ext(
    f: &Fixture,
    old_host: bool,
    ext: Option<Arc<dyn cua_spaces::extension::SpacesExtension>>,
) -> (String, Spaces, tempfile::TempDir) {
    client_fixture_full(f, old_host, false, ext).await
}

/// The client side of a host fixture: `old_host` predates listing its
/// creates, `no_egress` predates egress policies, `ext` is registered on
/// the client.
async fn client_fixture_full(
    f: &Fixture,
    old_host: bool,
    no_egress: bool,
    ext: Option<Arc<dyn cua_spaces::extension::SpacesExtension>>,
) -> (String, Spaces, tempfile::TempDir) {
    client_fixture_with_health_error(f, old_host, no_egress, ext, None).await
}

async fn client_fixture_with_health_error(
    f: &Fixture,
    old_host: bool,
    no_egress: bool,
    ext: Option<Arc<dyn cua_spaces::extension::SpacesExtension>>,
    health_error: Option<tonic::Status>,
) -> (String, Spaces, tempfile::TempDir) {
    let relay = cua_host::RelayClient::new(&f.relay.url).unwrap();
    let host = relay
        .machines("ada-token")
        .await
        .unwrap()
        .into_iter()
        .find(|m| m.name == "Mac mini (spare)")
        .expect("the host is on the relay")
        .id;
    f.relay.set_online(&host, true, "1.0.0");
    f.env.state.advertise(&["host_spaces"]);
    f.relay.tunnel(&host, &f.env.url());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let svc = pb::host_spaces_service_server::HostSpacesServiceServer::new(HostSvc(
        f.server.clone(),
        old_host,
        no_egress,
        health_error,
    ));
    let snaps = pb::host_snapshots_service_server::HostSnapshotsServiceServer::new(SnapSvc(
        f.server.clone(),
    ));
    tokio::spawn(
        tonic::transport::Server::builder()
            .accept_http1(true)
            .layer(tonic_web::GrpcWebLayer::new())
            .add_service(svc)
            .add_service(snaps)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    f.relay.tunnel_service(
        &host,
        "/cua.env.v1.HostSpacesService/",
        &format!("http://{addr}"),
    );
    f.relay.tunnel_service(
        &host,
        "/cua.env.v1.HostSnapshotsService/",
        &format!("http://{addr}"),
    );
    let other = tempfile::tempdir().unwrap();
    let mut builder = Spaces::builder()
        .home(other.path())
        .relay(cua_spaces::RelayAccount::new(
            &f.relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ));
    if let Some(ext) = ext {
        builder = builder.extension(ext);
    }
    let spaces = builder.build();
    (host, spaces, other)
}

#[tokio::test]
async fn machine_health_returns_the_hosts_response() {
    let f = fixture(HostProfile::Spare).await;
    let (host, spaces, _other) = client_fixture(&f).await;
    let health = spaces.machine_health(&host).await.unwrap();
    assert_eq!(health.cua_version, "0.0.0-test");
    assert_eq!(health.os, "macos");
    assert!(health.report.is_none());
}

#[tokio::test]
async fn machine_health_unimplemented_does_not_assume_an_old_host() {
    for reason in [
        "providing Spaces from a Windows host is not supported yet",
        "unknown method DiagnoseHost",
    ] {
        let f = fixture(HostProfile::Spare).await;
        let (host, spaces, _other) = client_fixture_with_health_error(
            &f,
            false,
            false,
            None,
            Some(tonic::Status::unimplemented(reason)),
        )
        .await;
        let error = spaces.machine_health(&host).await.unwrap_err();
        assert_eq!(error.tag(), "host_capability_missing");
        assert_eq!(
            error.to_string(),
            "machine health is not available on this host: Mac mini (spare) does not support remote health checks; run `cua doctor --machine this` on that computer"
        );
    }
}

#[tokio::test]
async fn machine_health_preserves_other_rpc_errors() {
    for status in [
        tonic::Status::permission_denied("health check denied"),
        tonic::Status::unavailable("host disconnected"),
    ] {
        let expected = if status.code() == tonic::Code::PermissionDenied {
            cua_spaces::Error::Relay(cua_host::Error::PermissionDenied(status.message().into()))
        } else {
            cua_spaces::Error::Env(cua_spacesd_client::Error::from(status.clone()))
        };
        let f = fixture(HostProfile::Spare).await;
        let (host, spaces, _other) =
            client_fixture_with_health_error(&f, false, false, None, Some(status)).await;
        let error = spaces.machine_health(&host).await.unwrap_err();
        assert_eq!(error.tag(), expected.tag());
        assert_eq!(error.to_string(), expected.to_string());
    }
}

/// The relay machines of ada's account a host provides (its Spaces).
async fn provided_machines(f: &Fixture) -> Vec<String> {
    cua_host::RelayClient::new(&f.relay.url)
        .unwrap()
        .machines("ada-token")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.host.is_some())
        .map(|m| m.id)
        .collect()
}

fn on_host(name: &str) -> cua_spaces::SpaceCreate {
    cua_spaces::SpaceCreate {
        image: Some("linux".into()),
        on: Some(cua_sandbox_core::placement::On::Host(name.into())),
        ..Default::default()
    }
}

/// A create the host fails before the Space exists (its container engine
/// does not answer) leaves no relay machine behind: the client removes the
/// one it registered, so no ghost Space lists forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_the_host_fails_leaves_no_relay_space_behind() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    *f.rt.fail.lock().unwrap() = Some("the Docker engine did not answer ping".into());
    let e = spaces
        .create(on_host("Mac mini (spare)"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("did not answer ping"), "{e}");
    assert!(provided_machines(&f).await.is_empty(), "no ghost Space");
    assert!(
        !spaces
            .list()
            .unwrap()
            .iter()
            .any(|s| s.id.starts_with("relay:space-")),
        "nothing lists"
    );
}

/// A Space's relay entry whose Space the host does not have (left by an
/// older client, or the host was set up again) deletes as a stale entry;
/// one whose host left the relay too. `relay-unregister` takes one off the
/// relay instead of calling it a host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_relay_space_is_removed_by_delete_or_relay_unregister() {
    let f = fixture(HostProfile::Spare).await;
    let (host, spaces, _other) = client_fixture(&f).await;
    let relay = cua_host::RelayClient::new(&f.relay.url).unwrap();
    let register = |id: &'static str, host: String| {
        let relay = relay.clone();
        async move {
            relay
                .register(
                    "ada-token",
                    &cua_host::relay::RegisterRequest {
                        id: id.into(),
                        name: "dogfood-linux".into(),
                        allow: vec![],
                        host: Some(host),
                        meta: Default::default(),
                    },
                )
                .await
                .unwrap();
        }
    };

    // The host answers "not a Space this host provides".
    register("space-d8e2e32864eda9c7", host.clone()).await;
    let msg = spaces.delete("relay:space-d8e2e32864eda9c7").await.unwrap();
    assert!(msg.contains("stale entry"), "{msg}");
    assert!(msg.contains("Mac mini (spare)"), "names the host: {msg}");
    assert!(provided_machines(&f).await.is_empty());

    // By the bare machine id, `relay-unregister` removes it (not "a host").
    register("space-00000000000000a1", host.clone()).await;
    assert!(
        spaces
            .relay_unregister("space-00000000000000a1")
            .await
            .unwrap()
    );
    assert!(provided_machines(&f).await.is_empty());
    // A host itself is still refused, and says how.
    let e = spaces
        .relay_unregister(&format!("relay:{host}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("cua host remove"), "{e}");

    // Its host left the relay (removed, set up again as a new machine).
    register("space-00000000000000a2", host.clone()).await;
    relay.delete("ada-token", &host).await.unwrap();
    let msg = spaces.delete("relay:space-00000000000000a2").await.unwrap();
    assert!(msg.contains("stale entry"), "{msg}");
    assert!(msg.contains("no longer on the relay"), "{msg}");
    assert!(provided_machines(&f).await.is_empty());
}

/// A host that stopped sharing says so (and how to resume) instead of
/// "has no cua-spacesd", for a create on it and for any call to it; the
/// list marks it, and an offline machine, in text and JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_stopped_sharing_says_so_and_lists_its_state() {
    let f = fixture(HostProfile::Spare).await;
    let (host, spaces, _other) = client_fixture(&f).await;
    let relay = cua_host::RelayClient::new(&f.relay.url).unwrap();
    relay.stop_sharing("ada-token", &host).await.unwrap();

    // "Run on" lists it as the Machines page does (connected) and says it
    // is not sharing, instead of calling it offline.
    let hosts = spaces.hosts().await.unwrap();
    let listed = hosts
        .iter()
        .find(|h| h.name == "Mac mini (spare)")
        .expect("still listed");
    assert!(listed.online, "the relay sees it connected: {listed:?}");
    assert!(
        listed
            .limits
            .iter()
            .any(|l| l.resource == cua_spaces::host_spaces::STOPPED_SHARING
                && l.reason.contains("stopped sharing: ask its owner")),
        "{listed:?}"
    );

    let e = spaces
        .create(on_host("Mac mini (spare)"))
        .await
        .unwrap_err();
    let text = e.to_string();
    assert!(
        text.contains(
            "Mac mini (spare) stopped sharing: ask its owner to Resume sharing (or run `cua host start` there)"
        ),
        "{text}"
    );
    assert!(!text.contains("has no cua-spacesd"), "{text}");
    assert_eq!(e.tag(), "permission_denied");
    assert!(provided_machines(&f).await.is_empty(), "nothing registered");

    // Any other call to it, through the relay's refusal.
    let e = spaces
        .space(&format!("relay:{host}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("stopped sharing: ask its owner"), "{e}");
    assert!(!e.contains("has no cua-spacesd"), "{e}");

    let _ = spaces.relay_machines().await.unwrap();
    let row = spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.id == format!("relay:{host}"))
        .unwrap();
    assert_eq!(row.status, "not sharing");
    assert_eq!(serde_json::to_value(&row).unwrap()["status"], "not sharing");
    f.relay.set_online(&host, false, "1.0.0");
    let _ = spaces.relay_machines().await.unwrap();
    let row = spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.id == format!("relay:{host}"))
        .unwrap();
    assert_eq!(row.status, "offline");
    relay.start_sharing("ada-token", &host).await.unwrap();
    f.relay.set_online(&host, true, "1.0.0");
    let _ = spaces.relay_machines().await.unwrap();
    let row = spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|s| s.id == format!("relay:{host}"))
        .unwrap();
    assert_eq!(row.status, "");
    assert!(serde_json::to_value(&row).unwrap().get("status").is_none());
}

/// A create on another machine reports what that machine does while it
/// runs: the image download with its bytes, then the disk and the boot,
/// in order, before the Space is ready. (Before, the call to the host was
/// one silent wait, so the row sat at "Starting… 1%" for the whole
/// download.)
/// Creates a Space on the fixture's host (whose runtime downloads
/// `TOTAL` bytes first) and returns every progress report the client saw.
async fn create_on_host_seeing(f: &Fixture, spaces: &Spaces) -> Vec<cua_spaces::CreateProgress> {
    *f.rt.pull_bytes.lock().unwrap() = Some(TOTAL);
    let seen = Arc::new(Mutex::new(Vec::<cua_spaces::CreateProgress>::new()));
    let s = seen.clone();
    let mut create = on_host("Mac mini (spare)");
    create.progress = Some(cua_spaces::ProgressSink::new(move |p| {
        s.lock().unwrap().push(p.clone())
    }));
    // The new Space's driver comes online on the relay once it is up.
    let relay = f.relay.clone();
    let online = tokio::spawn(async move {
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        loop {
            for m in client.machines("ada-token").await.unwrap_or_default() {
                if m.host.is_some() && !m.online {
                    relay.set_online(&m.id, true, "1.0.0");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    spaces.create(create).await.unwrap();
    online.abort();
    seen.lock().unwrap().clone()
}

const TOTAL: u64 = 30 << 30;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_on_a_host_reports_the_hosts_progress() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    let seen = create_on_host_seeing(&f, &spaces).await;
    let pulls: Vec<_> = seen
        .iter()
        .filter(|p| p.phase == cua_spaces::CreatePhase::Pulling)
        .collect();
    assert!(
        pulls.len() >= 2,
        "the download is reported while it runs: {seen:#?}"
    );
    let bytes = pulls.iter().filter_map(|p| p.bytes).collect::<Vec<_>>();
    assert!(
        bytes.iter().all(|b| b.total == TOTAL),
        "bytes of the host's download: {bytes:?}"
    );
    assert!(
        bytes.windows(2).all(|w| w[0].done <= w[1].done),
        "bytes only go up: {bytes:?}"
    );
    assert!(
        bytes.iter().any(|b| b.per_second.is_some()),
        "with a rate, for the time left"
    );
    assert_eq!(pulls[0].detail, "ghcr.io/trycua/macos:26");
    let ranks: Vec<u8> = seen.iter().map(|p| p.phase.rank()).collect();
    assert!(
        ranks.windows(2).all(|w| w[0] <= w[1]),
        "phases in order: {seen:#?}"
    );
    for phase in [
        cua_spaces::CreatePhase::Booting,
        cua_spaces::CreatePhase::Connecting,
        cua_spaces::CreatePhase::Ready,
    ] {
        assert!(
            seen.iter().any(|p| p.phase == phase),
            "{phase:?} reported: {seen:#?}"
        );
    }
}

/// A host whose image download stops partway fails the caller's create
/// right then, with the host's reason, after showing the download; the
/// host stops listing the create and no Space is left on the relay.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_that_stops_on_the_host_fails_the_create_with_its_reason() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    *f.rt.pull_bytes.lock().unwrap() = Some(TOTAL);
    let why = "The image download stopped before it finished (Lume stopped running). Try \
               again; it picks up where it stopped.";
    *f.rt.pull_stops.lock().unwrap() = Some((10, why.into()));
    // The host's download stops once the client has shown it (state, not a
    // race between the download's speed and the client's polling).
    let (shown, shown_rx) = tokio::sync::watch::channel(false);
    *f.rt.pull_stop_when.lock().unwrap() = Some(shown_rx);
    let shown_at = Arc::new(Mutex::new(None::<std::time::Instant>));
    let seen = Arc::new(Mutex::new(Vec::<cua_spaces::CreateProgress>::new()));
    let (s, at) = (seen.clone(), shown_at.clone());
    let mut create = on_host("Mac mini (spare)");
    create.progress = Some(cua_spaces::ProgressSink::new(move |p| {
        if p.phase == cua_spaces::CreatePhase::Pulling {
            at.lock()
                .unwrap()
                .get_or_insert_with(std::time::Instant::now);
            shown.send_replace(true);
        }
        s.lock().unwrap().push(p.clone())
    }));
    let e = spaces.create(create).await.unwrap_err().to_string();
    // Failed right after the stop, not after a create timeout.
    let since = shown_at
        .lock()
        .unwrap()
        .expect("the download showed")
        .elapsed();
    assert!(
        since < std::time::Duration::from_secs(10),
        "failed {since:?} after the download showed"
    );
    assert!(e.contains(why), "the host's reason: {e}");
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|p| p.phase == cua_spaces::CreatePhase::Pulling),
        "the download showed before it stopped"
    );
    assert!(provided_machines(&f).await.is_empty(), "no ghost Space");
    let creating = f.server.get(&ada()).await.unwrap().creating;
    assert!(creating.is_empty(), "{creating:?}");
}

/// A host that predates listing its creates still creates the Space; the
/// client reports what it does itself (as before: no download to show).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_on_an_older_host_still_works_without_its_progress() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture_of(&f, true).await;
    let seen = create_on_host_seeing(&f, &spaces).await;
    let phases: Vec<_> = seen.iter().map(|p| p.phase).collect();
    assert_eq!(
        phases,
        [
            cua_spaces::CreatePhase::Preparing,
            cua_spaces::CreatePhase::Connecting,
            cua_spaces::CreatePhase::Ready,
        ]
    );
}

/// While a create runs, the host lists it with what it is doing, to the
/// caller who started it and to the owner only, and stops listing it once
/// it ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_host_lists_a_running_create_to_its_caller_and_owner_only() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("bob-token", "space-b0b00001").await;
    *f.rt.pull_bytes.lock().unwrap() = Some(TOTAL);
    let server = f.server.clone();
    let create = tokio::spawn(async move {
        server
            .create(
                &bob(),
                pb::CreateHostSpaceRequest {
                    image: "linux".into(),
                    attach: Some(attach),
                    ..Default::default()
                },
            )
            .await
    });
    let carol =
        HostCaller::from_metadata(Some(r#"{"account":"carol","role":"shared","via":"relay"}"#))
            .unwrap();
    let mut listed = None;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let own = f.server.get(&bob()).await.unwrap().creating;
        if let Some(c) = own
            .into_iter()
            .find(|c| c.phase == "pulling" && c.bytes_done > Some(0))
        {
            listed = Some(c);
            break;
        }
    }
    let c = listed.expect("the caller sees its create downloading");
    assert_eq!(c.key, "space-b0b00001", "keyed by the relay machine");
    assert_eq!(c.bytes_total, Some(TOTAL));
    assert!(c.bytes_per_second.is_some());
    assert_eq!(c.detail, "ghcr.io/trycua/macos:26");
    assert_eq!(
        f.server.get(&ada()).await.unwrap().creating.len(),
        1,
        "the owner sees it"
    );
    assert!(
        f.server.get(&carol).await.unwrap().creating.is_empty(),
        "nobody else does"
    );
    create.await.unwrap().unwrap();
    assert!(
        f.server.get(&ada()).await.unwrap().creating.is_empty(),
        "gone once it ended"
    );
}

/// A provided Space whose sandbox is gone (deleted behind the host's back, or
/// lost when the machine went down) stops reserving capacity: the host
/// forgets its record when it reports its status.
#[tokio::test]
async fn a_provided_space_whose_sandbox_is_gone_is_forgotten() {
    let f = fixture(HostProfile::Spare).await;
    let lost = |n: u32| ProvidedSpace {
        local_space: format!("local:lost-{n}"),
        name: format!("lost-{n}"),
        kind: "vm".into(),
        cpus: 1,
        memory_mb: 1024,
        created_by_account: "ada".into(),
        ..Default::default()
    };
    provided::save_provided(&f.host_dir(), &[lost(1), lost(2)]).unwrap();
    assert_eq!(provided::load_provided(&f.host_dir()).unwrap().len(), 2);
    let node = f.server.node_status().await.expect("status");
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    assert_eq!(
        node.memory_free_mb,
        node.memory_total_mb.min(node.memory_free_mb)
    );
}

/// Brings every new Space's relay machine online, as its driver would
/// once it is up (until aborted).
fn bring_online(f: &Fixture) -> tokio::task::JoinHandle<()> {
    let relay = f.relay.clone();
    tokio::spawn(async move {
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        loop {
            for m in client.machines("ada-token").await.unwrap_or_default() {
                if m.host.is_some() && !m.online {
                    relay.set_online(&m.id, true, "1.0.0");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
}

fn deny_all_but(allow: &[&str]) -> cua_vmm::egress::EgressPolicy {
    cua_vmm::egress::EgressPolicy {
        allow: allow.iter().map(|s| s.to_string()).collect(),
        deny: vec!["all".into()],
    }
}

/// The host side: a create with an egress policy starts the Space with it
/// (the runtime enforces it from the first instruction) and answers where
/// it is enforced; `SetHostSpaceEgress` replaces it, for the owner and the
/// creator only, and both are in the audit.
#[tokio::test]
async fn a_host_starts_a_space_with_its_egress_policy_and_replaces_it() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("ada-token", "space-egress01").await;
    let policy = deny_all_but(&["1.1.1.1"]);
    let space = f
        .server
        .create(
            &ada(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                attach: Some(attach),
                egress: Some(pb::HostEgressPolicy {
                    allow: policy.allow.clone(),
                    deny: policy.deny.clone(),
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(space.egress_enforced, "host");
    let started = f.rt.started.lock().unwrap()[0].clone();
    assert_eq!(
        started.egress, policy,
        "in the start spec: before its first instruction"
    );
    let listed = f.server.get(&ada()).await.unwrap();
    assert_eq!(listed.spaces[0].egress_enforced, "host");
    let local = space.local_space.trim_start_matches("local:").to_string();

    // Someone else's Space: refused, and kept.
    let bob_says = f
        .server
        .set_egress(&bob(), "space-egress01", None)
        .await
        .unwrap_err();
    assert_eq!(bob_says.tag(), "permission_denied", "{bob_says}");
    assert_eq!(f.rt.egress.lock().unwrap()[&local], policy);

    let wider = deny_all_but(&["1.1.1.1", "8.8.8.8"]);
    let r = f
        .server
        .set_egress(
            &ada(),
            "relay:space-egress01",
            Some(pb::HostEgressPolicy {
                allow: wider.allow.clone(),
                deny: wider.deny.clone(),
            }),
        )
        .await
        .unwrap();
    assert_eq!(r.enforced, "host");
    assert_eq!(f.rt.egress.lock().unwrap()[&local], wider);
    // Removed: nothing enforced, the runtime has an empty policy.
    let r = f
        .server
        .set_egress(&ada(), "space-egress01", None)
        .await
        .unwrap();
    assert_eq!(r.enforced, "");
    assert!(f.rt.egress.lock().unwrap()[&local].is_empty());
    let audit = f.audit();
    assert!(
        audit
            .iter()
            .filter(|(a, _, s)| a == "egress" && s == "space-egress01")
            .count()
            == 2,
        "{audit:?}"
    );
}

/// The client side: `on="host:..."` carries the policy to the host, which
/// enforces it; `set_egress` on the relay Space goes to that host.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_on_a_host_takes_its_policy_there() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    let _online = bring_online(&f);
    let policy = deny_all_but(&["1.1.1.1"]);
    let mut opts = on_host("Mac mini (spare)");
    opts.egress = policy.clone();
    let info = spaces.create(opts).await.unwrap().ready().unwrap();
    assert!(info.id.starts_with("relay:"), "{}", info.id);
    assert_eq!(info.egress, policy);
    assert_eq!(
        info.egress_enforced,
        Some(cua_vmm::egress::Enforcement::Host)
    );
    let started = f.rt.started.lock().unwrap().last().cloned().unwrap();
    assert_eq!(started.egress, policy);
    let local = started.name.clone();

    let deny_one = cua_vmm::egress::EgressPolicy {
        allow: vec![],
        deny: vec!["8.8.8.8".into()],
    };
    spaces.set_egress(&info.id, &deny_one).await.unwrap();
    assert_eq!(f.rt.egress.lock().unwrap()[&local], deny_one);
    assert_eq!(spaces.egress(&info.id), deny_one);
    assert_eq!(
        spaces.egress_enforced(&info.id),
        Some(cua_vmm::egress::Enforcement::Host)
    );
}

/// A host whose cua predates egress policies would create the Space
/// without one: the client deletes it and says so, and a policy set later
/// is refused the same way. Never a Space that only looks enforced.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_does_not_enforce_egress_gets_no_space_with_a_policy() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture_with(&f, false, true).await;
    let _online = bring_online(&f);
    let mut opts = on_host("Mac mini (spare)");
    opts.egress = deny_all_but(&["1.1.1.1"]);
    let e = spaces.create(opts).await.unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    assert!(e.to_string().contains("update cua on it"), "{e}");
    assert!(
        provided_machines(&f).await.is_empty(),
        "nothing left on the relay"
    );
    assert_eq!(
        f.rt.deleted.lock().unwrap().len(),
        1,
        "the host deleted the Space it made"
    );

    // Without a policy that host still creates Spaces; a policy set on one
    // later is refused, not ignored.
    let info = spaces
        .create(on_host("Mac mini (spare)"))
        .await
        .unwrap()
        .ready()
        .unwrap();
    let e = spaces
        .set_egress(&info.id, &deny_all_but(&[]))
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "wrong_provider", "{e}");
    assert!(spaces.egress(&info.id).is_empty());
    assert_eq!(spaces.egress_enforced(&info.id), None);
}

/// Stands in for the Cua Volume extension: answers the operations a create
/// with volumes uses, and keeps what it was asked.
struct StubDrive {
    calls: Mutex<Vec<(String, serde_json::Value)>>,
    attach_fails: bool,
}

impl cua_spaces::extension::SpacesExtension for StubDrive {
    fn name(&self) -> &str {
        "drive"
    }
    fn serves(&self, tool: &str) -> bool {
        matches!(tool, "volumes.exists" | "volume.attach")
    }
    fn call_tool<'a>(
        &'a self,
        call: cua_spaces::extension::ToolCall<'a>,
    ) -> cua_spaces::extension::BoxFuture<'a, cua_spaces::Result<cua_spaces::mcp::ToolOutcome>>
    {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push((call.tool.to_string(), call.args.clone()));
            if call.tool == "volume.attach" && self.attach_fails {
                return Err(cua_spaces::Error::invalid("the guest cannot mount"));
            }
            Ok(cua_spaces::mcp::ToolOutcome::json(&serde_json::json!({})))
        })
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

fn mount(name: &str, path: &str, read_only: bool) -> cua_spaces::named_volumes::VolumeMount {
    cua_spaces::named_volumes::VolumeMount {
        name: name.into(),
        path: path.into(),
        read_only,
        subpath: String::new(),
    }
}

/// Named volumes reach a Space on a pool host: the host is not asked for
/// them (it has no Cua Volume), the creating machine records the mounts for
/// the Space's relay id and serves them over its connection; a failed mount
/// removes the Space again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_on_a_host_mounts_volumes_from_this_machine() {
    let f = fixture(HostProfile::Spare).await;
    let stub = Arc::new(StubDrive {
        calls: Mutex::new(vec![]),
        attach_fails: false,
    });
    let (_host, spaces, _other) = client_fixture_ext(&f, false, Some(stub.clone())).await;
    let online = bring_online(&f);
    let mut create = on_host("Mac mini (spare)");
    create.volumes = vec![mount("shared", "/mnt/shared", true)];
    let info = spaces.create(create).await.unwrap().ready().unwrap();
    online.abort();
    assert!(info.id.starts_with("relay:"), "{}", info.id);
    // The mounts are the Space's, here, whichever way it is looked up.
    assert_eq!(info.volumes, vec![mount("shared", "/mnt/shared", true)]);
    assert_eq!(
        spaces.volume_mounts(&info.id).unwrap(),
        vec![mount("shared", "/mnt/shared", true)]
    );
    assert_eq!(
        spaces.volume_users().unwrap()["shared"],
        vec![info.id.clone()]
    );
    let calls = stub.calls.lock().unwrap().clone();
    assert_eq!(calls[0].0, "volumes.exists");
    assert_eq!(calls[0].1["names"], serde_json::json!(["shared"]));
    assert_eq!(calls[1].0, "volume.attach");
    assert_eq!(calls[1].1["space"], serde_json::json!(info.id));
    // The host was not given the mounts and runs the Space as any other.
    assert_eq!(f.rt.started.lock().unwrap().len(), 1);
    // Deleting the Space forgets its mounts.
    spaces.delete(&info.id).await.unwrap();
    assert!(spaces.volume_users().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_space_that_cannot_mount_its_volumes_is_not_kept() {
    let f = fixture(HostProfile::Spare).await;
    let stub = Arc::new(StubDrive {
        calls: Mutex::new(vec![]),
        attach_fails: true,
    });
    let (_host, spaces, _other) = client_fixture_ext(&f, false, Some(stub)).await;
    let relay = f.relay.clone();
    let online = tokio::spawn(async move {
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        loop {
            for m in client.machines("ada-token").await.unwrap_or_default() {
                if m.host.is_some() && !m.online {
                    relay.set_online(&m.id, true, "1.0.0");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    let mut create = on_host("Mac mini (spare)");
    create.volumes = vec![mount("shared", "/mnt/shared", false)];
    let e = spaces.create(create).await.unwrap_err();
    online.abort();
    assert!(e.to_string().contains("cannot mount"), "{e}");
    assert!(spaces.volume_users().unwrap().is_empty());
    let _ = spaces.relay_machines().await;
    assert!(
        spaces.list().unwrap().iter().all(|s| s.volumes.is_empty()),
        "no Space with mounts is left"
    );
}

/// A durable Space has no client to serve a volume to it.
#[tokio::test]
async fn a_durable_space_refuses_volumes() {
    let dir = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(dir.path()).build();
    let e = spaces
        .create(cua_spaces::SpaceCreate {
            on: Some(cua_sandbox_core::placement::On::Auto),
            durable: true,
            volumes: vec![mount("shared", "/mnt/shared", false)],
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("durable"), "{e}");
}

/// The host's `HostSnapshotsService` (listing, taking and deleting
/// checkpoints), every call as ada.
struct SnapSvc(Arc<HostSpacesServer>);

type NoStream<T> =
    std::pin::Pin<Box<dyn tokio_stream::Stream<Item = Result<T, tonic::Status>> + Send>>;

#[tonic::async_trait]
impl pb::host_snapshots_service_server::HostSnapshotsService for SnapSvc {
    async fn list_host_snapshots(
        &self,
        _req: tonic::Request<pb::ListHostSnapshotsRequest>,
    ) -> Result<tonic::Response<pb::ListHostSnapshotsResponse>, tonic::Status> {
        self.0
            .snapshots_list(&ada())
            .await
            .map(tonic::Response::new)
            .map_err(|e| to_status(&e))
    }
    async fn export_host_snapshot(
        &self,
        _req: tonic::Request<pb::ExportHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::ExportHostSnapshotResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not in this test"))
    }
    type PullHostSnapshotStream = NoStream<pb::PullHostSnapshotResponse>;
    async fn pull_host_snapshot(
        &self,
        _req: tonic::Request<pb::PullHostSnapshotRequest>,
    ) -> Result<tonic::Response<Self::PullHostSnapshotStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not in this test"))
    }
    type PublishHostSnapshotStream = NoStream<pb::PublishHostSnapshotResponse>;
    async fn publish_host_snapshot(
        &self,
        _req: tonic::Request<pb::PublishHostSnapshotRequest>,
    ) -> Result<tonic::Response<Self::PublishHostSnapshotStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not in this test"))
    }
    async fn create_host_snapshot(
        &self,
        req: tonic::Request<pb::CreateHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSnapshotResponse>, tonic::Status> {
        self.0
            .snapshot_create(&ada(), req.into_inner())
            .await
            .map(|s| tonic::Response::new(pb::CreateHostSnapshotResponse { snapshot: Some(s) }))
            .map_err(|e| to_status(&e))
    }
    async fn delete_host_snapshot(
        &self,
        req: tonic::Request<pb::DeleteHostSnapshotRequest>,
    ) -> Result<tonic::Response<pb::DeleteHostSnapshotResponse>, tonic::Status> {
        self.0
            .snapshot_delete(&ada(), req.into_inner())
            .await
            .map(|message| tonic::Response::new(pb::DeleteHostSnapshotResponse { message }))
            .map_err(|e| to_status(&e))
    }
}

/// A checkpoint of a Space the host provides is taken on the host
/// (`CreateHostSnapshot`), where it stays: the owner checkpoints any, an
/// editor only its own Spaces, and deletes only the checkpoints it took;
/// the host lists each with its source and audits each step.
#[tokio::test]
async fn the_host_checkpoints_a_provided_space_and_deletes_it_for_its_taker() {
    let f = fixture(HostProfile::Spare).await;
    let ada_space = f
        .create(
            &ada(),
            "linux",
            f.register("ada-token", "space-ada00011").await,
        )
        .await
        .unwrap();
    let local = ada_space
        .local_space
        .strip_prefix("local:")
        .unwrap()
        .to_string();
    let snap = f
        .server
        .snapshot_create(
            &ada(),
            pb::CreateHostSnapshotRequest {
                space: format!("relay:{}", ada_space.relay_machine),
                name: "world".into(),
                pin: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(snap.name, "world");
    assert_eq!(snap.source, local);
    assert_eq!(snap.runtime_type, "fake");
    assert!(snap.created_at_ms > 0);
    let listed = f.server.snapshots_list(&ada()).await.unwrap();
    assert_eq!(
        listed
            .snapshots
            .iter()
            .map(|s| (s.name.as_str(), s.source.as_str()))
            .collect::<Vec<_>>(),
        vec![("world", local.as_str())]
    );
    // The name is taken.
    let e = f
        .server
        .snapshot_create(
            &ada(),
            pb::CreateHostSnapshotRequest {
                space: ada_space.relay_machine.clone(),
                name: "world".into(),
                pin: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument", "{e}");

    // An editor cannot checkpoint someone else's Space, nor delete a
    // checkpoint it did not take.
    let e = f
        .server
        .snapshot_create(
            &bob(),
            pb::CreateHostSnapshotRequest {
                space: ada_space.relay_machine.clone(),
                name: "theirs".into(),
                pin: false,
            },
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("created by someone else"), "{e}");
    let e = f
        .server
        .snapshot_delete(
            &bob(),
            pb::DeleteHostSnapshotRequest {
                name: "world".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("taken by someone else"), "{e}");

    // Its own Space: checkpointed and deleted by it.
    let bob_space = f
        .create(
            &bob(),
            "linux",
            f.register("bob-token", "space-bob00011").await,
        )
        .await
        .unwrap();
    f.server
        .snapshot_create(
            &bob(),
            pb::CreateHostSnapshotRequest {
                space: bob_space.relay_machine.clone(),
                name: "bobs".into(),
                pin: false,
            },
        )
        .await
        .unwrap();
    let m = f
        .server
        .snapshot_delete(
            &bob(),
            pb::DeleteHostSnapshotRequest {
                name: "bobs".into(),
            },
        )
        .await
        .unwrap();
    assert!(m.starts_with("Deleted checkpoint bobs on "), "{m}");

    // A Space the host does not provide, a checkpoint it does not hold.
    let e = f
        .server
        .snapshot_create(
            &ada(),
            pb::CreateHostSnapshotRequest {
                space: "relay:space-nope".into(),
                name: "x".into(),
                pin: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    f.server
        .snapshot_delete(
            &ada(),
            pb::DeleteHostSnapshotRequest {
                name: "world".into(),
            },
        )
        .await
        .unwrap();
    let e = f
        .server
        .snapshot_delete(
            &ada(),
            pb::DeleteHostSnapshotRequest {
                name: "world".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    assert!(f.rt.checkpoints.lock().unwrap().is_empty());
    let actions: Vec<String> = f.audit().into_iter().map(|(a, _, _)| a).collect();
    assert!(
        actions.iter().filter(|a| *a == "checkpoint").count() == 2
            && actions.iter().any(|a| a == "refused"),
        "{actions:?}"
    );
}

/// From another of the owner's devices, a Space a host provides is
/// checkpointed on that host (by its id or its name), the checkpoint is
/// listed with the host it is on, and deleting it deletes it there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_on_another_machine_is_checkpointed_listed_and_deleted_there() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    // The new Space's driver comes online on the relay once it is up.
    let relay = f.relay.clone();
    let online = tokio::spawn(async move {
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        loop {
            for m in client.machines("ada-token").await.unwrap_or_default() {
                if m.host.is_some() && !m.online {
                    relay.set_online(&m.id, true, "1.0.0");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    let info = spaces
        .create(cua_spaces::SpaceCreate {
            name: Some("baker".into()),
            ..on_host("Mac mini (spare)")
        })
        .await
        .unwrap()
        .ready()
        .unwrap();
    online.abort();
    assert!(info.id.starts_with("relay:"), "{info:?}");

    use cua_spaces::pool_checkpoints::SpaceCheckpoint;
    for (by, cp) in [(info.id.as_str(), "world"), ("baker", "world-2")] {
        match spaces.checkpoint_space(by, cp, false).await.unwrap() {
            SpaceCheckpoint::Remote { host, snapshot } => {
                assert_eq!(host, "Mac mini (spare)");
                assert_eq!(snapshot.name, cp);
            }
            other => panic!("checkpointed here: {other:?}"),
        }
    }
    assert_eq!(
        f.rt.checkpoints.lock().unwrap().len(),
        2,
        "taken on the host"
    );

    let pool = spaces.pool_checkpoints().await.unwrap();
    let world = pool
        .iter()
        .find(|c| c.snapshot.name == "world")
        .expect("listed");
    assert_eq!(world.host.as_deref(), Some("Mac mini (spare)"));
    assert!(world.host_id.is_some());
    assert_eq!(
        spaces.checkpoint_holder("world").await.unwrap(),
        Some(world.host_id.clone())
    );

    let on = spaces.delete_checkpoint_everywhere("world").await.unwrap();
    assert_eq!(on, vec!["Mac mini (spare)".to_string()]);
    assert!(!f.rt.checkpoints.lock().unwrap().contains("world"));
    let e = spaces
        .delete_checkpoint_everywhere("world")
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    assert_eq!(spaces.checkpoint_holder("world").await.unwrap(), None);
}

/// `checkpoint:<name>` images: a fork keeps the checkpoint's guest, so
/// what cannot apply is refused by name, a checkpoint no machine holds is
/// not found, and a location that cannot hold one is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_space_from_a_checkpoint_refuses_what_cannot_apply() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    let e = spaces
        .create(cua_spaces::SpaceCreate {
            image: Some("checkpoint:nowhere".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    assert!(
        e.to_string().contains("no machine of yours holds it"),
        "{e}"
    );

    let e = spaces
        .create(cua_spaces::SpaceCreate {
            image: Some("checkpoint:world".into()),
            command: Some(vec!["true".into()]),
            env: [("A".to_string(), "1".to_string())].into(),
            gpu: Some("auto".into()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument", "{e}");
    assert!(
        e.to_string()
            .contains("so it takes no command, environment variables, GPU"),
        "{e}"
    );

    let e = spaces
        .create(cua_spaces::SpaceCreate {
            image: Some("checkpoint:world".into()),
            on: Some(cua_sandbox_core::placement::On::Cloud),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "invalid_argument", "{e}");
}

/// `on_host` with no outbound network, the Space's driver brought online on
/// the relay once it is up (as a real one dials out).
async fn create_on_host_without_network(
    f: &Fixture,
    spaces: &Spaces,
) -> cua_spaces::Result<cua_spaces::SpaceCreated> {
    let mut create = on_host("Mac mini (spare)");
    create.network = cua_sandbox_core::NetworkMode::None;
    let relay = f.relay.clone();
    let online = tokio::spawn(async move {
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        loop {
            for m in client.machines("ada-token").await.unwrap_or_default() {
                if m.host.is_some() && !m.online {
                    relay.set_online(&m.id, true, "1.0.0");
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    });
    let r = spaces.create(create).await;
    online.abort();
    r
}

/// A host whose cua predates `network` would create the Space with outbound
/// network and say nothing: the create is refused, naming the host and what
/// to do, and nothing is left behind (no Space on the host, no relay
/// machine).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_none_on_a_host_too_old_to_enforce_it_is_refused() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture_of(&f, true).await;
    let e = create_on_host_without_network(&f, &spaces)
        .await
        .expect_err("an old host cannot give a Space no network");
    assert_eq!(e.tag(), "capability_missing", "{e}");
    let msg = e.to_string();
    assert!(msg.contains("Mac mini (spare)"), "{msg}");
    assert!(msg.contains("network none"), "{msg}");
    assert!(
        msg.contains("too old") && msg.contains("update cua"),
        "{msg}"
    );
    // The host made it with outbound network, then deleted it on request.
    let started = f.rt.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(!started[0].restrict_network, "the old host never cut it");
    assert_eq!(
        *f.rt.deleted.lock().unwrap(),
        [started[0].name.clone()],
        "deleted on the host"
    );
    assert!(
        f.server.get(&ada()).await.unwrap().spaces.is_empty(),
        "the host provides nothing"
    );
    assert!(provided_machines(&f).await.is_empty(), "no ghost Space");
    // Only the host itself is listed: no Space it provides.
    let listed = spaces.list().unwrap();
    assert!(
        listed.iter().all(|s| s.name == "Mac mini (spare)"),
        "nothing registered: {listed:#?}"
    );
    assert!(listed.len() <= 1, "{listed:#?}");
}

/// A current host applies `network: none` and says so: the create works.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn network_none_on_a_current_host_is_enforced() {
    let f = fixture(HostProfile::Spare).await;
    let (_host, spaces, _other) = client_fixture(&f).await;
    let created = create_on_host_without_network(&f, &spaces)
        .await
        .unwrap()
        .ready()
        .unwrap();
    assert!(created.id.starts_with("relay:"), "{created:?}");
    let started = f.rt.started.lock().unwrap().clone();
    assert_eq!(started.len(), 1, "{started:?}");
    assert!(started[0].restrict_network, "the host cut its network");
    assert!(f.rt.deleted.lock().unwrap().is_empty());
}

/// The host's create response says which network it applied, for the
/// client to check: `none` when asked for, empty for the default.
#[tokio::test]
async fn a_host_says_which_network_it_applied() {
    let f = direct_fixture().await;
    let local = HostCaller::local();
    let mut answered = Vec::new();
    for network in ["none", ""] {
        let space = f
            .server
            .create(
                &local,
                pb::CreateHostSpaceRequest {
                    image: "linux".into(),
                    direct: true,
                    network: network.into(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        answered.push(space.network);
    }
    assert_eq!(answered, ["none", ""]);
    let started = f.rt.started.lock().unwrap();
    assert_eq!(
        started
            .iter()
            .map(|s| s.restrict_network)
            .collect::<Vec<_>>(),
        [true, false]
    );
}

/// The host gives the Space the guest environment of the request (a secret
/// the caller passed), and its own mesh setting wins on a clash.
#[tokio::test]
async fn the_host_applies_the_requested_env() {
    let f = fixture(HostProfile::Spare).await;
    let attach = f.register("ada-token", "space-ada00009").await;
    f.server
        .create(
            &ada(),
            pb::CreateHostSpaceRequest {
                image: "linux".into(),
                attach: Some(attach),
                env: [
                    ("API_KEY".to_string(), "s3cret".to_string()),
                    ("CUA_MESH".to_string(), "hijack".to_string()),
                ]
                .into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let started = f.rt.started.lock().unwrap().last().cloned().unwrap();
    assert_eq!(
        started.env.get("API_KEY").map(String::as_str),
        Some("s3cret")
    );
    assert_ne!(
        started.env.get(cua_mesh::policy::ENV).map(String::as_str),
        Some("hijack")
    );
}

/// [`Fixture::register`] as the client does it: the new Space's relay
/// machine names its host.
async fn register_on(f: &Fixture, host: &str, id: &str) -> pb::AttachRelayRequest {
    let reg = cua_host::RelayClient::new(&f.relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: id.into(),
                allow: vec![],
                host: Some(host.into()),
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
    pb::AttachRelayRequest {
        relay_url: f.relay.url.clone(),
        machine_token: reg.machine_token,
        machine_id: id.into(),
        relay_jwks_json: reg.jwks.to_string(),
        owner: reg.machine.owner.id,
        owner_email: reg.machine.owner.email.unwrap_or_default(),
    }
}

/// Gives provided Space `local` (`local:<name>`) a disk this host can
/// measure: a QEMU VM directory with `mib` MiB in it.
fn give_disk(f: &Fixture, local: &str, mib: usize) {
    let name = local.trim_start_matches("local:");
    let mut fields = serde_json::Map::new();
    fields.insert("runtime_type".into(), "qemu".into());
    f.spaces.sandboxes().state().update(name, fields).unwrap();
    let dir = cua_disk::Layout::new(&f.home).qemu().join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("disk.qcow2"), vec![7u8; mib << 20]).unwrap();
}

/// Sizes on disk for Spaces on your other computers (CUA-2204): the host
/// measures what each of its Spaces takes when asked
/// (`GetHostSpacesRequest.disk`), and another device of the account puts
/// it in its resources under its own id for the Space; nothing for a host
/// that predates the question, or one that is offline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_device_reads_what_its_spaces_take_on_their_host() {
    use cua_spaces::resources::Measure;
    let f = fixture(HostProfile::Spare).await;
    let (host, other, _dir) = client_fixture(&f).await;
    let attach = register_on(&f, &host, "space-ada00007").await;
    let space = f.create(&ada(), "linux", attach).await.unwrap();
    // Not asked: the host measures nothing.
    let listed = f.server.get(&ada()).await.unwrap();
    assert!(listed.spaces.iter().all(|s| s.disk.is_none()));
    // Asked: a Space it can measure says what it takes.
    give_disk(&f, &space.local_space, 6);
    let mut spaces = listed.spaces.clone();
    f.server.with_disks(&mut spaces).await;
    let d = spaces[0].disk.expect("measured");
    assert!(d.allocated >= 6 << 20, "{d:?}");
    assert_eq!(d.bytes, Some(d.allocated));

    let ask = Measure {
        disk: true,
        guests: false,
    };
    let listed = other.list_all().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|s| s.id == "relay:space-ada00007" && s.host == host)
    );
    let r = other.resources_now(ask).await;
    let mine = r
        .spaces
        .iter()
        .find(|s| s.id == "relay:space-ada00007")
        .expect("keyed by this device's id for it");
    assert_eq!(mine.disk_on_host.unwrap().bytes, d.bytes);
    // Only its disk is known; this computer's own totals leave it out.
    assert!(mine.usage.is_none());
    assert_eq!(r.this_mac.spaces_count, 0);
    // Without `disk`, nobody is asked.
    let quick = other
        .resources_now(Measure {
            disk: false,
            guests: false,
        })
        .await;
    assert!(quick.spaces.is_empty());

    // The host goes offline: its Spaces get no size.
    f.relay.set_online(&host, false, "1.0.0");
    other.list_all().await.unwrap();
    let r = other.resources_now(ask).await;
    assert!(r.spaces.is_empty(), "{:?}", r.spaces);
}

/// A host that predates `GetHostSpacesRequest.disk` answers without sizes:
/// its Spaces get none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_that_predates_sizes_gives_none() {
    use cua_spaces::resources::Measure;
    let f = fixture(HostProfile::Spare).await;
    let (host, other, _dir) = client_fixture_of(&f, true).await;
    let attach = register_on(&f, &host, "space-ada00008").await;
    let space = f.create(&ada(), "linux", attach).await.unwrap();
    give_disk(&f, &space.local_space, 2);
    let listed = other.list_all().await.unwrap();
    assert!(
        listed
            .iter()
            .any(|s| s.id == "relay:space-ada00008" && s.host == host)
    );
    let r = other
        .resources_now(Measure {
            disk: true,
            guests: false,
        })
        .await;
    assert!(r.spaces.is_empty(), "{:?}", r.spaces);
}
