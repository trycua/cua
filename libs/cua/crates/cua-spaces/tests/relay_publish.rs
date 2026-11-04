//! Spaces running on this computer are listed on the signed-in account's
//! relay on their own, so the phone and the owner's other computers see and
//! open them: any kind of Space (macOS VM, QEMU VM, container, microVM), made
//! by any path, or already here. Hermetic: the fake relay and one mock
//! cua-spacesd per Space; nothing starts a VM or a container.

use async_trait::async_trait;
use cua_host::relay::AccountTokens;
use cua_host::testing::FakeRelay;
use cua_sandbox_core::placement::{Kind, On};
use cua_sandbox_core::{
    InstanceStatus, LocalEndpoints, LocalInstance, LocalRuntime, LocalStartSpec, LocalSummary,
    RuntimeError, Sandboxes,
};
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, SpaceCreate, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A local runtime whose Space `name` has its cua-spacesd at a loopback
/// port the test chose.
#[derive(Default)]
struct Runtime {
    spacesd: Mutex<BTreeMap<String, u16>>,
}

#[async_trait]
impl LocalRuntime for Runtime {
    fn backend(&self) -> String {
        "fake".into()
    }
    async fn start(&self, spec: &LocalStartSpec) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: spec.name.clone(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints_of(&spec.name),
        })
    }
    async fn stop(&self, _: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn resume(&self, name: &str) -> Result<LocalInstance, RuntimeError> {
        Ok(LocalInstance {
            name: name.into(),
            backend: "fake".into(),
            status: InstanceStatus::Running,
            endpoints: self.endpoints_of(name),
        })
    }
    async fn list(&self) -> Result<Vec<LocalSummary>, RuntimeError> {
        Ok(vec![])
    }
    async fn status(&self, _: &str) -> Result<InstanceStatus, RuntimeError> {
        Ok(InstanceStatus::Running)
    }
    async fn delete(&self, _: &str) -> Result<(), RuntimeError> {
        Ok(())
    }
    async fn endpoints(&self, name: &str) -> Result<LocalEndpoints, RuntimeError> {
        Ok(self.endpoints_of(name))
    }
}

impl Runtime {
    fn endpoints_of(&self, name: &str) -> LocalEndpoints {
        let port = self.spacesd.lock().unwrap().get(name).copied();
        LocalEndpoints {
            host: "127.0.0.1".into(),
            ports: port.map(|p| (3211u16, p)).into_iter().collect(),
            ..Default::default()
        }
    }
}

/// An account that can be signed out and in again.
struct Account(Mutex<Option<String>>);
#[async_trait]
impl AccountTokens for Account {
    async fn access_token(&self) -> cua_host::Result<String> {
        self.0
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| cua_host::Error::Unauthenticated("signed out".into()))
    }
    async fn signed_in(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }
}

struct World {
    relay: FakeRelay,
    rt: Arc<Runtime>,
    state: tempfile::TempDir,
    home: tempfile::TempDir,
    drivers: Mutex<BTreeMap<String, MockServer>>,
    account: Arc<Account>,
    spaces: Spaces,
    stop: Arc<AtomicBool>,
}

