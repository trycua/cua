//! The hosted Cua CI service from this computer's side: the HTTP client for
//! `/v1/ci` ([`ServiceClient`]), what `cua ci connect` keeps locally
//! ([`HostedState`]), and the loop that registers this computer, claims
//! jobs and hands each to a [`JobRunner`] ([`HostedAgent`]).
//!
//! The contract is `ci-contract/CONTRACT.md`; the wire types are in
//! [`crate::service_api`]. The client holds no GitHub credential: the
//! service keeps the App key and mints a one-job runner configuration on
//! request, and that configuration only ever goes into one Space.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::{Semaphore, watch};

use crate::paths::CiPaths;
use crate::service_api::*;
use crate::{Error, Result};

impl std::error::Error for ApiError {}

/// Gives the signed-in account's access token (the CLI implements it over
/// the Cua session; it refreshes the token when it is due).
#[async_trait]
pub trait TokenSource: Send + Sync {
    /// A valid bearer token, or an error that says to sign in.
    async fn token(&self) -> Result<String>;
}

/// The calls the agent and a [`JobRunner`] make on the service: a seam for
/// tests.
#[async_trait]
pub trait ServiceApi: Send + Sync {
    /// `PUT /v1/ci/computers/{id}`.
    async fn register(
        &self,
        machine_id: &str,
        reg: &MachineRegistration,
    ) -> Result<MachineRegistered>;
    /// `POST /v1/ci/claim`.
    async fn claim(&self, req: &ClaimRequest) -> Result<ClaimResponse>;
    /// `POST /v1/ci/jobs/{id}/jit`.
    async fn jit(&self, job_id: &str, req: &JitRequest) -> Result<JitReply>;
    /// `POST /v1/ci/jobs/{id}/report`.
    async fn report(&self, job_id: &str, report: &Report) -> Result<()>;
    /// `POST /v1/ci/jobs/{id}/logs`.
    async fn upload_logs(&self, job_id: &str, up: &LogUpload) -> Result<()>;
}

/// A response with the parts the client uses.
struct Reply {
    status: u16,
    etag: Option<String>,
    body: String,
}

/// The HTTP client for `/v1/ci`.
pub struct ServiceClient {
    base: String,
    http: reqwest::Client,
    tokens: Arc<dyn TokenSource>,
    account: String,
    max_attempts: u32,
}

