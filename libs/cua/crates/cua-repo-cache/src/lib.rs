//! The repo cache: public git repositories mirrored on this machine's disk by
//! the cua daemon, so a repeat `git clone <url>` inside a Space does not
//! download the repository again. The agent's command never changes.
//!
//! How it works:
//!
//! - The daemon sees a `git clone <https url>` (a command it runs or relays,
//!   or one reported by a Space's helper), asks the remote for its refs with
//!   no credentials at all, and, when that works, keeps a bare **mirror** of
//!   the repository under `$CUA_HOME/cache/repos`: filled by one normal clone,
//!   verified (`git fsck` connectivity and each pack's trailer), then
//!   refreshed in the background. Repositories that need a credential, hosts
//!   on a private network, and anything else that is not plainly public are
//!   never cached.
//! - The directory is mounted **read-only** into container Spaces at
//!   [`GUEST_ROOT`] (the same host-directory mount a Space gets for any
//!   mount). A system git config in it rewrites a mirrored repository's URL to
//!   a small remote helper ([`shim::HELPER`]) that the daemon installs next
//!   to the Space's git. The helper makes the clone's objects come from the
//!   mirror and asks the remote itself for the tip, so a clone is never
//!   older than the remote; it copies the mirror's packs into the clone, so
//!   the clone never depends on the mirror; and any failure runs the plain
//!   `git remote-https` instead.
//! - The cache is bounded ([`Config::cap_bytes`], default the smaller of 5 GB
//!   and a tenth of the free disk), evicts the least recently used mirror
//!   first, can be turned off and cleared.
//!
//! Counters only ([`Status`], and `cua_repo_cache` telemetry with fixed
//! words): never a URL, a repository name or a path.

mod fill;
mod git;
pub mod shim;
mod stats;
mod store;
#[cfg(test)]
mod tests;
pub mod url;

pub use fill::{Outcome, Reason};
pub use store::{Entry, Pack};
pub use url::RepoUrl;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// Where a Space sees the cache.
pub const GUEST_ROOT: &str = "/cua/repo-cache";

/// The size cap when nothing sets one, at most (it is also at most a tenth
/// of the free disk, see [`RepoCache::cap_bytes`]).
pub const DEFAULT_CAP_BYTES: u64 = 5 << 30;

/// How long a mirror stays untouched by automatic eviction after it was
/// used: a clone that has just started may still be reading it.
pub const EVICTION_GRACE: Duration = Duration::from_secs(120);

/// The settings of the cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// The cache is on (default).
    pub enabled: bool,
    /// The total size it may use; `None`: the default
    /// (see [`RepoCache::cap_bytes`]).
    pub cap_bytes: Option<u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            cap_bytes: None,
        }
    }
}

/// What Settings shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// The cache is on.
    pub enabled: bool,
    /// Repositories held.
    pub repos: u32,
    /// What they take on disk.
    pub bytes: u64,
    /// The cap in force.
    pub cap_bytes: u64,
    /// Clones served from a mirror in the last week.
    pub hits: u32,
    /// Clones that went the normal way in the last week (no mirror yet, or
    /// one that could not be used).
    pub misses: u32,
    /// Seconds those hits saved.
    pub seconds_saved: u64,
    /// Bytes they did not download.
    pub bytes_saved: u64,
}

/// The cache of one machine: a directory and its settings. Cheap to make;
/// what is shared between calls (one fill at a time, which repositories
/// were just found private) is process-wide.
#[derive(Clone, Debug)]
pub struct RepoCache {
    root: PathBuf,
    cfg: Config,
    pub(crate) policy: Policy,
}

/// Which protocols the daemon's own git runs may use and whether the host
/// must be public.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    pub allow: &'static str,
    pub check_host: bool,
}

pub(crate) const STRICT: Policy = Policy {
    allow: "https",
    check_host: true,
};

