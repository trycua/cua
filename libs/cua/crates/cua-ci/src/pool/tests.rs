//! The reconciler against the in-memory fakes: every path a slot can
//! take, orphans both ways, the macOS budget, backoff, and that no secret
//! reaches a log, a command line or the state file.

use super::*;
use crate::config::PoolConfig;
use crate::events::EventKind as K;
use crate::testing::{Call, FakeGitHub, FakeRunner, FakeSpaces, FakeTarballs, ManualClock, config};

const T0: u64 = 1_800_000_000_000;

struct H {
    _dir: tempfile::TempDir,
    paths: CiPaths,
    clock: Arc<ManualClock>,
    spaces: Arc<FakeSpaces>,
    gh: Arc<FakeGitHub>,
    tar: Arc<FakeTarballs>,
    pool: Pool,
}

impl H {
    fn new(extra: &str) -> Self {
        Self::with(config(extra), |_| {})
    }

    fn with(cfg: PoolConfig, before: impl FnOnce(&H0)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let h0 = H0 {
            paths: CiPaths::new(dir.path().join("ci")),
            clock: Arc::new(ManualClock::new(T0)),
            spaces: Arc::new(FakeSpaces::new()),
            gh: Arc::new(FakeGitHub::new()),
            tar: Arc::new(FakeTarballs::new(dir.path())),
        };
        before(&h0);
        let pool = Pool::open(cfg, h0.paths.clone(), h0.deps()).unwrap();
        H {
            _dir: dir,
            paths: h0.paths,
            clock: h0.clock,
            spaces: h0.spaces,
            gh: h0.gh,
            tar: h0.tar,
            pool,
        }
    }

    fn advance(&self, secs: u64) {
        self.clock.advance(Duration::from_secs(secs));
    }

    fn events(&self) -> Vec<Event> {
        self.pool.events().read()
    }

    fn kinds(&self) -> Vec<K> {
        self.events().iter().map(|e| e.kind).collect()
    }

    fn count(&self, k: K) -> usize {
        self.kinds().iter().filter(|x| **x == k).count()
    }

    fn deleted(&self, reason: &str) -> usize {
        self.events()
            .iter()
            .filter(|e| e.kind == K::SlotDeleted && e.reason.as_deref() == Some(reason))
            .count()
    }

    fn creates(&self) -> usize {
        self.spaces
            .calls()
            .iter()
            .filter(|c| matches!(c, Call::Create(_)))
            .count()
    }

    fn slot_space(&self, i: usize) -> (Slot, String) {
        let s = self.pool.slots()[i].clone();
        let id = s.space_id.clone().unwrap();
        (s, id)
    }
}

/// The fakes before the pool exists.
struct H0 {
    paths: CiPaths,
    clock: Arc<ManualClock>,
    spaces: Arc<FakeSpaces>,
    gh: Arc<FakeGitHub>,
    tar: Arc<FakeTarballs>,
}

impl H0 {
    fn deps(&self) -> PoolDeps {
        PoolDeps {
            spaces: self.spaces.clone(),
            github: self.gh.clone(),
            tarballs: self.tar.clone(),
            clock: self.clock.clone(),
            host: Some(HostResources {
                cpus: 20,
                memory_mb: 65536,
            }),
        }
    }
}

