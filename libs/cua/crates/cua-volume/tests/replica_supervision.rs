// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable agents across machines on the simulated network: the run goes to
//! the most reliable machine, survives a closed laptop, moves when its
//! machine dies, and a holder that wakes after being replaced is fenced.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use cua_volume::backend::{Backend, Condition};
use cua_volume::replica::PowerEvent;
use cua_volume::replica::supervision::{DriveEnd, Driver, RunCtx, Supervision, SupervisionConfig};

type StepLog = Arc<Mutex<Vec<(String, u64, u64, Instant)>>>;

/// An agent made of N steps; step i writes `out/step-i` (the same bytes in
/// any epoch) and a journal marker, and the end writes the checksum of every
/// step. It resumes from the steps its machine's journal holds.
struct Steps {
    n: u64,
    step_ms: u64,
    log: StepLog,
}

fn content(i: u64) -> Vec<u8> {
    format!("step {i}: {}", cua_volume::sha256_hex(&i.to_le_bytes())).into_bytes()
}

fn expected_sum(n: u64) -> String {
    let mut all = vec![];
    for i in 0..n {
        all.extend(content(i));
    }
    cua_volume::sha256_hex(&all)
}

#[async_trait::async_trait]
impl Driver for Steps {
    async fn drive(&self, mut ctx: RunCtx) -> DriveEnd {
        let prefix = format!("agents/{}/journal/", ctx.agent);
        // How far this machine's copy of the journal reaches (contiguous).
        let mut held = 0u64;
        while ctx
            .backend
            .head(&format!("{prefix}{held:06}"))
            .await
            .ok()
            .flatten()
            .is_some()
        {
            held += 1;
        }
        ctx.held_reaches(held);
        let mut i = held;
        while i < self.n {
            if ctx.fenced() {
                return DriveEnd::Fenced;
            }
            self.log
                .lock()
                .unwrap()
                .push((ctx.machine.clone(), ctx.epoch, i, Instant::now()));
            let out = format!("agents/{}/out/step-{i}", ctx.agent);
            if ctx
                .backend
                .put(&out, content(i), Condition::None)
                .await
                .is_err()
            {
                return DriveEnd::Failed("write".into());
            }
            let _ = ctx
                .backend
                .put(&format!("{prefix}{i:06}"), vec![b'.'], Condition::None)
                .await;
            ctx.report(i + 1);
            i += 1;
            tokio::time::sleep(Duration::from_millis(self.step_ms)).await;
        }
        if ctx.fenced() {
            return DriveEnd::Fenced;
        }
        // The result: the checksum of every step as the Volume holds it.
        let mut all = vec![];
        for s in 0..self.n {
            match ctx
                .backend
                .get(&format!("agents/{}/out/step-{s}", ctx.agent), None)
                .await
            {
                Ok((b, _)) => all.extend(b),
                Err(_) => return DriveEnd::Failed("a step is missing".into()),
            }
        }
        let sum = cua_volume::sha256_hex(&all);
        let _ = ctx
            .backend
            .put(
                &format!("agents/{}/result.txt", ctx.agent),
                sum.clone().into_bytes(),
                Condition::None,
            )
            .await;
        DriveEnd::Done(sum)
    }
}

struct World {
    sim: Sim,
    sups: HashMap<String, Supervision>,
    log: StepLog,
    awake: Arc<Mutex<Vec<String>>>,
}

async fn world(specs: &[(&str, u64)], n: u64, step_ms: u64) -> World {
    let sim = Sim::new(specs).await;
    let mut w = World {
        sim,
        sups: HashMap::new(),
        log: Arc::new(Mutex::new(vec![])),
        awake: Arc::new(Mutex::new(vec![])),
    };
    for (id, _) in specs {
        w.supervise(id, n, step_ms);
    }
    w
}

impl World {
    fn supervise(&mut self, id: &str, n: u64, step_ms: u64) {
        let m = self.sim.get(id);
        let awake = self.awake.clone();
        let s = Supervision::new(
            m.node.clone(),
            m.backend.clone(),
            SupervisionConfig::fast(),
            Arc::new(Steps {
                n,
                step_ms,
                log: self.log.clone(),
            }),
            Some(Arc::new(move |why: &str| {
                awake.lock().unwrap().push(why.to_string());
                Box::new(()) as Box<dyn std::any::Any + Send>
            })),
        );
        s.spawn();
        self.sups.insert(id.to_string(), s);
    }

