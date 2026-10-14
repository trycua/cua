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
            for i in 0..=30u64 {
                if let Some((_, e)) = stops.as_ref().filter(|(at, _)| *at == i) {
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
        pull_bytes: Mutex::default(),
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

/// After the host's daemon starts again, the ports of the Spaces it
/// provides directly open again on the same numbers.
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
            "Stopped relay:space-ada00011 (its disk is kept) ({} on Mac mini (spare))",
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
    assert_eq!(off.message, format!("Suspended {id} (its memory is kept)."));
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
/// `GetHostSpacesResponse.creating` answers (it never lists creates).
struct HostSvc(Arc<HostSpacesServer>, bool);

#[tonic::async_trait]
impl pb::host_spaces_service_server::HostSpacesService for HostSvc {
    async fn get_host_spaces(
        &self,
        _req: tonic::Request<pb::GetHostSpacesRequest>,
    ) -> Result<tonic::Response<pb::GetHostSpacesResponse>, tonic::Status> {
        let mut r = self.0.get(&ada()).await;
        if self.1
            && let Ok(r) = &mut r
        {
            r.creating.clear();
        }
        r.map(tonic::Response::new).map_err(|e| to_status(&e))
    }
    async fn create_host_space(
        &self,
        req: tonic::Request<pb::CreateHostSpaceRequest>,
    ) -> Result<tonic::Response<pb::CreateHostSpaceResponse>, tonic::Status> {
        let r = self.0.create(&ada(), req.into_inner()).await;
        r.map(|s| tonic::Response::new(pb::CreateHostSpaceResponse { space: Some(s) }))
            .map_err(|e| to_status(&e))
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
    ));
    tokio::spawn(
        tonic::transport::Server::builder()
            .accept_http1(true)
            .layer(tonic_web::GrpcWebLayer::new())
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
    );
    f.relay.tunnel_service(
        &host,
        "/cua.env.v1.HostSpacesService/",
        &format!("http://{addr}"),
    );
    let other = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder()
        .home(other.path())
        .relay(cua_spaces::RelayAccount::new(
            &f.relay.url,
            Arc::new(StaticToken("ada-token".into())),
        ))
        .build();
    (host, spaces, other)
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
    let seen = Arc::new(Mutex::new(Vec::<cua_spaces::CreateProgress>::new()));
    let s = seen.clone();
    let mut create = on_host("Mac mini (spare)");
    create.progress = Some(cua_spaces::ProgressSink::new(move |p| {
        s.lock().unwrap().push(p.clone())
    }));
    let t = std::time::Instant::now();
    let e = spaces.create(create).await.unwrap_err().to_string();
    assert!(
        t.elapsed() < std::time::Duration::from_secs(10),
        "failed after {:?}",
        t.elapsed()
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

/// The Spaces made on a machine that provides Spaces show on the account's
/// other devices (CUA-1638): the machine puts each on the relay under
/// itself, once, and lists each once itself. Another device lists them "on"
/// that machine with the state the machine reports (CUA-1637: starting,
/// not "Stopped"), and turns them off and on there. Turning "Provide
/// Spaces" off takes them off the relay again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spaces_made_on_a_providing_machine_list_on_other_devices_with_their_state() {
    use cua_spaces::host_spaces::published_machine;
    let f = fixture(HostProfile::Spare).await;
    let local = |name: &str| cua_spaces::SpaceCreate {
        image: Some("linux".into()),
        on: Some(cua_sandbox_core::placement::On::Local),
        name: Some(name.into()),
        wait: Some(true),
        spacesd: Some(true),
        ..Default::default()
    };
    let up = f.spaces.create(local("dev-linux")).await.unwrap();
    let up = up.ready().unwrap().id;
    let off = f.spaces.create(local("old-linux")).await.unwrap();
    let off = off.ready().unwrap().id;
    f.spaces.stop(&off).await.unwrap();

    // Not signed in here: nothing is published.
    assert!(f.server.publish_own().await.unwrap().is_empty());
    f.spaces.set_relay(Some(cua_spaces::RelayAccount::new(
        &f.relay.url,
        Arc::new(StaticToken("ada-token".into())),
    )));
    let host = f.spaces.this_relay_machine_id().unwrap();
    let (up_m, off_m) = (
        published_machine(&host, &up),
        published_machine(&host, &off),
    );
    let mut published = f.server.publish_own().await.unwrap();
    published.sort();
    let mut want = vec![up_m.clone(), off_m.clone()];
    want.sort();
    assert_eq!(published, want);
    // The running one's driver joined its relay machine (the stopped one
    // joins when it is started).
    let attached = f.env.state.relay_attached.lock().unwrap().clone().unwrap();
    assert_eq!(attached.machine_id, up_m);
    // Once: a second pass publishes nothing new.
    assert!(f.server.publish_own().await.unwrap().is_empty());
    let mut on_relay: Vec<String> = cua_host::RelayClient::new(&f.relay.url)
        .unwrap()
        .machines("ada-token")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.host.as_deref() == Some(host.as_str()))
        .map(|m| m.id)
        .collect();
    on_relay.sort();
    assert_eq!(on_relay, want, "under this machine, as Spaces");
    // Here each is listed once, as the local Space; and they are not
    // Spaces it provides to someone (its limit is untouched).
    let here: Vec<String> = f
        .spaces
        .list_all()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(here.contains(&up) && here.contains(&off), "{here:?}");
    assert!(
        !here.iter().any(|id| id.starts_with("relay:space-")),
        "{here:?}"
    );
    let answer = f.server.get(&ada()).await.unwrap();
    let used = answer
        .capacity
        .iter()
        .find(|c| c.resource == "spaces")
        .unwrap()
        .used;
    assert_eq!(used, 0);
    let state_of = |m: &str| {
        answer
            .spaces
            .iter()
            .find(|s| s.relay_machine == m)
            .map(|s| s.state.clone())
    };
    assert_eq!(state_of(&up_m).as_deref(), Some("running"));
    assert_eq!(state_of(&off_m).as_deref(), Some("suspended"));

    // Another of ada's devices.
    let (_host, other, _dir) = client_fixture(&f).await;
    f.relay.set_online(&up_m, true, "1.0.0");
    let row = |list: &[cua_spaces::SpaceInfo], m: &str| {
        list.iter()
            .find(|s| s.id == format!("relay:{m}"))
            .cloned()
            .unwrap_or_else(|| panic!("relay:{m} listed: {list:?}"))
    };
    let listed = other.list_all().await.unwrap();
    let r = row(&listed, &up_m);
    assert_eq!(
        (
            r.host.as_str(),
            r.host_name.as_str(),
            r.power.as_str(),
            r.power_state.as_str()
        ),
        (host.as_str(), "Mac mini (spare)", "stop", "running")
    );
    assert_eq!(r.name, "dev-linux");
    assert_eq!(row(&listed, &off_m).power_state, "suspended");

    // Started from this device: the machine boots it and attaches it. Until
    // its driver is on the relay it is starting, not stopped.
    other.start(&format!("relay:{off_m}")).await.unwrap();
    let attached = f.env.state.relay_attached.lock().unwrap().clone().unwrap();
    assert_eq!(attached.machine_id, off_m);
    let listed = other.list_all().await.unwrap();
    assert_eq!(row(&listed, &off_m).power_state, "starting");
    f.relay.set_online(&off_m, true, "1.0.0");
    let listed = other.list_all().await.unwrap();
    assert_eq!(row(&listed, &off_m).power_state, "running");

    // Stopped from this device.
    other.stop(&format!("relay:{up_m}")).await.unwrap();
    f.relay.set_online(&up_m, false, "1.0.0");
    let listed = other.list_all().await.unwrap();
    assert_eq!(row(&listed, &up_m).power_state, "stopped");

    // The machine goes offline: its Spaces say why they cannot be reached.
    f.relay.set_online(&host, false, "1.0.0");
    let listed = other.list_all().await.unwrap();
    let r = row(&listed, &up_m);
    assert_eq!(
        (r.power_state.as_str(), r.state_reason.as_str()),
        ("", "Mac mini (spare) is offline")
    );
    f.relay.set_online(&host, true, "1.0.0");

    // "Provide Spaces" off: they leave the relay; the Spaces stay here.
    Host::new(&f.home)
        .with_service_manager(Arc::new(FakeServiceManager::default()))
        .configure(HostSettingsChange {
            provide_spaces: Some(false),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(f.server.publish_own().await.unwrap().is_empty());
    assert!(provided_machines(&f).await.is_empty());
    assert!(provided::load_provided(&f.host_dir()).unwrap().is_empty());
    let here: Vec<String> = f.spaces.list().unwrap().into_iter().map(|s| s.id).collect();
    assert!(here.contains(&up) && here.contains(&off), "{here:?}");
}
