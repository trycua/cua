//! Copying checkpoints between your machines.
//!
//! A checkpoint (a sandbox's saved memory and disk that forks start from)
//! lives on the machine that took it. To fork it on another machine of the
//! pool, that machine needs a copy, and this crate makes the copy:
//!
//! - [`Manifest`]: the checkpoint as its files, each by SHA-256 and size,
//!   plus the runtime's state for it (with file paths replaced by
//!   [`placeholder`]s) and the sandbox record. The SHA-256 of the
//!   manifest's JSON is the checkpoint's digest.
//! - [`store::Store`]: a host's snapshot directory
//!   (`$CUA_HOME/snapshots`): files kept by content (`blobs/<sha256>`),
//!   partial downloads, manifests, export indexes and the grant key.
//! - [`grant`]: a short-lived read grant for one manifest's files, signed
//!   with the exporting host's key.
//! - `serve` (feature): the HTTP route a host serves the files of its
//!   exported checkpoints on (`GET /v1/snapshots/blobs/<sha256>`, with
//!   range requests), behind a grant.
//! - `pull` (feature): fetches the files a host does not have yet, from the
//!   hosts that have them (resuming a partial file where it stopped),
//!   checking every file's SHA-256 before it is kept, then from the
//!   fallback.
//! - `fallback` (feature): the S3 / Cua Volume copy, used when no host
//!   that has the files can be reached.

pub mod grant;
pub mod hash;
pub mod store;

#[cfg(feature = "fallback")]
pub mod fallback;
#[cfg(feature = "pull")]
pub mod pull;
#[cfg(feature = "serve")]
pub mod serve;

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The manifest format this crate writes.
pub const MANIFEST_VERSION: u32 = 1;

/// The URL path hosts serve snapshot files under.
pub const HTTP_BASE: &str = "/v1/snapshots";

/// What went wrong.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A file or directory could not be read or written.
    #[error("{0}")]
    Io(#[from] std::io::Error),
    /// A manifest or index could not be parsed.
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// The input is wrong.
    #[error("{0}")]
    Invalid(String),
    /// A grant is missing, expired or not valid for this file.
    #[error("{0}")]
    Denied(String),
    /// Not here.
    #[error("{0}")]
    NotFound(String),
    /// A file's content does not match its SHA-256.
    #[error("{0}")]
    Corrupt(String),
    /// A host or the fallback could not give the files.
    #[error("{0}")]
    Unavailable(String),
}

/// A `Result` with [`Error`].
pub type Result<T> = std::result::Result<T, Error>;

/// One file of a checkpoint, as a manifest lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Its id in the manifest (`f0`, `f1`, ...): what [`placeholder`]s in
    /// [`Manifest::vmm`] refer to.
    pub id: String,
    /// SHA-256 of its content (hex).
    pub sha256: String,
    /// Its size in bytes.
    pub size: u64,
    /// The directory (an id in [`Manifest::dirs`]) it must be in, with
    /// [`FileEntry::name`] (a runtime that finds it by name there), or
    /// `None` (anywhere).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    /// Its file name.
    pub name: String,
    /// A qcow2 image whose backing file is the file of this id: rebased onto
    /// the copy on the receiving host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub qcow2_backing: Option<String>,
    /// Unix permission bits (`0o600` for secrets).
    pub mode: u32,
}

/// A checkpoint as a set of files plus the state that ties them together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// [`MANIFEST_VERSION`].
    pub version: u32,
    /// The checkpoint's name.
    pub name: String,
    /// The engine that holds it (`qemu`, ...).
    pub runtime_type: String,
    /// Which hosts can restore it (`qemu/hvf/aarch64`, ...).
    pub abi: String,
    /// Its files, in a stable order.
    pub files: Vec<FileEntry>,
    /// Directory ids (`d0`, ...) files are grouped in, as [`placeholder`]s
    /// in [`Manifest::vmm`] may refer to directories too.
    #[serde(default)]
    pub dirs: Vec<String>,
    /// The runtime's state for the checkpoint, every file or directory
    /// path replaced by its [`placeholder`].
    pub vmm: serde_json::Value,
    /// The sandbox layer's checkpoint record (its template, guest token
    /// included).
    pub record: serde_json::Value,
}

/// How a manifest's runtime state names one of its files or directories.
pub fn placeholder(id: &str) -> String {
    format!("cua-snapshot://{id}")
}

/// The id a [`placeholder`] names, if `s` is one.
pub fn placeholder_id(s: &str) -> Option<&str> {
    s.strip_prefix("cua-snapshot://")
}

impl Manifest {
    /// The canonical JSON this manifest is stored and hashed as.
    pub fn to_bytes(&self) -> Vec<u8> {
        // Struct fields serialize in declaration order and `serde_json`
        // keeps `Value` maps sorted, so this is stable.
        serde_json::to_vec(self).expect("a manifest serializes")
    }

    /// Parses a manifest, refusing a format this crate does not know.
    pub fn from_bytes(b: &[u8]) -> Result<Manifest> {
        let m: Manifest = serde_json::from_slice(b)?;
        if m.version != MANIFEST_VERSION {
            return Err(Error::Invalid(format!(
                "snapshot manifest version {} (this cua reads {MANIFEST_VERSION}); update cua on both machines",
                m.version
            )));
        }
        m.validate()?;
        Ok(m)
    }

    /// The digest: SHA-256 of [`Manifest::to_bytes`] (hex).
    pub fn digest(&self) -> String {
        hex::encode(Sha256::digest(self.to_bytes()))
    }

