//! The pool's performance core against the in-memory fakes: warm VMs that
//! wait unregistered, a claim registering one, the Apple two-VM limit with
//! warm VMs, TTL and scale to zero, the budget, forks of the golden image,
//! the bench's phase events, the cache rules on a real tree, teardown
//! guarantees and recovery after a restart.

use super::*;
use crate::config::{HostResources, PoolConfig};
use crate::events::EventKind as K;
use crate::perf::golden::{GoldenRecord, GoldenSpec};
use crate::perf::{BudgetConfig, PerfConfig};
use crate::testing::{Call, FakeGitHub, FakeRunner, FakeSpaces, FakeTarballs, ManualClock, config};

const T0: u64 = 1_800_000_000_000;

struct H {
    _dir: tempfile::TempDir,
    paths: CiPaths,
    clock: Arc<ManualClock>,
    spaces: Arc<FakeSpaces>,
    gh: Arc<FakeGitHub>,
    tar: Arc<FakeTarballs>,
    perf: Arc<PerfRuntime>,
    pool: Pool,
}

fn perf_cfg() -> PerfConfig {
    PerfConfig {
        enabled: true,
        ..Default::default()
    }
}

impl H {
    fn new(extra: &str) -> Self {
        Self::with(extra, perf_cfg())
    }

    fn with(extra: &str, perf: PerfConfig) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let paths = CiPaths::new(dir.path().join("ci"));
        let clock = Arc::new(ManualClock::new(T0));
        let spaces = Arc::new(FakeSpaces::new());
        let gh = Arc::new(FakeGitHub::new());
        let tar = Arc::new(FakeTarballs::new(dir.path()));
        let perf = Arc::new(PerfRuntime::new(perf, dir.path().join("perf"), T0));
        let pool = Self::open(config(extra), &paths, &clock, &spaces, &gh, &tar, &perf);
        H {
            _dir: dir,
            paths,
            clock,
            spaces,
            gh,
            tar,
            perf,
            pool,
        }
    }

    fn open(
        cfg: PoolConfig,
        paths: &CiPaths,
        clock: &Arc<ManualClock>,
        spaces: &Arc<FakeSpaces>,
        gh: &Arc<FakeGitHub>,
        tar: &Arc<FakeTarballs>,
        perf: &Arc<PerfRuntime>,
    ) -> Pool {
        Pool::open(
            cfg,
            paths.clone(),
            PoolDeps {
                spaces: spaces.clone(),
                github: gh.clone(),
                tarballs: tar.clone(),
                clock: clock.clone(),
                host: Some(HostResources {
                    cpus: 20,
                    memory_mb: 65536,
                }),
            },
        )
        .unwrap()
        .with_perf(perf.clone())
    }

    fn reopen(&self) -> Pool {
        Self::open(
            self.pool.config().clone(),
            &self.paths,
            &self.clock,
            &self.spaces,
            &self.gh,
            &self.tar,
            &self.perf,
        )
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
    }

    fn events(&self) -> Vec<Event> {
        self.pool.events().read()
    }

    fn of(&self, k: K) -> Vec<Event> {
        self.events().into_iter().filter(|e| e.kind == k).collect()
    }

    /// VMs created: a macOS VM is asked for twice when the egress policy it
    /// asked for cannot be enforced (the retry has the same name).
    fn creates(&self) -> usize {
        self.spaces
            .calls()
            .iter()
            .filter_map(|c| match c {
                Call::Create(n) => Some(n.clone()),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    }

    fn phases(&self) -> Vec<Phase> {
        self.pool.slots().iter().map(|s| s.phase).collect()
    }

    async fn warm_up(&self) {
        self.pool.tick_and_provision().await;
    }

    fn slot_with(&self, phase: Phase) -> Slot {
        self.pool
            .slots()
            .into_iter()
            .find(|s| s.phase == phase)
            .unwrap_or_else(|| panic!("no {phase:?} slot in {:?}", self.phases()))
    }
}

fn job(key: &str, event: &str, git_ref: &str) -> JobClaim {
    JobClaim {
        job_key: key.into(),
        lease: Some(format!("lease-{key}")),
        repo: "acme/app".into(),
        account: "acct_1".into(),
        event: Some(event.into()),
        git_ref: Some(git_ref.into()),
        default_branch: Some("main".into()),
        private: Some(true),
        class: Some(GuestOs::Macos),
        arch: "arm64".into(),
        labels: vec![
            "self-hosted".into(),
            "macOS".into(),
            "ARM64".into(),
            "cua".into(),
        ],
        runner_name: Some(format!("cua-{}", key.replace('/', "-"))),
        queued_at_ms: Some(T0 - 400),
        ..Default::default()
    }
}

fn main_job(key: &str) -> JobClaim {
    job(key, "push", "refs/heads/main")
}

// ---- the warm pool ----------------------------------------------------------

#[tokio::test]
async fn warm_vms_are_prepared_but_not_registered() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    assert_eq!(h.phases(), vec![Phase::Warm]);
    assert!(h.gh.ids().is_empty(), "nothing registered at GitHub");
    assert!(h.gh.jit_requests.lock().unwrap().is_empty());
    let s = h.slot_with(Phase::Warm);
    let fs = h.spaces.get(s.space_id.as_ref().unwrap());
    assert!(fs.runner_present, "the runner is installed");
    assert!(
        fs.files
            .contains_key("/Users/lume/actions-runner/cua-run.sh"),
        "the wrapper is in place"
    );
    assert!(
        fs.files.keys().all(|k| !k.ends_with(".cua-jitconfig")),
        "no JIT config yet"
    );
    let wrapper =
        String::from_utf8(fs.files["/Users/lume/actions-runner/cua-run.sh"].0.clone()).unwrap();
    assert!(wrapper.contains("ACTIONS_RUNNER_HOOK_JOB_STARTED"));
    assert_eq!(h.of(K::WarmPool)[0].reason.as_deref(), Some("ready"));
    // A second pass changes nothing: the target is met.
    h.warm_up().await;
    assert_eq!(h.creates(), 1);
}