impl Drop for World {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl World {
    async fn new() -> World {
        let relay = FakeRelay::start().await;
        relay.add_account("ada-token", "ada", Some("ada@example.com"));
        relay.add_account("eve-token", "eve", Some("eve@example.com"));
        let rt = Arc::new(Runtime::default());
        let state = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let account = Arc::new(Account(Mutex::new(Some("ada-token".into()))));
        let spaces = Spaces::builder()
            .home(home.path())
            .sandboxes(
                Sandboxes::builder()
                    .local(rt.clone())
                    .state_dir(state.path())
                    .build(),
            )
            .relay(RelayAccount::new(&relay.url, account.clone()))
            .build();
        World {
            relay,
            rt,
            state,
            home,
            drivers: Mutex::new(BTreeMap::new()),
            account,
            spaces,
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    /// A cua-spacesd for the Space `name`, which "dials the relay" when
    /// asked to attach.
    async fn driver(&self, name: &str) {
        let d = MockServer::start(MockAuth::default()).await;
        d.state.advertise(&["relay_attach"]);
        let port: u16 = d.url().rsplit(':').next().unwrap().parse().unwrap();
        self.rt.spacesd.lock().unwrap().insert(name.into(), port);
        let state = d.state.clone();
        let relay = self.relay.clone();
        let stop = self.stop.clone();
        tokio::spawn(async move {
            let mut last: Option<String> = None;
            while !stop.load(Ordering::SeqCst) {
                let now = state.relay_attached.lock().unwrap().clone();
                match (now, &last) {
                    (Some(a), _) => {
                        relay.set_online(&a.machine_id, true, "0.1.0");
                        last = Some(a.machine_id);
                    }
                    // The driver left: the relay lists the machine offline.
                    (None, Some(m)) => {
                        relay.set_online(m, false, "0.1.0");
                        last = None;
                    }
                    (None, None) => {}
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        });
        self.drivers.lock().unwrap().insert(name.into(), d);
    }

    fn attached(&self, name: &str) -> bool {
        self.drivers.lock().unwrap()[name]
            .state
            .relay_attached
            .lock()
            .unwrap()
            .is_some()
    }

    /// The owner's relay rows that are Spaces Cua registered.
    async fn rows(&self) -> Vec<cua_host::Machine> {
        cua_host::RelayClient::new(&self.relay.url)
            .unwrap()
            .machines("ada-token")
            .await
            .unwrap()
            .into_iter()
            .filter(|m| m.id.starts_with("space-"))
            .collect()
    }

    async fn create(&self, name: &str, image: &str, kind: Kind) -> String {
        self.driver(name).await;
        self.spaces
            .create(SpaceCreate {
                on: Some(On::Local),
                image: Some(image.into()),
                kind,
                name: Some(name.into()),
                timeout: Some(Duration::from_secs(30)),
                ..Default::default()
            })
            .await
            .unwrap()
            .ready()
            .unwrap()
            .id
    }

    /// What the recorded state of the Space `name` says (a runtime turning
    /// it off or on).
    fn power(&self, name: &str, status: &str) {
        let path = self.state.path().join(format!("{name}.json"));
        let mut v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        v["status"] = status.into();
        std::fs::write(path, v.to_string()).unwrap();
    }
}

/// Every kind of Space runs the same rule: a running Space is on the
/// account once a pass runs, a stopped one leaves it, and deleting takes
/// its row with it.
#[tokio::test]
async fn every_kind_of_running_space_is_listed_and_unlisted_alike() {
    let w = World::new().await;
    let kinds = [
        ("mac", "macos", Kind::Vm),
        ("qemu", "ubuntu", Kind::Vm),
        ("box", "linux", Kind::Container),
        ("micro", "linux", Kind::MicroVm),
    ];
    let mut ids = vec![];
    for (name, image, kind) in kinds {
        ids.push((name, w.create(name, image, kind).await));
    }
    assert!(w.rows().await.is_empty(), "nothing yet: no pass has run");

    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published.len(), 4, "{r:?}");
    let rows = w.rows().await;
    assert_eq!(rows.len(), 4);
    // The owner's alone: shared with nobody, and another account sees none.
    assert!(rows.iter().all(|m| m.role == "owner"
        && m.allow.is_empty()
        && m.viewers.is_empty()
        && m.host.is_none()));
    assert!(
        cua_host::RelayClient::new(&w.relay.url)
            .unwrap()
            .machines("eve-token")
            .await
            .unwrap()
            .is_empty()
    );
    for (name, _) in &ids {
        assert!(w.attached(name), "{name}'s driver joined the relay");
    }
    // A second pass changes nothing.
    let again = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(again, Default::default());

    // The local Space is listed once, not again as its relay machine.
    w.spaces.relay_machines().await.unwrap();
    let listed = w.spaces.list().unwrap();
    assert_eq!(
        listed.iter().filter(|i| i.id.starts_with("relay:")).count(),
        0,
        "{listed:#?}"
    );
    assert_eq!(
        w.spaces
            .relay_states(&listed)
            .values()
            .filter(|s| **s == "on")
            .count(),
        4
    );

    // Stopped: it leaves the account.
    w.power("mac", "stopped");
    w.power("qemu", "suspended");
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.unpublished.len(), 2, "{r:?}");
    assert_eq!(w.rows().await.len(), 2);
    assert!(!w.attached("mac") && !w.attached("qemu"));
    // Started again: it is back.
    w.power("mac", "running");
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published.len(), 1, "{r:?}");
    assert_eq!(w.rows().await.len(), 3);

    // Deleted: its row goes with it.
    w.spaces.delete(&ids[2].1).await.unwrap();
    assert_eq!(w.rows().await.len(), 2);
}

/// Only what Cua made is listed, and only if it did not opt out.
#[tokio::test]
async fn spaces_added_by_address_and_opted_out_ones_stay_off() {
    let w = World::new().await;
    // Added by address: not made by Cua.
    let by_hand = MockServer::start(MockAuth::default()).await;
    by_hand.state.advertise(&["relay_attach"]);
    w.spaces
        .add(&by_hand.url(), None, Some("by-hand".into()))
        .await
        .unwrap();

    // `--no-relay` (the option) and `--label cua.relay=off` (the label every
    // path carries), and a child of an opted-out Space.
    w.driver("quiet").await;
    w.driver("labelled").await;
    w.driver("child").await;
    w.driver("open").await;
    let make = |name: &str, relay: Option<bool>, labels: &[(&str, &str)], parent: Option<&str>| {
        let spaces = w.spaces.clone();
        let opts = SpaceCreate {
            on: Some(On::Local),
            image: Some("linux".into()),
            name: Some(name.into()),
            timeout: Some(Duration::from_secs(30)),
            relay,
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            parent: parent.map(str::to_string),
            ..Default::default()
        };
        async move { spaces.create(opts).await.unwrap().ready().unwrap().id }
    };
    let quiet = make("quiet", Some(false), &[], None).await;
    make("labelled", None, &[("cua.relay", "off")], None).await;
    make("child", None, &[], Some(&quiet)).await;
    make("open", None, &[], None).await;

    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published, ["local:open"], "{r:?}");
    assert_eq!(w.rows().await.len(), 1);
    assert!(!w.attached("quiet") && !w.attached("labelled") && !w.attached("child"));
    assert!(by_hand.state.relay_attached.lock().unwrap().is_none());

