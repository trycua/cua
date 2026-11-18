//! The hosted Cua CI service's wire types (`/v1/ci`, protocol 1): the
//! contract between this computer's daemon or CLI and the service. The
//! contract lives in `ci-contract/CONTRACT.md` of the team's work folder;
//! this module is its Rust form, with the validation both sides share.
//!
//! Money is integer micro-dollars, timestamps are RFC 3339 strings, ids are
//! opaque strings. Unknown response fields are ignored; request bodies
//! carry only the fields below.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::secret::Secret;

/// The protocol version sent as `Cua-CI-Protocol`.
pub const PROTOCOL: u32 = 1;

/// The label every Cua CI runner carries and a workflow's `runs-on` names.
pub const RUNS_ON_LABEL: &str = "cua";

/// Labels that narrow a request (besides `cua`).
pub const EXTRA_LABELS: [&str; 5] = [
    "cua-macos",
    "cua-linux",
    "cua-arm64",
    "cua-x64",
    "cua-large",
];

/// The longest a claim call may hold, in seconds.
pub const MAX_CLAIM_WAIT_S: u32 = 20;

/// The exact line a workflow needs.
pub fn runs_on_line() -> String {
    format!("runs-on: {RUNS_ON_LABEL}")
}

// ---------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------

/// An error body: `{"error": "<code>", "detail": "<text>", ...}`. The
/// nested form `{"error": {"code", "message"}}` is accepted too.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApiError {
    /// HTTP status.
    pub status: u16,
    /// The computer-readable code (`budget_exhausted`, `lease_taken`, ...).
    pub code: String,
    /// Plain words fit to show a user.
    pub detail: String,
    /// Seconds to wait before retrying (429, 502, 503).
    pub retry_after_s: Option<u64>,
    /// Structured detail (`field`, `reason`, `min_client`).
    pub fields: BTreeMap<String, serde_json::Value>,
}

impl ApiError {
    /// Parses an error response's body (anything unparseable keeps the
    /// text as the detail).
    pub fn parse(status: u16, body: &str, retry_after_header: Option<u64>) -> Self {
        let mut e = ApiError {
            status,
            retry_after_s: retry_after_header,
            ..Default::default()
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
            e.detail = body.trim().chars().take(300).collect();
            return e;
        };
        match v.get("error") {
            Some(serde_json::Value::String(code)) => {
                e.code = code.clone();
                e.detail = v
                    .get("detail")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .to_string();
            }
            Some(serde_json::Value::Object(o)) => {
                e.code = o
                    .get("code")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string();
                e.detail = o
                    .get("message")
                    .and_then(|c| c.as_str())
                    .unwrap_or_default()
                    .to_string();
            }
            _ => {}
        }
        if let Some(n) = v.get("retry_after_s").and_then(|n| n.as_u64()) {
            e.retry_after_s = Some(n);
        }
        if let Some(o) = v.get("fields").and_then(|f| f.as_object()) {
            e.fields = o.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        }
        e
    }

    /// Whether the call is worth retrying (rate limit, GitHub or service
    /// trouble).
    pub fn retryable(&self) -> bool {
        matches!(self.status, 429 | 502 | 503)
    }

    /// `fields.reason`, when the service gave one.
    pub fn reason(&self) -> Option<&str> {
        self.fields.get("reason").and_then(|r| r.as_str())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.detail.is_empty() {
            write!(f, "Cua CI service: {} ({})", self.code, self.status)
        } else {
            write!(f, "{}", self.detail)
        }
    }
}

// ---------------------------------------------------------------------
// Account and install
// ---------------------------------------------------------------------

/// `GET|POST /v1/ci/account`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Account {
    /// `acct_...`.
    pub account_id: String,
    /// `personal` or `org`.
    #[serde(default)]
    pub kind: String,
    /// The Cua organization, for `org`.
    #[serde(default)]
    pub org_id: Option<String>,
    /// CI is enabled for the account.
    #[serde(default)]
    pub enabled: bool,
    /// The linked GitHub installations.
    #[serde(default)]
    pub installations: Vec<Installation>,
    /// Computers registered.
    #[serde(default)]
    pub machines: u32,
}

/// A linked GitHub installation of the hosted App.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Installation {
    /// `inst_...`.
    pub id: String,
    /// GitHub's numeric id.
    #[serde(default)]
    pub github_id: String,
    /// The GitHub organization.
    #[serde(default)]
    pub github_login: String,
    /// `Organization` (v1) or `User`.
    #[serde(default)]
    pub github_account_type: String,
    /// `selected` or `all`.
    #[serde(default)]
    pub repos_selection: String,
    /// Suspended at GitHub.
    #[serde(default)]
    pub suspended: bool,
}

/// `POST /v1/ci/install/start` body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallStart {
    /// The CLI's loopback port, so the browser can return to it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loopback_port: Option<u16>,
    /// A hint for the GitHub organization to install on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github_login: Option<String>,
}

/// `POST /v1/ci/install/start` response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallSession {
    /// `ins_...`.
    pub session_id: String,
    /// The GitHub page to open.
    pub url: String,
    /// The App's slug.
    #[serde(default)]
    pub app_slug: String,
    /// When the session ends.
    #[serde(default)]
    pub expires_at: String,
    /// Seconds between polls.
    #[serde(default = "default_poll")]
    pub poll_interval_s: u32,
}

fn default_poll() -> u32 {
    2
}

/// Where an install session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallStatus {
    /// The user has not finished on GitHub.
    Pending,
    /// Linked.
    Installed,
    /// The user cancelled, or the account type is not supported.
    Denied,
    /// Too late.
    Expired,
}

/// `GET /v1/ci/install/{session_id}`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InstallState {
    /// The session.
    #[serde(default)]
    pub session_id: String,
    /// Its status.
    pub status: InstallStatus,
    /// Why it was denied.
    #[serde(default)]
    pub error: Option<String>,
    /// The installation, when installed.
    #[serde(default)]
    pub installation: Option<Installation>,
}

// ---------------------------------------------------------------------
// Computers and jobs
// ---------------------------------------------------------------------

/// `macos` or `linux`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Class {
    /// A macOS VM.
    Macos,
    /// A Linux container or VM.
    Linux,
}

impl Class {
    /// `macos` or `linux`.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Macos => "macos",
            Class::Linux => "linux",
        }
    }
}

/// What a workflow asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestedClass {
    /// Bare `cua`: the computer's free class decides.
    Any,
    /// `cua-macos`.
    Macos,
    /// `cua-linux`.
    Linux,
}

