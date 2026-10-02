// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Steam import: the login files and Steam Guard sentries land in the
//! destination platform's Steam root, so Steam auto-logs-in on launch.

use std::io::Read;
use std::path::Path;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::layout::steam::{launch_spec_for, remap, root_for, DISPLAY, ID};
use cua_teleport_bundle::{LaunchSpec, Platform};

use super::write_entry;
use crate::{ImportProvider, ImportRecord, Result};

/// The Steam importer.
pub struct SteamImporter {
    max_total_bytes: u64,
}

impl Default for SteamImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl SteamImporter {
    pub fn new() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl ImportProvider for SteamImporter {
    fn id(&self) -> &str {
        ID
    }

    fn display_name(&self) -> &str {
        DISPLAY
    }

    fn import_recorded(
        &self,
        bundle: &mut dyn Read,
        dest_home: &Path,
        platform: Platform,
        record: &mut ImportRecord,
    ) -> Result<LaunchSpec> {
        record.create_dir_all(&dest_home.join(root_for(platform)))?;
        let mut reader = BundleReader::open_with_limit(bundle, self.max_total_bytes)?;
        while let Some(entry) = reader.next_entry()? {
            if let Some(rel) = remap(&entry.rel_path, platform) {
                write_entry(&dest_home.join(rel), &entry, false, record)?;
            }
        }
        Ok(launch_spec_for(platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::fixtures::bundle;
    use std::io::Cursor;

    #[test]
    fn import_lands_macos_and_launches() {
        let bytes = bundle(
            "steam",
            &[
                ("steam/config/config.vdf", 0o600, b"cfg"),
                ("steam/config/loginusers.vdf", 0o600, b"users"),
                ("steam/registry.vdf", 0o600, b"reg"),
                ("steam/ssfn1234567890", 0o600, b"sentry"),
            ],
        );
        let dest = tempfile::tempdir().unwrap();
        let spec = SteamImporter::new()
            .import_to(&mut Cursor::new(bytes), dest.path(), Platform::MacOS)
            .unwrap();
        let sroot = dest.path().join("Library/Application Support/Steam");
        assert!(sroot.join("config/config.vdf").is_file());
        assert!(sroot.join("config/loginusers.vdf").is_file());
        assert!(sroot.join("registry.vdf").is_file());
        assert!(sroot.join("ssfn1234567890").is_file());
        assert_eq!(spec.program, "open");
        assert_eq!(spec.args, vec!["-a".to_string(), "Steam".to_string()]);
    }

    #[test]
    fn import_uses_the_linux_steam_root() {
        let bytes = bundle("steam", &[("steam/registry.vdf", 0o600, b"reg")]);
        let dest = tempfile::tempdir().unwrap();
        let spec = SteamImporter::new()
            .import_to(&mut Cursor::new(bytes), dest.path(), Platform::Linux)
            .unwrap();
        assert!(dest.path().join(".steam/steam/registry.vdf").is_file());
        assert_eq!(spec.program, "steam");
    }
}
