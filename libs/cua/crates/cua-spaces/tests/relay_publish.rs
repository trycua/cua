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
    /// One cua-spacesd answering for every Space not named above (bursts).
    shared: Mutex<Option<u16>>,
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
        let port = self
            .spacesd
            .lock()
            .unwrap()
            .get(name)
            .copied()
            .or(*self.shared.lock().unwrap());
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

/// The tests share one process, and one of them counts its open sockets:
/// they run one at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct World {
    _serial: tokio::sync::MutexGuard<'static, ()>,
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
        let serial = SERIAL.lock().await;
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
            _serial: serial,
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

    /// One cua-spacesd for every Space made from now on.
    async fn shared_driver(&self) -> MockServer {
        let d = MockServer::start(MockAuth::default()).await;
        d.state.advertise(&["relay_attach"]);
        let port: u16 = d.url().rsplit(':').next().unwrap().parse().unwrap();
        *self.rt.shared.lock().unwrap() = Some(port);
        d
    }

    /// `n` Spaces made 16 at a time; the seconds it took.
    async fn burst(&self, n: usize, prefix: &str) -> f64 {
        use futures_util::StreamExt as _;
        let started = std::time::Instant::now();
        futures_util::stream::iter(0..n)
            .map(|i| {
                let spaces = self.spaces.clone();
                let name = format!("{prefix}-{i}");
                async move {
                    spaces
                        .create(SpaceCreate {
                            on: Some(On::Local),
                            image: Some("linux".into()),
                            name: Some(name),
                            timeout: Some(Duration::from_secs(60)),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                }
            })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await;
        started.elapsed().as_secs_f64()
    }

    fn age_all(&self) {
        for i in self.spaces.list().unwrap() {
            self.age(&i.id);
        }
    }

    /// Makes the Space `id` old enough to be listed (a new one waits a few
    /// seconds, so one made and deleted at once never reaches the relay).
    fn age(&self, id: &str) {
        self.spaces
            .registry()
            .update_meta(id, |m| m.created_at_unix = 1)
            .unwrap();
    }

    async fn create(&self, name: &str, image: &str, kind: Kind) -> String {
        self.driver(name).await;
        let id = self
            .spaces
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
            .id;
        self.age(&id);
        id
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

/// A Space of this computer whose relay row stays (shared with other people,
/// or listed by hand: no longer "listed by Cua") is still the one local row,
/// not a second top-level row beside it.
#[tokio::test]
async fn a_space_kept_on_the_account_by_hand_or_by_sharing_is_not_listed_twice() {
    let w = World::new().await;
    w.create("home-claude-code", "linux", Kind::Container).await;
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published.len(), 1, "{r:?}");
    // What an unpublish of a shared Space (or a hand registration) leaves:
    // the attach record without the automatic mark.
    let path = w.home.path().join("shared-spaces.json");
    let mut v: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    for a in v.as_object_mut().unwrap().values_mut() {
        a.as_object_mut().unwrap().remove("auto");
    }
    std::fs::write(&path, v.to_string()).unwrap();
    w.spaces.relay_machines().await.unwrap();
    let listed = w.spaces.list().unwrap();
    assert_eq!(
        listed
            .iter()
            .filter(|i| i.name == "home-claude-code")
            .count(),
        1,
        "{listed:#?}"
    );
    assert!(
        listed.iter().all(|i| !i.id.starts_with("relay:")),
        "{listed:#?}"
    );
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
        async move {
            let id = spaces.create(opts).await.unwrap().ready().unwrap().id;
            spaces
                .registry()
                .update_meta(&id, |m| m.created_at_unix = 1)
                .unwrap();
            id
        }
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

/// The home that warms in the background for Home's prompt box is made
/// hidden (`cua.hidden=home` with `cua.relay=off`, as the app makes it): it
/// stays off the relay (the phone) and `ls` / the tools still return it,
/// marked hidden. A person writing into it (`reveal`) lists it from the
/// next pass; a home of the same name the app did not prepare is never
/// hidden.
#[tokio::test]
async fn a_hidden_home_stays_off_the_relay_until_a_person_writes_into_it() {
    let w = World::new().await;
    for n in ["home-claude-code", "plain-home"] {
        w.driver(n).await;
    }
    let make = |name: &str, labels: &[(&str, &str)]| {
        let spaces = w.spaces.clone();
        let opts = SpaceCreate {
            on: Some(On::Local),
            image: Some("linux".into()),
            name: Some(name.into()),
            timeout: Some(Duration::from_secs(30)),
            labels: labels
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        async move {
            let id = spaces.create(opts).await.unwrap().ready().unwrap().id;
            spaces
                .registry()
                .update_meta(&id, |m| m.created_at_unix = 1)
                .unwrap();
            id
        }
    };
    make(
        "home-claude-code",
        &[("cua.relay", "off"), ("cua.hidden", "home")],
    )
    .await;
    make("plain-home", &[]).await;

    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published, ["local:plain-home"], "{r:?}");
    assert!(!w.attached("home-claude-code"));
    assert_eq!(w.rows().await.len(), 1);

    // `ls` and the tools keep returning it, marked hidden; the home the app
    // did not prepare is not.
    let list = w.spaces.list().unwrap();
    let flag = |id: &str| list.iter().find(|i| i.id == id).unwrap().hidden;
    assert!(flag("local:home-claude-code"));
    assert!(!flag("local:plain-home"));
    let json = serde_json::to_value(&list).unwrap();
    let hidden_rows: Vec<_> = json
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["hidden"] == true)
        .collect();
    assert_eq!(hidden_rows.len(), 1);
    assert_eq!(hidden_rows[0]["id"], "local:home-claude-code");

    // The first message from a person lists it.
    assert!(w.spaces.reveal("local:home-claude-code").await.unwrap());
    assert!(!w.spaces.reveal("local:home-claude-code").await.unwrap());
    assert!(!w.spaces.reveal("local:plain-home").await.unwrap());
    assert!(!w.spaces.list().unwrap().iter().any(|i| i.hidden));
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.rows().await.len(), 2);
    assert!(w.attached("home-claude-code"));
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

fn open_fds() -> usize {
    std::fs::read_dir("/dev/fd").map(|d| d.count()).unwrap_or(0)
}

/// A burst of 200 creates is not slowed by listing Spaces, and costs the
/// relay a registration each plus a few reads in all, never more than four
/// calls at once; once everything is listed, idle passes cost nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_burst_of_200_creates_is_as_fast_and_cheap_for_the_relay() {
    const N: usize = 200;
    // Without any listing.
    let without = {
        let quiet = World::new().await;
        let _d = quiet.shared_driver().await;
        quiet.burst(N, "q").await
    };

    // With the pass running the way the daemon runs it, on a relay that
    // takes 20 ms per call.
    let w = World::new().await;
    let _d2 = w.shared_driver().await;
    w.relay.set_latency_ms(20);
    w.relay.reset_load();
    let running = Arc::new(AtomicBool::new(true));
    let passes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let looped = {
        let (spaces, running, passes) = (w.spaces.clone(), running.clone(), passes.clone());
        tokio::spawn(async move {
            while running.load(Ordering::SeqCst) {
                let _ = spaces.publish_to_relay().await;
                passes.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        })
    };
    let with = w.burst(N, "b").await;
    running.store(false, Ordering::SeqCst);
    looped.await.unwrap();
    eprintln!("200 creates: {without:.2} s without listing, {with:.2} s with it");
    assert!(
        with <= without * 1.25 + 0.5,
        "creating was slowed: {without:.2} s without, {with:.2} s with"
    );

    // Five seconds later every Space is old enough; the passes finish the job.
    w.age_all();
    let mut n_passes = passes.load(Ordering::SeqCst);
    loop {
        let r = w.spaces.publish_to_relay().await.unwrap();
        n_passes += 1;
        if r.published.is_empty() && !r.more {
            break;
        }
        assert!(n_passes < 60, "never finished");
    }
    let requests = w.relay.requests();
    let peak = w.relay.peak_in_flight();
    eprintln!("200 Spaces: {requests} relay requests in {n_passes} passes, peak {peak} in flight");
    assert_eq!(w.rows().await.len(), N);
    // One registration each; everything else is the directory read of a pass.
    assert!(
        requests <= N + 30,
        "{requests} requests for {N} Spaces in {n_passes} passes"
    );
    assert!(peak <= 4, "{peak} calls at once");

    // All listed: more passes ask nothing.
    w.relay.reset_load();
    for _ in 0..20 {
        w.spaces.publish_to_relay().await.unwrap();
    }
    assert_eq!(w.relay.requests(), 0, "an idle pass called the relay");
}

/// A Space made and deleted before it settles never reaches the relay.
#[tokio::test]
async fn a_space_made_and_deleted_at_once_never_touches_the_relay() {
    let w = World::new().await;
    let _d = w.shared_driver().await;
    w.relay.reset_load();
    w.burst(30, "tmp").await;
    for i in w.spaces.list().unwrap() {
        w.spaces.delete(&i.id).await.unwrap();
        w.spaces.publish_to_relay().await.unwrap();
    }
    w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(w.relay.requests(), 0);
    assert_eq!(w.relay.register_calls(), 0);
}

/// A relay that fails stops the calls (they come back later, after a longer
/// wait each time), never slows making Spaces, and leaks no sockets.
#[tokio::test]
async fn a_relay_outage_pauses_listing_and_leaks_nothing() {
    let w = World::new().await;
    let _d = w.shared_driver().await;
    w.burst(10, "pre").await;
    w.age_all();
    w.relay.set_reads_down(true);
    w.relay.reset_load();
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert!(r.published.is_empty(), "{r:?}");
    let first = w.relay.requests();
    assert!(
        (1..=4).contains(&first),
        "{first} calls on the first failure"
    );

    // Creating goes on at full speed while calls are paused.
    let secs = w.burst(30, "during").await;
    assert!(secs < 30.0, "creates slowed to {secs:.1} s");
    w.age_all();
    // (Each Space keeps its own connections open; the passes add none.)
    let fds_before = open_fds();
    for _ in 0..50 {
        w.spaces.publish_to_relay().await.unwrap();
    }
    assert_eq!(w.relay.requests(), first, "calls went on during the pause");
    assert!(
        open_fds() <= fds_before + 3,
        "sockets leaked: {} -> {}",
        fds_before,
        open_fds()
    );
    let fds_paused = open_fds();

    // The relay is back: after the pause the Spaces are listed.
    w.relay.set_reads_down(false);
    let started = std::time::Instant::now();
    loop {
        let r = w.spaces.publish_to_relay().await.unwrap();
        if !r.published.is_empty() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "never resumed");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    eprintln!(
        "listing resumed {:.1} s after the relay came back",
        started.elapsed().as_secs_f64()
    );
    for _ in 0..20 {
        w.spaces.publish_to_relay().await.unwrap();
    }
    assert!(
        open_fds() <= fds_paused + 6,
        "sockets leaked after resuming"
    );
}

/// A throttled read (429) stops the calls for a while instead of being
/// asked again at once.
#[tokio::test]
async fn a_throttled_relay_is_left_alone() {
    let w = World::new().await;
    let _d = w.shared_driver().await;
    w.burst(5, "t").await;
    w.age_all();
    w.relay.fail_reads(&[429]);
    w.relay.reset_load();
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert!(r.published.is_empty(), "{r:?}");
    assert_eq!(w.relay.requests(), 1);
    for _ in 0..20 {
        w.spaces.publish_to_relay().await.unwrap();
    }
    assert_eq!(w.relay.requests(), 1, "asked again while paused");
}

/// A running Space that is not on the account is never a mystery: the
/// status says how many and why (an old driver, a relay that fails, signed
/// out, the switch, sharing stopped), and stopped Spaces are counted as
/// waiting until they run.
#[tokio::test]
async fn running_spaces_that_are_not_on_the_account_say_why() {
    let w = World::new().await;
    w.create("good", "linux", Kind::Container).await;
    // A driver that cannot join the relay (it predates `relay_attach`).
    let old = MockServer::start(MockAuth::default()).await;
    let port: u16 = old.url().rsplit(':').next().unwrap().parse().unwrap();
    w.rt.spacesd.lock().unwrap().insert("old".into(), port);
    let old_id = w
        .spaces
        .create(SpaceCreate {
            on: Some(On::Local),
            image: Some("linux".into()),
            name: Some("old".into()),
            timeout: Some(Duration::from_secs(30)),
            ..Default::default()
        })
        .await
        .unwrap()
        .ready()
        .unwrap()
        .id;
    w.age(&old_id);
    w.create("sleeper", "linux", Kind::Container).await;
    w.power("sleeper", "stopped");

    w.spaces.publish_to_relay().await.unwrap();
    let st = w
        .spaces
        .relay_publish_status(&w.spaces.list().unwrap())
        .await;
    assert_eq!(st.off, None);
    assert_eq!((st.running, st.listed, st.stopped), (2, 1, 1), "{st:?}");
    assert_eq!(st.not_listed.len(), 1, "{st:?}");
    assert_eq!(st.not_listed[0].0, "old");
    let notes = st.notes().join("\n");
    assert!(
        notes.contains("1 running Space is not on your account: old (")
            && notes.contains("1 stopped Space shows"),
        "{notes}"
    );

    // Signed out: one plain reason for all of them.
    *w.account.0.lock().unwrap() = None;
    let st = w
        .spaces
        .relay_publish_status(&w.spaces.list().unwrap())
        .await;
    assert_eq!(st.off, Some(cua_spaces::share::Off::SignedOut));
    assert!(st.notes()[0].contains("2 running Spaces are not on your account: you are signed out"));

    // The switch.
    *w.account.0.lock().unwrap() = Some("ada-token".into());
    std::fs::write(
        w.home.path().join("config.toml"),
        "[relay]\nspaces = \"off\"\n",
    )
    .unwrap();
    let st = w
        .spaces
        .relay_publish_status(&w.spaces.list().unwrap())
        .await;
    assert_eq!(st.off, Some(cua_spaces::share::Off::Setting));
    std::fs::remove_file(w.home.path().join("config.toml")).unwrap();
}

/// A relay that fails is told, from another process too (`cua spaces ls`
/// reads what the daemon's pass recorded), and the note goes once listing
/// works again.
#[tokio::test]
async fn a_failing_relay_is_told_and_forgotten_when_listing_works() {
    let w = World::new().await;
    w.create("a", "linux", Kind::Container).await;
    w.relay.set_reads_down(true);
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert!(!r.failed.is_empty(), "{r:?}");
    let infos = w.spaces.list().unwrap();
    // Another process: same home, no shared memory.
    let other = Spaces::builder()
        .home(w.home.path())
        .sandboxes(
            Sandboxes::builder()
                .local(w.rt.clone())
                .state_dir(w.state.path())
                .build(),
        )
        .relay(RelayAccount::new(&w.relay.url, w.account.clone()))
        .build();
    let st = other.relay_publish_status(&infos).await;
    assert_eq!(st.not_listed.len(), 1, "{st:?}");
    assert!(
        st.not_listed[0].1.contains("relay did not answer")
            || st.not_listed[0].1.contains("not listed yet"),
        "{st:?}"
    );

    w.relay.set_reads_down(false);
    let started = std::time::Instant::now();
    loop {
        let r = w.spaces.publish_to_relay().await.unwrap();
        if !r.published.is_empty() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(20), "never resumed");
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    let st = other.relay_publish_status(&w.spaces.list().unwrap()).await;
    assert_eq!((st.listed, st.not_listed.len()), (1, 0), "{st:?}");
}

impl World {
    /// The runtime publishes no port for `name`'s cua-spacesd (a restarted
    /// runtime brings the ports back a moment after the Space), or again.
    fn ports(&self, name: &str, port: Option<u16>) {
        let mut m = self.rt.spacesd.lock().unwrap();
        match port {
            Some(p) => m.insert(name.into(), p),
            None => m.remove(name),
        };
    }

    fn port_of(&self, name: &str) -> u16 {
        self.rt.spacesd.lock().unwrap()[name]
    }

    /// A daemon restart: a new `Spaces` over the same home and state, with
    /// no connection cached.
    fn restarted(&self) -> Spaces {
        Spaces::builder()
            .home(self.home.path())
            .sandboxes(
                Sandboxes::builder()
                    .local(self.rt.clone())
                    .state_dir(self.state.path())
                    .build(),
            )
            .relay(RelayAccount::new(&self.relay.url, self.account.clone()))
            .build()
    }
}

/// A Space that comes back after a runtime restart before its cua-spacesd
/// answers is not "unsupported": it is listed once the driver answers, with
/// no help, and the retries are paced, not a hot loop.
#[tokio::test]
async fn a_space_whose_spacesd_wakes_late_is_listed_when_it_does() {
    let w = World::new().await;
    cua_spaces::share::set_waking_retry_ms(100);
    w.create("late", "linux", Kind::Container).await;
    let port = w.port_of("late");
    // The runtime and the daemon restarted; the port is not back yet.
    w.ports("late", None);
    let spaces = w.restarted();

    let r = spaces.publish_to_relay().await.unwrap();
    assert!(r.published.is_empty(), "{r:?}");
    assert_eq!(r.failed.len(), 1, "{r:?}");
    assert!(
        r.failed[0].1.contains("not answering yet"),
        "waking, not unsupported: {r:?}"
    );
    assert!(!r.failed[0].1.contains("runs no cua-spacesd"), "{r:?}");
    // Bounded: an immediate second pass leaves it alone (paced retry).
    let again = spaces.publish_to_relay().await.unwrap();
    assert!(
        again.failed.is_empty() && again.published.is_empty(),
        "{again:?}"
    );

    // The driver answers: a later pass lists it.
    w.ports("late", Some(port));
    let mut listed = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let r = spaces.publish_to_relay().await.unwrap();
        if !r.published.is_empty() {
            listed = true;
            break;
        }
    }
    assert!(listed, "never listed");
    assert_eq!(w.rows().await.len(), 1);
    assert!(w.attached("late"));
}

/// Registering a Space whose cua-spacesd is not up yet says it is waking
/// (not that its image has none), and works as soon as it answers.
#[tokio::test]
async fn registering_a_waking_space_is_not_an_unsupported_one() {
    let w = World::new().await;
    cua_spaces::share::set_waking_retry_ms(100);
    let id = w.create("wake", "linux", Kind::Container).await;
    let port = w.port_of("wake");
    w.ports("wake", None);
    let spaces = w.restarted();
    let e = spaces.relay_register(&id).await.unwrap_err();
    assert!(
        matches!(e, cua_spaces::Error::SpacesdNotAvailable { .. }),
        "{e:?}"
    );
    w.ports("wake", Some(port));
    spaces.relay_register(&id).await.unwrap();
    assert!(w.attached("wake"));
}

/// A computer set up as a host: its relay machine exists on the account and
/// its config names it.
async fn set_up_host(w: &World, id: &str) {
    cua_host::RelayClient::new(&w.relay.url)
        .unwrap()
        .register(
            "ada-token",
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: "Studio".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let dir = w.home.path().join("host");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.json"),
        serde_json::json!({
            "mode": "relay", "name": "Studio", "runner": "process",
            "driver_bin": "/nonexistent", "sharing": true, "machine_id": id,
        })
        .to_string(),
    )
    .unwrap();
}

/// This computer's own relay machine (its shared desktop, `relay:<id>`) stays
/// in the list when Spaces on it are listed and shared: the machines a pass
/// attaches are always the Spaces' own `space-` ones, never the host's.
#[tokio::test]
async fn the_hosts_own_desktop_row_stays_listed_beside_the_spaces_it_lists() {
    let w = World::new().await;
    set_up_host(&w, "studio-host").await;
    w.create("home-claude-code", "linux", Kind::Container).await;
    let r = w.spaces.publish_to_relay().await.unwrap();
    assert_eq!(r.published.len(), 1, "{r:?}");
    w.spaces.relay_machines().await.unwrap();
    let ids: Vec<String> = w.spaces.list().unwrap().into_iter().map(|i| i.id).collect();
    assert!(ids.iter().any(|i| i == "relay:studio-host"), "{ids:?}");
    assert_eq!(
        ids.iter().filter(|i| i.starts_with("relay:space-")).count(),
        0,
        "{ids:?}"
    );
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.home.path().join("shared-spaces.json")).unwrap())
            .unwrap();
    for a in saved.as_object().unwrap().values() {
        let machine = a["machine"].as_str().unwrap();
        assert!(machine.starts_with("space-"), "{machine}");
    }
}

