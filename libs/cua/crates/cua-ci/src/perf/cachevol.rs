//! Dependency caches for job VMs: pnpm, cargo, SwiftPM and sccache.
//!
//! **The poisoning rule.** A cache is read by a job and written only by a
//! job that runs trusted code. Trust comes from the job's metadata (the
//! hosted service reads it from GitHub), never from the guest:
//!
//! | job | reads | writes |
//! |---|---|---|
//! | push to the default branch | the repo's cache | yes |
//! | push to another branch, same-repo pull request | the repo's cache | no |
//! | fork pull request, public repo | the repo's cache, read-only | no (a refused write is reported) |
//! | fork pull request, private repo | nothing | no |
//! | `pull_request_target`, `issue_comment`, `workflow_run`, anything unknown | nothing | no |
//!
//! Caches are keyed by account and repository ([`scope_key`]): another
//! repository, or the same name under another account, never shares a
//! byte. A key also carries a toolchain key ([`toolchain_key`]) so an Xcode
//! or Node upgrade starts a fresh cache instead of mixing versions.
//!
//! **No VM writes a cache.** A VM mounts the repo's cache folder read-only
//! (or gets it uploaded when the runtime cannot mount) and restores the
//! stores into local folders before the first step. A default-branch job
//! that succeeded hands back only the files it changed, as one tar; the
//! host checks it ([`CacheRoot::promote`]: no links out of the cache, only
//! plain files, size and file count limits), builds the next version beside
//! the current one and switches a `current` link atomically. A job that is
//! still reading the old version keeps reading it.
//!
//! On the host: `<root>/<scope key>/<store>/<toolchain key>/v-<n>/` and a
//! `current` link; `<root>/<scope key>/` is what a VM mounts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use sha2::{Digest as _, Sha256};

use super::CacheConfig;
use crate::config::GuestOs;
use crate::guest::sh_quote;

/// One cache store.
#[derive(Debug)]
pub struct Store {
    /// `pnpm`, `cargo`, `swiftpm`, `sccache`.
    pub name: &'static str,
    /// Folders of the store: (folder inside the cache, guest path under
    /// `$HOME`).
    pub dirs: &'static [(&'static str, &'static str)],
    /// The toolchain parts a cache of this store depends on (keys of
    /// [`toolchain_key`]'s input).
    pub tools: &'static [&'static str],
}

/// The stores, in restore order.
pub static STORES: [Store; 4] = [
    Store {
        name: "pnpm",
        dirs: &[("store", ".cache/cua-ci/pnpm-store")],
        tools: &["node", "pnpm"],
    },
    Store {
        name: "cargo",
        dirs: &[("registry", ".cargo/registry"), ("git", ".cargo/git")],
        tools: &[],
    },
    Store {
        name: "swiftpm",
        dirs: &[
            ("cache", "Library/Caches/org.swift.swiftpm"),
            ("config", ".swiftpm/cache"),
        ],
        tools: &["xcode"],
    },
    Store {
        name: "sccache",
        dirs: &[("objects", ".cache/cua-ci/sccache")],
        tools: &[],
    },
];

/// The environment the runner's wrapper exports so job steps use the
/// restored folders (relative to `$HOME`).
pub const STORE_ENV: &[(&str, &str)] = &[
    ("npm_config_store_dir", ".cache/cua-ci/pnpm-store"),
    ("SCCACHE_DIR", ".cache/cua-ci/sccache"),
];

/// A store by name.
pub fn store(name: &str) -> Option<&'static Store> {
    STORES.iter().find(|s| s.name == name)
}

/// How much to trust a job's code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trust {
    /// Push, schedule or manual run on the default branch: reviewed code.
    DefaultBranch,
    /// Push to another branch, or a pull request from the same repository.
    SameRepo,
    /// A pull request from a fork.
    Fork,
    /// `pull_request_target`, `issue_comment`, `workflow_run`: base-branch
    /// code with a stranger's input.
    Untrusted,
    /// Not enough metadata to say.
    Unknown,
}

