// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Reaches many machines at once over a real relay's mesh, as the Spaces
//! list does after a launch or a daemon restart, and prints how long each
//! one took. With some of the machines switched off, it shows whether
//! they hold up the rest.
//!
//! ```text
//! # a machine (static-token relay), echoing every stream:
//! mesh-fanout serve --relay URL --machine ID --relay-token T --env-token E --dir DIR
//! # a client with a persistent identity, to every machine at once:
//! mesh-fanout fanout --relay URL --machines ID,ID,... --env-token E --identity FILE
//! ```
//!
//! `fanout` prints one JSON line per machine and a summary line.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cua_mesh::dialer::Dialer;
use cua_mesh::{Mesh, MeshConfig, NodeKind};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

fn flag(args: &[String], name: &str) -> String {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| panic!("{name} is required"))
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("serve") => serve(&args).await,
        Some("fanout") => fanout(&args).await,
        _ => {
            eprintln!("usage: mesh-fanout serve|fanout ...");
            std::process::exit(2);
        }
    }
}

async fn serve(args: &[String]) {
    let relay = flag(args, "--relay");
    let machine = flag(args, "--machine");
    let token = flag(args, "--relay-token");
    let env = flag(args, "--env-token");
    let dir = std::path::PathBuf::from(flag(args, "--dir"));
    std::fs::create_dir_all(&dir).unwrap();
    let id = machine.clone();
    let auth: cua_mesh::AuthProvider = Arc::new(move || {
        let (token, id) = (token.clone(), id.clone());
        Box::pin(async move {
            Ok(vec![
                ("authorization".to_owned(), format!("Bearer {token}")),
                (cua_mesh::proto::MACHINE_ID_HEADER.to_owned(), id),
            ])
        })
    });
    let mut cfg = MeshConfig::new(
        relay,
        NodeKind::Machine,
        Some(dir.join("identity.json")),
        auth,
    );
    cfg.name = machine.clone();
    cfg.media_udp_port = 0;
    cfg.approved_path = Some(dir.join("approved.json"));
    let id = machine.clone();
    cfg.pair_verifier = Some(Arc::new(move |client, ts, proof| {
        cua_mesh::proto::verify_pair_proof(
            &env,
            &id,
            client,
            ts,
            proof,
            cua_mesh::proto::now_secs(),
        )
    }));
    let mesh = Mesh::start(cfg).await.expect("start");
    mesh.wait_ready(Duration::from_secs(20))
        .await
        .expect("ready");
    eprintln!("serving {machine}");
    while let Some(incoming) = mesh.accept().await {
        tokio::spawn(async move {
            let (mut r, mut w) = tokio::io::split(incoming.stream);
            let _ = tokio::io::copy(&mut r, &mut w).await;
            let _ = w.shutdown().await;
        });
    }
}

async fn fanout(args: &[String]) {
    let relay = flag(args, "--relay");
    let machines: Vec<String> = flag(args, "--machines")
        .split(',')
        .map(str::to_owned)
        .collect();
    let env = flag(args, "--env-token");
    let identity = std::path::PathBuf::from(flag(args, "--identity"));
    let cap = Duration::from_secs(
        args.iter()
            .position(|a| a == "--cap")
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(90),
    );
    let started = Instant::now();
    let dialer = Arc::new(
        Dialer::start(
            &relay,
            cua_mesh::static_auth(vec![]),
            Some(identity),
            Duration::from_secs(20),
        )
        .await
        .expect("client"),
    );
    let joined = started.elapsed();
    let start = Instant::now();
    let mut tasks = Vec::new();
    for id in machines {
        let (dialer, env) = (dialer.clone(), env.clone());
        tasks.push(tokio::spawn(async move {
            let r = tokio::time::timeout(cap, async {
                let route = dialer
                    .route(&id, Some(&env), Duration::from_secs(3))
                    .await
                    .map_err(|e| e.to_string())?;
                let mut s = tokio::net::TcpStream::connect(route.local)
                    .await
                    .map_err(|e| e.to_string())?;
                s.write_all(b"ping").await.map_err(|e| e.to_string())?;
                let mut got = [0u8; 4];
                s.read_exact(&mut got).await.map_err(|e| e.to_string())?;
                Ok::<_, String>(())
            })
            .await
            .unwrap_or_else(|_| Err(format!("no answer in {cap:?}")));
            let ms = start.elapsed().as_millis();
            let ok = r.is_ok();
            println!(
                "{}",
                serde_json::json!({"machine": id, "ok": ok, "ms": ms, "error": r.err()})
            );
            (ok, ms)
        }));
    }
    let (mut ok_max, mut fail_max, mut ok, mut failed) = (0, 0, 0, 0);
    for t in tasks {
        let (good, ms) = t.await.unwrap();
        if good {
            ok += 1;
            ok_max = ok_max.max(ms);
        } else {
            failed += 1;
            fail_max = fail_max.max(ms);
        }
    }
    println!(
        "{}",
        serde_json::json!({"summary": true, "joined_ms": joined.as_millis(), "answered": ok,
            "answered_max_ms": ok_max, "failed": failed, "failed_max_ms": fail_max})
    );
}