/// One class a computer serves, in `PUT /v1/ci/computers/{id}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineClass {
    /// The class.
    pub class: Class,
    /// `arm64` or `x64`.
    pub arch: String,
    /// At most this many jobs at once.
    pub max_slots: u32,
    /// vCPUs per job.
    #[serde(default)]
    pub cpus: Option<u32>,
    /// Memory per job in MiB.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// Disk per job in GiB.
    #[serde(default)]
    pub disk_gb: Option<u32>,
    /// Idle Spaces kept ready.
    #[serde(default)]
    pub warm_target: u32,
    /// The image jobs start from.
    #[serde(default)]
    pub image: Option<String>,
}

/// `local` (a linked computer) or `byoc` (a cloud pool).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineKind {
    /// The user's own computer.
    Local,
    /// A BYOC cloud pool.
    Byoc,
}

/// The BYOC part of a registration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByocInfo {
    /// `aws`, `gcp`, `azure`, `modal`, `hetzner`, ...
    pub provider: String,
    /// The region.
    #[serde(default)]
    pub region: String,
    /// At most this many jobs at once.
    pub max_slots: u32,
    /// The pool shrinks to nothing when idle.
    #[serde(default = "yes")]
    pub scale_to_zero: bool,
}

fn yes() -> bool {
    true
}

/// A computer's cost for budget estimates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineCost {
    /// List cost per slot hour in micro-dollars (0 for the user's own).
    #[serde(default)]
    pub per_slot_hour_micros: u64,
}

/// `PUT /v1/ci/computers/{machine_id}` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRegistration {
    /// A name people recognise.
    pub name: String,
    /// Local or BYOC.
    pub kind: MachineKind,
    /// `macos` or `linux` (the host).
    pub os: String,
    /// `arm64` or `x64` (the host).
    pub arch: String,
    /// The client's version.
    pub client_version: String,
    /// The classes served.
    pub classes: Vec<MachineClass>,
    /// BYOC details.
    #[serde(default)]
    pub byoc: Option<ByocInfo>,
    /// Cost.
    #[serde(default)]
    pub cost: MachineCost,
}

impl MachineRegistration {
    /// Checks the registration the way the service does.
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() || self.name.len() > 100 {
            return Err("computer name: 1 to 100 characters".into());
        }
        if self.classes.is_empty() {
            return Err("a computer serves at least one class".into());
        }
        let mut seen = Vec::new();
        for c in &self.classes {
            if c.max_slots == 0 || c.max_slots > 256 {
                return Err(format!("{}: max_slots is 1 to 256", c.class.as_str()));
            }
            if c.class == Class::Macos && self.kind == MachineKind::Local && c.max_slots > 2 {
                return Err("macOS: at most 2 jobs at once on one Mac (Apple's limit)".into());
            }
            if !matches!(c.arch.as_str(), "arm64" | "x64") {
                return Err(format!("{}: arch is arm64 or x64", c.class.as_str()));
            }
            if seen.contains(&(c.class, c.arch.clone())) {
                return Err(format!("{} {} twice", c.class.as_str(), c.arch));
            }
            seen.push((c.class, c.arch.clone()));
        }
        match (self.kind, &self.byoc) {
            (MachineKind::Byoc, None) => Err("a byoc computer needs its byoc details".into()),
            (MachineKind::Local, Some(_)) => Err("only a byoc computer has byoc details".into()),
            _ => Ok(()),
        }
    }
}

/// The reply to a registration.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MachineRegistered {
    /// The computer.
    #[serde(default)]
    pub machine_id: String,
    /// The account serving it.
    #[serde(default)]
    pub account_id: String,
    /// When.
    #[serde(default)]
    pub registered_at: String,
    /// How long a claim call may hold.
    #[serde(default = "default_claim_wait")]
    pub claim_wait_s: u32,
    /// The service will send it jobs.
    #[serde(default)]
    pub serving: bool,
    /// Drained by an administrator.
    #[serde(default)]
    pub paused: bool,
}

fn default_claim_wait() -> u32 {
    MAX_CLAIM_WAIT_S
}

/// Slots of one class in a claim call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassSlots {
    /// The class.
    pub class: Class,
    /// `arm64` or `x64`.
    pub arch: String,
    /// Free now.
    pub free_slots: u32,
    /// Running a job.
    pub busy_slots: u32,
    /// Idle Spaces kept ready.
    #[serde(default)]
    pub warm_slots: u32,
}

/// A lease the computer holds, in a claim call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseState {
    /// `lease_...`.
    pub lease_id: String,
    /// `job_...`.
    pub job_id: String,
    /// `booting`, `runner_online`, `running`.
    pub state: String,
}

/// `POST /v1/ci/claim` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimRequest {
    /// This computer.
    pub machine_id: String,
    /// Slots by class.
    pub classes: Vec<ClassSlots>,
    /// Leases held.
    #[serde(default)]
    pub leases: Vec<LeaseState>,
    /// How long to hold the call (at most [`MAX_CLAIM_WAIT_S`]).
    pub wait_s: u32,
}

impl ClaimRequest {
    /// Checks the body.
    pub fn validate(&self) -> Result<(), String> {
        if self.machine_id.trim().is_empty() {
            return Err("machine_id is needed".into());
        }
        if self.wait_s > MAX_CLAIM_WAIT_S {
            return Err(format!("wait_s is at most {MAX_CLAIM_WAIT_S}"));
        }
        Ok(())
    }
}

/// What a job's runner may do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobPolicy {
    /// Repository secrets may reach the job (false for fork pull requests).
    pub secrets: bool,
    /// `rw`, `ro` or `off`.
    pub cache: CacheAccess,
    /// `deny-private` or `off`.
    pub egress: String,
}

impl JobPolicy {
    /// The safe policy of a fork pull request.
    pub fn restricted() -> Self {
        Self {
            secrets: false,
            cache: CacheAccess::Ro,
            egress: "deny-private".into(),
        }
    }
}

/// Cache access of a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAccess {
    /// Read and write (default branch, same-repository pushes).
    Rw,
    /// Read only (pull requests, forks).
    Ro,
    /// None.
    Off,
}

/// Budget left, with a claimed job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetLeft {
    /// Micro-dollars left this month (`None`: no budget).
    #[serde(default)]
    pub remaining_micros: Option<i64>,
    /// `YYYY-MM`.
    #[serde(default)]
    pub month: String,
}

