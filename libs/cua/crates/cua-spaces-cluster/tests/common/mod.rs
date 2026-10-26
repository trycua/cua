// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! A fake executor (no Spaces: a job "runs" by sleeping) that records
//! every execution, so tests can tell at-least-once execution from
//! exactly-once completion.

#![allow(dead_code)]

pub mod hosts;

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
    /// Session drivers (`["driver", script]`): what each run saw.
    pub drivers: Vec<DriverRun>,
    /// Session driver jobs running now.
    pub live_jobs: std::collections::HashSet<String>,
    /// Session driver jobs that were canceled (by their host's runner).
    pub canceled: Vec<String>,
    /// Output of session drivers by job id (what a real host keeps in its
    /// output log).
    pub outputs: HashMap<String, Vec<u8>>,
}

/// One start of a session driver on a fake host.
#[derive(Clone, Debug)]
pub struct DriverRun {
    pub job_id: String,
    pub env: std::collections::BTreeMap<String, String>,
    pub at: std::time::Instant,
}

impl DriverRun {
    pub fn epoch(&self) -> u64 {
        self.env
            .get("CUA_SESSION_EPOCH")
            .and_then(|e| e.parse().ok())
            .unwrap_or(0)
    }
}

/// `beat:<ms>` prints a line every `<ms>` and never exits; `hang` is silent.
///
/// A session driver is `["driver", "<script>"]`; the script says what the
/// driver does at each epoch: `1=exit:1;2=hang;3=exit:0` (`exit:N`, `hang`
/// until canceled, `sleep:<ms>` then exit 0; an epoch not listed hangs).
///
/// argv: `["sleep", "<ms>"]`, `["exit", "<code>"]`, `["nospace"]` (the
/// Space does not start), `["echo", "<text>"]`.
#[derive(Clone, Default)]
pub struct FakeExec {
    pub seen: Arc<Mutex<Ledger>>,
    pub kinds: Vec<String>,
}

/// What the script says for `epoch`.
fn script_action(script: &str, epoch: u64) -> String {
    script
        .split(';')
        .filter_map(|p| p.split_once('='))
        .find(|(e, _)| e.trim().parse::<u64>().ok() == Some(epoch))
        .map(|(_, a)| a.trim().to_string())
        .unwrap_or_else(|| "hang".into())
}

impl FakeExec {
    /// Every start of a session driver on this host.
    pub fn drivers(&self) -> Vec<DriverRun> {
        self.seen.lock().unwrap().drivers.clone()
    }

    pub fn live_drivers(&self) -> usize {
        self.seen.lock().unwrap().live_jobs.len()
    }

    pub fn canceled(&self) -> Vec<String> {
        self.seen.lock().unwrap().canceled.clone()
    }

    pub fn new() -> Self {
        Self {
            seen: Arc::default(),
            kinds: vec!["container".into(), "vm".into()],
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
            "driver" => {
                let epoch: u64 = job
                    .env
                    .get("CUA_SESSION_EPOCH")
                    .and_then(|e| e.parse().ok())
                    .unwrap_or(0);
                {
                    let mut s = self.seen.lock().unwrap();
                    s.drivers.push(DriverRun {
                        job_id: job.job_id.clone(),
                        env: job.env.clone(),
                        at: std::time::Instant::now(),
                    });
                    s.live_jobs.insert(job.job_id.clone());
                    s.outputs.insert(
                        job.job_id.clone(),
                        format!("driver epoch {epoch}\n").into_bytes(),
                    );
                }
                let action = script_action(&arg, epoch);
                let end = match action.split_once(':') {
                    Some(("exit", n)) => ExecEnd::Exited(n.parse().unwrap_or(1)),
                    Some(("beat", ms)) => {
                        let ms: u64 = ms.parse().unwrap_or(100);
                        loop {
                            tokio::select! {
                                _ = cancel.cancelled() => break ExecEnd::Canceled,
                                _ = tokio::time::sleep(Duration::from_millis(ms)) => {
                                    self.seen.lock().unwrap()
                                        .outputs
                                        .entry(job.job_id.clone())
                                        .or_default()
                                        .extend_from_slice(b"CUA-HEARTBEAT\n");
                                }
                            }
                        }
                    }
                    Some(("sleep", ms)) => {
                        let ms: u64 = ms.parse().unwrap_or(10);
                        tokio::select! {
                            _ = cancel.cancelled() => ExecEnd::Canceled,
                            _ = tokio::time::sleep(Duration::from_millis(ms)) => ExecEnd::Exited(0),
                        }
                    }
                    _ => {
                        cancel.cancelled().await;
                        ExecEnd::Canceled
                    }
                };
                let mut s = self.seen.lock().unwrap();
                s.live_jobs.remove(&job.job_id);
                if end == ExecEnd::Canceled {
                    s.canceled.push(job.job_id.clone());
                }
                end
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
        if job.argv[0] == "driver" {
            out.stdout = format!(
                "driver epoch {}\n",
                job.env.get("CUA_SESSION_EPOCH").map_or("?", |e| e.as_str())
            )
            .into_bytes();
        }
        out.run_ms = 1;
        out
    }

    fn output(&self, job_id: &str, _attempt: u32, from: u64) -> Option<(Vec<u8>, u64)> {
        let s = self.seen.lock().unwrap();
        let all = s.outputs.get(job_id)?;
        let size = all.len() as u64;
        let start = (from.min(size)) as usize;
        Some((all[start..].to_vec(), size))
    }

    async fn remove_space(&self, space: &str) {
        let mut s = self.seen.lock().unwrap();
        s.removed.push(space.to_string());
        s.live.remove(space);
    }

    fn offers(&self) -> Offers {
        Offers {
            kinds: self.kinds.clone(),
            runtimes: vec!["gvisor".into(), "runc".into()],
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

/// An enrolled device: a P-256 device key (what the relay enrolled) that
/// certifies its pool key.
pub struct TestDevice {
    pair: ring::signature::EcdsaKeyPair,
    public: String,
}

impl TestDevice {
    pub fn new() -> Arc<Self> {
        use base64::Engine as _;
        use ring::signature::KeyPair as _;
        let rng = ring::rand::SystemRandom::new();
        let alg = &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING;
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(alg, &rng).unwrap();
        let pair = ring::signature::EcdsaKeyPair::from_pkcs8(alg, pkcs8.as_ref(), &rng).unwrap();
        let public =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(pair.public_key().as_ref());
        Arc::new(Self { pair, public })
    }
}

impl cua_spaces_cluster::pool::keys::DeviceSigner for TestDevice {
    fn device(&self) -> String {
        cua_spaces_cluster::pool::seal::device_id(&self.public).unwrap()
    }
    fn public_key(&self) -> String {
        self.public.clone()
    }
    fn sign(&self, message: &str) -> Result<String, String> {
        use base64::Engine as _;
        let rng = ring::rand::SystemRandom::new();
        self.pair
            .sign(&rng, message.as_bytes())
            .map(|s| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s.as_ref()))
            .map_err(|_| "sign".to_string())
    }
}