impl ServiceClient {
    /// A client for the API at `base` (`https://api.cua.ai`), acting for
    /// `account` (`me` or an organization).
    pub fn new(base: &str, account: &str, tokens: Arc<dyn TokenSource>) -> Result<Self> {
        let url = url::Url::parse(base.trim())
            .map_err(|e| Error::Config(format!("Cua API URL {base:?}: {e}")))?;
        let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost"));
        if url.scheme() != "https" && !loopback {
            return Err(Error::Config(format!("Cua API URL {base:?}: use https")));
        }
        let http = reqwest::Client::builder()
            .user_agent(concat!("cua-ci/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| Error::Service(ApiError::transport(format!("HTTP client: {e}"))))?;
        Ok(Self {
            base: base.trim().trim_end_matches('/').to_string(),
            http,
            tokens,
            account: if account.trim().is_empty() {
                "me".into()
            } else {
                account.trim().to_string()
            },
            max_attempts: 4,
        })
    }

    /// The account the client acts for.
    pub fn account(&self) -> &str {
        &self.account
    }

    /// Attempts per call (1 turns retrying off).
    pub fn with_attempts(mut self, n: u32) -> Self {
        self.max_attempts = n.max(1);
        self
    }

    fn idempotency_key() -> String {
        format!("{:032x}", rand::random::<u128>())
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
        if_match: Option<&str>,
        timeout: Duration,
    ) -> Result<Reply> {
        let key = Self::idempotency_key();
        let mut attempt = 0;
        loop {
            attempt += 1;
            let token = self.tokens.token().await?;
            let mut req = self
                .http
                .request(method.clone(), format!("{}/v1/ci/{}", self.base, path))
                .bearer_auth(token)
                .header("Cua-CI-Protocol", PROTOCOL.to_string())
                .query(&[("account", self.account.as_str())])
                .query(query)
                .timeout(timeout);
            if method != reqwest::Method::GET {
                req = req.header("Idempotency-Key", key.clone());
            }
            if let Some(m) = if_match {
                req = req.header("If-Match", m);
            }
            if let Some(b) = &body {
                req = req.json(b);
            }
            let outcome = match req.send().await {
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let retry = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok());
                    let etag = resp
                        .headers()
                        .get("etag")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string);
                    let text = resp.text().await.unwrap_or_default();
                    if (200..300).contains(&status) {
                        return Ok(Reply {
                            status,
                            etag,
                            body: text,
                        });
                    }
                    ApiError::parse(status, &text, retry)
                }
                Err(e) => ApiError::transport(format!("{}: {e}", self.base)),
            };
            if !(outcome.retryable() || outcome.status == 0) || attempt >= self.max_attempts {
                return Err(Error::Service(outcome));
            }
            let wait = outcome
                .retry_after_s
                .map(Duration::from_secs)
                .unwrap_or_else(|| backoff(attempt, 1000, 30_000));
            tokio::time::sleep(wait.min(Duration::from_secs(60))).await;
        }
    }

    async fn call<R: DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<R> {
        let r = self
            .send(method, path, query, body, None, Duration::from_secs(30))
            .await?;
        parse_body(&r)
    }

    async fn call_unit(
        &self,
        method: reqwest::Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<serde_json::Value>,
    ) -> Result<()> {
        self.send(method, path, query, body, None, Duration::from_secs(30))
            .await
            .map(|_| ())
    }

    fn json<T: Serialize>(v: &T) -> Result<serde_json::Value> {
        serde_json::to_value(v).map_err(|e| Error::Config(format!("request body: {e}")))
    }

    /// `POST /v1/ci/account`: enables CI for the account on first use.
    pub async fn ensure_account(&self) -> Result<Account> {
        self.call(
            reqwest::Method::POST,
            "account",
            &[],
            Some(serde_json::json!({})),
        )
        .await
    }

    /// `GET /v1/ci/account`.
    pub async fn account_info(&self) -> Result<Account> {
        self.call(reqwest::Method::GET, "account", &[], None).await
    }

    /// `POST /v1/ci/install/start`.
    pub async fn install_start(&self, body: &InstallStart) -> Result<InstallSession> {
        self.call(
            reqwest::Method::POST,
            "install/start",
            &[],
            Some(Self::json(body)?),
        )
        .await
    }

    /// `GET /v1/ci/install/{session}`.
    pub async fn install_state(&self, session: &str) -> Result<InstallState> {
        self.call(
            reqwest::Method::GET,
            &format!("install/{}", enc(session)),
            &[],
            None,
        )
        .await
    }

    /// `DELETE /v1/ci/installations/{id}`.
    pub async fn delete_installation(&self, id: &str, uninstall: bool) -> Result<()> {
        self.call_unit(
            reqwest::Method::DELETE,
            &format!("installations/{}", enc(id)),
            &[("uninstall", uninstall.to_string())],
            None,
        )
        .await
    }

    /// `GET /v1/ci/repos` (every page) with the allow list's etag.
    pub async fn repos(&self) -> Result<(Repos, Option<String>)> {
        let mut all = Repos::default();
        let mut cursor: Option<String> = None;
        let mut etag = None;
        for _ in 0..100 {
            let q: Vec<(&str, String)> = cursor
                .iter()
                .map(|c| ("cursor", c.clone()))
                .collect::<Vec<_>>();
            let r = self
                .send(
                    reqwest::Method::GET,
                    "repos",
                    &q,
                    None,
                    None,
                    Duration::from_secs(30),
                )
                .await?;
            etag = etag.or(r.etag.clone());
            let page: Repos = parse_body(&r)?;
            if all.installation.is_empty() {
                all.installation = page.installation.clone();
                all.repos_selection = page.repos_selection.clone();
            }
            all.repos.extend(page.repos);
            match page.next_cursor {
                Some(c) if !c.is_empty() => cursor = Some(c),
                _ => return Ok((all, etag)),
            }
        }
        Err(Error::Config(
            "the repository list has too many pages".into(),
        ))
    }

    /// `PUT /v1/ci/repos`.
    pub async fn put_allow(
        &self,
        list: &AllowList,
        etag: Option<&str>,
        allow_pattern: bool,
    ) -> Result<()> {
        let q = [("allow_pattern", allow_pattern.to_string())];
        self.send(
            reqwest::Method::PUT,
            "repos",
            &q,
            Some(Self::json(list)?),
            etag,
            Duration::from_secs(30),
        )
        .await
        .map(|_| ())
    }

    /// `GET /v1/ci/settings` with its etag.
    pub async fn settings(&self) -> Result<(Settings, Option<String>)> {
        let r = self
            .send(
                reqwest::Method::GET,
                "settings",
                &[],
                None,
                None,
                Duration::from_secs(30),
            )
            .await?;
        Ok((parse_body(&r)?, r.etag))
    }

    /// `PUT /v1/ci/settings` (`etag` from [`Self::settings`]).
    pub async fn put_settings(&self, s: &Settings, etag: Option<&str>) -> Result<Settings> {
        let r = self
            .send(
                reqwest::Method::PUT,
                "settings",
                &[],
                Some(Self::json(s)?),
                etag,
                Duration::from_secs(30),
            )
            .await?;
        parse_body(&r)
    }

    /// `GET /v1/ci/computers` as JSON rows.
    pub async fn machines(&self) -> Result<Vec<MachineStatus>> {
        #[derive(Deserialize)]
        struct Rows {
            #[serde(default)]
            machines: Vec<MachineStatus>,
        }
        let r: Rows = self
            .call(reqwest::Method::GET, "machines", &[], None)
            .await?;
        Ok(r.machines)
    }

    /// `DELETE /v1/ci/computers/{id}`.
    pub async fn delete_machine(&self, id: &str) -> Result<()> {
        self.call_unit(
            reqwest::Method::DELETE,
            &format!("machines/{}", enc(id)),
            &[],
            None,
        )
        .await
    }

    /// `GET /v1/ci/status`.
    pub async fn status(&self) -> Result<Status> {
        self.call(reqwest::Method::GET, "status", &[], None).await
    }

    /// `GET /v1/ci/jobs`.
    pub async fn jobs(
        &self,
        state: Option<&str>,
        repo: Option<&str>,
        limit: u32,
    ) -> Result<JobPage> {
        let mut q = vec![("limit", limit.to_string())];
        if let Some(s) = state {
            q.push(("state", s.to_string()));
        }
        if let Some(r) = repo {
            q.push(("repo", r.to_string()));
        }
        self.call(reqwest::Method::GET, "jobs", &q, None).await
    }

    /// `GET /v1/ci/jobs/{id}` (a job id or a `run/job` key).
    pub async fn job(&self, id: &str) -> Result<serde_json::Value> {
        self.call(
            reqwest::Method::GET,
            &format!("jobs/{}", enc(id)),
            &[],
            None,
        )
        .await
    }

    /// `GET /v1/ci/jobs/{id}/logs`.
    pub async fn logs(&self, id: &str, after: u64, limit: u32) -> Result<LogPage> {
        self.call(
            reqwest::Method::GET,
            &format!("jobs/{}/logs", enc(id)),
            &[("after", after.to_string()), ("limit", limit.to_string())],
            None,
        )
        .await
    }

    /// `GET /v1/ci/usage`.
    pub async fn usage(&self, month: Option<&str>) -> Result<UsageSummary> {
        let q: Vec<(&str, String)> = month.iter().map(|m| ("month", m.to_string())).collect();
        self.call(reqwest::Method::GET, "usage", &q, None).await
    }
}

#[async_trait]
impl ServiceApi for ServiceClient {
    async fn register(
        &self,
        machine_id: &str,
        reg: &MachineRegistration,
    ) -> Result<MachineRegistered> {
        reg.validate().map_err(Error::Config)?;
        self.call(
            reqwest::Method::PUT,
            &format!("machines/{}", enc(machine_id)),
            &[],
            Some(Self::json(reg)?),
        )
        .await
    }