    /// Bytes of all its files.
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Its distinct file contents (a file listed twice is copied once).
    pub fn blobs(&self) -> BTreeMap<String, u64> {
        self.files
            .iter()
            .map(|f| (f.sha256.clone(), f.size))
            .collect()
    }

    /// Checks that ids, names and hashes are well formed (a manifest from
    /// another machine is data: nothing in it may name a path outside the
    /// directory it is materialized in).
    pub fn validate(&self) -> Result<()> {
        let mut ids = std::collections::BTreeSet::new();
        for d in &self.dirs {
            if !ids.insert(d.clone()) || !valid_id(d) {
                return Err(Error::Invalid(format!("bad directory id {d:?}")));
            }
        }
        for f in &self.files {
            if !ids.insert(f.id.clone()) || !valid_id(&f.id) {
                return Err(Error::Invalid(format!("bad file id {:?}", f.id)));
            }
            if !valid_sha(&f.sha256) {
                return Err(Error::Invalid(format!("bad sha256 for {}", f.id)));
            }
            if f.name.is_empty()
                || f.name.len() > 200
                || f.name.contains('/')
                || f.name.contains('\\')
                || f.name == "."
                || f.name == ".."
                || f.name.contains('\0')
            {
                return Err(Error::Invalid(format!("bad file name {:?}", f.name)));
            }
            if let Some(d) = &f.dir
                && !self.dirs.contains(d)
            {
                return Err(Error::Invalid(format!(
                    "{} is in unknown directory {d}",
                    f.id
                )));
            }
        }
        for f in &self.files {
            if let Some(b) = &f.qcow2_backing
                && !self.files.iter().any(|g| &g.id == b)
            {
                return Err(Error::Invalid(format!(
                    "{} is backed by unknown file {b}",
                    f.id
                )));
            }
        }
        Ok(())
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 16 && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Whether `s` is a SHA-256 in lowercase hex.
pub fn valid_sha(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// A file the runtime says belongs to a checkpoint, before it is hashed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSpec {
    /// Its manifest id.
    pub id: String,
    /// Where it is on this host.
    pub path: PathBuf,
    /// The directory id it must be in, if any.
    pub dir: Option<String>,
    /// The id of its qcow2 backing file, if any.
    pub qcow2_backing: Option<String>,
}

/// Progress of a copy (or an upload), as `SnapshotSyncProgress` reports
/// it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// The checkpoint's name.
    pub name: String,
    /// Its digest.
    pub digest: String,
    /// Where the bytes come from now (a host's name, `s3`, `volume`).
    pub from: String,
    /// Bytes copied so far.
    pub bytes_done: u64,
    /// Bytes to copy in all.
    pub bytes_total: u64,
    /// Bytes this host had already.
    pub bytes_reused: u64,
    /// Finished.
    pub done: bool,
    /// Every file's SHA-256 matched.
    pub verified: bool,
}

/// Where progress reports go.
pub type ProgressFn = std::sync::Arc<dyn Fn(Progress) + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            version: MANIFEST_VERSION,
            name: "w0".into(),
            runtime_type: "qemu".into(),
            abi: "qemu/hvf/aarch64".into(),
            files: vec![
                FileEntry {
                    id: "f0".into(),
                    sha256: "a".repeat(64),
                    size: 10,
                    dir: Some("d0".into()),
                    name: "state.mig".into(),
                    qcow2_backing: None,
                    mode: 0o600,
                },
                FileEntry {
                    id: "f1".into(),
                    sha256: "b".repeat(64),
                    size: 5,
                    dir: None,
                    name: "disk.qcow2".into(),
                    qcow2_backing: Some("f0".into()),
                    mode: 0o644,
                },
            ],
            dirs: vec!["d0".into()],
            vmm: serde_json::json!({"disk": placeholder("f1"), "memory": {"dir": placeholder("d0")}}),
            record: serde_json::json!({"name": "w0"}),
        }
    }

    #[test]
    fn digest_is_stable_and_round_trips() {
        let m = manifest();
        let d = m.digest();
        assert!(valid_sha(&d));
        let back = Manifest::from_bytes(&m.to_bytes()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.digest(), d);
        let mut other = m.clone();
        other.files[0].size = 11;
        assert_ne!(other.digest(), d);
        assert_eq!(m.size(), 15);
        assert_eq!(placeholder_id(&placeholder("f1")), Some("f1"));
    }

    /// A manifest is data from another machine: a file name with a path in
    /// it, an unknown directory or backing, or a bad hash is refused.
    #[test]
    fn hostile_manifests_are_refused() {
        for bad in [
            |m: &mut Manifest| m.files[0].name = "../../etc/passwd".into(),
            |m: &mut Manifest| m.files[0].name = "..".into(),
            |m: &mut Manifest| m.files[0].dir = Some("d9".into()),
            |m: &mut Manifest| m.files[1].qcow2_backing = Some("f9".into()),
            |m: &mut Manifest| m.files[0].sha256 = "Z".repeat(64),
            |m: &mut Manifest| m.files[1].id = "f0".into(),
            |m: &mut Manifest| m.files[1].id = "../x".into(),
        ] {
            let mut m = manifest();
            bad(&mut m);
            let e = Manifest::from_bytes(&m.to_bytes()).unwrap_err();
            assert!(matches!(e, Error::Invalid(_)), "{e}");
        }
        let mut m = manifest();
        m.version = 2;
        assert!(
            Manifest::from_bytes(&m.to_bytes())
                .unwrap_err()
                .to_string()
                .contains("update cua")
        );
    }
}