/// A job the service assigned to this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimedJob {
    /// `job_...`.
    pub job_id: String,
    /// `lease_...`.
    pub lease_id: String,
    /// When the lease ends unless the runner is online.
    #[serde(default)]
    pub lease_expires_at: String,
    /// `<run id>/<job id>` at GitHub.
    pub job_key: String,
    /// `owner/name`.
    pub repo: String,
    /// The workflow event.
    #[serde(default)]
    pub event: String,
    /// The ref.
    #[serde(default, rename = "ref")]
    pub git_ref: String,
    /// The commit.
    #[serde(default)]
    pub sha: String,
    /// The repository's default branch (empty: the service did not say).
    /// Only a job on this branch may write caches.
    #[serde(default)]
    pub default_branch: String,
    /// The repository is private (absent: not said; caches then treat the
    /// repository as private).
    #[serde(default)]
    pub private: Option<bool>,
    /// A pull request's source repository `owner/name` (a fork's own name;
    /// empty when not a pull request or not said).
    #[serde(default)]
    pub head_repo: String,
    /// What the workflow asked for.
    pub requested_class: RequestedClass,
    /// What this computer serves it as.
    pub class: Class,
    /// `arm64` or `x64`.
    pub arch: String,
    /// `default` or `large`.
    #[serde(default = "default_size")]
    pub size: String,
    /// The runner's labels.
    pub labels: Vec<String>,
    /// The runner's name (the service chose it).
    pub runner_name: String,
    /// A pull request from a fork.
    #[serde(default)]
    pub fork: bool,
    /// Served under the `restricted` fork policy.
    #[serde(default)]
    pub restricted: bool,
    /// What the job may do.
    pub policy: JobPolicy,
    /// When GitHub queued it.
    #[serde(default)]
    pub queued_at: String,
    /// Budget left.
    #[serde(default)]
    pub budget: BudgetLeft,
}

fn default_size() -> String {
    "default".into()
}

impl ClaimedJob {
    /// The checks a daemon makes before it starts anything: a fork's job
    /// must not carry secrets or a writable cache, whatever else the body
    /// says.
    pub fn check(&self) -> Result<(), String> {
        if !self.repo.contains('/') {
            return Err(format!("repo {:?} is not owner/name", self.repo));
        }
        if self.runner_name.is_empty() || self.runner_name.len() > 64 {
            return Err("runner_name: 1 to 64 characters".into());
        }
        if (self.fork || self.restricted)
            && (self.policy.secrets || self.policy.cache != CacheAccess::Ro)
        {
            return Err(
                "a fork pull request must run without secrets and with a read-only cache".into(),
            );
        }
        if self.labels.iter().any(|l| l.contains(',') || l.is_empty()) {
            return Err("labels: no empty labels or commas".into());
        }
        Ok(())
    }
}

/// A lease's instruction.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseAction {
    /// `lease_...`.
    pub lease_id: String,
    /// `continue` or `abort`.
    pub action: String,
}

/// `POST /v1/ci/claim` response.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimResponse {
    /// Jobs assigned now.
    #[serde(default)]
    pub jobs: Vec<ClaimedJob>,
    /// What to do with the leases held.
    #[serde(default)]
    pub leases: Vec<LeaseAction>,
    /// An administrator drained this computer.
    #[serde(default)]
    pub paused: bool,
}

/// `POST /v1/ci/jobs/{job_id}/jit` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct JitRequest {
    /// The lease.
    pub lease_id: String,
    /// This computer.
    pub machine_id: String,
}

/// `POST /v1/ci/jobs/{job_id}/jit` response. The config is a secret: it
/// prints as `[redacted]` and goes straight into the Space.
#[derive(Clone, Debug)]
pub struct JitReply {
    /// The runner's one-job configuration.
    pub encoded_jit_config: Secret,
    /// GitHub's runner id.
    pub runner_id: u64,
    /// The runner's name.
    pub runner_name: String,
    /// Its labels.
    pub labels: Vec<String>,
    /// When the configuration stops working.
    pub expires_at: String,
}

impl<'de> Deserialize<'de> for JitReply {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            encoded_jit_config: String,
            runner_id: u64,
            runner_name: String,
            #[serde(default)]
            labels: Vec<String>,
            #[serde(default)]
            expires_at: String,
        }
        let r = Raw::deserialize(d)?;
        Ok(JitReply {
            encoded_jit_config: Secret::new(r.encoded_jit_config),
            runner_id: r.runner_id,
            runner_name: r.runner_name,
            labels: r.labels,
            expires_at: r.expires_at,
        })
    }
}

/// What a computer reports about a job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportState {
    /// The Space is starting.
    Booting,
    /// The runner is listening.
    RunnerOnline,
    /// The job's first step began.
    Running,
    /// The job ended.
    Finished,
    /// The job or its infrastructure failed.
    Failed,
    /// This computer cannot serve it any more.
    Released,
}

/// Phase timers in milliseconds.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phases {
    /// Job queued to VM claimed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm_claim_ms: Option<u64>,
    /// VM claimed to ready.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vm_boot_ms: Option<u64>,
    /// VM ready to runner registered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_register_ms: Option<u64>,
    /// Runner registered to job assigned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign_ms: Option<u64>,
    /// Job assigned to first step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_start_ms: Option<u64>,
    /// Checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkout_ms: Option<u64>,
    /// Job finished to Space gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teardown_ms: Option<u64>,
}

/// Cache counters of a job.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheStats {
    /// Restores that hit.
    #[serde(default)]
    pub hits: u32,
    /// Restores that missed.
    #[serde(default)]
    pub misses: u32,
    /// Bytes restored.
    #[serde(default)]
    pub bytes_restored: u64,
}

/// What a finished or failed job used.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// The VM started.
    pub vm_started_at: String,
    /// The job's first step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_started_at: Option<String>,
    /// The job ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_ended_at: Option<String>,
    /// The VM was gone.
    pub vm_ended_at: String,
    /// The class it ran as.
    pub class: Class,
    /// `arm64` or `x64`.
    pub arch: String,
    /// vCPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Memory in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    /// `warm_pool`, `fork` or `cold_create`.
    pub start_mode: String,
    /// What a BYOC computer measured; absent for local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_micros: Option<u64>,
    /// Phase timers.
    #[serde(default)]
    pub phases: Phases,
    /// Cache counters.
    #[serde(default)]
    pub cache: CacheStats,
}

/// `POST /v1/ci/jobs/{job_id}/report` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    /// The lease.
    pub lease_id: String,
    /// This computer.
    pub machine_id: String,
    /// The state.
    pub state: ReportState,
    /// When.
    pub at: String,
    /// `success`, `failure` or `cancelled`, for finished jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    /// The failure was the infrastructure's, not the job's.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub infra: bool,
    /// Plain words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// For finished and failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