/// What the controller knows about a job before it runs (from the hosted
/// service's claim; none of it comes from the VM).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobMeta {
    /// The account the computer serves (`me` or a team id).
    pub account: String,
    /// `owner/name`.
    pub repo: String,
    /// The repository is private (`None`: unknown, treated as private).
    pub private: Option<bool>,
    /// `GITHUB_EVENT_NAME`.
    pub event: Option<String>,
    /// `GITHUB_REF` (`refs/heads/main`, `refs/pull/7/merge`).
    pub git_ref: Option<String>,
    /// The repository the head commit lives in (a fork's `owner/name`).
    pub head_repo: Option<String>,
    /// The repository's default branch.
    pub default_branch: Option<String>,
    /// The service's own word that the job runs a fork's pull request
    /// (it decides from GitHub, never from the client).
    pub fork: Option<bool>,
    /// The ceiling the service's policy sets (`policy.cache` of the claim).
    pub policy_cache: Option<CachePolicy>,
}

/// The service's cache policy for a job (`rw`, `ro`, `off`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CachePolicy {
    /// Read and write.
    Rw,
    /// Read only.
    Ro,
    /// Neither.
    Off,
}

impl CachePolicy {
    /// `rw`, `ro` or `off`.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "rw" => Some(Self::Rw),
            "ro" => Some(Self::Ro),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// The trust a job gets.
pub fn trust_of(m: &JobMeta) -> Trust {
    let Some(event) = m.event.as_deref() else {
        return Trust::Unknown;
    };
    let same_head = m.fork != Some(true)
        && m.head_repo
            .as_deref()
            .is_none_or(|h| h.eq_ignore_ascii_case(&m.repo));
    match event {
        "pull_request_target"
        | "issue_comment"
        | "workflow_run"
        | "pull_request_review"
        | "pull_request_review_comment"
        | "issues"
        | "discussion"
        | "discussion_comment" => Trust::Untrusted,
        "pull_request" | "merge_group" => {
            if same_head {
                Trust::SameRepo
            } else {
                Trust::Fork
            }
        }
        "push" | "schedule" | "workflow_dispatch" | "repository_dispatch" | "release" => {
            let (Some(r), Some(d)) = (m.git_ref.as_deref(), m.default_branch.as_deref()) else {
                return Trust::Unknown;
            };
            if !same_head {
                Trust::Fork
            } else if r == format!("refs/heads/{d}") {
                Trust::DefaultBranch
            } else {
                Trust::SameRepo
            }
        }
        _ => Trust::Unknown,
    }
}

/// What a job does with the caches.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachePlan {
    /// The trust that decided it.
    pub trust: Trust,
    /// The scope the job reads: none, or this repository's key.
    pub read: Option<ScopeKey>,
    /// The job may hand back changed files when it succeeds.
    pub write: bool,
    /// `default_branch`, `repo` or `fork_readonly` (the bench's words);
    /// empty with no read.
    pub scope: &'static str,
    /// A write attempt is refused and reported (`fork_readonly_denied`).
    pub report_denied_writes: bool,
}

impl CachePlan {
    /// No cache at all.
    pub fn none(trust: Trust) -> Self {
        Self {
            trust,
            read: None,
            write: false,
            scope: "",
            report_denied_writes: false,
        }
    }
}

/// Decides what a job does with the caches.
pub fn plan_for(m: &JobMeta, cfg: &CacheConfig) -> CachePlan {
    if !cfg.enabled || cfg.stores.is_empty() || m.repo.is_empty() || m.account.is_empty() {
        return CachePlan::none(Trust::Unknown);
    }
    let trust = trust_of(m);
    let key = scope_key(&m.account, &m.repo);
    let private = m.private.unwrap_or(true);
    // The service's policy is a ceiling: it can only take rights away.
    if m.policy_cache == Some(CachePolicy::Off) {
        return CachePlan::none(trust);
    }
    let mut plan = decide(trust, key, private, cfg);
    if m.policy_cache == Some(CachePolicy::Ro) {
        plan.write = false;
    }
    plan
}

fn decide(trust: Trust, key: ScopeKey, private: bool, cfg: &CacheConfig) -> CachePlan {
    match trust {
        Trust::DefaultBranch => CachePlan {
            trust,
            read: Some(key),
            write: true,
            scope: "default_branch",
            report_denied_writes: false,
        },
        Trust::SameRepo => CachePlan {
            trust,
            read: Some(key),
            write: false,
            scope: "repo",
            report_denied_writes: false,
        },
        Trust::Fork if !private && cfg.fork_read_public => CachePlan {
            trust,
            read: Some(key),
            write: false,
            scope: "fork_readonly",
            report_denied_writes: true,
        },
        Trust::Fork => CachePlan {
            report_denied_writes: true,
            ..CachePlan::none(trust)
        },
        Trust::Untrusted | Trust::Unknown => CachePlan::none(trust),
    }
}

