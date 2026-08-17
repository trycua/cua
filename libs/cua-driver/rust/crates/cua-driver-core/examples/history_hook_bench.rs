//! Manual synchronous producer-boundary benchmark.
//!
//! Canonical release evidence is collected in the signed Lume candidate:
//! `cargo run -p cua-driver-core --release --example history_hook_bench`.
//! This deliberately has no always-on latency threshold.

use cua_driver_core::history::{
    ApplicationIdentity, ApplicationIdentityProvider, HistoryConfig, HistoryError, HistoryKey,
    HistoryManager, KeyProvider, DEFAULT_QUOTA_BYTES, DEFAULT_RETENTION_DAYS,
};
use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};
use zeroize::Zeroizing;

#[derive(Default)]
struct Keys(Mutex<HashMap<String, Vec<u8>>>);

impl KeyProvider for Keys {
    fn load_or_create(&self, namespace: &str) -> Result<HistoryKey, HistoryError> {
        let reference = format!("{namespace}.history.v1");
        let mut keys = self.0.lock().unwrap();
        let bytes = keys.entry(reference.clone()).or_insert_with(|| vec![7; 32]);
        Ok(HistoryKey {
            reference,
            epoch: 1,
            bytes: Zeroizing::new(bytes.clone()),
        })
    }
    fn load(&self, _: &str, reference: &str) -> Result<HistoryKey, HistoryError> {
        Ok(HistoryKey {
            reference: reference.into(),
            epoch: 1,
            bytes: Zeroizing::new(vec![7; 32]),
        })
    }
    fn references(&self, namespace: &str) -> Result<Vec<String>, HistoryError> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(namespace))
            .cloned()
            .collect())
    }
    fn destroy(&self, _: &str, reference: &str) -> Result<(), HistoryError> {
        self.0.lock().unwrap().remove(reference);
        Ok(())
    }
}

#[derive(Default)]
struct BlockingApps {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
impl BlockingApps {
    fn wait_entered(&self) {
        let mut s = self.state.lock().unwrap();
        while !s.0 {
            s = self.changed.wait(s).unwrap();
        }
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}
impl ApplicationIdentityProvider for BlockingApps {
    fn resolve(&self, _: i64, _: Option<u64>) -> Option<ApplicationIdentity> {
        let mut s = self.state.lock().unwrap();
        s.0 = true;
        self.changed.notify_all();
        while !s.1 {
            s = self.changed.wait(s).unwrap();
        }
        None
    }
}

fn report(label: &str, mut samples: Vec<Duration>) {
    samples.sort_unstable();
    let at = |p: usize| samples[(samples.len() - 1) * p / 100].as_nanos();
    println!(
        "{label}: p50={}ns p95={}ns p99={}ns n={}",
        at(50),
        at(95),
        at(99),
        samples.len()
    );
}

fn main() {
    let temp = tempfile::tempdir().unwrap();
    let apps = Arc::new(BlockingApps::default());
    let manager = HistoryManager::new(
        HistoryConfig {
            root: temp.path().into(),
            namespace: "hook-bench".into(),
            admitted: true,
            platform: "macos".into(),
            retention_days: DEFAULT_RETENTION_DAYS,
            quota_bytes: DEFAULT_QUOTA_BYTES,
        },
        Arc::new(Keys::default()),
        Some(apps.clone()),
    );
    manager.enable().unwrap();

    for _ in 0..100 {
        manager.begin_action("click", &serde_json::json!({}), None);
        manager.flush().unwrap();
    }
    let accepted = (0..2_000)
        .map(|_| {
            let now = Instant::now();
            manager.begin_action("click", &serde_json::json!({}), None);
            let elapsed = now.elapsed();
            manager.flush().unwrap();
            elapsed
        })
        .collect();
    report("accepted", accepted);

    manager.begin_action("click", &serde_json::json!({"pid": 1}), None);
    apps.wait_entered();
    let before = manager.status().dropped_events;
    while manager.status().dropped_events == before {
        manager.begin_action("click", &serde_json::json!({}), None);
    }
    let full = (0..20_000)
        .map(|_| {
            let now = Instant::now();
            manager.begin_action("click", &serde_json::json!({}), None);
            now.elapsed()
        })
        .collect();
    report("full_queue", full);
    apps.release();
}
