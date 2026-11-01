//! The gateway's records of the sandboxes it made, persisted so lifetimes
//! and access tokens survive a gateway restart, and the reaper that ends a
//! sandbox when its lifetime runs out (E2B's `timeout`, Daytona's
//! auto-stop): delete it, or suspend it when the record says so.

use crate::backend::{Backend, Identity, Liveness};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{Mutex, Notify};

/// What happens when a sandbox's lifetime runs out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnTimeout {
    #[default]
    Kill,
    Pause,
}

/// One sandbox the gateway made.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    /// The provider-facing id.
    pub id: String,
    /// The Cua sandbox name.
    pub cua_name: String,
    /// Id of the API key that made it (only that key sees it).
    pub key_id: String,
    #[serde(default)]
    pub identity_daemon: Option<String>,
    #[serde(default)]
    pub identity_token: Option<String>,
    /// The template / snapshot it started from, as the client named it.
    pub template_id: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    /// Default environment of every process.
    #[serde(default)]
    pub env_vars: BTreeMap<String, String>,
    /// envd / toolbox access token.
    pub access_token: String,
    /// Required on get_host traffic when public traffic is off.
    #[serde(default)]
    pub traffic_token: Option<String>,
    pub started_at_ms: u64,
    /// Lifetime deadline (unix ms); `None`: no deadline.
    #[serde(default)]
    pub end_at_ms: Option<u64>,
    #[serde(default)]
    pub on_timeout: OnTimeout,
    /// After a pause at the deadline, delete this long later (Daytona's
    /// auto-delete after auto-stop); `None`: keep it.
    #[serde(default)]
    pub delete_after_pause_ms: Option<u64>,
    /// An absolute end regardless of activity (Daytona's `ttlMinutes`).
    #[serde(default)]
    pub hard_end_at_ms: Option<u64>,
    /// Resume a paused sandbox on the next request to it.
    #[serde(default)]
    pub auto_resume: bool,
    #[serde(default)]
    pub paused: bool,
    #[serde(default = "yes")]
    pub internet: bool,
    pub cpus: u32,
    pub memory_mb: u64,
    pub disk_mb: u64,
    /// Checkpoint the gateway made for a fork of this sandbox's source, and
    /// holds until the last sandbox started from it is gone.
    #[serde(default)]
    pub held_checkpoint: Option<String>,
    /// Provider extras (Daytona labels, names, ...).
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn yes() -> bool {
    true
}

impl Record {
    pub fn identity(&self) -> Identity {
        Identity {
            daemon: self.identity_daemon.clone(),
            token: self.identity_token.clone(),
        }
    }
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Default, Serialize, Deserialize)]
struct Persisted {
    records: Vec<Record>,
    /// Gateway-made checkpoints and how many records hold each.
    #[serde(default)]
    held: BTreeMap<String, u32>,
    /// Snapshots made without a name (their id is all they have).
    #[serde(default)]
    unnamed: std::collections::BTreeSet<String>,
    /// Snapshots taken without memory (`create_snapshot(mode="filesystem")`):
    /// sandboxes made from them boot from the disk state.
    #[serde(default)]
    fs_only: std::collections::BTreeSet<String>,
}

/// All records of one provider, persisted to `path` (0600).
pub struct Store {
    path: Option<PathBuf>,
    inner: Mutex<(HashMap<String, Record>, BTreeMap<String, u32>)>,
    unnamed: Mutex<std::collections::BTreeSet<String>>,
    fs_only: Mutex<std::collections::BTreeSet<String>>,
    /// Woken when a deadline changes: the reaper's (one waiter; a second
    /// waiter on the same `Notify` would steal its wake-up, so each
    /// consumer has its own).
    pub changed: Notify,
    /// The same, for the lifetime syncer ([`crate::lifetime::sync`]).
    pub synced: Notify,
    /// Woken when the state changed and should be written.
    dirty: Arc<Notify>,
}

/// How long the writer waits to coalesce a burst of changes into one write.
const WRITE_BEHIND: Duration = Duration::from_millis(50);