#[tokio::test]
async fn a_claim_registers_a_warm_vm_without_creating_anything() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    let warm = h.slot_with(Phase::Warm);
    let c = h.pool.claim(main_job("11/22")).await.unwrap();
    assert_eq!(c.start_mode, StartMode::WarmPool);
    assert_eq!(c.slot, warm.id);
    assert_eq!(c.runner_name, "cua-11-22");
    assert_eq!(h.creates(), 1, "no new VM");
    assert_eq!(h.slot_with(Phase::Idle).id, warm.id);
    // The runner registered with the name and labels the claim carries.
    let reqs = h.gh.jit_requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].name, "cua-11-22");
    assert!(reqs[0].labels.contains(&"cua".to_string()));
    // The JIT config went in through the file API, mode 0600, and the
    // runner was started.
    let fs = h.spaces.get(warm.space_id.as_ref().unwrap());
    assert_eq!(
        fs.files["/Users/lume/actions-runner/.cua-jitconfig"].1,
        0o600
    );
    assert!(
        h.spaces
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Spawn(..)))
    );

    // The phase events, in order, with the schema's fields.
    let kinds: Vec<K> = h.events().iter().map(|e| e.kind).collect();
    let pos = |k: K| kinds.iter().position(|x| *x == k).unwrap();
    assert!(pos(K::JobQueued) < pos(K::VmClaimed));
    assert!(pos(K::VmClaimed) < pos(K::VmReady));
    assert!(pos(K::VmReady) < pos(K::RunnerRegistered));
    let claimed = &h.of(K::VmClaimed)[0];
    assert_eq!(claimed.extra["start_mode"], "warm_pool");
    assert_eq!(claimed.extra["job_key"], "11/22");
    assert_eq!(claimed.extra["class"], "macos");
    assert_eq!(h.of(K::JobQueued)[0].extra["queued_at_ms"], T0 - 400);

    // The pool refills behind the claim: one busy, one warm, within Apple's two.
    h.warm_up().await;
    assert_eq!(h.creates(), 2);
    let mut p = h.phases();
    p.sort_by_key(|x| x.as_str());
    assert_eq!(p, vec![Phase::Idle, Phase::Warm]);
}

#[tokio::test]
async fn with_no_warm_vm_the_claim_creates_one_and_says_so() {
    let h = H::new("max_runners = 2\nwarm = 1");
    let c = h.pool.claim(main_job("1/1")).await.unwrap();
    assert_eq!(c.start_mode, StartMode::ColdCreate);
    assert_eq!(h.phases(), vec![Phase::Idle]);
    assert_eq!(h.of(K::VmClaimed)[0].extra["start_mode"], "cold_create");
    assert_eq!(h.of(K::VmReady).len(), 1);
    assert_eq!(h.of(K::RunnerRegistered).len(), 1);
    assert_eq!(h.gh.jit_requests.lock().unwrap()[0].name, "cua-1-1");
}

#[tokio::test]
async fn apples_two_vm_limit_holds_with_warm_vms_in_the_mix() {
    let h = H::new("max_runners = 2\nwarm = 2");
    h.warm_up().await;
    assert_eq!(h.phases(), vec![Phase::Warm, Phase::Warm]);
    h.pool.claim(main_job("1/1")).await.unwrap();
    h.pool.claim(main_job("1/2")).await.unwrap();
    assert_eq!(h.creates(), 2, "both jobs took warm VMs");
    // Two jobs run: a third has no room and no warm VM is created.
    let e = h.pool.claim(main_job("1/3")).await.unwrap_err();
    assert!(matches!(e, ClaimError::NoCapacity(_)), "{e}");
    h.warm_up().await;
    assert_eq!(h.creates(), 2);
    let cap = h.pool.capacity().await;
    assert_eq!(
        (cap.max, cap.busy, cap.warm, cap.free_slots()),
        (2, 2, 0, 0)
    );
    // One macOS VM that is not the pool's leaves room for one.
    let h2 = H::new("max_runners = 2\nwarm = 2");
    *h2.spaces.other_vms.lock().unwrap() = 1;
    h2.warm_up().await;
    assert_eq!(h2.phases(), vec![Phase::Warm]);
}

#[tokio::test]
async fn warm_vms_are_replaced_at_their_ttl_and_never_two_macos_vms_too_many() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    let first = h.slot_with(Phase::Warm).id;
    h.advance(31 * 60);
    h.perf.mark_demand(h.clock_now());
    // The pass deletes; it creates nothing (the VM still holds its slot).
    h.pool.tick().await;
    assert!(h.pool.slots().is_empty());
    assert_eq!(
        h.of(K::WarmPool)
            .iter()
            .filter(|e| e.reason.as_deref() == Some("warm_ttl"))
            .count(),
        1
    );
    assert!(h.spaces.ids().is_empty());
    h.advance(3);
    h.warm_up().await;
    let second = h.slot_with(Phase::Warm).id;
    assert_ne!(first, second);
}

