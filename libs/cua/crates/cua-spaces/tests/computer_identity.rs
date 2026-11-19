//! A computer is one row, found by its id: removing a ghost the relay lists
//! as connected, and renaming a computer (this one or another) without it
//! changing id or going offline in any list. Hermetic: a throwaway cua home
//! per computer and the fake relay directory
//! (`cua_host::testing::FakeRelay`); nothing touches the real home, the
//! network, or the desktop.

use cua_host::testing::FakeRelay;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::path::Path;
use std::sync::Arc;

const MACHINE: &str = "0123abcd4567ef89";
const TOKEN: &str = "owner-token";

fn signed_in(home: &Path, relay: &FakeRelay) -> Spaces {
    Spaces::builder()
        .home(home)
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken(TOKEN.into())),
        ))
        .build()
}

async fn register(relay: &FakeRelay, id: &str, name: &str) {
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            TOKEN,
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: name.into(),
                allow: vec![],
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
}

/// Writes the host setup of `home` the way `cua host setup` leaves it.
fn host_setup(home: &Path, relay: &FakeRelay, id: &str, name: &str) {
    std::fs::create_dir_all(home.join("host")).unwrap();
    std::fs::write(
        home.join("host/config.json"),
        serde_json::json!({
            "mode": "relay",
            "relay_url": relay.url,
            "name": name,
            "machine_id": id,
            "runner": "process",
            "driver_bin": "/nonexistent/cua-spacesd",
        })
        .to_string(),
    )
    .unwrap();
}

/// `(name, status)` of the `relay:<id>` row in `spaces`' cached list.
fn row(spaces: &Spaces, id: &str) -> Option<(String, String)> {
    spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == format!("relay:{id}"))
        .map(|i| (i.shown_name().to_string(), i.status))
}

/// A computer the relay lists as connected that does not answer (a stale
/// entry: a relay that handed over, a crash behind a proxy) is a ghost the
/// owner can remove from the app; one that answers is connected for real
/// and is taken off from that computer.
#[tokio::test]
async fn a_ghost_listed_as_connected_can_be_removed_but_a_live_computer_cannot() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    let live = "aaaa111122223333";
    let ghost = "bbbb111122223333";
    register(&relay, live, "studio").await;
    register(&relay, ghost, "old-studio").await;
    relay.set_online(live, true, "1.0.0");
    relay.set_online(ghost, true, "1.0.0");
    // `live` answers through the relay; `ghost` does not (the fake relay
    // answers 502 for a machine with nothing behind it).
    let srv = MockServer::start(MockAuth {
        token: None,
        gateway: None,
        prefix: None,
    })
    .await;
    relay.tunnel(live, &srv.url());

    let home = tempfile::tempdir().unwrap();
    let app = signed_in(home.path(), &relay);
    app.list_all().await.unwrap();

    let err = app
        .relay_unregister(&format!("relay:{live}"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("connected now") && err.contains("cua host remove"),
        "{err}"
    );
    assert!(relay.machine(live).is_some(), "a live computer stays");

    assert!(
        app.relay_unregister(&format!("relay:{ghost}"))
            .await
            .unwrap()
    );
    assert!(
        relay.machine(ghost).is_none(),
        "the ghost is gone on the relay"
    );
    assert!(row(&app, ghost).is_none(), "and in this list");
    assert!(row(&app, live).is_some());
}

/// A computer that is offline is removed, as before.
#[tokio::test]
async fn an_offline_computer_is_removed_from_the_account() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    register(&relay, MACHINE, "studio").await;
    let home = tempfile::tempdir().unwrap();
    let app = signed_in(home.path(), &relay);
    app.list_all().await.unwrap();
    assert!(
        app.relay_unregister(&format!("relay:{MACHINE}"))
            .await
            .unwrap()
    );
    assert!(relay.machine(MACHINE).is_none());
}

