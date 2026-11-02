// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The chaos matrix: real nodes over real TCP, every pair of machines joined
//! by an impairment proxy ([`crate::shape`]), a durable agent running, and a
//! ledger of every write that was acknowledged. After each scenario the
//! network is made good again and the same questions are asked:
//!
//! - **No loss, no corruption.** Every acknowledged write is on every
//!   machine with exactly the bytes written; the agent's result checksum is
//!   right and every step file is right.
//! - **No split brain.** One owner and one era everywhere; the run is driven
//!   on one machine only; fenced writes were refused, never applied.
//! - **No false takeover.** A scenario that only makes the network bad (the
//!   owner is up) must not move ownership.
//! - **Bounded.** Takeover and sync lag are measured and held to a bound.
//! - **Incremental.** Bytes on the wire are counted by the proxies and held
//!   against the size of what had to move.
//! - **Truthful.** The words the user sees match the situation.
//! - **The laptop never blocks the studio.** Writes on the studio are timed
//!   while the laptop is impaired.
//!
//! Opt-in (it is slow and uses real sockets): `CUA_CHAOS=1 replica-chaos
//! --all`. It listens on loopback only, writes under a temporary directory it
//! removes, and touches no network configuration.

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_volume::backend::{Backend, Condition};
use cua_volume::replica::Facts;
use serde_json::{Value, json};

use crate::driver::{content, expected_sum};
use crate::node::{Node, NodeOpts};
use crate::shape::{Path, Proxy, Shape};

/// One machine of a scenario.
#[derive(Clone, Debug)]
pub struct Spec {
    pub id: String,
    pub name: String,
    pub rank: u64,
    pub laptop: bool,
}

pub fn spec(id: &str, rank: u64, laptop: bool) -> Spec {
    Spec {
        id: id.into(),
        name: id.into(),
        rank,
        laptop,
    }
}

/// The ledger of acknowledged writes: key to sha256 of the bytes.
#[derive(Default)]
pub struct Ledger(Mutex<BTreeMap<String, String>>);

impl Ledger {
    pub fn record(&self, key: &str, bytes: &[u8]) {
        self.0
            .lock()
            .expect("ledger")
            .insert(key.to_string(), cua_volume::sha256_hex(bytes));
    }

    pub fn len(&self) -> usize {
        self.0.lock().expect("ledger").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn all(&self) -> BTreeMap<String, String> {
        self.0.lock().expect("ledger").clone()
    }
}

pub struct Cluster {
    pub specs: Vec<Spec>,
    pub nodes: Vec<Arc<Node>>,
    pub proxies: BTreeMap<(usize, usize), Proxy>,
    pub ports: Vec<u16>,
    pub fast: bool,
    pub dir: tempfile::TempDir,
    opts: Vec<NodeOpts>,
    pub ledger: Arc<Ledger>,
}

fn free_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port");
    l.local_addr().expect("addr").port()
}

fn facts_of(s: &Spec) -> Facts {
    Facts {
        rank: s.rank,
        free_disk: 500 << 30,
        sleep_prone: s.laptop,
        on_battery: s.laptop,
        ..Default::default()
    }
}

impl Cluster {
    /// Starts every machine and the proxies between them.
    pub async fn start(specs: &[Spec], fast: bool) -> Result<Cluster, String> {
        Cluster::start_inner(specs, fast, &[]).await
    }

    async fn start_inner(specs: &[Spec], fast: bool, skew_ms: &[i64]) -> Result<Cluster, String> {
        let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
        let ports: Vec<u16> = specs.iter().map(|_| free_port()).collect();
        let mut proxies = BTreeMap::new();
        for i in 0..specs.len() {
            #[allow(clippy::needless_range_loop)]
            for j in 0..specs.len() {
                if i != j {
                    let p = Proxy::start(
                        "127.0.0.1:0".parse().expect("addr"),
                        SocketAddr::from(([127, 0, 0, 1], ports[j])),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                    proxies.insert((i, j), p);
                }
            }
        }
        let members: Vec<(String, String)> = specs
            .iter()
            .map(|s| (s.id.clone(), s.name.clone()))
            .collect();
        let mut opts = vec![];
        let mut nodes = vec![];
        for (i, s) in specs.iter().enumerate() {
            let mut o = NodeOpts::new(
                &dir.path().join(&s.id),
                &s.id,
                &s.name,
                SocketAddr::from(([127, 0, 0, 1], ports[i])),
            );
            o.members = members.clone();
            o.skew_ms = skew_ms.get(i).copied().unwrap_or(0);
            o.fast = fast;
            o.facts = Some(facts_of(s));
            o.peers = specs
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(j, p)| (p.id.clone(), proxies[&(i, j)].listen))
                .collect();
            nodes.push(Node::start(o.clone()).await?);
            opts.push(o);
        }
        Ok(Cluster {
            specs: specs.to_vec(),
            nodes,
            proxies,
            ports,
            fast,
            dir,
            opts,
            ledger: Arc::new(Ledger::default()),
        })
    }

    /// A duration in the scenario's time: real seconds on standard timing,
    /// 1/40 of them on fast timing.
    pub fn t(&self, secs: f64) -> Duration {
        Duration::from_secs_f64(if self.fast { secs / 40.0 } else { secs })
    }

    pub fn idx(&self, id: &str) -> usize {
        self.specs
            .iter()
            .position(|s| s.id == id)
            .expect("a machine")
    }

    pub fn node(&self, id: &str) -> &Arc<Node> {
        &self.nodes[self.idx(id)]
    }

    /// How much faster than the real world this cluster's time runs.
    pub fn scale(&self) -> u64 {
        if self.fast { 40 } else { 1 }
    }

    /// A real-world impairment (a second of latency, 100 kbit/s) in this
    /// cluster's time: on fast timing latencies shrink and bandwidths grow
    /// by the same factor, so the same fraction of a heartbeat is lost.
    pub fn real(&self, s: Shape) -> Shape {
        let k = self.scale();
        Shape {
            latency_ms: s.latency_ms / k,
            jitter_ms: s.jitter_ms / k,
            bandwidth: s.bandwidth * k,
            loss: s.loss,
            hold: s.hold,
            rto_ms: if s.rto_ms == 0 { 200 / k } else { s.rto_ms / k }.max(1),
        }
    }

    /// Both directions between `a` and `b` (the numbers are real-world
    /// ones, see [`Cluster::real`]).
    pub fn set_path(&self, a: &str, b: &str, p: Path) {
        let p = Path {
            up: self.real(p.up),
            down: self.real(p.down),
        };
        let (i, j) = (self.idx(a), self.idx(b));
        self.proxies[&(i, j)].set(p.clone());
        self.proxies[&(j, i)].set(Path {
            up: p.down,
            down: p.up,
        });
    }

