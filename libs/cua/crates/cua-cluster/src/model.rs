//! The job and node words shared by the host's runner and the coordinator.

use cua_proto::env::v1 as pb;
use serde::{Deserialize, Serialize};

/// Default CPU reservation of a job, in thousandths of a CPU.
pub const DEFAULT_MILLICPUS: u32 = 1000;
/// Default memory reservation of a job, in MiB.
pub const DEFAULT_MEMORY_MB: u64 = 1024;
/// Default time a job's command may run.
pub const DEFAULT_TIMEOUT_SECS: u32 = 600;
/// Default bytes of stdout, stderr and the result kept (each, the tail).
pub const DEFAULT_MAX_OUTPUT: u32 = 64 * 1024;
/// Most jobs in one `RunHostJobs` call.
pub const MAX_BATCH: usize = 512;
/// Largest job bundle (`HostJobSpec.bundle`, a gzip tar): sealed job specs
/// live in the pool log, so it stays small. 8 MiB fits a whole project's
/// source with its lockfile (`cua run -- CMD` ships the current directory);
/// data goes elsewhere (the run's data directory, a checkpoint store).
pub const MAX_BUNDLE: usize = 8 * 1024 * 1024;
/// The kind that runs a member in a dedicated host's own Space
/// ([`crate::machine`]): offered only by hosts set up as dedicated.
pub const KIND_MACHINE: &str = "machine";
/// The kind word of the removed native jobs (a command run on the host
/// itself, outside any Space): refused everywhere ([`process_refused`]).
pub const KIND_PROCESS: &str = "process";

/// Why a job asking for the removed `process` kind does not run.
pub fn process_refused() -> String {
    "kind \"process\" is gone: every job runs in a Space (leave the kind out: a macOS VM on a \
     Mac, a gVisor container on a shared Linux host, the machine's own Space on a dedicated \
     host)"
        .into()
}

/// A job's state on a host (`HostJob.state`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostJobState {
    /// Accepted; its Space is starting or its command runs.
    Running,
    /// The command exited 0.
    Succeeded,
    /// The command exited non-zero or ran out of time.
    Failed,
    /// The host could not run it (its Space did not start, the daemon
    /// restarted mid-job). Running it again may work.
    Error,
    /// Canceled, or an attempt fenced before it arrived.
    Canceled,
}

impl HostJobState {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Error => "error",
            Self::Canceled => "canceled",
        }
    }

    /// Parses the wire word.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "running" => Self::Running,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "error" => Self::Error,
            "canceled" => Self::Canceled,
            _ => return None,
        })
    }

    /// Whether the job ended.
    pub fn is_terminal(self) -> bool {
        self != Self::Running
    }
}

/// What a job runs, as the runner keeps it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSpec {
    /// Unique id (the idempotency key on the host).
    pub job_id: String,
    /// Delivery attempt, from 1.
    pub attempt: u32,
    /// Image (empty: the canonical Linux image).
    #[serde(default)]
    pub image: String,
    /// `container`, `vm`, `microvm` or empty (auto).
    #[serde(default)]
    pub kind: String,
    /// A runtime word or empty (auto).
    #[serde(default)]
    pub runtime: String,
    /// CPU reservation in thousandths.
    pub millicpus: u32,
    /// Memory reservation in MiB.
    pub memory_mb: u64,
    /// The command.
    pub argv: Vec<String>,
    /// Its environment.
    #[serde(default)]
    pub env: std::collections::BTreeMap<String, String>,
    /// Seconds the command may run.
    pub timeout_secs: u32,
    /// A file to return as the result.
    #[serde(default)]
    pub result_path: String,
    /// Bytes of each output kept.
    pub max_output_bytes: u32,
    /// Placement only: the host OS it needs (`linux`, `macos`, `windows`;
    /// empty: any). Never sent to the host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub os: String,
    /// Placement only: the host CPU architecture it needs (`x86_64`,
    /// `aarch64`; empty: any).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arch: String,
    /// Placement only: the one host (or elastic provider, `modal`) it must
    /// run on; empty: any.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on: String,
    /// Files unpacked into a member's work directory before it starts (a
    /// gzip tar, at most [`MAX_BUNDLE`]).
    #[serde(
        default,
        with = "crate::ledger::b64",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub bundle: Vec<u8>,
    /// The run it belongs to (a member's `CUA_JOB_DATA` is per run).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub run_id: String,
    /// The accelerator it needs (`gpu`: any; `apple`; `nvidia`; empty:
    /// none): matched against `HostNodeStatus.accelerators` for placement,
    /// and the host gives the member's Space that GPU.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub accelerator: String,
    /// Coordinator only: times a failed attempt (non-zero exit, out of
    /// time) runs again, with a backoff (2 s, 4 s, ... at most 60 s).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub retries: u32,
    /// Host services the member reaches on its own loopback for its
    /// lifetime (`compat:e2b`, `compat:daytona`): the host that runs it
    /// starts each with the provider its daemon registered and refuses a
    /// name it has none for.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub services: Vec<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The canonical spelling of an accelerator request: `gpu` (any),
