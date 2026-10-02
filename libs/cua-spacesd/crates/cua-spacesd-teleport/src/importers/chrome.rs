// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Chrome / Chromium import: land the canonical (Linux) bundle layout in the
//! destination platform's real Chrome user-data dir and relaunch that
//! platform's Chrome, so a macOS guest receives the profile where macOS
//! Chrome reads it (not a Linux `.config` path macOS Chrome ignores).

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use cua_teleport_bundle::bundle::{BundleReader, DEFAULT_MAX_TOTAL_BYTES};
use cua_teleport_bundle::chromium_crypto;
use cua_teleport_bundle::cookies::COOKIES_ENTRY;
use cua_teleport_bundle::layout::chrome::{
    launch_program_for, remap_to_platform, user_data_dir_for, DISPLAY, ID, TABS_JSON,
};
use cua_teleport_bundle::local_storage::LOCAL_STORAGE_ENTRY;
use cua_teleport_bundle::logins::LOGINS_ENTRY;
use cua_teleport_bundle::{LaunchSpec, Platform, WindowRestore};

use super::write_entry;
use crate::host::{default_host, HostEffects};
use crate::{ImportProvider, ImportRecord, Result};

/// `pgrep -f` pattern for every process of the destination's Google Chrome
/// (the main process and its helpers all live inside the bundle).
const MACOS_PROCESS_PATTERN: &str = "Google Chrome.app/Contents/";

/// The Chrome/Chromium importer.
pub struct ChromeImporter {
    max_total_bytes: u64,
    /// Ensuring this destination's own Safe Storage key (never the source's)
    /// goes through this host, like every other Keychain effect.
    host: Arc<dyn HostEffects>,
}

impl Default for ChromeImporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ChromeImporter {
    pub fn new() -> Self {
        Self {
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Set the total-size guard for imported bundles.
    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }
}

impl ImportProvider for ChromeImporter {
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
        let mut reader = BundleReader::open_with_limit(bundle, self.max_total_bytes)?;
        // Stop a running Chrome BEFORE touching its profile (macOS). A Chrome
        // the user already opened in the Space (the usual case once it has been
        // "opened once") holds the Cookies database open, caches the Safe
        // Storage key it read at startup, and would keep serving its old
        // signed-out session: the launch below is `open -a`, which only focuses
        // an app that is already running. Quitting it makes the teleport land,
        // and the relaunch read the key this import installed.
        if platform == Platform::MacOS {
            super::terminate_running(&*self.host, MACOS_PROCESS_PATTERN);
        }
        let mut tab_urls: Vec<String> = Vec::new();
        let mut cookies_entry: Option<Vec<u8>> = None;
        let mut local_storage_entry: Option<Vec<u8>> = None;
        let mut logins_entry: Option<Vec<u8>> = None;

        while let Some(entry) = reader.next_entry()? {
            if entry.rel_path == TABS_JSON {
                tab_urls = serde_json::from_slice(&entry.bytes).unwrap_or_default();
                continue;
            }
            // Decrypted cookies (from the Keyvault): re-encrypted under this
            // destination's OWN Safe Storage key below, never written
            // verbatim like an ordinary entry -- the source's key never
            // reaches this process, so a raw `encrypted_value` copy would be
            // undecryptable here anyway.
            if entry.rel_path == COOKIES_ENTRY {
                cookies_entry = Some(entry.bytes);
                continue;
            }
            // localStorage values: written into the destination's own
            // `Local Storage` LevelDB below (a raw copy of the sender's
            // LevelDB files would replace whatever the destination has).
            // Saved passwords the user ticked: re-encrypted under this
            // destination's own key below, like the cookies.
            if entry.rel_path == LOGINS_ENTRY {
                logins_entry = Some(entry.bytes);
                continue;
            }
            if entry.rel_path == LOCAL_STORAGE_ENTRY {
                local_storage_entry = Some(entry.bytes);
                continue;
            }
            let dest = dest_home.join(remap_to_platform(&entry.rel_path, platform));
            write_entry(&dest, &entry, false, record)?;
        }

