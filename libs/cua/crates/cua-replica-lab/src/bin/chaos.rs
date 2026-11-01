// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! `CUA_CHAOS=1 replica-chaos --list | --all | --scenario NAME [--standard] [--out FILE]`
//!
//! The opt-in chaos matrix for the replicated Volume (see
//! `cua_replica_lab::chaos`). Fast timing (heartbeat 50 ms, scenario time
//! 1/40) unless `--standard`. Exits non-zero if any scenario fails.

#[cfg(unix)]
mod imp {
    use cua_replica_lab::chaos;

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    #[tokio::main]
    pub async fn main() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("off")),
            )
            .try_init();
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.iter().any(|a| a == "--list") {
            for n in chaos::names() {
                println!("{n}");
            }
            return;
        }
        if std::env::var("CUA_CHAOS").ok().as_deref() != Some("1") {
            eprintln!(
                "the chaos matrix is opt-in (it is slow and uses real sockets on loopback): set CUA_CHAOS=1"
            );
            return;
        }
        let fast = !args.iter().any(|a| a == "--standard");
        let names: Vec<String> = match arg(&args, "--scenario") {
            Some(n) => n.split(',').map(str::to_string).collect(),
            None if args.iter().any(|a| a == "--all") => {
                chaos::names().into_iter().map(str::to_string).collect()
            }
            None => {
                eprintln!(
                    "usage: CUA_CHAOS=1 replica-chaos --list | --all | --scenario NAME[,NAME] [--standard] [--out FILE]"
                );
                std::process::exit(2);
            }
        };
        let mut all = vec![];
        let mut failed = 0;
        for n in names {
            eprintln!("== {n}");
            let t = std::time::Instant::now();
            let Some(o) = chaos::run(&n, fast).await else {
                eprintln!("unknown scenario {n}");
                std::process::exit(2);
            };
            let mut j = o.json();
            j["seconds"] = t.elapsed().as_secs_f64().into();
            j["timing"] = if fast { "fast" } else { "standard" }.into();
            println!("{j}");
            if !o.pass {
                failed += 1;
                eprintln!("FAILED: {}", o.problems.join("; "));
            }
            all.push(j);
        }
        if let Some(out) = arg(&args, "--out") {
            let _ = std::fs::write(out, serde_json::to_vec_pretty(&all).unwrap_or_default());
        }
        if failed > 0 {
            std::process::exit(1);
        }
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
