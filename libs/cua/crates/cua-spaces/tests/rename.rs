//! Renaming a Space (`Spaces::rename`), hermetic: a throwaway cua home, the
//! fake relay directory (`cua_host::testing::FakeRelay`) and a mock
//! spacesd. The id never changes; a Space of this device keeps the new name
//! in its metadata, a machine on the relay is renamed there (owner only).

use cua_host::testing::FakeRelay;
use cua_proto::daemon::v1 as dpb;
use cua_spaces::registry::Credential;
use cua_spaces::relay::StaticToken;
use cua_spaces::{RelayAccount, Spaces};
use cua_spacesd_client::testing::{MockAuth, MockServer};
use std::sync::Arc;

fn signed_in(home: &std::path::Path, relay: &FakeRelay, token: &str) -> Spaces {
    Spaces::builder()
        .home(home)
        .relay(RelayAccount::new(
            &relay.url,
            Arc::new(StaticToken(token.into())),
        ))
        .build()
}

async fn register(relay: &FakeRelay, token: &str, id: &str, name: &str, allow: &[&str]) {
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .register(
            token,
            &cua_host::relay::RegisterRequest {
                id: id.into(),
                name: name.into(),
                allow: allow.iter().map(|s| s.to_string()).collect(),
                host: None,
                meta: Default::default(),
            },
        )
        .await
        .unwrap();
}

fn shown(spaces: &Spaces, id: &str) -> String {
    spaces
        .list()
        .unwrap()
        .into_iter()
        .find(|i| i.id == id)
        .map(|i| i.shown_name().to_string())
        .unwrap_or_else(|| panic!("{id} not listed"))
}

#[tokio::test]
async fn a_space_of_this_device_keeps_its_id_and_shows_the_new_name() {
    let home = tempfile::tempdir().unwrap();
    let spaces = Spaces::builder().home(home.path()).build();
    spaces
        .registry()
        .upsert(
            dpb::Space {
                id: "local:qa-runner".into(),
                name: "qa-runner".into(),
                ..Default::default()
            },
            Credential::default(),
        )
        .unwrap();
    let env = MockServer::start(MockAuth::default()).await;
    let studio = spaces
        .add(&env.url(), None, Some("studio".into()))
        .await
        .unwrap();

    let renamed = spaces
        .rename("local:qa-runner", " ci-linux ")
        .await
        .unwrap();
    assert_eq!(renamed.id, "local:qa-runner");
    assert_eq!(renamed.name, "qa-runner");
    assert_eq!(renamed.display_name, "ci-linux");
    assert_eq!(shown(&spaces, "local:qa-runner"), "ci-linux");
    // Kept on disk: a fresh runtime on the same home lists it so.
    let again = Spaces::builder().home(home.path()).build();
    assert_eq!(shown(&again, "local:qa-runner"), "ci-linux");
    // Both names find it.
    assert_eq!(
        spaces.resolve("ci-linux").unwrap().to_string(),
        "local:qa-runner"
    );
    assert_eq!(
        spaces.resolve("qa-runner").unwrap().to_string(),
        "local:qa-runner"
    );
    // A Space added by address is renamed the same way.
    let lab = spaces.rename(&studio.id, "lab").await.unwrap();
    assert_eq!(
        (lab.id.as_str(), lab.display_name.as_str()),
        (studio.id.as_str(), "lab")
    );

    // The rules a create's name follows.
    for bad in ["", "CI Linux", "-ci", "ci_linux"] {
        let e = spaces.rename("local:qa-runner", bad).await.unwrap_err();
        assert_eq!(e.tag(), "invalid_argument", "{bad:?}: {e}");
    }
    // Another Space's name is taken.
    let e = spaces.rename("local:qa-runner", "lab").await.unwrap_err();
    assert_eq!(e.tag(), "invalid_argument");
    assert!(e.to_string().contains("already called lab"), "{e}");
    // Nothing there.
    let e = spaces.rename("local:nope", "x").await.unwrap_err();
    assert_eq!(e.tag(), "not_found", "{e}");
    assert_eq!(shown(&spaces, "local:qa-runner"), "ci-linux");

    // The same name again is no change; the created name drops the rename.
    spaces.rename("ci-linux", "ci-linux").await.unwrap();
    let back = spaces.rename("ci-linux", "qa-runner").await.unwrap();
    assert_eq!(back.display_name, "");
    assert!(
        spaces
            .registry()
            .meta("local:qa-runner")
            .is_none_or(|m| m.display_name.is_empty())
    );
    // Deleting a Space forgets its name.
    spaces.rename(&studio.id, "lab-2").await.unwrap();
    spaces.remove(&studio.id).await.unwrap();
    assert!(spaces.registry().meta(&studio.id).is_none());
}

