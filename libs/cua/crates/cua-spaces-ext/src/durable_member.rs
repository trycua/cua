// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable agent members: the pieces the supervisor uses for an agent with
//! `policy.durable`.
//!
//! - [`run_options`]: its runs are durable (`Agents::start{durable}`, guard
//!   mode) and ship their journal to the Volume.
//! - [`step`]: one supervisor look at a durable run. A run whose runner
//!   crashed is re-driven at the next epoch (`Agents::redrive(run, epoch+1)`)
//!   after a backoff of 2 s doubling to 60 s, at most [`MAX_DRIVES`] drives
//!   in a row (the session coordinator's numbers); a permanent failure (exit
//!   78) or a fence above it stops it. Before a re-drive the Volume fence is
//!   raised and `.cua/resume` runs in the surviving Space.
//! - [`start_from_volume`]: the Space is gone (or was re-placed from
//!   scratch): raise the fence, rebuild the journal from the Volume, run
//!   `.cua/setup`, and start the run again with the restored journal.
//! - [`once_done`] / [`finish_once`]: `--once` (add, run one task, remove;
//!   the home is kept unless asked).

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use cua_spaces::Result;
use cua_spaces::agents::{self as ag, Agents, RunOptions, RunStatus};
use cua_volume::Backend;

use crate::DriveResult as _;
use crate::hooks::{self, HookKind, HookReport};
use crate::session_journal;

/// The first re-drive waits this long; each next one twice as long ...
pub const BACKOFF_FIRST: Duration = Duration::from_secs(2);
/// ... up to this.
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);
/// Drives in a row (the first one included) before giving up, like the
/// session coordinator's default `max_attempts`.
pub const MAX_DRIVES: u32 = 5;

/// The wait before re-drive number `n` (1-based).
pub fn backoff(n: u32) -> Duration {
    let secs = BACKOFF_FIRST
        .as_secs()
        .saturating_mul(1u64 << n.saturating_sub(1).min(16));
    Duration::from_secs(secs).min(BACKOFF_MAX)
}

/// The durable options of a member's run: guard mode on, the journal
/// shipped to the Volume under `session`.
pub fn durable_options(session: &str) -> cua_agents::DurableOptions {
    cua_agents::DurableOptions {
        session: Some(session.to_string()),
        guard: true,
        ship: Some(cua_agents::durable::Ship::default()),
        ..Default::default()
    }
}

/// `opts` made durable for `session` (the run id of the first drive, or
/// the session a restored run continues).
pub fn run_options(mut opts: RunOptions, session: &str) -> RunOptions {
    opts.durable = Some(durable_options(session));
    opts
}

/// The re-drive bookkeeping of one durable run (kept with the member).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redrive {
    /// The run's current epoch (1: the first drive).
    #[serde(default)]
    pub epoch: u64,
    /// Re-drives since the run was last seen healthy.
    #[serde(default)]
    pub attempts: u32,
    /// When the next re-drive is due (Unix ms; 0: none is waiting).
    #[serde(default)]
    pub next_at_ms: u64,
    /// Why it stopped being re-driven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gave_up: Option<String>,
    /// The journal's session on the Volume (`None`: the run id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Redrive {
    /// Nothing recorded (a run that is not durable).
    pub fn is_empty(&self) -> bool {
        *self == Redrive::default()
    }
}

/// What the engine sees of a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    /// Working or waiting for a message.
    Healthy,
    /// The runner finished (exit 0).
    Finished,
    /// The runner said it cannot succeed (exit 78).
    Permanent(String),
    /// The runner is gone without finishing.
    Crashed(String),
    /// A newer epoch drives the session (the local floor is above ours).
    Fenced(u64),
    /// The run (its directory, or its Space) is gone.
    Lost(String),
    /// Could not tell.
    Unknown,
}