impl Report {
    /// Checks the body: finished and failed carry usage, `result` only
    /// goes with finished.
    pub fn validate(&self) -> Result<(), String> {
        match self.state {
            ReportState::Finished => {
                if self.usage.is_none() {
                    return Err("a finished report carries usage".into());
                }
                if !matches!(
                    self.result.as_deref(),
                    Some("success" | "failure" | "cancelled")
                ) {
                    return Err("result is success, failure or cancelled".into());
                }
            }
            ReportState::Failed => {
                if self.usage.is_none() {
                    return Err("a failed report carries usage".into());
                }
            }
            _ => {
                if self.result.is_some() || self.usage.is_some() {
                    return Err("result and usage go with finished or failed".into());
                }
            }
        }
        if self.infra && self.state != ReportState::Failed {
            return Err("infra goes with failed".into());
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------
// Repositories and settings
// ---------------------------------------------------------------------

/// How fork pull requests are treated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkPrPolicy {
    /// Served without secrets, with read-only caches and the egress
    /// policy.
    #[default]
    Restricted,
    /// Never served.
    Deny,
    /// Treated like a same-repository job (GitHub still sends no secrets).
    Allow,
}

impl ForkPrPolicy {
    /// Parses `restricted`, `deny` or `allow`.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "restricted" => Ok(Self::Restricted),
            "deny" => Ok(Self::Deny),
            "allow" => Ok(Self::Allow),
            other => Err(format!(
                "fork policy {other:?}: use restricted, deny or allow"
            )),
        }
    }

    /// The word.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Restricted => "restricted",
            Self::Deny => "deny",
            Self::Allow => "allow",
        }
    }
}

/// One repository in `GET /v1/ci/repos`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRow {
    /// `owner/name`.
    pub full_name: String,
    /// Private repository.
    #[serde(default)]
    pub private: bool,
    /// Default branch.
    #[serde(default)]
    pub default_branch: String,
    /// The App can see it.
    #[serde(default)]
    pub visible: bool,
    /// Cua serves its jobs.
    #[serde(default)]
    pub allowed: bool,
    /// A per-repository override.
    #[serde(default)]
    pub fork_pr_policy: Option<ForkPrPolicy>,
    /// A per-repository concurrency cap.
    #[serde(default)]
    pub max_concurrent: Option<u32>,
}

/// `GET /v1/ci/repos`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repos {
    /// The installation.
    #[serde(default)]
    pub installation: String,
    /// `selected` or `all`.
    #[serde(default)]
    pub repos_selection: String,
    /// The repositories.
    #[serde(default)]
    pub repos: Vec<RepoRow>,
    /// More.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A per-repository override in `PUT /v1/ci/repos`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoOverride {
    /// Fork policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fork_pr_policy: Option<ForkPrPolicy>,
    /// Concurrency cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent: Option<u32>,
}

/// `PUT /v1/ci/repos` body.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllowList {
    /// The installation.
    pub installation: String,
    /// The repositories served (`owner/name`; `owner/*` only with the
    /// org-wide flag).
    pub allow: Vec<String>,
    /// Overrides by repository.
    #[serde(default)]
    pub overrides: BTreeMap<String, RepoOverride>,
}

/// A repository name as `owner/name`, or `owner/*` when `pattern`.
pub fn parse_repo(s: &str, pattern: bool) -> Result<String, String> {
    let s = s.trim();
    let Some((owner, name)) = s.split_once('/') else {
        return Err(format!("{s:?}: use owner/name"));
    };
    let ok = |p: &str| {
        !p.is_empty()
            && p.len() <= 100
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if !ok(owner) {
        return Err(format!("{s:?}: the owner is letters, digits, - _ ."));
    }
    if name == "*" {
        return if pattern {
            Ok(format!("{owner}/*"))
        } else {
            Err(format!(
                "{s:?}: every repository of {owner} needs --allow-org-wide"
            ))
        };
    }
    if !ok(name) || name.contains('/') {
        return Err(format!("{s:?}: the name is letters, digits, - _ ."));
    }
    Ok(format!("{owner}/{name}"))
}

impl AllowList {
    /// Checks the list (and the overrides name listed repositories).
    pub fn validate(&self, pattern: bool) -> Result<(), String> {
        if self.installation.is_empty() {
            return Err("an installation is needed".into());
        }
        let mut seen = std::collections::BTreeSet::new();
        for r in &self.allow {
            let n = parse_repo(r, pattern)?;
            if !seen.insert(n.to_lowercase()) {
                return Err(format!("{r}: listed twice"));
            }
        }
        for (r, o) in &self.overrides {
            if !seen.contains(&r.to_lowercase()) {
                return Err(format!(
                    "{r}: an override for a repository that is not allowed"
                ));
            }
            if o.max_concurrent == Some(0) {
                return Err(format!("{r}: max_concurrent is at least 1"));
            }
        }
        Ok(())
    }
}

/// The settings' defaults.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsDefaults {
    /// Fork policy.
    #[serde(default)]
    pub fork_pr_policy: ForkPrPolicy,
    /// `deny-private` or `off`.
    #[serde(default = "default_egress")]
    pub egress: String,
}

fn default_egress() -> String {
    "deny-private".into()
}

impl Default for SettingsDefaults {
    fn default() -> Self {
        Self {
            fork_pr_policy: ForkPrPolicy::Restricted,
            egress: default_egress(),
        }
    }
}

/// Concurrency and time caps.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Caps {
    /// Jobs at once, all repositories.
    #[serde(default)]
    pub max_concurrent_jobs: Option<u32>,
    /// macOS jobs at once.
    #[serde(default)]
    pub max_concurrent_macos: Option<u32>,
    /// A job running longer is aborted.
    #[serde(default)]
    pub max_job_minutes: Option<u32>,
    /// A job with no capacity for this long is marked unserved.
    #[serde(default)]
    pub queue_timeout_s: Option<u32>,
}

/// `stop` or `warn`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnLimit {
    /// No new jobs go to BYOC computers.
    #[default]
    Stop,
    /// Only warn.
    Warn,
}

/// What the budget counts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetScope {
    /// The cost BYOC computers report.
    #[default]
    Byoc,
    /// Plus the list cost of local minutes.
    All,
}

