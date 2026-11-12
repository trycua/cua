//! Images the SDK pulled into the container engine.
//!
//! A pulled registry image cannot carry the `ai.cua.managed` label (labels
//! are part of the image config), so the SDK records each pull it made:
//! one file per reference under `$CUA_HOME/docker/pulls/`, written only when
//! the image was not in the engine before. The file's mtime is the last
//! time the SDK used the image. Cache garbage collection removes a recorded
//! image only while the engine still has the same image id under that
//! reference and no container uses it; images the user pulled themselves are
//! never recorded and never touched.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// One recorded pull.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRecord {
    /// Reference as pulled.
    pub reference: String,
    /// Engine image id right after the pull (`sha256:...`).
    pub image_id: String,
    /// Unix seconds of the pull.
    pub pulled_at: u64,
    /// Last use (the record file's mtime); filled in on read.
    #[serde(skip)]
    pub last_used: Option<SystemTime>,
}

/// The pull ledger directory.
#[derive(Clone, Debug)]
pub struct PullLedger {
    dir: PathBuf,
}

impl Default for PullLedger {
    fn default() -> Self {
        Self::new(crate::host::cua_home().join("docker").join("pulls"))
    }
}

impl PullLedger {
    /// A ledger in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The record file of `reference`.
    pub fn file(&self, reference: &str) -> PathBuf {
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(reference.as_bytes());
        let hex: String = d.iter().take(16).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{hex}.json"))
    }

    /// Records that the SDK pulled `reference` as `image_id`.
    pub fn record(&self, reference: &str, image_id: &str) {
        let rec = PullRecord {
            reference: reference.into(),
            image_id: image_id.into(),
            pulled_at: crate::host::now_secs(),
            last_used: None,
        };
        let res = std::fs::create_dir_all(&self.dir).and_then(|()| {
            let path = self.file(reference);
            let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
            std::fs::write(&tmp, serde_json::to_vec_pretty(&rec)?)?;
            std::fs::rename(tmp, path)
        });
        if let Err(e) = res {
            tracing::debug!(reference, error = %e, "could not record the image pull");
        }
    }

    /// Marks a recorded reference as used now (no-op when not recorded).
    pub fn touch(&self, reference: &str) {
        let f = self.file(reference);
        if f.exists() {
            crate::disk::mark_used(&f);
        }
    }

    /// Forgets `reference`.
    pub fn forget(&self, reference: &str) {
        let _ = std::fs::remove_file(self.file(reference));
    }

    /// Every record.
    pub fn records(&self) -> Vec<PullRecord> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        rd.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| {
                let mut r: PullRecord =
                    serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
                r.last_used = crate::disk::last_used(&e.path());
                Some(r)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_and_forget() {
        let d = tempfile::tempdir().unwrap();
        let l = PullLedger::new(d.path());
        assert!(l.records().is_empty());
        l.touch("never/recorded:1");
        assert!(l.records().is_empty());
        l.record("docker.io/library/alpine:3.20", "sha256:aa");
        let r = l.records();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].image_id, "sha256:aa");
        assert!(r[0].last_used.is_some());
        l.forget("docker.io/library/alpine:3.20");
        assert!(l.records().is_empty());
    }
}
