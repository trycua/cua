// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Creates in flight: cancelling one, and finishing what a process that
//! died mid-create left.
//!
//! Every [`Spaces::create`] is registered here while it runs, under the id
//! the Space will have (`local:<name>`, `cloud:<name>`), its bare name, and
//! the caller's own key ([`crate::SpaceCreate::create_id`]). It also keeps a
//! journal, `<home>/creating/<stem>.json` (0600: it holds the Space's token
//! until the registry does), recording what the create made so far
//! ([`Made`]: an instance, a cloud claim, a relay machine, a Space on a
//! host).
//!
//! [`Spaces::cancel_create`] finds the create:
//!
//! - running in this process: its token fires, the create's future is
//!   dropped (every await in it stops: an image download, a boot, a claim,
//!   a relay registration), the drop guards' clean-up
//!   ([`cua_sandbox_core::cleanup`]) finishes, and what the journal records
//!   is undone;
//! - running in another live process (the CLI, the app's daemon): a
//!   `<stem>.cancel` marker asks it to do the same, and the call waits for
//!   its journal to go;
//! - left by a process that died (a daemon restart mid-create): the journal
//!   is undone here.
//!
//! Only what the create made is removed: an instance under a name that was
//! in use before is never touched. Image layers that finished downloading
//! stay cached, so a later create resumes; partial downloads are dropped.

use crate::error::{Error, Result};
use crate::spaces::Spaces;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// One thing a create made, recorded as soon as it exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Made {
    /// A local sandbox (VM or container). `fresh`: nothing had that name
    /// before, so it is the create's to delete.
    LocalSandbox {
        /// Sandbox name.
        name: String,
        /// Nothing had that name before.
        fresh: bool,
    },
    /// A Cua Cloud claim the create made.
    FleetClaim {
        /// Its namespace.
        namespace: String,
        /// Its name.
        name: String,
    },
    /// A relay machine the create registered.
    RelayMachine {
        /// Machine id.
        id: String,
    },
    /// A Space being created on a host that provides Spaces.
    HostSpace {
        /// The host's relay machine id.
        host: String,
        /// The Space's relay machine id.
        machine: String,
    },
}

/// What [`Spaces::cancel_create`] found and did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelState {
    /// The create was stopped and what it made removed.
    Cancelled,
    /// No create by that key is running (already finished, failed or
    /// cancelled): nothing to do.
    NotCreating,
    /// It already finished: the Space exists (delete it instead).
    AlreadyCreated,
}

impl CancelState {
    /// `cancelled`, `not_creating`, `already_created`.
    pub fn as_str(&self) -> &'static str {
        match self {
            CancelState::Cancelled => "cancelled",
            CancelState::NotCreating => "not_creating",
            CancelState::AlreadyCreated => "already_created",
        }
    }
}

/// The result of [`Spaces::cancel_create`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelOutcome {
    /// The Space id the create had (`local:<name>`), when known.
    pub id: String,
    /// What happened.
    pub state: CancelState,
    /// For people: what was removed and what stays.
    pub message: String,
}

/// The keys a create is found by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CreateKey {
    /// `local:<name>`, `cloud:<name>`; empty when not known up front (a
    /// Space on a host).
    pub id: String,
    /// Its name (empty when not known).
    pub name: String,
    /// The caller's own key.
    pub create_id: Option<String>,
    /// The journal's file stem (unique per create).
    pub stem: String,
    /// The name was generated for this create (not the caller's).
    pub generated: bool,
}

impl CreateKey {
    fn matches(&self, key: &str) -> bool {
        let key = key.trim();
        !key.is_empty()
            && (self.create_id.as_deref() == Some(key)
                || (!self.id.is_empty() && self.id == key)
                || (!self.name.is_empty() && self.name == key))
    }
}

