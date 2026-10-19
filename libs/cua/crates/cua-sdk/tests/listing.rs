//! The default sandbox listing: Cua Cloud has ended, so cloud sandboxes are
//! never listed (quietly, with no warning) and listing never asks Fleet,
//! even when Fleet settings are given. Nothing here reaches a real Fleet or
//! the credential store (`fleet_from_env` and `fleet_from_session` are off;
//! the Fleet named below is a loopback port that must see no connection).

use cua_sdk::{Cua, CuaConfig, FleetSettings};

fn cua(dir: &tempfile::TempDir, fleet: Option<FleetSettings>) -> std::sync::Arc<Cua> {
    Cua::embedded(CuaConfig {
        state_dir: Some(dir.path().join("sandboxes").display().to_string()),
        spaces_home: Some(dir.path().join("cua").display().to_string()),
        fleet,
        fleet_from_env: false,
        fleet_from_session: false,
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_configured_fleet_is_never_asked_and_cloud_sandboxes_are_not_listed() {
    let fleet = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    fleet.set_nonblocking(true).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let c = cua(
        &dir,
        Some(FleetSettings {
            base_url: Some(format!("http://{}", fleet.local_addr().unwrap())),
            token: Some("not-a-token".into()),
            ..Default::default()
        }),
    );
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
    assert!(c.sandboxes().list(None).await.is_ok());
    assert!(
        matches!(fleet.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
        "something connected to the Fleet URL"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_fleet_credentials_list_local_silently() {
    let dir = tempfile::tempdir().unwrap();
    let c = cua(&dir, None);
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
    assert!(l.sandboxes.iter().all(|s| s.location != "cloud"));
    // Cua Cloud has ended: asking for cloud sandboxes lists none, quietly.
    let l = c
        .sandboxes()
        .list_with_warnings(Some("cloud".into()))
        .await
        .unwrap();
    assert!(l.sandboxes.is_empty());
    assert!(l.warnings.is_empty(), "{l:?}");
}
