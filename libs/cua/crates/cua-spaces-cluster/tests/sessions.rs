// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! The re-driver of durable agent sessions against fake hosts over real
//! gRPC on loopback (see `common::hosts`): a driver is a job whose fake
//! behavior is a script per epoch, so a test can crash it at epoch 1 and
//! let it run at epoch 2, kill or partition its host, and count exactly
//! which epochs started where.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::FakeExec;
use common::hosts::*;
use cua_spaces_cluster::coordinator::durable::{
    DurableConnection, DurableSpec, SetupHook, SetupOutcome, SpaceSetup, setup,
};
use cua_spaces_cluster::coordinator::sessions::{
    ENV_CONTRACT, ENV_SPACE, SessionSpec, SessionView, sstate,
};
use cua_spaces_cluster::coordinator::{Config, Coordinator};

fn cfg(dir: &std::path::Path) -> Config {
    let mut c = config(dir);
    c.session_tick = Duration::from_millis(100);
    c
}

fn driver(script: &str) -> SessionSpec {
    SessionSpec {
        argv: vec!["driver".into(), script.into()],
        label: "test".into(),
        ..Default::default()
    }
}

struct Lab {
    _root: tempfile::TempDir,
    hosts: Vec<Host>,
    execs: Vec<FakeExec>,
    coord: Arc<Coordinator>,
}

impl Lab {
    async fn new(hosts: &[&str]) -> Self {
        Self::with(hosts, |_| {}).await
    }

    async fn with(names: &[&str], tweak: impl FnOnce(&mut Config)) -> Self {
        let root = tempfile::tempdir().unwrap();
        let mut hosts = vec![];
        let mut execs = vec![];
        for n in names {
            let e = FakeExec::new();
            hosts.push(Host::new(n, root.path(), e.clone(), 8).await);
            execs.push(e);
        }
        let mut c = cfg(&root.path().join("c"));
        tweak(&mut c);
        let coord = Coordinator::start(c, Some(links())).await.unwrap();
        for h in &hosts {
            coord.add_node(h.row()).await.unwrap();
        }
        let n = names.len();
        wait_for("hosts up", Duration::from_secs(10), async || {
            (coord.nodes().iter().filter(|x| x.health == "ready").count() == n).then_some(())
        })
        .await;
        Self {
            _root: root,
            hosts,
            execs,
            coord,
        }
    }

    /// Every driver start on every host: (host index, epoch).
    fn starts(&self) -> Vec<(usize, u64)> {
        let mut v: Vec<(usize, u64)> = self
            .execs
            .iter()
            .enumerate()
            .flat_map(|(i, e)| e.drivers().into_iter().map(move |d| (i, d.epoch())))
            .collect();
        v.sort_by_key(|(_, e)| *e);
        v
    }

    fn starts_of(&self, epoch: u64) -> usize {
        self.starts().iter().filter(|(_, e)| *e == epoch).count()
    }

    fn live(&self) -> usize {
        self.execs.iter().map(|e| e.live_drivers()).sum()
    }

    async fn until(
        &self,
        name: &str,
        what: &str,
        within: Duration,
        pred: impl Fn(&SessionView) -> bool,
    ) -> SessionView {
        wait_for(what, within, async || {
            let v = self.coord.session_get(name).await.ok()?;
            pred(&v).then_some(v)
        })
        .await
    }

    fn stop(mut self) {
        self.coord.stop();
        for h in &mut self.hosts {
            h.kill();
        }
    }
}