/// The run's side of the supervisor.
#[async_trait::async_trait]
pub trait DurableEngine: Send + Sync {
    /// The run's state now.
    async fn observe(&self, run_id: &str, epoch: u64) -> Result<Seen>;
    /// Before re-driving at `epoch` on the same Space: raise the Volume
    /// fence and run `.cua/resume`.
    async fn before_redrive(&self, run_id: &str, epoch: u64) -> Result<Option<HookReport>>;
    /// Starts the runner again at `epoch`.
    async fn redrive(&self, run_id: &str, epoch: u64) -> Result<()>;
}

/// What [`step`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Healthy,
    /// The run ended; nothing to do.
    Finished,
    /// A re-drive is due at this time (Unix ms).
    Waiting {
        until_ms: u64,
    },
    /// Re-driven at this epoch (`hook`: what `.cua/resume` did).
    Redriven {
        epoch: u64,
        hook: Option<HookReport>,
    },
    /// No more re-drives (the reason is also in [`Redrive::gave_up`]).
    GaveUp {
        why: String,
    },
    /// The run is gone: restart it from the Volume ([`start_from_volume`]).
    Lost {
        why: String,
    },
    Unknown,
}

/// One look at a durable run.
pub async fn step(
    engine: &dyn DurableEngine,
    run_id: &str,
    st: &mut Redrive,
    now_ms: u64,
) -> Result<Step> {
    if let Some(why) = &st.gave_up {
        return Ok(Step::GaveUp { why: why.clone() });
    }
    st.epoch = st.epoch.max(1);
    let why = match engine.observe(run_id, st.epoch).await? {
        Seen::Healthy => {
            st.attempts = 0;
            st.next_at_ms = 0;
            return Ok(Step::Healthy);
        }
        Seen::Finished => {
            st.next_at_ms = 0;
            return Ok(Step::Finished);
        }
        Seen::Unknown => return Ok(Step::Unknown),
        Seen::Lost(why) => return Ok(Step::Lost { why }),
        Seen::Permanent(why) => {
            let why = format!("the run cannot succeed: {why}");
            st.gave_up = Some(why.clone());
            return Ok(Step::GaveUp { why });
        }
        Seen::Fenced(floor) => {
            let why = format!(
                "epoch {floor} drives this session elsewhere (this host had epoch {})",
                st.epoch
            );
            st.gave_up = Some(why.clone());
            return Ok(Step::GaveUp { why });
        }
        Seen::Crashed(why) => why,
    };
    // Drives so far: the first one plus the re-drives.
    if st.attempts + 1 >= MAX_DRIVES {
        let why = format!("it crashed {MAX_DRIVES} times in a row; the last time: {why}");
        st.gave_up = Some(why.clone());
        return Ok(Step::GaveUp { why });
    }
    if st.next_at_ms == 0 {
        st.next_at_ms = now_ms + backoff(st.attempts + 1).as_millis() as u64;
    }
    if now_ms < st.next_at_ms {
        return Ok(Step::Waiting {
            until_ms: st.next_at_ms,
        });
    }
    let epoch = st.epoch + 1;
    let hook = engine.before_redrive(run_id, epoch).await?;
    engine.redrive(run_id, epoch).await?;
    st.epoch = epoch;
    st.attempts += 1;
    st.next_at_ms = 0;
    Ok(Step::Redriven { epoch, hook })
}

/// [`DurableEngine`] over a Space's agent runs.
pub struct AgentsEngine {
    pub agents: Agents,
    /// The Volume's store (the journal and the fence); `None`: no Volume.
    pub backend: Option<Arc<dyn Backend>>,
    /// The Space folder the session lives in (a Space id, or a durable
    /// Space's stable name).
    pub volume_space: String,
    /// The session (the first drive's run id, or the member's session).
    pub session: String,
    /// Who drives it (for the fence), e.g. `<host>/<space>`.
    pub holder: String,
    /// Where `.cua/resume` is looked up.
    pub hooks: hooks::Lookup,
}

