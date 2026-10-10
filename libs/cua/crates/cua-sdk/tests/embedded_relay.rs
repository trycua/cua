//! An embedded runtime on the signed-in session lists the account's relay
//! machines, as `cua daemon` does. The Spaces app falls back to an embedded
//! runtime when its daemon cannot be started (an update whose old daemon
//! took longer than `cua daemon start` waited to stop it); that runtime had
//! no relay account and never refreshed the directory, so "My machines"
//! went empty while `cua spaces ls` still listed them.
//!
//! One test (it sets process environment): a temp `CUA_HOME`, the file
//! credential store and a loopback fake relay; nothing real is touched.

use cua_host::testing::FakeRelay;
use cua_sdk::{Cua, CuaConfig};

/// cua.local as the user's Mac saw it: online, sharing, desktop not shared
/// (spacesd 0.2.2 from an older app), and the user's Mac itself, not sharing.
const OTHER_MAC: &str = "9e9d969d41649428f693b403f6f8f225";
const THIS_MAC: &str = "b6953d14ff94b10fcbb38bdd13c47824";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn embedded_runtime_on_the_session_lists_your_relay_machines() {
    let relay = FakeRelay::start().await;
    relay.add_account("owner-token", "user-1", Some("ada@example.com"));
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    for (id, name) in [
        (OTHER_MAC, "Mac.localdomain"),
        (THIS_MAC, "Dillons-MacBook-Pro-2.local"),
    ] {
        client
            .register(
                "owner-token",
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
    relay.set_online(OTHER_MAC, true, "0.2.2");
    relay.set_online(THIS_MAC, true, "0.2.0");

    let home = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("CUA_HOME", home.path());
        std::env::set_var("CUA_CREDENTIAL_STORE", "file");
        std::env::set_var("CUA_RELAY_URL", &relay.url);
        std::env::remove_var("CUA_DAEMON_NO_RELAY");
        std::env::remove_var("CUA_FLEET_SESSION");
    }
    cua_auth::Store::from_env()
        .save(&cua_auth::Credentials {
            access_token: "owner-token".into(),
            refresh_token: None,
            expires_at: "2999-01-01T00:00:00Z".into(),
            token_type: "Bearer".into(),
            scope: None,
            id_token: None,
        })
        .unwrap();

    // The app's configuration (`CuaConfig(fleetFromSession: true)`).
    let cua = Cua::embedded(CuaConfig {
        state_dir: Some(home.path().join("sandboxes").display().to_string()),
        spaces_home: Some(home.path().display().to_string()),
        fleet_pool_home: Some(home.path().join("pools").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: true,
        ..Default::default()
    })
    .unwrap();
    let ids: Vec<String> = cua
        .spaces()
        .list()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(ids.contains(&format!("relay:{OTHER_MAC}")), "{ids:?}");
    assert!(ids.contains(&format!("relay:{THIS_MAC}")), "{ids:?}");

    // Without the session (`fleet_from_session` off) nothing reaches the
    // relay: an SDK user who did not ask for the session gets none of it.
    let bare = Cua::embedded(CuaConfig {
        state_dir: Some(home.path().join("sandboxes2").display().to_string()),
        spaces_home: Some(home.path().join("bare").display().to_string()),
        fleet_pool_home: Some(home.path().join("pools2").display().to_string()),
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap();
    assert!(
        bare.spaces()
            .list()
            .await
            .unwrap()
            .iter()
            .all(|s| !s.id.starts_with("relay:"))
    );
}
