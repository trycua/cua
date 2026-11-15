//! A fake executor (no Spaces: a job "runs" by sleeping) that records
//! every execution, so tests can tell at-least-once execution from
//! exactly-once completion.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use cua_cluster::JobSpec;
use cua_cluster::node::{ExecEnd, ExecOutcome, JobExecutor, Offers};
use tokio_util::sync::CancellationToken;

/// What ran.
#[derive(Default)]
pub struct Ledger {
    /// Starts per job id.
    pub starts: HashMap<String, u32>,
    /// Ends (not canceled) per job id.
    pub ends: HashMap<String, u32>,
    /// Spaces removed by crash recovery.
    pub removed: Vec<String>,
    /// Spaces alive now.
    pub live: std::collections::HashSet<String>,
}

/// argv: `["sleep", "<ms>"]`, `["exit", "<code>"]`, `["nospace"]` (the
/// Space does not start), `["echo", "<text>"]`.
#[derive(Clone, Default)]
pub struct FakeExec {
    pub seen: Arc<Mutex<Ledger>>,
    pub kinds: Vec<String>,
    /// The runtimes offered now (a test installs one while the host runs).
    pub runtimes: Arc<Mutex<Vec<String>>>,
}

impl FakeExec {
    pub fn new() -> Self {
        Self {
            seen: Arc::default(),
            kinds: vec!["container".into(), "vm".into()],
            runtimes: Arc::new(Mutex::new(vec!["gvisor".into(), "runc".into()])),
        }
    }

    pub fn starts(&self, id: &str) -> u32 {
        self.seen
            .lock()
            .unwrap()
            .starts
            .get(id)
            .copied()
            .unwrap_or(0)
    }
}

#[async_trait]
impl JobExecutor for FakeExec {
    async fn run(&self, job: &JobSpec, space: &str, cancel: CancellationToken) -> ExecOutcome {
        {
            let mut s = self.seen.lock().unwrap();
            *s.starts.entry(job.job_id.clone()).or_default() += 1;
            s.live.insert(space.to_string());
        }
        let arg = job.argv.get(1).cloned().unwrap_or_default();
        let end = match job.argv[0].as_str() {
            "sleep" => {
                let ms: u64 = arg.parse().unwrap_or(10);
                tokio::select! {
                    _ = cancel.cancelled() => ExecEnd::Canceled,
                    _ = tokio::time::sleep(Duration::from_millis(ms)) => ExecEnd::Exited(0),
                }
            }
            "exit" => ExecEnd::Exited(arg.parse().unwrap_or(1)),
            "nospace" => ExecEnd::Error("the Space did not start".into()),
            "hang" => {
                cancel.cancelled().await;
                ExecEnd::Canceled
            }
            _ => ExecEnd::Exited(0),
        };
        let mut s = self.seen.lock().unwrap();
        s.live.remove(space);
        if end != ExecEnd::Canceled {
            *s.ends.entry(job.job_id.clone()).or_default() += 1;
        }
        let mut out = ExecOutcome::ended(end);
        if job.argv[0] == "echo" {
            out.stdout = format!("{arg}\n").into_bytes();
        }
        out.run_ms = 1;
        out
    }

    async fn remove_space(&self, space: &str) {
        let mut s = self.seen.lock().unwrap();
        s.removed.push(space.to_string());
        s.live.remove(space);
    }

    fn offers(&self) -> Offers {
        Offers {
            kinds: self.kinds.clone(),
            runtimes: self.runtimes.lock().unwrap().clone(),
        }
    }
}

pub fn spec(id: &str, attempt: u32, argv: &[&str]) -> cua_proto::env::v1::HostJobSpec {
    cua_proto::env::v1::HostJobSpec {
        job_id: id.into(),
        attempt,
        argv: argv.iter().map(|s| s.to_string()).collect(),
        millicpus: 1000,
        memory_mb: 256,
        timeout_secs: 60,
        ..Default::default()
    }
}