#[async_trait::async_trait]
impl DurableEngine for AgentsEngine {
    async fn observe(&self, run_id: &str, epoch: u64) -> Result<Seen> {
        let info = match self.agents.status(run_id).await {
            Ok(i) => i,
            Err(cua_agents::Error::NotFound(e)) => return Ok(Seen::Lost(e)),
            Err(e) => return Err(e.into()),
        };
        let rdir = self.agents.run_dir(run_id);
        let floor = self
            .agents
            .guest()
            .download_vec(&format!("{rdir}/{}", cua_volume::journal::EPOCH_FILE))
            .await
            .ok()
            .and_then(|b| String::from_utf8_lossy(&b).trim().parse::<u64>().ok())
            .unwrap_or(0);
        if floor > epoch {
            return Ok(Seen::Fenced(floor));
        }
        Ok(match info.status {
            RunStatus::Running => Seen::Healthy,
            RunStatus::Idle if info.phase == "exited" => Seen::Finished,
            RunStatus::Idle => Seen::Healthy,
            RunStatus::Unknown => Seen::Unknown,
            RunStatus::Crashed => Seen::Crashed(info.reason),
            RunStatus::Failed => {
                let code = self
                    .agents
                    .guest()
                    .download_vec(&format!("{rdir}/state.json"))
                    .await
                    .ok()
                    .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
                    .and_then(|v| v.get("exitCode").and_then(|c| c.as_i64()));
                let why = info.error.unwrap_or(info.reason);
                if code == Some(i64::from(cua_agents::durable::exit::PERMANENT)) {
                    Seen::Permanent(why)
                } else {
                    Seen::Crashed(why)
                }
            }
        })
    }

    async fn before_redrive(&self, _run_id: &str, epoch: u64) -> Result<Option<HookReport>> {
        if let Some(b) = &self.backend {
            session_journal::raise_fence(
                b.as_ref(),
                &self.volume_space,
                &self.session,
                epoch,
                &self.holder,
            )
            .await?;
        }
        hooks::run(
            self.agents.guest(),
            self.backend.as_deref(),
            HookKind::Resume,
            &self.hooks,
            None,
        )
        .await
    }

    async fn redrive(&self, run_id: &str, epoch: u64) -> Result<()> {
        self.agents.redrive(run_id, epoch).await?;
        Ok(())
    }
}

/// What [`start_from_volume`] did.
#[derive(Debug)]
pub struct Restart {
    /// The new run; `None` when `.cua/setup` failed (the member is marked
    /// failed and gets no tasks).
    pub started: Option<ag::Started>,
    /// The epoch it runs at.
    pub epoch: u64,
    /// What was restored.
    pub restored: cua_volume::journal::Restored,
    /// What `.cua/setup` did.
    pub setup: Option<HookReport>,
}

/// Starts a durable run again in `agents`' Space from the journal on the
/// Volume: raises the fence above every epoch shipped (so a stale driver on
/// a host that comes back is cut off), runs `.cua/setup` when the Space is
/// new (`setup`: where to look), and starts `harness` with the restored
/// journal at the next epoch. The runner resumes the agent's session (or
/// starts degraded) and sends its recovery note; it never replays a
/// finished turn.
#[allow(clippy::too_many_arguments)]
pub async fn start_from_volume(
    agents: &Agents,
    backend: &Arc<dyn Backend>,
    volume_space: &str,
    session: &str,
    holder: &str,
    harness: &str,
    opts: RunOptions,
    setup: Option<&hooks::Lookup>,
) -> Result<Restart> {
    let prefix = cua_volume::journal::session_prefix(volume_space, session).drive()?;
    let floor = cua_volume::journal::read_fence(backend.as_ref(), &prefix)
        .await
        .drive()?
        .map(|(f, _)| f.epoch)
        .unwrap_or(0);
    let restored = session_journal::restore(backend.as_ref(), volume_space, session).await?;
    let epoch = floor.max(restored.epoch) + 1;
    session_journal::raise_fence(backend.as_ref(), volume_space, session, epoch, holder).await?;
    let setup = match setup {
        Some(lk) => {
            hooks::run(
                agents.guest(),
                Some(backend.as_ref()),
                HookKind::Setup,
                lk,
                None,
            )
            .await?
        }
        None => None,
    };
    if setup.as_ref().is_some_and(|r| !r.ok) {
        return Ok(Restart {
            started: None,
            epoch,
            restored,
            setup,
        });
    }
    let mut opts = run_options(opts, session);
    if let Some(d) = opts.durable.as_mut() {
        d.epoch = epoch;
        d.resume = restored.epoch > 0;
        d.restore = restored.files.clone();
    }
    // The prompt is only used when the journal never saw a turn start.
    let started = agents.start(harness, "", opts).await?;
    Ok(Restart {
        started: Some(started),
        epoch,
        restored,
        setup,
    })
}

