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

/// The repo cache's directory as a container Space gets it: read-only, at
/// [`GUEST_ROOT`], and nothing else (no network, no other host directory).
pub fn mount(root: std::path::PathBuf) -> cua_vmm::Mount {
    cua_vmm::Mount {
        host: root,
        guest: GUEST_ROOT.to_string(),
        read_only: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_mount_is_read_only_at_the_guest_root_and_accepted_by_a_container_backend() {
        let dir = tempfile::tempdir().unwrap();
        let m = mount(dir.path().to_path_buf());
        assert!(m.read_only);
        assert_eq!(m.guest, "/cua/repo-cache");
        assert_eq!(m.host, dir.path());
        cua_vmm::types::check_mounts(cua_vmm::BackendKind::Container, std::slice::from_ref(&m))
            .unwrap();
    }

    #[test]
    fn a_clone_in_a_space_never_names_a_secret_or_a_url_in_what_the_helper_logs() {
        // The helper's log line is a time, a fixed word, the repository's
        // hash key and seconds: the parser takes nothing else.
        let l = shim::parse_log("1700000000 hit_checked 0123456789abcdef 1\n");
        assert_eq!(l.len(), 1);
        assert!(shim::parse_log("1700000000 hit_checked https://github.com/o/r 1\n").is_empty());
    }
}
