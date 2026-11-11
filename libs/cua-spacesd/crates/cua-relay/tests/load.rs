// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Load through the relay: many clients at once to joined machines, with
//! request-rate and latency floors. A real relay and real machine tunnels
//! (`client::run`) to a small HTTP backend per machine; every request goes
//! client -> relay -> tunnel -> backend and back. Prints one `LOAD` line per
//! scenario (requests, rate, p50/p99) so a run can be compared with the
//! last. The floors are far below what a laptop does (debug build) so they
//! catch a collapse (a lock held across a stream, a serialised tunnel), not
//! noise.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_relay::server::{Relay, RelayConfig};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "static-registration-token";

/// A machine's local spacesd stand-in: answers every request with a short
/// body naming the machine.
async fn backend(name: String) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            let name = name.clone();
            tokio::spawn(async move {
                let svc = hyper::service::service_fn(
                    move |_req: Request<hyper::body::Incoming>| {
                        let body = format!("ok {name}");
                        async move { Ok::<_, Infallible>(Response::new(Full::new(Bytes::from(body)))) }
                    },
                );
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    addr
}

async fn start_relay() -> (Relay, String) {
    let relay = Relay::new(RelayConfig {
        tokens: vec![TOKEN.into()],
        require_client_credentials: false,
        // One static token joins every machine here (the default caps a token at 16).
        max_machines_per_token: 64,
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

async fn join(relay: &Relay, base: &str, stop: &CancellationToken) -> String {
    let id = format!("load{}", uuid::Uuid::new_v4().simple());
    let addr = backend(id.clone()).await;
    let mut config = cua_relay::client::JoinConfig::new(
        base.replace("http://", "ws://"),
        TOKEN.into(),
        id.clone(),
        addr,
    );
    config.heartbeat = Duration::from_secs(5);
    tokio::spawn(cua_relay::client::run(config, stop.clone()));
    for _ in 0..400 {
        if relay.machine_ids().contains(&id) {
            return id;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{id} did not join");
}

struct Run {
    rate: f64,
    p50: Duration,
    p99: Duration,
    failed: usize,
}

/// `total` requests from `concurrency` clients at once, spread over
/// `machines`; every answer must be that machine's own.
async fn drive(base: &str, machines: &[String], concurrency: usize, total: usize) -> Run {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(concurrency)
        .build()
        .unwrap();
    let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let start = Instant::now();
    let mut tasks = Vec::new();
    for _ in 0..concurrency {
        let (client, next, base, machines) = (
            client.clone(),
            next.clone(),
            base.to_string(),
            machines.to_vec(),
        );
        tasks.push(tokio::spawn(async move {
            let (mut latencies, mut failed) = (Vec::new(), 0usize);
            loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if i >= total {
                    return (latencies, failed);
                }
                let id = &machines[i % machines.len()];
                let t = Instant::now();
                let ok = match client
                    .get(format!("{base}/m/{id}/ping"))
                    .header("x-cua-env-authorization", "Bearer t")
                    .timeout(Duration::from_secs(10))
                    .send()
                    .await
                {
                    Ok(r) if r.status().is_success() => {
                        r.text().await.is_ok_and(|b| b == format!("ok {id}"))
                    }
                    _ => false,
                };
                latencies.push(t.elapsed());
                if !ok {
                    failed += 1;
                }
            }
        }));
    }
    let (mut latencies, mut failed) = (Vec::with_capacity(total), 0);
    for t in tasks {
        let (l, f) = t.await.unwrap();
        latencies.extend(l);
        failed += f;
    }
    let elapsed = start.elapsed();
    latencies.sort();
    let at = |q: f64| latencies[((latencies.len() as f64 * q) as usize).min(latencies.len() - 1)];
    Run {
        rate: total as f64 / elapsed.as_secs_f64(),
        p50: at(0.50),
        p99: at(0.99),
        failed,
    }
}

fn report(name: &str, total: usize, r: &Run) {
    println!(
        "LOAD {name}: requests={total} rate={:.0}/s p50={:?} p99={:?} failed={}",
        r.rate, r.p50, r.p99, r.failed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_machine_takes_many_clients_at_once() {
    let (relay, base) = start_relay().await;
    let stop = CancellationToken::new();
    let id = join(&relay, &base, &stop).await;
    const TOTAL: usize = 3000;
    let r = drive(&base, &[id], 64, TOTAL).await;
    report("one machine, 64 clients", TOTAL, &r);
    assert_eq!(r.failed, 0, "every request answered by its machine");
    assert!(
        r.rate >= 200.0,
        "{:.0} requests/s through one tunnel",
        r.rate
    );
    assert!(r.p99 < Duration::from_secs(2), "p99 {:?}", r.p99);
    stop.cancel();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_machines_share_the_relay_fairly() {
    let (relay, base) = start_relay().await;
    let stop = CancellationToken::new();
    let mut ids = Vec::new();
    for _ in 0..20 {
        ids.push(join(&relay, &base, &stop).await);
    }
    const TOTAL: usize = 4000;
    let r = drive(&base, &ids, 128, TOTAL).await;
    report("20 machines, 128 clients", TOTAL, &r);
    assert_eq!(r.failed, 0, "every request answered by its own machine");
    assert!(
        r.rate >= 200.0,
        "{:.0} requests/s across 20 tunnels",
        r.rate
    );
    assert!(r.p99 < Duration::from_secs(2), "p99 {:?}", r.p99);
    // Still answering its own health check right after.
    let health = reqwest::Client::new()
        .get(format!("{base}/healthz"))
        .timeout(Duration::from_millis(500))
        .send()
        .await
        .unwrap();
    assert!(health.status().is_success());
    stop.cancel();
}