/// A Space this computer lists names this computer as its host, so the
/// phone reads its thread through the computer's daemon; a row registered
/// before that (by hand, without a host) gets the host on the next pass,
/// and a Space re-registered after a restart keeps it.
#[tokio::test]
async fn a_listed_space_names_this_computer_as_its_host() {
    let w = World::new().await;
    let id = w.create("hosted", "linux", Kind::Container).await;
    let old = w.create("old", "linux", Kind::Container).await;
    // Registered by hand before the computer was a host: no host on the row.
    w.spaces.relay_register(&old).await.unwrap();
    assert!(w.rows().await[0].host.is_none());
    set_up_host(&w, "host-mac").await;

    w.spaces.publish_to_relay().await.unwrap();
    let rows = w.rows().await;
    assert_eq!(rows.len(), 2, "{rows:#?}");
    for m in &rows {
        assert_eq!(m.host.as_deref(), Some("host-mac"), "{m:#?}");
    }
    assert!(w.attached("old"), "repaired in place, driver on the relay");

    // A restart re-registers the same machine: the host stays.
    let manual = w.spaces.relay_register(&id).await.unwrap();
    let row = w
        .rows()
        .await
        .into_iter()
        .find(|m| m.id == manual.machine)
        .unwrap();
    assert_eq!(row.host.as_deref(), Some("host-mac"));
}
