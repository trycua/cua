// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Durable agent members, `on: auto` and the Space hooks in the supervisor
//! (see [`crate::durable_member`], [`crate::session_journal`] and
//! [`crate::hooks`] for the pieces).
//!
//! - A member with `policy.durable` starts its runs durable (guard mode,
//!   the journal shipped to the Volume). Each supervisor pass ships its
//!   journal and looks at the run: a crashed runner is re-driven at the next
//!   epoch (after `.cua/resume` and with the Volume fence raised); a run
//!   lost with its Space is started again from the journal on the Volume.
//! - `on: auto`: the member's Space is a durable Space. When it moves to
//!   another machine (created again from scratch), the member follows it,
//!   `.cua/setup` runs there, and the run starts again from the Volume.
//! - `.cua/setup` runs when a member is added and when its Space was
//!   created again (a durable Space that moved, a released cloud Space that
//!   resumed); `.cua/resume` when a paused member resumes and before a
//!   re-drive. Their notes go to the Space's thread as `system:hooks`
//!   entries. A failed setup holds the member's tasks until it succeeds.

use std::collections::BTreeMap;

use cua_spaces::agents::{Agents, RunStatus};
use cua_spaces::{Error, Result};

use super::{AgentRecord, Persistent, TickReport};
use crate::durable_member::{self, AgentsEngine, Step};
use crate::hooks::{self, HookKind, HookReport, Lookup};
use crate::session_journal::{self, GuestDir, Shipper};
use crate::thread::Draft;

/// [`AgentRecord::setup`] values.
pub mod setup {
    /// `.cua/setup` runs (or is about to).
    pub const PENDING: &str = "pending";
    /// It finished (or there was none).
    pub const OK: &str = "ok";
    /// It failed: no tasks are delivered.
    pub const FAILED: &str = "failed";
}

/// The journal's session of a member run that starts fresh.
fn new_session(name: &str) -> String {
    let base: String = name.chars().take(54).collect();
    format!("{base}-{:08x}", rand::random::<u32>())
}

impl Persistent {
    /// The Space folder a member's journal and hooks live under on the
    /// Volume: its durable Space's stable name, else its Space id.
    pub(crate) fn journal_space(rec: &AgentRecord) -> String {
        rec.durable_space
            .clone()
            .unwrap_or_else(|| rec.space.clone())
    }

    /// Who drives a member's runs here (the Volume fence's holder).
    fn holder(rec: &AgentRecord) -> String {
        format!("{}/{}", super::host_name(), rec.space)
    }

    /// The durable options of a durable member's next run. `restore`: a
    /// journal from the Volume, continuing `rec.redrive`'s session at the
    /// next epoch; `None`: a fresh run with a session of its own.
    pub(crate) fn durable_run(
        rec: &AgentRecord,
        restore: Option<(u64, BTreeMap<String, Vec<u8>>)>,
    ) -> (cua_agents::DurableOptions, crate::durable_member::Redrive) {
        let (session, epoch, files) = match restore {
            Some((epoch, files)) => (
                rec.redrive
                    .session
                    .clone()
                    .or_else(|| rec.run_id.clone())
                    .unwrap_or_else(|| new_session(&rec.name)),
                epoch,
                files,
            ),
            None => (new_session(&rec.name), 1, BTreeMap::new()),
        };
        let mut d = durable_member::durable_options(&session);
        d.guard = rec.policy.guard;
        d.epoch = epoch;
        d.resume = !files.is_empty();
        d.restore = files;
        let redrive = crate::durable_member::Redrive {
            epoch,
            session: Some(session),
            ..Default::default()
        };
        (d, redrive)
    }

    /// Where a member's hooks are looked up in its Space.
    fn hook_lookup(rec: &AgentRecord, agents: &Agents) -> Lookup {
        Lookup {
            workdirs: vec![format!("{}/work", agents.agent_home_dir(&rec.name))],
            volume_space: Some(Self::journal_space(rec)),
            stage_dir: Some(format!(
                "{}/.cua/hooks",
                agents.home().trim_end_matches('/')
            )),
        }
    }