        let user_data_dir = dest_home.join(user_data_dir_for(platform));
        if let Some(bytes) = cookies_entry {
            let items = cua_teleport_bundle::cookies::parse(&bytes);
            if !items.is_empty() {
                // Single-profile scope today, matching the sender
                // (`cua_teleport::browser_cookies`): a capture names one
                // profile, always landed at "Default" here. Multi-profile
                // Keyvault capture is tracked as follow-up work, not silently
                // guessed at.
                let profile_dir = user_data_dir.join("Default");
                let service = chromium_crypto::macos_safe_storage_service(ID)
                    .unwrap_or("Chrome Safe Storage");
                crate::cookies::install_cookies(
                    &*self.host,
                    &profile_dir,
                    service,
                    platform,
                    &items,
                    record,
                )?;
            }
        }
        if let Some(bytes) = logins_entry {
            let items = cua_teleport_bundle::logins::parse(&bytes);
            if !items.is_empty() {
                let service = chromium_crypto::macos_safe_storage_service(ID)
                    .unwrap_or("Chrome Safe Storage");
                crate::logins::install_logins(
                    &*self.host,
                    &user_data_dir.join("Default"),
                    service,
                    platform,
                    &items,
                    record,
                )?;
            }
        }
        if let Some(bytes) = local_storage_entry {
            let items = cua_teleport_bundle::local_storage::parse(&bytes);
            if !items.is_empty() {
                // Same single-profile scope as the cookies above. The
                // destination Chrome is not running yet (this import
                // launches it afterwards), so LevelDB's lock is free.
                let store = cua_chromium_storage::store_dir(&user_data_dir.join("Default"));
                let existed = store.is_dir();
                if !existed {
                    record.create_dir_all(&store)?;
                }
                let written =
                    cua_chromium_storage::write(&store, &items, crate::cookies::chrome_now_utc())
                        .map_err(|e| crate::TeleportError::Provider(e.to_string()))?;
                record.local_storage_written(&store, &written);
                // The database files themselves are new when it did not
                // exist: ledger them so a wipe removes them with the keys.
                if !existed {
                    if let Ok(entries) = std::fs::read_dir(&store) {
                        for e in entries.flatten() {
                            record.file_written(&e.path());
                        }
                    }
                }
            }
        }
        // Suppress the first-run/onboarding flow: on a freshly-imported profile it
        // starts its own new-tab session and discards the transferred session that
        // `--restore-last-session` would otherwise reopen.
        let mut args = vec![
            format!("--user-data-dir={}", user_data_dir.display()),
            "--no-first-run".to_string(),
            "--no-default-browser-check".to_string(),
        ];
        if tab_urls.is_empty() {
            args.push("--restore-last-session".to_string());
        } else {
            args.extend(tab_urls.iter().cloned());
        }

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

    fn profile_bundle(tabs: &[u8]) -> Vec<u8> {
        bundle(
            "chrome",
            &[
                ("tabs.json", 0o644, tabs),
                (
                    ".config/google-chrome/Default/Cookies",
                    0o600,
                    b"cookie-bytes",
                ),
                (
                    ".config/google-chrome/Default/Preferences",
                    0o600,
                    b"{\"profile\":1}",
                ),
                (
                    ".config/google-chrome/Default/Bookmarks",
                    0o600,
                    b"{\"roots\":{}}",
                ),
                (
                    ".config/google-chrome/Default/Local Storage/leveldb.log",
                    0o600,
                    b"ls-bytes",
                ),
            ],
        )
    }