#[tokio::test]
async fn happy_path_one_job_per_fresh_space() {
    let h = H::new("");
    h.pool.tick_and_provision().await;
    let slots = h.pool.slots();
    assert_eq!(slots.len(), 2, "warm = max = 2");
    assert!(slots.iter().all(|s| s.phase == Phase::Idle));
    assert_eq!(h.spaces.ids().len(), 2);

    // Each Space: labelled, sized from this Mac, egress refused by Lume and
    // recorded (not faked).
    let (s0, sp0) = h.slot_space(0);
    let fs = h.spaces.get(&sp0);
    assert_eq!(fs.spec.labels["cua-ci"], "acme");
    assert_eq!(fs.spec.labels["cua-ci-slot"], s0.id);
    assert_eq!((fs.spec.cpus, fs.spec.memory_mb), (Some(10), Some(8192)));
    assert_eq!(fs.spec.egress, None);
    assert!(s0.name.starts_with("cua-acme-"));
    assert_eq!(fs.row.name, s0.name);
    assert!(s0.egress.as_deref().unwrap().contains("pf needs root"));
    assert_eq!(h.count(K::EgressUnenforced), 2);
    assert_eq!(
        h.creates(),
        4,
        "each create is retried once without the policy"
    );

    // The runner: copied from the host cache, unpacked, registered with
    // the pool's labels, the JIT config written through the file API.
    assert_eq!(*h.tar.asked.lock().unwrap(), ["osx-arm64", "osx-arm64"]);
    let reqs = h.gh.jit_requests.lock().unwrap().clone();
    assert_eq!(reqs.len(), 2);
    assert_eq!(
        reqs[0].labels,
        ["self-hosted", "macOS", "ARM64", "cua", "cua-ci"]
    );
    assert_eq!(reqs[0].runner_group_id, 1);
    assert_eq!(reqs[0].work_folder, "_work");
    let rid = s0.runner_id.unwrap();
    let jit = &fs.files["/Users/lume/actions-runner/.cua-jitconfig"];
    assert_eq!(jit, &(FakeGitHub::jit_secret(rid).into_bytes(), 0o600));
    assert_eq!(fs.files["/Users/lume/actions-runner/cua-run.sh"].1, 0o755);
    assert!(h.spaces.calls().contains(&Call::Spawn(
        sp0.clone(),
        "/bin/bash".into(),
        vec!["/Users/lume/actions-runner/cua-run.sh".into()],
        "/Users/lume/actions-runner".into(),
        format!("cua-ci-{}", s0.id),
    )));
    assert_eq!(h.count(K::RunnerRegistered), 2);

    // Warm is satisfied: nothing more.
    h.advance(3);
    assert!(h.pool.tick().await.is_empty());

    // GitHub assigns a job.
    h.gh.edit(rid, |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.advance(15);
    assert!(h.pool.tick().await.is_empty(), "busy counts toward max");
    let s = h.pool.slots().into_iter().find(|s| s.id == s0.id).unwrap();
    assert_eq!(s.phase, Phase::Busy);
    let started = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::JobAssigned)
        .unwrap();
    assert_eq!(started.ms, Some(18_000));

    // The job ends; the ephemeral runner exits and GitHub removes it.
    h.spaces.edit(&sp0, |g| {
        g.console = "Running job: build\nJob build completed with result: Succeeded\n".into();
        g.runner = FakeRunner::Exited(0);
    });
    h.gh.forget(rid);
    h.advance(60);
    let replacement = h.pool.tick().await;
    assert_eq!(replacement.len(), 1, "a fresh slot replaces it");
    assert!(
        !h.spaces.ids().contains(&sp0),
        "the Space is deleted, never reused"
    );
    let fin = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::JobFinished)
        .unwrap();
    assert_eq!(fin.job.as_deref(), Some("build"));
    assert_eq!(fin.result.as_deref(), Some("success"));
    assert_eq!(fin.extra["runner_result"], "Succeeded");
    assert_eq!(fin.exit_code, Some(0));
    assert_eq!(fin.ms, Some(60_000));
    assert_eq!(h.deleted("job_done"), 1);
    let logs = h.paths.slot_logs("acme", &s0.id);
    assert!(logs.join("cua-run.log").is_file() && logs.join("_diag/Runner_1.log").is_file());
    h.pool.provision(&replacement[0]).await;
    assert_eq!(h.pool.slots().len(), 2);
    assert_eq!(h.spaces.ids().len(), 2);

    // Status from the files.
    let st = status(h.pool.config(), &h.paths, h.clock.now_ms());
    assert_eq!(st.counts["idle"], 2);
    assert_eq!(st.last_7_days.jobs_finished, 1);
    assert_eq!(st.last_7_days.create_to_ready.count, 3);
    assert!(
        st.egress
            .starts_with("not enforced: An egress policy (macOS VMs")
    );
    assert_eq!(
        st.labels,
        ["self-hosted", "macOS", "ARM64", "cua", "cua-ci"]
    );
}

#[tokio::test]
async fn a_golden_image_with_the_runner_skips_the_copy() {
    let h = H::with(config("max_runners = 1"), |h0| {
        *h0.spaces.golden.lock().unwrap() = true
    });
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.slots()[0].phase, Phase::Idle);
    assert!(h.tar.asked.lock().unwrap().is_empty());
    assert!(
        !h.spaces
            .calls()
            .iter()
            .any(|c| matches!(c, Call::Upload(..)))
    );
    assert_eq!(h.count(K::RunnerInstalled), 0);
}