    /// A hook's note in the member's Space thread, as `system:hooks`.
    pub(crate) fn post_hook_note(&self, rec: &AgentRecord, report: &HookReport) {
        let text = format!("{} ({}): {}", rec.name, report.kind.name(), report.note());
        let store = self.thread_store(&rec.space);
        if let Err(e) = store.append(Draft::note("hooks", None, text)) {
            tracing::warn!(agent = %rec.name, "hook note not posted: {e}");
        }
    }

    /// Runs `kind` for `rec` in its Space now (bounded by the hook's own
    /// timeout) and posts its note. `None`: there is no such hook.
    pub(crate) async fn run_hook(
        &self,
        rec: &AgentRecord,
        kind: HookKind,
    ) -> Result<Option<HookReport>> {
        let agents = self.agents_for(&rec.space).await?;
        let backend = self.drive.backend();
        let r = hooks::run(
            agents.guest(),
            Some(backend.as_ref()),
            kind,
            &Self::hook_lookup(rec, &agents),
            None,
        )
        .await?;
        if let Some(r) = &r {
            self.post_hook_note(rec, r);
        }
        Ok(r)
    }

    /// `.cua/setup` of members that wait for it, each in a task of its own
    /// (it may take 20 minutes); one at a time per member.
    pub(crate) async fn tick_setups(&self, report: &mut TickReport) {
        let records = match self.list() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("setup: {e}"));
                return;
            }
        };
        for rec in records
            .into_iter()
            .filter(|r| r.setup == setup::PENDING && !r.paused)
        {
            if self.live.lock().await.setups.contains(&rec.name) {
                continue;
            }
            // Most Spaces have no setup hook: settled here, before the
            // dispatcher looks, with no task of its own.
            if !rec.relaunch {
                let found = async {
                    let agents = self.cached_agents(&rec.space).await?;
                    let backend = self.drive.backend();
                    hooks::find(
                        agents.guest(),
                        Some(backend.as_ref()),
                        HookKind::Setup,
                        &Self::hook_lookup(&rec, &agents),
                    )
                    .await
                }
                .await;
                match found {
                    Ok(None) => {
                        if let Err(e) = self.update(&rec.name, |r| {
                            r.setup = setup::OK.into();
                            r.setup_note.clear();
                        }) {
                            report.errors.push(format!("{} setup: {e}", rec.name));
                        }
                        continue;
                    }
                    Ok(Some(_)) => {}
                    Err(e) => {
                        report.errors.push(format!("{} setup: {e}", rec.name));
                        continue;
                    }
                }
            }
            if !self.live.lock().await.setups.insert(rec.name.clone()) {
                continue;
            }
            let me = self.clone();
            tokio::spawn(async move {
                let name = rec.name.clone();
                if let Err(e) = me.run_setup(&rec).await.map(|_| ()) {
                    tracing::warn!(agent = %name, "setup: {e}");
                    let msg = e.to_string();
                    let _ = me.update(&name, |r| r.last_error = Some(msg));
                }
                me.live.lock().await.setups.remove(&name);
            });
        }
    }

    /// Runs `rec`'s `.cua/setup` and records it; then starts a lost durable
    /// run again from the Volume.
    pub(crate) async fn run_setup(&self, rec: &AgentRecord) -> Result<Option<HookReport>> {
        let r = self.run_hook(rec, HookKind::Setup).await?;
        let (state, note) = match &r {
            Some(r) if !r.ok => (setup::FAILED, r.note()),
            Some(r) => (setup::OK, r.note()),
            None => (setup::OK, String::new()),
        };
        let rec = self.update(&rec.name, |x| {
            x.setup = state.into();
            x.setup_note = note;
            x.clone()
        })?;
        if state == setup::OK && rec.relaunch {
            self.relaunch_from_volume(&rec.name).await?;
        }
        Ok(r)
    }

    /// Starts a durable member's lost run again in its (new) Space from the
    /// journal on the Volume: the fence above every shipped epoch first, so
    /// a driver left on a machine that comes back is cut off.
    pub(crate) async fn relaunch_from_volume(&self, name: &str) -> Result<()> {
        let rec = self.get(name)?;
        let space = Self::journal_space(&rec);
        let session = rec
            .redrive
            .session
            .clone()
            .or_else(|| rec.run_id.clone())
            .ok_or_else(|| Error::invalid(format!("{name} has no durable session")))?;
        let backend = self.drive.backend();
        let prefix =
            cua_volume::journal::session_prefix(&space, &session).map_err(crate::drive_err)?;
        let floor = cua_volume::journal::read_fence(backend.as_ref(), &prefix)
            .await
            .map_err(crate::drive_err)?
            .map(|(f, _)| f.epoch)
            .unwrap_or(0);
        let restored = session_journal::restore(backend.as_ref(), &space, &session).await?;
        let epoch = floor.max(restored.epoch).max(rec.redrive.epoch) + 1;
        session_journal::raise_fence(
            backend.as_ref(),
            &space,
            &session,
            epoch,
            &Self::holder(&rec),
        )
        .await?;
        self.update(name, |r| {
            r.redrive.session = Some(session.clone());
            r.run_id = None;
        })?;
        let files = restored.files;
        self.start_with(name, "", "durable", Some((epoch, files)))
            .await?;
        self.update(name, |r| r.relaunch = false)?;
        let rec = self.get(name)?;
        let _ = self.thread_store(&rec.space).append(Draft::note(
            "durable",
            None,
            format!(
                "{name} was started again from its journal on the Cua Volume (epoch {epoch}, {} chunks)",
                restored.chunks
            ),
        ));
        Ok(())
    }

    /// One supervisor look at a durable member's run: ship its journal,
    /// then re-drive it if it crashed. `Ok(false)`: the run is gone and will
    /// be started again from the Volume (nothing more to do this pass).
    pub(crate) async fn tick_durable(
        &self,
        rec: &AgentRecord,
        agents: &Agents,
        report: &mut TickReport,
    ) -> Result<bool> {
        let run = rec.run_id.clone().expect("a run");
        let session = rec.redrive.session.clone().unwrap_or_else(|| run.clone());
        let epoch = rec.redrive.epoch.max(1);
        let space = Self::journal_space(rec);
        // Ship (never fatal for the pass: the Volume may be down).
        {
            let key = format!("{run}#{epoch}");
            let mut shipper = self.live.lock().await.shippers.remove(&key);
            if shipper.is_none() {
                shipper = Shipper::new(self.drive.backend(), &space, &session, epoch).ok();
            }
            if let Some(mut s) = shipper {
                let dir = GuestDir {
                    guest: agents.guest().clone(),
                    dir: agents.run_dir(&run),
                };
                match s.tick(&dir).await {
                    Ok(r) if r.fenced.is_some() => {}
                    Ok(_) => {
                        self.live.lock().await.shippers.insert(key, s);
                    }
                    Err(e) => {
                        report.errors.push(format!("{} journal: {e}", rec.name));
                        self.live.lock().await.shippers.insert(key, s);
                    }
                }
            }
        }
        let engine = AgentsEngine {
            agents: agents.clone(),
            backend: Some(self.drive.backend()),
            volume_space: space,
            session,
            holder: Self::holder(rec),
            hooks: Self::hook_lookup(rec, agents),
        };
        let mut st = rec.redrive.clone();
        let step = durable_member::step(&engine, &run, &mut st, cua_volume::now_ms()).await?;
        if st != rec.redrive {
            let st2 = st.clone();
            self.update(&rec.name, |r| r.redrive = st2)?;
        }
        match step {
            Step::Redriven { epoch, hook } => {
                if let Some(h) = &hook {
                    self.post_hook_note(rec, h);
                }
                report
                    .fired
                    .push(format!("{}: re-driven at epoch {epoch}", rec.name));
                let _ = self.thread_store(&rec.space).append(Draft::note(
                    "durable",
                    rec.task.as_ref().map(|t| t.seq),
                    format!("{} crashed and was resumed (epoch {epoch})", rec.name),
                ));
                Ok(true)
            }
            Step::GaveUp { why } if rec.redrive.gave_up.is_none() => {
                let _ = self.thread_store(&rec.space).append(Draft::note(
                    "durable",
                    rec.task.as_ref().map(|t| t.seq),
                    format!("{} is no longer resumed: {why}", rec.name),
                ));
                Ok(true)
            }
            Step::Lost { why } => {
                tracing::warn!(agent = %rec.name, "durable run lost: {why}");
                self.update(&rec.name, |r| r.relaunch = true)?;
                Ok(false)
            }
            _ => Ok(true),
        }
    }

    /// A durable member whose run is lost (or whose Space cannot be
    /// reached): follow its durable Space to where it is now (set it up
    /// there first), or start the run again in the same Space.
    pub(crate) async fn tick_relaunch(&self, rec: &AgentRecord) -> Result<()> {
        if let Some(name) = &rec.durable_space {
            let info = Box::pin(durable_space(&self.spaces, name)).await?;
            if info.id != rec.space {
                let old = rec.space.clone();
                self.forget_space(&old).await;
                self.update(&rec.name, |r| {
                    r.space = info.id.clone();
                    r.relaunch = true;
                    r.setup = setup::PENDING.into();
                    r.setup_note.clear();
                })?;
                let _ = self.thread_store(&info.id).append(Draft::note(
                    "durable",
                    None,
                    format!(
                        "{}'s Space {name} moved (now {}); setting it up and resuming from the journal",
                        rec.name, info.id
                    ),
                ));
                return Ok(());
            }
        }
        if rec.relaunch
            && rec.setup != setup::PENDING
            && rec.setup != setup::FAILED
            && rec.redrive.gave_up.is_none()
            && cua_volume::now_ms() >= rec.redrive.next_at_ms
        {
            if let Err(e) = self.relaunch_from_volume(&rec.name).await {
                // Backed off like a re-drive, and given up the same way.
                let why = e.to_string();
                let st = self.update(&rec.name, |r| {
                    r.redrive.attempts += 1;
                    r.redrive.next_at_ms = cua_volume::now_ms()
                        + durable_member::backoff(r.redrive.attempts).as_millis() as u64;
                    if r.redrive.attempts >= durable_member::MAX_DRIVES {
                        r.redrive.gave_up = Some(format!(
                            "it could not be started again from its journal: {why}"
                        ));
                        r.relaunch = false;
                    }
                    r.clone()
                })?;
                if let Some(g) = st.redrive.gave_up {
                    let _ = self.thread_store(&st.space).append(Draft::note(
                        "durable",
                        None,
                        format!("{} is no longer resumed: {g}", st.name),
                    ));
                }
                return Err(e);
            }
            self.update(&rec.name, |r| {
                r.redrive.attempts = 0;
                r.redrive.next_at_ms = 0;
            })?;
        }
        Ok(())
    }

    /// Whether a durable member's run should be left for its re-drive
    /// (crashed or failed, not given up): its task stays in flight.
    pub(crate) fn redriving(rec: &AgentRecord, status: RunStatus) -> bool {
        rec.policy.durable
            && rec.redrive.gave_up.is_none()
            && matches!(status, RunStatus::Crashed | RunStatus::Failed)
    }
}

/// The durable Space `name` as it is now (created, or found where it moved).
pub(crate) async fn durable_space(
    spaces: &cua_spaces::Spaces,
    name: &str,
) -> Result<cua_spaces::SpaceInfo> {
    let created = Box::pin(spaces.create(cua_spaces::SpaceCreate {
        name: Some(name.to_string()),
        on: Some(cua_sandbox_core::placement::On::Auto),
        durable: true,
        wait: Some(true),
        ..Default::default()
    }))
    .await?;
    match created {
        cua_spaces::SpaceCreated::Ready { info, .. } => Ok(info),
        cua_spaces::SpaceCreated::Starting(p) => {
            Err(Error::Timeout(format!("{} is still starting", p.id)))
        }
    }
}
