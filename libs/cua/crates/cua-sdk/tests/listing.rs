//! The default sandbox listing includes cloud sandboxes, and the cloud part
//! never fails it: no Fleet credentials leave it out silently, an
//! unreachable Fleet leaves it out with a warning, bounded in time. Nothing
//! here reaches a real Fleet or the credential store (`fleet_from_env` and
//! `fleet_from_session` are off; the "unreachable" Fleet is a closed
//! loopback port).

use cua_sdk::{Cua, CuaConfig, FleetSettings};
use std::time::{Duration, Instant};

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
async fn unreachable_fleet_is_a_warning_not_an_error() {
    // A port nothing listens on.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
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
    let t = Instant::now();
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(20), "bounded");
    assert_eq!(l.warnings.len(), 1, "{l:?}");
    assert!(
        l.warnings[0].starts_with("cloud sandboxes not listed:"),
        "{l:?}"
    );
    // `list` logs the warning and still returns the rows.
    assert!(c.sandboxes().list(None).await.is_ok());
    // Local-only listings never ask Fleet.
    let l = c
        .sandboxes()
        .list_with_warnings(Some("local".into()))
        .await
        .unwrap();
    assert!(l.warnings.is_empty(), "{l:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hanging_fleet_is_cut_off_after_the_listing_timeout() {
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
    let t = Instant::now();
    let l = c.sandboxes().list_with_warnings(None).await.unwrap();
    let took = t.elapsed();
    assert!(
        took >= Duration::from_secs(4) && took < Duration::from_secs(10),
        "{took:?}"
    );
    assert!(l.warnings[0].contains("did not answer within 5s"), "{l:?}");
    drop(silent);
}