/// The folder name of one account's one repository's caches.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ScopeKey(String);

impl ScopeKey {
    /// 32 hex characters.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The key of an account's repository (case-insensitive, the account and
/// the repository both in the hash).
pub fn scope_key(account: &str, repo: &str) -> ScopeKey {
    let mut h = Sha256::new();
    h.update(b"cua-ci-cache-v1\0");
    h.update(account.trim().to_ascii_lowercase().as_bytes());
    h.update([0]);
    h.update(repo.trim().to_ascii_lowercase().as_bytes());
    ScopeKey(hex::encode(&h.finalize()[..16]))
}

/// `sha256` of a key in full, for events (the bench never sees a repo name
/// in clear in a cache event).
pub fn hash_for_event(s: &str) -> String {
    hex::encode(Sha256::digest(s.trim().to_ascii_lowercase().as_bytes()))
}

/// The toolchain part of a store's key: the versions of the tools the
/// store depends on (`xcode`, `node`, `pnpm`, ...), or `any`.
pub fn toolchain_key(store: &Store, versions: &BTreeMap<String, String>) -> String {
    if store.tools.is_empty() {
        return "any".into();
    }
    let mut h = Sha256::new();
    for t in store.tools {
        h.update(t.as_bytes());
        h.update([0]);
        h.update(
            versions
                .get(*t)
                .map(String::as_str)
                .unwrap_or("?")
                .as_bytes(),
        );
        h.update([0]);
    }
    hex::encode(&h.finalize()[..6])
}

/// Where a VM sees a host folder shared into it.
pub fn guest_mount_path(os: GuestOs, host_dir_name: &str) -> String {
    match os {
        // Lume shares appear under one root, by folder name.
        GuestOs::Macos => format!("/Volumes/My Shared Files/{host_dir_name}"),
        GuestOs::Linux => format!("/cua/ci-cache/{host_dir_name}"),
    }
}

/// `~/.cua-ci/job.env`: what the job-start hook reads (shell syntax).
/// `root` is where the cache is in the guest (a mount, or the upload's
/// folder); `stores` are the stores to restore with their toolchain keys.
pub fn job_env(
    plan: &CachePlan,
    root: &str,
    stores: &[(String, String)],
    mirror: Option<&str>,
) -> String {
    let mut out = String::from("# written by cua ci before the job; read by the job-start hook\n");
    out.push_str(&format!("CUA_CI_CACHE_SCOPE={}\n", sh_quote(plan.scope)));
    out.push_str(&format!("CUA_CI_CACHE_ROOT={}\n", sh_quote(root)));
    let list: Vec<String> = stores.iter().map(|(s, t)| format!("{s}/{t}")).collect();
    out.push_str(&format!(
        "CUA_CI_CACHE_STORES={}\n",
        sh_quote(&list.join(" "))
    ));
    out.push_str(&format!(
        "CUA_CI_CACHE_WRITE={}\n",
        if plan.write { 1 } else { 0 }
    ));
    if let Some(m) = mirror {
        out.push_str(&format!("CUA_CI_MIRROR={}\n", sh_quote(m)));
    }
    out
}

/// Limits of an incoming delta.
#[derive(Clone, Copy, Debug)]
pub struct PromoteLimits {
    /// The delta's extracted size.
    pub max_bytes: u64,
    /// Files in it.
    pub max_files: u64,
}

impl Default for PromoteLimits {
    fn default() -> Self {
        Self {
            max_bytes: 4 << 30,
            max_files: 1_000_000,
        }
    }
}

/// What a promotion did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Promoted {
    /// Bytes in the delta.
    pub bytes: u64,
    /// Files in it.
    pub files: u64,
}

/// Why a delta was refused (the cache is unchanged).
#[derive(Debug, thiserror::Error)]
pub enum PromoteError {
    /// The delta holds something a cache must not: a link out of the
    /// cache, a device, a socket.
    #[error("the cache delta holds {0}")]
    Unsafe(String),
    /// Past a limit.
    #[error("the cache delta is too large: {0}")]
    TooLarge(String),
    /// `tar` or the filesystem failed.
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for PromoteError {
    fn from(e: std::io::Error) -> Self {
        PromoteError::Io(e.to_string())
    }
}

static WRITE_LOCK: Mutex<()> = Mutex::new(());

/// The host's cache folder.
#[derive(Clone, Debug)]
pub struct CacheRoot {
    root: PathBuf,
}

impl CacheRoot {
    /// At `root` (`<ci home>/perf/cache`).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The folder a VM of this scope mounts read-only.
    pub fn scope_dir(&self, key: &ScopeKey) -> PathBuf {
        self.root.join(key.as_str())
    }

