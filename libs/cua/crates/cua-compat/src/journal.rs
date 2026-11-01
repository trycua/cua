//! The write-ahead journal of the Spaces a gateway owns, and the backend
//! wrapper that keeps it.
//!
//! A gateway that is killed (SIGKILL, a crash, the host rebooting) between
//! asking Cua to create a Space and recording the sandbox, or whose delete
//! fails because the Cua daemon is down, would leave a Space nothing owns
//! and nothing ends. The journal makes that impossible to forget: the name
//! is appended to `owned.jsonl` *before* the create call and removed only
//! after the delete succeeded (or found nothing). The sweeper deletes every
//! owned Space that no sandbox record claims, at start and while running,
//! and retries until the daemon accepts the delete.
//!
//! Appends are plain `O_APPEND` writes: they survive the process being
//! killed (only a machine crash could lose the last line, and then the
//! Spaces died with it).

use crate::backend::*;
use async_trait::async_trait;
use cua_spacesd_client::SpacesdClient;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Serialize, Deserialize)]
struct Line {
    op: String,
    name: String,
    #[serde(default)]
    daemon: Option<String>,
    #[serde(default)]
    token: Option<String>,
}

/// How long a Space this process made may stay unclaimed by a record
/// before the sweeper judges it: providers record a sandbox after more
/// steps than the create call (and a Daytona snapshot build keeps an
/// unrecorded Space for its whole build). Entries from an earlier process
/// and entries whose create or delete failed are judged at once.
const GRACE: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// A swept name whose delete found nothing is kept this long: the Cua
/// daemon may still be finishing a create the dead gateway had asked for,
/// and that Space appears afterwards.
const CREATE_SETTLE: std::time::Duration = std::time::Duration::from_secs(180);

struct Entry {
    who: Identity,
    /// When this process made it; `None`: an earlier process did.
    born: Option<std::time::Instant>,
    /// A failed create or delete: judge now.
    stale: bool,
}

/// The names of the Spaces this gateway owns, with the identity that made
/// each.
pub struct Journal {
    path: Option<PathBuf>,
    owned: Mutex<BTreeMap<String, Entry>>,
    /// Names whose create call is in flight: not orphans yet.
    creating: Mutex<HashSet<String>>,
    failed: Mutex<HashSet<String>>,
    /// When the sweeper first judged a name unowned.
    judged: Mutex<std::collections::HashMap<String, std::time::Instant>>,
    appended: Mutex<usize>,
}

impl Journal {
    /// Loads `path` (missing: empty; `None`: in memory only).
    pub fn open(path: Option<PathBuf>) -> std::io::Result<Arc<Journal>> {
        let mut owned = BTreeMap::new();
        if let Some(p) = &path
            && let Ok(text) = std::fs::read_to_string(p)
        {
            for l in text.lines() {
                // A torn last line (machine crash) is ignored.
                let Ok(l) = serde_json::from_str::<Line>(l) else {
                    continue;
                };
                match l.op.as_str() {
                    "+" => {
                        owned.insert(
                            l.name,
                            Entry {
                                who: Identity {
                                    daemon: l.daemon,
                                    token: l.token,
                                },
                                born: None,
                                stale: false,
                            },
                        );
                    }
                    "-" => {
                        owned.remove(&l.name);
                    }
                    _ => {}
                }
            }
        }
        let j = Arc::new(Journal {
            path,
            owned: Mutex::new(owned),
            creating: Mutex::default(),
            judged: Mutex::default(),
            failed: Mutex::default(),
            appended: Mutex::new(0),
        });
        j.compact();
        Ok(j)
    }

    fn lines(&self) -> String {
        let owned = self.owned.lock().expect("journal");
        let mut out = String::new();
        for (name, e) in owned.iter() {
            out.push_str(
                &serde_json::to_string(&Line {
                    op: "+".into(),
                    name: name.clone(),
                    daemon: e.who.daemon.clone(),
                    token: e.who.token.clone(),
                })
                .unwrap_or_default(),
            );
            out.push('\n');
        }
        out
    }

    /// Rewrites the file with just the live entries (atomically).
    fn compact(&self) {
        let Some(p) = &self.path else { return };
        let tmp = p.with_extension("jsonl.tmp");
        if crate::tls::write_file(&tmp, self.lines().as_bytes(), 0o600).is_ok() {
            let _ = std::fs::rename(&tmp, p);
        }
        *self.appended.lock().expect("journal") = 0;
    }

