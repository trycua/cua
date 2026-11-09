// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Anti-rollback anchor: a monotonic generation counter kept *outside* the
//! sealed vault files (red-team F5).
//!
//! The vault's `meta.sealed` carries a `generation` that is bumped on every
//! write, but nothing inside the vault directory can detect a whole-vault
//! rollback: an attacker with read/write to `~/.cua` (a backup, a synced home,
//! Time Machine) captures `meta.sealed` + `items/*`, lets the user revoke a
//! grant / trip the kill switch / spend a token, then restores the older set.
//! Because the KEK is unchanged the old files decrypt cleanly and the daemon
//! would accept the stale state (revoked grants live again, the kill switch
//! un-trips, spent uses refilled).
//!
//! The defence is a monotonic high-water mark held somewhere the rollback
//! cannot reach and cannot forge:
//!
//! - [`KeychainGenerationAnchor`] (macOS) keeps it in a dedicated login-keychain
//!   item, separate from the vault directory, so restoring the directory does
//!   not move it back. A Secure Enclave / TPM NV counter is the future upgrade.
//! - [`FileGenerationAnchor`] keeps it in a file the caller places outside the
//!   vault directory. Weaker (a wide enough restore can take it too), but it
//!   still catches partial restores and is the portable floor used in tests.
//!
//! On unlock the vault refuses to load state whose in-file generation is below
//! the anchor (fail closed); on every write it advances the anchor. The anchor
//! never moves backwards.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::Result;

/// A monotonic per-vault generation counter kept outside the sealed files.
pub trait GenerationAnchor: Send + Sync {
    /// The highest generation recorded for `vault_id` (0 if none yet).
    fn last(&self, vault_id: &str) -> Result<u64>;
    /// Advances the high-water mark to `generation`. Never moves it backwards:
    /// an implementation stores `max(existing, generation)`.
    fn record(&self, vault_id: &str, generation: u64) -> Result<()>;
}

/// A file-backed anchor. The file must live *outside* the vault directory (for
/// example next to it, or in a per-user state dir), or a directory-wide restore
/// rolls it back with the vault.
pub struct FileGenerationAnchor {
    path: PathBuf,
}

impl FileGenerationAnchor {
    /// An anchor stored at `path` (created 0600 on first write).
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load(&self) -> Result<BTreeMap<String, u64>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or_default()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.into()),
        }
    }
}

impl GenerationAnchor for FileGenerationAnchor {
    fn last(&self, vault_id: &str) -> Result<u64> {
        Ok(self.load()?.get(vault_id).copied().unwrap_or(0))
    }

    fn record(&self, vault_id: &str, generation: u64) -> Result<()> {
        let mut map = self.load()?;
        let slot = map.entry(vault_id.to_string()).or_insert(0);
        if generation > *slot {
            *slot = generation;
        }
        crate::store::write_private(&self.path, &serde_json::to_vec(&map)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_anchor_is_monotonic() {
        let d = tempfile::tempdir().unwrap();
        let a = FileGenerationAnchor::new(d.path().join("gen.json"));
        assert_eq!(a.last("v").unwrap(), 0);
        a.record("v", 5).unwrap();
        assert_eq!(a.last("v").unwrap(), 5);
        // Never moves backwards.
        a.record("v", 3).unwrap();
        assert_eq!(a.last("v").unwrap(), 5);
        a.record("v", 9).unwrap();
        assert_eq!(a.last("v").unwrap(), 9);
        // Independent per vault id.
        assert_eq!(a.last("other").unwrap(), 0);
    }
}