    fn store_dir(&self, key: &ScopeKey, store: &str, tc: &str) -> PathBuf {
        self.scope_dir(key).join(store).join(tc)
    }

    /// The current version of a store, when there is one.
    pub fn current(&self, key: &ScopeKey, store: &str, tc: &str) -> Option<PathBuf> {
        let link = self.store_dir(key, store, tc).join("current");
        let p = std::fs::canonicalize(&link).ok()?;
        p.is_dir().then_some(p)
    }

    /// Marks a scope as used now (eviction order and its short lease).
    pub fn touch(&self, key: &ScopeKey) {
        let d = self.scope_dir(key);
        if std::fs::create_dir_all(&d).is_ok() {
            let _ = std::fs::write(d.join(".last-used"), b"");
            // A rewrite sets the mtime.
        }
    }

    /// Stores of `want` that have a current version, with their toolchain
    /// keys, for the job's restore list.
    pub fn available(
        &self,
        key: &ScopeKey,
        want: &[String],
        versions: &BTreeMap<String, String>,
    ) -> Vec<(String, String)> {
        want.iter()
            .filter_map(|s| {
                let st = store(s)?;
                let tc = toolchain_key(st, versions);
                self.current(key, s, &tc).map(|_| (s.clone(), tc))
            })
            .collect()
    }

    /// Adds deltas (one tar per store folder, of the files a default-branch
    /// job changed, entries relative to that folder) to a store as a new
    /// version and switches `current` to it. The caller has decided the job
    /// may write ([`CachePlan::write`]); this checks the bytes. `deltas`
    /// pairs a folder of the store (`registry`, ...; it must be one the
    /// store table names) with its tar. On any error the cache is as it was.
    pub fn promote(
        &self,
        key: &ScopeKey,
        store_name: &str,
        tc: &str,
        deltas: &[(&str, &Path)],
        limits: PromoteLimits,
    ) -> Result<Promoted, PromoteError> {
        let st =
            store(store_name).ok_or_else(|| PromoteError::Io(format!("no store {store_name}")))?;
        for (sub, _) in deltas {
            if !st.dirs.iter().any(|(d, _)| d == sub) {
                return Err(PromoteError::Unsafe(format!(
                    "a folder the store does not have ({sub})"
                )));
            }
        }
        let _g = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = self.store_dir(key, st.name, tc);
        std::fs::create_dir_all(&base)?;
        let n = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let pending = base.join(format!(".incoming-{n}"));
        let fin = base.join(format!("v-{n}"));
        let result = (|| {
            if let Some(cur) = self.current(key, st.name, tc) {
                copy_tree(&cur, &pending)?;
            } else {
                std::fs::create_dir_all(&pending)?;
            }
            for (sub, tar) in deltas {
                let into = pending.join(sub);
                std::fs::create_dir_all(&into)?;
                let out = std::process::Command::new("tar")
                    .arg("-xf")
                    .arg(tar)
                    .arg("-C")
                    .arg(&into)
                    .arg("--no-same-owner")
                    .arg("--no-same-permissions")
                    .output()?;
                if !out.status.success() {
                    return Err(PromoteError::Io(format!(
                        "tar: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    )));
                }
            }
            let found = check_tree(&pending, limits)?;
            std::fs::rename(&pending, &fin)?;
            let tmp = base.join("current.tmp");
            let _ = std::fs::remove_file(&tmp);
            make_symlink(Path::new(fin.file_name().unwrap_or_default()), &tmp)?;
            std::fs::rename(&tmp, base.join("current"))?;
            Ok(found)
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&pending);
        } else {
            gc_versions(&base, 2);
        }
        result
    }

    /// Deletes the least recently used scopes until all caches fit in
    /// `cap_bytes`. A scope used in the last `lease` is never deleted
    /// (a VM may still read it). Returns what was deleted.
    pub fn evict(&self, cap_bytes: u64, lease: Duration) -> Vec<String> {
        let mut scopes: Vec<(SystemTime, String, u64)> = Vec::new();
        for e in std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
        {
            if !e.path().is_dir() {
                continue;
            }
            let used = std::fs::metadata(e.path().join(".last-used"))
                .and_then(|m| m.modified())
                .or_else(|_| e.metadata().and_then(|m| m.modified()))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            scopes.push((
                used,
                e.file_name().to_string_lossy().into_owned(),
                dir_size(&e.path()),
            ));
        }
        let mut total: u64 = scopes.iter().map(|s| s.2).sum();
        scopes.sort();
        let mut gone = Vec::new();
        let now = SystemTime::now();
        for (used, name, size) in scopes {
            if total <= cap_bytes {
                break;
            }
            if now.duration_since(used).unwrap_or_default() < lease {
                continue;
            }
            if std::fs::remove_dir_all(self.root.join(&name)).is_ok() {
                total = total.saturating_sub(size);
                gone.push(name);
            }
        }
        gone
    }
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn make_symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other("the cache needs a Unix host"))
}

fn gc_versions(base: &Path, keep: usize) {
    let mut vs: Vec<_> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("v-"))
        .collect();
    vs.sort_by_key(|e| e.file_name());
    while vs.len() > keep {
        let e = vs.remove(0);
        let _ = std::fs::remove_dir_all(e.path());
    }
}

/// Total size of the files under `p`.
pub fn dir_size(p: &Path) -> u64 {
    let mut n = 0;
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        match e.file_type() {
            Ok(t) if t.is_dir() => n += dir_size(&e.path()),
            Ok(t) if t.is_file() => n += e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => {}
        }
    }
    n
}

/// Copies a tree (files, folders, relative links) without following links.
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = e.file_type()?;
        let dst = to.join(e.file_name());
        if t.is_dir() {
            copy_tree(&e.path(), &dst)?;
        } else if t.is_symlink() {
            make_symlink(&std::fs::read_link(e.path())?, &dst)?;
        } else if t.is_file() {
            // A hard link is as good as a copy for a tree that is only
            // ever replaced whole (never edited in place), and instant.
            if std::fs::hard_link(e.path(), &dst).is_err() {
                std::fs::copy(e.path(), &dst)?;
            }
        }
    }
    Ok(())
}

