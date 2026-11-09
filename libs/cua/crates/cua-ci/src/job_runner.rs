//! [`JobRunner`] over Cua Spaces: one fresh Space per job, the actions
//! runner started in it with the service's one-job configuration, the
//! Space deleted afterwards. This is the plain cold-start path; warm
//! Spaces, saved-memory forks and cache volumes build on the same seams
//! ([`Spaces`], [`TarballSource`]).
//!
//! Safety rules kept here: the runner configuration is requested only
//! once the Space can run the runner, goes in through the file API (mode
//! 0600), is removed from every log line, and never reaches a log of the
//! host; a Space is never reused; every Space this computer creates carries
//! a label so [`SpaceJobRunner::sweep`] can delete what a crash left.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use crate::cache::TarballSource;
use crate::config::{DEFAULT_MACOS_IMAGE, SPACE_LABEL_POOL, SPACE_LABEL_SLOT};
use crate::egress;
use crate::guest::{self, GuestState};
use crate::hosted::{JobHandle, JobOutcome, JobRunner, rfc3339};
use crate::paths::CiPaths;
use crate::service_api::*;
use crate::spaces::{SpaceSpec, Spaces, SpacesError};

/// The Linux image jobs start from unless the computer says otherwise.
pub const DEFAULT_LINUX_IMAGE: &str = "ghcr.io/trycua/linux:24.04";

const CREATE_TIMEOUT: Duration = Duration::from_secs(600);
const SH_TIMEOUT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_secs(2);
/// The runner must be up this long after it was started.
const ONLINE_TIMEOUT: Duration = Duration::from_secs(300);

/// What one class starts from and gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassImage {
    /// The class.
    pub class: Class,
    /// The image (`ghcr.io/...` or `checkpoint:<name>`).
    pub image: String,
    /// vCPUs.
    pub cpus: Option<u32>,
    /// Memory in MiB.
    pub memory_mb: Option<u64>,
    /// Disk in GiB.
    pub disk_gb: Option<u32>,
}

impl ClassImage {
    /// The defaults of `class`.
    pub fn default_for(class: Class) -> Self {
        Self {
            class,
            image: match class {
                Class::Macos => DEFAULT_MACOS_IMAGE.into(),
                Class::Linux => DEFAULT_LINUX_IMAGE.into(),
            },
            cpus: None,
            memory_mb: None,
            disk_gb: None,
        }
    }
}

/// Runs jobs in Spaces.
pub struct SpaceJobRunner {
    spaces: Arc<dyn Spaces>,
    tarballs: Arc<dyn TarballSource>,
    paths: CiPaths,
    machine_id: String,
    images: Vec<ClassImage>,
    on: String,
    job_timeout: Duration,
    poll: Duration,
}

impl SpaceJobRunner {
    /// A runner for `images`' classes on `on` (`local`, `auto` or
    /// `host:<computer>`).
    pub fn new(
        spaces: Arc<dyn Spaces>,
        tarballs: Arc<dyn TarballSource>,
        paths: CiPaths,
        machine_id: &str,
        images: Vec<ClassImage>,
        on: &str,
    ) -> Self {
        Self {
            spaces,
            tarballs,
            paths,
            machine_id: machine_id.to_string(),
            images,
            on: on.to_string(),
            job_timeout: Duration::from_secs(6 * 3600),
            poll: POLL,
        }
    }

    /// Stops a job running longer than this (the service's own
    /// `max_job_minutes` aborts it too).
    pub fn with_job_timeout(mut self, t: Duration) -> Self {
        self.job_timeout = t;
        self
    }

    /// How often the runner checks its job (tests use short ones).
    pub fn with_poll(mut self, p: Duration) -> Self {
        self.poll = p;
        self
    }

    fn owner_label(&self) -> String {
        format!("hosted-{}", self.machine_id)
    }

