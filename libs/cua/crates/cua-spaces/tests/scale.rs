// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The Space list at scale, and under chaos: an account with many
//! computers, each with many Spaces, some of them offline, slow, lossy or
//! unresponsive.
//!
//! Each scenario does what the apps do after a launch or a daemon restart:
//! read this machine's Spaces, read the whole list (the relay directory),
//! then reach every Space at once with the app's per-Space deadline. It
//! asserts the budgets:
//!
//! - this machine's Spaces list within [`LOCAL_BUDGET`], and the list is
//!   never empty while they exist;
//! - every Space on a computer that answers is reached within
//!   [`ONLINE_BUDGET`];
//! - every Space on an offline or unresponsive computer is marked within
//!   [`OFFLINE_BUDGET`], and never holds up the rest;
//! - a computer that comes back is reachable within [`RECOVERY_BUDGET`];
//! - a computer that keeps failing is not dialed again on every poll.
//!
//! Sizes: `CUA_SCALE_HOSTS` (default 20) computers with `CUA_SCALE_SPACES`
//! (default 10) Spaces each. Each scenario prints a `SCALE` line.

use cua_host::relay::RegisterRequest;
use cua_host::testing::FakeRelay;
use cua_spaces::relay::{AccountTokens, StaticToken};
use cua_spaces::{RelayAccount, Spaces};
use cua_spacesd_client::testing::{Fault, FaultProxy, MockAuth, MockServer};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

const LOCAL_BUDGET: Duration = Duration::from_millis(200);
const LIST_BUDGET: Duration = Duration::from_secs(2);
const ONLINE_BUDGET: Duration = Duration::from_secs(2);
const OFFLINE_BUDGET: Duration = Duration::from_secs(3);
const RECOVERY_BUDGET: Duration = Duration::from_secs(3);
// The app gives up on one Space after this; the connect goes on, and the
// next read picks up its outcome.
const APP_PROBE: Duration = Duration::from_millis(2500);
/// This machine's own Spaces.
const LOCAL_SPACES: usize = 10;
const TOKEN: &str = "ada-token";

fn size(var: &str, default: usize) -> usize {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// How a computer behaves in a scenario.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Health {
    Online,
    /// Switched off: the relay lists it offline.
    Offline,
    /// The relay lists it online; its link has this fault.
    Faulty(Fault),
}

impl Health {
    /// Its Spaces are expected to answer.
    fn answers(self) -> bool {
        match self {
            Health::Online => true,
            Health::Offline => false,
            Health::Faulty(f) => matches!(
                f,
                Fault::Pass | Fault::Latency(_) | Fault::Bandwidth(_) | Fault::Lossy { .. }
            ),
        }
    }
}

struct Computer {
    id: String,
    spaces: Vec<String>,
    proxy: FaultProxy,
    _env: MockServer,
}

struct World {
    _dir: tempfile::TempDir,
    home: std::path::PathBuf,
    relay: FakeRelay,
    /// In front of the relay: its faults are the relay's.
    relay_link: FaultProxy,
    computers: Vec<Computer>,
    local: Vec<String>,
    _local_env: MockServer,
    _local_links: Vec<FaultProxy>,
}

fn machine_id(n: usize) -> String {
    format!("{n:016x}")
}