    pub fn clear_path(&self, a: &str, b: &str) {
        let (i, j) = (self.idx(a), self.idx(b));
        for k in [(i, j), (j, i)] {
            self.proxies[&k].set(Path::default());
            self.proxies[&k].sever(false);
        }
    }

    pub fn sever(&self, a: &str, b: &str, on: bool) {
        let (i, j) = (self.idx(a), self.idx(b));
        self.proxies[&(i, j)].sever(on);
        self.proxies[&(j, i)].sever(on);
    }

    pub fn reset(&self, a: &str, b: &str) {
        let (i, j) = (self.idx(a), self.idx(b));
        self.proxies[&(i, j)].reset_all();
        self.proxies[&(j, i)].reset_all();
    }

    /// Bytes through every proxy so far (both directions, all pairs).
    pub fn wire_bytes(&self) -> u64 {
        self.proxies
            .values()
            .map(|p| {
                let (u, d, _, _) = p.stats();
                u + d
            })
            .sum()
    }

    /// Bytes from `a`'s side toward `b`'s side and back (one pair).
    pub fn pair_bytes(&self, a: &str, b: &str) -> u64 {
        let (i, j) = (self.idx(a), self.idx(b));
        [(i, j), (j, i)]
            .iter()
            .map(|k| {
                let (u, d, _, _) = self.proxies[k].stats();
                u + d
            })
            .sum()
    }

    /// A node process dies (no goodbye) and starts again on its home.
    pub async fn crash_and_restart(&mut self, id: &str, down_for: Duration) -> Result<(), String> {
        let i = self.idx(id);
        self.nodes[i].crash().await;
        tokio::time::sleep(down_for).await;
        self.nodes[i] = Node::start(self.opts[i].clone()).await?;
        Ok(())
    }

    /// A graceful stop (the app quits) and start.
    pub async fn restart(&mut self, id: &str) -> Result<(), String> {
        let i = self.idx(id);
        self.nodes[i].stop().await;
        self.nodes[i] = Node::start(self.opts[i].clone()).await?;
        Ok(())
    }

    pub async fn stop(&self) {
        for n in &self.nodes {
            n.stop().await;
        }
        for p in self.proxies.values() {
            p.stop();
        }
    }

    /// A write of unique content, recorded in the ledger when acknowledged.
    pub async fn write(&self, id: &str, key: &str, size: usize, seed: u64) -> Result<(), String> {
        let body = content(seed, size);
        let n = self.node(id);
        match tokio::time::timeout(
            Duration::from_secs(120),
            n.backend.put(key, body.clone(), Condition::None),
        )
        .await
        {
            Ok(Ok(_)) => {
                self.ledger.record(key, &body);
                Ok(())
            }
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err("timed out".into()),
        }
    }

