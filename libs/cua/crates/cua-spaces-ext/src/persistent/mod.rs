// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Persistent agents: a named agent whose memory outlives its runs and its
//! Space.
//!
//! A persistent agent is a record (`<cua home>/persistent/agents.json`): a
//! name, a harness, the Space it works in, and how to reach its model. Its
//! home lives in the Cua Volume at `agents/<name>/`. Starting it restores the
//! home into the Space (`~/cua-volume/agents/<name>`), takes the home's
//! single-writer lease, and starts a run whose harness keeps its memory
//! there ([`cua_spaces::agents::harness::memory_dir`]). The supervisor
//! ([`Persistent::tick`], run by `cua daemon`) then, for every live run:
//!
//! - saves the home after each turn, and posts a notification with the
//!   agent's answer ("Your research is ready");
//! - answers the run's bridge (MCP server `cua`: `notify_user`, the drive,
//!   and the user's computers the agent was granted);
//! - fires the agent's routines when they come due, with or without an app
//!   open;
//! - renews the lease.
//!
//! [`Persistent::pause`] stops the run, saves the home, releases the lease,
//! keeps routines from firing, and suspends a local Space. A cloud Space
//! cannot be suspended (Fleet has no per-claim pause), so pausing releases it
//! after the home is saved, and [`Persistent::resume`] creates it again
//! from the same image and restores the home.

pub mod access;
pub mod bridge;
pub mod home;
pub mod notify;

use crate::DriveResult as _;
use crate::SpacesDrive as _;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use cua_spaces::agents::{self as ag, Agents, RunOptions, RunStatus};
use cua_spaces::routines::{
    FileStorage, Routine, RoutineFiring, RoutineRunner, RoutineStore, Schedule,
};
use cua_spaces::{Error, Result, SpaceId, Spaces};

/// The directory under the cua home.
pub const DIR: &str = "persistent";
/// How long a home lease lasts without renewal.
pub const LEASE_TTL: Duration = Duration::from_secs(10 * 60);
/// Budget for one agent's share of a supervisor pass.
const AGENT_TICK_BUDGET: Duration = Duration::from_secs(60);

/// Reads a JSON array file (`[]` when missing).
pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Vec<T>> {
    match std::fs::read(path) {
        Ok(b) if b.iter().all(u8::is_ascii_whitespace) => Ok(vec![]),
        Ok(b) => Ok(serde_json::from_slice(&b)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(vec![]),
        Err(e) => Err(e.into()),
    }
}

/// Read-modify-write of a JSON array file under an exclusive lock that
/// every process honors.
pub(crate) fn locked_json<T, R>(path: &Path, f: impl FnOnce(&mut Vec<T>) -> R) -> Result<R>
where
    T: Serialize + for<'de> Deserialize<'de>,
{
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    cua_home::guard_write(path)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut items: Vec<T> = read_json(path)?;
    let r = f(&mut items);
    cua_home::write_private(path, &serde_json::to_vec_pretty(&items)?)?;
    Ok(r)
}

/// Where the agent's Space is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpaceState {
    /// Running (or at least not paused by us).
    #[default]
    Running,
    /// A local Space we suspended on pause.
    Suspended,
    /// A cloud Space we released on pause; resume creates it again.
    Released,
}

/// What resume needs to create a released cloud Space again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recreate {
    /// The image the Space ran.
    pub image: String,
    /// The Space's name (`cloud:<name>`).
    pub name: String,
}

/// One persistent agent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AgentRecord {
    pub name: String,
    /// Harness id (`claude-code`, `openai-codex`, `hermes`, `openclaw`, ...).
    pub harness: String,
    /// The Space it works in (a Space id).
    pub space: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A custom model endpoint base URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Provider key variables forwarded from the host's environment at
    /// each start (names only).
    #[serde(default)]
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets).
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub created_ms: u64,
    /// Paused: no runs, no routines, no notifications.
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub space_state: SpaceState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recreate: Option<Recreate>,
    /// The current run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Event cursor of the current run (the supervisor's bookmark).
    #[serde(default)]
    pub cursor: u64,
    /// The last turn whose home was saved.
    #[serde(default)]
    pub saved_turn: u32,
    /// Post a notification when a turn ends. Default true.
    #[serde(default = "yes")]
    pub notify: bool,
    /// Unix ms of the last home save.
    #[serde(default)]
    pub saved_ms: u64,
    /// The last problem the supervisor hit (cleared on success).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    /// The failed turn last notified (`<run>#<turn>`): one notification
    /// per failed turn, however many passes see its events.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_error: Option<String>,
}

fn yes() -> bool {
    true
}

/// A run `agent_start` started outside a persistent agent, followed by the
/// supervisor until its process exits so that a failed run or turn posts
/// one notification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchedRun {
    pub space: String,
    pub run_id: String,
    /// Harness id.
    pub agent: String,
    /// Unix ms.
    pub added_ms: u64,
    /// Event cursor (the supervisor's bookmark).
    #[serde(default)]
    pub cursor: u64,
    /// The last failed turn notified.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notified_turn: Option<u32>,
}

/// How long a watched run is followed at most.
pub const WATCH_FOR: Duration = Duration::from_secs(24 * 3600);
/// Watched runs kept (oldest dropped first).
const MAX_WATCHED: usize = 200;

/// How a persistent agent is created.
#[derive(Clone, Debug, Default)]
pub struct AgentSpec {
    pub name: String,
    pub harness: String,
    pub space: String,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub env_from_host: Vec<String>,
    /// More environment for every run (not secrets).
    pub env: std::collections::BTreeMap<String, String>,
}

/// What [`Persistent::send`] did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Delivery {
    pub run_id: String,
    /// A new run was started (with the home restored) rather than a
    /// follow-up sent.
    pub started: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored: Option<home::Transfer>,
}

