//! `cua daemon stop` returns once the daemon no longer answers, so a
//! `cua daemon start` right after starts a new daemon. Before, stop returned
//! while the daemon was still on its way out: the start found it ("already
//! running"), it exited, and the next command failed with a transport error.
//! Everything runs in a temp home; the only daemons are this test's own.

mod common;

use common::Home;

/// Unix: socket only. Windows has no Unix-socket listener, so the daemon
/// needs its loopback listener there.
const LOOPBACK: &str = if cfg!(unix) { "off" } else { "127.0.0.1:0" };

fn pid_of(started: &str) -> Option<u32> {
    let rest = started.split("(pid ").nth(1)?;
    rest.split(')').next()?.trim().parse().ok()
}

#[tokio::test]
async fn stop_then_start_right_away_starts_a_new_daemon() {
    let mut h = Home::new();
    h.set("CUA_DAEMON_NO_RELAY", "1");
    let first = h.run(&["daemon", "start", "--loopback", LOOPBACK]).await;
    first.ok();
    let mut pid = pid_of(&first.stdout).expect("started (pid N)");
    for round in 0..5 {
        let stop = h.run(&["daemon", "stop"]).await;
        stop.ok();
        assert_eq!(stop.stdout.trim(), "stopped");
        let start = h.run(&["daemon", "start", "--loopback", LOOPBACK]).await;
        start.ok();
        assert!(
            start.stdout.contains("cua daemon started"),
            "round {round}: the old daemon was found on its way out: {}",
            start.stdout
        );
        let new = pid_of(&start.stdout).unwrap();
        assert_ne!(new, pid, "round {round}");
        pid = new;
        let status = h.run(&["daemon", "status", "--json"]).await;
        status.ok();
        assert_eq!(status.json()["pid"], pid, "round {round}");
    }
    h.run(&["daemon", "stop"]).await.ok();
}