    /// Waits until every machine has the same heads, or `within` passes.
    pub async fn converge(&self, within: Duration) -> bool {
        let end = Instant::now() + within;
        loop {
            let mut sums = vec![];
            let mut holders = 0;
            let mut pending = 0;
            for n in &self.nodes {
                let st = n.node.status().await;
                pending += st.pending.len();
                // A machine that keeps no copy (a client) asks the others.
                if st.role == cua_volume::replica::Role::Replica {
                    holders += 1;
                    if let Some(s) = n.command("sum").await.get("sum").cloned() {
                        sums.push(s);
                    }
                }
            }
            if pending == 0 && sums.len() == holders && sums.windows(2).all(|w| w[0] == w[1]) {
                return true;
            }
            if Instant::now() > end {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// What every machine thinks, for a failure's report.
    pub async fn dump(&self) -> String {
        let mut out = String::new();
        for n in &self.nodes {
            let s = n.node.status().await;
            out.push_str(&format!(
                "{}: owner={} epoch={} i_own={} applied={} tail={} pending={} peers={:?} fenced={} takeovers={} copies={}\n",
                n.opts.id, s.owner, s.epoch, s.i_own, s.applied, s.tail, s.pending.len(),
                s.peers.iter().map(|p| (p.id.clone(), p.health.clone(), p.connected, p.lag)).collect::<Vec<_>>(),
                s.fenced_writes, s.takeovers, s.copies_now
            ));
            for p in s.pending.iter().take(3) {
                out.push_str(&format!(
                    "    pending {} ({} bytes{})\n",
                    p.key,
                    p.size,
                    if p.deleted { ", delete" } else { "" }
                ));
            }
        }
        // Where the machines' heads differ.
        let mut all: BTreeMap<String, Vec<Option<String>>> = BTreeMap::new();
        for (i, n) in self.nodes.iter().enumerate() {
            for m in n.store.list("").await.unwrap_or_default() {
                all.entry(m.key)
                    .or_insert_with(|| vec![None; self.nodes.len()])[i] =
                    Some(m.version.chars().take(8).collect());
            }
        }
        let mut shown = 0;
        for (k, v) in &all {
            if v.windows(2).any(|w| w[0] != w[1]) && shown < 3 {
                out.push_str(&format!("    differs {k}: {v:?}\n"));
                for n in &self.nodes {
                    out.push_str(&format!(
                        "        {}: head={:?}{}\n",
                        n.opts.id,
                        n.store.head(k).await.ok().flatten().map(|h| h.version),
                        n.node.debug_key(k).await
                    ));
                }
                shown += 1;
            }
        }
        out
    }

    /// The invariants. Returns what is wrong, empty when nothing is.
    pub async fn verify(&self, runs: &[(&str, u64, usize)]) -> Vec<String> {
        let mut bad = vec![];
        // Every acknowledged write is on every machine, byte for byte.
        let ledger = self.ledger.all();
        for n in &self.nodes {
            for (key, sha) in &ledger {
                // Machines without a copy read through the others.
                let got = if n.node.status().await.role == cua_volume::replica::Role::Replica {
                    n.store.get(key, None).await
                } else {
                    n.backend.get(key, None).await
                };
                match got {
                    Ok((b, _)) if cua_volume::sha256_hex(&b) == *sha => {}
                    Ok(_) => bad.push(format!(
                        "{}: {key} has other bytes than were acknowledged",
                        n.opts.id
                    )),
                    Err(e) => bad.push(format!(
                        "{}: acknowledged {key} is missing ({e})",
                        n.opts.id
                    )),
                }
            }
        }
        // The agent: its result and every step.
        for (agent, steps, size) in runs {
            for n in self.nodes.iter().filter(|n| n.opts.facts.is_some()) {
                match n
                    .backend
                    .get(&format!("agents/{agent}/result.txt"), None)
                    .await
                {
                    Ok((b, _))
                        if String::from_utf8_lossy(&b).trim() == expected_sum(*steps, *size) => {}
                    Ok(_) => bad.push(format!("{}: the result of {agent} is wrong", n.opts.id)),
                    Err(_) => bad.push(format!("{}: no result for {agent}", n.opts.id)),
                }
                for i in 0..*steps {
                    match n
                        .backend
                        .get(&format!("agents/{agent}/out/step-{i}"), None)
                        .await
                    {
                        Ok((b, _)) if b == content(i, *size) => {}
                        _ => {
                            bad.push(format!(
                                "{}: step {i} of {agent} is wrong or missing",
                                n.opts.id
                            ));
                            break;
                        }
                    }
                }
            }
        }
        // One owner, one era.
        let mut owners = std::collections::BTreeSet::new();
        let mut epochs = std::collections::BTreeSet::new();
        let mut i_own = 0;
        for n in &self.nodes {
            let s = n.node.status().await;
            owners.insert(s.owner.clone());
            epochs.insert(s.epoch);
            if s.i_own {
                i_own += 1;
            }
        }
        if owners.len() != 1 || i_own != 1 {
            bad.push(format!(
                "split brain: owners {owners:?}, {i_own} machines think they own it"
            ));
        }
        if epochs.len() != 1 {
            bad.push(format!("eras differ: {epochs:?}"));
        }
        // One driver per run.
        for (agent, _, _) in runs {
            let mut here = 0;
            for n in &self.nodes {
                if let Ok(r) = n.sup.runs().await
                    && r.iter()
                        .any(|x| x.agent == *agent && x.running_here && x.state != "done")
                {
                    here += 1;
                }
            }
            if here > 1 {
                bad.push(format!("{agent} is being driven on {here} machines"));
            }
        }
        bad
    }

    pub async fn takeovers(&self) -> u64 {
        let mut t = 0;
        for n in &self.nodes {
            t += n.node.status().await.takeovers;
        }
        t
    }
}

/// What a scenario reports.
#[derive(Debug)]
pub struct Outcome {
    pub name: String,
    pub pass: bool,
    pub problems: Vec<String>,
    pub metrics: Value,
}

impl Outcome {
    fn new(name: &str) -> Outcome {
        Outcome {
            name: name.into(),
            pass: true,
            problems: vec![],
            metrics: json!({}),
        }
    }

    fn check(&mut self, ok: bool, what: impl Into<String>) {
        if !ok {
            self.pass = false;
            self.problems.push(what.into());
        }
    }

    fn metric(&mut self, k: &str, v: impl Into<Value>) {
        self.metrics[k] = v.into();
    }

    pub fn json(&self) -> Value {
        json!({"scenario": self.name, "pass": self.pass, "problems": self.problems, "metrics": self.metrics})
    }
}

fn laptop_studio() -> Vec<Spec> {
    vec![spec("studio", 9000, false), spec("laptop", 2000, true)]
}

async fn ready(c: &Cluster) -> bool {
    let end = Instant::now() + Duration::from_secs(60);
    while Instant::now() < end {
        let mut ok = true;
        for n in &c.nodes {
            let s = n.node.status().await;
            if s.owner != "studio" || s.peers.iter().any(|p| !p.connected || p.health != "up") {
                ok = false;
            }
        }
        if ok {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A durable run on the studio plus writes from both machines through an
/// impairment of the laptop-studio path, then the usual questions.
async fn impaired_run(
    name: &str,
    fast: bool,
    impair: impl Fn(&Cluster) + Send + Sync,
    expect_no_takeover: bool,
) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let step_ms = if fast { 25 } else { 250 };
    // Ten minutes of scenario time under the impairment (90 s on standard timing).
    let window = if fast {
        c.t(600.0)
    } else {
        Duration::from_secs(90)
    };
    let steps = ((window.as_millis() as u64 * 6 / 5) / step_ms).max(60);
    let size = 1024usize;
    c.node("laptop")
        .command(&format!("start ada {steps} {step_ms}"))
        .await;
    // The impairment on, with writes from both machines, a studio write timed.
    impair(&c);
    let mut studio_write_ms = vec![];
    let t_end = Instant::now() + window;
    let mut seq = 0u64;
    while Instant::now() < t_end {
        seq += 1;
        let t = Instant::now();
        let _ = c
            .write("studio", &format!("agents/studio-w/f{seq}"), 3000, seq)
            .await;
        studio_write_ms.push(t.elapsed().as_millis() as u64);
        let _ = c
            .write(
                "laptop",
                &format!("agents/laptop-w/f{seq}"),
                3000,
                10_000 + seq,
            )
            .await;
        tokio::time::sleep(c.t(1.0).max(Duration::from_millis(20))).await;
    }
    // The network is good again.
    for a in 0..c.specs.len() {
        for b in (a + 1)..c.specs.len() {
            let (x, y) = (c.specs[a].id.clone(), c.specs[b].id.clone());
            c.clear_path(&x, &y);
        }
    }
    let t_heal = Instant::now();
    let done = {
        let end = Instant::now() + Duration::from_secs(if fast { 120 } else { 600 });
        let exp = expected_sum(steps, size);
        let mut ok = false;
        while Instant::now() < end {
            if let Some(v) = c
                .node("studio")
                .command("cat agents/ada/result.txt")
                .await
                .get("text")
                && v.as_str().unwrap_or("").trim() == exp
            {
                ok = true;
                break;
            }
            if let Some(v) = c
                .node("laptop")
                .command("cat agents/ada/result.txt")
                .await
                .get("text")
                && v.as_str().unwrap_or("").trim() == exp
            {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        ok
    };
    o.check(done, "the run did not finish with the right result");
    o.metric("run_finished_after_heal_s", t_heal.elapsed().as_secs_f64());
    let t_conv = Instant::now();
    let conv = c
        .converge(Duration::from_secs(if fast { 120 } else { 600 }))
        .await;
    o.metric("converged_after_run_s", t_conv.elapsed().as_secs_f64());
    o.check(conv, "the machines did not converge");
    if !conv {
        eprintln!("{}", c.dump().await);
    }
    for p in c.verify(&[("ada", steps, size)]).await {
        o.check(false, p);
    }
    if expect_no_takeover {
        o.check(
            c.takeovers().await == 0,
            "ownership moved although the owner was up (false takeover)",
        );
    }
    studio_write_ms.sort();
    let med = studio_write_ms
        .get(studio_write_ms.len() / 2)
        .copied()
        .unwrap_or(0);
    o.metric("studio_write_median_ms", med);
    o.metric(
        "studio_write_max_ms",
        studio_write_ms.last().copied().unwrap_or(0),
    );
    o.metric("acknowledged_writes", c.ledger.len());
    o.metric("takeovers", c.takeovers().await);
    o.metric("wire_bytes", c.wire_bytes());
    c.stop().await;
    o
}

fn shape(f: impl Fn(&mut Shape)) -> Path {
    let mut s = Shape::default();
    f(&mut s);
    Path::both(s)
}

/// The scenarios, by name.
pub fn names() -> Vec<&'static str> {
    vec![
        "hotel_wifi_measured",
        "latency_600ms",
        "latency_1500ms_jitter_800ms",
        "loss_10",
        "loss_20",
        "loss_40",
        "bandwidth_500kbit",
        "bandwidth_100kbit",
        "asymmetric_loss_40",
        "flapping_20_to_60s",
        "partition_5min",
        "partition_30min",
        "nat_change_midsession",
        "dns_failure",
        "clock_skew_one_day",
        "laptop_sigstop_and_wake",
        "studio_reboot_during_run",
        "studio_kill_laptop_takes_run",
        "wake_after_takeover",
        "big_file_flapping_link",
        "scale_peers_5",
        "scale_peers_20",
        "scale_many_small_files",
        "scale_ten_runs",
        "false_takeover_rate_under_jitter",
    ]
}

pub async fn run(name: &str, fast: bool) -> Option<Outcome> {
    raise_open_files();
    Some(match name {
        // Measured from the owner's hotel wifi (read-only: 100 pings each to two
        // public resolvers and the gateway, two 10 MB downloads, one 3 MB
        // upload): RTT 8 ms average, 5 to 20 ms, 0 percent loss, about 2.9 MB/s
        // down and 3.9 MB/s up.
        "hotel_wifi_measured" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 4;
                            s.jitter_ms = 8;
                            s.bandwidth = 2_900_000;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "latency_600ms" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 600;
                            s.jitter_ms = 100;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "latency_1500ms_jitter_800ms" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 1500;
                            s.jitter_ms = 800;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "loss_10" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 100;
                            s.loss = 0.10;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "loss_20" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 100;
                            s.loss = 0.20;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "loss_40" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 150;
                            s.loss = 0.40;
                        }),
                    )
                },
                false,
            )
            .await
        }
        "bandwidth_500kbit" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 200;
                            s.bandwidth = 62_500;
                        }),
                    )
                },
                true,
            )
            .await
        }
        "bandwidth_100kbit" => {
            impaired_run(
                name,
                fast,
                |c| {
                    c.set_path(
                        "studio",
                        "laptop",
                        shape(|s| {
                            s.latency_ms = 300;
                            s.bandwidth = 12_500;
                        }),
                    )
                },
                false,
            )
            .await
        }
        "asymmetric_loss_40" => {
            impaired_run(
                name,
                fast,
                |c| {
                    let mut p = Path::default();
                    p.up.loss = 0.4;
                    p.up.latency_ms = 150;
                    c.set_path("studio", "laptop", p);
                },
                false,
            )
            .await
        }
        "flapping_20_to_60s" => {
            impaired_run(
                name,
                fast,
                |c| {
                    let s = if c.fast { 40.0 } else { 1.0 };
                    let every = ((20_000.0 / s) as u64, (60_000.0 / s) as u64);
                    let down = ((2_000.0 / s) as u64, (6_000.0 / s) as u64);
                    for k in [("studio", "laptop"), ("laptop", "studio")] {
                        drop(c.proxies[&(c.idx(k.0), c.idx(k.1))].flap(every, down));
                    }
                },
                !fast,
            )
            .await
        }
        "partition_5min" => partition(name, fast, 300.0).await,
        "partition_30min" => partition(name, fast, 1800.0).await,
        "nat_change_midsession" => {
            impaired_run(
                name,
                fast,
                |c| {
                    // Resets and a few seconds of refusals, three times.
                    let sev = c.proxies[&(c.idx("laptop"), c.idx("studio"))].flap(
                        (c.t(10.0).as_millis() as u64, c.t(15.0).as_millis() as u64),
                        (c.t(1.0).as_millis() as u64, c.t(3.0).as_millis() as u64),
                    );
                    drop(sev);
                },
                !fast,
            )
            .await
        }
        "dns_failure" => dns_failure(name, fast).await,
        "clock_skew_one_day" => clock_skew(name, fast).await,
        "laptop_sigstop_and_wake" => laptop_sigstop(name, fast).await,
        "studio_reboot_during_run" => studio_reboot(name, fast).await,
        "studio_kill_laptop_takes_run" => studio_kill(name, fast).await,
        "wake_after_takeover" => wake_after_takeover(name, fast).await,
        "big_file_flapping_link" => big_file_flapping(name, fast).await,
        "scale_peers_5" => scale_peers(name, fast, 5).await,
        "scale_peers_20" => scale_peers(name, fast, 20).await,
        "scale_many_small_files" => scale_small_files(name, fast).await,
        "scale_ten_runs" => scale_ten_runs(name, fast).await,
        "false_takeover_rate_under_jitter" => false_takeover_rate(name, fast).await,
        _ => return None,
    })
}

