// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! WhatsApp (macOS) import: every App Group / container root lands back at
//! its home-relative location.

use std::io::Read;
use std::path::Path;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::layout::whatsapp::{launch_spec, remap, DISPLAY, ID};
use cua_teleport_bundle::{LaunchSpec, Platform};

use super::write_entry;
use crate::{ImportProvider, ImportRecord, Result};

/// The WhatsApp importer.
pub struct WhatsAppImporter {
    max_total_bytes: u64,
}

impl Default for WhatsAppImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl WhatsAppImporter {
    pub fn new() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl ImportProvider for WhatsAppImporter {
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
        _platform: Platform,
        record: &mut ImportRecord,
    ) -> Result<LaunchSpec> {
        let mut reader = BundleReader::open_with_limit(bundle, self.max_total_bytes)?;
        while let Some(entry) = reader.next_entry()? {
            if let Some(rel) = remap(&entry.rel_path) {
                write_entry(&dest_home.join(rel), &entry, false, record)?;
            }
        }
        Ok(launch_spec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::fixtures::bundle;
    use std::io::Cursor;

    #[test]
    fn import_restores_all_roots() {
        let bytes = bundle(
            "whatsapp",
            &[
                (
                    "group-shared/ChatStorage/db.sqlite",
                    0o600,
                    b"session+chats",
                ),
                ("group-private/state.plist", 0o600, b"companion"),
                ("container/app.state", 0o600, b"appstate"),
                ("unknown/ignored", 0o600, b"x"),
            ],
        );
        let dest = tempfile::tempdir().unwrap();
        let spec = WhatsAppImporter::new()
            .import_to(&mut Cursor::new(bytes), dest.path(), Platform::MacOS)
            .unwrap();
        assert_eq!(
            std::fs::read(dest.path().join(
                "Library/Group Containers/group.net.whatsapp.WhatsApp.shared/ChatStorage/db.sqlite"
            ))
            .unwrap(),
            b"session+chats"
        );
        assert!(dest
            .path()
            .join("Library/Group Containers/group.net.whatsapp.WhatsApp.private/state.plist")
            .is_file());
        assert!(dest
            .path()
            .join("Library/Containers/net.whatsapp.WhatsApp/Data/Library/Application Support/net.whatsapp.WhatsApp/app.state")
            .is_file());
        assert!(!dest.path().join("unknown").exists());
        assert_eq!(spec.program, "open");
        assert_eq!(spec.args, vec!["-a".to_string(), "WhatsApp".to_string()]);
    }
}