impl Store {
    /// Loads `path` (missing: empty). `None`: in memory only (tests).
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Arc<Store>> {
        let persisted: Persisted = match &path {
            Some(p) => match std::fs::read(p) {
                Ok(b) => serde_json::from_slice(&b).unwrap_or_default(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Persisted::default(),
                Err(e) => return Err(e),
            },
            None => Persisted::default(),
        };
        let map = persisted
            .records
            .into_iter()
            .map(|r| (r.id.clone(), r))
            .collect();
        let store = Arc::new(Store {
            path,
            inner: Mutex::new((map, persisted.held)),
            unnamed: Mutex::new(persisted.unnamed),
            fs_only: Mutex::new(persisted.fs_only),
            changed: Notify::new(),
            synced: Notify::new(),
            dirty: Arc::new(Notify::new()),
        });
        // Writes are coalesced off the request path: with hundreds of
        // sandboxes a full rewrite per update would serialise everything.
        if store.path.is_some() && tokio::runtime::Handle::try_current().is_ok() {
            let weak = Arc::downgrade(&store);
            let dirty = store.dirty.clone();
            tokio::spawn(async move {
                loop {
                    dirty.notified().await;
                    tokio::time::sleep(WRITE_BEHIND).await;
                    let Some(s) = weak.upgrade() else { return };
                    s.flush().await;
                }
            });
        }
        Ok(store)
    }

    /// Marks the state as changed; the writer persists it shortly.
    fn save(&self) {
        self.dirty.notify_one();
    }

    /// Writes the current state now (the writer calls this; so do shutdown
    /// paths and tests that reopen the file).
    pub async fn flush(&self) {
        let Some(p) = self.path.clone() else { return };
        let (mut records, held) = {
            let g = self.inner.lock().await;
            (g.0.values().cloned().collect::<Vec<Record>>(), g.1.clone())
        };
        let unnamed = self.unnamed.lock().await.clone();
        let fs_only = self.fs_only.lock().await.clone();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let written = tokio::task::spawn_blocking(move || {
            let body = serde_json::to_vec_pretty(&Persisted {
                records,
                held,
                unnamed,
                fs_only,
            })
            .expect("state");
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            crate::tls::write_file(&p, &body, 0o600).map(|()| p)
        })
        .await;
        match written {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => tracing::warn!("compat: saving the state: {e}"),
            Err(e) => tracing::warn!("compat: saving the state: {e}"),
        }
    }

    pub async fn insert(&self, r: Record) {
        let mut g = self.inner.lock().await;
        if let Some(cp) = &r.held_checkpoint {
            *g.1.entry(cp.clone()).or_default() += 1;
        }
        g.0.insert(r.id.clone(), r);
        drop(g);
        self.save();
        self.changed.notify_one();
        self.synced.notify_one();
    }

    pub async fn get(&self, id: &str) -> Option<Record> {
        self.inner.lock().await.0.get(id).cloned()
    }

    pub async fn all(&self) -> Vec<Record> {
        let mut v: Vec<Record> = self.inner.lock().await.0.values().cloned().collect();
        v.sort_by_key(|r| (r.started_at_ms, r.id.clone()));
        v
    }

    /// Updates a record in place; `None` when it does not exist.
    pub async fn update<T>(&self, id: &str, f: impl FnOnce(&mut Record) -> T) -> Option<T> {
        let mut g = self.inner.lock().await;
        let out = g.0.get_mut(id).map(f)?;
        drop(g);
        self.save();
        self.changed.notify_one();
        self.synced.notify_one();
        Some(out)
    }

    /// Removes a record; returns it and the held checkpoint that is now
    /// unused (to delete), if any.
    pub async fn remove(&self, id: &str) -> Option<(Record, Option<String>)> {
        let mut g = self.inner.lock().await;
        let r = g.0.remove(id)?;
        let mut release = None;
        if let Some(cp) = &r.held_checkpoint {
            let left = g.1.get_mut(cp).map(|n| {
                *n = n.saturating_sub(1);
                *n
            });
            if left == Some(0) {
                g.1.remove(cp);
                release = Some(cp.clone());
            }
        }
        drop(g);
        self.save();
        Some((r, release))
    }

    /// Remembers that snapshot `id` was made without a name.
    pub async fn mark_unnamed(&self, id: &str) {
        self.unnamed.lock().await.insert(id.to_string());
        self.save();
    }

    pub async fn mark_fs_only(&self, id: &str) {
        self.fs_only.lock().await.insert(id.to_string());
        self.save();
    }

    pub async fn is_fs_only(&self, id: &str) -> bool {
        self.fs_only.lock().await.contains(id)
    }

    pub async fn is_unnamed(&self, id: &str) -> bool {
        self.unnamed.lock().await.contains(id)
    }

    pub async fn forget_snapshot(&self, id: &str) {
        self.unnamed.lock().await.remove(id);
        self.fs_only.lock().await.remove(id);
        self.save();
    }

    /// Whether `checkpoint` is one the gateway holds for forks.
    pub async fn is_held(&self, checkpoint: &str) -> bool {
        self.inner.lock().await.1.contains_key(checkpoint)
    }

    /// The earliest deadline of a running record, leaving out `skip` (the
    /// ones being ended right now).
    async fn next_deadline(
        &self,
        skip: &std::collections::HashSet<String>,
    ) -> Option<(u64, String)> {
        self.inner
            .lock()
            .await
            .0
            .values()
            .filter(|r| !skip.contains(&r.id))
            .flat_map(|r| {
                let soft = (!r.paused || r.on_timeout == OnTimeout::Kill)
                    .then_some(r.end_at_ms)
                    .flatten();
                [soft, r.hard_end_at_ms]
                    .into_iter()
                    .flatten()
                    .map(|t| (t, r.id.clone()))
                    .collect::<Vec<_>>()
            })
            .min()
    }
}

/// Ends sandboxes whose lifetime ran out, forever (spawn it once). Each is
/// ended in its own task: deleting a VM can take many seconds and must not
/// hold up the ones behind it.
pub async fn reaper(store: Arc<Store>, backend: Arc<dyn Backend>) {
    let inflight: Arc<std::sync::Mutex<std::collections::HashSet<String>>> = Default::default();
    loop {
        let skip = inflight.lock().expect("inflight").clone();
        let next = store.next_deadline(&skip).await;
        let now = now_ms();
        match next {
            Some((at, id)) if at <= now => {
                inflight.lock().expect("inflight").insert(id.clone());
                let (store, backend, inflight) = (store.clone(), backend.clone(), inflight.clone());
                tokio::spawn(async move {
                    expire(&store, backend.as_ref(), &id).await;
                    inflight.lock().expect("inflight").remove(&id);
                    store.changed.notify_one();
                });
            }
            Some((at, _)) => {
                let wait = Duration::from_millis((at - now).min(60_000));
                let _ = tokio::time::timeout(wait, store.changed.notified()).await;
            }
            None => {
                let _ =
                    tokio::time::timeout(Duration::from_secs(60), store.changed.notified()).await;
            }
        }
    }
}

/// Ends one sandbox now, as its lifetime policy says.
pub async fn expire(store: &Store, backend: &dyn Backend, id: &str) {
    let Some(r) = store.get(id).await else { return };
    if r.hard_end_at_ms.is_some_and(|t| t <= now_ms()) {
        kill(store, backend, id).await;
        return;
    }
    if r.paused {
        // Paused with a delete scheduled (auto-delete after auto-stop).
        kill(store, backend, id).await;
        return;
    }
    match r.on_timeout {
        OnTimeout::Pause => match backend.suspend(&r.identity(), &r.cua_name).await {
            Ok(()) => {
                store
                    .update(id, |r| {
                        r.paused = true;
                        // Auto-pause without memory (`lifecycle` with a
                        // filesystem-only pause) comes back from the disk.
                        if r.extra
                            .get("auto_pause_memory")
                            .and_then(serde_json::Value::as_bool)
                            == Some(false)
                        {
                            r.extra
                                .insert("reboot_on_resume".into(), serde_json::Value::Bool(true));
                        }
                        match r.delete_after_pause_ms {
                            Some(d) => {
                                r.end_at_ms = Some(now_ms() + d);
                                r.on_timeout = OnTimeout::Kill;
                            }
                            None => r.end_at_ms = None,
                        }
                    })
                    .await;
            }
            Err(e) => {
                tracing::warn!("compat: pausing {id} at its timeout failed ({e}); deleting it");
                kill(store, backend, id).await;
            }
        },
        OnTimeout::Kill => {
            kill(store, backend, id).await;
        }
    }
}

/// Deletes a sandbox and its record (and a held checkpoint nobody uses any
/// more). Returns whether the sandbox existed.
pub async fn kill(store: &Store, backend: &dyn Backend, id: &str) -> bool {
    let Some((r, release)) = store.remove(id).await else {
        return false;
    };
    let who = r.identity();
    let existed = match backend.delete(&who, &r.cua_name).await {
        Ok(existed) => existed,
        Err(e) => {
            tracing::warn!("compat: deleting {} ({}): {e}", r.id, r.cua_name);
            true
        }
    };
    if let Some(cp) = release
        && let Err(e) = backend.delete_checkpoint(&who, &cp).await
    {
        tracing::warn!("compat: deleting held checkpoint {cp}: {e}");
    }
    existed
}

/// Drops records whose sandbox no longer exists (deleted outside the
/// gateway) — run at start.
pub async fn reconcile(store: &Store, backend: &dyn Backend) {
    for r in store.all().await {
        if let Ok(Liveness::Gone) = backend.liveness(&r.identity(), &r.cua_name).await {
            tracing::info!(
                "compat: {} ({}) is gone; dropping its record",
                r.id,
                r.cua_name
            );
            if let Some((r, Some(cp))) = store.remove(&r.id).await {
                let _ = backend.delete_checkpoint(&r.identity(), &cp).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{CreateSpec, Source, mock::MockBackend};

    fn rec(id: &str, cua: &str, end: Option<u64>) -> Record {
        Record {
            id: id.into(),
            cua_name: cua.into(),
            key_id: "k".into(),
            identity_daemon: None,
            identity_token: None,
            template_id: "base".into(),
            alias: None,
            metadata: BTreeMap::new(),
            env_vars: BTreeMap::new(),
            access_token: "t".into(),
            traffic_token: None,
            started_at_ms: now_ms(),
            end_at_ms: end,
            on_timeout: OnTimeout::Kill,
            delete_after_pause_ms: None,
            hard_end_at_ms: None,
            auto_resume: false,
            paused: false,
            internet: true,
            cpus: 2,
            memory_mb: 2048,
            disk_mb: 0,
            held_checkpoint: None,
            extra: Default::default(),
        }
    }

    async fn made(b: &MockBackend, name: &str) {
        b.create(
            &Identity::default(),
            CreateSpec {
                name: name.into(),
                source: Source::Image(None),
                env: HashMap::new(),
                internet: true,
                place: Default::default(),
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn reaper_kills_at_the_deadline_and_pauses_when_asked() {
        let backend = Arc::new(MockBackend::new());
        made(&backend, "a").await;
        made(&backend, "b").await;
        let store = Store::open(None).unwrap();
        store.insert(rec("a", "a", Some(now_ms() + 150))).await;
        let mut b = rec("b", "b", Some(now_ms() + 150));
        b.on_timeout = OnTimeout::Pause;
        store.insert(b).await;
        tokio::spawn(reaper(store.clone(), backend.clone()));
        tokio::time::sleep(Duration::from_millis(600)).await;
        assert!(store.get("a").await.is_none());
        let who = Identity::default();
        assert_eq!(backend.liveness(&who, "a").await.unwrap(), Liveness::Gone);
        let b = store.get("b").await.unwrap();
        assert!(b.paused && b.end_at_ms.is_none());
        assert_eq!(
            backend.liveness(&who, "b").await.unwrap(),
            Liveness::Suspended
        );
    }

    /// The lifetime syncer and the reaper both wait for deadline changes. They
    /// once shared one `Notify`, whose `notify_one` wakes a single waiter (the
    /// first to have queued): with the syncer queued first, it took the
    /// wake-up for a new short deadline and the reaper slept on its far one
    /// (up to 60 s). This builds exactly that order.
    #[tokio::test]
    async fn the_reaper_wakes_for_a_new_deadline_even_when_the_syncer_waited_first() {
        let backend = Arc::new(MockBackend::new());
        let store = Store::open(None).unwrap();
        made(&backend, "far").await;
        store
            .insert(rec("far", "far", Some(now_ms() + 600_000)))
            .await;
        // The syncer queues on the store's wake-up first, then the reaper.
        tokio::spawn(crate::lifetime::sync(store.clone(), backend.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        tokio::spawn(reaper(store.clone(), backend.clone()));
        tokio::time::sleep(Duration::from_millis(100)).await;
        // One new, short deadline: a single wake-up.
        made(&backend, "soon").await;
        store
            .insert(rec("soon", "soon", Some(now_ms() + 200)))
            .await;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            store.get("soon").await.is_none(),
            "the reaper missed the new deadline"
        );
        assert!(store.get("far").await.is_some());
    }

    #[tokio::test]
    async fn state_survives_a_restart_with_0600() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let store = Store::open(Some(p.clone())).unwrap();
        let mut r = rec("x", "cx", Some(42));
        r.held_checkpoint = Some("cp".into());
        store.insert(r).await;
        store.flush().await;
        let again = Store::open(Some(p.clone())).unwrap();
        assert_eq!(again.get("x").await.unwrap().end_at_ms, Some(42));
        assert!(again.is_held("cp").await);
        let (_, release) = again.remove("x").await.unwrap();
        assert_eq!(release.as_deref(), Some("cp"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[tokio::test]
    async fn held_checkpoint_is_released_by_the_last_holder() {
        let store = Store::open(None).unwrap();
        for id in ["f1", "f2"] {
            let mut r = rec(id, id, None);
            r.held_checkpoint = Some("cp".into());
            store.insert(r).await;
        }
        assert_eq!(store.remove("f1").await.unwrap().1, None);
        assert_eq!(store.remove("f2").await.unwrap().1.as_deref(), Some("cp"));
    }
}