pub(crate) struct State {
    /// One fill or refresh at a time.
    pub gate: tokio::sync::Mutex<()>,
    /// Repositories found not public (or unreachable), until when.
    pub negative: Mutex<std::collections::HashMap<(PathBuf, String), std::time::Instant>>,
    /// Roots whose leftovers were swept.
    pub swept: Mutex<std::collections::HashSet<PathBuf>>,
}

pub(crate) fn state() -> &'static State {
    static S: OnceLock<State> = OnceLock::new();
    S.get_or_init(|| State {
        gate: tokio::sync::Mutex::new(()),
        negative: Mutex::new(Default::default()),
        swept: Mutex::new(Default::default()),
    })
}

impl RepoCache {
    /// The cache in `root`.
    pub fn new(root: impl Into<PathBuf>, cfg: Config) -> Self {
        Self {
            root: root.into(),
            cfg,
            policy: STRICT,
        }
    }

    /// Unit tests mirror local repositories.
    #[cfg(test)]
    pub(crate) fn allowing_local_repos(mut self) -> Self {
        self.policy = Policy {
            allow: "file,https",
            check_host: false,
        };
        self
    }

    /// `$CUA_HOME/cache/repos`.
    pub fn default_root() -> PathBuf {
        cua_home::cua_home().join("cache").join("repos")
    }

