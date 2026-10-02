// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Built-in [`crate::ImportProvider`] implementations, one per app of the
//! shared layout table ([`cua_teleport_bundle::layout`]).

pub mod chrome;
pub mod claude_code;
pub mod electron;
pub mod firefox;
pub mod steam;
pub mod whatsapp;

use std::path::Path;

use cua_teleport_bundle::bundle::VerifiedEntry;

use crate::ledger::ImportRecord;
use crate::Result;

/// Write one verified bundle entry to `dest`, creating parent directories.
/// Unix permissions come from the entry (0600 when it carries none, or when
/// `owner_only` is set — credential blobs are always tightened). The file and
/// any directory created for it are reported in `record`.
pub(crate) fn write_entry(
    dest: &Path,
    entry: &VerifiedEntry,
    owner_only: bool,
    record: &mut ImportRecord,
) -> Result<()> {
    if let Some(parent) = dest.parent() {
        record.create_dir_all(parent)?;
    }
    // Recorded before the write: a failure part way through still leaves a
    // path for the cleanup to remove.
    record.file_written(dest);
    std::fs::write(dest, &entry.bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if owner_only || entry.mode & 0o777 == 0 {
            0o600
        } else {
            entry.mode & 0o777
        };
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = owner_only;
    Ok(())
}

/// Write `bytes` to `dest`, creating parent directories, at mode 0600 (unix).
/// Used by imports that synthesize a file (profiles.ini, user.js) rather than
/// copying one from the bundle.
pub(crate) fn write_new(dest: &Path, bytes: &[u8], record: &mut ImportRecord) -> Result<()> {
    if let Some(parent) = dest.parent() {
        record.create_dir_all(parent)?;
    }
    record.file_written(dest);
    std::fs::write(dest, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Test fixtures: bundles written with the shared [`BundleWriter`], exactly
/// as the sender lays them out, so importer tests never need the sender.
///
/// [`BundleWriter`]: cua_teleport_bundle::bundle::BundleWriter
#[cfg(test)]
pub(crate) mod fixtures {
    use cua_teleport_bundle::bundle::BundleWriter;
    use cua_teleport_bundle::TransferScope;

    /// A bundle for `provider_id` holding `entries` (`rel_path`, mode, bytes).
    pub fn bundle(provider_id: &str, entries: &[(&str, u32, &[u8])]) -> Vec<u8> {
        let mut writer = BundleWriter::new(
            Vec::new(),
            provider_id,
            provider_id,
            TransferScope::FullProfile,
        );
        for (rel, mode, bytes) in entries {
            writer.add_bytes(*rel, *mode, bytes).unwrap();
        }
        writer.finish().unwrap()
    }
}