/// `<home>/creating/<stem>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Journal {
    /// The sandbox (local) or claim name; for a local create, the key the
    /// crash recovery registers.
    pub name: String,
    /// The Space's token (local), until the registry holds it.
    #[serde(default)]
    pub token: String,
    pub pid: u32,
    #[serde(default = "yes")]
    pub spacesd: bool,
    /// Unix seconds it started.
    #[serde(default)]
    pub started: u64,
    /// `local`, `cloud`, `host`.
    #[serde(default = "local")]
    pub kind: String,
    /// The Space id (see [`CreateKey::id`]).
    #[serde(default)]
    pub id: String,
    /// The caller's key.
    #[serde(default)]
    pub create_id: Option<String>,
    /// What it made so far.
    #[serde(default)]
    pub made: Vec<Made>,
    /// The name was generated for this create (not the caller's).
    #[serde(default)]
    pub generated: bool,
}

fn yes() -> bool {
    true
}

fn local() -> String {
    "local".into()
}

pub(crate) fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn dir(home: &Path) -> PathBuf {
    home.join("creating")
}

fn path(home: &Path, stem: &str) -> PathBuf {
    dir(home).join(format!("{stem}.json"))
}

fn marker(home: &Path, stem: &str) -> PathBuf {
    dir(home).join(format!("{stem}.cancel"))
}

fn write_file(path: &Path, j: &Journal) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::fs::OpenOptions::new();
    f.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut f, 0o600);
    let mut file = f.open(path)?;
    std::io::Write::write_all(&mut file, &serde_json::to_vec(j)?)
}

/// A create's live journal: updated as it makes things, removed when the
/// create returns (a process that dies leaves it).
#[derive(Clone)]
pub(crate) struct JournalHandle {
    home: PathBuf,
    stem: String,
    state: Arc<Mutex<Journal>>,
}

impl JournalHandle {
    /// Starts the journal of a create keyed by `key`.
    pub fn start(home: &Path, key: &CreateKey, kind: &str, spacesd: bool) -> Self {
        let j = Journal {
            name: key.name.clone(),
            token: String::new(),
            pid: std::process::id(),
            spacesd,
            started: now_secs(),
            kind: kind.into(),
            id: key.id.clone(),
            create_id: key.create_id.clone(),
            made: vec![],
            generated: key.generated,
        };
        let h = Self {
            home: home.to_path_buf(),
            stem: key.stem.clone(),
            state: Arc::new(Mutex::new(j)),
        };
        // A marker left from an earlier create under this stem is stale.
        let _ = std::fs::remove_file(marker(home, &key.stem));
        h.save();
        h
    }

    fn save(&self) {
        let j = self.state.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Err(e) = write_file(&path(&self.home, &self.stem), &j) {
            tracing::debug!(create = %self.stem, error = %e, "could not journal the create");
        }
    }

    /// Records something the create made.
    pub fn record(&self, made: Made) {
        {
            let mut j = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if j.made.contains(&made) {
                return;
            }
            j.made.push(made);
        }
        self.save();
    }

    /// Records the Space's token (the recovery registers with it).
    pub fn set_token(&self, token: &str) {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).token = token.into();
        self.save();
    }

    /// What it recorded.
    pub fn snapshot(&self) -> Journal {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The create returned: the journal and any marker go.
    pub fn close(&self) {
        let _ = std::fs::remove_file(path(&self.home, &self.stem));
        let _ = std::fs::remove_file(marker(&self.home, &self.stem));
    }
}

tokio::task_local! {
    static CURRENT: JournalHandle;
}

/// Runs `fut` with `journal` as the create's journal ([`record`]).
pub(crate) async fn scope<F: std::future::Future>(journal: JournalHandle, fut: F) -> F::Output {
    CURRENT.scope(journal, fut).await
}

/// Records `made` in the journal of the create this task runs (a no-op
/// outside one).
pub(crate) fn record(made: Made) {
    let _ = CURRENT.try_with(|j| j.record(made));
}

/// Whether the create this task runs generated its name (`false` outside
/// one).
pub(crate) fn generated_name() -> bool {
    CURRENT
        .try_with(|j| j.state.lock().unwrap_or_else(|e| e.into_inner()).generated)
        .unwrap_or(false)
}

/// Records the token of the create this task runs.
pub(crate) fn set_token(token: &str) {
    let _ = CURRENT.try_with(|j| j.set_token(token));
}

/// One create running in this process.
struct InFlight {
    home: PathBuf,
    key: CreateKey,
    cancel: cua_sandbox_core::CancellationToken,
    /// The outcome once it ended (`None` until then).
    done: tokio::sync::watch::Receiver<Option<CancelOutcome>>,
}

