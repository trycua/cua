// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Electron / Chromium-app import (Slack, Discord, Unity Hub).
//!
//! Stops a running instance first, lands the profile in the destination
//! platform's app-support dir (and `home/…` payloads relative to the home),
//! reinstalls the carried Keychain items (the "Safe Storage" key and any
//! auth-token items), and relaunches the app.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::layout::electron::{
    launch_spec_for, process_pattern, profile_dir, remap, ElectronApp, DISCORD, SLACK, UNITY_HUB,
};
use cua_teleport_bundle::{LaunchSpec, Platform};

use super::write_entry;
use crate::host::{default_host, HostEffects};
use crate::keychain;
use crate::{ImportProvider, ImportRecord, Result};

/// An importer bound to one [`ElectronApp`].
pub struct ElectronImporter {
    app: ElectronApp,
    max_total_bytes: u64,
    /// Stopping the running app and every Keychain write go through this.
    host: Arc<dyn HostEffects>,
}

impl ElectronImporter {
    pub fn new(app: ElectronApp) -> Self {
        Self {
            app,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Slack.
    pub fn slack() -> Self {
        Self::new(SLACK)
    }

    /// Discord.
    pub fn discord() -> Self {
        Self::new(DISCORD)
    }

    /// Unity Hub.
    pub fn unity_hub() -> Self {
        Self::new(UNITY_HUB)
    }
}

impl ImportProvider for ElectronImporter {
    fn id(&self) -> &str {
        self.app.id
    }

    fn display_name(&self) -> &str {
        self.app.display
    }

    fn import_recorded(
        &self,
        bundle: &mut dyn Read,
        dest_home: &Path,
        platform: Platform,
        record: &mut ImportRecord,
    ) -> Result<LaunchSpec> {
        // Open (and so size-check) the bundle before touching the running app.
        let mut reader = BundleReader::open_with_limit(bundle, self.max_total_bytes)?;

        // Stop any running instance BEFORE touching its profile. Two reasons,
        // both of which silently ruin a teleport:
        //  * a running Electron app rewrites its profile as it exits, so it can
        //    clobber the accounts database we just imported;
        //  * the launch below is `open -a`, which merely focuses an app that is
        //    already running. The destination then keeps the signed-out session
        //    it started with.
        if let Some(pattern) = process_pattern(platform, self.app.display) {
            super::terminate_running(&*self.host, &pattern);
        }

        record.create_dir_all(&dest_home.join(profile_dir(&self.app, platform)))?;
        while let Some(entry) = reader.next_entry()? {
            if entry.rel_path == keychain::KEYCHAIN_ENTRY {
                keychain::install_all_recorded(&*self.host, &entry.bytes, record);
                continue;
            }
            if let Some(rel) = remap(&self.app, &entry.rel_path, platform) {
                write_entry(&dest_home.join(rel), &entry, false, record)?;
            }
        }
        Ok(launch_spec_for(platform, self.app.display))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::{EffectKind, FakeHost, HostOutput};
    use crate::importers::fixtures::bundle;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn slack_bundle() -> Vec<u8> {
        bundle(
            "slack",
            &[
                ("electron/Cookies", 0o600, b"SQLite format 3\0cookies"),
                ("electron/Local State", 0o600, b"{}"),
                ("electron/Local Storage/leveldb/000003.log", 0o600, b"token"),
            ],
        )
    }

    #[test]
    fn import_lands_the_macos_profile_and_launches_with_open() {
        let dest = tempfile::tempdir().unwrap();
        let host = Arc::new(FakeHost::new());
        let spec = ElectronImporter::discord()
            .with_host(host.clone())
            .import_to(
                &mut Cursor::new(bundle(
                    "discord",
                    &[("electron/Cookies", 0o600, b"SQLite format 3\0cookies")],
                )),
                dest.path(),
                Platform::MacOS,
            )
            .unwrap();
        let prof = dest.path().join("Library/Application Support/discord");
        assert_eq!(
            std::fs::read(prof.join("Cookies")).unwrap(),
            b"SQLite format 3\0cookies"
        );
        assert_eq!(spec.program, "open");
        assert_eq!(spec.args, vec!["-a".to_string(), "Discord".to_string()]);
        assert!(!dest.path().join(".config").exists());
        // Nothing was running, so nothing was stopped.
        assert!(host.calls_of(EffectKind::ProcessTerminate).is_empty());
    }

    #[test]
    fn import_uses_linux_layout_and_binary() {
        let dest = tempfile::tempdir().unwrap();
        let spec = ElectronImporter::slack()
            .with_host(Arc::new(FakeHost::new()))
            .import_to(
                &mut Cursor::new(slack_bundle()),
                dest.path(),
                Platform::Linux,
            )
            .unwrap();
        assert!(dest.path().join(".config/Slack/Cookies").is_file());
        assert!(dest
            .path()
            .join(".config/Slack/Local Storage/leveldb/000003.log")
            .is_file());
        assert_eq!(spec.program, "slack");
    }

    /// Unity Hub's `home/…` payloads (UVCS credentials) land relative to the
    /// destination home, not in the profile dir.
    #[test]
    fn unity_hub_home_payloads_land_in_the_home() {
        let dest = tempfile::tempdir().unwrap();
        ElectronImporter::unity_hub()
            .with_host(Arc::new(FakeHost::new()))
            .import_to(
                &mut Cursor::new(bundle(
                    "unity-hub",
                    &[
                        ("electron/accounts.db", 0o600, b"SQLite format 3\0accounts"),
                        ("home/.plastic4/tokens.conf", 0o600, b"uvcs-token"),
                    ],
                )),
                dest.path(),
                Platform::MacOS,
            )
            .unwrap();
        assert_eq!(
            std::fs::read(dest.path().join(".plastic4/tokens.conf")).unwrap(),
            b"uvcs-token"
        );
        let prof = dest.path().join("Library/Application Support/UnityHub");
        assert!(prof.join("accounts.db").is_file());
        assert!(!prof.join(".plastic4").exists());
    }

    /// Importing a Slack bundle stops a running Slack first. That must go
    /// through the injected host: the fake reports Slack running once, then
    /// gone, and records the TERM — the real `pkill -f slack` never runs.
    #[test]
    fn import_stops_the_running_app_only_through_the_injected_host() {
        let lookups = Arc::new(AtomicUsize::new(0));
        let seen = lookups.clone();
        let host = Arc::new(FakeHost::new().with_responder(move |command| {
            Ok(match command.kind {
                // Running on the first lookup, exited after the TERM.
                EffectKind::ProcessLookup if seen.fetch_add(1, Ordering::SeqCst) == 0 => {
                    HostOutput::ok("")
                }
                _ => HostOutput::failed(),
            })
        }));
        let dest = tempfile::tempdir().unwrap();
        ElectronImporter::slack()
            .with_host(host.clone())
            .import_to(
                &mut Cursor::new(slack_bundle()),
                dest.path(),
                Platform::Linux,
            )
            .unwrap();
        let terminations = host.calls_of(EffectKind::ProcessTerminate);
        assert_eq!(terminations.len(), 1, "{:?}", host.calls());
        assert_eq!(terminations[0].program, "pkill");
        assert_eq!(terminations[0].args, ["-TERM", "-f", "slack"]);
        assert_eq!(lookups.load(Ordering::SeqCst), 2);
    }

    /// A carried keychain entry is installed through the host (a no-op off
    /// macOS) and never written to disk as a file.
    #[test]
    fn keychain_entry_is_installed_not_written() {
        let items = cua_teleport_bundle::keychain::serialize(&[keychain::KeychainItem {
            service: "Slack Safe Storage".into(),
            account: "Slack Key".into(),
            secret: b"k".to_vec(),
            trust_app: None,
        }]);
        let dest = tempfile::tempdir().unwrap();
        // Keychain commands succeed; no Slack is running.
        let host = Arc::new(FakeHost::new().with_home(dest.path()).with_responder(|c| {
            Ok(match c.kind {
                EffectKind::ProcessLookup => HostOutput::failed(),
                _ => HostOutput::ok(""),
            })
        }));
        ElectronImporter::slack()
            .with_host(host.clone())
            .import_to(
                &mut Cursor::new(bundle("slack", &[("keychain.json", 0o600, &items)])),
                dest.path(),
                Platform::current(),
            )
            .unwrap();
        assert!(!dest.path().join("keychain.json").exists());
        let adds = host
            .calls_of(EffectKind::KeychainWrite)
            .into_iter()
            .filter(keychain::is_add_command)
            .count();
        assert_eq!(adds, usize::from(cfg!(target_os = "macos")));
    }
}
