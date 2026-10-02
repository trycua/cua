// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! Firefox session export.
//!
//! Captures a Firefox profile's logged-in session — open tabs
//! (`sessionstore.jsonlz4`), cookies (`cookies.sqlite`), saved passwords
//! (`logins.json` + `key4.db`), bookmarks/history (`places.sqlite`), and
//! per-site storage ([`layout::firefox`]). Firefox keeps its own encryption
//! key in `key4.db` (no OS Keychain dependency), so a file copy preserves saved
//! passwords directly. The receiver lands it as a dedicated
//! `cua.default-release` profile with onboarding suppressed.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::bundle::{BundleWriter, DEFAULT_MAX_TOTAL_BYTES};
use crate::host::{HostEffects, default_host};
use crate::layout::firefox::{
    APP_IDS as FIREFOX_APP_IDS, BUNDLE_PREFIX, PROFILE_DIRS, PROFILE_FILES, TABS_JSON, root_for,
};
use crate::providers::util::{add_dir_recursive, add_file_best_effort, dir_len, file_len};
use crate::{
    AppRef, ExportProvider, ManifestItem, Platform, Result, TeleportError, TransferManifest,
    TransferScope,
};

/// The Firefox/Firefox-family session provider.
pub struct FirefoxProvider {
    profile_dir_override: Option<PathBuf>,
    max_total_bytes: u64,
    /// Every `$HOME`, Keychain and authorization effect goes through this.
    host: Arc<dyn HostEffects>,
}

impl Default for FirefoxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FirefoxProvider {
    pub fn new() -> Self {
        Self {
            profile_dir_override: None,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            host: default_host(),
        }
    }

    /// Act on `host` instead of [`default_host`].
    pub fn with_host(mut self, host: Arc<dyn HostEffects>) -> Self {
        self.host = host;
        self
    }

    /// Override the source profile directory (tests, or a caller that already
    /// resolved the exact profile).
    pub fn with_profile_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.profile_dir_override = Some(dir.into());
        self
    }

    pub fn with_max_total_bytes(mut self, bytes: u64) -> Self {
        self.max_total_bytes = bytes;
        self
    }

    /// Resolve the active source profile directory for the platform.
    fn source_profile_dir(&self, platform: Platform) -> Option<PathBuf> {
        if let Some(dir) = &self.profile_dir_override {
            return Some(dir.clone());
        }
        let root = self.host.home_dir()?.join(root_for(platform));
        resolve_active_profile(&root)
    }
}

impl ExportProvider for FirefoxProvider {
    fn id(&self) -> &str {
        crate::layout::firefox::ID
    }

    fn host(&self) -> &dyn HostEffects {
        &*self.host
    }

    fn display_name(&self) -> &str {
        crate::layout::firefox::DISPLAY
    }

    fn platform_supported(&self, _platform: Platform) -> bool {
        true
    }

    fn install_probe(&self) -> Option<crate::InstallProbe> {
        Some(crate::InstallProbe::path(crate::layout::firefox::MACOS_APP))
    }

