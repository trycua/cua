// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Claude Code import: credentials, config and transcripts land in the guest
//! home (credentials always 0600) and a terminal running `claude` opens.

use std::io::Read;
use std::path::Path;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::layout::claude_code::{
    is_credentials, launch_terminal_spec, remap, DISPLAY, ID,
};
use cua_teleport_bundle::{LaunchSpec, Platform};

use super::write_entry;
use crate::{ImportProvider, ImportRecord, Result};

/// The Claude Code importer.
pub struct ClaudeCodeImporter {
    max_total_bytes: u64,
}

impl Default for ClaudeCodeImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ClaudeCodeImporter {
    pub fn new() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl ImportProvider for ClaudeCodeImporter {
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
        record.create_dir_all(&dest_home.join(".claude"))?;
        while let Some(entry) = reader.next_entry()? {
            // An unexpected entry path is ignored rather than fatal.
            let Some(rel) = remap(&entry.rel_path) else {
                continue;
            };
            write_entry(
                &dest_home.join(rel),
                &entry,
                is_credentials(&entry.rel_path),
                record,
            )?;
        }
        Ok(launch_terminal_spec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::fixtures::bundle;
    use std::io::Cursor;

    #[test]
    fn import_lands_files_and_locks_credentials() {
        let bytes = bundle(
            "claude-code",
            &[
                (
                    "claude/.credentials.json",
                    0o644,
                    br#"{"claudeAiOauth":{"accessToken":"fixture-access"}}"#,
                ),
                ("claude/.claude.json", 0o644, br#"{"onboarding":true}"#),
                ("claude/projects/-a/session.jsonl", 0o600, b"{}\n"),
            ],
        );
        let dest = tempfile::tempdir().unwrap();
        let spec = ClaudeCodeImporter::new()
            .import_to(&mut Cursor::new(bytes), dest.path(), Platform::Linux)
            .unwrap();
        assert_eq!(
            std::fs::read(dest.path().join(".claude/.credentials.json")).unwrap(),
            br#"{"claudeAiOauth":{"accessToken":"fixture-access"}}"#
        );
        assert_eq!(
            std::fs::read(dest.path().join(".claude.json")).unwrap(),
            br#"{"onboarding":true}"#
        );
        assert!(dest
            .path()
            .join(".claude/projects/-a/session.jsonl")
            .is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &str| {
                std::fs::metadata(dest.path().join(p))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777
            };
            // Tightened even though the bundle said 0644.
            assert_eq!(mode(".claude/.credentials.json"), 0o600);
            assert_eq!(mode(".claude.json"), 0o644);
        }
        assert_eq!(spec.program, "bash");
        assert!(spec.args.iter().any(|a| a.contains("xfce4-terminal")));
        assert!(spec.env.iter().any(|(k, v)| k == "DISPLAY" && v == ":1"));
    }
}