    fn who_ran(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .log
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.0.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    async fn result_on(&self, id: &str, agent: &str) -> Option<String> {
        self.sim
            .get(id)
            .store
            .get(&format!("agents/{agent}/result.txt"), None)
            .await
            .ok()
            .map(|(b, _)| String::from_utf8(b).unwrap())
    }

    async fn stop_all(&self) {
        for s in self.sups.values() {
            s.stop().await;
        }
        self.sim.shutdown().await;
    }
}

#[tokio::test]
async fn a_run_started_on_the_laptop_runs_on_the_studio_and_says_so() {
    let w = world(&[("studio", 9000), ("laptop", 2000)], 20, 30).await;
    group_ready(&w.sim, "studio").await;
    let info = w.sups["laptop"]
        .start_run("ada", serde_json::json!({"task": "demo"}))
        .await
        .unwrap();
    assert_eq!(info.run_on, "studio");
    assert_eq!(
        info.words,
        "Your agent keeps running on studio. Files are on 2 machines. You can close this machine."
    );
    eventually("the run finishes", 30, || async {
        w.result_on("laptop", "ada").await.is_some()
    })
    .await;
    assert_eq!(
        w.who_ran(),
        vec!["studio".to_string()],
        "the laptop never drove it"
    );
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(20)
    );
    assert_eq!(
        w.result_on("studio", "ada").await.unwrap(),
        expected_sum(20)
    );
    // The studio held a sleep assertion for it with a readable reason.
    assert_eq!(
        w.awake.lock().unwrap().as_slice(),
        ["Cua: the durable agent ada is running".to_string()]
    );
    w.stop_all().await;
}

#[tokio::test]
async fn closing_the_laptop_does_not_stop_the_run_or_lose_files() {
    let w = world(&[("studio", 9000), ("laptop", 2000)], 60, 40).await;
    group_ready(&w.sim, "studio").await;
    w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    eventually("some steps ran", 20, || async {
        w.log.lock().unwrap().len() >= 8
    })
    .await;
    // The lid closes.
    w.sim
        .get("laptop")
        .node
        .notify_power(PowerEvent::WillSleep)
        .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    w.sim.get("laptop").node.freeze(true);
    w.sim.cut("studio", "laptop");
    let at_close = w.log.lock().unwrap().len();
    eventually(
        "the run finishes while the laptop is closed",
        30,
        || async { w.result_on("studio", "ada").await.is_some() },
    )
    .await;
    assert!(w.log.lock().unwrap().len() > at_close);
    assert_eq!(w.who_ran(), vec!["studio".to_string()]);
    // The lid opens; the laptop has every file.
    w.sim.heal("studio", "laptop");
    w.sim.get("laptop").node.freeze(false);
    w.sim
        .get("laptop")
        .node
        .notify_power(PowerEvent::DidWake)
        .await;
    eventually("the laptop has the result and every step", 20, || async {
        converged(&w.sim, &["studio", "laptop"]).await
            && w.result_on("laptop", "ada").await.is_some()
    })
    .await;
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(60)
    );
    assert_eq!(w.sim.get("studio").node.status().await.takeovers, 0);
    w.stop_all().await;
}

#[tokio::test]
async fn when_the_studio_dies_the_laptop_takes_the_run_and_resumes_it() {
    let w = world(&[("studio", 9000), ("laptop", 2000)], 80, 40).await;
    group_ready(&w.sim, "studio").await;
    w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    eventually("steps are running on the studio", 20, || async {
        w.log.lock().unwrap().len() >= 15
    })
    .await;
    // The studio's process dies.
    let dead_at = Instant::now();
    w.sups["studio"].stop().await;
    w.sim.get("studio").node.shutdown().await;
    w.sim.cut("studio", "laptop");
    let ran_on_studio = w.log.lock().unwrap().len();
    eventually("the laptop runs it", 30, || async {
        w.log.lock().unwrap().iter().any(|e| e.0 == "laptop")
    })
    .await;
    let took = w
        .log
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.0 == "laptop")
        .map(|e| e.3.duration_since(dead_at))
        .unwrap();
    assert!(took < Duration::from_secs(8), "took over in {took:?}");
    eventually("it finishes on the laptop", 30, || async {
        w.result_on("laptop", "ada").await.is_some()
    })
    .await;
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(80)
    );
    // It did not start again from the beginning: the laptop's first step is
    // after what the studio had done (its journal copy was there).
    let first_laptop_step = w
        .log
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.0 == "laptop")
        .map(|e| e.2)
        .unwrap();
    assert!(
        first_laptop_step >= 10,
        "restarted at step {first_laptop_step}"
    );
    assert!(ran_on_studio >= 15);
    let runs = w.sups["laptop"].runs().await.unwrap();
    let note = &runs[0].note;
    assert!(note.starts_with("Resumed on laptop"), "{note}");
    w.stop_all().await;
}