    /// Deletes the Spaces an earlier run of this agent left behind (the
    /// agent starts with no job, so every Space labelled with this
    /// computer's owner label is an orphan). Returns how many.
    pub async fn sweep(&self) -> Result<u32, SpacesError> {
        let owner = self.owner_label();
        let mut n = 0;
        for row in self.spaces.list().await? {
            if row.label(SPACE_LABEL_POOL) == Some(owner.as_str()) {
                match self.spaces.delete(&row.id).await {
                    Ok(()) | Err(SpacesError::NotFound(_)) => n += 1,
                    Err(e) => tracing::warn!(space = %row.id, "sweeping: {e}"),
                }
            }
        }
        Ok(n)
    }

    fn short(job_id: &str) -> String {
        job_id
            .trim_start_matches("job_")
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect()
    }
}

impl SpaceJobRunner {
    /// Runs `job` in a Space that already exists and is ready (a Modal
    /// sandbox the capacity manager created for it): no create and no
    /// delete, whoever made the Space ends it. Everything else is the same
    /// as [`JobRunner::run`].
    pub async fn run_in_existing(
        &self,
        job: &ClaimedJob,
        handle: &JobHandle,
        space_id: &str,
    ) -> JobOutcome {
        let Some(img) = self.images.iter().find(|i| i.class == job.class).cloned() else {
            return JobOutcome::Released {
                detail: format!("this computer does not serve {} jobs", job.class.as_str()),
            };
        };
        if handle.aborted() {
            return JobOutcome::Released {
                detail: "stopped before it started".into(),
            };
        }
        handle.state(ReportState::Booting).await;
        let mut tl = Timeline {
            vm_started_ms: crate::unix_ms(),
            boot_ms: Some(0),
            register_ms: None,
            job_started_ms: None,
            job_ended_ms: None,
        };
        let spec = SpaceSpec {
            name: String::new(),
            image: img.image.clone(),
            on: self.on.clone(),
            cpus: img.cpus,
            memory_mb: img.memory_mb,
            disk_gb: img.disk_gb,
            labels: BTreeMap::new(),
            // The cloud layer set the sandbox's network rules when it made it.
            egress: None,
            timeout: CREATE_TIMEOUT,
            create_id: String::new(),
            mounts: Vec::new(),
        };
        let outcome = self.in_space(job, handle, space_id, &spec, &mut tl).await;
        let end = crate::unix_ms();
        match outcome {
            Inner::Done { result } => JobOutcome::Finished {
                result,
                usage: tl.usage(job, &img, end),
            },
            Inner::Failed { infra, detail } => JobOutcome::Failed {
                infra,
                detail,
                usage: tl.usage(job, &img, end),
            },
            Inner::Released { detail } => JobOutcome::Released { detail },
        }
    }
}

struct Timeline {
    vm_started_ms: u64,
    boot_ms: Option<u64>,
    register_ms: Option<u64>,
    job_started_ms: Option<u64>,
    job_ended_ms: Option<u64>,
}

impl Timeline {
    fn usage(&self, job: &ClaimedJob, img: &ClassImage, end_ms: u64) -> Usage {
        Usage {
            vm_started_at: rfc3339(self.vm_started_ms),
            job_started_at: self.job_started_ms.map(rfc3339),
            job_ended_at: self.job_ended_ms.map(rfc3339),
            vm_ended_at: rfc3339(end_ms),
            class: job.class,
            arch: job.arch.clone(),
            cpus: img.cpus,
            memory_mb: img.memory_mb,
            start_mode: "cold_create".into(),
            cost_micros: None,
            phases: Phases {
                vm_boot_ms: self.boot_ms,
                runner_register_ms: self.register_ms,
                teardown_ms: self.job_ended_ms.map(|e| end_ms.saturating_sub(e)),
                ..Default::default()
            },
            cache: CacheStats::default(),
        }
    }
}