/// The monthly budget.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Budget {
    /// Micro-dollars a month (`None`: none set).
    #[serde(default)]
    pub monthly_micros: Option<u64>,
    /// Fractions of the budget that warn.
    #[serde(default = "default_warn")]
    pub warn_fractions: Vec<f64>,
    /// What happens at the limit.
    #[serde(default)]
    pub on_limit: OnLimit,
    /// What counts.
    #[serde(default)]
    pub scope: BudgetScope,
}

fn default_warn() -> Vec<f64> {
    vec![0.5, 0.8]
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            monthly_micros: None,
            warn_fractions: default_warn(),
            on_limit: OnLimit::Stop,
            scope: BudgetScope::Byoc,
        }
    }
}

/// `GET|PUT /v1/ci/settings`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// Version of this document (1).
    #[serde(default = "one")]
    pub version: u32,
    /// Defaults.
    #[serde(default)]
    pub defaults: SettingsDefaults,
    /// Caps.
    #[serde(default)]
    pub caps: Caps,
    /// The budget.
    #[serde(default)]
    pub budget: Budget,
    /// When last changed.
    #[serde(default, skip_serializing)]
    pub updated_at: String,
}

fn one() -> u32 {
    1
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            defaults: SettingsDefaults::default(),
            caps: Caps::default(),
            budget: Budget::default(),
            updated_at: String::new(),
        }
    }
}

impl Settings {
    /// Checks the way the service does.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err(format!(
                "settings version {} is not supported",
                self.version
            ));
        }
        if !matches!(self.defaults.egress.as_str(), "deny-private" | "off") {
            return Err("egress is deny-private or off".into());
        }
        for (name, v) in [
            ("max_concurrent_jobs", self.caps.max_concurrent_jobs),
            ("max_concurrent_macos", self.caps.max_concurrent_macos),
            ("max_job_minutes", self.caps.max_job_minutes),
            ("queue_timeout_s", self.caps.queue_timeout_s),
        ] {
            if v == Some(0) {
                return Err(format!("{name} is at least 1 (leave it unset for no cap)"));
            }
        }
        if let (Some(all), Some(mac)) = (
            self.caps.max_concurrent_jobs,
            self.caps.max_concurrent_macos,
        ) && mac > all
        {
            return Err("max_concurrent_macos is more than max_concurrent_jobs".into());
        }
        if self.budget.warn_fractions.is_empty()
            || self
                .budget
                .warn_fractions
                .iter()
                .any(|f| !(*f > 0.0 && *f <= 1.0))
        {
            return Err("warning fractions are between 0 (exclusive) and 1".into());
        }
        Ok(())
    }
}

/// Dollars as micro-dollars (`"50"`, `"12.50"`, `"$0.25"`).
pub fn parse_dollars(s: &str) -> Result<u64, String> {
    let t = s.trim().trim_start_matches('$');
    let bad = || format!("{s:?}: a dollar amount such as 50 or 12.50");
    if t.is_empty() {
        return Err(bad());
    }
    let (whole, frac) = match t.split_once('.') {
        Some((w, f)) => (w, f),
        None => (t, ""),
    };
    if frac.len() > 6
        || !whole.chars().all(|c| c.is_ascii_digit())
        || !frac.chars().all(|c| c.is_ascii_digit())
        || (whole.is_empty() && frac.is_empty())
    {
        return Err(bad());
    }
    let w: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().map_err(|_| bad())?
    };
    let f: u64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<6}").parse().map_err(|_| bad())?
    };
    w.checked_mul(1_000_000)
        .and_then(|m| m.checked_add(f))
        .ok_or_else(bad)
}

/// Micro-dollars as `$12.50` (two decimals, or more under a cent).
pub fn format_dollars(micros: u64) -> String {
    if micros > 0 && micros < 10_000 {
        return format!("${:.4}", micros as f64 / 1_000_000.0);
    }
    format!("${:.2}", micros as f64 / 1_000_000.0)
}

// ---------------------------------------------------------------------
// Status, jobs, usage
// ---------------------------------------------------------------------

/// Last 7 days.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Last7d {
    /// Jobs.
    #[serde(default)]
    pub jobs: u64,
    /// Minutes.
    #[serde(default)]
    pub minutes: f64,
    /// Successful.
    #[serde(default)]
    pub success: u64,
    /// Failed.
    #[serde(default)]
    pub failure: u64,
    /// Cancelled.
    #[serde(default)]
    pub cancelled: u64,
    /// Infrastructure failures.
    #[serde(default)]
    pub infra_failures: u64,
    /// Median queue to first step, seconds.
    #[serde(default)]
    pub median_start_s: Option<f64>,
    /// 95th percentile, seconds.
    #[serde(default)]
    pub p95_start_s: Option<f64>,
}

/// This month.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MonthStatus {
    /// Minutes.
    #[serde(default)]
    pub minutes: f64,
    /// Cost the BYOC computers reported.
    #[serde(default)]
    pub cost_micros: u64,
    /// The budget.
    #[serde(default)]
    pub budget_micros: Option<u64>,
    /// Spent over budget.
    #[serde(default)]
    pub budget_fraction: Option<f64>,
    /// `ok`, `warn` or `exhausted`.
    #[serde(default)]
    pub budget_state: String,
}

/// A computer in `status`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineStatus {
    /// Id.
    #[serde(default)]
    pub machine_id: String,
    /// Name.
    #[serde(default)]
    pub name: String,
    /// `local` or `byoc`.
    #[serde(default)]
    pub kind: String,
    /// `online`, `offline` or `draining`.
    #[serde(default)]
    pub state: String,
    /// Slots by class.
    #[serde(default)]
    pub classes: Vec<StatusClass>,
    /// Last call.
    #[serde(default)]
    pub last_seen_at: String,
}

/// Slots in `status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusClass {
    /// `macos` or `linux`.
    pub class: String,
    /// Free.
    #[serde(default)]
    pub free_slots: u32,
    /// Busy.
    #[serde(default)]
    pub busy_slots: u32,
    /// At most.
    #[serde(default)]
    pub max_slots: u32,
}

/// A queued job in `status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedJob {
    /// `job_...`.
    #[serde(default)]
    pub job_id: String,
    /// `<run>/<job>`.
    #[serde(default)]
    pub job_key: String,
    /// `owner/name`.
    #[serde(default)]
    pub repo: String,
    /// Class.
    #[serde(default)]
    pub class: String,
    /// Queued at.
    #[serde(default)]
    pub queued_at: String,
    /// Seconds waiting.
    #[serde(default)]
    pub waiting_s: u64,
    /// `queued`, `offered` or `unserved`.
    #[serde(default)]
    pub state: String,
    /// Why (unserved).
    #[serde(default)]
    pub reason: Option<String>,
}