static IN_FLIGHT: Mutex<Vec<InFlight>> = Mutex::new(Vec::new());

/// This create's registration; ends it with [`Flight::finish`].
pub(crate) struct Flight {
    home: PathBuf,
    stem: String,
    pub cancel: cua_sandbox_core::CancellationToken,
    done: tokio::sync::watch::Sender<Option<CancelOutcome>>,
}

/// Registers a create. A create with the same id already running here is
/// refused (two creates of one name would undo each other).
pub(crate) fn register(home: &Path, key: &CreateKey) -> Result<Flight> {
    let mut all = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    if !key.id.is_empty()
        && all
            .iter()
            .any(|f| f.home == home && f.key.id == key.id && f.done.borrow().is_none())
    {
        return Err(Error::invalid(format!(
            "{} is already being created; wait for it or cancel it (cancel_create)",
            key.id
        )));
    }
    all.retain(|f| f.done.borrow().is_none());
    let cancel = cua_sandbox_core::CancellationToken::new();
    let (tx, rx) = tokio::sync::watch::channel(None);
    all.push(InFlight {
        home: home.to_path_buf(),
        key: key.clone(),
        cancel: cancel.clone(),
        done: rx,
    });
    Ok(Flight {
        home: home.to_path_buf(),
        stem: key.stem.clone(),
        cancel,
        done: tx,
    })
}

impl Flight {
    /// Resolves when the create should stop: its token fired, or another
    /// process left a cancel marker (checked a few times a second).
    pub async fn cancelled(&self) {
        let m = marker(&self.home, &self.stem);
        let watch_marker = async {
            loop {
                if m.exists() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        };
        tokio::select! {
            _ = self.cancel.cancelled() => {}
            _ = watch_marker => {}
        }
    }

    /// The create ended with `outcome` (wakes every cancel waiting on it).
    pub fn finish(&self, outcome: CancelOutcome) {
        let _ = self.done.send(Some(outcome));
        let mut all = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|f| f.done.borrow().is_none());
    }
}

/// The running create in this process `key` names, if any: its token and
/// outcome channel.
fn find_running(
    home: &Path,
    key: &str,
) -> Option<(
    CreateKey,
    cua_sandbox_core::CancellationToken,
    tokio::sync::watch::Receiver<Option<CancelOutcome>>,
)> {
    let all = IN_FLIGHT.lock().unwrap_or_else(|e| e.into_inner());
    all.iter()
        .find(|f| f.home == home && f.key.matches(key) && f.done.borrow().is_none())
        .map(|f| (f.key.clone(), f.cancel.clone(), f.done.clone()))
}