/// `apple`, `nvidia`, or empty (none needed).
pub fn normalize_accelerator(a: &str) -> Result<String, String> {
    match a.trim().to_ascii_lowercase().as_str() {
        "" | "none" => Ok(String::new()),
        "gpu" | "any" | "auto" => Ok("gpu".into()),
        "apple" | "metal" | "mps" => Ok("apple".into()),
        "nvidia" | "cuda" => Ok("nvidia".into()),
        other => Err(format!(
            "accelerator {other:?}: use gpu (any), apple or nvidia"
        )),
    }
}

/// Whether a host that reports `accelerators` satisfies `want` (a
/// [`normalize_accelerator`] word).
pub fn accelerator_fits(want: &str, accelerators: &[String]) -> bool {
    match want {
        "" => true,
        "gpu" => !accelerators.is_empty(),
        vendor => accelerators.iter().any(|a| {
            a.split(':')
                .next()
                .is_some_and(|v| v.eq_ignore_ascii_case(vendor))
        }),
    }
}

/// The canonical spelling of a CPU architecture (`arm64` is `aarch64`,
/// `amd64` is `x86_64`).
pub fn normalize_arch(arch: &str) -> String {
    match arch.trim().to_ascii_lowercase().as_str() {
        "arm64" | "aarch64" => "aarch64".into(),
        "amd64" | "x86_64" | "x64" => "x86_64".into(),
        other => other.to_string(),
    }
}

impl JobSpec {
    /// From the wire, with defaults applied.
    pub fn from_pb(p: &pb::HostJobSpec) -> Self {
        Self {
            job_id: p.job_id.clone(),
            attempt: p.attempt,
            image: p.image.clone(),
            kind: normalize_auto(&p.kind),
            runtime: normalize_auto(&p.runtime),
            millicpus: if p.millicpus == 0 {
                DEFAULT_MILLICPUS
            } else {
                p.millicpus
            },
            memory_mb: if p.memory_mb == 0 {
                DEFAULT_MEMORY_MB
            } else {
                p.memory_mb
            },
            argv: p.argv.clone(),
            env: p.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            timeout_secs: if p.timeout_secs == 0 {
                DEFAULT_TIMEOUT_SECS
            } else {
                p.timeout_secs
            },
            result_path: p.result_path.clone(),
            max_output_bytes: if p.max_output_bytes == 0 {
                DEFAULT_MAX_OUTPUT
            } else {
                p.max_output_bytes.min(16 * 1024 * 1024)
            },
            os: String::new(),
            arch: String::new(),
            on: String::new(),
            bundle: p.bundle.clone(),
            run_id: p.run_id.clone(),
            accelerator: normalize_accelerator(&p.accelerator).unwrap_or_default(),
            retries: 0,
            services: p.services.clone(),
        }
    }

    /// To the wire.
    pub fn to_pb(&self) -> pb::HostJobSpec {
        pb::HostJobSpec {
            job_id: self.job_id.clone(),
            attempt: self.attempt,
            image: self.image.clone(),
            kind: self.kind.clone(),
            runtime: self.runtime.clone(),
            millicpus: self.millicpus,
            memory_mb: self.memory_mb,
            argv: self.argv.clone(),
            env: self
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            timeout_secs: self.timeout_secs,
            result_path: self.result_path.clone(),
            max_output_bytes: self.max_output_bytes,
            bundle: self.bundle.clone(),
            run_id: self.run_id.clone(),
            accelerator: self.accelerator.clone(),
            services: self.services.clone(),
        }
    }

    /// Why this spec cannot run, if it cannot.
    pub fn invalid(&self) -> Option<String> {
        if self.job_id.trim().is_empty() || self.job_id.len() > 200 {
            return Some("a job needs an id of 1 to 200 characters".into());
        }
        if self.attempt == 0 {
            return Some("attempts count from 1".into());
        }
        if self.argv.is_empty() || self.argv[0].trim().is_empty() {
            return Some("a job needs a command (argv)".into());
        }
        if self.bundle.len() > MAX_BUNDLE {
            return Some(format!(
                "a job's bundle is at most {MAX_BUNDLE} bytes (this one is {})",
                self.bundle.len()
            ));
        }
        if self.kind == KIND_PROCESS {
            return Some(process_refused());
        }
        if self.run_id.len() > 200 {
            return Some("a run id is at most 200 characters".into());
        }
        None
    }
}

/// `auto` (any case) and blank are the empty word.
pub fn normalize_auto(s: &str) -> String {
    let t = s.trim().to_ascii_lowercase();
    if t == "auto" { String::new() } else { t }
}

/// The name of the Space a job attempt runs in: unique per job and
/// attempt, a valid local Space name, and recognizable (`cj-`) so a host
/// can find the ones a crash left behind in its ledger.
pub fn space_name(job_id: &str, attempt: u32) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(job_id.as_bytes());
    format!("{JOB_SPACE_PREFIX}{}-{attempt}", &hex::encode(digest)[..16])
}

/// The name prefix of every job Space ([`space_name`]).
pub const JOB_SPACE_PREFIX: &str = "cj-";