    async fn claim(&self, req: &ClaimRequest) -> Result<ClaimResponse> {
        req.validate().map_err(Error::Config)?;
        let r = self
            .send(
                reqwest::Method::POST,
                "claim",
                &[],
                Some(Self::json(req)?),
                None,
                Duration::from_secs(u64::from(req.wait_s) + 20),
            )
            .await?;
        if r.status == 204 || r.body.trim().is_empty() {
            return Ok(ClaimResponse::default());
        }
        parse_body(&r)
    }

    async fn jit(&self, job_id: &str, req: &JitRequest) -> Result<JitReply> {
        self.call(
            reqwest::Method::POST,
            &format!("jobs/{}/jit", enc(job_id)),
            &[],
            Some(Self::json(req)?),
        )
        .await
    }

    async fn report(&self, job_id: &str, report: &Report) -> Result<()> {
        report.validate().map_err(Error::Config)?;
        self.call_unit(
            reqwest::Method::POST,
            &format!("jobs/{}/report", enc(job_id)),
            &[],
            Some(Self::json(report)?),
        )
        .await
    }

    async fn upload_logs(&self, job_id: &str, up: &LogUpload) -> Result<()> {
        up.validate().map_err(Error::Config)?;
        self.call_unit(
            reqwest::Method::POST,
            &format!("jobs/{}/logs", enc(job_id)),
            &[],
            Some(Self::json(up)?),
        )
        .await
    }
}

impl ApiError {
    /// A failure before any response (connection, timeout): status 0.
    pub fn transport(detail: String) -> Self {
        ApiError {
            status: 0,
            code: "unreachable".into(),
            detail: format!("the Cua service cannot be reached ({detail})"),
            ..Default::default()
        }
    }
}

fn parse_body<R: DeserializeOwned>(r: &Reply) -> Result<R> {
    let text = if r.body.trim().is_empty() {
        "null"
    } else {
        r.body.as_str()
    };
    serde_json::from_str(text).map_err(|e| {
        Error::Service(ApiError {
            status: r.status,
            code: "bad_response".into(),
            detail: format!("the Cua service answered something unexpected ({e})"),
            ..Default::default()
        })
    })
}

/// Percent-encodes one path segment (ids are plain, keys hold `/`).
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Jittered exponential backoff: `min_ms` doubling per attempt to `max_ms`,
/// then up to 25 percent more.
pub fn backoff(attempt: u32, min_ms: u64, max_ms: u64) -> Duration {
    let base = min_ms.saturating_mul(1u64 << attempt.saturating_sub(1).min(20));
    let capped = base.min(max_ms);
    let jitter = rand::random::<u64>() % (capped / 4 + 1);
    Duration::from_millis(capped + jitter)
}

/// An RFC 3339 UTC timestamp (`2026-10-28T10:00:00Z`) for a Unix time in
/// milliseconds.
pub fn rfc3339(ms: u64) -> String {
    let secs = (ms / 1000) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

// ---------------------------------------------------------------------
// Local state
// ---------------------------------------------------------------------

/// What `cua ci connect` keeps on this computer (no secret): which account
/// and installation this computer serves, and how it was registered.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostedState {
    /// `me` or an organization id.
    pub account: String,
    /// The linked installation (`inst_...`).
    pub installation: String,
    /// The GitHub organization.
    #[serde(default)]
    pub github_login: String,
    /// This computer's id.
    pub machine_id: String,
    /// The name shown in `cua ci status`.
    pub machine_name: String,
    /// macOS jobs at once (0: none).
    #[serde(default)]
    pub macos_slots: u32,
    /// Linux jobs at once (0: none).
    #[serde(default)]
    pub linux_slots: u32,
    /// Image macOS jobs start from.
    #[serde(default)]
    pub macos_image: Option<String>,
    /// Image Linux jobs start from.
    #[serde(default)]
    pub linux_image: Option<String>,
    /// Where job Spaces run: `local`, `auto` (any of your computers) or
    /// `host:<computer>`.
    #[serde(default = "local")]
    pub on: String,
    /// When connected (RFC 3339).
    #[serde(default)]
    pub connected_at: String,
}

fn local() -> String {
    "local".into()
}

impl HostedState {
    /// Where it lives.
    pub fn path(paths: &CiPaths) -> std::path::PathBuf {
        paths.root().join("hosted.json")
    }

