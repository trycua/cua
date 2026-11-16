// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

//! What macOS privacy settings do to a teleport, said in plain words before
//! it fails.
//!
//! Reading a browser's files can need Full Disk Access, and reading its open
//! tabs needs Automation. Neither can be granted by Cua: macOS only lets the
//! person grant them, and applies Full Disk Access to a process when it
//! starts. This module only *looks*: it opens the same files a teleport
//! reads (no content is read, nothing is decrypted) and turns the operating
//! system's refusal into a sentence with a next step. It never reads around a
//! refusal.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Whether a read is allowed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    /// Allowed (or not needed).
    Granted,
    /// macOS refused it.
    Denied,
    /// Cannot be known without asking (Automation is only decided when it is
    /// first used).
    #[default]
    Unknown,
}

/// One browser profile, as the picker shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileInfo {
    /// The directory name (`Default`, `Profile 1`): what `--profile` takes.
    pub dir: String,
    /// The name the person gave it (`Work`), or the directory name.
    pub name: String,
    /// The account signed in to it (an email), when the browser knows.
    #[serde(default)]
    pub account: Option<String>,
    /// The profile the browser used last.
    #[serde(default)]
    pub last_used: bool,
}

/// The profiles under a Chromium-family user-data directory, `Default` first
/// then the rest by name, with the names and accounts from `Local State` and
/// the last-used one marked. `Local State` holds no secret (names, emails),
/// and a missing or unreadable one just leaves the directory names.
pub fn browser_profiles(user_data_dir: &Path) -> Vec<ProfileInfo> {
    let state: serde_json::Value = std::fs::read(user_data_dir.join("Local State"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let cache = state.pointer("/profile/info_cache");
    let last_used = state
        .pointer("/profile/last_used")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let mut dirs: Vec<String> = std::fs::read_dir(user_data_dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("Preferences").is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    dirs.sort_by_key(|n| (n != "Default", n.clone()));
    let text = |dir: &str, key: &str| {
        cache
            .and_then(|c| c.get(dir))
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    dirs.into_iter()
        .map(|dir| ProfileInfo {
            name: text(&dir, "name").unwrap_or_else(|| dir.clone()),
            account: text(&dir, "user_name"),
            last_used: last_used.as_deref() == Some(dir.as_str()),
            dir,
        })
        .collect()
}

/// How reading a profile's files went when only looked at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// The cookie store can be opened.
    Readable,
    /// macOS refused (Operation not permitted).
    Denied,
    /// There is no such store (a wrong or empty profile).
    Missing,
    /// Something else went wrong.
    Other(String),
}

/// Opens (does not read) the profile directory and its cookie store.
pub fn probe_profile(profile_dir: &Path) -> Probe {
    if let Err(e) = std::fs::read_dir(profile_dir) {
        return match e.kind() {
            std::io::ErrorKind::PermissionDenied => Probe::Denied,
            std::io::ErrorKind::NotFound => Probe::Missing,
            _ => Probe::Other(e.to_string()),
        };
    }
    let store = cua_teleport_bundle::layout::chrome::cookies_store(profile_dir);
    match std::fs::File::open(&store) {
        Ok(_) => Probe::Readable,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Probe::Denied,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Probe::Missing,
        Err(e) => Probe::Other(e.to_string()),
    }
}

/// What to do about Full Disk Access, in words a person can follow.
pub const FULL_DISK_ACCESS_SAY: &str = "macOS is not letting Cua read this browser's files. \
Teleporting sign-ins needs Full Disk Access for Cua: open System Settings, Privacy & Security, \
Full Disk Access, and turn on Cua. Then restart the Cua daemon, because macOS only applies the \
setting to programs that start after it is on. Without it you can still teleport open tabs only.";

/// The Automation step, in plain words.
pub const AUTOMATION_SAY: &str = "macOS is not letting Cua ask the browser for its open tabs. \
Open System Settings, Privacy & Security, Automation, and allow Cua to control the browser. \
Without it you can still teleport signed-in sites, but not open tabs.";

/// The sentence for a profile that has no cookie store.
pub fn missing_profile_say(profile: &str, others: &[ProfileInfo]) -> String {
    if others.is_empty() {
        format!(
            "The browser profile {profile:?} has no sign-in data, and no other profiles were found."
        )
    } else {
        let names: Vec<&str> = others.iter().map(|p| p.dir.as_str()).collect();
        format!(
            "The browser profile {profile:?} has no sign-in data. Profiles found: {}.",
            names.join(", ")
        )
    }
}

/// Turns a raw failure message into a plain sentence when it is a macOS
/// privacy refusal, `None` when it is something else. Covers a refused file
/// read ("Operation not permitted") and a refused Apple event (-1743).
pub fn plain_words(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    if lower.contains("-1743") || lower.contains("not authorized to send apple events") {
        return Some(AUTOMATION_SAY.to_string());
    }
    if lower.contains("operation not permitted") || lower.contains("os error 1)") {
        return Some(FULL_DISK_ACCESS_SAY.to_string());
    }
    None
}

/// The user-data directory a profile directory sits in.
pub fn user_data_dir_of(profile_dir: &Path) -> Option<PathBuf> {
    profile_dir.parent().map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(root: &Path, dir: &str) {
        std::fs::create_dir_all(root.join(dir)).unwrap();
        std::fs::write(root.join(dir).join("Preferences"), "{}").unwrap();
    }

    #[test]
    fn profiles_carry_names_accounts_and_the_last_used_one() {
        let root = tempfile::tempdir().unwrap();
        for d in ["Profile 2", "Default", "Profile 1"] {
            profile(root.path(), d);
        }
        std::fs::create_dir_all(root.path().join("Crashpad")).unwrap();
        std::fs::write(
            root.path().join("Local State"),
            r#"{"profile":{"last_used":"Profile 1","info_cache":{
                "Default":{"name":"Personal","user_name":"me@example.com"},
                "Profile 1":{"name":"Work","user_name":"me@work.example"}}}}"#,
        )
        .unwrap();
        let got = browser_profiles(root.path());
        let dirs: Vec<&str> = got.iter().map(|p| p.dir.as_str()).collect();
        assert_eq!(dirs, ["Default", "Profile 1", "Profile 2"]);
        assert_eq!(got[0].name, "Personal");
        assert_eq!(got[1].account.as_deref(), Some("me@work.example"));
        assert!(got[1].last_used && !got[0].last_used && !got[2].last_used);
        // No Local State entry: the directory name stands in.
        assert_eq!(got[2].name, "Profile 2");
        assert_eq!(got[2].account, None);
    }

    #[test]
    fn a_missing_local_state_leaves_directory_names() {
        let root = tempfile::tempdir().unwrap();
        profile(root.path(), "Default");
        let got = browser_profiles(root.path());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Default");
        assert!(!got[0].last_used);
    }

    #[test]
    fn probing_distinguishes_readable_and_missing_profiles() {
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("Default");
        std::fs::create_dir_all(&p).unwrap();
        assert_eq!(probe_profile(&p), Probe::Missing, "no cookie store yet");
        std::fs::write(p.join("Cookies"), b"x").unwrap();
        assert_eq!(probe_profile(&p), Probe::Readable);
        assert_eq!(probe_profile(&root.path().join("nope")), Probe::Missing);
    }

    #[cfg(unix)]
    #[test]
    fn a_refused_read_is_denied_not_missing() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let p = root.path().join("Default");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("Cookies"), b"x").unwrap();
        std::fs::set_permissions(p.join("Cookies"), std::fs::Permissions::from_mode(0o000))
            .unwrap();
        // Root ignores file modes: nothing is refused, so nothing to check.
        if std::fs::File::open(p.join("Cookies")).is_ok() {
            return;
        }
        assert_eq!(probe_profile(&p), Probe::Denied);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
        assert_eq!(probe_profile(&p), Probe::Denied);
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn privacy_refusals_become_plain_sentences_and_other_errors_do_not() {
        let fda =
            plain_words("copying Cookies failed: Operation not permitted (os error 1)").unwrap();
        assert!(fda.contains("Full Disk Access") && fda.contains("restart the Cua daemon"));
        assert!(fda.contains("open tabs only"));
        let auto = plain_words(
            "Google Chrome is running but its open tabs could not be read: Not authorized to send Apple events to Google Chrome. (-1743)",
        )
        .unwrap();
        assert!(auto.contains("Automation"));
        assert_eq!(plain_words("no such file"), None);
        // Words, not codes, and no middle dot.
        for s in [fda, auto] {
            assert!(!s.contains('\u{b7}') && !s.contains("EPERM"), "{s}");
        }
    }

    #[test]
    fn a_missing_profile_lists_the_others() {
        let others = vec![
            ProfileInfo {
                dir: "Default".into(),
                name: "A".into(),
                ..Default::default()
            },
            ProfileInfo {
                dir: "Profile 1".into(),
                name: "B".into(),
                ..Default::default()
            },
        ];
        let s = missing_profile_say("Profile 9", &others);
        assert!(
            s.contains("Profile 9") && s.contains("Default, Profile 1"),
            "{s}"
        );
        assert!(missing_profile_say("X", &[]).contains("no other profiles"));
    }
}
