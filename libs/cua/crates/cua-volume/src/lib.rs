// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Cua Volume: one drive per account, shared by every Space and agent in it.
//!
//! A persistent agent's memory, identity and outputs must outlive its Space:
//! a cloud Space is a Fleet claim whose disk goes back to the pool, so an
//! agent that keeps its memory on its own disk forgets everything on
//! release. The drive is where that state lives instead.
//!
//! ```text
//! public/             shared reference: read by everyone, written by the user
//! agents/<agent>/     one persistent agent's home (memory, outputs, inbox)
//! spaces/<space>/     per-Space scratch and outputs
//! ```
//!
//! | Module | What it owns |
//! |---|---|
//! | [`path`] | key normalization and the layout |
//! | [`acl`] | principals, the default access table, grants |
//! | [`backend`] | the object-store trait: versioned puts with preconditions |
//! | [`fs`] | the local backend (`$CUA_HOME/volume`), versioned on disk |
//! | `s3` | the S3-compatible backend (feature `s3`): AWS S3, R2, MinIO, and the Cua cloud's vended keys |
//! | [`config`] | which backend a cua home uses (`$CUA_HOME/volume/config.json`, `CUA_DRIVE_*`) |
//! | [`audit`] | the hash-chained audit log |
//! | [`scan`] | the secret scanner run on writes under `agents/` |
//! | [`drive`] | [`Drive`] and its per-principal [`Session`] |
//! | [`sync`] | copy-in / copy-out of a directory tree with a manifest |
//! | [`lease`] | one writer per agent home |
//! | [`cache`] | the size-capped LRU block cache in front of a remote backend (`$CUA_HOME/volume/cache`) |
//! | [`stream`] | ranged reads of a pinned version with sequential read-ahead |
//! | [`feed`] | the change feed in the bucket: devices learn each other's changes in seconds |
//! | [`vfs`] | the drive as a filesystem (inodes, spooled writes, conflict copies) |
//! | `nfs` | the localhost NFSv3 server macOS mounts (feature `nfs`) |
//! | `fuse` | the FUSE mount on Linux (feature `fuse`) |
//! | [`service`] | what a runtime runs: storage switching, the mount, sync status, cache stats |
//!
//! Enforcement happens here, on the host side, never in the guest: a
//! [`Session`] carries its principal, and every read, write and listing is
//! checked against the defaults and the live grants before the backend is
//! touched. Widening access past the defaults is a grant made by the user
//! with presence (Touch ID or passphrase).

pub mod acl;
pub mod audit;
pub mod backend;
pub mod cache;
pub mod config;
#[doc(hidden)]
pub mod conformance;
pub mod drive;
pub mod feed;
pub mod fs;
#[cfg(all(feature = "fuse", target_os = "linux"))]
pub mod fuse;
pub mod guest;
pub mod lease;
#[cfg(feature = "nfs")]
pub mod nfs;
pub mod path;
pub mod remote;
#[cfg(feature = "s3")]
pub mod s3;
pub mod scan;
pub mod service;
pub mod stream;
pub mod sync;
pub mod vfs;

pub use acl::{Context, Grant, Mode, Principal};
pub use backend::{Backend, Condition, ObjectMeta, VersionInfo};
pub use drive::{AccessRequest, Drive, Entry, Presence, Session};

/// Every failure of the drive.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// A malformed path, name or argument.
    #[error("invalid: {0}")]
    Invalid(String),
    /// No such object, version, grant or request.
    #[error("not found: {0}")]
    NotFound(String),
    /// The principal may not do this.
    #[error("forbidden: {0}")]
    Forbidden(String),
    /// An `if_etag` / create-only precondition failed.
    #[error("precondition failed: {0}")]
    Precondition(String),
    /// The write carries something that looks like a secret; it was not
    /// stored. Secrets belong in the Keyvault.
    #[error(
        "secret detected: {kind} on line {line} of {path}; store secrets in the Keyvault, not the drive"
    )]
    SecretDetected {
        path: String,
        kind: String,
        line: usize,
    },
    /// Another writer holds the agent home.
    #[error("the agent home is in use by {holder} until {expires_ms}")]
    LeaseHeld { holder: String, expires_ms: u64 },
    /// The user did not confirm (presence).
    #[error("not confirmed: {0}")]
    NotConfirmed(String),
    /// The object store failed.
    #[error("backend: {0}")]
    Backend(String),
}

impl Error {
    /// A stable tag for tool results (`structuredContent.error.kind`).
    pub fn tag(&self) -> &'static str {
        match self {
            Error::Invalid(_) => "invalid_argument",
            Error::NotFound(_) => "not_found",
            Error::Forbidden(_) => "forbidden",
            Error::Precondition(_) => "precondition_failed",
            Error::SecretDetected { .. } => "secret_detected",
            Error::LeaseHeld { .. } => "lease_held",
            Error::NotConfirmed(_) => "not_confirmed",
            Error::Backend(_) => "backend",
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::NotFound => Error::NotFound(e.to_string()),
            std::io::ErrorKind::PermissionDenied => Error::Forbidden(e.to_string()),
            _ => Error::Backend(e.to_string()),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Backend(format!("json: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Unix milliseconds now.
/// The volume's state directory under a cua home.
pub const STATE_DIR: &str = "volume";

/// Its name before the rename to Cua Volume.
pub const LEGACY_STATE_DIR: &str = "drive";

/// `<cua_home>/volume`: the volume's grants, audit, config, local store
/// (`data/`), cache and spools. A home from before the rename keeps its
/// `drive/` directory until this first runs, which moves it here (one
/// rename on the same file system; nothing is copied).
pub fn state_dir(cua_home: &std::path::Path) -> std::path::PathBuf {
    let dir = cua_home.join(STATE_DIR);
    let legacy = cua_home.join(LEGACY_STATE_DIR);
    if !dir.exists() && legacy.is_dir() {
        match std::fs::rename(&legacy, &dir) {
            Ok(()) => {
                tracing::info!(from = %legacy.display(), to = %dir.display(), "moved the volume's state")
            }
            Err(e) => {
                tracing::warn!(error = %e, "could not move {} to {}; using it where it is", legacy.display(), dir.display());
                return legacy;
            }
        }
    }
    dir
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Lowercase hex SHA-256.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// A short random id (grants, requests, versions).
pub fn new_id() -> String {
    format!("{:016x}", rand::random::<u64>())
}

#[cfg(test)]
mod state_dir_tests {
    #[test]
    fn a_home_from_before_the_rename_moves_its_state_once() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("drive/data/public")).unwrap();
        std::fs::write(home.path().join("drive/grants.json"), b"[]").unwrap();
        let dir = super::state_dir(home.path());
        assert_eq!(dir, home.path().join("volume"));
        assert!(dir.join("grants.json").is_file());
        assert!(dir.join("data/public").is_dir());
        assert!(!home.path().join("drive").exists());
        // Afterwards (or in a fresh home) nothing moves.
        std::fs::create_dir_all(home.path().join("drive")).unwrap();
        assert_eq!(super::state_dir(home.path()), dir);
        assert!(home.path().join("drive").exists());
    }
}
