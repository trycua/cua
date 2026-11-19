//! `cua spaces share` through a running daemon without a terminal fails at
//! once (CUA-1419). The daemon confirms every share with Touch ID or the
//! login password; from a script or agent nobody may be there, and the OS
//! prompt used to hold the command for about two minutes before it failed.
//! Everything runs in a temp home; the only daemon is this test's own.

mod common;

use common::Home;
use std::time::{Duration, Instant};

/// Unix: socket only. Windows has no Unix-socket listener, so the daemon
/// needs its loopback listener there.
const LOOPBACK: &str = if cfg!(unix) { "off" } else { "127.0.0.1:0" };

#[tokio::test]
async fn share_through_the_daemon_without_a_terminal_needs_yes() {
    let mut h = Home::new();
    h.set("CUA_DAEMON_NO_RELAY", "1");
    h.run(&["daemon", "start", "--loopback", LOOPBACK])
        .await
        .ok();

    // `Home::run` gives the CLI no terminal on stdin.
    let started = Instant::now();
    let o = h
        .run(&["spaces", "share", "local:studio", "bob@example.com"])
        .await;
    let took = started.elapsed();
    h.run(&["daemon", "stop"]).await.ok();

    assert_eq!(o.code, 2, "{o:?}");
    assert!(o.stderr.contains("stdin is not a terminal"), "{o:?}");
    assert!(o.stderr.contains("--yes (-y)"), "{o:?}");
    // Refused before the daemon was asked (it would say the Space is
    // unknown) and without waiting on a prompt.
    assert!(!o.stderr.contains("not found"), "{o:?}");
    assert!(took < Duration::from_secs(20), "took {took:?}");
}
