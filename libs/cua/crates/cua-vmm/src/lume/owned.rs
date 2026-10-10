//! Lume VMs the SDK created.
//!
//! Lume keeps every VM in its own store (`~/.lume` by default), next to VMs
//! the user made with `lume` directly. The SDK records each VM it creates
//! (pulled bases, clones, Linux disk VMs, forks) as
//! `$CUA_HOME/vmm/lume/owned/<name>.json`, so `cua cache` can report them
//! and the cache garbage collection only ever removes VMs the SDK made. The
//! file's mtime is the VM's last use.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// Why the SDK created a VM.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnedKind {
    /// A pulled image (`cua-base-<sha12>`), the clone source of sandboxes.
    Base,
    /// A sandbox.
    Instance,
    /// A stopped-state checkpoint or fork.
    Checkpoint,
}

/// One recorded VM.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnedVm {
    /// VM name.
    pub name: String,
    /// Kind.
    pub kind: OwnedKind,
    /// Image reference (bases) or clone source.
    #[serde(default)]
    pub source: Option<String>,
    /// Unix seconds of creation.
    pub created_at: u64,
    /// Last use (the record's mtime); filled in on read.
    #[serde(skip)]
    pub last_used: Option<SystemTime>,
}

/// The ownership records.
#[derive(Clone, Debug)]
pub struct OwnedVms {
    dir: PathBuf,
}

impl Default for OwnedVms {
    fn default() -> Self {
        Self::new(
            crate::host::cua_home()
                .join("vmm")
                .join("lume")
                .join("owned"),
        )
    }
}

impl OwnedVms {
    /// Records in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The directory.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{name}.json"))
    }

    /// Records `name` as created by the SDK.
    pub fn mark(&self, name: &str, kind: OwnedKind, source: Option<&str>) {
        let rec = OwnedVm {
            name: name.into(),
            kind,
            source: source.map(str::to_string),
            created_at: crate::host::now_secs(),
            last_used: None,
        };
        let res = std::fs::create_dir_all(&self.dir)
            .and_then(|()| std::fs::write(self.file(name), serde_json::to_vec_pretty(&rec)?));
        if let Err(e) = res {
            tracing::debug!(vm = name, error = %e, "could not record the lume VM");
        }
    }

    /// Whether the SDK created `name`.
    pub fn owns(&self, name: &str) -> bool {
        self.file(name).exists()
    }

    /// Marks `name` as used now (no-op when not recorded).
    pub fn touch(&self, name: &str) {
        let f = self.file(name);
        if f.exists() {
            crate::disk::mark_used(&f);
        }
    }

    /// Forgets `name` (after deleting the VM).
    pub fn forget(&self, name: &str) {
        let _ = std::fs::remove_file(self.file(name));
    }

    /// One record.
    pub fn get(&self, name: &str) -> Option<OwnedVm> {
        let f = self.file(name);
        let mut r: OwnedVm = serde_json::from_slice(&std::fs::read(&f).ok()?).ok()?;
        r.last_used = crate::disk::last_used(&f);
        Some(r)
    }

    /// Every record.
    pub fn list(&self) -> Vec<OwnedVm> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return vec![];
        };
        let mut v: Vec<OwnedVm> = rd
            .flatten()
            .filter_map(|e| {
                let name = e
                    .file_name()
                    .to_string_lossy()
                    .strip_suffix(".json")?
                    .to_string();
                self.get(&name)
            })
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_touch_forget() {
        let d = tempfile::tempdir().unwrap();
        let o = OwnedVms::new(d.path());
        assert!(!o.owns("cua-base-abc"));
        o.mark(
            "cua-base-abc",
            OwnedKind::Base,
            Some("ghcr.io/trycua/macos:26"),
        );
        o.mark("box", OwnedKind::Instance, Some("cua-base-abc"));
        assert!(o.owns("box"));
        let l = o.list();
        assert_eq!(l.len(), 2);
        assert_eq!(l[1].kind, OwnedKind::Base);
        o.touch("box");
        o.forget("box");
        assert_eq!(o.list().len(), 1);
    }
}