#[tokio::test]
async fn linux_spaces_get_the_egress_policy_with_their_resolver() {
    let h = H::new("image = \"ghcr.io/trycua/linux:24.04\"\nmax_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    let fs = h.spaces.get(&sp);
    let created = fs.spec.egress.clone().unwrap();
    assert_eq!(created.deny, crate::egress::DEFAULT_DENY);
    assert_eq!(s.egress.as_deref(), Some("host"));
    // QEMU's resolver is in 10/8: allowed exactly, before the runner starts.
    assert_eq!(fs.egress_set.len(), 1);
    assert!(fs.egress_set[0].allow.contains(&"10.0.2.3/32".to_string()));
    assert!(fs.egress_set[0].blocks("10.0.2.2".parse().unwrap()));
    let order: Vec<Call> = h
        .spaces
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::SetEgress(_) | Call::Spawn(..)))
        .collect();
    assert!(matches!(order[0], Call::SetEgress(_)) && matches!(order[1], Call::Spawn(..)));
    assert_eq!(h.count(K::EgressUnenforced), 0);
    assert_eq!(*h.tar.asked.lock().unwrap(), ["linux-arm64"]);
    assert_eq!(
        h.gh.jit_requests.lock().unwrap()[0].labels,
        ["self-hosted", "Linux", "ARM64", "cua", "cua-ci"]
    );
    // No policy when the pool turns it off.
    let off = H::new("image = \"ghcr.io/trycua/linux:24.04\"\nmax_runners = 1\negress = \"off\"");
    off.pool.tick_and_provision().await;
    let (s, sp) = off.slot_space(0);
    assert_eq!(off.spaces.get(&sp).spec.egress, None);
    assert_eq!(s.egress.as_deref(), Some("off"));
}

#[tokio::test]
async fn require_egress_refuses_macos_instead_of_running_unprotected() {
    let h = H::new("max_runners = 1\nrequire_egress = true");
    h.pool.tick_and_provision().await;
    assert!(h.pool.slots().is_empty());
    assert!(h.spaces.ids().is_empty());
    assert_eq!(h.creates(), 1);
    let failed = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::CreateFailed)
        .unwrap();
    assert!(failed.detail.unwrap().contains("require_egress"));
}

#[tokio::test]
async fn a_space_lost_mid_job_removes_its_runner() {
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    let rid = s.runner_id.unwrap();
    h.gh.edit(rid, |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.advance(11);
    h.pool.tick().await;
    assert_eq!(h.pool.slots()[0].phase, Phase::Busy);

    h.spaces.vanish(&sp);
    h.advance(11);
    let next = h.pool.tick().await;
    assert_eq!(h.count(K::SpaceLost), 1);
    assert_eq!(h.deleted("space_lost"), 1);
    let fin = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::JobFinished)
        .unwrap();
    assert_eq!(fin.reason.as_deref(), Some("space_lost"));
    assert!(h.gh.deletes.lock().unwrap().contains(&rid));
    assert_eq!(next.len(), 1, "a fresh slot replaces it");
    // GitHub still lists it busy until it notices; the sweep removes it then.
    assert!(h.gh.ids().contains(&rid));
    h.gh.edit(rid, |r| {
        r.busy = false;
        r.status = "offline".into();
    });
    h.advance(301);
    h.pool.tick().await;
    assert!(!h.gh.ids().contains(&rid));
    assert_eq!(h.count(K::OrphanRunnerDeleted), 1);
}

#[tokio::test]
async fn crashed_unstarted_and_unreachable_runners_are_replaced() {
    for (guest, wait, reason) in [
        (FakeRunner::Gone, 5, "runner_crashed"),
        (FakeRunner::NotStarted, 121, "runner_never_started"),
        (FakeRunner::Exited(1), 5, "runner_exited"),
    ] {
        let h = H::new("max_runners = 1");
        h.pool.tick_and_provision().await;
        let (_, sp) = h.slot_space(0);
        h.spaces.edit(&sp, |g| g.runner = guest.clone());
        h.advance(wait);
        assert_eq!(h.pool.tick().await.len(), 1, "{reason}");
        assert_eq!(h.deleted(reason), 1, "{reason}");
        assert!(h.spaces.ids().is_empty(), "{reason}");
        assert!(h.gh.ids().is_empty(), "{reason}: runner removed");
    }
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (_, sp) = h.slot_space(0);
    h.spaces.edit(&sp, |g| g.unreachable = true);
    for _ in 0..GUEST_UNREACHABLE_CHECKS {
        h.advance(3);
        h.pool.tick().await;
    }
    assert_eq!(h.deleted("guest_unreachable"), 1);
}