#[tokio::test]
async fn an_idle_pool_scales_to_zero_and_a_queued_job_wakes_it() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    h.advance(20 * 60);
    h.pool.tick().await;
    assert!(h.pool.slots().is_empty(), "scaled to zero");
    assert!(h.spaces.ids().is_empty(), "the VM is gone, nothing leaks");
    assert_eq!(
        h.of(K::WarmPool)
            .iter()
            .filter(|e| e.reason.as_deref() == Some("scale_to_zero"))
            .count(),
        1
    );
    h.advance(3);
    h.warm_up().await;
    assert!(h.pool.slots().is_empty(), "stays at zero while idle");
    // A job queued for this computer wakes one VM.
    h.perf.set_queued_jobs(1);
    h.warm_up().await;
    assert_eq!(h.phases(), vec![Phase::Warm]);
    h.perf.set_queued_jobs(0);
    // A claim restarts the clock: after it the pool keeps a warm VM again.
    h.pool.claim(main_job("2/1")).await.unwrap();
    h.advance(60);
    h.warm_up().await;
    assert!(h.phases().contains(&Phase::Warm), "{:?}", h.phases());
}

#[tokio::test]
async fn the_budget_stops_new_vms_and_new_jobs() {
    let mut cfg = perf_cfg();
    cfg.budget = Some(BudgetConfig {
        monthly_usd: 10.0,
        usd_per_hour: 1.0,
    });
    let h = H::with("max_runners = 2\nwarm = 1", cfg);
    {
        let mut st = h.pool.state.lock().unwrap();
        st.ledger.month = crate::perf::warm::yyyymm(T0);
        st.ledger.spent_usd = 10.0;
    }
    h.warm_up().await;
    assert_eq!(h.creates(), 0, "no warm VM at the limit");
    let e = h.pool.claim(main_job("1/1")).await.unwrap_err();
    assert!(matches!(e, ClaimError::BudgetExhausted), "{e}");

    // A VM's lifetime is charged when it is torn down.
    let h = H::with("max_runners = 2\nwarm = 1", {
        let mut c = perf_cfg();
        c.budget = Some(BudgetConfig {
            monthly_usd: 100.0,
            usd_per_hour: 1.0,
        });
        c
    });
    h.warm_up().await;
    h.advance(3600);
    h.perf.mark_demand(h.clock_now());
    let id = h.slot_with(Phase::Warm).id;
    h.pool.finish(&id, "pool_removed", None).await;
    let spent = h.pool.state().ledger.spent_usd;
    assert!((spent - 1.0).abs() < 1e-6, "{spent}");
}

#[tokio::test]
async fn claims_the_pool_cannot_take_are_refused_plainly() {
    let h = H::new("max_runners = 1");
    let mut linux = main_job("1/1");
    linux.class = Some(GuestOs::Linux);
    assert!(matches!(
        h.pool.claim(linux).await.unwrap_err(),
        ClaimError::NoCapacity(_)
    ));
    // A pool without the performance core, or in eager mode, does not claim.
    let dir = tempfile::tempdir().unwrap();
    let plain = Pool::open(
        config(""),
        CiPaths::new(dir.path()),
        PoolDeps {
            spaces: Arc::new(FakeSpaces::new()),
            github: Arc::new(FakeGitHub::new()),
            tarballs: Arc::new(FakeTarballs::new(dir.path())),
            clock: Arc::new(ManualClock::new(T0)),
            host: None,
        },
    )
    .unwrap();
    assert!(matches!(
        plain.claim(main_job("1/1")).await.unwrap_err(),
        ClaimError::NotEnabled
    ));
    let mut eager = perf_cfg();
    eager.register = crate::perf::RegisterMode::Eager;
    let h = H::with("max_runners = 1", eager);
    assert!(matches!(
        h.pool.claim(main_job("1/1")).await.unwrap_err(),
        ClaimError::NotEnabled
    ));
}

// ---- the golden image ----------------------------------------------------------

fn golden(h: &H) -> GoldenRecord {
    let spec = GoldenSpec::basic(
        GuestOs::Macos,
        "arm64",
        "ghcr.io/trycua/macos:26",
        "2.338.2",
        true,
    );
    let rec = GoldenRecord {
        digest: spec.digest(),
        checkpoints: vec![spec.checkpoint_name(0), spec.checkpoint_name(1)],
        spec,
        built_ms: T0,
        verified_ms: Some(T0),
        build_ms: 1,
        fork_ready_ms: Some(1),
        versions: Default::default(),
    };
    h.spaces
        .checkpoints
        .lock()
        .unwrap()
        .extend(rec.checkpoints.clone());
    h.perf.golden.lock().unwrap().current = Some(rec.clone());
    rec
}

#[tokio::test]
async fn job_vms_are_forks_of_the_golden_checkpoint_one_variant_per_slot() {
    let h = H::new("max_runners = 2\nwarm = 2");
    let rec = golden(&h);
    h.warm_up().await;
    let slots = h.pool.slots();
    assert_eq!(slots.len(), 2);
    let mut images: Vec<String> = slots
        .iter()
        .map(|s| h.spaces.get(s.space_id.as_ref().unwrap()).spec.image)
        .collect();
    images.sort();
    assert_eq!(
        images,
        vec![
            format!("checkpoint:{}", rec.checkpoints[0]),
            format!("checkpoint:{}", rec.checkpoints[1])
        ],
        "each slot forks its own variant"
    );
    for s in &slots {
        let sp = h.spaces.get(s.space_id.as_ref().unwrap());
        assert!(
            sp.spec.cpus.is_none() && sp.spec.memory_mb.is_none() && sp.spec.disk_gb.is_none(),
            "a fork keeps the checkpoint's size"
        );
        assert_eq!(s.perf.start_mode, Some(StartMode::Fork));
        assert_eq!(s.perf.image_key.as_deref(), Some(rec.digest.as_str()));
        // The fixup ran before the probe.
        let calls = h.spaces.calls();
        let fix = calls.iter().position(|c| matches!(c, Call::Sh(i, sc) if Some(i) == s.space_id.as_ref() && sc.starts_with("# cua-ci:fixup"))).unwrap();
        let probe = calls.iter().position(|c| matches!(c, Call::Sh(i, sc) if Some(i) == s.space_id.as_ref() && sc.starts_with("# cua-ci:probe"))).unwrap();
        assert!(fix < probe);
    }
    // A claim on a fork reports it, and the warm VM is the fork.
    let c = h.pool.claim(main_job("1/1")).await.unwrap();
    assert_eq!(c.start_mode, StartMode::WarmPool);
}