/// What [`Persistent::pause`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct PauseReport {
    pub stopped_run: Option<String>,
    pub saved: Option<home::Transfer>,
    pub space_state: SpaceState,
    pub millis: u64,
}

/// What [`Persistent::resume`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ResumeReport {
    pub space: String,
    /// The cloud Space was created again.
    pub recreated: bool,
    pub restored: Option<home::Transfer>,
    pub run_id: Option<String>,
    /// From the call to the Space being ready with the home restored.
    pub ready_ms: u64,
}

/// One supervisor pass.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TickReport {
    pub fired: Vec<String>,
    pub saved: Vec<String>,
    pub notified: usize,
    pub bridged: usize,
    pub errors: Vec<String>,
}

#[derive(Default)]
pub(crate) struct Live {
    leases: HashMap<String, cua_volume::lease::Lease>,
    agents: HashMap<String, Agents>,
    bridges: HashMap<String, bridge::BridgeState>,
    routines: Option<Arc<tokio::sync::Mutex<RoutineStore>>>,
    routines_mtime: Option<std::time::SystemTime>,
    supervising: Option<std::fs::File>,
}

/// The persistent agents of one cua home. Cheap to clone.
#[derive(Clone)]
pub struct Persistent {
    pub(crate) spaces: Spaces,
    pub(crate) drive: cua_volume::Drive,
    dir: PathBuf,
    live: Arc<tokio::sync::Mutex<Live>>,
}

impl std::fmt::Debug for Persistent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Persistent")
            .field("dir", &self.dir)
            .finish()
    }
}

fn host_name() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "this-machine".into())
}

impl Persistent {
    pub(crate) fn new(
        spaces: Spaces,
        drive: cua_volume::Drive,
        live: Arc<tokio::sync::Mutex<Live>>,
    ) -> Persistent {
        let dir = spaces.home_dir().join(DIR);
        Persistent {
            spaces,
            drive,
            dir,
            live,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn agents_path(&self) -> PathBuf {
        self.dir.join("agents.json")
    }

    /// The notifications feed.
    pub fn feed(&self) -> notify::Feed {
        notify::Feed::new(&self.dir)
    }

    /// Per-agent access to the user's computers.
    pub fn access(&self) -> access::Access {
        access::Access::new(&self.dir)
    }

    fn routines_path(&self) -> PathBuf {
        self.dir.join("routines.json")
    }

    /// Every persistent agent.
    pub fn list(&self) -> Result<Vec<AgentRecord>> {
        read_json(&self.agents_path())
    }

    /// One persistent agent.
    pub fn get(&self, name: &str) -> Result<AgentRecord> {
        self.list()?
            .into_iter()
            .find(|a| a.name == name)
            .ok_or_else(|| Error::NotFound(format!("persistent agent {name}")))
    }

    fn update<R>(&self, name: &str, f: impl FnOnce(&mut AgentRecord) -> R) -> Result<R> {
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            all.iter_mut().find(|a| a.name == name).map(f)
        })?
        .ok_or_else(|| Error::NotFound(format!("persistent agent {name}")))
    }