#[tokio::test]
async fn a_runner_that_never_comes_online_is_replaced() {
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    h.advance(9 * 60);
    h.pool.tick().await;
    assert_eq!(
        h.pool.slots()[0].phase,
        Phase::Idle,
        "within register_timeout"
    );
    h.advance(2 * 60);
    h.pool.tick().await;
    assert_eq!(h.deleted("register_timeout"), 1);
}

#[tokio::test]
async fn idle_runners_are_recycled_but_never_a_busy_one() {
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    let rid = s.runner_id.unwrap();
    h.gh.edit(rid, |r| r.status = "online".into());
    h.advance(11);
    h.pool.tick().await;
    h.advance(59 * 60);
    h.pool.tick().await;
    assert_eq!(h.deleted("idle_recycled"), 0);
    h.advance(2 * 60);
    let next = h.pool.tick().await;
    assert_eq!(h.deleted("idle_recycled"), 1);
    assert!(!h.gh.ids().contains(&rid), "runner removed first");
    assert!(!h.spaces.ids().contains(&sp));
    assert_eq!(next.len(), 1);

    // A runner GitHub lists as busy past idle_timeout is running a job.
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    h.advance(61 * 60);
    h.gh.edit(s.runner_id.unwrap(), |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.pool.tick().await;
    assert_eq!(h.deleted("idle_recycled"), 0);
    assert_eq!(h.pool.slots()[0].phase, Phase::Busy);
    assert!(h.spaces.ids().contains(&sp));
}

#[tokio::test]
async fn a_job_past_job_timeout_is_stopped() {
    let h = H::new("max_runners = 1\njob_timeout = \"2h\"");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    h.gh.edit(s.runner_id.unwrap(), |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.advance(11);
    h.pool.tick().await;
    h.advance(3600);
    h.pool.tick().await;
    assert_eq!(h.deleted("job_timeout"), 0);
    h.advance(3601);
    h.pool.tick().await;
    assert_eq!(h.deleted("job_timeout"), 1);
    assert!(!h.spaces.ids().contains(&sp));
    assert!(h.gh.deletes.lock().unwrap().contains(&s.runner_id.unwrap()));
    assert_eq!(
        status(h.pool.config(), &h.paths, h.clock.now_ms())
            .last_7_days
            .job_timeouts,
        1
    );
}

#[tokio::test]
async fn orphans_both_ways_and_a_live_runner_is_taken_back() {
    let h = H::with(config("max_runners = 1\nwarm = 0"), |h0| {
        let row = |name: &str, pool: &str, slot: &str| crate::spaces::SpaceRow {
            id: format!("local:{name}"),
            name: name.into(),
            labels: BTreeMap::from([
                ("cua-ci".to_string(), pool.to_string()),
                ("cua-ci-slot".to_string(), slot.to_string()),
            ]),
            os: "macos".into(),
            arch: "arm64".into(),
            provider: "local".into(),
            power_state: "running".into(),
        };
        // No slot, no runner: deleted.
        h0.spaces
            .insert(row("cua-acme-dead0001", "acme", "dead0001"));
        // No slot, but its runner is registered and busy: taken back.
        h0.spaces
            .insert(row("cua-acme-live0002", "acme", "live0002"));
        // Another pool's: untouched.
        h0.spaces.insert(row("cua-other-0003", "other", "0003"));
        let r = |id: u64, name: &str, busy: bool| crate::github::Runner {
            id,
            name: name.into(),
            status: "online".into(),
            busy,
            labels: vec![],
        };
        h0.gh.add(r(1, "cua-acme-live0002", true));
        // A runner of the pool with no Space: removed.
        h0.gh.add(r(2, "cua-acme-gone0004", false));
        // Someone else's runner: untouched.
        h0.gh.add(r(3, "build-mac-1", false));
    });
    h.pool.tick().await;
    let ids = h.spaces.ids();
    assert!(!ids.contains(&"local:cua-acme-dead0001".to_string()));
    assert!(ids.contains(&"local:cua-acme-live0002".to_string()));
    assert!(ids.contains(&"local:cua-other-0003".to_string()));
    assert_eq!(h.gh.ids(), [1, 3]);
    assert_eq!(h.count(K::OrphanSpaceDeleted), 1);
    assert_eq!(h.count(K::OrphanRunnerDeleted), 1);
    assert_eq!(h.count(K::Adopted), 1);
    let s = &h.pool.slots()[0];
    assert_eq!(
        (s.id.as_str(), s.phase, s.runner_id),
        ("live0002", Phase::Busy, Some(1))
    );
    // The adopted slot's job ends like any other.
    h.spaces.edit("local:cua-acme-live0002", |g| {
        g.runner = FakeRunner::Exited(0)
    });
    h.gh.forget(1);
    h.advance(11);
    h.pool.tick().await;
    assert_eq!(h.deleted("job_done"), 1);
    assert!(
        !h.spaces
            .ids()
            .contains(&"local:cua-acme-live0002".to_string())
    );
}

#[tokio::test]
async fn interrupted_slots_are_cleaned_up_after_a_restart() {
    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    // The process dies while the slot is starting.
    {
        let mut st = h.pool.state.lock().unwrap();
        st.slots[0].phase = Phase::Starting;
        st.slots[0].space_id = None;
    }
    h.pool.save().unwrap();
    let deps = PoolDeps {
        spaces: h.spaces.clone(),
        github: h.gh.clone(),
        tarballs: h.tar.clone(),
        clock: h.clock.clone(),
        host: None,
    };
    let again = Pool::open(h.pool.config().clone(), h.paths.clone(), deps).unwrap();
    assert_eq!(again.slots()[0].phase, Phase::Ending);
    let next = again.tick().await;
    assert!(!h.spaces.ids().contains(&sp), "its labelled Space is swept");
    assert!(
        !h.gh.ids().contains(&s.runner_id.unwrap()),
        "its runner is removed"
    );
    let reasons: Vec<_> = again
        .events()
        .read()
        .into_iter()
        .filter(|e| e.kind == K::SlotDeleted)
        .filter_map(|e| e.reason)
        .collect();
    assert_eq!(reasons, ["interrupted"]);
    assert_eq!(next.len(), 1);
}

#[tokio::test]
async fn the_macos_budget_counts_other_vms_and_a_full_mac_is_not_a_failure() {
    let h = H::with(config(""), |h0| {
        h0.spaces
            .foreign
            .lock()
            .unwrap()
            .push(crate::spaces::SpaceRow {
                id: "local:my-mac-vm".into(),
                name: "my-mac-vm".into(),
                os: "macos".into(),
                provider: "local".into(),
                power_state: "running".into(),
                ..Default::default()
            });
    });
    let ids = h.pool.tick().await;
    assert_eq!(ids.len(), 1, "one macOS VM is someone else's");
    let wait = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::CapacityWait)
        .unwrap();
    assert!(wait.detail.unwrap().contains("Apple allows 2"));
    for id in ids {
        h.pool.provision(&id).await;
    }
    // The other VM goes; meanwhile the computer is still full when asked.
    h.spaces.foreign.lock().unwrap().clear();
    h.spaces
        .create_errors
        .lock()
        .unwrap()
        .push_back(SpacesError::LimitExceeded(
            "this Mac already runs 2 macOS VMs".into(),
        ));
    h.advance(3);
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.slots().len(), 1);
    assert_eq!(h.pool.state().failures, 0, "capacity is not a failure");
    assert_eq!(h.count(K::CreateFailed), 0);
    let before = h.creates();
    h.advance(10);
    h.pool.tick_and_provision().await;
    assert_eq!(h.creates(), before, "waits 30 s before asking again");
    h.advance(25);
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.slots().len(), 2);
    assert!(h.pool.slots().iter().all(|s| s.phase == Phase::Idle));
}

