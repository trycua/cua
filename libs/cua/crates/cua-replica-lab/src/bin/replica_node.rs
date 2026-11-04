// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One machine of a real two-machine test.
//!
//! ```text
//! replica-node --home DIR --id studio --name "Mac Studio" --listen 0.0.0.0:7101 \
//!   --member studio="Mac Studio" --member laptop="MacBook" \
//!   --peer laptop=127.0.0.1:7302 --control 127.0.0.1:7001 [--fast] [--rank N --sleep-prone]
//! ```
//!
//! Facts are measured on this machine unless `--rank` is given. Refuses any
//! home under the real `~/.cua`.

#[cfg(unix)]
mod imp {
    use cua_replica_lab::node::{Node, NodeOpts};
    use cua_volume::replica::Facts;

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    fn all(args: &[String], name: &str) -> Vec<String> {
        args.iter()
            .enumerate()
            .filter(|(_, a)| *a == name)
            .filter_map(|(i, _)| args.get(i + 1).cloned())
            .collect()
    }

    #[tokio::main]
    pub async fn main() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("cua_volume=info")),
            )
            .with_writer(std::io::stderr)
            .try_init();
        let args: Vec<String> = std::env::args().skip(1).collect();
        let home = arg(&args, "--home").expect("--home");
        let id = arg(&args, "--id").expect("--id");
        let name = arg(&args, "--name").unwrap_or_else(|| id.clone());
        let listen = arg(&args, "--listen")
            .expect("--listen")
            .parse()
            .expect("listen addr");
        let mut o = NodeOpts::new(std::path::Path::new(&home), &id, &name, listen);
        o.members = all(&args, "--member")
            .into_iter()
            .map(|m| {
                let (i, n) = m.split_once('=').expect("--member id=Name");
                (i.to_string(), n.to_string())
            })
            .collect();
        if !o.members.iter().any(|(i, _)| *i == id) {
            o.members.push((id.clone(), name.clone()));
        }
        o.peers = all(&args, "--peer")
            .into_iter()
            .map(|p| {
                let (i, a) = p.split_once('=').expect("--peer id=addr");
                (i.to_string(), a.parse().expect("peer addr"))
            })
            .collect();
        o.control = arg(&args, "--control").map(|c| c.parse().expect("control addr"));
        o.fast = args.iter().any(|a| a == "--fast");
        o.skew_ms = arg(&args, "--skew-ms")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        o.step_size = arg(&args, "--step-size")
            .and_then(|s| s.parse().ok())
            .unwrap_or(1024);
        o.print_events = true;
        if let Some(r) = arg(&args, "--rank") {
            o.facts = Some(Facts {
                rank: r.parse().expect("rank"),
                free_disk: 500 << 30,
                sleep_prone: args.iter().any(|a| a == "--sleep-prone"),
                ..Default::default()
            });
        }
        let node = Node::start(o).await.expect("start");
        eprintln!("replica-node {id} listening on {}", node.listen);
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        node.stop().await;
    }
}

#[cfg(unix)]
fn main() {
    imp::main()
}

#[cfg(not(unix))]
fn main() {
    eprintln!("cua-replica-lab runs on Unix only (it signals processes and raises file limits)");
}
