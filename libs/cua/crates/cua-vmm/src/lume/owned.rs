//! Lume VMs the SDK created.
//!
//! Lume keeps every VM in its own store (`~/.lume` by default), next to VMs
//! the user made with `lume` directly. The SDK records each VM it creates
//! (pulled bases, clones, Linux disk VMs, forks) as
//! `$CUA_HOME/vmm/lume/owned/<name>.json`, so `cua cache` can report them
//! and the cache garbage collection only ever removes VMs the SDK made. The
//! file's mtime is the VM's last use.
//!
//! A record also ties the name to the VM it was written for: the bundle
//! directory's on-disk identity (inode and birth time), the creating
//! process and a random nonce. A VM deleted outside the SDK and later
//! re-made under the same name (by the user, by `lume`) no longer matches,
//! so a record-driven delete never reaches it ([`OwnedVms::verify`]).

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
    /// The bundle directory's identity when it was recorded
    /// ([`bundle_identity`]); `None` in records older than identities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<String>,
    /// Who created it (`cua-sdk`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    /// The creating process.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// A random value unique to this creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

/// Whether a record still describes the VM on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ownership {
    /// The record was written for this very bundle.
    Matches,
    /// A record from before identities: matched by name only.
    NameOnly,
    /// The bundle on disk is another VM (re-made under the name): not ours.
    Replaced,
    /// No record, or no bundle.
    NotOwned,
}

/// The on-disk identity of a VM bundle directory: inode and birth time.
/// A directory deleted and made again under the same path differs.
pub fn bundle_identity(dir: &Path) -> Option<String> {
    let m = std::fs::metadata(dir).ok()?;
    if !m.is_dir() {
        return None;
    }
    #[cfg(unix)]
    let ino = std::os::unix::fs::MetadataExt::ino(&m);
    #[cfg(not(unix))]
    let ino = 0u64;
    let born = m
        .created()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    Some(format!("{ino}:{born}"))
}

fn nonce(name: &str) -> String {
    use sha2::{Digest, Sha256};
    let t = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let d = Sha256::digest(format!("{name}:{t}:{}", std::process::id()).as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
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

    /// Records `name` as created by the SDK (without the bundle's
    /// identity; prefer [`Self::mark_in`]).
    pub fn mark(&self, name: &str, kind: OwnedKind, source: Option<&str>) {
        self.write(name, kind, source, None);
    }

    /// Records `name`, whose bundle is `root/<name>`, as created by this
    /// process, with the bundle's identity.
    pub fn mark_in(&self, root: &Path, name: &str, kind: OwnedKind, source: Option<&str>) {
        self.write(name, kind, source, bundle_identity(&root.join(name)));
    }

    fn write(&self, name: &str, kind: OwnedKind, source: Option<&str>, identity: Option<String>) {
        let rec = OwnedVm {
            name: name.into(),
            kind,
            source: source.map(str::to_string),
            created_at: crate::host::now_secs(),
            last_used: None,
            identity,
            creator: Some("cua-sdk".into()),
            pid: Some(std::process::id()),
            nonce: Some(nonce(name)),
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

    /// Whether the record for `name` describes the bundle at `root/<name>`.
    pub fn verify(&self, root: &Path, name: &str) -> Ownership {
        let Some(rec) = self.get(name) else {
            return Ownership::NotOwned;
        };
        let Some(now) = bundle_identity(&root.join(name)) else {
            return Ownership::NotOwned;
        };
        match rec.identity {
            None => Ownership::NameOnly,
            Some(id) if id == now => Ownership::Matches,
            Some(_) => Ownership::Replaced,
        }
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

    /// A record matches the bundle it was written for, not one re-made
    /// under the same name, and never a VM it has no record of.
    #[test]
    fn verify_ties_a_record_to_its_bundle() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("lume");
        let o = OwnedVms::new(d.path().join("owned"));
        std::fs::create_dir_all(root.join("space-0123456789")).unwrap();
        assert_eq!(o.verify(&root, "space-0123456789"), Ownership::NotOwned);
        std::fs::create_dir_all(root.join("box")).unwrap();
        o.mark_in(&root, "box", OwnedKind::Instance, None);
        let rec = o.get("box").unwrap();
        assert!(rec.nonce.is_some() && rec.pid == Some(std::process::id()));
        assert_eq!(o.verify(&root, "box"), Ownership::Matches);
        // Deleted outside the SDK and made again under the same name.
        std::fs::remove_dir_all(root.join("box")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(root.join("box")).unwrap();
        assert_eq!(o.verify(&root, "box"), Ownership::Replaced);
        std::fs::remove_dir_all(root.join("box")).unwrap();
        assert_eq!(o.verify(&root, "box"), Ownership::NotOwned);
        // A record from before identities matches by name only.
        std::fs::create_dir_all(root.join("old")).unwrap();
        o.mark("old", OwnedKind::Base, None);
        assert_eq!(o.verify(&root, "old"), Ownership::NameOnly);
    }
}