#[tokio::test]
async fn macos_vms_that_are_not_spaces_count_too() {
    // Two VMs Lume runs for something else: no room at all.
    let h = H::with(config(""), |h0| *h0.spaces.other_vms.lock().unwrap() = 2);
    assert!(h.pool.tick().await.is_empty());
    assert_eq!(h.creates(), 0, "never asks Apple for a third");
    // One of them stops: one runner.
    *h.spaces.other_vms.lock().unwrap() = 1;
    h.advance(3);
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.slots().len(), 1);
    // Our own VM is not counted against us.
    h.advance(3);
    assert!(h.pool.tick().await.is_empty());
    assert_eq!(h.pool.slots().len(), 1);
}

#[tokio::test]
async fn never_more_spaces_than_max_runners() {
    let h = H::new("image = \"ghcr.io/trycua/linux:24.04\"\nmax_runners = 3\nwarm = 1");
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.slots().len(), 1, "one warm");
    // Every runner busy: one more warm each time, up to max.
    for _ in 0..5 {
        for s in h.pool.slots() {
            h.gh.edit(s.runner_id.unwrap(), |r| {
                r.status = "online".into();
                r.busy = true;
            });
        }
        h.advance(11);
        h.pool.tick_and_provision().await;
    }
    assert_eq!(h.pool.slots().len(), 3);
    assert_eq!(h.spaces.ids().len(), 3);
}