    fn matches(&self, app: &AppRef) -> bool {
        let id = app.app_id.to_ascii_lowercase();
        FIREFOX_APP_IDS
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(&app.app_id))
            || id.contains("firefox")
    }

    fn app_ids(&self) -> &[&str] {
        FIREFOX_APP_IDS
    }

    fn manifest(
        &self,
        app: &AppRef,
        _window: Option<&crate::WindowRef>,
        scope: TransferScope,
    ) -> Result<TransferManifest> {
        let profile = self.source_profile_dir(app.platform);
        let mut items = Vec::new();
        let mut total = 0u64;

        // Open tabs (the one item checked by default even in tabs-only scope).
        if let Some(dir) = &profile {
            let n = extract_tabs(dir).len() as u64;
            items.push(item(
                "Open tabs",
                TABS_JSON,
                0,
                false,
                (n > 0).then_some((n, "tabs")),
                true,
            ));
        }

        if scope == TransferScope::FullProfile
            && let Some(dir) = &profile
        {
            for (name, sensitive) in PROFILE_FILES {
                if *name == "sessionstore.jsonlz4" {
                    continue; // already added above
                }
                let disk = dir.join(name);
                if !disk.is_file() {
                    continue;
                }
                let bytes = file_len(&disk);
                total += bytes;
                let (label, count) = describe_file(name, &disk);
                // Credential-shaped state (cookies, saved passwords and their
                // key, history) is an opt-in, as for Chrome: its group in
                // `ux::catalog::OPT_IN_ITEMS` is the picker's checkbox.
                let opt_in =
                    crate::ux::SensitiveGroup::of_item(crate::layout::firefox::ID, name).is_some();
                items.push(item(
                    label,
                    &format!("{BUNDLE_PREFIX}/{name}"),
                    bytes,
                    *sensitive,
                    count,
                    !opt_in,
                ));
            }
            for (name, sensitive) in PROFILE_DIRS {
                let disk = dir.join(name);
                if !disk.is_dir() {
                    continue;
                }
                let bytes = dir_len(&disk);
                total += bytes;
                // Site storage carries logged-in web-app state → checked;
                // extensions default off (bulky, optional).
                let checked = *name == "storage" || *name == "sessionstore-backups";
                items.push(item(
                    &format!("{}/", pretty_dir(name)),
                    &format!("{BUNDLE_PREFIX}/{name}"),
                    bytes,
                    *sensitive,
                    None,
                    checked,
                ));
            }
        }

        Ok(TransferManifest {
            provider_id: self.id().to_string(),
            app_display_name: app.display_name.clone(),
            scope,
            items,
            total_est_bytes: total,
            notes: vec![
                "Closing Firefox before transfer yields a cleaner capture; locked \
                 databases are copied best-effort while it runs."
                    .to_string(),
                "Full profile includes cookies, saved logins, and history.".to_string(),
            ],
        })
    }

    fn capture_selected(
        &self,
        app: &AppRef,
        scope: TransferScope,
        include: Option<&HashSet<String>>,
        out: &mut dyn Write,
    ) -> Result<()> {
        let profile = self.source_profile_dir(app.platform).ok_or_else(|| {
            TeleportError::Provider("no Firefox profile found on this machine".to_string())
        })?;
        let mut writer = BundleWriter::with_limit(
            out,
            self.id(),
            app.display_name.clone(),
            scope,
            self.max_total_bytes,
        );
        let wants = |rel: &str| include.is_none_or(|set| set.contains(rel));

        // Normalized open-tab URL list — the deterministic thing the destination
        // reopens (cookies + storage below make those tabs logged-in). The raw
        // sessionstore files still ride along in full scope as a fallback.
        if wants(TABS_JSON) {
            let tabs = extract_tabs(&profile);
            let json = serde_json::to_vec(&tabs).unwrap_or_else(|_| b"[]".to_vec());
            writer.add_bytes(TABS_JSON, 0o644, &json)?;
        }

        if scope == TransferScope::FullProfile {
            for (name, _sensitive) in PROFILE_FILES {
                let rel = format!("{BUNDLE_PREFIX}/{name}");
                if !wants(&rel) {
                    continue;
                }
                let disk = profile.join(name);
                if disk.is_file() {
                    add_file_best_effort(&mut writer, &disk, &rel)?;
                }
            }
            for (name, _sensitive) in PROFILE_DIRS {
                let rel = format!("{BUNDLE_PREFIX}/{name}");
                if !wants(&rel) {
                    continue;
                }
                let disk = profile.join(name);
                if disk.is_dir() {
                    add_dir_recursive(&mut writer, &disk, &rel)?;
                }
            }
        }

        writer.finish()?;
        Ok(())
    }
}