/// Every journal under `home`, with its stem.
pub(crate) fn journals(home: &Path) -> Vec<(String, Journal)> {
    let Ok(rd) = std::fs::read_dir(dir(home)) else {
        return vec![];
    };
    let mut out: Vec<(String, Journal)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| {
            let stem = e.path().file_stem()?.to_string_lossy().into_owned();
            let j: Journal = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
            Some((stem, j))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Whether the process that wrote `j` is gone. Where liveness cannot be
/// checked (Windows), a journal older than any create (the 600 s budget,
/// twice) counts.
pub(crate) fn gone(j: &Journal) -> bool {
    if j.pid == std::process::id() {
        return false;
    }
    if cfg!(unix) {
        !cua_host::service::process_alive(j.pid)
    } else {
        now_secs().saturating_sub(j.started) > 1200
    }
}

pub(crate) fn remove(home: &Path, stem: &str) {
    let _ = std::fs::remove_file(path(home, stem));
    let _ = std::fs::remove_file(marker(home, stem));
}

pub(crate) fn cancel_marked(home: &Path, stem: &str) -> bool {
    marker(home, stem).exists()
}

fn journal_matches(j: &Journal, key: &str) -> bool {
    let key = key.trim();
    !key.is_empty()
        && (j.create_id.as_deref() == Some(key)
            || (!j.id.is_empty() && j.id == key)
            || (!j.name.is_empty() && j.name == key))
}

/// How long a cancel waits for another process to finish its clean-up.
const OTHER_PROCESS_WAIT: Duration = Duration::from_secs(150);

impl Spaces {
    /// Cancels a create that is still running: `key` is the caller's
    /// [`crate::SpaceCreate::create_id`], the id the Space will have
    /// (`local:<name>`, as every progress report carries it), or its name.
    ///
    /// The work in flight stops (an image download, a boot, a claim, a
    /// relay registration) and what the create made is removed: its VM or
    /// container and their disks, its cloud claim, its relay machine, its
    /// Space on a host, its journal. Nothing that existed before is
    /// touched. Image layers that finished downloading stay cached, so the
    /// next create resumes. Returns once the clean-up is done.
    ///
    /// Idempotent: a second call, or a call after the create ended,
    /// returns [`CancelState::NotCreating`] (or
    /// [`CancelState::AlreadyCreated`] when the Space now exists). Works
    /// for a create another process runs and for one whose process died
    /// (a daemon restart mid-create).
    pub async fn cancel_create(&self, key: &str) -> Result<CancelOutcome> {
        let home = self.home_dir().to_path_buf();
        if key.trim().is_empty() {
            return Err(Error::invalid(
                "cancel_create needs the create's id or name",
            ));
        }
        if let Some((k, token, mut done)) = find_running(&home, key) {
            token.cancel();
            // The create answers with its own outcome once it cleaned up.
            let outcome = loop {
                if let Some(o) = done.borrow().clone() {
                    break o;
                }
                if done.changed().await.is_err() {
                    break CancelOutcome {
                        id: k.id.clone(),
                        state: CancelState::Cancelled,
                        message: format!("Cancelled {}.", display(&k.id, &k.name)),
                    };
                }
            };
            return Ok(outcome);
        }
        if let Some((stem, j)) = journals(&home)
            .into_iter()
            .find(|(_, j)| journal_matches(j, key))
        {
            if gone(&j) {
                let message = self.undo(&j).await;
                remove(&home, &stem);
                return Ok(CancelOutcome {
                    id: j.id.clone(),
                    state: CancelState::Cancelled,
                    message,
                });
            }
            // Another process runs it: ask, then wait for it to finish.
            std::fs::create_dir_all(dir(&home))?;
            std::fs::write(marker(&home, &stem), b"cancel\n")?;
            let deadline = tokio::time::Instant::now() + OTHER_PROCESS_WAIT;
            while path(&home, &stem).exists() {
                if tokio::time::Instant::now() >= deadline {
                    return Err(Error::Timeout(format!(
                        "asked process {} to cancel {}, and it has not finished cleaning up",
                        j.pid,
                        display(&j.id, &j.name)
                    )));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            return Ok(CancelOutcome {
                id: j.id.clone(),
                state: CancelState::Cancelled,
                message: format!(
                    "Cancelled {} (it was being created by process {}).",
                    display(&j.id, &j.name),
                    j.pid
                ),
            });
        }
        // Not running: finished (listed), or never started / already gone.
        let listed = self
            .resolve(key)
            .ok()
            .map(|id| id.to_string())
            .filter(|id| self.inner.registry.get(id).ok().flatten().is_some());
        Ok(match listed {
            Some(id) => CancelOutcome {
                message: format!("{id} was already created; delete it to remove it."),
                id,
                state: CancelState::AlreadyCreated,
            },
            None => CancelOutcome {
                id: String::new(),
                state: CancelState::NotCreating,
                message: format!("No create of {key} is running."),
            },
        })
    }

    /// Removes what journal `j` records the create made, newest first, and
    /// says what it did. Never fails: what it could not remove is named.
    pub(crate) async fn undo(&self, j: &Journal) -> String {
        let mut done: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();
        let mut local = false;
        for made in j.made.iter().rev() {
            match made {
                Made::LocalSandbox { name, fresh } => {
                    local = true;
                    if !fresh {
                        done.push(format!("left {name} as it was (it existed before)"));
                        continue;
                    }
                    let sbx = &self.inner.sandboxes;
                    let r = match sbx.delete(name).await {
                        Ok(()) => Ok(true),
                        Err(cua_sandbox_core::Error::NotFound(_)) => {
                            sbx.delete_local_instance(name).await
                        }
                        Err(e) => Err(e),
                    };
                    match r {
                        Ok(true) => done.push("removed its VM or container".into()),
                        Ok(false) => {}
                        Err(e) => failed.push(format!("{name}: {e}")),
                    }
                    let _ = self
                        .inner
                        .registry
                        .remove(&crate::SpaceId::Local { name: name.clone() }.to_string());
                }
                Made::FleetClaim { namespace, name } => match self.inner.fleet.as_ref() {
                    Some(f) => match f.release(namespace, name).await {
                        Ok(()) => done.push("released its cloud sandbox".into()),
                        Err(e) => failed.push(format!("cloud:{name}: {e}")),
                    },
                    None => failed.push(format!("cloud:{name}: Fleet is not configured here")),
                },
                Made::HostSpace { host, machine } => {
                    match crate::host_spaces::cancel_on_host(self, host, machine).await {
                        Ok(m) => done.push(m),
                        Err(e) => failed.push(format!("the Space on its host: {e}")),
                    }
                }
                Made::RelayMachine { id } => match self.relay() {
                    Ok(relay) => {
                        let r = async {
                            let token = relay.tokens.access_token().await?;
                            relay.client().await?.delete(&token, id).await
                        }
                        .await;
                        match r {
                            Ok(()) | Err(cua_host::Error::NotFound(_)) => {
                                done.push("removed its relay machine".into())
                            }
                            Err(e) => failed.push(format!("relay machine {id}: {e}")),
                        }
                    }
                    Err(e) => failed.push(format!("relay machine {id}: {e}")),
                },
            }
        }
        if !j.id.is_empty() {
            let _ = self.inner.registry.remove(&j.id);
        }
        let mut msg = format!("Cancelled {}", display(&j.id, &j.name));
        if done.is_empty() {
            msg.push('.');
        } else {
            msg.push_str(&format!("; {}.", done.join(", ")));
        }
        if local {
            msg.push_str(
                " Partial downloads were removed; finished image layers stay in the engine's \
                 cache.",
            );
        }
        if !failed.is_empty() {
            msg.push_str(&format!(" Could not remove: {}.", failed.join("; ")));
        }
        msg
    }
}

fn display<'a>(id: &'a str, name: &'a str) -> &'a str {
    if !id.is_empty() {
        id
    } else if !name.is_empty() {
        name
    } else {
        "the Space"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_old_journal_reads_as_a_local_create() {
        let j: Journal =
            serde_json::from_str(r#"{"name":"a","token":"t","pid":1,"spacesd":true,"started":5}"#)
                .unwrap();
        assert_eq!((j.kind.as_str(), j.made.len()), ("local", 0));
    }

    #[test]
    fn a_journal_records_what_the_create_made_once_each() {
        let home = tempfile::tempdir().unwrap();
        let key = CreateKey {
            id: "local:a".into(),
            name: "a".into(),
            create_id: Some("pending:1".into()),
            stem: "a".into(),
            generated: false,
        };
        let j = JournalHandle::start(home.path(), &key, "local", true);
        let made = Made::LocalSandbox {
            name: "a".into(),
            fresh: true,
        };
        j.record(made.clone());
        j.record(made.clone());
        j.set_token("tok");
        let (stem, read) = journals(home.path()).pop().unwrap();
        assert_eq!(stem, "a");
        assert_eq!(read.made, vec![made]);
        assert_eq!(read.token, "tok");
        assert!(journal_matches(&read, "pending:1"));
        assert!(journal_matches(&read, "local:a"));
        assert!(!journal_matches(&read, "b"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path(home.path(), "a"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "it holds a token");
        }
        j.close();
        assert!(journals(home.path()).is_empty());
    }

    #[test]
    fn two_creates_of_one_id_are_refused() {
        let home = tempfile::tempdir().unwrap();
        let key = CreateKey {
            id: "local:dup".into(),
            name: "dup".into(),
            create_id: None,
            stem: "dup".into(),
            generated: false,
        };
        let first = register(home.path(), &key).unwrap();
        assert!(register(home.path(), &key).is_err());
        first.finish(CancelOutcome {
            id: key.id.clone(),
            state: CancelState::NotCreating,
            message: String::new(),
        });
        register(home.path(), &key).expect("free again once it ended");
    }
}
