// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! F53: an idle `cua daemon` (the Cua Spaces build the apps ship, with the
//! pool agent and extensions) stays idle while the Spaces app watches it.
//!
//! An account with 19 computers (3 online, 16 offline) on a fake relay, and
//! a client that polls the way the apps do at idle: every 10 s the Space
//! list and a connection to every Space on it, every 30 s the hosts and a
//! thumbnail of each reachable Space, and a visible-window resources lease.
//! Over a minute the daemon must use under [`BUDGET`] of one core, signed in
//! and signed out, and it
//! must not dial an offline computer on every poll.
//!
//! Before the fix the daemon read the operating system's root store (and
//! opened a new TLS connection) for every relay call, a quarter of a core
//! on macOS, and dialed every offline computer on every poll.
#![cfg(unix)]

mod common;

use common::Home;
use cua_daemon::fixtures;
use cua_host::testing::FakeRelay;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The share of one core the daemon may use while watched at idle.
const BUDGET: f64 = 0.015;
/// How long the daemon is measured.
const WINDOW: Duration = Duration::from_secs(60);
/// The apps' list poll.
const POLL: Duration = Duration::from_secs(10);
const ONLINE: usize = 3;
const OFFLINE: usize = 16;

fn machine_id(i: usize) -> String {
    format!("{i:032x}")
}

/// CPU seconds `pid` used so far (user + system).
fn cpu_seconds(pid: u32) -> f64 {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        // After the `(comm)` field: state is field 3; utime and stime are 14
        // and 15.
        let rest = &stat[stat.rfind(')').unwrap() + 2..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks: f64 = f[11].parse::<f64>().unwrap() + f[12].parse::<f64>().unwrap();
        // SAFETY: sysconf has no preconditions.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        ticks / hz
    }
    #[cfg(not(target_os = "linux"))]
    {
        // `[[dd-]hh:]mm:ss.cc`
        let out = std::process::Command::new("ps")
            .args(["-o", "time=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let (days, clock) = match text.split_once('-') {
            Some((d, c)) => (d.parse::<f64>().unwrap(), c.to_string()),
            None => (0.0, text),
        };
        clock
            .split(':')
            .fold(0.0, |acc, part| acc * 60.0 + part.parse::<f64>().unwrap())
            + days * 86_400.0
    }
}

/// One idle poll of the app: the list, then each Space (bounded, as the
/// app's rows probe is), and every third poll the hosts and thumbnails.
async fn app_poll(spaces: &cua_sdk::Spaces, n: usize) {
    let Ok(infos) = spaces.list().await else {
        return;
    };
    let mut reachable = Vec::new();
    for info in infos {
        if let Ok(Ok(space)) =
            tokio::time::timeout(Duration::from_secs(4), spaces.space(info.id.clone())).await
        {
            let _ = space.capabilities_json();
            reachable.push(space);
        }
    }
    if n.is_multiple_of(3) {
        let _ = tokio::time::timeout(Duration::from_secs(10), spaces.hosts()).await;
        for space in &reachable {
            let _ =
                tokio::time::timeout(Duration::from_secs(10), space.thumbnail(Some(30_000))).await;
        }
    }
    let _ = spaces.resources(true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_daemon_watched_by_the_app_stays_under_its_cpu_budget() {
    idle_daemon(true).await;
}

/// Signed out (a new user's first state): no account calls succeed, and
/// nothing may retry them hot (the pool's lease loop built an HTTPS client,
/// reading the root store, every 5 s).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_signed_out_daemon_watched_by_the_app_stays_under_its_cpu_budget() {
    idle_daemon(false).await;
}

async fn idle_daemon(signed_in: bool) {
    let relay = FakeRelay::start().await;
    relay.add_account("relay-token", "user-1", Some("ada@example.com"));
    let client = cua_host::RelayClient::new(&relay.url).unwrap();
    let mut envs = Vec::new();
    for i in 0..ONLINE + OFFLINE {
        let id = machine_id(i);
        client
            .register(
                "relay-token",
                &cua_host::relay::RegisterRequest {
                    id: id.clone(),
                    name: format!("computer-{i}"),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        if i < ONLINE {
            let env = fixtures::start_env(None, None).await;
            relay.tunnel(&id, &env.url);
            relay.set_online(&id, true, "0.0.0-mock");
            envs.push(env);
        }
    }

    let mut h = Home::new();
    h.set("CUA_RELAY_URL", &relay.url);
    std::fs::create_dir_all(h.cua_home()).unwrap();
    if signed_in {
        std::fs::write(
            h.cua_home().join("credentials.json"),
            serde_json::json!({
                "access_token": "relay-token",
                "expires_at": "2999-01-01T00:00:00Z",
                "token_type": "Bearer",
            })
            .to_string(),
        )
        .unwrap();
    }
    h.run(&["daemon", "start", "--loopback", "off"]).await.ok();
    let socket = h.cua_home().join("cua.sock");
    let cua = cua_sdk::Cua::connect(Some(format!("unix:{}", socket.display())), None).unwrap();
    let pid = cua.info().await.unwrap().daemon_pid.expect("daemon pid");
    let spaces: Arc<cua_sdk::Spaces> = cua.spaces();

    // Warm up: the first poll connects everything and starts the mesh,
    // sampler and caches; that is not idle.
    app_poll(&spaces, 1).await;
    let offline_before = offline_dials(&relay);
    let cpu_before = cpu_seconds(pid);
    let started = Instant::now();
    let mut n = 0;
    while started.elapsed() < WINDOW {
        app_poll(&spaces, n).await;
        n += 1;
        let next = POLL * n as u32;
        tokio::time::sleep(next.saturating_sub(started.elapsed())).await;
    }
    let used = cpu_seconds(pid) - cpu_before;
    let elapsed = started.elapsed().as_secs_f64();
    let offline = offline_dials(&relay) - offline_before;
    let label = if signed_in { "signed in" } else { "signed out" };
    h.run(&["daemon", "stop"]).await.ok();

    let share = used / elapsed;
    eprintln!(
        "idle daemon ({label}): {used:.2} s CPU over {elapsed:.0} s ({:.2}% of a core), {n} polls, \
         {offline} relay requests to {OFFLINE} offline computers",
        share * 100.0
    );
    assert!(
        share < BUDGET,
        "the idle daemon ({label}) used {:.2}% of a core (budget {:.0}%)",
        share * 100.0,
        BUDGET * 100.0
    );
    // Each offline computer is retried with backoff (at most about every
    // 20 s at first), not on every poll.
    assert!(
        offline <= OFFLINE * 3,
        "{offline} requests to {OFFLINE} offline computers in {n} polls"
    );
}

/// Requests the relay forwarded (or refused) for the offline computers.
fn offline_dials(relay: &FakeRelay) -> usize {
    let offline: Vec<String> = (ONLINE..ONLINE + OFFLINE)
        .map(|i| format!("/m/{}", machine_id(i)))
        .collect();
    relay
        .proxied()
        .iter()
        .filter(|(path, _)| offline.iter().any(|p| path.starts_with(p.as_str())))
        .count()
}