/// Whether Space `id` (`local:<name>` or a bare name) runs a cluster job.
pub fn is_job_space(id: &str) -> bool {
    let name = id.strip_prefix("local:").unwrap_or(id);
    name.strip_prefix(JOB_SPACE_PREFIX).is_some_and(|rest| {
        let mut parts = rest.splitn(2, '-');
        let hash = parts.next().unwrap_or("");
        let attempt = parts.next().unwrap_or("");
        hash.len() == 16
            && hash.bytes().all(|b| b.is_ascii_hexdigit())
            && !attempt.is_empty()
            && attempt.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Keeps the last `max` bytes of `data`; says whether it cut.
pub fn tail(mut data: Vec<u8>, max: usize) -> (Vec<u8>, bool) {
    if data.len() <= max {
        return (data, false);
    }
    let cut = data.len() - max;
    data.drain(..cut);
    (data, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_and_round_trip() {
        let p = pb::HostJobSpec {
            job_id: "c1-7".into(),
            attempt: 2,
            kind: "AUTO".into(),
            argv: vec!["true".into()],
            ..Default::default()
        };
        let s = JobSpec::from_pb(&p);
        assert_eq!(s.kind, "");
        assert_eq!(s.millicpus, DEFAULT_MILLICPUS);
        assert_eq!(s.memory_mb, DEFAULT_MEMORY_MB);
        assert_eq!(s.timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(JobSpec::from_pb(&s.to_pb()), s);
        assert_eq!(s.invalid(), None);
        assert!(
            JobSpec {
                attempt: 0,
                ..s.clone()
            }
            .invalid()
            .is_some()
        );
        assert!(
            JobSpec {
                argv: vec![],
                ..s.clone()
            }
            .invalid()
            .is_some()
        );
    }

    #[test]
    fn space_names_are_stable_per_job_and_attempt() {
        let a = space_name("c1-7", 1);
        assert_eq!(a, space_name("c1-7", 1));
        assert_ne!(a, space_name("c1-7", 2));
        assert_ne!(a, space_name("c1-8", 1));
        assert!(a.starts_with("cj-"));
        assert!(
            a.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
        assert!(is_job_space(&a) && is_job_space(&format!("local:{a}")));
        assert!(!is_job_space("cj-notahash-1") && !is_job_space("local:research"));
    }

    #[test]
    fn accelerators_match_by_vendor() {
        let mac = vec!["apple:M2 Max:38-core GPU:96 GB".to_string()];
        let l4 = vec!["nvidia:L4:24 GB".to_string()];
        assert!(accelerator_fits("", &[]));
        assert!(!accelerator_fits("gpu", &[]));
        assert!(accelerator_fits("gpu", &mac) && accelerator_fits("gpu", &l4));
        assert!(accelerator_fits("apple", &mac) && !accelerator_fits("apple", &l4));
        assert!(accelerator_fits("nvidia", &l4) && !accelerator_fits("nvidia", &mac));
        assert_eq!(normalize_accelerator("CUDA").unwrap(), "nvidia");
        assert_eq!(normalize_accelerator(" ").unwrap(), "");
        assert!(normalize_accelerator("tpu").is_err());
    }

    #[test]
    fn bundles_are_small_and_process_is_refused() {
        let s = JobSpec {
            job_id: "j".into(),
            attempt: 1,
            argv: vec!["true".into()],
            bundle: vec![1; 10],
            run_id: "r1".into(),
            ..Default::default()
        };
        assert_eq!(s.invalid(), None);
        assert_eq!(JobSpec::from_pb(&s.to_pb()).bundle, s.bundle);
        assert_eq!(JobSpec::from_pb(&s.to_pb()).run_id, "r1");
        // The accelerator reaches the host (it sizes the member's Space and
        // gives it the GPU), spelled canonically.
        let gpu = JobSpec {
            accelerator: "gpu".into(),
            ..s.clone()
        };
        assert_eq!(JobSpec::from_pb(&gpu.to_pb()).accelerator, "gpu");
        let mut p = s.to_pb();
        p.accelerator = "CUDA".into();
        assert_eq!(JobSpec::from_pb(&p).accelerator, "nvidia");
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<JobSpec>(&json).unwrap(), s);
        assert!(
            JobSpec {
                bundle: vec![0; MAX_BUNDLE + 1],
                ..s.clone()
            }
            .invalid()
            .is_some()
        );
        assert_eq!(
            JobSpec {
                kind: KIND_PROCESS.into(),
                ..s.clone()
            }
            .invalid(),
            Some(process_refused())
        );
        for kind in ["", "vm", "container", "microvm", KIND_MACHINE] {
            assert_eq!(
                JobSpec {
                    kind: kind.into(),
                    ..s.clone()
                }
                .invalid(),
                None,
                "{kind}"
            );
        }
    }

    #[test]
    fn tails_keep_the_end() {
        assert_eq!(tail(b"abcdef".to_vec(), 3), (b"def".to_vec(), true));
        assert_eq!(tail(b"ab".to_vec(), 3), (b"ab".to_vec(), false));
    }
}
