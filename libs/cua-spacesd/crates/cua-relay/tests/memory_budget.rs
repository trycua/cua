// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! S7: the relay's global concurrent-stream budget gives backpressure
//! instead of buffering without bound. A machine whose own spacesd never
//! reads or responds (the worst case: a slow reader, or one an attacker
//! wedges open on purpose) can only ever hold open as many streams as the
//! budget allows; every stream past it is refused at once, and the relay
//! itself stays responsive throughout.

use std::sync::Arc;
use std::time::Duration;

use cua_relay::server::{Relay, RelayConfig};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "static-registration-token";

/// A "local spacesd" that accepts every connection and then never reads or
/// writes a single byte: the worst-case slow reader / stuck backend. Kept
/// open for the test's lifetime so the relay's h1 handshake to it hangs
/// forever instead of erroring.
async fn slow_backend() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    addr
}

async fn start_relay(max_global_streams: u64) -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        tokens: vec![TOKEN.into()],
        max_global_streams,
        require_client_credentials: false,
        ..RelayConfig::default()
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let serving = relay.clone();
    tokio::spawn(async move {
        let _ = serving.serve(listener, std::future::pending()).await;
    });
    (relay, base)
}

#[tokio::test]
async fn the_global_stream_budget_refuses_instead_of_buffering_without_bound() {
    const BUDGET: u64 = 3;
    const TOTAL_REQUESTS: usize = BUDGET as usize + 5;

    let (relay, base) = start_relay(BUDGET).await;
    let backend = slow_backend().await;
    let id = format!("slow{}", uuid::Uuid::new_v4().simple());
    let mut join = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        TOKEN.into(),
        id.clone(),
        backend,
    );
    join.heartbeat = Duration::from_secs(5);
    let stop = CancellationToken::new();
    tokio::spawn(cua_relay::client::run(join, stop.clone()));
    for _ in 0..200 {
        if relay.machine_ids().contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(relay.machine_ids().contains(&id), "slow machine joined");

    // Fire every request at once. Each opens a fresh connection (a fresh
    // client per request), so none share a pooled connection that would
    // serialize them.
    let client = Arc::new(
        reqwest::Client::builder()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap(),
    );
    let mut tasks = Vec::new();
    for _ in 0..TOTAL_REQUESTS {
        let client = client.clone();
        let url = format!("{base}/m/{id}/anything");
        tasks.push(tokio::spawn(async move {
            // The budget check answers at once; a request that gets a slot
            // then hangs forever on the backend's silence. 300ms is ample
            // margin between "answered immediately" and "still pending".
            tokio::time::timeout(
                Duration::from_millis(300),
                client
                    .get(&url)
                    .header("x-cua-env-authorization", "Bearer t")
                    .send(),
            )
            .await
        }));
    }

    let mut refused = 0;
    let mut still_pending = 0;
    for t in tasks {
        match t.await.unwrap() {
            Ok(Ok(resp)) => {
                assert_eq!(
                    resp.status(),
                    reqwest::StatusCode::SERVICE_UNAVAILABLE,
                    "a request that got an answer at all must be the budget refusal"
                );
                refused += 1;
            }
            Ok(Err(e)) => panic!("unexpected transport error: {e}"),
            Err(_timed_out) => still_pending += 1,
        }
    }
    // Exactly the budget's worth of streams got through to hang on the
    // silent backend; everything past it was refused at once, not queued.
    assert_eq!(still_pending, BUDGET as usize, "streams within budget");
    assert_eq!(
        refused,
        TOTAL_REQUESTS - BUDGET as usize,
        "streams past budget refused, not buffered"
    );

    // The relay process itself stayed responsive throughout: no global lock
    // or unbounded buffer starved it.
    let health = reqwest::Client::new()
        .get(format!("{base}/healthz"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .expect("relay still answers while streams are stuck");
    assert!(health.status().is_success());

    stop.cancel();
}

async fn metric(base: &str, name: &str) -> u64 {
    let text = reqwest::get(format!("{base}/metrics"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    text.lines()
        .find_map(|l| l.strip_prefix(name)?.trim().parse().ok())
        .unwrap_or_else(|| panic!("{name} missing from /metrics"))
}

async fn wait_for(what: &str, mut ok: impl AsyncFnMut() -> bool) {
    for _ in 0..200 {
        if ok().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

/// A refusal at the budget carries a jittered `Retry-After`, never touches
/// the probe, metrics, tunnel and mesh routes, leaves the streams that are
/// open alone, and a closed stream gives its slot back.
#[tokio::test]
async fn a_refusal_retries_later_and_spares_open_streams_and_other_routes() {
    const BUDGET: u64 = 2;
    let (relay, base) = start_relay(BUDGET).await;
    let backend = slow_backend().await;
    let join_machine = |id: String| {
        let mut join = cua_relay::client::JoinConfig::new(
            base.replace("http://", "ws://"),
            TOKEN.into(),
            id,
            backend,
        );
        join.heartbeat = Duration::from_secs(5);
        let stop = CancellationToken::new();
        tokio::spawn(cua_relay::client::run(join, stop.clone()));
        stop
    };
    let id = format!("slow{}", uuid::Uuid::new_v4().simple());
    let stop = join_machine(id.clone());
    wait_for("machine joined", async || {
        relay.machine_ids().contains(&id)
    })
    .await;

    // Hold BUDGET streams open (the futures live in tasks we keep).
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let held: Vec<_> = (0..BUDGET)
        .map(|_| {
            let client = client.clone();
            let url = format!("{base}/m/{id}/held");
            tokio::spawn(async move {
                client
                    .get(&url)
                    .header("x-cua-env-authorization", "Bearer t")
                    .send()
                    .await
            })
        })
        .collect();
    wait_for("streams open", async || {
        metric(&base, "cua_relay_client_streams").await == BUDGET
    })
    .await;

    // Past the budget: 503 with a Retry-After of 1 to 5 seconds, every time.
    for _ in 0..8 {
        let resp = client
            .get(format!("{base}/m/{id}/more"))
            .header("x-cua-env-authorization", "Bearer t")
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
        let secs: u64 = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .expect("the budget refusal says when to retry")
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=5).contains(&secs), "Retry-After {secs}");
    }
    assert_eq!(metric(&base, "cua_relay_client_streams_refused_total").await, 8);

    // The probes and the metrics are not streams, so they are never refused.
    for path in ["/healthz", "/readyz", "/metrics"] {
        let resp = client.get(format!("{base}{path}")).send().await.unwrap();
        assert!(resp.status().is_success(), "{path}: {}", resp.status());
    }
    // Nor is a new machine tunnel...
    let id2 = format!("other{}", uuid::Uuid::new_v4().simple());
    let stop2 = join_machine(id2.clone());
    wait_for("second machine joined at the budget", async || {
        relay.machine_ids().contains(&id2)
    })
    .await;
    // ...or a mesh node.
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = cua_mesh::MeshConfig::new(
        base.clone(),
        cua_mesh::NodeKind::Client,
        Some(dir.path().join("node.json")),
        cua_mesh::static_auth(Vec::new()),
    );
    cfg.relay_only = true;
    let node = cua_mesh::Mesh::start(cfg).await.expect("mesh node starts");
    wait_for("mesh node connected at the budget", async || {
        cua_relay::mesh::status(&relay).connected >= 1
    })
    .await;
    node.shutdown();

    // The open streams were not touched by any of it.
    assert_eq!(metric(&base, "cua_relay_client_streams").await, BUDGET);
    assert!(held.iter().all(|t| !t.is_finished()), "open streams kept");

    // A closed stream gives its slot back: the next request gets in (and
    // hangs on the silent backend instead of being refused).
    held[0].abort();
    wait_for("a slot freed", async || {
        metric(&base, "cua_relay_client_streams").await < BUDGET
    })
    .await;
    let next = tokio::time::timeout(
        Duration::from_millis(300),
        client
            .get(format!("{base}/m/{id}/again"))
            .header("x-cua-env-authorization", "Bearer t")
            .send(),
    )
    .await;
    assert!(next.is_err(), "the freed slot was taken, not refused");

    for t in held {
        t.abort();
    }
    wait_for("every slot returned", async || {
        metric(&base, "cua_relay_client_streams").await == 0
    })
    .await;
    stop.cancel();
    stop2.cancel();
}
