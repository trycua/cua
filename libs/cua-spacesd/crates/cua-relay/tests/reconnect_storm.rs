// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A reconnect storm: a thousand machines join at once, the network between
//! them and the relay is cut and comes back, and they all rejoin. What keeps
//! that from starving the relay:
//!
//! * the relay admits joins at a steady rate (token bucket) and tells the rest
//!   to come back after a jittered `Retry-After` (HTTP 503), which the client
//!   honours;
//! * a machine whose session was up retries first after a random wait over
//!   `reconnect_spread`, not within half a second.
//!
//! The network is a TCP forwarder that can be cut (every connection dropped)
//! and held down for a moment. A second test checks the per-token machine
//! limit: the machines past it keep trying, and take a freed place.
//!
//! Prints `STORM` lines (time to all online, the most joins in one second,
//! the slowest `/healthz` during it) to compare runs. Debug build and a
//! laptop: a few tens of seconds.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_relay::server::{Relay, RelayConfig};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "static-registration-token";
const MACHINES: usize = 1000;

/// A thousand tunnels are four thousand sockets with the forwarder in the
/// middle: more than a default shell allows.
#[cfg(unix)]
fn raise_fd_limit() {
    unsafe {
        let mut r = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) == 0 {
            r.rlim_cur = r.rlim_max.min(10_240);
            libc::setrlimit(libc::RLIMIT_NOFILE, &r);
        }
    }
}
#[cfg(not(unix))]
fn raise_fd_limit() {}

/// A forwarder in front of the relay: `cut` drops every connection through it,
/// `down` makes it refuse new ones until `up`.
struct Network {
    addr: std::net::SocketAddr,
    conns: Arc<Mutex<Vec<AbortHandle>>>,
    down: Arc<AtomicBool>,
    attempts: Arc<AtomicUsize>,
}