#[tokio::test]
async fn a_frozen_holder_that_wakes_after_a_takeover_is_fenced() {
    let w = world(&[("studio", 9000), ("laptop", 2000)], 100, 40).await;
    group_ready(&w.sim, "studio").await;
    w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    eventually("running on the studio", 20, || async {
        w.log.lock().unwrap().len() >= 10
    })
    .await;
    // The studio freezes (a stopped process) and loses the network.
    w.sim.get("studio").node.freeze(true);
    w.sim.cut("studio", "laptop");
    eventually("the laptop takes the run", 30, || async {
        w.log.lock().unwrap().iter().any(|e| e.0 == "laptop")
    })
    .await;
    // The studio thaws and the network returns.
    tokio::time::sleep(Duration::from_millis(300)).await;
    w.sim.heal("studio", "laptop");
    w.sim.get("studio").node.freeze(false);
    w.sim
        .get("studio")
        .node
        .notify_power(PowerEvent::DidWake)
        .await;
    eventually("the run finishes", 60, || async {
        w.result_on("laptop", "ada").await.is_some()
            && converged(&w.sim, &["studio", "laptop"]).await
    })
    .await;
    // The output is right everywhere, whatever ran twice in between.
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(100)
    );
    assert_eq!(
        w.result_on("studio", "ada").await.unwrap(),
        expected_sum(100)
    );
    for i in 0..100 {
        let b = w
            .sim
            .get("studio")
            .store
            .get(&format!("agents/ada/out/step-{i}"), None)
            .await
            .unwrap()
            .0;
        assert_eq!(b, content(i), "step {i} is wrong or missing");
    }
    // Exactly one epoch holds the lease at the end, the later one; the
    // stopped holder's driver stopped.
    let runs = w.sups["studio"].runs().await.unwrap();
    assert!(runs[0].epoch >= 2, "{runs:?}");
    eventually("the stopped holder's driver is stopped", 30, || async {
        let r = w.sups["studio"].runs().await.unwrap();
        !r[0].running_here
    })
    .await;
    w.stop_all().await;
}

#[tokio::test]
async fn a_run_on_a_laptop_goes_back_to_the_studio_when_it_returns() {
    // The studio starts later than the laptop: the run begins on the laptop.
    let sim = Sim::new(&[("studio", 9000), ("laptop", 2000)]).await;
    group_ready(&sim, "studio").await;
    let mut w = World {
        sim,
        sups: HashMap::new(),
        log: Arc::new(Mutex::new(vec![])),
        awake: Arc::new(Mutex::new(vec![])),
    };
    // Only the laptop supervises at first, and the studio is cut off.
    w.supervise("laptop", 120, 40);
    w.sim.cut("studio", "laptop");
    eventually("the laptop sees the studio go", 10, || async {
        w.sim.get("laptop").node.status().await.peers[0].health != "up"
    })
    .await;
    w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    // (The write cannot reach the owner, so the laptop needs authority.)
    needs_authority(w.sim.get("laptop"), "agents/ada/.cua-lease").await;
    let started = w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await;
    assert!(started.is_ok());
    eventually("the laptop runs it", 20, || async {
        w.log.lock().unwrap().iter().any(|e| e.0 == "laptop")
    })
    .await;
    // The studio comes back and supervises.
    w.sim.heal("studio", "laptop");
    w.supervise("studio", 120, 40);
    eventually("the studio gets the run", 40, || async {
        w.log.lock().unwrap().iter().any(|e| e.0 == "studio")
    })
    .await;
    eventually("it finishes", 60, || async {
        w.result_on("laptop", "ada").await.is_some()
    })
    .await;
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(120)
    );
    w.stop_all().await;
}

#[tokio::test]
async fn one_machine_says_it_will_pause_and_keeps_itself_awake() {
    let w = world(&[("macbook", 2000)], 5, 20).await;
    let info = w.sups["macbook"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    assert!(
        info.words.contains("will pause when it sleeps"),
        "{}",
        info.words
    );
    assert!(info.words.contains("Add another machine"), "{}", info.words);
    eventually("done", 20, || async {
        w.result_on("macbook", "ada").await.is_some()
    })
    .await;
    assert_eq!(w.who_ran(), vec!["macbook".to_string()]);
    assert_eq!(w.awake.lock().unwrap().len(), 1);
    w.stop_all().await;
}

/// The run moves between architectures: replication and the journal are the
/// same, built programs are not, and the words say so.
#[tokio::test]
async fn a_takeover_across_architectures_says_built_programs_are_not_reused() {
    let w = world(&[("studio", 9000), ("laptop", 2000)], 60, 40).await;
    for (id, rank, arch) in [("studio", 9000, "x86_64"), ("laptop", 2000, "aarch64")] {
        w.sim.get(id).node.set_facts(cua_volume::replica::Facts {
            rank,
            free_disk: 500 << 30,
            arch: arch.into(),
            ..Default::default()
        });
    }
    group_ready(&w.sim, "studio").await;
    w.sim
        .get("studio")
        .backend
        .put(
            "agents/ada/target/debug/ada",
            b"ELF".to_vec(),
            Condition::None,
        )
        .await
        .unwrap();
    w.sups["laptop"]
        .start_run("ada", serde_json::json!({}))
        .await
        .unwrap();
    eventually("steps run on the studio", 20, || async {
        w.log.lock().unwrap().len() >= 10
    })
    .await;
    w.sups["studio"].stop().await;
    w.sim.get("studio").node.shutdown().await;
    w.sim.cut("studio", "laptop");
    eventually("it finishes on the laptop", 40, || async {
        w.result_on("laptop", "ada").await.is_some()
    })
    .await;
    assert_eq!(
        w.result_on("laptop", "ada").await.unwrap(),
        expected_sum(60)
    );
    let note = w.sups["laptop"].runs().await.unwrap()[0].note.clone();
    assert!(note.starts_with("Resumed on laptop"), "{note}");
    assert!(
        note.contains("studio is amd64")
            && note.contains("target/")
            && note.contains("built again"),
        "{note}"
    );
    w.stop_all().await;
}
