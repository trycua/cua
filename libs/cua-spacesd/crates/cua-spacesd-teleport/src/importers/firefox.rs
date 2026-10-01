// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Firefox import: the profile lands as a dedicated `cua.default-release`
//! profile, `profiles.ini` is pointed at it, and a `user.js` is written that
//! suppresses the first-run / what's-new / default-browser onboarding and
//! restores the previous session — so a launch opens straight into the
//! logged-in tabs. The same `user.js` sets `network.proxy.type = 5` (use the
//! system proxy) so the guest-wide hotspot proxy, when enabled, covers
//! Firefox too.

use std::io::Read;
use std::path::Path;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::layout::firefox::{
    default_profiles_ini, launch_program_for, root_for, BUNDLE_PREFIX, DEST_PROFILE, DISPLAY, ID,
    ONBOARDING_USER_JS, TABS_JSON,
};
use cua_teleport_bundle::{LaunchSpec, Platform, WindowRestore};

use super::{write_entry, write_new};
use crate::{ImportProvider, ImportRecord, Result};

/// The Firefox importer.
pub struct FirefoxImporter {
    max_total_bytes: u64,
}

impl Default for FirefoxImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl FirefoxImporter {
    pub fn new() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
        }
    }
}

impl ImportProvider for FirefoxImporter {
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
        let root = dest_home.join(root_for(platform));
        let profile_dir = root.join("Profiles").join(DEST_PROFILE);
        record.create_dir_all(&profile_dir)?;

        let mut reader = BundleReader::open_with_limit(bundle, self.max_total_bytes)?;
        let mut tab_urls: Vec<String> = Vec::new();
        while let Some(entry) = reader.next_entry()? {
            if entry.rel_path == TABS_JSON {
                tab_urls = serde_json::from_slice(&entry.bytes).unwrap_or_default();
                continue;
            }
            // Firefox keeps its own key database (key4.db); a keychain entry
            // or anything outside the profile prefix is ignored.
            let Some(within) = entry.rel_path.strip_prefix(&format!("{BUNDLE_PREFIX}/")) else {
                continue;
            };
            write_entry(&profile_dir.join(within), &entry, false, record)?;
        }

        // Session-restore fallback: a running source Firefox keeps the live
        // session only in `sessionstore-backups/recovery.jsonlz4`, so seed the
        // top-level `sessionstore.jsonlz4` from it when it wasn't captured.
        let top_ss = profile_dir.join("sessionstore.jsonlz4");
        if !top_ss.is_file() {
            let recovery = profile_dir.join("sessionstore-backups/recovery.jsonlz4");
            if recovery.is_file() {
                record.file_written(&top_ss);
                let _ = std::fs::copy(&recovery, &top_ss);
            }
        }

        // Onboarding-skip + proxy prefs, and make this the default profile so a
        // plain relaunch (Dock) also opens it.
        write_new(
            &profile_dir.join("user.js"),
            ONBOARDING_USER_JS.as_bytes(),
            record,
        )?;
        write_new(
            &root.join("profiles.ini"),
            default_profiles_ini().as_bytes(),
            record,
        )?;

        // Reopen the captured tabs deterministically as launch arguments (the
        // cookies/storage that landed above make them open logged-in), rather
        // than trusting Firefox's version-sensitive automatic session restore.
        let mut args = vec![
            "--profile".to_string(),
            profile_dir.to_string_lossy().into_owned(),
            "--no-remote".to_string(),
        ];
        args.extend(tab_urls.iter().cloned());

        Ok(LaunchSpec {
            program: launch_program_for(platform),
            args,
            env: Vec::new(),
            cwd: None,
            restore_windows: vec![WindowRestore {
                title: None,
                urls: tab_urls,
            }],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::importers::fixtures::bundle;
    use std::io::Cursor;

    fn firefox_bundle() -> Vec<u8> {
        bundle(
            "firefox",
            &[
                ("firefox/tabs.json", 0o644, br#"["https://mail.example/"]"#),
                ("firefox/cookies.sqlite", 0o600, b"SQLite format 3\0cookies"),
                ("firefox/key4.db", 0o600, b"SQLite format 3\0key"),
                (
                    "firefox/sessionstore-backups/recovery.jsonlz4",
                    0o600,
                    b"mozLz40\0recovery",
                ),
                (
                    "firefox/storage/default/https+++example.com/idb",
                    0o600,
                    b"idbdata",
                ),
            ],
        )
    }

    #[test]
    fn import_lands_macos_profile_and_skips_onboarding() {
        let dest = tempfile::tempdir().unwrap();
        let spec = FirefoxImporter::new()
            .import_to(
                &mut Cursor::new(firefox_bundle()),
                dest.path(),
                Platform::MacOS,
            )
            .unwrap();
        let prof = dest
            .path()
            .join("Library/Application Support/Firefox/Profiles")
            .join(DEST_PROFILE);
        assert_eq!(
            std::fs::read(prof.join("key4.db")).unwrap(),
            b"SQLite format 3\0key"
        );
        assert!(prof
            .join("storage/default/https+++example.com/idb")
            .is_file());
        // The recovery store seeded the top-level session store.
        assert_eq!(
            std::fs::read(prof.join("sessionstore.jsonlz4")).unwrap(),
            b"mozLz40\0recovery"
        );
        let user_js = std::fs::read_to_string(prof.join("user.js")).unwrap();
        assert!(user_js.contains("browser.aboutwelcome.enabled"));
        assert!(user_js.contains("\"browser.startup.page\", 3"));
        assert!(user_js.contains("network.proxy.type\", 5"));
        let ini = std::fs::read_to_string(
            dest.path()
                .join("Library/Application Support/Firefox/profiles.ini"),
        )
        .unwrap();
        assert!(ini.contains(&format!("Path=Profiles/{DEST_PROFILE}")));
        assert_eq!(
            spec.program,
            "/Applications/Firefox.app/Contents/MacOS/firefox"
        );
        assert!(spec.args.iter().any(|a| a.ends_with(DEST_PROFILE)));
        assert_eq!(spec.args.last().unwrap(), "https://mail.example/");
        assert!(!dest.path().join(".mozilla").exists());
    }

    #[test]
    fn import_uses_linux_layout_on_linux() {
        let dest = tempfile::tempdir().unwrap();
        let spec = FirefoxImporter::new()
            .import_to(
                &mut Cursor::new(firefox_bundle()),
                dest.path(),
                Platform::Linux,
            )
            .unwrap();
        assert!(dest
            .path()
            .join(".mozilla/firefox/Profiles")
            .join(DEST_PROFILE)
            .join("cookies.sqlite")
            .is_file());
        assert_eq!(spec.program, "firefox");
    }
}
