// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! One machine's replica node and supervisor on real TCP, driven over a
//! line protocol. The same code runs as a library inside the chaos matrix
//! and as the `replica-node` process on each machine of a real test.
//!
//! Control protocol (one command per line, one JSON line back):
//!
//! ```text
//! status                           replication status and the closing paragraph
//! start <agent> <steps> <step_ms>  record a durable run (says where it will run)
//! runs                             every run as this machine sees it
//! put <key> <bytes> [seed]         an unconditional write of deterministic bytes
//! cas <key>                        a create-only write (an ordered write)
//! get <key>                        size and sha256 of the head
//! sum [prefix]                     sha256 over every head under the prefix (convergence check)
//! count [prefix]                   number of heads
//! power sleep|wake|netdown|netup   a power or network event from the OS
//! freeze | thaw                    stop reading and timing (a stopped process, in-process)
//! facts                            this machine's reliability facts
//! quit
//! ```

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cua_volume::backend::{Backend, Condition};
use cua_volume::fs::FsBackend;
use cua_volume::replica::supervision::{Awake, Supervision, SupervisionConfig};
use cua_volume::replica::{
    Config, Facts, Member, PowerEvent, ReplicaNode, ReplicatedBackend, TcpTransport,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use crate::driver::{StepLog, Steps};

/// How a node is set up.
#[derive(Clone, Debug)]
pub struct NodeOpts {
    pub home: PathBuf,
    pub id: String,
    pub name: String,
    pub listen: SocketAddr,
    /// Where to dial each peer (usually an impairment proxy's port).
    pub peers: Vec<(String, SocketAddr)>,
    /// Every machine of the group: id and name (this one included).
    pub members: Vec<(String, String)>,
    /// `None`: measure this machine and keep measuring.
    pub facts: Option<Facts>,
    pub fast: bool,
    pub skew_ms: i64,
    pub control: Option<SocketAddr>,
    pub step_size: usize,
    /// Print JSON events to stdout.
    pub print_events: bool,
}

impl NodeOpts {
    pub fn new(home: &Path, id: &str, name: &str, listen: SocketAddr) -> NodeOpts {
        NodeOpts {
            home: home.to_path_buf(),
            id: id.into(),
            name: name.into(),
            listen,
            peers: vec![],
            members: vec![(id.into(), name.into())],
            facts: None,
            fast: false,
            skew_ms: 0,
            control: None,
            step_size: 1024,
            print_events: false,
        }
    }
}

/// Real facts of this machine, in the Volume's terms.
pub fn measured_facts(store_free: Option<u64>) -> Facts {
    let f = cua_cluster::reliability::probe();
    Facts {
        rank: cua_cluster::reliability::score(&Facts_to_cluster(&f)),
        free_disk: store_free.unwrap_or(f.free_disk_gb << 30),
        on_battery: f.on_battery,
        sleep_prone: f.sleep_prone(),
        user_here: f.user_active,
        sleep_prevented: !cua_cluster::power::active_reasons().is_empty(),
        arch: cua_volume::replica::roles::normalize_arch(std::env::consts::ARCH),
    }
}

#[allow(non_snake_case)]
fn Facts_to_cluster(f: &cua_cluster::reliability::Facts) -> cua_cluster::reliability::Facts {
    let mut f = f.clone();
    f.sleep_prevented = !cua_cluster::power::active_reasons().is_empty();
    f
}

pub struct Node {
    pub opts: NodeOpts,
    pub node: ReplicaNode,
    pub backend: Arc<ReplicatedBackend>,
    pub store: FsBackend,
    pub sup: Supervision,
    pub log: Arc<Mutex<Vec<StepLog>>>,
    pub transport: TcpTransport,
    pub listen: SocketAddr,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
}

fn event(print: bool, v: serde_json::Value) {
    if print {
        println!("{v}");
    }
}

impl Node {
    pub async fn start(mut opts: NodeOpts) -> Result<Arc<Node>, String> {
        let home = opts.home.clone();
        if let Some(h) = std::env::var_os("HOME") {
            let real = PathBuf::from(h).join(".cua");
            if home.starts_with(&real) {
                return Err(format!(
                    "refusing to use {} (the real cua home)",
                    home.display()
                ));
            }
        }
        std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        let store = FsBackend::new(home.join("data"));
        let transport = TcpTransport::new();
        for (id, a) in &opts.peers {
            transport.set(id, *a);
        }
        let facts = match &opts.facts {
            Some(f) => f.clone(),
            None => measured_facts(store.free_bytes()),
        };
        let members: Vec<Member> = opts
            .members
            .iter()
            .map(|(id, name)| Member {
                id: id.clone(),
                name: name.clone(),
                facts: if *id == opts.id {
                    facts.clone()
                } else {
                    Facts::default()
                },
                last_seen_ms: 0,
            })
            .collect();
        let mut cfg = if opts.fast {
            Config::fast()
        } else {
            Config::standard()
        };
        cfg.wall_skew_ms = opts.skew_ms;
        let node = ReplicaNode::open(
            cfg,
            &opts.id,
            &opts.name,
            &home,
            store.clone(),
            Arc::new(transport.clone()),
            members,
        )
        .await
        .map_err(|e| e.to_string())?;
        node.set_facts(facts);
        let listener = TcpListener::bind(opts.listen)
            .await
            .map_err(|e| e.to_string())?;
        let listen = listener.local_addr().map_err(|e| e.to_string())?;
        opts.listen = listen;
        let mut tasks = vec![];
        let n2 = node.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                let Ok((s, _)) = listener.accept().await else {
                    return;
                };
                let _ = s.set_nodelay(true);
                n2.accept(Box::new(s)).await;
            }
        }));
        node.start();
        let backend = node.backend();
        let log: Arc<Mutex<Vec<StepLog>>> = Arc::default();
        let print = opts.print_events;
        let on_step: Arc<dyn Fn(&StepLog) + Send + Sync> = Arc::new(move |e: &StepLog| {
            event(
                print,
                json!({"ev":"step","machine":e.machine,"epoch":e.epoch,"step":e.step,"wall_ms":e.wall_ms}),
            );
        });
        let steps_n = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let _ = steps_n;
        let awake: Awake = Arc::new(move |why: &str| {
            let g = cua_cluster::power::SleepGuard::hold(why);
            event(print, json!({"ev":"awake","reason": why}));
            Box::new(g) as Box<dyn std::any::Any + Send>
        });
        let sup_cfg = if opts.fast {
            SupervisionConfig::fast()
        } else {
            SupervisionConfig::standard()
        };
        // The driver's step count and pace come from the run's spec.
        let driver = Arc::new(SpecDriver {
            log: log.clone(),
            on_step: on_step.clone(),
            size: opts.step_size,
        });
        let sup = Supervision::new(node.clone(), backend.clone(), sup_cfg, driver, Some(awake));
        sup.spawn();
        let me = Arc::new(Node {
            opts: opts.clone(),
            node: node.clone(),
            backend,
            store,
            sup,
            log,
            transport,
            listen,
            tasks: Mutex::new(tasks),
        });
        // Keep this machine's facts current when they are measured.
        if opts.facts.is_none() {
            let m = me.clone();
            let t = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    let f = measured_facts(m.store.free_bytes());
                    m.node.set_facts(f);
                }
            });
            me.tasks.lock().expect("tasks").push(t);
        }
        if let Some(c) = opts.control {
            let l = TcpListener::bind(c).await.map_err(|e| e.to_string())?;
            let m = me.clone();
            let t = tokio::spawn(async move {
                loop {
                    let Ok((s, _)) = l.accept().await else { return };
                    let m = m.clone();
                    tokio::spawn(async move { m.control(s).await });
                }
            });
            me.tasks.lock().expect("tasks").push(t);
        }
        event(
            print,
            json!({"ev":"up","id":opts.id,"name":opts.name,"listen":listen.to_string()}),
        );
        Ok(me)
    }

    pub async fn stop(&self) {
        self.sup.stop().await;
        self.node.shutdown().await;
        for t in self.tasks.lock().expect("tasks").drain(..) {
            t.abort();
        }
    }

    /// Kills the node without a goodbye (a crash): tasks end, nothing is said.
    pub async fn crash(&self) {
        self.sup.stop().await;
        for t in self.tasks.lock().expect("tasks").drain(..) {
            t.abort();
        }
        self.node.freeze(true);
    }

    async fn control(self: Arc<Self>, s: tokio::net::TcpStream) {
        let (rd, mut wr) = s.into_split();
        let mut lines = BufReader::new(rd).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let line = line.trim().to_string();
            if line.is_empty() {
                continue;
            }
            if line == "quit" {
                let _ = wr.write_all(b"{\"ok\":true}\n").await;
                std::process::exit(0);
            }
            let out = self.command(&line).await;
            let mut s = out.to_string();
            s.push('\n');
            if wr.write_all(s.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    pub async fn command(&self, line: &str) -> serde_json::Value {
        let w: Vec<&str> = line.split_whitespace().collect();
        let err = |e: &dyn std::fmt::Display| json!({"ok": false, "error": e.to_string()});
        match w.as_slice() {
            ["status"] => {
                let s = self.node.status().await;
                let words = self.sup.words(None).await;
                json!({"ok": true, "status": s, "words": words})
            }
            ["facts"] => {
                json!({"ok": true, "facts": measured_facts(self.store.free_bytes()), "awake": cua_cluster::power::active_reasons()})
            }
            ["start", agent, steps, ms] => {
                let spec = json!({"steps": steps.parse::<u64>().unwrap_or(10), "step_ms": ms.parse::<u64>().unwrap_or(100)});
                match self.sup.start_run(agent, spec).await {
                    Ok(i) => json!({"ok": true, "run_on": i.run_on, "words": i.words}),
                    Err(e) => err(&e),
                }
            }
            ["runs"] => match self.sup.runs().await {
                Ok(r) => json!({"ok": true, "runs": r}),
                Err(e) => err(&e),
            },
            ["put", key, bytes, rest @ ..] => {
                let n: usize = bytes.parse().unwrap_or(0);
                let seed: u64 = rest.first().and_then(|s| s.parse().ok()).unwrap_or(1);
                let body = crate::driver::content(seed, n);
                let t = std::time::Instant::now();
                match self.backend.put_durable(key, body, Condition::None).await {
                    Ok((m, d)) => {
                        json!({"ok": true, "version": m.version, "etag": m.etag, "durability": d, "ms": t.elapsed().as_millis() as u64})
                    }
                    Err(e) => err(&e),
                }
            }
            ["cas", key] => {
                let t = std::time::Instant::now();
                match self
                    .backend
                    .put_durable(key, b"x".to_vec(), Condition::IfNoneMatch)
                    .await
                {
                    Ok((m, d)) => {
                        json!({"ok": true, "version": m.version, "durability": d, "ms": t.elapsed().as_millis() as u64})
                    }
                    Err(e) => json!({"ok": false, "error": e.to_string(), "kind": e.tag()}),
                }
            }
            ["get", key] => match self.backend.head(key).await {
                Ok(Some(m)) => {
                    json!({"ok": true, "size": m.size, "etag": m.etag, "version": m.version})
                }
                Ok(None) => json!({"ok": true, "missing": true}),
                Err(e) => err(&e),
            },
            ["sum", rest @ ..] => {
                let prefix = rest.first().copied().unwrap_or("");
                match self.store.list(prefix).await {
                    Ok(v) => {
                        let mut all = String::new();
                        for m in &v {
                            all.push_str(&format!("{} {}\n", m.key, m.etag));
                        }
                        json!({"ok": true, "sum": cua_volume::sha256_hex(all.as_bytes()), "count": v.len()})
                    }
                    Err(e) => err(&e),
                }
            }
            ["count", rest @ ..] => {
                let prefix = rest.first().copied().unwrap_or("");
                match self.store.list(prefix).await {
                    Ok(v) => json!({"ok": true, "count": v.len()}),
                    Err(e) => err(&e),
                }
            }
            ["power", what] => {
                let ev = match *what {
                    "sleep" => PowerEvent::WillSleep,
                    "wake" => PowerEvent::DidWake,
                    "netdown" => PowerEvent::NetworkLost,
                    "netup" => PowerEvent::NetworkUp,
                    _ => return err(&"sleep, wake, netdown or netup"),
                };
                self.node.notify_power(ev).await;
                json!({"ok": true})
            }
            ["freeze"] => {
                self.node.freeze(true);
                json!({"ok": true})
            }
            ["thaw"] => {
                self.node.freeze(false);
                json!({"ok": true})
            }
            ["expect", steps, rest @ ..] => {
                let size: usize = rest
                    .first()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(self.opts.step_size);
                json!({"ok": true, "sum": crate::driver::expected_sum(steps.parse().unwrap_or(0), size)})
            }
            ["cat", key] => match self.backend.get(key, None).await {
                Ok((b, m)) => {
                    json!({"ok": true, "text": String::from_utf8_lossy(&b[..b.len().min(4096)]), "size": m.size})
                }
                Err(e) => err(&e),
            },
            ["steps"] => {
                let l = self.log.lock().expect("log");
                json!({"ok": true, "count": l.len(), "machines": l.iter().map(|e| e.machine.clone()).collect::<std::collections::BTreeSet<_>>()})
            }
            _ => err(&format!("unknown command {line:?}")),
        }
    }
}

/// A driver that reads its step count and pace from the run's spec.
struct SpecDriver {
    log: Arc<Mutex<Vec<StepLog>>>,
    on_step: Arc<dyn Fn(&StepLog) + Send + Sync>,
    size: usize,
}

#[async_trait::async_trait]
impl cua_volume::replica::supervision::Driver for SpecDriver {
    async fn drive(
        &self,
        ctx: cua_volume::replica::supervision::RunCtx,
    ) -> cua_volume::replica::supervision::DriveEnd {
        let n = ctx.spec.get("steps").and_then(|v| v.as_u64()).unwrap_or(10);
        let ms = ctx
            .spec
            .get("step_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(100);
        Steps {
            n,
            step_ms: ms,
            size: self.size,
            log: self.log.clone(),
            on_step: Some(self.on_step.clone()),
        }
        .drive(ctx)
        .await
    }
}