/// Walks a tree: only folders, regular files and links that stay inside it.
fn check_tree(root: &Path, limits: PromoteLimits) -> Result<Promoted, PromoteError> {
    fn walk(
        root: &Path,
        dir: &Path,
        limits: PromoteLimits,
        acc: &mut Promoted,
    ) -> Result<(), PromoteError> {
        for e in std::fs::read_dir(dir)? {
            let e = e?;
            let path = e.path();
            let t = e.file_type()?;
            if t.is_dir() {
                walk(root, &path, limits, acc)?;
            } else if t.is_symlink() {
                let target = std::fs::read_link(&path)?;
                if target.is_absolute() {
                    return Err(PromoteError::Unsafe(format!(
                        "an absolute link ({})",
                        path.strip_prefix(root).unwrap_or(&path).display()
                    )));
                }
                // Resolve lexically from the link's folder: it must stay under root.
                let mut depth = path
                    .strip_prefix(root)
                    .map(|p| p.components().count().saturating_sub(1) as i64)
                    .unwrap_or(0);
                for c in target.components() {
                    match c {
                        std::path::Component::ParentDir => depth -= 1,
                        std::path::Component::Normal(_) => depth += 1,
                        _ => {}
                    }
                    if depth < 0 {
                        return Err(PromoteError::Unsafe(format!(
                            "a link out of the cache ({})",
                            path.strip_prefix(root).unwrap_or(&path).display()
                        )));
                    }
                }
            } else if t.is_file() {
                acc.files += 1;
                acc.bytes += e.metadata()?.len();
                if acc.files > limits.max_files {
                    return Err(PromoteError::TooLarge(format!(
                        "more than {} files",
                        limits.max_files
                    )));
                }
                if acc.bytes > limits.max_bytes {
                    return Err(PromoteError::TooLarge(format!(
                        "more than {} bytes",
                        limits.max_bytes
                    )));
                }
            } else {
                return Err(PromoteError::Unsafe(format!(
                    "a device or socket ({})",
                    path.strip_prefix(root).unwrap_or(&path).display()
                )));
            }
        }
        Ok(())
    }
    let mut acc = Promoted { bytes: 0, files: 0 };
    walk(root, root, limits, &mut acc)?;
    Ok(acc)
}

#[cfg(test)]
mod tests;
