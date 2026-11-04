use super::*;
use crate::perf::{PerfConfig, StartMode};
use crate::pool::SystemClock;
use crate::service_api::{
    ClaimRequest, ClaimResponse, LogUpload, MachineRegistered, MachineRegistration, Report,
};
use crate::testing::{Call, FakeRunner, FakeSpaces, FakeTarballs, ManualClock};

const SECRET: &str = "HOSTED-JIT-CONFIG-SECRET";

#[derive(Default)]
struct Svc {
    reports: Mutex<Vec<Report>>,
    jits: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl ServiceApi for Svc {
    async fn register(&self, _: &str, _: &MachineRegistration) -> Result<MachineRegistered> {
        Ok(MachineRegistered::default())
    }
    async fn claim(&self, _: &ClaimRequest) -> Result<ClaimResponse> {
        Ok(ClaimResponse::default())
    }
    async fn jit(&self, job_id: &str, req: &JitRequest) -> Result<crate::service_api::JitReply> {
        self.jits
            .lock()
            .unwrap()
            .push((job_id.to_string(), req.lease_id.clone()));
        Ok(serde_json::from_str(&format!(
            r#"{{"encoded_jit_config":"{SECRET}","runner_id":9,"runner_name":"cua-from-service","labels":["cua"],"expires_at":""}}"#
        ))
        .unwrap())
    }
    async fn report(&self, _: &str, r: &Report) -> Result<()> {
        self.reports.lock().unwrap().push(r.clone());
        Ok(())
    }
    async fn upload_logs(&self, _: &str, _: &LogUpload) -> Result<()> {
        Ok(())
    }
}

struct Repos;

#[async_trait]
impl RepoDirectory for Repos {
    async fn repos(&self) -> Option<HashMap<String, RepoInfo>> {
        Some(HashMap::from([(
            "acme/app".to_string(),
            RepoInfo {
                private: true,
                default_branch: "main".into(),
            },
        )]))
    }
}

fn job(fork: bool) -> ClaimedJob {
    serde_json::from_value(serde_json::json!({
        "job_id": "job_01HZX", "lease_id": "lease_1", "job_key": "1/2", "repo": "Acme/App",
        "event": if fork { "pull_request" } else { "push" },
        "ref": if fork { "refs/pull/7/merge" } else { "refs/heads/main" },
        "requested_class": "any", "class": "macos", "arch": "arm64",
        "labels": ["self-hosted", "macOS", "ARM64", "cua"], "runner_name": "cua-from-claim",
        "fork": fork, "restricted": fork, "queued_at": "2027-01-15T07:59:59.600Z",
        "policy": if fork {
            serde_json::json!({"secrets": false, "cache": "ro", "egress": "deny-private"})
        } else {
            serde_json::json!({"secrets": true, "cache": "rw", "egress": "deny-private"})
        }
    }))
    .unwrap()
}

struct Rig {
    _dir: tempfile::TempDir,
    spaces: Arc<FakeSpaces>,
    svc: Arc<Svc>,
    pool: Arc<Pool>,
    runner: PerfJobRunner,
}

fn rig(max: u32, warm: u32) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let spaces = Arc::new(FakeSpaces::new());
    let svc = Arc::new(Svc::default());
    let perf = Arc::new(PerfRuntime::new(
        PerfConfig {
            enabled: true,
            ..Default::default()
        },
        dir.path().join("perf"),
        crate::unix_ms(),
    ));
    let cfg = hosted_pool_config(
        &hosted_pool_name("m1-ABC", Class::Macos),
        Class::Macos,
        "ghcr.io/trycua/macos:26",
        "local",
        max,
        warm,
    )
    .unwrap();
    let pool = hosted_pool(
        cfg,
        CiPaths::new(dir.path().join("ci")),
        spaces.clone(),
        Arc::new(FakeTarballs::new(dir.path())),
        Arc::new(SystemClock),
        None,
        perf,
    )
    .unwrap();
    let runner = PerfJobRunner::new(svc.clone(), "m1", "acct_1")
        .with_pool(Class::Macos, pool.clone())
        .with_repos(Arc::new(Repos))
        .with_poll(Duration::from_millis(5));
    Rig {
        _dir: dir,
        spaces,
        svc,
        pool,
        runner,
    }
}