#[tokio::test]
async fn failures_back_off_exponentially_up_to_five_minutes() {
    let h = H::new("max_runners = 1");
    for _ in 0..10 {
        h.spaces
            .create_errors
            .lock()
            .unwrap()
            .push_back(SpacesError::Other("image pull failed".into()));
    }
    let mut delays = vec![];
    for _ in 0..10 {
        h.pool.tick_and_provision().await;
        let st = h.pool.state();
        let delay = st.next_create_ms - h.clock.now_ms();
        delays.push(delay / 1000);
        // Not before the backoff ends.
        let before = h.creates();
        h.advance(delay / 1000 - 1);
        h.pool.tick_and_provision().await;
        assert_eq!(h.creates(), before);
        h.advance(1);
    }
    assert_eq!(delays, [5, 10, 20, 40, 80, 160, 300, 300, 300, 300]);
    assert_eq!(h.count(K::CreateFailed), 10);
    // Success resets it.
    h.pool.tick_and_provision().await;
    assert_eq!(h.pool.state().failures, 0);
    assert_eq!(h.pool.slots()[0].phase, Phase::Idle);
    let st = status(h.pool.config(), &h.paths, h.clock.now_ms());
    assert_eq!(st.last_7_days.failures, 10);
    assert!(
        st.last_7_days
            .last_error
            .unwrap()
            .1
            .contains("image pull failed")
    );
}

#[tokio::test]
async fn a_failure_after_the_space_exists_tears_it_down() {
    let h = H::new("max_runners = 1");
    *h.gh.jit_failures.lock().unwrap() = 1;
    h.pool.tick_and_provision().await;
    assert!(h.pool.slots().is_empty());
    assert!(h.spaces.ids().is_empty(), "the Space is deleted");
    assert_eq!(h.deleted("start_failed"), 1);
    assert_eq!(h.pool.state().failures, 1);
    // A missing runner group is the same kind of failure, with its words.
    let h = H::new("max_runners = 1\nrunner_group = \"mac\"");
    h.pool.tick_and_provision().await;
    let failed = h
        .events()
        .into_iter()
        .find(|e| e.kind == K::CreateFailed)
        .unwrap();
    assert!(
        failed
            .detail
            .unwrap()
            .contains("no runner group named \"mac\"")
    );
    assert!(h.spaces.ids().is_empty());
}

#[tokio::test]
async fn remove_all_deletes_every_space_and_runner() {
    let h = H::new("");
    h.pool.tick_and_provision().await;
    let busy = h.pool.slots()[0].runner_id.unwrap();
    h.gh.edit(busy, |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.advance(11);
    h.pool.tick().await;
    // GitHub frees the busy runner once its Space is gone.
    h.gh.edit(busy, |r| r.busy = false);
    h.gh.add(crate::github::Runner {
        id: 9,
        name: "cua-acme-stale999".into(),
        status: "offline".into(),
        busy: false,
        labels: vec![],
    });
    let report = h.pool.remove_all().await.unwrap();
    assert_eq!(report.slots, 2);
    assert_eq!(report.busy_stopped, 1);
    assert_eq!(report.orphans, 1);
    assert!(h.spaces.ids().is_empty());
    assert!(h.gh.ids().is_empty());
    assert!(h.pool.slots().is_empty());
}

#[tokio::test(start_paused = true)]
async fn serve_provisions_concurrently_until_shutdown() {
    let h = H::new("");
    let pool = Arc::new(h.pool);
    pool.clone()
        .serve(tokio::time::sleep(Duration::from_secs(20)))
        .await
        .unwrap();
    assert_eq!(pool.slots().len(), 2);
    assert!(pool.slots().iter().all(|s| s.phase == Phase::Idle));
    let kinds: Vec<K> = pool.events().read().iter().map(|e| e.kind).collect();
    assert_eq!(kinds.first(), Some(&K::ServeStarted));
    assert_eq!(kinds.last(), Some(&K::ServeStopped));
}

/// Collects tracing output.
#[derive(Clone, Default)]
struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for Capture {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(b);
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
    type Writer = Capture;
    fn make_writer(&'a self) -> Capture {
        self.clone()
    }
}

/// Every tracing event of this test binary, from the first test that asks
/// (a process-wide subscriber: a per-thread one misses callsites other
/// test threads registered first).
fn capture() -> &'static Capture {
    static CAP: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    CAP.get_or_init(|| {
        let cap = Capture::default();
        let sub = tracing_subscriber::fmt()
            .with_writer(cap.clone())
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::set_global_default(sub).expect("one global subscriber");
        cap
    })
}