impl World {
    async fn new(hosts: usize, per_host: usize) -> Self {
        let relay = FakeRelay::start().await;
        relay.add_account(TOKEN, "ada", Some("ada@example.com"));
        relay.set_refuse_offline(true);
        let relay_link =
            FaultProxy::start(relay.url.trim_start_matches("http://").parse().unwrap()).await;
        let client = cua_host::RelayClient::new(&relay.url).unwrap();
        let mut computers = Vec::new();
        let mut n = 1;
        for h in 0..hosts {
            let env = MockServer::start(MockAuth {
                token: None,
                gateway: None,
                prefix: None,
            })
            .await;
            let proxy =
                FaultProxy::start(env.url().trim_start_matches("http://").parse().unwrap()).await;
            let id = machine_id(n);
            n += 1;
            client
                .register(
                    TOKEN,
                    &RegisterRequest {
                        id: id.clone(),
                        name: format!("computer-{h}"),
                        allow: vec![],
                        host: None,
                        meta: Default::default(),
                    },
                )
                .await
                .unwrap();
            let mut spaces = Vec::new();
            for s in 0..per_host {
                let sid = machine_id(n);
                n += 1;
                client
                    .register(
                        TOKEN,
                        &RegisterRequest {
                            id: sid.clone(),
                            name: format!("space-{h}-{s}"),
                            allow: vec![],
                            host: Some(id.clone()),
                            meta: Default::default(),
                        },
                    )
                    .await
                    .unwrap();
                spaces.push(sid);
            }
            computers.push(Computer {
                id,
                spaces,
                proxy,
                _env: env,
            });
        }
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join(".cua");
        // This machine's own Spaces: in its registry, read without the
        // network.
        let local_env = MockServer::start(MockAuth {
            token: None,
            gateway: None,
            prefix: None,
        })
        .await;
        let spaces = Spaces::builder().home(&home).build();
        let mut local = Vec::new();
        let mut local_links = Vec::new();
        for i in 0..LOCAL_SPACES {
            // One address each (a Space is registered by its address).
            let link = FaultProxy::start(
                local_env
                    .url()
                    .trim_start_matches("http://")
                    .parse()
                    .unwrap(),
            )
            .await;
            let info = spaces
                .add(&link.url(), None, Some(format!("local-{i}")))
                .await
                .unwrap();
            local.push(info.id);
            local_links.push(link);
        }
        let mut w = Self {
            _dir: dir,
            home,
            relay,
            relay_link,
            computers,
            local,
            _local_env: local_env,
            _local_links: local_links,
        };
        for i in 0..w.computers.len() {
            w.set(i, Health::Online);
        }
        w
    }

    /// Puts computer `i` (and its Spaces) in `health`.
    fn set(&mut self, i: usize, health: Health) {
        let c = &self.computers[i];
        let online = health != Health::Offline;
        for id in std::iter::once(&c.id).chain(c.spaces.iter()) {
            self.relay.set_online(id, online, "1.0.0");
            if online {
                // Idempotent enough: the newest tunnel is the one used.
                self.relay.tunnel(id, &c.proxy.url());
            }
        }
        c.proxy.set(match health {
            Health::Faulty(f) => f,
            _ => Fault::Pass,
        });
    }

    /// Computer `i` and its Spaces connect to the relay again (a new
    /// session).
    fn reconnect(&self, i: usize) {
        let c = &self.computers[i];
        let at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        for id in std::iter::once(&c.id).chain(c.spaces.iter()) {
            self.relay.set_session(id, true, Some(at));
        }
    }

    fn spaces_with(&self, tokens: Arc<dyn AccountTokens>) -> Spaces {
        Spaces::builder()
            .home(&self.home)
            .relay(RelayAccount::new(self.relay_link.url(), tokens))
            .build()
    }

    fn spaces(&self) -> Spaces {
        self.spaces_with(Arc::new(StaticToken(TOKEN.into())))
    }

    fn remote_ids(&self) -> BTreeSet<String> {
        self.computers
            .iter()
            .flat_map(|c| c.spaces.iter().map(|s| format!("relay:{s}")))
            .collect()
    }
}

#[derive(Default, Debug)]
struct Run {
    local: Duration,
    list: Duration,
    listed: usize,
    online_max: Duration,
    offline_max: Duration,
    online_failed: Vec<String>,
    offline_answered: Vec<String>,
}

/// What an app does after a launch: this machine's Spaces, the whole list,
/// then every Space at once (each bounded by the app's deadline).
async fn app_poll(w: &World, spaces: &Spaces, health: &[Health]) -> Run {
    let mut run = Run::default();
    let t = Instant::now();
    let local = spaces.list().unwrap();
    run.local = t.elapsed();
    let mine: BTreeSet<_> = local.iter().map(|i| i.id.clone()).collect();
    for id in &w.local {
        assert!(mine.contains(id), "this machine's Space {id} is not listed");
    }
    let t = Instant::now();
    let listed = tokio::time::timeout(Duration::from_secs(60), spaces.list_all()).await;
    run.list = t.elapsed();
    let listed = match listed {
        Ok(Ok(l)) => l,
        // The relay or the sign-in failed: the list still has this
        // machine's Spaces (what the daemon must show).
        _ => spaces.list().unwrap(),
    };
    run.listed = listed.len();
    let start = Instant::now();
    let mut tasks = Vec::new();
    for (c, h) in w.computers.iter().zip(health) {
        for s in &c.spaces {
            let id = format!("relay:{s}");
            let spaces = spaces.clone();
            let answers = h.answers();
            tasks.push(tokio::spawn(async move {
                let r = tokio::time::timeout(APP_PROBE, spaces.space(&id)).await;
                let ok = matches!(r, Ok(Ok(_)));
                let why = match r {
                    Ok(Ok(_)) => String::new(),
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "the app's deadline".into(),
                };
                (format!("{id}: {why}"), answers, ok, start.elapsed())
            }));
        }
    }
    for t in tasks {
        let (id, answers, ok, took) = t.await.unwrap();
        match (answers, ok) {
            (true, true) => run.online_max = run.online_max.max(took),
            (true, false) => run.online_failed.push(format!("{id} after {took:?}")),
            (false, true) => run.offline_answered.push(id),
            (false, false) => run.offline_max = run.offline_max.max(took),
        }
    }
    run
}

