//! `cua daemon start --foreground` stops cleanly on SIGTERM (what `kill`,
//! launchd and an app quitting send), and never runs next to another
//! daemon for the same home: a second one waits for the first to exit,
//! then takes over. Everything runs in a temp home; the only daemons are
//! this test's own.
#![cfg(unix)]

mod common;

use common::Home;
use std::time::Duration;

fn discovery_pid(h: &Home) -> Option<u32> {
    let text = std::fs::read_to_string(h.cua_home().join("daemon.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    v["pid"].as_u64().map(|p| p as u32)
}

async fn wait_for_discovery(h: &Home, pid: u32) {
    for _ in 0..300 {
        if discovery_pid(h) == Some(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("daemon {pid} did not start");
}

fn sigterm(pid: u32) {
    // SAFETY: kill(2) on this test's own child.
    assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) }, 0);
}

#[tokio::test]
async fn sigterm_stops_the_daemon_and_a_second_one_waits_for_it() {
    let mut h = Home::new();
    h.set("CUA_DAEMON_NO_RELAY", "1");
    let args = ["daemon", "start", "--foreground", "--loopback", "off"];
    let mut first = h.spawn(&args);
    let first_pid = first.id().unwrap();
    wait_for_discovery(&h, first_pid).await;

    // A second daemon for the same home does not start while the first runs.
    let mut second = h.spawn(&args);
    let second_pid = second.id().unwrap();
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        discovery_pid(&h),
        Some(first_pid),
        "two daemons ran at once"
    );
    assert!(
        second.try_wait().unwrap().is_none(),
        "the second gave up early"
    );

    // SIGTERM: the first exits cleanly and promptly...
    sigterm(first_pid);
    let status = tokio::time::timeout(Duration::from_secs(45), first.wait())
        .await
        .expect("the daemon ignored SIGTERM")
        .unwrap();
    assert!(status.success(), "{status:?}");
    // ...and the second takes over.
    wait_for_discovery(&h, second_pid).await;
    sigterm(second_pid);
    let status = tokio::time::timeout(Duration::from_secs(45), second.wait())
        .await
        .expect("the daemon ignored SIGTERM")
        .unwrap();
    assert!(status.success(), "{status:?}");
    assert_eq!(discovery_pid(&h), None, "the discovery file outlived it");
}
