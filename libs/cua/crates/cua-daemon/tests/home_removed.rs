//! A daemon whose home directory is removed from under it stops. Otherwise
//! every periodic writer it runs (the Cua Volume's device heartbeat, the
//! lifetimes reaper, the warm pool) brings the deleted directories back one
//! file at a time, 30 to 150 seconds after each removal, with nothing left
//! that can reach the daemon or stop it.
#![cfg(unix)]

use cua_daemon::{
    Runtime, RuntimeConfig,
    server::{self, ServerConfig},
};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_daemon_whose_home_is_deleted_stops_and_recreates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("env").join("home");
    std::fs::create_dir_all(&home).unwrap();
    // This binary has this one test: the process-wide home is its own.
    unsafe { std::env::set_var("CUA_HOME", &home) };
    let rt = Runtime::new(RuntimeConfig {
        state_dir: Some(home.join("sandboxes")),
        spaces_home: Some(home.join("spaces")),
        ..Default::default()
    })
    .unwrap();
    let h = server::start(
        rt,
        ServerConfig {
            socket_path: Some(home.join("cua.sock")),
            loopback: None,
            token: "t".into(),
            discovery_path: Some(home.join("daemon.json")),
            bridge_ticket_ttl: Duration::from_secs(30),
        },
    )
    .await
    .unwrap();
    assert!(home.join("daemon.json").exists());

    // The environment is torn down.
    std::fs::remove_dir_all(dir.path().join("env")).unwrap();
    // The daemon notices (two checks, 5 s apart) and stops by itself.
    tokio::time::timeout(Duration::from_secs(30), h.wait())
        .await
        .expect("the daemon kept running after its home was deleted");
    assert!(
        !dir.path().join("env").exists(),
        "nothing brought the home back"
    );
}

#[tokio::test]
async fn the_watchdog_ignores_a_home_that_never_existed_and_a_live_one() {
    let dir = tempfile::tempdir().unwrap();
    // Never existed: no watching at all (returns at once).
    let (tx, rx) = tokio::sync::watch::channel(false);
    tokio::time::timeout(
        Duration::from_secs(2),
        server::home_watchdog(
            dir.path().join("nope"),
            Duration::from_millis(10),
            tx.clone(),
            rx,
        ),
    )
    .await
    .expect("returns at once");
    assert!(!*tx.borrow());

    // A live home: it keeps watching and never fires.
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let rx = tx.subscribe();
    let task = tokio::spawn(server::home_watchdog(
        home.clone(),
        Duration::from_millis(10),
        tx.clone(),
        rx,
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!*tx.borrow());
    // Removed: it fires.
    std::fs::remove_dir_all(&home).unwrap();
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(*tx.borrow());
}