#[async_trait]
impl JobRunner for SpaceJobRunner {
    async fn run(&self, job: &ClaimedJob, handle: &JobHandle) -> JobOutcome {
        let Some(img) = self.images.iter().find(|i| i.class == job.class).cloned() else {
            return JobOutcome::Released {
                detail: format!("this computer does not serve {} jobs", job.class.as_str()),
            };
        };
        if handle.aborted() {
            return JobOutcome::Released {
                detail: "stopped before it started".into(),
            };
        }
        handle.state(ReportState::Booting).await;
        let mut tl = Timeline {
            vm_started_ms: crate::unix_ms(),
            boot_ms: None,
            register_ms: None,
            job_started_ms: None,
            job_ended_ms: None,
        };

        // The Space. A fork pull request (restricted) must have its egress
        // policy enforced; a job that may carry secrets starts without one
        // where the runtime cannot enforce it (macOS VMs today), as pools do.
        let policy = if job.policy.egress == "off" {
            None
        } else {
            egress::pool_policy(crate::config::EgressMode::DenyPrivate, &[], &[])
        };
        let mut spec = SpaceSpec {
            name: format!("cua-ci-{}", Self::short(&job.job_id)),
            image: img.image.clone(),
            on: self.on.clone(),
            cpus: img.cpus,
            memory_mb: img.memory_mb,
            disk_gb: img.disk_gb,
            labels: BTreeMap::from([
                (SPACE_LABEL_POOL.to_string(), self.owner_label()),
                (SPACE_LABEL_SLOT.to_string(), job.job_id.clone()),
            ]),
            egress: policy.clone(),
            timeout: CREATE_TIMEOUT,
            create_id: format!("cua-ci:{}:{}", self.machine_id, job.job_id),
            mounts: Vec::new(),
        };
        let mut created = self.spaces.create(&spec).await;
        if let Err(SpacesError::EgressUnsupported(why)) = &created {
            if job.restricted || job.fork {
                return JobOutcome::Released {
                    detail: format!(
                        "a fork pull request needs an enforced egress policy and this computer \
                         cannot enforce one: {why}"
                    ),
                };
            }
            spec.egress = None;
            created = self.spaces.create(&spec).await;
        }
        let row = match created {
            Ok(r) => r,
            Err(SpacesError::LimitExceeded(m)) => {
                return JobOutcome::Released {
                    detail: format!("no room for another Space: {m}"),
                };
            }
            Err(e) => {
                return JobOutcome::Failed {
                    infra: true,
                    detail: format!("creating the Space: {e}"),
                    usage: tl.usage(job, &img, crate::unix_ms()),
                };
            }
        };
        tl.boot_ms = Some(crate::unix_ms().saturating_sub(tl.vm_started_ms));

        let outcome = self.in_space(job, handle, &row.id, &spec, &mut tl).await;

        // Teardown, always.
        let _ = self.spaces.delete(&row.id).await.or_else(|e| match e {
            SpacesError::NotFound(_) => Ok(()),
            other => {
                tracing::warn!(space = %row.id, "deleting the Space: {other}");
                Err(other)
            }
        });
        let end = crate::unix_ms();
        match outcome {
            Inner::Done { result } => JobOutcome::Finished {
                result,
                usage: tl.usage(job, &img, end),
            },
            Inner::Failed { infra, detail } => JobOutcome::Failed {
                infra,
                detail,
                usage: tl.usage(job, &img, end),
            },
            Inner::Released { detail } => JobOutcome::Released { detail },
        }
    }
}

enum Inner {
    Done { result: String },
    Failed { infra: bool, detail: String },
    Released { detail: String },
}

fn infra(detail: impl Into<String>) -> Inner {
    Inner::Failed {
        infra: true,
        detail: detail.into(),
    }
}

