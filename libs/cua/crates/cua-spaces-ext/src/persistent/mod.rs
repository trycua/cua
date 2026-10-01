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
}

fn yes() -> bool {
    true
}

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
        for k in &spec.env_from_host {
            if !allowed.contains(&k.as_str()) {
                return Err(Error::invalid(format!(
                    "{k} is not a provider key variable"
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
        let (host_env, missing) = cua_spaces::agents::env_from_host(&rec.env_from_host)?;
        let mut env = rec.env.clone();
        env.extend(host_env);
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
            Ok(s) => s,
            Err(e) => {
                self.release_lease(name).await;
                return Err(e.into());
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
        self.start(name, text).await
    }

    async fn stop_run(&self, rec: &AgentRecord) -> Result<Option<String>> {
        let Some(run) = &rec.run_id else {
            return Ok(None);
        };
        let agents = self.agents_for(&rec.space).await?;
        agents.stop(run).await?;
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
        let ended: Vec<u32> = page
            .events
            .iter()
            .filter(|e| e.kind == "turn_ended")
            .map(|e| e.turn)
            .collect();
        let failed = page
            .events
            .iter()
            .rev()
            .find(|e| e.kind == "error")
            .and_then(|e| e.text.clone());
        if let Some(&turn) = ended.iter().max()
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
            if rec.notify {
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
        if let Some(err) = failed
            && page.cursor > rec.cursor
            && rec.notify
        {
            self.feed().post(
                Some(&rec.name),
                "error",
                &format!("{} ran into a problem", rec.name),
                &err,
                Some(&run),
                Some(&rec.space),
            )?;
            report.notified += 1;
        }
        if page.cursor != rec.cursor {
            let cursor = page.cursor;
            self.update(&rec.name, |r| r.cursor = cursor)?;
        }
        self.renew_lease(&rec.name).await?;
        Ok(())
    }
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
        match self.0.send(&routine.bot_id, &routine.turn_text()).await {
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