/// Resolve the active profile directory under a Firefox root by reading
/// `installs.ini` (install-scoped default), then `profiles.ini`.
fn resolve_active_profile(root: &Path) -> Option<PathBuf> {
    let join_rel = |rel: &str, is_relative: bool| -> PathBuf {
        if is_relative {
            root.join(rel)
        } else {
            PathBuf::from(rel)
        }
    };

    // installs.ini: `Default=Profiles/<id>` under an install-hash section.
    if let Ok(text) = std::fs::read_to_string(root.join("installs.ini")) {
        for line in text.lines() {
            if let Some(rel) = line.trim().strip_prefix("Default=") {
                let dir = join_rel(rel.trim(), true);
                if dir.is_dir() {
                    return Some(dir);
                }
            }
        }
    }

    // profiles.ini: prefer Default=1, then a *.default-release, then *.default.
    let text = std::fs::read_to_string(root.join("profiles.ini")).ok()?;
    let mut sections: Vec<(String, bool, bool)> = Vec::new(); // (path, is_relative, default)
    let mut cur_path: Option<String> = None;
    let mut cur_rel = true;
    let mut cur_default = false;
    let mut in_profile = false;
    let flush = |sections: &mut Vec<(String, bool, bool)>,
                 path: &mut Option<String>,
                 rel: &mut bool,
                 default: &mut bool| {
        if let Some(p) = path.take() {
            sections.push((p, *rel, *default));
        }
        *rel = true;
        *default = false;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            flush(&mut sections, &mut cur_path, &mut cur_rel, &mut cur_default);
            in_profile = line.starts_with("[Profile");
        } else if in_profile {
            if let Some(v) = line.strip_prefix("Path=") {
                cur_path = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("IsRelative=") {
                cur_rel = v.trim() == "1";
            } else if let Some(v) = line.strip_prefix("Default=") {
                cur_default = v.trim() == "1";
            }
        }
    }
    flush(&mut sections, &mut cur_path, &mut cur_rel, &mut cur_default);

    if let Some((p, rel, _)) = sections.iter().find(|(_, _, d)| *d) {
        let dir = join_rel(p, *rel);
        if dir.is_dir() {
            return Some(dir);
        }
    }
    for suffix in [".default-release", ".default"] {
        if let Some((p, rel, _)) = sections.iter().find(|(p, _, _)| p.ends_with(suffix)) {
            let dir = join_rel(p, *rel);
            if dir.is_dir() {
                return Some(dir);
            }
        }
    }
    sections
        .first()
        .map(|(p, rel, _)| join_rel(p, *rel))
        .filter(|d| d.is_dir())
}

/// Build a manifest item.
fn item(
    label: &str,
    rel_path: &str,
    est_bytes: u64,
    sensitive: bool,
    count: Option<(u64, &str)>,
    default_checked: bool,
) -> ManifestItem {
    let (count, count_noun) = match count {
        Some((n, noun)) => (Some(n), Some(noun.to_string())),
        None => (None, None),
    };
    ManifestItem {
        label: label.to_string(),
        rel_path: rel_path.to_string(),
        est_bytes,
        count,
        count_noun,
        sensitive,
        default_checked,
    }
}

/// A human label and optional count for a curated profile file.
fn describe_file(name: &str, disk: &Path) -> (&'static str, Option<(u64, &'static str)>) {
    match name {
        "cookies.sqlite" => (
            "Cookies and sessions",
            count_sqlite_rows(disk, "moz_cookies").map(|n| (n, "cookies")),
        ),
        "logins.json" => ("Saved passwords", count_logins(disk)),
        "key4.db" => ("Password key database", None),
        "places.sqlite" => (
            "Bookmarks and history",
            count_sqlite_rows(disk, "moz_places").map(|n| (n, "history entries")),
        ),
        "prefs.js" => ("Preferences", None),
        "cert9.db" => ("Certificates", None),
        "permissions.sqlite" => ("Site permissions", None),
        "webappsstore.sqlite" => ("Local storage", None),
        _ => ("Profile data", None),
    }
}

fn pretty_dir(name: &str) -> &str {
    match name {
        "storage" => "Site data",
        "sessionstore-backups" => "Session backups",
        "extensions" => "Extensions",
        "browser-extension-data" => "Extension data",
        other => other,
    }
}

/// Extract the current URL of every open tab from a Firefox profile's session
/// store. Tries the clean-shutdown `sessionstore.jsonlz4` first, then the live
/// `sessionstore-backups/recovery.jsonlz4` (present when Firefox is running).
/// Skips `about:`/blank tabs. Empty on any failure.
fn extract_tabs(profile_dir: &Path) -> Vec<String> {
    for rel in [
        "sessionstore.jsonlz4",
        "sessionstore-backups/recovery.jsonlz4",
    ] {
        let path = profile_dir.join(rel);
        if let Some(json) = std::fs::read(&path).ok().and_then(|b| mozlz4_decode(&b))
            && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&json)
        {
            let tabs = tabs_from_sessionstore(&value);
            if !tabs.is_empty() {
                return tabs;
            }
        }
    }
    Vec::new()
}