#[tokio::test]
async fn a_cold_claim_forks_and_says_fork() {
    let h = H::new("max_runners = 2\nwarm = 0");
    golden(&h);
    let c = h.pool.claim(main_job("1/1")).await.unwrap();
    assert_eq!(c.start_mode, StartMode::Fork);
    assert_eq!(h.of(K::VmClaimed)[0].extra["start_mode"], "fork");
}

#[tokio::test]
async fn a_fork_that_cannot_start_falls_back_to_a_cold_create() {
    let h = H::new("max_runners = 2\nwarm = 1");
    golden(&h);
    // The engine lost the checkpoint.
    h.spaces.checkpoints.lock().unwrap().clear();
    h.warm_up().await;
    let s = h.slot_with(Phase::Warm);
    let sp = h.spaces.get(s.space_id.as_ref().unwrap());
    assert_eq!(
        sp.spec.image, "ghcr.io/trycua/macos:26",
        "started cold instead"
    );
    assert_eq!(s.perf.start_mode, Some(StartMode::ColdCreate));
    assert_eq!(h.perf.golden_state().fork_failures, 1);
    assert_eq!(h.of(K::Golden)[0].reason.as_deref(), Some("fork_failed"));
}

#[tokio::test]
async fn a_warm_vm_from_an_old_golden_image_is_replaced_not_claimed_first() {
    let h = H::new("max_runners = 2\nwarm = 1");
    let old = golden(&h);
    h.warm_up().await;
    let stale = h.slot_with(Phase::Warm).id;
    // A new golden image becomes current.
    let mut spec = old.spec.clone();
    spec.xcode = Some("26.1".into());
    let new = GoldenRecord {
        digest: spec.digest(),
        checkpoints: vec![spec.checkpoint_name(0), spec.checkpoint_name(1)],
        spec,
        ..old.clone()
    };
    h.spaces
        .checkpoints
        .lock()
        .unwrap()
        .extend(new.checkpoints.clone());
    h.perf.golden.lock().unwrap().current = Some(new.clone());
    h.pool.tick().await;
    assert!(
        h.pool.slots().iter().all(|s| s.id != stale),
        "the stale warm VM is deleted"
    );
    assert_eq!(
        h.of(K::WarmPool)
            .iter()
            .filter(|e| e.reason.as_deref() == Some("stale_image"))
            .count(),
        1
    );
    h.advance(3);
    h.warm_up().await;
    let fresh = h.slot_with(Phase::Warm);
    assert_eq!(fresh.perf.image_key.as_deref(), Some(new.digest.as_str()));
}

#[tokio::test]
async fn the_golden_cycle_builds_when_missing_and_retires_the_old_image_when_unused() {
    let h = H::new("max_runners = 2\nwarm = 1");
    let spec = GoldenSpec::basic(
        GuestOs::Macos,
        "arm64",
        "ghcr.io/trycua/macos:26",
        "2.338.2",
        true,
    );
    h.perf.set_golden_spec(spec.clone());
    // The recorded spec carries the VM size (10 vCPUs and 8 GiB here).
    let sized = |s: &GoldenSpec| {
        let mut s = s.clone();
        s.tools.insert("vm_cpus".into(), "10".into());
        s.tools.insert("vm_memory_mb".into(), "8192".into());
        s
    };
    let pool = Arc::new(h.reopen());
    let task = pool.clone().golden_task().expect("a build is due");
    task.await;
    let st = h.perf.golden_state();
    assert_eq!(st.current.as_ref().unwrap().digest, sized(&spec).digest());
    assert_eq!(
        h.of(K::Golden)
            .iter()
            .map(|e| e.reason.clone().unwrap())
            .collect::<Vec<_>>(),
        vec!["building", "built"]
    );
    assert!(h.spaces.ids().is_empty(), "no build VM left");
    // The next check is a minute away, and nothing is due then either.
    assert!(pool.clone().golden_task().is_none());
    h.advance(61);
    pool.clone()
        .golden_task()
        .expect("the check runs again")
        .await;
    assert_eq!(
        h.of(K::Golden).len(),
        2,
        "nothing is due: the image is current"
    );
    // A spec change builds the next image; the previous one is retired the
    // cycle after, once no VM runs from it.
    let mut spec2 = spec.clone();
    spec2.xcode = Some("26.1".into());
    h.perf.set_golden_spec(spec2.clone());
    h.advance(61);
    pool.clone().golden_task().unwrap().await;
    let st = h.perf.golden_state();
    assert_eq!(st.current.as_ref().unwrap().digest, sized(&spec2).digest());
    assert_eq!(st.previous.as_ref().unwrap().digest, sized(&spec).digest());
    h.advance(61);
    pool.clone().golden_task().unwrap().await;
    assert!(h.perf.golden_state().previous.is_none());
    assert!(
        !h.spaces
            .checkpoints
            .lock()
            .unwrap()
            .contains(&spec.checkpoint_name(0))
    );
}