impl SpaceJobRunner {
    async fn in_space(
        &self,
        job: &ClaimedJob,
        handle: &JobHandle,
        space: &str,
        spec: &SpaceSpec,
        tl: &mut Timeline,
    ) -> Inner {
        // What the guest is.
        let probe = match self.spaces.sh(space, guest::PROBE, SH_TIMEOUT).await {
            Ok(o) if o.ok() => match guest::parse_probe(&o.stdout) {
                Ok(p) => p,
                Err(e) => return infra(format!("probing the Space: {e}")),
            },
            Ok(o) => return infra(format!("probing the Space failed: {}", o.stderr.trim())),
            Err(e) => return infra(format!("probing the Space: {e}")),
        };
        let home = probe.home.clone();

        // DNS: allow exactly the resolver the guest uses when the policy
        // blocks it, before any job code runs.
        if let Some(policy) = &spec.egress {
            let allow = egress::resolver_allowances(&probe.resolv, policy);
            if !allow.is_empty() {
                let mut p = policy.clone();
                p.allow.extend(allow);
                if let Err(e) = self.spaces.set_egress(space, &p).await {
                    return infra(format!("applying the egress policy: {e}"));
                }
            }
        }

        if !probe.runner_present {
            let Some(platform) = guest::platform(&probe.os, &probe.arch) else {
                return infra(format!("no actions runner for {} {}", probe.os, probe.arch));
            };
            let tarball = match self.tarballs.tarball(platform).await {
                Ok(t) => t,
                Err(e) => return infra(format!("fetching the actions runner: {e}")),
            };
            if let Err(e) = self
                .spaces
                .upload_file(space, &tarball.path, &guest::tarball_path(&home))
                .await
            {
                return infra(format!("copying the actions runner in: {e}"));
            }
            match self
                .spaces
                .sh(space, &guest::install_script(&home), INSTALL_TIMEOUT)
                .await
            {
                Ok(o) if o.ok() => {}
                Ok(o) => return infra(format!("unpacking the runner failed: {}", o.stderr.trim())),
                Err(e) => return infra(format!("unpacking the runner: {e}")),
            }
        }
        if let Err(e) = self
            .spaces
            .write_file(
                space,
                &guest::wrapper_path(&home),
                guest::WRAPPER.as_bytes(),
                0o755,
            )
            .await
        {
            return infra(format!("writing the runner wrapper: {e}"));
        }
        match self
            .spaces
            .sh(space, &guest::prepare_script(&home), SH_TIMEOUT)
            .await
        {
            Ok(o) if o.ok() => {}
            Ok(o) => return infra(format!("preparing the runner failed: {}", o.stderr.trim())),
            Err(e) => return infra(format!("preparing the runner: {e}")),
        }
        if handle.aborted() {
            return Inner::Released {
                detail: "stopped before the runner started".into(),
            };
        }

        // The one-job configuration: only now, so its validity window is
        // spent on the job.
        let t_register = crate::unix_ms();
        let jit = match handle.jit().await {
            Ok(j) => j,
            Err(e) => {
                return match &e {
                    crate::Error::Service(a) if matches!(a.status, 409 | 410 | 422 | 402) => {
                        Inner::Released {
                            detail: format!("the service did not give a runner: {a}"),
                        }
                    }
                    _ => infra(format!("asking for the runner configuration: {e}")),
                };
            }
        };
        if let Err(e) = self
            .spaces
            .write_file(
                space,
                &guest::jit_path(&home),
                jit.encoded_jit_config.expose().as_bytes(),
                0o600,
            )
            .await
        {
            return infra(format!("writing the runner configuration: {e}"));
        }
        if let Err(e) = self
            .spaces
            .spawn(
                space,
                "/bin/bash",
                &[guest::wrapper_path(&home)],
                &guest::runner_dir(&home),
                &guest::process_tag(&Self::short(&job.job_id)),
            )
            .await
        {
            return infra(format!("starting the runner: {e}"));
        }
        let started = std::time::Instant::now();
        let deadline = started + self.job_timeout;
        let mut announced_online = false;
        let mut running = false;
        let exit: Option<i32>;
        loop {
            tokio::select! {
                _ = handle.wait_aborted() => {
                    tl.job_ended_ms = Some(crate::unix_ms());
                    self.copy_logs(handle, space, &home, &job.job_id, jit.encoded_jit_config.expose()).await;
                    return Inner::Done { result: "cancelled".into() };
                }
                _ = tokio::time::sleep(self.poll) => {}
            }
            let st = match self
                .spaces
                .sh(space, &guest::status_script(&home), SH_TIMEOUT)
                .await
            {
                Ok(o) => guest::parse_status(&o.stdout),
                Err(e) => {
                    return infra(format!("checking the runner: {e}"));
                }
            };
            match st {
                GuestState::Exited(code) => {
                    exit = Some(code);
                    break;
                }
                GuestState::Gone => {
                    exit = None;
                    break;
                }
                GuestState::Running | GuestState::Starting => {
                    if st == GuestState::Running && !announced_online {
                        announced_online = true;
                        tl.register_ms = Some(crate::unix_ms().saturating_sub(t_register));
                        handle.state(ReportState::RunnerOnline).await;
                    }
                    if announced_online && !running && self.job_began(space, &home).await {
                        running = true;
                        tl.job_started_ms = Some(crate::unix_ms());
                        handle.state(ReportState::Running).await;
                    }
                    if !announced_online && started.elapsed() > ONLINE_TIMEOUT {
                        return infra("the runner did not start in time");
                    }
                }
            }
            if std::time::Instant::now() > deadline {
                tl.job_ended_ms = Some(crate::unix_ms());
                self.copy_logs(
                    handle,
                    space,
                    &home,
                    &job.job_id,
                    jit.encoded_jit_config.expose(),
                )
                .await;
                return Inner::Failed {
                    infra: false,
                    detail: format!(
                        "the job ran longer than {} minutes and was stopped",
                        self.job_timeout.as_secs() / 60
                    ),
                };
            }
        }
        tl.job_ended_ms = Some(crate::unix_ms());
        let summary = self
            .copy_logs(
                handle,
                space,
                &home,
                &job.job_id,
                jit.encoded_jit_config.expose(),
            )
            .await;
        match (summary.result.as_deref(), exit) {
            (Some(r), _) => Inner::Done {
                result: map_result(r),
            },
            (None, Some(0)) => infra("the runner exited without taking a job"),
            (None, Some(code)) => infra(format!(
                "the runner exited with code {code} before a result"
            )),
            (None, None) => {
                infra("the runner process disappeared (the Space was restarted or killed)")
            }
        }
    }