fn check(name: &str, run: &Run, expect_listed: usize) {
    eprintln!(
        "SCALE {name}: local={:?} list={:?} listed={} online_max={:?} offline_max={:?} online_failed={} offline_answered={}",
        run.local,
        run.list,
        run.listed,
        run.online_max,
        run.offline_max,
        run.online_failed.len(),
        run.offline_answered.len()
    );
    assert!(
        run.local <= LOCAL_BUDGET,
        "{name}: local list took {:?}",
        run.local
    );
    assert!(
        run.listed >= expect_listed,
        "{name}: listed {} < {expect_listed}",
        run.listed
    );
    assert!(
        run.list <= LIST_BUDGET,
        "{name}: the list took {:?}",
        run.list
    );
    assert!(
        run.online_failed.is_empty(),
        "{name}: Spaces that answer failed: {:?}",
        &run.online_failed[..run.online_failed.len().min(5)]
    );
    assert!(
        run.offline_answered.is_empty(),
        "{name}: {:?}",
        run.offline_answered
    );
    assert!(
        run.online_max <= ONLINE_BUDGET,
        "{name}: slowest reachable Space took {:?}",
        run.online_max
    );
    assert!(
        run.offline_max <= OFFLINE_BUDGET,
        "{name}: slowest unreachable Space was marked after {:?}",
        run.offline_max
    );
}

async fn world() -> (World, usize) {
    let hosts = size("CUA_SCALE_HOSTS", 20);
    let per = size("CUA_SCALE_SPACES", 10);
    let w = World::new(hosts, per).await;
    let n = hosts * (per + 1) + LOCAL_SPACES;
    (w, n)
}

