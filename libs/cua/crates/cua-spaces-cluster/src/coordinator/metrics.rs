// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What the coordinator counts: throughput, latencies, retries and the
//! duplicates it suppressed. In memory (reset when it restarts), except
//! the totals the database keeps.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Samples kept per latency.
const SAMPLES: usize = 10_000;

/// Counters and recent latencies.
#[derive(Default)]
pub struct Metrics {
    /// Jobs submitted.
    pub submitted: AtomicU64,
    /// Attempts sent to hosts.
    pub dispatched: AtomicU64,
    /// Attempts hosts accepted.
    pub accepted: AtomicU64,
    /// Attempts hosts refused for lack of room.
    pub rejected: AtomicU64,
    /// Jobs that ended succeeded.
    pub succeeded: AtomicU64,
    /// Jobs that ended failed (their command).
    pub failed: AtomicU64,
    /// Jobs that ended in error (out of attempts) or canceled.
    pub ended_other: AtomicU64,
    /// Attempts lost (host down, host error) and run again.
    pub retries: AtomicU64,
    /// Results that arrived for a job already ended or for an attempt
    /// that was not current, and were not recorded.
    pub duplicates_suppressed: AtomicU64,
    /// Hosts marked down.
    pub node_down: AtomicU64,
    /// Lookups of a job's fate on its host (lease expiry).
    pub reconciles: AtomicU64,
    /// Cancels sent to fence an attempt.
    pub fences: AtomicU64,
    /// Attempts newer than any this coordinator dispatched, seen on hosts
    /// (a newer coordinator runs the job): left alone.
    pub stale_view: AtomicU64,
    recent: Mutex<Recent>,
}

#[derive(Default)]
struct Recent {
    /// Ready (queued) to accepted by a host: queueing included.
    wait: VecDeque<u32>,
    /// One `RunHostJobs` call: sent to answered.
    dispatch: VecDeque<u32>,
    /// Submitted to ended.
    e2e: VecDeque<u32>,
    /// Completion times, for throughput.
    ends: VecDeque<Instant>,
}

fn push(q: &mut VecDeque<u32>, v: Duration) {
    if q.len() >= SAMPLES {
        q.pop_front();
    }
    q.push_back(v.as_millis().min(u128::from(u32::MAX)) as u32);
}

fn pct(q: &VecDeque<u32>, p: f64) -> u32 {
    if q.is_empty() {
        return 0;
    }
    let mut v: Vec<u32> = q.iter().copied().collect();
    v.sort_unstable();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

/// A snapshot for the API and `cua cluster status`.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Snapshot {
    /// Counters since the coordinator started.
    pub submitted: u64,
    /// Attempts sent.
    pub dispatched: u64,
    /// Attempts accepted.
    pub accepted: u64,
    /// Attempts refused for lack of room.
    pub rejected: u64,
    /// Ended succeeded.
    pub succeeded: u64,
    /// Ended failed.
    pub failed: u64,
    /// Ended otherwise.
    pub ended_other: u64,
    /// Attempts run again.
    pub retries: u64,
    /// Results not recorded (duplicates and stale attempts).
    pub duplicates_suppressed: u64,
    /// Hosts marked down.
    pub node_down: u64,
    /// Lease-expiry lookups.
    pub reconciles: u64,
    /// Fencing cancels sent.
    pub fences: u64,
    /// Newer attempts seen (this coordinator is stale), left alone.
    pub stale_view: u64,
    /// Ready (queued, or back in the queue) to accepted by a host,
    /// queueing for free capacity included: p50 (ms).
    pub wait_p50_ms: u32,
    /// p99 (ms).
    pub wait_p99_ms: u32,
    /// One dispatch (`RunHostJobs`: placed, sent, accepted or refused by
    /// the host): p50 (ms).
    pub dispatch_p50_ms: u32,
    /// p99 (ms).
    pub dispatch_p99_ms: u32,
    /// Submitted to ended: p50 (ms).
    pub e2e_p50_ms: u32,
    /// Submitted to ended: p99 (ms).
    pub e2e_p99_ms: u32,
    /// Jobs ended per second over the last 10 s.
    pub per_sec_10s: f64,
    /// Over the last 60 s.
    pub per_sec_60s: f64,
}

impl Metrics {
    /// One acceptance, `d` after the job became dispatchable.
    pub fn queue_wait(&self, d: Duration) {
        push(&mut self.recent.lock().expect("metrics").wait, d);
    }

    /// One `RunHostJobs` call that took `d`.
    pub fn dispatch_call(&self, d: Duration) {
        push(&mut self.recent.lock().expect("metrics").dispatch, d);
    }

    /// One job ended, `d` after it was submitted.
    pub fn ended(&self, d: Duration) {
        let mut r = self.recent.lock().expect("metrics");
        push(&mut r.e2e, d);
        let now = Instant::now();
        r.ends.push_back(now);
        while r
            .ends
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(60))
        {
            r.ends.pop_front();
        }
    }

    /// Adds one to a counter.
    pub fn inc(c: &AtomicU64) {
        c.fetch_add(1, Ordering::Relaxed);
    }

    /// The numbers now.
    pub fn snapshot(&self) -> Snapshot {
        let r = self.recent.lock().expect("metrics");
        let now = Instant::now();
        let in_last = |s: u64| {
            r.ends
                .iter()
                .filter(|t| now.duration_since(**t) <= Duration::from_secs(s))
                .count() as f64
                / s as f64
        };
        let g = |c: &AtomicU64| c.load(Ordering::Relaxed);
        Snapshot {
            submitted: g(&self.submitted),
            dispatched: g(&self.dispatched),
            accepted: g(&self.accepted),
            rejected: g(&self.rejected),
            succeeded: g(&self.succeeded),
            failed: g(&self.failed),
            ended_other: g(&self.ended_other),
            retries: g(&self.retries),
            duplicates_suppressed: g(&self.duplicates_suppressed),
            node_down: g(&self.node_down),
            reconciles: g(&self.reconciles),
            fences: g(&self.fences),
            stale_view: g(&self.stale_view),
            wait_p50_ms: pct(&r.wait, 0.5),
            wait_p99_ms: pct(&r.wait, 0.99),
            dispatch_p50_ms: pct(&r.dispatch, 0.5),
            dispatch_p99_ms: pct(&r.dispatch, 0.99),
            e2e_p50_ms: pct(&r.e2e, 0.5),
            e2e_p99_ms: pct(&r.e2e, 0.99),
            per_sec_10s: in_last(10),
            per_sec_60s: in_last(60),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_and_throughput() {
        let m = Metrics::default();
        for ms in 1..=100 {
            m.dispatch_call(Duration::from_millis(ms));
            m.queue_wait(Duration::from_millis(ms * 2));
            m.ended(Duration::from_millis(ms * 10));
        }
        Metrics::inc(&m.retries);
        let s = m.snapshot();
        assert_eq!(s.dispatch_p50_ms, 51);
        assert_eq!(s.dispatch_p99_ms, 99);
        assert_eq!(s.wait_p99_ms, 198);
        assert_eq!(s.e2e_p99_ms, 990);
        assert_eq!(s.retries, 1);
        assert!((s.per_sec_10s - 10.0).abs() < 1e-9);
    }
}
