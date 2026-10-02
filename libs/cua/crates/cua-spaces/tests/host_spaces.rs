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
use cua_spaces::host_spaces::{HostCaller, HostSpacesServer};
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
}

#[async_trait]
impl LocalRuntime for FakeRuntime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
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