    #[test]
    fn import_lands_in_the_linux_profile_and_restores_the_session() {
        let dest = tempfile::tempdir().unwrap();
        let spec = ChromeImporter::new()
            .import_to(
                &mut Cursor::new(profile_bundle(b"[]")),
                dest.path(),
                Platform::Linux,
            )
            .unwrap();
        let profile_root = dest.path().join(".config/google-chrome/Default");
        assert_eq!(
            std::fs::read(profile_root.join("Cookies")).unwrap(),
            b"cookie-bytes"
        );
        assert_eq!(
            std::fs::read(profile_root.join("Local Storage/leveldb.log")).unwrap(),
            b"ls-bytes"
        );
        // tabs.json is consumed, not written.
        assert!(!dest.path().join("tabs.json").exists());
        assert_eq!(spec.program, "google-chrome");
        assert!(spec
            .args
            .iter()
            .any(|arg| arg.starts_with("--user-data-dir=")));
        // No tabs -> restore-last-session fallback.
        assert!(spec.args.iter().any(|arg| arg == "--restore-last-session"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(profile_root.join("Cookies"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn import_lands_in_the_macos_chrome_profile() {
        // A macOS guest must receive the profile where macOS Chrome actually
        // reads it (~/Library/Application Support/Google/Chrome), NOT the Linux
        // .config path that macOS Chrome ignores — the teleport-into-Lume bug.
        let dest = tempfile::tempdir().unwrap();
        let spec = ChromeImporter::new()
            .import_to(
                &mut Cursor::new(profile_bundle(
                    br#"["https://a.example/","https://b.example/"]"#,
                )),
                dest.path(),
                Platform::MacOS,
            )
            .unwrap();
        let mac_root = dest
            .path()
            .join("Library/Application Support/Google/Chrome/Default");
        assert_eq!(
            std::fs::read(mac_root.join("Bookmarks")).unwrap(),
            b"{\"roots\":{}}"
        );
        assert!(
            !dest.path().join(".config/google-chrome").exists(),
            "must not write the Linux layout on a macOS import"
        );
        assert_eq!(
            spec.program,
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        );
        let uddir = format!(
            "--user-data-dir={}",
            dest.path()
                .join("Library/Application Support/Google/Chrome")
                .display()
        );
        assert!(spec.args.contains(&uddir), "{:?}", spec.args);
        // Tabs reopen as arguments, in order, instead of restore-last-session.
        assert!(!spec.args.iter().any(|a| a == "--restore-last-session"));
        assert_eq!(
            spec.restore_windows[0].urls,
            ["https://a.example/", "https://b.example/"]
        );
    }

    #[test]
    fn tampered_bundle_is_refused_before_anything_lands() {
        let mut bytes = profile_bundle(b"[]");
        let needle = b"cookie-bytes";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .unwrap();
        bytes[pos] ^= 0xff;
        let dest = tempfile::tempdir().unwrap();
        assert!(ChromeImporter::new()
            .import_to(&mut Cursor::new(bytes), dest.path(), Platform::Linux)
            .is_err());
        assert!(!dest
            .path()
            .join(".config/google-chrome/Default/Cookies")
            .exists());
    }

    /// End-to-end through the public `ImportProvider` seam: a bundle whose
    /// `cookies.json` (Keyvault-decrypted, plaintext on the wire inside the
    /// checksummed bundle) carries one cookie lands it, re-encrypted, in the
    /// destination's own `Cookies` database -- readable back with the FIXED
    /// Linux `v10` key a real Linux Chrome derives on its own (no Keychain
    /// involved, so this needs no host scripting to be deterministic).
    #[test]
    fn cookies_entry_is_reencrypted_under_the_destinations_own_key_not_written_verbatim() {
        use cua_teleport_bundle::cookies::CookieItem;

        let dest = tempfile::tempdir().unwrap();
        let profile = dest.path().join(".config/google-chrome/Default");
        std::fs::create_dir_all(&profile).unwrap();
        // A `Cookies` database this guest's Chrome already created (the
        // "after Chrome's first run" precondition `install_cookies`
        // documents): minimal but matches Chrome's real column names.
        {
            let conn = rusqlite::Connection::open(profile.join("Cookies")).unwrap();
            conn.execute_batch(
                "CREATE TABLE cookies (
                    creation_utc INTEGER NOT NULL, host_key TEXT NOT NULL, name TEXT NOT NULL,
                    value TEXT NOT NULL DEFAULT '', encrypted_value BLOB NOT NULL DEFAULT '',
                    path TEXT NOT NULL, expires_utc INTEGER NOT NULL,
                    is_secure INTEGER NOT NULL DEFAULT 0, is_httponly INTEGER NOT NULL DEFAULT 0,
                    samesite INTEGER NOT NULL DEFAULT -1,
                    UNIQUE (host_key, name, path)
                );",
            )
            .unwrap();
        }

        let cookies_json = cua_teleport_bundle::cookies::serialize(&[CookieItem {
            host_key: ".github.com".into(),
            name: "user_session".into(),
            value: b"gh-session-value".to_vec(),
            path: "/".into(),
            expires_utc: 0,
            is_secure: true,
            is_httponly: true,
            samesite: 1,
            extra: Default::default(),
        }]);
        let bundle_bytes = bundle(
            "chrome",
            &[
                ("tabs.json", 0o644, b"[]"),
                (COOKIES_ENTRY, 0o600, &cookies_json),
            ],
        );

        ChromeImporter::new()
            .import_to(&mut Cursor::new(bundle_bytes), dest.path(), Platform::Linux)
            .unwrap();

        // Not written verbatim: the reserved entry never lands as a file.
        assert!(!dest.path().join(COOKIES_ENTRY).exists());

        let conn = rusqlite::Connection::open(profile.join("Cookies")).unwrap();
        let encrypted: Vec<u8> = conn
            .query_row(
                "SELECT encrypted_value FROM cookies WHERE host_key = '.github.com'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(encrypted.starts_with(b"v10"));
        let key = chromium_crypto::derive_key(
            chromium_crypto::LINUX_V10_PASSWORD,
            chromium_crypto::LINUX_V10_PBKDF2_ROUNDS,
        );
        let plain = chromium_crypto::decrypt_prefixed(&key, &encrypted)
            .unwrap()
            .1;
        assert_eq!(&*plain, b"gh-session-value");
    }

    /// localStorage from the Keyvault lands in the destination's own
    /// `Local Storage` LevelDB, beside what that browser already held, and a
    /// wipe takes out exactly the keys the import wrote.
    #[test]
    fn local_storage_is_written_into_the_destinations_leveldb_and_wiped_by_key() {
        use cua_teleport_bundle::local_storage::{serialize, LocalStorageItem};
        let dest = tempfile::tempdir().unwrap();
        let store = dest
            .path()
            .join("Library/Application Support/Google/Chrome/Default/Local Storage/leveldb");
        // The destination already has a value of its own.
        let own = LocalStorageItem {
            origin: "https://mine.example".into(),
            key: "keep".into(),
            value: "me".into(),
            key_raw: None,
            value_raw: None,
        };
        cua_chromium_storage::write(&store, &[own], 1).unwrap();
        let item = |origin: &str, key: &str, value: &str| LocalStorageItem {
            origin: origin.into(),
            key: key.into(),
            value: value.into(),
            key_raw: None,
            value_raw: None,
        };
        let json = serialize(&[
            item("https://github.com", "color_mode", "dark"),
            item("https://github.com", "名前", "値"),
        ]);
        let bundle_bytes = bundle(
            "chrome",
            &[
                ("tabs.json", 0o644, b"[]"),
                (LOCAL_STORAGE_ENTRY, 0o600, &json),
            ],
        );
        let mut record = ImportRecord::default();
        ChromeImporter::new()
            .import_recorded(
                &mut Cursor::new(bundle_bytes),
                dest.path(),
                Platform::MacOS,
                &mut record,
            )
            .unwrap();
        // Not written as a file, and the sender's names are the destination's.
        assert!(!dest.path().join(LOCAL_STORAGE_ENTRY).exists());
        let mut got = cua_chromium_storage::read(&store).unwrap();
        got.sort_by(|a, b| (&a.origin, &a.key).cmp(&(&b.origin, &b.key)));
        let names: Vec<(&str, &str, &str)> = got
            .iter()
            .map(|i| (i.origin.as_str(), i.key.as_str(), i.value.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("https://github.com", "color_mode", "dark"),
                ("https://github.com", "名前", "値"),
                ("https://mine.example", "keep", "me"),
            ]
        );
        // Wipe removes the import's keys and leaves the browser's own.
        let ledger = crate::ledger::Ledger::new("i1", "chrome", &record, 0, 1);
        let report = crate::ledger::wipe(&ledger, dest.path(), &crate::host::FakeHost::new());
        assert!(report.complete(), "{:?}", report.errors);
        assert_eq!(
            report.local_storage_keys_removed, 3,
            "two values and the new origin META"
        );
        let left = cua_chromium_storage::read(&store).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].origin, "https://mine.example");
        // A second wipe finds nothing to remove.
        let again = crate::ledger::wipe(&ledger, dest.path(), &crate::host::FakeHost::new());
        assert_eq!(again.local_storage_keys_removed, 0);
    }

    /// A fresh destination (Chrome never launched) gets a new store, and a
    /// wipe removes the files the import created with it.
    #[test]
    fn local_storage_creates_the_store_for_a_never_launched_browser() {
        use cua_teleport_bundle::local_storage::{serialize, LocalStorageItem};
        let dest = tempfile::tempdir().unwrap();
        let json = serialize(&[LocalStorageItem {
            origin: "https://github.com".into(),
            key: "k".into(),
            value: "v".into(),
            key_raw: None,
            value_raw: None,
        }]);
        let bundle_bytes = bundle(
            "chrome",
            &[
                ("tabs.json", 0o644, b"[]"),
                (LOCAL_STORAGE_ENTRY, 0o600, &json),
            ],
        );
        let mut record = ImportRecord::default();
        ChromeImporter::new()
            .import_recorded(
                &mut Cursor::new(bundle_bytes),
                dest.path(),
                Platform::Linux,
                &mut record,
            )
            .unwrap();
        let store = dest
            .path()
            .join(".config/google-chrome/Default/Local Storage/leveldb");
        assert_eq!(cua_chromium_storage::read(&store).unwrap().len(), 1);
        let ledger = crate::ledger::Ledger::new("i2", "chrome", &record, 0, 1);
        let report = crate::ledger::wipe(&ledger, dest.path(), &crate::host::FakeHost::new());
        assert!(report.complete(), "{:?}", report.errors);
        assert!(!store.exists(), "the store the import created is gone");
    }
}