#[tokio::test]
async fn the_owner_renames_a_relay_space_for_everyone() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    register(
        &relay,
        "ada-token",
        "space-00000000000a",
        "studio",
        &["bob@example.com"],
    )
    .await;
    let home = tempfile::tempdir().unwrap();
    let spaces = signed_in(home.path(), &relay, "ada-token");
    spaces.list_all().await.unwrap();

    let renamed = spaces
        .rename("relay:space-00000000000a", "render-box")
        .await
        .unwrap();
    assert_eq!(renamed.id, "relay:space-00000000000a");
    assert_eq!(renamed.name, "render-box");
    // On the relay: every device and everyone it is shared with lists it so.
    assert_eq!(
        relay.machine("space-00000000000a").unwrap().name,
        "render-box"
    );
    assert_eq!(shown(&spaces, "relay:space-00000000000a"), "render-box");
    let other_device = tempfile::tempdir().unwrap();
    let other = signed_in(other_device.path(), &relay, "ada-token");
    other.list_all().await.unwrap();
    assert_eq!(shown(&other, "relay:space-00000000000a"), "render-box");
    // Kept on the relay, not here.
    assert!(spaces.registry().meta("relay:space-00000000000a").is_none());
}

#[tokio::test]
async fn a_space_shared_with_you_is_not_yours_to_rename() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    relay.add_account("bob-token", "bob", Some("bob@example.com"));
    register(
        &relay,
        "ada-token",
        "space-00000000000b",
        "studio",
        &["bob@example.com"],
    )
    .await;
    let home = tempfile::tempdir().unwrap();
    let bob = signed_in(home.path(), &relay, "bob-token");
    bob.list_all().await.unwrap();

    let e = bob
        .rename("relay:space-00000000000b", "mine-now")
        .await
        .unwrap_err();
    assert_eq!(e.tag(), "permission_denied", "{e}");
    assert!(
        e.to_string()
            .ends_with("studio is shared with you. Only its owner can rename it."),
        "{e}"
    );
    assert_eq!(relay.machine("space-00000000000b").unwrap().name, "studio");
    assert_eq!(shown(&bob, "relay:space-00000000000b"), "studio");
    assert!(bob.registry().meta("relay:space-00000000000b").is_none());
}

#[tokio::test]
async fn a_shared_space_of_this_device_is_renamed_on_the_relay_too() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let home = tempfile::tempdir().unwrap();
    let spaces = signed_in(home.path(), &relay, "ada-token");
    let env = MockServer::start(MockAuth::default()).await;
    let studio = spaces
        .add(&env.url(), None, Some("studio".into()))
        .await
        .unwrap();
    // Shared earlier: its driver is attached as this relay machine.
    register(
        &relay,
        "ada-token",
        "space-00000000000c",
        "studio",
        &["bob@example.com"],
    )
    .await;
    std::fs::write(
        home.path().join("shared-spaces.json"),
        serde_json::json!({ studio.id.clone(): { "machine": "space-00000000000c" } }).to_string(),
    )
    .unwrap();

    let renamed = spaces.rename(&studio.id, "lab").await.unwrap();
    assert_eq!(renamed.display_name, "lab");
    assert_eq!(relay.machine("space-00000000000c").unwrap().name, "lab");
}

#[tokio::test]
async fn a_space_this_computer_provides_shows_its_relay_name() {
    let relay = FakeRelay::start().await;
    relay.add_account("ada-token", "ada", Some("ada@example.com"));
    let home = tempfile::tempdir().unwrap();
    let spaces = signed_in(home.path(), &relay, "ada-token");
    spaces
        .registry()
        .upsert(
            dpb::Space {
                id: "local:ci".into(),
                name: "ci".into(),
                ..Default::default()
            },
            Credential::default(),
        )
        .unwrap();
    register(&relay, "ada-token", "space-00000000000d", "ci", &[]).await;
    let host_dir = cua_host::Host::new(home.path()).paths().dir.clone();
    std::fs::create_dir_all(&host_dir).unwrap();
    cua_host::provided::save_provided(
        &host_dir,
        &[cua_host::provided::ProvidedSpace {
            relay_machine: "space-00000000000d".into(),
            local_space: "local:ci".into(),
            name: "ci".into(),
            ..Default::default()
        }],
    )
    .unwrap();
    spaces.list_all().await.unwrap();
    // Registered under its own name: no rename.
    assert_eq!(shown(&spaces, "local:ci"), "ci");

    // Renamed here: on the relay too, for the devices it is provided to.
    spaces.rename("local:ci", "ci-arm").await.unwrap();
    assert_eq!(relay.machine("space-00000000000d").unwrap().name, "ci-arm");
    assert_eq!(shown(&spaces, "local:ci"), "ci-arm");

    // Renamed from another device (on the relay): this computer shows it.
    cua_host::RelayClient::new(&relay.url)
        .unwrap()
        .patch(
            "ada-token",
            "space-00000000000d",
            &cua_host::MachinePatch {
                name: Some("ci-x64".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    spaces.list_all().await.unwrap();
    // Listed once, as the local Space, with the relay's name.
    let rows: Vec<_> = spaces
        .list()
        .unwrap()
        .into_iter()
        .filter(|i| i.id == "local:ci" || i.id == "relay:space-00000000000d")
        .collect();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].shown_name(), "ci-x64");
}