    /// The cache in [`RepoCache::default_root`].
    pub fn at_default(cfg: Config) -> Self {
        Self::new(Self::default_root(), cfg)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config(&self) -> Config {
        self.cfg
    }

    /// The size cap in force: [`Config::cap_bytes`], else the smaller of
    /// [`DEFAULT_CAP_BYTES`] and a tenth of the disk that is free plus what
    /// the cache itself holds (so using the cache does not shrink it).
    pub fn cap_bytes(&self) -> u64 {
        if let Some(c) = self.cfg.cap_bytes {
            return c;
        }
        let held: u64 = store::entries(&self.root).iter().map(|e| e.bytes).sum();
        let free = store::free_bytes(if self.root.exists() {
            &self.root
        } else {
            self.root.parent().unwrap_or(&self.root)
        })
        .unwrap_or(DEFAULT_CAP_BYTES.saturating_mul(10));
        DEFAULT_CAP_BYTES.min(free.saturating_add(held) / 10)
    }

    /// Creates what a Space reads (the helper, the git config) and removes
    /// what an interrupted run left. Call before mounting.
    pub fn prepare(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(store::m_dir(&self.root))?;
        std::fs::create_dir_all(store::home_dir(&self.root))?;
        let first = state()
            .swept
            .lock()
            .map(|mut s| s.insert(self.root.clone()))
            .unwrap_or(false);
        if first {
            store::sweep_leftovers(&self.root);
            std::fs::create_dir_all(store::m_dir(&self.root))?;
        }
        shim::write_helper(&self.root)?;
        if !self.root.join("gitconfig").exists() || first {
            shim::write_config(&self.root)?;
        }
        Ok(())
    }

    /// The directory to mount read-only at [`GUEST_ROOT`] in a container
    /// Space: `Some` while the cache is on and ready.
    pub fn mount_source(&self) -> Option<PathBuf> {
        (self.cfg.enabled && self.prepare().is_ok()).then(|| self.root.clone())
    }

    /// What Settings shows.
    pub fn status(&self) -> Status {
        let entries = store::entries(&self.root);
        let w = stats::week(&self.root);
        Status {
            enabled: self.cfg.enabled,
            repos: entries.len() as u32,
            bytes: entries.iter().map(|e| e.bytes).sum(),
            cap_bytes: self.cap_bytes(),
            hits: w.hits,
            misses: w.misses,
            seconds_saved: w.seconds_saved,
            bytes_saved: w.bytes_saved,
        }
    }

    /// Removes every mirror (the counters stay).
    pub fn clear(&self) {
        store::clear(&self.root);
        let _ = shim::write_config(&self.root);
    }

    /// The mirrors, least recently used first.
    pub fn entries(&self) -> Vec<Entry> {
        store::entries(&self.root)
    }

    /// What a command line clones: counts a repeat of a mirrored repository
    /// as used, a repository with no mirror as a miss, and in a running
    /// tokio runtime starts filling or refreshing each in the background.
    /// Never waits, never fails; nothing happens while the cache is off.
    pub fn learn(&self, text: &str) {
        if !self.cfg.enabled {
            return;
        }
        let repos = url::urls_in(text);
        if repos.is_empty() {
            return;
        }
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        for r in repos {
            if store::entry(&self.root, &r.key).is_some() {
                store::touch(&self.root, &r.key);
            } else {
                self.note_miss();
            }
            let me = self.clone();
            rt.spawn(async move {
                let _ = me.ensure(&r).await;
            });
        }
    }

    /// A clone that went the normal way.
    pub fn note_miss(&self) {
        stats::miss(&self.root);
        cua_telemetry::global().repo_cache("miss", "network", 0, 0, "none");
    }

    /// Takes what a Space's helper logged (see
    /// [`shim::take_log_command`]): counts the hits, with what each saved,
    /// and the mirrors it could not use.
    pub fn ingest_guest_log(&self, text: &str) {
        for l in shim::parse_log(text) {
            match l.kind {
                shim::LogKind::HitLease | shim::LogKind::HitChecked => {
                    let Some(e) = store::entry(&self.root, &l.key) else {
                        continue;
                    };
                    store::touch(&self.root, &l.key);
                    let saved = (e.fill_secs - f64::from(l.secs)).max(0.0);
                    stats::hit(&self.root, saved, e.bytes);
                    cua_telemetry::global().repo_cache(
                        "hit",
                        "local",
                        e.bytes,
                        saved.round() as u64,
                        "none",
                    );
                }
                shim::LogKind::Skipped | shim::LogKind::Failed | shim::LogKind::Kept => {
                    stats::fallback(&self.root);
                    let reason = match l.kind {
                        shim::LogKind::Skipped => "verify_failed",
                        shim::LogKind::Kept => "disk_full",
                        _ => "other",
                    };
                    cua_telemetry::global().repo_cache("fallback", "network", 0, 0, reason);
                    // A mirror whose packs changed under us is not served
                    // again.
                    if l.kind == shim::LogKind::Skipped {
                        self.drop_if_damaged(&l.key);
                    }
                }
            }
        }
    }

    /// Checks one mirror again and removes it when it does not pass.
    fn drop_if_damaged(&self, key: &str) {
        let Some(e) = store::entry(&self.root, key) else {
            return;
        };
        let dir = store::mirror_dir(&self.root, key);
        let intact = e.packs.iter().all(|p| {
            std::fs::metadata(dir.join("objects/pack").join(&p.name))
                .is_ok_and(|m| m.len() == p.size)
        });
        if !intact {
            store::drop_entry(&self.root, key);
            let _ = shim::write_config(&self.root);
        }
    }

    /// Fills or refreshes the mirror of `repo` (see [`Outcome`]); one at a
    /// time. Never touches anything outside the cache directory.
    pub async fn ensure(&self, repo: &RepoUrl) -> Outcome {
        fill::ensure(self, repo).await
    }

    /// Keeps the cache within its cap, drops mirrors that no longer verify,
    /// and refreshes the ones used in the last week that were not refreshed
    /// for `older_than`, one at a time.
    pub async fn maintain(&self, older_than: Duration) {
        if !self.cfg.enabled {
            return;
        }
        let _ = self.prepare();
        fill::enforce_cap(self, None);
        let now = store::now_ms();
        for e in store::entries(&self.root).into_iter().rev() {
            let used_recently = now.saturating_sub(e.used_ms) < 7 * 24 * 3600 * 1000;
            let stale = now.saturating_sub(e.refreshed_ms) > older_than.as_millis() as u64;
            if used_recently
                && stale
                && let Ok(repo) = url::parse(&e.url)
            {
                let _ = self.ensure(&repo).await;
            }
        }
    }
}

/// A shared handle for the daemon.
pub type Shared = Arc<RepoCache>;