/// A running job in `status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunningJob {
    /// `<run>/<job>`.
    #[serde(default)]
    pub job_key: String,
    /// `owner/name`.
    #[serde(default)]
    pub repo: String,
    /// The computer.
    #[serde(default)]
    pub machine_id: String,
    /// Started.
    #[serde(default)]
    pub started_at: String,
    /// The runner.
    #[serde(default)]
    pub runner_name: String,
}

/// An installation in `status`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusInstallation {
    /// Id.
    #[serde(default)]
    pub id: String,
    /// GitHub organization.
    #[serde(default)]
    pub github_login: String,
    /// Repositories allowed.
    #[serde(default)]
    pub repos_allowed: u32,
}

/// `GET /v1/ci/status`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The account.
    #[serde(default)]
    pub account_id: String,
    /// Installations.
    #[serde(default)]
    pub installations: Vec<StatusInstallation>,
    /// Computers.
    #[serde(default)]
    pub machines: Vec<MachineStatus>,
    /// Waiting jobs.
    #[serde(default)]
    pub queue: Vec<QueuedJob>,
    /// Running jobs.
    #[serde(default)]
    pub running: Vec<RunningJob>,
    /// The last 7 days.
    #[serde(default)]
    pub last_7d: Last7d,
    /// This month.
    #[serde(default)]
    pub month: MonthStatus,
    /// Things worth saying.
    #[serde(default)]
    pub notices: Vec<String>,
}

/// A job in `GET /v1/ci/jobs`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobRow {
    /// `job_...`.
    #[serde(default)]
    pub job_id: String,
    /// `<run>/<job>`.
    #[serde(default)]
    pub job_key: String,
    /// `owner/name`.
    #[serde(default)]
    pub repo: String,
    /// Workflow name.
    #[serde(default)]
    pub workflow: String,
    /// Job name.
    #[serde(default)]
    pub job_name: String,
    /// Event.
    #[serde(default)]
    pub event: String,
    /// Class.
    #[serde(default)]
    pub class: String,
    /// The computer.
    #[serde(default)]
    pub machine_id: String,
    /// `queued`, `running` or `finished`.
    #[serde(default)]
    pub state: String,
    /// `success`, `failure` or `cancelled`.
    #[serde(default)]
    pub result: Option<String>,
    /// Queued at.
    #[serde(default)]
    pub queued_at: String,
    /// Minutes billed.
    #[serde(default)]
    pub minutes: Option<f64>,
    /// Queue to first step in milliseconds.
    #[serde(default)]
    pub start_latency_ms: Option<u64>,
    /// Attempts.
    #[serde(default)]
    pub attempts: u32,
}

/// A page of jobs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JobPage {
    /// The jobs.
    #[serde(default)]
    pub jobs: Vec<JobRow>,
    /// More.
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// A line of a job's runner log.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    /// When.
    #[serde(default)]
    pub at: String,
    /// `info`, `warn`, `error`.
    #[serde(default)]
    pub level: String,
    /// The text.
    #[serde(default)]
    pub text: String,
}

/// `GET /v1/ci/jobs/{job_id}/logs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogPage {
    /// Lines.
    #[serde(default)]
    pub lines: Vec<LogLine>,
    /// Pass back as `after`.
    #[serde(default)]
    pub next_seq: u64,
    /// The job is over and every line is here.
    #[serde(default)]
    pub complete: bool,
}

/// `POST /v1/ci/jobs/{job_id}/logs` body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogUpload {
    /// The lease.
    pub lease_id: String,
    /// This chunk's number.
    pub seq: u64,
    /// The lines (at most 2000, 1 MiB).
    pub lines: Vec<LogLine>,
}

impl LogUpload {
    /// The service's limits.
    pub fn validate(&self) -> Result<(), String> {
        if self.lines.len() > 2000 {
            return Err("at most 2000 lines a call".into());
        }
        let bytes: usize = self
            .lines
            .iter()
            .map(|l| l.text.len() + l.at.len() + 16)
            .sum();
        if bytes > 1 << 20 {
            return Err("at most 1 MiB a call".into());
        }
        Ok(())
    }
}

/// `GET /v1/ci/usage`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct UsageSummary {
    /// `YYYY-MM`.
    #[serde(default)]
    pub month: String,
    /// Jobs.
    #[serde(default)]
    pub jobs: u64,
    /// Minutes.
    #[serde(default)]
    pub minutes: f64,
    /// By class.
    #[serde(default)]
    pub by_class: BTreeMap<String, ClassUsage>,
    /// Infrastructure failures.
    #[serde(default)]
    pub infra_failures: u64,
    /// Rate of them.
    #[serde(default)]
    pub infra_failure_rate: f64,
}

/// One class in a usage summary.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ClassUsage {
    /// Jobs.
    #[serde(default)]
    pub jobs: u64,
    /// Minutes.
    #[serde(default)]
    pub minutes: f64,
    /// Reported cost.
    #[serde(default)]
    pub cost_micros: u64,
}

// ---------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------

