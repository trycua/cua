// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Measures a machine's spacesd over the mesh against the relay's HTTPS
//! path: connect time, call round trips, download throughput, and how
//! often (and how fast) the mesh finds a direct path.
//!
//! ```text
//! mesh-bench --relay https://relay.example --machine <id> --env-token <t> \
//!   [--path mesh|relay|both] [--rtt 200] [--mib 256] [--direct-trials 10]
//! ```
//!
//! Static-token relays only (the env token is the credential on both
//! paths). Prints one JSON object per measurement.

use std::time::{Duration, Instant};

use cua_mesh::dialer::{Dialer, SESSION_HEADER};
use cua_proto::env::v1 as pb;
use cua_spacesd_client::{ConnectOptions, SpacesdClient, TransportPreference};

struct Args {
    relay: String,
    machine: String,
    token: String,
    path: String,
    rtt: usize,
    mib: u64,
    direct_trials: usize,
    hold_secs: u64,
    rebind_at: Vec<u64>,
    relay_only: bool,
    serve_secs: u64,
    tunnel_port: u16,
}

fn args() -> Args {
    let mut a = Args {
        relay: String::new(),
        machine: String::new(),
        token: std::env::var("CUA_ENV_TOKEN").unwrap_or_default(),
        path: "both".into(),
        rtt: 200,
        mib: 256,
        direct_trials: 0,
        hold_secs: 0,
        rebind_at: Vec::new(),
        relay_only: false,
        serve_secs: 0,
        tunnel_port: 0,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().expect("value");
        match k.as_str() {
            "--relay" => a.relay = v(),
            "--machine" => a.machine = v(),
            "--env-token" => a.token = v(),
            "--path" => a.path = v(),
            "--rtt" => a.rtt = v().parse().expect("number"),
            "--mib" => a.mib = v().parse().expect("number"),
            "--direct-trials" => a.direct_trials = v().parse().expect("number"),
            "--hold" => a.hold_secs = v().parse().expect("number"),
            // Move to new UDP ports at these seconds of --hold (a NAT
            // rebinding, as after a network change).
            "--rebind-at" => a.rebind_at.push(v().parse().expect("number")),
            "--relay-only" => a.relay_only = true,
            // Keep a loopback route to the machine open for other tools
            // (the streaming bench): prints its addresses, then waits.
            "--serve" => a.serve_secs = v().parse().expect("number"),
            // With --serve: also forward this guest TCP port (spacesd's
            // ticketed tunnel, over the same path).
            "--tunnel-port" => a.tunnel_port = v().parse().expect("number"),
            other => panic!("unknown argument {other}"),
        }
    }
    assert!(!a.relay.is_empty() && !a.machine.is_empty() && !a.token.is_empty());
    a
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

async fn connect(options: ConnectOptions) -> SpacesdClient {
    SpacesdClient::connect(options.transport(TransportPreference::Native))
        .await
        .expect("connect")
}

async fn measure(label: &str, client: &SpacesdClient, a: &Args, extra: serde_json::Value) {
    let mut rtts = Vec::with_capacity(a.rtt);
    for _ in 0..a.rtt {
        let t = Instant::now();
        client
            .system()
            .health(pb::HealthRequest {})
            .await
            .expect("health");
        rtts.push(t.elapsed().as_secs_f64() * 1000.0);
    }
    // Download and upload a file of `mib` MiB of random bytes through the
    // files API (chunked, SHA-256 verified): a lossless transfer.
    let bytes = a.mib << 20;
    let remote = format!("/tmp/wgm-bench-{}.bin", std::process::id());
    let mut stream = client
        .process()
        .start_process(pb::StartProcessRequest {
            config: Some(pb::ProcessConfig {
                command: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    format!("head -c {bytes} /dev/urandom > {remote}"),
                ],
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .expect("start")
        .into_inner();
    while let Some(m) = stream.message().await.expect("stream") {
        if matches!(
            m.event.and_then(|e| e.event),
            Some(pb::process_event::Event::End(_))
        ) {
            break;
        }
    }
    // The file is complete once it has every byte (bounded wait).
    for _ in 0..600 {
        if client.stat(&remote).await.map(|e| e.size).unwrap_or(0) >= bytes {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let t = Instant::now();
    let data = client.download(&remote).await.expect("download");
    let down_secs = t.elapsed().as_secs_f64();
    let got = data.len() as u64;
    let t = Instant::now();
    client
        .upload(
            &format!("{remote}.up"),
            data,
            cua_spacesd_client::UploadOptions::default(),
        )
        .await
        .expect("upload");
    let up_secs = t.elapsed().as_secs_f64();
    let _ = client.remove(&remote, false).await;
    let _ = client.remove(&format!("{remote}.up"), false).await;
    let secs = down_secs;
    let out = serde_json::json!({
        "path": label,
        "rtt_ms_p50": pct(&mut rtts.clone(), 0.5),
        "rtt_ms_p95": pct(&mut rtts.clone(), 0.95),
        "rtt_ms_min": pct(&mut rtts.clone(), 0.0),
        "download_bytes": got,
        "download_mbit_s": got as f64 * 8.0 / secs / 1e6,
        "upload_mbit_s": got as f64 * 8.0 / up_secs / 1e6,
        "extra": extra,
    });
    println!("{out}");
}

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    let a = args();
    if a.serve_secs > 0 {
        serve(&a).await;
        return;
    }
    if a.path == "relay" || a.path == "both" {
        let t = Instant::now();
        let url = format!("{}/m/{}", a.relay.trim_end_matches('/'), a.machine);
        let c = connect(ConnectOptions::parse(&url).unwrap().token(a.token.clone())).await;
        c.health().await.expect("health");
        let connect_ms = t.elapsed().as_secs_f64() * 1000.0;
        measure(
            "relay-https",
            &c,
            &a,
            serde_json::json!({"connect_ms": connect_ms}),
        )
        .await;
    }
    if a.path == "mesh" || a.path == "both" {
        let t = Instant::now();
        let dialer = Dialer::start(
            &a.relay,
            cua_mesh::static_auth(Vec::new()),
            None,
            Duration::from_secs(15),
        )
        .await
        .expect("mesh");
        let route = dialer
            .route(&a.machine, Some(&a.token), Duration::from_secs(10))
            .await
            .expect("route");
        let mut o = ConnectOptions::parse(&format!("http://{}", route.local))
            .unwrap()
            .token(a.token.clone());
        if let Some(s) = &route.session {
            o = o.header(SESSION_HEADER, s.clone());
        }
        let c = connect(o).await;
        c.health().await.expect("health");
        let connect_ms = t.elapsed().as_secs_f64() * 1000.0;
        let path_at_connect = dialer.mesh().path(&route.peer).map(|p| p.to_string());
        // Time to a direct path, with traffic flowing.
        let mut direct_ms = None;
        for _ in 0..150 {
            if matches!(
                dialer.mesh().path(&route.peer),
                Some(cua_mesh::Path::Direct(_))
            ) {
                direct_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
                break;
            }
            let _ = c.system().health(pb::HealthRequest {}).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let path = dialer.mesh().path(&route.peer).map(|p| p.to_string());
        let label = match dialer.mesh().path(&route.peer) {
            Some(cua_mesh::Path::Direct(_)) => "mesh-direct",
            _ => "mesh-relayed",
        };
        measure(
            label,
            &c,
            &a,
            serde_json::json!({
                "connect_ms": connect_ms,
                "path_at_connect": path_at_connect,
                "time_to_direct_ms": direct_ms,
                "path": path,
            }),
        )
        .await;
        let st = dialer.mesh().status();
        println!("{}", serde_json::json!({"status": st}));
        if a.hold_secs > 0 {
            // Keep a call going every second (outage tests watch this).
            if a.relay_only {
                dialer.mesh().set_relay_only(true);
            }
            for i in 0..a.hold_secs {
                if a.rebind_at.contains(&i) {
                    dialer.mesh().rebind().await.expect("rebind");
                    println!("{}", serde_json::json!({"t": i, "event": "rebind"}));
                }
                let t = Instant::now();
                let ok = tokio::time::timeout(
                    Duration::from_secs(5),
                    c.system().health(pb::HealthRequest {}),
                )
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);
                println!(
                    "{}",
                    serde_json::json!({
                        "t": i,
                        "ok": ok,
                        "ms": t.elapsed().as_secs_f64() * 1000.0,
                        "path": dialer.mesh().path(&route.peer).map(|p| p.to_string()),
                        "coordinator": dialer.mesh().control_state().connected,
                    })
                );
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
    // Direct-path rate: fresh nodes, each paired anew.
    for trial in 0..a.direct_trials {
        let t = Instant::now();
        let dialer = Dialer::start(
            &a.relay,
            cua_mesh::static_auth(Vec::new()),
            None,
            Duration::from_secs(15),
        )
        .await
        .expect("mesh");
        let ready_ms = t.elapsed().as_secs_f64() * 1000.0;
        let route = dialer
            .route(&a.machine, Some(&a.token), Duration::from_secs(10))
            .await
            .expect("route");
        let routed_ms = t.elapsed().as_secs_f64() * 1000.0;
        let c = connect(
            ConnectOptions::parse(&format!("http://{}", route.local))
                .unwrap()
                .token(a.token.clone()),
        )
        .await;
        let connected_ms = t.elapsed().as_secs_f64() * 1000.0;
        let mut direct_ms = None;
        for _ in 0..150 {
            let _ = c.system().health(pb::HealthRequest {}).await;
            if matches!(
                dialer.mesh().path(&route.peer),
                Some(cua_mesh::Path::Direct(_))
            ) {
                direct_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        println!(
            "{}",
            serde_json::json!({"trial": trial, "direct": direct_ms.is_some(), "time_to_direct_ms": direct_ms,
                "ready_ms": ready_ms, "paired_and_forwarded_ms": routed_ms, "connected_ms": connected_ms,
                "path": dialer.mesh().path(&route.peer).map(|p| p.to_string())})
        );
    }
}

/// `--serve`: a route to the machine (the mesh, or the relay's HTTPS path
/// with `--path relay`) for another tool, plus a tunnel to a guest port.
async fn serve(a: &Args) {
    let mut dialer = None;
    let (url, session) = if a.path == "relay" {
        (
            format!("{}/m/{}", a.relay.trim_end_matches('/'), a.machine),
            None,
        )
    } else {
        let d = Dialer::start(
            &a.relay,
            cua_mesh::static_auth(Vec::new()),
            None,
            Duration::from_secs(15),
        )
        .await
        .expect("mesh");
        let route = d
            .route(&a.machine, Some(&a.token), Duration::from_secs(10))
            .await
            .expect("route");
        let url = format!("http://{}", route.local);
        dialer = Some(d);
        (url, route.session)
    };
    let mut o = ConnectOptions::parse(&url).unwrap().token(a.token.clone());
    if let Some(s) = &session {
        o = o.header(SESSION_HEADER, s.clone());
    }
    let c = connect(o).await;
    c.health().await.expect("health");
    let tunnel = if a.tunnel_port > 0 {
        Some(
            c.forward_tcp(cua_spacesd_client::ForwardOptions::new(a.tunnel_port))
                .await
                .expect("tunnel"),
        )
    } else {
        None
    };
    println!(
        "{}",
        serde_json::json!({
            "url": url,
            "tunnel": tunnel.as_ref().map(|t| t.local_addr().to_string()),
            "path": dialer.as_ref().and_then(|d| {
                d.mesh().machine(&a.machine).and_then(|p| d.mesh().path(&p.node_key)).map(|p| p.to_string())
            }),
        })
    );
    for _ in 0..a.serve_secs {
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    if let Some(d) = &dialer {
        println!("{}", serde_json::json!({"status": d.mesh().status()}));
    }
}