async fn partition(name: &str, fast: bool, minutes_secs: f64) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let (steps, step_ms, size) = (150u64, if fast { 25 } else { 250 }, 1024usize);
    c.node("laptop")
        .command(&format!("start ada {steps} {step_ms}"))
        .await;
    tokio::time::sleep(c.t(5.0)).await;
    // The partition: blackholed (bytes stop), the machines keep writing.
    let hold = shape(|s| s.hold = true);
    c.set_path("studio", "laptop", hold);
    let t_start = Instant::now();
    let mut seq = 0u64;
    let mut studio_ms = vec![];
    while t_start.elapsed() < c.t(minutes_secs) {
        seq += 1;
        let t = Instant::now();
        let _ = c
            .write("studio", &format!("agents/studio-w/p{seq}"), 2000, seq)
            .await;
        studio_ms.push(t.elapsed().as_millis() as u64);
        // The laptop's writes wait in its outbox.
        let _ = c
            .write(
                "laptop",
                &format!("agents/laptop-w/p{seq}"),
                2000,
                5_000 + seq,
            )
            .await;
        tokio::time::sleep(c.t(10.0).max(Duration::from_millis(50))).await;
    }
    // The words each machine shows while apart.
    let studio_words = c.node("studio").command("status").await["words"].clone();
    let laptop_pending = c.node("laptop").node.status().await.pending.len();
    o.metric("laptop_pending_while_apart", laptop_pending);
    o.metric("studio_words_while_apart", studio_words.clone());
    o.check(
        studio_words.as_str().is_some_and(|w| {
            w.contains("1 machine") || w.contains("MacBook") || w.contains("laptop")
        }),
        "the studio's words do not say the files are on one machine",
    );
    c.clear_path("studio", "laptop");
    let t_heal = Instant::now();
    let conv = c
        .converge(Duration::from_secs(if fast { 180 } else { 900 }))
        .await;
    if !conv {
        eprintln!("{}", c.dump().await);
    }
    o.check(conv, "no convergence after the partition");
    o.metric("converged_after_heal_s", t_heal.elapsed().as_secs_f64());
    // The run finishes wherever it is.
    let exp = expected_sum(steps, size);
    let end = Instant::now() + Duration::from_secs(if fast { 120 } else { 600 });
    let mut done = false;
    while Instant::now() < end && !done {
        done = c.node("studio").command("cat agents/ada/result.txt").await["text"]
            .as_str()
            .is_some_and(|t| t.trim() == exp);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(done, "the run did not finish");
    o.check(c.converge(Duration::from_secs(120)).await, "no convergence");
    for p in c.verify(&[("ada", steps, size)]).await {
        o.check(false, p);
    }
    studio_ms.sort();
    o.metric("studio_write_median_ms", studio_ms[studio_ms.len() / 2]);
    o.metric("studio_write_max_ms", *studio_ms.last().unwrap_or(&0));
    o.metric("partition_s", c.t(minutes_secs).as_secs_f64());
    o.metric("takeovers", c.takeovers().await);
    c.stop().await;
    o
}