    /// Whether the runner's console says a job began.
    async fn job_began(&self, space: &str, home: &str) -> bool {
        let script = format!(
            "grep -q 'Running job:' {} 2>/dev/null && echo yes || echo no",
            guest::sh_quote(&guest::console_log(home))
        );
        matches!(
            self.spaces.sh(space, &script, SH_TIMEOUT).await,
            Ok(o) if o.stdout.trim() == "yes"
        )
    }

    /// Copies the runner's console log out, sends its lines (without the
    /// runner configuration), removes the copy.
    async fn copy_logs(
        &self,
        handle: &JobHandle,
        space: &str,
        home: &str,
        job_id: &str,
        secret: &str,
    ) -> guest::JobSummary {
        let dir = self.paths.slot_logs("hosted", &Self::short(job_id));
        if cua_home::guard_write(&dir).is_err() || std::fs::create_dir_all(&dir).is_err() {
            return Default::default();
        }
        let copied = tokio::time::timeout(
            Duration::from_secs(120),
            self.spaces.download(space, &guest::console_log(home), &dir),
        )
        .await;
        let text = match copied {
            Ok(Ok(())) => std::fs::read_to_string(dir.join("cua-run.log")).unwrap_or_default(),
            _ => String::new(),
        };
        let _ = std::fs::remove_dir_all(&dir);
        let summary = guest::parse_console(&text);
        let now = rfc3339(crate::unix_ms());
        let lines: Vec<LogLine> = text
            .lines()
            .map(|l| LogLine {
                at: now.clone(),
                level: "info".into(),
                text: redact(l, secret),
            })
            .collect();
        if !lines.is_empty() {
            handle.log(lines).await;
        }
        summary
    }
}

/// `Succeeded`, `Failed`, `Canceled` as the service's words.
fn map_result(r: &str) -> String {
    match r.to_ascii_lowercase().as_str() {
        "succeeded" | "success" => "success",
        "canceled" | "cancelled" => "cancelled",
        _ => "failure",
    }
    .into()
}

