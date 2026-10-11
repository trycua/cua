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