// ---- the end of a job ----------------------------------------------------------

fn marks(t: u64) -> String {
    format!(
        "{{\"k\":\"cache_restore\",\"at\":{a},\"ms\":300,\"store\":\"cargo\",\"bytes\":4096,\"hit\":true,\"scope\":\"default_branch\"}}\n\
         {{\"k\":\"mirror\",\"used\":true}}\n\
         {{\"k\":\"job_start\",\"at\":{b},\"event\":\"push\",\"repo\":\"acme/app\",\"ref\":\"refs/heads/main\",\"bench\":\"bench-1\"}}\n\
         {{\"k\":\"job_end\",\"at\":{c}}}\n\
         ---worker\n\
         [x INFO ExecutionContext] Publish step telemetry for current step {{\"action\":\"actions/checkout\",\"displayName\":\"Run actions/checkout@v4\",\"startTime\":\"2026-10-28T12:00:01.000Z\",\"finishTime\":\"2026-10-28T12:00:02.500Z\",\"result\":\"Succeeded\"}}\n",
        a = t + 1_200,
        b = t + 1_500,
        c = t + 61_500,
    )
}

fn finish_job(h: &H, slot: &Slot, collect: String) {
    h.spaces.edit(slot.space_id.as_ref().unwrap(), |g| {
        g.runner = FakeRunner::Exited(0);
        g.console = "Running job: build\nJob build completed with result: Succeeded\n".into();
        g.collect = collect;
    });
}

#[tokio::test]
async fn a_finished_job_emits_every_phase_the_bench_reads() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    h.pool.claim(main_job("11/22")).await.unwrap();
    let slot = h.slot_with(Phase::Idle);
    h.advance(2);
    finish_job(&h, &slot, marks(h.clock_now()));
    h.advance(60);
    h.pool.tick().await;
    let kinds: Vec<K> = h.events().iter().map(|e| e.kind).collect();
    for k in [
        K::JobQueued,
        K::VmClaimed,
        K::VmReady,
        K::RunnerRegistered,
        K::JobStarted,
        K::CacheRestore,
        K::CheckoutDone,
        K::JobFinished,
        K::TeardownDone,
        K::ResourceSweep,
        K::SlotDeleted,
    ] {
        assert!(kinds.contains(&k), "{k:?} missing in {kinds:?}");
    }
    assert_eq!(
        h.of(K::VmReady)[0].extra["runner_version"],
        "2.338.2",
        "the installed runner's version"
    );
    let fin = &h.of(K::JobFinished)[0];
    assert_eq!(fin.result.as_deref(), Some("success"));
    assert_eq!(fin.extra["runner_result"], "Succeeded");
    assert_eq!(fin.extra["job_key"], "11/22");
    assert_eq!(
        fin.extra["bench_run_id"], "bench-1",
        "from the job's own env"
    );
    assert_eq!(fin.ms, Some(60_000), "start mark to end mark");
    let co = &h.of(K::CheckoutDone)[0];
    assert_eq!(co.ms, Some(1_500));
    assert_eq!(co.extra["mirror"], true);
    let cr = &h.of(K::CacheRestore)[0];
    assert_eq!(
        (cr.extra["store"].as_str(), cr.extra["hit"].as_bool(), cr.ms),
        (Some("cargo"), Some(true), Some(300))
    );
    let sweep = &h.of(K::ResourceSweep)[0];
    assert_eq!(sweep.extra["leaked_vms"], 0);
    assert!(h.of(K::TeardownDone)[0].extra["job_key"] == "11/22");
    // The Space and the runner are gone.
    assert!(h.spaces.ids().iter().all(|i| {
        h.pool
            .slots()
            .iter()
            .any(|s| s.space_id.as_ref() == Some(i))
    }));
}

#[tokio::test]
async fn the_vm_is_scanned_for_secrets_after_the_job_and_the_event_has_names_only() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    h.pool.claim(main_job("11/22")).await.unwrap();
    let slot = h.slot_with(Phase::Idle);
    h.advance(2);
    finish_job(&h, &slot, marks(h.clock_now()));
    h.spaces.edit(slot.space_id.as_ref().unwrap(), |g| {
        g.secret_scan = "found=1\ndetail=file:.aws/credentials\n".into();
    });
    h.advance(60);
    h.pool.tick().await;
    let scans = h.of(K::SecretScan);
    assert_eq!(scans.len(), 1);
    assert_eq!(scans[0].extra["found"], 1);
    assert_eq!(scans[0].detail.as_deref(), Some("file:.aws/credentials"));
    assert_eq!(scans[0].extra["job_key"], "11/22");
}

#[tokio::test]
async fn the_runners_console_log_dates_the_assignment_before_githubs_list_does() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    h.pool.claim(main_job("1/1")).await.unwrap();
    let slot = h.slot_with(Phase::Idle);
    h.advance(5);
    // 2027-01-15 08:00:03Z (the fake clock's T0 plus 3 s), as the runner's console wrote it.
    h.spaces.edit(slot.space_id.as_ref().unwrap(), |g| {
        g.runner = FakeRunner::Running;
        g.status_extra = "assigned=2027-01-15 08:00:03Z\n".into();
    });
    h.pool.tick().await;
    let s = h
        .pool
        .slots()
        .into_iter()
        .find(|s| s.id == slot.id)
        .unwrap();
    assert_eq!(
        s.phase,
        Phase::Busy,
        "busy without waiting for the GitHub poll"
    );
    assert!(h.of(K::JobAssigned).len() == 1);
}

