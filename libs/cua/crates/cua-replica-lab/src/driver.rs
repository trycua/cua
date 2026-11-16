// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The simulated durable agent: N steps, each writing a file and a journal
//! marker through the Volume; resumable from the machine's copy of the
//! journal; the end writes the checksum of every step.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cua_volume::backend::Condition;
use cua_volume::replica::supervision::{DriveEnd, Driver, RunCtx};

/// One step as it ran.
#[derive(Clone, Debug)]
pub struct StepLog {
    pub machine: String,
    pub epoch: u64,
    pub step: u64,
    pub at: Instant,
    /// Wall clock, for lines printed by a node process.
    pub wall_ms: u64,
}

/// What step `i` writes (the same in any epoch).
pub fn content(i: u64, size: usize) -> Vec<u8> {
    let head = format!("step {i}: {}\n", cua_volume::sha256_hex(&i.to_le_bytes()));
    let mut v = head.into_bytes();
    let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    while v.len() < size {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.to_le_bytes());
    }
    v.truncate(size.max(1));
    v
}

/// The checksum the run must end with.
pub fn expected_sum(n: u64, size: usize) -> String {
    let mut all = vec![];
    for i in 0..n {
        all.extend(content(i, size));
    }
    cua_volume::sha256_hex(&all)
}

/// Called with each step as it starts.
pub type OnStep = Arc<dyn Fn(&StepLog) + Send + Sync>;

pub struct Steps {
    pub n: u64,
    pub step_ms: u64,
    /// Bytes per step's file.
    pub size: usize,
    pub log: Arc<Mutex<Vec<StepLog>>>,
    /// Called with each step as it starts (a node process prints it).
    pub on_step: Option<OnStep>,
}

#[async_trait::async_trait]
impl Driver for Steps {
    async fn drive(&self, mut ctx: RunCtx) -> DriveEnd {
        let prefix = format!("agents/{}/journal/", ctx.agent);
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
            let e = StepLog {
                machine: ctx.machine.clone(),
                epoch: ctx.epoch,
                step: i,
                at: Instant::now(),
                wall_ms: cua_volume::now_ms(),
            };
            if let Some(f) = &self.on_step {
                f(&e);
            }
            self.log.lock().expect("log").push(e);
            let out = format!("agents/{}/out/step-{i}", ctx.agent);
            if let Err(e) = ctx
                .backend
                .put(&out, content(i, self.size), Condition::None)
                .await
            {
                return DriveEnd::Failed(format!("write: {e}"));
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
        let mut all = vec![];
        for s in 0..self.n {
            match ctx
                .backend
                .get(&format!("agents/{}/out/step-{s}", ctx.agent), None)
                .await
            {
                Ok((b, _)) => all.extend(b),
                Err(e) => return DriveEnd::Failed(format!("step {s} is missing: {e}")),
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