/// `--once`: whether the member's one task is done (a turn ended or the
/// run failed), from the run's events.
pub fn once_done(events: &[ag::AgentEvent]) -> bool {
    events
        .iter()
        .any(|e| (e.kind == "turn_ended" && e.turn >= 1) || e.kind == "exited")
}

/// `--once`: removes the member after its task (routines, run and lease
/// go). The home stays in the Volume unless `keep_home` is false.
pub async fn finish_once(
    p: &crate::persistent::Persistent,
    name: &str,
    keep_home: bool,
) -> Result<crate::persistent::AgentRecord> {
    let rec = p.remove(name).await?;
    if !keep_home {
        let prefix = cua_volume::path::agent_home(name).drive()?;
        let b = p.drive.backend();
        for m in b.list(&prefix).await.drive()? {
            match b.delete(&m.key, cua_volume::Condition::None).await {
                Ok(()) | Err(cua_volume::Error::NotFound(_)) => {}
                Err(e) => return Err(crate::drive_err(e)),
            }
        }
    }
    Ok(rec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A run whose observations are scripted; records what the supervisor
    /// did.
    #[derive(Default)]
    struct Fake {
        seen: Mutex<Vec<Seen>>,
        calls: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl DurableEngine for Fake {
        async fn observe(&self, _: &str, _: u64) -> Result<Seen> {
            let mut s = self.seen.lock().unwrap();
            Ok(if s.len() > 1 {
                s.remove(0)
            } else {
                s[0].clone()
            })
        }
        async fn before_redrive(&self, _: &str, epoch: u64) -> Result<Option<HookReport>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("fence+resume {epoch}"));
            Ok(None)
        }
        async fn redrive(&self, run: &str, epoch: u64) -> Result<()> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("redrive {run} {epoch}"));
            Ok(())
        }
    }

    fn fake(seen: Vec<Seen>) -> Fake {
        Fake {
            seen: Mutex::new(seen),
            ..Default::default()
        }
    }

    #[test]
    fn backoff_is_2s_doubling_to_60s() {
        let v: Vec<u64> = (1..=7).map(|n| backoff(n).as_secs()).collect();
        assert_eq!(v, [2, 4, 8, 16, 32, 60, 60]);
        assert_eq!(backoff(100), BACKOFF_MAX);
    }

    #[tokio::test]
    async fn a_crash_is_redriven_at_the_next_epoch_after_the_backoff() {
        let e = fake(vec![
            Seen::Healthy,
            Seen::Crashed("gone".into()),
            Seen::Crashed("gone".into()),
            Seen::Crashed("gone".into()),
            Seen::Healthy,
        ]);
        let mut st = Redrive::default();
        let t = 1_000_000;
        assert_eq!(step(&e, "run-1", &mut st, t).await.unwrap(), Step::Healthy);
        assert_eq!(st.epoch, 1);
        // First sight of the crash: wait 2 s.
        assert_eq!(
            step(&e, "run-1", &mut st, t).await.unwrap(),
            Step::Waiting { until_ms: t + 2000 }
        );
        assert_eq!(
            step(&e, "run-1", &mut st, t + 1999).await.unwrap(),
            Step::Waiting { until_ms: t + 2000 }
        );
        assert_eq!(
            step(&e, "run-1", &mut st, t + 2000).await.unwrap(),
            Step::Redriven {
                epoch: 2,
                hook: None
            }
        );
        assert_eq!(
            *e.calls.lock().unwrap(),
            ["fence+resume 2", "redrive run-1 2"],
            "the fence and the resume hook come first"
        );
        assert_eq!((st.epoch, st.attempts), (2, 1));
        // Healthy again: the count starts over.
        assert_eq!(
            step(&e, "run-1", &mut st, t + 3000).await.unwrap(),
            Step::Healthy
        );
        assert_eq!(st.attempts, 0);
    }

    #[tokio::test]
    async fn it_gives_up_after_max_drives_and_never_on_a_permanent_failure_retries() {
        let e = fake(vec![Seen::Crashed("boom".into())]);
        let mut st = Redrive::default();
        let mut now = 0u64;
        let mut epochs = vec![];
        // Bounded: MAX_DRIVES re-drives at most, each after its backoff.
        for _ in 0..100 {
            match step(&e, "r", &mut st, now).await.unwrap() {
                Step::Waiting { until_ms } => now = until_ms,
                Step::Redriven { epoch, .. } => epochs.push(epoch),
                Step::GaveUp { why } => {
                    assert!(why.starts_with("it crashed 5 times in a row"), "{why}");
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(epochs, [2, 3, 4, 5]);
        // The waits were 2, 4, 8 and 16 s.
        assert_eq!(now, 2000 + 4000 + 8000 + 16000);
        assert!(matches!(
            step(&e, "r", &mut st, now).await.unwrap(),
            Step::GaveUp { .. }
        ));

        let p = fake(vec![Seen::Permanent("bad key".into())]);
        let mut st = Redrive::default();
        assert_eq!(
            step(&p, "r", &mut st, 0).await.unwrap(),
            Step::GaveUp {
                why: "the run cannot succeed: bad key".into()
            }
        );
        assert!(p.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn fenced_finished_and_lost_runs_are_not_redriven() {
        for (seen, want) in [
            (Seen::Fenced(7), "GaveUp"),
            (Seen::Finished, "Finished"),
            (Seen::Lost("no run".into()), "Lost"),
            (Seen::Unknown, "Unknown"),
        ] {
            let e = fake(vec![seen]);
            let mut st = Redrive::default();
            let got = step(&e, "r", &mut st, 0).await.unwrap();
            assert!(format!("{got:?}").starts_with(want), "{got:?}");
            assert!(e.calls.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn once_is_done_when_the_first_turn_ends() {
        let evs = |lines: &str| ag::events::page(lines.as_bytes(), 0, 100).events;
        let started =
            "{\"seq\":1,\"ts\":0,\"turn\":1,\"type\":\"turn_started\",\"prompt\":\"go\"}\n";
        assert!(!once_done(&evs(started)));
        let ended = format!(
            "{started}{{\"seq\":2,\"ts\":0,\"turn\":1,\"type\":\"turn_ended\",\"stopReason\":\"end_turn\"}}\n"
        );
        assert!(once_done(&evs(&ended)));
        let exited = "{\"seq\":1,\"ts\":0,\"turn\":0,\"type\":\"run_exited\",\"code\":78,\"reason\":\"x\"}\n";
        assert!(once_done(&evs(exited)));
    }

    #[test]
    fn member_runs_are_guarded_and_shipped() {
        let o = run_options(RunOptions::default(), "run-1");
        let d = o.durable.unwrap();
        assert!(d.guard);
        assert_eq!(d.session.as_deref(), Some("run-1"));
        assert_eq!(d.ship, Some(cua_agents::durable::Ship::default()));
    }
}