async fn dns_failure(name: &str, fast: bool) -> Outcome {
    // The machine's address stops resolving: dials fail at once, with no
    // refusal and no timeout; then it resolves again.
    impaired_run(
        name,
        fast,
        |c| {
            let i = c.idx("laptop");
            let j = c.idx("studio");
            let t = c.t(25.0);
            let tp = c.nodes[i].transport.clone();
            let good = c.proxies[&(i, j)].listen;
            tp.set("studio", "127.0.0.1:1".parse().expect("addr"));
            // Existing links die with the failed lookup.
            c.proxies[&(i, j)].reset_all();
            c.proxies[&(j, i)].reset_all();
            tokio::spawn(async move {
                tokio::time::sleep(t).await;
                tp.set("studio", good);
            });
        },
        false,
    )
    .await
}

async fn clock_skew(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let mut specs = laptop_studio();
    specs[1].laptop = true;
    // The laptop's wall clock is a day ahead, the studio's an hour behind.
    let dir = tempfile::tempdir().unwrap();
    let _ = dir;
    let c = match Cluster::start_skewed(&specs, fast, &[0, 86_400_000]).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    // Concurrent edits of one file: the conflict copy's name uses wall time.
    c.set_path("studio", "laptop", shape(|s| s.hold = true));
    let _ = c.write("studio", "agents/ada/plan.md", 100, 1).await;
    let _ = c.write("laptop", "agents/ada/plan.md", 100, 2).await;
    tokio::time::sleep(c.t(4.0)).await;
    c.clear_path("studio", "laptop");
    o.check(
        c.converge(Duration::from_secs(if fast { 60 } else { 300 }))
            .await,
        "no convergence",
    );
    let names = c
        .node("studio")
        .store
        .list("agents/ada/")
        .await
        .unwrap_or_default();
    o.check(
        names.iter().any(|m| m.key.contains("(conflict from")),
        "the conflicting edit was not kept as a copy",
    );
    // (On fast timing a 100 ms hold is the size of the silence floor; the
    // check is for standard timing.)
    o.check(
        fast || c.takeovers().await == 0,
        "a skewed clock caused a takeover",
    );
    // The ledger holds the last acknowledged write per key; both writes were
    // to one key, so only check what is unique (the copy and the head hold both).
    c.ledger.0.lock().unwrap().clear();
    for p in c.verify(&[]).await {
        o.check(false, p);
    }
    c.stop().await;
    o
}