#[tokio::test]
async fn secrets_never_reach_a_log_a_command_line_or_the_state() {
    let cap = capture();

    let h = H::new("max_runners = 1");
    h.pool.tick_and_provision().await;
    let (s, sp) = h.slot_space(0);
    let rid = s.runner_id.unwrap();
    let secret = FakeGitHub::jit_secret(rid);
    h.gh.edit(rid, |r| {
        r.status = "online".into();
        r.busy = true;
    });
    h.advance(11);
    h.pool.tick().await;
    h.spaces.edit(&sp, |g| g.runner = FakeRunner::Exited(0));
    h.gh.forget(rid);
    h.advance(11);
    h.pool.tick().await;

    let traced = String::from_utf8(cap.0.lock().unwrap().clone()).unwrap();
    assert!(traced.contains("job_finished"), "the capture works");
    // Every test's JIT configs and keys stay out of every log line.
    let events = std::fs::read_to_string(h.paths.events("acme")).unwrap();
    let state = std::fs::read_to_string(h.paths.state("acme")).unwrap();
    for (what, text) in [("tracing", &traced), ("events", &events), ("state", &state)] {
        assert!(
            !text.contains("JITCONFIG-SECRET"),
            "{what} has the JIT config"
        );
        assert!(!text.contains("PRIVATE KEY"), "{what} has a key");
    }
    // Into the Space it goes only as the 0600 file; never on a command
    // line (shell scripts, spawn arguments) or in the Space's spec.
    for c in h.spaces.calls() {
        let line = format!("{c:?}");
        if !matches!(c, Call::Write(_, ref p, 0o600) if p.ends_with("/.cua-jitconfig")) {
            assert!(!line.contains(&secret), "{line}");
        }
    }
    let writes: Vec<Call> = h
        .spaces
        .calls()
        .into_iter()
        .filter(|c| matches!(c, Call::Write(_, p, _) if p.ends_with("/.cua-jitconfig")))
        .collect();
    assert_eq!(writes.len(), 1);
}

#[tokio::test]
async fn a_pool_on_a_checkpoint_image_starts_without_a_policy_and_keeps_its_labels() {
    // A Space started from a checkpoint takes no egress policy (the real
    // Spaces refuse one): the pool says so, goes on, and labels stay.
    let h = H::with(
        config("image = \"checkpoint:gold\"\nos = \"macos\"\nmax_runners = 1\nwarm = 1"),
        |h0| h0.spaces.checkpoints.lock().unwrap().push("gold".into()),
    );
    h.pool.tick_and_provision().await;
    assert_eq!(h.count(K::EgressUnenforced), 1, "{:?}", h.kinds());
    assert_eq!(h.count(K::CreateFailed), 0, "{:?}", h.kinds());
    let s = &h.pool.slots()[0];
    let sp = h.spaces.get(s.space_id.as_ref().unwrap());
    assert!(sp.spec.image.starts_with("checkpoint:"));
    assert!(sp.spec.egress.is_none());
    assert!(!sp.spec.labels.is_empty());
}

#[tokio::test]
async fn require_egress_refuses_a_checkpoint_image_before_anything_is_created() {
    let h = H::with(
        config(
            "image = \"checkpoint:gold\"\nos = \"macos\"\nmax_runners = 1\nwarm = 1\nrequire_egress = true",
        ),
        |h0| h0.spaces.checkpoints.lock().unwrap().push("gold".into()),
    );
    h.pool.tick_and_provision().await;
    assert_eq!(h.creates(), 0, "no Space is created unprotected");
    assert_eq!(h.count(K::CreateFailed), 1, "{:?}", h.kinds());
}
