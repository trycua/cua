//! The sandbox listing never asks Cua Cloud (closed): the default listing
//! has no cloud warning, with or without account credentials, and a
//! cloud-only listing says why it is empty. Nothing here reaches the network
//! or the credential store (`fleet_from_env` and `fleet_from_session` are
//! off).

use cua_sdk::{Cua, CuaConfig, FleetSettings};

fn cua(dir: &tempfile::TempDir, fleet: Option<FleetSettings>) -> std::sync::Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet_pool_home: Some(dir.path().join("pools").display().to_string()),
        fleet,
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_fleet_credentials_list_local_silently() {
    let dir = tempfile::tempdir().unwrap();
    let c = cua(&dir, None);
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
    // Asking for cloud sandboxes only says why there are none.
    let l = c
        .sandboxes()
        .list_with_warnings(Some("cloud".into()))
        .await
        .unwrap();
    assert!(l.sandboxes.is_empty());
    assert!(
        l.warnings
            .iter()
            .any(|w| w.starts_with("cloud sandboxes not listed:")),
        "{l:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn account_credentials_never_make_the_listing_ask_cua_cloud() {
    // Accepts connections (the kernel backlog) and never answers.
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = silent.local_addr().unwrap().port();
    let dir = tempfile::tempdir().unwrap();
    let c = cua(
        &dir,
        Some(FleetSettings {
            base_url: Some(format!("http://127.0.0.1:{port}")),
            token_url: None,
            client_id: None,
            client_secret: None,
            token: Some("cua-e2e-not-a-token".into()),
        }),
    );
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
    let l = c
        .sandboxes()
        .list_with_warnings(Some("cloud".into()))
        .await
        .unwrap();
    assert!(
        l.warnings
            .iter()
            .any(|w| w.contains("Cua Cloud has closed")),
        "{l:?}"
    );
    drop(silent);
}