    /// Reads it; `None` when this computer is not connected.
    pub fn load(paths: &CiPaths) -> Result<Option<Self>> {
        match std::fs::read_to_string(Self::path(paths)) {
            Ok(t) => serde_json::from_str(&t)
                .map(Some)
                .map_err(|e| Error::Config(format!("{}: {e}", Self::path(paths).display()))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes it (mode 0600, through a temporary file).
    pub fn save(&self, paths: &CiPaths) -> Result<()> {
        let path = Self::path(paths);
        cua_home::guard_write(&path)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        write_private(&tmp, &serde_json::to_vec_pretty(self).unwrap_or_default())?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Forgets it; `false` when there was nothing.
    pub fn remove(paths: &CiPaths) -> Result<bool> {
        match std::fs::remove_file(Self::path(paths)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// The registration this state describes.
    pub fn registration(&self, os: &str, arch: &str, resources: ClassSizes) -> MachineRegistration {
        let mut classes = Vec::new();
        if self.macos_slots > 0 {
            classes.push(MachineClass {
                class: Class::Macos,
                arch: arch.to_string(),
                max_slots: self.macos_slots,
                cpus: resources.macos_cpus,
                memory_mb: resources.macos_memory_mb,
                disk_gb: None,
                warm_target: 0,
                image: self.macos_image.clone(),
            });
        }
        if self.linux_slots > 0 {
            classes.push(MachineClass {
                class: Class::Linux,
                arch: arch.to_string(),
                max_slots: self.linux_slots,
                cpus: resources.linux_cpus,
                memory_mb: resources.linux_memory_mb,
                disk_gb: None,
                warm_target: 0,
                image: self.linux_image.clone(),
            });
        }
        MachineRegistration {
            name: self.machine_name.clone(),
            kind: MachineKind::Local,
            os: os.to_string(),
            arch: arch.to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
            classes,
            byoc: None,
            cost: MachineCost::default(),
        }
    }
}

/// vCPUs and memory per job of each class.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassSizes {
    /// macOS vCPUs.
    pub macos_cpus: Option<u32>,
    /// macOS memory in MiB.
    pub macos_memory_mb: Option<u64>,
    /// Linux vCPUs.
    pub linux_cpus: Option<u32>,
    /// Linux memory in MiB.
    pub linux_memory_mb: Option<u64>,
}

fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(data)?;
    f.sync_all()
}

/// The default slots of this computer: two macOS jobs on an Apple silicon
/// Mac (Apple's limit), Linux jobs one per four cores (at most 8), none
/// of a class the host cannot run.
pub fn default_slots(os: &str, arch: &str, cpus: u32) -> (u32, u32) {
    let macos = if os == "macos" && arch == "arm64" {
        2
    } else {
        0
    };
    let linux = (cpus / 4).clamp(1, 8);
    // Linux jobs also run on a Mac (in containers or VMs) but one at a
    // time beside the macOS ones so the Mac keeps its two macOS slots.
    let linux = if os == "macos" { linux.min(2) } else { linux };
    (macos, linux)
}

// ---------------------------------------------------------------------
// Running jobs
// ---------------------------------------------------------------------

/// What a [`JobRunner`] sees of one job: the service calls it may make for
/// it, the abort signal, and a log sink.
pub struct JobHandle {
    api: Arc<dyn ServiceApi>,
    machine_id: String,
    job_id: String,
    lease_id: String,
    abort: watch::Receiver<bool>,
    lease_state: Arc<Mutex<String>>,
    log_seq: Mutex<u64>,
}

impl JobHandle {
    /// A handle (the agent builds them; tests too).
    pub fn new(
        api: Arc<dyn ServiceApi>,
        machine_id: &str,
        job: &ClaimedJob,
        abort: watch::Receiver<bool>,
        lease_state: Arc<Mutex<String>>,
    ) -> Self {
        Self {
            api,
            machine_id: machine_id.to_string(),
            job_id: job.job_id.clone(),
            lease_id: job.lease_id.clone(),
            abort,
            lease_state,
            log_seq: Mutex::new(0),
        }
    }

    /// Asks the service for the job's one-job runner configuration. Call it
    /// only once the Space can run the runner.
    pub async fn jit(&self) -> Result<JitReply> {
        self.api
            .jit(
                &self.job_id,
                &JitRequest {
                    lease_id: self.lease_id.clone(),
                    machine_id: self.machine_id.clone(),
                },
            )
            .await
    }

    /// Reports a state that carries nothing else (`booting`,
    /// `runner_online`, `running`). A failure to report is logged, never
    /// fatal to the job.
    pub async fn state(&self, state: ReportState) {
        if let Some(word) = serde_json::to_value(state)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
        {
            *self.lease_state.lock().expect("lease state") = word;
        }
        let r = Report {
            lease_id: self.lease_id.clone(),
            machine_id: self.machine_id.clone(),
            state,
            at: rfc3339(crate::unix_ms()),
            result: None,
            infra: false,
            detail: None,
            usage: None,
        };
        if let Err(e) = self.api.report(&self.job_id, &r).await {
            tracing::warn!(job = %self.job_id, "report {state:?}: {e}");
        }
    }

    /// Whether the service asked to stop this job.
    pub fn aborted(&self) -> bool {
        *self.abort.borrow()
    }

    /// Resolves when the service asks to stop this job.
    pub async fn wait_aborted(&self) {
        let mut rx = self.abort.clone();
        while !*rx.borrow() {
            if rx.changed().await.is_err() {
                std::future::pending::<()>().await;
            }
        }
    }

    /// Sends log lines (secrets already removed by the caller).
    pub async fn log(&self, lines: Vec<LogLine>) {
        for chunk in lines.chunks(1000) {
            let seq = {
                let mut s = self.log_seq.lock().expect("log seq");
                *s += 1;
                *s
            };
            let up = LogUpload {
                lease_id: self.lease_id.clone(),
                seq,
                lines: chunk.to_vec(),
            };
            if let Err(e) = self.api.upload_logs(&self.job_id, &up).await {
                tracing::warn!(job = %self.job_id, "uploading logs: {e}");
            }
        }
    }
}

/// How a job ended, as the runner tells the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    /// The job ran and ended.
    Finished {
        /// `success`, `failure` or `cancelled`.
        result: String,
        /// What it used.
        usage: Usage,
    },
    /// The job or the infrastructure failed.
    Failed {
        /// Not the job's own failure.
        infra: bool,
        /// Plain words.
        detail: String,
        /// What it used.
        usage: Usage,
    },
    /// This computer cannot take it (no room, aborted before start): the
    /// service requeues it.
    Released {
        /// Plain words.
        detail: String,
    },
}

/// Runs one claimed job to its end: Space, runner, teardown.
#[async_trait]
pub trait JobRunner: Send + Sync {
    /// Runs `job`; every Space it created is gone when this returns.
    async fn run(&self, job: &ClaimedJob, handle: &JobHandle) -> JobOutcome;
}

/// Timing of the agent's loops.
#[derive(Clone, Copy, Debug)]
pub struct AgentTiming {
    /// How long a claim call holds (at most 20).
    pub claim_wait_s: u32,
    /// First wait after a failed call.
    pub backoff_min_ms: u64,
    /// Longest wait after failed calls.
    pub backoff_max_ms: u64,
    /// A computer unable to reach the service for this long takes no new
    /// work.
    pub offline_after: Duration,
}

impl Default for AgentTiming {
    fn default() -> Self {
        Self {
            claim_wait_s: MAX_CLAIM_WAIT_S,
            backoff_min_ms: 1000,
            backoff_max_ms: 30_000,
            offline_after: Duration::from_secs(120),
        }
    }
}

struct Lease {
    lease_id: String,
    class: Class,
    arch: String,
    state: Arc<Mutex<String>>,
    abort: watch::Sender<bool>,
}

/// Registers this computer and serves the jobs the service assigns.
pub struct HostedAgent {
    api: Arc<dyn ServiceApi>,
    runner: Arc<dyn JobRunner>,
    machine_id: String,
    registration: MachineRegistration,
    timing: AgentTiming,
    leases: Arc<Mutex<HashMap<String, Lease>>>,
    /// Jobs the agent runs at once: the sum of the classes' slots.
    permits: Arc<Semaphore>,
}

impl HostedAgent {
    /// An agent for `registration` (its classes' `max_slots` are the
    /// capacity).
    pub fn new(
        api: Arc<dyn ServiceApi>,
        runner: Arc<dyn JobRunner>,
        machine_id: &str,
        registration: MachineRegistration,
        timing: AgentTiming,
    ) -> Self {
        let total: u32 = registration.classes.iter().map(|c| c.max_slots).sum();
        Self {
            api,
            runner,
            machine_id: machine_id.to_string(),
            registration,
            timing,
            leases: Arc::new(Mutex::new(HashMap::new())),
            permits: Arc::new(Semaphore::new(total as usize)),
        }
    }

    /// Jobs held right now.
    pub fn active(&self) -> usize {
        self.leases.lock().expect("leases").len()
    }

    /// Slots by class, from the registration and the leases held.
    pub fn slots(&self) -> Vec<ClassSlots> {
        let leases = self.leases.lock().expect("leases");
        self.registration
            .classes
            .iter()
            .map(|c| {
                let busy = leases
                    .values()
                    .filter(|l| l.class == c.class && l.arch == c.arch)
                    .count() as u32;
                ClassSlots {
                    class: c.class,
                    arch: c.arch.clone(),
                    free_slots: c.max_slots.saturating_sub(busy),
                    busy_slots: busy.min(c.max_slots),
                    warm_slots: 0,
                }
            })
            .collect()
    }

    fn lease_states(&self) -> Vec<LeaseState> {
        self.leases
            .lock()
            .expect("leases")
            .iter()
            .map(|(job_id, l)| LeaseState {
                lease_id: l.lease_id.clone(),
                job_id: job_id.clone(),
                state: l.state.lock().expect("state").clone(),
            })
            .collect()
    }

    /// Registers, then claims and runs jobs until `shutdown` resolves. The
    /// jobs running then finish before this returns.
    pub async fn run(&self, shutdown: impl std::future::Future<Output = ()>) -> Result<()> {
        tokio::pin!(shutdown);
        // Registration, retried until it works or the client is too old.
        let mut attempt = 0;
        let registered = loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                r = self.api.register(&self.machine_id, &self.registration) => match r {
                    Ok(r) => break r,
                    Err(Error::Service(e)) if fatal(&e) => return Err(Error::Service(e)),
                    Err(e) => {
                        attempt += 1;
                        tracing::warn!("registering with Cua CI: {e}");
                        tokio::select! {
                            _ = &mut shutdown => return Ok(()),
                            _ = tokio::time::sleep(backoff(attempt, self.timing.backoff_min_ms, self.timing.backoff_max_ms)) => {}
                        }
                    }
                }
            }
        };
        let wait = registered.claim_wait_s.clamp(0, self.timing.claim_wait_s);
        let mut failures = 0u32;
        let mut first_failure: Option<std::time::Instant> = None;
        loop {
            let slots = self.slots();
            let free: u32 = slots.iter().map(|s| s.free_slots).sum();
            let offline = first_failure
                .map(|t| t.elapsed() >= self.timing.offline_after)
                .unwrap_or(false);
            // No free slot, or the service is out of reach for too long: a
            // heartbeat only (zero free slots), never a new job.
            let claim = ClaimRequest {
                machine_id: self.machine_id.clone(),
                classes: slots
                    .into_iter()
                    .map(|mut s| {
                        if offline || free == 0 {
                            s.free_slots = 0;
                        }
                        s
                    })
                    .collect(),
                leases: self.lease_states(),
                wait_s: wait,
            };
            let reply = tokio::select! {
                _ = &mut shutdown => break,
                r = self.api.claim(&claim) => r,
            };
            match reply {
                Ok(r) => {
                    failures = 0;
                    first_failure = None;
                    self.apply(r);
                }
                Err(Error::Service(e)) if fatal(&e) => {
                    self.drain().await;
                    return Err(Error::Service(e));
                }
                Err(e) => {
                    failures += 1;
                    first_failure.get_or_insert_with(std::time::Instant::now);
                    tracing::warn!("claiming from Cua CI: {e}");
                    tokio::select! {
                        _ = &mut shutdown => break,
                        _ = tokio::time::sleep(backoff(failures, self.timing.backoff_min_ms, self.timing.backoff_max_ms)) => {}
                    }
                }
            }
        }
        self.drain().await;
        Ok(())
    }

    /// Waits for every job in flight.
    async fn drain(&self) {
        let total = self
            .registration
            .classes
            .iter()
            .map(|c| c.max_slots)
            .sum::<u32>();
        // Holding every permit means no job is running.
        let _ = self.permits.acquire_many(total).await;
    }

    fn apply(&self, r: ClaimResponse) {
        for a in &r.leases {
            if a.action == "abort" {
                let leases = self.leases.lock().expect("leases");
                for l in leases.values().filter(|l| l.lease_id == a.lease_id) {
                    let _ = l.abort.send(true);
                }
            }
        }
        if r.paused {
            tracing::info!("this computer is drained by an administrator");
        }
        for job in r.jobs {
            self.start(job);
        }
    }

    fn start(&self, job: ClaimedJob) {
        if self
            .leases
            .lock()
            .expect("leases")
            .contains_key(&job.job_id)
        {
            return; // a replayed assignment
        }
        let (tx, rx) = watch::channel(false);
        let state = Arc::new(Mutex::new("booting".to_string()));
        self.leases.lock().expect("leases").insert(
            job.job_id.clone(),
            Lease {
                lease_id: job.lease_id.clone(),
                class: job.class,
                arch: job.arch.clone(),
                state: state.clone(),
                abort: tx,
            },
        );
        let api = self.api.clone();
        let runner = self.runner.clone();
        let leases = self.leases.clone();
        let machine = self.machine_id.clone();
        let permits = self.permits.clone();
        tokio::spawn(async move {
            // A permit per job: `drain` waits for all of them.
            let permit = permits.acquire_owned().await.ok();
            let handle = JobHandle::new(api.clone(), &machine, &job, rx, state);
            let outcome = match job.check() {
                Err(why) => JobOutcome::Released {
                    detail: format!("refused the assignment: {why}"),
                },
                Ok(()) => runner.run(&job, &handle).await,
            };
            let report = final_report(&machine, &job, outcome);
            if let Err(e) = api.report(&job.job_id, &report).await {
                tracing::warn!(job = %job.job_id, "final report: {e}");
            }
            leases.lock().expect("leases").remove(&job.job_id);
            drop(permit);
        });
    }
}

/// Runs several agents (this computer and each cloud pool) until `shutdown`
/// resolves, then waits for the jobs in flight. The first agent that stops
/// with an error ends the others and its error is returned.
pub async fn run_all(
    agents: Vec<Arc<HostedAgent>>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let (tx, rx) = watch::channel(false);
    tokio::spawn(async move {
        shutdown.await;
        let _ = tx.send(true);
    });
    let mut set = tokio::task::JoinSet::new();
    for a in agents {
        let mut rx = rx.clone();
        set.spawn(async move {
            a.run(async move {
                let _ = rx.wait_for(|v| *v).await;
            })
            .await
        });
    }
    let mut first: Result<()> = Ok(());
    while let Some(done) = set.join_next().await {
        match done {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                if first.is_ok() {
                    first = Err(e);
                }
            }
            Err(e) => tracing::warn!("an agent task ended: {e}"),
        }
    }
    first
}