    // `cua spaces ls` says which stay off.
    let states = w.spaces.relay_states(&w.spaces.list().unwrap());
    assert_eq!(states["local:quiet"], "off");
    assert_eq!(states["local:labelled"], "off");
    assert_eq!(states["local:child"], "off");
    assert_eq!(states["local:open"], "on");
    assert!(!states.contains_key("direct:by-hand"));

    // Opting out later takes it off at once; opting in lists it again.
    w.spaces.set_on_relay("local:open", false).await.unwrap();
    assert!(w.rows().await.is_empty());
    assert!(!w.attached("open"));
    w.spaces.set_on_relay("local:open", true).await.unwrap();
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 1);
}

/// A Space that was already here before this release is listed once it
/// runs, whichever way it came to be.
#[tokio::test]
async fn a_space_that_was_already_here_is_listed() {
    let w = World::new().await;
    let id = w.create("old", "linux", Kind::Container).await;
    // Its record predates the setting: nothing but the registry entry.
    w.spaces
        .registry()
        .update_meta(&id, |m| *m = Default::default())
        .unwrap();
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published, [id], "{r:?}");
}

/// The switch, the sign-out and a share.
#[tokio::test]
async fn the_switch_and_the_sign_out_take_spaces_off_and_back_on() {
    let w = World::new().await;
    w.create("a", "linux", Kind::Container).await;
    w.create("b", "linux", Kind::Container).await;
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 2);

    // Signed out: no account to call the relay with, so each driver just
    // stops dialing; the rows wait for the next sign-in.
    *w.account.0.lock().unwrap() = None;
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert!(r.published.is_empty() && r.unpublished.is_empty());
    assert!(
        !w.attached("a") && !w.attached("b"),
        "drivers left the relay"
    );
    // Not repeated every pass.
    w.spaces.publish_to_relay().await.unwrap();
    // The relay lists a machine whose driver left as offline.
    tokio::time::sleep(Duration::from_millis(100)).await;

    *w.account.0.lock().unwrap() = Some("ada-token".into());
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published.len(), 2, "{r:?}");
    assert!(w.attached("a") && w.attached("b"));
    assert_eq!(w.rows().await.len(), 2, "the same two rows, not four");

    // The switch: `relay.spaces = off` (config file; the environment does
    // the same) takes them off the account.
    std::fs::write(
        w.home.path().join("config.toml"),
        "[relay]\nspaces = \"off\"\n",
    )
    .unwrap();
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.unpublished.len(), 2, "{r:?}");
    assert!(w.rows().await.is_empty());
    assert!(!w.attached("a"));
    std::fs::write(
        w.home.path().join("config.toml"),
        "[relay]\nspaces = \"on\"\n",
    )
    .unwrap();
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 2);

    // A Space the owner shared with someone stays as shared when it stops.
    let row = w.rows().await.remove(0);
    let name = if w.attached("a") && row.name.contains('a') {
        "a"
    } else {
        "b"
    };
    let machine = row.id.clone();
    cua_host::RelayClient::new(&w.relay.url)
        .unwrap()
        .patch(
            "ada-token",
            &machine,
            &cua_host::MachinePatch {
                allow: Some(vec!["bob@example.com".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let stopped = w
        .spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|i| w.spaces.registry().meta(&i.id).is_some() && i.name == row.name)
        .map(|i| i.id)
        .unwrap_or_else(|| format!("local:{name}"));
    w.power(stopped.trim_start_matches("local:"), "stopped");
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.unpublished.len(), 0, "{r:?}");
    assert!(w.relay.machine(&machine).is_some());
}

/// A static token is enough: no host setup is needed to be listed.
#[tokio::test]
async fn nothing_is_listed_without_an_account() {
    let w = World::new().await;
    w.create("lonely", "linux", Kind::Container).await;
    let bare = Spaces::builder()
        .home(w.home.path())
        .sandboxes(
            Sandboxes::builder()
                .local(w.rt.clone())
                .state_dir(w.state.path())
                .build(),
        )
        .build();
    assert_eq!(bare.publish_to_relay().await.unwrap(), Default::default());
    assert!(w.rows().await.is_empty());
    let _ = StaticToken("x".into());
}

/// A computer that stopped sharing lists nothing; sharing again lists its
/// Spaces again (the same switch `cua host stop` and the app's sharing
/// toggle use).
#[tokio::test]
async fn a_computer_that_stopped_sharing_lists_nothing() {
    let w = World::new().await;
    w.create("a", "linux", Kind::Container).await;
    let host = |sharing: bool| {
        let dir = w.home.path().join("host");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.json"),
            serde_json::json!({
                "mode": "relay", "name": "Studio", "runner": "process",
                "driver_bin": "/nonexistent", "sharing": sharing,
            })
            .to_string(),
        )
        .unwrap();
    };
    host(true);
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 1);
    host(false);
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.unpublished.len(), 1, "{r:?}");
    assert!(w.rows().await.is_empty());
    host(true);
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 1);
}

/// A Space another of the owner's computers asked this one to make is on
/// the relay already, under this computer: it is not listed a second time.
#[tokio::test]
async fn a_space_a_host_provides_is_not_listed_twice() {
    let w = World::new().await;
    let id = w.create("provided", "linux", Kind::Container).await;
    w.create("mine", "linux", Kind::Container).await;
    cua_host::provided::save_provided(
        &cua_host::Host::new(w.home.path()).paths().dir,
        &[cua_host::provided::ProvidedSpace {
            relay_machine: "0123456789abcdef".into(),
            local_space: id,
            name: "provided".into(),
            ..Default::default()
        }],
    )
    .unwrap();
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published, ["local:mine"], "{r:?}");
    assert_eq!(w.rows().await.len(), 1);
}