const T: Duration = Duration::from_secs(30);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_that_exits_non_zero_is_resumed_at_the_next_epoch_exactly_once() {
    let lab = Lab::new(&["a"]).await;
    let v = lab
        .coord
        .session_start("work", driver("1=exit:1;2=hang"), 0)
        .await
        .unwrap();
    assert_eq!((v.max_attempts, v.epoch, v.attempts), (5, 1, 1), "{v:?}");
    let v = lab
        .until("work", "epoch 2 running", T, |v| {
            v.epoch == 2 && v.state == sstate::RUNNING
        })
        .await;
    assert_eq!(v.attempts, 2);
    // It stays at 2: no second resume of the same epoch.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!((v.epoch, v.attempts), (2, 2), "{v:?}");
    assert_eq!(lab.starts_of(1), 1);
    assert_eq!(lab.starts_of(2), 1);
    assert_eq!(lab.live(), 1);
    let events: Vec<&str> = v.history.iter().map(|h| h.event.as_str()).collect();
    assert_eq!(events, ["started", "crashed", "started"], "{:?}", v.history);
    assert_eq!(v.history[1].exit_code, Some(1));
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_whose_host_is_killed_resumes_on_another_host() {
    let mut lab = Lab::new(&["a", "b"]).await;
    lab.coord
        .session_start("work", driver("1=hang;2=hang"), 0)
        .await
        .unwrap();
    wait_for("epoch 1 started", T, async || {
        (lab.starts_of(1) == 1).then_some(())
    })
    .await;
    let first = lab.starts()[0].0;
    lab.hosts[first].kill();
    let v = lab
        .until("work", "resumed elsewhere", Duration::from_secs(60), |v| {
            v.epoch == 2 && v.state == sstate::RUNNING
        })
        .await;
    let second = lab.starts().iter().find(|(_, e)| *e == 2).unwrap().0;
    assert_ne!(first, second, "{v:?}");
    assert_eq!(v.node, lab.hosts[second].name);
    assert_eq!(lab.starts_of(2), 1);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_partitioned_host_is_fenced_when_it_heals_and_only_one_driver_lives() {
    let lab = Lab::new(&["a", "b"]).await;
    lab.coord
        .session_start("work", driver("1=hang;2=hang"), 0)
        .await
        .unwrap();
    wait_for("epoch 1 started", T, async || {
        (lab.starts_of(1) == 1).then_some(())
    })
    .await;
    let first = lab.starts()[0].0;
    let other = 1 - first;
    lab.hosts[first].partition();
    lab.until("work", "resumed elsewhere", Duration::from_secs(60), |v| {
        v.epoch == 2 && v.state == sstate::RUNNING
    })
    .await;
    wait_for("epoch 2 started on the other host", T, async || {
        (lab.execs[other].drivers().iter().any(|d| d.epoch() == 2)).then_some(())
    })
    .await;
    // The old driver still runs behind the partition: two exist for now.
    assert_eq!(lab.execs[first].live_drivers(), 1);
    lab.hosts[first].heal();
    wait_for(
        "the old driver is fenced",
        Duration::from_secs(30),
        async || (lab.execs[first].live_drivers() == 0).then_some(()),
    )
    .await;
    assert_eq!(lab.execs[first].canceled().len(), 1);
    assert_eq!(lab.live(), 1, "one live driver");
    assert_eq!(lab.execs[other].live_drivers(), 1);
    // And the session did not move on again because of it.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!((v.epoch, v.state.as_str()), (2, sstate::RUNNING), "{v:?}");
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_resume_triggers_issue_exactly_one_epoch() {
    // No background loop: only the triggers below act.
    let lab = Lab::with(&["a"], |c| c.session_tick = Duration::from_secs(3600)).await;
    lab.coord
        .session_start("work", driver("1=exit:1;2=hang"), 0)
        .await
        .unwrap();
    wait_for("epoch 1 ended", T, async || {
        let j = lab
            .coord
            .store()
            .call(|c| {
                cua_spaces_cluster::coordinator::store::jobs(c, "session-work", "", 0, 10, false)
            })
            .await
            .unwrap();
        j.iter()
            .all(|j| j.state == "failed")
            .then_some(())
            .filter(|_| !j.is_empty())
    })
    .await;
    // The first look sees the crash and starts the backoff.
    let v = lab.coord.session_check("work").await.unwrap().unwrap();
    assert_eq!(v.state, sstate::BACKOFF, "{v:?}");
    assert!(v.next_attempt_in_secs.is_some());
    tokio::time::sleep(Duration::from_millis(2300)).await;
    let mut tasks = vec![];
    for _ in 0..16 {
        let c = lab.coord.clone();
        tasks.push(tokio::spawn(async move { c.session_check("work").await }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!((v.epoch, v.attempts), (2, 2), "{v:?}");
    wait_for("epoch 2 started", T, async || {
        (lab.starts_of(2) >= 1).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(lab.starts_of(2), 1);
    assert_eq!(lab.starts_of(3), 0);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn resumes_are_spaced_by_the_backoff() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=exit:1;2=exit:1;3=hang"), 0)
        .await
        .unwrap();
    lab.until("work", "epoch 3", Duration::from_secs(40), |v| v.epoch == 3)
        .await;
    wait_for("three starts", T, async || {
        (lab.execs[0].drivers().len() == 3).then_some(())
    })
    .await;
    let mut runs = lab.execs[0].drivers();
    runs.sort_by_key(|r| r.epoch());
    let gap1 = runs[1].at.duration_since(runs[0].at);
    let gap2 = runs[2].at.duration_since(runs[1].at);
    // 2 s then 4 s after the crash (plus the detection of each).
    assert!(gap1 >= Duration::from_millis(1900), "{gap1:?}");
    assert!(gap2 >= Duration::from_millis(3900), "{gap2:?}");
    assert!(
        gap2 > gap1 + Duration::from_millis(1500),
        "{gap1:?} {gap2:?}"
    );
    assert!(gap2 < Duration::from_secs(12), "{gap2:?}");
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_that_keeps_crashing_ends_failed_exhausted_with_a_clear_error() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=exit:3;2=exit:3;3=exit:3;4=exit:3"), 3)
        .await
        .unwrap();
    let v = lab
        .until("work", "failed", Duration::from_secs(40), |v| {
            v.state == sstate::FAILED
        })
        .await;
    assert_eq!(v.reason, "exhausted");
    assert_eq!((v.attempts, v.max_attempts, v.epoch), (3, 3, 3));
    assert!(
        v.error.contains("gave up after 3 of 3 drives"),
        "{}",
        v.error
    );
    assert!(v.error.contains("exit 3"), "{}", v.error);
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(lab.starts().len(), 3, "no fourth drive");
    assert_eq!(
        lab.coord.session_get("work").await.unwrap().state,
        sstate::FAILED
    );
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_78_is_a_permanent_failure_and_is_not_resumed() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=exit:78"), 0)
        .await
        .unwrap();
    let v = lab
        .until("work", "failed", T, |v| v.state == sstate::FAILED)
        .await;
    assert_eq!(v.reason, "permanent");
    assert!(v.error.contains("permanent"), "{}", v.error);
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(lab.starts().len(), 1);
    assert_eq!(lab.coord.session_get("work").await.unwrap().attempts, 1);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn exit_0_ends_the_session_done() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=sleep:200"), 0)
        .await
        .unwrap();
    let v = lab
        .until("work", "done", T, |v| v.state == sstate::DONE)
        .await;
    assert_eq!((v.attempts, v.epoch), (1, 1));
    assert_eq!(v.reason, "");
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(lab.starts().len(), 1);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fenced_driver_exit_75_is_driven_again_at_a_higher_epoch() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=exit:75;2=hang"), 0)
        .await
        .unwrap();
    let v = lab.until("work", "epoch 2", T, |v| v.epoch == 2).await;
    let events: Vec<&str> = v.history.iter().map(|h| h.event.as_str()).collect();
    assert_eq!(events, ["started", "fenced", "started"], "{:?}", v.history);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_the_driver_and_nothing_resumes() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", driver("1=hang"), 0)
        .await
        .unwrap();
    wait_for("running", T, async || (lab.live() == 1).then_some(())).await;
    let v = lab.coord.session_stop("work").await.unwrap();
    assert_eq!(v.state, sstate::STOPPED);
    wait_for("driver canceled", T, async || {
        (lab.live() == 0 && lab.execs[0].canceled().len() == 1).then_some(())
    })
    .await;
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(lab.starts().len(), 1);
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!(v.state, sstate::STOPPED);
    // Stopping again changes nothing; deleting removes it.
    assert_eq!(
        lab.coord.session_stop("work").await.unwrap().state,
        sstate::STOPPED
    );
    lab.coord.session_delete("work").await.unwrap();
    assert!(lab.coord.session_get("work").await.is_err());
    assert!(lab.coord.session_list().await.unwrap().is_empty());
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_coordinator_restart_mid_backoff_keeps_the_attempts() {
    let mut lab = Lab::new(&["a"]).await;
    // A long backoff window: the first crash waits 2 s.
    lab.coord
        .session_start("work", driver("1=exit:1;2=exit:1;3=hang"), 0)
        .await
        .unwrap();
    let v = lab
        .until("work", "in backoff", T, |v| v.state == sstate::BACKOFF)
        .await;
    assert_eq!((v.attempts, v.epoch), (1, 1));
    lab.coord.stop();
    let dir = lab._root.path().join("c");
    let coord = Coordinator::start(cfg(&dir), Some(links())).await.unwrap();
    coord.add_node(lab.hosts[0].row()).await.ok();
    let v = coord.session_get("work").await.unwrap();
    assert_eq!(v.attempts, 1, "{v:?}");
    wait_for(
        "epoch 2 after the restart",
        Duration::from_secs(30),
        async || {
            let v = coord.session_get("work").await.ok()?;
            (v.epoch >= 2).then_some(v)
        },
    )
    .await;
    let v = wait_for(
        "epoch 3 (attempts kept: 2 then 3)",
        Duration::from_secs(40),
        async || {
            let v = coord.session_get("work").await.ok()?;
            (v.epoch == 3 && v.state == sstate::RUNNING).then_some(v)
        },
    )
    .await;
    assert_eq!(v.attempts, 3);
    assert_eq!(lab.starts_of(1), 1);
    assert_eq!(lab.starts_of(2), 1);
    coord.stop();
    lab.coord = coord;
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_session_whose_job_is_unknown_is_submitted_again_at_the_same_epoch() {
    let lab = Lab::with(&["a"], |c| c.session_tick = Duration::from_secs(3600)).await;
    let v = lab
        .coord
        .session_start("work", driver("1=hang"), 0)
        .await
        .unwrap();
    let old = v.driver_job.unwrap();
    // The coordinator that took over does not know the job.
    lab.coord
        .store()
        .call(move |c| {
            c.execute("DELETE FROM jobs WHERE id=?1", [old])
                .map_err(|e| e.to_string())
        })
        .await
        .unwrap();
    let v = lab.coord.session_check("work").await.unwrap().unwrap();
    assert_eq!(
        (v.epoch, v.attempts, v.state.as_str()),
        (1, 1, sstate::RUNNING),
        "{v:?}"
    );
    assert!(v.driver_job.is_some());
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_durable_space_is_leased_per_drive_and_the_epoch_follows_the_lease() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .durable_create("shared", DurableSpec::default())
        .await
        .unwrap();
    // The Space's lease counter is already ahead of the session's.
    for _ in 0..5 {
        let c = lab
            .coord
            .durable_acquire("shared", "someone", Duration::from_secs(60))
            .await
            .unwrap();
        lab.coord
            .durable_release("shared", c.lease_token)
            .await
            .unwrap();
    }
    let mut s = driver("6=exit:1;7=hang");
    s.space = "shared".into();
    s.state_dir = "/state/work".into();
    let v = lab.coord.session_start("work", s, 0).await.unwrap();
    assert_eq!(v.epoch, 6, "max(counter + 1, lease token 6): {v:?}");
    let v = lab
        .until("work", "epoch 7", T, |v| {
            v.epoch == 7 && v.state == sstate::RUNNING
        })
        .await;
    assert_eq!(v.attempts, 2);
    // A second session cannot take the Space while this one holds it.
    let mut other = driver("1=hang");
    other.space = "shared".into();
    let o = lab.coord.session_start("other", other, 0).await.unwrap();
    assert_eq!(o.state, sstate::PENDING);
    assert!(o.error.contains("held by session:work"), "{}", o.error);
    wait_for("epoch 7 started", T, async || {
        lab.execs[0]
            .drivers()
            .iter()
            .any(|d| d.epoch() == 7)
            .then_some(())
    })
    .await;
    let runs = lab.execs[0].drivers();
    let last = runs.iter().max_by_key(|r| r.epoch()).unwrap();
    assert_eq!(last.env["CUA_SESSION_SPACE"], "shared");
    assert!(last.env["CUA_SESSION_SPACE_ADDR"].starts_with("direct:127.0.0.1:"));
    assert!(!last.env["CUA_SESSION_SPACE_TOKEN"].is_empty());
    assert_eq!(last.env["CUA_SESSION_SPACE_LEASE"], "7");
    assert_eq!(last.env["CUA_SESSION_STATE_DIR"], "/state/work");
    // Ended: the lease is given back (a waiting session would take it).
    lab.coord.session_delete("other").await.unwrap();
    lab.coord.session_stop("work").await.unwrap();
    let c = lab
        .coord
        .durable_acquire("shared", "after", Duration::from_secs(60))
        .await
        .unwrap();
    assert!(c.lease_token > 7);
    lab.stop();
}

/// The host loses the durable Space between two drives: the next drive finds it
/// gone from the host's list, places it again, and starts the driver at the new
/// address, not at the dead one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_space_its_host_lost_is_placed_again_before_the_next_drive() {
    let lab = Lab::new(&["a"]).await;
    let first = lab
        .coord
        .durable_create("shared", DurableSpec::default())
        .await
        .unwrap();
    assert_eq!(first.generation, 1);
    let mut s = driver("1=exit:1;2=hang");
    s.space = "shared".into();
    let v = lab.coord.session_start("work", s, 0).await.unwrap();
    assert_eq!(v.epoch, 1);
    // The Space is gone from the host before the backoff ends.
    lab.hosts[0].spaces.0.lock().unwrap().spaces.clear();
    lab.until("work", "epoch 2 running", T, |v| {
        v.epoch == 2 && v.state == sstate::RUNNING
    })
    .await;
    wait_for("epoch 2 started", T, async || {
        lab.execs[0]
            .drivers()
            .iter()
            .any(|d| d.epoch() == 2)
            .then_some(())
    })
    .await;
    let again = lab.coord.durable_get("shared").await.unwrap();
    assert_eq!(
        (again.generation, again.restored.as_str()),
        (2, "from-scratch"),
        "{again:?}"
    );
    let runs = lab.execs[0].drivers();
    let second = runs.iter().find(|r| r.epoch() == 2).unwrap();
    assert_eq!(
        second.env["CUA_SESSION_SPACE_ADDR"], again.space,
        "the driver got the new copy's address"
    );
    assert_ne!(second.env["CUA_SESSION_SPACE_ADDR"], first.space);
    lab.coord.session_stop("work").await.unwrap();
    lab.stop();
}

/// A `.cua/setup` stand-in: records each copy it set up and fails the
/// generations it is told to.
#[derive(Default)]
struct FakeSetup {
    /// (generation, restored, finished at (Unix ms))
    calls: std::sync::Mutex<Vec<(u32, String, i64)>>,
    fail: Vec<u32>,
    takes: Duration,
}

#[async_trait::async_trait]
impl SpaceSetup for FakeSetup {
    async fn setup(&self, space: &DurableConnection) -> SetupOutcome {
        tokio::time::sleep(self.takes).await;
        self.calls.lock().unwrap().push((
            space.generation,
            space.restored.clone(),
            cua_cluster::model::now_ms(),
        ));
        let ok = !self.fail.contains(&space.generation);
        SetupOutcome {
            ok,
            note: if ok {
                ".cua/setup finished".into()
            } else {
                ".cua/setup failed with exit code 2 (from /home/cua/repo/.cua/setup).\nnpm ERR! missing script".into()
            },
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn setup_runs_on_creation_and_on_replacement_from_scratch_before_the_next_drive() {
    let hook = Arc::new(FakeSetup {
        takes: Duration::from_millis(700),
        ..Default::default()
    });
    let h = hook.clone();
    let lab = Lab::with(&["a"], move |c| c.space_setup = Some(SetupHook(h))).await;
    let first = lab
        .coord
        .durable_create("shared", DurableSpec::default())
        .await
        .unwrap();
    assert_eq!(
        (first.generation, first.setup.as_str()),
        (1, setup::PENDING)
    );
    let mut s = driver("1=exit:1;2=hang");
    s.space = "shared".into();
    let v = lab.coord.session_start("work", s, 0).await.unwrap();
    // Nothing is driven while the first copy is set up.
    assert_eq!((v.epoch, v.state.as_str()), (0, sstate::PENDING), "{v:?}");
    assert!(v.error.contains(".cua/setup"), "{}", v.error);
    lab.until("work", "epoch 1 driven after setup", T, |v| v.epoch >= 1)
        .await;
    assert_eq!(
        lab.coord.durable_get("shared").await.unwrap().setup,
        setup::OK
    );
    // The Space is lost; the next copy is set up from scratch, then epoch 2
    // runs on it.
    lab.hosts[0].spaces.0.lock().unwrap().spaces.clear();
    lab.until("work", "epoch 2 running", T, |v| {
        v.epoch == 2 && v.state == sstate::RUNNING
    })
    .await;
    wait_for("epoch 2 started", T, async || {
        lab.execs[0]
            .drivers()
            .iter()
            .any(|d| d.epoch() == 2)
            .then_some(())
    })
    .await;
    let started2 = cua_cluster::model::now_ms();
    let calls = hook.calls.lock().unwrap().clone();
    assert_eq!(
        calls
            .iter()
            .map(|(g, r, _)| (*g, r.as_str()))
            .collect::<Vec<_>>(),
        [(1, ""), (2, "from-scratch")]
    );
    assert!(calls[1].2 <= started2, "setup finished before the drive");
    let again = lab.coord.durable_get("shared").await.unwrap();
    assert_eq!(
        (
            again.generation,
            again.restored.as_str(),
            again.setup.as_str()
        ),
        (2, "from-scratch", setup::OK)
    );
    lab.coord.session_stop("work").await.unwrap();
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_setup_fails_the_session_and_nothing_is_driven() {
    let hook = Arc::new(FakeSetup {
        fail: vec![1],
        ..Default::default()
    });
    let h = hook.clone();
    let lab = Lab::with(&["a"], move |c| c.space_setup = Some(SetupHook(h))).await;
    lab.coord
        .durable_create("shared", DurableSpec::default())
        .await
        .unwrap();
    let mut s = driver("1=hang");
    s.space = "shared".into();
    lab.coord.session_start("work", s, 0).await.unwrap();
    let v = lab
        .until("work", "failed", T, |v| v.state == sstate::FAILED)
        .await;
    assert_eq!(v.reason, "permanent");
    assert_eq!(
        v.error,
        "the .cua/setup of its durable Space shared failed: .cua/setup failed with exit code 2 \
         (from /home/cua/repo/.cua/setup).\nnpm ERR! missing script"
    );
    assert_eq!(lab.starts().len(), 0);
    let d = lab.coord.durable_get("shared").await.unwrap();
    assert_eq!(d.setup, setup::FAILED);
    assert!(d.setup_note.contains("npm ERR!"));
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_session_on_a_missing_space_fails_permanently() {
    let lab = Lab::new(&["a"]).await;
    let mut s = driver("1=hang");
    s.space = "nowhere".into();
    let v = lab.coord.session_start("work", s, 0).await.unwrap();
    assert_eq!(
        (v.state.as_str(), v.reason.as_str()),
        (sstate::FAILED, "permanent")
    );
    assert!(v.error.contains("no durable Space nowhere"), "{}", v.error);
    assert_eq!(lab.starts().len(), 0);
    lab.stop();
}

/// The field names `cua agent run --durable` sends (the CLI keeps this list
/// in a test of its own).
#[test]
fn the_session_spec_fields_are_the_ones_the_cli_sends() {
    let v = serde_json::to_value(SessionSpec::default()).unwrap();
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(|k| k.as_str()).collect();
    keys.sort();
    let mut want = vec![
        "argv",
        "env",
        "image",
        "kind",
        "runtime",
        "cpus",
        "memory_mb",
        "timeout_secs",
        "on",
        "os",
        "arch",
        "accelerator",
        "bundle",
        "space",
        "state_dir",
        "label",
        "stall_secs",
    ];
    want.sort();
    assert_eq!(keys, want);
}

/// The names the drivers (TS, ACP) read must not drift.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_driver_environment_contract_is_exactly_the_documented_names() {
    assert_eq!(
        ENV_CONTRACT,
        [
            "CUA_SESSION",
            "CUA_SESSION_EPOCH",
            "CUA_SESSION_ATTEMPT",
            "CUA_SESSION_RESUME",
            "CUA_SESSION_SPACE",
            "CUA_SESSION_STATE_DIR",
            "CUA_SESSION_HEARTBEAT_SECS",
        ]
    );
    assert_eq!(
        ENV_SPACE,
        [
            "CUA_SESSION_SPACE_ADDR",
            "CUA_SESSION_SPACE_TOKEN",
            "CUA_SESSION_SPACE_LEASE"
        ]
    );
    let lab = Lab::new(&["a"]).await;
    let mut s = driver("1=exit:1;2=hang");
    s.env.insert("MY_KEY".into(), "s3cret-value".into());
    lab.coord.session_start("work", s, 0).await.unwrap();
    lab.until("work", "epoch 2", T, |v| v.epoch == 2).await;
    wait_for("both started", T, async || {
        (lab.execs[0].drivers().len() == 2).then_some(())
    })
    .await;
    let mut runs = lab.execs[0].drivers();
    runs.sort_by_key(|r| r.epoch());
    for (i, r) in runs.iter().enumerate() {
        let mut keys: Vec<&str> = r
            .env
            .keys()
            .map(|k| k.as_str())
            .filter(|k| k.starts_with("CUA_SESSION"))
            .collect();
        keys.sort();
        let mut want: Vec<&str> = ENV_CONTRACT.to_vec();
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(r.env["CUA_SESSION"], "work");
        assert_eq!(r.env["CUA_SESSION_EPOCH"], (i + 1).to_string());
        assert_eq!(r.env["CUA_SESSION_ATTEMPT"], (i + 1).to_string());
        assert_eq!(r.env["CUA_SESSION_RESUME"], if i == 0 { "0" } else { "1" });
        assert_eq!(r.env["CUA_SESSION_SPACE"], "");
        // The default limit is 120 s, so a heartbeat every 30 s.
        assert_eq!(r.env["CUA_SESSION_HEARTBEAT_SECS"], "30");
        assert_eq!(r.env["MY_KEY"], "s3cret-value");
    }
    // The view never shows environment values.
    let v = lab.coord.session_get("work").await.unwrap();
    let json = serde_json::to_string(&v).unwrap();
    assert!(!json.contains("s3cret-value"), "{json}");
    assert_eq!(v.spec.env_keys, ["MY_KEY"]);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pool_requests_start_list_get_output_stop_delete() {
    use cua_spaces_cluster::coordinator::requests::{Caller, answer};
    use serde_json::json;
    let lab = Lab::new(&["a"]).await;
    let me = Caller::local();
    let (st, v) = answer(
        &lab.coord,
        "session_start",
        json!({"name": "Bad Name", "spec": {"argv": ["driver", ""]}}),
        &me,
    )
    .await;
    assert_eq!(st, 400, "{v}");
    let (st, v) = answer(
        &lab.coord,
        "session_start",
        json!({"name": "work", "max_attempts": 2, "spec": {"argv": ["driver", "1=hang"], "env": {"K": "v"}, "on": "auto", "label": "x"}}),
        &me,
    )
    .await;
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["state"], "running");
    assert_eq!(v["max_attempts"], 2);
    assert_eq!(v["epoch"], 1);
    assert!(v["driver_job"].as_i64().unwrap() > 0);
    for key in [
        "name",
        "state",
        "reason",
        "epoch",
        "attempts",
        "max_attempts",
        "next_attempt_in_secs",
        "driver_job",
        "node",
        "error",
        "spec",
        "history",
    ] {
        assert!(v.get(key).is_some(), "SessionView lacks {key}: {v}");
    }
    // Another spec under the same name is refused; the same one is idempotent.
    let (st, _) = answer(
        &lab.coord,
        "session_start",
        json!({"name": "work", "spec": {"argv": ["driver", "other"]}}),
        &me,
    )
    .await;
    assert_eq!(st, 409);
    let (st, again) = answer(
        &lab.coord,
        "session_start",
        json!({"name": "work", "max_attempts": 2, "spec": {"argv": ["driver", "1=hang"], "env": {"K": "v"}, "on": "auto", "label": "x"}}),
        &me,
    )
    .await;
    assert_eq!((st, again["epoch"].clone()), (200, json!(1)));
    let (_, l) = answer(&lab.coord, "session_list", json!({}), &me).await;
    assert_eq!(l["sessions"].as_array().unwrap().len(), 1);
    let (st, g) = answer(&lab.coord, "session_get", json!({"name": "work"}), &me).await;
    assert_eq!((st, g["spec"]["env_keys"].clone()), (200, json!(["K"])));
    // The host has the job a moment after the start answers; until then
    // output is a retryable 503 ("no job ... on this host").
    let o = wait_for("the session's output", T, async || {
        let (st, o) = answer(
            &lab.coord,
            "session_output",
            json!({"name": "work", "from": 0}),
            &me,
        )
        .await;
        (st == 200).then_some(o)
    })
    .await;
    for key in [
        "epoch", "job", "host", "state", "from", "next", "size", "data_b64", "text", "done",
    ] {
        assert!(o.get(key).is_some(), "output lacks {key}: {o}");
    }
    let (st, s) = answer(&lab.coord, "session_stop", json!({"name": "work"}), &me).await;
    assert_eq!((st, s["state"].clone()), (200, json!("stopped")));
    let (st, _) = answer(&lab.coord, "session_get", json!({"name": "nope"}), &me).await;
    assert_eq!(st, 404);
    let (st, d) = answer(&lab.coord, "session_delete", json!({"name": "work"}), &me).await;
    assert_eq!((st, d["deleted"].clone()), (200, json!(true)));
    // An ended session's output is what the coordinator kept of its driver.
    answer(
        &lab.coord,
        "session_start",
        json!({"name": "short", "max_attempts": 1, "spec": {"argv": ["driver", "1=exit:1"]}}),
        &me,
    )
    .await;
    lab.until("short", "failed", T, |v| v.state == sstate::FAILED)
        .await;
    let (st, o) = answer(&lab.coord, "session_output", json!({"name": "short"}), &me).await;
    assert_eq!(st, 200, "{o}");
    assert!(
        o["text"].as_str().unwrap().contains("driver epoch 1"),
        "{o}"
    );
    assert_eq!(o["done"], true);
    lab.stop();
}

/// A machine that takes the pool over from the log keeps every session
/// and goes on re-driving them.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_new_holder_rebuilds_sessions_from_the_log_and_goes_on() {
    use cua_spaces_cluster::coordinator::journal;
    use cua_spaces_cluster::pool::memlog::MemPool;
    let root = tempfile::tempdir().unwrap();
    let exec = FakeExec::new();
    let mut host = Host::new("a", root.path(), exec.clone(), 8).await;
    let pool = MemPool::new();
    let e1 = pool.acquire("first").unwrap();
    let first = Coordinator::start_with(
        cfg(&root.path().join("first")),
        Some(links()),
        Arc::new(pool.log(e1)),
    )
    .await
    .unwrap();
    first.add_node(host.row()).await.unwrap();
    wait_for("host up", T, async || {
        first
            .nodes()
            .iter()
            .any(|n| n.health == "ready")
            .then_some(())
    })
    .await;
    first
        .session_start("work", driver("1=exit:1;2=hang"), 0)
        .await
        .unwrap();
    wait_for("backoff", T, async || {
        (first.session_get("work").await.ok()?.state == sstate::BACKOFF).then_some(())
    })
    .await;
    first.stop();
    pool.expire();
    let e2 = pool.acquire("second").unwrap();
    let dir = root.path().join("second");
    let log = pool.log(e2);
    journal::rebuild(&dir.join("cluster.db"), "p", &log)
        .await
        .expect("a fresh database rebuilds a log with sessions");
    let second = Coordinator::start_with(cfg(&dir), Some(links()), Arc::new(log))
        .await
        .unwrap();
    let v = second.session_get("work").await.unwrap();
    assert_eq!(v.attempts, 1, "{v:?}");
    wait_for(
        "epoch 2 after the takeover",
        Duration::from_secs(40),
        async || {
            let v = second.session_get("work").await.ok()?;
            (v.epoch == 2 && v.state == sstate::RUNNING).then_some(())
        },
    )
    .await;
    // RUNNING is recorded when the job is placed; the host starts its driver
    // a moment later.
    wait_for("the epoch 2 driver", T, async || {
        (exec.drivers().iter().filter(|d| d.epoch() == 2).count() == 1).then_some(())
    })
    .await;
    assert_eq!(exec.drivers().iter().filter(|d| d.epoch() == 2).count(), 1);
    second.stop();
    host.kill();
}

fn stalling(script: &str, stall: u32) -> SessionSpec {
    SessionSpec {
        stall_secs: Some(stall),
        ..driver(script)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_that_goes_silent_is_cancelled_and_resumed_once_as_stalled() {
    let lab = Lab::new(&["a"]).await;
    // Epoch 1 prints a line and then nothing; epoch 2 keeps beating.
    lab.coord
        .session_start("work", stalling("1=hang;2=beat:200", 2), 0)
        .await
        .unwrap();
    let v = lab
        .until("work", "epoch 2 running", Duration::from_secs(40), |v| {
            v.epoch == 2 && v.state == sstate::RUNNING
        })
        .await;
    let events: Vec<&str> = v.history.iter().map(|h| h.event.as_str()).collect();
    assert_eq!(events, ["started", "stalled", "started"], "{:?}", v.history);
    assert!(
        v.history[1].detail.contains("wrote no output for"),
        "{:?}",
        v.history[1]
    );
    // The wedged driver was cancelled on its host; one lives.
    wait_for("the wedged driver is cancelled", T, async || {
        (lab.execs[0].canceled().len() == 1).then_some(())
    })
    .await;
    // The env asked for a heartbeat every second (limit 2 s / 4, at least 1).
    let runs = lab.execs[0].drivers();
    assert!(
        runs.iter()
            .all(|r| r.env["CUA_SESSION_HEARTBEAT_SECS"] == "1")
    );
    tokio::time::sleep(Duration::from_secs(4)).await;
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!(
        (v.epoch, v.attempts),
        (2, 2),
        "heartbeats keep epoch 2 alive: {v:?}"
    );
    assert!(v.last_output_age_secs.is_some_and(|a| a <= 2), "{v:?}");
    assert_eq!(lab.starts_of(2), 1);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_that_heartbeats_but_never_exits_is_not_killed() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", stalling("1=beat:200", 2), 0)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(8)).await;
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!((v.epoch, v.state.as_str()), (1, sstate::RUNNING), "{v:?}");
    assert!(lab.execs[0].canceled().is_empty());
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stall_detection_can_be_turned_off() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", stalling("1=hang", 0), 0)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let v = lab.coord.session_get("work").await.unwrap();
    assert_eq!((v.epoch, v.state.as_str()), (1, sstate::RUNNING), "{v:?}");
    assert_eq!(
        lab.execs[0].drivers()[0].env["CUA_SESSION_HEARTBEAT_SECS"],
        "0"
    );
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stalls_count_toward_the_cap() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("work", stalling("1=hang;2=hang", 2), 2)
        .await
        .unwrap();
    let v = lab
        .until("work", "failed", Duration::from_secs(60), |v| {
            v.state == sstate::FAILED
        })
        .await;
    assert_eq!((v.reason.as_str(), v.attempts), ("exhausted", 2), "{v:?}");
    assert!(v.error.contains("wrote no output"), "{}", v.error);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_host_is_not_a_stall() {
    let lab = Lab::new(&["a", "b"]).await;
    lab.coord
        .session_start("work", stalling("1=beat:200;2=beat:200", 2), 0)
        .await
        .unwrap();
    wait_for("epoch 1 started", T, async || {
        (lab.starts_of(1) == 1).then_some(())
    })
    .await;
    let first = lab.starts()[0].0;
    lab.hosts[first].partition();
    let v = lab
        .until("work", "resumed elsewhere", Duration::from_secs(60), |v| {
            v.epoch == 2 && v.state == sstate::RUNNING
        })
        .await;
    // Through the job layer's lost-host path, not the output watch.
    assert!(
        v.history.iter().all(|h| h.event != "stalled"),
        "{:?}",
        v.history
    );
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_that_fits_on_no_machine_says_what_it_waits_for() {
    let lab = Lab::new(&["a"]).await;
    let mut spec = driver("1=hang");
    // The test hosts have 8 CPUs: this driver fits on none of them.
    spec.cpus = 64.0;
    spec.memory_mb = 4096;
    lab.coord.session_start("big", spec, 0).await.unwrap();
    let v = lab
        .until("big", "a waiting reason", T, |v| !v.waiting.is_empty())
        .await;
    assert_eq!(v.state, sstate::RUNNING);
    assert!(
        v.waiting.contains("64 CPUs") && v.waiting.contains("4 GB memory"),
        "{}",
        v.waiting
    );
    assert_eq!(lab.starts_of(1), 0, "it must not have started");
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_driver_that_fits_has_no_waiting_reason() {
    let lab = Lab::new(&["a"]).await;
    lab.coord
        .session_start("small", driver("1=hang"), 0)
        .await
        .unwrap();
    wait_for("epoch 1 started", T, async || {
        (lab.starts_of(1) == 1).then_some(())
    })
    .await;
    let v = lab.coord.session_get("small").await.unwrap();
    assert!(v.waiting.is_empty(), "{}", v.waiting);
    lab.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_next_drive_starts_on_the_machine_that_ran_the_last_when_it_has_room() {
    // The journal is in the data folder of the machine that ran the last
    // drive. Spreading would send the next drive to the idle machine `b`;
    // the session goes back to `a`.
    let lab = Lab::new(&["a", "b"]).await;
    lab.coord
        .session_start("work", driver("1=crashafter:1500;2=hang"), 0)
        .await
        .unwrap();
    wait_for("epoch 1 started", T, async || {
        (lab.starts_of(1) == 1).then_some(())
    })
    .await;
    let first = lab.starts()[0].0;
    // Load the first machine so that, without the preference, the next
    // drive would go to the other one.
    let other = 1 - first;
    let names = ["a", "b"];
    for (i, host) in [(0, names[other]), (1, names[first]), (2, names[first])] {
        let mut s = driver("1=hang");
        s.on = host.into();
        lab.coord
            .session_start(&format!("filler{i}"), s, 0)
            .await
            .unwrap();
    }
    let v = lab
        .until("work", "epoch 2 running", Duration::from_secs(30), |v| {
            v.epoch == 2 && v.state == sstate::RUNNING
        })
        .await;
    wait_for("epoch 2 started", T, async || {
        (lab.starts_of(2) == 1).then_some(())
    })
    .await;
    let second = lab
        .starts()
        .iter()
        .find(|(_, e)| *e == 2)
        .map(|(i, _)| *i)
        .unwrap();
    assert_eq!(second, first, "{v:?}");
    lab.stop();
}