impl Network {
    async fn start(relay: std::net::SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let conns: Arc<Mutex<Vec<AbortHandle>>> = Arc::default();
        let down = Arc::new(AtomicBool::new(false));
        let attempts = Arc::new(AtomicUsize::new(0));
        let (c, d, a) = (conns.clone(), down.clone(), attempts.clone());
        tokio::spawn(async move {
            while let Ok((mut inbound, _)) = listener.accept().await {
                a.fetch_add(1, Ordering::Relaxed);
                if d.load(Ordering::SeqCst) {
                    continue; // dropped: the machine sees a failed connect
                }
                let task = tokio::spawn(async move {
                    if let Ok(mut outbound) = TcpStream::connect(relay).await {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                c.lock().unwrap().push(task.abort_handle());
            }
        });
        Self { addr, conns, down, attempts }
    }

    fn cut(&self) {
        for h in self.conns.lock().unwrap().drain(..) {
            h.abort();
        }
    }
}

async fn start_relay(config: RelayConfig) -> (Relay, std::net::SocketAddr) {
    let relay = Relay::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (relay, addr)
}

/// A local spacesd stand-in every machine splices streams into (none are opened here).
async fn backend() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    addr
}

fn join_config(relay: std::net::SocketAddr, id: &str, local: std::net::SocketAddr) -> cua_relay::client::JoinConfig {
    let mut c = cua_relay::client::JoinConfig::new(format!("ws://{relay}"), TOKEN.into(), id.into(), local);
    c.heartbeat = Duration::from_secs(5);
    c.max_backoff = Duration::from_secs(4);
    c.reconnect_spread = Duration::from_secs(3);
    c
}

/// Waits for `want` machines online, noting the most that joined in one
/// second and the slowest `/healthz` meanwhile.
async fn until_online(relay: &Relay, base: &str, want: usize, within: Duration) -> (Duration, usize, Duration) {
    let started = Instant::now();
    let client = reqwest::Client::builder().pool_max_idle_per_host(0).build().unwrap();
    let (mut peak, mut slowest) = (0usize, Duration::ZERO);
    let mut window: std::collections::VecDeque<(Instant, usize)> = std::collections::VecDeque::new();
    loop {
        let online = relay.machine_ids().len();
        let now = Instant::now();
        window.push_back((now, online));
        while window.front().is_some_and(|(t, _)| now - *t > Duration::from_secs(1)) {
            window.pop_front();
        }
        let low = window.iter().map(|(_, n)| *n).min().unwrap_or(online);
        peak = peak.max(online.saturating_sub(low));
        let t = Instant::now();
        let health = client.get(format!("{base}/healthz")).timeout(Duration::from_secs(5)).send().await;
        slowest = slowest.max(t.elapsed());
        assert!(health.is_ok_and(|h| h.status().is_success()), "the relay stopped answering during the storm");
        if online >= want {
            return (started.elapsed(), peak, slowest);
        }
        assert!(started.elapsed() < within, "{online} of {want} online after {within:?}");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thousand_machines_cut_off_and_back_do_not_starve_the_relay() {
    raise_fd_limit();
    let (relay, relay_addr) = start_relay(RelayConfig {
        tokens: vec![TOKEN.into()],
        require_client_credentials: false,
        max_machines_per_token: MACHINES + 100,
        ..RelayConfig::default()
    })
    .await;
    let base = format!("http://{relay_addr}");
    let net = Network::start(relay_addr).await;
    let local = backend().await;
    let stop = CancellationToken::new();
    for i in 0..MACHINES {
        let id = format!("storm{i:04}{}", uuid::Uuid::new_v4().simple());
        tokio::spawn(cua_relay::client::run(join_config(net.addr, &id, local), stop.clone()));
    }

    // 1. Every machine knocks at once: the relay admits a burst then a steady
    //    rate, and the rest come back when told.
    let (first, peak, slowest) = until_online(&relay, &base, MACHINES, Duration::from_secs(90)).await;
    println!("STORM join: machines={MACHINES} all online in {first:?}, most joined in one second={peak}, slowest /healthz={slowest:?}");
    assert!(peak <= 800, "{peak} machines joined in one second: the limiter (burst 500, 250/s) did not hold");

    // 2. The network is cut, held down for a second, and comes back.
    tokio::time::sleep(Duration::from_secs(11)).await; // sessions count as "was up" after 10 s
    net.down.store(true, Ordering::SeqCst);
    net.cut();
    for _ in 0..100 {
        if relay.machine_ids().len() < MACHINES / 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let dropped = relay.machine_ids().len();
    assert!(dropped < MACHINES / 2, "{dropped} machines still online after the cut");
    tokio::time::sleep(Duration::from_secs(1)).await;
    let attempts_before = net.attempts.load(Ordering::Relaxed);
    net.down.store(false, Ordering::SeqCst);
    let back = Instant::now();

    let (again, peak, slowest) = until_online(&relay, &base, MACHINES, Duration::from_secs(60)).await;
    let attempts = net.attempts.load(Ordering::Relaxed) - attempts_before;
    println!(
        "STORM rejoin: all online again {again:?} after the network came back, connection attempts={attempts} for {MACHINES} machines, most joined in one second={peak}, slowest /healthz={slowest:?} ({:?} since)",
        back.elapsed()
    );
    assert!(peak <= 800, "{peak} machines joined in one second after the cut");
    assert!(slowest < Duration::from_secs(3), "/healthz took {slowest:?} during the rejoin");
    // Spread, not hammered: each machine knocked a few times at most.
    assert!(attempts <= MACHINES * 4, "{attempts} connection attempts for {MACHINES} machines");
    stop.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn machines_past_the_token_limit_keep_trying_and_take_a_freed_place() {
    const LIMIT: usize = 8;
    const TOTAL: usize = 20;
    let (relay, addr) = start_relay(RelayConfig {
        tokens: vec![TOKEN.into()],
        require_client_credentials: false,
        max_machines_per_token: LIMIT,
        ..RelayConfig::default()
    })
    .await;
    let local = backend().await;
    let mut stops = Vec::new();
    let mut ids = Vec::new();
    for i in 0..TOTAL {
        let id = format!("limit{i:02}{}", uuid::Uuid::new_v4().simple());
        let stop = CancellationToken::new();
        let mut config = join_config(addr, &id, local);
        config.max_backoff = Duration::from_millis(800);
        tokio::spawn(cua_relay::client::run(config, stop.clone()));
        stops.push(stop);
        ids.push(id);
    }
    let settle = |want: usize| {
        let relay = relay.clone();
        async move {
            let deadline = Instant::now() + Duration::from_secs(20);
            while relay.machine_ids().len() != want {
                assert!(Instant::now() < deadline, "{} online, wanted {want}", relay.machine_ids().len());
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    // Exactly the limit is online; the others are refused (429) and retry.
    settle(LIMIT).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(relay.machine_ids().len(), LIMIT, "the limit holds while the rest keep trying");

    // Three leave: three of the waiting take their places, the limit still holds.
    let online_before: std::collections::BTreeSet<String> = relay.machine_ids().into_iter().collect();
    let leaving: Vec<usize> = ids.iter().enumerate().filter(|(_, id)| online_before.contains(*id)).map(|(i, _)| i).take(3).collect();
    for i in &leaving {
        stops[*i].cancel();
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let now: std::collections::BTreeSet<String> = relay.machine_ids().into_iter().collect();
        let newcomers = now.difference(&online_before).count();
        if newcomers == 3 && now.len() == LIMIT {
            break;
        }
        assert!(now.len() <= LIMIT, "{} online past the limit of {LIMIT}", now.len());
        assert!(Instant::now() < deadline, "the freed places were not taken: {} online, {newcomers} new", now.len());
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for s in &stops {
        s.cancel();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_busy_relay_answers_503_with_a_retry_after() {
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Error};
    let (_relay, addr) = start_relay(RelayConfig {
        tokens: vec![TOKEN.into()],
        require_client_credentials: false,
        // Two joins at once, then one every five seconds.
        join_burst: 2,
        join_rate_per_sec: 0.2,
        ..RelayConfig::default()
    })
    .await;
    let join = |id: &str| {
        let mut request = format!("ws://{addr}{}", cua_relay::CONNECT_PATH).into_client_request().unwrap();
        let h = request.headers_mut();
        h.insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        h.insert(cua_relay::MACHINE_ID_HEADER, id.parse().unwrap());
        async move { tokio_tungstenite::connect_async(request).await }
    };
    let (_a, _) = join("busy-aaaa").await.expect("the first is admitted");
    let (_b, _) = join("busy-bbbb").await.expect("the second is admitted");
    match join("busy-cccc").await {
        Err(Error::Http(response)) => {
            assert_eq!(response.status(), 503);
            let secs: u64 = response.headers().get("retry-after").expect("a Retry-After").to_str().unwrap().parse().unwrap();
            // Five seconds for the next token, plus up to five more of spread.
            assert!((1..=11).contains(&secs), "Retry-After {secs}");
        }
        other => panic!("the third join should be told to wait, got {:?}", other.map(|_| ())),
    }
}