#[tokio::test]
async fn a_space_that_cannot_be_deleted_is_retried_every_tick_and_never_forgotten() {
    let h = H::new("max_runners = 1\nwarm = 1");
    h.warm_up().await;
    h.pool.claim(main_job("1/1")).await.unwrap();
    let slot = h.slot_with(Phase::Idle);
    finish_job(&h, &slot, marks(h.clock_now()));
    *h.spaces.delete_failures.lock().unwrap() = 2;
    h.pool.tick().await;
    assert_eq!(
        h.phases(),
        vec![Phase::Ending],
        "kept: the VM may still be running"
    );
    assert_eq!(h.of(K::JobFinished).len(), 1);
    h.pool.tick().await;
    assert_eq!(h.phases(), vec![Phase::Ending]);
    h.pool.tick().await;
    assert!(
        h.pool.slots().iter().all(|s| s.phase != Phase::Ending),
        "deleted on the third try"
    );
    assert_eq!(
        h.of(K::JobFinished).len(),
        1,
        "the job's events are not repeated"
    );
    assert_eq!(h.of(K::TeardownDone).len(), 1);
    assert!(
        h.spaces
            .ids()
            .iter()
            .all(|i| *i != slot.space_id.clone().unwrap())
    );
}

#[tokio::test]
async fn a_registration_failure_gives_the_job_back_and_leaves_no_vm() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    *h.gh.jit_failures.lock().unwrap() = 1;
    let e = h.pool.claim(main_job("5/6")).await.unwrap_err();
    assert!(matches!(e, ClaimError::Failed(_)), "{e}");
    let released = h.pool.drain_released();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].job_key, "5/6");
    assert_eq!(released[0].lease.as_deref(), Some("lease-5/6"));
    assert!(released[0].infra);
    assert!(h.pool.drain_released().is_empty(), "taken once");
    assert!(h.pool.slots().iter().all(|s| s.perf.job.is_none()));
    assert!(!h.of(K::InfraFailure).is_empty());
}

#[tokio::test]
async fn a_dead_warm_vm_is_skipped_and_the_claim_goes_on() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    let s = h.slot_with(Phase::Warm);
    h.spaces
        .edit(s.space_id.as_ref().unwrap(), |g| g.unreachable = true);
    let c = h.pool.claim(main_job("1/1")).await.unwrap();
    assert_eq!(
        c.start_mode,
        StartMode::ColdCreate,
        "a fresh VM took the job"
    );
    assert_ne!(c.slot, s.id);
    assert!(
        h.pool.drain_released().is_empty(),
        "the job was never given up"
    );
    assert_eq!(
        h.of(K::SlotDeleted)
            .iter()
            .filter(|e| e.reason.as_deref() == Some("guest_unreachable"))
            .count(),
        1
    );
}

#[tokio::test]
async fn a_restart_keeps_warm_vms_and_gives_back_a_claim_it_was_making() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    let warm = h.slot_with(Phase::Warm);
    // The process dies while a claim is registering a second VM.
    h.pool.claim(main_job("1/1")).await.unwrap();
    h.warm_up().await; // refill: a second VM
    let second = h.slot_with(Phase::Warm).id;
    {
        let mut st = h.pool.state.lock().unwrap();
        let s = st.slots.iter_mut().find(|s| s.id == second).unwrap();
        s.phase = Phase::Starting;
        s.perf.job = Some(JobCtx {
            job_key: "9/9".into(),
            lease: Some("lease-9".into()),
            repo: "acme/app".into(),
            account: "acct_1".into(),
            ..Default::default()
        });
    }
    h.pool.save().unwrap();
    let again = h.reopen();
    assert!(
        again
            .slots()
            .iter()
            .any(|s| s.id == warm.id && s.phase == Phase::Idle),
        "the running job's slot stays"
    );
    again.tick().await;
    let released = again.drain_released();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].job_key, "9/9");
    assert!(
        again.slots().iter().all(|s| s.id != second),
        "its VM is deleted"
    );
}

#[tokio::test]
async fn the_tick_is_one_second_while_a_job_runs() {
    let h = H::new("max_runners = 2\nwarm = 1");
    h.warm_up().await;
    assert_eq!(h.pool.tick_interval(), TICK);
    h.pool.claim(main_job("1/1")).await.unwrap();
    assert_eq!(h.pool.tick_interval(), Duration::from_secs(1));
}

// ---- caches and the mirror -----------------------------------------------------

#[cfg(unix)]
mod caches {
    use super::*;
    use crate::perf::cachevol::{PromoteLimits, scope_key};
    use std::process::Command;

