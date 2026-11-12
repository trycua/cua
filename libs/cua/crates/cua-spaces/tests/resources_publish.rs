//! A relay machine publishes its resource summary into its own machine
//! record, and your other computers read it from the machine list: numbers
//! only, at a low rate, and quietly nothing against a relay that predates
//! summaries (`cua_host::testing::FakeRelay`).

use cua_host::ConnectedClient;
use cua_host::testing::FakeRelay;
use cua_spaces::resources::publish::Published;
use cua_spaces::resources::summary;
use cua_spaces::{RelayAccount, Spaces};
use std::sync::Arc;

const MACHINE: &str = "aaaa000000000001";

/// A host registered on the relay, with this install's host config.
async fn host(relay: &FakeRelay, home: &std::path::Path) -> Spaces {
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            "owner-token",
            &cua_host::relay::RegisterRequest {
                id: MACHINE.into(),
                name: "Studio Mac".into(),
                allow: vec![],
                host: None,
                meta: [("cua.host.os".to_string(), "macos".to_string())].into(),
            },
        )
        .await
        .unwrap();
    std::fs::create_dir_all(home.join("host")).unwrap();
    std::fs::write(
        home.join("host/config.json"),
        serde_json::json!({
            "mode": "relay",
            "name": "Studio Mac",
            "machine_id": MACHINE,
            "runner": "process",
            "driver_bin": "/nonexistent/cua-spacesd",
        })
        .to_string(),
    )
    .unwrap();
    Spaces::builder()
        .home(home)
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(cua_spaces::relay::StaticToken("owner-token".into())),
        ))
        .build()
}

#[tokio::test]
async fn a_host_publishes_numbers_into_its_machine_record() {
    let relay = FakeRelay::start().await;
    let home = tempfile::tempdir().unwrap();
    let spaces = host(&relay, home.path()).await;

    let out = spaces.publish_resources().await.unwrap();
    assert_eq!(
        out,
        Published::Sent {
            watchers: 0,
            spaces: 0
        }
    );
    let meta = relay.machine(MACHINE).unwrap().meta;
    // What registration set stays; the summary joins it. Nothing is being
    // created: no creates key.
    assert_eq!(
        meta["cua.host.os"],
        cua_host::host_meta(false)["cua.host.os"]
    );
    assert!(!meta.contains_key(cua_spaces::resources::creates::META_KEY));
    let value = &meta[summary::META_KEY];
    assert!(value.len() <= summary::MAX_LEN, "{value}");
    let c = summary::decode(MACHINE, "Studio Mac", true, value).expect("a summary");
    assert!(c.cpu_count >= 1);
    assert!(c.memory_total > 0 && c.disk_total > 0);
    // Numbers only.
    assert!(
        value
            .chars()
            .all(|ch| ch.is_ascii_digit() || "{}\":,".contains(ch) || ch.is_ascii_lowercase()),
        "{value}"
    );
    assert!(!value.contains("Studio"));
}

/// The relay row follows this computer's real Provide Spaces setting: a row
/// registered as off (an earlier version's default, or a setting changed
/// since) says on once the setting is on, and off again when it goes off.
/// The phone reads this key and the phone key.
#[tokio::test]
async fn the_relay_row_follows_the_provide_spaces_setting() {
    let relay = FakeRelay::start().await;
    let home = tempfile::tempdir().unwrap();
    let spaces = host(&relay, home.path()).await;
    let set = |on: bool| {
        let path = home.path().join("host/config.json");
        let mut c: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        c["provide_spaces"] = on.into();
        std::fs::write(path, c.to_string()).unwrap();
    };
    // Registered before: no provides key at all, and a phone key missing.
    assert!(
        !relay
            .machine(MACHINE)
            .unwrap()
            .meta
            .contains_key("cua.host.spaces")
    );
    set(true);
    spaces.publish_resources().await.unwrap();
    let meta = relay.machine(MACHINE).unwrap().meta;
    assert_eq!(meta["cua.host.spaces"], "on");
    assert_eq!(meta["cua.host.phone"], "1");
    set(false);
    spaces.publish_resources().await.unwrap();
    assert_eq!(
        relay.machine(MACHINE).unwrap().meta["cua.host.spaces"],
        "off"
    );
    // Paused (signed out): the row is left as it is.
    set(true);
    let path = home.path().join("host/config.json");
    let mut c: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    c["paused_signed_out"] = true.into();
    std::fs::write(&path, c.to_string()).unwrap();
    spaces.publish_resources().await.unwrap();
    assert_eq!(
        relay.machine(MACHINE).unwrap().meta["cua.host.spaces"],
        "off"
    );
}

#[tokio::test]
async fn somebody_connected_means_watched() {
    let relay = FakeRelay::start().await;
    let home = tempfile::tempdir().unwrap();
    let spaces = host(&relay, home.path()).await;
    relay.add_client(
        MACHINE,
        ConnectedClient {
            id: "u2".into(),
            email: Some("b@example.com".into()),
            streams: 1,
            since: 5,
            ..Default::default()
        },
    );
    assert_eq!(
        spaces.publish_resources().await.unwrap(),
        Published::Sent {
            watchers: 1,
            spaces: 0
        }
    );
}

#[tokio::test]
async fn a_computer_that_is_not_a_relay_host_publishes_nothing() {
    let relay = FakeRelay::start().await;
    let home = tempfile::tempdir().unwrap();
    // Signed in, but `cua host setup` never ran here.
    let spaces = Spaces::builder()
        .home(home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(cua_spaces::relay::StaticToken("owner-token".into())),
        ))
        .build();
    assert_eq!(
        spaces.publish_resources().await.unwrap(),
        Published::NotAHost
    );
    // And a host with nobody signed in.
    let signed_out = Spaces::builder().home(home.path()).build();
    assert_eq!(
        signed_out.publish_resources().await.unwrap(),
        Published::NotAHost
    );
}

#[tokio::test]
async fn a_relay_that_drops_the_summary_is_reported_as_unsupported() {
    let relay = FakeRelay::start().await;
    relay.set_ignore_meta_patches(true);
    let home = tempfile::tempdir().unwrap();
    let spaces = host(&relay, home.path()).await;
    assert_eq!(
        spaces.publish_resources().await.unwrap(),
        Published::Unsupported
    );
    assert!(
        !relay
            .machine(MACHINE)
            .unwrap()
            .meta
            .contains_key(summary::META_KEY)
    );
}

#[tokio::test]
async fn another_computer_reads_the_summary_from_the_machine_list() {
    let relay = FakeRelay::start().await;
    let home = tempfile::tempdir().unwrap();
    let host_spaces = host(&relay, home.path()).await;
    host_spaces.publish_resources().await.unwrap();

    // Your laptop: signed in to the same account, its own cua home.
    let laptop_home = tempfile::tempdir().unwrap();
    let laptop = Spaces::builder()
        .home(laptop_home.path())
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(cua_spaces::relay::StaticToken("owner-token".into())),
        ))
        .build();
    laptop.relay_machines().await.unwrap();
    let r = laptop.resources(false).await.unwrap();
    assert_eq!(r.computers.len(), 1);
    assert_eq!(r.computers[0].id, MACHINE);
    assert_eq!(r.computers[0].name, "Studio Mac");
    assert!(r.computers[0].cpu_count >= 1);
    // The host itself does not list its own record next to itself.
    host_spaces.relay_machines().await.unwrap();
    assert!(
        host_spaces
            .resources(false)
            .await
            .unwrap()
            .computers
            .is_empty()
    );
}
