//! Where the runner keeps its files: `$CUA_HOME/ci/`.
//!
//! ```text
//! $CUA_HOME/ci/
//!   <pool>.toml               the pool's configuration (no secret)
//!   <pool>/state.json         the slots the reconciler owns
//!   <pool>/events.jsonl       the event log
//!   <pool>/serve.log          the service's output
//!   <pool>/logs/<slot>/       runner logs copied out of each Space
//!   cache/                    actions/runner tarballs, by platform and version
//!   secrets/                  credentials where there is no OS keychain (0600)
//! ```

use std::path::{Path, PathBuf};

/// The runner home and the paths under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiPaths {
    root: PathBuf,
}

impl CiPaths {
    /// `$CUA_HOME/ci` (see [`cua_home::cua_home`]).
    pub fn from_env() -> Self {
        Self::new(cua_home::cua_home().join("ci"))
    }

    /// Under `root` (tests use a temporary directory).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The runner home.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A pool's configuration file.
    pub fn config(&self, pool: &str) -> PathBuf {
        self.root.join(format!("{pool}.toml"))
    }

    /// A pool's own directory.
    pub fn pool_dir(&self, pool: &str) -> PathBuf {
        self.root.join(pool)
    }

    /// The slots a pool's reconciler owns.
    pub fn state(&self, pool: &str) -> PathBuf {
        self.pool_dir(pool).join("state.json")
    }

    /// A pool's event log.
    pub fn events(&self, pool: &str) -> PathBuf {
        self.pool_dir(pool).join("events.jsonl")
    }

    /// The service's output.
    pub fn serve_log(&self, pool: &str) -> PathBuf {
        self.pool_dir(pool).join("serve.log")
    }

    /// Runner logs copied out of each Space.
    pub fn logs(&self, pool: &str) -> PathBuf {
        self.pool_dir(pool).join("logs")
    }

    /// One slot's copied logs.
    pub fn slot_logs(&self, pool: &str, slot: &str) -> PathBuf {
        self.logs(pool).join(slot)
    }

    /// The actions/runner tarball cache.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }

    /// Credentials where there is no OS keychain.
    pub fn secrets(&self) -> PathBuf {
        self.root.join("secrets")
    }

    /// Every pool with a configuration file, sorted.
    pub fn pools(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.root)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                (p.extension().is_some_and(|x| x == "toml") && p.is_file())
                    .then(|| p.file_stem()?.to_str().map(str::to_string))
                    .flatten()
            })
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_are_the_toml_files() {
        let d = tempfile::tempdir().unwrap();
        let p = CiPaths::new(d.path());
        std::fs::write(p.config("b"), "").unwrap();
        std::fs::write(p.config("a"), "").unwrap();
        std::fs::create_dir_all(p.pool_dir("a")).unwrap();
        std::fs::write(d.path().join("notes.txt"), "").unwrap();
        assert_eq!(p.pools(), vec!["a".to_string(), "b".to_string()]);
        assert_eq!(p.events("a"), d.path().join("a/events.jsonl"));
        assert_eq!(p.slot_logs("a", "s1"), d.path().join("a/logs/s1"));
    }
}