/// A fatal answer: the client is too old, or the account is gone.
fn fatal(e: &ApiError) -> bool {
    e.status == 426 || (e.status == 403 && e.code == "forbidden")
}

/// The report that ends a job.
pub fn final_report(machine_id: &str, job: &ClaimedJob, outcome: JobOutcome) -> Report {
    let at = rfc3339(crate::unix_ms());
    let base = Report {
        lease_id: job.lease_id.clone(),
        machine_id: machine_id.to_string(),
        state: ReportState::Released,
        at,
        result: None,
        infra: false,
        detail: None,
        usage: None,
    };
    match outcome {
        JobOutcome::Finished { result, usage } => Report {
            state: ReportState::Finished,
            result: Some(result),
            usage: Some(usage),
            ..base
        },
        JobOutcome::Failed {
            infra,
            detail,
            usage,
        } => Report {
            state: ReportState::Failed,
            infra,
            detail: Some(detail),
            usage: Some(usage),
            ..base
        },
        JobOutcome::Released { detail } => Report {
            detail: Some(detail),
            ..base
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn timestamps() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_788_000_000_000), "2026-08-29T10:40:00Z");
        assert_eq!(rfc3339(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_000), "1970-01-01T00:00:01Z");
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(enc("job_1"), "job_1");
        assert_eq!(enc("99/27"), "99%2F27");
        assert_eq!(enc("a b"), "a%20b");
    }

    #[test]
    fn backoff_grows_and_is_capped() {
        let a = backoff(1, 1000, 30_000).as_millis();
        assert!((1000..=1250).contains(&a), "{a}");
        let c = backoff(10, 1000, 30_000).as_millis();
        assert!((30_000..=37_500).contains(&c), "{c}");
        let big = backoff(60, 1000, 30_000).as_millis();
        assert!(big <= 37_500, "{big}");
    }

    #[test]
    fn default_slots_follow_the_host() {
        assert_eq!(default_slots("macos", "arm64", 10), (2, 2));
        assert_eq!(default_slots("macos", "x64", 8), (0, 2));
        assert_eq!(default_slots("linux", "x64", 16), (0, 4));
        assert_eq!(default_slots("linux", "x64", 2), (0, 1));
        assert_eq!(default_slots("linux", "arm64", 128), (0, 8));
    }

    #[test]
    fn hosted_state_round_trips_and_is_private() {
        let d = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(d.path().join("ci"));
        assert_eq!(HostedState::load(&paths).unwrap(), None);
        let s = HostedState {
            account: "me".into(),
            installation: "inst_1".into(),
            github_login: "acme".into(),
            machine_id: "m1".into(),
            machine_name: "mac".into(),
            macos_slots: 2,
            linux_slots: 0,
            macos_image: None,
            linux_image: None,
            on: "local".into(),
            connected_at: "t".into(),
        };
        s.save(&paths).unwrap();
        assert_eq!(HostedState::load(&paths).unwrap(), Some(s.clone()));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(HostedState::path(&paths))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let reg = s.registration("macos", "arm64", ClassSizes::default());
        reg.validate().unwrap();
        assert_eq!(reg.classes.len(), 1);
        assert_eq!(reg.classes[0].class, Class::Macos);
        assert!(HostedState::remove(&paths).unwrap());
        assert!(!HostedState::remove(&paths).unwrap());
    }

    // ---- the agent, against an in-memory service ----

    struct FakeService {
        jobs: Mutex<Vec<ClaimedJob>>,
        registrations: AtomicU32,
        reports: Mutex<Vec<(String, Report)>>,
        claims: Mutex<Vec<ClaimRequest>>,
        aborts: Mutex<Vec<String>>,
        fail_claims: AtomicU32,
        jits: AtomicU32,
        register_error: Mutex<Option<ApiError>>,
    }

    impl FakeService {
        fn new(jobs: Vec<ClaimedJob>) -> Arc<Self> {
            Arc::new(Self {
                jobs: Mutex::new(jobs),
                registrations: AtomicU32::new(0),
                reports: Mutex::new(vec![]),
                claims: Mutex::new(vec![]),
                aborts: Mutex::new(vec![]),
                fail_claims: AtomicU32::new(0),
                jits: AtomicU32::new(0),
                register_error: Mutex::new(None),
            })
        }
    }

    #[async_trait]
    impl ServiceApi for FakeService {
        async fn register(
            &self,
            _id: &str,
            _reg: &MachineRegistration,
        ) -> Result<MachineRegistered> {
            self.registrations.fetch_add(1, Ordering::SeqCst);
            if let Some(e) = self.register_error.lock().unwrap().clone() {
                return Err(Error::Service(e));
            }
            Ok(MachineRegistered {
                claim_wait_s: 1,
                serving: true,
                ..Default::default()
            })
        }

        async fn claim(&self, req: &ClaimRequest) -> Result<ClaimResponse> {
            self.claims.lock().unwrap().push(req.clone());
            if self.fail_claims.load(Ordering::SeqCst) > 0 {
                self.fail_claims.fetch_sub(1, Ordering::SeqCst);
                return Err(Error::Service(ApiError::transport("down".into())));
            }
            let free: u32 = req.classes.iter().map(|c| c.free_slots).sum();
            let given: Vec<ClaimedJob> = {
                let mut jobs = self.jobs.lock().unwrap();
                let take = (free as usize).min(jobs.len());
                jobs.drain(..take).collect()
            };
            let leases = self
                .aborts
                .lock()
                .unwrap()
                .iter()
                .map(|l| LeaseAction {
                    lease_id: l.clone(),
                    action: "abort".into(),
                })
                .collect();
            if given.is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok(ClaimResponse {
                jobs: given,
                leases,
                paused: false,
            })
        }

        async fn jit(&self, _job: &str, _req: &JitRequest) -> Result<JitReply> {
            self.jits.fetch_add(1, Ordering::SeqCst);
            serde_json::from_str(
                r#"{"encoded_jit_config":"x","runner_id":1,"runner_name":"r","labels":[],"expires_at":""}"#,
            )
            .map_err(|e| Error::Config(e.to_string()))
        }

        async fn report(&self, job: &str, r: &Report) -> Result<()> {
            self.reports
                .lock()
                .unwrap()
                .push((job.to_string(), r.clone()));
            Ok(())
        }

        async fn upload_logs(&self, _job: &str, _up: &LogUpload) -> Result<()> {
            Ok(())
        }
    }

    fn claimed(id: &str, class: Class) -> ClaimedJob {
        serde_json::from_value(serde_json::json!({
            "job_id": id, "lease_id": format!("lease_{id}"), "job_key": "1/2", "repo": "acme/app",
            "requested_class": "any", "class": class.as_str(), "arch": "arm64",
            "labels": ["cua"], "runner_name": format!("cua-{id}"),
            "policy": {"secrets": true, "cache": "rw", "egress": "deny-private"}
        }))
        .unwrap()
    }

    fn usage() -> Usage {
        Usage {
            vm_started_at: "a".into(),
            job_started_at: None,
            job_ended_at: None,
            vm_ended_at: "b".into(),
            class: Class::Macos,
            arch: "arm64".into(),
            cpus: None,
            memory_mb: None,
            start_mode: "cold_create".into(),
            cost_micros: None,
            phases: Phases::default(),
            cache: CacheStats::default(),
        }
    }

    /// Runs a job until `gate` opens (or the service aborts it).
    struct GatedRunner {
        gate: Arc<tokio::sync::Notify>,
        started: AtomicU32,
        max_parallel: AtomicU32,
        running: AtomicU32,
    }

    #[async_trait]
    impl JobRunner for GatedRunner {
        async fn run(&self, job: &ClaimedJob, h: &JobHandle) -> JobOutcome {
            let n = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_parallel.fetch_max(n, Ordering::SeqCst);
            self.started.fetch_add(1, Ordering::SeqCst);
            h.state(ReportState::Booting).await;
            let r = tokio::select! {
                _ = self.gate.notified() => "success",
                _ = h.wait_aborted() => "cancelled",
            };
            self.running.fetch_sub(1, Ordering::SeqCst);
            let _ = job;
            JobOutcome::Finished {
                result: r.into(),
                usage: usage(),
            }
        }
    }

    fn registration(macos: u32, linux: u32) -> MachineRegistration {
        let mut classes = vec![];
        for (c, n) in [(Class::Macos, macos), (Class::Linux, linux)] {
            if n > 0 {
                classes.push(MachineClass {
                    class: c,
                    arch: "arm64".into(),
                    max_slots: n,
                    cpus: None,
                    memory_mb: None,
                    disk_gb: None,
                    warm_target: 0,
                    image: None,
                });
            }
        }
        MachineRegistration {
            name: "m".into(),
            kind: MachineKind::Local,
            os: "macos".into(),
            arch: "arm64".into(),
            client_version: "0".into(),
            classes,
            byoc: None,
            cost: MachineCost::default(),
        }
    }

    fn fast() -> AgentTiming {
        AgentTiming {
            claim_wait_s: 1,
            backoff_min_ms: 5,
            backoff_max_ms: 20,
            offline_after: Duration::from_millis(30),
        }
    }

    async fn until(mut f: impl FnMut() -> bool) {
        for _ in 0..400 {
            if f() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition not reached");
    }

    #[tokio::test]
    async fn the_agent_never_runs_more_jobs_than_it_has_slots() {
        let svc = FakeService::new(vec![
            claimed("a", Class::Macos),
            claimed("b", Class::Macos),
            claimed("c", Class::Macos),
        ]);
        let gate = Arc::new(tokio::sync::Notify::new());
        let runner = Arc::new(GatedRunner {
            gate: gate.clone(),
            started: AtomicU32::new(0),
            max_parallel: AtomicU32::new(0),
            running: AtomicU32::new(0),
        });
        let agent = Arc::new(HostedAgent::new(
            svc.clone(),
            runner.clone(),
            "m1",
            registration(2, 0),
            fast(),
        ));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = agent.clone();
        let task = tokio::spawn(async move {
            a.run(async {
                let _ = stop_rx.await;
            })
            .await
        });
        until(|| runner.started.load(Ordering::SeqCst) == 2).await;
        // Two slots busy: the claim that follows offers none, and the third
        // job (the fake only gives jobs to free slots) is still waiting.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(runner.started.load(Ordering::SeqCst), 2);
        assert_eq!(agent.active(), 2);
        {
            let claims = svc.claims.lock().unwrap();
            let last = claims.last().unwrap();
            assert_eq!(last.classes[0].free_slots, 0);
            assert_eq!(last.classes[0].busy_slots, 2);
            assert_eq!(last.leases.len(), 2);
        }
        gate.notify_one();
        until(|| runner.started.load(Ordering::SeqCst) == 3).await;
        for _ in 0..400 {
            if agent.active() == 0 {
                break;
            }
            gate.notify_one();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(agent.active(), 0);
        assert!(runner.max_parallel.load(Ordering::SeqCst) <= 2);
        let _ = stop_tx.send(());
        task.await.unwrap().unwrap();
        // Every job ended with a finished report that carries usage.
        let reports = svc.reports.lock().unwrap();
        let finished = reports
            .iter()
            .filter(|(_, r)| r.state == ReportState::Finished)
            .count();
        assert_eq!(finished, 3);
        assert!(reports.iter().all(|(_, r)| r.validate().is_ok()));
        assert_eq!(svc.registrations.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_abort_from_the_service_reaches_the_runner() {
        let svc = FakeService::new(vec![claimed("a", Class::Linux)]);
        let gate = Arc::new(tokio::sync::Notify::new());
        let runner = Arc::new(GatedRunner {
            gate,
            started: AtomicU32::new(0),
            max_parallel: AtomicU32::new(0),
            running: AtomicU32::new(0),
        });
        let agent = Arc::new(HostedAgent::new(
            svc.clone(),
            runner.clone(),
            "m1",
            registration(0, 2),
            fast(),
        ));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = agent.clone();
        let task = tokio::spawn(async move {
            a.run(async {
                let _ = stop_rx.await;
            })
            .await
        });
        until(|| runner.started.load(Ordering::SeqCst) == 1).await;
        svc.aborts.lock().unwrap().push("lease_a".into());
        until(|| agent.active() == 0).await;
        {
            let reports = svc.reports.lock().unwrap();
            let end = reports
                .iter()
                .find(|(_, r)| r.state == ReportState::Finished)
                .unwrap();
            assert_eq!(end.1.result.as_deref(), Some("cancelled"));
        }
        let _ = stop_tx.send(());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_bad_assignment_is_released_not_run() {
        let mut bad = claimed("a", Class::Linux);
        bad.fork = true; // with secrets and a writable cache: refused
        let svc = FakeService::new(vec![bad]);
        let runner = Arc::new(GatedRunner {
            gate: Arc::new(tokio::sync::Notify::new()),
            started: AtomicU32::new(0),
            max_parallel: AtomicU32::new(0),
            running: AtomicU32::new(0),
        });
        let agent = Arc::new(HostedAgent::new(
            svc.clone(),
            runner.clone(),
            "m1",
            registration(0, 1),
            fast(),
        ));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = agent.clone();
        let task = tokio::spawn(async move {
            a.run(async {
                let _ = stop_rx.await;
            })
            .await
        });
        until(|| {
            svc.reports
                .lock()
                .unwrap()
                .iter()
                .any(|(_, r)| r.state == ReportState::Released)
        })
        .await;
        assert_eq!(runner.started.load(Ordering::SeqCst), 0);
        assert_eq!(svc.jits.load(Ordering::SeqCst), 0);
        let _ = stop_tx.send(());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn the_agent_rides_out_an_unreachable_service_and_takes_no_work_while_offline() {
        let svc = FakeService::new(vec![claimed("a", Class::Linux)]);
        svc.fail_claims.store(6, Ordering::SeqCst);
        let runner = Arc::new(GatedRunner {
            gate: Arc::new(tokio::sync::Notify::new()),
            started: AtomicU32::new(0),
            max_parallel: AtomicU32::new(0),
            running: AtomicU32::new(0),
        });
        let agent = Arc::new(HostedAgent::new(
            svc.clone(),
            runner.clone(),
            "m1",
            registration(0, 1),
            fast(),
        ));
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let a = agent.clone();
        let task = tokio::spawn(async move {
            a.run(async {
                let _ = stop_rx.await;
            })
            .await
        });
        // It recovers and runs the job once the service answers again.
        until(|| runner.started.load(Ordering::SeqCst) == 1).await;
        assert!(svc.claims.lock().unwrap().len() >= 7);
        // The job in flight finishes before the agent returns.
        runner.gate.notify_one();
        let _ = stop_tx.send(());
        task.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_client_that_is_too_old_stops_the_agent() {
        let svc = FakeService::new(vec![]);
        *svc.register_error.lock().unwrap() = Some(ApiError {
            status: 426,
            code: "client_too_old".into(),
            detail: "update cua".into(),
            ..Default::default()
        });
        let runner = Arc::new(GatedRunner {
            gate: Arc::new(tokio::sync::Notify::new()),
            started: AtomicU32::new(0),
            max_parallel: AtomicU32::new(0),
            running: AtomicU32::new(0),
        });
        let agent = HostedAgent::new(svc, runner, "m1", registration(0, 1), fast());
        let e = agent.run(std::future::pending::<()>()).await.unwrap_err();
        assert!(e.to_string().contains("update cua"), "{e}");
    }

    #[tokio::test]
    async fn run_all_stops_every_agent_on_shutdown() {
        let svc = FakeService::new(vec![]);
        let mk = |m: &str| {
            Arc::new(HostedAgent::new(
                svc.clone(),
                Arc::new(GatedRunner {
                    gate: Arc::new(tokio::sync::Notify::new()),
                    started: AtomicU32::new(0),
                    max_parallel: AtomicU32::new(0),
                    running: AtomicU32::new(0),
                }),
                m,
                registration(0, 1),
                fast(),
            ))
        };
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(run_all(vec![mk("a"), mk("b")], async move {
            let _ = rx.await;
        }));
        until(|| svc.registrations.load(Ordering::SeqCst) == 2).await;
        let _ = tx.send(());
        task.await.unwrap().unwrap();
    }

    #[test]
    fn the_final_report_matches_the_outcome() {
        let job = claimed("a", Class::Macos);
        let r = final_report(
            "m",
            &job,
            JobOutcome::Finished {
                result: "success".into(),
                usage: usage(),
            },
        );
        assert_eq!(r.state, ReportState::Finished);
        r.validate().unwrap();
        let r = final_report(
            "m",
            &job,
            JobOutcome::Failed {
                infra: true,
                detail: "VM died".into(),
                usage: usage(),
            },
        );
        assert!(r.infra);
        r.validate().unwrap();
        let r = final_report(
            "m",
            &job,
            JobOutcome::Released {
                detail: "no room".into(),
            },
        );
        assert_eq!(r.state, ReportState::Released);
        assert_eq!(r.lease_id, "lease_a");
        r.validate().unwrap();
    }
}