/// `fraction` of the computers (the first ones) switched off.
async fn down_scenario(name: &str, down: usize) {
    let (mut w, n) = world().await;
    let mut health = vec![Health::Online; w.computers.len()];
    for (i, h) in health.iter_mut().enumerate().take(down) {
        *h = Health::Offline;
        w.set(i, *h);
    }
    let spaces = w.spaces();
    let run = app_poll(&w, &spaces, &health).await;
    check(name, &run, n);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_all_online() {
    down_scenario("all_online", 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_one_down() {
    down_scenario("one_down", 1).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_quarter_down() {
    let hosts = size("CUA_SCALE_HOSTS", 20);
    down_scenario("25pct_down", hosts / 4).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_half_down() {
    let hosts = size("CUA_SCALE_HOSTS", 20);
    down_scenario("50pct_down", hosts / 2).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_ninety_down() {
    let hosts = size("CUA_SCALE_HOSTS", 20);
    down_scenario("90pct_down", hosts * 9 / 10).await;
}

/// Slow, thin and lossy links answer within budget; unresponsive and
/// half-open computers are marked within budget and hold up nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_slow_and_unresponsive_computers() {
    let (mut w, n) = world().await;
    let faults = [
        Fault::Latency(Duration::from_millis(150)),
        Fault::Latency(Duration::from_millis(300)),
        Fault::Bandwidth(256 * 1024),
        Fault::Lossy {
            p: 0.05,
            stall: Duration::from_millis(200),
        },
        Fault::Blackhole,
        Fault::Blackhole,
        Fault::HalfOpen,
        Fault::Reset,
    ];
    let mut health = vec![Health::Online; w.computers.len()];
    for (i, f) in faults.iter().enumerate().take(w.computers.len()) {
        health[i] = Health::Faulty(*f);
        w.set(i, health[i]);
    }
    let spaces = w.spaces();
    let run = app_poll(&w, &spaces, &health).await;
    check("slow_and_unresponsive", &run, n);
}

/// Computers come back: reachable on the next poll, with no ghost and no
/// duplicate rows; one that keeps failing is not dialed on every poll.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_recovery_and_backoff() {
    let (mut w, n) = world().await;
    let hosts = w.computers.len();
    let mut health = vec![Health::Online; hosts];
    // Half off, two unresponsive.
    for (i, h) in health.iter_mut().enumerate().take(hosts / 2) {
        *h = Health::Offline;
        w.set(i, *h);
    }
    for (i, h) in health.iter_mut().enumerate().skip(hosts / 2).take(2) {
        *h = Health::Faulty(Fault::Blackhole);
        w.set(i, *h);
    }
    let spaces = w.spaces();
    let run = app_poll(&w, &spaces, &health).await;
    check("recovery_before", &run, n);

    // Polled again and again: the blackholed computers are not dialed on
    // every poll (backoff), and the offline ones never are.
    let dials_before: u64 = (hosts / 2..(hosts / 2 + 2).min(hosts))
        .map(|i| w.computers[i].proxy.accepted())
        .sum();
    let proxied_before = w.relay.proxied().len();
    for _ in 0..5 {
        let run = app_poll(&w, &spaces, &health).await;
        check("recovery_repoll", &run, n);
    }
    let dials_after: u64 = (hosts / 2..(hosts / 2 + 2).min(hosts))
        .map(|i| w.computers[i].proxy.accepted())
        .sum();
    let offline_spaces = (hosts / 2) * w.computers[0].spaces.len();
    let proxied = w.relay.proxied().len() - proxied_before;
    eprintln!(
        "SCALE backoff: blackholed dials over 5 polls={} relay requests={proxied}",
        dials_after - dials_before
    );
    assert_eq!(
        dials_after - dials_before,
        0,
        "unresponsive computers were dialed again inside their backoff"
    );
    assert!(
        proxied < offline_spaces,
        "offline Spaces were dialed through the relay on every poll ({proxied} requests)"
    );

    // Everything comes back: each computer reconnects to the relay (a new
    // session), as one that wakes up or boots does.
    for (i, h) in health.iter_mut().enumerate() {
        *h = Health::Online;
        w.set(i, *h);
        w.reconnect(i);
    }
    let t = Instant::now();
    let run = app_poll(&w, &spaces, &health).await;
    let took = t.elapsed();
    check("recovered", &run, n);
    assert!(took <= RECOVERY_BUDGET, "recovery took {took:?}");
    let listed = spaces.list_all().await.unwrap();
    let ids: BTreeSet<_> = listed.iter().map(|i| i.id.clone()).collect();
    assert_eq!(ids.len(), listed.len(), "duplicate rows");
    for id in w.remote_ids() {
        assert!(ids.contains(&id), "{id} missing after recovery");
    }

    // A computer removed from the account leaves the list (no ghost).
    let gone = w.computers[0].spaces[0].clone();
    cua_host::RelayClient::new(&w.relay.url)
        .unwrap()
        .delete(TOKEN, &gone)
        .await
        .unwrap();
    // Past the directory's cache age.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let listed = spaces.list_all().await.unwrap();
    assert!(
        !listed.iter().any(|i| i.id == format!("relay:{gone}")),
        "a removed Space is still listed"
    );
}

/// A daemon restart in the middle of a sync: the new process lists this
/// machine's Spaces at once and converges with no duplicates.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scale_restart_mid_sync() {
    let (mut w, n) = world().await;
    let hosts = w.computers.len();
    let mut health = vec![Health::Online; hosts];
    for (i, h) in health.iter_mut().enumerate().take(hosts / 4) {
        *h = Health::Offline;
        w.set(i, *h);
    }
    let first = w.spaces();
    let polling = {
        let first = first.clone();
        let ids: Vec<String> = w.remote_ids().into_iter().collect();
        tokio::spawn(async move {
            let _ = first.list_all().await;
            for id in ids {
                let _ = tokio::time::timeout(APP_PROBE, first.space(&id)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    polling.abort();
    drop(first);
    let second = w.spaces();
    let run = app_poll(&w, &second, &health).await;
    check("restart_mid_sync", &run, n);
    let listed = second.list_all().await.unwrap();
    let ids: BTreeSet<_> = listed.iter().map(|i| i.id.clone()).collect();
    assert_eq!(ids.len(), listed.len(), "duplicate rows after a restart");
}