/// A log line without the runner configuration.
pub fn redact(line: &str, secret: &str) -> String {
    if secret.len() >= 8 && line.contains(secret) {
        line.replace(secret, "[redacted]")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_map_to_the_services_words() {
        assert_eq!(map_result("Succeeded"), "success");
        assert_eq!(map_result("Failed"), "failure");
        assert_eq!(map_result("Canceled"), "cancelled");
        assert_eq!(map_result("weird"), "failure");
    }

    #[test]
    fn the_runner_configuration_never_reaches_a_log() {
        let secret = "ENCODEDJITCONFIGVALUE";
        assert_eq!(
            redact("config: ENCODEDJITCONFIGVALUE end", secret),
            "config: [redacted] end"
        );
        assert_eq!(redact("nothing here", secret), "nothing here");
        // A short value is not matched (it could appear in ordinary text).
        assert_eq!(redact("abc", "abc"), "abc");
    }

    #[test]
    fn space_names_are_short_and_plain() {
        assert_eq!(
            SpaceJobRunner::short("job_01HZX-ABCDEF123456789"),
            "01HZXABCDEF1"
        );
        assert_eq!(SpaceJobRunner::short("job_"), "");
    }

    #[test]
    fn defaults_per_class() {
        assert_eq!(
            ClassImage::default_for(Class::Macos).image,
            DEFAULT_MACOS_IMAGE
        );
        assert_eq!(
            ClassImage::default_for(Class::Linux).image,
            DEFAULT_LINUX_IMAGE
        );
    }

    // ---- the whole job, against in-memory Spaces and service ----

    use crate::hosted::ServiceApi;
    use crate::testing::{FakeRunner, FakeSpaces, FakeTarballs};
    use std::sync::Mutex;
    use tokio::sync::watch;

    const SECRET: &str = "ENCODED-JIT-CONFIG-SECRET";

    #[derive(Default)]
    struct Svc {
        reports: Mutex<Vec<Report>>,
        logs: Mutex<Vec<LogUpload>>,
        jits: Mutex<u32>,
        jit_error: Mutex<Option<ApiError>>,
    }

    #[async_trait]
    impl ServiceApi for Svc {
        async fn register(
            &self,
            _: &str,
            _: &MachineRegistration,
        ) -> crate::Result<MachineRegistered> {
            Ok(MachineRegistered::default())
        }
        async fn claim(&self, _: &ClaimRequest) -> crate::Result<ClaimResponse> {
            Ok(ClaimResponse::default())
        }
        async fn jit(&self, _: &str, _: &JitRequest) -> crate::Result<JitReply> {
            *self.jits.lock().unwrap() += 1;
            if let Some(e) = self.jit_error.lock().unwrap().clone() {
                return Err(crate::Error::Service(e));
            }
            Ok(serde_json::from_str(&format!(
                r#"{{"encoded_jit_config":"{SECRET}","runner_id":9,"runner_name":"r","labels":["cua"],"expires_at":""}}"#
            ))
            .unwrap())
        }
        async fn report(&self, _: &str, r: &Report) -> crate::Result<()> {
            self.reports.lock().unwrap().push(r.clone());
            Ok(())
        }
        async fn upload_logs(&self, _: &str, up: &LogUpload) -> crate::Result<()> {
            self.logs.lock().unwrap().push(up.clone());
            Ok(())
        }
    }

    fn job(class: Class, fork: bool) -> ClaimedJob {
        serde_json::from_value(serde_json::json!({
            "job_id": "job_01HZX", "lease_id": "lease_1", "job_key": "1/2", "repo": "acme/app",
            "requested_class": "any", "class": class.as_str(), "arch": "arm64",
            "labels": ["cua"], "runner_name": "cua-1", "fork": fork, "restricted": fork,
            "policy": if fork {
                serde_json::json!({"secrets": false, "cache": "ro", "egress": "deny-private"})
            } else {
                serde_json::json!({"secrets": true, "cache": "rw", "egress": "deny-private"})
            }
        }))
        .unwrap()
    }

    struct Rig {
        spaces: Arc<FakeSpaces>,
        svc: Arc<Svc>,
        runner: SpaceJobRunner,
        _dir: tempfile::TempDir,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Arc::new(FakeSpaces::new());
        let svc = Arc::new(Svc::default());
        let runner = SpaceJobRunner::new(
            spaces.clone(),
            Arc::new(FakeTarballs::new(dir.path())),
            CiPaths::new(dir.path().join("ci")),
            "m1",
            vec![
                ClassImage::default_for(Class::Linux),
                ClassImage::default_for(Class::Macos),
            ],
            "local",
        )
        .with_poll(Duration::from_millis(5));
        Rig {
            spaces,
            svc,
            runner,
            _dir: dir,
        }
    }

    fn handle(r: &Rig, j: &ClaimedJob) -> (JobHandle, watch::Sender<bool>) {
        let (tx, rx) = watch::channel(false);
        let h = JobHandle::new(
            r.svc.clone(),
            "m1",
            j,
            rx,
            Arc::new(Mutex::new(String::new())),
        );
        (h, tx)
    }

    /// Finishes the guest's runner once it has started.
    fn finish_when_started(spaces: Arc<FakeSpaces>, console: &'static str, code: i32) {
        tokio::spawn(async move {
            for _ in 0..600 {
                let ids = spaces.ids();
                if let Some(id) = ids.first()
                    && spaces.get(id).runner == FakeRunner::Running
                {
                    spaces.edit(id, |s| {
                        s.console = console.to_string();
                        s.runner = FakeRunner::Exited(code);
                    });
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
    }

    #[tokio::test]
    async fn a_job_runs_in_a_fresh_space_that_is_deleted_and_the_config_stays_secret() {
        let r = rig();
        let j = job(Class::Linux, false);
        let (h, _tx) = handle(&r, &j);
        finish_when_started(
            r.spaces.clone(),
            "Running job: build\nconfig ENCODED-JIT-CONFIG-SECRET leaked?\nJob build completed with result: Succeeded\n",
            0,
        );
        let out = r.runner.run(&j, &h).await;
        let JobOutcome::Finished { result, usage } = out else {
            panic!("{out:?}");
        };
        assert_eq!(result, "success");
        assert_eq!(usage.class, Class::Linux);
        assert_eq!(usage.start_mode, "cold_create");
        assert!(usage.phases.vm_boot_ms.is_some());
        // The Space is gone, and it was the job's own.
        assert!(r.spaces.ids().is_empty());
        // The configuration went in through the file API, mode 0600, once,
        // and only after the runner was installed.
        assert_eq!(*r.svc.jits.lock().unwrap(), 1);
        let calls = r.spaces.calls();
        let jit_write = calls
            .iter()
            .position(|c| matches!(c, crate::testing::Call::Write(_, p, 0o600) if p.ends_with(".cua-jitconfig")))
            .expect("jit file written 0600");
        let install = calls
            .iter()
            .position(|c| matches!(c, crate::testing::Call::Sh(_, s) if s.starts_with("# cua-ci:install")))
            .expect("runner installed");
        assert!(install < jit_write);
        // No log line carries the configuration.
        let logs = r.svc.logs.lock().unwrap();
        let all: String = logs
            .iter()
            .flat_map(|u| u.lines.iter().map(|l| l.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(all.contains("Running job: build"), "{all}");
        assert!(all.contains("[redacted]"), "{all}");
        assert!(!all.contains(SECRET), "{all}");
    }

    #[tokio::test]
    async fn a_failed_job_reports_failure_and_a_missing_result_is_an_infrastructure_failure() {
        let r = rig();
        let j = job(Class::Linux, false);
        let (h, _tx) = handle(&r, &j);
        finish_when_started(
            r.spaces.clone(),
            "Job build completed with result: Failed\n",
            0,
        );
        let JobOutcome::Finished { result, .. } = r.runner.run(&j, &h).await else {
            panic!()
        };
        assert_eq!(result, "failure");

        let r = rig();
        let (h, _tx) = handle(&r, &j);
        finish_when_started(r.spaces.clone(), "", 0);
        let out = r.runner.run(&j, &h).await;
        let JobOutcome::Failed { infra, detail, .. } = out else {
            panic!("{out:?}")
        };
        assert!(infra, "{detail}");
        assert!(r.spaces.ids().is_empty());
    }

    #[tokio::test]
    async fn an_abort_ends_the_job_as_cancelled_and_deletes_the_space() {
        let r = rig();
        let j = job(Class::Linux, false);
        let (h, tx) = handle(&r, &j);
        let spaces = r.spaces.clone();
        tokio::spawn(async move {
            for _ in 0..600 {
                if spaces
                    .ids()
                    .first()
                    .is_some_and(|id| spaces.get(id).runner == FakeRunner::Running)
                {
                    let _ = tx.send(true);
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let JobOutcome::Finished { result, .. } = r.runner.run(&j, &h).await else {
            panic!()
        };
        assert_eq!(result, "cancelled");
        assert!(r.spaces.ids().is_empty());
    }

    #[tokio::test]
    async fn a_fork_job_where_egress_cannot_be_enforced_is_released_not_run() {
        let r = rig();
        let j = job(Class::Macos, true);
        let (h, _tx) = handle(&r, &j);
        let out = r.runner.run(&j, &h).await;
        let JobOutcome::Released { detail } = out else {
            panic!("{out:?}")
        };
        assert!(detail.contains("egress"), "{detail}");
        assert_eq!(*r.svc.jits.lock().unwrap(), 0);
        assert!(r.spaces.ids().is_empty());
    }

    #[tokio::test]
    async fn a_trusted_macos_job_starts_without_egress_enforcement() {
        let r = rig();
        let j = job(Class::Macos, false);
        let (h, _tx) = handle(&r, &j);
        finish_when_started(
            r.spaces.clone(),
            "Job x completed with result: Succeeded\n",
            0,
        );
        let out = r.runner.run(&j, &h).await;
        assert!(matches!(out, JobOutcome::Finished { .. }), "{out:?}");
    }

    #[tokio::test]
    async fn no_room_and_a_refused_runner_release_the_job() {
        let r = rig();
        r.spaces
            .create_errors
            .lock()
            .unwrap()
            .push_back(SpacesError::LimitExceeded("two macOS VMs".into()));
        let j = job(Class::Linux, false);
        let (h, _tx) = handle(&r, &j);
        assert!(matches!(
            r.runner.run(&j, &h).await,
            JobOutcome::Released { .. }
        ));
        assert_eq!(*r.svc.jits.lock().unwrap(), 0);

        let r = rig();
        *r.svc.jit_error.lock().unwrap() = Some(ApiError {
            status: 409,
            code: "conflict".into(),
            detail: "already registered".into(),
            ..Default::default()
        });
        let (h, _tx) = handle(&r, &j);
        let out = r.runner.run(&j, &h).await;
        assert!(matches!(out, JobOutcome::Released { .. }), "{out:?}");
        // Released after the Space was made: it is deleted.
        assert!(r.spaces.ids().is_empty());
    }

    #[tokio::test]
    async fn a_class_the_machine_does_not_serve_is_released() {
        let dir = tempfile::tempdir().unwrap();
        let spaces = Arc::new(FakeSpaces::new());
        let runner = SpaceJobRunner::new(
            spaces,
            Arc::new(FakeTarballs::new(dir.path())),
            CiPaths::new(dir.path().join("ci")),
            "m1",
            vec![ClassImage::default_for(Class::Linux)],
            "local",
        );
        let svc = Arc::new(Svc::default());
        let j = job(Class::Macos, false);
        let (_tx, rx) = watch::channel(false);
        let h = JobHandle::new(svc, "m1", &j, rx, Arc::new(Mutex::new(String::new())));
        assert!(matches!(
            runner.run(&j, &h).await,
            JobOutcome::Released { .. }
        ));
    }

    #[tokio::test]
    async fn sweep_deletes_only_this_machines_leftovers() {
        let r = rig();
        for (name, owner) in [("a", "hosted-m1"), ("b", "hosted-other"), ("c", "pool")] {
            r.spaces.insert(crate::spaces::SpaceRow {
                id: format!("local:{name}"),
                name: name.into(),
                labels: BTreeMap::from([(SPACE_LABEL_POOL.to_string(), owner.to_string())]),
                ..Default::default()
            });
        }
        assert_eq!(r.runner.sweep().await.unwrap(), 1);
    }
}
