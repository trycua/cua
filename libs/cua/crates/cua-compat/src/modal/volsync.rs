//! Volume mounts as Modal v1 volumes behave: a container sees a copy of the
//! volume, its changes are committed back in the background and when it
//! ends, and `reload` fetches what others committed.
//!
//! Cua Volume is guarded per Space (a Space reaches `public/` and its own
//! `spaces/<name>/`, nothing else without a grant a person approves), so the
//! gateway does not bind a folder of it into the guest. It copies the volume
//! in at mount time (one tar through cua-spacesd), pushes changed files back
//! every few seconds and once more when the container ends, and `reload`
//! pulls files committed since.

use crate::backend::{Backend, BackendError, Identity, VolumeEntry};
use crate::modal::volumes::MARKER;
use cua_spacesd_client::SpacesdClient;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// How often changes in a mount are pushed.
const PUSH_EVERY: Duration = Duration::from_secs(3);

/// What a file was when last in sync: size and modification time (ms).
type Known = BTreeMap<String, (u64, i64)>;

struct Mount {
    /// Where it is mounted in the guest.
    guest: String,
    /// The host-side folder (`modal-volumes/<store>[/sub]`).
    folder: String,
    read_only: bool,
    /// Guest state at the last sync.
    known: Known,
    /// Host state at the last pull: path -> (size, modified ms).
    pulled: BTreeMap<String, (u64, u64)>,
}

pub struct VolSync {
    who: Identity,
    backend: Arc<dyn Backend>,
    client: SpacesdClient,
    driver: String,
    mounts: Mutex<Vec<Mount>>,
    task: std::sync::Mutex<Option<tokio::task::AbortHandle>>,
}

/// Files under `prefix` on the host: relative path, size, modified (ms).
async fn host_files(
    backend: &Arc<dyn Backend>,
    who: &Identity,
    prefix: &str,
) -> Result<Vec<(String, u64, u64)>, BackendError> {
    let mut out = Vec::new();
    let mut queue = vec![prefix.to_string()];
    while let Some(dir) = queue.pop() {
        let entries: Vec<VolumeEntry> = backend.volume_ls(who, &dir).await?;
        for e in entries {
            let rel = e
                .path
                .strip_prefix(&format!("{prefix}/"))
                .unwrap_or(&e.path)
                .trim_end_matches('/')
                .to_string();
            if e.folder {
                queue.push(e.path.trim_end_matches('/').to_string());
            } else if e.name != MARKER {
                out.push((rel, e.size, e.modified_ms));
            }
        }
    }
    Ok(out)
}

impl VolSync {
    pub fn new(
        who: Identity,
        backend: Arc<dyn Backend>,
        client: SpacesdClient,
        driver: String,
    ) -> Arc<VolSync> {
        Arc::new(VolSync {
            who,
            backend,
            client,
            driver,
            mounts: Mutex::new(Vec::new()),
            task: std::sync::Mutex::new(None),
        })
    }

    async fn sh(&self, script: &str, args: &[&str]) -> Result<String, String> {
        let out = crate::guest::run_as(&self.client, &self.driver, "root", script, args).await?;
        if out.success() {
            Ok(out.stdout_str())
        } else {
            Err(out.stderr_str().trim().to_string())
        }
    }

    /// The guest's files under `dir`: relative path -> (size, mtime ms).
    async fn guest_files(&self, dir: &str) -> Result<Known, String> {
        let out = self
            .sh(
                "find \"$1\" -type f -printf '%P\\t%s\\t%T@\\n' 2>/dev/null || true",
                &[dir],
            )
            .await?;
        let mut k = Known::new();
        for l in out.lines() {
            let mut p = l.split('\t');
            if let (Some(path), Some(size), Some(t)) = (p.next(), p.next(), p.next())
                && let (Ok(size), Ok(t)) = (size.parse::<u64>(), t.parse::<f64>())
            {
                k.insert(path.to_string(), (size, (t * 1000.0) as i64));
            }
        }
        Ok(k)
    }

    /// Copies host files (`paths`, relative to the mount's folder) into the
    /// guest mount in one tar.
    async fn copy_in(&self, m: &Mount, paths: &[String]) -> Result<(), String> {
        if paths.is_empty() {
            return Ok(());
        }
        let mut tar = tar::Builder::new(Vec::new());
        for rel in paths {
            let data = self
                .backend
                .volume_read(&self.who, &format!("{}/{rel}", m.folder))
                .await
                .map_err(|e| format!("reading {rel}: {e}"))?;
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_mtime(0);
            tar.append_data(&mut h, rel, data.as_slice())
                .map_err(|e| e.to_string())?;
        }
        let bytes = tar.into_inner().map_err(|e| e.to_string())?;
        let tmp = format!("/tmp/.cua-vol-{:016x}.tar", rand::random::<u64>());
        let mut w = crate::guestfs::Writer::begin(&self.client, &tmp, "root", &self.driver)
            .await
            .map_err(|e| e.1)?;
        w.write(bytes.into()).await.map_err(|e| e.1)?;
        w.finish().await.map_err(|e| e.1)?;
        self.sh(
            "set -e; mkdir -p -- \"$2\"; tar -xf \"$1\" -C \"$2\"; rm -f -- \"$1\"",
            &[&tmp, &m.guest],
        )
        .await
        .map(|_| ())
    }