fn handle(r: &Rig, j: &ClaimedJob) -> (JobHandle, watch::Sender<bool>) {
    let (tx, rx) = watch::channel(false);
    (
        JobHandle::new(
            r.svc.clone(),
            "m1",
            j,
            rx,
            Arc::new(Mutex::new(String::new())),
        ),
        tx,
    )
}

/// Drives the pool's loop by hand and ends the guest's job once the runner
/// has started.
fn drive(pool: Arc<Pool>, spaces: Arc<FakeSpaces>, finish: Option<&'static str>) {
    tokio::spawn(async move {
        for _ in 0..800 {
            pool.tick().await;
            if let Some(console) = finish {
                for id in spaces.ids() {
                    if spaces.get(&id).runner == FakeRunner::Running {
                        spaces.edit(&id, |s| {
                            s.console = console.to_string();
                            s.runner = FakeRunner::Exited(0);
                        });
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
}

const OK: &str = "Running job: build\nJob build completed with result: Succeeded\n";

#[test]
fn hosted_pool_names_and_config_are_valid() {
    let n = hosted_pool_name("Mac-Studio_1234567890ABCDEF", Class::Macos);
    assert_eq!(n, "hosted-macos-macstudio123");
    assert!(crate::config::valid_pool_name(&n));
    let cfg =
        hosted_pool_config(&n, Class::Linux, "ghcr.io/trycua/linux:24.04", "auto", 4, 1).unwrap();
    assert_eq!(cfg.guest_os(), GuestOs::Linux);
    assert_eq!((cfg.max_runners, cfg.effective_warm()), (4, 1));
    assert!(hosted_pool_config("Bad Name", Class::Macos, "x", "local", 1, 0).is_err());
    assert!(hosted_pool_config(&n, Class::Macos, "x", "somewhere", 1, 0).is_err());
}

#[tokio::test]
async fn the_claim_carries_what_the_service_and_the_directory_say() {
    let r = rig(2, 0);
    let c = r.runner.claim_of(&job(false)).await;
    assert_eq!(c.job_id, "job_01HZX");
    assert_eq!(c.lease.as_deref(), Some("lease_1"));
    assert_eq!(c.account, "acct_1");
    assert_eq!(c.event.as_deref(), Some("push"));
    assert_eq!(c.git_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(
        c.default_branch.as_deref(),
        Some("main"),
        "from the directory, case-insensitive"
    );
    assert_eq!(c.private, Some(true));
    assert_eq!(c.fork, Some(false));
    assert_eq!(c.cache_policy, Some(CachePolicy::Rw));
    assert_eq!(c.class, Some(GuestOs::Macos));
    assert_eq!(c.runner_name.as_deref(), Some("cua-from-claim"));
    assert_eq!(c.queued_at_ms, Some(1_800_000_000_000 - 400));
    assert!(!c.require_egress && !c.egress_off && c.jit.is_some());
    // The cache rules read it as a default-branch push.
    assert_eq!(
        crate::perf::cachevol::trust_of(&c.meta()),
        crate::perf::cachevol::Trust::DefaultBranch
    );

    let f = r.runner.claim_of(&job(true)).await;
    assert_eq!(f.fork, Some(true));
    assert_eq!(f.cache_policy, Some(CachePolicy::Ro));
    assert!(f.require_egress);
    assert_eq!(
        crate::perf::cachevol::trust_of(&f.meta()),
        crate::perf::cachevol::Trust::Fork
    );

    // A repository the directory does not know: no default branch, so no cache writes.
    let mut j = job(false);
    j.repo = "other/thing".into();
    let u = r.runner.claim_of(&j).await;
    assert!(u.default_branch.is_none() && u.private.is_none());
    assert_eq!(
        crate::perf::cachevol::trust_of(&u.meta()),
        crate::perf::cachevol::Trust::Unknown
    );
}

#[tokio::test]
async fn what_the_claim_says_about_the_repository_beats_the_directory() {
    let r = rig(2, 0);
    let mut j = job(false);
    j.repo = "other/thing".into();
    j.default_branch = "trunk".into();
    j.private = Some(false);
    j.head_repo = "someone/thing".into();
    let c = r.runner.claim_of(&j).await;
    assert_eq!(c.default_branch.as_deref(), Some("trunk"));
    assert_eq!(c.private, Some(false));
    assert_eq!(c.head_repo.as_deref(), Some("someone/thing"));
}

#[tokio::test]
async fn a_job_runs_on_a_warm_vm_and_the_outcome_carries_the_phases() {
    let r = rig(2, 1);
    r.pool.tick_and_provision().await;
    drive(r.pool.clone(), r.spaces.clone(), Some(OK));
    let j = job(false);
    let (h, _tx) = handle(&r, &j);
    let out = r.runner.run(&j, &h).await;
    let JobOutcome::Finished { result, usage } = out else {
        panic!("{out:?}");
    };
    assert_eq!(result, "success");
    assert_eq!(usage.start_mode, StartMode::WarmPool.as_str());
    assert_eq!(usage.class, Class::Macos);
    assert!(usage.phases.vm_boot_ms.is_some() && usage.phases.runner_register_ms.is_some());
    assert!(usage.phases.teardown_ms.is_some());
    // The service's JIT call was made once, for this lease; the configuration
    // went into the VM through the file API.
    assert_eq!(
        *r.svc.jits.lock().unwrap(),
        vec![("job_01HZX".to_string(), "lease_1".to_string())]
    );
    assert!(
        r.spaces
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Write(_, p, 0o600) if p.ends_with(".cua-jitconfig")))
    );
    // The states the service wants were reported.
    let states: Vec<_> = r
        .svc
        .reports
        .lock()
        .unwrap()
        .iter()
        .map(|x| x.state)
        .collect();
    assert!(
        states.contains(&ReportState::Booting) && states.contains(&ReportState::RunnerOnline),
        "{states:?}"
    );
    // Nothing of the job is left, and the secret never reached an event.
    assert!(r.pool.slots().iter().all(|s| s.perf.job.is_none()));
    let events = std::fs::read_to_string(r.pool.events().path()).unwrap();
    assert!(!events.contains(SECRET));
}

#[tokio::test]
async fn a_full_machine_gives_the_job_back() {
    let r = rig(1, 0);
    // The only VM is busy with another job.
    let mut other = r.runner.claim_of(&job(false)).await;
    other.job_key = "9/9".into();
    r.pool.claim(other).await.unwrap();
    let j = job(false);
    let (h, _tx) = handle(&r, &j);
    let out = r.runner.run(&j, &h).await;
    assert!(matches!(out, JobOutcome::Released { .. }), "{out:?}");
    assert!(
        r.svc.jits.lock().unwrap().len() == 1,
        "only the other job asked for a configuration"
    );
}

#[tokio::test]
async fn a_fork_pull_request_is_refused_where_egress_cannot_be_enforced() {
    // macOS VMs (Lume) refuse egress policies.
    let r = rig(2, 1);
    r.pool.tick_and_provision().await;
    let j = job(true);
    let (h, _tx) = handle(&r, &j);
    let out = r.runner.run(&j, &h).await;
    let JobOutcome::Released { detail } = out else {
        panic!("{out:?}");
    };
    assert!(detail.contains("egress"), "{detail}");
    assert!(
        r.svc.jits.lock().unwrap().is_empty(),
        "no configuration was minted for it"
    );
    assert!(
        r.pool.drain_released().iter().all(|x| x.job_key != "1/2"),
        "answered once"
    );
}

#[tokio::test]
async fn an_abort_stops_the_vm_and_the_job_is_not_the_infrastructures_fault() {
    let r = rig(2, 0);
    drive(r.pool.clone(), r.spaces.clone(), None);
    let j = job(false);
    let (h, tx) = handle(&r, &j);
    let spaces = r.spaces.clone();
    tokio::spawn(async move {
        // Once the runner has started, the service aborts the job.
        for _ in 0..400 {
            if spaces
                .ids()
                .iter()
                .any(|i| spaces.get(i).runner == FakeRunner::Running)
            {
                let _ = tx.send(true);
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    });
    let out = r.runner.run(&j, &h).await;
    let JobOutcome::Failed { infra, detail, .. } = out else {
        panic!("{out:?}");
    };
    assert!(!infra, "{detail}");
    assert!(r.spaces.ids().is_empty(), "the VM is deleted");
}

#[tokio::test]
async fn a_class_the_machine_does_not_serve_is_given_back() {
    let r = rig(1, 0);
    let mut j = job(false);
    j.class = Class::Linux;
    let (h, _tx) = handle(&r, &j);
    let out = r.runner.run(&j, &h).await;
    assert!(matches!(out, JobOutcome::Released { .. }), "{out:?}");
}

#[test]
fn outcomes_follow_how_the_slot_ended() {
    use crate::perf::timers::JobTimeline;
    let j = job(false);
    let end = |reason: &str, ran: bool, result: Option<&str>| JobEnd {
        reason: reason.into(),
        ran,
        runner_result: result.map(String::from),
        exit_code: None,
        timeline: JobTimeline::default(),
        claimed_ms: Some(1_000),
        created_ms: 500,
        start_mode: Some(StartMode::Fork),
    };
    let ms = 2_000;
    assert!(
        matches!(PerfJobRunner::outcome(&j, Some(end("job_done", true, Some("Succeeded"))), false, ms),
        JobOutcome::Finished { ref result, .. } if result == "success")
    );
    assert!(
        matches!(PerfJobRunner::outcome(&j, Some(end("job_done", true, Some("Failed"))), false, ms),
        JobOutcome::Finished { ref result, .. } if result == "failure")
    );
    assert!(
        matches!(PerfJobRunner::outcome(&j, Some(end("runner_exited", true, None)), false, ms),
        JobOutcome::Finished { ref result, .. } if result == "failure")
    );
    assert!(matches!(
        PerfJobRunner::outcome(&j, Some(end("job_timeout", true, None)), false, ms),
        JobOutcome::Failed { infra: false, .. }
    ));
    for lost in ["runner_crashed", "guest_unreachable", "space_lost"] {
        assert!(
            matches!(
                PerfJobRunner::outcome(&j, Some(end(lost, true, None)), false, ms),
                JobOutcome::Failed { infra: true, .. }
            ),
            "{lost}"
        );
    }
    assert!(matches!(
        PerfJobRunner::outcome(&j, Some(end("start_failed", false, None)), false, ms),
        JobOutcome::Released { .. }
    ));
    assert!(matches!(
        PerfJobRunner::outcome(&j, Some(end("job_done", true, None)), true, ms),
        JobOutcome::Failed { infra: false, .. }
    ));
    assert!(matches!(
        PerfJobRunner::outcome(&j, None, false, ms),
        JobOutcome::Failed { infra: true, .. }
    ));
    // The usage starts at the claim (a warm VM's idle minutes are not the job's).
    let JobOutcome::Finished { usage, .. } = PerfJobRunner::outcome(
        &j,
        Some(end("job_done", true, Some("Succeeded"))),
        false,
        ms,
    ) else {
        panic!()
    };
    assert_eq!(usage.vm_started_at, rfc3339(1_000));
    assert_eq!(usage.start_mode, "fork");
    let _ = ManualClock::new(0);
}