/// What `labels` (a job's `runs-on`) asks for, or `None` when it is not
/// a Cua CI job. Mirrors the service's routing: it needs `cua` or an
/// extra, and every other label must be a GitHub-standard one.
pub fn resolve_labels(labels: &[String]) -> Option<(RequestedClass, bool)> {
    let l: Vec<String> = labels.iter().map(|s| s.to_ascii_lowercase()).collect();
    if !l
        .iter()
        .any(|x| x == RUNS_ON_LABEL || EXTRA_LABELS.contains(&x.as_str()))
    {
        return None;
    }
    let mut class = RequestedClass::Any;
    let mut large = false;
    for x in &l {
        match x.as_str() {
            "cua" | "self-hosted" | "arm64" | "x64" | "cua-arm64" | "cua-x64" => {}
            "macos" | "cua-macos" => class = RequestedClass::Macos,
            "linux" | "cua-linux" => class = RequestedClass::Linux,
            "cua-large" => large = true,
            _ => return None,
        }
    }
    Some((class, large))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_runs_on_line() {
        assert_eq!(runs_on_line(), "runs-on: cua");
    }

    #[test]
    fn both_error_shapes_parse() {
        let e = ApiError::parse(
            402,
            r#"{"error":"budget_exhausted","detail":"Budget reached","retry_after_s":3,"fields":{"reason":"x"}}"#,
            None,
        );
        assert_eq!(
            (e.code.as_str(), e.detail.as_str(), e.retry_after_s),
            ("budget_exhausted", "Budget reached", Some(3))
        );
        assert_eq!(e.reason(), Some("x"));
        assert_eq!(e.to_string(), "Budget reached");
        let e = ApiError::parse(
            429,
            r#"{"error":{"code":"rate_limited","message":"Slow down"}}"#,
            Some(7),
        );
        assert_eq!(
            (e.code.as_str(), e.retry_after_s),
            ("rate_limited", Some(7))
        );
        assert!(e.retryable());
        let e = ApiError::parse(500, "boom", None);
        assert_eq!(e.detail, "boom");
        assert!(!e.retryable());
    }

    fn job() -> serde_json::Value {
        serde_json::json!({
            "job_id": "job_1", "lease_id": "lease_1", "job_key": "1/2", "repo": "acme/app",
            "event": "push", "ref": "refs/heads/main", "sha": "abc",
            "requested_class": "any", "class": "macos", "arch": "arm64", "size": "default",
            "labels": ["self-hosted", "macOS", "ARM64", "cua"], "runner_name": "cua-mac-1",
            "fork": false, "restricted": false,
            "policy": {"secrets": true, "cache": "rw", "egress": "deny-private"},
            "queued_at": "2026-10-28T10:00:00Z",
            "budget": {"remaining_micros": 25000000, "month": "2026-10"},
            "extra_field_from_a_newer_service": 1
        })
    }

    #[test]
    fn a_claimed_job_parses_and_checks() {
        let j: ClaimedJob = serde_json::from_value(job()).unwrap();
        assert_eq!(j.class, Class::Macos);
        assert_eq!(j.requested_class, RequestedClass::Any);
        assert_eq!(j.git_ref, "refs/heads/main");
        assert_eq!(j.policy.cache, CacheAccess::Rw);
        j.check().unwrap();
    }

    #[test]
    fn a_fork_job_with_secrets_or_a_writable_cache_is_refused() {
        let mut j: ClaimedJob = serde_json::from_value(job()).unwrap();
        j.fork = true;
        assert!(j.check().unwrap_err().contains("without secrets"));
        j.policy = JobPolicy::restricted();
        j.check().unwrap();
        j.policy.secrets = true;
        assert!(j.check().is_err());
        let mut j: ClaimedJob = serde_json::from_value(job()).unwrap();
        j.restricted = true;
        assert!(j.check().is_err());
        j.repo = "nope".into();
        assert!(j.check().unwrap_err().contains("owner/name"));
    }

    #[test]
    fn the_jit_config_never_prints() {
        let r: JitReply = serde_json::from_str(
            r#"{"encoded_jit_config":"TOPSECRET","runner_id":4,"runner_name":"r","labels":["cua"],"expires_at":"x"}"#,
        )
        .unwrap();
        assert_eq!(r.encoded_jit_config.expose(), "TOPSECRET");
        let shown = format!("{r:?} {}", r.encoded_jit_config);
        assert!(!shown.contains("TOPSECRET"), "{shown}");
    }

    fn reg() -> MachineRegistration {
        MachineRegistration {
            name: "mac".into(),
            kind: MachineKind::Local,
            os: "macos".into(),
            arch: "arm64".into(),
            client_version: "0".into(),
            classes: vec![MachineClass {
                class: Class::Macos,
                arch: "arm64".into(),
                max_slots: 2,
                cpus: Some(5),
                memory_mb: None,
                disk_gb: None,
                warm_target: 1,
                image: None,
            }],
            byoc: None,
            cost: MachineCost::default(),
        }
    }

    #[test]
    fn registrations_are_validated() {
        reg().validate().unwrap();
        let mut r = reg();
        r.classes[0].max_slots = 3;
        assert!(r.validate().unwrap_err().contains("at most 2"));
        let mut r = reg();
        r.classes.clear();
        assert!(r.validate().is_err());
        let mut r = reg();
        r.kind = MachineKind::Byoc;
        assert!(r.validate().unwrap_err().contains("byoc details"));
        r.byoc = Some(ByocInfo {
            provider: "aws".into(),
            region: "us-east-1".into(),
            max_slots: 10,
            scale_to_zero: true,
        });
        r.classes[0].max_slots = 10;
        r.validate().unwrap();
        let mut r = reg();
        r.classes.push(r.classes[0].clone());
        assert!(r.validate().unwrap_err().contains("twice"));
        let mut r = reg();
        r.classes[0].arch = "ppc".into();
        assert!(r.validate().is_err());
    }

    #[test]
    fn claims_are_bounded() {
        let c = ClaimRequest {
            machine_id: "m".into(),
            classes: vec![],
            leases: vec![],
            wait_s: 20,
        };
        c.validate().unwrap();
        let mut c2 = c.clone();
        c2.wait_s = 21;
        assert!(c2.validate().is_err());
        c2.wait_s = 0;
        c2.machine_id = " ".into();
        assert!(c2.validate().is_err());
    }

    fn usage() -> Usage {
        Usage {
            vm_started_at: "a".into(),
            job_started_at: None,
            job_ended_at: None,
            vm_ended_at: "b".into(),
            class: Class::Linux,
            arch: "x64".into(),
            cpus: None,
            memory_mb: None,
            start_mode: "cold_create".into(),
            cost_micros: None,
            phases: Phases::default(),
            cache: CacheStats::default(),
        }
    }

    #[test]
    fn reports_carry_what_their_state_needs() {
        let mut r = Report {
            lease_id: "l".into(),
            machine_id: "m".into(),
            state: ReportState::Booting,
            at: "t".into(),
            result: None,
            infra: false,
            detail: None,
            usage: None,
        };
        r.validate().unwrap();
        r.state = ReportState::Finished;
        assert!(r.validate().is_err());
        r.usage = Some(usage());
        assert!(r.validate().is_err());
        r.result = Some("success".into());
        r.validate().unwrap();
        r.state = ReportState::Failed;
        r.result = None;
        r.infra = true;
        r.validate().unwrap();
        r.state = ReportState::Running;
        assert!(r.validate().is_err());
        // Wire form: absent options are left out.
        let mut r = Report {
            lease_id: "l".into(),
            machine_id: "m".into(),
            state: ReportState::RunnerOnline,
            at: "t".into(),
            result: None,
            infra: false,
            detail: None,
            usage: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["state"], "runner_online");
        assert!(v.get("usage").is_none() && v.get("infra").is_none());
        r.state = ReportState::Released;
        assert_eq!(serde_json::to_value(&r).unwrap()["state"], "released");
    }

    #[test]
    fn repos_parse_and_the_wildcard_needs_a_flag() {
        assert_eq!(parse_repo(" acme/app ", false).unwrap(), "acme/app");
        assert_eq!(parse_repo("acme/*", true).unwrap(), "acme/*");
        assert!(
            parse_repo("acme/*", false)
                .unwrap_err()
                .contains("--allow-org-wide")
        );
        assert!(parse_repo("*", true).is_err());
        assert!(parse_repo("acme", false).is_err());
        assert!(parse_repo("acme/a/b", false).is_err());
        assert!(parse_repo("ac me/app", false).is_err());
    }

    #[test]
    fn allow_lists_are_validated() {
        let mut a = AllowList {
            installation: "inst_1".into(),
            allow: vec!["acme/app".into(), "acme/api".into()],
            overrides: BTreeMap::new(),
        };
        a.validate(false).unwrap();
        a.allow.push("ACME/app".into());
        assert!(a.validate(false).unwrap_err().contains("twice"));
        a.allow.pop();
        a.overrides.insert(
            "acme/other".into(),
            RepoOverride {
                fork_pr_policy: Some(ForkPrPolicy::Deny),
                max_concurrent: None,
            },
        );
        assert!(a.validate(false).unwrap_err().contains("not allowed"));
        a.overrides.clear();
        a.overrides.insert(
            "acme/app".into(),
            RepoOverride {
                fork_pr_policy: None,
                max_concurrent: Some(0),
            },
        );
        assert!(a.validate(false).is_err());
        a.overrides.clear();
        a.installation.clear();
        assert!(a.validate(false).is_err());
    }

    #[test]
    fn settings_default_safe_and_validate() {
        let s = Settings::default();
        assert_eq!(s.defaults.fork_pr_policy, ForkPrPolicy::Restricted);
        assert_eq!(s.defaults.egress, "deny-private");
        assert_eq!(s.budget.monthly_micros, None);
        assert_eq!(s.budget.on_limit, OnLimit::Stop);
        assert_eq!(s.budget.scope, BudgetScope::Byoc);
        s.validate().unwrap();
        let mut b = s.clone();
        b.caps.max_concurrent_jobs = Some(0);
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.caps.max_concurrent_jobs = Some(2);
        b.caps.max_concurrent_macos = Some(3);
        assert!(b.validate().unwrap_err().contains("more than"));
        let mut b = s.clone();
        b.budget.warn_fractions = vec![0.0];
        assert!(b.validate().is_err());
        b.budget.warn_fractions = vec![1.5];
        assert!(b.validate().is_err());
        let mut b = s.clone();
        b.defaults.egress = "everything".into();
        assert!(b.validate().is_err());
        let mut b = s;
        b.version = 2;
        assert!(b.validate().is_err());
    }

    #[test]
    fn a_settings_document_round_trips_with_defaults_filled() {
        let s: Settings = serde_json::from_str(r#"{"caps":{"max_job_minutes":60}}"#).unwrap();
        assert_eq!(s.caps.max_job_minutes, Some(60));
        assert_eq!(s.defaults, SettingsDefaults::default());
        assert_eq!(s.budget.warn_fractions, vec![0.5, 0.8]);
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("updated_at").is_none());
        assert_eq!(v["defaults"]["fork_pr_policy"], "restricted");
    }

    #[test]
    fn fork_policy_words() {
        for w in ["restricted", "deny", "allow"] {
            assert_eq!(ForkPrPolicy::parse(w).unwrap().as_str(), w);
        }
        assert!(ForkPrPolicy::parse("open").is_err());
        assert_eq!(ForkPrPolicy::default(), ForkPrPolicy::Restricted);
    }

    #[test]
    fn dollars() {
        assert_eq!(parse_dollars("50").unwrap(), 50_000_000);
        assert_eq!(parse_dollars("$12.50").unwrap(), 12_500_000);
        assert_eq!(parse_dollars("0.000001").unwrap(), 1);
        assert_eq!(parse_dollars(".5").unwrap(), 500_000);
        for bad in ["", "-1", "1e3", "1.0000001", "abc", "$", "."] {
            assert!(parse_dollars(bad).is_err(), "{bad}");
        }
        assert_eq!(format_dollars(12_500_000), "$12.50");
        assert_eq!(format_dollars(4_200), "$0.0042");
        assert_eq!(format_dollars(0), "$0.00");
    }

    #[test]
    fn labels_route_like_the_service() {
        let l = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            resolve_labels(&l(&["cua"])),
            Some((RequestedClass::Any, false))
        );
        assert_eq!(
            resolve_labels(&l(&["self-hosted", "macOS", "ARM64", "cua"])),
            Some((RequestedClass::Macos, false))
        );
        assert_eq!(
            resolve_labels(&l(&["cua", "cua-linux", "cua-large"])),
            Some((RequestedClass::Linux, true))
        );
        assert_eq!(resolve_labels(&l(&["ubuntu-latest"])), None);
        assert_eq!(resolve_labels(&l(&["cua", "gpu"])), None);
        assert_eq!(resolve_labels(&l(&["macOS"])), None);
    }

    #[test]
    fn log_uploads_are_bounded() {
        let line = LogLine {
            at: "t".into(),
            level: "info".into(),
            text: "x".into(),
        };
        let mut u = LogUpload {
            lease_id: "l".into(),
            seq: 1,
            lines: vec![line.clone(); 2000],
        };
        u.validate().unwrap();
        u.lines.push(line);
        assert!(u.validate().is_err());
        let big = LogLine {
            at: String::new(),
            level: String::new(),
            text: "x".repeat(1 << 20),
        };
        u.lines = vec![big];
        assert!(u.validate().is_err());
    }

    #[test]
    fn install_wire_forms() {
        let s: InstallSession =
            serde_json::from_str(r#"{"session_id":"ins_1","url":"https://github.com/apps/x"}"#)
                .unwrap();
        assert_eq!(s.poll_interval_s, 2);
        let st: InstallState = serde_json::from_str(
            r#"{"status":"installed","installation":{"id":"inst_1","github_login":"acme"}}"#,
        )
        .unwrap();
        assert_eq!(st.status, InstallStatus::Installed);
        assert_eq!(st.installation.unwrap().github_login, "acme");
        let body = serde_json::to_value(InstallStart {
            loopback_port: Some(1),
            github_login: None,
        })
        .unwrap();
        assert_eq!(body, serde_json::json!({"loopback_port": 1}));
    }
}