    /// Mounts a volume folder at `guest` (populated from the host).
    pub async fn add(&self, guest: &str, folder: &str, read_only: bool) -> Result<(), String> {
        let files = host_files(&self.backend, &self.who, folder)
            .await
            .map_err(|e| e.to_string())?;
        let mut m = Mount {
            guest: guest.to_string(),
            folder: folder.to_string(),
            read_only,
            known: Known::new(),
            pulled: files
                .iter()
                .map(|(p, s, t)| (p.clone(), (*s, *t)))
                .collect(),
        };
        self.sh("mkdir -p -- \"$1\"", &[guest]).await?;
        let paths: Vec<String> = files.into_iter().map(|f| f.0).collect();
        self.copy_in(&m, &paths).await?;
        m.known = self.guest_files(guest).await?;
        if read_only {
            // A bind mount where the Space can make one; otherwise the files
            // are made read-only (code running as root can still change them,
            // but nothing is pushed back).
            let _ = self
                .sh(
                    "mount --bind -- \"$1\" \"$1\" 2>/dev/null && mount -o remount,ro,bind -- \"$1\" 2>/dev/null || chmod -R a-w -- \"$1\"",
                    &[guest],
                )
                .await;
        }
        self.mounts.lock().await.push(m);
        Ok(())
    }

    /// Pushes what changed in the writable mounts to the host.
    pub async fn push(&self) {
        let mut mounts = self.mounts.lock().await;
        for m in mounts.iter_mut().filter(|m| !m.read_only) {
            let Ok(now) = self.guest_files(&m.guest).await else {
                continue;
            };
            for (rel, state) in &now {
                if m.known.get(rel) == Some(state) {
                    continue;
                }
                let Ok(data) = self.client.download(&format!("{}/{rel}", m.guest)).await else {
                    continue;
                };
                let key = format!("{}/{rel}", m.folder);
                if self
                    .backend
                    .volume_write(&self.who, &key, data.to_vec())
                    .await
                    .is_ok()
                {
                    m.pulled.insert(rel.clone(), (state.0, u64::MAX));
                    m.known.insert(rel.clone(), *state);
                }
            }
            let gone: Vec<String> = m
                .known
                .keys()
                .filter(|k| !now.contains_key(*k))
                .cloned()
                .collect();
            for rel in gone {
                let _ = self
                    .backend
                    .volume_delete(&self.who, &format!("{}/{rel}", m.folder))
                    .await;
                m.known.remove(&rel);
                m.pulled.remove(&rel);
            }
        }
    }

    /// Pulls what others committed since the last pull (and pushes first,
    /// so the guest's own changes are not overwritten).
    pub async fn reload(&self) -> Result<(), String> {
        self.push().await;
        let mut mounts = self.mounts.lock().await;
        for m in mounts.iter_mut() {
            let files = host_files(&self.backend, &self.who, &m.folder)
                .await
                .map_err(|e| e.to_string())?;
            let mut fresh = Vec::new();
            for (rel, size, modified) in &files {
                match m.pulled.get(rel) {
                    Some((s, t)) if s == size && (*t == u64::MAX || t == modified) => {}
                    _ => fresh.push(rel.clone()),
                }
            }
            // The mount may be read-only: write through a remount.
            if !fresh.is_empty() {
                if m.read_only {
                    let _ = self
                        .sh("mount -o remount,rw,bind -- \"$1\"", &[&m.guest])
                        .await;
                }
                let r = self.copy_in(m, &fresh).await;
                if m.read_only {
                    let _ = self
                        .sh("mount -o remount,ro,bind -- \"$1\"", &[&m.guest])
                        .await;
                }
                r?;
            }
            let present: std::collections::BTreeSet<&String> = files.iter().map(|f| &f.0).collect();
            let removed: Vec<String> = m
                .pulled
                .keys()
                .filter(|k| !present.contains(k))
                .cloned()
                .collect();
            for rel in &removed {
                let _ = self.sh("rm -f -- \"$1/$2\"", &[&m.guest, rel]).await;
            }
            m.pulled = files.into_iter().map(|(p, s, t)| (p, (s, t))).collect();
            m.known = self.guest_files(&m.guest).await.unwrap_or_default();
        }
        Ok(())
    }

    /// Pushes in the background until [`VolSync::stop`].
    pub fn start(self: &Arc<Self>) {
        let me = Arc::downgrade(self);
        let t = tokio::spawn(async move {
            loop {
                tokio::time::sleep(PUSH_EVERY).await;
                let Some(s) = me.upgrade() else { return };
                s.push().await;
            }
        });
        *self.task.lock().expect("volsync task") = Some(t.abort_handle());
    }

    /// The final push, then no more.
    pub async fn stop(&self) {
        if let Some(t) = self.task.lock().expect("volsync task").take() {
            t.abort();
        }
        self.push().await;
    }
}