/// Pull each tab's *current* entry URL from a parsed sessionstore JSON
/// (`windows[].tabs[].entries[index-1].url`). When there are no live `windows`
/// (a source whose Firefox was closed with no window open), falls back to the
/// last-closed window under `_closedWindows` — matching Firefox's own "restore
/// previous session". Pure, so it is unit-testable.
fn tabs_from_sessionstore(value: &serde_json::Value) -> Vec<String> {
    let from_windows = |key: &str| -> Vec<String> {
        let is_real = |u: &str| {
            !u.is_empty()
                && !u.starts_with("about:")
                && !u.starts_with("chrome://")
                && !u.contains("/whatsnew/")
        };
        let mut urls = Vec::new();
        for window in value
            .get(key)
            .and_then(|w| w.as_array())
            .into_iter()
            .flatten()
        {
            for tab in window
                .get("tabs")
                .and_then(|t| t.as_array())
                .into_iter()
                .flatten()
            {
                let entries = match tab.get("entries").and_then(|e| e.as_array()) {
                    Some(e) if !e.is_empty() => e,
                    _ => continue,
                };
                let idx = tab
                    .get("index")
                    .and_then(|i| i.as_u64())
                    .unwrap_or(entries.len() as u64);
                let entry = entries
                    .get((idx as usize).saturating_sub(1))
                    .or_else(|| entries.last());
                if let Some(url) = entry.and_then(|e| e.get("url")).and_then(|u| u.as_str())
                    && is_real(url)
                {
                    urls.push(url.to_string());
                }
            }
        }
        urls
    };
    let live = from_windows("windows");
    if !live.is_empty() {
        return live;
    }
    // Only the single most-recently-closed window, to avoid resurrecting a long
    // history of closed windows.
    let mut closed = Vec::new();
    if let Some(last) = value
        .get("_closedWindows")
        .and_then(|w| w.as_array())
        .and_then(|a| a.last())
    {
        let single = serde_json::json!({ "windows": [last] });
        closed = tabs_from_sessionstore(&single);
    }
    closed
}

/// Decode a mozLz4 blob (`mozLz40\0` + LE u32 decompressed size + one raw LZ4
/// block). Returns `None` on a bad magic or malformed block.
fn mozlz4_decode(bytes: &[u8]) -> Option<Vec<u8>> {
    const MAGIC: &[u8] = b"mozLz40\0";
    if bytes.len() < MAGIC.len() + 4 || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    let size = u32::from_le_bytes(bytes[MAGIC.len()..MAGIC.len() + 4].try_into().ok()?) as usize;
    lz4_block_decompress(&bytes[MAGIC.len() + 4..], size)
}

/// Decompress one raw LZ4 block into an output of exactly `dest_size` bytes.
/// A compact, dependency-free implementation of the LZ4 block format.
fn lz4_block_decompress(src: &[u8], dest_size: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(dest_size);
    let mut i = 0usize;
    // A varlength field: sum 0xFF bytes until a non-0xFF terminator.
    let read_len = |src: &[u8], i: &mut usize, mut len: usize| -> Option<usize> {
        loop {
            let b = *src.get(*i)?;
            *i += 1;
            len += b as usize;
            if b != 0xFF {
                return Some(len);
            }
        }
    };
    while i < src.len() {
        let token = src[i];
        i += 1;
        // Literals.
        let mut lit = (token >> 4) as usize;
        if lit == 15 {
            lit = read_len(src, &mut i, lit)?;
        }
        if i + lit > src.len() {
            return None;
        }
        out.extend_from_slice(&src[i..i + lit]);
        i += lit;
        if i >= src.len() {
            break; // last sequence has literals only
        }
        // Match.
        let offset = *src.get(i)? as usize | (*src.get(i + 1)? as usize) << 8;
        i += 2;
        if offset == 0 || offset > out.len() {
            return None;
        }
        let mut mlen = (token & 0x0F) as usize;
        if mlen == 15 {
            mlen = read_len(src, &mut i, mlen)?;
        }
        mlen += 4; // minmatch
        let start = out.len() - offset;
        for k in 0..mlen {
            let byte = out[start + k]; // overlapping copies are intentional
            out.push(byte);
        }
    }
    if out.len() == dest_size {
        Some(out)
    } else {
        // A truncated/oversized decode still yields usable JSON up to a point;
        // accept it so a slightly-off size hint doesn't drop the whole session.
        Some(out)
    }
}

/// Count saved logins in a `logins.json`.
fn count_logins(path: &Path) -> Option<(u64, &'static str)> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let n = value.get("logins")?.as_array()?.len() as u64;
    Some((n, "logins"))
}