    /// Creates a persistent agent (its home is created on first save).
    pub fn create(&self, spec: AgentSpec) -> Result<AgentRecord> {
        if !cua_volume::path::valid_agent_name(&spec.name) {
            return Err(Error::invalid(format!(
                "agent name {:?}: use 1-63 of a-z, 0-9, `.`, `_`, `-`",
                spec.name
            )));
        }
        let h = ag::harness::harness(&spec.harness).ok_or_else(|| {
            Error::invalid(format!(
                "unknown harness {:?}; known: {}",
                spec.harness,
                ag::harness::ids().join(", ")
            ))
        })?;
        let space = self.spaces.resolve(&spec.space)?.to_string();
        let allowed = cua_spaces::agents::forwardable_env();
        let saved = if spec.env_from_host.is_empty() {
            vec![]
        } else {
            cua_spaces::agents::keys::global().names()
        };
        for k in &spec.env_from_host {
            if !allowed.contains(&k.as_str()) && !saved.contains(k) {
                return Err(Error::invalid(format!(
                    "{k} is not a provider key variable (or a key saved in Cua Spaces → Settings → Agents)"
                )));
            }
        }
        let rec = AgentRecord {
            name: spec.name.clone(),
            harness: h.id.into(),
            space,
            model: spec.model,
            base_url: spec.base_url,
            env_from_host: spec.env_from_host,
            env: spec.env,
            created_ms: cua_volume::now_ms(),
            paused: false,
            space_state: SpaceState::Running,
            recreate: None,
            run_id: None,
            cursor: 0,
            saved_turn: 0,
            notify: true,
            saved_ms: 0,
            last_error: None,
            notified_error: None,
        };
        let out = rec.clone();
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            if all.iter().any(|a| a.name == rec.name) {
                return Err(Error::invalid(format!(
                    "a persistent agent named {} exists",
                    rec.name
                )));
            }
            all.push(rec);
            Ok(())
        })??;
        Ok(out)
    }

    /// Forgets a persistent agent (its home stays in the drive). Its run
    /// is stopped and its routines removed.
    pub async fn remove(&self, name: &str) -> Result<AgentRecord> {
        let rec = self.get(name)?;
        if rec.run_id.is_some() && !rec.paused {
            let _ = self.stop_run(&rec).await;
        }
        self.release_lease(name).await;
        // A home on the mounted volume: its writes land, and the volume
        // shows the Space's own view again.
        let _ = self.spaces.volume_release_agent(&rec.space, name).await;
        // Its home stays, but without the drive's bookkeeping (the reserved
        // sync manifest, the lease), so the user can delete it entirely.
        let _ = self.drive.forget_agent_home(name).await;
        let store = self.routine_store().await?;
        {
            let mut s = store.lock().await;
            for r in s.routines_for(name) {
                s.delete(&r.id);
            }
        }
        locked_json(&self.agents_path(), |all: &mut Vec<AgentRecord>| {
            all.retain(|a| a.name != name)
        })?;
        Ok(rec)
    }

    fn session(&self, rec: &AgentRecord) -> cua_volume::Session {
        self.drive
            .session(cua_volume::Context::agent(&rec.name, Some(&rec.space)))
    }

    /// A fresh agent runner for `space` (one guest round trip), remembered
    /// for the supervisor. User-initiated calls always take a fresh one, so
    /// a Space deleted and created again is never reached through a stale
    /// connection.
    async fn agents_for(&self, space: &str) -> Result<Agents> {
        let s = self.spaces.space(space).await?;
        let a = s.agents().await?;
        self.live
            .lock()
            .await
            .agents
            .insert(space.to_string(), a.clone());
        Ok(a)
    }

    /// The supervisor's runner for `space`: remembered between passes and
    /// dropped when a pass fails.
    async fn cached_agents(&self, space: &str) -> Result<Agents> {
        if let Some(a) = self.live.lock().await.agents.get(space) {
            return Ok(a.clone());
        }
        self.agents_for(space).await
    }

    async fn forget_space(&self, space: &str) {
        self.live.lock().await.agents.remove(space);
        let _ = self.spaces.forget_connection(space).await;
    }

    async fn take_lease(&self, rec: &AgentRecord) -> Result<()> {
        let holder = format!("{}/{}", host_name(), rec.space);
        let lease = cua_volume::lease::Lease::acquire(
            &self.drive,
            &rec.name,
            &holder,
            LEASE_TTL.as_millis() as u64,
        )
        .await
        .drive()?;
        self.live
            .lock()
            .await
            .leases
            .insert(rec.name.clone(), lease);
        Ok(())
    }

    async fn release_lease(&self, name: &str) {
        let lease = self.live.lock().await.leases.remove(name);
        if let Some(l) = lease {
            let _ = l.release(&self.drive).await;
        }
    }

    async fn renew_lease(&self, name: &str) -> Result<()> {
        let mut live = self.live.lock().await;
        if let Some(l) = live.leases.get_mut(name)
            && l.info.expires_ms < cua_volume::now_ms() + LEASE_TTL.as_millis() as u64 / 2
        {
            l.renew(&self.drive, LEASE_TTL.as_millis() as u64)
                .await
                .drive()?;
        }
        Ok(())
    }

    /// The agent's home on its Space's mounted Cua Volume, when the Space
    /// has one this agent can hold (`<mount>/agents/<name>`); `None` means
    /// the home is copied in and out.
    async fn mounted_home(&self, rec: &AgentRecord) -> Option<String> {
        match self.spaces.volume_for_agent(&rec.space, &rec.name).await {
            Ok(Some(v)) => {
                return Some(format!(
                    "{}/agents/{}",
                    v.mount_path.trim_end_matches('/'),
                    rec.name
                ));
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(agent = %rec.name, "copying the home: the volume is unavailable: {e}")
            }
        }
        None
    }

    /// Restores the home into the agent's Space (without starting a run).
    /// Nothing moves when the home is on the Space's mounted volume.
    pub async fn restore(&self, name: &str) -> Result<home::Transfer> {
        let rec = self.get(name)?;
        let t0 = Instant::now();
        if self.mounted_home(&rec).await.is_some() {
            return Ok(home::Transfer::mounted(t0.elapsed().as_millis() as u64));
        }
        let agents = self.agents_for(&rec.space).await?;
        let dir = agents.agent_home_dir(&rec.name);
        home::restore(agents.guest(), &self.session(&rec), &rec.name, &dir).await
    }

    /// Saves the agent's home from its Space into the drive now (on the
    /// mounted volume: lands the writes still pending).
    pub async fn save(&self, name: &str) -> Result<home::Transfer> {
        let rec = self.get(name)?;
        let t0 = Instant::now();
        let flushed = self
            .spaces
            .volume_flush_agent(&rec.space, &rec.name)
            .await?;
        let t = if flushed {
            home::Transfer::mounted(t0.elapsed().as_millis() as u64)
        } else {
            let agents = self.agents_for(&rec.space).await?;
            let dir = agents.agent_home_dir(&rec.name);
            home::save(agents.guest(), &self.session(&rec), &rec.name, &dir).await?
        };
        self.update(name, |r| {
            r.saved_ms = cua_volume::now_ms();
            r.last_error = None;
        })?;
        Ok(t)
    }

    /// Starts a run of `name` on `prompt`: takes the home's lease, restores
    /// the home, and starts the harness with its memory in the home and
    /// the bridge attached.
    pub async fn start(&self, name: &str, prompt: &str) -> Result<Delivery> {
        self.start_from(name, prompt, "persistent").await
    }

    /// [`Self::start`], recording `entry` (`persistent` or `routine`) as
    /// where the run was started from.
    async fn start_from(&self, name: &str, prompt: &str, entry: &str) -> Result<Delivery> {
        let rec = self.get(name)?;
        if rec.paused {
            return Err(Error::Agent(format!(
                "{name} is paused; resume it first (agent_resume)"
            )));
        }
        self.take_lease(&rec).await?;
        let agents = self.agents_for(&rec.space).await?;
        let t0 = Instant::now();
        let mounted = self.mounted_home(&rec).await;
        let restored = match &mounted {
            Some(_) => Ok(home::Transfer::mounted(t0.elapsed().as_millis() as u64)),
            None => {
                let dir = agents.agent_home_dir(&rec.name);
                home::restore(agents.guest(), &self.session(&rec), &rec.name, &dir).await
            }
        };
        let restored = match restored {
            Ok(t) => t,
            Err(e) => {
                self.release_lease(name).await;
                return Err(e);
            }
        };
        // Its env, its env_from_host keys, and the keys saved in Cua Spaces
        // that its harness reads.
        let (env, missing) = cua_spaces::agents::run_env(
            &rec.harness,
            &rec.env,
            &rec.env_from_host,
            rec.base_url.is_some(),
        )?;
        if !missing.is_empty() {
            self.release_lease(name).await;
            return Err(Error::invalid(format!(
                "not set in this server's environment: {}",
                missing.join(", ")
            )));
        }
        let endpoint = rec.base_url.clone().map(|base_url| ag::Endpoint {
            base_url,
            wire: None,
            model: rec.model.clone(),
        });
        let started = agents
            .start(
                &rec.harness,
                prompt,
                RunOptions {
                    env,
                    endpoint,
                    model: rec.model.clone(),
                    home: Some(rec.name.clone()),
                    home_dir: mounted,
                    bridge: true,
                    label: Some(rec.name.clone()),
                    ..Default::default()
                },
            )
            .await;
        let started = match started {
            Ok(s) => {
                cua_spaces::agents::telemetry::started(&agents, &rec.space, entry, &s);
                s
            }
            Err(e) => {
                self.release_lease(name).await;
                let e = Error::from(e);
                cua_spaces::agents::telemetry::start_failed(&rec.harness, &rec.space, entry, &e);
                let msg = e.to_string();
                let _ = self.update(name, |r| r.last_error = Some(msg.clone()));
                return Err(e);
            }
        };
        self.update(name, |r| {
            r.run_id = Some(started.run_id.clone());
            r.cursor = 0;
            r.saved_turn = 0;
            r.last_error = None;
        })?;
        Ok(Delivery {
            run_id: started.run_id,
            started: true,
            restored: Some(restored),
        })
    }

    /// Gives `name` a turn: a follow-up to its idle run, or a new run (with
    /// the home restored) when it has none. Refused while a turn runs or
    /// while paused.
    pub async fn send(&self, name: &str, text: &str) -> Result<Delivery> {
        self.send_from(name, text, "persistent").await
    }

    /// [`Self::send`], recording `entry` for a run it starts.
    async fn send_from(&self, name: &str, text: &str, entry: &str) -> Result<Delivery> {
        let rec = self.get(name)?;
        if rec.paused {
            return Err(Error::Agent(format!("{name} is paused")));
        }
        if let Some(run) = &rec.run_id {
            let agents = self.agents_for(&rec.space).await?;
            match agents.status(run).await {
                Ok(st) if st.status == RunStatus::Running => {
                    return Err(Error::Agent(format!(
                        "{name} is in the middle of a turn; try again when it ends"
                    )));
                }
                Ok(st) if st.accepts_message && st.status == RunStatus::Idle => {
                    if !self.live.lock().await.leases.contains_key(name) {
                        self.take_lease(&rec).await?;
                    }
                    agents.send(run, text, vec![]).await?;
                    return Ok(Delivery {
                        run_id: run.clone(),
                        started: false,
                        restored: None,
                    });
                }
                // Failed, crashed, gone or unreadable: start fresh (the
                // home carries the memory).
                _ => {}
            }
        }
        self.start_from(name, text, entry).await
    }

    async fn stop_run(&self, rec: &AgentRecord) -> Result<Option<String>> {
        let Some(run) = &rec.run_id else {
            return Ok(None);
        };
        let agents = self.agents_for(&rec.space).await?;
        cua_spaces::agents::telemetry::before_stop(&agents, run).await;
        agents.stop(run).await?;
        cua_spaces::agents::telemetry::stopped(run);
        Ok(Some(run.clone()))
    }

    /// Pauses `name` in one call: stops its run, saves its home, releases
    /// the lease, keeps its routines from firing, and suspends a local
    /// Space (a cloud Space is released; resume creates it again).
    pub async fn pause(&self, name: &str) -> Result<PauseReport> {
        let t0 = Instant::now();
        let rec = self.get(name)?;
        if rec.paused {
            return Ok(PauseReport {
                space_state: rec.space_state,
                ..Default::default()
            });
        }
        // Routines and notifications stop first, so nothing starts the
        // agent again while it is being paused.
        self.update(name, |r| r.paused = true)?;
        let mut report = PauseReport::default();
        let result: Result<()> = async {
            report.stopped_run = self.stop_run(&rec).await?;
            report.saved = Some(self.save(name).await?);
            self.release_lease(name).await;
            self.spaces
                .volume_release_agent(&rec.space, name)
                .await?;
            let id = self.spaces.resolve(&rec.space)?;
            match &id {
                SpaceId::Local { name: sandbox } => {
                    // The guest goes to sleep: unmount first.
                    self.spaces.volume_detach(&rec.space).await?;
                    self.forget_space(&rec.space).await;
                    self.spaces
                        .sandboxes()
                        .suspend(sandbox)
                        .await
                        .map_err(Error::Sandbox)?;
                    report.space_state = SpaceState::Suspended;
                }
                SpaceId::Cloud { name: claim, .. } => {
                    let image = self
                        .spaces
                        .list()?
                        .into_iter()
                        .find(|s| s.id == rec.space)
                        .map(|s| s.image)
                        .filter(|i| !i.is_empty())
                        .ok_or_else(|| {
                            Error::invalid(format!(
                                "{} has no recorded image, so it could not be created again; not releasing it",
                                rec.space
                            ))
                        })?;
                    self.forget_space(&rec.space).await;
                    self.spaces.delete(&rec.space).await?;
                    let recreate = Recreate {
                        image,
                        name: claim.clone(),
                    };
                    self.update(name, |r| r.recreate = Some(recreate))?;
                    report.space_state = SpaceState::Released;
                }
                // Someone's own machine or an added address: only the agent
                // pauses.
                _ => report.space_state = SpaceState::Running,
            }
            Ok(())
        }
        .await;
        let state = report.space_state;
        self.update(name, |r| {
            r.space_state = state;
            r.run_id = None;
            r.last_error = result.as_ref().err().map(|e| e.to_string());
        })?;
        result?;
        report.millis = t0.elapsed().as_millis() as u64;
        Ok(report)
    }

    /// Resumes `name`: resumes (or creates again) its Space, restores its
    /// home, lets its routines fire again, and when `prompt` is given
    /// starts a run on it.
    pub async fn resume(&self, name: &str, prompt: Option<&str>) -> Result<ResumeReport> {
        let t0 = Instant::now();
        let rec = self.get(name)?;
        let mut report = ResumeReport {
            space: rec.space.clone(),
            ..Default::default()
        };
        match rec.space_state {
            SpaceState::Running => {}
            SpaceState::Suspended => {
                if let SpaceId::Local { name: sandbox } = self.spaces.resolve(&rec.space)? {
                    self.spaces
                        .sandboxes()
                        .resume(&sandbox)
                        .await
                        .map_err(Error::Sandbox)?;
                    self.forget_space(&rec.space).await;
                }
            }
            SpaceState::Released => {
                let re = rec.recreate.clone().ok_or_else(|| {
                    Error::invalid(format!(
                        "{name}'s Space was released without a record of its image"
                    ))
                })?;
                let created = self
                    .spaces
                    .create(cua_spaces::SpaceCreate {
                        image: Some(re.image.clone()),
                        on: Some(cua_sandbox_core::placement::On::Cloud),
                        name: Some(re.name.clone()),
                        wait: Some(true),
                        ..Default::default()
                    })
                    .await?;
                let info = match created {
                    cua_spaces::SpaceCreated::Ready { info, .. } => info,
                    cua_spaces::SpaceCreated::Starting(p) => {
                        return Err(Error::Timeout(format!("{} is still starting", p.id)));
                    }
                };
                report.recreated = true;
                report.space = info.id.clone();
                self.update(name, |r| {
                    r.space = info.id.clone();
                    r.recreate = None;
                })?;
            }
        }
        self.update(name, |r| {
            r.space_state = SpaceState::Running;
            r.paused = false;
        })?;
        report.restored = Some(self.restore(name).await?);
        report.ready_ms = t0.elapsed().as_millis() as u64;
        if let Some(p) = prompt {
            report.run_id = Some(self.start(name, p).await?.run_id);
        }
        Ok(report)
    }

    // --- routines -------------------------------------------------------

    async fn routine_store(&self) -> Result<Arc<tokio::sync::Mutex<RoutineStore>>> {
        let mut live = self.live.lock().await;
        let path = self.routines_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if let Some(store) = live.routines.clone() {
            if live.routines_mtime != mtime {
                store.lock().await.load();
                live.routines_mtime = mtime;
            }
            return Ok(store);
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut store = RoutineStore::new(Box::new(FileStorage(path)));
        store.load();
        store.attach(Arc::new(Runner(self.clone())));
        let store = Arc::new(tokio::sync::Mutex::new(store));
        live.routines = Some(store.clone());
        live.routines_mtime = mtime;
        Ok(store)
    }

    async fn after_routine_write(&self) {
        let mut live = self.live.lock().await;
        live.routines_mtime = std::fs::metadata(self.routines_path())
            .and_then(|m| m.modified())
            .ok();
    }

    /// Adds a routine for `agent`.
    pub async fn routine_add(
        &self,
        agent: &str,
        title: &str,
        prompt: &str,
        schedule: Schedule,
        enabled: bool,
    ) -> Result<Routine> {
        self.get(agent)?;
        if schedule.next_fire(Utc::now()).is_none() {
            return Err(Error::invalid("that schedule never fires"));
        }
        let store = self.routine_store().await?;
        let r = store
            .lock()
            .await
            .create(agent, title, prompt, schedule, enabled, Utc::now());
        self.after_routine_write().await;
        Ok(r)
    }

    /// Routines (of one agent, or all).
    pub async fn routines(&self, agent: Option<&str>) -> Result<Vec<Routine>> {
        let store = self.routine_store().await?;
        let s = store.lock().await;
        Ok(match agent {
            Some(a) => s.routines_for(a),
            None => {
                let mut all: Vec<Routine> = self
                    .list()?
                    .iter()
                    .flat_map(|a| s.routines_for(&a.name))
                    .collect();
                all.sort_by_key(|r| r.created_at);
                all
            }
        })
    }

    /// Removes a routine.
    pub async fn routine_remove(&self, id: &str) -> Result<()> {
        let store = self.routine_store().await?;
        let mut s = store.lock().await;
        if s.routine(id).is_none() {
            return Err(Error::NotFound(format!("routine {id}")));
        }
        s.delete(id);
        drop(s);
        self.after_routine_write().await;
        Ok(())
    }

    /// Turns a routine on or off.
    pub async fn routine_set_enabled(&self, id: &str, enabled: bool) -> Result<Routine> {
        let store = self.routine_store().await?;
        let mut s = store.lock().await;
        if s.routine(id).is_none() {
            return Err(Error::NotFound(format!("routine {id}")));
        }
        s.set_enabled(id, enabled);
        let r = s.routine(id).cloned().expect("present");
        drop(s);
        self.after_routine_write().await;
        Ok(r)
    }

    // --- runs started with agent_start --------------------------------

    fn watched_path(&self) -> PathBuf {
        self.dir.join("watched-runs.json")
    }

    /// Follows `run_id` (an `agent_start` run in `space` of harness
    /// `agent`) until its process exits; the supervisor posts one
    /// notification for each failed turn, or for the run failing.
    pub fn watch_run(&self, space: &str, run_id: &str, agent: &str) -> Result<()> {
        let w = WatchedRun {
            space: space.into(),
            run_id: run_id.into(),
            agent: agent.into(),
            added_ms: cua_volume::now_ms(),
            cursor: 0,
            notified_turn: None,
        };
        locked_json(&self.watched_path(), |all: &mut Vec<WatchedRun>| {
            if !all
                .iter()
                .any(|x| x.run_id == w.run_id && x.space == w.space)
            {
                all.push(w);
            }
            let extra = all.len().saturating_sub(MAX_WATCHED);
            all.drain(..extra);
        })
    }

    /// The runs being followed.
    pub fn watched_runs(&self) -> Result<Vec<WatchedRun>> {
        read_json(&self.watched_path())
    }

    /// One pass over the watched runs.
    async fn tick_watched(&self, report: &mut TickReport) {
        let runs = match self.watched_runs() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("watched runs: {e}"));
                return;
            }
        };
        if runs.is_empty() {
            return;
        }
        let mut next: HashMap<(String, String), Option<WatchedRun>> = HashMap::new();
        for w in runs {
            let key = (w.space.clone(), w.run_id.clone());
            let expired =
                cua_volume::now_ms().saturating_sub(w.added_ms) > WATCH_FOR.as_millis() as u64;
            if expired {
                next.insert(key, None);
                continue;
            }
            match tokio::time::timeout(AGENT_TICK_BUDGET, self.tick_watched_run(w, report)).await {
                Ok(Ok(w)) => {
                    next.insert(key, w);
                }
                Ok(Err(e)) => {
                    self.forget_space(&key.0).await;
                    report.errors.push(format!("{}: {e}", key.1));
                }
                Err(_) => report
                    .errors
                    .push(format!("{}: supervisor pass timed out", key.1)),
            }
        }
        let saved = locked_json(&self.watched_path(), |all: &mut Vec<WatchedRun>| {
            all.retain_mut(|x| match next.get(&(x.space.clone(), x.run_id.clone())) {
                Some(Some(w)) => {
                    *x = w.clone();
                    true
                }
                Some(None) => false,
                None => true,
            });
        });
        if let Err(e) = saved {
            report.errors.push(format!("watched runs: {e}"));
        }
    }

    /// Reads `w`'s new events and status; posts what failed. `None` once
    /// there is nothing left to follow.
    async fn tick_watched_run(
        &self,
        mut w: WatchedRun,
        report: &mut TickReport,
    ) -> Result<Option<WatchedRun>> {
        let agents = self.cached_agents(&w.space).await?;
        let name = ag::harness::harness(&w.agent).map_or(w.agent.as_str(), |h| h.name);
        let page = agents.events(&w.run_id, w.cursor, 500).await?;
        for (turn, err) in failed_turns(&page.events) {
            if w.notified_turn.is_none_or(|t| turn > t) {
                self.post_stopped(&w, name, &err)?;
                w.notified_turn = Some(turn);
                report.notified += 1;
            }
        }
        w.cursor = page.cursor;
        if !page.caught_up {
            return Ok(Some(w));
        }
        let info = match agents.status(&w.run_id).await {
            Ok(info) => info,
            // Removed: nothing to follow.
            Err(cua_agents::Error::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        // A run that failed without an error event (the runner recorded
        // why in its state).
        if info.status == RunStatus::Failed && w.notified_turn.is_none_or(|t| info.turn > t) {
            let err = info.error.clone().unwrap_or_else(|| info.reason.clone());
            self.post_stopped(&w, name, &err)?;
            w.notified_turn = Some(info.turn);
            report.notified += 1;
        }
        Ok((info.alive != Some(false)).then_some(w))
    }

    fn post_stopped(&self, w: &WatchedRun, name: &str, err: &str) -> Result<()> {
        self.feed().post(
            None,
            "error",
            &notify::stopped_title(name, err),
            err,
            Some(&w.run_id),
            Some(&w.space),
        )?;
        Ok(())
    }

    // --- the supervisor -------------------------------------------------

    /// Takes the supervisor lock for this cua home (one supervising
    /// process: the daemon). `false` when another process holds it.
    pub async fn claim_supervisor(&self) -> Result<bool> {
        let mut live = self.live.lock().await;
        if live.supervising.is_some() {
            return Ok(true);
        }
        std::fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("supervisor.lock");
        cua_home::guard_write(&path)?;
        let f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)?;
        match f.try_lock() {
            Ok(()) => {
                live.supervising = Some(f);
                Ok(true)
            }
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    /// One supervisor pass: fire due routines, then for every live run
    /// answer its bridge, save the home after each finished turn (and
    /// notify), and renew its lease. Only the process holding the
    /// supervisor lock does anything.
    pub async fn tick(&self) -> TickReport {
        let mut report = TickReport::default();
        match self.claim_supervisor().await {
            Ok(true) => {}
            Ok(false) => return report,
            Err(e) => {
                report.errors.push(format!("supervisor lock: {e}"));
                return report;
            }
        }
        match self.routine_store().await {
            Ok(store) => {
                for f in cua_spaces::routines::tick_shared(&store, Utc::now()).await {
                    report
                        .fired
                        .push(format!("{}: {}", f.title, f.firing.summary()));
                }
                self.after_routine_write().await;
            }
            Err(e) => report.errors.push(format!("routines: {e}")),
        }
        let records = match self.list() {
            Ok(r) => r,
            Err(e) => {
                report.errors.push(format!("agents: {e}"));
                return report;
            }
        };
        for rec in records
            .into_iter()
            .filter(|r| !r.paused && r.run_id.is_some())
        {
            let name = rec.name.clone();
            match tokio::time::timeout(AGENT_TICK_BUDGET, self.tick_agent(rec, &mut report)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    let msg = e.to_string();
                    let _ = self.update(&name, |r| r.last_error = Some(msg.clone()));
                    report.errors.push(format!("{name}: {msg}"));
                }
                Err(_) => report
                    .errors
                    .push(format!("{name}: supervisor pass timed out")),
            }
        }
        self.tick_watched(&mut report).await;
        report
    }

    async fn tick_agent(&self, rec: AgentRecord, report: &mut TickReport) -> Result<()> {
        let space = rec.space.clone();
        let r = self.tick_agent_inner(rec, report).await;
        if r.is_err() {
            // Reconnect on the next pass (the Space may have been recreated).
            self.forget_space(&space).await;
        }
        r
    }

    async fn tick_agent_inner(&self, rec: AgentRecord, report: &mut TickReport) -> Result<()> {
        let run = rec.run_id.clone().expect("filtered");
        let agents = self.cached_agents(&rec.space).await?;
        // The bridge first: an agent may be waiting on it mid-turn.
        let run_dir = agents.run_dir(&run);
        let server = bridge::server(&self.spaces, self.clone(), &rec, &run);
        let mut state = self
            .live
            .lock()
            .await
            .bridges
            .remove(&run)
            .unwrap_or_default();
        let bridged = bridge::pump(agents.guest(), &run_dir, &mut state, &server).await;
        self.live.lock().await.bridges.insert(run.clone(), state);
        report.bridged += bridged?;
        // Then the run's events since the bookmark.
        let page = agents.events(&run, rec.cursor, 500).await?;
        let (ended, failed) = turn_outcome(&page.events);
        if let Some((turn, err)) = &failed {
            // One notification per failed turn, posted before anything
            // below can fail this pass (a later pass sees the same events).
            let key = format!("{run}#{turn}");
            let notify = rec.notify && rec.notified_error.as_deref() != Some(key.as_str());
            if notify {
                self.feed().post(
                    Some(&rec.name),
                    "error",
                    &notify::stopped_title(&rec.name, err),
                    err,
                    Some(&run),
                    Some(&rec.space),
                )?;
                report.notified += 1;
            }
            let err = err.clone();
            self.update(&rec.name, |r| {
                r.last_error = Some(err);
                r.notified_error = Some(key);
            })?;
        }
        if let Some(turn) = ended
            && turn > rec.saved_turn
        {
            let saved = self.save(&rec.name).await?;
            report.saved.push(format!(
                "{} turn {turn}: {} files, {} bytes, {} blocked",
                rec.name,
                saved.files,
                saved.bytes,
                saved.blocked.len()
            ));
            if !saved.blocked.is_empty() {
                let what: Vec<String> = saved
                    .blocked
                    .iter()
                    .map(|(p, k)| format!("{p} ({k})"))
                    .collect();
                self.feed().post(
                    Some(&rec.name),
                    "error",
                    &format!("{} kept a secret out of its memory", rec.name),
                    &format!(
                        "Not saved to the drive: {}. Secrets belong in the Keyvault.",
                        what.join(", ")
                    ),
                    Some(&run),
                    Some(&rec.space),
                )?;
                report.notified += 1;
            }
            if rec.notify && !ended_in_error(turn, failed.as_ref()) {
                let result = agents.result(&run).await?;
                let body = if result.text.trim().is_empty() {
                    "Finished.".to_string()
                } else {
                    result.text.trim().to_string()
                };
                self.feed().post(
                    Some(&rec.name),
                    "turn_ended",
                    &rec.name,
                    &body,
                    Some(&run),
                    Some(&rec.space),
                )?;
                report.notified += 1;
            }
            self.update(&rec.name, |r| r.saved_turn = turn)?;
        }
        let run_failed = failed.is_some();
        // Agent-run telemetry: the end of a run this install started.
        if (ended.is_some() || run_failed)
            && cua_telemetry::global().agent_run_pending(&run)
            && let Ok(info) = agents.status(&run).await
        {
            cua_spaces::agents::telemetry::observe(&info);
        }
        if page.cursor != rec.cursor {
            let cursor = page.cursor;
            self.update(&rec.name, |r| r.cursor = cursor)?;
        }
        self.renew_lease(&rec.name).await?;
        Ok(())
    }
}

/// In a page of a run's events: the last turn that ended, and the last
/// error (its turn and message).
fn turn_outcome(events: &[ag::AgentEvent]) -> (Option<u32>, Option<(u32, String)>) {
    let ended = events
        .iter()
        .filter(|e| e.kind == "turn_ended")
        .map(|e| e.turn)
        .max();
    (ended, last_failed_turn(events))
}

/// The last turn with an error, and its first error (the cause; a later
/// one such as "the agent exited" follows from it).
fn last_failed_turn(events: &[ag::AgentEvent]) -> Option<(u32, String)> {
    let turn = events.iter().rev().find(|e| e.kind == "error")?.turn;
    failed_turns(events).into_iter().find(|(t, _)| *t == turn)
}

/// Every turn with an error, in order, each with its first error.
fn failed_turns(events: &[ag::AgentEvent]) -> Vec<(u32, String)> {
    let mut out: Vec<(u32, String)> = vec![];
    for e in events.iter().filter(|e| e.kind == "error") {
        if out.last().is_none_or(|(t, _)| *t != e.turn) {
            out.push((e.turn, e.text.clone().unwrap_or_default()));
        }
    }
    out
}

/// Whether `turn` ended in `failed`'s error: it then gets the error's
/// notification only, not a "Finished." as well.
fn ended_in_error(turn: u32, failed: Option<&(u32, String)>) -> bool {
    failed.is_some_and(|(t, _)| *t >= turn)
}

/// Fires a routine as a turn of its agent (never interrupting one).
struct Runner(Persistent);

#[async_trait::async_trait]
impl RoutineRunner for Runner {
    async fn fire(&self, routine: &Routine) -> RoutineFiring {
        match self.0.get(&routine.bot_id) {
            Ok(rec) if rec.paused => {
                return RoutineFiring::Refused {
                    reason: format!("{} is paused", rec.name),
                };
            }
            Ok(_) => {}
            Err(e) => {
                return RoutineFiring::Failed {
                    reason: e.to_string(),
                };
            }
        }
        match self
            .0
            .send_from(&routine.bot_id, &routine.turn_text(), "routine")
            .await
        {
            Ok(d) => RoutineFiring::Started { run_id: d.run_id },
            Err(Error::Agent(m)) if m.contains("middle of a turn") => {
                RoutineFiring::Refused { reason: m }
            }
            Err(e) => RoutineFiring::Failed {
                reason: e.to_string(),
            },
        }
    }
}

/// Persistent agents on a [`Spaces`] runtime that registered the
/// [`crate::DriveExtension`].
pub trait SpacesPersistent {
    /// The persistent agents of this runtime's cua home.
    ///
    /// # Panics
    ///
    /// When the runtime has no [`crate::DriveExtension`] (register one with
    /// [`crate::register`]).
    fn persistent(&self) -> Persistent;

    /// Runs [`Persistent::tick`] every `every` until the returned task is
    /// aborted (the daemon runs one for its lifetime).
    fn spawn_supervisor(&self, every: Duration) -> tokio::task::JoinHandle<()> {
        let p = self.persistent();
        tokio::spawn(async move {
            loop {
                let r = p.tick().await;
                for e in &r.errors {
                    tracing::warn!("persistent agents: {e}");
                }
                tokio::time::sleep(every).await;
            }
        })
    }
}

impl SpacesPersistent for Spaces {
    fn persistent(&self) -> Persistent {
        let ext = self
            .extension::<crate::DriveExtension>()
            .expect("persistent agents need the Cua Volume extension (cua_spaces_ext::register)");
        Persistent::new(self.clone(), ext.drive().clone(), ext.live())
    }
}

/// Parses a routine schedule from the tool arguments: exactly one of
/// `every_minutes`, `daily_at` (`HH:MM`) or `weekly_on` (`<weekday> HH:MM`).
pub fn parse_schedule(
    every_minutes: Option<i64>,
    daily_at: Option<&str>,
    weekly_on: Option<&str>,
) -> Result<Schedule> {
    let hm = |s: &str| -> Result<(u32, u32)> {
        let (h, m) = s
            .trim()
            .split_once(':')
            .ok_or_else(|| Error::invalid(format!("{s:?}: use HH:MM")))?;
        let (h, m): (u32, u32) = (
            h.parse()
                .map_err(|_| Error::invalid(format!("{s:?}: use HH:MM")))?,
            m.parse()
                .map_err(|_| Error::invalid(format!("{s:?}: use HH:MM")))?,
        );
        if h > 23 || m > 59 {
            return Err(Error::invalid(format!("{s:?}: out of range")));
        }
        Ok((h, m))
    };
    match (every_minutes, daily_at, weekly_on) {
        (Some(m), None, None) if m > 0 => Ok(Schedule::EveryMinutes { minutes: m }),
        (Some(_), None, None) => Err(Error::invalid("every_minutes must be at least 1")),
        (None, Some(t), None) => {
            let (hour, minute) = hm(t)?;
            Ok(Schedule::DailyAt { hour, minute })
        }
        (None, None, Some(w)) => {
            let (day, t) = w
                .trim()
                .split_once(' ')
                .ok_or_else(|| Error::invalid(format!("{w:?}: use `<weekday> HH:MM`")))?;
            let days = [
                "sunday",
                "monday",
                "tuesday",
                "wednesday",
                "thursday",
                "friday",
                "saturday",
            ];
            let d = day.to_ascii_lowercase();
            let weekday = days
                .iter()
                .position(|x| x.starts_with(&d) && d.len() >= 3)
                .ok_or_else(|| Error::invalid(format!("{day:?}: not a weekday")))?
                as u32
                + 1;
            let (hour, minute) = hm(t)?;
            Ok(Schedule::WeeklyOn {
                weekday,
                hour,
                minute,
            })
        }
        _ => Err(Error::invalid(
            "give exactly one of every_minutes, daily_at or weekly_on",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // An events.jsonl line as the runner writes it.
    fn ev(kind: &str, turn: u32, text: Option<&str>) -> ag::AgentEvent {
        let line =
            serde_json::json!({"seq": 1, "ts": 0, "turn": turn, "type": kind, "message": text});
        ag::AgentEvent::parse(&line.to_string()).unwrap()
    }

    #[test]
    fn a_failed_turn_is_one_error_not_finished_plus_an_error() {
        // The runner's failed turn: an error, then turn_ended (stop: error).
        let evs = [
            ev("turn_started", 1, None),
            ev("error", 1, Some("Authentication required")),
            ev("turn_ended", 1, None),
        ];
        let (ended, failed) = turn_outcome(&evs);
        assert_eq!(ended, Some(1));
        assert_eq!(failed, Some((1, "Authentication required".into())));
        assert!(ended_in_error(1, failed.as_ref()));
        // An earlier turn's error does not hide a later turn's result.
        let evs = [
            ev("error", 1, Some("x")),
            ev("turn_ended", 1, None),
            ev("turn_ended", 2, None),
        ];
        let (ended, failed) = turn_outcome(&evs);
        assert!(!ended_in_error(ended.unwrap(), failed.as_ref()));
        let (ended, failed) = turn_outcome(&[ev("turn_ended", 1, None)]);
        assert!(!ended_in_error(ended.unwrap(), failed.as_ref()));
    }

    #[test]
    fn schedules_parse() {
        assert_eq!(
            parse_schedule(Some(30), None, None).unwrap(),
            Schedule::EveryMinutes { minutes: 30 }
        );
        assert_eq!(
            parse_schedule(None, Some("08:05"), None).unwrap(),
            Schedule::DailyAt { hour: 8, minute: 5 }
        );
        assert_eq!(
            parse_schedule(None, None, Some("Mon 09:30")).unwrap(),
            Schedule::WeeklyOn {
                weekday: 2,
                hour: 9,
                minute: 30
            }
        );
        for bad in [
            parse_schedule(None, None, None),
            parse_schedule(Some(0), None, None),
            parse_schedule(Some(5), Some("08:00"), None),
            parse_schedule(None, Some("25:00"), None),
            parse_schedule(None, None, Some("Funday 09:00")),
        ] {
            assert_eq!(bad.unwrap_err().tag(), "invalid_argument");
        }
    }
}