/// Renaming a computer from the Machines page patches the relay and then
/// every copy this device keeps by the machine id: this computer's host
/// setup, the saved directory (what the lists show first), the cached list.
/// It stays the same computer, online, in this computer's lists and in the
/// lists of the account's other computers.
#[tokio::test]
async fn renaming_a_computer_keeps_its_id_and_it_stays_online_in_every_list() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    register(&relay, MACHINE, "alpha").await;
    relay.set_online(MACHINE, true, "1.0.0");

    // This computer (a host), and another computer of the account.
    let this = tempfile::tempdir().unwrap();
    host_setup(this.path(), &relay, MACHINE, "alpha");
    let other = tempfile::tempdir().unwrap();
    let on_this = signed_in(this.path(), &relay);
    let on_other = signed_in(other.path(), &relay);
    on_this.list_all().await.unwrap();
    on_other.list_all().await.unwrap();
    assert_eq!(row(&on_other, MACHINE).unwrap().0, "alpha");

    // What the app's Machines page does (`Relay::rename_machine`).
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .patch(
            TOKEN,
            MACHINE,
            &cua_host::MachinePatch {
                name: Some("beta".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    cua_spaces::relay::adopt_machine_name(this.path(), &relay.url, MACHINE, "beta");

    // This computer's host setup follows.
    let cfg = cua_host::Host::new(this.path()).config().unwrap().unwrap();
    assert_eq!(cfg.name, "beta");
    assert_eq!(cfg.machine_id.as_deref(), Some(MACHINE));
    // A fresh start of the app reads the saved directory before the relay
    // answers: it already says Beta, not the old name.
    let restarted = Spaces::builder().home(this.path()).build();
    restarted.set_relay(Some(RelayAccount::new(
        &relay.url,
        Arc::new(StaticToken(TOKEN.into())),
    )));
    assert_eq!(row(&restarted, MACHINE).unwrap().0, "beta");

    // Both lists, once read again: same id, new name, still online.
    for app in [&on_this, &on_other] {
        let all = app.list_all().await.unwrap();
        let listed: Vec<_> = all.iter().filter(|i| i.id.starts_with("relay:")).collect();
        assert_eq!(listed.len(), 1, "one computer");
        assert_eq!(listed[0].id, format!("relay:{MACHINE}"));
        assert_eq!(listed[0].shown_name(), "beta");
        assert_ne!(listed[0].status, "offline");
        assert!(app.relay_machines().await.unwrap()[0].online);
    }
    // The old name no longer names it; the new one does.
    assert!(on_this.resolve("alpha").is_err());
    assert_eq!(
        on_this.resolve("beta").unwrap().to_string(),
        format!("relay:{MACHINE}")
    );
}

/// The Space rename path updates the same copies (and the in-memory list at
/// once), by the same function.
#[tokio::test]
async fn renaming_a_relay_computer_as_a_space_updates_the_same_copies() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    register(&relay, MACHINE, "alpha").await;
    relay.set_online(MACHINE, true, "1.0.0");
    let this = tempfile::tempdir().unwrap();
    host_setup(this.path(), &relay, MACHINE, "alpha");
    let app = signed_in(this.path(), &relay);
    app.list_all().await.unwrap();

    app.rename(&format!("relay:{MACHINE}"), "gamma")
        .await
        .unwrap();
    assert_eq!(row(&app, MACHINE).unwrap().0, "gamma");
    let cfg = cua_host::Host::new(this.path()).config().unwrap().unwrap();
    assert_eq!(cfg.name, "gamma", "this computer's own setup follows");
    assert_eq!(relay.machine(MACHINE).unwrap().name, "gamma");
    assert!(relay.machine(MACHINE).unwrap().online);
}

/// The `relay:` record in the registry and the host entries the "Run on"
/// list matches by name follow a rename, so the same computer is not listed
/// twice under two names.
#[tokio::test]
async fn a_rename_reaches_the_registry_record_and_the_host_entry_named_like_it() {
    use cua_proto::daemon::v1 as dpb;
    use cua_spaces::registry::Credential;
    let home = tempfile::tempdir().unwrap();
    let registry = cua_spaces::registry::Registry::new(home.path());
    registry
        .upsert(
            dpb::Space {
                id: format!("relay:{MACHINE}"),
                name: "alpha".into(),
                ..Default::default()
            },
            Credential::default(),
        )
        .unwrap();
    registry
        .upsert_direct_host("Alpha", "direct:100.64.0.9:3211")
        .unwrap();
    registry
        .upsert_direct_host("elsewhere", "direct:100.64.0.10:3211")
        .unwrap();

    cua_spaces::relay::adopt_machine_name(home.path(), "http://relay.test", MACHINE, "beta");

    let record = registry.get(&format!("relay:{MACHINE}")).unwrap().unwrap();
    assert_eq!(record.name, "beta");
    let hosts: Vec<_> = registry
        .direct_hosts()
        .unwrap()
        .into_iter()
        .map(|h| (h.space, h.name))
        .collect();
    assert!(
        hosts.contains(&("direct:100.64.0.9:3211".into(), "beta".into())),
        "{hosts:?}"
    );
    assert!(
        hosts.contains(&("direct:100.64.0.10:3211".into(), "elsewhere".into())),
        "{hosts:?}"
    );
}

/// "Run on" lists a computer set up both ways (on the relay and added by
/// address) once, matching them by name. A rename must take both names
/// along, or the address entry (unreachable from another network) is listed
/// again as an offline copy of the renamed computer.
#[tokio::test]
async fn a_renamed_computer_is_not_listed_again_as_an_offline_host() {
    let relay = FakeRelay::start().await;
    relay.add_account(TOKEN, "user-1", Some("ada@example.com"));
    register(&relay, MACHINE, "alpha").await;
    relay.set_online(MACHINE, true, "1.0.0");
    let home = tempfile::tempdir().unwrap();
    let app = signed_in(home.path(), &relay);
    // The same computer, added by an address this network cannot reach.
    app.registry()
        .upsert_direct_host("alpha", "direct:127.0.0.1:1")
        .unwrap();
    app.list_all().await.unwrap();
    let names = |hosts: Vec<cua_spaces::host_spaces::HostOffer>| {
        let mut n: Vec<String> = hosts.into_iter().map(|h| h.name).collect();
        n.sort();
        n
    };
    assert_eq!(names(app.hosts().await.unwrap()), ["alpha"]);

    app.rename(&format!("relay:{MACHINE}"), "beta")
        .await
        .unwrap();
    assert_eq!(names(app.hosts().await.unwrap()), ["beta"]);
}
