// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A user-space impairment proxy for one path (see `shape`).
//!
//! ```text
//! netshape --listen 127.0.0.1:7302 --target 100.71.227.45:7102 --control 127.0.0.1:7402
//! ```
//!
//! Control (one line per command, one line back):
//! `set <up|down|both> latency=MS jitter=MS bw=BYTES_PER_S loss=0.2 hold=0|1`,
//! `clear`, `reset`, `sever 0|1`, `flap EVERY_MIN_MS EVERY_MAX_MS DOWN_MIN_MS DOWN_MAX_MS`,
//! `stats`.

#[cfg(unix)]
mod imp {
    use cua_replica_lab::shape::{Path, Proxy, Shape};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn arg(args: &[String], name: &str) -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    }

    pub fn parse_shape(words: &[&str], mut s: Shape) -> Shape {
        for w in words {
            if let Some((k, v)) = w.split_once('=') {
                match k {
                    "latency" => s.latency_ms = v.parse().unwrap_or(0),
                    "jitter" => s.jitter_ms = v.parse().unwrap_or(0),
                    "bw" => s.bandwidth = v.parse().unwrap_or(0),
                    "loss" => s.loss = v.parse().unwrap_or(0.0),
                    "hold" => s.hold = v == "1",
                    _ => {}
                }
            }
        }
        s
    }

    #[tokio::main]
    pub async fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let listen = arg(&args, "--listen")
            .expect("--listen")
            .parse()
            .expect("addr");
        let target = arg(&args, "--target")
            .expect("--target")
            .parse()
            .expect("addr");
        let control: std::net::SocketAddr = arg(&args, "--control")
            .expect("--control")
            .parse()
            .expect("addr");
        let proxy = std::sync::Arc::new(Proxy::start(listen, target).await.expect("listen"));
        let flap = std::sync::Mutex::new(None::<tokio::task::JoinHandle<()>>);
        let flap = std::sync::Arc::new(flap);
        eprintln!("netshape {listen} -> {target}, control {control}");
        let l = tokio::net::TcpListener::bind(control)
            .await
            .expect("control");
        loop {
            let Ok((s, _)) = l.accept().await else {
                continue;
            };
            let proxy = proxy.clone();
            let flap = flap.clone();
            tokio::spawn(async move {
                let (rd, mut wr) = s.into_split();
                let mut lines = BufReader::new(rd).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let w: Vec<&str> = line.split_whitespace().collect();
                    let out = match w.as_slice() {
                        ["set", dir, rest @ ..] => {
                            let mut p = proxy.path();
                            match *dir {
                                "up" => p.up = parse_shape(rest, p.up.clone()),
                                "down" => p.down = parse_shape(rest, p.down.clone()),
                                _ => {
                                    p.up = parse_shape(rest, p.up.clone());
                                    p.down = parse_shape(rest, p.down.clone());
                                }
                            }
                            proxy.set(p);
                            "ok".to_string()
                        }
                        ["clear"] => {
                            proxy.set(Path::default());
                            proxy.sever(false);
                            if let Some(h) = flap.lock().expect("flap").take() {
                                h.abort();
                            }
                            "ok".into()
                        }
                        ["reset"] => {
                            proxy.reset_all();
                            "ok".into()
                        }
                        ["sever", v] => {
                            proxy.sever(*v == "1");
                            "ok".into()
                        }
                        ["flap", a, b, c, d] => {
                            let p = |x: &str| x.parse::<u64>().unwrap_or(1000);
                            let h = proxy.flap((p(a), p(b)), (p(c), p(d)));
                            if let Some(old) = flap.lock().expect("flap").replace(h) {
                                old.abort();
                            }
                            "ok".into()
                        }
                        ["stats"] => {
                            let (u, d, c, r) = proxy.stats();
                            format!(
                                "{{\"bytes_up\":{u},\"bytes_down\":{d},\"connections\":{c},\"resets\":{r}}}"
                            )
                        }
                        _ => "error: unknown command".into(),
                    };
                    if wr.write_all(format!("{out}\n").as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
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
