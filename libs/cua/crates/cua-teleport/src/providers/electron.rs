// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Electron / Chromium-app session export (Slack, Discord, Unity Hub).
//!
//! These apps keep a logged-in session in a Chromium profile: the auth token in
//! `Local Storage/leveldb`, session cookies in `Cookies`, and — on macOS — those
//! are encrypted with an app-specific **"<App> Safe Storage"** key held in the
//! login Keychain (the wrapped key is in `Local State`). So a session that opens
//! straight into the logged-in app requires copying the curated profile subset
//! AND carrying that Keychain key (see [`crate::keychain`]); the receiver
//! reinstalls it on the destination.
//!
//! One [`ElectronApp`] descriptor ([`layout::electron`]) drives both halves;
//! register one provider per app.

use std::collections::HashSet;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
use crate::host::{HostEffects, default_host};
use crate::keychain;
pub use crate::layout::electron::ElectronApp;
use crate::layout::electron::{
    DISCORD, PROFILE_DIRS, PROFILE_FILES, SLACK, UNITY_HUB, app_support_root, home_rel, rel,
};
use crate::providers::util::{add_dir_recursive, add_file_best_effort, dir_len, file_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope,
};

/// A provider bound to one [`ElectronApp`].
pub struct ElectronProvider {
    app: ElectronApp,
    max_total_bytes: u64,
    /// Whether the macOS Keychain Safe Storage read is attempted. Disabled by
    /// [`Self::without_keychain`] so tests never touch the real Keychain.
    allow_keychain: bool,
    /// Test/override hook: the home the source profile is resolved under.
    home_override: Option<PathBuf>,
    /// Every Keychain, `$HOME` and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl ElectronProvider {
    pub fn new(app: ElectronApp) -> Self {
        Self {
            app,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            allow_keychain: true,
            home_override: None,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// The home the source profile is resolved under.
    fn source_home(&self) -> Option<PathBuf> {
        self.home_override.clone().or_else(|| self.host.home_dir())
    }

    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Disable the macOS Keychain Safe Storage read (tests stay hermetic — no
    /// `security` call, no access prompt).
    pub fn without_keychain(mut self) -> Self {
        self.allow_keychain = false;
        self
    }

    /// Override the home the source profile is resolved under (tests).
    pub fn with_home(mut self, home: impl Into<PathBuf>) -> Self {
        self.home_override = Some(home.into());
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

    /// Unity Hub (also carries the Unity Version Control credentials and the
    /// Editor entitlement from `$HOME`; see [`UNITY_HUB`]).
    pub fn unity_hub() -> Self {
        Self::new(UNITY_HUB)
    }

    /// Source profile directory on this host (the export side runs on macOS).
    fn source_dir(&self, platform: Platform) -> Option<PathBuf> {
        let home = self.source_home()?;
        Some(
            home.join(app_support_root(platform))
                .join(self.app.support_dir),
        )
    }
}

impl ExportProvider for ElectronProvider {
    fn id(&self) -> &str {
        self.app.id
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        self.app.display
    }

    fn platform_supported(&self, _platform: Platform) -> bool {
        true
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        // The Electron apps we transfer all install as `/Applications/<display>.app`
        // (Slack, Discord, "Unity Hub"), so derive the probe from the display name
        // rather than carrying a separate field per app.
        Some(crate::InstallProbe::path(self.app.macos_app()))
    }

    fn matches(&self, app: &AppRef) -> bool {
        self.app
            .app_ids
            .iter()
            .any(|c| c.eq_ignore_ascii_case(&app.app_id))
    }

    fn app_ids(&self) -> &[&str] {
        self.app.app_ids
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&crate::WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let dir = self.source_dir(app.platform);
        let mut items = Vec::new();
        let mut total = 0u64;

        if let Some(dir) = &dir {
            for (name, sensitive) in PROFILE_FILES {
                let disk = dir.join(name);
                if disk.is_file() {
                    let bytes = file_len(&disk);
                    total += bytes;
                    items.push(item(pretty(name), &rel(name), bytes, *sensitive, true));
                }
            }
            for (name, sensitive) in PROFILE_DIRS {
                let disk = dir.join(name);
                if disk.is_dir() {
                    let bytes = dir_len(&disk);
                    total += bytes;
                    // IndexedDB is the app's cached content (bulky) — opt-in; the
                    // token/session (Local Storage) + cookies default on.
                    let checked = *name != "IndexedDB";
                    items.push(item(pretty(name), &rel(name), bytes, *sensitive, checked));
                }
            }
            for name in self.app.extra {
                let disk = dir.join(name);
                if disk.is_dir() {
                    let bytes = dir_len(&disk);
                    total += bytes;
                    items.push(item(pretty(name), &rel(name), bytes, false, true));
                } else if disk.is_file() {
                    let bytes = file_len(&disk);
                    total += bytes;
                    items.push(item(pretty(name), &rel(name), bytes, false, true));
                }
            }
        }

        // `$HOME`-relative payloads. These are credentials (Unity Version
        // Control tokens), so they're marked sensitive in the consent UI.
        if let Some(home) = self.source_home() {
            for name in self.app.home_extra {
                let disk = home.join(name);
                let bytes = if disk.is_dir() {
                    dir_len(&disk)
                } else if disk.is_file() {
                    file_len(&disk)
                } else {
                    continue;
                };
                total += bytes;
                items.push(item(pretty(name), &home_rel(name), bytes, true, true));
            }
        }

        // The Safe Storage key rides along as a synthetic sensitive item so the
        // consent UI shows it; it's captured from the Keychain, not a file.
        if !self.app.keychain_services.is_empty() {
            items.push(item(
                "App encryption key (Keychain)",
                keychain::KEYCHAIN_ENTRY,
                0,
                true,
                true,
            ));
        }

        Ok(TransferManifest {
            provider_id: self.app.id.to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes: total,
            notes: vec![
                format!(
                    "Closing {} before transfer yields a cleaner capture.",
                    self.app.display
                ),
                "Includes the logged-in session token, cookies, and the app's \
                 macOS Keychain encryption key."
                    .to_string(),
            ],
        })
    }

    fn capture_selected(
        &self,
        app: &AppRef,
        _scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        let dir = self.source_dir(app.platform).ok_or_else(|| {
            TeleportError::Provider(format!(
                "no {} profile found on this machine",
                self.app.display
            ))
        })?;
        let mut writer = BundleWriter::with_limit(
            out,
            self.app.id,
            app.display_name.clone(),
            _scope,
            self.max_total_bytes,
        );
        let wants = |r: &str| include.is_none_or(|set| set.contains(r));

        for (name, _s) in PROFILE_FILES {
            if wants(&rel(name)) {
                let disk = dir.join(name);
                if disk.is_file() {
                    add_file_best_effort(&mut writer, &disk, &rel(name))?;
                }
            }
        }
        for (name, _s) in PROFILE_DIRS {
            if wants(&rel(name)) {
                let disk = dir.join(name);
                if disk.is_dir() {
                    add_dir_recursive(&mut writer, &disk, &rel(name))?;
                }
            }
        }
        for name in self.app.extra {
            if wants(&rel(name)) {
                let disk = dir.join(name);
                if disk.is_dir() {
                    add_dir_recursive(&mut writer, &disk, &rel(name))?;
                } else if disk.is_file() {
                    add_file_best_effort(&mut writer, &disk, &rel(name))?;
                }
            }
        }

        // `$HOME`-relative payloads (e.g. Unity Hub's `~/.plastic4` UVCS creds).
        if let Some(home) = self.source_home() {
            for name in self.app.home_extra {
                if wants(&home_rel(name)) {
                    let disk = home.join(name);
                    if disk.is_dir() {
                        add_dir_recursive(&mut writer, &disk, &home_rel(name))?;
                    } else if disk.is_file() {
                        add_file_best_effort(&mut writer, &disk, &home_rel(name))?;
                    }
                }
            }
        }

        // Keychain items from the macOS export side (the Safe Storage key that
        // decrypts cookies, plus any auth-token item like Unity's "unity"
        // service), packed as the reserved keychain entry so import reinstalls
        // every one under its original service+account.
        if self.allow_keychain && wants(keychain::KEYCHAIN_ENTRY) {
            // Trust the app's macOS bundle on the destination so it reads these
            // items silently (a `security`-added item isn't in the app's ACL).
            let trust = self.app.macos_app();
            let mut items = Vec::new();
            for service in self.app.keychain_services {
                match keychain::probe_generic(&*self.host, service, None) {
                    keychain::KeychainRead::Read(mut item) => {
                        item.trust_app = Some(trust.clone());
                        items.push(item);
                    }
                    keychain::KeychainRead::Absent => {}
                    // The secret is on this machine but macOS would not release
                    // it. Say so loudly: the bundle still transfers, and the
                    // destination then looks signed in while being unable to
                    // authenticate (Unity Hub lands with its account but no
                    // license), which is indistinguishable from a broken image
                    // unless we name the cause.
                    keychain::KeychainRead::Unreadable => {
                        eprintln!(
                            "warning: {} keychain item \"{}\" exists but could not be read, so it \
                             is NOT being teleported — {} will arrive signed out. Re-run the \
                             teleport and approve the Keychain prompt (choose \"Always Allow\" to \
                             make later teleports silent).",
                            self.app.display, service, self.app.display
                        );
                    }
                }
            }
            if !items.is_empty() {
                writer.add_bytes(
                    keychain::KEYCHAIN_ENTRY,
                    0o600,
                    &keychain::serialize(&items),
                )?;
            }
        }

        writer.finish()?;
        Ok(())
    }
}

fn pretty(name: &str) -> &str {
    match name {
        "Cookies" => "Cookies and session",
        "Cookies-journal" => "Cookies journal",
        "Local State" => "Local state (encryption key ref)",
        "Local Storage" => "Local storage (auth token)",
        "Session Storage" => "Session storage",
        "IndexedDB" => "Cached content",
        "Preferences" => "Preferences",
        "storage" => "App storage",
        ".plastic4/tokens.conf" => "Unity Version Control sign-in token",
        ".plastic4/client.conf" => "Unity Version Control client config",
        ".plastic4/unityorgs.conf" => "Unity organizations",
        ".plastic4/cloudregions.conf" => "Unity Version Control cloud regions",
        ".plastic4/profiles.conf" => "Unity Version Control profiles",
        other => other,
    }
}

fn item(
    label: &str,
    rel_path: &str,
    est_bytes: u64,
    sensitive: bool,
    default_checked: bool,
) -> ManifestItem {
    ManifestItem {
        label: label.to_string(),
        rel_path: rel_path.to_string(),
        est_bytes,
        count: None,
        count_noun: None,
        sensitive,
        default_checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use std::io::Cursor;

    fn app(id: &str) -> AppRef {
        AppRef {
            app_id: id.into(),
            display_name: id.into(),
            platform: Platform::MacOS,
        }
    }

    /// A fake Slack/Discord profile under a temp home.
    fn fake_home(support_dir: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let prof = dir
            .path()
            .join("Library/Application Support")
            .join(support_dir);
        std::fs::create_dir_all(prof.join("Local Storage/leveldb")).unwrap();
        std::fs::create_dir_all(prof.join("IndexedDB/x.indexeddb.leveldb")).unwrap();
        std::fs::write(prof.join("Cookies"), b"SQLite format 3\0cookies").unwrap();
        std::fs::write(
            prof.join("Local State"),
            br#"{"os_crypt":{"encrypted_key":"AAA"}}"#,
        )
        .unwrap();
        std::fs::write(prof.join("Preferences"), b"{}").unwrap();
        std::fs::write(
            prof.join("Local Storage/leveldb/CURRENT"),
            b"MANIFEST-000001\n",
        )
        .unwrap();
        std::fs::write(
            prof.join("Local Storage/leveldb/000003.log"),
            b"token-bytes",
        )
        .unwrap();
        std::fs::write(
            prof.join("IndexedDB/x.indexeddb.leveldb/CURRENT"),
            b"MANIFEST\n",
        )
        .unwrap();
        dir
    }

    /// A fake Unity Hub profile plus the `~/.plastic4` UVCS credentials that
    /// live in `$HOME`, not under the app-support dir.
    fn fake_unity_home() -> tempfile::TempDir {
        let dir = fake_home("UnityHub");
        let prof = dir.path().join("Library/Application Support/UnityHub");
        std::fs::write(prof.join("accounts.db"), b"SQLite format 3\0accounts").unwrap();
        let plastic = dir.path().join(".plastic4");
        std::fs::create_dir_all(&plastic).unwrap();
        std::fs::write(plastic.join("tokens.conf"), b"uvcs-token").unwrap();
        std::fs::write(plastic.join("client.conf"), b"<config/>").unwrap();
        dir
    }

    /// Unity Hub can only clone a project from Unity Version Control on the
    /// destination if the teleport carries `~/.plastic4` — a `$HOME`-relative
    /// payload, packed under the `home/` prefix (the receiver restores it next
    /// to the destination home rather than in the profile dir).
    #[test]
    fn unity_hub_carries_uvcs_credentials_as_home_payloads() {
        let home = fake_unity_home();
        std::fs::create_dir_all(home.path().join(".plastic4/logs")).unwrap();
        std::fs::write(home.path().join(".plastic4/logs/big.log"), b"log").unwrap();
        let provider = ElectronProvider::unity_hub()
            .with_home(home.path())
            .without_keychain();

        let m = provider
            .manifest(&app("Unity Hub"), None, TransferScope::FullProfile)
            .unwrap();
        let creds = m
            .items
            .iter()
            .find(|i| i.rel_path == "home/.plastic4/tokens.conf")
            .expect("UVCS credentials listed in the manifest");
        assert!(creds.sensitive, "UVCS credentials must be marked sensitive");
        assert_eq!(creds.label, "Unity Version Control sign-in token");

        let mut bundle = Vec::new();
        provider
            .export(&app("Unity Hub"), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let entries = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .read_all()
            .unwrap();
        let tokens = entries
            .iter()
            .find(|e| e.rel_path == "home/.plastic4/tokens.conf")
            .expect("token carried");
        assert_eq!(tokens.bytes, b"uvcs-token");
        assert!(entries.iter().any(|e| e.rel_path == "electron/accounts.db"));
        // Bulky logs are never carried — they dwarfed the credentials.
        assert!(
            !entries.iter().any(|e| e.rel_path.contains("logs")),
            "logs must not be carried"
        );
    }

    #[test]
    fn matches_slack_and_discord_ids() {
        assert!(ElectronProvider::slack().matches(&app("com.tinyspeck.slackmacgap")));
        assert!(ElectronProvider::slack().matches(&app("Slack")));
        assert!(!ElectronProvider::slack().matches(&app("com.hnc.Discord")));
        assert!(ElectronProvider::discord().matches(&app("com.hnc.Discord")));
        assert!(ElectronProvider::discord().matches(&app("discord")));
    }

    #[test]
    fn manifest_lists_token_cookies_and_keychain() {
        let home = fake_home("Slack");
        let provider = ElectronProvider::slack().with_home(home.path());
        let m = provider
            .manifest(&app("Slack"), None, TransferScope::FullProfile)
            .unwrap();
        let labels: Vec<&str> = m.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Local storage (auth token)"), "{labels:?}");
        assert!(labels.contains(&"Cookies and session"), "{labels:?}");
        assert!(
            labels.contains(&"App encryption key (Keychain)"),
            "{labels:?}"
        );
        // Local Storage is checked by default; IndexedDB is opt-in.
        let idb = m
            .items
            .iter()
            .find(|i| i.label == "Cached content")
            .unwrap();
        assert!(!idb.default_checked);
    }

    #[test]
    fn full_export_packs_the_profile_under_the_electron_prefix() {
        let home = fake_home("discord");
        let provider = ElectronProvider::discord()
            .with_home(home.path())
            .without_keychain();

        let mut bundle = Vec::new();
        provider
            .export(&app("discord"), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "discord");
        let entries = reader.read_all().unwrap();
        let cookies = entries
            .iter()
            .find(|e| e.rel_path == "electron/Cookies")
            .unwrap();
        assert_eq!(cookies.bytes, b"SQLite format 3\0cookies");
        assert!(
            entries
                .iter()
                .any(|e| e.rel_path == "electron/Local Storage/leveldb/000003.log")
        );
        assert!(entries.iter().any(|e| e.rel_path == "electron/Local State"));
        // Keychain reads were disabled, so no keychain entry rides along.
        assert!(
            !entries
                .iter()
                .any(|e| e.rel_path == keychain::KEYCHAIN_ENTRY)
        );
    }

    /// The Safe Storage key is read through the injected host only and packed
    /// as the reserved keychain entry, trusting the app's bundle on the
    /// destination. The sensitive export asked the (fake) host for
    /// authorization exactly once.
    #[test]
    fn keychain_items_are_read_through_the_injected_host() {
        use crate::host::{EffectKind, FakeHost, HostOutput};

        let home = fake_home("Slack");
        let host = Arc::new(FakeHost::new().with_responder(|command| {
            Ok(match command.kind {
                EffectKind::KeychainRead if command.args.iter().any(|a| a == "-w") => {
                    HostOutput::ok("safe-storage-key\n")
                }
                EffectKind::KeychainRead => HostOutput::ok("    \"acct\"<blob>=\"Slack Key\"\n"),
                _ => HostOutput::failed(),
            })
        }));
        let provider = ElectronProvider::slack()
            .with_home(home.path())
            .with_host(host.clone());
        let mut bundle = Vec::new();
        provider
            .export(&app("Slack"), TransferScope::FullProfile, &mut bundle)
            .unwrap();
        assert_eq!(host.authorizations().len(), 1);
        let entries = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .read_all()
            .unwrap();
        let keychain_entry = entries
            .iter()
            .find(|e| e.rel_path == keychain::KEYCHAIN_ENTRY);
        if cfg!(target_os = "macos") {
            let items = cua_teleport_bundle::keychain::parse(&keychain_entry.unwrap().bytes);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].service, "Slack Safe Storage");
            assert_eq!(items[0].account, "Slack Key");
            assert_eq!(items[0].secret, b"safe-storage-key");
            assert_eq!(
                items[0].trust_app.as_deref(),
                Some("/Applications/Slack.app")
            );
            assert!(host.calls().iter().all(|c| c.program == "security"));
        } else {
            // Keychain reads are macOS-only.
            assert!(keychain_entry.is_none());
            assert!(host.calls().is_empty());
        }
    }

    /// A denied authorization aborts a sensitive export before anything is
    /// written, and the denial comes from the injected host.
    #[test]
    fn denied_authorization_aborts_sensitive_export() {
        use crate::host::FakeHost;

        let home = fake_home("Slack");
        let provider = ElectronProvider::slack()
            .with_home(home.path())
            .without_keychain()
            .with_host(Arc::new(FakeHost::new().denying_authorization()));
        let mut bundle = Vec::new();
        let error = provider
            .export(&app("Slack"), TransferScope::FullProfile, &mut bundle)
            .expect_err("denied authorization must abort");
        assert!(error.to_string().contains("not authorized"), "{error}");
        assert!(bundle.is_empty());
    }

    #[test]
    fn selected_export_limits_to_checked() {
        let home = fake_home("Slack");
        let provider = ElectronProvider::slack()
            .with_home(home.path())
            .without_keychain();
        let include: HashSet<String> = [rel("Cookies")].into_iter().collect();
        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app("Slack"),
                TransferScope::FullProfile,
                Some(&include),
                &mut bundle,
            )
            .unwrap();
        let paths: Vec<String> = BundleReader::open(Cursor::new(bundle))
            .unwrap()
            .header()
            .entries
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(
            paths,
            vec![rel("Cookies")],
            "only checked cookies: {paths:?}"
        );
    }
}