async fn laptop_sigstop(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let (steps, step_ms, size) = (200u64, if fast { 25 } else { 250 }, 1024usize);
    c.node("laptop")
        .command(&format!("start ada {steps} {step_ms}"))
        .await;
    tokio::time::sleep(c.t(8.0)).await;
    // The lid closes (abruptly: no goodbye), for a while.
    c.node("laptop").command("freeze").await;
    c.set_path("studio", "laptop", shape(|s| s.hold = true));
    let t0 = Instant::now();
    let mut detected = None;
    let mut seq = 0;
    let mut ms = vec![];
    while t0.elapsed() < c.t(60.0) {
        seq += 1;
        let t = Instant::now();
        let _ = c
            .write("studio", &format!("agents/studio-w/s{seq}"), 1500, seq)
            .await;
        ms.push(t.elapsed().as_millis() as u64);
        if detected.is_none() && c.node("studio").node.status().await.peers[0].health != "up" {
            detected = Some(t0.elapsed().as_secs_f64());
        }
        tokio::time::sleep(c.t(1.0).max(Duration::from_millis(20))).await;
    }
    o.metric("studio_noticed_after_s", detected.unwrap_or(-1.0));
    ms.sort();
    o.metric("studio_write_median_ms", ms[ms.len() / 2]);
    // It wakes.
    c.clear_path("studio", "laptop");
    c.node("laptop").command("thaw").await;
    c.node("laptop").command("power wake").await;
    let t1 = Instant::now();
    o.check(
        c.converge(Duration::from_secs(if fast { 90 } else { 400 }))
            .await,
        "no convergence after the wake",
    );
    o.metric("synced_after_wake_s", t1.elapsed().as_secs_f64());
    let exp = expected_sum(steps, size);
    let end = Instant::now() + Duration::from_secs(if fast { 90 } else { 400 });
    let mut done = false;
    while Instant::now() < end && !done {
        done = c.node("laptop").command("cat agents/ada/result.txt").await["text"]
            .as_str()
            .is_some_and(|t| t.trim() == exp);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(done, "the run did not finish");
    o.check(
        c.takeovers().await == 0,
        "the volume moved when only the laptop slept",
    );
    for p in c.verify(&[("ada", steps, size)]).await {
        o.check(false, p);
    }
    c.stop().await;
    o
}

fn scaled(secs: f64, fast: bool) -> Duration {
    Duration::from_secs_f64(if fast { secs / 40.0 } else { secs })
}

/// The studio's process is killed (SIGKILL, no goodbye) and started again on
/// its home, as a real reboot does. The nodes are real child processes (an
/// in-process "crash" cannot stop a write that is already in a disk call,
/// which a dead process cannot do), joined by in-process impairment proxies.
async fn studio_reboot(name: &str, fast: bool) -> Outcome {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    let mut o = Outcome::new(name);
    let exe = std::env::current_exe().unwrap_or_default();
    let node_bin = exe.with_file_name("replica-node");
    if !node_bin.is_file() {
        o.check(
            false,
            format!(
                "{} is missing; build it next to replica-chaos",
                node_bin.display()
            ),
        );
        return o;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let (p_s, p_l, c_s, c_l) = (free_port(), free_port(), free_port(), free_port());
    // laptop dials the studio through a proxy; the studio dials the laptop through another.
    let to_studio = Proxy::start(
        "127.0.0.1:0".parse().unwrap(),
        SocketAddr::from(([127, 0, 0, 1], p_s)),
    )
    .await
    .unwrap();
    let to_laptop = Proxy::start(
        "127.0.0.1:0".parse().unwrap(),
        SocketAddr::from(([127, 0, 0, 1], p_l)),
    )
    .await
    .unwrap();
    let spawn = |id: &str,
                 rank: u64,
                 laptop: bool,
                 listen: u16,
                 ctl: u16,
                 peer: &str,
                 peer_at: SocketAddr| {
        let mut cmd = std::process::Command::new(&node_bin);
        cmd.args([
            "--home",
            &dir.path().join(id).display().to_string(),
            "--id",
            id,
            "--name",
            id,
        ])
        .args([
            "--listen",
            &format!("127.0.0.1:{listen}"),
            "--control",
            &format!("127.0.0.1:{ctl}"),
        ])
        .args(["--member", "studio=studio", "--member", "laptop=laptop"])
        .args([
            "--peer",
            &format!("{peer}={peer_at}"),
            "--rank",
            &rank.to_string(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
        if laptop {
            cmd.arg("--sleep-prone");
        }
        if fast {
            cmd.arg("--fast");
        }
        cmd.spawn().expect("spawn a node")
    };
    let mut studio = spawn("studio", 9000, false, p_s, c_s, "laptop", to_laptop.listen);
    let mut laptop = spawn("laptop", 2000, true, p_l, c_l, "studio", to_studio.listen);
    let ctl = |port: u16, line: String| async move {
        for _ in 0..50 {
            if let Ok(s) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
                let (rd, mut wr) = s.into_split();
                if wr.write_all(format!("{line}\n").as_bytes()).await.is_ok() {
                    let mut l = String::new();
                    if tokio::io::BufReader::new(rd)
                        .read_line(&mut l)
                        .await
                        .is_ok()
                        && !l.is_empty()
                    {
                        return serde_json::from_str::<Value>(&l).unwrap_or(Value::Null);
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Value::Null
    };
    let step_ms = if fast { 25 } else { 250 };
    let steps = 200u64;
    // Ready: connected and agreeing on the owner.
    let end = Instant::now() + Duration::from_secs(60);
    loop {
        let a = ctl(c_l, "status".into()).await;
        let ok = a["status"]["owner"] == "studio"
            && a["status"]["peers"][0]["health"] == "up"
            && a["status"]["peers"][0]["connected"] == true;
        if ok {
            break;
        }
        if Instant::now() > end {
            o.check(false, "the machines did not connect");
            let _ = studio.kill();
            let _ = laptop.kill();
            let _ = studio.wait();
            let _ = laptop.wait();
            return o;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    ctl(c_l, format!("start ada {steps} {step_ms}")).await;
    tokio::time::sleep(scaled(10.0, fast)).await;
    let t0 = Instant::now();
    let _ = studio.kill();
    let _ = studio.wait();
    to_studio.reset_all();
    to_laptop.reset_all();
    tokio::time::sleep(scaled(30.0, fast)).await;
    studio = spawn("studio", 9000, false, p_s, c_s, "laptop", to_laptop.listen);
    o.metric("studio_down_s", scaled(30.0, fast).as_secs_f64());
    let exp = expected_sum(steps, 1024);
    let end = Instant::now() + Duration::from_secs(if fast { 180 } else { 900 });
    let mut done = false;
    while Instant::now() < end && !done {
        for port in [c_l, c_s] {
            let r = ctl(port, "cat agents/ada/result.txt".into()).await;
            if r["text"].as_str().is_some_and(|t| t.trim() == exp) {
                done = true;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    o.check(done, "the run did not finish after the studio rebooted");
    o.metric("finished_after_s", t0.elapsed().as_secs_f64());
    // The same heads on both, and the right result and steps on each.
    let end = Instant::now() + Duration::from_secs(if fast { 120 } else { 600 });
    let mut same = false;
    while Instant::now() < end && !same {
        let a = ctl(c_l, "sum".into()).await;
        let b = ctl(c_s, "sum".into()).await;
        same = a["sum"].is_string() && a["sum"] == b["sum"];
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    o.check(same, "no convergence");
    for port in [c_l, c_s] {
        let r = ctl(port, "cat agents/ada/result.txt".into()).await;
        o.check(
            r["text"].as_str().is_some_and(|t| t.trim() == exp),
            "a machine lacks the right result",
        );
    }
    let _ = studio.kill();
    let _ = laptop.kill();
    let _ = studio.wait();
    let _ = laptop.wait();
    o
}

async fn studio_kill(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let (steps, step_ms, size) = (150u64, if fast { 25 } else { 250 }, 1024usize);
    c.node("laptop")
        .command(&format!("start ada {steps} {step_ms}"))
        .await;
    tokio::time::sleep(c.t(8.0)).await;
    let before = c.node("laptop").log.lock().unwrap().len();
    let t0 = Instant::now();
    c.node("studio").crash().await;
    // Its sockets close as a dead process's do.
    for k in [("studio", "laptop"), ("laptop", "studio")] {
        c.proxies[&(c.idx(k.0), c.idx(k.1))].sever(true);
    }
    let mut took = None;
    let end = Instant::now() + Duration::from_secs(if fast { 60 } else { 300 });
    while Instant::now() < end && took.is_none() {
        if c.node("laptop").log.lock().unwrap().len() > before {
            took = Some(t0.elapsed().as_secs_f64());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    o.check(took.is_some(), "the laptop never took the run");
    o.metric("takeover_s", took.unwrap_or(-1.0));
    let exp = expected_sum(steps, size);
    let end = Instant::now() + Duration::from_secs(if fast { 120 } else { 600 });
    let mut done = false;
    while Instant::now() < end && !done {
        done = c.node("laptop").command("cat agents/ada/result.txt").await["text"]
            .as_str()
            .is_some_and(|t| t.trim() == exp);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(done, "the run did not finish on the laptop");
    let runs = c.node("laptop").sup.runs().await.unwrap_or_default();
    o.metric(
        "words",
        runs.first().map(|r| r.note.clone()).unwrap_or_default(),
    );
    o.check(
        runs.first()
            .is_some_and(|r| r.note.starts_with("Resumed on laptop")),
        "the takeover words do not say it resumed",
    );
    c.stop().await;
    o
}

async fn wake_after_takeover(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let (steps, step_ms, size) = (300u64, if fast { 25 } else { 250 }, 1024usize);
    c.node("laptop")
        .command(&format!("start ada {steps} {step_ms}"))
        .await;
    tokio::time::sleep(c.t(8.0)).await;
    // The studio freezes behind an open connection; the laptop takes over.
    c.node("studio").command("freeze").await;
    c.set_path("studio", "laptop", shape(|s| s.hold = true));
    let before = c.node("laptop").log.lock().unwrap().len();
    let t0 = Instant::now();
    let end = Instant::now() + Duration::from_secs(if fast { 60 } else { 300 });
    let mut took = None;
    while Instant::now() < end && took.is_none() {
        if c.node("laptop").log.lock().unwrap().len() > before {
            took = Some(t0.elapsed().as_secs_f64());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    o.check(took.is_some(), "the laptop never took the run");
    o.metric("takeover_s", took.unwrap_or(-1.0));
    tokio::time::sleep(c.t(10.0)).await;
    // The studio wakes.
    c.clear_path("studio", "laptop");
    c.node("studio").command("thaw").await;
    c.node("studio").command("power wake").await;
    let t1 = Instant::now();
    let mut fenced = None;
    let end = Instant::now() + Duration::from_secs(if fast { 60 } else { 300 });
    while Instant::now() < end && fenced.is_none() {
        let runs = c.node("studio").sup.runs().await.unwrap_or_default();
        if runs.iter().any(|r| !r.running_here) {
            fenced = Some(t1.elapsed().as_secs_f64());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    o.check(fenced.is_some(), "the woken studio kept driving the run");
    o.metric("fenced_after_wake_s", fenced.unwrap_or(-1.0));
    let exp = expected_sum(steps, size);
    let end = Instant::now() + Duration::from_secs(if fast { 180 } else { 900 });
    let mut done = false;
    while Instant::now() < end && !done {
        done = c.node("laptop").command("cat agents/ada/result.txt").await["text"]
            .as_str()
            .is_some_and(|t| t.trim() == exp);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(done, "the run did not finish correctly");
    o.check(
        c.converge(Duration::from_secs(if fast { 120 } else { 600 }))
            .await,
        "no convergence",
    );
    for p in c.verify(&[("ada", steps, size)]).await {
        o.check(false, p);
    }
    c.stop().await;
    o
}

async fn big_file_flapping(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let size: usize = std::env::var("CUA_CHAOS_BIG_MB")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(256)
        << 20;
    // 20 MB/s with a link that drops every few seconds.
    c.set_path(
        "studio",
        "laptop",
        shape(|s| {
            s.bandwidth = 20 << 20;
            s.latency_ms = 30;
        }),
    );
    let flap = [
        c.proxies[&(c.idx("laptop"), c.idx("studio"))].flap((2000, 5000), (300, 1500)),
        c.proxies[&(c.idx("studio"), c.idx("laptop"))].flap((2000, 5000), (300, 1500)),
    ];
    let before = c.pair_bytes("studio", "laptop");
    // A big file written in 1 MiB-patterned pieces to a file on the studio's disk, then put.
    let path = c.dir.path().join("big.bin");
    {
        use std::io::Write as _;
        let mut f = std::fs::File::create(&path).unwrap();
        let mut left = size;
        let mut i = 0u64;
        while left > 0 {
            let n = left.min(1 << 20);
            f.write_all(&content(i, n)).unwrap();
            left -= n;
            i += 1;
        }
    }
    let mut want = vec![];
    {
        use sha2::Digest as _;
        use std::io::Read as _;
        let mut h = sha2::Sha256::new();
        let mut f = std::fs::File::open(&path).unwrap();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        want.extend(h.finalize());
    }
    let t0 = Instant::now();
    let put = c
        .node("studio")
        .backend
        .put_file("agents/ada/big.bin", &path, Condition::None)
        .await;
    o.check(put.is_ok(), "the big write failed");
    let end = Instant::now() + Duration::from_secs(if fast { 600 } else { 3600 });
    let mut ok = false;
    while Instant::now() < end && !ok {
        ok = c
            .node("laptop")
            .store
            .head("agents/ada/big.bin")
            .await
            .ok()
            .flatten()
            .is_some_and(|h| h.size == size as u64);
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(ok, "the big file never arrived");
    for f in flap {
        f.abort();
    }
    c.clear_path("studio", "laptop");
    let moved = c.pair_bytes("studio", "laptop") - before;
    o.metric("file_mib", size >> 20);
    o.metric("transfer_s", t0.elapsed().as_secs_f64());
    o.metric("bytes_on_wire_mib", moved >> 20);
    o.metric("overhead_ratio", moved as f64 / size as f64);
    o.check(
        moved < (size as u64) * 3 / 2,
        format!("{moved} bytes crossed for {size}: parts were sent again"),
    );
    // Intact.
    if ok {
        use sha2::Digest as _;
        let p = c
            .node("laptop")
            .store
            .version_file(
                "agents/ada/big.bin",
                &c.node("laptop")
                    .store
                    .head("agents/ada/big.bin")
                    .await
                    .unwrap()
                    .unwrap()
                    .version,
            )
            .unwrap();
        let mut h = sha2::Sha256::new();
        let mut f = std::fs::File::open(p).unwrap();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            use std::io::Read as _;
            let n = f.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        o.check(
            h.finalize().to_vec() == want,
            "the big file differs after the transfer",
        );
    }
    c.stop().await;
    o
}

/// 20 machines joined by proxies need thousands of sockets; macOS starts at 256.
fn raise_open_files() {
    // SAFETY: plain getrlimit/setrlimit on valid structs.
    unsafe {
        let mut r: libc::rlimit = std::mem::zeroed();
        if libc::getrlimit(libc::RLIMIT_NOFILE, &mut r) == 0 {
            r.rlim_cur = r.rlim_max.min(65_536).max(r.rlim_cur);
            let _ = libc::setrlimit(libc::RLIMIT_NOFILE, &r);
        }
    }
}

fn rusage() -> (f64, f64, u64) {
    // SAFETY: an all-zero rusage is a valid out value.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
    (t(u.ru_utime), t(u.ru_stime), u.ru_maxrss as u64)
}

async fn scale_peers(name: &str, fast: bool, n: usize) -> Outcome {
    let mut o = Outcome::new(name);
    let specs: Vec<Spec> = (0..n)
        .map(|i| spec(&format!("m{i:02}"), 9000 - (i as u64) * 100, i != 0))
        .collect();
    let (u0, s0, _) = rusage();
    let t0 = Instant::now();
    let c = match Cluster::start(&specs, fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    // Everyone agrees on the owner (the most reliable) and sees everyone.
    let end = Instant::now() + Duration::from_secs(if fast { 120 } else { 300 });
    let mut agreed = false;
    while Instant::now() < end && !agreed {
        agreed = true;
        for nd in &c.nodes {
            let s = nd.node.status().await;
            if s.owner != "m00" || s.peers.iter().filter(|p| p.connected).count() < n - 1 {
                agreed = false;
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    o.check(agreed, "the group did not agree on one owner and connect");
    o.metric("machines", n);
    o.metric("agreed_after_s", t0.elapsed().as_secs_f64());
    // Writes from random machines; all converge.
    let w0 = Instant::now();
    for i in 0..(n * 20) {
        let who = format!("m{:02}", i % n);
        let _ = c
            .write(&who, &format!("agents/s/f{i}"), 2000, i as u64)
            .await;
    }
    o.metric("writes", n * 20);
    o.metric("write_phase_s", w0.elapsed().as_secs_f64());
    o.check(
        c.converge(Duration::from_secs(if fast { 180 } else { 600 }))
            .await,
        "no convergence",
    );
    // Two copies, not n: only the replicas hold the files, clients ask.
    let mut holders = 0;
    for nd in &c.nodes {
        if nd
            .store
            .list("agents/s/")
            .await
            .map(|v| v.len())
            .unwrap_or(0)
            > 0
        {
            holders += 1;
        }
    }
    o.metric("machines_holding_files", holders);
    for p in c.verify(&[]).await {
        o.check(false, p);
    }
    let (u1, s1, rss) = rusage();
    o.metric("cpu_user_s", u1 - u0);
    o.metric("cpu_sys_s", s1 - s0);
    o.metric(
        "peak_rss_mib",
        rss as f64
            / if cfg!(target_os = "macos") {
                1048576.0
            } else {
                1024.0
            },
    );
    o.metric("wire_mib", c.wire_bytes() >> 20);
    o.metric("takeovers", c.takeovers().await);
    c.stop().await;
    o
}

async fn scale_small_files(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let n: usize = std::env::var("CUA_CHAOS_SMALL_FILES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let (u0, s0, _) = rusage();
    let t0 = Instant::now();
    let studio = c.node("studio").clone();
    // Batches of 200 (what a folder copy-in does).
    let mut i = 0usize;
    while i < n {
        let end = (i + 200).min(n);
        let items: Vec<(String, Vec<u8>)> = (i..end)
            .map(|k| {
                (
                    format!("agents/small/d{}/f{k}", k % 100),
                    content(k as u64, 1500),
                )
            })
            .collect();
        if studio.backend.put_many(items.clone()).await.is_ok() {
            for (k, b) in &items {
                c.ledger.record(k, b);
            }
        }
        i = end;
    }
    o.metric("files", n);
    o.metric("write_s", t0.elapsed().as_secs_f64());
    o.metric("write_files_per_s", n as f64 / t0.elapsed().as_secs_f64());
    let t1 = Instant::now();
    o.check(
        c.converge(Duration::from_secs(if fast { 900 } else { 1800 }))
            .await,
        "no convergence",
    );
    o.metric("replicated_after_writes_s", t1.elapsed().as_secs_f64());
    for p in c.verify(&[]).await {
        o.check(false, p);
        if o.problems.len() > 5 {
            break;
        }
    }
    let (u1, s1, rss) = rusage();
    o.metric("cpu_user_s", u1 - u0);
    o.metric("cpu_sys_s", s1 - s0);
    o.metric(
        "peak_rss_mib",
        rss as f64
            / if cfg!(target_os = "macos") {
                1048576.0
            } else {
                1024.0
            },
    );
    o.metric("wire_mib", c.wire_bytes() >> 20);
    c.stop().await;
    o
}

async fn scale_ten_runs(name: &str, fast: bool) -> Outcome {
    let mut o = Outcome::new(name);
    let c = match Cluster::start(&laptop_studio(), fast).await {
        Ok(c) => c,
        Err(e) => {
            o.check(false, e);
            return o;
        }
    };
    o.check(ready(&c).await, "the machines did not connect");
    let (steps, step_ms, size) = (60u64, if fast { 20 } else { 100 }, 1024usize);
    let (u0, s0, _) = rusage();
    let t0 = Instant::now();
    let names: Vec<String> = (0..10).map(|i| format!("run{i}")).collect();
    for n in &names {
        c.node("laptop")
            .command(&format!("start {n} {steps} {step_ms}"))
            .await;
    }
    let exp = expected_sum(steps, size);
    let end = Instant::now() + Duration::from_secs(if fast { 300 } else { 900 });
    let mut done = 0;
    while Instant::now() < end {
        done = 0;
        for n in &names {
            if c.node("laptop")
                .command(&format!("cat agents/{n}/result.txt"))
                .await["text"]
                .as_str()
                .is_some_and(|t| t.trim() == exp)
            {
                done += 1;
            }
        }
        if done == names.len() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    o.check(
        done == names.len(),
        format!("only {done} of 10 runs finished"),
    );
    o.metric("ten_runs_finished_s", t0.elapsed().as_secs_f64());
    o.metric("ideal_s", (steps * step_ms) as f64 / 1000.0);
    o.check(c.converge(Duration::from_secs(120)).await, "no convergence");
    let runs: Vec<(&str, u64, usize)> = names.iter().map(|n| (n.as_str(), steps, size)).collect();
    for p in c.verify(&runs).await {
        o.check(false, p);
    }
    let (u1, s1, rss) = rusage();
    o.metric("cpu_user_s", u1 - u0);
    o.metric("cpu_sys_s", s1 - s0);
    o.metric(
        "peak_rss_mib",
        rss as f64
            / if cfg!(target_os = "macos") {
                1048576.0
            } else {
                1024.0
            },
    );
    o.metric("wire_mib", c.wire_bytes() >> 20);
    c.stop().await;
    o
}

async fn false_takeover_rate(name: &str, fast: bool) -> Outcome {
    // Many short trials under heavy jitter and loss with a healthy owner: a
    // takeover is a false one, always.
    let mut o = Outcome::new(name);
    let trials = std::env::var("CUA_CHAOS_TRIALS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10usize);
    let mut false_takeovers = 0u64;
    let mut total_s = 0.0;
    for t in 0..trials {
        let c = match Cluster::start(&laptop_studio(), fast).await {
            Ok(c) => c,
            Err(e) => {
                o.check(false, e);
                return o;
            }
        };
        if !ready(&c).await {
            o.check(false, "the machines did not connect");
            c.stop().await;
            continue;
        }
        // Heavy jitter, 20 % loss, both ways, for a minute of scenario time.
        c.set_path(
            "studio",
            "laptop",
            shape(|s| {
                s.latency_ms = 400;
                s.jitter_ms = 600;
                s.loss = 0.2;
            }),
        );
        // Ordered writes from the laptop make it want authority: the worst case.
        let d = c.t(60.0);
        let t0 = Instant::now();
        let mut i = 0;
        while t0.elapsed() < d {
            i += 1;
            let _ = c
                .node("laptop")
                .backend
                .put(
                    &format!("agents/ada/.cua-lease-{t}-{i}"),
                    b"x".to_vec(),
                    Condition::IfNoneMatch,
                )
                .await;
            tokio::time::sleep(c.t(2.0).max(Duration::from_millis(30))).await;
        }
        total_s += d.as_secs_f64();
        false_takeovers += c.takeovers().await;
        c.stop().await;
    }
    o.metric("trials", trials);
    o.metric("scenario_seconds_under_jitter", total_s);
    o.metric("false_takeovers", false_takeovers);
    o.check(
        false_takeovers == 0,
        format!("{false_takeovers} false takeovers in {trials} trials"),
    );
    o
}

impl Cluster {
    /// [`Cluster::start`] with each machine's wall clock moved (ms).
    pub async fn start_skewed(
        specs: &[Spec],
        fast: bool,
        skew_ms: &[i64],
    ) -> Result<Cluster, String> {
        Cluster::start_inner(specs, fast, skew_ms).await
    }
}

#[allow(dead_code)]
fn unused(_: PathBuf) {}