    fn append(&self, line: &Line) {
        let Some(p) = &self.path else { return };
        let mut text = serde_json::to_string(line).unwrap_or_default();
        text.push('\n');
        let mut f = {
            let mut o = std::fs::OpenOptions::new();
            o.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            match o.open(p) {
                Ok(f) => f,
                Err(e) => {
                    tracing::warn!("compat: owned-Spaces journal: {e}");
                    return;
                }
            }
        };
        if let Err(e) = f.write_all(text.as_bytes()) {
            tracing::warn!("compat: owned-Spaces journal: {e}");
        }
        let n = {
            let mut a = self.appended.lock().expect("journal");
            *a += 1;
            *a
        };
        if n > 4096 {
            self.compact();
        }
    }

    /// A create of `name` is about to start.
    pub fn begin(&self, name: &str, who: &Identity) {
        self.owned.lock().expect("journal").insert(
            name.to_string(),
            Entry {
                who: who.clone(),
                born: Some(std::time::Instant::now()),
                stale: false,
            },
        );
        self.creating
            .lock()
            .expect("journal")
            .insert(name.to_string());
        self.append(&Line {
            op: "+".into(),
            name: name.into(),
            daemon: who.daemon.clone(),
            token: who.token.clone(),
        });
    }

    /// The create call returned (either way).
    pub fn end_create(&self, name: &str) {
        self.creating.lock().expect("journal").remove(name);
    }

    /// The create or delete of `name` failed: the sweeper judges it now.
    pub fn mark_stale(&self, name: &str) {
        if let Some(e) = self.owned.lock().expect("journal").get_mut(name) {
            e.stale = true;
        }
    }

    /// The Space is gone for good.
    pub fn resolve(&self, name: &str) {
        if self.owned.lock().expect("journal").remove(name).is_some() {
            self.append(&Line {
                op: "-".into(),
                name: name.into(),
                daemon: None,
                token: None,
            });
        }
    }

    /// Owned Spaces that no record claims and no create is making.
    pub fn orphans(&self, recorded: &HashSet<String>) -> Vec<(String, Identity)> {
        let creating = self.creating.lock().expect("journal").clone();
        self.owned
            .lock()
            .expect("journal")
            .iter()
            .filter(|(n, e)| {
                !recorded.contains(*n)
                    && !creating.contains(*n)
                    && (e.stale || e.born.is_none_or(|b| b.elapsed() > GRACE))
            })
            .map(|(n, e)| (n.clone(), e.who.clone()))
            .collect()
    }

    /// Whether `name` has been unowned (and found absent) long enough that
    /// no late create can still produce it. Starts the clock on first use.
    pub fn settled(&self, name: &str) -> bool {
        self.judged
            .lock()
            .expect("journal")
            .entry(name.to_string())
            .or_insert_with(std::time::Instant::now)
            .elapsed()
            > CREATE_SETTLE
    }

    /// True the first time it is asked for `name` (to log a failure once).
    pub fn first_failure(&self, name: &str) -> bool {
        self.failed
            .lock()
            .expect("journal")
            .insert(name.to_string())
    }

    pub fn owned_names(&self) -> Vec<String> {
        self.owned
            .lock()
            .expect("journal")
            .keys()
            .cloned()
            .collect()
    }
}

pub type Recorded = Arc<
    dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = HashSet<String>> + Send>>
        + Send
        + Sync,
>;

/// A [`Backend`] that journals the Spaces it creates and deletes. Wrap the
/// real backend once, in the gateway; the providers need no changes.
///
/// Every `Backend` method is forwarded explicitly: a method left to the
/// trait's default would silently shadow the inner backend's own.
pub struct Journaled {
    inner: Arc<dyn Backend>,
    pub journal: Arc<Journal>,
    /// The names the provider's records claim (`None`: the provider keeps
    /// no records the sweeper can read, so it only retries failed deletes
    /// and failed creates, never judges a live Space).
    recorded: Option<Recorded>,
}

impl Journaled {
    pub fn new(
        inner: Arc<dyn Backend>,
        journal: Arc<Journal>,
        recorded: Option<Recorded>,
    ) -> Arc<Self> {
        Arc::new(Journaled {
            inner,
            journal,
            recorded,
        })
    }

    /// Deletes every owned Space nobody claims. Returns how many it ended.
    pub async fn sweep(&self) -> usize {
        let Some(rec) = &self.recorded else { return 0 };
        let recorded = rec().await;
        let mut ended = 0;
        for (name, who) in self.journal.orphans(&recorded) {
            match self.inner.delete(&who, &name).await {
                Ok(true) => {
                    tracing::info!("compat: ended the unowned Space {name}");
                    self.journal.resolve(&name);
                    ended += 1;
                }
                Ok(false) | Err(BackendError::NotFound(_)) => {
                    // Nothing there (yet): a create the dead gateway asked
                    // for may still be finishing in the daemon.
                    if self.journal.settled(&name) {
                        self.journal.resolve(&name);
                    }
                }
                Err(e) => {
                    // Say it once per name; the retries are quiet.
                    if self.journal.first_failure(&name) {
                        tracing::warn!(
                            "compat: ending the unowned Space {name}: {e} (retrying until it can)"
                        );
                    } else {
                        tracing::debug!("compat: ending the unowned Space {name}: {e}");
                    }
                }
            }
        }
        ended
    }
}

