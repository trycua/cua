//! The repo cache with this machine's settings (`cache.repos` and
//! `cache.repos_size_gb`, see [`crate::settings`]).

pub use cua_repo_cache::{
    Config, DEFAULT_CAP_BYTES, Entry, GUEST_ROOT, Outcome, Reason, RepoCache, RepoUrl, Status, shim,
};

/// The cache under `$CUA_HOME` with the settings read now: on unless
/// `cache.repos` is off, capped by `cache.repos_size_gb` (else the default).
pub fn current() -> RepoCache {
    let (enabled, cap_bytes) = crate::settings::Settings::load()
        .map(|s| s.repo_cache())
        .unwrap_or((true, None));
    RepoCache::at_default(Config { enabled, cap_bytes })
}

/// Whether a command line may clone a repository (a cheap check: no
/// settings are read for a command that does not).
pub fn names_a_clone(command: &str) -> bool {
    !cua_repo_cache::url::urls_in(command).is_empty()
}