    fn tar_with(
        dir: &std::path::Path,
        name: &str,
        file: &str,
        body: &[u8],
    ) -> (std::path::PathBuf, Vec<u8>) {
        let src = dir.join(format!("src-{name}"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join(file), body).unwrap();
        let tar = dir.join(format!("{name}.tar"));
        assert!(
            Command::new("tar")
                .arg("-cf")
                .arg(&tar)
                .arg("-C")
                .arg(&src)
                .arg(".")
                .status()
                .unwrap()
                .success()
        );
        let bytes = std::fs::read(&tar).unwrap();
        (tar, bytes)
    }

    fn seed(h: &H) {
        let (tar, _) = tar_with(h._dir.path(), "seed", "old.crate", b"OLD");
        h.perf
            .caches
            .promote(
                &scope_key("acct_1", "acme/app"),
                "cargo",
                "any",
                &[("registry", &tar)],
                PromoteLimits::default(),
            )
            .unwrap();
    }

    fn linux_job(key: &str) -> JobClaim {
        let mut j = main_job(key);
        j.class = Some(GuestOs::Linux);
        j
    }

    fn files_of(h: &H, slot: &Slot) -> std::collections::BTreeMap<String, (Vec<u8>, u32)> {
        h.spaces.get(slot.space_id.as_ref().unwrap()).files
    }

    #[tokio::test]
    async fn without_mounts_the_caches_are_uploaded_and_job_env_points_at_them() {
        let h = H::new("max_runners = 2\nwarm = 1");
        seed(&h);
        h.warm_up().await;
        h.pool.claim(main_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let calls = h.spaces.calls();
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, Call::Upload(_, _, p) if p == "/Users/lume/.cua-ci/in.tar"))
        );
        let env = String::from_utf8(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(
            env.contains("CUA_CI_CACHE_ROOT='/Users/lume/.cua-ci/in/cache'"),
            "{env}"
        );
        assert!(env.contains("CUA_CI_CACHE_STORES='cargo/any'"));
        assert!(env.contains("CUA_CI_CACHE_WRITE=1"));
        assert_eq!(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].1, 0o600);
        let cr = h.of(K::CacheRead);
        let cargo = cr.iter().find(|e| e.extra["store"] == "cargo").unwrap();
        assert_eq!(cargo.extra["hit"], true);
        let pnpm = cr.iter().find(|e| e.extra["store"] == "pnpm").unwrap();
        assert_eq!(
            pnpm.extra["hit"], false,
            "a store with no cache yet is a miss"
        );
        assert_ne!(
            cargo.extra["key_hash"].as_str().unwrap(),
            "acme/app",
            "hashes only"
        );
    }

    #[tokio::test]
    async fn a_linux_job_gets_the_registry_mirrors_even_with_no_caches() {
        let h = H::new("max_runners = 2\nwarm = 1\nos = \"linux\"");
        h.warm_up().await;
        h.pool.claim(linux_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let env = String::from_utf8(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(
            env.contains("CUA_CI_REGISTRY_MIRRORS='https://mirror.gcr.io'"),
            "{env}"
        );
        assert!(
            !env.contains("CUA_CI_CACHE_ROOT"),
            "nothing to mount: {env}"
        );
        assert_eq!(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].1, 0o600);
    }

    #[tokio::test]
    async fn the_registry_line_follows_the_caches_and_is_off_when_the_list_is_empty() {
        // With caches the line is appended to the usual job.env.
        let h = H::new("max_runners = 2\nwarm = 1\nos = \"linux\"");
        seed(&h);
        h.warm_up().await;
        h.pool.claim(linux_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let env = String::from_utf8(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(env.contains("CUA_CI_CACHE_STORES='cargo/any'"), "{env}");
        assert!(env.contains("CUA_CI_REGISTRY_MIRRORS="), "{env}");
        // An empty list turns it off: with no caches there is no job.env.
        let mut perf = perf_cfg();
        perf.registry.mirrors.clear();
        let h = H::with("max_runners = 2\nwarm = 1\nos = \"linux\"", perf);
        h.warm_up().await;
        h.pool.claim(linux_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        assert!(
            !files_of(&h, &slot).contains_key("/Users/lume/.cua-ci/job.env"),
            "nothing to hand over"
        );
    }

    #[tokio::test]
    async fn a_macos_guest_never_gets_registry_mirrors() {
        let h = H::new("max_runners = 2\nwarm = 1");
        seed(&h);
        h.warm_up().await;
        h.pool.claim(main_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let env = String::from_utf8(files_of(&h, &slot)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(!env.contains("REGISTRY"), "{env}");
    }

    #[tokio::test]
    async fn with_mounts_the_vm_shares_its_slot_folder_read_only() {
        let h = H::new("max_runners = 2\nwarm = 1");
        *h.spaces.mounts.lock().unwrap() = true;
        seed(&h);
        h.warm_up().await;
        let warm = h.slot_with(Phase::Warm);
        let sp = h.spaces.get(warm.space_id.as_ref().unwrap());
        assert_eq!(sp.spec.mounts.len(), 1);
        let m = &sp.spec.mounts[0];
        assert!(m.read_only);
        assert_eq!(m.guest, format!("/Volumes/My Shared Files/{}", warm.id));
        assert_eq!(m.host, h.perf.slot_dir(&warm.id));
        h.pool.claim(main_job("1/1")).await.unwrap();
        // The job's cache was linked into the folder the VM already shares.
        assert!(
            h.perf
                .slot_dir(&warm.id)
                .join("cache/cargo/any/current/registry/old.crate")
                .is_file()
        );
        assert!(
            h.spaces
                .calls()
                .iter()
                .all(|c| !matches!(c, Call::Upload(_, _, p) if p.ends_with("in.tar")))
        );
        let env = String::from_utf8(files_of(&h, &warm)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(
            env.contains(&format!(
                "CUA_CI_CACHE_ROOT='/Volumes/My Shared Files/{}/cache'",
                warm.id
            )),
            "{env}"
        );
        // The folder goes with the VM.
        let id = warm.id.clone();
        let slot = h.slot_with(Phase::Idle);
        finish_job(&h, &slot, marks(h.clock_now()));
        h.pool.tick().await;
        assert!(!h.perf.slot_dir(&id).exists());
    }

    fn run_to_the_end(h: &H, slot: &Slot, event: &str, git_ref: &str, delta_tar: &[u8]) {
        let t = h.clock_now();
        let collect = marks(t)
            .replace("\"event\":\"push\"", &format!("\"event\":\"{event}\""))
            .replace("refs/heads/main", git_ref);
        finish_job(h, slot, collect);
        h.spaces.edit(slot.space_id.as_ref().unwrap(), |g| {
            g.delta = "dirty=cargo:registry\n".into();
            g.delta_files
                .insert("cargo--registry.tar".into(), delta_tar.to_vec());
        });
    }

    #[tokio::test]
    async fn a_default_branch_job_that_succeeded_writes_the_cache_back_and_the_next_job_reads_it() {
        let h = H::new("max_runners = 2\nwarm = 1");
        h.warm_up().await;
        h.pool.claim(main_job("1/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let (_, bytes) = tar_with(h._dir.path(), "delta", "new.crate", b"NEW!");
        run_to_the_end(&h, &slot, "push", "refs/heads/main", &bytes);
        h.pool.tick().await;
        let w = h.of(K::CacheWrite);
        assert_eq!(w.len(), 1, "{:?}", h.events());
        assert_eq!(w[0].extra["scope"], "default_branch");
        assert_eq!(w[0].extra["store"], "cargo");
        assert_eq!(w[0].extra["bytes"], 4);
        let cur = h
            .perf
            .caches
            .current(&scope_key("acct_1", "acme/app"), "cargo", "any")
            .unwrap();
        assert_eq!(
            std::fs::read(cur.join("registry/new.crate")).unwrap(),
            b"NEW!"
        );
        // The next job's VM gets it.
        h.pool.claim(main_job("1/2")).await.unwrap();
        let next = h
            .pool
            .slots()
            .into_iter()
            .find(|s| s.phase == Phase::Idle)
            .unwrap();
        let env = String::from_utf8(files_of(&h, &next)["/Users/lume/.cua-ci/job.env"].0.clone())
            .unwrap();
        assert!(env.contains("cargo/any"));
    }

    #[tokio::test]
    async fn a_fork_pull_request_never_writes_and_the_refused_write_is_reported() {
        let h = H::new("max_runners = 2\nwarm = 1");
        h.warm_up().await;
        let mut fork = job("2/1", "pull_request", "refs/pull/7/merge");
        fork.private = Some(false);
        fork.head_repo = Some("evil/app".into());
        fork.fork = Some(true);
        h.pool.claim(fork).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let (_, bytes) = tar_with(h._dir.path(), "poison", "evil.crate", b"EVIL");
        run_to_the_end(&h, &slot, "pull_request", "refs/pull/7/merge", &bytes);
        h.pool.tick().await;
        assert!(
            h.perf
                .caches
                .current(&scope_key("acct_1", "acme/app"), "cargo", "any")
                .is_none(),
            "nothing was written"
        );
        let w = h.of(K::CacheWrite);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].extra["scope"], "fork_readonly_denied");
        assert_eq!(w[0].extra["bytes"], 0);
        // The tar was never taken out of the VM.
        assert!(
            h.spaces
                .calls()
                .iter()
                .all(|c| !matches!(c, Call::Download(_, r) if r.ends_with("/delta")))
        );
    }

    #[tokio::test]
    async fn a_guest_that_disagrees_about_its_event_writes_nothing() {
        let h = H::new("max_runners = 2\nwarm = 1");
        h.warm_up().await;
        h.pool.claim(main_job("3/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let (_, bytes) = tar_with(h._dir.path(), "x", "x.crate", b"X");
        // The claim said a push to main; the guest's hook saw another ref.
        run_to_the_end(&h, &slot, "push", "refs/heads/attacker", &bytes);
        h.pool.tick().await;
        assert!(
            h.perf
                .caches
                .current(&scope_key("acct_1", "acme/app"), "cargo", "any")
                .is_none()
        );
        assert!(
            h.of(K::CacheWrite)
                .iter()
                .all(|e| e.extra["scope"] == "fork_readonly_denied")
        );
        assert!(h.of(K::Error).iter().any(|e| {
            e.detail
                .as_deref()
                .unwrap_or("")
                .contains("differ from the claim")
        }));
    }

    #[tokio::test]
    async fn a_failed_job_writes_nothing() {
        let h = H::new("max_runners = 2\nwarm = 1");
        h.warm_up().await;
        h.pool.claim(main_job("4/1")).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        let (_, bytes) = tar_with(h._dir.path(), "y", "y.crate", b"Y");
        run_to_the_end(&h, &slot, "push", "refs/heads/main", &bytes);
        h.spaces.edit(slot.space_id.as_ref().unwrap(), |g| {
            g.console = "Running job: build\nJob build completed with result: Failed\n".into();
        });
        h.pool.tick().await;
        assert!(
            h.perf
                .caches
                .current(&scope_key("acct_1", "acme/app"), "cargo", "any")
                .is_none()
        );
        assert!(h.of(K::CacheWrite).is_empty());
        assert_eq!(h.of(K::JobFinished)[0].result.as_deref(), Some("failure"));
    }

    #[tokio::test]
    async fn a_neighbour_repo_never_sees_this_repos_cache() {
        let h = H::new("max_runners = 2\nwarm = 1");
        seed(&h);
        h.warm_up().await;
        let mut other = main_job("6/1");
        other.repo = "acme/other".into();
        h.pool.claim(other).await.unwrap();
        let slot = h.slot_with(Phase::Idle);
        assert!(
            slot.perf.job.as_ref().unwrap().stores.is_empty(),
            "no cargo cache of acme/app"
        );
        assert!(
            h.spaces
                .calls()
                .iter()
                .all(|c| !matches!(c, Call::Upload(_, _, p) if p.ends_with("in.tar")))
        );
    }
}

impl H {
    fn clock_now(&self) -> u64 {
        use crate::pool::Clock as _;
        self.clock.now_ms()
    }
}