/// Sweeps at start and then every few seconds, forever (spawn it once).
pub async fn sweeper(b: Arc<Journaled>) {
    let mut wait = std::time::Duration::from_secs(2);
    loop {
        let before = b.journal.owned_names().len();
        let ended = b.sweep().await;
        // Back off while nothing can be ended (the daemon is down).
        let pending = b.journal.owned_names().len();
        wait = if ended == 0 && pending == before && pending > 0 {
            (wait * 2).min(std::time::Duration::from_secs(5))
        } else {
            std::time::Duration::from_secs(2)
        };
        tokio::time::sleep(wait).await;
    }
}

#[async_trait]
impl Backend for Journaled {
    fn forks_fast(&self, place: &Placement) -> bool {
        self.inner.forks_fast(place)
    }
    async fn volume_ls(&self, who: &Identity, path: &str) -> Result<Vec<VolumeEntry>> {
        self.inner.volume_ls(who, path).await
    }
    async fn volume_read(&self, who: &Identity, path: &str) -> Result<Vec<u8>> {
        self.inner.volume_read(who, path).await
    }
    async fn volume_write(&self, who: &Identity, path: &str, data: Vec<u8>) -> Result<()> {
        self.inner.volume_write(who, path, data).await
    }
    async fn volume_delete(&self, who: &Identity, path: &str) -> Result<()> {
        self.inner.volume_delete(who, path).await
    }
    async fn create(&self, who: &Identity, spec: CreateSpec) -> Result<Created> {
        self.create_with(who, spec, CreateExtras::default()).await
    }
    async fn create_with(
        &self,
        who: &Identity,
        spec: CreateSpec,
        extras: CreateExtras,
    ) -> Result<Created> {
        let name = spec.name.clone();
        self.journal.begin(&name, who);
        let res = self.inner.create_with(who, spec, extras).await;
        self.journal.end_create(&name);
        if matches!(
            res,
            Err(BackendError::Unsupported(_) | BackendError::Invalid(_))
        ) {
            // Refused before anything was made.
            self.journal.resolve(&name);
        }
        // Any other failure may have left a half-made Space: the sweeper
        // ends it (and the journal keeps the name until it did).
        if res.is_err() {
            self.journal.mark_stale(&name);
        }
        res
    }
    async fn delete(&self, who: &Identity, name: &str) -> Result<bool> {
        let r = self.inner.delete(who, name).await;
        if matches!(r, Ok(_) | Err(BackendError::NotFound(_))) {
            self.journal.resolve(name);
        } else {
            // The record is gone but the Space is not: retry until it is.
            self.journal.mark_stale(name);
        }
        r
    }
    async fn cancel_create(&self, who: &Identity, name: &str) -> Result<bool> {
        self.inner.cancel_create(who, name).await
    }
    async fn network_blocked(&self, who: &Identity, name: &str) -> Result<bool> {
        self.inner.network_blocked(who, name).await
    }
    async fn set_lifetime(
        &self,
        who: &Identity,
        name: &str,
        seconds: u64,
        on_expire: &str,
    ) -> Result<bool> {
        self.inner.set_lifetime(who, name, seconds, on_expire).await
    }
    async fn liveness(&self, who: &Identity, name: &str) -> Result<Liveness> {
        self.inner.liveness(who, name).await
    }
    async fn spacesd(&self, who: &Identity, name: &str) -> Result<SpacesdClient> {
        self.inner.spacesd(who, name).await
    }
    async fn forward(&self, who: &Identity, name: &str, port: u16) -> Result<String> {
        self.inner.forward(who, name, port).await
    }
    async fn suspend(&self, who: &Identity, name: &str) -> Result<()> {
        self.inner.suspend(who, name).await
    }
    async fn resume(&self, who: &Identity, name: &str) -> Result<()> {
        self.inner.resume(who, name).await
    }
    async fn restart(&self, who: &Identity, name: &str) -> Result<()> {
        self.inner.restart(who, name).await
    }
    async fn resize(&self, who: &Identity, name: &str, shape: Shape) -> Result<()> {
        self.inner.resize(who, name, shape).await
    }
    async fn placement_of(&self, who: &Identity, name: &str) -> Result<Option<String>> {
        self.inner.placement_of(who, name).await
    }
    async fn pool_create(
        &self,
        who: &Identity,
        name: &str,
        image: &str,
        min: u32,
        place: &Placement,
    ) -> Result<()> {
        self.inner.pool_create(who, name, image, min, place).await
    }
    async fn pool_ready(&self, who: &Identity, name: &str) -> Result<Option<u32>> {
        self.inner.pool_ready(who, name).await
    }
    async fn pool_delete(&self, who: &Identity, name: &str) -> Result<bool> {
        self.inner.pool_delete(who, name).await
    }
    async fn check_image(&self, who: &Identity, image: &str) -> Result<()> {
        self.inner.check_image(who, image).await
    }
    async fn checkpoint(&self, who: &Identity, name: &str, checkpoint: &str) -> Result<()> {
        self.inner.checkpoint(who, name, checkpoint).await
    }
    async fn checkpoints(&self, who: &Identity) -> Result<Vec<CheckpointInfo>> {
        self.inner.checkpoints(who).await
    }
    async fn delete_checkpoint(&self, who: &Identity, checkpoint: &str) -> Result<bool> {
        self.inner.delete_checkpoint(who, checkpoint).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::mock::MockBackend;

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            name: name.into(),
            source: Source::Image(None),
            env: Default::default(),
            internet: true,
            place: Default::default(),
        }
    }

    /// A method left to the trait's default would silently shadow the inner
    /// backend's own (this once hid `forks_fast`): every method of the
    /// trait must be written out in the wrapper.
    #[test]
    fn the_wrapper_forwards_every_backend_method() {
        let names = |src: &str, start: &str| -> std::collections::BTreeSet<String> {
            let body = src.split(start).nth(1).expect("block");
            let body = &body[..body.find("\n}\n").expect("end")];
            body.lines()
                .filter_map(|l| {
                    let l = l.strip_prefix("    ")?;
                    let l = l.strip_prefix("async ").unwrap_or(l);
                    let l = l.strip_prefix("fn ")?;
                    Some(l.split(['(', '<']).next()?.to_string())
                })
                .collect()
        };
        let backend = names(
            include_str!("backend.rs"),
            "pub trait Backend: Send + Sync + 'static {",
        );
        let wrapper = names(include_str!("journal.rs"), "impl Backend for Journaled {");
        assert!(backend.len() > 15, "{backend:?}");
        let missing: Vec<_> = backend.difference(&wrapper).collect();
        assert!(missing.is_empty(), "Journaled does not forward {missing:?}");
    }

    #[tokio::test]
    async fn the_journal_survives_a_restart_and_forgets_deleted_spaces() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("owned.jsonl");
        let j = Journal::open(Some(p.clone())).unwrap();
        let who = Identity {
            daemon: Some("d".into()),
            token: Some("t".into()),
        };
        j.begin("a", &who);
        j.begin("b", &who);
        j.resolve("a");
        let again = Journal::open(Some(p)).unwrap();
        assert_eq!(again.owned_names(), ["b"]);
        let o = again.orphans(&HashSet::new());
        assert_eq!(o[0].1, who);
    }

    #[tokio::test]
    async fn an_unclaimed_space_is_swept_and_a_claimed_one_is_kept() {
        let mock = Arc::new(MockBackend::new());
        let j = Journal::open(None).unwrap();
        let claimed: Arc<Mutex<HashSet<String>>> = Arc::default();
        let c2 = claimed.clone();
        let b = Journaled::new(
            mock.clone(),
            j.clone(),
            Some(Arc::new(move || {
                let c = c2.lock().unwrap().clone();
                Box::pin(async move { c })
            })),
        );
        let who = Identity::default();
        b.create(&who, spec("kept")).await.unwrap();
        b.create(&who, spec("lost")).await.unwrap();
        // A Space this process made is judged after the grace period, or
        // when its record's delete failed.
        assert_eq!(b.sweep().await, 0);
        j.mark_stale("lost");
        claimed.lock().unwrap().insert("kept".into());
        assert_eq!(b.sweep().await, 1);
        assert_eq!(mock.names().await, ["kept"]);
        // Deleting through the wrapper resolves the entry.
        b.delete(&who, "kept").await.unwrap();
        assert!(j.owned_names().is_empty());
    }

    #[tokio::test]
    async fn a_create_in_flight_is_not_an_orphan() {
        let mock = Arc::new(MockBackend::new());
        mock.create_delay_ms
            .store(300, std::sync::atomic::Ordering::Relaxed);
        let j = Journal::open(None).unwrap();
        let b = Journaled::new(
            mock.clone(),
            j,
            Some(Arc::new(|| Box::pin(async { HashSet::new() }))),
        );
        let b2 = b.clone();
        let t = tokio::spawn(async move { b2.create(&Identity::default(), spec("x")).await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        b.journal.mark_stale("x");
        assert_eq!(b.sweep().await, 0, "the create is still running");
        t.await.unwrap().unwrap();
        b.journal.mark_stale("x");
        assert_eq!(b.sweep().await, 1, "now nobody claims it");
    }
}