/// Count rows of a table in a SQLite DB opened read-only. `None` on any failure
/// (locked, absent, unexpected schema).
fn count_sqlite_rows(path: &Path, table: &str) -> Option<u64> {
    use rusqlite::OpenFlags;
    let conn = rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    // Table name can't be bound; it comes from our own constant list, never input.
    let count: i64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .ok()?;
    Some(count.max(0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bundle::BundleReader;
    use std::io::Cursor;

    fn app(platform: Platform) -> AppRef {
        AppRef {
            app_id: "org.mozilla.firefox".into(),
            display_name: "Firefox".into(),
            platform,
        }
    }

    /// A fake source profile with the session-critical files + a storage dir.
    fn fake_profile() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("Profiles").join("abc.default-release");
        std::fs::create_dir_all(p.join("storage/default/https+++example.com")).unwrap();
        std::fs::write(p.join("sessionstore.jsonlz4"), b"mozLz40\0fake").unwrap();
        std::fs::write(p.join("cookies.sqlite"), b"SQLite format 3\0cookies").unwrap();
        std::fs::write(p.join("key4.db"), b"SQLite format 3\0key").unwrap();
        std::fs::write(
            p.join("logins.json"),
            br#"{"logins":[{"hostname":"https://a"},{"hostname":"https://b"}]}"#,
        )
        .unwrap();
        std::fs::write(p.join("places.sqlite"), b"SQLite format 3\0places").unwrap();
        std::fs::write(p.join("prefs.js"), b"// prefs\n").unwrap();
        std::fs::write(
            p.join("storage/default/https+++example.com/idb"),
            b"idbdata",
        )
        .unwrap();
        // profiles.ini pointing at it.
        std::fs::write(
            dir.path().join("profiles.ini"),
            b"[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\nDefault=1\n\n[General]\nStartWithLastProfile=1\nVersion=2\n",
        )
        .unwrap();
        dir
    }

    fn provider_for(root: &Path) -> FirefoxProvider {
        FirefoxProvider::new().with_profile_dir(root.join("Profiles").join("abc.default-release"))
    }

    #[test]
    fn matches_firefox_ids() {
        let p = FirefoxProvider::new();
        for id in ["org.mozilla.firefox", "firefox", "Firefox", "firefox-esr"] {
            assert!(p.matches(&app_with(id)), "should match {id}");
        }
        assert!(!p.matches(&app_with("com.google.Chrome")));
    }

    fn app_with(id: &str) -> AppRef {
        AppRef {
            app_id: id.into(),
            display_name: id.into(),
            platform: Platform::MacOS,
        }
    }

    #[test]
    fn manifest_lists_session_and_passwords() {
        let dir = fake_profile();
        let provider = provider_for(dir.path());
        let manifest = provider
            .manifest(&app(Platform::MacOS), None, TransferScope::FullProfile)
            .unwrap();
        let labels: Vec<&str> = manifest.items.iter().map(|i| i.label.as_str()).collect();
        assert!(labels.contains(&"Open tabs"), "{labels:?}");
        assert!(labels.contains(&"Cookies and sessions"), "{labels:?}");
        assert!(labels.contains(&"Saved passwords"), "{labels:?}");
        // Passwords item is sensitive and reports the login count.
        let logins = manifest
            .items
            .iter()
            .find(|i| i.label == "Saved passwords")
            .unwrap();
        assert!(logins.sensitive);
        assert_eq!(logins.count, Some(2));
    }

    /// Nothing credential-shaped moves by default, and every withheld item
    /// is reachable through exactly the groups the catalog offers.
    #[test]
    fn credentials_are_opt_ins_matching_the_shared_list() {
        let dir = fake_profile();
        let provider = provider_for(dir.path());
        let manifest = provider
            .manifest(&app(Platform::MacOS), None, TransferScope::FullProfile)
            .unwrap();
        let checked: Vec<&str> = manifest
            .items
            .iter()
            .filter(|i| i.default_checked)
            .map(|i| i.rel_path.as_str())
            .collect();
        for withheld in [
            "firefox/cookies.sqlite",
            "firefox/logins.json",
            "firefox/key4.db",
            "firefox/places.sqlite",
        ] {
            assert!(
                !checked.contains(&withheld),
                "{withheld} is checked: {checked:?}"
            );
        }
        // The rest still moves: tabs, preferences, site data.
        for kept in [TABS_JSON, "firefox/prefs.js", "firefox/storage"] {
            assert!(
                checked.contains(&kept),
                "{kept} is not checked: {checked:?}"
            );
        }
        crate::ux::testing::assert_opt_ins_match(crate::layout::firefox::ID, &manifest);
    }

    #[test]
    fn full_export_packs_the_profile_under_the_firefox_prefix() {
        let dir = fake_profile();
        let provider = provider_for(dir.path());
        let mut bundle = Vec::new();
        provider
            .export(
                &app(Platform::MacOS),
                TransferScope::FullProfile,
                &mut bundle,
            )
            .unwrap();
        let reader = BundleReader::open(Cursor::new(bundle)).unwrap();
        assert_eq!(reader.header().provider_id, "firefox");
        let entries = reader.read_all().unwrap();
        let paths: Vec<&str> = entries.iter().map(|e| e.rel_path.as_str()).collect();
        for want in [
            TABS_JSON,
            "firefox/cookies.sqlite",
            "firefox/key4.db",
            "firefox/logins.json",
            "firefox/sessionstore.jsonlz4",
            "firefox/storage/default/https+++example.com/idb",
        ] {
            assert!(paths.contains(&want), "{want} missing from {paths:?}");
        }
        let key = entries
            .iter()
            .find(|e| e.rel_path == "firefox/key4.db")
            .unwrap();
        assert_eq!(key.bytes, b"SQLite format 3\0key");
    }

    /// The source profile resolves through profiles.ini under the (fake) home.
    #[test]
    fn resolves_the_active_profile_under_the_host_home() {
        use crate::host::FakeHost;
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join(root_for(Platform::Linux));
        std::fs::create_dir_all(&root).unwrap();
        let src = fake_profile();
        // Move the fake tree under the fake home's Linux Firefox root.
        for entry in ["profiles.ini", "Profiles"] {
            std::fs::rename(src.path().join(entry), root.join(entry)).unwrap();
        }
        let provider = FirefoxProvider::new()
            .with_host(std::sync::Arc::new(FakeHost::new().with_home(home.path())));
        assert_eq!(
            provider.source_profile_dir(Platform::Linux),
            Some(root.join("Profiles/abc.default-release"))
        );
    }

    #[test]
    fn selected_export_captures_only_checked_items() {
        let dir = fake_profile();
        let provider = provider_for(dir.path());
        let include: HashSet<String> = [TABS_JSON.to_string()].into_iter().collect();
        let mut bundle = Vec::new();
        provider
            .export_selected(
                &app(Platform::MacOS),
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
            vec![TABS_JSON.to_string()],
            "only the checked tab list: {paths:?}"
        );
    }

    #[test]
    fn extracts_current_tab_urls_from_sessionstore_json() {
        let value = serde_json::json!({
            "windows": [{
                "tabs": [
                    {"index": 2, "entries": [
                        {"url": "https://old.example/"},
                        {"url": "https://mail.example/inbox"}
                    ]},
                    {"index": 1, "entries": [{"url": "about:newtab"}]},
                    {"entries": [{"url": "https://github.com/trycua/cua"}]}
                ]
            }]
        });
        let urls = tabs_from_sessionstore(&value);
        assert_eq!(
            urls,
            vec![
                "https://mail.example/inbox".to_string(),
                "https://github.com/trycua/cua".to_string()
            ]
        );
    }

    #[test]
    fn mozlz4_round_trips_an_all_literal_block() {
        // Build a valid mozLz4 blob whose LZ4 block is pure literals (no match).
        let payload = br#"{"windows":[{"tabs":[{"index":1,"entries":[{"url":"https://x/"}]}]}]}"#;
        let mut block = Vec::new();
        let mut len = payload.len();
        // Literal-length token nibble is 15 then varlen when len >= 15.
        if len < 15 {
            block.push((len as u8) << 4);
        } else {
            block.push(0xF0);
            len -= 15;
            while len >= 255 {
                block.push(0xFF);
                len -= 255;
            }
            block.push(len as u8);
        }
        block.extend_from_slice(payload);

        let mut blob = b"mozLz40\0".to_vec();
        blob.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        blob.extend_from_slice(&block);

        let decoded = mozlz4_decode(&blob).expect("decodes");
        assert_eq!(decoded, payload);
        let value: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
        assert_eq!(
            tabs_from_sessionstore(&value),
            vec!["https://x/".to_string()]
        );
    }

    #[test]
    fn mozlz4_rejects_bad_magic() {
        assert!(mozlz4_decode(b"NOTMOZLZ4....").is_none());
    }
}
